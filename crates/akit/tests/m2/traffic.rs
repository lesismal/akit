//! Upstreams and clients for M2's exit tests: test servers of their own (std threads; the tests
//! may block, akit may not), and clients that keep HTTP/1.1, h2c, streamed, WebSocket and TCP
//! traffic going and record every request that fails and every connection that is dropped.
#![allow(
    dead_code,
    unreachable_pub,
    missing_debug_implementations,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use akio::Engine;
use akio_http::client::ClientRequest;
use akio_http::h2client::{H2Client, H2Options};

/// How long a client waits for any answer: a request that takes longer has failed.
pub const WAIT: Duration = Duration::from_secs(10);

/// A streamed response: `STREAM_CHUNKS` chunks of `STREAM_CHUNK` bytes, `STREAM_GAP` apart, so
/// that each spans many swaps.
pub const STREAM_CHUNKS: usize = 20;
pub const STREAM_CHUNK: usize = 1024;
pub const STREAM_GAP: Duration = Duration::from_millis(50);

/// A head up to its blank line; `None` at the end of the stream.
pub fn read_head(r: &mut BufReader<TcpStream>) -> Option<String> {
    let mut head = String::new();
    loop {
        let mut line = String::new();
        match r.read_line(&mut line) {
            Ok(0) | Err(_) => return None,
            Ok(_) => {}
        }
        if line == "\r\n" || line == "\n" {
            return Some(head);
        }
        head.push_str(&line);
    }
}

/// A head's field `name`, in any case.
pub fn field<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines()
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim())
}

/// An upstream named `name`: answers `/health` (its probes, which can be made to hang),
/// `/stream` (a chunked body sent slowly), `/ws` (a WebSocket that echoes what it gets) and
/// anything else with its name, on keep-alive connections.
pub struct App {
    pub addr: SocketAddr,
    /// Connections it accepted.
    pub conns: Arc<AtomicUsize>,
    hang: Arc<AtomicBool>,
    pub probes: Arc<AtomicUsize>,
    pub served: Arc<AtomicUsize>,
}

impl App {
    pub fn start(name: &'static str) -> App {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let hang = Arc::new(AtomicBool::new(false));
        let probes = Arc::new(AtomicUsize::new(0));
        let served = Arc::new(AtomicUsize::new(0));
        let conns = Arc::new(AtomicUsize::new(0));
        let (h, p, s, n) = (hang.clone(), probes.clone(), served.clone(), conns.clone());
        thread::spawn(move || {
            for c in l.incoming() {
                let Ok(c) = c else { return };
                n.fetch_add(1, SeqCst);
                let (h, p, s) = (h.clone(), p.clone(), s.clone());
                thread::spawn(move || app_conn(c, name, &h, &p, &s));
            }
        });
        App {
            addr,
            conns,
            hang,
            probes,
            served,
        }
    }

    /// Its probes left unanswered (`true`) or answered again.
    pub fn hang_probes(&self, hang: bool) {
        self.hang.store(hang, SeqCst);
    }
}

fn app_conn(
    c: TcpStream,
    name: &str,
    hang: &AtomicBool,
    probes: &AtomicUsize,
    served: &AtomicUsize,
) {
    let mut w = c.try_clone().unwrap();
    let mut r = BufReader::new(c);
    while let Some(head) = read_head(&mut r) {
        let path = head.split(' ').nth(1).unwrap_or("");
        let close = field(&head, "connection").is_some_and(|v| v.eq_ignore_ascii_case("close"));
        let ok = match path {
            "/health" => {
                probes.fetch_add(1, SeqCst);
                if hang.load(SeqCst) {
                    // Never answered: held until the prober gives up and closes it.
                    let mut sink = Vec::new();
                    let _ = r.read_to_end(&mut sink);
                    return;
                }
                w.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .is_ok()
            }
            "/stream" => {
                served.fetch_add(1, SeqCst);
                stream(&mut w)
            }
            "/ws" => {
                served.fetch_add(1, SeqCst);
                ws_echo(&head, r, w);
                return;
            }
            _ => {
                served.fetch_add(1, SeqCst);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\n\r\n{name}",
                    name.len()
                );
                w.write_all(resp.as_bytes()).is_ok()
            }
        };
        if !ok || close {
            return;
        }
    }
}

fn stream(w: &mut TcpStream) -> bool {
    if w.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let chunk = vec![b's'; STREAM_CHUNK];
    for _ in 0..STREAM_CHUNKS {
        thread::sleep(STREAM_GAP);
        let mut part = format!("{:x}\r\n", chunk.len()).into_bytes();
        part.extend_from_slice(&chunk);
        part.extend_from_slice(b"\r\n");
        if w.write_all(&part).is_err() {
            return false;
        }
    }
    w.write_all(b"0\r\n\r\n").is_ok()
}

/// Switches to a WebSocket and echoes the bytes it gets: no framing, the proxy only relays.
fn ws_echo(head: &str, mut r: BufReader<TcpStream>, mut w: TcpStream) {
    let key = field(head, "sec-websocket-key").unwrap_or("");
    let resp = format!(
        "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        akio_http::ws::accept_key(key)
    );
    if w.write_all(resp.as_bytes()).is_err() {
        return;
    }
    let mut buf = vec![0; 16 << 10];
    loop {
        match r.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                if w.write_all(&buf[..n]).is_err() {
                    return;
                }
            }
        }
    }
}

/// A TCP server that echoes what it gets.
pub fn echo() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    thread::spawn(move || {
        for c in l.incoming() {
            let Ok(mut c) = c else { return };
            thread::spawn(move || {
                let mut w = c.try_clone().unwrap();
                let _ = std::io::copy(&mut c, &mut w);
            });
        }
    });
    addr
}

/// What the clients saw: requests answered by kind, failures, and the last `/v` body.
#[derive(Default)]
pub struct Seen {
    ok: Mutex<BTreeMap<&'static str, usize>>,
    failed: Mutex<Vec<String>>,
    last: Mutex<String>,
    total: AtomicUsize,
}

impl Seen {
    fn ok(&self, kind: &'static str) {
        *self.ok.lock().unwrap().entry(kind).or_default() += 1;
        self.total.fetch_add(1, SeqCst);
    }

    fn fail(&self, kind: &str, e: impl std::fmt::Display) {
        let mut f = self.failed.lock().unwrap();
        // Enough to tell what went wrong; the rest only counted.
        if f.len() < 50 {
            f.push(format!("{kind}: {e}"));
        } else if f.len() == 50 {
            f.push("...".into());
        }
    }

    /// Requests answered, by kind.
    pub fn counts(&self) -> BTreeMap<&'static str, usize> {
        self.ok.lock().unwrap().clone()
    }

    pub fn failures(&self) -> Vec<String> {
        self.failed.lock().unwrap().clone()
    }

    pub fn total(&self) -> usize {
        self.total.load(SeqCst)
    }

    /// Waits until a client has been answered `body` at `/v`.
    pub fn wait_for(&self, body: &str) {
        let deadline = Instant::now() + WAIT;
        while *self.last.lock().unwrap() != body {
            assert!(
                Instant::now() < deadline,
                "the clients never saw {body:?}, last {:?}",
                self.last.lock().unwrap()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Waits until the clients have been answered `n` more requests.
    pub fn wait_more(&self, n: usize) {
        let want = self.total() + n;
        let deadline = Instant::now() + WAIT;
        while self.total() < want {
            assert!(
                Instant::now() < deadline,
                "the clients stopped: {:?}",
                self.counts()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}

/// A response read off an HTTP/1.1 connection.
pub struct Response {
    pub status: u16,
    /// Whether the server closes the connection after it.
    pub closing: bool,
    pub body: Vec<u8>,
}

/// Reads a response, its body framed by `Content-Length` or chunked.
pub fn read_response(r: &mut BufReader<TcpStream>) -> Result<Response, String> {
    let head = read_head(r).ok_or("the connection was closed before an answer")?;
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("bad status line in {head:?}"))?;
    let closing = field(&head, "connection").is_some_and(|v| v.eq_ignore_ascii_case("close"));
    let mut body = Vec::new();
    if field(&head, "transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        loop {
            let mut size = String::new();
            r.read_line(&mut size)
                .map_err(|e| format!("chunk size: {e}"))?;
            let n = usize::from_str_radix(size.trim(), 16)
                .map_err(|_| format!("bad chunk size {size:?}"))?;
            let mut part = vec![0; n + 2];
            r.read_exact(&mut part).map_err(|e| format!("chunk: {e}"))?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&part[..n]);
        }
    } else if let Some(n) = field(&head, "content-length") {
        let n: usize = n.parse().map_err(|_| format!("bad length in {head:?}"))?;
        body.resize(n, 0);
        r.read_exact(&mut body).map_err(|e| format!("body: {e}"))?;
    }
    Ok(Response {
        status,
        closing,
        body,
    })
}

fn connect(addr: SocketAddr) -> Result<BufReader<TcpStream>, String> {
    let s = TcpStream::connect(addr).map_err(|e| format!("connect {addr}: {e}"))?;
    s.set_read_timeout(Some(WAIT)).unwrap();
    s.set_nodelay(true).unwrap();
    Ok(BufReader::new(s))
}

/// Sends a GET of `path` on `c` and reads its answer.
pub fn get_on(c: &mut BufReader<TcpStream>, path: &str, close: bool) -> Result<Response, String> {
    let conn = if close { "close" } else { "keep-alive" };
    write!(
        c.get_mut(),
        "GET {path} HTTP/1.1\r\nHost: churn.test\r\nConnection: {conn}\r\n\r\n"
    )
    .map_err(|e| format!("write: {e}"))?;
    read_response(c)
}

/// A GET of `path` on a connection of its own.
pub fn get(addr: SocketAddr, path: &str) -> Result<Response, String> {
    get_on(&mut connect(addr)?, path, true)
}

/// Whether `body`, answered at `path`, is what the configs serve there.
fn check(path: &str, body: &[u8]) -> Result<(), String> {
    let good = match path {
        "/v" => body.first() == Some(&b'v'),
        "/up" => body == b"a" || body == b"b",
        "/stream" => body.len() == STREAM_CHUNKS * STREAM_CHUNK && body.iter().all(|&b| b == b's'),
        _ => true,
    };
    if good {
        Ok(())
    } else {
        Err(format!(
            "{path}: unexpected body {:?}",
            String::from_utf8_lossy(&body[..body.len().min(64)])
        ))
    }
}

/// A client's loop.
type Run = Box<dyn FnOnce(&Seen, &AtomicBool) + Send>;

/// The traffic M2's churn test keeps going, until it is stopped.
pub struct Traffic {
    pub seen: Arc<Seen>,
    stop: Arc<AtomicBool>,
    clients: Vec<JoinHandle<()>>,
}

impl Traffic {
    /// Clients of `web` (HTTP/1.1 and h2c) and, when given, of the `tcp` listener `l4`:
    /// - `short`: a connection a request, `Connection: close`, every 10 ms;
    /// - `keep`: one keep-alive connection for the whole run, requests back to back, proxied;
    /// - `idle`: one keep-alive connection, a request every half second, idle in between;
    /// - `h2c`: one HTTP/2 connection for the whole run, four streams at a time;
    /// - `stream`: one keep-alive connection, slow streamed responses, each across many swaps;
    /// - `ws`: one WebSocket through the proxy for the whole run, echoing;
    /// - `tcp`: one connection through the `tcp` service for the whole run, echoing.
    pub fn start(web: SocketAddr, l4: Option<SocketAddr>) -> Traffic {
        let seen = Arc::new(Seen::default());
        let stop = Arc::new(AtomicBool::new(false));
        let mut clients = Vec::new();
        let mut spawn = |f: Run| {
            let (seen, stop) = (seen.clone(), stop.clone());
            clients.push(thread::spawn(move || f(&seen, &stop)));
        };
        for _ in 0..2 {
            spawn(Box::new(move |s, stop| short(web, s, stop)));
        }
        spawn(Box::new(move |s, stop| {
            keep(web, "keep", &["/up", "/v"], Duration::ZERO, s, stop);
        }));
        spawn(Box::new(move |s, stop| {
            keep(web, "idle", &["/v"], Duration::from_millis(500), s, stop);
        }));
        spawn(Box::new(move |s, stop| {
            keep(web, "stream", &["/stream"], Duration::ZERO, s, stop);
        }));
        spawn(Box::new(move |s, stop| h2c(web, s, stop)));
        spawn(Box::new(move |s, stop| ws(web, s, stop)));
        if let Some(l4) = l4 {
            spawn(Box::new(move |s, stop| tcp(l4, s, stop)));
        }
        Traffic {
            seen,
            stop,
            clients,
        }
    }

    /// Stops the clients, each after the request it is in; what they saw.
    pub fn stop(self) -> Arc<Seen> {
        self.stop.store(true, SeqCst);
        for c in self.clients {
            c.join().unwrap();
        }
        self.seen
    }
}

/// A connection a request, one every 10 ms: each closed connection holds an ephemeral port in
/// TIME_WAIT, and back to back they run out (macOS has 16,384).
fn short(web: SocketAddr, seen: &Seen, stop: &AtomicBool) {
    while !stop.load(SeqCst) {
        thread::sleep(Duration::from_millis(10));
        match get(web, "/v") {
            Ok(r) if r.status == 200 && check("/v", &r.body).is_ok() => {
                *seen.last.lock().unwrap() = String::from_utf8_lossy(&r.body).into_owned();
                seen.ok("short");
            }
            Ok(r) => seen.fail("short", format!("{} {:?}", r.status, r.body)),
            Err(e) => seen.fail("short", e),
        }
    }
}

/// One keep-alive connection for the whole run, `paths` in turn, `pace` between requests. A
/// connection the server closes is a dropped connection, and ends the client.
fn keep(
    web: SocketAddr,
    kind: &'static str,
    paths: &[&str],
    pace: Duration,
    seen: &Seen,
    stop: &AtomicBool,
) {
    let mut c = match connect(web) {
        Ok(c) => c,
        Err(e) => return seen.fail(kind, e),
    };
    let mut n = 0usize;
    while !stop.load(SeqCst) {
        let path = paths[n % paths.len()];
        n += 1;
        match get_on(&mut c, path, false) {
            Ok(r) if r.status != 200 => {
                return seen.fail(kind, format!("{path}: status {}", r.status));
            }
            Ok(r) => {
                if let Err(e) = check(path, &r.body) {
                    return seen.fail(kind, e);
                }
                if r.closing {
                    return seen.fail(kind, format!("{path}: the server closed the connection"));
                }
                seen.ok(kind);
            }
            Err(e) => return seen.fail(kind, format!("{path}: {e}")),
        }
        if !pace.is_zero() {
            thread::sleep(pace);
        }
    }
}

fn h2c(web: SocketAddr, seen: &Seen, stop: &AtomicBool) {
    let engine = Engine::builder().client_threads(1).build().unwrap();
    engine.start().unwrap();
    let c = match H2Client::dial_blocking(&engine, web, None, H2Options::default()) {
        Ok(c) => c,
        Err(e) => {
            seen.fail("h2c", format!("dial: {e}"));
            engine.shutdown();
            engine.wait();
            return;
        }
    };
    'run: while !stop.load(SeqCst) {
        let paths = ["/v", "/up", "/v", "/up"];
        let pending: Vec<_> = paths
            .iter()
            .map(|p| c.send(ClientRequest::get(&format!("http://churn.test{p}")).timeout(WAIT)))
            .collect();
        for (p, f) in paths.iter().zip(pending) {
            match f.wait() {
                Ok(r) if r.status == 200 => {
                    if let Err(e) = check(p, &r.body) {
                        seen.fail("h2c", e);
                        break 'run;
                    }
                    seen.ok("h2c");
                }
                Ok(r) => {
                    seen.fail("h2c", format!("{p}: status {}", r.status));
                    break 'run;
                }
                Err(e) => {
                    seen.fail("h2c", format!("{p}: {e}"));
                    break 'run;
                }
            }
        }
    }
    if !c.is_open() {
        seen.fail("h2c", "the connection was closed");
    }
    c.close();
    engine.shutdown();
    engine.wait();
}

/// Writes `ping N` and reads it back, every 20 ms, on a relayed connection.
fn ping(kind: &'static str, c: &mut BufReader<TcpStream>, seen: &Seen, stop: &AtomicBool) {
    let mut n = 0u64;
    while !stop.load(SeqCst) {
        let msg = format!("ping {n:08}");
        n += 1;
        if let Err(e) = c.get_mut().write_all(msg.as_bytes()) {
            return seen.fail(kind, format!("write: {e}"));
        }
        let mut got = vec![0; msg.len()];
        if let Err(e) = c.read_exact(&mut got) {
            return seen.fail(kind, format!("read: {e}"));
        }
        if got != msg.as_bytes() {
            return seen.fail(kind, format!("echoed {:?}", String::from_utf8_lossy(&got)));
        }
        seen.ok(kind);
        thread::sleep(Duration::from_millis(20));
    }
}

fn ws(web: SocketAddr, seen: &Seen, stop: &AtomicBool) {
    let mut c = match connect(web) {
        Ok(c) => c,
        Err(e) => return seen.fail("ws", e),
    };
    let key = "MDEyMzQ1Njc4OWFiY2RlZg==";
    let req = format!(
        "GET /ws HTTP/1.1\r\nHost: churn.test\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\n\r\n"
    );
    if let Err(e) = c.get_mut().write_all(req.as_bytes()) {
        return seen.fail("ws", e);
    }
    match read_head(&mut c) {
        Some(h) if h.starts_with("HTTP/1.1 101") => {}
        other => return seen.fail("ws", format!("not switched: {other:?}")),
    }
    ping("ws", &mut c, seen, stop);
}

fn tcp(l4: SocketAddr, seen: &Seen, stop: &AtomicBool) {
    match connect(l4) {
        Ok(mut c) => ping("tcp", &mut c, seen, stop),
        Err(e) => seen.fail("tcp", e),
    }
}
