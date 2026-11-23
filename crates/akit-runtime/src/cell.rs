//! Publishing a graph to the I/O threads, reading it per request, and retiring the old ones.
//!
//! # Reading (the hot path, rules 12 and 13)
//!
//! Each I/O thread keeps its own `Arc` of the current graph in a thread-local cache, with the
//! epoch it saw. [`GraphCell::with`] loads the cell's epoch (a load of a line the control plane
//! writes once per swap) and refreshes the cached `Arc` only when the epoch moved, taking a lock
//! once per thread per swap. A request touches no reference count that other threads touch: the
//! only per-request writes are to the thread's own slot (it marks itself inside the graph and
//! out again).
//!
//! # Retiring
//!
//! akio offers no way to run work on an idle I/O thread, so an idle thread's cache may keep an
//! old graph's `Arc` for a long time. Its memory is small; what matters is its modules (upstream
//! pools, files, plugins), which must be cleaned up promptly. So a graph is retired, its modules
//! cleaned up, by [`GraphCell::collect`], called from the control plane, as soon as:
//!
//! - every I/O thread that ever read the cell is either outside it or has moved to a later epoch
//!   (no request is running on the old graph), and
//! - no [`Hold`] keeps it (no request or connection that outlives its handler's call still uses
//!   it).
//!
//! The thread's "inside" mark and the control plane's epoch are a store followed by a load on each
//! side, sequentially consistent, so a thread that the control plane sees outside will see the new
//! epoch when it next comes in. A request never starts on a retired graph, and a retired graph's
//! modules are never used again.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use crate::graph::Graph;

/// Cell IDs, to tell cells apart in a thread's cache.
static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

/// One I/O thread's state for one cell, written by that thread alone and read by the control
/// plane when it retires graphs.
#[derive(Debug, Default)]
struct Slot {
    inside: AtomicBool,
    seen: AtomicU64,
}

/// The state every handle of one cell shares.
struct Shared {
    id: usize,
    epoch: AtomicU64,
    current: Mutex<Arc<Graph>>,
    /// Graphs replaced and not yet retired, each with the epoch that replaced it.
    retiring: Mutex<Vec<(u64, Arc<Graph>)>>,
    /// Every thread that has read the cell.
    slots: Mutex<Vec<Arc<Slot>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // Nothing guarded here is left inconsistent by a panic midway: take it as it is.
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Drop for Shared {
    fn drop(&mut self) {
        // No thread is inside: every `with` borrows a live handle. Graphs still held are cleaned
        // up when their last `Hold` drops.
        let current = lock(&self.current).clone();
        for g in std::iter::once(current).chain(lock(&self.retiring).drain(..).map(|(_, g)| g)) {
            // Store, then load, as `Hold::drop` does the other way round: one of the two sees
            // the other and retires the graph (retiring twice does nothing).
            g.orphaned.store(true, SeqCst);
            if g.holds.load(SeqCst) == 0 {
                g.retire();
            }
        }
    }
}

/// One thread's cache entry for one cell.
struct Entry {
    id: usize,
    shared: Weak<Shared>,
    slot: Arc<Slot>,
    epoch: Cell<u64>,
    depth: Cell<u32>,
    graph: RefCell<Arc<Graph>>,
}

thread_local! {
    static CACHE: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
}

/// Publishes graphs to the I/O threads and retires the ones replaced. Cheap to clone: clones
/// share one cell.
#[derive(Clone)]
pub struct GraphCell {
    shared: Arc<Shared>,
}

impl fmt::Debug for GraphCell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GraphCell")
            .field("epoch", &self.epoch())
            .field("retiring", &lock(&self.shared.retiring).len())
            .finish_non_exhaustive()
    }
}

/// A graph kept by a request or connection that outlives its handler's call (a proxied
/// response, a WebSocket, a tunnel): the graph is not retired while a `Hold` on it lives.
/// Taking one costs a shared reference count; take it once per request or connection, not per
/// event.
pub struct Hold {
    graph: Arc<Graph>,
}

impl Deref for Hold {
    type Target = Graph;
    fn deref(&self) -> &Graph {
        &self.graph
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        if self.graph.holds.fetch_sub(1, SeqCst) == 1 && self.graph.orphaned.load(SeqCst) {
            self.graph.retire();
        }
    }
}

impl fmt::Debug for Hold {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Hold").field(&self.graph).finish()
    }
}

/// Marks the thread outside the cell when the outermost `with` ends, panic or not.
struct Outside<'a> {
    entry: &'a Entry,
}

impl Drop for Outside<'_> {
    fn drop(&mut self) {
        let d = self.entry.depth.get().saturating_sub(1);
        self.entry.depth.set(d);
        if d == 0 {
            self.entry.slot.inside.store(false, SeqCst);
        }
    }
}

impl GraphCell {
    /// A cell publishing `graph` at epoch 0.
    #[must_use]
    pub fn new(graph: Graph) -> GraphCell {
        GraphCell {
            shared: Arc::new(Shared {
                id: NEXT_ID.fetch_add(1, SeqCst),
                epoch: AtomicU64::new(0),
                current: Mutex::new(Arc::new(graph)),
                retiring: Mutex::new(Vec::new()),
                slots: Mutex::new(Vec::new()),
            }),
        }
    }

    /// The epoch of the graph published last: 0, then one more per [`swap`](Self::swap).
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.shared.epoch.load(SeqCst)
    }

    /// Publishes `graph`: requests that start after this call see it; those running finish on
    /// the graph they started on. The replaced graph is retired by a later
    /// [`collect`](Self::collect). Returns the new epoch.
    pub fn swap(&self, graph: Graph) -> u64 {
        let mut current = lock(&self.shared.current);
        let old = std::mem::replace(&mut *current, Arc::new(graph));
        // Under the lock: a thread that sees the new epoch then takes the lock gets the new graph.
        let epoch = self.shared.epoch.fetch_add(1, SeqCst).saturating_add(1);
        lock(&self.shared.retiring).push((epoch, old));
        epoch
    }

    /// Retires every replaced graph that no thread is running on and no [`Hold`] keeps, cleaning
    /// up its modules. The control plane calls this after a swap and then now and then (I22 and
    /// I28 run it on a timer) until [`retiring`](Self::retiring) is 0; it takes a few locks and
    /// never blocks on the I/O threads. Returns how many graphs it retired.
    pub fn collect(&self) -> usize {
        let slots = lock(&self.shared.slots).clone();
        let mut retired = Vec::new();
        lock(&self.shared.retiring).retain(|(replaced_at, g)| {
            let quiet = slots
                .iter()
                .all(|s| !s.inside.load(SeqCst) || s.seen.load(SeqCst) >= *replaced_at);
            if quiet && g.holds.load(SeqCst) == 0 {
                retired.push(Arc::clone(g));
                false
            } else {
                true
            }
        });
        // Clean up outside the lock: cleanups may take their time.
        retired.iter().filter(|g| g.retire()).count()
    }

    /// How many replaced graphs are waiting to be retired.
    #[must_use]
    pub fn retiring(&self) -> usize {
        lock(&self.shared.retiring).len()
    }

    /// A [`Hold`] on the current graph (as this thread sees it).
    #[must_use]
    pub fn hold(&self) -> Hold {
        self.with_arc(|g| {
            // Counted before this thread leaves the cell, so `collect` cannot miss it.
            g.holds.fetch_add(1, SeqCst);
            Hold {
                graph: Arc::clone(g),
            }
        })
    }

    /// Runs `f` with the current graph, as this thread sees it: the hot path of every request.
    /// `f` may call `with` again (on this cell or another).
    pub fn with<R>(&self, f: impl FnOnce(&Graph) -> R) -> R {
        self.with_arc(|g| f(g))
    }

    fn with_arc<R, F: FnOnce(&Arc<Graph>) -> R>(&self, f: F) -> R {
        let cached = CACHE.with(|cache| -> Result<R, F> {
            if !cache.borrow().iter().any(|e| e.id == self.shared.id) {
                // First read of this cell on this thread.
                let Ok(mut c) = cache.try_borrow_mut() else {
                    return Err(f); // an outer `with` of another cell borrows the cache
                };
                c.retain(|e| e.shared.strong_count() > 0); // forget dead cells
                c.push(self.register());
            }
            let c = cache.borrow();
            match c.iter().find(|e| e.id == self.shared.id) {
                Some(entry) => Ok(self.enter(entry, f)),
                None => Err(f),
            }
        });
        // Registering needed the cache while an outer `with` of another cell borrowed it: read
        // the cell directly this once, at the cost of a shared reference count.
        cached.unwrap_or_else(|f| {
            let g = lock(&self.shared.current).clone();
            f(&g)
        })
    }

    fn register(&self) -> Entry {
        let slot = Arc::new(Slot::default());
        lock(&self.shared.slots).push(Arc::clone(&slot));
        // The slot is listed before the graph is read, so `collect` sees this thread from the
        // first request on.
        let epoch = self.shared.epoch.load(SeqCst);
        let graph = lock(&self.shared.current).clone();
        slot.seen.store(epoch, SeqCst);
        Entry {
            id: self.shared.id,
            shared: Arc::downgrade(&self.shared),
            slot,
            epoch: Cell::new(epoch),
            depth: Cell::new(0),
            graph: RefCell::new(graph),
        }
    }

    fn enter<R>(&self, entry: &Entry, f: impl FnOnce(&Arc<Graph>) -> R) -> R {
        let depth = entry.depth.get();
        if depth == 0 {
            // Store, then load: see the module docs.
            entry.slot.inside.store(true, SeqCst);
            let epoch = self.shared.epoch.load(SeqCst);
            if epoch != entry.epoch.get() {
                if let Ok(mut g) = entry.graph.try_borrow_mut() {
                    *g = lock(&self.shared.current).clone();
                    entry.epoch.set(epoch);
                    entry.slot.seen.store(epoch, SeqCst);
                }
            }
        }
        entry.depth.set(depth.saturating_add(1));
        let _outside = Outside { entry };
        let g = entry.graph.borrow();
        f(&g)
    }
}
