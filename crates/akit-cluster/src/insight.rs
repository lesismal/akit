//! What the manager keeps of each poll beside the member's state (I139c, fit's `insight.go` and
//! `logtail.go`, its I150): a series of its counters, bounded per member, the certificates its
//! listeners serve, and the members' logs tailed together.
//!
//! The series and the certificates are kept on the member's node, so that a member removed, or
//! changed so that it gets a node of its own, takes them with it, and a member added later under
//! its name starts afresh. The series are read as rates between polls (requests, 4xx and 5xx a
//! second; the configs applied and failed in between) and as gauges (connections open, bytes
//! queued for clients), per member and summed per group; nothing is asked of a member that the
//! poller does not read already.
//!
//! A member's own log keeps its latest messages in a ring, which its Admin API answers at
//! `GET /logs/recent`, after a sequence number. A tail asks each member named for the messages
//! after the last it saw, at once, and answers them merged by time, with a cursor that holds
//! each member's last number: a page that follows the tail sends it back and gets what came
//! since. A member that restarted numbers its messages from 1 again, and answers from its start,
//! which its own endpoint does for a number past its last.
//!
//! ```text
//! GET /api/metrics?node=&group=&points=   the members' and groups' series
//! GET /api/certificates                   every member's certificates, the soonest to expire first
//! GET /api/logs?node=&level=&logger=&contains=&limit=&cursor=
//!                                         the members' logs merged by time
//! ```

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use akio_http::{Context, Method, Response};
use akit_config::value::Value;
use akit_roles::pages::{fail, json as answer};

use crate::groups;
use crate::json::{self, Obj, quoted};
use crate::member::FieldError;
use crate::node::STATUS_ERROR;
use crate::transport::{KIND_BAD_ANSWER, KIND_STATUS, KIND_UNSUPPORTED, Problem};
use crate::{MAX_READS, Manager, Refusal};

/// How many polls of each member the manager keeps: an hour at the default `poll_interval` of
/// 10s.
pub(crate) const SERIES_POINTS: usize = 360;

/// How many points `GET /api/metrics` answers when it is not told.
pub(crate) const DEFAULT_SERIES_POINTS: usize = 90;

/// The most certificates kept of a member.
pub(crate) const MAX_MEMBER_CERTS: usize = 256;

/// How many poll intervals apart two polls may be and still make a rate: farther apart, the
/// member was down or the manager stopped in between, and the point has none.
const GAP_POLLS: u64 = 3;

/// The capability of a member's `GET /logs/recent`.
pub(crate) const CAP_LOGS_RECENT: &str = "logs.recent";

/// How many members one tail reads.
pub(crate) const MAX_TAIL_NODES: usize = 16;

/// How many messages a tail asks each member for when it is not told.
pub(crate) const DEFAULT_TAIL_LIMIT: usize = 200;

/// The most a tail asks each member for: a member's ring holds 1000.
pub(crate) const MAX_TAIL_LIMIT: usize = 1000;

/// The most a cursor sent back may be.
const MAX_CURSOR_BYTES: usize = 4096;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One poll's counters.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Sample {
    /// Unix milliseconds.
    pub(crate) at: i64,
    /// The member's start, Unix milliseconds.
    pub(crate) started: i64,
    pub(crate) has_req: bool,
    pub(crate) req: u64,
    pub(crate) r4: u64,
    pub(crate) r5: u64,
    pub(crate) open: i64,
    pub(crate) pending: i64,
    /// Reloads that succeeded.
    pub(crate) ok: u64,
    /// Reloads that failed.
    pub(crate) failed: u64,
}

/// A certificate a member's listener serves, as the latest poll that read the member found it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Cert {
    listener: String,
    names: Vec<String>,
    issuer: String,
    serial: String,
    not_before: Option<SystemTime>,
    not_after: Option<SystemTime>,
}

/// A member's counters as its latest poll read them, as fit's `Counters`.
#[derive(Debug, Clone, Default)]
pub(crate) struct Counters {
    at: Option<SystemTime>,
    /// The requests' total and their classes, when the member counts them.
    requests: Option<(u64, BTreeMap<String, u64>)>,
    open: i64,
    refused: u64,
    pending: i64,
    succeeded: u64,
    failed: u64,
}

impl Counters {
    fn json(&self) -> String {
        let mut o = Obj::new().raw("at", &time_json(self.at));
        if let Some((total, classes)) = &self.requests {
            let mut c = Obj::new();
            for (k, v) in classes {
                c = c.num(k, v);
            }
            o = o.raw(
                "requests",
                &Obj::new()
                    .num("total", total)
                    .raw("classes", &c.end())
                    .end(),
            );
        }
        o.raw(
            "connections",
            &Obj::new()
                .num("open", self.open)
                .num("refused", self.refused)
                .num("pending_bytes", self.pending)
                .end(),
        )
        .raw(
            "reloads",
            &Obj::new()
                .num("succeeded", self.succeeded)
                .num("failed", self.failed)
                .end(),
        )
        .end()
    }
}

/// What the polls of a member read for the series and the inventory: the samples, a ring of
/// [`SERIES_POINTS`] once full, and the certificates of the latest poll that read them.
#[derive(Debug, Default)]
pub(crate) struct Insight {
    samples: Vec<Sample>,
    /// Where the next sample goes, once full.
    next: usize,
    certs: Vec<Cert>,
    certs_at: Option<SystemTime>,
    last: Option<Counters>,
}

impl Insight {
    /// The samples kept, oldest first.
    fn ordered(&self) -> Vec<Sample> {
        if self.samples.len() < SERIES_POINTS {
            return self.samples.clone();
        }
        let (a, b) = self.samples.split_at(self.next.min(self.samples.len()));
        let mut out = b.to_vec();
        out.extend_from_slice(a);
        out
    }

    fn add(&mut self, s: Sample) {
        if self.samples.len() < SERIES_POINTS {
            if self.samples.capacity() == 0 {
                // Its whole ring at once, so that adding never grows it past.
                self.samples.reserve_exact(SERIES_POINTS);
            }
            self.samples.push(s);
        } else if let Some(slot) = self.samples.get_mut(self.next) {
            *slot = s;
            self.next = self.next.saturating_add(1) % SERIES_POINTS;
        }
    }
}

fn unix_ms(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        Err(e) => i64::try_from(e.duration().as_millis()).map_or(i64::MIN, |m| m.saturating_neg()),
    }
}

/// `t` as Go's `time.Time` writes itself, or Go's zero time.
fn time_json(t: Option<SystemTime>) -> String {
    t.map_or_else(|| "\"0001-01-01T00:00:00Z\"".to_owned(), json::time)
}

fn time_of(v: &Value, key: &str) -> Option<SystemTime> {
    crate::rollouts::parse_time(json::str_of(v, key))
}

/// Keeps what a poll that read the member found: its `GET /node` and `/status`.
pub(crate) fn observe(ins: &Mutex<Insight>, now: SystemTime, node: &[u8], status: &[u8]) {
    let Some(st) = json::parse(status) else {
        return;
    };
    let started = json::parse(node)
        .and_then(|n| time_of(&n, "started"))
        .map_or(0, unix_ms);
    let field = |k: &str| st.get(k).cloned().unwrap_or(Value::Null);
    let (conns, reloads) = (field("connections"), field("reloads"));
    let mut s = Sample {
        at: unix_ms(now),
        started,
        open: json::int_of(&conns, "open"),
        pending: json::int_of(&conns, "pending_bytes"),
        ok: json::uint_of(&reloads, "succeeded"),
        failed: json::uint_of(&reloads, "failed"),
        ..Sample::default()
    };
    let mut last = Counters {
        at: Some(now),
        open: s.open,
        refused: json::uint_of(&conns, "refused"),
        pending: s.pending,
        succeeded: s.ok,
        failed: s.failed,
        requests: None,
    };
    if let Some(rq @ Value::Object(_)) = st.get("requests") {
        let classes = rq.get("classes").cloned().unwrap_or(Value::Null);
        s.has_req = true;
        s.req = json::uint_of(rq, "total");
        s.r4 = json::uint_of(&classes, "4xx");
        s.r5 = json::uint_of(&classes, "5xx");
        let mut kept = BTreeMap::new();
        if let Value::Object(members) = &classes {
            for (k, _) in members {
                if kept.len() < 8 {
                    kept.insert(k.clone(), json::uint_of(&classes, k));
                }
            }
        }
        last.requests = Some((s.req, kept));
    }
    let mut certs = Vec::new();
    if let Some(Value::Array(list)) = st.get("certificates") {
        for c in list.iter().take(MAX_MEMBER_CERTS) {
            certs.push(Cert {
                listener: crate::api::clip(json::str_of(c, "listener"), 128),
                names: json::strings_of(c, "names")
                    .iter()
                    .take(16)
                    .map(|n| crate::api::clip(n, 253))
                    .collect(),
                issuer: crate::api::clip(json::str_of(c, "issuer"), 256),
                serial: crate::api::clip(json::str_of(c, "serial"), 64),
                not_before: time_of(c, "not_before"),
                not_after: time_of(c, "not_after"),
            });
        }
    }
    let mut ins = lock(ins);
    ins.add(s);
    ins.certs = certs;
    ins.certs_at = Some(now);
    ins.last = Some(last);
}

/// A number of a series, or none: JSON `null`. Rounded to thousandths.
pub(crate) type Val = Option<f64>;

fn some(v: f64) -> Val {
    Some((v * 1000.0).round() / 1000.0)
}

fn val_json(b: &mut String, v: Val) {
    match v {
        Some(f) if f.is_finite() => b.push_str(&f.to_string()),
        _ => b.push_str("null"),
    }
}

/// How many value columns a series has: requests, 4xx, 5xx, connections, pending bytes, reloads,
/// reloads failed.
const COLUMNS: usize = 7;

/// The columns' names, in their order.
const COLUMN_NAMES: [&str; COLUMNS] = [
    "requests",
    "errors_4xx",
    "errors_5xx",
    "connections",
    "pending_bytes",
    "reloads",
    "reloads_failed",
];

/// A member's points, or a group's, as columns: `t` the time of each point, in Unix
/// milliseconds, and the values at it, `None` where there is none. Requests and errors are a
/// second, over the time since the poll before; reloads those since the poll before; connections
/// and pending bytes as the poll read them. A group's `members` counts the members summed at
/// each point. fit's `Series`.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Series {
    pub(crate) t: Vec<i64>,
    pub(crate) cols: [Vec<Val>; COLUMNS],
    pub(crate) members: Option<Vec<usize>>,
}

impl Series {
    fn push(&mut self, t: i64, vals: [Val; COLUMNS]) {
        self.t.push(t);
        for (c, v) in self.cols.iter_mut().zip(vals) {
            c.push(v);
        }
    }

    fn at(&self, col: usize, i: usize) -> Val {
        self.cols.get(col).and_then(|c| c.get(i)).copied().flatten()
    }

    pub(crate) fn json(&self) -> String {
        let mut b = String::from("{\"t\":[");
        for (i, t) in self.t.iter().enumerate() {
            if i > 0 {
                b.push(',');
            }
            b.push_str(&t.to_string());
        }
        b.push(']');
        for (name, col) in COLUMN_NAMES.iter().zip(&self.cols) {
            b.push_str(",\"");
            b.push_str(name);
            b.push_str("\":[");
            for (i, v) in col.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                val_json(&mut b, *v);
            }
            b.push(']');
        }
        if let Some(m) = &self.members {
            b.push_str(",\"members\":");
            b.push_str(&format!(
                "[{}]",
                m.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        b.push('}');
        b
    }
}

/// The series of `samples`, the last `n` of them: each point's rates from the sample before,
/// when that is the same run of the member (its start unchanged, its counters not gone back) and
/// at most [`GAP_POLLS`] intervals before. fit's `points`.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn points(samples: &[Sample], n: usize, interval: Duration) -> Series {
    let mut out = Series::default();
    let from = samples.len().saturating_sub(n);
    let gap = i64::try_from(interval.as_millis())
        .unwrap_or(i64::MAX)
        .saturating_mul(GAP_POLLS.try_into().unwrap_or(3));
    for i in from..samples.len() {
        let Some(s) = samples.get(i) else { break };
        let mut vals: [Val; COLUMNS] = [None; COLUMNS];
        vals[3] = some(s.open as f64);
        vals[4] = some(s.pending as f64);
        if let Some(p) = i.checked_sub(1).and_then(|j| samples.get(j)) {
            let span = s.at.saturating_sub(p.at);
            let dt = span as f64 / 1000.0;
            if span > 0 && span <= gap && s.started == p.started {
                if s.has_req && p.has_req && s.req >= p.req && s.r4 >= p.r4 && s.r5 >= p.r5 {
                    vals[0] = some(s.req.saturating_sub(p.req) as f64 / dt);
                    vals[1] = some(s.r4.saturating_sub(p.r4) as f64 / dt);
                    vals[2] = some(s.r5.saturating_sub(p.r5) as f64 / dt);
                }
                if s.ok >= p.ok && s.failed >= p.failed {
                    vals[5] = some(s.ok.saturating_sub(p.ok) as f64);
                    vals[6] = some(s.failed.saturating_sub(p.failed) as f64);
                }
            }
        }
        out.push(s.at, vals);
    }
    out
}

/// Sums the members' series into buckets of `interval`: each member's latest point in a bucket
/// counts, and a value is the sum of the members that have one there, none when no member has.
/// fit's `sumGroup`.
pub(crate) fn sum_group(members: &[Series], interval: Duration) -> Series {
    let w = i64::try_from(interval.as_millis())
        .unwrap_or(i64::MAX)
        .max(1);
    let mut buckets: BTreeMap<i64, ([Val; COLUMNS], usize)> = BTreeMap::new();
    for s in members {
        // A member's latest point in each bucket.
        let mut seen: HashMap<i64, usize> = HashMap::new();
        for (i, t) in s.t.iter().enumerate() {
            seen.insert(t.div_euclid(w), i);
        }
        for (b, i) in seen {
            let bk = buckets.entry(b).or_insert(([None; COLUMNS], 0));
            bk.1 = bk.1.saturating_add(1);
            for (c, slot) in bk.0.iter_mut().enumerate() {
                if let Some(v) = s.at(c, i) {
                    *slot = some(slot.unwrap_or(0.0) + v);
                }
            }
        }
    }
    let mut out = Series {
        members: Some(Vec::new()),
        ..Series::default()
    };
    for (b, (vals, n)) in buckets {
        out.push(b.saturating_mul(w), vals);
        if let Some(m) = &mut out.members {
            m.push(n);
        }
    }
    out
}

/// Which series [`Manager::metrics`] returns: those of `nodes` and of `groups`, by name, every
/// member's and every group's when both are empty; the last `points` of each.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct MetricsQuery {
    pub(crate) nodes: Vec<String>,
    pub(crate) groups: Vec<String>,
    pub(crate) points: usize,
}

/// A query's parameters, `+` and percent escapes decoded, as Go's `url.Values`: every value of
/// each name, in order.
pub(crate) fn query_values(query: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for kv in query.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        let dec = |s: &str| crate::api::percent_decode(&s.replace('+', " "));
        out.push((dec(k), dec(v)));
    }
    out
}

/// The first value of `name`, or "".
pub(crate) fn first<'q>(q: &'q [(String, String)], name: &str) -> &'q str {
    q.iter()
        .find(|(k, _)| k == name)
        .map_or("", |(_, v)| v.as_str())
}

/// The values of `name`, each split at commas, without empty ones or repeats: fit's `listParam`.
pub(crate) fn list_param(q: &[(String, String)], name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (k, v) in q {
        if k != name {
            continue;
        }
        for p in v.split(',') {
            let p = p.trim();
            if !p.is_empty() && !out.iter().any(|o| o == p) {
                out.push(p.to_owned());
            }
        }
    }
    out
}

fn field(name: &str, msg: String) -> Refusal {
    Refusal::Field(FieldError {
        field: name.to_owned(),
        msg,
    })
}

/// A [`MetricsQuery`] from a request's query: `node` and `group`, repeated or separated by
/// commas, and `points`.
pub(crate) fn metrics_query(query: &str) -> Result<MetricsQuery, Refusal> {
    let q = query_values(query);
    let mut out = MetricsQuery {
        nodes: list_param(&q, "node"),
        groups: list_param(&q, "group"),
        points: 0,
    };
    let p = first(&q, "points");
    if !p.is_empty() {
        match p.parse::<usize>() {
            Ok(n) if (1..=SERIES_POINTS).contains(&n) => out.points = n,
            _ => return Err(field("points", format!("points is 1 to {SERIES_POINTS}"))),
        }
    }
    Ok(out)
}

impl Manager {
    /// The members' and groups' series `q` asks for, as `GET /api/metrics` answers them; a name
    /// there is no member or group of is the error. fit's `Metrics`.
    pub(crate) fn metrics(&self, q: &MetricsQuery) -> Result<String, Refusal> {
        let n = match q.points {
            0 => DEFAULT_SERIES_POINTS,
            p => p.min(SERIES_POINTS),
        };
        let all = q.nodes.is_empty() && q.groups.is_empty();
        let d = self.doc();
        for name in &q.nodes {
            if d.index(name).is_none() {
                return Err(Refusal::NotFound(format!(
                    "no member is named {}",
                    quoted(name)
                )));
            }
        }
        for g in &q.groups {
            if !d.groups.iter().any(|x| &x.name == g) {
                return Err(Refusal::NotFound(format!(
                    "no group is named {}",
                    quoted(g)
                )));
            }
        }
        let mut gs: Vec<(String, Vec<String>)> = d
            .groups
            .iter()
            .filter(|g| all || q.groups.contains(&g.name))
            .map(|g| (g.name.clone(), groups::nodes_of(&d, g)))
            .collect();
        gs.sort_by(|a, b| a.0.cmp(&b.0));
        let mut members: Vec<&crate::member::Member> = d.members.iter().collect();
        members.sort_by(|a, b| a.name.cmp(&b.name));
        let interval = self.interval();
        let mut by_name: HashMap<String, Series> = HashMap::new();
        let mut nodes = Vec::new();
        for mem in members {
            let shown = all || q.nodes.contains(&mem.name);
            if !shown && !gs.iter().any(|(_, ns)| ns.contains(&mem.name)) {
                continue;
            }
            let node = self.node(&mem.name);
            let status = node.as_ref().map_or(STATUS_ERROR, |x| x.status());
            let (series, counters) = match &node {
                Some(x) => {
                    let ins = lock(&x.ins);
                    let samples = ins.ordered();
                    let c = ins.last.as_ref().map(Counters::json);
                    drop(ins);
                    (points(&samples, n, interval), c)
                }
                None => (Series::default(), None),
            };
            if shown {
                let groups = groups::groups_of(&d, &mem.name, &mem.labels);
                nodes.push(
                    Obj::new()
                        .str("name", &mem.name)
                        .str("status", status)
                        .raw("groups", &json::string_list(&groups))
                        .opt("counters", counters.as_deref())
                        .raw("series", &series.json())
                        .end(),
                );
            }
            by_name.insert(mem.name.clone(), series);
        }
        let mut groups_out = Vec::new();
        for (name, ns) in &gs {
            let picked: Vec<Series> = ns.iter().filter_map(|x| by_name.get(x).cloned()).collect();
            groups_out.push(
                Obj::new()
                    .str("name", name)
                    .raw("nodes", &json::string_list(ns))
                    .raw("series", &sum_group(&picked, interval).json())
                    .end(),
            );
        }
        Ok(Obj::new()
            .num("interval_ms", json::millis(interval))
            .num("points_max", SERIES_POINTS)
            .raw("now", &json::time(SystemTime::now()))
            .raw("nodes", &format!("[{}]", nodes.join(",")))
            .raw("groups", &format!("[{}]", groups_out.join(",")))
            .end())
    }

    /// The certificates' inventory, as `GET /api/certificates` answers it: every member's
    /// certificates as its latest poll that read them found them, sorted by expiry, the soonest
    /// first; and each member, how many it serves and when they were read. fit's `Certificates`.
    pub(crate) fn certificates(&self) -> String {
        let now = SystemTime::now();
        let mut certs: Vec<(Option<SystemTime>, String, String, String)> = Vec::new();
        let mut nodes = Vec::new();
        for mem in self.members() {
            let mut o = Obj::new().str("name", &mem.name);
            let Some(n) = self.node(&mem.name) else {
                nodes.push(o.str("status", STATUS_ERROR).num("count", 0).end());
                continue;
            };
            let status = n.status();
            let ins = lock(&n.ins);
            o = o.str("status", status).num("count", ins.certs.len());
            if let Some(at) = ins.certs_at {
                o = o.raw("read", &json::time(at));
            }
            let read = time_json(ins.certs_at);
            for c in &ins.certs {
                let body = Obj::new()
                    .str("node", &mem.name)
                    .str("listener", &c.listener)
                    .raw("names", &json::string_list(&c.names))
                    .str("issuer", &c.issuer)
                    .str("serial", &c.serial)
                    .raw("not_before", &time_json(c.not_before))
                    .raw("not_after", &time_json(c.not_after))
                    .num("days_left", days_left(c.not_after, now))
                    .str("status", status)
                    .raw("read", &read)
                    .end();
                certs.push((c.not_after, mem.name.clone(), c.listener.clone(), body));
            }
            drop(ins);
            nodes.push(o.end());
        }
        // Go's zero time, a certificate without its expiry read, sorts first.
        certs.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        Obj::new()
            .raw("now", &json::time(now))
            .raw(
                "certificates",
                &format!(
                    "[{}]",
                    certs.into_iter().map(|c| c.3).collect::<Vec<_>>().join(",")
                ),
            )
            .raw("nodes", &format!("[{}]", nodes.join(",")))
            .end()
    }
}

/// How many whole days are left before `not_after`, less than zero once it has passed: Go's
/// `int(math.Floor(NotAfter.Sub(now).Hours() / 24))`, its duration saturated as Go's is.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
pub(crate) fn days_left(not_after: Option<SystemTime>, now: SystemTime) -> i64 {
    // Go's longest duration, in seconds.
    const MAX_SECS: f64 = 9_223_372_036.854_775;
    let secs = match not_after {
        Some(t) => match t.duration_since(now) {
            Ok(d) => d.as_secs_f64(),
            Err(e) => -e.duration().as_secs_f64(),
        },
        None => -MAX_SECS,
    };
    (secs.clamp(-MAX_SECS, MAX_SECS) / 86_400.0).floor() as i64
}

// ---- Logs tailed ----

/// A tail: the members, the filter each member's ring applies, and `cursor`, what the tail
/// before answered, or "" for the latest. fit's `TailQuery`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TailQuery {
    pub(crate) nodes: Vec<String>,
    pub(crate) logger: String,
    pub(crate) level: String,
    pub(crate) contains: String,
    pub(crate) limit: usize,
    pub(crate) cursor: String,
}

/// A [`TailQuery`] from a request's query: `node` (repeated or separated by commas), `logger`,
/// `level`, `contains`, `limit` and `cursor`.
pub(crate) fn tail_query(query: &str) -> Result<TailQuery, Refusal> {
    let q = query_values(query);
    let mut out = TailQuery {
        nodes: list_param(&q, "node"),
        logger: first(&q, "logger").to_owned(),
        level: first(&q, "level").to_owned(),
        contains: first(&q, "contains").to_owned(),
        cursor: first(&q, "cursor").to_owned(),
        limit: 0,
    };
    let l = first(&q, "limit");
    if !l.is_empty() {
        match l.parse::<usize>() {
            Ok(n) if n >= 1 => out.limit = n,
            _ => return Err(field("limit", format!("limit is 1 to {MAX_TAIL_LIMIT}"))),
        }
    }
    Ok(out)
}

/// Each member's last sequence number, as a cursor carries it.
pub(crate) type Cursor = BTreeMap<String, u64>;

/// The cursor written: JSON, base64 for URLs without padding, as fit's.
pub(crate) fn encode_cursor(c: &Cursor) -> String {
    let mut o = Obj::new();
    for (k, v) in c {
        o = o.num(k, v);
    }
    akio_crypto::base64::URL_SAFE_NO_PAD.encode(o.end())
}

/// A cursor read back, or why it is not one a tail answered.
pub(crate) fn decode_cursor(s: &str) -> Result<Cursor, Refusal> {
    let mut c = Cursor::new();
    if s.is_empty() {
        return Ok(c);
    }
    let bad = || field("cursor", "the cursor is not one a tail answered".to_owned());
    if s.len() > MAX_CURSOR_BYTES {
        return Err(bad());
    }
    let data = akio_crypto::base64::URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|_| bad())?;
    match json::parse(&data) {
        Some(Value::Object(members)) => {
            for (k, v) in members {
                match v {
                    Value::Number(n) => {
                        c.insert(k, n.parse::<u64>().map_err(|_| bad())?);
                    }
                    _ => return Err(bad()),
                }
            }
            Ok(c)
        }
        Some(Value::Null) => Ok(c),
        _ => Err(bad()),
    }
}

impl TailQuery {
    /// Checks the query, filling in its defaults, and returns its cursor. fit's `check`.
    fn check(&mut self, m: &Manager) -> Result<Cursor, Refusal> {
        if self.nodes.is_empty() {
            return Err(field(
                "node",
                "name the members whose logs to tail".to_owned(),
            ));
        }
        if self.nodes.len() > MAX_TAIL_NODES {
            return Err(field(
                "node",
                format!("a tail reads {MAX_TAIL_NODES} members at most"),
            ));
        }
        if !matches!(
            self.level.as_str(),
            "" | "debug" | "info" | "warn" | "error"
        ) {
            return Err(field(
                "level",
                "level is debug, info, warn or error".to_owned(),
            ));
        }
        if self.contains.len() > 256 || self.logger.len() > 64 {
            return Err(field(
                "contains",
                "contains is 256 bytes at most, and logger 64".to_owned(),
            ));
        }
        if self.limit > MAX_TAIL_LIMIT {
            return Err(field("limit", format!("limit is 1 to {MAX_TAIL_LIMIT}")));
        }
        if self.limit == 0 {
            self.limit = DEFAULT_TAIL_LIMIT;
        }
        for n in &self.nodes {
            if m.node(n).is_none() {
                return Err(Refusal::NotFound(format!(
                    "no member is named {}",
                    quoted(n)
                )));
            }
        }
        decode_cursor(&self.cursor)
    }
}

/// A member's messages, as its `GET /logs/recent` gave them.
#[derive(Debug, Clone)]
struct Rec {
    /// When, for the merge; `None` sorts first.
    time: Option<SystemTime>,
    seq: u64,
    /// The record, as the member wrote it, but for `node` put first.
    json: String,
}

/// How reading one member went.
#[derive(Debug, Clone, Default)]
struct Out {
    recs: Vec<Rec>,
    last: u64,
    problem: Option<Problem>,
    done: bool,
}

/// The record `r` of member `node` as a tail answers it: `{"node", ...the record's members}`.
fn tail_record(node: &str, r: &Value) -> Rec {
    let mut o = Obj::new().str("node", node);
    if let Value::Object(members) = r {
        for (k, v) in members {
            if k != "node" {
                o = o.raw(k, &json::compact(v));
            }
        }
    }
    Rec {
        time: time_of(r, "time"),
        seq: json::uint_of(r, "seq"),
        json: o.end(),
    }
}

/// The error a JSON answer holds, or its start.
fn error_of(body: &[u8]) -> String {
    let e = json::parse(body)
        .map(|v| json::str_of(&v, "error").to_owned())
        .unwrap_or_default();
    if e.is_empty() {
        crate::api::clip(&String::from_utf8_lossy(body), 128)
    } else {
        e
    }
}

/// What a tail calls when it ends: its answer, or why it was refused.
pub(crate) type TailDone = Box<dyn FnOnce(Result<String, Refusal>) + Send>;

impl Manager {
    /// Reads the latest messages of `q`'s members, after its cursor, and calls `done` once with
    /// them merged by time, the oldest first, with the cursor for what comes next and each
    /// member's outcome; from an I/O thread or the caller's, so it must not block. Never blocks
    /// itself, and checks nothing of the user's roles: its callers do. fit's `TailLogs`.
    pub(crate) fn tail_logs(&self, mut q: TailQuery, done: TailDone) {
        let cur = match q.check(self) {
            Ok(c) => c,
            Err(e) => return done(Err(e)),
        };
        let nodes = q.nodes.clone();
        let outs: Arc<Mutex<(Vec<Out>, Option<TailDone>)>> =
            Arc::new(Mutex::new((vec![Out::default(); nodes.len()], Some(done))));
        let (cur, names) = (Arc::new(cur), Arc::new(nodes));
        let end = {
            let (cur, names, outs) = (Arc::clone(&cur), Arc::clone(&names), Arc::clone(&outs));
            Arc::new(move |i: usize, out: Out| {
                let mut g = lock(&outs);
                if let Some(slot) = g.0.get_mut(i) {
                    *slot = Out { done: true, ..out };
                }
                if !g.0.iter().all(|o| o.done) {
                    return;
                }
                let Some(done) = g.1.take() else { return };
                let all = std::mem::take(&mut g.0);
                drop(g);
                done(Ok(finish_tail(&names, &cur, all)));
            })
        };
        let mut query = Vec::new();
        for (k, v) in [
            ("logger", &q.logger),
            ("level", &q.level),
            ("contains", &q.contains),
        ] {
            if !v.is_empty() {
                query.push(format!("{k}={}", query_escape(v)));
            }
        }
        query.push(format!("limit={}", q.limit));
        for (i, name) in names.iter().enumerate() {
            let after = cur.get(name).copied().unwrap_or(0);
            if let Some(caps) = self.node(name).and_then(|n| n.caps()) {
                if !caps.iter().any(|c| c == CAP_LOGS_RECENT) {
                    end(
                        i,
                        Out {
                            last: after,
                            problem: Some(Problem::new(
                                KIND_UNSUPPORTED,
                                format!(
                                    "the member does not list {CAP_LOGS_RECENT} in its \
                                     capabilities: it is older than the cluster's logs"
                                ),
                            )),
                            ..Out::default()
                        },
                    );
                    continue;
                }
            }
            let mut qi = query.clone();
            if after > 0 {
                qi.push(format!("after={after}"));
            }
            // Sorted, as Go's url.Values.Encode writes them.
            qi.sort();
            let (end, name2) = (Arc::clone(&end), name.clone());
            self.call_member_raw(
                name,
                "GET",
                &format!("/logs/recent?{}", qi.join("&")),
                Vec::new(),
                None,
                Box::new(move |r| {
                    let failed = |p: Problem| Out {
                        last: after,
                        problem: Some(p),
                        ..Out::default()
                    };
                    let out = match r {
                        Err(p) => failed(p),
                        Ok(rep) if rep.status != 200 => failed(
                            Problem::new(
                                KIND_STATUS,
                                crate::api::clip(
                                    &format!(
                                        "GET /logs/recent answered {}: {}",
                                        rep.status,
                                        error_of(&rep.body)
                                    ),
                                    crate::node::MAX_MESSAGE,
                                ),
                            )
                            .with_status(rep.status),
                        ),
                        Ok(rep) => match json::parse(&rep.body) {
                            Some(v @ Value::Object(_)) => {
                                let recs = match v.get("records") {
                                    Some(Value::Array(a)) => a.clone(),
                                    _ => Vec::new(),
                                };
                                let skip = recs.len().saturating_sub(MAX_TAIL_LIMIT);
                                Out {
                                    recs: recs
                                        .iter()
                                        .skip(skip)
                                        .map(|r| tail_record(&name2, r))
                                        .collect(),
                                    last: json::uint_of(&v, "last"),
                                    ..Out::default()
                                }
                            }
                            _ => failed(
                                Problem::new(
                                    KIND_BAD_ANSWER,
                                    "the member's /logs/recent is not the JSON expected",
                                )
                                .with_status(rep.status),
                            ),
                        },
                    };
                    end(i, out);
                }),
            );
        }
    }
}

/// The tail's answer from each member's outcome, fit's `finish`.
fn finish_tail(names: &[String], cur: &Cursor, outs: Vec<Out>) -> String {
    let mut next = Cursor::new();
    let mut recs: Vec<(&str, Rec)> = Vec::new();
    let mut nodes = Vec::new();
    for (name, out) in names.iter().zip(outs) {
        let last = if out.problem.is_none() {
            out.last
        } else {
            cur.get(name).copied().unwrap_or(0)
        };
        next.insert(name.clone(), last);
        nodes.push(
            Obj::new()
                .str("name", name)
                .num("count", out.recs.len())
                .num("last", out.last)
                .opt("error", out.problem.as_ref().map(Problem::json).as_deref())
                .end(),
        );
        for r in out.recs {
            recs.push((name.as_str(), r));
        }
    }
    recs.sort_by(|a, b| {
        a.1.time
            .cmp(&b.1.time)
            .then(a.0.cmp(b.0))
            .then(a.1.seq.cmp(&b.1.seq))
    });
    Obj::new()
        .raw(
            "records",
            &format!(
                "[{}]",
                recs.into_iter()
                    .map(|(_, r)| r.json)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        )
        .str("cursor", &encode_cursor(&next))
        .raw("nodes", &format!("[{}]", nodes.join(",")))
        .end()
}

/// `s` escaped for a query's value, as Go's `url.QueryEscape`.
pub(crate) fn query_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.bytes() {
        match c {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(c));
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{c:02X}")),
        }
    }
    out
}

// ---- The API ----

/// `GET /api/metrics`, `/api/certificates` and `/api/logs`, when `path` is one. The series and
/// the inventory are what the poller reads, as the members' states are: any role reads them, as
/// it reads every state. A member's logs are its own read, which needs a role on it.
pub(crate) fn serve(m: &Manager, ctx: &mut Context, user: &str, path: &str) -> bool {
    let get = matches!(ctx.method(), Method::Get | Method::Head);
    match path {
        "/api/metrics" | "/api/certificates" | "/api/logs" if !get => {
            ctx.respond(fail(405, "the method is not allowed").header("Allow", "GET"));
        }
        "/api/metrics" => ctx.respond(metrics_route(m, ctx.req().query().unwrap_or(""))),
        "/api/certificates" => ctx.respond(answer(200, m.certificates())),
        "/api/logs" => logs_route(m, ctx, Some(user)),
        _ => return false,
    }
    true
}

pub(crate) fn metrics_route(m: &Manager, query: &str) -> Response {
    match metrics_query(query).and_then(|q| m.metrics(&q)) {
        Ok(body) => answer(200, body),
        Err(e) => e.response(),
    }
}

/// `GET /api/logs`: the tail of the members named, which `user` needs a role on each of (none
/// for the Admin API, a cluster-wide admin); one of the reads of members ([`MAX_READS`]) while
/// it runs.
pub(crate) fn logs_route(m: &Manager, ctx: &mut Context, user: Option<&str>) {
    let q = match tail_query(ctx.req().query().unwrap_or("")) {
        Ok(q) => q,
        Err(e) => return ctx.respond(e.response()),
    };
    if let Some(user) = user {
        let need = crate::roles::Need::on(crate::roles::VIEWER, q.nodes.clone());
        if let Err(e) = m.allow(user, &need) {
            return ctx.respond(e.response());
        }
    }
    if m.0.reads.fetch_add(1, Ordering::AcqRel) >= MAX_READS {
        m.0.reads.fetch_sub(1, Ordering::AcqRel);
        return ctx.respond(fail(429, "other reads of members are under way; try again"));
    }
    let Some(d) = ctx.detach() else {
        m.0.reads.fetch_sub(1, Ordering::AcqRel);
        return;
    };
    let me = m.clone();
    m.tail_logs(
        q,
        Box::new(move |r| {
            me.0.reads.fetch_sub(1, Ordering::AcqRel);
            d.respond(match r {
                Ok(body) => answer(200, body),
                Err(e) => e.response(),
            });
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(at: i64, started: i64, req: Option<u64>, open: i64) -> Sample {
        Sample {
            at,
            started,
            has_req: req.is_some(),
            req: req.unwrap_or(0),
            open,
            ..Sample::default()
        }
    }

    // fit's TestPoints: rates from the poll before, none across a restart, a gap or counters gone
    // back, gauges as read; a group summed a bucket at a time.
    #[test]
    fn points_and_groups() {
        let iv = Duration::from_secs(10);
        let s = [
            Sample {
                at: 0,
                started: 1,
                has_req: true,
                req: 100,
                r4: 10,
                r5: 1,
                open: 5,
                pending: 7,
                ok: 2,
                failed: 0,
            },
            Sample {
                at: 10_000,
                started: 1,
                has_req: true,
                req: 150,
                r4: 20,
                r5: 6,
                open: 6,
                pending: 8,
                ok: 3,
                failed: 1,
            },
            sample(20_000, 2, Some(5), 1),   // restarted
            sample(30_000, 2, Some(25), 1),  // 2/s
            sample(100_000, 2, Some(95), 1), // a gap
            sample(110_000, 2, None, 2),     // no counts
            sample(120_000, 2, Some(90), 2), // after none
            sample(130_000, 2, Some(80), 2), // gone back
        ];
        let p = points(&s, 100, iv);
        assert_eq!(p.t.len(), s.len());
        assert_eq!(p.at(0, 0), None);
        assert_eq!(
            (p.at(0, 1), p.at(1, 1), p.at(2, 1), p.at(5, 1), p.at(6, 1)),
            (Some(5.0), Some(1.0), Some(0.5), Some(1.0), Some(1.0))
        );
        assert_eq!((p.at(3, 1), p.at(4, 1)), (Some(6.0), Some(8.0)));
        for (i, ok) in [false, true, false, true, false, false, false, false]
            .iter()
            .enumerate()
        {
            assert_eq!(p.at(0, i).is_some(), *ok, "point {i}");
        }
        assert_eq!(p.at(0, 3), Some(2.0));
        let last = points(&s, 2, iv);
        assert_eq!(last.t, [120_000, 130_000]);
        let two = points(s.get(..2).unwrap_or(&[]), 2, iv).json();
        assert!(two.contains("\"requests\":[null,5]"), "{two}");

        let a = points(
            &[
                sample(1_000, 0, None, 1),
                sample(11_000, 0, Some(0), 2),
                sample(12_000, 0, None, 3),
            ],
            10,
            iv,
        );
        let b = points(
            &[sample(2_000, 0, None, 10), sample(21_000, 0, None, 20)],
            10,
            iv,
        );
        let g = sum_group(&[a, b], iv);
        assert_eq!(g.t, [0, 10_000, 20_000]);
        assert_eq!(g.members.as_deref(), Some(&[2, 1, 1][..]));
        assert_eq!(
            (g.at(3, 0), g.at(3, 1), g.at(3, 2), g.at(0, 0)),
            (Some(11.0), Some(3.0), Some(20.0), None)
        );
        let e = sum_group(&[], iv);
        assert!(e.t.is_empty());
        assert!(e.json().contains("\"members\":[]"), "{}", e.json());
    }

    #[test]
    fn cursors_and_queries() {
        let mut c = Cursor::new();
        c.insert("b".to_owned(), 4);
        c.insert("a".to_owned(), 17);
        let s = encode_cursor(&c);
        assert_eq!(
            akio_crypto::base64::URL_SAFE_NO_PAD.decode(&s).ok(),
            Some(b"{\"a\":17,\"b\":4}".to_vec())
        );
        assert_eq!(decode_cursor(&s).ok(), Some(c));
        assert_eq!(decode_cursor("").ok(), Some(Cursor::new()));
        for bad in [
            "!!".to_owned(),
            akio_crypto::base64::URL_SAFE_NO_PAD.encode("{\"a\":-1}"),
            akio_crypto::base64::URL_SAFE_NO_PAD.encode("[1]"),
            "a".repeat(MAX_CURSOR_BYTES.saturating_add(4)),
        ] {
            assert!(decode_cursor(&bad).is_err(), "{bad}");
        }
        let q = metrics_query("node=a,b&node=a&group=+edge+&points=5").ok();
        assert_eq!(
            q,
            Some(MetricsQuery {
                nodes: vec!["a".to_owned(), "b".to_owned()],
                groups: vec!["edge".to_owned()],
                points: 5,
            })
        );
        for bad in ["points=0", "points=x", "points=361"] {
            assert!(metrics_query(bad).is_err(), "{bad}");
        }
        let t = tail_query("node=a%2Cb&level=warn&contains=two+words&limit=3&cursor=x").ok();
        assert_eq!(
            t,
            Some(TailQuery {
                nodes: vec!["a".to_owned(), "b".to_owned()],
                level: "warn".to_owned(),
                contains: "two words".to_owned(),
                limit: 3,
                cursor: "x".to_owned(),
                logger: String::new(),
            })
        );
        assert!(tail_query("node=a&limit=0").is_err());
        assert_eq!(query_escape("a b/é"), "a+b%2F%C3%A9");
    }

    #[test]
    fn days_as_go_counts_them() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        let day = Duration::from_secs(86_400);
        assert_eq!(
            days_left(Some(now + day * 3 + Duration::from_secs(3600)), now),
            3
        );
        assert_eq!(days_left(Some(now - Duration::from_secs(1)), now), -1);
        assert_eq!(days_left(None, now), -106_752);
    }
}
