//! Active health checks (I35): fit's tests of its prober (`upstream/health.go`'s `record`,
//! `TestProberEdges`, `TestWeightedRoundRobinDown`'s probed addresses, and the e2e
//! `TestHealthCheckRotation`), ported to akit's pools. The probes run on an engine on the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`), against blocking upstreams whose probe
//! path the tests make hang, fail or answer.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, Listener, UringConfig};
use akio_http::client::ClientRequest;
use akit_upstream::health::nanos;
use akit_upstream::probe::USER_AGENT;
use akit_upstream::{EngineSlot, Health, Pool};

/// An engine with one listener (answering 404), whose threads host the pools' connections and
/// probes.
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

/// How an upstream answers its probe path, `/health`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Probe {
    /// With the status in `Upstream::status`.
    Answer,
    /// Never: the probe times out.
    Hang,
    /// With a head announcing a body far larger than it sends, then nothing more: the probe
    /// must count the status and close the connection.
    Huge,
}

/// A blocking HTTP/1.1 upstream, a thread per connection, answering `/health` as `probe` says
/// and anything else 200 with its name, until dropped: then its port refuses connections.
struct Upstream {
    addr: String,
    name: &'static str,
    probe: Mutex<Probe>,
    status: AtomicU16,
    /// Probes received.
    probes: AtomicUsize,
    /// The last probe's `Host` and `User-Agent`.
    seen: Mutex<(String, String)>,
    /// Connections of `Huge` probes the prober closed.
    closed: AtomicUsize,
    stop: AtomicBool,
}

impl Upstream {
    fn start(name: &'static str) -> Arc<Upstream> {
        Upstream::on(TcpListener::bind("127.0.0.1:0").unwrap(), name)
    }

    fn on(l: TcpListener, name: &'static str) -> Arc<Upstream> {
        l.set_nonblocking(true).unwrap();
        let u = Arc::new(Upstream {
            addr: l.local_addr().unwrap().to_string(),
            name,
            probe: Mutex::new(Probe::Answer),
            status: AtomicU16::new(200),
            probes: AtomicUsize::new(0),
            seen: Mutex::new((String::new(), String::new())),
            closed: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
        });
        let me = Arc::clone(&u);
        thread::spawn(move || {
            while !me.stop.load(Ordering::SeqCst) {
                match l.accept() {
                    Ok((s, _)) => {
                        let me = Arc::clone(&me);
                        thread::spawn(move || me.serve(s));
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => return,
                }
            }
        });
        u
    }

    fn set(&self, p: Probe) {
        *self.probe.lock().unwrap() = p;
    }

    fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        // The accept loop sees it within its sleep; wait for the port to close.
        thread::sleep(Duration::from_millis(20));
    }

    fn serve(&self, mut s: TcpStream) {
        s.set_nonblocking(false).unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            match s.read(&mut chunk) {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        let head = String::from_utf8_lossy(&buf).into_owned();
        let path = head.split(' ').nth(1).unwrap_or("");
        if path != "/health" {
            let _ = s.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    self.name.len(),
                    self.name
                )
                .as_bytes(),
            );
            return;
        }
        let field = |name: &str| {
            head.lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
                })
                .unwrap_or_default()
        };
        *self.seen.lock().unwrap() = (field("host"), field("user-agent"));
        self.probes.fetch_add(1, Ordering::SeqCst);
        let probe = *self.probe.lock().unwrap();
        match probe {
            Probe::Answer => {
                let status = self.status.load(Ordering::SeqCst);
                let _ = s.write_all(
                    format!(
                        "HTTP/1.1 {status} X\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                    )
                    .as_bytes(),
                );
            }
            Probe::Hang => {
                // Held until the prober gives up on it.
                s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let _ = s.read(&mut chunk);
            }
            Probe::Huge => {
                // Before the write: the prober closes once it has the head, and macOS refuses a
                // socket's options (`EINVAL`) once its peer has reset it.
                s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let _ =
                    s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000000000\r\n\r\nsome of it");
                if matches!(s.read(&mut chunk), Ok(0)) {
                    self.closed.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
    }
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// A pool of `addresses`, with `fields` after them, dialing through `engine`.
fn pool(engine: EngineSlot, addresses: &[&str], fields: &str) -> Pool {
    let list: Vec<String> = addresses.iter().map(|a| format!("{a:?}")).collect();
    let json = format!(
        r#"{{"upstreams": [{{"name": "app", "addresses": [{}], {fields}}}]}}"#,
        list.join(", "),
    );
    let u = akit_config::from_json(&json, None)
        .unwrap()
        .upstreams
        .remove(0);
    Pool::new(&u, engine)
}

/// Waits up to `limit` for `f` to hold; how long it took.
fn until(what: &str, limit: Duration, f: impl Fn() -> bool) -> Duration {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < limit, "{what}: not within {limit:?}");
        thread::sleep(Duration::from_millis(5));
    }
    start.elapsed()
}

/// The body of one request through `pool`: the name of the upstream that answered it.
fn get(pool: &Pool) -> String {
    let (tx, rx) = channel();
    pool.request(ClientRequest::get("/"), move |r| {
        let _ = tx.send(r);
    });
    let r = rx
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .expect("a request failed");
    assert_eq!(r.status, 200);
    String::from_utf8(r.body).unwrap()
}

/// fit's `record`, through `Health`: probes and failures take an address out each on their own
/// account; a success clears the failures, not the probes.
#[test]
fn probed_down_and_failures_are_kept_apart() {
    let h = Health::new();
    let at = |s: u64| nanos(Duration::from_secs(s));
    assert!(h.up(|| at(1)) && !h.probed_down());
    h.set_probed_down(true);
    assert!(h.probed_down());
    assert!(
        !h.up(|| at(1)) && !h.up(|| at(1_000_000)),
        "probed down is down for good"
    );
    // A response does not put it back: only its probes do.
    h.succeeded();
    assert!(!h.up(|| at(1_000_000)));
    // Failures while it is out are counted, and keep it out past its probes passing.
    h.failed(1, Duration::from_secs(10), || at(1));
    h.set_probed_down(false);
    assert!(!h.probed_down());
    assert!(!h.up(|| at(5)), "failures took it out for fail_timeout");
    assert!(h.up(|| at(12)), "back after fail_timeout");
    // And a success then clears them.
    h.failed(1, Duration::from_secs(10), || at(20));
    assert!(!h.up(|| at(21)));
    h.succeeded();
    assert!(h.up(|| at(21)));
    assert_eq!(h.down_until(), None);
}

/// fit's e2e `TestHealthCheckRotation`, on a pool: an address whose probe path stops answering
/// (it hangs, so the probe times out) while its other requests are still served leaves rotation
/// within the window and comes back once it answers again, with no request failing meanwhile.
#[test]
fn health_check_rotation() {
    let a = Upstream::start("a");
    let b = Upstream::start("b");
    let interval = Duration::from_millis(200);
    let timeout = Duration::from_millis(100);
    let (fails, passes) = (2u32, 2u32);
    let p = pool(
        EngineSlot::with(engine()),
        &[&a.addr, &b.addr],
        r#""health_check": {"path": "/health", "interval": "200ms", "timeout": "100ms",
            "jitter": "1ms", "fails": 2, "passes": 2}"#,
    );
    assert!(p.health_check().is_some());
    until("both in rotation", Duration::from_secs(5), || {
        get(&p) == "b"
    });
    until("probed", Duration::from_secs(5), || {
        b.probes.load(Ordering::SeqCst) >= 2
    });

    // The window: fails probes, each started an interval apart and failing once its timeout
    // passes, plus a probe interval of slack for when the hang began within one.
    let window = interval * fails + timeout + interval;
    b.set(Probe::Hang);
    let left = until("out of rotation", window * 10, || p.is_probed_down(1));
    // Slack for a loaded host.
    assert!(
        left < window + Duration::from_secs(2),
        "left rotation after {left:?}, past the window of {window:?}"
    );
    assert!(p.is_down(1) && p.available());
    for _ in 0..20 {
        assert_eq!(
            get(&p),
            "a",
            "an address out of rotation was sent a request"
        );
    }

    let back = interval * passes + interval;
    b.set(Probe::Answer);
    let returned = until("back in rotation", back * 10, || !p.is_probed_down(1));
    assert!(
        returned < back + Duration::from_secs(2),
        "came back after {returned:?}, past the window of {back:?}"
    );
    until("b serves again", Duration::from_secs(5), || get(&p) == "b");
    p.close();
}

/// An http probe sends the check's `Host` and akit's `User-Agent`, passes on the statuses
/// `expect_status` lists (200 to 399 without it), and closes a response's connection once its
/// head is in, without reading its body.
#[test]
fn http_probe_status_host_and_body() {
    let e = engine();
    // One upstream for each pool, so that each sees its own pool's probes alone.
    let u = Upstream::start("u");
    let v = Upstream::start("v");
    u.status.store(503, Ordering::SeqCst);
    v.status.store(503, Ordering::SeqCst);
    // Each probe may take its whole interval, 250 ms, and two must fail to take an address out:
    // a probe a loaded host slows is no failure (40 ms with one failure were).
    let fast =
        r#""interval": "250ms", "timeout": "250ms", "jitter": "1ms", "fails": 2, "passes": 1"#;
    let any = pool(
        EngineSlot::with(e.clone()),
        &[&u.addr],
        &format!(r#""health_check": {{"path": "/health", "host": "app.internal", {fast}}}"#),
    );
    let listed = pool(
        EngineSlot::with(e.clone()),
        &[&v.addr],
        &format!(r#""health_check": {{"path": "/health", "expect_status": [503], {fast}}}"#),
    );
    until(
        "503 fails the default check",
        Duration::from_secs(10),
        || any.is_probed_down(0),
    );
    assert_eq!(
        *u.seen.lock().unwrap(),
        ("app.internal".to_owned(), USER_AGENT.to_owned())
    );
    until("probes of the other", Duration::from_secs(10), || {
        v.probes.load(Ordering::SeqCst) >= 4
    });
    assert!(!listed.is_probed_down(0), "503 is expected there");
    // Without the check's host, the address's.
    assert_eq!(
        *v.seen.lock().unwrap(),
        (v.addr.clone(), USER_AGENT.to_owned())
    );
    // 200 to 399 pass without expect_status.
    u.status.store(399, Ordering::SeqCst);
    until("399 passes", Duration::from_secs(10), || {
        !any.is_probed_down(0)
    });
    v.status.store(200, Ordering::SeqCst);
    until(
        "200 fails expect_status [503]",
        Duration::from_secs(10),
        || listed.is_probed_down(0),
    );
    // A huge body is not read: the status counts, and the connection is closed.
    u.set(Probe::Huge);
    until(
        "the probe closed the connection",
        Duration::from_secs(10),
        || u.closed.load(Ordering::SeqCst) >= 2,
    );
    assert!(!any.is_probed_down(0));
    any.close();
    listed.close();
}

/// A tcp probe passes once a connection opens: a check's `port` probes there rather than at the
/// address, whose port refusing connections takes it out, and listening again brings it back.
#[test]
fn tcp_probe_of_the_checks_port() {
    let u = Upstream::start("u");
    let hl = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = hl.local_addr().unwrap().port();
    let health = Upstream::on(hl, "h");
    let p = pool(
        EngineSlot::with(engine()),
        &[&u.addr],
        &format!(
            r#""health_check": {{"type": "tcp", "port": {port}, "interval": "50ms",
                "timeout": "40ms", "jitter": "1ms", "fails": 2, "passes": 2}}"#
        ),
    );
    thread::sleep(Duration::from_millis(300));
    assert!(!p.is_probed_down(0), "the health port is open");
    // A tcp probe sends nothing.
    assert_eq!(health.probes.load(Ordering::SeqCst), 0);
    health.stop();
    drop(health);
    until("out once the port refuses", Duration::from_secs(5), || {
        p.is_probed_down(0)
    });
    // Its own port still answers: only the probes took it out.
    assert_eq!(get(&p), "u");
    let Ok(hl) = TcpListener::bind(("127.0.0.1", port)) else {
        eprintln!("the health port did not come back; skipping the rest");
        return;
    };
    let _health = Upstream::on(hl, "h");
    until("back once it listens", Duration::from_secs(5), || {
        !p.is_probed_down(0)
    });
    p.close();
}

/// fit's `TestProberEdges`, as they apply: a refused tcp probe fails (port 1 refuses), and a
/// closed pool's probes stop.
#[test]
fn refused_probes_and_a_closed_pool() {
    let tcp = pool(
        EngineSlot::with(engine()),
        &["127.0.0.1:1"],
        r#""max_fails": 0, "health_check": {"type": "tcp", "interval": "20ms",
            "timeout": "10ms", "jitter": "1ms", "fails": 1}"#,
    );
    until("a refused tcp probe fails", Duration::from_secs(5), || {
        tcp.is_probed_down(0)
    });
    tcp.close();

    let u = Upstream::start("u");
    let p = pool(
        EngineSlot::with(engine()),
        &[&u.addr],
        r#""health_check": {"path": "/health", "interval": "20ms", "timeout": "10ms",
            "jitter": "1ms"}"#,
    );
    until("probing", Duration::from_secs(5), || {
        u.probes.load(Ordering::SeqCst) >= 3
    });
    p.close();
    // At most a probe under way when it closed is still to come.
    thread::sleep(Duration::from_millis(100));
    let after = u.probes.load(Ordering::SeqCst);
    thread::sleep(Duration::from_millis(300));
    assert_eq!(u.probes.load(Ordering::SeqCst), after, "probes after close");
}

/// A pool made before its engine (as `akit run` makes them) starts probing once the engine is
/// set, and not before.
#[test]
fn probes_wait_for_the_engine() {
    let u = Upstream::start("u");
    let slot = EngineSlot::new();
    let p = pool(
        slot.clone(),
        &[&u.addr],
        r#""health_check": {"path": "/health", "interval": "20ms", "timeout": "10ms",
            "jitter": "1ms"}"#,
    );
    thread::sleep(Duration::from_millis(100));
    assert_eq!(
        u.probes.load(Ordering::SeqCst),
        0,
        "probed without an engine"
    );
    assert!(slot.set(engine()));
    until(
        "probing once the engine is set",
        Duration::from_secs(5),
        || u.probes.load(Ordering::SeqCst) >= 2,
    );
    p.close();
    // A pool without a health check never probes.
    let quiet = Upstream::start("q");
    let none = pool(
        EngineSlot::with(engine()),
        &[&quiet.addr],
        r#""max_fails": 1"#,
    );
    assert!(none.health_check().is_none());
    thread::sleep(Duration::from_millis(100));
    assert_eq!(quiet.probes.load(Ordering::SeqCst), 0);
    assert!(!none.is_probed_down(0));
}
