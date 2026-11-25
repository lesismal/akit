//! OCSP stapling (I32), fit's `tlsmgr/ocsp_test.go`: responses fetched, stapled, refreshed
//! halfway through their validity, kept while the responder is down and dropped once expired;
//! revoked certificates reported; chains shared; fetches bounded; certificates from files
//! stapled only under trusted roots and responders at addresses allowed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod support;

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use akio::tls::CertifiedKey;
use akio_x509::{Certificate, OcspResponse, OcspValidation, RevocationStatus, RootStore};
use akit_config::{Protocol, Tls, TlsVersion};
use akit_tls::ocsp::OCSP_WORKERS;
use akit_tls::{ListenerTls, Ocsp, Staple};
use support::certs::dir;
use support::ocsp::Responder;

/// What a stapler reports.
#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<String>>>);

impl Lines {
    fn count(&self, s: &str) -> usize {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.contains(s))
            .count()
    }
}

fn stapler(lines: &Lines) -> Ocsp {
    let o = Ocsp::new();
    let l = lines.clone();
    o.set_log(Arc::new(move |line: &str| {
        eprintln!("ocsp: {line}");
        l.0.lock().unwrap().push(line.to_owned());
    }));
    o.set_backoff(Duration::from_millis(20), Duration::from_millis(200));
    o
}

fn eventually(what: &str, within: Duration, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + within;
    while !f() {
        assert!(Instant::now() < deadline, "no {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A certificate the responder answers for, as a listener loads it.
fn issue(r: &Responder, d: &Path, name: &str) -> (CertifiedKey, Certificate) {
    let (c, k, leaf) = r.ca.leaf_with_ocsp(d, name, &[name], &r.url);
    r.know(&leaf);
    let key =
        CertifiedKey::from_pem(&std::fs::read(c).unwrap(), &std::fs::read(k).unwrap()).unwrap();
    (key, leaf)
}

/// The status of `st`'s stapled response, or `None` for none.
fn stapled(st: &Staple, r: &Responder, leaf: &Certificate) -> Option<RevocationStatus> {
    let raw = st.response()?;
    let resp = OcspResponse::from_der(raw.to_vec()).unwrap();
    Some(
        resp.verify(leaf, &r.ca.cert, &OcspValidation::new())
            .unwrap(),
    )
}

const GOOD: Option<RevocationStatus> = Some(RevocationStatus::Good);

#[test]
fn staples_a_response() {
    let d = dir("ocsp-staple");
    let r = Responder::start();
    let lines = Lines::default();
    let o = stapler(&lines);
    let (cert, leaf) = issue(&r, &d, "a.akit.test");
    let st = o
        .add(&cert, None)
        .expect("a staple for a certificate naming a responder");
    eventually("stapled response", Duration::from_secs(5), || {
        stapled(&st, &r, &leaf) == GOOD
    });
    assert_eq!(
        st.certificate().chain_der().next(),
        cert.chain_der().next(),
        "the same certificate, stapled"
    );

    // A certificate that names no responder, or has no issuer, gets none.
    let (c, k) = r.ca.leaf(&d, "b", &["b.akit.test"]);
    let plain =
        CertifiedKey::from_pem(&std::fs::read(c).unwrap(), &std::fs::read(k).unwrap()).unwrap();
    assert!(o.add(&plain, None).is_none());
    assert_eq!(o.len(), 1);
    drop(st);
    assert!(o.is_empty());
    o.close();
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn refreshes_keeps_and_drops_responses() {
    let d = dir("ocsp-refresh");
    let r = Responder::start();
    r.set(|s| s.validity = 2);
    let lines = Lines::default();
    let o = stapler(&lines);
    let (cert, leaf) = issue(&r, &d, "a.akit.test");
    let st = o.add(&cert, None).unwrap();
    eventually("stapled response", Duration::from_secs(5), || {
        stapled(&st, &r, &leaf) == GOOD
    });
    let first = st.response().unwrap();
    // Fetched again halfway through its two seconds.
    eventually("refreshed response", Duration::from_secs(5), || {
        st.response().is_some_and(|now| now != first)
    });

    // The responder goes down: the response is kept while it is valid, and dropped once it is
    // not.
    r.set(|s| s.fail = true);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(stapled(&st, &r, &leaf), GOOD, "dropped while valid");
    eventually("expired response dropped", Duration::from_secs(5), || {
        st.response().is_none()
    });
    assert!(lines.count("trying again") > 0, "failures not reported");
    // Back up, the response is stapled again.
    r.set(|s| s.fail = false);
    eventually("response again", Duration::from_secs(5), || {
        stapled(&st, &r, &leaf) == GOOD
    });
    drop(st);
    o.close();
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_revoked_certificate_is_reported() {
    let d = dir("ocsp-revoked");
    let r = Responder::start();
    let lines = Lines::default();
    let o = stapler(&lines);
    let (cert, leaf) = issue(&r, &d, "a.akit.test");
    r.set(|s| s.revoked.push(leaf.serial().to_vec()));
    let heard = Arc::new(AtomicUsize::new(0));
    let h = Arc::clone(&heard);
    let st = o
        .add(
            &cert,
            Some(Arc::new(move || {
                h.fetch_add(1, Ordering::Relaxed);
            })),
        )
        .unwrap();
    eventually("revocation heard", Duration::from_secs(5), || {
        heard.load(Ordering::Relaxed) == 1
    });
    assert!(
        st.response().is_none(),
        "a revoked certificate's response stapled"
    );
    assert!(lines.count("revoked") > 0);
    drop(st);
    o.close();
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn chains_are_shared() {
    let d = dir("ocsp-shared");
    let r = Responder::start();
    let lines = Lines::default();
    let o = stapler(&lines);
    let (cert, leaf) = issue(&r, &d, "a.akit.test");
    // The same chain loaded again, as a reload's graph loads its files.
    let again = cert.clone();
    let (a, b) = (o.add(&cert, None).unwrap(), o.add(&again, None).unwrap());
    eventually("stapled response", Duration::from_secs(5), || {
        stapled(&a, &r, &leaf) == GOOD && b.response().is_some()
    });
    assert_eq!(
        r.requests.load(Ordering::Relaxed),
        1,
        "one chain asked for once"
    );
    drop(a);
    assert_eq!(o.len(), 1, "released while still added");
    assert!(b.response().is_some());
    drop(b);
    assert!(o.is_empty(), "kept after its last release");
    o.close();
    std::fs::remove_dir_all(&d).unwrap();
}

/// A responder that holds every request: no more than OCSP_WORKERS are made at once, and close
/// does not wait for them.
#[test]
fn a_slow_responder_is_bounded() {
    let d = dir("ocsp-slow");
    let r = Responder::start();
    r.set(|s| s.hold = true);
    let lines = Lines::default();
    let o = stapler(&lines);
    let staples: Vec<Staple> = (0..6)
        .map(|i| {
            let (c, _) = issue(&r, &d, &format!("s{i}.akit.test"));
            o.add(&c, None).unwrap()
        })
        .collect();
    eventually("responder busy", Duration::from_secs(5), || {
        r.in_flight.load(Ordering::Relaxed) == OCSP_WORKERS
    });
    std::thread::sleep(Duration::from_millis(200));
    assert!(r.most_in_flight.load(Ordering::Relaxed) <= OCSP_WORKERS);
    assert!(staples.iter().all(|s| s.response().is_none()));
    let t = Instant::now();
    o.close();
    assert!(
        t.elapsed() < Duration::from_secs(3),
        "close took {:?}",
        t.elapsed()
    );
    drop(staples);
    r.release();
    std::fs::remove_dir_all(&d).unwrap();
}

fn files_tls(cert: &Path, key: &Path, other: (&Path, &Path)) -> Tls {
    Tls {
        certificates: vec![
            akit_config::Certificate {
                cert_file: cert.display().to_string(),
                key_file: key.display().to_string(),
            },
            akit_config::Certificate {
                cert_file: other.0.display().to_string(),
                key_file: other.1.display().to_string(),
            },
        ],
        client_auth: None,
        max_version: TlsVersion::Tls13,
        acme: Vec::new(),
        on_demand: false,
    }
}

/// fit's `TestStaticStaple`: a listener's certificate from files is stapled under the roots
/// trusted, one without a responder served as it is, and the listener's TLS dropped gives its
/// staples back.
#[test]
fn certificates_from_files_under_trusted_roots() {
    let d = dir("ocsp-files");
    let r = Responder::start();
    let lines = Lines::default();
    let o = stapler(&lines);
    let mut roots = RootStore::new();
    roots.add(r.ca.cert.clone());
    // The test CA's own, at a loopback address.
    o.trust_roots(roots, true);
    let (c, k, leaf) = r.ca.leaf_with_ocsp(&d, "a", &["a.akit.test"], &r.url);
    r.know(&leaf);
    let (c2, k2) = r.ca.leaf(&d, "b", &["b.akit.test"]);
    let t = ListenerTls::load(&files_tls(&c, &k, (&c2, &k2)), &[Protocol::Http11])
        .unwrap()
        .with_ocsp(&o);
    let st = t
        .staple(0)
        .expect("a staple for the certificate naming a responder");
    eventually("stapled response", Duration::from_secs(5), || {
        stapled(st, &r, &leaf) == GOOD
    });
    assert!(t.staple(1).is_none());
    drop(t);
    assert!(o.is_empty(), "the listener's staples kept");
    o.close();
    std::fs::remove_dir_all(&d).unwrap();
}

/// A certificate from files whose chain does not verify against the trusted roots (none are,
/// by default) has its responder never asked; said once.
#[test]
fn certificates_from_files_not_trusted_are_not_asked() {
    let d = dir("ocsp-files-untrusted");
    let r = Responder::start();
    let lines = Lines::default();
    let o = stapler(&lines);
    let (c, k, leaf) = r.ca.leaf_with_ocsp(&d, "a", &["a.akit.test"], &r.url);
    r.know(&leaf);
    let (c2, k2) = r.ca.leaf(&d, "b", &["b.akit.test"]);
    let t = ListenerTls::load(&files_tls(&c, &k, (&c2, &k2)), &[Protocol::Http11])
        .unwrap()
        .with_ocsp(&o);
    eventually("report", Duration::from_secs(5), || {
        lines.count("no response stapled") == 1
    });
    // Without roots given, the chain was verified against the system's (fit's
    // x509.SystemCertPool), which do not hold this test CA.
    assert_eq!(lines.count("does not verify against the trusted roots"), 1);
    // Another CA's roots: still not.
    let mut roots = RootStore::new();
    roots.add(support::certs::Ca::new("another CA").cert);
    o.trust_roots(roots, true);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(r.requests.load(Ordering::Relaxed), 0);
    assert!(t.staple(0).unwrap().response().is_none());
    assert_eq!(lines.count("no response stapled"), 1, "said more than once");
    drop(t);
    o.close();
    std::fs::remove_dir_all(&d).unwrap();
}

/// fit's `TestStaticStapleSSRF`: a certificate from files whose responder is at a loopback
/// address is not fetched unless private addresses are allowed.
#[test]
fn responders_of_files_at_private_addresses_are_not_asked() {
    let d = dir("ocsp-files-ssrf");
    let r = Responder::start();
    let lines = Lines::default();
    let o = stapler(&lines);
    let mut roots = RootStore::new();
    roots.add(r.ca.cert.clone());
    o.trust_roots(roots, false);
    let (c, k, leaf) = r.ca.leaf_with_ocsp(&d, "a", &["a.akit.test"], &r.url);
    r.know(&leaf);
    let (c2, k2) = r.ca.leaf(&d, "b", &["b.akit.test"]);
    let t = ListenerTls::load(&files_tls(&c, &k, (&c2, &k2)), &[Protocol::Http11])
        .unwrap()
        .with_ocsp(&o);
    eventually("refusal", Duration::from_secs(5), || {
        lines.count("is not public") > 0
    });
    assert_eq!(r.requests.load(Ordering::Relaxed), 0);
    assert!(t.staple(0).unwrap().response().is_none());
    // The same certificate from ACME, whose CA the operator chose, is asked.
    let key = t.certs().certificates().next().unwrap().clone();
    let st = o.add(&key, None).unwrap();
    eventually("stapled response", Duration::from_secs(5), || {
        stapled(&st, &r, &leaf) == GOOD
    });
    drop((st, t));
    o.close();
    std::fs::remove_dir_all(&d).unwrap();
}

/// fit's `listener/ocsp_test.go`: a client asking for the certificate's status in a handshake
/// with the listener's configuration gets the response stapled.
#[test]
fn stapled_in_a_handshake() {
    use akio::tls::stream::Stream;
    use akio::tls::{ClientConfig, RootCerts};
    use std::net::{TcpListener, TcpStream};

    let d = dir("ocsp-handshake");
    let r = Responder::start();
    let lines = Lines::default();
    let o = stapler(&lines);
    let mut roots = RootStore::new();
    roots.add(r.ca.cert.clone());
    o.trust_roots(roots, true);
    let (c, k, leaf) = r.ca.leaf_with_ocsp(&d, "a", &["a.akit.test"], &r.url);
    r.know(&leaf);
    let (c2, k2) = r.ca.leaf(&d, "b", &["b.akit.test"]);
    let t = ListenerTls::load(&files_tls(&c, &k, (&c2, &k2)), &[Protocol::Http11])
        .unwrap()
        .with_ocsp(&o);
    eventually("stapled response", Duration::from_secs(5), || {
        t.staple(0).unwrap().response().is_some()
    });
    let server = t.server_configs(Arc::new(t.clone())).unwrap().tcp;
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let accept = std::thread::spawn(move || {
        for _ in 0..2 {
            let (s, _) = l.accept().unwrap();
            let mut s = Stream::server(&server, s);
            let _ = s.handshake();
        }
    });
    let mut tls_roots = RootCerts::empty();
    tls_roots.add_pem(r.ca.cert.to_pem().as_bytes()).unwrap();
    for (name, want) in [("a.akit.test", true), ("b.akit.test", false)] {
        let cfg = ClientConfig::builder()
            .roots(tls_roots.clone())
            .ocsp_stapling(true)
            .build()
            .unwrap();
        let tcp = TcpStream::connect(addr).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut s = Stream::client(&cfg, name, tcp).unwrap();
        s.handshake().unwrap();
        let got = s.conn().ocsp_response().map(<[u8]>::to_vec);
        if want {
            let resp = OcspResponse::from_der(got.expect("no response stapled")).unwrap();
            let status = resp
                .verify(&leaf, &r.ca.cert, &OcspValidation::new())
                .unwrap();
            assert_eq!(status, RevocationStatus::Good);
        } else {
            assert!(
                got.is_none(),
                "{name}: a response stapled to a certificate without one"
            );
        }
    }
    accept.join().unwrap();
    drop(t);
    o.close();
    std::fs::remove_dir_all(&d).unwrap();
}
