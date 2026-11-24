//! On-demand TLS (I34) through real handshakes: a TLS listener whose `tls.on_demand` is set has a
//! certificate obtained from Pebble during the handshake for a name the ask URL allows, the
//! handshakes for it waiting without holding the listener's I/O threads (handshakes and requests
//! for other names go on meanwhile); a refused name, and one past the rate, fail at once; a
//! name whose certificate does not come fails at `acme.on_demand.timeout`. fit's
//! `TestOnDemandAllowed` and the handshake side of its other on-demand tests. On the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`). The Pebble test is skipped without
//! Pebble's binaries (support/pebble.rs).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod support;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use akio::tls::ClientConfig;
use akio::tls::stream::Stream;
use akio::{Backend, UringConfig};
use akit_listener::Listeners;
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};
use akit_tls::Acme;
use support::certs::{Ca, dir};
use support::https::client;
use support::pebble::{Pebble, free_port};

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

/// An ask URL: allows the names it is told, counts what it is asked, and holds every ask while
/// `hold` is set.
struct Asker {
    addr: SocketAddr,
    asked: Arc<Mutex<HashMap<String, usize>>>,
    hold: Arc<AtomicBool>,
}

impl Asker {
    fn start(allowed: &[&str]) -> Asker {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let a = Asker {
            addr: l.local_addr().unwrap(),
            asked: Arc::default(),
            hold: Arc::default(),
        };
        let allowed: Arc<Vec<String>> = Arc::new(allowed.iter().map(|s| (*s).to_owned()).collect());
        let (asked, hold) = (Arc::clone(&a.asked), Arc::clone(&a.hold));
        std::thread::spawn(move || {
            for c in l.incoming().flatten() {
                let (allowed, asked, hold) =
                    (Arc::clone(&allowed), Arc::clone(&asked), Arc::clone(&hold));
                std::thread::spawn(move || {
                    let mut r = BufReader::new(c);
                    loop {
                        let mut line = String::new();
                        if r.read_line(&mut line).unwrap_or(0) == 0 {
                            return;
                        }
                        loop {
                            let mut h = String::new();
                            if r.read_line(&mut h).unwrap_or(0) == 0 {
                                return;
                            }
                            if h == "\r\n" {
                                break;
                            }
                        }
                        let target = line.split(' ').nth(1).unwrap_or("");
                        let name = target.rsplit("domain=").next().unwrap_or("").to_owned();
                        *asked.lock().unwrap().entry(name.clone()).or_default() += 1;
                        while hold.load(Ordering::Acquire) {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        let status = if allowed.contains(&name) {
                            "200 OK"
                        } else {
                            "403 Forbidden"
                        };
                        let mut c = r.get_ref();
                        if write!(c, "HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n").is_err() {
                            return;
                        }
                    }
                });
            }
        });
        a
    }

    fn url(&self) -> String {
        format!("http://{}/check?key=1", self.addr)
    }

    fn count(&self, name: &str) -> usize {
        self.asked.lock().unwrap().get(name).copied().unwrap_or(0)
    }
}

/// The listeners of `json`'s config, its names obtained by an ACME manager made from its `acme`.
struct Served {
    engine: Arc<akio::Engine>,
    listeners: Option<Listeners>,
    addrs: Vec<SocketAddr>,
    acme: Acme,
    obtained: Arc<Mutex<usize>>,
}

impl Served {
    fn start(json: &str) -> Served {
        let config = akit_config::from_json(json, None).unwrap();
        let acme = Acme::new(config.acme.as_ref().unwrap()).unwrap();
        let obtained = Arc::new(Mutex::new(0));
        let o = Arc::clone(&obtained);
        acme.set_log(Arc::new(move |line: &str| {
            eprintln!("acme: {line}");
            if line.contains("obtained a certificate") {
                *o.lock().unwrap() += 1;
            }
        }));
        let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
        let graph = compile(
            &config,
            &registry,
            &Env::default().with_acme(Some(acme.clone())),
        )
        .unwrap();
        let cell = GraphCell::new(graph);
        let engine = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
        engine.start().unwrap();
        let l = Listeners::new(Arc::clone(&engine), cell, &options());
        let addrs = l.served().iter().map(|s| s.address).collect();
        Served {
            engine,
            listeners: Some(l),
            addrs,
            acme,
            obtained,
        }
    }

    fn stop(mut self) {
        drop(self.listeners.take());
        assert!(akit_listener::shutdown(
            &self.engine,
            Duration::from_secs(5)
        ));
        self.acme.close();
    }
}

/// An HTTPS/1.1 GET of `/` for `name`, with a read timeout of `wait`: the leaf certificate and
/// the body.
fn get(
    addr: SocketAddr,
    cfg: &ClientConfig,
    name: &str,
    wait: Duration,
) -> std::io::Result<(Vec<u8>, String)> {
    let tcp = TcpStream::connect(addr)?;
    tcp.set_read_timeout(Some(wait))?;
    let mut s = Stream::client(cfg, name, tcp)?;
    s.handshake()?;
    let leaf = s
        .conn()
        .peer_certificates()
        .first()
        .cloned()
        .unwrap_or_default();
    write!(
        s,
        "GET / HTTP/1.1\r\nHost: {name}\r\nConnection: close\r\n\r\n"
    )?;
    s.flush()?;
    let mut resp = Vec::new();
    s.read_to_end(&mut resp)?;
    let resp = String::from_utf8(resp).unwrap();
    let body = resp.split_once("\r\n\r\n").map(|(_, b)| b.to_owned());
    Ok((leaf, body.unwrap_or_default()))
}

/// fit's `TestOnDemandAllowed`, through handshakes: several at once ask for a name the ask URL
/// allows; while the ask is held, the listener serves another name from its files, each
/// handshake at once, and refuses a name the ask URL refuses at once; then one certificate is
/// obtained from Pebble, every waiting handshake gets it, and the ask URL was asked once. A
/// reload listing no names keeps it, served from memory without asking again.
#[test]
fn obtains_a_certificate_during_the_handshake() {
    let d = dir("on-demand-obtain");
    let http_port = free_port();
    let Some(pebble) = Pebble::start(&d, http_port) else {
        return;
    };
    let files_ca = Ca::new("files CA");
    let files_ca_file = files_ca.write(&d, "files-ca.pem");
    let (cert, key) = files_ca.leaf(&d, "files", &["files.akit.test"]);
    let ask = Asker::start(&["ok.akit.test"]);
    ask.hold.store(true, Ordering::Release);
    let s = Served::start(&format!(
        r#"{{"acme": {{"directory": "{}", "ca_file": "{}", "storage": "{}",
                     "challenges": ["http-01"],
                     "on_demand": {{"ask": "{}", "timeout": "30s"}}}},
            "listeners": [
              {{"name": "plain", "address": "127.0.0.1:{http_port}", "protocols": ["http/1.1"]}},
              {{"name": "secure", "address": "127.0.0.1:0", "protocols": ["http/1.1"],
                "tls": {{"on_demand": true,
                         "certificates": [{{"cert_file": "{}", "key_file": "{}"}}]}}}}],
            "routes": [{{"handler": {{"module": "respond", "params": {{"body": "hello"}}}}}}]}}"#,
        pebble.directory,
        pebble.ca_file.display(),
        d.join("storage").display(),
        ask.url(),
        cert.display(),
        key.display(),
    ));
    let secure = s.addrs[1];
    let pebble_client = Arc::new(
        ClientConfig::builder()
            .roots(pebble.roots())
            .alpn(["http/1.1"])
            .build()
            .unwrap(),
    );
    let waiting: Vec<_> = (0..4)
        .map(|_| {
            let c = Arc::clone(&pebble_client);
            std::thread::spawn(move || get(secure, &c, "ok.akit.test", Duration::from_secs(30)))
        })
        .collect();
    let deadline = Instant::now() + Duration::from_secs(10);
    while ask.count("ok.akit.test") == 0 {
        assert!(Instant::now() < deadline, "never asked");
        std::thread::sleep(Duration::from_millis(10));
    }

    // The handshakes wait without holding the I/O threads: others go on.
    let files_client = client(&files_ca_file, &["http/1.1"], None);
    for _ in 0..20 {
        let t = Instant::now();
        let (_, body) = get(
            secure,
            &files_client,
            "files.akit.test",
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(body, "hello");
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    }
    // A name the ask URL refuses fails once it answers, without a certificate.
    let refused = std::thread::spawn({
        let c = Arc::clone(&pebble_client);
        move || get(secure, &c, "no.akit.test", Duration::from_secs(10))
    });

    ask.hold.store(false, Ordering::Release);
    assert!(refused.join().unwrap().is_err());
    let mut leaves = Vec::new();
    for w in waiting {
        let (leaf, body) = w.join().unwrap().unwrap();
        assert_eq!(body, "hello");
        leaves.push(leaf);
    }
    assert!(leaves.iter().all(|l| *l == leaves[0] && !l.is_empty()));
    assert_eq!(ask.count("ok.akit.test"), 1);
    assert_eq!(*s.obtained.lock().unwrap(), 1);

    // A reload that lists no names keeps it; served from memory, any case, not asking again.
    s.acme.manage::<&str>(&[]);
    let (leaf, _) = get(
        secure,
        &pebble_client,
        "OK.akit.test",
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(leaf, leaves[0]);
    assert_eq!(ask.count("ok.akit.test"), 1);
    // A refused name is refused from memory for a minute.
    assert!(
        get(
            secure,
            &pebble_client,
            "no.akit.test",
            Duration::from_secs(5)
        )
        .is_err()
    );
    assert_eq!(ask.count("no.akit.test"), 1);
    s.stop();
    drop(pebble);
    std::fs::remove_dir_all(&d).unwrap();
}

/// fit's `TestOnDemandRateLimited` through handshakes, with a CA that never answers: the first
/// name is taken on and its handshake gives up at `acme.on_demand.timeout`; the second, past the
/// rate, fails at once without asking; a name the files have is served all along.
#[test]
fn a_name_past_the_rate_fails_at_once() {
    let d = dir("on-demand-rate");
    // A CA that takes connections and never answers.
    let hanging = TcpListener::bind("127.0.0.1:0").unwrap();
    let ca_addr = hanging.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for c in hanging.incoming() {
            held.extend(c.ok());
        }
    });
    let files_ca = Ca::new("files CA");
    let files_ca_file = files_ca.write(&d, "files-ca.pem");
    let (cert, key) = files_ca.leaf(&d, "files", &["files.akit.test"]);
    let ask = Asker::start(&["a.akit.test", "b.akit.test"]);
    let s = Served::start(&format!(
        r#"{{"acme": {{"directory": "https://{ca_addr}/dir", "storage": "{}",
                     "on_demand": {{"ask": "{}", "rate": 1, "interval": "1h", "timeout": "1s"}}}},
            "listeners": [
              {{"name": "secure", "address": "127.0.0.1:0", "protocols": ["http/1.1"],
                "tls": {{"on_demand": true,
                         "certificates": [{{"cert_file": "{}", "key_file": "{}"}}]}}}}],
            "routes": [{{"handler": {{"module": "respond", "params": {{"body": "hello"}}}}}}]}}"#,
        d.join("storage").display(),
        ask.url(),
        cert.display(),
        key.display(),
    ));
    let secure = s.addrs[0];
    let c = client(&files_ca_file, &["http/1.1"], None);
    let t = Instant::now();
    assert!(get(secure, &c, "a.akit.test", Duration::from_secs(10)).is_err());
    let waited = t.elapsed();
    assert!(
        waited >= Duration::from_millis(900) && waited < Duration::from_secs(5),
        "the handshake waited {waited:?}"
    );
    let t = Instant::now();
    assert!(get(secure, &c, "b.akit.test", Duration::from_secs(10)).is_err());
    assert!(
        t.elapsed() < Duration::from_millis(900),
        "{:?}",
        t.elapsed()
    );
    assert_eq!(ask.count("b.akit.test"), 0);
    let (_, body) = get(secure, &c, "files.akit.test", Duration::from_secs(5)).unwrap();
    assert_eq!(body, "hello");
    s.stop();
    std::fs::remove_dir_all(&d).unwrap();
}
