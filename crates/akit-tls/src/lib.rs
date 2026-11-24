#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's certificate management: files, ACME, storage, OCSP stapling, on-demand TLS.
//!
//! So far (I09, I30): a listener's `tls` from files, as fit's `tlsmgr.LoadStatic` and
//! `LoadClientAuth` load it, made into the akio-tls [`ServerConfig`] the listener serves, its
//! certificates chosen per handshake by a certificate manager, and a storage with locks for
//! what ACME (I31) will keep:
//!
//! - **The manager** is akio-tls's [`CertResolver`] trait (what fit's `tlsmgr.Manager` is to Go's
//!   `GetCertificate`; akio ADR-098): asked for the certificate of every full handshake, it
//!   answers from memory in the call, or later from another thread (on-demand TLS, I34). [`Certs`]
//!   is the manager of certificates from files (fit's `tlsmgr.Static`), and
//!   [`ListenerTls::server_configs`] builds a listener's configuration around any manager.
//! - **Certificates** from PEM files (a chain and its key, each at most [`MAX_CERT_FILE_BYTES`]),
//!   chosen by the name the client asks for (SNI), as fit chooses them: the first certificate for
//!   that name, exactly or by a `*.` wildcard one label deep, that the client can take (its
//!   signature schemes and, in TLS 1.2, cipher suites); the first for the name if it can take
//!   none; the first of all when none is for the name or the client names none.
//! - **Reloads without dropping handshakes**: a listener's configuration asks a manager per
//!   handshake, so akit-runtime gives it one that reads the current graph's certificates. A reload
//!   that changes only the files' contents keeps the listener, its sockets and its connections,
//!   and the next handshake gets the new certificate; a handshake under way keeps the one it was
//!   given. [`FileWatcher`] has a reload made when the files change (`run -watch-certs`).
//! - **Storage** ([`Storage`], [`FileStorage`]): keys and values, and locks shared by instances
//!   on the same directory, as fit's `tlsmgr.FileStorage`; it blocks, so it is called off the I/O
//!   threads.
//! - **ALPN** from the listener's protocols: `h2` and `http/1.1`, in fit's order.
//! - **Client authentication** (`client_auth`): the authorities of its PEM files, a certificate
//!   required or verified if given.
//! - **Versions and cipher suites** as fit's: TLS 1.2 and 1.3, with only ECDHE and AEAD suites
//!   (AES-GCM, ChaCha20-Poly1305) for 1.2: akio-tls has no other.
//! - Session resumption by tickets, on as in Go's TLS servers, and by akio's session cache.
//! - **HTTP/3** (I10): a listener serving `h3` gets a second configuration from the same files
//!   ([`ServerConfigs::h3`]), with ALPN `h3` and TLS 1.3 alone, for its QUIC connections.
//!
//! - **ACME** (I31, [`Acme`]): certificates for the names a listener lists in `tls.acme`,
//!   obtained from an ACME CA through HTTP-01 or TLS-ALPN-01 (I32) and kept in the storage. A
//!   client asking for one of them is given its certificate once there is one
//!   ([`ListenerTls::get`]); until then, and for other names, the certificates from files. A
//!   listener that lists names offers [`ACME_TLS_ALPN`] last in ALPN, so that only a client
//!   offering it alone, as a CA checking a TLS-ALPN-01 challenge does, negotiates it, and gets the
//!   challenge's certificate.
//! - **OCSP stapling** (I32, [`Ocsp`]): the responses of the certificates ACME obtains, and of
//!   the certificates from files that name a responder, fetched off the I/O threads and stapled
//!   (see [`ocsp`](crate::ocsp) for which files' are).
//!
//! - **On-demand TLS** (I34, [`Acme::on_demand`]): a listener whose `tls.on_demand` is set has a
//!   certificate obtained during the handshake for a name neither its files nor its `tls.acme`
//!   cover, when `acme.on_demand.ask` allows it; the handshake waits for it through akio-tls's
//!   `CertDone`, without holding its I/O thread, at most `acme.on_demand.timeout`.
//!
//! - **Metrics** (I37): what each certificate is for and when it expires ([`CertificateInfo`]),
//!   for `fit_tls_certificate_not_after_seconds`.
//! - **Upstream TLS** (I54, [`UpstreamTls`]): an `h2` upstream's `tls_ca_file`, and its client
//!   certificate (`tls_cert_file`, `tls_key_file`), for its pool's connections and probes.
//!
//! Loading reads files, which blocks: this is control plane code, run when a config is compiled,
//! never on an I/O thread (rules.md rule 1).
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

mod acme;
mod certs;
pub mod ocsp;
mod published;
mod storage;
mod upstream;
mod watch;

pub use acme::{
    ACME_CHALLENGE_PREFIX, ACME_TLS_ALPN, ASK_BURST, ASK_EVERY, Acme, DEFAULT_RETRY_BASE,
    DEFAULT_RETRY_MAX, Log, MAX_ASKS, MAX_NAMES, MAX_REFUSED, MAX_WAITERS, OnDemandError,
    REFUSED_FOR, Waiter,
};
pub use ocsp::{Ocsp, Staple};

pub use certs::{Certs, Sum};
pub use storage::{
    DEFAULT_LOCK_POLL, DEFAULT_LOCK_REFRESH, DEFAULT_LOCK_STALE, FileStorage, MAX_HELD_LOCKS,
    MAX_VALUE_BYTES, Storage, StorageError,
};
pub use upstream::{UPSTREAM_ALPN, UpstreamTls};
pub use watch::{DEFAULT_WATCH_DEBOUNCE, DEFAULT_WATCH_INTERVAL, FileWatcher, Report};

use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use std::sync::Arc;

use akio::tls::{
    CertAlert, CertDone, CertResolver, Certificate, CertifiedKey, ClientHelloInfo, PrivateKey,
    RootCerts, ServerConfig,
};
use akio_crypto::hash::Sha256;
use akit_config::{ClientAuthMode, Protocol, Tls};

/// The largest certificate, key or authority file read, as fit's `tlsmgr.MaxCertFileBytes`.
pub const MAX_CERT_FILE_BYTES: u64 = 1 << 20;

/// What went wrong loading a listener's `tls`, and where: `path` is relative to the `tls`
/// object, as `certificates[1]` or `client_auth.ca_files[0]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// The path of what failed, under `tls` (or, for [`UpstreamTls`], under the upstream).
    pub path: String,
    /// What is wrong; starts with [`akit_config::NOT_SUPPORTED_YET`] for what fit serves and akit not yet.
    pub msg: String,
}

impl Error {
    fn new(path: impl Into<String>, msg: impl Into<String>) -> Error {
        Error {
            path: path.into(),
            msg: msg.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.msg)
    }
}

impl std::error::Error for Error {}

/// The ALPN protocols a TLS listener serving `protocols` offers, as fit's
/// `tlsmgr.ServerConfig`: `h2` then `http/1.1` for both, `h2` alone, or `http/1.1`.
#[must_use]
pub fn alpn(protocols: &[Protocol]) -> Vec<&'static str> {
    let h1 = protocols.contains(&Protocol::Http11);
    let h2 = protocols.contains(&Protocol::H2);
    match (h1, h2) {
        (true, true) => vec!["h2", "http/1.1"],
        (false, true) => vec!["h2"],
        _ => vec!["http/1.1"],
    }
}

/// A TLS listener's configurations, from one read of its files: the same certificates, chosen by
/// SNI the same way, and the same client authentication over TCP and over QUIC.
#[derive(Debug, Clone)]
pub struct ServerConfigs {
    /// What its TCP connections serve: ALPN from its protocols ([`alpn`]), TLS 1.2 and 1.3.
    pub tcp: ServerConfig,
    /// What its HTTP/3 connections serve, for a listener whose protocols include `h3`: TLS 1.3
    /// alone, which QUIC is built on, and ALPN `h3` (fib's `http3.ConfigureTLS` keeps TCP's
    /// protocols after it, which no HTTP/3 client asks for).
    pub h3: Option<ServerConfig>,
}

/// A certificate a listener serves, as fit's metrics name it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CertificateInfo {
    /// Its first DNS name, or its subject's common name when it has none (fit's choice).
    pub name: String,
    /// Its serial number in lower-case hexadecimal, as Go's `big.Int.Text(16)` writes it.
    pub serial: String,
    /// When it expires, in seconds since the Unix epoch.
    pub not_after: i64,
}

impl CertificateInfo {
    /// What the metrics say of `key`'s leaf; `None` when it does not parse (akio-tls parsed it
    /// when the key was made, so not expected). Parses the certificate: control plane only.
    #[must_use]
    pub fn of(key: &CertifiedKey) -> Option<CertificateInfo> {
        let c = akio_x509::Certificate::from_der(key.chain_der().next()?.to_vec()).ok()?;
        let name = c
            .subject_alt_names()
            .iter()
            .find_map(|n| match n {
                akio_x509::GeneralName::Dns(d) => Some(d.clone()),
                _ => None,
            })
            .or_else(|| c.subject().common_name())
            .unwrap_or_default();
        Some(CertificateInfo {
            name,
            serial: serial_hex(c.serial()),
            not_after: c.not_after(),
        })
    }
}

/// A serial number's INTEGER contents (two's complement, big-endian) as Go's `Text(16)` writes
/// the number: lower-case hexadecimal without leading zeros, `-` before a negative one.
fn serial_hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    let negative = b.first().is_some_and(|x| x & 0x80 != 0);
    let mut mag: Vec<u8> = b.to_vec();
    if negative {
        // The magnitude: the two's complement of the bytes.
        let mut carry = true;
        for x in mag.iter_mut().rev() {
            let (v, c) = (!*x).overflowing_add(u8::from(carry));
            *x = v;
            carry = c;
        }
    }
    let mut hex = String::new();
    for x in &mag {
        let _ = write!(hex, "{x:02x}");
    }
    let digits = hex.trim_start_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    if negative {
        format!("-{digits}")
    } else {
        digits.to_owned()
    }
}

/// A listener's `tls`, loaded from its files once: its certificates, the manager of
/// certificates from files ([`Certs`]); the authorities of its client authentication; and the
/// protocols it serves. Cheap to clone.
#[derive(Clone)]
pub struct ListenerTls {
    certs: Arc<Certs>,
    /// The OCSP response kept for each of `certs`, in their order, when one is (I32); empty
    /// when none is.
    staples: Arc<[Option<Staple>]>,
    /// The names listed in `tls.acme`, normalized, with the manager that obtains their
    /// certificates; `None` when it lists none, or when none obtains them (`akit validate`).
    acme: Option<ListenerAcme>,
    roots: Option<(RootCerts, ClientAuthMode)>,
    alpn: Vec<&'static str>,
    h3: bool,
    /// The SHA-256 of every file read, certificates, keys and authorities, by path.
    files: BTreeMap<String, Sum>,
    /// The SHA-256 of the authority files, in order: what a configuration holds itself.
    ca_sums: Vec<Sum>,
}

/// A listener's ACME names and their manager; and, with `tls.on_demand` and
/// `acme.on_demand`, how long a handshake waits for a name taken on.
#[derive(Clone)]
struct ListenerAcme {
    acme: Acme,
    names: Arc<[String]>,
    on_demand: Option<std::time::Duration>,
}

/// The handshake time added to `acme.on_demand.timeout` on a listener taking names on: akio's
/// default handshake timeout.
const ON_DEMAND_HANDSHAKE_EXTRA: std::time::Duration = std::time::Duration::from_secs(10);

/// What a handshake gets: a certificate (or none), or a name to take on demand.
enum Pick {
    Key(Option<CertifiedKey>),
    OnDemand(String),
}

/// A handshake waiting for a certificate on demand: akio-tls's answer.
struct Handshake(CertDone);

impl Waiter for Handshake {
    fn answer(self: Box<Self>, r: Result<CertifiedKey, OnDemandError>) {
        match r {
            Ok(k) => self.0.complete(k),
            Err(OnDemandError::Refused | OnDemandError::BadName | OnDemandError::Off) => {
                self.0.fail(CertAlert::UnrecognizedName);
            }
            Err(_) => self.0.fail(CertAlert::InternalError),
        }
    }

    fn abandoned(&self) -> bool {
        self.0.is_abandoned()
    }
}

impl fmt::Debug for ListenerTls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ListenerTls")
            .field("certs", &self.certs)
            .field("acme", &self.acme.as_ref().map(|a| &a.names))
            .field("on_demand", &self.acme.as_ref().and_then(|a| a.on_demand))
            .field("client_auth", &self.roots.as_ref().map(|(_, m)| m))
            .field("alpn", &self.alpn)
            .field("h3", &self.h3)
            .finish_non_exhaustive()
    }
}

impl ListenerTls {
    /// Loads `tls` from its files, for a listener serving `protocols`, and checks that a
    /// configuration can be built from it.
    ///
    /// # Errors
    ///
    /// The first problem found: a file that cannot be read or is too large, PEM with no
    /// certificate or key, a key that does not match its certificate, or an authority file with
    /// no certificate.
    pub fn load(tls: &Tls, protocols: &[Protocol]) -> Result<ListenerTls, Error> {
        ListenerTls::load_with(tls, protocols, None)
    }

    /// The same, the certificates of the names `tls.acme` lists obtained by `acme`, and the
    /// OCSP responses of the certificates from files kept by its [`Ocsp`]; with none, only the
    /// certificates from files are served, as when `akit validate` checks a config.
    ///
    /// # Errors
    ///
    /// As [`load`](Self::load).
    pub fn load_with(
        tls: &Tls,
        protocols: &[Protocol],
        acme: Option<&Acme>,
    ) -> Result<ListenerTls, Error> {
        let mut files = BTreeMap::new();
        let mut keys = Vec::with_capacity(tls.certificates.len());
        for (i, c) in tls.certificates.iter().enumerate() {
            let at = format!("certificates[{i}]");
            keys.push(
                load_pair(&c.cert_file, &c.key_file, &mut files)
                    .map_err(|msg| Error::new(&at, msg))?,
            );
        }
        if keys.is_empty() && tls.acme.is_empty() && !tls.on_demand {
            // akit-config gives such a tls its routes' host names (auto_https, I33), or refuses
            // it: only a tls built by hand gets here.
            return Err(Error::new(
                "",
                "a certificate, a name for acme or on_demand is needed",
            ));
        }
        let certs = Certs::new(keys)
            .map_err(|(i, msg)| Error::new(format!("certificates[{i}]"), msg))?
            .with_files(files.clone());
        let mut roots = None;
        let mut ca_sums = Vec::new();
        if let Some(ca) = &tls.client_auth {
            let mut r = RootCerts::empty();
            for (i, path) in ca.ca_files.iter().enumerate() {
                let at = format!("client_auth.ca_files[{i}]");
                let pem = read(path).map_err(|msg| Error::new(&at, msg))?;
                let sum = Sha256::digest(&pem);
                files.insert(path.clone(), sum);
                ca_sums.push(sum);
                match r.add_pem(&pem) {
                    Ok(0) => {
                        return Err(Error::new(at, format!("{path}: no PEM certificate found")));
                    }
                    Ok(_) => {}
                    Err(e) => return Err(Error::new(at, format!("{path}: {e}"))),
                }
            }
            roots = Some((r, ca.mode));
        }
        let ocsp = acme.map(|a| a.ocsp().clone());
        let on_demand = acme
            .filter(|_| tls.on_demand)
            .and_then(Acme::on_demand_timeout);
        let acme = match acme {
            Some(a) if !tls.acme.is_empty() || on_demand.is_some() => Some(ListenerAcme {
                acme: a.clone(),
                names: tls
                    .acme
                    .iter()
                    .map(|n| n.trim_end_matches('.').to_ascii_lowercase())
                    .collect(),
                on_demand,
            }),
            _ => None,
        };
        let mut alpn = alpn(protocols);
        // Last, so that only a client offering it alone, as a CA checking a TLS-ALPN-01
        // challenge does, negotiates it (fit's).
        if (!tls.acme.is_empty() || tls.on_demand)
            && acme.as_ref().is_none_or(|a| a.acme.answers_tls_alpn())
        {
            alpn.push(ACME_TLS_ALPN);
        }
        let t = ListenerTls {
            certs: Arc::new(certs),
            staples: Arc::new([]),
            acme,
            roots,
            alpn,
            h3: protocols.contains(&Protocol::H3),
            files,
            ca_sums,
        };
        // Built once here, so that `validate` finds what would fail when the listener opens.
        t.server_configs(Arc::new(t.clone()))?;
        Ok(match ocsp {
            Some(o) => t.with_ocsp(&o),
            None => t,
        })
    }

    /// The same, the OCSP responses of its certificates from files kept by `ocsp` (fit's
    /// `Static.Staple`): those that name a responder.
    #[must_use]
    pub fn with_ocsp(mut self, ocsp: &Ocsp) -> ListenerTls {
        let staples: Vec<Option<Staple>> = self
            .certs
            .certificates()
            .map(|c| ocsp.add_from_files(c))
            .collect();
        if staples.iter().any(Option::is_some) {
            self.staples = staples.into();
        }
        self
    }

    /// Its certificates from files.
    #[must_use]
    pub fn certs(&self) -> &Arc<Certs> {
        &self.certs
    }

    /// The OCSP response kept for its `i`th certificate from files, when one is.
    #[must_use]
    pub fn staple(&self, i: usize) -> Option<&Staple> {
        self.staples.get(i).and_then(Option::as_ref)
    }

    /// The names its `tls.acme` lists, normalized, when a manager obtains their certificates.
    #[must_use]
    pub fn acme_names(&self) -> &[String] {
        self.acme.as_ref().map_or(&[], |a| &a.names)
    }

    /// The certificate a handshake gets, as fit's `tlsmgr.Certs`: for a name `tls.acme` lists,
    /// the one obtained for it once there is one (a client naming none, on a listener with no
    /// files, gets the first name's); otherwise the certificates from files ([`Certs::get`]).
    /// Read from memory, in the handshake's call. `None` when there is none to give.
    ///
    /// A handshake offering [`ACME_TLS_ALPN`] alone is a CA checking a TLS-ALPN-01 challenge: it
    /// gets the challenge's certificate, for a name this listener lists, or nothing.
    #[must_use]
    pub fn get(&self, hello: &ClientHelloInfo<'_>) -> Option<CertifiedKey> {
        self.get_hello(&certs::Hello::from(hello))
    }

    pub(crate) fn get_hello(&self, hello: &certs::Hello<'_>) -> Option<CertifiedKey> {
        match self.pick(hello) {
            Pick::Key(k) => k,
            Pick::OnDemand(_) => None,
        }
    }

    /// Answers `done` with the certificate a handshake gets, as [`get`](Self::get), but for a
    /// name it takes on demand (`tls.on_demand`, fit's `Certs.GetCertificate`): one neither its
    /// `tls.acme` lists nor its files have, which [`Acme::on_demand`] answers, now or later.
    /// Never blocks.
    pub fn resolve(&self, hello: &ClientHelloInfo<'_>, done: CertDone) {
        match self.pick(&certs::Hello::from(hello)) {
            Pick::Key(Some(k)) => done.complete(k),
            Pick::Key(None) => done.fail(CertAlert::UnrecognizedName),
            Pick::OnDemand(name) => match &self.acme {
                Some(a) => a.acme.on_demand(&name, Box::new(Handshake(done))),
                None => done.fail(CertAlert::UnrecognizedName),
            },
        }
    }

    fn pick(&self, hello: &certs::Hello<'_>) -> Pick {
        let Some(a) = &self.acme else {
            return Pick::Key(self.files_cert(hello));
        };
        if let [p] = hello.alpn {
            if *p == ACME_TLS_ALPN.as_bytes() {
                let Some(name) = hello
                    .name
                    .map(|n| n.trim_end_matches('.').to_ascii_lowercase())
                else {
                    return Pick::Key(None);
                };
                if !a.names.contains(&name) && a.on_demand.is_none() {
                    return Pick::Key(None);
                }
                return Pick::Key(a.acme.alpn_challenge(&name));
            }
        }
        {
            let name = hello
                .name
                .map(|n| n.trim_end_matches('.').to_ascii_lowercase());
            let name = match name {
                Some(n) if !n.is_empty() => Some(n),
                _ if self.certs.is_empty() => a.names.first().cloned(),
                _ => None,
            };
            if let Some(n) = name.filter(|n| a.names.contains(n)) {
                if let Some(k) = a.acme.certificate(&n) {
                    return Pick::Key(Some(k));
                }
            }
        }
        if a.on_demand.is_some() {
            if let Some(n) = hello.name.filter(|n| !n.is_empty() && !self.certs.has(n)) {
                return Pick::OnDemand(n.to_owned());
            }
        }
        Pick::Key(self.files_cert(hello))
    }

    /// The certificate from files a handshake gets, with its OCSP response.
    fn files_cert(&self, hello: &certs::Hello<'_>) -> Option<CertifiedKey> {
        let i = self.certs.choose(hello)?;
        if self.staples.is_empty() {
            return self.certs.key(i).cloned();
        }
        match self.staples.get(i) {
            Some(Some(s)) => Some(s.certificate()),
            _ => self.certs.key(i).cloned(),
        }
    }

    /// The SHA-256 of every file it was loaded from (certificates, keys and authorities), by
    /// path: what [`FileWatcher`] compares the files with.
    #[must_use]
    pub fn files(&self) -> &BTreeMap<String, Sum> {
        &self.files
    }

    /// Whether a configuration built from `other` would be this one's, but for its certificates:
    /// the same protocols and the same client authentication, its authority files' contents
    /// included. A listener whose `tls` is the same and whose loaded TLS is `same_config` is
    /// kept by a reload, and serves the new certificates from its manager.
    #[must_use]
    pub fn same_config(&self, other: &ListenerTls) -> bool {
        self.alpn == other.alpn
            && self.h3 == other.h3
            && self.acme.as_ref().and_then(|a| a.on_demand)
                == other.acme.as_ref().and_then(|a| a.on_demand)
            && self.roots.as_ref().map(|(_, m)| m) == other.roots.as_ref().map(|(_, m)| m)
            && self.ca_sums == other.ca_sums
    }

    /// The configurations the listener serves, its certificates chosen per handshake by
    /// `manager`: over TCP, and over QUIC when it serves `h3`.
    ///
    /// # Errors
    ///
    /// When akio-tls refuses the configuration.
    pub fn server_configs(&self, manager: Arc<dyn CertResolver>) -> Result<ServerConfigs, Error> {
        let build = |alpn: Vec<&str>, tls13_only: bool| {
            let mut b = ServerConfig::builder_with_resolver(manager.clone())
                .alpn(alpn)
                .session_tickets(true);
            if let Some(t) = self.acme.as_ref().and_then(|a| a.on_demand) {
                // A handshake waits for its certificate on demand (akio's ADR-098): both
                // timeouts raised, as akio-tls asks.
                b = b
                    .cert_resolver_timeout(t)
                    .handshake_timeout(Some(t.saturating_add(ON_DEMAND_HANDSHAKE_EXTRA)));
            }
            if tls13_only {
                b = b.tls13_only(true);
            }
            if let Some((roots, mode)) = &self.roots {
                b = match mode {
                    ClientAuthMode::Require => b.client_auth_required(roots.clone()),
                    ClientAuthMode::VerifyIfGiven => b.client_auth_optional(roots.clone()),
                };
            }
            // max_version 1.2 is refused by akit-config (akio API request A9): 1.3 is the
            // default.
            b.build().map_err(|e| Error::new("", e.to_string()))
        };
        let tcp = build(self.alpn.clone(), false)?;
        // QUIC is TLS 1.3 alone, whatever max_version caps TCP at (fit's http3Config).
        let h3 = if self.h3 {
            Some(build(vec!["h3"], true)?)
        } else {
            None
        };
        Ok(ServerConfigs { tcp, h3 })
    }
}

/// Loads `tls` from its files into the configuration a listener serving `protocols` serves over
/// TCP: [`server_configs`]'s `tcp`.
///
/// # Errors
///
/// As [`ListenerTls::load`].
pub fn server_config(tls: &Tls, protocols: &[Protocol]) -> Result<ServerConfig, Error> {
    server_configs(tls, protocols).map(|c| c.tcp)
}

/// Loads `tls` from its files, once, into the configurations a listener serving `protocols`
/// serves, its certificates as loaded: over TCP, and over QUIC when it serves `h3`.
///
/// # Errors
///
/// As [`ListenerTls::load`].
pub fn server_configs(tls: &Tls, protocols: &[Protocol]) -> Result<ServerConfigs, Error> {
    let t = ListenerTls::load(tls, protocols)?;
    t.server_configs(Arc::new(t.clone()))
}

/// A listener's TLS chooses its certificates itself: [`ListenerTls::get`], in the handshake's
/// call.
impl CertResolver for ListenerTls {
    fn resolve(&self, hello: &ClientHelloInfo<'_>, done: CertDone) {
        ListenerTls::resolve(self, hello, done);
    }
}

/// A certificate's chain and its key, checked to match; the files' sums go into `files`.
fn load_pair(
    cert_file: &str,
    key_file: &str,
    files: &mut BTreeMap<String, Sum>,
) -> Result<CertifiedKey, String> {
    let chain_pem = read(cert_file)?;
    let key_pem = read(key_file)?;
    let chain = Certificate::from_pem(&chain_pem).map_err(|e| format!("{cert_file}: {e}"))?;
    if chain.is_empty() {
        return Err(format!("{cert_file}: no PEM certificate found"));
    }
    let key = PrivateKey::from_pem(&key_pem).map_err(|e| format!("{key_file}: {e}"))?;
    let ck = CertifiedKey::new(chain, key).map_err(|e| format!("{cert_file}, {key_file}: {e}"))?;
    files.insert(cert_file.to_owned(), Sha256::digest(&chain_pem));
    files.insert(key_file.to_owned(), Sha256::digest(&key_pem));
    Ok(ck)
}

/// Reads the file at `path`, refusing one larger than [`MAX_CERT_FILE_BYTES`].
pub(crate) fn read(path: &str) -> Result<Vec<u8>, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("open {path}: {e}"))?;
    let mut data = Vec::new();
    f.take(MAX_CERT_FILE_BYTES.saturating_add(1))
        .read_to_end(&mut data)
        .map_err(|e| format!("read {path}: {e}"))?;
    if u64::try_from(data.len()).unwrap_or(u64::MAX) > MAX_CERT_FILE_BYTES {
        return Err(format!("{path}: larger than {MAX_CERT_FILE_BYTES} bytes"));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic)]
    use super::*;

    #[test]
    fn serials_as_go_writes_them() {
        assert_eq!(serial_hex(&[0x00, 0x8f, 0x01]), "8f01");
        assert_eq!(serial_hex(&[0x01, 0x00]), "100");
        assert_eq!(serial_hex(&[0x00]), "0");
        assert_eq!(serial_hex(&[]), "0");
        assert_eq!(serial_hex(&[0xff]), "-1");
        assert_eq!(serial_hex(&[0x80, 0x00]), "-8000");
    }

    fn acme_tls(challenges: Vec<akit_config::Challenge>) -> (Acme, ListenerTls) {
        let a = Acme::with_storage(
            &akit_config::Acme {
                directory: "https://127.0.0.1:1/dir".into(),
                challenges,
                ..akit_config::Acme::default()
            },
            Arc::new(FileStorage::new(
                std::env::temp_dir().join("akit-tls-unused"),
            )),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let tls = Tls {
            certificates: Vec::new(),
            client_auth: None,
            max_version: akit_config::TlsVersion::Tls13,
            acme: vec!["A.akit.test".into()],
            on_demand: false,
        };
        let t = ListenerTls::load_with(&tls, &[Protocol::Http11, Protocol::H2], Some(&a))
            .unwrap_or_else(|e| panic!("{e}"));
        (a, t)
    }

    /// fit's `TestCertsALPN`: a handshake offering acme-tls/1 alone gets the challenge's
    /// certificate of a name the listener lists, or nothing; one offering other protocols too is
    /// no CA checking a challenge.
    #[test]
    fn tls_alpn_challenges_as_fit() {
        use akit_config::Challenge;
        let (a, t) = acme_tls(vec![Challenge::Http01, Challenge::TlsAlpn01]);
        // Offered last, so that only a client offering it alone negotiates it.
        assert_eq!(t.alpn, ["h2", "http/1.1", ACME_TLS_ALPN]);
        let acme_only: &[&[u8]] = &[ACME_TLS_ALPN.as_bytes()];
        let hello = |name, alpn| certs::Hello {
            name,
            schemes: &[0x0403],
            suites: &[],
            tls13: true,
            alpn,
        };
        assert!(
            t.get_hello(&hello(Some("a.akit.test"), acme_only))
                .is_none()
        );
        let challenge =
            acme::alpn_cert("a.akit.test", "token.thumb").unwrap_or_else(|e| panic!("{e}"));
        let other = acme::alpn_cert("b.akit.test", "token.thumb").unwrap_or_else(|e| panic!("{e}"));
        a.set_alpn("a.akit.test", Some(challenge.clone()));
        a.set_alpn("b.akit.test", Some(other));
        let leaf = |k: &CertifiedKey| k.chain_der().next().map(<[u8]>::to_vec);
        let got = t.get_hello(&hello(Some("A.akit.test."), acme_only));
        assert_eq!(got.as_ref().and_then(leaf), leaf(&challenge));
        // Not for a name the listener does not list, nor without a name.
        assert!(
            t.get_hello(&hello(Some("b.akit.test"), acme_only))
                .is_none()
        );
        assert!(t.get_hello(&hello(None, acme_only)).is_none());
        // A client offering h2 too gets no certificate yet: none was obtained.
        let both: &[&[u8]] = &[b"h2", ACME_TLS_ALPN.as_bytes()];
        assert!(t.get_hello(&hello(Some("a.akit.test"), both)).is_none());
        a.set_alpn("a.akit.test", None);
        assert!(
            t.get_hello(&hello(Some("a.akit.test"), acme_only))
                .is_none()
        );
        a.close();

        // Not offered when acme.challenges leaves TLS-ALPN-01 out.
        let (a, t) = acme_tls(vec![Challenge::Http01]);
        assert_eq!(t.alpn, ["h2", "http/1.1"]);
        a.close();
    }

    #[test]
    fn alpn_as_fit() {
        assert_eq!(alpn(&[Protocol::Http11, Protocol::H2]), ["h2", "http/1.1"]);
        assert_eq!(alpn(&[Protocol::H2, Protocol::Http11]), ["h2", "http/1.1"]);
        assert_eq!(alpn(&[Protocol::H2]), ["h2"]);
        assert_eq!(alpn(&[Protocol::Http11]), ["http/1.1"]);
    }
}
