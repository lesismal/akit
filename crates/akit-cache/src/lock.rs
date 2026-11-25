//! The cache lock and the cache's own fetches, as fit's `middleware/cache/lock.go`.
//!
//! A request that misses while another fetches the same response waits for that fetch, up to
//! `lock_timeout`, and is answered from what it keeps, so that many requests for a response not
//! kept make one fetch. It waits suspended (akio's `Context::suspend`, ADR-109), holding no
//! thread: the fetch's end resumes it on whichever thread ends the fetch, and the timeout on
//! akit's timer thread (`akit_upstream::timer`, akio API request A45's stopgap), one timer per
//! fetch.
//!
//! A stale response with validators, or one `stale-if-error` allows, is fetched again by the
//! cache itself, with a request of its own (akio's `transport::dispatch_conn` to a sink that
//! sees the response go by and drops it), which the client's request waits for with any others:
//! a 304 freshens the response, and a failure (no response, or a 500, 502, 503 or 504) may be
//! answered with it. A response `stale-while-revalidate` allows is answered at once, and fetched
//! again so in the background, at most [`MAX_BACKGROUND`] at once in a zone.
//!
//! The cache's own request is the client's as the cache saw it (its target, fields, peer, and
//! what akit's listener and middleware recorded on it: TLS, the canonical path, the body limit),
//! without its range and preconditions, as a GET with no body.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use akio_http::transport::{BodySink, ConnSink, SharedSink, dispatch_conn};
use akio_http::{Context, Headers, Method, Request, Response, Suspended};
use akit_runtime::upstream::timer;
use akit_runtime::{Handler, MAX_BODY_BYTES, TLS};

use crate::cache::{Fwd, Miss, Shared, hit_response, usable};
use crate::clock;
use crate::semantics::Directives;
use crate::store::{Entry, Store};

/// The requests that may wait for one fetch; past it a request fetches for itself.
pub(crate) const MAX_WAITERS: usize = 4096;
/// The fetches a zone makes that no request waits for; past it a stale response is answered
/// without being refreshed, until a later request finds room.
pub(crate) const MAX_BACKGROUND: u64 = 256;

/// How a fetch ended: its response's status, and `failed` when it got none, or one cut short.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FetchResult {
    pub(crate) status: u16,
    pub(crate) failed: bool,
}

impl FetchResult {
    /// Whether the fetch failed as `stale-if-error` means it: no response, or a 500, 502, 503
    /// or 504.
    pub(crate) fn erred(self) -> bool {
        self.failed || matches!(self.status, 500 | 502 | 503 | 504)
    }
}

/// A fetch of a key's response under way, which the requests for the key that come meanwhile
/// wait for. It is done once the response is kept, or is known not to be.
pub(crate) struct Fetch {
    key: Arc<str>,
    started: Instant,
    st: Mutex<FetchState>,
}

impl std::fmt::Debug for Fetch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fetch")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct FetchState {
    done: Option<FetchResult>,
    waiters: Vec<Waiter>,
    /// The fetch's timer is set.
    timer: bool,
}

impl Fetch {
    fn lock(&self) -> MutexGuard<'_, FetchState> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn key(&self) -> &str {
        &self.key
    }
}

impl Store {
    /// The fetch under way for `key`, or a new one, with `true`, which the caller then makes. A
    /// fetch older than `abandoned` is taken for one that will never end, as a handler that never
    /// answers would leave, and is replaced.
    pub(crate) fn lock(&self, key: &str, now: Instant, abandoned: Duration) -> (Arc<Fetch>, bool) {
        let mut sh = self.shard(key);
        if let Some(f) = sh.fetches.get(key) {
            if now.saturating_duration_since(f.started) < abandoned {
                return (Arc::clone(f), false);
            }
        }
        let key: Arc<str> = Arc::from(key);
        let f = Arc::new(Fetch {
            key: Arc::clone(&key),
            started: now,
            st: Mutex::new(FetchState::default()),
        });
        sh.fetches.insert(key, Arc::clone(&f));
        (f, true)
    }

    /// Ends `f` with `r`: it is no longer what requests for its key wait for, and those that
    /// waited are answered, on this thread. Only the first call does anything.
    pub(crate) fn finish(&self, f: &Arc<Fetch>, r: FetchResult) {
        {
            let mut sh = self.shard(&f.key);
            if sh.fetches.get(&f.key).is_some_and(|g| Arc::ptr_eq(g, f)) {
                sh.fetches.remove(&f.key);
            }
        }
        let waiters = {
            let mut st = f.lock();
            if st.done.is_some() {
                return;
            }
            st.done = Some(r);
            std::mem::take(&mut st.waiters)
        };
        for w in waiters {
            w.request.resume(move |ctx| w.wait.answer(ctx, r));
        }
    }

    /// How many fetches are under way; for tests.
    pub(crate) fn fetches(&self) -> usize {
        (0..crate::store::SHARDS)
            .map(|i| self.shard_at(i).fetches.len())
            .sum()
    }
}

/// Ends a fetch the request carrying it makes for others: with what it is given, or, dropped
/// first (the response never written, or its body cut), as failed.
pub(crate) struct FetchEnd(Option<(Arc<Store>, Arc<Fetch>)>);

impl FetchEnd {
    pub(crate) fn new(store: Arc<Store>, f: Arc<Fetch>) -> FetchEnd {
        FetchEnd(Some((store, f)))
    }

    pub(crate) fn end(mut self, r: FetchResult) {
        if let Some((s, f)) = self.0.take() {
            s.finish(&f, r);
        }
    }
}

impl Drop for FetchEnd {
    fn drop(&mut self) {
        if let Some((s, f)) = self.0.take() {
            s.finish(
                &f,
                FetchResult {
                    status: 0,
                    failed: true,
                },
            );
        }
    }
}

/// A request waiting for a fetch.
struct Waiter {
    request: Suspended,
    wait: Wait,
    since: Instant,
}

/// What a request waiting for a fetch is answered with.
pub(crate) struct Wait {
    pub(crate) next: Arc<dyn Handler>,
    pub(crate) miss: Miss,
    pub(crate) req: Directives,
}

impl Wait {
    /// Answers from what the fetch kept, from the stale response it had if the fetch failed and
    /// `stale-if-error` allows, or else sends the request on.
    fn answer(self, ctx: &mut Context, r: FetchResult) {
        let now = clock::now();
        let shared = Arc::clone(&self.miss.shared);
        if let (Some(e), _) = shared.store.get(&self.miss.key, &ctx.req().headers) {
            if usable(&e, &self.req, now).is_ok() {
                ctx.respond(hit_response(&e, &ctx.req().headers, e.age(now), None));
                return;
            }
        }
        if let Some(s) = &self.miss.stale {
            if r.erred() && !s.no_stale && s.stale_for(now, s.sie) {
                let detail = Some("stale-if-error");
                ctx.respond(hit_response(s, &ctx.req().headers, s.age(now), detail));
                return;
            }
        }
        shared.forward(ctx, self.miss, &self.next);
    }
}

impl Shared {
    /// Has the request of `ctx` wait for `f`, or go on if it cannot.
    pub(crate) fn wait(self: &Arc<Self>, f: &Arc<Fetch>, ctx: &mut Context, wait: Wait) {
        let mut st = f.lock();
        if let Some(r) = st.done {
            drop(st);
            return wait.answer(ctx, r);
        }
        if st.waiters.len() >= MAX_WAITERS {
            drop(st);
            return self.forward(ctx, wait.miss, &wait.next);
        }
        if !st.timer {
            let (f2, timeout) = (Arc::clone(f), self.lock_timeout);
            if !timer::after(timeout, move || expire(&f2, timeout)) {
                drop(st);
                return self.forward(ctx, wait.miss, &wait.next);
            }
            st.timer = true;
        }
        let Some(request) = ctx.suspend() else {
            drop(st);
            return self.forward(ctx, wait.miss, &wait.next);
        };
        st.waiters.push(Waiter {
            request,
            wait,
            since: Instant::now(),
        });
    }

    /// Refreshes `e`, `key`'s stale response, which the request of `ctx` was just answered with,
    /// by a fetch of the cache's own that nothing waits for, unless one is under way or the zone
    /// makes as many as it may.
    pub(crate) fn revalidate(
        self: &Arc<Self>,
        key: &str,
        e: &Arc<Entry>,
        base: Base,
        next: &Arc<dyn Handler>,
    ) {
        let (f, first) = self
            .store
            .lock(key, Instant::now(), self.lock_timeout.saturating_mul(2));
        if !first {
            return;
        }
        let now = self.store.get(key, &base.headers).0;
        if !now.is_some_and(|n| Arc::ptr_eq(&n, e)) {
            // A fetch that ended since `e` was looked up has refreshed it.
            return self.store.finish(
                &f,
                FetchResult {
                    status: 200,
                    failed: false,
                },
            );
        }
        if self.store.background.fetch_add(1, Ordering::AcqRel) >= MAX_BACKGROUND {
            self.store.background.fetch_sub(1, Ordering::AcqRel);
            return self.store.finish(
                &f,
                FetchResult {
                    status: 0,
                    failed: true,
                },
            );
        }
        self.refetch(f, base, Arc::clone(e), next, true);
    }

    /// Fetches the response of `f`'s key again with a request of the cache's own made from
    /// `base`, revalidating `stale` with its validators, and ends `f` once the response has
    /// gone by. Runs the handlers behind the cache on this thread, which they do not block.
    pub(crate) fn refetch(
        self: &Arc<Self>,
        f: Arc<Fetch>,
        base: Base,
        stale: Arc<Entry>,
        next: &Arc<dyn Handler>,
        background: bool,
    ) {
        let mut headers = Headers::with_capacity(base.headers.len().saturating_add(1), 256);
        for (n, v) in base.headers.iter() {
            if !PRECONDITIONS.iter().any(|p| p.eq_ignore_ascii_case(n)) {
                headers.append(n, v);
            }
        }
        if let Some(tag) = &stale.etag {
            headers.set("If-None-Match", tag);
        } else if let Some(lm) = &stale.last_modified {
            headers.set("If-Modified-Since", lm);
        }
        let authorized = base.authorized || headers.get("authorization").is_some();
        let mut req = Request::new(Method::Get, &base.target);
        req.headers = headers;
        req.peer = base.peer;
        let miss = Miss {
            shared: Arc::clone(self),
            key: f.key().to_owned(),
            request_time: clock::now(),
            fwd: Fwd::Stale,
            stale: Some(stale),
            revalidating: false,
            storable: true,
            authorized,
            ranged: false,
            fetch: None,
        };
        let end = Arc::new(Ended {
            store: Arc::clone(&self.store),
            fetch: f,
            background,
            done: AtomicBool::new(false),
        });
        let service = Own {
            miss: Mutex::new(Some(miss)),
            next: Arc::clone(next),
            locals: base.locals,
        };
        dispatch_conn(&service, req, &SharedSink::new(Sink(end)), 0);
    }
}

/// The request fields the cache's own request goes without: its range and preconditions, and
/// the client's own directives.
const PRECONDITIONS: [&str; 8] = [
    "range",
    "if-range",
    "if-match",
    "if-none-match",
    "if-modified-since",
    "if-unmodified-since",
    "cache-control",
    "pragma",
];

/// What the cache's own request is made from: the client's request as the cache saw it.
pub(crate) struct Base {
    target: String,
    pub(crate) headers: Headers,
    peer: SocketAddr,
    locals: Vec<(&'static str, String)>,
    authorized: bool,
}

/// What akit's listener and middleware record on a request that the handlers behind the cache
/// may read.
const LOCALS: [&str; 2] = [TLS, MAX_BODY_BYTES];

impl Base {
    pub(crate) fn of(ctx: &Context, authorized: bool) -> Base {
        Base {
            target: ctx.req().target.clone(),
            headers: ctx.req().headers.clone(),
            peer: ctx.peer(),
            locals: LOCALS
                .iter()
                .filter_map(|k| ctx.local(k).map(|v| (*k, v.to_owned())))
                .collect(),
            authorized,
        }
    }
}

/// The cache's own request through the handlers behind the cache, its response kept as a
/// client's would be.
struct Own {
    miss: Mutex<Option<Miss>>,
    next: Arc<dyn Handler>,
    locals: Vec<(&'static str, String)>,
}

impl akio_http::Service for Own {
    fn call(&self, ctx: &mut Context) {
        for (k, v) in &self.locals {
            ctx.set_local(k, v.as_str());
        }
        let miss = self
            .miss
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(miss) = miss {
            ctx.add_outer_response_filter(move |resp: &mut Response, req: &Request| {
                miss.filter(resp, req);
            });
        }
        self.next.call(ctx);
    }
}

/// Ends the fetch the cache's own request makes, once its response has gone by.
struct Ended {
    store: Arc<Store>,
    fetch: Arc<Fetch>,
    background: bool,
    done: AtomicBool,
}

impl Ended {
    fn end(&self, status: u16, failed: bool) {
        if self.done.swap(true, Ordering::AcqRel) {
            return;
        }
        self.store
            .finish(&self.fetch, FetchResult { status, failed });
        if self.background {
            self.store.background.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

impl Drop for Ended {
    fn drop(&mut self) {
        self.end(0, true);
    }
}

/// Where the response to the cache's own request goes: nowhere, once the cache's filter and
/// tap have seen it.
struct Sink(Arc<Ended>);

impl ConnSink for Sink {
    fn send(&self, _tag: u64, resp: Response) {
        self.0.end(resp.status, false);
    }

    fn start_stream(&self, _tag: u64, resp: Response) -> Option<Box<dyn BodySink>> {
        Some(Box::new(Drain {
            end: Arc::clone(&self.0),
            status: resp.status,
        }))
    }
}

/// A streamed response's body, dropped as it comes; one dropped unfinished was cut short.
struct Drain {
    end: Arc<Ended>,
    status: u16,
}

impl BodySink for Drain {
    fn write(&self, _data: &[u8]) -> akio::Result<()> {
        Ok(())
    }

    fn queued_bytes(&self) -> usize {
        0
    }

    fn finish(self: Box<Self>, _trailers: Option<Headers>) {
        self.end.end(self.status, false);
    }
}

impl Drop for Drain {
    fn drop(&mut self) {
        self.end.end(self.status, true);
    }
}

/// Sends on the requests that waited for `f` for `timeout`, and sets the timer again for the
/// others. Runs on the timer thread.
fn expire(f: &Arc<Fetch>, timeout: Duration) {
    let due = {
        let mut st = f.lock();
        let now = Instant::now();
        let (due, keep): (Vec<Waiter>, Vec<Waiter>) = std::mem::take(&mut st.waiters)
            .into_iter()
            .partition(|w| now.saturating_duration_since(w.since) >= timeout);
        st.waiters = keep;
        st.timer = false;
        if let Some(first) = st.waiters.iter().map(|w| w.since).min() {
            let left = timeout.saturating_sub(now.saturating_duration_since(first));
            let f2 = Arc::clone(f);
            if timer::after(left, move || expire(&f2, timeout)) {
                st.timer = true;
            }
        }
        if st.timer {
            due
        } else {
            // No timer: none waits any longer.
            let mut due = due;
            due.append(&mut st.waiters);
            due
        }
    };
    for w in due {
        let Waiter { request, wait, .. } = w;
        request.resume(move |ctx| {
            let shared = Arc::clone(&wait.miss.shared);
            shared.forward(ctx, wait.miss, &wait.next);
        });
    }
}
