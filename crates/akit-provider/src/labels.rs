//! Routes, services and an upstream's settings read from key-value labels under a prefix, as the
//! Docker provider reads containers' labels (and the Consul provider, I61, services' tags), and
//! the part they make: fit's `provider/labels`.
//!
//! The labels, under a prefix (`akit.`, say):
//!
//! ```text
//! akit.port=<port>                the port; the source's own when left out
//! akit.upstream=<JSON object>      more of the upstream's settings, as in the config
//! akit.routes.<r>.host=<a,b>      a route named <name>.<r>, matching these hosts;
//! akit.routes.<r>.path=<a,b>      ... paths, as in the config's match
//! akit.routes.<r>.path_regexp=<a,b>
//! akit.routes.<r>.method=<a,b>
//! akit.routes.<r>.expr=<expr>
//! akit.routes.<r>.priority=<n>
//! akit.routes.<r>.listeners=<a,b>
//! akit.routes.<r>.middleware=<JSON array>
//! akit.routes.<r>.handler=<JSON object>  proxy to the upstream when left out
//! akit.services.<s>.listeners=<a,b>     a layer 4 service named <name>.<s>
//! akit.services.<s>.port=<port>         to this port, through an upstream of its own
//! akit.services.<s>.server_names=<a,b>
//! akit.services.<s>.proxy_protocol=<v1|v2>
//! ```
//!
//! Other labels under the prefix are the caller's to read, or refused. A label's JSON is read
//! by akit's config reader, under its nesting limit.

use std::collections::BTreeMap;

use akit_config::value::{self, Value};

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
    /// More of the upstream's settings, as in the config.
    pub(crate) upstream: Vec<(String, Value)>,
    pub(crate) routes: BTreeMap<String, Route>,
    pub(crate) services: BTreeMap<String, Service>,
}

impl Set {
    /// The services with a port of their own.
    pub(crate) fn ports(&self) -> usize {
        self.services.values().filter(|s| s.port != 0).count()
    }
}

/// A route the labels write.
#[derive(Debug, Clone, Default)]
pub(crate) struct Route {
    listeners: Vec<String>,
    priority: i64,
    host: Vec<String>,
    path: Vec<String>,
    path_regexp: Vec<String>,
    method: Vec<String>,
    expr: String,
    middleware: Option<Value>,
    handler: Option<Value>,
}

/// A layer 4 service the labels write.
#[derive(Debug, Clone, Default)]
pub(crate) struct Service {
    listeners: Vec<String>,
    server_names: Vec<String>,
    proxy_protocol: String,
    /// The port the service sends to, through an upstream of its own; 0 when it sends to the
    /// upstream of its source.
    pub(crate) port: u16,
}

/// fit's `NamePattern`: what the config takes as a name.
pub(crate) fn valid_name(s: &str) -> bool {
    let mut b = s.bytes();
    b.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && b.all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
}

/// fit's `keyPattern`: a route's or a service's key in a label, a name without dots.
fn valid_key(s: &str) -> bool {
    let mut b = s.bytes();
    b.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && b.all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
}

/// Reads the labels of `m` under `prefix`, which ends in '.', leaving out those whose rest is
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
            let refused = || bad("must be a JSON object of an upstream's settings".into());
            let Ok(Value::Object(ms)) = value::parse(value) else {
                return Err(refused());
            };
            if ms.iter().any(|(k, _)| k == "name") {
                return Err(bad(format!(
                    "name is the {}'s: set {prefix}name",
                    kind.owner
                )));
            }
            if ms.iter().any(|(k, _)| k == "addresses") {
                return Err(bad(format!("addresses are the {}s'", kind.owner)));
            }
            l.upstream = ms;
        } else if let Some(r) = rest.strip_prefix("routes.") {
            let (k, field) = r.split_once('.').filter(|(k, _)| valid_key(k)).ok_or_else(|| {
                bad(format!(
                    "is not {prefix}routes.<name>.<setting>, with a name of letters, digits, '_' and '-'"
                ))
            })?;
            l.routes
                .entry(k.to_owned())
                .or_default()
                .set(field, value)
                .map_err(bad)?;
        } else if let Some(r) = rest.strip_prefix("services.") {
            let (k, field) = r.split_once('.').filter(|(k, _)| valid_key(k)).ok_or_else(|| {
                bad(format!(
                    "is not {prefix}services.<name>.<setting>, with a name of letters, digits, '_' and '-'"
                ))
            })?;
            l.services
                .entry(k.to_owned())
                .or_default()
                .set(field, value)
                .map_err(bad)?;
        } else {
            return Err(bad(format!("is not a {} akit reads", kind.label)));
        }
    }
    if let Some(k) = l
        .services
        .iter()
        .find(|(_, s)| s.listeners.is_empty())
        .map(|(k, _)| k)
    {
        return Err(format!("service {k}: set {prefix}services.{k}.listeners"));
    }
    Ok(l)
}

impl Route {
    fn set(&mut self, field: &str, value: &str) -> Result<(), String> {
        match field {
            "host" => self.host = list(value),
            "path" => self.path = list(value),
            "path_regexp" => self.path_regexp = list(value),
            "method" => self.method = list(value),
            "expr" => value.clone_into(&mut self.expr),
            "priority" => {
                self.priority = value
                    .parse()
                    .map_err(|_| format!("{value:?} is not a whole number"))?;
            }
            "listeners" => self.listeners = list(value),
            "middleware" => match value::parse(value) {
                Ok(v @ Value::Array(_)) => self.middleware = Some(v),
                _ => return Err("must be a JSON array of middleware, as in the config".into()),
            },
            "handler" => match value::parse(value) {
                Ok(v @ Value::Object(_)) => self.handler = Some(v),
                _ => return Err("must be a JSON object of a handler, as in the config".into()),
            },
            _ => return Err(format!("a route has no setting {field:?}")),
        }
        Ok(())
    }

    fn has_match(&self) -> bool {
        !(self.host.is_empty()
            && self.path.is_empty()
            && self.path_regexp.is_empty()
            && self.method.is_empty()
            && self.expr.is_empty())
    }
}

impl Service {
    fn set(&mut self, field: &str, value: &str) -> Result<(), String> {
        match field {
            "listeners" => self.listeners = list(value),
            "port" => self.port = parse_port(value)?,
            "server_names" => self.server_names = list(value),
            "proxy_protocol" => value.clone_into(&mut self.proxy_protocol),
            _ => return Err(format!("a service has no setting {field:?}")),
        }
        Ok(())
    }
}

/// fit's `ParsePort`: a port, 1 to 65535.
pub(crate) fn parse_port(s: &str) -> Result<u16, String> {
    s.parse::<u16>()
        .ok()
        .filter(|p| *p != 0)
        .ok_or_else(|| format!("{s:?} is not a port"))
}

/// A comma-separated value, empty entries left out.
fn list(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The sources of one name: one upstream, and the routes and services the first of them writes.
#[derive(Debug)]
pub(crate) struct Group {
    pub(crate) set: Set,
    pub(crate) addrs: Vec<String>,
    /// The addresses of the services with a port of their own, by key.
    pub(crate) svc_addrs: BTreeMap<String, Vec<String>>,
}

/// The part `groups` make, by name: an upstream of each, sending to its addresses, its routes,
/// proxying to it unless they name a handler, and its services, with upstreams of their own for
/// those with a port. The part is the same for the same groups, so that an unchanged one is
/// seen to be.
pub(crate) fn part(groups: &BTreeMap<String, Group>) -> Vec<u8> {
    let mut upstreams = Vec::new();
    let mut routes = Vec::new();
    let mut services = Vec::new();
    for (name, g) in groups {
        upstreams.push(upstream(name, &g.addrs, &g.set.upstream));
        for (key, r) in &g.set.routes {
            routes.push(route(name, key, r));
        }
        for (key, s) in &g.set.services {
            let full = format!("{name}.{key}");
            let up = if s.port == 0 {
                name.clone()
            } else {
                let addrs = g.svc_addrs.get(key).map_or(&[][..], Vec::as_slice);
                upstreams.push(upstream(&full, addrs, &[]));
                full.clone()
            };
            let mut o = vec![
                ("name".to_owned(), Value::String(full)),
                ("listeners".to_owned(), strings(&s.listeners)),
                ("upstream".to_owned(), Value::String(up)),
            ];
            if !s.server_names.is_empty() {
                o.push(("server_names".to_owned(), strings(&s.server_names)));
            }
            if !s.proxy_protocol.is_empty() {
                o.push((
                    "proxy_protocol".to_owned(),
                    Value::String(s.proxy_protocol.clone()),
                ));
            }
            services.push(Value::Object(o));
        }
    }
    let mut top = Vec::new();
    for (k, v) in [
        ("upstreams", upstreams),
        ("routes", routes),
        ("services", services),
    ] {
        if !v.is_empty() {
            top.push((k.to_owned(), Value::Array(v)));
        }
    }
    let mut b = String::new();
    write_compact(&mut b, &Value::Object(top));
    b.into_bytes()
}

fn route(name: &str, key: &str, r: &Route) -> Value {
    let mut o = vec![("name".to_owned(), Value::String(format!("{name}.{key}")))];
    if !r.listeners.is_empty() {
        o.push(("listeners".to_owned(), strings(&r.listeners)));
    }
    if r.priority != 0 {
        o.push(("priority".to_owned(), Value::Number(r.priority.to_string())));
    }
    if r.has_match() {
        let mut m = Vec::new();
        for (k, v) in [
            ("host", &r.host),
            ("path", &r.path),
            ("path_regexp", &r.path_regexp),
            ("method", &r.method),
        ] {
            if !v.is_empty() {
                m.push((k.to_owned(), strings(v)));
            }
        }
        if !r.expr.is_empty() {
            m.push(("expr".to_owned(), Value::String(r.expr.clone())));
        }
        o.push(("match".to_owned(), Value::Object(m)));
    }
    if let Some(mw) = &r.middleware {
        o.push(("middleware".to_owned(), mw.clone()));
    }
    let handler = r.handler.clone().unwrap_or_else(|| {
        Value::Object(vec![
            ("module".to_owned(), Value::String("proxy".to_owned())),
            (
                "params".to_owned(),
                Value::Object(vec![(
                    "upstream".to_owned(),
                    Value::String(name.to_owned()),
                )]),
            ),
        ])
    });
    o.push(("handler".to_owned(), handler));
    Value::Object(o)
}

/// An upstream of `name`, sending to `addrs`, with the other settings `extra` gives; its members
/// sorted by name, as Go writes a map.
fn upstream(name: &str, addrs: &[String], extra: &[(String, Value)]) -> Value {
    let mut ms: BTreeMap<&str, Value> =
        extra.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    ms.insert("name", Value::String(name.to_owned()));
    ms.insert("addresses", strings(addrs));
    Value::Object(ms.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn strings(v: &[String]) -> Value {
    Value::Array(v.iter().cloned().map(Value::String).collect())
}

/// Writes `v` as compact JSON.
pub(crate) fn write_compact(b: &mut String, v: &Value) {
    match v {
        Value::Null => b.push_str("null"),
        Value::Bool(t) => b.push_str(if *t { "true" } else { "false" }),
        Value::Number(n) => b.push_str(n),
        Value::String(s) => value::write_string(b, s),
        Value::Array(a) => {
            b.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                write_compact(b, x);
            }
            b.push(']');
        }
        Value::Object(ms) => {
            b.push('{');
            for (i, (k, x)) in ms.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                value::write_string(b, k);
                b.push(':');
                write_compact(b, x);
            }
            b.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    //! fit's `provider/labels` tests: `TestParse` and `TestParseRefused`.
    #![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

    use std::collections::BTreeMap;

    use akit_config::value::{self, Value};

    use super::{Group, Kind, parse, part};

    const KIND: Kind = Kind {
        label: "label",
        owner: "container",
    };

    fn map(kv: &[(&str, &str)]) -> BTreeMap<String, String> {
        kv.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn parses() {
        let l = parse(
            &map(&[
                ("other", "not ours"),
                ("akit.enable", "true"),
                ("akit.port", "8080"),
                ("akit.routes.r.path_regexp", "^/a, ^/b"),
                ("akit.routes.r.method", "GET,POST"),
                ("akit.services.s.listeners", "tcp"),
                ("akit.services.s.server_names", "a.example, b.example"),
                ("akit.services.s.port", "5432"),
            ]),
            "akit.",
            &["enable"],
            KIND,
        )
        .unwrap();
        assert_eq!((l.port, l.ports()), (8080, 1));
        let r = &l.routes["r"];
        assert_eq!(r.path_regexp, ["^/a", "^/b"]);
        assert_eq!(r.method, ["GET", "POST"]);
        let s = &l.services["s"];
        assert_eq!(s.server_names, ["a.example", "b.example"]);
        assert_eq!(s.port, 5432);
        let groups = BTreeMap::from([(
            "db".to_owned(),
            Group {
                set: l,
                addrs: vec!["10.0.0.1:8080".into()],
                svc_addrs: BTreeMap::from([("s".to_owned(), vec!["10.0.0.1:5432".into()])]),
            },
        )]);
        let p = String::from_utf8(part(&groups)).unwrap();
        let v = value::parse(&p).unwrap();
        let len = |k: &str| match v.get(k) {
            Some(Value::Array(a)) => a.len(),
            _ => 0,
        };
        assert_eq!(
            (len("upstreams"), len("routes"), len("services")),
            (2, 1, 1),
            "{p}"
        );
    }

    #[test]
    fn refused() {
        for (labels, want) in [
            (("akit.port", "0"), r#"label akit.port: "0" is not a port"#),
            (
                ("akit.port", "http"),
                r#"label akit.port: "http" is not a port"#,
            ),
            (
                ("akit.upstream", "[]"),
                "label akit.upstream: must be a JSON object of an upstream's settings",
            ),
            (
                ("akit.upstream", "null"),
                "label akit.upstream: must be a JSON object of an upstream's settings",
            ),
            (
                ("akit.routes.r", "x"),
                "label akit.routes.r: is not akit.routes.<name>.<setting>, with a name of letters, digits, '_' and '-'",
            ),
            (
                ("akit.routes._r.path", "/"),
                "label akit.routes._r.path: is not akit.routes.<name>.<setting>, with a name of letters, digits, '_' and '-'",
            ),
            (
                ("akit.services.s", "x"),
                "label akit.services.s: is not akit.services.<name>.<setting>, with a name of letters, digits, '_' and '-'",
            ),
            (
                ("akit.services.s.port", "70000"),
                r#"label akit.services.s.port: "70000" is not a port"#,
            ),
            (
                ("akit.services.s.colour", "red"),
                r#"label akit.services.s.colour: a service has no setting "colour""#,
            ),
            (
                ("akit.bogus", "x"),
                "label akit.bogus: is not a label akit reads",
            ),
            (
                ("akit.routes.r.priority", "high"),
                r#"label akit.routes.r.priority: "high" is not a whole number"#,
            ),
            (
                ("akit.routes.r.handler", "[]"),
                "label akit.routes.r.handler: must be a JSON object of a handler, as in the config",
            ),
            (
                ("akit.routes.r.handler", "null"),
                "label akit.routes.r.handler: must be a JSON object of a handler, as in the config",
            ),
        ] {
            let err = parse(&map(&[labels]), "akit.", &[], KIND).unwrap_err();
            assert_eq!(err, want, "{labels:?}");
        }
    }
}
