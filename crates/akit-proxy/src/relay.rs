//! One proxied request, its bodies streamed both ways with linked backpressure (I16), as fit's
//! `exchange` streams them:
//!
//! - **Up.** A request body that streams to the handler (`body_mode`) is written to the upstream
//!   as it arrives ([`Exchange::on_request_body`]). While more than [`UPLOAD_PENDING`] of it waits
//!   in the upstream connection's queue, the client's reads are held ([`BodyReads`]), and resumed
//!   once the queue has drained: the client waits in TCP (or for HTTP/2 and HTTP/3 window), not
//!   in akit's memory. What arrives before the upstream connection is ready waits here, the
//!   client's reads held meanwhile: one read's worth at most. A body that grows past the
//!   listener's `max_body_bytes` is answered 413, as fit answers it.
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
//! Nothing here blocks or waits: each side's callbacks run on their I/O threads, and the state
//! they share is behind one lock that is never held while calling into akio, which may call back.

use std::mem;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use akio_http::client::{RequestBody, ResponseHead};
use akio_http::{BodyEvent, BodyReads, BodyStream, Context, Detached, Headers, Response, Version};
use akit_runtime::upstream::{Download, Error, Exchange};

use crate::headers;

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
/// failed mid-body: fit's `Fit-Error` and its `brokenTrailer`.
pub(crate) const BROKEN_TRAILER: (&str, &str) = ("Fit-Error", "upstream failed mid-body");

/// What the request's two sides share.
struct Shared {
    /// The right to answer the client, until the response begins or a failure is answered.
    answer: Option<Detached>,
    up: Upload,
    /// The client went before its body ended: the upstream request was given up for it.
    gone: bool,
}

/// A streamed request body on its way up.
#[derive(Default)]
struct Upload {
    /// The upstream's body, once its head is written and what arrived before is flushed. An
    /// `Arc` so that it is written to outside the lock.
    body: Option<Arc<RequestBody>>,
    /// What arrived before `body`, the client's reads held meanwhile.
    pending: Vec<u8>,
    /// The client's body ended (with these trailers) before `body` was there.
    end: Option<Headers>,
    /// Given up: past the limit, or the exchange is over. What still arrives is dropped.
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
fn logged<R>(log: Option<&Logged>, f: impl FnOnce() -> R) -> R {
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
/// sink, held to `limit`, sent up `chunked` with the trailers of `names`), and detaches it.
/// `None` when it was answered already.
pub(crate) fn start(
    ctx: &mut Context,
    limit: u64,
    chunked: bool,
    names: Vec<String>,
) -> Option<(Client, bool)> {
    let reads = ctx.body_reads();
    let streamed = reads.is_some();
    let answer = ctx.detach()?;
    let shared = Arc::new(Mutex::new(Shared {
        answer: Some(answer),
        up: Upload::default(),
        gone: false,
    }));
    if let Some(reads) = &reads {
        let mut sink = Sink {
            shared: shared.clone(),
            reads: reads.clone(),
            limit,
            received: 0,
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
    limit: u64,
    received: u64,
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
        }
    }

    fn chunk(&mut self, data: &[u8]) {
        self.received = self.received.saturating_add(data.len() as u64);
        if self.received > self.limit {
            return self.too_large();
        }
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
        if body.write(data).is_err() {
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

    /// The body grew past the listener's `max_body_bytes`: answered 413, as fit answers it, and
    /// the upstream request given up.
    fn too_large(&mut self) {
        self.ended = true;
        let (body, answer) = {
            let mut s = lock(&self.shared);
            if s.up.done {
                return;
            }
            s.up.done = true;
            s.up.pending = Vec::new();
            (s.up.body.take(), s.answer.take())
        };
        // Unfinished, it closes the upstream connection.
        drop(body);
        self.reads.hold(false);
        if let Some(answer) = answer {
            answer.respond(crate::fail(413));
        }
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
fn pace(body: &RequestBody, reads: &BodyReads) {
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
fn finish(body: Arc<RequestBody>, trailers: &Headers, chunked: bool) {
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
    /// Sent as it arrives; `framed` says by a declared length.
    Stream {
        stream: BodyStream,
        up: Download,
        framed: bool,
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
    out: Out,
}

impl Relay {
    pub(crate) fn new(
        client: Client,
        head: bool,
        version: Version,
        chunked: bool,
        log: Option<Logged>,
    ) -> Relay {
        Relay {
            shared: client.shared,
            reads: client.reads,
            head,
            version,
            chunked,
            log,
            out: Out::Waiting,
        }
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

    /// Answers the client with fit's status for `e`, unless it was answered already.
    fn fail(&self, e: &Error) {
        if let Some(answer) = self.answer() {
            let resp = crate::fail(crate::status_of(e));
            logged(self.log.as_ref(), || answer.respond(resp));
        }
    }
}

impl Exchange for Relay {
    fn on_request_body(&mut self, body: RequestBody) {
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
            if !pending.is_empty() && body.write(&pending).is_err() {
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

    fn on_head(&mut self, head: ResponseHead, up: Download) {
        let length = head
            .headers
            .get("content-length")
            .and_then(|v| v.trim().parse::<u64>().ok());
        let bodyless = self.head || matches!(head.status, 100..=199 | 204 | 304);
        if bodyless || length.is_some_and(|n| n <= WHOLE_RESPONSE) {
            let capacity = if bodyless { 0 } else { length.unwrap_or(0) };
            self.out = Out::Whole {
                status: head.status,
                headers: headers::incoming(&head.headers, self.head),
                body: Vec::with_capacity(usize::try_from(capacity).unwrap_or(0)),
                names: headers::trailer_names(&head.headers),
                bodyless,
            };
            return;
        }
        let Some(answer) = self.answer() else {
            // Answered already (a body past the limit): the response is not wanted.
            up.cancel();
            self.out = Out::Done;
            return;
        };
        // HTTP/2 and HTTP/3 keep the length; akio sends an HTTP/1.1 response that streams
        // chunked, and has no way yet to send it framed by its length (akio API request A35).
        let keep_length =
            length.is_some() && matches!(self.version, Version::Http2 | Version::Http3);
        let mut fields = headers::incoming(&head.headers, keep_length);
        // Its trailers follow it, as their announcement does (fit's `begin`).
        let names = headers::trailer_names(&head.headers);
        if !names.is_empty() {
            fields.append("Trailer", names.join(", "));
        }
        let status = head.status;
        match logged(self.log.as_ref(), || answer.stream_response(status, fields)) {
            Some(stream) => {
                self.out = Out::Stream {
                    stream,
                    up,
                    framed: length.is_some(),
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
            Out::Whole { body, .. } => body.extend_from_slice(chunk),
            Out::Stream { stream, up, .. } => {
                if stream.write(chunk).is_err() {
                    // The client went: the response is not wanted.
                    up.cancel();
                    self.out = Out::Done;
                    return;
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
        self.end_upload();
        match mem::replace(&mut self.out, Out::Done) {
            // Broken off mid-body (or cut short by its timeout): unfinished, the stream closes an
            // HTTP/1 client's connection, which is what tells it that the body is short, and
            // resets an HTTP/2 or HTTP/3 stream. One of no declared length there ends with fit's
            // trailer instead, as fit ends it.
            Out::Stream { stream, framed, .. } => {
                if !framed && matches!(self.version, Version::Http2 | Version::Http3) {
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

    fn gave_up(&self) -> bool {
        lock(&self.shared).gone
    }
}
