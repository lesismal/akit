//! The console's logins (I89), as fit's package `login`: users and their bcrypt hashes, password
//! checks, lockouts by client address, a user's checks slowed after failures in a row, and
//! sessions kept by the SHA-256 of their tokens.
//!
//! - **Checks.** A name that is no user's is checked against a dummy hash, so that the time taken
//!   does not tell names apart; at most [`CHECKS`] run at once, the rest wait. A check takes
//!   bcrypt's time and blocks: it is made on the console's login pool, never on an I/O thread.
//! - **Lockouts.** A client address (an IPv6 one by its /64, [`key`]) that fails [`MAX_FAILURES`]
//!   times in [`WINDOW`] is refused for [`LOCKOUT`]; each further lockout before it has been quiet
//!   for [`WINDOW`] lasts twice as long, up to [`MAX_LOCKOUT`]. One check from an address runs at a
//!   time ([`Logins::admit`]), so that guesses sent at once cannot slip past the count. At most
//!   [`MAX_ADDRESSES`] are kept: filling the table never ends a lockout early, and while it is full
//!   of addresses locked out or being checked, a new one is refused until the first lockout ends.
//! - **Slowed users.** A user whose password fails [`USER_FAILURES`] times in a row, from any
//!   addresses, has its checks refused for [`USER_BACKOFF`], then twice as long after each further
//!   failure, up to [`MAX_USER_BACKOFF`]; the right password, or [`WINDOW`] without a failure, ends
//!   the count. Names that are no user's share one count.
//! - **Sessions.** 32 random bytes, of which only the SHA-256 is kept; ended after the idle time
//!   without a lookup, after the lifetime in all, at logout, or as the oldest when one more than
//!   the most kept starts. Each has a CSRF token, compared in constant time.

use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::{Duration, Instant};

use akio_crypto::base64::URL_SAFE_NO_PAD;
use akio_crypto::bcrypt;
use akio_crypto::hash::Sha256;

/// How many failed checks from an address, within [`WINDOW`], lock it out.
pub const MAX_FAILURES: u32 = 5;
/// The window the failures of an address, or a user's in a row, are counted in.
pub const WINDOW: Duration = Duration::from_secs(15 * 60);
/// How long an address is first locked out.
pub const LOCKOUT: Duration = Duration::from_secs(15 * 60);
/// The longest lockout.
pub const MAX_LOCKOUT: Duration = Duration::from_secs(60 * 60);
/// The most client addresses whose failures are kept.
pub const MAX_ADDRESSES: usize = 4096;
/// How many password checks run at once.
pub const CHECKS: usize = 2;
/// How many failures in a row slow a user's checks.
pub const USER_FAILURES: u32 = 10;
/// The first wait of a slowed user.
pub const USER_BACKOFF: Duration = Duration::from_secs(2);
/// The longest wait of a slowed user.
pub const MAX_USER_BACKOFF: Duration = Duration::from_secs(5 * 60);
/// The prefix an IPv6 address is counted by.
pub const IPV6_PREFIX: u8 = 64;

/// The key of the failures of names that are no user's: one count for them all.
const UNKNOWN_USER: &str = "\0";

/// The clock's origin: times are kept as durations since a point far enough back that a time
/// never set (zero) is long past, as Go's zero time is.
const ORIGIN_OFFSET: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// What the logins are told by the config: `admin.console`'s users and limits.
#[derive(Debug, Clone)]
pub struct Options {
    /// The users and their bcrypt hashes.
    pub users: Vec<(String, String)>,
    /// How long a session lasts without a request.
    pub idle: Duration,
    /// How long a session lasts at most.
    pub lifetime: Duration,
    /// The most sessions kept; one more ends the oldest.
    pub max_sessions: usize,
}

impl Options {
    /// The options of `admin.console`.
    #[must_use]
    pub fn of(c: &akit_config::Console) -> Options {
        Options {
            users: c
                .users
                .iter()
                .map(|u| (u.username.clone(), u.password_hash.clone()))
                .collect(),
            idle: c.session_timeout,
            lifetime: c.session_lifetime,
            max_sessions: c.max_sessions,
        }
    }
}

/// A session, as a lookup gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// Who logged in.
    pub user: String,
    /// The CSRF token every request but a read carries.
    pub csrf: String,
    created: Duration,
    seen: Duration,
}

/// The console's logins: its users, the failures of client addresses and users, and the
/// sessions.
pub struct Logins {
    users: RwLock<BTreeMap<String, String>>,
    dummy: String,
    idle: Duration,
    lifetime: Duration,
    max: usize,
    clock: Clock,
    checks: (Mutex<usize>, Condvar),
    state: Mutex<State>,
}

impl std::fmt::Debug for Logins {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Logins")
            .field("users", &self.users())
            .finish_non_exhaustive()
    }
}

/// The time now, as a duration since the clock's origin.
type Clock = Box<dyn Fn() -> Duration + Send + Sync>;

#[derive(Default)]
struct State {
    /// By the SHA-256 of their tokens.
    sessions: HashMap<[u8; 32], Session>,
    /// By [`key`] of the client address, at most [`MAX_ADDRESSES`].
    fails: HashMap<String, Failures>,
    /// By user name, or [`UNKNOWN_USER`].
    user_fails: HashMap<String, UserFailures>,
    /// Until when a full table of lockouts refuses new addresses.
    no_room_until: Duration,
}

#[derive(Default)]
struct Failures {
    n: u32,
    first: Duration,
    last: Duration,
    until: Duration,
    lockouts: u32,
    busy: bool,
}

impl Failures {
    /// Whether the failures are over: no lockout now, and quiet for a window since the last
    /// failure and since the lockout ended.
    fn over(&self, now: Duration) -> bool {
        now >= self.until
            && now.saturating_sub(self.last) > WINDOW
            && now.saturating_sub(self.until) > WINDOW
    }
}

#[derive(Default)]
struct UserFailures {
    n: u32,
    last: Duration,
    until: Duration,
}

/// `base` doubled `n` times, at most `max`.
fn doubled(base: Duration, n: u32, max: Duration) -> Duration {
    base.checked_mul(1u32 << n.min(16))
        .map_or(max, |d| d.min(max))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `n` random bytes, base64url without padding. The system's randomness does not fail where
/// akit runs; if it did, the bytes would be the time's, which a session cannot be: a login
/// refuses then rather than start one ([`Logins::start`] checks).
fn random(n: usize) -> Option<String> {
    let mut b = vec![0u8; n];
    akio_crypto::rand::fill_random(&mut b).ok()?;
    Some(URL_SAFE_NO_PAD.encode(&b))
}

/// Whether `a` and `b` are equal, compared in time that depends on their lengths only.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl Logins {
    /// The logins of `o`'s users. Hashes the dummy password names that are no user's are checked
    /// against, at the users' cost: bcrypt's time, on the control plane.
    #[must_use]
    pub fn new(o: &Options) -> Logins {
        let origin = Instant::now();
        Logins::with_clock(
            o,
            Box::new(move || origin.elapsed().saturating_add(ORIGIN_OFFSET)),
        )
    }

    /// The logins of `o`'s users, with `clock` as the time: for tests.
    #[must_use]
    pub fn with_clock(o: &Options, clock: Clock) -> Logins {
        let mut cost = bcrypt::DEFAULT_COST;
        for (_, h) in &o.users {
            if let Ok(p) = bcrypt::Hash::parse(h) {
                cost = p.cost();
            }
        }
        let dummy = random(24)
            .and_then(|pw| bcrypt::hash(pw.as_bytes(), cost).ok())
            .unwrap_or_default();
        Logins {
            users: RwLock::new(o.users.iter().cloned().collect()),
            dummy,
            idle: o.idle,
            lifetime: o.lifetime,
            max: o.max_sessions.max(1),
            clock,
            checks: (Mutex::new(0), Condvar::new()),
            state: Mutex::new(State::default()),
        }
    }

    fn now(&self) -> Duration {
        (self.clock)()
    }

    /// Whether `password` is `user`'s. Blocks: bcrypt's time, and a wait while [`CHECKS`] others
    /// run. A name that is no user's is checked against the dummy hash, and fails.
    #[must_use]
    pub fn check(&self, user: &str, password: &str) -> bool {
        let (slots, freed) = &self.checks;
        {
            let mut n = lock(slots);
            while *n >= CHECKS {
                n = freed.wait(n).unwrap_or_else(PoisonError::into_inner);
            }
            *n = n.saturating_add(1);
        }
        let hash = self
            .users
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(user)
            .cloned();
        let known = hash.is_some();
        let hash = hash.unwrap_or_else(|| self.dummy.clone());
        let ok = bcrypt::Hash::parse(&hash).is_ok_and(|h| h.verify(password.as_bytes()));
        {
            let mut n = lock(slots);
            *n = n.saturating_sub(1);
        }
        freed.notify_one();
        ok && known
    }

    /// The users who may log in, by name.
    #[must_use]
    pub fn users(&self) -> Vec<String> {
        self.users
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }

    /// Whether `user` may log in.
    #[must_use]
    pub fn has_user(&self, user: &str) -> bool {
        self.users
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(user)
    }

    /// Sets `user`'s password hash, adding the user when it is not one: checks use it from now on.
    pub fn set_password(&self, user: &str, hash: &str) {
        self.users
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(user.to_owned(), hash.to_owned());
    }

    /// Starts a session of `user`: its token, for the cookie, and the session. The oldest goes
    /// when the most are kept. `None` when the system gives no randomness.
    #[must_use]
    pub fn start(&self, user: &str) -> Option<(String, Session)> {
        let token = random(32)?;
        let csrf = random(24)?;
        let now = self.now();
        let s = Session {
            user: user.to_owned(),
            csrf,
            created: now,
            seen: now,
        };
        let mut st = lock(&self.state);
        while st.sessions.len() >= self.max {
            let oldest = st
                .sessions
                .iter()
                .min_by_key(|(_, v)| v.created)
                .map(|(k, _)| *k);
            match oldest {
                Some(k) => st.sessions.remove(&k),
                None => break,
            };
        }
        st.sessions
            .insert(Sha256::digest(token.as_bytes()), s.clone());
        Some((token, s))
    }

    /// The session of `token`, when it has not ended: its idle time starts again.
    #[must_use]
    pub fn lookup(&self, token: &str) -> Option<Session> {
        if token.is_empty() {
            return None;
        }
        let key = Sha256::digest(token.as_bytes());
        let now = self.now();
        let mut st = lock(&self.state);
        let s = st.sessions.get_mut(&key)?;
        if now.saturating_sub(s.seen) > self.idle || now.saturating_sub(s.created) > self.lifetime {
            st.sessions.remove(&key);
            return None;
        }
        s.seen = now;
        Some(s.clone())
    }

    /// Ends the session of `token`, returning it.
    pub fn end(&self, token: &str) -> Option<Session> {
        lock(&self.state)
            .sessions
            .remove(&Sha256::digest(token.as_bytes()))
    }

    /// Ends `user`'s sessions but the one of `keep` (none kept when it is empty), returning how
    /// many ended: another user's password set, or one's own changed, ends them elsewhere.
    pub fn end_user_sessions(&self, user: &str, keep: &str) -> usize {
        let keep = (!keep.is_empty()).then(|| Sha256::digest(keep.as_bytes()));
        let mut st = lock(&self.state);
        let before = st.sessions.len();
        st.sessions
            .retain(|k, s| s.user != user || keep.as_ref() == Some(k));
        before.saturating_sub(st.sessions.len())
    }

    /// Admits a check from `addr`, as [`Logins::admit_user`] does without a user.
    ///
    /// # Errors
    ///
    /// [`Refusal`] when the address is locked out, has a check under way, or the table is full.
    pub fn admit(self: &Arc<Self>, addr: &str) -> Result<Ticket, Refusal> {
        let key = key(addr);
        let now = self.now();
        let mut guard = lock(&self.state);
        let st = &mut *guard;
        match st.fails.get_mut(&key) {
            None => {
                if st.fails.len() >= MAX_ADDRESSES && !make_room(st, now) {
                    return Err(Refusal {
                        wait: st
                            .no_room_until
                            .saturating_sub(now)
                            .max(Duration::from_secs(1)),
                        busy: false,
                        slowed: false,
                    });
                }
                st.fails.insert(
                    key.clone(),
                    Failures {
                        busy: true,
                        ..Failures::default()
                    },
                );
            }
            Some(f) => {
                if f.until > now {
                    return Err(Refusal {
                        wait: f.until.saturating_sub(now),
                        busy: false,
                        slowed: false,
                    });
                }
                if f.busy {
                    return Err(Refusal {
                        wait: Duration::from_secs(1),
                        busy: true,
                        slowed: false,
                    });
                }
                f.busy = true;
            }
        }
        Ok(Ticket {
            l: Arc::clone(self),
            key,
            user: None,
            done: false,
        })
    }

    /// Admits a check of `user`'s password from `addr`: one from an address at a time, not
    /// while it is locked out, and not while the user's checks are slowed. The ticket says how
    /// the check went when it ends; dropped, it counts nothing.
    ///
    /// # Errors
    ///
    /// [`Refusal`]: what to answer, with how long to wait.
    pub fn admit_user(self: &Arc<Self>, addr: &str, user: &str) -> Result<Ticket, Refusal> {
        let ukey = if self.has_user(user) {
            user
        } else {
            UNKNOWN_USER
        };
        let mut t = self.admit(addr)?;
        t.user = Some(ukey.to_owned());
        let wait = t.user_wait();
        if !wait.is_zero() {
            t.done(false);
            return Err(Refusal {
                wait,
                busy: false,
                slowed: true,
            });
        }
        Ok(t)
    }

    /// How long `addr` is still locked out (zero when it is not).
    #[must_use]
    pub fn locked_out(&self, addr: &str) -> Duration {
        let now = self.now();
        lock(&self.state)
            .fails
            .get(&key(addr))
            .map_or(Duration::ZERO, |f| f.until.saturating_sub(now))
    }

    /// Counts a failure from `addr` outside a check: a wrong current password in a password
    /// change.
    pub fn failed(self: &Arc<Self>, addr: &str) {
        if let Ok(t) = self.admit(addr) {
            t.done(true);
        }
    }

    /// How many client addresses' failures are kept.
    #[must_use]
    pub fn addresses(&self) -> usize {
        lock(&self.state).fails.len()
    }
}

/// Makes room for a new address in a full table: forgets the failures that are over, else the
/// oldest that is neither locked out nor being checked. When none can go, records until when
/// new addresses are refused and returns false.
fn make_room(st: &mut State, now: Duration) -> bool {
    if now < st.no_room_until {
        return false;
    }
    st.fails.retain(|_, f| f.busy || !f.over(now));
    if st.fails.len() < MAX_ADDRESSES {
        return true;
    }
    let oldest = st
        .fails
        .iter()
        .filter(|(_, f)| !f.busy && now >= f.until)
        .min_by_key(|(_, f)| f.first)
        .map(|(k, _)| k.clone());
    if let Some(k) = oldest {
        st.fails.remove(&k);
        return true;
    }
    let until = st
        .fails
        .values()
        .filter(|f| !f.busy && now < f.until)
        .map(|f| f.until)
        .min()
        // Every one is being checked.
        .unwrap_or(now.saturating_add(Duration::from_secs(1)));
    st.no_room_until = until;
    false
}

/// A login refused before its check: how long to wait, and why: `busy`, another check from the
/// address is under way; `slowed`, the user has failed too often in a row; neither, the address
/// is locked out (or the table is full of those that are).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refusal {
    /// How long to wait before trying again.
    pub wait: Duration,
    /// Another check from the address is under way.
    pub busy: bool,
    /// The user's checks are slowed.
    pub slowed: bool,
}

/// A check admitted: it ends with [`Ticket::done`] or [`Ticket::passed`], once; dropped first, it
/// counts nothing.
pub struct Ticket {
    l: Arc<Logins>,
    key: String,
    user: Option<String>,
    done: bool,
}

impl std::fmt::Debug for Ticket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ticket")
            .field("key", &self.key)
            .field("done", &self.done)
            .finish_non_exhaustive()
    }
}

impl Ticket {
    fn user_wait(&self) -> Duration {
        let Some(u) = &self.user else {
            return Duration::ZERO;
        };
        let now = self.l.now();
        lock(&self.l.state)
            .user_fails
            .get(u)
            .map_or(Duration::ZERO, |f| f.until.saturating_sub(now))
    }

    /// How long the check must still wait, its address locked out or its user slowed meanwhile
    /// (a check that waited in a queue): zero when it may run.
    #[must_use]
    pub fn locked_out(&self) -> Duration {
        let now = self.l.now();
        let wait = lock(&self.l.state)
            .fails
            .get(&self.key)
            .map_or(Duration::ZERO, |f| f.until.saturating_sub(now));
        wait.max(self.user_wait())
    }

    /// Ends the check: `failed` counts a failure against the address and the user.
    pub fn done(mut self, failed: bool) {
        self.end(failed, false);
    }

    /// Ends the check, which passed: the user's count of failures ends.
    pub fn passed(mut self) {
        self.end(false, true);
    }

    fn end(&mut self, failed: bool, passed: bool) {
        if self.done {
            return;
        }
        self.done = true;
        let now = self.l.now();
        let mut guard = lock(&self.l.state);
        let st = &mut *guard;
        st.no_room_until = Duration::ZERO;
        let Some(f) = st.fails.get_mut(&self.key) else {
            return;
        };
        f.busy = false;
        if failed {
            count(f, now);
            if let Some(u) = &self.user {
                count_user(st.user_fails.entry(u.clone()).or_default(), now);
            }
            return;
        }
        if f.n == 0 && f.lockouts == 0 {
            st.fails.remove(&self.key);
        }
        if passed {
            if let Some(u) = &self.user {
                st.user_fails.remove(u);
            }
        }
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.end(false, false);
    }
}

/// Counts a failure of an address: past [`MAX_FAILURES`] in the window, a lockout, longer for
/// each one before the address has been quiet.
fn count(f: &mut Failures, now: Duration) {
    if f.over(now) {
        f.n = 0;
        f.lockouts = 0;
        f.first = now;
    } else if f.n == 0 || (now.saturating_sub(f.first) > WINDOW && now >= f.until) {
        f.n = 0;
        f.first = now;
    }
    f.last = now;
    f.n = f.n.saturating_add(1);
    if f.n >= MAX_FAILURES {
        f.until = now.saturating_add(doubled(LOCKOUT, f.lockouts, MAX_LOCKOUT));
        f.lockouts = f.lockouts.saturating_add(1);
        f.n = 0;
    }
}

/// Counts a user's failure in a row: past [`USER_FAILURES`], its checks wait.
fn count_user(u: &mut UserFailures, now: Duration) {
    if now >= u.until && now.saturating_sub(u.last) > WINDOW {
        u.n = 0;
    }
    u.n = u.n.saturating_add(1);
    u.last = now;
    if u.n >= USER_FAILURES {
        u.until = now.saturating_add(doubled(
            USER_BACKOFF,
            u.n.saturating_sub(USER_FAILURES),
            MAX_USER_BACKOFF,
        ));
    }
}

/// The key failures are counted by: an IPv4 address (an IPv4-mapped IPv6 one as IPv4), an IPv6
/// address's /64 prefix, its zone left out, or the text as it is when it is not an address.
#[must_use]
pub fn key(addr: &str) -> String {
    let bare = addr.split('%').next().unwrap_or(addr);
    let Ok(ip) = bare.parse::<IpAddr>() else {
        return addr.to_owned();
    };
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    };
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            let bits = u128::from(v6)
                & u128::MAX
                    .checked_shl(128u32.saturating_sub(u32::from(IPV6_PREFIX)))
                    .unwrap_or(0);
            format!("{}/{IPV6_PREFIX}", std::net::Ipv6Addr::from(bits))
        }
    }
}

/// Whether `got` is `s`'s CSRF token, compared in constant time.
#[must_use]
pub fn csrf_ok(s: &Session, got: &str) -> bool {
    same(got.as_bytes(), s.csrf.as_bytes())
}

/// The session cookie's name: `name`, then the console's port, so that consoles of several akits
/// (or fits) on one host keep their logins apart; `name` alone without a port.
#[must_use]
pub fn cookie_name(name: &str, port: u16) -> String {
    if port == 0 {
        name.to_owned()
    } else {
        format!("{name}_{port}")
    }
}

#[cfg(test)]
mod tests;
