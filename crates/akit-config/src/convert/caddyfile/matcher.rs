//! Routes' matches: a Caddyfile's named matchers converted, and matches intersected as the
//! blocks around a directive narrow it.

use std::collections::BTreeMap;

use super::parse::Directive;
use crate::convert::goregexp;
use crate::convert::gostd::{Fields, J, quote, quote_meta};
use crate::convert::nginx::{go_lower, go_upper};

/// A route's match, its fields in the order the native config has them. One with no field set
/// matches every request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct OutMatch {
    pub(crate) host: Vec<String>,
    pub(crate) path: Vec<String>,
    pub(crate) path_regexp: Vec<String>,
    pub(crate) method: Vec<String>,
    pub(crate) header: Option<BTreeMap<String, Vec<String>>>,
    pub(crate) query: Option<BTreeMap<String, Vec<String>>>,
    pub(crate) remote_ip: Vec<String>,
    pub(crate) expr: String,
}

fn values(v: Option<&BTreeMap<String, Vec<String>>>) -> Option<J> {
    v.filter(|m| !m.is_empty())
        .map(|m| J::Map(m.iter().map(|(k, v)| (k.clone(), J::strs(v))).collect()))
}

impl OutMatch {
    pub(crate) fn empty(&self) -> bool {
        self.host.is_empty()
            && self.path.is_empty()
            && self.path_regexp.is_empty()
            && self.method.is_empty()
            && self.header.as_ref().is_none_or(BTreeMap::is_empty)
            && self.query.as_ref().is_none_or(BTreeMap::is_empty)
            && self.remote_ip.is_empty()
            && self.expr.is_empty()
    }

    pub(crate) fn json(&self) -> J {
        Fields::new()
            .strs("host", &self.host)
            .strs("path", &self.path)
            .strs("path_regexp", &self.path_regexp)
            .strs("method", &self.method)
            .opt("header", values(self.header.as_ref()))
            .opt("query", values(self.query.as_ref()))
            .strs("remote_ip", &self.remote_ip)
            .str("expr", &self.expr)
            .done()
    }
}

/// Whether `a` and `b` match the same requests, as written.
pub(crate) fn same_match(a: &OutMatch, b: &OutMatch) -> bool {
    if a.empty() || b.empty() {
        return a.empty() == b.empty();
    }
    a.json().marshal() == b.json().marshal()
}

/// A match of the requests both `a` and `b` match, or `None` when none can match both. What
/// the native fields cannot hold of both is added to the match's expression.
pub(crate) fn intersect(a: Option<&OutMatch>, b: Option<&OutMatch>) -> Option<OutMatch> {
    let mut r = a.cloned().unwrap_or_default();
    let Some(b) = b.filter(|b| !b.empty()) else {
        return Some(r);
    };
    let mut extra = Vec::new();
    if !b.host.is_empty() {
        if r.host.is_empty() {
            r.host.clone_from(&b.host);
        } else if same_set(&r.host, &b.host) {
        } else if has_wildcard(&r.host) || has_wildcard(&b.host) {
            extra.push(call("Host", &b.host));
        } else {
            let both: Vec<String> = r
                .host
                .iter()
                .filter(|h| b.host.iter().any(|g| eq_fold(g, h)))
                .cloned()
                .collect();
            if both.is_empty() {
                return None;
            }
            r.host = both;
        }
    }
    if !b.path.is_empty() {
        if r.path.is_empty() {
            r.path.clone_from(&b.path);
        } else {
            let mut both = Vec::new();
            for p in &r.path {
                for q in &b.path {
                    if let Some(x) = intersect_path(p, q)
                        && !both.contains(&x)
                    {
                        both.push(x);
                    }
                }
            }
            if both.is_empty() {
                return None;
            }
            r.path = both;
        }
    }
    if !b.path_regexp.is_empty() {
        if r.path_regexp.is_empty() {
            r.path_regexp.clone_from(&b.path_regexp);
        } else if !same_set(&r.path_regexp, &b.path_regexp) {
            extra.push(call("PathRegexp", &b.path_regexp));
        }
    }
    if !b.method.is_empty() {
        if r.method.is_empty() {
            r.method.clone_from(&b.method);
        } else {
            let both: Vec<String> = r
                .method
                .iter()
                .filter(|m| b.method.contains(m))
                .cloned()
                .collect();
            if both.is_empty() {
                return None;
            }
            r.method = both;
        }
    }
    r.header = intersect_values(r.header.take(), b.header.as_ref(), true)?;
    r.query = intersect_values(r.query.take(), b.query.as_ref(), false)?;
    if !b.remote_ip.is_empty() {
        if r.remote_ip.is_empty() {
            r.remote_ip.clone_from(&b.remote_ip);
        } else if !same_set(&r.remote_ip, &b.remote_ip) {
            extra.push(call("RemoteIP", &b.remote_ip));
        }
    }
    if !b.expr.is_empty() && b.expr != r.expr {
        extra.push(b.expr.clone());
    }
    if !extra.is_empty() {
        let mut all = vec![r.expr.clone()];
        all.extend(extra);
        r.expr = and(&all);
    }
    Some(r)
}

/// The paths, exact or a prefix ending in `*`, that both `p` and `q` match, if any, as one.
pub(crate) fn intersect_path(p: &str, q: &str) -> Option<String> {
    match (p.strip_suffix('*'), q.strip_suffix('*')) {
        (None, None) => (p == q).then(|| p.to_owned()),
        (None, Some(qp)) => p.starts_with(qp).then(|| p.to_owned()),
        (Some(pp), None) => q.starts_with(pp).then(|| q.to_owned()),
        (Some(a), Some(b)) => {
            if a.starts_with(b) {
                Some(p.to_owned())
            } else if b.starts_with(a) {
                Some(q.to_owned())
            } else {
                None
            }
        }
    }
}

/// Go's `strings.EqualFold`, for the ASCII and the simple folds `to_lowercase` gives.
fn eq_fold(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.eq_ignore_ascii_case(b) || go_lower(a) == go_lower(b)
}

/// Intersects headers (with names that ignore case) or query parameters: a name both have
/// takes the values both allow, an empty list allowing any. `None` when none can match both.
#[allow(clippy::option_option)]
fn intersect_values(
    a: Option<BTreeMap<String, Vec<String>>>,
    b: Option<&BTreeMap<String, Vec<String>>>,
    fold: bool,
) -> Option<Option<BTreeMap<String, Vec<String>>>> {
    let Some(b) = b.filter(|b| !b.is_empty()) else {
        return Some(a);
    };
    let mut a = a.unwrap_or_default();
    for (k, vs) in b {
        let mut key = k.clone();
        if fold && let Some(ak) = a.keys().rfind(|ak| eq_fold(ak, k)) {
            key.clone_from(ak);
        }
        match a.get(&key) {
            None => {
                a.insert(key, vs.clone());
            }
            Some(have) if have.is_empty() => {
                a.insert(key, vs.clone());
            }
            Some(_) if vs.is_empty() => {}
            Some(have) => {
                let both: Vec<String> = have.iter().filter(|v| vs.contains(v)).cloned().collect();
                if both.is_empty() {
                    return None;
                }
                a.insert(key, both);
            }
        }
    }
    Some(Some(a))
}

pub(crate) fn same_set(a: &[String], b: &[String]) -> bool {
    a.len() == b.len() && a.iter().all(|x| b.contains(x))
}

fn has_wildcard(hosts: &[String]) -> bool {
    hosts.iter().any(|h| h.starts_with('*'))
}

/// A call of one of the expression's matchers.
pub(crate) fn call<S: AsRef<str>>(name: &str, args: &[S]) -> String {
    let q: Vec<String> = args.iter().map(|a| quote(a.as_ref())).collect();
    format!("{name}({})", q.join(", "))
}

/// Expressions joined with `&&`, those with `||` in parentheses.
pub(crate) fn and<S: AsRef<str>>(exprs: &[S]) -> String {
    let mut parts = Vec::new();
    for e in exprs {
        let e = e.as_ref();
        if e.is_empty() {
            continue;
        }
        if e.contains("||") && !(e.starts_with('(') && e.ends_with(')')) {
            parts.push(format!("({e})"));
        } else {
            parts.push(e.to_owned());
        }
    }
    parts.join(" && ")
}

/// `m` as an expression, or "" for a match of every request.
pub(crate) fn expr(m: &OutMatch) -> String {
    if m.empty() {
        return String::new();
    }
    let mut terms = Vec::new();
    if !m.host.is_empty() {
        terms.push(call("Host", &m.host));
    }
    if !m.path.is_empty() {
        let (mut exact, mut prefix) = (Vec::new(), Vec::new());
        for p in &m.path {
            match p.strip_suffix('*') {
                Some(s) => prefix.push(s.to_owned()),
                None => exact.push(p.clone()),
            }
        }
        if !exact.is_empty() && !prefix.is_empty() {
            terms.push(format!(
                "({} || {})",
                call("Path", &exact),
                call("PathPrefix", &prefix)
            ));
        } else if !exact.is_empty() {
            terms.push(call("Path", &exact));
        } else {
            terms.push(call("PathPrefix", &prefix));
        }
    }
    if !m.path_regexp.is_empty() {
        terms.push(call("PathRegexp", &m.path_regexp));
    }
    if !m.method.is_empty() {
        terms.push(call("Method", &m.method));
    }
    for (k, vs) in m.header.iter().flatten() {
        let mut a = vec![k.clone()];
        a.extend(vs.iter().cloned());
        terms.push(call("Header", &a));
    }
    for (k, vs) in m.query.iter().flatten() {
        let mut a = vec![k.clone()];
        a.extend(vs.iter().cloned());
        terms.push(call("Query", &a));
    }
    if !m.remote_ip.is_empty() {
        terms.push(call("RemoteIP", &m.remote_ip));
    }
    if !m.expr.is_empty() {
        terms.push(m.expr.clone());
    }
    and(&terms)
}

/// What Caddy's `private_ranges` stands for.
const PRIVATE_RANGES: &[&str] = &[
    "192.168.0.0/16",
    "172.16.0.0/12",
    "10.0.0.0/8",
    "127.0.0.1/8",
    "fd00::/8",
    "::1",
];

/// `args` with private_ranges replaced with the ranges it stands for.
pub(crate) fn ranges(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for a in args {
        if a == "private_ranges" {
            out.extend(PRIVATE_RANGES.iter().map(|s| (*s).to_owned()));
        } else {
            out.push(a.clone());
        }
    }
    out
}

/// Whether a path of Caddy's is one fit's path matcher takes as it is: exact, or a prefix
/// ending in `*`.
pub(crate) fn simple_path(p: &str) -> bool {
    p.starts_with('/') && !p.strip_suffix('*').unwrap_or(p).contains('*')
}

/// A path of Caddy's, whose `*` stands for anything, as a regular expression.
fn path_regexp(p: &str) -> String {
    let parts: Vec<String> = p.split('*').map(quote_meta).collect();
    format!("^{}$", parts.join(".*"))
}

/// Puts Caddy's paths in `m`: as paths when fit takes them all as they are, and as regular
/// expressions otherwise.
pub(crate) fn set_paths(m: &mut OutMatch, paths: &[String]) {
    if paths.is_empty() {
        return;
    }
    if paths.iter().all(|p| simple_path(p)) {
        m.path = paths.to_vec();
        return;
    }
    let res: Vec<String> = paths.iter().map(|p| path_regexp(p)).collect();
    if !m.path_regexp.is_empty() {
        m.expr = and(&[m.expr.clone(), call("PathRegexp", &res)]);
        return;
    }
    m.path_regexp = res;
}

/// The matchers a named matcher may use, those converted and those not.
const MATCHER_TYPES: &[&str] = &[
    "path",
    "path_regexp",
    "host",
    "method",
    "header",
    "query",
    "remote_ip",
    "client_ip",
    "not",
    "expression",
    "protocol",
    "file",
    "header_regexp",
    "vars",
    "vars_regexp",
    "tls",
];

/// Converts a named matcher's definition, `@name` with its matchers on its line or in its
/// block. A block after a matcher's name, as in `@name not { ... }`, is that matcher's. The
/// error says why it cannot.
pub(crate) fn named_matcher(d: &Directive) -> Result<OutMatch, String> {
    if !d.is_block || !d.args.is_empty() {
        let Some(first) = d.args.first() else {
            return Err("it has no matchers".into());
        };
        if !MATCHER_TYPES.contains(&first.as_str()) {
            return Err("a CEL expression is not converted".into());
        }
        let line = Directive {
            name: first.clone(),
            args: d.args.get(1..).unwrap_or_default().to_vec(),
            line: d.line,
            block: d.block.clone(),
            is_block: d.is_block,
        };
        return matcher_set(std::slice::from_ref(&line));
    }
    matcher_set(&d.block)
}

/// Converts the lines of a named matcher, all of which a request must match.
#[allow(clippy::too_many_lines)]
fn matcher_set(lines: &[Directive]) -> Result<OutMatch, String> {
    let mut m = OutMatch::default();
    let mut paths: Vec<String> = Vec::new();
    let mut exprs: Vec<String> = Vec::new();
    for l in lines {
        match l.name.as_str() {
            "path" => {
                if l.args.is_empty() {
                    return Err("path with no paths".into());
                }
                for p in &l.args {
                    if p.contains('%') || p.contains('{') {
                        return Err("a path with escapes or placeholders is not converted".into());
                    }
                    if !p.starts_with('/') && !p.starts_with('*') {
                        return Err(format!("path {p} starts with neither / nor *"));
                    }
                }
                paths.extend(l.args.iter().cloned());
            }
            "path_regexp" => {
                let re = match l.args.as_slice() {
                    [re] | [_, re] => re,
                    _ => return Err("path_regexp takes a name and a regular expression".into()),
                };
                if goregexp::check(re).is_err() {
                    return Err(format!(
                        "Go's regular expressions do not take {}",
                        quote(re)
                    ));
                }
                m.path_regexp.push(re.clone());
            }
            "host" => {
                if l.args.is_empty() {
                    return Err("host with no names".into());
                }
                for h in &l.args {
                    if h.contains('{')
                        || h.matches('*').count() > 1
                        || h.contains('*') && !h.starts_with("*.")
                    {
                        return Err(format!(
                            "host {h} is not converted: akit takes names and a leading \"*.\""
                        ));
                    }
                    m.host.push(go_lower(h));
                }
            }
            "method" => {
                if l.args.is_empty() {
                    return Err("method with no methods".into());
                }
                m.method.extend(l.args.iter().map(|x| go_upper(x)));
            }
            "header" => {
                let Some(name) = l.args.first() else {
                    return Err("header with no field".into());
                };
                if let Some(neg) = name.strip_prefix('!') {
                    exprs.push(format!("!{}", call("Header", &[neg])));
                    continue;
                }
                let mut vals = m
                    .header
                    .as_ref()
                    .and_then(|h| h.get(name))
                    .cloned()
                    .unwrap_or_default();
                for v in l.args.iter().skip(1) {
                    if v == "*" {
                        continue;
                    }
                    if v.contains(['*', '{']) {
                        return Err(
                            "a header value with wildcards or placeholders is not converted".into(),
                        );
                    }
                    vals.push(v.clone());
                }
                m.header
                    .get_or_insert_with(BTreeMap::new)
                    .insert(name.clone(), vals);
            }
            "query" => {
                if l.args.is_empty() {
                    return Err("query with no parameters".into());
                }
                for a in &l.args {
                    let (k, v) = a.split_once('=').unwrap_or((a, ""));
                    if k.is_empty() || v.contains('{') {
                        return Err(format!("query {a} is not converted"));
                    }
                    let q = m
                        .query
                        .get_or_insert_with(BTreeMap::new)
                        .entry(k.to_owned())
                        .or_default();
                    if v != "*" && !v.is_empty() {
                        q.push(v.to_owned());
                    }
                }
            }
            "remote_ip" | "client_ip" => {
                if l.args.first().is_some_and(|a| a == "forwarded") {
                    return Err(
                        "remote_ip forwarded is not converted: akit matches the connection's address"
                            .into(),
                    );
                }
                if l.args.is_empty() {
                    return Err(format!("{} with no ranges", l.name));
                }
                m.remote_ip.extend(ranges(&l.args));
            }
            "not" => {
                let sub = if l.is_block {
                    l.block.clone()
                } else {
                    let Some(first) = l.args.first() else {
                        return Err("not with no matcher".into());
                    };
                    vec![Directive {
                        name: first.clone(),
                        args: l.args.get(1..).unwrap_or_default().to_vec(),
                        line: l.line,
                        ..Directive::default()
                    }]
                };
                let n = matcher_set(&sub)?;
                let e = expr(&n);
                if e.is_empty() {
                    return Err("not with no matcher".into());
                }
                exprs.push(format!("!({e})"));
            }
            "expression" => return Err("CEL expressions are not converted".into()),
            other => return Err(format!("the {other} matcher is not converted")),
        }
    }
    set_paths(&mut m, &paths);
    if !exprs.is_empty() {
        let mut all = vec![m.expr.clone()];
        all.extend(exprs);
        m.expr = and(&all);
    }
    Ok(m)
}
