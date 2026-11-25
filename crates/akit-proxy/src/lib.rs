#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's reverse proxy for HTTP, WebSocket and gRPC, with streamed bodies and linked backpressure.
//!
//! [`MODULES`] lists its handler for the registry. So far (I15): `proxy`, which forwards each
//! request to its upstream's pool (akit-upstream, on akio's HTTP/1 client) and sends the
//! upstream's response back, as fit's `handler/proxy` does:
//!
//! - The request goes out with its method, target, fields and body; the response comes back with
//!   its status, fields and body. Hop-by-hop fields (`Connection` and those it names,
//!   `Keep-Alive`, `Proxy-Connection`, `Proxy-Authenticate`, `Proxy-Authorization`, `TE`,
//!   `Trailer`, `Transfer-Encoding`, `Upgrade`) are dropped both ways, as RFC 9110 asks of a
//!   proxy. The fields keep their order and case.
//! - `Host` is the client's, unless `host_header` says otherwise.
//! - `X-Forwarded-For`, `X-Forwarded-Proto`, `X-Forwarded-Host` and `Forwarded` (RFC 7239) tell
//!   the upstream who the client is. What a client sends in them is believed only when its
//!   address is in `trusted_proxies`.
//! - The handler never blocks (rules 1–3): it detaches the request's context, hands the request
//!   to the pool, and answers from the pool's callbacks. A request that gets no response is
//!   answered as fit answers it: 502, or 503 when the pool is full or closed, or 504 when it
//!   timed out.
//! - The address is the one the upstream's sticky cookie names, when it has one and the address
//!   is up, or else the one its `load_balancing` picks, which for a hash policy hashes the
//!   client's address, the path the route matched and the query, or a field (I36). A response
//!   that should set the sticky cookie gets its `Set-Cookie` after the upstream's own.
//! - With an access log (I21), the request's line names the upstream address it went to and how
//!   long the upstream took to answer.
//!
//! I16 streams the bodies both ways with linked backpressure, as fit's proxy does (`relay.rs`):
//!
//! - A request body of more than [`STREAM_THRESHOLD`], or of no declared length (chunked, or
//!   HTTP/2 and HTTP/3 without `Content-Length`), reaches the proxy as it arrives
//!   ([`BodyMode::Stream`]) and goes up as it does, framed as it came: by its length, or chunked.
//!   While the upstream is behind, the client's reads are held. A smaller one arrives whole and
//!   goes up whole. Either is held to the listener's `limits.max_body_bytes` (and a
//!   `body_limit`'s, akio's `Context::limit_body`): one that declares a length past it is
//!   answered 413 before the handler runs, and one that grows past it 413 when it does (its
//!   response cut short if it began), as fit answers both.
//! - A response body of a length within 64 KiB is sent whole; any other as it arrives, the
//!   upstream's reads held while the client is behind. A streamed response keeps its length on
//!   every version (on HTTP/1.1 akio frames it by it, ADR-157), but on HTTP/1.1 one with
//!   trailers, which goes chunked to carry them. A response that breaks off mid-body closes the
//!   client's connection, which tells the client its body is short.
//!
//! I17 brings timeouts, cancellation and trailers, as fit's proxy has them:
//!
//! - **Timeouts.** A connect that times out (the upstream's `connect_timeout`) is answered 504.
//!   The upstream's `response_timeout`, and the proxy's own `timeout` when shorter, bound the
//!   whole exchange: past it before the response began, 504; later, the response is cut short as
//!   one the upstream broke off.
//! - **Cancellation.** A client that goes mid-download is noticed at the next write, or at once
//!   when the download is held for it (akio calls `on_drain` as a connection closes), and the
//!   upstream's response is given up. A client that goes mid-upload drops its body's sink, and
//!   the upstream's request is given up with it (its unfinished body closes its connection).
//!   Neither counts against the upstream's health.
//!
//! I17b brings the rest of fit's timeouts and cancellation, on akio's ADR-108:
//!
//! - **`response_header_timeout`**: an upstream that takes longer than it, once the request is
//!   sent whole, to begin its response is answered 504. **`response_idle_timeout`**: one that
//!   goes silent mid-body for longer (the time its reads are held for a slow client aside) has
//!   its response cut short, as one that broke off. Both are fit's 60s by default.
//! - **The client gone at any point**, its body all sent or not, the response's head come or
//!   not: each request's upstream exchange has a [`CancelToken`], cancelled when akio says the
//!   client went (`Context::on_gone`: its connection closed, its HTTP/1 EOF, its HTTP/2 or
//!   HTTP/3 stream reset), which gives the upstream request up wherever it is, a wait for a
//!   connection included; it does not count against the upstream's health, as fit's `OnCancel`.
//! - **`limits.write_stall_timeout`** is akio's engine `write_timeout`, set by akit-listener
//!   when the engine is built and on each reload: a client that takes nothing of what it was
//!   sent for that long is closed.
//! - **Trailers** pass both ways, as fit passes them. A request that announces trailers
//!   (`Trailer`) goes up chunked, its `Trailer` kept, with the announced fields of the trailers
//!   it ends with (one whose body came whole too); a request that announced none sends none on,
//!   as fit's. A response's trailers reach the client: a streamed one's with its `Trailer`
//!   announcement, and a small one, which is otherwise sent whole, is streamed to carry them.
//! - **A body cut short** closes an HTTP/1 client's connection, and resets an HTTP/2 or HTTP/3
//!   stream of a body of a declared length; one of no declared length ends with fit's
//!   `Fit-Error: upstream failed mid-body` trailer, which is all such a client could tell it by.
//!
//! I18 adds upstreams over h2 and h2c (the upstream's `protocol`): the same exchange, through
//! akit-upstream's h2 pool, the request's `Host` going as HTTP/2's `:authority`. Since I18b (akio
//! API requests A51–A53 and A66), it is the same relay in every way: a held h2 download keeps its
//! stream's window closed, an upload is paced by what waits in its stream, request trailers go
//! up, a request given up resets its stream alone, and an `h2` upstream whose server chooses
//! HTTP/1.1 in ALPN is spoken to over HTTP/1.1.
//!
//! I45 brings retries, as fit's `retry` (`retry.rs`): a request whose connection could not be
//! made (or whose address had `max_pending` requests waiting) is sent again whatever its method,
//! and one of an idempotent method also when it failed before its response began or was answered
//! with a status of `statuses`, up to `attempts` times, each to the address the upstream picks
//! next (one not tried yet, while there is one) with its sticky cookie. A request whose body
//! streams is never sent again. Retries are held to a budget (`budget_ratio` of the requests,
//! plus `budget_min_per_second`), and the proxy's `timeout` bounds the attempts together.
//!
//! I46 spreads requests over several upstreams, as fit's `upstreams`, `failover` and `mirror`:
//! each request goes to one of `upstreams` drawn at random by weight (or to `upstream`), or to
//! the `failover` when none of that one's addresses is in rotation and one of the failover's is
//! (akit-upstream's `Split`); the address, sticky cookie and retries are then the chosen
//! upstream's. A request whose body came whole may have a copy sent to the `mirror`
//! (`mirror.rs`), a share of them by `percent`, at most 256 under way: the copy's answer is
//! dropped, and the request never waits for it.
//!
//! I49 proxies WebSockets (`websocket.rs`), as fit's proxy passes on a request that asks to
//! switch protocols: an HTTP/1.1 upgrade, or an HTTP/2 or HTTP/3 extended CONNECT for
//! `websocket`, goes to an HTTP/1.1 upstream as its handshake; once the upstream answers `101`,
//! its connection leaves the pool (akio's ADR-113) and is joined to the client's (an HTTP/1.1
//! connection taken over, or an HTTP/2 or HTTP/3 stream answered with a streamed `200`), each
//! side's reads held while more than 64 KiB waits for the other, until the tunnel ends or carries
//! nothing for `tunnel_idle_timeout`. Any other answer is passed on as a response.
//!
//! I50 proxies gRPC calls (`grpc.rs`), as fit's proxy passes them: an HTTP/2 or HTTP/3 `POST`
//! of `application/grpc` (or a subtype) streams both ways as any request does, its body from its
//! head, to an `h2` or `h2c` upstream, with `TE: trailers`; its `grpc-timeout` bounds it as the
//! proxy's `timeout` does, and the upstream is told the shorter of the two. Its response is never
//! collected whole: a trailers-only one is passed on as one, any other as it arrives, its
//! trailers after it. A call that gets no response is answered as gRPC answers, a trailers-only
//! `200` with fit's `grpc-status` (`UNAVAILABLE`, `DEADLINE_EXCEEDED`, `RESOURCE_EXHAUSTED`,
//! `INTERNAL`), and one cut short ends with those trailers. A request cut short by the proxy's own
//! timeout, gRPC or not, does not count against its address (fit's `timedOut`).
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

mod grpc;
mod headers;
mod mirror;
mod relay;
mod retry;
mod websocket;

use std::borrow::Cow;
use std::sync::Arc;
use std::time::{Duration, Instant};

use akio_http::client::{CancelToken, ClientError, ClientRequest};
use akio_http::{BodyMode, Context, Method, Request, Response};
use akit_config::Network;
use akit_runtime::upstream::{Error, Keys, Split};
use akit_runtime::{
    Handler, HandlerParams, Info, Provision, ProvisionError, Provisioned, TLS, TRACEPARENT,
};

/// The largest request body the proxy takes whole: a larger one, or one of no declared length,
/// streams to it as it arrives (fit's listener `StreamThreshold`).
pub const STREAM_THRESHOLD: u64 = 64 << 10;

/// The handler modules of this crate, for [`akit_runtime::Registry::new`].
pub static MODULES: &[Info<HandlerParams>] = &[Info::handler(
    "http.handlers.proxy",
    "Proxies requests to an upstream, or to several by weight, with retries, mirroring and failover.",
    new,
)];

/// The `proxy` handler.
struct Proxy {
    /// The upstream, or the upstreams by weight, and the failover (I46).
    split: Split,
    /// The params' `mirror` (I46).
    mirror: Option<mirror::Mirror>,
    trusted: Vec<Network>,
    host_header: String,
    /// The params' `timeout`.
    timeout: Option<Duration>,
    /// The params' `retry`, compiled with its budget (I45).
    retry: Option<Arc<retry::Policy>>,
    /// The params' `tunnel_idle_timeout` (I49).
    tunnel_idle: Duration,
}

fn new(p: &Provision<'_, HandlerParams>) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    let HandlerParams::Proxy(params) = p.params() else {
        return Err(ProvisionError::new(
            "the proxy handler was given another's params",
        ));
    };
    let lookup = |field: String, name: &str| {
        p.env().upstream(name).cloned().ok_or_else(|| {
            ProvisionError::field(&field, format!("there is no upstream named {name:?}"))
        })
    };
    // fit's provisionSplit: the upstream, or each of upstreams, then failover and mirror.
    let failover = match &params.failover {
        Some(name) => Some(lookup("failover".into(), name)?),
        None => None,
    };
    let split = if params.upstreams.is_empty() {
        let pool = lookup("upstream".into(), &params.upstream)?;
        match failover {
            Some(f) => Split::new([(pool, 1)], Some(f)),
            None => Some(Split::one(pool)),
        }
    } else {
        let mut pools = Vec::with_capacity(params.upstreams.len());
        for (i, s) in params.upstreams.iter().enumerate() {
            pools.push((
                lookup(format!("upstreams[{i}].upstream"), &s.upstream)?,
                s.weight,
            ));
        }
        Split::new(pools, failover)
    };
    let split = split
        .ok_or_else(|| ProvisionError::field("upstream", "upstream or upstreams is needed"))?;
    let mirror = match &params.mirror {
        Some(m) => Some(mirror::Mirror::new(
            lookup("mirror.upstream".into(), &m.upstream)?,
            m.percent,
        )),
        None => None,
    };
    Ok(Provisioned::new(Arc::new(Proxy {
        split,
        mirror,
        trusted: params.trusted_proxies.clone(),
        host_header: params.host_header.clone(),
        timeout: params.timeout,
        retry: params
            .retry
            .as_ref()
            .map(|r| Arc::new(retry::Policy::new(r))),
        tunnel_idle: params.tunnel_idle_timeout,
    }) as Arc<dyn Handler>))
}

impl akio_http::Service for Proxy {
    fn call(&self, ctx: &mut Context) {
        // The route's timeout counts from here, retries included.
        let start = Instant::now();
        if let Some(r) = &self.retry {
            r.deposit();
        }
        let req = ctx.req();
        // A gRPC call (I50) is answered as gRPC answers when it fails: by its content type.
        let grpc = grpc::call(req).map(str::to_owned);
        // A request that asks to switch protocols (I49): only a WebSocket's extended CONNECT
        // has an HTTP/1.1 handshake to go as.
        let asked = websocket::asked(req);
        if asked == websocket::Asked::Other {
            ctx.respond(fail(501));
            return;
        }
        // The fields as the middleware before left them (a `headers` middleware's changes).
        let client = headers::Client {
            headers: &req.headers,
            peer: req.peer,
            tls: ctx.local(TLS).is_some(),
            traceparent: ctx.local(TRACEPARENT),
        };
        let Ok(out_fields) = headers::outgoing(&client, &self.trusted, &self.host_header) else {
            ctx.respond(failure(400, grpc.as_deref()));
            return;
        };
        // The address, by the upstream's sticky cookie and load balancing, which may hash the
        // client's address, the path and query, or a field (I36); and the sticky cookie the
        // response sets, copied only when there is one to set.
        let keys = Keys {
            peer: Some(req.peer.ip()),
            path: req.path(),
            query: req.query().unwrap_or(""),
            headers: Some(&req.headers),
        };
        // The upstream, by weight, or the failover (I46).
        let pool = self.split.pick();
        let backend = pool.pick_for(&keys);
        let set_cookie = pool.set_cookie(&keys, backend).map(str::to_owned);
        let fields = out_fields;
        // Cancelled when the client goes (`relay::start`), wherever the exchange is.
        let cancel = CancelToken::new();
        let mut out = ClientRequest::new(req.method.clone(), &target(ctx)).cancel_with(&cancel);
        out.headers = fields;
        // The shorter of it and the upstream's response_timeout holds (akit-upstream).
        out.timeout = self.timeout;
        if grpc.is_some() {
            // As gRPC servers ask of their clients; and the call's grpc-timeout bounds it too,
            // the upstream told the route's when that is the shorter.
            out.headers.append("TE", "trailers");
            let (timeout, told) = grpc::deadline(&out.headers, self.timeout);
            if let Some(told) = told {
                out.headers.set("grpc-timeout", told);
            }
            out.timeout = timeout;
        }
        // Past it, the request was cut short by the proxy, which is no failure of the address's.
        let deadline = out.timeout.and_then(|t| start.checked_add(t));
        let head = req.method == Method::Head;
        let version = req.version;
        let length = declared_length(req);
        if asked != websocket::Asked::No {
            let log = akit_observe::logged().then(|| relay::Logged {
                addr: pool.addresses().nth(backend).unwrap_or_default().to_owned(),
                start: Instant::now(),
            });
            let h = websocket::Handshake {
                out,
                cancel,
                backend,
                set_cookie,
                log,
                idle: self.tunnel_idle,
            };
            return websocket::serve(pool, ctx, asked, h);
        }
        // The trailers the request announced go up with it, chunked, as fit sends them.
        let names = headers::trailer_names(&req.headers);
        if !names.is_empty() {
            out.headers.append("Trailer", names.join(", "));
        }
        let announced = !names.is_empty();
        // A body that streams goes up chunked when it came so or carries trailers.
        let chunked = length.is_none() || announced;
        let Some((client, streamed)) = relay::start(ctx, chunked, names, cancel, grpc) else {
            // Answered already: nothing left to do.
            return;
        };
        // One that came whole goes up chunked only to carry trailers: a request without a body,
        // which declares no length either, goes as it came, with none.
        let chunked = if streamed { chunked } else { announced };
        if streamed {
            out = out.streamed(if chunked { None } else { length });
        } else if chunked {
            // Whole, with trailers, which only a chunked body carries: written up as a stream.
            out = out.streamed(None);
            let req = ctx.req();
            client.preload(&req.body, &req.trailers);
        } else {
            // A copy: the body is the request's, and goes on with the context (rule 6).
            out.body = ctx.req().body.clone();
        }
        // A request whose body went whole (or that has none) can be sent again (I45); one whose
        // body streams cannot.
        let again = match &self.retry {
            Some(policy) if !streamed && !chunked => {
                let req = ctx.req();
                Some(Box::new(retry::Again {
                    policy: Arc::clone(policy),
                    pool: pool.clone(),
                    req: out.clone(),
                    idempotent: retry::idempotent(&req.method),
                    peer: req.peer.ip(),
                    path: req.path().to_owned(),
                    query: req.query().unwrap_or("").to_owned(),
                    deadline: self.timeout.and_then(|t| start.checked_add(t)),
                    attempt: 0,
                    tried: retry::bit(backend),
                }))
            }
            _ => None,
        };
        // The access log's line names the address the request went to and how long it took
        // (fit's upstream and upstream_duration): the line is written as the response is sent.
        let log = akit_observe::logged().then(|| relay::Logged {
            addr: pool.addresses().nth(backend).unwrap_or_default().to_owned(),
            start: Instant::now(),
        });
        // A copy of a request whose body came whole goes to the mirror, if it is chosen and has
        // room (I46), sent first, as fit sends it; the request does not wait for it.
        if let Some(m) = &self.mirror {
            if !streamed {
                if let Some(slot) = m.slot() {
                    m.send(slot, mirror::copy_of(&out, &ctx.req().body));
                }
            }
        }
        let x = relay::Relay::new(client, head, version, chunked, log, set_cookie, again)
            .with_deadline(deadline);
        pool.stream(backend, out, x);
    }

    /// A body of more than [`STREAM_THRESHOLD`], or of no declared length, streams; a gRPC
    /// call's always does, from its head (fit's `StreamRequestBodyAtHeader`).
    fn body_mode(&self, req: &Request) -> BodyMode {
        if grpc::call(req).is_some() {
            return BodyMode::Stream;
        }
        match declared_length(req) {
            Some(n) if n <= STREAM_THRESHOLD => BodyMode::Buffered,
            _ => BodyMode::Stream,
        }
    }
}

/// The body length `req` declares (`Content-Length`), if any.
fn declared_length(req: &Request) -> Option<u64> {
    req.headers
        .get("content-length")
        .and_then(|v| v.trim().parse().ok())
}

impl Handler for Proxy {}

/// The target the upstream is asked for: the request's, its path canonical (akit-listener makes
/// it so before the route is picked) or as a `rewrite` middleware left it, its query as it came.
/// An absolute-form target (`http://host/path`) is sent in origin form.
fn target(ctx: &Context) -> String {
    origin_form(&ctx.req().target).into_owned()
}

/// `target` in origin form: an absolute-form one without its scheme and authority.
fn origin_form(target: &str) -> Cow<'_, str> {
    if target.starts_with('/') {
        return Cow::Borrowed(target);
    }
    let Some((_, rest)) = target.split_once("://") else {
        return Cow::Borrowed(target);
    };
    match rest.find(['/', '?']).and_then(|i| rest.get(i..)) {
        Some(path) if path.starts_with('/') => Cow::Borrowed(path),
        Some(query) => Cow::Owned(format!("/{query}")),
        None => Cow::Borrowed("/"),
    }
}

/// The status a request that got no response is answered with: fit's `statusOf`, for what
/// akit's requests can fail with: 504 for a timeout, the connect's and the head's included.
fn status_of(e: &Error) -> u16 {
    match e {
        Error::Busy | Error::Closed => 503,
        Error::Timeout => 504,
        Error::Client(e) if e.is_timeout() => 504,
        Error::Client(ClientError::Io(e)) if e.kind() == std::io::ErrorKind::TimedOut => 504,
        _ => 502,
    }
}

/// The answer to a request that got no response, with `status`: [`fail`]'s, or for a gRPC call
/// of the content type `grpc`, gRPC's (`grpc::fail`).
fn failure(status: u16, grpc: Option<&str>) -> Response {
    match grpc {
        Some(ct) => grpc::fail(status, ct),
        None => fail(status),
    }
}

/// fit's answer with `status`: its code and reason, as text.
fn fail(status: u16) -> Response {
    let reason = match status {
        400 => "Bad Request",
        413 => "Request Entity Too Large",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Bad Gateway",
    };
    Response::text(status, format!("{status} {reason}\n"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    #[test]
    fn origin_forms() {
        for (target, want) in [
            ("/a%2Fb/c?x=1", "/a%2Fb/c?x=1"),
            ("http://h.example/p?q", "/p?q"),
            ("http://h.example?q", "/?q"),
            ("http://h.example", "/"),
            ("*", "*"),
        ] {
            assert_eq!(super::origin_form(target), want);
        }
    }

    #[test]
    fn failures() {
        use akit_runtime::upstream::Error;
        assert_eq!(super::status_of(&Error::Busy), 503);
        assert_eq!(super::status_of(&Error::Closed), 503);
        assert_eq!(super::status_of(&Error::NoEngine), 502);
        assert_eq!(super::status_of(&Error::Timeout), 504);
        let timed_out = std::io::Error::from(std::io::ErrorKind::TimedOut);
        let refused = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
        use akio_http::client::ClientError;
        assert_eq!(
            super::status_of(&Error::Client(ClientError::Io(timed_out))),
            504
        );
        assert_eq!(
            super::status_of(&Error::Client(ClientError::Io(refused))),
            502
        );
        assert_eq!(super::status_of(&Error::Client(ClientError::Timeout)), 504);
        assert_eq!(
            super::status_of(&Error::Client(ClientError::HeaderTimeout)),
            504
        );
        assert_eq!(
            super::status_of(&Error::Client(ClientError::Cancelled)),
            502
        );
        let r = super::fail(502);
        assert_eq!(r.status, 502);
        assert_eq!(r.body, b"502 Bad Gateway\n");
    }
}
