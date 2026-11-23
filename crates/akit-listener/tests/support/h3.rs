//! An HTTP/3 client for tests: akio-h3's, trusting a test CA, with a client certificate if
//! given.
#![allow(dead_code, unreachable_pub, clippy::unwrap_used)]

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use akio::Engine;
use akio::tls::{Certificate, ClientConfig, PrivateKey, RootCerts};
use akio_h3::H3Client;

/// A QUIC client config trusting the CA at `ca`, offering ALPN `h3`, with a client certificate
/// if given.
pub fn client(ca: &Path, cert: Option<(&Path, &Path)>) -> akio_quic::ClientConfig {
    let mut roots = RootCerts::empty();
    roots.add_pem(&std::fs::read(ca).unwrap()).unwrap();
    let mut b = ClientConfig::builder()
        .roots(roots)
        .alpn(["h3"])
        .tls13_only(true);
    if let Some((c, k)) = cert {
        b = b.client_cert(
            Certificate::from_pem(&std::fs::read(c).unwrap()).unwrap(),
            PrivateKey::from_pem(&std::fs::read(k).unwrap()).unwrap(),
        );
    }
    akio_quic::client_config_from(&b.build().unwrap()).unwrap()
}

/// What an HTTP/3 GET saw.
#[derive(Debug, PartialEq, Eq)]
pub struct Seen {
    /// The protocol ALPN chose.
    pub alpn: Option<String>,
    /// The status.
    pub status: u16,
    /// The `Alt-Svc` field, if any.
    pub alt_svc: Option<String>,
    /// The body.
    pub body: String,
}

/// An HTTP/3 GET of `path` from `addr` to `server_name` (SNI and `:authority`), on a client
/// engine of its own.
pub fn get(
    addr: SocketAddr,
    cfg: akio_quic::ClientConfig,
    server_name: &str,
    path: &str,
) -> Result<Seen, String> {
    let engine = Engine::builder()
        .udp_client_threads(1)
        .config({
            let mut v = akio::Config::default();
            v.shutdown_timeout = Duration::from_millis(200);
            v
        })
        .build()
        .unwrap();
    engine.start().unwrap();
    let r = (|| {
        let c = H3Client::dial(&engine, addr, server_name, cfg).map_err(|e| e.to_string())?;
        let r = c.get(path, Duration::from_secs(5));
        let alpn = c
            .connection()
            .alpn()
            .map(|a| String::from_utf8_lossy(&a).into_owned());
        c.close();
        let r = r.map_err(|e| e.to_string())?;
        Ok(Seen {
            alpn,
            status: r.status,
            alt_svc: r.headers.get("alt-svc").map(str::to_owned),
            body: String::from_utf8(r.body).unwrap(),
        })
    })();
    engine.shutdown();
    engine.wait();
    r
}
