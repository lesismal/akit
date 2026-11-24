//! TLS to an `h2` upstream (I54): fit's `tls_ca_file`, `tls_cert_file` and `tls_key_file`, read
//! when the config is compiled, as fit's `upstream.New` reads them.

use std::collections::BTreeMap;

use akio::tls::{Certificate, ClientConfig, PrivateKey, RootCerts};
use akio_crypto::hash::Sha256;
use akit_config::{Upstream, UpstreamProtocol};

use crate::{Error, Sum, read};

/// The ALPN protocols an `h2` upstream is offered, as fit's: HTTP/2, and HTTP/1.1 for a server
/// that chooses it.
pub const UPSTREAM_ALPN: [&str; 2] = ["h2", "http/1.1"];

/// The TLS configuration of an `h2` upstream whose config names files: the certificates of
/// `tls_ca_file` in place of the system's roots, the client certificate of `tls_cert_file` and
/// `tls_key_file`, presented to servers that ask for one, by the proxy's requests and the health
/// checks' probes alike, and [`UPSTREAM_ALPN`]. `tls_server_name` takes no file: the upstream's
/// pool reads it from the config.
#[derive(Debug, Clone)]
pub struct UpstreamTls {
    config: ClientConfig,
    files: BTreeMap<String, Sum>,
}

impl UpstreamTls {
    /// The TLS configuration of `u` (which akit-config has checked): `None` when it is not an
    /// `h2` upstream or names no file, which leaves it the system's roots. Reads the files, and
    /// the system's roots for a client certificate without `tls_ca_file`: run when the config is
    /// compiled, never on an I/O thread. An error's path is the field's, under the upstream
    /// (`tls_ca_file`, or `tls_cert_file` for anything about the certificate or its key, as fit
    /// reports them).
    pub fn load(u: &Upstream) -> Result<Option<UpstreamTls>, Error> {
        if u.protocol != UpstreamProtocol::H2
            || (u.tls_ca_file.is_empty() && u.tls_cert_file.is_empty())
        {
            return Ok(None);
        }
        let mut files = BTreeMap::new();
        let roots = if u.tls_ca_file.is_empty() {
            RootCerts::system().map_err(|e| Error::new("tls_cert_file", e.to_string()))?
        } else {
            let at = "tls_ca_file";
            let path = &u.tls_ca_file;
            let pem = read(path).map_err(|msg| Error::new(at, msg))?;
            files.insert(path.clone(), Sha256::digest(&pem));
            let mut r = RootCerts::empty();
            // As Go's `AppendCertsFromPEM`, which fit's message follows: no certificate read is
            // one message, whether the file held none or held something else.
            if !matches!(r.add_pem(&pem), Ok(n) if n > 0) {
                return Err(Error::new(at, format!("{path} holds no PEM certificate")));
            }
            r
        };
        let mut b = ClientConfig::builder().roots(roots).alpn(UPSTREAM_ALPN);
        if !u.tls_cert_file.is_empty() {
            let at = "tls_cert_file";
            let (cert, key) = (&u.tls_cert_file, &u.tls_key_file);
            let chain_pem = read(cert).map_err(|msg| Error::new(at, msg))?;
            let key_pem = read(key).map_err(|msg| Error::new(at, msg))?;
            let chain = Certificate::from_pem(&chain_pem)
                .map_err(|e| Error::new(at, format!("{cert}: {e}")))?;
            if chain.is_empty() {
                return Err(Error::new(at, format!("{cert}: no PEM certificate found")));
            }
            let k = PrivateKey::from_pem(&key_pem)
                .map_err(|e| Error::new(at, format!("{key}: {e}")))?;
            files.insert(cert.clone(), Sha256::digest(&chain_pem));
            files.insert(key.clone(), Sha256::digest(&key_pem));
            b = b.client_cert(chain, k);
        }
        // akio checks that the key is the certificate's (A109).
        let config = b.build().map_err(|e| {
            let at = if u.tls_cert_file.is_empty() {
                "tls_ca_file"
            } else {
                "tls_cert_file"
            };
            let what = if u.tls_cert_file.is_empty() {
                u.tls_ca_file.clone()
            } else {
                format!("{}, {}", u.tls_cert_file, u.tls_key_file)
            };
            Error::new(at, format!("{what}: {e}"))
        })?;
        Ok(Some(UpstreamTls { config, files }))
    }

    /// The configuration the upstream's connections are made with.
    #[must_use]
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// The files it was read from, with their SHA-256, by path: watched as a listener's
    /// certificate files are (`run -watch-certs`), so that a change compiles the config again,
    /// and the upstream's new pool reads them again.
    #[must_use]
    pub fn files(&self) -> &BTreeMap<String, Sum> {
        &self.files
    }
}
