//! Certificates obtained with ACME (RFC 8555), as fit's `tlsmgr.ACME` (fit's decision 007): an
//! ACME client of akit's own on akio-crypto (JWS, ECDSA), akio-x509 (the CSR) and akio-http's
//! client, run off the I/O threads.
//!
//! - **Accounts**: one per CA, its P-256 key kept in the storage and registered, agreeing to the
//!   CA's terms, the first time it is needed.
//! - **Orders**: a certificate per name, its own P-256 key, through the HTTP-01 challenge, which
//!   every listener in the clear answers from memory ([`Acme::http_challenge`]). TLS-ALPN-01 comes
//!   with I32.
//! - **The storage** ([`Storage`]): a certificate there is used as it is until it is due for
//!   renewal, so that a restart, or another instance sharing the storage, does not obtain it
//!   again; instances take a lock in it before they obtain one, and look again once they hold it.
//! - **Renewal**: once a third of a certificate's lifetime is left, up to a fiftieth of it
//!   earlier by its serial number, as fit's; a failure is tried again after a minute, then twice
//!   as long each time, up to an hour.
//! - **Threads**: `acme.max_concurrent` workers of the control plane, started when the first name
//!   is managed, each order bounded by `acme.timeout` and each request by
//!   `acme.request_timeout`. A handshake asks for a certificate ([`Acme::certificate`]) and a
//!   request for an HTTP-01 answer, both read from memory, without a lock (rules 1 and 13).

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
use akio_x509::{CsrParams, GeneralName};

use crate::published::Published;
use crate::storage::{FileStorage, Storage, StorageError};

use jws::AccountKey;
use protocol::{Ca, string, strings};

/// Where a CA asks for an HTTP-01 challenge's answer: this path, then the challenge's token.
pub const ACME_CHALLENGE_PREFIX: &str = "/.well-known/acme-challenge/";

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
}

#[derive(Debug, Default)]
struct State {
    names: HashMap<String, Managed>,
    started: bool,
    closed: bool,
    retry_base: Option<Duration>,
    retry_max: Option<Duration>,
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
    /// The certificate of each name that has one, and the key authorization of each HTTP-01
    /// challenge under way, by its token: what the I/O threads read.
    certs: Published<HashMap<String, CertifiedKey>>,
    tokens: Published<HashMap<String, String>>,
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
fn parse_stored(data: &[u8], name: &str) -> Result<(CertifiedKey, i64), String> {
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
    Ok((ck, due))
}

/// The key authorization of a token under way, given back when it is dropped.
struct Answering<'a> {
    acme: &'a Acme,
    token: String,
}

impl Drop for Answering<'_> {
    fn drop(&mut self) {
        self.acme.set_token(&self.token, None);
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
                certs: Published::new(HashMap::new()),
                tokens: Published::new(HashMap::new()),
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

    /// Has the manager report through `log` from now on (standard error by default).
    pub fn set_log(&self, log: Log) {
        *lock(&self.inner.log) = log;
    }

    /// The waits after failures: `base` after the first, twice as long after each next one, up
    /// to `max` ([`DEFAULT_RETRY_BASE`] and [`DEFAULT_RETRY_MAX`] by default).
    pub fn set_backoff(&self, base: Duration, max: Duration) {
        let mut st = lock(&self.inner.state);
        st.retry_base = Some(base);
        st.retry_max = Some(max);
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
        self.inner.certs.with(|m| m.get(name).cloned())
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
        let r = self.ensure(name, deadline);
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
            Ok((cert, renew)) => {
                m.failures = 0;
                m.last_error = None;
                let wait = u64::try_from(renew.saturating_sub(unix_now())).unwrap_or(0);
                m.due = after(Duration::from_secs(wait));
                let mut certs = (*self.inner.certs.get()).clone();
                certs.insert(name.to_owned(), cert);
                self.inner.certs.set(certs);
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

    /// `name`'s certificate from the storage, if there is one that can be served and is not due
    /// for renewal.
    fn load(&self, name: &str, deadline: Instant) -> Option<(CertifiedKey, i64)> {
        match self.inner.storage.load(&self.cert_key(name), deadline) {
            Ok(data) => match parse_stored(&data, name) {
                Ok((ck, due)) if unix_now() < due => Some((ck, due)),
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

    /// A certificate for `name` that is not due for renewal: the storage's, or a new one, which
    /// it stores; with when it is due.
    fn ensure(&self, name: &str, deadline: Instant) -> Result<(CertifiedKey, i64), String> {
        if let Some(c) = self.load(name, deadline) {
            return Ok(c);
        }
        let lock_name = format!("issue/{}/{name}", self.inner.ca_key);
        self.inner
            .storage
            .lock(&lock_name, deadline)
            .map_err(|e| format!("taking the storage's lock: {e}"))?;
        let r = (|| {
            // Another instance may have obtained it while this one waited.
            if let Some(c) = self.load(name, deadline) {
                return Ok(c);
            }
            let data = self.obtain(name, deadline)?;
            let (ck, due) =
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
            Ok((ck, due))
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
    fn obtain(&self, name: &str, deadline: Instant) -> Result<Vec<u8>, String> {
        let ca = self.ca()?;
        let account = self.account(&ca, deadline)?;
        let (key, kid) = (&account.0, account.1.as_str());
        let (order_url, order) = ca
            .new_order(key, kid, name, deadline)
            .map_err(|e| format!("ordering: {e}"))?;
        for authz in strings(&order, "authorizations") {
            self.authorize(&ca, key, kid, authz, deadline)?;
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

    /// Answers the HTTP-01 challenge of the authorization at `url`, unless it is valid already,
    /// and waits for the CA to check it.
    fn authorize(
        &self,
        ca: &Ca,
        key: &AccountKey,
        kid: &str,
        url: &str,
        deadline: Instant,
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
        let chal = match authz.get("challenges") {
            Some(akit_config::value::Value::Array(cs)) => {
                cs.iter().find(|c| string(c, "type") == Some("http-01"))
            }
            _ => None,
        }
        .ok_or_else(|| format!("the CA offers no http-01 challenge for {ident}"))?;
        let token = string(chal, "token").unwrap_or("");
        let chal_url = string(chal, "url").ok_or("the challenge has no URL")?;
        if token.is_empty()
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(format!("the challenge's token {token:?} is not base64url"));
        }
        self.set_token(token, Some(key.key_authorization(token)));
        let _answering = Answering {
            acme: self,
            token: token.to_owned(),
        };
        ca.accept(key, kid, chal_url, deadline)
            .map_err(|e| format!("accepting the challenge: {e}"))?;
        ca.poll(key, kid, url, &["valid"], None, deadline)
            .map_err(|e| format!("the challenge: {e}"))?;
        Ok(())
    }
}

/// How long to wait after the `n`th failure in a row: `base` doubled `n - 1` times, up to
/// `max`, less up to a tenth at random.
fn backoff(base: Duration, max: Duration, n: u32) -> Duration {
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
