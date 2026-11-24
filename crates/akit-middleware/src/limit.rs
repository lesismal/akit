//! The limit middleware, as fit's `middleware/limit`: `rate_limit`, requests a second by token
//! bucket, keyed by the client's address, a header or the route; `concurrency_limit`, requests
//! under way at once; and `body_limit`, the size of a request's body.
//!
//! Each is compiled when the config is (rule 12) and runs inline on the I/O thread, never
//! blocking (rule 1):
//!
//! - `rate_limit` keeps its buckets as fit does, in 64 shards by the key's hash, each a map under
//!   a lock of its own, at most `max_keys` keys in all. Per I/O thread they would let a client
//!   with connections on many threads take the rate that many times, so they are shared, as
//!   exactness asks; no lock is global, and a request takes one shard's for a lookup. A key is
//!   the client's address, or a header's value by its keyed hash (SipHash with a random key,
//!   which a client cannot aim collisions at), so a lookup allocates nothing.
//! - `concurrency_limit` counts the requests under way with one atomic, which an exact limit
//!   needs. A request's place is given back when its response is sent, or when the request ends
//!   without one (the [`Detached`](akio_http::Detached) a handler kept is dropped). akio has no
//!   hook yet for a streamed response's end or for a client gone while its request waits (akio
//!   API request A48): until then a streamed response gives its place back when its head is
//!   sent, and a request whose client went holds its place until its handler lets it go (the
//!   proxy's upstream answers or fails).
//! - `body_limit` answers 413 before the handler runs a request that declares a longer body
//!   (taking the body as a stream, so none of it is buffered) or whose buffered body is longer.
//!   One that streams to its handler with no declared length is held to the limit by the
//!   handler, as the listener's `max_body_bytes` is (akit-runtime's [`MAX_BODY_BYTES`], which
//!   the proxy reads): the middleware lowers it. akio cannot limit a body a middleware does not
//!   read itself yet (akio API request A49), nor close the connection after a 413 rather than
//!   read the rest of the body (A24).

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, BuildHasherDefault, Hash, Hasher};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use akio_http::{BodyMode, Context, Headers, Request, Response};
use akit_config::{Network, RateKey};
use akit_runtime::{
    Handler, HandlerParams, MAX_BODY_BYTES, Middleware, Provision, ProvisionError, Provisioned,
    request_headers,
};

/// How many shards the buckets are kept in.
const SHARDS: usize = 64;

/// A token, in nanotokens: buckets count in nanotokens so that refills add up exactly.
const TOKEN: i64 = 1_000_000_000;

/// How many keys an eviction looks at for one whose bucket is full.
const EVICT_LOOKS: usize = 8;

/// The middleware's params given to another's constructor.
fn wrong(name: &str) -> ProvisionError {
    ProvisionError::new(format!("the {name} middleware was given another's params"))
}

// ---- rate_limit ----

/// What a request is counted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    /// Every request of the route.
    Route,
    /// A client's address (an IPv6 one cut to its prefix).
    Addr(IpAddr),
    /// A header's value, by its keyed hash.
    Text(u64),
}

/// A key with its hash, which the shard's map uses as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Keyed {
    h: u64,
    key: Key,
}

impl Hash for Keyed {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.h);
    }
}

/// Passes a [`Keyed`]'s hash through.
#[derive(Debug, Default)]
struct Pass(u64);

impl Hasher for Pass {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        // Only Keyed is hashed with it, which writes a u64; this keeps any other use sound.
        for b in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(*b);
        }
    }

    fn write_u64(&mut self, h: u64) {
        self.0 = h;
    }
}

type Map = HashMap<Keyed, Bucket, BuildHasherDefault<Pass>>;

/// One token bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Bucket {
    /// Nanotokens.
    tokens: i64,
    /// When `tokens` was last brought up to date, in nanoseconds.
    last: i64,
}

/// A shard, on a cache line of its own.
#[derive(Debug, Default)]
#[repr(align(128))]
struct Shard(Mutex<Map>);

impl Shard {
    fn lock(&self) -> std::sync::MutexGuard<'_, Map> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The token buckets, by key.
#[derive(Debug)]
struct Buckets {
    shards: Box<[Shard]>,
    /// Nanotokens a nanosecond, which is tokens a second.
    rate: f64,
    /// Nanotokens.
    burst: i64,
    max_per_shard: usize,
    hasher: RandomState,
}

impl Buckets {
    fn new(rate: f64, burst: u64, max_keys: u64) -> Buckets {
        let per_shard =
            usize::try_from(max_keys.checked_div(SHARDS as u64).unwrap_or(0)).unwrap_or(usize::MAX);
        Buckets {
            shards: (0..SHARDS).map(|_| Shard::default()).collect(),
            rate,
            burst: i64::try_from(burst)
                .unwrap_or(i64::MAX)
                .saturating_mul(TOKEN),
            max_per_shard: per_shard.max(1),
            hasher: RandomState::new(),
        }
    }

    /// Takes a token from `key`'s bucket at `now`, in nanoseconds; when there is none, `Err`
    /// with how long until there is.
    fn take(&self, key: Key, now: i64) -> Result<(), i64> {
        let h = self.hasher.hash_one(key);
        // The map's index takes the low bits, its tags the high ones: the shard takes others.
        let i = usize::try_from((h >> 32).checked_rem(SHARDS as u64).unwrap_or(0)).unwrap_or(0);
        let Some(shard) = self.shards.get(i) else {
            return Ok(());
        };
        let k = Keyed { h, key };
        let mut m = shard.lock();
        let bucket = match m.get_mut(&k) {
            Some(b) => b,
            None => {
                if m.len() >= self.max_per_shard {
                    self.evict(&mut m, now);
                }
                m.entry(k).or_insert(Bucket {
                    tokens: self.burst,
                    last: now,
                })
            }
        };
        bucket.tokens = self.refill(*bucket, now);
        bucket.last = now;
        if bucket.tokens >= TOKEN {
            bucket.tokens = bucket.tokens.saturating_sub(TOKEN);
            Ok(())
        } else {
            let short = TOKEN.saturating_sub(bucket.tokens) as f64;
            // A float's cast saturates.
            #[allow(clippy::cast_possible_truncation)]
            Err((short / self.rate).ceil() as i64)
        }
    }

    /// `b`'s tokens at `now`, up to the burst.
    fn refill(&self, b: Bucket, now: i64) -> i64 {
        let add = now.saturating_sub(b.last).max(0) as f64 * self.rate;
        if add >= self.burst.saturating_sub(b.tokens) as f64 {
            self.burst
        } else {
            // Less than what is missing, so it fits.
            #[allow(clippy::cast_possible_truncation)]
            b.tokens.saturating_add(add as i64)
        }
    }

    /// Removes a key from `m`: the first of a few whose bucket is full by `now`, or else the
    /// first of them.
    fn evict(&self, m: &mut Map, now: i64) {
        let mut victim = None;
        for (k, b) in m.iter().take(EVICT_LOOKS) {
            if victim.is_none() {
                victim = Some(*k);
            }
            if self.refill(*b, now) == self.burst {
                victim = Some(*k);
                break;
            }
        }
        if let Some(k) = victim {
            m.remove(&k);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.shards.iter().map(|s| s.lock().len()).sum()
    }
}

/// The time, in nanoseconds since the middleware was compiled.
#[derive(Debug, Clone)]
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

/// The `rate_limit` middleware, compiled.
#[derive(Debug)]
struct Rate {
    buckets: Buckets,
    key: RateKey,
    trusted: Vec<Network>,
    v6bits: u8,
    status: u16,
    clock: Clock,
}

impl Rate {
    fn new(p: &akit_config::RateLimit, clock: Clock) -> Rate {
        Rate {
            buckets: Buckets::new(p.rate, p.burst, p.max_keys),
            key: p.key.clone(),
            trusted: p.trusted_proxies.clone(),
            v6bits: p.ipv6_prefix,
            status: p.status,
            clock,
        }
    }

    /// What a request from `peer` with `headers` is counted by.
    fn key(&self, peer: SocketAddr, headers: &Headers) -> Key {
        match &self.key {
            RateKey::Route => return Key::Route,
            RateKey::Header(name) => {
                if let Some(v) = headers.get(name) {
                    return Key::Text(self.buckets.hasher.hash_one(v.as_bytes()));
                }
            }
            RateKey::Ip => {}
        }
        Key::Addr(self.client(client_ip(peer, headers, &self.trusted)))
    }

    /// What `ip` is counted by: an IPv4 address as it is, an IPv6 one by its first `v6bits`.
    fn client(&self, ip: IpAddr) -> IpAddr {
        match ip {
            IpAddr::V6(a) if self.v6bits < 128 => {
                let bits = u32::from(self.v6bits);
                let mask = u128::MAX
                    .checked_shl(128u32.saturating_sub(bits))
                    .unwrap_or(0);
                IpAddr::V6(Ipv6Addr::from(u128::from(a) & mask))
            }
            ip => ip,
        }
    }

    /// Takes a token for the request in `ctx`: `Err` with the whole seconds until there is one,
    /// rounded up, when there is none.
    fn admit(&self, peer: SocketAddr, headers: &Headers) -> Result<(), i64> {
        let key = self.key(peer, headers);
        self.buckets
            .take(key, self.clock.now())
            .map_err(retry_after)
    }
}

/// A wait in nanoseconds as `Retry-After`'s whole seconds, rounded up.
fn retry_after(wait: i64) -> i64 {
    wait.saturating_add(TOKEN - 1)
        .checked_div(TOKEN)
        .unwrap_or(0)
}

/// The client's address: the connection's peer, or, when that is a trusted proxy, the last
/// address in `X-Forwarded-For` that is not one, as fit's `clientIP`. A list that does not parse
/// is not believed past where it fails. It allocates nothing.
///
/// fit walks the addresses from the last; akio's fields iterate forwards, so this walks them
/// forwards and keeps what the walk back would end on: the last address that stops it (one not
/// trusted, which is the client, or one that does not parse) and the first trusted one after it.
fn client_ip(peer: SocketAddr, headers: &Headers, trusted: &[Network]) -> IpAddr {
    let ip = unmap(peer.ip());
    if trusted.is_empty() || !is_trusted(ip, trusted) {
        return ip;
    }
    // The last address that stops the walk back: Some(Some(client)), or Some(None) when it
    // does not parse.
    let mut stop: Option<Option<IpAddr>> = None;
    let mut first_trusted_after: Option<IpAddr> = None;
    for list in headers.get_all("x-forwarded-for") {
        if list.is_empty() {
            continue;
        }
        for item in list.split(',') {
            match parse_ip(item.trim()) {
                Some(a) if is_trusted(a, trusted) => {
                    first_trusted_after.get_or_insert(a);
                }
                a => {
                    stop = Some(a);
                    first_trusted_after = None;
                }
            }
        }
    }
    match stop {
        Some(Some(client)) => client,
        _ => first_trusted_after.unwrap_or(ip),
    }
}

/// An address, as Go's `netip.ParseAddr` takes it, unmapped and without its zone.
fn parse_ip(s: &str) -> Option<IpAddr> {
    let s = s.split_once('%').map_or(s, |(a, _)| a);
    s.parse().ok().map(unmap)
}

fn unmap(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(a) => a.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        ip => ip,
    }
}

fn is_trusted(ip: IpAddr, trusted: &[Network]) -> bool {
    trusted.iter().any(|n| n.contains(ip))
}

pub(crate) fn new_rate(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Middleware>, ProvisionError> {
    let HandlerParams::RateLimit(r) = p.params() else {
        return Err(wrong("rate_limit"));
    };
    Ok(Provisioned::new(
        Arc::new(RateMiddleware(Arc::new(Rate::new(
            r,
            Clock::Real(Instant::now()),
        )))) as Arc<dyn Middleware>,
    ))
}

/// The `rate_limit` middleware; the buckets are the route's, shared by every handler it wraps.
#[derive(Debug)]
struct RateMiddleware(Arc<Rate>);

impl Middleware for RateMiddleware {
    fn wrap(&self, next: Arc<dyn Handler>) -> Arc<dyn Handler> {
        Arc::new(RateLimited {
            rate: Arc::clone(&self.0),
            next,
        })
    }
}

struct RateLimited {
    rate: Arc<Rate>,
    next: Arc<dyn Handler>,
}

impl akio_http::Service for RateLimited {
    fn call(&self, ctx: &mut Context) {
        let admitted = {
            let headers = request_headers(ctx);
            self.rate.admit(ctx.req().peer, &headers)
        };
        match admitted {
            Ok(()) => self.next.call(ctx),
            Err(secs) => {
                let mut resp = Response::new(self.rate.status);
                resp.headers.set("Retry-After", secs.to_string());
                ctx.respond(resp);
            }
        }
    }

    fn body_mode(&self, req: &Request) -> BodyMode {
        self.next.body_mode(req)
    }

    fn streams_bodies(&self) -> bool {
        self.next.streams_bodies()
    }
}

impl Handler for RateLimited {}

// ---- concurrency_limit ----

/// The count of requests under way, on a cache line of its own.
#[derive(Debug, Default)]
#[repr(align(128))]
struct InFlight(AtomicI64);

/// The `concurrency_limit` middleware, compiled; the count is the route's.
#[derive(Debug)]
struct Concurrency {
    max: i64,
    status: u16,
    in_flight: Arc<InFlight>,
}

pub(crate) fn new_concurrency(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Middleware>, ProvisionError> {
    let HandlerParams::ConcurrencyLimit(c) = p.params() else {
        return Err(wrong("concurrency_limit"));
    };
    Ok(Provisioned::new(Arc::new(Concurrency {
        max: i64::try_from(c.max).unwrap_or(i64::MAX),
        status: c.status,
        in_flight: Arc::default(),
    }) as Arc<dyn Middleware>))
}

impl Middleware for Concurrency {
    fn wrap(&self, next: Arc<dyn Handler>) -> Arc<dyn Handler> {
        Arc::new(ConcurrencyLimited {
            max: self.max,
            status: self.status,
            in_flight: Arc::clone(&self.in_flight),
            next,
        })
    }
}

struct ConcurrencyLimited {
    max: i64,
    status: u16,
    in_flight: Arc<InFlight>,
    next: Arc<dyn Handler>,
}

/// One request's place under the limit, given back when dropped.
struct Place(Arc<InFlight>);

impl Drop for Place {
    fn drop(&mut self) {
        self.0.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl akio_http::Service for ConcurrencyLimited {
    fn call(&self, ctx: &mut Context) {
        if self.in_flight.0.fetch_add(1, Ordering::AcqRel) >= self.max {
            self.in_flight.0.fetch_sub(1, Ordering::AcqRel);
            ctx.respond(Response::new(self.status));
            return;
        }
        // Given back when the response is sent, the filter's closure going with it, or when the
        // request ends without one, its filters dropped with it (akio API request A48 ends it
        // with a streamed response's body, and when the client goes).
        let place = Place(Arc::clone(&self.in_flight));
        ctx.add_response_filter(move |_: &mut Response, _: &Request| drop(place));
        self.next.call(ctx);
    }

    fn body_mode(&self, req: &Request) -> BodyMode {
        self.next.body_mode(req)
    }

    fn streams_bodies(&self) -> bool {
        self.next.streams_bodies()
    }
}

impl Handler for ConcurrencyLimited {}

// ---- body_limit ----

/// fit's (Go's) answer to a body past the limit.
const TOO_LARGE: &str = "Request Entity Too Large\n";

pub(crate) fn new_body(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Middleware>, ProvisionError> {
    let HandlerParams::BodyLimit(b) = p.params() else {
        return Err(wrong("body_limit"));
    };
    Ok(Provisioned::new(
        Arc::new(Body(b.max_bytes)) as Arc<dyn Middleware>
    ))
}

/// The `body_limit` middleware: its `max_bytes`.
#[derive(Debug)]
struct Body(u64);

impl Middleware for Body {
    fn wrap(&self, next: Arc<dyn Handler>) -> Arc<dyn Handler> {
        Arc::new(BodyLimited {
            max: self.0,
            max_text: self.0.to_string(),
            next,
        })
    }
}

struct BodyLimited {
    max: u64,
    /// `max`, written once, for [`MAX_BODY_BYTES`].
    max_text: String,
    next: Arc<dyn Handler>,
}

/// The body length `req` declares (`Content-Length`), if any.
fn declared_length(req: &Request) -> Option<u64> {
    req.headers
        .get("content-length")
        .and_then(|v| v.trim().parse().ok())
}

impl BodyLimited {
    fn refuse(ctx: &mut Context) {
        ctx.respond(Response::with_body(
            413,
            "text/plain; charset=utf-8",
            TOO_LARGE.as_bytes().to_vec(),
        ));
    }
}

impl akio_http::Service for BodyLimited {
    fn call(&self, ctx: &mut Context) {
        let declared = declared_length(ctx.req());
        if declared.is_some_and(|n| n > self.max) {
            Self::refuse(ctx);
            return;
        }
        match ctx.local(MAX_BODY_BYTES) {
            // The body streams to the handler, which holds it to MAX_BODY_BYTES; one that
            // declares its length cannot outgrow it.
            Some(limit) => {
                let lower = limit.parse::<u64>().map_or(true, |l| self.max < l);
                if declared.is_none() && lower {
                    ctx.set_local(MAX_BODY_BYTES, self.max_text.as_str());
                }
            }
            None => {
                if u64::try_from(ctx.body().len()).unwrap_or(u64::MAX) > self.max {
                    Self::refuse(ctx);
                    return;
                }
            }
        }
        self.next.call(ctx);
    }

    /// A body declared past the limit is taken as a stream, so that none of it is buffered
    /// before it is refused; any other as the handler takes it.
    fn body_mode(&self, req: &Request) -> BodyMode {
        if declared_length(req).is_some_and(|n| n > self.max) {
            BodyMode::Stream
        } else {
            self.next.body_mode(req)
        }
    }
}

impl Handler for BodyLimited {}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        clippy::cast_possible_truncation
    )]

    use super::*;

    const SECOND: i64 = 1_000_000_000;

    fn params(rate: f64, burst: u64) -> akit_config::RateLimit {
        akit_config::RateLimit {
            rate,
            burst,
            key: RateKey::Ip,
            trusted_proxies: Vec::new(),
            ipv6_prefix: 64,
            max_keys: akit_config::DEFAULT_RATE_MAX_KEYS,
            status: 429,
        }
    }

    /// A rate limit on a clock the test moves.
    fn rate(p: &akit_config::RateLimit) -> (Rate, Arc<AtomicI64>) {
        let t = Arc::new(AtomicI64::new(0));
        (Rate::new(p, Clock::Fake(Arc::clone(&t))), t)
    }

    fn peer(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    fn headers(pairs: &[(&str, &str)]) -> Headers {
        let mut h = Headers::new();
        for (n, v) in pairs {
            h.append(n, v);
        }
        h
    }

    /// The status the limit gives a request from `remote`: 200 or 429.
    fn status(rl: &Rate, remote: &str, h: &[(&str, &str)]) -> u16 {
        match rl.admit(peer(remote), &headers(h)) {
            Ok(()) => 200,
            Err(_) => 429,
        }
    }

    // fit's TestRateBucket.
    #[test]
    fn rate_bucket() {
        let (rl, clk) = rate(&params(1.0, 3));
        let serve = || rl.admit(peer("192.0.2.1:1"), &Headers::new());
        for i in 0..3 {
            assert_eq!(serve(), Ok(()), "request {i} of the burst");
        }
        assert_eq!(serve(), Err(1), "past the burst");
        clk.fetch_add(SECOND / 2, Ordering::Relaxed);
        assert_eq!(serve(), Err(1), "half a token later");
        clk.fetch_add(SECOND / 2, Ordering::Relaxed);
        assert_eq!(serve(), Ok(()), "a token later");
        assert!(serve().is_err(), "that token again");
        // Idle long, the bucket fills to the burst and no further.
        clk.fetch_add(3600 * SECOND, Ordering::Relaxed);
        for i in 0..3 {
            assert_eq!(serve(), Ok(()), "request {i} after an hour");
        }
        assert!(serve().is_err(), "past the burst after an hour");
    }

    // fit's TestRateSlow: Retry-After of a slow rate.
    #[test]
    fn rate_slow() {
        let (rl, _) = rate(&params(0.01, 1));
        assert_eq!(rl.admit(peer("192.0.2.1:1"), &Headers::new()), Ok(()));
        assert_eq!(rl.admit(peer("192.0.2.1:1"), &Headers::new()), Err(100));
    }

    // fit's TestRateKeys.
    #[test]
    fn rate_keys() {
        let long = "k".repeat(200);
        let other = format!("{}j", "k".repeat(199));
        type Case<'a> = (
            akit_config::RateLimit,
            Vec<(&'a str, Vec<(&'a str, &'a str)>)>,
            Vec<u16>,
        );
        let cases: Vec<(&str, Case<'_>)> = vec![
            (
                "by ip",
                (
                    params(1.0, 1),
                    vec![
                        ("192.0.2.1:1", vec![]),
                        ("192.0.2.1:2", vec![]),
                        ("192.0.2.2:1", vec![]),
                        ("[::ffff:192.0.2.2]:1", vec![]),
                        ("[2001:db8::1]:1", vec![]),
                    ],
                    vec![200, 429, 200, 429, 200],
                ),
            ),
            (
                "by route",
                (
                    akit_config::RateLimit {
                        key: RateKey::Route,
                        ..params(1.0, 2)
                    },
                    vec![
                        ("192.0.2.1:1", vec![]),
                        ("192.0.2.2:1", vec![]),
                        ("192.0.2.3:1", vec![]),
                    ],
                    vec![200, 200, 429],
                ),
            ),
            (
                "by header, and by ip without it",
                (
                    akit_config::RateLimit {
                        key: RateKey::Header("x-api-key".into()),
                        ..params(1.0, 1)
                    },
                    vec![
                        ("192.0.2.1:1", vec![("X-Api-Key", "a")]),
                        ("192.0.2.2:1", vec![("X-Api-Key", "a")]),
                        ("192.0.2.1:1", vec![("X-Api-Key", "b")]),
                        ("192.0.2.1:1", vec![]),
                        ("192.0.2.1:1", vec![]),
                        ("192.0.2.1:1", vec![("X-Api-Key", long.as_str())]),
                        ("192.0.2.2:1", vec![("x-api-key", long.as_str())]),
                        ("192.0.2.2:1", vec![("X-Api-Key", other.as_str())]),
                    ],
                    vec![200, 429, 200, 200, 429, 200, 429, 200],
                ),
            ),
            (
                "behind trusted proxies",
                (
                    akit_config::RateLimit {
                        trusted_proxies: vec![Network::parse("10.0.0.0/8").unwrap()],
                        ..params(1.0, 1)
                    },
                    vec![
                        ("10.0.0.1:1", vec![("X-Forwarded-For", "192.0.2.1")]),
                        (
                            "10.0.0.2:1",
                            vec![("X-Forwarded-For", "192.0.2.1, 10.0.0.9")],
                        ),
                        (
                            "10.0.0.1:1",
                            vec![("X-Forwarded-For", "192.0.2.7, 192.0.2.2")],
                        ),
                        ("192.0.2.3:1", vec![("X-Forwarded-For", "192.0.2.9")]),
                        ("192.0.2.4:1", vec![("X-Forwarded-For", "192.0.2.3")]),
                        ("10.0.0.1:1", vec![]),
                        // All trusted: the first is the client.
                        ("10.0.0.1:1", vec![("X-Forwarded-For", "10.9.9.9")]),
                    ],
                    vec![200, 429, 200, 200, 200, 200, 200],
                ),
            ),
        ];
        for (name, (p, requests, want)) in cases {
            let (rl, _) = rate(&p);
            for (i, (remote, h)) in requests.iter().enumerate() {
                assert_eq!(status(&rl, remote, h), want[i], "{name}: request {i}");
            }
        }
    }

    // fit's TestClientIP.
    #[test]
    fn client_ips() {
        let trusted = [
            Network::parse("10.0.0.0/8").unwrap(),
            Network::parse("2001:db8::/32").unwrap(),
        ];
        for (remote, xff, want) in [
            ("192.0.2.1:1", &["198.51.100.1"][..], "192.0.2.1"),
            ("10.0.0.1:1", &[], "10.0.0.1"),
            ("10.0.0.1:1", &["198.51.100.1"], "198.51.100.1"),
            (
                "10.0.0.1:1",
                &["198.51.100.1, 198.51.100.2"],
                "198.51.100.2",
            ),
            (
                "10.0.0.1:1",
                &["198.51.100.1", "198.51.100.2 ,10.1.1.1"],
                "198.51.100.2",
            ),
            (
                "10.0.0.1:1",
                &["198.51.100.1,10.1.1.1", "10.2.2.2"],
                "198.51.100.1",
            ),
            ("10.0.0.1:1", &["10.1.1.1"], "10.1.1.1"),
            ("10.0.0.1:1", &["garbage, 10.1.1.1"], "10.1.1.1"),
            ("10.0.0.1:1", &["198.51.100.1, garbage"], "10.0.0.1"),
            ("[2001:db8::1]:1", &["::ffff:198.51.100.1"], "198.51.100.1"),
            ("10.0.0.1:1", &[",,"], "10.0.0.1"),
            ("10.0.0.1:1", &["", "198.51.100.1"], "198.51.100.1"),
            ("10.0.0.1:1", &["198.51.100.1", ""], "198.51.100.1"),
            ("[::ffff:10.0.0.1]:1", &["fe80::1%eth0"], "fe80::1"),
        ] {
            let h = headers(
                &xff.iter()
                    .map(|v| ("X-Forwarded-For", *v))
                    .collect::<Vec<_>>(),
            );
            assert_eq!(
                client_ip(peer(remote), &h, &trusted).to_string(),
                want,
                "{remote}, {xff:?}"
            );
        }
    }

    // fit's TestRateMemoryBounded.
    #[test]
    fn rate_memory_bounded() {
        let (rl, clk) = rate(&akit_config::RateLimit {
            max_keys: 640,
            ..params(1.0, 1)
        });
        for i in 0..100_000u32 {
            let [_, a, b, c] = i.to_be_bytes();
            let ip = IpAddr::from([10, a, b, c]);
            let _ = rl.buckets.take(Key::Addr(ip), clk.load(Ordering::Relaxed));
            clk.fetch_add(1000, Ordering::Relaxed);
        }
        let n = rl.buckets.len();
        assert!(n > 0 && n <= 640, "{n} keys kept, want at most 640");
    }

    // fit's TestEvictPrefersFull.
    #[test]
    fn evict_prefers_full() {
        let b = Buckets::new(1.0, 2, SHARDS as u64 * 2);
        let full = Keyed {
            h: 1,
            key: Key::Text(1),
        };
        let busy = Keyed {
            h: 2,
            key: Key::Text(2),
        };
        for order in [[full, busy], [busy, full]] {
            let mut m = Map::default();
            for k in order {
                let tokens = if k == full { 2 * TOKEN } else { 0 };
                m.insert(k, Bucket { tokens, last: 0 });
            }
            b.evict(&mut m, 0);
            assert!(
                m.contains_key(&busy) && m.len() == 1,
                "evicted the busy key"
            );
        }
        // With none full, one goes all the same.
        let mut m = Map::default();
        m.insert(full, Bucket { tokens: 0, last: 0 });
        m.insert(busy, Bucket { tokens: 0, last: 0 });
        b.evict(&mut m, 0);
        assert_eq!(m.len(), 1);
    }

    // fit's TestRateIPv6Prefix: a client rotating through the addresses of its /64 shares one
    // bucket, and does not push other clients' out; 128 counts each address apart.
    #[test]
    fn rate_ipv6_prefix() {
        // Two keys a shard: the two clients' keys fit even in one shard (the shard a key goes to
        // is random).
        let (rl, _) = rate(&akit_config::RateLimit {
            max_keys: 2 * SHARDS as u64,
            ..params(1.0, 2)
        });
        let codes: Vec<u16> = (1..=200)
            .map(|i| status(&rl, &format!("[2001:db8:0:1::{i:x}]:1"), &[]))
            .collect();
        assert!(
            codes[0] == 200 && codes[1] == 200 && !codes[2..].contains(&200),
            "rotating through a /64: {codes:?}"
        );
        assert_eq!(status(&rl, "[2001:db8:0:2::1]:1", &[]), 200, "another /64");
        assert_eq!(rl.buckets.len(), 2);
        assert_eq!(status(&rl, "[::ffff:192.0.2.1]:1", &[]), 200);

        let (each, _) = rate(&akit_config::RateLimit {
            ipv6_prefix: 128,
            ..params(1.0, 1)
        });
        for i in 1..=3 {
            assert_eq!(status(&each, &format!("[2001:db8::{i:x}]:1"), &[]), 200);
        }
        let (wide, _) = rate(&akit_config::RateLimit {
            ipv6_prefix: 48,
            ..params(1.0, 1)
        });
        assert_eq!(status(&wide, "[2001:db8:0:1::1]:1", &[]), 200);
        assert_eq!(status(&wide, "[2001:db8:0:2::1]:1", &[]), 429);
    }

    // Rule 15: nothing a config or a clock can hold overflows.
    #[test]
    fn extremes() {
        let (rl, clk) = rate(&params(1e300, u64::MAX));
        assert_eq!(rl.admit(peer("192.0.2.1:1"), &Headers::new()), Ok(()));
        clk.store(i64::MAX, Ordering::Relaxed);
        assert_eq!(rl.admit(peer("192.0.2.1:1"), &Headers::new()), Ok(()));
        let (slow, clk) = rate(&params(0.001, 1));
        assert_eq!(slow.admit(peer("192.0.2.1:1"), &Headers::new()), Ok(()));
        assert_eq!(slow.admit(peer("192.0.2.1:1"), &Headers::new()), Err(1000));
        clk.store(i64::MIN, Ordering::Relaxed);
        assert!(slow.admit(peer("192.0.2.1:1"), &Headers::new()).is_err());
        assert_eq!(retry_after(i64::MAX), i64::MAX / TOKEN);
        assert_eq!(retry_after(1), 1);
        assert_eq!(retry_after(TOKEN), 1);
        assert_eq!(retry_after(TOKEN + 1), 2);
    }

    // fit's TestPeerAndName: zones and mapped addresses.
    #[test]
    fn addresses_parsed() {
        assert_eq!(parse_ip("fe80::1%eth0"), "fe80::1".parse().ok());
        assert_eq!(parse_ip("::ffff:192.0.2.1"), "192.0.2.1".parse().ok());
        assert_eq!(parse_ip(""), None);
        assert_eq!(
            unmap(peer("[::ffff:192.0.2.1]:80").ip()).to_string(),
            "192.0.2.1"
        );
    }
}
