//! fit's proxy conformance tests over h2 and h2c upstreams (I18): the tests fit runs for each of
//! its upstream protocols (`eachProto` in `handler/proxy`), ported for `"protocol": "h2c"`, from
//! clients over HTTP/1.1 and h2c: `TestForwarding`, `TestHopByHopRequest`,
//! `TestForwardedUntrusted`, `TestForwardedTrusted`, `TestHostHeader`, `TestManyRequests`,
//! `TestHead`, `TestPassiveHealth`, `TestStreamDownload`, `TestStreamUpload`, `TestTrailers`,
//! `TestUpstreamBreaksMidBody` and `TestClientAbort`, and the proxy's `timeout`; fit's
//! `TestResponseHeaderTimeout`, `TestResponseIdleTimeout` and `TestClientAbort`'s client gone
//! before the head (I17b; akio heeds them over h2 since its ADR-111, akio API request A66); and
//! one exchange over h2, with TLS, through a pool given the test CA (the `tls_*` fields, I54,
//! are tested in `upstream_tls.rs`).
//!
//! I18b's, on akio's A51–A53 and A66: request trailers to an h2 upstream (`TestTrailers`, the
//! request's way), a slow client holding an h2 download rather than having it cut short, and an
//! `h2` upstream whose server chooses HTTP/1.1 in ALPN spoken to over HTTP/1.1.
//!
//! The upstream is akio-http's own server, HTTP/1.1 and h2c with prior knowledge on one port,
//! which reports each request as it arrived. On the backend `AKIO_TEST_BACKEND` names (epoll by
//! default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

#[path = "../../akit-listener/tests/support/mod.rs"]
mod support;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, Listener, UringConfig};
use akio_http::client::ClientRequest;
use akio_http::h2client::{H2Client, H2Event, H2Options};
use akio_http::{BodyEvent, BodyMode, Context, Headers, Method, Request, Version};
use akit_runtime::upstream::{EngineSlot, Pool};
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};

fn backend() -> Backend {
    match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    }
}

fn options() -> akit_listener::Options {
    let mut o = akit_listener::Options::default();
    o.backend = backend();
    o.shutdown_timeout = Duration::from_secs(5);
    o
}

fn registry() -> Registry<HandlerParams> {
    Registry::new(&[akit_handler::MODULES, akit_proxy::MODULES]).unwrap()
}

/// akit serving `json`, as `akit run` does.
struct Akit {
    engine: Arc<Engine>,
    addr: SocketAddr,
    _cell: GraphCell,
}

impl Akit {
    fn start(json: &str) -> Akit {
        let config = akit_config::from_json(json, None).unwrap();
        let slot = EngineSlot::new();
        let cell = GraphCell::new(compile(&config, &registry(), &Env::new(slot.clone())).unwrap());
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

    /// The proxy to the h2c upstream `addresses`, with the proxy's `params` and the upstream's
    /// `fields`.
    fn proxy(addresses: &[String], params: &str, fields: &str) -> Akit {
        let list: Vec<String> = addresses.iter().map(|a| format!("{a:?}")).collect();
        Akit::start(&format!(
            r#"{{
  "listeners": [{{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}}],
  "upstreams": [{{"name": "app", "addresses": [{}], "protocol": "h2c"{fields}}}],
  "routes": [{{"handler": {{"module": "proxy", "params": {{"upstream": "app"{params}}}}}}}]
}}"#,
            list.join(", ")
        ))
    }
}

impl Drop for Akit {
    fn drop(&mut self) {
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
    }
}

/// A request as the upstream saw it.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    target: String,
    version: Version,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    trailers: Vec<(String, String)>,
}

impl Seen {
    fn values(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .collect()
    }

    fn one(&self, name: &str) -> Option<&str> {
        self.values(name).first().copied()
    }
}

type Reply = dyn Fn(&mut Context, &Seen) + Send + Sync;

/// The upstream's service: every request's body streams to it, and is reported once whole, on
/// the channel; then `reply` answers it.
struct Service {
    seen: Sender<Seen>,
    reply: Arc<Reply>,
}

impl akio_http::Service for Service {
    fn call(&self, ctx: &mut Context) {
        let req = ctx.req();
        let mut seen = Seen {
            method: req.method.as_str().to_owned(),
            target: req.target.clone(),
            version: req.version,
            headers: req
                .headers
                .iter()
                .map(|(n, v)| (n.to_owned(), v.to_owned()))
                .collect(),
            body: Vec::new(),
            trailers: Vec::new(),
        };
        let (tx, reply) = (self.seen.clone(), self.reply.clone());
        ctx.on_body(move |ctx, ev| match ev {
            BodyEvent::Chunk(b) => seen.body.extend_from_slice(b),
            BodyEvent::End(t) => {
                seen.trailers = t
                    .iter()
                    .map(|(n, v)| (n.to_owned(), v.to_owned()))
                    .collect();
                let _ = tx.send(seen.clone());
                reply(ctx, &seen);
            }
        });
    }

    fn body_mode(&self, _: &Request) -> BodyMode {
        BodyMode::Stream
    }
}

/// An upstream on akio-http's server, answering with `reply`; the requests it saw arrive on the
/// receiver.
struct Upstream {
    engine: Arc<Engine>,
    seen: std::sync::mpsc::Receiver<Seen>,
}

impl Upstream {
    fn start(reply: impl Fn(&mut Context, &Seen) + Send + Sync + 'static) -> Upstream {
        let (tx, rx) = channel();
        let service = Service {
            seen: tx,
            reply: Arc::new(reply),
        };
        Upstream {
            engine: serve(akio_http::serve(service)),
            seen: rx,
        }
    }

    fn addr(&self) -> String {
        self.engine.local_addrs()[0].to_string()
    }

    fn next(&self) -> Seen {
        self.seen.recv_timeout(Duration::from_secs(10)).unwrap()
    }
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.engine.shutdown();
    }
}

fn serve(acceptor: impl akio::Acceptor) -> Arc<Engine> {
    let e = Engine::builder()
        .listen(
            Listener::tcp("127.0.0.1:0".parse().unwrap())
                .backend(backend())
                .threads(2)
                .serve(acceptor),
        )
        .client_threads(1)
        .build()
        .unwrap();
    e.start().unwrap();
    Arc::new(e)
}

/// An engine for the tests' h2c clients.
fn clients() -> Arc<Engine> {
    static E: std::sync::OnceLock<Arc<Engine>> = std::sync::OnceLock::new();
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

    fn trailer(&self, name: &str) -> Option<&str> {
        self.trailers
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
    if head.starts_with("HEAD ") {
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
    fn get(self, addr: SocketAddr, path: &str, fields: &[(&str, &str)]) -> Got {
        self.send(addr, Method::Get, path, fields, b"")
    }

    fn send(
        self,
        addr: SocketAddr,
        method: Method,
        path: &str,
        fields: &[(&str, &str)],
        body: &[u8],
    ) -> Got {
        match self {
            Client::H2c => h2(addr, method, path, fields, body),
            Client::H1 => {
                let mut head = format!("{} {path} HTTP/1.1\r\nHost: {addr}", method.as_str());
                for (n, v) in fields {
                    head.push_str(&format!("\r\n{n}: {v}"));
                }
                if !body.is_empty() {
                    head.push_str(&format!("\r\nContent-Length: {}", body.len()));
                }
                h1(addr, &head, body)
            }
        }
    }
}

/// A body of `n` bytes no two neighbouring KiB of which are alike.
fn pattern(n: usize) -> Vec<u8> {
    (0..n)
        .map(|i| ((i % 251) ^ ((i >> 10) % 256)).to_le_bytes()[0])
        .collect()
}

/// fit's `TestForwarding`: method, target, fields and body up; status, fields and body back; the
/// request arrives over HTTP/2.
#[test]
fn forwarding() {
    let up = Upstream::start(|ctx, seen| {
        let mut resp = akio_http::Response::new(201);
        resp.headers.append("X-Up", "yes");
        resp.body = format!("got {}", seen.body.len()).into_bytes();
        ctx.respond(resp);
    });
    let akit = Akit::proxy(&[up.addr()], "", "");
    for client in CLIENTS {
        let got = client.send(
            akit.addr,
            Method::Post,
            "/a/b?c=d",
            &[("User-Agent", "fit-test"), ("X-Custom", "1")],
            b"hello",
        );
        assert_eq!(got.status, 201, "{client:?}: {got:?}");
        assert_eq!(got.one("x-up"), Some("yes"), "{client:?}");
        assert_eq!(got.body, b"got 5", "{client:?}");
        let seen = up.next();
        assert_eq!(seen.method, "POST", "{client:?}");
        assert_eq!(seen.target, "/a/b?c=d", "{client:?}");
        assert_eq!(seen.version, Version::Http2, "{client:?}");
        assert_eq!(seen.body, b"hello", "{client:?}");
        assert_eq!(seen.one("x-custom"), Some("1"), "{client:?}");
        assert_eq!(seen.one("user-agent"), Some("fit-test"), "{client:?}");
        assert_eq!(seen.one("x-forwarded-for"), Some("127.0.0.1"), "{client:?}");
    }
}

/// fit's `TestHopByHopRequest`: the client's hop-by-hop fields, and those its `Connection`
/// names, stay with the proxy (an HTTP/1.1 client alone can send them).
#[test]
fn hop_by_hop_request() {
    let up = Upstream::start(|ctx, _| ctx.text(200, "ok"));
    let akit = Akit::proxy(&[up.addr()], "", "");
    let got = Client::H1.get(
        akit.addr,
        "/",
        &[
            ("X-Drop", "1"),
            ("Connection", "X-Drop"),
            ("Keep-Alive", "timeout=5"),
            ("Proxy-Authorization", "Basic x"),
            ("X-Keep", "1"),
        ],
    );
    assert_eq!(got.status, 200, "{got:?}");
    let seen = up.next();
    for name in [
        "x-drop",
        "connection",
        "keep-alive",
        "proxy-authorization",
        "te",
    ] {
        assert_eq!(seen.one(name), None, "{name} reached the upstream");
    }
    assert_eq!(seen.one("x-keep"), Some("1"));
}

/// fit's `TestForwardedUntrusted` and `TestForwardedTrusted`.
#[test]
fn forwarded_fields() {
    let up = Upstream::start(|ctx, _| ctx.text(200, "ok"));
    let untrusted = Akit::proxy(&[up.addr()], "", "");
    let trusted = Akit::proxy(&[up.addr()], r#", "trusted_proxies": ["127.0.0.1/32"]"#, "");
    for client in CLIENTS {
        let fields = [
            ("X-Forwarded-For", "1.2.3.4"),
            ("X-Forwarded-Proto", "https"),
        ];
        assert_eq!(client.get(untrusted.addr, "/", &fields).status, 200);
        let seen = up.next();
        assert_eq!(seen.one("x-forwarded-for"), Some("127.0.0.1"), "{client:?}");
        assert_eq!(seen.one("x-forwarded-proto"), Some("http"), "{client:?}");
        assert_eq!(client.get(trusted.addr, "/", &fields).status, 200);
        let seen = up.next();
        assert_eq!(
            seen.one("x-forwarded-for"),
            Some("1.2.3.4, 127.0.0.1"),
            "{client:?}"
        );
        assert_eq!(seen.one("x-forwarded-proto"), Some("https"), "{client:?}");
    }
}

/// fit's `TestHostHeader`: the client's `Host` (HTTP/2's `:authority`) reaches the upstream,
/// unless `host_header` rewrites it.
#[test]
fn host_header() {
    let up = Upstream::start(|ctx, _| ctx.text(200, "ok"));
    let passed = Akit::proxy(&[up.addr()], "", "");
    let rewritten = Akit::proxy(&[up.addr()], r#", "host_header": "backend.internal""#, "");
    for client in CLIENTS {
        let got = client.get(passed.addr, "/", &[]);
        assert_eq!(got.status, 200);
        let seen = up.next();
        let want = passed.addr.to_string();
        assert_eq!(seen.one("host"), Some(want.as_str()), "{client:?}");
        assert_eq!(client.get(rewritten.addr, "/", &[]).status, 200);
        assert_eq!(
            up.next().one("host"),
            Some("backend.internal"),
            "{client:?}"
        );
    }
}

/// fit's `TestManyRequests`: many at once, from many h2c clients, each answered with its own.
#[test]
fn many_requests() {
    let up = Upstream::start(|ctx, seen| ctx.text(200, seen.target.clone()));
    let akit = Akit::proxy(&[up.addr()], "", "");
    let addr = akit.addr;
    let threads: Vec<_> = (0..16)
        .map(|t| {
            thread::spawn(move || {
                for i in 0..8 {
                    let client = if (t + i) % 2 == 0 {
                        Client::H1
                    } else {
                        Client::H2c
                    };
                    let path = format!("/n/{t}/{i}");
                    let got = client.get(addr, &path, &[]);
                    assert_eq!(got.status, 200);
                    assert_eq!(got.body, path.as_bytes());
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
}

/// fit's `TestHead`: a HEAD gets the upstream's fields, its length among them, and no body.
#[test]
fn head() {
    let up = Upstream::start(|ctx, _| ctx.text(200, "twelve bytes"));
    let akit = Akit::proxy(&[up.addr()], "", "");
    for client in CLIENTS {
        let got = client.send(akit.addr, Method::Head, "/", &[], b"");
        assert_eq!(got.status, 200, "{client:?}");
        assert_eq!(got.one("content-length"), Some("12"), "{client:?}");
        assert!(got.body.is_empty(), "{client:?}");
        assert_eq!(up.next().method, "HEAD");
    }
}

/// fit's `TestPassiveHealth`: an address that refuses connections fails its request (502) and
/// is taken out of rotation; the others take the rest.
#[test]
fn passive_health() {
    let up = Upstream::start(|ctx, _| ctx.text(200, "up"));
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().to_string()
    };
    let akit = Akit::proxy(&[dead, up.addr()], "", r#", "max_fails": 1"#);
    let codes: Vec<u16> = (0..6)
        .map(|_| Client::H1.get(akit.addr, "/", &[]).status)
        .collect();
    assert_eq!(codes.iter().filter(|&&c| c == 502).count(), 1, "{codes:?}");
    assert_eq!(codes.iter().filter(|&&c| c == 200).count(), 5, "{codes:?}");
}

/// A response streamed by the upstream: `chunks` of `size` bytes, `every` apart, with
/// `trailers` after them, of no declared length.
fn stream_reply(ctx: &mut Context, chunks: usize, size: usize, trailers: Option<Headers>) {
    let mut fields = Headers::new();
    if trailers.is_some() {
        fields.append("Trailer", "X-Sum");
    }
    let Some(s) = ctx.stream_response(200, fields) else {
        return;
    };
    thread::spawn(move || {
        let body = pattern(size * chunks);
        for piece in body.chunks(size) {
            if s.write(piece).is_err() {
                return;
            }
            while s.queued_bytes() > 1 << 20 {
                thread::sleep(Duration::from_millis(1));
            }
        }
        match trailers {
            Some(t) => s.finish_with_trailers(t),
            None => s.finish(),
        }
    });
}

/// fit's `TestStreamDownload`: a large response of no declared length, streamed through whole.
#[test]
fn stream_download() {
    let up = Upstream::start(|ctx, _| stream_reply(ctx, 128, 32 << 10, None));
    let akit = Akit::proxy(&[up.addr()], "", "");
    for client in CLIENTS {
        let got = client.get(akit.addr, "/big", &[]);
        assert_eq!(got.status, 200, "{client:?}");
        assert!(got.complete, "{client:?}");
        assert_eq!(got.body.len(), 4 << 20, "{client:?}");
        assert!(got.body == pattern(4 << 20), "{client:?}: the body differs");
    }
}

/// fit's `TestStreamUpload`: a large request body streamed to the upstream whole.
#[test]
fn stream_upload() {
    let up = Upstream::start(|ctx, seen| ctx.text(200, seen.body.len().to_string()));
    let akit = Akit::proxy(&[up.addr()], "", "");
    let body = pattern(3 << 20);
    for client in CLIENTS {
        let got = client.send(akit.addr, Method::Post, "/up", &[], &body);
        assert_eq!(got.status, 200, "{client:?}: {:?}", got.headers);
        assert_eq!(got.body, b"3145728", "{client:?}");
        let seen = up.next();
        assert!(seen.body == body, "{client:?}: the body differs");
    }
}

/// fit's `TestTrailers`, the response's way: the upstream's trailers reach the client.
#[test]
fn response_trailers() {
    let up = Upstream::start(|ctx, _| {
        let mut t = Headers::new();
        t.append("X-Sum", "42");
        stream_reply(ctx, 4, 1 << 10, Some(t));
    });
    let akit = Akit::proxy(&[up.addr()], "", "");
    for client in CLIENTS {
        let got = client.get(akit.addr, "/t", &[]);
        assert_eq!(got.status, 200, "{client:?}");
        assert!(got.complete, "{client:?}");
        assert_eq!(got.body.len(), 4 << 10, "{client:?}");
        assert_eq!(
            got.trailer("x-sum"),
            Some("42"),
            "{client:?}: {:?}",
            got.headers
        );
    }
}

/// fit's `TestTrailers`, the request's way: a request's announced trailers reach the upstream.
#[test]
fn request_trailers() {
    let up = Upstream::start(|ctx, _| ctx.text(200, "ok"));
    let akit = Akit::proxy(&[up.addr()], "", "");
    let got = h1(
        akit.addr,
        &format!(
            "POST / HTTP/1.1\r\nHost: {}\r\nTransfer-Encoding: chunked\r\nTrailer: X-Sum",
            akit.addr
        ),
        b"5\r\nhello\r\n0\r\nX-Sum: 42\r\n\r\n",
    );
    assert_eq!(got.status, 200);
    let seen = up.next();
    assert_eq!(seen.body, b"hello");
    assert_eq!(seen.trailers, [("x-sum".to_owned(), "42".to_owned())]);
}

/// fit's `TestUpstreamBreaksMidBody`: a body cut short by the upstream reaches the client cut
/// short: an HTTP/1.1 connection closed early, an h2 stream ended with fit's trailer.
#[test]
fn upstream_breaks_mid_body() {
    let up = Upstream::start(|ctx, _| {
        let Some(s) = ctx.stream_response(200, Headers::new()) else {
            return;
        };
        thread::spawn(move || {
            let _ = s.write(&pattern(100 << 10));
            thread::sleep(Duration::from_millis(50));
            // Dropped unfinished: akio resets the stream.
            drop(s);
        });
    });
    let akit = Akit::proxy(&[up.addr()], "", r#", "max_fails": 0"#);
    let got = Client::H1.get(akit.addr, "/", &[]);
    assert_eq!(got.status, 200);
    assert!(
        !got.complete,
        "{} bytes arrived as complete",
        got.body.len()
    );
    let got = Client::H2c.get(akit.addr, "/", &[]);
    assert_eq!(got.status, 200);
    assert_eq!(
        got.trailer("fit-error"),
        Some("upstream failed mid-body"),
        "{:?} after {} bytes",
        got.trailers,
        got.body.len()
    );
}

/// fit's `TestClientAbort`: a client that goes mid-download has the upstream's stream given up:
/// the upstream's writes fail once akit closes the connection that carried it.
#[test]
fn client_abort() {
    let failed = Arc::new(AtomicBool::new(false));
    let started = Arc::new(AtomicUsize::new(0));
    let (f, n) = (failed.clone(), started.clone());
    let up = Upstream::start(move |ctx, _| {
        let Some(s) = ctx.stream_response(200, Headers::new()) else {
            return;
        };
        n.fetch_add(1, Ordering::SeqCst);
        let f = f.clone();
        thread::spawn(move || {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(20) {
                if s.write(&[b'x'; 1024]).is_err() {
                    f.store(true, Ordering::SeqCst);
                    return;
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
    });
    let akit = Akit::proxy(&[up.addr()], "", "");
    let mut s = TcpStream::connect(akit.addr).unwrap();
    s.write_all(format!("GET /forever HTTP/1.1\r\nHost: {}\r\n\r\n", akit.addr).as_bytes())
        .unwrap();
    let mut buf = [0; 4096];
    let _ = s.read(&mut buf).unwrap();
    assert_eq!(started.load(Ordering::SeqCst), 1);
    drop(s);
    let start = Instant::now();
    while !failed.load(Ordering::SeqCst) {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the upstream's stream was not given up"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

/// The proxy's `timeout`: past it before the response began, 504.
#[test]
fn timeout() {
    let held = Arc::new(Mutex::new(Vec::new()));
    let h = held.clone();
    let up = Upstream::start(move |ctx, _| {
        if let Some(d) = ctx.detach() {
            h.lock().unwrap().push(d);
        }
    });
    let akit = Akit::proxy(&[up.addr()], r#", "timeout": "300ms""#, "");
    for client in CLIENTS {
        let start = Instant::now();
        let got = client.get(akit.addr, "/slow", &[]);
        assert_eq!(got.status, 504, "{client:?}");
        assert!(start.elapsed() < Duration::from_secs(5), "{client:?}");
    }
    held.lock().unwrap().clear();
}

/// A client that takes nothing of a large h2 download for a while holds it (akio keeps the
/// stream's window closed, I18b), and gets all of it once it reads; the memory's bound is
/// `stream.rs`'s.
#[test]
fn a_slow_client_holds_the_h2_download() {
    let up = Upstream::start(|ctx, _| stream_reply(ctx, 1024, 32 << 10, None));
    let akit = Akit::proxy(&[up.addr()], "", r#", "max_fails": 0"#);
    let mut s = TcpStream::connect(akit.addr).unwrap();
    s.write_all(format!("GET /huge HTTP/1.1\r\nHost: {}\r\n\r\n", akit.addr).as_bytes())
        .unwrap();
    // Nothing read for a while: the download is held.
    thread::sleep(Duration::from_secs(2));
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut r = BufReader::new(s);
    let mut head = String::new();
    loop {
        let mut line = String::new();
        assert!(r.read_line(&mut line).unwrap() > 0, "no head");
        head.push_str(&line);
        if line == "\r\n" {
            break;
        }
    }
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    // Chunked: count what the chunks carry.
    let mut total = 0usize;
    loop {
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        let n = usize::from_str_radix(line.trim(), 16).unwrap();
        if n == 0 {
            break;
        }
        let mut buf = vec![0; n + 2];
        r.read_exact(&mut buf).unwrap();
        total += n;
    }
    assert_eq!(total, 1024 * (32 << 10), "the whole body, not cut short");
}

/// An `h2` upstream whose server chooses HTTP/1.1 in ALPN (I18b, as fit's `h2` offers both): the
/// pool speaks HTTP/1.1 over TLS to it, the request that waited for the dial included.
#[test]
fn h2_falls_back_to_http1_when_alpn_chooses_it() {
    let d = support::certs::dir("akit-proxy-h2-fallback");
    let ca = support::certs::Ca::new("akit test CA");
    let ca_file = ca.write(&d, "ca.pem");
    let (cert, key) = ca.leaf(&d, "leaf", &["localhost", "127.0.0.1"]);
    let scfg = akio::tls::ServerConfig::builder(
        akio::tls::Certificate::from_pem(&std::fs::read(cert).unwrap()).unwrap(),
        akio::tls::PrivateKey::from_pem(&std::fs::read(key).unwrap()).unwrap(),
    )
    .alpn(["http/1.1"])
    .build()
    .unwrap();
    let up = serve(akio::tls::Tls::server(
        scfg,
        akio_http::serve(|ctx: &mut Context| {
            let v = if ctx.req().version == Version::Http2 {
                "h2"
            } else {
                "h1"
            };
            ctx.text(200, format!("{v} {}", ctx.req().target));
        }),
    ));
    let addr = up.local_addrs()[0].to_string();
    let config = akit_config::from_json(
        &format!(
            r#"{{"upstreams": [{{"name": "app", "addresses": ["{addr}"], "protocol": "h2"}}]}}"#
        ),
        None,
    )
    .unwrap();
    let tls = support::https::client(&ca_file, &["h2", "http/1.1"], None);
    let pool = Pool::new(&config.upstreams[0], EngineSlot::with(clients())).with_tls(tls);
    for path in ["/first", "/second"] {
        let (tx, rx) = channel();
        pool.request(ClientRequest::get(path), move |r| {
            let _ = tx.send(r.map(|r| String::from_utf8_lossy(&r.body).into_owned()));
        });
        let got = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(got.unwrap(), format!("h1 {path}"));
    }
    assert_eq!(pool.h2_connections(0), Some((0, 0)));
    assert_eq!(pool.backend_requests(0), (2, 0));
    up.shutdown();
}

/// The request's way over h2 with TLS: a pool of `"protocol": "h2"` given the test CA.
#[test]
fn h2_over_tls() {
    let d = support::certs::dir("akit-proxy-h2-tls");
    let ca = support::certs::Ca::new("akit test CA");
    let ca_file = ca.write(&d, "ca.pem");
    let (cert, key) = ca.leaf(&d, "leaf", &["localhost", "127.0.0.1"]);
    let scfg = akio::tls::ServerConfig::builder(
        akio::tls::Certificate::from_pem(&std::fs::read(cert).unwrap()).unwrap(),
        akio::tls::PrivateKey::from_pem(&std::fs::read(key).unwrap()).unwrap(),
    )
    .alpn(["h2"])
    .build()
    .unwrap();
    let up = serve(akio::tls::Tls::server(
        scfg,
        akio_http::serve(|ctx: &mut Context| {
            let v = if ctx.req().version == Version::Http2 {
                "h2"
            } else {
                "h1"
            };
            ctx.text(200, format!("{v} {}", ctx.req().target));
        }),
    ));
    let addr = up.local_addrs()[0].to_string();
    let config = akit_config::from_json(
        &format!(
            r#"{{"upstreams": [{{"name": "app", "addresses": ["{addr}"], "protocol": "h2"}}]}}"#
        ),
        None,
    )
    .unwrap();
    let tls = support::https::client(&ca_file, &["h2"], None);
    let pool = Pool::new(&config.upstreams[0], EngineSlot::with(clients())).with_tls(tls);
    let (tx, rx) = channel();
    pool.request(ClientRequest::get("/secure"), move |r| {
        let _ = tx.send(r.map(|r| String::from_utf8_lossy(&r.body).into_owned()));
    });
    let got = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(got.unwrap(), "h2 /secure");
    up.shutdown();
}

/// fit's `TestResponseHeaderTimeout` (I17b): an h2c upstream that does not begin its response
/// within `response_header_timeout` is answered 504.
#[test]
fn response_header_timeout() {
    let held = Arc::new(Mutex::new(Vec::new()));
    let h = held.clone();
    let up = Upstream::start(move |ctx, _| {
        if let Some(d) = ctx.detach() {
            h.lock().unwrap().push(d);
        }
    });
    let akit = Akit::proxy(&[up.addr()], "", r#", "response_header_timeout": "150ms""#);
    for client in CLIENTS {
        let start = Instant::now();
        let got = client.get(akit.addr, "/slow", &[]);
        let took = start.elapsed();
        assert_eq!(got.status, 504, "{client:?}");
        assert!(
            took >= Duration::from_millis(100) && took < Duration::from_secs(3),
            "{client:?}: {took:?}"
        );
    }
    held.lock().unwrap().clear();
}

/// fit's `TestResponseIdleTimeout` (I17b): an h2c upstream silent mid-body past
/// `response_idle_timeout` has its response cut short, as one that broke off.
#[test]
fn response_idle_timeout() {
    let held = Arc::new(Mutex::new(Vec::new()));
    let h = held.clone();
    let up = Upstream::start(move |ctx, seen| {
        let mut fields = Headers::new();
        if seen.target.contains("length=1") {
            fields.append("Content-Length", "1000000");
        }
        let Some(s) = ctx.stream_response(200, fields) else {
            return;
        };
        let _ = s.write(&pattern(200 << 10));
        h.lock().unwrap().push(s);
    });
    let akit = Akit::proxy(
        &[up.addr()],
        "",
        r#", "response_idle_timeout": "200ms", "max_fails": 0"#,
    );
    for client in CLIENTS {
        for path in ["/?length=1", "/"] {
            let start = Instant::now();
            let got = client.get(akit.addr, path, &[]);
            let took = start.elapsed();
            assert_eq!(got.status, 200, "{client:?} {path}");
            assert!(got.body.len() >= 200 << 10, "{client:?} {path}");
            assert!(took < Duration::from_secs(3), "{client:?} {path}: {took:?}");
            if matches!(client, Client::H2c) && path == "/" {
                assert_eq!(
                    got.trailer("fit-error"),
                    Some("upstream failed mid-body"),
                    "{client:?} {path}"
                );
            } else {
                assert!(
                    !got.complete,
                    "{client:?} {path}: a body cut short read as whole"
                );
            }
        }
    }
    held.lock().unwrap().clear();
}

/// The rest of fit's `TestClientAbort` (I17b): a client that goes before the response's head has
/// the h2c upstream's request given up (its connection closed, akio's h2 client being unable to
/// reset the stream: A51).
#[test]
fn client_abort_before_head() {
    let held = Arc::new(Mutex::new(Vec::new()));
    let (tx, gone) = channel();
    let tx = Mutex::new(tx);
    let h = held.clone();
    let up = Upstream::start(move |ctx, _| {
        let tx = tx.lock().unwrap().clone();
        ctx.on_gone(move || {
            let _ = tx.send(());
        });
        if let Some(d) = ctx.detach() {
            h.lock().unwrap().push(d);
        }
    });
    let akit = Akit::proxy(&[up.addr()], "", "");
    {
        let mut s = TcpStream::connect(akit.addr).unwrap();
        s.write_all(format!("GET /hang HTTP/1.1\r\nHost: {}\r\n\r\n", akit.addr).as_bytes())
            .unwrap();
        thread::sleep(Duration::from_millis(200));
    }
    gone.recv_timeout(Duration::from_secs(5))
        .expect("the upstream request was not given up (HTTP/1.1)");
    {
        let c = H2Client::dial_blocking(&clients(), akit.addr, None, H2Options::default()).unwrap();
        let up = c.open(
            ClientRequest::get(&format!("http://{}/hang", akit.addr)),
            |_| {},
        );
        up.finish();
        thread::sleep(Duration::from_millis(200));
        c.close();
    }
    gone.recv_timeout(Duration::from_secs(5))
        .expect("the upstream request was not given up (h2c)");
    held.lock().unwrap().clear();
}
