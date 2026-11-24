//! Passive health checks (I20): fit's tests (`upstream/health_test.go`, and
//! `TestWeightedRoundRobinDown` of `balance_test.go`), ported. Counting failures and the window
//! run on a clock the test sets; the balancers pass over addresses that are down, and ignore
//! health when all are; then a pool takes an address that refuses connections out of rotation
//! and back after `fail_timeout`, on the backend `AKIO_TEST_BACKEND` names (epoll by default,
//! `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::cell::Cell;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::thread;
use std::time::Duration;

use akio::{Backend, Engine, Listener, UringConfig};
use akio_http::client::ClientRequest;
use akit_upstream::health::nanos;
use akit_upstream::{Balancer, EngineSlot, Health, LoadBalancing, Outcome, Pool};

const HOUR: Duration = Duration::from_secs(3600);

/// A clock the test moves: what `Health` is given as the time.
struct Clock(Cell<Duration>);

impl Clock {
    fn new() -> Clock {
        Clock(Cell::new(Duration::from_secs(1)))
    }
    fn now(&self) -> u64 {
        nanos(self.0.get())
    }
    fn advance(&self, d: Duration) {
        self.0.set(self.0.get() + d);
    }
}

fn down(h: &Health, c: &Clock) -> bool {
    !h.up(|| c.now())
}

#[test]
fn max_fails() {
    let c = Clock::new();
    let b = Health::new();
    b.failed(2, HOUR, || c.now());
    assert!(!down(&b, &c), "down after 1 of 2 failures");
    b.failed(2, HOUR, || c.now());
    assert!(down(&b, &c), "up after 2 of 2 failures");
    b.succeeded();
    assert!(!down(&b, &c), "down after a success");
    assert_eq!(b.down_until(), None);
    // Failures outside the window do not add up.
    let window = Duration::from_millis(30);
    b.failed(2, window, || c.now());
    c.advance(Duration::from_millis(50));
    b.failed(2, window, || c.now());
    assert!(!down(&b, &c), "down after failures in two windows");
    // Within the window they do, and the address is out for the window from the last.
    c.advance(Duration::from_millis(29));
    b.failed(2, window, || c.now());
    assert!(down(&b, &c), "up after 2 failures in one window");
    c.advance(Duration::from_millis(29));
    assert!(down(&b, &c), "back before fail_timeout");
    c.advance(Duration::from_millis(1));
    assert!(!down(&b, &c), "still down after fail_timeout");
    // No max_fails: never down.
    let b = Health::new();
    for _ in 0..3 {
        b.failed(0, HOUR, || c.now());
    }
    assert!(!down(&b, &c), "down with max_fails 0");
    // max_fails 1, fit's default: one failure is enough.
    b.failed(1, HOUR, || c.now());
    assert!(down(&b, &c), "up after 1 of 1 failure");
}

#[test]
fn a_success_clears_the_failures_counted() {
    let c = Clock::new();
    let b = Health::new();
    b.failed(3, HOUR, || c.now());
    b.failed(3, HOUR, || c.now());
    b.succeeded();
    b.failed(3, HOUR, || c.now());
    b.failed(3, HOUR, || c.now());
    assert!(!down(&b, &c), "failures before a success still counted");
    b.failed(3, HOUR, || c.now());
    assert!(down(&b, &c));
}

/// How often each of `b`'s addresses came up in `n` picks with every address idle, `up` saying
/// which are up.
fn counts(b: &Balancer, n: usize, up: &impl Fn(usize) -> bool) -> Vec<usize> {
    let mut got = vec![0; b.len()];
    for _ in 0..n {
        got[b.pick_up(|_| 0, up)] += 1;
    }
    got
}

#[test]
fn picks_skip_down() {
    for policy in [
        LoadBalancing::RoundRobin,
        LoadBalancing::WeightedRoundRobin,
        LoadBalancing::LeastConn,
    ] {
        let c = Clock::new();
        let hs: Vec<Health> = (0..3).map(|_| Health::new()).collect();
        let b = Balancer::new(policy, &[1, 1, 1]);
        let window = Duration::from_millis(100);
        let up = |i: usize| hs[i].up(|| c.now());
        hs[1].failed(1, window, || c.now());
        assert_eq!(
            counts(&b, 300, &up),
            [150, 0, 150],
            "{policy:?} with b down"
        );
        // All down: health is ignored rather than failing everything.
        hs[0].failed(1, window, || c.now());
        hs[2].failed(1, window, || c.now());
        let got = counts(&b, 300, &up);
        assert!(
            got.iter().sum::<usize>() == 300 && got[0] > 0 && got[2] > 0,
            "{policy:?} with all down: {got:?}"
        );
        // After the window, they are back.
        c.advance(Duration::from_millis(120));
        assert_eq!(
            counts(&b, 300, &up),
            [100, 100, 100],
            "{policy:?} after the window"
        );
    }
}

#[test]
fn least_conn_skips_down_whatever_its_load() {
    let b = Balancer::new(LoadBalancing::LeastConn, &[1, 1, 1]);
    // b idle but down, a and c busy: a and c share the picks.
    let got = {
        let mut got = [0; 3];
        for _ in 0..300 {
            got[b.pick_up(|i| if i == 1 { 0 } else { 5 }, |i| i != 1)] += 1;
        }
        got
    };
    assert_eq!(got, [150, 0, 150]);
    // All down: the least busy, as without health.
    for _ in 0..10 {
        assert_eq!(b.pick_up(|i| if i == 1 { 0 } else { 5 }, |_| false), 1);
    }
}

#[test]
fn weighted_round_robin_down() {
    // a, of weight 5, down: its share goes to b and c by their weights.
    let b = Balancer::new(LoadBalancing::WeightedRoundRobin, &[5, 2, 1]);
    let got = counts(&b, 9000, &|i| i != 0);
    assert!(
        got[0] == 0 && (5700..=6300).contains(&got[1]) && (2700..=3300).contains(&got[2]),
        "a down: {got:?}"
    );
    // All down: health is ignored, and the weights hold again.
    assert_eq!(counts(&b, 8000, &|_| false), [5000, 2000, 1000], "all down");
    assert_eq!(counts(&b, 800, &|i| i == 0), [800, 0, 0], "only a up");
}

#[test]
fn weighted_round_robin_down_without_a_schedule() {
    // Too many weights to work out ahead: picked under the slot's lock, over the addresses up.
    let weights: Vec<u32> = (0..100).map(|i| 997 - i % 2).collect();
    let b = Balancer::new(LoadBalancing::WeightedRoundRobin, &weights);
    // Weights 997 and 996: whole rounds of the two give them exactly that.
    let got = counts(&b, 997 + 996, &|i| i < 2);
    assert_eq!(got[..2], [997, 996], "only the first two up");
    assert_eq!(got[2..].iter().sum::<usize>(), 0);
    let got = counts(&b, 5000, &|_| false);
    assert_eq!(got.iter().sum::<usize>(), 5000);
    assert!(got.iter().all(|&n| n >= 40), "all down: {got:?}");
}

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

/// A blocking HTTP/1.1 upstream answering every request with `body` and closing the connection,
/// on `l` until stopped: then its port refuses connections.
struct Server {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Server {
    fn start(l: TcpListener, body: &'static str) -> Server {
        l.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let thread = thread::spawn(move || {
            while !s2.load(Ordering::SeqCst) {
                match l.accept() {
                    Ok((s, _)) => {
                        thread::spawn(move || answer(s, body));
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => return,
                }
            }
        });
        Server {
            stop,
            thread: Some(thread),
        }
    }

    /// Stops it, and closes its listener.
    fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            t.join().unwrap();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

fn answer(mut s: TcpStream, body: &str) {
    s.set_nonblocking(false).unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let _ = s.write_all(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    );
}

/// An upstream config: `addresses`, then `fields`.
fn upstream(addresses: &[String], fields: &str) -> akit_config::Upstream {
    let list: Vec<String> = addresses.iter().map(|a| format!("{a:?}")).collect();
    let json = format!(
        r#"{{"upstreams": [{{"name": "app", "addresses": [{}], {fields}}}]}}"#,
        list.join(", "),
    );
    akit_config::from_json(&json, None)
        .unwrap()
        .upstreams
        .remove(0)
}

/// One request through `pool`, waited for.
fn get(pool: &Pool) -> Outcome {
    let (tx, rx) = channel();
    pool.request(ClientRequest::get("/"), move |r| {
        let _ = tx.send(r);
    });
    rx.recv_timeout(Duration::from_secs(10)).unwrap()
}

/// What `n` requests through `pool`, one after the other, got: `g`, `f` or `error`. Like fit's
/// test, rather than count on the requests being quick, it checks what the window promises of
/// each: none reaches flaky (address 1) while it is out, and a failure takes it out for
/// `fail_timeout`.
fn seen(pool: &Pool, n: usize, errors_ok: bool) -> (usize, usize, usize) {
    let (mut g, mut f, mut errors) = (0, 0, 0);
    for _ in 0..n {
        let was_down = pool.is_down(1);
        match get(pool) {
            Ok(r) if r.body == b"g" => g += 1,
            Ok(r) if r.body == b"f" => {
                assert!(
                    !was_down || !pool.is_down(1),
                    "flaky took a request while down"
                );
                f += 1;
            }
            Ok(r) => panic!("answered {r:?}"),
            Err(e) => {
                assert!(errors_ok, "error {e}");
                assert!(!was_down, "flaky took a request while down: {e}");
                assert!(e.is_upstream_failure(), "{e}");
                assert!(pool.is_down(1), "a failure did not take flaky out");
                errors += 1;
            }
        }
    }
    (g, f, errors)
}

#[test]
fn leave_and_rejoin() {
    let gl = TcpListener::bind("127.0.0.1:0").unwrap();
    let good_addr = gl.local_addr().unwrap().to_string();
    let _good = Server::start(gl, "g");
    let fl = TcpListener::bind("127.0.0.1:0").unwrap();
    let flaky_addr = fl.local_addr().unwrap().to_string();
    let mut flaky = Server::start(fl, "f");
    let fail_timeout = Duration::from_millis(300);
    let pool = Pool::new(
        &upstream(
            &[good_addr, flaky_addr.clone()],
            r#""fail_timeout": "300ms""#,
        ),
        EngineSlot::with(engine()),
    );
    assert_eq!((pool.max_fails(), pool.fail_timeout()), (1, fail_timeout));
    assert_eq!(seen(&pool, 10, false), (5, 5, 0), "both up");

    flaky.stop(); // its port now refuses connections
    // The first request to flaky fails and takes it out: one error in a quick run, one more each
    // fail_timeout in a slow one.
    let (g, f, errors) = seen(&pool, 20, true);
    assert!(
        errors >= 1 && f == 0 && g + errors == 20,
        "{g} {f} {errors}"
    );
    assert!(pool.available(), "the good address is up");

    // Back on the same port; while its window lasts, nothing reaches it, and after it, it is
    // tried again.
    let Ok(fl) = TcpListener::bind(&flaky_addr) else {
        eprintln!("the port did not come back; skipped the rest");
        return;
    };
    let _flaky = Server::start(fl, "f");
    let (_, _, errors) = seen(&pool, 10, false);
    assert_eq!(errors, 0);
    thread::sleep(fail_timeout + Duration::from_millis(50));
    assert!(!pool.is_down(1), "flaky is still down after fail_timeout");
    assert_eq!(seen(&pool, 10, false), (5, 5, 0), "after rejoining");
}

#[test]
fn all_down_still_picks() {
    // Both addresses refuse: each request fails, every address is out, and requests still go
    // out (to both) rather than failing without trying.
    let refusing = || {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().to_string()
    };
    let pool = Pool::new(
        &upstream(&[refusing(), refusing()], r#""fail_timeout": "1h""#),
        EngineSlot::with(engine()),
    );
    for _ in 0..6 {
        let r = get(&pool);
        assert!(matches!(&r, Err(e) if e.is_upstream_failure()), "{r:?}");
    }
    assert!(pool.is_down(0) && pool.is_down(1) && !pool.available());
    // max_fails 0: never out.
    let pool = Pool::new(
        &upstream(&[refusing()], r#""max_fails": 0"#),
        EngineSlot::with(engine()),
    );
    let _ = get(&pool);
    assert!(!pool.is_down(0) && pool.available());
}

#[test]
fn max_fails_counts_through_the_pool() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let a = l.local_addr().unwrap().to_string();
    drop(l);
    let pool = Pool::new(
        &upstream(&[a], r#""max_fails": 3, "fail_timeout": "1h""#),
        EngineSlot::with(engine()),
    );
    for i in 0..3 {
        assert!(!pool.is_down(0), "down after {i} of 3 failures");
        assert!(get(&pool).is_err());
    }
    assert!(pool.is_down(0), "up after 3 of 3 failures");
}
