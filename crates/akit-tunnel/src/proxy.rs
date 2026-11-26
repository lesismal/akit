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
use crate::msg::{self, Info, MAX_TUNNELS_PER_LINK, TunnelOpenReq, method};
use crate::uplink::{Owner, Uplink};

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

/// A forward listening.
struct Forward {
    cfg: TunnelForward,
    id: akio::ListenerId,
}

pub(crate) struct ProxyCore {
    host: Host,
    cfg: Mutex<TunnelProxy>,
    uplinks: Mutex<Vec<Arc<Uplink>>>,
    table: Arc<Table>,
    limit: Arc<Limit>,
    proven: Arc<Mutex<HashSet<u64>>>,
    /// Each hub's nodes, by the hub's name here: the node's name and info.
    nodes: Mutex<HashMap<String, Vec<Info>>>,
    forwards: Mutex<Vec<Forward>>,
    access: Mutex<Option<Arc<akit_access::Policy>>>,
    /// The web proxies (I103b).
    webs: web::WebProxies,
    me: Weak<ProxyCore>,
}

struct ProxyRouter(Weak<ProxyCore>);

impl ProxyCore {
    /// The link and uplink a tunnel to `node` goes through: of the hub `hub` names, or else of
    /// any hub that lists the node; the link carrying the fewest tunnels.
    fn route(&self, node: &str, hub: &str) -> Option<(Arc<Uplink>, Arc<Link>, msg::RegisterRsp)> {
        let ups = lock(&self.uplinks).clone();
        let nodes = lock(&self.nodes).clone();
        let has = |u: &Uplink| {
            nodes
                .get(&u.name)
                .is_some_and(|ns| ns.iter().any(|i| i.name == node))
        };
        let mut best: Option<(Arc<Uplink>, Arc<Link>, msg::RegisterRsp, usize)> = None;
        for u in ups.iter().filter(|u| {
            if hub.is_empty() {
                has(u)
            } else {
                u.name == hub
            }
        }) {
            for (l, rsp) in u.registered_links() {
                let load = self.table.on_link(&l);
                if load >= MAX_TUNNELS_PER_LINK {
                    continue;
                }
                if best.as_ref().is_none_or(|b| load < b.3) {
                    best = Some((Arc::clone(u), l, rsp, load));
                }
            }
        }
        best.map(|(u, l, r, _)| (u, l, r))
    }

    /// A connection accepted on forward `f`: a tunnel opened for it, its reads held until then.
    fn accepted(self: &Arc<Self>, c: &akio::Conn<EndHandler>, f: &TunnelForward) -> EndHandler {
        let Some((u, l, rsp)) = self.route(&f.node, &f.hub) else {
            akit_observe::warn!(
                Tunnel,
                "tunnel.proxy: no hub reaches node {:?} for {}",
                f.node,
                f.listen
            );
            c.close();
            return EndHandler(None);
        };
        let id = msg::tunnel_id(rsp.conn, u.next_seq());
        let e = match self.table.add(&l, id, false, &f.node, &f.service, &u.name) {
            Ok(e) => e,
            Err(err) => {
                akit_observe::warn!(Tunnel, "tunnel.proxy: {err}: {}", f.listen);
                c.close();
                return EndHandler(None);
            }
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
                Ok(_) => e2.set_ready(),
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

    fn take_nodes(&self, u: &Uplink, body: &[u8]) {
        let Some(v) = msg::parse(body) else { return };
        let list = match v.get("nodes") {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|n| n.get("info").map(Info::from))
                .collect(),
            _ => Vec::new(),
        };
        lock(&self.nodes).insert(u.name.clone(), list);
    }

    fn uplink_of(&self, l: &Link) -> Option<Arc<Uplink>> {
        lock(&self.uplinks).iter().find(|u| u.owns(l)).cloned()
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
                if u.registered_links().is_empty() {
                    lock(&p.nodes).remove(&u.name);
                }
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

    fn registered(&self, u: &Arc<Uplink>, l: &Arc<Link>, _first: bool) {
        let (me, u2) = (self.me.clone(), Arc::clone(u));
        l.call(
            method::NODES,
            b"",
            u.cfg.call(),
            None,
            Box::new(move |r| {
                if let (Ok(body), Some(p)) = (r, me.upgrade()) {
                    p.take_nodes(&u2, &body);
                }
            }),
        );
    }
}

/// The proxy role.
pub struct Proxy {
    core: Arc<ProxyCore>,
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
    Ok(Box::new(Proxy { core }))
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
    /// Listens on the forwards `want` lists: those on the same address with the same target
    /// stay; the others are removed and added anew.
    fn move_forwards(&self, want: &[TunnelForward]) -> Vec<String> {
        let c = &self.core;
        let engine = &c.host.engine;
        let mut errs = Vec::new();
        let mut have = lock(&c.forwards);
        let mut next = Vec::new();
        for f in want {
            if let Some(i) = have.iter().position(|h| h.cfg == *f) {
                next.push(have.remove(i));
                continue;
            }
            let Some(addr) = listen_addr(&f.listen) else {
                errs.push(format!("forward {}: not an address to listen on", f.listen));
                continue;
            };
            let (me, fc) = (c.me.clone(), f.clone());
            let mut l = akio::Listener::tcp(addr)
                .threads(1)
                .serve(akio::fn_acceptor(
                    move |conn: &akio::Conn<EndHandler>| match me.upgrade() {
                        Some(p) => p.accepted(conn, &fc),
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
                Ok(id) => next.push(Forward { cfg: f.clone(), id }),
                Err(e) => errs.push(format!("forward {}: {e}", f.listen)),
            }
        }
        for h in have.drain(..) {
            let _ = engine.remove_listener(h.id);
        }
        *have = next;
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
                for f in lock(&c.forwards).iter() {
                    let _ = match &p {
                        Some(p) => {
                            let p = Arc::clone(p);
                            c.host
                                .engine
                                .set_listener_filter(f.id, move |peer: &SocketAddr| {
                                    p.allows(peer.ip())
                                })
                        }
                        None => c.host.engine.clear_listener_filter(f.id),
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
        *lock(&c.cfg) = cfg;
        let owner: Weak<dyn Owner> = c.me.clone();
        let router: Arc<dyn Router> = Arc::new(ProxyRouter(c.me.clone()));
        {
            let mut ups = lock(&c.uplinks);
            errs.extend(crate::node::move_uplinks(
                &c.host.engine,
                &mut ups,
                &hubs,
                &router,
                owner,
                &c.proven,
            ));
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
        for f in lock(&c.forwards).drain(..) {
            let _ = c.host.engine.remove_listener(f.id);
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
}

/// The local addresses the forwards listen on, by their `listen`, for the tests.
#[must_use]
pub fn forward_addrs(engine: &akio::Engine) -> Vec<SocketAddr> {
    engine.listeners().iter().map(|l| l.local_addr).collect()
}
