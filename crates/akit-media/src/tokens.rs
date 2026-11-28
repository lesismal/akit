//! The `media_tokens` handler (I143, fit's `handler/mediatokens`): a route a site's backend calls
//! to sign many `media_auth` tokens at once, and to revoke the tokens of one key of the policy's
//! keyring, or of all of them, or of its single secret, by rotating its secret file. It is not the
//! Admin API: its route authenticates its callers with its own middleware, which the config
//! requires (fit's `NeedsAuthentication`).
//!
//! Nothing blocks an I/O thread: a batch is computation on the policy's state; a revoke's one slow
//! part, writing the new secret to the file, runs on a pool of the module's own, one thread and a
//! queue of one, with the request suspended and answered from there, or 504 past
//! `revoke_timeout`. Every part is bounded: `max_tokens` a request, `max_body_bytes` a body, one
//! revoke waiting, `revoke_timeout` a revoke, `max_ttl` a token, `prefixes` the streams.

use std::io::Write as _;
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use akio_http::{Context, Method, Response, Service, Suspended};
use akit_config::media::valid_media_pattern;
use akit_config::value::Value;
use akit_runtime::{Handler, HandlerParams, Provision, ProvisionError, Provisioned};

use crate::auth::{Grant, Policy};
use crate::http::HandlerAuth;

/// The longest a subject may be.
const MAX_SUBJECT_BYTES: usize = 128;

struct Tokens {
    prefix: String,
    prefixes: Vec<String>,
    max_ttl: Duration,
    max_tokens: usize,
    max_body: usize,
    revoke_timeout: Duration,
    url_base: String,
    url_prefixes: Vec<String>,
    auth: HandlerAuth,
    pool: Arc<RevokePool>,
}

/// The module's constructor.
pub(crate) fn new_tokens(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    let HandlerParams::MediaTokens(t) = p.params() else {
        return Err(ProvisionError::new(
            "the media_tokens handler's params are not media_tokens'",
        ));
    };
    let pool = Arc::new(RevokePool::start()?);
    let cleanup = Arc::clone(&pool);
    Ok(Provisioned::new(Arc::new(Tokens {
        prefix: t.prefix_or().to_owned(),
        prefixes: t.prefixes.clone(),
        max_ttl: t.ttl_bound(),
        max_tokens: t.tokens(),
        max_body: t.body_bytes(),
        revoke_timeout: t.revoke(),
        url_base: t.url_base.clone(),
        url_prefixes: t.url_prefixes.clone(),
        auth: HandlerAuth::new(p.env().engine(), &t.auth, t.policy.as_ref()),
        pool,
    }) as Arc<dyn Handler>)
    .on_cleanup(move || cleanup.close()))
}

impl Handler for Tokens {}

/// Answers with a status and a line of text, never cached.
fn plain(ctx: &mut Context, status: u16, msg: &str, extra: &[(&str, &str)]) {
    ctx.respond(text(status, msg, extra));
}

fn text(status: u16, msg: &str, extra: &[(&str, &str)]) -> Response {
    let mut r = Response::with_body(
        status,
        "text/plain; charset=utf-8",
        format!("{msg}\n").into_bytes(),
    )
    .header("Cache-Control", "no-store");
    for (k, v) in extra {
        r = r.header(k, v);
    }
    r
}

fn json(body: String) -> Response {
    Response::with_body(200, "application/json", body.into_bytes())
        .header("Cache-Control", "no-store")
}

/// A string written as JSON.
fn quote(s: &str) -> String {
    let mut b = String::new();
    akit_config::value::write_string(&mut b, s);
    b
}

impl Service for Tokens {
    fn call(&self, ctx: &mut Context) {
        let path = ctx.path().to_owned();
        let post = *ctx.method() == Method::Post;
        let rest = path.strip_prefix(self.prefix.as_str());
        match rest {
            Some("") if post => self.issue(ctx),
            Some("") => plain(
                ctx,
                405,
                "the batch endpoint takes POST",
                &[("Allow", "POST")],
            ),
            Some("/revoke") if post => self.revoke(ctx),
            Some("/revoke") => plain(
                ctx,
                405,
                "the revoke endpoint takes POST",
                &[("Allow", "POST")],
            ),
            Some("/urls") if post => self.urls(ctx),
            Some("/urls") => plain(
                ctx,
                405,
                "the urls endpoint takes POST",
                &[("Allow", "POST")],
            ),
            _ => plain(ctx, 404, "not found", &[]),
        }
    }
}

/// One token of a batch, as a request asks for it.
#[derive(Default)]
struct TokenRequest {
    subject: String,
    publish: Vec<String>,
    play: Vec<String>,
    ttl: String,
}

/// A JSON member as Go's `json.Unmarshal` reads it into a string: `null` is left out.
fn string_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Null => Some(String::new()),
        _ => None,
    }
}

fn strings_of(v: &Value) -> Option<Vec<String>> {
    match v {
        Value::Array(a) => a
            .iter()
            .map(|x| match x {
                Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        Value::Null => Some(Vec::new()),
        _ => None,
    }
}

/// The body as a JSON object's members, `None` when it is not one.
fn object(body: &[u8]) -> Option<Vec<(String, Value)>> {
    let s = std::str::from_utf8(body).ok()?;
    match akit_config::value::parse(s).ok()? {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

/// A batch's request: the key to sign with, and the tokens.
fn issue_request(body: &[u8]) -> Option<(String, Vec<TokenRequest>)> {
    let mut key = String::new();
    let mut tokens = Vec::new();
    for (k, v) in object(body)? {
        match k.as_str() {
            "key" => key = string_of(&v)?,
            "tokens" => {
                let list = match v {
                    Value::Array(a) => a,
                    Value::Null => Vec::new(),
                    _ => return None,
                };
                for t in list {
                    let Value::Object(members) = t else {
                        return None;
                    };
                    let mut r = TokenRequest::default();
                    for (k, v) in members {
                        match k.as_str() {
                            "subject" => r.subject = string_of(&v)?,
                            "publish" => r.publish = strings_of(&v)?,
                            "play" => r.play = strings_of(&v)?,
                            "ttl" => r.ttl = string_of(&v)?,
                            // Go's decoder leaves members it does not know.
                            _ => {}
                        }
                    }
                    tokens.push(r);
                }
            }
            _ => {}
        }
    }
    Some((key, tokens))
}

/// What `<prefix>/urls` is asked for: the viewer, the paths to sign URLs of, and their lifetime
/// and key.
#[derive(Default)]
struct UrlsRequest {
    subject: String,
    address: String,
    session: String,
    ttl: String,
    key: String,
    paths: Vec<String>,
}

fn urls_request(body: &[u8]) -> Option<UrlsRequest> {
    let mut r = UrlsRequest::default();
    for (k, v) in object(body)? {
        match k.as_str() {
            "subject" => r.subject = string_of(&v)?,
            "address" => r.address = string_of(&v)?,
            "session" => r.session = string_of(&v)?,
            "ttl" => r.ttl = string_of(&v)?,
            "key" => r.key = string_of(&v)?,
            "paths" => r.paths = strings_of(&v)?,
            _ => {}
        }
    }
    Some(r)
}

/// A new playback session's ID: 12 random bytes in base64url.
fn new_session() -> String {
    let mut b = [0u8; 12];
    if akio_crypto::rand::fill_random(&mut b).is_err() {
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        b.copy_from_slice(t.to_be_bytes().get(4..).unwrap_or(&[0; 12]));
    }
    akio_crypto::base64::URL_SAFE_NO_PAD.encode(b)
}

/// The request's body, within `limit`; otherwise answered 413.
fn body(ctx: &mut Context, limit: usize) -> Option<Vec<u8>> {
    let declared = ctx
        .header("content-length")
        .and_then(|v| v.trim().parse::<u64>().ok());
    if declared.is_some_and(|n| n > limit as u64) || ctx.body().len() > limit {
        plain(ctx, 413, "the body is too large", &[]);
        return None;
    }
    Some(ctx.body().to_vec())
}

impl Tokens {
    fn policy(&self, ctx: &mut Context) -> Option<Arc<Policy>> {
        let p = self.auth.policy().cloned();
        if p.is_none() {
            plain(
                ctx,
                503,
                "the media_auth policy is not loaded",
                &[("Retry-After", "1")],
            );
        }
        p
    }

    /// A batch: every token checked, then all issued, or none.
    fn issue(&self, ctx: &mut Context) {
        let Some(b) = body(ctx, self.max_body) else {
            return;
        };
        let Some((key, reqs)) = issue_request(&b) else {
            return plain(ctx, 400, "the body must be JSON: {\"tokens\": [...]}", &[]);
        };
        if reqs.is_empty() {
            return plain(
                ctx,
                400,
                "the batch is empty: ask for at least one token",
                &[],
            );
        }
        if reqs.len() > self.max_tokens {
            return plain(
                ctx,
                400,
                &format!(
                    "the batch has {} tokens: at most {} a request",
                    reqs.len(),
                    self.max_tokens
                ),
                &[],
            );
        }
        let Some(p) = self.policy(ctx) else { return };
        let ring = !p.keys().is_empty();
        let key = if !key.is_empty() {
            if !ring {
                return plain(
                    ctx,
                    400,
                    "the policy has one key: leave key out of the request",
                    &[],
                );
            }
            if p.key_file(&key).is_none() {
                return plain(ctx, 400, &format!("no key named {key:?}"), &[]);
            }
            key
        } else if ring {
            p.default_key().unwrap_or_default()
        } else {
            key
        };
        let mut out = Vec::with_capacity(reqs.len());
        for (i, r) in reqs.iter().enumerate() {
            let ttl = match self.check(r, &p) {
                Ok(t) => t,
                Err(msg) => return plain(ctx, 400, &format!("tokens[{i}]: {msg}"), &[]),
            };
            let grant = Grant {
                publish: r.publish.clone(),
                play: r.play.clone(),
            };
            let signed = if ring {
                p.issue_key(&key, &r.subject, &grant, ttl)
            } else {
                p.issue(&r.subject, &grant, ttl)
            };
            match signed {
                Ok((tok, exp)) => {
                    let k = if key.is_empty() {
                        String::new()
                    } else {
                        format!(",\"key\":{}", quote(&key))
                    };
                    out.push(format!(
                        "{{\"token\":{},\"subject\":{},\"expires\":{exp}{k}}}",
                        quote(&tok),
                        quote(&r.subject)
                    ));
                }
                Err(e) if e == crate::auth::Refusal::Unavailable.to_string() => {
                    return plain(
                        ctx,
                        503,
                        "the media_auth policy is not loaded",
                        &[("Retry-After", "1")],
                    );
                }
                Err(e) => return plain(ctx, 400, &format!("tokens[{i}]: {e}"), &[]),
            }
        }
        ctx.respond(json(format!("{{\"tokens\":[{}]}}", out.join(","))));
    }

    /// One token of a batch checked: its lifetime, the request's or the policy's `token_ttl`,
    /// never past `max_ttl` (fit's `check`).
    fn check(&self, r: &TokenRequest, p: &Policy) -> Result<Duration, String> {
        if r.subject.is_empty()
            || r.subject.len() > MAX_SUBJECT_BYTES
            || r.subject.chars().any(|c| c < '\u{20}' || c == '\u{7f}')
        {
            return Err(format!(
                "{:?} is not a subject: 1 to {MAX_SUBJECT_BYTES} bytes with no control characters",
                r.subject
            ));
        }
        if r.publish.is_empty() && r.play.is_empty() {
            return Err("a token needs at least one publish or play pattern".into());
        }
        for pat in r.publish.iter().chain(&r.play) {
            if !valid_media_pattern(pat) {
                return Err(format!(
                    "{pat:?} is not a stream pattern: 1 to 256 letters, digits, '.', '_', '~', '-', '/' and '*'"
                ));
            }
            if !self.prefixes.is_empty()
                && !self.prefixes.iter().any(|x| pat.starts_with(x.as_str()))
            {
                return Err(format!(
                    "{pat:?} starts with none of the prefixes this route may sign for: {}",
                    self.prefixes.join(", ")
                ));
            }
        }
        if r.ttl.is_empty() {
            let ttl = p.ttl();
            return Ok(if ttl.is_zero() || ttl > self.max_ttl {
                self.max_ttl
            } else {
                ttl
            });
        }
        match akit_config::duration::parse(&r.ttl) {
            Some(Ok(d)) if !d.is_zero() => {
                if d > self.max_ttl {
                    Err(format!(
                        "the lifetime {} is longer than max_ttl {}",
                        akit_config::duration::format(d),
                        akit_config::duration::format(self.max_ttl)
                    ))
                } else {
                    Ok(d)
                }
            }
            _ => Err(format!("{:?} is not a positive duration, as 2h", r.ttl)),
        }
    }

    /// Playback URLs signed (`url_signing`) for one viewer (fit's `urls`): each path's resource,
    /// as [`crate::urlsign::resource_of`] gives it, for the subject, bound to the address when
    /// one is named, in the session named or a new one; all or none, as a batch of tokens.
    fn urls(&self, ctx: &mut Context) {
        let Some(b) = body(ctx, self.max_body) else {
            return;
        };
        let Some(p) = self.policy(ctx) else { return };
        if !p.url_signing() {
            return plain(ctx, 404, "the media_auth policy has no url_signing", &[]);
        }
        let Some(r) = urls_request(&b) else {
            return plain(
                ctx,
                400,
                "the body must be JSON: {\"subject\": \"...\", \"paths\": [...]}",
                &[],
            );
        };
        match self.sign_urls(&p, r) {
            Ok(body) => ctx.respond(json(body)),
            Err((status, msg)) => {
                let extra: &[(&str, &str)] = if status == 503 {
                    &[("Retry-After", "1")]
                } else {
                    &[]
                };
                plain(ctx, status, &msg, extra);
            }
        }
    }

    fn sign_urls(&self, p: &Policy, r: UrlsRequest) -> Result<String, (u16, String)> {
        let bad = |m: String| (400, m);
        if r.paths.is_empty() {
            return Err(bad("paths is empty: ask for at least one URL".into()));
        }
        if r.paths.len() > self.max_tokens {
            return Err(bad(format!(
                "paths has {} paths: at most {} a request",
                r.paths.len(),
                self.max_tokens
            )));
        }
        if !crate::urlsign::valid_subject(&r.subject) {
            return Err(bad(format!(
                "{:?} is not a subject: 1 to {MAX_SUBJECT_BYTES} bytes with no control characters",
                r.subject
            )));
        }
        if !r.session.is_empty() && !crate::urlsign::valid_session(&r.session) {
            return Err(bad(format!(
                "{:?} is not a session: 1 to {} letters, digits, '.', '_', '~' and '-'",
                r.session,
                crate::urlsign::MAX_SESSION_BYTES
            )));
        }
        let mut v = crate::auth::Viewer {
            subject: r.subject.clone(),
            address: None,
            session: r.session.clone(),
        };
        if !r.address.is_empty() {
            match r.address.parse::<std::net::IpAddr>() {
                Ok(a) => v.address = Some(a),
                Err(_) => return Err(bad(format!("{:?} is not an IP address", r.address))),
            }
        }
        if v.session.is_empty() {
            v.session = new_session();
        }
        let mut ttl = p.url_ttl().min(self.max_ttl);
        if !r.ttl.is_empty() {
            match akit_config::duration::parse(&r.ttl) {
                Some(Ok(d)) if d >= Duration::from_secs(1) => {
                    if d > self.max_ttl {
                        return Err(bad(format!(
                            "the lifetime {} is longer than max_ttl {}",
                            akit_config::duration::format(d),
                            akit_config::duration::format(self.max_ttl)
                        )));
                    }
                    ttl = d;
                }
                _ => {
                    return Err(bad(format!(
                        "{:?} is not a duration of a second or more, as 10m",
                        r.ttl
                    )));
                }
            }
        }
        let ring = !p.keys().is_empty();
        let key = if !r.key.is_empty() {
            if !ring {
                return Err(bad(
                    "the policy has one key: leave key out of the request".into()
                ));
            }
            if p.key_file(&r.key).is_none() {
                return Err(bad(format!("no key named {:?}", r.key)));
            }
            r.key.clone()
        } else if ring {
            p.default_key().unwrap_or_default()
        } else {
            String::new()
        };
        let expiry = crate::urlsign::now_secs()
            .saturating_add(i64::try_from(ttl.as_secs()).unwrap_or(i64::MAX));
        let mut urls = Vec::with_capacity(r.paths.len());
        for (i, path) in r.paths.iter().enumerate() {
            let Some(res) = crate::urlsign::resource_of(path) else {
                return Err(bad(format!(
                    "paths[{i}]: {path:?} is not a path a URL is signed for, as /vod/films/films/bbb/index.m3u8 or /live/cam.flv"
                )));
            };
            if !self.url_prefixes.is_empty()
                && !self
                    .url_prefixes
                    .iter()
                    .any(|x| path.starts_with(x.as_str()))
            {
                return Err(bad(format!(
                    "paths[{i}]: {path:?} starts with none of the url_prefixes this route may sign for: {}",
                    self.url_prefixes.join(", ")
                )));
            }
            // What the request holds is checked above: an error is a policy removed, or one that
            // lost url_signing, by a reload under way.
            let q = p.sign_url(&res, &v, expiry, &key).map_err(|_| {
                (
                    503,
                    "the media_auth policy is not loaded with url_signing".to_owned(),
                )
            })?;
            urls.push(quote(&format!("{}{path}?{q}", self.url_base)));
        }
        let k = if key.is_empty() {
            String::new()
        } else {
            format!(",\"key\":{}", quote(&key))
        };
        Ok(format!(
            "{{\"subject\":{},\"session\":{},\"expires\":{expiry}{k},\"urls\":[{}]}}",
            quote(&r.subject),
            quote(&v.session),
            urls.join(",")
        ))
    }

    /// A revoke: the policy's secret, one key of its keyring, or all of them, rotated, its new
    /// secret written to its file first; answered from the pool, or 504 past `revoke_timeout`.
    fn revoke(&self, ctx: &mut Context) {
        let Some(b) = body(ctx, self.max_body) else {
            return;
        };
        let (mut key, mut all) = (String::new(), false);
        if !b.iter().all(u8::is_ascii_whitespace) {
            let Some(members) = object(&b) else {
                return plain(
                    ctx,
                    400,
                    "the body must be JSON: {\"key\": \"...\"} or {\"all\": true}, or empty",
                    &[],
                );
            };
            for (k, v) in members {
                match (k.as_str(), v) {
                    ("key", Value::String(s)) => key = s,
                    ("all", Value::Bool(x)) => all = x,
                    ("key" | "all", Value::Null) => {}
                    ("key" | "all", _) => {
                        return plain(
                            ctx,
                            400,
                            "the body must be JSON: {\"key\": \"...\"} or {\"all\": true}, or empty",
                            &[],
                        );
                    }
                    _ => {}
                }
            }
        }
        let Some(p) = self.policy(ctx) else { return };
        let keys = p.keys();
        let ring = !keys.is_empty();
        if ring && key.is_empty() && !all {
            return plain(ctx, 400, "name a key to rotate, or all: true", &[]);
        }
        if !ring && !key.is_empty() {
            return plain(ctx, 400, "the policy has one key: leave key out", &[]);
        }
        let mut targets: Vec<(String, String)> = Vec::new();
        if ring && all {
            for k in keys {
                match p.key_file(&k).filter(|f| !f.is_empty()) {
                    Some(f) => targets.push((k, f)),
                    None => {
                        return plain(ctx, 500, &format!("no secret file for key {k:?}"), &[]);
                    }
                }
            }
        } else if ring {
            match p.key_file(&key) {
                Some(f) if !f.is_empty() => targets.push((key, f)),
                Some(_) => {
                    return plain(ctx, 500, &format!("no secret file for key {key:?}"), &[]);
                }
                None => return plain(ctx, 400, &format!("no key named {key:?}"), &[]),
            }
        } else {
            let f = p.secret_file();
            if f.is_empty() {
                return plain(ctx, 500, "the revoke has no secret to rotate", &[]);
            }
            targets.push((String::new(), f));
        }
        let Some(s) = ctx.suspend() else {
            return plain(
                ctx,
                503,
                "a revoke cannot wait now: try again",
                &[("Retry-After", "1")],
            );
        };
        let reply = Reply::new(s, self.revoke_timeout);
        let r2 = reply.clone();
        let job: Job = Box::new(move || {
            let mut rotated = Vec::new();
            for (key, file) in &targets {
                let secret = new_secret();
                if write_secret(file, &secret).is_err() {
                    return r2.answer(text(500, "the secret file cannot be written", &[]));
                }
                let r = if key.is_empty() {
                    p.rotate(&secret)
                } else {
                    p.rotate_key(key, &secret)
                };
                if let Err(e) = r {
                    let resp = if e == crate::auth::Refusal::Unavailable.to_string() {
                        text(
                            503,
                            "the media_auth policy is not loaded",
                            &[("Retry-After", "1")],
                        )
                    } else {
                        text(500, "the new secret cannot be used", &[])
                    };
                    return r2.answer(resp);
                }
                if !key.is_empty() {
                    rotated.push(quote(key));
                }
            }
            let since = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let keys = if rotated.is_empty() {
                String::new()
            } else {
                format!(",\"keys\":[{}]", rotated.join(","))
            };
            r2.answer(json(format!(
                "{{\"revoked\":true,\"since\":{since}{keys}}}"
            )));
        });
        if !self.pool.submit(job) {
            reply.answer(text(
                503,
                "a revoke is already waiting: try again",
                &[("Retry-After", "1")],
            ));
        }
    }
}

/// A suspended request, answered once: by the revoke's job or by its deadline.
#[derive(Clone)]
struct Reply(Arc<Mutex<Option<(Suspended, crate::deadline::Deadline)>>>);

impl Reply {
    fn new(s: Suspended, timeout: Duration) -> Reply {
        let slot: Arc<Mutex<Option<(Suspended, crate::deadline::Deadline)>>> =
            Arc::new(Mutex::new(None));
        let late = Reply(Arc::clone(&slot));
        let d = crate::deadline::after(timeout, move || {
            late.answer(text(
                504,
                "the secret files' write did not finish in time",
                &[],
            ));
        });
        *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some((s, d));
        Reply(slot)
    }

    fn answer(&self, resp: Response) {
        let taken = self.0.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some((s, d)) = taken {
            crate::deadline::cancel(d);
            s.resume(move |ctx| ctx.respond(resp));
        }
    }
}

type Job = Box<dyn FnOnce() + Send>;

/// The revoke pool: one thread, a queue of one (fit's `pool`).
struct RevokePool {
    tx: Mutex<Option<SyncSender<Job>>>,
}

impl RevokePool {
    fn start() -> Result<RevokePool, ProvisionError> {
        let (tx, rx) = mpsc::sync_channel::<Job>(1);
        std::thread::Builder::new()
            .name("akit-media-tokens".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    job();
                }
            })
            .map_err(|e| ProvisionError::new(format!("the revoke pool: {e}")))?;
        Ok(RevokePool {
            tx: Mutex::new(Some(tx)),
        })
    }

    fn submit(&self, job: Job) -> bool {
        let g = self.tx.lock().unwrap_or_else(PoisonError::into_inner);
        match g.as_ref().map(|tx| tx.try_send(job)) {
            Some(Ok(())) => true,
            Some(Err(TrySendError::Full(_) | TrySendError::Disconnected(_))) | None => false,
        }
    }

    /// Refuses jobs from now on; the thread ends once the job it holds is done.
    fn close(&self) {
        self.tx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }
}

/// A new signing secret: 32 random bytes in unpadded base64url (fit's `newSecret`).
fn new_secret() -> String {
    let mut b = [0u8; 32];
    if akio_crypto::rand::fill_random(&mut b).is_err() {
        // Never a predictable secret: the rotation fails instead (too short).
        return String::new();
    }
    akio_crypto::base64::URL_SAFE_NO_PAD.encode(b)
}

/// Writes `secret` to `path` atomically: a temporary file beside it, mode 0600, renamed over it
/// (fit's `writeSecret`). It blocks: the revoke pool's.
fn write_secret(path: &str, secret: &str) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let target = std::path::Path::new(path);
    let dir = target.parent().unwrap_or_else(|| std::path::Path::new("."));
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("secret");
    let mut rnd = [0u8; 8];
    let _ = akio_crypto::rand::fill_random(&mut rnd);
    let tmp = dir.join(format!("{name}.tmp-{}", crate::library::catalog::hex(&rnd)));
    let written = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(secret.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, target)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}
