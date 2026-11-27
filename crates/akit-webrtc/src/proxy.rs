//! Media servers behind akit (I114, fit's `webrtc/media/proxy.go` and `relay.go`): the role
//! `webrtc.proxy`, one UDP port that the peers of every media server behind the `webrtc_proxy`
//! handler reach, each new peer address relayed to the media server that answered its session.
//!
//! The handler (`proxy_http`) records each answered session's ICE credentials, the ufrag and pwd
//! of the media server's answer, and the address its peers are relayed to. A new peer address's
//! first datagram must be a STUN Binding request whose USERNAME begins with one of those ufrags
//! and whose MESSAGE-INTEGRITY that ufrag's pwd signs; anything else is dropped, so that the port
//! relays nothing to anyone who did not take an answer. The relay is a UDP socket of its own toward
//! the media server (from `relay_ip`, on a port of `port_min`..`port_max` when set), and relays the
//! 5-tuple both ways from then on, sticky; a peer address silent for `idle_timeout` (whatever the
//! media server sends it) has its relay closed (akio's per-connection idle timeout). A session
//! with nothing relayed for `session_timeout` is forgotten. A DELETE forgets it at once, its relays
//! closed a second later, for the media server's answer to the peer's last datagrams.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock, Weak};
use std::time::Duration;

use akio::{Conn, ListenerId, UdpBinding};
use akit_config::Config;
use akit_config::webrtc::WebRtcProxy;
use akit_observe::metrics::TextWriter;
use akit_roles::host::{Host, Info, MetricsFn, Role};

/// `webrtc.proxy`.
pub const PROXY: Info = Info {
    id: "webrtc.proxy",
    set: |c| c.webrtc.as_ref().is_some_and(|w| w.proxy.is_some()),
    new,
};

/// The most peer addresses one session relays (fit's `maxPaths`).
const MAX_PATHS: usize = 8;
/// The most ufrags one session keeps, its ICE restarts' among them (fit's `maxUfrags`).
const MAX_UFRAGS: usize = 4;
/// How long a session's relays outlive its DELETE (fit's `endLinger`).
const END_LINGER: Duration = Duration::from_secs(1);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn read<T: Clone>(l: &RwLock<T>) -> T {
    l.read().unwrap_or_else(PoisonError::into_inner).clone()
}

// ---------------------------------------------------------------------------------------------
// The proxies of the process, by the engine the handlers are given.

type Proxies = Mutex<Vec<(usize, Weak<Core>)>>;

fn proxies() -> &'static Proxies {
    static S: OnceLock<Proxies> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Vec::new()))
}

/// The proxy the `webrtc_proxy` handlers of `engine` use, or `None` while none runs.
pub(crate) fn lookup(engine: &Arc<akio::Engine>) -> Option<Arc<Core>> {
    let k = Arc::as_ptr(engine) as usize;
    let mut s = lock(proxies());
    s.retain(|(_, w)| w.strong_count() > 0);
    s.iter()
        .find(|(e, _)| *e == k)
        .and_then(|(_, w)| w.upgrade())
}

// ---------------------------------------------------------------------------------------------
// The state.

/// A session's ICE credentials, from a media server's answer, and where its peers are relayed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cred {
    pub(crate) ufrag: String,
    pub(crate) pwd: String,
    pub(crate) media: Option<SocketAddr>,
}

/// A session a media server answered through akit.
pub(crate) struct Session {
    pub(crate) id: String,
    /// The media server's address in the upstream.
    pub(crate) backend: String,
    /// The session's URL at the media server.
    pub(crate) location: String,
    gone: AtomicBool,
    /// Bumped by every arm of the session timer, so that an older one does nothing.
    timer: AtomicU64,
}

struct Route {
    s: Arc<Session>,
    pwd: Vec<u8>,
    media: Option<SocketAddr>,
}

struct Settings {
    max_sessions: usize,
    max_relays: usize,
    session_idle: Duration,
    idle: Duration,
    public: Vec<IpAddr>,
    relay_ip: Option<IpAddr>,
    port_min: u16,
    port_max: u16,
}

impl Settings {
    fn of(p: &WebRtcProxy) -> Settings {
        Settings {
            max_sessions: p.sessions(),
            max_relays: p.relays(),
            session_idle: p.session_idle(),
            idle: p.idle(),
            public: p.public_ips.clone(),
            relay_ip: p.relay_ip,
            port_min: p.port_min,
            port_max: p.port_max,
        }
    }
}

struct Port {
    listen: String,
    id: ListenerId,
    bound: SocketAddr,
}

#[derive(Default)]
struct State {
    port: Option<Port>,
    stopped: bool,
    sessions: HashMap<String, Arc<Session>>,
    /// Each session's ufrags, oldest first.
    ufrag_lists: HashMap<String, Vec<String>>,
    ufrags: HashMap<String, Route>,
    /// Each session's relays.
    relays: HashMap<String, Vec<Arc<Relay>>>,
    relay_count: usize,
    reserved: usize,
}

const DROPS: [&str; 5] = ["not_stun", "unknown_ufrag", "integrity", "limit", "no_port"];
const DROP_NOT_STUN: usize = 0;
const DROP_UNKNOWN: usize = 1;
const DROP_INTEGRITY: usize = 2;
const DROP_LIMIT: usize = 3;
const DROP_NO_PORT: usize = 4;
const ENDS: [&str; 3] = ["delete", "expired", "reload"];
const END_DELETE: usize = 0;
const END_EXPIRED: usize = 1;
const END_RELOAD: usize = 2;

#[derive(Default)]
struct Meters {
    answered: AtomicU64,
    refused: AtomicU64,
    opened: AtomicU64,
    to_backend: AtomicU64,
    to_client: AtomicU64,
    restarts: AtomicU64,
    dropped: [AtomicU64; 5],
    ended: [AtomicU64; 3],
}

fn count(c: &AtomicU64) {
    c.fetch_add(1, Ordering::Relaxed);
}

fn add(c: &AtomicU64, n: usize) {
    c.fetch_add(u64::try_from(n).unwrap_or(0), Ordering::Relaxed);
}

/// The role's state, shared with the handlers and the port's connections.
pub(crate) struct Core {
    engine: Arc<akio::Engine>,
    set: RwLock<Arc<Settings>>,
    st: Mutex<State>,
    access: Mutex<Option<Arc<akit_access::Policy>>>,
    m: Meters,
    me: Weak<Core>,
}

impl Core {
    /// The addresses the port is reached at: `public_ips` with its port, the interfaces' for a
    /// wildcard, or the bound address.
    pub(crate) fn addrs(&self) -> Vec<SocketAddr> {
        let Some(bound) = lock(&self.st).port.as_ref().map(|p| p.bound) else {
            return Vec::new();
        };
        crate::media::advertised(bound, &read(&self.set).public)
    }

    /// Takes a place for a session about to be answered, or says there is none.
    pub(crate) fn reserve(&self) -> bool {
        let max = read(&self.set).max_sessions;
        let mut st = lock(&self.st);
        if st.port.is_none() || st.sessions.len().saturating_add(st.reserved) >= max {
            count(&self.m.refused);
            return false;
        }
        st.reserved = st.reserved.saturating_add(1);
        true
    }

    /// Gives back a place [`reserve`](Self::reserve) took for an answer that did not come.
    pub(crate) fn unreserve(&self) {
        let mut st = lock(&self.st);
        st.reserved = st.reserved.saturating_sub(1);
    }

    /// Records a session answered by `backend` at `location`, with its credentials; its ID.
    pub(crate) fn add(
        &self,
        backend: &str,
        location: &str,
        creds: &[Cred],
    ) -> Result<String, String> {
        let s = Arc::new(Session {
            id: crate::media::hex_id(),
            backend: backend.to_owned(),
            location: location.to_owned(),
            gone: AtomicBool::new(false),
            timer: AtomicU64::new(0),
        });
        {
            let mut st = lock(&self.st);
            st.reserved = st.reserved.saturating_sub(1);
            if st.port.is_none() {
                return Err("webrtc.proxy is not running".to_owned());
            }
            add_ufrags(&mut st, &s, creds)?;
            st.sessions.insert(s.id.clone(), Arc::clone(&s));
        }
        count(&self.m.answered);
        self.arm(&s);
        Ok(s.id.clone())
    }

    /// Adds the credentials of an ICE restart's answer to the session `id`.
    pub(crate) fn restart(&self, id: &str, creds: &[Cred]) -> Result<(), String> {
        let mut st = lock(&self.st);
        let Some(s) = st.sessions.get(id).cloned() else {
            return Err("no such session".to_owned());
        };
        add_ufrags(&mut st, &s, creds)
    }

    /// The session `id`.
    pub(crate) fn session(&self, id: &str) -> Option<Arc<Session>> {
        lock(&self.st).sessions.get(id).cloned()
    }

    /// Forgets the session `id`, its relays closed a moment later.
    pub(crate) fn end(&self, id: &str) {
        let relays = {
            let mut st = lock(&self.st);
            let Some(s) = st.sessions.get(id).cloned() else {
                return;
            };
            self.forget(&mut st, &s, END_DELETE)
        };
        if relays.is_empty() {
            return;
        }
        // About time: the relays outlive the DELETE for the media server's last answers.
        let _ = akio::timer::after_send(END_LINGER, move || {
            for r in relays {
                r.close();
            }
        });
    }

    /// Arms the session timer: the session is forgotten after `session_timeout` with nothing
    /// relayed for it.
    fn arm(&self, s: &Arc<Session>) {
        let armed = s.timer.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let idle = read(&self.set).session_idle;
        let (core, sess) = (self.me.clone(), Arc::downgrade(s));
        let _ = akio::timer::after_send(idle, move || {
            let (Some(c), Some(s)) = (core.upgrade(), sess.upgrade()) else {
                return;
            };
            if s.timer.load(Ordering::Relaxed) != armed {
                return;
            }
            let mut st = lock(&c.st);
            if !s.gone.load(Ordering::Relaxed) && st.relays.get(&s.id).is_none_or(Vec::is_empty) {
                let _ = c.forget(&mut st, &s, END_EXPIRED);
            }
        });
    }

    /// Forgets `s`; its relays, for the caller to close once the lock is released.
    fn forget(&self, st: &mut State, s: &Arc<Session>, why: usize) -> Vec<Arc<Relay>> {
        if s.gone.swap(true, Ordering::Relaxed) {
            return Vec::new();
        }
        s.timer.fetch_add(1, Ordering::Relaxed);
        st.sessions.remove(&s.id);
        for u in st.ufrag_lists.remove(&s.id).unwrap_or_default() {
            if st.ufrags.get(&u).is_some_and(|r| Arc::ptr_eq(&r.s, s)) {
                st.ufrags.remove(&u);
            }
        }
        if let Some(c) = self.m.ended.get(why) {
            count(c);
        }
        st.relays.remove(&s.id).unwrap_or_default()
    }

    /// A new peer address's first datagram on the port: its relay, or `None` when it is dropped.
    fn open(&self, c: &Conn<PortHandler>, data: &[u8]) -> Option<Arc<Relay>> {
        let refuse = |why: usize| {
            if let Some(m) = self.m.dropped.get(why) {
                count(m);
            }
            c.close();
            None
        };
        let Some(ufrag) = request_ufrag(data) else {
            return refuse(DROP_NOT_STUN);
        };
        let route = {
            let st = lock(&self.st);
            st.ufrags
                .get(ufrag)
                .map(|r| (Arc::clone(&r.s), r.pwd.clone(), r.media))
        };
        let Some((s, pwd, media)) = route else {
            return refuse(DROP_UNKNOWN);
        };
        let signed = akio_turn::stun::Message::parse(data).is_ok_and(|m| m.check_integrity(&pwd));
        if !signed {
            return refuse(DROP_INTEGRITY);
        }
        let Some(media) = media else {
            return refuse(DROP_UNKNOWN);
        };
        let set = read(&self.set);
        {
            let mut st = lock(&self.st);
            let paths = st.relays.get(&s.id).map_or(0, Vec::len);
            if s.gone.load(Ordering::Relaxed)
                || st.relay_count >= set.max_relays
                || paths >= MAX_PATHS
            {
                drop(st);
                return refuse(DROP_LIMIT);
            }
            st.relay_count = st.relay_count.saturating_add(1);
        }
        let relay = Arc::new(Relay {
            core: self.me.clone(),
            session: Arc::clone(&s),
            client: c.clone(),
            up: OnceLock::new(),
            media,
            ended: AtomicBool::new(false),
        });
        let Some(up) = self.bind_up(&relay, &set) else {
            let mut st = lock(&self.st);
            st.relay_count = st.relay_count.saturating_sub(1);
            drop(st);
            return refuse(DROP_NO_PORT);
        };
        let _ = relay.up.set(up);
        {
            let mut st = lock(&self.st);
            if s.gone.load(Ordering::Relaxed) {
                st.relay_count = st.relay_count.saturating_sub(1);
                drop(st);
                relay.close_up();
                return refuse(DROP_LIMIT);
            }
            st.relays
                .entry(s.id.clone())
                .or_default()
                .push(Arc::clone(&relay));
        }
        count(&self.m.opened);
        // akio closes the peer's connection after idle_timeout without a datagram from it.
        let _ = c.set_idle_timeout(Some(set.idle));
        relay.to_backend(data);
        Some(relay)
    }

    /// The relay's socket toward its media server: from `relay_ip`, or the wildcard of the media
    /// server's family, on a port of the range when one is set (one in use skipped).
    fn bind_up(&self, relay: &Arc<Relay>, set: &Settings) -> Option<UdpBinding> {
        let ip = set.relay_ip.unwrap_or(if relay.media.is_ipv4() {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        } else {
            IpAddr::V6(Ipv6Addr::UNSPECIFIED)
        });
        let up = UpAcceptor(Arc::downgrade(relay));
        if set.port_max == 0 {
            return self.engine.bind_udp(SocketAddr::new(ip, 0), up).ok();
        }
        let span = u32::from(set.port_max.saturating_sub(set.port_min)).saturating_add(1);
        let mut r = [0u8; 4];
        let _ = akio_crypto::rand::fill_random(&mut r);
        let start = u32::from_le_bytes(r).checked_rem(span).unwrap_or(0);
        for i in 0..span {
            let off = start.saturating_add(i).checked_rem(span).unwrap_or(0);
            let port = u16::try_from(u32::from(set.port_min).saturating_add(off)).ok()?;
            match self
                .engine
                .bind_udp(SocketAddr::new(ip, port), UpAcceptor(Arc::downgrade(relay)))
            {
                Ok(b) => return Some(b),
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {}
                Err(_) => return None,
            }
        }
        None
    }

    /// A relay ended: forgotten, its session's timer armed again when it was the last.
    fn relay_closed(&self, r: &Arc<Relay>) {
        let rearm = {
            let mut st = lock(&self.st);
            st.relay_count = st.relay_count.saturating_sub(1);
            let left = st.relays.get_mut(&r.session.id).map(|v| {
                v.retain(|x| !Arc::ptr_eq(x, r));
                v.len()
            });
            left == Some(0) && !r.session.gone.load(Ordering::Relaxed)
        };
        if rearm {
            self.arm(&r.session);
        }
    }

    /// Forgets every session (a move of the port or a stop); their relays, to close.
    fn forget_all(&self, st: &mut State) -> Vec<Arc<Relay>> {
        let all: Vec<Arc<Session>> = st.sessions.values().cloned().collect();
        let mut relays = Vec::new();
        for s in all {
            relays.extend(self.forget(st, &s, END_RELOAD));
        }
        relays
    }
}

fn add_ufrags(st: &mut State, s: &Arc<Session>, creds: &[Cred]) -> Result<(), String> {
    for c in creds {
        if st
            .ufrags
            .get(&c.ufrag)
            .is_some_and(|r| !Arc::ptr_eq(&r.s, s))
        {
            return Err(format!("the ufrag {:?} is another session's", c.ufrag));
        }
    }
    let list = st.ufrag_lists.entry(s.id.clone()).or_default().clone();
    let keep = list
        .last()
        .and_then(|u| st.ufrags.get(u))
        .and_then(|r| r.media);
    let mut list = list;
    for c in creds {
        if !list.contains(&c.ufrag) {
            list.push(c.ufrag.clone());
        }
        st.ufrags.insert(
            c.ufrag.clone(),
            Route {
                s: Arc::clone(s),
                pwd: c.pwd.as_bytes().to_vec(),
                media: c.media.or(keep),
            },
        );
    }
    while list.len() > MAX_UFRAGS {
        let old = list.remove(0);
        st.ufrags.remove(&old);
    }
    st.ufrag_lists.insert(s.id.clone(), list);
    Ok(())
}

/// The ufrag of the server a STUN Binding request is for: its USERNAME's part before the colon;
/// `None` for anything else (fit's `requestUfrag`).
fn request_ufrag(b: &[u8]) -> Option<&str> {
    let head = b.get(..20)?;
    if head.get(..2)? != [0x00, 0x01] || head.get(4..8)? != [0x21, 0x12, 0xa4, 0x42] {
        return None;
    }
    let len = usize::from(u16::from_be_bytes([*head.get(2)?, *head.get(3)?]));
    let end = len.checked_add(20)?;
    let body = b.get(20..end)?;
    let mut i = 0usize;
    while i.checked_add(4)? <= body.len() {
        let t = u16::from_be_bytes([*body.get(i)?, *body.get(i.checked_add(1)?)?]);
        let l = usize::from(u16::from_be_bytes([
            *body.get(i.checked_add(2)?)?,
            *body.get(i.checked_add(3)?)?,
        ]));
        let v = i.checked_add(4)?;
        let value = body.get(v..v.checked_add(l)?)?;
        if t == 0x0006 {
            let colon = value.iter().position(|c| *c == b':')?;
            return std::str::from_utf8(value.get(..colon)?).ok();
        }
        i = v.checked_add(l.checked_add(3)? & !3)?;
    }
    None
}

// ---------------------------------------------------------------------------------------------
// The relays.

/// A peer address relayed to its media server.
struct Relay {
    core: Weak<Core>,
    session: Arc<Session>,
    client: Conn<PortHandler>,
    up: OnceLock<UdpBinding>,
    media: SocketAddr,
    ended: AtomicBool,
}

impl Relay {
    fn to_backend(&self, data: &[u8]) {
        if let (Some(up), Some(c)) = (self.up.get(), self.core.upgrade()) {
            if up.send_to(data, self.media).is_ok() {
                add(&c.m.to_backend, data.len());
            }
        }
    }

    fn close_up(&self) {
        if let Some(up) = self.up.get() {
            up.close();
        }
    }

    /// Ends the relay: both sockets closed, forgotten by the proxy.
    fn close(self: &Arc<Self>) {
        if self.ended.swap(true, Ordering::Relaxed) {
            return;
        }
        self.client.close();
        self.close_up();
        if let Some(c) = self.core.upgrade() {
            c.relay_closed(self);
        }
    }
}

/// A peer address on the port: relayed once its first datagram opened a relay.
pub(crate) struct PortHandler {
    core: Weak<Core>,
    relay: Option<Arc<Relay>>,
}

impl akio::Handler for PortHandler {
    fn on_data(&mut self, c: &Conn<Self>, data: &[u8]) {
        match &self.relay {
            Some(r) => r.to_backend(data),
            None => {
                if let Some(core) = self.core.upgrade() {
                    self.relay = core.open(c, data);
                } else {
                    c.close();
                }
            }
        }
    }

    fn on_close(&mut self, _c: &Conn<Self>, _err: Option<akio::Error>) {
        if let Some(r) = self.relay.take() {
            r.close();
        }
    }
}

/// The relay's socket's acceptor: the media server's datagrams go to the peer.
struct UpAcceptor(Weak<Relay>);

struct UpHandler(Weak<Relay>);

impl akio::Acceptor for UpAcceptor {
    type Handler = UpHandler;
    fn on_open(&self, c: &Conn<UpHandler>) -> UpHandler {
        // Only the media server is answered: anyone else's datagrams to the relay's port close
        // their connection.
        if self.0.upgrade().is_none_or(|r| c.peer_addr() != r.media) {
            c.close();
        }
        UpHandler(self.0.clone())
    }
}

impl akio::Handler for UpHandler {
    fn on_data(&mut self, _c: &Conn<Self>, data: &[u8]) {
        if let Some(r) = self.0.upgrade() {
            if r.client.send(data).is_ok() {
                if let Some(c) = r.core.upgrade() {
                    add(&c.m.to_client, data.len());
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The role.

struct Proxy {
    host: Host,
    core: Arc<Core>,
    /// The config's `engine.event_loops`, for the port's I/O threads.
    event_loops: usize,
}

fn new(host: Host) -> Result<Box<dyn Role>, String> {
    let core = Arc::new_cyclic(|me| Core {
        engine: Arc::clone(&host.engine),
        set: RwLock::new(Arc::new(Settings::of(&WebRtcProxy::default()))),
        st: Mutex::new(State::default()),
        access: Mutex::new(None),
        m: Meters::default(),
        me: me.clone(),
    });
    Ok(Box::new(Proxy {
        host,
        core,
        event_loops: 0,
    }))
}

impl Proxy {
    fn bind(&self, listen: &str) -> Result<Port, String> {
        let addr = crate::turn::listen_addr(listen)
            .ok_or_else(|| format!("listen: {listen:?} is not an address to listen on"))?;
        let core = Arc::downgrade(&self.core);
        let mut l = crate::udp_listener(addr, self.event_loops).serve(akio::fn_acceptor(
            move |_c: &Conn<PortHandler>| PortHandler {
                core: core.clone(),
                relay: None,
            },
        ));
        if let Some(p) = lock(&self.core.access).clone() {
            l = l.filter(move |peer: &SocketAddr| p.allows(peer.ip()));
        }
        let id = self
            .host
            .engine
            .add_listener(l)
            .map_err(|e| format!("listen on {listen}: {e}"))?;
        let bound = self
            .host
            .engine
            .listeners()
            .into_iter()
            .find(|i| i.id == id)
            .map_or(addr, |i| i.local_addr);
        Ok(Port {
            listen: listen.to_owned(),
            id,
            bound,
        })
    }

    fn close_port(&self, port: &Port) -> Vec<Arc<Relay>> {
        let relays = {
            let mut st = lock(&self.core.st);
            st.port = None;
            self.core.forget_all(&mut st)
        };
        let _ = self.host.engine.shutdown_listener(port.id, Duration::ZERO);
        relays
    }
}

fn register(engine: &Arc<akio::Engine>, core: &Arc<Core>) {
    let k = Arc::as_ptr(engine) as usize;
    let mut s = lock(proxies());
    s.retain(|(e, w)| *e != k && w.strong_count() > 0);
    s.push((k, Arc::downgrade(core)));
}

impl Role for Proxy {
    fn apply(&mut self, config: &Config) -> Result<(), String> {
        self.event_loops = config.engine.event_loops;
        let Some(p) = config.webrtc.as_ref().and_then(|w| w.proxy.as_ref()) else {
            return Ok(());
        };
        let access = crate::turn::policy(p.access.as_ref(), config.access.as_ref())?;
        let settings = Arc::new(Settings::of(p));
        let core = &self.core;
        let prev = std::mem::replace(&mut *lock(&core.access), access.clone());
        let old = {
            let st = lock(&core.st);
            if st.stopped {
                return Err("the role stopped".to_owned());
            }
            st.port.as_ref().map(|x| (x.listen.clone(), x.id))
        };
        if let Some((listen, id)) = &old {
            if *listen == p.listen {
                let _ = match access {
                    Some(a) => self
                        .host
                        .engine
                        .set_listener_filter(*id, move |peer: &SocketAddr| a.allows(peer.ip())),
                    None => self.host.engine.clear_listener_filter(*id),
                };
                *core.set.write().unwrap_or_else(PoisonError::into_inner) = settings;
                return Ok(());
            }
        }
        // A new port: the old one's sessions end, the old port closes first (it may hold the
        // address).
        let old_port = lock(&core.st).port.take();
        let mut relays = Vec::new();
        if let Some(o) = &old_port {
            relays = self.close_port(o);
        }
        for r in relays {
            r.close();
        }
        match self.bind(&p.listen) {
            Ok(port) => {
                let at = port.bound;
                *core.set.write().unwrap_or_else(PoisonError::into_inner) = settings;
                lock(&core.st).port = Some(port);
                register(&self.host.engine, core);
                if old_port.is_some() {
                    count(&core.m.restarts);
                    akit_observe::warn!(
                        Webrtc,
                        "webrtc.proxy: the media proxy moved to {at}, ending its sessions"
                    );
                } else {
                    akit_observe::info!(Webrtc, "webrtc.proxy: media proxy on {at}");
                }
                Ok(())
            }
            Err(e) => {
                *lock(&core.access) = prev;
                if let Some(o) = old_port {
                    if let Ok(again) = self.bind(&o.listen) {
                        lock(&core.st).port = Some(again);
                    }
                }
                Err(e)
            }
        }
    }

    fn stop(&mut self, _grace: Duration) {
        let port = {
            let mut st = lock(&self.core.st);
            st.stopped = true;
            st.port.take()
        };
        let k = Arc::as_ptr(&self.host.engine) as usize;
        let me = Arc::as_ptr(&self.core);
        lock(proxies()).retain(|(e, w)| !(*e == k && w.as_ptr() == me));
        if let Some(p) = port {
            for r in self.close_port(&p) {
                r.close();
            }
        }
    }

    fn metrics(&self) -> Option<MetricsFn> {
        let core = Arc::downgrade(&self.core);
        Some(Arc::new(move |out: &mut Vec<u8>| {
            if let Some(c) = core.upgrade() {
                write_metrics(&c, out);
            }
        }))
    }
}

fn write_metrics(c: &Core, out: &mut Vec<u8>) {
    let (sessions, relays) = {
        let st = lock(&c.st);
        (st.sessions.len(), st.relay_count)
    };
    let n = |v: usize| u64::try_from(v).unwrap_or(u64::MAX);
    let load = |a: &AtomicU64| a.load(Ordering::Relaxed);
    let mut w = TextWriter::new(std::mem::take(out));
    for (name, typ, help, v) in [
        (
            "akit_webrtc_proxy_sessions",
            "gauge",
            "Sessions proxied to media servers, kept by their ufrags.",
            n(sessions),
        ),
        (
            "akit_webrtc_proxy_relays",
            "gauge",
            "Peer addresses relayed to their media servers, each with a socket of its own.",
            n(relays),
        ),
        (
            "akit_webrtc_proxy_sessions_total",
            "counter",
            "Sessions a media server answered through akit.",
            load(&c.m.answered),
        ),
        (
            "akit_webrtc_proxy_refused_total",
            "counter",
            "Offers answered 503 for max_sessions, or while the proxy does not run.",
            load(&c.m.refused),
        ),
        (
            "akit_webrtc_proxy_relays_total",
            "counter",
            "Relays opened for peers' STUN requests.",
            load(&c.m.opened),
        ),
    ] {
        w.family(name, typ, help);
        w.sample_u64(name, v, &[]);
    }
    w.family(
        "akit_webrtc_proxy_sessions_ended_total",
        "counter",
        "Sessions forgotten, by reason: a DELETE, nothing relayed for session_timeout, or the \
         port moved or stopped.",
    );
    for (name, v) in ENDS.iter().zip(&c.m.ended) {
        w.sample_u64(
            "akit_webrtc_proxy_sessions_ended_total",
            load(v),
            &[("reason", name)],
        );
    }
    w.family(
        "akit_webrtc_proxy_dropped_total",
        "counter",
        "First datagrams of new peer addresses dropped, by reason: not a STUN Binding request, a \
         ufrag no session has, a MESSAGE-INTEGRITY its ice-pwd does not sign, past max_relays or \
         a session's 8, or no local port free in port_min to port_max for its socket.",
    );
    for (name, v) in DROPS.iter().zip(&c.m.dropped) {
        w.sample_u64(
            "akit_webrtc_proxy_dropped_total",
            load(v),
            &[("reason", name)],
        );
    }
    w.family(
        "akit_webrtc_proxy_relayed_bytes_total",
        "counter",
        "Bytes relayed, by direction.",
    );
    w.sample_u64(
        "akit_webrtc_proxy_relayed_bytes_total",
        load(&c.m.to_backend),
        &[("direction", "to_backend")],
    );
    w.sample_u64(
        "akit_webrtc_proxy_relayed_bytes_total",
        load(&c.m.to_client),
        &[("direction", "to_client")],
    );
    w.family(
        "akit_webrtc_proxy_restarts_total",
        "counter",
        "Times a change of listen moved the port, ending its sessions.",
    );
    w.sample_u64("akit_webrtc_proxy_restarts_total", load(&c.m.restarts), &[]);
    *out = w.out;
}

#[cfg(test)]
mod tests;
