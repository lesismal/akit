//! `tunnel.node` (fit's `tunnel/node`): it links to its hubs, registers with the services it
//! reaches, and connects to a service for each tunnel a hub routes to it, to no other address.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use akit_config::Config;
use akit_config::roles::TunnelNode;
use akit_config::value::Value;
use akit_roles::host::{Host, Role};

use crate::end::{EndHandler, Limit, Table};
use crate::link::{Link, Responder, Router};
use crate::msg::{self, Info, Service, Stats, TunnelOpenReq, method};
use crate::uplink::{Owner, Uplink};

mod pages;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) struct NodeCore {
    host: Host,
    cfg: Mutex<TunnelNode>,
    uplinks: Mutex<Vec<Arc<Uplink>>>,
    table: Arc<Table>,
    limit: Arc<Limit>,
    proven: Arc<Mutex<HashSet<u64>>>,
    /// By service name, how long the connects to the services took (fit's `serviceRTT`).
    service_rtt: Mutex<HashMap<String, Stats>>,
    me: Weak<NodeCore>,
}

/// The node's router, which holds the node weakly: its links and uplinks hold the router.
struct NodeRouter(Weak<NodeCore>);

/// The services a node's config lists, as the hubs are told them.
fn services(cfg: &TunnelNode) -> Vec<Service> {
    cfg.services
        .iter()
        .map(|s| Service {
            name: s.name.clone(),
            addr: s.address.clone(),
            desc: s.description.clone(),
        })
        .collect()
}

/// `host:port` taken apart, an IPv6 host in brackets.
pub(crate) fn host_port(addr: &str) -> Option<(String, u16)> {
    let (h, p) = if let Some(rest) = addr.strip_prefix('[') {
        let (h, p) = rest.split_once(']')?;
        (h, p.strip_prefix(':')?)
    } else {
        addr.rsplit_once(':')?
    };
    Some((h.to_owned(), p.parse().ok()?))
}

impl NodeCore {
    fn info(&self) -> Info {
        let cfg = lock(&self.cfg);
        let mut i = Info::new(&cfg.node_name(), msg::TYPE_NODE);
        i.services = services(&cfg);
        i.meta.clone_from(&cfg.meta);
        i
    }

    fn router(&self) -> Arc<dyn Router> {
        Arc::new(NodeRouter(self.me.clone()))
    }

    fn open(self: &Arc<Self>, l: &Arc<Link>, body: &[u8], r: Responder) {
        let Some(req) = msg::parse(body).map(|v| TunnelOpenReq::from(&v)) else {
            r.err("tunnel: a bad request");
            return;
        };
        let (svc, dial) = {
            let cfg = lock(&self.cfg);
            (
                cfg.services.iter().find(|s| s.name == req.service).cloned(),
                cfg.dial(),
            )
        };
        let Some(svc) = svc else {
            r.err(&format!("tunnel: no service {:?}", req.service));
            return;
        };
        let Some((host, port)) = host_port(&svc.address) else {
            r.err(&format!("tunnel: {:?} is not a host and port", svc.address));
            return;
        };
        let slot = Arc::new(Mutex::new(Some(r)));
        let (slot2, me, link) = (Arc::clone(&slot), Arc::clone(self), Arc::clone(l));
        let addr = svc.address.clone();
        let addr2 = addr.clone();
        let (svc_name, svc_name2) = (svc.name.clone(), svc.name.clone());
        let me2 = Arc::clone(self);
        let start = Instant::now();
        let mut he = akio::dns::HappyEyeballs::default();
        he.connect_timeout = dial;
        he.connect(
            &self.host.engine,
            &host,
            port,
            akio::fn_acceptor(move |c: &akio::Conn<EndHandler>| {
                let Some(r) = lock(&slot).take() else {
                    c.close();
                    return EndHandler(None);
                };
                let ms = (start.elapsed().as_secs_f64() * 1_000_000.0).round() / 1000.0;
                me.service_meter(&svc_name, |m| m.add(ms));
                let name = lock(&me.cfg).node_name();
                match me
                    .table
                    .add(&link, req.id, true, &name, &req.service, &req.proxy)
                {
                    Ok(e) => {
                        e.set_addr(&addr, ms);
                        e.attach(c);
                        let rsp = msg::Obj::new().num("id", req.id).str("addr", &addr).end();
                        r.ok(rsp.as_bytes());
                        EndHandler(Some(e))
                    }
                    Err(e) => {
                        me.service_meter(&svc_name, Stats::lost);
                        r.err(e);
                        c.close();
                        EndHandler(None)
                    }
                }
            }),
            move |e| {
                if let Some(r) = lock(&slot2).take() {
                    me2.service_meter(&svc_name2, Stats::lost);
                    r.err(&format!("tunnel: connect to {addr2}: {e}"));
                }
            },
        );
    }

    /// Changes the meter of the connects to service `name` with `f`.
    fn service_meter(&self, name: &str, f: impl FnOnce(&mut Stats)) {
        let mut m = lock(&self.service_rtt);
        if let Some(s) = m.get_mut(name) {
            f(s);
            return;
        }
        // A meter for each service the config lists, and no more.
        if lock(&self.cfg).services.iter().any(|s| s.name == name) {
            let mut s = Stats::default();
            f(&mut s);
            m.insert(name.to_owned(), s);
        }
    }

    /// By service name, how long the connects to the services took, as JSON (fit's `ServiceRTT`);
    /// `{}` for none.
    fn service_rtt_json(&self) -> String {
        let m = lock(&self.service_rtt);
        let mut names: Vec<&String> = m.keys().collect();
        names.sort();
        let mut o = msg::Obj::new();
        for n in names {
            if let Some(s) = m.get(n) {
                o = o.raw(n, &s.json());
            }
        }
        o.end()
    }

    /// The uplinks now.
    fn uplinks(&self) -> Vec<Arc<Uplink>> {
        lock(&self.uplinks).clone()
    }

    /// The node's state as its pages show it (fit's `node.Status`).
    pub(crate) fn status(&self) -> String {
        let cfg = lock(&self.cfg).clone();
        let ups = self.uplinks();
        let ends = self.table.list();
        let services = msg::array(&services(&cfg), crate::pages::service_view);
        let hubs = msg::array(&ups, |u| {
            let n = ends.iter().filter(|e| u.owns(e.link())).count();
            u.status_json(n)
        });
        let tunnels = msg::array(&ends, |e| e.view(&crate::pages::hub_of(&ups, e)));
        let mut meta = msg::Obj::new();
        for (k, v) in &cfg.meta {
            meta = meta.str(k, v);
        }
        msg::Obj::new()
            .str("name", &cfg.node_name())
            .str("version", &msg::version())
            .raw("meta", &meta.end())
            .raw("services", &services)
            .raw("service_rtt", &self.service_rtt_json())
            .raw("hubs", &hubs)
            .raw("tunnels", &tunnels)
            .num("open", self.limit.open())
            .num("max_tunnels", cfg.tunnels())
            .end()
    }

    fn set_services(self: &Arc<Self>, body: &[u8], r: Responder) {
        if !lock(&self.cfg).allow_hub_services {
            r.err("tunnel: the node does not let its hubs set its services (allow_hub_services)");
            return;
        }
        let list = match std::str::from_utf8(body)
            .ok()
            .and_then(|t| akit_config::value::parse(t).ok())
        {
            Some(Value::Array(items)) => items.iter().map(Service::from).collect::<Vec<_>>(),
            Some(Value::Null) => Vec::new(),
            _ => {
                r.err("tunnel: the services are not a JSON array");
                return;
            }
        };
        let value = Value::Array(
            list.iter()
                .map(|s| {
                    let mut ms = vec![
                        ("name".to_owned(), Value::String(s.name.clone())),
                        ("address".to_owned(), Value::String(s.addr.clone())),
                    ];
                    if !s.desc.is_empty() {
                        ms.push(("description".to_owned(), Value::String(s.desc.clone())));
                    }
                    Value::Object(ms)
                })
                .collect(),
        );
        let me = Arc::clone(self);
        self.host.edit(
            Box::new(move |m| {
                let Value::Object(ms) = m else {
                    return Err("tunnel: the node's member is not an object".to_owned());
                };
                ms.retain(|(k, _)| k != "services");
                if !matches!(&value, Value::Array(a) if a.is_empty()) {
                    ms.push(("services".to_owned(), value));
                }
                Ok(())
            }),
            Box::new(move |res| match res {
                Ok(_) => r.ok(me.info().json().as_bytes()),
                Err(e) => r.err(&format!(
                    "tunnel: the node's services were not changed: {e}"
                )),
            }),
        );
    }
}

impl Router for NodeRouter {
    fn guard(&self, l: &Arc<Link>, _m: u8) -> Result<(), String> {
        let Some(n) = self.0.upgrade() else {
            return Err("tunnel: the node stopped".to_owned());
        };
        if lock(&n.proven).contains(&l.id) {
            Ok(())
        } else {
            Err("tunnel: the hub has not proven itself on this link yet".to_owned())
        }
    }

    fn call(&self, l: &Arc<Link>, m: u8, body: &[u8], r: Responder) {
        let Some(n) = self.0.upgrade() else {
            r.err("tunnel: the node stopped");
            return;
        };
        match m {
            method::NODE_INFO => r.ok(n.info().json().as_bytes()),
            method::NODE_SET_SERVICES => n.set_services(body, r),
            method::TUNNEL_OPEN => n.open(l, body, r),
            _ => r.err(&format!("link: no method {m}")),
        }
    }

    fn notify(&self, l: &Arc<Link>, m: u8, body: &[u8]) {
        let Some(n) = self.0.upgrade() else { return };
        match m {
            method::TUNNEL_DATA | method::TUNNEL_ACK | method::TUNNEL_CLOSE => {
                n.table.frame(l, m, body);
            }
            method::KICK => {
                let why = String::from_utf8_lossy(body);
                akit_observe::warn!(Tunnel, "tunnel.node: kicked by the hub: {why}");
                let ups = lock(&n.uplinks).clone();
                if let Some(u) = ups.iter().find(|u| u.owns(l)) {
                    u.kicked(&why);
                }
            }
            _ => {}
        }
    }

    fn closed(&self, l: &Arc<Link>, _err: &str) {
        let Some(n) = self.0.upgrade() else { return };
        n.table.close_link(l, "hub connection lost");
        let ups = lock(&n.uplinks).clone();
        for u in ups {
            if u.lost(l) {
                break;
            }
        }
    }
}

impl Owner for NodeCore {
    fn info(&self) -> Info {
        NodeCore::info(self)
    }

    /// Hubs tell proxies how fast the node reaches its services.
    fn ping_extra(&self) -> String {
        if lock(&self.service_rtt).is_empty() {
            return String::new();
        }
        self.service_rtt_json()
    }
}

/// The node role.
pub struct Node {
    core: Arc<NodeCore>,
    /// Its module of the pages (I109b), made once, so that its consoles keep serving it.
    module: akit_roles::pages::ModuleRef,
}

/// The node's handle for its pages and tests (`Role::api`).
#[derive(Clone)]
pub struct NodeApi(Arc<NodeCore>);

impl std::fmt::Debug for NodeApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeApi").finish_non_exhaustive()
    }
}

impl NodeApi {
    /// The node's state, as its pages show it (`GET /api/status`).
    #[must_use]
    pub fn status(&self) -> String {
        self.0.status()
    }

    /// How many tunnels are open.
    #[must_use]
    pub fn tunnels(&self) -> usize {
        self.0.table.len()
    }

    /// The names of the hubs the node links to.
    #[must_use]
    pub fn hubs(&self) -> Vec<String> {
        self.0.uplinks().iter().map(|u| u.name.clone()).collect()
    }
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node").finish_non_exhaustive()
    }
}

/// Makes the node role.
///
/// # Errors
///
/// None: the first apply starts it.
pub fn new(host: Host) -> Result<Box<dyn Role>, String> {
    let limit = Limit::new(akit_config::roles::DEFAULT_TUNNEL_TUNNELS);
    let core = Arc::new_cyclic(|me| NodeCore {
        host,
        cfg: Mutex::new(TunnelNode::default()),
        uplinks: Mutex::new(Vec::new()),
        table: Table::new(Arc::clone(&limit)),
        limit,
        proven: Arc::new(Mutex::new(HashSet::new())),
        service_rtt: Mutex::new(HashMap::new()),
        me: me.clone(),
    });
    sweep(&core);
    let module: akit_roles::pages::ModuleRef = Arc::new(pages::NodePages(Arc::downgrade(&core)));
    Ok(Box::new(Node { core, module }))
}

/// Closes the tunnels idle past `idle_timeout`, every minute at most.
fn sweep(core: &Arc<NodeCore>) {
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

/// Moves `uplinks` to the hubs `want` lists: those whose config is the same stay, with their
/// links; the others are stopped and started anew.
pub(crate) fn move_uplinks(
    engine: &Arc<akio::Engine>,
    have: &mut Vec<Arc<Uplink>>,
    want: &[akit_config::roles::TunnelUplink],
    router: &Arc<dyn Router>,
    owner: Weak<dyn Owner>,
    proven: &Arc<Mutex<HashSet<u64>>>,
) -> Vec<String> {
    let mut errs = Vec::new();
    let mut next = Vec::new();
    for cfg in want {
        if let Some(i) = have.iter().position(|u| u.cfg == *cfg) {
            next.push(have.remove(i));
            continue;
        }
        match Uplink::new(
            Arc::clone(engine),
            cfg,
            Arc::clone(router),
            owner.clone(),
            Arc::clone(proven),
        ) {
            Ok(u) => {
                u.start();
                next.push(u);
            }
            Err(e) => errs.push(format!("hub {}: {e}", cfg.hub_name())),
        }
    }
    for u in have.drain(..) {
        u.stop();
    }
    *have = next;
    errs
}

impl Role for Node {
    fn apply(&mut self, config: &Config) -> Result<(), String> {
        let Some(cfg) = config.tunnel.as_ref().and_then(|t| t.node.clone()) else {
            return Ok(());
        };
        let c = &self.core;
        let info_before = c.info();
        c.limit.set_max(cfg.tunnels());
        let hubs = cfg.hubs.clone();
        // What was measured of a service goes with it, and a service whose address changes is
        // measured anew (fit's).
        {
            let was = lock(&c.cfg).services.clone();
            lock(&c.service_rtt).retain(|name, _| {
                let old = was.iter().find(|s| &s.name == name);
                cfg.services
                    .iter()
                    .find(|s| &s.name == name)
                    .is_some_and(|s| old.is_some_and(|o| o.address == s.address))
            });
        }
        *lock(&c.cfg) = cfg;
        let owner: Weak<dyn Owner> = c.me.clone();
        let router = c.router();
        let errs = {
            let mut ups = lock(&c.uplinks);
            move_uplinks(&c.host.engine, &mut ups, &hubs, &router, owner, &c.proven)
        };
        let info = c.info();
        if info != info_before {
            let body = info.json();
            for u in lock(&c.uplinks).iter() {
                for (l, _) in u.registered_links().into_iter().take(1) {
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
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs.join("; "))
        }
    }

    fn stop(&mut self, _grace: Duration) {
        for u in lock(&self.core.uplinks).drain(..) {
            u.stop();
        }
        self.core.table.close_all("the node stopped");
    }

    fn api(&self) -> Option<akit_roles::host::Api> {
        Some(Arc::new(NodeApi(Arc::clone(&self.core))))
    }

    fn module(&self) -> Option<akit_roles::pages::ModuleRef> {
        Some(Arc::clone(&self.module))
    }
}
