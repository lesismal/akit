//! TLS links (I102, fit's `tunnel/internal/transport/tls.go` and `internal/certpin`): TLS 1.3
//! with the ALPN `fit-tunnel`, no session resumed, the hub's certificate checked by `pin`, by
//! `ca_file`, or by the system's roots; the hub's own self-signed certificate made once and kept
//! beside the config.
//!
//! A pin is checked in the handshake (akio-tls's `pin_sha256`, A235): the leaf's SHA-256 must be
//! the pin, compared in constant time, in place of the chain and the name; a mismatch fails the
//! handshake, and what the link sent meanwhile never leaves. The handshake's signature proves the
//! peer holds the key of the certificate the pin names, as fit's `VerifyPeerCertificate` does.

use std::sync::Arc;

use akio::tls::{Certificate, ClientConfig, PrivateKey, RootCerts, ServerConfig};
use akio_crypto::hash::Sha256;

/// The ALPN of a tunnel link: fit's (decision 038).
pub const ALPN: &str = "fit-tunnel";

/// The organization of the hub's self-signed certificate's subject (fit's "fit tunnel", decision
/// 038's names).
pub const ORG: &str = "akit tunnel";

/// A self-signed certificate for `hosts`, valid ten years: its PEM and its key's PEM.
///
/// # Errors
///
/// No random bytes, or a host that is not one.
pub fn self_signed(hosts: &[String]) -> Result<(String, String), String> {
    let key = akio_crypto::p256::SecretKey::generate().map_err(|e| e.to_string())?;
    let key = akio_crypto::pkey::PrivateKey::P256(key);
    let hosts: Vec<&str> = if hosts.is_empty() {
        vec!["localhost"]
    } else {
        hosts
            .iter()
            .map(String::as_str)
            .filter(|h| !h.is_empty())
            .collect()
    };
    let mut p = akio_x509::CertificateParams::new(hosts);
    let now = akit_roles::linktoken::unix_now();
    p.not_before = now.saturating_sub(3600);
    p.not_after = now.saturating_add(10 * 365 * 86_400);
    let cert = p.self_signed(&key).map_err(|e| e.to_string())?;
    Ok((
        cert.to_pem(),
        akio_crypto::pem::encode("PRIVATE KEY", &key.to_pkcs8_der()),
    ))
}

/// Splits a PEM file into its certificates and its key.
#[must_use]
pub fn split_pem(data: &str) -> (String, String) {
    let mut cert = String::new();
    let mut key = String::new();
    let mut cur = String::new();
    let mut is_cert = false;
    for line in data.lines() {
        if line.starts_with("-----BEGIN ") {
            cur.clear();
            is_cert = line.contains("CERTIFICATE");
        }
        cur.push_str(line);
        cur.push('\n');
        if line.starts_with("-----END ") {
            if is_cert {
                cert.push_str(&cur);
            } else {
                key.push_str(&cur);
            }
            cur.clear();
        }
    }
    (cert, key)
}

/// The certificate and key kept at `path`, or a new self-signed one for `hosts` written there,
/// mode 0600; and whether it was made. Reads and writes a file: the control thread's.
///
/// # Errors
///
/// A file that holds no certificate and key, or one that cannot be read or written.
pub fn keep(path: &str, hosts: &[String]) -> Result<(String, String, bool), String> {
    match std::fs::read_to_string(path) {
        Ok(data) => {
            let (cert, key) = split_pem(&data);
            server_config(&cert, &key)
                .map_err(|e| format!("{path} does not hold a certificate and its key: {e}"))?;
            Ok((cert, key, false))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let (cert, key) = self_signed(hosts)?;
            write_private(path, &format!("{cert}{key}")).map_err(|e| format!("{path}: {e}"))?;
            Ok((cert, key, true))
        }
        Err(e) => Err(format!("{path}: {e}")),
    }
}

fn write_private(path: &str, data: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(path)?;
    f.write_all(data.as_bytes())?;
    f.sync_all()
}

/// The SHA-256 of a PEM's first certificate, in hex: what peers pin.
#[must_use]
pub fn fingerprint(cert_pem: &str) -> String {
    Certificate::from_pem(cert_pem.as_bytes())
        .ok()
        .and_then(|c| {
            c.first()
                .map(|c| akit_roles::linktoken::hex(&Sha256::digest(c.as_der())))
        })
        .unwrap_or_default()
}

/// A hub listener's TLS: TLS 1.3, the ALPN, no session tickets.
///
/// # Errors
///
/// A certificate or key that does not load.
pub fn server_config(cert_pem: &str, key_pem: &str) -> Result<ServerConfig, String> {
    server_config_alpn(cert_pem, key_pem, ALPN)
}

/// [`server_config`] with another ALPN: `wss` listeners' `http/1.1` (`crate::ws::ALPN`).
///
/// # Errors
///
/// A certificate or key that does not load.
pub fn server_config_alpn(
    cert_pem: &str,
    key_pem: &str,
    alpn: &str,
) -> Result<ServerConfig, String> {
    server_config_with(cert_pem, key_pem, alpn, None)
}

/// [`server_config_alpn`] whose certificate `store` can replace while it serves: what
/// [`akio::tls::CertStore::set_default`] gives it is served to the handshakes from then on, the
/// certificate given here until then (I102d).
///
/// # Errors
///
/// A certificate or key that does not load.
pub fn server_config_store(
    cert_pem: &str,
    key_pem: &str,
    alpn: &str,
    store: &akio::tls::CertStore,
) -> Result<ServerConfig, String> {
    server_config_with(cert_pem, key_pem, alpn, Some(store))
}

fn server_config_with(
    cert_pem: &str,
    key_pem: &str,
    alpn: &str,
    store: Option<&akio::tls::CertStore>,
) -> Result<ServerConfig, String> {
    let chain = Certificate::from_pem(cert_pem.as_bytes()).map_err(|e| e.to_string())?;
    if chain.is_empty() {
        return Err("no certificate".to_owned());
    }
    let key = PrivateKey::from_pem(key_pem.as_bytes()).map_err(|e| e.to_string())?;
    let mut b = ServerConfig::builder(chain, key)
        .alpn([alpn])
        .tls13_only(true)
        .session_tickets(false);
    if let Some(s) = store {
        b = b.cert_resolver(Arc::new(s.clone()));
    }
    b.build().map_err(|e| e.to_string())
}

/// A certificate and key, for a [`akio::tls::CertStore`] to serve.
///
/// # Errors
///
/// A certificate or key that does not load, or a key that is not the certificate's.
pub fn certified_key(cert_pem: &str, key_pem: &str) -> Result<akio::tls::CertifiedKey, String> {
    akio::tls::CertifiedKey::from_pem(cert_pem.as_bytes(), key_pem.as_bytes())
        .map_err(|e| e.to_string())
}

/// How a link checks the hub's certificate.
#[derive(Debug, Clone)]
pub enum Check {
    /// By its SHA-256.
    Pin([u8; 32]),
    /// By the CAs of a PEM file, and its name.
    Ca(String),
    /// By the system's roots, and its name.
    System,
    /// Not at all (`quic_insecure`, QUIC alone).
    Insecure,
}

impl Check {
    /// The check an uplink's fields ask for (fit's `CheckBy`).
    #[must_use]
    pub fn of(pin: &str, ca_file: &str, insecure: bool) -> Check {
        if let Some(p) = akit_config::roles::parse_pin(pin).filter(|_| !pin.is_empty()) {
            Check::Pin(p)
        } else if !ca_file.is_empty() {
            Check::Ca(ca_file.to_owned())
        } else if insecure {
            Check::Insecure
        } else {
            Check::System
        }
    }
}

/// A link's client TLS. Reads `ca_file` (at each dial, as fit's) and the system's roots: off any
/// I/O thread (a dial's resolver callback or the control thread).
///
/// # Errors
///
/// CAs that do not load.
pub fn client_config(check: &Check) -> Result<ClientConfig, String> {
    client_config_alpn(check, ALPN)
}

/// [`client_config`] with another ALPN: `wss` links' `http/1.1` (`crate::ws::ALPN`).
///
/// # Errors
///
/// CAs that do not load.
pub fn client_config_alpn(check: &Check, alpn: &str) -> Result<ClientConfig, String> {
    let b = ClientConfig::builder()
        .alpn([alpn])
        .tls13_only(true)
        .resumption(false);
    let b = match check {
        Check::Pin(pin) => b.pin_sha256(vec![*pin]),
        Check::Insecure => b.insecure_skip_verify(),
        Check::Ca(path) => {
            let pem = std::fs::read(path).map_err(|e| format!("tunnel: the hub's CAs: {e}"))?;
            let mut roots = RootCerts::empty();
            let n = roots
                .add_pem(&pem)
                .map_err(|e| format!("tunnel: the hub's CAs: {e}"))?;
            if n == 0 {
                return Err(format!(
                    "tunnel: the hub's CAs: {path} has no PEM certificate"
                ));
            }
            b.roots(roots)
        }
        Check::System => {
            b.roots(RootCerts::system().map_err(|e| format!("the system's roots: {e}"))?)
        }
    };
    b.build().map_err(|e| e.to_string())
}

/// A hub's certificate for its TLS listeners without their own: `quic.cert` and `quic.key`, or the
/// one kept beside the config, made when there is none; with whether it was made. Reads and
/// writes files: the control thread's.
///
/// # Errors
///
/// A certificate that does not load, a file that cannot be written; or no place to keep one.
pub fn hub_cert(
    cfg: &akit_config::roles::TunnelHub,
    kept: Option<&str>,
) -> Result<(Arc<(String, String)>, bool), String> {
    if let Some(q) = cfg.quic.as_ref().filter(|q| !q.cert.is_empty()) {
        server_config(&q.cert, &q.key).map_err(|e| format!("quic.cert: {e}"))?;
        return Ok((Arc::new((q.cert.clone(), q.key.clone())), false));
    }
    let hosts = vec!["localhost".to_owned(), cfg.hub_name()];
    match kept {
        Some(path) => {
            let (c, k, made) = keep(path, &hosts)?;
            Ok((Arc::new((c, k)), made))
        }
        None => {
            let (c, k) = self_signed(&hosts)?;
            Ok((Arc::new((c, k)), true))
        }
    }
}
