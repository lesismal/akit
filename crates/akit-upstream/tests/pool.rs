//! Upstream pools (I14): the limits on connections and on requests waiting for one, the idle
//! connections kept, a pool drained under load, and an address that refuses connections (one
//! that does not answer them is akit-listener's test, since its engine sets the connect
//! timeout). The pools dial through an engine whose only threads are a listener's, on the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`), as `akit run`'s pools dial on its
//! listeners' engine.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, Listener, UringConfig};
use akio_http::client::{ClientError, ClientRequest};
use akit_upstream::{EngineSlot, Error, Outcome, Pool};

/// An engine with one listener (answering 404), whose threads host the pools' connections.
fn engine() -> Arc<Engine> {
    let backend = match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    };
    let e = Engine::builder()
        .listen(
            Listener::tcp("127.0.0.1:0".parse().unwrap())
                .backend(backend)
                .serve(akio_http::serve(|ctx: &mut akio_http::Context| {
                    ctx.text(404, "");
                })),
        )
        .build()
        .unwrap();
    e.start().unwrap();
    Arc::new(e)
}

/// A blocking HTTP/1.1 upstream, a thread per connection, that holds every response until it is
/// let go, and counts its connections.
struct Upstream {
    addr: SocketAddr,
    accepted: AtomicUsize,
    open: AtomicUsize,
    most_open: AtomicUsize,
    closed: AtomicUsize,
    /// Requests read and not yet answered.
    held: AtomicUsize,
    gate: (Mutex<bool>, Condvar),
}

impl Upstream {
    /// An upstream that answers at once (`hold` false) or once let go.
    fn start(hold: bool) -> Arc<Upstream> {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let u = Arc::new(Upstream {
            addr: l.local_addr().unwrap(),
            accepted: AtomicUsize::new(0),
            open: AtomicUsize::new(0),
            most_open: AtomicUsize::new(0),
            closed: AtomicUsize::new(0),
            held: AtomicUsize::new(0),
            gate: (Mutex::new(!hold), Condvar::new()),
        });
        let u2 = u.clone();
        thread::spawn(move || {
            for s in l.incoming() {
                let Ok(s) = s else { return };
                u2.accepted.fetch_add(1, Ordering::SeqCst);
                let n = u2.open.fetch_add(1, Ordering::SeqCst) + 1;
                u2.most_open.fetch_max(n, Ordering::SeqCst);
                let u3 = u2.clone();
                thread::spawn(move || u3.serve(s));
            }
        });
        u
    }

    fn serve(&self, mut s: TcpStream) {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        'conn: loop {
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut chunk) {
                    Ok(0) | Err(_) => break 'conn,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            }
            let end = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
            buf.drain(..end);
            self.held.fetch_add(1, Ordering::SeqCst);
            {
                let (open, cv) = &self.gate;
                let mut g = open.lock().unwrap();
                while !*g {
                    g = cv.wait(g).unwrap();
                }
            }
            self.held.fetch_sub(1, Ordering::SeqCst);
            if s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .is_err()
            {
                break;
            }
        }
        self.open.fetch_sub(1, Ordering::SeqCst);
        self.closed.fetch_add(1, Ordering::SeqCst);
    }

    fn release(&self) {
        let (open, cv) = &self.gate;
        *open.lock().unwrap() = true;
        cv.notify_all();
    }
}

/// Waits up to 10s for `f` to hold.
fn eventually(what: &str, f: impl Fn() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < Duration::from_secs(10), "never: {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

/// An upstream config: `addresses`, then `fields`.
fn upstream(addresses: &[String], fields: &str) -> akit_config::Upstream {
    let list: Vec<String> = addresses.iter().map(|a| format!("{a:?}")).collect();
    let json = format!(
        r#"{{"upstreams": [{{"name": "app", "addresses": [{}]{}{fields}}}]}}"#,
        list.join(", "),
        if fields.is_empty() { "" } else { ", " }
    );
    akit_config::from_json(&json, None)
        .unwrap()
        .upstreams
        .remove(0)
}

/// Sends `n` GETs of `/n/i` through the pool; their outcomes arrive on the receiver.
fn send(pool: &Pool, n: usize) -> Receiver<(usize, Outcome)> {
    let (tx, rx) = channel();
    for i in 0..n {
        let tx = tx.clone();
        pool.request(ClientRequest::get(&format!("/n/{i}")), move |r| {
            let _ = tx.send((i, r));
        });
    }
    rx
}

/// The requests under way to the pool's first address, and waiting for it.
fn load(pool: &Pool) -> (usize, usize) {
    let l = pool.load(0).unwrap();
    (l.busy, l.waiting)
}

/// The pool's requests in flight once the last callback has returned: a request's callback runs
/// before the pool counts it finished, on the I/O thread, so the count can lag the result the
/// test thread got by a moment.
fn settled(pool: &Pool) -> usize {
    let deadline = Instant::now() + Duration::from_secs(5);
    while pool.in_flight() != 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    pool.in_flight()
}

fn ok(r: &Outcome) -> bool {
    matches!(r, Ok(resp) if resp.status == 200 && resp.body == b"ok")
}

#[test]
fn max_conns_and_max_pending() {
    let up = Upstream::start(true);
    let pool = Pool::new(
        &upstream(
            &[up.addr.to_string()],
            r#""max_conns": 2, "max_pending": 3"#,
        ),
        EngineSlot::with(engine()),
    );
    // Two under way (at max_conns), three waiting (at max_pending): the sixth is one over.
    let rx = send(&pool, 5);
    eventually("two held, three waiting", || {
        up.held.load(Ordering::SeqCst) == 2 && load(&pool) == (2, 3)
    });
    let (tx6, rx6) = channel();
    pool.request(ClientRequest::get("/six"), move |r| {
        let _ = tx6.send(r);
    });
    let six = rx6.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(six, Err(Error::Busy)), "{six:?}");
    assert!(!six.unwrap_err().is_upstream_failure());
    // Nothing more reached the upstream meanwhile.
    thread::sleep(Duration::from_millis(100));
    assert_eq!(up.accepted.load(Ordering::SeqCst), 2);
    assert_eq!(up.held.load(Ordering::SeqCst), 2);
    up.release();
    let mut done = [false; 5];
    for _ in 0..5 {
        let (i, r) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(ok(&r), "{i}: {r:?}");
        done[i] = true;
    }
    assert!(done.iter().all(|d| *d));
    // The waiting requests went over the two connections, kept alive: never more than two.
    assert_eq!(up.accepted.load(Ordering::SeqCst), 2);
    assert_eq!(up.most_open.load(Ordering::SeqCst), 2);
    assert_eq!(settled(&pool), 0);
    assert_eq!(load(&pool), (0, 0));
}

#[test]
fn max_pending_zero_is_fits_default() {
    let up = Upstream::start(true);
    let pool = Pool::new(
        &upstream(
            &[up.addr.to_string()],
            r#""max_conns": 1, "max_pending": 0"#,
        ),
        EngineSlot::with(engine()),
    );
    // 0 is fit's default, 50000, not none: a request past max_conns waits.
    let rx = send(&pool, 3);
    eventually("one held, two waiting", || load(&pool) == (1, 2));
    up.release();
    for _ in 0..3 {
        let (i, r) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(ok(&r), "{i}: {r:?}");
    }
    assert_eq!(up.most_open.load(Ordering::SeqCst), 1);
}

#[test]
fn idle_connections_kept_and_closed() {
    let up = Upstream::start(false);
    let engine = engine();
    // One kept idle: requests one after another share it.
    let pool = Pool::new(
        &upstream(&[up.addr.to_string()], r#""max_idle_conns": 1"#),
        EngineSlot::with(engine.clone()),
    );
    for _ in 0..3 {
        let (_, r) = send(&pool, 1).recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(ok(&r), "{r:?}");
    }
    assert_eq!(up.accepted.load(Ordering::SeqCst), 1);
    // None kept: each request opens a connection and closes it.
    let up = Upstream::start(false);
    let pool = Pool::new(
        &upstream(&[up.addr.to_string()], r#""max_idle_conns": 0"#),
        EngineSlot::with(engine.clone()),
    );
    for _ in 0..3 {
        let (_, r) = send(&pool, 1).recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(ok(&r), "{r:?}");
    }
    assert_eq!(up.accepted.load(Ordering::SeqCst), 3);
    eventually("all three closed", || up.closed.load(Ordering::SeqCst) == 3);
    // Kept idle for idle_timeout, then closed.
    let up = Upstream::start(false);
    let pool = Pool::new(
        &upstream(&[up.addr.to_string()], r#""idle_timeout": "200ms""#),
        EngineSlot::with(engine),
    );
    let (_, r) = send(&pool, 1).recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(ok(&r), "{r:?}");
    thread::sleep(Duration::from_millis(50));
    assert_eq!(
        up.closed.load(Ordering::SeqCst),
        0,
        "closed before its idle timeout"
    );
    eventually("the idle connection closed", || {
        up.closed.load(Ordering::SeqCst) == 1
    });
}

#[test]
fn drained_under_load() {
    let up = Upstream::start(true);
    let pool = Pool::new(
        &upstream(&[up.addr.to_string()], r#""max_conns": 4"#),
        EngineSlot::with(engine()),
    );
    let (closed_tx, closed_rx) = channel();
    pool.on_closed(move || {
        let _ = closed_tx.send(());
    });
    // 4 under way, 16 waiting, when the pool is closed.
    let rx = send(&pool, 20);
    eventually("four held, sixteen waiting", || {
        up.held.load(Ordering::SeqCst) == 4 && load(&pool) == (4, 16)
    });
    pool.close();
    assert!(pool.is_closing() && !pool.is_closed());
    // New requests are refused at once...
    let (tx, late) = channel();
    pool.request(ClientRequest::get("/late"), move |r| {
        let _ = tx.send(r);
    });
    let r = late.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(r, Err(Error::Closed)), "{r:?}");
    assert!(closed_rx.try_recv().is_err());
    assert_eq!(pool.in_flight(), 20);
    // ...while the twenty sent before finish, the waiting ones included.
    up.release();
    for _ in 0..20 {
        let (i, r) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(ok(&r), "{i}: {r:?}");
    }
    closed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(pool.is_closed());
    assert_eq!(settled(&pool), 0);
    assert!(up.most_open.load(Ordering::SeqCst) <= 4);
    // Closed for good, and a callback added now runs at once.
    let (tx, again) = channel();
    pool.on_closed(move || {
        let _ = tx.send(());
    });
    again.try_recv().unwrap();
    pool.close();
}

#[test]
fn an_idle_pool_closes_at_once() {
    let pool = Pool::new(&upstream(&["127.0.0.1:1".into()], ""), EngineSlot::new());
    pool.close();
    assert!(pool.is_closed());
}

/// An address on this host where nothing listens.
fn refusing() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let a = l.local_addr().unwrap().to_string();
    drop(l);
    a
}

#[test]
fn an_upstream_that_refuses_connections() {
    let pool = Pool::new(
        &upstream(&[refusing()], r#""max_conns": 1, "max_pending": 10"#),
        EngineSlot::with(engine()),
    );
    // One under way at a time, the rest waiting: each fails in turn, none is left waiting.
    let rx = send(&pool, 8);
    for _ in 0..8 {
        let (i, r) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        match &r {
            Err(e @ Error::Client(ClientError::Io(io))) => {
                assert_eq!(io.kind(), ErrorKind::ConnectionRefused, "{i}: {io}");
                assert!(e.is_upstream_failure());
            }
            other => panic!("{i}: {other:?}"),
        }
    }
    assert_eq!(settled(&pool), 0);
    assert_eq!(load(&pool), (0, 0));
    // A pool of a refusing address and one that answers: requests in turn, each its own way.
    let up = Upstream::start(false);
    let pool = Pool::new(
        &upstream(&[refusing(), up.addr.to_string()], ""),
        EngineSlot::with(engine()),
    );
    let rx = send(&pool, 4);
    let mut by_index = vec![None, None, None, None];
    for _ in 0..4 {
        let (i, r) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        by_index[i] = Some(r);
    }
    for (i, r) in by_index.into_iter().enumerate() {
        let r = r.unwrap();
        if i % 2 == 0 {
            assert!(
                matches!(r, Err(Error::Client(ClientError::Io(_)))),
                "{i}: {r:?}"
            );
        } else {
            assert!(ok(&r), "{i}: {r:?}");
        }
    }
}

#[test]
fn without_an_engine() {
    let slot = EngineSlot::new();
    let pool = Pool::new(&upstream(&["127.0.0.1:1".into()], ""), slot.clone());
    let (_, r) = send(&pool, 1).recv().unwrap();
    assert!(matches!(r, Err(Error::NoEngine)), "{r:?}");
    assert!(!r.unwrap_err().is_upstream_failure());
    // Filled later, as `akit run` fills it once its engine is built.
    let up = Upstream::start(false);
    let pool = Pool::new(&upstream(&[up.addr.to_string()], ""), slot.clone());
    assert!(slot.set(engine()));
    assert!(!slot.set(engine()));
    let (_, r) = send(&pool, 1).recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(ok(&r), "{r:?}");
}

#[test]
fn bad_targets_and_timeouts() {
    let up = Upstream::start(true);
    let pool = Pool::new(
        &upstream(&[up.addr.to_string()], r#""max_conns": 1"#),
        EngineSlot::with(engine()),
    )
    .with_timeout(Duration::from_millis(300));
    let (tx, rx) = channel();
    let t = tx.clone();
    pool.send(0, ClientRequest::get("http://elsewhere/"), move |r| {
        let _ = t.send(r);
    });
    let r = rx.recv().unwrap();
    assert!(
        matches!(r, Err(Error::Client(ClientError::InvalidUrl(_)))),
        "{r:?}"
    );
    let t = tx.clone();
    pool.send(7, ClientRequest::get("/"), move |r| {
        let _ = t.send(r);
    });
    let r = rx.recv().unwrap();
    assert!(
        matches!(r, Err(Error::Client(ClientError::InvalidUrl(_)))),
        "{r:?}"
    );
    // The one under way times out at the pool's timeout; the one that waited behind it has used
    // up its time waiting.
    let rx2 = send(&pool, 2);
    for _ in 0..2 {
        let (i, r) = rx2.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            matches!(
                r,
                Err(Error::Client(ClientError::Timeout)) | Err(Error::Timeout)
            ),
            "{i}: {r:?}"
        );
    }
    assert_eq!(settled(&pool), 0);
    up.release();
}
