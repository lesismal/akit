//! HTTPS by default (I33): a config that writes no listeners is served on the `https` and `http`
//! listeners it makes; `http` redirects every request to HTTPS with a 308 (fit's
//! `TestAutoHTTPSRedirect`), but for ACME's HTTP-01 challenges, through which a certificate for
//! the routes' host name is obtained from Pebble and served (fit's `TestAutoHTTPSFromPebble`).
//! On the backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`). The Pebble test is
//! skipped without Pebble's binaries (support/pebble.rs).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod support;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use akio::tls::ClientConfig;
use akio::{Backend, UringConfig};
use akit_listener::Listeners;
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};
use akit_tls::Acme;
use support::certs::dir;
use support::https::get_h1;
use support::pebble::{Pebble, free_port};

fn options() -> akit_listener::Options {
    let mut o = akit_listener::Options::default();
    o.backend = Some(match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    });
    o.shutdown_timeout = Duration::from_secs(5);
    o
}

/// `json`'s config served, with `acme` obtaining its names when given.
struct Served {
    engine: Arc<akio::Engine>,
    listeners: Option<Listeners>,
    names: Vec<String>,
}

impl Served {
    fn start(json: &str, acme: Option<&Acme>) -> Served {
        let config = akit_config::from_json(json, None).unwrap();
        let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
        let graph = compile(&config, &registry, &Env::default().with_acme(acme.cloned())).unwrap();
        let names = graph.acme_names().to_vec();
        let cell = GraphCell::new(graph);
        let engine = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
        engine.start().unwrap();
        let listeners = Listeners::new(Arc::clone(&engine), cell, &options());
        Served {
            engine,
            listeners: Some(listeners),
            names,
        }
    }

    /// Each listener's name and port, in the order served.
    fn ports(&self) -> Vec<(String, u16)> {
        self.listeners
            .as_ref()
            .unwrap()
            .served()
            .into_iter()
            .map(|s| (s.name, s.address.port()))
            .collect()
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        drop(self.listeners.take());
        assert!(akit_listener::shutdown(
            &self.engine,
            Duration::from_secs(5)
        ));
    }
}

/// A request in the clear to 127.0.0.1:`port`: the response's status line, its `Location`, and
/// its body.
fn plain(port: u16, method: &str, host: Option<&str>, target: &str) -> (String, String, String) {
    let mut s = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port))).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let host = host.map(|h| format!("Host: {h}\r\n")).unwrap_or_default();
    write!(
        s,
        "{method} {target} HTTP/1.1\r\n{host}Content-Length: 4\r\nConnection: close\r\n\r\nbody"
    )
    .unwrap();
    let mut r = String::new();
    s.read_to_string(&mut r).unwrap();
    let (head, body) = r.split_once("\r\n\r\n").unwrap_or((&r, ""));
    let location = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("location")
                .then(|| v.trim().to_owned())
        })
        .unwrap_or_default();
    (
        head.lines().next().unwrap_or("").to_owned(),
        location,
        body.to_owned(),
    )
}

/// A config that writes no listeners: `https` and `http` are made on `auto_https`'s ports, and
/// `http` redirects every request to `https` with 308, keeping the host, the target and the
/// method; the routes are served on `https` alone.
#[test]
fn http_redirects_to_https() {
    let (http_port, https_port) = (free_port(), free_port());
    let s = Served::start(
        &format!(
            r#"{{"auto_https": {{"http_port": {http_port}, "https_port": {https_port}}},
  "routes": [{{"match": {{"host": ["a.example"]}}, "handler": {{"module": "respond", "params": {{"body": "ok"}}}}}}]}}"#
        ),
        None,
    );
    assert_eq!(
        s.ports(),
        [
            ("https".to_owned(), https_port),
            ("http".to_owned(), http_port)
        ]
    );
    for (method, host, target, want) in [
        (
            "GET",
            Some("a.example".to_owned()),
            "/x?y=1",
            format!("https://a.example:{https_port}/x?y=1"),
        ),
        (
            "POST",
            Some(format!("A.example:{http_port}")),
            "/form",
            format!("https://A.example:{https_port}/form"),
        ),
        (
            "GET",
            Some("[::1]".to_owned()),
            "/",
            format!("https://[::1]:{https_port}/"),
        ),
        (
            "GET",
            Some("other.example".to_owned()),
            "/.well-known/acme-challenge/none",
            format!("https://other.example:{https_port}/.well-known/acme-challenge/none"),
        ),
        (
            "GET",
            Some("a.example".to_owned()),
            "http://a.example/y?z=1",
            format!("https://a.example:{https_port}/y?z=1"),
        ),
    ] {
        let (status, location, _) = plain(http_port, method, host.as_deref(), target);
        assert!(
            status.starts_with("HTTP/1.1 308"),
            "{method} {host:?} {target}: {status}"
        );
        assert_eq!(location, want, "{method} {host:?} {target}");
    }
}

/// Without redirects, only `https` is made; a listener written with an empty `tls` gets the host
/// names of the routes it serves, and a listener in the clear written out serves its routes.
#[test]
fn no_redirects_and_written_listeners() {
    let https_port = free_port();
    let s = Served::start(
        &format!(
            r#"{{"auto_https": {{"disable_redirects": true, "https_port": {https_port}}},
  "routes": [{{"match": {{"host": ["b.example"]}}, "handler": {{"module": "respond"}}}}]}}"#
        ),
        None,
    );
    assert_eq!(s.ports(), [("https".to_owned(), https_port)]);
    drop(s);

    let s = Served::start(
        r#"{"listeners": [
    {"name": "web", "address": "127.0.0.1:0", "tls": {}},
    {"name": "plain", "address": "127.0.0.1:0"}],
  "routes": [{"match": {"host": ["c.example", "localhost"]}, "handler": {"module": "respond", "params": {"body": "plain"}}}]}"#,
        None,
    );
    let plain_port = s.ports()[1].1;
    let (status, _, body) = plain(plain_port, "GET", Some("c.example"), "/");
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    assert_eq!(body, "plain");
}

/// fit's `TestAutoHTTPSFromPebble`: a config that names only a host, and the CA's address and
/// ports. The certificate is obtained through HTTP-01 on the `http` listener HTTPS by default
/// makes, served on `https`, and every other request in the clear is redirected there.
#[test]
fn serves_https_from_pebble() {
    const NAME: &str = "auto.akit.test";
    let d = dir("auto-https-pebble");
    let (http_port, https_port) = (free_port(), free_port());
    let Some(pebble) = Pebble::start_with(&d, http_port, https_port, 7_776_000) else {
        return;
    };
    let storage = d.join("storage");
    let json = format!(
        r#"{{"acme": {{"directory": "{}", "ca_file": "{}", "storage": "{}", "timeout": "60s", "request_timeout": "10s"}},
  "auto_https": {{"http_port": {http_port}, "https_port": {https_port}}},
  "routes": [{{"match": {{"host": ["{NAME}"]}}, "handler": {{"module": "respond", "params": {{"body": "by default"}}}}}}]}}"#,
        pebble.directory,
        pebble.ca_file.display(),
        storage.display(),
    );
    let config = akit_config::from_json(&json, None).unwrap();
    let acme = Acme::new(config.acme.as_ref().unwrap()).unwrap();
    acme.set_log(Arc::new(|line: &str| eprintln!("acme: {line}")));
    let s = Served::start(&json, Some(&acme));
    assert_eq!(s.names, [NAME]);
    acme.manage(&s.names);

    let deadline = Instant::now() + Duration::from_secs(60);
    let key = loop {
        if let Some(k) = acme.certificate(NAME) {
            break k;
        }
        assert!(
            Instant::now() < deadline,
            "no certificate for {NAME}: {:?}",
            acme.last_error(NAME)
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let leaf = key.chain_der().next().unwrap().to_vec();
    let c = ClientConfig::builder()
        .roots(pebble.roots())
        .alpn(["http/1.1"])
        .build()
        .unwrap();
    let addr = SocketAddr::from(([127, 0, 0, 1], https_port));
    let seen = get_h1(addr, &c, NAME, NAME, "/").unwrap();
    assert_eq!(seen.leaf, leaf);
    assert!(seen.status.starts_with("HTTP/1.1 200"), "{}", seen.status);
    assert_eq!(seen.body, "by default");

    let (status, location, _) = plain(http_port, "GET", Some(NAME), "/page");
    assert!(status.starts_with("HTTP/1.1 308"), "{status}");
    assert_eq!(location, format!("https://{NAME}:{https_port}/page"));

    drop(s);
    acme.close();
    drop(pebble);
    std::fs::remove_dir_all(&d).unwrap();
}
