//! End to end: `akit run` serves a route over HTTPS, with HTTP/1.1 and HTTP/2 chosen by ALPN and
//! the certificate chosen by SNI (I09), and `akit validate` checks the certificate files, as
//! fit's does.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use support::certs::{Ca, dir};
use support::https::{client, get_h1, get_h2, leaf_der};

/// The running binary, killed when dropped.
struct Akit(Child);

impl Drop for Akit {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

/// fit's TLS listener, as its templates write one, with two certificates: a.example (first, the
/// default) and b.example; a route for every host, and one for b.example.
fn config(d: &Path, a: &(PathBuf, PathBuf), b: &(PathBuf, PathBuf)) -> PathBuf {
    let json = format!(
        r#"{{
  "listeners": [{{"name": "https", "address": "127.0.0.1:0", "tls": {{
    "certificates": [
      {{"cert_file": "{}", "key_file": "{}"}},
      {{"cert_file": "{}", "key_file": "{}"}}
    ],
    "max_version": "1.3"
  }}}}],
  "routes": [
    {{"name": "hello", "match": {{"path": ["/hello"]}},
     "handler": {{"module": "respond", "params": {{"body": "hello from fit\n"}}}}}},
    {{"name": "b", "match": {{"host": ["b.example"], "path": ["/hello"]}},
     "handler": {{"module": "respond", "params": {{"body": "hello from b\n"}}}}}}
  ]
}}"#,
        s(&a.0),
        s(&a.1),
        s(&b.0),
        s(&b.1)
    );
    let p = d.join("fit.json");
    std::fs::write(&p, json).unwrap();
    p
}

fn start(config: &Path) -> (Akit, SocketAddr) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["run", "-c"])
        .arg(config)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let akit = Akit(child);
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line).unwrap();
    // fit's default protocols with tls.
    assert!(line.starts_with("akit: listener \"https\" on "), "{line:?}");
    assert!(line.trim_end().ends_with("(http/1.1, h2)"), "{line:?}");
    let addr = line.split(' ').nth(4).unwrap().parse().unwrap();
    (akit, addr)
}

#[test]
fn a_route_over_https_with_http1_h2_and_sni() {
    let d = dir("e2e-https");
    let ca = Ca::new("akit e2e ca");
    let roots = ca.write(&d, "ca.pem");
    let a = ca.leaf(&d, "a", &["a.example"]);
    let b = ca.leaf(&d, "b", &["b.example"]);
    let (_akit, addr) = start(&config(&d, &a, &b));

    // HTTP/1.1, by ALPN.
    let seen = get_h1(
        addr,
        &client(&roots, &["http/1.1"], None),
        "a.example",
        "a.example",
        "/hello",
    )
    .unwrap();
    assert_eq!(seen.alpn.as_deref(), Some("http/1.1"));
    assert_eq!(seen.version, Some("TLSv1_3"), "max_version 1.3");
    assert_eq!(
        (seen.status.as_str(), seen.body.as_str()),
        ("HTTP/1.1 200 OK", "hello from fit\n")
    );
    assert_eq!(seen.leaf, leaf_der(&a.0));

    // HTTP/2, by ALPN.
    assert_eq!(
        get_h2(
            addr,
            &client(&roots, &["h2", "http/1.1"], None),
            "a.example",
            "/hello"
        ),
        Ok((200, "hello from fit\n".into()))
    );

    // By SNI: b.example's certificate, and its route, over both.
    let seen = get_h1(
        addr,
        &client(&roots, &["http/1.1"], None),
        "b.example",
        "b.example",
        "/hello",
    )
    .unwrap();
    assert_eq!(seen.leaf, leaf_der(&b.0));
    assert_eq!(seen.body, "hello from b\n");
    assert_eq!(
        get_h2(addr, &client(&roots, &["h2"], None), "b.example", "/hello"),
        Ok((200, "hello from b\n".into()))
    );
}

#[test]
fn validate_and_run_check_the_certificate_files() {
    let d = dir("e2e-https-bad");
    let ca = Ca::new("akit e2e ca");
    let a = ca.leaf(&d, "a", &["a.example"]);
    let missing = (d.join("missing.pem"), d.join("missing.key"));
    let path = config(&d, &a, &missing);
    for cmd in ["validate", "run"] {
        let out = Command::new(env!("CARGO_BIN_EXE_akit"))
            .args([cmd, "-c"])
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{cmd}");
        let err = String::from_utf8(out.stderr).unwrap();
        assert!(
            err.starts_with(&format!(
                "{}: listeners[0].tls.certificates[1]: open {}: ",
                path.display(),
                missing.0.display()
            )),
            "{cmd}: {err}"
        );
    }
}

/// fit's `TestTLSMaxVersion`: `tls.max_version: "1.2"` caps a listener's TCP connections at
/// TLS 1.2 (akio's `tls12_only`, akio API request A9), over HTTP/1.1 and HTTP/2 alike; a client
/// that offers TLS 1.3 alone is refused.
#[test]
fn max_version_1_2_caps_tcp_at_tls_1_2() {
    let d = dir("e2e-https-tls12");
    let ca = Ca::new("akit e2e ca");
    let roots = ca.write(&d, "ca.pem");
    let a = ca.leaf(&d, "a", &["a.example"]);
    let b = ca.leaf(&d, "b", &["b.example"]);
    let cfg = config(&d, &a, &b);
    let json = std::fs::read_to_string(&cfg)
        .unwrap()
        .replace(r#""max_version": "1.3""#, r#""max_version": "1.2""#);
    std::fs::write(&cfg, json).unwrap();
    let (_akit, addr) = start(&cfg);
    let seen = get_h1(
        addr,
        &client(&roots, &["http/1.1"], None),
        "a.example",
        "a.example",
        "/hello",
    )
    .unwrap();
    assert_eq!(seen.version, Some("TLSv1_2"));
    assert_eq!(seen.body, "hello from fit\n");
    assert_eq!(
        get_h2(addr, &client(&roots, &["h2"], None), "a.example", "/hello"),
        Ok((200, "hello from fit\n".into()))
    );
    let mut roots13 = akio::tls::RootCerts::empty();
    roots13.add_pem(&std::fs::read(&roots).unwrap()).unwrap();
    let only13 = akio::tls::ClientConfig::builder()
        .roots(roots13)
        .alpn(["http/1.1"])
        .tls13_only(true)
        .build()
        .unwrap();
    assert!(get_h1(addr, &only13, "a.example", "a.example", "/hello").is_err());
}
