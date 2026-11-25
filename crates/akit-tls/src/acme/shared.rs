//! Challenges shared through a storage instances on several hosts share (I62, fit's
//! `tlsmgr/shared.go`, decision 014): with a [`Storage`] whose [`shared`](Storage::shared) is
//! true, ACME keeps each challenge's answer in it while the challenge is under way, as
//! `challenges/<CA>/http-01/<token>` and `challenges/<CA>/tls-alpn-01/<name>`, so that the
//! instance the CA reaches answers it, whichever ordered the certificate.
//!
//! - **Looked up only while a challenge is in progress.** An instance with a challenge under way
//!   keeps a marker, `acme/<CA>/challenges-in-progress`, holding when it runs out: two minutes
//!   on, rewritten every 30s while the challenge lasts, and left to run out after. The others
//!   look a challenge up only while the marker is good, reading it at most once a second; when no
//!   instance has an order under way, a request for a challenge gets its 404, and a handshake no
//!   certificate, without a lookup. The marker's time is compared with each instance's clock.
//! - **Bounded** (rules 1 and 11): a lookup never runs on an I/O thread. The request's or
//!   handshake's callback is answered from a thread of the lookup's own, at most
//!   [`MAX_SHARED_LOOKUPS`] at once for both challenge types, each within five seconds; past
//!   them it is answered at once, as one with no challenge. One client, an IPv4 address or an
//!   IPv6 /64, takes at most [`MAX_CLIENT_LOOKUPS`] HTTP-01 lookups; a name looked up in vain is
//!   not looked up again for a second (up to [`MAX_MISSES`] names remembered).
//! - **Settled.** The ordering instance waits 1.25s after keeping an answer before it asks the
//!   CA to check it, past what any instance read before the answer was there. Tokens and names
//!   that could make other keys are refused before any lookup.

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use akio::tls::{Certificate, CertifiedKey, PrivateKey};

use super::{Acme, after, lock};
use crate::storage::StorageError;

/// The most lookups of shared challenges, HTTP-01 and TLS-ALPN-01 together, under way at once;
/// one past it is answered as one with no challenge, at once (fit's `maxSharedLookups`).
pub const MAX_SHARED_LOOKUPS: usize = 16;
/// The most shared HTTP-01 lookups under way for one client, an IPv4 address or an IPv6 /64
/// (fit's `maxClientLookups`), as near as [`CLIENT_SLOTS`] counters tell clients apart.
pub const MAX_CLIENT_LOOKUPS: usize = 4;
const CLIENT_SLOTS: usize = 64;
/// The most names remembered with no TLS-ALPN-01 challenge found (fit's `maxMisses`).
pub const MAX_MISSES: usize = 1024;
/// How long the marker of challenges in progress is good for once written (fit's `markerTTL`).
const MARKER_TTL: Duration = Duration::from_secs(120);
/// How often it is rewritten while a challenge is in progress (fit's `markerRefresh`).
const MARKER_REFRESH: Duration = Duration::from_secs(30);
/// What bounds a lookup of a shared challenge's answer (fit's `sharedLookupTimeout`).
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);
/// How long an instance trusts what it last read of the marker (fit's `gateFresh`), and how
/// long a name found with no TLS-ALPN-01 challenge is not looked up again (fit's `missFor`).
const GATE_FRESH: Duration = Duration::from_secs(1);
const MISS_FOR: Duration = Duration::from_secs(1);
/// How long a challenge's answer is given to leave the storage once it is done.
const UNSHARE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long an instance waits, once it keeps a challenge's answer in the shared storage, before
/// it asks the CA to check it: past [`GATE_FRESH`] and [`MISS_FOR`], so that no instance still
/// holds what it read before the answer was there.
pub(crate) fn settle() -> Duration {
    GATE_FRESH
        .max(MISS_FOR)
        .saturating_add(Duration::from_millis(250))
}

/// The states of the gate, as of a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateState {
    /// No challenge in progress, as read lately.
    Closed,
    /// A challenge may be in progress.
    Open,
    /// Not read lately: read it before looking up.
    Stale,
}

/// What an ACME with a shared storage knows of the challenges other instances have in progress,
/// and what bounds its lookups of them (fit's `sharedGate`).
pub(crate) struct Gate {
    /// The marker's expiry as last read, and when that read started, in Unix nanoseconds;
    /// `checked` is 0 before the first.
    until: AtomicI64,
    checked: AtomicI64,
    /// A read of the marker is under way.
    reading: AtomicBool,
    /// The lookups under way.
    lookups: AtomicUsize,
    seed: RandomState,
    /// The shared HTTP-01 lookups under way, per client counter.
    clients: Vec<AtomicUsize>,
    /// The names a lookup found no TLS-ALPN-01 challenge for, with when that lookup started.
    misses: Mutex<HashMap<String, i64>>,
    /// Added to the clock, in nanoseconds: what tests move it with.
    offset: AtomicI64,
}

impl Gate {
    pub(crate) fn new() -> Gate {
        Gate {
            until: AtomicI64::new(0),
            checked: AtomicI64::new(0),
            reading: AtomicBool::new(false),
            lookups: AtomicUsize::new(0),
            seed: RandomState::new(),
            clients: (0..CLIENT_SLOTS).map(|_| AtomicUsize::new(0)).collect(),
            misses: Mutex::new(HashMap::new()),
            offset: AtomicI64::new(0),
        }
    }

    fn state(&self, now: i64) -> GateState {
        if now < self.until.load(Ordering::Acquire) {
            return GateState::Open;
        }
        let c = self.checked.load(Ordering::Acquire);
        if c != 0 && now.saturating_sub(c) < nanos(GATE_FRESH) {
            return GateState::Closed;
        }
        GateState::Stale
    }

    /// Takes a lookup's slot, unless every one is taken.
    fn take_slot(&self) -> bool {
        update(&self.lookups, |n| {
            (n < MAX_SHARED_LOOKUPS).then_some(n.saturating_add(1))
        })
    }

    fn free_slot(&self) {
        update(&self.lookups, |n| n.checked_sub(1));
    }
}

fn nanos(d: Duration) -> i64 {
    i64::try_from(d.as_nanos()).unwrap_or(i64::MAX)
}

/// Whether `token` is what an HTTP-01 token looks like: base64url, as RFC 8555 has it, so that
/// a request's path makes no other storage key.
fn token_ok(token: &str) -> bool {
    (1..=256).contains(&token.len())
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Whether `name` makes no other storage key (fit's `checkName`): lower-case letters, digits,
/// `-` and `.`, at most 253 of them.
pub(crate) fn name_ok(name: &str) -> bool {
    (1..=253).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
}

/// A challenge's certificate as kept in the storage, its chain and key in one PEM, read back.
pub(crate) fn parse_alpn(data: &[u8]) -> Result<CertifiedKey, String> {
    let chain = Certificate::from_pem(data).map_err(|e| e.to_string())?;
    let keys = akio_crypto::pem::decode_label(data, "PRIVATE KEY").map_err(|e| e.to_string())?;
    let key = keys.into_iter().next().ok_or("no private key")?;
    CertifiedKey::new(chain, PrivateKey::from_pkcs8_der(key)).map_err(|e| e.to_string())
}

/// The marker of challenges in progress kept good while it lives: its thread rewrites it every
/// [`MARKER_REFRESH`] until it is dropped, or the challenge's deadline passes.
pub(crate) struct Marker {
    stop: Option<SyncSender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Marker {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A challenge's answer kept in the shared storage, removed when it is dropped.
pub(crate) struct SharedAnswer<'a> {
    acme: &'a Acme,
    key: String,
}

impl Drop for SharedAnswer<'_> {
    fn drop(&mut self) {
        // A time limit of its own: the challenge's deadline may have passed.
        if let Err(e) = self
            .acme
            .inner
            .storage
            .delete(&self.key, after(UNSHARE_TIMEOUT))
        {
            self.acme.log(&format!(
                "removing the challenge's answer from the shared storage: {e}"
            ));
        }
    }
}

/// What a lookup holds: its slot, and its client's, given back when it is dropped.
struct Slots {
    acme: Acme,
    client: Option<usize>,
}

/// Sets `a` to what `f` makes of it, unless `f` makes nothing; whether it did. As
/// `fetch_update`, which newer Rust deprecates for `try_update`, a name the oldest Rust akit
/// builds with (its `rust-version`) does not have.
fn update(a: &AtomicUsize, f: impl Fn(usize) -> Option<usize>) -> bool {
    let mut n = a.load(Ordering::Acquire);
    loop {
        let Some(m) = f(n) else { return false };
        match a.compare_exchange_weak(n, m, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(now) => n = now,
        }
    }
}

impl Drop for Slots {
    fn drop(&mut self) {
        let g = &self.acme.inner.gate;
        g.free_slot();
        if let Some(c) = self.client.and_then(|i| g.clients.get(i)) {
            update(c, |n| n.checked_sub(1));
        }
    }
}

impl Acme {
    /// Whether its storage is shared between hosts (`acme.shared_storage`), so that a challenge
    /// another instance answers may be found there.
    #[must_use]
    pub fn shared(&self) -> bool {
        self.inner.shared
    }

    /// Now, in Unix nanoseconds, as the gate reads its clock.
    fn now_nanos(&self) -> i64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX));
        now.saturating_add(self.inner.gate.offset.load(Ordering::Relaxed))
    }

    pub(crate) fn http_challenge_key(&self, token: &str) -> String {
        format!("challenges/{}/http-01/{token}", self.inner.ca_key)
    }

    pub(crate) fn alpn_challenge_key(&self, name: &str) -> String {
        format!("challenges/{}/tls-alpn-01/{name}", self.inner.ca_key)
    }

    /// The marker of challenges in progress, outside `challenges/`, which holds only challenges
    /// under way.
    pub(crate) fn marker_key(&self) -> String {
        format!("acme/{}/challenges-in-progress", self.inner.ca_key)
    }

    /// Keeps the marker of challenges in progress good until the [`Marker`] is dropped, or
    /// `deadline` passes: written now, and again every [`MARKER_REFRESH`].
    pub(crate) fn mark_in_progress(&self, deadline: Instant) -> Marker {
        self.write_marker(deadline);
        let (tx, rx) = mpsc::sync_channel::<()>(0);
        let a = self.clone();
        let thread = std::thread::Builder::new()
            .name("akit-acme-marker".into())
            .spawn(move || {
                loop {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() || a.inner.stop.load(Ordering::Acquire) {
                        return;
                    }
                    match rx.recv_timeout(MARKER_REFRESH.min(left)) {
                        Err(RecvTimeoutError::Timeout) if Instant::now() < deadline => {
                            a.write_marker(deadline);
                        }
                        _ => return,
                    }
                }
            });
        match thread {
            Ok(t) => Marker {
                stop: Some(tx),
                thread: Some(t),
            },
            Err(e) => {
                self.log(&format!(
                    "keeping the marker of challenges in progress: {e}"
                ));
                Marker {
                    stop: None,
                    thread: None,
                }
            }
        }
    }

    /// Writes the marker, good for [`MARKER_TTL`]. It is not removed once the challenge is done,
    /// since another instance may have one under way; it runs out.
    pub(crate) fn write_marker(&self, deadline: Instant) {
        let until = self.now_nanos().saturating_add(nanos(MARKER_TTL));
        if let Err(e) =
            self.inner
                .storage
                .store(&self.marker_key(), until.to_string().as_bytes(), deadline)
        {
            self.log(&format!(
                "keeping the marker of challenges in progress in the shared storage: {e}"
            ));
            return;
        }
        self.inner.gate.until.fetch_max(until, Ordering::AcqRel);
    }

    /// Reads the marker of challenges in progress, unless another read of it is under way, and
    /// says whether a challenge may be in progress: true too when another reads it, or it cannot
    /// be read, so that a challenge is looked up rather than missed.
    pub(crate) fn read_gate(&self, deadline: Instant) -> bool {
        let g = &self.inner.gate;
        if g.reading
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return true;
        }
        // Stamped with when the read started, so that what it finds is trusted no longer than
        // GATE_FRESH past a marker written meanwhile.
        let start = self.now_nanos();
        let r = self.inner.storage.load(&self.marker_key(), deadline);
        let open = match r {
            Err(StorageError::NotFound) => None,
            Err(e) => {
                self.log(&format!(
                    "reading the marker of challenges in progress from the shared storage: {e}"
                ));
                g.reading.store(false, Ordering::Release);
                return true;
            }
            Ok(data) => std::str::from_utf8(&data)
                .ok()
                .and_then(|s| s.parse::<i64>().ok()),
        };
        if let Some(until) = open {
            g.until.store(until, Ordering::Release);
        }
        g.checked.store(start, Ordering::Release);
        g.reading.store(false, Ordering::Release);
        start < g.until.load(Ordering::Acquire)
    }

    /// The counter of `client`'s shared HTTP-01 lookups: of its IPv4 address, or its IPv6 /64.
    pub(crate) fn client_slot(&self, client: IpAddr) -> usize {
        let b = match client {
            IpAddr::V4(v4) => v4.to_ipv6_mapped().octets(),
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => v4.to_ipv6_mapped().octets(),
                None => {
                    // An IPv6 client is its /64, which one host often has whole.
                    let mut o = v6.octets();
                    o.iter_mut().skip(8).for_each(|b| *b = 0);
                    o
                }
            },
        };
        let h = self.inner.gate.seed.hash_one(b);
        usize::try_from(h.checked_rem(CLIENT_SLOTS as u64).unwrap_or(0)).unwrap_or(0)
    }

    /// Whether a lookup that started less than [`MISS_FOR`] before `now` found no TLS-ALPN-01
    /// challenge for `name`.
    fn missed(&self, name: &str, now: i64) -> bool {
        lock(&self.inner.gate.misses)
            .get(name)
            .is_some_and(|at| now.saturating_sub(*at) < nanos(MISS_FOR))
    }

    /// Remembers that a lookup started at `at` found no TLS-ALPN-01 challenge for `name`.
    pub(crate) fn miss(&self, name: &str, at: i64) {
        let mut m = lock(&self.inner.gate.misses);
        if m.len() >= MAX_MISSES {
            m.clear();
        }
        m.insert(name.to_owned(), at);
    }

    /// Keeps a challenge's answer in the shared storage, until the guard is dropped. A failure is
    /// reported, not returned: this instance can still answer it.
    pub(crate) fn share_challenge(
        &self,
        key: String,
        value: &[u8],
        deadline: Instant,
    ) -> SharedAnswer<'_> {
        if let Err(e) = self.inner.storage.store(&key, value, deadline) {
            self.log(&format!(
                "keeping the challenge's answer in the shared storage: {e}"
            ));
        }
        SharedAnswer { acme: self, key }
    }

    /// Runs `f`, a lookup holding its slots, on a thread of its own; when the thread cannot be
    /// made, `f` is dropped, which gives its slots back, and `fail` is called.
    fn spawn_lookup(f: impl FnOnce() + Send + 'static, fail: impl FnOnce()) {
        let r = std::thread::Builder::new()
            .name("akit-acme-lookup".into())
            .spawn(f);
        if r.is_err() {
            fail();
        }
    }

    /// Looks the answer to the HTTP-01 challenge of `token` up in the shared storage, for a
    /// challenge another instance has under way, and calls `done` with it, or with `None` when
    /// there is none. `client` is the request's address. It never blocks: the lookup runs on a
    /// thread of its own, and `done` is called there, or at once when the token cannot be one, no
    /// instance has a challenge in progress, or every slot is taken ([`MAX_SHARED_LOOKUPS`] in
    /// all, [`MAX_CLIENT_LOOKUPS`] for `client`'s address), each lookup within five seconds.
    /// Call it only once [`http_challenge`](Self::http_challenge) found no answer, and only
    /// when [`shared`](Self::shared).
    pub fn lookup_http_challenge(
        &self,
        token: &str,
        client: Option<IpAddr>,
        done: Box<dyn FnOnce(Option<String>) + Send>,
    ) {
        if !self.inner.shared || !token_ok(token) {
            return done(None);
        }
        let g = &self.inner.gate;
        let state = g.state(self.now_nanos());
        if state == GateState::Closed {
            return done(None);
        }
        let slot = client.map(|c| self.client_slot(c));
        if let Some(c) = slot.and_then(|i| g.clients.get(i)) {
            if c.fetch_add(1, Ordering::AcqRel) >= MAX_CLIENT_LOOKUPS {
                c.fetch_sub(1, Ordering::AcqRel);
                return done(None);
            }
        }
        if !g.take_slot() {
            if let Some(c) = slot.and_then(|i| g.clients.get(i)) {
                c.fetch_sub(1, Ordering::AcqRel);
            }
            return done(None);
        }
        let slots = Slots {
            acme: self.clone(),
            client: slot,
        };
        let key = self.http_challenge_key(token);
        let done = Arc::new(Mutex::new(Some(done)));
        let d2 = Arc::clone(&done);
        Acme::spawn_lookup(
            move || {
                let a = slots.acme.clone();
                let r = (|| {
                    let deadline = after(LOOKUP_TIMEOUT);
                    if state == GateState::Stale && !a.read_gate(deadline) {
                        return Err(StorageError::NotFound);
                    }
                    a.inner.storage.load(&key, deadline)
                })();
                // The slots are free again before done runs, so that one whom done answers may
                // look up again at once.
                drop(slots);
                let answer = match r {
                    Ok(v) => Some(String::from_utf8_lossy(&v).into_owned()),
                    Err(StorageError::NotFound) => None,
                    Err(e) => {
                        a.log(&format!(
                            "looking up a challenge in the shared storage: {e}"
                        ));
                        None
                    }
                };
                if let Some(d) = lock(&d2).take() {
                    d(answer);
                }
            },
            move || {
                if let Some(d) = lock(&done).take() {
                    d(None);
                }
            },
        );
    }

    /// Looks up the certificate of a TLS-ALPN-01 challenge another instance has under way for
    /// `name` (normalized) in the shared storage, and calls `done` with it, or with `None`. It
    /// never blocks: as [`lookup_http_challenge`](Self::lookup_http_challenge), but answered at
    /// once too for a name looked up in vain within the last second.
    pub fn lookup_alpn_challenge(
        &self,
        name: &str,
        done: Box<dyn FnOnce(Option<CertifiedKey>) + Send>,
    ) {
        if !self.inner.shared || !name_ok(name) {
            return done(None);
        }
        let now = self.now_nanos();
        let state = self.inner.gate.state(now);
        if state == GateState::Closed || self.missed(name, now) {
            return done(None);
        }
        if !self.inner.gate.take_slot() {
            return done(None);
        }
        let slots = Slots {
            acme: self.clone(),
            client: None,
        };
        let (key, name) = (self.alpn_challenge_key(name), name.to_owned());
        let done = Arc::new(Mutex::new(Some(done)));
        let d2 = Arc::clone(&done);
        Acme::spawn_lookup(
            move || {
                let a = slots.acme.clone();
                let r = (|| {
                    let deadline = after(LOOKUP_TIMEOUT);
                    if state == GateState::Stale && !a.read_gate(deadline) {
                        return Ok(None);
                    }
                    a.inner.storage.load(&key, deadline).map(Some)
                })();
                drop(slots);
                let cert = match r {
                    Ok(None) => None,
                    Ok(Some(data)) => match parse_alpn(&data) {
                        Ok(c) => Some(c),
                        Err(e) => {
                            a.log(&format!("{name}: the shared challenge's certificate: {e}"));
                            None
                        }
                    },
                    Err(e) => {
                        // A failure too, so that a store in trouble is not asked again at once.
                        a.miss(&name, now);
                        if !matches!(e, StorageError::NotFound) {
                            a.log(&format!(
                                "looking up a challenge in the shared storage: {e}"
                            ));
                        }
                        None
                    }
                };
                if let Some(d) = lock(&d2).take() {
                    d(cert);
                }
            },
            move || {
                if let Some(d) = lock(&done).take() {
                    d(None);
                }
            },
        );
    }

    /// Waits [`settle`] before the CA is asked to check a shared challenge, until `deadline` or
    /// the manager stops.
    pub(crate) fn settle_shared(&self, deadline: Instant) -> Result<(), String> {
        let end = after(settle()).min(deadline);
        while Instant::now() < end {
            if self.inner.stop.load(Ordering::Acquire) {
                return Err("stopped".to_owned());
            }
            std::thread::sleep(
                end.saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(50)),
            );
        }
        if Instant::now() >= deadline {
            return Err("the order's deadline passed".to_owned());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
