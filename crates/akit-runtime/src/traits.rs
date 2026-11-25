//! The traits akit's data plane is built from: what a listener, matcher, router, middleware,
//! handler, service and upstream offer.
//!
//! They fit akio's callback model. Every method here that takes a request's [`Context`] runs
//! inline on the akio I/O thread that read the request, so it never blocks (rule 1): no disk I/O,
//! no DNS, no waiting on a lock, channel or another thread. A handler that cannot answer at once
//! [detaches](Context::detach) the response and answers later from a completion callback, keeping
//! what it needs alive explicitly (rule 3). What can be built when the config is compiled is built
//! then (rule 12): a middleware wraps its handler once, at compile time, not per request.

use std::sync::Arc;

use akio::AnyAcceptor;
use akio_http::{BodyMode, Context, Request};

/// Answers HTTP requests: an akio [`akio_http::Service`] that is a module of akit.
///
/// [`call`](akio_http::Service::call) runs on the I/O thread and must either respond before it
/// returns or detach the context and respond later; it never blocks. A handler that streams its
/// request body says so in [`body_mode`](akio_http::Service::body_mode).
///
/// This is fit's `module.Handler`, which is fib's handler; here it is akio's `Service`, with room
/// for what akit asks of a handler beyond it (fit's `Unwrapped` and `NeedsAuthentication` become
/// provided methods here when their iterations come).
pub trait Handler: akio_http::Service {
    /// Whether [`call`](akio_http::Service::call) answers every request before it returns, never
    /// detaching it: fit's `module.Inline`. The listener then counts such a request's metrics
    /// once the call returns, without a hook on its response (I37). False unless the handler
    /// says so; a middleware's wrapper says false, as fit's graph takes only a handler with no
    /// middleware around it as inline.
    fn inline(&self) -> bool {
        false
    }
}

/// A [`Handler`] as an akio [`akio_http::Service`], to hand to akio's server
/// ([`akio_http::serve`]) or router.
#[derive(Clone)]
pub struct HandlerService(pub Arc<dyn Handler>);

impl std::fmt::Debug for HandlerService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HandlerService").finish_non_exhaustive()
    }
}

impl akio_http::Service for HandlerService {
    #[inline]
    fn call(&self, ctx: &mut Context) {
        self.0.call(ctx);
    }

    #[inline]
    fn body_mode(&self, req: &Request) -> BodyMode {
        self.0.body_mode(req)
    }

    #[inline]
    fn streams_bodies(&self) -> bool {
        self.0.streams_bodies()
    }
}

/// Wraps the handler that follows it in a route.
///
/// [`wrap`](Self::wrap) is called once, when the route is compiled, so a chain costs nothing to
/// build per request (rule 12). The handler it returns forwards
/// [`body_mode`](akio_http::Service::body_mode) and
/// [`streams_bodies`](akio_http::Service::streams_bodies) to `next` unless the middleware itself
/// decides how the body is read. As fit's `module.Middleware`.
pub trait Middleware: Send + Sync + 'static {
    /// Returns the handler that runs this middleware around `next`.
    fn wrap(&self, next: Arc<dyn Handler>) -> Arc<dyn Handler>;

    /// Whether the handler it returns may change the request's target (its path or query) for
    /// the handlers after it, as `rewrite` and `wasm` do. The access log writes the target the
    /// listener dispatched (fit's `RequestURI`): a route with such a middleware has it kept
    /// for its line, which costs an allocation a request; others read it off the request as it
    /// is when the response goes (akio's response observer). False unless the middleware says so.
    fn changes_target(&self) -> bool {
        false
    }
}

/// Wraps `handler` in `middleware`, the first outermost: a request goes through `middleware` in
/// order, then reaches `handler`. Called when a route is compiled.
pub fn chain(middleware: &[Arc<dyn Middleware>], handler: Arc<dyn Handler>) -> Arc<dyn Handler> {
    middleware
        .iter()
        .rev()
        .fold(handler, |next, mw| mw.wrap(next))
}

/// One of akio's per-request middleware ([`akio_http::Middleware`], as `akio_http::middleware`'s
/// ready-made ones) as an akit [`Middleware`].
pub struct AkioMiddleware<M>(Arc<M>);

impl<M: akio_http::Middleware> AkioMiddleware<M> {
    /// Adapts `mw`.
    pub fn new(mw: M) -> AkioMiddleware<M> {
        AkioMiddleware(Arc::new(mw))
    }
}

impl<M> std::fmt::Debug for AkioMiddleware<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AkioMiddleware").finish_non_exhaustive()
    }
}

impl<M: akio_http::Middleware> Middleware for AkioMiddleware<M> {
    fn wrap(&self, next: Arc<dyn Handler>) -> Arc<dyn Handler> {
        Arc::new(Layered {
            mw: Arc::clone(&self.0),
            next: HandlerService(next),
        })
    }
}

/// An akio middleware around the handler it wraps.
struct Layered<M> {
    mw: Arc<M>,
    next: HandlerService,
}

impl<M: akio_http::Middleware> akio_http::Service for Layered<M> {
    #[inline]
    fn call(&self, ctx: &mut Context) {
        self.mw.handle(ctx, &self.next);
    }

    #[inline]
    fn body_mode(&self, req: &Request) -> BodyMode {
        self.next.body_mode(req)
    }

    #[inline]
    fn streams_bodies(&self) -> bool {
        self.next.streams_bodies()
    }
}

impl<M: akio_http::Middleware> Handler for Layered<M> {}

/// Reports whether a request matches, as fit's `module.Matcher`.
///
/// Matchers run for every request, on the I/O thread, once the request head is read: `matches`
/// is cheap, allocates nothing it can avoid, and never blocks. What it needs (a compiled regex, a
/// parsed CIDR list) is built when the config is compiled.
pub trait Matcher: Send + Sync + 'static {
    /// Whether the request in `ctx` matches.
    fn matches(&self, ctx: &Context) -> bool;
}

/// Finds what serves a request (a route) among those a listener has.
///
/// Built when the config is compiled; [`route`](Self::route) runs per request on the I/O thread
/// and only reads. I11 builds the host and path router on it.
pub trait Router: Send + Sync + 'static {
    /// What a request is routed to.
    type Route: ?Sized;

    /// The route for the request in `ctx`, or `None` when no route matches (a 404).
    fn route(&self, ctx: &Context) -> Option<&Self::Route>;
}

/// One address requests can be sent to, as fit's `module.Upstream`.
///
/// Provisional, as in fit: I14 (upstream pools) and I19 (load balancing) settle what an upstream
/// and a service have to offer.
pub trait Upstream: Send + Sync + 'static {
    /// The upstream's address, as written in the config.
    fn address(&self) -> &str;
}

/// Picks the upstream a request goes to, as fit's `module.Service` (not akio's
/// [`akio_http::Service`], which is a [`Handler`] here).
///
/// Runs per request on the I/O thread: it reads, never blocks, and takes no shared reference
/// count (the upstream is borrowed from the service). Provisional, like [`Upstream`].
pub trait Service: Send + Sync + 'static {
    /// The upstream for the request in `ctx`, or `None` when none is available.
    fn select(&self, ctx: &Context) -> Option<&dyn Upstream>;
}

/// An entry point: a named listener of the config, and the akio acceptor the connections it
/// accepts are opened with.
///
/// Built when the config is compiled; binding it on akio's engine is I05's (and I28's, at run
/// time). Provisional until then.
pub trait Listener: Send + Sync + 'static {
    /// The listener's name in the config.
    fn name(&self) -> &str;

    /// The acceptor akio opens this listener's connections with.
    fn acceptor(&self) -> AnyAcceptor;
}
