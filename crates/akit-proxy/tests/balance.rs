//! fit's load balancing tests for its proxy (`handler/proxy/balance_test.go`), ported (I36): the
//! sticky cookie set after the upstream's own `Set-Cookie`, kept to, and replaced when stale; and
//! `header_hash` keeping each user on one address. Over HTTP/1.1, on the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`), with responses sent whole and streamed.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use akio::{Backend, Engine, UringConfig};
use akit_runtime::upstream::{EngineSlot, sticky_id};
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};

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

/// akit serving `json`, as `akit run` does.
struct Akit {
    engine: Arc<Engine>,
    addr: SocketAddr,
    _cell: GraphCell,
}

impl Akit {
    fn start(json: &str) -> Akit {
        let config = akit_config::from_json(json, None).unwrap();
        let registry =
            Registry::<HandlerParams>::new(&[akit_handler::MODULES, akit_proxy::MODULES]).unwrap();
        let slot = EngineSlot::new();
        let cell = GraphCell::new(compile(&config, &registry, &Env::new(slot.clone())).unwrap());
        let engine = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
        engine.start().unwrap();
        assert!(slot.set(engine.clone()));
        let addr = *engine.local_addrs().first().unwrap();
        Akit {
            engine,
            addr,
            _cell: cell,
        }
    }
}

impl Drop for Akit {
    fn drop(&mut self) {
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
    }
}

/// One listener, the upstream `app` at `addrs` with `fields`, and a route to it.
fn config(addrs: &[String], fields: &str) -> String {
    let list: Vec<String> = addrs.iter().map(|a| format!("{a:?}")).collect();
    format!(
        r#"{{
  "listeners": [{{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1"]}}],
  "upstreams": [{{"name": "app", "addresses": [{}], {fields}}}],
  "routes": [{{"handler": {{"module": "proxy", "params": {{"upstream": "app"}}}}}}]
}}"#,
        list.join(", ")
    )
}

/// fit's `named`: an upstream that answers its name and sets a cookie of its own; to `/stream`,
/// chunked, so that the proxy streams the response rather than send it whole.
fn named(name: &'static str) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            thread::spawn(move || {
                let mut w = s.try_clone().unwrap();
                let mut r = BufReader::new(s);
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let stream = line.contains(" /stream ");
                    loop {
                        let mut h = String::new();
                        if r.read_line(&mut h).unwrap_or(0) == 0 {
                            return;
                        }
                        if h == "\r\n" {
                            break;
                        }
                    }
                    let out = if stream {
                        format!(
                            "HTTP/1.1 200 OK\r\nSet-Cookie: own=1\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{name}\r\n0\r\n\r\n",
                            name.len()
                        )
                    } else {
                        format!(
                            "HTTP/1.1 200 OK\r\nSet-Cookie: own=1\r\nContent-Length: {}\r\n\r\n{name}",
                            name.len()
                        )
                    };
                    if w.write_all(out.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
    addr
}

/// A GET of `path` with `fields`: the body and the `Set-Cookie` values of the response.
fn get(addr: SocketAddr, path: &str, fields: &[(&str, &str)]) -> (String, Vec<String>) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let extra: String = fields
        .iter()
        .map(|(n, v)| format!("{n}: {v}\r\n"))
        .collect();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: x\r\n{extra}Connection: close\r\n\r\n"
    )
    .unwrap();
    let mut text = String::new();
    BufReader::new(s).read_to_string(&mut text).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let cookies = head
        .lines()
        .filter_map(|l| l.split_once(':'))
        .filter(|(n, _)| n.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, v)| v.trim().to_owned())
        .collect();
    let chunked = head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked");
    let body = if chunked {
        // One chunk and the last: the name.
        body.split("\r\n").nth(1).unwrap_or("").to_owned()
    } else {
        body.to_owned()
    };
    (body, cookies)
}

/// fit's TestStickyCookie, with responses whole and streamed.
#[test]
fn sticky_cookie() {
    let addrs = vec![named("a"), named("b")];
    let akit = Akit::start(&config(&addrs, r#""sticky_cookie": {"max_age": "1h"}"#));
    for path in ["/", "/stream"] {
        let (first, cookies) = get(akit.addr, path, &[]);
        assert_eq!(cookies.len(), 2, "{path}: {cookies:?}");
        assert_eq!(cookies[0], "own=1");
        let addr = if first == "a" { &addrs[0] } else { &addrs[1] };
        assert_eq!(
            cookies[1],
            format!(
                "akit_sticky={}; Path=/; Max-Age=3600; HttpOnly; SameSite=Lax",
                sticky_id(addr)
            ),
            "{path}"
        );
        let cookie = cookies[1].split(';').next().unwrap().to_owned();
        for _ in 0..10 {
            let (body, cookies) = get(akit.addr, path, &[("Cookie", &format!("x=y; {cookie}"))]);
            assert_eq!(body, first, "{path}: with the cookie");
            assert_eq!(cookies, ["own=1"], "{path}: with the cookie");
        }
        // A cookie naming no address is replaced.
        let (_, cookies) = get(akit.addr, path, &[("Cookie", "akit_sticky=0000")]);
        assert_eq!(cookies.len(), 2, "{path}: a stale cookie: {cookies:?}");
    }
}

/// fit's TestHeaderHash.
#[test]
fn header_hash() {
    let addrs = vec![named("a"), named("b"), named("c")];
    let akit = Akit::start(&config(
        &addrs,
        r#""load_balancing": "header_hash", "hash_key": "x-user""#,
    ));
    let mut seen = BTreeSet::new();
    for user in ["ann", "bob", "cat", "dan", "eve", "fay", "gus", "hal"] {
        let (first, cookies) = get(akit.addr, "/", &[("X-User", user)]);
        assert_eq!(cookies, ["own=1"], "no sticky cookie");
        for _ in 0..5 {
            let (got, _) = get(akit.addr, "/", &[("X-User", user)]);
            assert_eq!(got, first, "user {user}");
        }
        seen.insert(first);
    }
    assert!(seen.len() >= 2, "eight users all went to {seen:?}");
}

/// `uri_hash` keeps each path on one address, and `ip_hash` every request from one client on
/// one.
#[test]
fn uri_and_ip_hash() {
    let addrs = vec![named("a"), named("b"), named("c")];
    let akit = Akit::start(&config(&addrs, r#""load_balancing": "uri_hash""#));
    let mut seen = BTreeSet::new();
    for i in 0..12 {
        let path = format!("/p/{i}?q=1");
        let (first, _) = get(akit.addr, &path, &[]);
        for _ in 0..3 {
            assert_eq!(get(akit.addr, &path, &[]).0, first, "{path}");
        }
        seen.insert(first);
    }
    assert!(seen.len() >= 2, "twelve paths all went to {seen:?}");
    drop(akit);
    let akit = Akit::start(&config(&addrs, r#""load_balancing": "ip_hash""#));
    let (first, _) = get(akit.addr, "/", &[]);
    for i in 0..10 {
        assert_eq!(get(akit.addr, &format!("/{i}"), &[]).0, first);
    }
}
