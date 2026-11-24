//! Certificates obtained with ACME (RFC 8555), as fit's `tlsmgr.ACME` (fit's decision 007): an
//! ACME client of akit's own on akio-crypto (JWS, ECDSA), akio-x509 (the CSR) and akio-http's
//! client, run off the I/O threads.
//!
//! - **Accounts**: one per CA, its P-256 key kept in the storage and registered, agreeing to the
//!   CA's terms, the first time it is needed.
//! - **Orders**: a certificate per name, its own P-256 key, through the challenges
//!   `acme.challenges` lists, in its order (HTTP-01 then TLS-ALPN-01 by default, as fit's), each
//!   attempt after a failure starting with the next: HTTP-01, which every listener in the clear
//!   answers from memory ([`Acme::http_challenge`]), and TLS-ALPN-01 (RFC 8737, I32), which the
//!   TLS listener that lists the name answers, to a handshake offering `acme-tls/1` alone, with
//!   the challenge's certificate ([`Acme::alpn_challenge`]).
//! - **The storage** ([`Storage`]): a certificate there is used as it is until it is due for
//!   renewal, so that a restart, or another instance sharing the storage, does not obtain it
//!   again; instances take a lock in it before they obtain one, and look again once they hold it.
//! - **Renewal**: once a third of a certificate's lifetime is left, up to a fiftieth of it
//!   earlier by its serial number, as fit's; a failure is tried again after a minute, then twice
//!   as long each time, up to an hour. A certificate whose OCSP responder says it is revoked is
//!   obtained again at once, the stored one not used.
//! - **ARI** (I32, RFC 9773, as fit's): after each run that leaves a name with a certificate,
//!   the CA's suggested renewal window is asked for, and again when its `Retry-After` says (a
//!   minute to a day, six hours when it says nothing); the certificate is due at a time picked at
//!   random in the window, at once once it has begun, and by its lifetime when the CA has no ARI
//!   or does not answer.
//! - **OCSP** (I32): the response of each certificate obtained is stapled ([`Ocsp`], which the
//!   manager owns, and which staples the listeners' certificates from files too).
//! - **Threads**: `acme.max_concurrent` workers of the control plane, started when the first name
//!   is managed, each order bounded by `acme.timeout` and each request by
//!   `acme.request_timeout`. A handshake asks for a certificate ([`Acme::certificate`]) and a
//!   request for an HTTP-01 answer, both read from memory, without a lock (rules 1 and 13).

mod ari;
mod jws;
mod protocol;

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use akio::tls::{Certificate, CertifiedKey, PrivateKey, RootCerts};
use akio_crypto::hash::Sha256;
use akio_x509::{CertificateParams, CsrParams, Extension, GeneralName};
use akit_config::Challenge;

use crate::ocsp::{Ocsp, Staple};
use crate::published::Published;
use crate::storage::{FileStorage, Storage, StorageError};

use ari::AriState;
use jws::AccountKey;
use protocol::{Ca, string, strings};

/// Where a CA asks for an HTTP-01 challenge's answer: this path, then the challenge's token.
pub const ACME_CHALLENGE_PREFIX: &str = "/.well-known/acme-challenge/";
/// The protocol a CA offers alone through ALPN when it checks a TLS-ALPN-01 challenge (RFC 8737).
pub const ACME_TLS_ALPN: &str = "acme-tls/1";
/// id-pe-acmeIdentifier, 1.3.6.1.5.5.7.1.31: the extension of a TLS-ALPN-01 challenge's
/// certificate (RFC 8737 section 3).
const ACME_IDENTIFIER: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x01, 0x1f];

/// How long after the first failure in a row a name is tried again, by default; each failure
/// after doubles it, up to [`DEFAULT_RETRY_MAX`] (fit's).
pub const DEFAULT_RETRY_BASE: Duration = Duration::from_secs(60);
/// The longest wait between two tries of a name, by default.
pub const DEFAULT_RETRY_MAX: Duration = Duration::from_secs(3600);
/// The most names one manager keeps certificates for (rule 11): far more than a config lists.
pub const MAX_NAMES: usize = 10_000;
/// The longest a worker sleeps with nothing due, before it looks again.
const IDLE_WAIT: Duration = Duration::from_secs(3600);

/// What a manager reports: a line on what it did or why it failed.
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// A name a manager keeps a certificate for, and when it next works on it.
#[derive(Debug)]
struct Managed {
    due: Instant,
    failures: u32,
    running: bool,
    last_error: Option<String>,
    /// The serial number of a certificate its OCSP responder says is revoked: not taken from
    /// the storage again.
    revoked: Option<Vec<u8>>,
    /// What ARI says of its certificate.
    ari: Option<AriState>,
}

/// A certificate obtained for a name, with its OCSP response kept.
#[derive(Clone)]
struct Served {
    key: CertifiedKey,
    staple: Option<Arc<Staple>>,
}

impl Served {
    fn certificate(&self) -> CertifiedKey {
        self.staple
            .as_ref()
            .map_or_else(|| self.key.clone(), |s| s.certificate())
    }
}

#[derive(Debug, Default)]
struct State {
    names: HashMap<String, Managed>,
    started: bool,
    closed: bool,
    retry_base: Option<Duration>,
    retry_max: Option<Duration>,
    /// The bounds of the wait before ARI is asked about a certificate again.
    ari_min: Option<Duration>,
    ari_max: Option<Duration>,
}

struct Inner {
    storage: Arc<dyn Storage>,
    directory: String,
    email: String,
    roots: Option<RootCerts>,
    /// The CA's part of the storage's keys.
    ca_key: String,
    timeout: Duration,
    request_timeout: Duration,
    workers: usize,
    /// The challenges answered, in the order tried.
    challenges: Vec<Challenge>,
    /// The certificate of each name that has one, the key authorization of each HTTP-01
    /// challenge under way, by its token, and the certificate of each TLS-ALPN-01 challenge
    /// under way, by its name: what the I/O threads read.
    certs: Published<HashMap<String, Served>>,
    tokens: Published<HashMap<String, String>>,
    alpn_certs: Published<HashMap<String, CertifiedKey>>,
    ocsp: Ocsp,
    state: Mutex<State>,
    wake: Condvar,
    stop: Arc<AtomicBool>,
    ca: Mutex<Option<Arc<Ca>>>,
    account: Mutex<Option<Arc<(AccountKey, String)>>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    log: Mutex<Log>,
}

/// Obtains certificates for the names it manages from an ACME CA, keeps them in a [`Storage`],
/// and renews them. Cheap to clone: the clones share it. [`close`](Self::close) stops it.
#[derive(Clone)]
pub struct Acme {
    inner: Arc<Inner>,
}

impl fmt::Debug for Acme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Acme")
            .field("directory", &self.inner.directory)
            .field("storage", &self.inner.storage)
            .finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A host name as ACME and a handshake compare it: in lower case, without a trailing dot.
fn normalize(name: &str) -> String {
    name.trim_end_matches('.').to_ascii_lowercase()
}

/// The directory a config with no `acme.storage` keeps what ACME obtains in:
/// `$XDG_DATA_HOME/akit`, or `~/.local/share/akit`.
fn default_root() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d).join("akit"));
    }
    let home = std::env::var_os("HOME").filter(|d| !d.is_empty())?;
    Some(
        PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("akit"),
    )
}

/// A directory's URL as one part of a storage key, as fit's `caKey`: its host and path, with
/// every byte but letters, digits, `.` and `-` made `-`.
fn ca_key(directory: &str) -> String {
    let rest = directory
        .split_once("://")
        .map_or(directory, |(_, rest)| rest);
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let key: String = rest
        .trim_matches('/')
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if key.is_empty() || key.starts_with('.') {
        format!("ca-{key}")
    } else {
        key
    }
}

/// The instant of Unix second `t`, as seen now.
fn instant_of(t: i64) -> Instant {
    let now = Instant::now();
    let d = t.saturating_sub(unix_now());
    let secs = Duration::from_secs(d.unsigned_abs());
    if d >= 0 {
        now.checked_add(secs).unwrap_or(now)
    } else {
        now.checked_sub(secs).unwrap_or(now)
    }
}

/// `d` from now, or now when that cannot be told.
fn after(d: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(d).unwrap_or(now)
}

/// Seconds since the Unix epoch, now.
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// When a certificate valid from `not_before` to `not_after` with serial number `serial` is due
/// for renewal, in seconds since the epoch: once a third of its lifetime is left, and up to a
/// fiftieth of its lifetime before that, by its serial number, so that certificates obtained
/// together are not renewed together and every instance sharing one agrees when (fit's
/// `defaultRenewAt`).
fn renew_at(not_before: i64, not_after: i64, serial: &[u8]) -> i64 {
    let life = not_after.saturating_sub(not_before).max(0);
    // FNV-1a, 32 bits, as fit's.
    let mut h: u32 = 0x811c_9dc5;
    for b in serial {
        h = (h ^ u32::from(*b)).wrapping_mul(0x0100_0193);
    }
    let per_mille = i64::from(h % 1000);
    let jitter = (life / 50).saturating_mul(per_mille) / 1000;
    not_after.saturating_sub(life / 3).saturating_sub(jitter)
}

/// A certificate as the storage keeps it: its chain and key in one PEM, checked to be for
/// `name`; with when it is due for renewal.
fn parse_stored(data: &[u8], name: &str) -> Result<Stored, String> {
    let chain = Certificate::from_pem(data).map_err(|e| e.to_string())?;
    let keys = akio_crypto::pem::decode_label(data, "PRIVATE KEY").map_err(|e| e.to_string())?;
    let key = keys.into_iter().next().ok_or("no private key")?;
    let leaf_der = chain.first().ok_or("no certificate")?.as_der().to_vec();
    let leaf = akio_x509::Certificate::from_der(leaf_der).map_err(|e| e.to_string())?;
    let for_name = leaf
        .subject_alt_names()
        .iter()
        .any(|n| matches!(n, GeneralName::Dns(d) if d.eq_ignore_ascii_case(name)));
    if !for_name {
        return Err(format!("the certificate is not for {name}"));
    }
    let due = renew_at(leaf.not_before(), leaf.not_after(), leaf.serial());
    let ck =
        CertifiedKey::new(chain, PrivateKey::from_pkcs8_der(key)).map_err(|e| e.to_string())?;
    Ok(Stored {
        key: ck,
        due,
        serial: leaf.serial().to_vec(),
    })
}

/// A certificate as [`parse_stored`] reads it.
struct Stored {
    key: CertifiedKey,
    /// When it is due for renewal, in seconds since the epoch.
    due: i64,
    serial: Vec<u8>,
}

/// The certificate that answers the TLS-ALPN-01 challenge of `name` whose key authorization is
/// `key_auth` (RFC 8737 section 3): self-signed, for `name` alone, with the critical
/// acmeIdentifier extension holding the authorization's SHA-256, on a key of its own.
pub(crate) fn alpn_cert(name: &str, key_auth: &str) -> Result<CertifiedKey, String> {
    let key = akio_crypto::p256::SecretKey::generate().map_err(|e| e.to_string())?;
    let key = akio_crypto::pkey::PrivateKey::P256(key);
    let mut p = CertificateParams::new([name]);
    // extnValue holds an OCTET STRING of the 32 bytes.
    let mut value = vec![0x04, 0x20];
    value.extend_from_slice(&Sha256::digest(key_auth.as_bytes()));
    p.custom_extensions.push(Extension {
        oid: ACME_IDENTIFIER.to_vec(),
        critical: true,
        value,
    });
    let cert = p.self_signed(&key).map_err(|e| e.to_string())?;
    CertifiedKey::new(
        vec![Certificate::from_der(cert.as_der().to_vec())],
        PrivateKey::from_pkcs8_der(key.to_pkcs8_der()),
    )
    .map_err(|e| e.to_string())
}

/// A challenge's answer under way, given back when it is dropped: the key authorization of an
/// HTTP-01 token, or the certificate of a TLS-ALPN-01 name.
enum Answering<'a> {
    Token(&'a Acme, String),
    Alpn(&'a Acme, String),
}

impl Drop for Answering<'_> {
    fn drop(&mut self) {
        match self {
            Answering::Token(a, token) => a.set_token(token, None),
            Answering::Alpn(a, name) => a.set_alpn(name, None),
        }
    }
}

impl Acme {
    /// A manager that obtains certificates as `config` says, kept in its storage directory. It
    /// manages no names, and runs no threads, until [`manage`](Self::manage).
    ///
    /// # Errors
    ///
    /// An `acme.ca_file` that cannot be read or holds no certificate.
    pub fn new(config: &akit_config::Acme) -> Result<Acme, crate::Error> {
        let storage: Arc<dyn Storage> = match (config.storage.is_empty(), default_root()) {
            (false, _) => Arc::new(FileStorage::new(&config.storage)),
            (true, Some(root)) => Arc::new(FileStorage::new(root)),
            // No home to keep certificates in: fails when one is first needed, not at start.
            (true, None) => Arc::new(NoStorage),
        };
        Acme::with_storage(config, storage)
    }

    /// The same, keeping what it obtains in `storage`.
    ///
    /// # Errors
    ///
    /// As [`new`](Self::new).
    pub fn with_storage(
        config: &akit_config::Acme,
        storage: Arc<dyn Storage>,
    ) -> Result<Acme, crate::Error> {
        let roots = if config.ca_file.is_empty() {
            None
        } else {
            let path = &config.ca_file;
            let pem = crate::read(path).map_err(|m| crate::Error::new("acme.ca_file", m))?;
            let mut roots = RootCerts::empty();
            match roots.add_pem(&pem) {
                Ok(0) => {
                    return Err(crate::Error::new(
                        "acme.ca_file",
                        format!("{path} holds no PEM certificate"),
                    ));
                }
                Ok(_) => {}
                Err(e) => return Err(crate::Error::new("acme.ca_file", format!("{path}: {e}"))),
            }
            Some(roots)
        };
        let log: Log = Arc::new(|line: &str| eprintln!("akit: acme: {line}"));
        Ok(Acme {
            inner: Arc::new(Inner {
                storage,
                directory: config.directory.clone(),
                email: config.email.clone(),
                roots,
                ca_key: ca_key(&config.directory),
                timeout: config.timeout,
                request_timeout: config.request_timeout,
                workers: config.max_concurrent.max(1),
                challenges: if config.challenges.is_empty() {
                    vec![Challenge::Http01, Challenge::TlsAlpn01]
                } else {
                    config.challenges.clone()
                },
                certs: Published::new(HashMap::new()),
                tokens: Published::new(HashMap::new()),
                alpn_certs: Published::new(HashMap::new()),
                ocsp: Ocsp::new(),
                state: Mutex::new(State::default()),
                wake: Condvar::new(),
                stop: Arc::new(AtomicBool::new(false)),
                ca: Mutex::new(None),
                account: Mutex::new(None),
                threads: Mutex::new(Vec::new()),
                log: Mutex::new(log),
            }),
        })
    }

    /// Has the manager, and its [`Ocsp`], report through `log` from now on (standard error by
    /// default).
    pub fn set_log(&self, log: Log) {
        self.inner.ocsp.set_log(Arc::clone(&log));
        *lock(&self.inner.log) = log;
    }

    /// Its OCSP stapler: of the certificates it obtains, and of the listeners' from files.
    #[must_use]
    pub fn ocsp(&self) -> &Ocsp {
        &self.inner.ocsp
    }

    /// Whether it answers TLS-ALPN-01 challenges (`acme.challenges` lists it): the listeners
    /// that list names offer [`ACME_TLS_ALPN`] then.
    #[must_use]
    pub fn answers_tls_alpn(&self) -> bool {
        self.inner.challenges.contains(&Challenge::TlsAlpn01)
    }

    /// The waits after failures: `base` after the first, twice as long after each next one, up
    /// to `max` ([`DEFAULT_RETRY_BASE`] and [`DEFAULT_RETRY_MAX`] by default).
    pub fn set_backoff(&self, base: Duration, max: Duration) {
        let mut st = lock(&self.inner.state);
        st.retry_base = Some(base);
        st.retry_max = Some(max);
    }

    /// The bounds of the wait before the CA's renewal information about a certificate is asked
    /// for again, whatever its `Retry-After` says (a minute and a day by default).
    pub fn set_ari_bounds(&self, min: Duration, max: Duration) {
        let mut st = lock(&self.inner.state);
        st.ari_min = Some(min);
        st.ari_max = Some(max);
    }

    /// When `name`'s certificate is due for renewal, by its lifetime or by what ARI says,
    /// whichever comes first; `None` for a name it does not manage or has no certificate for.
    #[must_use]
    pub fn renew_at(&self, name: &str) -> Option<SystemTime> {
        let name = normalize(name);
        self.inner
            .certs
            .with(|c| c.contains_key(&name))
            .then_some(())?;
        let st = lock(&self.inner.state);
        let m = st.names.get(&name)?;
        let left = m.due.saturating_duration_since(Instant::now());
        SystemTime::now().checked_add(left)
    }

    /// When the CA's renewal information (ARI) says `name`'s certificate is due: the time picked
    /// in the window it suggested, if it suggested one.
    #[must_use]
    pub fn renewal_info(&self, name: &str) -> Option<SystemTime> {
        let st = lock(&self.inner.state);
        let t = st.names.get(&normalize(name))?.ari.as_ref()?.renew_at?;
        UNIX_EPOCH.checked_add(Duration::from_secs(u64::try_from(t).ok()?))
    }

    fn log(&self, line: &str) {
        let log = Arc::clone(&lock(&self.inner.log));
        log(line);
    }

    /// The URL of the CA's directory.
    #[must_use]
    pub fn directory(&self) -> &str {
        &self.inner.directory
    }

    /// Sets the names it keeps certificates for. A name new to it is worked on at once; one no
    /// longer among them is dropped, and its certificate no longer served, though it stays in
    /// the storage. Names past [`MAX_NAMES`] are left out, and reported.
    pub fn manage<S: AsRef<str>>(&self, names: &[S]) {
        let mut want: Vec<String> = names.iter().map(|n| normalize(n.as_ref())).collect();
        want.sort_unstable();
        want.dedup();
        if want.len() > MAX_NAMES {
            self.log(&format!(
                "{} names to manage: only the first {MAX_NAMES} are",
                want.len()
            ));
            want.truncate(MAX_NAMES);
        }
        let start = {
            let mut st = lock(&self.inner.state);
            if st.closed {
                return;
            }
            let before = st.names.len();
            st.names.retain(|n, _| want.binary_search(n).is_ok());
            let dropped = st.names.len() != before;
            let now = Instant::now();
            for n in want {
                st.names.entry(n).or_insert(Managed {
                    due: now,
                    failures: 0,
                    running: false,
                    last_error: None,
                    revoked: None,
                    ari: None,
                });
            }
            if dropped {
                let certs = self.inner.certs.get();
                let kept = certs
                    .iter()
                    .filter(|(n, _)| st.names.contains_key(*n))
                    .map(|(n, c)| (n.clone(), c.clone()))
                    .collect();
                self.inner.certs.set(kept);
            }
            let start = !st.started && !st.names.is_empty();
            st.started |= start;
            start
        };
        if start {
            let mut threads = lock(&self.inner.threads);
            for i in 0..self.inner.workers {
                let a = self.clone();
                match std::thread::Builder::new()
                    .name(format!("akit-acme-{i}"))
                    .spawn(move || a.work())
                {
                    Ok(t) => threads.push(t),
                    Err(e) => self.log(&format!("starting a worker: {e}")),
                }
            }
        }
        self.inner.wake.notify_all();
    }

    /// The names it manages, sorted.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = lock(&self.inner.state).names.keys().cloned().collect();
        v.sort_unstable();
        v
    }

    /// The certificate it has for `name`, if any: read from memory, without a lock, for a
    /// handshake.
    #[must_use]
    pub fn certificate(&self, name: &str) -> Option<CertifiedKey> {
        self.inner
            .certs
            .with(|m| m.get(name).map(Served::certificate))
    }

    /// The certificate that answers the TLS-ALPN-01 challenge of `name` (normalized: lower case,
    /// no trailing dot), if one is under way: read from memory, without a lock, for a handshake
    /// offering [`ACME_TLS_ALPN`] alone.
    #[must_use]
    pub fn alpn_challenge(&self, name: &str) -> Option<CertifiedKey> {
        self.inner.alpn_certs.with(|m| m.get(name).cloned())
    }

    pub(crate) fn set_alpn(&self, name: &str, cert: Option<CertifiedKey>) {
        let _st = lock(&self.inner.state);
        let mut m = (*self.inner.alpn_certs.get()).clone();
        match cert {
            Some(c) => m.insert(name.to_owned(), c),
            None => m.remove(name),
        };
        self.inner.alpn_certs.set(m);
    }

    /// Has `name` obtained again at once, its certificate with serial number `serial` not taken
    /// from the storage: what its OCSP responder saying it is revoked does.
    fn renew_revoked(&self, name: &str, serial: &[u8]) {
        {
            let mut st = lock(&self.inner.state);
            let Some(m) = st.names.get_mut(name) else {
                return;
            };
            m.revoked = Some(serial.to_vec());
            if !m.running {
                m.due = Instant::now();
            }
        }
        self.log(&format!(
            "{name}: its certificate is revoked; obtaining another"
        ));
        self.inner.wake.notify_all();
    }

    /// Why the last attempt for `name` failed, if it did and none has succeeded since.
    #[must_use]
    pub fn last_error(&self, name: &str) -> Option<String> {
        lock(&self.inner.state)
            .names
            .get(&normalize(name))
            .and_then(|m| m.last_error.clone())
    }

    /// The answer to the HTTP-01 challenge of `token`, if one is under way: read from memory,
    /// without a lock, for a request in the clear to [`ACME_CHALLENGE_PREFIX`].
    #[must_use]
    pub fn http_challenge(&self, token: &str) -> Option<String> {
        self.inner.tokens.with(|m| m.get(token).cloned())
    }

    fn set_token(&self, token: &str, answer: Option<String>) {
        // Only workers change it; the lock orders them.
        let _st = lock(&self.inner.state);
        let mut m = (*self.inner.tokens.get()).clone();
        match answer {
            Some(a) => m.insert(token.to_owned(), a),
            None => m.remove(token),
        };
        self.inner.tokens.set(m);
    }

    /// Stops it: work under way is given up, its requests failed, and its threads joined.
    pub fn close(&self) {
        {
            let mut st = lock(&self.inner.state);
            st.closed = true;
            self.inner.stop.store(true, Ordering::Release);
        }
        self.inner.wake.notify_all();
        let threads: Vec<JoinHandle<()>> = lock(&self.inner.threads).drain(..).collect();
        if let Some(ca) = lock(&self.inner.ca).take() {
            // Fails the requests the workers wait for.
            ca.close();
        }
        for t in threads {
            let _ = t.join();
        }
        self.inner.ocsp.close();
    }

    /// A worker: takes a name that is due and not being worked on, works on it, and sleeps
    /// until the next is due when none is.
    fn work(&self) {
        let mut st = lock(&self.inner.state);
        loop {
            if st.closed {
                return;
            }
            let now = Instant::now();
            let due = st
                .names
                .iter_mut()
                .filter(|(_, m)| !m.running)
                .min_by_key(|(_, m)| m.due)
                .map(|(n, m)| (n.clone(), m));
            match due {
                Some((name, m)) if m.due <= now => {
                    m.running = true;
                    drop(st);
                    self.run(&name);
                    st = lock(&self.inner.state);
                }
                other => {
                    let wait = other.map_or(IDLE_WAIT, |(_, m)| {
                        m.due.saturating_duration_since(now).min(IDLE_WAIT)
                    });
                    st = self
                        .inner
                        .wake
                        .wait_timeout(st, wait)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
            }
        }
    }

    /// Works on `name`, and sets when it is next due: when its certificate is due for renewal,
    /// or, after a failure, later and later.
    fn run(&self, name: &str) {
        let deadline = after(self.inner.timeout);
        let (attempt, revoked, ari) = {
            let st = lock(&self.inner.state);
            st.names.get(name).map_or((0, None, None), |m| {
                (m.failures, m.revoked.clone(), m.ari.clone())
            })
        };
        let r = self.ensure(name, deadline, attempt, revoked.as_deref(), ari.as_ref());
        let serial = r.as_ref().ok().map(|s| s.serial.clone());
        self.settle(name, r);
        // ARI is asked once the certificate is served, so that a CA out of reach delays nothing.
        if let Some(serial) = serial {
            self.check_ari(name, &serial, ari, deadline);
        }
    }

    /// Records how a run of `name` went: serves its certificate, and sets when it is next due.
    fn settle(&self, name: &str, r: Result<Stored, String>) {
        let mut st = lock(&self.inner.state);
        let closed = st.closed;
        let (base, max) = (
            st.retry_base.unwrap_or(DEFAULT_RETRY_BASE),
            st.retry_max.unwrap_or(DEFAULT_RETRY_MAX),
        );
        let Some(m) = st.names.get_mut(name) else {
            // Dropped while it was worked on.
            return;
        };
        m.running = false;
        match r {
            Ok(stored) => {
                m.failures = 0;
                m.last_error = None;
                if m.revoked.as_deref() != Some(stored.serial.as_slice()) {
                    m.revoked = None;
                }
                let wait = u64::try_from(stored.due.saturating_sub(unix_now())).unwrap_or(0);
                m.due = after(Duration::from_secs(wait));
                match &m.ari {
                    // Earlier when the CA's window says so, and when it is time to ask again.
                    Some(a) if a.serial == stored.serial => {
                        if let Some(t) = a.renew_at {
                            m.due = m.due.min(instant_of(t));
                        }
                        m.due = m.due.min(a.check_at);
                    }
                    Some(_) => m.ari = None,
                    None => {}
                }
                let same = self.inner.certs.with(|c| {
                    c.get(name)
                        .is_some_and(|s| s.key.chain_der().next() == stored.key.chain_der().next())
                });
                if !same {
                    // With its staple: a certificate is never served without its response,
                    // nor the last one's response with it.
                    let weak = Arc::downgrade(&self.inner);
                    let (n, serial) = (name.to_owned(), stored.serial.clone());
                    let revoked: crate::ocsp::Revoked = Arc::new(move || {
                        if let Some(inner) = weak.upgrade() {
                            Acme { inner }.renew_revoked(&n, &serial);
                        }
                    });
                    let staple = self
                        .inner
                        .ocsp
                        .add(&stored.key, Some(revoked))
                        .map(Arc::new);
                    let mut certs = (*self.inner.certs.get()).clone();
                    certs.insert(
                        name.to_owned(),
                        Served {
                            key: stored.key,
                            staple,
                        },
                    );
                    self.inner.certs.set(certs);
                }
            }
            Err(_) if closed => {}
            Err(e) => {
                m.failures = m.failures.saturating_add(1);
                let wait = backoff(base, max, m.failures);
                m.due = after(wait);
                m.last_error = Some(e.clone());
                drop(st);
                self.log(&format!("{name}: {e}; trying again in {}s", wait.as_secs()));
                self.inner.wake.notify_all();
                return;
            }
        }
        drop(st);
        self.inner.wake.notify_all();
    }

    /// The storage's keys.
    fn cert_key(&self, name: &str) -> String {
        format!("certificates/{}/{name}.pem", self.inner.ca_key)
    }

    fn account_key(&self) -> String {
        format!("acme/{}/account.pem", self.inner.ca_key)
    }

    /// Asks the CA what ARI says of `name`'s certificate, whose serial number is `serial`, unless
    /// `cur` is about it and it is not yet time to ask again; and has the certificate due when
    /// ARI says, if that is sooner.
    fn check_ari(&self, name: &str, serial: &[u8], cur: Option<AriState>, deadline: Instant) {
        let (least, most) = {
            let st = lock(&self.inner.state);
            (
                st.ari_min.unwrap_or(ari::ARI_CHECK_MIN),
                st.ari_max.unwrap_or(ari::ARI_CHECK_MAX),
            )
        };
        if cur
            .as_ref()
            .is_some_and(|c| c.serial == serial && Instant::now() < c.check_at)
        {
            return;
        }
        let Some(id) = self.inner.certs.with(|c| {
            let leaf = c.get(name)?.key.chain_der().next()?.to_vec();
            let leaf = akio_x509::Certificate::from_der(leaf).ok()?;
            ari::cert_id(leaf.authority_key_id(), leaf.serial())
        }) else {
            return;
        };
        let fallback = AriState {
            serial: serial.to_vec(),
            renew_at: None,
            check_at: after(ari::ARI_CHECK_DEFAULT.min(most)),
        };
        let s = match self.ca().and_then(|ca| ca.renewal_info(&id, deadline)) {
            Ok(None) => return,
            Ok(Some((v, retry))) => {
                let now = unix_now();
                let window = v.get("suggestedWindow");
                let at = |k| window.and_then(|w| string(w, k)).and_then(ari::rfc3339);
                let mut s = AriState {
                    serial: serial.to_vec(),
                    renew_at: None,
                    check_at: after(ari::retry_after(retry.as_deref(), now, least, most)),
                };
                match (at("start"), at("end")) {
                    (Some(start), Some(end)) if end > start => {
                        s.renew_at = ari::pick(start, end, now);
                    }
                    _ => self.log(&format!(
                        "{name}: renewal information: no valid suggested window"
                    )),
                }
                s
            }
            Err(e) => {
                self.log(&format!("{name}: renewal information: {e}"));
                fallback
            }
        };
        let mut st = lock(&self.inner.state);
        let Some(m) = st.names.get_mut(name) else {
            return;
        };
        if m.running {
            // Being worked on again: what it finds decides.
            return;
        }
        if let Some(t) = s.renew_at {
            m.due = m.due.min(instant_of(t));
        }
        m.due = m.due.min(s.check_at);
        m.ari = Some(s);
        drop(st);
        self.inner.wake.notify_all();
    }

    /// `name`'s certificate from the storage, if there is one that can be served, is not due for
    /// renewal (by its lifetime or by `ari`), and is not the revoked one, whose serial number is
    /// `revoked`.
    fn load(
        &self,
        name: &str,
        deadline: Instant,
        revoked: Option<&[u8]>,
        ari: Option<&AriState>,
    ) -> Option<Stored> {
        match self.inner.storage.load(&self.cert_key(name), deadline) {
            Ok(data) => match parse_stored(&data, name) {
                Ok(s)
                    if unix_now() < s.due
                        && revoked != Some(s.serial.as_slice())
                        && !ari.is_some_and(|a| a.due(&s.serial, unix_now())) =>
                {
                    Some(s)
                }
                Ok(_) => None,
                Err(e) => {
                    self.log(&format!(
                        "{name}: the stored certificate cannot be used ({e}); obtaining another"
                    ));
                    None
                }
            },
            Err(StorageError::NotFound) => None,
            Err(e) => {
                self.log(&format!("{name}: loading the certificate: {e}"));
                None
            }
        }
    }

    /// A certificate for `name` that is not due for renewal and not the revoked one: the
    /// storage's, or a new one, which it stores; with when it is due. `attempt` counts the
    /// failures before it, which pick the challenge tried first.
    fn ensure(
        &self,
        name: &str,
        deadline: Instant,
        attempt: u32,
        revoked: Option<&[u8]>,
        ari: Option<&AriState>,
    ) -> Result<Stored, String> {
        if let Some(c) = self.load(name, deadline, revoked, ari) {
            return Ok(c);
        }
        let lock_name = format!("issue/{}/{name}", self.inner.ca_key);
        self.inner
            .storage
            .lock(&lock_name, deadline)
            .map_err(|e| format!("taking the storage's lock: {e}"))?;
        let r = (|| {
            // Another instance may have obtained it while this one waited.
            if let Some(c) = self.load(name, deadline, revoked, ari) {
                return Ok(c);
            }
            let data = self.obtain(name, deadline, attempt)?;
            let stored =
                parse_stored(&data, name).map_err(|e| format!("the CA's certificate: {e}"))?;
            if let Err(e) = self
                .inner
                .storage
                .store(&self.cert_key(name), &data, deadline)
            {
                // Served all the same; the next renewal stores it again.
                self.log(&format!("{name}: storing the certificate: {e}"));
            }
            self.log(&format!("{name}: obtained a certificate"));
            Ok(stored)
        })();
        self.unlock(&lock_name);
        r
    }

    fn unlock(&self, name: &str) {
        if let Err(e) = self.inner.storage.unlock(name) {
            self.log(&format!("giving back the storage's lock {name}: {e}"));
        }
    }

    /// The client of the CA, made the first time.
    fn ca(&self) -> Result<Arc<Ca>, String> {
        let mut ca = lock(&self.inner.ca);
        if let Some(c) = ca.as_ref() {
            return Ok(Arc::clone(c));
        }
        if self.inner.stop.load(Ordering::Acquire) {
            return Err("stopped".into());
        }
        let c = Arc::new(Ca::new(
            &self.inner.directory,
            self.inner.roots.clone(),
            self.inner.request_timeout,
            Arc::clone(&self.inner.stop),
        )?);
        *ca = Some(Arc::clone(&c));
        Ok(c)
    }

    /// The account's key and URL, registering it, or making its key, the first time.
    fn account(&self, ca: &Ca, deadline: Instant) -> Result<Arc<(AccountKey, String)>, String> {
        if let Some(a) = lock(&self.inner.account).as_ref() {
            return Ok(Arc::clone(a));
        }
        let lock_name = format!("account/{}", self.inner.ca_key);
        self.inner
            .storage
            .lock(&lock_name, deadline)
            .map_err(|e| format!("taking the storage's lock: {e}"))?;
        let r = (|| {
            let key = match self.inner.storage.load(&self.account_key(), deadline) {
                Ok(pem) => AccountKey::from_pem(&pem)
                    .map_err(|e| format!("the account key in {}: {e}", self.account_key()))?,
                Err(StorageError::NotFound) => {
                    let key = AccountKey::generate()?;
                    self.inner
                        .storage
                        .store(&self.account_key(), key.to_pem().as_bytes(), deadline)
                        .map_err(|e| format!("storing the account key: {e}"))?;
                    key
                }
                Err(e) => return Err(format!("loading the account key: {e}")),
            };
            let kid = ca
                .register(&key, &self.inner.email, deadline)
                .map_err(|e| format!("registering the account: {e}"))?;
            Ok(Arc::new((key, kid)))
        })();
        self.unlock(&lock_name);
        let a = r?;
        let mut slot = lock(&self.inner.account);
        Ok(Arc::clone(slot.get_or_insert(a)))
    }

    /// Orders a certificate for `name`, answers its challenges, and returns it, its chain and
    /// key as one PEM to store.
    fn obtain(&self, name: &str, deadline: Instant, attempt: u32) -> Result<Vec<u8>, String> {
        let ca = self.ca()?;
        let account = self.account(&ca, deadline)?;
        let (key, kid) = (&account.0, account.1.as_str());
        let (order_url, order) = ca
            .new_order(key, kid, name, deadline)
            .map_err(|e| format!("ordering: {e}"))?;
        for authz in strings(&order, "authorizations") {
            self.authorize(&ca, key, kid, authz, deadline, attempt)?;
        }
        let order = ca
            .poll(key, kid, &order_url, &["ready"], None, deadline)
            .map_err(|e| format!("waiting for the order: {e}"))?;
        let finalize = string(&order, "finalize").ok_or("the order has no finalize URL")?;
        let cert_key = akio_crypto::p256::SecretKey::generate().map_err(|e| e.to_string())?;
        let cert_key = akio_crypto::pkey::PrivateKey::P256(cert_key);
        let csr = CsrParams::new([name])
            .sign(&cert_key)
            .map_err(|e| format!("the CSR: {e}"))?;
        let (order, retry) = ca
            .finalize(key, kid, finalize, csr.as_der(), deadline)
            .map_err(|e| format!("finalizing the order: {e}"))?;
        let order = if string(&order, "status") == Some("valid") {
            order
        } else {
            ca.poll(key, kid, &order_url, &["valid"], retry, deadline)
                .map_err(|e| format!("finalizing the order: {e}"))?
        };
        let cert_url = string(&order, "certificate").ok_or("the order has no certificate URL")?;
        let mut data = ca
            .certificate(key, kid, cert_url, deadline)
            .map_err(|e| format!("downloading the certificate: {e}"))?;
        if !data.ends_with(b"\n") {
            data.push(b'\n');
        }
        data.extend_from_slice(
            akio_crypto::pem::encode("PRIVATE KEY", &cert_key.to_pkcs8_der()).as_bytes(),
        );
        Ok(data)
    }

    /// Answers a challenge of the authorization at `url`, unless it is valid already, and waits
    /// for the CA to check it: the first of `acme.challenges`, starting at the `attempt`th, that
    /// the CA offers (fit's).
    fn authorize(
        &self,
        ca: &Ca,
        key: &AccountKey,
        kid: &str,
        url: &str,
        deadline: Instant,
        attempt: u32,
    ) -> Result<(), String> {
        let (authz, _) = ca
            .get(key, kid, url, deadline)
            .map_err(|e| format!("authorization: {e}"))?;
        if string(&authz, "status") == Some("valid") {
            return Ok(());
        }
        let ident = authz
            .get("identifier")
            .and_then(|i| string(i, "value"))
            .unwrap_or("");
        let offered = match authz.get("challenges") {
            Some(akit_config::value::Value::Array(cs)) => cs.as_slice(),
            _ => &[],
        };
        let types = &self.inner.challenges;
        let first = usize::try_from(attempt)
            .unwrap_or(0)
            .checked_rem(types.len())
            .unwrap_or(0);
        let (typ, chal) = types
            .iter()
            .cycle()
            .skip(first)
            .take(types.len())
            .find_map(|t| {
                offered
                    .iter()
                    .find(|c| string(c, "type") == Some(challenge_name(*t)))
                    .map(|c| (*t, c))
            })
            .ok_or_else(|| {
                let names: Vec<&str> = types.iter().map(|t| challenge_name(*t)).collect();
                format!(
                    "the CA offers none of the challenges {} for {ident}",
                    names.join(", ")
                )
            })?;
        let token = string(chal, "token").unwrap_or("");
        let chal_url = string(chal, "url").ok_or("the challenge has no URL")?;
        if token.is_empty()
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(format!("the challenge's token {token:?} is not base64url"));
        }
        let key_auth = key.key_authorization(token);
        let _answering = match typ {
            Challenge::Http01 => {
                self.set_token(token, Some(key_auth));
                Answering::Token(self, token.to_owned())
            }
            Challenge::TlsAlpn01 => {
                let name = normalize(ident);
                let cert = alpn_cert(&name, &key_auth)
                    .map_err(|e| format!("the tls-alpn-01 certificate: {e}"))?;
                self.set_alpn(&name, Some(cert));
                Answering::Alpn(self, name)
            }
        };
        ca.accept(key, kid, chal_url, deadline)
            .map_err(|e| format!("accepting the challenge: {e}"))?;
        ca.poll(key, kid, url, &["valid"], None, deadline)
            .map_err(|e| format!("the challenge: {e}"))?;
        Ok(())
    }
}

/// A challenge as ACME names it.
fn challenge_name(c: Challenge) -> &'static str {
    match c {
        Challenge::Http01 => "http-01",
        Challenge::TlsAlpn01 => "tls-alpn-01",
    }
}

/// How long to wait after the `n`th failure in a row: `base` doubled `n - 1` times, up to
/// `max`, less up to a tenth at random.
pub(crate) fn backoff(base: Duration, max: Duration, n: u32) -> Duration {
    let mut d = base;
    for _ in 1..n {
        if d >= max {
            break;
        }
        d = d.saturating_mul(2);
    }
    let d = d.min(max);
    let tenth = d.checked_div(10).unwrap_or_default();
    let mut r = [0u8; 8];
    // Without randomness the waits just line up.
    let _ = akio_crypto::rand::fill_random(&mut r);
    let nanos = u64::try_from(tenth.as_nanos()).unwrap_or(u64::MAX);
    let less = u64::from_le_bytes(r)
        .checked_rem(nanos.saturating_add(1))
        .unwrap_or(0);
    d.saturating_sub(Duration::from_nanos(less))
}

/// A storage for a process with no home to keep certificates in: every call fails.
#[derive(Debug)]
struct NoStorage;

impl NoStorage {
    fn err() -> StorageError {
        StorageError::Io(std::io::Error::other(
            "no storage directory: set acme.storage ($HOME is not set)",
        ))
    }
}

impl Storage for NoStorage {
    fn load(&self, _: &str, _: Instant) -> Result<Vec<u8>, StorageError> {
        Err(NoStorage::err())
    }
    fn store(&self, _: &str, _: &[u8], _: Instant) -> Result<(), StorageError> {
        Err(NoStorage::err())
    }
    fn delete(&self, _: &str, _: Instant) -> Result<(), StorageError> {
        Err(NoStorage::err())
    }
    fn list(&self, _: &str, _: Instant) -> Result<Vec<String>, StorageError> {
        Err(NoStorage::err())
    }
    fn lock(&self, _: &str, _: Instant) -> Result<(), StorageError> {
        Err(NoStorage::err())
    }
    fn unlock(&self, _: &str) -> Result<(), StorageError> {
        Err(NoStorage::err())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic)]
    use super::*;

    #[test]
    fn ca_keys_as_fit() {
        assert_eq!(
            ca_key("https://acme-v02.api.letsencrypt.org/directory"),
            "acme-v02.api.letsencrypt.org-directory"
        );
        assert_eq!(ca_key("https://127.0.0.1:14000/dir"), "127.0.0.1-14000-dir");
        assert_eq!(ca_key(""), "ca-");
        assert_eq!(ca_key("https://.x/"), "ca-.x");
    }

    #[test]
    fn renewal_a_third_before_expiry() {
        let day = 86_400;
        let at = renew_at(0, 90 * day, &[1, 2, 3]);
        // A third of 90 days before the end, less up to a fiftieth of them.
        assert!(at <= 60 * day && at > 60 * day - 90 * day / 50, "{at}");
        assert_eq!(at, renew_at(0, 90 * day, &[1, 2, 3]));
        assert_eq!(renew_at(10, 10, &[]), 10);
    }

    /// RFC 8737 section 3: self-signed, for the name alone, with a critical acmeIdentifier
    /// holding the key authorization's SHA-256.
    #[test]
    fn tls_alpn_certificates() {
        let k = alpn_cert("a.akit.test", "tok.thumb").unwrap_or_else(|e| panic!("{e}"));
        let der = k.chain_der().next().unwrap_or_default().to_vec();
        assert_eq!(k.chain_der().count(), 1);
        let c = akio_x509::Certificate::from_der(der).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            c.subject_alt_names(),
            [GeneralName::Dns("a.akit.test".into())]
        );
        let e = c
            .extensions()
            .iter()
            .find(|e| e.oid == ACME_IDENTIFIER)
            .unwrap_or_else(|| panic!("no acmeIdentifier"));
        assert!(e.critical);
        let mut want = vec![0x04, 0x20];
        want.extend_from_slice(&Sha256::digest(b"tok.thumb"));
        assert_eq!(e.value, want);
        assert_eq!(c.issuer(), c.subject());
    }

    /// A certificate its OCSP responder says is revoked is obtained again at once, the stored
    /// one not taken; until another is obtained, the one in memory is still served.
    #[test]
    fn a_revoked_certificate_is_not_taken_from_the_storage() {
        let name = "revoked.akit.test";
        let root = std::env::temp_dir().join(format!("akit-acme-revoked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let a = Acme::with_storage(
            &akit_config::Acme {
                directory: "https://127.0.0.1:1/dir".into(),
                timeout: Duration::from_secs(5),
                ..akit_config::Acme::default()
            },
            Arc::new(FileStorage::new(&root)),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        a.set_log(Arc::new(|l: &str| eprintln!("acme: {l}")));
        a.set_backoff(Duration::from_secs(60), Duration::from_secs(60));
        let key = akio_crypto::pkey::PrivateKey::P256(
            akio_crypto::p256::SecretKey::generate().unwrap_or_else(|e| panic!("{e}")),
        );
        let cert = CertificateParams::new([name])
            .self_signed(&key)
            .unwrap_or_else(|e| panic!("{e}"));
        let pem = format!(
            "{}{}",
            cert.to_pem(),
            akio_crypto::pem::encode("PRIVATE KEY", &key.to_pkcs8_der())
        );
        let deadline = after(Duration::from_secs(5));
        a.inner
            .storage
            .store(&a.cert_key(name), pem.as_bytes(), deadline)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(a.load(name, deadline, None, None).is_some());
        assert!(a.load(name, deadline, Some(cert.serial()), None).is_none());
        assert!(a.load(name, deadline, Some(&[1, 2, 3]), None).is_some());

        // Managed: the stored one is served, without the CA.
        a.manage(&[name]);
        let wait = |f: &dyn Fn() -> bool| {
            let end = after(Duration::from_secs(10));
            while !f() {
                assert!(Instant::now() < end, "timed out");
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        wait(&|| a.certificate(name).is_some());
        // Revoked: obtained again at once, which fails without a CA.
        a.renew_revoked(name, cert.serial());
        wait(&|| a.last_error(name).is_some());
        let e = a.last_error(name).unwrap_or_default();
        assert!(e.contains("127.0.0.1:1"), "{e}");
        let st = lock(&a.inner.state);
        assert_eq!(
            st.names
                .get(name)
                .and_then(|m| m.revoked.clone())
                .as_deref(),
            Some(cert.serial())
        );
        drop(st);
        assert!(a.certificate(name).is_some(), "the one in memory dropped");
        a.close();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn challenges_named_as_acme() {
        assert_eq!(challenge_name(Challenge::Http01), "http-01");
        assert_eq!(challenge_name(Challenge::TlsAlpn01), "tls-alpn-01");
    }

    #[test]
    fn backoff_doubles_up_to_its_cap() {
        let (b, m) = (Duration::from_secs(60), Duration::from_secs(3600));
        for (n, full) in [(1, 60), (2, 120), (3, 240), (7, 3600), (40, 3600)] {
            let d = backoff(b, m, n);
            let full = Duration::from_secs(full);
            assert!(d <= full && d >= full - full / 10, "{n}: {d:?}");
        }
    }
}
