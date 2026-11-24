//! WebSocket proxying (I49): fit's `handler/proxy/websocket_test.go`, `websocket_ext_test.go`,
//! `websocket_unix_test.go` and `upgrade_test.go`, ported. HTTP/1.1 clients upgrade; HTTP/2
//! (h2c and TLS) and HTTP/3 clients open an extended CONNECT (RFC 8441, RFC 9220); each goes to
//! an HTTP/1.1 upstream as its handshake, and once the upstream switched the tunnel carries the
//! bytes both ways, each side held back by the other: a client that reads nothing holds the
//! upstream back far short of what it sends. Refusals pass on as responses do. On the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation
)]

#[path = "../../akit-listener/tests/support/mod.rs"]
mod support;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, Listener, UringConfig};
use akio_http::h2client::{H2Client, H2Options, H2WsOptions};
use akio_http::hpack::{Decoder, Encoder};
use akio_http::ws::{Message, WsHandler, WsSender};
use akio_http::{Context, Router, WsOptions};
use akio_quic::{Bytes, Connection, ConnectionError, QuicHandler, StreamId};
use akit_listener::Addrs;
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};
use support::certs::{Ca, dir};

const WAIT: Duration = Duration::from_secs(20);

/// The tests one at a time: each starts engines of several threads, and together they would
/// pass a host's 1024 open files (gpu51's limit).
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

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

/// akit serving a config, as `akit run` does.
struct Akit {
    engine: Arc<Engine>,
    addrs: Vec<Addrs>,
    _cell: GraphCell,
}

impl Akit {
    fn start(json: &str) -> Akit {
        let config = akit_config::from_json(json, None).unwrap();
        let registry: Registry<HandlerParams> =
            Registry::new(&[akit_handler::MODULES, akit_proxy::MODULES]).unwrap();
        let slot = EngineSlot::new();
        let cell = GraphCell::new(compile(&config, &registry, &Env::new(slot.clone())).unwrap());
        let engine = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
        engine.start().unwrap();
        assert!(slot.set(engine.clone()));
        let addrs = akit_listener::addrs(&config.listeners, &engine);
        Akit {
            engine,
            addrs,
            _cell: cell,
        }
    }

    /// One HTTP/1.1 and h2c listener proxying to `up`, the upstream given `upstream` and the
    /// proxy `params` more fields.
    fn plain(up: &str, upstream: &str, params: &str) -> Akit {
        Akit::start(&format!(
            r#"{{
  "listeners": [{{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}}],
  "upstreams": [{{"name": "app", "addresses": ["{up}"]{upstream}}}],
  "routes": [{{"handler": {{"module": "proxy", "params": {{"upstream": "app"{params}}}}}}}]
}}"#
        ))
    }

    fn tcp(&self) -> SocketAddr {
        self.addrs[0].tcp.unwrap()
    }
}

impl Drop for Akit {
    fn drop(&mut self) {
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
    }
}

/// A head's field `name`, in any case.
fn field<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines()
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim())
}

fn status(head: &str) -> u16 {
    head.split(' ').nth(1).unwrap().parse().unwrap()
}

fn read_head(r: &mut impl BufRead) -> Option<String> {
    let mut head = String::new();
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        head.push_str(&line);
        if line == "\r\n" {
            return Some(head);
        }
    }
}

// ---- fit's `wsBackend` ------------------------------------------------------------------------

/// What fit's flood sends.
const FLOOD: u64 = 64 << 20;

/// A WebSocket server that, once switched, echoes the bytes it gets, or on `flood` sends
/// [`FLOOD`] bytes as fast as the connection takes them. A request that does not ask for a
/// WebSocket, or asks for `/refuse`, is answered 403.
struct WsBackend {
    addr: SocketAddr,
    written: Arc<AtomicU64>,
    closed: mpsc::Receiver<()>,
    heads: mpsc::Receiver<String>,
}

fn ws_backend() -> WsBackend {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let written = Arc::new(AtomicU64::new(0));
    let (ctx, closed) = mpsc::channel();
    let (htx, heads) = mpsc::channel();
    let w = written.clone();
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            let (w, ctx, htx) = (w.clone(), ctx.clone(), htx.clone());
            thread::spawn(move || {
                ws_serve(s, &w, &htx);
                let _ = ctx.send(());
            });
        }
    });
    WsBackend {
        addr,
        written,
        closed,
        heads,
    }
}

fn ws_serve(s: TcpStream, written: &AtomicU64, heads: &mpsc::Sender<String>) {
    let mut w = s.try_clone().unwrap();
    let mut r = BufReader::new(s);
    let Some(head) = read_head(&mut r) else {
        return;
    };
    let _ = heads.send(head.clone());
    let path = head.split(' ').nth(1).unwrap_or("");
    if path == "/refuse"
        || !field(&head, "upgrade").is_some_and(|u| u.eq_ignore_ascii_case("websocket"))
    {
        let _ = w.write_all(
            b"HTTP/1.1 403 Forbidden\r\nContent-Length: 7\r\nX-Why: test\r\n\r\nrefused",
        );
        return;
    }
    let key = field(&head, "sec-websocket-key").unwrap_or("");
    let mut resp = format!(
        "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {}\r\n",
        akio_http::ws::accept_key(key)
    );
    if field(&head, "sec-websocket-protocol").is_some() {
        resp.push_str("Sec-WebSocket-Protocol: chat\r\n");
    }
    resp.push_str("\r\n");
    if w.write_all(resp.as_bytes()).is_err() {
        return;
    }
    let mut buf = vec![0; 64 << 10];
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        if &buf[..n] == b"flood" {
            let chunk = vec![b'f'; 64 << 10];
            while written.load(Ordering::Relaxed) < FLOOD {
                if w.write_all(&chunk).is_err() {
                    return;
                }
                written.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            }
            continue;
        }
        if w.write_all(&buf[..n]).is_err() {
            return;
        }
    }
}

/// An address nothing listens on.
fn refused() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap()
}

/// Opens a WebSocket through akit at `addr` to `path` over HTTP/1.1: the connection, its reader
/// past the answer's head, and the head.
fn ws_dial(
    addr: SocketAddr,
    path: &str,
    protocols: &str,
) -> (TcpStream, BufReader<TcpStream>, String) {
    let c = TcpStream::connect(addr).unwrap();
    c.set_read_timeout(Some(WAIT)).unwrap();
    let key = "MDEyMzQ1Njc4OWFiY2RlZg==";
    let mut req = format!(
        "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: keep-alive, Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\n"
    );
    if !protocols.is_empty() {
        req.push_str(&format!("Sec-WebSocket-Protocol: {protocols}\r\n"));
    }
    req.push_str("\r\n");
    (&c).write_all(req.as_bytes()).unwrap();
    let mut r = BufReader::new(c.try_clone().unwrap());
    let head = read_head(&mut r).expect("an answer");
    if status(&head) == 101 {
        assert_eq!(
            field(&head, "sec-websocket-accept"),
            Some(akio_http::ws::accept_key(key).as_str())
        );
    }
    (c, r, head)
}

fn closed_soon(rx: &mpsc::Receiver<()>, what: &str) {
    rx.recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|_| panic!("{what}"));
}

#[test]
fn websocket_echo() {
    let _serial = serial();
    let b = ws_backend();
    let akit = Akit::plain(&b.addr.to_string(), "", "");
    let (mut c, mut r, head) = ws_dial(akit.tcp(), "/chat", "chat, superchat");
    assert_eq!(status(&head), 101, "{head}");
    assert_eq!(field(&head, "sec-websocket-protocol"), Some("chat"));
    assert!(
        field(&head, "upgrade")
            .unwrap()
            .eq_ignore_ascii_case("websocket")
    );
    assert!(
        field(&head, "connection")
            .unwrap()
            .eq_ignore_ascii_case("upgrade")
    );
    let up = b.heads.recv_timeout(WAIT).unwrap();
    assert_eq!(field(&up, "sec-websocket-version"), Some("13"), "{up}");
    assert!(field(&up, "x-forwarded-for").is_some(), "{up}");
    assert!(
        field(&up, "connection")
            .unwrap()
            .eq_ignore_ascii_case("upgrade"),
        "{up}"
    );
    for msg in ["hello", "world"] {
        c.write_all(msg.as_bytes()).unwrap();
        let mut got = vec![0; msg.len()];
        r.read_exact(&mut got).unwrap();
        assert_eq!(got, msg.as_bytes());
    }

    // 16 MiB each way at once.
    const SIZE: usize = 16 << 20;
    let payload: Vec<u8> = (0..SIZE).map(|i| (i % 251) as u8).collect();
    let mut w = c.try_clone().unwrap();
    let sent = payload.clone();
    let writer = thread::spawn(move || w.write_all(&sent).unwrap());
    let mut got = vec![0; SIZE];
    r.read_exact(&mut got).unwrap();
    assert!(got == payload, "the large echo differs");
    writer.join().unwrap();

    // The client goes: the upstream's connection ends too.
    drop(r);
    drop(c);
    closed_soon(&b.closed, "the upstream connection outlived the client's");
}

#[test]
fn websocket_refused() {
    let _serial = serial();
    let b = ws_backend();
    let akit = Akit::plain(&b.addr.to_string(), "", "");
    let (_c, mut r, head) = ws_dial(akit.tcp(), "/refuse", "");
    assert_eq!(status(&head), 403, "{head}");
    assert_eq!(field(&head, "x-why"), Some("test"));
    let mut body = String::new();
    r.read_to_string(&mut body).unwrap();
    assert_eq!(body, "refused");

    // No upstream at all.
    let akit = Akit::plain(&refused().to_string(), "", "");
    let (_c, _r, head) = ws_dial(akit.tcp(), "/", "");
    assert_eq!(status(&head), 502, "{head}");
}

#[test]
fn websocket_idle_timeout() {
    let _serial = serial();
    let b = ws_backend();
    let akit = Akit::plain(
        &b.addr.to_string(),
        "",
        r#", "tunnel_idle_timeout": "300ms""#,
    );
    let (mut c, mut r, head) = ws_dial(akit.tcp(), "/", "");
    assert_eq!(status(&head), 101);
    // Traffic keeps it open past the timeout.
    for _ in 0..4 {
        thread::sleep(Duration::from_millis(150));
        c.write_all(b"x").unwrap();
        let mut one = [0; 1];
        r.read_exact(&mut one).expect("closed while in use");
    }
    let start = Instant::now();
    let mut one = [0; 1];
    let n = r.read(&mut one).unwrap_or(0);
    assert_eq!(n, 0, "idle, yet open");
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
    closed_soon(
        &b.closed,
        "the upstream connection outlived the idle timeout",
    );
}

/// A WebSocket's upstream connection leaves the pool once it switched: open WebSockets do not
/// fill `max_conns` and `max_pending` for the requests after them.
#[test]
fn websockets_leave_the_pool() {
    let _serial = serial();
    let b = ws_backend();
    let akit = Akit::plain(
        &b.addr.to_string(),
        r#", "max_conns": 1, "max_pending": 1"#,
        "",
    );
    let mut open = Vec::new();
    for _ in 0..3 {
        let (c, r, head) = ws_dial(akit.tcp(), "/chat", "");
        assert_eq!(status(&head), 101, "a WebSocket: {head}");
        open.push((c, r));
    }
    // The backend refuses what is not a WebSocket with 403: the request reached it.
    let mut c = TcpStream::connect(akit.tcp()).unwrap();
    c.set_read_timeout(Some(WAIT)).unwrap();
    c.write_all(b"GET /plain HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .unwrap();
    let head = read_head(&mut BufReader::new(c)).unwrap();
    assert_eq!(
        status(&head),
        403,
        "a request with three WebSockets open: {head}"
    );
}

/// The upstream sends far more than a client that does not read can take: it is held back, not
/// buffered for, and the client gets every byte once it reads.
#[test]
fn websocket_backpressure() {
    let _serial = serial();
    let b = ws_backend();
    let akit = Akit::plain(&b.addr.to_string(), "", "");
    let (mut c, mut r, head) = ws_dial(akit.tcp(), "/flood", "");
    assert_eq!(status(&head), 101);
    c.write_all(b"flood").unwrap();
    thread::sleep(Duration::from_secs(1));
    let held = b.written.load(Ordering::Relaxed);
    assert!(
        held < FLOOD / 2,
        "the upstream sent {held} bytes to a client that read none"
    );
    let mut left = FLOOD;
    let mut buf = vec![0; 256 << 10];
    while left > 0 {
        let n = r.read(&mut buf).unwrap();
        assert!(n > 0, "{left} bytes short");
        left -= n as u64;
    }
}

// ---- fit's `upgrade_test.go` ------------------------------------------------------------------

/// An upstream that serves each connection's request that asks to switch with `serve`, and
/// answers one that does not with 200 `ok`.
fn upgrade_backend(serve: impl Fn(TcpStream) + Send + Sync + 'static) -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let serve = Arc::new(serve);
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            let serve = serve.clone();
            thread::spawn(move || {
                let mut w = s.try_clone().unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                while let Some(head) = read_head(&mut r) {
                    if field(&head, "upgrade").is_none() {
                        if w.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                            .is_err()
                        {
                            return;
                        }
                        continue;
                    }
                    serve(s);
                    return;
                }
            });
        }
    });
    addr
}

/// A config whose one backend leaves rotation at its first failure, so that a test sees any
/// failure an answer counts.
fn upgrade_akit(up: SocketAddr) -> Akit {
    Akit::plain(
        &up.to_string(),
        r#", "max_fails": 1, "fail_timeout": "1m""#,
        "",
    )
}

/// A plain request answered by the backend: it is still in rotation.
fn still_in_rotation(akit: &Akit) {
    let mut c = TcpStream::connect(akit.tcp()).unwrap();
    c.set_read_timeout(Some(WAIT)).unwrap();
    c.write_all(b"GET /plain HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut out = String::new();
    c.read_to_string(&mut out).unwrap();
    assert!(
        out.starts_with("HTTP/1.1 200") && out.ends_with("ok"),
        "{out}"
    );
}

/// The body after `head`, framed as it says (by its length, or by the connection's close).
fn read_rest(r: &mut BufReader<TcpStream>, head: &str, mut each: impl FnMut(usize)) -> (u64, bool) {
    let want = field(head, "content-length").map(|v| v.parse::<u64>().unwrap());
    let mut n = 0u64;
    let mut buf = vec![0; 64 << 10];
    loop {
        match r.read(&mut buf) {
            Ok(0) | Err(_) => return (n, want.is_none_or(|w| w == n)),
            Ok(k) => {
                n += k as u64;
                each(k);
                if want == Some(n) {
                    return (n, true);
                }
            }
        }
    }
}

/// An upstream that answers an upgrade with anything but 101 and a large body has it streamed
/// back as any response is: while the client does not read, the backend's writes stall far
/// short of the body; once it reads, it gets all of it, and the backend stays in rotation.
#[test]
fn upgrade_large_refusal() {
    let _serial = serial();
    const PIECE: usize = 64 << 10;
    const PIECES: usize = 512; // 32 MiB
    for framing in ["length", "chunked", "close"] {
        let written = Arc::new(AtomicU64::new(0));
        let w2 = written.clone();
        let up = upgrade_backend(move |mut c| {
            let mut head = String::from("HTTP/1.1 200 OK\r\n");
            match framing {
                "length" => head.push_str(&format!("Content-Length: {}\r\n", PIECE * PIECES)),
                "chunked" => head.push_str("Transfer-Encoding: chunked\r\n"),
                _ => head.push_str("Connection: close\r\n"),
            }
            head.push_str("\r\n");
            if c.write_all(head.as_bytes()).is_err() {
                return;
            }
            let mut data = vec![b'x'; PIECE];
            if framing == "chunked" {
                data = [
                    format!("{PIECE:x}\r\n").into_bytes(),
                    data,
                    b"\r\n".to_vec(),
                ]
                .concat();
            }
            for _ in 0..PIECES {
                if c.write_all(&data).is_err() {
                    return;
                }
                w2.fetch_add(PIECE as u64, Ordering::Relaxed);
            }
            if framing == "chunked" {
                let _ = c.write_all(b"0\r\n\r\n");
            }
            let _ = c.shutdown(Shutdown::Write);
        });
        let akit = upgrade_akit(up);
        let (_c, mut r, head) = ws_dial(akit.tcp(), "/big", "");
        assert_eq!(status(&head), 200, "{framing}: {head}");
        // The client reads nothing: the backend's writes stall.
        let deadline = Instant::now() + Duration::from_secs(10);
        let stalled = loop {
            let before = written.load(Ordering::Relaxed);
            thread::sleep(Duration::from_millis(200));
            let after = written.load(Ordering::Relaxed);
            if after == before {
                break after;
            }
            assert!(
                Instant::now() < deadline,
                "{framing}: the backend's writes did not stall"
            );
        };
        assert!(
            stalled < (PIECE * PIECES / 2) as u64,
            "{framing}: the backend wrote {stalled} bytes to a client that read none"
        );
        let (n, whole) = read_rest(&mut r, &head, |_| {});
        assert!(whole && n == (PIECE * PIECES) as u64, "{framing}: read {n}");
        still_in_rotation(&akit);
    }
}

/// A refusal within the threshold reaches a client that reads it slowly whole, its fields as
/// the upstream sent them.
#[test]
fn upgrade_small_refusal_slow_client() {
    let _serial = serial();
    let body: Vec<u8> = b"0123456789abcdef".repeat((60 << 10) / 16);
    let b2 = body.clone();
    let up = upgrade_backend(move |mut c| {
        let head = format!(
            "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nX-Why: test\r\n\r\n",
            b2.len()
        );
        let _ = c.write_all(head.as_bytes());
        let _ = c.write_all(&b2);
    });
    let akit = upgrade_akit(up);
    let (_c, mut r, head) = ws_dial(akit.tcp(), "/missing", "");
    assert_eq!(status(&head), 404, "{head}");
    assert_eq!(field(&head, "x-why"), Some("test"));
    let mut got = Vec::new();
    let mut buf = [0; 4 << 10];
    while got.len() < body.len() {
        thread::sleep(Duration::from_millis(5));
        let n = r.read(&mut buf).unwrap();
        assert!(n > 0, "short at {}", got.len());
        got.extend_from_slice(&buf[..n]);
    }
    assert!(got == body);
    still_in_rotation(&akit);
}

/// A refusal the upstream cuts short: 502 when small enough to arrive whole; cut short in turn
/// when it was streaming; a client that goes while a refusal trickles in has the upstream's
/// connection closed.
#[test]
fn upgrade_refusal_cut() {
    let _serial = serial();
    // Streamed: the client's response is cut short too.
    let up = upgrade_backend(|mut c| {
        let _ = c.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 1048576\r\n\r\n");
        let _ = c.write_all(&vec![b'x'; 128 << 10]);
    });
    let akit = upgrade_akit(up);
    let (_c, mut r, head) = ws_dial(akit.tcp(), "/", "");
    assert_eq!(status(&head), 404, "{head}");
    let (n, whole) = read_rest(&mut r, &head, |_| {});
    assert!(!whole && n == 128 << 10, "read {n}, whole {whole}");

    // Small: 502.
    let up = upgrade_backend(|mut c| {
        let _ = c.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 32768\r\n\r\n");
        let _ = c.write_all(&[b'x'; 1024]);
    });
    let akit = upgrade_akit(up);
    let (_c, _r, head) = ws_dial(akit.tcp(), "/", "");
    assert_eq!(status(&head), 502, "{head}");

    // The client goes while it trickles in.
    let (tx, closed) = mpsc::channel();
    let tx = Mutex::new(tx);
    let up = upgrade_backend(move |mut c| {
        if c.write_all(b"HTTP/1.1 404 Not Found\r\nTransfer-Encoding: chunked\r\n\r\n")
            .is_ok()
        {
            for _ in 0..200 {
                thread::sleep(Duration::from_millis(50));
                if c.write_all(b"1\r\nx\r\n").is_err() {
                    break;
                }
            }
        }
        let _ = tx.lock().unwrap().send(());
    });
    let akit = upgrade_akit(up);
    let mut c = TcpStream::connect(akit.tcp()).unwrap();
    c.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n")
        .unwrap();
    thread::sleep(Duration::from_millis(200));
    drop(c);
    closed_soon(&closed, "the upstream's connection outlived the client");
}

// ---- HTTP/2 and HTTP/3 entry points (fit's `websocket_ext_test.go`) ---------------------------

/// An upstream WebSocket server (akio's, message by message): `/chat` echoes, its handshake's
/// head sent on `heads`.
fn akio_ws_backend() -> (Engine, SocketAddr, mpsc::Receiver<String>) {
    struct Echo;
    impl WsHandler for Echo {
        fn on_message(&mut self, ws: &WsSender, msg: Message) {
            let _ = match msg {
                Message::Text(t) => ws.send_text(&t),
                Message::Binary(b) => ws.send_binary(&b),
            };
        }
    }
    let (tx, heads) = mpsc::channel();
    let tx = Mutex::new(tx);
    let app = Router::new().get("/chat", move |ctx: &mut Context| {
        let mut head = String::new();
        for (n, v) in ctx.req().headers.iter() {
            head.push_str(&format!("{n}: {v}\n"));
        }
        let _ = tx.lock().unwrap().send(head);
        let mut o = WsOptions::default();
        o.protocols = vec!["chat".into()];
        ctx.upgrade_websocket(Echo, &o);
    });
    let e = Engine::builder()
        .listen(
            Listener::tcp("127.0.0.1:0".parse().unwrap())
                .threads(1)
                .serve(akio_http::serve(app)),
        )
        .build()
        .unwrap();
    e.start().unwrap();
    let addr = e.local_addrs()[0];
    (e, addr, heads)
}

/// Records what a WebSocket client gets.
struct Rec(mpsc::Sender<Message>);

impl WsHandler for Rec {
    fn on_message(&mut self, _ws: &WsSender, msg: Message) {
        let _ = self.0.send(msg);
    }
}

/// Echoes of a text, and of a binary message of 1 MiB, which the windows pace.
fn echoes(ws: &WsSender, rx: &mpsc::Receiver<Message>) {
    ws.send_text("hello").unwrap();
    assert_eq!(
        rx.recv_timeout(WAIT).unwrap(),
        Message::Text("hello".into())
    );
    let big: Vec<u8> = (0..1 << 20).map(|i| (i % 253) as u8).collect();
    ws.send_binary(&big).unwrap();
    assert!(rx.recv_timeout(WAIT).unwrap() == Message::Binary(big));
}

fn client_engine() -> Engine {
    let e = Engine::builder()
        .client_threads(1)
        .udp_client_threads(1)
        .config({
            let mut v = akio::Config::default();
            v.shutdown_timeout = Duration::from_millis(300);
            v
        })
        .build()
        .unwrap();
    e.start().unwrap();
    e
}

/// A TLS listener with HTTP/1.1, h2 and h3, and a plain one with HTTP/1.1 and h2c, proxying to
/// `up`.
fn ext_akit(up: SocketAddr, cert: &(std::path::PathBuf, std::path::PathBuf)) -> Akit {
    Akit::start(&format!(
        r#"{{
  "listeners": [
    {{"name": "secure", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2", "h3"],
      "tls": {{"certificates": [{{"cert_file": "{}", "key_file": "{}"}}]}}}},
    {{"name": "plain", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}}
  ],
  "upstreams": [{{"name": "app", "addresses": ["{up}"]}}],
  "routes": [{{"handler": {{"module": "proxy", "params": {{"upstream": "app"}}}}}}]
}}"#,
        cert.0.display(),
        cert.1.display()
    ))
}

#[test]
fn websocket_extended_connect() {
    let _serial = serial();
    let (backend, up, heads) = akio_ws_backend();
    let d = dir("proxy-ws-ext");
    let ca = Ca::new("akit ws test ca");
    let ca_file = ca.write(&d, "ca.pem");
    let cert = ca.leaf(&d, "a", &["a.example"]);
    let akit = ext_akit(up, &cert);
    let (tls, udp, h2c) = (
        akit.addrs[0].tcp.unwrap(),
        akit.addrs[0].udp.unwrap(),
        akit.addrs[1].tcp.unwrap(),
    );
    let ce = client_engine();
    let mut opts = H2WsOptions::default();
    opts.protocols = vec!["chat".into()];

    // h2c.
    let c = H2Client::dial_blocking(&ce, h2c, None, H2Options::default()).unwrap();
    let (tx, rx) = mpsc::channel();
    let ws = c
        .websocket_blocking("http://x/chat", &opts, Rec(tx))
        .unwrap();
    assert_eq!(ws.subprotocol().as_deref(), Some("chat"));
    echoes(&ws, &rx);
    let h = heads.recv_timeout(WAIT).unwrap().to_ascii_lowercase();
    assert!(
        h.contains("sec-websocket-key: ")
            && h.contains("upgrade: websocket")
            && h.contains("sec-websocket-version: 13")
            && h.contains("x-forwarded-for: "),
        "the upstream's handshake: {h}"
    );
    ws.close(1000, "");

    // h2, over TLS.
    let tls_cfg = support::https::client(&ca_file, &["h2"], None);
    let c = H2Client::dial_blocking(&ce, tls, Some((tls_cfg, "a.example")), H2Options::default())
        .unwrap();
    let (tx, rx) = mpsc::channel();
    let ws = c
        .websocket_blocking("https://a.example/chat", &opts, Rec(tx))
        .unwrap();
    echoes(&ws, &rx);
    heads.recv_timeout(WAIT).unwrap();
    ws.close(1000, "");

    // h3.
    let c = akio_h3::H3Client::dial(&ce, udp, "a.example", support::h3::client(&ca_file, None))
        .unwrap();
    let (tx, rx) = mpsc::channel();
    let (dtx, done) = mpsc::channel();
    let mut o3 = akio_h3::H3WsOptions::default();
    o3.protocols = vec!["chat".into()];
    c.websocket("/chat", &o3, Rec(tx), move |r| {
        let _ = dtx.send(r.map_err(|e| e.to_string()));
    });
    let ws = done.recv_timeout(WAIT).unwrap().unwrap();
    echoes(&ws, &rx);
    heads.recv_timeout(WAIT).unwrap();
    ws.close(1000, "");
    c.close();

    ce.shutdown();
    ce.wait();
    backend.shutdown();
    backend.wait();
    drop(akit);
    let _ = std::fs::remove_dir_all(&d);
}

// ---- A bare h2c peer, to read and pace a tunnel by hand --------------------------------------

const DATA: u8 = 0;
const HEADERS: u8 = 1;
const RST_STREAM: u8 = 3;
const SETTINGS: u8 = 4;
const GOAWAY: u8 = 7;
const WINDOW_UPDATE: u8 = 8;
const END_STREAM: u8 = 0x1;
const ACK: u8 = 0x1;
const END_HEADERS: u8 = 0x4;

struct Frame {
    ty: u8,
    flags: u8,
    sid: u32,
    payload: Vec<u8>,
}

struct Peer {
    s: TcpStream,
    enc: Encoder,
    dec: Decoder,
}

impl Peer {
    fn open(addr: SocketAddr) -> Peer {
        let s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(WAIT)).unwrap();
        let mut p = Peer {
            s,
            enc: Encoder::new(),
            dec: Decoder::new(),
        };
        p.s.write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n").unwrap();
        p.send(SETTINGS, 0, 0, &[]);
        p
    }

    fn send(&mut self, ty: u8, flags: u8, sid: u32, payload: &[u8]) {
        let mut b = (payload.len() as u32).to_be_bytes()[1..].to_vec();
        b.push(ty);
        b.push(flags);
        b.extend_from_slice(&sid.to_be_bytes());
        b.extend_from_slice(payload);
        self.s.write_all(&b).unwrap();
    }

    /// An extended CONNECT for `path` with `protocol`.
    fn connect(&mut self, sid: u32, path: &str, protocol: &str) {
        let fields = [
            (":method", "CONNECT"),
            (":protocol", protocol),
            (":scheme", "http"),
            (":path", path),
            (":authority", "x"),
            ("sec-websocket-version", "13"),
        ];
        let b = self
            .enc
            .encode(fields.iter().map(|(n, v)| (n.as_bytes(), v.as_bytes())));
        self.send(HEADERS, END_HEADERS, sid, &b);
    }

    fn read(&mut self) -> Frame {
        let mut h = [0u8; 9];
        self.s.read_exact(&mut h).unwrap();
        let len = u32::from_be_bytes([0, h[0], h[1], h[2]]) as usize;
        let mut payload = vec![0; len];
        self.s.read_exact(&mut payload).unwrap();
        let f = Frame {
            ty: h[3],
            flags: h[4],
            sid: u32::from_be_bytes([h[5], h[6], h[7], h[8]]) & 0x7fff_ffff,
            payload,
        };
        if f.ty == SETTINGS && f.flags & ACK == 0 {
            self.send(SETTINGS, ACK, 0, &[]);
        }
        f
    }

    /// The response head on `sid`: its status and fields.
    fn head(&mut self, sid: u32) -> (u16, Vec<(String, String)>) {
        loop {
            let f = self.read();
            assert!(f.ty != GOAWAY, "GOAWAY");
            if f.sid != sid {
                continue;
            }
            assert_eq!(f.ty, HEADERS, "stream {sid}: frame {}", f.ty);
            let fields: Vec<(String, String)> = self
                .dec
                .decode(&f.payload)
                .unwrap()
                .into_iter()
                .map(|(n, v)| (String::from_utf8(n).unwrap(), String::from_utf8(v).unwrap()))
                .collect();
            let status = fields
                .iter()
                .find(|(n, _)| n == ":status")
                .unwrap()
                .1
                .parse()
                .unwrap();
            return (status, fields);
        }
    }

    /// DATA on `sid` until `n` bytes came or the stream ended, the windows given back as it
    /// arrives: (bytes, ended).
    fn body(&mut self, sid: u32, n: usize) -> (Vec<u8>, bool) {
        let mut out = Vec::new();
        while out.len() < n {
            let f = self.read();
            if f.sid != sid {
                continue;
            }
            match f.ty {
                DATA => {
                    if !f.payload.is_empty() {
                        let inc = (f.payload.len() as u32).to_be_bytes();
                        self.send(WINDOW_UPDATE, 0, 0, &inc);
                        self.send(WINDOW_UPDATE, 0, sid, &inc);
                    }
                    out.extend_from_slice(&f.payload);
                    if f.flags & END_STREAM != 0 {
                        return (out, true);
                    }
                }
                RST_STREAM => return (out, true),
                _ => {}
            }
        }
        (out, false)
    }
}

/// An HTTP/2 client that reads nothing holds the upstream back; once it reads, it gets every
/// byte. Its stream's window is all the proxy may send it meanwhile.
#[test]
fn h2c_backpressure_and_other_protocols() {
    let _serial = serial();
    let b = ws_backend();
    let akit = Akit::plain(&b.addr.to_string(), "", "");
    let mut p = Peer::open(akit.tcp());
    p.connect(1, "/flood", "websocket");
    let (st, fields) = p.head(1);
    assert_eq!(st, 200, "{fields:?}");
    assert!(
        !fields.iter().any(|(n, _)| n == "sec-websocket-accept"),
        "{fields:?}"
    );
    p.send(DATA, 0, 1, b"flood");
    thread::sleep(Duration::from_secs(1));
    let held = b.written.load(Ordering::Relaxed);
    assert!(
        held < FLOOD / 2,
        "the upstream sent {held} bytes to a client that read none"
    );
    let (got, ended) = p.body(1, FLOOD as usize);
    assert!(!ended && got.len() as u64 == FLOOD && got.iter().all(|b| *b == b'f'));
    // The client ends its half: the upstream's connection closes, and so does the stream.
    p.send(DATA, END_STREAM, 1, &[]);
    closed_soon(
        &b.closed,
        "the upstream connection outlived the client's END_STREAM",
    );
    let (_, ended) = p.body(1, usize::MAX);
    assert!(ended);

    // An extended CONNECT for another protocol has no HTTP/1.1 handshake: 501.
    p.connect(3, "/", "connect-udp");
    assert_eq!(p.head(3).0, 501);
}

/// An upstream that answers the handshake without switching: its 2xx, which an extended
/// CONNECT's client would take for an open tunnel and its body for WebSocket frames, is a 502;
/// a 403 passes on with its body; a 2xx whose body keeps coming is a 502 at once, its upstream
/// connection closed. Over h2c and h3.
#[test]
fn websocket_extended_connect_no_switch() {
    let _serial = serial();
    let frame: &'static [u8] = &[0x81, 0x05, b'p', b'w', b'n', b'e', b'd'];
    let (gone_tx, slow_gone) = mpsc::channel::<()>();
    let gone_tx = Mutex::new(gone_tx);
    let up = no_switch_backend(frame, move || {
        let _ = gone_tx.lock().unwrap().send(());
    });
    let d = dir("proxy-ws-noswitch");
    let ca = Ca::new("akit ws test ca");
    let ca_file = ca.write(&d, "ca.pem");
    let cert = ca.leaf(&d, "a", &["a.example"]);
    let akit = ext_akit(up, &cert);
    let (udp, h2c) = (akit.addrs[0].udp.unwrap(), akit.addrs[1].tcp.unwrap());

    // h2c, by hand, to see the bodies.
    let mut p = Peer::open(h2c);
    p.connect(1, "/chat", "websocket");
    let (st, fields) = p.head(1);
    assert_eq!(st, 502, "an upstream's 200 to the handshake: {fields:?}");
    assert!(!fields.iter().any(|(_, v)| v == "text/plain-upstream"));
    let (body, _) = p.body(1, 64);
    assert!(!body.windows(5).any(|w| w == b"pwned"), "{body:?}");
    p.connect(3, "/deny", "websocket");
    assert_eq!(p.head(3).0, 403);
    let (body, _) = p.body(3, frame.len());
    assert_eq!(body, frame);
    p.connect(5, "/slow", "websocket");
    assert_eq!(p.head(5).0, 502);
    closed_soon(
        &slow_gone,
        "the upstream's connection of a 200 still sending was kept",
    );

    // h3: the statuses.
    let ce = client_engine();
    let c = akio_h3::H3Client::dial(&ce, udp, "a.example", support::h3::client(&ca_file, None))
        .unwrap();
    for (path, want) in [("/chat", "502"), ("/deny", "403"), ("/slow", "502")] {
        let (tx, _rx) = mpsc::channel();
        let (dtx, done) = mpsc::channel();
        c.websocket(path, &akio_h3::H3WsOptions::default(), Rec(tx), move |r| {
            let _ = dtx.send(r.map(|_| ()).map_err(|e| e.to_string()));
        });
        let e = done.recv_timeout(WAIT).unwrap().unwrap_err();
        assert!(e.contains(want), "{path}: {e}");
    }
    closed_soon(
        &slow_gone,
        "h3: the upstream's connection of a 200 still sending was kept",
    );
    c.close();
    ce.shutdown();
    ce.wait();
    drop(akit);
    let _ = std::fs::remove_dir_all(&d);
}

/// fit's no-switch backend: `/deny` is 403 with `frame`, `/slow` a 200 whose body never ends
/// (`gone` runs once its connection is closed), anything else a 200 with `frame`; each marked
/// `text/plain-upstream`.
fn no_switch_backend(frame: &'static [u8], gone: impl Fn() + Send + Sync + 'static) -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let gone = Arc::new(gone);
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            let gone = gone.clone();
            thread::spawn(move || {
                let mut w = s.try_clone().unwrap();
                let mut r = BufReader::new(s);
                let Some(head) = read_head(&mut r) else {
                    return;
                };
                let path = head.split(' ').nth(1).unwrap_or("").to_owned();
                let ct = "Content-Type: text/plain-upstream\r\n";
                match path.as_str() {
                    "/deny" => {
                        let _ = w.write_all(
                            format!(
                                "HTTP/1.1 403 Forbidden\r\n{ct}Content-Length: {}\r\n\r\n",
                                frame.len()
                            )
                            .as_bytes(),
                        );
                        let _ = w.write_all(frame);
                    }
                    "/slow" => {
                        let _ = w.write_all(
                            format!("HTTP/1.1 200 OK\r\n{ct}Transfer-Encoding: chunked\r\n\r\n")
                                .as_bytes(),
                        );
                        let _ = w.write_all(format!("{:x}\r\n", frame.len()).as_bytes());
                        let _ = w.write_all(frame);
                        let _ = w.write_all(b"\r\n");
                        // Until akit lets go of it.
                        let mut one = [0; 1];
                        let _ = r.get_mut().set_read_timeout(Some(Duration::from_secs(10)));
                        match r.read(&mut one) {
                            Ok(0) => gone(),
                            Err(e)
                                if e.kind() != std::io::ErrorKind::WouldBlock
                                    && e.kind() != std::io::ErrorKind::TimedOut =>
                            {
                                gone();
                            }
                            _ => {}
                        }
                    }
                    _ => {
                        let _ = w.write_all(
                            format!(
                                "HTTP/1.1 200 OK\r\n{ct}Content-Length: {}\r\n\r\n",
                                frame.len()
                            )
                            .as_bytes(),
                        );
                        let _ = w.write_all(frame);
                    }
                }
            });
        }
    });
    addr
}

// ---- A bare HTTP/3 client, to hold a tunnel's stream ------------------------------------------

#[derive(Debug)]
enum Got {
    Connected,
    Data(StreamId, Vec<u8>, bool),
    Reset(StreamId),
    Closed,
}

struct Raw(mpsc::Sender<Got>);

impl QuicHandler for Raw {
    fn on_connected(&mut self, _c: &Connection) {
        let _ = self.0.send(Got::Connected);
    }
    fn on_stream_data(&mut self, _c: &Connection, id: StreamId, data: Bytes, fin: bool) {
        let _ = self.0.send(Got::Data(id, data.to_vec(), fin));
    }
    fn on_stream_reset(&mut self, _c: &Connection, id: StreamId, _code: u64) {
        let _ = self.0.send(Got::Reset(id));
    }
    fn on_close(&mut self, _c: &Connection, _r: ConnectionError) {
        let _ = self.0.send(Got::Closed);
    }
}

fn varint(out: &mut Vec<u8>, v: u64) {
    match v {
        0..0x40 => out.push(v as u8),
        0x40..0x4000 => out.extend_from_slice(&(0x4000 | v as u16).to_be_bytes()),
        0x4000..0x4000_0000 => out.extend_from_slice(&(0x8000_0000 | v as u32).to_be_bytes()),
        _ => out.extend_from_slice(&(0xc000_0000_0000_0000 | v).to_be_bytes()),
    }
}

fn read_varint(b: &[u8]) -> Option<(u64, usize)> {
    let first = *b.first()?;
    let len = 1usize << (first >> 6);
    if b.len() < len {
        return None;
    }
    let mut v = u64::from(first & 0x3f);
    for x in &b[1..len] {
        v = (v << 8) | u64::from(*x);
    }
    Some((v, len))
}

fn h3_frame(out: &mut Vec<u8>, ty: u64, payload: &[u8]) {
    varint(out, ty);
    varint(out, payload.len() as u64);
    out.extend_from_slice(payload);
}

/// An HTTP/3 client that holds its stream's reads holds the upstream back; once it reads, it
/// gets every byte.
#[test]
fn h3_backpressure() {
    let _serial = serial();
    let b = ws_backend();
    let d = dir("proxy-ws-h3");
    let ca = Ca::new("akit ws test ca");
    let ca_file = ca.write(&d, "ca.pem");
    let cert = ca.leaf(&d, "a", &["a.example"]);
    let akit = ext_akit(b.addr, &cert);
    let udp = akit.addrs[0].udp.unwrap();
    let ce = client_engine();
    let (tx, rx) = mpsc::channel();
    let c = akio_quic::dial(
        &ce,
        udp,
        "a.example",
        support::h3::client(&ca_file, None),
        Raw(tx),
    )
    .unwrap();
    assert!(matches!(rx.recv_timeout(WAIT).unwrap(), Got::Connected));
    let ctl = c.open_uni().unwrap();
    let mut b0 = Vec::new();
    varint(&mut b0, 0); // the control stream
    let mut p = Vec::new();
    varint(&mut p, 0x6);
    varint(&mut p, 1 << 16);
    h3_frame(&mut b0, 0x4, &p);
    c.send(ctl, b0).unwrap();
    let id = c.open_bi().unwrap();
    let mut block = Vec::new();
    akio_h3::qpack::encode(
        [
            (":method", "CONNECT"),
            (":protocol", "websocket"),
            (":scheme", "https"),
            (":authority", "a.example"),
            (":path", "/flood"),
            ("sec-websocket-version", "13"),
        ],
        &mut block,
    );
    let mut head = Vec::new();
    h3_frame(&mut head, 0x1, &block);
    let mut flood = Vec::new();
    h3_frame(&mut flood, 0x0, b"flood");
    head.extend_from_slice(&flood);
    c.hold_reads(id, true).unwrap();
    c.send(id, head).unwrap();
    thread::sleep(Duration::from_secs(1));
    let held = b.written.load(Ordering::Relaxed);
    assert!(
        held < FLOOD / 2,
        "the upstream sent {held} bytes to a client that held its reads"
    );
    c.hold_reads(id, false).unwrap();
    // The frames on the stream: the head, then DATA until the flood came whole.
    let mut buf = Vec::new();
    let (mut data, mut headed) = (0u64, false);
    let deadline = Instant::now() + Duration::from_secs(60);
    while data < FLOOD {
        assert!(Instant::now() < deadline, "{data} bytes in a minute");
        match rx.recv_timeout(WAIT).unwrap() {
            Got::Data(i, bytes, fin) if i == id => {
                buf.extend_from_slice(&bytes);
                assert!(!fin, "the stream ended at {data}");
            }
            Got::Reset(i) if i == id => panic!("reset at {data}"),
            Got::Closed => panic!("closed at {data}"),
            _ => continue,
        }
        let mut at = 0;
        while let Some((ty, a)) = read_varint(&buf[at..]) {
            let Some((len, l)) = read_varint(&buf[at + a..]) else {
                break;
            };
            let end = at + a + l + len as usize;
            if buf.len() < end {
                break;
            }
            match ty {
                0x1 => headed = true,
                0x0 => data += len,
                _ => {}
            }
            at = end;
        }
        buf.drain(..at);
    }
    assert!(headed);
    c.close(0, b"");
    ce.shutdown();
    ce.wait();
    drop(akit);
    let _ = std::fs::remove_dir_all(&d);
}
