//! OCSP stapling (I32), as fit's `tlsmgr.OCSP`: the response of each certificate that names a
//! responder, fetched from it off the I/O threads and stapled to the certificate a handshake
//! gets.
//!
//! - **Fetches** run on [`OCSP_WORKERS`] threads of the control plane, started with the first
//!   certificate, each bounded by [`OCSP_TIMEOUT`], its response at most [`OCSP_MAX_BYTES`],
//!   through akio-http's client on an engine of its own, no redirect followed. A response is
//!   checked (akio-x509's `OcspResponse::verify`: signed by the issuer or a responder it
//!   delegated to, about this certificate, current) before it is stapled; it is fetched again
//!   halfway through its validity, or after [`OCSP_REFRESH`] when it gives no next update; a
//!   failure is tried again later and later, as ACME's, and a response is never stapled past its
//!   next update.
//! - **Handshakes** read the certificate with its latest good response from memory, without a
//!   lock ([`Staple::certificate`]): a response revoked, unknown or expired leaves the
//!   certificate served without one.
//! - **Shared by chain**: certificates are kept by the SHA-256 of their chain, with a count of
//!   their users, so the graphs a reload makes share the response of a certificate both serve;
//!   the last [`Staple`] dropped drops it.
//! - **Revoked**: a certificate ACME obtained whose responder says it is revoked is obtained
//!   again ([`crate::Acme`]).
//! - **Certificates from files** may have been written from what someone else gave, so their
//!   responders are asked only when their chain verifies against the roots
//!   [`trust_roots`](Ocsp::trust_roots) gives, and only at a public address (unless those roots
//!   allow private ones), over `http`, dialled at the address checked. fit checks them against
//!   the system's roots by default; akio-tls does not expose those to a verification outside a
//!   handshake yet (akio API request A57), so until then a certificate from files is stapled only
//!   under roots given.

use std::collections::HashMap;
use std::fmt;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use akio::Engine;
use akio::tls::CertifiedKey;
use akio_crypto::hash::Sha256;
use akio_http::client::{Client, ClientOptions, ClientRequest, RedirectPolicy};
use akio_x509::{
    Certificate, OcspRequest, OcspResponse, OcspValidation, RevocationStatus, RootStore, Usage,
    Verifier,
};

use crate::acme::{DEFAULT_RETRY_BASE, DEFAULT_RETRY_MAX, Log, backoff};
use crate::published::Published;

/// How many responses are fetched at once.
pub const OCSP_WORKERS: usize = 2;
/// How long one fetch may take.
pub const OCSP_TIMEOUT: Duration = Duration::from_secs(10);
/// The largest response read (rule 11).
pub const OCSP_MAX_BYTES: usize = 64 << 10;
/// How often a response with no next update is fetched again, and a certificate whose responder
/// is not asked looked at again.
pub const OCSP_REFRESH: Duration = Duration::from_secs(3600);
/// The most certificates kept responses for (rule 11): far more than configs and ACME hold.
pub const MAX_STAPLES: usize = 20_000;
/// How long a wait for a response sleeps before it looks at the stop flag again.
const SLICE: Duration = Duration::from_millis(50);

/// A chain's SHA-256: what certificates are kept by.
type Id = [u8; 32];

/// A certificate with its response stapled, and that response.
type Stapled = (CertifiedKey, Arc<[u8]>);

/// What a revoked certificate's owner hears.
pub(crate) type Revoked = Arc<dyn Fn() + Send + Sync>;

/// A certificate kept a response for, as fetches see it.
struct Entry {
    base: CertifiedKey,
    leaf: Certificate,
    issuer: Certificate,
    /// The rest of the chain, for a certificate from files to be verified.
    intermediates: Vec<Certificate>,
    url: String,
    /// The first [`Ocsp::add`] of the chain sets it.
    revoked: Option<Revoked>,
    /// How the certificate is named in what is reported.
    subject: String,
}

/// When an entry is next fetched, and what its fetches found.
struct Sched {
    entry: Arc<Entry>,
    refs: usize,
    /// Every add of it was for a certificate from files.
    from_files: bool,
    due: Instant,
    /// When the response stapled stops being valid (Unix seconds), while one is.
    expires: Option<i64>,
    failures: u32,
    running: bool,
    /// The chain verified against the trusted roots (a certificate from files).
    verified: bool,
    /// Its responder not being asked was reported.
    told: bool,
}

#[derive(Default)]
struct State {
    staples: HashMap<Id, Sched>,
    started: bool,
    closed: bool,
}

struct Inner {
    state: Mutex<State>,
    wake: Condvar,
    stop: Arc<AtomicBool>,
    /// Each certificate with its latest good response, and that response: what handshakes
    /// read.
    stapled: Published<HashMap<Id, Stapled>>,
    /// The roots certificates from files must verify against, and whether their responders may
    /// be at private addresses.
    trust: Mutex<Option<(Arc<RootStore>, bool)>>,
    retry: Mutex<(Duration, Duration)>,
    log: Mutex<Log>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    client: Mutex<Option<Arc<Fetcher>>>,
}

/// Keeps the OCSP responses of the certificates it is given, and staples them. Cheap to clone:
/// the clones share it. [`close`](Self::close) stops it.
#[derive(Clone)]
pub struct Ocsp {
    inner: Arc<Inner>,
}

impl fmt::Debug for Ocsp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ocsp")
            .field("staples", &lock(&self.inner.state).staples.len())
            .finish_non_exhaustive()
    }
}

/// A certificate an [`Ocsp`] keeps a response for; dropping it undoes the [`Ocsp::add`].
pub struct Staple {
    id: Id,
    base: CertifiedKey,
    inner: Arc<Inner>,
}

impl fmt::Debug for Staple {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Staple").finish_non_exhaustive()
    }
}

impl Staple {
    /// The certificate with its latest good response stapled, or without one while there is
    /// none: read from memory, without a lock, for a handshake.
    #[must_use]
    pub fn certificate(&self) -> CertifiedKey {
        self.inner
            .stapled
            .with(|m| m.get(&self.id).map(|(k, _)| k.clone()))
            .unwrap_or_else(|| self.base.clone())
    }

    /// The response stapled now (DER), if one is.
    #[must_use]
    pub fn response(&self) -> Option<Arc<[u8]>> {
        self.inner
            .stapled
            .with(|m| m.get(&self.id).map(|(_, r)| Arc::clone(r)))
    }
}

impl Drop for Staple {
    fn drop(&mut self) {
        let mut st = lock(&self.inner.state);
        let gone = match st.staples.get_mut(&self.id) {
            Some(s) => {
                s.refs = s.refs.saturating_sub(1);
                s.refs == 0
            }
            None => false,
        };
        if gone {
            st.staples.remove(&self.id);
            unstaple(&self.inner, &self.id);
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
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

fn after(d: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(d).unwrap_or(now)
}

/// Removes `id`'s response from what handshakes read. The state's lock is held.
fn unstaple(inner: &Inner, id: &Id) {
    if inner.stapled.with(|m| m.contains_key(id)) {
        let mut m = (*inner.stapled.get()).clone();
        m.remove(id);
        inner.stapled.set(m);
    }
}

/// A host's addresses are public: global unicast, not loopback, private, link-local, shared
/// (RFC 6598), unique local, documentation or the like; an IPv4-mapped or NAT64 (RFC 6052)
/// address as its IPv4 address (fit's `publicAddr`).
#[must_use]
pub fn public_addr(addr: IpAddr) -> bool {
    let v4 = |a: std::net::Ipv4Addr| {
        let o = a.octets();
        !(a.is_unspecified()
            || a.is_loopback()
            || a.is_private()
            || a.is_link_local()
            || a.is_broadcast()
            || a.is_multicast()
            || a.is_documentation()
            // 0.0.0.0/8, 100.64.0.0/10 (shared), 192.0.0.0/24, 198.18.0.0/15, 240.0.0.0/4.
            || o[0] == 0
            || (o[0] == 100 && (o[1] & 0xc0) == 64)
            || (o[0] == 192 && o[1] == 0 && o[2] == 0)
            || (o[0] == 198 && (o[1] & 0xfe) == 18)
            || o[0] >= 240)
    };
    match addr {
        IpAddr::V4(a) => v4(a),
        IpAddr::V6(a) => {
            if let Some(m) = a.to_ipv4_mapped() {
                return v4(m);
            }
            let s = a.segments();
            if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                let [_, _, _, _, _, _, hi, lo] = s;
                let [a0, a1] = hi.to_be_bytes();
                let [a2, a3] = lo.to_be_bytes();
                return v4(std::net::Ipv4Addr::new(a0, a1, a2, a3));
            }
            // Global unicast is 2000::/3; within it, documentation (2001:db8::/32).
            (s[0] & 0xe000) == 0x2000 && !(s[0] == 0x2001 && s[1] == 0x0db8)
        }
    }
}

impl Default for Ocsp {
    fn default() -> Ocsp {
        Ocsp::new()
    }
}

impl Ocsp {
    /// A stapler with no certificates; it runs no thread until the first is added.
    #[must_use]
    pub fn new() -> Ocsp {
        let log: Log = Arc::new(|line: &str| eprintln!("akit: ocsp: {line}"));
        Ocsp {
            inner: Arc::new(Inner {
                state: Mutex::new(State::default()),
                wake: Condvar::new(),
                stop: Arc::new(AtomicBool::new(false)),
                stapled: Published::new(HashMap::new()),
                trust: Mutex::new(None),
                retry: Mutex::new((DEFAULT_RETRY_BASE, DEFAULT_RETRY_MAX)),
                log: Mutex::new(log),
                threads: Mutex::new(Vec::new()),
                client: Mutex::new(None),
            }),
        }
    }

    /// Has it report through `log` from now on (standard error by default).
    pub fn set_log(&self, log: Log) {
        *lock(&self.inner.log) = log;
    }

    /// The waits after failed fetches: `base` after the first, twice as long after each next
    /// one, up to `max` (ACME's by default).
    pub fn set_backoff(&self, base: Duration, max: Duration) {
        *lock(&self.inner.retry) = (base, max);
    }

    /// Has it fetch the responses of certificates from files whose chain verifies against
    /// `roots`, from responders at public addresses, or, with `private`, at any: for an operator
    /// whose files hold certificates of a CA of their own, and for tests (fit's `TrustRoots`).
    pub fn trust_roots(&self, roots: RootStore, private: bool) {
        *lock(&self.inner.trust) = Some((Arc::new(roots), private));
        let mut st = lock(&self.inner.state);
        let now = Instant::now();
        for s in st.staples.values_mut() {
            if s.from_files && !s.verified && !s.running {
                s.due = now;
            }
        }
        drop(st);
        self.inner.wake.notify_all();
    }

    fn log(&self, line: &str) {
        let log = Arc::clone(&lock(&self.inner.log));
        log(line);
    }

    /// Keeps a response for `cert`, which comes from a CA the operator chose, as ACME's do, and
    /// returns its [`Staple`]; `None` when it names no responder, has no issuer in its chain to
    /// check a response against, or [`MAX_STAPLES`] are kept. `revoked` hears that the
    /// responder says it is revoked; the first add of a chain sets it.
    #[must_use]
    pub fn add(&self, cert: &CertifiedKey, revoked: Option<Revoked>) -> Option<Staple> {
        self.add_as(cert, revoked, false)
    }

    /// The same, for a certificate from files (see [`crate::ocsp`]).
    #[must_use]
    pub fn add_from_files(&self, cert: &CertifiedKey) -> Option<Staple> {
        self.add_as(cert, None, true)
    }

    fn add_as(
        &self,
        cert: &CertifiedKey,
        revoked: Option<Revoked>,
        from_files: bool,
    ) -> Option<Staple> {
        let ders: Vec<&[u8]> = cert.chain_der().collect();
        let (leaf_der, issuer_der) = match ders.as_slice() {
            [leaf, issuer, ..] => (*leaf, *issuer),
            _ => return None,
        };
        let mut joined = Vec::new();
        for d in &ders {
            joined.extend_from_slice(d);
        }
        let id = Sha256::digest(&joined);
        let staple = Staple {
            id,
            base: cert.clone(),
            inner: Arc::clone(&self.inner),
        };
        let start = {
            let mut st = lock(&self.inner.state);
            if st.closed {
                return None;
            }
            if let Some(s) = st.staples.get_mut(&id) {
                s.refs = s.refs.saturating_add(1);
                // A chain an operator's CA issued is no less trusted for being in files too.
                s.from_files &= from_files;
                return Some(staple);
            }
            let leaf = Certificate::from_der(leaf_der.to_vec()).ok()?;
            let url = leaf.ocsp_urls().first()?.clone();
            let issuer = Certificate::from_der(issuer_der.to_vec()).ok()?;
            if st.staples.len() >= MAX_STAPLES {
                drop(st);
                self.log(&format!(
                    "{MAX_STAPLES} certificates are kept responses for: no more are"
                ));
                return None;
            }
            let intermediates = ders
                .iter()
                .skip(1)
                .filter_map(|d| Certificate::from_der(d.to_vec()).ok())
                .collect();
            let subject = subject(&leaf);
            st.staples.insert(
                id,
                Sched {
                    entry: Arc::new(Entry {
                        base: cert.clone(),
                        leaf,
                        issuer,
                        intermediates,
                        url,
                        revoked,
                        subject,
                    }),
                    refs: 1,
                    from_files,
                    due: Instant::now(),
                    expires: None,
                    failures: 0,
                    running: false,
                    verified: false,
                    told: false,
                },
            );
            let start = !st.started;
            st.started = true;
            start
        };
        if start {
            let mut threads = lock(&self.inner.threads);
            for i in 0..OCSP_WORKERS {
                let o = self.clone();
                match std::thread::Builder::new()
                    .name(format!("akit-ocsp-{i}"))
                    .spawn(move || o.work())
                {
                    Ok(t) => threads.push(t),
                    Err(e) => self.log(&format!("starting a worker: {e}")),
                }
            }
        }
        self.inner.wake.notify_all();
        Some(staple)
    }

    /// How many certificates it keeps responses for.
    #[must_use]
    pub fn len(&self) -> usize {
        lock(&self.inner.state).staples.len()
    }

    /// It keeps none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Stops it: fetches under way are given up and its threads joined. The certificates it
    /// stapled keep their last responses, which nothing fetches again.
    pub fn close(&self) {
        {
            let mut st = lock(&self.inner.state);
            st.closed = true;
            self.inner.stop.store(true, Ordering::Release);
        }
        self.inner.wake.notify_all();
        let threads: Vec<JoinHandle<()>> = lock(&self.inner.threads).drain(..).collect();
        if let Some(c) = lock(&self.inner.client).take() {
            c.close();
        }
        for t in threads {
            let _ = t.join();
        }
    }

    /// A worker: takes a certificate that is due and not being fetched, fetches its response,
    /// and sleeps until the next is due when none is.
    fn work(&self) {
        let mut st = lock(&self.inner.state);
        loop {
            if st.closed {
                return;
            }
            let now = Instant::now();
            let due = st
                .staples
                .iter_mut()
                .filter(|(_, s)| !s.running)
                .min_by_key(|(_, s)| s.due)
                .map(|(id, s)| (*id, s));
            match due {
                Some((id, s)) if s.due <= now => {
                    s.running = true;
                    let job = (Arc::clone(&s.entry), s.from_files, s.verified);
                    drop(st);
                    self.run(id, job);
                    st = lock(&self.inner.state);
                }
                other => {
                    let wait = other.map_or(OCSP_REFRESH, |(_, s)| {
                        s.due.saturating_duration_since(now).min(OCSP_REFRESH)
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

    /// Fetches `id`'s response, staples it if it is good, and sets when it is next due.
    fn run(&self, id: Id, (entry, from_files, verified): (Arc<Entry>, bool, bool)) {
        let mut verified_now = verified;
        let r = if from_files && !verified {
            self.verify(&entry).map(|()| {
                verified_now = true;
            })
        } else {
            Ok(())
        };
        let r = r.and_then(|()| self.fetch(&entry, from_files));
        let (base, max) = *lock(&self.inner.retry);
        let mut st = lock(&self.inner.state);
        let closed = st.closed;
        let Some(s) = st.staples.get_mut(&id) else {
            // Released while it was fetched.
            return;
        };
        s.running = false;
        s.verified = verified_now;
        let now = Instant::now();
        let mut report = None;
        let mut revoked = None;
        match r {
            Err(Failure::Unverified(e)) => {
                // Looked at again now and then, as the roots may change; said once.
                if !s.told {
                    s.told = true;
                    report = Some(format!("{}: no response stapled: {e}", entry.subject));
                }
                s.due = after(OCSP_REFRESH);
            }
            Err(Failure::Fetch(_)) if closed => {}
            Err(Failure::Fetch(e)) => {
                s.failures = s.failures.saturating_add(1);
                let wait = backoff(base, max, s.failures);
                s.due = after(wait);
                if let Some(exp) = s.expires {
                    // Looked at again when the response stapled expires, at the latest.
                    s.due = s.due.min(instant_of(exp));
                }
                report = Some(format!(
                    "{}: {e}; trying again in {}s",
                    entry.subject,
                    wait.as_secs()
                ));
            }
            Ok(Fetched {
                status: RevocationStatus::Good,
                raw,
                this_update,
                next_update,
            }) => match entry.base.clone().with_ocsp_response(raw.clone()) {
                Ok(stapled) => {
                    s.failures = 0;
                    let mut m = (*self.inner.stapled.get()).clone();
                    m.insert(id, (stapled, raw.into()));
                    self.inner.stapled.set(m);
                    s.expires = next_update;
                    s.due = match next_update {
                        None => after(OCSP_REFRESH),
                        Some(next) => {
                            let half = this_update
                                .saturating_add(next.saturating_sub(this_update).max(0) / 2);
                            let at = instant_of(half);
                            if at > now {
                                at
                            } else {
                                let left = instant_of(next)
                                    .saturating_duration_since(now)
                                    .checked_div(2)
                                    .unwrap_or_default();
                                after(left.min(OCSP_REFRESH))
                            }
                        }
                    };
                }
                Err(e) => {
                    s.failures = s.failures.saturating_add(1);
                    s.due = after(backoff(base, max, s.failures));
                    report = Some(format!("{}: the response: {e}", entry.subject));
                }
            },
            Ok(f) => {
                // Revoked, or unknown to the responder: nothing to staple.
                s.failures = 0;
                s.expires = None;
                unstaple(&self.inner, &id);
                s.due = after(OCSP_REFRESH);
                if let RevocationStatus::Revoked { time, .. } = f.status {
                    report = Some(format!("{}: revoked at {time}", entry.subject));
                    revoked.clone_from(&entry.revoked);
                } else {
                    report = Some(format!("{}: unknown to the responder", entry.subject));
                }
            }
        }
        // A response is not stapled past its validity, whatever the fetches since.
        if s.expires.is_some_and(|exp| unix_now() >= exp) {
            s.expires = None;
            unstaple(&self.inner, &id);
        }
        drop(st);
        self.inner.wake.notify_all();
        if let Some(line) = report {
            self.log(&line);
        }
        if let Some(f) = revoked {
            f();
        }
    }

    /// Checks that a certificate from files chains to the trusted roots, for any use.
    fn verify(&self, e: &Entry) -> Result<(), Failure> {
        let trust = lock(&self.inner.trust).clone();
        let Some((roots, _)) = trust else {
            return Err(Failure::Unverified(
                "a certificate from files, and no roots its chain must verify against are \
                 trusted (the system's are not yet: akio API request A57)"
                    .into(),
            ));
        };
        Verifier::new(&roots)
            .usage(Usage::Any)
            .verify(&e.leaf, &e.intermediates, None)
            .map(drop)
            .map_err(|err| {
                Failure::Unverified(format!(
                    "its chain does not verify against the trusted roots ({err}), and its \
                     responder is not asked"
                ))
            })
    }

    /// The client of the responders, made the first time.
    fn client(&self) -> Result<Arc<Fetcher>, String> {
        let mut c = lock(&self.inner.client);
        if let Some(f) = c.as_ref() {
            return Ok(Arc::clone(f));
        }
        if self.inner.stop.load(Ordering::Acquire) {
            return Err("stopped".into());
        }
        let f = Arc::new(Fetcher::new(Arc::clone(&self.inner.stop))?);
        *c = Some(Arc::clone(&f));
        Ok(f)
    }

    /// Fetches and checks `e`'s response.
    fn fetch(&self, e: &Entry, from_files: bool) -> Result<Fetched, Failure> {
        let body = OcspRequest::new(&e.leaf, &e.issuer)
            .map_err(|err| Failure::Fetch(format!("the request: {err}")))?
            .to_der();
        let mut req = if from_files {
            let private = lock(&self.inner.trust).as_ref().is_some_and(|t| t.1);
            let (url, host) = pinned(&e.url, private).map_err(Failure::Fetch)?;
            ClientRequest::post(&url, body).header("host", &host)
        } else {
            ClientRequest::post(&e.url, body)
        };
        req = req.header("content-type", "application/ocsp-request");
        let client = self.client().map_err(Failure::Fetch)?;
        let raw = client.send(req, &e.url).map_err(Failure::Fetch)?;
        let resp = OcspResponse::from_der(raw.clone())
            .map_err(|err| Failure::Fetch(format!("the response: {err}")))?;
        let status = resp
            .verify(&e.leaf, &e.issuer, &OcspValidation::new())
            .map_err(|err| Failure::Fetch(format!("the response: {err}")))?;
        let single = resp
            .basic()
            .and_then(|b| {
                b.responses()
                    .iter()
                    .find(|s| s.cert_id.matches(&e.leaf, &e.issuer))
            })
            .ok_or_else(|| Failure::Fetch("the response is not about the certificate".into()))?;
        Ok(Fetched {
            status,
            raw,
            this_update: single.this_update,
            next_update: single.next_update,
        })
    }
}

/// Why a fetch got no response.
enum Failure {
    /// A certificate from files whose responder is not asked.
    Unverified(String),
    Fetch(String),
}

/// A checked response.
struct Fetched {
    status: RevocationStatus,
    raw: Vec<u8>,
    this_update: i64,
    next_update: Option<i64>,
}

/// How a certificate is named in what is reported: its first DNS name, or its subject's common
/// name.
fn subject(leaf: &Certificate) -> String {
    leaf.subject_alt_names()
        .iter()
        .find_map(|n| match n {
            akio_x509::GeneralName::Dns(d) => Some(d.clone()),
            _ => None,
        })
        .or_else(|| leaf.subject().common_name())
        .unwrap_or_default()
}

/// The responder URL `url` of a certificate from files with its host resolved to an address it
/// may be dialled at, and the `Host` it is asked with: so that a name cannot point somewhere
/// else once checked. Resolving blocks: called on a worker of the control plane.
fn pinned(url: &str, private: bool) -> Result<(String, String), String> {
    let rest = url.strip_prefix("http://").ok_or_else(|| {
        format!("the responder {url} is not http, which a certificate from files is asked over")
    })?;
    let (authority, path) = match rest.find(['/', '?']) {
        Some(i) => rest.split_at(i),
        None => (rest, "/"),
    };
    let path = if path.starts_with('?') {
        format!("/{path}")
    } else {
        path.to_owned()
    };
    if authority.is_empty() || authority.contains('@') {
        return Err(format!("the responder {url} has no host"));
    }
    let with_port = if authority.ends_with(']') || !authority.contains(':') {
        format!("{authority}:80")
    } else {
        authority.to_owned()
    };
    let addr: SocketAddr = with_port
        .to_socket_addrs()
        .map_err(|e| format!("the responder {url}: {e}"))?
        .next()
        .ok_or_else(|| format!("the responder {url} resolves to no address"))?;
    if !private && !public_addr(addr.ip()) {
        return Err(format!(
            "the responder's address {} is not public",
            addr.ip()
        ));
    }
    Ok((format!("http://{addr}{path}"), authority.to_owned()))
}

/// The client of the responders: akio-http's, on an engine of its own with one thread, whose
/// callbacks only hand the response over to the worker waiting for it.
struct Fetcher {
    engine: Arc<Engine>,
    client: Client,
    stop: Arc<AtomicBool>,
}

impl Fetcher {
    fn new(stop: Arc<AtomicBool>) -> Result<Fetcher, String> {
        let mut cfg = akio::Config::default();
        cfg.shutdown_timeout = Duration::from_secs(1);
        let engine = Engine::builder()
            .client_threads(1)
            .config(cfg)
            .build()
            .map_err(|e| format!("the OCSP client's engine: {e}"))?;
        engine
            .start()
            .map_err(|e| format!("the OCSP client's engine: {e}"))?;
        let engine = Arc::new(engine);
        let mut opts = ClientOptions::default();
        opts.max_body_bytes = OCSP_MAX_BYTES;
        opts.max_idle_per_host = 2;
        opts.max_idle = 8;
        let client = Client::new(Arc::clone(&engine))
            .with_options(opts)
            .with_redirects(RedirectPolicy::none())
            .with_timeout(OCSP_TIMEOUT);
        Ok(Fetcher {
            engine,
            client,
            stop,
        })
    }

    fn close(&self) {
        self.engine.shutdown();
        self.engine.wait();
    }

    /// Sends `req` and waits for a 200's body, at most [`OCSP_TIMEOUT`].
    fn send(&self, req: ClientRequest, url: &str) -> Result<Vec<u8>, String> {
        let deadline = after(OCSP_TIMEOUT);
        let (tx, rx) = mpsc::sync_channel(1);
        // On an I/O thread: hands the outcome over, never blocks (the channel has room for it).
        self.client
            .request(req.timeout(OCSP_TIMEOUT).header("user-agent", "akit"), {
                move |r| {
                    let _ = tx.try_send(r);
                }
            });
        loop {
            if self.stop.load(Ordering::Acquire) {
                return Err("stopped".into());
            }
            match rx.recv_timeout(SLICE) {
                Ok(Ok(r)) if r.status == 200 => return Ok(r.body),
                Ok(Ok(r)) => return Err(format!("{url}: the responder answered {}", r.status)),
                Ok(Err(e)) => return Err(format!("{url}: {e}")),
                Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                    return Err(format!("{url}: no answer in {}s", OCSP_TIMEOUT.as_secs()));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(format!("{url}: the request was dropped"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_addresses_as_fit() {
        for (a, public) in [
            ("8.8.8.8", true),
            ("2606:4700::1111", true),
            ("::ffff:8.8.8.8", true),
            ("64:ff9b::808:808", true),
            ("127.0.0.1", false),
            ("10.1.2.3", false),
            ("172.16.0.1", false),
            ("192.168.1.1", false),
            ("169.254.169.254", false),
            ("100.64.0.1", false),
            ("0.0.0.0", false),
            ("224.0.0.1", false),
            ("::1", false),
            ("::", false),
            ("fe80::1", false),
            ("fc00::1", false),
            ("ff02::1", false),
            ("::ffff:127.0.0.1", false),
            ("64:ff9b::7f00:1", false),
            ("64:ff9b::a9fe:a9fe", false),
        ] {
            let ip: IpAddr = a.parse().unwrap_or_else(|_| IpAddr::from([0, 0, 0, 0]));
            assert_eq!(public_addr(ip), public, "{a}");
        }
    }

    #[test]
    fn pinned_responders() {
        let (u, h) = pinned("http://127.0.0.1:8080/ocsp", true).unwrap_or_default();
        assert_eq!(
            (u.as_str(), h.as_str()),
            ("http://127.0.0.1:8080/ocsp", "127.0.0.1:8080")
        );
        let (u, h) = pinned("http://127.0.0.1", true).unwrap_or_default();
        assert_eq!(
            (u.as_str(), h.as_str()),
            ("http://127.0.0.1:80/", "127.0.0.1")
        );
        let (u, _) = pinned("http://[::1]:81?x", true).unwrap_or_default();
        assert_eq!(u, "http://[::1]:81/?x");
        for (url, why) in [
            ("http://127.0.0.1/", "is not public"),
            ("https://ocsp.example/", "is not http"),
            ("http:///x", "has no host"),
            ("http://u@ocsp.example/", "has no host"),
        ] {
            let e = pinned(url, false).err().unwrap_or_default();
            assert!(e.contains(why), "{url}: {e}");
        }
    }
}
