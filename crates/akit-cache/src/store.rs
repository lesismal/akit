//! The memory tier, as fit's `middleware/cache/store.go`: responses by key, each key with its
//! variants, bounded in size, the least recently used key evicted first. A store is split into
//! [`SHARDS`] parts, each with its lock, its share of the size and its own order, so that requests
//! for different keys rarely meet on a lock, and none is held for more than a lookup or an
//! insertion. Stores are named (zones) and shared by the caches of one zone, across reloads.

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, SystemTime};

use akio_http::Headers;

use crate::semantics::vary_value;

/// How many parts a store is split into.
pub(crate) const SHARDS: usize = 16;

/// The most variants kept for one key; past it the oldest goes.
pub(crate) const MAX_VARIANTS: usize = 8;

/// What an entry costs beyond its body, fields and key, counted against the store's size.
const ENTRY_OVERHEAD: u64 = 256;

/// A stored response. It does not change once stored: one freshened by a 304 is stored as a new
/// entry in its place.
#[derive(Debug)]
pub(crate) struct Entry {
    pub(crate) status: u16,
    /// Its fields, without those [`crate::semantics::unstored`] names.
    pub(crate) headers: Headers,
    pub(crate) body: Arc<[u8]>,
    /// The request fields it varies on (lower case), with their values in the request it
    /// answered.
    pub(crate) vary: Vec<(String, String)>,
    /// When it arrived, its age then (RFC 9111 section 4.2.3), and how long it is fresh for.
    pub(crate) response_time: SystemTime,
    pub(crate) initial_age: Duration,
    pub(crate) lifetime: Duration,
    /// It must not be answered stale: `must-revalidate`, `proxy-revalidate` or `s-maxage`.
    pub(crate) no_stale: bool,
    pub(crate) etag: Option<String>,
    pub(crate) last_modified: Option<String>,
    /// What it costs in the store, set when stored.
    pub(crate) size: u64,
}

impl Entry {
    /// Its age at `now`.
    pub(crate) fn age(&self, now: SystemTime) -> Duration {
        let since = now
            .duration_since(self.response_time)
            .unwrap_or(Duration::ZERO);
        self.initial_age.saturating_add(since)
    }

    /// Whether it answers a request with fields `h`: the fields it varies on have the values
    /// they had.
    pub(crate) fn matches(&self, h: &Headers) -> bool {
        self.vary
            .iter()
            .all(|(name, value)| vary_value(h, name) == *value)
    }

    /// Whether it answers the same requests as `o`.
    fn same_variant(&self, o: &Entry) -> bool {
        self.vary == o.vary
    }

    /// What it costs in the store under `key`.
    fn cost(&self, key: &str) -> u64 {
        let mut n = ENTRY_OVERHEAD
            .saturating_add(key.len() as u64)
            .saturating_add(self.body.len() as u64);
        for (k, v) in self.headers.iter() {
            n = n
                .saturating_add(k.len() as u64)
                .saturating_add(v.len() as u64)
                .saturating_add(16);
        }
        for (k, v) in &self.vary {
            n = n
                .saturating_add(k.len() as u64)
                .saturating_add(v.len() as u64);
        }
        n
    }
}

/// No node: the end of the order.
const NIL: usize = usize::MAX;

/// A key's variants, in a shard's order.
#[derive(Debug)]
struct Node {
    key: Arc<str>,
    variants: Vec<Arc<Entry>>,
    size: u64,
    /// Toward the most recently used, and toward the least.
    prev: usize,
    next: usize,
}

/// One part of a store.
#[derive(Debug)]
struct Shard {
    index: HashMap<Arc<str>, usize>,
    /// The nodes, by index; a free one is `None`, its index in `free`.
    nodes: Vec<Option<Node>>,
    free: Vec<usize>,
    /// The most and the least recently used.
    head: usize,
    tail: usize,
    size: u64,
}

impl Shard {
    fn new() -> Shard {
        Shard {
            index: HashMap::new(),
            nodes: Vec::new(),
            free: Vec::new(),
            head: NIL,
            tail: NIL,
            size: 0,
        }
    }

    fn node(&mut self, i: usize) -> Option<&mut Node> {
        self.nodes.get_mut(i).and_then(Option::as_mut)
    }

    fn set_prev(&mut self, i: usize, prev: usize) {
        if let Some(n) = self.node(i) {
            n.prev = prev;
        }
    }

    fn set_next(&mut self, i: usize, next: usize) {
        if let Some(n) = self.node(i) {
            n.next = next;
        }
    }

    /// Takes node `i` out of the order.
    fn unlink(&mut self, i: usize) {
        let Some((prev, next)) = self.node(i).map(|n| (n.prev, n.next)) else {
            return;
        };
        if prev == NIL {
            self.head = next;
        } else {
            self.set_next(prev, next);
        }
        if next == NIL {
            self.tail = prev;
        } else {
            self.set_prev(next, prev);
        }
    }

    /// Puts node `i` first in the order.
    fn link_first(&mut self, i: usize) {
        let head = self.head;
        if let Some(n) = self.node(i) {
            n.prev = NIL;
            n.next = head;
        }
        if head == NIL {
            self.tail = i;
        } else {
            self.set_prev(head, i);
        }
        self.head = i;
    }

    /// Marks node `i` used.
    fn touch(&mut self, i: usize) {
        if self.head != i {
            self.unlink(i);
            self.link_first(i);
        }
    }

    /// The node of `key`, made (first in the order) if there is none.
    fn node_of(&mut self, key: &str) -> usize {
        if let Some(&i) = self.index.get(key) {
            self.touch(i);
            return i;
        }
        let key: Arc<str> = Arc::from(key);
        let node = Node {
            key: Arc::clone(&key),
            variants: Vec::new(),
            size: 0,
            prev: NIL,
            next: NIL,
        };
        let i = match self.free.pop() {
            Some(i) => {
                if let Some(slot) = self.nodes.get_mut(i) {
                    *slot = Some(node);
                }
                i
            }
            None => {
                self.nodes.push(Some(node));
                self.nodes.len().saturating_sub(1)
            }
        };
        self.index.insert(key, i);
        self.link_first(i);
        i
    }

    /// Drops node `i` and everything it holds.
    fn drop_node(&mut self, i: usize) {
        self.unlink(i);
        if let Some(n) = self.nodes.get_mut(i).and_then(Option::take) {
            self.index.remove(&n.key);
            self.size = self.size.saturating_sub(n.size);
            self.free.push(i);
        }
    }

    fn grow(&mut self, i: usize, add: u64, sub: u64) {
        if let Some(n) = self.node(i) {
            n.size = n.size.saturating_add(add).saturating_sub(sub);
        }
        self.size = self.size.saturating_add(add).saturating_sub(sub);
    }
}

/// The memory tier of one zone.
#[derive(Debug)]
pub(crate) struct Store {
    /// Its zone, for debugging.
    #[allow(dead_code, reason = "read through Debug")]
    name: String,
    hasher: RandomState,
    max_bytes: AtomicU64,
    /// What responses being stored hold so far, bounded by a quarter of `max_bytes`: past it, a
    /// response goes without being kept.
    pub(crate) capturing: AtomicU64,
    shards: [Mutex<Shard>; SHARDS],
}

impl Store {
    pub(crate) fn new(name: &str, max_bytes: u64) -> Store {
        Store {
            name: name.to_owned(),
            hasher: RandomState::new(),
            max_bytes: AtomicU64::new(max_bytes),
            capturing: AtomicU64::new(0),
            shards: std::array::from_fn(|_| Mutex::new(Shard::new())),
        }
    }

    pub(crate) fn max_bytes(&self) -> u64 {
        self.max_bytes.load(Ordering::Relaxed)
    }

    /// A shard's share of the size.
    fn shard_max(&self) -> u64 {
        self.max_bytes().checked_div(SHARDS as u64).unwrap_or(0)
    }

    fn shard(&self, key: &str) -> MutexGuard<'_, Shard> {
        let h = self.hasher.hash_one(key);
        // The remainder is below SHARDS: the index is in range.
        let i = usize::try_from(h.checked_rem(SHARDS as u64).unwrap_or(0)).unwrap_or(0);
        let m = self.shards.get(i).unwrap_or(&self.shards[0]);
        m.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The entry under `key` that answers a request with fields `h`, if any, and whether
    /// anything is stored under `key`; marks the key used.
    pub(crate) fn get(&self, key: &str, h: &Headers) -> (Option<Arc<Entry>>, bool) {
        let mut sh = self.shard(key);
        let Some(&i) = sh.index.get(key) else {
            return (None, false);
        };
        sh.touch(i);
        let found = sh
            .node(i)
            .and_then(|n| n.variants.iter().rev().find(|e| e.matches(h)).cloned());
        (found, true)
    }

    /// Stores `e` under `key`, in place of the variant it answers for, and evicts the least
    /// recently used keys past the shard's size. An entry larger than a shard is not kept.
    pub(crate) fn put(&self, key: &str, mut e: Entry) {
        e.size = e.cost(key);
        let limit = self.shard_max();
        if e.size > limit {
            return;
        }
        let e = Arc::new(e);
        let mut sh = self.shard(key);
        let i = sh.node_of(key);
        let mut freed = 0u64;
        if let Some(n) = sh.node(i) {
            if let Some(at) = n.variants.iter().position(|v| v.same_variant(&e)) {
                freed = n.variants.remove(at).size;
            } else if n.variants.len() >= MAX_VARIANTS && !n.variants.is_empty() {
                freed = n.variants.remove(0).size;
            }
            n.variants.push(Arc::clone(&e));
        }
        sh.grow(i, e.size, freed);
        while sh.size > limit && sh.tail != NIL {
            let tail = sh.tail;
            sh.drop_node(tail);
        }
    }

    /// Stores `e` in place of `old`, if `old` is still stored under `key`: a response freshened
    /// by a 304 does not bring back one removed meanwhile. Returns the entry stored.
    pub(crate) fn replace(&self, key: &str, old: &Arc<Entry>, mut e: Entry) -> Arc<Entry> {
        e.size = e.cost(key);
        let e = Arc::new(e);
        let mut sh = self.shard(key);
        let Some(&i) = sh.index.get(key) else {
            return e;
        };
        let mut swapped = None;
        if let Some(n) = sh.node(i) {
            if let Some(slot) = n.variants.iter_mut().find(|v| Arc::ptr_eq(v, old)) {
                swapped = Some(std::mem::replace(slot, Arc::clone(&e)).size);
            }
        }
        if let Some(freed) = swapped {
            sh.grow(i, e.size, freed);
            let limit = self.shard_max();
            while sh.size > limit && sh.tail != NIL {
                let tail = sh.tail;
                sh.drop_node(tail);
            }
        }
        e
    }

    /// Drops every variant under `key`; reports whether there was any.
    pub(crate) fn remove(&self, key: &str) -> bool {
        let mut sh = self.shard(key);
        match sh.index.get(key) {
            Some(&i) => {
                sh.drop_node(i);
                true
            }
            None => false,
        }
    }

    /// How many keys the store holds, and its size.
    pub(crate) fn stats(&self) -> (usize, u64) {
        let mut keys = 0usize;
        let mut size = 0u64;
        for m in &self.shards {
            let sh = m.lock().unwrap_or_else(PoisonError::into_inner);
            keys = keys.saturating_add(sh.index.len());
            size = size.saturating_add(sh.size);
        }
        (keys, size)
    }

    /// How many variants `key` has.
    #[cfg(test)]
    fn variants(&self, key: &str) -> usize {
        let mut sh = self.shard(key);
        match sh.index.get(key).copied() {
            Some(i) => sh.node(i).map_or(0, |n| n.variants.len()),
            None => 0,
        }
    }
}

/// The stores in use, by zone, so that a reloaded config keeps what its caches hold: the new
/// config's caches take a store before the old config's let it go. Weak, so that a zone no cache
/// uses goes. Locked only when a config is compiled.
static ZONES: Mutex<Vec<(String, Weak<Store>)>> = Mutex::new(Vec::new());

/// How many keys zone `name` holds, its size, and what responses being stored hold; `None`
/// when no cache uses it.
pub(crate) fn zone_stats(name: &str) -> Option<(usize, u64, u64)> {
    let s = ZONES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|(n, _)| n == name)
        .and_then(|(_, w)| w.upgrade())?;
    let (keys, size) = s.stats();
    Some((keys, size, s.capturing.load(Ordering::Acquire)))
}

/// The store of zone `name`, made if no cache uses it, sized `max_bytes` (the size of the last
/// cache of the zone compiled).
pub(crate) fn acquire(name: &str, max_bytes: u64) -> Arc<Store> {
    let mut zones = ZONES.lock().unwrap_or_else(PoisonError::into_inner);
    zones.retain(|(_, w)| w.strong_count() > 0);
    if let Some(s) = zones
        .iter()
        .find(|(n, _)| n == name)
        .and_then(|(_, w)| w.upgrade())
    {
        s.max_bytes.store(max_bytes, Ordering::Relaxed);
        return s;
    }
    let s = Arc::new(Store::new(name, max_bytes));
    zones.push((name.to_owned(), Arc::downgrade(&s)));
    s
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;

    fn entry(body: usize) -> Entry {
        Entry {
            status: 200,
            headers: Headers::new(),
            body: vec![0u8; body].into(),
            vary: Vec::new(),
            response_time: SystemTime::now(),
            initial_age: Duration::ZERO,
            lifetime: Duration::from_secs(60),
            no_stale: false,
            etag: None,
            last_modified: None,
            size: 0,
        }
    }

    // fit's TestEviction.
    #[test]
    fn eviction() {
        let s = Store::new("evict", 16 * 4096);
        let none = Headers::new();
        for i in 0..1000 {
            s.put(&format!("/k{i}"), entry(1024));
            if i == 0 {
                continue;
            }
            assert!(s.get("/k0", &none).0.is_some(), "/k0 was evicted after {i}");
        }
        let (keys, size) = s.stats();
        assert!(
            size <= 16 * 4096 && (16..=64).contains(&keys),
            "{keys} keys, {size} bytes"
        );
        assert!(s.remove("/k0"));
        assert_eq!(s.get("/k0", &none).0.map(|_| ()), None);
        assert!(!s.get("/k0", &none).1);
        s.put("/big", entry(8192));
        assert!(
            s.get("/big", &none).0.is_none(),
            "an entry larger than a shard was kept"
        );
    }

    // fit's TestVariantsBounded.
    #[test]
    fn variants_bounded() {
        let s = Store::new("variants", 1 << 20);
        for i in 0..20 {
            let mut e = entry(0);
            e.vary = vec![("x".into(), i.to_string())];
            s.put("/v", e);
        }
        assert_eq!(s.variants("/v"), MAX_VARIANTS);
        let mut h = Headers::new();
        h.set("X", "19");
        assert!(s.get("/v", &h).0.is_some(), "the newest variant went");
        h.set("X", "0");
        assert!(s.get("/v", &h).0.is_none(), "the oldest variant stayed");
        // A variant stored again replaces its old self.
        let mut e = entry(10);
        e.vary = vec![("x".into(), "19".into())];
        s.put("/v", e);
        assert_eq!(s.variants("/v"), MAX_VARIANTS);
    }

    #[test]
    fn replace_only_what_is_still_there() {
        let s = Store::new("replace", 1 << 20);
        let none = Headers::new();
        s.put("/r", entry(1));
        let old = s.get("/r", &none).0.unwrap();
        let new = s.replace("/r", &old, entry(2));
        assert!(Arc::ptr_eq(&s.get("/r", &none).0.unwrap(), &new));
        s.remove("/r");
        s.replace("/r", &new, entry(3));
        assert!(
            s.get("/r", &none).0.is_none(),
            "a removed response came back"
        );
        let (keys, size) = s.stats();
        assert_eq!((keys, size), (0, 0));
    }

    // fit's TestZones: a zone outlives a reload, and goes when no cache uses it.
    #[test]
    fn zones() {
        let none = Headers::new();
        let old = acquire("reload", 1 << 20);
        old.put("/k", entry(1));
        let next = acquire("reload", 2 << 20);
        drop(old);
        assert!(
            next.get("/k", &none).0.is_some(),
            "the zone did not outlive the reload"
        );
        assert_eq!(next.max_bytes(), 2 << 20);
        let weak = Arc::downgrade(&next);
        drop(next);
        assert!(weak.upgrade().is_none());
        let again = acquire("reload", 1 << 20);
        assert!(
            again.get("/k", &none).0.is_none(),
            "a zone no cache uses was kept"
        );
    }
}
