//! What a `vod` store keeps of what it builds (fit's `handler/vod/cache.go`): plans, media
//! playlists, initialization sections, segments and WebVTT, bounded in bytes, the least recently
//! used dropped first; each built once for every request that waits for it.
//!
//! Unlike fit's, a build holds no bytes of the cache while it runs: builds are bounded by the
//! library's file pool, each by `max_segment_bytes`; what is built is admitted when it is done,
//! and one larger than the whole cache is handed to those waiting and not kept.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use crate::library::Error;

/// The most requests that may wait for one build; one more is answered busy.
pub(crate) const MAX_WAITERS: usize = 1024;

/// A file of a library, as the catalog has it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct FileKey {
    pub(crate) abs: PathBuf,
    pub(crate) size: u64,
    pub(crate) modified: SystemTime,
}

impl FileKey {
    pub(crate) fn of(f: &crate::library::File) -> FileKey {
        FileKey {
            abs: f.abs.clone(),
            size: f.size,
            modified: f.modified,
        }
    }
}

/// What of a file is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum What {
    Plan,
    List(super::names::C),
    Init,
    Segment(super::names::C, u32),
    Vtt,
}

/// A key of the cache.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    pub(crate) file: FileKey,
    pub(crate) what: What,
}

pub(crate) type Waiter<V> = Box<dyn FnOnce(Result<V, Error>) + Send>;

struct Entry<V> {
    id: u64,
    value: Option<V>,
    bytes: u64,
    waiters: Vec<Waiter<V>>,
    tick: u64,
}

struct State<V> {
    max: u64,
    used: u64,
    m: HashMap<Key, Entry<V>>,
    lru: BTreeMap<u64, Key>,
    tick: u64,
    next_id: u64,
    orphans: HashMap<u64, Vec<Waiter<V>>>,
    hits: u64,
    misses: u64,
}

/// What a cache holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CacheStats {
    /// Entries kept.
    pub entries: usize,
    /// Their bytes.
    pub bytes: u64,
    /// The bound.
    pub max: u64,
    /// Lookups that found their entry.
    pub hits: u64,
    /// Lookups that did not.
    pub misses: u64,
}

/// The cache.
pub(crate) struct Cache<V: Clone> {
    st: Mutex<State<V>>,
}

/// A build begun: its ID, to end it with [`Cache::done`].
pub(crate) struct Build(pub(crate) u64);

impl<V: Clone> Cache<V> {
    pub(crate) fn new(max: u64) -> Cache<V> {
        Cache {
            st: Mutex::new(State {
                max,
                used: 0,
                m: HashMap::new(),
                lru: BTreeMap::new(),
                tick: 0,
                next_id: 0,
                orphans: HashMap::new(),
                hits: 0,
                misses: 0,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State<V>> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn stats(&self) -> CacheStats {
        let st = self.lock();
        CacheStats {
            entries: st.lru.len(),
            bytes: st.used,
            max: st.max,
            hits: st.hits,
            misses: st.misses,
        }
    }

    /// The value kept of `k`, marked used.
    pub(crate) fn peek(&self, k: &Key) -> Option<V> {
        let mut st = self.lock();
        st.tick = st.tick.saturating_add(1);
        let tick = st.tick;
        let e = st.m.get_mut(k)?;
        let v = e.value.clone()?;
        let old = std::mem::replace(&mut e.tick, tick);
        st.lru.remove(&old);
        st.lru.insert(tick, k.clone());
        st.hits = st.hits.saturating_add(1);
        Some(v)
    }

    /// Looks for `k`: its value, with `w` given back to call; `w` queued behind a build under
    /// way; refused; or a build for the caller to run, `w` its first waiter.
    pub(crate) fn begin(&self, k: &Key, w: Waiter<V>) -> Begin<V> {
        let mut st = self.lock();
        st.tick = st.tick.saturating_add(1);
        let tick = st.tick;
        if let Some(e) = st.m.get_mut(k) {
            if let Some(v) = e.value.clone() {
                let old = std::mem::replace(&mut e.tick, tick);
                st.lru.remove(&old);
                st.lru.insert(tick, k.clone());
                st.hits = st.hits.saturating_add(1);
                return Begin::Kept(v, w);
            }
            if e.waiters.len() >= MAX_WAITERS {
                st.misses = st.misses.saturating_add(1);
                return Begin::Refused(w);
            }
            e.waiters.push(w);
            st.misses = st.misses.saturating_add(1);
            return Begin::Waiting;
        }
        st.misses = st.misses.saturating_add(1);
        st.next_id = st.next_id.saturating_add(1);
        let id = st.next_id;
        st.m.insert(
            k.clone(),
            Entry {
                id,
                value: None,
                bytes: 0,
                waiters: vec![w],
                tick: 0,
            },
        );
        Begin::Build(Build(id))
    }

    /// Ends build `b` of `k` with `r`, of `bytes` bytes, and calls those that wait. A value
    /// larger than the cache, or a build dropped meanwhile, is handed over and not kept.
    pub(crate) fn done(&self, k: &Key, b: &Build, r: Result<V, Error>, bytes: u64) {
        let mut st = self.lock();
        let ours = st.m.get(k).is_some_and(|e| e.id == b.0);
        let waiters = if !ours {
            st.orphans.remove(&b.0).unwrap_or_default()
        } else {
            let e = st.m.remove(k);
            let waiters = e.map(|e| e.waiters).unwrap_or_default();
            if let Ok(v) = &r {
                if bytes <= st.max {
                    while st.used.saturating_add(bytes) > st.max {
                        let Some((_, old)) = st.lru.pop_first() else {
                            break;
                        };
                        if let Some(e) = st.m.remove(&old) {
                            st.used = st.used.saturating_sub(e.bytes);
                        }
                    }
                    st.tick = st.tick.saturating_add(1);
                    let tick = st.tick;
                    st.m.insert(
                        k.clone(),
                        Entry {
                            id: b.0,
                            value: Some(v.clone()),
                            bytes,
                            waiters: Vec::new(),
                            tick,
                        },
                    );
                    st.lru.insert(tick, k.clone());
                    st.used = st.used.saturating_add(bytes);
                }
            }
            waiters
        };
        drop(st);
        for w in waiters {
            w(r.clone());
        }
    }

    /// Keeps `v` as `k`'s value, built at once.
    pub(crate) fn put(&self, k: &Key, v: V, bytes: u64) {
        let mut st = self.lock();
        if st.m.get(k).is_some_and(|e| e.value.is_none()) || bytes > st.max {
            return;
        }
        st.next_id = st.next_id.saturating_add(1);
        let id = st.next_id;
        drop(st);
        self.done_new(k, id, v, bytes);
    }

    fn done_new(&self, k: &Key, id: u64, v: V, bytes: u64) {
        let mut st = self.lock();
        if let Some(e) = st.m.remove(k) {
            st.used = st.used.saturating_sub(e.bytes);
            st.lru.remove(&e.tick);
        }
        st.m.insert(
            k.clone(),
            Entry {
                id,
                value: None,
                bytes: 0,
                waiters: Vec::new(),
                tick: 0,
            },
        );
        drop(st);
        self.done(k, &Build(id), Ok(v), bytes);
    }

    fn forget(st: &mut State<V>, k: &Key) {
        if let Some(e) = st.m.remove(k) {
            st.used = st.used.saturating_sub(e.bytes);
            if e.value.is_some() {
                st.lru.remove(&e.tick);
            } else {
                st.orphans.insert(e.id, e.waiters);
            }
        }
    }

    /// Drops what is kept of `f`.
    pub(crate) fn drop_file(&self, f: &FileKey) {
        let mut st = self.lock();
        let keys: Vec<Key> = st.m.keys().filter(|k| k.file == *f).cloned().collect();
        for k in keys {
            Cache::forget(&mut st, &k);
        }
    }

    /// Drops what is kept of the files `keep` does not hold.
    pub(crate) fn keep(&self, keep: &HashSet<FileKey>) {
        let mut st = self.lock();
        let keys: Vec<Key> =
            st.m.keys()
                .filter(|k| !keep.contains(&k.file))
                .cloned()
                .collect();
        for k in keys {
            Cache::forget(&mut st, &k);
        }
    }
}

/// What [`Cache::begin`] found.
pub(crate) enum Begin<V> {
    Kept(V, Waiter<V>),
    Waiting,
    Refused(Waiter<V>),
    Build(Build),
}
