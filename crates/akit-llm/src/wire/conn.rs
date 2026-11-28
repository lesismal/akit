//! fit's links (`llm/internal/wire/conn.go`): a [`Session`] over a WebSocket, akio-ws's, after a
//! handshake of fit's that follows the upgrade:
//!
//! 1. the listener sends a random challenge of 32 bytes;
//! 2. the dialer answers with its proof: a token's (`FTK1`, the token's ID, a nonce, the proof),
//!    or a password's HMAC of the challenge followed by a nonce of its own;
//! 3. the listener checks it against its tokens or each of its passwords, and acknowledges with
//!    `0x01` followed by its own proof (of the token, or of the password over the challenge and
//!    the nonce), which the dialer checks; or closes the link with 4001.
//!
//! A link is paced by its send queue (fit's `Conn.send`): once more than 64 KiB wait for the
//! peer, its streams stop sending data and its reads are held, for a quarter of the shorter of
//! the ping interval and the slow timeout at most, until the queue drains (akio-ws's `on_drain`).
//! Calls, answers, pushes and control frames are not held: the calls the peer may have running
//! and the reads held bound them. The links are relayed connections (`set_relayed`): the engine's
//! own pause past its write high water, which only sends on the connection's loop re-check, would
//! hold a link that is fed from other links' threads; the link's own pacing replaces it.
//!
//! A heartbeat per link pings after an interval without frames, or after frames sent, and drops a
//! link silent for three intervals or whose ping waited the slow timeout for its pong.

use std::collections::HashMap;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use akio::timer::{SendTimer, after_send};
use akio::{ConnHandle, Engine};
use akio_http::{Context, WsOptions};
use akio_ws::client::{DialOptions, WsClientError};
use akio_ws::{MessageRef, WsConfig, WsHandler, WsSender};

use super::session::{Handler, Options, Session, now_nanos};
use super::{
    CHALLENGE_SIZE, Error, NONCE_SIZE, PROOF_SIZE, parse_token_answer, password_listener_proof,
    password_proof, token_answer, token_transcript,
};

/// The interval of the heartbeat, by default.
pub const DEFAULT_PING_INTERVAL: Duration = Duration::from_secs(10);
/// How long a ping may wait for its pong, by default.
pub const DEFAULT_SLOW_TIMEOUT: Duration = Duration::from_secs(30);
/// The handshake's bound, from the upgrade to the proof.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// The links a listener holds that have not authenticated, by default.
pub const DEFAULT_MAX_PENDING: usize = 256;
/// Those of one IP address, by default.
pub const DEFAULT_MAX_PENDING_PER_IP: usize = 16;
/// The only path a listener upgrades.
pub const PATH: &str = "/ws";
/// The bytes waiting for the peer past which a link holds what feeds it (fib's
/// `PendingWriteLimit`, fit's).
pub const HIGH_WATER: usize = 64 << 10;

const ACK_OK: u8 = 0x01;
const CLOSE_NORMAL: u16 = 1000;
const CLOSE_GOING_AWAY: u16 = 1001;
const CLOSE_PROTOCOL_ERROR: u16 = 1002;
const CLOSE_INTERNAL_ERROR: u16 = 1011;
/// The listener refused the proof.
pub const CLOSE_AUTH_FAILED: u16 = 4001;
/// The listener is full or going away.
pub const CLOSE_REFUSED: u16 = 4003;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    if akio_crypto::rand::fill_random(&mut b).is_err() {
        // Never a constant: the clock's nanoseconds, as a last resort.
        let t = now_nanos().to_le_bytes();
        for (i, x) in b.iter_mut().enumerate() {
            *x = t.get(i % 8).copied().unwrap_or(0) ^ u8::try_from(i % 256).unwrap_or(0);
        }
    }
    b
}

/// The link tokens a listener issued: the stored key of one by its ID, or why it is refused.
pub trait Tokens: Send + Sync {
    /// Token `id`'s stored key.
    ///
    /// # Errors
    ///
    /// Why the token is refused (unknown, expired).
    fn lookup(&self, id: &str) -> Result<[u8; 32], String>;
}

impl Tokens for akit_roles::linktoken::Registry {
    fn lookup(&self, id: &str) -> Result<[u8; 32], String> {
        akit_roles::linktoken::Registry::lookup(self, id).map_err(|e| e.to_string())
    }
}

/// One password a listener takes, and the roles a dialer that proves it may say hello as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    /// The password, not empty.
    pub password: String,
    /// Its roles.
    pub roles: Vec<String>,
}

/// The heartbeat's and the pacing's bounds of a link.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// The heartbeat's interval; zero: none.
    pub ping: Duration,
    /// How long a ping may wait for its pong; zero: no bound.
    pub slow: Duration,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing {
            ping: DEFAULT_PING_INTERVAL,
            slow: DEFAULT_SLOW_TIMEOUT,
        }
    }
}

impl Timing {
    /// How long a link above its high water holds its reads: a quarter of the shorter of the
    /// ping interval and the slow timeout (fit's `maxHold`).
    #[must_use]
    pub fn max_hold(&self) -> Duration {
        let mut d = self.ping;
        if !self.slow.is_zero() && (d.is_zero() || self.slow < d) {
            d = self.slow;
        }
        if d.is_zero() {
            d = DEFAULT_PING_INTERVAL;
        }
        d.checked_div(4).unwrap_or_default()
    }
}

type MakeHandler = Arc<dyn Fn(&Arc<Conn>) -> Arc<dyn Handler> + Send + Sync>;
type OnConn = Arc<dyn Fn(&Arc<Conn>) + Send + Sync>;
type OnClosed = Arc<dyn Fn(&Arc<Conn>, &Error) + Send + Sync>;
type OnAuthFailed = Arc<dyn Fn(&str) + Send + Sync>;
/// Which peers a listener takes, by their address.
pub type PeerFilter = Arc<dyn Fn(&SocketAddr) -> bool + Send + Sync>;

struct Flow {
    above: bool,
    epoch: u64,
    hold: Option<SendTimer>,
}

/// An authenticated link carrying a [`Session`].
pub struct Conn {
    session: Session,
    ws: WsSender,
    handle: Option<ConnHandle>,
    remote: String,
    roles: Vec<String>,
    token: String,
    timing: Timing,
    last_beat: AtomicU64,
    flow: Mutex<Flow>,
    closed: AtomicBool,
    on_close: Option<OnClosed>,
}

impl fmt::Debug for Conn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Conn")
            .field("remote", &self.remote)
            .field("dialer", &self.session.options().dialer)
            .finish_non_exhaustive()
    }
}

impl Conn {
    #[allow(clippy::too_many_arguments)]
    fn new(
        ws: WsSender,
        handle: Option<ConnHandle>,
        opts: Options,
        timing: Timing,
        remote: String,
        roles: Vec<String>,
        token: String,
        make: &MakeHandler,
        on_close: Option<OnClosed>,
    ) -> Arc<Conn> {
        ws.set_relayed(true);
        let c = Arc::new_cyclic(|me: &Weak<Conn>| {
            let me = me.clone();
            let session = Session::new(opts, move |b| match me.upgrade() {
                Some(c) => c.send(&b),
                None => Err("the link is gone".to_owned()),
            });
            Conn {
                session,
                ws,
                handle,
                remote,
                roles,
                token,
                timing,
                last_beat: AtomicU64::new(now_nanos()),
                flow: Mutex::new(Flow {
                    above: false,
                    epoch: 0,
                    hold: None,
                }),
                closed: AtomicBool::new(false),
                on_close,
            }
        });
        c.session.set_handler(make(&c));
        c
    }

    /// The link's session.
    #[must_use]
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// The peer's address as the link opened.
    #[must_use]
    pub fn remote(&self) -> &str {
        &self.remote
    }

    /// The roles of the password the dialer proved, or the token's (listener's side).
    #[must_use]
    pub fn roles(&self) -> &[String] {
        &self.roles
    }

    /// The ID of the token the dialer proved (listener's side), empty for a password.
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Writes a frame without waiting, and holds what feeds the link once more than
    /// [`HIGH_WATER`] waits for the peer.
    fn send(self: &Arc<Self>, b: &[u8]) -> Result<(), String> {
        self.ws.send_binary(b).map_err(|e| e.to_string())?;
        if self.ws.queued_bytes() > HIGH_WATER {
            self.over_limit();
        }
        Ok(())
    }

    fn over_limit(self: &Arc<Self>) {
        let epoch = {
            let mut f = lock(&self.flow);
            if f.above {
                return;
            }
            f.above = true;
            f.epoch = f.epoch.wrapping_add(1);
            let epoch = f.epoch;
            self.session.hold_streams_quiet();
            self.ws.hold_reads(true);
            let me = Arc::downgrade(self);
            f.hold = after_send(self.timing.max_hold(), move || {
                if let Some(c) = me.upgrade() {
                    c.release_reads(epoch);
                }
            })
            .ok();
            epoch
        };
        let me = Arc::downgrade(self);
        self.ws.on_drain(move || {
            if let Some(c) = me.upgrade() {
                c.writable(epoch);
            }
        });
    }

    fn release_reads(&self, epoch: u64) {
        let f = lock(&self.flow);
        if f.above && f.epoch == epoch {
            self.ws.hold_reads(false);
        }
    }

    fn writable(&self, epoch: u64) {
        {
            let mut f = lock(&self.flow);
            if !f.above || f.epoch != epoch {
                return;
            }
            f.above = false;
            if let Some(t) = f.hold.take() {
                t.cancel();
            }
            self.ws.hold_reads(false);
        }
        // Outside the lock: a sender woken may pass the high water again at once.
        self.session.hold_streams(false);
    }

    /// Closes the link and its session.
    pub fn close(&self) {
        self.session.close(Error::Closed);
        self.ws.close(CLOSE_NORMAL, "");
    }

    /// Closes a link whose peer is dead or too slow at once: what waits for the peer is dropped
    /// rather than sent before a close frame (fit's `forceClose`, `Abort`; akio's A238).
    fn force_close(&self, err: Error) {
        self.session.close(err);
        match &self.handle {
            Some(h) => h.close(),
            None => self.ws.abort(),
        }
    }

    fn receive(&self, data: &[u8]) {
        if self.session.receive(data).is_err() {
            self.ws.close(CLOSE_PROTOCOL_ERROR, "protocol error");
        }
    }

    fn start(self: &Arc<Self>) {
        let d = self.timing.ping;
        if d.is_zero() {
            return;
        }
        self.last_beat.store(now_nanos(), Ordering::Relaxed);
        self.beat_after(d);
    }

    fn beat_after(self: &Arc<Self>, d: Duration) {
        let me = Arc::downgrade(self);
        let _ = after_send(d, move || {
            if let Some(c) = me.upgrade() {
                c.beat(d);
            }
        });
    }

    /// Every ping interval: a ping goes out when nothing came for an interval, when something was
    /// sent since the last beat, or when the last round trip is two intervals old, unless one
    /// waits for its pong; three intervals without a frame, or a ping unanswered for the slow
    /// timeout, drop the link.
    fn beat(self: &Arc<Self>, d: Duration) {
        if self.session.is_closed() {
            return;
        }
        let idle = self.session.since_recv();
        if idle >= d.saturating_mul(3) {
            return self.force_close(Error::Heartbeat);
        }
        let waited = self.session.unanswered();
        if !self.timing.slow.is_zero() && waited >= self.timing.slow {
            return self.force_close(Error::Slow);
        }
        let last = self.last_beat.load(Ordering::Relaxed);
        let rtt_old = Duration::from_nanos(now_nanos().saturating_sub(self.session.rtt_at()))
            >= d.saturating_mul(2);
        if waited.is_zero() && (idle >= d || self.session.last_send() > last || rtt_old) {
            let _ = self.session.ping();
        }
        self.last_beat.store(now_nanos(), Ordering::Relaxed);
        self.beat_after(d);
    }

    /// The WebSocket ended: once.
    fn closed(self: &Arc<Self>, err: Error) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(t) = lock(&self.flow).hold.take() {
            t.cancel();
        }
        self.session.close(err);
        if let Some(f) = &self.on_close {
            let e = self.session.err().unwrap_or(Error::Closed);
            f(self, &e);
        }
    }
}

fn close_error(code: Option<u16>, reason: &str) -> Error {
    match code {
        Some(CLOSE_NORMAL) | None => Error::Closed,
        Some(c) if reason.is_empty() => Error::Other(format!("websocket: close {c}")),
        Some(c) => Error::Other(format!("websocket: close {c}: {reason}")),
    }
}

fn ws_config(max_frame: usize) -> WsConfig {
    let mut c = WsConfig::default();
    c.max_message_bytes = max_frame;
    // The frames compress themselves; fib's peers offer no extension.
    c.permessage_deflate = false;
    c
}

// ---- Listener ----

/// A listener's settings.
#[derive(Clone)]
pub struct ListenerConfig {
    /// Every link's session (its `dialer` is ignored).
    pub session: Options,
    /// The heartbeat.
    pub timing: Timing,
    /// The passwords it takes, distinct and not empty.
    pub credentials: Vec<Credential>,
    /// The tokens it issued.
    pub tokens: Option<Arc<dyn Tokens>>,
    /// The roles of a link that proved a token.
    pub token_roles: Vec<String>,
    /// The links it holds at once, handshakes included; 0: no bound.
    pub max_conns: usize,
    /// The links not authenticated it holds, from their TCP connect; 0: the default.
    pub max_pending: usize,
    /// Those of one IP address; 0: the default.
    pub max_pending_per_ip: usize,
    /// Vets each link before the handshake: an error refuses it (4003).
    pub accept: Option<Arc<dyn Fn() -> Result<(), String> + Send + Sync>>,
    /// The handler of a link authenticated.
    pub handlers: MakeHandler,
    /// A proof refused, with the peer's address.
    pub on_auth_failed: Option<OnAuthFailed>,
    /// A link authenticated.
    pub on_open: Option<OnConn>,
    /// An authenticated link closed.
    pub on_close: Option<OnClosed>,
}

impl fmt::Debug for ListenerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ListenerConfig")
            .field("max_conns", &self.max_conns)
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct Pending {
    closed: bool,
    by_conn: HashMap<u64, IpAddr>,
    by_ip: HashMap<IpAddr, usize>,
}

struct Shared {
    cfg: ListenerConfig,
    max_pending: usize,
    max_per_ip: usize,
    pending: Mutex<Pending>,
    count: AtomicUsize,
    open: Mutex<HashMap<u64, ConnHandle>>,
}

impl Shared {
    fn hold(&self, h: &ConnHandle) -> bool {
        let ip = h.peer_addr().ip().to_canonical();
        let mut p = lock(&self.pending);
        let of_ip = p.by_ip.get(&ip).copied().unwrap_or(0);
        if p.closed || p.by_conn.len() >= self.max_pending || of_ip >= self.max_per_ip {
            return false;
        }
        p.by_conn.insert(h.id(), ip);
        p.by_ip.insert(ip, of_ip.saturating_add(1));
        true
    }

    fn authenticated(&self, id: u64) {
        let mut p = lock(&self.pending);
        let Some(ip) = p.by_conn.remove(&id) else {
            return;
        };
        match p.by_ip.get(&ip).copied() {
            Some(n) if n > 1 => {
                p.by_ip.insert(ip, n.saturating_sub(1));
            }
            _ => {
                p.by_ip.remove(&ip);
            }
        }
    }
}

/// A listener of links: an HTTP server that upgrades [`PATH`] alone (404 for any other) on
/// whatever akio listener [`Listener::serve`] makes, authenticates each link and runs a session
/// on it.
#[derive(Clone)]
pub struct Listener(Arc<Shared>);

impl fmt::Debug for Listener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Listener").finish_non_exhaustive()
    }
}

impl Listener {
    /// A listener of `cfg`.
    #[must_use]
    pub fn new(cfg: ListenerConfig) -> Listener {
        let max_pending = if cfg.max_pending == 0 {
            DEFAULT_MAX_PENDING
        } else {
            cfg.max_pending
        };
        let max_per_ip = if cfg.max_pending_per_ip == 0 {
            DEFAULT_MAX_PENDING_PER_IP
        } else {
            cfg.max_pending_per_ip
        };
        Listener(Arc::new(Shared {
            cfg,
            max_pending,
            max_per_ip,
            pending: Mutex::new(Pending::default()),
            count: AtomicUsize::new(0),
            open: Mutex::new(HashMap::new()),
        }))
    }

    /// The akio listener on `addr`, over TLS with `tls`, taking the peers `filter` allows.
    #[must_use]
    pub fn serve(
        &self,
        addr: SocketAddr,
        tls: Option<akio::tls::ServerConfig>,
        filter: Option<PeerFilter>,
    ) -> akio::Listener {
        let mut http = akio_http::HttpConfig::default();
        http.h2 = None;
        http.max_header_bytes = 16 << 10;
        http.max_body_bytes = 0;
        http.head_timeout = Some(HANDSHAKE_TIMEOUT);
        let service = akio_http::serve_with(http, Service(Arc::clone(&self.0)));
        let mut l = akit_runtime::scheduled(akio::Listener::tcp(addr)).threads(1);
        if let Some(f) = filter {
            l = l.filter(move |a: &SocketAddr| f(a));
        }
        let (a, b) = (Arc::clone(&self.0), Arc::clone(&self.0));
        let l = l
            .on_accept(move |h| {
                // Counted from the TCP connect, TLS and the upgrade included, to the proof.
                if !a.hold(h) {
                    h.close();
                    return;
                }
                lock(&a.open).insert(h.id(), h.clone());
                let _ = h.close_after(HANDSHAKE_TIMEOUT.saturating_mul(2));
            })
            .on_close(move |h| {
                b.authenticated(h.id());
                lock(&b.open).remove(&h.id());
            });
        match tls {
            Some(cfg) => l.serve(akio::tls::Tls::server(cfg, service)),
            None => l.serve(service),
        }
    }

    /// Closes every link the listener took, authenticated or not, and refuses every one it takes
    /// from now on: for once its akio listener is removed, which leaves its connections open.
    pub fn close_all(&self) {
        lock(&self.0.pending).closed = true;
        let open: Vec<ConnHandle> = lock(&self.0.open).values().cloned().collect();
        for h in open {
            h.close();
        }
    }

    /// The links it holds, handshakes included.
    #[must_use]
    pub fn count(&self) -> usize {
        self.0.count.load(Ordering::Acquire)
    }
}

struct Service(Arc<Shared>);

impl akio_http::Service for Service {
    fn call(&self, ctx: &mut Context) {
        if ctx.path() != PATH {
            ctx.text(404, "404 page not found\n");
            return;
        }
        let s = &self.0;
        let mut refuse = s.cfg.accept.as_ref().and_then(|a| a().err());
        if refuse.is_none()
            && s.cfg.max_conns > 0
            && s.count.load(Ordering::Acquire) >= s.cfg.max_conns
        {
            refuse = Some("too many connections".to_owned());
        }
        let handle = ctx.connection();
        let mut opts = WsOptions::default();
        opts.config = ws_config(s.cfg.session.max_frame);
        ctx.upgrade_websocket(
            ServerWs {
                s: Arc::clone(s),
                handle,
                refuse,
                counted: false,
                challenge: [0; CHALLENGE_SIZE],
                authed: Arc::new(AtomicBool::new(false)),
                conn: None,
            },
            &opts,
        );
    }

    fn streams_bodies(&self) -> bool {
        false
    }
}

struct ServerWs {
    s: Arc<Shared>,
    handle: Option<ConnHandle>,
    refuse: Option<String>,
    counted: bool,
    challenge: [u8; CHALLENGE_SIZE],
    authed: Arc<AtomicBool>,
    conn: Option<Arc<Conn>>,
}

impl ServerWs {
    fn remote(&self, ws: &WsSender) -> String {
        self.handle
            .as_ref()
            .map(ConnHandle::peer_addr)
            .or_else(|| ws.peer_addr())
            .map_or_else(String::new, |a| a.to_string())
    }

    /// Checks the dialer's answer: the roles it may take, the token it proved, and the
    /// acknowledgement.
    fn verify(&self, data: &[u8]) -> Result<(Vec<String>, String, Vec<u8>), String> {
        let cfg = &self.s.cfg;
        let mut ack = vec![ACK_OK];
        if let Some((id, nonce, proof)) = parse_token_answer(data) {
            let Some(tokens) = &cfg.tokens else {
                return Err("authentication failed".to_owned());
            };
            let stored = tokens.lookup(&id)?;
            let t = token_transcript(&id, nonce, &self.challenge);
            if !akit_roles::linktoken::verify(&stored, &t, proof) {
                return Err("authentication failed".to_owned());
            }
            ack.extend_from_slice(&akit_roles::linktoken::listener_proof(&stored, &t, &[]));
            return Ok((cfg.token_roles.clone(), id, ack));
        }
        // A dialer of fit's version sends a nonce after its proof, for the listener to prove
        // the password back.
        let (proof, nonce) = if data.len() == PROOF_SIZE.saturating_add(NONCE_SIZE) {
            data.split_at(PROOF_SIZE)
        } else {
            (data, &[][..])
        };
        // Every credential is checked, not stopping at the first that matches.
        let mut found: Option<&Credential> = None;
        for c in &cfg.credentials {
            if !c.password.is_empty()
                && akio_crypto::ct_eq(proof, &password_proof(&c.password, &self.challenge))
                && found.is_none()
            {
                found = Some(c);
            }
        }
        let Some(c) = found else {
            return Err("authentication failed".to_owned());
        };
        if !nonce.is_empty() {
            ack.extend_from_slice(&password_listener_proof(
                &c.password,
                &self.challenge,
                nonce,
            ));
        }
        Ok((c.roles.clone(), String::new(), ack))
    }
}

impl WsHandler for ServerWs {
    fn on_open(&mut self, ws: &WsSender) {
        if let Some(r) = self.refuse.take() {
            ws.close(CLOSE_REFUSED, &r);
            return;
        }
        self.counted = true;
        self.s.count.fetch_add(1, Ordering::AcqRel);
        self.challenge = random();
        let (authed, w) = (Arc::clone(&self.authed), ws.clone());
        let _ = after_send(HANDSHAKE_TIMEOUT, move || {
            if !authed.load(Ordering::Acquire) {
                w.close(CLOSE_AUTH_FAILED, "handshake timeout");
            }
        });
        if ws.send_binary(&self.challenge).is_err() {
            ws.close(CLOSE_INTERNAL_ERROR, "");
        }
    }

    fn on_message_ref(&mut self, ws: &WsSender, msg: MessageRef<'_>) {
        if let Some(c) = &self.conn {
            c.receive(msg.as_bytes());
            return;
        }
        if self.authed.load(Ordering::Acquire) || !self.counted {
            return;
        }
        let (roles, token, ack) = match self.verify(msg.as_bytes()) {
            Ok(v) => v,
            Err(reason) => {
                let remote = self.remote(ws);
                if let Some(f) = &self.s.cfg.on_auth_failed {
                    f(&remote);
                }
                ws.close(CLOSE_AUTH_FAILED, &reason);
                return;
            }
        };
        self.authed.store(true, Ordering::Release);
        if let Some(h) = &self.handle {
            self.s.authenticated(h.id());
            h.cancel_close();
        }
        let cfg = &self.s.cfg;
        let c = Conn::new(
            ws.clone(),
            self.handle.clone(),
            Options {
                dialer: false,
                ..cfg.session
            },
            cfg.timing,
            self.remote(ws),
            roles,
            token,
            &cfg.handlers,
            cfg.on_close.clone(),
        );
        self.conn = Some(Arc::clone(&c));
        // The session exists before the ack goes, so whatever the dialer sends after finds it.
        if ws.send_binary(&ack).is_err() {
            ws.close(CLOSE_INTERNAL_ERROR, "");
            return;
        }
        c.start();
        if let Some(f) = &cfg.on_open {
            f(&c);
        }
    }

    fn on_close(&mut self, code: Option<u16>, reason: &str) {
        if self.counted {
            self.counted = false;
            self.s.count.fetch_sub(1, Ordering::AcqRel);
        }
        if let Some(c) = self.conn.take() {
            c.closed(close_error(code, reason));
        }
    }
}

// ---- Dialer ----

/// How to dial a link.
#[derive(Clone)]
pub struct DialConfig {
    /// The link's session (its `dialer` is set).
    pub session: Options,
    /// The heartbeat.
    pub timing: Timing,
    /// The token proved, in place of the password.
    pub token: Option<akit_roles::linktoken::Secret>,
    /// The password proved; proved both ways when not empty.
    pub password: String,
    /// The TLS a `wss://` URL is dialed with: it must check the listener's certificate.
    pub tls: Option<akio::tls::ClientConfig>,
    /// The name the certificate is checked for, and the SNI; the URL's host when `None`.
    pub server_name: Option<String>,
    /// The bound of the whole dial, handshakes included.
    pub timeout: Duration,
    /// The handler of the link.
    pub handlers: MakeHandler,
    /// The link closed, once it was made.
    pub on_close: Option<OnClosed>,
}

impl fmt::Debug for DialConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DialConfig")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

type DialDone = Box<dyn FnOnce(Result<Arc<Conn>, Error>) + Send>;

struct DialShared {
    done: Mutex<Option<DialDone>>,
    ws: Mutex<Option<WsSender>>,
}

impl DialShared {
    /// Hands `r` to the dial's caller, unless it has its answer; false then.
    fn settle(&self, r: Result<Arc<Conn>, Error>) -> bool {
        let done = lock(&self.done).take();
        match done {
            Some(d) => {
                d(r);
                true
            }
            None => {
                if let Ok(c) = r {
                    c.close(); // the caller gave up
                }
                false
            }
        }
    }
}

/// A dial under way: [`DialHandle::abort`] gives it up.
#[derive(Clone)]
pub struct DialHandle(Arc<DialShared>);

impl fmt::Debug for DialHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DialHandle").finish_non_exhaustive()
    }
}

impl DialHandle {
    /// Gives the dial up: its caller is told [`Error::Closed`], and a link made from now on is
    /// closed.
    pub fn abort(&self) {
        if self.0.settle(Err(Error::Closed)) {
            if let Some(w) = lock(&self.0.ws).take() {
                w.close(CLOSE_GOING_AWAY, "");
            }
        }
    }
}

enum Phase {
    Challenge,
    Proved { challenge: Vec<u8>, nonce: Vec<u8> },
    Authed(Arc<Conn>),
    Done,
}

struct ClientWs {
    sh: Arc<DialShared>,
    cfg: DialConfig,
    remote: String,
    phase: Phase,
}

impl ClientWs {
    fn fail(&mut self, ws: &WsSender, e: Error, code: u16) {
        self.phase = Phase::Done;
        self.sh.settle(Err(e));
        ws.close(code, "");
    }
}

impl WsHandler for ClientWs {
    fn on_open(&mut self, ws: &WsSender) {
        ws.set_relayed(true);
        if lock(&self.sh.done).is_none() {
            ws.close(CLOSE_GOING_AWAY, "");
            return;
        }
        *lock(&self.sh.ws) = Some(ws.clone());
    }

    fn on_message_ref(&mut self, ws: &WsSender, msg: MessageRef<'_>) {
        let data = msg.as_bytes();
        match &self.phase {
            Phase::Authed(c) => c.receive(data),
            Phase::Done => {}
            Phase::Challenge => {
                if data.len() != CHALLENGE_SIZE {
                    // A dialer answers only a challenge: anything else might be a listener's
                    // proof to be had from it.
                    let e = Error::Protocol(format!("a challenge of {} bytes", data.len()));
                    return self.fail(ws, e, CLOSE_GOING_AWAY);
                }
                let (proof, nonce) = if let Some(sec) = &self.cfg.token {
                    match token_answer(sec, data) {
                        Ok(v) => v,
                        Err(e) => {
                            return self.fail(ws, Error::Other(e.to_string()), CLOSE_GOING_AWAY);
                        }
                    }
                } else if self.cfg.password.is_empty() {
                    (Vec::new(), Vec::new())
                } else {
                    let nonce: [u8; NONCE_SIZE] = random();
                    let mut p = password_proof(&self.cfg.password, data).to_vec();
                    p.extend_from_slice(&nonce);
                    (p, nonce.to_vec())
                };
                if ws.send_binary(&proof).is_err() {
                    return self.fail(ws, Error::Closed, CLOSE_GOING_AWAY);
                }
                self.phase = Phase::Proved {
                    challenge: data.to_vec(),
                    nonce,
                };
            }
            Phase::Proved { challenge, nonce } => {
                let mutual = self.cfg.token.is_none() && !self.cfg.password.is_empty();
                if data.first() != Some(&ACK_OK)
                    || (!mutual && self.cfg.token.is_none() && data.len() != 1)
                {
                    let e = Error::Protocol("bad handshake ack".to_owned());
                    return self.fail(ws, e, CLOSE_PROTOCOL_ERROR);
                }
                let rest = data.get(1..).unwrap_or(&[]);
                if let Some(sec) = &self.cfg.token {
                    let t = token_transcript(sec.id(), nonce, challenge);
                    if !sec.verify_listener(&t, rest, &[]) {
                        let e = Error::Auth(
                            "wire: the listener did not prove it issued the token".to_owned(),
                        );
                        return self.fail(ws, e, CLOSE_AUTH_FAILED);
                    }
                } else if mutual
                    && !akio_crypto::ct_eq(
                        rest,
                        &password_listener_proof(&self.cfg.password, challenge, nonce),
                    )
                {
                    // The listener does not know the password.
                    let e = Error::Auth("the listener did not prove the password".to_owned());
                    return self.fail(ws, e, CLOSE_AUTH_FAILED);
                }
                let c = Conn::new(
                    ws.clone(),
                    None,
                    Options {
                        dialer: true,
                        ..self.cfg.session
                    },
                    self.cfg.timing,
                    self.remote.clone(),
                    Vec::new(),
                    String::new(),
                    &self.cfg.handlers,
                    self.cfg.on_close.clone(),
                );
                self.phase = Phase::Authed(Arc::clone(&c));
                c.start();
                self.sh.settle(Ok(c));
            }
        }
    }

    fn on_close(&mut self, code: Option<u16>, reason: &str) {
        match std::mem::replace(&mut self.phase, Phase::Done) {
            Phase::Authed(c) => c.closed(close_error(code, reason)),
            Phase::Done => {}
            _ => {
                let e = match code {
                    Some(CLOSE_AUTH_FAILED) => Error::Auth(reason.to_owned()),
                    Some(CLOSE_REFUSED) => Error::Refused(reason.to_owned()),
                    c => Error::Other(format!(
                        "wire: connection closed during handshake (code {})",
                        c.unwrap_or(1006)
                    )),
                };
                self.sh.settle(Err(e));
            }
        }
    }
}

/// Dials `url` (`ws://` or `wss://`, which needs `cfg.tls`) on `engine` and runs the handshake;
/// `done` is told the link, or why there is none, once, on an I/O, resolver or timer thread.
pub fn dial(
    engine: &Arc<Engine>,
    url: &str,
    cfg: DialConfig,
    done: impl FnOnce(Result<Arc<Conn>, Error>) + Send + 'static,
) -> DialHandle {
    let sh = Arc::new(DialShared {
        done: Mutex::new(Some(Box::new(done))),
        ws: Mutex::new(None),
    });
    let handle = DialHandle(Arc::clone(&sh));
    if url.starts_with("wss://") && cfg.tls.is_none() {
        sh.settle(Err(Error::Other(
            "wire: a wss:// URL needs a TLS configuration that checks the listener's certificate"
                .to_owned(),
        )));
        return handle;
    }
    let mut opts = DialOptions::default();
    opts.config = ws_config(cfg.session.max_frame);
    opts.tls = cfg.tls.clone();
    opts.server_name = cfg.server_name.clone();
    opts.handshake_timeout = cfg.timeout;
    let remote = url
        .split_once("://")
        .map_or(url, |(_, r)| r)
        .split('/')
        .next()
        .unwrap_or("")
        .to_owned();
    let deadline = cfg.timeout;
    let h = ClientWs {
        sh: Arc::clone(&sh),
        cfg,
        remote,
        phase: Phase::Challenge,
    };
    let s2 = Arc::clone(&sh);
    akio_ws::client::dial(engine, url, &opts, h, move |e: WsClientError| {
        s2.settle(Err(Error::Other(format!("websocket dial: {e}"))));
    });
    let t = handle.clone();
    let started = Instant::now();
    let _ = after_send(deadline, move || {
        if t.0.settle(Err(Error::Timeout)) {
            akit_observe::debug!(Llm, "wire: a dial timed out after {:?}", started.elapsed());
            if let Some(w) = lock(&t.0.ws).take() {
                w.close(CLOSE_GOING_AWAY, "");
            }
        }
    });
    handle
}

#[cfg(test)]
pub(crate) mod tests;
