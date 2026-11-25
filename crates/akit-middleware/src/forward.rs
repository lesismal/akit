//! The `forward_auth` middleware, as fit's `middleware/auth` `ForwardAuth`: each request is first
//! asked about at an auth service, an upstream of the config's.
//!
//! - The auth service is sent a `GET` of `uri`, on akit-upstream's pool (akio's asynchronous
//!   client), with the request's fields (those `request_headers` names, or all but the hop-by-hop
//!   ones, those `Connection` names and those of the body), its `Host`, and where it was going in
//!   `X-Forwarded-Method`, `-Proto`, `-Host`, `-Uri` and `-For`.
//! - The request waits without holding the I/O thread (rule 1): it is suspended
//!   ([`Context::suspend`], akio's ADR-109) and resumed from the pool's callbacks, on whichever
//!   I/O thread the answer arrives.
//! - A 2xx answer lets it through to the next middleware and the handler, with the fields
//!   `copy_headers` names taken from the answer. Any of these the client sent, in any spelling
//!   an application behind CGI reads as the same field, is removed first, so that a client
//!   cannot pass them off as the auth service's. The answer's body is not wanted: one of a small
//!   declared length is read and dropped, so that the connection is kept, and any other given up.
//! - Any other answer is the client's: its status, its fields but the hop-by-hop ones, and its
//!   body when it is within [`WHOLE_BODY`] (a login redirect, a 401), as fit passes it; a larger
//!   one is given up and the answer goes without it.
//! - An auth service that does not answer within `timeout` is answered 504; one that cannot be
//!   reached, or fails, 502; a pool closed meanwhile (its config replaced) 503, as fit answers a
//!   check its drain ended.
//! - A client that goes while its request waits cancels the auth service's request (akio's
//!   `on_gone` and `CancelToken`, ADR-108), which does not count against the auth service's
//!   health.
//!
//! akio cannot suspend a request whose body streams to its handler (ADR-109): as `basic_auth`
//! does, the middleware asks for such a body whole, within the listener's `max_body_bytes`, and
//! the handler then has it whole (akio API request A75 would let it stream after the check).

use std::sync::Arc;
use std::time::Duration;

use akio_http::client::{CancelToken, ClientError, ClientRequest};
use akio_http::{BodyMode, Context, Headers, Method, Request, Response, Suspended};
use akit_runtime::upstream::{Download, Error, Exchange, Head, Keys, Pool};
use akit_runtime::{
    Handler, HandlerParams, Middleware, Provision, ProvisionError, Provisioned, TLS,
    set_request_headers,
};

use crate::auth::same_field;
use crate::url::{canonical_key, escaped_path};

/// The largest answer body passed on with a refusal, and the largest an allowing answer's that
/// is read (and dropped) rather than given up: fit's pool `StreamThreshold`.
pub(crate) const WHOLE_BODY: usize = 64 << 10;

/// The fields the middleware sets on the auth service's request, which a request's own are not
/// sent in place of.
const FORWARDED: [&str; 5] = [
    "X-Forwarded-Method",
    "X-Forwarded-Proto",
    "X-Forwarded-Host",
    "X-Forwarded-Uri",
    "X-Forwarded-For",
];

pub(crate) fn new(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Middleware>, ProvisionError> {
    let HandlerParams::ForwardAuth(f) = p.params() else {
        return Err(ProvisionError::new(
            "the forward_auth middleware was given another's params",
        ));
    };
    let pool = p.env().upstream(&f.upstream).cloned().ok_or_else(|| {
        ProvisionError::field(
            "upstream",
            format!("there is no upstream named {:?}", f.upstream),
        )
    })?;
    Ok(Provisioned::new(
        Arc::new(ForwardMiddleware(Arc::new(Forward::new(f, pool)?))) as Arc<dyn Middleware>,
    ))
}

/// The `forward_auth` middleware, compiled.
struct Forward {
    pool: Pool,
    /// `uri`, its path escaped as Go writes it.
    uri: String,
    /// `request_headers`, canonical; every field but those [`sent_by_default`] refuses when
    /// empty.
    request_headers: Vec<String>,
    /// `copy_headers`, canonical.
    copy_headers: Vec<String>,
    timeout: Duration,
}

impl std::fmt::Debug for Forward {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Forward")
            .field("upstream", &self.pool.name())
            .field("uri", &self.uri)
            .field("request_headers", &self.request_headers)
            .field("copy_headers", &self.copy_headers)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl Forward {
    fn new(f: &akit_config::ForwardAuth, pool: Pool) -> Result<Forward, ProvisionError> {
        let uri = if f.uri.is_empty() { "/" } else { &f.uri };
        // akit-config checked these; this keeps a config built another way sound.
        if !uri.starts_with('/') || uri.bytes().any(|c| c < 0x20 || c == 0x7f) {
            return Err(ProvisionError::field(
                "uri",
                format!("{uri:?} is not a path, with a query or not"),
            ));
        }
        let uri = match uri.split_once('?') {
            Some((path, query)) => format!("{}?{query}", escaped_path(path)),
            None => escaped_path(uri).into_owned(),
        };
        let names = |field: &str, list: &[String]| {
            list.iter()
                .map(|n| {
                    if valid_name(n) {
                        Ok(canonical_key(n))
                    } else {
                        Err(ProvisionError::field(
                            field,
                            format!("{n:?} is not a field name"),
                        ))
                    }
                })
                .collect::<Result<Vec<_>, _>>()
        };
        Ok(Forward {
            pool,
            uri,
            request_headers: names("request_headers", &f.request_headers)?,
            copy_headers: names("copy_headers", &f.copy_headers)?,
            timeout: if f.timeout.is_zero() {
                akit_config::DEFAULT_FORWARD_AUTH_TIMEOUT
            } else {
                f.timeout
            },
        })
    }

    /// The fields of the auth service's request for the one `ctx` serves.
    fn fields(&self, ctx: &Context) -> Headers {
        let req = ctx.req();
        let fields = &req.headers;
        let mut h = Headers::with_capacity(fields.len().saturating_add(6), 0);
        let ours = |n: &str| {
            n.eq_ignore_ascii_case("host") || FORWARDED.iter().any(|f| f.eq_ignore_ascii_case(n))
        };
        if self.request_headers.is_empty() {
            let named = connection_names(fields);
            for (n, v) in fields.iter() {
                if !ours(n)
                    && sent_by_default(n)
                    && !named.iter().any(|c| c.eq_ignore_ascii_case(n))
                {
                    h.append(n, v);
                }
            }
        } else {
            for name in &self.request_headers {
                if ours(name) {
                    continue;
                }
                for (_, v) in fields.iter().filter(|(n, _)| n.eq_ignore_ascii_case(name)) {
                    h.append(name, v);
                }
            }
        }
        let host = fields.get("host").unwrap_or("");
        h.append("X-Forwarded-Method", req.method.as_str());
        h.append(
            "X-Forwarded-Proto",
            if ctx.local(TLS).is_some() {
                "https"
            } else {
                "http"
            },
        );
        h.append("X-Forwarded-Host", host);
        h.append("X-Forwarded-Uri", origin_form(&req.target));
        h.append("X-Forwarded-For", req.peer.ip().to_string());
        if !host.is_empty() {
            h.append("Host", host);
        }
        h
    }

    /// Lets the request through: any field that passes for one of `copy_headers` is removed,
    /// and those of the answer's (`copied`) set.
    fn admit(&self, ctx: &mut Context, copied: &[(String, String)]) {
        if self.copy_headers.is_empty() {
            return;
        }
        let fields = &ctx.req().headers;
        let mut kept = Headers::with_capacity(fields.len().saturating_add(copied.len()), 0);
        for (n, v) in fields
            .iter()
            .filter(|(n, _)| !self.copy_headers.iter().any(|c| same_field(n, c)))
        {
            kept.append(n, v);
        }
        for (n, v) in copied {
            kept.append(n, v);
        }
        set_request_headers(ctx, kept);
    }

    /// The fields of an allowing answer that `copy_headers` names, under their names.
    fn copied(&self, answer: &Headers) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for name in &self.copy_headers {
            for (_, v) in answer.iter().filter(|(n, _)| n.eq_ignore_ascii_case(name)) {
                out.push((name.clone(), v.to_owned()));
            }
        }
        out
    }
}

/// Whether `name` is an HTTP token, what a field name must be: fit's `validName`.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c))
}

/// Whether the field `name` is hop-by-hop (RFC 9110), as fit's `isHopByHop` lists them.
fn is_hop_by_hop(name: &str) -> bool {
    [
        "connection",
        "keep-alive",
        "proxy-connection",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    ]
    .iter()
    .any(|h| h.eq_ignore_ascii_case(name))
}

/// Whether a request's field `name` goes to the auth service when `request_headers` is left
/// out: all but the hop-by-hop ones and those of the body.
fn sent_by_default(name: &str) -> bool {
    !is_hop_by_hop(name)
        && !["content-length", "content-type", "expect"]
            .iter()
            .any(|h| h.eq_ignore_ascii_case(name))
}

/// The names the `Connection` fields list.
fn connection_names(h: &Headers) -> Vec<&str> {
    h.iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case("connection"))
        .flat_map(|(_, v)| v.split(','))
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .collect()
}

/// `target` in origin form, as Go's `URL.RequestURI` gives it: an absolute-form one without its
/// scheme and authority.
fn origin_form(target: &str) -> &str {
    if target.starts_with('/') {
        return target;
    }
    match target.split_once("://") {
        Some((_, rest)) => match rest.find('/') {
            Some(i) => rest.get(i..).unwrap_or("/"),
            None => "/",
        },
        None => target,
    }
}

/// The `forward_auth` middleware's handle.
#[derive(Debug)]
struct ForwardMiddleware(Arc<Forward>);

impl Middleware for ForwardMiddleware {
    fn wrap(&self, next: Arc<dyn Handler>) -> Arc<dyn Handler> {
        Arc::new(ForwardAuthed {
            fa: Arc::clone(&self.0),
            next,
        })
    }
}

struct ForwardAuthed {
    fa: Arc<Forward>,
    next: Arc<dyn Handler>,
}

impl akio_http::Service for ForwardAuthed {
    fn call(&self, ctx: &mut Context) {
        let fa = &self.fa;
        let mut out = ClientRequest::new(Method::Get, &fa.uri);
        out.headers = fa.fields(ctx);
        out.timeout = Some(fa.timeout);
        let token = CancelToken::new();
        out = out.cancel_with(&token);
        let backend = {
            let (path, query) = fa.uri.split_once('?').unwrap_or((&fa.uri, ""));
            fa.pool.pick_for(&Keys {
                peer: None,
                path,
                query,
                headers: Some(&out.headers),
            })
        };
        // The client going cancels the check; it is carried to the suspended request.
        let gone = token.clone();
        ctx.on_gone(move || gone.cancel());
        let Some(suspended) = ctx.suspend() else {
            // Not with a body that streams, which `body_mode` asks for whole.
            ctx.respond(Response::new(500));
            return;
        };
        fa.pool.stream(
            backend,
            out,
            Check {
                fa: Arc::clone(fa),
                next: Arc::clone(&self.next),
                waiting: Some(suspended),
                token,
                refusal: None,
            },
        );
    }

    /// The handler's, except that a body that would stream comes whole, so that the request can
    /// be suspended.
    fn body_mode(&self, req: &Request) -> BodyMode {
        match self.next.body_mode(req) {
            BodyMode::Stream => BodyMode::Buffered,
            mode => mode,
        }
    }

    fn streams_bodies(&self) -> bool {
        self.next.streams_bodies()
    }
}

impl Handler for ForwardAuthed {}

/// A refusal being read: the answer the client gets once its body is in.
struct Refusal {
    request: Suspended,
    status: u16,
    headers: Headers,
    body: Vec<u8>,
    up: Download,
}

impl Refusal {
    /// Answers the client with the refusal, its body if `whole`.
    fn send(self, whole: bool) {
        let mut resp = Response::new(self.status);
        resp.headers = self.headers;
        if whole {
            resp.body = self.body;
        }
        self.request.resume(move |ctx| ctx.respond(resp));
    }
}

/// One request's check: the auth service's answer, as it arrives (akit-upstream's
/// [`Exchange`]). Its methods run on an I/O thread and never block.
struct Check {
    fa: Arc<Forward>,
    next: Arc<dyn Handler>,
    /// The request, until the answer's head (or the failure) is dealt with.
    waiting: Option<Suspended>,
    token: CancelToken,
    refusal: Option<Refusal>,
}

/// Whether a response of `status` has no body.
fn bodyless(status: u16) -> bool {
    matches!(status, 100..=199 | 204 | 304)
}

impl Exchange for Check {
    fn on_head(&mut self, head: Head, up: Download) {
        let Some(request) = self.waiting.take() else {
            up.cancel();
            return;
        };
        let length = head
            .headers
            .get("content-length")
            .and_then(|v| v.trim().parse::<u64>().ok());
        let small = bodyless(head.status) || length.is_some_and(|n| n <= WHOLE_BODY as u64);
        if (200..300).contains(&head.status) {
            // Its body is not wanted: a small one is read and dropped, any other given up.
            if !small {
                up.cancel();
            }
            let copied = self.fa.copied(&head.headers);
            let (fa, next) = (Arc::clone(&self.fa), Arc::clone(&self.next));
            request.resume(move |ctx| {
                fa.admit(ctx, &copied);
                next.call(ctx);
            });
            return;
        }
        let mut headers = Headers::with_capacity(head.headers.len(), 0);
        for (n, v) in head.headers.iter() {
            if !is_hop_by_hop(n) && !n.eq_ignore_ascii_case("content-length") {
                headers.append(n, v);
            }
        }
        let refusal = Refusal {
            request,
            status: head.status,
            headers,
            body: Vec::new(),
            up,
        };
        if small {
            self.refusal = Some(refusal);
        } else {
            // Too large to pass on: the answer goes without it.
            refusal.up.cancel();
            refusal.send(false);
        }
    }

    fn on_data(&mut self, chunk: &[u8]) {
        let Some(r) = &mut self.refusal else { return };
        if r.body.len().saturating_add(chunk.len()) > WHOLE_BODY {
            if let Some(r) = self.refusal.take() {
                r.up.cancel();
                r.send(false);
            }
            return;
        }
        r.body.extend_from_slice(chunk);
    }

    fn on_end(&mut self, _trailers: &Headers) {
        if let Some(r) = self.refusal.take() {
            r.send(true);
        }
    }

    fn on_error(&mut self, e: Error) {
        if let Some(r) = self.refusal.take() {
            // Its body broke off: the answer goes without it.
            r.send(false);
            return;
        }
        let Some(request) = self.waiting.take() else {
            return;
        };
        if self.token.is_cancelled() {
            // The client went: nobody to answer.
            drop(request);
            return;
        }
        let status = status_of(&e);
        request.resume(move |ctx| ctx.respond(Response::new(status)));
    }

    fn gave_up(&self) -> bool {
        self.token.is_cancelled()
    }
}

/// The status a check that got no answer is answered with, as fit's: 504 past the timeout, 503
/// when the pool was closed (its config replaced), else 502.
fn status_of(e: &Error) -> u16 {
    match e {
        Error::Timeout | Error::Client(ClientError::Timeout) => 504,
        Error::Closed => 503,
        _ => 502,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn field_names() {
        for (n, ok) in [
            ("X-Auth-User", true),
            ("x_y", true),
            ("a b", false),
            ("x:y", false),
            ("", false),
        ] {
            assert_eq!(valid_name(n), ok, "{n:?}");
        }
    }

    #[test]
    fn origin_forms() {
        assert_eq!(origin_form("/p?q=1"), "/p?q=1");
        assert_eq!(origin_form("http://h.example/p?q"), "/p?q");
        assert_eq!(origin_form("http://h.example"), "/");
        assert_eq!(origin_form("*"), "*");
    }

    #[test]
    fn fields_sent_by_default() {
        for (n, sent) in [
            ("Authorization", true),
            ("Cookie", true),
            ("Content-Length", false),
            ("content-type", false),
            ("Expect", false),
            ("Transfer-Encoding", false),
            ("Proxy-Authorization", false),
            ("TE", false),
        ] {
            assert_eq!(sent_by_default(n), sent, "{n}");
        }
        let mut h = Headers::new();
        h.append("Connection", "keep-alive, X-Secret ,");
        h.append("connection", "Other");
        assert_eq!(connection_names(&h), ["keep-alive", "X-Secret", "Other"]);
    }

    #[test]
    fn failures() {
        assert_eq!(status_of(&Error::Timeout), 504);
        assert_eq!(status_of(&Error::Client(ClientError::Timeout)), 504);
        assert_eq!(status_of(&Error::Closed), 503);
        assert_eq!(status_of(&Error::Busy), 502);
        assert_eq!(status_of(&Error::NoEngine), 502);
        let refused = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
        assert_eq!(status_of(&Error::Client(ClientError::Io(refused))), 502);
    }
}
