//! fit's stream tests for its proxy (`handler/proxy/stream_test.go`), ported: a download to a
//! client slower than its upstream, and an upload to an upstream slower than its client, each
//! framed by its length and chunked, over HTTP/1.1 and h2c, with the bytes checked against a
//! pattern and the memory's rise while they move bounded far below their size: neither side's
//! body piles up in akit (I16, linked backpressure). The same against an h2c upstream (I18b, as
//! fit runs its stream tests for each upstream protocol): a held download keeps its stream's
//! window closed, and an upload is paced by what waits in its stream. On the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
//!
//! fit samples Go's live heap; here the process's resident memory is sampled (Linux, where the
//! bound is checked; akit has no `unsafe` for a counting allocator, rule 14). The clients and the
//! upstream are plain threads on std sockets whose buffers are small; the h2c client is a minimal
//! one of its own, so that it can read as slowly as a test wants and give its window back only
//! as it does.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, UringConfig};
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};

/// How much each test moves, and how far the resident memory may rise meanwhile: fit's sizes for
/// its race builds, a quarter of what moves.
const SIZE: usize = 64 << 20;
const MAX_RISE: usize = 16 << 20;

/// The tests one at a time: the memory is the process's.
static SERIAL: Mutex<()> = Mutex::new(());

/// The process's resident memory, sampled every few milliseconds while a body moves, and how far
/// it rose above where it started. A body held whole would show there. Linux only (`/proc`);
/// elsewhere it reads nothing, and the bodies are checked without it.
struct Watch {
    base: usize,
    peak: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    sampler: Option<thread::JoinHandle<()>>,
}

/// Resident bytes, from `/proc/self/statm` (its second field, in pages of 4 KiB).
fn resident() -> Option<usize> {
    let s = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: usize = s.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4096)
}

impl Watch {
    fn new() -> Watch {
        let base = resident().unwrap_or(0);
        let peak = Arc::new(AtomicUsize::new(base));
        let stop = Arc::new(AtomicBool::new(false));
        let sampler = resident().is_some().then(|| {
            let (peak, stop) = (peak.clone(), stop.clone());
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    peak.fetch_max(resident().unwrap_or(0), Ordering::Relaxed);
                    thread::sleep(Duration::from_millis(5));
                }
            })
        });
        Watch {
            base,
            peak,
            stop,
            sampler,
        }
    }

    /// Stops sampling: the rise, or `None` where nothing was sampled.
    fn rise(mut self) -> Option<usize> {
        self.stop.store(true, Ordering::Relaxed);
        let sampler = self.sampler.take()?;
        sampler.join().unwrap();
        Some(self.peak.load(Ordering::Relaxed).saturating_sub(self.base))
    }
}

/// The body the tests move: a cycle of a prime length, so that a byte lost, repeated or moved
/// shows.
fn pattern() -> &'static [u8] {
    static P: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    P.get_or_init(|| (0..4093usize).map(|i| (i * 7 + i / 251) as u8).collect())
}

/// Fills `buf` with the pattern from offset `off`.
fn fill(buf: &mut [u8], off: usize) {
    let p = pattern();
    for (i, b) in buf.iter_mut().enumerate() {
        *b = p[(off + i) % p.len()];
    }
}

/// Checks the pattern in `data` at offset `off`.
fn check(data: &[u8], off: usize) {
    let p = pattern();
    for (i, b) in data.iter().enumerate() {
        assert_eq!(*b, p[(off + i) % p.len()], "byte {} differs", off + i);
    }
}

/// Reads as a slow peer does: a pause after every 4 MiB.
struct Pace {
    since: usize,
}

impl Pace {
    fn took(&mut self, n: usize) {
        self.since += n;
        if self.since >= 4 << 20 {
            self.since = 0;
            thread::sleep(Duration::from_millis(20));
        }
    }
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

/// akit proxying to the upstream at `up`, on HTTP/1.1 and h2c, as `akit run` serves it.
struct Akit {
    engine: Arc<Engine>,
    addr: SocketAddr,
    _cell: GraphCell,
}

impl Akit {
    fn start(up: SocketAddr) -> Akit {
        Akit::start_with(up, "http/1.1")
    }

    /// akit proxying to the upstream at `up`, spoken to over `protocol`.
    fn start_with(up: SocketAddr, protocol: &str) -> Akit {
        let json = format!(
            r#"{{
  "listeners": [{{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}}],
  "limits": {{"max_body_bytes": {}}},
  "upstreams": [{{"name": "app", "addresses": ["{up}"], "protocol": "{protocol}"}}],
  "routes": [{{"handler": {{"module": "proxy", "params": {{"upstream": "app"}}}}}}]
}}"#,
            SIZE * 2
        );
        let config = akit_config::from_json(&json, None).unwrap();
        let registry = Registry::<HandlerParams>::new(&[akit_proxy::MODULES]).unwrap();
        let slot = EngineSlot::new();
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

/// Reads a body framed as `head` says, handing each piece to `f`; its length.
fn read_body(r: &mut impl BufRead, head: &str, mut f: impl FnMut(&[u8], usize)) -> usize {
    let mut buf = vec![0; 32 << 10];
    let mut off = 0;
    if field(head, "transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        loop {
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            let mut left = usize::from_str_radix(line.trim(), 16).unwrap();
            if left == 0 {
                // No trailers.
                r.read_line(&mut line).unwrap();
                return off;
            }
            while left > 0 {
                let want = left.min(buf.len());
                let n = r.read(&mut buf[..want]).unwrap();
                assert!(n > 0, "the body ended at {off}");
                f(&buf[..n], off);
                off += n;
                left -= n;
            }
            r.read_exact(&mut [0; 2]).unwrap();
        }
    }
    let mut left: usize = field(head, "content-length").unwrap().parse().unwrap();
    while left > 0 {
        let want = left.min(buf.len());
        let n = r.read(&mut buf[..want]).unwrap();
        assert!(n > 0, "the body ended at {off}");
        f(&buf[..n], off);
        off += n;
        left -= n;
    }
    off
}

/// The upstream: on its own threads, an HTTP/1.1 server that sends `GET /big` the pattern as
/// fast as it can (by its length with `?length=1`, else chunked), and reads the body of a `PUT`
/// slowly, checking it, and answers with its length, its `Content-Length` (-1 for none) and its
/// `Transfer-Encoding`.
fn upstream() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            thread::spawn(move || serve(s));
        }
    });
    addr
}

fn serve(s: TcpStream) {
    let mut w = s.try_clone().unwrap();
    let mut r = BufReader::new(s);
    while let Some(head) = read_head(&mut r) {
        let line = head.lines().next().unwrap_or("").to_owned();
        if line.starts_with("GET /broken") {
            // A length it does not keep: 1 MiB of it, then the connection ends.
            let mut buf = vec![0; 1 << 20];
            fill(&mut buf, 0);
            let _ = write!(w, "HTTP/1.1 200 OK\r\nContent-Length: {SIZE}\r\n\r\n")
                .and_then(|()| w.write_all(&buf));
            return;
        }
        if line.starts_with("GET /big") {
            let length = line.contains("length=1");
            let framing = if length {
                format!("Content-Length: {SIZE}")
            } else {
                "Transfer-Encoding: chunked".to_owned()
            };
            if write!(w, "HTTP/1.1 200 OK\r\n{framing}\r\n\r\n").is_err() {
                return;
            }
            let mut buf = vec![0; 64 << 10];
            let mut off = 0;
            while off < SIZE {
                let n = buf.len().min(SIZE - off);
                fill(&mut buf[..n], off);
                let sent = if length {
                    w.write_all(&buf[..n])
                } else {
                    write!(w, "{n:x}\r\n")
                        .and_then(|()| w.write_all(&buf[..n]))
                        .and_then(|()| w.write_all(b"\r\n"))
                };
                if sent.is_err() {
                    return;
                }
                off += n;
            }
            if !length && w.write_all(b"0\r\n\r\n").is_err() {
                return;
            }
        } else {
            let cl = field(&head, "content-length").map_or("-1".to_owned(), str::to_owned);
            let te = field(&head, "transfer-encoding").unwrap_or("").to_owned();
            let mut pace = Pace { since: 0 };
            let n = read_body(&mut r, &head, |data, off| {
                check(data, off);
                pace.took(data.len());
            });
            let body = format!("{n} {cl} {te}");
            if write!(
                w,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .is_err()
            {
                return;
            }
        }
    }
}

/// The upstream over h2c with prior knowledge, as [`upstream`] over HTTP/1.1: `GET /big` sends
/// the pattern as fast as akit's windows let it (with its length with `?length=1`), and the body
/// of a `PUT` is read slowly, its window given back only as it is read, and answered with its
/// length and its `content-length` (-1 for none). One stream at a time per connection.
fn h2_upstream() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            thread::spawn(move || {
                // A connection akit closed ends its thread.
                let _ = std::panic::catch_unwind(move || H2Up::serve(s));
            });
        }
    });
    addr
}

/// akit closed the connection: its thread ends, quietly (no panic message).
fn gone() -> ! {
    std::panic::resume_unwind(Box::new("connection closed"))
}

/// One h2c connection of [`h2_upstream`].
struct H2Up {
    s: TcpStream,
    dec: akio_http::hpack::Decoder,
    send_conn: i64,
    /// The current stream's send window, and the initial one akit's SETTINGS give.
    send_stream: i64,
    initial: i64,
    sid: u32,
}

impl H2Up {
    fn serve(s: TcpStream) {
        s.set_nodelay(true).unwrap();
        let mut h = H2Up {
            s,
            dec: akio_http::hpack::Decoder::new(),
            send_conn: 65_535,
            send_stream: 65_535,
            initial: 65_535,
            sid: 0,
        };
        let mut preface = [0; 24];
        h.s.read_exact(&mut preface).unwrap_or_else(|_| gone());
        h.frame(SETTINGS, 0, 0, &[]);
        loop {
            let (kind, flags, sid, p) = h.next();
            if kind != HEADERS {
                continue;
            }
            h.sid = sid;
            h.send_stream = h.initial;
            let fields = h.dec.decode(&p).unwrap();
            let get = |n: &str| {
                fields
                    .iter()
                    .find(|(k, _)| k == n.as_bytes())
                    .map(|(_, v)| String::from_utf8_lossy(v).into_owned())
            };
            let path = get(":path").unwrap_or_default();
            if path.starts_with("/big") {
                h.download(path.contains("length=1"));
            } else {
                let cl = get("content-length").unwrap_or_else(|| "-1".to_owned());
                let n = if flags & END_STREAM != 0 {
                    0
                } else {
                    h.read_upload()
                };
                let body = format!("{n} {cl}");
                h.head(
                    &[
                        (":status", "200"),
                        ("content-length", &body.len().to_string()),
                    ],
                    false,
                );
                h.frame(DATA, END_STREAM, sid, body.as_bytes());
            }
        }
    }

    fn frame(&mut self, kind: u8, flags: u8, stream: u32, payload: &[u8]) {
        let len = payload.len();
        let mut f = vec![(len >> 16) as u8, (len >> 8) as u8, len as u8, kind, flags];
        f.extend_from_slice(&stream.to_be_bytes());
        f.extend_from_slice(payload);
        self.s.write_all(&f).unwrap_or_else(|_| gone());
    }

    fn head(&mut self, fields: &[(&str, &str)], end: bool) {
        let mut block = Vec::new();
        for (n, v) in fields {
            block.push(0);
            hpack_str(&mut block, n);
            hpack_str(&mut block, v);
        }
        let flags = END_HEADERS | if end { END_STREAM } else { 0 };
        let sid = self.sid;
        self.frame(HEADERS, flags, sid, &block);
    }

    /// The next frame of akit's, after answering the connection's own: kind, flags, stream,
    /// payload.
    fn next(&mut self) -> (u8, u8, u32, Vec<u8>) {
        loop {
            if let Some(f) = self.step() {
                return f;
            }
        }
    }

    /// Reads one frame: the connection's own (settings, pings, window updates) are answered and
    /// give `None`.
    fn step(&mut self) -> Option<(u8, u8, u32, Vec<u8>)> {
        {
            let mut h = [0; 9];
            self.s.read_exact(&mut h).unwrap_or_else(|_| gone());
            let len = (usize::from(h[0]) << 16) | (usize::from(h[1]) << 8) | usize::from(h[2]);
            let (kind, flags) = (h[3], h[4]);
            let stream = u32::from_be_bytes([h[5], h[6], h[7], h[8]]) & 0x7fff_ffff;
            let mut p = vec![0; len];
            self.s.read_exact(&mut p).unwrap_or_else(|_| gone());
            match kind {
                SETTINGS if flags & ACK == 0 => {
                    for s in p.chunks_exact(6) {
                        if u16::from_be_bytes([s[0], s[1]]) == 4 {
                            let v = i64::from(u32::from_be_bytes([s[2], s[3], s[4], s[5]]));
                            self.send_stream += v - self.initial;
                            self.initial = v;
                        }
                    }
                    self.frame(SETTINGS, ACK, 0, &[]);
                }
                SETTINGS => {}
                PING if flags & ACK == 0 => self.frame(PING, ACK, 0, &p),
                PING => {}
                WINDOW_UPDATE => {
                    let v = i64::from(u32::from_be_bytes([p[0], p[1], p[2], p[3]]) & 0x7fff_ffff);
                    if stream == 0 {
                        self.send_conn += v;
                    } else if stream == self.sid {
                        self.send_stream += v;
                    }
                }
                RST_STREAM | GOAWAY => panic!("frame {kind} from akit: {p:?}"),
                _ => return Some((kind, flags, stream, p)),
            }
            None
        }
    }

    /// Sends `SIZE` bytes of the pattern on the current stream as akit's windows let it.
    fn download(&mut self, length: bool) {
        let size = SIZE.to_string();
        let mut fields = vec![(":status", "200")];
        if length {
            fields.push(("content-length", &size));
        }
        self.head(&fields, false);
        let mut buf = vec![0; 16 << 10];
        let mut off = 0;
        while off < SIZE {
            let room = self.send_conn.min(self.send_stream);
            if room <= 0 {
                // Blocked on akit's window: a held download keeps it closed.
                if let Some((kind, _, _, _)) = self.step() {
                    assert!(kind != HEADERS, "a request while the download is blocked");
                }
                continue;
            }
            let n = buf
                .len()
                .min(SIZE - off)
                .min(usize::try_from(room).unwrap());
            fill(&mut buf[..n], off);
            off += n;
            let end = if off == SIZE { END_STREAM } else { 0 };
            let data = buf[..n].to_vec();
            let sid = self.sid;
            self.frame(DATA, end, sid, &data);
            self.send_conn -= n as i64;
            self.send_stream -= n as i64;
        }
    }

    /// Reads the current stream's body slowly, checking it and giving its window back as it
    /// does; its length.
    fn read_upload(&mut self) -> usize {
        let mut pace = Pace { since: 0 };
        let mut off = 0;
        loop {
            let (kind, flags, sid, p) = self.next();
            if sid != self.sid {
                continue;
            }
            if kind == DATA {
                check(&p, off);
                off += p.len();
                pace.took(p.len());
                if !p.is_empty() && flags & END_STREAM == 0 {
                    let n = u32::try_from(p.len()).unwrap().to_be_bytes();
                    self.frame(WINDOW_UPDATE, 0, 0, &n);
                    self.frame(WINDOW_UPDATE, 0, sid, &n);
                }
            }
            if flags & END_STREAM != 0 {
                return off;
            }
        }
    }
}

/// A download over HTTP/1.1 by a client slower than the upstream: the response's head and the
/// body's length.
fn h1_download(addr: SocketAddr, length: bool) -> (String, usize) {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let mut w = s.try_clone().unwrap();
    let q = if length { "?length=1" } else { "" };
    write!(w, "GET /big{q} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    let mut r = BufReader::with_capacity(32 << 10, s);
    let head = read_head(&mut r).unwrap();
    let mut pace = Pace { since: 0 };
    let n = read_body(&mut r, &head, |data, off| {
        check(data, off);
        pace.took(data.len());
    });
    (head, n)
}

/// An upload over HTTP/1.1, as fast as akit takes it: the response's body.
fn h1_upload(addr: SocketAddr, length: bool) -> String {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let mut w = s.try_clone().unwrap();
    let framing = if length {
        format!("Content-Length: {SIZE}")
    } else {
        "Transfer-Encoding: chunked".to_owned()
    };
    write!(w, "PUT /upload HTTP/1.1\r\nHost: x\r\n{framing}\r\n\r\n").unwrap();
    let mut buf = vec![0; 64 << 10];
    let mut off = 0;
    while off < SIZE {
        let n = buf.len().min(SIZE - off);
        fill(&mut buf[..n], off);
        if length {
            w.write_all(&buf[..n]).unwrap();
        } else {
            write!(w, "{n:x}\r\n").unwrap();
            w.write_all(&buf[..n]).unwrap();
            w.write_all(b"\r\n").unwrap();
        }
        off += n;
    }
    if !length {
        w.write_all(b"0\r\n\r\n").unwrap();
    }
    let mut r = BufReader::new(s);
    let head = read_head(&mut r).unwrap();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let mut body = Vec::new();
    read_body(&mut r, &head, |data, _| body.extend_from_slice(data));
    String::from_utf8(body).unwrap()
}

/// A minimal h2c client, prior knowledge, one stream: what these tests need of HTTP/2, and no
/// more. It reads frames only when asked, so it can be as slow as a test wants, and gives the
/// window back only for what it took.
struct H2 {
    s: TcpStream,
    /// The peer's windows for what this client sends: the connection's and stream 1's.
    send_conn: i64,
    send_stream: i64,
    /// The peer's initial stream window, and largest frame.
    initial: i64,
}

const DATA: u8 = 0;
const HEADERS: u8 = 1;
const RST_STREAM: u8 = 3;
const SETTINGS: u8 = 4;
const PING: u8 = 6;
const GOAWAY: u8 = 7;
const WINDOW_UPDATE: u8 = 8;
const END_STREAM: u8 = 1;
const END_HEADERS: u8 = 4;
const ACK: u8 = 1;

/// An HPACK string literal, not Huffman-coded.
fn hpack_str(out: &mut Vec<u8>, s: &str) {
    hpack_int(out, 0, 7, s.len());
    out.extend_from_slice(s.as_bytes());
}

/// An HPACK integer with an `n`-bit prefix, after the bits in `first`.
fn hpack_int(out: &mut Vec<u8>, first: u8, n: u32, mut v: usize) {
    let max = (1usize << n) - 1;
    if v < max {
        out.push(first | v as u8);
        return;
    }
    out.push(first | max as u8);
    v -= max;
    while v >= 128 {
        out.push((v % 128) as u8 | 0x80);
        v /= 128;
    }
    out.push(v as u8);
}

impl H2 {
    fn connect(addr: SocketAddr) -> H2 {
        let s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
        // Its frames are small (window updates): sent at once, not held for Nagle.
        s.set_nodelay(true).unwrap();
        let mut h = H2 {
            s,
            send_conn: 65_535,
            send_stream: 65_535,
            initial: 65_535,
        };
        h.s.write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n").unwrap();
        // Default settings: a 64 KiB window, which the reader gives back as it reads.
        h.frame(SETTINGS, 0, 0, &[]);
        h
    }

    fn frame(&mut self, kind: u8, flags: u8, stream: u32, payload: &[u8]) {
        let len = payload.len();
        let mut f = vec![(len >> 16) as u8, (len >> 8) as u8, len as u8, kind, flags];
        f.extend_from_slice(&stream.to_be_bytes());
        f.extend_from_slice(payload);
        self.s.write_all(&f).unwrap();
    }

    /// Sends the request's head on stream 1: every field a literal without indexing.
    fn request(&mut self, method: &str, path: &str, fields: &[(&str, &str)], end: bool) {
        let mut block = Vec::new();
        for (n, v) in [
            (":method", method),
            (":scheme", "http"),
            (":path", path),
            (":authority", "x"),
        ]
        .iter()
        .chain(fields)
        {
            block.push(0);
            hpack_str(&mut block, n);
            hpack_str(&mut block, v);
        }
        let flags = END_HEADERS | if end { END_STREAM } else { 0 };
        self.frame(HEADERS, flags, 1, &block);
    }

    /// The next frame, after answering the connection's own (settings, pings, window updates).
    /// Panics on a reset or GOAWAY.
    fn next(&mut self) -> (u8, u8, Vec<u8>) {
        loop {
            if let Some(f) = self.step() {
                return f;
            }
        }
    }

    /// Reads one frame: the connection's own are answered and give `None`. Panics on a reset or
    /// GOAWAY.
    fn step(&mut self) -> Option<(u8, u8, Vec<u8>)> {
        {
            let mut h = [0; 9];
            self.s.read_exact(&mut h).unwrap();
            let len = (usize::from(h[0]) << 16) | (usize::from(h[1]) << 8) | usize::from(h[2]);
            let (kind, flags) = (h[3], h[4]);
            let stream = u32::from_be_bytes([h[5], h[6], h[7], h[8]]) & 0x7fff_ffff;
            let mut p = vec![0; len];
            self.s.read_exact(&mut p).unwrap();
            match kind {
                SETTINGS if flags & ACK == 0 => {
                    for s in p.chunks_exact(6) {
                        if u16::from_be_bytes([s[0], s[1]]) == 4 {
                            let v = i64::from(u32::from_be_bytes([s[2], s[3], s[4], s[5]]));
                            self.send_stream += v - self.initial;
                            self.initial = v;
                        }
                    }
                    self.frame(SETTINGS, ACK, 0, &[]);
                }
                SETTINGS => {}
                PING if flags & ACK == 0 => self.frame(PING, ACK, 0, &p),
                PING => {}
                WINDOW_UPDATE => {
                    let v = i64::from(u32::from_be_bytes([p[0], p[1], p[2], p[3]]) & 0x7fff_ffff);
                    if stream == 0 {
                        self.send_conn += v;
                    } else {
                        self.send_stream += v;
                    }
                }
                RST_STREAM | GOAWAY => panic!("frame {kind} from the server: {p:?}"),
                _ => return Some((kind, flags, p)),
            }
            None
        }
    }

    /// The response's body on stream 1, each piece handed to `f` and the window given back for
    /// it after; its length.
    fn response(&mut self, mut f: impl FnMut(&[u8], usize)) -> usize {
        let mut off = 0;
        loop {
            let (kind, flags, p) = self.next();
            if kind == DATA {
                let (data, padded) = if flags & 8 != 0 {
                    let pad = usize::from(p[0]);
                    (&p[1..p.len() - pad], true)
                } else {
                    (&p[..], false)
                };
                let _ = padded;
                f(data, off);
                off += data.len();
                if !p.is_empty() && flags & END_STREAM == 0 {
                    let n = u32::try_from(p.len()).unwrap().to_be_bytes();
                    self.frame(WINDOW_UPDATE, 0, 0, &n);
                    self.frame(WINDOW_UPDATE, 0, 1, &n);
                }
            }
            if flags & END_STREAM != 0 && (kind == DATA || kind == HEADERS) {
                return off;
            }
        }
    }

    /// Sends `SIZE` bytes of the pattern on stream 1 as the peer's windows let it, then ends
    /// the stream.
    fn upload(&mut self) {
        let mut buf = vec![0; 16 << 10];
        let mut off = 0;
        while off < SIZE {
            let room = self.send_conn.min(self.send_stream);
            if room <= 0 {
                if let Some((kind, _, _)) = self.step() {
                    assert!(kind != HEADERS, "answered before the body was sent");
                }
                continue;
            }
            let n = buf
                .len()
                .min(SIZE - off)
                .min(usize::try_from(room).unwrap());
            fill(&mut buf[..n], off);
            off += n;
            let end = if off == SIZE { END_STREAM } else { 0 };
            let data = buf[..n].to_vec();
            self.frame(DATA, end, 1, &data);
            self.send_conn -= n as i64;
            self.send_stream -= n as i64;
        }
    }
}

/// Reports how long a body took and how far the memory rose, and checks that it stayed below
/// [`MAX_RISE`] where it was sampled.
fn bounded(name: &str, watch: Watch, start: Instant) {
    let took = start.elapsed();
    match watch.rise() {
        Some(rise) => {
            eprintln!(
                "{name}: {} MiB in {took:?}, resident memory rose {:.1} MiB",
                SIZE >> 20,
                rise as f64 / f64::from(1 << 20)
            );
            assert!(
                rise < MAX_RISE,
                "{name}: resident memory rose {rise} bytes moving {SIZE}"
            );
        }
        None => eprintln!("{name}: {} MiB in {took:?}", SIZE >> 20),
    }
}

#[test]
fn stream_download() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let akit = Akit::start(upstream());
    for (h2c, length) in [(false, true), (false, false), (true, true), (true, false)] {
        let name = format!("h2c={h2c} length={length}");
        let watch = Watch::new();
        let start = Instant::now();
        let mut pace = Pace { since: 0 };
        let n = if h2c {
            let mut h = H2::connect(akit.addr);
            let q = if length { "?length=1" } else { "" };
            h.request("GET", &format!("/big{q}"), &[], true);
            h.response(|data, off| {
                check(data, off);
                pace.took(data.len());
            })
        } else {
            let (head, n) = h1_download(akit.addr, length);
            assert!(head.starts_with("HTTP/1.1 200"), "{name}: {head}");
            // akio streams an HTTP/1.1 response chunked, whatever its length (akio API A35).
            assert_eq!(field(&head, "transfer-encoding"), Some("chunked"), "{name}");
            n
        };
        assert_eq!(n, SIZE, "{name}");
        bounded(&name, watch, start);
    }
}

#[test]
fn stream_upload() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let akit = Akit::start(upstream());
    for (h2c, length) in [(false, true), (false, false), (true, true), (true, false)] {
        let name = format!("h2c={h2c} length={length}");
        let watch = Watch::new();
        let start = Instant::now();
        let got = if h2c {
            let mut h = H2::connect(akit.addr);
            let size = SIZE.to_string();
            let fields: &[(&str, &str)] = if length {
                &[("content-length", &size)]
            } else {
                &[]
            };
            h.request("PUT", "/upload", fields, false);
            h.upload();
            let mut body = Vec::new();
            h.response(|data, _| body.extend_from_slice(data));
            String::from_utf8(body).unwrap()
        } else {
            h1_upload(akit.addr, length)
        };
        // The upstream got the body as it came: by its length, or chunked (HTTP/2 has no
        // framing of its own, so one without a length goes chunked too).
        let want = if length {
            format!("{SIZE} {SIZE} ")
        } else {
            format!("{SIZE} -1 chunked")
        };
        assert_eq!(got, want, "{name}");
        bounded(&name, watch, start);
    }
}

/// [`stream_download`] from an h2c upstream (I18b): the download to a slow client holds the
/// upstream's stream (its window kept closed), rather than piling up in akit.
#[test]
fn stream_download_from_h2c() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let akit = Akit::start_with(h2_upstream(), "h2c");
    for (h2c, length) in [(false, true), (false, false), (true, true), (true, false)] {
        let name = format!("to h2c, h2c={h2c} length={length}");
        let watch = Watch::new();
        let start = Instant::now();
        let mut pace = Pace { since: 0 };
        let n = if h2c {
            let mut h = H2::connect(akit.addr);
            let q = if length { "?length=1" } else { "" };
            h.request("GET", &format!("/big{q}"), &[], true);
            h.response(|data, off| {
                check(data, off);
                pace.took(data.len());
            })
        } else {
            let (head, n) = h1_download(akit.addr, length);
            assert!(head.starts_with("HTTP/1.1 200"), "{name}: {head}");
            n
        };
        assert_eq!(n, SIZE, "{name}");
        bounded(&name, watch, start);
    }
}

/// [`stream_upload`] to an h2c upstream (I18b): the upload is paced by what waits in its stream,
/// rather than piling up in akit.
#[test]
fn stream_upload_to_h2c() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let akit = Akit::start_with(h2_upstream(), "h2c");
    for (h2c, length) in [(false, true), (false, false), (true, true), (true, false)] {
        let name = format!("to h2c, h2c={h2c} length={length}");
        let watch = Watch::new();
        let start = Instant::now();
        let got = if h2c {
            let mut h = H2::connect(akit.addr);
            let size = SIZE.to_string();
            let fields: &[(&str, &str)] = if length {
                &[("content-length", &size)]
            } else {
                &[]
            };
            h.request("PUT", "/upload", fields, false);
            h.upload();
            let mut body = Vec::new();
            h.response(|data, _| body.extend_from_slice(data));
            String::from_utf8(body).unwrap()
        } else {
            h1_upload(akit.addr, length)
        };
        let want = if length {
            format!("{SIZE} {SIZE}")
        } else {
            format!("{SIZE} -1")
        };
        assert_eq!(got, want, "{name}");
        bounded(&name, watch, start);
    }
}

/// fit's `TestUpstreamBreaksMidBody` (`failure_test.go`), over HTTP/1.1: a body that breaks off: once the response has begun, the
/// client's connection is closed short of the body's end, which is how an HTTP/1.1 client tells.
#[test]
fn broken_download() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let akit = Akit::start(upstream());
    let s = TcpStream::connect(akit.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let mut w = s.try_clone().unwrap();
    write!(w, "GET /broken HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    let mut r = BufReader::new(s);
    let head = read_head(&mut r).unwrap();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(field(&head, "transfer-encoding"), Some("chunked"));
    let mut rest = Vec::new();
    // Read to the end: the connection closes.
    r.read_to_end(&mut rest).unwrap();
    assert!(rest.len() >= 1 << 20, "{} bytes", rest.len());
    assert!(rest.len() < SIZE);
    assert!(
        !rest.ends_with(b"0\r\n\r\n"),
        "the body was ended as if whole"
    );
}
