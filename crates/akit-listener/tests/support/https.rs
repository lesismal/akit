//! HTTPS clients for tests: HTTP/1.1 over akio-tls's blocking stream, with the server name and
//! ALPN protocols chosen, and HTTP/2 with akio-http's client.
#![allow(dead_code, unreachable_pub, clippy::unwrap_used)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use akio::Engine;
use akio::tls::stream::Stream;
use akio::tls::{Certificate, ClientConfig, PrivateKey, RootCerts};
use akio_http::client::ClientRequest;
use akio_http::h2client::{H2Client, H2Options};

/// A client trusting the CA at `ca`, offering `alpn`, with a client certificate if given.
pub fn client(ca: &Path, alpn: &[&str], cert: Option<(&Path, &Path)>) -> ClientConfig {
    let mut roots = RootCerts::empty();
    roots.add_pem(&std::fs::read(ca).unwrap()).unwrap();
    let mut b = ClientConfig::builder().roots(roots);
    if !alpn.is_empty() {
        b = b.alpn(alpn);
    }
    if let Some((c, k)) = cert {
        b = b.client_cert(
            Certificate::from_pem(&std::fs::read(c).unwrap()).unwrap(),
            PrivateKey::from_pem(&std::fs::read(k).unwrap()).unwrap(),
        );
    }
    b.build().unwrap()
}

/// What an HTTPS/1.1 exchange saw.
#[derive(Debug)]
pub struct Seen {
    /// The protocol ALPN chose.
    pub alpn: Option<String>,
    /// The DER of the certificate the server sent.
    pub leaf: Vec<u8>,
    /// The status line.
    pub status: String,
    /// The `Alt-Svc` field, if any.
    pub alt_svc: Option<String>,
    /// The body.
    pub body: String,
}

/// A GET over TLS to `server_name` (SNI), with `host` as its Host field.
pub fn get_h1(
    addr: SocketAddr,
    cfg: &ClientConfig,
    server_name: &str,
    host: &str,
    path: &str,
) -> std::io::Result<Seen> {
    let tcp = TcpStream::connect(addr)?;
    tcp.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut s = Stream::client(cfg, server_name, tcp)?;
    s.handshake()?;
    let alpn = s
        .conn()
        .alpn()
        .map(|a| String::from_utf8_lossy(a).into_owned());
    let leaf = s
        .conn()
        .peer_certificates()
        .first()
        .cloned()
        .unwrap_or_default();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )?;
    s.flush()?;
    let mut resp = Vec::new();
    s.read_to_end(&mut resp)?;
    let resp = String::from_utf8(resp).unwrap();
    let Some((head, body)) = resp.split_once("\r\n\r\n") else {
        return Err(std::io::Error::other(format!("no response: {resp:?}")));
    };
    let alt_svc = head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.eq_ignore_ascii_case("alt-svc")
            .then(|| v.trim().to_owned())
    });
    Ok(Seen {
        alpn,
        leaf,
        status: head.lines().next().unwrap().to_owned(),
        alt_svc,
        body: body.to_owned(),
    })
}

/// An HTTP/2 GET over TLS to `server_name`, its ALPN `h2`; the status and the body.
pub fn get_h2(
    addr: SocketAddr,
    cfg: &ClientConfig,
    server_name: &str,
    path: &str,
) -> Result<(u16, String), String> {
    get_h2_alt_svc(addr, cfg, server_name, path).map(|(s, b, _)| (s, b))
}

/// [`get_h2`], with the response's `Alt-Svc` field, if any.
pub fn get_h2_alt_svc(
    addr: SocketAddr,
    cfg: &ClientConfig,
    server_name: &str,
    path: &str,
) -> Result<(u16, String, Option<String>), String> {
    let client = Engine::builder()
        .client_threads(1)
        .config({
            let mut v = akio::Config::default();
            v.shutdown_timeout = Duration::from_millis(200);
            v
        })
        .build()
        .unwrap();
    client.start().unwrap();
    let r = (|| {
        let c = H2Client::dial_blocking(
            &client,
            addr,
            Some((cfg.clone(), server_name)),
            H2Options::default(),
        )
        .map_err(|e| e.to_string())?;
        let r = c
            .send(
                ClientRequest::get(&format!("https://{server_name}{path}"))
                    .timeout(Duration::from_secs(5)),
            )
            .wait()
            .map_err(|e| e.to_string())?;
        let alt_svc = r.headers.get("alt-svc").map(str::to_owned);
        Ok((r.status, String::from_utf8(r.body).unwrap(), alt_svc))
    })();
    client.shutdown();
    client.wait();
    r
}

/// The DER of the first certificate of the PEM file at `path`.
pub fn leaf_der(path: &Path) -> Vec<u8> {
    Certificate::from_pem(&std::fs::read(path).unwrap()).unwrap()[0]
        .as_der()
        .to_vec()
}
