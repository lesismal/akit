//! ACME (I31) against Pebble: a certificate obtained for a name a TLS listener lists in
//! `tls.acme`, through HTTP-01 answered by a listener in the clear, then served to a handshake
//! for that name; and kept in the storage, where a restart finds it without asking the CA. On the
//! backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`). Skipped without Pebble's
//! binaries (support/pebble.rs).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod support;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use akio::tls::ClientConfig;
use akio::{Backend, UringConfig};
use akit_listener::Listeners;
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};
use akit_tls::Acme;
use support::certs::dir;
use support::https::get_h1;
use support::pebble::{Pebble, free_port};

const NAME: &str = "acme.akit.test";

fn options() -> akit_listener::Options {
    let mut o = akit_listener::Options::default();
    o.backend = match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    };
    o.shutdown_timeout = Duration::from_secs(5);
    o
}

fn manager(p: &Pebble, storage: &Path) -> Acme {
    let a = Acme::new(&akit_config::Acme {
        directory: p.directory.clone(),
        ca_file: p.ca_file.display().to_string(),
        storage: storage.display().to_string(),
        timeout: Duration::from_secs(60),
        request_timeout: Duration::from_secs(10),
        ..akit_config::Acme::default()
    })
    .unwrap();
    a.set_log(Arc::new(|line: &str| eprintln!("acme: {line}")));
    a
}

/// Waits for `acme` to have a certificate for `name`.
fn obtained(acme: &Acme, name: &str) -> akio::tls::CertifiedKey {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(k) = acme.certificate(name) {
            return k;
        }
        assert!(
            Instant::now() < deadline,
            "no certificate for {name}: {:?}",
            acme.last_error(name)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A listener in the clear on the port Pebble checks HTTP-01 on, and a TLS one listing `NAME`
/// in `tls.acme` with no certificate from files: the certificate is obtained, served, and found
/// in the storage by a manager started later, with the CA gone.
#[test]
fn obtains_serves_and_keeps_a_certificate() {
    let d = dir("acme-obtain");
    let http_port = free_port();
    let Some(pebble) = Pebble::start(&d, http_port) else {
        return;
    };
    let storage = d.join("storage");
    let acme = manager(&pebble, &storage);
    let json = format!(
        r#"{{"listeners": [
              {{"name": "plain", "address": "127.0.0.1:{http_port}", "protocols": ["http/1.1"]}},
              {{"name": "secure", "address": "127.0.0.1:0", "protocols": ["http/1.1"],
                "tls": {{"acme": ["ACME.akit.test"]}}}}],
            "routes": [{{"handler": {{"module": "respond", "params": {{"body": "hello"}}}}}}]}}"#
    );
    let config = akit_config::from_json(&json, None).unwrap();
    let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
    let graph = compile(
        &config,
        &registry,
        &Env::default().with_acme(Some(acme.clone())),
    )
    .unwrap();
    assert_eq!(graph.acme_names(), [NAME]);
    let names = graph.acme_names().to_vec();
    let cell = GraphCell::new(graph);
    let e = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
    e.start().unwrap();
    let l = Listeners::new(Arc::clone(&e), cell.clone(), &options());
    let secure = l.served()[1].address;
    let plain = l.served()[0].address;

    // Until there is a certificate, a handshake for the name has none to get.
    let roots = pebble.roots();
    let c = ClientConfig::builder()
        .roots(roots)
        .alpn(["http/1.1"])
        .build()
        .unwrap();
    assert!(get_h1(secure, &c, NAME, NAME, "/").is_err());

    acme.manage(&names);
    let key = obtained(&acme, NAME);
    assert_eq!(acme.last_error(NAME), None);
    let leaf = key.chain_der().next().unwrap().to_vec();

    // Served, and verified against Pebble's root, to a client asking for the name in any case.
    for sni in [NAME, "ACME.akit.test"] {
        let seen = get_h1(secure, &c, sni, NAME, "/").unwrap();
        assert_eq!(seen.leaf, leaf, "{sni}");
        assert!(seen.status.starts_with("HTTP/1.1 200"), "{}", seen.status);
        assert_eq!(seen.body, "hello");
    }
    // The challenge is over: its path goes to the routes again.
    let r = plain_get(plain, "/.well-known/acme-challenge/none");
    assert!(r.starts_with("HTTP/1.1 200") && r.ends_with("hello"), "{r}");

    drop(l);
    assert!(akit_listener::shutdown(&e, Duration::from_secs(5)));
    acme.close();

    // A restart: the stored certificate is used as it is, without the CA.
    let c2 = akit_config::Acme {
        directory: pebble.directory.clone(),
        ca_file: pebble.ca_file.display().to_string(),
        storage: storage.display().to_string(),
        timeout: Duration::from_secs(5),
        ..akit_config::Acme::default()
    };
    drop(pebble);
    let again = Acme::new(&c2).unwrap();
    again.manage(&[NAME]);
    let key = obtained(&again, NAME);
    assert_eq!(key.chain_der().next().unwrap(), &leaf[..]);
    // Dropped from the names: no longer served.
    again.manage::<&str>(&[]);
    assert!(again.certificate(NAME).is_none());
    again.close();
    std::fs::remove_dir_all(&d).unwrap();
}

/// A CA that cannot be reached: the failure is reported, and tried again later.
#[test]
fn a_ca_out_of_reach_fails_and_is_tried_again() {
    let d = dir("acme-unreachable");
    let acme = Acme::new(&akit_config::Acme {
        // Port 1 refuses connections.
        directory: "https://127.0.0.1:1/dir".into(),
        storage: d.join("storage").display().to_string(),
        timeout: Duration::from_secs(5),
        ..akit_config::Acme::default()
    })
    .unwrap();
    let failures = Arc::new(AtomicUsize::new(0));
    let f = Arc::clone(&failures);
    acme.set_log(Arc::new(move |line: &str| {
        if line.contains("trying again") {
            f.fetch_add(1, Ordering::Relaxed);
        }
    }));
    acme.set_backoff(Duration::from_millis(100), Duration::from_millis(200));
    acme.manage(&["x.akit.test"]);
    let deadline = Instant::now() + Duration::from_secs(20);
    let first = loop {
        if let Some(e) = acme.last_error("x.akit.test") {
            break e;
        }
        assert!(Instant::now() < deadline, "no failure reported");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(first.contains("127.0.0.1:1"), "{first}");
    // Tried again, after the backoff.
    while failures.load(Ordering::Relaxed) < 2 {
        assert!(Instant::now() < deadline, "not tried again");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(acme.certificate("x.akit.test").is_none());
    // Closing gives up at once, whatever is under way.
    let t = Instant::now();
    acme.close();
    assert!(t.elapsed() < Duration::from_secs(5));
    std::fs::remove_dir_all(&d).unwrap();
}

/// A GET in the clear; the whole response.
fn plain_get(addr: std::net::SocketAddr, path: &str) -> String {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {NAME}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut r = String::new();
    s.read_to_string(&mut r).unwrap();
    r
}
