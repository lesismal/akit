//! Load balancing (I19): which of an upstream's addresses a request goes to, by fit's
//! `round_robin`, `weighted_round_robin` and `least_conn`.
//!
//! fit keeps one turn per upstream, shared by every goroutine. akit keeps the turn **per I/O
//! thread** instead (rules.md rule 13): a [`Balancer`] has a few cache-line-sized slots, and each
//! thread takes its turns from the slot it was given when it first picked (threads are given the
//! slots in order, so one per core while there are no more threads than slots). Each thread then
//! sees fit's sequences exactly (round robin's `a b c a b c`, smooth weighted round robin's
//! `a a b a c a a` for weights 5, 1 and 1), and the shares hold over all threads, exactly over
//! whole rounds, since each thread's picks are whole rounds of its own. Slots start at different
//! places in the sequence, so that the first requests of every thread do not all go to the first
//! address. Two threads that share a slot (more threads than slots) still each take an entry of
//! its sequence, as fit's goroutines do of fit's.
//!
//! What must be shared is shared: `least_conn` reads the requests under way to each address,
//! which the pool keeps per address for `max_conns` anyway; picking writes nothing shared but the
//! thread's own turn.
//!
//! Picking never blocks or allocates. The one lock is weighted round robin's for weights whose
//! sequence is too long to work out ahead (more than [`MAX_SCHEDULE`] entries, which takes over
//! 65 addresses of large, coprime weights): then each slot runs nginx's algorithm on running
//! weights of its own, under the slot's own lock, which only threads sharing the slot contend
//! for.

use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

pub use akit_config::LoadBalancing;

/// The longest weighted round robin sequence worked out ahead, as fit's `maxSchedule`; at 4 bytes
/// an entry, 256 KiB.
pub const MAX_SCHEDULE: usize = 1 << 16;

/// How an upstream's addresses are picked: the policy, the weights, and each thread's turn.
#[derive(Debug)]
pub struct Balancer {
    policy: LoadBalancing,
    weights: Box<[u32]>,
    /// Weighted round robin's sequence of address indexes, one round of it; empty for the other
    /// policies, and for weights whose round is over [`MAX_SCHEDULE`].
    schedule: Box<[u32]>,
    slots: Box<[Slot]>,
}

/// One thread's turn (or a few threads', past one per slot), on a cache line of its own.
#[derive(Debug)]
#[repr(align(128))]
struct Slot {
    next: AtomicUsize,
    /// Weighted round robin's running weights, when there is no schedule.
    current: Mutex<Vec<i64>>,
}

impl Balancer {
    /// A balancer for `weights.len()` addresses (at least one), each of that weight (0 is taken
    /// as 1), by `policy`.
    #[must_use]
    pub fn new(policy: LoadBalancing, weights: &[u32]) -> Balancer {
        let weights: Box<[u32]> = if weights.is_empty() {
            Box::new([1])
        } else {
            weights.iter().map(|&w| w.max(1)).collect()
        };
        let schedule = if policy == LoadBalancing::WeightedRoundRobin {
            schedule(&weights)
        } else {
            Box::default()
        };
        let locked = policy == LoadBalancing::WeightedRoundRobin && schedule.is_empty();
        let n = slot_count();
        // Each slot starts its turn at a different place of the round: the round spread evenly
        // over the slots.
        let round = match policy {
            LoadBalancing::WeightedRoundRobin if !schedule.is_empty() => schedule.len(),
            _ => weights.len(),
        };
        let slots = (0..n)
            .map(|i| Slot {
                next: AtomicUsize::new(i.saturating_mul(round).checked_div(n).unwrap_or(0)),
                current: Mutex::new(if locked {
                    vec![0; weights.len()]
                } else {
                    Vec::new()
                }),
            })
            .collect();
        Balancer {
            policy,
            weights,
            schedule,
            slots,
        }
    }

    /// The policy.
    #[must_use]
    pub fn policy(&self) -> LoadBalancing {
        self.policy
    }

    /// How many addresses it picks from.
    #[must_use]
    pub fn len(&self) -> usize {
        self.weights.len()
    }

    /// Always false: a balancer has at least one address.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// The weight of the address at `i`.
    #[must_use]
    pub fn weight(&self, i: usize) -> u32 {
        self.weights.get(i).copied().unwrap_or(1)
    }

    /// The index of the address the next request goes to. `load(i)` is how many requests are
    /// under way to the address at `i` (waiting for a connection included), which only
    /// `least_conn` reads. Every address is taken as up: [`pick_up`](Self::pick_up) passes over
    /// those that are down.
    pub fn pick(&self, load: impl Fn(usize) -> usize) -> usize {
        self.pick_up(load, |_| true)
    }

    /// [`pick`](Self::pick), passing over the addresses that are down (I20): `up(i)` says whether
    /// the address at `i` may take a request. As fit does, round robin and least connections move
    /// the turn past the addresses passed over, and weighted round robin draws more entries of its
    /// sequence, so that the share of an address that is down goes to the others by their weights.
    /// When every address is down, health is ignored and the pick is the one `pick` makes: an
    /// upstream that may still answer serves better than one that surely will not.
    pub fn pick_up(&self, load: impl Fn(usize) -> usize, up: impl Fn(usize) -> bool) -> usize {
        let Some(slot) = self.slot() else {
            return 0;
        };
        match self.policy {
            LoadBalancing::WeightedRoundRobin => self.weighted(slot, &up),
            LoadBalancing::LeastConn => self.least_conn(slot, load, &up),
            _ => self.round_robin(slot, &up),
        }
    }

    /// This thread's slot.
    fn slot(&self) -> Option<&Slot> {
        let i = THREAD_SLOT.with(|s| {
            s.get().unwrap_or_else(|| {
                let i = NEXT_THREAD.fetch_add(1, Ordering::Relaxed);
                s.set(Some(i));
                i
            })
        });
        // `slots.len()` is a power of two, and at least one.
        let mask = self.slots.len().saturating_sub(1);
        self.slots.get(i & mask)
    }

    fn turn(&self, slot: &Slot) -> usize {
        slot.next.fetch_add(1, Ordering::Relaxed)
    }

    /// The first address up from `first` on, in turn, and how many were passed over; `None`
    /// when none is up.
    fn next_up(&self, first: usize, up: &impl Fn(usize) -> bool) -> Option<(usize, usize)> {
        let n = self.weights.len();
        (0..n).find_map(|i| {
            let b = first.wrapping_add(i).checked_rem(n).unwrap_or(0);
            up(b).then_some((b, i))
        })
    }

    fn round_robin(&self, slot: &Slot, up: &impl Fn(usize) -> bool) -> usize {
        let first = self.turn(slot);
        match self.next_up(first, up) {
            Some((b, skipped)) => {
                if skipped > 0 {
                    // Move the turn past the addresses passed over.
                    slot.next.fetch_add(skipped, Ordering::Relaxed);
                }
                b
            }
            None => first.checked_rem(self.weights.len()).unwrap_or(0),
        }
    }

    fn weighted(&self, slot: &Slot, up: &impl Fn(usize) -> bool) -> usize {
        if self.schedule.is_empty() {
            return self.weighted_locked(slot, up);
        }
        let draw = || {
            let i = self
                .turn(slot)
                .checked_rem(self.schedule.len())
                .unwrap_or(0);
            self.schedule.get(i).map_or(0, |&b| b as usize)
        };
        let first = draw();
        if up(first) {
            return first;
        }
        // Another entry, as many times as it takes: each is a fair draw of the sequence, so the
        // addresses up share the pick by weight. One up is found within a round unless none is,
        // which the scan settles first.
        if self.next_up(0, up).is_none() {
            return first;
        }
        for _ in 0..self.schedule.len() {
            let b = draw();
            if up(b) {
                return b;
            }
        }
        self.next_up(0, up).map_or(first, |(b, _)| b)
    }

    fn weighted_locked(&self, slot: &Slot, up: &impl Fn(usize) -> bool) -> usize {
        let mut current = slot.current.lock().unwrap_or_else(PoisonError::into_inner);
        let healthy = self.next_up(0, up).is_some();
        // nginx's smooth weighted round robin, on this slot's running weights: each pick adds
        // every weight to its running weight, takes the highest, and takes the total back off it;
        // over the addresses up, or over all of them when none is.
        let mut best = None;
        let mut best_current = i64::MIN;
        let mut total = 0i64;
        for (i, (c, &w)) in current.iter_mut().zip(self.weights.iter()).enumerate() {
            if healthy && !up(i) {
                continue;
            }
            *c = c.saturating_add(i64::from(w));
            total = total.saturating_add(i64::from(w));
            if best.is_none() || *c > best_current {
                best = Some(i);
                best_current = *c;
            }
        }
        let best = best.unwrap_or(0);
        if let Some(c) = current.get_mut(best) {
            *c = c.saturating_sub(total);
        }
        best
    }

    fn least_conn(
        &self,
        slot: &Slot,
        load: impl Fn(usize) -> usize,
        up: &impl Fn(usize) -> bool,
    ) -> usize {
        // Ties go to the first found from a starting point that moves on with each pick, so
        // that idle addresses share the load rather than the first taking all of it. The scan
        // starts at the next address in turn that is up, the turn moved past those passed over,
        // so that ties spread over the addresses up rather than going to whichever follows one
        // that is down.
        let n = self.weights.len();
        let first = self.turn(slot);
        let (start, healthy) = match self.next_up(first, up) {
            Some((b, skipped)) => {
                if skipped > 0 {
                    slot.next.fetch_add(skipped, Ordering::Relaxed);
                }
                (b, true)
            }
            None => (first.checked_rem(n).unwrap_or(0), false),
        };
        let mut best = start;
        // (load, weight) of the best so far; u128 products of a usize and a u32 cannot overflow.
        let mut best_lw: Option<(u128, u128)> = None;
        for i in (start..n).chain(0..start) {
            if healthy && !up(i) {
                continue;
            }
            let l = load(i) as u128;
            let w = u128::from(self.weight(i));
            // l/w < best_load/best_weight, without dividing.
            let better = best_lw.is_none_or(|(bl, bw)| l.saturating_mul(bw) < bl.saturating_mul(w));
            if better {
                best = i;
                best_lw = Some((l, w));
            }
        }
        best
    }
}

/// One round of smooth weighted round robin for `weights`, divided by their greatest common
/// divisor; empty when that is longer than [`MAX_SCHEDULE`].
fn schedule(weights: &[u32]) -> Box<[u32]> {
    let g = weights.iter().fold(0, |g, &w| gcd(g, w));
    let w: Vec<i64> = weights
        .iter()
        .map(|&x| i64::from(x.checked_div(g).unwrap_or(0)))
        .collect();
    let total: i64 = w.iter().fold(0i64, |t, &x| t.saturating_add(x));
    let len = usize::try_from(total).unwrap_or(usize::MAX);
    if len == 0 || len > MAX_SCHEDULE {
        return Box::default();
    }
    let mut current = vec![0i64; w.len()];
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        let mut best = 0;
        let mut best_current = i64::MIN;
        for (j, (c, &x)) in current.iter_mut().zip(w.iter()).enumerate() {
            *c = c.saturating_add(x);
            if *c > best_current {
                best = j;
                best_current = *c;
            }
        }
        if let Some(c) = current.get_mut(best) {
            *c = c.saturating_sub(total);
        }
        out.push(u32::try_from(best).unwrap_or(0));
    }
    out.into_boxed_slice()
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a.checked_rem(b).unwrap_or(0));
    }
    a
}

/// How many slots a balancer has: the machine's parallelism (akio runs an I/O thread per core),
/// rounded up to a power of two.
fn slot_count() -> usize {
    static COUNT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *COUNT.get_or_init(|| {
        std::thread::available_parallelism()
            .map_or(1, std::num::NonZeroUsize::get)
            .next_power_of_two()
    })
}

/// The next thread's slot number.
static NEXT_THREAD: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// This thread's slot number, given on its first pick.
    static THREAD_SLOT: Cell<Option<usize>> = const { Cell::new(None) };
}
