//! The hub's console socket (I103b, fit's `tunnel/hub/socket.go`): the console's encrypted
//! WebSocket, which carries the hub's pages to the web proxies of proxies that cannot reach them
//! otherwise. Each WebSocket, once the hellos are exchanged ([`crate::wscrypt`]), gets a TCP
//! connection of its own to the hub's console, and the two pass on what they receive: the
//! WebSocket's messages opened, the console's bytes sealed, one message per piece of at most
//! [`SEAL_CHUNK`].
//!
//! Bounds, as fit's: `max_conns` WebSockets at once; of those, at most a quarter from one IP
//! address that have not proven the password, by a first message that opens, and each closed if it
//! has not within [`AUTH_TIMEOUT`]. What arrives before the console is connected waits, the
//! WebSocket's reads held, at most [`MAX_PENDING`] bytes of it. Each side's reads wait while the
//! other's backlog is past [`crate::pace::WATERMARK`] (rules 8 and 9).
//!
//! A WebSocket refused or failed is ended at once, what is queued dropped, as fit aborts it
//! (`WsSender::abort`, akio's A238); one whose console connection closed gets a close frame behind
//! what is queued, as fit's.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use akio_http::ws::{MessageRef, WsConfig, WsHandler, WsSender};

use super::{HubCore, lock};
use crate::pace::Gate;
use crate::wscrypt::{Secret, Session};

/// The most of the console's bytes one message carries.
pub const SEAL_CHUNK: usize = 64 << 10;
/// The longest message the socket takes: a sealed chunk, with room to spare.
pub const MAX_MESSAGE: usize = 2 * SEAL_CHUNK;
/// How long the connect to the console may take.
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a WebSocket may take to prove it knows the password, with a first message that opens,
/// before it is closed: until then anyone who reaches the address could hold its place.
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
/// The most bytes that wait for the console to be connected; past them the WebSocket closes.
pub const MAX_PENDING: usize = 1 << 20;

/// The most WebSockets from one IP address that may be open at once and not have proven the
/// password: a share of `max_conns`, so that one address cannot hold every place.
#[must_use]
pub fn unauth_per_ip(conns: usize) -> usize {
    (conns / 4).max(1)
}

/// Where the console is dialed: its address, an unspecified host the loopback; `None` for an
/// address without a port.
#[must_use]
pub fn console_addr(addr: &str) -> Option<(String, u16)> {
    let (host, port) = addr.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if bare.is_empty() || bare.parse::<IpAddr>().is_ok_and(|ip| ip.is_unspecified()) {
        return Some(("127.0.0.1".to_owned(), port));
    }
    Some((bare.to_owned(), port))
}

/// The console socket's state, shared by its WebSockets.
pub(crate) struct SocketState {
    hub: Weak<HubCore>,
    max: AtomicUsize,
    secret: Mutex<Option<(String, Secret)>>,
    /// How long a WebSocket has to prove the password: [`AUTH_TIMEOUT`], shorter in tests.
    pub(crate) auth_timeout: Mutex<Duration>,
    pub(crate) inner: Mutex<Inner>,
}

#[derive(Default)]
pub(crate) struct Inner {
    /// The open WebSockets, which bound their number and are closed as the socket stops.
    pub(crate) conns: HashMap<u64, Arc<SockConn>>,
    closed: bool,
    next: u64,
    /// By IP address, the WebSockets that have not proven the password.
    pub(crate) unauth: HashMap<IpAddr, usize>,
}

/// The console socket's listener.
pub(crate) struct Socket {
    pub(crate) address: String,
    pub(crate) id: akio::ListenerId,
    pub(crate) state: Arc<SocketState>,
}

/// One WebSocket and its connection to the console.
pub(crate) struct SockConn {
    id: u64,
    state: Weak<SocketState>,
    ws: WsSender,
    peer: SocketAddr,
    ip: IpAddr,
    hello: [u8; crate::wscrypt::HELLO_LEN],
    secret: Secret,
    ws_gate: Arc<Gate>,
    /// Counted among its address's WebSockets that have not proven the password; changed under
    /// the socket's lock.
    counted: AtomicBool,
    st: Mutex<ConnSt>,
}

#[derive(Default)]
struct ConnSt {
    sess: Option<Session>,
    up: Option<akio::Conn<SockUp>>,
    up_gate: Option<Arc<Gate>>,
    dialing: bool,
    pending: Vec<Vec<u8>>,
    pending_bytes: usize,
    closed: bool,
}

impl SocketState {
    fn new(hub: Weak<HubCore>, max: usize) -> Arc<SocketState> {
        Arc::new(SocketState {
            hub,
            max: AtomicUsize::new(max),
            secret: Mutex::new(None),
            auth_timeout: Mutex::new(AUTH_TIMEOUT),
            inner: Mutex::new(Inner::default()),
        })
    }

    /// Moves the socket to `password` (derived again only when it changes) and `max`.
    fn set(&self, password: &str, max: usize) {
        self.max.store(max, Ordering::Relaxed);
        let mut s = lock(&self.secret);
        if s.as_ref().is_none_or(|(p, _)| p != password) {
            *s = Some((password.to_owned(), Secret::new(password)));
        }
    }

    /// A WebSocket that opened: kept, its hello sent, unless the bounds refuse it.
    fn open(self: &Arc<Self>, ws: &WsSender) -> Option<Arc<SockConn>> {
        let peer = ws.peer_addr()?;
        let ip = peer.ip().to_canonical();
        let secret = lock(&self.secret).as_ref().map(|(_, s)| s.clone())?;
        let max = self.max.load(Ordering::Relaxed);
        let c = {
            let mut inner = lock(&self.inner);
            if inner.closed
                || inner.conns.len() >= max
                || inner.unauth.get(&ip).copied().unwrap_or(0) >= unauth_per_ip(max)
            {
                return None;
            }
            ws.set_relayed(true);
            inner.next = inner.next.wrapping_add(1);
            let c = Arc::new(SockConn {
                id: inner.next,
                state: Arc::downgrade(self),
                ws: ws.clone(),
                peer,
                ip,
                hello: crate::wscrypt::hello(),
                secret,
                ws_gate: Gate::new(Box::new(ws.clone())),
                counted: AtomicBool::new(true),
                st: Mutex::new(ConnSt::default()),
            });
            inner.conns.insert(c.id, Arc::clone(&c));
            let n = inner.unauth.entry(ip).or_insert(0);
            *n = n.saturating_add(1);
            c
        };
        let weak = Arc::downgrade(&c);
        let within = *lock(&self.auth_timeout);
        let _ = akio::timer::after_send(within, move || {
            if let Some(c) = weak.upgrade() {
                if c.counted.load(Ordering::Acquire) {
                    akit_observe::warn!(
                        Tunnel,
                        "tunnel.hub: console socket: {} did not prove the password in {}",
                        c.peer,
                        akit_config::duration::format(within)
                    );
                    c.abort();
                }
            }
        });
        let _ = ws.send_binary(&c.hello);
        Some(c)
    }

    /// Takes `c` off the WebSockets that have not proven the password.
    fn uncount(&self, c: &SockConn) {
        let mut inner = lock(&self.inner);
        if !c.counted.swap(false, Ordering::AcqRel) {
            return;
        }
        match inner.unauth.get_mut(&c.ip) {
            Some(n) if *n > 1 => *n = n.saturating_sub(1),
            _ => {
                inner.unauth.remove(&c.ip);
            }
        }
    }

    fn closed(&self, c: &Arc<SockConn>) {
        lock(&self.inner).conns.remove(&c.id);
        self.uncount(c);
        c.close();
    }

    /// Closes every WebSocket, as the socket stops.
    pub(crate) fn close_all(&self) {
        let conns: Vec<Arc<SockConn>> = {
            let mut inner = lock(&self.inner);
            inner.closed = true;
            inner.conns.values().cloned().collect()
        };
        for c in conns {
            c.abort();
        }
    }

    /// How many WebSockets are open.
    #[cfg(test)]
    pub(crate) fn conns(&self) -> usize {
        lock(&self.inner).conns.len()
    }
}

impl SockConn {
    fn abort(&self) {
        self.ws.abort();
    }

    /// The proxy's hello, and then what the browser sends, sealed.
    fn message(self: &Arc<Self>, data: &[u8]) {
        let mut st = lock(&self.st);
        if st.closed {
            return;
        }
        let Some(sess) = st.sess.as_mut() else {
            match Session::new(&self.secret, &self.hello, data, true) {
                Ok(s) => st.sess = Some(s),
                Err(_) => {
                    drop(st);
                    self.abort();
                }
            }
            return;
        };
        let plain = match sess.open(data) {
            Ok(p) => p,
            Err(_) => {
                drop(st);
                akit_observe::warn!(
                    Tunnel,
                    "tunnel.hub: console socket: a message from {} failed to decrypt; is the password right?",
                    self.peer
                );
                self.abort();
                return;
            }
        };
        // A message that opens proves the password.
        if let Some(s) = self.state.upgrade() {
            s.uncount(self);
        }
        if plain.is_empty() {
            // A proof alone, which a web proxy sends after its hello.
            return;
        }
        if let Some(up) = st.up.clone() {
            drop(st);
            if up.send(&plain).is_err() {
                self.abort();
                return;
            }
            self.ws_gate.pace(&up);
            return;
        }
        // The console is not connected yet: what arrives waits, and the WebSocket's reads with it.
        st.pending_bytes = st.pending_bytes.saturating_add(plain.len());
        if st.pending_bytes > MAX_PENDING {
            drop(st);
            self.abort();
            return;
        }
        st.pending.push(plain);
        self.ws_gate.hold();
        let dial = !std::mem::replace(&mut st.dialing, true);
        drop(st);
        if dial {
            self.dial();
        }
    }

    fn dial(self: &Arc<Self>) {
        let hub = self.state.upgrade().and_then(|s| s.hub.upgrade());
        let target = hub.as_ref().and_then(|h| {
            lock(&h.cfg)
                .console
                .as_ref()
                .and_then(|c| console_addr(&c.address))
        });
        let (Some(hub), Some((host, port))) = (hub, target) else {
            // Without the console, there is nothing to carry.
            self.abort();
            return;
        };
        let mut he = akio::dns::HappyEyeballs::default();
        he.connect_timeout = DIAL_TIMEOUT;
        let (me, me2) = (Arc::clone(self), Arc::clone(self));
        he.connect(
            &hub.host.engine,
            &host,
            port,
            akio::fn_acceptor(move |c: &akio::Conn<SockUp>| me.up_open(c)),
            move |e| {
                akit_observe::debug!(Tunnel, "tunnel.hub: console socket: the console: {e}");
                me2.abort();
            },
        );
    }

    /// The connection to the console opened: what waited goes to it, and the WebSocket is read
    /// again.
    fn up_open(self: &Arc<Self>, c: &akio::Conn<SockUp>) -> SockUp {
        // Paced by the WebSocket's gate, not by its own output (fit's SetWriteBackpressure(false)).
        c.set_relayed(true);
        let mut st = lock(&self.st);
        if st.closed {
            drop(st);
            c.close();
            return SockUp(None);
        }
        // Sent under the lock, so that nothing the WebSocket delivers meanwhile goes before it.
        for b in std::mem::take(&mut st.pending) {
            let _ = c.send(&b);
        }
        st.pending_bytes = 0;
        st.up = Some(c.clone());
        st.up_gate = Some(Gate::new(Box::new(c.clone())));
        drop(st);
        self.ws_gate.release();
        self.ws_gate.pace(c);
        SockUp(Some(Arc::clone(self)))
    }

    /// What the console sends, sealed, a message per piece.
    fn console_sent(&self, data: &[u8]) {
        let mut st = lock(&self.st);
        if st.closed {
            return;
        }
        let Some(sess) = st.sess.as_mut() else { return };
        // Sealed and sent under the lock, so that the messages go out in the order of their
        // numbers.
        for chunk in data.chunks(SEAL_CHUNK) {
            if self.ws.send_binary(&sess.seal(chunk)).is_err() {
                break;
            }
        }
        let gate = st.up_gate.clone();
        drop(st);
        if let Some(g) = gate {
            g.pace(&self.ws);
        }
    }

    fn close(&self) {
        let up = {
            let mut st = lock(&self.st);
            st.closed = true;
            st.pending = Vec::new();
            st.pending_bytes = 0;
            st.up.take()
        };
        if let Some(up) = up {
            up.close_after_send();
        }
    }
}

/// akio's handler of a WebSocket's connection to the console.
pub(crate) struct SockUp(Option<Arc<SockConn>>);

impl akio::Handler for SockUp {
    fn on_data(&mut self, _c: &akio::Conn<Self>, data: &[u8]) {
        if let Some(s) = &self.0 {
            s.console_sent(data);
        }
    }

    fn on_close(&mut self, _c: &akio::Conn<Self>, _err: Option<akio::Error>) {
        if let Some(s) = self.0.take() {
            s.ws.close(1000, "");
        }
    }
}

/// akio-ws's handler of the socket's WebSockets.
struct SockWs {
    state: Arc<SocketState>,
    conn: Option<Arc<SockConn>>,
}

impl WsHandler for SockWs {
    fn on_open(&mut self, ws: &WsSender) {
        match self.state.open(ws) {
            Some(c) => self.conn = Some(c),
            None => ws.abort(),
        }
    }

    fn on_message_ref(&mut self, _ws: &WsSender, msg: MessageRef<'_>) {
        if let Some(c) = &self.conn {
            c.message(msg.as_bytes());
        }
    }

    fn on_close(&mut self, _code: Option<u16>, _reason: &str) {
        if let Some(c) = self.conn.take() {
            self.state.closed(&c);
        }
    }
}

/// The WebSocket settings of the socket: [`MAX_MESSAGE`], no compression (sealed messages do not
/// compress, and fit's offer none).
fn ws_config() -> WsConfig {
    let mut c = WsConfig::default();
    c.max_message_bytes = MAX_MESSAGE;
    c.permessage_deflate = false;
    c
}

/// The socket's service: every path upgraded, as fit's (`ws.NewServer` with no path), any
/// `Origin`.
fn service(state: Arc<SocketState>) -> impl akio_http::Service {
    move |ctx: &mut akio_http::Context| {
        let h = SockWs {
            state: Arc::clone(&state),
            conn: None,
        };
        let mut o = akio_http::WsOptions::default();
        o.config = ws_config();
        ctx.upgrade_websocket(h, &o);
    }
}

impl HubCore {
    /// Moves the console socket to the hub's config (`None`: none): it listens anew only when its
    /// address changes, and closes the WebSockets it had then.
    pub(crate) fn apply_socket(
        self: &Arc<Self>,
        cfg: &akit_config::roles::TunnelHub,
    ) -> Result<(), String> {
        let engine = &self.host.engine;
        let want = cfg.console_socket.as_ref();
        let password = want
            .map(|s| {
                if s.password.is_empty() {
                    cfg.password.clone()
                } else {
                    s.password.clone()
                }
            })
            .unwrap_or_default();
        let mut have = lock(&self.socket);
        if let (Some(s), Some(w)) = (have.as_ref(), want) {
            if s.address == w.address {
                s.state.set(&password, w.conns());
                return Ok(());
            }
        }
        if let Some(s) = have.take() {
            let _ = engine.remove_listener(s.id);
            s.state.close_all();
        }
        let Some(w) = want else {
            return Ok(());
        };
        let Some(addr) = crate::proxy::listen_addr(&w.address) else {
            return Err(format!(
                "console_socket {}: not an address to listen on",
                w.address
            ));
        };
        let state = SocketState::new(self.me.clone(), w.conns());
        state.set(&password, w.conns());
        let mut l = akit_runtime::scheduled(akio::Listener::tcp(addr))
            .threads(1)
            .serve(akio_http::serve_with(
                crate::ws::http_config(),
                service(Arc::clone(&state)),
            ));
        if let Some(p) = lock(&self.access).clone() {
            l = l.filter(move |peer: &SocketAddr| p.allows(peer.ip()));
        }
        let id = engine
            .add_listener(l)
            .map_err(|e| format!("console_socket {}: {e}", w.address))?;
        let bound = engine
            .listeners()
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.local_addr);
        if let Some(a) = bound {
            akit_observe::info!(
                Tunnel,
                "tunnel.hub: the console over an encrypted WebSocket on ws://{a}/"
            );
        }
        *have = Some(Socket {
            address: w.address.clone(),
            id,
            state,
        });
        Ok(())
    }

    /// Where the console socket listens, or `None`.
    pub(crate) fn socket_addr(&self) -> Option<SocketAddr> {
        let id = lock(&self.socket).as_ref().map(|s| s.id)?;
        self.host
            .engine
            .listeners()
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.local_addr)
    }

    /// Stops the console socket, as the hub stops.
    pub(crate) fn stop_socket(&self) {
        if let Some(s) = lock(&self.socket).take() {
            let _ = self.host.engine.remove_listener(s.id);
            s.state.close_all();
        }
    }
}

#[cfg(test)]
mod tests;
