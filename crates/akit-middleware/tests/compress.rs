//! The `compress` middleware and the `static` handler's `compress` through akit's listener on
//! akio, with fit's tests (`middleware/compress/compress_test.go`, `cover_test.go`,
//! `handler/static/compress_test.go`) ported, over HTTP/1.1 and h2c, on the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
//!
//! The routes' handler is a test one, registered as `respond`, which answers by path as fit's
//! `textHandler` does: whole bodies, a stream of events held until the test lets it go on, a
//! large body written in pieces. Others go through the `proxy` handler to an upstream of std
//! sockets that streams, and through the `static` handler.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::channel;
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, UringConfig};
use akio_http::Method;
use akio_http::client::ClientRequest;
use akio_http::h2client::{H2Client, H2Event, H2Options};
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, HandlerParams, Info, Registry, compile};

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

/// A body that compresses well: fit's `long`.
fn long() -> &'static str {
    static LONG: OnceLock<String> = OnceLock::new();
    LONG.get_or_init(|| {
        "{\"name\": \"a record\", \"value\": 12345, \"tags\": [\"x\", \"y\"]}\n".repeat(4000)
    })
}

/// The test handler, as fit's `textHandler`.
mod text {
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use akio_http::{Context, Headers, Response};
    use akit_runtime::{Handler, HandlerParams, Info, Provision, ProvisionError, Provisioned};

    /// The streams waiting for the test to let them write their second event, by path: each
    /// test streams from a path of its own, as tests run at once.
    static WAITING: Mutex<Vec<(String, Sender<()>)>> = Mutex::new(Vec::new());

    /// Lets the streams of `path` go on.
    pub(crate) fn go(path: &str) {
        for (p, tx) in WAITING.lock().unwrap().iter() {
            if p == path {
                let _ = tx.send(());
            }
        }
        WAITING.lock().unwrap().retain(|(p, _)| p != path);
    }

    fn wait(path: &str) -> Receiver<()> {
        let (tx, rx) = channel();
        WAITING.lock().unwrap().push((path.to_owned(), tx));
        rx
    }

    fn whole(ctx: &mut Context, status: u16, fields: &[(&str, &str)], body: &[u8]) {
        let mut r = Response::new(status);
        for (n, v) in fields {
            r.headers.append(n, v);
        }
        r.body = body.to_vec();
        ctx.respond(r);
    }

    struct Text;

    impl akio_http::Service for Text {
        fn call(&self, ctx: &mut Context) {
            let long = super::long().as_bytes();
            let path = ctx.path().to_owned();
            match path.as_str() {
                "/whole" => whole(
                    ctx,
                    200,
                    &[("Content-Type", "application/json"), ("ETag", "\"v1\"")],
                    long,
                ),
                p if p.starts_with("/stream") => {
                    let mut h = Headers::new();
                    h.set("Content-Type", "text/event-stream");
                    let Some(s) = ctx.stream_response(200, h) else {
                        return;
                    };
                    let rx = wait(p);
                    thread::spawn(move || {
                        if s.write(b"data: one\n\n").is_err() {
                            return;
                        }
                        // Its client going ends the wait, as it would a real stream's: a write
                        // then fails and the stream is dropped.
                        let _ = rx.recv_timeout(Duration::from_secs(5));
                        if s.write(b"data: two\n\n").is_ok() {
                            s.finish();
                        }
                    });
                }
                "/large" => {
                    let mut h = Headers::new();
                    h.set("Content-Type", "text/plain");
                    let Some(s) = ctx.stream_response(200, h) else {
                        return;
                    };
                    for piece in long.chunks(10_000) {
                        let _ = s.write(piece);
                    }
                    s.finish();
                }
                "/small" => whole(ctx, 200, &[("Content-Type", "text/plain")], b"tiny"),
                "/png" => whole(ctx, 200, &[("Content-Type", "image/png")], long),
                "/encoded" => whole(
                    ctx,
                    200,
                    &[("Content-Type", "text/plain"), ("Content-Encoding", "gzip")],
                    long,
                ),
                "/partial" => whole(
                    ctx,
                    206,
                    &[
                        ("Content-Type", "text/plain"),
                        ("Content-Range", "bytes 0-9/100"),
                    ],
                    long,
                ),
                "/notransform" => whole(
                    ctx,
                    200,
                    &[
                        ("Content-Type", "text/plain"),
                        ("Cache-Control", "no-transform"),
                    ],
                    long,
                ),
                _ => ctx.respond(Response::text(404, "no")),
            }
        }
    }

    impl Handler for Text {}

    fn new(_: &Provision<'_, HandlerParams>) -> Result<Provisioned<dyn Handler>, ProvisionError> {
        Ok(Provisioned::new(Arc::new(Text) as Arc<dyn Handler>))
    }

    pub(crate) static MODULES: &[Info<HandlerParams>] = &[Info::handler(
        "http.handlers.respond",
        "Answers by path.",
        new,
    )];
}

/// akit serving one route.
struct Akit {
    engine: Arc<Engine>,
    addr: SocketAddr,
    _cell: GraphCell,
}

impl Akit {
    fn start(json: &str, handlers: &'static [Info<HandlerParams>]) -> Akit {
        let config = akit_config::from_json(json, None).unwrap_or_else(|e| panic!("{json}: {e}"));
        let slot = EngineSlot::new();
        let registry =
            Registry::new(&[handlers, akit_middleware::MODULES, akit_proxy::MODULES]).unwrap();
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

    /// The route: `handler` (a module reference) behind `middleware` (a JSON array).
    fn route(
        middleware: &str,
        handler: &str,
        extra: &str,
        modules: &'static [Info<HandlerParams>],
    ) -> Akit {
        Akit::start(
            &format!(
                r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}}],
                    {extra}
                    "routes": [{{"middleware": {middleware}, "handler": {handler}}}]}}"#
            ),
            modules,
        )
    }

    /// The test handler behind the `compress` middleware with `params`.
    fn text(params: &str) -> Akit {
        Akit::route(
            &format!(r#"[{{"module": "compress", "params": {params}}}]"#),
            r#"{"module": "respond"}"#,
            "",
            text::MODULES,
        )
    }
}

impl Drop for Akit {
    fn drop(&mut self) {
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
    }
}

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
}

/// `req` over h2c to `addr`, with `fields`, and `body`.
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
    if !body.is_empty() {
        for piece in body.chunks(32 << 10) {
            assert!(up.write(piece));
        }
    }
    up.finish();
    let got = rx.recv_timeout(Duration::from_secs(20)).unwrap();
    c.close();
    got
}

/// A raw HTTP/1.1 exchange on a connection kept alive: `head` (its request line and fields,
/// without the blank line) and `body` written as they are, the response read as it is framed
/// (chunked, by its length, or, without either, until the connection closes).
fn h1(addr: SocketAddr, head: &str, body: &[u8]) -> Got {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    s.write_all(format!("{head}\r\n\r\n").as_bytes()).unwrap();
    s.write_all(body).unwrap();
    h1_response(s, head.starts_with("HEAD "))
}

/// A response read from `s`, to a GET.
fn h1_read(s: TcpStream) -> Got {
    h1_response(s, false)
}

/// A response read from `s` as it is framed; `head`: to a HEAD request.
fn h1_response(s: TcpStream, head_only: bool) -> Got {
    let mut r = BufReader::new(s);
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
    let chunked = got
        .one("transfer-encoding")
        .is_some_and(|v| v.eq_ignore_ascii_case("chunked"));
    if chunked {
        // Each chunk, then the trailers.
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
    if head_only || matches!(got.status, 204 | 304) {
        got.complete = true;
        return got;
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

/// The two kinds of client fit's tests run from.
#[derive(Clone, Copy, Debug)]
enum Client {
    H1,
    H2c,
}

const CLIENTS: [Client; 2] = [Client::H1, Client::H2c];

impl Client {
    fn send(self, addr: SocketAddr, method: Method, path: &str, fields: &[(&str, &str)]) -> Got {
        match self {
            Client::H2c => h2(addr, method, path, fields, b""),
            Client::H1 => {
                let mut head = format!("{} {path} HTTP/1.1\r\nHost: {addr}", method.as_str());
                for (n, v) in fields {
                    head.push_str(&format!("\r\n{n}: {v}"));
                }
                h1(addr, &head, b"")
            }
        }
    }
}

/// `body` decoded from `coding`.
fn decode(coding: &str, body: &[u8]) -> Vec<u8> {
    match coding {
        "" => body.to_vec(),
        "gzip" => akio_deflate::decompress(akio_deflate::Format::Gzip, body, 1 << 26).unwrap(),
        "br" => akio_brotli::decompress(body, 1 << 26).unwrap(),
        "zstd" => akio_zstd::decompress(body, 1 << 26).unwrap(),
        c => panic!("coding {c:?}"),
    }
}

/// What fit's `fetch` reads of a response: the encoding, `Vary`, `ETag`, the bytes on the wire
/// and the body decoded.
#[derive(Debug)]
struct Fetched {
    status: u16,
    encoding: String,
    vary: String,
    etag: String,
    wire: usize,
    body: Vec<u8>,
}

fn fetch(client: Client, addr: SocketAddr, method: Method, path: &str, accept: &str) -> Fetched {
    let fields: Vec<(&str, &str)> = if accept.is_empty() {
        Vec::new()
    } else {
        vec![("Accept-Encoding", accept)]
    };
    let head = method == Method::Head;
    let got = client.send(addr, method, path, &fields);
    assert!(got.complete, "{client:?} {path}: cut short");
    let encoding = got.one("content-encoding").unwrap_or("").to_owned();
    if !head {
        if let Some(n) = got.one("content-length") {
            assert_eq!(
                n.parse::<usize>().unwrap(),
                got.body.len(),
                "{client:?} {path}"
            );
        }
    }
    let body = if path == "/encoded" || head {
        got.body.clone()
    } else {
        decode(&encoding, &got.body)
    };
    Fetched {
        status: got.status,
        vary: got.one("vary").unwrap_or("").to_owned(),
        etag: got.one("etag").unwrap_or("").to_owned(),
        wire: got.body.len(),
        encoding,
        body,
    }
}

/// fit's `TestCompress`: negotiation, what is left alone, and HEAD.
#[test]
fn compress() {
    let akit = Akit::text("{}");
    let long = long().as_bytes();
    for client in CLIENTS {
        for (path, accept, encoding) in [
            ("/whole", "gzip", "gzip"),
            ("/whole", "br", "br"),
            ("/whole", "zstd", "zstd"),
            ("/whole", "gzip, br, zstd", "zstd"), // equal: the server's order
            ("/whole", "gzip;q=1, br;q=0.5, zstd;q=0.2", "gzip"),
            ("/whole", "*", "zstd"),
            ("/whole", "*;q=0.5, gzip", "gzip"),
            ("/whole", "x-gzip", "gzip"),
            ("/whole", "identity", ""),
            ("/whole", "gzip;q=0", ""),
            ("/whole", "", ""),
            ("/large", "br", "br"),
            ("/large", "gzip", "gzip"),
            ("/large", "zstd", "zstd"),
        ] {
            let g = fetch(client, akit.addr, Method::Get, path, accept);
            assert_eq!(
                (g.encoding.as_str(), g.vary.as_str(), g.body.len()),
                (encoding, "Accept-Encoding", long.len()),
                "{client:?} {path} {accept:?}"
            );
            assert!(
                g.status == 200 && g.body == long,
                "{client:?} {path} {accept:?}"
            );
            if !encoding.is_empty() {
                assert!(
                    g.wire <= long.len() / 10,
                    "{client:?} {path} {accept:?}: {} bytes",
                    g.wire
                );
            }
            if path == "/whole" {
                let etag = if encoding.is_empty() {
                    "\"v1\""
                } else {
                    "W/\"v1\""
                };
                assert_eq!(g.etag, etag, "{client:?} {accept:?}");
            }
        }
        for path in ["/small", "/png", "/encoded", "/partial", "/notransform"] {
            let g = fetch(client, akit.addr, Method::Get, path, "gzip, br, zstd");
            if path == "/encoded" {
                assert_eq!(
                    (g.encoding.as_str(), g.body == long),
                    ("gzip", true),
                    "recompressed"
                );
                continue;
            }
            assert_eq!(
                (g.encoding.as_str(), g.vary.as_str()),
                ("", ""),
                "{client:?} {path}"
            );
        }
        let g = fetch(client, akit.addr, Method::Head, "/whole", "gzip");
        assert_eq!(
            (g.encoding.as_str(), g.vary.as_str()),
            ("", "Accept-Encoding"),
            "{client:?} HEAD"
        );
    }
}

/// The head and the first chunk of a streamed response over HTTP/1.1, then the rest once `then`
/// ran: fit's check that the first event arrives, decoded, before the second is written.
fn h1_events(
    addr: SocketAddr,
    path: &str,
    accept: &str,
    then: impl FnOnce(),
) -> (String, Vec<u8>, Vec<u8>) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: x\r\nAccept-Encoding: {accept}\r\n\r\n").as_bytes(),
    )
    .unwrap();
    let mut r = BufReader::new(s);
    let mut coding = String::new();
    loop {
        let mut l = String::new();
        r.read_line(&mut l).unwrap();
        let l = l.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((n, v)) = l.split_once(':') {
            if n.eq_ignore_ascii_case("content-encoding") {
                coding = v.trim().to_owned();
            }
        }
    }
    let chunk = |r: &mut BufReader<TcpStream>| -> Option<Vec<u8>> {
        let mut size = String::new();
        r.read_line(&mut size).ok()?;
        let n = usize::from_str_radix(size.trim(), 16).ok()?;
        let mut data = vec![0; n + 2];
        r.read_exact(&mut data).ok()?;
        data.truncate(n);
        (n > 0).then_some(data)
    };
    let first = chunk(&mut r).unwrap();
    then();
    let mut rest = Vec::new();
    while let Some(c) = chunk(&mut r) {
        rest.extend_from_slice(&c);
    }
    (coding, first, rest)
}

/// A decoder fed piece by piece, as a browser decodes a response as it arrives.
#[allow(clippy::large_enum_variant, reason = "one per test")]
enum Dec {
    Gzip(akio_deflate::Inflater),
    Br(akio_brotli::Decompressor),
    Zstd(akio_zstd::Decompressor),
}

impl Dec {
    fn new(coding: &str) -> Dec {
        match coding {
            "gzip" => Dec::Gzip(akio_deflate::Inflater::new(akio_deflate::Format::Gzip)),
            "br" => Dec::Br(akio_brotli::Decompressor::new()),
            "zstd" => Dec::Zstd(akio_zstd::Decompressor::new()),
            c => panic!("coding {c:?}"),
        }
    }

    fn feed(&mut self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut rest = data;
        loop {
            let used = match self {
                Dec::Gzip(d) => d.decompress_vec(rest, &mut out, 1 << 26).unwrap().consumed,
                Dec::Br(d) => d.decompress_vec(rest, &mut out, 1 << 26).unwrap().consumed,
                Dec::Zstd(d) => d.decompress_vec(rest, &mut out, 1 << 26).unwrap().consumed,
            };
            rest = &rest[used..];
            if rest.is_empty() || used == 0 {
                return out;
            }
        }
    }
}

/// The events of a streamed response over h2c: the coding, then each DATA frame decoded as it
/// arrives; `then` runs after the first.
fn h2_events(
    addr: SocketAddr,
    path: &str,
    accept: &str,
    then: impl FnOnce(),
) -> (String, Vec<u8>, Vec<u8>) {
    let c = H2Client::dial_blocking(&clients(), addr, None, H2Options::default()).unwrap();
    let (tx, rx) = channel();
    let up = c.open(
        ClientRequest::get(&format!("http://{addr}{path}")).header("accept-encoding", accept),
        move |ev| {
            let _ = tx.send(match ev {
                H2Event::Head { headers, .. } => {
                    Err(headers.get("content-encoding").unwrap_or("").to_owned())
                }
                H2Event::Data(d) => Ok(d.to_vec()),
                H2Event::End => Err("end".to_owned()),
                H2Event::Trailers(_) => Err("trailers".to_owned()),
                H2Event::Error(e) => Err(format!("error {e}")),
            });
        },
    );
    assert!(up.finish());
    let next = || rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let coding = next().unwrap_err();
    let mut dec = Dec::new(&coding);
    let mut first = Vec::new();
    while first.is_empty() {
        first = dec.feed(&next().unwrap());
    }
    then();
    let mut rest = Vec::new();
    loop {
        match next() {
            Ok(d) => rest.extend(dec.feed(&d)),
            Err(e) if e == "end" => break,
            Err(e) => panic!("{e}"),
        }
    }
    c.close();
    (coding, first, rest)
}

/// fit's `TestCompressStream`: a streamed response stays streamed: its first event arrives,
/// decompressed, before the handler writes the second.
#[test]
fn compress_stream() {
    let akit = Akit::text("{}");
    for enc in ["gzip", "br", "zstd"] {
        let start = Instant::now();
        let (coding, first, rest) =
            h1_events(akit.addr, "/stream/one", enc, || text::go("/stream/one"));
        assert_eq!(coding, enc);
        let mut d = Dec::new(enc);
        assert_eq!(d.feed(&first), b"data: one\n\n", "h1 {enc}");
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "h1 {enc}: the first event waited"
        );
        assert_eq!(d.feed(&rest), b"data: two\n\n", "h1 {enc}");
        let start = Instant::now();
        let (coding, first, rest) =
            h2_events(akit.addr, "/stream/one", enc, || text::go("/stream/one"));
        assert_eq!(coding, enc);
        assert_eq!(first, b"data: one\n\n", "h2c {enc}");
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "h2c {enc}: the first event waited"
        );
        assert_eq!(rest, b"data: two\n\n", "h2c {enc}");
    }
}

/// fit's `TestCompressBounded`: past `max_in_flight` responses go uncompressed, and a stream
/// whose client goes gives its place back.
#[test]
fn compress_bounded() {
    let akit = Akit::text(r#"{"max_in_flight": 1}"#);
    let long = long().as_bytes();
    let mut s = TcpStream::connect(akit.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(b"GET /stream/bounded HTTP/1.1\r\nHost: x\r\nAccept-Encoding: gzip\r\n\r\n")
        .unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        s.read_exact(&mut b).unwrap();
        head.push(b[0]);
    }
    let head = String::from_utf8(head).unwrap().to_ascii_lowercase();
    assert!(
        head.contains("content-encoding: gzip"),
        "the stream is not compressed: {head}"
    );
    // The stream holds the one place.
    let g = fetch(Client::H2c, akit.addr, Method::Get, "/whole", "gzip");
    assert_eq!(
        (g.encoding.as_str(), g.body == long),
        ("", true),
        "past max_in_flight"
    );
    drop(s);
    thread::sleep(Duration::from_millis(50));
    text::go("/stream/bounded"); // its write fails, and the stream is dropped
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if fetch(Client::H2c, akit.addr, Method::Get, "/whole", "gzip").encoding == "gzip" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the stream's place was not given back when its client went"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

/// fit's `TestCompressReleased`: with one place, responses one after another, whole and
/// streamed, over each protocol, are all compressed: each gives its place back.
#[test]
fn compress_released() {
    let akit = Akit::text(r#"{"max_in_flight": 1}"#);
    for client in CLIENTS {
        for _ in 0..3 {
            for path in ["/whole", "/large"] {
                let g = fetch(client, akit.addr, Method::Get, path, "gzip, br, zstd");
                assert_eq!(
                    (g.encoding.as_str(), g.body.len()),
                    ("zstd", long().len()),
                    "{client:?} {path}"
                );
            }
        }
    }
}

/// `min_bytes`: a declared length below it is left alone, at it compressed.
#[test]
fn compress_min_bytes() {
    let akit = Akit::text(r#"{"min_bytes": 240000, "encodings": ["gzip"]}"#);
    // /whole is 236,000 bytes: below.
    let g = fetch(Client::H1, akit.addr, Method::Get, "/whole", "gzip");
    assert_eq!((g.encoding.as_str(), g.vary.as_str()), ("", ""));
    // /large streams, declaring no length: compressed.
    let g = fetch(Client::H1, akit.addr, Method::Get, "/large", "gzip");
    assert_eq!(g.encoding, "gzip");
    let akit = Akit::text(&format!(
        r#"{{"min_bytes": {}, "encodings": ["gzip"]}}"#,
        long().len()
    ));
    let g = fetch(Client::H1, akit.addr, Method::Get, "/whole", "gzip");
    assert_eq!(g.encoding, "gzip", "at min_bytes");
    // Offered gzip alone: zstd is not chosen.
    let g = fetch(
        Client::H2c,
        akit.addr,
        Method::Get,
        "/whole",
        "zstd, gzip;q=0.1",
    );
    assert_eq!(g.encoding, "gzip");
}

/// An upstream that answers every request with events, chunked: two at once and a third after
/// a pause, or, for `/sized`, `long` by its length.
fn upstream() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            thread::spawn(move || {
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut w = s;
                loop {
                    let mut target = String::new();
                    let mut l = String::new();
                    if r.read_line(&mut l).unwrap_or(0) == 0 {
                        return;
                    }
                    if let Some(t) = l.split_whitespace().nth(1) {
                        target = t.to_owned();
                    }
                    loop {
                        let mut l = String::new();
                        if r.read_line(&mut l).unwrap_or(0) == 0 || l.trim_end().is_empty() {
                            break;
                        }
                    }
                    if target == "/noise" {
                        let body = noise(NOISE);
                        let head = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n",
                            body.len()
                        );
                        if w.write_all(head.as_bytes()).is_err() || w.write_all(&body).is_err() {
                            return;
                        }
                        continue;
                    }
                    if target == "/sized" {
                        let long = long();
                        let head = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nETag: \"up\"\r\n\r\n",
                            long.len()
                        );
                        if w.write_all(head.as_bytes()).is_err()
                            || w.write_all(long.as_bytes()).is_err()
                        {
                            return;
                        }
                        continue;
                    }
                    let _ = w.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n");
                    for (i, pause) in [(1, 0), (2, 0), (3, 600)] {
                        thread::sleep(Duration::from_millis(pause));
                        let ev = format!("data: {i}\n\n");
                        if w.write_all(format!("{:x}\r\n{ev}\r\n", ev.len()).as_bytes())
                            .is_err()
                        {
                            return;
                        }
                    }
                    let _ = w.write_all(b"0\r\n\r\n");
                }
            });
        }
    });
    addr
}

/// The proxy's responses, streamed with linked backpressure, compressed as they stream: the
/// events before the upstream's pause arrive, decoded, before it ends; a body by its length is
/// compressed and loses its length.
#[test]
fn compress_proxied() {
    let up = upstream();
    let akit = Akit::route(
        r#"[{"module": "compress"}]"#,
        r#"{"module": "proxy", "params": {"upstream": "app"}}"#,
        &format!(r#""upstreams": [{{"name": "app", "addresses": ["{up}"]}}],"#),
        akit_handler::MODULES,
    );
    for enc in ["gzip", "br", "zstd"] {
        let start = Instant::now();
        let mut early = Duration::ZERO;
        let (coding, first, rest) =
            h1_events(akit.addr, "/events", enc, || early = start.elapsed());
        assert_eq!(coding, enc);
        let mut d = Dec::new(enc);
        let mut all = d.feed(&first);
        assert!(
            early < Duration::from_millis(500),
            "h1 {enc}: the first events waited {early:?}"
        );
        all.extend(d.feed(&rest));
        assert_eq!(all, b"data: 1\n\ndata: 2\n\ndata: 3\n\n", "h1 {enc}");
        let start = Instant::now();
        let mut early = Duration::ZERO;
        let (coding, first, rest) =
            h2_events(akit.addr, "/events", enc, || early = start.elapsed());
        assert_eq!(coding, enc);
        assert!(
            early < Duration::from_millis(500),
            "h2c {enc}: the first events waited {early:?}"
        );
        let mut all = first;
        all.extend(rest);
        assert_eq!(all, b"data: 1\n\ndata: 2\n\ndata: 3\n\n", "h2c {enc}");
    }
    for client in CLIENTS {
        let got = client.send(
            akit.addr,
            Method::Get,
            "/sized",
            &[("Accept-Encoding", "br")],
        );
        assert!(got.complete);
        assert_eq!(got.one("content-encoding"), Some("br"), "{client:?}");
        assert_eq!(got.one("etag"), Some("W/\"up\""), "{client:?}");
        assert_eq!(got.one("vary"), Some("Accept-Encoding"), "{client:?}");
        assert!(
            got.one("content-length")
                .is_none_or(|n| n.parse::<usize>().unwrap() == got.body.len())
        );
        assert!(got.body.len() < long().len() / 10);
        assert!(decode("br", &got.body) == long().as_bytes(), "{client:?}");
    }
}

/// The size of the upstream's `/noise`.
const NOISE: usize = 4 << 20;

/// A slow client: one that reads nothing for a while gets the whole body, compressed as it
/// streamed under the proxy's backpressure, and one that goes midway lets the stream go.
#[test]
fn compress_proxied_slow_and_gone_clients() {
    let up = upstream();
    let akit = Akit::route(
        r#"[{"module": "compress", "params": {"encodings": ["gzip"], "max_in_flight": 1}}]"#,
        r#"{"module": "proxy", "params": {"upstream": "app"}}"#,
        &format!(r#""upstreams": [{{"name": "app", "addresses": ["{up}"]}}],"#),
        akit_handler::MODULES,
    );
    let mut s = TcpStream::connect(akit.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    s.write_all(b"GET /noise HTTP/1.1\r\nHost: x\r\nAccept-Encoding: gzip\r\n\r\n")
        .unwrap();
    thread::sleep(Duration::from_millis(500));
    let got = h1_read(s);
    assert!(got.complete, "cut short");
    assert_eq!(got.one("content-encoding"), Some("gzip"));
    assert!(decode("gzip", &got.body) == noise(NOISE), "the body");
    // Gone midway: the place comes back (max_in_flight 1), and the next is compressed.
    let mut s = TcpStream::connect(akit.addr).unwrap();
    s.write_all(b"GET /noise HTTP/1.1\r\nHost: x\r\nAccept-Encoding: gzip\r\n\r\n")
        .unwrap();
    let mut some = [0u8; 4096];
    s.read_exact(&mut some).unwrap();
    drop(s);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = Client::H1.send(
            akit.addr,
            Method::Get,
            "/sized",
            &[("Accept-Encoding", "gzip")],
        );
        if got.one("content-encoding") == Some("gzip") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the gone client's place was not given back"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

/// A directory of files for the `static` handler.
fn tree(name: &str, files: &[(&str, &[u8])]) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("akit-compress-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    for (p, body) in files {
        std::fs::write(d.join(p), body).unwrap();
    }
    d
}

/// `n` bytes of JavaScript-like text that compresses as such text does: fit's `text`.
fn js(n: usize) -> String {
    let words = [
        "function", "return", "const", "let", "var", "if", "else", "for", "while", "this",
    ];
    let mut s = String::new();
    let mut x = 3u64;
    while s.len() < n {
        x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        s.push_str(&format!(
            "{} v{}={};",
            words[(x >> 33) as usize % words.len()],
            (x >> 20) % 1000,
            (x >> 40) % 1_000_000
        ));
    }
    s.truncate(n);
    s
}

/// `n` bytes that do not compress: fit's `noise`.
fn noise(n: usize) -> Vec<u8> {
    let mut x = 5u64;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            (x >> 56) as u8
        })
        .collect()
}

/// The `compress` middleware in front of the `static` handler: a file kept in memory is
/// compressed whole; one sent from disk is compressed as it is read (akio ADR-124), as fit's
/// middleware compresses whatever the handler writes.
#[test]
fn compress_static_files() {
    let small = js(20_000);
    let dir = tree(
        "mw",
        &[
            ("app.js", small.as_bytes()),
            ("big.js", js(50_000).as_bytes()),
        ],
    );
    let akit = Akit::route(
        r#"[{"module": "compress"}]"#,
        &format!(
            r#"{{"module": "static", "params": {{"root": "{}", "max_file_bytes": 30000}}}}"#,
            dir.display()
        ),
        "",
        akit_handler::MODULES,
    );
    for client in CLIENTS {
        let g = fetch(client, akit.addr, Method::Get, "/app.js", "gzip");
        assert_eq!(
            (g.encoding.as_str(), g.vary.as_str()),
            ("gzip", "Accept-Encoding"),
            "{client:?}"
        );
        assert!(g.body == small.as_bytes());
        assert!(g.etag.starts_with("W/"), "{client:?}: {}", g.etag);
        for coding in ["gzip", "br", "zstd"] {
            let g = fetch(client, akit.addr, Method::Get, "/big.js", coding);
            assert_eq!(g.encoding, coding, "{client:?}: from disk");
            assert!(g.body == js(50_000).as_bytes(), "{client:?} {coding}");
            assert!(g.etag.starts_with("W/"), "{client:?}: {}", g.etag);
        }
        // HEAD: the head alone.
        let h = fetch(client, akit.addr, Method::Head, "/big.js", "gzip");
        assert_eq!(h.vary, "Accept-Encoding", "{client:?}");
        assert!(h.body.is_empty());
    }
    drop(akit);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Gets `path` with `accept` until its answer's `Content-Encoding` is `coding`, as fit's `until`:
/// a copy is made on the disk pool after the first request for it.
fn until(addr: SocketAddr, path: &str, accept: &str, coding: &str) -> Got {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let got = Client::H1.send(addr, Method::Get, path, &[("Accept-Encoding", accept)]);
        if got.one("content-encoding").unwrap_or("") == coding {
            return got;
        }
        assert!(
            Instant::now() < deadline,
            "{path} {accept:?}: still {:?}",
            got.one("content-encoding")
        );
        thread::sleep(Duration::from_millis(5));
    }
}

/// fit's `TestCompressCopies`: a file kept in memory goes as it is the first time a client asks
/// for an encoding, and compressed from then on, one copy for each encoding, which decompresses
/// to the file and has its own ETag; and the files, requests and encodings that get the file as
/// it is.
#[test]
fn static_compress_copies() {
    let app = js(20_000);
    let dir = tree(
        "copies",
        &[
            ("app.js", app.as_bytes()),
            ("small.txt", b"short"),
            ("pic.png", app.as_bytes()),
            ("noise.txt", &noise(20_000)),
            ("twin.js", app.as_bytes()),
            ("twin.js.gz", b"gzip-twin"),
            ("big.js", js(50_000).as_bytes()),
        ],
    );
    let akit = Akit::route(
        "[]",
        &format!(
            r#"{{"module": "static", "params": {{"root": "{}", "compress": ["br", "zstd", "gzip"], "precompressed": ["gzip"], "max_file_bytes": 30000}}}}"#,
            dir.display()
        ),
        "",
        akit_handler::MODULES,
    );
    let a = akit.addr;
    let identity = Client::H1.send(a, Method::Get, "/app.js", &[]);
    assert_eq!(identity.one("content-encoding"), None);
    assert!(identity.body == app.as_bytes());
    assert_eq!(identity.one("vary"), Some("Accept-Encoding"));
    let tag = identity.one("etag").unwrap().to_owned();
    for (accept, coding) in [
        ("gzip", "gzip"),
        ("gzip, br, zstd", "br"),
        ("br;q=0.5, zstd", "zstd"),
    ] {
        let got = until(a, "/app.js", accept, coding);
        assert!(decode(coding, &got.body) == app.as_bytes(), "{accept}");
        assert!(
            got.body.len() < app.len() / 2,
            "{accept}: {} bytes",
            got.body.len()
        );
        let own = format!("{}-{coding}\"", tag.trim_end_matches('"'));
        assert_eq!(got.one("etag"), Some(own.as_str()), "{accept}");
        assert_eq!(got.one("accept-ranges"), None, "{accept}");
        assert_eq!(got.one("vary"), Some("Accept-Encoding"), "{accept}");
        assert_eq!(
            got.one("content-type"),
            identity.one("content-type"),
            "{accept}"
        );
        for client in CLIENTS {
            let got = client.send(
                a,
                Method::Get,
                "/app.js",
                &[("Accept-Encoding", accept), ("If-None-Match", &own)],
            );
            assert_eq!(
                got.status, 304,
                "{client:?} {accept}: If-None-Match on the copy"
            );
        }
    }
    // A range is of the file as it is.
    let got = Client::H1.send(
        a,
        Method::Get,
        "/app.js",
        &[("Accept-Encoding", "gzip"), ("Range", "bytes=0-3")],
    );
    assert_eq!(
        (got.status, got.body.as_slice(), got.one("content-encoding")),
        (206, &app.as_bytes()[..4], None)
    );
    // A precompressed twin goes ahead of a copy.
    let got = Client::H2c.send(a, Method::Get, "/twin.js", &[("Accept-Encoding", "gzip")]);
    assert_eq!(got.body, b"gzip-twin");
    // Small files, images, files that do not compress, and files too large to keep go as they
    // are.
    for path in ["/small.txt", "/pic.png", "/noise.txt", "/big.js"] {
        for _ in 0..3 {
            let got = Client::H1.send(a, Method::Get, path, &[("Accept-Encoding", "gzip")]);
            assert_eq!(got.one("content-encoding"), None, "{path}");
            assert_eq!(got.one("vary"), Some("Accept-Encoding"), "{path}");
            thread::sleep(Duration::from_millis(10));
        }
    }
    // Refused, nothing compresses.
    let got = Client::H2c.send(
        a,
        Method::Get,
        "/app.js",
        &[("Accept-Encoding", "gzip;q=0, identity")],
    );
    assert_eq!(got.one("content-encoding"), None);
    drop(akit);
    let _ = std::fs::remove_dir_all(&dir);
}

/// What the config gives the middleware: fit's `TestCompressLevels` and `TestCompressParams`.
fn compress_params(params: &str) -> Result<akit_config::Compress, String> {
    let json = format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
            "routes": [{{"middleware": [{{"module": "compress", "params": {params}}}], "handler": {{"module": "respond"}}}}]}}"#
    );
    let c = akit_config::from_json(&json, None).map_err(|e| e.to_string())?;
    match &c.routes[0].middleware[0] {
        akit_config::Handler::Compress(c) => Ok(c.clone()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn compress_levels_and_params() {
    use akit_config::Encoding::{Br, Gzip, Zstd};
    for (params, want) in [
        ("{}", [1, 1, 1]),
        (r#"{"level": "fastest"}"#, [1, 1, 1]),
        (r#"{"level": "default"}"#, [5, 4, 3]),
        (r#"{"level": "better"}"#, [7, 6, 6]),
        (r#"{"gzip_level": 9}"#, [9, 1, 1]),
        (
            r#"{"level": "better", "brotli_level": 11, "zstd_level": 22}"#,
            [7, 11, 22],
        ),
    ] {
        let c = compress_params(params).unwrap();
        assert_eq!(
            [c.gzip_level, c.brotli_level, c.zstd_level],
            want,
            "{params}"
        );
    }
    let c = compress_params("{}").unwrap();
    assert_eq!((c.min_bytes, c.max_in_flight), (1400, 1024));
    assert_eq!(c.encodings, [Zstd, Gzip, Br]);
    let c = compress_params(r#"{"min_length": 10, "encodings": ["br"]}"#).unwrap();
    assert_eq!(
        (c.min_bytes, c.encodings.as_slice()),
        (10, [Br].as_slice()),
        "the old name"
    );
    for (params, err) in [
        (
            r#"{"encodings": ["deflate"]}"#,
            r#""deflate" is not one of"#,
        ),
        (r#"{"encodings": ["gzip", "gzip"]}"#, "listed twice"),
        (r#"{"level": "max"}"#, r#""max" is not one of"#),
        (r#"{"gzip_level": 10}"#, "gzip_level"),
        (r#"{"brotli_level": 0.5}"#, "brotli_level"),
        (r#"{"zstd_level": 23}"#, "zstd_level"),
        (r#"{"min_bytes": 1, "min_length": 2}"#, "min_length"),
        (r#"{"max_in_flight": -1}"#, "max_in_flight"),
        (r#"{"other": 1}"#, "unknown field"),
    ] {
        let e = compress_params(params).unwrap_err();
        assert!(e.contains(err), "{params}: {e}");
    }
}
