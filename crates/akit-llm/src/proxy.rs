//! fit's LLM proxy (`llm/proxy`, I107): the agents' API on `api.address`, an HTTP listener of its
//! own on the listeners' engine, with OpenAI's, Anthropic's, Gemini's, Ollama's and Cohere's
//! paths and model lists, checked by the API keys of the member (their SHA-256 kept, compared in
//! constant time); each call routed to the nodes the proxy links to directly, then to its hubs
//! (4 routes at most before the response's head), streamed back as it comes with the node's ID in
//! `X-Akit-Node`, and failed in the shape of the agent's API.
//!
//! Its pages' methods (its keys made and deleted, its API's settings, its stats, its models and
//! nodes) are [`methods`]' (I109d).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use akio::timer::{SendTimer, after_send};
use akio::{ConnHandle, ListenerId};
use akio_http::{BodyStream, Context, Detached, Headers, Method, Response};
use akit_config::Config;
use akit_config::roles::{LlmProxy, LlmProxyApi, LlmProxyKey};
use akit_roles::host::{Host, Info, Role};
use akit_roles::linktoken::Registry;

use crate::form;
use crate::json::{self, Obj};
use crate::mesh::{RouteCall, Router};
use crate::model::{
    API_ANTHROPIC, API_COHERE, API_GEMINI, API_OLLAMA, API_OPENAI, CODE_BAD_REQUEST, CODE_INTERNAL,
    CODE_NOT_FOUND, CODE_RATE_LIMITED, CODE_UNAVAILABLE, HubModels, Identity, METHOD_HUB_MODELS,
    METHOD_HUB_MODELS_UPDATE, METHOD_HUB_NODE_LIST, METHOD_HUB_NODE_LIST_UPDATE, METHOD_HUB_PROXY,
    METHOD_NODE_INFO, METHOD_NODE_PROXY, METHOD_NODE_UPDATE, NodeInfo, PROXY_HEADERS, PeerConn,
    ProxyHead, ProxyReq, ROLE_HUB, ROLE_NODE, protocol, proxy_paths,
};
use crate::role::Base;
use crate::wire::{Error, Event, Stream};

/// `llm.proxy`.
pub const PROXY: Info = Info {
    id: "llm.proxy",
    set: |c| c.llm.as_ref().is_some_and(|l| l.proxy.is_some()),
    new,
};

/// The largest body sent in the call's open itself; a larger one follows in frames.
const INLINE_MAX: usize = 1 << 20;
/// The frames a larger body is sent in.
const FRAME_SIZE: usize = 64 << 10;
/// The routes one call is tried on.
const MAX_ATTEMPTS: usize = 4;
/// The longest model name a call gives.
const MAX_MODEL_NAME: usize = 256;
/// An agent's response is held from the stream once this much waits for the agent.
const WRITE_HIGH: usize = 256 << 10;
/// The node's ID in a response (decision 038's name).
pub const NODE_HEADER: &str = "X-Akit-Node";

pub mod methods;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- The catalog: what the hubs and the direct nodes serve ----

#[derive(Default)]
struct Catalog {
    hubs: HashMap<String, (Identity, HubModels)>,
    nodes: HashMap<String, (Identity, NodeInfo)>,
    /// The hubs' node lists, by hub ID, for the pages.
    lists: HashMap<String, (Identity, methods::HubList)>,
    inflight: HashMap<String, i64>,
    rr: usize,
}

/// A way to a model: a node linked directly, or a hub that picks one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RouteTo {
    hub: bool,
    id: String,
}

impl Catalog {
    fn set_hub(&mut self, peer: &Identity, hm: HubModels) {
        if let Some((old, o)) = self.hubs.get(&peer.id) {
            if old.started_at == peer.started_at && hm.seq <= o.seq {
                return;
            }
        }
        self.hubs.insert(peer.id.clone(), (peer.clone(), hm));
    }

    fn set_node(&mut self, peer: &Identity, info: NodeInfo) {
        if let Some((old, o)) = self.nodes.get(&peer.id) {
            if old.started_at == peer.started_at && info.seq <= o.seq {
                return;
            }
        }
        self.nodes.insert(peer.id.clone(), (peer.clone(), info));
    }

    /// Keeps hub `peer`'s node list, unless it is older than the one kept (fit's `setList`).
    fn set_list(&mut self, peer: &Identity, l: methods::HubList) {
        if let Some((old, o)) = self.lists.get(&peer.id) {
            if old.started_at == peer.started_at && l.seq < o.seq {
                return;
            }
        }
        self.lists.insert(peer.id.clone(), (peer.clone(), l));
    }

    fn drop_peer(&mut self, id: &str) {
        self.hubs.remove(id);
        self.nodes.remove(id);
        self.lists.remove(id);
    }

    /// The routes to `name` called with `api`: the ready direct nodes, least loaded first, then
    /// the hubs with a ready node, round robin; whether the name is known at all, and another
    /// API it is called with when not with `api`.
    fn routes(&mut self, name: &str, api: &str) -> (Vec<RouteTo>, bool, String) {
        let (mut known, mut other, mut same) = (false, String::new(), false);
        let mut nodes: Vec<(i64, String)> = Vec::new();
        for (id, (_, info)) in &self.nodes {
            for m in info.models.iter().filter(|m| m.name == name) {
                known = true;
                let p = protocol(&m.api);
                if p != api {
                    if other.is_empty() {
                        other = p.to_owned();
                    }
                    continue;
                }
                same = true;
                if m.ready() {
                    let load = m
                        .inflight
                        .saturating_add(self.inflight.get(id).copied().unwrap_or(0));
                    nodes.push((load, id.clone()));
                }
            }
        }
        nodes.sort();
        let mut out: Vec<RouteTo> = nodes
            .into_iter()
            .map(|(_, id)| RouteTo { hub: false, id })
            .collect();
        let mut hubs: Vec<String> = Vec::new();
        for (id, (_, hm)) in &self.hubs {
            for m in hm.models.iter().filter(|m| m.name == name) {
                known = true;
                let p = protocol(&m.api);
                if p != api {
                    if other.is_empty() {
                        other = p.to_owned();
                    }
                    continue;
                }
                same = true;
                if m.ready > 0 && !hubs.contains(id) {
                    hubs.push(id.clone());
                }
            }
        }
        if !hubs.is_empty() {
            hubs.sort();
            self.rr = self.rr.wrapping_add(1);
            let start = self.rr.checked_rem(hubs.len()).unwrap_or(0);
            hubs.rotate_left(start);
            out.extend(hubs.into_iter().map(|id| RouteTo { hub: true, id }));
        }
        if same {
            other.clear();
        }
        (out, known, other)
    }

    /// The names of the models reached, once each, sorted; those called with `api` alone unless
    /// it is empty.
    fn names(&self, api: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let node_models = self
            .nodes
            .values()
            .flat_map(|(_, i)| i.models.iter().map(|m| (m.name.clone(), m.api.clone())));
        let hub_models = self
            .hubs
            .values()
            .flat_map(|(_, h)| h.models.iter().map(|m| (m.name.clone(), m.api.clone())));
        for (n, a) in node_models.chain(hub_models) {
            if (api.is_empty() || protocol(&a) == api) && !out.contains(&n) {
                out.push(n);
            }
        }
        out.sort();
        out
    }
}

// ---- The role ----

struct Api {
    cfg: LlmProxyApi,
    id: ListenerId,
    conns: Arc<Conns>,
}

#[derive(Default)]
struct Conns {
    max: usize,
    open: Mutex<HashMap<u64, ConnHandle>>,
}

struct Core {
    base: Base,
    me: Weak<Core>,
    cat: Mutex<Catalog>,
    keys: Mutex<Vec<LlmProxyKey>>,
    api: Mutex<Option<Api>>,
    api_cfg: Mutex<LlmProxyApi>,
    policy: Mutex<Option<Arc<akit_access::Policy>>>,
    inflight: AtomicUsize,
    /// The calls counted per model, for the pages.
    stats: methods::Stats,
    /// Each key's last use and calls since the start, by its ID.
    usage: Mutex<HashMap<String, (i64, i64)>>,
    /// The API the member asks for, as the pages read it.
    api_want: Mutex<Option<LlmProxyApi>>,
}

struct Proxy(Arc<Core>);

impl std::fmt::Debug for Proxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Proxy").finish_non_exhaustive()
    }
}

/// Makes the proxy role; its first apply starts it.
///
/// # Errors
///
/// None.
pub fn new(host: Host) -> Result<Box<dyn Role>, String> {
    let core = Arc::new_cyclic(|me: &Weak<Core>| {
        let mut r = Router::new();
        methods::register(&mut r, me);
        Core {
            base: Base::new(host, "proxy", r),
            me: me.clone(),
            cat: Mutex::new(Catalog::default()),
            keys: Mutex::new(Vec::new()),
            api: Mutex::new(None),
            api_cfg: Mutex::new(LlmProxyApi::default()),
            policy: Mutex::new(None),
            inflight: AtomicUsize::new(0),
            stats: methods::Stats::default(),
            usage: Mutex::new(HashMap::new()),
            api_want: Mutex::new(None),
        }
    });
    // The pages' `GET /api/me` says where the agents' API is (fit's `me`).
    let w = Arc::downgrade(&core);
    core.base.pages.set_me(Arc::new(move || {
        let Some(c) = w.upgrade() else {
            return Vec::new();
        };
        let Some(addr) = c.api_addr() else {
            return Vec::new();
        };
        let secure = !lock(&c.api_cfg).cert_file.is_empty();
        let url = format!("{}://{addr}", if secure { "https" } else { "http" });
        vec![("api_url", json::string(&url))]
    }));
    let m = &core.base.mesh;
    let w = Arc::downgrade(&core);
    m.on_push(METHOD_HUB_MODELS_UPDATE, &[ROLE_HUB], move |caller, msg| {
        if let (Some(c), Ok(v)) = (w.upgrade(), json::parse(msg)) {
            lock(&c.cat).set_hub(caller, HubModels::from_value(&v));
        }
    });
    let w = Arc::downgrade(&core);
    m.on_push(
        METHOD_HUB_NODE_LIST_UPDATE,
        &[ROLE_HUB],
        move |caller, msg| match (w.upgrade(), json::parse(msg)) {
            (Some(c), Ok(v)) => lock(&c.cat).set_list(caller, methods::HubList::from_value(&v)),
            (_, Err(e)) => akit_observe::warn!(
                Llm,
                "llm.proxy: a bad node list from hub {}: {e}",
                caller.id
            ),
            _ => {}
        },
    );
    let w = Arc::downgrade(&core);
    m.on_push(METHOD_NODE_UPDATE, &[ROLE_NODE], move |caller, msg| {
        if let (Some(c), Ok(v)) = (w.upgrade(), json::parse(msg)) {
            lock(&c.cat).set_node(caller, NodeInfo::from_value(&v));
        }
    });
    let w = Arc::downgrade(&core);
    m.on_peer_up(move |p| {
        if let Some(c) = w.upgrade() {
            c.peer_up(p);
        }
    });
    let w = Arc::downgrade(&core);
    m.on_peer_down(move |p| {
        if let Some(c) = w.upgrade() {
            if !c.base.mesh.online(&p.identity.id) {
                lock(&c.cat).drop_peer(&p.identity.id);
            }
        }
    });
    Ok(Box::new(Proxy(core)))
}

impl Role for Proxy {
    fn apply(&mut self, config: &Config) -> Result<(), String> {
        let Some(p) = config.llm.as_ref().and_then(|l| l.proxy.as_ref()) else {
            return Ok(());
        };
        self.0.apply(p, config)
    }

    fn stop(&mut self, grace: Duration) {
        self.0.stop_api();
        self.0.base.stop(grace);
    }

    fn tokens(&self) -> Option<Registry> {
        Some(self.0.base.mesh.tokens())
    }

    fn files(&self) -> Vec<(String, [u8; 32])> {
        self.0.base.mesh.files()
    }

    fn api(&self) -> Option<akit_roles::host::Api> {
        Some(Arc::new(self.0.base.local.clone()))
    }

    fn module(&self) -> Option<akit_roles::pages::ModuleRef> {
        Some(self.0.base.pages.clone())
    }
}

impl Core {
    fn apply(&self, p: &LlmProxy, config: &Config) -> Result<(), String> {
        lock(&self.keys).clone_from(&p.api_keys);
        // The use of the keys kept stays.
        lock(&self.usage).retain(|id, _| p.api_keys.iter().any(|k| k.id == *id));
        *lock(&self.api_want) = methods::api_of(p);
        let mut errs = Vec::new();
        if let Err(e) = self.base.apply(&p.role, config) {
            errs.push(e);
        }
        // The API takes the agents the role's access, then the global one, serve.
        match crate::role::policy(p.role.access.as_ref(), config.access.as_ref()) {
            Ok(pol) => {
                *lock(&self.policy) = pol.clone();
                if let Some(a) = lock(&self.api).as_ref() {
                    let engine = &self.base.host.engine;
                    let _ = match pol {
                        Some(pol) => engine
                            .set_listener_filter(a.id, move |x: &SocketAddr| pol.allows(x.ip())),
                        None => engine.clear_listener_filter(a.id),
                    };
                }
            }
            Err(e) => errs.push(e),
        }
        let want = p.api.clone().filter(|a| !a.address.is_empty());
        let same = match (&*lock(&self.api), &want) {
            (Some(a), Some(w)) => a.cfg == *w,
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.stop_api();
            if let Some(a) = want {
                if let Err(e) = self.start_api(&a) {
                    errs.push(format!("api: {e}"));
                }
            }
        }
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs.join("; "))
        }
    }

    fn peer_up(&self, p: &PeerConn) {
        let id = p.identity.clone();
        let (method, hub) = match id.role.as_str() {
            ROLE_HUB => (METHOD_HUB_MODELS, true),
            ROLE_NODE => (METHOD_NODE_INFO, false),
            _ => return,
        };
        let me = self.me.clone();
        let peer = id.id.clone();
        self.base
            .mesh
            .call(&peer, method, "{}", Duration::ZERO, move |r| {
                let Some(c) = me.upgrade() else { return };
                match r.and_then(|b| json::parse(&b).map_err(Error::Other)) {
                    Ok(v) if c.base.mesh.online(&id.id) => {
                        let mut cat = lock(&c.cat);
                        if hub {
                            cat.set_hub(&id, HubModels::from_value(&v));
                        } else {
                            cat.set_node(&id, NodeInfo::from_value(&v));
                        }
                    }
                    Ok(_) => {}
                    Err(e) => akit_observe::warn!(
                        Llm,
                        "llm.proxy: fetching {method} of {} failed: {e}",
                        id.id
                    ),
                }
            });
        if hub {
            let me = self.me.clone();
            let id = p.identity.clone();
            self.base.mesh.call(
                &id.id.clone(),
                METHOD_HUB_NODE_LIST,
                "{}",
                Duration::ZERO,
                move |r| {
                    let Some(c) = me.upgrade() else { return };
                    match r.and_then(|b| json::parse(&b).map_err(Error::Other)) {
                        Ok(v) if c.base.mesh.online(&id.id) => {
                            lock(&c.cat).set_list(&id, methods::HubList::from_value(&v));
                        }
                        Ok(_) => {}
                        Err(e) => akit_observe::warn!(
                            Llm,
                            "llm.proxy: fetching the node list of hub {} failed: {e}",
                            id.id
                        ),
                    }
                },
            );
        }
    }

    fn start_api(&self, cfg: &LlmProxyApi) -> Result<(), String> {
        let addr: SocketAddr = cfg
            .address
            .replacen("localhost:", "127.0.0.1:", 1)
            .parse()
            .map_err(|_| format!("{}: not an address to listen on", cfg.address))?;
        let mut http = akio_http::HttpConfig::default();
        http.h2 = None;
        http.max_header_bytes = 64 << 10;
        http.max_body_bytes = usize::try_from(cfg.body()).unwrap_or(usize::MAX);
        http.head_timeout = Some(Duration::from_secs(10));
        http.body_timeout = Some(Duration::from_secs(120));
        http.keep_alive_timeout = Some(Duration::from_secs(120));
        *lock(&self.api_cfg) = cfg.clone();
        let service = akio_http::serve_with(
            http,
            Service {
                core: self.me.clone(),
            },
        );
        let conns = Arc::new(Conns {
            max: cfg.conns(),
            open: Mutex::new(HashMap::new()),
        });
        let (a, b) = (Arc::clone(&conns), Arc::clone(&conns));
        let mut l = akit_runtime::scheduled(akio::Listener::tcp(addr))
            .threads(1)
            .on_accept(move |h| {
                let mut g = lock(&a.open);
                if g.len() >= a.max {
                    drop(g);
                    h.close();
                    return;
                }
                g.insert(h.id(), h.clone());
            })
            .on_close(move |h| {
                lock(&b.open).remove(&h.id());
            });
        if let Some(p) = lock(&self.policy).clone() {
            l = l.filter(move |a: &SocketAddr| p.allows(a.ip()));
        }
        let l = if cfg.cert_file.is_empty() {
            l.serve(service)
        } else {
            let c = std::fs::read_to_string(&cfg.cert_file).map_err(|e| e.to_string())?;
            let k = std::fs::read_to_string(&cfg.key_file).map_err(|e| e.to_string())?;
            let chain =
                akio::tls::Certificate::from_pem(c.as_bytes()).map_err(|e| e.to_string())?;
            let key = akio::tls::PrivateKey::from_pem(k.as_bytes()).map_err(|e| e.to_string())?;
            let sc = akio::tls::ServerConfig::builder(chain, key)
                .build()
                .map_err(|e| e.to_string())?;
            l.serve(akio::tls::Tls::server(sc, service))
        };
        let id = self
            .base
            .host
            .engine
            .add_listener(l)
            .map_err(|e| format!("listen {}: {e}", cfg.address))?;
        let bound = self
            .base
            .host
            .engine
            .listeners()
            .into_iter()
            .find(|i| i.id == id)
            .map_or_else(|| cfg.address.clone(), |i| i.local_addr.to_string());
        akit_observe::info!(Llm, "llm.proxy: the agents' API listening on {bound}");
        *lock(&self.api) = Some(Api {
            cfg: cfg.clone(),
            id,
            conns,
        });
        Ok(())
    }

    fn stop_api(&self) {
        if let Some(a) = lock(&self.api).take() {
            let _ = self.base.host.engine.remove_listener(a.id);
            let open: Vec<ConnHandle> = lock(&a.conns.open).values().cloned().collect();
            for h in open {
                h.close();
            }
        }
    }

    /// The API's address, its port chosen when the config's was 0 (tests).
    fn api_addr(&self) -> Option<SocketAddr> {
        let id = lock(&self.api).as_ref()?.id;
        self.base
            .host
            .engine
            .listeners()
            .into_iter()
            .find(|i| i.id == id)
            .map(|i| i.local_addr)
    }

    /// Whether an agent's request carries one of the keys.
    fn authorized(&self, ctx: &Context, api: &str) -> bool {
        let mut key = request_key(ctx);
        if key.is_empty() && api == API_GEMINI {
            key = query_param(ctx.req().query().unwrap_or(""), "key").unwrap_or_default();
        }
        if key.is_empty() {
            return false;
        }
        let sum = akit_roles::linktoken::hex(&akio_crypto::hash::Sha256::digest(key.as_bytes()));
        let mut found = None;
        for k in lock(&self.keys).iter() {
            if akio_crypto::ct_eq(sum.as_bytes(), k.hash.as_bytes()) {
                found = Some(k.id.clone());
            }
        }
        let Some(id) = found else { return false };
        // Counted in memory, for the pages (fit's `keyStore.auth`).
        let mut u = lock(&self.usage);
        if u.len() < akit_config::roles::MAX_LLM_KEYS || u.contains_key(&id) {
            let e = u.entry(id).or_default();
            e.0 = akit_roles::linktoken::unix_now();
            e.1 = e.1.saturating_add(1);
        }
        true
    }
}

/// The address of the proxy role's agent API, for tests: `None` when it does not run.
#[must_use]
pub fn api_addr_of(role: &dyn std::any::Any) -> Option<SocketAddr> {
    role.downcast_ref::<Proxy>().and_then(|p| p.0.api_addr())
}

fn request_key(ctx: &Context) -> String {
    if let Some(a) = ctx.header("authorization").map(str::trim) {
        if a.len() > 7
            && a.get(..7)
                .is_some_and(|p| p.eq_ignore_ascii_case("bearer "))
        {
            return a.get(7..).unwrap_or("").trim().to_owned();
        }
    }
    for h in ["x-api-key", "x-goog-api-key"] {
        if let Some(k) = ctx.header(h).map(str::trim).filter(|k| !k.is_empty()) {
            return k.to_owned();
        }
    }
    String::new()
}

fn query_param(q: &str, key: &str) -> Option<String> {
    q.split('&').find_map(|p| {
        let (k, v) = p.split_once('=').unwrap_or((p, ""));
        (k == key).then(|| unescape(v))
    })
}

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0usize;
    while let Some(&c) = b.get(i) {
        match c {
            b'+' => out.push(b' '),
            b'%' => {
                let h = b
                    .get(i.saturating_add(1)..i.saturating_add(3))
                    .and_then(|h| std::str::from_utf8(h).ok())
                    .and_then(|h| u8::from_str_radix(h, 16).ok());
                if let Some(v) = h {
                    out.push(v);
                    i = i.saturating_add(2);
                } else {
                    out.push(c);
                }
            }
            c => out.push(c),
        }
        i = i.saturating_add(1);
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---- The agents' API ----

/// The call an agent's POST makes: the protocol, the call, and Gemini's model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Endpoint {
    api: &'static str,
    path: String,
    model: String,
}

/// The call of a POST to `path`; `Err` with the protocol, for the error's shape, otherwise.
fn agent_endpoint(path: &str) -> Result<Endpoint, &'static str> {
    for (api, prefix) in [
        (API_OPENAI, "/v1/"),
        (API_ANTHROPIC, "/v1/"),
        (API_OLLAMA, "/"),
        (API_COHERE, "/v2/"),
    ] {
        if let Some(p) = path.strip_prefix(prefix) {
            if proxy_paths(api).contains(&p) {
                return Ok(Endpoint {
                    api,
                    path: p.to_owned(),
                    model: String::new(),
                });
            }
        }
    }
    for pre in ["/v1beta/models/", "/v1/models/"] {
        let Some(rest) = path.strip_prefix(pre) else {
            continue;
        };
        // The method follows the last colon: a model's name may hold one.
        return match rest.rfind(':') {
            Some(i) if i > 0 => {
                let method = rest.get(i.saturating_add(1)..).unwrap_or("");
                if proxy_paths(API_GEMINI).contains(&method) {
                    Ok(Endpoint {
                        api: API_GEMINI,
                        path: method.to_owned(),
                        model: rest.get(..i).unwrap_or("").to_owned(),
                    })
                } else {
                    Err(API_GEMINI)
                }
            }
            _ => Err(API_GEMINI),
        };
    }
    Err("")
}

/// The error shape of a request: its protocol's, or the one its path or headers suggest.
fn error_api(ctx: &Context, api: &str) -> &'static str {
    match api {
        API_OPENAI => return API_OPENAI,
        API_ANTHROPIC => return API_ANTHROPIC,
        API_GEMINI => return API_GEMINI,
        API_OLLAMA => return API_OLLAMA,
        API_COHERE => return API_COHERE,
        _ => {}
    }
    let p = ctx.path();
    if p.starts_with("/v1beta/") || ctx.header("x-goog-api-key").is_some() {
        API_GEMINI
    } else if p.starts_with("/api/") {
        API_OLLAMA
    } else if p.starts_with("/v2/") {
        API_COHERE
    } else if ctx.header("anthropic-version").is_some() || ctx.header("x-api-key").is_some() {
        API_ANTHROPIC
    } else {
        API_OPENAI
    }
}

/// An error the agent's SDK understands, in the shape of `api`.
fn error_body(api: &str, status: u16, oa_code: &str, msg: &str) -> String {
    match api {
        API_ANTHROPIC => {
            let t = match status {
                400 => "invalid_request_error",
                401 => "authentication_error",
                403 => "permission_error",
                404 => "not_found_error",
                413 => "request_too_large",
                429 => "rate_limit_error",
                503 => "overloaded_error",
                _ => "api_error",
            };
            Obj::new()
                .str("type", "error")
                .raw(
                    "error",
                    &Obj::new().str("type", t).str("message", msg).end(),
                )
                .end()
        }
        API_GEMINI => {
            let s = match status {
                400 | 413 => "INVALID_ARGUMENT",
                401 => "UNAUTHENTICATED",
                403 => "PERMISSION_DENIED",
                404 => "NOT_FOUND",
                429 => "RESOURCE_EXHAUSTED",
                503 | 502 => "UNAVAILABLE",
                504 => "DEADLINE_EXCEEDED",
                _ => "INTERNAL",
            };
            Obj::new()
                .raw(
                    "error",
                    &Obj::new()
                        .int("code", i64::from(status))
                        .str("message", msg)
                        .str("status", s)
                        .end(),
                )
                .end()
        }
        API_OLLAMA => Obj::new().str("error", msg).end(),
        API_COHERE => Obj::new().str("message", msg).end(),
        _ => {
            let (t, mut code) = match status {
                400 | 413 => ("invalid_request_error", ""),
                401 => ("invalid_request_error", "invalid_api_key"),
                404 => ("invalid_request_error", "model_not_found"),
                429 => ("rate_limit_exceeded", "rate_limit_exceeded"),
                _ => ("server_error", ""),
            };
            if !oa_code.is_empty() {
                code = oa_code;
            }
            let e = Obj::new().str("message", msg).str("type", t);
            let e = if code.is_empty() {
                e.raw("code", "null")
            } else {
                e.str("code", code)
            };
            Obj::new().raw("error", &e.end()).end()
        }
    }
}

fn error_response(api: &str, status: u16, oa_code: &str, msg: &str) -> Response {
    let mut r = Response::new(status);
    r.headers.set("Content-Type", "application/json");
    r.headers.set("Cache-Control", "no-store");
    r.body = error_body(api, status, oa_code, msg).into_bytes();
    r
}

/// The HTTP status of a mesh error's code.
fn status_for(code: i64) -> u16 {
    match code {
        CODE_BAD_REQUEST => 400,
        CODE_NOT_FOUND => 404,
        CODE_RATE_LIMITED => 429,
        CODE_UNAVAILABLE => 503,
        _ => 502,
    }
}

struct Service {
    core: Weak<Core>,
}

impl akio_http::Service for Service {
    fn call(&self, ctx: &mut Context) {
        let Some(c) = self.core.upgrade() else {
            return ctx.respond(error_response(API_OPENAI, 503, "", "the proxy is stopping"));
        };
        match *ctx.method() {
            Method::Post => c.serve_proxy(ctx),
            Method::Get => c.serve_get(ctx),
            _ => {
                let msg = format!("unknown endpoint {} {}", ctx.method().as_str(), ctx.path());
                ctx.respond(error_response(error_api(ctx, ""), 404, "unknown_url", &msg));
            }
        }
    }

    fn streams_bodies(&self) -> bool {
        false
    }
}

fn json_response(body: String) -> Response {
    let mut r = Response::new(200);
    r.headers.set("Content-Type", "application/json");
    r.headers.set("Cache-Control", "no-store");
    r.body = body.into_bytes();
    r
}

impl Core {
    /// The model lists: OpenAI's and Anthropic's (`/v1/models`), Gemini's (`/v1beta/models`),
    /// Ollama's (`/api/tags`).
    fn serve_get(&self, ctx: &mut Context) {
        let p = ctx.path().to_owned();
        let api = if p == "/v1/models" || p.starts_with("/v1/models/") {
            API_OPENAI
        } else if p == "/v1beta/models" || p.starts_with("/v1beta/models/") {
            API_GEMINI
        } else if p == "/api/tags" {
            API_OLLAMA
        } else {
            let e = error_api(ctx, "");
            return ctx.respond(error_response(
                e,
                404,
                "unknown_url",
                &format!("unknown endpoint GET {p}"),
            ));
        };
        let eapi = if api == API_OPENAI {
            error_api(ctx, "")
        } else {
            api
        };
        if !self.authorized(ctx, api) {
            return ctx.respond(error_response(eapi, 401, "", "invalid or missing api key"));
        }
        let names = lock(&self.cat).names(if api == API_OPENAI { "" } else { api });
        let not_found =
            |id: &str| error_response(eapi, 404, "", &format!("model {id:?} not found"));
        let body = match api {
            API_GEMINI => {
                let gm = |n: &str| {
                    Obj::new()
                        .str("name", &format!("models/{n}"))
                        .str("baseModelId", n)
                        .str("version", "001")
                        .str("displayName", n)
                        .raw(
                            "supportedGenerationMethods",
                            &json::array(proxy_paths(API_GEMINI).iter().map(|m| json::string(m))),
                        )
                        .end()
                };
                if let Some(id) = p.strip_prefix("/v1beta/models/") {
                    if !names.iter().any(|n| n == id) {
                        return ctx.respond(not_found(id));
                    }
                    gm(id)
                } else {
                    Obj::new()
                        .raw("models", &json::array(names.iter().map(|n| gm(n))))
                        .end()
                }
            }
            API_OLLAMA => Obj::new()
                .raw(
                    "models",
                    &json::array(names.iter().map(|n| {
                        Obj::new()
                            .str("name", n)
                            .str("model", n)
                            .str("modified_at", "1970-01-01T00:00:00Z")
                            .int("size", 0)
                            .str("digest", "")
                            .raw("details", "{}")
                            .end()
                    })),
                )
                .end(),
            _ => {
                let obj = |n: &str| {
                    Obj::new()
                        .str("id", n)
                        .str("object", "model")
                        .int("created", 0)
                        .str("owned_by", "akit")
                        .str("type", "model")
                        .str("display_name", n)
                        .str("created_at", "1970-01-01T00:00:00Z")
                        .end()
                };
                if let Some(id) = p.strip_prefix("/v1/models/") {
                    if !names.iter().any(|n| n == id) {
                        return ctx.respond(not_found(id));
                    }
                    obj(id)
                } else {
                    let (first, last) = match (names.first(), names.last()) {
                        (Some(f), Some(l)) => (json::string(f), json::string(l)),
                        _ => ("null".to_owned(), "null".to_owned()),
                    };
                    Obj::new()
                        .str("object", "list")
                        .raw("data", &json::array(names.iter().map(|n| obj(n))))
                        .bool("has_more", false)
                        .raw("first_id", &first)
                        .raw("last_id", &last)
                        .end()
                }
            }
        };
        ctx.respond(json_response(body));
    }

    /// Forwards a call. The body is held whole, within `max_body_bytes`: the route depends on
    /// the model it names, and a route that cannot serve it gets the body again.
    fn serve_proxy(&self, ctx: &mut Context) {
        let path = ctx.path().to_owned();
        let e = match agent_endpoint(&path) {
            Ok(e) => e,
            Err(api) => {
                let ea = error_api(ctx, api);
                return ctx.respond(error_response(
                    ea,
                    404,
                    "unknown_url",
                    &format!("unknown endpoint POST {path}"),
                ));
            }
        };
        let api = e.api;
        if !self.authorized(ctx, api) {
            return ctx.respond(error_response(api, 401, "", "invalid or missing api key"));
        }
        let body = ctx.body().to_vec();
        let (name, ctype) = match call_model(ctx, &e, &body) {
            Ok(v) => v,
            Err(msg) => return ctx.respond(error_response(api, 400, "", &msg)),
        };
        if name.len() > MAX_MODEL_NAME || name.chars().any(char::is_whitespace) {
            return ctx.respond(error_response(
                api,
                400,
                "",
                "the model name is up to 256 characters without spaces",
            ));
        }
        let mut req = ProxyReq {
            api: api.to_owned(),
            path: e.path.clone(),
            model: name.clone(),
            content_type: ctype,
            ..ProxyReq::default()
        };
        for k in PROXY_HEADERS {
            if let Some(v) = ctx.header(k).filter(|v| !v.is_empty()) {
                req.header.push(((*k).to_owned(), v.to_owned()));
            }
        }
        if api == API_GEMINI {
            if let Some(v) = query_param(ctx.req().query().unwrap_or(""), "alt") {
                req.query = format!("alt={}", form_escape(&v));
            }
        }
        let (routes, known, other) = lock(&self.cat).routes(&name, api);
        if routes.is_empty() {
            let r = if !known {
                error_response(api, 404, "", &format!("model {name:?} not found"))
            } else if !other.is_empty() {
                error_response(
                    api,
                    404,
                    "",
                    &format!("model {name:?} is served with the {other} API, not {api}"),
                )
            } else {
                error_response(
                    api,
                    503,
                    "",
                    &format!("model {name:?} has no ready node now"),
                )
            };
            return ctx.respond(r);
        }
        let cfg = lock(&self.api_cfg).clone();
        if self.inflight.fetch_add(1, Ordering::AcqRel) >= cfg.inflight() {
            self.inflight.fetch_sub(1, Ordering::AcqRel);
            return ctx.respond(error_response(
                api,
                429,
                "",
                "too many calls in flight; try again",
            ));
        }
        let conn = ctx.connection();
        let stat = self.stats.begin(&name);
        let x = Arc::new(Call {
            core: self.me.clone(),
            stat,
            req,
            body,
            routes,
            conn,
            s: Mutex::new(CState::default()),
        });
        let x2 = Arc::clone(&x);
        ctx.on_gone(move || x2.cancel(Gone::Agent));
        let Some(d) = ctx.detach() else {
            self.inflight.fetch_sub(1, Ordering::AcqRel);
            self.stats.end(&x.stat, 0, true);
            return;
        };
        {
            let mut s = lock(&x.s);
            s.out = Some(d);
            let (a, b) = (Arc::downgrade(&x), Arc::downgrade(&x));
            s.first = after_send(cfg.first_byte(), move || {
                if let Some(x) = a.upgrade() {
                    x.cancel(Gone::FirstByte);
                }
            })
            .ok();
            s.total = after_send(cfg.total(), move || {
                if let Some(x) = b.upgrade() {
                    x.cancel(Gone::Total);
                }
            })
            .ok();
        }
        x.next();
    }
}

fn form_escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(char::from(b));
        } else if b == b' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The model a call names and the body's media type to pass on: Gemini's in the path, a
/// `multipart/form-data` body's in its model field (OpenAI's uploads), any other body's in its
/// JSON's `model`.
fn call_model(ctx: &Context, e: &Endpoint, body: &[u8]) -> Result<(String, String), String> {
    if e.api == API_GEMINI {
        return Ok((e.model.clone(), String::new()));
    }
    let ct = ctx.header("content-type").unwrap_or("");
    if let Some(b) = form::form_boundary(ct) {
        if e.api != API_OPENAI {
            return Err("a multipart body is taken only by OpenAI's endpoints".to_owned());
        }
        let (s, end) = form::form_field(body, &b, "model")
            .map_err(|e| format!("a bad multipart body: {e}"))?;
        if s == end {
            return Err("missing \"model\" in the form".to_owned());
        }
        let name = String::from_utf8_lossy(body.get(s..end).unwrap_or(&[])).into_owned();
        return Ok((name, ct.to_owned()));
    }
    let v = json::parse(body).map_err(|e| format!("request body is not valid json: {e}"))?;
    let name = json::get_str(&v, "model");
    if name.is_empty() {
        return Err("missing \"model\" in request body".to_owned());
    }
    Ok((name, String::new()))
}

// ---- One agent call ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gone {
    Agent,
    FirstByte,
    Total,
}

#[derive(Default)]
struct CState {
    attempt: usize,
    exclude: Vec<String>,
    node_held: String,
    st: Option<Stream>,
    out: Option<Detached>,
    body: Option<BodyStream>,
    started: bool,
    done: bool,
    cause: Option<Gone>,
    last: Option<Error>,
    sent: usize,
    held: usize,
    waiting: bool,
    first: Option<SendTimer>,
    total: Option<SendTimer>,
    /// The attempt's count on its route.
    rc: Option<RouteCall>,
    /// The response's bytes written to the agent.
    relayed: i64,
    /// The call failed.
    failed: bool,
}

struct Call {
    core: Weak<Core>,
    /// The model the call is counted under (`Stats::begin`).
    stat: String,
    req: ProxyReq,
    body: Vec<u8>,
    routes: Vec<RouteTo>,
    conn: Option<ConnHandle>,
    s: Mutex<CState>,
}

fn retryable(e: &Error) -> bool {
    match e {
        Error::Remote { code, .. } => *code == CODE_UNAVAILABLE,
        Error::Closed => true,
        _ => false,
    }
}

impl Call {
    fn core(&self) -> Option<Arc<Core>> {
        self.core.upgrade()
    }

    fn unhold(&self, s: &mut CState) {
        if !s.node_held.is_empty() {
            let id = std::mem::take(&mut s.node_held);
            if let Some(c) = self.core() {
                let mut cat = lock(&c.cat);
                let n = cat.inflight.entry(id.clone()).or_insert(0);
                *n = n.saturating_sub(1);
                if *n <= 0 {
                    cat.inflight.remove(&id);
                }
            }
        }
    }

    /// Tries the next route, or fails the call.
    fn next(self: &Arc<Self>) {
        let Some(core) = self.core() else { return };
        let (rt, req) = {
            let mut s = lock(&self.s);
            if s.done {
                return;
            }
            if s.attempt >= self.routes.len() || s.attempt >= MAX_ATTEMPTS || s.cause.is_some() {
                let err = s.last.take();
                drop(s);
                return self.fail(err.unwrap_or_else(|| {
                    Error::code(
                        CODE_UNAVAILABLE,
                        format!("no route to model {}", self.req.model),
                    )
                }));
            }
            let Some(rt) = self.routes.get(s.attempt).cloned() else {
                return;
            };
            s.attempt = s.attempt.saturating_add(1);
            let mut req = self.req.clone();
            if rt.hub {
                req.exclude.clone_from(&s.exclude);
            } else {
                let mut cat = lock(&core.cat);
                let n = cat.inflight.entry(rt.id.clone()).or_insert(0);
                *n = n.saturating_add(1);
                s.node_held.clone_from(&rt.id);
            }
            if self.body.len() <= INLINE_MAX {
                req.body.clone_from(&self.body);
            } else {
                req.body_stream = true;
                req.body_size = i64::try_from(self.body.len()).unwrap_or(i64::MAX);
            }
            s.sent = 0;
            (rt, req)
        };
        let method = if rt.hub {
            METHOD_HUB_PROXY
        } else {
            METHOD_NODE_PROXY
        };
        let st = match core.base.mesh.open_stream(&rt.id, method, &req.to_json()) {
            Ok(st) => st,
            Err(e) => return self.retry(&rt, e),
        };
        let rc = core.base.mesh.routes().begin("", &rt.id, &self.req.model);
        rc.add_req(req.body.len());
        rc.attach(&st, false);
        let cause = {
            let mut s = lock(&self.s);
            s.st = Some(st.clone());
            s.rc = Some(rc);
            s.cause
        };
        if cause.is_some() {
            st.cancel();
        }
        if req.body_stream {
            self.send_body(&st);
        }
        let (me, st2) = (Arc::clone(self), st.clone());
        st.on_data(move |ev| me.frame(&st2, &rt, ev));
    }

    /// Moves to the next route after a failure before the head, when it may, or fails the call.
    fn retry(self: &Arc<Self>, rt: &RouteTo, e: Error) {
        let cause = {
            let mut s = lock(&self.s);
            if let Some(rc) = s.rc.take() {
                rc.end(true);
            }
            self.unhold(&mut s);
            if !rt.hub {
                s.exclude.push(rt.id.clone());
            }
            s.last = Some(e.clone());
            s.cause
        };
        if cause.is_none() && retryable(&e) {
            akit_observe::info!(
                Llm,
                "llm.proxy: route {} {} unavailable for {}, trying the next one: {e}",
                if rt.hub { "hub" } else { "node" },
                rt.id,
                self.req.model
            );
            return self.next();
        }
        self.fail(e);
    }

    /// Sends a large body in frames as the stream's credit allows, then half-closes it.
    fn send_body(self: &Arc<Self>, st: &Stream) {
        loop {
            let off = lock(&self.s).sent;
            if off >= self.body.len() {
                let _ = st.close_send();
                return;
            }
            let end = off.saturating_add(FRAME_SIZE).min(self.body.len());
            let chunk = self.body.get(off..end).unwrap_or(&[]);
            match st.try_send(chunk) {
                Err(_) => return, // the stream's end says why
                Ok(true) => lock(&self.s).sent = end,
                Ok(false) => {
                    let (me, st2) = (Arc::clone(self), st.clone());
                    if st.on_credit(move || me.send_body(&st2)) {
                        return;
                    }
                }
            }
        }
    }

    fn frame(self: &Arc<Self>, st: &Stream, rt: &RouteTo, ev: Event) {
        let (current, started, done) = {
            let s = lock(&self.s);
            (s.st.as_ref().is_some_and(|x| x.same(st)), s.started, s.done)
        };
        if done || !current {
            if let Event::Data(d) = ev {
                st.release(d.len());
            }
            return;
        }
        if !started {
            match ev {
                Event::End(r) => {
                    let e = match r {
                        Ok(()) => Error::code(
                            CODE_INTERNAL,
                            format!("{} ended before its head frame", rt.id),
                        ),
                        Err(e) => e,
                    };
                    self.retry(rt, e);
                }
                Event::Data(d) => {
                    st.release(d.len());
                    match ProxyHead::parse(&d) {
                        Ok(h) if (100..=999).contains(&h.status) => self.begin(st, &h),
                        _ => {
                            st.cancel();
                            self.retry(
                                rt,
                                Error::code(
                                    CODE_INTERNAL,
                                    format!("bad head frame from {}", rt.id),
                                ),
                            );
                        }
                    }
                }
            }
            return;
        }
        match ev {
            Event::Data(d) => self.write(st, &d),
            Event::End(r) => self.finish(r),
        }
    }

    /// Writes the response's status and header.
    fn begin(&self, st: &Stream, head: &ProxyHead) {
        let mut s = lock(&self.s);
        s.started = true;
        if let Some(t) = s.first.take() {
            t.cancel();
        }
        let Some(out) = s.out.take() else {
            drop(s);
            st.cancel();
            return;
        };
        let mut h = Headers::new();
        h.set(
            "Content-Type",
            if head.content_type.is_empty() {
                "application/json"
            } else {
                &head.content_type
            },
        );
        h.set("Cache-Control", "no-cache");
        h.set("X-Accel-Buffering", "no");
        if !head.node.is_empty() {
            h.set(NODE_HEADER, &head.node);
        }
        let status = u16::try_from(head.status).unwrap_or(502);
        if status >= 400 {
            s.last = Some(Error::Other(format!("upstream status {status}")));
        }
        s.body = out.stream_response(status, h);
        if s.body.is_none() {
            drop(s);
            st.cancel();
        }
    }

    /// Writes a piece of the body; while the agent is behind, the frame stays unreleased, so
    /// that the far side waits for credit.
    fn write(self: &Arc<Self>, st: &Stream, data: &[u8]) {
        // Taken out while written: its drain callback may run at once, here, and takes the lock.
        let Some(b) = lock(&self.s).body.take() else {
            st.release(data.len());
            return;
        };
        if b.write(data).is_err() {
            st.release(data.len());
            return self.cancel(Gone::Agent);
        }
        let high = b.queued_bytes() > WRITE_HIGH;
        let start = {
            let mut s = lock(&self.s);
            s.relayed = s
                .relayed
                .saturating_add(i64::try_from(data.len()).unwrap_or(i64::MAX));
            if !high && !s.waiting {
                None
            } else {
                s.held = s.held.saturating_add(data.len());
                Some(!std::mem::replace(&mut s.waiting, true))
            }
        };
        match start {
            None => st.release(data.len()),
            Some(true) => {
                let (me, st2) = (Arc::clone(self), st.clone());
                b.on_drain(move || {
                    let n = {
                        let mut s = lock(&me.s);
                        s.waiting = false;
                        std::mem::take(&mut s.held)
                    };
                    st2.release(n);
                });
            }
            Some(false) => {}
        }
        let mut s = lock(&self.s);
        if !s.done {
            s.body = Some(b);
        }
    }

    /// Ends a response that began: whole when the stream ended well, cut short otherwise.
    fn finish(&self, r: Result<(), Error>) {
        let body = {
            let mut s = lock(&self.s);
            if s.done {
                return;
            }
            s.done = true;
            s.failed = r.is_err();
            self.stop(&mut s);
            if let Some(rc) = s.rc.take() {
                rc.end(r.is_err());
            }
            s.body.take()
        };
        match r {
            Ok(()) => {
                if let Some(b) = body {
                    b.finish();
                }
            }
            Err(e) => {
                akit_observe::warn!(
                    Llm,
                    "llm.proxy: the call to {} broke after its head: {e}",
                    self.req.model
                );
                // A chunked body without its last chunk: closing the connection tells the agent.
                drop(body);
                if let Some(c) = &self.conn {
                    c.close();
                }
            }
        }
        self.done();
    }

    fn done(&self) {
        if let Some(c) = self.core() {
            c.inflight.fetch_sub(1, Ordering::AcqRel);
            let (bytes, failed) = {
                let s = lock(&self.s);
                (s.relayed, s.failed)
            };
            c.stats.end(&self.stat, bytes, failed);
        }
    }

    fn stop(&self, s: &mut CState) {
        self.unhold(s);
        if let Some(t) = s.first.take() {
            t.cancel();
        }
        if let Some(t) = s.total.take() {
            t.cancel();
        }
    }

    /// Answers a call no route served.
    fn fail(&self, e: Error) {
        let (out, cause) = {
            let mut s = lock(&self.s);
            if s.done {
                return;
            }
            s.done = true;
            s.failed = true;
            self.stop(&mut s);
            if let Some(rc) = s.rc.take() {
                rc.end(true);
            }
            (s.out.take(), s.cause)
        };
        if cause != Some(Gone::Agent) {
            let (code, msg) = e.code_msg();
            let status = if cause == Some(Gone::Total) {
                504
            } else {
                status_for(code)
            };
            if let Some(o) = out {
                o.respond(error_response(&self.req.api, status, "", &msg));
            }
        }
        self.done();
    }

    /// Ends the call for `why`: the agent left, or a timer ran out.
    fn cancel(&self, why: Gone) {
        let (st, started) = {
            let mut s = lock(&self.s);
            if s.done || s.cause.is_some() {
                return;
            }
            s.cause = Some(why);
            s.last = Some(match why {
                Gone::Agent => Error::Other("the agent went away".to_owned()),
                Gone::FirstByte => Error::code(CODE_UNAVAILABLE, "no response began in time"),
                Gone::Total => {
                    Error::Other("the call took longer than the API's timeout".to_owned())
                }
            });
            (s.st.clone(), s.started)
        };
        match st {
            Some(st) => {
                st.cancel();
                if started {
                    self.finish(Err(Error::Canceled));
                } else {
                    let e = lock(&self.s).last.clone().unwrap_or(Error::Canceled);
                    self.fail(e);
                }
            }
            None => {
                let e = lock(&self.s).last.clone().unwrap_or(Error::Canceled);
                self.fail(e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn endpoints() {
        assert_eq!(
            agent_endpoint("/v1/chat/completions").unwrap(),
            Endpoint {
                api: API_OPENAI,
                path: "chat/completions".to_owned(),
                model: String::new()
            }
        );
        assert_eq!(agent_endpoint("/v1/messages").unwrap().api, API_ANTHROPIC);
        assert_eq!(agent_endpoint("/api/chat").unwrap().api, API_OLLAMA);
        assert_eq!(agent_endpoint("/v2/chat").unwrap().api, API_COHERE);
        let g = agent_endpoint("/v1beta/models/gemini:2.5:streamGenerateContent").unwrap();
        assert_eq!(
            (g.model.as_str(), g.path.as_str()),
            ("gemini:2.5", "streamGenerateContent")
        );
        assert_eq!(agent_endpoint("/v1beta/models/x:nope"), Err(API_GEMINI));
        assert_eq!(agent_endpoint("/nope"), Err(""));
    }

    #[test]
    fn error_shapes() {
        assert_eq!(
            error_body(API_OPENAI, 401, "", "no"),
            r#"{"error":{"message":"no","type":"invalid_request_error","code":"invalid_api_key"}}"#
        );
        assert_eq!(
            error_body(API_OPENAI, 503, "", "x"),
            r#"{"error":{"message":"x","type":"server_error","code":null}}"#
        );
        assert_eq!(
            error_body(API_ANTHROPIC, 503, "", "x"),
            r#"{"type":"error","error":{"type":"overloaded_error","message":"x"}}"#
        );
        assert_eq!(
            error_body(API_GEMINI, 404, "", "x"),
            r#"{"error":{"code":404,"message":"x","status":"NOT_FOUND"}}"#
        );
        assert_eq!(error_body(API_OLLAMA, 500, "", "x"), r#"{"error":"x"}"#);
        assert_eq!(error_body(API_COHERE, 500, "", "x"), r#"{"message":"x"}"#);
        assert_eq!(status_for(CODE_UNAVAILABLE), 503);
        assert_eq!(status_for(CODE_INTERNAL), 502);
        assert_eq!(unescape("a%20b+c"), "a b c");
    }
}
