//! The checks of HTTP requests against a policy (I121), as fit's `mediaauth/http.go`: the
//! credentials a request carries ([`credentials`]), the guard that lets it on or answers it
//! ([`guard`]), and the answers of a refusal ([`refuse`]).
//!
//! A token taken from the query, and any `token` parameter, is written `REDACTED` in the
//! request's target before anything answers, so that the access log, which writes the target the
//! response goes with, never holds it (fit's `Redact`).

use std::sync::Arc;

use akio_http::{Context, Request, Response};
use akit_config::media::MediaAction as Action;

use std::time::Duration;

use crate::auth::{Credentials, Policy, Refusal, unescape};
use crate::urlsign::{Grant as UrlGrant, Seen, TouchError, UrlReason};

/// The cookie a token may come in, as the media pages set it: fit's `fit_media_token`, renamed
/// (decision 037).
pub const COOKIE: &str = "akit_media_token";

/// The challenge of a 401: a bearer token, said to be invalid when one was sent (RFC 6750
/// section 3); the realm akit's (decision 037).
const CHALLENGE: &str = "Bearer realm=\"akit\"";
const CHALLENGE_INVALID: &str = "Bearer realm=\"akit\", error=\"invalid_token\"";

/// The credentials of an HTTP request, the first of: `Authorization: Bearer`,
/// `Authorization: Basic`, the `token` query parameter and the `akit_media_token` cookie (fit's
/// `FromHTTP`).
#[must_use]
pub fn credentials(req: &Request) -> Credentials {
    if let Some(h) = req.headers.get("authorization") {
        let (scheme, rest) = h.split_once(' ').unwrap_or((h, ""));
        let rest = rest.trim();
        if scheme.eq_ignore_ascii_case("bearer") && !rest.is_empty() {
            return Credentials {
                token: rest.to_owned(),
                ..Credentials::default()
            };
        }
        if scheme.eq_ignore_ascii_case("basic") {
            if let Some((u, p)) = basic(rest) {
                if !u.is_empty() {
                    return Credentials {
                        user: u,
                        password: p,
                        ..Credentials::default()
                    };
                }
            }
        }
    }
    if let Some(q) = req.query().filter(|q| q.contains("token=")) {
        if let Some(t) = query_param(q, "token").filter(|t| !t.is_empty()) {
            return Credentials {
                token: t,
                ..Credentials::default()
            };
        }
    }
    if let Some(t) = cookie(req, COOKIE).filter(|t| !t.is_empty()) {
        return Credentials {
            token: t,
            ..Credentials::default()
        };
    }
    Credentials::default()
}

/// A Basic authorization's user and password, as Go's `Request.BasicAuth` reads them.
fn basic(b64: &str) -> Option<(String, String)> {
    let raw = akio_crypto::base64::STANDARD.decode(b64).ok()?;
    let s = String::from_utf8(raw).ok()?;
    let (u, p) = s.split_once(':')?;
    Some((u.to_owned(), p.to_owned()))
}

/// The first value of the query parameter `key`, decoded.
#[must_use]
pub fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (unescape(k).as_deref() == Some(key)).then(|| unescape(v).unwrap_or_default())
    })
}

/// The value of the cookie `name`, the first one sent.
fn cookie(req: &Request, name: &str) -> Option<String> {
    req.headers
        .get_all("cookie")
        .flat_map(|line| line.split(';'))
        .find_map(|part| {
            let (k, v) = part.trim().split_once('=')?;
            (k == name).then(|| v.trim_matches('"').to_owned())
        })
}

/// The request's target with the value of every `token` and `sig` parameter of its query written
/// `REDACTED` (fit's `Redact`); `None` when there is none to write.
#[must_use]
pub fn redacted(target: &str) -> Option<String> {
    let (path, query) = target.split_once('?')?;
    let needs = query.split('&').any(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (k == "token" || k == "sig") && v != "REDACTED"
    });
    if !needs {
        return None;
    }
    let q: Vec<String> = query
        .split('&')
        .map(|kv| {
            let k = kv.split_once('=').map_or(kv, |(k, _)| k);
            if k == "token" || k == "sig" {
                format!("{k}=REDACTED")
            } else {
                kv.to_owned()
            }
        })
        .collect();
    Some(format!("{path}?{}", q.join("&")))
}

/// Writes the request's tokens `REDACTED` in its target ([`redacted`]), for the access log.
pub fn redact(ctx: &mut Context) {
    let Some(t) = redacted(&ctx.req().target) else {
        return;
    };
    if let Some(r) = ctx.req_mut() {
        r.target = t;
    }
}

/// Answers a request a check refused, as fit's `WriteError`: the status [`Refusal::status`] gives
/// and a line of text; a 401 with a `WWW-Authenticate` challenge for a bearer token, a 503 with
/// `Retry-After: 1`. `cors`, when not empty, goes as `Access-Control-Allow-Origin`, so that a
/// player on another origin reads why.
pub fn refuse(ctx: &mut Context, r: Refusal, cors: &str) {
    let msg = match r {
        Refusal::NoCredentials => "credentials are needed: a bearer token, or a user and password",
        Refusal::BadCredentials => "the credentials are not valid",
        Refusal::Forbidden => "the credentials do not allow this",
        Refusal::Busy | Refusal::Pending => "too many password checks are waiting",
        Refusal::Unavailable => "the media_auth policy is not loaded",
    };
    let mut resp = Response::with_body(
        r.status(),
        "text/plain; charset=utf-8",
        format!("{msg}\n").into_bytes(),
    );
    match r {
        Refusal::NoCredentials => resp.headers.set("WWW-Authenticate", CHALLENGE),
        Refusal::BadCredentials => resp.headers.set("WWW-Authenticate", CHALLENGE_INVALID),
        Refusal::Forbidden => {}
        Refusal::Busy | Refusal::Pending | Refusal::Unavailable => {
            resp.headers.set("Retry-After", "1");
        }
    }
    if !cors.is_empty() {
        resp.headers.set("Access-Control-Allow-Origin", cors);
    }
    ctx.respond(resp);
}

/// What let a request in: the subject it is authenticated as (empty for an action the policy
/// makes public, or for no policy), the token it carried in its query, and the signed URL or
/// grant it was let in by (I144), which a playlist hands on to what it names (fit's `Access`).
#[derive(Clone, Default)]
pub struct Access {
    /// Who it is.
    pub subject: String,
    /// The token of its query, or empty.
    pub query_token: String,
    /// The signed URL or grant that let it in.
    url: Option<Arc<ByUrl>>,
}

/// A request let in by a signed URL or a grant.
struct ByUrl {
    policy: Arc<Policy>,
    resource: String,
    grant: UrlGrant,
    key: Vec<u8>,
    tls: bool,
}

impl std::fmt::Debug for Access {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Access")
            .field("subject", &self.subject)
            .field("url", &self.url.is_some())
            .finish_non_exhaustive()
    }
}

impl Access {
    /// Whether the request was let in by a signed URL or a grant.
    #[must_use]
    pub fn by_url(&self) -> bool {
        self.url.is_some()
    }

    /// When the URL or grant the request was let in by expires, in seconds since 1970.
    #[must_use]
    pub fn expiry(&self) -> Option<i64> {
        self.url.as_ref().map(|u| u.grant.exp)
    }

    /// What a playlist of the resource hands the URIs it names, for an item whose length is
    /// `length` (zero for a live stream): the query each URI carries (empty for none) and the
    /// value of a `Set-Cookie` field to send with the playlist (empty for none) (fit's `PassOn`).
    /// Let in by a signed URL, that is a grant of its own, living `play_grace` past the later of
    /// the URL's expiry and now and `length`, in the query, or with `url_signing`'s `cookie` in
    /// the cookie; let in by a grant, the same grant; let in by a token in its query, that token.
    #[must_use]
    pub fn pass_on(&self, length: Duration) -> (String, String) {
        self.pass_on_at(length, crate::urlsign::now_secs())
    }

    /// The access of a request let in by grant `g` for `resource`, signed with `key`.
    pub(crate) fn of_url(
        p: &Arc<Policy>,
        resource: &str,
        g: UrlGrant,
        key: Vec<u8>,
        tls: bool,
    ) -> Access {
        Access {
            subject: g.sub.clone(),
            query_token: String::new(),
            url: Some(Arc::new(ByUrl {
                policy: Arc::clone(p),
                resource: resource.to_owned(),
                grant: g,
                key,
                tls,
            })),
        }
    }

    pub(crate) fn pass_on_at(&self, length: Duration, now: i64) -> (String, String) {
        let Some(u) = &self.url else {
            if self.query_token.is_empty() {
                return (String::new(), String::new());
            }
            return (
                format!("token={}", crate::handlers::query_escape(&self.query_token)),
                String::new(),
            );
        };
        let Some(conf) = u.policy.url_config() else {
            return (String::new(), String::new());
        };
        if u.grant.child {
            if conf.cookie {
                return (String::new(), String::new());
            }
            return (u.grant.query(), String::new());
        }
        let g = crate::urlsign::child(&conf, &u.key, &u.grant, &u.resource, length, now);
        if !g.sid.is_empty() {
            u.policy.extend_session(&g.sub, &g.sid, g.exp);
        }
        if conf.cookie {
            return (
                String::new(),
                crate::urlsign::cookie(&g, &u.resource, u.tls, now),
            );
        }
        (g.query(), String::new())
    }
}

/// Whether the request came over TLS.
fn over_tls(ctx: &Context) -> bool {
    ctx.local(akit_runtime::TLS).is_some()
}

/// Answers a request whose signed URL or grant was refused: 403 with why, or 503 with
/// `Retry-After: 1` past `max_tracked_sessions`; `cors` as [`refuse`]'s.
pub(crate) fn refuse_url(ctx: &mut Context, e: TouchError, cors: &str) {
    let (status, msg) = match e {
        TouchError::Url(r) => (
            403,
            format!("the playback URL is refused: {}\n", r.as_str()),
        ),
        TouchError::Full => (503, "too many playback sessions are tracked\n".to_owned()),
    };
    let mut resp = Response::with_body(status, "text/plain; charset=utf-8", msg.into_bytes());
    if e == TouchError::Full {
        resp.headers.set("Retry-After", "1");
    }
    if !cors.is_empty() {
        resp.headers.set("Access-Control-Allow-Origin", cors);
    }
    ctx.respond(resp);
}

/// Checks `g`, from the query of the request or a cookie, against `resource`, and lets the
/// request in, or refuses it (fit's `letIn`).
fn let_in(
    p: &Arc<Policy>,
    ctx: &mut Context,
    resource: &str,
    cors: &str,
    g: UrlGrant,
    next: impl FnOnce(&mut Context, Access) + Send + 'static,
) {
    let r = {
        let req = ctx.req();
        let seen = seen(req, ctx.peer());
        p.let_in(
            &seen,
            resource,
            &g,
            &ctx.peer().to_string(),
            crate::urlsign::now_nanos(),
        )
    };
    match r {
        Err(e) => refuse_url(ctx, e, cors),
        Ok(key) => {
            let tls = over_tls(ctx);
            next(ctx, Access::of_url(p, resource, g, key, tls));
        }
    }
}

/// What of a request a URL's check reads.
fn seen(req: &Request, peer: std::net::SocketAddr) -> Seen<'_> {
    Seen {
        peer: Some(peer.ip()),
        xff: req.headers.get_all("x-forwarded-for").collect(),
        referer: req.headers.get("referer"),
        origin: req.headers.get("origin"),
    }
}

/// [`guard`] for playing `resource`, the path prefix of an item or a stream as
/// [`crate::urlsign::resource_of`] gives it, whose stream name is `stream` (fit's
/// `GuardResource`). With the policy's `url_signing`, a request whose query has a signature is
/// let in by that signed URL, or grant, alone, checked against `resource`, or refused 403 (503
/// past `max_tracked_sessions`); and, with `url_signing`'s `cookie`, a request with the cookie
/// `akit_media_grant` is let in by a grant of it that holds. Any other request is checked as
/// [`guard`] checks it. The signature is written `REDACTED` as a token is.
pub fn guard_resource(
    policy: Option<&Arc<Policy>>,
    ctx: &mut Context,
    stream: &str,
    resource: &str,
    cors: &str,
    next: impl FnOnce(&mut Context, Access) + Send + 'static,
) {
    let Some(p) = policy else {
        return guard(None, ctx, Action::Play, stream, cors, next);
    };
    if let Some(conf) = p.url_config().filter(|_| !p.is_public(Action::Play)) {
        let query = ctx.req().query().unwrap_or("").to_owned();
        if query.contains("sig=") {
            redact(ctx);
            let Some(g) = UrlGrant::parse(&query) else {
                p.count_url_refusal(UrlReason::Signature);
                return refuse_url(ctx, TouchError::Url(UrlReason::Signature), cors);
            };
            return let_in(p, ctx, resource, cors, g, next);
        }
        if conf.cookie {
            match cookie_grant(p, ctx, resource) {
                CookieGrant::None => {}
                CookieGrant::Good(g) => return let_in(p, ctx, resource, cors, g, next),
                CookieGrant::Refused(r) => {
                    p.count_url_refusal(r);
                    redact(ctx);
                    return refuse_url(ctx, TouchError::Url(r), cors);
                }
            }
        }
    }
    guard(Some(p), ctx, Action::Play, stream, cors, next);
}

enum CookieGrant {
    /// No grant cookie, or none that holds beside other credentials: on to [`guard`].
    None,
    Good(UrlGrant),
    Refused(UrlReason),
}

/// The `akit_media_grant` cookie of the request that holds for `resource` (fit's `cookieGrant`):
/// a request whose grant cookies all fail, with no other credentials, is refused for the last
/// one's reason.
fn cookie_grant(p: &Arc<Policy>, ctx: &Context, resource: &str) -> CookieGrant {
    let req = ctx.req();
    let seen = seen(req, ctx.peer());
    let now = crate::urlsign::now_nanos();
    let mut last = None;
    for part in req.headers.get_all("cookie").flat_map(|l| l.split(';')) {
        let (name, value) = part.trim().split_once('=').unwrap_or((part.trim(), ""));
        if name != crate::urlsign::GRANT_COOKIE {
            continue;
        }
        let Some(g) = UrlGrant::parse(value) else {
            last = Some(UrlReason::Signature);
            continue;
        };
        match p.verify_url(&seen, resource, &g, now) {
            Ok(_) => return CookieGrant::Good(g),
            Err(TouchError::Url(r)) => last = Some(r),
            Err(TouchError::Full) => {}
        }
    }
    let others = req
        .headers
        .get("authorization")
        .is_some_and(|a| !a.is_empty())
        || req.query().is_some_and(|q| q.contains("token="));
    match last {
        Some(r) if !others => CookieGrant::Refused(r),
        _ => CookieGrant::None,
    }
}

/// Checks an HTTP request against `policy` for `a` on `stream`, its credentials as
/// [`credentials`] finds them, and calls `next` when it may go on; otherwise answers it
/// ([`refuse`], with `cors`). Its tokens are written `REDACTED` in its target first. A bcrypt
/// check runs on the crate's pool while the request is suspended, and `next` is called there;
/// otherwise before `guard` returns. No policy calls `next` at once.
pub fn guard(
    policy: Option<&Arc<Policy>>,
    ctx: &mut Context,
    a: Action,
    stream: &str,
    cors: &str,
    next: impl FnOnce(&mut Context, Access) + Send + 'static,
) {
    let creds = credentials(ctx.req());
    let query_token = ctx
        .req()
        .query()
        .and_then(|q| query_param(q, "token"))
        .filter(|t| !t.is_empty() && *t == creds.token)
        .unwrap_or_default();
    redact(ctx);
    let Some(p) = policy else {
        return next(
            ctx,
            Access {
                subject: String::new(),
                query_token,
                url: None,
            },
        );
    };
    match p.allow(a, stream, &creds) {
        Err(Refusal::Pending) => {}
        Ok(subject) => {
            return next(
                ctx,
                Access {
                    subject,
                    query_token,
                    url: None,
                },
            );
        }
        Err(r) => return refuse(ctx, r, cors),
    }
    let Some(suspended) = ctx.suspend() else {
        return refuse(ctx, Refusal::Busy, cors);
    };
    let cors = cors.to_owned();
    p.check(a, stream, &creds, move |r| {
        suspended.resume(move |ctx| match r {
            Ok(subject) => next(
                ctx,
                Access {
                    subject,
                    query_token,
                    url: None,
                },
            ),
            Err(r) => refuse(ctx, r, &cors),
        });
    });
}

/// A handler's `auth`: the policy it names, found once the engine is there
/// ([`akit_runtime::upstream::EngineSlot::when_set`]) and read per request without a lock (I123,
/// I124, I126). A handler without `auth` lets every request on; one whose policy is not there (the
/// engine not set yet, or the policy's config not compiling, which the role `media_auth` reports)
/// refuses every request as unavailable, never letting it on unchecked.
#[derive(Debug, Clone)]
pub struct HandlerAuth {
    named: bool,
    found: Arc<std::sync::OnceLock<Option<Arc<Policy>>>>,
}

impl HandlerAuth {
    /// The `auth` of a handler: `name`, whose config is `cfg` (handed to the handler by
    /// akit-config), on the engine `slot` will hold.
    #[must_use]
    pub fn new(
        slot: &akit_runtime::upstream::EngineSlot,
        name: &str,
        cfg: Option<&akit_config::media::MediaAuth>,
    ) -> HandlerAuth {
        let found = Arc::new(std::sync::OnceLock::new());
        let f = Arc::clone(&found);
        let cfg = cfg.cloned();
        slot.when_set(move |engine| {
            let key = crate::auth::registry::key(engine);
            let policy = cfg.and_then(|c| crate::auth::registry::provision(key, &c).ok());
            let _ = f.set(policy);
        });
        HandlerAuth {
            named: !name.is_empty(),
            found,
        }
    }

    /// Whether the handler names a policy.
    #[must_use]
    pub fn named(&self) -> bool {
        self.named
    }

    /// The policy, once found; `None` without one, or while it is not there.
    #[must_use]
    pub fn policy(&self) -> Option<&Arc<Policy>> {
        self.found.get().and_then(Option::as_ref)
    }

    /// [`guard`] with the policy: `next` called when the request may go on, else the request
    /// answered (refused as unavailable while the policy is not there).
    pub fn guard(
        &self,
        ctx: &mut Context,
        a: Action,
        stream: &str,
        cors: &str,
        next: impl FnOnce(&mut Context, Access) + Send + 'static,
    ) {
        if !self.named {
            return guard(None, ctx, a, stream, cors, next);
        }
        match self.found.get() {
            Some(Some(p)) => guard(Some(p), ctx, a, stream, cors, next),
            _ => {
                redact(ctx);
                refuse(ctx, Refusal::Unavailable, cors);
            }
        }
    }

    /// [`guard_resource`] with the policy, as [`guard`](Self::guard) is [`guard`] with it.
    pub fn guard_resource(
        &self,
        ctx: &mut Context,
        stream: &str,
        resource: &str,
        cors: &str,
        next: impl FnOnce(&mut Context, Access) + Send + 'static,
    ) {
        if !self.named {
            return guard(None, ctx, Action::Play, stream, cors, next);
        }
        match self.found.get() {
            Some(Some(p)) => guard_resource(Some(p), ctx, stream, resource, cors, next),
            _ => {
                redact(ctx);
                refuse(ctx, Refusal::Unavailable, cors);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use akio_http::Method;

    fn req(target: &str, fields: &[(&str, &str)]) -> Request {
        let mut r = Request::new(Method::Get, target);
        for (k, v) in fields {
            r.headers.append(k, v);
        }
        r
    }

    /// fit's `TestFromHTTP`: the order of the credentials a request may carry.
    #[test]
    fn credentials_in_order() {
        let basic = format!(
            "Basic {}",
            akio_crypto::base64::STANDARD.encode(b"alice:pw:x")
        );
        let c = credentials(&req(
            "/a?token=q",
            &[
                ("Authorization", "Bearer b"),
                ("Cookie", "akit_media_token=c"),
            ],
        ));
        assert_eq!(c.token, "b");
        let c = credentials(&req("/a?token=q", &[("Authorization", &basic)]));
        assert_eq!((c.user.as_str(), c.password.as_str()), ("alice", "pw:x"));
        let c = credentials(&req(
            "/a?x=1&token=q%2B1",
            &[("Cookie", "a=b; akit_media_token=c")],
        ));
        assert_eq!(c.token, "q+1");
        let c = credentials(&req("/a", &[("Cookie", "a=b; akit_media_token=c")]));
        assert_eq!(c.token, "c");
        // fit's cookie is not akit's (decision 037).
        assert!(credentials(&req("/a", &[("Cookie", "fit_media_token=c")])).is_empty());
        assert!(credentials(&req("/a?token=", &[("Authorization", "Bearer ")])).is_empty());
    }

    #[test]
    fn tokens_redacted() {
        assert_eq!(
            redacted("/live/cam.flv?a=1&token=t&sig=s").as_deref(),
            Some("/live/cam.flv?a=1&token=REDACTED&sig=REDACTED")
        );
        assert_eq!(redacted("/live/cam.flv?a=1"), None);
        assert_eq!(redacted("/live/cam.flv?token=REDACTED"), None);
        assert_eq!(redacted("/live/cam.flv"), None);
        assert_eq!(query_param("a=1&b=x%20y", "b").as_deref(), Some("x y"));
    }
}
