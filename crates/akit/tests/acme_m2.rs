//! M2's exit criterion (I38): a config with only a host name gets a certificate and serves HTTPS,
//! through `akit run`.
//!
//! - `only_a_host_name_from_pebble`: against Pebble, Let's Encrypt's ACME test server (its
//!   binaries, or in Docker with `AKIT_PEBBLE_DOCKER=1`: `m2/pebble.rs`), which validates the
//!   HTTP-01 challenge on the `http` listener HTTPS by default makes. The config names the CA and
//!   the ports besides the host name, since Pebble is not Let's Encrypt and the test does not own
//!   80 and 443. Skipped without Pebble.
//! - `only_a_host_name_from_lets_encrypt_staging`: the criterion itself, against Let's Encrypt's
//!   staging CA, on ports 80 and 443, for the public name in `AKIT_ACME_STAGING_NAME`, which must
//!   resolve to the host the test runs on and reach it from the Internet on port 80. Skipped
//!   without it: the maintainer's step (`docs/progress.md`, I38).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod m2;
mod support;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use akio::tls::{ClientConfig, RootCerts};
use akio_x509::{Certificate, GeneralName};
use m2::pebble::{Pebble, free_port};
use m2::target::Proc;
use support::certs::dir;
use support::https::{get_h1, get_h2};

/// Let's Encrypt's staging directory.
const STAGING: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

/// The config: `acme` (`{...}` fields), maybe `auto_https`, and one route for `name`.
fn config(acme: &str, auto_https: &str, name: &str) -> String {
    format!(
        r#"{{"acme": {{{acme}}},{auto_https}
  "routes": [{{"match": {{"host": ["{name}"]}}, "handler": {{"module": "respond", "params": {{"body": "hello over https"}}}}}}]}}"#
    )
}

/// The certificate chain akit stored for `name` (`certificates/<ca>/<name>.pem`), once there.
fn stored(storage: &Path, name: &str, within: Duration, akit: &Proc) -> Vec<Certificate> {
    let deadline = Instant::now() + within;
    loop {
        if let Some(chain) = find(storage, &format!("{name}.pem")) {
            let certs = Certificate::from_pem(&std::fs::read(chain).unwrap()).unwrap();
            if !certs.is_empty() {
                return certs;
            }
        }
        assert!(
            Instant::now() < deadline,
            "no certificate for {name} within {within:?}; stderr: {:?}",
            akit.stderr()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn find(d: &Path, file: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(d).ok()? {
        let p = e.ok()?.path();
        if p.is_dir() {
            if let Some(f) = find(&p, file) {
                return Some(f);
            }
        } else if p.file_name().is_some_and(|n| n == file) {
            return Some(p);
        }
    }
    None
}

fn dns_names(c: &Certificate) -> Vec<String> {
    c.subject_alt_names()
        .iter()
        .filter_map(|n| match n {
            GeneralName::Dns(d) => Some(d.clone()),
            _ => None,
        })
        .collect()
}

/// A client trusting `roots`, offering `alpn`.
fn client(roots: RootCerts, alpn: &str) -> ClientConfig {
    ClientConfig::builder()
        .roots(roots)
        .alpn([alpn])
        .build()
        .unwrap()
}

/// The chain's last certificate (its issuing intermediate) as the only root: the served chain is
/// checked up to it, for a CA whose root the test cannot fetch (Let's Encrypt's staging roots
/// are in no trust store).
fn anchored_at_issuer(chain: &[Certificate]) -> RootCerts {
    let mut roots = RootCerts::empty();
    assert!(
        roots
            .add_pem(chain.last().unwrap().to_pem().as_bytes())
            .unwrap()
            > 0
    );
    roots
}

/// HTTPS at `addr` for `name`, over HTTP/1.1 and h2, its chain checked against `roots`: answered
/// with the route's body and the certificate `leaf`.
fn serves(addr: SocketAddr, name: &str, roots: impl Fn() -> RootCerts, leaf: &Certificate) {
    let seen = get_h1(addr, &client(roots(), "http/1.1"), name, name, "/").unwrap();
    assert!(seen.status.starts_with("HTTP/1.1 200"), "{}", seen.status);
    assert_eq!(seen.body, "hello over https");
    assert_eq!(seen.leaf, leaf.as_der());
    let (status, body) = get_h2(addr, &client(roots(), "h2"), name, "/").unwrap();
    assert_eq!((status, body.as_str()), (200, "hello over https"));
}

/// A GET in the clear to `addr` for `name`: the status line and the `Location`.
fn plain(addr: SocketAddr, name: &str, path: &str) -> (String, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {name}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    let head = resp.split("\r\n\r\n").next().unwrap();
    let location = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("location")
                .then(|| v.trim().to_owned())
        })
        .unwrap_or_default();
    (head.lines().next().unwrap().to_owned(), location)
}

#[test]
fn only_a_host_name_from_pebble() {
    const NAME: &str = "shop.akit.test";
    let d = dir("m2-acme-pebble");
    let (http_port, https_port) = (free_port(), free_port());
    let Some(pebble) = Pebble::start(&d, http_port, free_port()) else {
        return;
    };
    let storage = d.join("storage");
    let json = config(
        &format!(
            r#""directory": "{}", "ca_file": "{}", "storage": "{}", "timeout": "60s", "request_timeout": "10s""#,
            pebble.directory,
            pebble.ca_file.display(),
            storage.display(),
        ),
        &format!(r#" "auto_https": {{"http_port": {http_port}, "https_port": {https_port}}},"#),
        NAME,
    );
    let akit = Proc::start(&d.join("akit.json"), &json, None, "5s", "akit: listener");

    // The certificate obtained through HTTP-01 on the `http` listener and kept in the storage.
    let chain = stored(&storage, NAME, Duration::from_secs(60), &akit);
    assert_eq!(dns_names(&chain[0]), [NAME]);
    // Served on `https` once published: the chain checked up to Pebble's root, and up to the
    // issuing intermediate alone, as the staging test checks it.
    let https = SocketAddr::from(([127, 0, 0, 1], https_port));
    let deadline = Instant::now() + Duration::from_secs(30);
    while get_h1(https, &client(pebble.roots(), "http/1.1"), NAME, NAME, "/").is_err() {
        assert!(Instant::now() < deadline, "{NAME} not served over HTTPS");
        std::thread::sleep(Duration::from_millis(100));
    }
    serves(https, NAME, || pebble.roots(), &chain[0]);
    serves(https, NAME, || anchored_at_issuer(&chain), &chain[0]);
    // Everything else in the clear is sent to HTTPS.
    let (status, location) = plain(SocketAddr::from(([127, 0, 0, 1], http_port)), NAME, "/page");
    assert!(status.starts_with("HTTP/1.1 308"), "{status}");
    assert_eq!(location, format!("https://{NAME}:{https_port}/page"));

    akit.stop();
    drop(pebble);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn only_a_host_name_from_lets_encrypt_staging() {
    let Ok(name) = std::env::var("AKIT_ACME_STAGING_NAME") else {
        eprintln!("AKIT_ACME_STAGING_NAME not set: skipped (the maintainer's step, I38)");
        return;
    };
    let d = dir("m2-acme-staging");
    let storage = d.join("storage");
    // Only the host name, and the staging CA in place of Let's Encrypt's production one.
    let json = config(
        &format!(
            r#""directory": "{STAGING}", "storage": "{}""#,
            storage.display()
        ),
        "",
        &name,
    );
    let akit = Proc::start(&d.join("akit.json"), &json, None, "5s", "akit: listener");
    let chain = stored(&storage, &name, Duration::from_secs(300), &akit);
    assert_eq!(dns_names(&chain[0]), std::slice::from_ref(&name));
    let issuer = chain[0].issuer().to_string();
    assert!(issuer.contains("(STAGING)"), "issued by {issuer}");
    eprintln!("{name}: issued by {issuer}");
    let https = SocketAddr::from(([127, 0, 0, 1], 443));
    let deadline = Instant::now() + Duration::from_secs(30);
    while get_h1(
        https,
        &client(anchored_at_issuer(&chain), "http/1.1"),
        &name,
        &name,
        "/",
    )
    .is_err()
    {
        assert!(Instant::now() < deadline, "{name} not served over HTTPS");
        std::thread::sleep(Duration::from_millis(100));
    }
    serves(https, &name, || anchored_at_issuer(&chain), &chain[0]);
    let (status, location) = plain(SocketAddr::from(([127, 0, 0, 1], 80)), &name, "/page");
    assert!(status.starts_with("HTTP/1.1 308"), "{status}");
    assert_eq!(location, format!("https://{name}/page"));
    akit.stop();
    std::fs::remove_dir_all(&d).unwrap();
}
