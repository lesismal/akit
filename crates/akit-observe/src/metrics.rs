//! Prometheus metrics, as fit's `observe` counts them (I37).
//!
//! Requests are counted by listener and route, upstream responses timed by upstream, and
//! exchanges with each backend counted, with atomic adds alone on the request path: no lock, no
//! shared reference count, nothing formatted (rule 13). The counters are summed only when
//! scraped, and written in Prometheus's text format by [`TextWriter`], with fit's names, labels
//! and help.
//!
//! **Requests.** Each `(listener, route)` a graph serves takes a *slot* from [`Metrics`] when the
//! graph is compiled ([`Lease::route`]), and gives it back when the graph is retired, so the
//! counts of a route a reload keeps go on, and a route no graph has is no longer scraped, as in
//! fit. The counters themselves are kept per thread ([`Metrics::shard`]): a request adds to its
//! thread's shard (the one of the thread that dispatched it, whichever thread sends its response),
//! so no two cores take turns on a cache line. A shard's slots are allocated in chunks of
//! [`CHUNK`], on a thread's first request to a slot of a chunk.
//!
//! **Upstreams.** An upstream's response times ([`Histogram`]) and each of its backends' counts
//! ([`BackendCounters`]) are shared by every graph that has the upstream, by name and address,
//! so they carry over reloads too ([`Lease::upstream`]).
//!
//! **Bounds** (rule 11): at most [`MAX_ROUTE_SLOTS`] `(listener, route)` pairs are counted at
//! once; past it a pair is not counted (and a warning is printed). Every other table here is keyed
//! by the config alone.
//!
//! A slot given back is reused by the next pair that needs one, its counts zeroed then: a request
//! still in flight from a graph retired by a reload that also removed its route, and answered
//! only after a later reload gave the slot to another pair, is counted under that pair. fit,
//! whose counters are garbage collected, has no such window; it takes two reloads during one
//! request.

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The upper bounds, in seconds, of fit's duration histograms (`observe.DurationBuckets`).
pub const DURATION_BUCKETS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

/// [`DURATION_BUCKETS`] in nanoseconds.
const BUCKET_NANOS: [u64; 11] = [
    5_000_000,
    10_000_000,
    25_000_000,
    50_000_000,
    100_000_000,
    250_000_000,
    500_000_000,
    1_000_000_000,
    2_500_000_000,
    5_000_000_000,
    10_000_000_000,
];

/// [`DURATION_BUCKETS`] as their `le` labels, as Go's `FormatFloat(b, 'g', -1, 64)` writes them.
const BUCKET_LABELS: [&str; 11] = [
    "0.005", "0.01", "0.025", "0.05", "0.1", "0.25", "0.5", "1", "2.5", "5", "10",
];

/// The buckets counted: [`DURATION_BUCKETS`]'s, and one past them all.
const COUNTS: usize = DURATION_BUCKETS.len() + 1;

/// The labels of a status's class: `STATUS_CLASSES[status / 100 - 1]`, a status below 200 counted
/// as `1xx` and one of 600 or more as `5xx`, as fit counts them.
pub const STATUS_CLASSES: [&str; 5] = ["1xx", "2xx", "3xx", "4xx", "5xx"];

/// The index of `5xx`, the class of every status past it.
const LAST_CLASS: usize = 4;

/// The media type of what [`TextWriter`] writes: Prometheus's text format, version 0.0.4.
pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// The most `(listener, route)` pairs counted at once.
pub const MAX_ROUTE_SLOTS: usize = 1 << 16;

/// How many slots a shard allocates at once.
pub const CHUNK: usize = 64;

/// The most per-thread shards of the request counters.
pub const MAX_SHARDS: usize = 64;

/// The shards of a [`Histogram`].
const HISTOGRAM_SHARDS: usize = 16;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // Every update under these locks leaves the tables sound: a panic poisons nothing worth
    // refusing.
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn nanos(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

/// The index of the bucket `nanos` falls in.
fn bucket(nanos: u64) -> usize {
    BUCKET_NANOS
        .iter()
        .position(|&b| nanos <= b)
        .unwrap_or(COUNTS - 1)
}

/// The index of `status`'s class in [`STATUS_CLASSES`].
#[must_use]
pub fn status_class(status: u16) -> usize {
    usize::from(status / 100).saturating_sub(1).min(LAST_CLASS)
}

thread_local! {
    /// This thread's shard index, taken once, in the order threads first count.
    static SLOT: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
}

static NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);

/// This thread's index among those that count, for picking a shard.
pub(crate) fn thread_index() -> usize {
    SLOT.with(|s| {
        let mut v = s.get();
        if v == usize::MAX {
            v = NEXT_SLOT.fetch_add(1, Ordering::Relaxed);
            s.set(v);
        }
        v
    })
}

/// A shard of a histogram: a count per bucket, the last for durations past every bound, and
/// their sum in nanoseconds.
#[derive(Default)]
struct HistCounters {
    counts: [AtomicU64; COUNTS],
    sum: AtomicU64,
}

impl HistCounters {
    fn observe(&self, nanos: u64) {
        if let Some(c) = self.counts.get(bucket(nanos)) {
            c.fetch_add(1, Ordering::Relaxed);
        }
        self.sum.fetch_add(nanos, Ordering::Relaxed);
    }

    fn add_to(&self, s: &mut HistogramSnapshot) {
        let mut cum = 0u64;
        for (out, c) in s.buckets.iter_mut().zip(&self.counts) {
            cum = cum.wrapping_add(c.load(Ordering::Relaxed));
            *out = out.wrapping_add(cum);
        }
        let past = self.counts.last().map_or(0, |c| c.load(Ordering::Relaxed));
        s.count = s.count.wrapping_add(cum).wrapping_add(past);
        s.sum_nanos = s
            .sum_nanos
            .wrapping_add(u128::from(self.sum.load(Ordering::Relaxed)));
    }

    fn reset(&self) {
        for c in &self.counts {
            c.store(0, Ordering::Relaxed);
        }
        self.sum.store(0, Ordering::Relaxed);
    }
}

/// A histogram as scraped: the cumulative count of each bucket of [`DURATION_BUCKETS`], the
/// total, and the sum.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct HistogramSnapshot {
    /// The durations at or under each bound of [`DURATION_BUCKETS`].
    pub buckets: [u64; DURATION_BUCKETS.len()],
    /// Every duration counted.
    pub count: u64,
    /// Their sum, in nanoseconds.
    pub sum_nanos: u128,
}

impl HistogramSnapshot {
    /// The sum, in seconds.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn sum_seconds(&self) -> f64 {
        self.sum_nanos as f64 / 1e9
    }
}

/// Aligned to two cache lines, so neighbouring shards never share one (prefetchers pull lines in
/// pairs).
#[repr(align(128))]
#[derive(Default)]
struct Padded<T>(T);

/// Durations counted by [`DURATION_BUCKETS`], over a few shards picked by thread.
pub struct Histogram {
    shards: Box<[Padded<HistCounters>]>,
}

impl Default for Histogram {
    fn default() -> Histogram {
        Histogram::new()
    }
}

impl std::fmt::Debug for Histogram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Histogram").field(&self.snapshot()).finish()
    }
}

impl Histogram {
    /// An empty histogram.
    #[must_use]
    pub fn new() -> Histogram {
        Histogram {
            shards: (0..HISTOGRAM_SHARDS).map(|_| Padded::default()).collect(),
        }
    }

    /// Counts `d`, on this thread's shard.
    pub fn observe(&self, d: Duration) {
        let i = thread_index() & (HISTOGRAM_SHARDS - 1);
        if let Some(s) = self.shards.get(i) {
            s.0.observe(nanos(d));
        }
    }

    /// What it has counted.
    #[must_use]
    pub fn snapshot(&self) -> HistogramSnapshot {
        let mut s = HistogramSnapshot::default();
        for sh in &*self.shards {
            sh.0.add_to(&mut s);
        }
        s
    }
}

/// The exchanges with one backend of an upstream that are over, and how many of them it failed
/// (could not be reached, broke off, timed out), as fit's `Backend.Requests`.
#[derive(Debug, Default)]
pub struct BackendCounters {
    requests: AtomicU64,
    failures: AtomicU64,
}

impl BackendCounters {
    /// Counts an exchange that is over; `failed` says the backend failed it.
    pub fn done(&self, failed: bool) {
        if failed {
            self.failures.fetch_add(1, Ordering::Relaxed);
        }
        self.requests.fetch_add(1, Ordering::Relaxed);
    }

    /// The exchanges over, and those failed among them.
    #[must_use]
    pub fn requests(&self) -> (u64, u64) {
        let failed = self.failures.load(Ordering::Relaxed);
        let done = self.requests.load(Ordering::Relaxed);
        (done.max(failed), failed)
    }
}

/// What a pool counts its exchanges into: its upstream's response times, and each backend's
/// counts, in the order of its addresses.
#[derive(Clone, Debug, Default)]
pub struct UpstreamStats {
    /// How long the upstream took to send a response's header, or to fail without one.
    pub responses: Arc<Histogram>,
    /// Per address, in the upstream's order.
    pub backends: Vec<Arc<BackendCounters>>,
}

impl UpstreamStats {
    /// Counters of its own for an upstream of `backends` addresses, shared with no other graph.
    #[must_use]
    pub fn new(backends: usize) -> UpstreamStats {
        UpstreamStats {
            responses: Arc::new(Histogram::new()),
            backends: (0..backends).map(|_| Arc::default()).collect(),
        }
    }
}

/// The counters of one `(listener, route)` in one shard.
#[repr(align(64))]
#[derive(Default)]
struct RouteCounters {
    codes: [AtomicU64; STATUS_CLASSES.len()],
    hist: HistCounters,
}

/// One thread's request counters, for every slot ([`Metrics::shard`]). A request takes its
/// thread's shard when it is dispatched and counts into it once its response is sent.
pub struct Shard {
    chunks: Box<[OnceLock<Box<[RouteCounters]>>]>,
}

impl std::fmt::Debug for Shard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shard").finish_non_exhaustive()
    }
}

impl Shard {
    fn new() -> Shard {
        Shard {
            chunks: (0..MAX_ROUTE_SLOTS / CHUNK)
                .map(|_| OnceLock::new())
                .collect(),
        }
    }

    /// The counters of `slot`, its chunk allocated if this is its first use.
    fn counters(&self, slot: u32) -> Option<&RouteCounters> {
        let slot = usize::try_from(slot).ok()?;
        let chunk = self
            .chunks
            .get(slot / CHUNK)?
            .get_or_init(|| (0..CHUNK).map(|_| RouteCounters::default()).collect());
        chunk.get(slot % CHUNK)
    }

    /// The counters of `slot`, if its chunk was ever used.
    fn peek(&self, slot: u32) -> Option<&RouteCounters> {
        let slot = usize::try_from(slot).ok()?;
        self.chunks.get(slot / CHUNK)?.get()?.get(slot % CHUNK)
    }

    /// Counts a request of the pair at `slot` answered with `status` after `d`. Three atomic adds,
    /// on this shard's line of the slot.
    pub fn observe(&self, slot: u32, status: u16, d: Duration) {
        let Some(c) = self.counters(slot) else {
            return;
        };
        if let Some(n) = c.codes.get(status_class(status)) {
            n.fetch_add(1, Ordering::Relaxed);
        }
        c.hist.observe(nanos(d));
    }
}

/// The requests counted for one `(listener, route)`, as scraped.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RouteSnapshot {
    /// The listener's name.
    pub listener: String,
    /// The route's name, `#` and its index for a route without one, or empty for requests no
    /// route took.
    pub route: String,
    /// By the class of their status ([`STATUS_CLASSES`]).
    pub codes: [u64; STATUS_CLASSES.len()],
    /// Their durations.
    pub duration: HistogramSnapshot,
}

struct RouteEntry {
    slot: u32,
    refs: usize,
}

#[derive(Default)]
struct Table {
    routes: HashMap<(String, String), RouteEntry>,
    /// The pair holding each slot taken.
    names: HashMap<u32, (String, String)>,
    /// Slots given back, the oldest first.
    free: VecDeque<u32>,
    /// The next slot never taken.
    next: u32,
    /// The requests counted by the slots given back, by class, so that totals never go back.
    retired: [u64; STATUS_CLASSES.len()],
    upstreams: HashMap<String, (Arc<Histogram>, usize)>,
    backends: HashMap<(String, String), (Arc<BackendCounters>, usize)>,
    /// Whether the bound on slots was reported.
    warned: bool,
}

struct Inner {
    table: Mutex<Table>,
    /// Made with the first slot: a process whose configs count no requests has none.
    shards: OnceLock<Box<[Arc<Shard>]>>,
    nshards: usize,
}

/// fit's metrics registry (`observe.Metrics`): the request counters of the routes the graphs in
/// use have, and the upstream counters they share. Made once, when akit starts; cheap to clone.
#[derive(Clone)]
pub struct Metrics(Arc<Inner>);

impl std::fmt::Debug for Metrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Metrics")
            .field("routes", &lock(&self.0.table).routes.len())
            .finish_non_exhaustive()
    }
}

impl Default for Metrics {
    fn default() -> Metrics {
        Metrics::new()
    }
}

impl Metrics {
    /// An empty registry, its request counters sharded over as many threads as the host runs at
    /// once (up to [`MAX_SHARDS`]).
    #[must_use]
    pub fn new() -> Metrics {
        let n = std::thread::available_parallelism().map_or(1, usize::from);
        Metrics::with_shards(n)
    }

    /// The same, with `n` shards (rounded up to a power of two, between 1 and [`MAX_SHARDS`]).
    #[must_use]
    pub fn with_shards(n: usize) -> Metrics {
        Metrics(Arc::new(Inner {
            table: Mutex::new(Table::default()),
            shards: OnceLock::new(),
            nshards: n.clamp(1, MAX_SHARDS).next_power_of_two(),
        }))
    }

    fn shards(&self) -> &[Arc<Shard>] {
        self.0.shards.get_or_init(|| {
            (0..self.0.nshards)
                .map(|_| Arc::new(Shard::new()))
                .collect()
        })
    }

    /// This thread's shard, for a request it dispatches to count into once its response is sent
    /// (from whichever thread sends it). Taking it is a clone of a handle only this thread (and
    /// the threads that outnumber the shards) takes.
    #[must_use]
    pub fn shard(&self) -> Option<Arc<Shard>> {
        let shards = self.0.shards.get()?;
        let i = thread_index() & shards.len().wrapping_sub(1);
        shards.get(i).cloned()
    }

    /// Counts a request of the pair at `slot` answered with `status` after `d`, on this thread's
    /// shard: for a request answered on the thread that dispatched it, before its handler
    /// returned (fit's inline count). Takes no handle.
    pub fn observe(&self, slot: u32, status: u16, d: Duration) {
        let Some(shards) = self.0.shards.get() else {
            return;
        };
        let i = thread_index() & shards.len().wrapping_sub(1);
        if let Some(s) = shards.get(i) {
            s.observe(slot, status, d);
        }
    }

    /// A lease of this registry's counters, for one graph: what it takes is given back when the
    /// lease is dropped.
    #[must_use]
    pub fn lease(&self) -> Lease {
        Lease {
            metrics: self.clone(),
            routes: Vec::new(),
            upstreams: Vec::new(),
            backends: Vec::new(),
        }
    }

    fn take_route(&self, listener: &str, route: &str) -> Option<u32> {
        let mut t = lock(&self.0.table);
        let key = (listener.to_owned(), route.to_owned());
        if let Some(e) = t.routes.get_mut(&key) {
            e.refs = e.refs.saturating_add(1);
            return Some(e.slot);
        }
        let reused = t.free.pop_front();
        let slot = match reused {
            Some(s) => s,
            None if usize::try_from(t.next).is_ok_and(|n| n < MAX_ROUTE_SLOTS) => {
                let s = t.next;
                t.next = t.next.saturating_add(1);
                s
            }
            None => {
                if !t.warned {
                    t.warned = true;
                    crate::warn!(
                        Server,
                        "metrics: more than {MAX_ROUTE_SLOTS} listener and route pairs; the rest are not counted"
                    );
                }
                return None;
            }
        };
        let shards = self.shards();
        if reused.is_some() {
            for s in shards {
                if let Some(c) = s.peek(slot) {
                    for n in &c.codes {
                        n.store(0, Ordering::Relaxed);
                    }
                    c.hist.reset();
                }
            }
        }
        t.names.insert(slot, key.clone());
        t.routes.insert(key, RouteEntry { slot, refs: 1 });
        Some(slot)
    }

    fn give_route(&self, slot: u32) {
        let mut t = lock(&self.0.table);
        let Some(key) = t.names.get(&slot).cloned() else {
            return;
        };
        let Some(e) = t.routes.get_mut(&key) else {
            return;
        };
        e.refs = e.refs.saturating_sub(1);
        if e.refs > 0 {
            return;
        }
        t.routes.remove(&key);
        t.names.remove(&slot);
        let codes = self.codes(slot);
        for (r, n) in t.retired.iter_mut().zip(codes) {
            *r = r.wrapping_add(n);
        }
        t.free.push_back(slot);
    }

    fn codes(&self, slot: u32) -> [u64; STATUS_CLASSES.len()] {
        let mut out = [0u64; STATUS_CLASSES.len()];
        for s in self.0.shards.get().map_or(&[][..], |s| &s[..]) {
            if let Some(c) = s.peek(slot) {
                for (o, n) in out.iter_mut().zip(&c.codes) {
                    *o = o.wrapping_add(n.load(Ordering::Relaxed));
                }
            }
        }
        out
    }

    /// The requests counted for each `(listener, route)` the graphs in use have, sorted by
    /// listener and route.
    #[must_use]
    pub fn routes(&self) -> Vec<RouteSnapshot> {
        let names: Vec<(u32, (String, String))> = lock(&self.0.table)
            .names
            .iter()
            .map(|(s, k)| (*s, k.clone()))
            .collect();
        let shards = self.0.shards.get().map_or(&[][..], |s| &s[..]);
        let mut out: Vec<RouteSnapshot> = names
            .into_iter()
            .map(|(slot, (listener, route))| {
                let mut duration = HistogramSnapshot::default();
                for s in shards {
                    if let Some(c) = s.peek(slot) {
                        c.hist.add_to(&mut duration);
                    }
                }
                RouteSnapshot {
                    listener,
                    route,
                    codes: self.codes(slot),
                    duration,
                }
            })
            .collect();
        out.sort_by(|a, b| (&a.listener, &a.route).cmp(&(&b.listener, &b.route)));
        out
    }

    /// Every request counted since the registry was made, by class, routes no graph has any
    /// more included: counters that never go back (fit's `Totals`, `GET /status`'s requests).
    #[must_use]
    pub fn totals(&self) -> [u64; STATUS_CLASSES.len()] {
        let (mut out, slots) = {
            let t = lock(&self.0.table);
            (t.retired, t.names.keys().copied().collect::<Vec<_>>())
        };
        for slot in slots {
            for (o, n) in out.iter_mut().zip(self.codes(slot)) {
                *o = o.wrapping_add(n);
            }
        }
        out
    }

    fn take_upstream(&self, name: &str, addresses: &[&str]) -> UpstreamStats {
        let mut t = lock(&self.0.table);
        let e = t
            .upstreams
            .entry(name.to_owned())
            .or_insert_with(|| (Arc::new(Histogram::new()), 0));
        e.1 = e.1.saturating_add(1);
        let responses = Arc::clone(&e.0);
        let backends = addresses
            .iter()
            .map(|a| {
                let e = t
                    .backends
                    .entry((name.to_owned(), (*a).to_owned()))
                    .or_insert_with(|| (Arc::default(), 0));
                e.1 = e.1.saturating_add(1);
                Arc::clone(&e.0)
            })
            .collect();
        UpstreamStats {
            responses,
            backends,
        }
    }
}

/// The counters one graph took from [`Metrics`], given back when it is dropped: when the graph
/// is retired, as fit's graph gives back its `RequestMetrics`.
pub struct Lease {
    metrics: Metrics,
    routes: Vec<u32>,
    upstreams: Vec<String>,
    backends: Vec<(String, String)>,
}

impl std::fmt::Debug for Lease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lease")
            .field("routes", &self.routes.len())
            .field("upstreams", &self.upstreams)
            .finish_non_exhaustive()
    }
}

impl Lease {
    /// The slot the requests `route` (its name, `#` and its index, or empty for those no route
    /// takes) answers on `listener` are counted in: [`Shard::observe`]'s. `None` past
    /// [`MAX_ROUTE_SLOTS`] pairs.
    pub fn route(&mut self, listener: &str, route: &str) -> Option<u32> {
        let slot = self.metrics.take_route(listener, route)?;
        self.routes.push(slot);
        Some(slot)
    }

    /// The counters of the upstream `name` and of its `addresses`, shared with every graph that
    /// has them.
    pub fn upstream(&mut self, name: &str, addresses: &[&str]) -> UpstreamStats {
        self.upstreams.push(name.to_owned());
        self.backends
            .extend(addresses.iter().map(|a| (name.to_owned(), (*a).to_owned())));
        self.metrics.take_upstream(name, addresses)
    }

    /// The registry.
    #[must_use]
    pub fn metrics(&self) -> &Metrics {
        &self.metrics
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        for slot in self.routes.drain(..) {
            self.metrics.give_route(slot);
        }
        let mut t = lock(&self.metrics.0.table);
        for name in self.upstreams.drain(..) {
            if let Some(e) = t.upstreams.get_mut(&name) {
                e.1 = e.1.saturating_sub(1);
                if e.1 == 0 {
                    t.upstreams.remove(&name);
                }
            }
        }
        for key in self.backends.drain(..) {
            if let Some(e) = t.backends.get_mut(&key) {
                e.1 = e.1.saturating_sub(1);
                if e.1 == 0 {
                    t.backends.remove(&key);
                }
            }
        }
    }
}

/// The configs applied: fit's `Configs.Reloads`, version and time applied. Cheap to clone.
#[derive(Clone, Debug)]
pub struct ConfigStats(Arc<ConfigInner>);

#[derive(Debug)]
struct ConfigInner {
    tried: AtomicU64,
    failed: AtomicU64,
    version: AtomicU64,
    applied_ms: AtomicI64,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

impl Default for ConfigStats {
    fn default() -> ConfigStats {
        ConfigStats::new()
    }
}

impl ConfigStats {
    /// The first config, version 1, applied now.
    #[must_use]
    pub fn new() -> ConfigStats {
        ConfigStats(Arc::new(ConfigInner {
            tried: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            version: AtomicU64::new(1),
            applied_ms: AtomicI64::new(now_ms()),
        }))
    }

    /// Counts a config applied after the first (`ok`: it was, and is the running one now, with
    /// the next version) or one that failed (the running one stays).
    pub fn reloaded(&self, ok: bool) {
        self.0.tried.fetch_add(1, Ordering::Relaxed);
        if ok {
            self.0.version.fetch_add(1, Ordering::Relaxed);
            self.0.applied_ms.store(now_ms(), Ordering::Relaxed);
        } else {
            self.0.failed.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The reloads tried, and failed among them.
    #[must_use]
    pub fn reloads(&self) -> (u64, u64) {
        (
            self.0.tried.load(Ordering::Relaxed),
            self.0.failed.load(Ordering::Relaxed),
        )
    }

    /// The running config's version: 1 for the first, one more for each applied since.
    #[must_use]
    pub fn version(&self) -> u64 {
        self.0.version.load(Ordering::Relaxed)
    }

    /// When the running config was applied, in milliseconds since the Unix epoch.
    #[must_use]
    pub fn applied_ms(&self) -> i64 {
        self.0.applied_ms.load(Ordering::Relaxed)
    }
}

/// Writes metrics in Prometheus's text format, version 0.0.4, as fit's `observe.TextWriter`:
/// each family begun with [`family`](Self::family), its samples after it.
#[derive(Debug, Default)]
pub struct TextWriter {
    /// What was written.
    pub out: Vec<u8>,
}

impl TextWriter {
    /// A writer appending to `out`.
    #[must_use]
    pub fn new(out: Vec<u8>) -> TextWriter {
        TextWriter { out }
    }

    /// Begins a family of metrics: its `HELP` and `TYPE` lines.
    pub fn family(&mut self, name: &str, typ: &str, help: &str) {
        self.out.extend_from_slice(b"# HELP ");
        self.out.extend_from_slice(name.as_bytes());
        self.out.push(b' ');
        for c in help.chars() {
            match c {
                '\\' => self.out.extend_from_slice(b"\\\\"),
                '\n' => self.out.extend_from_slice(b"\\n"),
                c => self.push_char(c),
            }
        }
        self.out.extend_from_slice(b"\n# TYPE ");
        self.out.extend_from_slice(name.as_bytes());
        self.out.push(b' ');
        self.out.extend_from_slice(typ.as_bytes());
        self.out.push(b'\n');
    }

    fn push_char(&mut self, c: char) {
        let mut b = [0u8; 4];
        self.out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
    }

    /// A sample of `name` with `labels`, names and values.
    pub fn sample(&mut self, name: &str, value: f64, labels: &[(&str, &str)]) {
        self.out.extend_from_slice(name.as_bytes());
        if !labels.is_empty() {
            self.out.push(b'{');
            for (i, (k, v)) in labels.iter().enumerate() {
                if i > 0 {
                    self.out.push(b',');
                }
                self.out.extend_from_slice(k.as_bytes());
                self.out.extend_from_slice(b"=\"");
                for c in v.chars() {
                    match c {
                        '\\' => self.out.extend_from_slice(b"\\\\"),
                        '"' => self.out.extend_from_slice(b"\\\""),
                        '\n' => self.out.extend_from_slice(b"\\n"),
                        c => self.push_char(c),
                    }
                }
                self.out.push(b'"');
            }
            self.out.push(b'}');
        }
        self.out.push(b' ');
        let mut s = String::new();
        append_float(&mut s, value);
        self.out.extend_from_slice(s.as_bytes());
        self.out.push(b'\n');
    }

    /// A counter's or gauge's sample of an integer.
    #[allow(clippy::cast_precision_loss)]
    pub fn sample_u64(&mut self, name: &str, value: u64, labels: &[(&str, &str)]) {
        self.sample(name, value as f64, labels);
    }

    /// The samples of a histogram: its buckets, its sum and its count.
    #[allow(clippy::cast_precision_loss)]
    pub fn histogram(&mut self, name: &str, h: &HistogramSnapshot, labels: &[(&str, &str)]) {
        let bucket = format!("{name}_bucket");
        let mut with_le: Vec<(&str, &str)> = labels.to_vec();
        for (le, n) in BUCKET_LABELS.iter().zip(h.buckets) {
            with_le.truncate(labels.len());
            with_le.push(("le", le));
            self.sample(&bucket, n as f64, &with_le);
        }
        with_le.truncate(labels.len());
        with_le.push(("le", "+Inf"));
        self.sample(&bucket, h.count as f64, &with_le);
        self.sample(&format!("{name}_sum"), h.sum_seconds(), labels);
        self.sample(&format!("{name}_count"), h.count as f64, labels);
    }
}

/// `v` as fit writes it: an integer as one, `+Inf`, `-Inf` and `NaN` as Prometheus spells them,
/// other values in the shortest form that reads back the same.
#[allow(clippy::cast_possible_truncation)]
fn append_float(s: &mut String, v: f64) {
    if v.is_nan() {
        s.push_str("NaN");
    } else if v.is_infinite() {
        s.push_str(if v > 0.0 { "+Inf" } else { "-Inf" });
    } else if v == v.trunc() && v.abs() < 1e15 {
        let _ = write!(s, "{}", v as i64);
    } else {
        let _ = write!(s, "{v}");
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::time::Duration;

    use super::{
        ConfigStats, DURATION_BUCKETS, Histogram, Metrics, TextWriter, append_float, bucket,
        status_class,
    };

    #[test]
    fn buckets_and_classes() {
        assert_eq!(bucket(0), 0);
        assert_eq!(bucket(5_000_000), 0);
        assert_eq!(bucket(5_000_001), 1);
        assert_eq!(bucket(10_000_000_000), DURATION_BUCKETS.len() - 1);
        assert_eq!(bucket(u64::MAX), DURATION_BUCKETS.len());
        for (status, class) in [(0, 0), (101, 0), (200, 1), (404, 3), (503, 4), (999, 4)] {
            assert_eq!(status_class(status), class, "{status}");
        }
    }

    #[test]
    fn floats() {
        for (v, want) in [
            (0.0, "0"),
            (3.0, "3"),
            (0.25, "0.25"),
            (f64::INFINITY, "+Inf"),
            (f64::NEG_INFINITY, "-Inf"),
            (f64::NAN, "NaN"),
            (1e15, "1000000000000000"),
        ] {
            let mut s = String::new();
            append_float(&mut s, v);
            assert_eq!(s, want);
        }
    }

    #[test]
    fn histograms_are_cumulative() {
        let h = Histogram::new();
        h.observe(Duration::from_millis(1));
        h.observe(Duration::from_millis(30));
        h.observe(Duration::from_secs(20));
        let s = h.snapshot();
        assert_eq!(s.count, 3);
        assert_eq!(s.buckets.first(), Some(&1));
        assert_eq!(s.buckets.get(3), Some(&2));
        assert_eq!(s.buckets.last(), Some(&2));
        assert_eq!(s.sum_nanos, 20_031_000_000);
        let mut w = TextWriter::default();
        w.family("x_seconds", "histogram", "Help with a \\ and a\nline.");
        w.histogram("x_seconds", &s, &[("a", "q\"uote")]);
        let text = String::from_utf8(w.out).unwrap();
        assert!(text.starts_with(
            "# HELP x_seconds Help with a \\\\ and a\\nline.\n# TYPE x_seconds histogram\n"
        ));
        assert!(text.contains("x_seconds_bucket{a=\"q\\\"uote\",le=\"0.005\"} 1\n"));
        assert!(text.contains("x_seconds_bucket{a=\"q\\\"uote\",le=\"10\"} 2\n"));
        assert!(text.contains("x_seconds_bucket{a=\"q\\\"uote\",le=\"+Inf\"} 3\n"));
        assert!(text.contains("x_seconds_sum{a=\"q\\\"uote\"} 20.031\n"));
        assert!(text.ends_with("x_seconds_count{a=\"q\\\"uote\"} 3\n"));
    }

    #[test]
    fn slots_carry_over_and_retire() {
        let m = Metrics::with_shards(4);
        assert!(m.shard().is_none());
        let mut a = m.lease();
        let s = a.route("web", "hello").unwrap();
        let shard = m.shard().unwrap();
        shard.observe(s, 200, Duration::from_millis(2));
        shard.observe(s, 503, Duration::from_millis(2));
        // A second graph with the same pair shares its counts.
        let mut b = m.lease();
        assert_eq!(b.route("web", "hello"), Some(s));
        drop(a);
        let routes = m.routes();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].codes, [0, 1, 0, 0, 1]);
        assert_eq!(routes[0].duration.count, 2);
        // Gone with the last graph: its counts stay in the totals, and its slot, reused, starts
        // from nothing.
        drop(b);
        assert!(m.routes().is_empty());
        assert_eq!(m.totals(), [0, 1, 0, 0, 1]);
        let mut c = m.lease();
        assert_eq!(c.route("web", "other"), Some(s));
        assert_eq!(m.routes()[0].codes, [0; 5]);
        assert_eq!(m.totals(), [0, 1, 0, 0, 1]);
    }

    #[test]
    fn upstreams_are_shared_by_name_and_address() {
        let m = Metrics::new();
        let mut a = m.lease();
        let ua = a.upstream("back", &["10.0.0.1:80", "10.0.0.2:80"]);
        ua.backends[1].done(true);
        ua.backends[1].done(false);
        let mut b = m.lease();
        let ub = b.upstream("back", &["10.0.0.2:80"]);
        assert_eq!(ub.backends[0].requests(), (2, 1));
        drop(a);
        drop(b);
        let mut c = m.lease();
        assert_eq!(
            c.upstream("back", &["10.0.0.2:80"]).backends[0].requests(),
            (0, 0)
        );
    }

    #[test]
    fn config_stats() {
        let c = ConfigStats::new();
        assert_eq!((c.version(), c.reloads()), (1, (0, 0)));
        c.reloaded(true);
        c.reloaded(false);
        assert_eq!((c.version(), c.reloads()), (2, (2, 1)));
        assert!(c.applied_ms() > 0);
    }
}
