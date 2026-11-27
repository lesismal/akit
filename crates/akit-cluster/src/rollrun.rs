//! A rollout's plan and its run (I137, fit's `cluster/rollrun.go` and `rollouts.go`'s
//! `planRollout` and `planMember`), each on a thread of its own, never an I/O thread: the calls
//! to members go out on akio's asynchronous client and the thread waits for their answers,
//! `max_concurrency` members at once. The pause between batches and a gate's window are waited
//! for on the rollouts' condition variable, woken by a person's pause or abort and by the
//! manager's close (rule: timers only where the logic is about time).

// A member's problem is the answer itself, made once per member: its size does not matter.
#![allow(clippy::result_large_err)]

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant, SystemTime};

use akit_config::value::Value;

use crate::Manager;
use crate::api::clip;
use crate::json::{self, quoted};
use crate::jv;
use crate::node::{CAP_CHECK, MAX_MESSAGE};
use crate::ops::{KIND_NO_EARLIER, KIND_REFUSED, reply_problem};
use crate::rollouts::{
    ABORTED, B_ABORTED, B_APPLYING, B_FAILED, B_GATING, B_PASSED, B_ROLLED_BACK, CAP_DRY_RUN,
    CAP_IF_MATCH, CAP_VERSIONS, ConfigError, DONE, FAILED, Fault, Gate, GateResult, KIND_BUSY,
    KIND_COPY, KIND_PATCH, KIND_TEMPLATE, M_APPLIED, M_APPLYING, M_DONE, M_ERROR, M_FAILED,
    M_PENDING, M_READY, M_ROLLBACK_FAILED, M_ROLLED_BACK, M_SKIPPED, M_UNCHANGED, MAX_ERRORS,
    MAX_GATE_REASONS, MAX_ROLLOUT_BYTES, ON_FAILURE_ALL, PAUSED, PLANNED, ROLLED_BACK,
    ROLLING_BACK, RolloutMember, WAITING, fragment_parts, unredacted,
};
use crate::sync::{self, MemberSync, Out, Rendered};
use crate::templates::{self, SECRET_MASK};
use crate::transport::{
    KIND_BAD_ANSWER, KIND_TOO_LARGE, KIND_UNREACHABLE, KIND_UNSUPPORTED, Problem, Reply,
};

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn ok(status: u16) -> bool {
    (200..300).contains(&status)
}

/// How a batch went: fit's `batchResult`.
#[derive(Debug, Default)]
struct BatchResult {
    passed: bool,
    why: String,
    /// The manager stops.
    stopped: bool,
    /// A person aborted it during its gate's wait.
    aborted: bool,
}

impl BatchResult {
    fn stopped() -> BatchResult {
        BatchResult {
            stopped: true,
            ..BatchResult::default()
        }
    }

    fn failed(why: String) -> BatchResult {
        BatchResult {
            why,
            ..BatchResult::default()
        }
    }
}

/// How a gate's window ended.
enum Wait {
    Elapsed,
    Stopped,
    Aborted,
}

/// A config as Go's `json.MarshalIndent(v, "", "  ")` writes it, the body a rollout sends.
fn pretty(v: &Value) -> String {
    fn indent(b: &mut String, depth: usize) {
        b.push('\n');
        for _ in 0..depth {
            b.push_str("  ");
        }
    }
    fn write(b: &mut String, v: &Value, depth: usize) {
        let inner = depth.saturating_add(1);
        match v {
            Value::Object(ms) if !ms.is_empty() => {
                b.push('{');
                for (i, (k, x)) in ms.iter().enumerate() {
                    if i > 0 {
                        b.push(',');
                    }
                    indent(b, inner);
                    b.push_str(&quoted(k));
                    b.push_str(": ");
                    write(b, x, inner);
                }
                indent(b, depth);
                b.push('}');
            }
            Value::Array(items) if !items.is_empty() => {
                b.push('[');
                for (i, x) in items.iter().enumerate() {
                    if i > 0 {
                        b.push(',');
                    }
                    indent(b, inner);
                    write(b, x, inner);
                }
                indent(b, depth);
                b.push(']');
            }
            _ => json::write(b, v),
        }
    }
    let mut b = String::new();
    write(&mut b, v, 0);
    b.push('\n');
    b
}

/// The errors of a config refused, bounded: fit's `configErrors`.
fn config_errors(body: &[u8]) -> Vec<ConfigError> {
    let v = json::parse(body).unwrap_or(Value::Null);
    let Some(Value::Array(errs)) = v.get("errors") else {
        return Vec::new();
    };
    errs.iter()
        .take(MAX_ERRORS)
        .map(|e| ConfigError {
            path: clip(json::str_of(e, "path"), 256),
            message: clip(json::str_of(e, "message"), 512),
        })
        .collect()
}

fn clip_list(list: &[String], n: usize, each: usize) -> Vec<String> {
    list.iter().take(n).map(|s| clip(s, each)).collect()
}

/// The requests a member's `GET /status` counts: the total and the 5xx.
fn requests(v: &Value) -> Option<(u64, u64)> {
    let r = v
        .get("requests")
        .filter(|r| matches!(r, Value::Object(_)))?;
    let fives = r.get("classes").map_or(0, |c| json::uint_of(c, "5xx"));
    Some((json::uint_of(r, "total"), fives))
}

/// The parts that change from `have` to `want`, with their sync states when a sync planned them:
/// fit's `rolloutChanges` over `compareParts`.
fn rollout_changes(
    have: &Value,
    want: &Value,
    sy: Option<&MemberSync>,
) -> Vec<crate::rollouts::Change> {
    let (pa, pb) = (sync::parts_of(have), sync::parts_of(want));
    let masked = |v: &Value| json::compact(&templates::masked(v, SECRET_MASK));
    let mut out = Vec::new();
    for p in &pa {
        let (state, b) = match pb.iter().find(|q| q.0 == p.0) {
            None => ("only_a", None),
            Some(q) if jv::hash(&q.5) == jv::hash(&p.5) => continue,
            Some(q) => ("differs", Some(masked(&q.5))),
        };
        out.push(crate::rollouts::Change {
            path: p.0.clone(),
            state: state.to_owned(),
            have: Some(masked(&p.5)),
            want: b,
            ..crate::rollouts::Change::default()
        });
    }
    for q in &pb {
        if !pa.iter().any(|p| p.0 == q.0) {
            out.push(crate::rollouts::Change {
                path: q.0.clone(),
                state: "added".to_owned(),
                want: Some(masked(&q.5)),
                ..crate::rollouts::Change::default()
            });
        }
    }
    if let Some(sy) = sy {
        for c in &mut out {
            if let Some(ps) = sy.parts.iter().rev().find(|ps| ps.path == c.path) {
                ps.state.clone_into(&mut c.sync);
                ps.reason.clone_into(&mut c.reason);
                c.template.clone_from(&ps.template);
            }
        }
    }
    out
}

/// `parts` with the secrets they mask put back from `cfg`, the member's own: fit's
/// `withOwnSecrets`.
fn with_own_secrets(parts: &[Rendered], cfg: &Value) -> Result<Vec<Rendered>, String> {
    parts
        .iter()
        .map(|p| {
            let mut q = p.clone();
            templates::restore_secrets(
                &mut q.value,
                jv::lookup(cfg, &p.member, &p.name),
                &p.display(),
            )?;
            Ok(q)
        })
        .collect()
}

impl Manager {
    fn stopping(&self) -> bool {
        self.0.closed.load(Ordering::Acquire)
    }

    /// A call to member `name`, waited for: fit's `callSync`. Never on an I/O thread.
    fn call_sync(
        &self,
        name: &str,
        method: &str,
        path: &str,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
    ) -> Result<Reply, Problem> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.call_member_raw(
            name,
            method,
            path,
            headers,
            body,
            Box::new(move |r| {
                let _ = tx.send(r);
            }),
        );
        rx.recv().unwrap_or_else(|_| {
            Err(Problem::new(
                KIND_UNREACHABLE,
                "the call to the member ended unanswered",
            ))
        })
    }

    /// `GET path` of member `name`, as JSON: fit's `getSync`.
    fn get_sync(&self, name: &str, path: &str) -> Result<Value, Problem> {
        let rep = self.call_sync(name, "GET", path, Vec::new(), None)?;
        if !ok(rep.status) {
            return Err(reply_problem(&rep, &format!("GET {path}")));
        }
        json::parse(&rep.body).ok_or_else(|| {
            Problem::new(
                KIND_BAD_ANSWER,
                format!("the member's {path} is not the JSON expected"),
            )
            .with_status(rep.status)
        })
    }

    /// The capabilities member `name` lists, as polled or read now.
    fn caps_sync(&self, name: &str) -> Result<Vec<String>, Problem> {
        if let Some(c) = self.node(name).and_then(|n| n.caps()) {
            return Ok(c);
        }
        self.get_sync(name, "/node")
            .map(|v| json::strings_of(&v, "capabilities"))
    }

    /// The version and the ETag of the config member `name` runs, its `GET /node`'s.
    fn node_config(&self, name: &str) -> Result<(u64, String), Problem> {
        self.get_sync(name, "/node").map(|v| {
            (
                json::uint_of(&v, "config_version"),
                json::str_of(&v, "config_etag").to_owned(),
            )
        })
    }

    fn fetch_config_sync(&self, name: &str) -> Result<(Value, String), Problem> {
        let (tx, rx) = mpsc::sync_channel(1);
        sync::fetch_config(
            self,
            name,
            Box::new(move |r| {
                let _ = tx.send(r);
            }),
        );
        rx.recv().unwrap_or_else(|_| {
            Err(Problem::new(
                KIND_UNREACHABLE,
                "the call to the member ended unanswered",
            ))
        })
    }

    /// Member `name` against its templates, with a draft in place of one of them.
    fn sync_sync(&self, name: &str, draft: Option<(String, Value)>) -> Out {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sync_draft(
            name,
            false,
            draft,
            Box::new(move |o| {
                let _ = tx.send(o);
            }),
        );
        rx.recv().unwrap_or_else(|_| Out {
            sync: MemberSync::failed(
                name,
                Problem::new(KIND_UNREACHABLE, "the member's read ended unanswered"),
            ),
            config: None,
            desired: None,
        })
    }

    /// Calls `f` on each of `xs`, `max_concurrency` at once, on threads of the caller's, and
    /// returns once each has: fit's `each`.
    fn each<T: Send>(&self, xs: Vec<T>, f: impl Fn(T) + Sync) {
        let workers = self.0.concurrency.min(xs.len());
        let q = Mutex::new(xs.into_iter());
        let work = || {
            loop {
                let next = lock(&q).next();
                match next {
                    Some(x) => f(x),
                    None => return,
                }
            }
        };
        std::thread::scope(|s| {
            for _ in 1..workers {
                // One that does not start leaves its share to the others.
                let _ = std::thread::Builder::new()
                    .name("akit-rollout-member".to_owned())
                    .spawn_scoped(s, work);
            }
            work();
        });
    }

    /// Changes member `name` of rollout `id` with `f`.
    fn set_member(&self, id: &str, name: &str, f: impl FnOnce(&mut RolloutMember)) {
        let mut ro = lock(&self.0.ro);
        if let Some(r) = ro.get_mut(id) {
            if let Some(x) = r.member_mut(name) {
                f(x);
            }
            r.count();
        }
    }

    /// Plans rollout `id`: each member's diff and dry run: fit's `planRollout`.
    pub(crate) fn plan_run(&self, id: &str) {
        let Some((kind, from, paths, config, template, names)) =
            lock(&self.0.ro).get(id).map(|r| {
                (
                    r.kind.clone(),
                    r.from.clone(),
                    r.paths.clone(),
                    r.config.clone(),
                    r.template.clone(),
                    r.nodes(),
                )
            })
        else {
            return;
        };
        let mut parts: Vec<Rendered> = Vec::new();
        let mut src: Option<Fault> = None;
        if kind == KIND_PATCH {
            if let Some(c) = &config {
                parts = fragment_parts(c);
            }
        }
        if kind == KIND_COPY {
            match self.fetch_config_sync(&from) {
                Err(p) => {
                    let message = clip(
                        &format!(
                            "the member {} the parts are copied from could not be read: {}",
                            quoted(&from),
                            p.message
                        ),
                        MAX_MESSAGE,
                    );
                    src = Some(Fault::of(&Problem { message, ..p }));
                }
                Ok((cfg, _)) => match sync::fragment_of(&cfg, &paths) {
                    Err(e) => src = Some(Fault::new(KIND_REFUSED, clip(&e.msg, MAX_MESSAGE))),
                    Ok(frag) => parts = fragment_parts(&unredacted(frag)),
                },
            }
        }
        let draft = if kind == KIND_TEMPLATE {
            config.map(|c| (template, c))
        } else {
            None
        };
        if let Some(f) = src {
            for n in &names {
                self.set_member(id, n, |x| {
                    M_ERROR.clone_into(&mut x.status);
                    x.error = Some(f.clone());
                });
            }
        } else {
            let size = Mutex::new(0usize);
            self.each(names, |name: String| {
                let mut pm = self.plan_member(&kind, &name, &parts, draft.clone());
                if pm.status == M_READY {
                    let n =
                        pm.changes
                            .iter()
                            .fold(pm.config.as_ref().map_or(0, Vec::len), |n, c| {
                                n.saturating_add(c.have.as_ref().map_or(0, String::len))
                                    .saturating_add(c.want.as_ref().map_or(0, String::len))
                                    .saturating_add(c.path.len())
                                    .saturating_add(64)
                            });
                    let mut sz = lock(&size);
                    if sz.saturating_add(n) > MAX_ROLLOUT_BYTES {
                        M_ERROR.clone_into(&mut pm.status);
                        pm.config = None;
                        pm.changes.clear();
                        pm.error = Some(Fault::new(
                            KIND_TOO_LARGE,
                            format!(
                                "the rollout keeps {MAX_ROLLOUT_BYTES} bytes of its members' \
                                 configs and diffs at most: plan fewer members at once"
                            ),
                        ));
                    } else {
                        *sz = sz.saturating_add(n);
                    }
                }
                self.set_member(id, &name, move |x| *x = pm);
            });
        }
        if self.stopping() {
            return;
        }
        let Some((ready, unchanged, errs)) = lock(&self.0.ro).get_mut(id).map(|r| {
            PLANNED.clone_into(&mut r.state);
            r.planned = Some(SystemTime::now());
            r.count();
            (
                r.counted(M_READY),
                r.counted(M_UNCHANGED),
                r.counted(M_ERROR),
            )
        }) else {
            return;
        };
        self.save_rollouts(Vec::new());
        self.log(format!(
            "cluster: the rollout {id} is planned: {ready} members ready, {unchanged} unchanged, \
             {errs} not ready"
        ));
    }

    /// Member `name`'s part of a plan: its config read, the one it is to run made, its changes
    /// and its dry run: fit's `planMember`.
    fn plan_member(
        &self,
        kind: &str,
        name: &str,
        parts: &[Rendered],
        draft: Option<(String, Value)>,
    ) -> RolloutMember {
        let pm = RolloutMember::new(name, M_ERROR);
        let fail = |mut pm: RolloutMember, f: Fault| {
            pm.error = Some(f);
            pm
        };
        let held = lock(&self.0.ops).busy.get(name).cloned();
        if let Some(by) = held {
            return fail(
                pm,
                Fault::new(
                    KIND_BUSY,
                    format!(
                        "the member is held by {by}: one rollout or operation runs on a member \
                         at a time"
                    ),
                ),
            );
        }
        let caps = match self.caps_sync(name) {
            Ok(c) => c,
            Err(p) => return fail(pm, Fault::of(&p)),
        };
        for c in [CAP_IF_MATCH, CAP_DRY_RUN, CAP_VERSIONS] {
            if !caps.iter().any(|x| x == c) {
                return fail(
                    pm,
                    Fault::new(
                        KIND_UNSUPPORTED,
                        format!(
                            "the member does not list {c} in its capabilities: it is older than \
                             rollouts"
                        ),
                    ),
                );
            }
        }
        self.plan_config(pm, kind, name, parts, draft)
    }

    fn plan_config(
        &self,
        mut pm: RolloutMember,
        kind: &str,
        name: &str,
        parts: &[Rendered],
        draft: Option<(String, Value)>,
    ) -> RolloutMember {
        let fail = |mut pm: RolloutMember, f: Fault| {
            pm.error = Some(f);
            pm
        };
        let (cfg, want, etag, sy) = if kind == KIND_COPY || kind == KIND_PATCH {
            let (cfg, etag) = match self.fetch_config_sync(name) {
                Ok(x) => x,
                Err(p) => return fail(pm, Fault::of(&p)),
            };
            let own = match with_own_secrets(parts, &cfg) {
                Ok(o) => o,
                Err(e) => return fail(pm, Fault::new(KIND_REFUSED, clip(&e, MAX_MESSAGE))),
            };
            let want = sync::desired(&cfg, &own);
            (cfg, want, etag, None)
        } else {
            let o = self.sync_sync(name, draft);
            let (Some(cfg), Some(want)) = (o.config, o.desired) else {
                let f = o.sync.error.as_ref().map_or_else(
                    || Fault::new(KIND_UNREACHABLE, "the member could not be read"),
                    Fault::of,
                );
                return fail(pm, f);
            };
            if o.sync.state == "error" {
                return fail(
                    pm,
                    Fault::new(
                        KIND_REFUSED,
                        clip(
                            &format!(
                                "its templates do not render for it: {}",
                                o.sync.errors.join("; ")
                            ),
                            MAX_MESSAGE,
                        ),
                    ),
                );
            }
            pm.note.clone_from(&o.sync.note);
            let etag = o.sync.config_etag.clone();
            (cfg, want, etag, Some(o.sync))
        };
        pm.config_etag.clone_from(&etag);
        if jv::hash(&cfg) == jv::hash(&want) {
            M_UNCHANGED.clone_into(&mut pm.status);
            return pm;
        }
        pm.changes = rollout_changes(&cfg, &want, sy.as_ref());
        pm.changed = pm.changes.iter().map(|c| c.path.clone()).collect();
        let body = pretty(&want).into_bytes();
        pm.digest = jv::hash(&want);
        let mut h = Vec::new();
        if !etag.is_empty() {
            h.push(("If-Match".to_owned(), etag));
        }
        let rep = match self.call_sync(name, "PUT", "/config?dry_run=1", h, Some(body.clone())) {
            Ok(r) => r,
            Err(p) => return fail(pm, Fault::of(&p)),
        };
        if rep.status == 412 {
            return fail(
                pm,
                Fault::status(
                    KIND_REFUSED,
                    rep.status,
                    "the member's config changed while it was planned: plan the rollout again",
                ),
            );
        }
        if !ok(rep.status) {
            pm.errors = config_errors(&rep.body);
            let p = reply_problem(&rep, "its dry run");
            return fail(pm, Fault::of(&p));
        }
        let a = json::parse(&rep.body).unwrap_or(Value::Null);
        let new_etag = json::str_of(&a, "new_etag").to_owned();
        if new_etag.is_empty() {
            return fail(
                pm,
                Fault::status(
                    KIND_BAD_ANSWER,
                    rep.status,
                    "the member's dry run is not the JSON expected",
                ),
            );
        }
        if json::bool_of(&a, "unchanged") {
            M_UNCHANGED.clone_into(&mut pm.status);
            pm.changes.clear();
            pm.changed.clear();
            return pm;
        }
        M_READY.clone_into(&mut pm.status);
        pm.version = json::uint_of(&a, "version");
        pm.new_etag = new_etag;
        pm.config = Some(body);
        pm.warnings = clip_list(&json::strings_of(&a, "warnings"), MAX_ERRORS, 256);
        pm
    }

    fn finish(&self, ended: Option<(Value, String)>) {
        if let Some((e, line)) = ended {
            self.save_rollouts(vec![e]);
            self.log(line);
        }
    }

    /// Runs rollout `id`, batch by batch, until it ends, pauses, waits after its canary or the
    /// manager stops: fit's `runRollout`.
    pub(crate) fn run_rollout(&self, id: &str, run_no: u64) {
        struct Ends<'a>(&'a Manager, &'a str, u64);
        impl Drop for Ends<'_> {
            fn drop(&mut self) {
                let mut ro = lock(&self.0.0.ro);
                if ro.runs.get(self.1).is_some_and(|r| r.run_no == self.2) {
                    ro.runs.remove(self.1);
                }
            }
        }
        let _ends = Ends(self, id, run_no);
        let rolling = lock(&self.0.ro)
            .get(id)
            .is_some_and(|r| r.state == ROLLING_BACK);
        if rolling {
            return self.roll_back_all(id);
        }
        loop {
            let b = {
                let mut ro = lock(&self.0.ro);
                let f = ro.flags(id, run_no);
                if f.abort {
                    let e = self.end(
                        &mut ro,
                        id,
                        ABORTED,
                        "aborted by a person; what was applied stays",
                    );
                    drop(ro);
                    return self.finish(e);
                }
                let Some(r) = ro.get_mut(id) else { return };
                if f.pause {
                    PAUSED.clone_into(&mut r.state);
                    "paused by a person after its batch".clone_into(&mut r.reason);
                    r.next_at = None;
                    r.count();
                    drop(ro);
                    return self.save_rollouts(Vec::new());
                }
                if r.batch >= r.batches.len() {
                    let e = self.end(&mut ro, id, DONE, "every batch passed its gate");
                    drop(ro);
                    return self.finish(e);
                }
                r.batch
            };
            let res = self.run_batch(id, run_no, b);
            if res.stopped {
                return self.pause_for_stop(id);
            }
            if res.aborted {
                let mut ro = lock(&self.0.ro);
                if let Some(bt) = ro.get_mut(id).and_then(|r| r.batches.get_mut(b)) {
                    B_ABORTED.clone_into(&mut bt.state);
                    bt.ended = Some(SystemTime::now());
                }
                let e = self.end(
                    &mut ro,
                    id,
                    ABORTED,
                    &format!(
                        "aborted by a person during the gate of batch {}; what was applied stays",
                        b.saturating_add(1)
                    ),
                );
                drop(ro);
                return self.finish(e);
            }
            if !res.passed {
                return self.fail_rollout(id, b, &res.why);
            }
            self.after_batch(id, b);
            let pause = {
                let mut ro = lock(&self.0.ro);
                let f = ro.flags(id, run_no);
                let Some(r) = ro.get_mut(id) else { return };
                let canary = r.batches.get(b).is_some_and(|x| x.canary);
                if let Some(bt) = r.batches.get_mut(b) {
                    B_PASSED.clone_into(&mut bt.state);
                    bt.ended = Some(SystemTime::now());
                }
                r.batch = b.saturating_add(1);
                let more = r.batch < r.batches.len();
                r.count();
                if more && canary && !f.abort {
                    WAITING.clone_into(&mut r.state);
                    "the canary passed its gate: go on to apply the rest".clone_into(&mut r.reason);
                    drop(ro);
                    self.save_rollouts(Vec::new());
                    self.log(format!(
                        "cluster: the rollout {id}'s canary passed its gate; it waits for a person \
                         to go on"
                    ));
                    return;
                }
                let ms = r.strategy.as_ref().map_or(0, |s| s.pause_ms);
                let ms = u64::try_from(ms).unwrap_or(0);
                if more && ms > 0 && !f.abort && !f.pause {
                    let d = Duration::from_millis(ms);
                    r.next_at = SystemTime::now().checked_add(d);
                    Some(d)
                } else {
                    None
                }
            };
            self.save_rollouts(Vec::new());
            if let Some(d) = pause {
                if !self.wait_pause(id, run_no, d) {
                    return self.pause_for_stop(id);
                }
            }
            if let Some(r) = lock(&self.0.ro).get_mut(id) {
                r.next_at = None;
            }
        }
    }

    /// Waits `d` between batches, or until a person pauses or aborts: `false` when the manager
    /// stops. fit's `waitPause`.
    fn wait_pause(&self, id: &str, run_no: u64, d: Duration) -> bool {
        !matches!(self.wait_for(id, run_no, d, true), Wait::Stopped)
    }

    /// Waits `d`, or until the manager stops or a person aborts (or, `pause`, pauses).
    fn wait_for(&self, id: &str, run_no: u64, d: Duration, pause: bool) -> Wait {
        let start = Instant::now();
        let mut ro = lock(&self.0.ro);
        loop {
            if self.stopping() {
                return Wait::Stopped;
            }
            let f = ro.flags(id, run_no);
            if f.abort || (pause && f.pause) {
                return Wait::Aborted;
            }
            let left = d.saturating_sub(start.elapsed());
            if left.is_zero() {
                return Wait::Elapsed;
            }
            ro = self
                .0
                .ro_cv
                .wait_timeout(ro, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn pause_for_stop(&self, id: &str) {
        if let Some(r) = lock(&self.0.ro).get_mut(id) {
            PAUSED.clone_into(&mut r.state);
            "the manager stopped while it ran: it was paused where it stopped; resume it"
                .clone_into(&mut r.reason);
            r.next_at = None;
            r.count();
        }
        self.save_rollouts(Vec::new());
    }

    /// Applies batch `b` and gates it: fit's `runBatch`.
    fn run_batch(&self, id: &str, run_no: u64, b: usize) -> BatchResult {
        let Some((s, apply, nodes)) = lock(&self.0.ro).get_mut(id).map(|r| {
            let nodes = r
                .batches
                .get(b)
                .map(|x| x.nodes.clone())
                .unwrap_or_default();
            if let Some(bt) = r.batches.get_mut(b) {
                B_APPLYING.clone_into(&mut bt.state);
                bt.started = Some(SystemTime::now());
                bt.gate_at = None;
                bt.ended = None;
            }
            // Those applied before the manager restarted are gated, not applied again.
            let apply: Vec<RolloutMember> = nodes
                .iter()
                .filter_map(|n| r.member(n))
                .filter(|x| x.status == M_PENDING || x.status == M_APPLYING)
                .cloned()
                .collect();
            (r.strategy.clone().unwrap_or_default(), apply, nodes)
        }) else {
            return BatchResult::stopped();
        };
        self.save_rollouts(Vec::new());
        let gated = s.gate.max_5xx_ratio > 0.0;
        if gated {
            let bad = Mutex::new(BTreeMap::new());
            self.each(apply.clone(), |x| {
                let why = match self.get_sync(&x.node, "/status") {
                    Err(p) => format!("its GET /status could not be read: {}", p.message),
                    Ok(v) if requests(&v).is_some() => return,
                    Ok(_) => "it counts no requests (set metrics in its config), so its share \
                              of 5xx cannot be gated"
                        .to_owned(),
                };
                lock(&bad).insert(x.node.clone(), why);
            });
            if self.stopping() {
                return BatchResult::stopped();
            }
            let bad = bad.into_inner().unwrap_or_else(PoisonError::into_inner);
            if !bad.is_empty() {
                for (n, why) in &bad {
                    self.set_member(id, n, |x| {
                        M_FAILED.clone_into(&mut x.status);
                        x.error = Some(Fault::new(KIND_UNSUPPORTED, clip(why, MAX_MESSAGE)));
                    });
                }
                let names: Vec<&str> = bad.keys().map(String::as_str).collect();
                return BatchResult::failed(format!(
                    "the share of 5xx cannot be gated on {}: nothing of the batch was applied",
                    names.join(", ")
                ));
            }
        }
        self.each(apply, |x| {
            if lock(&self.0.ro).flags(id, run_no).abort {
                return; // left pending
            }
            self.apply_member(id, &x);
        });
        if self.stopping() {
            return BatchResult::stopped();
        }
        let (failed, gate, abort) = {
            let ro = lock(&self.0.ro);
            let abort = ro.flags(id, run_no).abort;
            let Some(r) = ro.get(id) else {
                return BatchResult::stopped();
            };
            let mut failed = Vec::new();
            let mut gate = Vec::new();
            for x in nodes.iter().filter_map(|n| r.member(n)) {
                if x.status == M_APPLIED {
                    gate.push(x.clone());
                } else if x.status == M_FAILED {
                    let why = x.error.as_ref().map_or("", |e| e.message.as_str());
                    failed.push(format!("{} ({why})", x.node));
                }
            }
            (failed, gate, abort)
        };
        let n = b.saturating_add(1);
        if !failed.is_empty() {
            return BatchResult::failed(format!(
                "batch {n} failed: {}",
                clip(&failed.join("; "), 768)
            ));
        }
        if abort {
            return BatchResult {
                aborted: true,
                ..BatchResult::default()
            };
        }
        if gate.is_empty() {
            return BatchResult {
                passed: true,
                ..BatchResult::default()
            };
        }
        let mut wait = s.gate.wait_ms;
        if gated && s.gate.window_ms > wait {
            wait = s.gate.window_ms;
        }
        let wait = Duration::from_millis(u64::try_from(wait).unwrap_or(0));
        if let Some(bt) = lock(&self.0.ro)
            .get_mut(id)
            .and_then(|r| r.batches.get_mut(b))
        {
            B_GATING.clone_into(&mut bt.state);
            bt.gate_at = SystemTime::now().checked_add(wait);
        }
        self.save_rollouts(Vec::new());
        let base = Mutex::new(HashMap::new());
        if gated {
            self.each(gate.clone(), |x| {
                if let Some(c) = self
                    .get_sync(&x.node, "/status")
                    .ok()
                    .and_then(|v| requests(&v))
                {
                    lock(&base).insert(x.node.clone(), c);
                }
            });
        }
        if !wait.is_zero() {
            match self.wait_for(id, run_no, wait, false) {
                Wait::Stopped => return BatchResult::stopped(),
                Wait::Aborted => {
                    return BatchResult {
                        aborted: true,
                        ..BatchResult::default()
                    };
                }
                Wait::Elapsed => {}
            }
        }
        let gates = Mutex::new(HashMap::new());
        self.each(gate.clone(), |x| {
            let before = lock(&base).get(&x.node).copied();
            let g = self.gate_member(&x, &s.gate, before);
            lock(&gates).insert(x.node.clone(), g);
        });
        if self.stopping() {
            return BatchResult::stopped(); // what failed then is the stop's
        }
        let mut gates = gates.into_inner().unwrap_or_else(PoisonError::into_inner);
        let mut fails = Vec::new();
        for x in &gate {
            let mut g = gates.remove(&x.node).unwrap_or_default();
            let reasons = g.reasons.join("; ");
            let passed = g.passed;
            self.set_member(id, &x.node, |y| {
                if let Some(old) = &y.gate {
                    g.drift_before = old.drift_before;
                }
                y.gate = Some(g);
                if !passed {
                    M_FAILED.clone_into(&mut y.status);
                    y.error = Some(Fault::new(
                        KIND_REFUSED,
                        clip(&format!("its gate failed: {reasons}"), MAX_MESSAGE),
                    ));
                }
            });
            if !passed {
                fails.push(format!("{} ({reasons})", x.node));
            }
        }
        if !fails.is_empty() {
            fails.sort();
            return BatchResult::failed(format!(
                "the gate of batch {n} failed: {}",
                clip(&fails.join("; "), 768)
            ));
        }
        if let Some(r) = lock(&self.0.ro).get_mut(id) {
            for x in &gate {
                if let Some(y) = r.member_mut(&x.node).filter(|y| y.status == M_APPLIED) {
                    M_DONE.clone_into(&mut y.status);
                }
            }
            r.count();
        }
        BatchResult {
            passed: true,
            ..BatchResult::default()
        }
    }

    /// Applies member `x`'s config with `If-Match` the ETag planned; a member whose answer was
    /// lost, or that refused it as changed, is settled by its `GET /node`'s ETag: fit's
    /// `applyMember`.
    fn apply_member(&self, id: &str, x: &RolloutMember) {
        self.set_member(id, &x.node, |y| M_APPLYING.clone_into(&mut y.status));
        let before = match self.caps_sync(&x.node) {
            Ok(c) if c.iter().any(|c| c == CAP_CHECK) => self
                .get_sync(&x.node, "/config/check")
                .ok()
                .map(|v| v.get("summary").map_or(0, |s| json::int_of(s, "drift"))),
            _ => None,
        };
        let mut h = Vec::new();
        if !x.config_etag.is_empty() {
            h.push(("If-Match".to_owned(), x.config_etag.clone()));
        }
        let r = self.call_sync(
            &x.node,
            "PUT",
            "/config",
            h,
            Some(x.config.clone().unwrap_or_default()),
        );
        if self.stopping() {
            return; // as it is: a restart finds out by its ETag
        }
        let set = |f: &dyn Fn(&mut RolloutMember)| {
            self.set_member(id, &x.node, |y| {
                if before.is_some() {
                    y.gate = Some(GateResult {
                        drift_before: before,
                        ..GateResult::default()
                    });
                }
                f(y);
            });
        };
        let applied = match &r {
            Ok(rep) if ok(rep.status) => json::parse(&rep.body)
                .map(|a| {
                    (
                        json::uint_of(&a, "version"),
                        json::strings_of(&a, "warnings"),
                    )
                })
                .filter(|(v, _)| *v != 0),
            _ => None,
        };
        match (&r, applied) {
            (_, Some((v, w))) => set(&|y| {
                M_APPLIED.clone_into(&mut y.status);
                y.applied = v;
                y.error = None;
                y.warnings.extend(clip_list(&w, MAX_ERRORS, 256));
                y.warnings.truncate(MAX_ERRORS);
            }),
            (Ok(rep), None) if rep.status != 412 && !ok(rep.status) => {
                let p = Fault::of(&reply_problem(rep, "its apply"));
                let errs = config_errors(&rep.body);
                set(&|y| {
                    M_FAILED.clone_into(&mut y.status);
                    y.error = Some(p.clone());
                    y.errors.clone_from(&errs);
                });
            }
            _ => match self.node_config(&x.node) {
                Err(p) => {
                    let mut why = "its apply was not answered".to_owned();
                    if let Err(e) = &r {
                        why.push_str(&format!(" ({})", e.message));
                    }
                    let msg = clip(
                        &format!(
                            "{why}, and its GET /node could not be read ({}): whether it runs the \
                             rollout's config is not known",
                            p.message
                        ),
                        MAX_MESSAGE,
                    );
                    set(&|y| {
                        M_FAILED.clone_into(&mut y.status);
                        y.unsure = true;
                        y.error = Some(Fault::new(KIND_UNREACHABLE, msg.clone()));
                    });
                }
                Ok((v, etag)) if etag == x.new_etag => set(&|y| {
                    M_APPLIED.clone_into(&mut y.status);
                    y.applied = v;
                    y.error = None;
                }),
                Ok((v, etag)) if matches!(&r, Ok(rep) if rep.status == 412) => {
                    let msg = clip(
                        &format!(
                            "its config changed since the plan (it runs version {v}, ETag \
                             {etag}; the plan read {}): it was not overwritten",
                            x.config_etag
                        ),
                        MAX_MESSAGE,
                    );
                    set(&|y| {
                        M_SKIPPED.clone_into(&mut y.status);
                        y.error = Some(Fault::status(KIND_REFUSED, 412, msg.clone()));
                    });
                }
                Ok(_) => {
                    let p = match &r {
                        Err(p) => Fault::of(p),
                        Ok(rep) => Fault::status(
                            KIND_BAD_ANSWER,
                            rep.status,
                            "its apply's answer is not the JSON expected",
                        ),
                    };
                    set(&|y| {
                        M_FAILED.clone_into(&mut y.status);
                        y.error = Some(p.clone());
                    });
                }
            },
        }
        self.repoll(&x.node);
    }

    /// Has the poller read member `name` at once.
    fn repoll(&self, name: &str) {
        if let Some(n) = self.node(name) {
            n.next.store(0, Ordering::Release);
        }
        self.poke();
    }

    /// Member `x`'s gate: reachable, running the config applied, no more drift than before
    /// where it checks its config, its share of 5xx in the window: fit's `gateMember`.
    fn gate_member(&self, x: &RolloutMember, g: &Gate, before: Option<(u64, u64)>) -> GateResult {
        let mut res = GateResult {
            checked: Some(SystemTime::now()),
            ..GateResult::default()
        };
        let fail = |res: &mut GateResult, why: String| {
            if res.reasons.len() < MAX_GATE_REASONS {
                res.reasons.push(clip(&why, 256));
            }
        };
        match self.node_config(&x.node) {
            Err(p) => {
                fail(&mut res, format!("it is not reachable: {}", p.message));
                return res;
            }
            Ok((v, etag)) => {
                res.version = v;
                if etag != x.new_etag {
                    fail(
                        &mut res,
                        format!(
                            "it runs another config now (version {v}, ETag {etag}), not the one \
                             the rollout applied"
                        ),
                    );
                }
            }
        }
        // The drift gate, only where the member checks its config (fit's config.check).
        if let Some(db) = x.gate.as_ref().and_then(|g| g.drift_before) {
            match self.get_sync(&x.node, "/config/check") {
                Err(p) => fail(
                    &mut res,
                    format!("its consistency check could not be read: {}", p.message),
                ),
                Ok(v) => {
                    let d = v.get("summary").map_or(0, |s| json::int_of(s, "drift"));
                    res.drift_after = Some(d);
                    if d > db {
                        fail(
                            &mut res,
                            format!(
                                "its consistency check finds {d} differences, {db} before the \
                                 rollout"
                            ),
                        );
                    }
                }
            }
        }
        if g.max_5xx_ratio > 0.0 {
            match self.get_sync(&x.node, "/status") {
                Err(p) => fail(
                    &mut res,
                    format!("its GET /status could not be read: {}", p.message),
                ),
                Ok(v) => match (requests(&v), before) {
                    (None, _) => fail(
                        &mut res,
                        "it counts no requests now (set metrics in its config)".to_owned(),
                    ),
                    (_, None) => fail(
                        &mut res,
                        "its requests could not be counted when the batch was applied".to_owned(),
                    ),
                    (Some((t, e)), Some((bt, be))) if t < bt || e < be => fail(
                        &mut res,
                        "its counts of requests went back: it restarted in the window".to_owned(),
                    ),
                    (Some((t, e)), Some((bt, be))) => {
                        let (total, errs) = (t.saturating_sub(bt), e.saturating_sub(be));
                        res.requests = Some(total);
                        res.errors_5xx = Some(errs);
                        let min = u64::try_from(g.min_requests).unwrap_or(0);
                        if total < min {
                            res.notes.push(format!(
                                "{total} requests in the window, fewer than {min}: its share of \
                                 5xx is not judged"
                            ));
                        } else {
                            #[allow(clippy::cast_precision_loss)]
                            let ratio = errs as f64 / total as f64;
                            res.ratio = Some(ratio);
                            if ratio > g.max_5xx_ratio {
                                fail(
                                    &mut res,
                                    format!(
                                        "{errs} of its {total} requests in the window were 5xx \
                                         ({:.1}%), more than {:.1}%",
                                        ratio * 100.0,
                                        g.max_5xx_ratio * 100.0
                                    ),
                                );
                            }
                        }
                    }
                },
            }
        }
        res.passed = res.reasons.is_empty();
        res
    }

    /// The members of batch `b` done are told what the manager owns there: fit's `afterBatch`.
    fn after_batch(&self, id: &str, b: usize) {
        let names: Vec<String> = lock(&self.0.ro)
            .get(id)
            .map(|r| {
                r.batches
                    .get(b)
                    .map(|bt| bt.nodes.clone())
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|n| r.member(n).is_some_and(|x| x.status == M_DONE))
                    .collect()
            })
            .unwrap_or_default();
        if names.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::sync_channel(1);
        self.run_sync(
            names,
            true,
            Arc::new(|_| {}),
            Box::new(move || {
                let _ = tx.send(());
            }),
        );
        let _ = rx.recv();
    }

    /// Ends a rollout whose batch `b` failed, rolling back that batch, or every member applied:
    /// fit's `failRollout`.
    fn fail_rollout(&self, id: &str, b: usize, why: &str) {
        let Some((all, back)) = lock(&self.0.ro).get_mut(id).map(|r| {
            let nodes = r
                .batches
                .get(b)
                .map(|x| x.nodes.clone())
                .unwrap_or_default();
            if let Some(bt) = r.batches.get_mut(b) {
                B_FAILED.clone_into(&mut bt.state);
                bt.ended = Some(SystemTime::now());
            }
            let all = r
                .strategy
                .as_ref()
                .is_some_and(|s| s.on_failure == ON_FAILURE_ALL);
            let back: Vec<RolloutMember> = r
                .members
                .iter()
                .filter(|x| x.to_roll_back() && (all || nodes.contains(&x.node)))
                .cloned()
                .collect();
            (all, back)
        }) else {
            return;
        };
        self.save_rollouts(Vec::new());
        let failed = self.roll_back(id, back.clone());
        let mut ro = lock(&self.0.ro);
        if let Some(r) = ro.get_mut(id) {
            let stuck: Vec<String> = r
                .members
                .iter()
                .filter(|x| x.status == M_ROLLBACK_FAILED)
                .map(|x| x.node.clone())
                .collect();
            for (i, c) in r.batches.iter_mut().enumerate() {
                let mine = i == b || (all && c.state == B_PASSED);
                if mine && !back.is_empty() && !c.nodes.iter().any(|n| stuck.contains(n)) {
                    B_ROLLED_BACK.clone_into(&mut c.state);
                }
            }
        }
        let scope = if all {
            "every member applied"
        } else {
            "its batch"
        };
        let why = if back.is_empty() {
            format!("{why}; nothing was applied to roll back")
        } else if failed > 0 {
            format!("{why}; {scope} rolled back, but {failed} members could not be")
        } else {
            format!("{why}; {scope} rolled back")
        };
        let e = self.end(&mut ro, id, FAILED, &why);
        drop(ro);
        self.finish(e);
    }

    /// Rolls back every member of rollout `id` that may run a config of its: a person's
    /// rollback, fit's `rollBackAll`.
    fn roll_back_all(&self, id: &str) {
        let back: Vec<RolloutMember> = lock(&self.0.ro)
            .get(id)
            .map(|r| {
                r.members
                    .iter()
                    .filter(|x| x.to_roll_back())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let failed = self.roll_back(id, back);
        let mut ro = lock(&self.0.ro);
        if self.stopping() {
            if let Some(r) = ro.get_mut(id) {
                FAILED.clone_into(&mut r.state);
                "the manager stopped while it rolled back: roll it back again"
                    .clone_into(&mut r.reason);
                r.ended = Some(SystemTime::now());
                r.count();
            }
            drop(ro);
            return self.save_rollouts(Vec::new());
        }
        let Some(r) = ro.get_mut(id) else { return };
        for c in &mut r.batches {
            if c.state == B_PASSED || c.state == B_FAILED {
                B_ROLLED_BACK.clone_into(&mut c.state);
            }
        }
        let mut why = r.reason.clone();
        if failed > 0 {
            why.push_str(&format!("; {failed} members could not be rolled back"));
        }
        let e = self.end(&mut ro, id, ROLLED_BACK, &why);
        drop(ro);
        self.finish(e);
    }

    /// Rolls back `xs`: how many could not be. fit's `rollBack`.
    fn roll_back(&self, id: &str, xs: Vec<RolloutMember>) -> usize {
        let failed = Mutex::new(0usize);
        self.each(xs, |x| {
            if !self.roll_back_member(id, &x) && !self.stopping() {
                let mut n = lock(&failed);
                *n = n.saturating_add(1);
            }
        });
        failed.into_inner().unwrap_or_else(PoisonError::into_inner)
    }

    /// Rolls member `x` back to the version it ran when planned, `If-Match` the config the
    /// rollout applied: fit's `rollBackMember`.
    fn roll_back_member(&self, id: &str, x: &RolloutMember) -> bool {
        let mut h = Vec::new();
        if !x.new_etag.is_empty() {
            h.push(("If-Match".to_owned(), x.new_etag.clone()));
        }
        let r = self.call_sync(
            &x.node,
            "POST",
            &format!("/rollback/{}", x.version),
            h,
            Some(Vec::new()),
        );
        if self.stopping() {
            return false;
        }
        let set = |f: &dyn Fn(&mut RolloutMember)| self.set_member(id, &x.node, f);
        let done = match r {
            Err(p) => {
                let f = Fault::of(&p);
                set(&|y| {
                    M_ROLLBACK_FAILED.clone_into(&mut y.status);
                    y.error = Some(f.clone());
                });
                false
            }
            Ok(rep) if ok(rep.status) && json::parse(&rep.body).is_some() => {
                let v = json::parse(&rep.body).map_or(0, |a| json::uint_of(&a, "version"));
                set(&|y| {
                    M_ROLLED_BACK.clone_into(&mut y.status);
                    y.rolled_back = v;
                    y.unsure = false;
                });
                true
            }
            Ok(rep) if rep.status == 412 => match self.node_config(&x.node) {
                Ok((_, etag)) if x.unsure && etag == x.config_etag => {
                    // Its apply never landed: nothing to roll back.
                    set(&|y| y.unsure = false);
                    true
                }
                _ => {
                    set(&|y| {
                        M_ROLLBACK_FAILED.clone_into(&mut y.status);
                        y.error = Some(Fault::status(
                            KIND_REFUSED,
                            412,
                            "its config changed since the rollout applied it: it was not rolled \
                             back",
                        ));
                    });
                    false
                }
            },
            Ok(rep) if rep.status == 404 => {
                let msg = format!("it keeps no version {} any more to roll back to", x.version);
                set(&|y| {
                    M_ROLLBACK_FAILED.clone_into(&mut y.status);
                    y.error = Some(Fault::status(KIND_NO_EARLIER, 404, msg.clone()));
                });
                false
            }
            Ok(rep) => {
                let f = Fault::of(&reply_problem(&rep, "its rollback"));
                set(&|y| {
                    M_ROLLBACK_FAILED.clone_into(&mut y.status);
                    y.error = Some(f.clone());
                });
                false
            }
        };
        self.repoll(&x.node);
        done
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    fn v(s: &str) -> Value {
        json::parse(s.as_bytes()).unwrap()
    }

    #[test]
    fn changes_between_configs() {
        let have = v(
            r#"{"log": {"level": "info"}, "routes": [{"name": "a", "x": 1}, {"name": "b"}], "tls": {"key": "k"}}"#,
        );
        let want = v(
            r#"{"log": {"level": "info"}, "routes": [{"name": "a", "x": 2}, {"name": "c"}], "tls": {"key": "k2"}}"#,
        );
        let c = rollout_changes(&have, &want, None);
        let got: Vec<(&str, &str)> = c
            .iter()
            .map(|c| (c.path.as_str(), c.state.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("routes[a]", "differs"),
                ("routes[b]", "only_a"),
                ("tls", "differs"),
                ("routes[c]", "added")
            ]
        );
        // Secrets masked in what is shown.
        let tls = c.iter().find(|c| c.path == "tls").unwrap();
        assert!(!tls.want.as_deref().unwrap().contains("k2"), "{tls:?}");
    }

    #[test]
    fn bodies_and_answers() {
        assert_eq!(
            pretty(&v(r#"{"a": [1, {"b": {}}], "c": []}"#)),
            "{\n  \"a\": [\n    1,\n    {\n      \"b\": {}\n    }\n  ],\n  \"c\": []\n}\n"
        );
        let errs = config_errors(br#"{"errors": [{"path": "routes[0]", "message": "bad"}]}"#);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].path, "routes[0]");
        assert_eq!(
            requests(&v(
                r#"{"requests": {"total": 10, "classes": {"5xx": 3, "2xx": 7}}}"#
            )),
            Some((10, 3))
        );
        assert_eq!(requests(&v(r#"{"uptime": 1}"#)), None);
        let parts = fragment_parts(&v(r#"{"tls": {"key": "[secret]"}}"#));
        let own = with_own_secrets(&parts, &v(r#"{"tls": {"key": "mine"}}"#)).unwrap();
        assert_eq!(json::compact(&own[0].value), r#"{"key":"mine"}"#);
        assert!(with_own_secrets(&parts, &v("{}")).is_err());
    }
}
