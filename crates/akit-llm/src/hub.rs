//! fit's LLM hub (`llm/hub`, I106): the table of its nodes (what they push), the models of its
//! online nodes, pushed to its proxies when they change, and `/hub/proxy`: a proxy's call relayed
//! to the least loaded node serving the model, frame by frame under each stream's credit
//! ([`pump`]), another node tried (3 at most) while nothing reached the proxy.
//!
//! The hub's pages' methods (`/hub/nodes`, `/hub/node/call`, `/hub/node/stream`, its settings)
//! are I109's.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use akio::timer::{SendTimer, after_send};
use akit_config::Config;
use akit_roles::host::{Host, Info, Role};
use akit_roles::linktoken::Registry;

use crate::json::{self, Obj};
use crate::mesh::{Answer, Router, bad_request};
use crate::model::{
    CODE_INTERNAL, CODE_UNAVAILABLE, HubModel, HubModels, Identity, METHOD_HUB_MODELS,
    METHOD_HUB_MODELS_UPDATE, METHOD_HUB_NODE_LIST, METHOD_HUB_NODE_LIST_UPDATE, METHOD_HUB_PROXY,
    METHOD_NODE_INFO, METHOD_NODE_PROXY, METHOD_NODE_UPDATE, MODEL_DISABLED, NodeInfo, PROTOCOLS,
    PeerConn, ProxyReq, ROLE_NODE, ROLE_PROXY, protocol,
};
use crate::role::Base;
use crate::wire::{Error, Stream, pump};

/// `llm.hub`.
pub const HUB: Info = Info {
    id: "llm.hub",
    set: |c| c.llm.as_ref().is_some_and(|l| l.hub.is_some()),
    new,
};

/// Model changes gathered into one push.
const COALESCE: Duration = Duration::from_millis(200);
/// The nodes one call is tried on.
const MAX_TRIES: usize = 3;
/// The models a node lists in the node list.
const MAX_NODE_LIST_MODELS: usize = 64;

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

struct St {
    nodes: HashMap<String, Entry>,
    models: HubModels,
    list_seq: i64,
    inflight: HashMap<String, i64>,
    rr: usize,
    timer: Option<SendTimer>,
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

/// Makes the hub role; its first apply starts it.
///
/// # Errors
///
/// None.
pub fn new(host: Host) -> Result<Box<dyn Role>, String> {
    let core = Arc::new_cyclic(|me: &Weak<Core>| {
        let mut r = Router::new();
        let (a, b, c) = (me.clone(), me.clone(), me.clone());
        r.handle(
            METHOD_HUB_MODELS,
            &[ROLE_PROXY],
            Duration::ZERO,
            move |_, _, ans: Answer| match a.upgrade() {
                Some(h) => ans.ok(h.models().to_json()),
                None => ans.err(Error::Closed),
            },
        );
        r.handle(
            METHOD_HUB_NODE_LIST,
            &[ROLE_PROXY],
            Duration::ZERO,
            move |_, _, ans: Answer| match c.upgrade() {
                Some(h) => ans.ok(h.node_list()),
                None => ans.err(Error::Closed),
            },
        );
        r.handle_stream(
            METHOD_HUB_PROXY,
            &[ROLE_PROXY],
            move |caller, req, st| match b.upgrade() {
                Some(h) => h.proxy(caller, req, st),
                None => st.end(Err(Error::Closed)),
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
                list_seq: now_ms(),
                inflight: HashMap::new(),
                rr: 0,
                timer: None,
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
        if let Some(t) = lock(&self.0.st).timer.take() {
            t.cancel();
        }
        self.0.base.stop(grace);
    }

    fn tokens(&self) -> Option<Registry> {
        Some(self.0.base.mesh.tokens())
    }
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

    /// Schedules one recompute and push of the models and the node list, 200 ms on.
    fn changed(&self) {
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
                h.push_list();
            }
        })
        .ok();
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

    /// The node list proxies show: online nodes first, each with 64 models at most.
    fn node_list(&self) -> String {
        let t = now();
        let mut st = lock(&self.st);
        st.list_seq = st.list_seq.saturating_add(1).max(now_ms());
        let mut nodes: Vec<(bool, String, String)> = st
            .nodes
            .iter()
            .map(|(id, e)| {
                let online = self.base.mesh.online(id);
                let models = &e.info.models;
                let shown: Vec<String> = models
                    .iter()
                    .take(MAX_NODE_LIST_MODELS)
                    .map(crate::model::ModelView::to_json)
                    .collect();
                let j = Obj::new()
                    .str("id", id)
                    .str("name", &e.info.name)
                    .str_omit("description", &e.info.description)
                    .bool("online", online)
                    .int("last_seen", if online { t } else { e.last_seen })
                    .raw(
                        "hw",
                        if e.info.hw.is_empty() {
                            "{}"
                        } else {
                            &e.info.hw
                        },
                    )
                    .raw("models", &json::array(shown))
                    .int("models_total", i64::try_from(models.len()).unwrap_or(0))
                    .int("inflight", e.info.inflight)
                    .int("jobs", e.info.jobs)
                    .raw(
                        "traffic",
                        r#"{"calls":0,"errors":0,"inflight":0,"req_bytes":0,"resp_bytes":0}"#,
                    )
                    .end();
                (online, e.info.name.clone(), j)
            })
            .collect();
        nodes.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let total = nodes.len();
        Obj::new()
            .str("hub", &self.base.id())
            .raw("nodes", &json::array(nodes.into_iter().map(|n| n.2)))
            .int("total", i64::try_from(total).unwrap_or(0))
            .int("seq", st.list_seq)
            .end()
    }

    fn push_list(&self) {
        let body = self.node_list();
        let m = &self.base.mesh;
        for p in m.peers_of(ROLE_PROXY) {
            let _ = m.push(&p.id, METHOD_HUB_NODE_LIST_UPDATE, &body);
        }
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
            move |r, frames| me.node_ended(&id, &ns4, r, frames),
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
