//! Fuzz entry points (feature `fuzzing`; not a public API): the `fuzz/` cargo-fuzz project calls
//! them, and the tests below replay `fuzz/corpus/<target>` on stable. fit's `FuzzParseExpr`
//! (`router/expr_test.go`), the canonical path every request is routed by, and the router itself
//! against a reference that tries every route.
#![allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use std::net::IpAddr;

use crate::{Expr, MAX_DEEP, MAX_EXPR, MAX_HOST, Matcher, Parts, Route, Router, canonicalize};

/// `router_expr`: the expression parser never panics, and what it parses prints back as an
/// expression that parses to the same, which matches the same requests. Up to a NUL, the
/// expression; after it, a request (see [`request`]) to match it against.
pub fn router_expr(data: &[u8]) {
    let (src, rest) = match data.iter().position(|&b| b == 0) {
        Some(i) => (&data[..i], &data[i + 1..]),
        None => (data, &[][..]),
    };
    let Ok(s) = std::str::from_utf8(src) else {
        return;
    };
    let Ok(e) = Expr::parse(s) else {
        assert!(Matcher::expr(s).is_err());
        return;
    };
    let printed = e.to_string();
    if printed.len() <= MAX_EXPR {
        let again = Expr::parse(&printed)
            .unwrap_or_else(|err| panic!("{s:?} printed as {printed:?}, which fails: {err}"));
        assert_eq!(
            again.to_string(),
            printed,
            "{s:?} printed as {printed:?}, which prints back otherwise"
        );
        if let Some(r) = std::str::from_utf8(rest).ok().and_then(request) {
            let fields = r.fields();
            let parts = r.parts(&fields);
            assert_eq!(
                e.matches(&parts),
                again.matches(&parts),
                "{s:?} and {printed:?}"
            );
        }
    }
    let m = Matcher::expr(s).expect("an expression that parses builds");
    if let Some(r) = std::str::from_utf8(rest).ok().and_then(request) {
        let fields = r.fields();
        let parts = r.parts(&fields);
        assert_eq!(e.matches(&parts), m.matches(&parts));
    }
}

/// `router_path`: canonicalizing a request's path never panics, and a canonical path is a fixed
/// point: it has no dot or empty segment left, and is its own canonical form.
pub fn router_path(data: &[u8]) {
    let Ok(path) = std::str::from_utf8(data) else {
        return;
    };
    let Some(c) = canonicalize(path) else {
        return;
    };
    if !path.starts_with('/') {
        assert_eq!(c.escaped, path);
        return;
    }
    assert!(c.escaped.starts_with('/'), "{path:?}: {:?}", c.escaped);
    let segs: Vec<&str> = c.escaped.split('/').skip(1).collect();
    for (i, s) in segs.iter().enumerate() {
        assert!(
            *s != "." && *s != ".." && (!s.is_empty() || i + 1 == segs.len()),
            "{path:?}: {:?} keeps a dot or empty segment",
            c.escaped
        );
    }
    let again =
        canonicalize(&c.escaped).unwrap_or_else(|| panic!("{path:?}: {:?} is refused", c.escaped));
    assert!(!again.changed(), "{path:?}: {:?} changes again", c.escaped);
    assert_eq!(again.decoded, c.decoded);
    if !c.changed() {
        assert_eq!(c.escaped, path);
    }
}

/// A request read from `host|path|method|query|ip|name=value;name=value`.
struct Req {
    host: String,
    path: Vec<u8>,
    method: String,
    query: String,
    ip: Option<IpAddr>,
    headers: Vec<(String, String)>,
}

/// The [`Req`] in `line`, its path canonical as the listener makes it; `None` for one refused.
fn request(line: &str) -> Option<Req> {
    let mut f = line.split('|');
    let host = f.next().unwrap_or("").to_owned();
    let raw = f.next().unwrap_or("/");
    let path = canonicalize(raw)?.decoded.into_owned();
    let method = match f.next().unwrap_or("") {
        "" => "GET".to_owned(),
        m => m.to_owned(),
    };
    let query = f.next().unwrap_or("").to_owned();
    let ip = f.next().and_then(|s| s.parse().ok());
    let headers = f
        .next()
        .unwrap_or("")
        .split(';')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    Some(Req {
        host,
        path,
        method,
        query,
        ip,
        headers,
    })
}

impl Req {
    fn fields(&self) -> Vec<(&str, &str)> {
        self.headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }

    fn parts<'a>(&'a self, fields: &'a [(&'a str, &'a str)]) -> Parts<'a> {
        Parts {
            method: &self.method,
            host: &self.host,
            path: &self.path,
            query: &self.query,
            remote_ip: self.ip,
            headers: fields,
        }
    }
}

/// The host specificity of a pattern, normalized as [`Router::new`] files it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum HostKind {
    Any,
    Deep,
    Wild,
    Exact,
}

fn host_pattern(h: &str) -> (HostKind, String) {
    if h.is_empty() {
        return (HostKind::Any, String::new());
    }
    let h = h.trim_matches(|c| c == '[' || c == ']');
    let h = h.strip_suffix('.').unwrap_or(h).to_ascii_lowercase();
    if let Some(s) = h.strip_prefix("**") {
        (HostKind::Deep, s.to_owned())
    } else if let Some(s) = h.strip_prefix('*') {
        (HostKind::Wild, s.to_owned())
    } else {
        (HostKind::Exact, h)
    }
}

/// The deep keys a request for `host` (lower case, no port or trailing dot) is matched against:
/// its suffixes from a dot on, after at least one label, the longest first, of those `keys`
/// holds, at most [`MAX_DEEP`].
fn deep_keys<'k>(host: &str, keys: &'k [String]) -> Vec<&'k str> {
    let mut out = Vec::new();
    let Some(first) = host.find('.').filter(|&i| i > 0) else {
        return out;
    };
    for (i, _) in host.match_indices('.').filter(|(i, _)| *i >= first) {
        if out.len() == MAX_DEEP {
            break;
        }
        if let Some(k) = keys.iter().find(|k| **k == host[i..]) {
            out.push(k.as_str());
        }
    }
    out
}

/// `router_find`: the router picks the route a reference picks by trying every route: the
/// highest priority, then the most specific host (exact, `*.`, `**.`, none), then the most
/// specific path (exact, then the longest prefix), then the most matchers, then the first.
///
/// Lines: `R priority|host,host|path,path|expr<TAB>expr` add a route, `Q` + a request (see
/// [`request`]) asks for one.
pub fn router_find(data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let mut routes: Vec<Route<usize>> = Vec::new();
    let mut queries = Vec::new();
    for line in text.split('\n') {
        if let Some(r) = line.strip_prefix('R') {
            let mut f = r.split('|');
            let priority = f.next().unwrap_or("").trim().parse().unwrap_or(0);
            let list = |s: Option<&str>| -> Vec<String> {
                s.unwrap_or("")
                    .split(',')
                    .filter(|x| !x.is_empty())
                    .map(str::to_owned)
                    .collect()
            };
            let hosts = list(f.next());
            let paths = list(f.next());
            let matchers: Vec<Matcher> = f
                .next()
                .unwrap_or("")
                .split('\t')
                .filter(|x| !x.is_empty())
                .filter_map(|x| Matcher::expr(x).ok())
                .collect();
            if routes.len() < 64 {
                let value = routes.len();
                routes.push(Route {
                    hosts,
                    paths,
                    matchers,
                    priority,
                    value,
                });
            }
        } else if let Some(q) = line.strip_prefix('Q') {
            if let Some(r) = request(q) {
                queries.push(r);
            }
        }
    }
    let router = Router::new(routes.clone());
    assert_eq!(router.len(), routes.len());
    let deep: Vec<String> = routes
        .iter()
        .flat_map(|r| r.hosts.iter())
        .map(|h| host_pattern(h))
        .filter(|(k, _)| *k == HostKind::Deep)
        .map(|(_, s)| s)
        .collect();
    for q in &queries {
        let fields = q.fields();
        let parts = q.parts(&fields);
        let got = router.find_request(&parts).copied();
        let (want, matching, deeps) = reference(&routes, &deep, &parts);
        if deeps > 1 {
            // Two `**.` suffixes of the host have routes: fit tries the longer first and, past
            // a route it found there, no prefix of the shorter one's of the same priority,
            // though `better` would rank a longer one higher (an exact path still is). akit
            // does as fit; the reference only asks for a route that matches.
            assert_eq!(got.is_some(), want.is_some());
            if let Some(g) = got {
                assert!(matching.contains(&g), "{g} does not match");
            }
            continue;
        }
        assert_eq!(
            got,
            want,
            "host {:?} path {:?}: the router picked {got:?}, the reference {want:?}",
            q.host,
            String::from_utf8_lossy(&q.path)
        );
        if q.method == "GET" && q.query.is_empty() && q.ip.is_none() && q.headers.is_empty() {
            assert_eq!(router.find(&q.host, &q.path).copied(), got);
        }
    }
}

/// The route the reference picks for `r`, the routes that match it, and how many `**.` suffixes
/// of its host have routes.
fn reference(
    routes: &[Route<usize>],
    deep: &[String],
    r: &Parts<'_>,
) -> (Option<usize>, Vec<usize>, usize) {
    // The host as the router looks it up: without its port, brackets and trailing dot, in lower
    // case; a longer host than it lower-cases only matches routes with no hosts.
    let host = crate::host_only(r.host.as_bytes());
    let host = host.strip_suffix(b".").unwrap_or(host);
    let host = (host.len() <= MAX_HOST || !host.iter().any(u8::is_ascii_uppercase))
        .then(|| String::from_utf8_lossy(host).to_ascii_lowercase());
    let deep_keys = host
        .as_deref()
        .map_or_else(Vec::new, |h| deep_keys(h, deep));
    let mut best: Option<((i64, HostKind, usize, usize), usize)> = None;
    let mut matching = Vec::new();
    for (order, route) in routes.iter().enumerate() {
        if !route.matchers.iter().all(|m| m.matches(r)) {
            continue;
        }
        let all = [String::new()];
        let hosts = if route.hosts.is_empty() {
            &all[..]
        } else {
            &route.hosts[..]
        };
        let mut hspec = None;
        for h in hosts {
            let (kind, key) = host_pattern(h);
            let ok = match (kind, &host) {
                (HostKind::Any, _) => true,
                (_, None) => false,
                (HostKind::Exact, Some(host)) => *host == key,
                (HostKind::Wild, Some(host)) => host
                    .find('.')
                    .filter(|&i| i > 0)
                    .is_some_and(|i| host[i..] == key),
                (HostKind::Deep, Some(_)) => deep_keys.contains(&key.as_str()),
            };
            if ok {
                hspec = hspec.max(Some(kind));
            }
        }
        let Some(hspec) = hspec else { continue };
        let star = [String::from("*")];
        let paths = if route.paths.is_empty() {
            &star[..]
        } else {
            &route.paths[..]
        };
        let mut pspec = None;
        for p in paths {
            let spec = match p.strip_suffix('*') {
                Some(pre) => r.path.starts_with(pre.as_bytes()).then_some(pre.len()),
                None => (r.path == p.as_bytes()).then_some(usize::MAX),
            };
            pspec = pspec.max(spec);
        }
        let Some(pspec) = pspec else { continue };
        matching.push(route.value);
        let key = (route.priority, hspec, pspec, route.matchers.len());
        if best.is_none_or(|(b, _)| key > b) {
            best = Some((key, order));
        }
    }
    (
        best.map(|(_, order)| routes[order].value),
        matching,
        deep_keys.len(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `f` on every file of `fuzz/corpus/<target>`.
    fn replay(target: &str, f: fn(&[u8])) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fuzz/corpus")
            .join(target);
        let mut n = 0;
        for e in std::fs::read_dir(&dir).unwrap() {
            f(&std::fs::read(e.unwrap().path()).unwrap());
            n += 1;
        }
        assert!(n > 0, "{} is empty", dir.display());
    }

    #[test]
    fn corpus_router_expr() {
        replay("router_expr", router_expr);
    }

    #[test]
    fn corpus_router_path() {
        replay("router_path", router_path);
    }

    #[test]
    fn corpus_router_find() {
        replay("router_find", router_find);
    }
}
