//! `tunnel.hub` (fit's `tunnel/hub`): it takes links from nodes and proxies on its listeners,
//! registers them (a token it issued, or the deprecated shared password), routes each tunnel a
//! proxy opens to the node it names, and passes the tunnel's frames between them, checking each
//! end's window: the windows bound what waits on each link, as fit's.

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use akit_config::Config;
use akit_config::roles::{TunnelHub, TunnelListener, TunnelProtocol};
use akit_config::value::Value;
use akit_roles::host::{Host, Role};
use akit_roles::linktoken::{Registry, Use};

use crate::crypt::Coder;
use crate::link::{ConnWire, Link, LinkHandler, Responder, Router, Wire};
use crate::msg::{
    self, Info, MAX_IN_FLIGHT, MAX_TUNNELS_PER_LINK, RegisterReq, RegisterRsp, Stats,
    TunnelOpenReq, Verifier, method,
};

mod api;
mod pages;
pub mod socket;
pub use api::{HubApi, InfoDone};

/// The most bytes of a frame on a link that has not registered.
const UNREGISTERED_FRAME: usize = 64 << 10;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How a registration's answer is proven: the hub's proof of the token or the password.
type Prove = Box<dyn FnOnce(&mut RegisterRsp) + Send>;

struct Challenge {
    token: String,
    peer: Vec<u8>,
    hub: Vec<u8>,
}

/// A registered link: its peer, its number, its place among the peer's links.
struct Reg {
    peer: u32,
    conn: u32,
    ready: bool,
}

struct LinkRec {
    link: Arc<Link>,
    ip: IpAddr,
    reg: Option<Reg>,
    challenge: Option<Challenge>,
    token_use: Option<Use>,
}

struct Peer {
    id: u32,
    info: Info,
    addr: String,
    since: i64,
    /// Its links by their place.
    links: BTreeMap<i64, u64>,
    rtt: Stats,
    service_rtt: String,
    by_token: bool,
}

struct Route {
    proxy: u32,
    node: u32,
    proxy_link: Arc<Link>,
    node_link: Arc<Link>,
    open: bool,
    up_flight: i64,
    down_flight: i64,
    /// The node's service, and its address as the node reports it once the tunnel is open.
    service: String,
    addr: String,
    /// When it opened, in seconds since 1970.
    created: i64,
    /// Bytes of data passed each way: from the proxy, and to it.
    up: i64,
    down: i64,
}

#[derive(Default)]
struct HubState {
    links: HashMap<u64, LinkRec>,
    unreg_ip: HashMap<IpAddr, usize>,
    by_name: HashMap<String, u32>,
    peers: HashMap<u32, Peer>,
    routes: HashMap<u64, Route>,
    next_peer: u32,
    next_conn: u32,
    nodes_version: u64,
    closing: bool,
}

pub(crate) struct HubCore {
    host: Host,
    cfg: Mutex<TunnelHub>,
    tokens: Registry,
    verifier: Mutex<Option<Verifier>>,
    coder: Mutex<Option<Coder>>,
    listeners: Mutex<Vec<Open>>,
    /// The files the listeners' certificates were read from when the member was last applied,
    /// with the SHA-256 of what was read (zeros for a file that could not be): what
    /// `-watch-certs` looks at (I102d).
    files: Mutex<Vec<(String, [u8; 32])>>,
    /// The certificate the hub made for itself when there is nowhere to keep it: made once, so
    /// that it stays the same across applies while akit runs.
    generated: Mutex<Option<Arc<(String, String)>>>,
    access: Mutex<Option<Arc<akit_access::Policy>>>,
    /// The SHA-256 of the hub's own certificate, which peers pin.
    fingerprint: Mutex<String>,
    st: Mutex<HubState>,
    epoch: u64,
    /// When the hub started, in seconds since 1970.
    started: i64,
    /// The sweep of expired tokens in force: an earlier one, superseded when the tokens change,
    /// ends at its time.
    sweep_gen: std::sync::atomic::AtomicU64,
    /// The console socket (I103b).
    socket: Mutex<Option<socket::Socket>>,
    me: Weak<HubCore>,
}

struct HubRouter(Weak<HubCore>);

/// A listener of the hub's: a TCP one of the engine's, or a UDP socket bound for QUIC.
enum Bound {
    Tcp(akio::ListenerId),
    Udp(akio::UdpBinding),
}

impl Bound {
    fn remove(self, engine: &akio::Engine) {
        match self {
            Bound::Tcp(id) => {
                let _ = engine.remove_listener(id);
            }
            Bound::Udp(b) => b.close(),
        }
    }
}

/// A listener of the hub's, open: its config, what it is bound as, and the certificate it serves
/// when it takes TLS.
struct Open {
    cfg: TunnelListener,
    bound: Bound,
    cert: Option<Served>,
}

/// The certificate a `tls`, `wss` or `quic` listener serves, replaced in place, without closing
/// the listener or a connection, when its files change (I102d).
struct Served {
    store: akio::tls::CertStore,
    /// What is served, to tell a change.
    pem: Arc<(String, String)>,
    /// Its SHA-256.
    fp: String,
}

impl Served {
    /// A listener's configuration serving `pem`, which [`Served::replace`] can replace.
    fn new(
        pem: Arc<(String, String)>,
        alpn: &str,
    ) -> Result<(Served, akio::tls::ServerConfig), String> {
        let store = akio::tls::CertStore::new();
        let cfg = crate::tls::server_config_store(&pem.0, &pem.1, alpn, &store)?;
        let fp = crate::tls::fingerprint(&pem.0);
        Ok((Served { store, pem, fp }, cfg))
    }

    /// Serves `pem` to the handshakes from now on; whether it changed. One that does not load
    /// leaves what is served.
    fn replace(&mut self, pem: Arc<(String, String)>) -> Result<bool, String> {
        if pem == self.pem {
            return Ok(false);
        }
        let key = crate::tls::certified_key(&pem.0, &pem.1)?;
        self.store.set_default(Some(key));
        self.fp = crate::tls::fingerprint(&pem.0);
        self.pem = pem;
        Ok(true)
    }
}

/// The SHA-256 of a file read, zeros for one that could not be: for `-watch-certs`.
fn file_sum(r: &std::io::Result<String>) -> [u8; 32] {
    r.as_ref()
        .map(|d| akio_crypto::hash::Sha256::digest(d.as_bytes()))
        .unwrap_or([0u8; 32])
}

fn kind_name(info: &Info) -> String {
    format!("{}/{}", info.kind, info.name)
}

impl HubCore {
    fn router(&self) -> Arc<dyn Router> {
        Arc::new(HubRouter(self.me.clone()))
    }

    /// A connection accepted on a TCP, TLS or WebSocket listener: a link, unless the hub's bounds
    /// refuse it.
    fn accept(self: &Arc<Self>, c: &akio::Conn<LinkHandler>) -> LinkHandler {
        match self.accept_wire(c.peer_addr(), Arc::new(ConnWire::new(c))) {
            Some(l) => LinkHandler(Some(l)),
            None => {
                c.close();
                LinkHandler(None)
            }
        }
    }

    /// A link on `wire` from `peer`, unless the hub's bounds refuse it (`None`: the caller closes
    /// the wire). Its access policy refused the peer before, at akio's filter: a TCP listener's
    /// at accept, a QUIC one's before a connection is made (ADR-178).
    fn accept_wire(self: &Arc<Self>, peer: SocketAddr, wire: Arc<dyn Wire>) -> Option<Arc<Link>> {
        let cfg = lock(&self.cfg).clone();
        let ip = peer.ip().to_canonical();
        let coder = lock(&self.coder).clone();
        let l = {
            let mut st = lock(&self.st);
            let unreg = st.links.values().filter(|r| r.reg.is_none()).count();
            if st.closing
                || st.links.len() >= cfg.links()
                || unreg >= cfg.unregistered()
                || st.unreg_ip.get(&ip).copied().unwrap_or(0) >= cfg.unregistered_per_ip()
            {
                return None;
            }
            let l = Link::new(wire, self.router(), coder);
            l.set_max_frame(UNREGISTERED_FRAME);
            st.links.insert(
                l.id,
                LinkRec {
                    link: Arc::clone(&l),
                    ip,
                    reg: None,
                    challenge: None,
                    token_use: None,
                },
            );
            let n = st.unreg_ip.entry(ip).or_insert(0);
            *n = n.saturating_add(1);
            l
        };
        let weak = Arc::downgrade(&l);
        let me = self.me.clone();
        let _ = akio::timer::after_send(cfg.register(), move || {
            if let (Some(l), Some(h)) = (weak.upgrade(), me.upgrade()) {
                let registered = lock(&h.st)
                    .links
                    .get(&l.id)
                    .is_some_and(|r| r.reg.is_some());
                if !registered {
                    l.close();
                }
            }
        });
        Some(l)
    }

    fn registered(st: &mut HubState, lid: u64) {
        let Some(ip) = st.links.get(&lid).map(|r| r.ip) else {
            return;
        };
        if let Some(n) = st.unreg_ip.get_mut(&ip) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                st.unreg_ip.remove(&ip);
            }
        }
    }

    fn challenge(&self, l: &Arc<Link>, body: &[u8], r: Responder) {
        let Some(v) = msg::parse(body) else {
            return r.err("tunnel: a bad request");
        };
        let Some(peer) = msg::decode_nonce(&msg::get_str(&v, "nonce")) else {
            return r.err("tunnel: a challenge's nonce is 32 bytes in hex");
        };
        let hub = akit_roles::linktoken::nonce().unwrap_or([0; 32]).to_vec();
        let mut st = lock(&self.st);
        let Some(rec) = st.links.get_mut(&l.id) else {
            return r.err("tunnel: the link closed");
        };
        if rec.challenge.is_some() || rec.reg.is_some() {
            return r.err("tunnel: a link asks for one challenge, before it registers");
        }
        let rsp = msg::Obj::new()
            .str("nonce", &akit_roles::linktoken::hex(&hub))
            .end();
        rec.challenge = Some(Challenge {
            token: msg::get_str(&v, "token"),
            peer,
            hub,
        });
        drop(st);
        r.ok(rsp.as_bytes());
    }

    fn register(self: &Arc<Self>, l: &Arc<Link>, body: &[u8], r: Responder) {
        let Some(req) = msg::parse(body).map(|v| RegisterReq::from(&v)) else {
            return r.err("tunnel: a bad request");
        };
        if let Err(e) = msg::check_peer(&req.info) {
            return r.err(&e);
        }
        if lock(&self.st)
            .links
            .get(&l.id)
            .is_some_and(|x| x.reg.is_some())
        {
            return r.err("tunnel: already registered");
        }
        // Authenticate: a token's proof, or the shared password's MAC.
        let auth: Result<(Prove, Option<Use>), String> = if req.token.is_empty() {
            if !l.hello_token().is_empty() {
                Err("tunnel: the link's hello named a token, and its registration none".to_owned())
            } else {
                let mut v = lock(&self.verifier);
                match v.as_mut() {
                    None => Err(
                        "tunnel: the hub takes tokens only: register with a token it issued"
                            .to_owned(),
                    ),
                    Some(v) => v.verify(&req).map(|()| {
                        let pw = v.password.clone();
                        let rq = req.clone();
                        (
                            Box::new(move |rsp: &mut RegisterRsp| msg::prove(&rq, rsp, &pw))
                                as Prove,
                            None,
                        )
                    }),
                }
            }
        } else {
            let ch = lock(&self.st)
                .links
                .get_mut(&l.id)
                .and_then(|x| x.challenge.take());
            match ch {
                Some(ch) if ch.token == req.token => {
                    let ht = l.hello_token();
                    if !ht.is_empty() && ht != req.token {
                        Err("tunnel: the link's hello named another token".to_owned())
                    } else {
                        match self.tokens.lookup(&req.token) {
                            Err(e) => Err(e.to_string()),
                            Ok(stored) => {
                                if !msg::verify_token(&req, &stored, &ch.peer, &ch.hub) {
                                    Err("tunnel: authentication failed".to_owned())
                                } else {
                                    let lw = Arc::downgrade(l);
                                    match self.tokens.admit(
                                        &req.token,
                                        &kind_name(&req.info),
                                        &l.remote(),
                                        Some(Box::new(move || {
                                            if let Some(l) = lw.upgrade() {
                                                l.close();
                                            }
                                        })),
                                    ) {
                                        Err(e) => Err(e.to_string()),
                                        Ok(u) => {
                                            let rq = req.clone();
                                            Ok((
                                                Box::new(move |rsp: &mut RegisterRsp| {
                                                    msg::prove_token_hub(
                                                        &rq, rsp, &stored, &ch.peer, &ch.hub,
                                                    );
                                                })
                                                    as Prove,
                                                Some(u),
                                            ))
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                _ => Err("tunnel: a registration with a token follows its challenge".to_owned()),
            }
        };
        let (prove, token_use) = match auth {
            Ok(a) => a,
            Err(e) => {
                akit_observe::warn!(
                    Tunnel,
                    "tunnel.hub: refused the registration of {} {:?} from {}: {e}",
                    req.info.kind,
                    req.info.name,
                    l.remote()
                );
                return r.err(&e);
            }
        };
        let by_token = token_use.is_some();
        let cfg = lock(&self.cfg).clone();
        let key = kind_name(&req.info);
        let (mut rsp, created) = {
            let mut st = lock(&self.st);
            let st = &mut *st;
            let existing = st.by_name.get(&key).copied();
            let created = existing.is_none();
            if created && st.peers.len() >= cfg.peers() {
                return r.err("tunnel: the hub has as many peers as it takes");
            }
            if let Some(pid) = existing {
                if st
                    .peers
                    .get(&pid)
                    .is_some_and(|p| p.links.len() >= cfg.links_per_peer())
                {
                    return r.err("tunnel: the peer has as many links as the hub takes");
                }
            }
            if l.is_closed() {
                return r.err("link: closed");
            }
            let pid = match existing {
                Some(p) => p,
                None => {
                    st.next_peer = st.next_peer.wrapping_add(1).max(1);
                    let id = st.next_peer;
                    st.peers.insert(
                        id,
                        Peer {
                            id,
                            info: req.info.clone(),
                            addr: l.remote(),
                            since: akit_roles::linktoken::unix_now(),
                            links: BTreeMap::new(),
                            rtt: Stats::default(),
                            service_rtt: String::new(),
                            by_token,
                        },
                    );
                    st.by_name.insert(key.clone(), id);
                    id
                }
            };
            st.next_conn = st.next_conn.wrapping_add(1).max(1);
            let conn = st.next_conn;
            let Some(peer) = st.peers.get_mut(&pid) else {
                return r.err("tunnel: the peer is gone");
            };
            let mut index = 0i64;
            while peer.links.contains_key(&index) {
                index = index.saturating_add(1);
            }
            peer.links.insert(index, l.id);
            Self::registered(st, l.id);
            if let Some(rec) = st.links.get_mut(&l.id) {
                rec.reg = Some(Reg {
                    peer: pid,
                    conn,
                    ready: false,
                });
                rec.token_use = token_use;
            }
            (
                RegisterRsp {
                    peer: pid,
                    conn,
                    index,
                    hub: cfg.hub_name(),
                    proof: String::new(),
                },
                created,
            )
        };
        l.set_max_frame(crate::link::MAX_FRAME);
        if created {
            akit_observe::info!(
                Tunnel,
                "tunnel.hub: {} {:?} registered from {} as peer {}, with {}",
                req.info.kind,
                req.info.name,
                l.remote(),
                rsp.peer,
                if by_token {
                    format!("token {}", req.token)
                } else {
                    "the shared password".to_owned()
                }
            );
        }
        prove(&mut rsp);
        r.ok(rsp.json().as_bytes());
        if let Some(rec) = lock(&self.st).links.get_mut(&l.id) {
            if let Some(reg) = rec.reg.as_mut() {
                reg.ready = true;
            }
        }
        if created && req.info.kind == msg::TYPE_NODE {
            self.broadcast_nodes();
        }
    }

    fn peer_of(&self, l: &Link) -> Option<(u32, Info, u32)> {
        let st = lock(&self.st);
        let reg = st.links.get(&l.id)?.reg.as_ref()?;
        let p = st.peers.get(&reg.peer)?;
        Some((p.id, p.info.clone(), reg.conn))
    }

    fn update(self: &Arc<Self>, l: &Arc<Link>, body: &[u8], r: Responder) {
        let Some(info) = msg::parse(body).map(|v| Info::from(&v)) else {
            return r.err("tunnel: a bad request");
        };
        let Some((pid, old, _)) = self.peer_of(l) else {
            return r.err("tunnel: register first");
        };
        if info.name != old.name || info.kind != old.kind {
            return r.err("tunnel: a peer cannot change its name or type");
        }
        if let Some(p) = lock(&self.st).peers.get_mut(&pid) {
            p.info = info.clone();
        }
        r.ok(b"\"ok\"");
        if info.kind == msg::TYPE_NODE {
            self.broadcast_nodes();
        }
    }

    fn ping(&self, l: &Arc<Link>, body: &[u8], r: Responder) {
        let Some(v) = msg::parse(body) else {
            return r.err("tunnel: a bad request");
        };
        let Some((pid, info, _)) = self.peer_of(l) else {
            return r.err("tunnel: register first");
        };
        let mut st = lock(&self.st);
        if let Some(p) = st.peers.get_mut(&pid) {
            p.rtt = Stats::from(v.get("rtt"));
            p.service_rtt = match v.get("serviceRtt") {
                Some(sr @ Value::Object(_)) => sr.to_json().trim().to_owned(),
                _ => String::new(),
            };
        }
        let mut rsp = msg::Obj::new();
        if info.kind == msg::TYPE_PROXY {
            let mut nodes = msg::Obj::new();
            let mut ps: Vec<&Peer> = st
                .peers
                .values()
                .filter(|p| p.info.kind == msg::TYPE_NODE)
                .collect();
            ps.sort_by_key(|p| p.id);
            for p in ps {
                nodes = nodes.raw(&p.info.name, &latency(p));
            }
            rsp = rsp.raw("nodes", &nodes.end());
        }
        drop(st);
        r.ok(rsp.end().as_bytes());
    }

    fn node_list(&self, version: u64) -> String {
        let st = lock(&self.st);
        let mut ps: Vec<&Peer> = st
            .peers
            .values()
            .filter(|p| p.info.kind == msg::TYPE_NODE)
            .collect();
        ps.sort_by(|a, b| a.info.name.cmp(&b.info.name));
        let nodes = msg::array(&ps, |p| {
            let mut o = msg::Obj::new()
                .raw("info", &p.info.json())
                .str("addr", &p.addr)
                .str("since", &akit_roles::linktoken::rfc3339(p.since))
                .raw("rtt", &p.rtt.json());
            if !p.service_rtt.is_empty() {
                o = o.raw("serviceRtt", &p.service_rtt);
            }
            o.end()
        });
        msg::Obj::new()
            .str("hub", &lock(&self.cfg).hub_name())
            .raw("nodes", &nodes)
            .num_omit("epoch", i64::try_from(self.epoch).unwrap_or(0))
            .num_omit("version", i64::try_from(version).unwrap_or(0))
            .end()
    }

    fn broadcast_nodes(&self) {
        let version = {
            let mut st = lock(&self.st);
            st.nodes_version = st.nodes_version.wrapping_add(1);
            st.nodes_version
        };
        let list = self.node_list(version);
        let targets: Vec<Arc<Link>> = {
            let st = lock(&self.st);
            st.peers
                .values()
                .filter(|p| p.info.kind == msg::TYPE_PROXY)
                .filter_map(|p| {
                    p.links.values().find_map(|lid| {
                        st.links
                            .get(lid)
                            .filter(|r| r.reg.as_ref().is_some_and(|g| g.ready))
                            .map(|r| Arc::clone(&r.link))
                    })
                })
                .collect()
        };
        for l in targets {
            l.notify(method::NODES, list.as_bytes());
        }
    }

    /// The node's ready link carrying the fewest of its tunnels.
    fn pick_link(st: &HubState, node: u32) -> Option<Arc<Link>> {
        let p = st.peers.get(&node)?;
        let mut best: Option<(Arc<Link>, usize)> = None;
        for lid in p.links.values() {
            let Some(rec) = st.links.get(lid) else {
                continue;
            };
            if !rec.reg.as_ref().is_some_and(|g| g.ready) {
                continue;
            }
            let load = st
                .routes
                .values()
                .filter(|r| r.node == node && r.node_link.id == *lid)
                .count();
            if best.as_ref().is_none_or(|b| load < b.1) {
                best = Some((Arc::clone(&rec.link), load));
            }
        }
        best.map(|(l, _)| l)
    }

    fn load(st: &HubState, l: &Link) -> usize {
        st.routes
            .values()
            .filter(|r| r.proxy_link.id == l.id || r.node_link.id == l.id)
            .count()
    }

    fn open(self: &Arc<Self>, l: &Arc<Link>, body: &[u8], r: Responder) {
        let Some(mut req) = msg::parse(body).map(|v| TunnelOpenReq::from(&v)) else {
            return r.err("tunnel: a bad request");
        };
        let Some((proxy, info, conn)) = self.peer_of(l) else {
            return r.err("tunnel: register first");
        };
        if info.kind != msg::TYPE_PROXY {
            return r.err("tunnel: only a proxy opens tunnels");
        }
        if msg::tunnel_conn(req.id) != conn {
            return r.err("tunnel: a tunnel ID must carry its link's number");
        }
        let cfg = lock(&self.cfg).clone();
        let node_link = {
            let mut st = lock(&self.st);
            let Some(&node) = st.by_name.get(&format!("{}/{}", msg::TYPE_NODE, req.node)) else {
                return r.err(&format!("tunnel: no node {:?}", req.node));
            };
            let svc_addr = st
                .peers
                .get(&node)
                .and_then(|p| p.info.service(&req.service).map(|s| s.addr.clone()));
            let Some(svc_addr) = svc_addr else {
                return r.err(&format!(
                    "tunnel: node {:?} has no service {:?}",
                    req.node, req.service
                ));
            };
            if st.routes.contains_key(&req.id) {
                return r.err("tunnel: tunnel ID in use");
            }
            if st.routes.len() >= cfg.tunnels() {
                return r.err("tunnel: the hub passes as many tunnels as it takes");
            }
            if Self::load(&st, l) >= MAX_TUNNELS_PER_LINK {
                return r.err("tunnel: the link carries as many tunnels as it may");
            }
            let Some(nl) = Self::pick_link(&st, node) else {
                return r.err(&format!("tunnel: node {:?} disconnected", req.node));
            };
            if Self::load(&st, &nl) >= MAX_TUNNELS_PER_LINK {
                return r.err(&format!(
                    "tunnel: every link of node {:?} carries as many tunnels as it may",
                    req.node
                ));
            }
            st.routes.insert(
                req.id,
                Route {
                    proxy,
                    node,
                    proxy_link: Arc::clone(l),
                    node_link: Arc::clone(&nl),
                    open: true,
                    up_flight: 0,
                    down_flight: 0,
                    service: req.service.clone(),
                    addr: svc_addr,
                    created: akit_roles::linktoken::unix_now(),
                    up: 0,
                    down: 0,
                },
            );
            nl
        };
        req.proxy = info.name;
        let me = Arc::clone(self);
        let id = req.id;
        node_link.call(
            method::TUNNEL_OPEN,
            req.json().as_bytes(),
            cfg.call(),
            None,
            Box::new(move |res| match res {
                Ok(b) => {
                    let addr = msg::parse(&b)
                        .map(|v| msg::get_str(&v, "addr"))
                        .unwrap_or_default();
                    let open = {
                        let mut st = lock(&me.st);
                        match st.routes.get_mut(&id) {
                            Some(x) => {
                                if !addr.is_empty() {
                                    x.addr = addr;
                                }
                                x.open
                            }
                            None => false,
                        }
                    };
                    if open {
                        r.ok(&b);
                    } else {
                        r.err("tunnel: tunnel closed while opening");
                    }
                }
                Err(e) => {
                    lock(&me.st).routes.remove(&id);
                    r.err(&e.to_string());
                }
            }),
        );
    }

    fn set_node_services(self: &Arc<Self>, l: &Arc<Link>, body: &[u8], r: Responder) {
        let Some((_, info, _)) = self.peer_of(l) else {
            return r.err("tunnel: register first");
        };
        if info.kind != msg::TYPE_PROXY || !lock(&self.cfg).allow_proxy_services {
            return r.err(
                "tunnel: the hub does not let proxies set nodes' services (allow_proxy_services)",
            );
        }
        let Some(v) = msg::parse(body) else {
            return r.err("tunnel: a bad request");
        };
        let node = msg::get_str(&v, "node");
        let svcs = msg::services(v.get("services"));
        let target = {
            let st = lock(&self.st);
            st.by_name
                .get(&format!("{}/{node}", msg::TYPE_NODE))
                .and_then(|id| Self::pick_link(&st, *id))
        };
        let Some(nl) = target else {
            return r.err(&format!("tunnel: no node {node:?}"));
        };
        let me = Arc::clone(self);
        nl.call(
            method::NODE_SET_SERVICES,
            msg::array(&svcs, msg::Service::json).as_bytes(),
            lock(&self.cfg).call(),
            None,
            Box::new(move |res| match res {
                Ok(b) => {
                    if let Some(info) = msg::parse(&b).map(|v| Info::from(&v)) {
                        let key = kind_name(&info);
                        let mut st = lock(&me.st);
                        if let Some(id) = st.by_name.get(&key).copied() {
                            if let Some(p) = st.peers.get_mut(&id) {
                                p.info = info;
                            }
                        }
                    }
                    r.ok(&b);
                    me.broadcast_nodes();
                }
                Err(e) => r.err(&e.to_string()),
            }),
        );
    }

    /// Passes a tunnel frame on to the other end, checking the window of the end it came from.
    fn forward(&self, from: &Arc<Link>, m: u8, body: &[u8]) {
        let Some((id, payload)) = msg::parse_frame(body) else {
            return;
        };
        let mut st = lock(&self.st);
        let Some(r) = st.routes.get_mut(&id) else {
            return;
        };
        let up = if from.id == r.proxy_link.id {
            true
        } else if from.id == r.node_link.id {
            false
        } else {
            return;
        };
        let to = Arc::clone(if up { &r.node_link } else { &r.proxy_link });
        let n = i64::try_from(payload.len()).unwrap_or(i64::MAX);
        let breach = match m {
            method::TUNNEL_DATA => {
                let sent = if up {
                    &mut r.up_flight
                } else {
                    &mut r.down_flight
                };
                *sent = sent.saturating_add(n);
                let over = *sent > MAX_IN_FLIGHT;
                let passed = if up { &mut r.up } else { &mut r.down };
                *passed = passed.saturating_add(n);
                over.then_some("sent past its window")
            }
            method::TUNNEL_ACK => match msg::parse_ack(payload) {
                Some(k) if k > 0 => {
                    let acked = if up {
                        &mut r.down_flight
                    } else {
                        &mut r.up_flight
                    };
                    *acked = acked.saturating_sub(k);
                    (*acked < 0).then_some("acknowledged what it was not sent")
                }
                _ => Some("acknowledged what it was not sent"),
            },
            _ => None,
        };
        if let Some(what) = breach {
            let end = if up { "proxy" } else { "node" };
            let reason = format!("the {end} end {what}");
            let r = st.routes.remove(&id);
            drop(st);
            akit_observe::warn!(Tunnel, "tunnel.hub: tunnel {id}: {reason}; closing it");
            if let Some(r) = r {
                r.proxy_link
                    .send_tunnel(method::TUNNEL_CLOSE, id, reason.as_bytes());
                r.node_link
                    .send_tunnel(method::TUNNEL_CLOSE, id, reason.as_bytes());
            }
            return;
        }
        if m == method::TUNNEL_CLOSE {
            st.routes.remove(&id);
        }
        drop(st);
        to.notify(m, body);
    }

    fn disconnected(&self, l: &Arc<Link>) {
        let (routes, gone, info) = {
            let mut st = lock(&self.st);
            Self::registered(&mut st, l.id);
            let Some(rec) = st.links.remove(&l.id) else {
                return;
            };
            drop(rec.token_use);
            let Some(reg) = rec.reg else {
                return;
            };
            let gone = match st.peers.get_mut(&reg.peer) {
                Some(p) => {
                    p.links.retain(|_, lid| *lid != l.id);
                    p.links.is_empty()
                }
                None => false,
            };
            let ids: Vec<u64> = st
                .routes
                .iter()
                .filter(|(_, r)| r.proxy_link.id == l.id || r.node_link.id == l.id)
                .map(|(id, _)| *id)
                .collect();
            let routes: Vec<(u64, Route)> = ids
                .into_iter()
                .filter_map(|id| st.routes.remove(&id).map(|r| (id, r)))
                .collect();
            let info = st
                .peers
                .get(&reg.peer)
                .map(|p| p.info.clone())
                .unwrap_or_default();
            if gone {
                st.peers.remove(&reg.peer);
                st.by_name.remove(&kind_name(&info));
            }
            (routes, gone, info)
        };
        let reason = if gone {
            format!("{} {} disconnected", info.kind, info.name)
        } else {
            format!("a link of {} {} closed", info.kind, info.name)
        };
        if gone {
            akit_observe::info!(
                Tunnel,
                "tunnel.hub: {} {:?} disconnected, closing {} tunnels",
                info.kind,
                info.name,
                routes.len()
            );
        }
        for (id, r) in routes {
            let other = if r.node_link.id == l.id {
                &r.proxy_link
            } else {
                &r.node_link
            };
            other.send_tunnel(method::TUNNEL_CLOSE, id, reason.as_bytes());
            let _ = (r.proxy, r.node);
        }
        if gone && info.kind == msg::TYPE_NODE {
            self.broadcast_nodes();
        }
    }

    /// The peers that registered with the shared password, as the pages warn of.
    #[must_use]
    pub(crate) fn password_peers(&self) -> Vec<String> {
        lock(&self.st)
            .peers
            .values()
            .filter(|p| !p.by_token)
            .map(|p| kind_name(&p.info))
            .collect()
    }
}

fn latency(p: &Peer) -> String {
    let mut o = msg::Obj::new().raw("rtt", &p.rtt.json());
    if !p.service_rtt.is_empty() {
        o = o.raw("serviceRtt", &p.service_rtt);
    }
    o.end()
}

impl Router for HubRouter {
    fn guard(&self, l: &Arc<Link>, m: u8) -> Result<(), String> {
        let Some(h) = self.0.upgrade() else {
            return Err("tunnel: the hub stopped".to_owned());
        };
        if m == method::REGISTER || m == method::CHALLENGE {
            return Ok(());
        }
        let registered = lock(&h.st)
            .links
            .get(&l.id)
            .is_some_and(|r| r.reg.is_some());
        if registered {
            Ok(())
        } else {
            Err("tunnel: register first".to_owned())
        }
    }

    fn call(&self, l: &Arc<Link>, m: u8, body: &[u8], r: Responder) {
        let Some(h) = self.0.upgrade() else {
            return r.err("tunnel: the hub stopped");
        };
        match m {
            method::CHALLENGE => h.challenge(l, body, r),
            method::REGISTER => h.register(l, body, r),
            method::UPDATE => h.update(l, body, r),
            method::PING => h.ping(l, body, r),
            method::NODES => {
                let v = lock(&h.st).nodes_version;
                r.ok(h.node_list(v).as_bytes());
            }
            method::SET_NODE_SERVICES => h.set_node_services(l, body, r),
            method::TUNNEL_OPEN => h.open(l, body, r),
            _ => r.err(&format!("link: no method {m}")),
        }
    }

    fn notify(&self, l: &Arc<Link>, m: u8, body: &[u8]) {
        let Some(h) = self.0.upgrade() else { return };
        if matches!(
            m,
            method::TUNNEL_DATA | method::TUNNEL_ACK | method::TUNNEL_CLOSE
        ) {
            h.forward(l, m, body);
        }
    }

    fn closed(&self, l: &Arc<Link>, _err: &str) {
        if let Some(h) = self.0.upgrade() {
            h.disconnected(l);
        }
    }
}

/// The hub role.
pub struct Hub {
    core: Arc<HubCore>,
    /// Its module of the pages (I109), made once, so that its consoles keep serving it.
    module: akit_roles::pages::ModuleRef,
}

impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hub").finish_non_exhaustive()
    }
}

/// Binds `member` to token `id` in the hub's member of the config (fit's `Bind`), as the
/// registry asks when a member first links with a token.
fn bind_in(member_value: &mut Value, id: &str, member: &str) -> Result<(), String> {
    let Value::Object(ms) = member_value else {
        return Err("tunnel: the hub's member is not an object".to_owned());
    };
    let Some((_, Value::Array(tokens))) = ms.iter_mut().find(|(k, _)| k == "tokens") else {
        return Ok(());
    };
    for t in tokens.iter_mut() {
        let Value::Object(fields) = t else { continue };
        if !fields
            .iter()
            .any(|(k, v)| k == "id" && *v == Value::String(id.to_owned()))
        {
            continue;
        }
        match fields.iter_mut().find(|(k, _)| k == "members") {
            Some((_, Value::Array(m))) => {
                if !m.contains(&Value::String(member.to_owned()))
                    && m.len() < akit_config::roles::MAX_LINK_TOKEN_MEMBERS
                {
                    m.push(Value::String(member.to_owned()));
                }
            }
            _ => fields.push((
                "members".to_owned(),
                Value::Array(vec![Value::String(member.to_owned())]),
            )),
        }
    }
    Ok(())
}

/// Makes the hub role.
///
/// # Errors
///
/// None: the first apply starts it.
pub fn new(host: Host) -> Result<Box<dyn Role>, String> {
    let edit_host = host.clone();
    let on_bind: akit_roles::linktoken::OnBind = Arc::new(move |id: &str, member: &str| {
        let (id, member) = (id.to_owned(), member.to_owned());
        edit_host.edit(
            Box::new(move |m| bind_in(m, &id, &member)),
            Box::new(|r| {
                if let Err(e) = r {
                    akit_observe::warn!(
                        Tunnel,
                        "tunnel.hub: a token's member was not written: {e}"
                    );
                }
            }),
        );
    });
    let mut epoch = [0u8; 8];
    let _ = akio_crypto::rand::fill_random(&mut epoch);
    let core = Arc::new_cyclic(|me| HubCore {
        host,
        cfg: Mutex::new(TunnelHub::default()),
        tokens: Registry::new(Some(on_bind), Arc::new(akit_roles::linktoken::unix_now)),
        verifier: Mutex::new(None),
        coder: Mutex::new(None),
        listeners: Mutex::new(Vec::new()),
        files: Mutex::new(Vec::new()),
        generated: Mutex::new(None),
        access: Mutex::new(None),
        fingerprint: Mutex::new(String::new()),
        st: Mutex::new(HubState::default()),
        epoch: u64::from_be_bytes(epoch) >> 1,
        started: akit_roles::linktoken::unix_now(),
        sweep_gen: std::sync::atomic::AtomicU64::new(0),
        socket: Mutex::new(None),
        me: me.clone(),
    });
    let module: akit_roles::pages::ModuleRef = Arc::new(pages::HubPages(Arc::downgrade(&core)));
    Ok(Box::new(Hub { core, module }))
}

/// Closes the links of tokens that expired, at their expiry (fit's registry's timer): scheduled
/// for the next expiry, a minute at most, and again each time the tokens change, which supersedes
/// the sweep scheduled before.
fn sweep(core: &Arc<HubCore>) {
    use std::sync::atomic::Ordering;
    let generation = core
        .sweep_gen
        .fetch_add(1, Ordering::SeqCst)
        .wrapping_add(1);
    arm_sweep(core, generation);
}

fn arm_sweep(core: &Arc<HubCore>, generation: u64) {
    use std::sync::atomic::Ordering;
    let weak = Arc::downgrade(core);
    let every = core
        .tokens
        .next_sweep()
        .unwrap_or(akit_roles::linktoken::MAX_SWEEP)
        .max(Duration::from_millis(200));
    let _ = akio::timer::after_send(every, move || {
        if let Some(c) = weak.upgrade() {
            if c.sweep_gen.load(Ordering::SeqCst) != generation {
                return;
            }
            c.tokens.sweep();
            arm_sweep(&c, generation);
        }
    });
}

/// A TLS, WSS or QUIC listener's certificate: its own, from its files, or the hub's. Reads its
/// files each time the member is applied (fit's), noting them with what was read in `files`: the
/// control thread's.
fn listener_cert(
    l: &TunnelListener,
    own: Option<&Arc<(String, String)>>,
    files: &mut Vec<(String, [u8; 32])>,
) -> Result<Arc<(String, String)>, String> {
    if l.cert_file.is_empty() {
        return own.cloned().ok_or_else(|| "no certificate".to_owned());
    }
    let cert = std::fs::read_to_string(&l.cert_file);
    let key = std::fs::read_to_string(&l.key_file);
    files.push((l.cert_file.clone(), file_sum(&cert)));
    files.push((l.key_file.clone(), file_sum(&key)));
    let (cert, key) = (
        cert.map_err(|e| format!("{}: {e}", l.cert_file))?,
        key.map_err(|e| format!("{}: {e}", l.key_file))?,
    );
    Ok(Arc::new((cert, key)))
}

/// The TLS application protocol of a listener: `http/1.1` for `wss`, `fit-tunnel` otherwise.
fn listener_alpn(l: &TunnelListener) -> &'static str {
    if l.protocol.is_ws() {
        crate::ws::ALPN
    } else {
        crate::tls::ALPN
    }
}

impl Hub {
    /// How a WebSocket listener takes a link: as the TCP ones, by the hub's bounds.
    fn ws_accept(&self) -> crate::ws::Accept {
        let me = self.core.me.clone();
        Arc::new(move |peer, wire| me.upgrade().and_then(|h| h.accept_wire(peer, wire)))
    }

    /// A QUIC listener: a UDP socket bound on the engine's UDP threads, served by akio-quic.
    fn bind_quic(
        &self,
        tls: &akio::tls::ServerConfig,
        addr: SocketAddr,
    ) -> Result<akio::UdpBinding, String> {
        let c = &self.core;
        let cfg = lock(&c.cfg).clone();
        let me = c.me.clone();
        let accept: crate::quic::Accept =
            Arc::new(move |peer, wire| me.upgrade().and_then(|h| h.accept_wire(peer, wire)));
        let max = cfg
            .links()
            .saturating_add(akit_config::roles::TunnelHubQuic::handshakes(
                cfg.quic.as_ref(),
            ));
        let server = crate::quic::server(tls, cfg.quic.as_ref(), max, accept)?;
        // The access policy at akio's filter from the first datagram (ADR-178): a refused peer's
        // packets are dropped before a QUIC connection, or any TLS work, is made for it.
        let bound = match lock(&c.access).clone() {
            Some(p) => c
                .host
                .engine
                .bind_udp_filtered(addr, server, move |peer: &SocketAddr| p.allows(peer.ip())),
            None => c.host.engine.bind_udp(addr, server),
        };
        bound.map_err(|e| e.to_string())
    }

    /// Opens the listeners `want` adds and closes those it leaves out; the rest stay open. A
    /// listener with TLS that stays serves its certificate as read now: its files', or the hub's
    /// own, replaced in place when they changed (I102d); files that do not load leave what it
    /// serves. The files read, with `files` (the hub's own certificate's), are noted for
    /// `-watch-certs`. Why each that failed did is returned.
    fn move_listeners(
        &self,
        want: &[TunnelListener],
        own: Option<&Arc<(String, String)>>,
        mut files: Vec<(String, [u8; 32])>,
    ) -> Vec<String> {
        let c = &self.core;
        let engine = &c.host.engine;
        let mut errs = Vec::new();
        let mut have = lock(&c.listeners);
        let mut next = Vec::new();
        for l in want {
            let pem = l
                .protocol
                .takes_tls()
                .then(|| listener_cert(l, own, &mut files));
            if let Some(i) = have.iter().position(|o| o.cfg == *l) {
                let mut o = have.remove(i);
                if let (Some(served), Some(pem)) = (o.cert.as_mut(), pem) {
                    match pem.and_then(|p| served.replace(p)) {
                        Ok(false) => {}
                        Ok(true) => akit_observe::info!(
                            Tunnel,
                            "tunnel.hub: listener {} {}: certificate changed; serving the new one, fingerprint {}",
                            l.protocol.as_str(),
                            l.address,
                            served.fp
                        ),
                        Err(e) => errs.push(format!(
                            "listener {} {}: the certificate: {e}; the running one stays",
                            l.protocol.as_str(),
                            l.address
                        )),
                    }
                }
                next.push(o);
                continue;
            }
            let Some(addr) = crate::proxy::listen_addr(&l.address) else {
                errs.push(format!(
                    "listener {}: not an address to listen on",
                    l.address
                ));
                continue;
            };
            // The TLS of one that takes it, with the certificate it serves.
            let (cert, tls) = match pem {
                None => (None, None),
                Some(pem) => match pem.and_then(|p| Served::new(p, listener_alpn(l))) {
                    Ok((served, cfg)) => (Some(served), Some(cfg)),
                    Err(e) => {
                        errs.push(format!(
                            "listener {} {}: the certificate: {e}",
                            l.protocol.as_str(),
                            l.address
                        ));
                        continue;
                    }
                },
            };
            let me = c.me.clone();
            let acceptor =
                akio::fn_acceptor(move |conn: &akio::Conn<LinkHandler>| match me.upgrade() {
                    Some(h) => h.accept(conn),
                    None => {
                        conn.close();
                        LinkHandler(None)
                    }
                });
            let mut el = match (l.protocol, tls) {
                (TunnelProtocol::Tcp, _) => akio::Listener::tcp(addr).threads(1).serve(acceptor),
                (TunnelProtocol::Tls, Some(cfg)) => akio::Listener::tcp(addr)
                    .threads(1)
                    .serve(akio::tls::Tls::server(cfg, acceptor)),
                (TunnelProtocol::Quic, Some(cfg)) => {
                    match self.bind_quic(&cfg, addr) {
                        Ok(b) => next.push(Open {
                            cfg: l.clone(),
                            bound: Bound::Udp(b),
                            cert,
                        }),
                        Err(e) => errs.push(format!("listener quic {}: {e}", l.address)),
                    }
                    continue;
                }
                (TunnelProtocol::Ws, _) => {
                    akio::Listener::tcp(addr)
                        .threads(1)
                        .serve(akio_http::serve_with(
                            crate::ws::http_config(),
                            crate::ws::service(l.ws_path().to_owned(), self.ws_accept()),
                        ))
                }
                (TunnelProtocol::Wss, Some(cfg)) => {
                    akio::Listener::tcp(addr)
                        .threads(1)
                        .serve(akio::tls::Tls::server(
                            cfg,
                            akio_http::serve_with(
                                crate::ws::http_config(),
                                crate::ws::service(l.ws_path().to_owned(), self.ws_accept()),
                            ),
                        ))
                }
                (TunnelProtocol::Tls | TunnelProtocol::Quic | TunnelProtocol::Wss, None) => {
                    errs.push(format!("listener {}: no certificate", l.address));
                    continue;
                }
            };
            if let Some(p) = lock(&c.access).clone() {
                el = el.filter(move |peer: &SocketAddr| p.allows(peer.ip()));
            }
            match engine.add_listener(el) {
                Ok(id) => next.push(Open {
                    cfg: l.clone(),
                    bound: Bound::Tcp(id),
                    cert,
                }),
                Err(e) => errs.push(format!("listener {}: {e}", l.address)),
            }
        }
        for o in have.drain(..) {
            o.bound.remove(engine);
        }
        *have = next;
        *lock(&c.files) = files;
        errs
    }
}

impl Role for Hub {
    fn apply(&mut self, config: &Config) -> Result<(), String> {
        let Some(cfg) = config.tunnel.as_ref().and_then(|t| t.hub.clone()) else {
            return Ok(());
        };
        let c = &self.core;
        let mut errs = Vec::new();
        match crate::proxy::policy(cfg.access.as_ref(), config.access.as_ref()) {
            Ok(p) => {
                for Open { bound: b, .. } in lock(&c.listeners).iter() {
                    // Peers accepted before are kept; new ones go by the new policy.
                    let filter = p
                        .clone()
                        .map(|p| move |peer: &SocketAddr| -> bool { p.allows(peer.ip()) });
                    let _ = match (b, filter) {
                        (Bound::Tcp(id), Some(f)) => c.host.engine.set_listener_filter(*id, f),
                        (Bound::Tcp(id), None) => c.host.engine.clear_listener_filter(*id),
                        (Bound::Udp(u), Some(f)) => u.set_filter(f),
                        (Bound::Udp(u), None) => u.clear_filter(),
                    };
                }
                // The console socket's too (I103b).
                if let Some(s) = lock(&c.socket).as_ref() {
                    let _ = match p.clone() {
                        Some(p) => c
                            .host
                            .engine
                            .set_listener_filter(s.id, move |peer: &SocketAddr| {
                                p.allows(peer.ip())
                            }),
                        None => c.host.engine.clear_listener_filter(s.id),
                    };
                }
                *lock(&c.access) = p;
            }
            Err(e) => errs.push(e),
        }
        c.tokens.set(&cfg.tokens);
        sweep(c);
        *lock(&c.verifier) = (!cfg.password.is_empty()).then(|| Verifier::new(&cfg.password));
        let tokens = c.tokens.clone();
        let body_password = cfg.body_password.clone();
        *lock(&c.coder) = Coder::new(&cfg.body_password).map(|cd| {
            cd.with_tokens(Arc::new(move |id: &str| {
                let stored = tokens.lookup(id).map_err(|e| e.to_string())?;
                Ok(Coder::token(
                    id,
                    &akit_roles::linktoken::body_key(&stored),
                    &body_password,
                ))
            }))
        });
        let listeners = cfg.listeners.clone();
        // The hub's own certificate, for its TLS listeners without one of their own (I102): made
        // once and kept beside the config, so that its fingerprint, which peers pin, outlives a
        // restart.
        let needs_own = listeners
            .iter()
            .any(|l| l.protocol.takes_tls() && l.cert_file.is_empty());
        // The files read for the listeners' certificates, the kept one's too, for -watch-certs.
        let mut files = Vec::new();
        let own = if needs_own {
            let kept = c.host.kept_file("pem");
            let in_config = cfg.quic.as_ref().is_some_and(|q| !q.cert.is_empty());
            let generated = lock(&c.generated).clone();
            let r = match (&kept, generated) {
                // Made when there is nowhere to keep it: the same while akit runs.
                (None, Some(g)) if !in_config => Ok((g, false)),
                _ => crate::tls::hub_cert(&cfg, kept.as_deref()),
            };
            if let (Some(path), false) = (&kept, in_config) {
                // Read again by each apply, as the listeners' own files.
                files.push((path.clone(), file_sum(&std::fs::read_to_string(path))));
            }
            match r {
                Ok((cert, made)) => {
                    if made && kept.is_none() && !in_config {
                        *lock(&c.generated) = Some(Arc::clone(&cert));
                    }
                    let fp = crate::tls::fingerprint(&cert.0);
                    if made {
                        match &kept {
                            Some(path) => akit_observe::info!(
                                Tunnel,
                                "tunnel.hub: made a certificate, kept in {path}; its fingerprint, which nodes and proxies pin: {fp}"
                            ),
                            None => akit_observe::warn!(
                                Tunnel,
                                "tunnel.hub: made a certificate that is kept nowhere: it changes at the next start; its fingerprint: {fp}"
                            ),
                        }
                    } else {
                        akit_observe::info!(Tunnel, "tunnel.hub: certificate fingerprint {fp}");
                    }
                    *lock(&c.fingerprint) = fp;
                    Some(cert)
                }
                Err(e) => {
                    errs.push(format!("the hub's certificate: {e}"));
                    None
                }
            }
        } else {
            None
        };
        *lock(&c.cfg) = cfg.clone();
        errs.extend(self.move_listeners(&listeners, own.as_ref(), files));
        // The console socket (I103b), which carries the console the config names.
        if let Err(e) = c.apply_socket(&cfg) {
            errs.push(e);
        }
        if !c.password_peers().is_empty() {
            akit_observe::debug!(
                Tunnel,
                "tunnel.hub: peers registered with the shared password"
            );
        }
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs.join("; "))
        }
    }

    fn stop(&mut self, _grace: Duration) {
        let c = &self.core;
        lock(&c.st).closing = true;
        // The sweep scheduled ends at its time.
        c.sweep_gen
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        for o in lock(&c.listeners).drain(..) {
            o.bound.remove(&c.host.engine);
        }
        c.stop_socket();
        let links: Vec<Arc<Link>> = lock(&c.st)
            .links
            .values()
            .map(|r| Arc::clone(&r.link))
            .collect();
        for l in links {
            l.close();
        }
    }

    fn tokens(&self) -> Option<Registry> {
        Some(self.core.tokens.clone())
    }

    fn files(&self) -> Vec<(String, [u8; 32])> {
        lock(&self.core.files).clone()
    }

    fn api(&self) -> Option<akit_roles::host::Api> {
        Some(Arc::new(HubApi(Arc::clone(&self.core))))
    }

    fn module(&self) -> Option<akit_roles::pages::ModuleRef> {
        Some(Arc::clone(&self.module))
    }
}
