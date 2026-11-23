//! End to end: `akit run` serves a route over HTTP/3 on its TLS listener's port in UDP, and its
//! HTTP/2 and HTTP/1.1 responses announce it with `Alt-Svc` (I10), as fit's do.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::process::{Child, Command, Stdio};

use support::certs::{Ca, dir};
use support::h3;
use support::https::{client, get_h1, get_h2_alt_svc};

/// The running binary, killed when dropped.
struct Akit(Child);

impl Drop for Akit {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_route_over_h3_and_alt_svc_on_h2() {
    let d = dir("e2e-h3");
    let ca = Ca::new("akit e2e ca");
    let roots = ca.write(&d, "ca.pem");
    let (cert, key) = ca.leaf(&d, "a", &["a.example"]);
    // As fit's config reference writes one: http/1.1, h2 and h3 on one TLS listener.
    let json = format!(
        r#"{{
  "listeners": [{{"name": "secure", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2", "h3"],
    "tls": {{"certificates": [{{"cert_file": "{}", "key_file": "{}"}}]}}}}],
  "routes": [{{"match": {{"path": ["/hello"]}},
    "handler": {{"module": "respond", "params": {{"body": "hello from fit\n"}}}}}}]
}}"#,
        cert.display(),
        key.display()
    );
    let config = d.join("fit.json");
    std::fs::write(&config, json).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["run", "-c"])
        .arg(&config)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let _akit = Akit(child);
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line).unwrap();
    assert!(
        line.starts_with("akit: listener \"secure\" on "),
        "{line:?}"
    );
    assert!(line.trim_end().ends_with("(http/1.1, h2, h3)"), "{line:?}");
    // TCP's address; HTTP/3 listens on the same port in UDP.
    let addr: SocketAddr = line.split(' ').nth(4).unwrap().parse().unwrap();
    let alt_svc = format!("h3=\":{}\"; ma=86400", addr.port());

    let (status, body, seen) =
        get_h2_alt_svc(addr, &client(&roots, &["h2"], None), "a.example", "/hello").unwrap();
    assert_eq!((status, body.as_str()), (200, "hello from fit\n"));
    assert_eq!(seen.as_deref(), Some(alt_svc.as_str()));
    let seen = get_h1(
        addr,
        &client(&roots, &["http/1.1"], None),
        "a.example",
        "a.example",
        "/hello",
    )
    .unwrap();
    assert_eq!(seen.alt_svc.as_deref(), Some(alt_svc.as_str()));

    let seen = h3::get(addr, h3::client(&roots, None), "a.example", "/hello").unwrap();
    assert_eq!(seen.alpn.as_deref(), Some("h3"));
    assert_eq!((seen.status, seen.body.as_str()), (200, "hello from fit\n"));
}
