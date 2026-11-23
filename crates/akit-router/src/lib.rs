#![forbid(unsafe_code)]
//! akit's routers: they pick the route that serves a request.
//!
//! [`Router`] is v0 (I05): fit's host and path rules and its order, by a scan of the routes.
//! I11 replaces the scan with lookups by host and path, ports fit's router table tests, and adds
//! path canonicalization.
//!
//! A route matches a request when one of its hosts matches the request's host and one of its
//! paths matches the request's path; a route with no hosts matches every host, one with no paths
//! every path. Hosts are exact (`example.com`), a `*.` wildcard standing for exactly one label
//! (`*.example.com`), or a `**.` wildcard standing for one label or more (`**.example.com`); they
//! ignore case. Paths are exact (`/hello`) or end in `*` to match a prefix (`/api/*`); they do not
//! ignore case.
//!
//! When several routes match, the one that serves the request is, in order: the one with the
//! highest priority; the one whose host matched most specifically (exact, then `*.`, then `**.`,
//! then no hosts); the one whose path matched most specifically (exact, then the longest prefix,
//! then no paths); the one written first. As fit's `router` package.

/// A route as the router sees it.
#[derive(Debug, Clone)]
pub struct Route<T> {
    /// Host patterns, checked when the config was read; empty matches every host.
    pub hosts: Vec<String>,
    /// Path patterns; empty matches every path.
    pub paths: Vec<String>,
    /// Higher first.
    pub priority: i64,
    /// What [`Router::find`] returns for the route.
    pub value: T,
}

/// Host specificity, least to most specific.
const HOST_ANY: u8 = 0;
const HOST_DEEP: u8 = 1;
const HOST_WILD: u8 = 2;
const HOST_EXACT: u8 = 3;

/// A host pattern, lower-cased.
#[derive(Debug, Clone)]
enum Host {
    Exact(String),
    /// `*.` and the suffix from its dot on: `.example.com`.
    Wild(String),
    /// `**.` and the suffix from its dot on.
    Deep(String),
}

/// A path pattern.
#[derive(Debug, Clone)]
enum Path {
    Exact(String),
    /// Without the `*`.
    Prefix(String),
}

#[derive(Debug, Clone)]
struct Entry<T> {
    hosts: Vec<Host>,
    paths: Vec<Path>,
    priority: i64,
    value: T,
}

/// Picks the route for a request; built once, when the config is compiled, and only read
/// afterwards.
#[derive(Debug, Clone)]
pub struct Router<T> {
    entries: Vec<Entry<T>>,
}

impl<T> Router<T> {
    /// A router over `routes`; routes that tie on priority and specificity are picked in the
    /// order given.
    #[must_use]
    pub fn new(routes: Vec<Route<T>>) -> Router<T> {
        let entries = routes
            .into_iter()
            .map(|r| Entry {
                hosts: r.hosts.iter().map(|h| host_pattern(h)).collect(),
                paths: r.paths.iter().map(|p| path_pattern(p)).collect(),
                priority: r.priority,
                value: r.value,
            })
            .collect();
        Router { entries }
    }

    /// How many routes it has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether it has no routes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The value of the route for a request to `host` (a `Host` field's value: a port, the
    /// brackets of an IPv6 address, a trailing dot and case are ignored) and `path`, or `None`
    /// when no route matches. Does not allocate.
    #[must_use]
    pub fn find(&self, host: &str, path: &str) -> Option<&T> {
        let host = host_only(host);
        let host = host.strip_suffix('.').unwrap_or(host);
        let mut best: Option<(&Entry<T>, u8, usize)> = None;
        for e in &self.entries {
            let Some(h) = e.host_match(host) else {
                continue;
            };
            let Some(p) = e.path_match(path) else {
                continue;
            };
            // Earlier entries win ties: replace only when strictly better.
            let better = best.is_none_or(|(b, bh, bp)| (e.priority, h, p) > (b.priority, bh, bp));
            if better {
                best = Some((e, h, p));
            }
        }
        best.map(|(e, _, _)| &e.value)
    }
}

impl<T> Entry<T> {
    fn host_match(&self, host: &str) -> Option<u8> {
        if self.hosts.is_empty() {
            return Some(HOST_ANY);
        }
        self.hosts
            .iter()
            .filter_map(|h| match h {
                Host::Exact(x) => host.eq_ignore_ascii_case(x).then_some(HOST_EXACT),
                Host::Wild(sfx) => {
                    // Exactly one label before the suffix.
                    let label = strip_suffix_ci(host, sfx)?;
                    (!label.is_empty() && !label.contains('.')).then_some(HOST_WILD)
                }
                Host::Deep(sfx) => {
                    let labels = strip_suffix_ci(host, sfx)?;
                    (!labels.is_empty() && !labels.starts_with('.') && !labels.ends_with('.'))
                        .then_some(HOST_DEEP)
                }
            })
            .max()
    }

    fn path_match(&self, path: &str) -> Option<usize> {
        if self.paths.is_empty() {
            return Some(0);
        }
        self.paths
            .iter()
            .filter_map(|p| match p {
                Path::Exact(x) => (path == x).then_some(usize::MAX),
                Path::Prefix(x) => path.starts_with(x.as_str()).then_some(x.len()),
            })
            .max()
    }
}

fn host_pattern(h: &str) -> Host {
    let h = h.to_ascii_lowercase();
    if let Some(rest) = h.strip_prefix("**") {
        Host::Deep(rest.to_owned())
    } else if let Some(rest) = h.strip_prefix('*') {
        Host::Wild(rest.to_owned())
    } else {
        Host::Exact(h.strip_suffix('.').unwrap_or(&h).to_owned())
    }
}

fn path_pattern(p: &str) -> Path {
    match p.strip_suffix('*') {
        Some(prefix) => Path::Prefix(prefix.to_owned()),
        None => Path::Exact(p.to_owned()),
    }
}

/// `host` without `suffix` (lower-case), ignoring case, if it ends in it.
fn strip_suffix_ci<'a>(host: &'a str, suffix: &str) -> Option<&'a str> {
    let cut = host.len().checked_sub(suffix.len())?;
    let (head, tail) = (host.get(..cut)?, host.get(cut..)?);
    tail.eq_ignore_ascii_case(suffix).then_some(head)
}

/// A `Host` field's host: without its port, and without the brackets of an IPv6 address.
fn host_only(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split_once(']').map_or(host, |(h, _)| h);
    }
    match (host.find(':'), host.rfind(':')) {
        (Some(i), Some(j)) if i == j => host.get(..i).unwrap_or(host),
        _ => host,
    }
}

#[cfg(test)]
mod tests {
    use super::{Route, Router};

    fn route(
        hosts: &[&str],
        paths: &[&str],
        priority: i64,
        value: &'static str,
    ) -> Route<&'static str> {
        Route {
            hosts: hosts.iter().map(|s| (*s).to_owned()).collect(),
            paths: paths.iter().map(|s| (*s).to_owned()).collect(),
            priority,
            value,
        }
    }

    #[test]
    fn fits_hello_example() {
        let r = Router::new(vec![
            route(&[], &["/hello"], 0, "hello"),
            route(&[], &[], -1, "not-found"),
        ]);
        assert_eq!(r.find("127.0.0.1:8080", "/hello"), Some(&"hello"));
        assert_eq!(r.find("x", "/hello/"), Some(&"not-found"));
        assert_eq!(r.find("x", "/"), Some(&"not-found"));
    }

    #[test]
    fn nothing_matches() {
        let r = Router::new(vec![route(&["a.example"], &["/x"], 0, "x")]);
        assert_eq!(r.find("a.example", "/y"), None);
        assert_eq!(r.find("b.example", "/x"), None);
        assert!(Router::<u8>::new(vec![]).find("a", "/").is_none());
    }

    #[test]
    fn hosts() {
        let r = Router::new(vec![
            route(&["**.example.com"], &[], 0, "deep"),
            route(&["*.example.com"], &[], 0, "wild"),
            route(&["www.example.com"], &[], 0, "exact"),
            route(&[], &[], 0, "any"),
        ]);
        assert_eq!(r.find("WWW.Example.COM.", "/"), Some(&"exact"));
        assert_eq!(r.find("www.example.com:8080", "/"), Some(&"exact"));
        assert_eq!(r.find("api.example.com", "/"), Some(&"wild"));
        assert_eq!(r.find("a.b.example.com", "/"), Some(&"deep"));
        assert_eq!(r.find("example.com", "/"), Some(&"any"));
        assert_eq!(r.find("xexample.com", "/"), Some(&"any"));
        assert_eq!(r.find("[::1]:8080", "/"), Some(&"any"));
    }

    #[test]
    fn ipv6_hosts() {
        let r = Router::new(vec![route(&["::1"], &[], 0, "v6")]);
        assert_eq!(r.find("[::1]:8080", "/"), Some(&"v6"));
        assert_eq!(r.find("[::1]", "/"), Some(&"v6"));
        assert_eq!(r.find("::1", "/"), Some(&"v6"));
    }

    #[test]
    fn paths() {
        let r = Router::new(vec![
            route(&[], &["/api/*"], 0, "api"),
            route(&[], &["/api/v1/*"], 0, "v1"),
            route(&[], &["/api/v1/users"], 0, "users"),
            route(&[], &[], 0, "any"),
        ]);
        assert_eq!(r.find("h", "/api/v1/users"), Some(&"users"));
        assert_eq!(r.find("h", "/api/v1/x"), Some(&"v1"));
        assert_eq!(r.find("h", "/api/x"), Some(&"api"));
        assert_eq!(r.find("h", "/API/x"), Some(&"any"));
    }

    #[test]
    fn priority_then_host_then_path_then_order() {
        let r = Router::new(vec![
            route(&[], &["/a"], 0, "path"),
            route(&["h"], &[], 0, "host"),
            route(&[], &[], 1, "priority"),
        ]);
        assert_eq!(r.find("h", "/a"), Some(&"priority"));
        let r = Router::new(vec![
            route(&[], &["/a"], 0, "path"),
            route(&["h"], &[], 0, "host"),
        ]);
        assert_eq!(r.find("h", "/a"), Some(&"host"));
        let r = Router::new(vec![
            route(&[], &["/a"], 0, "first"),
            route(&[], &["/a"], 0, "second"),
        ]);
        assert_eq!(r.find("h", "/a"), Some(&"first"));
    }
}
