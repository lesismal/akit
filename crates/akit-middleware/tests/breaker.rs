//! The `circuit_breaker` middleware through akit's listener on akio, on the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`): it opens on failures of the `respond`
//! handler and of a proxied upstream (whose requests it then stops), closes again once its
//! probes fare well, and counts a proxied response slower than `slow_threshold` to begin. fit's
//! own tests of its states, window and limits run on a test clock in the crate
//! (`src/breaker.rs`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use akio::{Backend, Engine, UringConfig};
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, Registry, compile};

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

struct Akit {
    engine: Arc<Engine>,
    addr: SocketAddr,
    _cell: GraphCell,
}

impl Akit {
    fn start(json: &str) -> Akit {
        let config = akit_config::from_json(json, None).unwrap_or_else(|e| panic!("{json}: {e}"));
        let slot = EngineSlot::new();
        let registry = Registry::new(&[
            akit_handler::MODULES,
            akit_middleware::MODULES,
            akit_proxy::MODULES,
        ])
        .unwrap();
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

/// A config of one route: the breaker with `params`, before `handler`, and `extra` top-level
/// fields.
fn config(params: &str, handler: &str, extra: &str) -> String {
    format!(
        r#"{{
  "listeners": [{{"name": "web", "address": "127.0.0.1:0"}}]{extra},
  "routes": [{{"middleware": [{{"module": "circuit_breaker", "params": {{{params}}}}}], "handler": {handler}}}]
}}"#
    )
}

/// A GET: the status, the `Retry-After` and the body.
fn get(addr: SocketAddr) -> (u16, Option<String>, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(s, "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
    let mut text = String::new();
    BufReader::new(s).read_to_string(&mut text).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    let retry_after = head
        .lines()
        .filter_map(|l| l.split_once(':'))
        .find(|(n, _)| n.eq_ignore_ascii_case("retry-after"))
        .map(|(_, v)| v.trim().to_owned());
    (status, retry_after, body.to_owned())
}

/// An upstream answering each request with the status `status` holds, after `delay`, counting
/// them.
fn upstream(delay: Duration) -> (SocketAddr, Arc<AtomicU16>, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let status = Arc::new(AtomicU16::new(200));
    let hits = Arc::new(AtomicUsize::new(0));
    let (st, h) = (status.clone(), hits.clone());
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            let (st, h) = (st.clone(), h.clone());
            thread::spawn(move || {
                let mut w = s.try_clone().unwrap();
                let mut r = BufReader::new(s);
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    if line != "\r\n" {
                        continue;
                    }
                    h.fetch_add(1, Ordering::SeqCst);
                    thread::sleep(delay);
                    let reply = format!(
                        "HTTP/1.1 {} X\r\nContent-Length: 2\r\n\r\nup",
                        st.load(Ordering::SeqCst)
                    );
                    if w.write_all(reply.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
    (addr, status, hits)
}

fn proxied(addr: SocketAddr) -> (String, String) {
    (
        r#"{"module": "proxy", "params": {"upstream": "app"}}"#.to_owned(),
        format!(r#", "upstreams": [{{"name": "app", "addresses": ["{addr}"], "max_fails": 0}}]"#),
    )
}

/// The `respond` handler's 500s open it; open, it answers 503 with `Retry-After`; after
/// `open_duration` one probe goes through, and failing opens it again.
#[test]
fn opens_on_failures() {
    let akit = Akit::start(&config(
        r#""min_requests": 4, "open_duration": "1s", "half_open_requests": 1"#,
        r#"{"module": "respond", "params": {"status": 500, "body": "no"}}"#,
        "",
    ));
    for _ in 0..4 {
        assert_eq!(get(akit.addr).0, 500);
    }
    let (status, retry_after, body) = get(akit.addr);
    assert_eq!((status, retry_after.as_deref()), (503, Some("1")));
    assert!(body.is_empty(), "{body:?}");
    thread::sleep(Duration::from_millis(1100));
    // The probe, which fails.
    assert_eq!(get(akit.addr).0, 500);
    assert_eq!(get(akit.addr).0, 503);
}

/// Through the proxy: failing upstream responses open it, and the upstream sees no requests
/// while it is open; once the upstream recovers, the probes close it.
#[test]
fn stops_and_resumes_a_proxied_upstream() {
    let (addr, status, hits) = upstream(Duration::ZERO);
    let (handler, extra) = proxied(addr);
    let akit = Akit::start(&config(
        r#""min_requests": 3, "open_duration": "1s", "half_open_requests": 2"#,
        &handler,
        &extra,
    ));
    status.store(502, Ordering::SeqCst);
    for _ in 0..3 {
        assert_eq!(get(akit.addr).0, 502);
    }
    for _ in 0..3 {
        assert_eq!(get(akit.addr).0, 503);
    }
    assert_eq!(hits.load(Ordering::SeqCst), 3, "requests reached it open");
    status.store(200, Ordering::SeqCst);
    thread::sleep(Duration::from_millis(1100));
    for _ in 0..2 {
        let (s, _, body) = get(akit.addr);
        assert_eq!((s, body.as_str()), (200, "up"));
    }
    // Closed: everything goes through, the window afresh.
    status.store(502, Ordering::SeqCst);
    for _ in 0..2 {
        assert_eq!(get(akit.addr).0, 502);
    }
    assert_eq!(hits.load(Ordering::SeqCst), 7);
}

/// A proxied response slower than `slow_threshold` to begin counts as slow.
#[test]
fn slow_responses() {
    let (addr, _, _) = upstream(Duration::from_millis(250));
    let (handler, extra) = proxied(addr);
    let akit = Akit::start(&config(
        r#""min_requests": 2, "slow_threshold": "100ms", "open_duration": "30s""#,
        &handler,
        &extra,
    ));
    for _ in 0..2 {
        assert_eq!(get(akit.addr).0, 200);
    }
    let (status, retry_after, _) = get(akit.addr);
    assert_eq!((status, retry_after.as_deref()), (503, Some("30")));
}
