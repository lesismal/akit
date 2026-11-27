//! fit's LLM hub (`llm/hub`, I106 and I106b): the table of its nodes (what they push), the models
//! of its online nodes, pushed to its proxies when they change, its node list for them (bounded
//! at 4 MiB, pushed a second apart at most, its traffic every 5 s), and `/hub/proxy`: a proxy's
//! call relayed to the least loaded node serving the model, frame by frame under each stream's
//! credit ([`pump`]), another node tried (3 at most) while nothing reached the proxy, counted on
//! its route.
//!
//! Its own pages call `/hub/nodes`, `/hub/node/call` and `/hub/node/stream` (a node's management
//! methods relayed, where the node allows a hub to manage it), its settings and its name.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use akio::timer::{SendTimer, after_send};
use akit_config::Config;
use akit_roles::host::{Api, Host, Info, Role};
use akit_roles::linktoken::Registry;

use crate::hw::HwInfo;
use crate::json::{self, Obj, Req};
use crate::mesh::{Answer, Router, bad_request, offline};
use crate::model::{
    CODE_FORBIDDEN, CODE_INTERNAL, CODE_UNAVAILABLE, HubModel, HubModels, Identity,
    MAX_NODE_LIST_BYTES, MAX_NODE_LIST_MODELS, METHOD_HUB_MODELS, METHOD_HUB_MODELS_UPDATE,
    METHOD_HUB_NODE_CALL, METHOD_HUB_NODE_LIST, METHOD_HUB_NODE_LIST_UPDATE,
    METHOD_HUB_NODE_STREAM, METHOD_HUB_NODES, METHOD_HUB_PROFILE_SET, METHOD_HUB_PROXY,
    METHOD_HUB_SETTINGS_GET, METHOD_HUB_SETTINGS_SET, METHOD_NODE_INFO, METHOD_NODE_PROXY,
    METHOD_NODE_UPDATE, MODEL_DISABLED, NODE_MANAGE_METHODS, NODE_STREAM_METHODS, NodeInfo,
    PROTOCOLS, PeerConn, ProxyReq, ROLE_NODE, ROLE_PROXY, protocol,
};
use crate::role::{Base, EDIT_TIMEOUT, count_setting, number, set};
use crate::wire::{Error, Stream, pump};

/// `llm.hub`.
pub const HUB: Info = Info {
    id: "llm.hub",
    set: |c| c.llm.as_ref().is_some_and(|l| l.hub.is_some()),
    new,
};

/// Model changes gathered into one push.
const COALESCE: Duration = Duration::from_millis(200);
/// The node list goes out no sooner than this after the last one...
const LIST_GAP: Duration = Duration::from_secs(1);
/// ...and its traffic and latency, which change all the time, this often at most.
const LIST_REFRESH: Duration = Duration::from_secs(5);
/// The GPUs a node of the list shows.
const MAX_LIST_GPUS: usize = 16;
/// A managed call's bound (a model check probes the upstream).
const NODE_CALL_TIMEOUT: Duration = Duration::from_secs(90);
/// The nodes one call is tried on.
const MAX_TRIES: usize = 3;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now() -> i64 {
    akit_roles::linktoken::unix_now()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

struct Entry {
    info: NodeInfo,
    started: i64,
    last_seen: i64,
}

/// The node list pushed to the proxies: the last one, its nodes' encoding, when it went out,
/// and the push waiting.
struct List {
    seq: i64,
    nodes: String,
    total: usize,
    at: Option<Instant>,
    timer: Option<SendTimer>,
}

struct St {
    nodes: HashMap<String, Entry>,
    models: HubModels,
    list: List,
    inflight: HashMap<String, i64>,
    rr: usize,
    timer: Option<SendTimer>,
    tick: Option<SendTimer>,
    max_nodes: usize,
}

struct Core {
    base: Base,
    me: Weak<Core>,
    st: Mutex<St>,
    stopped: AtomicBool,
}

struct Hub(Arc<Core>);

impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hub").finish_non_exhaustive()
    }
}

/// Runs `f` on the hub, or answers that it is gone.
fn with(me: &Weak<Core>, ans: Answer, f: impl FnOnce(&Arc<Core>, Answer)) {
    match me.upgrade() {
        Some(h) => f(&h, ans),
        None => ans.err(Error::Closed),
    }
}

/// Makes the hub role; its first apply starts it.
///
/// # Errors
///
/// None.
#[allow(clippy::too_many_lines)]
pub fn new(host: Host) -> Result<Box<dyn Role>, String> {
    let core = Arc::new_cyclic(|me: &Weak<Core>| {
        let mut r = Router::new();
        let m = me.clone();
        r.handle(
            METHOD_HUB_MODELS,
            &[ROLE_PROXY],
            Duration::ZERO,
            move |_, _, a| {
                with(&m, a, |h, a| a.ok(h.models().to_json()));
            },
        );
        let m = me.clone();
        r.handle(
            METHOD_HUB_NODE_LIST,
            &[ROLE_PROXY],
            Duration::ZERO,
            move |_, _, a| with(&m, a, |h, a| a.ok(h.node_list())),
        );
        let m = me.clone();
        r.handle_stream(
            METHOD_HUB_PROXY,
            &[ROLE_PROXY],
            move |caller, req, st| match m.upgrade() {
                Some(h) => h.proxy(caller, req, st),
                None => st.end(Err(Error::Closed)),
            },
        );
        // Its own pages only.
        let m = me.clone();
        r.handle(METHOD_HUB_NODES, &[], Duration::ZERO, move |_, _, a| {
            with(&m, a, |h, a| a.ok(h.nodes_json()));
        });
        let m = me.clone();
        r.handle(
            METHOD_HUB_NODE_CALL,
            &[],
            NODE_CALL_TIMEOUT,
            move |_, req, a| with(&m, a, |h, a| h.node_call(req, a)),
        );
        let m = me.clone();
        r.handle_stream(METHOD_HUB_NODE_STREAM, &[], move |_, req, st| {
            match m.upgrade() {
                Some(h) => h.node_stream(req, st),
                None => st.end(Err(Error::Closed)),
            }
        });
        let m = me.clone();
        r.handle(
            METHOD_HUB_SETTINGS_GET,
            &[],
            Duration::ZERO,
            move |_, _, a| {
                with(&m, a, |h, a| a.ok(h.settings()));
            },
        );
        let m = me.clone();
        r.handle(
            METHOD_HUB_SETTINGS_SET,
            &[],
            EDIT_TIMEOUT,
            move |_, req, a| with(&m, a, |h, a| h.set_settings(req, a)),
        );
        let m = me.clone();
        r.handle(
            METHOD_HUB_PROFILE_SET,
            &[],
            Duration::ZERO,
            move |_, req, a| {
                with(&m, a, |h, a| h.set_profile(req, a));
            },
        );
        Core {
            base: Base::new(host, "hub", r),
            me: me.clone(),
            st: Mutex::new(St {
                nodes: HashMap::new(),
                models: HubModels {
                    seq: now_ms(),
                    ..HubModels::default()
                },
                list: List {
                    seq: now_ms(),
                    nodes: String::new(),
                    total: 0,
                    at: None,
                    timer: None,
                },
                inflight: HashMap::new(),
                rr: 0,
                timer: None,
                tick: None,
                max_nodes: akit_config::roles::DEFAULT_LLM_MAX_NODES,
            }),
            stopped: AtomicBool::new(false),
        }
    });
    let m = &core.base.mesh;
    let w = Arc::downgrade(&core);
    m.on_push(METHOD_NODE_UPDATE, &[ROLE_NODE], move |caller, msg| {
        let Some(h) = w.upgrade() else { return };
        match json::parse(msg) {
            Ok(v) => h.apply_info(caller, NodeInfo::from_value(&v)),
            Err(e) => {
                akit_observe::warn!(Llm, "llm.hub: a bad node update from {}: {e}", caller.id);
            }
        }
    });
    let w = Arc::downgrade(&core);
    m.on_peer_up(move |p| {
        if let Some(h) = w.upgrade() {
            h.peer_up(p);
        }
    });
    let w = Arc::downgrade(&core);
    m.on_peer_down(move |p| {
        if let Some(h) = w.upgrade() {
            h.peer_down(p);
        }
    });
    core.arm_refresh();
    Ok(Box::new(Hub(core)))
}

impl Role for Hub {
    fn apply(&mut self, config: &Config) -> Result<(), String> {
        let Some(h) = config.llm.as_ref().and_then(|l| l.hub.as_ref()) else {
            return Ok(());
        };
        {
            let mut st = lock(&self.0.st);
            st.max_nodes = h.nodes();
        }
        self.0.prune();
        self.0.base.apply(&h.role, config)
    }

    fn stop(&mut self, grace: Duration) {
        self.0.stopped.store(true, Ordering::Release);
        {
            let mut st = lock(&self.0.st);
            for t in [st.timer.take(), st.list.timer.take(), st.tick.take()]
                .into_iter()
                .flatten()
            {
                t.cancel();
            }
        }
        self.0.base.stop(grace.min(Duration::from_secs(10)));
    }

    fn tokens(&self) -> Option<Registry> {
        Some(self.0.base.mesh.tokens())
    }

    fn files(&self) -> Vec<(String, [u8; 32])> {
        self.0.base.mesh.files()
    }

    fn api(&self) -> Option<Api> {
        Some(Arc::new(self.0.base.local.clone()))
    }

    fn module(&self) -> Option<akit_roles::pages::ModuleRef> {
        Some(self.0.base.pages.clone())
    }
}

/// A node's links and their traffic, as a hub's node list shows them.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Traffic {
    calls: i64,
    errors: i64,
    inflight: i64,
    req_bytes: i64,
    resp_bytes: i64,
}

/// The traffic of the calls relayed to each node, by its ID, as the routes counted it.
pub(crate) fn traffic_by_node(routes: &crate::mesh::Routes) -> HashMap<String, Traffic> {
    let mut traffic: HashMap<String, Traffic> = HashMap::new();
    for r in routes.snapshot() {
        let t = traffic.entry(r.to).or_default();
        t.calls = t.calls.saturating_add(r.calls);
        t.errors = t.errors.saturating_add(r.errors);
        t.inflight = t.inflight.saturating_add(r.inflight);
        t.req_bytes = t.req_bytes.saturating_add(r.req_bytes);
        t.resp_bytes = t.resp_bytes.saturating_add(r.resp_bytes);
    }
    traffic
}

impl Traffic {
    fn to_json(self) -> String {
        Obj::new()
            .int("calls", self.calls)
            .int("errors", self.errors)
            .int("inflight", self.inflight)
            .int("req_bytes", self.req_bytes)
            .int("resp_bytes", self.resp_bytes)
            .end()
    }
}

/// One node of the table, as the hub's pages list it (fit's `HubNode`).
pub(crate) struct NodeView {
    pub(crate) info: NodeInfo,
    pub(crate) online: bool,
    pub(crate) conns: Vec<PeerConn>,
    pub(crate) last_seen: i64,
}

/// `nodes` encoded within [`MAX_NODE_LIST_BYTES`], the nodes at the end (offline ones first, as
/// they are sorted) left out while it is larger.
fn encode_list(mut nodes: Vec<String>) -> String {
    loop {
        let enc = json::array(nodes.iter().cloned());
        if enc.len() <= MAX_NODE_LIST_BYTES || nodes.is_empty() {
            return enc;
        }
        let keep = nodes.len().saturating_mul(3) / 4;
        nodes.truncate(keep);
    }
}

/// A node as a hub's proxies see it (fit's `NodeSummary`): its hardware without its disks and
/// addresses, 16 GPUs and 64 models at most, its load, its links with the hub and their traffic.
pub(crate) fn summary(n: &NodeView, traffic: Traffic) -> String {
    let mut hw = HwInfo::parse(&n.info.hw);
    hw.disks = None;
    hw.ips = None;
    if let Some(g) = hw.gpus.as_mut() {
        g.truncate(MAX_LIST_GPUS);
    }
    let models = &n.info.models;
    let links: Vec<String> = n
        .conns
        .iter()
        .map(|c| {
            Obj::new()
                .str("direction", &c.direction)
                .int("connected_at", c.connected_at)
                .bool_omit("draining", c.draining)
                .raw("link", &c.link.to_json())
                .end()
        })
        .collect();
    let mut o = Obj::new()
        .str("id", &n.info.id)
        .str("name", &n.info.name)
        .str_omit("description", &n.info.description)
        .bool("online", n.online)
        .int("last_seen", n.last_seen)
        .raw("hw", &hw.to_json())
        .raw(
            "models",
            &json::array(
                models
                    .iter()
                    .take(MAX_NODE_LIST_MODELS)
                    .map(crate::model::ModelView::to_json),
            ),
        )
        .int("models_total", i64::try_from(models.len()).unwrap_or(0))
        .int("inflight", n.info.inflight)
        .int("jobs", n.info.jobs);
    if !links.is_empty() {
        o = o.raw("links", &json::array(links));
    }
    o.raw("traffic", &traffic.to_json()).end()
}

impl Core {
    /// Forgets the offline nodes seen longest ago while the table is past its bound.
    fn prune(&self) {
        let mut st = lock(&self.st);
        while st.nodes.len() > st.max_nodes {
            let oldest = st
                .nodes
                .iter()
                .filter(|(id, _)| !self.base.mesh.online(id))
                .min_by_key(|(_, e)| e.last_seen)
                .map(|(id, _)| id.clone());
            match oldest {
                Some(id) => {
                    st.nodes.remove(&id);
                }
                None => return, // online nodes are bounded by the links
            }
        }
    }

    fn peer_up(&self, p: &PeerConn) {
        let id = p.identity.clone();
        match id.role.as_str() {
            ROLE_NODE => {
                let me = self.me.clone();
                let peer = id.id.clone();
                self.base.mesh.call(
                    &peer,
                    METHOD_NODE_INFO,
                    "{}",
                    Duration::from_secs(10),
                    move |r| {
                        let Some(h) = me.upgrade() else { return };
                        match r.and_then(|b| json::parse(&b).map_err(Error::Other)) {
                            Ok(v) => h.apply_info(&id, NodeInfo::from_value(&v)),
                            Err(e) => {
                                akit_observe::warn!(
                                    Llm,
                                    "llm.hub: node info of {} failed: {e}",
                                    id.id
                                );
                                h.touch(&id);
                            }
                        }
                    },
                );
            }
            ROLE_PROXY => {
                let m = &self.base.mesh;
                let _ = m.push(&id.id, METHOD_HUB_MODELS_UPDATE, &self.models().to_json());
                let _ = m.push(&id.id, METHOD_HUB_NODE_LIST_UPDATE, &self.node_list());
            }
            _ => {}
        }
    }

    fn peer_down(&self, p: &PeerConn) {
        if p.identity.role != ROLE_NODE || self.base.mesh.online(&p.identity.id) {
            return;
        }
        if let Some(e) = lock(&self.st).nodes.get_mut(&p.identity.id) {
            e.last_seen = now();
        }
        self.changed();
    }

    /// A linked node whose info could not be read has an entry nonetheless.
    fn touch(&self, id: &Identity) {
        lock(&self.st)
            .nodes
            .entry(id.id.clone())
            .or_insert_with(|| Entry {
                info: NodeInfo {
                    id: id.id.clone(),
                    name: id.name.clone(),
                    ..NodeInfo::default()
                },
                started: id.started_at,
                last_seen: now(),
            });
        self.prune();
    }

    /// Records a node's info: within one run of the node (the same start) a sequence not above
    /// the current one is stale and dropped.
    fn apply_info(&self, caller: &Identity, mut info: NodeInfo) {
        info.id.clone_from(&caller.id);
        if info.name.is_empty() {
            info.name.clone_from(&caller.name);
        }
        {
            let mut st = lock(&self.st);
            let e = st.nodes.entry(caller.id.clone()).or_insert_with(|| Entry {
                info: NodeInfo::default(),
                started: 0,
                last_seen: 0,
            });
            if e.started == caller.started_at && e.info.seq != 0 && info.seq <= e.info.seq {
                return;
            }
            e.info = info;
            e.started = caller.started_at;
            e.last_seen = now();
        }
        self.prune();
        self.changed();
    }

    /// Schedules one recompute and push of the models 200 ms on, and one of the node list.
    fn changed(&self) {
        self.list_changed();
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        let mut st = lock(&self.st);
        if st.timer.is_some() {
            return;
        }
        let me = self.me.clone();
        st.timer = after_send(COALESCE, move || {
            if let Some(h) = me.upgrade() {
                lock(&h.st).timer = None;
                h.models();
            }
        })
        .ok();
    }

    /// Schedules a push of the node list: 200 ms on, and no sooner than a second after the last.
    fn list_changed(&self) {
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        let mut st = lock(&self.st);
        if st.list.timer.is_some() {
            return;
        }
        let gap = st
            .list
            .at
            .map_or(Duration::ZERO, |at| LIST_GAP.saturating_sub(at.elapsed()));
        let me = self.me.clone();
        st.list.timer = after_send(COALESCE.max(gap), move || {
            if let Some(h) = me.upgrade() {
                lock(&h.st).list.timer = None;
                h.node_list();
            }
        })
        .ok();
    }

    /// Every 5 s while proxies are linked, the node list's traffic and latency go out again.
    fn arm_refresh(&self) {
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        let me = self.me.clone();
        let t = after_send(LIST_REFRESH, move || {
            if let Some(h) = me.upgrade() {
                if !h.base.mesh.peers_of(ROLE_PROXY).is_empty() {
                    h.list_changed();
                }
                h.arm_refresh();
            }
        });
        lock(&self.st).tick = t.ok();
    }

    fn aggregate(&self, st: &St) -> Vec<HubModel> {
        let mut out: Vec<HubModel> = Vec::new();
        for (id, e) in &st.nodes {
            if !self.base.mesh.online(id) {
                continue;
            }
            for mv in &e.info.models {
                if mv.state == MODEL_DISABLED {
                    continue;
                }
                let i = match out
                    .iter()
                    .position(|h| h.name == mv.name && h.api == mv.api)
                {
                    Some(i) => i,
                    None => {
                        out.push(HubModel {
                            name: mv.name.clone(),
                            api: mv.api.clone(),
                            ..HubModel::default()
                        });
                        out.len().saturating_sub(1)
                    }
                };
                if let Some(h) = out.get_mut(i) {
                    if !h.nodes.contains(id) {
                        h.nodes.push(id.clone());
                    }
                    if mv.ready() {
                        h.ready = h.ready.saturating_add(1);
                    }
                }
            }
        }
        for h in &mut out {
            h.nodes.sort();
        }
        out.sort_by(|a, b| (&a.name, &a.api).cmp(&(&b.name, &b.api)));
        out
    }

    /// The model list; when it changed, its sequence grows and every proxy gets it.
    fn models(&self) -> HubModels {
        let hub = self.base.id();
        let changed = {
            let mut st = lock(&self.st);
            let list = self.aggregate(&st);
            if list == st.models.models {
                None
            } else {
                st.models = HubModels {
                    hub: hub.clone(),
                    models: list,
                    seq: st.models.seq.saturating_add(1).max(now_ms()),
                };
                Some(st.models.clone())
            }
        };
        match changed {
            Some(cur) => {
                let body = cur.to_json();
                let m = &self.base.mesh;
                for p in m.peers_of(ROLE_PROXY) {
                    let _ = m.push(&p.id, METHOD_HUB_MODELS_UPDATE, &body);
                }
                cur
            }
            None => {
                let mut cur = lock(&self.st).models.clone();
                cur.hub = hub;
                cur
            }
        }
    }

    /// The node table for the hub's pages (fit's `Nodes`): online nodes first, then by name and
    /// ID.
    fn nodes(&self) -> Vec<NodeView> {
        let mut conns: HashMap<String, Vec<PeerConn>> = HashMap::new();
        for p in self.base.mesh.peers() {
            if p.identity.role == ROLE_NODE {
                conns.entry(p.identity.id.clone()).or_default().push(p);
            }
        }
        let t = now();
        let mut out: Vec<NodeView> = {
            let st = lock(&self.st);
            st.nodes
                .iter()
                .map(|(id, e)| {
                    let online = self.base.mesh.online(id);
                    NodeView {
                        info: e.info.clone(),
                        online,
                        conns: conns.remove(id).unwrap_or_default(),
                        last_seen: if online { t } else { e.last_seen },
                    }
                })
                .collect()
        };
        out.sort_by(|a, b| {
            b.online
                .cmp(&a.online)
                .then_with(|| a.info.name.cmp(&b.info.name))
                .then_with(|| a.info.id.cmp(&b.info.id))
        });
        out
    }

    /// `/hub/nodes`: fit's `[]HubNode`.
    fn nodes_json(&self) -> String {
        json::array(self.nodes().iter().map(|n| {
            let mut o = Obj::new()
                .raw("info", &n.info.to_json())
                .bool("online", n.online);
            if !n.conns.is_empty() {
                o = o.raw("conns", &json::array(n.conns.iter().map(PeerConn::to_json)));
            }
            o.int("last_seen", n.last_seen).end()
        }))
    }

    /// The node list for the proxies (fit's `NodeList`); when it changed since the last one,
    /// its sequence grows and every proxy gets it.
    fn node_list(&self) -> String {
        let traffic = traffic_by_node(self.base.mesh.routes());
        let all = self.nodes();
        let total = all.len();
        let enc = encode_list(
            all.iter()
                .map(|n| summary(n, traffic.get(&n.info.id).copied().unwrap_or_default()))
                .collect(),
        );
        let hub = self.base.id();
        let (seq, changed) = {
            let mut st = lock(&self.st);
            if st.list.nodes == enc && st.list.total == total {
                (st.list.seq, false)
            } else {
                st.list.seq = st.list.seq.saturating_add(1).max(now_ms());
                st.list.nodes.clone_from(&enc);
                st.list.total = total;
                st.list.at = Some(Instant::now());
                (st.list.seq, true)
            }
        };
        let body = Obj::new()
            .str("hub", &hub)
            .raw("nodes", &enc)
            .int("total", i64::try_from(total).unwrap_or(0))
            .int("seq", seq)
            .end();
        if changed {
            let m = &self.base.mesh;
            for p in m.peers_of(ROLE_PROXY) {
                let _ = m.push(&p.id, METHOD_HUB_NODE_LIST_UPDATE, &body);
            }
        }
        body
    }

    // ---- The pages' calls ----

    /// A node's name for the pages' errors: `name (id)` when it has one of its own.
    fn node_name(&self, id: &str) -> String {
        match lock(&self.st).nodes.get(id) {
            Some(e) if !e.info.name.is_empty() && e.info.name != id => {
                format!("{} ({id})", e.info.name)
            }
            _ => id.to_owned(),
        }
    }

    /// The errors of managed calls, made clear on the pages.
    fn manage_err(&self, id: &str, e: Error) -> Error {
        if e == offline() {
            return Error::code(
                CODE_UNAVAILABLE,
                format!("node {} is offline", self.node_name(id)),
            );
        }
        match e {
            Error::Remote { code, msg } if code == CODE_FORBIDDEN => Error::code(
                CODE_FORBIDDEN,
                format!(
                    "node {} does not allow hub management (enable allow_hub_manage on the node): {msg}",
                    self.node_name(id)
                ),
            ),
            e @ Error::Remote { .. } => e,
            e => Error::Other(format!("node {}: {e}", self.node_name(id))),
        }
    }

    /// `/hub/node/call`: a node management method, not a stream one, called on the node.
    fn node_call(&self, req: &[u8], a: Answer) {
        let r = match Req::parse(req) {
            Ok(r) => r,
            Err(e) => return a.err(e),
        };
        let (id, method) = match (r.str("id"), r.str("method")) {
            (Ok(i), Ok(m)) => (i, m),
            (Err(e), _) | (_, Err(e)) => return a.err(e),
        };
        if !NODE_MANAGE_METHODS.contains(&method.as_str())
            || NODE_STREAM_METHODS.contains(&method.as_str())
        {
            return a.err(bad_request(format!(
                "{method:?} is not a node management method"
            )));
        }
        let me = self.me.clone();
        let peer = id.clone();
        self.base.mesh.call(
            &peer,
            &method,
            &r.raw("req"),
            NODE_CALL_TIMEOUT,
            move |res| match res {
                Ok(b) => a.ok(String::from_utf8_lossy(&b).into_owned()),
                Err(e) => match me.upgrade() {
                    Some(h) => a.err(h.manage_err(&id, e)),
                    None => a.err(e),
                },
            },
        );
    }

    /// `/hub/node/stream`: a node management stream method opened on the node, its frames and
    /// result relayed as they are.
    fn node_stream(&self, req: &[u8], st: Stream) {
        let r = match Req::parse(req) {
            Ok(r) => r,
            Err(e) => return st.end(Err(e)),
        };
        let (id, method) = match (r.str("id"), r.str("method")) {
            (Ok(i), Ok(m)) => (i, m),
            (Err(e), _) | (_, Err(e)) => return st.end(Err(e)),
        };
        if !NODE_STREAM_METHODS.contains(&method.as_str()) {
            return st.end(Err(bad_request(format!(
                "{method:?} is not a node management stream method"
            ))));
        }
        let mut payload = r.raw("req");
        if payload.is_empty() {
            "{}".clone_into(&mut payload);
        }
        let ns = match self.base.mesh.open_stream(&id, &method, &payload) {
            Ok(s) => s,
            Err(e) => return st.end(Err(self.manage_err(&id, e))),
        };
        // The pages' stream ending (they canceled, the pipe broke) cancels the node's.
        let ns2 = ns.clone();
        st.on_finish(move |e| {
            if e.is_some_and(|e| *e != Error::StreamEnded) {
                ns2.cancel();
            }
        });
        let (me, st2, ns3) = (self.me.clone(), st.clone(), ns.clone());
        pump(&ns, &st, None, move |res, _| match res {
            Ok(()) => st2.end(Ok(ns3.result().unwrap_or_default())),
            Err(e) => {
                ns3.cancel();
                let e = match me.upgrade() {
                    Some(h) => h.manage_err(&id, e),
                    None => e,
                };
                st2.end(Err(e));
            }
        });
    }

    /// `/hub/settings/get`: fit's `HubSettings`, its defaults filled in.
    fn settings(&self) -> String {
        let n = lock(&self.st).max_nodes;
        Obj::new()
            .int("max_nodes", i64::try_from(n).unwrap_or(i64::MAX))
            .end()
    }

    /// `/hub/settings/set`: the settings written into the member, answered once applied.
    fn set_settings(&self, req: &[u8], a: Answer) {
        let n = match Req::parse(req)
            .and_then(|r| r.int("max_nodes"))
            .and_then(|n| {
                count_setting(
                    "max_nodes",
                    n,
                    i64::try_from(akit_config::roles::DEFAULT_LLM_MAX_NODES).unwrap_or(0),
                )
            }) {
            Ok(n) => n,
            Err(e) => return a.err(e),
        };
        let me = self.me.clone();
        self.base.edit(
            move |m| {
                set(m, "max_nodes", number(n));
                Ok(())
            },
            move |r| match (r, me.upgrade()) {
                (Ok(()), Some(h)) => a.ok(h.settings()),
                (Ok(()), None) => a.err(Error::Closed),
                (Err(e), _) => a.err(e),
            },
        );
    }

    /// `/hub/profile/set`: the hub's name (1 to 64 characters), answered with its identity once
    /// applied.
    fn set_profile(&self, req: &[u8], a: Answer) {
        let name = match Req::parse(req).and_then(|r| r.str("name")) {
            Ok(n) => n.trim().to_owned(),
            Err(e) => return a.err(e),
        };
        if name.is_empty() || name.chars().count() > 64 {
            return a.err(bad_request("the name needs 1-64 characters"));
        }
        let me = self.me.clone();
        self.base.set_name(name, move |r| match (r, me.upgrade()) {
            (Ok(()), Some(h)) => a.ok(h.base.mesh.identity().to_json()),
            (Ok(()), None) => a.err(Error::Closed),
            (Err(e), _) => a.err(e),
        });
    }

    /// The online node serving `name` ready to agents of `api`, not excluded, with the fewest
    /// calls in flight (those it reported and those relayed to it now); ties round robin.
    fn pick(&self, api: &str, name: &str, exclude: &HashSet<String>) -> Option<String> {
        let mut st = lock(&self.st);
        let mut best: Option<i64> = None;
        let mut ties: Vec<String> = Vec::new();
        for (id, e) in &st.nodes {
            if exclude.contains(id) || !self.base.mesh.online(id) {
                continue;
            }
            let Some(mv) = e
                .info
                .models
                .iter()
                .find(|m| m.name == name && protocol(&m.api) == api && m.ready())
            else {
                continue;
            };
            let load = mv
                .inflight
                .saturating_add(st.inflight.get(id).copied().unwrap_or(0));
            match best {
                Some(b) if load > b => {}
                Some(b) if load == b => ties.push(id.clone()),
                _ => {
                    best = Some(load);
                    ties = vec![id.clone()];
                }
            }
        }
        if ties.is_empty() {
            return None;
        }
        ties.sort();
        st.rr = st.rr.wrapping_add(1);
        ties.get(st.rr.checked_rem(ties.len()).unwrap_or(0))
            .cloned()
    }

    fn add_inflight(&self, id: &str, d: i64) {
        let mut st = lock(&self.st);
        let n = st.inflight.entry(id.to_owned()).or_insert(0);
        *n = n.saturating_add(d);
        if *n <= 0 {
            st.inflight.remove(id);
        }
    }

    /// Serves `/hub/proxy`.
    fn proxy(&self, caller: &Identity, payload: &[u8], st: Stream) {
        let mut req = match ProxyReq::parse(payload) {
            Ok(r) => r,
            Err(e) => return st.end(Err(bad_request(format!("bad request: {e}")))),
        };
        req.api = protocol(&req.api).to_owned();
        if req.model.is_empty() || !PROTOCOLS.contains(&req.api.as_str()) {
            return st.end(Err(bad_request("model and a known api are required")));
        }
        let Some(core) = self.me.upgrade() else {
            return st.end(Err(Error::Closed));
        };
        let keep = st.keep_alive(Duration::ZERO);
        let r = Arc::new(Relay {
            core,
            st,
            from: caller.id.clone(),
            exclude: Mutex::new(req.exclude.drain(..).collect()),
            req,
            tries: Mutex::new(0),
            taken: Arc::new(AtomicBool::new(false)),
            keep,
            last: Mutex::new(None),
        });
        r.next();
    }
}

/// One call relayed: the proxy's stream, the nodes tried.
struct Relay {
    core: Arc<Core>,
    st: Stream,
    from: String,
    req: ProxyReq,
    exclude: Mutex<HashSet<String>>,
    tries: Mutex<usize>,
    /// Some of the proxy's streamed body was taken: the call cannot go to another node.
    taken: Arc<AtomicBool>,
    keep: Arc<AtomicBool>,
    last: Mutex<Option<Error>>,
}

impl Relay {
    fn end(&self, r: Result<Vec<u8>, Error>) {
        self.keep.store(true, Ordering::Release);
        self.st.end(r);
    }

    /// Tries the next node, or ends the call.
    fn next(self: &Arc<Self>) {
        let n = {
            let mut t = lock(&self.tries);
            *t = t.saturating_add(1);
            *t
        };
        let picked = if n > MAX_TRIES || self.st.is_finished() {
            None
        } else {
            let ex = lock(&self.exclude).clone();
            self.core.pick(&self.req.api, &self.req.model, &ex)
        };
        let Some(id) = picked else {
            let msg = match lock(&self.last).take() {
                Some(e) => format!("no node could serve model {}: {e}", self.req.model),
                None => format!("no node serves model {}", self.req.model),
            };
            return self.end(Err(Error::code(CODE_UNAVAILABLE, msg)));
        };
        lock(&self.exclude).insert(id.clone());
        let nreq = ProxyReq {
            exclude: Vec::new(),
            ..self.req.clone()
        };
        let ns = match self
            .core
            .base
            .mesh
            .open_stream(&id, METHOD_NODE_PROXY, &nreq.to_json())
        {
            Ok(s) => s,
            Err(e) => {
                *lock(&self.last) = Some(e);
                return self.next();
            }
        };
        self.core.add_inflight(&id, 1);
        // The call is counted on its route from the proxy to the node, its bytes as they cross
        // the node's stream.
        let rc = self
            .core
            .base
            .mesh
            .routes()
            .begin(&self.from, &id, &self.req.model);
        rc.add_req(self.req.body.len());
        rc.attach(&ns, false);
        // The proxy's stream ending (it canceled, its link broke) cancels the node's.
        let ns2 = ns.clone();
        self.st.on_finish(move |e| {
            if e.is_some_and(|e| *e != Error::StreamEnded) {
                ns2.cancel();
            }
        });
        if self.req.body_stream {
            // The proxy's body goes on to the node; its end half-closes the node's side.
            let taken = Arc::clone(&self.taken);
            let ns3 = ns.clone();
            pump(
                &self.st,
                &ns,
                Some(Box::new(move || taken.store(true, Ordering::Release))),
                move |r, _| {
                    if r.is_ok() {
                        let _ = ns3.close_send();
                    }
                },
            );
        }
        let keep = Arc::clone(&self.keep);
        let me = Arc::clone(self);
        let ns4 = ns.clone();
        pump(
            &ns,
            &self.st,
            Some(Box::new(move || keep.store(true, Ordering::Release))),
            move |r, frames| {
                rc.end(r.is_err() || frames == 0);
                me.node_ended(&id, &ns4, r, frames);
            },
        );
    }

    fn node_ended(self: &Arc<Self>, id: &str, ns: &Stream, r: Result<(), Error>, frames: usize) {
        self.core.add_inflight(id, -1);
        match r {
            Ok(()) if frames == 0 => self.end(Err(Error::code(
                CODE_INTERNAL,
                format!("node {id} ended the call without a head"),
            ))),
            Ok(()) => self.end(Ok(ns.result().unwrap_or_default())),
            Err(e) if frames == 0 && !self.st.is_finished() => {
                // Nothing reached the proxy: another node may serve it, if this one could not,
                // or its link broke, and none of a streamed body was taken.
                let retry = match &e {
                    Error::Remote { code, .. } => *code == CODE_UNAVAILABLE,
                    _ => true,
                } && !self.taken.load(Ordering::Acquire);
                ns.cancel();
                if retry {
                    akit_observe::info!(
                        Llm,
                        "llm.hub: node {id} could not serve {} for {}, trying another: {e}",
                        self.req.model,
                        self.from
                    );
                    *lock(&self.last) = Some(e);
                    self.next();
                } else {
                    self.end(Err(e));
                }
            }
            Err(e) => {
                ns.cancel();
                self.end(Err(e));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;
    use crate::hw::{DiskInfo, GpuInfo};
    use crate::model::{LinkStats, ModelView, Rtt};
    use akit_config::value::Value;

    /// fit's `TestNodeSummary`: a node of the list shows its hardware without disks and
    /// addresses, at most 16 GPUs and 64 models, its links and traffic.
    #[test]
    fn node_summary() {
        let hw = HwInfo {
            arch: "amd64".to_owned(),
            disks: Some(vec![DiskInfo {
                mount: "/".to_owned(),
                ..DiskInfo::default()
            }]),
            ips: Some(vec!["10.0.0.1".to_owned()]),
            gpus: Some(vec![GpuInfo::default(); 20]),
            ..HwInfo::default()
        };
        let mut info = NodeInfo {
            id: "n".to_owned(),
            name: "nn".to_owned(),
            description: "d".to_owned(),
            inflight: 2,
            jobs: 1,
            hw: hw.to_json(),
            ..NodeInfo::default()
        };
        for i in 0..MAX_NODE_LIST_MODELS + 6 {
            info.models.push(ModelView {
                name: format!("m{i}"),
                api: "openai".to_owned(),
                state: "ready".to_owned(),
                ..ModelView::default()
            });
        }
        let n = NodeView {
            info,
            online: true,
            last_seen: 7,
            conns: vec![PeerConn {
                direction: "in".to_owned(),
                connected_at: 5,
                link: LinkStats {
                    bytes_in: 3,
                    rtt: Rtt {
                        samples: 1,
                        ..Rtt::default()
                    },
                    ..LinkStats::default()
                },
                ..PeerConn::default()
            }],
        };
        let s = summary(
            &n,
            Traffic {
                calls: 4,
                ..Traffic::default()
            },
        );
        let v = json::parse(s.as_bytes()).unwrap();
        let h = v.get("hw").unwrap();
        assert_eq!(h.get("disks"), Some(&Value::Null));
        assert_eq!(h.get("ips"), Some(&Value::Null));
        assert_eq!(json::get_arr(h, "gpus").len(), MAX_LIST_GPUS);
        assert_eq!(json::get_str(h, "arch"), "amd64");
        assert_eq!(json::get_arr(&v, "models").len(), MAX_NODE_LIST_MODELS);
        assert_eq!(
            json::get_int(&v, "models_total"),
            i64::try_from(MAX_NODE_LIST_MODELS + 6).unwrap()
        );
        assert_eq!(json::get_int(v.get("traffic").unwrap(), "calls"), 4);
        let links = json::get_arr(&v, "links");
        assert_eq!(links.len(), 1);
        assert_eq!(json::get_int(&links[0], "connected_at"), 5);
        assert_eq!(json::get_int(links[0].get("link").unwrap(), "bytes_in"), 3);
        assert!(json::get_bool(&v, "online"));
        assert_eq!(
            (json::get_int(&v, "inflight"), json::get_int(&v, "jobs")),
            (2, 1)
        );
        // No models is an empty list; no links, none at all.
        let empty = NodeView {
            info: NodeInfo::default(),
            online: false,
            conns: Vec::new(),
            last_seen: 0,
        };
        let s = summary(&empty, Traffic::default());
        assert!(s.contains(r#""models":[]"#) && !s.contains("links"), "{s}");
    }

    /// fit's `TestEncodeListBound`: a list past 4 MiB leaves out the nodes at its end.
    #[test]
    fn encode_list_bound() {
        let desc = "d".repeat(1024);
        let nodes: Vec<String> = (0..6000)
            .map(|i| {
                Obj::new()
                    .str("id", &format!("n{i}"))
                    .str("description", &desc)
                    .end()
            })
            .collect();
        let enc = encode_list(nodes.clone());
        let back = json::parse(enc.as_bytes()).unwrap();
        let Value::Array(kept) = back else {
            panic!("not a list")
        };
        assert!(enc.len() <= MAX_NODE_LIST_BYTES);
        assert!(!kept.is_empty() && kept.len() < nodes.len());
        assert_eq!(json::get_str(&kept[0], "id"), "n0");
        let small = encode_list(nodes[..3].to_vec());
        assert_eq!(small, json::array(nodes[..3].iter().cloned()));
        assert_eq!(encode_list(Vec::new()), "[]");
    }
}
