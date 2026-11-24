#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's certificate management: files, ACME, storage, OCSP stapling, on-demand TLS.
//!
//! So far (I09): a listener's `tls` from files, as fit's `tlsmgr.LoadStatic` and
//! `LoadClientAuth` load it, made into the akio-tls [`ServerConfig`] the listener serves:
//!
//! - **Certificates** from PEM files (a chain and its key, each at most [`MAX_CERT_FILE_BYTES`]),
//!   chosen by the name the client asks for (SNI): the first certificate whose names (its
//!   subjectAltName's DNS names and IP addresses) hold that name, or the first of all when none
//!   does or the client names none. akio-tls picks a certificate by its exact name, so a
//!   wildcard name (`*.example.com`) picks its certificate only in the first certificate, the one
//!   served by default; elsewhere it is refused as not supported yet until akio's certificate
//!   resolver (A2) is on akio's `main`.
//! - **ALPN** from the listener's protocols: `h2` and `http/1.1`, in fit's order.
//! - **Client authentication** (`client_auth`): the authorities of its PEM files, a certificate
//!   required or verified if given.
//! - **Versions and cipher suites** as fit's: TLS 1.2 and 1.3, with only ECDHE and AEAD suites
//!   (AES-GCM, ChaCha20-Poly1305) for 1.2: akio-tls has no other.
//! - Session resumption by tickets, on as in Go's TLS servers, and by akio's session cache.
//! - **HTTP/3** (I10): a listener serving `h3` gets a second configuration from the same files
//!   ([`server_configs`]), with ALPN `h3` and TLS 1.3 alone, for its QUIC connections.
//!
//! - **Metrics** (I37): what each certificate is for and when it expires
//!   ([`ServerConfigs::certificates`]), for `fit_tls_certificate_not_after_seconds`.
//!
//! Loading reads files, which blocks: this is control plane code, run when a config is compiled,
//! never on an I/O thread (rules.md rule 1).
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

use std::collections::HashMap;
use std::fmt;
use std::io::Read;
use std::net::IpAddr;

use akio::tls::{Certificate, PrivateKey, RootCerts, ServerConfig};
use akio_x509::GeneralName;
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
    /// The certificates served, in the config's order: what the metrics say of each.
    pub certificates: Vec<CertificateInfo>,
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
    fn of(c: &akio_x509::Certificate) -> CertificateInfo {
        let name = c
            .subject_alt_names()
            .iter()
            .find_map(|n| match n {
                GeneralName::Dns(d) => Some(d.clone()),
                _ => None,
            })
            .or_else(|| c.subject().common_name())
            .unwrap_or_default();
        CertificateInfo {
            name,
            serial: serial_hex(c.serial()),
            not_after: c.not_after(),
        }
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

/// Loads `tls` from its files into the configuration a listener serving `protocols` serves over
/// TCP: [`server_configs`]'s `tcp`.
///
/// # Errors
///
/// As [`server_configs`].
pub fn server_config(tls: &Tls, protocols: &[Protocol]) -> Result<ServerConfig, Error> {
    server_configs(tls, protocols).map(|c| c.tcp)
}

/// Loads `tls` from its files, once, into the configurations a listener serving `protocols`
/// serves: over TCP, and over QUIC when it serves `h3`.
///
/// # Errors
///
/// The first problem found: a file that cannot be read or is too large, PEM with no certificate
/// or key, a key that does not match its certificate, an authority file with no certificate, or
/// a wildcard name akit cannot pick its certificate by yet.
pub fn server_configs(tls: &Tls, protocols: &[Protocol]) -> Result<ServerConfigs, Error> {
    let mut certs: Vec<(Vec<Certificate>, PrivateKey, Vec<String>)> = Vec::new();
    let mut certificates = Vec::with_capacity(tls.certificates.len());
    for (i, c) in tls.certificates.iter().enumerate() {
        let at = format!("certificates[{i}]");
        let (chain, key, names, info) =
            load_pair(&c.cert_file, &c.key_file).map_err(|msg| Error::new(&at, msg))?;
        certs.push((chain, key, names));
        certificates.push(info);
    }
    let picks = sni_picks(certs.iter().map(|(_, _, names)| names.as_slice()))
        .map_err(|(i, name)| {
            Error::new(
                format!("certificates[{i}]"),
                format!(
                    "{NOT_SUPPORTED_YET}: the wildcard name {name:?} in a certificate other than the first (akio-tls picks a certificate by its exact name)"
                ),
            )
        })?;
    let Some((chain, key, _)) = certs.first() else {
        // akit-config refuses a tls without certificates (ACME obtains them in fit).
        return Err(Error::new(
            "certificates",
            format!("{NOT_SUPPORTED_YET}: a tls without certificates"),
        ));
    };
    let mut roots = None;
    if let Some(ca) = &tls.client_auth {
        let mut r = RootCerts::empty();
        for (i, path) in ca.ca_files.iter().enumerate() {
            let at = format!("client_auth.ca_files[{i}]");
            let pem = read(path).map_err(|msg| Error::new(&at, msg))?;
            match r.add_pem(&pem) {
                Ok(0) => return Err(Error::new(at, format!("{path}: no PEM certificate found"))),
                Ok(_) => {}
                Err(e) => return Err(Error::new(at, format!("{path}: {e}"))),
            }
        }
        roots = Some((r, ca.mode));
    }
    // One builder per configuration, from what was read once.
    let build = |alpn: Vec<&str>, tls13_only: bool| {
        let mut b = ServerConfig::builder(chain.clone(), key.clone())
            .alpn(alpn)
            .session_tickets(true);
        if tls13_only {
            b = b.tls13_only(true);
        }
        for (name, i) in &picks {
            if let Some((chain, key, _)) = certs.get(*i) {
                b = b.sni_cert(name.clone(), chain.clone(), key.clone());
            }
        }
        if let Some((roots, mode)) = &roots {
            b = match mode {
                ClientAuthMode::Require => b.client_auth_required(roots.clone()),
                ClientAuthMode::VerifyIfGiven => b.client_auth_optional(roots.clone()),
            };
        }
        // max_version 1.2 is refused by akit-config (akio API request A9): 1.3 is the default.
        b.build().map_err(|e| Error::new("", e.to_string()))
    };
    let tcp = build(alpn(protocols), false)?;
    // QUIC is TLS 1.3 alone, whatever max_version caps TCP at (fit's http3Config).
    let h3 = if protocols.contains(&Protocol::H3) {
        Some(build(vec!["h3"], true)?)
    } else {
        None
    };
    Ok(ServerConfigs {
        tcp,
        h3,
        certificates,
    })
}

/// A certificate's chain, its key, checked to match, the names it is for (lower case), and what
/// the metrics say of it.
fn load_pair(
    cert_file: &str,
    key_file: &str,
) -> Result<(Vec<Certificate>, PrivateKey, Vec<String>, CertificateInfo), String> {
    let chain_pem = read(cert_file)?;
    let key_pem = read(key_file)?;
    let chain = Certificate::from_pem(&chain_pem).map_err(|e| format!("{cert_file}: {e}"))?;
    let Some(leaf) = chain.first() else {
        return Err(format!("{cert_file}: no PEM certificate found"));
    };
    let key = PrivateKey::from_pem(&key_pem).map_err(|e| format!("{key_file}: {e}"))?;
    // Built alone to check the pair here, where its path is known.
    ServerConfig::builder(chain.clone(), key.clone())
        .build()
        .map_err(|e| format!("{cert_file}, {key_file}: {e}"))?;
    let parsed =
        akio_x509::Certificate::from_der(leaf.as_der()).map_err(|e| format!("{cert_file}: {e}"))?;
    let names = parsed
        .subject_alt_names()
        .iter()
        .filter_map(|n| match n {
            GeneralName::Dns(d) => Some(d.to_ascii_lowercase()),
            GeneralName::Ip(b) => ip(b).map(|a| a.to_string()),
            _ => None,
        })
        .collect();
    let info = CertificateInfo::of(&parsed);
    Ok((chain, key, names, info))
}

fn ip(b: &[u8]) -> Option<IpAddr> {
    if let Ok(v4) = <[u8; 4]>::try_from(b) {
        return Some(IpAddr::from(v4));
    }
    <[u8; 16]>::try_from(b).ok().map(IpAddr::from)
}

/// Reads the file at `path`, refusing one larger than [`MAX_CERT_FILE_BYTES`].
fn read(path: &str) -> Result<Vec<u8>, String> {
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

/// Which certificate each exact name picks, for akio-tls's by-name table, given each
/// certificate's names in order: fit's rule (the first certificate that holds the name, exactly
/// or by a `*.` wildcard of its parent) where it differs from the default, the first of all.
/// A wildcard whose names would pick a certificate other than the first cannot be told to
/// akio-tls, which looks names up exactly: `Err` gives that certificate and name.
fn sni_picks<'a>(
    certs: impl Iterator<Item = &'a [String]>,
) -> Result<Vec<(String, usize)>, (usize, String)> {
    let certs: Vec<&[String]> = certs.collect();
    // The first certificate holding each exact name, and each wildcard's parent.
    let mut exact: HashMap<&str, usize> = HashMap::new();
    let mut wild: HashMap<&str, usize> = HashMap::new();
    for (i, names) in certs.iter().enumerate() {
        for n in *names {
            match n.strip_prefix("*.") {
                Some(parent) => {
                    wild.entry(parent).or_insert(i);
                }
                None => {
                    exact.entry(n.as_str()).or_insert(i);
                }
            }
        }
    }
    for (i, names) in certs.iter().enumerate() {
        for n in *names {
            if let Some(parent) = n.strip_prefix("*.") {
                if wild.get(parent) == Some(&i) && i > 0 {
                    return Err((i, n.clone()));
                }
            }
        }
    }
    let mut picks: Vec<(String, usize)> = exact
        .into_iter()
        .filter_map(|(name, i)| {
            let by_wild = name
                .split_once('.')
                .and_then(|(_, parent)| wild.get(parent))
                .copied();
            let pick = by_wild.map_or(i, |w| w.min(i));
            (pick > 0).then(|| (name.to_owned(), pick))
        })
        .collect();
    picks.sort();
    Ok(picks)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn names(v: &[&[&str]]) -> Vec<Vec<String>> {
        v.iter()
            .map(|n| n.iter().map(|s| (*s).to_owned()).collect())
            .collect()
    }

    fn picks(v: &[&[&str]]) -> Result<Vec<(String, usize)>, (usize, String)> {
        let n = names(v);
        sni_picks(n.iter().map(Vec::as_slice))
    }

    #[test]
    fn serials_as_go_writes_them() {
        assert_eq!(serial_hex(&[0x00, 0x8f, 0x01]), "8f01");
        assert_eq!(serial_hex(&[0x01, 0x00]), "100");
        assert_eq!(serial_hex(&[0x00]), "0");
        assert_eq!(serial_hex(&[]), "0");
        assert_eq!(serial_hex(&[0xff]), "-1");
        assert_eq!(serial_hex(&[0x80, 0x00]), "-8000");
    }

    #[test]
    fn sni_picks_as_fit() {
        // The first certificate is the default: its names need no entry.
        assert_eq!(
            picks(&[&["a.example"], &["b.example", "a.example"], &["c.example"]]),
            Ok(vec![("b.example".into(), 1), ("c.example".into(), 2)])
        );
        // A name the first certificate's wildcard holds picks the first, though a later one
        // names it exactly (fit walks the certificates in order).
        assert_eq!(
            picks(&[&["*.example"], &["a.example", "x.other"]]),
            Ok(vec![("x.other".into(), 1)])
        );
        // A later wildcard whose parent an earlier certificate's wildcard already has.
        assert_eq!(
            picks(&[&["*.example"], &["*.example", "b.other"]]),
            Ok(vec![("b.other".into(), 1)])
        );
        // A wildcard picking a later certificate: akio-tls cannot be told.
        assert_eq!(
            picks(&[&["a.example"], &["*.example"]]),
            Err((1, "*.example".into()))
        );
        assert_eq!(picks(&[]), Ok(vec![]));
    }

    #[test]
    fn alpn_as_fit() {
        assert_eq!(alpn(&[Protocol::Http11, Protocol::H2]), ["h2", "http/1.1"]);
        assert_eq!(alpn(&[Protocol::H2, Protocol::Http11]), ["h2", "http/1.1"]);
        assert_eq!(alpn(&[Protocol::H2]), ["h2"]);
        assert_eq!(alpn(&[Protocol::Http11]), ["http/1.1"]);
    }

    #[test]
    fn ip_sans() {
        assert_eq!(ip(&[127, 0, 0, 1]), Some("127.0.0.1".parse().unwrap()));
        assert_eq!(ip(&[0; 16]), Some("::".parse().unwrap()));
        assert_eq!(ip(&[1, 2, 3]), None);
    }
}
