//! The console's connections counted against `max_conns` and each client's share of them, as
//! fit's `login.Slots` and `login.Peers` (fit's I161), on akio's connection handles and accept
//! hooks (akio's A232, ADR-180).
//!
//! A connection that has not authenticated yet is one anyone may open. When the connections are
//! all taken, or the unauthenticated ones hold all but the share kept for authenticated ones
//! (`max_conns / AUTH_SHARE`), a new connection is not refused: it closes an unauthenticated one
//! and takes its slot, the oldest of the client that holds the most of them (the host's own
//! clients, on loopback, counting as one), so that clients that only reconnect, from a few
//! addresses or from the host, cannot keep out one that authenticates, and their reconnecting
//! closes their own connections first. Only when every connection has authenticated is a new one
//! refused; authenticated connections are never closed for another. A client other than the
//! host's own is refused past its share of the connections (`max_conns / PEER_SHARE`, one at
//! least), before any other is closed for it.

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv6Addr};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// The part of the connections kept for authenticated ones: unauthenticated ones hold at most
/// `max_conns - max_conns / AUTH_SHARE` of them (fit's `AuthShare`).
pub const AUTH_SHARE: usize = 4;

/// The part of the connections one client may hold: `max_conns / PEER_SHARE`, one at least
/// (fit's `PeerShare`). The host's own clients are not held to it.
pub const PEER_SHARE: usize = 4;

/// The client `ip` is counted as, as failed logins are: an IPv4 address as it is (an
/// IPv4-mapped one as IPv4), an IPv6 one by its /64; `None` for the host's own (loopback, or no
/// address), which are not held to a share and count as one client (fit's `Peer`).
#[must_use]
pub fn peer(ip: IpAddr) -> Option<IpAddr> {
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    };
    if ip.is_loopback() || ip.is_unspecified() {
        return None;
    }
    match ip {
        IpAddr::V4(_) => Some(ip),
        IpAddr::V6(v6) => {
            let mask = u128::MAX
                .checked_shl(128u32.saturating_sub(u32::from(super::login::IPV6_PREFIX)))
                .unwrap_or(0);
            Some(IpAddr::V6(Ipv6Addr::from(u128::from(v6) & mask)))
        }
    }
}

/// One connection counted.
struct Entry {
    /// Its client, as [`peer`] names it.
    key: Option<IpAddr>,
    /// Where it is among the unauthenticated connections (oldest first); `None` once it has
    /// authenticated.
    seq: Option<u64>,
    /// Closes it, when it is the one a newer connection takes the slot of.
    evict: Box<dyn Fn() + Send + Sync>,
}

#[derive(Default)]
struct State {
    /// The connections open, by id.
    open: HashMap<u64, Entry>,
    /// The unauthenticated ones, oldest first: their ids by their place.
    unauth: BTreeMap<u64, u64>,
    /// How many unauthenticated connections each client holds.
    held: HashMap<Option<IpAddr>, usize>,
    /// How many connections each client other than the host's own holds.
    peers: HashMap<IpAddr, usize>,
    next: u64,
}

/// The connections of the console, counted (fit's `login.Slots` with its `Peers`). Its methods
/// take one lock, briefly, and call no connection's eviction under it: safe from any thread.
pub struct Slots {
    max: usize,
    unauth_max: usize,
    share: usize,
    st: Mutex<State>,
}

impl std::fmt::Debug for Slots {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let st = self.lock();
        f.debug_struct("Slots")
            .field("max", &self.max)
            .field("open", &st.open.len())
            .field("unauth", &st.unauth.len())
            .finish()
    }
}

impl Slots {
    /// The counts of a console that keeps `conns` connections open at most, none for 0.
    #[must_use]
    pub fn new(conns: usize) -> Slots {
        Slots {
            max: conns,
            unauth_max: conns.saturating_sub(conns / AUTH_SHARE),
            share: (conns / PEER_SHARE).max(1),
            st: Mutex::new(State::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many connections a client other than the host's own may hold.
    #[must_use]
    pub fn share(&self) -> usize {
        self.share
    }

    /// Counts connection `id` from `ip`, unauthenticated, `evict` closing it if a newer one takes
    /// its slot; false, counting nothing, when its client already holds its share or every
    /// connection has authenticated. It may close an unauthenticated connection for it (calling
    /// that one's `evict`, after which that one's [`release`](Self::release) does nothing). Each
    /// connection counted is released when it closes.
    pub fn open(&self, id: u64, ip: IpAddr, evict: impl Fn() + Send + Sync + 'static) -> bool {
        let key = peer(ip);
        let victim = {
            let mut st = self.lock();
            if st.open.contains_key(&id) {
                return false;
            }
            if let Some(k) = key {
                if st.peers.get(&k).copied().unwrap_or(0) >= self.share {
                    return false;
                }
            }
            let mut victim = None;
            if st.open.len() >= self.max || st.unauth.len() >= self.unauth_max {
                let Some(v) = victim_of(&st) else {
                    return false;
                };
                victim = drop_entry(&mut st, v);
            }
            if let Some(k) = key {
                let n = st.peers.entry(k).or_insert(0);
                *n = n.saturating_add(1);
            }
            let seq = st.next;
            st.next = st.next.wrapping_add(1);
            st.unauth.insert(seq, id);
            let n = st.held.entry(key).or_insert(0);
            *n = n.saturating_add(1);
            st.open.insert(
                id,
                Entry {
                    key,
                    seq: Some(seq),
                    evict: Box::new(evict),
                },
            );
            victim
        };
        if let Some(v) = victim {
            (v.evict)();
        }
        true
    }

    /// Frees connection `id`'s slot, once it has closed; again, or after its slot was taken for
    /// another, it does nothing.
    pub fn release(&self, id: u64) {
        let gone = drop_entry(&mut self.lock(), id);
        drop(gone);
    }

    /// Marks connection `id` as authenticated: no longer closed for a newer one. Whether it was
    /// not marked so already (and is counted).
    pub fn authenticate(&self, id: u64) -> bool {
        let mut st = self.lock();
        let Some(seq) = st.open.get_mut(&id).and_then(|e| e.seq.take()) else {
            return false;
        };
        st.unauth.remove(&seq);
        let key = st.open.get(&id).and_then(|e| e.key);
        unhold(&mut st, key);
        true
    }

    /// Marks an authenticated connection `id` as unauthenticated again, as when its login has
    /// ended: it is then the newest of those a new connection may close. Whether it was
    /// authenticated (and is counted).
    pub fn unauthenticate(&self, id: u64) -> bool {
        let mut st = self.lock();
        let seq = st.next;
        let Some(e) = st.open.get_mut(&id).filter(|e| e.seq.is_none()) else {
            return false;
        };
        e.seq = Some(seq);
        let key = e.key;
        st.next = st.next.wrapping_add(1);
        st.unauth.insert(seq, id);
        let n = st.held.entry(key).or_insert(0);
        *n = n.saturating_add(1);
        true
    }

    /// Whether connection `id` is counted and authenticated.
    #[must_use]
    pub fn authenticated(&self, id: u64) -> bool {
        self.lock().open.get(&id).is_some_and(|e| e.seq.is_none())
    }

    /// How many connections are counted, and how many of them have not authenticated.
    #[must_use]
    pub fn counts(&self) -> (usize, usize) {
        let st = self.lock();
        (st.open.len(), st.unauth.len())
    }
}

/// The unauthenticated connection a new one closes: the oldest of the client that holds the
/// most; `None` when there is none.
fn victim_of(st: &State) -> Option<u64> {
    let most = st.held.values().copied().max()?;
    st.unauth.values().copied().find(|id| {
        st.open
            .get(id)
            .is_some_and(|e| st.held.get(&e.key).copied() == Some(most))
    })
}

/// Takes connection `id` out of every count; its entry, if it was counted.
fn drop_entry(st: &mut State, id: u64) -> Option<Entry> {
    let e = st.open.remove(&id)?;
    if let Some(seq) = e.seq {
        st.unauth.remove(&seq);
        unhold(st, e.key);
    }
    if let Some(k) = e.key {
        match st.peers.get_mut(&k) {
            Some(n) if *n > 1 => *n = n.saturating_sub(1),
            _ => {
                st.peers.remove(&k);
            }
        }
    }
    Some(e)
}

fn unhold(st: &mut State, key: Option<IpAddr>) {
    match st.held.get_mut(&key) {
        Some(n) if *n > 1 => *n = n.saturating_sub(1),
        _ => {
            st.held.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    /// A connection a test opens, and whether it was closed for a newer one.
    struct Conn {
        id: u64,
        evicted: Arc<AtomicBool>,
    }

    impl Conn {
        fn evicted(&self) -> bool {
            self.evicted.load(Ordering::SeqCst)
        }
    }

    static IDS: AtomicU64 = AtomicU64::new(1);

    fn open(s: &Slots, ip: &str) -> Option<Conn> {
        let ip: IpAddr = if ip.is_empty() {
            IpAddr::V6(Ipv6Addr::UNSPECIFIED)
        } else {
            ip.parse().unwrap()
        };
        let id = IDS.fetch_add(1, Ordering::Relaxed);
        let evicted = Arc::new(AtomicBool::new(false));
        let e = Arc::clone(&evicted);
        s.open(id, ip, move || e.store(true, Ordering::SeqCst))
            .then_some(Conn { id, evicted })
    }

    /// fit's `TestPeers`, through the slots: each client's share, an IPv6 /64 as one, the
    /// host's own not held to it.
    #[test]
    fn peers() {
        assert_eq!(Slots::new(8).share(), 2);
        assert_eq!(Slots::new(1).share(), 1);
        assert_eq!(Slots::new(0).share(), 1);
        let s = Slots::new(8);
        let a = open(&s, "203.0.113.9").unwrap();
        assert!(open(&s, "::ffff:203.0.113.9").is_some());
        assert!(open(&s, "203.0.113.9").is_none(), "an IPv4 client's share");
        let s = Slots::new(8);
        assert!(open(&s, "2001:db8::1").is_some());
        assert!(open(&s, "2001:db8::2").is_some());
        assert!(open(&s, "2001:db8::ffff").is_none(), "an IPv6 /64's share");
        assert!(open(&s, "2001:db8:0:1::1").is_some(), "another /64");
        let s = Slots::new(64);
        for _ in 0..10 {
            assert!(open(&s, "127.0.0.1").is_some());
            assert!(open(&s, "::1").is_some());
        }
        assert_eq!(peer("::1".parse().unwrap()), None);
        drop(a);
    }

    /// fit's `TestSlotsEvictOldestUnauthenticated`.
    #[test]
    fn evict_oldest_unauthenticated() {
        let s = Slots::new(8);
        assert_eq!(s.share(), 2);
        let cs: Vec<Conn> = (0..6).map(|_| open(&s, "127.0.0.1").unwrap()).collect();
        // The seventh takes the first's slot.
        let c7 = open(&s, "127.0.0.1").unwrap();
        assert!(cs[0].evicted() && !cs[1].evicted());
        s.release(cs[0].id); // after its eviction, nothing
        assert_eq!(s.counts(), (6, 6));
        // Two authenticate: unauthenticated ones hold 4, so that a new one takes a free slot.
        assert!(s.authenticate(cs[1].id));
        assert!(!s.authenticate(cs[1].id));
        assert!(s.authenticated(cs[1].id));
        s.authenticate(cs[2].id);
        let c8 = open(&s, "127.0.0.1").unwrap();
        assert!(!cs[3].evicted(), "a connection was closed with a slot free");
        assert_eq!(s.counts().0, 7);
        for c in [&cs[3], &cs[4], &cs[5], &c7, &c8] {
            s.authenticate(c.id);
        }
        // Full, with one unauthenticated: it goes, and never an authenticated one.
        let a = open(&s, "::1").unwrap();
        let b = open(&s, "").unwrap();
        assert!(a.evicted());
        assert_eq!(s.counts().0, 8);
        for c in cs[1..].iter().chain([&c7, &c8]) {
            assert!(!c.evicted(), "an authenticated connection was closed");
        }
        s.authenticate(b.id);
        assert!(
            open(&s, "127.0.0.1").is_none(),
            "accepted with every connection authenticated"
        );
        // One that closes frees its slot.
        s.release(cs[1].id);
        s.release(cs[1].id);
        assert!(open(&s, "127.0.0.1").is_some(), "refused with a slot free");
    }

    /// fit's `TestSlotsUnauthenticate`.
    #[test]
    fn unauthenticate() {
        let s = Slots::new(4); // 3 unauthenticated at most
        let (a, b, c) = (
            open(&s, "127.0.0.1").unwrap(),
            open(&s, "127.0.0.1").unwrap(),
            open(&s, "127.0.0.1").unwrap(),
        );
        assert!(!s.unauthenticate(a.id));
        s.authenticate(a.id);
        assert!(s.unauthenticate(a.id) && !s.authenticated(a.id));
        // b is the oldest unauthenticated one now, then c, then a.
        open(&s, "127.0.0.1").unwrap();
        open(&s, "127.0.0.1").unwrap();
        assert!(b.evicted() && c.evicted() && !a.evicted());
        // Neither changes a slot taken for another.
        assert!(!s.authenticate(b.id) && !s.unauthenticate(b.id));
        s.authenticate(a.id);
        s.release(a.id);
        assert!(!s.unauthenticate(a.id), "a released slot changed");
    }

    /// fit's `TestSlotsPeerShare`.
    #[test]
    fn peer_share() {
        let s = Slots::new(8);
        let x = open(&s, "203.0.113.9").unwrap();
        let y = open(&s, "203.0.113.9").unwrap();
        assert!(open(&s, "203.0.113.9").is_none(), "the client's share");
        let z = open(&s, "2001:db8::1").unwrap();
        let w1 = open(&s, "127.0.0.1").unwrap();
        let w2 = open(&s, "::1").unwrap();
        let w3 = open(&s, "").unwrap();
        // Full of unauthenticated ones: the oldest of the client that holds the most, the host
        // with 3, goes.
        assert!(open(&s, "198.51.100.1").is_some());
        assert!(
            w1.evicted() && !x.evicted(),
            "not the host's oldest was closed"
        );
        // 203.0.113.9 and the host hold 2 each: the oldest of them, x, goes, and its client's
        // count with it.
        assert!(open(&s, "2001:db8::2").is_some());
        assert!(x.evicted() && !z.evicted());
        assert!(
            open(&s, "203.0.113.9").is_some(),
            "the client's evicted connection is still counted"
        );
        // Now the /64 of z holds 2, as the host does: z is older.
        assert!(z.evicted() && !w2.evicted() && !w3.evicted() && !y.evicted());
        assert!(
            !Slots::new(0).open(0, IpAddr::V6(Ipv6Addr::UNSPECIFIED), || {}),
            "a connection taken with max_conns 0"
        );
        let one = Slots::new(1);
        assert!(open(&one, "").is_some() && open(&one, "").is_some());
    }

    /// fit's `TestSlotsReconnectingAttacker`: clients from three addresses that hold every
    /// unauthenticated slot, and reconnect each time one of theirs is closed, close only their
    /// own connections from then on.
    #[test]
    fn reconnecting_attacker() {
        let s = Slots::new(16); // 12 unauthenticated, 4 a client
        let addrs = ["203.0.113.1", "203.0.113.2", "203.0.113.3"];
        let mut attackers: Vec<Conn> = addrs
            .iter()
            .flat_map(|a| (0..4).map(|_| open(&s, a).unwrap()).collect::<Vec<_>>())
            .collect();
        let admin = open(&s, "198.51.100.7").expect("the admin was refused");
        for i in 0..1000 {
            for j in 0..attackers.len() {
                if attackers[j].evicted() {
                    attackers[j] = open(&s, addrs[j / 4]).expect("an attacker's reconnect");
                }
            }
            assert!(
                !admin.evicted(),
                "the admin's connection was closed after {i} rounds"
            );
        }
        assert!(s.authenticate(admin.id));
    }

    /// fit's `TestSlotsConcurrent`: connections that open, authenticate, unauthenticate and close
    /// at once leave the counts at 0.
    #[test]
    fn concurrent() {
        let s = Arc::new(Slots::new(16));
        let ts: Vec<_> = (0..8u64)
            .map(|g| {
                let s = Arc::clone(&s);
                std::thread::spawn(move || {
                    for i in 0..200u64 {
                        let id = g * 1000 + i + 1;
                        if !s.open(id, "127.0.0.1".parse().unwrap(), || {}) {
                            continue;
                        }
                        if (g + i) % 2 == 0 {
                            s.authenticate(id);
                            s.unauthenticate(id);
                        }
                        s.release(id);
                    }
                })
            })
            .collect();
        for t in ts {
            t.join().unwrap();
        }
        assert_eq!(s.counts(), (0, 0));
        let st = s.lock();
        assert!(st.held.is_empty() && st.peers.is_empty());
    }
}
