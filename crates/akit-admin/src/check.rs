//! The consistency check (I138, fit's `admin/drift.go`): `GET /config/check` compares what akit
//! runs with the config it runs from, and `POST /config/reconcile` applies the fixes it offers.
//!
//! A difference is a drift (something to act on, with the fixes the check offers: a patch of the
//! base config, or the config applied again) or an info (a known fact of how akit runs). The
//! categories, in fit's order: settings that take a restart (as the warnings of a config applied
//! say: `access_log`'s output, `log`'s outputs and formats, `telemetry`, `engine`, `acme`,
//! `providers`, `limits.max_conns`, `max_write_queue_bytes` and `max_pending_bytes`; `admin`, an
//! info), listeners bound or not, the certificates asked for and served, upstreams running or
//! not and with every backend out of rotation, the roles set and running, cache zones used and
//! kept, and what providers provided. What runs is read from `GET /status`'s writer and `GET
//! /node`'s; without them, only the config's settings and its providers are checked.
//!
//! A reconcile takes `If-Match` the check's ETag (428 without), checks again, and applies the
//! fixes named, the deletes last and from the end of their arrays, as a new version.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::time::SystemTime;

use akio_http::Response;
use akit_config::value::{Value, write_string};

use crate::configs::{Configs, Refused, Version};
use crate::node::NodeView;
use crate::pointer;
use crate::server::{Live, VERSION_HEADER, json};

/// A difference to act on.
pub(crate) const DRIFT: &str = "drift";
/// A difference that is a known fact, not an inconsistency.
pub(crate) const INFO: &str = "info";

/// The categories, in the order the check answers them.
const CATEGORIES: &[&str] = &[
    "settings",
    "listeners",
    "routes",
    "upstreams",
    "services",
    "tls",
    "roles",
    "cache",
    "providers",
];

/// A change of the base config a fix makes: fit's `ConfigPatch`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Patch {
    op: &'static str,
    path: String,
    value: Option<Value>,
}

/// A way to act on a drift: fit's `Fix`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Fix {
    action: &'static str,
    label: &'static str,
    patch: Option<Patch>,
}

/// One difference: fit's `Difference`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Difference {
    id: String,
    category: &'static str,
    kind: &'static str,
    name: String,
    detail: String,
    config: Option<Value>,
    running: Option<Value>,
    fixes: Vec<Fix>,
    /// Where its values are in a config, for their secrets to be redacted (decision 035).
    at: String,
}

/// `v`, at the pointer `at` of a config, as JSON with its secrets redacted.
fn redacted(v: &Value, at: &str) -> String {
    let mut c = v.clone();
    let tokens = pointer::parse(at).unwrap_or_default();
    crate::redact::at_pointer(&mut c, &tokens);
    c.to_json().trim_end().to_owned()
}

fn s(v: &str) -> Value {
    Value::String(v.to_owned())
}

fn reapply(label: &'static str) -> Fix {
    Fix {
        action: "reapply",
        label,
        patch: None,
    }
}

fn delete(path: String, label: &'static str) -> Fix {
    Fix {
        action: "delete",
        label,
        patch: Some(Patch {
            op: "delete",
            path,
            value: None,
        }),
    }
}

impl Difference {
    fn new(id: String, category: &'static str, kind: &'static str, name: &str) -> Difference {
        Difference {
            id,
            category,
            kind,
            name: name.to_owned(),
            detail: String::new(),
            config: None,
            running: None,
            fixes: Vec::new(),
            at: String::new(),
        }
    }

    fn write(&self, b: &mut String) {
        b.push_str("{\"id\":");
        write_string(b, &self.id);
        b.push_str(",\"category\":");
        write_string(b, self.category);
        b.push_str(",\"kind\":");
        write_string(b, self.kind);
        b.push_str(",\"name\":");
        write_string(b, &self.name);
        b.push_str(",\"detail\":");
        write_string(b, &self.detail);
        for (k, v) in [("config", &self.config), ("running", &self.running)] {
            if let Some(v) = v {
                let _ = write!(b, ",\"{k}\":{}", redacted(v, &self.at));
            }
        }
        if !self.fixes.is_empty() {
            b.push_str(",\"fixes\":[");
            for (i, f) in self.fixes.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                b.push_str("{\"action\":");
                write_string(b, f.action);
                b.push_str(",\"label\":");
                write_string(b, f.label);
                if let Some(p) = &f.patch {
                    b.push_str(",\"patch\":{\"op\":");
                    write_string(b, p.op);
                    b.push_str(",\"path\":");
                    write_string(b, &p.path);
                    if let Some(v) = &p.value {
                        b.push_str(",\"value\":");
                        b.push_str(&redacted(v, &p.path));
                    }
                    b.push('}');
                }
                b.push('}');
            }
            b.push(']');
        }
        b.push('}');
    }
}

/// What runs, as `GET /status` and `GET /node` read it.
#[derive(Debug, Default)]
struct Running {
    /// `None` when nothing says what runs.
    status: Option<Value>,
    roles: Option<Vec<String>>,
}

fn arr(v: Option<&Value>) -> &[Value] {
    match v {
        Some(Value::Array(a)) => a,
        _ => &[],
    }
}

fn str_of<'v>(v: &'v Value, k: &str) -> &'v str {
    match v.get(k) {
        Some(Value::String(s)) => s,
        _ => "",
    }
}

/// The index of the element named `name` of the array `member` in `base`.
fn base_index(base: &Value, member: &str, name: &str) -> Option<usize> {
    arr(base.get(member))
        .iter()
        .position(|e| str_of(e, "name") == name)
}

/// The settings that take a restart and differ from the ones akit started with, by the rules
/// of the warnings a config applied gets.
fn settings(first: &akit_config::Config, cur: &akit_config::Config) -> Vec<&'static str> {
    let mut out = Vec::new();
    let access =
        |a: &akit_config::AccessLog| (a.output.clone(), a.format, a.flush_interval, a.buffer_bytes);
    match (&first.access_log, &cur.access_log) {
        (None, Some(_)) => out.push("access_log"),
        (Some(f), Some(c)) if access(f) != access(c) => out.push("access_log"),
        _ => {}
    }
    let shape = |l: Option<&akit_config::Log>| {
        let default = akit_config::Log::default();
        let l = l.unwrap_or(&default);
        akit_config::LOGGERS
            .iter()
            .map(|n| {
                let out = l.output_of(n);
                let out = if akit_config::is_file_output(out) {
                    akit_config::output_path(out)
                } else {
                    out.to_owned()
                };
                (out, l.format_of(n))
            })
            .collect::<Vec<_>>()
    };
    if shape(first.log.as_ref()) != shape(cur.log.as_ref()) {
        out.push("log");
    }
    if first.telemetry != cur.telemetry {
        out.push("telemetry");
    }
    if first.engine != cur.engine {
        out.push("engine");
    }
    if first.acme != cur.acme {
        out.push("acme");
    }
    if first.providers != cur.providers {
        out.push("providers");
    }
    let (f, c) = (&first.limits, &cur.limits);
    if f.max_conns != c.max_conns {
        out.push("limits.max_conns");
    }
    if f.max_write_queue_bytes != c.max_write_queue_bytes {
        out.push("limits.max_write_queue_bytes");
    }
    if f.max_pending_bytes != c.max_pending_bytes {
        out.push("limits.max_pending_bytes");
    }
    out
}

/// The value at `name` (`limits.max_conns` is `limits`'s member) in a config's JSON.
fn member<'v>(root: &'v Value, name: &str) -> Option<&'v Value> {
    name.split('.').try_fold(root, |v, k| v.get(k))
}

fn check_settings(first: &Version, cur: &Version, diffs: &mut Vec<Difference>) {
    let parse = |v: &Version| akit_config::from_json(v.effective(), None).ok();
    let (Some(fc), Some(cc)) = (parse(first), parse(cur)) else {
        return;
    };
    let fj = akit_config::value::parse(first.data()).unwrap_or(Value::Null);
    let cj = akit_config::value::parse(cur.data()).unwrap_or(Value::Null);
    for name in settings(&fc, &cc) {
        let mut d = Difference::new(format!("setting:{name}"), "settings", DRIFT, name);
        d.detail = format!(
            "the config sets {name} differently from the value akit started with, which is in \
             force until akit restarts"
        );
        d.config = member(&cj, name).cloned();
        d.running = member(&fj, name).cloned();
        let path = format!("/{}", name.replace('.', "/"));
        d.at.clone_from(&path);
        d.fixes.push(Fix {
            action: "replace",
            label: "revert the config to the value in force",
            patch: Some(match &d.running {
                Some(v) => Patch {
                    op: "replace",
                    path,
                    value: Some(v.clone()),
                },
                None => Patch {
                    op: "delete",
                    path,
                    value: None,
                },
            }),
        });
        diffs.push(d);
    }
    if crate::admin_warning(fc.admin.as_ref(), cc.admin.as_ref()).is_some() {
        let mut d = Difference::new("setting:admin".to_owned(), "settings", INFO, "admin");
        d.detail = "admin comes from the config akit started with; a change to it takes effect \
                    when akit restarts"
            .to_owned();
        d.config = cj.get("admin").cloned();
        d.running = fj.get("admin").cloned();
        "/admin".clone_into(&mut d.at);
        diffs.push(d);
    }
}

fn check_roles(cfg: &akit_config::Config, running: &[String], diffs: &mut Vec<Difference>) {
    for (id, set) in akit_roles::host::members(cfg) {
        let up = running.iter().any(|r| r == id);
        if set && !up {
            let mut d = Difference::new(format!("role:{id}"), "roles", DRIFT, id);
            d.detail = format!("the config sets {id}, but the role is not running");
            d.running = Some(Value::Bool(false));
            d.fixes = vec![
                delete(
                    format!("/{}", id.replace('.', "/")),
                    "remove it from the config",
                ),
                reapply("apply the config again to retry starting it"),
            ];
            diffs.push(d);
        } else if !set && up {
            let mut d = Difference::new(format!("role:{id}:extra"), "roles", DRIFT, id);
            d.detail = format!("the role {id} is running, but the config does not set it");
            d.running = Some(Value::Bool(true));
            d.fixes = vec![reapply("apply the config again to stop it")];
            diffs.push(d);
        }
    }
}

/// An element's name for the check: its `name`, or its handler's module, or its path.
fn element_name(v: &Value, path: &str) -> String {
    let n = str_of(v, "name");
    if !n.is_empty() {
        return n.to_owned();
    }
    let m = v.get("handler").map_or("", |h| str_of(h, "module"));
    if m.is_empty() {
        path.to_owned()
    } else {
        m.to_owned()
    }
}

fn check_providers(cur: &Version, diffs: &mut Vec<Difference>) {
    if let Ok(tree) = akit_config::value::parse(cur.effective()) {
        let mut seen = BTreeSet::new();
        for src in cur.sources() {
            if *src.source == *cur.base_source() {
                continue;
            }
            let Some(kind) = akit_provider::KINDS
                .iter()
                .find(|k| src.path.starts_with(&format!("/{k}/")))
            else {
                continue;
            };
            if !seen.insert(src.path.clone()) {
                continue;
            }
            let Ok(tokens) = pointer::parse(&src.path) else {
                continue;
            };
            let Ok(v) = pointer::lookup(&tree, &tokens) else {
                continue;
            };
            let mut d = Difference::new(
                format!("provider:{}:{}", src.source, src.path),
                kind,
                DRIFT,
                &element_name(v, &src.path),
            );
            d.detail = format!(
                "provided by {}, and running, but not in the base config",
                src.source
            );
            d.running = Some(v.clone());
            d.at.clone_from(&src.path);
            d.fixes = vec![Fix {
                action: "add",
                label: "save it in the config",
                patch: Some(Patch {
                    op: "add",
                    path: format!("/{kind}/-"),
                    value: Some(v.clone()),
                }),
            }];
            diffs.push(d);
        }
    }
    for sh in cur.shadowed() {
        let mut d = Difference::new(
            format!("shadowed:{}:{}", sh.source, sh.name),
            "providers",
            INFO,
            &sh.name,
        );
        d.detail = format!(
            "provided by {}, but left out because {} already uses the name",
            sh.source, sh.by
        );
        diffs.push(d);
    }
}

/// Whether `address`'s port is 0.
fn port_zero(address: &str) -> bool {
    address.rsplit_once(':').is_some_and(|(_, p)| p == "0")
}

fn check_listeners(
    cfg: &akit_config::Config,
    base: &Value,
    status: &Value,
    diffs: &mut Vec<Difference>,
) {
    // With no listeners written, those akit serves are made by HTTPS by default.
    let generated = arr(base.get("listeners")).is_empty();
    let bound: Vec<(&str, &str)> = arr(status.get("listeners"))
        .iter()
        .map(|l| (str_of(l, "name"), str_of(l, "address")))
        .collect();
    let bound_addr = |name: &str| bound.iter().find(|(n, _)| *n == name).map(|(_, a)| *a);
    if generated {
        return;
    }
    for l in &cfg.listeners {
        match bound_addr(&l.name) {
            None => {
                let mut d = Difference::new(
                    format!("listener:{}:missing", l.name),
                    "listeners",
                    DRIFT,
                    &l.name,
                );
                d.detail = format!(
                    "the config serves listener {}, but no address is bound for it",
                    l.name
                );
                d.config = Some(s(&l.address));
                if let Some(i) = base_index(base, "listeners", &l.name) {
                    d.fixes.push(delete(
                        format!("/listeners/{i}"),
                        "remove it from the config",
                    ));
                }
                d.fixes
                    .push(reapply("apply the config again to retry binding it"));
                diffs.push(d);
            }
            Some(a) if port_zero(&l.address) => {
                let mut d = Difference::new(
                    format!("listener:{}:port", l.name),
                    "listeners",
                    INFO,
                    &l.name,
                );
                d.detail = format!(
                    "the config writes address {}, whose port is 0, so the kernel chose {a}: a \
                     dynamic port is deliberate",
                    l.address
                );
                d.config = Some(s(&l.address));
                d.running = Some(s(a));
                diffs.push(d);
            }
            Some(_) => {}
        }
    }
    for (name, a) in &bound {
        if cfg.listeners.iter().any(|l| l.name == *name) {
            continue;
        }
        let mut d = Difference::new(format!("listener:{name}:extra"), "listeners", DRIFT, name);
        d.detail = format!("a listener {name} is bound, but the config does not serve it");
        d.running = Some(s(a));
        d.fixes = vec![reapply("apply the config again to stop it")];
        diffs.push(d);
    }
}

/// Whether a certificate for `served` covers `name`: the same, or a wildcard one label above.
fn covers(served: &str, name: &str) -> bool {
    let served = served.trim_end_matches('.').to_ascii_lowercase();
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    if served == name {
        return true;
    }
    let Some(suffix) = served.strip_prefix("*.") else {
        return false;
    };
    name.strip_suffix(suffix)
        .and_then(|l| l.strip_suffix('.'))
        .is_some_and(|label| !label.is_empty() && !label.contains('.'))
}

fn check_tls(cfg: &akit_config::Config, base: &Value, status: &Value, diffs: &mut Vec<Difference>) {
    for (li, l) in cfg.listeners.iter().enumerate() {
        let Some(tls) = &l.tls else { continue };
        let mut names: Vec<String> = arr(status.get("certificates"))
            .iter()
            .filter(|c| str_of(c, "listener") == l.name)
            .flat_map(|c| arr(c.get("names")).iter())
            .filter_map(|n| match n {
                Value::String(s) if !s.is_empty() => Some(s.clone()),
                _ => None,
            })
            .collect();
        names.sort();
        names.dedup();
        let served =
            (!names.is_empty()).then(|| Value::Array(names.iter().map(|n| s(n)).collect()));
        let at = base_index(base, "listeners", &l.name);
        for name in &tls.acme {
            if names.iter().any(|have| covers(have, name)) {
                continue;
            }
            let j = at.and_then(|i| {
                let written = arr(base.get("listeners"))
                    .get(i)
                    .and_then(|x| x.get("tls"))
                    .and_then(|t| t.get("acme"));
                arr(written)
                    .iter()
                    .position(|n| matches!(n, Value::String(s) if s == name))
                    .map(|j| (i, j))
            });
            match j {
                Some((i, j)) => {
                    let mut d =
                        Difference::new(format!("tls:{}:{name}", l.name), "tls", DRIFT, name);
                    d.detail = format!(
                        "the config wants a certificate for {name} on listener {}, but none is \
                         served for it",
                        l.name
                    );
                    d.config = Some(s(name));
                    d.running.clone_from(&served);
                    d.fixes = vec![delete(
                        format!("/listeners/{i}/tls/acme/{j}"),
                        "remove the name from the config",
                    )];
                    diffs.push(d);
                }
                None => {
                    let mut d =
                        Difference::new(format!("tls:{}:{name}:auto", l.name), "tls", INFO, name);
                    d.detail = format!(
                        "HTTPS by default added {name} to listener {}, and its certificate is \
                         being obtained",
                        l.name
                    );
                    d.running.clone_from(&served);
                    diffs.push(d);
                }
            }
        }
        for name in &names {
            if tls
                .acme
                .iter()
                .any(|want| covers(name, want) || covers(want, name))
            {
                continue;
            }
            let mut d = Difference::new(format!("tls:{}:served:{name}", l.name), "tls", INFO, name);
            d.detail = format!(
                "listener {} serves a certificate for {name}, which the config does not ask for: \
                 on-demand TLS, or a certificate from a file",
                l.name
            );
            d.running = Some(s(name));
            diffs.push(d);
        }
        let _ = li;
    }
}

fn check_upstreams(
    cfg: &akit_config::Config,
    base: &Value,
    status: &Value,
    diffs: &mut Vec<Difference>,
) {
    let pools = arr(status.get("upstreams"));
    for u in &cfg.upstreams {
        if pools.iter().any(|p| str_of(p, "name") == u.name) {
            continue;
        }
        let mut d = Difference::new(
            format!("upstream:{}:missing", u.name),
            "upstreams",
            DRIFT,
            &u.name,
        );
        d.detail = format!(
            "the config sets upstream {}, but no pool of that name is running",
            u.name
        );
        d.config = Some(Value::Array(u.addresses.iter().map(|a| s(a)).collect()));
        if let Some(i) = base_index(base, "upstreams", &u.name) {
            d.fixes.push(delete(
                format!("/upstreams/{i}"),
                "remove it from the config",
            ));
        }
        d.fixes.push(reapply("apply the config again to start it"));
        diffs.push(d);
    }
    for p in pools {
        let name = str_of(p, "name");
        let backends = arr(p.get("backends"));
        if !cfg.upstreams.iter().any(|u| u.name == name) {
            let mut d = Difference::new(format!("upstream:{name}:extra"), "upstreams", DRIFT, name);
            d.detail = format!("upstream {name} is running, but the config does not set it");
            d.running = Some(Value::Array(
                backends.iter().map(|b| s(str_of(b, "address"))).collect(),
            ));
            d.fixes = vec![reapply("apply the config again to stop it")];
            diffs.push(d);
            continue;
        }
        let down = !backends.is_empty()
            && backends
                .iter()
                .all(|b| matches!(b.get("up"), Some(Value::Bool(false))));
        if down {
            let mut d = Difference::new(format!("upstream:{name}:down"), "upstreams", INFO, name);
            d.detail = format!("every backend of upstream {name} is out of rotation");
            d.running = Some(Value::Array(
                backends
                    .iter()
                    .map(|b| {
                        Value::Object(vec![
                            ("address".to_owned(), s(str_of(b, "address"))),
                            ("up".to_owned(), Value::Bool(false)),
                            (
                                "in_flight".to_owned(),
                                b.get("in_flight")
                                    .cloned()
                                    .unwrap_or(Value::Number("0".to_owned())),
                            ),
                        ])
                    })
                    .collect(),
            ));
            diffs.push(d);
        }
    }
}

fn check_cache(effective: &Value, diffs: &mut Vec<Difference>) {
    let mut in_use = BTreeSet::new();
    for r in arr(effective.get("routes")) {
        for m in arr(r.get("middleware")) {
            if str_of(m, "module") == "cache" {
                let z = m.get("params").map_or("", |p| str_of(p, "zone"));
                in_use.insert(if z.is_empty() { "default" } else { z }.to_owned());
            }
        }
    }
    let zones = akit_cache::zones();
    for name in &in_use {
        if zones.iter().any(|z| &z.zone == name) {
            continue;
        }
        let mut d = Difference::new(format!("cache:zone:{name}:missing"), "cache", DRIFT, name);
        d.detail = format!("a route uses cache zone {name}, but no zone of that name is running");
        d.config = Some(s(name));
        d.fixes = vec![reapply("apply the config again to open the zone")];
        diffs.push(d);
    }
    for z in &zones {
        if in_use.contains(&z.zone) {
            continue;
        }
        let mut d = Difference::new(
            format!("cache:zone:{}:kept", z.zone),
            "cache",
            INFO,
            &z.zone,
        );
        d.detail = format!(
            "cache zone {} is kept, though no route uses it now: a config applied before still \
             holds it",
            z.zone
        );
        let n = |v: u64| Value::Number(v.to_string());
        d.running = Some(Value::Object(vec![
            ("keys".to_owned(), n(z.keys as u64)),
            ("bytes".to_owned(), n(z.bytes)),
            ("disk_entries".to_owned(), n(z.disk_entries)),
            ("disk_bytes".to_owned(), n(z.disk_bytes)),
        ]));
        diffs.push(d);
    }
}

/// The differences between what runs and `cur`, the running config.
fn differences(configs: &Configs, cur: &Version, run: &Running) -> Vec<Difference> {
    let mut diffs = Vec::new();
    check_settings(&configs.first(), cur, &mut diffs);
    let cfg = akit_config::from_json(cur.effective(), None).ok();
    if let (Some(cfg), Some(roles)) = (&cfg, &run.roles) {
        check_roles(cfg, roles, &mut diffs);
    }
    check_providers(cur, &mut diffs);
    let base = akit_config::value::parse(cur.data()).unwrap_or(Value::Null);
    if let (Some(cfg), Some(status)) = (&cfg, &run.status) {
        check_listeners(cfg, &base, status, &mut diffs);
        check_tls(cfg, &base, status, &mut diffs);
        check_upstreams(cfg, &base, status, &mut diffs);
    }
    if let Ok(eff) = akit_config::value::parse(cur.effective()) {
        check_cache(&eff, &mut diffs);
    }
    diffs.sort_by(|a, b| {
        let pos = |c: &str| {
            CATEGORIES
                .iter()
                .position(|x| *x == c)
                .unwrap_or(usize::MAX)
        };
        pos(a.category)
            .cmp(&pos(b.category))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.id.cmp(&b.id))
    });
    diffs
}

fn running(status: Option<&Live>, node: Option<&NodeView>) -> Running {
    Running {
        status: status.and_then(|w| {
            let mut b = String::from("{");
            w(&mut b);
            b.push('}');
            akit_config::value::parse(&b).ok()
        }),
        roles: node.map(|n| n().roles),
    }
}

/// `GET /config/check`.
pub(crate) fn check(configs: &Configs, status: Option<&Live>, node: Option<&NodeView>) -> Response {
    let cur = configs.current();
    let diffs = differences(configs, &cur, &running(status, node));
    let drift = diffs.iter().filter(|d| d.kind == DRIFT).count();
    let mut b = format!("{{\"version\":{},\"etag\":", cur.number);
    write_string(&mut b, &cur.etag);
    b.push_str(",\"checked_at\":");
    let mut t = Vec::new();
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    akit_observe::time::rfc3339_nano(
        &mut t,
        i64::try_from(now.as_secs()).unwrap_or(i64::MAX),
        now.subsec_nanos(),
        0,
    );
    write_string(&mut b, &String::from_utf8_lossy(&t));
    b.push_str(",\"differences\":[");
    for (i, d) in diffs.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        d.write(&mut b);
    }
    let _ = write!(
        b,
        "],\"summary\":{{\"drift\":{drift},\"info\":{}}}}}",
        diffs.len().saturating_sub(drift)
    );
    b.push('\n');
    json(200, b)
        .header("ETag", &cur.etag)
        .header(VERSION_HEADER, cur.number.to_string())
}

/// The fixes a reconcile names, checked against the differences there are now: the patches
/// to apply, or why not. fit's `Configs.reconcile`'s first half.
pub(crate) fn fixes(
    configs: &Configs,
    status: Option<&Live>,
    node: Option<&NodeView>,
    body: &Value,
) -> Result<Vec<Patch>, String> {
    let named = arr(body.get("fixes"));
    if named.is_empty() {
        return Err("name at least one fix".to_owned());
    }
    let cur = configs.current();
    let diffs = differences(configs, &cur, &running(status, node));
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for f in named {
        let (id, action) = (str_of(f, "id"), str_of(f, "action"));
        if !seen.insert(id.to_owned()) {
            return Err(format!("the difference {id:?} is named more than once"));
        }
        let Some(d) = diffs.iter().find(|d| d.id == id) else {
            return Err(format!(
                "the difference {id:?} is not there any more: check again"
            ));
        };
        let Some(fix) = d.fixes.iter().find(|x| x.action == action) else {
            return Err(format!("the difference {id:?} offers no {action:?} fix"));
        };
        if let Some(p) = &fix.patch {
            out.push(p.clone());
        }
    }
    Ok(out)
}

/// The base config `cur` with `patches` applied: the adds and replaces in their order, then the
/// deletes, from the end of their arrays first. fit's `Configs.reconcile`'s second half.
pub(crate) fn reconciled(cur: &Version, patches: &[Patch]) -> Result<String, Refused> {
    if patches.is_empty() {
        return Ok(cur.data().to_owned());
    }
    let bad = |msg: String| Refused::Request { status: 400, msg };
    let mut root = akit_config::value::parse(cur.data()).map_err(|_| Refused::Request {
        status: 500,
        msg: "the running config is not JSON".to_owned(),
    })?;
    let mut deletes: Vec<(Vec<String>, &Patch)> = Vec::new();
    for p in patches {
        let tokens = pointer::parse(&p.path).map_err(|e| bad(e.msg))?;
        if p.op == "delete" {
            deletes.push((tokens, p));
            continue;
        }
        let Some(v) = p.value.clone() else {
            return Err(bad(format!("the fix for {} sets nothing", p.path)));
        };
        if tokens.last().is_some_and(|t| t == "-") {
            let parent = tokens.get(..tokens.len().saturating_sub(1)).unwrap_or(&[]);
            if pointer::lookup(&root, parent).is_err() {
                pointer::put(&mut root, parent, Value::Array(Vec::new()))
                    .map_err(|e| bad(e.msg))?;
            }
        }
        pointer::put(&mut root, &tokens, v).map_err(|e| bad(e.msg))?;
    }
    // Later elements first, so that an index removed does not move the next one named.
    deletes.sort_by(|(a, _), (b, _)| {
        for (x, y) in a.iter().zip(b.iter()) {
            if x == y {
                continue;
            }
            return match (x.parse::<usize>(), y.parse::<usize>()) {
                (Ok(i), Ok(j)) => j.cmp(&i),
                _ => x.cmp(y),
            };
        }
        b.len().cmp(&a.len())
    });
    for (tokens, _) in &deletes {
        pointer::remove(&mut root, tokens).map_err(|e| bad(e.msg))?;
    }
    Ok(root.to_json())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    fn v(s: &str) -> Value {
        akit_config::value::parse(s).unwrap()
    }

    fn cfg(s: &str) -> akit_config::Config {
        akit_config::from_json(s, None).unwrap()
    }

    #[test]
    fn certificates_covered() {
        assert!(covers("a.example", "A.example."));
        assert!(covers("*.example", "a.example"));
        assert!(!covers("*.example", "a.b.example"));
        assert!(!covers("*.example", "example"));
    }

    #[test]
    fn settings_that_take_a_restart() {
        let a = cfg(r#"{"limits": {"max_conns": 10}, "engine": {}}"#);
        let b = cfg(r#"{"limits": {"max_conns": 20}, "log": {"level": "debug"}}"#);
        assert_eq!(settings(&a, &b), ["limits.max_conns"]);
        let c = cfg(r#"{"limits": {"max_conns": 10}, "log": {"format": "json"}}"#);
        assert_eq!(settings(&a, &c), ["log"]);
    }

    #[test]
    fn listeners_upstreams_and_tls() {
        let base = v(r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"},
            {"name": "gone", "address": "127.0.0.1:1"},
            {"name": "s", "address": "127.0.0.1:0", "tls": {"acme": ["a.example"]}}],
            "upstreams": [{"name": "u", "addresses": ["127.0.0.1:2"]}, {"name": "x", "addresses": ["127.0.0.1:3"]}],
            "routes": [{"handler": {"module": "respond"}}]}"#);
        let c = cfg(&base.to_json());
        let status = v(
            r#"{"listeners": [{"name": "web", "address": "127.0.0.1:4000"},
            {"name": "s", "address": "127.0.0.1:4001"}, {"name": "old", "address": "127.0.0.1:9"}],
            "upstreams": [{"name": "u", "backends": [{"address": "127.0.0.1:2", "up": false}]},
            {"name": "y", "backends": []}],
            "certificates": [{"listener": "s", "names": ["b.example"]}]}"#,
        );
        let mut d = Vec::new();
        check_listeners(&c, &base, &status, &mut d);
        check_tls(&c, &base, &status, &mut d);
        check_upstreams(&c, &base, &status, &mut d);
        let ids: Vec<(&str, &str)> = d.iter().map(|x| (x.id.as_str(), x.kind)).collect();
        for want in [
            ("listener:gone:missing", DRIFT),
            ("listener:web:port", INFO),
            ("listener:old:extra", DRIFT),
            ("tls:s:a.example", DRIFT),
            ("tls:s:served:b.example", INFO),
            ("upstream:x:missing", DRIFT),
            ("upstream:y:extra", DRIFT),
            ("upstream:u:down", INFO),
        ] {
            assert!(ids.contains(&want), "{want:?} in {ids:?}");
        }
        let gone = d.iter().find(|x| x.id == "listener:gone:missing").unwrap();
        assert_eq!(gone.fixes[0].patch.as_ref().unwrap().path, "/listeners/1");
        let tls = d.iter().find(|x| x.id == "tls:s:a.example").unwrap();
        assert_eq!(
            tls.fixes[0].patch.as_ref().unwrap().path,
            "/listeners/2/tls/acme/0"
        );
    }

    #[test]
    fn deletes_from_the_end() {
        let cs = Configs::new(
            r#"{"upstreams": [{"name": "a"}, {"name": "b"}, {"name": "c"}]}"#.to_owned(),
            "file",
            4,
        );
        let cur = cs.current();
        let del = |p: &str| Patch {
            op: "delete",
            path: p.to_owned(),
            value: None,
        };
        let out = reconciled(
            &cur,
            &[
                del("/upstreams/0"),
                del("/upstreams/2"),
                Patch {
                    op: "add",
                    path: "/routes/-".to_owned(),
                    value: Some(v(r#"{"name": "r"}"#)),
                },
            ],
        )
        .unwrap();
        assert_eq!(
            v(&out),
            v(r#"{"upstreams": [{"name": "b"}], "routes": [{"name": "r"}]}"#)
        );
    }
}
