//! A token bucket for each username and direction, holding a second's worth of the rate: fit's
//! `webrtc/turn/limiter.go`, `user_bandwidth` (I112).

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// The most buckets kept. A bucket exists only for a username that relayed lately, and a full
/// bucket is the same as none, so buckets that have refilled are dropped first; past the bound, a
/// username with no bucket has its datagrams dropped until one refills.
pub(crate) const MAX_BUCKETS: usize = 1 << 16;

/// How often the buckets that have refilled are dropped.
const SWEEP_EVERY: Duration = Duration::from_secs(10);

#[derive(Default)]
struct State {
    buckets: HashMap<(String, bool), Bucket>,
    last_sweep: Option<Instant>,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

/// The buckets of every username and direction.
#[derive(Default)]
pub(crate) struct Limiter(Mutex<State>);

#[allow(clippy::cast_precision_loss)]
fn f(n: u64) -> f64 {
    n as f64
}

impl Limiter {
    /// Takes `n` bytes from `user`'s bucket for the direction, at `rate` bytes a second, and
    /// says whether it held them.
    pub(crate) fn allow(
        &self,
        user: &str,
        to_peer: bool,
        n: usize,
        rate: u64,
        now: Instant,
    ) -> bool {
        let mut st = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let rate = f(rate);
        let n = f(u64::try_from(n).unwrap_or(u64::MAX));
        if st
            .last_sweep
            .is_none_or(|t| now.saturating_duration_since(t) >= SWEEP_EVERY)
        {
            sweep(&mut st, rate, now);
        }
        let key = (user.to_owned(), to_peer);
        if !st.buckets.contains_key(&key) {
            if st.buckets.len() >= MAX_BUCKETS {
                sweep(&mut st, rate, now);
                if st.buckets.len() >= MAX_BUCKETS {
                    return false;
                }
            }
            st.buckets.insert(
                key.clone(),
                Bucket {
                    tokens: rate,
                    last: now,
                },
            );
        }
        let Some(b) = st.buckets.get_mut(&key) else {
            return false;
        };
        let d = now.saturating_duration_since(b.last).as_secs_f64();
        if d > 0.0 {
            b.tokens = (b.tokens + d * rate).min(rate);
            b.last = now;
        }
        if b.tokens < n {
            return false;
        }
        b.tokens -= n;
        true
    }
}

/// Drops the buckets that have refilled by `now`.
fn sweep(st: &mut State, rate: f64, now: Instant) {
    st.last_sweep = Some(now);
    st.buckets.retain(|_, b| {
        b.tokens + now.saturating_duration_since(b.last).as_secs_f64() * rate < rate
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// fit's `TestLimiter`: a second's burst, then the rate; each username and direction alone;
    /// the buckets bounded.
    #[test]
    fn limiter() {
        let l = Limiter::default();
        let t0 = Instant::now();
        assert!(l.allow("a", true, 1000, 1000, t0));
        assert!(!l.allow("a", true, 1, 1000, t0));
        assert!(l.allow("a", false, 1000, 1000, t0));
        assert!(l.allow("b", true, 1000, 1000, t0));
        let t1 = t0 + Duration::from_millis(500);
        assert!(l.allow("a", true, 500, 1000, t1));
        assert!(!l.allow("a", true, 1, 1000, t1));
        // Refilled buckets go at the next sweep.
        let t2 = t0 + Duration::from_secs(20);
        assert!(l.allow("c", true, 1, 1000, t2));
        let n =
            l.0.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .buckets
                .len();
        assert_eq!(n, 1);
        // Past the bound, a username without a bucket is refused until one refills.
        let l = Limiter::default();
        for i in 0..MAX_BUCKETS {
            assert!(l.allow(&i.to_string(), true, 10, 10, t0));
        }
        assert!(!l.allow("new", true, 1, 10, t0));
        assert!(l.allow("new", true, 1, 10, t0 + Duration::from_secs(1)));
    }
}
