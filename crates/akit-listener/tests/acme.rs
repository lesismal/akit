//! ACME (I31, I32) against Pebble: a certificate obtained for a name a TLS listener lists in
//! `tls.acme`, through HTTP-01 answered by a listener in the clear, then served to a handshake
//! for that name; and kept in the storage, where a restart finds it without asking the CA.
//! Through TLS-ALPN-01, answered by the TLS listener itself; a challenge nobody answers followed
//! by the next; a certificate renewed ahead of its expiry and served in place of the last, and
//! when the CA's renewal information (ARI) says. On
//! the backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`). Skipped without Pebble's
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
use akit_config::Challenge;
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
    manager_with(p, storage, &[])
}

/// A manager answering `challenges` (the default when empty).
fn manager_with(p: &Pebble, storage: &Path, challenges: &[Challenge]) -> Acme {
    let mut c = akit_config::Acme {
        directory: p.directory.clone(),
        ca_file: p.ca_file.display().to_string(),
        storage: storage.display().to_string(),
        timeout: Duration::from_secs(60),
        request_timeout: Duration::from_secs(10),
        ..akit_config::Acme::default()
    };
    if !challenges.is_empty() {
        c.challenges = challenges.to_vec();
    }
    let a = Acme::new(&c).unwrap();
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

/// Listeners serving `json`'s config, their names obtained by `acme`, and what each serves at.
struct Served {
    engine: Arc<akio::Engine>,
    listeners: Option<Listeners>,
    addrs: Vec<std::net::SocketAddr>,
    names: Vec<String>,
}

impl Served {
    fn start(json: &str, acme: &Acme) -> Served {
        let config = akit_config::from_json(json, None).unwrap();
        let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
        let graph = compile(
            &config,
            &registry,
            &Env::default().with_acme(Some(acme.clone())),
        )
        .unwrap();
        let names = graph.acme_names().to_vec();
        let cell = GraphCell::new(graph);
        let engine = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
        engine.start().unwrap();
        let l = Listeners::new(Arc::clone(&engine), cell, &options());
        let addrs = l.served().iter().map(|s| s.address).collect();
        Served {
            engine,
            listeners: Some(l),
            addrs,
            names,
        }
    }

    fn stop(mut self) {
        drop(self.listeners.take());
        assert!(akit_listener::shutdown(
            &self.engine,
            Duration::from_secs(5)
        ));
    }
}

fn client(p: &Pebble, alpn: &[&str]) -> ClientConfig {
    ClientConfig::builder()
        .roots(p.roots())
        .alpn(alpn.iter().copied())
        .build()
        .unwrap()
}

/// fit's `TestPebbleTLSALPN` and `TestACMETLSALPNFromPebble`: a TLS listener on the port Pebble
/// checks TLS-ALPN-01 on, and no listener in the clear: the certificate is obtained through the
/// challenge the listener answers itself, and served; the challenge's certificate is not kept
/// once it is over.
#[test]
fn obtains_through_tls_alpn_01() {
    let d = dir("acme-tls-alpn");
    let tls_port = free_port();
    let Some(pebble) = Pebble::start_with(&d, free_port(), tls_port, 7_776_000) else {
        return;
    };
    let acme = manager_with(&pebble, &d.join("storage"), &[Challenge::TlsAlpn01]);
    let s = Served::start(
        &format!(
            r#"{{"listeners": [
                  {{"name": "secure", "address": "127.0.0.1:{tls_port}", "protocols": ["http/1.1"],
                    "tls": {{"acme": ["{NAME}"]}}}}],
                "routes": [{{"handler": {{"module": "respond", "params": {{"body": "over alpn"}}}}}}]}}"#
        ),
        &acme,
    );
    acme.manage(&s.names);
    let key = obtained(&acme, NAME);
    assert!(
        acme.alpn_challenge(NAME).is_none(),
        "the challenge's certificate kept"
    );
    let seen = get_h1(s.addrs[0], &client(&pebble, &["http/1.1"]), NAME, NAME, "/").unwrap();
    assert_eq!(seen.leaf, key.chain_der().next().unwrap());
    assert_eq!(seen.body, "over alpn");
    // A client offering acme-tls/1 alone, with no challenge under way, gets no certificate.
    let c = ClientConfig::builder()
        .insecure_skip_verify()
        .alpn(["acme-tls/1"])
        .build()
        .unwrap();
    assert!(get_h1(s.addrs[0], &c, NAME, NAME, "/").is_err());
    s.stop();
    acme.close();
    std::fs::remove_dir_all(&d).unwrap();
}

/// fit's `TestChallengeFallback`: a CA checking first a challenge nobody answers (TLS-ALPN-01, on
/// a port nothing listens on): the attempt after the failure starts with the next, HTTP-01, which
/// is answered.
#[test]
fn a_failed_challenge_is_followed_by_the_next() {
    let d = dir("acme-fallback");
    let http_port = free_port();
    let Some(pebble) = Pebble::start_with(&d, http_port, free_port(), 7_776_000) else {
        return;
    };
    let acme = manager_with(
        &pebble,
        &d.join("storage"),
        &[Challenge::TlsAlpn01, Challenge::Http01],
    );
    let failures = Arc::new(AtomicUsize::new(0));
    let f = Arc::clone(&failures);
    acme.set_log(Arc::new(move |line: &str| {
        eprintln!("acme: {line}");
        if line.contains("trying again") {
            f.fetch_add(1, Ordering::Relaxed);
        }
    }));
    acme.set_backoff(Duration::from_millis(100), Duration::from_millis(200));
    let s = Served::start(
        &format!(
            r#"{{"listeners": [
                  {{"name": "plain", "address": "127.0.0.1:{http_port}", "protocols": ["http/1.1"]}},
                  {{"name": "secure", "address": "127.0.0.1:0", "protocols": ["http/1.1"],
                    "tls": {{"acme": ["{NAME}"]}}}}],
                "routes": [{{"handler": {{"module": "respond", "params": {{"body": "hello"}}}}}}]}}"#
        ),
        &acme,
    );
    acme.manage(&s.names);
    let key = obtained(&acme, NAME);
    assert!(
        failures.load(Ordering::Relaxed) >= 1,
        "the first attempt did not fail"
    );
    let seen = get_h1(s.addrs[1], &client(&pebble, &["http/1.1"]), NAME, NAME, "/").unwrap();
    assert_eq!(seen.leaf, key.chain_der().next().unwrap());
    s.stop();
    acme.close();
    std::fs::remove_dir_all(&d).unwrap();
}

/// Renewal: a certificate valid for 30 seconds is renewed before it expires, stored, and
/// handshakes are given the new one. Pebble suggests (ARI) a window from a third of the life before
/// the end, a day either side, cut to the certificate's life: for one this short, all of it, so
/// the renewal comes from ARI at once; the renewal by lifetime alone (a third left, less up to a
/// fiftieth) is `renew_at`'s unit test.
#[test]
fn renews_ahead_of_expiry() {
    let d = dir("acme-renew");
    let http_port = free_port();
    let Some(pebble) = Pebble::start_with(&d, http_port, free_port(), 30) else {
        return;
    };
    let storage = d.join("storage");
    let acme = manager(&pebble, &storage);
    let s = Served::start(
        &format!(
            r#"{{"listeners": [
                  {{"name": "plain", "address": "127.0.0.1:{http_port}", "protocols": ["http/1.1"]}},
                  {{"name": "secure", "address": "127.0.0.1:0", "protocols": ["http/1.1"],
                    "tls": {{"acme": ["{NAME}"]}}}}],
                "routes": [{{"handler": {{"module": "respond", "params": {{"body": "hello"}}}}}}]}}"#
        ),
        &acme,
    );
    let start = Instant::now();
    acme.manage(&s.names);
    let first = obtained(&acme, NAME).chain_der().next().unwrap().to_vec();
    // Served, verified against Pebble's root (it may be renewed already).
    let c = client(&pebble, &["http/1.1"]);
    get_h1(s.addrs[1], &c, NAME, NAME, "/").unwrap();
    let leaf = akio_x509::Certificate::from_der(first.clone()).unwrap();
    let life = leaf.not_after() - leaf.not_before();
    assert!(life <= 31, "Pebble's certificate lives {life}s");
    // Renewed once two thirds of its life have passed, give or take a fiftieth.
    let deadline = Instant::now() + Duration::from_secs(40);
    let renewed = loop {
        let now = acme
            .certificate(NAME)
            .unwrap()
            .chain_der()
            .next()
            .unwrap()
            .to_vec();
        if now != first {
            break now;
        }
        assert!(
            Instant::now() < deadline,
            "not renewed: {:?}",
            acme.last_error(NAME)
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    let took = start.elapsed().as_secs_f64();
    assert!(took < 30.0, "renewed after {took}s, once the first expired");
    // A new client: one resuming its session would be sent no certificate.
    let c = client(&pebble, &["http/1.1"]);
    // Renewed again since, maybe: Pebble's window has always begun for so short a life.
    let served = get_h1(s.addrs[1], &c, NAME, NAME, "/").unwrap().leaf;
    assert_ne!(served, first, "the first served after its renewal");
    drop(renewed);
    // Stored: what a restart finds.
    let stored = std::fs::read(
        std::fs::read_dir(storage.join("certificates"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
            .join(format!("{NAME}.pem")),
    )
    .unwrap();
    let stored = akio::tls::Certificate::from_pem(&stored).unwrap();
    assert_ne!(stored[0].as_der(), &first[..], "the first still stored");
    s.stop();
    acme.close();
    std::fs::remove_dir_all(&d).unwrap();
}

/// fit's `TestPebbleARI`: the window Pebble suggests for a certificate is within its lifetime;
/// once Pebble moves it into the past, the certificate is renewed.
#[test]
fn renews_when_ari_says() {
    let d = dir("acme-ari");
    let http_port = free_port();
    let Some(pebble) = Pebble::start(&d, http_port) else {
        return;
    };
    let acme = manager_with(&pebble, &d.join("storage"), &[Challenge::Http01]);
    // Pebble says six hours.
    acme.set_ari_bounds(Duration::from_millis(100), Duration::from_millis(200));
    let s = Served::start(
        &format!(
            r#"{{"listeners": [
                  {{"name": "plain", "address": "127.0.0.1:{http_port}", "protocols": ["http/1.1"]}},
                  {{"name": "secure", "address": "127.0.0.1:0", "protocols": ["http/1.1"],
                    "tls": {{"acme": ["{NAME}"]}}}}],
                "routes": [{{"handler": {{"module": "respond", "params": {{"body": "hello"}}}}}}]}}"#
        ),
        &acme,
    );
    acme.manage(&s.names);
    let first = obtained(&acme, NAME).chain_der().next().unwrap().to_vec();
    let leaf = akio_x509::Certificate::from_der(first.clone()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let at = loop {
        if let Some(t) = acme.renewal_info(NAME) {
            break t;
        }
        assert!(Instant::now() < deadline, "no renewal information");
        std::thread::sleep(Duration::from_millis(20));
    };
    let at = i64::try_from(at.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()).unwrap();
    assert!(
        at >= leaf.not_before() && at <= leaf.not_after(),
        "renewal picked at {at}, outside the certificate's lifetime {} to {}",
        leaf.not_before(),
        leaf.not_after()
    );

    let fmt = |t: std::time::SystemTime| {
        let secs =
            i64::try_from(t.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()).unwrap();
        rfc3339(secs)
    };
    let now = std::time::SystemTime::now();
    pebble.set_renewal_info(
        &leaf.to_pem(),
        &fmt(now - Duration::from_secs(3600)),
        &fmt(now - Duration::from_secs(60)),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while acme.certificate(NAME).unwrap().chain_der().next().unwrap() == first.as_slice() {
        assert!(
            Instant::now() < deadline,
            "not renewed: {:?}",
            acme.last_error(NAME)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    s.stop();
    acme.close();
    std::fs::remove_dir_all(&d).unwrap();
}

/// Unix seconds as RFC 3339.
fn rfc3339(t: i64) -> String {
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}
