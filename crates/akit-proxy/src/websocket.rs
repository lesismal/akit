//! Requests that ask to switch protocols, a WebSocket handshake above all (I49), as fit's
//! `handler/proxy/upgrade.go` passes them on:
//!
//! - **The handshake.** An HTTP/1.1 request that asks to switch (`Connection: upgrade` and an
//!   `Upgrade` other than `h2c`) goes to the upstream as the HTTP/1.1 upgrade it is; an HTTP/2 or
//!   HTTP/3 extended CONNECT for `websocket` (RFC 8441, RFC 9220) goes as the HTTP/1.1 handshake
//!   it stands for (`GET`, `Upgrade: websocket`, a `Sec-WebSocket-Key` of its own, which the
//!   client never sees the accept of). One for another protocol is answered 501, as fit answers
//!   it. Only HTTP/1.1 upstreams can switch: an h2 or h2c upstream answers 502.
//! - **The switch.** When the upstream answers `101`, its connection leaves the pool (akio's
//!   `ResponseHandler::on_upgrade`, ADR-113; akit-upstream's `Exchange::on_upgrade`) and the
//!   client is switched with the upstream's answer: an HTTP/1.1 client gets the `101` with the
//!   upstream's fields, its connection taken over at the handshake (`Context::upgrade_raw`); an
//!   HTTP/2 or HTTP/3 client gets a streamed `200` with them (without `Sec-WebSocket-Accept`),
//!   whose stream is the tunnel (ADR-113). Bytes the client sent before the switch wait, its
//!   reads held, up to [`EARLY_LIMIT`].
//! - **The tunnel.** What arrives on one side is sent on the other, as it arrives; while more
//!   than [`TUNNEL_PENDING`] waits to go to one side, the other side's reads are held, and
//!   resumed once it drained (fit's `tunnelWatermark`): an HTTP/1.1 client's connection or an
//!   HTTP/2 or HTTP/3 stream's reads ([`BodyReads`]) one way, the upstream connection's the other.
//!   A side that ends ends the other once what it sent is written; a client that goes closes the
//!   upstream's connection. A tunnel that carries nothing either way for the proxy's
//!   `tunnel_idle_timeout` (10 minutes by default) is closed.
//! - **Any other answer** is the client's, as any response is: an HTTP/2 or HTTP/3 client's
//!   through the proxy's ordinary relay (its body streamed past 64 KiB with backpressure, a
//!   refusal cut short answered 502 or cut short in turn), except a `2xx`, which an extended
//!   CONNECT's client would take for an open tunnel and its body for WebSocket frames: that is a
//!   502, the upstream's connection closed (fit's `refused`). An HTTP/1.1 client, whose connection
//!   is already the proxy's, gets it written on it, framed by its length or by the connection's
//!   close, and the connection closed after it.
//!
//! Nothing here blocks: each side's callbacks run on its own I/O thread, and the state they share
//! is behind one lock held only to read or swap it (and to hold or resume reads, which calls
//! nothing back).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use akio_http::client::{CancelToken, ClientRequest};
use akio_http::{BodyEvent, BodyReads, BodyStream, Context, Detached, Headers, Method, Request};
use akit_runtime::upstream::{
    Download, Error, Exchange, Head, Pool, RawConn, RawHandler, UpstreamProtocol,
};

use crate::headers;
use crate::relay::{self, Logged, logged};

/// How much may wait to go to one side of a tunnel before the other side's reads are held:
/// fit's `tunnelWatermark`.
pub(crate) const TUNNEL_PENDING: usize = 64 << 10;

/// The most a client may send before the upstream switched (a WebSocket client sends nothing
/// before its answer): past it the request is given up.
pub(crate) const EARLY_LIMIT: usize = 64 << 10;

/// The largest refusal an HTTP/1.1 client gets whole, a cut short one answered 502; a larger one
/// is written as it arrives.
const WHOLE_REFUSAL: u64 = 64 << 10;

/// What a request asks to switch to.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Asked {
    /// Nothing: an ordinary request.
    No,
    /// HTTP/1.1 `Upgrade` to this protocol.
    Upgrade(String),
    /// An extended CONNECT for `websocket`.
    WebSocket,
    /// An extended CONNECT for another protocol, which has no HTTP/1.1 handshake to go as.
    Other,
}

/// What `req` asks to switch to: fit's `upgradeProtocol`. A request asking for `h2c` offers it to
/// the hop it came over, and is the ordinary request it also is.
pub(crate) fn asked(req: &Request) -> Asked {
    if req.method == Method::Connect {
        return match req.protocol.as_deref() {
            Some(p) if p.eq_ignore_ascii_case("websocket") => Asked::WebSocket,
            Some(p) if !p.is_empty() => Asked::Other,
            _ => Asked::No,
        };
    }
    if req.protocol.is_some() || !req.headers.has_token("connection", "upgrade") {
        return Asked::No;
    }
    if !matches!(
        req.version,
        akio_http::Version::Http11 | akio_http::Version::Http10
    ) {
        return Asked::No;
    }
    for value in req.headers.get_all("upgrade") {
        let name = value.split(',').next().unwrap_or("").trim();
        if !name.is_empty() && !name.eq_ignore_ascii_case("h2c") {
            return Asked::Upgrade(name.to_owned());
        }
    }
    Asked::No
}

/// The upstream request the proxy built for the handshake, and what goes with it.
pub(crate) struct Handshake {
    pub(crate) out: ClientRequest,
    pub(crate) cancel: CancelToken,
    pub(crate) backend: usize,
    pub(crate) set_cookie: Option<String>,
    pub(crate) log: Option<Logged>,
    pub(crate) idle: Duration,
}

/// Passes `ctx`, which asked to switch (`asked`, not [`Asked::No`]), on to `pool`.
pub(crate) fn serve(pool: &Pool, ctx: &mut Context, asked: Asked, h: Handshake) {
    let (protocol, ext) = match asked {
        Asked::Upgrade(p) => (p, false),
        Asked::WebSocket => ("websocket".to_owned(), true),
        Asked::Other => return ctx.respond(crate::fail(501)),
        Asked::No => return ctx.respond(crate::fail(502)),
    };
    if pool.protocol() != UpstreamProtocol::Http11 {
        // akio's h2 client has no handshake to switch with: only HTTP/1.1 upstreams tunnel.
        return ctx.respond(crate::fail(502));
    }
    let Handshake {
        mut out,
        cancel,
        backend,
        set_cookie,
        log,
        idle,
    } = h;
    out.method = Method::Get;
    out.body = Vec::new();
    if ext {
        // An HTTP/2 or HTTP/3 WebSocket has no key: the handshake upstream needs one, and its
        // accept goes no further.
        let Some(key) = websocket_key() else {
            return ctx.respond(crate::fail(502));
        };
        out.headers.set("Sec-WebSocket-Key", key);
    }
    let out = out.upgrade(&protocol);
    let t = Arc::new(Tunnel {
        ext,
        idle,
        epoch: Instant::now(),
        last: AtomicU64::new(0),
        up: OnceLock::new(),
        client: OnceLock::new(),
        reads: OnceLock::new(),
        holding_client: AtomicBool::new(false),
        holding_up: AtomicBool::new(false),
        cancel: cancel.clone(),
        st: Mutex::new(St::default()),
    });
    let version = ctx.req().version;
    if ext {
        // An extended CONNECT's stream is its body, which akio streams: its reads are taken
        // before the context is detached.
        let Some(reads) = ctx.body_reads() else {
            return ctx.respond(crate::fail(502));
        };
        let _ = t.reads.set(reads.clone());
        let weak = Arc::downgrade(&t);
        ctx.on_gone(move || {
            if let Some(t) = weak.upgrade() {
                t.client_gone();
            }
        });
        let Some(answer) = ctx.detach() else {
            return;
        };
        t.lock().answer = Some(answer);
        let mut sink = Sink {
            t: t.clone(),
            reads,
            ended: false,
        };
        ctx.on_body(move |_, ev| sink.event(ev));
    } else if !ctx.upgrade_raw(ClientConn(t.clone())) {
        return ctx.respond(crate::fail(502));
    }
    let x = Ws {
        t,
        set_cookie,
        log,
        version,
        protocol,
        answer: Answer::Waiting,
    };
    pool.stream(backend, out, x);
}

/// A `Sec-WebSocket-Key`: 16 random bytes in base64 (RFC 6455 section 4.1).
fn websocket_key() -> Option<String> {
    let mut key = [0u8; 16];
    akio_crypto::rand::fill_random(&mut key).ok()?;
    Some(akio_crypto::base64::STANDARD.encode(key))
}

/// What the two sides of one request share.
struct Tunnel {
    /// The client asked with an extended CONNECT (HTTP/2 or HTTP/3).
    ext: bool,
    /// `tunnel_idle_timeout`.
    idle: Duration,
    /// What `last` counts from.
    epoch: Instant,
    /// When data last went either way, in milliseconds since `epoch`.
    last: AtomicU64,
    /// The upstream's connection, once it switched.
    up: OnceLock<RawConn>,
    /// An HTTP/1.1 client's connection, once akio handed it over.
    client: OnceLock<RawConn>,
    /// The reads of an extended CONNECT's stream.
    reads: OnceLock<BodyReads>,
    /// The client's reads are held for the upstream (or the upstream's for the client), until
    /// it drains: one hold at a time each way.
    holding_client: AtomicBool,
    holding_up: AtomicBool,
    /// The handshake's: cancelled when the client goes before the upstream answered.
    cancel: CancelToken,
    st: Mutex<St>,
}

/// The part of [`Tunnel`] that changes together.
#[derive(Default)]
struct St {
    /// An extended CONNECT's right to answer, until the upstream answered.
    answer: Option<Detached>,
    /// What the client sent before the upstream switched, its reads held meanwhile.
    early: Vec<u8>,
    /// An HTTP/1.1 client's bytes written before akio handed its connection over, and whether
    /// to close it after them.
    to_client: Vec<u8>,
    close_client: bool,
    /// Resumes the upstream's reads once `to_client` is written.
    resume: Option<Download>,
    /// The client ended its half (an extended CONNECT's END_STREAM) before the upstream switched.
    client_ended: bool,
    /// The upstream switched: `up` is set and the client's bytes go to it.
    switched: bool,
    /// The client went.
    gone: bool,
    /// The upstream's connection closed after it switched.
    over: bool,
}

fn lock(m: &Mutex<St>) -> MutexGuard<'_, St> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Tunnel {
    fn lock(&self) -> MutexGuard<'_, St> {
        lock(&self.st)
    }

    /// Data went one way: the idle timeout starts over.
    fn touch(&self) {
        let ms = u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.last.store(ms, Ordering::Relaxed);
    }

    /// Holds or resumes the client's reads.
    fn hold_client(&self, hold: bool) {
        if let Some(c) = self.client.get() {
            c.hold_reads(hold);
        }
        if let Some(r) = self.reads.get() {
            r.hold(hold);
        }
    }

    /// The client went (its connection closed, its stream reset): the handshake is given up, or
    /// the tunnel's upstream connection closed.
    fn client_gone(&self) {
        self.lock().gone = true;
        self.cancel.cancel();
        if let Some(up) = self.up.get() {
            up.close();
        }
    }

    /// Bytes for an HTTP/1.1 client, then its connection closed if `close`: written now, or once
    /// akio hands the connection over.
    fn to_client(&self, bytes: Vec<u8>, close: bool) {
        if let Some(c) = self.client.get() {
            let _ = c.send_owned(bytes);
            if close {
                c.close_after_send();
            }
            return;
        }
        let c = {
            let mut st = self.lock();
            match self.client.get() {
                Some(c) => c.clone(),
                None => {
                    st.to_client.extend_from_slice(&bytes);
                    st.close_client |= close;
                    return;
                }
            }
        };
        let _ = c.send_owned(bytes);
        if close {
            c.close_after_send();
        }
    }

    /// Paces what an HTTP/1.1 client was sent from `down`: its reads held while the client has
    /// more than [`TUNNEL_PENDING`] waiting, until the client drained.
    fn pace_refusal(&self, down: &Download) {
        let Some(c) = self.client.get() else {
            let mut st = self.lock();
            if self.client.get().is_none() {
                // Resumed once the connection is handed over and what waits is written.
                down.hold_reads(true);
                st.resume = Some(down.clone());
            }
            return;
        };
        if c.queued_bytes() > TUNNEL_PENDING {
            down.hold_reads(true);
            let down = down.clone();
            c.on_drain(move || down.hold_reads(false));
        }
    }

    /// What arrived from the client, `data`, to the upstream: kept, the client's reads held,
    /// while the upstream has not switched yet; dropped once the client went.
    fn from_client(self: &Arc<Self>, data: &[u8]) {
        self.touch();
        let up = match self.up.get() {
            Some(up) => up.clone(),
            None => {
                let mut st = self.lock();
                if st.gone {
                    return;
                }
                if !st.switched {
                    if st.early.len().saturating_add(data.len()) > EARLY_LIMIT {
                        // More than a handshake's client sends: given up.
                        st.early = Vec::new();
                        drop(st);
                        return self.too_early();
                    }
                    st.early.extend_from_slice(data);
                    // Resumed by the switch (`UpConn::on_open`), under the same lock.
                    self.hold_client(true);
                    return;
                }
                match self.up.get() {
                    Some(up) => up.clone(),
                    None => return,
                }
            }
        };
        if up.send(data).is_err() {
            return;
        }
        if up.queued_bytes() > TUNNEL_PENDING
            && self
                .holding_client
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            self.hold_client(true);
            let weak = Arc::downgrade(self);
            up.on_drain(move || {
                if let Some(t) = weak.upgrade() {
                    t.holding_client.store(false, Ordering::Release);
                    t.hold_client(false);
                }
            });
        }
    }

    /// A client that sent more than [`EARLY_LIMIT`] before the switch: the handshake is given
    /// up and the client answered 400 (an extended CONNECT) or closed.
    fn too_early(&self) {
        let answer = {
            let mut st = self.lock();
            st.gone = true;
            st.answer.take()
        };
        self.cancel.cancel();
        if let Some(a) = answer {
            a.respond(crate::fail(400));
        }
        if let Some(c) = self.client.get() {
            c.close();
        }
    }
}

/// An HTTP/1.1 client's connection, taken over at the handshake.
struct ClientConn(Arc<Tunnel>);

impl RawHandler for ClientConn {
    fn on_open(&mut self, conn: &RawConn) {
        let t = &self.0;
        // Paced by the relay, not by its own queue (akio's `set_relayed`).
        conn.set_relayed(true);
        let (pending, close, resume) = {
            let mut st = t.lock();
            let _ = t.client.set(conn.clone());
            if !st.switched && !st.close_client {
                // Nothing goes up before the upstream switched: its first bytes wait there.
                conn.hold_reads(true);
            }
            (
                std::mem::take(&mut st.to_client),
                st.close_client,
                st.resume.take(),
            )
        };
        if !pending.is_empty() {
            let _ = conn.send_owned(pending);
        }
        if close {
            conn.close_after_send();
        }
        if let Some(down) = resume {
            t.pace_refusal(&down);
            if conn.queued_bytes() <= TUNNEL_PENDING {
                down.hold_reads(false);
            }
        }
    }

    fn on_data(&mut self, _conn: &RawConn, data: &[u8]) {
        self.0.from_client(data);
    }

    fn on_close(&mut self) {
        let t = &self.0;
        t.lock().gone = true;
        t.cancel.cancel();
        if let Some(up) = t.up.get() {
            up.close_after_send();
        }
    }
}

/// An extended CONNECT's stream, as it arrives: the client's half of the tunnel.
struct Sink {
    t: Arc<Tunnel>,
    reads: BodyReads,
    ended: bool,
}

impl Sink {
    fn event(&mut self, ev: BodyEvent<'_>) {
        match ev {
            BodyEvent::Chunk(data) => self.t.from_client(data),
            BodyEvent::End(_) => {
                self.ended = true;
                let up = {
                    let mut st = self.t.lock();
                    if !st.switched {
                        st.client_ended = true;
                        return;
                    }
                    self.t.up.get().cloned()
                };
                if let Some(up) = up {
                    up.close_after_send();
                }
            }
            // Bounded by a `body_limit` (akio's `Context::limit_body`; the listener bounds no
            // tunnel): akio stops the stream, and the sink, dropped next, ends the tunnel as a
            // client gone.
            BodyEvent::TooLarge => {}
        }
    }
}

impl Drop for Sink {
    /// Dropped before the stream ended: the client reset it, or its connection went.
    fn drop(&mut self) {
        if !self.ended {
            self.reads.hold(false);
            self.t.client_gone();
        }
    }
}

/// The upstream's connection once it switched: the upstream's half of the tunnel.
struct UpConn {
    t: Arc<Tunnel>,
    /// The client's streamed `200`, for an extended CONNECT.
    stream: Option<BodyStream>,
}

impl RawHandler for UpConn {
    fn on_open(&mut self, conn: &RawConn) {
        let t = &self.t;
        conn.set_relayed(true);
        let (early, ended, gone) = {
            let mut st = t.lock();
            let _ = t.up.set(conn.clone());
            st.switched = true;
            let early = std::mem::take(&mut st.early);
            // What the client sent meanwhile is written first: its reads go on after it.
            if !st.gone {
                t.hold_client(false);
            }
            (early, st.client_ended, st.gone)
        };
        if gone {
            return conn.close();
        }
        t.touch();
        if !early.is_empty() {
            let _ = conn.send_owned(early);
        }
        if ended {
            conn.close_after_send();
        }
        arm_idle(&self.t, t.idle);
    }

    fn on_data(&mut self, conn: &RawConn, data: &[u8]) {
        let t = &self.t;
        t.touch();
        match &self.stream {
            Some(s) => {
                if s.write(data).is_err() {
                    // The client's stream is gone.
                    return conn.close();
                }
                if s.queued_bytes() > TUNNEL_PENDING
                    && t.holding_up
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                {
                    conn.hold_reads(true);
                    let (conn, weak) = (conn.clone(), Arc::downgrade(t));
                    s.on_drain(move || {
                        if let Some(t) = weak.upgrade() {
                            t.holding_up.store(false, Ordering::Release);
                        }
                        conn.hold_reads(false);
                    });
                }
            }
            None => {
                let Some(c) = t.client.get() else {
                    // Not handed over yet (it was, before the switch was answered): kept.
                    return t.to_client(data.to_vec(), false);
                };
                if c.send(data).is_err() {
                    return conn.close();
                }
                if c.queued_bytes() > TUNNEL_PENDING
                    && t.holding_up
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                {
                    conn.hold_reads(true);
                    let (conn, weak) = (conn.clone(), Arc::downgrade(t));
                    c.on_drain(move || {
                        if let Some(t) = weak.upgrade() {
                            t.holding_up.store(false, Ordering::Release);
                        }
                        conn.hold_reads(false);
                    });
                }
            }
        }
    }

    fn on_close(&mut self) {
        self.t.lock().over = true;
        // The upstream ended: the client's side ends once what it was sent is written.
        match self.stream.take() {
            Some(s) => s.finish(),
            None => self.t.to_client(Vec::new(), true),
        }
    }
}

/// Closes `t` once it carried nothing either way for its idle timeout, looking again `after`.
fn arm_idle(t: &Arc<Tunnel>, after: Duration) {
    let weak = Arc::downgrade(t);
    // Past the timers' bound the tunnel is not timed out, as before akio's timer thread was
    // full; its connections' own write timeouts still apply.
    let _ = akio::timer::after_send(after, move || {
        let Some(t) = weak.upgrade() else { return };
        if t.lock().over {
            return;
        }
        let now = u64::try_from(t.epoch.elapsed().as_millis()).unwrap_or(u64::MAX);
        let since = Duration::from_millis(now.saturating_sub(t.last.load(Ordering::Relaxed)));
        if since >= t.idle {
            if let Some(up) = t.up.get() {
                up.close();
            }
            if let Some(c) = t.client.get() {
                c.close();
            }
            return;
        }
        arm_idle(&t, t.idle.saturating_sub(since));
    });
}

/// How the client is answered when the upstream does not switch.
enum Answer {
    /// No answer from the upstream yet.
    Waiting,
    /// An extended CONNECT's, through the proxy's ordinary relay.
    Relay(Box<relay::Relay>),
    /// An HTTP/1.1 client's, collected to be written whole.
    Whole { head: Vec<u8>, body: Vec<u8> },
    /// An HTTP/1.1 client's, written as it arrives.
    Stream(Download),
    /// Answered, switched, or given up.
    Done,
}

/// The upstream's side of the handshake: akit-upstream's [`Exchange`].
struct Ws {
    t: Arc<Tunnel>,
    set_cookie: Option<String>,
    log: Option<Logged>,
    version: akio_http::Version,
    /// What the client asked to switch to.
    protocol: String,
    answer: Answer,
}

impl Ws {
    /// The right to answer an extended CONNECT, if it is still here's.
    fn take_answer(&self) -> Option<Detached> {
        self.t.lock().answer.take()
    }

    /// Answers the client `status`, fit's way, unless it went.
    fn fail(&self, status: u16) {
        if self.t.ext {
            if let Some(a) = self.take_answer() {
                let resp = crate::fail(status);
                logged(self.log.as_ref(), || a.respond(resp));
            }
            return;
        }
        if self.t.lock().gone {
            return;
        }
        let resp = crate::fail(status);
        let mut out = status_line(status);
        out.extend_from_slice(b"Content-Type: text/plain; charset=utf-8\r\n");
        out.extend_from_slice(format!("Content-Length: {}\r\n", resp.body.len()).as_bytes());
        out.extend_from_slice(b"Connection: close\r\n\r\n");
        out.extend_from_slice(&resp.body);
        self.t.to_client(out, true);
    }

    /// An HTTP/1.1 client's head for the upstream's answer `head`: its fields without the
    /// hop-by-hop ones (with its length when `length`), then `extra`, then the sticky cookie.
    fn h1_head(&mut self, head: &Head, length: bool, extra: &[(&str, &str)]) -> Vec<u8> {
        let mut out = status_line(head.status);
        let fields = headers::incoming(&head.headers, length);
        for (n, v) in fields.iter() {
            push_field(&mut out, n, v);
        }
        for (n, v) in extra {
            push_field(&mut out, n, v);
        }
        if let Some(c) = self.set_cookie.take() {
            push_field(&mut out, "Set-Cookie", &c);
        }
        out.extend_from_slice(b"\r\n");
        out
    }
}

/// `HTTP/1.1 <status> <reason>` and its line end.
fn status_line(status: u16) -> Vec<u8> {
    let reason = akio_http::message::reason(status);
    format!("HTTP/1.1 {status} {reason}\r\n").into_bytes()
}

fn push_field(out: &mut Vec<u8>, name: &str, value: &str) {
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(b": ");
    // akio's parser gave no line breaks in a value; none go out either way.
    out.extend(value.bytes().filter(|b| *b != b'\r' && *b != b'\n'));
    out.extend_from_slice(b"\r\n");
}

impl Exchange for Ws {
    fn on_upgrade(&mut self, head: Head) -> Option<Box<dyn RawHandler>> {
        self.answer = Answer::Done;
        if self.t.ext {
            let answer = self.take_answer()?;
            let mut fields = headers::incoming(&head.headers, false);
            fields.remove("sec-websocket-accept");
            if let Some(c) = self.set_cookie.take() {
                fields.append("Set-Cookie", c);
            }
            let stream = logged(self.log.as_ref(), || answer.stream_response(200, fields))?;
            return Some(Box::new(UpConn {
                t: self.t.clone(),
                stream: Some(stream),
            }));
        }
        if self.t.lock().gone {
            return None;
        }
        let upgrade = head
            .headers
            .get("upgrade")
            .unwrap_or(&self.protocol)
            .to_owned();
        let out = self.h1_head(
            &head,
            false,
            &[("Connection", "Upgrade"), ("Upgrade", &upgrade)],
        );
        self.t.to_client(out, false);
        Some(Box::new(UpConn {
            t: self.t.clone(),
            stream: None,
        }))
    }

    fn on_head(&mut self, head: Head, down: Download) {
        if self.t.ext {
            if (200..300).contains(&head.status) {
                // A 2xx without a switch: the client would take it for an open tunnel and its
                // body for WebSocket frames. The gateway's failure; not the upstream's.
                down.cancel();
                self.answer = Answer::Done;
                return self.fail(502);
            }
            let Some(answer) = self.take_answer() else {
                down.cancel();
                self.answer = Answer::Done;
                return;
            };
            let mut r = relay::Relay::new(
                relay::Client::answered(answer),
                false,
                self.version,
                false,
                self.log.clone(),
                self.set_cookie.take(),
                // A handshake answered with a response is not sent again (I45's retries).
                None,
            );
            r.on_head(head, down);
            self.answer = Answer::Relay(Box::new(r));
            return;
        }
        if self.t.lock().gone {
            down.cancel();
            self.answer = Answer::Done;
            return;
        }
        let length = head
            .headers
            .get("content-length")
            .and_then(|v| v.trim().parse::<u64>().ok());
        let bodyless = matches!(head.status, 100..=199 | 204 | 304);
        if bodyless || length.is_some_and(|n| n <= WHOLE_REFUSAL) {
            let out = self.h1_head(&head, !bodyless, &[("Connection", "close")]);
            let capacity = if bodyless { 0 } else { length.unwrap_or(0) };
            self.answer = Answer::Whole {
                head: out,
                body: Vec::with_capacity(usize::try_from(capacity).unwrap_or(0)),
            };
            return;
        }
        // Framed by its length when it has one, else by the connection's close.
        let out = self.h1_head(&head, length.is_some(), &[("Connection", "close")]);
        self.t.to_client(out, false);
        self.answer = Answer::Stream(down);
    }

    fn on_data(&mut self, chunk: &[u8]) {
        match &mut self.answer {
            Answer::Relay(r) => r.on_data(chunk),
            Answer::Whole { body, .. } => body.extend_from_slice(chunk),
            Answer::Stream(down) => {
                let down = down.clone();
                self.t.to_client(chunk.to_vec(), false);
                self.t.pace_refusal(&down);
            }
            Answer::Waiting | Answer::Done => {}
        }
    }

    fn on_end(&mut self, trailers: &Headers) {
        match std::mem::replace(&mut self.answer, Answer::Done) {
            Answer::Relay(mut r) => r.on_end(trailers),
            Answer::Whole { mut head, body } => {
                head.extend_from_slice(&body);
                self.t.to_client(head, true);
            }
            Answer::Stream(_) => self.t.to_client(Vec::new(), true),
            Answer::Waiting | Answer::Done => {}
        }
    }

    fn on_error(&mut self, e: Error) {
        match std::mem::replace(&mut self.answer, Answer::Done) {
            Answer::Relay(mut r) => r.on_error(e),
            // Cut short mid-body: the client's connection closes before the length it was told.
            Answer::Stream(_) => {
                if let Some(c) = self.t.client.get() {
                    c.close();
                } else {
                    self.t.to_client(Vec::new(), true);
                }
            }
            Answer::Waiting | Answer::Whole { .. } => {
                if !self.t.lock().gone {
                    self.fail(crate::status_of(&e));
                }
            }
            Answer::Done => {}
        }
    }

    fn gave_up(&self) -> bool {
        self.t.lock().gone
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn req(method: Method, version: akio_http::Version, fields: &[(&str, &str)]) -> Request {
        let mut r = Request::new(method, "/");
        r.version = version;
        for (n, v) in fields {
            r.headers.append(*n, *v);
        }
        r
    }

    #[test]
    fn what_is_asked() {
        use akio_http::Version::{Http2, Http10, Http11};
        let ws = [
            ("Connection", "keep-alive, Upgrade"),
            ("Upgrade", "websocket"),
        ];
        assert_eq!(
            asked(&req(Method::Get, Http11, &ws)),
            Asked::Upgrade("websocket".into())
        );
        assert_eq!(
            asked(&req(Method::Get, Http10, &ws)),
            Asked::Upgrade("websocket".into())
        );
        // Without the Connection token, h2c, or over HTTP/2: no switch.
        assert_eq!(
            asked(&req(Method::Get, Http11, &[("Upgrade", "websocket")])),
            Asked::No
        );
        assert_eq!(
            asked(&req(
                Method::Get,
                Http11,
                &[
                    ("Connection", "Upgrade, HTTP2-Settings"),
                    ("Upgrade", "h2c")
                ]
            )),
            Asked::No
        );
        assert_eq!(asked(&req(Method::Get, Http2, &ws)), Asked::No);
        // The first protocol named.
        assert_eq!(
            asked(&req(
                Method::Get,
                Http11,
                &[("Connection", "upgrade"), ("Upgrade", "IRC/6.9, websocket")]
            )),
            Asked::Upgrade("IRC/6.9".into())
        );
        let mut c = req(Method::Connect, Http2, &[]);
        c.protocol = Some("websocket".into());
        assert_eq!(asked(&c), Asked::WebSocket);
        c.protocol = Some("connect-udp".into());
        assert_eq!(asked(&c), Asked::Other);
        c.protocol = None;
        assert_eq!(asked(&c), Asked::No);
    }

    #[test]
    fn keys_and_heads() {
        let k = websocket_key().unwrap();
        assert_eq!(akio_crypto::base64::STANDARD.decode(&k).unwrap().len(), 16);
        assert_ne!(k, websocket_key().unwrap());
        assert_eq!(status_line(101), b"HTTP/1.1 101 Switching Protocols\r\n");
        let mut out = Vec::new();
        push_field(&mut out, "X-A", "b\r\nc");
        assert_eq!(out, b"X-A: bc\r\n");
    }
}
