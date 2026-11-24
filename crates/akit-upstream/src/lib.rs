#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's upstream pools on akio's clients: load balancing and health checks.
//!
//! I14 gives a [`Pool`] per named upstream of the config (fit's `upstreams`), sending
//! HTTP/1.1 requests to its addresses through akio-http's asynchronous [`Client`], which keeps
//! the connections to each address alive between requests and reuses them. The pool bounds them
//! as fit's does:
//!
//! - **`max_conns`**: at most that many requests are under way to one address, so at most that
//!   many connections to it are open or being opened (an HTTP/1.1 connection carries one request
//!   at a time, and akio's client reuses an idle connection before it dials a new one, and puts a
//!   connection back in its idle pool before it reports the response). A request past it waits
//!   for one to be free, first come first served.
//! - **`max_pending`**: at most that many requests wait for one address; one more fails at once
//!   with [`Error::Busy`] (fit answers it 503).
//! - **`max_idle_conns`** and **`idle_timeout`**: akio's client keeps that many idle connections
//!   per address, each until it has been idle that long.
//! - **`connect_timeout`**: how long one attempt to connect may take: akio's Happy Eyeballs
//!   `connect_timeout`, per address tried, when the address resolves to several; akio dials a
//!   lone one with its engine's `connect_timeout` instead (akio API request A12), which
//!   `akit_listener::engine` sets to the shortest of the upstreams' as a stopgap.
//! - Every exchange is bounded in all, from the request's start, waiting for a connection
//!   included, by [`DEFAULT_RESPONSE_TIMEOUT`], fit's `response_timeout` default (the field
//!   itself comes with I17).
//!
//! I19 adds load balancing ([`balance`]): [`Pool::pick`] chooses the address by the upstream's
//! `load_balancing` and `weights` (round robin, smooth weighted round robin, least connections),
//! each I/O thread keeping its own turn.
//!
//! I20 adds passive health checks ([`health`]), as fit's (nginx's `max_fails` and
//! `fail_timeout`): `max_fails` failures of an address within `fail_timeout` take it out of
//! rotation for `fail_timeout`, and one response from it puts it back; the balancers pass over an
//! address that is out, unless every address is. A failure is what
//! [`Error::is_upstream_failure`] says: the address could not be reached, broke off, sent no
//! valid response, or timed out (waiting for a connection to it included).
//!
//! I35 adds active health checks ([`probe`]), as fit's: with an upstream's `health_check`, each
//! address is probed every `interval` (plus a random `jitter`), by an HTTP `GET` or a TCP
//! connect on akio's clients; `fails` failed probes in a row take it out of rotation until
//! `passes` in a row put it back. Probes start once the pool's engine is there
//! ([`EngineSlot::set`]) and stop when the pool is closed.
//!
//! I16 adds [`Pool::stream`]: an exchange whose request body is written as it is produced and
//! whose response is delivered as it arrives ([`Exchange`], on akio's `Client::request_with`), for
//! the proxy to stream bodies with linked backpressure. It is bounded, balanced and counted
//! towards health as [`Pool::send`] is, and holds its address until the response's body is
//! complete, so that `max_conns` and `least_conn` count a long download as busy for as long as it
//! lasts, as fit's do.
//!
//! Nothing here blocks: [`Pool::send`] runs on an I/O thread, and its callback runs once, on an
//! I/O thread (or the resolver's or the timer's thread, as akio's client says), never blocking
//! either.
//!
//! A pool belongs to the graph its config compiled into (decision 004): when the graph is
//! retired, its cleanup [closes](Pool::close) the pool, which **drains**: requests already sent,
//! and those waiting for a connection, still finish; new ones fail at once with
//! [`Error::Closed`]; the pool is [closed](Pool::is_closed) once the last is done.
//!
//! Pools are made before the engine they dial through exists (`akit run` builds its engine from
//! the compiled graph), so they reach it through an [`EngineSlot`] that `akit run` fills once the
//! engine is built; a pool whose slot is empty, as in `akit validate`, fails every request with
//! [`Error::NoEngine`]. Connections are dialed on the engine's client threads if it has some,
//! else on its last TCP listener's threads: the listeners' own engine serves them.
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

pub mod balance;
pub mod health;
pub mod probe;
mod timer;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use akio::Engine;
use akio_http::Headers;
use akio_http::client::{
    Client, ClientError, ClientOptions, ClientRequest, ClientResponse, RequestBody, ResponseBody,
    ResponseHandler, ResponseHead,
};
use akit_observe::metrics::{HistogramSnapshot, UpstreamStats};

pub use akit_config::{HealthCheck, HealthCheckType};
pub use balance::{Balancer, LoadBalancing};
pub use health::Health;

/// How long a whole exchange may take, from [`Pool::send`] to the end of the response, waiting for
/// a connection included: fit's `response_timeout` default, 1h.
pub const DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(3600);

/// Where the engine pools dial through will be: filled once, by whoever builds the engine,
/// after the pools are made. Cheap to clone; clones share the engine.
#[derive(Clone, Default)]
pub struct EngineSlot(Arc<Slot>);

/// What to run with the engine once a slot is filled.
type OnSet = Box<dyn FnOnce(&Arc<Engine>) + Send>;

#[derive(Default)]
struct Slot {
    engine: OnceLock<Arc<Engine>>,
    /// Run once the engine is there (the pools' probes, I35). One for each pool with a health
    /// check made while the slot was empty: as many as the config has, and each holds its pool
    /// weakly.
    waiting: Mutex<Vec<OnSet>>,
}

impl EngineSlot {
    /// An empty slot.
    #[must_use]
    pub fn new() -> EngineSlot {
        EngineSlot::default()
    }

    /// A slot holding `engine`.
    #[must_use]
    pub fn with(engine: Arc<Engine>) -> EngineSlot {
        let s = EngineSlot::new();
        s.set(engine);
        s
    }

    /// Fills the slot with `engine`, which must be started before requests are sent; false when
    /// it was filled already (the first engine stays).
    pub fn set(&self, engine: Arc<Engine>) -> bool {
        if self.0.engine.set(engine).is_err() {
            return false;
        }
        // Taken under the lock that `when_set` checks the engine under: each runs here or there.
        let waiting = std::mem::take(
            &mut *self
                .0
                .waiting
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        if let Some(e) = self.0.engine.get() {
            for f in waiting {
                f(e);
            }
        }
        true
    }

    /// The engine, once set.
    #[must_use]
    pub fn get(&self) -> Option<&Arc<Engine>> {
        self.0.engine.get()
    }

    /// Runs `f` with the engine: now, on this thread, if the slot is filled; else on the thread
    /// that fills it, when it does.
    fn when_set(&self, f: impl FnOnce(&Arc<Engine>) + Send + 'static) {
        {
            let mut w = self
                .0
                .waiting
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if self.0.engine.get().is_none() {
                w.push(Box::new(f));
                return;
            }
        }
        if let Some(e) = self.0.engine.get() {
            f(e);
        }
    }
}

impl fmt::Debug for EngineSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineSlot")
            .field("set", &self.0.engine.get().is_some())
            .finish()
    }
}

/// Why a request through a pool has no response.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// `max_pending` requests already wait for a connection to the address: fit answers 503.
    /// No failure of the address's.
    Busy,
    /// The pool was closed (its graph retired) before the request was sent: fit answers 503.
    /// No failure of the address's.
    Closed,
    /// The pool has no engine to dial through ([`EngineSlot`] empty), as in `akit validate`.
    NoEngine,
    /// The request's timeout passed while it waited for a connection.
    Timeout,
    /// akio's client failed it: the address could not be reached (`Io`, a connect that was
    /// refused or timed out among them), the connection broke, the response was malformed or too
    /// large, or the exchange timed out.
    Client(ClientError),
}

impl Error {
    /// Whether it counts against the address, as fit's passive health checks count failures:
    /// the address failed it, rather than the pool refusing it.
    #[must_use]
    pub fn is_upstream_failure(&self) -> bool {
        matches!(self, Error::Timeout | Error::Client(_))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Busy => f.write_str("upstream: too many requests wait for a connection"),
            Error::Closed => f.write_str("upstream: the pool is closed"),
            Error::NoEngine => f.write_str("upstream: the pool has no engine to dial through"),
            Error::Timeout => f.write_str("upstream: timed out waiting for a connection"),
            Error::Client(e) => write!(f, "upstream: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Client(e) => Some(e),
            _ => None,
        }
    }
}

/// What a request's callback is given.
pub type Outcome = Result<ClientResponse, Error>;

type Done = Box<dyn FnOnce(Outcome) + Send>;

/// An exchange with an upstream whose bodies stream ([`Pool::stream`]): akio's
/// [`ResponseHandler`], with the pool's errors. Its methods run on an I/O thread (or, for a
/// failure before the request is sent, on the thread that sent it) and must not block. After
/// [`on_head`](Self::on_head), exactly one of [`on_end`](Self::on_end) and
/// [`on_error`](Self::on_error) follows; a request that gets no response gets `on_error` alone.
pub trait Exchange: Send + 'static {
    /// The request's body, when it is streamed ([`ClientRequest::streamed`]), once its head is
    /// written. The default drops it, which fails the request.
    fn on_request_body(&mut self, body: RequestBody) {
        drop(body);
    }
    /// The response head. `body` holds and resumes the reads of the body that follows.
    fn on_head(&mut self, head: ResponseHead, body: Download);
    /// A piece of the body, as read (dechunked); valid for the call only.
    fn on_data(&mut self, chunk: &[u8]);
    /// The body is complete, with the trailer fields of a chunked one.
    fn on_end(&mut self, trailers: &Headers);
    /// The request failed, or (after the head) its body did not complete.
    fn on_error(&mut self, e: Error);
}

/// Holds and resumes the reads of a response's body ([`Exchange::on_head`]), or gives it up.
/// Cheap to clone, usable from any thread; it does nothing once the response is over.
#[derive(Clone)]
pub struct Download {
    body: ResponseBody,
    cancelled: Arc<AtomicBool>,
}

impl Download {
    /// Holds (`true`) or resumes the upstream connection's reads.
    pub fn hold_reads(&self, hold: bool) {
        self.body.hold_reads(hold);
    }

    /// Gives the response up: its connection is closed, and the exchange ends with
    /// [`Exchange::on_error`]. The address is not counted as failing for it: the proxy's client
    /// went, not the upstream.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.body.cancel();
    }
}

impl fmt::Debug for Download {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Download").finish_non_exhaustive()
    }
}

/// Where a request's response goes.
enum Deliver {
    /// Whole, to a callback ([`Pool::send`]).
    Whole(Done),
    /// As it arrives ([`Pool::stream`]).
    Stream(Box<dyn Exchange>),
}

impl Deliver {
    fn fail(self, e: Error) {
        match self {
            Deliver::Whole(done) => done(Err(e)),
            Deliver::Stream(mut x) => x.on_error(e),
        }
    }
}

/// A request waiting for a connection to its address.
struct Waiter {
    req: ClientRequest,
    done: Deliver,
    since: Instant,
}

/// One address of an upstream, and the requests under way to it and waiting for it.
struct Backend {
    address: String,
    /// `http://` and the address: what a request's target is put after.
    base: String,
    /// Requests under way: at most `max_conns`.
    busy: AtomicUsize,
    /// Requests in `queue`, or about to be put there or taken out (see [`Inner::acquire`]).
    waiting: AtomicUsize,
    queue: Mutex<VecDeque<Waiter>>,
    /// Whether it is in rotation, by its failures.
    health: Health,
}

impl Backend {
    fn queue(&self) -> MutexGuard<'_, VecDeque<Waiter>> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes a slot under `max`; false when all are taken.
    fn claim(&self, max: usize) -> bool {
        let mut cur = self.busy.load(Ordering::SeqCst);
        loop {
            if cur >= max {
                return false;
            }
            match self.busy.compare_exchange_weak(
                cur,
                cur.saturating_add(1),
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return true,
                Err(now) => cur = now,
            }
        }
    }
}

struct Inner {
    name: String,
    backends: Box<[Backend]>,
    /// Which backend [`Pool::pick`] gives.
    balancer: Balancer,
    /// `max_fails` and `fail_timeout`.
    max_fails: usize,
    fail_timeout: Duration,
    /// `health_check`: the probes of its addresses ([`probe`]).
    health_check: Option<HealthCheck>,
    /// What the backends' health times count from.
    epoch: Instant,
    max_conns: usize,
    max_pending: usize,
    timeout: Duration,
    opts: ClientOptions,
    engine: EngineSlot,
    /// Made on the first request, once the engine is there.
    client: OnceLock<Client>,
    /// Requests sent and not done, waiting ones included.
    in_flight: AtomicUsize,
    /// Its response times and its backends' counts (I37): its own, or shared with the other
    /// graphs that have the upstream ([`Pool::with_stats`]).
    stats: UpstreamStats,
    closing: AtomicBool,
    closed: AtomicBool,
    on_closed: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
}

/// The connections to one upstream's addresses, and the limits on them. Cheap to clone: clones
/// share the pool.
#[derive(Clone)]
pub struct Pool {
    inner: Arc<Inner>,
}

impl fmt::Debug for Pool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pool")
            .field("name", &self.inner.name)
            .field("addresses", &self.addresses().collect::<Vec<_>>())
            .field("in_flight", &self.in_flight())
            .field("closing", &self.is_closing())
            .finish_non_exhaustive()
    }
}

/// The requests under way to one address, and waiting for it, at one moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct BackendLoad {
    /// Requests under way: each holds a connection, or is opening one.
    pub busy: usize,
    /// Requests waiting for a connection.
    pub waiting: usize,
}

impl Pool {
    /// The pool of the upstream `u` (which akit-config has checked), dialing through the engine
    /// `engine` holds once it is filled. Makes no connection: the first request does.
    #[must_use]
    pub fn new(u: &akit_config::Upstream, engine: EngineSlot) -> Pool {
        let mut opts = ClientOptions::default();
        opts.max_idle_per_host = u.max_idle_conns;
        opts.max_idle = u.max_idle_conns.saturating_mul(u.addresses.len().max(1));
        opts.idle_timeout = Some(u.idle_timeout);
        opts.happy_eyeballs.connect_timeout = u.connect_timeout;
        let weights: Vec<u32> = (0..u.addresses.len()).map(|i| u.weight(i)).collect();
        let backends = u
            .addresses
            .iter()
            .map(|a| Backend {
                address: a.clone(),
                base: format!("http://{a}"),
                busy: AtomicUsize::new(0),
                waiting: AtomicUsize::new(0),
                queue: Mutex::new(VecDeque::new()),
                health: Health::new(),
            })
            .collect();
        let pool = Pool {
            inner: Arc::new(Inner {
                name: u.name.clone(),
                backends,
                balancer: Balancer::new(u.load_balancing, &weights),
                max_fails: u.max_fails,
                fail_timeout: u.fail_timeout,
                health_check: u.health_check.clone(),
                epoch: Instant::now(),
                max_conns: u.max_conns.max(1),
                max_pending: u.max_pending,
                timeout: DEFAULT_RESPONSE_TIMEOUT,
                opts,
                engine,
                client: OnceLock::new(),
                in_flight: AtomicUsize::new(0),
                stats: UpstreamStats::new(u.addresses.len()),
                closing: AtomicBool::new(false),
                closed: AtomicBool::new(false),
                on_closed: Mutex::new(Vec::new()),
            }),
        };
        if pool.inner.health_check.is_some() {
            let weak = Arc::downgrade(&pool.inner);
            pool.inner.engine.when_set(move |engine| {
                if let Some(inner) = weak.upgrade() {
                    probe::start(&inner, engine);
                }
            });
        }
        pool
    }

    /// The same pool, but bounding each exchange by `timeout` in all, waiting for a connection
    /// included, rather than [`DEFAULT_RESPONSE_TIMEOUT`]. Only before the pool is shared.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Pool {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.timeout = timeout;
        }
        self
    }

    /// The same pool, counting into `stats` (I37): counters shared by every graph that has the
    /// upstream, so they carry over reloads ([`akit_observe::metrics::Lease::upstream`]). Only
    /// before the pool is shared, and with a counter for each address.
    #[must_use]
    pub fn with_stats(mut self, stats: UpstreamStats) -> Pool {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            if stats.backends.len() == inner.backends.len() {
                inner.stats = stats;
            }
        }
        self
    }

    /// How long the upstream took to send a response, or to fail without one (fit's
    /// `ResponseTimes`).
    #[must_use]
    pub fn response_times(&self) -> HistogramSnapshot {
        self.inner.stats.responses.snapshot()
    }

    /// The exchanges with the address at `backend` that are over, and those it failed among them
    /// (fit's `Backend.Requests`).
    #[must_use]
    pub fn backend_requests(&self, backend: usize) -> (u64, u64) {
        self.inner
            .stats
            .backends
            .get(backend)
            .map_or((0, 0), |b| b.requests())
    }

    /// Requests to the address at `backend` that are not over: under way and waiting for a
    /// connection (fit's `Backend.Active`).
    #[must_use]
    pub fn backend_active(&self, backend: usize) -> usize {
        self.inner.backends.get(backend).map_or(0, |b| {
            b.busy
                .load(Ordering::Relaxed)
                .saturating_add(b.waiting.load(Ordering::Relaxed))
        })
    }

    /// The upstream's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// The upstream's addresses, in the config's order; [`send`](Self::send) takes an index of
    /// this list.
    pub fn addresses(&self) -> impl ExactSizeIterator<Item = &str> + '_ {
        self.inner.backends.iter().map(|b| b.address.as_str())
    }

    /// The upstream's `connect_timeout`.
    #[must_use]
    pub fn connect_timeout(&self) -> Duration {
        self.inner.opts.happy_eyeballs.connect_timeout
    }

    /// The akio client options the pool's connections are kept with.
    #[must_use]
    pub fn client_options(&self) -> &ClientOptions {
        &self.inner.opts
    }

    /// The upstream's `load_balancing` policy.
    #[must_use]
    pub fn load_balancing(&self) -> LoadBalancing {
        self.inner.balancer.policy()
    }

    /// The weight of the address at `backend` (1 when the upstream gives no `weights`).
    #[must_use]
    pub fn weight(&self, backend: usize) -> u32 {
        self.inner.balancer.weight(backend)
    }

    /// The upstream's `max_fails`: failures within `fail_timeout` that take an address out of
    /// rotation (0: never).
    #[must_use]
    pub fn max_fails(&self) -> usize {
        self.inner.max_fails
    }

    /// The upstream's `fail_timeout`.
    #[must_use]
    pub fn fail_timeout(&self) -> Duration {
        self.inner.fail_timeout
    }

    /// The upstream's `health_check`, when it has one.
    #[must_use]
    pub fn health_check(&self) -> Option<&HealthCheck> {
        self.inner.health_check.as_ref()
    }

    /// Whether the address at `backend` is out of rotation for its probes (fit's `ProbedDown`).
    #[must_use]
    pub fn is_probed_down(&self, backend: usize) -> bool {
        self.inner
            .backends
            .get(backend)
            .is_some_and(|b| b.health.probed_down())
    }

    /// The index of the address the next request goes to, by the upstream's `load_balancing`
    /// ([`balance`]): in turn, in turn by weight, or the one with the fewest requests under way
    /// (waiting for a connection included) for its weight, passing over the addresses that are
    /// [down](Self::is_down) unless all are. Each I/O thread keeps its own turn.
    #[must_use]
    pub fn pick(&self) -> usize {
        let inner = &*self.inner;
        let backends = &inner.backends;
        // The clock is read once, and only if an address has been out.
        let now = Cell::new(None);
        inner.balancer.pick_up(
            |i| {
                backends.get(i).map_or(0, |b| {
                    b.busy
                        .load(Ordering::Relaxed)
                        .saturating_add(b.waiting.load(Ordering::Relaxed))
                })
            },
            |i| {
                backends
                    .get(i)
                    .is_none_or(|b| b.health.up(|| inner.now(&now)))
            },
        )
    }

    /// Whether the address at `backend` is out of rotation: for having failed `max_fails`
    /// requests within `fail_timeout`, less than `fail_timeout` ago, or for its probes.
    #[must_use]
    pub fn is_down(&self, backend: usize) -> bool {
        let now = Cell::new(None);
        self.inner
            .backends
            .get(backend)
            .is_some_and(|b| !b.health.up(|| self.inner.now(&now)))
    }

    /// Whether any address is in rotation, as fit's `Available`: a pool none of whose addresses
    /// is still [picks](Self::pick) one, as a last resort.
    #[must_use]
    pub fn available(&self) -> bool {
        let now = Cell::new(None);
        self.inner
            .backends
            .iter()
            .any(|b| b.health.up(|| self.inner.now(&now)))
    }

    /// Sends `req` to the address at `backend` ([`addresses`](Self::addresses)), and calls `done`
    /// exactly once with the response or the reason there is none. `req.url` is the request's
    /// target in origin form (`/path?query`), which the pool puts after the address; its timeout,
    /// when set, bounds the exchange in place of the pool's. Runs on any thread and never blocks;
    /// `done` must not block either.
    ///
    /// Past `max_conns` requests under way to the address, it waits for one to finish; past
    /// `max_pending` waiting, it fails at once with [`Error::Busy`]. A closed pool fails it at
    /// once with [`Error::Closed`]; an address index out of range, or a target not starting with
    /// `/`, fails with [`ClientError::InvalidUrl`].
    pub fn send(
        &self,
        backend: usize,
        req: ClientRequest,
        done: impl FnOnce(Outcome) + Send + 'static,
    ) {
        self.inner
            .send(backend, req, Deliver::Whole(Box::new(done)));
    }

    /// Like [`send`](Self::send), with the response delivered to `x` as it arrives, and the
    /// request's body written as it is produced when it is [streamed](ClientRequest::streamed)
    /// ([`Exchange::on_request_body`]). The address is held until the response is complete; a
    /// failure after the head counts against it unless the exchange
    /// [cancelled](Download::cancel) the response. A request that fails before it is sent gets
    /// [`Exchange::on_error`] at once, on this thread.
    pub fn stream(&self, backend: usize, req: ClientRequest, x: impl Exchange) {
        self.inner.send(backend, req, Deliver::Stream(Box::new(x)));
    }

    /// [`pick`](Self::pick) and [`send`](Self::send).
    pub fn request(&self, req: ClientRequest, done: impl FnOnce(Outcome) + Send + 'static) {
        self.send(self.pick(), req, done);
    }

    /// Requests sent and not yet done, waiting ones included.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.inner.in_flight.load(Ordering::SeqCst)
    }

    /// The requests under way to the address at `backend`, and waiting for it.
    #[must_use]
    pub fn load(&self, backend: usize) -> Option<BackendLoad> {
        let b = self.inner.backends.get(backend)?;
        Some(BackendLoad {
            busy: b.busy.load(Ordering::SeqCst),
            waiting: b.queue().len(),
        })
    }

    /// Closes the pool, draining it: requests sent from now on fail at once with
    /// [`Error::Closed`], while those already sent, waiting ones included, finish; once the last
    /// is done the pool is [closed](Self::is_closed). The connections the pool's client keeps
    /// idle close after `idle_timeout`, or when the engine shuts down (akio's client cannot be
    /// told to close them now: akio API request A10). Closing again does nothing more. Never
    /// blocks: graph cleanup calls it (decision 004).
    pub fn close(&self) {
        if self.inner.closing.swap(true, Ordering::SeqCst) {
            return;
        }
        if self.inner.in_flight.load(Ordering::SeqCst) == 0 {
            self.inner.mark_closed();
        }
    }

    /// Whether [`close`](Self::close) was called.
    #[must_use]
    pub fn is_closing(&self) -> bool {
        self.inner.closing.load(Ordering::SeqCst)
    }

    /// Whether the pool is closed: [`close`](Self::close) was called and every request sent
    /// before it is done.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst)
    }

    /// Runs `f` once the pool is closed: now, on this thread, if it is; else on the thread that
    /// finishes its last request. `f` must not block.
    pub fn on_closed(&self, f: impl FnOnce() + Send + 'static) {
        {
            let mut v = self
                .inner
                .on_closed
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if !self.inner.closed.load(Ordering::SeqCst) {
                v.push(Box::new(f));
                return;
            }
        }
        f();
    }
}

impl Inner {
    /// The time on the backends' health clock, read once into `now`.
    fn now(&self, now: &Cell<Option<u64>>) -> u64 {
        if let Some(t) = now.get() {
            return t;
        }
        let t = health::nanos(self.epoch.elapsed());
        now.set(Some(t));
        t
    }

    /// Counts how an exchange with `backend` ended, towards its health and in its metrics.
    fn record(&self, backend: usize, failed: bool) {
        if let Some(c) = self.stats.backends.get(backend) {
            c.done(failed);
        }
        let Some(b) = self.backends.get(backend) else {
            return;
        };
        if failed {
            b.health.failed(self.max_fails, self.fail_timeout, || {
                health::nanos(self.epoch.elapsed())
            });
        } else {
            b.health.succeeded();
        }
    }

    fn send(self: &Arc<Self>, backend: usize, req: ClientRequest, done: Deliver) {
        // Counted before `closing` is read, and `close` sets `closing` before it reads the count:
        // either this request sees the pool closing or `close` sees it in flight.
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        if self.closing.load(Ordering::SeqCst) {
            self.finish_one();
            return done.fail(Error::Closed);
        }
        let Some(b) = self.backends.get(backend) else {
            self.finish_one();
            return done.fail(Error::Client(ClientError::InvalidUrl(format!(
                "upstream {}: no address {backend}",
                self.name
            ))));
        };
        if !req.url.starts_with('/') {
            self.finish_one();
            let url = req.url;
            return done.fail(Error::Client(ClientError::InvalidUrl(url)));
        }
        if self.client().is_none() {
            self.finish_one();
            return done.fail(Error::NoEngine);
        }
        let w = Waiter {
            req,
            done,
            since: Instant::now(),
        };
        if let Some(w) = self.acquire(b, w) {
            self.start(backend, w);
        }
    }

    /// The client, made on first use once the engine is there.
    fn client(&self) -> Option<&Client> {
        if let Some(c) = self.client.get() {
            return Some(c);
        }
        let engine = self.engine.get()?;
        Some(
            self.client
                .get_or_init(|| Client::new(Arc::clone(engine)).with_options(self.opts.clone())),
        )
    }

    /// A slot for `w` on `b`: `Some(w)` when it has one now, `None` when it was queued or
    /// refused (its callback then ran).
    fn acquire(&self, b: &Backend, w: Waiter) -> Option<Waiter> {
        // The fast way, while nobody waits: first come, first served.
        if b.waiting.load(Ordering::SeqCst) == 0 && b.claim(self.max_conns) {
            return Some(w);
        }
        let mut q = b.queue();
        // `waiting` is raised before the slots are tried again, and `release` lowers `busy`
        // before it reads `waiting`: either this sees the slot released, or `release` sees this
        // waiting and takes the queue's lock, which this holds until it has queued.
        b.waiting.fetch_add(1, Ordering::SeqCst);
        if b.claim(self.max_conns) {
            b.waiting.fetch_sub(1, Ordering::SeqCst);
            return Some(w);
        }
        if q.len() >= self.max_pending {
            b.waiting.fetch_sub(1, Ordering::SeqCst);
            drop(q);
            self.finish_one();
            w.done.fail(Error::Busy);
            return None;
        }
        q.push_back(w);
        None
    }

    /// Sends `w` to `backend`, whose slot it holds.
    fn start(self: &Arc<Self>, backend: usize, w: Waiter) {
        let Waiter {
            mut req,
            done,
            since,
        } = w;
        let limit = req.timeout.unwrap_or(self.timeout);
        let left = limit.saturating_sub(since.elapsed());
        let (Some(b), Some(client)) = (self.backends.get(backend), self.client.get()) else {
            // Neither happens: `send` checked both before the slot was taken.
            self.release(backend);
            self.finish_one();
            return done.fail(Error::NoEngine);
        };
        if left.is_zero() {
            self.release(backend);
            self.record(backend, true);
            self.finish_one();
            return done.fail(Error::Timeout);
        }
        req.timeout = Some(left);
        req.url.insert_str(0, &b.base);
        let me = Arc::clone(self);
        let done = match done {
            Deliver::Whole(done) => done,
            Deliver::Stream(x) => {
                return client.request_with(
                    req,
                    Streamed {
                        pool: me,
                        backend,
                        x,
                        headed: false,
                        cancelled: Arc::new(AtomicBool::new(false)),
                        since,
                    },
                );
            }
        };
        client.request(req, move |r| {
            // The time to the response, or to the failure without one: handed over whole, its
            // body read (a streamed response, `Streamed`, is timed to its head, as fit times it).
            me.stats.responses.observe(since.elapsed());
            // The connection is back in the client's idle pool (or closed) by now: the slot
            // goes to the next request before this one's callback runs.
            me.release(backend);
            // Counted before the callback runs, so that what it does next sees the address's
            // health as this exchange left it. Any response is a success, whatever its status,
            // as in fit.
            me.record(backend, r.is_err());
            done(r.map_err(Error::Client));
            me.finish_one();
        });
    }

    /// Gives back a slot on `backend`, to the request that has waited longest if one does.
    fn release(self: &Arc<Self>, backend: usize) {
        let Some(b) = self.backends.get(backend) else {
            return;
        };
        b.busy.fetch_sub(1, Ordering::SeqCst);
        if b.waiting.load(Ordering::SeqCst) == 0 {
            return;
        }
        let next = {
            let mut q = b.queue();
            if !q.is_empty() && b.claim(self.max_conns) {
                b.waiting.fetch_sub(1, Ordering::SeqCst);
                q.pop_front()
            } else {
                None
            }
        };
        if let Some(w) = next {
            let me = Arc::clone(self);
            // Not from inside this call: a request that fails at once (an engine without I/O
            // threads) would start the next from its callback, as deep as the queue is long.
            flat(Box::new(move || me.start(backend, w)));
        }
    }

    fn finish_one(&self) {
        if self.in_flight.fetch_sub(1, Ordering::SeqCst) == 1 && self.closing.load(Ordering::SeqCst)
        {
            self.mark_closed();
        }
    }

    fn mark_closed(&self) {
        // In flight is 0 and closing is set: no request can be added (each that comes is turned
        // away, though it counts itself for a moment, and may call this again).
        if self.in_flight.load(Ordering::SeqCst) != 0 || self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let callbacks = std::mem::take(
            &mut *self
                .on_closed
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        for f in callbacks {
            f();
        }
    }
}

/// akio's handler for an exchange of [`Pool::stream`]: passes it on to the [`Exchange`], and
/// gives the address back and counts how it did once the response is over.
struct Streamed {
    pool: Arc<Inner>,
    backend: usize,
    x: Box<dyn Exchange>,
    headed: bool,
    cancelled: Arc<AtomicBool>,
    /// When the request began waiting, for the time to its response's head (I37).
    since: Instant,
}

impl ResponseHandler for Streamed {
    fn on_request_body(&mut self, body: RequestBody) {
        self.x.on_request_body(body);
    }

    fn on_head(&mut self, head: ResponseHead, body: ResponseBody) {
        self.headed = true;
        self.pool.stats.responses.observe(self.since.elapsed());
        let body = Download {
            body,
            cancelled: self.cancelled.clone(),
        };
        self.x.on_head(head, body);
    }

    fn on_data(&mut self, chunk: &[u8]) {
        self.x.on_data(chunk);
    }

    fn on_end(&mut self, trailers: &Headers) {
        // The connection is back in the client's idle pool by now, as for `send`.
        self.pool.release(self.backend);
        self.pool.record(self.backend, false);
        self.x.on_end(trailers);
        self.pool.finish_one();
    }

    fn on_error(&mut self, e: ClientError) {
        if !self.headed {
            // The time to the failure, as for a response handed over whole.
            self.pool.stats.responses.observe(self.since.elapsed());
        }
        self.pool.release(self.backend);
        // Any response is a success, as for `send`, unless its body broke off: the client going
        // (a cancel) is not the address's failure.
        let failed = !(self.headed && self.cancelled.load(Ordering::SeqCst));
        self.pool.record(self.backend, failed);
        self.x.on_error(Error::Client(e));
        self.pool.finish_one();
    }
}

/// Work [`flat`] runs after what it is running.
type Deferred = RefCell<VecDeque<Box<dyn FnOnce()>>>;

thread_local! {
    /// Whether [`flat`] is running on this thread, and what it was handed meanwhile.
    static FLAT: (Cell<bool>, Deferred) =
        const { (Cell::new(false), RefCell::new(VecDeque::new())) };
}

/// Runs `f`, and what is handed to `flat` while it runs, one after the other on this thread
/// rather than one inside the other.
fn flat(f: Box<dyn FnOnce()>) {
    let queued = FLAT.with(|(running, queue)| {
        if running.get() {
            queue.borrow_mut().push_back(f);
            None
        } else {
            running.set(true);
            Some(f)
        }
    });
    let Some(f) = queued else { return };
    // Even if one of them panics, the thread is not left queueing forever.
    struct Running;
    impl Drop for Running {
        fn drop(&mut self) {
            FLAT.with(|(running, _)| running.set(false));
        }
    }
    let _running = Running;
    f();
    while let Some(next) = FLAT.with(|(_, queue)| queue.borrow_mut().pop_front()) {
        next();
    }
}
