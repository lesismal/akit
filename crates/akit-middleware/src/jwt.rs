//! The `jwt` middleware, as fit's `middleware/auth` `JWT`: a request goes on only with a JSON
//! Web Token whose signature verifies with one of the middleware's keys and whose claims hold:
//! `exp` in the future, `nbf`, if any, in the past (both within `leeway`), and `iss` and `aud`
//! as `issuer` and `audience` say. `claims_headers` copies claims onto request fields. A request
//! without a valid token is answered 401 with a `Bearer` challenge.
//!
//! - Keys come from PEM files and an HMAC secret file, read when the middleware is provisioned
//!   (on the control plane, never on an I/O thread), and from a JWK set ([`crate::jwks`]), which
//!   akio's asynchronous client fetches.
//! - A token is verified on the I/O thread (`akio-crypto`'s signatures: tens of microseconds for
//!   RSA, ECDSA and Ed25519, less for HMAC; nothing blocks), then kept, by its SHA-256, in a cache
//!   of the thread's own ([`MAX_VERIFIED`] tokens), so that a token used again, as one is for its
//!   whole life, costs a hash and a lookup, as fit's `tokenCache`. An entry holds only while the
//!   keys are those it was verified with (their generation, one atomic load); `exp` and `nbf`
//!   are checked on every use.
//! - A token naming a `kid` the JWK set does not have, as a key rotation brings, or any token
//!   before the first fetch is done, waits for a fetch: the request is suspended (akio's
//!   `Context::suspend`, ADR-109) and verified again when it is done, holding no thread
//!   meanwhile. akio cannot suspend a request whose body streams to its handler: for one that
//!   may wait, the middleware asks for its body whole, within the listener's `max_body_bytes`
//!   (akio API request A75 would let it stream).
//! - A field `claims_headers` sets is first removed in every spelling a client could send to pass
//!   for it behind CGI, with or without the claim, as `basic_auth`'s `user_header` is.
//!
//! Who a request was authenticated as (`sub`) and its claims go into fit's access log and to
//! fit's later middleware (`ClaimsOf`); akit's access log cannot carry what a request set yet
//! (akio API request A29), so they do not.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read as _;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use akio_crypto::hash::Sha256;
use akio_http::{BodyMode, Context, Headers, Request, Response};
use akit_config::TokenFrom;
use akit_config::value::Value;
use akit_runtime::{
    Handler, HandlerParams, Middleware, Provision, ProvisionError, Provisioned, set_request_headers,
};

use crate::auth::{UNAUTHORIZED, header_safe, same_field};
use crate::jwks::Jwks;
use crate::jws::{
    Alg, Claims, Key, KeySet, MAX_TOKEN_BYTES, Material, Refusal, Token, claim, go_json,
    numeric_date, parse_public_keys, parse_token,
};
use crate::url::canonical_key;

/// The largest key file read, as fit's `readSmall`.
const MAX_KEY_FILE_BYTES: u64 = 1 << 20;

/// The tokens each thread keeps verified, for all the `jwt` middleware there are: fit keeps
/// 16 shards of 1024 for each.
pub(crate) const MAX_VERIFIED: usize = 1024;

pub(crate) fn new(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Middleware>, ProvisionError> {
    let HandlerParams::Jwt(j) = p.params() else {
        return Err(ProvisionError::new(
            "the jwt middleware was given another's params",
        ));
    };
    let jwt = Arc::new(Jwt::new(j, p.env().engine())?);
    let jwks = match &jwt.keys {
        Keys::Jwks(j) => Some(j.clone()),
        Keys::Static(_) => None,
    };
    let provisioned = Provisioned::new(Arc::new(JwtMiddleware(jwt)) as Arc<dyn Middleware>);
    Ok(match jwks {
        Some(j) => provisioned.on_cleanup(move || j.close()),
        None => provisioned,
    })
}

/// Where the keys come from.
#[derive(Debug)]
enum Keys {
    /// The config's own, without a JWK set.
    Static(KeySet),
    /// A JWK set, with the config's own in it.
    Jwks(Jwks),
}

/// Where the token is, its names canonical.
#[derive(Debug)]
enum From {
    Authorization,
    Header(String),
    Cookie(String),
    Query(String),
}

/// The `jwt` middleware, compiled; shared by every handler it wraps.
#[derive(Debug)]
pub(crate) struct Jwt {
    /// Unique to the middleware: its tokens' key in the threads' caches.
    id: u64,
    keys: Keys,
    /// The algorithms allowed, by [`Alg::bit`].
    allowed: u16,
    from: From,
    issuer: String,
    audience: Vec<String>,
    /// In seconds.
    leeway: f64,
    /// `claims_headers`: (field, claim), the fields canonical, sorted.
    claim_headers: Vec<(String, String)>,
}

/// A token verified: what of it is checked again or used on each request.
#[derive(Debug, Clone)]
pub(crate) struct Verified {
    /// The generation of the keys it was verified with.
    generation: u64,
    exp: f64,
    nbf: Option<f64>,
    /// The value of each of `claim_headers`' fields, `None` for a claim the token lacks.
    fields: Rc<[Option<String>]>,
}

/// Reads a key file, of at most 1 MiB.
fn read_small(path: &str) -> Result<Vec<u8>, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("open {path}: {e}"))?;
    let mut data = Vec::new();
    f.take(MAX_KEY_FILE_BYTES.saturating_add(1))
        .read_to_end(&mut data)
        .map_err(|e| format!("read {path}: {e}"))?;
    if data.len() as u64 > MAX_KEY_FILE_BYTES {
        return Err(format!("{path}: larger than 1 MiB"));
    }
    Ok(data)
}

/// Unique ids of the middleware.
static IDS: AtomicU64 = AtomicU64::new(1);

impl Jwt {
    fn new(
        j: &akit_config::Jwt,
        slot: &akit_runtime::upstream::EngineSlot,
    ) -> Result<Jwt, ProvisionError> {
        let mut keys: Vec<Arc<Key>> = Vec::new();
        for (i, path) in j.public_key_files.iter().enumerate() {
            let found = read_small(path).and_then(|data| parse_public_keys(&data));
            match found {
                Ok(k) => keys.extend(k),
                Err(e) => {
                    return Err(ProvisionError::field(&format!("public_key_files[{i}]"), e));
                }
            }
        }
        if !j.hmac_secret_file.is_empty() {
            let secret = read_small(&j.hmac_secret_file).and_then(|s| {
                if s.len() < 32 {
                    Err(format!(
                        "the secret has {} bytes; at least 32 are needed",
                        s.len()
                    ))
                } else {
                    Ok(s)
                }
            });
            match secret {
                Ok(s) => keys.push(Arc::new(Key::new(Material::Secret(s)))),
                Err(e) => return Err(ProvisionError::field("hmac_secret_file", e)),
            }
        }
        if j.jwks_url.is_empty() && keys.is_empty() {
            return Err(ProvisionError::new(
                "a key is needed: jwks_url, public_key_files or hmac_secret_file",
            ));
        }
        let mut allowed = 0u16;
        for a in Alg::ALL {
            if j.algorithms.is_empty() || j.algorithms.iter().any(|n| n == a.name()) {
                allowed |= a.bit();
            }
        }
        let from = match &j.token_from {
            TokenFrom::Authorization => From::Authorization,
            TokenFrom::Header(n) => From::Header(canonical_key(n)),
            TokenFrom::Cookie(n) => From::Cookie(n.clone()),
            TokenFrom::Query(n) => From::Query(n.clone()),
        };
        let mut claim_headers: Vec<(String, String)> = j
            .claims_headers
            .iter()
            .map(|(f, c)| (canonical_key(f), c.clone()))
            .collect();
        claim_headers.sort_by(|a, b| a.0.cmp(&b.0));
        let keys = if j.jwks_url.is_empty() {
            Keys::Static(KeySet::new([&keys[..]]))
        } else {
            let refresh = if j.jwks_refresh_interval.is_zero() {
                akit_config::DEFAULT_JWKS_REFRESH_INTERVAL
            } else {
                j.jwks_refresh_interval
            };
            Keys::Jwks(Jwks::new(&j.jwks_url, refresh, keys, slot))
        };
        Ok(Jwt {
            id: IDS.fetch_add(1, Ordering::Relaxed),
            keys,
            allowed,
            from,
            issuer: j.issuer.clone(),
            audience: j.audience.clone(),
            leeway: j.leeway.as_secs_f64(),
            claim_headers,
        })
    }

    /// The generation of the keys to verify with now; 0 before a JWK set was fetched.
    fn generation(&self) -> u64 {
        match &self.keys {
            Keys::Static(k) => k.generation,
            Keys::Jwks(j) => j.generation(),
        }
    }

    /// Runs `f` with the keys to verify with now.
    fn with_keys<R>(&self, f: impl FnOnce(Option<&KeySet>) -> R) -> R {
        match &self.keys {
            Keys::Static(k) => f(Some(k)),
            Keys::Jwks(j) => f(j.current().as_deref()),
        }
    }

    /// The token of a request with fields `headers` and target `target`.
    fn token(&self, headers: &Headers, target: &str) -> Option<String> {
        let s = match &self.from {
            From::Authorization => {
                let auth = headers.get("authorization")?;
                let scheme = auth.get(..7)?;
                if !scheme.eq_ignore_ascii_case("bearer ") {
                    return None;
                }
                auth.get(7..)?.trim()
            }
            From::Header(name) => headers.get(name)?.trim(),
            From::Cookie(name) => cookie(headers, name)?,
            From::Query(name) => return query(target, name).filter(|s| !s.is_empty()),
        };
        (!s.is_empty()).then(|| s.to_owned())
    }

    /// Verifies `t`'s signature with `keys` and its claims at time `now` (seconds since the
    /// epoch), as fit's `check`.
    fn check(&self, t: &Token, keys: Option<&KeySet>, now: f64) -> Result<Verified, Refusal> {
        let alg = Alg::from_name(&t.alg)
            .filter(|a| self.allowed & a.bit() != 0)
            .ok_or(Refusal::Algorithm)?;
        let keys = keys.ok_or(Refusal::UnknownKey)?;
        let (mut candidates, known) = keys.lookup(&t.kid);
        if !candidates.any(|k| k.fits(alg) && k.verify(alg, t.signed.as_bytes(), &t.sig)) {
            return Err(if known {
                Refusal::Malformed
            } else {
                Refusal::UnknownKey
            });
        }
        let claims = t.claims()?;
        let exp = numeric_date(claim(&claims, "exp")).ok_or(Refusal::Claims)?;
        let nbf = match claim(&claims, "nbf") {
            None => None,
            v => Some(numeric_date(v).ok_or(Refusal::Claims)?),
        };
        if !self.timely(exp, nbf, now) {
            return Err(Refusal::Claims);
        }
        if !self.issuer.is_empty()
            && !matches!(claim(&claims, "iss"), Some(Value::String(s)) if *s == self.issuer)
        {
            return Err(Refusal::Claims);
        }
        if !self.audience.is_empty() && !self.audience_holds(claim(&claims, "aud")) {
            return Err(Refusal::Claims);
        }
        Ok(Verified {
            generation: keys.generation,
            exp,
            nbf,
            fields: self.fields_of(&claims),
        })
    }

    /// Whether `exp` and `nbf` hold at `now`, within the leeway: fit's `timely`.
    fn timely(&self, exp: f64, nbf: Option<f64>, now: f64) -> bool {
        now <= exp + self.leeway && nbf.is_none_or(|nbf| now >= nbf - self.leeway)
    }

    /// Whether `aud`, a string or an array of them, names one of the audiences.
    fn audience_holds(&self, aud: Option<&Value>) -> bool {
        let named = |s: &str| self.audience.iter().any(|a| a == s);
        match aud {
            Some(Value::String(s)) => named(s),
            Some(Value::Array(list)) => list
                .iter()
                .any(|v| matches!(v, Value::String(s) if named(s))),
            _ => false,
        }
    }

    /// The values of `claim_headers`' fields: a string claim as it is, anything else as JSON,
    /// either made safe for a field.
    fn fields_of(&self, claims: &Claims) -> Rc<[Option<String>]> {
        self.claim_headers
            .iter()
            .map(|(_, name)| {
                claim(claims, name).map(|v| match v {
                    Value::String(s) => header_safe(s),
                    other => header_safe(&go_json(other)),
                })
            })
            .collect()
    }

    /// Lets the request through: any field that passes for one of `claim_headers` is removed,
    /// and those of the token's claims set.
    fn admit(&self, ctx: &mut Context, fields: &[Option<String>]) {
        if self.claim_headers.is_empty() {
            return;
        }
        let headers = &ctx.req().headers;
        let mut kept = Headers::with_capacity(headers.len().saturating_add(fields.len()), 0);
        for (n, v) in headers
            .iter()
            .filter(|(n, _)| !self.claim_headers.iter().any(|(f, _)| same_field(n, f)))
        {
            kept.append(n, v);
        }
        for ((field, _), value) in self.claim_headers.iter().zip(fields) {
            if let Some(v) = value {
                kept.append(field, v);
            }
        }
        set_request_headers(ctx, kept);
    }

    /// Answers 401 with a `Bearer` challenge; `invalid` says a token was sent but did not pass.
    fn refuse(ctx: &mut Context, invalid: bool) {
        let mut resp = Response::with_body(
            401,
            "text/plain; charset=utf-8",
            UNAUTHORIZED.as_bytes().to_vec(),
        );
        resp.headers.set(
            "WWW-Authenticate",
            if invalid {
                r#"Bearer realm="fit", error="invalid_token""#
            } else {
                r#"Bearer realm="fit""#
            },
        );
        ctx.respond(resp);
    }

    /// The token kept verified for `sum`, if it was verified with the keys of `generation`.
    fn cached(&self, sum: &[u8; 32], generation: u64) -> Option<Verified> {
        VERIFIED.with(|c| {
            let c = c.try_borrow().ok()?;
            c.get(&(self.id, *sum))
                .filter(|v| v.generation == generation)
                .cloned()
        })
    }

    /// Keeps `v` verified for `sum`; a full cache forgets one entry for it.
    fn keep(&self, sum: [u8; 32], v: &Verified) {
        VERIFIED.with(|c| {
            let Ok(mut c) = c.try_borrow_mut() else {
                return;
            };
            if c.len() >= MAX_VERIFIED && !c.contains_key(&(self.id, sum)) {
                if let Some(old) = c.keys().next().copied() {
                    c.remove(&old);
                }
            }
            c.insert((self.id, sum), v.clone());
        });
    }

    /// Whether a request with fields `headers` and target `target` may wait for a JWK set to be
    /// fetched: with a token whose `kid` the current set does not have, or before a set was.
    fn may_wait(&self, headers: &Headers, target: &str) -> bool {
        let Keys::Jwks(jwks) = &self.keys else {
            return false;
        };
        let Some(raw) = self.token(headers, target) else {
            return false;
        };
        let generation = jwks.generation();
        if generation == 0 {
            return true;
        }
        if self
            .cached(&Sha256::digest(raw.as_bytes()), generation)
            .is_some()
        {
            return false;
        }
        let Ok(t) = parse_token(&raw) else {
            return false;
        };
        if Alg::from_name(&t.alg).is_none_or(|a| self.allowed & a.bit() == 0) {
            return false;
        }
        jwks.current().is_none_or(|k| !k.lookup(&t.kid).1)
    }
}

thread_local! {
    /// The tokens this thread verified, by the middleware's id and the token's SHA-256.
    static VERIFIED: RefCell<HashMap<(u64, [u8; 32]), Verified>> = RefCell::new(HashMap::new());
}

/// Seconds since the epoch.
fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// The value of the cookie `name` in `headers`' `Cookie` fields, as Go's `Request.Cookie` reads
/// it: its quotes removed, and only if every byte of it may be in a cookie.
fn cookie<'h>(headers: &'h Headers, name: &str) -> Option<&'h str> {
    headers.get_all("cookie").find_map(|line| {
        line.split(';').find_map(|part| {
            let (k, v) = part.trim_matches(' ').split_once('=')?;
            if k != name {
                return None;
            }
            let v = v
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(v);
            v.bytes()
                .all(|b| (0x20..0x7f).contains(&b) && !matches!(b, b'"' | b';' | b'\\'))
                .then_some(v)
        })
    })
}

/// The first value of the query parameter `name` of `target`, decoded, as Go's
/// `URL.Query().Get`: pairs with `;` or a bad escape are skipped.
fn query(target: &str, name: &str) -> Option<String> {
    let (_, q) = target.split_once('?')?;
    let q = q.split_once('#').map_or(q, |(q, _)| q);
    q.split('&').find_map(|pair| {
        if pair.is_empty() || pair.contains(';') {
            return None;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let k = unescape_query(k)?;
        (k == name).then(|| unescape_query(v)).flatten()
    })
}

/// `s` decoded as a query's part: `+` a space, `%XX` a byte; `None` for a bad escape or bytes
/// that are not UTF-8.
fn unescape_query(s: &str) -> Option<String> {
    let plus: Vec<u8> = s
        .bytes()
        .map(|b| if b == b'+' { b' ' } else { b })
        .collect();
    String::from_utf8(crate::url::decode(&plus)?).ok()
}

/// The `jwt` middleware's handle.
#[derive(Debug)]
struct JwtMiddleware(Arc<Jwt>);

impl Middleware for JwtMiddleware {
    fn wrap(&self, next: Arc<dyn Handler>) -> Arc<dyn Handler> {
        Arc::new(JwtChecked {
            jwt: Arc::clone(&self.0),
            next,
        })
    }
}

struct JwtChecked {
    jwt: Arc<Jwt>,
    next: Arc<dyn Handler>,
}

impl JwtChecked {
    /// Waits for the JWK set to be fetched, the request suspended, then verifies `t` again.
    fn wait(&self, ctx: &mut Context, jwks: &Jwks, t: Token, sum: [u8; 32], seen: u64) {
        let Some(suspended) = ctx.suspend() else {
            // Its body streams (`body_mode` asked for it whole, but the keys changed since).
            Jwt::refuse(ctx, true);
            return;
        };
        let (jwt, next) = (Arc::clone(&self.jwt), Arc::clone(&self.next));
        let then = move |keys: Option<Arc<KeySet>>| {
            suspended.resume(|ctx| match jwt.check(&t, keys.as_deref(), now()) {
                Ok(v) => {
                    jwt.keep(sum, &v);
                    jwt.admit(ctx, &v.fields);
                    next.call(ctx);
                }
                Err(_) => Jwt::refuse(ctx, true),
            });
        };
        if let Err(then) = jwks.wait(seen, then) {
            // Too many wait already (or the config is gone): refused, as with no keys.
            then(None);
        }
    }
}

impl akio_http::Service for JwtChecked {
    fn call(&self, ctx: &mut Context) {
        let jwt = &*self.jwt;
        let raw = {
            let req = ctx.req();
            jwt.token(&req.headers, &req.target)
        };
        let Some(raw) = raw else {
            Jwt::refuse(ctx, false);
            return;
        };
        if raw.len() > MAX_TOKEN_BYTES {
            Jwt::refuse(ctx, true);
            return;
        }
        let sum: [u8; 32] = Sha256::digest(raw.as_bytes());
        let now = now();
        let generation = jwt.generation();
        if generation != 0 {
            if let Some(v) = jwt.cached(&sum, generation) {
                if jwt.timely(v.exp, v.nbf, now) {
                    jwt.admit(ctx, &v.fields);
                    self.next.call(ctx);
                } else {
                    Jwt::refuse(ctx, true);
                }
                return;
            }
        }
        let Ok(t) = parse_token(&raw) else {
            Jwt::refuse(ctx, true);
            return;
        };
        let (checked, seen) =
            jwt.with_keys(|keys| (jwt.check(&t, keys, now), keys.map_or(0, |k| k.generation)));
        match checked {
            Ok(v) => {
                jwt.keep(sum, &v);
                jwt.admit(ctx, &v.fields);
                self.next.call(ctx);
            }
            Err(Refusal::UnknownKey) => match &jwt.keys {
                // The set may have rotated: the request waits for a fetch.
                Keys::Jwks(jwks) => self.wait(ctx, jwks, t, sum, seen),
                Keys::Static(_) => Jwt::refuse(ctx, true),
            },
            Err(_) => Jwt::refuse(ctx, true),
        }
    }

    /// The handler's, except that a request that may wait for the JWK set has its body whole,
    /// so that it can be suspended.
    fn body_mode(&self, req: &Request) -> BodyMode {
        let mode = self.next.body_mode(req);
        if mode == BodyMode::Stream && self.jwt.may_wait(&req.headers, &req.target) {
            return BodyMode::Buffered;
        }
        mode
    }

    fn streams_bodies(&self) -> bool {
        self.next.streams_bodies()
    }
}

impl Handler for JwtChecked {}

/// Sets the least time between two fetches of the JWK sets provisioned from now on, which a
/// token with an unknown `kid` asks for: 10s unless a test sets it.
pub fn set_jwks_min_refetch(d: std::time::Duration) {
    crate::jwks::set_min_refetch(d);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use std::time::Duration;

    use akio_crypto::base64::URL_SAFE_NO_PAD as B64;
    use akio_crypto::mac::Hmac;

    use super::*;

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";

    /// A file of [`SECRET`], one of its own for each caller: tests run at once.
    fn secret_file() -> String {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "akit-jwt-unit-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&p, SECRET).unwrap();
        p.to_string_lossy().into_owned()
    }

    fn jwt(change: impl FnOnce(&mut akit_config::Jwt)) -> Jwt {
        let mut j = akit_config::Jwt {
            jwks_url: String::new(),
            jwks_refresh_interval: Duration::ZERO,
            public_key_files: Vec::new(),
            hmac_secret_file: secret_file(),
            algorithms: Vec::new(),
            issuer: String::new(),
            audience: Vec::new(),
            leeway: Duration::ZERO,
            token_from: TokenFrom::Authorization,
            claims_headers: Vec::new(),
        };
        change(&mut j);
        let jwt = Jwt::new(&j, &akit_runtime::upstream::EngineSlot::new()).unwrap();
        // Read when provisioned: no longer needed.
        let _ = std::fs::remove_file(&j.hmac_secret_file);
        jwt
    }

    fn hs256(claims: &str) -> Token {
        let signed = format!(
            "{}.{}",
            B64.encode(r#"{"alg":"HS256"}"#),
            B64.encode(claims)
        );
        let sig = Hmac::<Sha256>::mac(SECRET, signed.as_bytes());
        parse_token(&format!("{signed}.{}", B64.encode(sig.as_ref()))).unwrap()
    }

    fn check(j: &Jwt, claims: &str, now: f64) -> Result<Verified, Refusal> {
        j.with_keys(|k| j.check(&hs256(claims), k, now))
    }

    // fit's TestJWTClaims, the claims' part; the clock is the caller's.
    #[test]
    fn claims_checked() {
        let j = jwt(|j| {
            j.issuer = "https://issuer.example".into();
            j.audience = vec!["api".into(), "web".into()];
            j.leeway = Duration::from_secs(30);
        });
        let now = 1_700_000_000.0;
        let base = |more: &str| {
            format!(r#"{{"sub":"alice","iss":"https://issuer.example","aud":"api"{more}}}"#)
        };
        let exp = |d: i64| format!(r#","exp":{}"#, 1_700_000_000 + d);
        for (what, claims, ok) in [
            ("valid", base(&exp(60)), true),
            ("expired within the leeway", base(&exp(-20)), true),
            ("expired", base(&exp(-40)), false),
            ("no exp", base(""), false),
            ("exp a string", base(r#","exp":"1700000060""#), false),
            ("fractional exp", base(r#","exp":1700000060.5"#), true),
            (
                "nbf within the leeway",
                base(&format!(r#"{},"nbf":1700000020"#, exp(60))),
                true,
            ),
            (
                "not yet",
                base(&format!(r#"{},"nbf":1700000040"#, exp(60))),
                false,
            ),
            (
                "nbf a string",
                base(&format!(r#"{},"nbf":"x""#, exp(60))),
                false,
            ),
            (
                "another issuer",
                format!(r#"{{"iss":"https://evil.example","aud":"api"{}}}"#, exp(60)),
                false,
            ),
            ("no issuer", format!(r#"{{"aud":"api"{}}}"#, exp(60)), false),
            (
                "audiences",
                format!(
                    r#"{{"iss":"https://issuer.example","aud":["other","web"]{}}}"#,
                    exp(60)
                ),
                true,
            ),
            (
                "another audience",
                format!(
                    r#"{{"iss":"https://issuer.example","aud":["other"]{}}}"#,
                    exp(60)
                ),
                false,
            ),
            (
                "no audience",
                format!(r#"{{"iss":"https://issuer.example"{}}}"#, exp(60)),
                false,
            ),
        ] {
            assert_eq!(check(&j, &claims, now).is_ok(), ok, "{what}: {claims}");
        }

        // The clock: a token is let in until exp and the leeway.
        let v = check(&j, &base(&exp(60)), now).unwrap();
        assert!(j.timely(v.exp, v.nbf, now + 89.0));
        assert!(!j.timely(v.exp, v.nbf, now + 91.0));
        assert_eq!(
            check(&j, "[1]", now).unwrap_err(),
            Refusal::Malformed,
            "claims no object"
        );
    }

    #[test]
    fn algorithms_allowed() {
        let only = jwt(|j| j.algorithms = vec!["HS384".into()]);
        assert_eq!(
            check(&only, r#"{"exp":1e12}"#, 0.0).unwrap_err(),
            Refusal::Algorithm
        );
        let any = jwt(|_| {});
        assert!(check(&any, r#"{"exp":1e12}"#, 0.0).is_ok());
    }

    #[test]
    fn claim_fields() {
        let j = jwt(|j| {
            j.claims_headers = vec![
                ("x-sub".into(), "sub".into()),
                ("X-Roles".into(), "roles".into()),
                ("X-Missing".into(), "nope".into()),
            ];
        });
        let v = check(
            &j,
            r#"{"sub":"alice\r\nX-Evil: 1","roles":["admin","dev"],"exp":1e12}"#,
            0.0,
        )
        .unwrap();
        let names: Vec<&str> = j.claim_headers.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(names, ["X-Missing", "X-Roles", "X-Sub"]);
        assert_eq!(
            &*v.fields,
            [
                None,
                Some(r#"["admin","dev"]"#.to_owned()),
                Some("alice??X-Evil: 1".to_owned())
            ]
        );
    }

    #[test]
    fn tokens_found() {
        let mut h = Headers::new();
        h.append("Authorization", "bearer  abc ");
        h.append("X-Token", " xyz");
        h.append("Cookie", "a=1; session=\"tok\"; b=2");
        let j = jwt(|_| {});
        assert_eq!(j.token(&h, "/").as_deref(), Some("abc"));
        let j = jwt(|j| j.token_from = TokenFrom::Header("x-token".into()));
        assert_eq!(j.token(&h, "/").as_deref(), Some("xyz"));
        let j = jwt(|j| j.token_from = TokenFrom::Cookie("session".into()));
        assert_eq!(j.token(&h, "/").as_deref(), Some("tok"));
        let j = jwt(|j| j.token_from = TokenFrom::Cookie("none".into()));
        assert_eq!(j.token(&h, "/"), None);
        let j = jwt(|j| j.token_from = TokenFrom::Query("access_token".into()));
        assert_eq!(
            j.token(&h, "/p?x=1;y&access_token=a%2Bb+c&access_token=2")
                .as_deref(),
            Some("a+b c")
        );
        assert_eq!(
            j.token(&h, "/p?access_token=%zz&access_token=2").as_deref(),
            Some("2")
        );
        assert_eq!(j.token(&h, "/p?access_token="), None);
        let mut basic = Headers::new();
        basic.append("Authorization", "Basic YWxpY2U6dG9rZW4=");
        assert_eq!(jwt(|_| {}).token(&basic, "/"), None);
    }

    // fit's TestJWTCache: kept, for the keys it was verified with only, and bounded.
    #[test]
    fn verified_tokens_kept() {
        let j = jwt(|_| {});
        let v = check(&j, r#"{"exp":1e12}"#, 0.0).unwrap();
        let sum = [7u8; 32];
        j.keep(sum, &v);
        assert!(j.cached(&sum, j.generation()).is_some());
        assert!(j.cached(&sum, j.generation() + 1).is_none(), "other keys");
        assert!(
            jwt(|_| {}).cached(&sum, j.generation()).is_none(),
            "another jwt"
        );
        for i in 0..20_000u32 {
            let mut s = [0u8; 32];
            s[..4].copy_from_slice(&i.to_be_bytes());
            j.keep(s, &v);
        }
        assert!(VERIFIED.with(|c| c.borrow().len()) <= MAX_VERIFIED);
    }

    #[test]
    fn key_files_checked() {
        let dir = std::env::temp_dir().join(format!("akit-jwt-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let short = dir.join("short");
        std::fs::write(&short, "too short").unwrap();
        let large = dir.join("large.pem");
        std::fs::write(&large, vec![b'a'; (1 << 20) + 1]).unwrap();
        let path = |p: &std::path::Path| p.to_string_lossy().into_owned();
        let err = |change: &dyn Fn(&mut akit_config::Jwt)| {
            let mut j = akit_config::Jwt {
                jwks_url: String::new(),
                jwks_refresh_interval: Duration::ZERO,
                public_key_files: Vec::new(),
                hmac_secret_file: String::new(),
                algorithms: Vec::new(),
                issuer: String::new(),
                audience: Vec::new(),
                leeway: Duration::ZERO,
                token_from: TokenFrom::Authorization,
                claims_headers: Vec::new(),
            };
            change(&mut j);
            let e = Jwt::new(&j, &akit_runtime::upstream::EngineSlot::new()).unwrap_err();
            format!("{}: {}", e.field_name().unwrap_or(""), e.message())
        };
        let s = err(&|j| j.hmac_secret_file = path(&short));
        assert!(
            s.contains("hmac_secret_file") && s.contains("at least 32 are needed"),
            "{s}"
        );
        let s = err(&|j| j.public_key_files = vec!["/nonexistent".into()]);
        assert!(s.starts_with("public_key_files[0]"), "{s}");
        let s = err(&|j| j.public_key_files = vec![path(&large)]);
        assert!(s.contains("larger than 1 MiB"), "{s}");
        let s = err(&|j| j.public_key_files = vec![path(&short)]);
        assert!(s.contains("no PEM public key"), "{s}");
        let s = err(&|_| {});
        assert!(s.contains("a key is needed"), "{s}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
