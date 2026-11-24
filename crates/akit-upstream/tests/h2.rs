//! Upstream pools over h2c (I18): requests multiplexed on a connection, a second connection once
//! one carries its streams, `max_conns` and `max_pending` over HTTP/2, an address that refuses
//! connections, the exchange's timeout giving a stream up, idle connections closed, and a pool
//! drained. The upstream is akio-http's own server (HTTP/1.1 and h2c with prior knowledge on one
//! port); the pools dial through an engine whose only threads are a listener's, on the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, Listener, UringConfig};
use akio_http::client::{ClientError, ClientRequest};
use akio_http::{Context, Detached, Headers, Response, Version};
use akit_upstream::{
    Download, EngineSlot, Error, Exchange, Head, Outcome, Pool, STREAMS_PER_CONN, UpstreamProtocol,
};

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

fn engine_with(service: impl akio_http::Service) -> Arc<Engine> {
    let e = Engine::builder()
        .listen(
            Listener::tcp("127.0.0.1:0".parse().unwrap())
                .backend(backend())
                .threads(2)
                .serve(akio_http::serve(service)),
        )
        .build()
        .unwrap();
    e.start().unwrap();
    Arc::new(e)
}

/// An engine with one listener (answering 404), whose threads host the pools' connections.
fn engine() -> Arc<Engine> {
    engine_with(|ctx: &mut Context| ctx.text(404, ""))
}

/// An h2c upstream on akio-http's server. `/hold/...` is answered only once let go; anything
/// else at once, with the request's version and path. It counts its connections by peer.
struct Upstream {
    engine: Arc<Engine>,
    held: Arc<Mutex<Vec<Detached>>>,
    peers: Arc<Mutex<Vec<std::net::SocketAddr>>>,
    requests: Arc<AtomicUsize>,
}

impl Upstream {
    fn start() -> Upstream {
        let held: Arc<Mutex<Vec<Detached>>> = Arc::default();
        let peers: Arc<Mutex<Vec<std::net::SocketAddr>>> = Arc::default();
        let requests = Arc::new(AtomicUsize::new(0));
        let (h, p, n) = (held.clone(), peers.clone(), requests.clone());
        let engine = engine_with(move |ctx: &mut Context| {
            n.fetch_add(1, Ordering::SeqCst);
            let req = ctx.req();
            {
                let mut p = p.lock().unwrap();
                if !p.contains(&req.peer) {
                    p.push(req.peer);
                }
            }
            let version = if req.version == Version::Http2 {
                "h2"
            } else {
                "h1"
            };
            let body = format!("{version} {}", req.target);
            if req.target.starts_with("/hold") {
                if let Some(d) = ctx.detach() {
                    h.lock().unwrap().push(d);
                }
                return;
            }
            ctx.text(200, body);
        });
        Upstream {
            engine,
            held,
            peers,
            requests,
        }
    }

    fn addr(&self) -> String {
        self.engine.local_addrs()[0].to_string()
    }

    fn held(&self) -> usize {
        self.held.lock().unwrap().len()
    }

    /// Answers every held request.
    fn release(&self) {
        let held: Vec<Detached> = std::mem::take(&mut *self.held.lock().unwrap());
        for d in held {
            d.respond(Response::text(200, "released"));
        }
    }

    fn connections(&self) -> usize {
        self.peers.lock().unwrap().len()
    }
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.release();
        self.engine.shutdown();
    }
}

fn eventually(what: &str, f: impl Fn() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < Duration::from_secs(10), "never: {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

/// An h2c upstream config: `addresses`, then `fields`.
fn upstream(addresses: &[String], fields: &str) -> akit_config::Upstream {
    let list: Vec<String> = addresses.iter().map(|a| format!("{a:?}")).collect();
    let json = format!(
        r#"{{"upstreams": [{{"name": "app", "addresses": [{}], "protocol": "h2c"{}{fields}}}]}}"#,
        list.join(", "),
        if fields.is_empty() { "" } else { ", " }
    );
    akit_config::from_json(&json, None)
        .unwrap()
        .upstreams
        .remove(0)
}

/// Sends GETs of `path/i` for `i` in `range`; their outcomes arrive on the receiver.
fn send(pool: &Pool, path: &str, range: std::ops::Range<usize>) -> Receiver<(usize, Outcome)> {
    let (tx, rx) = channel();
    for i in range {
        let tx = tx.clone();
        pool.request(ClientRequest::get(&format!("{path}/{i}")), move |r| {
            let _ = tx.send((i, r));
        });
    }
    rx
}

fn body(r: &Outcome) -> String {
    match r {
        Ok(resp) => String::from_utf8_lossy(&resp.body).into_owned(),
        Err(e) => format!("error: {e}"),
    }
}

#[test]
fn requests_go_over_h2c() {
    let up = Upstream::start();
    let pool = Pool::new(&upstream(&[up.addr()], ""), EngineSlot::with(engine()));
    assert_eq!(pool.protocol(), UpstreamProtocol::H2c);
    let rx = send(&pool, "/a", 0..3);
    let mut got: Vec<String> = (0..3).map(|_| body(&rx.recv().unwrap().1)).collect();
    got.sort();
    assert_eq!(got, ["h2 /a/0", "h2 /a/1", "h2 /a/2"]);
    // One connection carried them all, and stays for the next request.
    assert_eq!(up.connections(), 1);
    eventually("the requests are over", || pool.in_flight() == 0);
    assert_eq!(pool.h2_connections(0), Some((1, 0)));
    assert_eq!(pool.backend_requests(0), (3, 0));
}

#[test]
fn streams_share_a_connection_up_to_its_limit() {
    let up = Upstream::start();
    let pool = Pool::new(&upstream(&[up.addr()], ""), EngineSlot::with(engine()));
    // One more than a connection carries: a second connection is dialed for the last.
    let n = STREAMS_PER_CONN + 1;
    let rx = send(&pool, "/hold", 0..n);
    eventually("every request reached the upstream", || up.held() == n);
    assert_eq!(pool.h2_connections(0), Some((2, n)));
    assert_eq!(up.connections(), 2);
    up.release();
    for _ in 0..n {
        let (_, r) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(body(&r), "released");
    }
    eventually("the requests are over", || pool.in_flight() == 0);
}

#[test]
fn max_conns_and_max_pending_over_h2() {
    let up = Upstream::start();
    let pool = Pool::new(
        &upstream(&[up.addr()], r#""max_conns": 1, "max_pending": 2"#),
        EngineSlot::with(engine()),
    );
    // One connection's streams are under way; two more wait, and the next is refused.
    let n = STREAMS_PER_CONN;
    let rx = send(&pool, "/hold", 0..n + 3);
    let (i, r) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(i, n + 2);
    assert!(matches!(r, Err(Error::Busy)), "{}", body(&r));
    eventually("a connection's streams reached the upstream", || {
        up.held() == n
    });
    assert_eq!(pool.load(0).unwrap().waiting, 2);
    assert_eq!(up.connections(), 1);
    // As streams end, the waiting take them, on the same connection.
    up.release();
    eventually("the waiting reached the upstream", || up.held() == 2);
    up.release();
    for _ in 0..n + 2 {
        let (_, r) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(body(&r), "released");
    }
    assert_eq!(up.connections(), 1);
    // A refused request is no failure of the address's.
    eventually("the requests are over", || pool.in_flight() == 0);
    assert_eq!(pool.backend_requests(0), (n as u64 + 2, 0));
}

#[test]
fn an_address_that_refuses_connections() {
    // A port nothing listens on.
    let addr = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().to_string()
    };
    let pool = Pool::new(
        &upstream(&[addr], r#""max_fails": 1"#),
        EngineSlot::with(engine()),
    );
    let rx = send(&pool, "/x", 0..3);
    for _ in 0..3 {
        let (_, r) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(
            matches!(&r, Err(Error::Client(ClientError::Io(_)))),
            "{}",
            body(&r)
        );
    }
    eventually("the requests are over", || pool.in_flight() == 0);
    assert!(pool.is_down(0));
    assert_eq!(pool.h2_connections(0), Some((0, 0)));
}

#[test]
fn idle_connections_close_after_idle_timeout() {
    let up = Upstream::start();
    let pool = Pool::new(
        &upstream(&[up.addr()], r#""idle_timeout": "300ms""#),
        EngineSlot::with(engine()),
    );
    let rx = send(&pool, "/a", 0..1);
    assert_eq!(body(&rx.recv().unwrap().1), "h2 /a/0");
    eventually("the request is over", || pool.in_flight() == 0);
    assert_eq!(pool.h2_connections(0), Some((1, 0)));
    eventually("the idle connection is closed", || {
        pool.h2_connections(0) == Some((0, 0))
    });
    // The next request dials a new one.
    let rx = send(&pool, "/b", 0..1);
    assert_eq!(body(&rx.recv().unwrap().1), "h2 /b/0");
    assert_eq!(up.connections(), 2);
}

#[test]
fn max_idle_conns_zero_keeps_none() {
    let up = Upstream::start();
    let pool = Pool::new(
        &upstream(&[up.addr()], r#""max_idle_conns": 0"#),
        EngineSlot::with(engine()),
    );
    let rx = send(&pool, "/a", 0..1);
    assert_eq!(body(&rx.recv().unwrap().1), "h2 /a/0");
    eventually("the connection is closed", || {
        pool.h2_connections(0) == Some((0, 0))
    });
}

/// An exchange that reports what happened to it on a channel.
struct Report(std::sync::mpsc::Sender<String>);

impl Exchange for Report {
    fn on_head(&mut self, head: Head, _body: Download) {
        let _ = self.0.send(format!("head {}", head.status));
    }
    fn on_data(&mut self, chunk: &[u8]) {
        let _ = self
            .0
            .send(format!("data {}", String::from_utf8_lossy(chunk)));
    }
    fn on_end(&mut self, _trailers: &Headers) {
        let _ = self.0.send("end".into());
    }
    fn on_error(&mut self, e: Error) {
        let _ = self.0.send(format!("error {e}"));
    }
}

#[test]
fn streamed_exchange_over_h2c() {
    let up = Upstream::start();
    let pool = Pool::new(&upstream(&[up.addr()], ""), EngineSlot::with(engine()));
    let (tx, rx) = channel();
    pool.stream(0, ClientRequest::get("/s"), Report(tx));
    let got: Vec<String> = (0..3)
        .map(|_| rx.recv_timeout(Duration::from_secs(10)).unwrap())
        .collect();
    assert_eq!(got, ["head 200", "data h2 /s", "end"]);
    eventually("the exchange is over", || pool.in_flight() == 0);
    assert_eq!(pool.backend_requests(0), (1, 0));
}

#[test]
fn a_streamed_exchange_past_its_timeout_is_given_up() {
    let up = Upstream::start();
    let pool = Pool::new(
        &upstream(&[up.addr()], r#""response_timeout": "300ms""#),
        EngineSlot::with(engine()),
    );
    let (tx, rx) = channel();
    let start = Instant::now();
    pool.stream(0, ClientRequest::get("/hold/1"), Report(tx));
    let got = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        got.starts_with("error upstream: request timed out"),
        "{got}"
    );
    assert!(start.elapsed() < Duration::from_secs(3));
    eventually("the exchange is over", || pool.in_flight() == 0);
    // The timeout is the address's failure; its stream was left, and its connection closed,
    // since it carried no other.
    assert_eq!(pool.backend_requests(0), (1, 1));
    eventually("the connection is closed", || {
        pool.h2_connections(0) == Some((0, 0))
    });
    // A new request goes on a new connection.
    up.release();
    let rx = send(&pool, "/a", 0..1);
    assert_eq!(body(&rx.recv().unwrap().1), "h2 /a/0");
    assert_eq!(up.connections(), 2);
}

#[test]
fn a_whole_request_past_its_timeout() {
    let up = Upstream::start();
    let pool = Pool::new(
        &upstream(&[up.addr()], r#""response_timeout": "300ms""#),
        EngineSlot::with(engine()),
    );
    let rx = send(&pool, "/hold", 0..1);
    let (_, r) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        matches!(r, Err(Error::Client(ClientError::Timeout))),
        "{}",
        body(&r)
    );
    eventually("the request is over", || pool.in_flight() == 0);
    // akio reset its stream: the connection stays for the next.
    let rx = send(&pool, "/a", 0..1);
    assert_eq!(body(&rx.recv().unwrap().1), "h2 /a/0");
    assert_eq!(up.connections(), 1);
}

#[test]
fn a_closed_pool_drains_and_closes_its_connections() {
    let up = Upstream::start();
    let pool = Pool::new(&upstream(&[up.addr()], ""), EngineSlot::with(engine()));
    let rx = send(&pool, "/hold", 0..3);
    eventually("the requests reached the upstream", || up.held() == 3);
    pool.close();
    assert!(pool.is_closing() && !pool.is_closed());
    // New requests are refused; those under way finish.
    let late = send(&pool, "/late", 0..1);
    assert!(matches!(late.recv().unwrap().1, Err(Error::Closed)));
    up.release();
    for _ in 0..3 {
        assert_eq!(
            body(&rx.recv_timeout(Duration::from_secs(10)).unwrap().1),
            "released"
        );
    }
    eventually("the pool is closed", || pool.is_closed());
    eventually("its connection is closed", || {
        pool.h2_connections(0) == Some((0, 0))
    });
    assert_eq!(up.requests.load(Ordering::SeqCst), 3);
}

/// Active health checks over h2c: a probe goes over HTTP/2, as fit probes over the upstream's
/// protocol, and an address whose probes fail is taken out, then back once they pass.
#[test]
fn probes_go_over_h2c() {
    let healthy = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let versions: Arc<Mutex<Vec<Version>>> = Arc::default();
    let (h, v) = (healthy.clone(), versions.clone());
    let up = engine_with(move |ctx: &mut Context| {
        v.lock().unwrap().push(ctx.req().version);
        let status = if h.load(Ordering::SeqCst) { 200 } else { 503 };
        ctx.text(status, "");
    });
    let addr = up.local_addrs()[0].to_string();
    let pool = Pool::new(
        &upstream(
            &[addr],
            r#""health_check": {"path": "/health", "interval": "100ms", "timeout": "100ms", "fails": 1, "passes": 1}"#,
        ),
        EngineSlot::with(engine()),
    );
    eventually("a probe arrived", || !versions.lock().unwrap().is_empty());
    assert!(
        versions
            .lock()
            .unwrap()
            .iter()
            .all(|v| *v == Version::Http2),
        "a probe went over HTTP/1"
    );
    assert!(!pool.is_probed_down(0));
    healthy.store(false, Ordering::SeqCst);
    eventually("the address is probed down", || pool.is_probed_down(0));
    healthy.store(true, Ordering::SeqCst);
    eventually("the address is probed up", || !pool.is_probed_down(0));
    pool.close();
    up.shutdown();
}
