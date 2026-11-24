//! Deferred jobs for the active health checks (I35), and for the layer 4 services' idle sweeps and
//! drains (I51, `akit-l4`): one thread of akit's own that runs each job once its delay has passed.
//!
//! A stopgap for akio API request A45 (a public timer on akio's engine): akio's own timers, the
//! I/O loops' wheels and akio-http's request-timeout thread, are not public at the pinned commit.
//! The thread waits on a condition variable, never on an I/O thread; a job runs on it and must be
//! quick and must not block: the probes' jobs only start a probe (each call of which returns at
//! once) or fail one that took too long. Scheduling takes the timers' lock for a push, from any
//! thread, the I/O threads among them; the thread holds it only to pop.
//!
//! The pending jobs are bounded ([`MAX_TIMERS`], rules.md rule 11): each probed address has at
//! most two (its next probe, and its probe's timeout), so the bound is the addresses probed at
//! once, by every graph not yet retired. Past it a job is refused, and [`after`] says so.

use std::cmp::Ordering as CmpOrdering;
use std::collections::BinaryHeap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// The most jobs pending at once.
pub const MAX_TIMERS: usize = 1 << 16;

type Job = Box<dyn FnOnce() + Send>;

/// A job and when it runs; the heap's order is the earliest first, then the first scheduled.
struct Entry {
    at: Instant,
    seq: u64,
    job: Job,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == CmpOrdering::Equal
    }
}

impl Eq for Entry {}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

impl Ord for Entry {
    /// Reversed, so that `BinaryHeap`, a max-heap, gives the earliest.
    fn cmp(&self, other: &Self) -> CmpOrdering {
        (other.at, other.seq).cmp(&(self.at, self.seq))
    }
}

#[derive(Default)]
struct State {
    heap: BinaryHeap<Entry>,
    seq: u64,
}

#[derive(Default)]
struct Timers {
    state: Mutex<State>,
    ready: Condvar,
}

impl Timers {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The thread's loop: runs each job once it is due, outside the lock.
    fn run(&self) {
        let mut st = self.lock();
        loop {
            let now = Instant::now();
            let wait = match st.heap.peek() {
                Some(e) if e.at <= now => None,
                Some(e) => Some(e.at.saturating_duration_since(now)),
                None => {
                    st = self.ready.wait(st).unwrap_or_else(PoisonError::into_inner);
                    continue;
                }
            };
            match wait {
                Some(d) => {
                    st = self
                        .ready
                        .wait_timeout(st, d)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
                None => {
                    let Some(e) = st.heap.pop() else { continue };
                    drop(st);
                    // A job that panics is a bug, but must not stop the others' timers.
                    let _ = catch_unwind(AssertUnwindSafe(e.job));
                    st = self.lock();
                }
            }
        }
    }
}

/// The timers, their thread started on first use; `None` when it could not be.
fn timers() -> Option<&'static Arc<Timers>> {
    static T: OnceLock<Option<Arc<Timers>>> = OnceLock::new();
    T.get_or_init(|| {
        let t = Arc::new(Timers::default());
        let run = Arc::clone(&t);
        std::thread::Builder::new()
            .name("akit-probe-timer".into())
            .spawn(move || run.run())
            .ok()
            .map(|_| t)
    })
    .as_ref()
}

/// Runs `job` on the timers' thread once `delay` has passed. Never blocks for long. False, and
/// `job` dropped unrun, when [`MAX_TIMERS`] jobs are pending or the thread could not be started.
pub fn after(delay: Duration, job: impl FnOnce() + Send + 'static) -> bool {
    let Some(t) = timers() else {
        return false;
    };
    let now = Instant::now();
    // A delay too long for the clock: a year is as good as never, but still a bound.
    let at = now
        .checked_add(delay)
        .or_else(|| now.checked_add(Duration::from_secs(365 * 24 * 3600)))
        .unwrap_or(now);
    let mut st = t.lock();
    if st.heap.len() >= MAX_TIMERS {
        return false;
    }
    st.seq = st.seq.wrapping_add(1);
    let seq = st.seq;
    st.heap.push(Entry {
        at,
        seq,
        job: Box::new(job),
    });
    drop(st);
    t.ready.notify_one();
    true
}
