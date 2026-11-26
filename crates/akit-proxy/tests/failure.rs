//! fit's failure tests for its proxy (`handler/proxy/failure_test.go`), ported for I17 and I17b:
//! a connect that times out, the upstream's `response_timeout` and the proxy's `timeout` before
//! the response's head and in the middle of its body, `response_header_timeout` and
//! `response_idle_timeout` (I17b), an upstream that breaks off mid-body, a client that goes
//! before the response's head, mid-download and mid-upload (the upstream's request given up, and
//! the upstream not counted as failing for it), trailers both ways, and `limits.write_stall_timeout`
//! (I17b). Over HTTP/1.1 and h2c, to an HTTP/1.1 upstream, on the backend `AKIO_TEST_BACKEND`
//! names (epoll by default, `uring`). `tests/h2.rs` runs the same against an h2c upstream.
//!
//! A request's trailers are sent over HTTP/1.1 alone: akio's h2 client sends none.
//!
//! The upstream is plain threads on std sockets, which report on a channel each request whose
//! client (akit) went before it was answered in full.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, UringConfig};
use akio_http::client::ClientRequest;
use akio_http::h2client::{H2Client, H2Event, H2Options};
use akio_http::{Headers, Method};
use akit_runtime::upstream::EngineSlot;
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

/// fit's `upstreamConfig`: the upstream `app` at `addr` with the extra `fields`, and a route to
/// the proxy with the extra `params`.
fn config(addr: &str, fields: &str, params: &str) -> String {
    format!(
        r#"{{
  "listeners": [{{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}}],
  "upstreams": [{{"name": "app", "addresses": ["{addr}"]{fields}}}],
  "routes": [{{"handler": {{"module": "proxy", "params": {{"upstream": "app"{params}}}}}}}]
}}"#
    )
}

/// akit serving `json`, as `akit run` does.
struct Akit {
    engine: Arc<Engine>,
    addr: SocketAddr,
    cell: GraphCell,
}

impl Akit {
    fn start(json: &str) -> Akit {
        let config = akit_config::from_json(json, None).unwrap();
        let registry = Registry::<HandlerParams>::new(&[akit_proxy::MODULES]).unwrap();
        let slot = EngineSlot::new();
        let cell = GraphCell::new(compile(&config, &registry, &Env::new(slot.clone())).unwrap());
        let engine = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
        engine.start().unwrap();
        assert!(slot.set(engine.clone()));
        let addr = *engine.local_addrs().first().unwrap();
        Akit { engine, addr, cell }
    }

    /// The exchanges with the upstream's first address that failed.
    fn failures(&self) -> u64 {
        self.cell.with(|g| g.upstreams()[0].backend_requests(0).1)
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

/// A body read as its head frames it.
#[derive(Debug, Default)]
struct Body {
    len: usize,
    /// The trailer fields, as `name: value` lines.
    trailers: Vec<String>,
    /// It ended as its framing says, rather than with its connection.
    whole: bool,
}

impl Body {
    fn trailer(&self, name: &str) -> Option<&str> {
        self.trailers
            .iter()
            .filter_map(|l| l.split_once(':'))
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim())
    }
}

/// Reads a body framed as `head` says, to its end or to its connection's.
fn read_body(r: &mut impl BufRead, head: &str) -> Body {
    let mut b = Body::default();
    let mut buf = vec![0; 32 << 10];
    let mut take = |r: &mut dyn BufRead, mut left: usize, b: &mut Body| -> bool {
        while left > 0 {
            let want = left.min(buf.len());
            match r.read(&mut buf[..want]) {
                Ok(n) if n > 0 => {
                    b.len += n;
                    left -= n;
                }
                _ => return false,
            }
        }
        true
    };
    if field(head, "transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        loop {
            let mut line = String::new();
            if r.read_line(&mut line).unwrap_or(0) == 0 {
                return b;
            }
            let Ok(size) = usize::from_str_radix(line.trim(), 16) else {
                return b;
            };
            if size == 0 {
                loop {
                    let mut t = String::new();
                    if r.read_line(&mut t).unwrap_or(0) == 0 {
                        return b;
                    }
                    if t == "\r\n" {
                        b.whole = true;
                        return b;
                    }
                    b.trailers.push(t.trim_end().to_owned());
                }
            }
            if !take(r, size, &mut b) || r.read_exact(&mut [0; 2]).is_err() {
                return b;
            }
        }
    }
    let len = field(head, "content-length").map_or(0, |v| v.parse().unwrap());
    b.whole = take(r, len, &mut b);
    b
}

/// The upstream, on its own threads. By the request's path:
///
/// - `/hang`: no answer;
/// - `/stall`: 200 KiB of a body (of 1 MB by its length with `?length=1`, else chunked), then
///   nothing;
/// - `/broken`: the same, then the connection closed;
/// - `/download`: a chunked body of up to 1000 pieces of 64 KiB, 5ms apart;
/// - `/large`: a chunked body of [`LARGE`] bytes, as fast as it is taken;
/// - `/upload`: the body read to its end, then 200;
/// - `/trailers`: the body read, then a chunked one of 10 bytes (200 KiB with `?big=1`) with an
///   `X-Sum` trailer saying what came: `in=<length> in-trailer=<X-Check trailer> out=<length>`,
///   and the request's `Trailer` field and trailer names in `X-Seen`;
/// - anything else: 200 `ok`.
///
/// Each request whose client went before it was answered in full is sent on the channel.
fn upstream() -> (SocketAddr, Receiver<String>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let (tx, rx) = channel();
    let tx = Arc::new(Mutex::new(tx));
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            let tx = tx.lock().unwrap().clone();
            thread::spawn(move || serve(s, &tx));
        }
    });
    (addr, rx)
}

/// Waits for the connection to end (its reads time out after 10s): what the client's going
/// looks like here.
fn wait_closed(r: &mut impl Read) -> bool {
    let mut buf = [0; 4096];
    loop {
        match r.read(&mut buf) {
            Ok(0) => return true,
            Ok(_) => {}
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return false;
            }
            Err(_) => return true,
        }
    }
}

fn serve(s: TcpStream, gone: &Sender<String>) {
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut w = s.try_clone().unwrap();
    let mut r = BufReader::new(s);
    while let Some(head) = read_head(&mut r) {
        let target = head.split(' ').nth(1).unwrap_or("").to_owned();
        let path = target.split('?').next().unwrap_or("").to_owned();
        let body = read_body(&mut r, &head);
        match path.as_str() {
            "/upload" if !body.whole => {
                let _ = gone.send(path);
                return;
            }
            "/hang" => {
                if wait_closed(&mut r) {
                    let _ = gone.send(path);
                }
                return;
            }
            "/stall" | "/broken" => {
                let framing = if target.contains("length=1") {
                    "Content-Length: 1000000"
                } else {
                    "Transfer-Encoding: chunked"
                };
                let piece = vec![b'x'; 200 << 10];
                let sent = write!(w, "HTTP/1.1 200 OK\r\n{framing}\r\n\r\n").and_then(|()| {
                    if framing.starts_with("Content") {
                        w.write_all(&piece)
                    } else {
                        write!(w, "{:x}\r\n", piece.len())
                            .and_then(|()| w.write_all(&piece))
                            .and_then(|()| w.write_all(b"\r\n"))
                    }
                });
                if sent.is_ok() && path == "/stall" && wait_closed(&mut r) {
                    let _ = gone.send(path);
                }
                return;
            }
            "/large" => {
                let piece = vec![b'l'; 64 << 10];
                let mut sent =
                    write!(w, "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n").is_ok();
                for _ in 0..LARGE / piece.len() {
                    sent = sent
                        && write!(w, "{:x}\r\n", piece.len())
                            .and_then(|()| w.write_all(&piece))
                            .and_then(|()| w.write_all(b"\r\n"))
                            .is_ok();
                }
                if !(sent && w.write_all(b"0\r\n\r\n").is_ok()) {
                    return;
                }
            }
            "/download" => {
                if write!(w, "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n").is_err() {
                    return;
                }
                let piece = vec![b'd'; 64 << 10];
                for _ in 0..1000 {
                    let sent = write!(w, "{:x}\r\n", piece.len())
                        .and_then(|()| w.write_all(&piece))
                        .and_then(|()| w.write_all(b"\r\n"));
                    if sent.is_err() {
                        let _ = gone.send(path);
                        return;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                return;
            }
            "/trailers" => {
                let out = if target.contains("big=1") {
                    200 << 10
                } else {
                    10
                };
                let sum = format!(
                    "in={} in-trailer={} out={out}",
                    body.len,
                    body.trailer("x-check").unwrap_or("")
                );
                let seen = format!(
                    "trailer=[{}] names=[{}]",
                    field(&head, "trailer").unwrap_or(""),
                    body.trailers
                        .iter()
                        .filter_map(|l| l.split_once(':').map(|(n, _)| n.to_ascii_lowercase()))
                        .collect::<Vec<_>>()
                        .join(",")
                );
                let sent = write!(
                    w,
                    "HTTP/1.1 200 OK\r\nTrailer: X-Sum\r\nX-Seen: {seen}\r\n\
                     Transfer-Encoding: chunked\r\n\r\n{out:x}\r\n"
                )
                .and_then(|()| w.write_all(&vec![b'o'; out]))
                .and_then(|()| write!(w, "\r\n0\r\nX-Sum: {sum}\r\n\r\n"));
                if sent.is_err() {
                    return;
                }
            }
            _ => {
                if w.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .is_err()
                {
                    return;
                }
            }
        }
    }
}

/// The length of `/large`'s body: more than a client's socket buffers and akit's queue hold.
const LARGE: usize = 16 << 20;

/// A response as a client read it.
#[derive(Debug)]
struct Got {
    status: u16,
    head: String,
    body: Body,
    took: Duration,
}

/// An HTTP/1.1 request, `extra` after its head (a body), and its response read in full.
fn h1(addr: SocketAddr, head: &str, extra: &[u8]) -> Got {
    let start = Instant::now();
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut w = s.try_clone().unwrap();
    w.write_all(head.as_bytes()).unwrap();
    w.write_all(extra).unwrap();
    let mut r = BufReader::new(s);
    let head = read_head(&mut r).expect("no response head");
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    let body = read_body(&mut r, &head);
    Got {
        status,
        head,
        body,
        took: start.elapsed(),
    }
}

fn get(addr: SocketAddr, path: &str) -> Got {
    h1(
        addr,
        &format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n"),
        b"",
    )
}

/// A response over h2c, as its events reported it.
#[derive(Debug, Default)]
struct H2Got {
    status: u16,
    len: usize,
    trailers: Headers,
    /// It ended (`End`), rather than failed (`Error`).
    ended: bool,
    took: Duration,
}

/// A client engine and an h2c connection to `addr` on it.
fn h2c_conn(addr: SocketAddr) -> (Engine, H2Client) {
    let client = Engine::builder().client_threads(1).build().unwrap();
    client.start().unwrap();
    let c = H2Client::dial_blocking(&client, addr, None, H2Options::default()).unwrap();
    (client, c)
}

/// `GET path` over h2c, with its events collected until it ends or fails.
fn h2c_get(addr: SocketAddr, path: &str) -> H2Got {
    let start = Instant::now();
    let (client, c) = h2c_conn(addr);
    let (tx, rx) = channel();
    let got = Arc::new(Mutex::new(H2Got::default()));
    let g = got.clone();
    let req = ClientRequest::get(&format!("http://{addr}{path}"));
    let upload = c.open(req, move |ev| {
        let mut g = g.lock().unwrap();
        match ev {
            H2Event::Head { status, .. } => g.status = status,
            H2Event::Data(d) => g.len += d.len(),
            H2Event::Trailers(t) => g.trailers = t.clone(),
            H2Event::End => {
                g.ended = true;
                let _ = tx.send(());
            }
            H2Event::Error(_) => {
                let _ = tx.send(());
            }
        }
    });
    // No body: the request ends with its head.
    assert!(upload.finish());
    rx.recv_timeout(Duration::from_secs(10))
        .expect("the h2c request neither ended nor failed");
    client.shutdown();
    let mut g = std::mem::take(&mut *got.lock().unwrap());
    g.took = start.elapsed();
    g
}

/// fit's `waitGone`: the upstream request for `path` is given up within 5s.
fn wait_gone(gone: &Receiver<String>, path: &str) {
    match gone.recv_timeout(Duration::from_secs(5)) {
        Ok(got) => assert_eq!(got, path, "{got} went, want {path}"),
        Err(_) => panic!("the upstream request for {path} was not cancelled"),
    }
}

/// fit's `blackhole`: an address whose connects neither succeed nor fail, a listener that never
/// accepts with its backlog filled; `None` where a full backlog refuses connects (macOS).
fn blackhole() -> Option<(SocketAddr, Vec<TcpStream>, TcpListener)> {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let mut held = Vec::new();
    for _ in 0..4096 {
        match TcpStream::connect_timeout(&addr, Duration::from_millis(100)) {
            Ok(c) => held.push(c),
            Err(e) if e.kind() == ErrorKind::TimedOut || e.kind() == ErrorKind::WouldBlock => {
                return Some((addr, held, l));
            }
            Err(_) => return None,
        }
    }
    None
}

/// fit's `TestConnectTimeout`: a connect that times out is answered 504, soon.
#[test]
fn connect_timeout() {
    let Some((addr, _held, _l)) = blackhole() else {
        eprintln!("skipped: no address here leaves a connect waiting");
        return;
    };
    let akit = Akit::start(&config(
        &addr.to_string(),
        r#", "connect_timeout": "200ms""#,
        "",
    ));
    let got = get(akit.addr, "/");
    assert_eq!(got.status, 504, "{got:?}");
    assert!(got.took < Duration::from_secs(3), "{got:?}");
    let got = h2c_get(akit.addr, "/");
    assert_eq!(got.status, 504, "{got:?}");
    assert!(got.took < Duration::from_secs(3), "{got:?}");
}

/// The upstream's `response_timeout`, and the proxy's `timeout`, before the response's head: 504,
/// once it passed, and the upstream's request given up.
#[test]
fn timeout_before_head() {
    for (fields, params) in [
        (r#", "response_timeout": "300ms""#, ""),
        ("", r#", "timeout": "300ms""#),
        // The shorter of the two holds.
        (r#", "response_timeout": "300ms""#, r#", "timeout": "1h""#),
    ] {
        let (up, gone) = upstream();
        let akit = Akit::start(&config(&up.to_string(), fields, params));
        let got = get(akit.addr, "/hang");
        assert_eq!(got.status, 504, "{fields}{params}: {got:?}");
        assert_eq!(got.body.len, "504 Gateway Timeout\n".len());
        assert!(
            got.took >= Duration::from_millis(250) && got.took < Duration::from_secs(3),
            "{fields}{params}: {got:?}"
        );
        wait_gone(&gone, "/hang");
        let got = h2c_get(akit.addr, "/hang");
        assert_eq!(got.status, 504, "{fields}{params}: {got:?}");
        assert!(got.took < Duration::from_secs(3), "{got:?}");
        wait_gone(&gone, "/hang");
    }
}

/// `response_timeout` mid-body, and fit's `TestUpstreamBreaksMidBody`: a response cut short once
/// it began, by its timeout or by its upstream. The client can tell: an HTTP/1.1 body ends with its connection,
/// short of its length or of its last chunk; an h2c one of a length fails, and one of no length
/// ends with fit's `Akit-Error` trailer.
#[test]
fn cut_short_mid_body() {
    let (up, gone) = upstream();
    let akit = Akit::start(&config(
        &up.to_string(),
        r#", "response_timeout": "500ms""#,
        "",
    ));
    for path in ["/stall", "/broken"] {
        for length in [true, false] {
            let target = if length {
                format!("{path}?length=1")
            } else {
                path.to_owned()
            };
            let got = get(akit.addr, &target);
            assert_eq!(got.status, 200, "{target}: {got:?}");
            assert!(got.body.len >= 200 << 10, "{target}: {got:?}");
            assert!(!got.body.whole, "{target}: a body cut short read as whole");
            assert!(got.took < Duration::from_secs(3), "{target}: {got:?}");
            if path == "/stall" {
                wait_gone(&gone, "/stall");
            }

            let got = h2c_get(akit.addr, &target);
            assert_eq!(got.status, 200, "h2c {target}: {got:?}");
            assert!(got.len >= 200 << 10, "h2c {target}: {got:?}");
            assert!(got.took < Duration::from_secs(3), "h2c {target}: {got:?}");
            if length {
                assert!(!got.ended, "h2c {target}: a body cut short read as whole");
            } else {
                assert_eq!(
                    got.trailers.get("akit-error"),
                    Some("upstream failed mid-body"),
                    "h2c {target}: {got:?}"
                );
            }
            if path == "/stall" {
                wait_gone(&gone, "/stall");
            }
        }
    }
}

/// fit's `TestClientAbort` (the rest is [`client_abort_before_head_and_mid_upload`]): a client
/// that goes mid-download, and an h2c one that goes mid-upload, have the upstream's request given
/// up; and neither takes the upstream out of rotation (`max_fails` is 1 by default).
#[test]
fn client_abort() {
    let (up, gone) = upstream();
    let akit = Akit::start(&config(&up.to_string(), "", ""));

    // Mid-download, over HTTP/1.1: 256 KiB read, then the connection closed.
    {
        let s = TcpStream::connect(akit.addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut w = s.try_clone().unwrap();
        w.write_all(b"GET /download HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let mut r = BufReader::new(s);
        let head = read_head(&mut r).unwrap();
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        let mut buf = vec![0; 256 << 10];
        r.read_exact(&mut buf).unwrap();
    }
    wait_gone(&gone, "/download");
    assert_eq!(get(akit.addr, "/").status, 200);

    // Mid-download, over h2c: the connection dropped once 256 KiB came.
    {
        let (client, c) = h2c_conn(akit.addr);
        let (tx, rx) = channel();
        let mut n = 0;
        let req = ClientRequest::get(&format!("http://{}/download", akit.addr));
        let upload = c.open(req, move |ev| {
            if let H2Event::Data(d) = ev {
                n += d.len();
                if n >= 256 << 10 {
                    let _ = tx.send(());
                }
            }
        });
        assert!(upload.finish());
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        // The connection closed now, rather than once the engine's shutdown is through.
        c.close();
        client.shutdown();
    }
    wait_gone(&gone, "/download");
    assert_eq!(get(akit.addr, "/").status, 200);

    // Mid-upload, over h2c, a body of no declared length.
    {
        let (client, c) = h2c_conn(akit.addr);
        let req = ClientRequest::new(Method::Put, &format!("http://{}/upload", akit.addr));
        let upload = c.open(req, |_| {});
        assert!(upload.write(&vec![b'u'; 256 << 10]));
        thread::sleep(Duration::from_millis(200));
        c.close();
        client.shutdown();
    }
    wait_gone(&gone, "/upload");
    assert_eq!(get(akit.addr, "/").status, 200);
    let got = h2c_get(akit.addr, "/");
    assert_eq!((got.status, got.len), (200, 2), "{got:?}");
    // The upstream did nothing wrong.
    assert_eq!(akit.failures(), 0);
}

/// A chunked body of `body`, ended with the trailer lines `trailers`.
fn chunked(body: &[u8], trailers: &str) -> Vec<u8> {
    let mut out = format!("{:x}\r\n", body.len()).into_bytes();
    out.extend_from_slice(body);
    out.extend_from_slice(format!("\r\n0\r\n{trailers}\r\n").as_bytes());
    out
}

/// fit's `TestTrailers`: a chunked request with a trailer it announced reaches the upstream with
/// it (a trailer it did not announce does not), and the upstream's response trailer reaches the
/// client, for a small body and one that streams, each way, announced; over HTTP/1.1, and for
/// the response's alone over h2c (akio's h2 client sends no request trailers).
#[test]
fn trailers() {
    let (up, _gone) = upstream();
    let akit = Akit::start(&config(&up.to_string(), "", ""));
    for big in [false, true] {
        let (upload, out, target) = if big {
            (200 << 10, 200 << 10, "/trailers?big=1")
        } else {
            (10, 10, "/trailers")
        };
        let got = h1(
            akit.addr,
            &format!(
                "POST {target} HTTP/1.1\r\nHost: x\r\nTrailer: X-Check\r\n\
                 Transfer-Encoding: chunked\r\n\r\n"
            ),
            &chunked(&vec![b'u'; upload], "X-Check: ok\r\nX-Unannounced: no\r\n"),
        );
        let want = format!("in={upload} in-trailer=ok out={out}");
        assert_eq!(got.status, 200, "big={big}: {got:?}");
        assert_eq!(got.body.len, out, "big={big}: {got:?}");
        assert!(got.body.whole, "big={big}: {got:?}");
        assert_eq!(got.body.trailer("x-sum"), Some(want.as_str()), "{got:?}");
        assert_eq!(
            field(&got.head, "x-seen"),
            Some("trailer=[X-Check] names=[x-check]"),
            "big={big}: {got:?}"
        );
        assert_eq!(field(&got.head, "trailer"), Some("X-Sum"), "{got:?}");

        // A request that announces none sends none on.
        let got = h1(
            akit.addr,
            &format!("POST {target} HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n"),
            &chunked(&vec![b'u'; upload], "X-Check: ok\r\n"),
        );
        assert_eq!(
            got.body.trailer("x-sum"),
            Some(format!("in={upload} in-trailer= out={out}").as_str()),
            "{got:?}"
        );
        assert_eq!(field(&got.head, "x-seen"), Some("trailer=[] names=[]"));

        let got = h2c_get(akit.addr, target);
        let want = format!("in=0 in-trailer= out={out}");
        assert!(got.ended, "big={big}: {got:?}");
        assert_eq!((got.status, got.len), (200, out), "big={big}: {got:?}");
        assert_eq!(got.trailers.get("x-sum"), Some(want.as_str()), "{got:?}");
    }
}

/// fit's `TestResponseHeaderTimeout`: an upstream that does not begin its response within
/// `response_header_timeout` is answered 504, and its request given up.
#[test]
fn response_header_timeout() {
    let (up, gone) = upstream();
    let akit = Akit::start(&config(
        &up.to_string(),
        r#", "response_header_timeout": "150ms""#,
        "",
    ));
    let got = get(akit.addr, "/hang");
    assert_eq!(got.status, 504, "{got:?}");
    assert_eq!(got.body.len, "504 Gateway Timeout\n".len());
    assert!(
        got.took >= Duration::from_millis(100) && got.took < Duration::from_secs(3),
        "{got:?}"
    );
    wait_gone(&gone, "/hang");
    let got = h2c_get(akit.addr, "/hang");
    assert_eq!(got.status, 504, "{got:?}");
    assert!(
        got.took >= Duration::from_millis(100) && got.took < Duration::from_secs(3),
        "{got:?}"
    );
    wait_gone(&gone, "/hang");
    // An upstream that answers in time is not cut off, and a long body is not bound by it.
    assert_eq!(get(akit.addr, "/").status, 200);
    let got = get(akit.addr, "/large");
    assert_eq!((got.status, got.body.len), (200, LARGE), "{:?}", got.status);
    assert!(got.body.whole);
}

/// fit's `TestResponseIdleTimeout`: an upstream silent mid-body past `response_idle_timeout` has
/// its response cut short, and the client can tell: an HTTP/1.1 body ends with its connection,
/// an h2c one of a length fails, and one of no length ends with fit's `Akit-Error` trailer.
#[test]
fn response_idle_timeout() {
    let (up, gone) = upstream();
    let akit = Akit::start(&config(
        &up.to_string(),
        r#", "response_idle_timeout": "200ms""#,
        "",
    ));
    for length in [true, false] {
        let target = if length { "/stall?length=1" } else { "/stall" };
        let got = get(akit.addr, target);
        assert_eq!(got.status, 200, "{target}: {got:?}");
        assert!(got.body.len >= 200 << 10, "{target}: {got:?}");
        assert!(!got.body.whole, "{target}: a body cut short read as whole");
        assert!(got.took < Duration::from_secs(3), "{target}: {got:?}");
        wait_gone(&gone, "/stall");

        let got = h2c_get(akit.addr, target);
        assert_eq!(got.status, 200, "h2c {target}: {got:?}");
        assert!(got.len >= 200 << 10, "h2c {target}: {got:?}");
        assert!(got.took < Duration::from_secs(3), "h2c {target}: {got:?}");
        if length {
            assert!(!got.ended, "h2c {target}: a body cut short read as whole");
        } else {
            assert_eq!(
                got.trailers.get("akit-error"),
                Some("upstream failed mid-body"),
                "h2c {target}: {got:?}"
            );
        }
        wait_gone(&gone, "/stall");
    }
}

/// The idle timeout stops while the proxy holds the upstream's reads for a client that is
/// behind: a client that takes nothing of a large body for longer than it still gets it whole.
#[test]
fn response_idle_timeout_spares_a_slow_client() {
    let (up, _gone) = upstream();
    let akit = Akit::start(&config(
        &up.to_string(),
        r#", "response_idle_timeout": "200ms", "max_fails": 0"#,
        "",
    ));
    let s = TcpStream::connect(akit.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut w = s.try_clone().unwrap();
    w.write_all(b"GET /large HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    thread::sleep(Duration::from_millis(800));
    let mut r = BufReader::new(s);
    let head = read_head(&mut r).unwrap();
    let body = read_body(&mut r, &head);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body.len, LARGE);
    assert!(body.whole);
}

/// The rest of fit's `TestClientAbort` (I17b): a client that goes before the response's head,
/// over HTTP/1.1 and h2c, and one that goes mid-upload over HTTP/1.1, have the upstream's request
/// given up, which does not count against the upstream.
#[test]
fn client_abort_before_head_and_mid_upload() {
    let (up, gone) = upstream();
    let akit = Akit::start(&config(&up.to_string(), "", ""));

    // Before the head, over HTTP/1.1: the client gives up waiting.
    {
        let mut s = TcpStream::connect(akit.addr).unwrap();
        s.write_all(b"GET /hang HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        thread::sleep(Duration::from_millis(200));
    }
    wait_gone(&gone, "/hang");

    // Before the head, over h2c.
    {
        let (client, c) = h2c_conn(akit.addr);
        let req = ClientRequest::get(&format!("http://{}/hang", akit.addr));
        let upload = c.open(req, |_| {});
        assert!(upload.finish());
        thread::sleep(Duration::from_millis(200));
        c.close();
        client.shutdown();
    }
    wait_gone(&gone, "/hang");

    // Mid-upload, over HTTP/1.1: a body that stops arriving, then a client that goes.
    {
        let mut s = TcpStream::connect(akit.addr).unwrap();
        s.write_all(b"PUT /upload HTTP/1.1\r\nHost: x\r\nContent-Length: 10485760\r\n\r\n")
            .unwrap();
        s.write_all(&vec![b'u'; 256 << 10]).unwrap();
        thread::sleep(Duration::from_millis(200));
    }
    wait_gone(&gone, "/upload");

    assert_eq!(get(akit.addr, "/").status, 200);
    // The upstream did nothing wrong.
    assert_eq!(akit.failures(), 0);
}

/// `limits.write_stall_timeout` (I17b): a client that takes nothing of a large response for
/// longer loses it, its connection closed, rather than holding it open.
#[test]
fn write_stall_timeout() {
    let (up, _gone) = upstream();
    let json = config(&up.to_string(), r#", "max_fails": 0"#, "").replacen(
        "\"upstreams\"",
        r#""limits": {"write_stall_timeout": "300ms"}, "upstreams""#,
        1,
    );
    let akit = Akit::start(&json);
    let s = TcpStream::connect(akit.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut w = s.try_clone().unwrap();
    w.write_all(b"GET /large HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    // Closed between one and two timeouts after its last progress.
    thread::sleep(Duration::from_secs(2));
    let mut r = BufReader::new(s);
    let head = read_head(&mut r).unwrap();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let body = read_body(&mut r, &head);
    assert!(!body.whole, "the body came whole: {} bytes", body.len);
    assert!(body.len < LARGE, "{} bytes", body.len);
}
