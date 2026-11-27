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

use crate::auth::{Credentials, Policy, Refusal, unescape};

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
/// makes public, or for no policy), and the token it carried in its query, which a playlist
/// hands on to what it names.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Access {
    /// Who it is.
    pub subject: String,
    /// The token of its query, or empty.
    pub query_token: String,
}

impl std::fmt::Debug for Access {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Access")
            .field("subject", &self.subject)
            .finish_non_exhaustive()
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
                },
            ),
            Err(r) => refuse(ctx, r, &cors),
        });
    });
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
