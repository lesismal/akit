//! `GET /cache` and `POST /cache/purge` (I69), with fit's `admin/cache_test.go` ported: a route
//! behind the `cache` middleware served by akit's listener, its responses purged through the
//! Admin API by URL, by tag and by prefix, and what is refused. On the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use akio::{Backend, UringConfig};
use akit_admin::{Configs, Options, SOURCE_FILE, Server};
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};

fn backend() -> Backend {
    match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    }
}

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

/// One request on a connection of its own, `Connection: close`.
fn request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Resp {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\nHost: {addr}\r\n");
    for (n, v) in headers {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    if let Some(b) = body {
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).unwrap();
    if let Some(b) = body {
        let _ = s.write_all(b.as_bytes());
    }
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let raw = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = raw.split_once("\r\n\r\n").expect(&raw);
    let mut lines = head.lines();
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    Resp {
        status,
        headers: lines
            .filter_map(|l| l.split_once(':'))
            .map(|(n, v)| (n.to_owned(), v.trim().to_owned()))
            .collect(),
        body: body.to_owned(),
    }
}

impl Resp {
    fn header(&self, name: &str) -> &str {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map_or("", |(_, v)| v.as_str())
    }
}

// fit's TestCachePurge: a response a cache zone keeps purged through the Admin API, by URL, by
// tag and by prefix, and what the zones keep read.
#[test]
fn cache_purge() {
    let doc = r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}],
  "routes": [{"middleware": [{"module": "cache", "params": {"zone": "admin"}}],
    "handler": {"module": "respond", "params": {"body": "x", "headers": {"Cache-Control": "max-age=60", "Cache-Tag": "t"}}}}]}"#;
    let config = akit_config::from_json(doc, None).unwrap();
    let registry: Registry<HandlerParams> =
        Registry::new(&[akit_handler::MODULES, akit_cache::MODULES]).unwrap();
    let cell = GraphCell::new(compile(&config, &registry, &Env::default()).unwrap());
    let mut lo = akit_listener::Options::default();
    lo.backend = Some(backend());
    lo.shutdown_timeout = Duration::from_secs(5);
    let engine = Arc::new(akit_listener::engine(&cell, &lo).unwrap());
    engine.start().unwrap();
    let site = *engine.local_addrs().first().unwrap();

    let settings = akit_config::Admin {
        address: "127.0.0.1:0".into(),
        ..akit_config::Admin::default()
    };
    let configs = Configs::new(
        akit_config::value::standardize(doc).unwrap(),
        SOURCE_FILE,
        settings.history,
    );
    let mut opts = Options::default();
    opts.backend = Some(backend());
    let server = Server::start(&settings, configs, Box::new(|_| Ok(())), &opts).unwrap();
    let api = server.addr();

    let fetch = |path: &str| {
        request(site, "GET", path, &[], None)
            .header("Cache-Status")
            .to_owned()
    };
    let purge = |body: &str| {
        let r = request(
            api,
            "POST",
            "/cache/purge",
            &[("Content-Type", "application/json")],
            Some(body),
        );
        (r.status, r.body)
    };
    fetch("/a");
    fetch("/b");
    let zones = request(api, "GET", "/cache", &[], None);
    assert!(
        zones.status == 200 && zones.body.contains(r#""zone":"admin","keys":2"#),
        "GET /cache: {}",
        zones.body
    );
    let site = format!("http://{site}");
    assert_eq!(
        purge(&format!(r#"{{"zone": "admin", "urls": ["{site}/a"]}}"#)),
        (200, "{\"purged\":1}\n".to_owned()),
        "by URL"
    );
    let cs = fetch("/a");
    assert!(
        cs.starts_with("akit; fwd=uri-miss"),
        "/a after its purge: {cs:?}"
    );
    let cs = fetch("/b");
    assert!(cs.starts_with("akit; hit"), "/b: {cs:?}");
    assert_eq!(
        purge(r#"{"tags": ["t"]}"#),
        (200, "{\"purged\":2}\n".to_owned()),
        "by tag"
    );
    fetch("/img/a");
    fetch("/img/b");
    fetch("/other");
    assert_eq!(
        purge(&format!(
            r#"{{"zone": "admin", "prefixes": ["{site}/img/"]}}"#
        )),
        (200, "{\"purged\":2}\n".to_owned()),
        "by prefix"
    );
    let cs = fetch("/other");
    assert!(
        cs.starts_with("akit; hit"),
        "/other after the purge of /img/: {cs:?}"
    );
    for (body, want) in [
        (r#"{"zone": "nothing", "all": true}"#, 404),
        (r#"{"zone": "admin"}"#, 400),
        (r#"{"urls": ["/relative"]}"#, 400),
        (r#"{"prefixes": ["img/"]}"#, 400),
        ("not json", 400),
    ] {
        let (status, got) = purge(body);
        assert_eq!(status, want, "{body}: {got}");
    }
    assert_eq!(
        purge(r#"{"zone": "admin", "all": true}"#),
        (200, "{\"purged\":1}\n".to_owned()),
        "all"
    );
    // Sent as anything but JSON, or with another method.
    let r = request(
        api,
        "POST",
        "/cache/purge",
        &[("Content-Type", "text/plain")],
        Some("{}"),
    );
    assert_eq!(r.status, 415, "{}", r.body);
    let r = request(api, "GET", "/cache/purge", &[], None);
    assert_eq!((r.status, r.header("Allow")), (405, "POST"));
    let r = request(api, "DELETE", "/cache", &[], None);
    assert_eq!(r.status, 405);
    server.shutdown();
    engine.shutdown();
    engine.wait();
}

/// I80b (the threat model's L14): `POST /cache/purge` is guarded as every endpoint is. With a
/// token asked for, a purge without it, or with a wrong one, is refused (401) and purges nothing;
/// a purge sent from a web page (`Origin`) is refused (403), token or not, even from the API's
/// own origin, which may only read; with the token, it purges.
#[test]
fn cache_purge_guarded() {
    let doc = r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}],
  "routes": [{"middleware": [{"module": "cache", "params": {"zone": "guarded"}}],
    "handler": {"module": "respond", "params": {"body": "x", "headers": {"Cache-Control": "max-age=60"}}}}]}"#;
    let config = akit_config::from_json(doc, None).unwrap();
    let registry: Registry<HandlerParams> =
        Registry::new(&[akit_handler::MODULES, akit_cache::MODULES]).unwrap();
    let cell = GraphCell::new(compile(&config, &registry, &Env::default()).unwrap());
    let mut lo = akit_listener::Options::default();
    lo.backend = Some(backend());
    lo.shutdown_timeout = Duration::from_secs(5);
    let engine = Arc::new(akit_listener::engine(&cell, &lo).unwrap());
    engine.start().unwrap();
    let site = *engine.local_addrs().first().unwrap();

    let file = std::env::temp_dir().join(format!("akit-admin-purge-token-{}", std::process::id()));
    std::fs::write(&file, "purge-s3cret\n").unwrap();
    let settings = akit_config::Admin {
        address: "127.0.0.1:0".into(),
        token_file: Some(file.to_str().unwrap().to_owned()),
        ..akit_config::Admin::default()
    };
    let configs = Configs::new(
        akit_config::value::standardize(doc).unwrap(),
        SOURCE_FILE,
        settings.history,
    );
    let mut opts = Options::default();
    opts.backend = Some(backend());
    let server = Server::start(&settings, configs, Box::new(|_| Ok(())), &opts).unwrap();
    let api = server.addr();
    let token = "Bearer purge-s3cret";
    let body = r#"{"zone": "guarded", "all": true}"#;
    let json = ("Content-Type", "application/json");
    let kept = || {
        let r = request(api, "GET", "/cache", &[("Authorization", token)], None);
        assert_eq!(r.status, 200, "{}", r.body);
        r.body.contains(r#""zone":"guarded","keys":1"#)
    };

    let cs = request(site, "GET", "/a", &[], None);
    assert_eq!(cs.status, 200);
    assert!(kept(), "the response is kept");
    let api_origin = format!("http://{api}");
    for (headers, want) in [
        (vec![json], 401),
        (vec![json, ("Authorization", "Bearer wrong")], 401),
        (vec![json, ("Authorization", "purge-s3cret")], 401),
        (
            vec![
                json,
                ("Authorization", token),
                ("Origin", "https://evil.example.com"),
            ],
            403,
        ),
        (
            vec![
                json,
                ("Authorization", token),
                ("Origin", api_origin.as_str()),
            ],
            403,
        ),
        (vec![json, ("Origin", "https://evil.example.com")], 401),
    ] {
        let r = request(api, "POST", "/cache/purge", &headers, Some(body));
        assert_eq!(r.status, want, "{headers:?}: {}", r.body);
        assert!(kept(), "{headers:?} purged nothing");
    }
    let r = request(
        api,
        "POST",
        "/cache/purge",
        &[json, ("Authorization", token)],
        Some(body),
    );
    assert_eq!(
        (r.status, r.body.as_str()),
        (200, "{\"purged\":1}\n"),
        "with the token"
    );
    assert!(!kept(), "purged");
    server.shutdown();
    engine.shutdown();
    engine.wait();
    let _ = std::fs::remove_file(&file);
}
