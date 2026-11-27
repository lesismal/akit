//! M2's exit criterion (I38): an upstream that stops answering its probes leaves rotation within
//! the window and returns after, end to end: requests proxied by akit to an upstream of two
//! addresses with an http `health_check` (I35), one address's probe path made to hang while its
//! other requests are still served (fit's e2e `TestHealthCheckRotation`), no request failing
//! throughout. A reload while it is out keeps it out: the unchanged upstream's pool, and its
//! probed state, carry over (fit's `TestSharedPools`).
//!
//! - `health_window_through_akit_run`: `akit run`, with the address's `akit_upstream_backend_up`
//!   gauge read at the Admin API's `GET /metrics` as it leaves and returns.
//! - `health_window_in_process`: in process, on the backend `AKIO_TEST_BACKEND` names (epoll by
//!   default, `uring`), the probes on that backend's engine.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod m2;
mod support;

use std::io::BufReader;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use m2::target::{Inproc, Proc};
use m2::traffic::{App, WAIT, get_on};

const INTERVAL: Duration = Duration::from_millis(200);
const TIMEOUT: Duration = Duration::from_millis(100);
const FAILS: u32 = 2;
const PASSES: u32 = 2;
/// Room for a loaded host, as `akit-upstream`'s `health_check_rotation` gives.
const SLACK: Duration = Duration::from_secs(2);

/// Listener `web` and, with `api`, the Admin API, each on port 0: where they listen is asked.
fn config(api: bool, a: SocketAddr, b: SocketAddr) -> String {
    let admin = if api {
        r#""admin": {"address": "127.0.0.1:0"}, "metrics": {},"#
    } else {
        ""
    };
    format!(
        r#"{{{admin}
  "listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
  "upstreams": [{{"name": "app", "addresses": ["{a}", "{b}"], "load_balancing": "round_robin",
    "health_check": {{"path": "/health", "interval": "200ms", "timeout": "100ms", "jitter": "1ms",
      "fails": {FAILS}, "passes": {PASSES}}}}}],
  "routes": [{{"handler": {{"module": "proxy", "params": {{"upstream": "app"}}}}}}]
}}"#
    )
}

/// Requests sent through akit every 10 ms on one keep-alive connection: when each was answered
/// and by which address, and what failed.
struct Client {
    answers: Arc<Mutex<Vec<(Instant, String)>>>,
    failed: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: thread::JoinHandle<()>,
}

impl Client {
    fn start(web: SocketAddr) -> Client {
        let answers = Arc::new(Mutex::new(Vec::new()));
        let failed = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (an, f, s) = (answers.clone(), failed.clone(), stop.clone());
        let thread = thread::spawn(move || {
            let c = TcpStream::connect(web).unwrap();
            c.set_read_timeout(Some(WAIT)).unwrap();
            let mut c = BufReader::new(c);
            while !s.load(SeqCst) {
                match get_on(&mut c, "/", false) {
                    Ok(r) if r.status == 200 && !r.closing => {
                        let body = String::from_utf8_lossy(&r.body).into_owned();
                        an.lock().unwrap().push((Instant::now(), body));
                    }
                    Ok(r) => {
                        f.lock()
                            .unwrap()
                            .push(format!("status {} {:?}", r.status, r.body));
                        return;
                    }
                    Err(e) => {
                        f.lock().unwrap().push(e);
                        return;
                    }
                }
                thread::sleep(Duration::from_millis(10));
            }
        });
        Client {
            answers,
            failed,
            stop,
            thread,
        }
    }

    /// The answers since `t`.
    fn since(&self, t: Instant) -> Vec<(Instant, String)> {
        let a = self.answers.lock().unwrap();
        a.iter().filter(|(at, _)| *at >= t).cloned().collect()
    }

    fn failures(&self) -> Vec<String> {
        self.failed.lock().unwrap().clone()
    }

    fn stop(self) -> Vec<String> {
        self.stop.store(true, SeqCst);
        self.thread.join().unwrap();
        self.failed.lock().unwrap().clone()
    }
}

/// Waits up to `within` for `f`; how long it took.
fn until(what: &str, within: Duration, mut f: impl FnMut() -> bool) -> Duration {
    let start = Instant::now();
    loop {
        if f() {
            return start.elapsed();
        }
        assert!(start.elapsed() < within, "{what}: not within {within:?}");
        thread::sleep(Duration::from_millis(5));
    }
}

/// The scrape's `akit_upstream_backend_up` for `addr`, when akit runs with the Admin API.
fn gauge(akit: &Proc, addr: SocketAddr) -> Option<u32> {
    let want = format!("akit_upstream_backend_up{{upstream=\"app\",backend=\"{addr}\"}} ");
    akit.metrics()
        .lines()
        .find_map(|l| l.strip_prefix(&want).and_then(|v| v.trim().parse().ok()))
}

/// The window, through akit at `web` proxying to `a` and `b`; `akit` when it runs as a process,
/// for its gauge; `reload` applies the same config again.
fn window(web: SocketAddr, a: &App, b: &App, akit: Option<&Proc>, reload: &mut dyn FnMut()) {
    let client = Client::start(web);
    let start = Instant::now();
    until("both addresses serve", WAIT, || {
        let seen = client.since(start);
        seen.iter().any(|(_, x)| x == "a") && seen.iter().any(|(_, x)| x == "b")
    });
    until("b probed", WAIT, || b.probes.load(SeqCst) >= 2);
    if let Some(akit) = akit {
        assert_eq!(gauge(akit, b.addr), Some(1));
    }

    // The window: `fails` probes, each started an interval apart and failing once its timeout
    // passes, plus an interval for when the hang began within one.
    let window = INTERVAL * FAILS + TIMEOUT + INTERVAL;
    let t0 = Instant::now();
    b.hang_probes(true);
    // Out once 30 answers in a row (300 ms of them) came from a alone: round robin would have
    // sent b every other one.
    until("b out of rotation", (window + SLACK) * 5, || {
        let seen = client.since(t0);
        seen.len() >= 30 && seen.iter().rev().take(30).all(|(_, x)| x == "a")
    });
    let left = client
        .since(t0)
        .iter()
        .rev()
        .find(|(_, x)| x == "b")
        .map_or(Duration::ZERO, |(at, _)| at.duration_since(t0));
    eprintln!("b left rotation {left:?} after its probes stopped (window {window:?})");
    assert!(
        left < window + SLACK,
        "b was sent requests {left:?} after its probes stopped, past the window of {window:?}"
    );
    if let Some(akit) = akit {
        until("b's gauge 0", WAIT, || gauge(akit, b.addr) == Some(0));
        assert_eq!(gauge(akit, a.addr), Some(1));
    }
    // Sent nothing while out, a reload included: the upstream, unchanged, keeps its pool and
    // so its probed state (fit's `TestSharedPools`); a new pool would put b back in rotation
    // until its probes failed again.
    let served = b.served.load(SeqCst);
    thread::sleep(Duration::from_millis(500));
    reload();
    thread::sleep(Duration::from_millis(500));
    assert_eq!(
        b.served.load(SeqCst),
        served,
        "b was sent requests while out"
    );

    // Back once `passes` probes in a row pass, plus an interval.
    let back = INTERVAL * PASSES + INTERVAL;
    let t1 = Instant::now();
    b.hang_probes(false);
    until("b back in rotation", (back + SLACK) * 5, || {
        client.since(t1).iter().any(|(_, x)| x == "b")
    });
    let returned = client
        .since(t1)
        .iter()
        .find(|(_, x)| x == "b")
        .map(|(at, _)| at.duration_since(t1))
        .unwrap();
    eprintln!("b came back {returned:?} after its probes passed again (window {back:?})");
    assert!(
        returned < back + SLACK,
        "b came back after {returned:?}, past the window of {back:?}"
    );
    if let Some(akit) = akit {
        until("b's gauge 1", WAIT, || gauge(akit, b.addr) == Some(1));
    }
    assert!(client.failures().is_empty(), "{:?}", client.failures());
    let failed = client.stop();
    assert!(failed.is_empty(), "failed: {failed:?}");
}

#[test]
fn health_window_through_akit_run() {
    let (a, b) = (App::start("a"), App::start("b"));
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("health-window.json");
    let json = config(true, a.addr, b.addr);
    let akit = Proc::start(&path, &json, "5s", "akit: admin API on");
    let web = akit.listener("web");
    window(web, &a, &b, Some(&akit), &mut || {
        akit.sighup(&json);
    });
    akit.stop();
    let _ = std::fs::remove_file(&path);
}

#[test]
fn health_window_in_process() {
    let (a, b) = (App::start("a"), App::start("b"));
    let json = config(false, a.addr, b.addr);
    let mut akit = Inproc::start(&json, Duration::from_secs(5));
    let web = akit.listener("web");
    window(web, &a, &b, None, &mut || {
        akit.reload(&json);
    });
    drop(akit);
}
