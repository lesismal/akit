//! Each member's config against its templates (I136, fit's `cluster/render.go` and `sync.go`):
//! the parts its templates render, with its variables; its desired config; each part's sync state
//! (in sync, out of sync, or a conflict where the member changed an owned part since the manager
//! last saw it in sync); and the record of what the manager owns there, which the member keeps
//! (`PUT /node/managed`) so that its own pages mark those fields. Members are read on akio's
//! asynchronous client, `max_concurrency` at once, each answer starting the next read.
//!
//! An akit member answers its config with its secrets redacted (decision 035): a part is compared,
//! and its hash kept, as the member answers it, secrets redacted on both sides, so that it reads in
//! sync; a fit member's are compared whole, as fit's manager compares them.

// Callbacks carry a member's answer or its problem, made once per member: their types and sizes
// are what they are.
#![allow(clippy::type_complexity, clippy::result_large_err)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use akio_http::{Context, Method, Response};
use akit_config::value::Value;
use akit_roles::pages::{fail, json as answer};

use crate::json::{self, Obj};
use crate::jv;
use crate::member::{FieldError, MAX_BODY_BYTES};
use crate::templates::{self, SECRET_MASK, Template};
use crate::transport::{KIND_BAD_ANSWER, KIND_UNREACHABLE, Problem};
use crate::{MAX_READS, Manager, Refusal};

/// How many syncs the pages may run at once; one more is answered 429.
pub const MAX_SYNCS: usize = 4;
/// How long a page waits for its members' sync states; those not read by then are pending.
pub const SYNC_WAIT: Duration = Duration::from_secs(30);
/// The capability of a member's `GET /node` for `/node/managed`.
pub const CAP_MANAGED: &str = "node.managed";
/// What akit's API answers a secret as (decision 035).
const REDACTED: &str = "[redacted]";

/// A part a member's templates render, from the template that wins it.
#[derive(Debug, Clone)]
pub(crate) struct Rendered {
    pub(crate) member: String,
    pub(crate) name: String,
    pub(crate) template: String,
    pub(crate) overrides: Vec<String>,
    pub(crate) value: Value,
}

impl Rendered {
    pub(crate) fn display(&self) -> String {
        jv::display(&self.member, &self.name)
    }
}

/// What a member's parts render from: its own variables, the groups it is in with their
/// variables, and the templates. fit's `renderInput`.
#[derive(Debug, Clone, Default)]
struct Input {
    name: String,
    id: String,
    hostname: String,
    labels: BTreeMap<String, String>,
    groups: BTreeMap<String, BTreeMap<String, String>>,
    templates: Vec<Template>,
}

impl Input {
    /// The templates attached to a group of the member's, by order then name.
    fn attached(&self) -> Vec<&Template> {
        let mut out: Vec<&Template> = self
            .templates
            .iter()
            .filter(|t| t.groups.iter().any(|g| self.groups.contains_key(g)))
            .collect();
        out.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.name.cmp(&b.name)));
        out
    }

    /// The parts the member's templates render, the templates that do, and what failed: fit's
    /// `render`.
    fn render(&self) -> (Vec<Rendered>, Vec<String>, Vec<String>) {
        let mut parts: Vec<Rendered> = Vec::new();
        let (mut names, mut errs) = (Vec::new(), Vec::new());
        for t in self.attached() {
            names.push(t.name.clone());
            let mut vars: BTreeMap<String, String> = BTreeMap::new();
            for g in &t.groups {
                if let Some(vs) = self.groups.get(g) {
                    vars.extend(vs.iter().map(|(k, v)| (k.clone(), v.clone())));
                }
            }
            let lookup =
                |name: &str| -> Result<String, String> {
                    match name {
                    "node.name" => Ok(self.name.clone()),
                    "node.id" | "node.hostname" => {
                        let v = if name == "node.id" { &self.id } else { &self.hostname };
                        if v.is_empty() {
                            Err(format!("${{{name}}}: the member was not read yet"))
                        } else {
                            Ok(v.clone())
                        }
                    }
                    n if n.starts_with("node.labels.") => {
                        let k = n.trim_start_matches("node.labels.");
                        self.labels.get(k).cloned().ok_or_else(|| {
                            format!("${{{n}}}: the member has no label {k:?}")
                        })
                    }
                    n => vars.get(n).cloned().ok_or_else(|| {
                        format!(
                            "${{{n}}}: no group of the member's that the template is attached to \
                             has the variable {n:?}"
                        )
                    }),
                }
                };
            let tparts = match templates::read_config(&t.config, &mut |_| {}) {
                Ok(p) => p,
                Err(e) => {
                    errs.push(format!("the template {:?}: {e}", t.name));
                    continue;
                }
            };
            let mut out = Vec::new();
            let mut failed = None;
            for p in tparts {
                let mut v = p.value.clone();
                if let Err(e) = jv::walk_strings(&mut v, &mut Vec::new(), &mut |_, s| {
                    templates::expand(s, Some(&lookup), &mut |_| {})
                }) {
                    failed = Some(e);
                    break;
                }
                out.push(Rendered {
                    member: p.member,
                    name: p.name,
                    template: t.name.clone(),
                    overrides: Vec::new(),
                    value: v,
                });
            }
            if let Some(e) = failed {
                errs.push(format!("the template {:?}: {e}", t.name));
                continue;
            }
            for mut r in out {
                if let Some(old) = parts.iter_mut().find(|o| o.display() == r.display()) {
                    let mut ov = old.overrides.clone();
                    ov.push(old.template.clone());
                    r.overrides = ov;
                    *old = r;
                    continue;
                }
                parts.push(r);
            }
        }
        (parts, names, errs)
    }
}

/// `cfg` with the parts taken from the templates: fit's `desired`. A new element goes at the end
/// of its list.
pub(crate) fn desired(cfg: &Value, parts: &[Rendered]) -> Value {
    let mut root = cfg.clone();
    for p in parts {
        if p.name.is_empty() {
            jv::set(&mut root, &p.member, p.value.clone());
            continue;
        }
        if !matches!(root.get(&p.member), Some(Value::Array(_))) {
            jv::set(&mut root, &p.member, Value::Array(Vec::new()));
        }
        if let Value::Object(ms) = &mut root {
            if let Some((_, Value::Array(items))) = ms.iter_mut().find(|(k, _)| k == &p.member) {
                match items
                    .iter_mut()
                    .find(|it| matches!(it, Value::Object(_)) && jv::name_of(it) == p.name)
                {
                    Some(slot) => *slot = p.value.clone(),
                    None => items.push(p.value.clone()),
                }
            }
        }
    }
    root
}

/// A part's sync state, fit's `PartSync`.
#[derive(Debug, Clone)]
pub(crate) struct PartSync {
    pub(crate) path: String,
    pub(crate) member: String,
    pub(crate) name: String,
    pub(crate) template: String,
    pub(crate) overrides: Vec<String>,
    pub(crate) state: &'static str,
    pub(crate) reason: &'static str,
    pub(crate) have: Option<String>,
    pub(crate) want: Option<String>,
}

/// A member's sync state, fit's `MemberSync`.
#[derive(Debug, Clone)]
pub(crate) struct MemberSync {
    pub(crate) node: String,
    pub(crate) state: &'static str,
    pub(crate) error: Option<Problem>,
    pub(crate) errors: Vec<String>,
    pub(crate) checked: SystemTime,
    pub(crate) config_etag: String,
    pub(crate) templates: Vec<String>,
    pub(crate) parts: Vec<PartSync>,
    pub(crate) counts: [usize; 3],
    pub(crate) note: String,
    pub(crate) released: Vec<String>,
    pub(crate) managed: String,
    pub(crate) managed_error: String,
}

impl MemberSync {
    pub(crate) fn failed(node: &str, p: Problem) -> MemberSync {
        MemberSync {
            node: node.to_owned(),
            state: "error",
            error: Some(p),
            errors: Vec::new(),
            checked: SystemTime::now(),
            config_etag: String::new(),
            templates: Vec::new(),
            parts: Vec::new(),
            counts: [0; 3],
            note: String::new(),
            released: Vec::new(),
            managed: String::new(),
            managed_error: String::new(),
        }
    }

    /// As the API answers it; without the parts' values for a summary.
    fn json(&self, values: bool) -> String {
        let mut o = Obj::new().str("node", &self.node).str("state", self.state);
        if let Some(e) = &self.error {
            o = o.raw("error", &e.json());
        }
        if !self.errors.is_empty() {
            o = o.raw("errors", &json::string_list(&self.errors));
        }
        o = o
            .raw("checked", &json::time(self.checked))
            .str_omit("config_etag", &self.config_etag)
            .raw("templates", &json::string_list(&self.templates));
        let parts = self
            .parts
            .iter()
            .map(|p| {
                let mut x = Obj::new()
                    .str("path", &p.path)
                    .str("member", &p.member)
                    .str_omit("name", &p.name)
                    .str("template", &p.template);
                if !p.overrides.is_empty() {
                    x = x.raw("overrides", &json::string_list(&p.overrides));
                }
                x = x.str("state", p.state).str_omit("reason", p.reason);
                if values {
                    x = x
                        .opt("have", p.have.as_deref())
                        .opt("want", p.want.as_deref());
                }
                x.end()
            })
            .collect::<Vec<_>>()
            .join(",");
        o = o.raw("parts", &format!("[{parts}]")).raw(
            "counts",
            &Obj::new()
                .num("in_sync", self.counts[0])
                .num("out_of_sync", self.counts[1])
                .num("conflict", self.counts[2])
                .end(),
        );
        o = o.str_omit("note", &self.note);
        if !self.released.is_empty() {
            o = o.raw("released", &json::string_list(&self.released));
        }
        o.str_omit("managed", &self.managed)
            .str_omit("managed_error", &self.managed_error)
            .end()
    }
}

/// What the manager owns on a member, as the member keeps it: fit's `managed.Record`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Record {
    manager: String,
    manager_host: String,
    paths: Vec<(String, String, String, String)>,
}

impl Record {
    fn from_value(v: &Value) -> Record {
        let mut r = Record {
            manager: json::str_of(v, "manager").to_owned(),
            manager_host: json::str_of(v, "manager_host").to_owned(),
            paths: Vec::new(),
        };
        if let Some(Value::Array(ps)) = v.get("paths") {
            for p in ps {
                r.paths.push((
                    json::str_of(p, "member").to_owned(),
                    json::str_of(p, "name").to_owned(),
                    json::str_of(p, "template").to_owned(),
                    json::str_of(p, "hash").to_owned(),
                ));
            }
        }
        r.paths.sort();
        r
    }

    fn json(&self, updated: SystemTime) -> String {
        let paths = self
            .paths
            .iter()
            .map(|(m, n, t, h)| {
                Obj::new()
                    .str("member", m)
                    .str_omit("name", n)
                    .str_omit("template", t)
                    .str_omit("hash", h)
                    .end()
            })
            .collect::<Vec<_>>()
            .join(",");
        Obj::new()
            .str("manager", &self.manager)
            .str_omit("manager_host", &self.manager_host)
            .raw("updated", &json::time(updated))
            .raw("paths", &format!("[{paths}]"))
            .end()
    }
}

/// A member's config compared with what its templates render: fit's `compare`. `redacts` is a
/// member that answers its secrets redacted, an akit.
fn compare(
    input: &Input,
    cfg: &Value,
    old: &Record,
    me: &Record,
    redacts: bool,
) -> (MemberSync, Record, Value) {
    let (parts, names, errs) = input.render();
    let mut sync = MemberSync::failed(&input.name, Problem::new(KIND_UNREACHABLE, ""));
    sync.error = None;
    sync.templates = names;
    sync.errors = errs;
    let mut rec = Record {
        manager: me.manager.clone(),
        manager_host: me.manager_host.clone(),
        paths: Vec::new(),
    };
    let mut base: BTreeMap<String, String> = old
        .paths
        .iter()
        .map(|(m, n, _, h)| (jv::display(m, n), h.clone()))
        .collect();
    let seen = |v: &Value| {
        if redacts {
            jv::hash(&templates::masked(v, REDACTED))
        } else {
            jv::hash(v)
        }
    };
    for p in &parts {
        let k = p.display();
        let have = jv::lookup(cfg, &p.member, &p.name);
        let h_want = seen(&p.value);
        let h_have = have.map(&seen);
        let mut b = base.remove(&k).unwrap_or_default();
        let (state, reason) = match &h_have {
            Some(h) if *h == h_want => {
                b.clone_from(&h_want);
                ("in_sync", "")
            }
            None if b.is_empty() => ("out_of_sync", "missing"),
            None => ("conflict", "removed_here"),
            Some(h) if !b.is_empty() && *h == b => ("out_of_sync", "template_changed"),
            Some(_) if b.is_empty() => ("conflict", "own_value"),
            Some(_) => ("conflict", "changed_here"),
        };
        let i = match state {
            "in_sync" => 0,
            "out_of_sync" => 1,
            _ => 2,
        };
        if let Some(c) = sync.counts.get_mut(i) {
            *c = c.saturating_add(1);
        }
        sync.parts.push(PartSync {
            path: k,
            member: p.member.clone(),
            name: p.name.clone(),
            template: p.template.clone(),
            overrides: p.overrides.clone(),
            state,
            reason,
            have: have.map(|v| json::compact(&templates::masked(v, SECRET_MASK))),
            want: Some(json::compact(&templates::masked(&p.value, SECRET_MASK))),
        });
        rec.paths
            .push((p.member.clone(), p.name.clone(), p.template.clone(), b));
    }
    sync.released = base.into_keys().collect();
    rec.paths.sort();
    let want = desired(cfg, &parts);
    sync.state = if !sync.errors.is_empty() {
        "error"
    } else if sync.templates.is_empty() {
        "unmanaged"
    } else if sync.counts[2] > 0 {
        "conflict"
    } else if sync.counts[1] > 0 {
        "out_of_sync"
    } else {
        "in_sync"
    };
    (sync, rec, want)
}

/// The syncs under way and the states the pages read.
#[derive(Default)]
pub(crate) struct Tpl {
    /// Each member's last sync state, a summary without values, as JSON.
    pub(crate) last: BTreeMap<String, String>,
    /// The members the manager told it owns something there.
    owned: BTreeSet<String>,
    syncs: usize,
    refreshing: bool,
    dirty: bool,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A member read and compared.
pub(crate) struct Out {
    pub(crate) sync: MemberSync,
    pub(crate) config: Option<Value>,
    pub(crate) desired: Option<Value>,
}

type OutDone = Box<dyn FnOnce(Out) + Send>;

/// Reads member `name`'s config, its `GET /node`, and what it keeps of what the manager owns
/// there: the config and its ETag, or why not.
pub(crate) fn fetch_config(
    m: &Manager,
    name: &str,
    done: Box<dyn FnOnce(Result<(Value, String), Problem>) + Send>,
) {
    m.call_member(
        name,
        "GET",
        "/config",
        Vec::new(),
        None,
        Box::new(move |r| {
            done(r.and_then(|rep| {
                match json::parse(&rep.body) {
                    Some(v @ Value::Object(_)) => {
                        Ok((v, rep.header("ETag").unwrap_or("").to_owned()))
                    }
                    _ => Err(Problem::new(
                        KIND_BAD_ANSWER,
                        "the member's /config is not a JSON object",
                    )
                    .with_status(rep.status)),
                }
            }));
        }),
    );
}

/// A read's failure, fit's `readFailure`: 502 with the problem.
pub(crate) fn read_failure(p: &Problem) -> Response {
    answer(
        502,
        Obj::new()
            .str("error", &p.message)
            .raw("problem", &p.json())
            .end(),
    )
}

impl Manager {
    /// The manager in a record it gives its members: its node ID and host.
    fn self_record(&self) -> Record {
        Record {
            manager: self.0.o.node_id.clone(),
            manager_host: akit_config::roles::host_name(),
            paths: Vec::new(),
        }
    }

    /// What member `name`'s parts render from, as the document has it now.
    fn input_of(&self, name: &str) -> Option<Input> {
        let d = self.doc();
        let mem = d.members.iter().find(|m| m.name == name)?;
        let mut input = Input {
            name: mem.name.clone(),
            labels: mem.labels.clone(),
            templates: d.templates.clone(),
            ..Input::default()
        };
        for g in &d.groups {
            if g.check(usize::MAX).is_ok() && g.has(&mem.name, &mem.labels) {
                input.groups.insert(g.name.clone(), g.variables.clone());
            }
        }
        Some(input)
    }

    /// Reads member `name` and compares it with its templates; with `push`, tells it what the
    /// manager owns there. fit's `syncMember`.
    pub(crate) fn sync_member(&self, name: &str, push: bool, done: OutDone) {
        self.sync_draft(name, push, None, done);
    }

    /// [`sync_member`](Manager::sync_member), with template `draft.0`'s config `draft.1` in
    /// place of the one kept: a rollout's draft (fit's `draftMember`), whose sync state is
    /// neither kept nor told.
    pub(crate) fn sync_draft(
        &self,
        name: &str,
        push: bool,
        draft: Option<(String, Value)>,
        done: OutDone,
    ) {
        let (me, name2) = (self.clone(), name.to_owned());
        let drafting = draft.is_some();
        let fail = move |me: &Manager, name: &str, p: Problem, done: OutDone| {
            let out = Out {
                sync: MemberSync::failed(name, p),
                config: None,
                desired: None,
            };
            if !drafting {
                me.keep_sync(&out, false, false);
            }
            done(out);
        };
        self.call_member(
            name,
            "GET",
            "/node",
            Vec::new(),
            None,
            Box::new(move |r| {
                let info = match r.map(|rep| json::parse(&rep.body).unwrap_or(Value::Null)) {
                    Ok(v) => v,
                    Err(p) => return fail(&me, &name2, p, done),
                };
                let me2 = me.clone();
                fetch_config(
                    &me,
                    &name2.clone(),
                    Box::new(move |r| {
                        let (cfg, etag) = match r {
                            Ok(x) => x,
                            Err(p) => return fail(&me2, &name2, p, done),
                        };
                        let caps = json::strings_of(&info, "capabilities");
                        if !caps.iter().any(|c| c == CAP_MANAGED) {
                            return me2.finish_sync(
                                &name2, &info, &cfg, &etag, None, "", push, draft, done,
                            );
                        }
                        let me3 = me2.clone();
                        me2.call_member(
                            &name2.clone(),
                            "GET",
                            "/node/managed",
                            Vec::new(),
                            None,
                            Box::new(move |r| match r {
                                Err(p) => fail(&me3, &name2, p, done),
                                Ok(rep) => {
                                    let rec = Record::from_value(
                                        &json::parse(&rep.body).unwrap_or(Value::Null),
                                    );
                                    let tag = rep.header("ETag").unwrap_or("").to_owned();
                                    me3.finish_sync(
                                        &name2,
                                        &info,
                                        &cfg,
                                        &etag,
                                        Some(rec),
                                        &tag,
                                        push,
                                        draft,
                                        done,
                                    );
                                }
                            }),
                        );
                    }),
                );
            }),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_sync(
        &self,
        name: &str,
        info: &Value,
        cfg: &Value,
        etag: &str,
        old: Option<Record>,
        old_tag: &str,
        push: bool,
        draft: Option<(String, Value)>,
        done: OutDone,
    ) {
        let Some(mut input) = self.input_of(name) else {
            let p = Problem::new(KIND_UNREACHABLE, format!("no member is named {name:?}"));
            return done(Out {
                sync: MemberSync::failed(name, p),
                config: None,
                desired: None,
            });
        };
        input.id = json::str_of(info, "id").to_owned();
        input.hostname = json::str_of(info, "hostname").to_owned();
        let drafting = draft.is_some();
        if let Some((tpl, cfg)) = draft {
            for t in &mut input.templates {
                if t.name == tpl {
                    t.config.clone_from(&cfg);
                }
            }
        }
        let redacts = info.get("akit_version").is_some();
        let base = old.clone().unwrap_or_default();
        let (mut sync, rec, want) = compare(&input, cfg, &base, &self.self_record(), redacts);
        sync.config_etag = etag.to_owned();
        if let Some((kind, why)) = self.compatible(json::str_of(info, "schema_digest")) {
            if kind == crate::pass::READ_ONLY_VERSION {
                sync.note = why;
            }
        }
        let owns = !rec.paths.is_empty();
        let out = |sync: MemberSync| Out {
            sync,
            config: Some(cfg.clone()),
            desired: Some(want.clone()),
        };
        if drafting {
            return done(out(sync));
        }
        match old {
            None => {
                if owns {
                    sync.managed = "unsupported".to_owned();
                }
                let o = out(sync);
                self.keep_sync(&o, push, owns);
                done(o);
            }
            Some(_) if !push => {
                let o = out(sync);
                self.keep_sync(&o, false, owns);
                done(o);
            }
            Some(o)
                if (o.paths.is_empty() && !owns)
                    || (o.manager == rec.manager
                        && o.manager_host == rec.manager_host
                        && o.paths == rec.paths) =>
            {
                sync.managed = "kept".to_owned();
                let o = out(sync);
                self.keep_sync(&o, true, owns);
                done(o);
            }
            Some(_) => {
                let me = self.clone();
                let o = out(sync);
                self.tell(
                    name,
                    rec,
                    old_tag.to_owned(),
                    true,
                    Box::new(move |state, why| {
                        let mut o = o;
                        o.sync.managed = state.to_owned();
                        o.sync.managed_error = why;
                        me.keep_sync(&o, state == "told" || state == "kept", owns);
                        done(o);
                    }),
                );
            }
        }
    }

    /// Tells member `name` what the manager owns there, `If-Match` its record's ETag, once more
    /// with the new ETag when it changed meanwhile: fit's `tell`.
    fn tell(
        &self,
        name: &str,
        rec: Record,
        tag: String,
        again: bool,
        done: Box<dyn FnOnce(&'static str, String) + Send>,
    ) {
        let body = rec.json(SystemTime::now()).into_bytes();
        let headers = if tag.is_empty() {
            Vec::new()
        } else {
            vec![("If-Match".to_owned(), tag)]
        };
        let (me, name2) = (self.clone(), name.to_owned());
        self.call_member(
            name,
            "PUT",
            "/node/managed",
            headers,
            Some(body),
            Box::new(move |r| match r {
                Ok(_) => {
                    me.log(format!(
                        "cluster: told the member {name2:?} it is managed in {} parts",
                        rec.paths.len()
                    ));
                    done("told", String::new());
                }
                Err(p) if p.status != 412 || !again => done("failed", p.message),
                Err(_) => {
                    let me2 = me.clone();
                    me.call_member(
                        &name2.clone(),
                        "GET",
                        "/node/managed",
                        Vec::new(),
                        None,
                        Box::new(move |r| match r {
                            Err(p) => done("failed", p.message),
                            Ok(rep) => {
                                let now = Record::from_value(
                                    &json::parse(&rep.body).unwrap_or(Value::Null),
                                );
                                if now.manager == rec.manager && now.paths == rec.paths {
                                    return done("kept", String::new());
                                }
                                let tag = rep.header("ETag").unwrap_or("").to_owned();
                                me2.tell(&name2, rec, tag, false, done);
                            }
                        }),
                    );
                }
            }),
        );
    }

    fn keep_sync(&self, o: &Out, told: bool, owns: bool) {
        let mut t = lock(&self.0.tpl);
        t.last.insert(o.sync.node.clone(), o.sync.json(false));
        if told {
            if owns {
                t.owned.insert(o.sync.node.clone());
            } else {
                t.owned.remove(&o.sync.node);
            }
        }
    }

    /// Syncs `names`, at most `max_concurrency` at once, each answer starting the next member;
    /// `each` hears each member, `then` once all are done. fit's `runSync`.
    pub(crate) fn run_sync(
        &self,
        names: Vec<String>,
        push: bool,
        each: Arc<dyn Fn(&Out) + Send + Sync>,
        then: Box<dyn FnOnce() + Send>,
    ) {
        if names.is_empty() {
            return then();
        }
        let total = names.len();
        let first = total.min(self.0.concurrency);
        let state = Arc::new(Mutex::new((first, total, Some(then))));
        let names = Arc::new(names);
        for i in 0..first {
            self.sync_next(
                Arc::clone(&names),
                i,
                push,
                Arc::clone(&each),
                Arc::clone(&state),
            );
        }
    }

    #[allow(clippy::type_complexity)]
    fn sync_next(
        &self,
        names: Arc<Vec<String>>,
        i: usize,
        push: bool,
        each: Arc<dyn Fn(&Out) + Send + Sync>,
        state: Arc<Mutex<(usize, usize, Option<Box<dyn FnOnce() + Send>>)>>,
    ) {
        let Some(name) = names.get(i).cloned() else {
            return;
        };
        let me = self.clone();
        self.sync_member(
            &name,
            push,
            Box::new(move |o| {
                each(&o);
                let (next, then) = {
                    let mut s = lock(&state);
                    s.1 = s.1.saturating_sub(1);
                    let next = (s.0 < names.len()).then_some(s.0);
                    s.0 = s.0.saturating_add(1);
                    let then = if s.1 == 0 { s.2.take() } else { None };
                    (next, then)
                };
                if let Some(j) = next {
                    if !me.0.closed.load(Ordering::Acquire) {
                        me.sync_next(names, j, push, each, state);
                        return;
                    }
                    // The manager stops: the rest are not read.
                    let mut s = lock(&state);
                    s.1 = 0;
                    if let Some(f) = s.2.take() {
                        drop(s);
                        f();
                    }
                    return;
                }
                if let Some(f) = then {
                    f();
                }
            }),
        );
    }

    /// Tells the members what the manager owns there after a change of the templates, the groups
    /// or the members: those with a template attached, and those told before, which may own
    /// nothing now. One refresh at a time; a change meanwhile runs another after it. fit's
    /// `kickManaged`, without its delay (rule: no timer that is not about time).
    pub(crate) fn kick_managed(&self) {
        {
            let mut t = lock(&self.0.tpl);
            t.dirty = true;
            if t.refreshing || self.0.closed.load(Ordering::Acquire) {
                return;
            }
            t.refreshing = true;
            t.dirty = false;
        }
        self.refresh_managed();
    }

    fn refresh_managed(&self) {
        let owned: BTreeSet<String> = lock(&self.0.tpl).owned.clone();
        let d = self.doc();
        let names: Vec<String> = d
            .members
            .iter()
            .filter(|m| {
                owned.contains(&m.name)
                    || self
                        .input_of(&m.name)
                        .is_some_and(|i| !i.attached().is_empty())
            })
            .filter(|m| self.node(&m.name).is_some())
            .map(|m| m.name.clone())
            .collect();
        let me = self.clone();
        self.run_sync(
            names,
            true,
            Arc::new(|_| {}),
            Box::new(move || {
                let again = {
                    let mut t = lock(&me.0.tpl);
                    if t.dirty && !me.0.closed.load(Ordering::Acquire) {
                        t.dirty = false;
                        true
                    } else {
                        t.refreshing = false;
                        false
                    }
                };
                if again {
                    me.refresh_managed();
                }
            }),
        );
    }
}

/// The parts of a member's config, as a template could own them: fit's `ConfigPart`.
pub(crate) fn parts_of(cfg: &Value) -> Vec<(String, String, String, bool, String, Value)> {
    let mut out = Vec::new();
    let Value::Object(ms) = cfg else { return out };
    for (k, v) in ms {
        match v {
            Value::Array(items) if jv::is_list(k) => {
                for (j, it) in items.iter().enumerate() {
                    let name = if matches!(it, Value::Object(_)) {
                        jv::name_of(it).to_owned()
                    } else {
                        String::new()
                    };
                    if name.is_empty() {
                        out.push((
                            format!("{k}[#{j}]"),
                            k.clone(),
                            String::new(),
                            false,
                            format!("it has no name: a template owns the elements of {k} by name"),
                            it.clone(),
                        ));
                    } else {
                        out.push((
                            jv::display(k, &name),
                            k.clone(),
                            name,
                            true,
                            String::new(),
                            it.clone(),
                        ));
                    }
                }
            }
            _ => {
                let why = jv::check_part(k, "").unwrap_or_default();
                out.push((
                    k.clone(),
                    k.clone(),
                    String::new(),
                    why.is_empty(),
                    why,
                    v.clone(),
                ));
            }
        }
    }
    out
}

fn part_json(p: &(String, String, String, bool, String, Value)) -> String {
    let masked = templates::masked(&p.5, SECRET_MASK);
    let secret = jv::hash(&masked) != jv::hash(&p.5) || json::compact(&p.5).contains(REDACTED);
    let mut o = Obj::new()
        .str("path", &p.0)
        .str("member", &p.1)
        .str_omit("name", &p.2)
        .bool("ownable", p.3)
        .str_omit("why", &p.4);
    if secret {
        o = o.bool("secret", true);
    }
    o.raw("value", &json::compact(&masked)).end()
}

/// The template config made of the parts `paths` names of `cfg`: fit's `fragmentOf`.
///
/// # Errors
///
/// A part not written right, or not in the config.
pub(crate) fn fragment_of(cfg: &Value, paths: &[String]) -> Result<Value, FieldError> {
    let field = |msg: String| FieldError {
        field: "from.paths".to_owned(),
        msg,
    };
    if paths.is_empty() {
        return Err(field(
            "pick the parts of the member's config the template is made of".to_owned(),
        ));
    }
    if paths.len() > templates::MAX_TEMPLATE_PARTS {
        return Err(field(format!(
            "{} parts, more than {}",
            paths.len(),
            templates::MAX_TEMPLATE_PARTS
        )));
    }
    let mut root = Value::Object(Vec::new());
    for s in paths {
        let s = s.trim();
        let (member, name) = match (s.find('['), s.strip_suffix(']')) {
            (Some(i), Some(_)) => (
                s.get(..i).unwrap_or(""),
                s.get(i.saturating_add(1)..s.len().saturating_sub(1))
                    .unwrap_or(""),
            ),
            _ => (s, ""),
        };
        if let Some(e) = jv::check_part(member, name) {
            return Err(field(format!("{s:?}: {e}")));
        }
        let Some(v) = jv::lookup(cfg, member, name) else {
            return Err(field(format!(
                "the member's config has no {}",
                jv::display(member, name)
            )));
        };
        if name.is_empty() {
            jv::set(&mut root, member, v.clone());
            continue;
        }
        if root.get(member).is_none() {
            jv::set(&mut root, member, Value::Array(Vec::new()));
        }
        if jv::lookup(&root, member, name).is_none() {
            if let Value::Object(ms) = &mut root {
                if let Some((_, Value::Array(items))) = ms.iter_mut().find(|(k, _)| k == member) {
                    items.push(v.clone());
                }
            }
        }
    }
    Ok(root)
}

fn decode(ctx: &mut Context) -> Option<Value> {
    if ctx.body().len() > MAX_BODY_BYTES {
        ctx.respond(fail(
            413,
            &format!("a request is {MAX_BODY_BYTES} bytes at most"),
        ));
        return None;
    }
    match akit_roles::pages::decode(ctx) {
        Ok(b) => Some(b.0),
        Err(r) => {
            ctx.respond(r);
            None
        }
    }
}

/// Takes one of the [`MAX_READS`] reads of members, answering 429 when none is free.
fn take_read(m: &Manager, ctx: &mut Context) -> bool {
    if m.0.reads.fetch_add(1, Ordering::AcqRel) >= MAX_READS {
        m.0.reads.fetch_sub(1, Ordering::AcqRel);
        ctx.respond(fail(429, "other reads of members are under way; try again"));
        return false;
    }
    true
}

/// Reads the configs of `names`, answering with what `answer_of` makes of them, or 502 with the
/// first problem.
fn read_configs(
    m: &Manager,
    ctx: &mut Context,
    names: Vec<String>,
    answer_of: Box<dyn FnOnce(&[(Value, String)]) -> String + Send>,
) {
    for n in &names {
        if m.node(n).is_none() {
            return ctx.respond(fail(
                404,
                &format!("no member is named {}", json::quoted(n)),
            ));
        }
    }
    if !take_read(m, ctx) {
        return;
    }
    let Some(d) = ctx.detach() else {
        m.0.reads.fetch_sub(1, Ordering::AcqRel);
        return;
    };
    #[allow(clippy::type_complexity)]
    let got: Arc<
        Mutex<(
            Vec<Option<Result<(Value, String), Problem>>>,
            Option<akio_http::Detached>,
            Option<Box<dyn FnOnce(&[(Value, String)]) -> String + Send>>,
        )>,
    > = Arc::new(Mutex::new((
        vec![None; names.len()],
        Some(d),
        Some(answer_of),
    )));
    for (i, n) in names.iter().enumerate() {
        let (got, me) = (Arc::clone(&got), m.clone());
        fetch_config(
            m,
            n,
            Box::new(move |r| {
                let mut g = lock(&got);
                if let Some(slot) = g.0.get_mut(i) {
                    *slot = Some(r);
                }
                if g.0.iter().any(Option::is_none) {
                    return;
                }
                me.0.reads.fetch_sub(1, Ordering::AcqRel);
                let (Some(d), Some(f)) = (g.1.take(), g.2.take()) else {
                    return;
                };
                let mut ok = Vec::new();
                for x in g.0.drain(..).flatten() {
                    match x {
                        Ok(c) => ok.push(c),
                        Err(p) => return d.respond(read_failure(&p)),
                    }
                }
                drop(g);
                d.respond(answer(200, f(&ok)));
            }),
        );
    }
}

/// The sync and template endpoints, when `path` is one:
///
/// ```text
/// GET  /api/sync            each member's last sync state, without values
/// POST /api/sync            {nodes} or {template}, or every member: read now, compared, and told
///                           what the manager owns there; those not read in 30 s pending
/// GET  /api/desired/<name>  a member's sync state with values, its config and desired config,
///                           secrets masked
/// POST /api/import          {node}: the parts of a member's config, for a template made of them
/// POST /api/compare         {a, b}: two members' configs, part by part
/// POST /api/text/render     ?format=conf|yaml|json: a template's config as text
/// POST /api/text/parse      ?format=...: the text as a template's config
/// ```
pub(crate) fn serve(m: &Manager, ctx: &mut Context, path: &str) -> bool {
    let method = ctx.method().clone();
    let get = matches!(method, Method::Get | Method::Head);
    match path {
        "/api/sync" if get => {
            let names: Vec<String> = m.members().into_iter().map(|x| x.name).collect();
            let t = lock(&m.0.tpl);
            let list = names
                .iter()
                .filter_map(|n| t.last.get(n).cloned())
                .collect::<Vec<_>>()
                .join(",");
            drop(t);
            ctx.respond(answer(200, format!("{{\"nodes\":[{list}]}}")));
        }
        "/api/sync" if method == Method::Post => sync_route(m, ctx),
        p if p.starts_with("/api/desired/") && get => {
            let name = crate::api::percent_decode(p.trim_start_matches("/api/desired/"));
            if m.node(&name).is_none() {
                ctx.respond(fail(
                    404,
                    &format!("no member is named {}", json::quoted(&name)),
                ));
                return true;
            }
            if !take_read(m, ctx) {
                return true;
            }
            let Some(d) = ctx.detach() else {
                m.0.reads.fetch_sub(1, Ordering::AcqRel);
                return true;
            };
            let me = m.clone();
            m.sync_member(
                &name,
                true,
                Box::new(move |o| {
                    me.0.reads.fetch_sub(1, Ordering::AcqRel);
                    let mut body = o.sync.json(true);
                    if let (Some(c), Some(w)) = (&o.config, &o.desired) {
                        body.pop();
                        body.push_str(&format!(
                            ",\"config\":{},\"desired\":{}}}",
                            json::compact(&templates::masked(c, SECRET_MASK)),
                            json::compact(&templates::masked(w, SECRET_MASK))
                        ));
                    }
                    d.respond(answer(200, body));
                }),
            );
        }
        "/api/import" if method == Method::Post => {
            let Some(v) = decode(ctx) else { return true };
            let node = json::str_of(&v, "node").to_owned();
            let n2 = node.clone();
            read_configs(
                m,
                ctx,
                vec![node],
                Box::new(move |got| {
                    let (cfg, etag) = got.first().cloned().unwrap_or((Value::Null, String::new()));
                    let parts = parts_of(&cfg)
                        .iter()
                        .map(part_json)
                        .collect::<Vec<_>>()
                        .join(",");
                    Obj::new()
                        .str("node", &n2)
                        .str("config_etag", &etag)
                        .raw("parts", &format!("[{parts}]"))
                        .end()
                }),
            );
        }
        "/api/compare" if method == Method::Post => {
            let Some(v) = decode(ctx) else { return true };
            let (a, b) = (
                json::str_of(&v, "a").to_owned(),
                json::str_of(&v, "b").to_owned(),
            );
            if a.is_empty() || b.is_empty() || a == b {
                ctx.respond(answer(
                    400,
                    FieldError {
                        field: "b".to_owned(),
                        msg: "compare two members: name both, each once".to_owned(),
                    }
                    .json(),
                ));
                return true;
            }
            let (a2, b2) = (a.clone(), b.clone());
            read_configs(
                m,
                ctx,
                vec![a, b],
                Box::new(move |got| {
                    let empty = (Value::Null, String::new());
                    let (ca, ea) = got.first().unwrap_or(&empty);
                    let (cb, eb) = got.get(1).unwrap_or(&empty);
                    let side = |n: &str, c: &Value, e: &str| {
                        Obj::new()
                            .str("node", n)
                            .str("config_etag", e)
                            .raw("config", &json::compact(&templates::masked(c, SECRET_MASK)))
                            .end()
                    };
                    let pb = parts_of(cb);
                    let mut used = vec![false; pb.len()];
                    let mut parts = Vec::new();
                    for p in parts_of(ca) {
                        let mut c = Obj::new().str("path", &p.0);
                        let av = json::compact(&templates::masked(&p.5, SECRET_MASK));
                        match pb.iter().position(|q| q.0 == p.0) {
                            None => c = c.str("state", "only_a").raw("a", &av),
                            Some(j) => {
                                if let Some(u) = used.get_mut(j) {
                                    *u = true;
                                }
                                let q = pb.get(j).map(|q| q.5.clone()).unwrap_or(Value::Null);
                                if jv::hash(&p.5) == jv::hash(&q) {
                                    c = c.str("state", "same").raw("a", &av);
                                } else {
                                    c = c.str("state", "differs").raw("a", &av).raw(
                                        "b",
                                        &json::compact(&templates::masked(&q, SECRET_MASK)),
                                    );
                                }
                            }
                        }
                        parts.push(c.end());
                    }
                    for (q, u) in pb.iter().zip(&used) {
                        if !u {
                            parts.push(
                                Obj::new()
                                    .str("path", &q.0)
                                    .str("state", "only_b")
                                    .raw("b", &json::compact(&templates::masked(&q.5, SECRET_MASK)))
                                    .end(),
                            );
                        }
                    }
                    Obj::new()
                        .raw("a", &side(&a2, ca, ea))
                        .raw("b", &side(&b2, cb, eb))
                        .raw("parts", &format!("[{}]", parts.join(",")))
                        .end()
                }),
            );
        }
        "/api/text/render" | "/api/text/parse" if method == Method::Post => {
            ctx.respond(text(ctx, path == "/api/text/render"));
        }
        "/api/sync" | "/api/import" | "/api/compare" | "/api/text/render" | "/api/text/parse" => {
            ctx.respond(fail(405, "the method is not allowed"));
        }
        _ => return false,
    }
    true
}

fn sync_route(m: &Manager, ctx: &mut Context) {
    let Some(v) = decode(ctx) else { return };
    let nodes = json::strings_of(&v, "nodes");
    let template = json::str_of(&v, "template").to_owned();
    let d = m.doc();
    let names: Vec<String> = if !nodes.is_empty() {
        if nodes.len() > m.max_nodes() {
            return ctx.respond(
                Refusal::Field(FieldError {
                    field: "nodes".to_owned(),
                    msg: format!("{} members, more than {}", nodes.len(), m.max_nodes()),
                })
                .response(),
            );
        }
        let mut out: Vec<String> = Vec::new();
        for n in nodes {
            if d.index(&n).is_none() {
                return ctx.respond(fail(
                    404,
                    &format!("no member is named {}", json::quoted(&n)),
                ));
            }
            if !out.contains(&n) {
                out.push(n);
            }
        }
        out
    } else if !template.is_empty() {
        match d.templates.iter().find(|t| t.name == template) {
            None => {
                return ctx.respond(fail(
                    404,
                    &format!("no template is named {}", json::quoted(&template)),
                ));
            }
            Some(t) => templates::nodes_of(&d, t).0,
        }
    } else {
        let mut all: Vec<String> = d.members.iter().map(|x| x.name.clone()).collect();
        all.sort();
        all
    };
    {
        let mut t = lock(&m.0.tpl);
        if t.syncs >= MAX_SYNCS {
            drop(t);
            return ctx.respond(fail(429, "other reads of members are under way; try again"));
        }
        t.syncs = t.syncs.saturating_add(1);
    }
    let Some(det) = ctx.detach() else {
        let mut t = lock(&m.0.tpl);
        t.syncs = t.syncs.saturating_sub(1);
        return;
    };
    #[allow(clippy::type_complexity)]
    let shared: Arc<Mutex<(BTreeMap<String, String>, Option<akio_http::Detached>)>> =
        Arc::new(Mutex::new((BTreeMap::new(), Some(det))));
    let names = Arc::new(names);
    let (me, names2) = (m.clone(), Arc::clone(&names));
    let finish = Arc::new(
        move |shared: &Arc<Mutex<(BTreeMap<String, String>, Option<akio_http::Detached>)>>| {
            let mut s = lock(shared);
            let Some(d) = s.1.take() else { return };
            {
                let mut t = lock(&me.0.tpl);
                t.syncs = t.syncs.saturating_sub(1);
            }
            let mut done = Vec::new();
            let mut pending = Vec::new();
            for n in names2.iter() {
                match s.0.get(n) {
                    Some(j) => done.push(j.clone()),
                    None => pending.push(n.clone()),
                }
            }
            drop(s);
            d.respond(answer(
                200,
                format!(
                    "{{\"nodes\":[{}],\"pending\":{}}}",
                    done.join(","),
                    json::string_list(&pending)
                ),
            ));
        },
    );
    let (s1, s2, f1, f2) = (
        Arc::clone(&shared),
        Arc::clone(&shared),
        Arc::clone(&finish),
        Arc::clone(&finish),
    );
    // Those not read by then are answered pending: a timeout.
    let _ = akio::timer::after_send(SYNC_WAIT, move || f1(&s1));
    let each_shared = Arc::clone(&shared);
    m.run_sync(
        names.to_vec(),
        true,
        Arc::new(move |o: &Out| {
            lock(&each_shared)
                .0
                .insert(o.sync.node.clone(), o.sync.json(true));
        }),
        Box::new(move || f2(&s2)),
    );
}

/// `POST /api/text/render` and `/api/text/parse`: a template's config as text, or the text as
/// one, converted without the config's checks. fit's `cluster/text.go`.
fn text(ctx: &Context, render: bool) -> Response {
    use akit_config::adapt::{Target, document};
    use akit_config::{Format, Source};
    let (format, target) = match ctx.req().query_param("format") {
        Some("conf") => (Format::Conf, Target::Conf),
        Some("yaml") => (Format::Yaml, Target::Yaml),
        Some("json") => (Format::Json, Target::Json),
        _ => return fail(400, "the format must be conf, yaml or json"),
    };
    if ctx.body().len() > MAX_BODY_BYTES {
        return fail(
            413,
            &format!("a template's text is {MAX_BODY_BYTES} bytes at most"),
        );
    }
    let Ok(text) = std::str::from_utf8(ctx.body()) else {
        return fail(400, "the body is not UTF-8");
    };
    if render {
        let Some(v @ Value::Object(_)) = json::parse(text.as_bytes()) else {
            return fail(
                400,
                "rendering the config: a template's config is a JSON object",
            );
        };
        let src = json::compact(&v);
        return match document(&[Source {
            name: None,
            format: Format::Json,
            src: &src,
        }])
        .map_err(|e| e.to_string())
        .and_then(|d| d.write(target).map_err(|e| e.to_string()))
        {
            Ok(out) => Response::with_body(200, "text/plain; charset=utf-8", out.into_bytes()),
            Err(e) => fail(400, &format!("rendering the config: {e}")),
        };
    }
    let src = if text.trim().is_empty() && format == Format::Json {
        "{}"
    } else {
        text
    };
    let parsed = document(&[Source {
        name: None,
        format,
        src,
    }])
    .map_err(|e| e.to_string())
    .and_then(|d| d.write(Target::Json).map_err(|e| e.to_string()));
    match parsed.map(|j| akit_config::value::parse(&j)) {
        Ok(Ok(v @ Value::Object(_))) => answer(200, v.to_json()),
        Ok(Ok(Value::Null)) => answer(200, "{}\n".to_owned()),
        Ok(_) => fail(
            400,
            "parsing the text: a template's config is an object of top-level members",
        ),
        Err(e) => fail(400, &format!("parsing the text: {e}")),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    fn v(s: &str) -> Value {
        json::parse(s.as_bytes()).unwrap()
    }

    fn input() -> Input {
        Input {
            name: "edge-1".into(),
            id: "id-1".into(),
            labels: [("region".to_owned(), "eu".to_owned())].into(),
            groups: [(
                "eu".to_owned(),
                [("domain".to_owned(), "eu.example".to_owned())].into(),
            )]
            .into(),
            templates: vec![
                Template::from_value(&v(
                    r#"{"name": "base", "groups": ["eu"], "order": 0, "config": {"log": {"level": "info"},
                    "routes": [{"name": "api", "match": {"host": ["${node.name}.${domain}"]}}]}}"#,
                )),
                Template::from_value(&v(
                    r#"{"name": "over", "groups": ["eu"], "order": 5, "config": {"log": {"level": "debug"}}}"#,
                )),
                Template::from_value(&v(
                    r#"{"name": "other", "groups": ["us"], "config": {"x": 1}}"#,
                )),
            ],
            ..Input::default()
        }
    }

    #[test]
    fn rendered_by_order_with_variables() {
        // fit's TestRender.
        let (parts, names, errs) = input().render();
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(names, ["base", "over"]);
        let log = parts.iter().find(|p| p.member == "log").unwrap();
        assert_eq!(
            (log.template.as_str(), log.overrides.clone()),
            ("over", vec!["base".to_owned()])
        );
        let api = parts.iter().find(|p| p.name == "api").unwrap();
        assert!(json::compact(&api.value).contains("edge-1.eu.example"));
        let mut bad = input();
        bad.templates[0].config = v(r#"{"log": {"x": "${missing}"}}"#);
        let (_, _, errs) = bad.render();
        assert!(errs[0].contains("no group"), "{errs:?}");
    }

    #[test]
    fn sync_states() {
        // fit's TestCompare: in sync, missing, template changed, own value, changed here, removed
        // here; the record and the desired config.
        let me = Record {
            manager: "m".into(),
            ..Record::default()
        };
        let (parts, _, _) = input().render();
        let api = parts.iter().find(|p| p.name == "api").unwrap();
        let log = parts.iter().find(|p| p.member == "log").unwrap();
        // Nothing there yet: missing, and the desired config adds both.
        let cfg = v(r#"{"routes": [{"name": "other"}]}"#);
        let (s, rec, want) = compare(&input(), &cfg, &Record::default(), &me, false);
        assert_eq!(s.state, "out_of_sync");
        assert!(s.parts.iter().all(|p| p.reason == "missing"));
        assert!(rec.paths.iter().all(|p| p.3.is_empty()));
        let w = json::compact(&want);
        assert!(
            w.contains(r#""level":"debug""#) && w.contains(r#"{"name":"other"}"#),
            "{w}"
        );
        // Applied: in sync, the hashes kept.
        let (s, rec, _) = compare(&input(), &want, &Record::default(), &me, false);
        assert_eq!(s.state, "in_sync");
        assert!(rec.paths.iter().all(|p| p.3.len() == 64));
        // A template changed: out of sync; the member changed a part: a conflict.
        let mut changed_tpl = input();
        changed_tpl.templates[1].config = v(r#"{"log": {"level": "warn"}}"#);
        let (s, _, _) = compare(&changed_tpl, &want, &rec, &me, false);
        assert_eq!(
            s.parts.iter().find(|p| p.member == "log").unwrap().reason,
            "template_changed"
        );
        let mut edited = want.clone();
        jv::set(&mut edited, "log", v(r#"{"level": "error"}"#));
        let (s, _, _) = compare(&input(), &edited, &rec, &me, false);
        assert_eq!(s.state, "conflict");
        assert_eq!(
            s.parts.iter().find(|p| p.member == "log").unwrap().reason,
            "changed_here"
        );
        let (s, _, _) = compare(&input(), &edited, &Record::default(), &me, false);
        assert_eq!(
            s.parts.iter().find(|p| p.member == "log").unwrap().reason,
            "own_value"
        );
        let removed = v(r#"{"routes": []}"#);
        let (s, _, _) = compare(&input(), &removed, &rec, &me, false);
        assert_eq!(
            s.parts.iter().find(|p| p.member == "log").unwrap().reason,
            "removed_here"
        );
        // A part no template owns now is released.
        let mut old = rec.clone();
        old.paths
            .push(("tls".into(), String::new(), "gone".into(), String::new()));
        let (s, _, _) = compare(&input(), &want, &old, &me, false);
        assert_eq!(s.released, ["tls"]);
        let _ = (api, log);
    }

    #[test]
    fn redacted_secrets_compare_in_sync() {
        let mut i = input();
        i.templates = vec![Template::from_value(&v(
            r#"{"name": "auth", "groups": ["eu"], "config": {"routes": [{"name": "a", "password": "s3cret"}]}}"#,
        ))];
        let cfg = v(r#"{"routes": [{"name": "a", "password": "[redacted]"}]}"#);
        let me = Record::default();
        assert_eq!(
            compare(&i, &cfg, &Record::default(), &me, true).0.state,
            "in_sync"
        );
        assert_ne!(
            compare(&i, &cfg, &Record::default(), &me, false).0.state,
            "in_sync"
        );
    }

    #[test]
    fn fragments() {
        let cfg = v(r#"{"log": {"level": "info"}, "routes": [{"name": "a"}, {"name": "b"}]}"#);
        let f = fragment_of(&cfg, &["log".into(), "routes[b]".into()]).unwrap();
        assert_eq!(
            json::compact(&f),
            r#"{"log":{"level":"info"},"routes":[{"name":"b"}]}"#
        );
        assert!(fragment_of(&cfg, &["routes[c]".into()]).is_err());
        assert!(fragment_of(&cfg, &["admin".into()]).is_err());
        assert!(fragment_of(&cfg, &[]).is_err());
        let parts = parts_of(&v(r#"{"routes": [{"x": 1}], "admin": {}}"#));
        assert_eq!(parts[0].0, "routes[#0]");
        assert!(!parts[0].3 && !parts[1].3);
    }
}
