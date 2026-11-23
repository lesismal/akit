//! Router v0 (I05), as it was before I11: a scan of the routes, kept here to compare v1 with.

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
