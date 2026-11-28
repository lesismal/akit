//! `tunnel.proxy` (fit's `tunnel/proxy`): it links to its hubs, learns their nodes and services,
//! and listens on a local address per forward, each connection there a tunnel through a hub to a
//! node's service. A connection is not read until its tunnel is open. Its web proxies (I103b,
//! [`web`]) serve hubs' console pages through their console sockets.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use akit_config::Config;
use akit_config::roles::{TunnelForward, TunnelProxy};
use akit_config::value::Value;
use akit_roles::host::{Host, Role};

use crate::end::{EndHandler, Limit, Table};
use crate::link::{Link, Responder, Router};
use crate::msg::{self, Info, MAX_TUNNELS_PER_LINK, Stats, TunnelOpenReq, method};
use crate::uplink::{Owner, STATE_REGISTERED, Uplink};

mod pages;
pub mod pick;
pub mod web;
pub use web::WebProxyView;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A local address to listen on: `:port` every address, `localhost` the loopback one.
pub(crate) fn listen_addr(a: &str) -> Option<SocketAddr> {
    if let Ok(s) = a.parse() {
        return Some(s);
    }
    let (host, port) = a.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let ip = match host.trim_start_matches('[').trim_end_matches(']') {
        "" => std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
        h if h.eq_ignore_ascii_case("localhost") => {
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        }
        h => h.parse().ok()?,
    };
    Some(SocketAddr::new(ip, port))
}

/// The policy of a role's own `access`, then the config's global one.
pub(crate) fn policy(
    own: Option<&akit_config::Access>,
    global: Option<&akit_config::Access>,
) -> Result<Option<Arc<akit_access::Policy>>, String> {
    let err = |es: Vec<akit_access::Error>| {
        es.iter()
            .map(|e| format!("access.{}: {}", e.field, e.msg))
            .collect::<Vec<_>>()
            .join("; ")
    };
    let own = akit_access::compile(own, None).map_err(err)?;
    let global = akit_access::compile(global, None).map_err(err)?;
    Ok(akit_access::Policy::chain([own.as_ref(), global.as_ref()]))
}

/// A forward: listening at `addr`, or not, with why. Its target may change while it keeps its
/// listener (fit's).
struct Forward {
    cfg: Arc<Mutex<TunnelForward>>,
    id: Option<akio::ListenerId>,
    addr: String,
    err: String,
}

/// A node of a hub, as the hub lists it (fit's `proto.NodeView`).
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct NodeView {
    pub info: Info,
    pub addr: String,
    pub since: String,
    /// The round trip between the hub and the node.
    pub rtt: Stats,
    /// By service name, how long the node's connects took, as JSON; `""` for none.
    pub service_rtt: String,
}

impl NodeView {
    fn latency(&mut self, v: Option<&Value>) {
        self.rtt = Stats::from(v.and_then(|v| v.get("rtt")));
        self.service_rtt = match v.and_then(|v| v.get("serviceRtt")) {
            Some(sr @ Value::Object(o)) if !o.is_empty() => sr.to_json().trim().to_owned(),
            _ => String::new(),
        };
    }

    /// As the pages show it (fit's `proxy.NodeView`), with its `share`.
    fn view(&self, share: f64) -> String {
        let services = msg::array(&self.info.services, crate::pages::service_view);
        let mut o = msg::Obj::new()
            .str("name", &self.info.name)
            .str_omit("version", &self.info.version)
            .str_omit("hostname", &self.info.hostname)
            .str_omit("os", &self.info.os)
            .raw("services", &services);
        if !self.info.meta.is_empty() {
            let mut m = msg::Obj::new();
            for (k, v) in &self.info.meta {
                m = m.str(k, v);
            }
            o = o.raw("meta", &m.end());
        }
        o = o.str("addr", &self.addr).raw("rtt", &self.rtt.json());
        if !self.service_rtt.is_empty() {
            o = o.raw("service_rtt", &self.service_rtt);
        }
        o.float("share", share).end()
    }
}

/// A hub's nodes (fit's `proto.NodeList`): its epoch and the version of the list, so that the
/// newest of those it sends is kept.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct NodeList {
    pub hub: String,
    pub nodes: Vec<NodeView>,
    pub epoch: i64,
    pub version: i64,
}

impl NodeList {
    fn parse(body: &[u8]) -> Option<NodeList> {
        let v = msg::parse(body)?;
        let nodes = match v.get("nodes") {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|n| {
                    let info = Info::from(n.get("info")?);
                    let mut nv = NodeView {
                        info,
                        addr: msg::get_str(n, "addr"),
                        since: msg::get_str(n, "since"),
                        ..NodeView::default()
                    };
                    nv.latency(Some(n));
                    Some(nv)
                })
                .collect(),
            _ => Vec::new(),
        };
        Some(NodeList {
            hub: msg::get_str(&v, "hub"),
            nodes,
            epoch: msg::get_num(&v, "epoch"),
            version: msg::get_num(&v, "version"),
        })
    }

    /// Whether it is older than `cur`, a list from the same hub (fit's `Older`): both counted, in
    /// the same epoch, this one with the lower version.
    fn older(&self, cur: Option<&NodeList>) -> bool {
        cur.is_some_and(|c| {
            self.version != 0 && c.version != 0 && self.epoch == c.epoch && self.version < c.version
        })
    }
}

/// What a call on a hub ends in: its answer, or why not.
pub type CallDone = Box<dyn FnOnce(Result<String, String>) + Send>;

pub(crate) struct ProxyCore {
    host: Host,
    cfg: Mutex<TunnelProxy>,
    uplinks: Mutex<Vec<Arc<Uplink>>>,
    table: Arc<Table>,
    limit: Arc<Limit>,
    proven: Arc<Mutex<HashSet<u64>>>,
    /// Each hub's nodes, by the hub's name here.
    nodes: Mutex<HashMap<String, NodeList>>,
    forwards: Mutex<Vec<Forward>>,
    access: Mutex<Option<Arc<akit_access::Policy>>>,
    /// The web proxies (I103b).
    webs: web::WebProxies,
    me: Weak<ProxyCore>,
}

struct ProxyRouter(Weak<ProxyCore>);

impl ProxyCore {
    fn uplinks(&self) -> Vec<Arc<Uplink>> {
        lock(&self.uplinks).clone()
    }

    /// The tunnels through uplink `u`: those its registered links carry.
    fn on_uplink(&self, u: &Uplink) -> usize {
        u.registered_links()
            .iter()
            .map(|(l, _)| self.table.on_link(l))
            .sum()
    }

    /// The hub a tunnel of forward `f` goes through (fit's `pick`): the one `f.hub` names, or
    /// else, among the registered hubs that list the node with the service, the one
    /// [`pick::choose`] picks by their round trips and the tunnels they carry.
    fn pick(&self, f: &TunnelForward) -> Result<Arc<Uplink>, String> {
        let ups = self.uplinks();
        if !f.hub.is_empty() {
            return ups
                .iter()
                .find(|u| u.name == f.hub)
                .cloned()
                .ok_or_else(|| format!("no hub named {:?}", f.hub));
        }
        let candidates: Vec<(Arc<Uplink>, Stats)> = {
            let nodes = lock(&self.nodes);
            ups.iter()
                .filter(|u| u.is_registered())
                .filter_map(|u| {
                    let list = nodes.get(&u.name)?;
                    let p = list
                        .nodes
                        .iter()
                        .find(|p| p.info.name == f.node && p.info.service(&f.service).is_some())?;
                    Some((Arc::clone(u), p.rtt))
                })
                .collect()
        };
        if candidates.is_empty() {
            return Err(format!(
                "no hub has node {:?} with service {:?}",
                f.node, f.service
            ));
        }
        let paths: Vec<pick::Path> = candidates
            .iter()
            .map(|(u, node)| pick::Path {
                hub: u.rtt(),
                node: *node,
                tunnels: self.on_uplink(u),
            })
            .collect();
        let i = pick::choose(&paths);
        candidates
            .get(i)
            .map(|(u, _)| Arc::clone(u))
            .ok_or_else(|| "no path".to_owned())
    }

    /// The registered link of `u` carrying the fewest tunnels, under the most a link carries.
    fn link_of(&self, u: &Uplink) -> Result<(Arc<Link>, msg::RegisterRsp), String> {
        let links = u.registered_links();
        if links.is_empty() {
            return Err("not registered".to_owned());
        }
        links
            .into_iter()
            .map(|(l, r)| (self.table.on_link(&l), l, r))
            .filter(|(load, _, _)| *load < MAX_TUNNELS_PER_LINK)
            .min_by_key(|(load, _, _)| *load)
            .map(|(_, l, r)| (l, r))
            .ok_or_else(|| "every link carries the most tunnels a link may".to_owned())
    }

    /// A connection accepted on forward `f`: a tunnel opened for it, its reads held until then.
    fn accepted(self: &Arc<Self>, c: &akio::Conn<EndHandler>, f: &TunnelForward) -> EndHandler {
        let refuse = |why: &str| {
            akit_observe::warn!(
                Tunnel,
                "tunnel.proxy: forward {} refused a connection: {why}",
                f.listen
            );
            c.close();
            EndHandler(None)
        };
        let u = match self.pick(f) {
            Ok(u) => u,
            Err(e) => return refuse(&e),
        };
        let (l, rsp) = match self.link_of(&u) {
            Ok(x) => x,
            Err(e) => return refuse(&format!("hub {}: {e}", u.name)),
        };
        let id = msg::tunnel_id(rsp.conn, u.next_seq());
        let e = match self
            .table
            .add(&l, id, false, &f.node, &f.service, &f.listen)
        {
            Ok(e) => e,
            Err(err) => return refuse(err),
        };
        e.attach(c);
        let req = TunnelOpenReq {
            id,
            node: f.node.clone(),
            service: f.service.clone(),
            proxy: String::new(),
        };
        let e2 = Arc::clone(&e);
        l.call(
            method::TUNNEL_OPEN,
            req.json().as_bytes(),
            lock(&self.cfg).open(),
            None,
            Box::new(move |r| match r {
                Ok(body) => {
                    if let Some(v) = msg::parse(&body) {
                        e2.set_addr(&msg::get_str(&v, "addr"), 0.0);
                    }
                    e2.set_ready();
                }
                Err(err) => {
                    akit_observe::info!(
                        Tunnel,
                        "tunnel.proxy: opening a tunnel to {}/{}: {err}",
                        e2.node,
                        e2.service
                    );
                    e2.close(&format!("open: {err}"));
                }
            }),
        );
        EndHandler(Some(e))
    }

    /// Takes `list`, hub `u`'s nodes, unless `u` is no longer the proxy's or `list` is older than
    /// the one kept (fit's `setNodes`). The uplinks stay locked until it is taken, as fit's
    /// `setNodes` holds its lock: an apply that replaces or drops `u` meanwhile, and forgets the
    /// nodes of its hub, does so before or after, never between the check and the take.
    fn set_nodes(&self, u: &Arc<Uplink>, list: NodeList) {
        let ups = lock(&self.uplinks);
        if !ups.iter().any(|x| Arc::ptr_eq(x, u)) {
            return;
        }
        let mut nodes = lock(&self.nodes);
        if !list.older(nodes.get(&u.name)) {
            nodes.insert(u.name.clone(), list);
        }
    }

    /// Forgets hub `u`'s nodes once none of its links is registered, unless `u` is no longer the
    /// proxy's (its hub's nodes are then a newer uplink's). Under the uplinks' lock, as
    /// [`ProxyCore::set_nodes`]: a link of `u` that registers after the check takes the nodes
    /// after they are forgotten.
    fn forget_nodes(&self, u: &Arc<Uplink>) {
        let ups = lock(&self.uplinks);
        if ups.iter().any(|x| Arc::ptr_eq(x, u)) && u.registered_links().is_empty() {
            lock(&self.nodes).remove(&u.name);
        }
    }

    fn take_nodes(&self, u: &Arc<Uplink>, body: &[u8]) -> Option<NodeList> {
        let list = NodeList::parse(body)?;
        self.set_nodes(u, list.clone());
        Some(list)
    }

    fn uplink_of(&self, l: &Link) -> Option<Arc<Uplink>> {
        lock(&self.uplinks).iter().find(|u| u.owns(l)).cloned()
    }

    /// Calls `m` with `body` on hub `u`, over its registered link carrying the fewest tunnels.
    fn hub_call(
        u: &Uplink,
        m: u8,
        body: &[u8],
        done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
    ) {
        let Some((l, _)) = u.registered_links().into_iter().next() else {
            return done(Err("tunnel: not registered with the hub".to_owned()));
        };
        l.call(
            m,
            body,
            u.cfg.call(),
            None,
            Box::new(move |r| done(r.map_err(|e| e.to_string()))),
        );
    }

    /// Asks hub `u` for its nodes again, and takes them (fit's `RefreshNodes`): answered with
    /// the list.
    fn refresh_nodes(
        self: &Arc<Self>,
        u: &Arc<Uplink>,
        done: Box<dyn FnOnce(Result<NodeList, String>) + Send>,
    ) {
        let (me, u2) = (Arc::clone(self), Arc::clone(u));
        Self::hub_call(
            u,
            method::NODES,
            b"null",
            Box::new(move |r| {
                done(r.and_then(|b| {
                    me.take_nodes(&u2, &b)
                        .ok_or_else(|| "tunnel: the hub's answer is not JSON".to_owned())
                }));
            }),
        );
    }

    /// Has hub `u` set node `node`'s services (fit's `SetNodeServices`): the hub's nodes asked for
    /// again once it did.
    fn set_node_services(
        self: &Arc<Self>,
        u: &Arc<Uplink>,
        node: &str,
        services: &[msg::Service],
        done: Box<dyn FnOnce(Result<(), String>) + Send>,
    ) {
        let body = msg::Obj::new()
            .str("node", node)
            .raw("services", &msg::array(services, msg::Service::json))
            .end();
        let (me, u2) = (Arc::clone(self), Arc::clone(u));
        Self::hub_call(
            u,
            method::SET_NODE_SERVICES,
            body.as_bytes(),
            Box::new(move |r| match r {
                Ok(_) => me.refresh_nodes(&u2, Box::new(move |_| done(Ok(())))),
                Err(e) => done(Err(e)),
            }),
        );
    }

    /// The forwards as the pages show them (fit's `ForwardView`).
    fn forwards_json(&self) -> String {
        let fs = lock(&self.forwards);
        msg::array(&fs, |f| {
            let c = lock(&f.cfg).clone();
            msg::Obj::new()
                .str("listen", &c.listen)
                .str("node", &c.node)
                .str("service", &c.service)
                .str_omit("hub", &c.hub)
                .str_omit("addr", &f.addr)
                .str_omit("error", &f.err)
                .end()
        })
    }

    /// The proxy's state as its pages show it (fit's `proxy.Status`): each hub with its nodes, if
    /// registered, and their shares of the tunnels to them by round trip (fit's `Weights`).
    pub(crate) fn status(&self) -> String {
        let cfg = lock(&self.cfg).clone();
        let ups = self.uplinks();
        let ends = self.table.list();
        let nodes = lock(&self.nodes).clone();
        // The paths to each node, to weigh them against each other.
        let mut hubs: Vec<(String, Vec<&NodeView>)> = Vec::new();
        let mut paths: HashMap<&str, Vec<pick::Path>> = HashMap::new();
        for u in &ups {
            let n = ends.iter().filter(|e| u.owns(e.link())).count();
            let status = u.status_json(n);
            let (state, _, _) = u.status();
            let mut ns: Vec<&NodeView> = Vec::new();
            if state == STATE_REGISTERED {
                if let Some(list) = nodes.get(&u.name) {
                    let hub_rtt = u.rtt();
                    for p in &list.nodes {
                        ns.push(p);
                        paths
                            .entry(p.info.name.as_str())
                            .or_default()
                            .push(pick::Path {
                                hub: hub_rtt,
                                node: p.rtt,
                                tunnels: 0,
                            });
                    }
                }
            }
            hubs.push((status, ns));
        }
        let mut weights: HashMap<&str, std::collections::VecDeque<f64>> = paths
            .iter()
            .map(|(name, ps)| (*name, pick::weights(ps).into()))
            .collect();
        // The hubs in the same order again, so each takes its nodes' weights.
        let hub_views: Vec<String> = hubs
            .iter()
            .map(|(status, ns)| {
                let shares: Vec<String> = ns
                    .iter()
                    .map(|p| {
                        let share = weights
                            .get_mut(p.info.name.as_str())
                            .and_then(std::collections::VecDeque::pop_front)
                            .unwrap_or(0.0);
                        p.view(share)
                    })
                    .collect();
                let views = msg::array(&shares, Clone::clone);
                match status.strip_suffix('}') {
                    Some(head) => format!("{head},\"nodes\":{views}}}"),
                    None => status.clone(),
                }
            })
            .collect();
        let tunnels = msg::array(&ends, |e| e.view(&crate::pages::hub_of(&ups, e)));
        let webs = msg::array(&self.webs.views(), |w| {
            msg::Obj::new()
                .str_omit("name", &w.name)
                .str("listen", &w.listen)
                .str_omit("addr", &w.addr)
                .str("url", &w.url)
                .num("conns", w.conns)
                .str_omit("error", &w.error)
                .end()
        });
        let mut meta = msg::Obj::new();
        for (k, v) in &cfg.meta {
            meta = meta.str(k, v);
        }
        msg::Obj::new()
            .str("name", &cfg.proxy_name())
            .str("version", &msg::version())
            .raw("meta", &meta.end())
            .raw("hubs", &msg::array(&hub_views, Clone::clone))
            .raw("forwards", &self.forwards_json())
            .raw("web_proxies", &webs)
            .raw("tunnels", &tunnels)
            .num("open", self.limit.open())
            .num("max_tunnels", cfg.tunnels())
            .end()
    }
}

impl Router for ProxyRouter {
    fn guard(&self, l: &Arc<Link>, _m: u8) -> Result<(), String> {
        let Some(p) = self.0.upgrade() else {
            return Err("tunnel: the proxy stopped".to_owned());
        };
        if lock(&p.proven).contains(&l.id) {
            Ok(())
        } else {
            Err("tunnel: the hub has not proven itself on this link yet".to_owned())
        }
    }

    fn call(&self, _l: &Arc<Link>, m: u8, _body: &[u8], r: Responder) {
        r.err(&format!("link: no method {m}"));
    }

    fn notify(&self, l: &Arc<Link>, m: u8, body: &[u8]) {
        let Some(p) = self.0.upgrade() else { return };
        match m {
            method::TUNNEL_DATA | method::TUNNEL_ACK | method::TUNNEL_CLOSE => {
                p.table.frame(l, m, body);
            }
            method::NODES => {
                if let Some(u) = p.uplink_of(l) {
                    p.take_nodes(&u, body);
                }
            }
            method::KICK => {
                let why = String::from_utf8_lossy(body);
                akit_observe::warn!(Tunnel, "tunnel.proxy: kicked by the hub: {why}");
                if let Some(u) = p.uplink_of(l) {
                    u.kicked(&why);
                }
            }
            _ => {}
        }
    }

    fn closed(&self, l: &Arc<Link>, _err: &str) {
        let Some(p) = self.0.upgrade() else { return };
        p.table.close_link(l, "hub connection lost");
        let ups = lock(&p.uplinks).clone();
        for u in ups {
            if u.lost(l) {
                p.forget_nodes(&u);
                break;
            }
        }
    }
}

impl Owner for ProxyCore {
    fn info(&self) -> Info {
        let cfg = lock(&self.cfg);
        let mut i = Info::new(&cfg.proxy_name(), msg::TYPE_PROXY);
        i.meta.clone_from(&cfg.meta);
        i
    }

    fn registered(&self, u: &Arc<Uplink>, _l: &Arc<Link>, first: bool) {
        if !first {
            return;
        }
        if let Some(me) = self.me.upgrade() {
            let name = u.name.clone();
            me.refresh_nodes(
                u,
                Box::new(move |r| {
                    if let Err(e) = r {
                        akit_observe::warn!(Tunnel, "hub {name}: listing the nodes: {e}");
                    }
                }),
            );
        }
    }

    /// The hub's answer to a ping: how fast each node's links are, by name (fit's
    /// `setNodeLatency`).
    fn pong(&self, u: &Arc<Uplink>, body: &[u8]) {
        let Some(v) = msg::parse(body) else { return };
        let Some(Value::Object(by_name)) = v.get("nodes") else {
            return;
        };
        let mut nodes = lock(&self.nodes);
        let Some(list) = nodes.get_mut(&u.name) else {
            return;
        };
        for n in &mut list.nodes {
            if let Some((_, l)) = by_name.iter().find(|(k, _)| *k == n.info.name) {
                n.latency(Some(l));
            }
        }
    }
}

/// The proxy role.
pub struct Proxy {
    core: Arc<ProxyCore>,
    /// Its module of the pages (I109b), made once, so that its consoles keep serving it.
    module: akit_roles::pages::ModuleRef,
}

impl std::fmt::Debug for Proxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Proxy").finish_non_exhaustive()
    }
}

/// Makes the proxy role.
///
/// # Errors
///
/// None: the first apply starts it.
pub fn new(host: Host) -> Result<Box<dyn Role>, String> {
    let limit = Limit::new(akit_config::roles::DEFAULT_TUNNEL_TUNNELS);
    let core = Arc::new_cyclic(|me| ProxyCore {
        host,
        cfg: Mutex::new(TunnelProxy::default()),
        uplinks: Mutex::new(Vec::new()),
        table: Table::new(Arc::clone(&limit)),
        limit,
        proven: Arc::new(Mutex::new(HashSet::new())),
        nodes: Mutex::new(HashMap::new()),
        forwards: Mutex::new(Vec::new()),
        access: Mutex::new(None),
        webs: web::WebProxies::default(),
        me: me.clone(),
    });
    sweep(&core);
    let module: akit_roles::pages::ModuleRef = Arc::new(pages::ProxyPages(Arc::downgrade(&core)));
    Ok(Box::new(Proxy { core, module }))
}

fn sweep(core: &Arc<ProxyCore>) {
    let weak = Arc::downgrade(core);
    let every = lock(&core.cfg)
        .idle()
        .min(Duration::from_secs(60))
        .max(Duration::from_secs(1));
    let _ = akio::timer::after_send(every, move || {
        if let Some(c) = weak.upgrade() {
            let idle = lock(&c.cfg).idle();
            c.table.close_idle(idle);
            sweep(&c);
        }
    });
}

impl Proxy {
    /// Listens on the forwards `want` lists, as fit's `applyForwards`: one on an address that
    /// listens already keeps its listener and takes its new target; the others listen anew (one
    /// that could not before tried again), and those `want` leaves out stop.
    fn move_forwards(&self, want: &[TunnelForward]) -> Vec<String> {
        let c = &self.core;
        let engine = &c.host.engine;
        let mut errs = Vec::new();
        let mut have = std::mem::take(&mut *lock(&c.forwards));
        let mut kept: Vec<Option<Forward>> = want
            .iter()
            .map(|f| {
                let i = have
                    .iter()
                    .position(|h| h.id.is_some() && lock(&h.cfg).listen == f.listen)?;
                let h = have.remove(i);
                *lock(&h.cfg) = f.clone();
                Some(h)
            })
            .collect();
        for h in have.drain(..) {
            if let Some(id) = h.id {
                let _ = engine.remove_listener(id);
            }
        }
        let mut next = Vec::with_capacity(want.len());
        for (f, k) in want.iter().zip(kept.iter_mut()) {
            if let Some(h) = k.take() {
                next.push(h);
                continue;
            }
            let cfg = Arc::new(Mutex::new(f.clone()));
            let mut fw = Forward {
                cfg: Arc::clone(&cfg),
                id: None,
                addr: String::new(),
                err: String::new(),
            };
            let Some(addr) = listen_addr(&f.listen) else {
                fw.err = "not an address to listen on".to_owned();
                errs.push(format!("forward {}: {}", f.listen, fw.err));
                next.push(fw);
                continue;
            };
            let me = c.me.clone();
            let mut l = akit_runtime::scheduled(akio::Listener::tcp(addr))
                .threads(1)
                .serve(akio::fn_acceptor(
                    move |conn: &akio::Conn<EndHandler>| match me.upgrade() {
                        Some(p) => {
                            let fc = lock(&cfg).clone();
                            p.accepted(conn, &fc)
                        }
                        None => {
                            conn.close();
                            EndHandler(None)
                        }
                    },
                ));
            if let Some(p) = lock(&c.access).clone() {
                l = l.filter(move |peer: &SocketAddr| p.allows(peer.ip()));
            }
            match engine.add_listener(l) {
                Ok(id) => {
                    fw.id = Some(id);
                    fw.addr = engine
                        .listeners()
                        .iter()
                        .find(|i| i.id == id)
                        .map(|i| i.local_addr.to_string())
                        .unwrap_or_default();
                }
                Err(e) => {
                    fw.err = e.to_string();
                    errs.push(format!("forward {}: {e}", f.listen));
                }
            }
            next.push(fw);
        }
        *lock(&c.forwards) = next;
        errs
    }
}

impl Role for Proxy {
    fn apply(&mut self, config: &Config) -> Result<(), String> {
        let Some(cfg) = config.tunnel.as_ref().and_then(|t| t.proxy.clone()) else {
            return Ok(());
        };
        let c = &self.core;
        let mut errs = Vec::new();
        match policy(cfg.access.as_ref(), config.access.as_ref()) {
            Ok(p) => {
                // Set again on every apply, the forwards kept as they are (fit's).
                for id in lock(&c.forwards).iter().filter_map(|f| f.id) {
                    let _ = match &p {
                        Some(p) => {
                            let p = Arc::clone(p);
                            c.host
                                .engine
                                .set_listener_filter(id, move |peer: &SocketAddr| {
                                    p.allows(peer.ip())
                                })
                        }
                        None => c.host.engine.clear_listener_filter(id),
                    };
                }
                *lock(&c.access) = p;
            }
            Err(e) => errs.push(e),
        }
        c.limit.set_max(cfg.tunnels());
        let (hubs, forwards, webs) = (
            cfg.hubs.clone(),
            cfg.forwards.clone(),
            cfg.web_proxies.clone(),
        );
        let meta_before = lock(&c.cfg).meta.clone();
        let meta_changed = meta_before != cfg.meta;
        *lock(&c.cfg) = cfg;
        let owner: Weak<dyn Owner> = c.me.clone();
        let router: Arc<dyn Router> = Arc::new(ProxyRouter(c.me.clone()));
        let (before, after) = {
            let mut ups = lock(&c.uplinks);
            let before = ups.clone();
            errs.extend(crate::node::move_uplinks(
                &c.host.engine,
                &mut ups,
                &hubs,
                &router,
                owner,
                &c.proven,
            ));
            // The nodes of a hub stopped or linked anew go with it (fit's), while the uplinks
            // are still locked: a new uplink dials as it starts, and may register and be sent
            // its hub's nodes at once; `set_nodes` waits for the lock to take them, so they come
            // after the old ones go, never before.
            lock(&c.nodes).retain(|name, _| {
                ups.iter()
                    .any(|u| &u.name == name && before.iter().any(|b| Arc::ptr_eq(b, u)))
            });
            (before, ups.clone())
        };
        // The hubs kept are told of the new meta (fit's).
        if meta_changed {
            let body = Owner::info(&**c).json();
            for u in after
                .iter()
                .filter(|u| before.iter().any(|b| Arc::ptr_eq(b, u)))
            {
                if let Some((l, _)) = u.registered_links().into_iter().next() {
                    l.call(
                        method::UPDATE,
                        body.as_bytes(),
                        u.cfg.call(),
                        None,
                        Box::new(|_| {}),
                    );
                }
            }
        }
        errs.extend(self.move_forwards(&forwards));
        let access = lock(&c.access).clone();
        errs.extend(c.webs.apply(&c.host.engine, &webs, access.as_ref()));
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs.join("; "))
        }
    }

    fn stop(&mut self, _grace: Duration) {
        let c = &self.core;
        for id in lock(&c.forwards).drain(..).filter_map(|f| f.id) {
            let _ = c.host.engine.remove_listener(id);
        }
        for u in lock(&c.uplinks).drain(..) {
            u.stop();
        }
        c.webs.stop();
        c.table.close_all("the proxy stopped");
    }

    fn api(&self) -> Option<akit_roles::host::Api> {
        Some(Arc::new(ProxyApi(Arc::clone(&self.core))))
    }

    fn module(&self) -> Option<akit_roles::pages::ModuleRef> {
        Some(Arc::clone(&self.module))
    }
}

/// The proxy's handle for its pages (`Role::api`): what they show of it.
#[derive(Clone)]
pub struct ProxyApi(Arc<ProxyCore>);

impl std::fmt::Debug for ProxyApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyApi").finish_non_exhaustive()
    }
}

impl ProxyApi {
    /// The proxy's web proxies (I103b, fit's `WebProxies`): where each listens or why it does
    /// not, and its connections open.
    #[must_use]
    pub fn web_proxies(&self) -> Vec<WebProxyView> {
        self.0.webs.views()
    }

    /// The proxy's state, as its pages show it (`GET /api/status`).
    #[must_use]
    pub fn status(&self) -> String {
        self.0.status()
    }

    /// How many tunnels are open.
    #[must_use]
    pub fn tunnels(&self) -> usize {
        self.0.table.len()
    }

    /// The names of the hubs the proxy links to.
    #[must_use]
    pub fn hubs(&self) -> Vec<String> {
        self.0.uplinks().iter().map(|u| u.name.clone()).collect()
    }

    /// Whether hub `hub` lists node `node` with service `service`, as the proxy knows it.
    #[must_use]
    pub fn has_node(&self, hub: &str, node: &str, service: &str) -> bool {
        lock(&self.0.nodes).get(hub).is_some_and(|l| {
            l.nodes
                .iter()
                .any(|n| n.info.name == node && n.info.service(service).is_some())
        })
    }

    /// The hub a tunnel of `f` would go through, or why none (fit's `pick`).
    ///
    /// # Errors
    ///
    /// No hub has the node with the service, or the hub `f` names is not the proxy's.
    pub fn pick(&self, f: &TunnelForward) -> Result<String, String> {
        self.0.pick(f).map(|u| u.name.clone())
    }
}

/// The local addresses the forwards listen on, by their `listen`, for the tests.
#[must_use]
pub fn forward_addrs(engine: &akio::Engine) -> Vec<SocketAddr> {
    engine.listeners().iter().map(|l| l.local_addr).collect()
}
