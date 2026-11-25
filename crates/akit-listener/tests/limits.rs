//! I79: the config's `limits.max_header_bytes`, `limits.max_conns`, `limits.max_write_queue_bytes`
//! and `engine` on the listeners' akio engine, as fit's listeners take them. On the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
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
    s.set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
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
        let mut s = TcpStream::connect(a).unwrap();
        s.write_all(b"GET / HTTP/1.1\r\nHost: a.example\r\n\r\n")
            .unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut b = [0u8; 256];
        if matches!(s.read(&mut b), Ok(n) if b[..n].starts_with(b"HTTP/1.1 200")) {
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
    assert_eq!(live.max_connections(), 7);
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
    assert_eq!(live.max_connections(), 100_000);
    assert_eq!(live.max_queued_bytes(), 32 << 20);
    assert_eq!(live.read_buffer(), 64 << 10);
    e.shutdown();
    e.wait();
}
