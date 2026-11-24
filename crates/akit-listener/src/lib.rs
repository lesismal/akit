#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's listeners on akio: TCP, UDP, TLS and QUIC entry points, and protocol splitting with
//! akio-mux.
//!
//! So far (I05): HTTP/1.1 and h2c listeners from the graph's config, each an akio TCP listener
//! serving a [`Dispatcher`], which routes every request through the current graph of a
//! [`GraphCell`] and answers 404 when no route matches. I09: a listener with `tls` serves HTTPS,
//! its HTTP acceptor wrapped in akio-tls with the configuration the graph loaded (certificates
//! by SNI, ALPN choosing h2 or http/1.1, client authentication); since I30 its certificates are
//! chosen per handshake by an [`akit_runtime::ListenerCerts`], from the current graph. Since I11 the routes go by the
//! request's canonical path, as fit's listener does: dot and empty segments removed, a path
//! climbing above the root or hiding a separator in an escape answered `400 bad request path`.
//! I10: a TLS listener serving `h3` serves HTTP/3 on akio-h3, over QUIC on its port in UDP, with
//! the certificates its TCP connections serve; its HTTP/1.1 and HTTP/2 responses announce it with
//! `Alt-Svc`, as fit's do.
//!
//! I21: a graph with an access log has every request's line written once its response is done,
//! by a response filter the [`Dispatcher`] registers: it runs on the thread that sends the
//! response (the I/O thread, or the one a detached handler answers from), and builds the line
//! there, from the request and the response ([`access_record`]).
//!
//! I22: [`Listeners::reload`] publishes a new graph and moves the listeners to its (see
//! [`reload`]).
//!
//! I37: a graph whose config has `metrics` has each request counted by listener and route
//! (`fit_http_requests_total`, `fit_http_request_duration_seconds`): one answered before its
//! handler returns (an inline handler's, the 404, the 400) once the call returns, with no hook;
//! any other by a response filter, the access log's when there is one. [`metrics::Scrape`] writes
//! every metric, in fit's names, for the Admin API's `/metrics`.
//!
//! I16: each listener takes request bodies up to the config's `limits.max_body_bytes` (fit's
//! 16 MiB by default) over HTTP/1.1, HTTP/2 and HTTP/3, and answers a larger one 413. A request
//! whose route's handler takes its body as it arrives (`body_mode`, the proxy's) gets it so,
//! with [`MAX_BODY_BYTES`] telling the handler the limit, which it holds the body to; one that
//! declares a length past it is answered 413 before its handler runs, as before.
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

use std::borrow::Cow;
use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv6Addr, SocketAddr, TcpListener, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use akio::tls::Tls;
use akio::{Backend, Config as EngineConfig, Engine, Listener as EngineListener};
use akio_http::{BodyMode, Context, HttpConfig, Method, Request, Response, Version};
use akit_config::{Listener, Protocol};
use akit_observe::metrics::Shard as MetricsShard;
use akit_observe::{AccessLog, Name, Record, Shard};
use akit_runtime::GraphCell;

pub mod metrics;
pub mod reload;
pub use reload::{Listeners, ReloadError, Reloaded, Served};

/// fit's answer when no route matches.
const NOT_FOUND: &str = "404 page not found\n";

/// fit's answer to a path that has no canonical form ([`akit_router::canonicalize`]).
const BAD_REQUEST_PATH: &str = "400 bad request path\n";

pub use akit_runtime::{CANONICAL_PATH, MAX_BODY_BYTES, TLS};

/// The akio service of one listener: each request goes to the route the current graph picks for
/// it on this listener, or is answered 404. Runs on the I/O thread, and only reads the graph.
pub struct Dispatcher {
    cell: GraphCell,
    listener: String,
    /// The listener's name, as each logged request carries it to its line.
    log_name: Name,
    tls: bool,
    /// The `Alt-Svc` value every response gets, for the TCP side of a listener that also serves
    /// HTTP/3; made once, when the listener is set up.
    alt_svc: Option<String>,
    /// The listener's `max_body_bytes`, as [`MAX_BODY_BYTES`] gives it to a handler that takes
    /// its body as it arrives.
    max_body_bytes: u64,
}

impl Dispatcher {
    /// The dispatcher of the listener named `listener`.
    #[must_use]
    pub fn new(cell: GraphCell, listener: impl Into<String>) -> Dispatcher {
        let listener = listener.into();
        Dispatcher {
            cell,
            log_name: Name::new(&listener),
            listener,
            tls: false,
            alt_svc: None,
            max_body_bytes: akit_config::DEFAULT_MAX_BODY_BYTES,
        }
    }

    /// The same, for a listener that serves TLS: its requests carry [`TLS`].
    #[must_use]
    pub fn tls(mut self, tls: bool) -> Dispatcher {
        self.tls = tls;
        self
    }

    /// The listener's `max_body_bytes`: a request body streamed to its handler is held to it
    /// ([`MAX_BODY_BYTES`]), and one that declares a length past it is answered 413 by akio
    /// before its handler runs. fit's 16 MiB unless set.
    #[must_use]
    pub fn max_body_bytes(mut self, n: u64) -> Dispatcher {
        self.max_body_bytes = n;
        self
    }

    /// Adds `Alt-Svc: <value>` to every response, as fit's dispatcher does on a listener that also
    /// serves HTTP/3 ([`alt_svc`]).
    #[must_use]
    pub fn with_alt_svc(mut self, value: impl Into<String>) -> Dispatcher {
        self.alt_svc = Some(value.into());
        self
    }
}

/// The `Alt-Svc` value announcing HTTP/3 on `port` of the same host, as fib's `http3.AltSvc`
/// writes it: `h3=":443"; ma=86400`.
#[must_use]
pub fn alt_svc(port: u16) -> String {
    format!("h3=\":{port}\"; ma=86400")
}

impl fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dispatcher")
            .field("listener", &self.listener)
            .field("alt_svc", &self.alt_svc)
            .finish_non_exhaustive()
    }
}

impl Dispatcher {
    /// Has `log` write the request's line, and `count` count it (I37), once its response is
    /// done, from the thread that sends it, as fit's `logRequest`: one hook for both. A request
    /// whose response is never sent (its connection went first) has no line and is not counted.
    ///
    /// The hook is a boxed closure, one allocation a request, until akio can call a listener's
    /// observer with a few words of the request's own (akio API request A29).
    fn track(
        &self,
        ctx: &mut Context,
        log: Option<&AccessLog>,
        count: Option<(Arc<MetricsShard>, u32)>,
        route: &str,
    ) {
        let start = Instant::now();
        match log {
            Some(log) => {
                let shard = log.shard();
                let listener = self.log_name.clone();
                let route = Name::new(route);
                ctx.add_response_filter(move |resp: &mut Response, req: &Request| {
                    let took =
                        write_line(&shard, listener.as_str(), route.as_str(), start, resp, req);
                    if let Some((m, slot)) = count {
                        m.observe(slot, resp.status, took);
                    }
                });
            }
            None => {
                if let Some((m, slot)) = count {
                    ctx.add_response_filter(move |resp: &mut Response, _: &Request| {
                        m.observe(slot, resp.status, start.elapsed());
                    });
                }
            }
        }
    }
}

/// Writes the line of `req`, answered with `resp`, to `shard`, with the upstream the handler
/// told akit-observe of, if any.
fn write_line(
    shard: &Shard,
    listener: &str,
    route: &str,
    start: Instant,
    resp: &Response,
    req: &Request,
) -> Duration {
    let duration = start.elapsed();
    akit_observe::upstream(|upstream| {
        shard.log(&access_record(
            listener, route, duration, upstream, resp, req,
        ));
    });
    duration
}

/// The access log's record of `req` answered with `resp`, as fit's `observe` fills its fields:
/// the client's address (an IPv4 address mapped into IPv6 written as IPv4, as Go has it), the
/// protocol as Go's `Request.Proto`, the `Host` (the target's authority for an absolute-form
/// target without one), the target as sent, the request body's bytes (its `Content-Length` for a
/// body not read whole), and the response body's: none for `HEAD` and for a status that has no
/// body, the file's for a file, the `Content-Length` for a streamed one (its bytes are not known
/// when its head is sent).
#[must_use]
pub fn access_record<'a>(
    listener: &'a str,
    route: &'a str,
    duration: Duration,
    upstream: Option<(&'a str, Duration)>,
    resp: &'a Response,
    req: &'a Request,
) -> Record<'a> {
    let content_length = |h: &akio_http::Headers| {
        h.get("content-length")
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(0)
    };
    let bytes_in = if req.body.is_empty() {
        content_length(&req.headers)
    } else {
        req.body.len() as u64
    };
    let bytes_out = if req.method == Method::Head || matches!(resp.status, 100..=199 | 204 | 304) {
        0
    } else if let Some(f) = &resp.file {
        f.len
    } else if resp.body.is_empty() {
        content_length(&resp.headers)
    } else {
        resp.body.len() as u64
    };
    let ip = req.peer.ip();
    let client = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        IpAddr::V4(_) => ip,
    };
    let host = req.headers.get("host").unwrap_or_else(|| {
        req.target
            .split_once("://")
            .map(|(_, rest)| rest.split(['/', '?']).next().unwrap_or(""))
            .unwrap_or("")
    });
    Record {
        listener,
        route,
        client,
        user: "",
        proto: match req.version {
            Version::Http10 => "HTTP/1.0",
            Version::Http11 => "HTTP/1.1",
            Version::Http2 => "HTTP/2.0",
            Version::Http3 => "HTTP/3.0",
            _ => "HTTP/1.1",
        },
        method: req.method.as_str(),
        host,
        uri: &req.target,
        status: resp.status,
        bytes_in,
        bytes_out,
        duration,
        upstream,
        user_agent: req.headers.get("user-agent").unwrap_or(""),
        referer: req.headers.get("referer").unwrap_or(""),
    }
}

impl akio_http::Service for Dispatcher {
    fn call(&self, ctx: &mut Context) {
        // On every response, the 400 and 404 below included, as fit adds it before routing.
        if let Some(v) = &self.alt_svc {
            ctx.add_response_header("alt-svc", v);
        }
        self.cell.with(|g| {
            let log = g.access_log();
            let listener = g.listener_index(&self.listener);
            // The registry, when the graph counts requests (I37).
            let metrics = g.metrics();
            // The routes go by the canonical path, as fit's do; a path that has none is answered
            // 400, and counted with the requests no route takes.
            let (route, changed) = {
                let Some(path) = akit_router::canonicalize(ctx.path()) else {
                    let slot = metrics.zip(listener.and_then(|i| g.request_slot(i, None)));
                    if let Some(log) = log {
                        self.track(ctx, Some(log), None, "");
                    }
                    let start = Instant::now();
                    ctx.text(400, BAD_REQUEST_PATH);
                    if let Some((m, slot)) = slot {
                        m.observe(slot, 400, start.elapsed());
                    }
                    return;
                };
                let req = RouterRequest {
                    req: ctx.req(),
                    path: &path.decoded,
                };
                let route = listener.and_then(|i| g.route_request_at(i, &req));
                let changed = match path.escaped {
                    Cow::Owned(p) => Some(p),
                    Cow::Borrowed(_) => None,
                };
                (route, changed)
            };
            if let Some(p) = changed {
                ctx.set_local(CANONICAL_PATH, p);
            }
            if self.tls {
                ctx.set_local(TLS, "");
            }
            if ctx.body_reads().is_some() {
                // Its body streams (`body_mode`): the handler holds it to the limit.
                ctx.set_local(MAX_BODY_BYTES, self.max_body_bytes.to_string());
            }
            let slot = metrics.zip(listener.and_then(|i| g.request_slot(i, route.map(|(i, _)| i))));
            // A request answered before its handler returns (fit's inline ones, and the 404) is
            // counted then, with no hook; any other is counted by the hook on its response.
            let inline = route.is_none_or(|(_, r)| r.handler.inline());
            let hooked = match slot {
                Some((m, slot)) if !inline || log.is_some() => m.shard().map(|s| (s, slot)),
                _ => None,
            };
            let start = Instant::now();
            match log {
                Some(log) => {
                    self.track(
                        ctx,
                        Some(log),
                        hooked,
                        route.and_then(|(_, r)| r.name.as_deref()).unwrap_or(""),
                    );
                    match route {
                        Some((_, r)) => akit_observe::dispatch_logged(|| r.handler.call(ctx)),
                        None => ctx.text(404, NOT_FOUND),
                    }
                }
                None => {
                    if hooked.is_some() {
                        self.track(ctx, None, hooked, "");
                    }
                    match route {
                        Some((_, r)) => r.handler.call(ctx),
                        None => ctx.text(404, NOT_FOUND),
                    }
                    if let (true, Some((m, slot)), Some(status)) = (inline, slot, ctx.status()) {
                        m.observe(slot, status, start.elapsed());
                    }
                }
            }
        });
    }

    /// The route's handler's: a request whose route takes its body as it arrives (the proxy's,
    /// I16) gets it so, unless it declares a length past `max_body_bytes`, which is then read
    /// as buffered bodies are, and answered 413 by akio. Others are buffered (akio's default),
    /// within the listener's `max_body_bytes` (`http_config`).
    ///
    /// The route is looked up here and again in [`call`](Self::call), on the graph current each
    /// time; a reload in between can hand a streamed body to a route that buffers, which then
    /// sees it empty.
    fn body_mode(&self, req: &Request) -> BodyMode {
        self.cell.with(|g| {
            let Some(path) = akit_router::canonicalize(req.path()) else {
                return BodyMode::Buffered;
            };
            let rr = RouterRequest {
                req,
                path: &path.decoded,
            };
            let Some(route) = g
                .listener_index(&self.listener)
                .and_then(|i| g.route_request(i, &rr))
            else {
                return BodyMode::Buffered;
            };
            match route.handler.body_mode(req) {
                BodyMode::Stream
                    if declared_length(req).is_some_and(|n| n > self.max_body_bytes) =>
                {
                    BodyMode::Buffered
                }
                mode => mode,
            }
        })
    }
}

/// The body length `req` declares (`Content-Length`), if any.
fn declared_length(req: &Request) -> Option<u64> {
    req.headers
        .get("content-length")
        .and_then(|v| v.trim().parse().ok())
}

/// A request as the router's matchers read it: akio's request, with the canonical path.
struct RouterRequest<'a> {
    req: &'a Request,
    path: &'a [u8],
}

impl akit_router::Request for RouterRequest<'_> {
    fn method(&self) -> &str {
        self.req.method.as_str()
    }
    fn host(&self) -> &str {
        self.req.headers.get("host").unwrap_or("")
    }
    fn path(&self) -> &[u8] {
        self.path
    }
    fn query(&self) -> &str {
        self.req.query().unwrap_or("")
    }
    fn remote_ip(&self) -> Option<IpAddr> {
        Some(self.req.peer.ip())
    }
    fn any_header(&self, name: &str, f: impl FnMut(&str) -> bool) -> bool {
        self.req.headers.get_all(name).any(f)
    }
}

/// Why the listeners could not be set up.
#[derive(Debug)]
pub enum Error {
    /// A listener's address does not resolve.
    Address {
        /// The listener's name.
        listener: String,
        /// Its address.
        address: String,
        /// Why.
        error: io::Error,
    },
    /// A listener asks for what akit does not support yet.
    NotSupportedYet {
        /// The listener's name.
        listener: String,
        /// What.
        what: String,
    },
    /// akio could not build or start the engine (an address in use, say).
    Engine(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Address {
                listener,
                address,
                error,
            } => write!(f, "listener {listener}: address {address:?}: {error}"),
            Error::NotSupportedYet { listener, what } => {
                write!(f, "listener {listener}: not supported yet: {what}")
            }
            Error::Engine(e) => write!(f, "listening: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Address { error, .. } | Error::Engine(error) => Some(error),
            Error::NotSupportedYet { .. } => None,
        }
    }
}

/// Where a listener's `address` listens: `":8080"` on every address, IPv6 and IPv4 (akio binds
/// `[::]` dual-stack), as Go's `net.Listen` does; a host name resolved, its first address taken.
/// Resolving may block: call it on the control plane.
///
/// # Errors
///
/// When the address does not resolve.
pub fn resolve(address: &str) -> io::Result<SocketAddr> {
    if let Some(port) = address.strip_prefix(':') {
        let port = port
            .parse::<u16>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "not a port"))?;
        return Ok(SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), port));
    }
    // "[::1]:80", "127.0.0.1:80" and "localhost:80" as std takes them.
    address
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "resolves to no address"))
}

/// The akio HTTP settings of a listener serving `protocols`.
fn http_config(l: &Listener) -> Result<HttpConfig, Error> {
    let h1 = l.protocols.contains(&Protocol::Http11);
    let h2c = l.protocols.contains(&Protocol::H2c);
    // Over TLS, ALPN picks h2 (akit_tls::alpn): a client that negotiates it sends HTTP/2's
    // preface. One that offers no ALPN is served HTTP/1.1 even by an h2-only listener, as Go's
    // server serves it.
    let h2 = l.protocols.contains(&Protocol::H2);
    if h2c && !h1 {
        // fit serves HTTP/2 alone then; akio's server always takes HTTP/1 as well.
        return Err(Error::NotSupportedYet {
            listener: l.name.clone(),
            what: "protocols h2c without http/1.1".into(),
        });
    }
    let mut cfg = HttpConfig::default();
    cfg.max_body_bytes = body_limit(l);
    match &mut cfg.h2 {
        Some(h2cfg) if h2c || h2 => h2cfg.max_body_bytes = cfg.max_body_bytes,
        _ => cfg.h2 = None,
    }
    Ok(cfg)
}

/// The listener's `limits.max_body_bytes` for akio, which answers a body past it 413 (one that
/// declares its length, at once; a chunked one, once it passes it), as fit's listener does.
fn body_limit(l: &Listener) -> usize {
    usize::try_from(l.max_body_bytes).unwrap_or(usize::MAX)
}

/// The akio HTTP/3 settings of a listener serving `h3`.
fn h3_config(l: &Listener) -> akio_h3::H3Config {
    let mut cfg = akio_h3::H3Config::default();
    cfg.max_body_bytes = body_limit(l);
    cfg
}

/// How the listeners are served.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Options {
    /// The akio backend of every listener (epoll, kqueue on macOS, by default).
    pub backend: Backend,
    /// How long [`shutdown`] waits for requests in flight before closing their connections:
    /// fit's `-shutdown-timeout`, 30 s by default.
    pub shutdown_timeout: Duration,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            backend: Backend::default(),
            shutdown_timeout: Duration::from_secs(30),
        }
    }
}

/// Whether a listener serves HTTP over TCP: HTTP/1.1, h2c or h2.
fn serves_tcp(l: &Listener) -> bool {
    l.protocols
        .iter()
        .any(|p| matches!(p, Protocol::Http11 | Protocol::H2c | Protocol::H2))
}

/// Whether a listener serves HTTP/3, over QUIC in UDP.
fn serves_h3(l: &Listener) -> bool {
    l.protocols.contains(&Protocol::H3)
}

/// How many ports [`engine`] tries for listeners at port 0 that serve HTTP/3 beside TCP, whose
/// UDP port is the one TCP gets and may be taken (fit's `maxPortZeroBinds`).
const MAX_PORT_ZERO_BINDS: usize = 8;

/// Where one listener listens, as [`addrs`] gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Addrs {
    /// Its TCP address, for a listener serving HTTP/1.1, h2c or h2.
    pub tcp: Option<SocketAddr>,
    /// Its UDP address, where HTTP/3 listens, for a listener serving h3: the same port as TCP's.
    pub udp: Option<SocketAddr>,
}

impl Addrs {
    /// The address to report: TCP's, or UDP's for a listener serving HTTP/3 alone (fit's
    /// `Addr.Addr`).
    #[must_use]
    pub fn addr(&self) -> Option<SocketAddr> {
        self.tcp.or(self.udp)
    }
}

/// Where each of `listeners` (the graph's [`engine`] was built from) listens on `engine`, in
/// their order.
#[must_use]
pub fn addrs(listeners: &[Listener], engine: &Engine) -> Vec<Addrs> {
    let mut bound = engine.local_addrs().into_iter();
    listeners
        .iter()
        .map(|l| Addrs {
            tcp: if serves_tcp(l) { bound.next() } else { None },
            udp: if serves_h3(l) { bound.next() } else { None },
        })
        .collect()
}

/// An akio engine serving every listener of the cell's current graph, each with its own
/// [`Dispatcher`], and hosting the connections of its upstream pools (on the last listener's
/// threads); built, not started. A listener serving `h3` serves HTTP/3 over UDP on the port its
/// TCP side listens on, which the `Alt-Svc` of its TCP responses names. [`addrs`] gives where
/// each listens, in the graph's order ([`Engine::local_addrs`] lists each listener's TCP address,
/// then its UDP one).
///
/// # Errors
///
/// When an address does not resolve, a listener asks for what akit does not support yet, or akio
/// cannot build the engine (an address in use).
pub fn engine(cell: &GraphCell, opts: &Options) -> Result<Engine, Error> {
    let mut last = None;
    for _ in 0..MAX_PORT_ZERO_BINDS {
        match build(cell, opts) {
            // A listener at port 0 serving h3 beside TCP: the port probed free for both was
            // taken before akio bound it. Another one, as fit's bind tries.
            Err((Error::Engine(e), true)) if e.kind() == io::ErrorKind::AddrInUse => {
                last = Some(e);
            }
            Ok(e) => return Ok(e),
            Err((e, _)) => return Err(e),
        }
    }
    Err(Error::Engine(
        last.unwrap_or_else(|| io::ErrorKind::AddrInUse.into()),
    ))
}

/// [`engine`]'s one try; the error says too whether a port was probed for a listener at port 0.
fn build(cell: &GraphCell, opts: &Options) -> Result<Engine, (Error, bool)> {
    type Loaded = (Listener, Option<akit_tls::ServerConfigs>);
    // Each TLS listener's configurations, its certificates served from the current graph per
    // handshake (I30), so that a reload keeping the listener changes them.
    let listeners: Vec<Loaded> = cell
        .with(|g| {
            g.listeners()
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    let tls = akit_runtime::tls::server_configs(cell, g, i, false).transpose()?;
                    Ok((l.clone(), tls))
                })
                .collect::<Result<_, akit_tls::Error>>()
        })
        .map_err(|e| (tls_error(&e), false))?;
    let mut probed = false;
    let mut cfg = EngineConfig::default();
    cfg.shutdown_timeout = opts.shutdown_timeout;
    // The upstream pools dial on this engine. akio's client honours an upstream's
    // connect_timeout only for a name that resolves to several addresses, and dials a lone one
    // with the engine's (akio API request A12): until then the engine's is the shortest of the
    // upstreams', which is each one's own when they agree (fit's default, 10s, when none sets it).
    if let Some(t) = cell.with(|g| g.upstreams().iter().map(|p| p.connect_timeout()).min()) {
        cfg.connect_timeout = Some(t);
    }
    let mut b = Engine::builder().config(cfg);
    for (l, tls) in &listeners {
        let fail = |e: Error| (e, false);
        let mut addr = resolve(&l.address).map_err(|error| {
            fail(Error::Address {
                listener: l.name.clone(),
                address: l.address.clone(),
                error,
            })
        })?;
        let (tcp, h3) = (serves_tcp(l), serves_h3(l));
        if tcp && h3 && addr.port() == 0 {
            // HTTP/3 goes on the port TCP gets, the one Alt-Svc names: a port free for both.
            probed = true;
            let port = free_port(addr.ip()).map_err(|e| (Error::Engine(e), true))?;
            addr.set_port(port);
        }
        if tcp {
            let listener = EngineListener::tcp(addr).backend(opts.backend.clone());
            b = b.listen(
                tcp_listener(
                    cell,
                    l,
                    tls.as_ref().map(|c| &c.tcp),
                    listener,
                    h3.then_some(addr.port()),
                )
                .map_err(fail)?,
            );
        }
        if h3 {
            // Per-peer UDP listeners run on akio's epoll (kqueue) UDP loop whatever the TCP
            // backend: the backend is left to akio.
            b = b.listen(EngineListener::udp(addr).serve(
                h3_acceptor(cell, l, tls.as_ref().and_then(|c| c.h3.as_ref())).map_err(fail)?,
            ));
        }
    }
    // The HTTP/3 of a listener a reload adds is bound on the engine's UDP threads
    // (`Engine::bind_udp`, I22): those of the last listener serving h3, or one of their own when
    // none does. (akio adds TCP listeners at run time, A1, not UDP ones.)
    if !listeners.iter().any(|(l, _)| serves_h3(l)) {
        b = b.udp_client_threads(1);
    }
    b.build().map_err(|e| (Error::Engine(e), probed))
}

/// The TCP side of listener `l` on `listener` (its address and backend set): HTTP/1.1 and h2c,
/// or HTTPS with `tls` (the graph's, loaded); `h3_port` names the port of its HTTP/3, which
/// its responses' `Alt-Svc` advertises.
fn tcp_listener(
    cell: &GraphCell,
    l: &Listener,
    tls: Option<&akio::tls::ServerConfig>,
    listener: EngineListener,
    h3_port: Option<u16>,
) -> Result<EngineListener, Error> {
    let mut d = Dispatcher::new(cell.clone(), l.name.clone())
        .tls(l.tls.is_some())
        .max_body_bytes(l.max_body_bytes);
    if let Some(port) = h3_port {
        d = d.with_alt_svc(alt_svc(port));
    }
    let http = akio_http::serve_with(http_config(l)?, d);
    Ok(match (&l.tls, tls) {
        (None, _) => listener.serve(http),
        (Some(_), Some(cfg)) => listener.serve(Tls::server(cfg.clone(), http)),
        // The graph loads every listener's tls, or is not compiled.
        (Some(_), None) => return Err(not_loaded(l)),
    })
}

/// The HTTP/3 acceptor of listener `l`, with its h3 TLS (the graph's, loaded).
fn h3_acceptor(
    cell: &GraphCell,
    l: &Listener,
    h3_tls: Option<&akio::tls::ServerConfig>,
) -> Result<impl akio::Acceptor, Error> {
    // akit-config refuses h3 without tls; the graph loads its h3 TLS with the rest.
    let Some(tls) = h3_tls else {
        return Err(not_loaded(l));
    };
    let quic = akio_quic::server_config_from(tls)
        .map_err(|e| Error::Engine(io::Error::other(e.to_string())))?;
    Ok(akio_h3::serve_with(
        quic,
        h3_config(l),
        // HTTP/3 is always over TLS.
        Dispatcher::new(cell.clone(), l.name.clone())
            .tls(true)
            .max_body_bytes(l.max_body_bytes),
    ))
}

/// A listener's TLS configuration refused by akio-tls (the graph built one when it was
/// compiled, so this is not expected).
fn tls_error(e: &akit_tls::Error) -> Error {
    Error::Engine(io::Error::other(e.to_string()))
}

fn not_loaded(l: &Listener) -> Error {
    Error::Engine(io::Error::other(format!(
        "listener {}: its tls was not loaded",
        l.name
    )))
}

/// A port on `ip` free for TCP and for UDP at once, for a listener at port 0 serving HTTP/3 on
/// the port TCP gets: TCP's free port, unless UDP's is taken, as fit's bind tries them. Binds
/// and closes sockets: control plane only. The port may still be taken before akio binds it;
/// [`engine`] then tries again.
fn free_port(ip: IpAddr) -> io::Result<u16> {
    let mut last = io::Error::from(io::ErrorKind::AddrInUse);
    for _ in 0..MAX_PORT_ZERO_BINDS {
        let tcp = TcpListener::bind(SocketAddr::new(ip, 0))?;
        let port = tcp.local_addr()?.port();
        match UdpSocket::bind(SocketAddr::new(ip, port)) {
            Ok(_) => return Ok(port),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Shuts `engine` down gracefully and waits until it has: the listeners stop accepting, idle
/// connections are closed, and a connection with requests in flight is closed once their
/// responses are sent (an HTTP/2 one gets `GOAWAY`), or when the engine's shutdown timeout
/// ([`Options::shutdown_timeout`], `timeout` here) passes. Returns false when it passed with
/// connections still open, which akio then closed.
pub fn shutdown(engine: &Engine, timeout: Duration) -> bool {
    let open = engine.telemetry().open_connections();
    let start = Instant::now();
    engine.shutdown();
    engine.wait();
    // akio waits as long as the timeout only for connections it then closes.
    open == 0 || start.elapsed() < timeout
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::net::SocketAddr;

    #[test]
    fn addresses() {
        assert_eq!(
            super::resolve(":8080").unwrap(),
            "[::]:8080".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            super::resolve("127.0.0.1:0").unwrap(),
            "127.0.0.1:0".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            super::resolve("[::1]:80").unwrap(),
            "[::1]:80".parse::<SocketAddr>().unwrap()
        );
        assert!(super::resolve("localhost:80").unwrap().ip().is_loopback());
        assert!(super::resolve(":x").is_err());
    }
}
