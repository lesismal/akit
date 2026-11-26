//! fit's `tunnel/internal/link`, byte for byte (decision 038): framed calls, replies and notifies
//! over a transport connection, and the tunnel frames among them.
//!
//! ```text
//! frame  = length:u32 header body        length: header and body, 6 to the link's limit
//! header = kind:u8 method:u8 id:u32      kinds: 1 call, 2 reply, 3 error, 4 notify, 0x85 hello
//! ```
//!
//! With a body password, each side first sends a hello (version 3, 16 random bytes; version 4
//! naming the token after them), and every frame after has its body sealed (`crate::crypt`), bit
//! 0x80 of its kind set. A frame that does not open, a hello of another version, one sent back, an
//! unsealed frame on a sealed link or a sealed one on a plain link closes the link.
//!
//! On akio: a link is read in its connection's `on_data`, on an I/O thread, and its handlers run
//! there: they must not block. A call that cannot be answered at once keeps its [`Responder`] and
//! answers later from any thread; calls the link makes are answered through a callback, their
//! timeouts on akio's timer thread. Nothing here waits.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::crypt::{Coder, OVERHEAD, RANDOM_SIZE, Session};

/// The size of a frame's length, and of its header.
pub const LENGTH_SIZE: usize = 4;
/// The size of a frame's header.
pub const HEADER_SIZE: usize = 6;
/// The most bytes of header and body a frame has.
pub const MAX_FRAME: usize = 4 << 20;

const KIND_CALL: u8 = 1;
const KIND_REPLY: u8 = 2;
const KIND_ERROR: u8 = 3;
const KIND_NOTIFY: u8 = 4;
const KIND_HELLO: u8 = 5;
const KIND_SEALED: u8 = 0x80;

/// The link protocol's version a hello names.
pub const VERSION: u8 = 3;
/// The version of a hello whose link registers with a token.
pub const VERSION_TOKEN: u8 = 4;

/// What waits before the peer's hello: a registration, not the tunnels after it.
const MAX_BEFORE_HELLO: usize = MAX_FRAME;

/// The transport a link runs over: an akio connection (TCP or TLS), a QUIC stream or a WebSocket.
/// Every method must not block.
pub trait Wire: Send + Sync + 'static {
    /// Queues `b`; false when the connection is gone or its queue is full.
    fn send(&self, b: &[u8]) -> bool;
    /// Closes now.
    fn close(&self);
    /// Closes once what is queued is sent.
    fn close_after_send(&self);
    /// The peer's address.
    fn peer(&self) -> String;
    /// Stops or resumes reading.
    fn hold_reads(&self, hold: bool);
    /// Bytes queued and not yet sent.
    fn queued(&self) -> usize;
    /// Runs `f` once what is queued drains.
    fn on_drain(&self, f: Box<dyn FnOnce() + Send>);
    /// Bounds what may be queued at `n` bytes, past the engine's bound or below it: a send past
    /// it fails. Nothing for a wire whose queue has a bound of its own.
    fn set_max_queued(&self, _n: usize) {}
}

/// Why a call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// The link closed, or would not send.
    Closed(String),
    /// Not answered in time.
    Timeout,
    /// The other side answered with a failure.
    Remote(String),
    /// The answer failed the caller's check: the link closes.
    Check(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Closed(e) if e.is_empty() => f.write_str("link: closed"),
            CallError::Closed(e) => write!(f, "link: closed: {e}"),
            CallError::Timeout => f.write_str("link: call timed out"),
            CallError::Remote(e) | CallError::Check(e) => f.write_str(e),
        }
    }
}

/// What a call's caller is told.
pub type Done = Box<dyn FnOnce(Result<Vec<u8>, CallError>) + Send>;
/// A caller's check of an answer, before it is taken: an error closes the link.
pub type Check = Box<dyn FnOnce(&[u8]) -> Result<(), String> + Send>;

/// What the links that share it serve. Its methods run on the link's I/O thread: they must not
/// block.
pub trait Router: Send + Sync + 'static {
    /// Runs before each call or notify is served: an error fails the call with it, or drops the
    /// notify.
    ///
    /// # Errors
    ///
    /// Why the method is not served on this link now.
    fn guard(&self, _l: &Arc<Link>, _method: u8) -> Result<(), String> {
        Ok(())
    }
    /// Serves a call: answers it through `r`, now or later.
    fn call(&self, l: &Arc<Link>, method: u8, body: &[u8], r: Responder);
    /// Serves a notify.
    fn notify(&self, l: &Arc<Link>, method: u8, body: &[u8]);
    /// The link closed, with why.
    fn closed(&self, l: &Arc<Link>, err: &str);
}

/// Answers one call, once; a responder dropped unanswered answers with an error.
pub struct Responder {
    link: Arc<Link>,
    method: u8,
    id: u32,
    done: bool,
}

impl std::fmt::Debug for Responder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Responder")
            .field("method", &self.method)
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Responder {
    /// Answers with `body`.
    pub fn ok(mut self, body: &[u8]) {
        self.done = true;
        self.link.send(KIND_REPLY, self.method, self.id, &[body]);
    }

    /// Answers with a failure.
    pub fn err(mut self, msg: &str) {
        self.done = true;
        self.link
            .send(KIND_ERROR, self.method, self.id, &[msg.as_bytes()]);
    }

    /// The link the call came on.
    #[must_use]
    pub fn link(&self) -> &Arc<Link> {
        &self.link
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        if !self.done {
            self.link.send(
                KIND_ERROR,
                self.method,
                self.id,
                &[b"link: the call was dropped unanswered"],
            );
        }
    }
}

struct Pending {
    done: Done,
    check: Option<Check>,
}

struct State {
    random: [u8; RANDOM_SIZE],
    recv: Option<Session>,
    send: Option<Session>,
    held: Vec<Vec<u8>>,
    held_size: usize,
    input: Vec<u8>,
    seq: u32,
    pending: HashMap<u32, Pending>,
    closed: bool,
    failing: bool,
    err: String,
    hello_token: String,
    coder: Option<Coder>,
}

/// A link: one transport connection of the tunnel's protocol.
pub struct Link {
    /// A number of the process's own, for the roles' tables.
    pub id: u64,
    wire: Arc<dyn Wire>,
    router: Arc<dyn Router>,
    max_frame: AtomicUsize,
    st: Mutex<State>,
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Link")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

static NEXT: AtomicU64 = AtomicU64::new(1);

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Link {
    /// A link on `wire`, served by `router`, its bodies sealed by `coder` (the dialer's coder on
    /// the side that dialed). With a coder, its hello is sent at once. The wire's queue is bounded
    /// by [`LINK_QUEUE`](crate::msg::LINK_QUEUE), as fit's links' are: its tunnels' windows pace
    /// what waits on it.
    pub fn new(wire: Arc<dyn Wire>, router: Arc<dyn Router>, coder: Option<Coder>) -> Arc<Link> {
        wire.set_max_queued(crate::msg::LINK_QUEUE);
        let random = crate::crypt::random();
        let hello = coder.as_ref().map(|c| {
            let tok = c.token_id();
            let mut body = vec![if tok.is_empty() {
                VERSION
            } else {
                VERSION_TOKEN
            }];
            body.extend_from_slice(&random);
            if !tok.is_empty() {
                body.push(u8::try_from(tok.len()).unwrap_or(u8::MAX));
                body.extend_from_slice(tok.as_bytes());
            }
            let mut f = Vec::with_capacity(body.len().saturating_add(LENGTH_SIZE + HEADER_SIZE));
            let n = u32::try_from(body.len().saturating_add(HEADER_SIZE)).unwrap_or(u32::MAX);
            f.extend_from_slice(&n.to_be_bytes());
            f.extend_from_slice(&[KIND_SEALED | KIND_HELLO, 0, 0, 0, 0, 0]);
            f.extend_from_slice(&body);
            f
        });
        let l = Arc::new(Link {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            wire,
            router,
            max_frame: AtomicUsize::new(MAX_FRAME),
            st: Mutex::new(State {
                random,
                recv: None,
                send: None,
                held: Vec::new(),
                held_size: 0,
                input: Vec::new(),
                seq: 0,
                pending: HashMap::new(),
                closed: false,
                failing: false,
                err: String::new(),
                hello_token: String::new(),
                coder,
            }),
        });
        if let Some(h) = hello {
            l.wire.send(&h);
        }
        l
    }

    /// Lowers the most bytes a frame may have (an unregistered link's 64 KiB).
    pub fn set_max_frame(&self, n: usize) {
        self.max_frame.store(n.min(MAX_FRAME), Ordering::Relaxed);
    }

    /// The token the peer's hello named, on a listener.
    #[must_use]
    pub fn hello_token(&self) -> String {
        lock(&self.st).hello_token.clone()
    }

    /// The peer's address.
    #[must_use]
    pub fn remote(&self) -> String {
        self.wire.peer()
    }

    /// Whether the link is closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        lock(&self.st).closed
    }

    /// Stops or resumes reading the link's connection.
    pub fn hold_reads(&self, hold: bool) {
        self.wire.hold_reads(hold);
    }

    /// Closes the link now.
    pub fn close(&self) {
        self.wire.close();
    }

    /// Closes the link once what is queued is sent.
    pub fn close_after_send(&self) {
        self.wire.close_after_send();
    }

    /// Builds a frame and sends it, sealed on a sealed link; held until the peer's hello comes.
    fn send(&self, kind: u8, method: u8, id: u32, parts: &[&[u8]]) -> bool {
        let n: usize = parts.iter().map(|p| p.len()).sum();
        let mut st = lock(&self.st);
        if st.closed {
            return false;
        }
        let sealed = st.coder.is_some();
        let extra = if sealed { OVERHEAD } else { 0 };
        let total = HEADER_SIZE.saturating_add(n).saturating_add(extra);
        if total > MAX_FRAME {
            return false;
        }
        let mut f = Vec::with_capacity(total.saturating_add(LENGTH_SIZE));
        f.extend_from_slice(&u32::try_from(total).unwrap_or(u32::MAX).to_be_bytes());
        f.push(if sealed { kind | KIND_SEALED } else { kind });
        f.push(method);
        f.extend_from_slice(&id.to_be_bytes());
        for p in parts {
            f.extend_from_slice(p);
        }
        if !sealed {
            drop(st);
            return self.wire.send(&f);
        }
        f.resize(total.saturating_add(LENGTH_SIZE), 0);
        match st.send.as_mut() {
            None => {
                if st.held_size.saturating_add(f.len()) > MAX_BEFORE_HELLO {
                    drop(st);
                    self.fail("link: too much sent before the peer's hello");
                    return false;
                }
                st.held_size = st.held_size.saturating_add(f.len());
                st.held.push(f);
                true
            }
            Some(sess) => {
                sess.seal_in_place(
                    &mut f,
                    LENGTH_SIZE..LENGTH_SIZE + HEADER_SIZE,
                    LENGTH_SIZE + HEADER_SIZE,
                );
                // Under the lock: the frames go in the order of their numbers.
                self.wire.send(&f)
            }
        }
    }

    /// Calls `method` with `body`; `done` is told the answer, or why there is none, within
    /// `timeout`. `check`, when given, checks the answer first; its error closes the link.
    pub fn call(
        self: &Arc<Self>,
        method: u8,
        body: &[u8],
        timeout: Duration,
        check: Option<Check>,
        done: Done,
    ) {
        let id = {
            let mut st = lock(&self.st);
            if st.closed {
                let e = st.err.clone();
                drop(st);
                done(Err(CallError::Closed(e)));
                return;
            }
            st.seq = st.seq.wrapping_add(1);
            if st.seq == 0 {
                st.seq = 1;
            }
            let id = st.seq;
            st.pending.insert(id, Pending { done, check });
            id
        };
        if !self.send(KIND_CALL, method, id, &[body]) {
            if let Some(p) = lock(&self.st).pending.remove(&id) {
                (p.done)(Err(CallError::Closed(String::new())));
            }
            return;
        }
        let weak = Arc::downgrade(self);
        let timer = akio::timer::after_send(timeout, move || {
            if let Some(l) = weak.upgrade() {
                let p = lock(&l.st).pending.remove(&id);
                if let Some(p) = p {
                    (p.done)(Err(CallError::Timeout));
                }
            }
        });
        if timer.is_err() {
            // No timer: the call is not left waiting forever.
            if let Some(p) = lock(&self.st).pending.remove(&id) {
                (p.done)(Err(CallError::Timeout));
            }
        }
    }

    /// Sends a notify.
    pub fn notify(&self, method: u8, body: &[u8]) -> bool {
        self.send(KIND_NOTIFY, method, 0, &[body])
    }

    /// Sends a tunnel frame: the tunnel's ID, then the payload.
    pub fn send_tunnel(&self, method: u8, id: u64, payload: &[u8]) -> bool {
        self.send(KIND_NOTIFY, method, 0, &[&id.to_be_bytes(), payload])
    }

    /// Closes the link for a protocol error.
    fn fail(&self, err: &str) {
        {
            let mut st = lock(&self.st);
            if st.failing {
                return;
            }
            st.failing = true;
            if st.err.is_empty() {
                err.clone_into(&mut st.err);
            }
        }
        akit_observe::debug!(Tunnel, "link to {}: {err}", self.wire.peer());
        self.wire.close();
    }

    /// Bytes arrived on the link's connection: on its I/O thread.
    pub fn on_data(self: &Arc<Self>, data: &[u8]) {
        let mut frames: Vec<(u8, u8, u32, Vec<u8>)> = Vec::new();
        let mut failed: Option<String> = None;
        let mut flush: Vec<Vec<u8>> = Vec::new();
        {
            let mut st = lock(&self.st);
            if st.failing || st.closed {
                return;
            }
            st.input.extend_from_slice(data);
            let limit = self.max_frame.load(Ordering::Relaxed);
            let mut at = 0usize;
            while let Some(len) = st
                .input
                .get(at..at.saturating_add(LENGTH_SIZE))
                .and_then(|b| <[u8; 4]>::try_from(b).ok())
                .map(u32::from_be_bytes)
            {
                let n = usize::try_from(len).unwrap_or(usize::MAX);
                if n < HEADER_SIZE || n > limit {
                    failed = Some(format!("link: a frame of {n} bytes"));
                    break;
                }
                let start = at.saturating_add(LENGTH_SIZE);
                let end = start.saturating_add(n);
                if st.input.len() < end {
                    break;
                }
                let mut frame = st
                    .input
                    .get(start..end)
                    .map(<[u8]>::to_vec)
                    .unwrap_or_default();
                at = end;
                match self.unseal(&mut st, &mut frame, &mut flush) {
                    Ok(None) => {}
                    Ok(Some(f)) => frames.push(f),
                    Err(e) => {
                        failed = Some(e);
                        break;
                    }
                }
            }
            let used = at.min(st.input.len());
            st.input.drain(..used);
            if st.input.capacity() > 1 << 20 && st.input.is_empty() {
                st.input = Vec::new();
            }
            for f in flush {
                if !self.wire.send(&f) {
                    break;
                }
            }
        }
        for (kind, method, id, body) in frames {
            self.dispatch(kind, method, id, body);
        }
        if let Some(e) = failed {
            self.fail(&e);
        }
    }

    /// Opens a frame: its kind, method, ID and plain body; `None` for a hello taken.
    #[allow(clippy::type_complexity)]
    fn unseal(
        &self,
        st: &mut State,
        frame: &mut [u8],
        flush: &mut Vec<Vec<u8>>,
    ) -> Result<Option<(u8, u8, u32, Vec<u8>)>, String> {
        let (header, body) = frame.split_at_mut(HEADER_SIZE);
        let mut kind = header.first().copied().unwrap_or(0);
        let method = header.get(1).copied().unwrap_or(0);
        let id = header
            .get(2..6)
            .and_then(|b| <[u8; 4]>::try_from(b).ok())
            .map_or(0, u32::from_be_bytes);
        let sealed_link = st.coder.is_some();
        if sealed_link && kind & KIND_SEALED == 0 {
            return Err(
                "link: received an unencrypted frame; is a body password set on both sides?"
                    .to_owned(),
            );
        }
        if !sealed_link && kind & KIND_SEALED != 0 {
            return Err("link: received an encrypted frame without a body password".to_owned());
        }
        if sealed_link && st.recv.is_none() {
            if kind != KIND_SEALED | KIND_HELLO {
                return Err(format!(
                    "link: the peer sealed a frame without a hello: it runs a fit whose link protocol is version 1, this side's is {VERSION}; upgrade it"
                ));
            }
            self.hello(st, body, flush)?;
            return Ok(None);
        }
        if sealed_link {
            if kind == KIND_SEALED | KIND_HELLO {
                return Err("link: a hello that is not the first frame, or is malformed".to_owned());
            }
            let header: Vec<u8> = header.to_vec();
            let Some(recv) = st.recv.as_mut() else {
                return Err("link: no session".to_owned());
            };
            let plain = recv.open(body, &header)?;
            kind &= !KIND_SEALED;
            return Ok(Some((kind, method, id, plain.to_vec())));
        }
        Ok(Some((kind, method, id, body.to_vec())))
    }

    fn hello(&self, st: &mut State, body: &[u8], flush: &mut Vec<Vec<u8>>) -> Result<(), String> {
        let Some(coder) = st.coder.clone() else {
            return Err("link: no body password".to_owned());
        };
        let version = body
            .first()
            .copied()
            .ok_or("link: a hello that is not the first frame, or is malformed")?;
        let mut coder = coder;
        let random = match version {
            VERSION if body.len() == 1 + RANDOM_SIZE => body.get(1..).unwrap_or_default().to_vec(),
            VERSION_TOKEN
                if body.len() > 2 + RANDOM_SIZE
                    && usize::from(body.get(1 + RANDOM_SIZE).copied().unwrap_or(0))
                        == body.len().saturating_sub(2 + RANDOM_SIZE) =>
            {
                let id = String::from_utf8_lossy(body.get(2 + RANDOM_SIZE..).unwrap_or_default())
                    .into_owned();
                coder = coder
                    .for_token(&id)
                    .map_err(|e| format!("link: the peer's hello names token {id}: {e}"))?;
                st.hello_token = id;
                body.get(1..=RANDOM_SIZE).unwrap_or_default().to_vec()
            }
            VERSION | VERSION_TOKEN => {
                return Err("link: a hello that is not the first frame, or is malformed".to_owned());
            }
            v => {
                return Err(format!(
                    "link: the peer's link protocol is version {v}, this side's {VERSION_TOKEN}; run the same version on both sides"
                ));
            }
        };
        let random_own = st.random;
        let recv = coder.session(&random_own, &random)?;
        let mut send = coder.session(&random_own, &random)?;
        for mut f in std::mem::take(&mut st.held) {
            send.seal_in_place(
                &mut f,
                LENGTH_SIZE..LENGTH_SIZE + HEADER_SIZE,
                LENGTH_SIZE + HEADER_SIZE,
            );
            flush.push(f);
        }
        st.held_size = 0;
        st.recv = Some(recv);
        st.send = Some(send);
        Ok(())
    }

    fn dispatch(self: &Arc<Self>, kind: u8, method: u8, id: u32, body: Vec<u8>) {
        match kind {
            KIND_REPLY | KIND_ERROR => {
                let p = lock(&self.st).pending.remove(&id);
                let Some(p) = p else { return };
                if kind == KIND_ERROR {
                    (p.done)(Err(CallError::Remote(
                        String::from_utf8_lossy(&body).into_owned(),
                    )));
                    return;
                }
                if let Some(check) = p.check {
                    if let Err(e) = check(&body) {
                        (p.done)(Err(CallError::Check(e.clone())));
                        self.fail(&e);
                        return;
                    }
                }
                (p.done)(Ok(body));
            }
            KIND_CALL => {
                let r = Responder {
                    link: Arc::clone(self),
                    method,
                    id,
                    done: false,
                };
                if let Err(e) = self.router.guard(self, method) {
                    r.err(&e);
                    return;
                }
                self.router.call(self, method, &body, r);
            }
            KIND_NOTIFY => {
                if self.router.guard(self, method).is_ok() {
                    self.router.notify(self, method, &body);
                }
            }
            k => self.fail(&format!("link: a frame of kind {k}")),
        }
    }

    /// The link's connection closed: its calls fail, and its router is told.
    pub fn on_closed(self: &Arc<Self>, err: Option<String>) {
        let (pending, why) = {
            let mut st = lock(&self.st);
            if st.closed {
                return;
            }
            st.closed = true;
            if st.err.is_empty() {
                if let Some(e) = &err {
                    e.clone_into(&mut st.err);
                }
            }
            (std::mem::take(&mut st.pending), st.err.clone())
        };
        for (_, p) in pending {
            (p.done)(Err(CallError::Closed(why.clone())));
        }
        self.router.closed(self, &why);
    }
}

/// akio's handler of a link's connection.
pub struct LinkHandler(pub Option<Arc<Link>>);

impl std::fmt::Debug for LinkHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkHandler").finish_non_exhaustive()
    }
}

impl akio::Handler for LinkHandler {
    fn on_data(&mut self, _c: &akio::Conn<Self>, data: &[u8]) {
        if let Some(l) = &self.0 {
            l.on_data(data);
        }
    }

    fn on_close(&mut self, _c: &akio::Conn<Self>, err: Option<akio::Error>) {
        if let Some(l) = self.0.take() {
            l.on_closed(err.map(|e| e.to_string()));
        }
    }
}

/// A link's wire on an akio connection.
pub struct ConnWire<H: akio::Handler>(pub akio::Conn<H>);

impl<H: akio::Handler> ConnWire<H> {
    /// The wire of `c`, which no longer stops reading while its own output is queued (akio's
    /// `set_relayed`, fit's `SetWriteBackpressure(false)`): both ends of a link writing to each
    /// other would otherwise each stop reading the other's frames and wait forever. Its senders
    /// pace themselves instead, each tunnel by its window.
    #[must_use]
    pub fn new(c: &akio::Conn<H>) -> ConnWire<H> {
        c.set_relayed(true);
        ConnWire(c.clone())
    }
}

impl<H: akio::Handler> std::fmt::Debug for ConnWire<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ConnWire").field(&self.0).finish()
    }
}

impl<H: akio::Handler> Wire for ConnWire<H> {
    fn send(&self, b: &[u8]) -> bool {
        self.0.send(b).is_ok()
    }
    fn close(&self) {
        self.0.close();
    }
    fn close_after_send(&self) {
        self.0.close_after_send();
    }
    fn peer(&self) -> String {
        self.0.peer_addr().to_string()
    }
    fn hold_reads(&self, hold: bool) {
        self.0.hold_reads(hold);
    }
    fn queued(&self) -> usize {
        self.0.queued_bytes()
    }
    fn on_drain(&self, f: Box<dyn FnOnce() + Send>) {
        self.0.on_drain(f);
    }
    fn set_max_queued(&self, n: usize) {
        self.0.set_max_queued_bytes(Some(n));
    }
}

#[cfg(test)]
mod tests;
