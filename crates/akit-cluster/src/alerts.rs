//! Alerts (I138, fit's `cluster/alerts.go` and `alertapi.go`, its I147): rules over the members'
//! states (a member down, drift, a rollout failed, a certificate near its expiry, upstream
//! backends down, reloads failing), each firing once its condition has held for its time,
//! repeated to its webhooks, resolved once the condition is gone; silences by rule, member or
//! both; webhooks in fit's formats ([`crate::webhooks`]). The rules, webhooks and silences, the
//! alerts firing and the last resolved are kept in the store's section `cluster.alerts`, fit's
//! document.
//!
//! ```text
//! GET    /api/alerts                     the alerts firing, pending and resolved, with counts
//! GET    /api/alerts/summary             the counts
//! GET    /api/alerts/config              the rules, webhooks and silences, with their ETag
//! POST   /api/alerts/rules               a rule added (If-Match the ETag, when sent)
//! PUT    /api/alerts/rules/<name>        a rule changed, or renamed
//! DELETE /api/alerts/rules/<name>        a rule removed
//! POST   /api/alerts/webhooks            a webhook added
//! PUT    /api/alerts/webhooks/<name>     a webhook changed; an empty secret keeps the one kept
//! DELETE /api/alerts/webhooks/<name>     a webhook removed, once no rule sends to it
//! POST   /api/alerts/webhooks/<name>/test  a test sent, answered with how it went
//! POST   /api/alerts/silences            a silence added
//! DELETE /api/alerts/silences/<id>       a silence ended now
//! GET    /api/drift                      each member's drift: its templates and its check
//! POST   /api/drift/fix                  a sync rollout of members planned, to bring them in sync
//! ```

// A member's problem is part of what is answered, made once per request: its size does not
// matter.
#![allow(clippy::result_large_err)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use akio_http::{Context, Method, Response};
use akit_config::value::Value;
use akit_roles::pages::{fail, json as answer};

use crate::api::clip;
use crate::json::{self, Obj, quoted};
use crate::member::{FieldError, MAX_BODY_BYTES, MAX_NAME_BYTES, name_ok};
use crate::rollouts::{go_duration, parse_time, whole};
use crate::webhooks::{self, Sender, Webhook};
use crate::{Manager, Refusal};

/// The store's section the alerts are kept in, fit's.
pub const SECTION: &str = "cluster.alerts";
/// The version of the document this akit writes.
pub const VERSION: i64 = 1;

/// The kinds of rules.
pub const MEMBER_DOWN: &str = "member_down";
/// See [`MEMBER_DOWN`].
pub const DRIFT: &str = "drift";
/// See [`MEMBER_DOWN`].
pub const ROLLOUT_FAILED: &str = "rollout_failed";
/// See [`MEMBER_DOWN`].
pub const CERT_EXPIRY: &str = "cert_expiry";
/// See [`MEMBER_DOWN`].
pub const BACKENDS_DOWN: &str = "backends_down";
/// See [`MEMBER_DOWN`].
pub const RELOAD_FAILED: &str = "reload_failed";
/// Every kind, in fit's order.
pub const KINDS: &[&str] = &[
    MEMBER_DOWN,
    DRIFT,
    ROLLOUT_FAILED,
    CERT_EXPIRY,
    BACKENDS_DOWN,
    RELOAD_FAILED,
];
/// The severities, least first.
pub const SEVERITIES: &[&str] = &["info", "warning", "critical"];
/// What a drift rule watches: its templates, its consistency check, or both (empty).
pub const DRIFT_TEMPLATES: &str = "templates";
/// See [`DRIFT_TEMPLATES`].
pub const DRIFT_CONSISTENCY: &str = "consistency";

/// An alert's states.
pub const PENDING: &str = "pending";
/// See [`PENDING`].
pub const FIRING: &str = "firing";
/// See [`PENDING`].
pub const RESOLVED: &str = "resolved";

/// The bounds.
pub const MAX_RULES: usize = 64;
/// See [`MAX_RULES`].
pub const MAX_WEBHOOKS: usize = 16;
/// See [`MAX_RULES`].
pub const MAX_SILENCES: usize = 256;
/// See [`MAX_RULES`].
pub const MAX_ACTIVE: usize = 2048;
/// See [`MAX_RULES`].
pub const MAX_HISTORY: usize = 500;
pub(crate) const MAX_TEXT: usize = 512;
const MAX_SILENCE_SPAN: Duration = Duration::from_secs(30 * 24 * 3600);
const SILENCE_KEPT: Duration = Duration::from_secs(7 * 24 * 3600);
/// How often an alert is sent again while it fires, by default.
pub const DEFAULT_REPEAT_MS: i64 = 4 * 3600 * 1000;
/// The days before a certificate's expiry a rule fires, by default.
pub const DEFAULT_CERT_DAYS: i64 = 14;
const MAX_FOR_MS: i64 = 24 * 3600 * 1000;
const MIN_REPEAT_MS: i64 = 60 * 1000;
const MAX_REPEAT_MS: i64 = 7 * 24 * 3600 * 1000;
const MIN_RESOLVE_MS: i64 = 60 * 1000;
const MAX_RESOLVE_MS: i64 = 30 * 24 * 3600 * 1000;
const MAX_CERT_DAYS: i64 = 365;
const MAX_BACKENDS: i64 = 10_000;

/// The audit log's actions.
const RULE_ADD: &str = "alert.rule.add";
const RULE_UPDATE: &str = "alert.rule.update";
const RULE_REMOVE: &str = "alert.rule.remove";
const WEBHOOK_ADD: &str = "alert.webhook.add";
const WEBHOOK_UPDATE: &str = "alert.webhook.update";
const WEBHOOK_REMOVE: &str = "alert.webhook.remove";
pub(crate) const WEBHOOK_TEST: &str = "alert.webhook.test";
const SILENCE_ADD: &str = "alert.silence.add";
const SILENCE_EXPIRE: &str = "alert.silence.expire";

pub(crate) fn field(f: &str, msg: impl Into<String>) -> Refusal {
    Refusal::Field(FieldError {
        field: f.to_owned(),
        msg: msg.into(),
    })
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn printable(s: &str) -> bool {
    s.chars().all(|c| c >= ' ' && c != '\u{7f}')
}

pub(crate) fn time_or(o: Obj, key: &str, t: Option<SystemTime>) -> Obj {
    match t {
        Some(t) => o.raw(key, &json::time(t)),
        None => o,
    }
}

pub(crate) fn time_of(v: &Value, key: &str) -> Option<SystemTime> {
    parse_time(json::str_of(v, key))
}

/// `t` as Go's `time.RFC3339` formats it: to the second, in UTC.
pub(crate) fn rfc3339(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let mut v = Vec::new();
    akit_observe::time::rfc3339_nano(&mut v, i64::try_from(d.as_secs()).unwrap_or(i64::MAX), 0, 0);
    String::from_utf8_lossy(&v).into_owned()
}

pub(crate) fn ms(n: i64) -> Duration {
    Duration::from_millis(u64::try_from(n).unwrap_or(0))
}

fn name_error(what: &str, name: &str) -> Refusal {
    field(
        what,
        format!(
            "{} is not a name: 1 to {MAX_NAME_BYTES} letters, digits, '_', '.' and '-', starting \
             with a letter or digit",
            quoted(name)
        ),
    )
}

/// A rule: fit's `AlertRule`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rule {
    /// Its name.
    pub name: String,
    /// One of [`KINDS`].
    pub kind: String,
    /// Whether it is turned off.
    pub disabled: bool,
    /// The group whose members it watches; every member when empty.
    pub group: String,
    /// One of [`SEVERITIES`].
    pub severity: String,
    /// How long its condition holds before it fires.
    pub for_ms: i64,
    /// How often it is sent again while it fires; never when 0.
    pub repeat_ms: i64,
    /// A failed rollout's or a failed reload's alert resolves after this; never when 0.
    pub resolve_after_ms: i64,
    /// A certificate's days left that fire it.
    pub days: i64,
    /// The backends down that fire it.
    pub threshold: i64,
    /// What a drift rule watches.
    pub source: String,
    /// The webhooks it is sent to; every one when empty.
    pub webhooks: Vec<String>,
    /// When it was added and last changed.
    pub created: Option<SystemTime>,
    /// See [`created`](Rule::created).
    pub updated: Option<SystemTime>,
}

impl Rule {
    pub(crate) fn json(&self) -> String {
        let mut o = Obj::new().str("name", &self.name).str("kind", &self.kind);
        if self.disabled {
            o = o.bool("disabled", true);
        }
        o = o
            .str_omit("group", &self.group)
            .str("severity", &self.severity)
            .num("for_ms", self.for_ms)
            .num("repeat_ms", self.repeat_ms);
        for (k, n) in [
            ("resolve_after_ms", self.resolve_after_ms),
            ("days", self.days),
            ("threshold", self.threshold),
        ] {
            if n != 0 {
                o = o.num(k, n);
            }
        }
        o = o
            .str_omit("source", &self.source)
            .raw("webhooks", &json::string_list(&self.webhooks));
        o = time_or(o, "created", self.created);
        time_or(o, "updated", self.updated).end()
    }

    /// A rule as sent or kept.
    ///
    /// # Errors
    ///
    /// A number that is not.
    pub fn from_value(v: &Value) -> Result<Rule, Refusal> {
        Ok(Rule {
            name: json::str_of(v, "name").to_owned(),
            kind: json::str_of(v, "kind").to_owned(),
            disabled: json::bool_of(v, "disabled"),
            group: json::str_of(v, "group").to_owned(),
            severity: json::str_of(v, "severity").to_owned(),
            for_ms: whole(v, "for_ms", "for_ms")?,
            repeat_ms: whole(v, "repeat_ms", "repeat_ms")?,
            resolve_after_ms: whole(v, "resolve_after_ms", "resolve_after_ms")?,
            days: whole(v, "days", "days")?,
            threshold: whole(v, "threshold", "threshold")?,
            source: json::str_of(v, "source").to_owned(),
            webhooks: json::strings_of(v, "webhooks"),
            created: time_of(v, "created"),
            updated: time_of(v, "updated"),
        })
    }

    /// Checks it, filling in the defaults: fit's `AlertRule.check`.
    ///
    /// # Errors
    ///
    /// The field refused.
    pub fn check(&mut self) -> Result<(), Refusal> {
        if self.name.is_empty() || self.name.len() > MAX_NAME_BYTES || !name_ok(&self.name) {
            return Err(name_error("name", &self.name));
        }
        if !KINDS.contains(&self.kind.as_str()) {
            return Err(field(
                "kind",
                format!(
                    "{} is not a kind of rule: {}",
                    quoted(&self.kind),
                    KINDS.join(", ")
                ),
            ));
        }
        if self.severity.is_empty() {
            self.severity = if self.kind == MEMBER_DOWN || self.kind == ROLLOUT_FAILED {
                "critical"
            } else {
                "warning"
            }
            .to_owned();
        }
        if !SEVERITIES.contains(&self.severity.as_str()) {
            return Err(field(
                "severity",
                format!(
                    "{} is not a severity: {}",
                    quoted(&self.severity),
                    SEVERITIES.join(", ")
                ),
            ));
        }
        if self.for_ms < 0 || self.for_ms > MAX_FOR_MS {
            return Err(field(
                "for_ms",
                format!("a rule's time is 0 to {}", go_duration(MAX_FOR_MS)),
            ));
        }
        if self.repeat_ms != 0 && (self.repeat_ms < MIN_REPEAT_MS || self.repeat_ms > MAX_REPEAT_MS)
        {
            return Err(field(
                "repeat_ms",
                format!(
                    "a repeat interval is 0 (never) or {} to {}",
                    go_duration(MIN_REPEAT_MS),
                    go_duration(MAX_REPEAT_MS)
                ),
            ));
        }
        if self.kind != CERT_EXPIRY {
            self.days = 0;
        }
        if self.kind != BACKENDS_DOWN {
            self.threshold = 0;
        }
        if self.kind != DRIFT {
            self.source.clear();
        }
        if self.kind != ROLLOUT_FAILED && self.kind != RELOAD_FAILED {
            self.resolve_after_ms = 0;
        }
        match self.kind.as_str() {
            CERT_EXPIRY => {
                if self.days == 0 {
                    self.days = DEFAULT_CERT_DAYS;
                }
                if self.days < 1 || self.days > MAX_CERT_DAYS {
                    return Err(field(
                        "days",
                        format!("a certificate's days are 1 to {MAX_CERT_DAYS}"),
                    ));
                }
            }
            BACKENDS_DOWN => {
                if self.threshold == 0 {
                    self.threshold = 1;
                }
                if self.threshold < 1 || self.threshold > MAX_BACKENDS {
                    return Err(field(
                        "threshold",
                        format!("the backends down are 1 to {MAX_BACKENDS}"),
                    ));
                }
            }
            DRIFT => {
                if !["", DRIFT_TEMPLATES, DRIFT_CONSISTENCY].contains(&self.source.as_str()) {
                    return Err(field(
                        "source",
                        format!(
                            "{} is not what a drift rule watches: {DRIFT_TEMPLATES}, \
                             {DRIFT_CONSISTENCY}, or empty for both",
                            quoted(&self.source)
                        ),
                    ));
                }
            }
            _ => {
                if self.resolve_after_ms != 0
                    && (self.resolve_after_ms < MIN_RESOLVE_MS
                        || self.resolve_after_ms > MAX_RESOLVE_MS)
                {
                    return Err(field(
                        "resolve_after_ms",
                        format!(
                            "a rule resolves after 0 (never) or {} to {}",
                            go_duration(MIN_RESOLVE_MS),
                            go_duration(MAX_RESOLVE_MS)
                        ),
                    ));
                }
            }
        }
        if self.webhooks.len() > MAX_WEBHOOKS {
            return Err(field(
                "webhooks",
                format!("{} webhooks, more than {MAX_WEBHOOKS}", self.webhooks.len()),
            ));
        }
        self.webhooks.sort();
        self.webhooks.dedup();
        Ok(())
    }

    fn describe(&self) -> String {
        let mut s = format!("{} for {}", self.kind, go_duration(self.for_ms));
        if !self.group.is_empty() {
            s.push_str(&format!(" on the group {}", self.group));
        }
        if self.disabled {
            s.push_str(", disabled");
        }
        s
    }
}

/// The rules a manager starts with: fit's `defaultRules`.
#[must_use]
pub fn default_rules() -> Vec<Rule> {
    let mk = |name: &str, kind: &str, for_ms: i64| Rule {
        name: name.to_owned(),
        kind: kind.to_owned(),
        for_ms,
        repeat_ms: DEFAULT_REPEAT_MS,
        ..Rule::default()
    };
    let mut rules = vec![
        mk("member-down", MEMBER_DOWN, 60_000),
        mk("drift", DRIFT, 300_000),
        Rule {
            resolve_after_ms: 24 * 3600 * 1000,
            ..mk("rollout-failed", ROLLOUT_FAILED, 0)
        },
        Rule {
            days: DEFAULT_CERT_DAYS,
            ..mk("certificate-expiry", CERT_EXPIRY, 0)
        },
        Rule {
            threshold: 1,
            ..mk("backends-down", BACKENDS_DOWN, 120_000)
        },
        Rule {
            resolve_after_ms: 3600 * 1000,
            ..mk("reloads-failing", RELOAD_FAILED, 0)
        },
    ];
    for r in &mut rules {
        let _ = r.check();
    }
    rules
}

/// A silence: fit's `Silence`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Silence {
    /// Its ID.
    pub id: String,
    /// The rule it silences, or every rule.
    pub rule: String,
    /// The member it silences, or every member.
    pub node: String,
    /// What a person said of it.
    pub comment: String,
    /// Who added it, and when.
    pub created_by: String,
    /// See [`created_by`](Silence::created_by).
    pub created: Option<SystemTime>,
    /// When it starts and ends.
    pub starts: Option<SystemTime>,
    /// See [`starts`](Silence::starts).
    pub ends: Option<SystemTime>,
}

impl Silence {
    pub(crate) fn json(&self) -> String {
        let o = Obj::new()
            .str("id", &self.id)
            .str_omit("rule", &self.rule)
            .str_omit("node", &self.node)
            .str_omit("comment", &self.comment)
            .str("created_by", &self.created_by)
            .raw("created", &json::time(self.created.unwrap_or(UNIX_EPOCH)))
            .raw("starts", &json::time(self.starts.unwrap_or(UNIX_EPOCH)));
        o.raw("ends", &json::time(self.ends.unwrap_or(UNIX_EPOCH)))
            .end()
    }

    fn from_value(v: &Value) -> Silence {
        Silence {
            id: json::str_of(v, "id").to_owned(),
            rule: json::str_of(v, "rule").to_owned(),
            node: json::str_of(v, "node").to_owned(),
            comment: json::str_of(v, "comment").to_owned(),
            created_by: json::str_of(v, "created_by").to_owned(),
            created: time_of(v, "created"),
            starts: time_of(v, "starts"),
            ends: time_of(v, "ends"),
        }
    }

    pub(crate) fn active(&self, now: SystemTime) -> bool {
        self.starts.is_some_and(|s| s <= now) && self.ends.is_some_and(|e| now < e)
    }

    pub(crate) fn matches(&self, a: &Alert) -> bool {
        (self.rule.is_empty() || self.rule == a.rule)
            && (self.node.is_empty() || self.node == a.node)
    }

    fn describe(&self) -> String {
        let mut what = if self.rule.is_empty() {
            "every rule".to_owned()
        } else {
            format!("the rule {}", self.rule)
        };
        if !self.node.is_empty() {
            what.push_str(&format!(" on {}", self.node));
        }
        let comment = if self.comment.is_empty() {
            String::new()
        } else {
            format!(" ({})", self.comment)
        };
        clip(
            &format!(
                "{}: {what} from {} to {}{comment}",
                self.id,
                rfc3339(self.starts.unwrap_or(UNIX_EPOCH)),
                rfc3339(self.ends.unwrap_or(UNIX_EPOCH))
            ),
            512,
        )
    }
}

/// A silence as sent: fit's `SilenceInput.silence`.
fn silence_of(v: &Value, user: &str, now: SystemTime) -> Result<Silence, Refusal> {
    let rule = json::str_of(v, "rule").to_owned();
    let node = json::str_of(v, "node").to_owned();
    let comment = json::str_of(v, "comment").trim().to_owned();
    if rule.is_empty() && node.is_empty() {
        return Err(field("rule", "a silence names a rule, a member, or both"));
    }
    if !rule.is_empty() && (!name_ok(&rule) || rule.len() > MAX_NAME_BYTES) {
        return Err(field(
            "rule",
            format!("{} is not a rule's name", quoted(&rule)),
        ));
    }
    if !node.is_empty() && (!name_ok(&node) || node.len() > MAX_NAME_BYTES) {
        return Err(field(
            "node",
            format!("{} is not a member's name", quoted(&node)),
        ));
    }
    if comment.len() > MAX_TEXT || !printable(&comment) {
        return Err(field(
            "comment",
            format!("a comment is at most {MAX_TEXT} bytes of printable characters"),
        ));
    }
    let given = |k: &str| -> Result<Option<SystemTime>, Refusal> {
        match json::str_of(v, k) {
            "" => Ok(None),
            t => parse_time(t)
                .map(Some)
                .ok_or_else(|| field(k, format!("{k} is a time, as RFC 3339 writes it"))),
        }
    };
    let starts = given("starts")?.unwrap_or(now);
    let ends = match given("ends")? {
        Some(e) => e,
        None => {
            let d = whole(v, "duration_ms", "duration_ms")?;
            if d <= 0 {
                return Err(field(
                    "ends",
                    "a silence ends: give its end or its duration",
                ));
            }
            starts.checked_add(ms(d)).unwrap_or(starts)
        }
    };
    if ends <= starts || ends <= now {
        return Err(field(
            "ends",
            "a silence ends after it starts, and after now",
        ));
    }
    if ends.duration_since(now).unwrap_or_default() > MAX_SILENCE_SPAN {
        return Err(field(
            "ends",
            format!(
                "a silence ends within {}",
                go_duration(i64::try_from(MAX_SILENCE_SPAN.as_millis()).unwrap_or(0))
            ),
        ));
    }
    Ok(Silence {
        id: crate::ops::new_id(),
        rule,
        node,
        comment,
        created_by: user.to_owned(),
        created: Some(now),
        starts: Some(starts),
        ends: Some(ends),
    })
}

/// An alert: fit's `Alert`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Alert {
    /// Its ID, once it fired.
    pub id: String,
    /// Its rule, the rule's kind and severity.
    pub rule: String,
    /// See [`rule`](Alert::rule).
    pub kind: String,
    /// See [`rule`](Alert::rule).
    pub severity: String,
    /// The member it is of, or
    pub node: String,
    /// what else: a rollout's ID.
    pub subject: String,
    /// [`PENDING`], [`FIRING`] or [`RESOLVED`].
    pub state: String,
    /// What it found.
    pub summary: String,
    /// When its condition began, it fired and it resolved, and why.
    pub since: Option<SystemTime>,
    /// See [`since`](Alert::since).
    pub fired: Option<SystemTime>,
    /// See [`since`](Alert::since).
    pub resolved: Option<SystemTime>,
    /// See [`since`](Alert::since).
    pub resolved_why: String,
    /// When it was last sent, and how many times.
    pub last_sent: Option<SystemTime>,
    /// See [`last_sent`](Alert::last_sent).
    pub sends: u64,
    /// The silence it is silenced by, as answered.
    pub silenced_by: String,
}

impl Alert {
    pub(crate) fn key(&self) -> String {
        key(&self.rule, &self.node, &self.subject)
    }

    pub(crate) fn json(&self) -> String {
        let mut o = Obj::new()
            .str("id", &self.id)
            .str("rule", &self.rule)
            .str("kind", &self.kind)
            .str("severity", &self.severity)
            .str_omit("node", &self.node)
            .str_omit("subject", &self.subject)
            .str("state", &self.state)
            .str("summary", &self.summary)
            .raw("since", &json::time(self.since.unwrap_or(UNIX_EPOCH)));
        o = time_or(o, "fired", self.fired);
        o = time_or(o, "resolved", self.resolved);
        o = o.str_omit("resolved_why", &self.resolved_why);
        o = time_or(o, "last_sent", self.last_sent);
        o.num("sends", self.sends)
            .str_omit("silenced_by", &self.silenced_by)
            .end()
    }

    fn from_value(v: &Value) -> Alert {
        Alert {
            id: json::str_of(v, "id").to_owned(),
            rule: json::str_of(v, "rule").to_owned(),
            kind: json::str_of(v, "kind").to_owned(),
            severity: json::str_of(v, "severity").to_owned(),
            node: json::str_of(v, "node").to_owned(),
            subject: json::str_of(v, "subject").to_owned(),
            state: json::str_of(v, "state").to_owned(),
            summary: json::str_of(v, "summary").to_owned(),
            since: time_of(v, "since"),
            fired: time_of(v, "fired"),
            resolved: time_of(v, "resolved"),
            resolved_why: json::str_of(v, "resolved_why").to_owned(),
            last_sent: time_of(v, "last_sent"),
            sends: json::uint_of(v, "sends"),
            silenced_by: String::new(),
        }
    }
}

pub(crate) fn key(rule: &str, node: &str, subject: &str) -> String {
    format!("{rule}|{node}|{subject}")
}

/// The rules, webhooks and silences.
#[derive(Debug, Clone, Default)]
pub(crate) struct Config {
    pub(crate) rules: Vec<Rule>,
    pub(crate) webhooks: Vec<Webhook>,
    pub(crate) silences: Vec<Silence>,
}

impl Config {
    fn rule(&self, name: &str) -> Option<usize> {
        self.rules.iter().position(|r| r.name == name)
    }

    fn webhook(&self, name: &str) -> Option<usize> {
        self.webhooks.iter().position(|w| w.name == name)
    }
}

/// How a member's reloads go, as its polls count them.
#[derive(Debug, Clone, Default)]
pub(crate) struct Track {
    pub(crate) polled: Option<SystemTime>,
    pub(crate) succeeded: u64,
    pub(crate) failed: u64,
    pub(crate) failing: bool,
    pub(crate) failed_at: Option<SystemTime>,
}

/// The alerts, under their lock: fit's `alertsState`.
#[derive(Default)]
pub(crate) struct Al {
    pub(crate) cfg: Config,
    pub(crate) rev: u64,
    pub(crate) read_only: String,
    pub(crate) active: BTreeMap<String, Alert>,
    pub(crate) pending: BTreeMap<String, Alert>,
    pub(crate) history: Vec<Alert>,
    pub(crate) reloads: HashMap<String, Track>,
    pub(crate) dirty: bool,
    pub(crate) full: Option<SystemTime>,
    /// The members whose templates are being read for drift, and the config each was last read
    /// at (its ETag).
    pub(crate) drift_busy: BTreeSet<String>,
    pub(crate) drift_poll: HashMap<String, SystemTime>,
    pub(crate) senders: HashMap<String, Arc<Sender>>,
}

impl Al {
    /// The document as the store keeps it, with `c` in place of the config: fit's
    /// `marshalLocked`.
    fn marshal(&self, c: &Config) -> Vec<u8> {
        let list = |items: Vec<String>| format!("[{}]", items.join(","));
        Obj::new()
            .num("version", VERSION)
            .raw("rules", &list(c.rules.iter().map(Rule::json).collect()))
            .raw(
                "webhooks",
                &list(c.webhooks.iter().map(Webhook::stored).collect()),
            )
            .raw(
                "silences",
                &list(c.silences.iter().map(Silence::json).collect()),
            )
            .raw(
                "active",
                &list(self.active.values().map(Alert::json).collect()),
            )
            .raw(
                "history",
                &list(self.history.iter().map(Alert::json).collect()),
            )
            .end()
            .into_bytes()
    }

    pub(crate) fn silence(&self, a: &Alert, now: SystemTime) -> Option<&Silence> {
        self.cfg
            .silences
            .iter()
            .find(|s| s.active(now) && s.matches(a))
    }

    fn counts(&self, now: SystemTime) -> String {
        let (mut firing, mut silenced) = (0usize, 0usize);
        let mut by = [0usize; 3];
        for a in self.active.values() {
            if self.silence(a, now).is_some() {
                silenced = silenced.saturating_add(1);
                continue;
            }
            firing = firing.saturating_add(1);
            let i = match a.severity.as_str() {
                "critical" => 0,
                "warning" => 1,
                _ => 2,
            };
            if let Some(n) = by.get_mut(i) {
                *n = n.saturating_add(1);
            }
        }
        Obj::new()
            .num("firing", firing)
            .num("silenced", silenced)
            .num("pending", self.pending.len())
            .num("critical", by[0])
            .num("warning", by[1])
            .num("info", by[2])
            .end()
    }
}

fn prune(list: &mut Vec<Silence>, now: SystemTime) {
    list.retain(|s| {
        s.ends
            .and_then(|e| now.duration_since(e).ok())
            .is_none_or(|d| d <= SILENCE_KEPT)
    });
}

impl Manager {
    /// Reads the alerts' document, or starts with the default rules: fit's `loadAlerts`.
    pub(crate) fn load_alerts(&self) {
        let data = self.0.o.store.section(SECTION);
        let mut al = lock(&self.0.al);
        let Some(data) = data.filter(|d| !d.is_empty()) else {
            al.cfg.rules = default_rules();
            al.dirty = true;
            return;
        };
        let Some(d @ Value::Object(_)) = json::parse(&data) else {
            al.read_only = format!(
                "the store's section {SECTION:?} is not the alerts' document; it is left as it \
                 is, and the default rules watch the members"
            );
            al.cfg.rules = default_rules();
            drop(al);
            self.log(format!("cluster: {}", self.alerts_read_only()));
            return;
        };
        let version = json::int_of(&d, "version");
        if version > VERSION {
            al.read_only = format!(
                "the alerts were written by a newer manager (version {version}; this akit writes \
                 {VERSION}): they are shown, and changed only by that manager"
            );
        }
        let list = |k: &str| match d.get(k) {
            Some(Value::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        let mut skipped = Vec::new();
        for v in list("rules") {
            match Rule::from_value(&v).and_then(|mut r| r.check().map(|()| r)) {
                Ok(r) => al.cfg.rules.push(r),
                Err(_) => skipped.push(json::str_of(&v, "name").to_owned()),
            }
        }
        al.cfg.webhooks = list("webhooks").iter().map(Webhook::from_value).collect();
        al.cfg.silences = list("silences").iter().map(Silence::from_value).collect();
        for v in list("active") {
            let a = Alert::from_value(&v);
            al.active.insert(a.key(), a);
        }
        al.history = list("history").iter().map(Alert::from_value).collect();
        let ro = al.read_only.clone();
        drop(al);
        if !ro.is_empty() {
            self.log(format!("cluster: {ro}"));
        }
        for n in skipped {
            self.log(format!(
                "cluster: the alert rule {n:?} kept in the store is not used: it does not check"
            ));
        }
    }

    fn alerts_read_only(&self) -> String {
        lock(&self.0.al).read_only.clone()
    }

    /// Writes the alerts when what fired or resolved changed: the alerts' thread's.
    pub(crate) fn save_alerts(&self) {
        let _w = lock(&self.0.al_write);
        let data = {
            let mut al = lock(&self.0.al);
            if !al.dirty || !al.read_only.is_empty() {
                return;
            }
            al.dirty = false;
            al.marshal(&al.cfg)
        };
        if let Err(e) = self.0.o.store.set_section(SECTION, &data) {
            self.log(format!("cluster: the alerts were not saved: {e}"));
            lock(&self.0.al).dirty = true;
        }
    }

    /// Changes the rules, webhooks or silences with `f`, writes them, and records `f`'s audit
    /// entry (its action, detail and member): fit's `changeAlerts`. On the manager's pool.
    fn change_alerts<T>(
        &self,
        user: &str,
        f: impl FnOnce(&mut Config, SystemTime) -> Result<((&'static str, String, String), T), Refusal>,
    ) -> Result<T, Refusal> {
        let _w = lock(&self.0.al_write);
        let now = SystemTime::now();
        let (mut c, rev) = {
            let al = lock(&self.0.al);
            if !al.read_only.is_empty() {
                return Err(Refusal::Conflict(al.read_only.clone()));
            }
            (al.cfg.clone(), al.rev)
        };
        let ((action, detail, node), out) = f(&mut c, now)?;
        prune(&mut c.silences, now);
        let data = lock(&self.0.al).marshal(&c);
        self.0
            .o
            .store
            .set_section(SECTION, &data)
            .map_err(|e| Refusal::Failed(format!("saving the alerts: {e}")))?;
        {
            let mut al = lock(&self.0.al);
            al.cfg = c;
            al.rev = al.rev.saturating_add(1);
            al.dirty = false;
        }
        self.sync_senders();
        let s = |v: &str| Value::String(v.to_owned());
        let mut e = vec![("action".to_owned(), s(action))];
        if !node.is_empty() {
            e.push(("node".to_owned(), s(&node)));
        }
        e.push(("detail".to_owned(), s(&clip(&detail, 512))));
        e.push(("outcome".to_owned(), s("ok")));
        e.push(("before".to_owned(), s(&format!("a{rev}"))));
        e.push((
            "after".to_owned(),
            s(&format!("a{}", rev.saturating_add(1))),
        ));
        self.record(user, e);
        self.log(format!("cluster: {user:?}: {action} {detail}"));
        self.poke_alerts();
        Ok(out)
    }

    /// The alerts' ETag.
    pub(crate) fn alerts_etag(&self) -> String {
        format!("\"a{}\"", lock(&self.0.al).rev)
    }

    fn check_refs(&self, r: &Rule, c: &Config) -> Result<(), Refusal> {
        if !r.group.is_empty() && !self.doc().groups.iter().any(|g| g.name == r.group) {
            return Err(field(
                "group",
                format!("no group is named {}", quoted(&r.group)),
            ));
        }
        if let Some(w) = r.webhooks.iter().find(|w| c.webhook(w).is_none()) {
            return Err(field(
                "webhooks",
                format!("no webhook is named {}", quoted(w)),
            ));
        }
        Ok(())
    }

    fn add_rule(&self, user: &str, mut r: Rule) -> Result<String, Refusal> {
        self.change_alerts(user, |c, now| {
            r.check()?;
            self.check_refs(&r, c)?;
            if c.rule(&r.name).is_some() {
                return Err(Refusal::Conflict(format!(
                    "a rule is already named {}",
                    quoted(&r.name)
                )));
            }
            if c.rules.len() >= MAX_RULES {
                return Err(Refusal::Conflict(format!(
                    "the manager keeps {MAX_RULES} rules at most"
                )));
            }
            r.created = Some(now);
            r.updated = Some(now);
            c.rules.push(r.clone());
            Ok((
                (
                    RULE_ADD,
                    format!("{}: {}", r.name, r.describe()),
                    String::new(),
                ),
                r.json(),
            ))
        })
    }

    fn update_rule(&self, user: &str, name: &str, mut r: Rule) -> Result<String, Refusal> {
        if r.name.is_empty() {
            name.clone_into(&mut r.name);
        }
        self.change_alerts(user, |c, now| {
            let i = c
                .rule(name)
                .ok_or_else(|| Refusal::NotFound(format!("no rule is named {}", quoted(name))))?;
            r.check()?;
            self.check_refs(&r, c)?;
            if r.name != name && c.rule(&r.name).is_some() {
                return Err(Refusal::Conflict(format!(
                    "a rule is already named {}",
                    quoted(&r.name)
                )));
            }
            r.created = c.rules.get(i).and_then(|x| x.created);
            r.updated = Some(now);
            if let Some(slot) = c.rules.get_mut(i) {
                *slot = r.clone();
            }
            Ok((
                (
                    RULE_UPDATE,
                    format!("{name}: {}", r.describe()),
                    String::new(),
                ),
                r.json(),
            ))
        })
    }

    fn remove_rule(&self, user: &str, name: &str) -> Result<String, Refusal> {
        self.change_alerts(user, |c, _| {
            let i = c
                .rule(name)
                .ok_or_else(|| Refusal::NotFound(format!("no rule is named {}", quoted(name))))?;
            c.rules.remove(i);
            Ok((
                (RULE_REMOVE, name.to_owned(), String::new()),
                Obj::new().str("removed", name).end(),
            ))
        })
    }

    fn add_webhook(&self, user: &str, v: &Value) -> Result<String, Refusal> {
        self.change_alerts(user, |c, now| {
            let w = Webhook::of(v, None, now)?;
            if c.webhook(&w.name).is_some() {
                return Err(Refusal::Conflict(format!(
                    "a webhook is already named {}",
                    quoted(&w.name)
                )));
            }
            if c.webhooks.len() >= MAX_WEBHOOKS {
                return Err(Refusal::Conflict(format!(
                    "the manager keeps {MAX_WEBHOOKS} webhooks at most"
                )));
            }
            let detail = format!("{}: {} to {}", w.name, w.format, webhooks::url_host(&w.url));
            let out = w.view(None);
            c.webhooks.push(w);
            Ok(((WEBHOOK_ADD, detail, String::new()), out))
        })
    }

    fn update_webhook(&self, user: &str, name: &str, v: &Value) -> Result<String, Refusal> {
        self.change_alerts(user, |c, now| {
            let i = c.webhook(name).ok_or_else(|| {
                Refusal::NotFound(format!("no webhook is named {}", quoted(name)))
            })?;
            let old = c.webhooks.get(i).cloned();
            let mut w = Webhook::of(v, old.as_ref(), now)?;
            if w.name.is_empty() {
                name.clone_into(&mut w.name);
            }
            if w.name != name && c.webhook(&w.name).is_some() {
                return Err(Refusal::Conflict(format!(
                    "a webhook is already named {}",
                    quoted(&w.name)
                )));
            }
            let changed = old.as_ref().map_or_else(String::new, |o| o.changes(&w));
            let new_name = w.name.clone();
            let out = w.view(None);
            if let Some(slot) = c.webhooks.get_mut(i) {
                *slot = w;
            }
            for r in &mut c.rules {
                for n in &mut r.webhooks {
                    if n == name {
                        n.clone_from(&new_name);
                    }
                }
                r.webhooks.sort();
            }
            Ok((
                (WEBHOOK_UPDATE, format!("{name}: {changed}"), String::new()),
                out,
            ))
        })
    }

    fn remove_webhook(&self, user: &str, name: &str) -> Result<String, Refusal> {
        self.change_alerts(user, |c, _| {
            let i = c.webhook(name).ok_or_else(|| {
                Refusal::NotFound(format!("no webhook is named {}", quoted(name)))
            })?;
            let users: Vec<String> = c
                .rules
                .iter()
                .filter(|r| r.webhooks.iter().any(|w| w == name))
                .map(|r| r.name.clone())
                .collect();
            if !users.is_empty() {
                return Err(Refusal::Conflict(format!(
                    "the rules {} send to the webhook: change them first",
                    users.join(", ")
                )));
            }
            c.webhooks.remove(i);
            Ok((
                (WEBHOOK_REMOVE, name.to_owned(), String::new()),
                Obj::new().str("removed", name).end(),
            ))
        })
    }

    fn add_silence(&self, user: &str, v: &Value) -> Result<String, Refusal> {
        self.change_alerts(user, |c, now| {
            let s = silence_of(v, user, now)?;
            prune(&mut c.silences, now);
            if c.silences.len() >= MAX_SILENCES {
                let Some(i) = c
                    .silences
                    .iter()
                    .position(|s| s.ends.is_some_and(|e| e <= now))
                else {
                    return Err(Refusal::Conflict(format!(
                        "the manager keeps {MAX_SILENCES} silences at most"
                    )));
                };
                c.silences.remove(i);
            }
            let detail = s.describe();
            let node = s.node.clone();
            let out = s.json();
            c.silences.push(s);
            Ok(((SILENCE_ADD, detail, node), out))
        })
    }

    fn expire_silence(&self, user: &str, id: &str) -> Result<String, Refusal> {
        self.change_alerts(user, |c, now| {
            let s =
                c.silences.iter_mut().find(|s| s.id == id).ok_or_else(|| {
                    Refusal::NotFound(format!("no silence {} is kept", quoted(id)))
                })?;
            if s.ends.is_none_or(|e| now < e) {
                s.ends = Some(now);
            }
            if s.starts > s.ends {
                s.starts = s.ends;
            }
            Ok(((SILENCE_EXPIRE, s.describe(), s.node.clone()), s.json()))
        })
    }

    /// `GET /api/alerts`: fit's `Alerts`.
    fn alerts_json(&self) -> String {
        let now = SystemTime::now();
        let al = lock(&self.0.al);
        let order = |list: &mut Vec<Alert>| {
            list.sort_by(|a, b| {
                let s = |x: &Alert| SEVERITIES.iter().position(|v| *v == x.severity);
                s(b).cmp(&s(a)).then_with(|| a.since.cmp(&b.since))
            });
        };
        let mut active: Vec<Alert> = al
            .active
            .values()
            .map(|a| {
                let mut c = a.clone();
                if let Some(s) = al.silence(a, now) {
                    c.silenced_by.clone_from(&s.id);
                }
                c
            })
            .collect();
        let mut pending: Vec<Alert> = al.pending.values().cloned().collect();
        order(&mut active);
        order(&mut pending);
        let list = |l: &[Alert]| {
            format!(
                "[{}]",
                l.iter().map(Alert::json).collect::<Vec<_>>().join(",")
            )
        };
        let history: Vec<Alert> = al.history.iter().rev().cloned().collect();
        Obj::new()
            .str_omit("read_only", &al.read_only)
            .raw("counts", &al.counts(now))
            .raw("active", &list(&active))
            .raw("pending", &list(&pending))
            .raw("history", &list(&history))
            .end()
    }

    /// `GET /api/alerts/config`: fit's `AlertConfig`.
    fn alerts_config_json(&self) -> String {
        let al = lock(&self.0.al);
        let mut silences = al.cfg.silences.clone();
        silences.sort_by_key(|s| std::cmp::Reverse(s.ends));
        let views: Vec<String> = al
            .cfg
            .webhooks
            .iter()
            .map(|w| w.view(al.senders.get(&w.name).map(|s| s.status())))
            .collect();
        let strs =
            |l: &[&str]| json::string_list(&l.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
        Obj::new()
            .str("version", &format!("a{}", al.rev))
            .str_omit("read_only", &al.read_only)
            .raw(
                "rules",
                &format!(
                    "[{}]",
                    al.cfg
                        .rules
                        .iter()
                        .map(Rule::json)
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            )
            .raw("webhooks", &format!("[{}]", views.join(",")))
            .raw(
                "silences",
                &format!(
                    "[{}]",
                    silences
                        .iter()
                        .map(Silence::json)
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            )
            .raw("kinds", &strs(KINDS))
            .raw("formats", &strs(webhooks::FORMATS))
            .raw("severities", &strs(SEVERITIES))
            .raw(
                "bounds",
                &Obj::new()
                    .num("max_rules", MAX_RULES)
                    .num("max_webhooks", MAX_WEBHOOKS)
                    .num("max_silences", MAX_SILENCES)
                    .num("max_history", MAX_HISTORY)
                    .num("max_queue", webhooks::MAX_QUEUE)
                    .num("tick_ms", 0)
                    .end(),
            )
            .end()
    }

    /// `GET /api/drift`: each member's drift, its templates' and its check's: fit's
    /// `DriftStates`.
    fn drift_json(&self) -> String {
        let d = self.doc();
        let tgroups: BTreeSet<String> = d
            .templates
            .iter()
            .flat_map(|t| t.groups.iter().cloned())
            .collect();
        let syncs = lock(&self.0.tpl).last.clone();
        let mut nodes = Vec::new();
        for m in self.members() {
            let groups = crate::groups::groups_of(&d, &m.name, &m.labels);
            let st = self.node(&m.name).map(|n| n.state());
            let status = st.as_ref().map_or(crate::node::STATUS_ERROR, |s| s.status);
            let managed = groups.iter().any(|g| tgroups.contains(g));
            let drift = st.as_ref().and_then(|s| s.drift.clone());
            let sync = syncs.get(&m.name).cloned();
            let sv = sync.as_deref().and_then(|s| json::parse(s.as_bytes()));
            let state = sv.as_ref().map_or("", |v| json::str_of(v, "state"));
            let fixable = state == "out_of_sync" || state == "conflict";
            let render_error = state == "error"
                && sv
                    .as_ref()
                    .is_some_and(|v| !json::strings_of(v, "errors").is_empty());
            let count = drift
                .as_deref()
                .and_then(|d| json::parse(d.as_bytes()))
                .map_or(0, |v| json::int_of(&v, "count"));
            let drifted = fixable || render_error || count > 0;
            nodes.push(
                Obj::new()
                    .str("node", &m.name)
                    .str("status", status)
                    .raw("groups", &json::string_list(&groups))
                    .bool("managed", managed)
                    .opt("templates", sync.as_deref())
                    .opt("consistency", drift.as_deref())
                    .bool("drifted", drifted)
                    .bool("fixable", fixable)
                    .end(),
            );
        }
        Obj::new()
            .num("interval_ms", json::millis(self.interval()))
            .str("read_only", &self.read_only())
            .raw("nodes", &format!("[{}]", nodes.join(",")))
            .end()
    }

    /// `POST /api/drift/fix`: a sync rollout of `nodes` planned: fit's `FixDrift`.
    fn fix_drift(&self, user: &str, nodes: Vec<String>) -> Result<String, Refusal> {
        if nodes.is_empty() {
            return Err(field("nodes", "a fix names the members it brings in sync"));
        }
        if nodes.len() > self.max_nodes() {
            return Err(field(
                "nodes",
                format!("{} members, more than {}", nodes.len(), self.max_nodes()),
            ));
        }
        let desc = clip(
            &format!("a fix of the drift of {}", nodes.join(", ")),
            512 - 4,
        );
        self.plan_rollout(
            user,
            crate::rollouts::Request {
                kind: crate::rollouts::KIND_SYNC.to_owned(),
                nodes,
                description: desc,
                ..crate::rollouts::Request::default()
            },
        )
    }
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

/// Whether `If-Match`, when sent, names the alerts as they are; 412 when not.
fn matches(m: &Manager, ctx: &mut Context) -> bool {
    let want = ctx.header("If-Match").unwrap_or("").to_owned();
    if want.is_empty() || want == "*" || want == m.alerts_etag() {
        return true;
    }
    ctx.respond(fail(
        412,
        "the alerts' rules, webhooks or silences changed since the page read them; read them again",
    ));
    false
}

/// Runs `work` on the pool, answering with its outcome and the alerts' ETag.
fn on_pool(
    m: &Manager,
    ctx: &mut Context,
    status: u16,
    work: impl FnOnce(&Manager) -> Result<String, Refusal> + Send + 'static,
) {
    let me = m.clone();
    crate::api::on_pool(m, ctx, move || match work(&me) {
        Ok(body) => answer(status, body).header("ETag", me.alerts_etag()),
        Err(e) => e.response(),
    });
}

/// The alerts' and the drift's endpoints; `false` for a path that is not theirs.
pub(crate) fn serve(m: &Manager, ctx: &mut Context, user: &str, path: &str) -> bool {
    let method = ctx.method().clone();
    let get = matches!(method, Method::Get | Method::Head);
    let post = method == Method::Post;
    let user = user.to_owned();
    match path {
        "/api/alerts" if get => ctx.respond(answer(200, m.alerts_json())),
        "/api/drift" if get => ctx.respond(answer(200, m.drift_json())),
        "/api/alerts/summary" if get => {
            let c = lock(&m.0.al).counts(SystemTime::now());
            ctx.respond(answer(200, c));
        }
        "/api/alerts/config" if get => {
            let etag = m.alerts_etag();
            ctx.respond(answer(200, m.alerts_config_json()).header("ETag", &etag));
        }
        "/api/alerts" | "/api/alerts/summary" | "/api/alerts/config" | "/api/drift" => {
            ctx.respond(not_allowed("GET"));
        }
        "/api/alerts/rules"
        | "/api/alerts/webhooks"
        | "/api/alerts/silences"
        | "/api/drift/fix"
            if !post =>
        {
            ctx.respond(not_allowed("POST"));
        }
        "/api/alerts/rules" => {
            let Some(v) = decode(ctx) else { return true };
            if !matches(m, ctx) {
                return true;
            }
            match Rule::from_value(&v) {
                Ok(r) => on_pool(m, ctx, 201, move |me| me.add_rule(&user, r)),
                Err(e) => ctx.respond(e.response()),
            }
        }
        "/api/alerts/webhooks" => {
            let Some(v) = decode(ctx) else { return true };
            if matches(m, ctx) {
                on_pool(m, ctx, 201, move |me| me.add_webhook(&user, &v));
            }
        }
        "/api/alerts/silences" => {
            let Some(v) = decode(ctx) else { return true };
            if matches(m, ctx) {
                on_pool(m, ctx, 201, move |me| me.add_silence(&user, &v));
            }
        }
        "/api/drift/fix" => {
            let Some(v) = decode(ctx) else { return true };
            let nodes = json::strings_of(&v, "nodes");
            if m.0.closed.load(Ordering::Acquire) {
                ctx.respond(fail(503, "the manager is shutting down"));
                return true;
            }
            let me = m.clone();
            crate::api::on_pool(m, ctx, move || match me.fix_drift(&user, nodes) {
                Ok(body) => answer(202, body),
                Err(e) => e.response(),
            });
        }
        p => {
            let items = [
                ("/api/alerts/rules/", "PUT, DELETE"),
                ("/api/alerts/webhooks/", "PUT, DELETE"),
                ("/api/alerts/silences/", "DELETE"),
            ];
            let Some((prefix, allow)) = items.iter().find(|(x, _)| p.starts_with(x)) else {
                return false;
            };
            let rest = p.trim_start_matches(prefix);
            let (name, action) = rest.split_once('/').unwrap_or((rest, ""));
            let name = crate::api::percent_decode(name);
            match (*prefix, action, &method) {
                ("/api/alerts/webhooks/", "test", Method::Post) => {
                    webhooks::test(m, ctx, &user, &name);
                }
                (_, a, _) if !a.is_empty() => ctx.respond(fail(404, "no such endpoint")),
                ("/api/alerts/rules/", _, Method::Put) => {
                    let Some(v) = decode(ctx) else { return true };
                    if !matches(m, ctx) {
                        return true;
                    }
                    match Rule::from_value(&v) {
                        Ok(r) => on_pool(m, ctx, 200, move |me| me.update_rule(&user, &name, r)),
                        Err(e) => ctx.respond(e.response()),
                    }
                }
                ("/api/alerts/rules/", _, Method::Delete) => {
                    if matches(m, ctx) {
                        on_pool(m, ctx, 200, move |me| me.remove_rule(&user, &name));
                    }
                }
                ("/api/alerts/webhooks/", _, Method::Put) => {
                    let Some(v) = decode(ctx) else { return true };
                    if matches(m, ctx) {
                        on_pool(m, ctx, 200, move |me| me.update_webhook(&user, &name, &v));
                    }
                }
                ("/api/alerts/webhooks/", _, Method::Delete) => {
                    if matches(m, ctx) {
                        on_pool(m, ctx, 200, move |me| me.remove_webhook(&user, &name));
                    }
                }
                ("/api/alerts/silences/", _, Method::Delete) => {
                    if matches(m, ctx) {
                        on_pool(m, ctx, 200, move |me| me.expire_silence(&user, &name));
                    }
                }
                _ => ctx.respond(not_allowed(allow)),
            }
        }
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

    fn refused(mut r: Rule) -> String {
        match r.check() {
            Err(Refusal::Field(f)) => f.field,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rules_checked() {
        // fit's TestAlertRuleCheck.
        let defaults = default_rules();
        assert_eq!(defaults.len(), 6);
        assert_eq!(defaults[0].severity, "critical");
        assert_eq!(defaults[3].days, DEFAULT_CERT_DAYS);
        let r = |kind: &str| Rule {
            name: "r".into(),
            kind: kind.into(),
            ..Rule::default()
        };
        assert_eq!(
            refused(Rule {
                name: "-x".into(),
                ..r(DRIFT)
            }),
            "name"
        );
        assert_eq!(refused(r("nope")), "kind");
        assert_eq!(
            refused(Rule {
                severity: "loud".into(),
                ..r(DRIFT)
            }),
            "severity"
        );
        assert_eq!(
            refused(Rule {
                repeat_ms: 1000,
                ..r(DRIFT)
            }),
            "repeat_ms"
        );
        assert_eq!(
            refused(Rule {
                days: 400,
                ..r(CERT_EXPIRY)
            }),
            "days"
        );
        assert_eq!(
            refused(Rule {
                source: "x".into(),
                ..r(DRIFT)
            }),
            "source"
        );
        let mut x = Rule {
            days: 3,
            threshold: 4,
            source: "templates".into(),
            resolve_after_ms: 5,
            webhooks: vec!["b".into(), "a".into(), "b".into()],
            ..r(MEMBER_DOWN)
        };
        x.check().unwrap();
        assert_eq!((x.days, x.threshold, x.source.as_str()), (0, 0, ""));
        assert_eq!(x.resolve_after_ms, 0);
        assert_eq!(x.webhooks, ["a", "b"]);
        let back = Rule::from_value(&json::parse(x.json().as_bytes()).unwrap()).unwrap();
        assert_eq!(back, x);
    }

    #[test]
    fn silences_checked() {
        let now = SystemTime::now();
        let v = |s: &str| json::parse(s.as_bytes()).unwrap();
        let s = silence_of(&v(r#"{"rule": "drift", "duration_ms": 60000}"#), "ada", now).unwrap();
        assert!(s.active(now) && s.ends == now.checked_add(Duration::from_secs(60)));
        let a = Alert {
            rule: "drift".into(),
            node: "x".into(),
            ..Alert::default()
        };
        assert!(s.matches(&a));
        let err = |body: &str| match silence_of(&v(body), "ada", now) {
            Err(Refusal::Field(f)) => f.field,
            other => panic!("{other:?}"),
        };
        assert_eq!(err("{}"), "rule");
        assert_eq!(err(r#"{"node": "x"}"#), "ends");
        assert_eq!(
            err(r#"{"node": "x", "duration_ms": 9999999999999}"#),
            "ends"
        );
        let back = Silence::from_value(&v(&s.json()));
        assert_eq!(back.ends.map(rfc3339), s.ends.map(rfc3339));
        let mut list = vec![
            Silence {
                ends: now.checked_sub(Duration::from_secs(8 * 24 * 3600)),
                ..s.clone()
            },
            s,
        ];
        prune(&mut list, now);
        assert_eq!(list.len(), 1);
    }
}
