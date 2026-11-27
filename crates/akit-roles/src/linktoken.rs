//! The tokens a listener issues for the links of the tunnel and of the LLM mesh, in place of a
//! password every member shares: fit's `internal/linktoken` (fit's decision 035), with fit's keys
//! and proofs byte for byte (decision 038), so that a token's proof is the same on either.
//!
//! # Keys
//!
//! A token is [`LINK_TOKEN_PREFIX`], its ID (16 hex digits) and a 32-byte random secret. From the
//! secret:
//!
//! ```text
//! client = HMAC-SHA256(secret, "fit link token client key")
//! stored = SHA-256(client)                 the token's hash, all the listener keeps
//! server = HKDF-SHA256(stored, info "fit link token server key")
//! body   = HKDF-SHA256(stored, info "fit link token body key")
//! ```
//!
//! # Handshake
//!
//! The dialer and the listener each pick a fresh 32-byte nonce. The transcript is a label naming
//! the protocol, the token's ID, the dialer's nonce, the listener's nonce and what else the
//! protocol binds, each after its length. The dialer proves the token with
//! `client XOR HMAC-SHA256(stored, "dialer to listener" | transcript)`: the listener recovers
//! `client` and checks its SHA-256 against `stored`, so that what the listener keeps is not enough
//! to link. The listener proves the token back with `HMAC-SHA256(server, "listener to dialer" |
//! transcript | its answer)`. The proofs name their direction and both nonces, so a proof sent
//! back, replayed on another link or answered to another nonce does not check.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use akio_crypto::base64::URL_SAFE_NO_PAD;
use akio_crypto::hash::Sha256;
use akio_crypto::mac::Hmac;
use akit_config::roles::{
    LINK_TOKEN_PREFIX, LinkToken, MAX_LINK_TOKEN_MEMBERS, MAX_LINK_TOKENS, check_link_token_string,
    valid_link_token_id,
};
use akit_config::value::Value;

/// The size of a token's secret.
pub const SECRET_SIZE: usize = 32;
/// The size of a handshake's nonces.
pub const NONCE_SIZE: usize = 32;
/// The size of a proof.
pub const PROOF_SIZE: usize = 32;

// fit's labels, byte for byte (decision 038).
const LABEL_CLIENT: &str = "fit link token client key";
const LABEL_SERVER: &str = "fit link token server key";
const LABEL_BODY: &str = "fit link token body key";
const LABEL_DIALER: &str = "fit link token v1 dialer to listener";
const LABEL_LISTENER: &str = "fit link token v1 listener to dialer";

/// The longest a registry waits before it looks again for tokens that expired: a token's links
/// are closed at its expiry, or at most this late when the wall clock jumped.
pub const MAX_SWEEP: Duration = Duration::from_secs(60);

/// A token as its holder has it.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret {
    id: String,
    secret: Vec<u8>,
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Secret")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut m = Hmac::<Sha256>::new(key);
    for p in parts {
        m.update(p);
    }
    m.finalize()
}

fn derive(stored: &[u8], info: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    // HKDF-SHA256 of a 32-byte key to 32 bytes cannot fail.
    let _ = akio_crypto::kdf::hkdf::<Sha256>(&[], stored, info.as_bytes(), &mut out);
    out
}

/// Random bytes from the operating system.
fn random<const N: usize>() -> std::io::Result<[u8; N]> {
    let mut b = [0u8; N];
    akio_crypto::rand::fill_random(&mut b)?;
    Ok(b)
}

impl Secret {
    /// Makes a token: a random ID and a random secret.
    ///
    /// # Errors
    ///
    /// When the operating system gives no random bytes.
    pub fn new() -> std::io::Result<Secret> {
        let id: [u8; 8] = random()?;
        let secret: [u8; SECRET_SIZE] = random()?;
        Ok(Secret {
            id: hex(&id),
            secret: secret.to_vec(),
        })
    }

    /// Reads a token, as [`Secret::token`] writes it.
    ///
    /// # Errors
    ///
    /// Why `tok` is not a token.
    pub fn parse(tok: &str) -> Result<Secret, String> {
        if let Some(msg) = check_link_token_string(tok) {
            return Err(msg);
        }
        let rest = tok.strip_prefix(LINK_TOKEN_PREFIX).unwrap_or_default();
        let (id, enc) = rest.split_once('.').unwrap_or_default();
        match URL_SAFE_NO_PAD.decode(enc) {
            Ok(s) if s.len() >= 16 => Ok(Secret {
                id: id.to_owned(),
                secret: s,
            }),
            _ => Err("the token's secret is not 16 or more bytes of base64url".to_owned()),
        }
    }

    /// The token's ID.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The token, which its holder configures: shown once, kept nowhere.
    #[must_use]
    pub fn token(&self) -> String {
        format!(
            "{LINK_TOKEN_PREFIX}{}.{}",
            self.id,
            URL_SAFE_NO_PAD.encode(&self.secret)
        )
    }

    fn client(&self) -> [u8; 32] {
        hmac(&self.secret, &[LABEL_CLIENT.as_bytes()])
    }

    /// What the listener keeps of the token: the SHA-256 of its client key.
    #[must_use]
    pub fn stored(&self) -> [u8; 32] {
        Sha256::digest(&self.client())
    }

    /// [`Secret::stored`] in hex, as a config's token has it.
    #[must_use]
    pub fn hash(&self) -> String {
        hex(&self.stored())
    }

    /// The dialer's proof of the token over `t`.
    #[must_use]
    pub fn prove(&self, t: &Transcript<'_>) -> [u8; PROOF_SIZE] {
        let sig = hmac(&self.stored(), &[&t.bytes(LABEL_DIALER)]);
        let c = self.client();
        let mut out = [0u8; PROOF_SIZE];
        for ((o, a), b) in out.iter_mut().zip(c).zip(sig) {
            *o = a ^ b;
        }
        out
    }

    /// Whether `proof` is the listener's proof over `t` and its `answer`.
    #[must_use]
    pub fn verify_listener(&self, t: &Transcript<'_>, proof: &[u8], answer: &[&str]) -> bool {
        akio_crypto::ct_eq(proof, &listener_proof(&self.stored(), t, answer))
    }
}

/// Decodes a config token's hash.
#[must_use]
pub fn stored_of(hash: &str) -> Option<[u8; 32]> {
    let b = unhex(hash)?;
    b.try_into().ok()
}

/// The key a link's bodies are sealed under derives from, for the token whose stored key is
/// `stored`.
#[must_use]
pub fn body_key(stored: &[u8]) -> [u8; 32] {
    derive(stored, LABEL_BODY)
}

/// What a handshake's proofs bind: `label` names the protocol, `token` the token's ID, `dialer`
/// and `listener` the two sides' nonces, and `extra` what else the protocol binds.
#[derive(Debug, Clone, Copy)]
pub struct Transcript<'a> {
    /// The protocol's label, as fit's.
    pub label: &'a str,
    /// The token's ID.
    pub token: &'a str,
    /// The dialer's nonce.
    pub dialer: &'a [u8],
    /// The listener's nonce.
    pub listener: &'a [u8],
    /// What else the protocol binds.
    pub extra: &'a [&'a str],
}

impl Transcript<'_> {
    /// The transcript, each part after its length, so that no two transcripts' parts run together
    /// into the same bytes.
    fn bytes(&self, direction: &str) -> Vec<u8> {
        let mut b = Vec::new();
        let mut put = |p: &[u8]| {
            b.extend_from_slice(&u32::try_from(p.len()).unwrap_or(u32::MAX).to_be_bytes());
            b.extend_from_slice(p);
        };
        put(direction.as_bytes());
        put(self.label.as_bytes());
        put(self.token.as_bytes());
        put(self.dialer);
        put(self.listener);
        for e in self.extra {
            put(e.as_bytes());
        }
        b
    }
}

/// Whether `proof` is a dialer's proof over `t` of the token whose stored key is `stored`.
#[must_use]
pub fn verify(stored: &[u8], t: &Transcript<'_>, proof: &[u8]) -> bool {
    if proof.len() != PROOF_SIZE || stored.len() != 32 {
        return false;
    }
    let sig = hmac(stored, &[&t.bytes(LABEL_DIALER)]);
    let c: Vec<u8> = proof.iter().zip(sig).map(|(a, b)| a ^ b).collect();
    akio_crypto::ct_eq(&Sha256::digest(&c), stored)
}

/// The listener's proof, over `t` and its `answer`, of the token whose stored key is `stored`.
#[must_use]
pub fn listener_proof(stored: &[u8], t: &Transcript<'_>, answer: &[&str]) -> [u8; PROOF_SIZE] {
    let mut m = Hmac::<Sha256>::new(&derive(stored, LABEL_SERVER));
    m.update(&t.bytes(LABEL_LISTENER));
    for a in answer {
        m.update(&u32::try_from(a.len()).unwrap_or(u32::MAX).to_be_bytes());
        m.update(a.as_bytes());
    }
    m.finalize()
}

/// A fresh nonce.
///
/// # Errors
///
/// When the operating system gives no random bytes.
pub fn nonce() -> std::io::Result<[u8; NONCE_SIZE]> {
    random()
}

/// `b` in lowercase hex.
#[must_use]
pub fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len().saturating_mul(2));
    for x in b {
        for d in [x >> 4, x & 15] {
            s.push(char::from_digit(u32::from(d), 16).unwrap_or('0'));
        }
    }
    s
}

/// `s`, hex digits of either case, as bytes.
#[must_use]
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    s.as_bytes()
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok())
        .collect()
}

// ---------------------------------------------------------------------------------------------
// The registry.

/// Why a token a link presents, or a change asked of the tokens, is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenError {
    /// The token was never issued here, or was revoked.
    Unknown,
    /// The token has expired.
    Expired,
    /// The member is bound to another token.
    Bound {
        /// The member.
        member: String,
        /// The token it first linked with.
        token: String,
    },
    /// The token is bound to as many members as it may be.
    Full,
    /// No such token, for a change.
    NotFound,
    /// What was asked is wrong: the pages and the Admin API answer it 400.
    BadRequest(String),
}

impl fmt::Display for TokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenError::Unknown => {
                f.write_str("linktoken: no such token: it was never issued here, or it was revoked")
            }
            TokenError::Expired => f.write_str("linktoken: the token has expired"),
            TokenError::Bound { member, token } => write!(
                f,
                "linktoken: the member is bound to another token: {member} first linked with \
                 token {token}"
            ),
            TokenError::Full => write!(
                f,
                "linktoken: the token is bound to {MAX_LINK_TOKEN_MEMBERS} members, as many as \
                 it may be"
            ),
            TokenError::NotFound => f.write_str("linktoken: no such token"),
            TokenError::BadRequest(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for TokenError {}

/// What runs when a link's token is revoked or expires: it closes the link.
pub type Closer = Box<dyn Fn() + Send + Sync>;

/// What runs, off the registry's lock, when a member links with a token for the first time, so
/// that the binding is written into the config.
pub type OnBind = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// The registry's clock: seconds since 1970.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// The wall clock, in seconds since 1970.
#[must_use]
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

struct Entry {
    rec: LinkToken,
    stored: [u8; 32],
    last_use: i64,
    last_addr: String,
    uses: HashMap<u64, UseRec>,
}

struct UseRec {
    member: String,
    addr: String,
    since: i64,
    close: Option<Arc<Closer>>,
}

#[derive(Default)]
struct Inner {
    tokens: HashMap<String, Entry>,
    bound: HashMap<String, String>,
    next_use: u64,
}

/// A listener's tokens, as its config lists them, and the links that use them. Safe for
/// concurrent use; nothing of it blocks but its short lock, and the links' closers run off it.
#[derive(Clone)]
pub struct Registry {
    inner: Arc<Mutex<Inner>>,
    on_bind: Option<OnBind>,
    now: Clock,
}

impl fmt::Debug for Registry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Registry").finish_non_exhaustive()
    }
}

impl Default for Registry {
    fn default() -> Registry {
        Registry::new(None, Arc::new(unix_now))
    }
}

/// A link that presented a token: dropping it forgets the link.
pub struct Use {
    inner: Arc<Mutex<Inner>>,
    token: String,
    key: u64,
    /// The member, as `node/<name>` or an LLM role's ID.
    pub member: String,
}

impl fmt::Debug for Use {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Use")
            .field("token", &self.token)
            .field("member", &self.member)
            .finish_non_exhaustive()
    }
}

impl Use {
    /// The ID of the token the link presented.
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }
}

impl Drop for Use {
    fn drop(&mut self) {
        let mut g = lock(&self.inner);
        if let Some(e) = g.tokens.get_mut(&self.token) {
            e.uses.remove(&self.key);
        }
    }
}

fn lock(m: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn close_all(closers: Vec<Arc<Closer>>) {
    for c in closers {
        c();
    }
}

impl Registry {
    /// A registry: `on_bind` runs when a member binds to a token, `now` is its clock.
    #[must_use]
    pub fn new(on_bind: Option<OnBind>, now: Clock) -> Registry {
        Registry {
            inner: Arc::new(Mutex::new(Inner::default())),
            on_bind,
            now,
        }
    }

    /// Moves the registry to `recs`, the listener's tokens as its config has them. The links of
    /// a token that is gone, whose hash changed, or that has expired are closed, at once and off
    /// the lock; the bindings of a token that is gone are forgotten. A binding not yet in the
    /// config is kept while its token is.
    pub fn set(&self, recs: &[LinkToken]) {
        let now = (self.now)();
        let mut closing = Vec::new();
        {
            let mut g = lock(&self.inner);
            let mut old = std::mem::take(&mut g.tokens);
            let mut next = HashMap::with_capacity(recs.len());
            let mut bound = HashMap::new();
            for rec in recs {
                let Some(stored) = stored_of(&rec.hash) else {
                    continue; // config validation refuses it
                };
                let mut e = match old.remove(&rec.id) {
                    Some(mut e) if e.rec.hash == rec.hash => {
                        e.rec = rec.clone();
                        e
                    }
                    other => {
                        if let Some(e) = other {
                            closing.extend(e.uses.into_values().filter_map(|u| u.close));
                        }
                        Entry {
                            rec: rec.clone(),
                            stored,
                            last_use: 0,
                            last_addr: String::new(),
                            uses: HashMap::new(),
                        }
                    }
                };
                if rec.expired(now) {
                    closing.extend(e.uses.drain().filter_map(|(_, u)| u.close));
                }
                for m in &rec.members {
                    bound.insert(m.clone(), rec.id.clone());
                }
                next.insert(rec.id.clone(), e);
            }
            for (_, e) in old {
                closing.extend(e.uses.into_values().filter_map(|u| u.close));
            }
            for (m, id) in std::mem::take(&mut g.bound) {
                if !bound.contains_key(&m) && next.contains_key(&id) {
                    bound.insert(m, id);
                }
            }
            g.tokens = next;
            g.bound = bound;
        }
        close_all(closing);
    }

    /// Closes the links of the tokens that have expired. The role calls it at least every
    /// [`Registry::next_sweep`].
    pub fn sweep(&self) {
        let now = (self.now)();
        let mut closing = Vec::new();
        {
            let mut g = lock(&self.inner);
            for e in g.tokens.values_mut() {
                if e.rec.expired(now) {
                    closing.extend(e.uses.drain().filter_map(|(_, u)| u.close));
                }
            }
        }
        close_all(closing);
    }

    /// How long until the next sweep: the next expiry of a token, at most [`MAX_SWEEP`]; `None`
    /// when no token expires.
    #[must_use]
    pub fn next_sweep(&self) -> Option<Duration> {
        let now = (self.now)();
        let g = lock(&self.inner);
        g.tokens
            .values()
            .filter(|e| e.rec.expires_at > now)
            .map(|e| e.rec.expires_at.saturating_sub(now))
            .min()
            .map(|s| Duration::from_secs(u64::try_from(s).unwrap_or(0)).min(MAX_SWEEP))
    }

    /// The stored key of token `id`, for a handshake to check a proof with.
    ///
    /// # Errors
    ///
    /// Why the token is refused.
    pub fn lookup(&self, id: &str) -> Result<[u8; 32], TokenError> {
        let now = (self.now)();
        let g = lock(&self.inner);
        match g.tokens.get(id) {
            None => Err(TokenError::Unknown),
            Some(e) if e.rec.expired(now) => Err(TokenError::Expired),
            Some(e) => Ok(e.stored),
        }
    }

    /// How many tokens the registry has.
    #[must_use]
    pub fn len(&self) -> usize {
        lock(&self.inner).tokens.len()
    }

    /// Whether the registry has no token.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Records a link, from `addr`, of `member` that proved token `id`: `close` closes the link
    /// when the token is revoked or expires. A member's first link binds it to the token.
    ///
    /// # Errors
    ///
    /// A token that is gone or has expired, a member bound to another token, a token bound to
    /// as many members as it may be.
    pub fn admit(
        &self,
        id: &str,
        member: &str,
        addr: &str,
        close: Option<Closer>,
    ) -> Result<Use, TokenError> {
        let now = (self.now)();
        let (u, first) = {
            let mut g = lock(&self.inner);
            let inner = &mut *g;
            match inner.tokens.get(id) {
                None => return Err(TokenError::Unknown),
                Some(e) if e.rec.expired(now) => return Err(TokenError::Expired),
                Some(_) => {}
            }
            let have = inner.bound.get(member).cloned();
            match &have {
                Some(t) if t != id => {
                    return Err(TokenError::Bound {
                        member: member.to_owned(),
                        token: t.clone(),
                    });
                }
                Some(_) => {}
                None => {
                    let n = inner.bound.values().filter(|t| *t == id).count();
                    if n >= MAX_LINK_TOKEN_MEMBERS {
                        return Err(TokenError::Full);
                    }
                    inner.bound.insert(member.to_owned(), id.to_owned());
                }
            }
            inner.next_use = inner.next_use.wrapping_add(1);
            let key = inner.next_use;
            let Some(e) = inner.tokens.get_mut(id) else {
                return Err(TokenError::Unknown);
            };
            e.uses.insert(
                key,
                UseRec {
                    member: member.to_owned(),
                    addr: addr.to_owned(),
                    since: now,
                    close: close.map(Arc::new),
                },
            );
            e.last_use = now;
            addr.clone_into(&mut e.last_addr);
            (
                Use {
                    inner: Arc::clone(&self.inner),
                    token: id.to_owned(),
                    key,
                    member: member.to_owned(),
                },
                have.is_none(),
            )
        };
        if first {
            if let Some(b) = &self.on_bind {
                b(id, member);
            }
        }
        Ok(u)
    }

    /// The tokens, by name and then ID, each with its last use since akit started, the members
    /// bound to it and the links that use it.
    #[must_use]
    pub fn list(&self) -> Vec<View> {
        let now = (self.now)();
        let g = lock(&self.inner);
        let mut out: Vec<View> = g
            .tokens
            .iter()
            .map(|(id, e)| view(&g, id, e, now))
            .collect();
        out.sort_by(|a, b| (&a.name, &a.id).cmp(&(&b.name, &b.id)));
        out
    }

    /// Token `id`'s view.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<View> {
        let now = (self.now)();
        let g = lock(&self.inner);
        g.tokens.get(id).map(|e| view(&g, id, e, now))
    }
}

fn view(g: &Inner, id: &str, e: &Entry, now: i64) -> View {
    let mut members: Vec<String> = g
        .bound
        .iter()
        .filter(|(_, t)| *t == id)
        .map(|(m, _)| m.clone())
        .collect();
    members.sort();
    let mut links: Vec<LinkView> = e
        .uses
        .values()
        .map(|u| LinkView {
            member: u.member.clone(),
            addr: u.addr.clone(),
            since: u.since,
        })
        .collect();
    links.sort_by(|a, b| (&a.member, &a.addr).cmp(&(&b.member, &b.addr)));
    View {
        id: id.to_owned(),
        name: e.rec.name.clone(),
        note: e.rec.note.clone(),
        created_at: e.rec.created_at,
        expires_at: e.rec.expires_at,
        expired: e.rec.expired(now),
        last_used: e.last_use,
        last_addr: e.last_addr.clone(),
        members,
        links,
    }
}

/// A token as the pages, the Admin API and the CLI list it: never its hash.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct View {
    /// The token's ID.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Its note.
    pub note: String,
    /// When it was made; 0 when unknown.
    pub created_at: i64,
    /// When it expires; 0, never.
    pub expires_at: i64,
    /// Whether it has expired.
    pub expired: bool,
    /// When it was last used since akit started; 0, not since.
    pub last_used: i64,
    /// The address it was last used from.
    pub last_addr: String,
    /// The members bound to it.
    pub members: Vec<String>,
    /// The links that use it now.
    pub links: Vec<LinkView>,
}

/// A link that uses a token.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LinkView {
    /// The member.
    pub member: String,
    /// The address it linked from.
    pub addr: String,
    /// Since when, in seconds since 1970.
    pub since: i64,
}

impl View {
    /// The view as fit's JSON: `omitempty` fields left out when empty.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut s = String::from("{");
        s.push_str("\"id\":");
        akit_config::value::write_string(&mut s, &self.id);
        s.push_str(",\"name\":");
        akit_config::value::write_string(&mut s, &self.name);
        if !self.note.is_empty() {
            s.push_str(",\"note\":");
            akit_config::value::write_string(&mut s, &self.note);
        }
        for (k, v) in [
            ("created_at", self.created_at),
            ("expires_at", self.expires_at),
        ] {
            if v != 0 {
                s.push_str(&format!(",\"{k}\":{v}"));
            }
        }
        s.push_str(&format!(",\"expired\":{}", self.expired));
        if self.last_used != 0 {
            s.push_str(&format!(",\"last_used\":{}", self.last_used));
        }
        if !self.last_addr.is_empty() {
            s.push_str(",\"last_addr\":");
            akit_config::value::write_string(&mut s, &self.last_addr);
        }
        s.push_str(",\"members\":[");
        for (i, m) in self.members.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            akit_config::value::write_string(&mut s, m);
        }
        s.push_str("],\"links\":[");
        for (i, l) in self.links.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str("{\"member\":");
            akit_config::value::write_string(&mut s, &l.member);
            s.push_str(",\"addr\":");
            akit_config::value::write_string(&mut s, &l.addr);
            s.push_str(&format!(",\"since\":{}}}", l.since));
        }
        s.push_str("]}");
        s
    }
}

// ---------------------------------------------------------------------------------------------
// Management: what the pages, the Admin API and the CLI change, as edits of the token list a
// listener's member holds. The caller applies the list as a config edit.

/// A request for a token: `expires_at`, in seconds since 1970, or `ttl`, a duration from now,
/// set its expiry; neither, never.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IssueReq {
    /// What the token is for.
    pub name: String,
    /// Anything else about it.
    pub note: String,
    /// When it expires.
    pub expires_at: i64,
    /// How long it is good for.
    pub ttl: String,
}

/// A change of a token's expiry, as [`IssueReq`]'s; neither, never.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExpiryReq {
    /// When it expires.
    pub expires_at: i64,
    /// How long it is good for, from now.
    pub ttl: String,
}

fn bad(msg: impl Into<String>) -> TokenError {
    TokenError::BadRequest(msg.into())
}

/// Reads a token's lifetime: a Go duration, as `720h`, or a whole number of days, as `30d`.
///
/// # Errors
///
/// A lifetime that is not one, or not positive.
pub fn parse_ttl(s: &str) -> Result<Duration, String> {
    let d = match s.strip_suffix('d') {
        Some(n) => n
            .parse::<i32>()
            .ok()
            .and_then(|days| u64::try_from(days).ok())
            .map(|days| Duration::from_secs(days.saturating_mul(24 * 3600))),
        None => akit_config::duration::parse(s).and_then(Result::ok),
    };
    match d {
        Some(d) if !d.is_zero() => Ok(d),
        _ => Err(format!(
            "ttl {s:?} is not a positive duration, as 720h or 30d"
        )),
    }
}

/// The expiry asked for, from `now`.
fn expiry(at: i64, ttl: &str, now: i64) -> Result<i64, TokenError> {
    if at != 0 && !ttl.is_empty() {
        return Err(bad("give expires_at or ttl, not both"));
    }
    if at < 0 {
        return Err(bad("expires_at is seconds since 1970, not negative"));
    }
    if at != 0 && at <= now {
        return Err(bad("expires_at is in the past"));
    }
    if !ttl.is_empty() {
        let d = parse_ttl(ttl).map_err(bad)?;
        let secs = i64::try_from(d.as_secs()).unwrap_or(i64::MAX);
        return Ok(now.saturating_add(secs));
    }
    Ok(at)
}

/// What [`issue`] makes: the token, shown this once, and its record.
#[derive(Debug, Clone)]
pub struct Issued {
    /// The token itself.
    pub secret: Secret,
    /// What the config keeps of it.
    pub record: LinkToken,
}

/// Makes a token and adds it to `tokens`, a listener's list.
///
/// # Errors
///
/// A request that is wrong, a listener with as many tokens as it may have, no random bytes.
pub fn issue(tokens: &mut Vec<LinkToken>, req: &IssueReq, now: i64) -> Result<Issued, TokenError> {
    let name = req.name.trim();
    let note = req.note.trim();
    let n = name.chars().count();
    if n == 0 || n > 64 || name.chars().any(char::is_control) {
        return Err(bad(
            "a token's name is 1 to 64 characters, without control characters",
        ));
    }
    if note.len() > 1024 {
        return Err(bad("a token's note is up to 1024 bytes"));
    }
    let exp = expiry(req.expires_at, &req.ttl, now)?;
    if tokens.len() >= MAX_LINK_TOKENS {
        return Err(bad(format!(
            "the listener has {} tokens, as many as it may: revoke one first",
            tokens.len()
        )));
    }
    let secret = Secret::new().map_err(|e| bad(format!("no random bytes: {e}")))?;
    let record = LinkToken {
        id: secret.id().to_owned(),
        name: name.to_owned(),
        hash: secret.hash(),
        created_at: now,
        expires_at: exp,
        note: note.to_owned(),
        members: Vec::new(),
    };
    tokens.push(record.clone());
    Ok(Issued { secret, record })
}

/// Changes token `id`'s expiry in `tokens`; returns the expiry set.
///
/// # Errors
///
/// A request that is wrong; no such token.
pub fn set_expiry(
    tokens: &mut [LinkToken],
    id: &str,
    req: &ExpiryReq,
    now: i64,
) -> Result<i64, TokenError> {
    let exp = expiry(req.expires_at, &req.ttl, now)?;
    let t = tokens
        .iter_mut()
        .find(|t| t.id == id)
        .ok_or(TokenError::NotFound)?;
    t.expires_at = exp;
    Ok(exp)
}

/// Removes token `id` from `tokens`: applied, it closes the token's links at once and frees its
/// members.
///
/// # Errors
///
/// No such token.
pub fn revoke(tokens: &mut Vec<LinkToken>, id: &str) -> Result<(), TokenError> {
    let i = tokens
        .iter()
        .position(|t| t.id == id)
        .ok_or(TokenError::NotFound)?;
    tokens.remove(i);
    Ok(())
}

/// Adds `member` to token `id`'s members in `tokens`, as a registry's bind asks: unchanged when
/// the token is gone, has it already, or has as many as it may. Whether it changed.
pub fn bind(tokens: &mut [LinkToken], id: &str, member: &str) -> bool {
    match tokens.iter_mut().find(|t| t.id == id) {
        Some(t)
            if !t.members.iter().any(|m| m == member)
                && t.members.len() < MAX_LINK_TOKEN_MEMBERS =>
        {
            t.members.push(member.to_owned());
            true
        }
        _ => false,
    }
}

fn int_of(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n.parse().unwrap_or(0),
        _ => 0,
    }
}

fn text_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// A member's tokens, as its config has them (`tokens` of the member's object).
#[must_use]
pub fn tokens_of(member: &Value) -> Vec<LinkToken> {
    let Some(Value::Array(items)) = member.get("tokens") else {
        return Vec::new();
    };
    items
        .iter()
        .map(|t| LinkToken {
            id: text_of(t.get("id")),
            name: text_of(t.get("name")),
            hash: text_of(t.get("hash")),
            created_at: int_of(t.get("created_at")),
            expires_at: int_of(t.get("expires_at")),
            note: text_of(t.get("note")),
            members: match t.get("members") {
                Some(Value::Array(ms)) => ms.iter().map(|m| text_of(Some(m))).collect(),
                _ => Vec::new(),
            },
        })
        .collect()
}

/// `tokens` as the config writes them, fit's fields in fit's order, the empty ones left out.
#[must_use]
pub fn tokens_value(tokens: &[LinkToken]) -> Value {
    Value::Array(
        tokens
            .iter()
            .map(|t| {
                let mut ms = vec![
                    ("id".to_owned(), Value::String(t.id.clone())),
                    ("name".to_owned(), Value::String(t.name.clone())),
                    ("hash".to_owned(), Value::String(t.hash.clone())),
                ];
                for (k, v) in [("created_at", t.created_at), ("expires_at", t.expires_at)] {
                    if v != 0 {
                        ms.push((k.to_owned(), Value::Number(v.to_string())));
                    }
                }
                if !t.note.is_empty() {
                    ms.push(("note".to_owned(), Value::String(t.note.clone())));
                }
                if !t.members.is_empty() {
                    ms.push((
                        "members".to_owned(),
                        Value::Array(t.members.iter().cloned().map(Value::String).collect()),
                    ));
                }
                Value::Object(ms)
            })
            .collect(),
    )
}

/// A token's view from its record alone: what the config holds, with no use since akit started.
#[must_use]
pub fn record_view(t: &LinkToken, now: i64) -> View {
    View {
        id: t.id.clone(),
        name: t.name.clone(),
        note: t.note.clone(),
        created_at: t.created_at,
        expires_at: t.expires_at,
        expired: t.expired(now),
        members: {
            let mut m = t.members.clone();
            m.sort();
            m
        },
        ..View::default()
    }
}

/// Views as a JSON array.
#[must_use]
pub fn list_json(vs: &[View]) -> String {
    let mut s = String::from("[");
    for (i, v) in vs.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&v.to_json());
    }
    s.push(']');
    s
}

/// How an expiry reads in the `admin` logger.
#[must_use]
pub fn expires_text(at: i64) -> String {
    if at == 0 {
        "never expiring".to_owned()
    } else {
        format!("expiring at {}", rfc3339(at))
    }
}

/// `secs` since 1970 as RFC 3339 in UTC, as Go's `time.RFC3339`.
// The days of an i64 of seconds, and the era of those days times 400, are far inside an i64:
// nothing here overflows, whatever the config's `expires_at` says.
#[allow(clippy::arithmetic_side_effects)]
#[must_use]
pub fn rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days.saturating_add(719_468);
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Whether `id` is a token's ID: re-exported for the role crates.
#[must_use]
pub fn valid_id(id: &str) -> bool {
    valid_link_token_id(id)
}

#[cfg(test)]
mod tests;
