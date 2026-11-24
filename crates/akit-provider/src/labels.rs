//! Routes, services and an upstream's settings read from key-value labels under a prefix, as
//! the Docker provider reads containers' labels and the Consul provider services' tags, and the
//! part they make: fit's `provider/labels`.
//!
//! The labels, under a prefix (`fit`, say):
//!
//! ```text
//! fit.port=<port>                 the port; the source's own when left out
//! fit.upstream=<JSON object>      more of the upstream's settings, as in the config
//! fit.routes.<r>.host=<a,b>       a route named <name>.<r>, matching these hosts;
//! fit.routes.<r>.path=<a,b>       ... paths, as in the config's match
//! fit.routes.<r>.path_regexp=<a,b>
//! fit.routes.<r>.method=<a,b>
//! fit.routes.<r>.expr=<expr>
//! fit.routes.<r>.priority=<n>
//! fit.routes.<r>.listeners=<a,b>
//! fit.routes.<r>.middleware=<JSON array>
//! fit.routes.<r>.handler=<JSON object>  proxy to the upstream when left out
//! fit.services.<s>.listeners=<a,b>      a layer 4 service named <name>.<s>
//! fit.services.<s>.port=<port>          to this port, through an upstream of its own
//! fit.services.<s>.server_names=<a,b>
//! fit.services.<s>.proxy_protocol=<v1|v2>
//! ```
//!
//! Other labels under the prefix are the caller's to read, or refused.

use std::collections::BTreeMap;

use akit_config::value::{self, Value};

use crate::kvpart::compact;

/// fit's `NamePattern`, what the config takes as a name: `^[A-Za-z0-9][A-Za-z0-9_.-]*$`.
#[must_use]
pub(crate) fn valid_name(s: &str) -> bool {
    let mut b = s.bytes();
    b.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && b.all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
}

/// What a route's or a service's key in a label looks like: a name without dots, which
/// separate a label's parts (`^[A-Za-z0-9][A-Za-z0-9_-]*$`).
fn valid_key(s: &str) -> bool {
    let mut b = s.bytes();
    b.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && b.all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
}

/// What labels are called, and what has them, in errors.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Kind {
    /// `label`, or `tag`.
    pub(crate) label: &'static str,
    /// `container`, or `service`.
    pub(crate) owner: &'static str,
}

/// What a source's labels say.
#[derive(Debug, Clone, Default)]
pub(crate) struct Set {
    /// 0 when the labels name none.
    pub(crate) port: u16,
    /// More of the upstream's settings, compact JSON by key.
    pub(crate) upstream: BTreeMap<String, String>,
    pub(crate) routes: BTreeMap<String, Route>,
    pub(crate) services: BTreeMap<String, Service>,
}

impl Set {
    /// The services with a port of their own (the Docker provider's, I58).
    #[must_use]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn ports(&self) -> usize {
        self.services.values().filter(|s| s.port != 0).count()
    }
}

/// A route the labels write.
#[derive(Debug, Clone, Default)]
pub(crate) struct Route {
    pub(crate) listeners: Vec<String>,
    pub(crate) priority: i64,
    pub(crate) host: Vec<String>,
    pub(crate) path: Vec<String>,
    pub(crate) path_regexp: Vec<String>,
    pub(crate) method: Vec<String>,
    pub(crate) expr: String,
    /// Compact JSON.
    pub(crate) middleware: Option<String>,
    /// Compact JSON.
    pub(crate) handler: Option<String>,
}

/// A layer 4 service the labels write.
#[derive(Debug, Clone, Default)]
pub(crate) struct Service {
    pub(crate) listeners: Vec<String>,
    pub(crate) server_names: Vec<String>,
    pub(crate) proxy_protocol: String,
    /// The port it sends to, through an upstream of its own; 0 when it sends to the upstream of
    /// its source.
    pub(crate) port: u16,
}

/// A JSON value as written in a label, `None` when it is not JSON.
fn json(s: &str) -> Option<Value> {
    value::parse(s).ok()
}

fn compact_of(v: &Value) -> String {
    let mut b = String::new();
    compact(v, &mut b);
    b
}

/// Reads the labels of `m` under `prefix`, which ends in `.`, leaving out those whose rest is
/// among `own`, the caller's.
pub(crate) fn parse(
    m: &BTreeMap<String, String>,
    prefix: &str,
    own: &[&str],
    kind: Kind,
) -> Result<Set, String> {
    let mut l = Set::default();
    for (key, value) in m {
        let Some(rest) = key.strip_prefix(prefix) else {
            continue;
        };
        let bad = |msg: String| format!("{} {key}: {msg}", kind.label);
        if own.contains(&rest) {
            continue;
        }
        if rest == "port" {
            l.port = parse_port(value).map_err(bad)?;
        } else if rest == "upstream" {
            let Some(Value::Object(members)) = json(value) else {
                return Err(bad(
                    "must be a JSON object of an upstream's settings".to_owned()
                ));
            };
            for (k, v) in &members {
                if k == "name" {
                    return Err(bad(format!(
                        "name is the {}'s: set {prefix}name",
                        kind.owner
                    )));
                }
                if k == "addresses" {
                    return Err(bad(format!("addresses are the {}s'", kind.owner)));
                }
                l.upstream.insert(k.clone(), compact_of(v));
            }
        } else if let Some(r) = rest.strip_prefix("routes.") {
            let (k, field) = r
                .split_once('.')
                .filter(|(k, _)| valid_key(k))
                .ok_or_else(|| {
                    bad(format!(
                        "is not {prefix}routes.<name>.<setting>, with a name of letters, digits, '_' and '-'"
                    ))
                })?;
            route_set(l.routes.entry(k.to_owned()).or_default(), field, value).map_err(bad)?;
        } else if let Some(r) = rest.strip_prefix("services.") {
            let (k, field) = r
                .split_once('.')
                .filter(|(k, _)| valid_key(k))
                .ok_or_else(|| {
                    bad(format!(
                        "is not {prefix}services.<name>.<setting>, with a name of letters, digits, '_' and '-'"
                    ))
                })?;
            service_set(l.services.entry(k.to_owned()).or_default(), field, value).map_err(bad)?;
        } else {
            return Err(bad(format!("is not a {} fit reads", kind.label)));
        }
    }
    for (k, s) in &l.services {
        if s.listeners.is_empty() {
            return Err(format!("service {k}: set {prefix}services.{k}.listeners"));
        }
    }
    Ok(l)
}

fn route_set(r: &mut Route, field: &str, value: &str) -> Result<(), String> {
    match field {
        "host" => r.host = list(value),
        "path" => r.path = list(value),
        "path_regexp" => r.path_regexp = list(value),
        "method" => r.method = list(value),
        "expr" => value.clone_into(&mut r.expr),
        "priority" => {
            r.priority = value
                .parse()
                .map_err(|_| format!("{value:?} is not a whole number"))?;
        }
        "listeners" => r.listeners = list(value),
        "middleware" => match json(value) {
            Some(v @ Value::Array(_)) => r.middleware = Some(compact_of(&v)),
            _ => return Err("must be a JSON array of middleware, as in the config".to_owned()),
        },
        "handler" => match json(value) {
            Some(v @ Value::Object(_)) => r.handler = Some(compact_of(&v)),
            _ => return Err("must be a JSON object of a handler, as in the config".to_owned()),
        },
        _ => return Err(format!("a route has no setting {field:?}")),
    }
    Ok(())
}

fn service_set(s: &mut Service, field: &str, value: &str) -> Result<(), String> {
    match field {
        "listeners" => s.listeners = list(value),
        "port" => s.port = parse_port(value)?,
        "server_names" => s.server_names = list(value),
        "proxy_protocol" => value.clone_into(&mut s.proxy_protocol),
        _ => return Err(format!("a service has no setting {field:?}")),
    }
    Ok(())
}

/// Reads a port, 1 to 65535.
pub(crate) fn parse_port(s: &str) -> Result<u16, String> {
    s.parse::<u16>()
        .ok()
        .filter(|p| *p != 0)
        .ok_or_else(|| format!("{s:?} is not a port"))
}

/// Splits a comma-separated value, leaving out empty entries.
fn list(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The sources of one name: one upstream, and the routes and services the first of them
/// writes.
#[derive(Debug, Clone, Default)]
pub(crate) struct Group {
    pub(crate) set: Set,
    pub(crate) addrs: Vec<String>,
    /// The addresses of the services with a port of their own, by key.
    pub(crate) svc_addrs: BTreeMap<String, Vec<String>>,
}

fn string(b: &mut String, s: &str) {
    value::write_string(b, s);
}

fn strings(b: &mut String, key: &str, list: &[String]) {
    if list.is_empty() {
        return;
    }
    b.push_str(",\"");
    b.push_str(key);
    b.push_str("\":[");
    for (i, s) in list.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        string(b, s);
    }
    b.push(']');
}

/// An upstream of `name`, sending to `addrs`, with the other settings `extra` gives: its keys
/// in order, as Go writes a map.
fn upstream(name: &str, addrs: &[String], extra: &BTreeMap<String, String>) -> String {
    let mut m: BTreeMap<&str, String> =
        extra.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    let mut a = String::from("[");
    for (i, s) in addrs.iter().enumerate() {
        if i > 0 {
            a.push(',');
        }
        string(&mut a, s);
    }
    a.push(']');
    m.insert("addresses", a);
    let mut n = String::new();
    string(&mut n, name);
    m.insert("name", n);
    let mut b = String::from("{");
    for (i, (k, v)) in m.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        string(&mut b, k);
        b.push(':');
        b.push_str(v);
    }
    b.push('}');
    b
}

fn route(name: &str, upstream: &str, r: &Route) -> String {
    let mut b = String::from("{\"name\":");
    string(&mut b, name);
    strings(&mut b, "listeners", &r.listeners);
    if r.priority != 0 {
        b.push_str(&format!(",\"priority\":{}", r.priority));
    }
    let matches = !r.host.is_empty()
        || !r.path.is_empty()
        || !r.path_regexp.is_empty()
        || !r.method.is_empty()
        || !r.expr.is_empty();
    if matches {
        let mut m = String::new();
        strings(&mut m, "host", &r.host);
        strings(&mut m, "path", &r.path);
        strings(&mut m, "path_regexp", &r.path_regexp);
        strings(&mut m, "method", &r.method);
        if !r.expr.is_empty() {
            m.push_str(",\"expr\":");
            string(&mut m, &r.expr);
        }
        b.push_str(",\"match\":{");
        b.push_str(m.get(1..).unwrap_or_default());
        b.push('}');
    }
    if let Some(mw) = &r.middleware {
        b.push_str(",\"middleware\":");
        b.push_str(mw);
    }
    b.push_str(",\"handler\":");
    match &r.handler {
        Some(h) => b.push_str(h),
        None => {
            b.push_str("{\"module\":\"proxy\",\"params\":{\"upstream\":");
            string(&mut b, upstream);
            b.push_str("}}");
        }
    }
    b.push('}');
    b
}

fn service(name: &str, upstream: &str, s: &Service) -> String {
    let mut b = String::from("{\"name\":");
    string(&mut b, name);
    b.push_str(",\"listeners\":[");
    for (i, l) in s.listeners.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        string(&mut b, l);
    }
    b.push_str("],\"upstream\":");
    string(&mut b, upstream);
    strings(&mut b, "server_names", &s.server_names);
    if !s.proxy_protocol.is_empty() {
        b.push_str(",\"proxy_protocol\":");
        string(&mut b, &s.proxy_protocol);
    }
    b.push('}');
    b
}

/// The part `groups` make, by name: an upstream of each, sending to its addresses, its routes,
/// proxying to it unless they name a handler, and its services, with upstreams of their own for
/// those with a port. The part is the same for the same groups, so that an unchanged one is
/// seen to be.
pub(crate) fn part(groups: &BTreeMap<String, Group>) -> Vec<u8> {
    let (mut ups, mut routes, mut services) = (Vec::new(), Vec::new(), Vec::new());
    for (name, g) in groups {
        ups.push(upstream(name, &g.addrs, &g.set.upstream));
        for (key, r) in &g.set.routes {
            routes.push(route(&format!("{name}.{key}"), name, r));
        }
        for (key, s) in &g.set.services {
            let sname = format!("{name}.{key}");
            let up = if s.port == 0 {
                name.clone()
            } else {
                let addrs = g.svc_addrs.get(key).map_or(&[][..], Vec::as_slice);
                ups.push(upstream(&sname, addrs, &BTreeMap::new()));
                sname.clone()
            };
            services.push(service(&sname, &up, s));
        }
    }
    let mut b = String::from("{");
    for (key, items) in [
        ("upstreams", &ups),
        ("routes", &routes),
        ("services", &services),
    ] {
        if items.is_empty() {
            continue;
        }
        if b.len() > 1 {
            b.push(',');
        }
        b.push('"');
        b.push_str(key);
        b.push_str("\":[");
        b.push_str(&items.join(","));
        b.push(']');
    }
    b.push('}');
    b.into_bytes()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use std::collections::BTreeMap;

    use akit_config::value::{self, Value};

    use super::{Group, Kind, parse, part};

    const KIND: Kind = Kind {
        label: "label",
        owner: "container",
    };

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn len(v: &Value, key: &str) -> usize {
        match v.get(key) {
            Some(Value::Array(a)) => a.len(),
            _ => 0,
        }
    }

    /// fit's `TestParse`.
    #[test]
    fn parses() {
        let l = parse(
            &labels(&[
                ("other", "not ours"),
                ("fit.enable", "true"),
                ("fit.port", "8080"),
                ("fit.routes.r.path_regexp", "^/a, ^/b"),
                ("fit.routes.r.method", "GET,POST"),
                ("fit.services.s.listeners", "tcp"),
                ("fit.services.s.server_names", "a.example, b.example"),
                ("fit.services.s.port", "5432"),
            ]),
            "fit.",
            &["enable"],
            KIND,
        )
        .unwrap();
        assert_eq!((l.port, l.ports()), (8080, 1));
        let r = &l.routes["r"];
        assert_eq!(r.path_regexp, ["^/a", "^/b"]);
        assert_eq!(r.method, ["GET", "POST"]);
        let s = &l.services["s"];
        assert_eq!(
            (s.server_names.as_slice(), s.port),
            (&["a.example".to_owned(), "b.example".to_owned()][..], 5432)
        );
        let mut groups = BTreeMap::new();
        groups.insert(
            "db".to_owned(),
            Group {
                set: l,
                addrs: vec!["10.0.0.1:8080".to_owned()],
                svc_addrs: [("s".to_owned(), vec!["10.0.0.1:5432".to_owned()])]
                    .into_iter()
                    .collect(),
            },
        );
        let p = String::from_utf8(part(&groups)).unwrap();
        let got = value::parse(&p).unwrap();
        assert_eq!(
            (
                len(&got, "upstreams"),
                len(&got, "routes"),
                len(&got, "services")
            ),
            (2, 1, 1),
            "{p}"
        );
        // A part providers take.
        crate::parse("p", p.as_bytes()).unwrap();
    }

    /// fit's `TestParseRefused`.
    #[test]
    fn refused() {
        for (l, want) in [
            (("fit.port", "0"), r#"label fit.port: "0" is not a port"#),
            (
                ("fit.port", "http"),
                r#"label fit.port: "http" is not a port"#,
            ),
            (
                ("fit.upstream", "[]"),
                "label fit.upstream: must be a JSON object of an upstream's settings",
            ),
            (
                ("fit.upstream", "null"),
                "label fit.upstream: must be a JSON object of an upstream's settings",
            ),
            (
                ("fit.routes.r", "x"),
                "label fit.routes.r: is not fit.routes.<name>.<setting>, with a name of letters, digits, '_' and '-'",
            ),
            (
                ("fit.routes._r.path", "/"),
                "label fit.routes._r.path: is not fit.routes.<name>.<setting>, with a name of letters, digits, '_' and '-'",
            ),
            (
                ("fit.services.s", "x"),
                "label fit.services.s: is not fit.services.<name>.<setting>, with a name of letters, digits, '_' and '-'",
            ),
            (
                ("fit.services.s.port", "70000"),
                r#"label fit.services.s.port: "70000" is not a port"#,
            ),
            (
                ("fit.services.s.colour", "red"),
                r#"label fit.services.s.colour: a service has no setting "colour""#,
            ),
            (
                ("fit.bogus", "x"),
                "label fit.bogus: is not a label fit reads",
            ),
            (
                ("fit.routes.r.priority", "high"),
                r#"label fit.routes.r.priority: "high" is not a whole number"#,
            ),
            (
                ("fit.routes.r.handler", "[]"),
                "label fit.routes.r.handler: must be a JSON object of a handler, as in the config",
            ),
            (
                ("fit.routes.r.handler", "null"),
                "label fit.routes.r.handler: must be a JSON object of a handler, as in the config",
            ),
        ] {
            let err = parse(&labels(&[l]), "fit.", &[], KIND).unwrap_err();
            assert_eq!(err, want, "{l:?}");
        }
        let err = parse(
            &labels(&[("fit.services.s.port", "5432")]),
            "fit.",
            &[],
            KIND,
        )
        .unwrap_err();
        assert_eq!(err, "service s: set fit.services.s.listeners");
    }
}
