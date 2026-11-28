//! The proxy conformance suite (I25): what RFC 9110, RFC 9112 and RFC 9113 ask of an
//! intermediary, checked through the `akit` binary in front of an HTTP/1.1 backend that records
//! what reached it.
//!
//! The HTTP/1.1 half is fit's `test/e2e/conformance_test.go` (`TestProxyConformance`), ported
//! case by case, with requests written byte by byte. The HTTP/2 half sends h2c frames written by
//! hand (prior knowledge), so that a request can be malformed in the ways RFC 9113 section 8
//! names, and checks that such a request never reaches the upstream, that what reaches it is
//! HTTP/1.1 (cookie crumbs joined, `:authority` as `Host`), and that what comes back is HTTP/2
//! (no connection-specific fields, no body where there is none, the window honoured).
//!
//! fit's other proxy conformance points (header forwarding, streamed bodies both ways, trailers,
//! timeouts and client aborts) are akit-proxy's own tests over HTTP/1.1 and h2c: `proxy.rs`,
//! `h2.rs`, `stream.rs` and `failure.rs` (the M1 report lists them).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use akio_http::hpack::Decoder;

/// How long any one read may wait: a hang fails the test (rule 31).
const WAIT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------------------------
// The backend: answers each request by its path and records the requests that reached it.

/// A request's fields, in order.
type Fields = Vec<(String, String)>;

/// What reached the backend: "METHOD URI body", and each request's fields.
#[derive(Clone, Default)]
struct Recorder {
    seen: Arc<Mutex<Vec<(String, Fields)>>>,
}

impl Recorder {
    fn requests(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|(r, _)| r.clone())
            .collect()
    }

    /// The values of field `name` in the request whose line starts with `prefix`.
    fn field(&self, prefix: &str, name: &str) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(r, _)| r.starts_with(prefix))
            .flat_map(|(_, f)| {
                f.iter()
                    .filter(|(n, _)| n.eq_ignore_ascii_case(name))
                    .map(|(_, v)| v.clone())
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

fn start_recorder() -> (Recorder, SocketAddr) {
    let rec = Recorder::default();
    let ln = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = ln.local_addr().unwrap();
    let r = rec.clone();
    std::thread::spawn(move || {
        for conn in ln.incoming() {
            let Ok(conn) = conn else { return };
            let r = r.clone();
            std::thread::spawn(move || serve(&r, conn));
        }
    });
    (rec, addr)
}

/// One request read off `br`: its method, target, fields and body; `None` when the connection
/// ends or what comes is not a whole request.
fn read_request(br: &mut impl BufRead) -> Option<(String, String, Fields, Vec<u8>)> {
    let mut line = String::new();
    if br.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.trim_end().splitn(3, ' ');
    let method = parts.next()?.to_owned();
    let target = parts.next()?.to_owned();
    let mut fields = Vec::new();
    loop {
        let mut l = String::new();
        if br.read_line(&mut l).ok()? == 0 {
            return None;
        }
        let l = l.trim_end_matches(['\r', '\n']);
        if l.is_empty() {
            break;
        }
        let (n, v) = l.split_once(':')?;
        fields.push((n.to_owned(), v.trim().to_owned()));
    }
    let get = |name: &str| {
        fields
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    let mut body = Vec::new();
    if get("transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        loop {
            let mut size = String::new();
            br.read_line(&mut size).ok()?;
            let size = usize::from_str_radix(size.trim().split(';').next()?, 16).ok()?;
            if size == 0 {
                // Trailers, then the blank line.
                loop {
                    let mut t = String::new();
                    if br.read_line(&mut t).ok()? == 0 {
                        return None;
                    }
                    if t == "\r\n" {
                        break;
                    }
                }
                break;
            }
            let start = body.len();
            body.resize(start + size + 2, 0);
            br.read_exact(&mut body[start..]).ok()?;
            body.truncate(start + size);
        }
    } else if let Some(n) = get("content-length") {
        body.resize(n.parse().ok()?, 0);
        br.read_exact(&mut body).ok()?;
    }
    Some((method, target, fields, body))
}

/// Answers requests on one connection: raw, so that the backend can send what a well-behaved
/// server library would not.
fn serve(rec: &Recorder, conn: TcpStream) {
    conn.set_read_timeout(Some(WAIT)).unwrap();
    let mut br = BufReader::new(conn.try_clone().unwrap());
    let mut conn = conn;
    while let Some((method, target, fields, body)) = read_request(&mut br) {
        let body = String::from_utf8_lossy(&body).into_owned();
        rec.seen
            .lock()
            .unwrap()
            .push((format!("{method} {target} {body}"), fields.clone()));
        let path = target.split('?').next().unwrap_or("");
        let out = match path {
            "/garbage" => "this is not HTTP\r\n\r\n".to_owned(),
            "/304" => "HTTP/1.1 304 Not Modified\r\nEtag: \"v1\"\r\n\r\n".to_owned(),
            "/204" => "HTTP/1.1 204 No Content\r\nX-Kind: empty\r\n\r\n".to_owned(),
            "/103" => "HTTP/1.1 103 Early Hints\r\nLink: </a.css>; rel=preload\r\n\r\n\
                       HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nafter"
                .to_owned(),
            "/close" => {
                "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 6\r\n\r\nclosed".to_owned()
            }
            "/big" => {
                let chunk = "b".repeat(32 << 10);
                let mut out = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_owned();
                for _ in 0..4 {
                    out += &format!("{:x}\r\n{chunk}\r\n", chunk.len());
                }
                out + "0\r\n\r\n"
            }
            "/fold" => {
                let x = fields
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case("x-a"))
                    .map_or("", |(_, v)| v.as_str());
                let reply = format!("x-a={x}");
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{reply}",
                    reply.len()
                )
            }
            // Connection-specific fields, which HTTP/2 must not carry (RFC 9113 section 8.2.2).
            "/hop" => "HTTP/1.1 200 OK\r\nConnection: x-drop\r\nX-Drop: 1\r\n\
                       Keep-Alive: timeout=5\r\nProxy-Connection: keep-alive\r\n\
                       X-Kept: yes\r\nContent-Length: 3\r\n\r\nhop"
                .to_owned(),
            _ => {
                let reply = format!("echo {method} {target} {body}");
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{reply}",
                    reply.len()
                )
            }
        };
        if conn.write_all(out.as_bytes()).is_err() || path == "/garbage" || path == "/close" {
            return;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The binary.

/// The running binary, killed when dropped.
struct Akit(Child);

impl Drop for Akit {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// akit with one listener (HTTP/1.1 and h2c) proxying every request to a recorder, as fit's
/// test configures fit.
fn start() -> (Akit, SocketAddr, Recorder) {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let (rec, up) = start_recorder();
    let json = format!(
        r#"{{
  "listeners": [{{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}}],
  "upstreams": [{{"name": "app", "addresses": ["{up}"], "max_fails": 0}}],
  "routes": [{{"handler": {{"module": "proxy", "params": {{"upstream": "app"}}}}}}]
}}"#
    );
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("conformance-{}-{n}.json", std::process::id()));
    std::fs::write(&path, json).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["run", "-c"])
        .arg(&path)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let akit = Akit(child);
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line).unwrap();
    assert!(line.starts_with("akit: listener \"web\" on "), "{line:?}");
    let addr = line.split(' ').nth(4).unwrap().parse().unwrap();
    let _ = std::fs::remove_file(&path);
    (akit, addr, rec)
}

// ---------------------------------------------------------------------------------------------
// HTTP/1.1: fit's TestProxyConformance.

/// Writes `raw` to akit and reads back what it answers until the connection is quiet for a
/// moment or closes.
fn exchange(addr: SocketAddr, raw: &[u8]) -> String {
    let mut conn = TcpStream::connect(addr).unwrap();
    // Before the write: akit may refuse `raw` and close before it reads all of it, and macOS
    // refuses a socket's options (`EINVAL`) once its peer has reset it.
    conn.set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    conn.write_all(raw).unwrap();
    let mut out = Vec::new();
    let mut buf = vec![0; 64 << 10];
    let end = Instant::now() + WAIT;
    while Instant::now() < end {
        match conn.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether akit closes the connection after answering `raw`.
fn closes(addr: SocketAddr, raw: &str) -> bool {
    let mut conn = TcpStream::connect(addr).unwrap();
    // Before the write: akit may answer and close before it reads all of `raw`, and macOS
    // refuses a socket's options (`EINVAL`) once its peer has reset it.
    conn.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    conn.write_all(raw.as_bytes()).unwrap();
    let mut br = BufReader::new(conn);
    let mut head = String::new();
    loop {
        let mut l = String::new();
        assert!(br.read_line(&mut l).unwrap() > 0, "no response: {head:?}");
        head += &l;
        if l == "\r\n" {
            break;
        }
    }
    let len: usize = head
        .lines()
        .find_map(|l| {
            let (n, v) = l.split_once(':')?;
            n.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse().unwrap())
        })
        .unwrap_or(0);
    let mut body = vec![0; len];
    br.read_exact(&mut body).unwrap();
    br.get_ref()
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    matches!(br.read(&mut [0; 1]), Ok(0))
}

fn status_line(resp: &str) -> &str {
    resp.split("\r\n").next().unwrap_or("")
}

#[test]
fn h1_smuggling_with_content_length_and_transfer_encoding() {
    let (_akit, addr, rec) = start();
    let resp = exchange(
        addr,
        b"POST /a HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\n\r\n\
          0\r\n\r\nGET /smuggled HTTP/1.1\r\nHost: x\r\n\r\n",
    );
    for r in rec.requests() {
        assert!(
            !r.contains("/smuggled"),
            "a smuggled request reached the upstream: {r:?} (akit answered {:?})",
            status_line(&resp)
        );
    }
    assert!(
        resp.starts_with("HTTP/1.1 400") || resp.starts_with("HTTP/1.1 200"),
        "answered {:?}",
        status_line(&resp)
    );
}

#[test]
fn h1_bad_requests_get_400_and_reach_nothing() {
    let (_akit, addr, rec) = start();
    let mut failed = Vec::new();
    for (name, raw) in [
        (
            "conflicting Content-Length",
            &b"POST /a HTTP/1.1\r\nHost: x\r\nContent-Length: 3\r\nContent-Length: 4\r\n\r\nabcd"[..],
        ),
        (
            "space before the colon",
            b"GET /a HTTP/1.1\r\nHost: x\r\nX-A : one\r\n\r\n",
        ),
        ("no Host in HTTP/1.1", b"GET /a HTTP/1.1\r\n\r\n"),
        (
            "two Hosts",
            b"GET /a HTTP/1.1\r\nHost: x\r\nHost: y\r\n\r\n",
        ),
        (
            "a NUL in a header",
            b"GET /a HTTP/1.1\r\nHost: x\r\nX-A: a\x00b\r\n\r\n",
        ),
    ] {
        let before = rec.seen.lock().unwrap().len();
        let resp = exchange(addr, raw);
        let seen = rec.seen.lock().unwrap()[before..].to_vec();
        if !resp.starts_with("HTTP/1.1 400") || !seen.is_empty() {
            failed.push(format!(
                "{name}: answered {:?}, want 400; the upstream saw {seen:?}",
                status_line(&resp)
            ));
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}

#[test]
fn h1_a_bad_chunk_size() {
    // fit answers 400. akit streams a proxied body, so its request is under way when the bad
    // size comes: akio closes the connection then, without a response (RFC 9112 section 2.2:
    // "SHOULD respond with a 400"; akio API request A169 asks for it). Either way nothing
    // reaches the upstream as a request it could answer.
    let (_akit, addr, rec) = start();
    let resp = exchange(
        addr,
        b"POST /a HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\nabc\r\n0\r\n\r\n",
    );
    assert!(
        resp.is_empty() || resp.starts_with("HTTP/1.1 400"),
        "answered {:?}",
        status_line(&resp)
    );
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        rec.requests().is_empty(),
        "the upstream saw {:?}",
        rec.requests()
    );
}

#[test]
fn h1_obs_fold() {
    // RFC 9112 section 5.2: refused, or each fold replaced by a space.
    let (_akit, addr, _rec) = start();
    let resp = exchange(
        addr,
        b"GET /fold HTTP/1.1\r\nHost: x\r\nX-A: one\r\n two\r\n\r\n",
    );
    assert!(
        resp.starts_with("HTTP/1.1 400") || resp.contains("x-a=one two"),
        "answered {resp:?}"
    );
}

#[test]
fn h1_a_header_past_the_limit() {
    let (_akit, addr, _rec) = start();
    let raw = format!(
        "GET /a HTTP/1.1\r\nHost: x\r\nX-Big: {}\r\n\r\n",
        "a".repeat(2 << 20)
    );
    // akit may close before reading it all: the write may fail.
    let mut conn = TcpStream::connect(addr).unwrap();
    conn.set_read_timeout(Some(WAIT)).unwrap();
    let _ = conn.write_all(raw.as_bytes());
    let mut resp = Vec::new();
    let _ = conn.read_to_end(&mut resp);
    let resp = String::from_utf8_lossy(&resp);
    assert!(
        resp.starts_with("HTTP/1.1 431"),
        "answered {:?}, want 431",
        status_line(&resp)
    );
}

#[test]
fn h1_expect_100_continue() {
    let (_akit, addr, _rec) = start();
    let mut conn = TcpStream::connect(addr).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let body = "e".repeat(100 << 10); // past any buffering threshold
    write!(
        conn,
        "PUT /expect HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nExpect: 100-continue\r\n\r\n",
        body.len()
    )
    .unwrap();
    let mut br = BufReader::new(conn.try_clone().unwrap());
    let mut line = String::new();
    br.read_line(&mut line).unwrap();
    assert!(
        line.starts_with("HTTP/1.1 100"),
        "before the body: {line:?}"
    );
    while line != "\r\n" {
        line.clear();
        br.read_line(&mut line).unwrap();
    }
    conn.write_all(body.as_bytes()).unwrap();
    let (head, got) = read_response(&mut br);
    assert!(head.starts_with("HTTP/1.1 200"), "after the body: {head:?}");
    let want = format!("echo PUT /expect {body}");
    assert!(
        got == want.as_bytes(),
        "after the body: {} bytes",
        got.len()
    );
}

/// A response's head and its body, framed by its length or chunked.
fn read_response(br: &mut impl BufRead) -> (String, Vec<u8>) {
    let mut head = String::new();
    loop {
        let mut l = String::new();
        assert!(br.read_line(&mut l).unwrap() > 0, "cut short: {head:?}");
        head += &l;
        if l == "\r\n" {
            break;
        }
    }
    let field = |name: &str| {
        head.lines().find_map(|l| {
            let (n, v) = l.split_once(':')?;
            n.eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
        })
    };
    let mut body = Vec::new();
    if field("transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        loop {
            let mut size = String::new();
            br.read_line(&mut size).unwrap();
            let n = usize::from_str_radix(size.trim(), 16).unwrap();
            let mut chunk = vec![0; n + 2];
            br.read_exact(&mut chunk).unwrap();
            body.extend_from_slice(&chunk[..n]);
            if n == 0 {
                break;
            }
        }
    } else if let Some(n) = field("content-length") {
        body.resize(n.parse().unwrap(), 0);
        br.read_exact(&mut body).unwrap();
    }
    (head, body)
}

#[test]
fn h1_an_http10_client() {
    let (_akit, addr, _rec) = start();
    let resp = exchange(addr, b"GET /big HTTP/1.0\r\nHost: x\r\n\r\n");
    let (head, body) = resp.split_once("\r\n\r\n").unwrap();
    assert!(
        head.starts_with("HTTP/1.")
            && !head
                .to_ascii_lowercase()
                .contains("transfer-encoding: chunked")
            && body.len() == 128 << 10,
        "{head:?}, {} bytes of body",
        body.len()
    );
}

#[test]
fn h1_pipelined_requests() {
    let (_akit, addr, _rec) = start();
    let resp = exchange(
        addr,
        b"GET /one HTTP/1.1\r\nHost: x\r\n\r\nGET /two HTTP/1.1\r\nHost: x\r\n\r\n\
          GET /three HTTP/1.1\r\nHost: x\r\n\r\n",
    );
    let (i, j, k) = (
        resp.find("echo GET /one"),
        resp.find("echo GET /two"),
        resp.find("echo GET /three"),
    );
    assert!(
        i.is_some() && j > i && k > j,
        "responses out of order or missing: {resp:?}"
    );
}

#[test]
fn h1_absolute_form_target() {
    let (_akit, addr, _rec) = start();
    let resp = exchange(addr, b"GET http://x/abs?q=1 HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(
        resp.contains("echo GET /abs?q=1 "),
        "the upstream saw {resp:?}"
    );
}

#[test]
fn h1_304_and_204_from_the_upstream() {
    let (_akit, addr, _rec) = start();
    for (path, status, field) in [
        ("/304", "HTTP/1.1 304", "Etag: \"v1\""),
        ("/204", "HTTP/1.1 204", "X-Kind: empty"),
    ] {
        let resp = exchange(
            addr,
            format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\nGET /after HTTP/1.1\r\nHost: x\r\n\r\n")
                .as_bytes(),
        );
        assert!(
            resp.starts_with(status)
                && resp
                    .to_ascii_lowercase()
                    .contains(&field.to_ascii_lowercase())
                && resp.contains("echo GET /after"),
            "{path}: {resp:?}"
        );
    }
}

#[test]
fn h1_1xx_from_the_upstream() {
    let (_akit, addr, _rec) = start();
    let resp = exchange(addr, b"GET /103 HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(
        resp.contains("HTTP/1.1 200") && resp.ends_with("after"),
        "{resp:?}"
    );
}

#[test]
fn h1_an_upstream_that_does_not_speak_http() {
    let (_akit, addr, _rec) = start();
    let resp = exchange(addr, b"GET /garbage HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(
        resp.starts_with("HTTP/1.1 502"),
        "answered {:?}, want 502",
        status_line(&resp)
    );
}

#[test]
fn h1_connection_close_from_the_client() {
    let (_akit, addr, _rec) = start();
    assert!(
        closes(
            addr,
            "GET /a HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
        ),
        "the connection stayed open"
    );
}

#[test]
fn h1_connection_close_from_the_upstream_is_the_upstreams() {
    let (_akit, addr, _rec) = start();
    let resp = exchange(
        addr,
        b"GET /close HTTP/1.1\r\nHost: x\r\n\r\nGET /after HTTP/1.1\r\nHost: x\r\n\r\n",
    );
    assert!(
        resp.contains("closed")
            && resp.contains("echo GET /after")
            && !resp.to_ascii_lowercase().contains("connection: close"),
        "{resp:?}"
    );
}

#[test]
fn h1_connection_specific_fields_from_the_upstream_are_dropped() {
    let (_akit, addr, _rec) = start();
    let resp = exchange(addr, b"GET /hop HTTP/1.1\r\nHost: x\r\n\r\n");
    let head = resp.split("\r\n\r\n").next().unwrap().to_ascii_lowercase();
    assert!(head.starts_with("http/1.1 200"), "{resp:?}");
    assert!(head.contains("x-kept: yes"), "{resp:?}");
    for name in [
        "x-drop:",
        "keep-alive:",
        "proxy-connection:",
        "connection: x-drop",
    ] {
        assert!(!head.contains(name), "{name} passed: {resp:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// HTTP/2: a minimal h2c client, prior knowledge, frames written by hand.

const DATA: u8 = 0;
const HEADERS: u8 = 1;
const RST_STREAM: u8 = 3;
const SETTINGS: u8 = 4;
const PING: u8 = 6;
const GOAWAY: u8 = 7;
const WINDOW_UPDATE: u8 = 8;
const CONTINUATION: u8 = 9;
const END_STREAM: u8 = 1;
const END_HEADERS: u8 = 4;
const PADDED: u8 = 8;
const PRIORITY: u8 = 0x20;
const ACK: u8 = 1;

/// An HPACK string literal, not Huffman-coded.
fn hpack_str(out: &mut Vec<u8>, s: &[u8]) {
    hpack_int(out, 0, 7, s.len());
    out.extend_from_slice(s);
}

/// An HPACK integer with an `n`-bit prefix, after the bits in `first`.
fn hpack_int(out: &mut Vec<u8>, first: u8, n: u32, mut v: usize) {
    let max = (1usize << n) - 1;
    if v < max {
        out.push(first | u8::try_from(v).unwrap());
        return;
    }
    out.push(first | u8::try_from(max).unwrap());
    v -= max;
    while v >= 128 {
        out.push(u8::try_from(v % 128).unwrap() | 0x80);
        v /= 128;
    }
    out.push(u8::try_from(v).unwrap());
}

/// A header block of literals without indexing, in the order given, whatever they are.
fn block(fields: &[(&[u8], &[u8])]) -> Vec<u8> {
    let mut b = Vec::new();
    for (n, v) in fields {
        b.push(0);
        hpack_str(&mut b, n);
        hpack_str(&mut b, v);
    }
    b
}

/// How a stream ended, as the client saw it.
#[derive(Debug, Default)]
struct Answer {
    /// The final response's fields, pseudo-fields included, lowercase as HTTP/2 sends them.
    fields: Vec<(String, String)>,
    /// Informational responses' statuses, in order.
    informational: Vec<String>,
    body: Vec<u8>,
    /// The error code of a RST_STREAM on the stream, or of a GOAWAY.
    reset: Option<u32>,
}

impl Answer {
    fn status(&self) -> Option<&str> {
        self.field(":status")
    }

    fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// Whether the request was refused: reset, or answered 400.
    fn refused(&self) -> bool {
        self.reset.is_some() || self.status() == Some("400")
    }
}

struct H2 {
    s: TcpStream,
    dec: Decoder,
}

impl H2 {
    fn connect(addr: SocketAddr) -> H2 {
        let s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(WAIT)).unwrap();
        s.set_nodelay(true).unwrap();
        let mut h = H2 {
            s,
            dec: Decoder::new(),
        };
        h.s.write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n").unwrap();
        h.frame(SETTINGS, 0, 0, &[]);
        h
    }

    fn frame(&mut self, kind: u8, flags: u8, stream: u32, payload: &[u8]) {
        let len = payload.len();
        let mut f = len.to_be_bytes()[size_of::<usize>() - 3..].to_vec();
        f.extend_from_slice(&[kind, flags]);
        f.extend_from_slice(&stream.to_be_bytes());
        f.extend_from_slice(payload);
        // A server that refused the stream may have closed the connection already.
        let _ = self.s.write_all(&f);
    }

    /// The usual pseudo-fields for `method` and `path`, then `fields`.
    fn head<'a>(method: &'a str, path: &'a str, fields: &[(&'a str, &'a str)]) -> Vec<u8> {
        let mut all: Vec<(&[u8], &[u8])> = vec![
            (b":method", method.as_bytes()),
            (b":scheme", b"http"),
            (b":path", path.as_bytes()),
            (b":authority", b"x"),
        ];
        all.extend(fields.iter().map(|(n, v)| (n.as_bytes(), v.as_bytes())));
        block(&all)
    }

    /// Sends a header block on `stream`, ending the stream with it when `end`.
    fn headers(&mut self, stream: u32, block: &[u8], end: bool) {
        let flags = END_HEADERS | if end { END_STREAM } else { 0 };
        self.frame(HEADERS, flags, stream, block);
    }

    /// Reads frames until `stream` ends: its last response frame, a RST_STREAM on it, a GOAWAY,
    /// or the connection closed. Answers the connection's own frames and gives back the window
    /// for every DATA frame it reads.
    fn answer(&mut self, stream: u32) -> Answer {
        let mut a = Answer::default();
        let mut pending: Option<(u32, u8, Vec<u8>)> = None;
        loop {
            let mut h = [0; 9];
            if self.s.read_exact(&mut h).is_err() {
                assert!(
                    a.reset.is_some() || !a.fields.is_empty(),
                    "the connection ended with nothing for stream {stream}"
                );
                return a;
            }
            let len = (usize::from(h[0]) << 16) | (usize::from(h[1]) << 8) | usize::from(h[2]);
            let (kind, flags) = (h[3], h[4]);
            let id = u32::from_be_bytes([h[5], h[6], h[7], h[8]]) & 0x7fff_ffff;
            let mut p = vec![0; len];
            self.s.read_exact(&mut p).unwrap();
            match kind {
                SETTINGS if flags & ACK == 0 => self.frame(SETTINGS, ACK, 0, &[]),
                PING if flags & ACK == 0 => self.frame(PING, ACK, 0, &p),
                GOAWAY => {
                    a.reset = Some(u32::from_be_bytes([p[4], p[5], p[6], p[7]]));
                    return a;
                }
                RST_STREAM if id == stream => {
                    a.reset = Some(u32::from_be_bytes([p[0], p[1], p[2], p[3]]));
                    return a;
                }
                HEADERS | CONTINUATION if id == stream => {
                    let mut frag = &p[..];
                    if kind == HEADERS {
                        if flags & PADDED != 0 {
                            let pad = usize::from(frag[0]);
                            frag = &frag[1..frag.len() - pad];
                        }
                        if flags & PRIORITY != 0 {
                            frag = &frag[5..];
                        }
                        pending = Some((id, flags, frag.to_vec()));
                    } else {
                        pending.as_mut().unwrap().2.extend_from_slice(frag);
                    }
                    if flags & END_HEADERS != 0 {
                        let (_, first, blk) = pending.take().unwrap();
                        let fields: Vec<(String, String)> = self
                            .dec
                            .decode(&blk)
                            .unwrap()
                            .into_iter()
                            .map(|(n, v)| {
                                (
                                    String::from_utf8(n).unwrap(),
                                    String::from_utf8_lossy(&v).into_owned(),
                                )
                            })
                            .collect();
                        let status = fields
                            .iter()
                            .find(|(n, _)| n == ":status")
                            .map(|(_, v)| v.clone());
                        match status {
                            Some(s) if s.starts_with('1') => a.informational.push(s),
                            // Trailers have no status.
                            None if !a.fields.is_empty() => {}
                            _ => a.fields = fields,
                        }
                        if first & END_STREAM != 0 {
                            return a;
                        }
                    }
                }
                DATA if id == stream => {
                    let mut data = &p[..];
                    if flags & PADDED != 0 {
                        let pad = usize::from(data[0]);
                        data = &data[1..data.len() - pad];
                    }
                    a.body.extend_from_slice(data);
                    if !p.is_empty() {
                        let n = u32::try_from(p.len()).unwrap().to_be_bytes();
                        self.frame(WINDOW_UPDATE, 0, 0, &n);
                        if flags & END_STREAM == 0 {
                            self.frame(WINDOW_UPDATE, 0, stream, &n);
                        }
                    }
                    if flags & END_STREAM != 0 {
                        return a;
                    }
                }
                _ => {}
            }
        }
    }
}

/// Whether `fields` (an HTTP/2 response's) hold a connection-specific field, which RFC 9113
/// section 8.2.2 forbids.
fn connection_specific(fields: &[(String, String)]) -> Option<&str> {
    fields.iter().map(|(n, _)| n.as_str()).find(|n| {
        matches!(
            *n,
            "connection" | "keep-alive" | "proxy-connection" | "transfer-encoding" | "upgrade"
        )
    })
}

#[test]
fn h2_malformed_requests_are_refused_and_reach_nothing() {
    let (_akit, addr, rec) = start();
    let pseudo: [(&[u8], &[u8]); 4] = [
        (b":method", b"GET"),
        (b":scheme", b"http"),
        (b":path", b"/m"),
        (b":authority", b"x"),
    ];
    let with = |extra: &[(&'static [u8], &'static [u8])]| {
        let mut all = pseudo.to_vec();
        all.extend_from_slice(extra);
        block(&all)
    };
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("connection", with(&[(b"connection", b"keep-alive")])),
        ("keep-alive", with(&[(b"keep-alive", b"timeout=5")])),
        (
            "proxy-connection",
            with(&[(b"proxy-connection", b"keep-alive")]),
        ),
        (
            "transfer-encoding",
            with(&[(b"transfer-encoding", b"chunked")]),
        ),
        ("upgrade", with(&[(b"upgrade", b"websocket")])),
        ("te other than trailers", with(&[(b"te", b"gzip")])),
        ("an uppercase field name", with(&[(b"X-Upper", b"1")])),
        ("a NUL in a value", with(&[(b"x-a", b"a\x00b")])),
        ("a CR in a value", with(&[(b"x-a", b"a\rb")])),
        ("a LF in a value", with(&[(b"x-a", b"a\nb")])),
        ("a space in a name", with(&[(b"x a", b"1")])),
        (
            "no :path",
            block(&[
                (b":method", b"GET"),
                (b":scheme", b"http"),
                (b":authority", b"x"),
            ]),
        ),
        (
            "no :method",
            block(&[
                (b":scheme", b"http"),
                (b":path", b"/m"),
                (b":authority", b"x"),
            ]),
        ),
        (
            "an empty :path",
            block(&[
                (b":method", b"GET"),
                (b":scheme", b"http"),
                (b":path", b""),
                (b":authority", b"x"),
            ]),
        ),
        (
            "two :path",
            block(&[
                (b":method", b"GET"),
                (b":scheme", b"http"),
                (b":path", b"/m"),
                (b":path", b"/n"),
                (b":authority", b"x"),
            ]),
        ),
        (
            "a pseudo-field after a field",
            block(&[
                (b":method", b"GET"),
                (b":scheme", b"http"),
                (b"x-a", b"1"),
                (b":path", b"/m"),
                (b":authority", b"x"),
            ]),
        ),
        ("an unknown pseudo-field", with(&[(b":unknown", b"1")])),
        ("a response pseudo-field", with(&[(b":status", b"200")])),
    ];
    let mut failed = Vec::new();
    for (name, blk) in cases {
        let before = rec.seen.lock().unwrap().len();
        let mut c = H2::connect(addr);
        c.headers(1, &blk, true);
        let a = c.answer(1);
        let seen = rec.seen.lock().unwrap()[before..].to_vec();
        if !a.refused() || !seen.is_empty() {
            failed.push(format!(
                "{name}: answered {:?}, the upstream saw {:?}",
                a.status(),
                seen
            ));
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}

#[test]
fn h2_content_length_that_disagrees_with_the_body() {
    let (_akit, addr, rec) = start();
    // RFC 9113 section 8.1.1: more or less DATA than content-length is malformed. Whatever the
    // proxy sent on, the upstream never gets a whole request out of it.
    for (name, len, body) in [("short", "5", &b"abc"[..]), ("long", "2", b"abc")] {
        let mut c = H2::connect(addr);
        c.headers(
            1,
            &H2::head("POST", &format!("/cl-{name}"), &[("content-length", len)]),
            false,
        );
        c.frame(DATA, END_STREAM, 1, body);
        let a = c.answer(1);
        assert!(a.refused(), "{name}: answered {a:?}");
    }
    // The upstream may have seen the head; it must not have seen a request it could answer.
    std::thread::sleep(Duration::from_millis(200));
    for r in rec.requests() {
        assert!(!r.starts_with("POST /cl-"), "the upstream saw {r:?}");
    }
}

#[test]
fn h2_to_http1_cookie_crumbs_are_joined_and_authority_is_host() {
    let (_akit, addr, rec) = start();
    let mut c = H2::connect(addr);
    c.headers(
        1,
        &H2::head(
            "GET",
            "/cookies?q=1",
            &[("cookie", "a=1"), ("cookie", "b=2"), ("x-a", "one")],
        ),
        true,
    );
    let a = c.answer(1);
    assert_eq!(a.status(), Some("200"), "{a:?}");
    assert_eq!(String::from_utf8_lossy(&a.body), "echo GET /cookies?q=1 ");
    // RFC 9113 section 8.2.3: joined with "; " into one field for HTTP/1.1.
    assert_eq!(rec.field("GET /cookies", "cookie"), ["a=1; b=2"]);
    assert_eq!(rec.field("GET /cookies", "host"), ["x"]);
    assert_eq!(rec.field("GET /cookies", "x-a"), ["one"]);
}

#[test]
fn h2_bodies_both_ways_past_the_initial_window() {
    let (_akit, addr, rec) = start();
    let mut c = H2::connect(addr);
    // A response of 128 KiB, past the 64 KiB initial window: the client gives it back as it reads.
    c.headers(1, &H2::head("GET", "/big", &[]), true);
    let a = c.answer(1);
    assert_eq!(a.status(), Some("200"));
    assert_eq!(a.body.len(), 128 << 10);
    assert!(a.body.iter().all(|&b| b == b'b'));
    assert_eq!(connection_specific(&a.fields), None, "{:?}", a.fields);
    // An upload in several DATA frames on the same connection, without a length: HTTP/1.1
    // chunked to the upstream, or framed by a length the proxy learned.
    c.headers(3, &H2::head("POST", "/up", &[]), false);
    let piece = vec![b'u'; 16 << 10];
    for i in 0..3 {
        c.frame(DATA, if i == 2 { END_STREAM } else { 0 }, 3, &piece);
    }
    let a = c.answer(3);
    assert_eq!(a.status(), Some("200"), "{a:?}");
    assert_eq!(a.body.len(), "echo POST /up ".len() + (48 << 10));
    assert!(rec.requests().iter().any(|r| r.starts_with("POST /up uuu")));
}

#[test]
fn h2_connection_specific_fields_from_the_upstream_are_dropped() {
    let (_akit, addr, _rec) = start();
    let mut c = H2::connect(addr);
    c.headers(1, &H2::head("GET", "/hop", &[]), true);
    let a = c.answer(1);
    assert_eq!(a.status(), Some("200"), "{a:?}");
    assert_eq!(a.body, b"hop");
    assert_eq!(a.field("x-kept"), Some("yes"));
    assert_eq!(
        a.field("x-drop"),
        None,
        "named by Connection: {:?}",
        a.fields
    );
    assert_eq!(connection_specific(&a.fields), None, "{:?}", a.fields);
    // The upstream's Connection: close is the upstream's: the h2 connection goes on.
    c.headers(3, &H2::head("GET", "/close", &[]), true);
    let a = c.answer(3);
    assert_eq!(
        (a.status(), a.body.as_slice()),
        (Some("200"), &b"closed"[..])
    );
    assert_eq!(connection_specific(&a.fields), None, "{:?}", a.fields);
    c.headers(5, &H2::head("GET", "/after", &[]), true);
    let a = c.answer(5);
    assert_eq!(String::from_utf8_lossy(&a.body), "echo GET /after ");
}

#[test]
fn h2_304_204_and_1xx_from_the_upstream() {
    let (_akit, addr, _rec) = start();
    let mut c = H2::connect(addr);
    c.headers(1, &H2::head("GET", "/304", &[]), true);
    let a = c.answer(1);
    assert_eq!(a.status(), Some("304"), "{a:?}");
    assert_eq!(a.field("etag"), Some("\"v1\""));
    assert!(a.body.is_empty(), "{a:?}");
    c.headers(3, &H2::head("GET", "/204", &[]), true);
    let a = c.answer(3);
    assert_eq!(a.status(), Some("204"), "{a:?}");
    assert_eq!(a.field("x-kind"), Some("empty"));
    assert!(a.body.is_empty(), "{a:?}");
    c.headers(5, &H2::head("GET", "/103", &[]), true);
    let a = c.answer(5);
    assert_eq!(
        (a.status(), a.body.as_slice()),
        (Some("200"), &b"after"[..])
    );
    // The connection is still usable after all three.
    c.headers(7, &H2::head("GET", "/after", &[]), true);
    let a = c.answer(7);
    assert_eq!(String::from_utf8_lossy(&a.body), "echo GET /after ");
}

#[test]
fn h2_an_upstream_that_does_not_speak_http() {
    let (_akit, addr, _rec) = start();
    let mut c = H2::connect(addr);
    c.headers(1, &H2::head("GET", "/garbage", &[]), true);
    let a = c.answer(1);
    assert_eq!(a.status(), Some("502"), "{a:?}");
}

#[test]
fn h2_expect_100_continue() {
    let (_akit, addr, _rec) = start();
    let mut c = H2::connect(addr);
    // 60 KiB: within the 64 KiB windows a client may assume before the server's settings.
    let body = vec![b'e'; 60 << 10];
    let len = body.len().to_string();
    c.headers(
        1,
        &H2::head(
            "PUT",
            "/expect",
            &[("content-length", &len), ("expect", "100-continue")],
        ),
        false,
    );
    for (i, piece) in body.chunks(16 << 10).enumerate() {
        let end = if (i + 1) * (16 << 10) >= body.len() {
            END_STREAM
        } else {
            0
        };
        c.frame(DATA, end, 1, piece);
    }
    let a = c.answer(1);
    assert_eq!(a.status(), Some("200"), "{a:?}");
    assert_eq!(a.body.len(), "echo PUT /expect ".len() + body.len());
    assert!(a.informational.iter().all(|s| s == "100"), "{a:?}");
    let _ = c.s.shutdown(Shutdown::Both);
}
