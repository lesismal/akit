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
//! Loading reads files, which blocks: this is control plane code, run when a config is compiled,
//! never on an I/O thread (rules.md rule 1).
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

mod certs;
mod storage;
mod watch;

pub use certs::{Certs, Sum};
pub use storage::{
    DEFAULT_LOCK_POLL, DEFAULT_LOCK_REFRESH, DEFAULT_LOCK_STALE, FileStorage, MAX_HELD_LOCKS,
    MAX_VALUE_BYTES, Storage, StorageError,
};
pub use watch::{DEFAULT_WATCH_DEBOUNCE, DEFAULT_WATCH_INTERVAL, FileWatcher, Report};

use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use std::sync::Arc;

use akio::tls::{CertResolver, Certificate, CertifiedKey, PrivateKey, RootCerts, ServerConfig};
use akio_crypto::hash::Sha256;
use akit_config::{ClientAuthMode, NOT_SUPPORTED_YET, Protocol, Tls};

/// The largest certificate, key or authority file read, as fit's `tlsmgr.MaxCertFileBytes`.
pub const MAX_CERT_FILE_BYTES: u64 = 1 << 20;

/// What went wrong loading a listener's `tls`, and where: `path` is relative to the `tls`
/// object, as `certificates[1]` or `client_auth.ca_files[0]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// The path of what failed, under `tls`.
    pub path: String,
    /// What is wrong; starts with [`NOT_SUPPORTED_YET`] for what fit serves and akit not yet.
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

/// A listener's `tls`, loaded from its files once: its certificates, the manager of
/// certificates from files ([`Certs`]); the authorities of its client authentication; and the
/// protocols it serves. Cheap to clone.
#[derive(Clone)]
pub struct ListenerTls {
    certs: Arc<Certs>,
    roots: Option<(RootCerts, ClientAuthMode)>,
    alpn: Vec<&'static str>,
    h3: bool,
    /// The SHA-256 of every file read, certificates, keys and authorities, by path.
    files: BTreeMap<String, Sum>,
    /// The SHA-256 of the authority files, in order: what a configuration holds itself.
    ca_sums: Vec<Sum>,
}

impl fmt::Debug for ListenerTls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ListenerTls")
            .field("certs", &self.certs)
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
        let mut files = BTreeMap::new();
        let mut keys = Vec::with_capacity(tls.certificates.len());
        for (i, c) in tls.certificates.iter().enumerate() {
            let at = format!("certificates[{i}]");
            keys.push(
                load_pair(&c.cert_file, &c.key_file, &mut files)
                    .map_err(|msg| Error::new(&at, msg))?,
            );
        }
        if keys.is_empty() {
            // akit-config refuses a tls without certificates (ACME obtains them in fit).
            return Err(Error::new(
                "certificates",
                format!("{NOT_SUPPORTED_YET}: a tls without certificates"),
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
        let t = ListenerTls {
            certs: Arc::new(certs),
            roots,
            alpn: alpn(protocols),
            h3: protocols.contains(&Protocol::H3),
            files,
            ca_sums,
        };
        // Built once here, so that `validate` finds what would fail when the listener opens.
        t.server_configs(t.certs.clone())?;
        Ok(t)
    }

    /// Its certificates.
    #[must_use]
    pub fn certs(&self) -> &Arc<Certs> {
        &self.certs
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
    t.server_configs(t.certs.clone())
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
    use super::*;

    #[test]
    fn alpn_as_fit() {
        assert_eq!(alpn(&[Protocol::Http11, Protocol::H2]), ["h2", "http/1.1"]);
        assert_eq!(alpn(&[Protocol::H2, Protocol::Http11]), ["h2", "http/1.1"]);
        assert_eq!(alpn(&[Protocol::H2]), ["h2"]);
        assert_eq!(alpn(&[Protocol::Http11]), ["http/1.1"]);
    }
}
