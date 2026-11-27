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
//!   `connect_timeout`, per address tried, for a lone address too (each attempt dialed with
//!   `Engine::dial_with`), whatever the engine's own connect timeout.
//! - **`response_timeout`** (I17): every exchange is bounded in all, from the request's start,
//!   waiting for a connection included, to the end of its response's body; fit's default,
//!   [`DEFAULT_RESPONSE_TIMEOUT`], when the config leaves it out. A request may ask for less
//!   ([`ClientRequest::timeout`]), never more.
//! - **`response_header_timeout`** and **`response_idle_timeout`** (I17b): how long the upstream
//!   may take, once the request is sent whole, to begin its response
//!   ([`ClientError::HeaderTimeout`]), and how long it may go silent mid-body while its reads are
//!   not held ([`ClientError::IdleTimeout`]): akio's clients' own (ADR-108 over HTTP/1.1,
//!   ADR-111 over h2).
//! - **Cancellation** (I17b): a request given a [`CancelToken`] ([`ClientRequest::cancel_with`])
//!   is given up when the token is cancelled, whether it waits for a connection, waits for its
//!   head or reads its body; it fails with [`ClientError::Cancelled`], which is not the address's
//!   failure.
//!
//! I19 adds load balancing ([`balance`]): [`Pool::pick`] chooses the address by the upstream's
//! `load_balancing` and `weights` (round robin, smooth weighted round robin, least connections),
//! each I/O thread keeping its own turn.
//!
//! I36 adds fit's other policies ([`balance`]): `random`, `p2c`, and consistent hashing of the
//! client's IP address, the path and query, a header or a cookie ([`Pool::pick_for`] with the
//! request's [`Keys`]); and sticky sessions by cookie ([`sticky`]): a request whose cookie names
//! an address that is up goes there, and the response to any other sets the cookie
//! ([`Pool::set_cookie`]).
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
//! I18 adds upstreams over h2 and h2c (the upstream's `protocol`), on akio's h2 client
//! ([`h2`]): each address's connections carry as many requests at once as its server allows,
//! and `max_conns` bounds the connections; `max_pending`, the balancers, health and the
//! timeouts work as over HTTP/1.1. [`Exchange`]'s bodies ([`Upload`], [`Download`]) are the same
//! for both. I18b moves what I18 did by stopgaps to akio (akio API requests A51–A53 and A66): a
//! stream given up is reset, akio times it, a held download keeps its window closed, an upload
//! is paced by what waits in its stream, request trailers go up, the dial races a name's resolved
//! addresses within `connect_timeout`, and an `h2` address whose server chooses HTTP/1.1 in ALPN
//! is spoken HTTP/1.1 over TLS.
//!
//! I46 adds [`Split`]: a proxy's requests spread over several upstreams at random by weight
//! (fit's `upstreams`), with a failover that takes them while none of the chosen upstream's
//! addresses is in rotation and one of its own is.
//!
//! I54 brings fit's upstream TLS: an `h2` upstream's `tls_server_name` is the name its
//! connections send in SNI and check the certificates against, over HTTP/2 and over HTTP/1.1
//! (akio's `Client::with_tls_server_name`, akio API request A108), probes included; its
//! `tls_ca_file` and client certificate are read when the config is compiled
//! (`akit_tls::UpstreamTls`) and handed to the pool when it is made ([`Pool::new_with_tls`]).
//!
//! Nothing here blocks: [`Pool::send`] runs on an I/O thread, and its callback runs once, on an
//! I/O thread (or the resolver's or the timer's thread, as akio's client says), never blocking
//! either.
//!
//! A pool belongs to the graph its config compiled into (decision 004): when the graph is
//! retired, its cleanup [closes](Pool::close) the pool, which **drains**: requests already sent,
//! and those waiting for a connection, still finish; new ones fail at once with
//! [`Error::Closed`]; the pool is [closed](Pool::is_closed) once the last is done, and its idle
//! connections with it. When the engine shuts down, [`EngineSlot::shutdown`] keeps the pools
//! from holding its graceful shutdown open with idle connections: an idle h2 connection is closed
//! then, as akio's HTTP/1 client closes its own (akio API request A301 moves this to akio).
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
#[cfg(any(test, feature = "fuzzing"))]
#[doc(hidden)]
pub mod fuzzing;
pub mod h2;
pub mod health;
pub mod probe;
pub mod split;
pub mod sticky;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use akio::Engine;
use akio_http::Headers;
use akio_http::client::{
    Client, ClientError, ClientOptions, ClientRequest, ClientResponse, RequestBody, ResponseBody,
    ResponseHandler, ResponseHead,
};
use akio_http::h2client::H2Options;
use akit_observe::metrics::{HistogramSnapshot, UpstreamStats};

/// What cancels a request ([`ClientRequest::cancel_with`]), over HTTP/1.1 and h2 alike.
pub use akio_http::client::CancelToken;
pub use akio_http::raw::{RawConn, RawHandler};
pub use akit_config::{HealthCheck, HealthCheckType, UpstreamProtocol};
pub use balance::{Balancer, Keys, LoadBalancing};
pub use h2::STREAMS_PER_CONN;
pub use health::Health;
pub use split::Split;
pub use sticky::{Sticky, sticky_id};

/// How long a whole exchange may take, from [`Pool::send`] to the end of the response, waiting for
/// a connection included: fit's `response_timeout` default, 1h.
pub use akit_config::DEFAULT_RESPONSE_TIMEOUT;

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
    /// The h2 and h2c pools dialing through the engine, held weakly, to be told when it shuts
    /// down ([`EngineSlot::shutdown`]); those gone are pruned as pools are added.
    h2_pools: Mutex<Vec<Weak<Inner>>>,
    /// The engine is shutting down: the pools keep no connection idle.
    shutting_down: AtomicBool,
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
    /// that fills it, when it does (never, for a slot never filled, as in `akit validate`). The
    /// pools' probes start so (I35), and the `jwt` middleware's JWK set fetches (I44).
    pub fn when_set(&self, f: impl FnOnce(&Arc<Engine>) + Send + 'static) {
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

impl EngineSlot {
    /// The engine is shutting down gracefully: from now on the pools dialing through it keep no
    /// connection idle, so that none holds the engine's shutdown open until its timeout. Each
    /// idle h2 or h2c connection is closed now (`GOAWAY`, then closed), and each other once its
    /// last stream is over; requests still go through them meanwhile. akio's HTTP/1 client
    /// does as much at the engine's shutdown on its own, its h2 client not yet (akio API request
    /// A301): until it does, `akit run` calls this before it shuts the engine down. Idempotent;
    /// never blocks.
    pub fn shutdown(&self) {
        if self.0.shutting_down.swap(true, Ordering::SeqCst) {
            return;
        }
        let pools: Vec<Arc<Inner>> = self
            .0
            .h2_pools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        for p in pools {
            for b in p.backends.iter() {
                b.h2.close_idle(false);
            }
        }
    }

    /// Whether [`shutdown`](Self::shutdown) was called.
    #[must_use]
    pub fn is_shutting_down(&self) -> bool {
        self.0.shutting_down.load(Ordering::SeqCst)
    }

    /// Adds an h2 pool to those [`shutdown`](Self::shutdown) tells, at its first dial. One added
    /// after it keeps no connection idle either: its streams' ends read the flag.
    fn add_h2_pool(&self, pool: &Arc<Inner>) {
        if pool.told_at_shutdown.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut pools = self
            .0
            .h2_pools
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        pools.retain(|p| p.strong_count() > 0);
        pools.push(Arc::downgrade(pool));
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
    /// the address failed it, rather than the pool refusing it or its sender cancelling it.
    #[must_use]
    pub fn is_upstream_failure(&self) -> bool {
        match self {
            Error::Timeout => true,
            Error::Client(e) => !matches!(e, ClientError::Cancelled),
            _ => false,
        }
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
    /// written (over h2, once the request is started). The default drops it, which fails the
    /// request.
    fn on_request_body(&mut self, body: Upload) {
        drop(body);
    }
    /// The response head. `body` holds and resumes the reads of the body that follows.
    fn on_head(&mut self, head: Head, body: Download);
    /// A piece of the body, as read (dechunked); valid for the call only.
    fn on_data(&mut self, chunk: &[u8]);
    /// The body is complete, with the trailer fields of a chunked one.
    fn on_end(&mut self, trailers: &Headers);
    /// The request failed, or (after the head) its body did not complete.
    fn on_error(&mut self, e: Error);
    /// Whether the exchange gave the request up itself, its client gone (as by dropping its
    /// unfinished [`RequestBody`], or cancelling its [`CancelToken`]), or its own deadline past
    /// (akit-proxy's `timeout`, a gRPC call's `grpc-timeout`): asked as the request fails, which
    /// is then not counted against the address.
    fn gave_up(&self) -> bool {
        false
    }
    /// A `101 Switching Protocols` to a request that asked to upgrade (I49; akio's
    /// [`ResponseHandler::on_upgrade`], ADR-113): returning a handler takes the upstream's
    /// connection over as a byte stream (its `on_open` gets the [`RawConn`]), and the request is
    /// over: no other method follows. The connection leaves the pool: the address's slot is
    /// given back at once, and the connection counts among its
    /// [`stream_count`](Pool::stream_count) until it closes. `None`, the default: the `101` is
    /// delivered as a response without a body (`on_head`, `on_end`), its connection closed.
    fn on_upgrade(&mut self, head: Head) -> Option<Box<dyn RawHandler>> {
        let _ = head;
        None
    }
}

/// The head of a response delivered as it arrives ([`Exchange::on_head`]).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Head {
    /// Status code.
    pub status: u16,
    /// Header fields.
    pub headers: Headers,
}

/// A request body written as it is produced ([`Exchange::on_request_body`]), to an HTTP/1.1
/// upstream or an h2 one. Dropped unfinished, it gives the request up.
pub struct Upload(UploadKind);

enum UploadKind {
    H1(RequestBody),
    H2(h2::Body),
}

impl Upload {
    fn h2(body: h2::Body) -> Upload {
        Upload(UploadKind::H2(body))
    }

    /// Appends `data`; false when the request is over (answered, failed or given up), or past
    /// what the connection may queue.
    pub fn write(&self, data: &[u8]) -> bool {
        match &self.0 {
            UploadKind::H1(b) => b.write(data).is_ok(),
            UploadKind::H2(b) => b.write(data),
        }
    }

    /// Bytes written but not yet sent (over h2: not yet in a DATA frame, the stream's or the
    /// connection's window closed).
    #[must_use]
    pub fn queued_bytes(&self) -> usize {
        match &self.0 {
            UploadKind::H1(b) => b.queued_bytes(),
            UploadKind::H2(b) => b.queued_bytes(),
        }
    }

    /// Calls `f` once what is queued is sent (at once, on this thread, if nothing is), or when
    /// the connection (over h2, the stream) is over.
    pub fn on_drain(&self, f: impl FnOnce() + Send + 'static) {
        match &self.0 {
            UploadKind::H1(b) => b.on_drain(f),
            UploadKind::H2(b) => b.on_drain(f),
        }
    }

    /// Ends the body; false when it could not be (the request is over, or a `Content-Length`
    /// body is short).
    pub fn finish(self) -> bool {
        match self.0 {
            UploadKind::H1(b) => b.finish().is_ok(),
            UploadKind::H2(b) => b.finish(),
        }
    }

    /// Ends the body with `trailers` after it, when it is chunked (over h2, always: a HEADERS
    /// frame ends the stream).
    pub fn finish_with_trailers(self, trailers: Headers) -> bool {
        match self.0 {
            UploadKind::H1(b) => b.finish_with_trailers(trailers).is_ok(),
            UploadKind::H2(b) => b.finish_with_trailers(trailers),
        }
    }
}

impl fmt::Debug for Upload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Upload").finish_non_exhaustive()
    }
}

/// Holds and resumes the reads of a response's body ([`Exchange::on_head`]), or gives it up.
/// Cheap to clone, usable from any thread; it does nothing once the response is over.
#[derive(Clone)]
pub struct Download(DownloadKind);

#[derive(Clone)]
enum DownloadKind {
    H1 {
        body: ResponseBody,
        cancelled: Arc<AtomicBool>,
    },
    H2(Arc<h2::Stream>),
}

impl Download {
    fn h2(s: Arc<h2::Stream>) -> Download {
        Download(DownloadKind::H2(s))
    }

    /// Holds (`true`) or resumes the upstream connection's reads (over h2, the stream's: its
    /// window is not given back, so the upstream stops once it has sent what it had; what is
    /// already in flight is still passed on).
    pub fn hold_reads(&self, hold: bool) {
        match &self.0 {
            DownloadKind::H1 { body, .. } => body.hold_reads(hold),
            DownloadKind::H2(s) => s.hold_reads(hold),
        }
    }

    /// Gives the response up: its connection is closed (over h2, its stream is reset), and the
    /// exchange ends with [`Exchange::on_error`]. The address is not counted as failing for it:
    /// the proxy's client went, not the upstream.
    pub fn cancel(&self) {
        match &self.0 {
            DownloadKind::H1 { body, cancelled } => {
                cancelled.store(true, Ordering::SeqCst);
                body.cancel();
            }
            DownloadKind::H2(s) => s.cancel(),
        }
    }
}

impl fmt::Debug for Download {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Download").finish_non_exhaustive()
    }
}

/// Where a request's response goes.
pub(crate) enum Deliver {
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
    /// Its connections, over h2 (I18).
    h2: h2::Conns,
    /// Its address as a socket address, when it is an IP literal: what a layer 4 connection
    /// dials without resolving it (I51).
    socket: Option<std::net::SocketAddr>,
    /// Layer 4 connections to it, open or being opened (I51), which `least_conn` counts too.
    streams: AtomicUsize,
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
    /// `sticky_cookie`, checked before the balancer.
    sticky: Option<Sticky>,
    /// `max_fails` and `fail_timeout`.
    max_fails: usize,
    fail_timeout: Duration,
    /// `health_check`: the probes of its addresses ([`probe`]).
    health_check: Option<HealthCheck>,
    /// What the backends' health times count from.
    epoch: Instant,
    max_conns: usize,
    /// Requests under way to one address at most: `max_conns`, times the streams each
    /// connection carries over h2.
    max_busy: usize,
    max_pending: usize,
    timeout: Duration,
    opts: ClientOptions,
    protocol: UpstreamProtocol,
    /// How h2 and h2c upstreams are reached; `None` for HTTP/1.1.
    h2: Option<h2::Transport>,
    engine: EngineSlot,
    /// Made on the first request, once the engine is there.
    client: OnceLock<Client>,
    /// Requests sent and not done, waiting ones included.
    in_flight: AtomicUsize,
    /// Its response times and its backends' counts (I37): its own, or shared with the other
    /// graphs that have the upstream ([`Pool::with_stats`]).
    stats: UpstreamStats,
    /// The graphs that hold the pool (fit's `Acquire`): one when it is made, one more for each
    /// graph a reload shares it with ([`Pool::share`]); each lets go with [`Pool::release`], and
    /// the last one closes it.
    holders: AtomicUsize,
    closing: AtomicBool,
    closed: AtomicBool,
    on_closed: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
    /// An h2 pool: it is among those [`EngineSlot::shutdown`] tells, from its first dial on
    /// (not when it is made: `with_tls` and the like need it unshared until then).
    told_at_shutdown: AtomicBool,
}

/// The h2 client's options of upstream `u`: its dial bounded by `connect_timeout`.
fn h2_options(u: &akit_config::Upstream) -> H2Options {
    let mut o = H2Options::default();
    o.happy_eyeballs.connect_timeout = u.connect_timeout;
    o
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
        Pool::new_with_tls(u, engine, None)
    }

    /// [`Pool::new`], an h2 upstream's connections made with the TLS configuration `tls` when
    /// given rather than the system's roots, as its `tls_ca_file` and client certificate make it
    /// (I54, `akit_tls::UpstreamTls`), its probes included: `tls` must offer ALPN `h2` and
    /// `http/1.1` (an address whose server chooses HTTP/1.1 is then spoken HTTP/1.1 with `tls`).
    /// Ignored for other protocols.
    #[must_use]
    pub fn new_with_tls(
        u: &akit_config::Upstream,
        engine: EngineSlot,
        tls: Option<akio::tls::ClientConfig>,
    ) -> Pool {
        let mut opts = ClientOptions::default();
        opts.max_idle_per_host = u.max_idle_conns;
        opts.max_idle = u.max_idle_conns.saturating_mul(u.addresses.len().max(1));
        opts.idle_timeout = Some(u.idle_timeout);
        opts.happy_eyeballs.connect_timeout = u.connect_timeout;
        opts.response_header_timeout = Some(u.response_header_timeout);
        opts.response_idle_timeout = Some(u.response_idle_timeout);
        let weights: Vec<u32> = (0..u.addresses.len()).map(|i| u.weight(i)).collect();
        let backends = u
            .addresses
            .iter()
            .map(|a| Backend {
                address: a.clone(),
                // Over HTTP/1.1, and for an `h2` address whose server chose HTTP/1.1, over TLS.
                base: if u.protocol == UpstreamProtocol::H2 {
                    format!("https://{a}")
                } else {
                    format!("http://{a}")
                },
                busy: AtomicUsize::new(0),
                waiting: AtomicUsize::new(0),
                queue: Mutex::new(VecDeque::new()),
                health: Health::new(),
                h2: h2::Conns::default(),
                socket: a.parse().ok(),
                streams: AtomicUsize::new(0),
            })
            .collect();
        let pool = Pool {
            inner: Arc::new(Inner {
                name: u.name.clone(),
                backends,
                balancer: Balancer::keyed(u.load_balancing, &u.addresses, &weights, &u.hash_key),
                sticky: u
                    .sticky_cookie
                    .as_ref()
                    .map(|c| Sticky::new(c, &u.addresses)),
                max_fails: u.max_fails,
                fail_timeout: u.fail_timeout,
                health_check: u.health_check.clone(),
                epoch: Instant::now(),
                max_conns: u.max_conns.max(1),
                max_busy: if u.protocol.is_h2() {
                    u.max_conns.max(1).saturating_mul(STREAMS_PER_CONN)
                } else {
                    u.max_conns.max(1)
                },
                max_pending: u.max_pending,
                timeout: u.response_timeout,
                opts,
                protocol: u.protocol,
                h2: match u.protocol {
                    UpstreamProtocol::H2c => Some(h2::Transport {
                        opts: h2_options(u),
                        tls: None,
                        server_name: None,
                    }),
                    UpstreamProtocol::H2 => Some(h2::Transport {
                        opts: h2_options(u),
                        tls: Some(tls.map_or_else(h2::system_tls, Ok)),
                        server_name: Some(u.tls_server_name.clone()).filter(|n| !n.is_empty()),
                    }),
                    _ => None,
                },
                engine,
                client: OnceLock::new(),
                in_flight: AtomicUsize::new(0),
                stats: UpstreamStats::new(u.addresses.len()),
                holders: AtomicUsize::new(1),
                closing: AtomicBool::new(false),
                closed: AtomicBool::new(false),
                on_closed: Mutex::new(Vec::new()),
                told_at_shutdown: AtomicBool::new(false),
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
    /// included, rather than by the upstream's `response_timeout`. Only before the pool is
    /// shared.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Pool {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.timeout = timeout;
        }
        self
    }

    /// The same h2 pool, its connections made with the TLS configuration `tls` rather than the
    /// system's roots; `tls` must offer ALPN `h2` (and
    /// `http/1.1`, for a server that may choose it: an address whose server does is then spoken
    /// HTTP/1.1 with `tls`). Only before the pool is shared, and only for an h2 upstream: a pool
    /// with a health check whose engine is set is already shared with its probes, so use
    /// [`Pool::new_with_tls`].
    #[must_use]
    pub fn with_tls(mut self, tls: akio::tls::ClientConfig) -> Pool {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            if let Some(t) = inner.h2.as_mut().filter(|t| t.tls.is_some()) {
                t.tls = Some(Ok(tls));
            }
        }
        self
    }

    /// The upstream's `max_conns`: connections to one address at most.
    #[must_use]
    pub fn max_conns(&self) -> usize {
        self.inner.max_conns
    }

    /// The upstream's `protocol`.
    #[must_use]
    pub fn protocol(&self) -> UpstreamProtocol {
        self.inner.protocol
    }

    /// Over h2: the connections to the address at `backend` open or being dialed, and the
    /// streams under way on them (`None` over HTTP/1.1, whose connections akio's client keeps).
    #[must_use]
    pub fn h2_connections(&self, backend: usize) -> Option<(usize, usize)> {
        self.inner.h2.as_ref()?;
        self.inner.backends.get(backend).map(|b| b.h2.counts())
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

    /// The most an exchange may take in all: the upstream's `response_timeout`.
    #[must_use]
    pub fn response_timeout(&self) -> Duration {
        self.inner.timeout
    }

    /// The upstream's `response_header_timeout`: how long it may take, once a request is sent
    /// whole, to begin its response.
    #[must_use]
    pub fn response_header_timeout(&self) -> Duration {
        self.inner.header_timeout()
    }

    /// The upstream's `response_idle_timeout`: how long it may go silent mid-body.
    #[must_use]
    pub fn response_idle_timeout(&self) -> Duration {
        self.inner.idle_timeout()
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
    /// A request with nothing to go by: the hash policies go in turn, and the sticky cookie is
    /// not read.
    #[must_use]
    pub fn pick(&self) -> usize {
        self.pick_key(None, &Cell::new(None))
    }

    /// [`pick`](Self::pick) for a request with `keys`: to the address its sticky cookie names,
    /// when the upstream has one and that address is up; otherwise by the upstream's
    /// `load_balancing`, which for a hash policy hashes the request's key (I36).
    #[must_use]
    pub fn pick_for(&self, keys: &Keys<'_>) -> usize {
        let inner = &*self.inner;
        let now = Cell::new(None);
        if let (Some(sticky), Some(headers)) = (&inner.sticky, keys.headers) {
            if let Some(b) = sticky.backend(headers) {
                if inner
                    .backends
                    .get(b)
                    .is_some_and(|x| x.health.up(|| inner.now(&now)))
                {
                    return b;
                }
            }
        }
        self.pick_key(inner.balancer.key(keys), &now)
    }

    /// The `Set-Cookie` value the response to a request with `keys`, which went to the address
    /// at `backend`, carries: the sticky cookie naming that address, unless the request's names
    /// it already; `None` without a sticky cookie.
    #[must_use]
    pub fn set_cookie(&self, keys: &Keys<'_>, backend: usize) -> Option<&str> {
        self.inner
            .sticky
            .as_ref()?
            .set_cookie(keys.headers, backend)
    }

    /// Whether the upstream has a sticky cookie.
    #[must_use]
    pub fn is_sticky(&self) -> bool {
        self.inner.sticky.is_some()
    }

    fn pick_key(&self, key: Option<u64>, now: &Cell<Option<u64>>) -> usize {
        let inner = &*self.inner;
        let backends = &inner.backends;
        // The clock is read once, and only if an address has been out.
        inner.balancer.pick_keyed(
            key,
            |i| {
                backends.get(i).map_or(0, |b| {
                    b.busy
                        .load(Ordering::Relaxed)
                        .saturating_add(b.waiting.load(Ordering::Relaxed))
                        .saturating_add(b.streams.load(Ordering::Relaxed))
                })
            },
            |i| {
                backends
                    .get(i)
                    .is_none_or(|b| b.health.up(|| inner.now(now)))
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
    /// when set, bounds the exchange too: the shorter of it and the pool's holds. Runs on any thread and never blocks;
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

    /// The address a layer 4 connection from `peer` goes to (I51), as fit's services pick it:
    /// by the upstream's `load_balancing`, `ip_hash` hashing `peer`, the other hash policies, which
    /// a connection has nothing for, in turn. Counted at once as one of the address's
    /// connections ([`stream_count`](Self::stream_count)), for `least_conn`, until
    /// [`stream_done`](Self::stream_done).
    #[must_use]
    pub fn stream_pick(&self, peer: std::net::IpAddr) -> usize {
        let b = self.pick_for(&Keys {
            peer: Some(peer),
            ..Keys::default()
        });
        if let Some(x) = self.inner.backends.get(b) {
            x.streams.fetch_add(1, Ordering::Relaxed);
        }
        b
    }

    /// A layer 4 connection to the address at `backend`, from [`stream_pick`](Self::stream_pick),
    /// is over: `failed` when it could not be opened (or a UDP session was refused), which
    /// counts against the address as a failed request does.
    pub fn stream_done(&self, backend: usize, failed: bool) {
        if let Some(x) = self.inner.backends.get(backend) {
            x.streams.fetch_sub(1, Ordering::Relaxed);
        }
        self.inner.record(backend, failed);
    }

    /// The layer 4 connections to the address at `backend` open or being opened.
    #[must_use]
    pub fn stream_count(&self, backend: usize) -> usize {
        self.inner
            .backends
            .get(backend)
            .map_or(0, |b| b.streams.load(Ordering::Relaxed))
    }

    /// The address at `backend` as a socket address, when it is an IP literal; a host name is
    /// resolved when it is dialed.
    #[must_use]
    pub fn socket_addr(&self, backend: usize) -> Option<std::net::SocketAddr> {
        self.inner.backends.get(backend).and_then(|b| b.socket)
    }

    /// The address at `backend`, as the config writes it.
    #[must_use]
    pub fn address(&self, backend: usize) -> Option<&str> {
        self.inner.backends.get(backend).map(|b| b.address.as_str())
    }

    /// The engine the pool dials through, once it is set.
    #[must_use]
    pub fn engine(&self) -> Option<&Arc<Engine>> {
        self.inner.engine.get()
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
    /// is done the pool is [closed](Self::is_closed), and its idle connections are closed then
    /// (its HTTP/1 client's with akio's `Client::close_idle`, akio API request A10; its h2 ones
    /// with `GOAWAY`). Closing again does nothing more. Never blocks: graph cleanup calls it
    /// (decision 004).
    pub fn close(&self) {
        if self.inner.closing.swap(true, Ordering::SeqCst) {
            return;
        }
        if self.inner.in_flight.load(Ordering::SeqCst) == 0 {
            self.inner.mark_closed();
        }
    }

    /// The same pool, held by one more graph (fit's `Pool.Acquire`): a reload gives the new
    /// graph the running one's pool of an upstream whose config and TLS files are the same, so
    /// that its connections, its backends' health and its probes carry over. `None` once the
    /// pool is closing: a new pool is made instead.
    #[must_use]
    pub fn share(&self) -> Option<Pool> {
        let mut n = self.inner.holders.load(Ordering::SeqCst);
        loop {
            if n == 0 || self.is_closing() {
                return None;
            }
            match self.inner.holders.compare_exchange(
                n,
                n.saturating_add(1),
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return Some(self.clone()),
                Err(now) => n = now,
            }
        }
    }

    /// Whether `other` is this same pool (a clone, or one [shared](Self::share)).
    #[must_use]
    pub fn ptr_eq(&self, other: &Pool) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// Lets go of the pool for one graph that held it (its cleanup); the last to let go closes
    /// it ([`close`](Self::close)). Never blocks.
    pub fn release(&self) {
        let mut n = self.inner.holders.load(Ordering::SeqCst);
        loop {
            if n == 0 {
                return;
            }
            match self.inner.holders.compare_exchange(
                n,
                n.saturating_sub(1),
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(1) => return self.close(),
                Ok(_) => return,
                Err(now) => n = now,
            }
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
    fn header_timeout(&self) -> Duration {
        self.opts
            .response_header_timeout
            .unwrap_or(akit_config::DEFAULT_RESPONSE_HEADER_TIMEOUT)
    }

    fn idle_timeout(&self) -> Duration {
        self.opts
            .response_idle_timeout
            .unwrap_or(akit_config::DEFAULT_RESPONSE_IDLE_TIMEOUT)
    }

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
        let ready = if self.h2.is_some() {
            self.engine.get().is_some()
        } else {
            self.client().is_some()
        };
        if !ready {
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
        Some(self.client.get_or_init(|| {
            let c = Client::new(Arc::clone(engine)).with_options(self.opts.clone());
            // An `h2` pool's, for the addresses whose server chose HTTP/1.1 in ALPN.
            h2::http1_tls(self.h2.as_ref(), c)
        }))
    }

    /// A slot for `w` on `b`: `Some(w)` when it has one now, `None` when it was queued or
    /// refused (its callback then ran).
    fn acquire(&self, b: &Backend, w: Waiter) -> Option<Waiter> {
        // The fast way, while nobody waits: first come, first served.
        if b.waiting.load(Ordering::SeqCst) == 0 && b.claim(self.max_busy) {
            return Some(w);
        }
        let mut q = b.queue();
        // `waiting` is raised before the slots are tried again, and `release` lowers `busy`
        // before it reads `waiting`: either this sees the slot released, or `release` sees this
        // waiting and takes the queue's lock, which this holds until it has queued.
        b.waiting.fetch_add(1, Ordering::SeqCst);
        if b.claim(self.max_busy) {
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
        let Waiter { req, done, since } = w;
        let limit = req.timeout.map_or(self.timeout, |t| t.min(self.timeout));
        let left = limit.saturating_sub(since.elapsed());
        if left.is_zero() {
            self.release(backend);
            self.record(backend, true);
            self.finish_one();
            return done.fail(Error::Timeout);
        }
        let http1 = self.backends.get(backend).is_some_and(|b| b.h2.fell_back());
        if self.h2.is_some() && !http1 {
            return self.start_h2(backend, req, done, since, left);
        }
        self.start_http1(backend, req, done, since, left);
    }

    /// Sends `req` (its target in origin form) to `backend` over HTTP/1.1, its slot held, the
    /// exchange bounded by `left` from now.
    fn start_http1(
        self: &Arc<Self>,
        backend: usize,
        mut req: ClientRequest,
        done: Deliver,
        since: Instant,
        left: Duration,
    ) {
        let (Some(b), Some(client)) = (self.backends.get(backend), self.client()) else {
            // Neither happens: `send` checked both before the slot was taken.
            self.release(backend);
            self.finish_one();
            return done.fail(Error::NoEngine);
        };
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
            me.record(
                backend,
                r.as_ref()
                    .is_err_and(|e| !matches!(e, ClientError::Cancelled)),
            );
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
            if !q.is_empty() && b.claim(self.max_busy) {
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
        if self.h2.is_some() {
            for b in self.backends.iter() {
                b.h2.close_idle(true);
            }
        }
        // And the HTTP/1 client's (an `h2` pool's too, for the addresses whose server chose
        // HTTP/1.1): its idle connections now, the others once their response is in (A10).
        if let Some(c) = self.client.get() {
            c.close_idle();
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
        self.x.on_request_body(Upload(UploadKind::H1(body)));
    }

    fn on_head(&mut self, head: ResponseHead, body: ResponseBody) {
        self.headed = true;
        self.pool.stats.responses.observe(self.since.elapsed());
        let body = Download(DownloadKind::H1 {
            body,
            cancelled: self.cancelled.clone(),
        });
        let head = Head {
            status: head.status,
            headers: head.headers,
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

    fn on_upgrade(&mut self, head: &ResponseHead, _conn: &RawConn) -> Option<Box<dyn RawHandler>> {
        let up = Head {
            status: head.status,
            headers: head.headers.clone(),
        };
        let h = self.x.on_upgrade(up)?;
        self.headed = true;
        self.pool.stats.responses.observe(self.since.elapsed());
        // Out of the pool, as fit's `Tunneled`: an open tunnel does not hold one of the address's
        // `max_conns`, and counts as one of its connections until it closes.
        if let Some(b) = self.pool.backends.get(self.backend) {
            b.streams.fetch_add(1, Ordering::Relaxed);
        }
        self.pool.release(self.backend);
        self.pool.record(self.backend, false);
        self.pool.finish_one();
        Some(Box::new(Tunneled {
            pool: self.pool.clone(),
            backend: self.backend,
            h,
        }))
    }

    fn on_error(&mut self, e: ClientError) {
        if !self.headed {
            // The time to the failure, as for a response handed over whole.
            self.pool.stats.responses.observe(self.since.elapsed());
        }
        self.pool.release(self.backend);
        // Any response is a success, as for `send`, unless its body broke off: the client going
        // (a cancel, its token's, or the request given up) is not the address's failure.
        let failed = !(self.headed && self.cancelled.load(Ordering::SeqCst))
            && !matches!(e, ClientError::Cancelled)
            && !self.x.gave_up();
        self.pool.record(self.backend, failed);
        self.x.on_error(Error::Client(e));
        self.pool.finish_one();
    }
}

/// An upstream connection switched to another protocol ([`Exchange::on_upgrade`]): passes its
/// bytes to the exchange's handler, and counts as one of its address's connections until it
/// is dropped (it closed, or akio let it go).
struct Tunneled {
    pool: Arc<Inner>,
    backend: usize,
    h: Box<dyn RawHandler>,
}

impl RawHandler for Tunneled {
    fn on_open(&mut self, conn: &RawConn) {
        self.h.on_open(conn);
    }
    fn on_data(&mut self, conn: &RawConn, data: &[u8]) {
        self.h.on_data(conn, data);
    }
    fn on_close(&mut self) {
        self.h.on_close();
    }
}

impl Drop for Tunneled {
    fn drop(&mut self) {
        if let Some(b) = self.pool.backends.get(self.backend) {
            b.streams.fetch_sub(1, Ordering::Relaxed);
        }
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
