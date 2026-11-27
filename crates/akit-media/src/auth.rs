//! `media_auth`'s policies (I121), as fit's package `mediaauth`: who may publish and play which
//! streams, and the checks the streaming role's listeners and the media handlers make against
//! them.
//!
//! A check is synchronous and cheap, and never blocks an I/O thread (rule 1): a static token is
//! looked up by its SHA-256, a signed token's HMAC is checked and its claims read, a plaintext
//! password is compared in constant time. The one slow check, a bcrypt `password_hash`, runs on
//! the crate's bounded pool ([`crate::pool`]); a password found right is remembered as an
//! HMAC-SHA256 under the policy's own random key, which no one could check passwords against, so
//! that the next check of it is at once. [`Policy::allow`] says [`Refusal::Pending`] for it
//! rather than wait; [`Policy::check`] runs it on the pool.
//!
//! Tokens akit signs are fit's, byte for byte (decision 040): `fm1.<claims>.<mac>`, the claims a
//! JSON object of the subject, the expiry and the grant in unpadded base64url, the MAC an
//! HMAC-SHA256 of `fm1.` and the claims under a key derived from the policy's secret and name with
//! fit's label. A token fit's `media-token` signed is taken by akit given the same secret and
//! policy name, and one `akit media-token` signs by fit.
//!
//! Policies are found by name per engine ([`registry`]): a policy stays the same object across
//! reloads and changes in place, so that a handler or a listener holding one sees what the config
//! now says. The role `media_auth` moves them with every config applied.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use akio_crypto::bcrypt;
use akio_crypto::hash::Sha256;
use akio_crypto::mac::Hmac;
use akit_config::media::{MIN_MEDIA_SECRET, MediaAction as Action, MediaAuth, valid_media_pattern};

use crate::swap::Swap;

/// The prefix of every token akit signs, telling it from a static one: fit's.
pub const TOKEN_PREFIX: &str = "fm1.";
/// The longest token looked at; a longer one is bad.
pub const MAX_TOKEN_BYTES: usize = 4096;
/// The label a policy's signing key is derived with: fit's, so that tokens sign and check alike.
const SIGN_LABEL: &str = "fit media token\0";
/// The longest subject a signed token may carry.
const MAX_SUBJECT: usize = 128;

/// What a request may do with a stream: the stream patterns it may publish and play, `*`
/// matching any run of characters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grant {
    /// The streams it may publish.
    pub publish: Vec<String>,
    /// The streams it may play.
    pub play: Vec<String>,
}

impl Grant {
    /// Whether it lets `a` take place on `stream`.
    #[must_use]
    pub fn allows(&self, a: Action, stream: &str) -> bool {
        let list = match a {
            Action::Publish => &self.publish,
            Action::Play => &self.play,
        };
        list.iter().any(|p| matches(p, stream))
    }
}

/// Whether `stream` matches `pattern`, whose `*` matches any run of characters, '/' among them,
/// and whose other characters match themselves (fit's `Match`).
#[must_use]
pub fn matches(pattern: &str, stream: &str) -> bool {
    let (p, s) = (pattern.as_bytes(), stream.as_bytes());
    let (mut pi, mut si) = (0usize, 0usize);
    let (mut star, mut mark): (Option<usize>, usize) = (None, 0);
    while si < s.len() {
        match (p.get(pi), s.get(si)) {
            (Some(b'*'), _) => {
                star = Some(pi);
                mark = si;
                pi = pi.saturating_add(1);
            }
            (Some(a), Some(b)) if a == b => {
                pi = pi.saturating_add(1);
                si = si.saturating_add(1);
            }
            _ => match star {
                Some(st) => {
                    pi = st.saturating_add(1);
                    mark = mark.saturating_add(1);
                    si = mark;
                }
                None => return false,
            },
        }
    }
    while p.get(pi) == Some(&b'*') {
        pi = pi.saturating_add(1);
    }
    pi == p.len()
}

/// Why a check refused, which [`Refusal::status`] maps to an HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No credentials, for an action the policy does not make public: 401, with a challenge.
    NoCredentials,
    /// An unknown, expired or tampered token, or a wrong user or password: 401.
    BadCredentials,
    /// Credentials that may not take the action on the stream: 403.
    Forbidden,
    /// A password a bcrypt hash checks, not found right before: [`Policy::check`] checks it on
    /// the pool.
    Pending,
    /// A bcrypt check the pool had no room or time for: 503.
    Busy,
    /// A policy the running config does not have (any more, or yet): 503.
    Unavailable,
}

impl Refusal {
    /// The HTTP status for it: 401, 403 or 503.
    #[must_use]
    pub fn status(self) -> u16 {
        match self {
            Refusal::NoCredentials | Refusal::BadCredentials => 401,
            Refusal::Forbidden => 403,
            Refusal::Pending | Refusal::Busy | Refusal::Unavailable => 503,
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Refusal::NoCredentials => "credentials are needed",
            Refusal::BadCredentials => "the credentials are not valid",
            Refusal::Forbidden => "the credentials do not allow this",
            Refusal::Pending => "the password needs a bcrypt check",
            Refusal::Busy => "too many password checks are waiting",
            Refusal::Unavailable => "the media_auth policy is not loaded",
        })
    }
}

impl std::error::Error for Refusal {}

/// What a request says it is: a token, or a user and a password.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Credentials {
    /// A token: a static one, or one akit or fit signed.
    pub token: String,
    /// A user's name.
    pub user: String,
    /// The user's password.
    pub password: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

impl Credentials {
    /// Whether it holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.token.is_empty() && self.user.is_empty()
    }

    /// The credentials of a URL's query, for protocols that carry no header (RTMP, SRT, RTSP's
    /// URL): `token`, or `user` and `pass` (fit's `FromQuery`).
    #[must_use]
    pub fn from_query(query: &str) -> Credentials {
        let q = query.strip_prefix('?').unwrap_or(query);
        let (mut token, mut user, mut pass) = (None, None, None);
        for kv in q.split('&') {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            let slot = match unescape(k).as_deref() {
                Some("token") => &mut token,
                Some("user") => &mut user,
                Some("pass") => &mut pass,
                _ => continue,
            };
            // Go's url.Values.Get: the first value.
            if slot.is_none() {
                *slot = Some(unescape(v).unwrap_or_default());
            }
        }
        match (token, user) {
            (Some(t), _) if !t.is_empty() => Credentials {
                token: t,
                ..Credentials::default()
            },
            (_, Some(u)) if !u.is_empty() => Credentials {
                user: u,
                password: pass.unwrap_or_default(),
                ..Credentials::default()
            },
            _ => Credentials::default(),
        }
    }
}

/// `s` with `%XX` escapes and `+` decoded, as a query's; `None` for a malformed escape or what is
/// not UTF-8.
pub(crate) fn unescape(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0usize;
    while let Some(&c) = b.get(i) {
        match c {
            b'%' => {
                let hex = |j: usize| b.get(j).and_then(|h| (*h as char).to_digit(16));
                let (hi, lo) = (hex(i.saturating_add(1))?, hex(i.saturating_add(2))?);
                out.push(u8::try_from(hi.saturating_mul(16).saturating_add(lo)).ok()?);
                i = i.saturating_add(3);
            }
            b'+' => {
                out.push(b' ');
                i = i.saturating_add(1);
            }
            _ => {
                out.push(c);
                i = i.saturating_add(1);
            }
        }
    }
    String::from_utf8(out).ok()
}

/// A password found right, remembered as an HMAC under the policy's key.
#[derive(Debug, Default)]
struct Remembered(Mutex<Option<[u8; 32]>>);

impl Remembered {
    fn matches(&self, mac: &[u8; 32]) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|good| akio_crypto::ct_eq(good, mac))
    }

    fn store(&self, mac: [u8; 32]) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(mac);
    }
}

#[derive(Debug)]
struct Token {
    subject: String,
    grant: Grant,
}

struct User {
    name: String,
    /// The plaintext password, for RTSP's Digest; empty with a hash.
    password: String,
    /// The SHA-256 of `password`.
    sum: [u8; 32],
    hash: Option<bcrypt::Hash>,
    /// The hash as written, to keep what is remembered across a reload that keeps it.
    hash_text: String,
    grant: Grant,
    good: Arc<Remembered>,
}

/// A policy as compiled from one config, swapped whole.
struct State {
    public: [bool; 2],
    tokens: HashMap<[u8; 32], Token>,
    users: HashMap<String, User>,
    /// Checked against for an unknown user, when a user has a hash, so that a refusal takes as
    /// long whether the user exists or not.
    decoy: Option<bcrypt::Hash>,
    sign_key: Vec<u8>,
    ttl: Duration,
}

fn action_index(a: Action) -> usize {
    match a {
        Action::Publish => 0,
        Action::Play => 1,
    }
}

/// A named `media_auth` policy. Its methods may be called from any thread.
pub struct Policy {
    name: String,
    /// Signs tokens when the config has no secret; it lasts as long as the process.
    key: [u8; 32],
    /// Keys what is remembered of bcrypt passwords found right.
    mac: [u8; 32],
    st: Swap<State>,
    /// Whether the role has set the state from a config applied, after which a handler's
    /// provisioning leaves it to the role.
    applied: Mutex<bool>,
    /// The `secret_file` the state was read from, with the SHA-256 of what was read.
    file: Mutex<Option<(String, [u8; 32])>>,
}

impl std::fmt::Debug for Policy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Policy")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

fn random32() -> [u8; 32] {
    let mut b = [0u8; 32];
    // Without the system's randomness, the keys are of the clock: the process keeps going, its
    // tokens signed without a secret then guessable, which a failing RNG makes of everything.
    if akio_crypto::rand::fill_random(&mut b).is_err() {
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        b = Sha256::digest(&t.to_be_bytes());
    }
    b
}

/// The key the tokens of policy `name` are signed and checked with, derived from its secret
/// (fit's `signKeyFor`).
fn sign_key_for(name: &str, secret: &[u8]) -> Vec<u8> {
    let mut m = Hmac::<Sha256>::new(secret);
    m.update(SIGN_LABEL.as_bytes());
    m.update(name.as_bytes());
    m.finalize().to_vec()
}

/// Reads a secret from `path`, its line breaks at the end taken off, with the SHA-256 of what was
/// read. Reads the disk: the control plane's.
fn secret_file(path: &str) -> Result<(String, [u8; 32]), String> {
    let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let sum = Sha256::digest(&data);
    let s = String::from_utf8(data).map_err(|_| format!("{path} is not UTF-8"))?;
    let secret = s.trim_end_matches(['\r', '\n']).to_owned();
    if secret.len() < MIN_MEDIA_SECRET {
        return Err(format!(
            "{path} holds {} bytes: use at least {MIN_MEDIA_SECRET}",
            secret.len()
        ));
    }
    Ok((secret, sum))
}

/// The time now in seconds since 1970, for tokens' expiry.
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

impl Policy {
    fn new(name: &str) -> Policy {
        Policy {
            name: name.to_owned(),
            key: random32(),
            mac: random32(),
            st: Swap::default(),
            applied: Mutex::new(false),
            file: Mutex::new(None),
        }
    }

    /// A policy made from `cfg` alone, outside any engine's registry: for `akit media-token` and
    /// tests. Its key without a secret is its own. Reads `secret_file`.
    ///
    /// # Errors
    ///
    /// What does not compile: a `secret_file` that cannot be read or is too short.
    pub fn from_config(cfg: &MediaAuth) -> Result<Arc<Policy>, String> {
        let p = Arc::new(Policy::new(&cfg.name));
        let st = p.compile(cfg, None)?;
        p.st.set(Some(Arc::new(st)));
        Ok(p)
    }

    /// Compiles `cfg` for the policy, keeping what `old` remembers of passwords whose hashes are
    /// the same. Reads `secret_file`.
    fn compile(&self, cfg: &MediaAuth, old: Option<&State>) -> Result<State, String> {
        let mut st = State {
            public: [false; 2],
            tokens: HashMap::new(),
            users: HashMap::new(),
            decoy: None,
            sign_key: Vec::new(),
            ttl: cfg.ttl(),
        };
        for a in &cfg.public {
            if let Some(p) = st.public.get_mut(action_index(*a)) {
                *p = true;
            }
        }
        let check = |list: &[String]| -> Result<(), String> {
            match list.iter().find(|p| !valid_media_pattern(p)) {
                Some(p) => Err(format!("{p:?} is not a stream pattern")),
                None => Ok(()),
            }
        };
        for (i, t) in cfg.tokens.iter().enumerate() {
            let sum = if !t.token.is_empty() {
                Sha256::digest(t.token.as_bytes())
            } else if let Some(h) = t.hash_bytes() {
                h
            } else {
                return Err(format!("tokens[{i}]: set token or token_sha256"));
            };
            check(&t.publish)
                .and(check(&t.play))
                .map_err(|e| format!("tokens[{i}]: {e}"))?;
            let subject = if t.name.is_empty() {
                "token".to_owned()
            } else {
                format!("token:{}", t.name)
            };
            st.tokens.insert(
                sum,
                Token {
                    subject,
                    grant: Grant {
                        publish: t.publish.clone(),
                        play: t.play.clone(),
                    },
                },
            );
        }
        for (i, u) in cfg.users.iter().enumerate() {
            check(&u.publish)
                .and(check(&u.play))
                .map_err(|e| format!("users[{i}]: {e}"))?;
            let mut x = User {
                name: u.name.clone(),
                password: u.password.clone(),
                sum: [0; 32],
                hash: None,
                hash_text: String::new(),
                grant: Grant {
                    publish: u.publish.clone(),
                    play: u.play.clone(),
                },
                good: Arc::new(Remembered::default()),
            };
            if !u.password.is_empty() {
                x.sum = Sha256::digest(u.password.as_bytes());
            } else if !u.password_hash.is_empty() {
                let h = bcrypt::Hash::parse(&u.password_hash)
                    .map_err(|e| format!("users[{i}]: password_hash is not a bcrypt hash: {e}"))?;
                x.hash = Some(h);
                x.hash_text.clone_from(&u.password_hash);
                if let Some(o) = old.and_then(|o| o.users.get(&u.name)) {
                    if o.hash_text == u.password_hash {
                        x.good = Arc::clone(&o.good);
                    }
                }
            } else {
                return Err(format!("users[{i}]: set password or password_hash"));
            }
            if st.users.insert(u.name.clone(), x).is_some() {
                return Err(format!("users[{i}]: {:?} is listed twice", u.name));
            }
        }
        if let Some(cost) = st
            .users
            .values()
            .filter_map(|u| u.hash.as_ref().map(bcrypt::Hash::cost))
            .next()
        {
            let pw = random32();
            st.decoy = bcrypt::hash(&pw, cost)
                .ok()
                .and_then(|h| bcrypt::Hash::parse(&h).ok());
        }
        let mut file = None;
        let secret = if !cfg.secret_file.is_empty() {
            if !cfg.secret.is_empty() {
                return Err("secret_file: set secret or secret_file, one of the two".into());
            }
            let (s, sum) =
                secret_file(&cfg.secret_file).map_err(|e| format!("secret_file: {e}"))?;
            file = Some((cfg.secret_file.clone(), sum));
            s
        } else {
            cfg.secret.clone()
        };
        st.sign_key = if secret.is_empty() {
            self.key.to_vec()
        } else {
            sign_key_for(&cfg.name, secret.as_bytes())
        };
        *self.file.lock().unwrap_or_else(PoisonError::into_inner) = file;
        Ok(st)
    }

    /// The policy's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The `secret_file` the policy read when it was last compiled, with the SHA-256 of what it
    /// read, for `-watch-certs` to apply the config again when it changes.
    #[must_use]
    pub fn file(&self) -> Option<(String, [u8; 32])> {
        self.file
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether anyone may take `a` without credentials.
    #[must_use]
    pub fn is_public(&self, a: Action) -> bool {
        self.st.with(|st| {
            st.is_some_and(|st| st.public.get(action_index(a)).copied().unwrap_or(false))
        })
    }

    /// Whether the running config has the policy.
    #[must_use]
    pub fn loaded(&self) -> bool {
        self.st.with(|st| st.is_some())
    }

    /// How long the tokens the policy issues are good for when their issuer names no lifetime.
    #[must_use]
    pub fn ttl(&self) -> Duration {
        self.st
            .with(|st| st.map_or(akit_config::media::DEFAULT_MEDIA_TOKEN_TTL, |s| s.ttl))
    }

    /// Whether the policy has users who could log in.
    #[must_use]
    pub fn has_users(&self) -> bool {
        self.st.with(|st| st.is_some_and(|s| !s.users.is_empty()))
    }

    /// The plaintext password of `user`, for RTSP's Digest; `None` for a user with a hash or
    /// none.
    #[must_use]
    pub fn password(&self, user: &str) -> Option<String> {
        self.st.with(|st| {
            st.and_then(|s| s.users.get(user))
                .filter(|u| u.hash.is_none())
                .map(|u| u.password.clone())
        })
    }

    /// Checks whether `c` may take `a` on `stream`, without waiting: the subject the request is
    /// authenticated as (a user's name, `token` or `token:<name>` for a static token, a signed
    /// token's subject; empty for an action the policy makes public), or why not. A token is
    /// checked before a user and password. A password a bcrypt hash must check, not found right
    /// before, is [`Refusal::Pending`]: [`Policy::check`] checks it off the caller's thread.
    ///
    /// # Errors
    ///
    /// The refusal.
    pub fn allow(&self, a: Action, stream: &str, c: &Credentials) -> Result<String, Refusal> {
        self.st.with(|st| {
            let Some(st) = st else {
                return Err(Refusal::Unavailable);
            };
            if st.public.get(action_index(a)).copied().unwrap_or(false) {
                return Ok(String::new());
            }
            let (subject, grant) = if !c.token.is_empty() {
                token(st, &c.token)?
            } else if !c.user.is_empty() {
                let u = self.password_check(st, &c.user, &c.password)?;
                (u.name.clone(), u.grant.clone())
            } else {
                return Err(Refusal::NoCredentials);
            };
            if grant.allows(a, stream) {
                Ok(subject)
            } else {
                Err(Refusal::Forbidden)
            }
        })
    }

    /// A user's password checked at once where it can be: [`Refusal::Pending`] for a bcrypt hash
    /// that has not found it right before.
    fn password_check<'s>(
        &self,
        st: &'s State,
        name: &str,
        password: &str,
    ) -> Result<&'s User, Refusal> {
        let Some(u) = st.users.get(name) else {
            return Err(if st.decoy.is_some() {
                // The same work as a known user's: checked on the pool, and refused there.
                Refusal::Pending
            } else {
                Refusal::BadCredentials
            });
        };
        if u.hash.is_none() {
            let sum = Sha256::digest(password.as_bytes());
            return if akio_crypto::ct_eq(&sum, &u.sum) {
                Ok(u)
            } else {
                Err(Refusal::BadCredentials)
            };
        }
        if u.good.matches(&self.remembered(password)) {
            Ok(u)
        } else {
            Err(Refusal::Pending)
        }
    }

    /// What a user's record keeps of a password found right.
    fn remembered(&self, password: &str) -> [u8; 32] {
        let mut m = Hmac::<Sha256>::new(&self.mac);
        m.update(password.as_bytes());
        m.finalize()
    }

    /// [`Policy::allow`], with a bcrypt check run on the crate's pool: `done` is called once,
    /// before `check` returns when no bcrypt check is needed, or later on the pool's thread. A
    /// check the pool has no room or time for is [`Refusal::Busy`].
    pub fn check(
        self: &Arc<Self>,
        a: Action,
        stream: &str,
        c: &Credentials,
        done: impl FnOnce(Result<String, Refusal>) + Send + 'static,
    ) {
        match self.allow(a, stream, c) {
            Err(Refusal::Pending) => {}
            r => return done(r),
        }
        let p = Arc::clone(self);
        let (stream, c) = (stream.to_owned(), c.clone());
        crate::pool::submit(Box::new(move |late| {
            if late {
                return done(Err(Refusal::Busy));
            }
            done(p.slow(a, &stream, &c));
        }));
    }

    /// Checks `c`'s password against its user's bcrypt hash (or the decoy), remembering it when
    /// right. Runs on the pool.
    fn slow(&self, a: Action, stream: &str, c: &Credentials) -> Result<String, Refusal> {
        let hashed = self.st.with(|st| {
            st.and_then(|st| st.users.get(&c.user))
                .is_none_or(|u| u.hash.is_some())
        });
        if !hashed {
            // A user whose password is no longer hashed after a reload: checked as it is now.
            return self.allow(a, stream, c);
        }
        let (name, grant) = self.slow_user(&c.user, &c.password)?;
        if grant.allows(a, stream) {
            Ok(name)
        } else {
            Err(Refusal::Forbidden)
        }
    }

    /// Checks a user's password for the media pages' login (fit's `Login`): `done` is called once
    /// with the user's name and grant, before `login` returns when no bcrypt check is needed, or
    /// later on the crate's pool. A check the pool has no room or time for is [`Refusal::Busy`].
    pub fn login(
        self: &Arc<Self>,
        user: &str,
        password: &str,
        done: impl FnOnce(Result<(String, Grant), Refusal>) + Send + 'static,
    ) {
        match self.login_now(user, password) {
            Err(Refusal::Pending) => {}
            r => return done(r),
        }
        let p = Arc::clone(self);
        let (user, password) = (user.to_owned(), password.to_owned());
        crate::pool::submit(Box::new(move |late| {
            if late {
                return done(Err(Refusal::Busy));
            }
            done(p.slow_user(&user, &password));
        }));
    }

    /// [`Policy::login`] without waiting: [`Refusal::Pending`] for a password a bcrypt hash must
    /// check that has not been found right before.
    ///
    /// # Errors
    ///
    /// The refusal.
    pub fn login_now(&self, user: &str, password: &str) -> Result<(String, Grant), Refusal> {
        self.st.with(|st| {
            let Some(st) = st else {
                return Err(Refusal::Unavailable);
            };
            self.password_check(st, user, password)
                .map(|u| (u.name.clone(), u.grant.clone()))
        })
    }

    /// A user's password checked against its bcrypt hash (or the decoy), remembered when right:
    /// its name and grant. Runs on the pool.
    fn slow_user(&self, user: &str, password: &str) -> Result<(String, Grant), Refusal> {
        let (hash, good, grant, name) = {
            let r = self.st.with(|st| {
                let st = st.ok_or(Refusal::Unavailable)?;
                match st.users.get(user) {
                    Some(u) => Ok((
                        u.hash.clone(),
                        Some(Arc::clone(&u.good)),
                        Some(u.grant.clone()),
                        u.name.clone(),
                    )),
                    None => Ok((st.decoy.clone(), None, None, String::new())),
                }
            });
            r?
        };
        let Some(hash) = hash else {
            // A user whose password is no longer hashed after a reload: checked as it is now.
            return self.st.with(|st| {
                let st = st.ok_or(Refusal::Unavailable)?;
                self.password_check(st, user, password)
                    .map(|u| (u.name.clone(), u.grant.clone()))
            });
        };
        if !hash.verify(password.as_bytes()) {
            return Err(Refusal::BadCredentials);
        }
        let (Some(good), Some(grant)) = (good, grant) else {
            return Err(Refusal::BadCredentials);
        };
        good.store(self.remembered(password));
        Ok((name, grant))
    }

    /// Signs a token good for `subject` to take `g`'s actions on `g`'s streams until now and
    /// `ttl` (the policy's `token_ttl` when zero), and its expiry in seconds since 1970. The
    /// subject is 1 to 128 bytes with no control character; the patterns are the config's.
    ///
    /// # Errors
    ///
    /// A bad subject or pattern, a grant too long for a token, a policy not loaded.
    pub fn issue(&self, subject: &str, g: &Grant, ttl: Duration) -> Result<(String, i64), String> {
        let r = self.st.with(|st| {
            st.map(|st| {
                (
                    st.sign_key.clone(),
                    if ttl.is_zero() { st.ttl } else { ttl },
                )
            })
        });
        let Some((key, ttl)) = r else {
            return Err(Refusal::Unavailable.to_string());
        };
        issue(&key, subject, g, ttl, now())
    }
}

/// Checks a token: one akit or fit signed, or a static one.
fn token(st: &State, tok: &str) -> Result<(String, Grant), Refusal> {
    if tok.len() > MAX_TOKEN_BYTES {
        return Err(Refusal::BadCredentials);
    }
    if tok.starts_with(TOKEN_PREFIX) {
        return verify(&st.sign_key, tok, now());
    }
    // Looked up by its SHA-256: how long the lookup takes says nothing of the token.
    st.tokens
        .get(&Sha256::digest(tok.as_bytes()))
        .map(|t| (t.subject.clone(), t.grant.clone()))
        .ok_or(Refusal::BadCredentials)
}

fn b64(b: &[u8]) -> String {
    akio_crypto::base64::URL_SAFE_NO_PAD.encode(b)
}

fn sign(key: &[u8], payload: &str) -> String {
    let mut m = Hmac::<Sha256>::new(key);
    m.update(TOKEN_PREFIX.as_bytes());
    m.update(payload.as_bytes());
    b64(&m.finalize())
}

/// Signs a token with `key`, at `now` in seconds since 1970.
pub(crate) fn issue(
    key: &[u8],
    subject: &str,
    g: &Grant,
    ttl: Duration,
    now: i64,
) -> Result<(String, i64), String> {
    if subject.is_empty()
        || subject.len() > MAX_SUBJECT
        || subject.chars().any(|c| c < '\u{20}' || c == '\u{7f}')
    {
        return Err(format!(
            "{subject:?} is not a subject: 1 to 128 bytes with no control characters"
        ));
    }
    if let Some(p) = g
        .publish
        .iter()
        .chain(&g.play)
        .find(|p| !valid_media_pattern(p))
    {
        return Err(format!("{p:?} is not a stream pattern"));
    }
    let expiry = now.saturating_add(i64::try_from(ttl.as_secs()).unwrap_or(i64::MAX));
    // fit's claims, as Go's encoding/json writes them: sub, exp, then the grant's lists when not
    // empty.
    let mut body = String::from("{\"sub\":");
    akit_config::value::write_string(&mut body, subject);
    body.push_str(",\"exp\":");
    body.push_str(&expiry.to_string());
    for (k, list) in [("pub", &g.publish), ("play", &g.play)] {
        if list.is_empty() {
            continue;
        }
        body.push_str(",\"");
        body.push_str(k);
        body.push_str("\":[");
        for (i, p) in list.iter().enumerate() {
            if i > 0 {
                body.push(',');
            }
            akit_config::value::write_string(&mut body, p);
        }
        body.push(']');
    }
    body.push('}');
    let payload = b64(body.as_bytes());
    // The prefix, the payload, a dot and 43 characters of MAC.
    if TOKEN_PREFIX
        .len()
        .saturating_add(payload.len())
        .saturating_add(44)
        > MAX_TOKEN_BYTES
    {
        return Err("the grant is too long for a token".into());
    }
    let mac = sign(key, &payload);
    Ok((format!("{TOKEN_PREFIX}{payload}.{mac}"), expiry))
}

/// Checks a signed token's MAC and expiry at `now`, and returns its subject and grant.
pub(crate) fn verify(key: &[u8], tok: &str, now: i64) -> Result<(String, Grant), Refusal> {
    use akit_config::value::Value;
    let bad = Refusal::BadCredentials;
    let rest = tok.strip_prefix(TOKEN_PREFIX).ok_or(bad)?;
    let (payload, mac) = rest.split_once('.').ok_or(bad)?;
    if !akio_crypto::ct_eq(mac.as_bytes(), sign(key, payload).as_bytes()) {
        return Err(bad);
    }
    let body = akio_crypto::base64::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| bad)?;
    let body = String::from_utf8(body).map_err(|_| bad)?;
    let v = akit_config::value::parse(&body).map_err(|_| bad)?;
    let subject = match v.get("sub") {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        _ => return Err(bad),
    };
    let expiry = match v.get("exp") {
        Some(Value::Number(n)) => n.parse::<i64>().map_err(|_| bad)?,
        _ => 0,
    };
    if now >= expiry {
        return Err(bad);
    }
    let list = |k: &str| -> Result<Vec<String>, Refusal> {
        match v.get(k) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(a)) => a
                .iter()
                .map(|x| match x {
                    Value::String(s) => Ok(s.clone()),
                    _ => Err(bad),
                })
                .collect(),
            Some(_) => Err(bad),
        }
    };
    Ok((
        subject,
        Grant {
            publish: list("pub")?,
            play: list("play")?,
        },
    ))
}

/// The policies by engine and name, as fit's `registry`: what a handler and a role find a
/// policy by, the same object across reloads.
pub mod registry {
    use super::{Arc, HashMap, MediaAuth, Mutex, PoisonError, Policy, State};

    type Map = HashMap<usize, HashMap<String, Arc<Policy>>>;

    fn map() -> std::sync::MutexGuard<'static, Map> {
        static REG: std::sync::LazyLock<Mutex<Map>> =
            std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
        REG.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The key of an engine in the registry: its address.
    #[must_use]
    pub fn key(engine: &Arc<akio::Engine>) -> usize {
        Arc::as_ptr(engine).addr()
    }

    /// The policy named `name` of the engine `key`, or `None` when the running config has none
    /// of that name. The policy returned changes in place with each config applied.
    #[must_use]
    pub fn lookup(key: usize, name: &str) -> Option<Arc<Policy>> {
        map()
            .get(&key)
            .and_then(|m| m.get(name))
            .filter(|p| p.loaded())
            .cloned()
    }

    /// The policy a module names, made from `cfg`, the policy of the config being compiled, when
    /// the engine has none of that name yet or the role has not applied it, so that the module
    /// serves before the role applies the config; one the role has applied is left to it.
    ///
    /// # Errors
    ///
    /// What does not compile.
    pub fn provision(key: usize, cfg: &MediaAuth) -> Result<Arc<Policy>, String> {
        let mut m = map();
        let p = Arc::clone(
            m.entry(key)
                .or_default()
                .entry(cfg.name.clone())
                .or_insert_with(|| Arc::new(Policy::new(&cfg.name))),
        );
        drop(m);
        let applied = *p.applied.lock().unwrap_or_else(PoisonError::into_inner);
        if !applied || !p.loaded() {
            let old = p.st.get();
            let st = p.compile(cfg, old.as_deref())?;
            p.st.set(Some(Arc::new(st)));
        }
        Ok(p)
    }

    /// Moves the engine's policies to `list`: each changed in place, or made, and those `list`
    /// leaves out removed, so that whoever still holds one is refused as unavailable. A policy
    /// that does not compile changes nothing. Reads the `secret_file`s.
    ///
    /// # Errors
    ///
    /// Each policy that does not compile, by name.
    pub fn apply(key: usize, list: &[MediaAuth]) -> Result<(), String> {
        let mut m = map();
        let cur = m.entry(key).or_default();
        let mut next: Vec<(Arc<Policy>, State)> = Vec::new();
        let mut errs = Vec::new();
        for cfg in list {
            if next.iter().any(|(p, _)| p.name == cfg.name) {
                errs.push(format!("{:?} is listed twice", cfg.name));
                continue;
            }
            let p = cur
                .get(&cfg.name)
                .cloned()
                .unwrap_or_else(|| Arc::new(Policy::new(&cfg.name)));
            let old = p.st.get();
            match p.compile(cfg, old.as_deref()) {
                Ok(st) => next.push((p, st)),
                Err(e) => errs.push(format!("{}: {e}", cfg.name)),
            }
        }
        if !errs.is_empty() {
            return Err(errs.join("; "));
        }
        for (name, p) in cur.iter() {
            if !next.iter().any(|(n, _)| n.name == *name) {
                p.st.set(None);
            }
        }
        let mut pols = HashMap::new();
        for (p, st) in next {
            p.st.set(Some(Arc::new(st)));
            *p.applied.lock().unwrap_or_else(PoisonError::into_inner) = true;
            pols.insert(p.name.clone(), p);
        }
        if pols.is_empty() {
            m.remove(&key);
        } else {
            m.insert(key, pols);
        }
        Ok(())
    }

    /// Removes every policy of the engine `key`.
    pub fn remove(key: usize) {
        if let Some(pols) = map().remove(&key) {
            for p in pols.values() {
                p.st.set(None);
            }
        }
    }

    /// The `secret_file`s the engine's policies read, with their SHA-256.
    #[must_use]
    pub fn files(key: usize) -> Vec<(String, [u8; 32])> {
        map()
            .get(&key)
            .map(|m| m.values().filter_map(|p| p.file()).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests;
