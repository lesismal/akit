//! The statuses akit writes back (fit's `status.go`): whether each of akit's GatewayClasses and
//! Gateways is accepted and programmed, each of a Gateway's listeners, and each route's status
//! on each of akit's Gateways, as JSON merge patches of the objects whose status differs from
//! what it should be. A condition that changes is said to have changed now; one that does not
//! keeps its time. Each patch names the resource version it was made from, so that the API
//! server refuses it when the object changed since.

use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use akit_config::value::{Value, write_string};

use super::gateway::{GATEWAY_GROUP, Why};
use super::translate::Translator;
use super::types::{
    Condition, GatewayAddress, GatewayStatus, ListenerStatus, Meta, Object, ParentRef,
    RouteGroupKind, RouteParentStatus, parent_status,
};

/// How many parents a route's status holds, as the Gateway API bounds it.
pub(crate) const MAX_PARENTS: usize = 32;

/// A status to write: a JSON merge patch of the status of the object of a kind, by its plural,
/// and its namespace and name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusUpdate {
    pub(crate) kind: &'static str,
    pub(crate) ns: String,
    pub(crate) name: String,
    pub(crate) patch: Vec<u8>,
}

impl Translator<'_> {
    /// The statuses akit's GatewayClasses, Gateways and the routes in the namespaces watched
    /// should have, after [`Translator::run`], where they differ from those they have.
    pub(crate) fn statuses(&self, now: SystemTime) -> Vec<StatusUpdate> {
        let at = rfc3339(now);
        let controller = &self.opts.controller_name;
        let mut out = Vec::new();
        let add = |out: &mut Vec<StatusUpdate>, kind: &'static str, m: &Meta, status: &str| {
            let mut p = String::from(r#"{"metadata":{"resourceVersion":"#);
            write_string(&mut p, &m.resource_version);
            p.push_str(r#"},"status":"#);
            p.push_str(status);
            p.push('}');
            out.push(StatusUpdate {
                kind,
                ns: m.namespace.clone(),
                name: m.name.clone(),
                patch: p.into_bytes(),
            });
        };

        for o in &self.classes {
            let Object::GatewayClass(c) = &**o else {
                continue;
            };
            if &c.controller_name != controller {
                continue;
            }
            let want = settle(
                vec![cond(
                    "Accepted",
                    true,
                    "Accepted",
                    "akit serves this class's Gateways",
                    c.meta.generation,
                )],
                &c.conditions,
                &at,
            );
            if conditions_json(&want) != conditions_json(&c.conditions) {
                let mut st = String::from(r#"{"conditions":"#);
                st.push_str(&conditions_json(&want));
                st.push('}');
                add(&mut out, "gatewayclasses", &c.meta, &st);
            }
        }

        for gw in &self.gws {
            let Some(g) = gw.gateway() else {
                continue;
            };
            let generation = g.meta.generation;
            // Addresses as `addresses` gives them, or as they are when it gives none.
            let mut want = GatewayStatus {
                addresses: g.status.addresses.clone(),
                ..GatewayStatus::default()
            };
            if !self.opts.addresses.is_empty() {
                want.addresses = self
                    .opts
                    .addresses
                    .iter()
                    .map(|a| GatewayAddress {
                        typ: Some(
                            if a.parse::<std::net::IpAddr>().is_ok() {
                                "IPAddress"
                            } else {
                                "Hostname"
                            }
                            .to_owned(),
                        ),
                        value: a.clone(),
                    })
                    .collect();
            }
            let (mut accepted, mut programmed, mut valid) = (0usize, 0usize, 0usize);
            for gl in &gw.listeners {
                let have = g
                    .status
                    .listeners
                    .iter()
                    .rfind(|h| h.name == gl.l.name)
                    .map_or(&[][..], |h| &h.conditions[..]);
                let conditions = settle(
                    vec![
                        why_cond(
                            "Accepted",
                            gl.refused.as_ref(),
                            "Accepted",
                            false,
                            generation,
                        ),
                        programmed_cond(
                            gl.fit.is_some(),
                            "akit does not serve it: see its other conditions",
                            generation,
                        ),
                        why_cond(
                            "ResolvedRefs",
                            gl.unresolved.as_ref(),
                            "ResolvedRefs",
                            false,
                            generation,
                        ),
                        why_cond(
                            "Conflicted",
                            gl.conflict.as_ref(),
                            "NoConflicts",
                            true,
                            generation,
                        ),
                    ],
                    have,
                    &at,
                );
                want.listeners.push(ListenerStatus {
                    name: gl.l.name.clone(),
                    supported_kinds: gl.kinds.clone(),
                    attached_routes: gl.attached,
                    conditions,
                });
                if gl.refused.is_none() {
                    accepted = accepted.saturating_add(1);
                }
                if gl.fit.is_some() {
                    programmed = programmed.saturating_add(1);
                }
                if gl.refused.is_none() && gl.conflict.is_none() && gl.unresolved.is_none() {
                    valid = valid.saturating_add(1);
                }
            }
            let reason = if valid < gw.listeners.len() {
                "ListenersNotValid"
            } else {
                "Accepted"
            };
            want.conditions = settle(
                vec![
                    cond("Accepted", accepted > 0, reason, "", generation),
                    programmed_cond(
                        programmed > 0,
                        "akit serves none of its listeners",
                        generation,
                    ),
                ],
                &g.status.conditions,
                &at,
            );
            let st = gateway_status_json(&want);
            if st != gateway_status_json(&g.status) {
                add(&mut out, "gateways", &g.meta, &st);
            }
        }

        for st in &self.states {
            let Some(r) = st.route() else {
                continue;
            };
            let mut mine: Vec<RouteParentStatus> = Vec::new();
            let mut others: Vec<&Value> = Vec::new();
            for raw in &r.parents {
                let mut p = parent_status(raw);
                if &p.controller_name == controller {
                    p.parent_ref = normal_ref(&p.parent_ref);
                    mine.push(p);
                } else {
                    others.push(raw);
                }
            }
            let generation = r.meta.generation;
            let mut want: Vec<RouteParentStatus> = Vec::new();
            for ps in &st.parents {
                if others.len().saturating_add(want.len()) >= MAX_PARENTS {
                    break;
                }
                let mut conditions = vec![
                    why_cond("Accepted", ps.why.as_ref(), "Accepted", false, generation),
                    why_cond(
                        "ResolvedRefs",
                        st.unresolved.as_ref(),
                        "ResolvedRefs",
                        false,
                        generation,
                    ),
                ];
                if let Some(msg) = &st.partial {
                    conditions.push(cond(
                        "PartiallyInvalid",
                        true,
                        "UnsupportedValue",
                        msg,
                        generation,
                    ));
                }
                let parent_ref = normal_ref(&ps.r);
                let rj = parent_ref_json(&parent_ref);
                let have = mine
                    .iter()
                    .rfind(|h| parent_ref_json(&h.parent_ref) == rj)
                    .map_or(&[][..], |h| &h.conditions[..]);
                let conditions = settle(conditions, have, &at);
                want.push(RouteParentStatus {
                    parent_ref,
                    controller_name: controller.clone(),
                    conditions,
                });
            }
            let json = |ps: &[RouteParentStatus]| -> Vec<String> {
                ps.iter().map(parent_status_json).collect()
            };
            let (want_j, mine_j) = (json(&want), json(&mine));
            if want_j == mine_j {
                continue;
            }
            let mut s = String::from(r#"{"parents":["#);
            let mut first = true;
            for p in others.iter().map(|v| value_json(v)).chain(want_j) {
                if !first {
                    s.push(',');
                }
                first = false;
                s.push_str(&p);
            }
            s.push_str("]}");
            add(&mut out, st.kind.plural(), &r.meta, &s);
        }
        out
    }
}

/// A condition of type `typ`, True when `ok`, with no time yet.
fn cond(typ: &str, ok: bool, reason: &str, msg: &str, generation: i64) -> Condition {
    Condition {
        typ: typ.to_owned(),
        status: if ok { "True" } else { "False" }.to_owned(),
        observed_generation: generation,
        last_transition_time: String::new(),
        reason: reason.to_owned(),
        message: msg.to_owned(),
    }
}

/// A condition that is as it should be without `why`: True with `reason` (False when
/// `inverted`, as Conflicted is), or else as `why` says.
fn why_cond(
    typ: &str,
    why: Option<&Why>,
    reason: &str,
    inverted: bool,
    generation: i64,
) -> Condition {
    match why {
        None => cond(typ, !inverted, reason, "", generation),
        Some((r, msg)) => cond(typ, inverted, r, msg, generation),
    }
}

/// A Programmed condition, with why not when not `ok`.
fn programmed_cond(ok: bool, why: &str, generation: i64) -> Condition {
    if ok {
        cond("Programmed", true, "Programmed", "", generation)
    } else {
        cond("Programmed", false, "Invalid", why, generation)
    }
}

/// Gives each of `want`'s conditions the time of `have`'s of its type when its status is the
/// same, and `at` otherwise.
fn settle(mut want: Vec<Condition>, have: &[Condition], at: &str) -> Vec<Condition> {
    for w in &mut want {
        at.clone_into(&mut w.last_transition_time);
        for h in have {
            if h.typ == w.typ && h.status == w.status && !h.last_transition_time.is_empty() {
                w.last_transition_time.clone_from(&h.last_transition_time);
            }
        }
    }
    want
}

/// `r` with its group and kind written out, as the API server's defaults write them, so that
/// refs compare as they are stored.
fn normal_ref(r: &ParentRef) -> ParentRef {
    let mut r = r.clone();
    r.group = Some(r.group.unwrap_or_else(|| GATEWAY_GROUP.to_owned()));
    r.kind = Some(r.kind.unwrap_or_else(|| "Gateway".to_owned()));
    r
}

/// The path of the object named `name` in namespace `ns` (none for a cluster's) of the kind
/// whose path across namespaces is `collection`.
pub(crate) fn object_path(collection: &str, ns: &str, name: &str) -> String {
    if ns.is_empty() {
        return format!("{collection}/{name}");
    }
    match collection.rsplit_once('/') {
        Some((dir, plural)) => format!("{dir}/namespaces/{ns}/{plural}/{name}"),
        None => format!("{collection}/{name}"),
    }
}

/// `t` as RFC 3339 in UTC, to the second.
// Days from seconds fit an i64 many times over: none of the arithmetic below can overflow.
#[allow(clippy::arithmetic_side_effects)]
pub(crate) fn rfc3339(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days.saturating_add(719_468);
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn field(b: &mut String, first: &mut bool, k: &str) {
    if !*first {
        b.push(',');
    }
    *first = false;
    write_string(b, k);
    b.push(':');
}

fn condition_json(b: &mut String, c: &Condition) {
    b.push_str(r#"{"type":"#);
    write_string(b, &c.typ);
    b.push_str(r#","status":"#);
    write_string(b, &c.status);
    if c.observed_generation != 0 {
        let _ = write!(b, r#","observedGeneration":{}"#, c.observed_generation);
    }
    b.push_str(r#","lastTransitionTime":"#);
    write_string(b, &c.last_transition_time);
    b.push_str(r#","reason":"#);
    write_string(b, &c.reason);
    b.push_str(r#","message":"#);
    write_string(b, &c.message);
    b.push('}');
}

fn list<T>(b: &mut String, items: &[T], each: impl Fn(&mut String, &T)) {
    b.push('[');
    for (i, x) in items.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        each(b, x);
    }
    b.push(']');
}

fn conditions_json(cs: &[Condition]) -> String {
    let mut b = String::new();
    list(&mut b, cs, condition_json);
    b
}

fn group_kind_json(b: &mut String, k: &RouteGroupKind) {
    b.push('{');
    let mut first = true;
    if let Some(g) = &k.group {
        field(b, &mut first, "group");
        write_string(b, g);
    }
    field(b, &mut first, "kind");
    write_string(b, &k.kind);
    b.push('}');
}

/// A Gateway's status, as JSON: its fields left out when empty.
fn gateway_status_json(s: &GatewayStatus) -> String {
    let mut b = String::from("{");
    let mut first = true;
    if !s.addresses.is_empty() {
        field(&mut b, &mut first, "addresses");
        list(&mut b, &s.addresses, |b, a| {
            b.push('{');
            let mut first = true;
            if let Some(t) = &a.typ {
                field(b, &mut first, "type");
                write_string(b, t);
            }
            field(b, &mut first, "value");
            write_string(b, &a.value);
            b.push('}');
        });
    }
    if !s.conditions.is_empty() {
        field(&mut b, &mut first, "conditions");
        list(&mut b, &s.conditions, condition_json);
    }
    if !s.listeners.is_empty() {
        field(&mut b, &mut first, "listeners");
        list(&mut b, &s.listeners, |b, l| {
            b.push_str(r#"{"name":"#);
            write_string(b, &l.name);
            b.push_str(r#","supportedKinds":"#);
            list(b, &l.supported_kinds, group_kind_json);
            let _ = write!(b, r#","attachedRoutes":{}"#, l.attached_routes);
            b.push_str(r#","conditions":"#);
            list(b, &l.conditions, condition_json);
            b.push('}');
        });
    }
    b.push('}');
    b
}

fn parent_ref_json(r: &ParentRef) -> String {
    let mut b = String::from("{");
    let mut first = true;
    for (k, v) in [
        ("group", &r.group),
        ("kind", &r.kind),
        ("namespace", &r.namespace),
    ] {
        if let Some(v) = v {
            field(&mut b, &mut first, k);
            write_string(&mut b, v);
        }
    }
    field(&mut b, &mut first, "name");
    write_string(&mut b, &r.name);
    if let Some(s) = &r.section_name {
        field(&mut b, &mut first, "sectionName");
        write_string(&mut b, s);
    }
    if let Some(p) = r.port {
        field(&mut b, &mut first, "port");
        let _ = write!(b, "{p}");
    }
    b.push('}');
    b
}

fn parent_status_json(p: &RouteParentStatus) -> String {
    let mut b = String::from(r#"{"parentRef":"#);
    b.push_str(&parent_ref_json(&p.parent_ref));
    b.push_str(r#","controllerName":"#);
    write_string(&mut b, &p.controller_name);
    b.push_str(r#","conditions":"#);
    list(&mut b, &p.conditions, condition_json);
    b.push('}');
    b
}

/// `v` as compact JSON.
pub(crate) fn value_json(v: &Value) -> String {
    let mut b = String::new();
    write_value(&mut b, v);
    b
}

fn write_value(b: &mut String, v: &Value) {
    match v {
        Value::Null => b.push_str("null"),
        Value::Bool(x) => b.push_str(if *x { "true" } else { "false" }),
        Value::Number(n) => b.push_str(n),
        Value::String(s) => write_string(b, s),
        Value::Array(a) => list(b, a, write_value),
        Value::Object(ms) => {
            b.push('{');
            for (i, (k, x)) in ms.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                write_string(b, k);
                b.push(':');
                write_value(b, x);
            }
            b.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{object_path, rfc3339};
    use std::time::{Duration, UNIX_EPOCH};

    /// fit's `TestObjectPath`.
    #[test]
    fn object_paths() {
        let g = "/apis/gateway.networking.k8s.io/";
        for (coll, ns, name, want) in [
            (
                format!("{g}v1/gatewayclasses"),
                "",
                "fit",
                format!("{g}v1/gatewayclasses/fit"),
            ),
            (
                format!("{g}v1/gateways"),
                "default",
                "gw",
                format!("{g}v1/namespaces/default/gateways/gw"),
            ),
            (
                format!("{g}v1alpha2/tlsroutes"),
                "team",
                "db",
                format!("{g}v1alpha2/namespaces/team/tlsroutes/db"),
            ),
        ] {
            assert_eq!(object_path(&coll, ns, name), want);
        }
    }

    #[test]
    fn times() {
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            rfc3339(UNIX_EPOCH + Duration::from_secs(1_792_663_200)),
            "2026-10-22T10:00:00Z"
        );
        assert_eq!(
            rfc3339(UNIX_EPOCH + Duration::from_secs(951_825_599)),
            "2000-02-29T11:59:59Z"
        );
    }
}
