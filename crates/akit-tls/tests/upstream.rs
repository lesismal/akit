//! An h2 upstream's TLS loaded from its files (I54): fit's `TestTLSCAFileErrors` and
//! `TestTLSCertFileErrors`, and what loads.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

use std::path::Path;

use akit_config::Upstream;
use akit_tls::{UPSTREAM_ALPN, UpstreamTls};
use support::certs::{Ca, dir};

fn s(p: &Path) -> String {
    p.display().to_string()
}

/// The upstream of `fields`, as akit-config reads it.
fn upstream(fields: &str) -> Upstream {
    let json =
        format!(r#"{{"upstreams": [{{"name": "app", "addresses": ["127.0.0.1:1"], {fields}}}]}}"#);
    akit_config::from_json(&json, None)
        .unwrap()
        .upstreams
        .remove(0)
}

fn load_err(fields: &str) -> akit_tls::Error {
    UpstreamTls::load(&upstream(fields)).expect_err(fields)
}

#[test]
fn ca_file_errors() {
    let d = dir("upstream-tls-ca-errors");
    let empty = d.join("empty.pem");
    std::fs::write(&empty, "not a certificate").unwrap();
    let missing = d.join("missing.pem");
    for (file, want) in [
        (&missing, "No such file or directory"),
        (&empty, "holds no PEM certificate"),
    ] {
        let e = load_err(&format!(
            r#""protocol": "h2", "tls_ca_file": "{}""#,
            s(file)
        ));
        assert_eq!(e.path, "tls_ca_file", "{e}");
        assert!(e.msg.contains(want), "{e}");
        assert!(e.msg.contains(&s(file)), "{e}");
    }
}

#[test]
fn cert_file_errors() {
    let d = dir("upstream-tls-cert-errors");
    let ca = Ca::new("upstream CA");
    let (a_cert, a_key) = ca.leaf(&d, "a", &["a"]);
    let (_, b_key) = ca.leaf(&d, "b", &["b"]);
    let missing = d.join("missing.pem");
    for (cert, key, want) in [
        (&missing, &a_key, "No such file or directory"),
        (&a_cert, &missing, "No such file or directory"),
        (&a_cert, &b_key, "private key does not match"),
        (&a_key, &a_key, "no certificate in PEM input"),
    ] {
        let e = load_err(&format!(
            r#""protocol": "h2", "tls_cert_file": "{}", "tls_key_file": "{}""#,
            s(cert),
            s(key)
        ));
        // Anything about the certificate or its key, at tls_cert_file, as fit reports it.
        assert_eq!(e.path, "tls_cert_file", "{cert:?}, {key:?}: {e}");
        assert!(e.msg.contains(want), "{cert:?}, {key:?}: {e}");
    }
}

#[test]
fn what_loads() {
    let d = dir("upstream-tls-loads");
    let ca = Ca::new("upstream CA");
    let ca_file = ca.write(&d, "ca.pem");
    let (cert, key) = ca.leaf(&d, "client", &["akit"]);
    // Nothing to load: not h2, or h2 with no file (the pool keeps the system's roots).
    for fields in [
        r#""protocol": "h2c""#,
        r#""protocol": "h2""#,
        r#""protocol": "h2", "tls_server_name": "app.internal""#,
    ] {
        assert!(
            UpstreamTls::load(&upstream(fields)).unwrap().is_none(),
            "{fields}"
        );
    }
    // A CA file, then a client certificate as well: their files and sums, and ALPN as fit's.
    let t = UpstreamTls::load(&upstream(&format!(
        r#""protocol": "h2", "tls_ca_file": "{}""#,
        s(&ca_file)
    )))
    .unwrap()
    .unwrap();
    assert_eq!(t.files().keys().collect::<Vec<_>>(), [&s(&ca_file)]);
    let alpn: Vec<&[u8]> = t
        .config()
        .alpn_protocols()
        .iter()
        .map(Vec::as_slice)
        .collect();
    let want: Vec<&[u8]> = UPSTREAM_ALPN.iter().map(|p| p.as_bytes()).collect();
    assert_eq!(alpn, want);
    let t = UpstreamTls::load(&upstream(&format!(
        r#""protocol": "h2", "tls_ca_file": "{}", "tls_cert_file": "{}", "tls_key_file": "{}""#,
        s(&ca_file),
        s(&cert),
        s(&key)
    )))
    .unwrap()
    .unwrap();
    let mut want = vec![s(&ca_file), s(&cert), s(&key)];
    want.sort();
    assert_eq!(t.files().keys().cloned().collect::<Vec<_>>(), want);
    assert_eq!(
        t.files()[&s(&ca_file)],
        akio_crypto::hash::Sha256::digest(&std::fs::read(&ca_file).unwrap())
    );
}
