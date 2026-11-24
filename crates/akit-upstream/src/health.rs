//! Passive health checks (I20): an address out of rotation after `max_fails` failed requests
//! within `fail_timeout`, for `fail_timeout`, as fit's (nginx's `max_fails` and `fail_timeout`).
//! Active ones (I35, [`crate::probe`]) take it out on their own account, until its probes pass
//! again ([`Health::set_probed_down`]): it is in rotation only while neither keeps it out.
//!
//! The state is the address's, shared by every I/O thread, as fit's is: a failure seen on one
//! thread takes the address out for all of them. What a pick reads is one atomic, and the clock
//! only while it is set; a success reads one more atomic and writes nothing while the address has
//! no failure counted. Only a failure, and the first success after failures, take the address's
//! own lock (rules.md rule 13 forbids global locks per request: this one is per address, and off
//! the path of requests that succeed).
//!
//! Times are nanoseconds on a clock the caller keeps (a pool counts from when it was made),
//! offset by one so that 0 means "never": [`nanos`] makes them.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// A time on a health clock: `elapsed` since the clock's start, in nanoseconds, plus one, so that
/// it is never 0.
#[must_use]
pub fn nanos(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_nanos())
        .unwrap_or(u64::MAX)
        .saturating_add(1)
}

fn span(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

/// One address's health, by its failures.
#[derive(Debug, Default)]
pub struct Health {
    /// When the address may take requests again: `u64::MAX` while its probes keep it out, else
    /// when its failures stop keeping it out; 0 while it is in rotation. Picks read only it.
    down_until: AtomicU64,
    /// Whether its probes took it out of rotation; written under `window`'s lock.
    probed_down: AtomicBool,
    /// Whether failures may be counted in `window`, so that a success need not take the lock to
    /// find none are.
    failing: AtomicBool,
    window: Mutex<Window>,
}

#[derive(Debug, Default)]
struct Window {
    /// Failures counted in the window.
    fails: usize,
    /// When the failures counted so far stop counting.
    end: u64,
    /// When an address its failures took out may take requests again, or 0.
    passive_until: u64,
}

impl Health {
    /// An address in rotation, with no failure counted.
    #[must_use]
    pub fn new() -> Health {
        Health::default()
    }

    /// Whether the address may take a request; `now` gives the time, called only when the
    /// address has been out of rotation.
    pub fn up(&self, now: impl FnOnce() -> u64) -> bool {
        let d = self.down_until.load(Ordering::Acquire);
        d == 0 || now() >= d
    }

    /// When the address may take requests again, or `None` while it is in rotation (a time past
    /// may be given: it is back then).
    #[must_use]
    pub fn down_until(&self) -> Option<u64> {
        match self.down_until.load(Ordering::Acquire) {
            0 => None,
            d => Some(d),
        }
    }

    /// Counts a failed request. `max_fails` of them within `window` take the address out of
    /// rotation for `window`, from the last of them; `max_fails` 0 never does. `now` gives the
    /// time.
    pub fn failed(&self, max_fails: usize, window: Duration, now: impl FnOnce() -> u64) {
        if max_fails == 0 {
            return;
        }
        let now = now();
        let span = span(window);
        let mut w = self.window.lock().unwrap_or_else(PoisonError::into_inner);
        if now >= w.end {
            w.fails = 0;
            w.end = now.saturating_add(span);
        }
        w.fails = w.fails.saturating_add(1);
        self.failing.store(true, Ordering::Release);
        if w.fails >= max_fails {
            w.fails = 0;
            w.end = 0;
            w.passive_until = now.saturating_add(span).max(1);
            self.store_down(&w);
        }
    }

    /// Sets `down_until` from the passive and the active state; `w` is the held lock.
    fn store_down(&self, w: &Window) {
        let d = if self.probed_down.load(Ordering::Acquire) {
            u64::MAX
        } else {
            w.passive_until
        };
        self.down_until.store(d, Ordering::Release);
    }

    /// Whether the address's probes took it out of rotation.
    #[must_use]
    pub fn probed_down(&self) -> bool {
        self.probed_down.load(Ordering::Acquire)
    }

    /// Takes the address out of rotation (`true`) for its probes, until they put it back
    /// (`false`). `down_until` is stored before `probed_down`, so that whoever sees
    /// [`probed_down`](Self::probed_down) change sees the picks agree.
    pub fn set_probed_down(&self, down: bool) {
        let w = self.window.lock().unwrap_or_else(PoisonError::into_inner);
        let d = if down { u64::MAX } else { w.passive_until };
        self.down_until.store(d, Ordering::Release);
        self.probed_down.store(down, Ordering::Release);
    }

    /// Counts a request that got its response: it clears the failures, and puts an address that
    /// was out back in rotation, as fit does (one success is enough to show it has come back),
    /// unless its probes keep it out.
    pub fn succeeded(&self) {
        if !self.failing.load(Ordering::Acquire) && self.down_until.load(Ordering::Acquire) == 0 {
            return;
        }
        let mut w = self.window.lock().unwrap_or_else(PoisonError::into_inner);
        w.fails = 0;
        w.end = 0;
        w.passive_until = 0;
        self.failing.store(false, Ordering::Release);
        self.store_down(&w);
    }
}
