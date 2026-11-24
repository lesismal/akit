//! Requests to an h2 or h2c upstream (I18), on akio's [`H2Client`]: the connections to each
//! address, the streams on them, and an exchange's two bodies.
//!
//! Each address has its own connections ([`Conns`]). A request takes a stream on the first
//! connection with one free, and a connection is dialed when none has one: at most
//! [`STREAMS_PER_CONN`] streams to a connection, and the pool's `max_conns` bounds the
//! connections, through the streams under way to the address, which are at most `max_conns`
//! times as many. A connection that has carried its last stream stays for the next request,
//! as an HTTP/1 one does: at most `max_idle_conns` of them to an address, each for
//! `idle_timeout`.
//!
//! What akio's h2 client cannot do yet is done here by stopgaps, each bounded, until akio API
//! requests A51–A53 replace them (I18b):
//!
//! - **Streams per connection** are [`STREAMS_PER_CONN`], the least RFC 9113 lets a server
//!   advertise, since akio's client neither reports nor heeds the server's
//!   `SETTINGS_MAX_CONCURRENT_STREAMS` (A53).
//! - **Giving a stream up** (the client gone, the exchange's timeout, a client too slow): akio's
//!   streamed requests ([`H2Client::open`]) cannot be reset (A51), so the stream is left to run
//!   with its events dropped, and its connection takes no more streams and is closed once its
//!   other streams are over, which ends it. A connection that failed a stream as closed (as
//!   after a `GOAWAY`) takes no more either.
//! - **Timeouts**: akio's streamed requests have none (A51), so each address's streams are
//!   checked every [`TICK`] by a job of akit's timer thread ([`crate::timer`]), which gives up
//!   those past their deadline. The same job closes connections idle past `idle_timeout`.
//! - **Backpressure on the response**: akio's client gives each stream's window back as soon as
//!   data arrives (A52), so holding a download cannot slow the upstream. While a download is
//!   held, what still arrives is passed on and counted; past [`HELD_LIMIT`] the stream is given
//!   up, its response cut short, as a client that took nothing of it would have it.
//! - **Backpressure on the request**: akio's [`H2Upload`] reports nothing of what waits in its
//!   queue (A52), so an upload is not paced: what waits there is bounded by the body's limit
//!   (the listener's `limits.max_body_bytes`).
//! - **Request trailers** are dropped: [`H2Upload`] cannot send them (A51).

use std::io;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError, Weak};
use std::time::{Duration, Instant};

use akio::Engine;
use akio::tls::ClientConfig as TlsClientConfig;
use akio_http::Headers;
use akio_http::client::{ClientError, ClientRequest, ClientResponse, StreamedBody};
use akio_http::h2client::{H2Client, H2Event, H2Options, H2Upload};

use crate::{Error, Exchange, Head, Inner};

/// The most streams a request opens on one connection at once: the least RFC 9113 (§6.5.2)
/// lets a server advertise, until akio reports the server's own (akio API request A53).
pub const STREAMS_PER_CONN: usize = 100;

/// How much of a held download may still arrive before the stream is given up (the stopgap for
/// akio API request A52).
pub const HELD_LIMIT: usize = 4 << 20;

/// How often each address's streams are checked for their deadline, and its idle connections
/// for their `idle_timeout`, while it has connections (the stopgap for akio API request A51).
pub const TICK: Duration = Duration::from_millis(100);

/// Why a stream is given up before akio ends it.
const STOP_NONE: u8 = 0;
/// The proxy's client went, after the response began ([`crate::Download::cancel`]).
const STOP_CANCEL: u8 = 1;
/// The request's body was dropped unfinished: its client went before it ended.
const STOP_GAVE_UP: u8 = 2;
/// The exchange's timeout passed.
const STOP_TIMEOUT: u8 = 3;
/// More than [`HELD_LIMIT`] arrived while the download was held.
const STOP_SLOW: u8 = 4;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a request waiting for a connection being dialed is given: the connection, or why there
/// is none.
type Ready = Box<dyn FnOnce(Result<(H2Client, u64), Error>) + Send>;

/// One connection to an address, or one being dialed.
struct Conn {
    id: u64,
    /// `None` while it is dialed.
    client: Option<H2Client>,
    /// The requests waiting for it to be dialed.
    waiting: Vec<Ready>,
    /// Streams on it that are not over (those waiting for it included), given-up ones aside.
    streams: usize,
    /// It takes no more streams, and is closed once `streams` is 0.
    retiring: bool,
    /// Since when it has carried no stream.
    idle_since: Option<Instant>,
    /// Its streams with a deadline, for the tick.
    timed: Vec<Arc<Stream>>,
}

impl Conn {
    fn open(&self) -> bool {
        self.client.as_ref().is_none_or(H2Client::is_open)
    }
}

#[derive(Default)]
struct State {
    list: Vec<Conn>,
    next_id: u64,
    /// A tick is scheduled.
    ticking: bool,
}

/// The h2 connections to one address.
#[derive(Default)]
pub(crate) struct Conns {
    state: Mutex<State>,
}

impl Conns {
    /// Closes the connections that carry no stream: the pool is closed.
    pub(crate) fn close_idle(&self) {
        let closing: Vec<H2Client> = {
            let mut st = lock(&self.state);
            let mut out = Vec::new();
            st.list.retain_mut(|c| {
                if c.streams == 0 {
                    out.extend(c.client.take());
                    false
                } else {
                    c.retiring = true;
                    true
                }
            });
            out
        };
        for c in closing {
            c.close();
        }
    }

    /// The connections open or being dialed, and the streams on them.
    pub(crate) fn counts(&self) -> (usize, usize) {
        let st = lock(&self.state);
        (st.list.len(), st.list.iter().map(|c| c.streams).sum())
    }
}

/// How the pool reaches an h2 or h2c upstream.
pub(crate) struct Transport {
    pub(crate) opts: H2Options,
    /// With h2: the TLS configuration, or why there is none.
    pub(crate) tls: Option<Result<TlsClientConfig, String>>,
}

/// The TLS configuration of an h2 upstream with the system's roots and ALPN `h2`. Reads the
/// roots from disk: run when the config is compiled, never on an I/O thread.
pub(crate) fn system_tls() -> Result<TlsClientConfig, String> {
    let roots = akio::tls::RootCerts::system().map_err(|e| e.to_string())?;
    TlsClientConfig::builder()
        .roots(roots)
        .alpn(["h2"])
        .build()
        .map_err(|e| e.to_string())
}

/// The host and port of `address` (`host:port`, `[v6]:port`).
fn split(address: &str) -> Option<(&str, u16)> {
    let (host, port) = address.rsplit_once(':')?;
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    Some((host, port.parse().ok()?))
}

fn io_error(e: &io::Error) -> Error {
    Error::Client(ClientError::Io(io::Error::new(e.kind(), e.to_string())))
}

impl Inner {
    /// A stream to `backend`'s address, given to `ready` once its connection is there: on a
    /// connection with one free, or on one dialed for it. Runs `ready` on this thread when a
    /// connection is open, or on the thread that dialed one.
    fn lease(self: &Arc<Self>, backend: usize, ready: Ready) {
        let Some(b) = self.backends.get(backend) else {
            return ready(Err(Error::NoEngine));
        };
        let now = Instant::now();
        let (got, dial, closing) = {
            let mut st = lock(&b.h2.state);
            // Connections that died carrying nothing, and idle ones past their time, go.
            let idle = self.opts.idle_timeout.unwrap_or(Duration::MAX);
            let mut closing = Vec::new();
            st.list.retain_mut(|c| {
                let stale = c.idle_since.is_some_and(|t| now.duration_since(t) >= idle);
                if c.streams == 0 && c.client.is_some() && (!c.open() || stale) {
                    closing.extend(c.client.take());
                    false
                } else {
                    true
                }
            });
            let found = st
                .list
                .iter_mut()
                .find(|c| !c.retiring && c.streams < STREAMS_PER_CONN && c.open());
            match found {
                Some(c) => {
                    c.streams = c.streams.saturating_add(1);
                    c.idle_since = None;
                    match &c.client {
                        Some(client) => (Some((client.clone(), c.id, ready)), None, closing),
                        None => {
                            c.waiting.push(ready);
                            (None, None, closing)
                        }
                    }
                }
                None => {
                    let id = st.next_id;
                    st.next_id = st.next_id.wrapping_add(1);
                    st.list.push(Conn {
                        id,
                        client: None,
                        waiting: vec![ready],
                        streams: 1,
                        retiring: false,
                        idle_since: None,
                        timed: Vec::new(),
                    });
                    (None, Some(id), closing)
                }
            }
        };
        for c in closing {
            c.close();
        }
        if let Some((client, id, ready)) = got {
            ready(Ok((client, id)));
        }
        if let Some(id) = dial {
            self.dial(backend, id);
        }
    }

    /// Dials connection `id` to `backend`'s address, and hands it to the requests waiting for it.
    fn dial(self: &Arc<Self>, backend: usize, id: u64) {
        let (Some(b), Some(engine), Some(t)) = (
            self.backends.get(backend),
            self.engine.get(),
            self.h2.as_ref(),
        ) else {
            return self.dialed(backend, id, Err(Error::NoEngine));
        };
        let Some((host, port)) = split(&b.address) else {
            let e = ClientError::InvalidUrl(b.address.clone());
            return self.dialed(backend, id, Err(Error::Client(e)));
        };
        let tls = match &t.tls {
            None => None,
            Some(Ok(cfg)) => Some((cfg.clone(), host)),
            Some(Err(why)) => {
                let e = io::Error::other(format!("upstream TLS: {why}"));
                return self.dialed(backend, id, Err(Error::Client(ClientError::Io(e))));
            }
        };
        let me = Arc::clone(self);
        // akio resolves a name off this thread, and dials within the engine's connect timeout
        // (akio API request A53 asks for the upstream's own).
        H2Client::connect(
            &Arc::clone(engine) as &Arc<Engine>,
            host,
            port,
            tls,
            t.opts.clone(),
            move |r| {
                let r = r.map_err(|e| io_error(&e));
                me.dialed(backend, id, r);
            },
        );
    }

    /// Connection `id` to `backend` is open, or could not be.
    fn dialed(self: &Arc<Self>, backend: usize, id: u64, r: Result<H2Client, Error>) {
        let Some(b) = self.backends.get(backend) else {
            return;
        };
        let waiting = {
            let mut st = lock(&b.h2.state);
            let Some(i) = st.list.iter().position(|c| c.id == id) else {
                drop(st);
                if let Ok(c) = r {
                    c.close();
                }
                return;
            };
            match &r {
                Ok(client) => match st.list.get_mut(i) {
                    Some(c) => {
                        c.client = Some(client.clone());
                        std::mem::take(&mut c.waiting)
                    }
                    None => Vec::new(),
                },
                Err(_) => st.list.swap_remove(i).waiting,
            }
        };
        match r {
            Ok(client) => {
                for w in waiting {
                    w(Ok((client.clone(), id)));
                }
            }
            Err(e) => {
                let mut waiting = waiting.into_iter();
                // Each gets the error; the last gets this one, the others copies of it.
                let last = waiting.next_back();
                for w in waiting {
                    w(Err(copy(&e)));
                }
                if let Some(w) = last {
                    w(Err(e));
                }
            }
        }
    }

    /// A stream on connection `id` to `backend` is over: given up (`retire`: the connection takes
    /// no more, as when its stream is left running) or not.
    fn stream_done(
        self: &Arc<Self>,
        backend: usize,
        id: u64,
        stream: Option<&Arc<Stream>>,
        retire: bool,
    ) {
        let Some(b) = self.backends.get(backend) else {
            return;
        };
        let now = Instant::now();
        let (close, tick) = {
            let mut st = lock(&b.h2.state);
            let max_idle = self.opts.max_idle_per_host;
            let idle = st
                .list
                .iter()
                .filter(|c| c.streams == 0 && c.client.is_some())
                .count();
            let Some(i) = st.list.iter().position(|c| c.id == id) else {
                return;
            };
            let Some(c) = st.list.get_mut(i) else {
                return;
            };
            if let Some(s) = stream {
                if let Some(j) = c.timed.iter().position(|t| Arc::ptr_eq(t, s)) {
                    c.timed.swap_remove(j);
                }
            }
            c.streams = c.streams.saturating_sub(1);
            c.retiring |= retire;
            let mut close = None;
            if c.streams == 0 {
                if c.retiring
                    || !c.open()
                    || idle >= max_idle
                    || self.closing.load(Ordering::SeqCst)
                {
                    close = st.list.swap_remove(i).client;
                } else {
                    c.idle_since = Some(now);
                }
            }
            let tick = !st.ticking && !st.list.is_empty();
            if tick {
                st.ticking = true;
            }
            (close, tick)
        };
        if let Some(c) = close {
            c.close();
        }
        if tick {
            self.schedule_tick(backend);
        }
    }

    /// Registers `s`, on connection `id` to `backend`, for the tick to give up past its deadline.
    fn track(self: &Arc<Self>, backend: usize, id: u64, s: &Arc<Stream>) {
        let Some(b) = self.backends.get(backend) else {
            return;
        };
        let tick = {
            let mut st = lock(&b.h2.state);
            if let Some(c) = st.list.iter_mut().find(|c| c.id == id) {
                c.timed.push(Arc::clone(s));
            }
            let tick = !st.ticking;
            st.ticking = true;
            tick
        };
        if tick {
            self.schedule_tick(backend);
        }
    }

    fn schedule_tick(self: &Arc<Self>, backend: usize) {
        let weak: Weak<Inner> = Arc::downgrade(self);
        let scheduled = crate::timer::after(TICK, move || {
            if let Some(inner) = weak.upgrade() {
                inner.tick(backend);
            }
        });
        if !scheduled {
            // The timers are full: the next stream that starts or ends tries again.
            if let Some(b) = self.backends.get(backend) {
                lock(&b.h2.state).ticking = false;
            }
        }
    }

    /// Gives up `backend`'s streams past their deadline and closes its connections idle past
    /// `idle_timeout`; runs again while it has connections.
    fn tick(self: &Arc<Self>, backend: usize) {
        let Some(b) = self.backends.get(backend) else {
            return;
        };
        let now = Instant::now();
        let idle = self.opts.idle_timeout.unwrap_or(Duration::MAX);
        let (late, closing, again) = {
            let mut st = lock(&b.h2.state);
            let mut late = Vec::new();
            for c in &st.list {
                late.extend(c.timed.iter().filter(|s| s.deadline <= now).cloned());
            }
            let mut closing = Vec::new();
            st.list.retain_mut(|c| {
                let stale = c.idle_since.is_some_and(|t| now.duration_since(t) >= idle);
                if c.streams == 0 && c.client.is_some() && (stale || !c.open()) {
                    closing.extend(c.client.take());
                    false
                } else {
                    true
                }
            });
            let again = !st.list.is_empty();
            st.ticking = again;
            (late, closing, again)
        };
        for c in closing {
            c.close();
        }
        for s in late {
            s.stop(STOP_TIMEOUT);
        }
        if again {
            self.schedule_tick(backend);
        }
    }

    /// Sends `req` (its target in origin form) to `backend` over h2, its slot held, the exchange
    /// bounded by `left` from now; `whole` gets the response whole, else `x` as it arrives.
    pub(crate) fn start_h2(
        self: &Arc<Self>,
        backend: usize,
        req: ClientRequest,
        deliver: crate::Deliver,
        since: Instant,
        left: Duration,
    ) {
        let me = Arc::clone(self);
        self.lease(
            backend,
            Box::new(move |r| match r {
                Err(e) => me.fail_unsent(backend, deliver, since, e),
                Ok((client, id)) => me.begin(backend, id, &client, req, deliver, since, left),
            }),
        );
    }

    /// A request that got no stream: its slot back, counted, and failed.
    fn fail_unsent(
        self: &Arc<Self>,
        backend: usize,
        deliver: crate::Deliver,
        since: Instant,
        e: Error,
    ) {
        self.stats.responses.observe(since.elapsed());
        self.release(backend);
        self.record(backend, e.is_upstream_failure());
        deliver.fail(e);
        self.finish_one();
    }

    #[allow(clippy::too_many_arguments)]
    fn begin(
        self: &Arc<Self>,
        backend: usize,
        id: u64,
        client: &H2Client,
        mut req: ClientRequest,
        deliver: crate::Deliver,
        since: Instant,
        left: Duration,
    ) {
        let Some(b) = self.backends.get(backend) else {
            return;
        };
        let scheme = if matches!(self.h2.as_ref().map(|t| t.tls.is_some()), Some(true)) {
            "https"
        } else {
            "http"
        };
        // HTTP/2 has no Host field: the request's `Host`, or the address, is its `:authority`,
        // which akio takes from the URL (the connection is the one leased, whatever it names).
        let authority = req
            .headers
            .get("host")
            .map(str::to_owned)
            .unwrap_or_else(|| b.address.clone());
        req.headers.remove("host");
        req.url = format!("{scheme}://{authority}{}", req.url);
        let streamed = req.streamed.take();
        if let Some(StreamedBody::Length(n)) = streamed {
            if req.headers.get("content-length").is_none() {
                req.headers.append("Content-Length", n.to_string());
            }
        }
        let x = match deliver {
            crate::Deliver::Whole(done) => {
                req.timeout = Some(left);
                let me = Arc::clone(self);
                return client.request(req, move |r: Result<ClientResponse, ClientError>| {
                    me.stats.responses.observe(since.elapsed());
                    // Off its connection before the slot goes to a waiting request, which then
                    // finds the stream free.
                    let closed = matches!(r, Err(ClientError::Closed));
                    me.stream_done(backend, id, None, closed);
                    me.release(backend);
                    me.record(backend, r.is_err());
                    done(r.map_err(Error::Client));
                    me.finish_one();
                });
            }
            crate::Deliver::Stream(x) => x,
        };
        req.timeout = None;
        let s = Arc::new(Stream {
            pool: Arc::clone(self),
            backend,
            conn: id,
            since,
            deadline: Instant::now()
                .checked_add(left)
                .unwrap_or_else(Instant::now),
            x: Mutex::new(Some(x)),
            stop: AtomicU8::new(STOP_NONE),
            headed: AtomicBool::new(false),
            held: AtomicBool::new(false),
            held_bytes: AtomicUsize::new(0),
            ended: AtomicBool::new(false),
        });
        self.track(backend, id, &s);
        let mut trailers = Headers::new();
        let ev = Arc::clone(&s);
        let up = client.open(req, move |e| ev.event(e, &mut trailers));
        if streamed.is_some() {
            let body = crate::Upload::h2(Body {
                up: Some(up),
                stream: Arc::clone(&s),
            });
            {
                let mut g = lock(&s.x);
                match g.as_mut() {
                    Some(x) => x.on_request_body(body),
                    None => drop(body),
                }
            }
            s.check_stop();
        } else {
            up.finish();
        }
    }
}

/// A copy of `e`, for each of the requests a failed dial fails.
fn copy(e: &Error) -> Error {
    match e {
        Error::Client(ClientError::Io(x)) => io_error(x),
        Error::Client(ClientError::InvalidUrl(u)) => {
            Error::Client(ClientError::InvalidUrl(u.clone()))
        }
        Error::Client(ClientError::Timeout) => Error::Client(ClientError::Timeout),
        Error::Busy => Error::Busy,
        Error::Closed => Error::Closed,
        Error::NoEngine => Error::NoEngine,
        Error::Timeout => Error::Timeout,
        Error::Client(_) => Error::Client(ClientError::Closed),
    }
}

/// One streamed exchange on an h2 connection: the [`Exchange`] it reports to, until it is over.
pub(crate) struct Stream {
    pool: Arc<Inner>,
    backend: usize,
    conn: u64,
    /// When the request began waiting, for the time to its response's head.
    since: Instant,
    deadline: Instant,
    /// Taken by whoever ends the exchange; held while one of its methods runs.
    x: Mutex<Option<Box<dyn Exchange>>>,
    /// Why it is to be given up (`STOP_*`), set before the one holding `x` is told.
    stop: AtomicU8,
    headed: AtomicBool,
    held: AtomicBool,
    /// What arrived while the download was held.
    held_bytes: AtomicUsize,
    /// `x` was taken: the exchange is over (read without its lock, which a body written from
    /// inside one of `x`'s methods cannot take).
    ended: AtomicBool,
}

impl Stream {
    /// akio's report of the response, on the connection's I/O thread.
    fn event(self: &Arc<Self>, e: H2Event<'_>, trailers: &mut Headers) {
        match e {
            H2Event::Head { status, headers } => {
                {
                    let mut g = lock(&self.x);
                    if let Some(x) = g.as_mut() {
                        self.headed.store(true, Ordering::SeqCst);
                        self.pool.stats.responses.observe(self.since.elapsed());
                        let head = Head {
                            status,
                            headers: headers.clone(),
                        };
                        x.on_head(head, crate::Download::h2(Arc::clone(self)));
                    }
                }
                self.check_stop();
            }
            H2Event::Data(d) => {
                if self.held.load(Ordering::SeqCst) {
                    let n = self
                        .held_bytes
                        .fetch_add(d.len(), Ordering::SeqCst)
                        .saturating_add(d.len());
                    if n > HELD_LIMIT {
                        self.stop(STOP_SLOW);
                        return;
                    }
                }
                {
                    let mut g = lock(&self.x);
                    if let Some(x) = g.as_mut() {
                        x.on_data(d);
                    }
                }
                self.check_stop();
            }
            H2Event::Trailers(t) => *trailers = t.clone(),
            H2Event::End => {
                let Some(mut x) = self.take(lock(&self.x).take()) else {
                    return;
                };
                // Off its connection before the slot goes to a waiting request.
                self.pool
                    .stream_done(self.backend, self.conn, Some(self), false);
                self.pool.release(self.backend);
                self.pool.record(self.backend, false);
                x.on_end(trailers);
                self.pool.finish_one();
            }
            H2Event::Error(e) => {
                let Some(mut x) = self.take(lock(&self.x).take()) else {
                    return;
                };
                let headed = self.headed.load(Ordering::SeqCst);
                if !headed {
                    self.pool.stats.responses.observe(self.since.elapsed());
                }
                // Refused as closed (as after a GOAWAY): the connection takes no more.
                let retire = matches!(e, ClientError::Closed);
                self.pool
                    .stream_done(self.backend, self.conn, Some(self), retire);
                self.pool.release(self.backend);
                let failed =
                    !(headed && self.stop.load(Ordering::SeqCst) == STOP_CANCEL) && !x.gave_up();
                self.pool.record(self.backend, failed);
                x.on_error(Error::Client(e));
                self.pool.finish_one();
            }
        }
    }

    /// Asks for the exchange to be given up for `why`, unless it is over or asked already.
    fn stop(self: &Arc<Self>, why: u8) {
        if self
            .stop
            .compare_exchange(STOP_NONE, why, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            self.check_stop();
        }
    }

    /// Gives the exchange up if it was asked to be and nobody holds it: whoever holds it calls
    /// this once done, so one of the two does.
    fn check_stop(self: &Arc<Self>) {
        let why = self.stop.load(Ordering::SeqCst);
        if why == STOP_NONE {
            return;
        }
        let x = match self.x.try_lock() {
            Ok(mut g) => g.take(),
            Err(TryLockError::Poisoned(p)) => p.into_inner().take(),
            Err(TryLockError::WouldBlock) => return,
        };
        let Some(mut x) = self.take(x) else { return };
        let headed = self.headed.load(Ordering::SeqCst);
        if !headed {
            self.pool.stats.responses.observe(self.since.elapsed());
        }
        // Left running (akio cannot reset it, A51): its connection takes no more streams and is
        // closed once the others are over.
        self.pool
            .stream_done(self.backend, self.conn, Some(self), true);
        self.pool.release(self.backend);
        // Only the timeout is the address's failure: the others are the client's doing.
        self.pool
            .record(self.backend, why == STOP_TIMEOUT && !x.gave_up());
        let e = if why == STOP_TIMEOUT {
            ClientError::Timeout
        } else {
            ClientError::Closed
        };
        x.on_error(Error::Client(e));
        self.pool.finish_one();
    }

    pub(crate) fn hold_reads(&self, hold: bool) {
        if !hold {
            self.held_bytes.store(0, Ordering::SeqCst);
        }
        self.held.store(hold, Ordering::SeqCst);
    }

    pub(crate) fn cancel(self: &Arc<Self>) {
        self.stop(STOP_CANCEL);
    }

    /// Takes the exchange, to end it: `None` when another did.
    fn take(&self, g: Option<Box<dyn Exchange>>) -> Option<Box<dyn Exchange>> {
        if g.is_some() {
            self.ended.store(true, Ordering::SeqCst);
        }
        g
    }

    /// Whether the exchange is over, or being given up.
    fn over(&self) -> bool {
        self.stop.load(Ordering::SeqCst) != STOP_NONE || self.ended.load(Ordering::SeqCst)
    }
}

/// A request body streamed to an h2 upstream.
pub(crate) struct Body {
    up: Option<H2Upload>,
    stream: Arc<Stream>,
}

impl Body {
    pub(crate) fn write(&self, data: &[u8]) -> bool {
        match &self.up {
            Some(up) if !self.stream.over() => up.write(data),
            _ => false,
        }
    }

    pub(crate) fn finish(mut self) -> bool {
        self.up.take().is_some_and(|up| up.finish())
    }
}

impl Drop for Body {
    /// Dropped unfinished, the body is its client gone: the exchange is given up.
    fn drop(&mut self) {
        if self.up.take().is_some() {
            self.stream.stop(STOP_GAVE_UP);
        }
    }
}
