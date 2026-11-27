//! The parsed movies a library keeps (fit's `medialib/tables.go`): a rendition's `moov` read and
//! parsed by akio-media's `mp4` once, for every request that waits for it, and kept, bounded by
//! `max_table_bytes`, the least recently used dropped first, until its file changes or leaves the
//! catalog. A movie is admitted by the bytes its probe said it takes before it is read.
//!
//! fit keeps a table a track; akio-media's `Movie::parse` reads every track's tables at once, so
//! akit keeps a movie a file (decision 043).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use akio_media::mp4::{Limits, MoovLocation, Movie};

use super::Error;
use super::catalog::Rendition;

/// The most calls that may wait for one movie being read; one more is answered busy.
pub const MAX_WAITERS: usize = 1024;

/// What a call for a movie gets.
pub type MovieResult = Result<Arc<Movie>, Error>;
/// A call waiting for a movie.
pub(crate) type Waiter = Box<dyn FnOnce(MovieResult) + Send>;

/// A movie's key: its file as the catalog has it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    pub(crate) abs: PathBuf,
    pub(crate) size: u64,
    pub(crate) modified: SystemTime,
}

struct Entry {
    id: u64,
    movie: Option<Arc<Movie>>,
    bytes: u64,
    waiters: Vec<Waiter>,
    tick: u64,
}

#[derive(Default)]
struct State {
    max: u64,
    used: u64,
    m: HashMap<Key, Entry>,
    /// The movies read, by when last used.
    lru: BTreeMap<u64, Key>,
    tick: u64,
    next_id: u64,
    /// The waiters of reads dropped before they ended, by the read's ID.
    orphans: HashMap<u64, Vec<Waiter>>,
    hits: u64,
    misses: u64,
}

/// What a library's movies take, and how often one asked for was kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TableStats {
    /// Movies kept.
    pub tables: usize,
    /// Their bytes, with those being read.
    pub bytes: u64,
    /// `max_table_bytes`.
    pub max: u64,
    /// Calls that found their movie kept.
    pub hits: u64,
    /// Calls that did not: read from the file, a read under way waited for, or refused.
    pub misses: u64,
}

/// A library's movies.
pub(crate) struct Tables(Mutex<State>);

impl std::fmt::Debug for Tables {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tables")
            .field("stats", &self.stats())
            .finish()
    }
}

/// What a movie of `tracks` tracks whose indexes take `index` bytes is reckoned to take, with its
/// codec configurations of `configs` bytes.
pub(crate) fn reckon(index: u64, configs: u64, tracks: usize) -> u64 {
    index
        .saturating_add(configs)
        .saturating_add((tracks as u64).saturating_mul(512))
        .saturating_add(256)
}

fn movie_bytes(m: &Movie) -> u64 {
    let configs: u64 = m.tracks.iter().map(|t| t.track.config.len() as u64).sum();
    reckon(m.index_bytes() as u64, configs, m.tracks.len())
}

/// The start of a read: what the caller does with the movie's slot.
pub(crate) enum Begin {
    /// Kept: here it is.
    Kept(Arc<Movie>),
    /// Being read: the waiter was added.
    Waiting,
    /// Refused at once.
    Refused(Error),
    /// The caller reads it, then calls [`Tables::done`] with this ID.
    Read(u64),
}

impl Tables {
    pub(crate) fn new(max: u64) -> Tables {
        Tables(Mutex::new(State {
            max,
            ..State::default()
        }))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn stats(&self) -> TableStats {
        let st = self.lock();
        TableStats {
            tables: st.lru.len(),
            bytes: st.used,
            max: st.max,
            hits: st.hits,
            misses: st.misses,
        }
    }

    /// Looks for `key`, whose movie is reckoned to take `need` bytes: kept, being read (`waiter`
    /// added), refused, or to be read by the caller, its bytes held meanwhile.
    pub(crate) fn begin(&self, key: &Key, need: u64, waiter: Waiter) -> (Begin, Option<Waiter>) {
        let mut st = self.lock();
        st.tick = st.tick.saturating_add(1);
        let tick = st.tick;
        if let Some(e) = st.m.get_mut(key) {
            if let Some(m) = &e.movie {
                let m = Arc::clone(m);
                let old = std::mem::replace(&mut e.tick, tick);
                st.hits = st.hits.saturating_add(1);
                st.lru.remove(&old);
                st.lru.insert(tick, key.clone());
                return (Begin::Kept(m), Some(waiter));
            }
            if e.waiters.len() >= MAX_WAITERS {
                st.misses = st.misses.saturating_add(1);
                return (Begin::Refused(Error::Busy), Some(waiter));
            }
            e.waiters.push(waiter);
            st.misses = st.misses.saturating_add(1);
            return (Begin::Waiting, None);
        }
        st.misses = st.misses.saturating_add(1);
        if need > st.max {
            return (Begin::Refused(Error::TooLarge), Some(waiter));
        }
        while st.used.saturating_add(need) > st.max {
            let Some((_, k)) = st.lru.pop_first() else {
                break;
            };
            if let Some(e) = st.m.remove(&k) {
                st.used = st.used.saturating_sub(e.bytes);
            }
        }
        if st.used.saturating_add(need) > st.max {
            // Movies being read hold the rest.
            return (Begin::Refused(Error::Busy), Some(waiter));
        }
        st.next_id = st.next_id.saturating_add(1);
        let id = st.next_id;
        st.m.insert(
            key.clone(),
            Entry {
                id,
                movie: None,
                bytes: need,
                waiters: vec![waiter],
                tick: 0,
            },
        );
        st.used = st.used.saturating_add(need);
        (Begin::Read(id), None)
    }

    /// Ends the read `id` of `key` with `r`, and calls those that wait.
    pub(crate) fn done(&self, key: &Key, id: u64, r: MovieResult) {
        let mut st = self.lock();
        let ours = st.m.get(key).is_some_and(|e| e.id == id);
        let waiters = if !ours {
            // Dropped while it was read: those waiting get it, and it is not kept.
            st.orphans.remove(&id).unwrap_or_default()
        } else if let Ok(m) = &r {
            st.tick = st.tick.saturating_add(1);
            let tick = st.tick;
            let bytes = movie_bytes(m);
            let mut waiters = Vec::new();
            let mut was = 0;
            if let Some(e) = st.m.get_mut(key) {
                was = e.bytes;
                e.movie = Some(Arc::clone(m));
                e.bytes = bytes;
                e.tick = tick;
                waiters = std::mem::take(&mut e.waiters);
            }
            st.used = st.used.saturating_sub(was).saturating_add(bytes);
            st.lru.insert(tick, key.clone());
            waiters
        } else {
            let e = st.m.remove(key);
            let (bytes, waiters) = e.map_or((0, Vec::new()), |e| (e.bytes, e.waiters));
            st.used = st.used.saturating_sub(bytes);
            waiters
        };
        drop(st);
        for w in waiters {
            w(r.clone());
        }
    }

    /// Drops `key`'s movie; the waiters of a read under way are kept for its end, which hands
    /// them what it reads.
    fn forget(st: &mut State, key: &Key) {
        let Some(e) = st.m.remove(key) else {
            return;
        };
        st.used = st.used.saturating_sub(e.bytes);
        if e.movie.is_some() {
            st.lru.remove(&e.tick);
        } else {
            st.orphans.insert(e.id, e.waiters);
        }
    }

    /// Sets `max_table_bytes`, dropping the least recently used past it.
    pub(crate) fn resize(&self, max: u64) {
        let mut st = self.lock();
        st.max = max;
        while st.used > st.max {
            let Some((_, k)) = st.lru.pop_first() else {
                break;
            };
            if let Some(e) = st.m.remove(&k) {
                st.used = st.used.saturating_sub(e.bytes);
            }
        }
    }

    /// Drops the movies of the file at `abs`; a read under way hands what it reads to those
    /// waiting, then keeps nothing.
    pub(crate) fn drop_file(&self, abs: &std::path::Path) {
        let mut st = self.lock();
        let keys: Vec<Key> = st.m.keys().filter(|k| k.abs == abs).cloned().collect();
        for k in keys {
            Tables::forget(&mut st, &k);
        }
    }

    /// Drops the movies of files `keep` does not hold as they were read.
    pub(crate) fn keep(&self, keep: &std::collections::HashSet<Key>) {
        let mut st = self.lock();
        let keys: Vec<Key> = st.m.keys().filter(|k| !keep.contains(k)).cloned().collect();
        for k in keys {
            Tables::forget(&mut st, &k);
        }
    }

    /// Drops every movie.
    pub(crate) fn clear(&self) {
        let mut st = self.lock();
        let keys: Vec<Key> = st.m.keys().cloned().collect();
        for k in keys {
            Tables::forget(&mut st, &k);
        }
    }
}

/// Reads and parses the `moov` of `r` from `h`.
pub(crate) fn read_movie(
    h: &super::files::Handle,
    r: &Rendition,
    limits: &Limits,
) -> Result<Movie, Error> {
    let (off, size) = r.moov.ok_or(Error::NotServed)?;
    let len = usize::try_from(size).map_err(|_| Error::TooLarge)?;
    let moov = h
        .read_exact(off, len)
        .map_err(|e| Error::Io(e.to_string()))?;
    Movie::parse(&moov, &MoovLocation::new(off, size, h.size), limits)
        .map_err(|e| Error::Malformed(e.to_string()))
}
