//! The `fastcgi` handler through akit's listener on akio, with fit's `handler/fastcgi` tests
//! (`fastcgi_test.go`, `script_test.go`) ported against a scripted FastCGI application of the
//! tests' own, as fit's `fakeApp`: parameters, keep-alive, large bodies both ways and a slow
//! client, timeouts and limits, connections cut by either side, `try_files`, dotfiles, malformed
//! responses, idle and stale connections, queued requests, clients that go, a body read slowly or
//! answered early, and an application that is not there. Over HTTP/1.1 and h2c, on the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::type_complexity
)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, UringConfig};
use akio_http::Method;
use akio_http::client::ClientRequest;
use akio_http::h2client::{H2Client, H2Event, H2Options};
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, Registry, compile};

// ---- FastCGI records, as the application reads and writes them ---------------------------------

const PARAMS: u8 = 4;
const STDIN: u8 = 5;
const STDOUT: u8 = 6;
const STDERR: u8 = 7;
const END_REQUEST: u8 = 3;

fn record(t: u8, content: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    let mut rest = content;
    loop {
        let n = rest.len().min(65535);
        let pad = (8 - n % 8) % 8;
        b.extend_from_slice(&[1, t, 0, 1, (n >> 8) as u8, n as u8, pad as u8, 0]);
        b.extend_from_slice(&rest[..n]);
        b.extend(std::iter::repeat_n(0u8, pad));
        rest = &rest[n..];
        if rest.is_empty() {
            return b;
        }
    }
}

fn out(s: &str) -> Vec<u8> {
    record(STDOUT, s.as_bytes())
}

fn end() -> Vec<u8> {
    record(END_REQUEST, &[0; 8])
}

/// A connection of the application's: TCP or a Unix-domain socket.
enum Sock {
    Tcp(TcpStream),
    Unix(UnixStream),
}

impl Sock {
    fn try_clone(&self) -> std::io::Result<Sock> {
        Ok(match self {
            Sock::Tcp(s) => Sock::Tcp(s.try_clone()?),
            Sock::Unix(s) => Sock::Unix(s.try_clone()?),
        })
    }

    fn shutdown(&self) {
        let _ = match self {
            Sock::Tcp(s) => s.shutdown(Shutdown::Both),
            Sock::Unix(s) => s.shutdown(Shutdown::Both),
        };
    }

    fn set_read_timeout(&self, d: Option<Duration>) {
        let _ = match self {
            Sock::Tcp(s) => s.set_read_timeout(d),
            Sock::Unix(s) => s.set_read_timeout(d),
        };
    }
}

impl Read for Sock {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Sock::Tcp(s) => s.read(b),
            Sock::Unix(s) => s.read(b),
        }
    }
}

impl Write for Sock {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        match self {
            Sock::Tcp(s) => s.write(b),
            Sock::Unix(s) => s.write(b),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Reads records from a connection, keeping what is left of one between reads.
struct Records {
    conn: Sock,
    buf: Vec<u8>,
}

impl Records {
    /// The next record: its type, id and content; `None` once the connection ends.
    fn next(&mut self) -> Option<(u8, u16, Vec<u8>)> {
        loop {
            if self.buf.len() >= 8 {
                let n = usize::from(u16::from_be_bytes([self.buf[4], self.buf[5]]));
                let total = 8 + n + usize::from(self.buf[6]);
                if self.buf.len() >= total {
                    let rec = (
                        self.buf[1],
                        u16::from_be_bytes([self.buf[2], self.buf[3]]),
                        self.buf[8..8 + n].to_vec(),
                    );
                    self.buf.drain(..total);
                    return Some(rec);
                }
            }
            let mut b = [0u8; 64 << 10];
            match self.conn.read(&mut b) {
                Ok(0) | Err(_) => return None,
                Ok(k) => self.buf.extend_from_slice(&b[..k]),
            }
        }
    }
}

fn decode_params(mut b: &[u8]) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let len = |b: &mut &[u8]| -> usize {
        if b[0] & 0x80 == 0 {
            let n = usize::from(b[0]);
            *b = &b[1..];
            n
        } else {
            let n = u32::from_be_bytes([b[0] & 0x7f, b[1], b[2], b[3]]) as usize;
            *b = &b[4..];
            n
        }
    };
    while !b.is_empty() {
        let n = len(&mut b);
        let v = len(&mut b);
        m.insert(
            String::from_utf8_lossy(&b[..n]).into_owned(),
            String::from_utf8_lossy(&b[n..n + v]).into_owned(),
        );
        b = &b[n + v..];
    }
    m
}

/// A request the application read: its parameters, and its body so far.
struct Req {
    params: HashMap<String, String>,
    stdin: Vec<u8>,
    stdin_end: bool,
}

/// Reads a request's parameters; `None` once the connection ends.
fn read_request(r: &mut Records) -> Option<Req> {
    let mut raw = Vec::new();
    loop {
        let (t, _, content) = r.next()?;
        if t == PARAMS {
            if content.is_empty() {
                break;
            }
            raw.extend_from_slice(&content);
        }
    }
    Some(Req {
        params: decode_params(&raw),
        stdin: Vec::new(),
        stdin_end: false,
    })
}

/// Reads the rest of a request's body.
fn rest(r: &mut Records, q: &mut Req) -> bool {
    while !q.stdin_end {
        let Some((t, _, content)) = r.next() else {
            return false;
        };
        if t == STDIN {
            if content.is_empty() {
                q.stdin_end = true;
            }
            q.stdin.extend_from_slice(&content);
        }
    }
    true
}

/// A digest of a body, the same on both sides (FNV-1a).
fn digest(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, c| {
        (h ^ u64::from(*c)).wrapping_mul(0x0100_0000_01b3)
    })
}

// ---- The application ----------------------------------------------------------------------------

/// A scripted FastCGI application: it answers each request as its script, the `SCRIPT_NAME`
/// akit sends, says, breaking the protocol where a test wants it broken.
struct App {
    addr: Option<SocketAddr>,
    path: Option<std::path::PathBuf>,
    /// Hears of each `/hang.php`-like request as it arrives, and of a request dropped.
    got: Receiver<String>,
    /// Lets a `/hang.php`-like request go on.
    release: Sender<()>,
    /// Hears the script of a connection's last request once the connection is closed.
    closed: Receiver<String>,
    accepted: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl Drop for App {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(a) = self.addr {
            let _ = TcpStream::connect(a);
        }
        if let Some(p) = &self.path {
            let _ = UnixStream::connect(p);
        }
    }
}

#[derive(Clone)]
struct AppSide {
    got: Sender<String>,
    release: Arc<Mutex<Receiver<()>>>,
    closed: Sender<String>,
}

/// Where the application listens.
enum Listener {
    Tcp(TcpListener),
    Unix(UnixListener),
}

fn app() -> App {
    app_on(Listener::Tcp(TcpListener::bind("127.0.0.1:0").unwrap()))
}

fn app_on(ln: Listener) -> App {
    let addr = match &ln {
        Listener::Tcp(l) => Some(l.local_addr().unwrap()),
        Listener::Unix(_) => None,
    };
    let path = match &ln {
        Listener::Unix(l) => l
            .local_addr()
            .ok()
            .and_then(|a| a.as_pathname().map(Path::to_path_buf)),
        Listener::Tcp(_) => None,
    };
    let (got_tx, got) = channel();
    let (release, release_rx) = channel();
    let (closed_tx, closed) = channel();
    let accepted = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let side = AppSide {
        got: got_tx,
        release: Arc::new(Mutex::new(release_rx)),
        closed: closed_tx,
    };
    let (acc, st) = (accepted.clone(), stop.clone());
    thread::spawn(move || {
        loop {
            let conn = match &ln {
                Listener::Tcp(l) => l.accept().map(|(c, _)| Sock::Tcp(c)),
                Listener::Unix(l) => l.accept().map(|(c, _)| Sock::Unix(c)),
            };
            if st.load(Ordering::Relaxed) {
                return;
            }
            let Ok(conn) = conn else { continue };
            acc.fetch_add(1, Ordering::SeqCst);
            let side = side.clone();
            thread::spawn(move || serve_conn(&side, conn));
        }
    });
    App {
        addr,
        path,
        got,
        release,
        closed,
        accepted,
        stop,
    }
}

/// How a wait for the release ended.
#[derive(PartialEq)]
enum Wait {
    Released,
    Dead,
    TimedOut,
}

/// Waits for the release, or for akit to close `conn`.
fn wait_release(side: &AppSide, conn: &Sock) -> Wait {
    let dead = Arc::new(AtomicBool::new(false));
    let mut reader = conn.try_clone().unwrap();
    let d = dead.clone();
    thread::spawn(move || {
        let mut b = [0u8; 64 << 10];
        loop {
            match reader.read(&mut b) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        d.store(true, Ordering::SeqCst);
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if side.release.lock().unwrap().try_recv().is_ok() {
            return Wait::Released;
        }
        if dead.load(Ordering::SeqCst) {
            return Wait::Dead;
        }
        thread::sleep(Duration::from_millis(5));
    }
    Wait::TimedOut
}

/// Reads `conn` until akit closes it.
fn wait_closed(conn: &Sock) {
    let mut c = conn.try_clone().unwrap();
    c.set_read_timeout(Some(Duration::from_secs(15)));
    let mut b = [0u8; 64 << 10];
    while matches!(c.read(&mut b), Ok(k) if k > 0) {}
}

fn serve_conn(side: &AppSide, conn: Sock) {
    let mut r = Records {
        conn: conn.try_clone().unwrap(),
        buf: Vec::new(),
    };
    let mut last = String::new();
    while let Some(mut q) = read_request(&mut r) {
        last = q.params.get("SCRIPT_NAME").cloned().unwrap_or_default();
        if !answer(side, &mut r, &conn, &mut q, &last) {
            break;
        }
    }
    conn.shutdown();
    let _ = side.closed.send(last);
}

/// Answers `q` as `script` says; whether the connection is kept.
fn answer(side: &AppSide, r: &mut Records, conn: &Sock, q: &mut Req, script: &str) -> bool {
    let mut w = conn.try_clone().unwrap();
    let mut write = |bs: &[&[u8]]| bs.iter().all(|b| w.write_all(b).is_ok());
    match script {
        "/env.php" | "/index.php" | "/a.php" => {
            rest(r, q);
            let script_name = q.params["SCRIPT_FILENAME"]
                .strip_prefix(q.params["DOCUMENT_ROOT"].as_str())
                .unwrap_or("")
                .to_owned();
            let mut body = String::new();
            for k in [
                "SCRIPT_FILENAME",
                "PATH_TRANSLATED",
                "PATH_INFO",
                "DOCUMENT_ROOT",
                "REQUEST_SCHEME",
                "REDIRECT_STATUS",
                "APP_ENV",
                "QUERY_STRING",
                "REQUEST_METHOD",
                "REQUEST_URI",
                "HTTP_X_TEST",
                "HTTP_PROXY",
                "REMOTE_ADDR",
                "SERVER_ADDR",
                "SERVER_PORT",
                "SERVER_PROTOCOL",
                "CONTENT_LENGTH",
            ] {
                body.push_str(&format!(
                    "{k}={}\n",
                    q.params.get(k).map_or("", String::as_str)
                ));
            }
            write(&[
                &out(&format!(
                    "X-Script: {script_name}\r\nContent-Type: text/plain\r\n\r\n"
                )),
                &out(&body),
                &end(),
            ])
        }
        "/echo.php" => {
            rest(r, q);
            write(&[
                &out(&format!("\r\n\r\n{} {}", q.stdin.len(), digest(&q.stdin))),
                &end(),
            ])
        }
        "/big.php" => {
            rest(r, q);
            if !write(&[&out("Content-Type: application/octet-stream\r\n\r\n")]) {
                return false;
            }
            let chunk = record(STDOUT, &b"0123456789abcdef".repeat(4096));
            for _ in 0..256 {
                // 16 MiB
                if !write(&[&chunk]) {
                    return false;
                }
            }
            write(&[&end()])
        }
        "/status.php" => {
            rest(r, q);
            write(&[
                &out("Status: 301 Moved\r\nLocation: /elsewhere\r\n\r\n"),
                &end(),
            ])
        }
        "/lf.php" => {
            rest(r, q);
            write(&[&out("Content-Type: text/plain\n\nhello"), &end()])
        }
        "/split.php" => {
            rest(r, q);
            write(&[
                &out("X-A: 1\r\nConnection: close\r\nContent-Length: 4\r\n"),
                &out("\r\nbody"),
                &end(),
            ])
        }
        "/bighead.php" => {
            rest(r, q);
            write(&[&out(&"X-Long: aaaaaaaaaaaaaaaa\r\n".repeat(200))]);
            wait_closed(conn);
            false
        }
        "/nocolon.php" => {
            rest(r, q);
            write(&[&out("garbage\r\n\r\n")]);
            wait_closed(conn);
            false
        }
        "/badstatus.php" => {
            rest(r, q);
            write(&[&out("Status: abc\r\n\r\n")]);
            wait_closed(conn);
            false
        }
        "/bye.php" => {
            // Answers, then reads the next request on the connection and closes it unanswered,
            // as an application that went away while the connection was idle.
            rest(r, q);
            write(&[&out("\r\n\r\nbye"), &end()]);
            if read_request(r).is_some() {
                let _ = side.got.send("dropped".into());
            }
            false
        }
        "/location.php" => {
            rest(r, q);
            write(&[&out("Location: /x\r\n\r\n"), &end()])
        }
        "/empty.php" => {
            rest(r, q);
            write(&[&end()])
        }
        "/headbad.php" => {
            rest(r, q);
            write(&[&out("junk"), &end()]);
            wait_closed(conn);
            false
        }
        "/stderr.php" => {
            rest(r, q);
            let mut b = Vec::new();
            for i in 0..300 {
                b.extend(record(STDERR, format!("line {i}\n").as_bytes()));
            }
            write(&[&b, &out("Content-Type: text/plain\r\n\r\nok"), &end()])
        }
        "/wrongid.php" => {
            rest(r, q);
            let mut b = out("X: y\r\n\r\n");
            b[3] = 2;
            write(&[&b]);
            wait_closed(conn);
            false
        }
        "/shortend.php" => {
            rest(r, q);
            write(&[
                &out("X: y\r\n\r\nbody"),
                &record(END_REQUEST, &[0, 0, 0, 0]),
            ]);
            wait_closed(conn);
            false
        }
        "/badversion.php" => {
            rest(r, q);
            write(&[&[2, 6, 0, 1, 0, 0, 0, 0]]);
            wait_closed(conn);
            false
        }
        "/cl.php" | "/cut.php" => {
            rest(r, q);
            write(&[&out("Content-Length: 100\r\n\r\npartial")]);
            thread::sleep(Duration::from_millis(50));
            false
        }
        "/idlejunk.php" => {
            rest(r, q);
            if !write(&[&out("\r\n\r\nok"), &end()]) {
                return false;
            }
            thread::sleep(Duration::from_millis(100));
            // A record while the connection is idle.
            write(&[&out("late")]);
            wait_closed(conn);
            false
        }
        "/closeafter.php" => {
            rest(r, q);
            write(&[&out("\r\n\r\nok"), &end()]);
            thread::sleep(Duration::from_millis(100));
            false
        }
        "/early.php" => {
            // Answers before the body has all come.
            write(&[&out("\r\n\r\nearly"), &end()]);
            wait_closed(conn);
            false
        }
        "/slowread.php" => {
            thread::sleep(Duration::from_millis(300));
            if !rest(r, q) {
                return false;
            }
            write(&[&out(&format!("\r\n\r\n{}", q.stdin.len())), &end()])
        }
        "/stream.php" | "/gone.php" => {
            rest(r, q);
            if !write(&[&out("Content-Type: application/octet-stream\r\n\r\n")]) {
                return false;
            }
            let chunk = out(&"x".repeat(60000));
            for _ in 0..300 {
                // 18 MB
                if !write(&[&chunk]) {
                    return false;
                }
            }
            write(&[&end()]);
            false
        }
        "/hang.php" | "/die.php" | "/hold.php" | "/slow.php" => {
            if script != "/hold.php" {
                rest(r, q);
            }
            let _ = side.got.send(script.to_owned());
            if wait_release(side, conn) == Wait::Dead {
                return false;
            }
            // Closed after: the watch on the connection reads on.
            match script {
                "/hang.php" => write(&[&out("\r\n\r\nhung"), &end()]),
                "/slow.php" => write(&[&out("\r\n\r\nslow"), &end()]),
                _ => false,
            };
            false
        }
        _ => {
            rest(r, q);
            write(&[&out("Status: 404\r\n\r\n"), &end()])
        }
    }
}

impl App {
    fn addr(&self) -> String {
        self.addr.unwrap().to_string()
    }

    /// Waits until a connection whose last request ran `script` is closed.
    fn wait_closed(&self, script: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.closed.recv_timeout(left) {
                Ok(s) if s == script => return,
                Ok(_) => {}
                Err(_) => panic!("the connection of {script} stayed open"),
            }
        }
    }

    fn wait_got(&self, what: &str) -> String {
        self.got
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("no {what}"))
    }
}

// ---- akit -----------------------------------------------------------------------------------------

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

/// akit serving the `fastcgi` handler of `params` on every route, `top` being more of the
/// config's top level (its limits).
struct Akit {
    engine: Arc<Engine>,
    addr: SocketAddr,
    _cell: GraphCell,
}

impl Drop for Akit {
    fn drop(&mut self) {
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
    }
}

fn serve_top(top: &str, params: &str) -> Akit {
    let json = format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}}], {top}
            "routes": [{{"handler": {{"module": "fastcgi", "params": {params}}}}}]}}"#
    );
    let config = akit_config::from_json(&json, None).unwrap_or_else(|e| panic!("{json}: {e}"));
    let slot = EngineSlot::new();
    let registry = Registry::new(&[akit_handler::MODULES]).unwrap();
    let cell = GraphCell::new(compile(&config, &registry, &Env::new(slot.clone())).unwrap());
    let engine = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
    engine.start().unwrap();
    assert!(slot.set(engine.clone()));
    let addr = *engine.local_addrs().first().unwrap();
    Akit {
        engine,
        addr,
        _cell: cell,
    }
}

fn serve(params: &str) -> Akit {
    serve_top("", params)
}

// ---- Clients ---------------------------------------------------------------------------------------

fn clients() -> Arc<Engine> {
    static E: OnceLock<Arc<Engine>> = OnceLock::new();
    E.get_or_init(|| {
        let e = Engine::builder().client_threads(2).build().unwrap();
        e.start().unwrap();
        Arc::new(e)
    })
    .clone()
}

/// A response as a client got it.
#[derive(Debug, Default)]
struct Got {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    trailers: Vec<(String, String)>,
    /// It ended as it should (a broken one is cut short or reset).
    complete: bool,
}

impl Got {
    fn one(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn trailer(&self, name: &str) -> Option<&str> {
        self.trailers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// `method path` over h2c to `addr`, with `fields` and `body`.
fn h2(addr: SocketAddr, method: Method, path: &str, fields: &[(&str, &str)], body: &[u8]) -> Got {
    let c = H2Client::dial_blocking(&clients(), addr, None, H2Options::default()).unwrap();
    let mut req = ClientRequest::new(method, &format!("http://{addr}{path}"));
    for (n, v) in fields {
        req.headers.append(n, v);
    }
    let (tx, rx) = channel();
    let mut got = Got::default();
    let up = c.open(req, move |e| match e {
        H2Event::Head { status, headers } => {
            got.status = status;
            got.headers = headers
                .iter()
                .map(|(n, v)| (n.to_owned(), v.to_owned()))
                .collect();
        }
        H2Event::Data(d) => got.body.extend_from_slice(d),
        H2Event::Trailers(t) => {
            got.trailers = t
                .iter()
                .map(|(n, v)| (n.to_owned(), v.to_owned()))
                .collect();
        }
        H2Event::End => {
            got.complete = true;
            let _ = tx.send(std::mem::take(&mut got));
        }
        H2Event::Error(_) => {
            let _ = tx.send(std::mem::take(&mut got));
        }
    });
    for piece in body.chunks(32 << 10) {
        assert!(up.write(piece));
    }
    up.finish();
    let got = rx.recv_timeout(Duration::from_secs(30)).unwrap();
    c.close();
    got
}

fn h2_get(addr: SocketAddr, path: &str) -> Got {
    h2(addr, Method::Get, path, &[], &[])
}

/// A raw HTTP/1.1 exchange on a connection of its own: `head` (its request line and fields,
/// without the blank line) and `body` written as they are, the response read as it is framed.
fn h1(addr: SocketAddr, head: &str, body: &[u8]) -> Got {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s.write_all(format!("{head}\r\n\r\n").as_bytes()).unwrap();
    s.write_all(body).unwrap();
    h1_response(&mut BufReader::new(s), head.starts_with("HEAD "))
}

fn h1_get(addr: SocketAddr, path: &str) -> Got {
    h1(addr, &format!("GET {path} HTTP/1.1\r\nHost: x"), &[])
}

/// A response read from `r` as it is framed; `head_only`: to a HEAD request.
fn h1_response(r: &mut BufReader<TcpStream>, head_only: bool) -> Got {
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let status = line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut headers = Vec::new();
    loop {
        let mut l = String::new();
        if r.read_line(&mut l).unwrap() == 0 {
            break;
        }
        let l = l.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((n, v)) = l.split_once(':') {
            headers.push((n.to_owned(), v.trim().to_owned()));
        }
    }
    let mut got = Got {
        status,
        headers,
        ..Got::default()
    };
    if head_only || matches!(got.status, 204 | 304) {
        got.complete = true;
        return got;
    }
    let chunked = got
        .one("transfer-encoding")
        .is_some_and(|v| v.eq_ignore_ascii_case("chunked"));
    if chunked {
        loop {
            let mut size = String::new();
            if r.read_line(&mut size).unwrap_or(0) == 0 {
                return got;
            }
            let Ok(n) = usize::from_str_radix(size.trim(), 16) else {
                return got;
            };
            if n == 0 {
                loop {
                    let mut l = String::new();
                    if r.read_line(&mut l).unwrap_or(0) == 0 {
                        return got;
                    }
                    let l = l.trim_end();
                    if l.is_empty() {
                        got.complete = true;
                        return got;
                    }
                    if let Some((n, v)) = l.split_once(':') {
                        got.trailers.push((n.to_owned(), v.trim().to_owned()));
                    }
                }
            }
            let mut chunk = vec![0; n + 2];
            if r.read_exact(&mut chunk).is_err() {
                return got;
            }
            got.body.extend_from_slice(&chunk[..n]);
        }
    }
    match got
        .one("content-length")
        .and_then(|v| v.parse::<usize>().ok())
    {
        Some(n) => {
            let mut body = vec![0; n];
            let mut read = 0;
            while read < n {
                match r.read(&mut body[read..]) {
                    Ok(0) | Err(_) => break,
                    Ok(k) => read += k,
                }
            }
            body.truncate(read);
            got.complete = read == n;
            got.body = body;
        }
        None => {
            let _ = r.read_to_end(&mut got.body);
            got.complete = true;
        }
    }
    got
}

/// A GET over each protocol.
fn both(addr: SocketAddr, path: &str) -> [(&'static str, Got); 2] {
    [
        ("http/1.1", h1_get(addr, path)),
        ("h2c", h2_get(addr, path)),
    ]
}

// ---- The tests ---------------------------------------------------------------------------------------

/// fit's `TestParams`.
#[test]
fn params() {
    let a = app();
    let s = serve(&format!(
        r#"{{"address": "{}", "root": "/srv/www", "env": {{"APP_ENV": "test"}}}}"#,
        a.addr()
    ));
    let one = h1(
        s.addr,
        "GET /env.php/extra/path?a=1&b=2 HTTP/1.1\r\nHost: x\r\nX-Test: yes\r\nProxy: http://evil",
        &[],
    );
    let two = h2(
        s.addr,
        Method::Get,
        "/env.php/extra/path?a=1&b=2",
        &[("x-test", "yes"), ("proxy", "http://evil")],
        &[],
    );
    // The address the request came to (akio's `Request::local`), as fit's from its TCP
    // connection: the listener's, its port too, not the Host field's.
    let server_port = format!("SERVER_PORT={}\n", s.addr.port());
    for (proto, got) in [("http/1.1", one), ("h2c", two)] {
        let body = got.text();
        for want in [
            "SERVER_ADDR=127.0.0.1\n",
            server_port.as_str(),
            "SCRIPT_FILENAME=/srv/www/env.php\n",
            "PATH_TRANSLATED=/srv/www/extra/path\n",
            "PATH_INFO=/extra/path\n",
            "DOCUMENT_ROOT=/srv/www\n",
            "REQUEST_SCHEME=http\n",
            "REDIRECT_STATUS=200\n",
            "APP_ENV=test\n",
            "QUERY_STRING=a=1&b=2\n",
            "REQUEST_METHOD=GET\n",
            "REQUEST_URI=/env.php/extra/path?a=1&b=2\n",
            "HTTP_X_TEST=yes\n",
            "HTTP_PROXY=\n",
            "REMOTE_ADDR=127.0.0.1\n",
        ] {
            assert!(body.contains(want), "{proto}: no {want:?} in\n{body}");
        }
        assert_eq!(got.status, 200, "{proto}");
        assert_eq!(got.one("x-script"), Some("/env.php"), "{proto}");
    }
    // A field with _ in its name would be HTTP_X_TEST as well, passing for the X-Test an auth
    // middleware set: it is not sent.
    let got = h1(
        s.addr,
        "GET /env.php HTTP/1.1\r\nHost: x\r\nX_Test: forged\r\nx_test: forged",
        &[],
    );
    assert!(got.text().contains("HTTP_X_TEST=\n"), "{}", got.text());
}

/// fit's `TestKeepAlive`: requests one after another reuse a connection; the status and
/// Location a script sets are the response's.
#[test]
fn keep_alive() {
    let a = app();
    let s = serve(&format!(r#"{{"address": "{}", "root": "/srv"}}"#, a.addr()));
    for _ in 0..20 {
        assert_eq!(h1_get(s.addr, "/env.php").status, 200);
    }
    assert_eq!(
        a.accepted.load(Ordering::SeqCst),
        1,
        "connections for 20 requests one after another"
    );
    let got = h1_get(s.addr, "/status.php");
    assert_eq!((got.status, got.one("location")), (301, Some("/elsewhere")));
    assert_eq!(h1_get(s.addr, "/nothing.php").status, 404);
}

/// fit's `TestLargeBodies`: a large upload, a chunked one, and a large download read slowly at
/// first, over each protocol.
#[test]
fn large_bodies() {
    let a = app();
    let s = serve(&format!(r#"{{"address": "{}", "root": "/srv"}}"#, a.addr()));
    let upload = b"upload!".repeat(12 << 17); // 10.5 MiB, under the default body limit
    let want = format!("{} {}", upload.len(), digest(&upload));
    let got = h1(
        s.addr,
        &format!(
            "POST /echo.php HTTP/1.1\r\nHost: x\r\nContent-Length: {}",
            upload.len()
        ),
        &upload,
    );
    assert_eq!(got.text(), want, "http/1.1 upload");
    let got = h2(
        s.addr,
        Method::Post,
        "/echo.php",
        &[("content-length", &upload.len().to_string())],
        &upload,
    );
    assert_eq!(got.text(), want, "h2c upload");
    // Chunked, of no declared length.
    let piece = &upload[..1000];
    let mut chunked = format!("{:x}\r\n", piece.len()).into_bytes();
    chunked.extend_from_slice(piece);
    chunked.extend_from_slice(b"\r\n0\r\n\r\n");
    let got = h1(
        s.addr,
        "POST /echo.php HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked",
        &chunked,
    );
    assert_eq!(got.text(), format!("1000 {}", digest(piece)), "chunked");
    let got = h2(s.addr, Method::Post, "/echo.php", &[], piece);
    assert_eq!(
        got.text(),
        format!("1000 {}", digest(piece)),
        "h2c, no length"
    );

    // A slow client at first.
    let mut c = TcpStream::connect(s.addr).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    c.write_all(b"GET /big.php HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    thread::sleep(Duration::from_millis(200));
    let got = h1_response(&mut BufReader::new(c), false);
    assert!(
        got.complete && got.body.len() == 16 << 20,
        "http/1.1 download: {}",
        got.body.len()
    );
    let got = h2_get(s.addr, "/big.php");
    assert!(
        got.complete && got.body.len() == 16 << 20,
        "h2c download: {}",
        got.body.len()
    );
}

/// fit's `TestTimeoutsAndLimits`: a script slower than `response_header_timeout` (504), and
/// past `max_conns` and `max_pending` a request gets 503.
#[test]
fn timeouts_and_limits() {
    let a = app();
    let s = serve(&format!(
        r#"{{"address": "{}", "root": "/srv", "response_header_timeout": "300ms"}}"#,
        a.addr()
    ));
    assert_eq!(h2_get(s.addr, "/slow.php").status, 504, "a slow script");
    a.wait_got("the slow request");
    let s = serve(&format!(
        r#"{{"address": "{}", "root": "/srv", "max_conns": 1, "max_pending": 1}}"#,
        a.addr()
    ));
    // One busy, one waiting, the third refused.
    let (tx, rx) = channel();
    let mut threads = Vec::new();
    for i in 0..3u64 {
        let (tx, addr) = (tx.clone(), s.addr);
        threads.push(thread::spawn(move || {
            thread::sleep(Duration::from_millis(i * 100));
            tx.send(h1_get(addr, "/slow.php").status).unwrap();
        }));
    }
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(10)).unwrap(),
        503,
        "past max_pending"
    );
    for _ in 0..2 {
        a.wait_got("a request");
        a.release.send(()).unwrap();
    }
    for t in threads {
        t.join().unwrap();
    }
    for _ in 0..2 {
        assert_eq!(rx.recv().unwrap(), 200, "the busy and the waiting");
    }
}

/// fit's `TestCutConnections`: an application that cuts its connection in the middle of a
/// response, and a client that goes in the middle of one.
#[test]
fn cut_connections() {
    let a = app();
    let s = serve(&format!(r#"{{"address": "{}", "root": "/srv"}}"#, a.addr()));
    // HTTP/1.1 closes the connection; HTTP/2 resets the stream, or ends it with a trailer that
    // says so.
    let got = h1_get(s.addr, "/cut.php");
    assert!(!got.complete, "http/1.1: a response cut short read whole");
    let got = h2_get(s.addr, "/cut.php");
    assert!(
        !got.complete || got.trailer("akit-error").is_some(),
        "h2c: a response cut short read whole"
    );
    // The client goes in the middle of a response: the application's connection is closed.
    let mut c = TcpStream::connect(s.addr).unwrap();
    c.write_all(b"GET /gone.php HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut b = [0u8; 4];
    c.read_exact(&mut b).unwrap();
    drop(c);
    a.wait_closed("/gone.php");
    let c = H2Client::dial_blocking(&clients(), s.addr, None, H2Options::default()).unwrap();
    let (tx, rx) = channel();
    let mut tx = Some(tx);
    let up = c.open(
        ClientRequest::new(Method::Get, &format!("http://{}/gone.php", s.addr)),
        move |e| {
            if let H2Event::Data(_) = e {
                if let Some(tx) = tx.take() {
                    let _ = tx.send(());
                }
            }
        },
    );
    up.finish();
    rx.recv_timeout(Duration::from_secs(10)).unwrap();
    c.close();
    a.wait_closed("/gone.php");
}

/// fit's `TestTryFilesAndUnix`, over TCP (a Unix-domain socket in `unix_socket`).
#[test]
fn try_files() {
    let root = tempdir("try");
    for f in ["index.php", "a.php", "dir/index.php"] {
        put(&root, f);
    }
    let a = app();
    let s = serve(&format!(
        r#"{{"address": "{}", "root": {:?}, "try_files": ["{{path}}", "{{path}}/index.php", "/index.php"]}}"#,
        a.addr(),
        root.to_str().unwrap()
    ));
    for (path, want) in [
        ("/a.php", "/a.php"),
        ("/blog/post", "/index.php"),
        ("/a.php/x", "/a.php"),
        ("/", "/index.php"),
    ] {
        let got = h1_get(s.addr, path);
        assert_eq!(got.one("x-script"), Some(want), "{path}: {}", got.text());
    }
    let s = serve(&format!(
        r#"{{"address": "{}", "root": {:?}, "try_files": ["{{path}}"]}}"#,
        a.addr(),
        root.to_str().unwrap()
    ));
    assert_eq!(h1_get(s.addr, "/missing.php").status, 404, "no script");
    let _ = std::fs::remove_dir_all(&root);
}

/// fit's `TestTryFilesAndUnix`, its Unix-domain socket: the application on one, akit dialing
/// it, and a connection kept for the next request.
#[test]
fn unix_socket() {
    let root = tempdir("unix");
    for f in ["index.php", "a.php"] {
        put(&root, f);
    }
    let sock = root.join("fpm.sock");
    let a = app_on(Listener::Unix(UnixListener::bind(&sock).unwrap()));
    let s = serve(&format!(
        r#"{{"address": "unix/{}", "root": {:?}, "try_files": ["{{path}}", "{{path}}/index.php", "/index.php"]}}"#,
        sock.to_str().unwrap(),
        root.to_str().unwrap()
    ));
    for (path, want) in [
        ("/a.php", "/a.php"),
        ("/blog/post", "/index.php"),
        ("/", "/index.php"),
    ] {
        let got = h1_get(s.addr, path);
        assert_eq!(got.one("x-script"), Some(want), "{path}: {}", got.text());
    }
    let got = h2_get(s.addr, "/a.php/x");
    assert_eq!(got.one("x-script"), Some("/a.php"), "h2c: {}", got.text());
    assert_eq!(
        a.accepted.load(Ordering::SeqCst),
        1,
        "connections for 4 requests one after another"
    );
    drop(s);
    drop(a);
    let _ = std::fs::remove_dir_all(&root);
}

fn tempdir(name: &str) -> std::path::PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!(
        "akit-fastcgi-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn put(root: &Path, f: &str) {
    let p = root.join(f);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, "<?php").unwrap();
}

/// fit's `TestDotfiles`: a script in a dotfile's path does not run by default, with try_files or
/// without, but under `/.well-known/` and with `dotfiles` `allow`; its path info may hold dots.
#[test]
fn dotfiles() {
    let root = tempdir("dot");
    for f in ["index.php", ".env", ".git/hooks/x.php", ".hidden.php"] {
        put(&root, f);
    }
    let a = app();
    let r = root.to_str().unwrap();
    let check = |s: &Akit, path: &str, status: u16, script: Option<&str>| {
        let got = h1_get(s.addr, path);
        assert_eq!(
            (got.status, got.one("x-script")),
            (status, script),
            "{path}: {}",
            got.text()
        );
    };
    let s = serve(&format!(r#"{{"address": "{}", "root": {r:?}}}"#, a.addr()));
    for path in [
        "/.env",
        "/.git/hooks/x.php",
        "/.hidden.php",
        "/.git/hooks/x.php/a",
        "/a/.b/index.php",
    ] {
        check(&s, path, 404, None);
    }
    assert_eq!(
        a.accepted.load(Ordering::SeqCst),
        0,
        "a dotfile's request reached the application"
    );
    // The application answers /.well-known/ itself: it is passed on.
    check(&s, "/.well-known/x.php", 404, None);
    assert_eq!(
        a.accepted.load(Ordering::SeqCst),
        1,
        "/.well-known/ did not reach the application"
    );
    check(&s, "/index.php/.env", 200, Some("/index.php"));
    // A path that could name a file outside the root, escaped or not.
    for path in ["/a%5Cb.php", "/a%00.php", "/%2e%2e/x.php"] {
        check(&s, path, 400, None);
    }

    let s = serve(&format!(
        r#"{{"address": "{}", "root": {r:?}, "try_files": ["{{path}}", "/index.php"]}}"#,
        a.addr()
    ));
    for path in ["/.env", "/.git/hooks/x.php", "/.hidden.php"] {
        check(&s, path, 200, Some("/index.php"));
    }
    check(&s, "/a%5Cb.php", 400, None);

    let s = serve(&format!(
        r#"{{"address": "{}", "root": {r:?}, "dotfiles": "allow", "try_files": ["{{path}}", "/index.php"]}}"#,
        a.addr()
    ));
    // The test application knows no such script: 404 from it, not from akit.
    check(&s, "/.hidden.php", 404, None);
    check(&s, "/nothing", 200, Some("/index.php"));
    let _ = std::fs::remove_dir_all(&root);
}

/// fit's `TestResponses`: responses the application writes, well and badly formed.
#[test]
fn responses() {
    let a = app();
    let s = serve(&format!(
        r#"{{"address": "{}", "root": "/srv", "max_header_bytes": 2048}}"#,
        a.addr()
    ));
    for (script, status, body) in [
        ("/lf.php", 200, "hello"),
        ("/split.php", 200, "body"),
        ("/location.php", 302, ""),
        ("/empty.php", 200, ""),
        ("/stderr.php", 200, "ok"),
        ("/bighead.php", 502, ""),
        ("/nocolon.php", 502, ""),
        ("/badstatus.php", 502, ""),
        ("/headbad.php", 502, ""),
        ("/wrongid.php", 502, ""),
        ("/badversion.php", 502, ""),
    ] {
        for (proto, got) in both(s.addr, script) {
            assert_eq!(got.status, status, "{proto} {script}: {}", got.text());
            if !body.is_empty() {
                assert_eq!(got.text(), body, "{proto} {script}");
            }
            if script == "/split.php" {
                assert_eq!(got.one("x-a"), Some("1"), "{proto}");
                assert_eq!(got.one("connection"), None, "{proto}");
            }
        }
    }
    // The body has begun when the end record breaks the protocol, and when the application cuts
    // a response of a declared length: the client is told it broke off.
    for script in ["/shortend.php", "/cl.php"] {
        for (proto, got) in both(s.addr, script) {
            assert!(
                !got.complete || got.trailer("akit-error").is_some() || got.status == 502,
                "{proto} {script}: a broken response read whole"
            );
        }
    }
}

/// fit's `TestIdleConnections`: a record on an idle connection closes it, as does the
/// application, the idle timeout, and more idle connections than are kept.
#[test]
fn idle_connections() {
    let a = app();
    let s = serve(&format!(
        r#"{{"address": "{}", "root": "/srv", "max_idle_conns": 1, "idle_timeout": "300ms"}}"#,
        a.addr()
    ));
    for script in ["/idlejunk.php", "/closeafter.php"] {
        let got = h1_get(s.addr, script);
        assert_eq!((got.status, got.text().as_str()), (200, "ok"), "{script}");
        a.wait_closed(script);
    }
    // The idle timeout.
    assert_eq!(h1_get(s.addr, "/lf.php").status, 200);
    a.wait_closed("/lf.php");
    // Two at once, one kept idle after, the other closed.
    let (tx, rx) = channel();
    for _ in 0..2 {
        let (tx, addr) = (tx.clone(), s.addr);
        thread::spawn(move || tx.send(h1_get(addr, "/hang.php").status).unwrap());
    }
    a.wait_got("the first request");
    a.wait_got("the second request");
    a.release.send(()).unwrap();
    a.release.send(()).unwrap();
    for _ in 0..2 {
        assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), 200);
    }
}

/// fit's `TestQueued`: a request waiting for the only connection whose client goes, and one
/// that gets a new connection once the application closes the busy one.
#[test]
fn queued() {
    let a = app();
    let s = serve(&format!(
        r#"{{"address": "{}", "root": "/srv", "max_conns": 1}}"#,
        a.addr()
    ));
    let (busy_tx, busy) = channel();
    let addr = s.addr;
    let tx = busy_tx.clone();
    thread::spawn(move || tx.send(h1_get(addr, "/hang.php").status).unwrap());
    a.wait_got("the busy request");
    // Queued, then gone.
    let mut c = TcpStream::connect(s.addr).unwrap();
    c.write_all(b"GET /lf.php HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    thread::sleep(Duration::from_millis(300));
    drop(c);
    thread::sleep(Duration::from_millis(100));
    a.release.send(()).unwrap();
    assert_eq!(
        busy.recv_timeout(Duration::from_secs(10)).unwrap(),
        200,
        "busy"
    );

    // A pool of its own: the application closes /die.php's connection unanswered.
    let s = serve(&format!(
        r#"{{"address": "{}", "root": "/srv", "max_conns": 1}}"#,
        a.addr()
    ));
    let addr = s.addr;
    let tx = busy_tx.clone();
    thread::spawn(move || tx.send(h1_get(addr, "/die.php").status).unwrap());
    a.wait_got("the dying request");
    let (queued_tx, queued) = channel();
    thread::spawn(move || queued_tx.send(h2_get(addr, "/lf.php").status).unwrap());
    thread::sleep(Duration::from_millis(300));
    a.release.send(()).unwrap();
    assert_eq!(
        busy.recv_timeout(Duration::from_secs(10)).unwrap(),
        502,
        "a connection closed under its request"
    );
    assert_eq!(
        queued.recv_timeout(Duration::from_secs(10)).unwrap(),
        200,
        "queued behind a closed connection"
    );
}

/// fit's `TestClientsGo`: clients that go while the application has not answered, in the
/// middle of a body, and that stop reading a response.
#[test]
fn clients_go() {
    let a = app();
    let s = serve_top(
        r#""limits": {"write_stall_timeout": "500ms"},"#,
        &format!(r#"{{"address": "{}", "root": "/srv"}}"#, a.addr()),
    );
    // Gone before the response's header.
    let mut c = TcpStream::connect(s.addr).unwrap();
    c.write_all(b"GET /hang.php HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    a.wait_got("the request");
    drop(c);
    a.wait_closed("/hang.php");

    // Gone in the middle of its body, once it streams.
    let mut c = TcpStream::connect(s.addr).unwrap();
    c.write_all(b"POST /hold.php HTTP/1.1\r\nHost: x\r\nContent-Length: 8000000\r\n\r\n")
        .unwrap();
    c.write_all(&[b'0'; 100 << 10]).unwrap();
    a.wait_got("the request with a body");
    drop(c);
    a.wait_closed("/hold.php");

    // Stops reading a long response.
    let mut c = TcpStream::connect(s.addr).unwrap();
    c.write_all(b"GET /stream.php HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    a.wait_closed("/stream.php");
    drop(c);
}

/// fit's `TestBodies`: a body the application reads slowly, held back while it does, and one it
/// answers before reading.
#[test]
fn bodies() {
    let a = app();
    let s = serve(&format!(r#"{{"address": "{}", "root": "/srv"}}"#, a.addr()));
    let upload = vec![b'u'; 8 << 20];
    let got = h1(
        s.addr,
        &format!(
            "POST /slowread.php HTTP/1.1\r\nHost: x\r\nContent-Length: {}",
            upload.len()
        ),
        &upload,
    );
    assert_eq!(
        (got.status, got.text()),
        (200, upload.len().to_string()),
        "http/1.1 slow reader"
    );
    let got = h2(
        s.addr,
        Method::Post,
        "/slowread.php",
        &[("content-length", &upload.len().to_string())],
        &upload,
    );
    assert_eq!(
        (got.status, got.text()),
        (200, upload.len().to_string()),
        "h2c slow reader"
    );

    // Answered before the body has all come.
    let mut c = TcpStream::connect(s.addr).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    c.write_all(b"POST /early.php HTTP/1.1\r\nHost: x\r\nContent-Length: 1048576\r\n\r\n")
        .unwrap();
    let w = c.try_clone().unwrap();
    let writer = thread::spawn(move || {
        let mut w = w;
        for _ in 0..16 {
            if w.write_all(&[0u8; 64 << 10]).is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
    });
    let got = h1_response(&mut BufReader::new(c), false);
    assert_eq!(got.text(), "early", "answered early: {}", got.status);
    writer.join().unwrap();
}

/// fit's `TestOverallTimeout`: an exchange longer than `timeout` gets 504, and its connection
/// is closed.
#[test]
fn overall_timeout() {
    let a = app();
    let s = serve(&format!(
        r#"{{"address": "{}", "root": "/srv", "timeout": "300ms"}}"#,
        a.addr()
    ));
    assert_eq!(h1_get(s.addr, "/hang.php").status, 504);
    a.wait_closed("/hang.php");
}

/// fit's `TestNoApplication`: an address nothing listens on, 502.
#[test]
fn no_application() {
    let ln = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = ln.local_addr().unwrap();
    drop(ln);
    let s = serve(&format!(r#"{{"address": "{addr}", "root": "/srv"}}"#));
    assert_eq!(h1_get(s.addr, "/x.php").status, 502, "a closed port");
    let sock = tempdir("none").join("none.sock");
    let s = serve(&format!(
        r#"{{"address": "unix/{}", "root": "/srv"}}"#,
        sock.to_str().unwrap()
    ));
    assert_eq!(h1_get(s.addr, "/x.php").status, 502, "no socket");
}

/// fit's `TestStaleIdle`: a request that takes an idle connection the application closes
/// unanswered is sent again on a new connection when it has no body and a method safe to
/// repeat, and answered 502 otherwise.
#[test]
fn stale_idle() {
    let a = app();
    let s = serve(&format!(r#"{{"address": "{}", "root": "/srv"}}"#, a.addr()));
    for (method, body, code) in [
        ("GET", "", 200),
        ("HEAD", "", 200),
        ("POST", "", 502),
        ("POST", "abc", 502),
    ] {
        let got = h1_get(s.addr, "/bye.php");
        assert_eq!(got.status, 200, "bye");
        // bye.php's connection is idle once given back; let it settle there.
        thread::sleep(Duration::from_millis(100));
        let got = h1(
            s.addr,
            &format!(
                "{method} /lf.php HTTP/1.1\r\nHost: x\r\nContent-Length: {}",
                body.len()
            ),
            body.as_bytes(),
        );
        assert_eq!(a.wait_got("the dropped request"), "dropped");
        assert_eq!(got.status, code, "{method} {body:?}");
        // The connection of the request sent again stays idle; the next bye.php would take
        // it, not the one that went: closed, so that each round starts alike.
        a.wait_closed("/bye.php");
    }
}

/// fit's `TestProvisionErrors` and `TestDotfilesProvision`: the config's errors, with fit's
/// messages.
#[test]
fn config_errors() {
    let read = |params: &str| {
        akit_config::from_json(
            &format!(
                r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
                    "routes": [{{"handler": {{"module": "fastcgi", "params": {params}}}}}]}}"#
            ),
            None,
        )
    };
    for (params, want) in [
        (
            r#"{"address": "nowhere", "root": "/srv"}"#,
            "is neither a host and port",
        ),
        (
            r#"{"address": "127.0.0.1:9000", "root": "srv"}"#,
            "is not an absolute path",
        ),
        (
            r#"{"address": "127.0.0.1:9000", "root": "/srv", "try_files": ["index.php"]}"#,
            "starts with neither / nor {path}",
        ),
        (
            r#"{"address": "127.0.0.1:9000", "root": "/srv", "index": ".index.php"}"#,
            r#"".index.php" starts with a dot"#,
        ),
        (
            r#"{"address": "127.0.0.1:9000", "root": "/srv", "try_files": ["{path}", "/.app/index.php"]}"#,
            r#"try_files[1]: "/.app/index.php" names a script"#,
        ),
        (
            r#"{"address": "127.0.0.1:9000", "root": "/srv", "try_files": ["/a/../index.php"]}"#,
            r#"names a script with a ".." segment"#,
        ),
        (
            r#"{"address": "127.0.0.1:9000", "root": "/srv", "dotfiles": "no"}"#,
            r#""no" is not one of "deny", "allow""#,
        ),
        (r#"{"root": "/srv"}"#, "address: missing required field"),
        (
            r#"{"address": "127.0.0.1:9000", "root": "/srv", "nope": 1}"#,
            "nope",
        ),
    ] {
        let e = read(params).expect_err(params).to_string();
        assert!(e.contains(want), "{params}: {e}");
    }
    for params in [
        r#"{"address": "127.0.0.1:9000", "root": "/srv", "try_files": ["{path}", "/index.php/.x", "{path}.php"]}"#,
        r#"{"address": "127.0.0.1:9000", "root": "/srv", "dotfiles": "allow", "index": ".index.php", "try_files": ["/.app/index.php"]}"#,
        r#"{"address": "unix//run/php/php-fpm.sock", "root": "/srv", "split_path": [".PHP", ".phtml"], "max_conns": 2, "max_idle_conns": 1, "max_pending": 3, "connect_timeout": "1s", "response_header_timeout": "2s", "response_idle_timeout": "3s", "timeout": "4s", "idle_timeout": "5s", "max_header_bytes": 4096, "env": {"A": "b"}}"#,
    ] {
        read(params).unwrap_or_else(|e| panic!("{params}: {e}"));
    }
}

/// fit's `TestPHPFPM` (`test/e2e/php_test.go`): PHP scripts served by PHP-FPM through the
/// handler, over a Unix-domain socket: their parameters, path info and query, POST forms and
/// bodies, status, fields and cookies, redirects, a large response, and a front controller
/// through `try_files`. PHP-FPM is the binary `AKIT_PHP_FPM` names (`/usr/sbin/php-fpm8.3`, say);
/// skipped without it.
#[test]
fn php_fpm() {
    let Ok(bin) = std::env::var("AKIT_PHP_FPM") else {
        eprintln!("php_fpm: skipped, AKIT_PHP_FPM names no PHP-FPM binary");
        return;
    };
    let root = tempdir("php");
    let scripts = [
        ("hello.php", r#"<?php echo "hello from ", PHP_SAPI;"#),
        (
            "info.php",
            r#"<?php foreach (['SCRIPT_NAME', 'SCRIPT_FILENAME', 'PATH_INFO', 'REQUEST_METHOD', 'REQUEST_URI', 'HTTP_X_TEST', 'HTTP_HOST', 'REMOTE_ADDR', 'HTTP_PROXY'] as $k) { echo $k, '=', $_SERVER[$k] ?? '', "\n"; } echo 'x=', $_GET['x'] ?? '', "\ny=", $_GET['y'] ?? '', "\n";"#,
        ),
        (
            "post.php",
            r#"<?php echo $_POST['name'] ?? '', '|', strlen(file_get_contents('php://input')), '|', md5(file_get_contents('php://input'));"#,
        ),
        (
            "status.php",
            r#"<?php http_response_code(418); header('X-Teapot: yes'); setcookie('a', 'b'); echo 'teapot';"#,
        ),
        ("redirect.php", r#"<?php header('Location: /hello.php');"#),
        (
            "big.php",
            r#"<?php for ($i = 0; $i < 1024; $i++) { echo str_repeat('x', 4096); }"#,
        ),
        (
            "index.php",
            r#"<?php echo "front ", $_SERVER['REQUEST_URI'];"#,
        ),
    ];
    for (name, src) in scripts {
        std::fs::write(root.join(name), src).unwrap();
    }
    let sock = root.join("fpm.sock");
    let conf = root.join("fpm.conf");
    std::fs::write(
        &conf,
        format!(
            "[global]\nerror_log = {}\ndaemonize = no\n[www]\nlisten = {}\npm = static\npm.max_children = 4\nclear_env = no\n",
            root.join("fpm.log").display(),
            sock.display()
        ),
    )
    .unwrap();
    let fpm = std::process::Command::new(&bin)
        .args(["-F", "-R", "-n", "-y"])
        .arg(&conf)
        .spawn()
        .unwrap_or_else(|e| panic!("{bin}: {e}"));
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let fpm = Kill(fpm);
    let s = serve(&format!(
        r#"{{"address": "unix/{}", "root": {:?}, "try_files": ["{{path}}", "{{path}}/index.php", "/index.php"]}}"#,
        sock.display(),
        root.to_str().unwrap()
    ));
    // PHP-FPM takes a moment to start.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let got = h1_get(s.addr, "/hello.php");
        if got.status == 200 {
            assert_eq!(got.text(), "hello from fpm-fcgi");
            break;
        }
        assert!(Instant::now() < deadline, "PHP-FPM did not answer: {got:?}");
        thread::sleep(Duration::from_millis(200));
    }
    let host = s.addr.to_string();
    for proto in ["http/1.1", "h2c"] {
        let get = |path: &str, fields: &[(&str, &str)]| -> Got {
            if proto == "h2c" {
                h2(s.addr, Method::Get, path, fields, &[])
            } else {
                let mut head = format!("GET {path} HTTP/1.1\r\nHost: {host}");
                for (n, v) in fields {
                    head.push_str(&format!("\r\n{n}: {v}"));
                }
                h1(s.addr, &head, &[])
            }
        };
        let post = |path: &str, ctype: &str, body: &[u8]| -> Got {
            if proto == "h2c" {
                h2(
                    s.addr,
                    Method::Post,
                    path,
                    &[
                        ("content-type", ctype),
                        ("content-length", &body.len().to_string()),
                    ],
                    body,
                )
            } else {
                h1(
                    s.addr,
                    &format!(
                        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: {ctype}\r\nContent-Length: {}",
                        body.len()
                    ),
                    body,
                )
            }
        };
        let info = get(
            "/info.php/a/b?x=1&y=two",
            &[("x-test", "yes"), ("proxy", "http://evil.test")],
        )
        .text();
        for want in [
            "SCRIPT_NAME=/info.php\n".to_owned(),
            format!("SCRIPT_FILENAME={}/info.php\n", root.display()),
            "PATH_INFO=/a/b\n".to_owned(),
            "REQUEST_METHOD=GET\n".to_owned(),
            "REQUEST_URI=/info.php/a/b?x=1&y=two\n".to_owned(),
            "HTTP_X_TEST=yes\n".to_owned(),
            "REMOTE_ADDR=127.0.0.1\n".to_owned(),
            "HTTP_PROXY=\n".to_owned(),
            "x=1\ny=two\n".to_owned(),
        ] {
            assert!(info.contains(&want), "{proto}: no {want:?} in\n{info}");
        }
        if proto == "http/1.1" {
            assert!(info.contains(&format!("HTTP_HOST={host}\n")), "{info}");
        }
        let form = b"name=akit";
        let got = post("/post.php", "application/x-www-form-urlencoded", form);
        assert!(
            got.text().starts_with("akit|9|"),
            "{proto}: a form: {}",
            got.text()
        );
        let upload = b"0123456789".repeat(300_000); // 3 MB
        let got = post("/post.php", "application/octet-stream", &upload);
        assert!(
            got.text().starts_with("|3000000|"),
            "{proto}: an upload: {}",
            got.text()
        );
        let got = get("/status.php", &[]);
        assert_eq!(
            (got.status, got.one("x-teapot"), got.text().as_str()),
            (418, Some("yes"), "teapot"),
            "{proto}"
        );
        assert!(
            got.one("set-cookie").is_some_and(|c| c.starts_with("a=b")),
            "{proto}"
        );
        let got = get("/redirect.php", &[]);
        assert_eq!(
            (got.status, got.one("location")),
            (302, Some("/hello.php")),
            "{proto}"
        );
        assert_eq!(get("/big.php", &[]).body.len(), 4 << 20, "{proto}: big");
        assert_eq!(
            get("/blog/post?id=7", &[]).text(),
            "front /blog/post?id=7",
            "{proto}"
        );
    }
    drop(s);
    drop(fpm);
    let _ = std::fs::remove_dir_all(&root);
}

/// A streamed body under, at and over the listener's `max_body_bytes`, over HTTP/1.1 (chunked)
/// and h2c (no length): akio bounds it (`Context::limit_body`), and past it the handler answers
/// 413 and gives the application's request up; HTTP/1 closes the connection rather than read
/// the rest.
#[test]
fn streamed_body_limit() {
    let a = app();
    let s = serve_top(
        r#""limits": {"max_body_bytes": 1024},"#,
        &format!(r#"{{"address": "{}", "root": "/srv"}}"#, a.addr()),
    );
    let body = b"x".repeat(4096);
    let chunked = |n: usize| {
        let mut b = format!("{n:x}\r\n").into_bytes();
        b.extend_from_slice(&body[..n]);
        b.extend_from_slice(b"\r\n0\r\n\r\n");
        b
    };
    for n in [1000, 1024] {
        let piece = &body[..n];
        let got = h1(
            s.addr,
            "POST /echo.php HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked",
            &chunked(n),
        );
        assert_eq!(got.status, 200, "http/1.1 {n}");
        assert_eq!(got.text(), format!("{n} {}", digest(piece)), "http/1.1 {n}");
        let got = h2(s.addr, Method::Post, "/echo.php", &[], piece);
        assert_eq!(got.text(), format!("{n} {}", digest(piece)), "h2c {n}");
    }
    let mut c = TcpStream::connect(s.addr).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut req =
        b"POST /echo.php HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    req.extend_from_slice(&chunked(2048));
    c.write_all(&req).unwrap();
    let mut r = BufReader::new(c);
    let got = h1_response(&mut r, false);
    assert_eq!(got.status, 413, "http/1.1 over");
    let mut rest = Vec::new();
    let _ = r.read_to_end(&mut rest);
    assert!(rest.is_empty(), "http/1.1 over: the connection stays");
    let got = h2(s.addr, Method::Post, "/echo.php", &[], &body[..2048]);
    assert_eq!(got.status, 413, "h2c over");
}
