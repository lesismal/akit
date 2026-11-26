//! One request on its way through the application (fit's `exchange`), both bodies streamed with
//! linked backpressure:
//!
//! - **Up.** The request's body goes to the application as `STDIN` records as it arrives. While
//!   more than [`WATERMARK`] of it waits in the connection's queue, the client's reads are held
//!   ([`BodyReads`]), and resumed once the queue drained. What arrives before the connection is
//!   there waits here, the client's reads held meanwhile: one read's worth at most. A body past
//!   its bound (the listener's `max_body_bytes`, or a `body_limit`'s: akio's
//!   `Context::limit_body`) is answered 413, or its response cut short if it began.
//! - **Down.** The response's CGI header (`STDOUT` up to the blank line, at most
//!   `max_header_bytes`) gives its status and fields. A response whose end comes in the same
//!   read as its header is sent whole; any other is sent as it arrives
//!   ([`Detached::stream_response`]). While more than [`WATERMARK`] of it waits for the client,
//!   the connection's reads are held, and resumed once the client took it
//!   ([`BodyStream::on_drain`]).
//!
//! Timeouts: `connect_timeout` and `response_header_timeout` until the header's first bytes,
//! `response_idle_timeout` between the response's records (not counted while the client holds
//! the response back), and `timeout` for the whole; the handler's sweep checks them every
//! [`TICK`](super::pool::TICK) ([`Exchange::check`]).
//!
//! Locking: the exchange's state is behind one lock. Under it the exchange queues bytes on the
//! application's connection and writes to the client's response stream, which call nothing back;
//! it answers, starts and ends the client's response, and gives the connection back, only
//! outside it: those may run callbacks (the client gone, a waiting request handed the
//! connection) that take this lock or another exchange's. The callbacks it gives akio
//! (`on_drain`) take no lock of the exchange's.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use akio::Conn;
use akio_http::{BodyReads, BodyStream, Detached, Headers, Response, Version};

use super::Shared;
use super::params::Params;
use super::pool::{DialError, FcgiConn, Link};
use super::record::{self, END_REQUEST, Reader, STDERR, STDIN, STDOUT};

/// How much may wait to be sent on a connection, or to the client, before the side sending it is
/// held back: fit's `watermark`.
pub(crate) const WATERMARK: usize = 64 << 10;

/// The trailer an HTTP/2 or HTTP/3 response of no declared length ends with when the application
/// broke off mid-body: `Akit-Error`, fit's `Fit-Error` renamed (decision 037).
const BROKEN_TRAILER: (&str, &str) = ("Akit-Error", "the FastCGI application's response broke off");

/// Why an exchange ended before its response did.
#[derive(Debug)]
pub(crate) enum Failure {
    /// The client went: nothing to answer.
    ClientGone,
    /// A timeout passed: 504.
    Timeout,
    /// The application failed or broke the protocol: 502. Why, for debugging.
    Bad(#[allow(dead_code, reason = "read through Debug")] String),
    /// Answered with this status (a request refused: 400, 404, 413, 503).
    Status(u16),
}

impl Failure {
    fn status(&self) -> u16 {
        match self {
            Failure::ClientGone | Failure::Bad(_) => 502,
            Failure::Timeout => 504,
            Failure::Status(s) => *s,
        }
    }
}

/// Where the response to the client is.
enum Out {
    /// The CGI header so far.
    Head(Vec<u8>),
    /// The header read, the body so far: sent whole if the response ends in this read, else
    /// streamed once the read is over.
    Collect {
        status: u16,
        headers: Headers,
        body: Vec<u8>,
        declared: bool,
    },
    /// Being started, outside the lock, by the connection's thread.
    Starting,
    /// Sent as it arrives; `framed` says by a declared length.
    Stream { stream: BodyStream, framed: bool },
    /// Over.
    Done,
}

/// The request's body on its way up.
#[derive(Default)]
struct Up {
    /// What arrived before the connection was there.
    pending: Vec<u8>,
    /// The client's body ended (or there is none).
    ended: bool,
    /// The empty `STDIN` record that ends it was sent.
    sent_end: bool,
}

struct State {
    answer: Option<Detached>,
    script: String,
    path_info: String,
    conn: Option<Arc<FcgiConn>>,
    out: Out,
    up: Up,
    done: bool,
    retried: bool,
    /// When the whole exchange times out.
    total: Instant,
    /// When the current phase times out: connecting and the header, then the gaps between
    /// records.
    phase: Instant,
}

/// A request through the application.
pub(crate) struct Exchange {
    pub(crate) h: Arc<Shared>,
    params: Params,
    reads: Option<BodyReads>,
    /// The request may be sent again on a new connection (no body, a method safe to repeat).
    replayable: bool,
    version: Version,
    /// The connection's reads are held for the client.
    held: Arc<AtomicBool>,
    /// The last time the client let the response go on, in milliseconds since `born`: the idle
    /// timeout does not count time the client held it back.
    born: Instant,
    resumed: Arc<AtomicU64>,
    /// The exchange is over (`State::done`), for the sweep to read without the lock.
    finished: AtomicBool,
    st: Mutex<State>,
}

impl std::fmt::Debug for Exchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Exchange").finish_non_exhaustive()
    }
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// fit's answer to a request refused or failed before its response began.
pub(crate) fn error_response(status: u16) -> Response {
    let text = match status {
        400 => "400 Bad Request\n",
        404 => "404 Not Found\n",
        413 => "413 Request Entity Too Large\n",
        503 => "503 Service Unavailable\n",
        504 => "504 Gateway Timeout\n",
        _ => "502 Bad Gateway\n",
    };
    Response::with_body(
        status,
        "text/plain; charset=utf-8",
        text.as_bytes().to_vec(),
    )
}

/// What a new exchange is made of.
pub(crate) struct Parts {
    pub(crate) h: Arc<Shared>,
    pub(crate) params: Params,
    pub(crate) reads: Option<BodyReads>,
    pub(crate) replayable: bool,
    pub(crate) version: Version,
    /// The whole body, when it is not streamed.
    pub(crate) body: Option<Vec<u8>>,
}

impl Exchange {
    pub(crate) fn new(p: Parts) -> Arc<Exchange> {
        let now = Instant::now();
        let cfg = &p.h.cfg;
        let total = now.checked_add(cfg.timeout).unwrap_or(now);
        let phase = now
            .checked_add(cfg.connect_timeout)
            .and_then(|t| t.checked_add(cfg.response_header_timeout))
            .unwrap_or(now);
        let up = match p.body {
            Some(body) => Up {
                pending: body,
                ended: true,
                ..Up::default()
            },
            None => Up::default(),
        };
        Arc::new(Exchange {
            h: p.h,
            params: p.params,
            reads: p.reads,
            replayable: p.replayable,
            version: p.version,
            held: Arc::new(AtomicBool::new(false)),
            born: now,
            resumed: Arc::new(AtomicU64::new(0)),
            finished: AtomicBool::new(false),
            st: Mutex::new(State {
                answer: None,
                script: String::new(),
                path_info: String::new(),
                conn: None,
                out: Out::Head(Vec::new()),
                up,
                done: false,
                retried: false,
                total,
                phase,
            }),
        })
    }

    /// Takes the right to answer the client, and has the handler's sweep watch the exchange's
    /// deadlines. False, the request answered 503, when it cannot.
    pub(crate) fn begin(self: &Arc<Self>, answer: Detached) -> bool {
        lock(&self.st).answer = Some(answer);
        if !self.h.pool.watch(self) {
            self.fail(Failure::Status(503));
            return false;
        }
        true
    }

    /// Whether the exchange is over.
    pub(crate) fn finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    /// The sweep's look at the exchange: it fails once a deadline passed.
    pub(crate) fn check(self: &Arc<Self>, now: Instant) {
        {
            let mut st = lock(&self.st);
            if st.done {
                return;
            }
            if self.held.load(Ordering::Acquire) {
                // The client holds the response back: no gap of the application's.
                st.phase = st.phase.max(
                    now.checked_add(self.h.cfg.response_idle_timeout)
                        .unwrap_or(now),
                );
            } else if matches!(st.out, Out::Stream { .. }) {
                // Time the client held it back does not count either.
                let resumed = self
                    .born
                    .checked_add(Duration::from_millis(self.resumed.load(Ordering::Acquire)))
                    .unwrap_or(now);
                if let Some(t) = resumed.checked_add(self.h.cfg.response_idle_timeout) {
                    st.phase = st.phase.max(t);
                }
            }
            if now < st.total.min(st.phase) {
                return;
            }
        }
        self.fail(Failure::Timeout);
    }

    /// Takes a connection for the request, to run `script` with `path_info`.
    pub(crate) fn start(self: &Arc<Self>, script: String, path_info: String) {
        {
            let mut st = lock(&self.st);
            if st.done {
                return;
            }
            st.script = script;
            st.path_info = path_info;
        }
        let x = self.clone();
        self.h.pool.take(Box::new(move |r| x.connected(r)), true);
    }

    /// Sends the request on the connection it got.
    fn connected(self: &Arc<Self>, r: Result<Arc<FcgiConn>, DialError>) {
        let fc = match r {
            Ok(fc) => fc,
            Err(DialError::Busy | DialError::Closed) => return self.fail(Failure::Status(503)),
            Err(DialError::Timeout) => return self.fail(Failure::Timeout),
            Err(DialError::Io(e)) if e.kind() == std::io::ErrorKind::TimedOut => {
                return self.fail(Failure::Timeout);
            }
            Err(DialError::Io(e)) => return self.fail(Failure::Bad(e.to_string())),
        };
        let mut st = lock(&self.st);
        if st.done {
            drop(st);
            return fc.give_back();
        }
        st.conn = Some(fc.clone());
        let now = Instant::now();
        let at = now
            .checked_add(self.h.cfg.response_header_timeout)
            .unwrap_or(now);
        st.phase = at;
        fc.start(self.clone());
        let params = self
            .params
            .encode(&self.h.cfg.root, &st.script, &st.path_info);
        let mut b = Vec::with_capacity(
            params
                .len()
                .saturating_add(st.up.pending.len())
                .saturating_add(64),
        );
        record::append_begin_request(&mut b);
        record::append_record(&mut b, record::PARAMS, &params);
        record::append_record(&mut b, record::PARAMS, &[]);
        let pending = std::mem::take(&mut st.up.pending);
        if !pending.is_empty() {
            record::append_record(&mut b, STDIN, &pending);
        }
        if st.up.ended {
            record::append_record(&mut b, STDIN, &[]);
            st.up.sent_end = true;
        }
        let paced = !st.up.sent_end;
        if let Err(e) = fc.send(b) {
            drop(st);
            let stale = fc.stale();
            return self.lost(&fc, e.to_string(), stale);
        }
        if paced {
            self.pace(&fc);
        } else if let Some(reads) = &self.reads {
            // Held while it waited for the connection.
            reads.hold(false);
        }
    }

    /// Holds the client's reads while more than [`WATERMARK`] of its body waits for the
    /// application, until that drained; else lets them go on.
    fn pace(&self, fc: &FcgiConn) {
        let Some(reads) = &self.reads else { return };
        if fc.queued_bytes() > WATERMARK {
            reads.hold(true);
            let reads = reads.clone();
            // At once, on this thread, if it drained meanwhile.
            fc.on_drain(move || reads.hold(false));
        } else {
            reads.hold(false);
        }
    }

    /// A piece of the client's streamed body.
    pub(crate) fn body_chunk(self: &Arc<Self>, data: &[u8]) {
        let mut st = lock(&self.st);
        if st.done {
            return;
        }
        let Some(fc) = st.conn.clone() else {
            // Not connected yet: kept, the reads held until it is sent.
            st.up.pending.extend_from_slice(data);
            if let Some(reads) = &self.reads {
                reads.hold(true);
            }
            return;
        };
        let mut b = Vec::with_capacity(data.len().saturating_add(16));
        record::append_record(&mut b, STDIN, data);
        if let Err(e) = fc.send(b) {
            drop(st);
            return self.fail(Failure::Bad(e.to_string()));
        }
        self.pace(&fc);
    }

    /// The client's streamed body ended.
    pub(crate) fn body_end(self: &Arc<Self>) {
        let mut st = lock(&self.st);
        if st.done {
            return;
        }
        st.up.ended = true;
        let Some(fc) = st.conn.clone() else { return };
        let mut b = Vec::with_capacity(8);
        record::append_record(&mut b, STDIN, &[]);
        st.up.sent_end = true;
        if let Err(e) = fc.send(b) {
            drop(st);
            self.fail(Failure::Bad(e.to_string()));
        }
    }

    /// A part of the CGI response: its header, until the blank line after it, then its body.
    /// It runs on the connection's thread.
    fn stdout(self: &Arc<Self>, fc: &Arc<FcgiConn>, p: &[u8]) -> Result<(), Failure> {
        let mut st = lock(&self.st);
        if st.done || p.is_empty() {
            return Ok(());
        }
        let now = Instant::now();
        if let Some(at) = now.checked_add(self.h.cfg.response_idle_timeout) {
            st.phase = at;
        }
        match &mut st.out {
            Out::Head(head) => {
                head.extend_from_slice(p);
                let Some((end, sep)) = header_end(head) else {
                    if head.len() > self.h.cfg.max_header_bytes {
                        return Err(Failure::Bad(
                            "the response's header is too large".to_owned(),
                        ));
                    }
                    return Ok(());
                };
                let (status, headers, declared) = parse_head(head.get(..end).unwrap_or_default())?;
                let body = head
                    .get(end.saturating_add(sep)..)
                    .unwrap_or_default()
                    .to_vec();
                st.out = Out::Collect {
                    status,
                    headers,
                    body,
                    declared,
                };
            }
            Out::Collect { body, .. } => body.extend_from_slice(p),
            Out::Stream { stream, .. } => {
                if stream.write(p).is_err() {
                    return Err(Failure::ClientGone);
                }
                self.pace_down(fc, stream);
            }
            Out::Starting | Out::Done => {}
        }
        Ok(())
    }

    /// Holds the connection's reads while the client has more than [`WATERMARK`] queued.
    fn pace_down(&self, fc: &Arc<FcgiConn>, stream: &BodyStream) {
        if stream.queued_bytes() <= WATERMARK || self.held.swap(true, Ordering::AcqRel) {
            return;
        }
        // Records already read may still come; one hold stands for them all.
        fc.hold_reads(true);
        let (held, resumed, born, fc) = (
            self.held.clone(),
            self.resumed.clone(),
            self.born,
            fc.clone(),
        );
        stream.on_drain(move || {
            let ms = u64::try_from(born.elapsed().as_millis()).unwrap_or(u64::MAX);
            resumed.store(ms, Ordering::Release);
            held.store(false, Ordering::Release);
            fc.hold_reads(false);
        });
    }

    /// A read's records are all taken: a response collected but not ended starts streaming.
    fn flush(self: &Arc<Self>, fc: &Arc<FcgiConn>) {
        let (answer, status, headers, body, declared) = {
            let mut st = lock(&self.st);
            if st.done || !matches!(st.out, Out::Collect { .. }) {
                return;
            }
            let Some(answer) = st.answer.take() else {
                return;
            };
            let Out::Collect {
                status,
                headers,
                body,
                declared,
            } = std::mem::replace(&mut st.out, Out::Starting)
            else {
                return;
            };
            (answer, status, headers, body, declared)
        };
        // The length is kept on every version: akio frames an HTTP/1.1 response that streams by
        // it (ADR-157), and closes the connection if the application ends it short; it refuses
        // a write past it, which cuts the response short too.
        let keep_length = declared;
        let stream = answer.stream_response(status, headers);
        let mut st = lock(&self.st);
        let Some(stream) = stream else {
            st.out = Out::Done;
            drop(st);
            return self.fail(Failure::ClientGone);
        };
        if st.done {
            // Failed meanwhile: cut short.
            st.out = Out::Done;
            drop(st);
            return self.broken(stream, keep_length);
        }
        if !body.is_empty() && stream.write(&body).is_err() {
            st.out = Out::Done;
            drop(st);
            drop(stream);
            return self.fail(Failure::ClientGone);
        }
        self.pace_down(fc, &stream);
        st.out = Out::Stream {
            stream,
            framed: keep_length,
        };
    }

    /// The application ended the request. Returns whether the connection may carry another: the
    /// request ended as FastCGI says, its body all sent.
    fn end(self: &Arc<Self>, complete: bool) -> bool {
        let (answer, out, reuse) = {
            let mut st = lock(&self.st);
            if st.done {
                return false;
            }
            if let Out::Head(head) = &st.out {
                match parse_head(head) {
                    Ok((status, headers, declared)) => {
                        st.out = Out::Collect {
                            status,
                            headers,
                            body: Vec::new(),
                            declared,
                        };
                    }
                    Err(e) => {
                        drop(st);
                        self.fail(e);
                        return false;
                    }
                }
            }
            st.done = true;
            self.finished.store(true, Ordering::Release);
            st.conn = None;
            let reuse = complete && st.up.sent_end;
            (
                st.answer.take(),
                std::mem::replace(&mut st.out, Out::Done),
                reuse,
            )
        };
        self.end_upload();
        match out {
            Out::Collect {
                status,
                mut headers,
                body,
                ..
            } => {
                if let Some(answer) = answer {
                    // Sent whole: framed by the body's length, which akio writes.
                    headers.remove("content-length");
                    let mut resp = Response::new(status);
                    resp.headers = headers;
                    resp.body = body;
                    answer.respond(resp);
                }
            }
            Out::Stream { stream, .. } => stream.finish(),
            Out::Head(_) | Out::Starting | Out::Done => {}
        }
        reuse
    }

    /// The exchange is over: what is left of the client's body is not wanted, nor held.
    fn end_upload(&self) {
        if let Some(reads) = &self.reads {
            reads.hold(false);
        }
        self.held.store(false, Ordering::Release);
    }

    /// The connection `fc` went: the exchange fails with `why`, but the request is sent again,
    /// once, on a new connection when `fc` went stale under it and it can be sent again (an idle
    /// connection the application closed, as PHP-FPM does when a worker reaches
    /// `pm.max_requests`, may be taken before akit heard). A connection the exchange no longer
    /// has is no news.
    pub(crate) fn lost(self: &Arc<Self>, fc: &Arc<FcgiConn>, why: String, stale: bool) {
        {
            let mut st = lock(&self.st);
            if !st.conn.as_ref().is_some_and(|c| Arc::ptr_eq(c, fc)) || st.done {
                return;
            }
            if stale && !st.retried && self.replayable {
                st.retried = true;
                st.conn = None;
            } else {
                drop(st);
                return self.fail(Failure::Bad(why));
            }
        }
        fc.release(self, false);
        let x = self.clone();
        // A new connection: the other idle ones may have gone stale with it.
        self.h.pool.take(Box::new(move |r| x.connected(r)), false);
    }

    /// Ends the exchange for `f`: answered with its status when the response has not begun,
    /// cut short when it has; nothing more for a client gone. The connection is closed.
    pub(crate) fn fail(self: &Arc<Self>, f: Failure) {
        let (answer, out, conn) = {
            let mut st = lock(&self.st);
            if st.done {
                return;
            }
            st.done = true;
            self.finished.store(true, Ordering::Release);
            (
                st.answer.take(),
                std::mem::replace(&mut st.out, Out::Done),
                st.conn.take(),
            )
        };
        if let Some(fc) = conn {
            fc.release(self, false);
        }
        self.end_upload();
        if matches!(f, Failure::ClientGone) {
            return;
        }
        match out {
            Out::Stream { stream, framed } => self.broken(stream, framed),
            Out::Head(_) | Out::Collect { .. } => {
                if let Some(answer) = answer {
                    answer.respond(error_response(f.status()));
                }
            }
            // Being started: the one starting it cuts it short.
            Out::Starting | Out::Done => {}
        }
    }

    /// Cuts short a response that failed after it began, so that the client can tell, as the
    /// proxy does: unfinished, the stream closes an HTTP/1 client's connection and resets an
    /// HTTP/2 or HTTP/3 stream; one of no declared length there ends with fit's trailer.
    fn broken(&self, stream: BodyStream, framed: bool) {
        if !framed && matches!(self.version, Version::Http2 | Version::Http3) {
            let mut t = Headers::new();
            t.append(BROKEN_TRAILER.0, BROKEN_TRAILER.1);
            stream.finish_with_trailers(t);
        } else {
            drop(stream);
        }
    }
}

/// Where the CGI header ends, and the length of the blank line's separator.
pub(crate) fn header_end(head: &[u8]) -> Option<(usize, usize)> {
    let crlf = head.windows(4).position(|w| w == b"\r\n\r\n");
    let lf = head.windows(2).position(|w| w == b"\n\n");
    match (crlf, lf) {
        (Some(c), Some(l)) if l < c => Some((l, 2)),
        (Some(c), _) => Some((c, 4)),
        (None, Some(l)) => Some((l, 2)),
        (None, None) => None,
    }
}

/// Whether `s` is an HTTP token, as a field name must be.
fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// The status, fields and whether a length is declared, from a CGI header: `Status` gives the
/// status (200 by default, 302 with a `Location`); the hop-by-hop fields are dropped.
pub(crate) fn parse_head(head: &[u8]) -> Result<(u16, Headers, bool), Failure> {
    let bad = |m: &str| Failure::Bad(format!("the application's response header: {m}"));
    let text = std::str::from_utf8(head).map_err(|_| bad("not UTF-8"))?;
    let mut status = 0u16;
    let mut headers = Headers::new();
    let mut location = false;
    let mut declared = false;
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(bad("a line with no colon"));
        };
        let (name, value) = (name.trim(), value.trim());
        if !is_token(name) || value.contains(['\r', '\0']) {
            return Err(bad("a malformed field"));
        }
        if name.eq_ignore_ascii_case("status") {
            let code = value.split(' ').next().unwrap_or("");
            match code.parse::<u16>() {
                Ok(n) if (100..=999).contains(&n) => status = n,
                _ => return Err(bad("a malformed status")),
            }
            continue;
        }
        if ["connection", "transfer-encoding", "keep-alive", "upgrade"]
            .iter()
            .any(|h| name.eq_ignore_ascii_case(h))
        {
            continue;
        }
        if name.eq_ignore_ascii_case("location") {
            location = true;
        }
        if name.eq_ignore_ascii_case("content-length") {
            // One that is no length is dropped: a streamed response is framed by it.
            if value.parse::<u64>().is_err() {
                continue;
            }
            declared = true;
        }
        headers.append(name, value);
    }
    if status == 0 {
        status = if location { 302 } else { 200 };
    }
    Ok((status, headers, declared))
}

/// What the application writes to its standard error goes to akit's, at most this many lines a
/// second (fit's `maxStderrLines`), each cut to 1 KiB, written on akio's logical pool.
const MAX_STDERR_LINES: u64 = 100;

fn log_stderr(c: &Conn<Link>, addr: &str, p: &[u8]) {
    static SECOND: AtomicU64 = AtomicU64::new(0);
    static LINES: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if SECOND.swap(now, Ordering::AcqRel) != now {
        LINES.store(0, Ordering::Release);
    }
    if LINES.fetch_add(1, Ordering::AcqRel) >= MAX_STDERR_LINES {
        return;
    }
    let msg = String::from_utf8_lossy(p.get(..p.len().min(1024)).unwrap_or(p))
        .trim()
        .to_owned();
    let addr = addr.to_owned();
    // Dropped when the pool is full.
    let _ = c.execute(move || akit_observe::warn!(Fastcgi, "fastcgi {addr}: {msg}"));
}

/// Takes the records in `data` that the application sent on `fc` for `x`, on the connection's
/// thread.
pub(crate) fn records(
    x: &Arc<Exchange>,
    fc: &Arc<FcgiConn>,
    reader: &mut Reader,
    c: &Conn<Link>,
    data: &[u8],
) {
    let mut failure: Option<Failure> = None;
    let mut ended: Option<bool> = None;
    let addr = &x.h.cfg.address_text;
    let fed = reader.feed(data, |rec| {
        if rec.id != record::REQUEST_ID {
            failure = Some(Failure::Bad("a record of another request".to_owned()));
            return false;
        }
        match rec.t {
            STDOUT => {
                if let Err(f) = x.stdout(fc, rec.content) {
                    failure = Some(f);
                    return false;
                }
            }
            STDERR => log_stderr(c, addr, rec.content),
            END_REQUEST => {
                match rec.content.get(4) {
                    Some(&protocol_status) if rec.content.len() >= 8 => {
                        ended = Some(protocol_status == 0);
                    }
                    _ => {
                        failure = Some(Failure::Bad("a short end record".to_owned()));
                    }
                }
                return false;
            }
            _ => {}
        }
        true
    });
    let left = match fed {
        Ok(left) => left,
        Err(record::Protocol) => {
            failure = Some(Failure::Bad(
                "the application broke the protocol".to_owned(),
            ));
            false
        }
    };
    if let Some(f) = failure {
        x.fail(f);
        return c.close();
    }
    match ended {
        Some(complete) => {
            // Whatever follows the end record breaks the protocol, but is the next request's
            // problem only if the connection is kept: it is not.
            let reuse = x.end(complete) && !left && reader.is_empty();
            if left {
                *reader = Reader::default();
            }
            fc.release(x, reuse);
        }
        None => x.flush(fc),
    }
}
