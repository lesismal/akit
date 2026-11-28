//! The `forward_proxy` handler (I55) through akit's listener on akio, with fit's
//! `handler/forward/forward_test.go` ported: CONNECT tunnels to what the rules allow, by address
//! and by a name whose address decides, the rest refused (403 before the dial, 502 for a name
//! refused once connected), 400 for a target that is not `host:port`, 405 for other methods, the
//! idle timeout; and beyond fit's: large transfers with slow peers, a tunnel cut midway, the
//! connect timeout (504) and fit's `TestConnectH2C`, a tunnel on an h2c stream (akio API request
//! A134). On the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, UringConfig};
use akio_http::client::ClientRequest;
use akio_http::h2client::{H2Client, H2Event, H2Options};
use akit_runtime::upstream::EngineSlot;
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
    o.shutdown_timeout = Duration::from_secs(2);
    o
}

/// A listener of HTTP/1.1 and h2c whose CONNECT requests go to a forward_proxy with `params`,
/// and whose others to a respond (fit's `serve`); stops when dropped.
struct Server {
    engine: Arc<Engine>,
    addr: SocketAddr,
}

impl Drop for Server {
    fn drop(&mut self) {
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
    }
}

fn serve(params: &str) -> Server {
    let json = format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}}],
  "routes": [{{"match": {{"method": ["CONNECT"]}}, "handler": {{"module": "forward_proxy", "params": {params}}}}},
             {{"match": {{"path": ["/proxy"]}}, "handler": {{"module": "forward_proxy", "params": {params}}}}}]}}"#
    );
    let config = akit_config::from_json(&json, None).unwrap_or_else(|e| panic!("{json}: {e}"));
    let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
    let slot = EngineSlot::new();
    let cell = GraphCell::new(compile(&config, &registry, &Env::new(slot.clone())).unwrap());
    let engine = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
    engine.start().unwrap();
    assert!(slot.set(engine.clone()));
    let addr = engine.local_addrs()[0];
    Server { engine, addr }
}

/// A TCP server serving each connection with `serve` on a thread of its own.
fn backend(serve: impl Fn(TcpStream) + Send + Sync + 'static) -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let serve = Arc::new(serve);
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            let serve = Arc::clone(&serve);
            thread::spawn(move || serve(s));
        }
    });
    addr
}

fn echo(mut s: TcpStream) {
    let mut r = s.try_clone().unwrap();
    let _ = std::io::copy(&mut r, &mut s);
}

/// Sends a CONNECT for `target` over HTTP/1.1; the connection, past the response's head, and
/// its status (fit's `connect`).
fn connect(proxy: SocketAddr, target: &str) -> (TcpStream, u16) {
    let mut c = TcpStream::connect_timeout(&proxy, Duration::from_secs(5)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    c.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes())
        .unwrap();
    let head = read_head(&mut c);
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("CONNECT {target}: {head:?}"));
    (c, status)
}

/// The response's head, read a byte at a time so that nothing behind it is taken.
fn read_head(c: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if c.read(&mut b).unwrap() == 0 {
            break;
        }
        head.push(b[0]);
    }
    String::from_utf8(head).unwrap()
}

fn round_trip(c: &mut TcpStream, msg: &[u8]) {
    c.write_all(msg).unwrap();
    let mut got = vec![0u8; msg.len()];
    c.read_exact(&mut got).unwrap();
    assert_eq!(got, msg);
}

/// Reads until the peer closes, within `d`; what came before.
fn expect_closed(c: &mut TcpStream, d: Duration) -> Vec<u8> {
    // Closed already when the timeout is refused: macOS refuses a socket's options
    // (`EINVAL`) once its peer has reset it.
    if c.set_read_timeout(Some(d)).is_err() {
        return Vec::new();
    }
    let mut all = Vec::new();
    let mut b = [0u8; 4096];
    loop {
        match c.read(&mut b) {
            Ok(0) => return all,
            Ok(n) => all.extend_from_slice(&b[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                panic!("still open after {d:?}")
            }
            Err(_) => return all,
        }
    }
}

/// CONNECTs tunneled to what the rules allow, by address and by a host name whose address
/// decides, and the rest refused (fit's `TestConnect`).
#[test]
fn tunnels_and_refusals() {
    let target = backend(echo);
    let port = target.port();
    let proxy = serve(&format!(
        r#"{{"clients": ["127.0.0.0/8"], "allow": ["127.0.0.0/8"], "ports": [{port}]}}"#
    ));
    for dest in [target.to_string(), format!("localhost:{port}")] {
        let (mut c, status) = connect(proxy.addr, &dest);
        assert_eq!(status, 200, "{dest}");
        round_trip(&mut c, format!("through {dest}").as_bytes());
    }
    for (dest, want) in [
        ("127.0.0.1:1".to_owned(), 403),
        (format!("192.0.2.1:{port}"), 403),
        ("nohost".to_owned(), 400),
    ] {
        assert_eq!(connect(proxy.addr, &dest).1, want, "{dest}");
    }
    drop(proxy);

    // A host name whose address deny holds is refused once connected.
    let proxy = serve(r#"{"clients": ["127.0.0.1"], "deny": ["127.0.0.0/8", "::1"]}"#);
    assert_eq!(connect(proxy.addr, &target.to_string()).1, 403);
    assert_eq!(connect(proxy.addr, &format!("localhost:{port}")).1, 502);
    drop(proxy);

    // With allow and deny left out, loopback and link-local are refused, by address and by a
    // name that resolves there: the Admin API, on 127.0.0.1:2019 by default, and the cloud's
    // metadata service among them.
    let proxy = serve(r#"{"clients": ["127.0.0.1"]}"#);
    for (dest, want) in [
        (target.to_string(), 403),
        (format!("[::ffff:127.0.0.1]:{port}"), 403),
        ("169.254.169.254:80".to_owned(), 403),
        (format!("0.0.0.0:{port}"), 403),
        (format!("localhost:{port}"), 502),
    ] {
        assert_eq!(connect(proxy.addr, &dest).1, want, "{dest} with no rules");
    }
    drop(proxy);
    let proxy = serve(r#"{"clients": ["10.0.0.0/8"]}"#);
    assert_eq!(
        connect(proxy.addr, &target.to_string()).1,
        403,
        "a client not allowed"
    );
}

/// Another method on the handler's route is answered 405, naming CONNECT.
#[test]
fn not_connect() {
    let proxy = serve(r#"{"clients": ["127.0.0.1"]}"#);
    let mut c = TcpStream::connect(proxy.addr).unwrap();
    c.write_all(b"GET /proxy HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut resp = String::new();
    c.read_to_string(&mut resp).unwrap();
    assert!(resp.starts_with("HTTP/1.1 405"), "{resp}");
    assert!(
        resp.to_ascii_lowercase().contains("allow: connect"),
        "{resp}"
    );
}

/// fit's `TestConnectH2C`: a CONNECT on an h2c stream is tunneled on it (akio API request
/// A134); and a destination the rules refuse is answered 403 there too.
#[test]
fn connect_over_h2c() {
    let target = backend(echo);
    let proxy = serve(r#"{"clients": ["127.0.0.1"], "allow": ["127.0.0.0/8"]}"#);
    let client = Engine::builder().client_threads(1).build().unwrap();
    client.start().unwrap();
    let c = H2Client::dial_blocking(&client, proxy.addr, None, H2Options::default()).unwrap();
    let open = |dest: String| {
        let mut req = ClientRequest::get(&format!("http://{}/", proxy.addr));
        req.method = akio_http::Method::Connect;
        req.headers.set("host", dest);
        let (tx, rx) = mpsc::channel::<String>();
        let up = c.open(req, move |ev| {
            let _ = tx.send(match ev {
                H2Event::Head { status, .. } => format!("head {status}"),
                H2Event::Data(d) => format!("data {}", String::from_utf8_lossy(d)),
                H2Event::Trailers(_) => "trailers".into(),
                H2Event::End => "end".into(),
                H2Event::Error(e) => format!("error {e}"),
            });
        });
        (up, rx)
    };
    let wait = Duration::from_secs(10);
    let (up, rx) = open(target.to_string());
    assert_eq!(rx.recv_timeout(wait).unwrap(), "head 200");
    let msg = "h2c ".repeat(1000);
    assert!(up.write(msg.as_bytes()));
    let mut got = String::new();
    while got.len() < msg.len() {
        let ev = rx.recv_timeout(wait).unwrap();
        got.push_str(ev.strip_prefix("data ").unwrap_or_else(|| panic!("{ev}")));
    }
    assert_eq!(got, msg);
    assert!(up.finish());
    loop {
        match rx.recv_timeout(wait).unwrap().as_str() {
            "end" => break,
            "data " => {}
            ev => panic!("{ev}"),
        }
    }
    let (_up, rx) = open("192.0.2.1:80".into());
    assert_eq!(rx.recv_timeout(wait).unwrap(), "head 403");
    c.close();
    drop(client);
}

/// A tunnel that carries nothing for the idle timeout is closed (fit's `TestConnectIdleTimeout`).
#[test]
fn idle_timeout() {
    let target = backend(echo);
    let proxy =
        serve(r#"{"clients": ["127.0.0.1"], "allow": ["127.0.0.0/8"], "idle_timeout": "300ms"}"#);
    let (mut c, status) = connect(proxy.addr, &target.to_string());
    assert_eq!(status, 200);
    for _ in 0..4 {
        thread::sleep(Duration::from_millis(100));
        round_trip(&mut c, b"x");
    }
    expect_closed(&mut c, Duration::from_secs(3));
}

/// A destination not connected within the connect timeout is answered 504 (or 502 where the
/// network refuses 10.255.255.1 at once), well before the handshake's own timeout.
#[test]
fn connect_timeout() {
    let proxy =
        serve(r#"{"clients": ["127.0.0.1"], "allow": ["10.0.0.0/8"], "connect_timeout": "300ms"}"#);
    let start = Instant::now();
    let (_c, status) = connect(proxy.addr, "10.255.255.1:9");
    assert!(status == 504 || status == 502, "{status}");
    if status == 504 {
        assert!(
            start.elapsed() >= Duration::from_millis(250),
            "{:?}",
            start.elapsed()
        );
    }
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
}

fn pattern(i: usize) -> u8 {
    u8::try_from((i * 13 + i / 509) % 256).unwrap()
}

/// 8 MiB each way while the destination, then the client, read slowly: every byte arrives, in
/// order; and a destination that closes midway closes the tunnel.
#[test]
fn slow_peers_and_cut() {
    const N: usize = 8 << 20;
    let slow = backend(|mut s| {
        let mut got = 0;
        let mut b = [0u8; 16 << 10];
        while got < N {
            let n = s.read(&mut b).unwrap();
            assert!(n > 0, "cut at {got}");
            assert!(
                b[..n]
                    .iter()
                    .enumerate()
                    .all(|(i, x)| *x == pattern(got + i))
            );
            got += n;
            if got % (1 << 20) < n {
                thread::sleep(Duration::from_millis(50));
            }
        }
        let out: Vec<u8> = (0..N).map(pattern).collect();
        s.write_all(&out).unwrap();
    });
    let proxy = serve(r#"{"clients": ["127.0.0.1"], "allow": ["127.0.0.0/8"]}"#);
    let (mut c, status) = connect(proxy.addr, &slow.to_string());
    assert_eq!(status, 200);
    let data: Vec<u8> = (0..N).map(pattern).collect();
    c.write_all(&data).unwrap();
    let mut got = 0;
    let mut b = vec![0u8; 32 << 10];
    while got < N {
        let n = c.read(&mut b).unwrap();
        assert!(n > 0, "cut at {got}");
        assert!(
            b[..n]
                .iter()
                .enumerate()
                .all(|(i, x)| *x == pattern(got + i)),
            "at {got}"
        );
        got += n;
        if got % (1 << 20) < n {
            thread::sleep(Duration::from_millis(50));
        }
    }

    let closer = backend(|mut s| {
        s.write_all(b"bye").unwrap();
        let _ = s.shutdown(Shutdown::Both);
    });
    let (mut c, status) = connect(proxy.addr, &closer.to_string());
    assert_eq!(status, 200);
    assert_eq!(expect_closed(&mut c, Duration::from_secs(5)), b"bye");
}
