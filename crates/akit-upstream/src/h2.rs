//! Requests to an h2 or h2c upstream (I18, I18b), on akio's [`H2Client`]: the connections to
//! each address, the streams on them, and an exchange's two bodies.
//!
//! Each address has its own connections ([`Conns`]). A request takes a stream on the first
//! connection with one free, by the server's `SETTINGS_MAX_CONCURRENT_STREAMS`
//! ([`H2Client::max_concurrent_streams`]; [`STREAMS_PER_CONN`] until the server says, or when it
//! sets no limit), and a connection is dialed when none has one, while the address has fewer
//! than `max_conns` open. Past that, the request goes to the open connection with the fewest
//! streams, where akio holds it until one of them ends (`H2Options::max_pending_streams`, its
//! timeouts and cancel applying meanwhile). The pool's slots bound the streams under way to an
//! address at `max_conns` times [`STREAMS_PER_CONN`]. A connection that has carried its last
//! stream stays for the next request, as an HTTP/1 one does: at most `max_idle_conns` of them to
//! an address, each for `idle_timeout`, closed by a job of akit's timer thread ([`crate::timer`])
//! that runs every [`TICK`] while the address has an idle connection. A connection that is no
//! longer open (closed, or after the server's `GOAWAY`: [`H2Client::is_open`]) takes no more
//! streams and is closed once its last is over.
//!
//! akio does the rest (its ADR-110 and ADR-111, akio API requests A51–A53 and A66):
//!
//! - **Giving a stream up** resets it (`RST_STREAM CANCEL`) and leaves its connection to the
//!   others: the client gone after the head ([`crate::Download::cancel`]), the request's body
//!   dropped unfinished (its client went mid-upload), the request's [`CancelToken`]
//!   (`ClientRequest::cancel`) at any point. Each fails the exchange with
//!   `ClientError::Cancelled`, which is not the address's failure.
//! - **Timeouts**: the exchange's `response_timeout` (`ClientRequest::timeout`), and the
//!   upstream's `response_header_timeout` (from the request's END_STREAM) and
//!   `response_idle_timeout` (between DATA frames, the time the download is held aside), each
//!   resetting the stream alone.
//! - **Backpressure** both ways: a held download keeps the stream's window closed
//!   (`H2Stream::hold_reads`), and an upload is paced by what waits in its stream
//!   (`H2Upload::queued_bytes`, `on_drain`), as over HTTP/1.1.
//! - **Request trailers** end the body (`H2Upload::finish_with_trailers`).
//! - **The dial** is bounded by the upstream's `connect_timeout` (`H2Options::happy_eyeballs`), per
//!   address raced when its name resolves to several; a lone address is dialed within the
//!   engine's, as over HTTP/1.1 (akio API request A12). With `h2`, the TLS handshake offers `h2` and `http/1.1` in ALPN, as fit's does: when the
//!   server chooses `http/1.1` (or nothing), akio fails the dial with [`NotH2`], and the address
//!   is spoken HTTP/1.1 over TLS by the pool's HTTP/1 client from then on, the requests that
//!   waited for the dial included.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use akio::Engine;
use akio::tls::ClientConfig as TlsClientConfig;
use akio_http::Headers;
use akio_http::client::{ClientError, ClientRequest, ClientResponse, StreamedBody};
use akio_http::h2client::{H2Client, H2Event, H2Options, H2Stream, H2Upload, NotH2};

#[cfg(doc)]
use akio_http::client::CancelToken;

use crate::{Error, Exchange, Head, Inner};

/// The streams a request opens on one connection at once until the server says how many it
/// allows (or when it sets no limit): the least RFC 9113 (§6.5.2) lets a server advertise, and
/// Go's transport's guess. A pool's slots bound an address's streams at `max_conns` times it.
pub const STREAMS_PER_CONN: usize = 100;

/// How often an address's idle connections are checked for their `idle_timeout`, while it has
/// one.
pub const TICK: Duration = Duration::from_millis(100);

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
    /// Streams on it that are not over (those waiting for it included).
    streams: usize,
    /// It takes no more streams, and is closed once `streams` is 0 (the pool is closing).
    retiring: bool,
    /// Since when it has carried no stream.
    idle_since: Option<Instant>,
}

impl Conn {
    fn open(&self) -> bool {
        self.client.as_ref().is_none_or(H2Client::is_open)
    }

    /// Whether it may take a stream: open, not retiring.
    fn usable(&self) -> bool {
        !self.retiring && self.open()
    }

    /// The streams it may carry at once: the server's limit, once it said one.
    fn cap(&self) -> usize {
        match self.client.as_ref().map(H2Client::max_concurrent_streams) {
            None | Some(u32::MAX) => STREAMS_PER_CONN,
            Some(n) => usize::try_from(n).unwrap_or(usize::MAX),
        }
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
    /// The server chose HTTP/1.1 in ALPN: requests go to it over HTTP/1.1 (TLS).
    http1: AtomicBool,
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

    /// Whether the address speaks HTTP/1.1 rather than h2 (its server chose it in ALPN).
    pub(crate) fn fell_back(&self) -> bool {
        self.http1.load(Ordering::SeqCst)
    }

    /// The address's server chose HTTP/1.1 (or nothing) in ALPN.
    pub(crate) fn fall_back(&self) {
        self.http1.store(true, Ordering::SeqCst);
    }
}

/// How the pool reaches an h2 or h2c upstream.
pub(crate) struct Transport {
    pub(crate) opts: H2Options,
    /// With h2: the TLS configuration, or why there is none.
    pub(crate) tls: Option<Result<TlsClientConfig, String>>,
    /// With h2: `tls_server_name` (I54), the name sent in SNI and checked against the servers'
    /// certificates in place of each address's host; `None` when left out.
    pub(crate) server_name: Option<String>,
}

impl Transport {
    /// The name TLS to `host` sends in SNI and checks the certificate against.
    pub(crate) fn tls_name<'a>(&'a self, host: &'a str) -> &'a str {
        self.server_name.as_deref().unwrap_or(host)
    }
}

/// The TLS configuration of an h2 upstream with the system's roots, offering `h2` and
/// `http/1.1` in ALPN, as fit's does. Reads the roots from disk: run when the config is compiled,
/// never on an I/O thread.
pub(crate) fn system_tls() -> Result<TlsClientConfig, String> {
    let roots = akio::tls::RootCerts::system().map_err(|e| e.to_string())?;
    TlsClientConfig::builder()
        .roots(roots)
        .alpn(["h2", "http/1.1"])
        .build()
        .map_err(|e| e.to_string())
}

/// `c` with an `h2` pool's TLS, and its `tls_server_name`, for the addresses whose server chose
/// HTTP/1.1 in ALPN, and their probes; `c` as it is for other pools.
pub(crate) fn http1_tls(
    t: Option<&Transport>,
    c: akio_http::client::Client,
) -> akio_http::client::Client {
    let Some(t) = t else { return c };
    let c = match &t.tls {
        Some(Ok(tls)) => c.with_tls(tls.clone()),
        _ => c,
    };
    match &t.server_name {
        Some(name) => c.with_tls_server_name(name.clone()),
        None => c,
    }
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
    /// connection with one free, on one dialed for it, or, once the address has `max_conns`, on
    /// the one with the fewest streams. Runs `ready` on this thread when a connection is open,
    /// or on the thread that dialed one.
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
            let live = st.list.iter().filter(|c| c.usable()).count();
            let free = st
                .list
                .iter()
                .position(|c| c.usable() && c.streams < c.cap());
            // All full and as many as allowed: akio holds the stream on the least busy one.
            let found = free.or_else(|| {
                (live >= self.max_conns)
                    .then(|| {
                        st.list
                            .iter()
                            .enumerate()
                            .filter(|(_, c)| c.usable())
                            .min_by_key(|(_, c)| c.streams)
                            .map(|(i, _)| i)
                    })
                    .flatten()
            });
            match found.and_then(|i| st.list.get_mut(i)) {
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
            Some(Ok(cfg)) => Some((cfg.clone(), t.tls_name(host))),
            Some(Err(why)) => {
                let e = io::Error::other(format!("upstream TLS: {why}"));
                return self.dialed(backend, id, Err(Error::Client(ClientError::Io(e))));
            }
        };
        let me = Arc::clone(self);
        // akio resolves a name off this thread, and races its addresses within the upstream's
        // connect timeout (`t.opts.happy_eyeballs`; a lone one, the engine's: A12).
        H2Client::connect(
            &Arc::clone(engine) as &Arc<Engine>,
            host,
            port,
            tls,
            t.opts.clone(),
            move |r| {
                let r = r.map_err(|e| {
                    if NotH2::of(&e).is_some() {
                        // The waiting requests go over HTTP/1.1 (`start_h2`), and those after.
                        if let Some(b) = me.backends.get(backend) {
                            b.h2.fall_back();
                        }
                    }
                    io_error(&e)
                });
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

    /// A stream on connection `id` to `backend` is over.
    fn stream_done(self: &Arc<Self>, backend: usize, id: u64) {
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
            c.streams = c.streams.saturating_sub(1);
            let mut close = None;
            let mut tick = false;
            if c.streams == 0 {
                if c.retiring
                    || !c.open()
                    || idle >= max_idle
                    || self.closing.load(Ordering::SeqCst)
                {
                    close = st.list.swap_remove(i).client;
                } else {
                    c.idle_since = Some(now);
                    tick = !std::mem::replace(&mut st.ticking, true);
                }
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

    fn schedule_tick(self: &Arc<Self>, backend: usize) {
        let weak: Weak<Inner> = Arc::downgrade(self);
        let scheduled = crate::timer::after(TICK, move || {
            if let Some(inner) = weak.upgrade() {
                inner.tick(backend);
            }
        });
        if !scheduled {
            // The timers are full: the next connection that goes idle tries again.
            if let Some(b) = self.backends.get(backend) {
                lock(&b.h2.state).ticking = false;
            }
        }
    }

    /// Closes `backend`'s connections idle past `idle_timeout`, and those that died idle; runs
    /// again while it has an idle connection.
    fn tick(self: &Arc<Self>, backend: usize) {
        let Some(b) = self.backends.get(backend) else {
            return;
        };
        let now = Instant::now();
        let idle = self.opts.idle_timeout.unwrap_or(Duration::MAX);
        let (closing, again) = {
            let mut st = lock(&b.h2.state);
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
            let again = st.list.iter().any(|c| c.idle_since.is_some());
            st.ticking = again;
            (closing, again)
        };
        for c in closing {
            c.close();
        }
        if again {
            self.schedule_tick(backend);
        }
    }

    /// Sends `req` (its target in origin form) to `backend` over h2, its slot held, the exchange
    /// bounded by `left` from now; `whole` gets the response whole, else `x` as it arrives. An
    /// address whose server chose HTTP/1.1 in ALPN is sent it over HTTP/1.1 instead.
    pub(crate) fn start_h2(
        self: &Arc<Self>,
        backend: usize,
        req: ClientRequest,
        deliver: crate::Deliver,
        since: Instant,
        left: Duration,
    ) {
        let me = Arc::clone(self);
        let deadline = Instant::now().checked_add(left);
        self.lease(
            backend,
            Box::new(move |r| match r {
                Err(e) => {
                    let fell_back = me.backends.get(backend).is_some_and(|b| b.h2.fell_back());
                    let left =
                        deadline.map_or(left, |d| d.saturating_duration_since(Instant::now()));
                    if fell_back && !left.is_zero() {
                        me.start_http1(backend, req, deliver, since, left);
                    } else {
                        me.fail_unsent(backend, deliver, since, e);
                    }
                }
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
        // akio bounds the exchange, its head and its silences, and heeds the request's token.
        req.timeout = Some(left);
        req.response_header_timeout = req.response_header_timeout.or(Some(self.header_timeout()));
        req.response_idle_timeout = req.response_idle_timeout.or(Some(self.idle_timeout()));
        let x = match deliver {
            crate::Deliver::Whole(done) => {
                let me = Arc::clone(self);
                return client.request(req, move |r: Result<ClientResponse, ClientError>| {
                    me.stats.responses.observe(since.elapsed());
                    // Off its connection before the slot goes to a waiting request, which then
                    // finds the stream free.
                    me.stream_done(backend, id);
                    me.release(backend);
                    me.record(
                        backend,
                        r.as_ref()
                            .is_err_and(|e| !matches!(e, ClientError::Cancelled)),
                    );
                    done(r.map_err(Error::Client));
                    me.finish_one();
                });
            }
            crate::Deliver::Stream(x) => x,
        };
        let s = Arc::new(Stream {
            pool: Arc::clone(self),
            backend,
            conn: id,
            since,
            x: Mutex::new(Some(x)),
            headed: AtomicBool::new(false),
            ctl: Mutex::new(Ctl::default()),
        });
        let mut trailers = Headers::new();
        let ev = Arc::clone(&s);
        let up = client.open(req, move |e| ev.event(e, &mut trailers));
        s.attach(up.stream());
        if streamed.is_some() {
            let body = crate::Upload::h2(Body { up: Some(up) });
            let mut g = lock(&s.x);
            match g.as_mut() {
                Some(x) => x.on_request_body(body),
                None => drop(body),
            }
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

/// What the download's holder asked of a stream, kept until akio's handle to it is there.
#[derive(Default)]
struct Ctl {
    h2: Option<H2Stream>,
    held: bool,
    cancelled: bool,
}

/// One streamed exchange on an h2 connection: the [`Exchange`] it reports to, until it is over.
pub(crate) struct Stream {
    pool: Arc<Inner>,
    backend: usize,
    conn: u64,
    /// When the request began waiting, for the time to its response's head.
    since: Instant,
    /// Taken by whichever event ends the exchange; held while one of its methods runs.
    x: Mutex<Option<Box<dyn Exchange>>>,
    headed: AtomicBool,
    /// akio's handle to the stream, which `client.open` gives after its events may have begun.
    ctl: Mutex<Ctl>,
}

impl Stream {
    /// akio's handle to the stream: what was asked of it before is done now.
    fn attach(&self, h: H2Stream) {
        let mut c = lock(&self.ctl);
        if c.held {
            h.hold_reads(true);
        }
        if c.cancelled {
            h.cancel();
        }
        c.h2 = Some(h);
    }

    /// akio's report of the response, on the connection's I/O thread.
    fn event(self: &Arc<Self>, e: H2Event<'_>, trailers: &mut Headers) {
        match e {
            H2Event::Head { status, headers } => {
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
            H2Event::Data(d) => {
                let mut g = lock(&self.x);
                if let Some(x) = g.as_mut() {
                    x.on_data(d);
                }
            }
            H2Event::Trailers(t) => *trailers = t.clone(),
            H2Event::End => {
                let Some(mut x) = lock(&self.x).take() else {
                    return;
                };
                // Off its connection before the slot goes to a waiting request.
                self.pool.stream_done(self.backend, self.conn);
                self.pool.release(self.backend);
                self.pool.record(self.backend, false);
                x.on_end(trailers);
                self.pool.finish_one();
            }
            H2Event::Error(e) => {
                let Some(mut x) = lock(&self.x).take() else {
                    return;
                };
                if !self.headed.load(Ordering::SeqCst) {
                    self.pool.stats.responses.observe(self.since.elapsed());
                }
                self.pool.stream_done(self.backend, self.conn);
                self.pool.release(self.backend);
                // Given up for the client (cancelled: gone, its body dropped, its token) is
                // not the address's failure.
                let failed = !matches!(e, ClientError::Cancelled) && !x.gave_up();
                self.pool.record(self.backend, failed);
                x.on_error(Error::Client(e));
                self.pool.finish_one();
            }
        }
    }

    /// Holds or resumes the download: akio keeps the stream's window closed while it is held.
    pub(crate) fn hold_reads(&self, hold: bool) {
        let mut c = lock(&self.ctl);
        c.held = hold;
        if let Some(h) = &c.h2 {
            h.hold_reads(hold);
        }
    }

    /// Gives the response up: akio resets the stream, and the exchange ends with
    /// `ClientError::Cancelled`.
    pub(crate) fn cancel(&self) {
        let mut c = lock(&self.ctl);
        c.cancelled = true;
        if let Some(h) = &c.h2 {
            h.cancel();
        }
    }
}

/// A request body streamed to an h2 upstream.
pub(crate) struct Body {
    up: Option<H2Upload>,
}

impl Body {
    pub(crate) fn write(&self, data: &[u8]) -> bool {
        self.up.as_ref().is_some_and(|up| up.write(data))
    }

    /// Bytes written and not yet in a DATA frame.
    pub(crate) fn queued_bytes(&self) -> usize {
        self.up.as_ref().map_or(0, H2Upload::queued_bytes)
    }

    /// Calls `f` once nothing written waits, or the stream is over.
    pub(crate) fn on_drain(&self, f: impl FnOnce() + Send + 'static) {
        match &self.up {
            Some(up) => up.on_drain(f),
            None => f(),
        }
    }

    /// Ends the body; the header timeout runs from here.
    pub(crate) fn finish(self) -> bool {
        self.finish_with_trailers(Headers::new())
    }

    /// Ends the body with `trailers` after it (none: as [`finish`](Self::finish)).
    pub(crate) fn finish_with_trailers(mut self, trailers: Headers) -> bool {
        self.up
            .take()
            .is_some_and(|up| up.finish_with_trailers(trailers))
    }
}

impl Drop for Body {
    /// Dropped unfinished, the body is its client gone: the stream is reset.
    fn drop(&mut self) {
        if let Some(up) = self.up.take() {
            up.cancel();
        }
    }
}
