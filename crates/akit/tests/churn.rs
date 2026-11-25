//! M2's exit criterion (I38): thousands of config swaps under traffic drop no connection and fail
//! no request.
//!
//! The traffic (`m2::traffic::Traffic`): HTTP/1.1 on a connection a request, on a keep-alive
//! connection busy throughout and on one mostly idle; HTTP/2 with prior knowledge, four streams at
//! a time on one connection for the whole run; slow streamed responses through the proxy, each
//! across many swaps; a WebSocket through the proxy and a connection through a `tcp` service,
//! both open for the whole run. Each swap changes the routes' answer, the proxied upstream's
//! addresses and load balancing policy; every 50th adds or removes a listener (from the 10th). The listeners the
//! traffic uses stay as they are, as fit's `binding.same` keeps them; a changed listener is
//! replaced and its connections drained, which `reload.rs` and `akit-listener`'s tests cover.
//!
//! - `churn_through_sighup_and_the_admin_api`: `akit run`, its config swapped through SIGHUP,
//!   the Admin API's `POST /load` and its edits by pointer (`PATCH /config/...`), one after
//!   another as fast as each is applied. `akit run`'s engine is epoll's (kqueue on macOS): akit has
//!   no setting for another backend yet.
//! - `churn_in_process`: akit built in process as `akit run` builds it, its config swapped by
//!   `Listeners::reload`, the path SIGHUP and the Admin API take, on the backend
//!   `AKIO_TEST_BACKEND` names (epoll by default, `uring`): the churn on io_uring.
//!
//! A reload drains what the graph it replaced still serves for the drain timeout
//! (`-shutdown-timeout`), as fit's does: past it, the connections a `tcp` service still relays for
//! the old graph are closed (fit's `TestTCPReload`, ported in `akit-listener`'s `l4.rs`). The
//! churns run with a drain timeout longer than they take, so that every connection must last the
//! whole run.
//!
//! `AKIT_CHURN_SWAPS` sets how many swaps (2000 by default); each takes at least `PACE`, so that
//! the traffic spans them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod m2;
mod support;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use m2::target::{Inproc, Proc};
use m2::traffic::{App, Seen, Traffic, echo, free, get};

/// One churn at a time: their timings are not muddled by each other.
static SERIAL: Mutex<()> = Mutex::new(());

/// The least time a swap takes: the churn lasts long enough for slow responses to span it.
const PACE: Duration = Duration::from_millis(2);

/// The drain timeout: longer than a churn takes.
const DRAIN: Duration = Duration::from_secs(300);

fn swaps() -> usize {
    std::env::var("AKIT_CHURN_SWAPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000)
}

/// Where things are: the listeners the configs name, the Admin API, the upstreams.
struct At {
    web: SocketAddr,
    l4: SocketAddr,
    api: Option<SocketAddr>,
    a: SocketAddr,
    b: SocketAddr,
    echo: SocketAddr,
}

/// How often `app` changes: every `APP_EVERY`th swap. The others leave it as it is, and the new
/// graph takes over its pool, connections and all, as fit's does.
const APP_EVERY: usize = 20;

/// Swap `i`'s config: `/v` answers `v{i}`; `/up` is proxied to `app`, whose addresses and policy
/// change every [`APP_EVERY`] swaps; `/stream` and `/ws` to `slow`; the `tcp` listener `l4` to
/// `echo`; `extra` served too when given. `app` keeps idle connections a second: those of a
/// replaced pool stay open until then (akio API request A10).
fn config(at: &At, i: usize, extra: Option<SocketAddr>) -> String {
    let (a, b) = (at.a, at.b);
    let k = i / APP_EVERY;
    let app = match k % 3 {
        0 => format!(r#""{a}", "{b}""#),
        1 => format!(r#""{a}""#),
        _ => format!(r#""{b}", "{a}""#),
    };
    let policy = ["round_robin", "least_conn", "random", "p2c"][k % 4];
    let admin = at.api.map_or(String::new(), |api| {
        format!(r#""admin": {{"address": "{api}", "history": 10}}, "metrics": {{}},"#)
    });
    let extra = extra.map_or(String::new(), |e| {
        format!(r#", {{"name": "extra", "address": "{e}"}}"#)
    });
    format!(
        r#"{{{admin}
  "listeners": [
    {{"name": "web", "address": "{web}", "protocols": ["http/1.1", "h2c"]}},
    {{"name": "l4", "address": "{l4}", "protocols": ["tcp"]}}{extra}
  ],
  "upstreams": [
    {{"name": "app", "addresses": [{app}], "load_balancing": "{policy}", "idle_timeout": "1s"}},
    {{"name": "slow", "addresses": ["{a}"]}},
    {{"name": "echo", "addresses": ["{echo}"]}}
  ],
  "routes": [
    {{"match": {{"path": ["/v"]}}, "handler": {{"module": "respond", "params": {{"body": "v{i}"}}}}}},
    {{"match": {{"path": ["/up"]}}, "handler": {{"module": "proxy", "params": {{"upstream": "app"}}}}}},
    {{"match": {{"path": ["/stream", "/ws"]}}, "handler": {{"module": "proxy", "params": {{"upstream": "slow"}}}}}}
  ],
  "services": [{{"name": "svc", "listeners": ["l4"], "upstream": "echo"}}]
}}"#,
        web = at.web,
        l4 = at.l4,
        echo = at.echo,
    )
}

/// The `extra` listener of swap `i`: added at every 100th swap from the 10th, at a port of its
/// own, and removed 50 swaps later, so that a churn of whole hundreds ends without it (each
/// listener added at run time holds a socket and a poller on every I/O thread, which the
/// descriptor count would see).
fn extra_at(i: usize, now: Option<SocketAddr>) -> Option<SocketAddr> {
    match i % 100 {
        10 => Some(free()),
        60 => None,
        _ => now,
    }
}

/// What every churn checks once its traffic stopped: no request failed, no connection was
/// dropped, and every kind of traffic kept going; and the upstreams' connections were kept
/// across the swaps that left their upstream as it was. Without that every swap would dial
/// `app` anew (thousands of connections); with it, about one swap in [`APP_EVERY`] does.
fn check(seen: &Seen, n: usize, took: Duration, a: &App, b: &App) {
    let counts = seen.counts();
    let dialed = a.conns.load(std::sync::atomic::Ordering::SeqCst)
        + b.conns.load(std::sync::atomic::Ordering::SeqCst);
    eprintln!(
        "{n} swaps in {took:?}; requests answered: {counts:?}; upstream connections: {dialed}"
    );
    assert!(
        dialed <= n / 2,
        "{dialed} upstream connections over {n} swaps: pools are not kept across swaps"
    );
    let failed = seen.failures();
    assert!(failed.is_empty(), "failed: {failed:#?}");
    for kind in ["short", "keep", "idle", "h2c", "stream", "ws", "tcp"] {
        assert!(
            counts.get(kind).copied().unwrap_or(0) > 0,
            "no {kind} traffic: {counts:?}"
        );
    }
}

/// A GET of `/v` on `extra`, just added by swap `i`.
fn check_extra(extra: SocketAddr, i: usize) {
    let r = get(extra, "/v").unwrap();
    assert_eq!((r.status, r.body), (200, format!("v{i}").into_bytes()));
}

#[test]
fn churn_through_sighup_and_the_admin_api() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (a, b) = (App::start("a"), App::start("b"));
    let at = At {
        web: free(),
        l4: free(),
        api: Some(free()),
        a: a.addr,
        b: b.addr,
        echo: echo(),
    };
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("churn.json");
    let akit = Proc::start(
        &path,
        &config(&at, 0, None),
        at.api,
        "300s",
        "akit: admin API on",
    );
    // A listener added and removed once first: akio makes some of what runtime listeners need
    // (per-thread pipes) the first time, and keeps it. Counted from here, the descriptors must
    // not grow with the swaps.
    akit.sighup(&config(&at, 0, Some(free())));
    akit.sighup(&config(&at, 0, None));
    let (fds, kinds) = (akit.fds(), akit.fd_kinds());
    let traffic = Traffic::start(at.web, Some(at.l4));
    traffic.seen.wait_for("v0");
    traffic.seen.wait_more(50);

    let n = swaps();
    let start = Instant::now();
    let mut extra = None;
    for i in 1..=n {
        let swap = Instant::now();
        let was = extra;
        extra = extra_at(i, extra);
        let json = config(&at, i, extra);
        match i % 10 {
            // An edit by pointer: only `/v`'s answer, the rest as the last swap left it.
            5 if extra == was => akit.patch(
                "/routes/0/handler/params",
                &format!(r#"{{"body": "v{i}"}}"#),
            ),
            k if k % 2 == 0 => akit.sighup(&json),
            _ => akit.load(&json),
        }
        if let Some(e) = extra.filter(|_| was.is_none()) {
            check_extra(e, i);
        }
        std::thread::sleep(PACE.saturating_sub(swap.elapsed()));
    }
    let took = start.elapsed();
    traffic.seen.wait_for(&format!("v{n}"));
    traffic.seen.wait_more(50);
    let seen = traffic.stop();
    check(&seen, n, took, &a, &b);

    // Every swap counted as applied (and the two of the warm-up), none failed.
    let metrics = akit.metrics();
    let line = format!("fit_config_reloads_total{{result=\"success\"}} {}", n + 2);
    assert!(
        metrics.lines().any(|l| l == line),
        "no {line:?} in:\n{metrics}"
    );
    assert!(
        !metrics.lines().any(
            |l| l.starts_with("fit_config_reloads_total{result=\"failure\"}") && !l.ends_with(" 0")
        ),
        "{metrics}"
    );
    // The descriptors the swaps opened (listeners, upstream connections of replaced pools)
    // closed again: the count where it was before the traffic (on Linux from `/proc`, elsewhere
    // from `lsof`), with room for the current pools' idle connections.
    if let (Some(before), Some(_)) = (fds, akit.fds()) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let now = akit.fds().unwrap();
            if now <= before + 16 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{now} descriptors open after the churn, {before} before: {:#?}\nbefore: {kinds:#?}",
                akit.fd_kinds()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    akit.stop();
    let _ = std::fs::remove_file(&path);
}

#[test]
fn churn_in_process() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (a, b) = (App::start("a"), App::start("b"));
    let at = At {
        web: free(),
        l4: free(),
        api: None,
        a: a.addr,
        b: b.addr,
        echo: echo(),
    };
    let mut akit = Inproc::start(&config(&at, 0, None), DRAIN);
    let traffic = Traffic::start(at.web, Some(at.l4));
    traffic.seen.wait_for("v0");
    traffic.seen.wait_more(50);

    let n = swaps();
    let start = Instant::now();
    let mut extra = None;
    for i in 1..=n {
        let swap = Instant::now();
        let was = extra;
        extra = extra_at(i, extra);
        let r = akit.reload(&config(&at, i, extra));
        // The listeners the traffic uses are kept, every swap.
        let added = usize::from(extra.is_some() && was.is_none());
        let removed = usize::from(extra.is_none() && was.is_some());
        assert_eq!(
            (r.added.len(), r.removed.len()),
            (added, removed),
            "swap {i}"
        );
        if let Some(e) = extra.filter(|_| was.is_none()) {
            check_extra(e, i);
        }
        std::thread::sleep(PACE.saturating_sub(swap.elapsed()));
    }
    let took = start.elapsed();
    traffic.seen.wait_for(&format!("v{n}"));
    traffic.seen.wait_more(50);
    let seen = traffic.stop();
    check(&seen, n, took, &a, &b);

    // The graphs the swaps replaced are all retired once their requests are over: nothing of
    // thousands of configs is left behind.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        akit.cell.collect();
        if akit.cell.retiring() == 0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "{} graphs still retiring",
            akit.cell.retiring()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let served: Vec<String> = akit.served().into_iter().map(|s| s.name).collect();
    let want: &[&str] = if extra.is_some() {
        &["web", "l4", "extra"]
    } else {
        &["web", "l4"]
    };
    assert_eq!(served, want);
}
