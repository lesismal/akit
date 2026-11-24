//! The `circuit_breaker` middleware (I45), as fit's `middleware/breaker`: it stops sending a
//! route's requests on while too many of them fail or are slow, and lets a few through after a
//! while to see whether what failed has recovered.
//!
//! - **Closed**, it lets requests through and counts, over the last `window` (in ten steps), the
//!   responses that were failures (5xx, or a status of `statuses`) and those slower than
//!   `slow_threshold` to begin. Once the window holds at least `min_requests` and the failures
//!   reach `error_ratio`, or the slow ones `slow_ratio`, it opens.
//! - **Open**, it answers every request at once, 503 with `Retry-After` the seconds until it
//!   tries again, for `open_duration`.
//! - **Half-open**, it lets `half_open_requests` through, answering the others 503: if they all
//!   fare well it closes, its window starting afresh, and if one fails it opens again. A probe
//!   whose client goes before its answer gives its place back.
//!
//! How a request fared is read by a response filter ([`Context::add_response_filter`]), which
//! sees the status of any response, whole, later (a proxied one) or streamed, when its head is
//! sent; a request that ends without one (its client gone) drops the filter unrun.
//!
//! fit keeps the breaker behind a mutex taken twice a request. Here a closed breaker takes no
//! lock (rule 13): the state is an atomic, and the window's ten steps are atomic counters, each
//! tagged with the step it counts, which the first request of a new step clears. Two requests
//! racing at a step's start may count one in the step before it is cleared: the window is a
//! sample either way. The lock is taken only to change the state, and by the few requests a
//! half-open breaker decides on.

use std::sync::atomic::{AtomicI64, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use akio_http::{BodyMode, Context, Request, Response};
use akit_runtime::{Handler, HandlerParams, Middleware, Provision, ProvisionError, Provisioned};

/// The window's steps, as fit's `buckets`.
const STEPS: usize = 10;

/// A second, in nanoseconds.
const SECOND: i64 = 1_000_000_000;

/// The breaker's state, as fit's `State`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum State {
    Closed = 0,
    Open = 1,
    HalfOpen = 2,
}

impl State {
    fn from(v: u8) -> State {
        match v {
            1 => State::Open,
            2 => State::HalfOpen,
            _ => State::Closed,
        }
    }

    #[cfg(test)]
    fn name(self) -> &'static str {
        match self {
            State::Closed => "closed",
            State::Open => "open",
            State::HalfOpen => "half-open",
        }
    }
}

/// One step of the window: the step it counts (its index from the clock's start, plus one; 0
/// for none), and its counts.
#[derive(Debug, Default)]
struct Step {
    tag: AtomicU64,
    requests: AtomicU64,
    failures: AtomicU64,
    slow: AtomicU64,
}

/// What a half-open breaker keeps, behind the lock.
#[derive(Debug, Default)]
struct Probes {
    /// Probes let through and not yet over.
    trying: u32,
    /// Probes that fared well.
    passed: u32,
}

/// The time, in nanoseconds since the breaker was compiled.
#[derive(Debug)]
enum Clock {
    Real(Instant),
    #[cfg(test)]
    Fake(Arc<AtomicI64>),
}

impl Clock {
    fn now(&self) -> i64 {
        match self {
            Clock::Real(start) => i64::try_from(start.elapsed().as_nanos()).unwrap_or(i64::MAX),
            #[cfg(test)]
            Clock::Fake(t) => t.load(Ordering::Relaxed),
        }
    }
}

/// Whether a request goes through, as fit's `admit` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admit {
    /// Through, counted in the window.
    Pass,
    /// Through, as one of a half-open breaker's probes.
    Probe,
    /// Refused, the breaker trying again in this many nanoseconds.
    Refuse(i64),
}

/// The breaker: the route's, shared by every request of it.
#[derive(Debug)]
pub(crate) struct Breaker {
    /// A step of the window, in nanoseconds: a tenth of it, at least 1.
    step: i64,
    min_requests: u64,
    error_ratio: f64,
    /// `statuses`, or empty for 500 to 599.
    statuses: Vec<u16>,
    /// `slow_threshold` in nanoseconds; 0 for none.
    slow: i64,
    slow_ratio: f64,
    /// `open_duration` in nanoseconds.
    open: i64,
    half_open: u32,
    clock: Clock,
    state: AtomicU8,
    /// When an open breaker turns half-open.
    until: AtomicI64,
    steps: [Step; STEPS],
    probes: Mutex<Probes>,
    #[cfg(test)]
    transitions: Mutex<Vec<String>>,
}

fn nanos(d: std::time::Duration) -> i64 {
    i64::try_from(d.as_nanos()).unwrap_or(i64::MAX)
}

impl Breaker {
    fn new(p: &akit_config::CircuitBreaker, clock: Clock) -> Breaker {
        Breaker {
            step: nanos(p.window)
                .checked_div(STEPS as i64)
                .unwrap_or(0)
                .max(1),
            min_requests: p.min_requests.max(1),
            error_ratio: p.error_ratio,
            statuses: p.statuses.clone(),
            slow: p.slow_threshold.map_or(0, nanos),
            slow_ratio: p.slow_ratio,
            open: nanos(p.open_duration),
            half_open: p.half_open_requests.max(1),
            clock,
            state: AtomicU8::new(State::Closed as u8),
            until: AtomicI64::new(0),
            steps: Default::default(),
            probes: Mutex::new(Probes::default()),
            #[cfg(test)]
            transitions: Mutex::new(Vec::new()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Probes> {
        self.probes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn load(&self) -> State {
        State::from(self.state.load(Ordering::Acquire))
    }

    /// Whether `status` is a failure.
    fn is_failure(&self, status: u16) -> bool {
        if self.statuses.is_empty() {
            status >= 500
        } else {
            self.statuses.contains(&status)
        }
    }

    /// The state now: an open breaker whose time is up turns half-open.
    pub(crate) fn state(&self, now: i64) -> State {
        let s = self.load();
        if s == State::Open && now >= self.until.load(Ordering::Acquire) {
            let mut p = self.lock();
            if self.load() == State::Open && now >= self.until.load(Ordering::Acquire) {
                *p = Probes::default();
                self.set(State::HalfOpen, now);
            }
        }
        self.load()
    }

    /// Moves the breaker to `to`. The lock is held.
    fn set(&self, to: State, now: i64) {
        #[cfg(test)]
        let from = self.load();
        match to {
            State::Open => self
                .until
                .store(now.saturating_add(self.open), Ordering::Release),
            State::Closed => {
                // The window starts afresh.
                for s in &self.steps {
                    s.tag.store(0, Ordering::Relaxed);
                }
            }
            State::HalfOpen => {}
        }
        self.state.store(to as u8, Ordering::Release);
        #[cfg(test)]
        self.transitions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("{}>{}", from.name(), to.name()));
    }

    /// Decides whether a request goes through.
    pub(crate) fn admit(&self, now: i64) -> Admit {
        match self.state(now) {
            State::Closed => Admit::Pass,
            State::Open => Admit::Refuse(self.until.load(Ordering::Acquire).saturating_sub(now)),
            State::HalfOpen => {
                let mut p = self.lock();
                match self.load() {
                    State::Closed => Admit::Pass,
                    State::Open => {
                        Admit::Refuse(self.until.load(Ordering::Acquire).saturating_sub(now))
                    }
                    State::HalfOpen => {
                        if p.trying.saturating_add(p.passed) >= self.half_open {
                            return Admit::Refuse(SECOND);
                        }
                        p.trying = p.trying.saturating_add(1);
                        Admit::Probe
                    }
                }
            }
        }
    }

    /// Counts how a request fared.
    pub(crate) fn record(&self, now: i64, probe: bool, failed: bool, slow: bool) {
        let s = self.state(now);
        if probe && s == State::HalfOpen {
            let mut p = self.lock();
            if self.load() == State::HalfOpen {
                p.trying = p.trying.saturating_sub(1);
                if failed || slow {
                    self.set(State::Open, now);
                    return;
                }
                p.passed = p.passed.saturating_add(1);
                if p.passed >= self.half_open {
                    self.set(State::Closed, now);
                }
                return;
            }
        }
        if self.load() != State::Closed {
            // From before it opened.
            return;
        }
        let index = now.max(0).checked_div(self.step).unwrap_or(0);
        let tag = u64::try_from(index).unwrap_or(0).saturating_add(1);
        let Some(step) = usize::try_from(index)
            .ok()
            .and_then(|i| self.steps.get(i % STEPS))
        else {
            return;
        };
        let old = step.tag.load(Ordering::Acquire);
        if old != tag
            && step
                .tag
                .compare_exchange(old, tag, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            step.requests.store(0, Ordering::Relaxed);
            step.failures.store(0, Ordering::Relaxed);
            step.slow.store(0, Ordering::Relaxed);
        }
        step.requests.fetch_add(1, Ordering::Relaxed);
        if failed {
            step.failures.fetch_add(1, Ordering::Relaxed);
        }
        if slow {
            step.slow.fetch_add(1, Ordering::Relaxed);
        }
        // The steps of the window: this one and the nine before it.
        let oldest = tag.saturating_sub(STEPS as u64).saturating_add(1);
        let (mut requests, mut failures, mut slows) = (0u64, 0u64, 0u64);
        for s in &self.steps {
            let t = s.tag.load(Ordering::Acquire);
            if t >= oldest && t <= tag && t != 0 {
                requests = requests.saturating_add(s.requests.load(Ordering::Relaxed));
                failures = failures.saturating_add(s.failures.load(Ordering::Relaxed));
                slows = slows.saturating_add(s.slow.load(Ordering::Relaxed));
            }
        }
        if requests < self.min_requests {
            return;
        }
        #[allow(clippy::cast_precision_loss)]
        let ratio = |k: u64| k as f64 / requests as f64;
        if ratio(failures) >= self.error_ratio || self.slow > 0 && ratio(slows) >= self.slow_ratio {
            let _p = self.lock();
            if self.load() == State::Closed {
                self.set(State::Open, now);
            }
        }
    }

    /// A probe whose client went before its answer: it says nothing of the upstream, but gives
    /// its place back.
    fn cancel(&self) {
        let mut p = self.lock();
        if self.load() == State::HalfOpen {
            p.trying = p.trying.saturating_sub(1);
        }
    }

    #[cfg(test)]
    fn counts(&self) -> Vec<(u64, u64, u64, u64)> {
        self.steps
            .iter()
            .map(|s| {
                (
                    s.tag.load(Ordering::Relaxed),
                    s.requests.load(Ordering::Relaxed),
                    s.failures.load(Ordering::Relaxed),
                    s.slow.load(Ordering::Relaxed),
                )
            })
            .collect()
    }
}

/// How one request fared, recorded once: when its response's head goes, or, dropped without,
/// as a request whose client went.
struct Outcome {
    breaker: Arc<Breaker>,
    probe: bool,
    start: i64,
    done: bool,
}

impl Outcome {
    fn header(mut self, status: u16) {
        self.done = true;
        let b = &self.breaker;
        let now = b.clock.now();
        let slow = b.slow > 0 && now.saturating_sub(self.start) > b.slow;
        b.record(now, self.probe, b.is_failure(status), slow);
    }
}

impl Drop for Outcome {
    fn drop(&mut self) {
        if !self.done && self.probe {
            self.breaker.cancel();
        }
    }
}

/// The answer to a request refused: 503, with when the breaker tries again, in whole seconds
/// rounded up.
fn refused(wait: i64) -> Response {
    let secs = wait.max(0).saturating_add(SECOND - 1) / SECOND;
    let mut r = Response::new(503);
    r.headers.set("Retry-After", secs.to_string());
    r
}

pub(crate) fn new(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Middleware>, ProvisionError> {
    let HandlerParams::CircuitBreaker(c) = p.params() else {
        return Err(ProvisionError::new(
            "the circuit_breaker middleware was given another's params",
        ));
    };
    Ok(Provisioned::new(
        Arc::new(CircuitBreaker(Arc::new(Breaker::new(
            c,
            Clock::Real(Instant::now()),
        )))) as Arc<dyn Middleware>,
    ))
}

/// The `circuit_breaker` middleware, compiled; the breaker is the route's.
struct CircuitBreaker(Arc<Breaker>);

impl Middleware for CircuitBreaker {
    fn wrap(&self, next: Arc<dyn Handler>) -> Arc<dyn Handler> {
        Arc::new(Broken {
            breaker: Arc::clone(&self.0),
            next,
        })
    }
}

struct Broken {
    breaker: Arc<Breaker>,
    next: Arc<dyn Handler>,
}

impl akio_http::Service for Broken {
    fn call(&self, ctx: &mut Context) {
        let b = &self.breaker;
        let start = b.clock.now();
        let probe = match b.admit(start) {
            Admit::Pass => false,
            Admit::Probe => true,
            Admit::Refuse(wait) => {
                ctx.respond(refused(wait));
                return;
            }
        };
        let outcome = Outcome {
            breaker: Arc::clone(b),
            probe,
            start,
            done: false,
        };
        ctx.add_response_filter(move |resp: &mut Response, _: &Request| {
            outcome.header(resp.status);
        });
        self.next.call(ctx);
    }

    fn body_mode(&self, req: &Request) -> BodyMode {
        self.next.body_mode(req)
    }

    fn streams_bodies(&self) -> bool {
        self.next.streams_bodies()
    }
}

impl Handler for Broken {}

#[cfg(test)]
mod tests {
    //! fit's `middleware/breaker` tests, on a breaker with a test clock.
    #![allow(clippy::unwrap_used, clippy::arithmetic_side_effects)]

    use std::time::Duration;

    use super::*;

    const S: i64 = SECOND;

    /// A breaker on a test clock, with what serving a request through it gives.
    struct Rig {
        b: Arc<Breaker>,
        now: Arc<AtomicI64>,
        /// How long the handler takes to answer.
        takes: i64,
    }

    impl Rig {
        fn new(p: akit_config::CircuitBreaker) -> Rig {
            let now = Arc::new(AtomicI64::new(1_000_000 * S));
            Rig {
                b: Arc::new(Breaker::new(&p, Clock::Fake(now.clone()))),
                now,
                takes: 0,
            }
        }

        fn advance(&self, d: i64) {
            self.now.fetch_add(d, Ordering::Relaxed);
        }

        /// One request answered `status` by the handler, through the breaker: the status the
        /// client gets and the `Retry-After` of a refusal.
        fn one(&self, status: u16) -> (u16, Option<String>) {
            let b = &self.b;
            let start = b.clock.now();
            let probe = match b.admit(start) {
                Admit::Pass => false,
                Admit::Probe => true,
                Admit::Refuse(wait) => {
                    let r = refused(wait);
                    return (r.status, r.headers.get("retry-after").map(str::to_owned));
                }
            };
            self.advance(self.takes);
            Outcome {
                breaker: Arc::clone(b),
                probe,
                start,
                done: false,
            }
            .header(status);
            (status, None)
        }

        /// `n` requests answered `status`: how many the client got of each status.
        fn serve(&self, n: usize, status: u16) -> std::collections::BTreeMap<u16, usize> {
            let mut got = std::collections::BTreeMap::new();
            for _ in 0..n {
                *got.entry(self.one(status).0).or_default() += 1;
            }
            got
        }

        fn expect(&self, state: State, transitions: &[&str]) {
            assert_eq!(self.b.state(self.b.clock.now()), state);
            assert_eq!(*self.b.transitions.lock().unwrap(), transitions);
        }
    }

    fn params(f: impl FnOnce(&mut akit_config::CircuitBreaker)) -> akit_config::CircuitBreaker {
        let mut p = akit_config::CircuitBreaker::default();
        f(&mut p);
        p
    }

    #[test]
    fn transitions() {
        // fit's TestBreakerTransitions.
        let r = Rig::new(params(|p| {
            p.min_requests = 10;
            p.error_ratio = 0.5;
            p.open_duration = Duration::from_secs(30);
            p.half_open_requests = 3;
        }));
        // Closed: failures under the ratio, or under min_requests, keep it so.
        r.serve(6, 200);
        r.serve(3, 502);
        r.expect(State::Closed, &[]);
        r.serve(4, 200);
        r.serve(3, 500); // 6 of 16
        r.expect(State::Closed, &[]);

        // Past the ratio it opens, and answers 503 at once with when to retry.
        r.serve(10, 503); // 16 of 26
        r.expect(State::Open, &["closed>open"]);
        assert_eq!(r.one(200), (503, Some("30".into())));
        assert_eq!(r.serve(5, 200).get(&503), Some(&5));
        r.advance(20 * S);
        assert_eq!(r.one(200).1.as_deref(), Some("10"));

        // Half-open after open_duration: a failed probe opens it again.
        r.advance(10 * S);
        r.expect(State::HalfOpen, &["closed>open", "open>half-open"]);
        r.serve(1, 200);
        r.serve(1, 500);
        r.expect(
            State::Open,
            &["closed>open", "open>half-open", "half-open>open"],
        );

        // Probes that all fare well close it, and the window starts afresh.
        r.advance(30 * S);
        assert_eq!(r.serve(3, 200).get(&200), Some(&3));
        let all = [
            "closed>open",
            "open>half-open",
            "half-open>open",
            "open>half-open",
            "half-open>closed",
        ];
        r.expect(State::Closed, &all);
        r.serve(9, 500);
        r.expect(State::Closed, &all);
    }

    #[test]
    fn half_open_limit() {
        // fit's TestBreakerHalfOpenLimit: half-open lets only half_open_requests through at
        // once, and a probe whose client goes gives its place back.
        let r = Rig::new(params(|p| {
            p.min_requests = 1;
            p.open_duration = Duration::from_secs(1);
            p.half_open_requests = 2;
        }));
        let b = &r.b;
        let now = b.clock.now();
        b.record(now, false, true, false);
        assert_eq!(b.state(now), State::Open);
        r.advance(S);
        let now = b.clock.now();
        assert_eq!(b.admit(now), Admit::Probe);
        assert_eq!(b.admit(now), Admit::Probe);
        assert_eq!(b.admit(now), Admit::Refuse(S));
        drop(Outcome {
            breaker: Arc::clone(b),
            probe: true,
            start: now,
            done: false,
        });
        assert_eq!(b.admit(now), Admit::Probe, "a gone probe's place was kept");
    }

    #[test]
    fn window() {
        // fit's TestBreakerWindow.
        let r = Rig::new(params(|p| {
            p.window = Duration::from_secs(10);
            p.min_requests = 10;
        }));
        r.serve(9, 500);
        r.advance(11 * S); // they leave the window
        r.serve(9, 200);
        r.serve(1, 500);
        r.expect(State::Closed, &[]);
        r.advance(5 * S);
        r.serve(9, 500); // 10 of 19 in the last 10s
        r.expect(State::Open, &["closed>open"]);
    }

    #[test]
    fn slow_and_statuses() {
        // fit's TestBreakerSlowAndStatuses.
        let mut r = Rig::new(params(|p| {
            p.min_requests = 4;
            p.slow_threshold = Some(Duration::from_secs(1));
            p.slow_ratio = 0.5;
            p.statuses = vec![429];
        }));
        r.serve(4, 500); // not a failure here
        r.expect(State::Closed, &[]);
        r.takes = 2 * S;
        r.serve(4, 200);
        r.expect(State::Open, &["closed>open"]); // 4 slow of 8

        let r = Rig::new(params(|p| {
            p.min_requests = 4;
            p.statuses = vec![429];
        }));
        r.serve(4, 429);
        r.expect(State::Open, &["closed>open"]);
    }

    #[test]
    fn late_outcome() {
        // fit's TestBreakerLateOutcome: the outcome of a request let through while it was
        // closed, which comes once it has opened, is not counted.
        let r = Rig::new(params(|p| {
            p.min_requests = 1;
            p.open_duration = Duration::from_secs(60);
        }));
        let b = &r.b;
        let now = b.clock.now();
        b.record(now, false, true, false);
        assert_eq!(b.state(now), State::Open);
        let before = b.counts();
        b.record(now, false, true, false);
        assert_eq!(b.state(now), State::Open);
        assert_eq!(b.counts(), before, "a late outcome counted");
    }

    #[test]
    fn retry_after_rounds_up() {
        assert_eq!(refused(1).headers.get("retry-after"), Some("1"));
        assert_eq!(refused(S).headers.get("retry-after"), Some("1"));
        assert_eq!(refused(S + 1).headers.get("retry-after"), Some("2"));
        assert_eq!(refused(-5).headers.get("retry-after"), Some("0"));
    }
}
