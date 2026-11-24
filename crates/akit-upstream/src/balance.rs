//! Load balancing (I19, I36): which of an upstream's addresses a request goes to, by fit's
//! `round_robin`, `weighted_round_robin` and `least_conn` (I19), and `random`, `p2c`, `ip_hash`,
//! `uri_hash`, `header_hash` and `cookie_hash` (I36).
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
//! I36's policies, as fit's (`upstream/hash.go`):
//!
//! - **`random`** draws an address at random, each as likely as its weight; **`p2c`** (the power
//!   of two choices) draws two and takes the one with fewer requests under way for its weight.
//!   Each thread draws from a generator of its own (wyrand, seeded from std's random hash keys),
//!   so a draw writes nothing shared.
//! - **The hashes** place 160 points per unit of weight for each address on a ring of 64-bit
//!   hashes (FNV-1a finished with MurmurHash3's mix, of `address#i`), as fit does, so that a key
//!   goes to the same address in akit and in fit, and adding or removing an address moves only
//!   the keys that go to it. The ring is built when the config is compiled (rule 12), and holds
//!   at most [`MAX_RING_POINTS`]: past that (weights summing over 6,553) each unit of weight gets
//!   fewer points, which spreads keys a little less evenly and is the one place akit's ring
//!   differs from fit's. A request's key is hashed without allocating ([`Balancer::key`]); one
//!   without a key (no header or cookie of that name) goes in turn, as fit's does.
//!
//! The sticky cookie is the pool's ([`crate::sticky`]): it is checked before the balancer.
//!
//! Picking never blocks or allocates. The one lock is weighted round robin's for weights whose
//! sequence is too long to work out ahead (more than [`MAX_SCHEDULE`] entries, which takes over
//! 65 addresses of large, coprime weights): then each slot runs nginx's algorithm on running
//! weights of its own, under the slot's own lock, which only threads sharing the slot contend
//! for.

use std::cell::Cell;
use std::fmt::Write as _;
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use akio_http::Headers;
pub use akit_config::LoadBalancing;

/// The longest weighted round robin sequence worked out ahead, as fit's `maxSchedule`; at 4 bytes
/// an entry, 256 KiB.
pub const MAX_SCHEDULE: usize = 1 << 16;

/// How many points on the ring each unit of an address's weight gets, as nginx's and fit's
/// consistent hashes do.
pub const POINTS_PER_WEIGHT: usize = 160;

/// The most points a hash ring holds: 16 MiB of them. fit's ring holds 160 per unit of weight
/// whatever the total; akit's holds that many while it fits here, and fewer per unit past it.
pub const MAX_RING_POINTS: usize = 1 << 20;

/// What a request is picked by: the parts of it the hash policies and the sticky cookie read.
/// Borrowed from the request for the pick only.
#[derive(Clone, Copy, Debug, Default)]
pub struct Keys<'a> {
    /// The client's address, which `ip_hash` hashes.
    pub peer: Option<IpAddr>,
    /// The path the route matched, which `uri_hash` hashes with the query.
    pub path: &'a str,
    /// The query, without its `?`; empty when there is none.
    pub query: &'a str,
    /// The request's fields: `header_hash`'s header, and the `Cookie` fields `cookie_hash` and
    /// the sticky cookie read.
    pub headers: Option<&'a Headers>,
}

/// How an upstream's addresses are picked: the policy, the weights, and each thread's turn.
#[derive(Debug)]
pub struct Balancer {
    policy: LoadBalancing,
    weights: Box<[u32]>,
    /// Weighted round robin's sequence of address indexes, one round of it; empty for the other
    /// policies, and for weights whose round is over [`MAX_SCHEDULE`].
    schedule: Box<[u32]>,
    slots: Box<[Slot]>,
    /// The hash policies' ring: each point's hash and address index, sorted by hash (then by
    /// address, as fit sorts them); empty for the other policies.
    ring: Box<[(u64, u32)]>,
    /// `header_hash`'s header or `cookie_hash`'s cookie.
    hash_key: Box<str>,
    /// `random` draws by weight: some address weighs other than 1.
    weighted: bool,
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
    /// as 1), by `policy`. A hash policy places the addresses on its ring by their indexes, as
    /// `"0"`, `"1"` and so on, and has no `hash_key`: [`keyed`](Self::keyed) takes the
    /// addresses and the key.
    #[must_use]
    pub fn new(policy: LoadBalancing, weights: &[u32]) -> Balancer {
        let names: Vec<String> = (0..weights.len().max(1)).map(|i| i.to_string()).collect();
        Balancer::keyed(policy, &names, weights, "")
    }

    /// A balancer for `addresses` (at least one), each of its weight in `weights` (1 where
    /// `weights` has none, and for 0), by `policy`; `hash_key` is the header `header_hash`
    /// hashes, or the cookie `cookie_hash` does.
    #[must_use]
    pub fn keyed(
        policy: LoadBalancing,
        addresses: &[impl AsRef<str>],
        weights: &[u32],
        hash_key: &str,
    ) -> Balancer {
        let n = addresses.len().max(1);
        let weights: Box<[u32]> = (0..n)
            .map(|i| weights.get(i).copied().unwrap_or(1).max(1))
            .collect();
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
        let ring = if policy.is_hash() {
            ring(addresses, &weights)
        } else {
            Box::default()
        };
        let weighted = weights.iter().any(|&w| w != 1);
        Balancer {
            policy,
            weights,
            schedule,
            slots,
            ring,
            hash_key: hash_key.into(),
            weighted,
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
        self.pick_keyed(None, load, up)
    }

    /// [`pick_up`](Self::pick_up) for a request whose key, for a hash policy, hashes to `key`
    /// ([`key`](Self::key)): the address of the first point of the ring at or after it, or past
    /// it the first whose address is up. Without a key, a hash policy goes in turn; the other
    /// policies do not read it.
    pub fn pick_keyed(
        &self,
        key: Option<u64>,
        load: impl Fn(usize) -> usize,
        up: impl Fn(usize) -> bool,
    ) -> usize {
        match self.policy {
            LoadBalancing::Random => return self.random(&up),
            LoadBalancing::P2c => return self.p2c(load, &up),
            p if p.is_hash() && !self.ring.is_empty() => {
                if let Some(key) = key {
                    return self.lookup(key, &up);
                }
            }
            _ => {}
        }
        let Some(slot) = self.slot() else {
            return 0;
        };
        match self.policy {
            LoadBalancing::WeightedRoundRobin => self.weighted(slot, &up),
            LoadBalancing::LeastConn => self.least_conn(slot, load, &up),
            _ => self.round_robin(slot, &up),
        }
    }

    /// The hash of the request's key, for a hash policy: the client's IP address
    /// (`ip_hash`), the path and query (`uri_hash`), or the value of the header or cookie
    /// `hash_key` names (`header_hash`, `cookie_hash`); `None` when the request has none, or
    /// for the other policies. Never allocates.
    #[must_use]
    pub fn key(&self, keys: &Keys<'_>) -> Option<u64> {
        match self.policy {
            LoadBalancing::IpHash => {
                let ip = keys.peer?.to_canonical();
                let mut h = Fnv(FNV_OFFSET);
                // Writing to an Fnv never fails.
                let _ = write!(h, "{ip}");
                Some(mix(h.0))
            }
            LoadBalancing::UriHash => {
                let mut s = fnv_add(FNV_OFFSET, keys.path.as_bytes());
                if !keys.query.is_empty() {
                    s = fnv_add(fnv_add(s, b"?"), keys.query.as_bytes());
                }
                Some(mix(s))
            }
            LoadBalancing::HeaderHash => keys
                .headers?
                .get(&self.hash_key)
                .filter(|v| !v.is_empty())
                .map(hash_str),
            LoadBalancing::CookieHash => cookie_value(keys.headers?, &self.hash_key)
                .filter(|v| !v.is_empty())
                .map(hash_str),
            _ => None,
        }
    }

    /// `random`: an address at random, each as likely as its weight, among those up, or among
    /// all of them when none is.
    fn random(&self, up: &impl Fn(usize) -> bool) -> usize {
        let n = self.weights.len();
        if !self.weighted {
            let first = below(n);
            return self.next_up(first, up).map_or(first, |(b, _)| b);
        }
        self.random_weighted(Some(up))
            .or_else(|| self.random_weighted(None::<&fn(usize) -> bool>))
            .unwrap_or(0)
    }

    /// A draw by weight over the addresses up, or over all of them without `up`; `None` when
    /// none is up.
    fn random_weighted(&self, up: Option<&impl Fn(usize) -> bool>) -> Option<usize> {
        let counts = |i: usize| up.is_none_or(|up| up(i));
        let total = self
            .weights
            .iter()
            .enumerate()
            .filter(|&(i, _)| counts(i))
            .fold(0u64, |t, (_, &w)| t.saturating_add(u64::from(w)));
        if total == 0 {
            return None;
        }
        let mut r = below_u64(total);
        for (i, &w) in self.weights.iter().enumerate() {
            if !counts(i) {
                continue;
            }
            match r.checked_sub(u64::from(w)) {
                Some(rest) => r = rest,
                None => return Some(i),
            }
        }
        None
    }

    /// `p2c`: of two addresses drawn at random, the one with fewer requests under way for its
    /// weight. A choice that is down is drawn again from those up other than the other choice;
    /// when none is up, health is ignored.
    fn p2c(&self, load: impl Fn(usize) -> usize, up: &impl Fn(usize) -> bool) -> usize {
        let n = self.weights.len();
        if n <= 1 {
            return 0;
        }
        let mut i = below(n);
        let mut j = below(n.saturating_sub(1));
        if j >= i {
            j = j.saturating_add(1);
        }
        let (mut i_up, mut j_up) = (up(i), up(j));
        if !i_up {
            if let Some(k) = self.draw_up(j, up) {
                (i, i_up) = (k, true);
            }
        }
        if !j_up {
            if let Some(k) = self.draw_up(i, up) {
                (j, j_up) = (k, true);
            }
        }
        match (i_up, j_up) {
            (true, false) => i,
            (false, true) => j,
            _ => {
                // load(i)/weight(i) <= load(j)/weight(j), without dividing.
                let li = (load(i) as u128).saturating_mul(u128::from(self.weight(j)));
                let lj = (load(j) as u128).saturating_mul(u128::from(self.weight(i)));
                if li <= lj { i } else { j }
            }
        }
    }

    /// An address up other than `not`, at random: the first up from a random start.
    fn draw_up(&self, not: usize, up: &impl Fn(usize) -> bool) -> Option<usize> {
        let n = self.weights.len();
        let first = below(n);
        (0..n).find_map(|i| {
            let k = first.wrapping_add(i).checked_rem(n).unwrap_or(0);
            (k != not && up(k)).then_some(k)
        })
    }

    /// The address for a key whose hash is `key`: that of the first point at or after it, or
    /// past it the first whose address is up; when none is, that of the first point.
    fn lookup(&self, key: u64, up: &impl Fn(usize) -> bool) -> usize {
        let n = self.ring.len();
        let lo = self.ring.partition_point(|&(h, _)| h < key);
        let at = |i: usize| {
            self.ring
                .get(lo.wrapping_add(i).checked_rem(n).unwrap_or(0))
                .map_or(0, |&(_, b)| b as usize)
        };
        let first = at(0);
        if up(first) {
            return first;
        }
        // Every point is walked only when every address is down, which the scan settles first.
        if self.next_up(0, up).is_none() {
            return first;
        }
        (1..n).map(at).find(|&b| up(b)).unwrap_or(first)
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

/// The hash ring of `addresses`, each with [`POINTS_PER_WEIGHT`] points per unit of its weight
/// (fewer when they would come to more than [`MAX_RING_POINTS`]), at the hashes of
/// `address#0`, `address#1` and so on, sorted by hash and then by address.
fn ring(addresses: &[impl AsRef<str>], weights: &[u32]) -> Box<[(u64, u32)]> {
    let total: usize = weights
        .iter()
        .fold(0usize, |t, &w| t.saturating_add(w as usize));
    let per_weight = POINTS_PER_WEIGHT
        .min(MAX_RING_POINTS.checked_div(total).unwrap_or(1))
        .max(1);
    let mut points = Vec::with_capacity(total.saturating_mul(per_weight).min(MAX_RING_POINTS));
    let mut name = String::new();
    for (b, &w) in weights.iter().enumerate() {
        let addr = addresses.get(b).map_or("", AsRef::as_ref);
        let Ok(index) = u32::try_from(b) else {
            break;
        };
        for i in 0..(w as usize).saturating_mul(per_weight) {
            name.clear();
            // Writing to a String never fails.
            let _ = write!(name, "{addr}#{i}");
            points.push((hash_str(&name), index));
        }
    }
    let addr = |b: u32| addresses.get(b as usize).map_or("", AsRef::as_ref);
    points.sort_unstable_by(|&(ha, a), &(hb, b)| ha.cmp(&hb).then_with(|| addr(a).cmp(addr(b))));
    points.into_boxed_slice()
}

/// The value of the cookie `name` in `headers`' `Cookie` fields, without its quotes, as fit's
/// `cookieValue`; `None` when there is no such cookie. Never allocates.
pub(crate) fn cookie_value<'h>(headers: &'h Headers, name: &str) -> Option<&'h str> {
    headers.get_all("cookie").find_map(|line| {
        line.split(';').find_map(|part| {
            let (k, v) = part.trim().split_once('=')?;
            (k == name).then(|| v.trim_matches('"'))
        })
    })
}

// FNV-1a, 64 bits, finished with MurmurHash3's mix so that keys that differ in a byte or two
// land far apart on the ring: fit's hash, so that keys land where fit's land.
const FNV_OFFSET: u64 = 14_695_981_039_346_656_037;
const FNV_PRIME: u64 = 1_099_511_628_211;

fn fnv_add(mut h: u64, s: &[u8]) -> u64 {
    for &b in s {
        h ^= u64::from(b);
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

fn mix(mut h: u64) -> u64 {
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^= h >> 33;
    h
}

/// fit's `hashString`.
pub(crate) fn hash_str(s: &str) -> u64 {
    mix(fnv_add(FNV_OFFSET, s.as_bytes()))
}

/// FNV-1a over what is written to it: an IP address hashed as text without formatting it into
/// a buffer.
struct Fnv(u64);

impl std::fmt::Write for Fnv {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0 = fnv_add(self.0, s.as_bytes());
        Ok(())
    }
}

/// A random number below `n` (0 for 0), from this thread's generator.
fn below(n: usize) -> usize {
    usize::try_from(below_u64(n as u64)).unwrap_or(0)
}

/// A random number below `n` (0 for 0): Lemire's multiply-shift, whose bias is below 2^-32 for
/// any `n` a balancer draws from.
pub(crate) fn below_u64(n: u64) -> u64 {
    let wide = u128::from(next_random()).wrapping_mul(u128::from(n));
    u64::try_from(wide >> 64).unwrap_or(0)
}

/// The next number of this thread's wyrand generator, seeded on its first draw from std's
/// random hash keys.
pub(crate) fn next_random() -> u64 {
    RANDOM.with(|s| {
        let mut x = s.get();
        if x == 0 {
            use std::hash::BuildHasher;
            x = std::collections::hash_map::RandomState::new()
                .hash_one(NEXT_THREAD.load(Ordering::Relaxed))
                | 1;
        }
        x = x.wrapping_add(0xa076_1d64_78bd_642f);
        s.set(x);
        let t = u128::from(x).wrapping_mul(u128::from(x ^ 0xe703_7ed1_a0b4_28db));
        // The high and low halves, folded.
        u64::try_from(t >> 64).unwrap_or(0) ^ u64::try_from(t & u128::from(u64::MAX)).unwrap_or(0)
    })
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
    /// This thread's random generator's state; 0 until its first draw.
    static RANDOM: Cell<u64> = const { Cell::new(0) };
}
