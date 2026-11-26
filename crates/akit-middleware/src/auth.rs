//! The authentication middleware, as fit's `middleware/auth`: `basic_auth`, users and their
//! passwords' bcrypt hashes, and `client_cert`, the certificate a client sent to a listener with
//! `tls.client_auth`.
//!
//! - `basic_auth` asks for a user and password (HTTP Basic, RFC 7617) and checks them against
//!   bcrypt hashes (`akio_crypto::bcrypt`). A bcrypt check takes milliseconds of CPU (tens at
//!   cost 10), so it never runs on an I/O thread (rule 1): the request is suspended
//!   ([`Context::suspend_to_pool`]), the password checked on akio's logical pool, and the request
//!   goes on from there to the next middleware and the handler, or is refused. A password that
//!   passed is remembered as an HMAC-SHA-256 under a random key of the middleware's, which no one
//!   could check passwords against, so that the same user's next requests are let in inline, at
//!   the cost of one HMAC compared in constant time. An unknown user's password is checked
//!   against another user's hash all the same, so that a refusal takes as long whether the user
//!   exists or not. As in fit, at most [`MAX_CHECKS`] checks wait or run at once, and one that
//!   waited past [`MAX_WAIT`] is answered 503, so that a flood of wrong passwords takes at most
//!   the pool, while remembered users are let in meanwhile.
//!
//!   A request whose body streams is suspended with its body's reads held (akio's ADR-159): the
//!   body streams to the handler once the password passed, as the handler asks.
//! - `client_cert` lets a request through only when its client sent a certificate the listener
//!   verified (akio's `Request::peer_certificates`), and may pass its subject on, as Go writes a
//!   distinguished name.
//!
//! A field the middleware sets for the handler (`user_header`, `subject_header`) is first removed
//! from the request in every spelling a client could send to pass for it with an application that
//! reads `_` as `-` (CGI, FastCGI and the frameworks behind them), as fit's `fieldSet` removes it.
//!
//! Who a request was authenticated as goes into fit's access log line as `user`; akit's line
//! does not carry it yet: akio's response observer (A29) is told two words a request set, not a
//! name.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use akio_crypto::bcrypt;
use akio_crypto::hash::Sha256;
use akio_crypto::mac::Hmac;
use akio_http::{BodyMode, Context, Headers, Request, Response};
use akit_runtime::{
    Handler, HandlerParams, Middleware, Provision, ProvisionError, Provisioned, set_request_headers,
};

use crate::url::canonical_key;

/// fit's (Go's) body of a 401.
pub(crate) const UNAUTHORIZED: &str = "Unauthorized\n";

/// fit's (Go's) body of a 403.
const FORBIDDEN: &str = "Forbidden\n";

/// The most password checks waiting or running at once, as fit's queue.
pub(crate) const MAX_CHECKS: usize = 1024;

/// How long a check may wait for the pool before it is answered 503, as fit's.
pub(crate) const MAX_WAIT: Duration = Duration::from_secs(5);

// ---- basic_auth ----

/// A user's account: their password's hash, and the HMAC of the password last found to be
/// theirs.
struct Account {
    hash: bcrypt::Hash,
    good: Remembered,
}

/// An HMAC-SHA-256 remembered, read without a lock: a sequence count, odd while a writer is
/// storing, around the four words, as a seqlock. Writers take `write`; a read that sees the count
/// move reads as nothing remembered (the password is checked anew).
#[derive(Default)]
struct Remembered {
    seq: AtomicU64,
    words: [AtomicU64; 4],
    write: Mutex<()>,
}

impl Remembered {
    fn load(&self) -> Option<[u8; 32]> {
        let before = self.seq.load(SeqCst);
        if before == 0 || before % 2 == 1 {
            return None;
        }
        let mut out = [0u8; 32];
        for (chunk, w) in out.chunks_exact_mut(8).zip(&self.words) {
            chunk.copy_from_slice(&w.load(SeqCst).to_be_bytes());
        }
        (self.seq.load(SeqCst) == before).then_some(out)
    }

    fn store(&self, mac: &[u8; 32]) {
        let _guard = self.write.lock().unwrap_or_else(PoisonError::into_inner);
        self.seq.fetch_add(1, SeqCst);
        for (chunk, w) in mac.chunks_exact(8).zip(&self.words) {
            let mut b = [0u8; 8];
            b.copy_from_slice(chunk);
            w.store(u64::from_be_bytes(b), SeqCst);
        }
        self.seq.fetch_add(1, SeqCst);
    }

    /// Whether `mac` is the one remembered, compared in constant time.
    fn matches(&self, mac: &[u8; 32]) -> bool {
        self.load()
            .is_some_and(|good| akio_crypto::ct_eq(&good, mac))
    }
}

/// The `basic_auth` middleware, compiled; shared by every handler it wraps.
pub(crate) struct Basic {
    accounts: HashMap<Vec<u8>, Account>,
    /// The hash an unknown user's password is checked against: the first user's.
    decoy: bcrypt::Hash,
    /// Keyed with a random key when compiled.
    mac: Hmac<Sha256>,
    /// The `WWW-Authenticate` value.
    challenge: String,
    /// `user_header`, canonical; empty when not set.
    user_header: String,
    forward_authorization: bool,
    checks: Arc<Checks>,
}

impl std::fmt::Debug for Basic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Basic")
            .field("users", &self.accounts.len())
            .field("challenge", &self.challenge)
            .field("user_header", &self.user_header)
            .finish_non_exhaustive()
    }
}

pub(crate) fn new_basic(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Middleware>, ProvisionError> {
    let HandlerParams::BasicAuth(b) = p.params() else {
        return Err(ProvisionError::new(
            "the basic_auth middleware was given another's params",
        ));
    };
    Ok(Provisioned::new(
        Arc::new(BasicMiddleware(Arc::new(Basic::new(b, Checks::shared())?)))
            as Arc<dyn Middleware>,
    ))
}

impl Basic {
    fn new(b: &akit_config::BasicAuth, checks: Arc<Checks>) -> Result<Basic, ProvisionError> {
        let mut accounts = HashMap::with_capacity(b.users.len());
        let mut decoy = None;
        for (i, u) in b.users.iter().enumerate() {
            // akit-config checked the hashes; this keeps a config built another way sound.
            let hash = bcrypt::Hash::parse(&u.password_hash).map_err(|e| {
                ProvisionError::field(
                    "users",
                    format!("user {i}: password_hash is not a bcrypt hash: {e}"),
                )
            })?;
            decoy.get_or_insert_with(|| hash.clone());
            let account = Account {
                hash,
                good: Remembered::default(),
            };
            if accounts
                .insert(u.username.as_bytes().to_vec(), account)
                .is_some()
            {
                return Err(ProvisionError::field(
                    "users",
                    format!("user {i}: {:?} is listed twice", u.username),
                ));
            }
        }
        let decoy =
            decoy.ok_or_else(|| ProvisionError::field("users", "at least one user is needed"))?;
        let mut key = [0u8; 32];
        akio_crypto::rand::fill_random(&mut key)
            .map_err(|e| ProvisionError::new(format!("no randomness for the key: {e}")))?;
        Ok(Basic {
            accounts,
            decoy,
            mac: Hmac::new(&key),
            challenge: format!("Basic realm=\"{}\", charset=\"UTF-8\"", b.realm),
            user_header: if b.user_header.is_empty() {
                String::new()
            } else {
                canonical_key(&b.user_header)
            },
            forward_authorization: b.forward_authorization,
            checks,
        })
    }

    /// What is remembered of `password`.
    fn mac_of(&self, password: &[u8]) -> [u8; 32] {
        self.mac.clone().chain(password).finalize()
    }

    /// Whether `password` is the one remembered for `user`.
    fn remembers(&self, user: &[u8], password: &[u8]) -> bool {
        self.accounts
            .get(user)
            .is_some_and(|a| a.good.matches(&self.mac_of(password)))
    }

    /// Answers 401, asking for a user and password.
    fn refuse(&self, ctx: &mut Context) {
        let mut resp = Response::with_body(
            401,
            "text/plain; charset=utf-8",
            UNAUTHORIZED.as_bytes().to_vec(),
        );
        resp.headers.set("WWW-Authenticate", &self.challenge);
        ctx.respond(resp);
    }

    /// The slow check of `password` for `user`: bcrypt, against the user's hash or the decoy.
    /// A password that passes is remembered. Runs on the pool.
    fn verify(&self, user: &[u8], password: &[u8]) -> bool {
        let account = self.accounts.get(user);
        let ok = account.map_or(&self.decoy, |a| &a.hash).verify(password);
        match account {
            Some(a) if ok => {
                a.good.store(&self.mac_of(password));
                true
            }
            _ => false,
        }
    }

    /// Lets `user` in: the request as the handler sees it loses `Authorization` (unless
    /// `forward_authorization`) and any field passing for `user_header`, which is set to `user`.
    fn admit(&self, ctx: &mut Context, user: &[u8]) {
        // A user name the accounts hold came from the config, as UTF-8.
        let name = String::from_utf8_lossy(user);
        let header = (!self.user_header.is_empty()).then(|| (self.user_header.as_str(), &*name));
        change_fields(ctx, !self.forward_authorization, header);
    }
}

/// Rewrites the request's fields for the handlers after a middleware: without `Authorization`
/// when `drop_auth`; without any field that passes for `set`'s name, which is set to its value.
/// The fields are left alone (no copy) when nothing changes.
fn change_fields(ctx: &mut Context, drop_auth: bool, set: Option<(&str, &str)>) {
    let fields = &ctx.req().headers;
    let gone = |n: &str| {
        (drop_auth && n.eq_ignore_ascii_case("authorization"))
            || set.is_some_and(|(h, _)| same_field(n, h))
    };
    if set.is_none() && !fields.iter().any(|(n, _)| gone(n)) {
        return;
    }
    let mut kept = Headers::with_capacity(fields.len().saturating_add(1), 0);
    for (n, v) in fields.iter().filter(|(n, _)| !gone(n)) {
        kept.append(n, v);
    }
    if let Some((h, v)) = set {
        kept.append(h, v);
    }
    set_request_headers(ctx, kept);
}

/// The `basic_auth` middleware's handle; the accounts are shared by every handler it wraps.
#[derive(Debug)]
struct BasicMiddleware(Arc<Basic>);

impl Middleware for BasicMiddleware {
    fn wrap(&self, next: Arc<dyn Handler>) -> Arc<dyn Handler> {
        Arc::new(BasicAuthed {
            basic: Arc::clone(&self.0),
            next,
        })
    }
}

struct BasicAuthed {
    basic: Arc<Basic>,
    next: Arc<dyn Handler>,
}

/// What a request's `Authorization` field says: a user and password, as Go's
/// `Request.BasicAuth` reads them.
pub(crate) fn credentials(headers: &Headers) -> Option<(Vec<u8>, Vec<u8>)> {
    let v = headers.get("authorization")?;
    let scheme = v.get(..6)?;
    if !scheme.eq_ignore_ascii_case("basic ") {
        return None;
    }
    let decoded = akio_crypto::base64::STANDARD.decode(v.get(6..)?).ok()?;
    let colon = decoded.iter().position(|&b| b == b':')?;
    let (user, rest) = decoded.split_at(colon);
    Some((user.to_vec(), rest.get(1..)?.to_vec()))
}

/// Answers 503 with `Retry-After: 1`: no room to check a password.
fn busy(ctx: &mut Context) {
    let mut resp = Response::new(503);
    resp.headers.set("Retry-After", "1");
    ctx.respond(resp);
}

impl akio_http::Service for BasicAuthed {
    fn call(&self, ctx: &mut Context) {
        let Some((user, password)) = credentials(&ctx.req().headers) else {
            self.basic.refuse(ctx);
            return;
        };
        if self.basic.remembers(&user, &password) {
            self.basic.admit(ctx, &user);
            self.next.call(ctx);
            return;
        }
        let Some(slot) = self.basic.checks.enter() else {
            busy(ctx);
            return;
        };
        let basic = Arc::clone(&self.basic);
        let next = Arc::clone(&self.next);
        let queued = Instant::now();
        let suspended = ctx.suspend_to_pool(move |s| {
            let _slot = slot;
            if queued.elapsed() > MAX_WAIT {
                s.resume(busy);
                return;
            }
            let ok = basic.verify(&user, &password);
            s.resume(|ctx| {
                if ok {
                    basic.admit(ctx, &user);
                    next.call(ctx);
                } else {
                    basic.refuse(ctx);
                }
            });
        });
        // Not suspended: the pool had no room (answered 503), or the request cannot be (a
        // resumed one already taking its streamed body).
        if !suspended && !ctx.responded() {
            busy(ctx);
        }
    }

    fn body_mode(&self, req: &Request) -> BodyMode {
        self.next.body_mode(req)
    }

    fn streams_bodies(&self) -> bool {
        self.next.streams_bodies()
    }
}

impl Handler for BasicAuthed {}

/// Whether field names `a` and `b` are the same once case is ignored and `_` is read as `-`: an
/// application behind CGI reads them as one.
pub(crate) fn same_field(a: &str, b: &str) -> bool {
    let fold = |c: u8| {
        if c == b'_' {
            b'-'
        } else {
            c.to_ascii_lowercase()
        }
    };
    a.len() == b.len() && a.bytes().zip(b.bytes()).all(|(x, y)| fold(x) == fold(y))
}

// ---- the checks ----

/// Password checks waiting or running, at most [`MAX_CHECKS`].
#[derive(Debug, Default)]
pub(crate) struct Checks {
    in_flight: AtomicUsize,
}

/// One check's place, given back when dropped.
struct Slot(Arc<Checks>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, SeqCst);
    }
}

impl Checks {
    /// The checks every `basic_auth` of the process shares, as fit's pool.
    fn shared() -> Arc<Checks> {
        static SHARED: std::sync::OnceLock<Arc<Checks>> = std::sync::OnceLock::new();
        Arc::clone(SHARED.get_or_init(Arc::default))
    }

    /// A place for one more check, or `None` when [`MAX_CHECKS`] are under way.
    fn enter(self: &Arc<Checks>) -> Option<Slot> {
        if self.in_flight.fetch_add(1, SeqCst) >= MAX_CHECKS {
            self.in_flight.fetch_sub(1, SeqCst);
            return None;
        }
        Some(Slot(Arc::clone(self)))
    }
}

// ---- client_cert ----

pub(crate) fn new_client_cert(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Middleware>, ProvisionError> {
    let HandlerParams::ClientCert(c) = p.params() else {
        return Err(ProvisionError::new(
            "the client_cert middleware was given another's params",
        ));
    };
    Ok(Provisioned::new(Arc::new(ClientCert {
        optional: c.optional,
        subject_header: Arc::from(if c.subject_header.is_empty() {
            String::new()
        } else {
            canonical_key(&c.subject_header)
        }),
    }) as Arc<dyn Middleware>))
}

/// The `client_cert` middleware, compiled.
#[derive(Debug, Clone)]
struct ClientCert {
    optional: bool,
    /// Canonical; empty when not set.
    subject_header: Arc<str>,
}

impl Middleware for ClientCert {
    fn wrap(&self, next: Arc<dyn Handler>) -> Arc<dyn Handler> {
        Arc::new(CertChecked {
            cc: self.clone(),
            next,
        })
    }
}

struct CertChecked {
    cc: ClientCert,
    next: Arc<dyn Handler>,
}

impl akio_http::Service for CertChecked {
    fn call(&self, ctx: &mut Context) {
        let header = &*self.cc.subject_header;
        let Some(chain) = ctx.req().peer_certificates.clone() else {
            if !self.cc.optional {
                ctx.respond(Response::with_body(
                    403,
                    "text/plain; charset=utf-8",
                    FORBIDDEN.as_bytes().to_vec(),
                ));
                return;
            }
            if !header.is_empty() {
                // Removed with or without a certificate: a client may not name its own subject.
                remove_field(ctx, header);
            }
            self.next.call(ctx);
            return;
        };
        if !header.is_empty() {
            let subject = header_safe(&subject_of(&chain));
            change_fields(ctx, false, Some((header, &subject)));
        }
        self.next.call(ctx);
    }

    fn body_mode(&self, req: &Request) -> BodyMode {
        self.next.body_mode(req)
    }

    fn streams_bodies(&self) -> bool {
        self.next.streams_bodies()
    }
}

impl Handler for CertChecked {}

/// Removes from the request every field that passes for `name`.
fn remove_field(ctx: &mut Context, name: &str) {
    let fields = &ctx.req().headers;
    if !fields.iter().any(|(n, _)| same_field(n, name)) {
        return;
    }
    let mut kept = Headers::with_capacity(fields.len(), 0);
    for (n, v) in fields.iter().filter(|(n, _)| !same_field(n, name)) {
        kept.append(n, v);
    }
    set_request_headers(ctx, kept);
}

/// A chain, weakly, and its leaf's subject.
type Subject = Option<(Weak<[Vec<u8>]>, String)>;

thread_local! {
    /// The last chain whose subject this thread wrote, and the subject: a connection's
    /// requests share their chain, so its leaf is parsed once per run of them.
    static LAST_SUBJECT: RefCell<Subject> = const { RefCell::new(None) };
}

/// The subject of `chain`'s leaf, as Go's `pkix.Name.String` writes it; empty when the leaf does
/// not parse (the handshake verified it, so it does).
fn subject_of(chain: &Arc<[Vec<u8>]>) -> String {
    let cached = LAST_SUBJECT.with(|c| {
        c.borrow().as_ref().and_then(|(w, s)| {
            w.upgrade()
                .is_some_and(|a| Arc::ptr_eq(&a, chain))
                .then(|| s.clone())
        })
    });
    if let Some(s) = cached {
        return s;
    }
    let subject = chain
        .first()
        .and_then(|der| akio_x509::Certificate::from_der(der.clone()).ok())
        .map(|cert| go_name(cert.subject()))
        .unwrap_or_default();
    LAST_SUBJECT.with(|c| {
        if let Ok(mut c) = c.try_borrow_mut() {
            *c = Some((Arc::downgrade(chain), subject.clone()));
        }
    });
    subject
}

/// The types Go's `pkix.Name` keeps in fields of its own, in the order `ToRDNSequence` writes
/// them, with their names: each type's values become one RDN.
const GO_NAME_TYPES: &[(&[u8], &str)] = &[
    (&[0x55, 0x04, 0x06], "C"),
    (&[0x55, 0x04, 0x08], "ST"),
    (&[0x55, 0x04, 0x07], "L"),
    (&[0x55, 0x04, 0x09], "STREET"),
    (&[0x55, 0x04, 0x11], "POSTALCODE"),
    (&[0x55, 0x04, 0x0a], "O"),
    (&[0x55, 0x04, 0x0b], "OU"),
    (&[0x55, 0x04, 0x03], "CN"),
    (&[0x55, 0x04, 0x05], "SERIALNUMBER"),
];

/// `name` as Go's `pkix.Name.String` writes it (RFC 2253-like, most specific first): the other
/// attribute types, each its own RDN, then those of [`GO_NAME_TYPES`] in that order (a common
/// name and serial number by their last value), the RDNs written last first, joined by `,`,
/// the values of one by `+`; an attribute of another type by its OID, and a value that is not
/// a string as `#` and its DER in hex.
fn go_name(name: &akio_x509::Name) -> String {
    let attrs: Vec<&akio_x509::Attribute> = name.rdns().iter().flatten().collect();
    let mut rdns: Vec<Vec<String>> = Vec::new();
    for a in &attrs {
        if !GO_NAME_TYPES
            .iter()
            .any(|(oid, _)| *oid == a.oid.as_slice())
        {
            let value = match a.value_str() {
                Some(v) => escape_value(&v),
                None => format!("#{}", hex(&der_of(a))),
            };
            rdns.push(vec![format!("{}={value}", oid_string(&a.oid))]);
        }
    }
    for (oid, label) in GO_NAME_TYPES {
        let mut values: Vec<String> = attrs
            .iter()
            .filter(|a| a.oid.as_slice() == *oid)
            .filter_map(|a| a.value_str())
            .collect();
        if *label == "CN" || *label == "SERIALNUMBER" {
            values = values.pop().into_iter().filter(|v| !v.is_empty()).collect();
        }
        if !values.is_empty() {
            rdns.push(
                values
                    .iter()
                    .map(|v| format!("{label}={}", escape_value(v)))
                    .collect(),
            );
        }
    }
    rdns.iter()
        .rev()
        .map(|rdn| rdn.join("+"))
        .collect::<Vec<_>>()
        .join(",")
}

/// A value escaped as Go's `RDNSequence.String` escapes it.
fn escape_value(v: &str) -> String {
    let last = v.len().saturating_sub(1);
    let mut out = String::with_capacity(v.len());
    for (k, c) in v.char_indices() {
        let escape = match c {
            ',' | '+' | '"' | '\\' | '<' | '>' | ';' => true,
            ' ' => k == 0 || k == last,
            '#' => k == 0,
            _ => false,
        };
        if escape {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// An attribute's value as DER: its tag, length and bytes.
fn der_of(a: &akio_x509::Attribute) -> Vec<u8> {
    let len = a.value.len();
    let mut out = vec![a.tag];
    if len < 0x80 {
        out.push(u8::try_from(len).unwrap_or(0));
    } else {
        let bytes = len.to_be_bytes();
        let skip = bytes.iter().take_while(|&&b| b == 0).count();
        out.push(0x80 | u8::try_from(bytes.len().saturating_sub(skip)).unwrap_or(0));
        out.extend_from_slice(bytes.get(skip..).unwrap_or(&[]));
    }
    out.extend_from_slice(&a.value);
    out
}

/// An OID's content bytes in dotted form.
fn oid_string(der: &[u8]) -> String {
    let mut arcs: Vec<u64> = Vec::new();
    let mut v: u64 = 0;
    for &b in der {
        v = v.checked_shl(7).unwrap_or(0) | u64::from(b & 0x7f);
        if b & 0x80 == 0 {
            if arcs.is_empty() {
                let first = (v / 40).min(2);
                arcs.push(first);
                arcs.push(v.saturating_sub(first.saturating_mul(40)));
            } else {
                arcs.push(v);
            }
            v = 0;
        }
    }
    arcs.iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

fn hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    b.iter().fold(
        String::with_capacity(b.len().saturating_mul(2)),
        |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        },
    )
}

/// `s` with the bytes a field value may not hold, the control characters but tab, replaced by
/// `?`, as fit's `headerSafe`.
pub(crate) fn header_safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            if (c < ' ' && c != '\t') || c == '\x7f' {
                '?'
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn same_fields() {
        for (a, b, same) in [
            ("X-User", "X-User", true),
            ("x_user", "X-User", true),
            ("X_USER", "X-User", true),
            ("X-Forwarded_User", "X-Forwarded-User", true),
            ("X-Users", "X-User", false),
            ("X-Other", "X-User", false),
            ("XUser", "X-User", false),
        ] {
            assert_eq!(same_field(a, b), same, "{a} {b}");
        }
    }

    #[test]
    fn credentials_read_as_go_reads_them() {
        let h = |v: &str| {
            let mut h = Headers::new();
            h.append("Authorization", v);
            h
        };
        let b64 = |s: &str| akio_crypto::base64::STANDARD.encode(s);
        assert_eq!(
            credentials(&h(&format!("Basic {}", b64("alice:pw")))),
            Some((b"alice".to_vec(), b"pw".to_vec()))
        );
        assert_eq!(
            credentials(&h(&format!("bAsIc {}", b64("a:b:c")))),
            Some((b"a".to_vec(), b"b:c".to_vec()))
        );
        assert_eq!(
            credentials(&h(&format!("Basic {}", b64(":")))),
            Some((Vec::new(), Vec::new()))
        );
        for bad in [
            "Basic".to_owned(),
            format!("Bearer {}", b64("a:b")),
            format!("Basic {}", b64("nocolon")),
            "Basic !!!".to_owned(),
            format!("Basic  {}", b64("a:b")),
        ] {
            assert_eq!(credentials(&h(&bad)), None, "{bad}");
        }
        assert_eq!(credentials(&Headers::new()), None);
    }

    #[test]
    fn remembered_macs() {
        let r = Remembered::default();
        let a = [1u8; 32];
        let mut b = [1u8; 32];
        b[31] = 2;
        assert!(!r.matches(&a), "nothing remembered");
        r.store(&a);
        assert!(r.matches(&a));
        assert!(!r.matches(&b));
        r.store(&b);
        assert!(r.matches(&b) && !r.matches(&a));
    }

    #[test]
    fn checks_bounded() {
        let c = Arc::new(Checks::default());
        let slots: Vec<Slot> = (0..MAX_CHECKS).map(|_| c.enter().unwrap()).collect();
        assert!(c.enter().is_none(), "one past the bound");
        drop(slots);
        assert_eq!(c.in_flight.load(SeqCst), 0);
        assert!(c.enter().is_some());
    }

    // fit's TestHeaderSafe.
    #[test]
    fn header_safe_values() {
        for (input, want) in [
            ("CN=alice", "CN=alice"),
            ("CN=a\tb", "CN=a\tb"),
            ("CN=a\r\nX-Evil: 1", "CN=a??X-Evil: 1"),
            ("\x00\x7f", "??"),
            ("CN=h\u{e9}llo", "CN=h\u{e9}llo"),
        ] {
            assert_eq!(header_safe(input), want, "{input:?}");
        }
    }

    /// Names written as Go's `pkix.Name.String` writes them (its output for the same names).
    #[test]
    fn names_as_go_writes_them() {
        use akio_x509::{Name, attr};
        type Attrs<'a> = Vec<(&'a [u8], &'a str)>;
        let cases: Vec<(Attrs<'_>, &str)> = vec![
            (
                vec![
                    (attr::COMMON_NAME, "alice"),
                    (attr::ORGANIZATION, "Example"),
                ],
                "CN=alice,O=Example",
            ),
            (
                vec![
                    (attr::COUNTRY, "US"),
                    (attr::ORGANIZATION, "Example"),
                    (attr::ORGANIZATIONAL_UNIT, "Eng"),
                    (attr::ORGANIZATIONAL_UNIT, "Ops"),
                    (attr::COMMON_NAME, "bob"),
                ],
                "CN=bob,OU=Eng+OU=Ops,O=Example,C=US",
            ),
            (
                vec![
                    (attr::COMMON_NAME, "a,b+c \"d\" <e>;#"),
                    (attr::LOCALITY, " x "),
                ],
                r#"CN=a\,b\+c \"d\" \<e\>\;#,L=\ x\ "#,
            ),
            (vec![(attr::COMMON_NAME, "#lead")], r"CN=\#lead"),
            (
                vec![(attr::EMAIL_ADDRESS, "a@b.c"), (attr::COMMON_NAME, "alice")],
                "CN=alice,1.2.840.113549.1.9.1=a@b.c",
            ),
            (
                vec![(attr::COMMON_NAME, "first"), (attr::COMMON_NAME, "last")],
                "CN=last",
            ),
            (vec![], ""),
        ];
        for (attrs, want) in cases {
            assert_eq!(
                go_name(&Name::from_attributes(attrs.clone())),
                want,
                "{attrs:?}"
            );
        } // A value that is not a string (INTEGER 5 under 1.2.3): its DER in hex.
        let der = [
            0x30, 0x0b, 0x31, 0x09, 0x30, 0x07, 0x06, 0x02, 0x2a, 0x03, 0x02, 0x01, 0x05,
        ];
        assert_eq!(go_name(&Name::from_der(&der).unwrap()), "1.2.3=#020105");
    }

    #[test]
    fn oids_dotted() {
        assert_eq!(oid_string(&[0x55, 0x04, 0x03]), "2.5.4.3");
        assert_eq!(
            oid_string(&[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x01]),
            "1.2.840.113549.1.9.1"
        );
    }
}
