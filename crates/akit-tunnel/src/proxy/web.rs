//! A proxy's web proxies (I103b, fit's `tunnel/proxy/webproxy.go`): each serves a hub's console
//! pages on a local address, through the hub's console socket. Each connection to the address gets
//! a WebSocket of its own to the socket, which carries what the two exchange, sealed with the
//! socket's password ([`crate::wscrypt`]), one message per piece of at most [`SEAL_CHUNK`].
//!
//! The local connection is not read until the session is up; each side's reads then wait while
//! the other's backlog is past [`crate::pace::WATERMARK`] (rules 8 and 9). At most `max_conns`
//! connections at once per web proxy; the WebSocket's dial and handshake within
//! [`DIAL_TIMEOUT`]. A WebSocket that fails (a hello or a message that does not open) is ended at
//! once, what is queued dropped (`WsSender::abort`, akio's A238), as fit aborts it; one whose local
//! connection closed gets a close frame behind what is queued, as fit's. So is one a sealed message
//! could not be sent on (a send akio refuses, as `Overloaded`, A305): that message's number would
//! be skipped, and nothing after it would open.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use akio_http::ws::{MessageRef, WsConfig, WsHandler, WsSender};
use akit_config::roles::TunnelWebProxy;

use crate::pace::Gate;
use crate::wscrypt::{Secret, Session};

/// The most of the local connection's bytes one message carries.
pub const SEAL_CHUNK: usize = 64 << 10;
/// How long the dial of a WebSocket and its handshake may take.
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(10);
/// The most bytes read off a local connection before its session is up (one read, as its reads
/// are held from the start); past them it closes.
pub const MAX_PENDING: usize = 1 << 20;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A web proxy's settings with the secret of its password.
struct ProxyCfg {
    cfg: TunnelWebProxy,
    secret: Secret,
}

/// A web proxy as the pages show it (fit's `WebProxyView`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebProxyView {
    /// `name`.
    pub name: String,
    /// `listen`.
    pub listen: String,
    /// Where it listens, or "" when it could not.
    pub addr: String,
    /// `url`.
    pub url: String,
    /// Its connections open.
    pub conns: usize,
    /// Why it does not listen, or "".
    pub error: String,
}

/// A web proxy of the config and its listener: listening at `addr`, or failed to with `err`.
pub(crate) struct WebProxy {
    engine: Arc<akio::Engine>,
    set: Mutex<Arc<ProxyCfg>>,
    id: Option<akio::ListenerId>,
    addr: Option<SocketAddr>,
    err: String,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    conns: HashMap<u64, Arc<ProxyConn>>,
    closed: bool,
    next: u64,
}

/// A local connection and its WebSocket.
pub(crate) struct ProxyConn {
    id: u64,
    proxy: Weak<WebProxy>,
    listen: String,
    local: akio::Conn<LocalHandler>,
    lgate: Arc<Gate>,
    secret: Secret,
    st: Mutex<ConnSt>,
}

#[derive(Default)]
struct ConnSt {
    ws: Option<WsSender>,
    ws_gate: Option<Arc<Gate>>,
    sess: Option<Session>,
    pending: Vec<Vec<u8>>,
    pending_bytes: usize,
    closed: bool,
}

/// The WebSocket settings of a web proxy's dial: a message a sealed chunk with room to spare, no
/// compression, the handshake within [`DIAL_TIMEOUT`], `User-Agent: akit`.
fn dial_options() -> akio_http::ws::client::DialOptions {
    let mut c = WsConfig::default();
    c.max_message_bytes = 2 * SEAL_CHUNK;
    c.permessage_deflate = false;
    let mut o = akio_http::ws::client::DialOptions::default();
    o.config = c;
    o.handshake_timeout = DIAL_TIMEOUT;
    o.happy_eyeballs.connect_timeout = DIAL_TIMEOUT;
    o.headers = vec![("User-Agent".to_owned(), "akit".to_owned())];
    o
}

impl WebProxy {
    /// Listens for `cfg`, its connections filtered by `access`. A failure is kept in the web
    /// proxy, which listens on nothing.
    fn start(
        engine: &Arc<akio::Engine>,
        cfg: &TunnelWebProxy,
        access: Option<&Arc<akit_access::Policy>>,
    ) -> Arc<WebProxy> {
        let set = Arc::new(ProxyCfg {
            cfg: cfg.clone(),
            secret: Secret::new(&cfg.password),
        });
        let mut err = String::new();
        let mut id = None;
        let mut addr = None;
        let p = Arc::new_cyclic(|me: &Weak<WebProxy>| {
            match crate::proxy::listen_addr(&cfg.listen) {
                None => err = "not an address to listen on".to_owned(),
                Some(at) => {
                    let me = me.clone();
                    let mut l = akio::Listener::tcp(at).threads(1).serve(akio::fn_acceptor(
                        move |c: &akio::Conn<LocalHandler>| match me.upgrade() {
                            Some(p) => p.accepted(c),
                            None => {
                                c.close();
                                LocalHandler(None)
                            }
                        },
                    ));
                    if let Some(p) = access.cloned() {
                        l = l.filter(move |peer: &SocketAddr| p.allows(peer.ip()));
                    }
                    match engine.add_listener(l) {
                        Ok(i) => {
                            id = Some(i);
                            addr = engine
                                .listeners()
                                .iter()
                                .find(|x| x.id == i)
                                .map(|x| x.local_addr);
                        }
                        Err(e) => err = e.to_string(),
                    }
                }
            }
            WebProxy {
                engine: Arc::clone(engine),
                set: Mutex::new(Arc::clone(&set)),
                id,
                addr,
                err: err.clone(),
                inner: Mutex::new(Inner::default()),
            }
        });
        if p.err.is_empty() {
            if let Some(a) = p.addr {
                akit_observe::info!(
                    Tunnel,
                    "tunnel.proxy: serving the console of {} on http://{a}",
                    cfg.url
                );
            }
        } else {
            akit_observe::warn!(Tunnel, "tunnel.proxy: web proxy {}: {}", cfg.listen, p.err);
        }
        p
    }

    /// Stops listening, and closes every connection.
    fn close(&self) {
        if let Some(id) = self.id {
            let _ = self.engine.remove_listener(id);
        }
        let conns: Vec<Arc<ProxyConn>> = {
            let mut inner = lock(&self.inner);
            inner.closed = true;
            inner.conns.values().cloned().collect()
        };
        for c in conns {
            c.local.close();
        }
    }

    /// A local connection: its WebSocket dialed, its reads held until the session is up.
    fn accepted(self: &Arc<Self>, c: &akio::Conn<LocalHandler>) -> LocalHandler {
        let set = Arc::clone(&lock(&self.set));
        let pc = {
            let mut inner = lock(&self.inner);
            if inner.closed || inner.conns.len() >= set.cfg.conns() {
                drop(inner);
                c.close();
                return LocalHandler(None);
            }
            // Paced by the WebSocket's gate, not by its own output.
            c.set_relayed(true);
            inner.next = inner.next.wrapping_add(1);
            let pc = Arc::new(ProxyConn {
                id: inner.next,
                proxy: Arc::downgrade(self),
                listen: set.cfg.listen.clone(),
                local: c.clone(),
                lgate: Gate::new(Box::new(c.clone())),
                secret: set.secret.clone(),
                st: Mutex::new(ConnSt::default()),
            });
            inner.conns.insert(pc.id, Arc::clone(&pc));
            pc
        };
        // The local connection waits for the WebSocket.
        pc.lgate.hold();
        let (listen, url) = (set.cfg.listen.clone(), set.cfg.url.clone());
        let failed = Arc::clone(&pc);
        akio_http::ws::client::dial(
            &self.engine,
            &set.cfg.url,
            &dial_options(),
            ProxyWs(Some(Arc::clone(&pc))),
            move |e| {
                akit_observe::warn!(Tunnel, "tunnel.proxy: web proxy {listen}: {url}: {e}");
                failed.local.close();
            },
        );
        LocalHandler(Some(pc))
    }

    fn view(&self) -> WebProxyView {
        let set = Arc::clone(&lock(&self.set));
        WebProxyView {
            name: set.cfg.name.clone(),
            listen: set.cfg.listen.clone(),
            addr: self.addr.map(|a| a.to_string()).unwrap_or_default(),
            url: set.cfg.url.clone(),
            conns: lock(&self.inner).conns.len(),
            error: self.err.clone(),
        }
    }
}

impl ProxyConn {
    /// Sealed, a message per piece, and sent in the order of the messages' numbers: under the
    /// connection's lock. A message not sent leaves its number unused, and nothing after it
    /// would open: the caller ends the WebSocket ([`ProxyConn::send_failed`]).
    fn send_sealed(st: &mut ConnSt, data: &[u8]) -> Result<(), akio::Error> {
        let (Some(ws), Some(sess)) = (st.ws.as_ref(), st.sess.as_mut()) else {
            return Err(akio::Error::Closed);
        };
        for chunk in data.chunks(SEAL_CHUNK) {
            ws.send_binary(&sess.seal(chunk))?;
        }
        Ok(())
    }

    /// A message to the hub was not sent: the WebSocket ends, as it would at the hub on the
    /// next message, which would not open.
    fn send_failed(&self, ws: &WsSender, e: &akio::Error) {
        if !matches!(e, akio::Error::Closed) {
            akit_observe::warn!(
                Tunnel,
                "tunnel.proxy: web proxy {}: a message could not be sent ({e}); the WebSocket is ended",
                self.listen
            );
        }
        ws.abort();
    }

    /// What the local connection sends: sealed once the session is up.
    fn local_sent(&self, data: &[u8]) {
        let mut st = lock(&self.st);
        if st.closed {
            return;
        }
        if st.sess.is_none() {
            // What came before the hold took effect.
            st.pending_bytes = st.pending_bytes.saturating_add(data.len());
            if st.pending_bytes > MAX_PENDING {
                drop(st);
                self.local.close();
                return;
            }
            st.pending.push(data.to_vec());
            return;
        }
        let sent = Self::send_sealed(&mut st, data);
        let ws = st.ws.clone();
        drop(st);
        if let Some(ws) = ws {
            if let Err(e) = sent {
                self.send_failed(&ws, &e);
                return;
            }
            self.lgate.pace(&ws);
        }
    }

    fn local_closed(&self) {
        if let Some(p) = self.proxy.upgrade() {
            lock(&p.inner).conns.remove(&self.id);
        }
        let ws = {
            let mut st = lock(&self.st);
            st.closed = true;
            st.pending = Vec::new();
            st.ws.clone()
        };
        if let Some(ws) = ws {
            ws.close(1000, "");
        }
    }

    fn ws_open(&self, ws: &WsSender) {
        ws.set_relayed(true);
        let closed = {
            let mut st = lock(&self.st);
            st.ws = Some(ws.clone());
            st.ws_gate = Some(Gate::new(Box::new(ws.clone())));
            st.closed
        };
        if closed {
            ws.abort();
        }
    }

    /// The hub's hello, and then what the console sends.
    fn ws_sent(&self, ws: &WsSender, data: &[u8]) {
        let mut st = lock(&self.st);
        if st.closed {
            return;
        }
        let Some(sess) = st.sess.as_mut() else {
            let hello = crate::wscrypt::hello();
            let mut sess = match Session::new(&self.secret, data, &hello, false) {
                Ok(s) => s,
                Err(_) => {
                    drop(st);
                    ws.abort();
                    return;
                }
            };
            // An empty message proves the password at once, so that the hub keeps the WebSocket
            // while the browser sends nothing yet.
            let mut sent = ws
                .send_binary(&hello)
                .and_then(|()| ws.send_binary(&sess.seal(&[])));
            st.sess = Some(sess);
            for b in std::mem::take(&mut st.pending) {
                if sent.is_ok() {
                    sent = Self::send_sealed(&mut st, &b);
                }
            }
            st.pending_bytes = 0;
            drop(st);
            if let Err(e) = sent {
                self.send_failed(ws, &e);
                return;
            }
            self.lgate.release();
            self.lgate.pace(ws);
            return;
        };
        let plain = sess.open(data);
        let gate = st.ws_gate.clone();
        drop(st);
        match plain {
            Ok(p) => {
                if self.local.send(&p).is_err() {
                    ws.abort();
                    return;
                }
                if let Some(g) = gate {
                    g.pace(&self.local);
                }
            }
            Err(_) => {
                akit_observe::warn!(
                    Tunnel,
                    "tunnel.proxy: web proxy {}: a message failed to decrypt; is the password right?",
                    self.listen
                );
                ws.abort();
            }
        }
    }
}

/// akio's handler of a web proxy's local connection.
pub(crate) struct LocalHandler(Option<Arc<ProxyConn>>);

impl akio::Handler for LocalHandler {
    fn on_data(&mut self, _c: &akio::Conn<Self>, data: &[u8]) {
        if let Some(pc) = &self.0 {
            pc.local_sent(data);
        }
    }

    fn on_close(&mut self, _c: &akio::Conn<Self>, _err: Option<akio::Error>) {
        if let Some(pc) = self.0.take() {
            pc.local_closed();
        }
    }
}

/// akio-ws's handler of a local connection's WebSocket.
struct ProxyWs(Option<Arc<ProxyConn>>);

impl WsHandler for ProxyWs {
    fn on_open(&mut self, ws: &WsSender) {
        if let Some(pc) = &self.0 {
            pc.ws_open(ws);
        }
    }

    fn on_message_ref(&mut self, ws: &WsSender, msg: MessageRef<'_>) {
        if let Some(pc) = &self.0 {
            pc.ws_sent(ws, msg.as_bytes());
        }
    }

    fn on_close(&mut self, _code: Option<u16>, _reason: &str) {
        if let Some(pc) = self.0.take() {
            pc.local.close_after_send();
        }
    }
}

/// A role's web proxies.
#[derive(Default)]
pub(crate) struct WebProxies {
    list: Mutex<Vec<Arc<WebProxy>>>,
}

impl WebProxies {
    /// Listens for the web proxies `want` adds, stops those it leaves out, and moves the rest to
    /// their new settings (a new password, a new bound) for the connections they accept from
    /// then on, their filter to `access`. A web proxy that failed to listen is tried again.
    pub(crate) fn apply(
        &self,
        engine: &Arc<akio::Engine>,
        want: &[TunnelWebProxy],
        access: Option<&Arc<akit_access::Policy>>,
    ) -> Vec<String> {
        let mut list = lock(&self.list);
        let mut have: HashMap<String, Arc<WebProxy>> = list
            .drain(..)
            .map(|p| {
                let listen = lock(&p.set).cfg.listen.clone();
                (listen, p)
            })
            .collect();
        let mut errs = Vec::new();
        let mut next = Vec::with_capacity(want.len());
        for w in want {
            if let Some(p) = have.remove(&w.listen).filter(|p| p.id.is_some()) {
                let old = Arc::clone(&lock(&p.set));
                let secret = if old.cfg.password == w.password {
                    old.secret.clone()
                } else {
                    Secret::new(&w.password)
                };
                *lock(&p.set) = Arc::new(ProxyCfg {
                    cfg: w.clone(),
                    secret,
                });
                if let Some(id) = p.id {
                    let _ = match access.cloned() {
                        Some(a) => engine
                            .set_listener_filter(id, move |peer: &SocketAddr| a.allows(peer.ip())),
                        None => engine.clear_listener_filter(id),
                    };
                }
                next.push(p);
                continue;
            }
            let p = WebProxy::start(engine, w, access);
            if !p.err.is_empty() {
                errs.push(format!("web proxy {}: {}", w.listen, p.err));
            }
            next.push(p);
        }
        for (_, p) in have {
            p.close();
        }
        *list = next;
        errs
    }

    /// Stops every web proxy.
    pub(crate) fn stop(&self) {
        for p in lock(&self.list).drain(..) {
            p.close();
        }
    }

    /// The web proxies, as the pages show them.
    pub(crate) fn views(&self) -> Vec<WebProxyView> {
        lock(&self.list).iter().map(|p| p.view()).collect()
    }
}

#[cfg(test)]
mod tests;
