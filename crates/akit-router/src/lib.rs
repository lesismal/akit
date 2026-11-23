#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's routers: they pick the route that serves a request.
//!
//! [`Router`] is v1 (I11): fit's `router` package without its matchers (I39), its hosts and paths
//! looked up in maps rather than scanned, and [`canonicalize`] puts a request's path in the
//! canonical form the routes are matched against, as fit's listener does.
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
//! the longest suffix first, then no hosts); the one whose path matched most specifically (exact,
//! then the longest prefix, then no paths); the one written first. As fit's `router` package.
//!
//! A lookup does not allocate: the host's patterns are found by up to six map lookups (its exact
//! name, its `*.` suffix, up to four `**.` suffixes), and under each the path's by one map lookup
//! for the exact path and one per distinct prefix length, the longest first, stopping as soon as
//! no prefix left can win. A map of a few keys is scanned rather than hashed; a router whose
//! routes have no hosts does not look at the host.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

mod path;

pub use path::{Canonical, canonicalize};

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

/// The `**.` patterns one host is matched against, the longest suffixes first (fit's `maxDeep`).
const MAX_DEEP: usize = 4;

/// The longest host lower-cased for a lookup: a DNS name is at most 253 bytes, so a longer host
/// matches only routes with no hosts (fit's `maxHost`).
const MAX_HOST: usize = 255;

/// The path specificity of an exact path; a prefix's is its length.
const PATH_EXACT: usize = usize::MAX;

/// A fast hash for the router's maps (FxHash's mixing, eight bytes at a time). They are built
/// from the config and only read afterwards, so a request's host or path, whatever it hashes to,
/// probes no more than the entries the config put there: no need for `SipHash`'s random keys.
#[derive(Default, Clone, Copy)]
struct Fx(u64);

impl Fx {
    #[inline]
    fn add(&mut self, w: u64) {
        self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

impl Hasher for Fx {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            let mut w = [0u8; 8];
            w.copy_from_slice(c);
            self.add(u64::from_le_bytes(w));
        }
        let mut w = [0u8; 8];
        for (d, s) in w.iter_mut().zip(chunks.remainder()) {
            *d = *s;
        }
        self.add(u64::from_le_bytes(w));
    }

    #[inline]
    fn write_usize(&mut self, n: usize) {
        self.add(n as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

type Map<V> = HashMap<Box<[u8]>, V, BuildHasherDefault<Fx>>;

/// Up to this many keys, a [`Table`] is scanned rather than hashed: comparing a few lengths, and
/// the bytes of a key of the same length, costs less than hashing.
const FEW: usize = 8;

/// A map by bytes, read-only: a scan of a few keys, a hash map past that.
#[derive(Debug, Clone)]
enum Table<V> {
    Few(Box<[(Box<[u8]>, V)]>),
    Many(Map<V>),
}

impl<V> Default for Table<V> {
    fn default() -> Table<V> {
        Table::Few(Box::default())
    }
}

impl<V> Table<V> {
    fn freeze(m: Map<V>) -> Table<V> {
        if m.len() <= FEW {
            Table::Few(m.into_iter().collect())
        } else {
            Table::Many(m)
        }
    }

    #[inline]
    fn get(&self, key: &[u8]) -> Option<&V> {
        match self {
            Table::Few(kv) => kv.iter().find(|(k, _)| **k == *key).map(|(_, v)| v),
            Table::Many(m) => m.get(key),
        }
    }

    #[inline]
    fn is_empty(&self) -> bool {
        match self {
            Table::Few(kv) => kv.is_empty(),
            Table::Many(m) => m.is_empty(),
        }
    }
}

/// The best route for one host pattern and one path pattern: with no matchers yet (I39), the one
/// that beats every other there always wins, so only it is kept.
#[derive(Debug, Clone, Copy)]
struct Entry {
    priority: i64,
    /// Path specificity: [`PATH_EXACT`] or the prefix's length.
    path: usize,
    /// The route's position in the routes given to [`Router::new`].
    order: usize,
}

impl Entry {
    /// Whether `self`, matched with host specificity `sh`, beats `b`, matched with `bh`.
    #[inline]
    fn beats(&self, sh: u8, b: &Entry, bh: u8) -> bool {
        (self.priority, sh, self.path) > (b.priority, bh, b.path)
            || (self.priority, sh, self.path) == (b.priority, bh, b.path) && self.order < b.order
    }
}

/// A [`Node`] being built.
#[derive(Default)]
struct NodeBuilder {
    exact: Map<Entry>,
    prefix: Map<Entry>,
}

/// The routes one host pattern leads to.
#[derive(Debug, Clone, Default)]
struct Node {
    exact: Table<Entry>,
    /// By prefix, without the `*`.
    prefix: Table<Entry>,
    /// The lengths of the prefixes, longest first.
    lens: Box<[usize]>,
    /// The highest priority of any prefix entry.
    max_prefix: i64,
}

impl NodeBuilder {
    fn add(&mut self, pattern: &str, priority: i64, order: usize) {
        let (map, key, path) = match pattern.strip_suffix('*') {
            Some(p) => (&mut self.prefix, p, p.len()),
            None => (&mut self.exact, pattern, PATH_EXACT),
        };
        let e = Entry {
            priority,
            path,
            order,
        };
        map.entry(key.as_bytes().into())
            .and_modify(|old| {
                if e.beats(HOST_ANY, old, HOST_ANY) {
                    *old = e;
                }
            })
            .or_insert(e);
    }

    fn finish(self) -> Node {
        let mut lens: Vec<usize> = self.prefix.keys().map(|k| k.len()).collect();
        lens.sort_unstable_by(|a, b| b.cmp(a));
        lens.dedup();
        let max_prefix = self
            .prefix
            .values()
            .map(|e| e.priority)
            .max()
            .unwrap_or(i64::MIN);
        Node {
            exact: Table::freeze(self.exact),
            prefix: Table::freeze(self.prefix),
            lens: lens.into(),
            max_prefix,
        }
    }
}

impl Node {
    /// The node's entry for `path`, matched with host specificity `h`, if it beats `best`.
    #[inline]
    fn find(&self, path: &[u8], h: u8, best: Option<(&Entry, u8)>) -> Option<&Entry> {
        let mut found: Option<&Entry> = None;
        // Makes `e` what was found if it beats it, or, with nothing found yet, `best`.
        let try_entry = |e: Option<&'_ Entry>, found: Option<&'_ Entry>| -> bool {
            let Some(e) = e else { return false };
            match (found, best) {
                (Some(f), _) => e.beats(h, f, h),
                (None, Some((b, bh))) => e.beats(h, b, bh),
                (None, None) => true,
            }
        };
        if !self.exact.is_empty() {
            let e = self.exact.get(path);
            if try_entry(e, found) {
                found = e;
            }
        }
        for &l in &*self.lens {
            let top = found.map(|f| f.priority).or(best.map(|(b, _)| b.priority));
            if top.is_some_and(|p| p >= self.max_prefix) {
                // No prefix left can beat it: the ones still to try are shorter, and this host is
                // no more specific than the one best matched.
                break;
            }
            let e = path.get(..l).and_then(|p| self.prefix.get(p));
            if try_entry(e, found) {
                found = e;
            }
        }
        found
    }
}

/// Picks the route for a request; built once, when the config is compiled, and only read
/// afterwards.
#[derive(Debug, Clone)]
pub struct Router<T> {
    values: Vec<T>,
    /// By lower-case host.
    exact: Table<Node>,
    /// `*.` patterns, by lower-case suffix from the dot on: `.example.com`.
    wild: Table<Node>,
    /// `**.` patterns, likewise.
    deep: Table<Node>,
    /// Routes with no hosts.
    any: Option<Node>,
}

/// A [`Router`] being built.
#[derive(Default)]
struct Builder {
    exact: Map<NodeBuilder>,
    wild: Map<NodeBuilder>,
    deep: Map<NodeBuilder>,
    any: Option<NodeBuilder>,
}

impl Builder {
    /// The node for host pattern `h`, made if needed; `""` is the node of routes with no hosts.
    fn node_for(&mut self, h: &str) -> &mut NodeBuilder {
        if h.is_empty() {
            return self.any.get_or_insert_with(NodeBuilder::default);
        }
        let h = h.trim_matches(|c| c == '[' || c == ']');
        let h = h.strip_suffix('.').unwrap_or(h).to_ascii_lowercase();
        let (map, key) = if let Some(s) = h.strip_prefix("**") {
            (&mut self.deep, s)
        } else if let Some(s) = h.strip_prefix('*') {
            (&mut self.wild, s)
        } else {
            (&mut self.exact, h.as_str())
        };
        map.entry(key.as_bytes().into()).or_default()
    }
}

/// Builds the nodes of `m` and freezes it.
fn freeze(m: Map<NodeBuilder>) -> Table<Node> {
    Table::freeze(m.into_iter().map(|(k, n)| (k, n.finish())).collect())
}

impl<T> Router<T> {
    /// A router over `routes`; routes that tie on priority and specificity are picked in the
    /// order given.
    #[must_use]
    pub fn new(routes: Vec<Route<T>>) -> Router<T> {
        let mut b = Builder::default();
        let mut values = Vec::with_capacity(routes.len());
        for (order, route) in routes.into_iter().enumerate() {
            let all = [String::new()];
            let hosts: &[String] = if route.hosts.is_empty() {
                &all
            } else {
                &route.hosts
            };
            for h in hosts {
                let node = b.node_for(h);
                if route.paths.is_empty() {
                    node.add("*", route.priority, order);
                }
                for p in &route.paths {
                    node.add(p, route.priority, order);
                }
            }
            values.push(route.value);
        }
        Router {
            values,
            exact: freeze(b.exact),
            wild: freeze(b.wild),
            deep: freeze(b.deep),
            any: b.any.map(NodeBuilder::finish),
        }
    }

    /// How many routes it has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether it has no routes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// The value of the route for a request to `host` (a `Host` field's value: a port, the
    /// brackets of an IPv6 address, a trailing dot and case are ignored) and `path` (the
    /// canonical, percent-decoded path: [`Canonical::decoded`]), or `None` when no route matches.
    /// Does not allocate.
    #[must_use]
    pub fn find(&self, host: &str, path: impl AsRef<[u8]>) -> Option<&T> {
        let path = path.as_ref();
        let mut best: Option<(&Entry, u8)> = None;
        if self.exact.is_empty() && self.wild.is_empty() && self.deep.is_empty() {
            // No route has hosts: the host is not looked at.
            try_node(self.any.as_ref(), path, HOST_ANY, &mut best);
            return best.and_then(|(e, _)| self.values.get(e.order));
        }
        let host = host_only(host.as_bytes());
        let host = host.strip_suffix(b".").unwrap_or(host);

        let mut buf = [0u8; MAX_HOST];
        let host: Option<&[u8]> = if !host.iter().any(u8::is_ascii_uppercase) {
            Some(host)
        } else if let Some(b) = buf.get_mut(..host.len()) {
            b.copy_from_slice(host);
            b.make_ascii_lowercase();
            Some(b)
        } else {
            None
        };

        // The most specific host first.
        if let Some(host) = host {
            if !self.exact.is_empty() {
                try_node(self.exact.get(host), path, HOST_EXACT, &mut best);
            }
            if let Some(i) = host.iter().position(|&c| c == b'.').filter(|&i| i > 0) {
                if !self.wild.is_empty() {
                    try_node(
                        host.get(i..).and_then(|s| self.wild.get(s)),
                        path,
                        HOST_WILD,
                        &mut best,
                    );
                }
                if !self.deep.is_empty() {
                    // Every suffix from a dot on, the longest first, after at least one label.
                    let mut deep: [Option<&Node>; MAX_DEEP] = [None; MAX_DEEP];
                    let mut nd = 0;
                    let mut at = Some(i);
                    while let Some(i) = at {
                        if nd == MAX_DEEP {
                            break;
                        }
                        if let Some(n) = host.get(i..).and_then(|s| self.deep.get(s)) {
                            if let Some(slot) = deep.get_mut(nd) {
                                *slot = Some(n);
                            }
                            nd = nd.saturating_add(1);
                        }
                        at = host
                            .get(i.saturating_add(1)..)
                            .and_then(|s| s.iter().position(|&c| c == b'.'))
                            .map(|j| i.saturating_add(1).saturating_add(j));
                    }
                    for n in deep.into_iter().flatten() {
                        try_node(Some(n), path, HOST_DEEP, &mut best);
                    }
                }
            }
        }
        try_node(self.any.as_ref(), path, HOST_ANY, &mut best);
        best.and_then(|(e, _)| self.values.get(e.order))
    }
}

/// Makes `n`'s entry for `path`, matched with host specificity `h`, the best, if it beats it.
#[inline]
fn try_node<'a>(n: Option<&'a Node>, path: &[u8], h: u8, best: &mut Option<(&'a Entry, u8)>) {
    if let Some(e) = n.and_then(|n| n.find(path, h, *best)) {
        *best = Some((e, h));
    }
}

/// A `Host` field's host: without its port, and without the brackets of an IPv6 address.
fn host_only(host: &[u8]) -> &[u8] {
    if let Some(rest) = host.strip_prefix(b"[") {
        return match rest.iter().position(|&c| c == b']') {
            Some(end) => rest.get(..end).unwrap_or(host),
            None => host,
        };
    }
    let mut colons = host.iter().enumerate().filter(|(_, c)| **c == b':');
    match (colons.next(), colons.next()) {
        (Some((i, _)), None) => host.get(..i).unwrap_or(host),
        _ => host,
    }
}

#[cfg(test)]
mod tests;
