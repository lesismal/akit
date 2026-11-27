//! Operations on many members at once (I135, fit's `cluster/ops.go`, its I144): reload, roll
//! back, check, purge and test, on a group, a selection, or members of a group; at most
//! `max_concurrency` members at once, each answered apart; 4 operations at once and one on a
//! member, 32 kept, each in the audit log once it ends with every member's outcome.
//!
//! An operation runs without a thread of its own: each member's answer starts the next member's
//! call, on akio's asynchronous client (rule 1).

// A member's problem is the answer itself, made once per member: its size does not matter.
#![allow(clippy::result_large_err)]

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Instant, SystemTime};

use akio_http::{Context, Method};
use akit_config::value::Value;
use akit_roles::pages::{fail, json as answer};

use crate::json::{self, Obj};
use crate::member::{ACCESS_CONSOLE, FieldError, MAX_BODY_BYTES, MAX_NAME_BYTES};
use crate::transport::{
    KIND_AUTH, KIND_BAD_ANSWER, KIND_FORBIDDEN, KIND_UNREACHABLE, KIND_UNSUPPORTED, Problem, Reply,
};
use crate::{Manager, Refusal};

/// The operations, in the order the pages offer them.
pub const OPS: &[&str] = &["reload", "rollback", "check", "purge", "test"];
/// How many operations run at once; one more is answered 429.
pub const MAX_RUNNING_OPS: usize = 4;
/// How many operations the manager keeps, the oldest finished dropped first.
pub const MAX_KEPT_OPS: usize = 32;
/// The most items of each list of a purge, and the longest item.
pub const MAX_PURGE_ITEMS: usize = 64;
/// See [`MAX_PURGE_ITEMS`].
pub const MAX_PURGE_ITEM_BYTES: usize = 2048;
/// The longest member's error in the audit log.
const MAX_AUDIT_ERROR: usize = 256;

/// An operation the member answered with an error.
pub const KIND_REFUSED: &str = "refused";
/// A rollback with no earlier config to roll back to.
pub const KIND_NO_EARLIER: &str = "no_earlier_config";

/// What a purge removes from each member's cache: fit's `PurgeParams`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Purge {
    /// The zone, or every zone when empty.
    pub zone: String,
    /// The absolute URLs requests named, the cache's keys.
    pub keys: Vec<String>,
    /// URLs that start with any of these.
    pub prefixes: Vec<String>,
    /// Responses tagged with any of these.
    pub tags: Vec<String>,
    /// Everything.
    pub all: bool,
}

fn field(f: &str, msg: impl Into<String>) -> FieldError {
    FieldError {
        field: f.to_owned(),
        msg: msg.into(),
    }
}

fn printable(s: &str) -> bool {
    s.chars().all(|c| c >= ' ' && c != '\u{7f}')
}

impl Purge {
    fn from_value(v: &Value) -> Purge {
        Purge {
            zone: json::str_of(v, "zone").to_owned(),
            keys: json::strings_of(v, "keys"),
            prefixes: json::strings_of(v, "prefixes"),
            tags: json::strings_of(v, "tags"),
            all: json::bool_of(v, "all"),
        }
    }

    /// Checks it, fit's `PurgeParams.check`.
    fn check(&self) -> Result<(), FieldError> {
        let none = || field("purge", "a purge names keys, prefixes, tags, or all");
        let max_zone = MAX_NAME_BYTES.saturating_mul(2);
        if self.zone.len() > max_zone || !printable(&self.zone) {
            return Err(field(
                "purge.zone",
                format!("the zone is a name of at most {max_zone} bytes"),
            ));
        }
        let mut n = 0usize;
        for (name, list) in [
            ("purge.keys", &self.keys),
            ("purge.prefixes", &self.prefixes),
            ("purge.tags", &self.tags),
        ] {
            if list.len() > MAX_PURGE_ITEMS {
                return Err(field(
                    name,
                    format!("{} items, more than {MAX_PURGE_ITEMS}", list.len()),
                ));
            }
            for s in list {
                if s.is_empty() || s.len() > MAX_PURGE_ITEM_BYTES || !printable(s) {
                    return Err(field(
                        name,
                        format!("an item is 1 to {MAX_PURGE_ITEM_BYTES} printable characters"),
                    ));
                }
                if name != "purge.tags" && !s.starts_with("http://") && !s.starts_with("https://") {
                    return Err(field(
                        name,
                        format!(
                            "{s:?} is not an absolute http or https URL, as the cache's keys are"
                        ),
                    ));
                }
            }
            n = n.saturating_add(list.len());
        }
        if n == 0 && !self.all {
            return Err(none());
        }
        if n > 0 && self.all {
            return Err(field(
                "purge.all",
                "all purges everything: name no keys, prefixes or tags with it",
            ));
        }
        Ok(())
    }

    fn json(&self) -> String {
        let mut o = Obj::new().str_omit("zone", &self.zone);
        for (k, l) in [
            ("keys", &self.keys),
            ("prefixes", &self.prefixes),
            ("tags", &self.tags),
        ] {
            if !l.is_empty() {
                o = o.raw(k, &json::string_list(l));
            }
        }
        if self.all {
            o = o.bool("all", true);
        }
        o.end()
    }

    fn describe(&self) -> String {
        let zone = if self.zone.is_empty() {
            "every zone".to_owned()
        } else {
            format!("zone {}", self.zone)
        };
        if self.all {
            return format!("all of {zone}");
        }
        let mut parts = Vec::new();
        for (k, l) in [
            ("keys", &self.keys),
            ("prefixes", &self.prefixes),
            ("tags", &self.tags),
        ] {
            if !l.is_empty() {
                parts.push(format!("{k} {}", l.join(" ")));
            }
        }
        crate::api::clip(&format!("{zone}: {}", parts.join("; ")), 512)
    }
}

/// How an operation went on one member: fit's `Outcome`.
#[derive(Debug, Clone)]
struct Outcome {
    node: String,
    status: &'static str,
    detail: String,
    error: Option<Problem>,
    /// What the member answered, fit's `OpResult` as JSON.
    result: Option<String>,
    /// The config versions before and after a reload or a rollback.
    before: u64,
    after: u64,
    started: Option<SystemTime>,
    ended: Option<SystemTime>,
    took_ms: u64,
}

impl Outcome {
    fn json(&self) -> String {
        let mut o = Obj::new()
            .str("node", &self.node)
            .str("status", self.status)
            .str_omit("detail", &self.detail);
        if let Some(e) = &self.error {
            o = o.raw("error", &e.json());
        }
        o = o.opt("result", self.result.as_deref());
        if let Some(t) = self.started {
            o = o.raw("started", &json::time(t));
        }
        if let Some(t) = self.ended {
            o = o.raw("ended", &json::time(t));
        }
        if self.took_ms > 0 {
            o = o.num("took_ms", self.took_ms);
        }
        o.end()
    }
}

/// An operation on many members: fit's `Op`.
#[derive(Debug, Clone)]
struct Op {
    id: String,
    op: String,
    group: String,
    user: String,
    purge: Option<Purge>,
    started: SystemTime,
    ended: Option<SystemTime>,
    finished: bool,
    outcomes: Vec<Outcome>,
}

impl Op {
    fn counts(&self) -> [usize; 4] {
        let mut c = [0usize; 4];
        for o in &self.outcomes {
            let i = match o.status {
                "pending" => 0,
                "running" => 1,
                "done" => 2,
                _ => 3,
            };
            if let Some(x) = c.get_mut(i) {
                *x = x.saturating_add(1);
            }
        }
        c
    }

    fn json(&self) -> String {
        let [p, r, d, f] = self.counts();
        let mut o = Obj::new()
            .str("id", &self.id)
            .str("op", &self.op)
            .str_omit("group", &self.group)
            .str("user", &self.user);
        if let Some(pg) = &self.purge {
            o = o.raw("purge", &pg.json());
        }
        o = o.raw("started", &json::time(self.started));
        if let Some(t) = self.ended {
            o = o.raw("ended", &json::time(t));
        }
        let outcomes = self
            .outcomes
            .iter()
            .map(Outcome::json)
            .collect::<Vec<_>>()
            .join(",");
        o.bool("finished", self.finished)
            .raw(
                "counts",
                &Obj::new()
                    .num("pending", p)
                    .num("running", r)
                    .num("done", d)
                    .num("failed", f)
                    .end(),
            )
            .raw("outcomes", &format!("[{outcomes}]"))
            .end()
    }

    /// Its audit entry, fit's `Op.audit`.
    fn audit(&self) -> Vec<(String, Value)> {
        let [_, _, done, failed] = self.counts();
        let mut detail = format!("{done} done, {failed} failed");
        if let Some(p) = &self.purge {
            detail.push_str(&format!("; {}", p.describe()));
        }
        let s = |v: &str| Value::String(v.to_owned());
        let outcomes = self
            .outcomes
            .iter()
            .map(|o| {
                let mut a = vec![
                    ("node".to_owned(), s(&o.node)),
                    ("status".to_owned(), s(o.status)),
                ];
                if !o.detail.is_empty() {
                    a.push(("detail".to_owned(), s(&o.detail)));
                }
                if let Some(e) = &o.error {
                    a.push((
                        "error".to_owned(),
                        s(&crate::api::clip(
                            &format!("{}: {}", e.kind, e.message),
                            MAX_AUDIT_ERROR,
                        )),
                    ));
                }
                if o.before != 0 {
                    a.push(("before".to_owned(), Value::Number(o.before.to_string())));
                }
                if o.after != 0 {
                    a.push(("after".to_owned(), Value::Number(o.after.to_string())));
                }
                Value::Object(a)
            })
            .collect();
        let outcome = if failed == 0 {
            "ok"
        } else if done == 0 {
            "failed"
        } else {
            "partial"
        };
        let mut e = vec![("action".to_owned(), s(&format!("op.{}", self.op)))];
        if !self.group.is_empty() {
            e.push(("group".to_owned(), s(&self.group)));
        }
        e.push(("detail".to_owned(), s(&detail)));
        e.push(("op".to_owned(), s(&self.id)));
        e.push(("outcomes".to_owned(), Value::Array(outcomes)));
        e.push(("outcome".to_owned(), s(outcome)));
        e
    }
}

/// The operations the manager keeps.
#[derive(Default)]
pub(crate) struct Ops {
    list: VecDeque<Arc<Mutex<Op>>>,
    /// The members an operation or a rollout holds, and which: an operation's ID, or
    /// `rollout <id>`.
    pub(crate) busy: HashMap<String, String>,
    running: usize,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn new_id() -> String {
    let mut b = [0u8; 6];
    let _ = akio_crypto::rand::fill_random(&mut b);
    akit_roles::linktoken::hex(&b)
}

/// What a member's answer that is not 2xx means: its error, and the first of its errors in a
/// config refused. fit's `replyProblem`.
pub(crate) fn reply_problem(r: &Reply, what: &str) -> Problem {
    let b = json::parse(&r.body).unwrap_or(Value::Null);
    let mut msg = json::str_of(&b, "error").to_owned();
    if msg.is_empty() {
        msg = format!("status {}", r.status);
    }
    if let Some(Value::Array(errs)) = b.get("errors") {
        if let Some(e) = errs.first() {
            let p = json::str_of(e, "path");
            let m = json::str_of(e, "message");
            msg.push_str(": ");
            if !p.is_empty() {
                msg.push_str(&format!("{p}: "));
            }
            msg.push_str(m);
            if errs.len() > 1 {
                msg.push_str(&format!(" (and {} more)", errs.len().saturating_sub(1)));
            }
        }
    }
    let kind = match r.status {
        401 => KIND_AUTH,
        403 => KIND_FORBIDDEN,
        _ => KIND_REFUSED,
    };
    Problem::new(
        kind,
        crate::api::clip(&format!("{what}: {msg}"), crate::node::MAX_MESSAGE),
    )
    .with_status(r.status)
}

type Done = Box<dyn FnOnce(Result<(Option<String>, String, u64, u64), Problem>) + Send>;

impl Manager {
    /// Sends a request to member `name`'s Admin API as the manager reaches it (through its
    /// console's `/api/m/admin` or with its token), and calls `done` with its 2xx answer, or why
    /// not: fit's `Manager.call`.
    pub(crate) fn call_member(
        &self,
        name: &str,
        method: &str,
        path: &str,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
        done: Box<dyn FnOnce(Result<Reply, Problem>) + Send>,
    ) {
        let what = format!("{method} {path}");
        let n = self.node(name);
        self.call_member_raw(
            name,
            method,
            path,
            headers,
            body,
            Box::new(move |r| {
                done(r.and_then(|rep| {
                    if (200..300).contains(&rep.status) {
                        return Ok(rep);
                    }
                    let mut p = reply_problem(&rep, &what);
                    if let Some(n) = &n {
                        p.message = n.redact(&p.message);
                    }
                    Err(p)
                }));
            }),
        );
    }

    /// [`call_member`](Manager::call_member), with every answer the member gives, whatever its
    /// status: fit's `Manager.Call`.
    pub(crate) fn call_member_raw(
        &self,
        name: &str,
        method: &str,
        path: &str,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
        done: Box<dyn FnOnce(Result<Reply, Problem>) + Send>,
    ) {
        let Some(n) = self.node(name) else {
            return done(Err(Problem::new(
                KIND_UNREACHABLE,
                format!("no member is named {}", json::quoted(name)),
            )));
        };
        let full = if n.spec.access == ACCESS_CONSOLE {
            format!("/api/m/admin{path}")
        } else {
            path.to_owned()
        };
        let n2 = Arc::clone(&n);
        n.call(
            method,
            &full,
            headers,
            body,
            Box::new(move |r| {
                done(match r {
                    Err(mut p) => {
                        p.message = n2.redact(&p.message);
                        Err(p)
                    }
                    Ok((rep, _)) => Ok(rep),
                });
            }),
        );
    }

    fn targets(&self, group: &str, members: &[String]) -> Result<Vec<String>, Refusal> {
        let d = self.doc();
        let in_group = if group.is_empty() {
            Vec::new()
        } else {
            let g = d.groups.iter().find(|g| g.name == group).ok_or_else(|| {
                Refusal::NotFound(format!("no group is named {}", json::quoted(group)))
            })?;
            crate::groups::nodes_of(&d, g)
        };
        if members.is_empty() {
            if group.is_empty() {
                return Err(Refusal::Field(field(
                    "members",
                    "an operation runs on a group, members, or members of a group: name them",
                )));
            }
            if in_group.is_empty() {
                return Err(Refusal::Field(field(
                    "group",
                    format!("the group {group:?} has no members now"),
                )));
            }
            return Ok(in_group);
        }
        let mut names: Vec<String> = Vec::new();
        for n in members {
            if names.contains(n) {
                continue;
            }
            if d.index(n).is_none() {
                return Err(Refusal::NotFound(format!(
                    "no member is named {}",
                    json::quoted(n)
                )));
            }
            if !group.is_empty() && !in_group.contains(n) {
                return Err(Refusal::Field(field(
                    "members",
                    format!("{n:?} is not in the group {group:?}"),
                )));
            }
            names.push(n.clone());
        }
        names.sort();
        Ok(names)
    }

    /// Starts an operation as `user`, and returns it as it starts, every member pending: fit's
    /// `StartOp`.
    fn start_op(&self, user: &str, v: &Value) -> Result<String, Refusal> {
        let op = json::str_of(v, "op").to_owned();
        if !OPS.contains(&op.as_str()) {
            return Err(Refusal::Field(field(
                "op",
                format!("{op:?} is not an operation: {}", OPS.join(", ")),
            )));
        }
        let purge = match v.get("purge") {
            Some(p @ Value::Object(_)) => Some(Purge::from_value(p)),
            _ => None,
        };
        if op == "purge" {
            match &purge {
                Some(p) => p.check().map_err(Refusal::Field)?,
                None => {
                    return Err(Refusal::Field(field(
                        "purge",
                        "a purge names keys, prefixes, tags, or all",
                    )));
                }
            }
        } else if purge.is_some() {
            return Err(Refusal::Field(field("purge", "only a purge takes purge")));
        }
        let ro = self.read_only();
        if !ro.is_empty() {
            // An operation is recorded in the audit log, which a read-only document cannot keep.
            return Err(Refusal::Conflict(ro));
        }
        let group = json::str_of(v, "group").to_owned();
        let names = self.targets(&group, &json::strings_of(v, "members"))?;
        // An operator of each member (I139).
        self.allow(
            user,
            &crate::roles::Need::on(crate::roles::OPERATOR, names.clone()),
        )?;
        let op = {
            let mut ops = lock(&self.0.ops);
            let taken: Vec<String> = names
                .iter()
                .filter_map(|n| ops.busy.get(n).map(|id| format!("{n} ({id})")))
                .collect();
            if !taken.is_empty() {
                return Err(Refusal::Conflict(format!(
                    "an operation runs on {} already: wait for it to end",
                    taken.join(", ")
                )));
            }
            if ops.running >= MAX_RUNNING_OPS {
                return Err(Refusal::Busy(format!(
                    "{MAX_RUNNING_OPS} operations run already; try again once one ends"
                )));
            }
            let op = Op {
                id: new_id(),
                op,
                group,
                user: user.to_owned(),
                purge,
                started: SystemTime::now(),
                ended: None,
                finished: false,
                outcomes: names
                    .iter()
                    .map(|n| Outcome {
                        node: n.clone(),
                        status: "pending",
                        detail: String::new(),
                        error: None,
                        result: None,
                        before: 0,
                        after: 0,
                        started: None,
                        ended: None,
                        took_ms: 0,
                    })
                    .collect(),
            };
            for n in &names {
                ops.busy.insert(n.clone(), op.id.clone());
            }
            ops.running = ops.running.saturating_add(1);
            let op = Arc::new(Mutex::new(op));
            ops.list.push_back(Arc::clone(&op));
            trim(&mut ops);
            op
        };
        let (out, id, kind) = {
            let o = lock(&op);
            (o.json(), o.id.clone(), o.op.clone())
        };
        self.log(format!(
            "cluster: {user:?} started the operation {id} {kind} on {}",
            names.join(", ")
        ));
        let total = names.len();
        let first = total.min(self.0.concurrency);
        // The next member to start, and the members not answered yet.
        let progress = Arc::new(Mutex::new((first, total)));
        for i in 0..first {
            self.run_member(&op, i, &progress, total);
        }
        Ok(out)
    }

    /// Runs `op` on its `i`-th member, and, once it answers, on the next one waiting.
    fn run_member(
        &self,
        op: &Arc<Mutex<Op>>,
        i: usize,
        progress: &Arc<Mutex<(usize, usize)>>,
        total: usize,
    ) {
        let start = Instant::now();
        let (name, kind, purge) = {
            let mut o = lock(op);
            let Some(out) = o.outcomes.get_mut(i) else {
                return;
            };
            out.status = "running";
            out.started = Some(SystemTime::now());
            (out.node.clone(), o.op.clone(), o.purge.clone())
        };
        let (me, op2, progress2) = (self.clone(), Arc::clone(op), Arc::clone(progress));
        let done: Done = Box::new(move |r| {
            {
                let mut o = lock(&op2);
                if let Some(out) = o.outcomes.get_mut(i) {
                    out.ended = Some(SystemTime::now());
                    out.took_ms = json::millis(start.elapsed());
                    match r {
                        Ok((result, detail, before, after)) => {
                            out.status = "done";
                            out.result = result;
                            out.detail = detail;
                            out.before = before;
                            out.after = after;
                        }
                        Err(p) => {
                            out.status = "failed";
                            out.error = Some(p);
                        }
                    }
                }
            }
            let (next, left) = {
                let mut p = lock(&progress2);
                p.1 = p.1.saturating_sub(1);
                let next = (p.0 < total).then_some(p.0);
                p.0 = p.0.saturating_add(1);
                (next, p.1)
            };
            if let Some(j) = next {
                me.run_member(&op2, j, &progress2, total);
            } else if left == 0 {
                // The last member to answer ends the operation.
                me.end_op(&op2);
            }
        });
        if self.0.closed.load(Ordering::Acquire) {
            return done(Err(Problem::new(
                KIND_UNREACHABLE,
                "the manager stopped before the operation reached the member",
            )));
        }
        self.run_one(&name, &kind, purge, done);
    }

    fn end_op(&self, op: &Arc<Mutex<Op>>) {
        let (entry, user, id, kind, done, failed, nodes) = {
            let mut o = lock(op);
            o.ended = Some(SystemTime::now());
            o.finished = true;
            let [_, _, d, f] = o.counts();
            let nodes: Vec<String> = o.outcomes.iter().map(|x| x.node.clone()).collect();
            (
                o.audit(),
                o.user.clone(),
                o.id.clone(),
                o.op.clone(),
                d,
                f,
                nodes,
            )
        };
        {
            let mut ops = lock(&self.0.ops);
            for n in &nodes {
                if ops.busy.get(n) == Some(&id) {
                    ops.busy.remove(n);
                }
            }
            ops.running = ops.running.saturating_sub(1);
            trim(&mut ops);
        }
        // What the operation changed is read by the next poll, at once.
        for n in &nodes {
            if let Some(node) = self.node(n) {
                node.next.store(0, Ordering::Release);
            }
        }
        self.poke();
        self.log(format!(
            "cluster: the operation {id} {kind} ended: {done} done, {failed} failed"
        ));
        self.record(&user, entry);
    }

    /// Runs operation `kind` on member `name`: fit's `runOne`.
    fn run_one(&self, name: &str, kind: &str, purge: Option<Purge>, done: Done) {
        if kind == "test" {
            return self.op_test(name, done);
        }
        let mut need: Vec<&str> = match kind {
            "reload" => vec!["config.reload"],
            "rollback" => vec!["config.versions"],
            "check" => vec!["config.check"],
            _ => vec!["cache"],
        };
        if purge.as_ref().is_some_and(|p| !p.prefixes.is_empty()) {
            need.push("cache.prefix");
        }
        let caps = self.node(name).and_then(|n| n.caps());
        let (me, name2, kind2) = (self.clone(), name.to_owned(), kind.to_owned());
        let go = move |caps: Vec<String>, done: Done| {
            if let Some(c) = need.iter().find(|c| !caps.iter().any(|x| x == **c)) {
                return done(Err(Problem::new(
                    KIND_UNSUPPORTED,
                    format!(
                        "the member does not list {c} in its capabilities: it is older than this \
                         operation, or does not serve it"
                    ),
                )));
            }
            match kind2.as_str() {
                "reload" => me.op_reload(&name2, done),
                "rollback" => me.op_rollback(&name2, done),
                "check" => me.op_check(&name2, done),
                _ => me.op_purge(&name2, purge.unwrap_or_default(), done),
            }
        };
        match caps {
            Some(c) => go(c, done),
            // Not polled yet: its GET /node says what it serves.
            None => self.call_member(
                name,
                "GET",
                "/node",
                Vec::new(),
                None,
                Box::new(move |r| match r {
                    Err(p) => done(Err(p)),
                    Ok(rep) => {
                        let caps = json::parse(&rep.body)
                            .map(|v| json::strings_of(&v, "capabilities"))
                            .unwrap_or_default();
                        go(caps, done);
                    }
                }),
            ),
        }
    }

    fn op_test(&self, name: &str, done: Done) {
        let start = Instant::now();
        self.call_member(
            name,
            "GET",
            "/node",
            Vec::new(),
            None,
            Box::new(move |r| {
                done(r.and_then(|rep| {
                    let v = json::parse(&rep.body).ok_or_else(|| {
                        Problem::new(
                            KIND_BAD_ANSWER,
                            "the member's /node is not the JSON expected",
                        )
                    })?;
                    let info = json::parse(crate::node::node_info_json(&v).as_bytes())
                        .unwrap_or(Value::Null);
                    let version = json::str_of(&info, "akit_version").to_owned();
                    let ms = json::millis(start.elapsed());
                    let result = Obj::new()
                        .num("latency_ms", ms)
                        .str("node_id", json::str_of(&v, "id"))
                        .str("akit_version", &version)
                        .end();
                    Ok((Some(result), format!("reached in {ms} ms: {version}"), 0, 0))
                }));
            }),
        );
    }

    fn polled_version(&self, name: &str) -> u64 {
        self.node(name).map_or(0, |n| n.state().config_version)
    }

    fn op_reload(&self, name: &str, done: Done) {
        let before = self.polled_version(name);
        self.call_member(
            name,
            "POST",
            "/reload",
            Vec::new(),
            Some(Vec::new()),
            Box::new(move |r| {
                done(r.map(|rep| {
                    let (v, detail) = applied("reloaded", &rep);
                    let result = Obj::new().num("version", v).num("previous", before).end();
                    (Some(result), detail, before, v)
                }));
            }),
        );
    }

    fn op_rollback(&self, name: &str, done: Done) {
        let (me, name2) = (self.clone(), name.to_owned());
        self.call_member(
            name,
            "GET",
            "/versions",
            Vec::new(),
            None,
            Box::new(move |r| {
                let rep = match r {
                    Ok(rep) => rep,
                    Err(p) => return done(Err(p)),
                };
                let v = json::parse(&rep.body).unwrap_or(Value::Null);
                let current = json::uint_of(&v, "current");
                let versions = match v.get("versions") {
                    Some(Value::Array(a)) => a.clone(),
                    _ => Vec::new(),
                };
                let etag = versions
                    .iter()
                    .find(|x| json::uint_of(x, "version") == current)
                    .map(|x| json::str_of(x, "etag").to_owned())
                    .unwrap_or_default();
                let to = versions
                    .iter()
                    .rev()
                    .find(|x| {
                        json::uint_of(x, "version") < current && json::str_of(x, "etag") != etag
                    })
                    .map_or(0, |x| json::uint_of(x, "version"));
                if to == 0 {
                    return done(Err(Problem::new(
                        KIND_NO_EARLIER,
                        format!(
                            "the member keeps no earlier config than the running one (version \
                             {current}) to roll back to"
                        ),
                    )));
                }
                let mut h = Vec::new();
                if !etag.is_empty() {
                    h.push(("If-Match".to_owned(), etag));
                }
                me.call_member(
                    &name2,
                    "POST",
                    &format!("/rollback/{to}"),
                    h,
                    Some(Vec::new()),
                    Box::new(move |r| {
                        done(r.map(|rep| {
                            let (v, detail) =
                                applied(&format!("rolled back to version {to},"), &rep);
                            let result = Obj::new()
                                .num("version", v)
                                .num("from", to)
                                .num("previous", current)
                                .end();
                            (Some(result), detail, current, v)
                        }));
                    }),
                );
            }),
        );
    }

    fn op_check(&self, name: &str, done: Done) {
        self.call_member(
            name,
            "GET",
            "/config/check",
            Vec::new(),
            None,
            Box::new(move |r| {
                done(r.map(|rep| {
                    let c = json::parse(&rep.body).unwrap_or(Value::Null);
                    let sum = c.get("summary").cloned().unwrap_or(Value::Null);
                    let count = json::int_of(&sum, "drift");
                    let drift = Obj::new()
                        .num("count", count)
                        .num("info", json::int_of(&sum, "info"))
                        .raw(
                            "checked_at",
                            &c.get("checked_at").map_or_else(
                                || "\"0001-01-01T00:00:00Z\"".to_owned(),
                                json::compact,
                            ),
                        )
                        .end();
                    let detail = if count > 0 {
                        format!("{count} differences")
                    } else {
                        "in sync".to_owned()
                    };
                    (Some(Obj::new().raw("drift", &drift).end()), detail, 0, 0)
                }));
            }),
        );
    }

    fn op_purge(&self, name: &str, p: Purge, done: Done) {
        let body = Obj::new()
            .str("zone", &p.zone)
            .raw("urls", &json::string_list(&p.keys))
            .raw("prefixes", &json::string_list(&p.prefixes))
            .raw("tags", &json::string_list(&p.tags))
            .bool("all", p.all)
            .end();
        self.call_member(
            name,
            "POST",
            "/cache/purge",
            Vec::new(),
            Some(body.into_bytes()),
            Box::new(move |r| {
                done(r.map(|rep| {
                    let n = json::parse(&rep.body).map_or(0, |v| json::int_of(&v, "purged"));
                    (
                        Some(Obj::new().num("purged", n).end()),
                        format!("purged {n} keys"),
                        0,
                        0,
                    )
                }));
            }),
        );
    }
}

/// The version a member's apply answered, and what it did with its warnings.
fn applied(what: &str, rep: &Reply) -> (u64, String) {
    let v = json::parse(&rep.body).unwrap_or(Value::Null);
    let version = json::uint_of(&v, "version");
    let mut s = format!("{what} as version {version}");
    let warnings = json::strings_of(&v, "warnings");
    if !warnings.is_empty() {
        s.push_str(&format!(
            "; {}",
            crate::api::clip(&warnings.join("; "), 256)
        ));
    }
    (version, s)
}

/// Drops the oldest finished operations past [`MAX_KEPT_OPS`].
fn trim(ops: &mut Ops) {
    while ops.list.len() > MAX_KEPT_OPS {
        let Some(i) = ops.list.iter().position(|o| lock(o).finished) else {
            return;
        };
        ops.list.remove(i);
    }
}

/// The operations' endpoints, when `path` is one:
///
/// ```text
/// GET  /api/ops        the operations kept, the latest first
/// POST /api/ops        an operation started: 202, followed at GET /api/ops/<id>
/// GET  /api/ops/<id>   an operation and each member's outcome
/// ```
pub(crate) fn serve(m: &Manager, ctx: &mut Context, user: &str, path: &str) -> bool {
    let method = ctx.method().clone();
    let get = matches!(method, Method::Get | Method::Head);
    match path {
        "/api/ops" if get => {
            let list = lock(&m.0.ops)
                .list
                .iter()
                .rev()
                .map(|o| lock(o).json())
                .collect::<Vec<_>>()
                .join(",");
            ctx.respond(answer(200, format!("{{\"ops\":[{list}]}}")));
        }
        "/api/ops" if method == Method::Post => {
            if ctx.body().len() > MAX_BODY_BYTES {
                ctx.respond(fail(
                    413,
                    &format!("an operation is {MAX_BODY_BYTES} bytes at most"),
                ));
                return true;
            }
            let v = match akit_roles::pages::decode(ctx) {
                Ok(b) => b.0,
                Err(r) => {
                    ctx.respond(r);
                    return true;
                }
            };
            if m.0.closed.load(Ordering::Acquire) {
                ctx.respond(fail(503, "the manager is shutting down"));
                return true;
            }
            ctx.respond(match m.start_op(user, &v) {
                Ok(op) => answer(202, op),
                Err(e) => e.response(),
            });
        }
        "/api/ops" => {
            ctx.respond(fail(405, "the method is not allowed").header("Allow", "GET, POST"));
        }
        p if p.starts_with("/api/ops/") && get => {
            let id = p.trim_start_matches("/api/ops/");
            let found = lock(&m.0.ops)
                .list
                .iter()
                .find(|o| lock(o).id == id)
                .map(|o| lock(o).json());
            ctx.respond(match found {
                Some(op) => answer(200, op),
                None => fail(404, &format!("no operation {} is kept", json::quoted(id))),
            });
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn purges_checked() {
        // fit's TestPurgeParamsCheck.
        let ok = Purge {
            prefixes: vec!["https://a.example/x/".into()],
            ..Purge::default()
        };
        assert!(ok.check().is_ok());
        assert_eq!(Purge::default().check().unwrap_err().field, "purge");
        let both = Purge {
            all: true,
            tags: vec!["t".into()],
            ..Purge::default()
        };
        assert_eq!(both.check().unwrap_err().field, "purge.all");
        let rel = Purge {
            keys: vec!["/x".into()],
            ..Purge::default()
        };
        assert_eq!(rel.check().unwrap_err().field, "purge.keys");
        let many = Purge {
            tags: vec!["t".into(); MAX_PURGE_ITEMS + 1],
            ..Purge::default()
        };
        assert_eq!(many.check().unwrap_err().field, "purge.tags");
        assert_eq!(ok.describe(), "every zone: prefixes https://a.example/x/");
    }

    #[test]
    fn refusals_read() {
        let r = Reply {
            status: 400,
            headers: Vec::new(),
            body: br#"{"error": "the config does not load", "errors": [{"path": "routes[0]", "message": "bad"}, {"path": "x", "message": "y"}]}"#.to_vec(),
            took: std::time::Duration::ZERO,
        };
        let p = reply_problem(&r, "POST /reload");
        assert_eq!(p.kind, KIND_REFUSED);
        assert_eq!(
            p.message,
            "POST /reload: the config does not load: routes[0]: bad (and 1 more)"
        );
    }
}
