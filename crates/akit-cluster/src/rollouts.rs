//! Rollouts with health gates (I137, fit's `cluster/rollouts.go`, `rollsteer.go`, `rolldraft.go`
//! and `rollapi.go`, its I146): a change to many members' configs planned first, each member's
//! diff and dry run read with its ETag, then applied all at once, in rolling batches or a canary
//! first, each batch gated (the member reachable and running the config applied, no more drift
//! than before where it checks its config, a share of 5xx under a threshold over a window) and
//! rolled back when its gate fails.
//!
//! A rollout is of a template (or a draft of one, saved when it starts), of a sync with the
//! templates, of a copy of parts of one member's config to others, or of a patch of parts. Each
//! member's config is replaced whole, with `If-Match` the ETag the plan read: a member changed
//! since is skipped, never overwritten; one whose answer was lost is settled by its `GET /node`'s
//! ETag. A rollout holds its members while it runs, as an operation does (one at a time on a
//! member); it is paused, resumed, gone on with after its canary, aborted, rolled back or, as a
//! plan, discarded; it is kept in the store's document, `rollouts`, and a manager restarted finds
//! one that ran paused where it stopped.
//!
//! Bounds: 4 rollouts planned or under way at once, 50 kept, 8 MiB of configs and diffs a
//! rollout, 20 errors a member, a plan expired after an hour. A plan and a run each take a
//! thread of their own (never an I/O thread), which calls members on akio's asynchronous client
//! and waits for their answers, `max_concurrency` members at once; the pause between batches and
//! a gate's window are waited for there, woken by a person's pause or abort (rule: timers only
//! where the logic is about time).
//!
//! ```text
//! GET    /api/rollouts                   the rollouts, newest first, with the bounds
//! POST   /api/rollouts                   a rollout planned: 202, planning; read it for its plan
//! GET    /api/rollouts/<id>              a rollout, with each member's changes
//! DELETE /api/rollouts/<id>              a plan discarded
//! POST   /api/rollouts/<id>/start        a plan started, with its strategy and exclusions
//! POST   /api/rollouts/<id>/<action>     pause, resume, go, abort or rollback
//! ```

// A member's problem is part of a rollout, made once per member: its size does not matter.
#![allow(clippy::result_large_err)]

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::Ordering;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use akio_http::{Context, Method, Response};
use akit_config::value::Value;
use akit_roles::pages::{fail, json as answer};

use crate::api::clip;
use crate::json::{self, Obj, quoted};
use crate::jv;
use crate::member::{Document, FieldError, MAX_BODY_BYTES};
use crate::sync::Rendered;
use crate::templates::{self, MAX_TEMPLATE_BYTES, MAX_TEMPLATE_PARTS, SECRET_MASK};
use crate::transport::Problem;
use crate::{Manager, Refusal};

/// A rollout of a template's config, or of a draft of it.
pub const KIND_TEMPLATE: &str = "template";
/// A rollout bringing members in sync with their templates.
pub const KIND_SYNC: &str = "sync";
/// A rollout of parts of one member's config copied to others.
pub const KIND_COPY: &str = "copy";
/// A rollout of parts written for it.
pub const KIND_PATCH: &str = "patch";

/// A rollout's states: fit's `Rollout*`.
pub const PLANNING: &str = "planning";
/// See [`PLANNING`].
pub const PLANNED: &str = "planned";
/// See [`PLANNING`].
pub const RUNNING: &str = "running";
/// See [`PLANNING`].
pub const PAUSED: &str = "paused";
/// See [`PLANNING`]: its canary passed, and it waits for a person to go on.
pub const WAITING: &str = "waiting";
/// See [`PLANNING`].
pub const ROLLING_BACK: &str = "rolling_back";
/// See [`PLANNING`].
pub const DONE: &str = "done";
/// See [`PLANNING`].
pub const FAILED: &str = "failed";
/// See [`PLANNING`].
pub const ABORTED: &str = "aborted";
/// See [`PLANNING`].
pub const ROLLED_BACK: &str = "rolled_back";
/// See [`PLANNING`].
pub const EXPIRED: &str = "expired";

/// A member's states in a rollout: fit's `Member*`.
pub const M_PLANNING: &str = "planning";
/// See [`M_PLANNING`].
pub const M_READY: &str = "ready";
/// See [`M_PLANNING`].
pub const M_UNCHANGED: &str = "unchanged";
/// See [`M_PLANNING`].
pub const M_ERROR: &str = "error";
/// See [`M_PLANNING`].
pub const M_EXCLUDED: &str = "excluded";
/// See [`M_PLANNING`].
pub const M_PENDING: &str = "pending";
/// See [`M_PLANNING`].
pub const M_APPLYING: &str = "applying";
/// See [`M_PLANNING`].
pub const M_APPLIED: &str = "applied";
/// See [`M_PLANNING`].
pub const M_DONE: &str = "done";
/// See [`M_PLANNING`].
pub const M_FAILED: &str = "failed";
/// See [`M_PLANNING`]: its config changed since the plan, and was not overwritten.
pub const M_SKIPPED: &str = "skipped";
/// See [`M_PLANNING`].
pub const M_ROLLED_BACK: &str = "rolled_back";
/// See [`M_PLANNING`].
pub const M_ROLLBACK_FAILED: &str = "rollback_failed";

/// The strategies.
pub const ALL: &str = "all";
/// See [`ALL`].
pub const ROLLING: &str = "rolling";
/// See [`ALL`].
pub const CANARY: &str = "canary";

/// What a failed gate rolls back: its batch, or every member applied.
pub const ON_FAILURE_BATCH: &str = "batch";
/// See [`ON_FAILURE_BATCH`].
pub const ON_FAILURE_ALL: &str = "all";

/// A batch's states.
pub const B_PENDING: &str = "pending";
/// See [`B_PENDING`].
pub const B_APPLYING: &str = "applying";
/// See [`B_PENDING`].
pub const B_GATING: &str = "gating";
/// See [`B_PENDING`].
pub const B_PASSED: &str = "passed";
/// See [`B_PENDING`].
pub const B_FAILED: &str = "failed";
/// See [`B_PENDING`].
pub const B_ROLLED_BACK: &str = "rolled_back";
/// See [`B_PENDING`].
pub const B_ABORTED: &str = "aborted";

/// A member held by another rollout or an operation.
pub const KIND_BUSY: &str = "busy";

/// How many rollouts are planned or under way at once; one more is answered 429.
pub const MAX_ACTIVE: usize = 4;
/// How many rollouts are kept, the oldest ended dropped first.
pub const MAX_KEPT: usize = 50;
/// The bytes of members' configs and diffs a rollout keeps at most.
pub const MAX_ROLLOUT_BYTES: usize = 8 << 20;
/// The most errors and warnings kept of a member.
pub const MAX_ERRORS: usize = 20;
/// How long a plan waits to be started.
pub const PLAN_TTL: Duration = Duration::from_secs(3600);
const MAX_DESCRIPTION: usize = 512;
pub(crate) const MAX_GATE_REASONS: usize = 8;

/// A gate's defaults: its wait, and the window and the fewest requests of a share of 5xx.
pub const DEFAULT_GATE_WAIT_MS: i64 = 5000;
/// See [`DEFAULT_GATE_WAIT_MS`].
pub const DEFAULT_GATE_WINDOW_MS: i64 = 30_000;
/// See [`DEFAULT_GATE_WAIT_MS`].
pub const DEFAULT_MIN_REQUESTS: i64 = 20;
const MAX_GATE_WAIT_MS: i64 = 10 * 60 * 1000;
const MAX_GATE_WINDOW_MS: i64 = 30 * 60 * 1000;
const MAX_PAUSE_MS: i64 = 24 * 60 * 60 * 1000;
const MAX_GATE_MIN_REQUESTS: i64 = 1_000_000_000;

/// The capabilities a member lists for rollouts.
pub const CAP_IF_MATCH: &str = "config.if_match";
/// See [`CAP_IF_MATCH`].
pub const CAP_DRY_RUN: &str = "config.dry_run";
/// See [`CAP_IF_MATCH`].
pub const CAP_VERSIONS: &str = "config.versions";

/// The audit log's actions.
pub const ACTION_PLAN: &str = "rollout.plan";
/// See [`ACTION_PLAN`].
pub const ACTION_START: &str = "rollout.start";
/// See [`ACTION_PLAN`].
pub const ACTION_END: &str = "rollout.end";

/// How a rollout is steered.
pub const STEER_PAUSE: &str = "pause";
/// See [`STEER_PAUSE`].
pub const STEER_RESUME: &str = "resume";
/// See [`STEER_PAUSE`].
pub const STEER_GO: &str = "go";
/// See [`STEER_PAUSE`].
pub const STEER_ABORT: &str = "abort";
/// See [`STEER_PAUSE`].
pub const STEER_ROLLBACK: &str = "rollback";
/// See [`STEER_PAUSE`].
pub const STEER_DISCARD: &str = "discard";

/// What akit's API answers a secret as (decision 035).
const REDACTED: &str = "[redacted]";

fn field(f: &str, msg: impl Into<String>) -> Refusal {
    Refusal::Field(FieldError {
        field: f.to_owned(),
        msg: msg.into(),
    })
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn printable(s: &str) -> bool {
    s.chars().all(|c| c >= ' ' && c != '\u{7f}')
}

/// RFC 3339 read back, as Go writes `time.Time`: `None` for Go's zero time, or what does not read.
pub(crate) fn parse_time(s: &str) -> Option<SystemTime> {
    let b = s.as_bytes();
    let num = |from: usize, to: usize| -> Option<i64> {
        let t = s.get(from..to)?;
        if t.bytes().all(|c| c.is_ascii_digit()) {
            t.parse().ok()
        } else {
            None
        }
    };
    if b.len() < 20 || b.get(4) != Some(&b'-') || b.get(10) != Some(&b'T') {
        return None;
    }
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, sec) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut i = 19;
    let mut nanos: i64 = 0;
    if b.get(i) == Some(&b'.') {
        let start = i.saturating_add(1);
        let mut end = start;
        while b.get(end).is_some_and(u8::is_ascii_digit) {
            end = end.saturating_add(1);
        }
        let digits = s.get(start..end)?;
        let mut n: i64 = 0;
        for (k, c) in digits.bytes().enumerate() {
            if k < 9 {
                n = n
                    .checked_mul(10)?
                    .checked_add(i64::from(c.checked_sub(b'0')?))?;
            }
        }
        for _ in digits.len()..9 {
            n = n.checked_mul(10)?;
        }
        nanos = n;
        i = end;
    }
    let offset = match b.get(i) {
        Some(b'Z') => 0,
        Some(c @ (b'+' | b'-')) => {
            let o = num(i.saturating_add(1), i.saturating_add(3))?
                .checked_mul(3600)?
                .checked_add(num(i.saturating_add(4), i.saturating_add(6))?.checked_mul(60)?)?;
            if *c == b'-' { o.checked_neg()? } else { o }
        }
        _ => return None,
    };
    // Days from the civil date (Howard Hinnant's algorithm).
    let yy = if mo <= 2 { y.checked_sub(1)? } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy.checked_sub(era.checked_mul(400)?)?;
    let mp = if mo > 2 {
        mo.checked_sub(3)?
    } else {
        mo.checked_add(9)?
    };
    let doy = mp
        .checked_mul(153)?
        .checked_add(2)?
        .checked_div(5)?
        .checked_add(d)?
        .checked_sub(1)?;
    let doe = yoe
        .checked_mul(365)?
        .checked_add(yoe / 4)?
        .checked_sub(yoe / 100)?
        .checked_add(doy)?;
    let days = era
        .checked_mul(146_097)?
        .checked_add(doe)?
        .checked_sub(719_468)?;
    let secs = days
        .checked_mul(86_400)?
        .checked_add(h.checked_mul(3600)?)?
        .checked_add(mi.checked_mul(60)?)?
        .checked_add(sec)?
        .checked_sub(offset)?;
    let secs = u64::try_from(secs).ok()?;
    UNIX_EPOCH.checked_add(Duration::new(secs, u32::try_from(nanos).ok()?))
}

/// A float as Go's `encoding/json` writes it.
pub(crate) fn float(f: f64) -> String {
    let a = f.abs();
    if a == 0.0 || (1e-6..1e21).contains(&a) {
        return format!("{f}");
    }
    let s = format!("{f:e}");
    match s.split_once('e') {
        Some((m, e)) => {
            let (sign, digits) = e.strip_prefix('-').map_or(("+", e), |d| ("-", d));
            format!("{m}e{sign}{digits:0>2}")
        }
        None => s,
    }
}

/// A duration of `ms` as Go's `time.Duration` says itself: `500ms`, `1.5s`, `1m0s`, `1h0m0s`.
pub(crate) fn go_duration(ms: i64) -> String {
    if ms == 0 {
        return "0s".to_owned();
    }
    let neg = if ms < 0 { "-" } else { "" };
    let ms = ms.unsigned_abs();
    if ms < 1000 {
        return format!("{neg}{ms}ms");
    }
    let (secs, frac) = (ms / 1000, ms % 1000);
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    let mut out = neg.to_owned();
    if h > 0 {
        out.push_str(&format!("{h}h"));
    }
    if h > 0 || m > 0 {
        out.push_str(&format!("{m}m"));
    }
    out.push_str(&s.to_string());
    if frac > 0 {
        let f = format!("{frac:03}");
        out.push('.');
        out.push_str(f.trim_end_matches('0'));
    }
    out.push('s');
    out
}

fn time_or(o: Obj, key: &str, t: Option<SystemTime>) -> Obj {
    match t {
        Some(t) => o.raw(key, &json::time(t)),
        None => o,
    }
}

fn time_of(v: &Value, key: &str) -> Option<SystemTime> {
    parse_time(json::str_of(v, key))
}

fn list_of(v: &Value, key: &str) -> Vec<Value> {
    match v.get(key) {
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    }
}

/// A whole number of member `key`, or 0 when it is absent.
fn whole(v: &Value, key: &str, name: &str) -> Result<i64, Refusal> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(0),
        Some(Value::Number(n)) => n
            .parse()
            .map_err(|_| field(name, format!("{name} is a whole number"))),
        Some(_) => Err(field(name, format!("{name} is a whole number"))),
    }
}

/// A member's problem kept in a rollout: its kind and message, and the JSON it is answered as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    /// Its kind.
    pub kind: String,
    /// What happened.
    pub message: String,
    json: String,
}

impl Fault {
    /// `p`, kept.
    #[must_use]
    pub fn of(p: &Problem) -> Fault {
        Fault {
            kind: p.kind.to_owned(),
            message: p.message.clone(),
            json: p.json(),
        }
    }

    /// A problem of `kind`, now.
    #[must_use]
    pub fn new(kind: &'static str, message: impl Into<String>) -> Fault {
        Fault::of(&Problem::new(kind, message))
    }

    /// With the member's status.
    #[must_use]
    pub fn status(kind: &'static str, status: u16, message: impl Into<String>) -> Fault {
        Fault::of(&Problem::new(kind, message).with_status(status))
    }

    fn from_value(v: &Value) -> Fault {
        Fault {
            kind: json::str_of(v, "kind").to_owned(),
            message: json::str_of(v, "message").to_owned(),
            json: json::compact(v),
        }
    }
}

/// A batch's health gate: fit's `Gate`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Gate {
    /// How long after a batch is applied its members are checked.
    pub wait_ms: i64,
    /// The share of 5xx a member may answer in the window; 0 does not gate on it.
    pub max_5xx_ratio: f64,
    /// The window the share is counted over.
    pub window_ms: i64,
    /// The fewest requests in the window the share is judged from.
    pub min_requests: i64,
}

impl Gate {
    fn json(&self) -> String {
        let mut o = Obj::new().num("wait_ms", self.wait_ms);
        if self.max_5xx_ratio > 0.0 {
            o = o.raw("max_5xx_ratio", &float(self.max_5xx_ratio));
        }
        if self.window_ms != 0 {
            o = o.num("window_ms", self.window_ms);
        }
        if self.min_requests != 0 {
            o = o.num("min_requests", self.min_requests);
        }
        o.end()
    }
}

/// How a rollout applies its members: fit's `Strategy`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Strategy {
    /// [`ALL`], [`ROLLING`] or [`CANARY`].
    pub mode: String,
    /// A batch's members, or
    pub batch_size: i64,
    /// a batch's share of the members, in percent.
    pub batch_percent: i64,
    /// The pause between batches.
    pub pause_ms: i64,
    /// The canary's members.
    pub canary: i64,
    /// [`ON_FAILURE_BATCH`] or [`ON_FAILURE_ALL`].
    pub on_failure: String,
    /// The gate after each batch.
    pub gate: Gate,
}

impl Strategy {
    /// Reads a strategy sent.
    ///
    /// # Errors
    ///
    /// A number that is not.
    pub fn from_value(v: &Value) -> Result<Strategy, Refusal> {
        let g = v.get("gate").cloned().unwrap_or(Value::Null);
        let ratio = match g.get("max_5xx_ratio") {
            None | Some(Value::Null) => 0.0,
            Some(Value::Number(n)) => n.parse::<f64>().map_err(|_| {
                field(
                    "strategy.gate.max_5xx_ratio",
                    "the share of 5xx is a number",
                )
            })?,
            Some(_) => {
                return Err(field(
                    "strategy.gate.max_5xx_ratio",
                    "the share of 5xx is a number",
                ));
            }
        };
        Ok(Strategy {
            mode: json::str_of(v, "mode").to_owned(),
            batch_size: whole(v, "batch_size", "strategy.batch_size")?,
            batch_percent: whole(v, "batch_percent", "strategy.batch_percent")?,
            pause_ms: whole(v, "pause_ms", "strategy.pause_ms")?,
            canary: whole(v, "canary", "strategy.canary")?,
            on_failure: json::str_of(v, "on_failure").to_owned(),
            gate: Gate {
                wait_ms: whole(&g, "wait_ms", "strategy.gate.wait_ms")?,
                max_5xx_ratio: ratio,
                window_ms: whole(&g, "window_ms", "strategy.gate.window_ms")?,
                min_requests: whole(&g, "min_requests", "strategy.gate.min_requests")?,
            },
        })
    }

    fn json(&self) -> String {
        let mut o = Obj::new().str("mode", &self.mode);
        for (k, n) in [
            ("batch_size", self.batch_size),
            ("batch_percent", self.batch_percent),
            ("pause_ms", self.pause_ms),
            ("canary", self.canary),
        ] {
            if n != 0 {
                o = o.num(k, n);
            }
        }
        o.str_omit("on_failure", &self.on_failure)
            .raw("gate", &self.gate.json())
            .end()
    }

    /// Checks it for `n` members, filling in the defaults: fit's `Strategy.check`.
    ///
    /// # Errors
    ///
    /// The field refused.
    pub fn check(&mut self, n: usize) -> Result<(), Refusal> {
        let n = i64::try_from(n).unwrap_or(i64::MAX);
        match self.mode.as_str() {
            ALL => {
                if self.batch_size != 0
                    || self.batch_percent != 0
                    || self.canary != 0
                    || self.pause_ms != 0
                {
                    return Err(field(
                        "strategy.mode",
                        "all at once takes no batches, canary or pause",
                    ));
                }
            }
            ROLLING | CANARY => {
                let rolling = self.mode == ROLLING;
                if self.batch_size < 0 || self.batch_size > n {
                    return Err(field(
                        "strategy.batch_size",
                        format!("a batch is 1 to {n} members"),
                    ));
                }
                if self.batch_percent < 0 || self.batch_percent > 100 {
                    return Err(field(
                        "strategy.batch_percent",
                        "a batch is 1 to 100 percent of the members",
                    ));
                }
                if self.batch_size > 0 && self.batch_percent > 0 {
                    return Err(field(
                        "strategy.batch_size",
                        "a batch is given by its size or its percentage, not both",
                    ));
                }
                if rolling && self.batch_size == 0 && self.batch_percent == 0 {
                    return Err(field(
                        "strategy.batch_size",
                        "a rolling rollout's batches are given by their size or percentage",
                    ));
                }
                if rolling && self.canary != 0 {
                    return Err(field(
                        "strategy.canary",
                        "only a canary rollout has a canary",
                    ));
                }
                if !rolling && (self.canary < 1 || self.canary >= n) {
                    if n < 2 {
                        return Err(field(
                            "strategy.mode",
                            "a canary needs two members at least: one to go first, and the rest",
                        ));
                    }
                    return Err(field(
                        "strategy.canary",
                        format!(
                            "the canary is 1 to {} members, fewer than all {n}",
                            n.saturating_sub(1)
                        ),
                    ));
                }
                if self.pause_ms < 0 || self.pause_ms > MAX_PAUSE_MS {
                    return Err(field(
                        "strategy.pause_ms",
                        format!("the pause between batches is 0 to {MAX_PAUSE_MS} ms"),
                    ));
                }
            }
            m => {
                return Err(field(
                    "strategy.mode",
                    format!(
                        "{} is not a strategy: {ALL}, {ROLLING} or {CANARY}",
                        quoted(m)
                    ),
                ));
            }
        }
        match self.on_failure.as_str() {
            "" => ON_FAILURE_BATCH.clone_into(&mut self.on_failure),
            ON_FAILURE_BATCH | ON_FAILURE_ALL => {}
            o => {
                return Err(field(
                    "strategy.on_failure",
                    format!("{}: a failed gate rolls back its batch, or all", quoted(o)),
                ));
            }
        }
        let g = &mut self.gate;
        if g.wait_ms < 0 || g.wait_ms > MAX_GATE_WAIT_MS {
            return Err(field(
                "strategy.gate.wait_ms",
                format!("the gate's wait is 0 to {MAX_GATE_WAIT_MS} ms"),
            ));
        }
        if g.max_5xx_ratio.is_nan() || !(0.0..=1.0).contains(&g.max_5xx_ratio) {
            return Err(field(
                "strategy.gate.max_5xx_ratio",
                "the share of 5xx is from 0 (not gated) to 1",
            ));
        }
        if g.window_ms < 0 || g.window_ms > MAX_GATE_WINDOW_MS {
            return Err(field(
                "strategy.gate.window_ms",
                format!("the window is 0 to {MAX_GATE_WINDOW_MS} ms"),
            ));
        }
        if g.min_requests < 0 || g.min_requests > MAX_GATE_MIN_REQUESTS {
            return Err(field(
                "strategy.gate.min_requests",
                format!("min_requests is 0 to {MAX_GATE_MIN_REQUESTS}"),
            ));
        }
        let gated = g.max_5xx_ratio > 0.0;
        if !gated && (g.window_ms != 0 || g.min_requests != 0) {
            return Err(field(
                "strategy.gate.max_5xx_ratio",
                "a window and min_requests go with a share of 5xx to gate",
            ));
        }
        if gated {
            if g.window_ms == 0 {
                g.window_ms = DEFAULT_GATE_WINDOW_MS;
            }
            if g.min_requests == 0 {
                g.min_requests = DEFAULT_MIN_REQUESTS;
            }
        }
        Ok(())
    }

    /// `names` in batches: fit's `Strategy.batches`.
    #[must_use]
    pub fn batches(&self, names: &[String]) -> Vec<Batch> {
        let mut out: Vec<Batch> = Vec::new();
        let mut add = |nodes: &[String], canary: bool| {
            out.push(Batch {
                index: out.len(),
                nodes: nodes.to_vec(),
                canary,
                state: B_PENDING.to_owned(),
                ..Batch::default()
            });
        };
        if self.mode == ALL {
            add(names, false);
            return out;
        }
        let mut rest = names;
        if self.mode == CANARY {
            let k = usize::try_from(self.canary).unwrap_or(0).min(rest.len());
            let (c, r) = rest.split_at(k);
            add(c, true);
            rest = r;
        }
        let mut size = usize::try_from(self.batch_size).unwrap_or(0);
        if self.batch_percent > 0 {
            let p = usize::try_from(self.batch_percent).unwrap_or(0);
            size = rest.len().saturating_mul(p).saturating_add(99) / 100;
        }
        if size == 0 {
            size = rest.len();
        }
        while !rest.is_empty() {
            let (b, r) = rest.split_at(size.min(rest.len()));
            add(b, false);
            rest = r;
        }
        out
    }

    /// How it goes, in words: fit's `Strategy.describe`.
    #[must_use]
    pub fn describe(&self) -> String {
        let mut b = String::new();
        if self.mode == ALL {
            b.push_str("all at once");
        } else {
            if self.mode == CANARY {
                b.push_str(&format!("a canary of {}, then", self.canary));
            }
            if self.batch_size > 0 {
                b.push_str(&format!(" batches of {}", self.batch_size));
            } else if self.batch_percent > 0 {
                b.push_str(&format!(" batches of {}%", self.batch_percent));
            } else {
                b.push_str(" the rest at once");
            }
            if self.pause_ms > 0 {
                b.push_str(&format!(", {} apart", go_duration(self.pause_ms)));
            }
        }
        b.push_str(&format!("; gate after {}", go_duration(self.gate.wait_ms)));
        if self.gate.max_5xx_ratio > 0.0 {
            b.push_str(&format!(
                ", 5xx at most {} over {} (from {} requests)",
                self.gate.max_5xx_ratio,
                go_duration(self.gate.window_ms),
                self.gate.min_requests
            ));
        }
        b.push_str("; a failed gate rolls back ");
        b.push_str(if self.on_failure == ON_FAILURE_ALL {
            "all"
        } else {
            "its batch"
        });
        b.trim().to_owned()
    }
}

/// A batch of a rollout's members: fit's `Batch`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Batch {
    /// Its index, from 0.
    pub index: usize,
    /// Its members.
    pub nodes: Vec<String>,
    /// Whether it is the canary.
    pub canary: bool,
    /// One of the `B_*`.
    pub state: String,
    /// When it was applied.
    pub started: Option<SystemTime>,
    /// When its gate checks its members.
    pub gate_at: Option<SystemTime>,
    /// When it ended.
    pub ended: Option<SystemTime>,
}

impl Batch {
    fn json(&self) -> String {
        let mut o = Obj::new()
            .num("index", self.index)
            .raw("nodes", &json::string_list(&self.nodes));
        if self.canary {
            o = o.bool("canary", true);
        }
        o = o.str("state", &self.state);
        o = time_or(o, "started", self.started);
        o = time_or(o, "gate_at", self.gate_at);
        time_or(o, "ended", self.ended).end()
    }

    fn from_value(v: &Value) -> Batch {
        Batch {
            index: usize::try_from(json::uint_of(v, "index")).unwrap_or(0),
            nodes: json::strings_of(v, "nodes"),
            canary: json::bool_of(v, "canary"),
            state: json::str_of(v, "state").to_owned(),
            started: time_of(v, "started"),
            gate_at: time_of(v, "gate_at"),
            ended: time_of(v, "ended"),
        }
    }
}

/// A part a rollout changes on a member: fit's `Change`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Change {
    /// The part, as `log` or `routes[api]`.
    pub path: String,
    /// `differs`, `added` or `only_a` (removed).
    pub state: String,
    /// Its sync state against the templates, for a template's or a sync's rollout.
    pub sync: String,
    /// Why it is in that state.
    pub reason: String,
    /// The template that owns it.
    pub template: String,
    /// The value now, its secrets masked, as JSON.
    pub have: Option<String>,
    /// The value rolled out, its secrets masked, as JSON.
    pub want: Option<String>,
}

impl Change {
    fn json(&self) -> String {
        Obj::new()
            .str("path", &self.path)
            .str("state", &self.state)
            .str_omit("sync", &self.sync)
            .str_omit("reason", &self.reason)
            .str_omit("template", &self.template)
            .opt("have", self.have.as_deref())
            .opt("want", self.want.as_deref())
            .end()
    }

    fn from_value(v: &Value) -> Change {
        Change {
            path: json::str_of(v, "path").to_owned(),
            state: json::str_of(v, "state").to_owned(),
            sync: json::str_of(v, "sync").to_owned(),
            reason: json::str_of(v, "reason").to_owned(),
            template: json::str_of(v, "template").to_owned(),
            have: v
                .get("have")
                .filter(|x| **x != Value::Null)
                .map(json::compact),
            want: v
                .get("want")
                .filter(|x| **x != Value::Null)
                .map(json::compact),
        }
    }
}

/// An error of a member's config refused: fit's `ConfigError`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigError {
    /// Where.
    pub path: String,
    /// What.
    pub message: String,
}

/// What a member's gate found: fit's `GateResult`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GateResult {
    /// Whether it passed.
    pub passed: bool,
    /// When it was checked.
    pub checked: Option<SystemTime>,
    /// Why it failed.
    pub reasons: Vec<String>,
    /// What it did not judge, and why.
    pub notes: Vec<String>,
    /// The config version the member ran.
    pub version: u64,
    /// The drift its consistency check found before the rollout applied it, and after.
    pub drift_before: Option<i64>,
    /// See [`drift_before`](GateResult::drift_before).
    pub drift_after: Option<i64>,
    /// The requests it answered in the window, the 5xx among them, and their share.
    pub requests: Option<u64>,
    /// See [`requests`](GateResult::requests).
    pub errors_5xx: Option<u64>,
    /// See [`requests`](GateResult::requests).
    pub ratio: Option<f64>,
}

impl GateResult {
    fn json(&self) -> String {
        let mut o = Obj::new().bool("passed", self.passed);
        o = time_or(o, "checked", self.checked);
        if !self.reasons.is_empty() {
            o = o.raw("reasons", &json::string_list(&self.reasons));
        }
        if !self.notes.is_empty() {
            o = o.raw("notes", &json::string_list(&self.notes));
        }
        if self.version != 0 {
            o = o.num("version", self.version);
        }
        if let Some(d) = self.drift_before {
            o = o.num("drift_before", d);
        }
        if let Some(d) = self.drift_after {
            o = o.num("drift_after", d);
        }
        if let Some(n) = self.requests {
            o = o.num("requests", n);
        }
        if let Some(n) = self.errors_5xx {
            o = o.num("errors_5xx", n);
        }
        if let Some(r) = self.ratio {
            o = o.raw("ratio", &float(r));
        }
        o.end()
    }

    fn from_value(v: &Value) -> GateResult {
        let int = |k: &str| {
            v.get(k)
                .filter(|x| **x != Value::Null)
                .map(|_| json::int_of(v, k))
        };
        let uint = |k: &str| {
            v.get(k)
                .filter(|x| **x != Value::Null)
                .map(|_| json::uint_of(v, k))
        };
        GateResult {
            passed: json::bool_of(v, "passed"),
            checked: time_of(v, "checked"),
            reasons: json::strings_of(v, "reasons"),
            notes: json::strings_of(v, "notes"),
            version: json::uint_of(v, "version"),
            drift_before: int("drift_before"),
            drift_after: int("drift_after"),
            requests: uint("requests"),
            errors_5xx: uint("errors_5xx"),
            ratio: match v.get("ratio") {
                Some(Value::Number(n)) => n.parse().ok(),
                _ => None,
            },
        }
    }
}

/// A member of a rollout: fit's `RolloutMember`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RolloutMember {
    /// Its name.
    pub node: String,
    /// One of the `M_*`.
    pub status: String,
    /// The ETag of its config the plan read, which the apply sends as `If-Match`.
    pub config_etag: String,
    /// The version it ran when planned.
    pub version: u64,
    /// The ETag of the config rolled out, as its dry run said.
    pub new_etag: String,
    /// The SHA-256 of the config rolled out, canonical (fit's `managed.Hash`).
    pub digest: String,
    /// The version the config rolled out runs as, and the one rolled back to.
    pub applied: u64,
    /// See [`applied`](RolloutMember::applied).
    pub rolled_back: u64,
    /// Its apply was not answered, and whether it runs the config rolled out is not known.
    pub unsure: bool,
    /// The parts that change.
    pub changes: Vec<Change>,
    /// Their paths, kept once the rollout ends.
    pub changed: Vec<String>,
    /// Its config's errors, refused.
    pub errors: Vec<ConfigError>,
    /// Its warnings.
    pub warnings: Vec<String>,
    /// A note: the member is of another version.
    pub note: String,
    /// What went wrong.
    pub error: Option<Fault>,
    /// What its gate found.
    pub gate: Option<GateResult>,
    /// The config rolled out, kept until the rollout ends; never answered.
    pub config: Option<Vec<u8>>,
}

impl RolloutMember {
    pub(crate) fn new(node: &str, status: &str) -> RolloutMember {
        RolloutMember {
            node: node.to_owned(),
            status: status.to_owned(),
            ..RolloutMember::default()
        }
    }

    fn json(&self, full: bool, secrets: bool) -> String {
        let mut o = Obj::new()
            .str("node", &self.node)
            .str("status", &self.status)
            .str_omit("config_etag", &self.config_etag);
        if self.version != 0 {
            o = o.num("version", self.version);
        }
        o = o
            .str_omit("new_etag", &self.new_etag)
            .str_omit("digest", &self.digest);
        if self.applied != 0 {
            o = o.num("applied", self.applied);
        }
        if self.rolled_back != 0 {
            o = o.num("rolled_back", self.rolled_back);
        }
        if self.unsure {
            o = o.bool("unsure", true);
        }
        if full && !self.changes.is_empty() {
            let c = self
                .changes
                .iter()
                .map(Change::json)
                .collect::<Vec<_>>()
                .join(",");
            o = o.raw("changes", &format!("[{c}]"));
        }
        if !self.changed.is_empty() {
            o = o.raw("changed", &json::string_list(&self.changed));
        }
        if !self.errors.is_empty() {
            let e = self
                .errors
                .iter()
                .map(|e| {
                    Obj::new()
                        .str_omit("path", &e.path)
                        .str("message", &e.message)
                        .end()
                })
                .collect::<Vec<_>>()
                .join(",");
            o = o.raw("errors", &format!("[{e}]"));
        }
        if !self.warnings.is_empty() {
            o = o.raw("warnings", &json::string_list(&self.warnings));
        }
        o = o.str_omit("note", &self.note);
        if let Some(e) = &self.error {
            o = o.raw("error", &e.json);
        }
        if let Some(g) = &self.gate {
            o = o.raw("gate", &g.json());
        }
        if secrets {
            if let Some(c) = self.config.as_ref().filter(|c| !c.is_empty()) {
                o = o.str("config", &akio_crypto::base64::STANDARD.encode(c));
            }
        }
        o.end()
    }

    fn from_value(v: &Value) -> RolloutMember {
        RolloutMember {
            node: json::str_of(v, "node").to_owned(),
            status: json::str_of(v, "status").to_owned(),
            config_etag: json::str_of(v, "config_etag").to_owned(),
            version: json::uint_of(v, "version"),
            new_etag: json::str_of(v, "new_etag").to_owned(),
            digest: json::str_of(v, "digest").to_owned(),
            applied: json::uint_of(v, "applied"),
            rolled_back: json::uint_of(v, "rolled_back"),
            unsure: json::bool_of(v, "unsure"),
            changes: list_of(v, "changes")
                .iter()
                .map(Change::from_value)
                .collect(),
            changed: json::strings_of(v, "changed"),
            errors: list_of(v, "errors")
                .iter()
                .map(|e| ConfigError {
                    path: json::str_of(e, "path").to_owned(),
                    message: json::str_of(e, "message").to_owned(),
                })
                .collect(),
            warnings: json::strings_of(v, "warnings"),
            note: json::str_of(v, "note").to_owned(),
            error: v
                .get("error")
                .filter(|e| matches!(e, Value::Object(_)))
                .map(Fault::from_value),
            gate: v
                .get("gate")
                .filter(|g| matches!(g, Value::Object(_)))
                .map(GateResult::from_value),
            config: match v.get("config") {
                Some(Value::String(s)) => akio_crypto::base64::STANDARD.decode(s).ok(),
                _ => None,
            },
        }
    }

    /// Whether a config of the rollout's may run on it, to be rolled back: fit's `toRollBack`.
    pub(crate) fn to_roll_back(&self) -> bool {
        matches!(
            self.status.as_str(),
            M_APPLIED | M_DONE | M_FAILED | M_ROLLBACK_FAILED
        ) && (self.applied != 0 || self.unsure)
    }
}

/// A rollout: fit's `Rollout`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rollout {
    /// Its ID.
    pub id: String,
    /// One of the `KIND_*`.
    pub kind: String,
    /// The template rolled out.
    pub template: String,
    /// The group whose members it changes.
    pub group: String,
    /// The member a copy copies from.
    pub from: String,
    /// The parts a copy copies.
    pub paths: Vec<String>,
    /// A patch's parts, or a template's draft.
    pub config: Option<Value>,
    /// The hash of the template's config a draft was made from.
    pub template_base: String,
    /// What a person said of it.
    pub description: String,
    /// Who planned it.
    pub planned_by: String,
    /// What proposed it (fit's drift and alerts, I138).
    pub source: String,
    /// The strategy proposed with it.
    pub suggested: Option<Strategy>,
    /// Who started it.
    pub started_by: String,
    /// One of the states.
    pub state: String,
    /// Why it is in that state.
    pub reason: String,
    /// How it applies its members, once started.
    pub strategy: Option<Strategy>,
    /// When it was made, planned, started and ended.
    pub created: Option<SystemTime>,
    /// See [`created`](Rollout::created).
    pub planned: Option<SystemTime>,
    /// See [`created`](Rollout::created).
    pub started: Option<SystemTime>,
    /// See [`created`](Rollout::created).
    pub ended: Option<SystemTime>,
    /// The batch under way, or the next.
    pub batch: usize,
    /// When the next batch starts, after a pause.
    pub next_at: Option<SystemTime>,
    /// Its batches.
    pub batches: Vec<Batch>,
    /// Its members.
    pub members: Vec<RolloutMember>,
    /// Its members by status.
    pub counts: BTreeMap<String, usize>,
}

impl Rollout {
    /// Whether it is planned or under way.
    #[must_use]
    pub fn active(&self) -> bool {
        matches!(
            self.state.as_str(),
            PLANNING | PLANNED | RUNNING | PAUSED | WAITING | ROLLING_BACK
        )
    }

    /// Whether it holds its members.
    #[must_use]
    pub fn holds(&self) -> bool {
        matches!(
            self.state.as_str(),
            RUNNING | PAUSED | WAITING | ROLLING_BACK
        )
    }

    pub(crate) fn member_mut(&mut self, name: &str) -> Option<&mut RolloutMember> {
        self.members.iter_mut().find(|x| x.node == name)
    }

    pub(crate) fn member(&self, name: &str) -> Option<&RolloutMember> {
        self.members.iter().find(|x| x.node == name)
    }

    pub(crate) fn count(&mut self) {
        let mut c = BTreeMap::new();
        for x in &self.members {
            let n: &mut usize = c.entry(x.status.clone()).or_default();
            *n = n.saturating_add(1);
        }
        self.counts = c;
    }

    pub(crate) fn counted(&self, status: &str) -> usize {
        self.counts.get(status).copied().unwrap_or(0)
    }

    /// Drops what an ended rollout no longer needs: the configs and the diffs' values.
    pub(crate) fn shed(&mut self) {
        self.config = None;
        for x in &mut self.members {
            x.config = None;
            for c in &mut x.changes {
                c.have = None;
                c.want = None;
            }
        }
    }

    /// Its members' names.
    #[must_use]
    pub fn nodes(&self) -> Vec<String> {
        self.members.iter().map(|x| x.node.clone()).collect()
    }

    /// As JSON: `full` with each member's changes, `secrets` with the configs as kept (the
    /// store's), without them the draft's secrets masked and the members' configs left out.
    #[must_use]
    pub fn json(&self, full: bool, secrets: bool) -> String {
        let mut o = Obj::new()
            .str("id", &self.id)
            .str("kind", &self.kind)
            .str_omit("template", &self.template)
            .str_omit("group", &self.group)
            .str_omit("from", &self.from);
        if !self.paths.is_empty() {
            o = o.raw("paths", &json::string_list(&self.paths));
        }
        if let Some(c) = &self.config {
            let c = if secrets {
                json::compact(c)
            } else {
                json::compact(&templates::masked(c, SECRET_MASK))
            };
            o = o.raw("config", &c);
        }
        o = o
            .str_omit("template_base", &self.template_base)
            .str_omit("description", &self.description)
            .str("planned_by", &self.planned_by)
            .str_omit("source", &self.source);
        if let Some(s) = &self.suggested {
            o = o.raw("suggested", &s.json());
        }
        o = o
            .str_omit("started_by", &self.started_by)
            .str("state", &self.state)
            .str_omit("reason", &self.reason);
        if let Some(s) = &self.strategy {
            o = o.raw("strategy", &s.json());
        }
        o = o.raw("created", &json::time(self.created.unwrap_or(UNIX_EPOCH)));
        o = time_or(o, "planned", self.planned);
        o = time_or(o, "started", self.started);
        o = time_or(o, "ended", self.ended);
        o = o.num("batch", self.batch);
        o = time_or(o, "next_at", self.next_at);
        if !self.batches.is_empty() {
            let b = self
                .batches
                .iter()
                .map(Batch::json)
                .collect::<Vec<_>>()
                .join(",");
            o = o.raw("batches", &format!("[{b}]"));
        }
        let m = self
            .members
            .iter()
            .map(|x| x.json(full, secrets))
            .collect::<Vec<_>>()
            .join(",");
        let mut counts = Obj::new();
        for (k, n) in &self.counts {
            counts = counts.num(k, n);
        }
        o.raw("members", &format!("[{m}]"))
            .raw("counts", &counts.end())
            .end()
    }

    /// A rollout as the store keeps it.
    #[must_use]
    pub fn from_value(v: &Value) -> Rollout {
        let strategy = |k: &str| {
            v.get(k)
                .filter(|s| matches!(s, Value::Object(_)))
                .and_then(|s| Strategy::from_value(s).ok())
        };
        let mut r = Rollout {
            id: json::str_of(v, "id").to_owned(),
            kind: json::str_of(v, "kind").to_owned(),
            template: json::str_of(v, "template").to_owned(),
            group: json::str_of(v, "group").to_owned(),
            from: json::str_of(v, "from").to_owned(),
            paths: json::strings_of(v, "paths"),
            config: v.get("config").filter(|c| **c != Value::Null).cloned(),
            template_base: json::str_of(v, "template_base").to_owned(),
            description: json::str_of(v, "description").to_owned(),
            planned_by: json::str_of(v, "planned_by").to_owned(),
            source: json::str_of(v, "source").to_owned(),
            suggested: strategy("suggested"),
            started_by: json::str_of(v, "started_by").to_owned(),
            state: json::str_of(v, "state").to_owned(),
            reason: json::str_of(v, "reason").to_owned(),
            strategy: strategy("strategy"),
            created: time_of(v, "created"),
            planned: time_of(v, "planned"),
            started: time_of(v, "started"),
            ended: time_of(v, "ended"),
            batch: usize::try_from(json::uint_of(v, "batch")).unwrap_or(0),
            next_at: time_of(v, "next_at"),
            batches: list_of(v, "batches")
                .iter()
                .map(Batch::from_value)
                .collect(),
            members: list_of(v, "members")
                .iter()
                .map(RolloutMember::from_value)
                .collect(),
            counts: BTreeMap::new(),
        };
        r.count();
        r
    }
}

/// What a person asked of a running rollout, read by its thread.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Run {
    pub(crate) run_no: u64,
    pub(crate) pause: bool,
    pub(crate) abort: bool,
}

/// The rollouts the manager keeps, under its lock.
#[derive(Default)]
pub(crate) struct Ro {
    /// Oldest first.
    pub(crate) list: Vec<Rollout>,
    /// The runs under way, by rollout.
    pub(crate) runs: HashMap<String, Run>,
    run_no: u64,
    /// The plans' and runs' threads alive.
    threads: usize,
    /// The audit entries waiting to be written, and whether a write is queued.
    pending: Vec<Value>,
    saving: bool,
}

impl Ro {
    pub(crate) fn get(&self, id: &str) -> Option<&Rollout> {
        self.list.iter().find(|r| r.id == id)
    }

    pub(crate) fn get_mut(&mut self, id: &str) -> Option<&mut Rollout> {
        self.list.iter_mut().find(|r| r.id == id)
    }

    /// What run `run_no` of rollout `id` was asked.
    pub(crate) fn flags(&self, id: &str, run_no: u64) -> Run {
        self.runs
            .get(id)
            .filter(|r| r.run_no == run_no)
            .copied()
            .unwrap_or_default()
    }

    fn active(&self) -> usize {
        self.list.iter().filter(|r| r.active()).count()
    }

    /// A new run of rollout `id`, its generation.
    fn run(&mut self, id: &str) -> u64 {
        self.run_no = self.run_no.wrapping_add(1);
        let run_no = self.run_no;
        self.runs.insert(
            id.to_owned(),
            Run {
                run_no,
                ..Run::default()
            },
        );
        run_no
    }

    /// Plans not started within [`PLAN_TTL`] expired; past [`MAX_KEPT`], the oldest ended
    /// dropped: fit's `expireLocked`.
    pub(crate) fn expire(&mut self) {
        let now = SystemTime::now();
        for r in &mut self.list {
            let old = r
                .planned
                .and_then(|p| now.duration_since(p).ok())
                .is_some_and(|d| d > PLAN_TTL);
            if r.state == PLANNED && old {
                EXPIRED.clone_into(&mut r.state);
                r.reason = format!(
                    "not started within {} of its plan: plan it again",
                    go_duration(i64::try_from(PLAN_TTL.as_millis()).unwrap_or(i64::MAX))
                );
                r.ended = Some(now);
                r.shed();
            }
        }
        while self.list.len() > MAX_KEPT {
            let Some(i) = self.list.iter().position(|r| !r.active()) else {
                return;
            };
            self.list.remove(i);
        }
    }
}

/// A rollout asked for: fit's `RolloutRequest`.
#[derive(Debug, Clone, Default)]
pub(crate) struct Request {
    pub(crate) kind: String,
    pub(crate) template: String,
    pub(crate) group: String,
    pub(crate) nodes: Vec<String>,
    pub(crate) from: String,
    pub(crate) paths: Vec<String>,
    pub(crate) config: Option<Value>,
    pub(crate) description: String,
}

impl Request {
    fn from_value(v: &Value) -> Request {
        Request {
            kind: json::str_of(v, "kind").to_owned(),
            template: json::str_of(v, "template").to_owned(),
            group: json::str_of(v, "group").to_owned(),
            nodes: json::strings_of(v, "nodes"),
            from: json::str_of(v, "from").to_owned(),
            paths: json::strings_of(v, "paths"),
            config: v.get("config").filter(|c| **c != Value::Null).cloned(),
            description: json::str_of(v, "description").to_owned(),
        }
    }
}

/// `part` read as `member` or `member[name]`, checked.
pub(crate) fn parse_part(s: &str) -> Result<(String, String), String> {
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
        return Err(e);
    }
    Ok((member.to_owned(), name.to_owned()))
}

/// The parts of a fragment of a config: each top-level member, or each element of a named list.
pub(crate) fn fragment_parts(frag: &Value) -> Vec<Rendered> {
    let mut out = Vec::new();
    let Value::Object(ms) = frag else { return out };
    for (k, v) in ms {
        match v {
            Value::Array(items) if jv::is_list(k) => {
                for it in items {
                    out.push(Rendered {
                        member: k.clone(),
                        name: jv::name_of(it).to_owned(),
                        template: String::new(),
                        overrides: Vec::new(),
                        value: it.clone(),
                    });
                }
            }
            _ => out.push(Rendered {
                member: k.clone(),
                name: String::new(),
                template: String::new(),
                overrides: Vec::new(),
                value: v.clone(),
            }),
        }
    }
    out
}

/// A fragment of an akit member's config, its secrets answered `[redacted]`, with them masked as
/// a template's are, so that each member it is copied to keeps its own (decision 041, 9).
pub(crate) fn unredacted(mut frag: Value) -> Value {
    let _ = jv::walk_strings(&mut frag, &mut Vec::new(), &mut |_, s| {
        Ok(if s == REDACTED {
            SECRET_MASK.to_owned()
        } else {
            s.to_owned()
        })
    });
    frag
}

/// The members a rollout of `req` plans, sorted: fit's `rolloutTargetsLocked`.
fn targets(d: &Document, req: &Request) -> Result<Vec<String>, Refusal> {
    let mut names: Vec<String> = Vec::new();
    let pick = |names: &mut Vec<String>, list: &[String]| -> Result<(), Refusal> {
        for n in list {
            if names.contains(n) {
                continue;
            }
            if d.index(n).is_none() {
                return Err(Refusal::NotFound(format!(
                    "no member is named {}",
                    quoted(n)
                )));
            }
            names.push(n.clone());
        }
        Ok(())
    };
    let group = |name: &str| -> Result<Vec<String>, Refusal> {
        let g = d
            .groups
            .iter()
            .find(|g| g.name == name)
            .ok_or_else(|| Refusal::NotFound(format!("no group is named {}", quoted(name))))?;
        Ok(crate::groups::nodes_of(d, g))
    };
    match req.kind.as_str() {
        KIND_TEMPLATE => {
            let Some(t) = d.templates.iter().find(|t| t.name == req.template) else {
                if req.template.is_empty() {
                    return Err(field("template", "a template's rollout names the template"));
                }
                return Err(Refusal::NotFound(format!(
                    "no template is named {}",
                    quoted(&req.template)
                )));
            };
            let of = templates::nodes_of(d, t).0;
            if req.nodes.is_empty() {
                names = of;
            } else {
                pick(&mut names, &req.nodes)?;
                if let Some(n) = names.iter().find(|n| !of.contains(n)) {
                    return Err(field(
                        "nodes",
                        format!(
                            "{} is in none of the groups of the template {}",
                            quoted(n),
                            quoted(&req.template)
                        ),
                    ));
                }
            }
        }
        KIND_SYNC => {
            if !req.nodes.is_empty() {
                pick(&mut names, &req.nodes)?;
            } else if !req.group.is_empty() {
                names = group(&req.group)?;
            } else {
                names = d.members.iter().map(|m| m.name.clone()).collect();
            }
        }
        KIND_COPY | KIND_PATCH => {
            let copy = req.kind == KIND_COPY;
            if copy {
                if req.from.is_empty() {
                    return Err(field(
                        "from",
                        "a copy names the member whose parts it copies",
                    ));
                }
                if d.index(&req.from).is_none() {
                    return Err(Refusal::NotFound(format!(
                        "no member is named {}",
                        quoted(&req.from)
                    )));
                }
                if req.paths.is_empty() {
                    return Err(field(
                        "paths",
                        "a copy names the parts it copies, as \"log\" or \"routes[api]\"",
                    ));
                }
                if req.paths.len() > MAX_TEMPLATE_PARTS {
                    return Err(field(
                        "paths",
                        format!("{} parts, more than {MAX_TEMPLATE_PARTS}", req.paths.len()),
                    ));
                }
                for p in &req.paths {
                    parse_part(p).map_err(|e| field("paths", format!("{}: {e}", quoted(p))))?;
                }
            }
            if !req.nodes.is_empty() {
                pick(&mut names, &req.nodes)?;
            } else if !req.group.is_empty() {
                names = group(&req.group)?;
            } else if copy {
                return Err(field(
                    "nodes",
                    "a copy names the members it copies to, or a group",
                ));
            } else {
                return Err(field(
                    "nodes",
                    "a patch names the members it changes, or a group",
                ));
            }
            if copy {
                names.retain(|n| *n != req.from);
            }
        }
        k => {
            return Err(field(
                "kind",
                format!(
                    "{} is not a kind of rollout: {KIND_TEMPLATE}, {KIND_SYNC}, {KIND_COPY} or \
                     {KIND_PATCH}",
                    quoted(k)
                ),
            ));
        }
    }
    if names.is_empty() {
        return Err(field("nodes", "the rollout has no members to plan"));
    }
    names.sort();
    Ok(names)
}

/// The config a rollout of `req` carries, and the hash of the template a draft was made from:
/// fit's `rolloutConfigLocked`.
fn rollout_config(d: &Document, req: &Request) -> Result<(Option<Value>, String), Refusal> {
    let Some(cfg) = &req.config else {
        if req.kind == KIND_PATCH {
            return Err(field(
                "config",
                "a patch carries the parts it puts on the members, as {\"log\": {...}} or \
                 {\"routes\": [{\"name\": ...}]}",
            ));
        }
        return Ok((None, String::new()));
    };
    if req.kind != KIND_PATCH && req.kind != KIND_TEMPLATE {
        return Err(field(
            "config",
            "only a patch or a template's draft carries config",
        ));
    }
    if json::compact(cfg).len() > MAX_TEMPLATE_BYTES {
        return Err(field(
            "config",
            format!("the config is {MAX_TEMPLATE_BYTES} bytes at most"),
        ));
    }
    templates::read_config(cfg, &mut |_| {}).map_err(Refusal::Field)?;
    let mut root = cfg.clone();
    if req.kind == KIND_PATCH {
        jv::walk_strings(&mut root, &mut Vec::new(), &mut |_, s| {
            if s.contains("${") {
                return Err(format!(
                    "{}: a patch takes no variables, which are a template's: write each member's \
                     value, or change a template",
                    quoted(&clip(s, 64))
                ));
            }
            Ok(s.to_owned())
        })
        .map_err(|e| field("config", e))?;
        return Ok((Some(root), String::new()));
    }
    let Some(t) = d.templates.iter().find(|t| t.name == req.template) else {
        return Err(Refusal::NotFound(format!(
            "no template is named {}",
            quoted(&req.template)
        )));
    };
    templates::restore_secrets(&mut root, Some(&t.config), "config")
        .map_err(|e| field("config", e))?;
    Ok((Some(root), jv::hash(&t.config)))
}

fn describe_rollout(r: &Rollout, members: usize) -> String {
    let s = match r.kind.as_str() {
        KIND_TEMPLATE if r.config.is_some() => format!(
            "a draft of the template {}, saved when it starts",
            r.template
        ),
        KIND_TEMPLATE => format!("the template {}", r.template),
        KIND_SYNC => "a sync with the templates".to_owned(),
        KIND_PATCH => {
            let parts: Vec<String> = r
                .config
                .as_ref()
                .map(|c| fragment_parts(c).iter().map(Rendered::display).collect())
                .unwrap_or_default();
            format!("a patch of {}", parts.join(", "))
        }
        _ => format!("a copy of {} from {}", r.paths.join(", "), r.from),
    };
    clip(&format!("{s}, on {members} members"), 512)
}

/// An audit entry of rollout `r`: fit's `AuditEntry`.
pub(crate) fn entry(
    user: &str,
    action: &str,
    r: &Rollout,
    detail: &str,
    nodes: &[String],
    outcomes: Vec<Value>,
    outcome: &str,
) -> Value {
    let s = |v: &str| Value::String(v.to_owned());
    let mut e = vec![
        ("at".to_owned(), s(&crate::now_text())),
        ("user".to_owned(), s(user)),
        ("action".to_owned(), s(action)),
    ];
    if !r.group.is_empty() {
        e.push(("group".to_owned(), s(&r.group)));
    }
    if !r.template.is_empty() {
        e.push(("template".to_owned(), s(&r.template)));
    }
    if !detail.is_empty() {
        e.push(("detail".to_owned(), s(detail)));
    }
    e.push(("rollout".to_owned(), s(&r.id)));
    if !outcomes.is_empty() {
        e.push(("outcomes".to_owned(), Value::Array(outcomes)));
    }
    if !nodes.is_empty() {
        e.push((
            "nodes".to_owned(),
            Value::Array(nodes.iter().map(|n| s(n)).collect()),
        ));
    }
    e.push(("outcome".to_owned(), s(outcome)));
    Value::Object(e)
}

/// A member's outcome in the audit log: fit's `AuditOutcome`.
pub(crate) fn outcome(
    node: &str,
    status: &str,
    detail: &str,
    error: &str,
    before: u64,
    after: u64,
    digest: &str,
) -> Value {
    let s = |v: &str| Value::String(v.to_owned());
    let mut o = vec![
        ("node".to_owned(), s(node)),
        ("status".to_owned(), s(status)),
    ];
    if !detail.is_empty() {
        o.push(("detail".to_owned(), s(detail)));
    }
    if !error.is_empty() {
        o.push(("error".to_owned(), s(error)));
    }
    if before != 0 {
        o.push(("before".to_owned(), Value::Number(before.to_string())));
    }
    if after != 0 {
        o.push(("after".to_owned(), Value::Number(after.to_string())));
    }
    if !digest.is_empty() {
        o.push(("digest".to_owned(), s(digest)));
    }
    Value::Object(o)
}

fn short_digest(d: &str) -> String {
    match d.get(..19) {
        Some(p) if d.len() > 19 => format!("{p}…"),
        _ => d.to_owned(),
    }
}

/// What became of a member, in words: fit's `memberDetail`.
fn member_detail(x: &RolloutMember) -> String {
    let mut parts = Vec::new();
    if x.version != 0 {
        parts.push(format!("from version {}", x.version));
    }
    if x.applied != 0 {
        parts.push(format!("applied as version {}", x.applied));
    }
    if x.rolled_back != 0 {
        parts.push(format!("rolled back as version {}", x.rolled_back));
    }
    if !x.digest.is_empty() {
        parts.push(format!("config {}", short_digest(&x.digest)));
    }
    parts.join(", ")
}

/// The audit entry of a rollout that ended: fit's `rolloutAudit`.
pub(crate) fn end_entry(r: &Rollout) -> Value {
    let outcomes = r
        .members
        .iter()
        .filter(|x| !matches!(x.status.as_str(), M_UNCHANGED | M_EXCLUDED | M_ERROR))
        .map(|x| {
            let after = if x.rolled_back != 0 {
                x.rolled_back
            } else {
                x.applied
            };
            let err = x.error.as_ref().map_or_else(String::new, |e| {
                clip(&format!("{}: {}", e.kind, e.message), 256)
            });
            outcome(
                &x.node,
                &x.status,
                &member_detail(x),
                &err,
                x.version,
                after,
                &x.digest,
            )
        })
        .collect();
    entry(
        &r.started_by,
        ACTION_END,
        r,
        &clip(&format!("{}: {}", r.state, r.reason), 512),
        &[],
        outcomes,
        &r.state,
    )
}

impl Manager {
    /// Reads the rollouts the document keeps: one that ran is paused where it stopped, one being
    /// planned expired, one rolling back failed (fit's `loadRollouts`); those under way hold
    /// their members.
    pub(crate) fn load_rollouts(&self) {
        let raw = lock(&self.0.doc).doc.rollouts.clone();
        let Some(Value::Array(list)) = json::parse(raw.as_bytes()) else {
            return;
        };
        let now = SystemTime::now();
        let mut ro = lock(&self.0.ro);
        let mut ops = lock(&self.0.ops);
        for v in &list {
            let mut r = Rollout::from_value(v);
            match r.state.as_str() {
                PLANNING => {
                    EXPIRED.clone_into(&mut r.state);
                    "the manager restarted while it was planned: plan it again"
                        .clone_into(&mut r.reason);
                    r.ended = Some(now);
                    r.shed();
                }
                RUNNING => {
                    PAUSED.clone_into(&mut r.state);
                    "the manager restarted while it ran: it was paused where it stopped; resume it"
                        .clone_into(&mut r.reason);
                    r.next_at = None;
                }
                ROLLING_BACK => {
                    FAILED.clone_into(&mut r.state);
                    "the manager restarted while it rolled back: roll it back again"
                        .clone_into(&mut r.reason);
                    r.ended = Some(now);
                }
                _ => {}
            }
            r.count();
            if r.holds() {
                hold(&r, &mut ops.busy);
            }
            ro.list.push(r);
        }
    }

    /// The rollouts as the store keeps them, as JSON.
    pub(crate) fn rollouts_json(&self) -> String {
        let ro = lock(&self.0.ro);
        let list = ro
            .list
            .iter()
            .map(|r| r.json(true, true))
            .collect::<Vec<_>>()
            .join(",");
        format!("[{list}]")
    }

    /// Writes the rollouts, with `entries` appended to the audit log, on the manager's pool: one
    /// write queued at a time, carrying what is there when it runs (fit's `saveRollouts`).
    pub(crate) fn save_rollouts(&self, entries: Vec<Value>) {
        {
            let mut ro = lock(&self.0.ro);
            ro.pending.extend(entries);
            if ro.saving {
                return;
            }
            ro.saving = true;
        }
        let me = self.clone();
        let job = Box::new(move || {
            let entries = {
                let mut ro = lock(&me.0.ro);
                ro.saving = false;
                std::mem::take(&mut ro.pending)
            };
            let mut d = {
                let g = lock(&me.0.doc);
                if !g.read_only.is_empty() {
                    return;
                }
                g.doc.clone()
            };
            for e in entries {
                d.append_audit(e);
            }
            match me.write_doc(&mut d) {
                Ok(()) => {
                    let mut g = lock(&me.0.doc);
                    g.doc.audit = d.audit;
                    g.doc.rollouts = d.rollouts;
                }
                Err(e) => me.log(format!("cluster: the rollouts were not saved: {e}")),
            }
        });
        if !self.on_pool(job) {
            // The entries wait for the next write.
            lock(&self.0.ro).saving = false;
            self.log(
                "cluster: the rollouts were not saved: too many writes waiting; the next write \
                 saves them"
                    .to_owned(),
            );
        }
    }

    /// Waits for the rollouts' threads to end, `wait` at most: the manager's close.
    pub(crate) fn wait_rollouts(&self, wait: Duration) {
        let deadline = Instant::now().checked_add(wait);
        let mut ro = lock(&self.0.ro);
        while ro.threads > 0 {
            let left = deadline.map_or(Duration::ZERO, |d| {
                d.saturating_duration_since(Instant::now())
            });
            if left.is_zero() {
                return;
            }
            ro = self
                .0
                .ro_cv
                .wait_timeout(ro, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Runs `f` on a thread of its own, counted: `false` when it could not start.
    pub(crate) fn spawn_rollout(&self, name: &str, f: impl FnOnce() + Send + 'static) -> bool {
        struct Count(Manager);
        impl Drop for Count {
            fn drop(&mut self) {
                let mut ro = lock(&self.0.0.ro);
                ro.threads = ro.threads.saturating_sub(1);
                drop(ro);
                self.0.0.ro_cv.notify_all();
            }
        }
        {
            let mut ro = lock(&self.0.ro);
            ro.threads = ro.threads.saturating_add(1);
        }
        let count = Count(self.clone());
        let started = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let _count = count;
                f();
            });
        // A thread that did not start dropped its count with its closure.
        started.is_ok()
    }

    /// Whether a rollout may be steered: the manager runs, its document is written.
    fn steerable(&self) -> Result<(), Refusal> {
        if self.0.closed.load(Ordering::Acquire) {
            return Err(Refusal::Unavailable(
                "the manager is shutting down".to_owned(),
            ));
        }
        let why = self.read_only();
        if !why.is_empty() {
            return Err(Refusal::Conflict(why));
        }
        Ok(())
    }

    /// Plans a rollout: its members read, each one's diff and dry run, on a thread of its own;
    /// answered at once, planning. fit's `PlanRollout`.
    pub(crate) fn plan_rollout(&self, user: &str, mut req: Request) -> Result<String, Refusal> {
        req.description = req.description.trim().to_owned();
        if req.description.len() > MAX_DESCRIPTION || !printable(&req.description) {
            return Err(field(
                "description",
                format!(
                    "the description is at most {MAX_DESCRIPTION} bytes of printable characters"
                ),
            ));
        }
        if self.0.closed.load(Ordering::Acquire) {
            return Err(Refusal::Unavailable(
                "the manager is shutting down".to_owned(),
            ));
        }
        let why = self.read_only();
        if !why.is_empty() {
            return Err(Refusal::Conflict(why));
        }
        let d = self.doc();
        let names = targets(&d, &req)?;
        let (config, base) = rollout_config(&d, &req)?;
        let mut ro = lock(&self.0.ro);
        ro.expire();
        let n = ro.active();
        if n >= MAX_ACTIVE {
            return Err(Refusal::Busy(format!(
                "{n} rollouts are planned or under way already: start, discard or end one first"
            )));
        }
        let mut r = Rollout {
            id: crate::ops::new_id(),
            kind: req.kind.clone(),
            template: req.template.clone(),
            group: if req.kind == KIND_TEMPLATE {
                String::new()
            } else {
                req.group.clone()
            },
            from: req.from.clone(),
            paths: req.paths.clone(),
            config,
            template_base: base,
            description: req.description.clone(),
            planned_by: user.to_owned(),
            state: PLANNING.to_owned(),
            created: Some(SystemTime::now()),
            members: names
                .iter()
                .map(|n| RolloutMember::new(n, M_PLANNING))
                .collect(),
            ..Rollout::default()
        };
        r.count();
        let out = r.json(true, false);
        let e = entry(
            user,
            ACTION_PLAN,
            &r,
            &describe_rollout(&r, names.len()),
            &names,
            Vec::new(),
            "ok",
        );
        let (id, kind) = (r.id.clone(), r.kind.clone());
        ro.list.push(r);
        drop(ro);
        let me = self.clone();
        let id2 = id.clone();
        if !self.spawn_rollout("akit-rollout-plan", move || me.plan_run(&id2)) {
            lock(&self.0.ro).list.retain(|r| r.id != id);
            return Err(Refusal::Failed(
                "the rollout's thread could not start".to_owned(),
            ));
        }
        self.save_rollouts(vec![e]);
        self.log(format!(
            "cluster: {user:?} planned the rollout {id} ({kind}) of {}",
            names.join(", ")
        ));
        Ok(out)
    }

    /// Starts a planned rollout, saving its template's draft first: fit's `StartRollout`. On the
    /// manager's pool: it may write the store.
    pub(crate) fn start_rollout(
        &self,
        user: &str,
        id: &str,
        strategy: Strategy,
        exclude: &[String],
    ) -> Result<String, Refusal> {
        self.save_draft(user, id, &strategy, exclude)?;
        self.steerable()?;
        let mut ro = lock(&self.0.ro);
        ro.expire();
        let r = ro.get_mut(id).ok_or_else(|| no_rollout(id))?;
        if r.state != PLANNED {
            return Err(Refusal::Conflict(format!(
                "the rollout is {}: only a planned rollout starts",
                r.state
            )));
        }
        if let Some(n) = exclude.iter().find(|n| r.member(n).is_none()) {
            return Err(field(
                "exclude",
                format!("{} is not a member of the rollout", quoted(n)),
            ));
        }
        let mut names = Vec::new();
        let mut busy = Vec::new();
        {
            let ops = lock(&self.0.ops);
            for x in &r.members {
                if x.status != M_READY || exclude.contains(&x.node) {
                    continue;
                }
                if let Some(by) = ops.busy.get(&x.node) {
                    busy.push(format!("{} ({by})", x.node));
                }
                names.push(x.node.clone());
            }
        }
        if names.is_empty() {
            return Err(Refusal::Conflict(
                "no member of the rollout is ready to be applied".to_owned(),
            ));
        }
        let mut s = strategy;
        s.check(names.len())?;
        if !busy.is_empty() {
            return Err(Refusal::Conflict(format!(
                "a rollout or an operation runs on {}: wait for it to end",
                busy.join(", ")
            )));
        }
        for x in &mut r.members {
            if x.status == M_READY && exclude.contains(&x.node) {
                M_EXCLUDED.clone_into(&mut x.status);
                x.config = None;
            } else if x.status == M_READY {
                M_PENDING.clone_into(&mut x.status);
            }
        }
        r.batches = s.batches(&names);
        r.batch = 0;
        r.strategy = Some(s.clone());
        RUNNING.clone_into(&mut r.state);
        user.clone_into(&mut r.started_by);
        r.started = Some(SystemTime::now());
        r.reason.clear();
        r.count();
        hold(r, &mut lock(&self.0.ops).busy);
        let outcomes = r
            .members
            .iter()
            .filter(|x| names.contains(&x.node))
            .map(|x| outcome(&x.node, &x.status, "", "", x.version, 0, &x.digest))
            .collect();
        let e = entry(
            user,
            ACTION_START,
            r,
            &clip(
                &format!(
                    "{} members in {} batches: {}",
                    names.len(),
                    r.batches.len(),
                    s.describe()
                ),
                512,
            ),
            &names,
            outcomes,
            "ok",
        );
        let out = r.json(true, false);
        let run_no = ro.run(id);
        drop(ro);
        self.save_rollouts(vec![e]);
        self.log(format!(
            "cluster: {user:?} started the rollout {id} on {}: {}",
            names.join(", "),
            s.describe()
        ));
        self.go(id, run_no);
        Ok(out)
    }

    /// Starts run `run_no` of rollout `id` on its thread; one that cannot start is paused, to be
    /// resumed.
    fn go(&self, id: &str, run_no: u64) {
        let (me, id2) = (self.clone(), id.to_owned());
        if self.spawn_rollout("akit-rollout", move || me.run_rollout(&id2, run_no)) {
            return;
        }
        let mut ro = lock(&self.0.ro);
        if ro.flags(id, run_no).run_no == run_no {
            ro.runs.remove(id);
        }
        if let Some(r) = ro.get_mut(id) {
            if r.state == ROLLING_BACK {
                FAILED.clone_into(&mut r.state);
                "its thread could not start: roll it back again".clone_into(&mut r.reason);
            } else {
                PAUSED.clone_into(&mut r.state);
                "its thread could not start: resume it".clone_into(&mut r.reason);
            }
            r.count();
        }
        drop(ro);
        self.save_rollouts(Vec::new());
    }

    /// Saves the draft a template's rollout carries as the template, when it starts: fit's
    /// `saveDraft`. On the manager's pool, as every write of the document.
    fn save_draft(
        &self,
        user: &str,
        id: &str,
        strategy: &Strategy,
        exclude: &[String],
    ) -> Result<(), Refusal> {
        let (tpl, draft, base, n) = {
            let ro = lock(&self.0.ro);
            let Some(r) = ro.get(id) else { return Ok(()) };
            let Some(draft) = r.config.clone() else {
                return Ok(());
            };
            if r.kind != KIND_TEMPLATE
                || r.state != PLANNED
                || self.0.closed.load(Ordering::Acquire)
                || !self.read_only().is_empty()
            {
                return Ok(());
            }
            let n = r
                .members
                .iter()
                .filter(|x| x.status == M_READY && !exclude.contains(&x.node))
                .count();
            (r.template.clone(), draft, r.template_base.clone(), n)
        };
        if n == 0 {
            // None ready: the start says so, saving nothing.
            return Ok(());
        }
        strategy.clone().check(n)?;
        let d = self.doc();
        if d.templates
            .iter()
            .find(|t| t.name == tpl)
            .is_some_and(|t| jv::hash(&t.config) == jv::hash(&draft))
        {
            // Saved by an earlier start that went no further.
            return Ok(());
        }
        self.change(user, |d| {
            let Some(i) = d.templates.iter().position(|t| t.name == tpl) else {
                return Err(Refusal::Conflict(format!(
                    "the template {} was removed since the rollout was planned: plan it again",
                    quoted(&tpl)
                )));
            };
            let t = d.templates.get_mut(i).ok_or_else(|| {
                Refusal::Conflict(format!("the template {} was removed", quoted(&tpl)))
            })?;
            if jv::hash(&t.config) != base {
                return Err(Refusal::Conflict(format!(
                    "the template {} changed since the rollout was planned: plan it again from \
                     the template as it is now",
                    quoted(&tpl)
                )));
            }
            t.config = draft;
            t.updated = crate::now_text();
            let t = t.clone();
            let nodes = templates::nodes_of(d, &t).0;
            let s = |v: &str| Value::String(v.to_owned());
            Ok((
                vec![
                    ("action".to_owned(), s("template.update")),
                    ("template".to_owned(), s(&tpl)),
                    (
                        "detail".to_owned(),
                        s(&clip(
                            &format!("config, saved by starting the rollout {id} that planned it"),
                            512,
                        )),
                    ),
                    ("rollout".to_owned(), s(id)),
                    (
                        "nodes".to_owned(),
                        Value::Array(nodes.iter().map(|n| s(n)).collect()),
                    ),
                ],
                (),
            ))
        })?;
        self.log(format!(
            "cluster: {user:?} saved the template {tpl:?}, drafted by the rollout {id} it starts"
        ));
        Ok(())
    }

    /// Pauses, resumes, goes on with, aborts, rolls back or discards rollout `id`: fit's
    /// `SteerRollout`.
    pub(crate) fn steer_rollout(
        &self,
        user: &str,
        id: &str,
        what: &str,
    ) -> Result<String, Refusal> {
        self.steerable()?;
        let mut ro = lock(&self.0.ro);
        if ro.get(id).is_none() {
            return Err(no_rollout(id));
        }
        ro.expire();
        let Ro { list, runs, .. } = &mut *ro;
        let r = list
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or_else(|| no_rollout(id))?;
        let run = runs.get_mut(id);
        let refuse = |r: &Rollout, why: &str| {
            Err(Refusal::Conflict(format!(
                "the rollout is {}: {why}",
                r.state
            )))
        };
        let mut start = false;
        let mut detail = "";
        let mut abort_now = false;
        match what {
            STEER_PAUSE => {
                let Some(run) = run.filter(|_| r.state == RUNNING) else {
                    return refuse(r, "only a running rollout pauses");
                };
                run.pause = true;
                detail = "pauses after the batch under way";
            }
            STEER_RESUME | STEER_GO => {
                let running = r.state == RUNNING;
                match run {
                    // The pause asked for is taken back.
                    Some(run) if what == STEER_RESUME && running && run.pause => run.pause = false,
                    _ if (what == STEER_RESUME && r.state != PAUSED)
                        || (what == STEER_GO && r.state != WAITING) =>
                    {
                        return refuse(
                            r,
                            if what == STEER_RESUME {
                                "only a paused rollout resumes"
                            } else {
                                "only a rollout whose canary waits goes on"
                            },
                        );
                    }
                    _ => {
                        RUNNING.clone_into(&mut r.state);
                        r.reason.clear();
                        r.count();
                        start = true;
                    }
                }
            }
            STEER_ABORT => match run {
                Some(run) if r.state == RUNNING => {
                    run.abort = true;
                    detail = "stops after the members being applied";
                }
                _ if r.state == PAUSED || r.state == WAITING => abort_now = true,
                _ => return refuse(r, "only a running, paused or waiting rollout is aborted"),
            },
            STEER_ROLLBACK => {
                if !matches!(r.state.as_str(), PAUSED | WAITING | DONE | FAILED | ABORTED) {
                    return refuse(
                        r,
                        "a rollout is rolled back once it is paused, waiting or ended",
                    );
                }
                if !r.members.iter().any(RolloutMember::to_roll_back) {
                    return refuse(
                        r,
                        "none of its members has a config of the rollout's to roll back",
                    );
                }
                let mine = format!("rollout {id}");
                let mut ops = lock(&self.0.ops);
                let busy: Vec<String> = r
                    .members
                    .iter()
                    .filter(|x| x.to_roll_back())
                    .filter_map(|x| {
                        ops.busy
                            .get(&x.node)
                            .filter(|by| **by != mine)
                            .map(|by| format!("{} ({by})", x.node))
                    })
                    .collect();
                if !busy.is_empty() {
                    return Err(Refusal::Conflict(format!(
                        "a rollout or an operation runs on {}: wait for it to end",
                        busy.join(", ")
                    )));
                }
                ROLLING_BACK.clone_into(&mut r.state);
                r.reason = format!("rolled back by {user}");
                r.ended = None;
                r.count();
                hold(r, &mut ops.busy);
                start = true;
            }
            STEER_DISCARD => {
                if r.state != PLANNED && r.state != EXPIRED {
                    return refuse(r, "only a plan is discarded");
                }
                "discarded".clone_into(&mut r.state);
            }
            _ => {
                return Err(field(
                    "action",
                    format!(
                        "{}: a rollout is paused, resumed, gone on with, aborted, rolled back or \
                         discarded",
                        quoted(what)
                    ),
                ));
            }
        }
        let ended = if abort_now {
            self.end(
                &mut ro,
                id,
                ABORTED,
                &format!("aborted by {user}; what was applied stays"),
            )
        } else {
            None
        };
        let Some(r) = ro.get(id) else {
            return Err(no_rollout(id));
        };
        let out = r.json(true, false);
        let mut entries = vec![entry(
            user,
            &format!("rollout.{what}"),
            r,
            detail,
            &r.nodes(),
            Vec::new(),
            "ok",
        )];
        if what == STEER_DISCARD {
            ro.list.retain(|r| r.id != id);
        }
        let run_no = start.then(|| ro.run(id));
        drop(ro);
        self.0.ro_cv.notify_all();
        let mut line = None;
        if let Some((e, l)) = ended {
            entries.push(e);
            line = Some(l);
        }
        self.save_rollouts(entries);
        self.log(format!(
            "cluster: {user:?} asked the rollout {id} to {what}"
        ));
        if let Some(l) = line {
            self.log(l);
        }
        if let Some(run_no) = run_no {
            self.go(id, run_no);
        }
        Ok(out)
    }

    /// Ends rollout `id` in `state`, releasing its members: its audit entry and the log's line
    /// (fit's `endLocked`, with `finishLocked`'s entry).
    pub(crate) fn end(
        &self,
        ro: &mut Ro,
        id: &str,
        state: &str,
        why: &str,
    ) -> Option<(Value, String)> {
        let r = ro.get_mut(id)?;
        state.clone_into(&mut r.state);
        why.clone_into(&mut r.reason);
        r.ended = Some(SystemTime::now());
        r.next_at = None;
        r.shed();
        r.count();
        release(r, &mut lock(&self.0.ops).busy);
        let e = end_entry(r);
        let line = format!(
            "cluster: the rollout {id} ended {state} ({why}): {} done, {} failed, {} skipped, {} \
             rolled back",
            r.counted(M_DONE),
            r.counted(M_FAILED),
            r.counted(M_SKIPPED),
            r.counted(M_ROLLED_BACK)
        );
        ro.expire();
        Some((e, line))
    }
}

/// Holds `r`'s members, as an operation does: fit's `holdLocked`.
pub(crate) fn hold(r: &Rollout, busy: &mut HashMap<String, String>) {
    for x in &r.members {
        if !matches!(x.status.as_str(), M_EXCLUDED | M_UNCHANGED | M_ERROR)
            && !busy.contains_key(&x.node)
        {
            busy.insert(x.node.clone(), format!("rollout {}", r.id));
        }
    }
}

/// Lets `r`'s members go: fit's `releaseLocked`.
pub(crate) fn release(r: &Rollout, busy: &mut HashMap<String, String>) {
    let mine = format!("rollout {}", r.id);
    busy.retain(|_, by| *by != mine);
}

fn no_rollout(id: &str) -> Refusal {
    Refusal::NotFound(format!("no rollout {} is kept", quoted(id)))
}

fn not_allowed(allow: &str) -> Response {
    fail(405, "the method is not allowed").header("Allow", allow)
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

fn answered(status: u16, r: Result<String, Refusal>) -> Response {
    match r {
        Ok(body) => answer(status, body),
        Err(e) => e.response(),
    }
}

/// The rollouts' endpoints; `false` for a path that is not theirs.
pub(crate) fn serve(m: &Manager, ctx: &mut Context, user: &str, path: &str) -> bool {
    let method = ctx.method().clone();
    let get = matches!(method, Method::Get | Method::Head);
    if path == "/api/rollouts" {
        if get {
            let ro_why = m.read_only();
            let mut ro = lock(&m.0.ro);
            ro.expire();
            let list = ro
                .list
                .iter()
                .rev()
                .map(|r| r.json(false, false))
                .collect::<Vec<_>>()
                .join(",");
            drop(ro);
            ctx.respond(answer(
                200,
                Obj::new()
                    .num("max_active", MAX_ACTIVE)
                    .num("plan_ttl_ms", PLAN_TTL.as_millis())
                    .str_omit("read_only", &ro_why)
                    .raw("rollouts", &format!("[{list}]"))
                    .end(),
            ));
        } else if method == Method::Post {
            let Some(v) = decode(ctx) else { return true };
            let (me, user) = (m.clone(), user.to_owned());
            crate::api::on_pool(m, ctx, move || {
                answered(202, me.plan_rollout(&user, Request::from_value(&v)))
            });
        } else {
            ctx.respond(not_allowed("GET, POST"));
        }
        return true;
    }
    let Some(rest) = path.strip_prefix("/api/rollouts/") else {
        return false;
    };
    let (id, action) = rest.split_once('/').unwrap_or((rest, ""));
    let (id, action) = (id.to_owned(), action.to_owned());
    let (me, user) = (m.clone(), user.to_owned());
    match (action.as_str(), &method) {
        ("", _) if get => {
            let mut ro = lock(&m.0.ro);
            ro.expire();
            let found = ro.get(&id).map(|r| r.json(true, false));
            drop(ro);
            ctx.respond(match found {
                Some(r) => answer(200, r),
                None => fail(404, &format!("no rollout {} is kept", quoted(&id))),
            });
        }
        ("", Method::Delete) => crate::api::on_pool(m, ctx, move || {
            match me.steer_rollout(&user, &id, STEER_DISCARD) {
                Ok(_) => answer(200, Obj::new().str("discarded", &id).end()),
                Err(e) => e.response(),
            }
        }),
        ("start", Method::Post) => {
            let Some(v) = decode(ctx) else { return true };
            let strategy =
                match Strategy::from_value(&v.get("strategy").cloned().unwrap_or(Value::Null)) {
                    Ok(s) => s,
                    Err(e) => {
                        ctx.respond(e.response());
                        return true;
                    }
                };
            let exclude = json::strings_of(&v, "exclude");
            crate::api::on_pool(m, ctx, move || {
                answered(200, me.start_rollout(&user, &id, strategy, &exclude))
            });
        }
        (STEER_PAUSE | STEER_RESUME | STEER_GO | STEER_ABORT | STEER_ROLLBACK, Method::Post) => {
            crate::api::on_pool(m, ctx, move || {
                answered(200, me.steer_rollout(&user, &id, &action))
            });
        }
        ("", _) => ctx.respond(not_allowed("GET, DELETE")),
        _ => ctx.respond(fail(404, "no such endpoint")),
    }
    true
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects
    )]
    use super::*;

    fn names(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("n{i}")).collect()
    }

    #[test]
    fn strategies() {
        // fit's TestStrategies.
        let mut s = Strategy {
            mode: ALL.into(),
            ..Strategy::default()
        };
        s.check(3).unwrap();
        assert_eq!(s.on_failure, ON_FAILURE_BATCH);
        let b = s.batches(&names(3));
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].nodes.len(), 3);
        assert_eq!(
            s.describe(),
            "all at once; gate after 0s; a failed gate rolls back its batch"
        );

        let mut s = Strategy {
            mode: ROLLING.into(),
            batch_size: 2,
            pause_ms: 1500,
            gate: Gate {
                wait_ms: 5000,
                max_5xx_ratio: 0.05,
                ..Gate::default()
            },
            ..Strategy::default()
        };
        s.check(5).unwrap();
        assert_eq!(s.gate.window_ms, DEFAULT_GATE_WINDOW_MS);
        assert_eq!(s.gate.min_requests, DEFAULT_MIN_REQUESTS);
        let b = s.batches(&names(5));
        assert_eq!(
            b.iter().map(|b| b.nodes.len()).collect::<Vec<_>>(),
            [2, 2, 1]
        );
        assert_eq!(
            s.describe(),
            "batches of 2, 1.5s apart; gate after 5s, 5xx at most 0.05 over 30s (from 20 \
             requests); a failed gate rolls back its batch"
        );

        let mut s = Strategy {
            mode: CANARY.into(),
            canary: 1,
            batch_percent: 50,
            on_failure: ON_FAILURE_ALL.into(),
            ..Strategy::default()
        };
        s.check(5).unwrap();
        let b = s.batches(&names(5));
        assert_eq!(
            b.iter()
                .map(|b| (b.nodes.len(), b.canary))
                .collect::<Vec<_>>(),
            [(1, true), (2, false), (2, false)]
        );
        assert!(
            s.describe()
                .starts_with("a canary of 1, then batches of 50%")
        );
        let mut s = Strategy {
            mode: CANARY.into(),
            canary: 2,
            ..Strategy::default()
        };
        s.check(3).unwrap();
        assert_eq!(
            s.batches(&names(3))
                .iter()
                .map(|b| b.nodes.len())
                .collect::<Vec<_>>(),
            [2, 1]
        );

        let refused = |mut s: Strategy, n: usize| match s.check(n) {
            Err(Refusal::Field(f)) => f.field,
            other => panic!("{other:?}"),
        };
        let st = |mode: &str| Strategy {
            mode: mode.into(),
            ..Strategy::default()
        };
        assert_eq!(refused(st("x"), 2), "strategy.mode");
        assert_eq!(
            refused(
                Strategy {
                    pause_ms: 1,
                    ..st(ALL)
                },
                2
            ),
            "strategy.mode"
        );
        assert_eq!(refused(st(ROLLING), 2), "strategy.batch_size");
        assert_eq!(
            refused(
                Strategy {
                    batch_size: 3,
                    ..st(ROLLING)
                },
                2
            ),
            "strategy.batch_size"
        );
        assert_eq!(
            refused(
                Strategy {
                    batch_size: 1,
                    batch_percent: 10,
                    ..st(ROLLING)
                },
                2
            ),
            "strategy.batch_size"
        );
        assert_eq!(
            refused(
                Strategy {
                    canary: 1,
                    ..st(CANARY)
                },
                1
            ),
            "strategy.mode"
        );
        assert_eq!(
            refused(
                Strategy {
                    canary: 2,
                    ..st(CANARY)
                },
                2
            ),
            "strategy.canary"
        );
        assert_eq!(
            refused(
                Strategy {
                    on_failure: "x".into(),
                    ..st(ALL)
                },
                2
            ),
            "strategy.on_failure"
        );
        let gated = |g: Gate| Strategy { gate: g, ..st(ALL) };
        assert_eq!(
            refused(
                gated(Gate {
                    wait_ms: MAX_GATE_WAIT_MS + 1,
                    ..Gate::default()
                }),
                2
            ),
            "strategy.gate.wait_ms"
        );
        assert_eq!(
            refused(
                gated(Gate {
                    max_5xx_ratio: 1.5,
                    ..Gate::default()
                }),
                2
            ),
            "strategy.gate.max_5xx_ratio"
        );
        assert_eq!(
            refused(
                gated(Gate {
                    window_ms: 10,
                    ..Gate::default()
                }),
                2
            ),
            "strategy.gate.max_5xx_ratio"
        );
    }

    #[test]
    fn times_and_numbers_as_go_writes_them() {
        let t = UNIX_EPOCH + Duration::new(1_700_000_000, 123_456_789);
        let text = json::time(t);
        assert_eq!(parse_time(text.trim_matches('"')), Some(t));
        assert_eq!(
            parse_time("2023-11-14T22:13:20+01:00"),
            Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000 - 3600))
        );
        assert_eq!(parse_time("0001-01-01T00:00:00Z"), None);
        assert_eq!(parse_time("nonsense"), None);
        assert_eq!(float(0.05), "0.05");
        assert_eq!(float(1.0), "1");
        assert_eq!(float(1e-7), "1e-07");
        assert_eq!(go_duration(500), "500ms");
        assert_eq!(go_duration(60_000), "1m0s");
        assert_eq!(go_duration(3_600_000), "1h0m0s");
        assert_eq!(go_duration(90_250), "1m30.25s");
    }

    #[test]
    fn rollouts_kept_as_fit_keeps_them() {
        let mut r = Rollout {
            id: "abc".into(),
            kind: KIND_PATCH.into(),
            group: "g".into(),
            config: Some(json::parse(br#"{"log": {"level": "debug"}}"#).unwrap()),
            planned_by: "ada".into(),
            state: RUNNING.into(),
            strategy: Some(Strategy {
                mode: ROLLING.into(),
                batch_size: 1,
                on_failure: ON_FAILURE_BATCH.into(),
                gate: Gate {
                    wait_ms: 10,
                    max_5xx_ratio: 0.5,
                    window_ms: 100,
                    min_requests: 3,
                },
                ..Strategy::default()
            }),
            created: Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            batches: vec![Batch {
                nodes: vec!["a".into()],
                state: B_GATING.into(),
                ..Batch::default()
            }],
            members: vec![RolloutMember {
                node: "a".into(),
                status: M_APPLIED.into(),
                config_etag: "\"e1\"".into(),
                applied: 4,
                changes: vec![Change {
                    path: "log".into(),
                    state: "differs".into(),
                    have: Some("{}".into()),
                    want: Some(r#"{"level":"debug"}"#.into()),
                    ..Change::default()
                }],
                error: Some(Fault::new(KIND_BUSY, "held")),
                gate: Some(GateResult {
                    drift_before: Some(0),
                    ratio: Some(0.25),
                    ..GateResult::default()
                }),
                config: Some(b"{\"log\": {}}".to_vec()),
                ..RolloutMember::default()
            }],
            ..Rollout::default()
        };
        r.count();
        let stored = r.json(true, true);
        assert!(
            stored.contains(r#""config":"eyJsb2ciOiB7fX0=""#),
            "{stored}"
        );
        assert!(stored.contains(r#""counts":{"applied":1}"#), "{stored}");
        let back = Rollout::from_value(&json::parse(stored.as_bytes()).unwrap());
        assert_eq!(back.json(true, true), stored);
        // What the API answers leaves the configs and, in a list, the changes out.
        let listed = r.json(false, false);
        assert!(
            !listed.contains("eyJ") && !listed.contains("\"changes\""),
            "{listed}"
        );
        assert!(
            listed.contains(r#""changed""#) || !listed.contains("have"),
            "{listed}"
        );
    }

    #[test]
    fn a_restart_pauses_what_ran() {
        let mk = |id: &str, state: &str| Rollout {
            id: id.into(),
            kind: KIND_SYNC.into(),
            state: state.into(),
            members: vec![RolloutMember::new(id, M_PENDING)],
            ..Rollout::default()
        };
        let list = [
            mk("a", RUNNING),
            mk("b", PLANNING),
            mk("c", ROLLING_BACK),
            mk("d", DONE),
        ]
        .iter()
        .map(|r| r.json(true, true))
        .collect::<Vec<_>>()
        .join(",");
        let d = Document {
            rollouts: format!("[{list}]"),
            ..Document::default()
        };
        let parsed = json::parse(d.marshal().as_slice()).unwrap();
        assert!(matches!(parsed.get("rollouts"), Some(Value::Array(a)) if a.len() == 4));
        let mut ro = Ro::default();
        let mut busy = HashMap::new();
        for v in match parsed.get("rollouts") {
            Some(Value::Array(a)) => a.clone(),
            _ => Vec::new(),
        } {
            let r = Rollout::from_value(&v);
            if r.holds() {
                hold(&r, &mut busy);
            }
            ro.list.push(r);
        }
        assert_eq!(busy.get("a").map(String::as_str), Some("rollout a"));
        assert!(!busy.contains_key("d"));
        release(&ro.list[0], &mut busy);
        assert!(!busy.contains_key("a"));
    }

    #[test]
    fn plans_expire_and_ended_ones_are_dropped() {
        let mut ro = Ro::default();
        let old = SystemTime::now() - PLAN_TTL - Duration::from_secs(1);
        ro.list.push(Rollout {
            id: "p".into(),
            state: PLANNED.into(),
            planned: Some(old),
            config: Some(Value::Object(Vec::new())),
            ..Rollout::default()
        });
        for i in 0..MAX_KEPT {
            ro.list.push(Rollout {
                id: format!("d{i}"),
                state: DONE.into(),
                ..Rollout::default()
            });
        }
        ro.expire();
        assert_eq!(ro.list.len(), MAX_KEPT);
        assert!(
            ro.get("p").is_none(),
            "the expired plan, the oldest ended, is dropped"
        );
        let mut ro = Ro::default();
        ro.list.push(Rollout {
            id: "p".into(),
            state: PLANNED.into(),
            planned: Some(old),
            config: Some(Value::Object(Vec::new())),
            ..Rollout::default()
        });
        ro.expire();
        let p = ro.get("p").unwrap();
        assert_eq!(p.state, EXPIRED);
        assert!(p.config.is_none());
        assert!(p.reason.contains("1h0m0s"), "{}", p.reason);
    }

    #[test]
    fn parts_and_secrets() {
        let frag = json::parse(
            br#"{"log": {"level": "info"}, "routes": [{"name": "a"}, {"name": "b", "x": "[redacted]"}]}"#,
        )
        .unwrap();
        let parts = fragment_parts(&unredacted(frag));
        assert_eq!(
            parts.iter().map(Rendered::display).collect::<Vec<_>>(),
            ["log", "routes[a]", "routes[b]"]
        );
        assert!(json::compact(&parts[2].value).contains("[secret]"));
        assert_eq!(
            parse_part("routes[api]").unwrap(),
            ("routes".to_owned(), "api".to_owned())
        );
        assert!(parse_part("admin").is_err());
    }
}
