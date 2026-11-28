//! I79: the config's `limits.max_header_bytes`, `limits.max_conns`, `limits.max_write_queue_bytes`
//! and `engine` on the listeners' akio engine, as fit's listeners take them; I79b:
//! `limits.max_pending_bytes` (akio's budget across connections) and `max_conns` counting the
//! clients alone (with upstream connections open: akit-proxy's `proxy.rs`). On the backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use akio::{Backend, Engine, UringConfig};
use akio_http::client::ClientRequest;
use akio_http::h2client::{H2Client, H2Options};
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};

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

/// Serves `json`; returns the engine and where each listener listens.
fn serve(json: &str) -> (Engine, Vec<SocketAddr>) {
    let config = akit_config::from_json(json, None).unwrap();
    let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
    let cell = GraphCell::new(compile(&config, &registry, &Env::default()).unwrap());
    let e = akit_listener::engine(&cell, &options()).unwrap();
    e.start().unwrap();
    let addrs = e.local_addrs();
    (e, addrs)
}

fn config(limits: &str, engine: &str) -> String {
    format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}}],
            "limits": {{{limits}}}, "engine": {{{engine}}},
            "routes": [{{"handler": {{"module": "respond", "params": {{"body": "ok"}}}}}}]}}"#
    )
}

/// An HTTP/1.1 GET with a field `X-Pad` of `pad` bytes; the status.
fn get_padded(addr: SocketAddr, pad: usize) -> Option<u16> {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let head = format!(
        "GET / HTTP/1.1\r\nHost: a.example\r\nX-Pad: {}\r\nConnection: close\r\n\r\n",
        "p".repeat(pad)
    );
    s.write_all(head.as_bytes()).ok()?;
    let mut resp = String::new();
    s.read_to_string(&mut resp).ok()?;
    resp.split(' ').nth(1)?.parse().ok()
}

/// The same over h2c.
fn get_padded_h2c(addr: SocketAddr, pad: usize) -> Result<u16, String> {
    let client = Engine::builder().client_threads(1).build().unwrap();
    client.start().unwrap();
    let c = H2Client::dial_blocking(&client, addr, None, H2Options::default())
        .map_err(|e| e.to_string())?;
    let r = c
        .send(
            ClientRequest::get(&format!("http://{addr}/"))
                .header("x-pad", &"p".repeat(pad))
                .timeout(Duration::from_secs(5)),
        )
        .wait()
        .map_err(|e| e.to_string())?;
    Ok(r.status)
}

/// fit's `max_header_bytes`: a request head past it is answered 431, under it served, over
/// HTTP/1.1 and h2c.
#[test]
fn header_limit() {
    let (e, addrs) = serve(&config(r#""max_header_bytes": 4096"#, ""));
    let a = addrs[0];
    assert_eq!(get_padded(a, 1000), Some(200));
    assert_eq!(get_padded(a, 8000), Some(431));
    assert_eq!(get_padded_h2c(a, 1000), Ok(200));
    let over = get_padded_h2c(a, 8000);
    assert!(over != Ok(200), "{over:?}");
    e.shutdown();
    e.wait();
}

/// Without `limits`, fit's 1 MiB: a head of 100 KiB is served (akio's own limit is 64 KiB), one
/// past 1 MiB is not.
#[test]
fn header_limit_default() {
    let (e, addrs) = serve(&config("", ""));
    let a = addrs[0];
    assert_eq!(get_padded(a, 100 << 10), Some(200));
    assert_eq!(get_padded_h2c(a, 100 << 10), Ok(200));
    assert_eq!(get_padded(a, (1 << 20) + 1), Some(431));
    e.shutdown();
    e.wait();
}

/// Whether `s` was closed by the server: a read that ends or fails, within a few seconds.
fn closed(s: &mut TcpStream) -> bool {
    // Closed already when the timeout is refused: macOS refuses a socket's options
    // (`EINVAL`) once its peer has reset it.
    if s.set_read_timeout(Some(Duration::from_millis(200)))
        .is_err()
    {
        return true;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut b = [0u8; 64];
    while Instant::now() < deadline {
        match s.read(&mut b) {
            Ok(0) => return true,
            Ok(_) => {}
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return false;
            }
            Err(_) => return true,
        }
    }
    false
}

/// fit's `max_conns`: the connections open across the listeners; one past it is closed as soon
/// as it is accepted, and one may open again once another has gone.
#[test]
fn connection_limit() {
    let (e, addrs) = serve(&config(r#""max_conns": 2"#, ""));
    let a = addrs[0];
    let ask = |s: &mut TcpStream| {
        s.write_all(b"GET / HTTP/1.1\r\nHost: a.example\r\n\r\n")
            .unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut b = [0u8; 256];
        let n = s.read(&mut b).unwrap();
        assert!(b[..n].starts_with(b"HTTP/1.1 200"), "{:?}", &b[..n]);
    };
    let mut one = TcpStream::connect(a).unwrap();
    ask(&mut one);
    let mut two = TcpStream::connect(a).unwrap();
    ask(&mut two);
    // The third is closed as it is accepted.
    let mut three = TcpStream::connect(a).unwrap();
    let _ = three.write_all(b"GET / HTTP/1.1\r\nHost: a.example\r\n\r\n");
    assert!(closed(&mut three), "a connection past max_conns was served");
    assert!(e.telemetry().metrics().total.rejected >= 1);
    // Under it again once one goes.
    drop(one);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        // A try closed as it is accepted, while the server has not seen the other go, gets
        // nothing: its write may fail, and its options too (macOS refuses a socket's options,
        // `EINVAL`, once its peer has reset it).
        let mut s = TcpStream::connect(a).unwrap();
        let mut b = [0u8; 256];
        if s.set_read_timeout(Some(Duration::from_secs(5))).is_ok()
            && s.write_all(b"GET / HTTP/1.1\r\nHost: a.example\r\n\r\n")
                .is_ok()
            && matches!(s.read(&mut b), Ok(n) if b[..n].starts_with(b"HTTP/1.1 200"))
        {
            break;
        }
        assert!(Instant::now() < deadline, "no room after a connection went");
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(two);
    e.shutdown();
    e.wait();
}

/// `engine` and the limits taken at start are akio's settings: the I/O threads of each listener,
/// the read buffer, the watermark, the backlog, the connection and write queue limits.
#[test]
fn engine_settings() {
    let (e, addrs) = serve(&config(
        r#""max_conns": 7, "max_write_queue_bytes": 1048576"#,
        r#""event_loops": 2, "workers": 4, "min_workers": 1, "task_queue": 16,
           "read_buffer_size": 16384, "write_watermark": 65536, "backlog": 64"#,
    ));
    assert_eq!(e.threads()[0], 2);
    let live = e.telemetry().live();
    // max_conns counts the clients alone (I79b, akio's ADR-173): the engine's own limit, which
    // counts the upstream connections too, is off.
    assert_eq!(live.max_accepted_connections(), 7);
    assert_eq!(live.max_connections(), 0);
    assert_eq!(live.max_queued_bytes(), 1 << 20);
    assert_eq!(live.read_buffer(), 16384);
    assert_eq!(live.write_high_watermark(), 65536);
    assert_eq!(live.backlog(), 64);
    // Served as before.
    assert_eq!(get_padded(addrs[0], 10), Some(200));
    e.shutdown();
    e.wait();
    // Left out: fit's limits, akio's engine.
    let (e, _) = serve(&config("", ""));
    let live = e.telemetry().live();
    assert_eq!(live.max_accepted_connections(), 100_000);
    assert_eq!(live.max_queued_bytes(), 32 << 20);
    assert_eq!(live.read_buffer(), 64 << 10);
    // max_pending_bytes: half the memory, or fit's 256 MiB where akit reads none (macOS).
    let half = live.max_pending_bytes().unwrap();
    if cfg!(target_os = "linux") {
        assert!(half > 256 << 20, "{half}");
    } else {
        assert_eq!(half, 256 << 20);
    }
    e.shutdown();
    e.wait();
    // In bytes, or a share.
    let (e, _) = serve(&config(r#""max_pending_bytes": 1048576"#, ""));
    assert_eq!(e.telemetry().live().max_pending_bytes(), Some(1 << 20));
    e.shutdown();
    e.wait();
    let (e, _) = serve(&config(r#""max_pending_bytes": "25%""#, ""));
    let quarter = e.telemetry().live().max_pending_bytes().unwrap();
    if cfg!(target_os = "linux") {
        assert!(quarter.abs_diff(half / 2) <= 1, "{quarter} {half}");
    } else {
        assert_eq!(quarter, 256 << 20);
    }
    e.shutdown();
    e.wait();
}

/// Waits, up to ten seconds, until `done` holds; `what` says what was awaited when it does not.
/// The budget is summed by akio's watcher thread on its own clock and its counters are read
/// from another thread, so the test asks for the state rather than waiting for an event.
fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// fit's `TestPendingBytesBudget`: a client that asks for more than `max_pending_bytes` and
/// reads none of it leaves that much queued, and the engine pauses reads for the budget (I79b,
/// akio's ADR-172): a request read meanwhile on that connection, and a connection opened
/// meanwhile, wait; once the client reads, the budget frees and both are served.
///
/// Which connections the budget pauses is akio's contract, not a timing: a connection is
/// paused (and counted in `budget_pauses`) when it reads, or re-arms its reads, while the
/// budget is over. A connection whose own queue passed `write_high_watermark` has stopped
/// reading already, and is not asked again until its queue drains, so the hog's watermark is
/// raised above its answer: otherwise its second request waits on the watermark, and whether the
/// budget counts it too depended on the backend (io_uring's one ring send of the answer never
/// completes while the client reads nothing, epoll's writability events may still come after the
/// watcher raised the budget).
#[test]
fn pending_bytes_budget() {
    const BUDGET: usize = 1 << 20;
    const BIG: usize = 32 << 20;
    // A directory of this process's own: copies of the test binary run at once.
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("pending-budget-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    // Kept in memory, so it goes out whole, queued on the connection rather than read as the
    // client takes it; and far more than the kernel's socket buffers take.
    std::fs::write(root.join("big.bin"), vec![0u8; BIG]).unwrap();
    std::fs::write(root.join("small.txt"), b"small").unwrap();
    let (e, addrs) = serve(&format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
            "limits": {{"max_pending_bytes": {BUDGET}, "max_write_queue_bytes": 67108864}},
            "engine": {{"write_watermark": 67108864}},
            "routes": [{{"handler": {{"module": "static",
              "params": {{"root": {root:?}, "max_file_bytes": 67108864, "max_cache_bytes": 134217728}}}}}}]}}"#,
        root = root.to_str().unwrap()
    ));
    let a = addrs[0];
    let tel = e.telemetry();
    let mut hog = TcpStream::connect(a).unwrap();
    hog.write_all(b"GET /big.bin HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    // The answer is queued and the client reads none of it: the budget is reached, and stays
    // over (above three quarters of it) while the client does not read.
    wait_until("the budget was not reached", || tel.over_budget());
    assert!(tel.pending_bytes() > BUDGET, "{}", tel.pending_bytes());
    // Read while over: the hog's second request pauses its connection (if a writability event
    // did not already), and a connection opened now is paused before it reads at all. One
    // episode, each connection once.
    hog.write_all(b"GET /small.txt HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut s = TcpStream::connect(a).unwrap();
    // Set before the write: once answered, the connection closes, and macOS refuses a socket's
    // options (`EINVAL`) once its peer has reset it.
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(b"GET /small.txt HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .unwrap();
    wait_until("both connections were not paused by the budget", || {
        tel.metrics().total.budget_pauses >= 2
    });
    assert_eq!(tel.metrics().total.budget_pauses, 2);
    assert!(tel.over_budget());
    // Paused before its first read, the second client has no answer.
    s.set_nonblocking(true).unwrap();
    let mut b = [0u8; 64];
    let r = s.read(&mut b);
    assert!(
        matches!(&r, Err(e) if e.kind() == ErrorKind::WouldBlock),
        "served while over the budget: {r:?}"
    );
    s.set_nonblocking(false).unwrap();
    // The hog reads what it was sent: the budget frees, and both its answers and the other
    // client's request go through.
    let reader = std::thread::spawn(move || {
        hog.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut got = 0usize;
        let mut b = vec![0u8; 1 << 16];
        let mut tail = Vec::new();
        while let Ok(n) = hog.read(&mut b) {
            if n == 0 {
                break;
            }
            got += n;
            tail.extend_from_slice(&b[..n]);
            if tail.len() > 64 {
                tail.drain(..tail.len() - 64);
            }
            if tail.ends_with(b"small") {
                break;
            }
        }
        got
    });
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    assert!(
        resp.starts_with("HTTP/1.1 200") && resp.ends_with("small"),
        "{resp}"
    );
    assert!(reader.join().unwrap() > BIG);
    assert!(!tel.over_budget());
    // The scrape's akit_reads_paused_total{reason="budget"} is akio's count.
    assert_eq!(tel.metrics().total.budget_pauses, 2);
    e.shutdown();
    e.wait();
    let _ = std::fs::remove_dir_all(&root);
}
