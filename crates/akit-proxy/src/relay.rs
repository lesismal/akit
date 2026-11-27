//! One proxied request, its bodies streamed both ways with linked backpressure (I16), as fit's
//! `exchange` streams them:
//!
//! - **Up.** A request body that streams to the handler (`body_mode`) is written to the upstream
//!   as it arrives ([`Exchange::on_request_body`]). While more than [`UPLOAD_PENDING`] of it waits
//!   in the upstream connection's queue, the client's reads are held ([`BodyReads`]), and resumed
//!   once the queue has drained: the client waits in TCP (or for HTTP/2 and HTTP/3 window), not
//!   in akit's memory. What arrives before the upstream connection is ready waits here, the
//!   client's reads held meanwhile: one read's worth at most. A body that grows past its bound
//!   (the listener's `max_body_bytes`, or a `body_limit`'s: akio's `Context::limit_body`, which
//!   tells the sink [`BodyEvent::TooLarge`]) is answered 413, as fit answers it, or its response
//!   cut short if it began; the upstream request is given up either way.
//! - **Down.** A response of a length within [`WHOLE_RESPONSE`] is collected and sent whole, as
//!   fit sends one that arrived complete; any other is sent as it arrives
//!   ([`Detached::stream_response`]). While more than [`DOWNLOAD_PENDING`] of it waits for the
//!   client, the upstream connection's reads are held ([`Download::hold_reads`]), and resumed once
//!   the client has taken it ([`BodyStream::on_drain`]).
//!
//! I17 adds what ends an exchange early, and the trailers (see the crate's docs): the client's
//! body sink dropped before the body ended is the client gone mid-upload ([`Sink`]'s `Drop`),
//! which gives the upstream request up; trailers go on both ways, those of a request filtered to
//! the names it announced; a body of no declared length cut short on HTTP/2 or HTTP/3 ends with
//! [`BROKEN_TRAILER`].
//!
//! I17b hears the client go at any point ([`start`] registers `Context::on_gone`): the upstream
//! request's [`CancelToken`] is cancelled, and the exchange says it gave up. A response framed by
//! its length whose every byte was written is finished all the same: its client has it whole.
//!
//! I45 sends a request again ([`crate::retry`]) from [`Exchange::on_error`] before its response
//! began, or from [`Exchange::on_head`] for a status the proxy's `retry` lists: the next
//! attempt is a new [`Relay`] that takes over the client, handed to the pool; this one is left
//! with nothing to answer.
//!
//! Nothing here blocks or waits: each side's callbacks run on their I/O threads, and the state
//! they share is behind one lock that is never held while calling into akio, which may call back.

use std::mem;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use akio_http::client::CancelToken;
use akio_http::{BodyEvent, BodyReads, BodyStream, Context, Detached, Headers, Response, Version};
use akit_runtime::upstream::{Download, Error, Exchange, Head, Upload as Body};

use crate::headers;
use crate::retry::Again;

/// How much of an upload may wait in the upstream connection's queue before the client's reads
/// are held: fit's `uploadPending`, one read of the client's.
pub(crate) const UPLOAD_PENDING: usize = 64 << 10;

/// How much of a download may wait for the client before the upstream connection's reads are
/// held.
pub(crate) const DOWNLOAD_PENDING: usize = 64 << 10;

/// The largest response collected and sent whole rather than as it arrives: fit's upstream
/// `StreamThreshold`.
pub(crate) const WHOLE_RESPONSE: u64 = 64 << 10;

/// The trailer an HTTP/2 or HTTP/3 response of no declared length ends with when its upstream
/// failed mid-body: fit's `brokenTrailer`, its `Fit-Error` renamed `Akit-Error` (decision 037).
pub(crate) const BROKEN_TRAILER: (&str, &str) = ("Akit-Error", "upstream failed mid-body");

/// What the request's two sides share.
struct Shared {
    /// The right to answer the client, until the response begins or a failure is answered.
    answer: Option<Detached>,
    up: Upload,
    /// The client went (before its body ended, or before the response was over): the upstream
    /// request was given up for it.
    gone: bool,
    /// The client's body passed its bound: the upstream request was given up for it, which is
    /// no failure of the address's.
    refused: bool,
    /// The request's content type when it is a gRPC call (I50), whose failures are answered as
    /// gRPC answers them.
    grpc: Option<String>,
}

/// A streamed request body on its way up.
#[derive(Default)]
struct Upload {
    /// The upstream's body, once its head is written and what arrived before is flushed. An
    /// `Arc` so that it is written to outside the lock.
    body: Option<Arc<Body>>,
    /// What arrived before `body`, the client's reads held meanwhile.
    pending: Vec<u8>,
    /// The client's body ended (with these trailers) before `body` was there.
    end: Option<Headers>,
    /// Given up: past its bound, or the exchange is over. What still arrives is dropped.
    done: bool,
}

fn lock(m: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The access log's upstream for the request: the address and when the request went to it.
#[derive(Clone)]
pub(crate) struct Logged {
    pub(crate) addr: String,
    pub(crate) start: Instant,
}

/// Runs `f`, which answers the client, with the upstream the access log's line names, if logged.
pub(crate) fn logged<R>(log: Option<&Logged>, f: impl FnOnce() -> R) -> R {
    match log {
        Some(l) => {
            let mut f = Some(f);
            let mut out = None;
            akit_observe::with_upstream(&l.addr, l.start.elapsed(), || {
                out = f.take().map(|f| f());
            });
            match out {
                Some(r) => r,
                // `with_upstream` runs its function exactly once.
                None => unreachable!("with_upstream did not run its function"),
            }
        }
        None => f(),
    }
}

/// The client's side of a request whose body streams: its body sink and its reads.
pub(crate) struct Client {
    shared: Arc<Mutex<Shared>>,
    reads: Option<BodyReads>,
    /// The trailers the request announced.
    names: Vec<String>,
}

/// Starts relaying `ctx`'s request: takes its body's reads when it streams (registering its body
/// sink, sent up `chunked` with the trailers of `names`), has `cancel` cancelled when its client
/// goes or its body passes its bound, and detaches it. `grpc` is the request's content type when
/// it is a gRPC call. `None` when it was answered already.
pub(crate) fn start(
    ctx: &mut Context,
    chunked: bool,
    names: Vec<String>,
    cancel: CancelToken,
    grpc: Option<String>,
) -> Option<(Client, bool)> {
    let reads = ctx.body_reads();
    let streamed = reads.is_some();
    let shared = Arc::new(Mutex::new(Shared {
        answer: None,
        up: Upload::default(),
        gone: false,
        refused: false,
        grpc,
    }));
    // The client gone, whenever: the upstream request given up, wherever it is. Weak: akio keeps
    // the callback with the answer, which `shared` holds. Run at once if it went already, before
    // the answer is put there (the lock is not held).
    let weak = Arc::downgrade(&shared);
    let refuse = cancel.clone();
    ctx.on_gone(move || {
        if let Some(s) = weak.upgrade() {
            lock(&s).gone = true;
        }
        cancel.cancel();
    });
    let answer = ctx.detach()?;
    lock(&shared).answer = Some(answer);
    if let Some(reads) = &reads {
        let mut sink = Sink {
            shared: shared.clone(),
            reads: reads.clone(),
            cancel: refuse,
            chunked,
            names: names.clone(),
            ended: false,
        };
        ctx.on_body(move |_, ev| sink.event(ev));
    }
    Some((
        Client {
            shared,
            reads,
            names,
        },
        streamed,
    ))
}

impl Client {
    /// The client's side of a request already detached whose body goes nowhere: an extended
    /// CONNECT's, whose upstream answered without switching (I49, `websocket`).
    pub(crate) fn answered(answer: Detached) -> Client {
        Client {
            shared: Arc::new(Mutex::new(Shared {
                answer: Some(answer),
                up: Upload {
                    done: true,
                    ..Upload::default()
                },
                gone: false,
                refused: false,
                grpc: None,
            })),
            reads: None,
            names: Vec::new(),
        }
    }

    /// A body that arrived whole, with `trailers`, to go up as a stream: chunked, so that it
    /// carries those of them the request announced.
    pub(crate) fn preload(&self, body: &[u8], trailers: &Headers) {
        let mut s = lock(&self.shared);
        s.up.pending.extend_from_slice(body);
        s.up.end = Some(headers::announced(trailers, &self.names));
    }
}

/// The client's streamed body, as it arrives.
struct Sink {
    shared: Arc<Mutex<Shared>>,
    reads: BodyReads,
    /// The upstream request's, given up when the body passes its bound.
    cancel: CancelToken,
    chunked: bool,
    /// The trailers the request announced: only those go up.
    names: Vec<String>,
    /// The body ended, or was refused: dropped after that, the sink is done, not cut off.
    ended: bool,
}

impl Sink {
    fn event(&mut self, ev: BodyEvent<'_>) {
        match ev {
            BodyEvent::Chunk(data) => self.chunk(data),
            BodyEvent::End(trailers) => self.end(trailers),
            BodyEvent::TooLarge => self.too_large(),
        }
    }

    fn chunk(&mut self, data: &[u8]) {
        let body = {
            let s = lock(&self.shared);
            if s.up.done {
                return;
            }
            s.up.body.clone()
        };
        let body = match body {
            Some(b) => b,
            None => {
                // Not ready yet: held first, so that the release that comes once it is ready,
                // having flushed what waits, is not missed.
                self.reads.hold(true);
                let mut s = lock(&self.shared);
                if s.up.done {
                    drop(s);
                    self.reads.hold(false);
                    return;
                }
                match s.up.body.clone() {
                    Some(b) => b,
                    None => {
                        s.up.pending.extend_from_slice(data);
                        return;
                    }
                }
            }
        };
        if !body.write(data) {
            // The upstream request is over (answered early, or failed): the rest is not wanted.
            return;
        }
        pace(&body, &self.reads);
    }

    fn end(&mut self, trailers: &Headers) {
        self.ended = true;
        let trailers = headers::announced(trailers, &self.names);
        let body = {
            let mut s = lock(&self.shared);
            if s.up.done {
                return;
            }
            match s.up.body.take() {
                Some(b) => b,
                None => {
                    s.up.end = Some(trailers);
                    return;
                }
            }
        };
        finish(body, &trailers, self.chunked);
    }

    /// The body grew past its bound (akio's `Context::limit_body`: the listener's
    /// `max_body_bytes`, or a `body_limit`'s), and akio reads no more of it: answered 413, as fit
    /// answers it, when the response has not begun, and the upstream request given up, which
    /// cuts short a response that has (fit's upload failing under it).
    fn too_large(&mut self) {
        self.ended = true;
        let (body, answer, grpc) = {
            let mut s = lock(&self.shared);
            s.refused = true;
            let body = if s.up.done {
                None
            } else {
                s.up.done = true;
                s.up.pending = Vec::new();
                s.up.end = None;
                s.up.body.take()
            };
            (body, s.answer.take(), s.grpc.clone())
        };
        // Unfinished, it closes the upstream connection.
        drop(body);
        if let Some(answer) = answer {
            answer.respond(crate::failure(413, grpc.as_deref()));
        }
        self.cancel.cancel();
    }
}

impl Drop for Sink {
    /// Dropped before the body ended, the sink is the client gone mid-upload (akio drops a
    /// request's body sink with its connection or stream): the upstream request is given up, its
    /// unfinished body closing the upstream connection, as fit's `OnCancel` cancels it.
    fn drop(&mut self) {
        if self.ended {
            return;
        }
        let body = {
            let mut s = lock(&self.shared);
            if s.up.done {
                return;
            }
            s.up.done = true;
            s.gone = true;
            s.up.pending = Vec::new();
            s.up.end = None;
            s.up.body.take()
        };
        drop(body);
    }
}

/// Holds the client's reads while more than [`UPLOAD_PENDING`] of its body waits for the
/// upstream, until that has drained; else lets them go on.
fn pace(body: &Body, reads: &BodyReads) {
    if body.queued_bytes() > UPLOAD_PENDING {
        reads.hold(true);
        let reads = reads.clone();
        // At once, on this thread, if it drained meanwhile.
        body.on_drain(move || reads.hold(false));
    } else {
        reads.hold(false);
    }
}

/// Ends the upstream's body, with the client's trailers when it is chunked.
fn finish(body: Arc<Body>, trailers: &Headers, chunked: bool) {
    // Held alone: the sink drops its clone before its next event, and `on_request_body` hands
    // its over.
    let Ok(body) = Arc::try_unwrap(body) else {
        return;
    };
    let _ = if chunked && !trailers.is_empty() {
        body.finish_with_trailers(trailers.clone())
    } else {
        body.finish()
    };
}

/// Where the response to the client is.
enum Out {
    /// No head from the upstream yet.
    Waiting,
    /// Collected, to be sent whole (or, ending with trailers, streamed to carry them).
    Whole {
        status: u16,
        headers: Headers,
        body: Vec<u8>,
        /// The trailers the upstream announced, and whether the response may have a body.
        names: Vec<String>,
        bodyless: bool,
    },
    /// Sent as it arrives; `left` is, for one framed by a declared length, how much of that is
    /// still to be written.
    Stream {
        stream: BodyStream,
        up: Download,
        left: Option<u64>,
    },
    /// Over, or given up.
    Done,
}

/// The upstream's side of a proxied request: akit-upstream's [`Exchange`].
pub(crate) struct Relay {
    shared: Arc<Mutex<Shared>>,
    reads: Option<BodyReads>,
    /// The request is a `HEAD`.
    head: bool,
    /// The client's HTTP version: an HTTP/1.1 response streamed is chunked.
    version: Version,
    /// The request's body is sent chunked upstream.
    chunked: bool,
    log: Option<Logged>,
    /// The sticky cookie the response sets (I36), after the upstream's fields.
    set_cookie: Option<String>,
    out: Out,
    /// What the request needs to be sent again (I45), when the proxy retries and its body can
    /// be replayed.
    again: Option<Box<Again>>,
    /// The request is a gRPC call (I50).
    grpc: bool,
    /// When the proxy's own timeout (its `timeout`, or a gRPC call's `grpc-timeout`) cuts the
    /// request short: a failure past it is no failure of the address's, as fit's `timedOut`.
    deadline: Option<Instant>,
}

impl Relay {
    pub(crate) fn new(
        client: Client,
        head: bool,
        version: Version,
        chunked: bool,
        log: Option<Logged>,
        set_cookie: Option<String>,
        again: Option<Box<Again>>,
    ) -> Relay {
        let grpc = lock(&client.shared).grpc.is_some();
        Relay {
            shared: client.shared,
            reads: client.reads,
            head,
            version,
            chunked,
            log,
            set_cookie,
            out: Out::Waiting,
            again,
            grpc,
            deadline: None,
        }
    }

    /// The same, the request cut short by the proxy at `deadline`.
    #[must_use]
    pub(crate) fn with_deadline(mut self, deadline: Option<Instant>) -> Relay {
        self.deadline = deadline;
        self
    }

    /// Whether the proxy's own timeout has passed.
    fn timed_out(&self) -> bool {
        self.deadline.is_some_and(|d| Instant::now() >= d)
    }

    /// Sends the request again, if its retries allow it for the attempt that just ended,
    /// answered with `status` or failed with `err` (I45), and reports whether it did. The next
    /// attempt takes the client over; this one is left with nothing to answer.
    fn retry(&mut self, status: Option<u16>, err: Option<&Error>) -> bool {
        let gone = lock(&self.shared).gone;
        let Some(again) = self.again.as_mut() else {
            return false;
        };
        if !again.may(status, err, gone) {
            return false;
        }
        let Some(next) = again.next() else {
            return false;
        };
        let pool = again.pool.clone();
        let log = self.log.as_ref().map(|_| Logged {
            addr: pool.address(next.backend).unwrap_or_default().to_owned(),
            start: Instant::now(),
        });
        let relay = Relay {
            shared: Arc::clone(&self.shared),
            reads: self.reads.clone(),
            head: self.head,
            version: self.version,
            chunked: self.chunked,
            log,
            set_cookie: next.set_cookie,
            out: Out::Waiting,
            again: self.again.take(),
            grpc: self.grpc,
            deadline: self.deadline,
        };
        self.out = Out::Done;
        pool.stream(next.backend, next.req, relay);
        true
    }

    /// The right to answer the client, if it is still here's.
    fn answer(&self) -> Option<Detached> {
        lock(&self.shared).answer.take()
    }

    /// The exchange is over: what is left of the client's body is not wanted, nor held.
    fn end_upload(&mut self) {
        let body = {
            let mut s = lock(&self.shared);
            if s.up.done {
                return;
            }
            s.up.done = true;
            s.up.pending = Vec::new();
            s.up.end = None;
            s.up.body.take()
        };
        drop(body);
        if let Some(reads) = &self.reads {
            reads.hold(false);
        }
    }

    /// Answers the client with fit's status for `e`, unless it was answered already; a gRPC
    /// call with its gRPC status, a past deadline being `DEADLINE_EXCEEDED`.
    fn fail(&self, e: &Error) {
        let (answer, grpc) = {
            let mut s = lock(&self.shared);
            (s.answer.take(), s.grpc.clone())
        };
        if let Some(answer) = answer {
            let status = if grpc.is_some() && self.timed_out() {
                504
            } else {
                crate::status_of(e)
            };
            let resp = crate::failure(status, grpc.as_deref());
            logged(self.log.as_ref(), || answer.respond(resp));
        }
    }
}

impl Exchange for Relay {
    fn on_request_body(&mut self, body: Body) {
        let body = Arc::new(body);
        // What arrived meanwhile goes first. More may arrive while it is written, and once the
        // reads are let go, so the body is handed to the sink, which keeps no clone of its own
        // past one event, only once nothing waits: the one who ends it then holds it alone.
        loop {
            let (pending, end) = {
                let mut s = lock(&self.shared);
                if s.up.done {
                    return;
                }
                (mem::take(&mut s.up.pending), s.up.end.take())
            };
            if !pending.is_empty() && !body.write(&pending) {
                return;
            }
            if let Some(end) = end {
                return finish(body, &end, self.chunked);
            }
            if !pending.is_empty() {
                continue;
            }
            if let Some(reads) = &self.reads {
                pace(&body, reads);
            }
            let mut s = lock(&self.shared);
            if s.up.done {
                return;
            }
            if s.up.pending.is_empty() && s.up.end.is_none() {
                s.up.body = Some(body);
                return;
            }
        }
    }

    fn on_head(&mut self, head: Head, up: Download) {
        let length = head
            .headers
            .get("content-length")
            .and_then(|v| v.trim().parse::<u64>().ok());
        // Only 5xx statuses are retried (`retry.statuses`): a healthy response asks nothing more.
        if head.status >= 500 && self.again.is_some() && self.retry(Some(head.status), None) {
            // A small body is read and dropped, which keeps the connection; a larger one is not
            // worth waiting for.
            if !length.is_some_and(|n| n <= WHOLE_RESPONSE) {
                up.cancel();
            }
            return;
        }
        // A gRPC call's trailers-only response (its status in its head) is passed on as one; any
        // other of its responses streams, however short (fit's `blockResponse`).
        let trailers_only = self.grpc && head.headers.get("grpc-status").is_some();
        let bodyless = self.head || trailers_only || matches!(head.status, 100..=199 | 204 | 304);
        let whole = !self.grpc && length.is_some_and(|n| n <= WHOLE_RESPONSE);
        if bodyless || whole {
            let capacity = if bodyless { 0 } else { length.unwrap_or(0) };
            let mut fields = headers::incoming(&head.headers, self.head);
            if let Some(c) = self.set_cookie.take() {
                fields.append("Set-Cookie", c);
            }
            self.out = Out::Whole {
                status: head.status,
                headers: fields,
                body: Vec::with_capacity(usize::try_from(capacity).unwrap_or(0)),
                names: headers::trailer_names(&head.headers),
                bodyless,
            };
            return;
        }
        let Some(answer) = self.answer() else {
            // Answered already (a body past its bound): the response is not wanted.
            up.cancel();
            self.out = Out::Done;
            return;
        };
        // Its trailers follow it, as their announcement does (fit's `begin`).
        let names = headers::trailer_names(&head.headers);
        // The length is kept: akio frames an HTTP/1.1 response that streams by it (ADR-157),
        // and its client can tell a body cut short. But on HTTP/1.1 a response with trailers is
        // sent chunked, which alone carries them; HTTP/2 and HTTP/3 carry both.
        let keep_length = length.is_some()
            && (names.is_empty() || matches!(self.version, Version::Http2 | Version::Http3));
        let mut fields = headers::incoming(&head.headers, keep_length);
        if let Some(c) = self.set_cookie.take() {
            fields.append("Set-Cookie", c);
        }
        if !names.is_empty() {
            fields.append("Trailer", names.join(", "));
        }
        let status = head.status;
        match logged(self.log.as_ref(), || answer.stream_response(status, fields)) {
            Some(stream) => {
                self.out = Out::Stream {
                    stream,
                    up,
                    left: length.filter(|_| keep_length),
                }
            }
            None => {
                up.cancel();
                self.out = Out::Done;
            }
        }
    }

    fn on_data(&mut self, chunk: &[u8]) {
        match &mut self.out {
            // What follows a head that says there is no body is not wanted.
            Out::Whole { body, bodyless, .. } => {
                if !*bodyless {
                    body.extend_from_slice(chunk);
                }
            }
            Out::Stream { stream, up, left } => {
                if stream.write(chunk).is_err() {
                    // The client went: the response is not wanted.
                    up.cancel();
                    self.out = Out::Done;
                    return;
                }
                if let Some(l) = left {
                    *l = l.saturating_sub(chunk.len() as u64);
                }
                if stream.queued_bytes() > DOWNLOAD_PENDING {
                    up.hold_reads(true);
                    let up = up.clone();
                    // At once, on this thread, if the client took it meanwhile.
                    stream.on_drain(move || up.hold_reads(false));
                }
            }
            Out::Waiting | Out::Done => {}
        }
    }

    fn on_end(&mut self, trailers: &Headers) {
        self.end_upload();
        match mem::replace(&mut self.out, Out::Done) {
            Out::Whole {
                status,
                mut headers,
                body,
                names,
                bodyless,
            } => {
                let Some(answer) = self.answer() else { return };
                if bodyless || trailers.is_empty() {
                    let mut resp = Response::new(status);
                    resp.headers = headers;
                    resp.body = body;
                    return logged(self.log.as_ref(), || answer.respond(resp));
                }
                // akio sends a whole response's trailers on HTTP/2 alone: streamed, chunked
                // on HTTP/1.1, it carries them everywhere, announced.
                let names = if names.is_empty() {
                    trailers.iter().map(|(n, _)| n.to_owned()).collect()
                } else {
                    names
                };
                headers.append("Trailer", names.join(", "));
                if let Some(stream) = logged(self.log.as_ref(), || {
                    answer.stream_response(status, headers)
                }) {
                    if stream.write(&body).is_ok() {
                        stream.finish_with_trailers(trailers.clone());
                    }
                }
            }
            Out::Stream { stream, .. } => {
                if trailers.is_empty() {
                    stream.finish();
                } else {
                    stream.finish_with_trailers(trailers.clone());
                }
            }
            Out::Waiting | Out::Done => {}
        }
    }

    fn on_error(&mut self, e: Error) {
        if matches!(self.out, Out::Waiting) && self.again.is_some() && self.retry(None, Some(&e)) {
            return;
        }
        self.end_upload();
        match mem::replace(&mut self.out, Out::Done) {
            // Broken off mid-body (or cut short by its timeout): unfinished, the stream closes an
            // HTTP/1 client's connection, which is what tells it that the body is short, and
            // resets an HTTP/2 or HTTP/3 stream. One of no declared length there ends with fit's
            // trailer instead, as fit ends it.
            // A gRPC call's ends with gRPC's status, which is all its client reads.
            Out::Stream { stream, .. } if self.grpc => {
                let timed_out = self.timed_out()
                    || matches!(e, Error::Client(akio_http::client::ClientError::Timeout));
                stream.finish_with_trailers(crate::grpc::broken(timed_out));
            }
            // Every byte its length declares was written: the body is whole, and finished as
            // such (a cache's tap keeps it). An HTTP/1 client that read them all may close before
            // the upstream's end comes, and its going gives the upstream request up first.
            Out::Stream {
                stream,
                left: Some(0),
                ..
            } => stream.finish(),
            Out::Stream { stream, left, .. } => {
                if left.is_none() && matches!(self.version, Version::Http2 | Version::Http3) {
                    let mut t = Headers::new();
                    t.append(BROKEN_TRAILER.0, BROKEN_TRAILER.1);
                    stream.finish_with_trailers(t);
                } else {
                    drop(stream);
                }
            }
            Out::Waiting | Out::Whole { .. } => self.fail(&e),
            Out::Done => {}
        }
    }

    /// The client gone, its body refused, or the proxy's own timeout passed (fit's `timedOut`):
    /// none is the address's failure.
    fn gave_up(&self) -> bool {
        let s = lock(&self.shared);
        s.gone || s.refused || self.timed_out()
    }
}
