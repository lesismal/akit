//! Retries (I45), as fit's `handler/proxy/retry.go` makes them: a request is sent again, to the
//! address the upstream picks next, when the attempt before got no response or one of the
//! statuses the proxy's `retry` lists, and nothing was sent to the client yet:
//!
//! - A request whose connection could not be made (refused, unreachable, its connect timed out),
//!   or whose address had `max_pending` requests waiting already, never left: it is sent again
//!   whatever its method.
//! - One of an idempotent method (RFC 9110: GET, HEAD, OPTIONS, TRACE, PUT, DELETE) is sent again
//!   too when it failed before its response began, or was answered with a status of `statuses`,
//!   but not when the upstream's pool is closed.
//! - A request whose body streams (past [`STREAM_THRESHOLD`](crate::STREAM_THRESHOLD), of no
//!   declared length, or carrying trailers) is never sent again: what was sent of it is gone.
//!   One whose client went, or whose route's `timeout` passed, is not either.
//! - Retries are held to a budget ([`Budget`]), as Finagle's and Linkerd's are, so that they
//!   cannot multiply the load on an upstream that is failing already.
//!
//! Each attempt goes through akit-upstream's pool as the first did, from the pool's callback
//! for the attempt before, and its failure counts against its address as any failure does.

use std::net::IpAddr;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use akio_http::Method;
use akio_http::client::{ClientError, ClientRequest};
use akit_runtime::upstream::{Error, Keys, Pool};

/// A proxy's `retry`, compiled: fit's `retryPolicy`.
#[derive(Debug)]
pub(crate) struct Policy {
    attempts: u32,
    statuses: Vec<u16>,
    budget: Budget,
}

impl Policy {
    pub(crate) fn new(r: &akit_config::Retry) -> Policy {
        Policy {
            attempts: r.attempts,
            statuses: r.statuses.clone(),
            budget: Budget::new(r.budget_ratio, r.budget_min_per_second),
        }
    }

    /// Counts a request the proxy takes towards the budget.
    pub(crate) fn deposit(&self) {
        self.budget.deposit();
    }
}

/// The retry budget, as fit's `retryBudget`: each request adds `ratio` of a retry to it, it gains
/// `per_second` retries each second, and it holds at most `max`; a retry takes one, and none is
/// made without.
///
/// fit keeps it behind a mutex. Here the tokens are an `f64`'s bits in an atomic, so that a
/// request takes no lock (rule 13): a deposit into a full budget, as a healthy upstream's always
/// is, only reads it, and writes nothing to the line the other threads read; a withdrawal, which
/// only a failed attempt makes, adds what the time since the last one earned, under a small lock
/// of its own for that time.
#[derive(Debug)]
pub(crate) struct Budget {
    tokens: AtomicU64,
    ratio: f64,
    per_second: f64,
    max: f64,
    /// When the time earned was last added.
    last: Mutex<Instant>,
}

impl Budget {
    pub(crate) fn new(ratio: f64, per_second: f64) -> Budget {
        // It starts with a second's allowance, so that the first requests may be retried too.
        Budget {
            tokens: AtomicU64::new(per_second.to_bits()),
            ratio,
            per_second,
            max: (10.0 * per_second).max(10.0),
            last: Mutex::new(Instant::now()),
        }
    }

    /// Changes the tokens by `f` (given them, returning what they become, or `None` to leave
    /// them), and returns what `f` saw.
    fn update(&self, mut f: impl FnMut(f64) -> Option<f64>) -> f64 {
        let mut cur = self.tokens.load(Ordering::Relaxed);
        loop {
            let t = f64::from_bits(cur);
            let Some(next) = f(t) else { return t };
            match self.tokens.compare_exchange_weak(
                cur,
                next.to_bits(),
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return t,
                Err(now) => cur = now,
            }
        }
    }

    /// Counts a request.
    pub(crate) fn deposit(&self) {
        let (ratio, max) = (self.ratio, self.max);
        self.update(|t| (t < max && ratio > 0.0).then(|| (t + ratio).min(max)));
    }

    /// Takes a retry, and reports false when there is none.
    pub(crate) fn withdraw(&self) -> bool {
        self.withdraw_at(Instant::now())
    }

    fn withdraw_at(&self, now: Instant) -> bool {
        let earned = {
            let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
            let since = now.saturating_duration_since(*last);
            *last = (*last).max(now);
            since.as_secs_f64() * self.per_second
        };
        let max = self.max;
        let mut took = false;
        self.update(|t| {
            let t = (t + earned).min(max);
            took = t >= 1.0;
            Some(if took { t - 1.0 } else { t })
        });
        took
    }

    #[cfg(test)]
    fn tokens(&self) -> f64 {
        f64::from_bits(self.tokens.load(Ordering::Relaxed))
    }
}

/// Whether a request of `method` may be sent twice without the server acting on it twice, as
/// RFC 9110 says.
pub(crate) fn idempotent(method: &Method) -> bool {
    matches!(
        method,
        Method::Get | Method::Head | Method::Options | Method::Trace | Method::Put | Method::Delete
    )
}

/// Whether `e` says the request never left: its connection could not be made, or its address
/// had `max_pending` requests waiting already (fit's `notSent`).
pub(crate) fn not_sent(e: &Error) -> bool {
    use std::io::ErrorKind::{
        AddrNotAvailable, ConnectionRefused, HostUnreachable, NetworkUnreachable, NotFound,
        TimedOut,
    };
    match e {
        Error::Busy => true,
        // akio's client reports a connect (or its resolve) that failed as these; a connection
        // that broke later is reset, closed or at its end.
        Error::Client(ClientError::Io(io)) => matches!(
            io.kind(),
            ConnectionRefused
                | HostUnreachable
                | NetworkUnreachable
                | AddrNotAvailable
                | NotFound
                | TimedOut
        ),
        _ => false,
    }
}

/// What a request needs to be sent again: the policy, the request as it went the first time,
/// and what the address is picked by.
pub(crate) struct Again {
    pub(crate) policy: std::sync::Arc<Policy>,
    pub(crate) pool: Pool,
    /// The request as it was sent first: its target in origin form, its fields, its whole body
    /// and its cancel token, which serves every attempt.
    pub(crate) req: ClientRequest,
    pub(crate) idempotent: bool,
    /// What the address is picked by, as the first time: the client's address, the path and the
    /// query (the fields are the request's, `req.headers`).
    pub(crate) peer: IpAddr,
    pub(crate) path: String,
    pub(crate) query: String,
    /// When the route's `timeout` passes, if it has one.
    pub(crate) deadline: Option<Instant>,
    /// The attempts made after the first.
    pub(crate) attempt: u32,
    /// The addresses tried, by index (those past 63 are not told apart).
    pub(crate) tried: u64,
}

/// `backend`'s bit in [`Again::tried`].
pub(crate) fn bit(backend: usize) -> u64 {
    1u64.checked_shl(u32::try_from(backend).unwrap_or(u32::MAX))
        .unwrap_or(0)
}

/// The next attempt: where it goes and what is sent.
pub(crate) struct Next {
    pub(crate) backend: usize,
    pub(crate) req: ClientRequest,
    pub(crate) set_cookie: Option<String>,
}

impl Again {
    /// Whether the attempt that just ended, answered with `status` or failed with `err`, may be
    /// made again: within `attempts`, before the route's deadline, for its method and outcome,
    /// and with a retry in the budget (taken). `gone` says the client went.
    pub(crate) fn may(&self, status: Option<u16>, err: Option<&Error>, gone: bool) -> bool {
        if gone || self.attempt >= self.policy.attempts {
            return false;
        }
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            return false;
        }
        let again = match (status, err) {
            (_, Some(e)) => {
                not_sent(e)
                    || self.idempotent
                        && !matches!(
                            e,
                            Error::Closed
                                | Error::NoEngine
                                | Error::Client(
                                    ClientError::Cancelled | ClientError::InvalidUrl(_)
                                )
                        )
            }
            (Some(s), None) => self.idempotent && self.policy.statuses.contains(&s),
            (None, None) => false,
        };
        again && self.policy.budget.withdraw()
    }

    /// The next attempt: to the address the upstream picks next, with the sticky cookie for
    /// it, and what is left of the route's timeout. `None` when the deadline passed meanwhile.
    pub(crate) fn next(&mut self) -> Option<Next> {
        let mut req = self.req.clone();
        if let Some(d) = self.deadline {
            let left = d.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            req.timeout = Some(left);
        }
        self.attempt = self.attempt.saturating_add(1);
        let keys = Keys {
            peer: Some(self.peer),
            path: &self.path,
            query: &self.query,
            headers: Some(&self.req.headers),
        };
        // fit's turn is the upstream's, so its next pick is another address; akit's is each I/O
        // thread's, and the attempt before may have been picked on another thread. So an address
        // tried already is passed over while one is left that was not: the key's (sticky cookie,
        // hash) first, then the turn's.
        let mut backend = self.pool.pick_for(&keys);
        let n = self.pool.addresses().len();
        let mut turns = 0;
        while self.tried & bit(backend) != 0 && turns < n {
            backend = self.pool.pick();
            turns = turns.saturating_add(1);
        }
        self.tried |= bit(backend);
        let set_cookie = self.pool.set_cookie(&keys, backend).map(str::to_owned);
        Some(Next {
            backend,
            req,
            set_cookie,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn budget_ratio_and_rate() {
        // fit's TestRetryBudget, on the budget alone: a fifth of the requests, none a second
        // besides.
        let b = Budget::new(0.2, 0.0);
        let now = Instant::now();
        let mut retries = 0;
        for _ in 0..50 {
            b.deposit();
            if b.withdraw_at(now) {
                retries += 1;
            }
        }
        assert!((9..=10).contains(&retries), "{retries}");

        // A second's allowance at first, then per_second a second.
        let b = Budget::new(0.0, 2.0);
        let start = *b.last.lock().unwrap_or_else(PoisonError::into_inner);
        assert!(b.withdraw_at(start));
        assert!(b.withdraw_at(start));
        assert!(!b.withdraw_at(start));
        assert!(!b.withdraw_at(start + Duration::from_millis(400)));
        assert!(b.withdraw_at(start + Duration::from_millis(600)));
        // Never past max: 10 * per_second, at least 10.
        assert!(b.withdraw_at(start + Duration::from_secs(3600)));
        assert!((b.tokens() - 19.0).abs() < 1e-9, "{}", b.tokens());
    }

    #[test]
    fn deposits_stop_at_max() {
        let b = Budget::new(1.0, 0.0);
        for _ in 0..100 {
            b.deposit();
        }
        assert!((b.tokens() - 10.0).abs() < 1e-9, "{}", b.tokens());
    }

    #[test]
    fn methods_and_errors() {
        for m in [
            Method::Get,
            Method::Head,
            Method::Options,
            Method::Trace,
            Method::Put,
            Method::Delete,
        ] {
            assert!(idempotent(&m), "{m:?}");
        }
        for m in [
            Method::Post,
            Method::Patch,
            Method::Connect,
            Method::Other("PURGE".into()),
        ] {
            assert!(!idempotent(&m), "{m:?}");
        }
        let io = |k| Error::Client(ClientError::Io(std::io::Error::from(k)));
        assert!(not_sent(&Error::Busy));
        assert!(not_sent(&io(std::io::ErrorKind::ConnectionRefused)));
        assert!(not_sent(&io(std::io::ErrorKind::TimedOut)));
        assert!(!not_sent(&io(std::io::ErrorKind::ConnectionReset)));
        assert!(!not_sent(&Error::Client(ClientError::Closed)));
        assert!(!not_sent(&Error::Closed));
        assert!(!not_sent(&Error::Timeout));
    }
}
