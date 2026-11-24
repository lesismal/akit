//! The `cache` middleware, as fit's `middleware/cache`: an HTTP cache shared by a route's
//! clients, as RFC 9111 has a shared cache work.
//!
//! It keeps the responses to GET requests a shared cache may keep: ones whose status it
//! understands, that do not say `no-store`, `private` or `no-cache`, do not vary on everything
//! (`Vary: *`), set no cookie, carry no trailers and are no larger than `max_entry_bytes`, with a
//! freshness lifetime from `s-maxage`, `max-age` or `Expires`, or, for the statuses that allow
//! one, from `default_ttl` or a tenth of the time since `Last-Modified`. A request with
//! `Authorization`, or on a route with an auth middleware, has its response kept only when it
//! says `public`, `s-maxage` or `must-revalidate`.
//!
//! A fresh response answers GET and HEAD requests, with its `Age`, as a 304 for one whose
//! `If-None-Match` or `If-Modified-Since` it meets. A request's `no-cache`, `max-age` and
//! `min-fresh` send it on, and its `max-stale` lets it have a stale response that allows it;
//! `only-if-cached` gets 504 when nothing is kept. A request with a `Range` goes on. A stale
//! response with validators is fetched again with them: the upstream's 304 freshens it and the
//! client is answered from it. A request that changes something (POST, PUT, DELETE and the like)
//! and succeeds drops what is kept for its URI, and for its `Location` and `Content-Location` on
//! the same host. Every answer to GET and HEAD says what the cache did in `Cache-Status`
//! (RFC 9211).
//!
//! A response is kept as it goes to the client, never held back to be stored: a whole one from
//! its body, a streamed one (a proxied body, I16) copied write by write by a body tap (akio's
//! `Response::tap_body`, ADR-116) and stored only once all of it went. The cache's filter is an
//! outer one (akio's `Context::add_outer_response_filter`), so it sees the response as the
//! middlewares behind it (`compress`) left it: each encoding is a variant of its own. What
//! responses being stored hold at once is bounded by a quarter of `max_bytes`.
//!
//! A request that misses while another fetches the same response waits for that fetch (the cache
//! lock, [`crate::lock`]); a stale response is fetched again by the cache itself, answered at
//! once while it is if `stale-while-revalidate` allows, and answered when fetching it fails if
//! `stale-if-error` allows (RFC 5861; the `stale_while_revalidate` and `stale_if_error` params
//! for a response that says nothing of it). With a disk tier ([`crate::disk`]) every response
//! kept in memory is written to disk too, and one memory no longer holds is read back, on the
//! disk pool, while its request waits suspended.
//!
//! Per request it reads the clock once, takes one shard lock for the lookup (another for a store
//! or an invalidation, or for the cache lock on a miss), and allocates the key; a hit copies the
//! stored fields and body into the response, as akio's file cache copies its memory bodies.
//! Nothing waits or blocks on an I/O thread. A GET or HEAD whose body would stream to the
//! handlers comes whole (within the listener's `max_body_bytes`), so that it can wait.
//!
//! Not yet: a body akio sends from a file on disk is not kept. Purge by key and tag is I69's.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime};

use akio_http::{BodyMode, BodyTap, Context, Headers, Request, Response, TapEnd};
use akit_runtime::{
    Handler, HandlerParams, Middleware, Provision, ProvisionError, Provisioned, TLS,
};

use crate::clock;
use crate::disk::{self, Disk, DiskEntry};
use crate::lock::{Base, FetchEnd, FetchResult, Wait};
use crate::semantics::{
    Directives, NOT_MODIFIED_FIELDS, freshness, http_date, not_modified, parse_directives,
    request_directives, storable_status, unsafe_method, unstored, update_excluded, vary_names,
    vary_value, weak_equal,
};
use crate::store::{Entry, Store, acquire};

/// What the cache did with a request it sent on, as `Cache-Status` says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fwd {
    UriMiss,
    VaryMiss,
    Stale,
    Request,
    Bypass,
}

impl Fwd {
    /// The `Cache-Status` member, without and with `stored`.
    fn field(self, stored: bool) -> &'static str {
        match (self, stored) {
            (Fwd::UriMiss, false) => "fit; fwd=uri-miss",
            (Fwd::UriMiss, true) => "fit; fwd=uri-miss; stored",
            (Fwd::VaryMiss, false) => "fit; fwd=vary-miss",
            (Fwd::VaryMiss, true) => "fit; fwd=vary-miss; stored",
            (Fwd::Stale, false) => "fit; fwd=stale",
            (Fwd::Stale, true) => "fit; fwd=stale; stored",
            (Fwd::Request, false) => "fit; fwd=request",
            (Fwd::Request, true) => "fit; fwd=request; stored",
            (Fwd::Bypass, false) => "fit; fwd=bypass",
            (Fwd::Bypass, true) => "fit; fwd=bypass; stored",
        }
    }
}

/// The middleware, compiled.
#[derive(Debug)]
pub(crate) struct Cache {
    shared: Arc<Shared>,
}

/// What every request of the middleware's routes reads.
#[derive(Debug)]
pub(crate) struct Shared {
    pub(crate) store: Arc<Store>,
    /// The zone's disk tier, as this cache was compiled with it.
    pub(crate) disk: Option<Arc<Disk>>,
    max_entry_bytes: u64,
    default_ttl: Option<Duration>,
    /// The route lets requests in on credentials: each is treated as carrying `Authorization`.
    authenticated: bool,
    /// Misses wait for another's fetch of their response (no `disable_lock`).
    lock: bool,
    pub(crate) lock_timeout: Duration,
    /// The stale windows of a response that says nothing of them; zero for none.
    swr: Duration,
    sie: Duration,
}

pub(crate) fn new(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Middleware>, ProvisionError> {
    let HandlerParams::Cache(c) = p.params() else {
        return Err(ProvisionError::new(
            "the cache middleware was given another's params",
        ));
    };
    let (store, disk) = acquire(&c.zone, c.max_bytes, c.disk.as_ref())
        .map_err(|e| ProvisionError::field("disk", e))?;
    Ok(Provisioned::new(Arc::new(Cache {
        shared: Arc::new(Shared {
            store,
            disk,
            max_entry_bytes: c.max_entry_bytes,
            default_ttl: c.default_ttl,
            authenticated: c.authenticated,
            lock: !c.disable_lock,
            lock_timeout: c.lock_timeout,
            swr: c.stale_while_revalidate.unwrap_or(Duration::ZERO),
            sie: c.stale_if_error.unwrap_or(Duration::ZERO),
        }),
    }) as Arc<dyn Middleware>))
}

impl Middleware for Cache {
    fn wrap(&self, next: Arc<dyn Handler>) -> Arc<dyn Handler> {
        Arc::new(Cached {
            shared: Arc::clone(&self.shared),
            next,
        })
    }
}

/// A handler behind the middleware.
struct Cached {
    shared: Arc<Shared>,
    next: Arc<dyn Handler>,
}

impl akio_http::Service for Cached {
    fn call(&self, ctx: &mut Context) {
        self.shared.serve(ctx, &self.next);
    }

    /// The handler's, except that a GET or HEAD whose body would stream comes whole, so that
    /// the request can wait for a fetch or a read from disk.
    fn body_mode(&self, req: &Request) -> BodyMode {
        match self.next.body_mode(req) {
            BodyMode::Stream if matches!(req.method.as_str(), "GET" | "HEAD") => BodyMode::Buffered,
            mode => mode,
        }
    }

    fn streams_bodies(&self) -> bool {
        self.next.streams_bodies()
    }
}

impl Handler for Cached {}

/// The key of a response: the request's scheme, host (in lower case) and target.
fn key_of(ctx: &Context, target: &str) -> String {
    let scheme = if ctx.local(TLS).is_some() {
        "https://"
    } else {
        "http://"
    };
    let host = ctx.header("host").unwrap_or("");
    let mut k = String::with_capacity(
        scheme
            .len()
            .saturating_add(host.len())
            .saturating_add(target.len()),
    );
    k.push_str(scheme);
    k.extend(host.chars().map(|c| c.to_ascii_lowercase()));
    k.push_str(target);
    k
}

/// Whether a stored response may answer a request with directives `req` at `now`, or else why
/// the request goes on.
pub(crate) fn usable(e: &Entry, req: &Directives, now: SystemTime) -> Result<(), Fwd> {
    let age = e.age(now);
    let fresh = age < e.lifetime;
    if req.no_cache || req.max_age.is_some_and(|m| age > m) {
        return Err(Fwd::Request);
    }
    let left = e.lifetime.saturating_sub(age);
    if fresh && req.min_fresh.is_none_or(|m| left >= m) {
        return Ok(());
    }
    if fresh {
        return Err(Fwd::Request);
    }
    let over = age.saturating_sub(e.lifetime);
    if !e.no_stale && (req.max_stale_any || req.max_stale.is_some_and(|m| over <= m)) {
        return Ok(());
    }
    Err(Fwd::Stale)
}

/// Seconds of `a - b`, toward zero, as Go divides a `time.Duration`.
fn signed_secs(a: Duration, b: Duration) -> i64 {
    // Whole seconds of the difference, truncated, then signed.
    let whole = |d: Duration| i64::try_from(d.as_secs()).unwrap_or(i64::MAX);
    match a.checked_sub(b) {
        Some(d) => whole(d),
        None => whole(b.saturating_sub(a)).saturating_neg(),
    }
}

/// The answer to a request with fields `req` from `e`, which is `age` old; `detail`, if any, says
/// more in `Cache-Status`.
pub(crate) fn hit_response(
    e: &Entry,
    req: &Headers,
    age: Duration,
    detail: Option<&str>,
) -> Response {
    let use_304 =
        e.status == 200 && not_modified(req, e.etag.as_deref(), e.last_modified.as_deref());
    let mut r = Response::new(if use_304 { 304 } else { e.status });
    if use_304 {
        let mut h = Headers::with_capacity(NOT_MODIFIED_FIELDS.len().saturating_add(2), 256);
        for name in NOT_MODIFIED_FIELDS {
            for v in e.headers.get_all(name) {
                h.append(name, v);
            }
        }
        r.headers = h;
    } else {
        r.headers = e.headers.clone();
        r.body = e.body.to_vec();
    }
    r.headers.set("Age", age.as_secs().to_string());
    let ttl = signed_secs(e.lifetime, age);
    let cs = match detail {
        Some(d) => format!("fit; hit; ttl={ttl}; detail={d}"),
        None => format!("fit; hit; ttl={ttl}"),
    };
    r.headers.set("Cache-Status", cs);
    r
}

impl Shared {
    fn serve(self: &Arc<Self>, ctx: &mut Context, next: &Arc<dyn Handler>) {
        let method = ctx.req().method.as_str();
        let get = method == "GET";
        if !get && method != "HEAD" {
            if unsafe_method(method) {
                let me = Arc::clone(self);
                let key = key_of(ctx, "");
                ctx.add_outer_response_filter(move |resp: &mut Response, req: &Request| {
                    if (200..400).contains(&resp.status) {
                        me.invalidate(&key, req, &resp.headers);
                    }
                });
            }
            next.call(ctx);
            return;
        }
        let key = key_of(ctx, &ctx.req().target);
        let now = clock::now();
        let req = request_directives(&ctx.req().headers);
        let mut fwd = Fwd::UriMiss;
        let mut stale = None;
        let ranged = ctx.req().headers.get("range").is_some();
        let authorized = self.authenticated || ctx.req().headers.get("authorization").is_some();
        if ranged {
            fwd = Fwd::Bypass;
        } else {
            match self.store.get(&key, &ctx.req().headers) {
                (Some(e), _) => match usable(&e, &req, now) {
                    Ok(()) => {
                        let r = hit_response(&e, &ctx.req().headers, e.age(now), None);
                        ctx.respond(r);
                        return;
                    }
                    Err(Fwd::Stale) if !e.no_stale && e.stale_for(now, e.swr) => {
                        let base = Base::of(ctx, authorized);
                        let detail = Some("stale-while-revalidate");
                        ctx.respond(hit_response(&e, &ctx.req().headers, e.age(now), detail));
                        self.revalidate(&key, &e, base, next);
                        return;
                    }
                    Err(why) => {
                        fwd = why;
                        stale = Some(e);
                    }
                },
                (None, found) => {
                    if found {
                        fwd = Fwd::VaryMiss;
                    }
                    let on_disk = self
                        .disk
                        .as_ref()
                        .and_then(|d| Some((d, d.get(&key, &ctx.req().headers)?)));
                    if let Some((d, de)) = on_disk {
                        let m = &de.meta;
                        match usable(m, &req, now) {
                            Ok(()) => {
                                let d = Arc::clone(d);
                                return self.from_disk(ctx, next, d, de, key, fwd, false);
                            }
                            Err(Fwd::Stale) if !m.no_stale && m.stale_for(now, m.swr) => {
                                let d = Arc::clone(d);
                                return self.from_disk(ctx, next, d, de, key, fwd, true);
                            }
                            Err(why) => fwd = why,
                        }
                    }
                }
            }
        }
        if req.only_if_cached {
            ctx.respond(Response::text(504, "504 Gateway Timeout\n"));
            return;
        }
        let mut miss = Miss {
            shared: Arc::clone(self),
            key,
            request_time: now,
            fwd,
            stale,
            revalidating: false,
            storable: get && !req.no_store,
            authorized,
            ranged,
            fetch: None,
        };
        if get && self.lock && !ranged && fwd != Fwd::Request && !req.no_store {
            let abandoned = self.lock_timeout.saturating_mul(2);
            let (f, first) = self.store.lock(&miss.key, Instant::now(), abandoned);
            let next = Arc::clone(next);
            if !first {
                return self.wait(&f, ctx, Wait { next, miss, req });
            }
            let refetched = miss.stale.clone().filter(|s| {
                s.etag.is_some() || s.last_modified.is_some() || (!s.no_stale && !s.sie.is_zero())
            });
            if let Some(s) = refetched {
                // The cache fetches it again itself, with the stale response's validators, and
                // the request waits for that with any others: a 304 freshens the response, and
                // a failure may be answered with it, as stale-if-error allows.
                let base = Base::of(ctx, authorized);
                self.wait(
                    &f,
                    ctx,
                    Wait {
                        next: Arc::clone(&next),
                        miss,
                        req,
                    },
                );
                self.refetch(f, base, s, &next, false);
                return;
            }
            miss.fetch = Some(FetchEnd::new(Arc::clone(&self.store), f));
        }
        self.forward(ctx, miss, next);
    }

    /// Sends the request on to `next`, with the filter that keeps its response. A stale response
    /// with validators is asked for again with them, unless the client asks with its own: a 304
    /// then freshens it, and the client is answered from it.
    pub(crate) fn forward(&self, ctx: &mut Context, mut miss: Miss, next: &Arc<dyn Handler>) {
        let get = ctx.req().method.as_str() == "GET";
        if let Some(s) = miss.stale.as_ref().filter(|_| get) {
            let headers = &ctx.req().headers;
            let own = headers.get("if-none-match").is_some()
                || headers.get("if-modified-since").is_some();
            if !own && (s.etag.is_some() || s.last_modified.is_some()) {
                let mut fields = akit_runtime::request::request_headers(ctx).into_owned();
                if let Some(tag) = &s.etag {
                    fields.set("If-None-Match", tag);
                }
                if let Some(lm) = &s.last_modified {
                    fields.set("If-Modified-Since", lm);
                }
                akit_runtime::request::set_request_headers(ctx, &fields);
                miss.revalidating = true;
            }
        }
        ctx.add_outer_response_filter(move |resp: &mut Response, req: &Request| {
            miss.filter(resp, req);
        });
        next.call(ctx);
    }

    /// Answers the request of `ctx` from `de`, read back from `d` on the disk pool while the
    /// request waits suspended, and keeps it in memory again. A read that fails, or does not
    /// come within the pool's timeout, as from a disk that hangs, leaves the request to go on as
    /// a miss (`fwd`). `stale` says the response is stale, and answered as
    /// stale-while-revalidate allows: it is fetched again in the background once answered.
    #[allow(
        clippy::too_many_arguments,
        reason = "the request's state, as it was looked up"
    )]
    fn from_disk(
        self: &Arc<Self>,
        ctx: &mut Context,
        next: &Arc<dyn Handler>,
        d: Arc<Disk>,
        de: Arc<DiskEntry>,
        key: String,
        fwd: Fwd,
        stale: bool,
    ) {
        let authorized = self.authenticated || ctx.req().headers.get("authorization").is_some();
        let miss = Miss {
            shared: Arc::clone(self),
            key,
            request_time: clock::now(),
            fwd,
            stale: None,
            revalidating: false,
            storable: ctx.req().method.as_str() == "GET",
            authorized,
            ranged: false,
            fetch: None,
        };
        let gone = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let g = Arc::clone(&gone);
        ctx.on_gone(move || g.store(true, Ordering::Release));
        let Some(request) = ctx.suspend() else {
            return self.forward(ctx, miss, next);
        };
        // Whichever takes it first answers: the read, or the timeout.
        let slot: Arc<Reading> = Arc::new(Mutex::new(Some((request, miss, Arc::clone(next)))));
        let (s1, s2) = (Arc::clone(&slot), Arc::clone(&slot));
        if !akit_runtime::upstream::timer::after(disk::timeout(), move || as_miss(&s1)) {
            return as_miss(&slot);
        }
        let me = Arc::clone(self);
        let run = move || {
            if gone.load(Ordering::Acquire) {
                // Its client went: nothing to read for.
                drop(take(&s2));
                return;
            }
            let read = d.read(&de);
            let Some((request, miss, next)) = take(&s2) else {
                return;
            };
            let e = match read {
                Ok(e) => e,
                Err(_) => {
                    d.drop_entry(&miss.key, &de);
                    return request.resume(move |ctx| {
                        let shared = Arc::clone(&miss.shared);
                        shared.forward(ctx, miss, &next);
                    });
                }
            };
            let e = me.store.promote(&miss.key, e);
            request.resume(move |ctx| {
                let now = clock::now();
                if stale {
                    let base = Base::of(ctx, miss.authorized);
                    let detail = Some("stale-while-revalidate");
                    ctx.respond(hit_response(&e, &ctx.req().headers, e.age(now), detail));
                    me.revalidate(&miss.key, &e, base, &next);
                } else {
                    let detail = Some("disk");
                    ctx.respond(hit_response(&e, &ctx.req().headers, e.age(now), detail));
                }
            });
        };
        let s3 = Arc::clone(&slot);
        if !disk::submit(run, move || as_miss(&s3)) {
            as_miss(&slot);
        }
    }

    /// Keeps `e` under `key`, in memory, and on disk when the zone has a disk tier.
    fn keep(&self, key: &str, e: Entry) {
        let e = self.store.put(key, e);
        if let Some(d) = &self.disk {
            d.write(key, &e);
        }
    }

    /// Drops what is kept under `key`, in memory and on disk.
    fn remove(&self, key: &str) {
        self.store.remove(key);
        if let Some(d) = &self.disk {
            d.remove(key);
        }
    }

    /// Drops what is stored for the target of `req`, and for the `Location` and
    /// `Content-Location` of its response `h` when they are on the same host (RFC 9111 section
    /// 4.4). `base` is the request's key without its target.
    fn invalidate(&self, base: &str, req: &Request, h: &Headers) {
        let mut key = String::with_capacity(base.len().saturating_add(req.target.len()));
        key.push_str(base);
        key.push_str(&req.target);
        self.remove(&key);
        for field in ["location", "content-location"] {
            if let Some(target) = h.get(field).and_then(|v| same_origin(base, req.path(), v)) {
                key.truncate(base.len());
                key.push_str(&target);
                self.remove(&key);
            }
        }
    }

    /// The entry of a response with `status` and `headers` (those it keeps), and the `Age` field
    /// `age`, to a request sent at `request_time` with fields `req`; `None` when a shared cache
    /// must not keep it.
    fn entry_of(
        &self,
        status: u16,
        headers: Headers,
        req: &Headers,
        request_time: SystemTime,
        age: Option<&str>,
        authorized: bool,
    ) -> Option<Entry> {
        let d = parse_directives(headers.get_all("cache-control"));
        if d.no_store || d.private || d.no_cache {
            return None;
        }
        if authorized && !d.public && d.s_maxage.is_none() && !d.must_revalidate {
            return None;
        }
        if ["set-cookie", "trailer", "content-range"]
            .iter()
            .any(|f| headers.get(f).is_some())
        {
            return None;
        }
        let names = vary_names(&headers)?;
        let now = clock::now();
        let date = headers.get("date").and_then(http_date).unwrap_or(now);
        let (lifetime, explicit) = freshness(&headers, &d, status, date, self.default_ttl);
        if !storable_status(status, explicit) {
            return None;
        }
        let no_stale = d.must_revalidate || d.proxy_revalidate || d.s_maxage.is_some();
        let (swr, sie) = if no_stale {
            (Duration::ZERO, Duration::ZERO)
        } else {
            (
                d.stale_while_revalidate.unwrap_or(self.swr),
                d.stale_if_error.unwrap_or(self.sie),
            )
        };
        let age_value = age.map_or(Duration::ZERO, crate::semantics::delta);
        let since_date = now.duration_since(date).unwrap_or(Duration::ZERO);
        let since_request = now.duration_since(request_time).unwrap_or(Duration::ZERO);
        let initial_age = since_date.max(age_value.saturating_add(since_request));
        if initial_age >= lifetime {
            return None;
        }
        let vary = names
            .into_iter()
            .map(|n| {
                let v = vary_value(req, &n);
                (n, v)
            })
            .collect();
        let etag = headers.get("etag").map(str::to_owned);
        let last_modified = headers.get("last-modified").map(str::to_owned);
        Some(Entry {
            status,
            headers,
            body: Arc::from(&[][..]),
            vary,
            response_time: now,
            initial_age,
            lifetime,
            no_stale,
            swr,
            sie,
            etag,
            last_modified,
            size: 0,
        })
    }
}

/// A request waiting for a read from disk, until the read or its timeout takes it.
type Reading = Mutex<Option<(akio_http::Suspended, Miss, Arc<dyn Handler>)>>;

fn take(slot: &Reading) -> Option<(akio_http::Suspended, Miss, Arc<dyn Handler>)> {
    slot.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
}

/// Sends the request waiting in `slot` on as a miss, unless the read took it.
fn as_miss(slot: &Reading) {
    if let Some((request, miss, next)) = take(slot) {
        request.resume(move |ctx| {
            let shared = Arc::clone(&miss.shared);
            shared.forward(ctx, miss, &next);
        });
    }
}

/// The path-and-query `v` (a `Location` or `Content-Location`, perhaps relative to `path`) names
/// on the request's own scheme and host, which `base` (`scheme://host`) gives; `None` for one
/// elsewhere, or one that does not parse.
fn same_origin(base: &str, path: &str, v: &str) -> Option<String> {
    let v = v.trim();
    let v = v.split('#').next().unwrap_or("");
    let (scheme, host) = base.split_once("://")?;
    let target = if let Some(rest) = v.strip_prefix("//") {
        let (h, t) = split_authority(rest);
        if !h.eq_ignore_ascii_case(host) {
            return None;
        }
        t
    } else if let Some((s, rest)) = v.split_once("://").filter(|(s, _)| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
    }) {
        if !s.eq_ignore_ascii_case(scheme) {
            return None;
        }
        let (h, t) = split_authority(rest);
        if !h.eq_ignore_ascii_case(host) {
            return None;
        }
        t
    } else if v.starts_with('/') {
        v.to_owned()
    } else if v.is_empty() {
        return None;
    } else {
        // Relative to the request's path: its directory.
        let dir = path
            .rfind('/')
            .map_or("/", |i| path.get(..=i).unwrap_or("/"));
        format!("{dir}{v}")
    };
    Some(remove_dots(&target))
}

/// The authority of `rest` (what follows `//`) and its target, `/` when it has none.
fn split_authority(rest: &str) -> (&str, String) {
    match rest.find(['/', '?']) {
        Some(i) => {
            let (h, t) = rest.split_at(i);
            let t = if t.starts_with('?') {
                format!("/{t}")
            } else {
                t.to_owned()
            };
            (h, t)
        }
        None => (rest, "/".to_owned()),
    }
}

/// `target` with its path's `.` and `..` segments resolved (RFC 3986 section 5.2.4).
fn remove_dots(target: &str) -> String {
    let (path, query) = match target.find('?') {
        Some(i) => target.split_at(i),
        None => (target, ""),
    };
    if !path.split('/').any(|s| s == "." || s == "..") {
        return target.to_owned();
    }
    let mut out: Vec<&str> = Vec::new();
    let segs: Vec<&str> = path.split('/').skip(1).collect();
    let last = segs.len().saturating_sub(1);
    for (i, s) in segs.iter().enumerate() {
        match *s {
            "." => {
                if i == last {
                    out.push("");
                }
            }
            ".." => {
                out.pop();
                if i == last {
                    out.push("");
                }
            }
            s => out.push(s),
        }
    }
    let mut p = String::with_capacity(target.len());
    for s in out {
        p.push('/');
        p.push_str(s);
    }
    if p.is_empty() {
        p.push('/');
    }
    p.push_str(query);
    p
}

/// A request the cache sent on, and what it does with its response.
pub(crate) struct Miss {
    pub(crate) shared: Arc<Shared>,
    pub(crate) key: String,
    pub(crate) request_time: SystemTime,
    pub(crate) fwd: Fwd,
    /// The stored response the request did not take, freshened by a 304 for it.
    pub(crate) stale: Option<Arc<Entry>>,
    /// The cache asked with the stale response's validators, the client with none of its own:
    /// the client is answered from the stale response, not with the upstream's 304.
    pub(crate) revalidating: bool,
    /// The request allows keeping its response (a GET without `no-store`).
    pub(crate) storable: bool,
    pub(crate) authorized: bool,
    pub(crate) ranged: bool,
    /// The fetch this request makes for others waiting for the same response, ended once the
    /// response is kept or known not to be.
    pub(crate) fetch: Option<FetchEnd>,
}

impl Miss {
    /// Runs once the handlers behind the cache settled the response: answers from the stale
    /// response a 304 is for, decides whether to keep the response, and says what the cache did.
    pub(crate) fn filter(mut self, resp: &mut Response, req: &Request) {
        let status = resp.status;
        if status == 304 && !resp.is_streaming() {
            if let Some(old) = self.stale.clone() {
                if let Some(answer) = self.freshen(&old, resp, req) {
                    *resp = answer;
                    return self.done(status, false);
                }
            }
        }
        let keep = self.capture(resp, req);
        resp.headers.append("Cache-Status", self.fwd.field(keep));
        self.done(status, false);
    }

    /// Ends the fetch the request makes for others, if it still has it.
    fn done(&mut self, status: u16, failed: bool) {
        if let Some(f) = self.fetch.take() {
            f.end(FetchResult { status, failed });
        }
    }

    /// The answer to the client from `old`, which the 304 `resp` is for (their ETags, or else
    /// their `Last-Modified`, are the same), stored again with its fields updated from `resp`;
    /// `None` when the 304 is not for it and goes to the client as it is. A response that may
    /// no longer be kept is removed, and the client answered from it all the same when the cache
    /// asked for it.
    fn freshen(&self, old: &Arc<Entry>, resp: &Response, req: &Request) -> Option<Response> {
        let h = &resp.headers;
        let same = match h.get("etag") {
            Some(tag) => old.etag.as_deref().is_some_and(|o| weak_equal(tag, o)),
            None => h.get("last-modified") == old.last_modified.as_deref(),
        };
        let store = &self.shared.store;
        if !same {
            if !self.revalidating {
                return None;
            }
            // Not for what the cache asked about: answer from it as it is, and keep it no more.
            self.shared.remove(&self.key);
            let mut r = hit_response(old, &req.headers, old.age(clock::now()), None);
            r.headers.set("Cache-Status", Fwd::Stale.field(false));
            return Some(r);
        }
        let mut merged = old.headers.clone();
        let mut done: Vec<&str> = Vec::new();
        for (name, _) in h.iter() {
            if unstored(name)
                || update_excluded(name)
                || done.iter().any(|d| d.eq_ignore_ascii_case(name))
            {
                continue;
            }
            done.push(name);
            merged.remove(name);
            for v in h.get_all(name) {
                merged.append(name, v);
            }
        }
        let fresh = self.shared.entry_of(
            old.status,
            merged.clone(),
            &req.headers,
            self.request_time,
            h.get("age"),
            self.authorized,
        );
        match fresh {
            Some(mut e) => {
                e.body = Arc::clone(&old.body);
                let (e, stored) = store.replace(&self.key, old, e);
                if let Some(d) = self.shared.disk.as_ref().filter(|_| stored) {
                    d.write(&self.key, &e);
                }
                Some(hit_response(&e, &req.headers, e.age(clock::now()), None))
            }
            None => {
                self.shared.remove(&self.key);
                if !self.revalidating {
                    return None;
                }
                let mut r = Response::new(old.status);
                r.headers = merged;
                r.body = old.body.to_vec();
                r.headers.set("Cache-Status", Fwd::Stale.field(false));
                Some(r)
            }
        }
    }

    /// Starts keeping `resp` if it may be kept: a whole body at once, a streamed one through a
    /// tap. Reports whether it is being kept.
    fn capture(&mut self, resp: &mut Response, req: &Request) -> bool {
        if !self.storable || matches!(resp.status, 204 | 304) || resp.file.is_some() {
            return false;
        }
        if resp.trailers.is_some() {
            return false;
        }
        if self.ranged && (resp.status != 200 || resp.headers.get("content-range").is_some()) {
            return false;
        }
        let limit = self.shared.max_entry_bytes;
        let declared = match resp.headers.get("content-length") {
            Some(v) => match v.trim().parse::<u64>() {
                Ok(n) if n <= limit => Some(n),
                _ => return false,
            },
            None => None,
        };
        let streaming = resp.is_streaming();
        if !streaming {
            let n = resp.body.len() as u64;
            if n > limit || declared.is_some_and(|d| d != n) {
                return false;
            }
        }
        let mut kept = Headers::with_capacity(resp.headers.len(), 256);
        for (n, v) in resp.headers.iter() {
            if !unstored(n) {
                kept.append(n, v);
            }
        }
        let Some(mut e) = self.shared.entry_of(
            resp.status,
            kept,
            &req.headers,
            self.request_time,
            resp.headers.get("age"),
            self.authorized,
        ) else {
            return false;
        };
        if !streaming {
            e.body = Arc::from(resp.body.as_slice());
            self.shared.keep(&self.key, e);
            return true;
        }
        let capacity = declared.and_then(|d| usize::try_from(d).ok()).unwrap_or(0);
        resp.tap_body(Capture {
            shared: Arc::clone(&self.shared),
            key: self.key.clone(),
            entry: Some(e),
            want: declared,
            limit,
            buf: Vec::with_capacity(capacity),
            held: 0,
            fetch: self.fetch.take(),
        });
        true
    }
}

/// Copies a streamed response's body as it goes, and stores the response once all of it went.
struct Capture {
    shared: Arc<Shared>,
    key: String,
    /// `None` once given up.
    entry: Option<Entry>,
    /// The declared length, and the most kept.
    want: Option<u64>,
    limit: u64,
    buf: Vec<u8>,
    /// What this capture counts in its store's `capturing`.
    held: u64,
    /// The fetch the request makes for others, ended once the response is kept or given up.
    fetch: Option<FetchEnd>,
}

impl Capture {
    /// Gives up keeping the response, and what it held.
    fn give_up(&mut self) {
        let status = self.entry.take().map_or(0, |e| e.status);
        self.buf = Vec::new();
        self.release();
        if let Some(f) = self.fetch.take() {
            f.end(FetchResult {
                status,
                failed: false,
            });
        }
    }

    fn release(&mut self) {
        if self.held > 0 {
            let store = &self.shared.store;
            store.capturing.fetch_sub(self.held, Ordering::AcqRel);
            self.held = 0;
        }
    }
}

impl BodyTap for Capture {
    fn data(&mut self, data: &[u8]) {
        if self.entry.is_none() {
            return;
        }
        let n = data.len() as u64;
        if (self.buf.len() as u64).saturating_add(n) > self.limit {
            return self.give_up();
        }
        self.held = self.held.saturating_add(n);
        let store = &self.shared.store;
        let before = store.capturing.fetch_add(n, Ordering::AcqRel);
        if before.saturating_add(n) > store.max_bytes() / 4 {
            return self.give_up();
        }
        self.buf.extend_from_slice(data);
    }

    fn end(&mut self, end: TapEnd) {
        let Some(mut e) = self.entry.take() else {
            return;
        };
        let body = std::mem::take(&mut self.buf);
        self.release();
        let whole = end == TapEnd::Finished && self.want.is_none_or(|w| w == body.len() as u64);
        let status = e.status;
        if whole {
            e.body = body.into();
            self.shared.keep(&self.key, e);
        }
        if let Some(f) = self.fetch.take() {
            f.end(FetchResult {
                status,
                failed: !whole,
            });
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_on_the_same_origin() {
        let b = "http://example.com";
        for (v, want) in [
            ("/fresh", Some("/fresh")),
            ("/a/../b?x=1#f", Some("/b?x=1")),
            ("c", Some("/dir/c")),
            ("./c/.", Some("/dir/c/")),
            ("http://EXAMPLE.com/s", Some("/s")),
            ("http://example.com", Some("/")),
            ("http://example.com?q", Some("/?q")),
            ("//example.com/s", Some("/s")),
            ("https://example.com/s", None),
            ("http://elsewhere/s-maxage", None),
            ("//elsewhere/s", None),
            ("", None),
        ] {
            assert_eq!(same_origin(b, "/dir/page", v).as_deref(), want, "{v:?}");
        }
    }

    #[test]
    fn go_rounds_toward_zero() {
        let s = Duration::from_secs;
        assert_eq!(signed_secs(s(60), Duration::from_millis(30_500)), 29);
        assert_eq!(
            signed_secs(Duration::from_millis(60_000), Duration::from_millis(80_900)),
            -20
        );
        assert_eq!(signed_secs(s(1), s(1)), 0);
    }
}
