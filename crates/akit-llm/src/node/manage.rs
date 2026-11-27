//! The node's management (fit's `llm/node/{routes,models,actions,gpu}.go` and its settings,
//! I108): the methods its own pages call and, with `allow_hub_manage`, a hub: its models saved,
//! deleted and checked, its name, its GPUs, its fixed actions and jobs; its pages alone: its
//! settings and the bounds of the calls it forwards. Every change is written into the node's
//! member, answered once the config changed was applied.

use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use akit_config::roles::{LlmModel, LlmNodeProxy};
use akit_config::value::Value;

use super::{CHECK_TIMEOUT, Core, key, lock, view};
use crate::actions;
use crate::exec::Command;
use crate::gpu;
use crate::jobs::{KIND_ACTION, KIND_GPU_INSTALL, KIND_GPU_SET, Run, run_command, run_commands};
use crate::json::{self, Obj, Req};
use crate::mesh::{Answer, Router, bad_request};
use crate::model::{
    CODE_FORBIDDEN, CODE_NOT_FOUND, CODE_RATE_LIMITED, Identity, METHOD_NODE_ACTION_LIST,
    METHOD_NODE_ACTION_RUN, METHOD_NODE_GPU_CATALOG, METHOD_NODE_GPU_INSTALL, METHOD_NODE_GPU_SET,
    METHOD_NODE_GPU_STATUS, METHOD_NODE_JOB_CANCEL, METHOD_NODE_JOB_FOLLOW, METHOD_NODE_JOB_GET,
    METHOD_NODE_JOB_LIST, METHOD_NODE_MODEL_CHECK, METHOD_NODE_MODEL_DELETE,
    METHOD_NODE_MODEL_LIST, METHOD_NODE_MODEL_SAVE, METHOD_NODE_PROFILE_SET,
    METHOD_NODE_SETTINGS_GET, METHOD_NODE_SETTINGS_SET, METHOD_NODE_UPSTREAM_GET,
    METHOD_NODE_UPSTREAM_SET, ROLE_HUB, ROLE_LOCAL,
};
use crate::role::{EDIT_TIMEOUT, count_setting, duration_setting, number, set};
use crate::wire::{Error, Stream};

/// A GPU status read is reused this long.
pub(super) const GPU_CACHE_TTL: Duration = Duration::from_secs(5);
/// A GPU status read's bound, the pool's wait included.
const GPU_READ_TIMEOUT: Duration = Duration::from_secs(25);

/// The node's settings, defaults filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Settings {
    pub(super) allow_hub_manage: bool,
    pub(super) gpu_sudo: bool,
    pub(super) refresh: Duration,
    pub(super) job_timeout: Duration,
    pub(super) gpu_data_dir: String,
}

impl Settings {
    pub(super) fn of(s: Option<&akit_config::roles::LlmNodeSettings>) -> Settings {
        use akit_config::roles::LlmNodeSettings as S;
        Settings {
            allow_hub_manage: s.is_some_and(|s| s.allow_hub_manage),
            gpu_sudo: S::sudo(s),
            refresh: S::refresh(s),
            job_timeout: S::job(s),
            gpu_data_dir: s.map(|s| s.gpu_data_dir.clone()).unwrap_or_default(),
        }
    }
}

/// The GPUs' status last read.
pub(super) struct GpuCache {
    pub(super) status: Option<(gpu::Status, Instant, bool)>,
}

fn go(d: Duration) -> String {
    akit_config::duration::format(d)
}

/// The call-time check of a management method: a hub only while the node allows it.
fn allow(c: &Core, caller: &Identity) -> Result<(), Error> {
    match caller.role.as_str() {
        ROLE_LOCAL => Ok(()),
        ROLE_HUB if lock(&c.st).set.allow_hub_manage => Ok(()),
        ROLE_HUB => Err(Error::code(
            CODE_FORBIDDEN,
            "this node does not allow hub management (allow_hub_manage)",
        )),
        r => Err(Error::code(
            CODE_FORBIDDEN,
            format!("{r} may not manage this node"),
        )),
    }
}

type ManageFn = fn(&Arc<Core>, &Identity, &[u8], Answer);
type StatusFn = Box<dyn FnOnce(gpu::Status) + Send>;

/// Registers the management methods on `r`, each for its own pages and for hubs, checked at
/// call time; the settings and upstream bounds for its pages alone.
pub(super) fn register(r: &mut Router, me: &Weak<Core>) {
    let manage = |r: &mut Router, method: &str, timeout: Duration, f: ManageFn| {
        let me = me.clone();
        r.handle(method, &[ROLE_HUB], timeout, move |caller, req, a| {
            let Some(c) = me.upgrade() else {
                return a.err(Error::Closed);
            };
            match allow(&c, caller) {
                Ok(()) => f(&c, caller, req, a),
                Err(e) => a.err(e),
            }
        });
    };
    manage(r, METHOD_NODE_MODEL_LIST, Duration::ZERO, |c, _, _, a| {
        a.ok(c.list_models());
    });
    manage(r, METHOD_NODE_MODEL_SAVE, EDIT_TIMEOUT, Core::save_model);
    manage(
        r,
        METHOD_NODE_MODEL_DELETE,
        EDIT_TIMEOUT,
        Core::delete_model,
    );
    manage(
        r,
        METHOD_NODE_MODEL_CHECK,
        CHECK_TIMEOUT.saturating_add(Duration::from_secs(5)),
        Core::check_model,
    );
    manage(r, METHOD_NODE_PROFILE_SET, EDIT_TIMEOUT, Core::set_profile);
    manage(
        r,
        METHOD_NODE_GPU_STATUS,
        Duration::from_secs(30),
        |c, _, req, a| match Req::parse(req).and_then(|r| r.bool("fresh")) {
            Ok(fresh) => c.gpu_status(fresh, move |st| a.ok(st.to_json())),
            Err(e) => a.err(e),
        },
    );
    manage(r, METHOD_NODE_GPU_CATALOG, Duration::ZERO, |_, _, _, a| {
        a.ok(gpu::catalog_json());
    });
    manage(
        r,
        METHOD_NODE_GPU_INSTALL,
        Duration::ZERO,
        Core::gpu_install,
    );
    manage(
        r,
        METHOD_NODE_GPU_SET,
        Duration::from_secs(30),
        Core::gpu_set,
    );
    manage(r, METHOD_NODE_ACTION_LIST, Duration::ZERO, |_, _, _, a| {
        a.ok(actions::list_json());
    });
    manage(r, METHOD_NODE_ACTION_RUN, Duration::ZERO, Core::run_action);
    manage(r, METHOD_NODE_JOB_LIST, Duration::ZERO, |c, _, _, a| {
        a.ok(json::array(
            c.jobs.list().iter().map(crate::jobs::Job::to_json),
        ));
    });
    manage(r, METHOD_NODE_JOB_GET, Duration::ZERO, |c, _, req, a| {
        let (id, offset) = match job_req(req) {
            Ok(v) => v,
            Err(e) => return a.err(e),
        };
        match c.jobs.get(id, offset) {
            Some(j) => a.ok(j.to_json()),
            None => a.err(Error::code(CODE_NOT_FOUND, format!("job {id} not found"))),
        }
    });
    manage(r, METHOD_NODE_JOB_CANCEL, Duration::ZERO, |c, _, req, a| {
        let id = match Req::parse(req).and_then(|r| r.int("id")) {
            Ok(v) => v,
            Err(e) => return a.err(e),
        };
        if c.jobs.cancel(id) {
            a.ok("{}".to_owned());
        } else {
            a.err(Error::code(
                CODE_NOT_FOUND,
                format!("job {id} is not running"),
            ));
        }
    });
    let m = me.clone();
    r.handle_stream(
        METHOD_NODE_JOB_FOLLOW,
        &[ROLE_HUB],
        move |caller, req, st: Stream| {
            let Some(c) = m.upgrade() else {
                return st.end(Err(Error::Closed));
            };
            if let Err(e) = allow(&c, caller) {
                return st.end(Err(e));
            }
            match job_req(req) {
                Ok((id, offset)) => c.jobs.follow(id, offset, st),
                Err(e) => st.end(Err(e)),
            }
        },
    );
    // Its own pages only.
    let local = |r: &mut Router, method: &str, timeout: Duration, f: ManageFn| {
        let me = me.clone();
        r.handle(method, &[], timeout, move |caller, req, a| {
            match me.upgrade() {
                Some(c) => f(&c, caller, req, a),
                None => a.err(Error::Closed),
            }
        });
    };
    local(r, METHOD_NODE_SETTINGS_GET, Duration::ZERO, |c, _, _, a| {
        a.ok(c.settings_json());
    });
    local(
        r,
        METHOD_NODE_SETTINGS_SET,
        EDIT_TIMEOUT,
        Core::set_settings,
    );
    local(r, METHOD_NODE_UPSTREAM_GET, Duration::ZERO, |c, _, _, a| {
        a.ok(c.upstream_json());
    });
    local(
        r,
        METHOD_NODE_UPSTREAM_SET,
        EDIT_TIMEOUT,
        Core::set_upstream,
    );
}

fn job_req(req: &[u8]) -> Result<(i64, usize), Error> {
    let r = Req::parse(req)?;
    let id = r.int("id")?;
    let offset = usize::try_from(r.int("offset")?.max(0)).unwrap_or(0);
    Ok((id, offset))
}

/// An API key hidden but for a few characters at both ends.
#[must_use]
pub fn mask_key(k: &str) -> String {
    if k.is_empty() {
        return String::new();
    }
    let n = k.len();
    match (k.get(..4), k.get(n.saturating_sub(4)..)) {
        (Some(a), Some(b)) if n > 12 => format!("{a}****{b}"),
        _ => "****".to_owned(),
    }
}

fn is_masked(k: &str) -> bool {
    k.contains("****")
}

/// A model as its member writes it (fit's `model.Model`).
fn model_value(m: &LlmModel) -> Value {
    let mut o: Vec<(String, Value)> = vec![
        ("name".into(), Value::String(m.name.clone())),
        ("api".into(), Value::String(m.api.clone())),
        ("base_url".into(), Value::String(m.base_url.clone())),
    ];
    let mut opt = |k: &str, v: &str| {
        if !v.is_empty() {
            o.push((k.into(), Value::String(v.to_owned())));
        }
    };
    opt("upstream", &m.upstream);
    opt("api_version", &m.api_version);
    opt("api_key", &m.api_key);
    if m.disabled {
        o.push(("disabled".into(), Value::Bool(true)));
    }
    if !m.description.is_empty() {
        o.push(("description".into(), Value::String(m.description.clone())));
    }
    if m.max_inflight > 0 {
        o.push(("max_inflight".into(), number(m.max_inflight)));
    }
    Value::Object(o)
}

/// A model as the pages see it: its key masked.
fn model_json(m: &LlmModel) -> String {
    let mut m = m.clone();
    m.api_key = mask_key(&m.api_key);
    json::write(&model_value(&m))
}

/// A model of a request or a member.
fn model_of(r: &Req) -> Result<LlmModel, Error> {
    Ok(LlmModel {
        name: r.str("name")?,
        api: r.str("api")?,
        base_url: r.str("base_url")?,
        upstream: r.str("upstream")?,
        api_key: r.str("api_key")?,
        api_version: r.str("api_version")?,
        disabled: r.bool("disabled")?,
        description: r.str("description")?,
        max_inflight: usize::try_from(r.int("max_inflight")?)
            .map_err(|_| bad_request("max_inflight is 0 to 100000"))?,
    })
}

/// The models of a member, as they are written in it.
fn member_models(m: &[(String, Value)]) -> Result<Vec<LlmModel>, Error> {
    let Some((_, v)) = m.iter().find(|(k, _)| k == "models") else {
        return Ok(Vec::new());
    };
    let Value::Array(items) = v else {
        return Err(bad_request("the member's models are not a list"));
    };
    items
        .iter()
        .map(|it| model_of(&Req::parse(json::write(it).as_bytes())?))
        .collect()
}

/// Checks and normalizes a model as the config will (fit's `validModel`).
fn valid_model(m: &mut LlmModel) -> Result<(), Error> {
    m.name = m.name.trim().to_owned();
    m.upstream = m.upstream.trim().to_owned();
    m.base_url = m.base_url.trim().trim_end_matches('/').to_owned();
    m.api_key = m.api_key.trim().to_owned();
    m.description = m.description.trim().to_owned();
    m.api_version = m.api_version.trim().to_owned();
    let mut c = m.clone();
    if is_masked(&c.api_key) {
        c.api_key.clear();
    }
    match akit_config::roles::check_llm_model(&c) {
        Some((_, msg)) => Err(bad_request(msg)),
        None => Ok(()),
    }
}

/// The model `name` (of `api`, when given) among `models`.
fn find(models: &[LlmModel], name: &str, api: &str) -> Result<usize, Error> {
    let mut idx = None;
    for (i, m) in models.iter().enumerate() {
        if m.name == name && (api.is_empty() || m.api == api) {
            if idx.is_some() {
                return Err(bad_request(format!(
                    "model {name} exists for several API formats: give api"
                )));
            }
            idx = Some(i);
        }
    }
    idx.ok_or_else(|| Error::code(CODE_NOT_FOUND, format!("model {name} not found")))
}

/// Answers `a` once an edit was applied, with `ok`'s JSON then.
fn answer(
    me: Weak<Core>,
    a: Answer,
    ok: impl FnOnce(&Arc<Core>) -> String + Send + 'static,
) -> impl FnOnce(Result<(), Error>) + Send + 'static {
    move |r| match (r, me.upgrade()) {
        (Ok(()), Some(c)) => a.ok(ok(&c)),
        (Ok(()), None) => a.err(Error::Closed),
        (Err(e), _) => a.err(e),
    }
}

impl Core {
    fn list_models(&self) -> String {
        json::array(lock(&self.st).models.iter().map(model_json))
    }

    /// Changes the models of the member with `f`, within the bound.
    fn edit_models(
        &self,
        f: impl FnOnce(&mut Vec<LlmModel>) -> Result<(), Error> + Send + 'static,
        done: impl FnOnce(Result<(), Error>) + Send + 'static,
    ) {
        self.base.edit(
            move |m| {
                let mut models = member_models(m)?;
                f(&mut models)?;
                if models.len() > akit_config::roles::MAX_LLM_MODELS {
                    return Err(bad_request(format!(
                        "at most {} models",
                        akit_config::roles::MAX_LLM_MODELS
                    )));
                }
                set(
                    m,
                    "models",
                    Value::Array(models.iter().map(model_value).collect()),
                );
                Ok(())
            },
            done,
        );
    }

    /// `/node/model/save`: a model added, or the one of the same name and API replaced (an
    /// empty or masked key keeps the one kept).
    fn save_model(self: &Arc<Self>, caller: &Identity, req: &[u8], a: Answer) {
        let mut m = match Req::parse(req).and_then(|r| model_of(&r)) {
            Ok(m) => m,
            Err(e) => return a.err(e),
        };
        if let Err(e) = valid_model(&mut m) {
            return a.err(e);
        }
        let m2 = m.clone();
        let saved: Arc<std::sync::Mutex<LlmModel>> = Arc::new(std::sync::Mutex::new(m.clone()));
        let s2 = Arc::clone(&saved);
        let role = caller.role.clone();
        self.edit_models(
            move |models| {
                let mut m = m2;
                let idx = models
                    .iter()
                    .position(|x| x.name == m.name && x.api == m.api);
                if m.api_key.is_empty() || is_masked(&m.api_key) {
                    m.api_key = idx
                        .and_then(|i| models.get(i))
                        .map(|x| x.api_key.clone())
                        .unwrap_or_default();
                }
                *lock(&s2) = m.clone();
                match idx.and_then(|i| models.get_mut(i)) {
                    Some(x) => *x = m,
                    None => models.push(m),
                }
                Ok(())
            },
            answer(self.me.clone(), a, move |_| {
                let m = lock(&saved).clone();
                akit_observe::info!(
                    Llm,
                    "llm.node: model {} ({}) saved by {role}, base_url {}",
                    m.name,
                    m.api,
                    m.base_url
                );
                model_json(&m)
            }),
        );
    }

    /// `/node/model/delete`.
    fn delete_model(self: &Arc<Self>, caller: &Identity, req: &[u8], a: Answer) {
        let (name, api) = match Req::parse(req).and_then(|r| Ok((r.str("name")?, r.str("api")?))) {
            Ok(v) => v,
            Err(e) => return a.err(e),
        };
        let role = caller.role.clone();
        let n2 = name.clone();
        self.edit_models(
            move |models| {
                let i = find(models, &n2, &api)?;
                models.remove(i);
                Ok(())
            },
            answer(self.me.clone(), a, move |_| {
                akit_observe::info!(Llm, "llm.node: model {name} deleted by {role}");
                "{}".to_owned()
            }),
        );
    }

    /// `/node/model/check`: the model's upstream probed now, its state answered.
    fn check_model(self: &Arc<Self>, _: &Identity, req: &[u8], a: Answer) {
        let (name, api) = match Req::parse(req).and_then(|r| Ok((r.str("name")?, r.str("api")?))) {
            Ok(v) => v,
            Err(e) => return a.err(e),
        };
        let m = {
            let st = lock(&self.st);
            match find(&st.models, &name, &api) {
                Ok(i) => st.models.get(i).cloned(),
                Err(e) => return a.err(e),
            }
        };
        let Some(m) = m else { return };
        let me = self.me.clone();
        let m2 = m.clone();
        self.check(&m, move || {
            let Some(c) = me.upgrade() else {
                return a.err(Error::Closed);
            };
            let st = lock(&c.st);
            a.ok(view(&m2, st.rt.get(&key(&m2.name, &m2.api))).to_json());
        });
    }

    /// `/node/profile/set`: the node's name (1 to 64 characters) and description (1,024 bytes
    /// at most).
    fn set_profile(self: &Arc<Self>, caller: &Identity, req: &[u8], a: Answer) {
        let (name, desc) =
            match Req::parse(req).and_then(|r| Ok((r.str("name")?, r.str("description")?))) {
                Ok((n, d)) => (n.trim().to_owned(), d.trim().to_owned()),
                Err(e) => return a.err(e),
            };
        if name.is_empty() || name.chars().count() > 64 {
            return a.err(bad_request("the name needs 1-64 characters"));
        }
        if desc.len() > 1024 {
            return a.err(bad_request("the description is longer than 1024 bytes"));
        }
        let role = caller.role.clone();
        self.base.edit(
            move |m| {
                set(m, "name", Value::String(name));
                set(m, "description", Value::String(desc));
                Ok(())
            },
            answer(self.me.clone(), a, move |_| {
                akit_observe::info!(Llm, "llm.node: profile set by {role}");
                "{}".to_owned()
            }),
        );
    }

    fn settings_json(&self) -> String {
        let s = lock(&self.st).set.clone();
        Obj::new()
            .bool("allow_hub_manage", s.allow_hub_manage)
            .bool("gpu_sudo", s.gpu_sudo)
            .str("refresh_interval", &go(s.refresh))
            .str("job_timeout", &go(s.job_timeout))
            .str_omit("gpu_data_dir", &s.gpu_data_dir)
            .end()
    }

    /// `/node/settings/set`: the settings written into the member, the defaults and false
    /// values left out.
    fn set_settings(self: &Arc<Self>, _: &Identity, req: &[u8], a: Answer) {
        let parse = |name: &str, v: &str, lo: Duration, hi: Duration| -> Result<String, Error> {
            if v.is_empty() {
                return Ok(String::new());
            }
            match akit_config::duration::parse(v) {
                Some(Ok(d)) if d >= lo && d <= hi => Ok(go(d)),
                _ => Err(bad_request(format!(
                    "{name} must be a duration from {} to {}",
                    go(lo),
                    go(hi)
                ))),
            }
        };
        let r = match Req::parse(req) {
            Ok(r) => r,
            Err(e) => return a.err(e),
        };
        let fields = (|| -> Result<_, Error> {
            Ok((
                r.bool("allow_hub_manage")?,
                r.bool("gpu_sudo")?,
                parse(
                    "refresh_interval",
                    &r.str("refresh_interval")?,
                    Duration::from_secs(5),
                    Duration::from_secs(3600),
                )?,
                parse(
                    "job_timeout",
                    &r.str("job_timeout")?,
                    Duration::from_secs(10),
                    Duration::from_secs(7 * 24 * 3600),
                )?,
                r.str("gpu_data_dir")?,
            ))
        })();
        let (manage, sudo, refresh, job, dir) = match fields {
            Ok(v) => v,
            Err(e) => return a.err(e),
        };
        if dir.len() > 4096 {
            return a.err(bad_request("gpu_data_dir is too long"));
        }
        // As Go writes a map: its keys sorted.
        let mut s: Vec<(String, Value)> = Vec::new();
        if manage {
            s.push(("allow_hub_manage".into(), Value::Bool(true)));
        }
        if !dir.is_empty() {
            s.push(("gpu_data_dir".into(), Value::String(dir)));
        }
        if !sudo {
            s.push(("gpu_sudo".into(), Value::Bool(false)));
        }
        if !job.is_empty() && job != go(akit_config::roles::DEFAULT_LLM_JOB_TIMEOUT) {
            s.push(("job_timeout".into(), Value::String(job)));
        }
        if !refresh.is_empty() && refresh != go(akit_config::roles::DEFAULT_LLM_REFRESH_INTERVAL) {
            s.push(("refresh_interval".into(), Value::String(refresh)));
        }
        self.base.edit(
            move |m| {
                set(m, "settings", Value::Object(s));
                Ok(())
            },
            answer(self.me.clone(), a, |c| c.settings_json()),
        );
    }

    fn upstream_json(&self) -> String {
        let p = lock(&self.st).proxy.clone();
        let p = p.as_ref();
        Obj::new()
            .str("connect_timeout", &go(LlmNodeProxy::connect(p)))
            .str("first_byte_timeout", &go(LlmNodeProxy::first_byte(p)))
            .str("idle_timeout", &go(LlmNodeProxy::idle(p)))
            .str("timeout", &go(LlmNodeProxy::total(p)))
            .int(
                "max_inflight",
                i64::try_from(LlmNodeProxy::inflight(p)).unwrap_or(i64::MAX),
            )
            .int(
                "max_body_bytes",
                i64::try_from(LlmNodeProxy::body(p)).unwrap_or(i64::MAX),
            )
            .end()
    }

    /// `/node/upstream/set`: the bounds of the calls the node forwards, written into the
    /// member's `proxy` (the defaults left out), answered as applied. Calls in flight keep
    /// theirs.
    fn set_upstream(self: &Arc<Self>, _: &Identity, req: &[u8], a: Answer) {
        use akit_config::roles as c;
        let r = match Req::parse(req) {
            Ok(r) => r,
            Err(e) => return a.err(e),
        };
        let fields = (|| -> Result<Vec<(String, Value)>, Error> {
            let d = |f: &str, def: Duration| -> Result<String, Error> {
                duration_setting(f, &r.str(f)?, Duration::ZERO, def)
            };
            let connect = d("connect_timeout", c::DEFAULT_LLM_CONNECT_TIMEOUT)?;
            let first = d("first_byte_timeout", c::DEFAULT_LLM_FIRST_BYTE_TIMEOUT)?;
            let idle = d("idle_timeout", c::DEFAULT_LLM_IDLE_TIMEOUT)?;
            let total = d("timeout", c::DEFAULT_LLM_CALL_TIMEOUT)?;
            let inflight = count_setting(
                "max_inflight",
                r.int("max_inflight")?,
                i64::try_from(c::DEFAULT_LLM_NODE_MAX_INFLIGHT).unwrap_or(0),
            )?;
            let body = count_setting(
                "max_body_bytes",
                r.int("max_body_bytes")?,
                i64::try_from(c::DEFAULT_LLM_MAX_BODY_BYTES).unwrap_or(0),
            )?;
            // As Go writes a map: its keys sorted, the empty ones left out.
            let mut o = Vec::new();
            for (k, v) in [
                ("connect_timeout", connect),
                ("first_byte_timeout", first),
                ("idle_timeout", idle),
            ] {
                if !v.is_empty() {
                    o.push((k.to_owned(), Value::String(v)));
                }
            }
            if body != 0 {
                o.push(("max_body_bytes".to_owned(), number(body)));
            }
            if inflight != 0 {
                o.push(("max_inflight".to_owned(), number(inflight)));
            }
            if !total.is_empty() {
                o.push(("timeout".to_owned(), Value::String(total)));
            }
            Ok(o)
        })();
        let p = match fields {
            Ok(p) => p,
            Err(e) => return a.err(e),
        };
        self.base.edit(
            move |m| {
                set(m, "proxy", Value::Object(p));
                Ok(())
            },
            answer(self.me.clone(), a, |c| c.upstream_json()),
        );
    }

    // ---- GPUs, actions and jobs ----

    fn gpu_opts(&self) -> gpu::Options {
        gpu::Options {
            use_sudo: lock(&self.st).set.gpu_sudo,
        }
    }

    fn gpu_env(&self) -> Vec<String> {
        let dir = lock(&self.st).set.gpu_data_dir.clone();
        if dir.is_empty() {
            Vec::new()
        } else {
            vec![format!("{}={dir}", gpu::ENV_DATA_DIR)]
        }
    }

    /// The GPUs' status, read at most every 5 s (`fresh`: now), on the node's pool, one read
    /// at a time; `done` gets it, an empty one when the pool could not read it in time.
    pub(super) fn gpu_status(
        self: &Arc<Self>,
        fresh: bool,
        done: impl FnOnce(gpu::Status) + Send + 'static,
    ) {
        let opts = self.gpu_opts();
        let cached = move |c: &Core| {
            lock(&c.gpu)
                .status
                .as_ref()
                .filter(|(_, at, sudo)| *sudo == opts.use_sudo && at.elapsed() < GPU_CACHE_TTL)
                .map(|(s, _, _)| s.clone())
        };
        if !fresh {
            if let Some(s) = cached(self) {
                return done(s);
            }
        }
        let me = Arc::downgrade(self);
        let queued = Instant::now();
        let done: Arc<std::sync::Mutex<Option<StatusFn>>> =
            Arc::new(std::sync::Mutex::new(Some(Box::new(done))));
        let d2 = Arc::clone(&done);
        let run = move || {
            let Some(c) = me.upgrade() else { return };
            if queued.elapsed() > GPU_READ_TIMEOUT {
                // Waited too long for the pool: answered empty, as fit's timed out read.
                if let Some(f) = lock(&d2).take() {
                    f(empty_status(opts));
                }
                return;
            }
            let st = {
                // One read at a time.
                let _g = lock(&c.gpu_read);
                match (!fresh).then(|| cached(&c)).flatten() {
                    Some(s) => s,
                    None => {
                        let s = gpu::collect(&c.work, opts);
                        lock(&c.gpu).status = Some((s.clone(), Instant::now(), opts.use_sudo));
                        s
                    }
                }
            };
            if let Some(f) = lock(&d2).take() {
                f(st);
            }
        };
        if self.work.try_execute(Box::new(run)).is_err() {
            if let Some(f) = lock(&done).take() {
                f(empty_status(opts));
            }
        }
    }

    /// Starts a job with the node's job timeout; a full house is rate limited.
    fn start_job(&self, kind: &str, title: &str, run: Run) -> Result<String, Error> {
        let timeout = lock(&self.st).set.job_timeout;
        self.jobs
            .start(kind, title, timeout, run)
            .map(|j| j.to_json())
            .map_err(|e| Error::code(CODE_RATE_LIMITED, e))
    }

    /// `/node/gpu/install`: a vendor's tool installed, as a job.
    fn gpu_install(self: &Arc<Self>, _: &Identity, req: &[u8], a: Answer) {
        let vendor = match Req::parse(req).and_then(|r| r.str("vendor")) {
            Ok(v) => v,
            Err(e) => return a.err(e),
        };
        let (title, argv) = match gpu::install_command(&vendor) {
            Ok(v) => v,
            Err(e) => return a.err(bad_request(e)),
        };
        let c = Command {
            argv: self.gpu_opts().wrap(argv),
            env: self.gpu_env(),
            ..Command::default()
        };
        a.answer(self.start_job(KIND_GPU_INSTALL, &title, run_command(c)));
    }

    /// `/node/gpu/set`: one setting applied as a job, to one GPU or, with a negative index,
    /// to every GPU of the vendor one after another.
    fn gpu_set(self: &Arc<Self>, _: &Identity, req: &[u8], a: Answer) {
        let fields = Req::parse(req).and_then(|r| {
            Ok((
                r.str("vendor")?,
                r.int("index")?,
                r.str("key")?,
                r.str("value")?,
            ))
        });
        let (vendor, index, k, value) = match fields {
            Ok(v) => v,
            Err(e) => return a.err(e),
        };
        let me = Arc::downgrade(self);
        let go_on = move |r: Result<(Vec<i64>, String), Error>| {
            let (indices, target) = match r {
                Ok(v) => v,
                Err(e) => return a.err(e),
            };
            let Some(c) = me.upgrade() else {
                return a.err(Error::Closed);
            };
            let opts = c.gpu_opts();
            let mut cmds = Vec::new();
            for i in indices {
                match gpu::command(&vendor, i, &k, &value) {
                    Ok(argv) => cmds.push(Command {
                        argv: opts.wrap(argv),
                        env: c.gpu_env(),
                        ..Command::default()
                    }),
                    Err(e) => return a.err(bad_request(e)),
                }
            }
            let mut title = format!("{vendor} {k}");
            if !value.is_empty() {
                title.push('=');
                title.push_str(&value);
            }
            title.push_str(" · ");
            title.push_str(&target);
            a.answer(c.start_job(KIND_GPU_SET, &title, run_commands(cmds)));
        };
        if index >= 0 {
            return go_on(Ok((vec![index], format!("GPU {index}"))));
        }
        let v2 = vendor_of(req);
        self.gpu_status(false, move |st| {
            let indices = gpu::device_indices(&st, &v2);
            if indices.is_empty() {
                return go_on(Err(bad_request(format!("no {v2} GPU found"))));
            }
            go_on(Ok((indices, "all GPUs".to_owned())));
        });
    }

    /// `/node/action/run`: a fixed action run as a job.
    fn run_action(self: &Arc<Self>, _: &Identity, req: &[u8], a: Answer) {
        let fields = Req::parse(req).and_then(|r| Ok((r.str("id")?, r.str_map("params")?)));
        let (id, params) = match fields {
            Ok(v) => v,
            Err(e) => return a.err(e),
        };
        let (c, title) = match actions::build(&id, &params) {
            Ok(v) => v,
            Err(e) => return a.err(bad_request(e)),
        };
        let c = c.sudo(lock(&self.st).set.gpu_sudo);
        a.answer(self.start_job(KIND_ACTION, &title, run_command(c)));
    }
}

fn vendor_of(req: &[u8]) -> String {
    Req::parse(req)
        .and_then(|r| r.str("vendor"))
        .unwrap_or_default()
}

/// The status of a read that could not be made.
fn empty_status(opts: gpu::Options) -> gpu::Status {
    gpu::Status {
        os: actions::goos().to_owned(),
        root: crate::exec::is_root(),
        use_sudo: opts.use_sudo,
        vendors: Vec::new(),
        devices: Vec::new(),
        raw: Vec::new(),
        errors: Vec::new(),
        collected_at: akit_roles::linktoken::unix_now(),
    }
}
