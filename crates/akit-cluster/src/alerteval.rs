//! The alerts' evaluation (I138, fit's `cluster/alerteval.go`): the rules over the members'
//! states, on a thread of the manager's, never an I/O thread. It runs when a poll ends, a rule,
//! webhook or silence changes, a member's templates were read, and when the earliest of its
//! times comes (an alert pending for its rule's time, one to send again, a silence that starts
//! or ends, a failed rollout's or reload's alert to resolve): those waits are the logic of time
//! (the rule: timers only where the logic is about time), and nothing else wakes it. fit ticks
//! every second instead.
//!
//! The members a template manages are read against their templates when their running config
//! changed since they were last read (their `GET /node`'s ETag), at most `max_concurrency` at
//! once and once a poll, so that a drift rule sees a member changed by hand; fit reads them again
//! every ten minutes too.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, SystemTime};

use akit_config::value::Value;

use crate::Manager;
use crate::alerts::{
    Al, Alert, BACKENDS_DOWN, CERT_EXPIRY, DRIFT, DRIFT_CONSISTENCY, DRIFT_TEMPLATES, FIRING,
    MAX_ACTIVE, MAX_HISTORY, MAX_TEXT, MEMBER_DOWN, PENDING, RESOLVED, ROLLOUT_FAILED, Rule, Track,
    key, lock, ms, rfc3339,
};
use crate::api::clip;
use crate::json;
use crate::node::{STATUS_DOWN, STATUS_ERROR, STATUS_UNKNOWN, STATUS_UP};
use crate::webhooks::{Notice, due};

/// A member as the rules read it.
#[derive(Debug, Clone, Default)]
struct Member {
    name: String,
    status: &'static str,
    since: Option<SystemTime>,
    error: String,
    groups: Vec<String>,
    polled: Option<SystemTime>,
    reloads: Option<(u64, u64)>,
    drift: Option<i64>,
    cert: Option<(SystemTime, Vec<String>, String)>,
    backends: Option<(u64, u64)>,
    etag: String,
}

/// A rollout that failed.
#[derive(Debug, Clone)]
struct Failed {
    id: String,
    what: String,
    reason: String,
    ended: Option<SystemTime>,
    nodes: Vec<String>,
}

/// What the rules read: fit's `evalInput`.
#[derive(Debug, Default)]
struct Input {
    members: Vec<Member>,
    syncs: HashMap<String, Value>,
    rollouts: Vec<Failed>,
}

/// What a rule found on a member or a subject: fit's `finding`.
#[derive(Debug, Clone, Default)]
struct Finding {
    node: String,
    subject: String,
    summary: String,
    since: Option<SystemTime>,
}

/// What each webhook is sent: the alerts firing, and those resolved.
type Out = BTreeMap<String, (Vec<Alert>, Vec<Alert>)>;

fn in_scope(r: &Rule, groups: &[String]) -> bool {
    r.group.is_empty() || groups.contains(&r.group)
}

fn after(t: Option<SystemTime>, now: SystemTime) -> Duration {
    t.and_then(|t| now.duration_since(t).ok())
        .unwrap_or_default()
}

impl Al {
    /// Follows each member's reloads, poll by poll: fit's `trackReloads`.
    fn track_reloads(&mut self, members: &[Member]) {
        let names: BTreeSet<&str> = members.iter().map(|m| m.name.as_str()).collect();
        for m in members {
            let Some((ok, failed)) = m.reloads.filter(|_| m.status == STATUS_UP) else {
                continue;
            };
            let Some(t) = self.reloads.get_mut(&m.name) else {
                self.reloads.insert(
                    m.name.clone(),
                    Track {
                        polled: m.polled,
                        succeeded: ok,
                        failed,
                        ..Track::default()
                    },
                );
                continue;
            };
            if m.polled <= t.polled {
                continue;
            }
            t.polled = m.polled;
            if failed < t.failed || ok < t.succeeded {
                t.failing = false;
            } else if failed > t.failed {
                t.failing = true;
                t.failed_at = m.polled;
            } else if ok > t.succeeded {
                t.failing = false;
            }
            t.succeeded = ok;
            t.failed = failed;
        }
        self.reloads.retain(|n, _| names.contains(n.as_str()));
    }

    /// What rule `r` finds, and the members not polled yet: fit's `findings`.
    fn findings(
        &self,
        r: &Rule,
        input: &Input,
        now: SystemTime,
    ) -> (Vec<Finding>, BTreeSet<String>) {
        let mut out = Vec::new();
        let mut unknown = BTreeSet::new();
        if r.kind == ROLLOUT_FAILED {
            for ro in &input.rollouts {
                if !r.group.is_empty()
                    && !ro.nodes.iter().any(|n| {
                        input
                            .members
                            .iter()
                            .any(|m| m.name == *n && m.groups.contains(&r.group))
                    })
                {
                    continue;
                }
                if r.resolve_after_ms > 0 && after(ro.ended, now) >= ms(r.resolve_after_ms) {
                    continue;
                }
                out.push(Finding {
                    subject: ro.id.clone(),
                    since: ro.ended,
                    summary: clip(
                        &format!("the rollout {} ({}) failed: {}", ro.id, ro.what, ro.reason),
                        MAX_TEXT,
                    ),
                    ..Finding::default()
                });
            }
            return (out, unknown);
        }
        for m in &input.members {
            if !in_scope(r, &m.groups) {
                continue;
            }
            if m.status == STATUS_UNKNOWN {
                unknown.insert(m.name.clone());
                continue;
            }
            if let Some(mut f) = self.member_finding(r, m, input, now) {
                f.node.clone_from(&m.name);
                out.push(f);
            }
        }
        (out, unknown)
    }

    /// What rule `r` finds on member `m`: fit's `memberFinding`.
    fn member_finding(
        &self,
        r: &Rule,
        m: &Member,
        input: &Input,
        now: SystemTime,
    ) -> Option<Finding> {
        let found = |summary: String, since: Option<SystemTime>| Finding {
            summary: clip(&summary, MAX_TEXT),
            since,
            ..Finding::default()
        };
        match r.kind.as_str() {
            MEMBER_DOWN => {
                if m.status != STATUS_DOWN && m.status != STATUS_ERROR {
                    return None;
                }
                let mut s = format!(
                    "{} since {}",
                    m.status,
                    m.since.map_or_else(String::new, rfc3339)
                );
                if !m.error.is_empty() {
                    s.push_str(&format!(": {}", m.error));
                }
                Some(found(s, m.since))
            }
            DRIFT => {
                let mut parts = Vec::new();
                if let Some(s) = input
                    .syncs
                    .get(&m.name)
                    .filter(|_| r.source != DRIFT_CONSISTENCY)
                {
                    let state = json::str_of(s, "state");
                    let counts = s.get("counts").cloned().unwrap_or(Value::Null);
                    let errors = json::strings_of(s, "errors");
                    if state == "out_of_sync" || state == "conflict" {
                        parts.push(format!(
                            "out of sync with its templates: {} parts out of sync, {} in conflict",
                            json::int_of(&counts, "out_of_sync"),
                            json::int_of(&counts, "conflict")
                        ));
                    } else if state == "error" {
                        if let Some(e) = errors.first() {
                            parts.push(format!("its templates do not render for it: {e}"));
                        }
                    }
                }
                if let Some(d) = m.drift.filter(|d| *d > 0 && r.source != DRIFT_TEMPLATES) {
                    parts.push(format!("its consistency check finds {d} differences"));
                }
                (!parts.is_empty()).then(|| found(parts.join("; "), None))
            }
            CERT_EXPIRY => {
                let (at, names, listener) = m.cert.as_ref()?;
                let days =
                    Duration::from_secs(u64::try_from(r.days).unwrap_or(0).saturating_mul(86_400));
                let left = at.duration_since(now).ok();
                if left.is_some_and(|l| l >= days) {
                    return None;
                }
                let mut what = format!("the certificate of {}", names.join(", "));
                if !listener.is_empty() {
                    what.push_str(&format!(" (listener {listener})"));
                }
                let when = rfc3339(*at);
                Some(match left.filter(|l| !l.is_zero()) {
                    None => found(format!("{what} expired on {when}"), None),
                    Some(l) => found(
                        format!("{what} expires in {} days, on {when}", l.as_secs() / 86_400),
                        None,
                    ),
                })
            }
            BACKENDS_DOWN => {
                let (down, total) = m.backends?;
                if down < u64::try_from(r.threshold).unwrap_or(u64::MAX) {
                    return None;
                }
                Some(found(
                    format!("{down} of its {total} upstream backends are down"),
                    None,
                ))
            }
            _ => {
                let t = self.reloads.get(&m.name)?;
                if !t.failing
                    || (r.resolve_after_ms > 0 && after(t.failed_at, now) >= ms(r.resolve_after_ms))
                {
                    return None;
                }
                Some(found(
                    format!(
                        "a reload failed, seen at {}: {} failed and {} succeeded since it started",
                        t.failed_at.map_or_else(String::new, rfc3339),
                        t.failed,
                        t.succeeded
                    ),
                    t.failed_at,
                ))
            }
        }
    }

    /// The webhooks rule `r`'s alerts go to: fit's `targets`.
    fn targets(&self, r: Option<&Rule>, resolved: bool) -> Vec<String> {
        self.cfg
            .webhooks
            .iter()
            .filter(|w| !w.disabled && (!resolved || w.send_resolved))
            .filter(|w| r.is_none_or(|r| r.webhooks.is_empty() || r.webhooks.contains(&w.name)))
            .map(|w| w.name.clone())
            .collect()
    }

    fn resolve(
        &mut self,
        k: &str,
        now: SystemTime,
        why: &str,
        log: &dyn Fn(String),
    ) -> Option<Alert> {
        let mut a = self.active.remove(k)?;
        a.state = RESOLVED.to_owned();
        a.resolved = Some(now);
        why.clone_into(&mut a.resolved_why);
        self.history.push(a.clone());
        let excess = self.history.len().saturating_sub(MAX_HISTORY);
        self.history.drain(..excess);
        self.dirty = true;
        let why = if why.is_empty() {
            "its condition is gone"
        } else {
            why
        };
        log(format!(
            "cluster: the alert {} ({}) resolved{}: {why}",
            a.id,
            a.rule,
            on_what(&a)
        ));
        Some(a)
    }

    /// An alert of rule `r` held by `f`: pending until the rule's time, then firing: fit's
    /// `hold`.
    fn hold(&mut self, r: &Rule, k: &str, f: &Finding, now: SystemTime, log: &dyn Fn(String)) {
        if let Some(a) = self.active.get_mut(k) {
            if a.kind == r.kind {
                a.summary.clone_from(&f.summary);
                a.severity.clone_from(&r.severity);
                return;
            }
        }
        if self.active.contains_key(k) {
            self.resolve(k, now, "the rule changed", log);
        }
        let stale = self.pending.get(k).is_none_or(|a| a.kind != r.kind);
        if stale {
            if self.active.len().saturating_add(self.pending.len()) >= MAX_ACTIVE {
                if self
                    .full
                    .is_none_or(|t| after(Some(t), now) > Duration::from_secs(3600))
                {
                    self.full = Some(now);
                    log(format!(
                        "cluster: {MAX_ACTIVE} alerts are firing or pending, the most kept: new \
                         ones are not followed until some resolve"
                    ));
                }
                return;
            }
            let since = f.since.filter(|s| *s <= now).unwrap_or(now);
            self.pending.insert(
                k.to_owned(),
                Alert {
                    rule: r.name.clone(),
                    kind: r.kind.clone(),
                    node: f.node.clone(),
                    subject: f.subject.clone(),
                    state: PENDING.to_owned(),
                    since: Some(since),
                    ..Alert::default()
                },
            );
        }
        let Some(a) = self.pending.get_mut(k) else {
            return;
        };
        a.summary.clone_from(&f.summary);
        a.severity.clone_from(&r.severity);
        if after(a.since, now) < ms(r.for_ms) {
            return;
        }
        let Some(mut a) = self.pending.remove(k) else {
            return;
        };
        a.id = crate::ops::new_id();
        FIRING.clone_into(&mut a.state);
        a.fired = Some(now);
        log(format!(
            "cluster: the alert {} ({}) fired{}: {}",
            a.id,
            a.rule,
            on_what(&a),
            a.summary
        ));
        self.active.insert(k.to_owned(), a);
        self.dirty = true;
    }

    /// Every rule over `input`: what each webhook is sent. fit's `evaluate`.
    fn evaluate(&mut self, now: SystemTime, input: &Input, log: &dyn Fn(String)) -> Out {
        self.track_reloads(&input.members);
        let members: BTreeSet<&str> = input.members.iter().map(|m| m.name.as_str()).collect();
        let rules: Vec<Rule> = self
            .cfg
            .rules
            .iter()
            .filter(|r| !r.disabled)
            .cloned()
            .collect();
        let mut held = BTreeSet::new();
        let mut founds = Vec::new();
        for r in &rules {
            let (fs, unknown) = self.findings(r, input, now);
            for f in &fs {
                held.insert(key(&r.name, &f.node, &f.subject));
            }
            for (k, a) in self.active.iter().chain(self.pending.iter()) {
                if a.rule == r.name
                    && a.kind == r.kind
                    && !a.node.is_empty()
                    && unknown.contains(&a.node)
                {
                    held.insert(k.clone());
                }
            }
            founds.push((r.clone(), fs));
        }
        let mut out = Out::new();
        let all: Vec<(String, Alert)> = self
            .active
            .iter()
            .chain(self.pending.iter())
            .map(|(k, a)| (k.clone(), a.clone()))
            .collect();
        for (k, a) in all {
            if held.contains(&k) {
                continue;
            }
            let r = rules.iter().find(|r| r.name == a.rule);
            if a.state == PENDING {
                self.pending.remove(&k);
                continue;
            }
            let why = match r {
                None => "the rule was removed or turned off",
                Some(r) if r.kind != a.kind => "the rule changed",
                _ if !a.node.is_empty() && !members.contains(a.node.as_str()) => {
                    "the member is no longer managed"
                }
                _ => "",
            };
            let Some(done) = self.resolve(&k, now, why, log) else {
                continue;
            };
            if done.sends > 0 {
                for w in self.targets(r, true) {
                    out.entry(w).or_default().1.push(done.clone());
                }
            }
        }
        for (r, fs) in &founds {
            for f in fs {
                self.hold(r, &key(&r.name, &f.node, &f.subject), f, now, log);
            }
        }
        let keys: Vec<String> = self.active.keys().cloned().collect();
        for k in keys {
            let Some(a) = self.active.get(&k).cloned() else {
                continue;
            };
            let Some(r) = rules.iter().find(|r| r.name == a.rule) else {
                continue;
            };
            if self.silence(&a, now).is_some() {
                continue;
            }
            if a.sends > 0 && (r.repeat_ms == 0 || after(a.last_sent, now) < ms(r.repeat_ms)) {
                continue;
            }
            let targets = self.targets(Some(r), false);
            if targets.is_empty() {
                continue;
            }
            let Some(x) = self.active.get_mut(&k) else {
                continue;
            };
            x.sends = x.sends.saturating_add(1);
            x.last_sent = Some(now);
            let sent = x.clone();
            self.dirty = true;
            for w in targets {
                out.entry(w).or_default().0.push(sent.clone());
            }
        }
        out
    }

    /// When the evaluation is due next, by time alone: the earliest of an alert pending for its
    /// rule's time, one to send again, a silence's start or end, a failed rollout's or reload's
    /// alert to resolve.
    fn next_due(
        &self,
        now: SystemTime,
        rollouts: &[(Option<SystemTime>, Vec<String>)],
    ) -> Option<SystemTime> {
        let mut times = Vec::new();
        let rule = |name: &str| {
            self.cfg
                .rules
                .iter()
                .find(|r| r.name == name && !r.disabled)
        };
        for a in self.pending.values() {
            if let Some(r) = rule(&a.rule) {
                times.extend(due(a.since, ms(r.for_ms)));
            }
        }
        for a in self.active.values() {
            if let Some(r) = rule(&a.rule).filter(|r| r.repeat_ms > 0 && a.sends > 0) {
                times.extend(due(a.last_sent, ms(r.repeat_ms)));
            }
        }
        for s in &self.cfg.silences {
            times.extend(s.starts);
            times.extend(s.ends);
        }
        for r in self
            .cfg
            .rules
            .iter()
            .filter(|r| !r.disabled && r.resolve_after_ms > 0)
        {
            if r.kind == ROLLOUT_FAILED {
                for (ended, _) in rollouts {
                    times.extend(due(*ended, ms(r.resolve_after_ms)));
                }
            } else {
                for t in self.reloads.values().filter(|t| t.failing) {
                    times.extend(due(t.failed_at, ms(r.resolve_after_ms)));
                }
            }
        }
        times.into_iter().filter(|t| *t > now).min()
    }
}

fn on_what(a: &Alert) -> String {
    if a.node.is_empty() {
        format!(" on {}", a.subject)
    } else {
        format!(" on {}", a.node)
    }
}

impl Manager {
    /// Has the alerts' thread evaluate the rules.
    pub(crate) fn poke_alerts(&self) {
        if let Some(tx) = lock(&self.0.al_wake).as_ref() {
            let _ = tx.try_send(());
        }
    }

    /// Starts the alerts' thread.
    pub(crate) fn start_alerts(&self) -> std::io::Result<()> {
        let (tx, rx) = mpsc::sync_channel(1);
        *lock(&self.0.al_wake) = Some(tx);
        let me = self.clone();
        let h = std::thread::Builder::new()
            .name("akit-cluster-alerts".to_owned())
            .spawn(move || me.run_alerts(&rx))?;
        *lock(&self.0.al_thread) = Some(h);
        Ok(())
    }

    /// Stops the alerts' thread, which saves them, and closes the webhooks' senders.
    pub(crate) fn stop_alerts(&self) {
        lock(&self.0.al_wake).take();
        if let Some(h) = lock(&self.0.al_thread).take() {
            let _ = h.join();
        }
        self.close_senders();
    }

    fn run_alerts(&self, wake: &Receiver<()>) {
        loop {
            if self.0.closed.load(Ordering::Acquire) {
                self.save_alerts();
                return;
            }
            let input = self.alert_input();
            let rollouts: Vec<(Option<SystemTime>, Vec<String>)> = input
                .rollouts
                .iter()
                .map(|r| (r.ended, r.nodes.clone()))
                .collect();
            let out = {
                let log = |s: String| self.log(s);
                lock(&self.0.al).evaluate(SystemTime::now(), &input, &log)
            };
            self.deliver(out);
            self.check_drift(&input);
            self.save_alerts();
            let next = lock(&self.0.al).next_due(SystemTime::now(), &rollouts);
            let r = match next {
                Some(t) => {
                    wake.recv_timeout(t.duration_since(SystemTime::now()).unwrap_or_default())
                }
                None => wake.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            if r == Err(RecvTimeoutError::Disconnected) {
                self.save_alerts();
                return;
            }
        }
    }

    /// The members' states, their sync states and the rollouts failed: fit's `alertInput`.
    fn alert_input(&self) -> Input {
        let d = self.doc();
        let syncs = lock(&self.0.tpl)
            .last
            .iter()
            .filter_map(|(k, v)| json::parse(v.as_bytes()).map(|v| (k.clone(), v)))
            .collect();
        let parse = |s: &Option<String>| s.as_deref().and_then(|s| json::parse(s.as_bytes()));
        let members = d
            .members
            .iter()
            .map(|m| {
                let groups = crate::groups::groups_of(&d, &m.name, &m.labels);
                let Some(st) = self.node(&m.name).map(|n| n.state()) else {
                    return Member {
                        name: m.name.clone(),
                        status: STATUS_ERROR,
                        groups,
                        ..Member::default()
                    };
                };
                let reloads = parse(&st.reloads)
                    .map(|r| (json::uint_of(&r, "succeeded"), json::uint_of(&r, "failed")));
                let drift = parse(&st.drift).map(|v| json::int_of(&v, "count"));
                let cert = parse(&st.certificates).and_then(|c| {
                    let at = crate::rollouts::parse_time(json::str_of(&c, "nearest_expiry"))?;
                    Some((
                        at,
                        json::strings_of(&c, "nearest_names"),
                        json::str_of(&c, "nearest_listener").to_owned(),
                    ))
                });
                let backends = parse(&st.upstreams).map(|u| {
                    (
                        json::uint_of(&u, "backends_down"),
                        json::uint_of(&u, "backends"),
                    )
                });
                let etag = parse(&st.config)
                    .map(|c| json::str_of(&c, "etag").to_owned())
                    .unwrap_or_default();
                Member {
                    name: m.name.clone(),
                    status: st.status,
                    since: Some(st.since),
                    error: st
                        .error
                        .as_ref()
                        .map(|e| format!("{}: {}", e.kind, e.message))
                        .unwrap_or_default(),
                    groups,
                    polled: st.polled,
                    reloads,
                    drift,
                    cert,
                    backends,
                    etag,
                }
            })
            .collect();
        let rollouts = lock(&self.0.ro)
            .list
            .iter()
            .filter(|r| r.state == crate::rollouts::FAILED)
            .map(|r| Failed {
                id: r.id.clone(),
                what: crate::rollouts::describe(r),
                reason: r.reason.clone(),
                ended: r.ended,
                nodes: r.nodes(),
            })
            .collect();
        Input {
            members,
            syncs,
            rollouts,
        }
    }

    /// Sends each webhook what it is sent: fit's `deliver`.
    fn deliver(&self, out: Out) {
        for (name, (firing, resolved)) in out {
            let Some(s) = lock(&self.0.al).senders.get(&name).cloned() else {
                continue;
            };
            if !firing.is_empty() {
                s.enqueue(Notice::of(FIRING, firing));
            }
            if !resolved.is_empty() {
                s.enqueue(Notice::of(RESOLVED, resolved));
            }
        }
    }

    /// Reads against their templates the members a template manages whose running config
    /// changed since they were last read, once a poll: fit's `startDriftReads`.
    fn check_drift(&self, input: &Input) {
        let d = self.doc();
        let tgroups: BTreeSet<&String> = d.templates.iter().flat_map(|t| t.groups.iter()).collect();
        let names: BTreeSet<&str> = input.members.iter().map(|m| m.name.as_str()).collect();
        for m in &input.members {
            if m.status != STATUS_UP || !m.groups.iter().any(|g| tgroups.contains(g)) {
                continue;
            }
            let synced = input
                .syncs
                .get(&m.name)
                .map(|s| json::str_of(s, "config_etag").to_owned());
            if synced.as_deref().is_some_and(|e| e == m.etag) {
                continue;
            }
            {
                let mut al = lock(&self.0.al);
                if al.drift_busy.len() >= self.0.concurrency
                    || al.drift_busy.contains(&m.name)
                    || al
                        .drift_poll
                        .get(&m.name)
                        .is_some_and(|p| m.polled <= Some(*p))
                {
                    continue;
                }
                al.drift_busy.insert(m.name.clone());
                if let Some(p) = m.polled {
                    al.drift_poll.insert(m.name.clone(), p);
                }
            }
            let (me, name) = (self.clone(), m.name.clone());
            self.sync_member(
                &m.name,
                false,
                Box::new(move |_| {
                    lock(&me.0.al).drift_busy.remove(&name);
                    me.poke_alerts();
                }),
            );
        }
        lock(&self.0.al)
            .drift_poll
            .retain(|n, _| names.contains(n.as_str()));
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects
    )]
    use super::*;
    use crate::alerts::default_rules;
    use crate::webhooks::Webhook;

    fn member(name: &str, status: &'static str) -> Member {
        Member {
            name: name.into(),
            status,
            since: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1000)),
            ..Member::default()
        }
    }

    #[test]
    fn pending_fire_repeat_and_resolve() {
        // fit's TestAlertLifecycle: pending for the rule's time, firing, sent, sent again after
        // its repeat, silenced, resolved and sent as resolved.
        let mut al = Al::default();
        al.cfg.rules = default_rules();
        al.cfg.webhooks.push(Webhook {
            name: "w".into(),
            send_resolved: true,
            ..Webhook::default()
        });
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        let log = |_: String| {};
        let down = Input {
            members: vec![member("a", STATUS_DOWN)],
            ..Input::default()
        };
        // Down since t0 - 9000 s: past the minute at once.
        let out = al.evaluate(t0, &down, &log);
        assert_eq!(al.active.len(), 1);
        assert_eq!(out["w"].0.len(), 1);
        let a = al.active.values().next().unwrap().clone();
        assert_eq!((a.state.as_str(), a.sends), (FIRING, 1));
        // Not sent again before its repeat; sent at it.
        let out = al.evaluate(t0 + Duration::from_secs(60), &down, &log);
        assert!(out.is_empty());
        assert_eq!(
            al.next_due(t0 + Duration::from_secs(60), &[]),
            Some(t0 + ms(crate::alerts::DEFAULT_REPEAT_MS))
        );
        let out = al.evaluate(t0 + ms(crate::alerts::DEFAULT_REPEAT_MS), &down, &log);
        assert_eq!(out["w"].0[0].sends, 2);
        // Unknown holds it; up resolves it, sent as resolved.
        let unknown = Input {
            members: vec![member("a", STATUS_UNKNOWN)],
            ..Input::default()
        };
        al.evaluate(t0 + Duration::from_secs(5), &unknown, &log);
        assert_eq!(al.active.len(), 1);
        let up = Input {
            members: vec![member("a", STATUS_UP)],
            ..Input::default()
        };
        let out = al.evaluate(t0 + Duration::from_secs(6), &up, &log);
        assert!(al.active.is_empty());
        assert_eq!(out["w"].1[0].state, RESOLVED);
        assert_eq!(al.history.len(), 1);
    }

    #[test]
    fn pending_waits_its_time() {
        let mut al = Al::default();
        al.cfg.rules = default_rules();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        let mut m = member("a", STATUS_ERROR);
        m.since = Some(now);
        let input = Input {
            members: vec![m],
            ..Input::default()
        };
        al.evaluate(now, &input, &|_| {});
        assert_eq!((al.pending.len(), al.active.len()), (1, 0));
        assert_eq!(al.next_due(now, &[]), Some(now + Duration::from_secs(60)));
        al.evaluate(now + Duration::from_secs(60), &input, &|_| {});
        assert_eq!((al.pending.len(), al.active.len()), (0, 1));
        // Without webhooks, nothing is sent, and nothing is due by time.
        assert_eq!(al.next_due(now + Duration::from_secs(61), &[]), None);
    }

    #[test]
    fn findings_of_each_kind() {
        let mut al = Al::default();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100 * 86_400);
        let mut m = member("a", STATUS_UP);
        m.drift = Some(2);
        m.backends = Some((2, 3));
        m.cert = Some((
            now + Duration::from_secs(3 * 86_400),
            vec!["a.example".into()],
            "web".into(),
        ));
        m.reloads = Some((1, 0));
        m.polled = Some(now);
        let mut input = Input {
            members: vec![m.clone()],
            ..Input::default()
        };
        input.syncs.insert(
            "a".into(),
            json::parse(br#"{"state": "conflict", "counts": {"out_of_sync": 1, "conflict": 2}}"#)
                .unwrap(),
        );
        let rule = |kind: &str| {
            let mut r = Rule {
                name: kind.into(),
                kind: kind.into(),
                ..Rule::default()
            };
            r.check().unwrap();
            r
        };
        let f = |al: &Al, r: &Rule, input: &Input| al.findings(r, input, now).0;
        let drift = f(&al, &rule(DRIFT), &input);
        assert!(
            drift[0].summary.contains("2 in conflict")
                && drift[0].summary.contains("finds 2 differences")
        );
        assert!(
            f(&al, &rule(CERT_EXPIRY), &input)[0]
                .summary
                .contains("expires in 3 days")
        );
        assert_eq!(
            f(&al, &rule(BACKENDS_DOWN), &input)[0].summary,
            "2 of its 3 upstream backends are down"
        );
        assert!(f(&al, &rule(MEMBER_DOWN), &input).is_empty());
        // A reload failing, seen by the next poll.
        al.track_reloads(&input.members);
        m.reloads = Some((1, 1));
        m.polled = Some(now + Duration::from_secs(1));
        input.members = vec![m];
        al.track_reloads(&input.members);
        assert!(
            f(&al, &rule(crate::alerts::RELOAD_FAILED), &input)[0]
                .summary
                .contains("1 failed")
        );
        input.rollouts.push(Failed {
            id: "r1".into(),
            what: "a sync".into(),
            reason: "gate".into(),
            ended: Some(now),
            nodes: vec!["a".into()],
        });
        assert_eq!(f(&al, &rule(ROLLOUT_FAILED), &input)[0].subject, "r1");
    }
}
