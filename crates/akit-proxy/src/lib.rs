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
//! - With an access log (I21), the request's line names the upstream address it went to and how
//!   long the upstream took to answer.
//!
//! I16 streams the bodies both ways with linked backpressure, as fit's proxy does (`relay.rs`):
//!
//! - A request body of more than [`STREAM_THRESHOLD`], or of no declared length (chunked, or
//!   HTTP/2 and HTTP/3 without `Content-Length`), reaches the proxy as it arrives
//!   ([`BodyMode::Stream`]) and goes up as it does, framed as it came: by its length, or chunked.
//!   While the upstream is behind, the client's reads are held. A smaller one arrives whole and
//!   goes up whole. Either is held to the listener's `limits.max_body_bytes`: one that declares
//!   a length past it is answered 413 before the handler runs, and one that grows past it 413
//!   when it does, as fit answers both.
//! - A response body of a length within 64 KiB is sent whole; any other as it arrives, the
//!   upstream's reads held while the client is behind. akio sends an HTTP/1.1 response that
//!   streams chunked, its length dropped, until it can frame one by its length (akio API request
//!   A35); HTTP/2 and HTTP/3 keep it. A response that breaks off mid-body closes the client's
//!   connection, which tells the client its body is short.
//!
//! I17 brings the timeouts (fit's `write_stall_timeout` for a client that stops reading among
//! them), cancellation when the client goes, and the trailers' announcements.
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

mod headers;
mod relay;

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Instant;

use akio_http::client::{ClientError, ClientRequest};
use akio_http::{BodyMode, Context, Method, Request, Response};
use akit_config::Network;
use akit_runtime::upstream::{Error, Pool};
use akit_runtime::{
    CANONICAL_PATH, Handler, HandlerParams, Info, MAX_BODY_BYTES, Provision, ProvisionError,
    Provisioned, TLS,
};

/// The largest request body the proxy takes whole: a larger one, or one of no declared length,
/// streams to it as it arrives (fit's listener `StreamThreshold`).
pub const STREAM_THRESHOLD: u64 = 64 << 10;

/// The handler modules of this crate, for [`akit_runtime::Registry::new`].
pub static MODULES: &[Info<HandlerParams>] = &[Info::handler(
    "http.handlers.proxy",
    "Proxies requests to an upstream.",
    new,
)];

/// The `proxy` handler.
struct Proxy {
    pool: Pool,
    trusted: Vec<Network>,
    host_header: String,
}

fn new(p: &Provision<'_, HandlerParams>) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    let HandlerParams::Proxy(params) = p.params() else {
        return Err(ProvisionError::new(
            "the proxy handler was given another's params",
        ));
    };
    let pool = p.env().upstream(&params.upstream).cloned().ok_or_else(|| {
        ProvisionError::field(
            "upstream",
            format!("there is no upstream named {:?}", params.upstream),
        )
    })?;
    Ok(Provisioned::new(Arc::new(Proxy {
        pool,
        trusted: params.trusted_proxies.clone(),
        host_header: params.host_header.clone(),
    }) as Arc<dyn Handler>))
}

impl akio_http::Service for Proxy {
    fn call(&self, ctx: &mut Context) {
        let req = ctx.req();
        let client = headers::Client {
            headers: &req.headers,
            peer: req.peer,
            tls: ctx.local(TLS).is_some(),
        };
        let Ok(fields) = headers::outgoing(&client, &self.trusted, &self.host_header) else {
            ctx.respond(fail(400));
            return;
        };
        let mut out = ClientRequest::new(req.method.clone(), &target(ctx));
        out.headers = fields;
        let head = req.method == Method::Head;
        let version = req.version;
        let length = declared_length(req);
        let limit = ctx
            .local(MAX_BODY_BYTES)
            .and_then(|v| v.parse().ok())
            .unwrap_or(akit_config::DEFAULT_MAX_BODY_BYTES);
        let chunked = length.is_none();
        let Some((client, streamed)) = relay::start(ctx, limit, chunked) else {
            // Answered already: nothing left to do.
            return;
        };
        if streamed {
            // Framed as it came: by its length, or chunked.
            out = out.streamed(length);
        } else {
            // A copy: the body is the request's, and goes on with the context (rule 6).
            out.body = ctx.req().body.clone();
        }
        let backend = self.pool.pick();
        // The access log's line names the address the request went to and how long it took
        // (fit's upstream and upstream_duration): the line is written as the response is sent.
        let log = akit_observe::logged().then(|| relay::Logged {
            addr: self
                .pool
                .addresses()
                .nth(backend)
                .unwrap_or_default()
                .to_owned(),
            start: Instant::now(),
        });
        let x = relay::Relay::new(client, head, version, chunked && streamed, log);
        self.pool.stream(backend, out, x);
    }

    /// A body of more than [`STREAM_THRESHOLD`], or of no declared length, streams.
    fn body_mode(&self, req: &Request) -> BodyMode {
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

/// The target the upstream is asked for: the request's canonical path, which the route matched
/// (akit-listener's [`CANONICAL_PATH`] when it differs from the one sent), and its query, as
/// they came. An absolute-form target (`http://host/path`) is sent in origin form.
fn target(ctx: &Context) -> String {
    let req = ctx.req();
    let sent = origin_form(&req.target);
    match ctx.local(CANONICAL_PATH) {
        Some(path) => match sent.split_once('?') {
            Some((_, query)) => format!("{path}?{query}"),
            None => path.to_owned(),
        },
        None => sent.into_owned(),
    }
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
/// I15's requests can fail with.
fn status_of(e: &Error) -> u16 {
    match e {
        Error::Busy | Error::Closed => 503,
        Error::Timeout | Error::Client(ClientError::Timeout) => 504,
        _ => 502,
    }
}

/// fit's answer with `status`: its code and reason, as text.
fn fail(status: u16) -> Response {
    let reason = match status {
        400 => "Bad Request",
        413 => "Request Entity Too Large",
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
        let r = super::fail(502);
        assert_eq!(r.status, 502);
        assert_eq!(r.body, b"502 Bad Gateway\n");
    }
}
