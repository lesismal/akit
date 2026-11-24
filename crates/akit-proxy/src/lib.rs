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
//!   to the pool, and answers from the pool's completion callback. A request that gets no
//!   response is answered as fit answers it: 502, or 503 when the pool is full or closed, or 504
//!   when it timed out.
//!
//! Bodies travel whole for now: a request body within the listener's `limits.max_body_bytes`
//! (one past it is answered 413 by the listener before the handler runs, as fit answers it), a
//! response body within akio's client limit (64 MiB). Streaming them both ways with linked
//! backpressure (I16) waits for akio API requests A16 to A18: akio's client takes a request body
//! only whole and buffers each response whole, and a server request cannot hold its client's
//! reads or hear when it drained. I17 brings the timeouts, cancellation and trailers.
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

mod headers;

use std::borrow::Cow;
use std::sync::Arc;

use akio_http::client::{ClientError, ClientRequest};
use akio_http::{Context, Method, Response};
use akit_config::Network;
use akit_runtime::upstream::{Error, Outcome, Pool};
use akit_runtime::{
    CANONICAL_PATH, Handler, HandlerParams, Info, Provision, ProvisionError, Provisioned, TLS,
};

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
        // A copy: the body is the request's, and goes on with the context (rule 6). I16 streams
        // it instead.
        out.body = req.body.clone();
        let head = req.method == Method::Head;
        let Some(d) = ctx.detach() else {
            // Answered already: nothing left to do.
            return;
        };
        self.pool
            .request(out, move |outcome| d.respond(response(outcome, head)));
    }
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

/// The client's response for the upstream's outcome: the upstream's response, its fields edited
/// ([`headers::incoming`]), or fit's answer to a request that got none.
fn response(outcome: Outcome, head: bool) -> Response {
    match outcome {
        Ok(up) => {
            let mut resp = Response::new(up.status);
            resp.headers = headers::incoming(&up.headers, head);
            resp.body = up.body;
            resp
        }
        Err(e) => fail(status_of(&e)),
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
