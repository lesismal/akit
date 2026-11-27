//! fit's LLM node (`llm/node`, I105): the models of its member, each `ready`, `disabled` or in
//! `error` as its last check found it, checked every 60 s (15 s in error); its info answered
//! (`/node/info`) and pushed to its hubs and proxies when it changes (200 ms apart, a growing
//! sequence); and `/node/proxy`: one agent call forwarded to the model's upstream on akio's
//! asynchronous HTTP client, the request body and the response streamed both ways under the
//! stream's credit.
//!
//! The node's management (its pages' and a hub's model edits, hardware, GPUs, actions and jobs)
//! is I108's and I109's.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use akio::timer::{SendTimer, after_send};
use akio_http::client::{
    CancelToken, Client, ClientError, ClientOptions, ClientRequest, RequestBody, ResponseBody,
    ResponseHandler, ResponseHead,
};
use akio_http::{Headers, Method};
use akit_config::Config;
use akit_config::roles::{LlmModel, LlmNode, LlmNodeProxy};
use akit_roles::host::{Api, Host, Info, Role};
use akit_roles::linktoken::Registry;

use crate::form;
use crate::json::Obj;
use crate::mesh::{Answer, RouteCall, Router, bad_request};
use crate::model::{
    API_ANTHROPIC, API_AZURE_OPENAI, API_GEMINI, API_OLLAMA, API_OPENAI, CODE_BAD_REQUEST,
    CODE_INTERNAL, CODE_NOT_FOUND, CODE_UNAVAILABLE, Identity, METHOD_NODE_INFO, METHOD_NODE_PROXY,
    METHOD_NODE_UPDATE, MODEL_DISABLED, MODEL_ERROR, MODEL_READY, ModelView, NodeInfo, PeerConn,
    ProxyHead, ProxyReq, ProxyResult, ROLE_HUB, ROLE_PROXY, protocol, proxy_paths, serves,
};
use crate::role::Base;
use crate::wire::{Error, Event, Stream};

/// `llm.node`.
pub const NODE: Info = Info {
    id: "llm.node",
    set: |c| c.llm.as_ref().is_some_and(|l| l.node.is_some()),
    new,
};

/// How often the checks look for models due.
const CHECK_TICK: Duration = Duration::from_secs(5);
/// A model is checked this often...
const CHECK_EVERY: i64 = 60;
/// ...and this often while in error.
const CHECK_ERROR_EVERY: i64 = 15;
/// A check's bound.
const CHECK_TIMEOUT: Duration = Duration::from_secs(15);
/// The changes of a node's info gathered into one push.
const PUSH_DELAY: Duration = Duration::from_millis(200);
/// Sent upstream when the caller sent no `anthropic-version`.
const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
/// The longest query a call passes upstream.
const MAX_QUERY: usize = 1024;
/// A streamed request body is held from the stream once this much waits for the upstream.
const UPLOAD_HIGH: usize = 256 << 10;

/// A body rewrite: the model the call names set to the upstream's.
type Rewrite = Box<dyn Fn(&[u8]) -> Result<Vec<u8>, String> + Send>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now() -> i64 {
    akit_roles::linktoken::unix_now()
}

fn unavailable(msg: impl Into<String>) -> Error {
    Error::code(CODE_UNAVAILABLE, msg)
}

#[derive(Debug, Default)]
struct Rt {
    err: String,
    checked_at: i64,
    inflight: i64,
    checking: bool,
    probes: u64,
    applied: u64,
}

#[derive(Debug, Clone, Copy)]
struct Lim {
    first_byte: Duration,
    idle: Duration,
    total: Duration,
    inflight: i64,
    body: u64,
}

impl Lim {
    fn of(p: Option<&LlmNodeProxy>) -> Lim {
        Lim {
            first_byte: LlmNodeProxy::first_byte(p).max(LlmNodeProxy::connect(p)),
            idle: LlmNodeProxy::idle(p),
            total: LlmNodeProxy::total(p),
            inflight: i64::try_from(LlmNodeProxy::inflight(p)).unwrap_or(i64::MAX),
            body: LlmNodeProxy::body(p),
        }
    }
}

fn key(name: &str, api: &str) -> String {
    format!("{api}\0{name}")
}

struct St {
    models: Vec<LlmModel>,
    rt: HashMap<String, Rt>,
    total: i64,
    lim: Lim,
    description: String,
    hub_manage: bool,
    started: bool,
}

struct Push {
    timer: Option<SendTimer>,
    last: String,
    seq: i64,
}

struct Core {
    base: Base,
    me: Weak<Core>,
    st: Mutex<St>,
    push: Mutex<Push>,
    client: Client,
    probe: Client,
    stopped: AtomicBool,
}

/// A running node.
struct Node(Arc<Core>);

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node").finish_non_exhaustive()
    }
}

/// Makes the node role; its first apply starts it.
///
/// # Errors
///
/// None.
pub fn new(host: Host) -> Result<Box<dyn Role>, String> {
    let engine = Arc::clone(&host.engine);
    let core = Arc::new_cyclic(|me: &Weak<Core>| {
        let mut r = Router::new();
        let (a, b) = (me.clone(), me.clone());
        r.handle(
            METHOD_NODE_INFO,
            &[ROLE_HUB, ROLE_PROXY],
            Duration::ZERO,
            move |_, _, ans: Answer| match a.upgrade() {
                Some(c) => ans.ok(c.info().to_json()),
                None => ans.err(Error::Closed),
            },
        );
        r.handle_stream(
            METHOD_NODE_PROXY,
            &[ROLE_HUB, ROLE_PROXY],
            move |caller, req, st| match b.upgrade() {
                Some(c) => c.proxy(caller, req, st),
                None => st.end(Err(Error::Closed)),
            },
        );
        let mut po = ClientOptions::default();
        po.max_body_bytes = 64 << 10;
        po.max_idle_per_host = 2;
        Core {
            base: Base::new(host, "node", r),
            me: me.clone(),
            st: Mutex::new(St {
                models: Vec::new(),
                rt: HashMap::new(),
                total: 0,
                lim: Lim::of(None),
                description: String::new(),
                hub_manage: false,
                started: false,
            }),
            push: Mutex::new(Push {
                timer: None,
                last: String::new(),
                seq: akit_roles::linktoken::unix_now().saturating_mul(1000),
            }),
            client: Client::new(Arc::clone(&engine)),
            probe: Client::new(Arc::clone(&engine))
                .with_options(po)
                .with_timeout(CHECK_TIMEOUT),
            stopped: AtomicBool::new(false),
        }
    });
    let w = Arc::downgrade(&core);
    core.base.mesh.on_peer_up(move |p: &PeerConn| {
        let Some(c) = w.upgrade() else { return };
        if p.identity.role == ROLE_HUB || p.identity.role == ROLE_PROXY {
            let _ = c
                .base
                .mesh
                .push(&p.identity.id, METHOD_NODE_UPDATE, &c.info().to_json());
        }
    });
    Ok(Box::new(Node(core)))
}

impl Role for Node {
    fn apply(&mut self, config: &Config) -> Result<(), String> {
        let Some(m) = config.llm.as_ref().and_then(|l| l.node.as_ref()) else {
            return Ok(());
        };
        self.0.apply(m, config)
    }

    fn stop(&mut self, grace: Duration) {
        let c = &self.0;
        c.stopped.store(true, Ordering::Release);
        if let Some(t) = lock(&c.push).timer.take() {
            t.cancel();
        }
        c.base.stop(grace.min(Duration::from_secs(10)));
    }

    fn tokens(&self) -> Option<Registry> {
        Some(self.0.base.mesh.tokens())
    }

    fn api(&self) -> Option<Api> {
        Some(Arc::new(self.0.base.local.clone()))
    }
}

/// The hardware part of a node's info (I108 reads the rest): the host, its system and its CPUs.
fn hw() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        o => o,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        a => a,
    };
    let cores = std::thread::available_parallelism().map_or(0, std::num::NonZero::get);
    Obj::new()
        .str("hostname", &akit_config::roles::host_name())
        .str("os", os)
        .str("arch", arch)
        .str("cpu_model", "")
        .int("cpu_cores", i64::try_from(cores).unwrap_or(0))
        .int("load1", 0)
        .int("mem_total_mb", 0)
        .int("mem_available_mb", 0)
        .raw("gpus", "[]")
        .raw("disks", "[]")
        .raw("ips", "[]")
        .int("uptime_seconds", 0)
        .int("collected_at", now())
        .end()
}

fn view(m: &LlmModel, rt: Option<&Rt>) -> ModelView {
    let mut v = ModelView {
        name: m.name.clone(),
        api: m.api.clone(),
        state: MODEL_READY.to_owned(),
        description: m.description.clone(),
        max_inflight: i64::try_from(m.max_inflight).unwrap_or(i64::MAX),
        ..ModelView::default()
    };
    if let Some(r) = rt {
        v.inflight = r.inflight;
        v.checked_at = r.checked_at;
        v.error.clone_from(&r.err);
    }
    if m.disabled {
        v.state = MODEL_DISABLED.to_owned();
    } else if !v.error.is_empty() {
        v.state = MODEL_ERROR.to_owned();
    }
    v
}

impl Core {
    fn apply(&self, m: &LlmNode, config: &Config) -> Result<(), String> {
        {
            let mut st = lock(&self.st);
            let old = std::mem::take(&mut st.models);
            for nm in &m.models {
                let k = key(&nm.name, &nm.api);
                let changed = old
                    .iter()
                    .find(|o| o.name == nm.name && o.api == nm.api)
                    .is_some_and(|o| {
                        o.base_url != nm.base_url
                            || o.api_key != nm.api_key
                            || o.upstream != nm.upstream
                            || o.api_version != nm.api_version
                    });
                let r = st.rt.entry(k).or_default();
                if changed {
                    // A probe of the old upstream is stale.
                    r.err.clear();
                    r.checked_at = 0;
                    r.applied = r.probes;
                }
            }
            let keep: Vec<String> = m.models.iter().map(|x| key(&x.name, &x.api)).collect();
            st.rt.retain(|k, r| r.inflight > 0 || keep.contains(k));
            st.models.clone_from(&m.models);
            st.lim = Lim::of(m.proxy.as_ref());
            st.description.clone_from(&m.description);
            st.hub_manage = m.settings.as_ref().is_some_and(|s| s.allow_hub_manage);
        }
        let r = self.base.apply(&m.role, config);
        let start = {
            let mut st = lock(&self.st);
            !std::mem::replace(&mut st.started, true)
        };
        if start {
            self.check_tick();
        }
        self.changed();
        r
    }

    /// The info without its sequence.
    fn snapshot(&self) -> NodeInfo {
        let id = self.base.mesh.identity();
        let st = lock(&self.st);
        NodeInfo {
            id: id.id,
            name: id.name,
            description: st.description.clone(),
            hw: hw(),
            models: st
                .models
                .iter()
                .map(|m| view(m, st.rt.get(&key(&m.name, &m.api))))
                .collect(),
            inflight: st.total,
            jobs: 0,
            hub_manage: st.hub_manage,
            seq: 0,
            updated_at: 0,
        }
    }

    /// The info with a new sequence.
    fn info(&self) -> NodeInfo {
        let mut info = self.snapshot();
        let mut p = lock(&self.push);
        p.seq = p.seq.saturating_add(1);
        info.seq = p.seq;
        info.updated_at = now();
        info
    }

    /// The info changed: one push follows, 200 ms on, when it differs from the last.
    fn changed(&self) {
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        let mut p = lock(&self.push);
        if p.timer.is_some() {
            return;
        }
        let me = self.me.clone();
        p.timer = after_send(PUSH_DELAY, move || {
            if let Some(c) = me.upgrade() {
                lock(&c.push).timer = None;
                c.flush();
            }
        })
        .ok();
    }

    fn flush(&self) {
        let mut info = self.snapshot();
        // The hardware's time of collection aside, which changes every time.
        let mut cmp = info.clone();
        cmp.hw.clear();
        let text = cmp.to_json();
        {
            let mut p = lock(&self.push);
            if p.last == text {
                return;
            }
            p.last = text;
            p.seq = p.seq.saturating_add(1);
            info.seq = p.seq;
        }
        info.updated_at = now();
        let body = info.to_json();
        let m = &self.base.mesh;
        for role in [ROLE_HUB, ROLE_PROXY] {
            for id in m.peers_of(role) {
                let _ = m.push(&id.id, METHOD_NODE_UPDATE, &body);
            }
        }
    }

    // ---- Checks ----

    fn check_tick(&self) {
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        let t = now();
        let due: Vec<LlmModel> = {
            let mut st = lock(&self.st);
            let models = st.models.clone();
            models
                .into_iter()
                .filter(|m| {
                    let Some(r) = st.rt.get_mut(&key(&m.name, &m.api)) else {
                        return false;
                    };
                    let every = if r.err.is_empty() {
                        CHECK_EVERY
                    } else {
                        CHECK_ERROR_EVERY
                    };
                    if m.disabled || r.checking || t.saturating_sub(r.checked_at) < every {
                        return false;
                    }
                    r.checking = true;
                    true
                })
                .collect()
        };
        for m in due {
            let me = self.me.clone();
            let k = key(&m.name, &m.api);
            self.check(&m, move || {
                if let Some(c) = me.upgrade() {
                    if let Some(r) = lock(&c.st).rt.get_mut(&k) {
                        r.checking = false;
                    }
                }
            });
        }
        let me = self.me.clone();
        let _ = after_send(CHECK_TICK, move || {
            if let Some(c) = me.upgrade() {
                c.check_tick();
            }
        });
    }

    /// Probes the upstream of `m`, records the outcome, then runs `done`.
    fn check(&self, m: &LlmModel, done: impl FnOnce() + Send + 'static) {
        let seq = {
            let mut st = lock(&self.st);
            st.rt.get_mut(&key(&m.name, &m.api)).map_or(0, |r| {
                r.probes = r.probes.saturating_add(1);
                r.probes
            })
        };
        let mut req = ClientRequest::get(&probe_url(m));
        set_auth(&mut req.headers, m, &[]);
        let (me, m2) = (self.me.clone(), m.clone());
        self.probe.request(req, move |r| {
            let msg = match r {
                Err(e) => format!("upstream unreachable: {e}"),
                Ok(resp) if resp.status == 401 || resp.status == 403 || resp.status >= 500 => {
                    let b = resp.body.get(..resp.body.len().min(512)).unwrap_or(&[]);
                    format!(
                        "upstream HTTP {}: {}",
                        resp.status,
                        String::from_utf8_lossy(b).trim()
                    )
                }
                Ok(_) => String::new(),
            };
            if let Some(c) = me.upgrade() {
                c.set_state(&m2, &msg, true, seq);
            }
            done();
        });
    }

    /// Records the state of `m`: the outcome of probe `seq`, or of a call when 0; a probe older
    /// than the state it would replace is dropped.
    fn set_state(&self, m: &LlmModel, msg: &str, checked: bool, seq: u64) {
        let changed = {
            let mut st = lock(&self.st);
            let Some(r) = st.rt.get_mut(&key(&m.name, &m.api)) else {
                return;
            };
            if seq != 0 && seq <= r.applied {
                return;
            }
            r.applied = if seq == 0 { r.probes } else { seq };
            let changed = r.err != msg;
            if changed && !msg.is_empty() {
                akit_observe::warn!(
                    Llm,
                    "llm.node: model {} ({}) unavailable: {msg}",
                    m.name,
                    m.api
                );
            } else if changed {
                akit_observe::info!(
                    Llm,
                    "llm.node: model {} ({}) available again",
                    m.name,
                    m.api
                );
            }
            msg.clone_into(&mut r.err);
            if checked {
                r.checked_at = now();
            }
            changed || checked
        };
        if changed {
            self.changed();
        }
    }

    // ---- Calls ----

    /// Takes a call's place on model `name` of protocol `api`, within the node's bound.
    fn acquire(&self, name: &str, api: &str) -> Result<(LlmModel, Lim), Error> {
        let mut st = lock(&self.st);
        let lim = st.lim;
        let Some(m) = st
            .models
            .iter()
            .find(|m| m.name == name && protocol(&m.api) == api)
            .cloned()
        else {
            return Err(unavailable(format!(
                "model {name} ({api}) is not served by this node"
            )));
        };
        let total = st.total;
        let r = st.rt.entry(key(&m.name, &m.api)).or_default();
        if m.disabled {
            return Err(unavailable(format!("model {name} is disabled")));
        }
        if !r.err.is_empty() {
            return Err(unavailable(format!("model {name}: {}", r.err)));
        }
        let max = i64::try_from(m.max_inflight).unwrap_or(i64::MAX);
        if max > 0 && r.inflight >= max {
            return Err(unavailable(format!(
                "model {name} is busy ({} in flight)",
                r.inflight
            )));
        }
        if total >= lim.inflight {
            return Err(unavailable(format!(
                "the node is busy ({total} calls in flight)"
            )));
        }
        r.inflight = r.inflight.saturating_add(1);
        st.total = st.total.saturating_add(1);
        Ok((m, lim))
    }

    fn release(&self, m: &LlmModel) {
        {
            let mut st = lock(&self.st);
            let k = key(&m.name, &m.api);
            let gone = !st.models.iter().any(|x| x.name == m.name && x.api == m.api);
            if let Some(r) = st.rt.get_mut(&k) {
                r.inflight = r.inflight.saturating_sub(1);
                if r.inflight == 0 && gone {
                    st.rt.remove(&k);
                }
            }
            st.total = st.total.saturating_sub(1);
        }
        self.changed();
    }

    /// Serves `/node/proxy`: forwards one agent call to the model's upstream and streams the
    /// response back as it comes, a frame per piece, held while the caller grants no credit.
    fn proxy(&self, caller: &Identity, payload: &[u8], st: Stream) {
        let mut req = match ProxyReq::parse(payload) {
            Ok(r) => r,
            Err(e) => return st.end(Err(bad_request(format!("bad request: {e}")))),
        };
        req.api = protocol(&req.api).to_owned();
        if !proxy_paths(&req.api).contains(&req.path.as_str()) {
            return st.end(Err(bad_request(format!(
                "path {:?} is not allowed for api {:?}",
                req.path, req.api
            ))));
        }
        let (ctype, boundary) = match body_type(&req.content_type) {
            Ok(v) => v,
            Err(e) => return st.end(Err(e)),
        };
        let (m, lim) = match self.acquire(&req.model, &req.api) {
            Ok(v) => v,
            Err(e) => return st.end(Err(e)),
        };
        self.changed();
        // The call is counted on its route from the caller to the upstream; dropped before the
        // call goes upstream, it counts as failed.
        let rc = self.base.mesh.routes().begin(&caller.id, "", &m.name);
        rc.add_req(req.body.len());
        rc.attach(&st, true);
        let fail = |e: Error| {
            self.release(&m);
            st.end(Err(e));
        };
        if !serves(&m.api, &req.path) {
            return fail(Error::code(
                CODE_NOT_FOUND,
                format!(
                    "model {} is served by {}, which has no {}",
                    m.name, m.api, req.path
                ),
            ));
        }
        let size = u64::try_from(req.body.len()).unwrap_or(u64::MAX);
        if size > lim.body || u64::try_from(req.body_size).is_ok_and(|n| n > lim.body) {
            return fail(Error::code(
                CODE_BAD_REQUEST,
                format!(
                    "the request body is larger than the node's {} bytes",
                    lim.body
                ),
            ));
        }
        let upstream = if m.upstream.is_empty() {
            m.name.clone()
        } else {
            m.upstream.clone()
        };
        let (target, in_body) = match upstream_url(&m, &req, &upstream) {
            Ok(v) => v,
            Err(e) => return fail(e),
        };
        // An upstream ID equal to the model the call names leaves the body as it is, and a
        // streamed one streams.
        let rewrite: Option<Rewrite> = if upstream == req.model {
            None
        } else if m.api == API_GEMINI && req.path == "batchEmbedContents" {
            let u = upstream.clone();
            Some(Box::new(move |b| form::rewrite_batch_models(b, &u)))
        } else if !in_body {
            None
        } else if let Some(bd) = boundary.clone() {
            let u = upstream.clone();
            Some(Box::new(move |b| form::rewrite_form(b, &bd, &u)))
        } else {
            let u = upstream.clone();
            Some(Box::new(move |b| form::rewrite_model(b, &u)))
        };
        let mut hreq = ClientRequest::new(Method::Post, &target);
        hreq.headers.set("Content-Type", &ctype);
        set_auth(&mut hreq.headers, &m, &req.header);
        let cancel = CancelToken::new();
        hreq = hreq
            .cancel_with(&cancel)
            .timeout(lim.total)
            .response_idle_timeout(lim.idle);
        let x = Arc::new(Exchange {
            core: self.me.clone(),
            st: st.clone(),
            model: m.clone(),
            caller: format!("{}:{}", caller.role, caller.id),
            path: req.path.clone(),
            lim,
            start: Instant::now(),
            cancel,
            node: self.base.id(),
            rc: Mutex::new(Some(rc)),
            s: Mutex::new(XState::default()),
        });
        x.arm_first_byte();
        let x2 = Arc::clone(&x);
        st.on_finish(move |e| {
            if let Some(e) = e {
                if *e != Error::StreamEnded {
                    x2.cancel(e.clone());
                }
            }
        });
        lock(&x.s).keep = Some(st.keep_alive(Duration::ZERO));
        match (req.body_stream, rewrite) {
            (false, rw) => {
                let body = match rw {
                    Some(f) => match f(&req.body) {
                        Ok(b) => b,
                        Err(e) => return x.finish(Err(bad_request(e))),
                    },
                    None => std::mem::take(&mut req.body),
                };
                hreq.body = body;
                self.client.request_with(hreq, H(x));
            }
            (true, Some(f)) => {
                // The whole body first, within the bound, then the call.
                let client = self.client.clone();
                collect(&st, lim.body, move |r| {
                    match r.and_then(|b| f(&b).map_err(bad_request)) {
                        Ok(b) => {
                            hreq.body = b;
                            client.request_with(hreq, H(x));
                        }
                        Err(e) => x.finish(Err(e)),
                    }
                });
            }
            (true, None) => {
                let len = u64::try_from(req.body_size).ok().filter(|n| *n > 0);
                hreq = hreq.streamed(len);
                lock(&x.s).upload = true;
                self.client.request_with(hreq, H(x));
            }
        }
    }
}

/// Reads a streamed body whole, up to `limit` bytes.
fn collect(st: &Stream, limit: u64, done: impl FnOnce(Result<Vec<u8>, Error>) + Send + 'static) {
    let s = st.clone();
    let mut buf = Vec::new();
    let mut done = Some(done);
    st.on_data(move |ev| match ev {
        Event::Data(d) => {
            s.release(d.len());
            if u64::try_from(buf.len().saturating_add(d.len())).unwrap_or(u64::MAX) > limit {
                if let Some(f) = done.take() {
                    f(Err(Error::code(
                        CODE_BAD_REQUEST,
                        format!("the request body is larger than the node's {limit} bytes"),
                    )));
                }
                return;
            }
            buf.extend_from_slice(&d);
        }
        Event::End(r) => {
            if let Some(f) = done.take() {
                f(r.map(|()| std::mem::take(&mut buf)));
            }
        }
    });
}

/// The URL of a call to `m` with `upstream` as its model ID, and whether the body names the
/// model: openai, anthropic, cohere, ollama at `base_url/path`; gemini at
/// `base_url/models/{upstream}:{method}` with the query it lets through; azure_openai at
/// `base_url/openai/deployments/{upstream}/{path}` with its `api-version`.
fn upstream_url(m: &LlmModel, req: &ProxyReq, upstream: &str) -> Result<(String, bool), Error> {
    let base = m.base_url.trim_end_matches('/');
    match m.api.as_str() {
        API_GEMINI => {
            let mut u = format!("{base}/models/{}:{}", path_escape(upstream), req.path);
            if !req.query.is_empty() {
                if req.query.len() > MAX_QUERY {
                    return Err(bad_request(format!(
                        "the query is longer than {MAX_QUERY} bytes"
                    )));
                }
                let alt: Vec<&str> = req
                    .query
                    .split('&')
                    .filter(|p| p.split('=').next() == Some("alt"))
                    .collect();
                if !alt.is_empty() {
                    u.push('?');
                    u.push_str(&alt.join("&"));
                }
            }
            Ok((u, false))
        }
        API_AZURE_OPENAI => {
            let v = format!("?api-version={}", query_escape(&m.api_version));
            if req.path == "responses" {
                return Ok((format!("{base}/openai/responses{v}"), true));
            }
            Ok((
                format!(
                    "{base}/openai/deployments/{}/{}{v}",
                    path_escape(upstream),
                    req.path
                ),
                false,
            ))
        }
        _ => Ok((format!("{base}/{}", req.path), true)),
    }
}

fn escape(s: &str, keep: impl Fn(u8) -> bool) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || keep(b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn path_escape(s: &str) -> String {
    escape(s, |b| b"-._~!$&'()*+,;=:@".contains(&b))
}

fn query_escape(s: &str) -> String {
    escape(s, |b| b"-._~".contains(&b))
}

/// The body's media type a call gives: JSON when none, or `multipart/form-data` with its
/// boundary.
fn body_type(ct: &str) -> Result<(String, Option<String>), Error> {
    if ct.is_empty() {
        return Ok(("application/json".to_owned(), None));
    }
    if let Some(b) = form::form_boundary(ct) {
        return Ok((ct.to_owned(), Some(b)));
    }
    if form::media_type(ct).is_some_and(|(mt, _)| mt == "application/json") {
        return Ok((ct.to_owned(), None));
    }
    Err(bad_request(format!(
        "the body's content type {ct:?} is neither application/json nor multipart/form-data with a boundary"
    )))
}

/// The URL of `m`'s upstream model list.
fn probe_url(m: &LlmModel) -> String {
    let base = m.base_url.trim_end_matches('/');
    match m.api.as_str() {
        API_AZURE_OPENAI => format!(
            "{base}/openai/models?api-version={}",
            query_escape(&m.api_version)
        ),
        API_OLLAMA => format!("{base}/api/tags"),
        _ => format!("{base}/models"),
    }
}

/// The upstream's auth and behaviour headers for `m`'s kind; of the caller's headers only
/// `pass` ([`crate::model::PROXY_HEADERS`]) go upstream, never its key.
fn set_auth(h: &mut Headers, m: &LlmModel, pass: &[(String, String)]) {
    let get = |k: &str| {
        pass.iter()
            .find(|(hk, _)| hk.eq_ignore_ascii_case(k))
            .map_or("", |(_, v)| v.as_str())
    };
    let key = &m.api_key;
    match m.api.as_str() {
        API_ANTHROPIC => {
            if !key.is_empty() {
                h.set("x-api-key", key);
            }
            let v = get("anthropic-version");
            h.set(
                "anthropic-version",
                if v.is_empty() {
                    DEFAULT_ANTHROPIC_VERSION
                } else {
                    v
                },
            );
            let b = get("anthropic-beta");
            if !b.is_empty() {
                h.set("anthropic-beta", b);
            }
        }
        API_GEMINI => {
            if !key.is_empty() {
                h.set("x-goog-api-key", key);
            }
        }
        API_AZURE_OPENAI => {
            if !key.is_empty() {
                h.set("api-key", key);
            }
        }
        api => {
            if !key.is_empty() {
                h.set("Authorization", format!("Bearer {key}"));
            }
            let b = get("openai-beta");
            if !b.is_empty() && api == API_OPENAI {
                h.set("OpenAI-Beta", b);
            }
        }
    }
}

// ---- One call on its way upstream and back ----

#[derive(Default)]
struct XState {
    head: bool,
    status: i64,
    bytes: i64,
    pending: VecDeque<Vec<u8>>,
    waiting: bool,
    ended: bool,
    done: bool,
    upload: bool,
    body: Option<ResponseBody>,
    first: Option<SendTimer>,
    keep: Option<Arc<AtomicBool>>,
}

struct Exchange {
    core: Weak<Core>,
    st: Stream,
    model: LlmModel,
    caller: String,
    path: String,
    lim: Lim,
    start: Instant,
    cancel: CancelToken,
    node: String,
    rc: Mutex<Option<RouteCall>>,
    s: Mutex<XState>,
}

impl Exchange {
    fn arm_first_byte(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        let t = after_send(self.lim.first_byte, move || {
            if let Some(x) = me.upgrade() {
                if !lock(&x.s).head {
                    let name = x.model.name.clone();
                    x.cancel(unavailable(format!(
                        "model {name}: the upstream did not answer in time"
                    )));
                }
            }
        });
        lock(&self.s).first = t.ok();
    }

    fn stop_keep(&self) {
        if let Some(k) = lock(&self.s).keep.take() {
            k.store(true, Ordering::Release);
        }
    }

    /// Abandons the upstream request for `cause`.
    fn cancel(&self, cause: Error) {
        self.cancel.cancel();
        self.finish(Err(cause));
    }

    /// Ends the call once: the stream ends with the result or the error.
    fn finish(&self, r: Result<(), Error>) {
        let (status, bytes, first) = {
            let mut s = lock(&self.s);
            if s.done {
                return;
            }
            s.done = true;
            s.pending.clear();
            (s.status, s.bytes, s.first.take())
        };
        if let Some(t) = first {
            t.cancel();
        }
        if let Some(rc) = lock(&self.rc).take() {
            // An upstream's error status counts as failed too.
            rc.end(r.is_err() || status >= 400);
        }
        self.stop_keep();
        let ms = i64::try_from(self.start.elapsed().as_millis()).unwrap_or(i64::MAX);
        akit_observe::info!(
            Llm,
            "llm.node: proxy model={} api={} path={} status={status} bytes={bytes} duration_ms={ms} caller={} err={}",
            self.model.name,
            self.model.api,
            self.path,
            self.caller,
            r.as_ref()
                .err()
                .map_or_else(|| "<nil>".to_owned(), ToString::to_string)
        );
        match r {
            Ok(()) => self.st.end(Ok(ProxyResult {
                node: self.node.clone(),
                model: self.model.name.clone(),
                status,
                bytes,
                duration_ms: ms,
            }
            .to_json()
            .into_bytes())),
            Err(e) => {
                self.cancel.cancel();
                self.st.end(Err(e));
            }
        }
        if let Some(c) = self.core.upgrade() {
            c.release(&self.model);
        }
    }

    /// Sends what waits while the caller grants credit; once it all went, ends the call or lets
    /// the upstream's body go on.
    fn resume(self: &Arc<Self>) {
        loop {
            let (front, ended, body) = {
                let s = lock(&self.s);
                if s.done {
                    return;
                }
                (s.pending.front().cloned(), s.ended, s.body.clone())
            };
            let Some(data) = front else {
                lock(&self.s).waiting = false;
                if ended {
                    return self.finish(Ok(()));
                }
                if let Some(b) = body {
                    b.hold_reads(false);
                }
                return;
            };
            match self.st.try_send(&data) {
                Ok(true) => {
                    lock(&self.s).pending.pop_front();
                }
                Ok(false) => {
                    let me = Arc::clone(self);
                    if self.st.on_credit(move || me.resume()) {
                        return;
                    }
                }
                Err(e) => return self.cancel(e),
            }
        }
    }

    /// Sends `data`, or queues it and holds the upstream's body until credit comes.
    fn send(self: &Arc<Self>, data: Vec<u8>) {
        let (queue, body) = {
            let s = lock(&self.s);
            if s.done {
                return;
            }
            (s.waiting || !s.pending.is_empty(), s.body.clone())
        };
        if !queue {
            match self.st.try_send(&data) {
                Ok(true) => return,
                Ok(false) => {}
                Err(e) => return self.cancel(e),
            }
        }
        let start = {
            let mut s = lock(&self.s);
            s.pending.push_back(data);
            !std::mem::replace(&mut s.waiting, true)
        };
        if let Some(b) = body {
            b.hold_reads(true);
        }
        if start {
            let me = Arc::clone(self);
            if !self.st.on_credit(move || me.resume()) {
                self.resume();
            }
        }
    }
}

struct H(Arc<Exchange>);

impl ResponseHandler for H {
    fn on_request_body(&mut self, body: RequestBody) {
        let x = Arc::clone(&self.0);
        if !lock(&x.s).upload {
            return;
        }
        // The stream's frames go upstream as they come, held against its window while more than
        // `UPLOAD_HIGH` waits for the upstream.
        let body = Arc::new(Mutex::new(Some(body)));
        let st = x.st.clone();
        let limit = x.lim.body;
        let mut total: u64 = 0;
        let held = Arc::new(Mutex::new((0usize, false)));
        st.clone().on_data(move |ev| match ev {
            Event::Data(d) => {
                total = total.saturating_add(u64::try_from(d.len()).unwrap_or(u64::MAX));
                if total > limit {
                    x.cancel(Error::code(
                        CODE_BAD_REQUEST,
                        format!("the request body is larger than the node's {limit} bytes"),
                    ));
                    st.release(d.len());
                    return;
                }
                let g = lock(&body);
                let Some(b) = g.as_ref() else {
                    st.release(d.len());
                    return;
                };
                if b.write(&d).is_err() {
                    st.release(d.len());
                    return;
                }
                let high = b.queued_bytes() > UPLOAD_HIGH;
                let mut h = lock(&held);
                if !h.1 && !high {
                    drop(h);
                    st.release(d.len());
                    return;
                }
                h.0 = h.0.saturating_add(d.len());
                if h.1 {
                    return;
                }
                h.1 = true;
                drop(h);
                let (held2, st2) = (Arc::clone(&held), st.clone());
                b.on_drain(move || {
                    let n = {
                        let mut h = lock(&held2);
                        h.1 = false;
                        std::mem::take(&mut h.0)
                    };
                    st2.release(n);
                });
            }
            Event::End(Ok(())) => {
                if let Some(b) = lock(&body).take() {
                    let _ = b.finish();
                }
            }
            Event::End(Err(e)) => {
                lock(&body).take();
                x.cancel(e);
            }
        });
    }

    fn on_head(&mut self, head: ResponseHead, body: ResponseBody) {
        let x = &self.0;
        let first = {
            let mut s = lock(&x.s);
            s.head = true;
            s.status = i64::from(head.status);
            s.body = Some(body);
            s.first.take()
        };
        if let Some(t) = first {
            t.cancel();
        }
        if head.status < 500 {
            if let Some(c) = x.core.upgrade() {
                c.set_state(&x.model, "", false, 0);
            }
        }
        let h = ProxyHead {
            status: i64::from(head.status),
            content_type: head.headers.get("content-type").unwrap_or("").to_owned(),
            node: x.node.clone(),
        };
        x.send(h.to_json().into_bytes());
    }

    fn on_data(&mut self, chunk: &[u8]) {
        let x = &self.0;
        x.stop_keep();
        {
            let mut s = lock(&x.s);
            s.bytes = s
                .bytes
                .saturating_add(i64::try_from(chunk.len()).unwrap_or(i64::MAX));
        }
        x.send(chunk.to_vec());
    }

    fn on_end(&mut self, _trailers: &Headers) {
        let x = &self.0;
        let now_done = {
            let mut s = lock(&x.s);
            s.ended = true;
            s.pending.is_empty() && !s.waiting
        };
        if now_done {
            x.finish(Ok(()));
        }
    }

    fn on_error(&mut self, e: ClientError) {
        let x = &self.0;
        let (head, done) = {
            let s = lock(&x.s);
            (s.head, s.done)
        };
        if done {
            return;
        }
        let name = &x.model.name;
        let err = if head {
            Error::code(CODE_INTERNAL, format!("upstream body: {e}"))
        } else {
            match e {
                ClientError::Timeout | ClientError::HeaderTimeout | ClientError::IdleTimeout => {
                    unavailable(format!("model {name}: the upstream did not answer in time"))
                }
                e => {
                    let msg = format!("upstream unreachable: {e}");
                    if let Some(c) = x.core.upgrade() {
                        c.set_state(&x.model, &msg, false, 0);
                    }
                    unavailable(format!("model {name}: {msg}"))
                }
            }
        };
        x.finish(Err(err));
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn model(api: &str) -> LlmModel {
        LlmModel {
            name: "m".to_owned(),
            api: api.to_owned(),
            base_url: "http://u/v1/".to_owned(),
            api_version: "2024-10-21".to_owned(),
            ..LlmModel::default()
        }
    }

    #[test]
    fn upstream_urls() {
        let mut r = ProxyReq {
            path: "chat/completions".to_owned(),
            ..ProxyReq::default()
        };
        assert_eq!(
            upstream_url(&model(API_OPENAI), &r, "x").unwrap(),
            ("http://u/v1/chat/completions".to_owned(), true)
        );
        assert_eq!(
            upstream_url(&model(API_AZURE_OPENAI), &r, "dep 1")
                .unwrap()
                .0,
            "http://u/v1/openai/deployments/dep%201/chat/completions?api-version=2024-10-21"
        );
        r.path = "streamGenerateContent".to_owned();
        r.query = "alt=sse&key=secret".to_owned();
        assert_eq!(
            upstream_url(&model(API_GEMINI), &r, "g").unwrap(),
            (
                "http://u/v1/models/g:streamGenerateContent?alt=sse".to_owned(),
                false
            )
        );
        assert_eq!(
            probe_url(&model(API_OLLAMA)),
            "http://u/v1/api/tags".to_owned()
        );
    }

    #[test]
    fn auth_headers() {
        let mut m = model(API_ANTHROPIC);
        m.api_key = "k".to_owned();
        let mut h = Headers::new();
        set_auth(&mut h, &m, &[("Anthropic-Beta".to_owned(), "b".to_owned())]);
        assert_eq!(h.get("x-api-key"), Some("k"));
        assert_eq!(h.get("anthropic-version"), Some(DEFAULT_ANTHROPIC_VERSION));
        assert_eq!(h.get("anthropic-beta"), Some("b"));
        let mut m = model(API_OPENAI);
        m.api_key = "k".to_owned();
        let mut h = Headers::new();
        set_auth(&mut h, &m, &[("openai-beta".to_owned(), "x".to_owned())]);
        assert_eq!(h.get("authorization"), Some("Bearer k"));
        assert_eq!(h.get("openai-beta"), Some("x"));
        assert!(body_type("text/plain").is_err());
        assert_eq!(body_type("").unwrap().0, "application/json");
    }
}
