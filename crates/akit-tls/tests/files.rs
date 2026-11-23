//! A listener's `tls` loaded from files: what loads, and each problem with the path fit gives it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

use std::path::Path;

use akit_config::{Certificate, ClientAuth, ClientAuthMode, Protocol, Tls, TlsVersion};
use support::certs::{Ca, dir};

fn cert(c: &Path, k: &Path) -> Certificate {
    Certificate {
        cert_file: c.display().to_string(),
        key_file: k.display().to_string(),
    }
}

fn tls(certificates: Vec<Certificate>, client_auth: Option<ClientAuth>) -> Tls {
    Tls {
        certificates,
        client_auth,
        max_version: TlsVersion::Tls13,
    }
}

const H1_H2: &[Protocol] = &[Protocol::Http11, Protocol::H2];

#[test]
fn loads_certificates_and_client_authorities() {
    let d = dir("loads");
    let ca = Ca::new("test ca");
    let (a, ak) = ca.leaf(&d, "a", &["a.example", "127.0.0.1"]);
    let (b, bk) = ca.leaf(&d, "b", &["b.example", "c.example"]);
    let roots = ca.write(&d, "ca.pem");
    let cfg = akit_tls::server_config(
        &tls(
            vec![cert(&a, &ak), cert(&b, &bk)],
            Some(ClientAuth {
                ca_files: vec![roots.display().to_string()],
                mode: ClientAuthMode::VerifyIfGiven,
            }),
        ),
        H1_H2,
    )
    .unwrap();
    assert_eq!(cfg.alpn_protocols(), [b"h2".to_vec(), b"http/1.1".to_vec()]);
}

/// A listener serving h3 gets a second configuration from the same files: ALPN h3 alone (I10).
#[test]
fn h3_gets_its_own_alpn_from_the_same_files() {
    let d = dir("h3");
    let ca = Ca::new("test ca");
    let (a, ak) = ca.leaf(&d, "a", &["a.example"]);
    let t = tls(vec![cert(&a, &ak)], None);
    let c = akit_tls::server_configs(&t, &[Protocol::Http11, Protocol::H2, Protocol::H3]).unwrap();
    assert_eq!(
        c.tcp.alpn_protocols(),
        [b"h2".to_vec(), b"http/1.1".to_vec()]
    );
    assert_eq!(c.h3.unwrap().alpn_protocols(), [b"h3".to_vec()]);
    // h3 alone: TCP's would offer HTTP/1.1, and is not served.
    let c = akit_tls::server_configs(&t, &[Protocol::H3]).unwrap();
    assert_eq!(c.h3.unwrap().alpn_protocols(), [b"h3".to_vec()]);
    // Without h3, none.
    assert!(akit_tls::server_configs(&t, H1_H2).unwrap().h3.is_none());
    // Problems are the same either way.
    let bad = tls(vec![cert(&a, &d.join("missing.key"))], None);
    let e = akit_tls::server_configs(&bad, &[Protocol::H3]).unwrap_err();
    assert_eq!(e.path, "certificates[0]");
}

#[test]
fn problems_name_the_certificate_or_file() {
    let d = dir("problems");
    let ca = Ca::new("test ca");
    let (a, ak) = ca.leaf(&d, "a", &["a.example"]);
    let (b, bk) = ca.leaf(&d, "b", &["b.example"]);
    let missing = d.join("missing.pem");
    let big = d.join("big.pem");
    std::fs::write(
        &big,
        vec![b'x'; usize::try_from(akit_tls::MAX_CERT_FILE_BYTES + 1).unwrap()],
    )
    .unwrap();
    let empty = d.join("empty.pem");
    std::fs::write(&empty, "nothing here\n").unwrap();

    let err = |t: Tls| akit_tls::server_config(&t, H1_H2).unwrap_err();
    let e = err(tls(vec![cert(&a, &ak), cert(&missing, &bk)], None));
    assert_eq!(e.path, "certificates[1]");
    assert!(
        e.msg.starts_with(&format!("open {}: ", missing.display())),
        "{e}"
    );

    let e = err(tls(vec![cert(&a, &bk)], None));
    assert_eq!(e.path, "certificates[0]");
    assert!(e.msg.contains("does not match"), "{e}");

    let e = err(tls(vec![cert(&big, &ak)], None));
    assert!(e.msg.contains("larger than 1048576 bytes"), "{e}");

    let e = err(tls(vec![cert(&empty, &ak)], None));
    assert_eq!(e.path, "certificates[0]");

    let e = err(tls(vec![cert(&a, &empty)], None));
    assert!(e.msg.starts_with(&empty.display().to_string()), "{e}");

    // An authority file with no certificate, as fit's addCerts.
    let e = err(tls(
        vec![cert(&b, &bk)],
        Some(ClientAuth {
            ca_files: vec![
                ca.write(&d, "ca.pem").display().to_string(),
                ak.display().to_string(),
            ],
            mode: ClientAuthMode::Require,
        }),
    ));
    assert_eq!(e.path, "client_auth.ca_files[1]");
    assert!(e.msg.starts_with(&ak.display().to_string()), "{e}");
}

#[test]
fn a_wildcard_past_the_first_certificate_is_not_supported_yet() {
    let d = dir("wildcard");
    let ca = Ca::new("test ca");
    let (a, ak) = ca.leaf(&d, "a", &["a.example"]);
    let (w, wk) = ca.leaf(&d, "w", &["*.example"]);
    let e =
        akit_tls::server_config(&tls(vec![cert(&a, &ak), cert(&w, &wk)], None), H1_H2).unwrap_err();
    assert_eq!(e.path, "certificates[1]");
    assert!(e.msg.starts_with(akit_config::NOT_SUPPORTED_YET), "{e}");
    // First, it is the default, which serves the names it holds.
    akit_tls::server_config(&tls(vec![cert(&w, &wk), cert(&a, &ak)], None), H1_H2).unwrap();
}
