//! Proxy-Wasm filters through the `wasm` middleware and akit's listener on akio, on the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`), with fit's tests
//! (`plugin/wasm/proxywasm_test.go`, `response_test.go`, `respcall_test.go`) ported.
//!
//! The filters are fit's test filters, `testdata/proxywasm`, built with Go for wasip1 and the
//! Proxy-Wasm Go SDK (`tests/guests/pw-*.wasm.gz`; see `tests/guests/README.md`), run unchanged:
//! the SDK's `http_routing`, `properties`, `http_headers` and `http_body` examples, written for
//! Envoy, and fit's own `response`. Filters that misbehave in ways the SDK would not let them are
//! encoded by the tests (`akit_plugin::encode`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, UringConfig};
use akit_plugin::encode::{self, Func};
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

/// The tests' handler, by path, as fit's tests' handlers: `/host` answers with the request's
/// host (fit's `host`), `/echo` with `[initial body]` and its `Test` field (`echoBody`),
/// `/streamed` 201 in three writes with the trailer `X-Upstream` (`streamed`), `/target` with
/// the request's method, target and `X-From` field, `/204` with 204,
/// `/long` with 100 bytes, `/held` once the test releases it (`TestProxyWasmMaxStreams`'s), and
/// anything else with `ok`.
mod app {
    use std::sync::{Arc, Mutex};

    use akio_http::{Context, Headers, Response};
    use akit_runtime::{Handler, HandlerParams, Info, Provision, ProvisionError, Provisioned};

    /// The `/held` requests waiting to be answered.
    pub(crate) static HELD: Mutex<Vec<akio_http::Detached>> = Mutex::new(Vec::new());

    struct App;

    impl akio_http::Service for App {
        fn call(&self, ctx: &mut Context) {
            let text = |s: &str| Response::with_body(200, "text/plain", s.as_bytes().to_vec());
            match ctx.path() {
                "/host" => {
                    let host = ctx.header("host").unwrap_or("").to_owned();
                    ctx.respond(text(&host));
                }
                "/echo" => {
                    let t = ctx.header("test").unwrap_or("").to_owned();
                    ctx.respond(text(&format!("[initial body]{t}")));
                }
                "/streamed" => {
                    let mut h = Headers::new();
                    h.append("Content-Type", "text/plain");
                    h.append("Trailer", "X-Upstream");
                    let Some(s) = ctx.stream_response(201, h) else {
                        return;
                    };
                    for part in ["one ", "two ", "three"] {
                        let _ = s.write(part.as_bytes());
                    }
                    let mut t = Headers::new();
                    t.append("X-Upstream", "up");
                    s.finish_with_trailers(t);
                }
                "/target" => {
                    let r = ctx.req();
                    let body = format!(
                        "{} {} {}",
                        r.method.as_str(),
                        r.target,
                        r.headers.get("x-from").unwrap_or("")
                    );
                    ctx.respond(text(&body));
                }
                "/big" => {
                    let mut h = Headers::new();
                    h.append("Content-Type", "text/plain");
                    if let Some(s) = ctx.stream_response(200, h) {
                        pump(Arc::new(Mutex::new(Some(s))), BIG);
                    }
                }
                "/204" => ctx.respond(Response::new(204)),
                "/long" => ctx.respond(text(&"a".repeat(100))),
                "/held" => {
                    if let Some(d) = ctx.detach() {
                        HELD.lock().unwrap().push(d);
                    }
                }
                _ => ctx.respond(text("ok")),
            }
        }
    }

    impl Handler for App {}

    /// The size of `/big`'s body.
    pub(crate) const BIG: usize = 4 << 20;

    /// Writes `left` more bytes of `/big`'s body in 64 KiB parts, paced as a proxy paces an
    /// upstream: past 256 KiB queued it waits for the stream to drain.
    fn pump(s: Arc<Mutex<Option<akio_http::BodyStream>>>, mut left: usize) {
        let part = vec![b'a'; 64 << 10];
        let mut g = s.lock().unwrap();
        loop {
            let Some(st) = g.as_ref() else {
                return;
            };
            if left == 0 {
                if let Some(st) = g.take() {
                    st.finish();
                }
                return;
            }
            if st.queued_bytes() > 256 << 10 {
                let again = Arc::clone(&s);
                st.on_drain(move || pump(again, left));
                return;
            }
            let n = left.min(part.len());
            if st.write(&part[..n]).is_err() {
                g.take();
                return;
            }
            left -= n;
        }
    }

    fn new(_: &Provision<'_, HandlerParams>) -> Result<Provisioned<dyn Handler>, ProvisionError> {
        Ok(Provisioned::new(Arc::new(App) as Arc<dyn Handler>))
    }

    pub(crate) static MODULES: &[Info<HandlerParams>] = &[Info::handler(
        "http.handlers.respond",
        "The tests' app.",
        new,
    )];
}

fn registry() -> Registry<HandlerParams> {
    Registry::new(&[app::MODULES, akit_plugin::MODULES]).unwrap()
}

/// A file of the tests' own, under the target's temporary directory.
fn temp_file(name: &str, bytes: &[u8]) -> String {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("akit-plugin");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!(
        "{}-{}-{name}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, bytes).unwrap();
    path.to_str().unwrap().to_owned()
}

/// fit's test filter `name`, inflated once.
fn filter(name: &str) -> String {
    static PATHS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
    let mut paths = PATHS.lock().unwrap();
    if let Some((_, p)) = paths.iter().find(|(n, _)| n == name) {
        return p.clone();
    }
    let gz = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/guests/pw-{name}.wasm.gz")),
    )
    .unwrap();
    let wasm = akio_deflate::decompress(akio_deflate::Format::Gzip, &gz, 64 << 20).unwrap();
    let p = temp_file(&format!("{name}.wasm"), &wasm);
    paths.push((name.to_owned(), p.clone()));
    p
}

/// akit serving `json`, as `akit run` does.
struct Akit {
    engine: Arc<Engine>,
    addr: SocketAddr,
    _cell: GraphCell,
}

impl Akit {
    fn start(json: &str) -> Akit {
        let config = akit_config::from_json(json, None).unwrap_or_else(|e| panic!("{json}: {e}"));
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

    /// A route to the app behind the `wasm` middleware running `path` with `params` besides
    /// (JSON members, each followed by a comma, or nothing).
    fn plugin(path: &str, params: &str) -> Akit {
        Akit::start(&config(path, params))
    }

    /// fit's test filter `name` with one instance (fit's `loadOne`), unless `params` says.
    fn filter(name: &str, params: &str) -> Akit {
        let params = if params.contains("\"instances\"") {
            params.to_owned()
        } else {
            format!(r#""instances": 1, {params}"#)
        };
        Akit::plugin(&filter(name), &params)
    }
}

impl Drop for Akit {
    fn drop(&mut self) {
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
    }
}

fn config(path: &str, params: &str) -> String {
    format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}}],
            "routes": [{{"middleware": [{{"module": "wasm", "params": {{{params} "path": "{path}"}}}}],
                         "handler": {{"module": "respond"}}}}]}}"#
    )
}

/// A response as the client saw it, its chunked body decoded.
#[derive(Debug, Default)]
struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    trailers: Vec<(String, String)>,
}

fn find<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        find(&self.headers, name)
    }

    fn trailer(&self, name: &str) -> Option<&str> {
        find(&self.trailers, name)
    }

    fn text(&self) -> &str {
        std::str::from_utf8(&self.body).unwrap()
    }
}

/// Sends a request on a connection of its own.
fn send(
    addr: SocketAddr,
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> TcpStream {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut req = format!("{method} {target} HTTP/1.1\r\nHost: example.com\r\n");
    for (n, v) in headers {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    if !body.is_empty() {
        req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).unwrap();
    s.write_all(body).unwrap();
    s
}

fn line(r: &mut impl BufRead) -> Option<String> {
    let mut l = String::new();
    match r.read_line(&mut l) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(l.trim_end_matches(['\r', '\n']).to_owned()),
    }
}

fn fields(r: &mut impl BufRead) -> Option<Vec<(String, String)>> {
    let mut out = Vec::new();
    loop {
        let l = line(r)?;
        if l.is_empty() {
            return Some(out);
        }
        let (n, v) = l.split_once(':')?;
        out.push((n.to_owned(), v.trim().to_owned()));
    }
}

/// The response on `s`: `None` when the connection closed before a whole one came (a response
/// failed midway).
fn read(s: TcpStream, head_request: bool) -> Option<Resp> {
    read_from(BufReader::new(s), head_request)
}

fn read_from(mut r: impl BufRead, head_request: bool) -> Option<Resp> {
    let status_line = line(&mut r)?;
    let status = status_line.split(' ').nth(1)?.parse().ok()?;
    let headers = fields(&mut r)?;
    let mut resp = Resp {
        status,
        headers,
        ..Resp::default()
    };
    if head_request || status == 204 || status == 304 {
        return Some(resp);
    }
    if resp
        .header("transfer-encoding")
        .is_some_and(|v| v.eq_ignore_ascii_case("chunked"))
    {
        loop {
            let size = line(&mut r)?;
            let size = usize::from_str_radix(size.split(';').next()?.trim(), 16).ok()?;
            if size == 0 {
                resp.trailers = fields(&mut r)?;
                return Some(resp);
            }
            let mut chunk = vec![0; size + 2];
            r.read_exact(&mut chunk).ok()?;
            chunk.truncate(size);
            resp.body.extend_from_slice(&chunk);
        }
    }
    match resp.header("content-length") {
        Some(n) => {
            let mut body = vec![0; n.parse().ok()?];
            r.read_exact(&mut body).ok()?;
            resp.body = body;
        }
        None => {
            r.read_to_end(&mut resp.body).ok()?;
        }
    }
    Some(resp)
}

fn request(
    addr: SocketAddr,
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Option<Resp> {
    read(send(addr, method, target, headers, body), method == "HEAD")
}

fn get(addr: SocketAddr, target: &str) -> Resp {
    request(addr, "GET", target, &[], b"").unwrap_or_else(|| panic!("GET {target}: no response"))
}

/// Waits until a request to `/204` (whose response has no body for a plugin to fail) is let in,
/// answered other than 503 or cut by a plugin that fails it: with `max_streams: 1` and one
/// instance, that every response the plugin followed has had its context closed (fit's tests
/// count the config's holds for this).
fn settled(addr: SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match request(addr, "GET", "/204", &[], b"") {
            Some(r) if r.status == 503 => {
                assert!(Instant::now() < deadline, "contexts left open: {r:?}");
            }
            _ => return,
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// Taken by the tests that hold requests in [`app::HELD`], one at a time.
static HOLDING: Mutex<()> = Mutex::new(());

/// fit's `TestProxyWasmRouting`: the SDK's `http_routing` example reads its plugin
/// configuration and routes a request to the canary by changing its `:authority` when a die,
/// which the configuration's first byte overrides, comes up even.
#[test]
fn routing() {
    for (config, want) in [("2", "example.com-canary"), ("1", "example.com")] {
        let a = Akit::filter("http_routing", &format!(r#""config": "{config}","#));
        for _ in 0..3 {
            let r = get(a.addr, "/host");
            assert_eq!((r.status, r.text()), (200, want), "config {config}");
        }
    }
}

/// fit's `TestProxyWasmProperties`: the SDK's `properties` example answers 401 to a request
/// without the field a route's property names.
#[test]
fn properties() {
    let a = Akit::filter(
        "properties",
        r#""properties": [{"path": ["route_metadata", "filter_metadata", "envoy.filters.http.wasm", "auth"], "value": "x-auth"}],"#,
    );
    let r = get(a.addr, "/");
    assert_eq!(r.status, 401, "without the field: {r:?}");
    let r = request(a.addr, "GET", "/", &[("X-Auth", "yes")], b"").unwrap();
    assert_eq!(r.status, 200, "with it: {r:?}");
    // Without the property, every request goes on.
    let a = Akit::filter("properties", "");
    assert_eq!(get(a.addr, "/").status, 200);
}

/// fit's `TestProxyWasmResponseHeaders`: the SDK's `http_headers` example adds response fields
/// in `proxy_on_response_headers`, one of them from its configuration, and changes a request
/// field.
#[test]
fn response_headers() {
    let a = Akit::filter(
        "http_headers",
        r#""config": "{\"header\": \"x-config\", \"value\": \"from config\"}", "max_streams": 1,"#,
    );
    for _ in 0..3 {
        let r = get(a.addr, "/echo");
        assert_eq!((r.status, r.text()), (200, "[initial body]best"), "{r:?}");
        assert_eq!(
            r.header("X-Proxy-Wasm-Go-Sdk-Example"),
            Some("http_headers")
        );
        assert_eq!(r.header("X-Config"), Some("from config"));
    }
    settled(a.addr);
    // Without the response's callbacks the fields are not added.
    let a = Akit::filter(
        "http_headers",
        r#""config": "{\"header\": \"x-config\", \"value\": \"v\"}", "response": "none","#,
    );
    let r = get(a.addr, "/echo");
    assert_eq!(
        (r.text(), r.header("X-Config")),
        ("[initial body]best", None)
    );
}

/// fit's `TestProxyWasmResponseBody`: the SDK's `http_body` example holds the response's body
/// until its end, then appends, prepends or replaces.
#[test]
fn response_body() {
    let a = Akit::filter("http_body", r#""max_streams": 1,"#);
    for (op, want) in [
        ("append", "[initial body]best[this is appended body]"),
        ("prepend", "[this is prepended body][initial body]best"),
        ("replace", "[this is replaced body]"),
    ] {
        let r = request(
            a.addr,
            "PUT",
            "/echo",
            &[
                ("Test", "best"),
                ("Buffer-Operation", op),
                ("Buffer-Replace-At", "response"),
            ],
            b"x",
        )
        .unwrap();
        assert_eq!((r.status, r.text()), (200, want), "{op}: {r:?}");
        assert_eq!(r.header("Content-Length"), None, "{op}");
    }
    settled(a.addr);
    // With the header alone the body goes by untouched.
    let a = Akit::filter("http_body", r#""response": "headers","#);
    let r = request(
        a.addr,
        "PUT",
        "/echo",
        &[("Buffer-Replace-At", "response")],
        b"x",
    )
    .unwrap();
    assert_eq!(r.text(), "[initial body]");
}

/// fit's `TestProxyWasmResponseTrailers`: fit's own filter on a response written in parts with
/// a trailer: each part is upper-cased, the status is read as `:status`, and the trailer
/// changed.
#[test]
fn response_trailers() {
    let a = Akit::filter("response", r#""config": "upper", "max_streams": 1,"#);
    for _ in 0..3 {
        let r = get(a.addr, "/streamed");
        assert_eq!((r.status, r.text()), (201, "ONE TWO THREE"), "{r:?}");
        assert_eq!(r.header("X-Response-Code"), Some("201"));
        assert_eq!(r.trailer("X-Body-Bytes"), Some("13"), "{r:?}");
        assert_eq!(r.trailer("X-Upstream-Seen"), Some("up"), "{r:?}");
        assert_eq!(r.trailer("X-Upstream"), None, "{r:?}");
    }
    // A response with no trailer gets the one the plugin adds at its end.
    let r = get(a.addr, "/echo");
    assert_eq!(r.text(), "[INITIAL BODY]", "{r:?}");
    assert_eq!(r.trailer("X-Body-Bytes"), Some("14"), "{r:?}");
    // Nor is the body called for a response that has none.
    let r = get(a.addr, "/204");
    assert_eq!(r.status, 204);
    assert_eq!(r.header("X-Response-Code"), Some("204"));
    assert!(r.trailers.is_empty());
    // A response to HEAD neither.
    let r = request(a.addr, "HEAD", "/echo", &[], b"").unwrap();
    assert_eq!((r.status, r.header("X-Response-Code")), (200, Some("200")));
    settled(a.addr);
}

/// fit's `TestProxyWasmResponseFails`: a response whose callback cannot go on fails, or goes on
/// without the plugin with `on_error: skip`; one that runs past its timeout, and a body held
/// past `max_body_bytes`, each fail. A local response from `proxy_on_response_headers` goes in
/// the response's place (fit, on fib, fails the response: akio can replace a held head).
#[test]
fn response_fails() {
    for (name, filter, params, target, skipped) in [
        (
            "too slow",
            "response",
            r#""config": "slow:400", "timeout": "100ms","#,
            "/echo",
            "[initial body]",
        ),
        (
            "holds too much",
            "http_body",
            r#""max_body_bytes": 50,"#,
            "/long",
            &*"a".repeat(100),
        ),
    ] {
        for on_error in ["fail", "skip"] {
            let a = Akit::filter(
                filter,
                &format!(r#"{params} "on_error": "{on_error}", "max_streams": 1,"#),
            );
            // And again, on an instance made again.
            for _ in 0..2 {
                let r = request(
                    a.addr,
                    "PUT",
                    target,
                    &[("Buffer-Replace-At", "response")],
                    b"x",
                );
                match on_error {
                    "fail" => assert!(
                        r.as_ref()
                            .is_none_or(|r| r.status != 200 || r.text() != skipped),
                        "{name}, fail: answered {r:?}"
                    ),
                    _ => {
                        let r = r.unwrap_or_else(|| panic!("{name}, skip: cut"));
                        assert_eq!((r.status, r.text()), (200, skipped), "{name}, skip");
                    }
                }
            }
            settled(a.addr);
        }
    }
    for on_error in ["fail", "skip"] {
        let a = Akit::filter(
            "response",
            &format!(r#""config": "answer", "on_error": "{on_error}", "max_streams": 1,"#),
        );
        for _ in 0..2 {
            let r = get(a.addr, "/echo");
            assert_eq!((r.status, r.text()), (418, "too late"), "{on_error}: {r:?}");
        }
        settled(a.addr);
    }
}

/// fit's `TestProxyWasmMaxStreams`: an instance follows no more responses than `max_streams` at
/// once: past it a request is answered 503, and once one ends there is room again.
#[test]
fn max_streams() {
    let _one = HOLDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let a = Akit::filter(
        "response",
        r#""config": "upper", "instances": 1, "max_streams": 1,"#,
    );
    let first = send(a.addr, "GET", "/held", &[], b"");
    // The first is under way once the second finds no room.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let r = get(a.addr, "/");
        if r.status == 503 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no 503 while one is followed: {r:?}"
        );
        thread::sleep(Duration::from_millis(1));
    }
    let held = std::mem::take(&mut *app::HELD.lock().unwrap());
    assert_eq!(held.len(), 1);
    for d in held {
        d.respond(akio_http::Response::with_body(
            200,
            "text/plain",
            b"held".to_vec(),
        ));
    }
    let r = read(first, false).unwrap();
    assert_eq!(r.text(), "HELD");
    settled(a.addr);
    assert_eq!(get(a.addr, "/").text(), "OK");
}

/// fit's `TestProxyWasmResponseClientGone`: a request whose client goes while its response is
/// held has its context closed, and holds nothing.
#[test]
fn response_client_gone() {
    let _one = HOLDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let a = Akit::filter(
        "response",
        r#""config": "upper", "instances": 1, "max_streams": 1,"#,
    );
    let first = send(a.addr, "GET", "/held", &[], b"");
    let deadline = Instant::now() + Duration::from_secs(10);
    while app::HELD.lock().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "not held");
        thread::sleep(Duration::from_millis(1));
    }
    drop(first);
    // The client is gone; the late answer goes nowhere.
    thread::sleep(Duration::from_millis(50));
    for d in std::mem::take(&mut *app::HELD.lock().unwrap()) {
        d.respond(akio_http::Response::with_body(
            200,
            "text/plain",
            b"late".to_vec(),
        ));
    }
    settled(a.addr);
}

/// Where the encoded filters' strings are, in their memory.
const DATA: [(u32, &str); 7] = [
    (100, ":path"),
    (110, "/target?x=1"),
    (130, ":method"),
    (140, "PATCH"),
    (150, "x-from"),
    (160, "plugin"),
    (300, "denied"),
];
/// Where the encoded filters' local response's fields are.
const PAIRS_AT: u32 = 200;

fn local_pairs() -> Vec<u8> {
    // One pair, x-l: 1, as Proxy-Wasm encodes them.
    let mut b = Vec::new();
    for n in [1u32, 3, 1] {
        b.extend(n.to_le_bytes());
    }
    b.extend(b"x-l\x001\x00");
    b
}

/// A body that answers 418 `denied` with `x-l: 1` and gRPC status 7, then returns 0.
fn answers() -> Vec<u8> {
    let pairs = local_pairs().len() as i32;
    encode::call_with(
        0,
        &[418, 0, 0, 300, 6, PAIRS_AT as i32, pairs, 7],
        1,
        Some(0),
    )
}

/// A body that sets `:path`, `:method` and `x-from` of the request, then returns 0.
fn rewrites() -> Vec<u8> {
    let mut b = encode::call_with(1, &[0, 100, 5, 110, 11], 1, None);
    b.extend(encode::call_with(1, &[0, 130, 7, 140, 5], 1, None));
    b.extend(encode::call_with(1, &[0, 150, 6, 160, 6], 1, Some(0)));
    b
}

/// A Proxy-Wasm filter of the tests' own (fit's `respModule`): its callbacks go on, but those
/// `bodies` names, whose bodies they have instead (`None`: not exported).
fn pw_module(bodies: &[(&str, Option<Vec<u8>>)]) -> String {
    let zero = || encode::i32_const(0);
    let mut funcs = Vec::new();
    for (name, params, results, body) in [
        ("proxy_abi_version_0_2_1", 0, 0, Vec::new()),
        ("proxy_on_context_create", 2, 0, Vec::new()),
        ("proxy_on_request_headers", 3, 1, zero()),
        ("proxy_on_response_headers", 3, 1, zero()),
        ("proxy_on_response_body", 3, 1, zero()),
        ("proxy_on_response_trailers", 2, 1, zero()),
        ("proxy_on_done", 1, 1, encode::i32_const(1)),
        ("proxy_on_log", 1, 0, Vec::new()),
        ("proxy_on_delete", 1, 0, Vec::new()),
        ("proxy_on_memory_allocate", 1, 1, encode::i32_const(4096)),
        ("proxy_on_vm_start", 2, 1, encode::i32_const(1)),
        ("proxy_on_configure", 2, 1, encode::i32_const(1)),
    ] {
        let body = match bodies.iter().find(|(n, _)| *n == name) {
            Some((_, None)) => continue,
            Some((_, Some(b))) => b.clone(),
            None => body,
        };
        funcs.push(Func::export(name, params, results, body));
    }
    let mut data: Vec<(u32, Vec<u8>)> = DATA
        .iter()
        .map(|(at, s)| (*at, s.as_bytes().to_vec()))
        .collect();
    data.push((PAIRS_AT, local_pairs()));
    let code = encode::Module {
        imports: vec![
            encode::Import::new("env", "proxy_send_local_response", 8, 1),
            encode::Import::new("env", "proxy_replace_header_map_value", 5, 1),
        ],
        funcs,
        pages: 1,
        data,
    }
    .encode();
    temp_file("pw.wasm", &code)
}

fn trap() -> Option<Vec<u8>> {
    Some(vec![encode::OP_UNREACHABLE])
}

/// One instance, following one response at a time, so that `settled` finds a context left open.
const ONE: &str = r#""instances": 1, "max_streams": 1,"#;

/// The request's callbacks (fit's `TestHostProxyWasm` through a request): the plugin's changes to
/// `:path`, `:method` and a field reach the handler; its local response is sent with its fields
/// and gRPC status; a pause without an answer, and a trap, fail the request, or let it go on
/// without the plugin with `on_error: skip`.
#[test]
fn request_phase() {
    let a = Akit::plugin(
        &pw_module(&[("proxy_on_request_headers", Some(rewrites()))]),
        ONE,
    );
    for _ in 0..2 {
        let r = get(a.addr, "/somewhere");
        assert_eq!(r.text(), "PATCH /target?x=1 plugin", "{r:?}");
    }
    settled(a.addr);
    let a = Akit::plugin(
        &pw_module(&[("proxy_on_request_headers", Some(answers()))]),
        ONE,
    );
    let r = get(a.addr, "/");
    assert_eq!((r.status, r.text()), (418, "denied"), "{r:?}");
    assert_eq!(r.header("X-L"), Some("1"));
    assert_eq!(r.header("Grpc-Status"), Some("7"));
    assert_eq!(r.header("Content-Type"), Some("text/plain; charset=utf-8"));
    settled(a.addr);
    for (what, body) in [("pauses", Some(encode::i32_const(1))), ("traps", trap())] {
        for (on_error, status, text) in [
            ("fail", 500, "500 Internal Server Error\n"),
            ("skip", 200, "ok"),
        ] {
            let a = Akit::plugin(
                &pw_module(&[("proxy_on_request_headers", body.clone())]),
                &format!(r#"{ONE} "on_error": "{on_error}","#),
            );
            // And again, on an instance made again.
            for _ in 0..2 {
                let r = get(a.addr, "/");
                assert_eq!((r.status, r.text()), (status, text), "{what}, {on_error}");
            }
        }
    }
}

/// fit's `TestResponseCallbacksFail`: the head's, the body's and the trailer's callbacks that
/// trap, or answer once the response has begun (the body's and the trailer's): the response
/// fails, or goes on without the plugin, as `on_error` says; the context closes either way.
#[test]
fn response_callbacks_fail() {
    for (what, bodies, target, skipped, trailer) in [
        (
            "the head's callback traps",
            vec![("proxy_on_response_headers", trap())],
            "/echo",
            "[initial body]",
            None,
        ),
        (
            "the body's callback traps",
            vec![("proxy_on_response_body", trap())],
            "/echo",
            "[initial body]",
            None,
        ),
        (
            "the body's callback answers",
            vec![("proxy_on_response_body", Some(answers()))],
            "/echo",
            "[initial body]",
            None,
        ),
        (
            "the trailer's callback traps",
            vec![("proxy_on_response_trailers", trap())],
            "/streamed",
            "one two three",
            Some("up"),
        ),
        (
            "the trailer's callback answers",
            vec![
                ("proxy_on_response_body", None),
                ("proxy_on_response_trailers", Some(answers())),
            ],
            "/streamed",
            "one two three",
            Some("up"),
        ),
    ] {
        for on_error in ["fail", "skip"] {
            let a = Akit::plugin(
                &pw_module(&bodies),
                &format!(r#"{ONE} "on_error": "{on_error}","#),
            );
            for _ in 0..2 {
                let r = request(a.addr, "GET", target, &[], b"");
                if on_error == "fail" {
                    assert!(
                        r.as_ref().is_none_or(|r| r.text() != skipped),
                        "{what}, fail: answered {r:?}"
                    );
                } else {
                    let r = r.unwrap_or_else(|| panic!("{what}, skip: cut"));
                    assert_eq!(r.text(), skipped, "{what}, skip: {r:?}");
                    assert_eq!(r.trailer("X-Upstream"), trailer, "{what}, skip: {r:?}");
                }
            }
            settled(a.addr);
        }
    }
}

/// fit's `TestResponseBodyPaused`: a plugin that pauses the body holds it, and sees it with more
/// the next time; at the end a pause goes on, the plugin having all of it.
#[test]
fn response_body_paused() {
    let a = Akit::plugin(
        &pw_module(&[("proxy_on_response_body", Some(encode::i32_const(1)))]),
        ONE,
    );
    let r = get(a.addr, "/streamed");
    assert_eq!((r.status, r.text()), (201, "one two three"), "{r:?}");
    assert_eq!(r.trailer("X-Upstream"), Some("up"));
    settled(a.addr);
}

/// fit's `TestCloseContextTraps`: a context whose `proxy_on_done` traps is not logged or
/// deleted, and still closes; the instance made again serves the next.
#[test]
fn close_context_traps() {
    let a = Akit::plugin(
        &pw_module(&[("proxy_on_done", trap()), ("proxy_on_log", trap())]),
        ONE,
    );
    for _ in 0..3 {
        assert_eq!(get(a.addr, "/echo").text(), "[initial body]");
        settled(a.addr);
    }
}

/// A plugin that refuses its configuration, or lacks what Proxy-Wasm calls, or has it of another
/// type, fails its load, as fit's.
#[test]
fn load_errors() {
    for (bodies, want) in [
        (
            vec![("proxy_on_vm_start", Some(encode::i32_const(0)))],
            "the plugin refused its vm_config",
        ),
        (
            vec![("proxy_on_configure", Some(encode::i32_const(0)))],
            "the plugin refused its config",
        ),
        (
            vec![("proxy_on_memory_allocate", None)],
            "the Proxy-Wasm plugin does not export proxy_on_memory_allocate",
        ),
        (vec![("proxy_on_context_create", trap())], "unreachable"),
    ] {
        let json = config(&pw_module(&bodies), "");
        let config = akit_config::from_json(&json, None).unwrap();
        let err = compile(&config, &registry(), &Env::new(EngineSlot::new()))
            .err()
            .unwrap_or_else(|| panic!("{want}: compiled"))
            .to_string();
        assert!(err.contains(want), "{err}, want {want:?}");
    }
    // An export of another type than Proxy-Wasm's.
    let code = encode::Module {
        funcs: [
            ("proxy_abi_version_0_2_1", 0, 0),
            ("proxy_on_context_create", 2, 0),
            ("proxy_on_request_headers", 2, 1),
            ("proxy_on_done", 1, 1),
            ("proxy_on_log", 1, 0),
            ("proxy_on_delete", 1, 0),
            ("proxy_on_memory_allocate", 1, 1),
        ]
        .into_iter()
        .map(|(n, p, r)| {
            Func::export(
                n,
                p,
                r,
                if r == 1 {
                    encode::i32_const(0)
                } else {
                    Vec::new()
                },
            )
        })
        .collect(),
        pages: 1,
        ..encode::Module::default()
    }
    .encode();
    let json = config(&temp_file("typed.wasm", &code), "");
    let config = akit_config::from_json(&json, None).unwrap();
    let err = compile(&config, &registry(), &Env::new(EngineSlot::new()))
        .err()
        .unwrap()
        .to_string();
    assert!(
        err.contains("proxy_on_request_headers is not of the type"),
        "{err}"
    );
}

/// A large body, written by a handler paced as a proxy is, through a plugin that changes each
/// part, to a client that reads it slowly: it comes whole and changed, the writer paced by the
/// client through the plugin's calls (akio holds at most 4 MiB, and the plugin never more than
/// `max_body_bytes` at once).
#[test]
fn large_body_slow_client() {
    // Upper-casing a part of up to a few hundred KiB in a Go guest takes the interpreter more
    // than the default 100ms in a debug build.
    let a = Akit::filter(
        "response",
        &format!(r#""config": "upper", "timeout": "20s", {ONE}"#),
    );
    let mut s = send(a.addr, "GET", "/big", &[], b"");
    // Slow: the first bytes come, then the client waits before it reads on.
    let mut first = [0; 1024];
    if let Err(e) = s.read_exact(&mut first) {
        let mut raw = Vec::new();
        let _ = s.read_to_end(&mut raw);
        panic!("{e}: {:?}", String::from_utf8_lossy(&first));
    }
    thread::sleep(Duration::from_millis(300));
    let r = read_rest(s, &first);
    assert_eq!(r.status, 200);
    assert_eq!(r.body.len(), app::BIG);
    assert!(r.body.iter().all(|&b| b == b'A'), "not upper-cased");
    assert_eq!(r.trailer("X-Body-Bytes"), Some(&*app::BIG.to_string()));
    settled(a.addr);
}

/// The response read from `s`, of which `first` was read already.
fn read_rest(s: TcpStream, first: &[u8]) -> Resp {
    let rest = BufReader::new(first.chain(s));
    read_from(rest, false).unwrap()
}

/// `max_body_bytes` under, at and over the body the plugin holds (the SDK's `http_body` holds a
/// response's whole body): the body passes at and under it, and past it the response fails.
#[test]
fn max_body_bytes() {
    for (max, ok) in [(101, true), (100, true), (99, false)] {
        let a = Akit::filter("http_body", &format!(r#""max_body_bytes": {max}, {ONE}"#));
        let r = request(
            a.addr,
            "PUT",
            "/long",
            &[
                ("Buffer-Replace-At", "response"),
                ("Buffer-Operation", "replace"),
            ],
            b"x",
        );
        if ok {
            let r = r.unwrap();
            assert_eq!(r.text(), "[this is replaced body]", "{max}");
        } else {
            assert!(
                r.is_none_or(|r| r.status != 200 || r.body.len() != 100),
                "{max}"
            );
        }
        settled(a.addr);
    }
}

/// An engine for the tests' HTTP/2 clients.
fn clients() -> Arc<Engine> {
    static E: std::sync::OnceLock<Arc<Engine>> = std::sync::OnceLock::new();
    E.get_or_init(|| {
        let e = Engine::builder().client_threads(2).build().unwrap();
        e.start().unwrap();
        Arc::new(e)
    })
    .clone()
}

/// `GET path` over h2c to `addr`, with `fields`: the response, or `None` when its stream was
/// reset.
fn h2_get(addr: SocketAddr, path: &str, fields: &[(&str, &str)]) -> Option<Resp> {
    use akio_http::client::ClientRequest;
    use akio_http::h2client::{H2Client, H2Event, H2Options};
    let c = H2Client::dial_blocking(&clients(), addr, None, H2Options::default()).unwrap();
    let mut req = ClientRequest::new(akio_http::Method::Get, &format!("http://example.com{path}"));
    for (n, v) in fields {
        req.headers.append(n, v);
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let mut got = Resp::default();
    let pairs = |h: &akio_http::Headers| -> Vec<(String, String)> {
        h.iter()
            .map(|(n, v)| (n.to_owned(), v.to_owned()))
            .collect()
    };
    let up = c.open(req, move |e| match e {
        H2Event::Head { status, headers } => {
            got.status = status;
            got.headers = pairs(headers);
        }
        H2Event::Data(d) => got.body.extend_from_slice(d),
        H2Event::Trailers(t) => got.trailers = pairs(t),
        H2Event::End => {
            let _ = tx.send(Some(std::mem::take(&mut got)));
        }
        H2Event::Error(_) => {
            let _ = tx.send(None);
        }
    });
    up.finish();
    let got = rx.recv_timeout(Duration::from_secs(30)).unwrap();
    c.close();
    got
}

/// The filters over HTTP/2: `:authority` rewritten, a request field changed and response fields
/// added in the held head, a streamed body upper-cased with its trailer changed, and a local response in the held head's
/// place; a response that fails resets its stream.
#[test]
fn h2() {
    let a = Akit::filter("http_routing", r#""config": "2","#);
    let r = h2_get(a.addr, "/host", &[]).unwrap();
    assert_eq!((r.status, r.text()), (200, "example.com-canary"));
    let a = Akit::filter(
        "http_headers",
        &format!(r#""config": "{{\"header\": \"x-config\", \"value\": \"v\"}}", {ONE}"#),
    );
    let r = h2_get(a.addr, "/echo", &[]).unwrap();
    assert_eq!((r.status, r.text()), (200, "[initial body]best"));
    assert_eq!(r.header("x-config"), Some("v"));
    settled(a.addr);
    let a = Akit::filter("response", &format!(r#""config": "upper", {ONE}"#));
    let r = h2_get(a.addr, "/streamed", &[]).unwrap();
    assert_eq!((r.status, r.text()), (201, "ONE TWO THREE"), "{r:?}");
    assert_eq!(r.trailer("x-body-bytes"), Some("13"), "{r:?}");
    assert_eq!(r.trailer("x-upstream-seen"), Some("up"), "{r:?}");
    settled(a.addr);
    let a = Akit::filter("response", &format!(r#""config": "answer", {ONE}"#));
    let r = h2_get(a.addr, "/echo", &[]).unwrap();
    assert_eq!((r.status, r.text()), (418, "too late"));
    settled(a.addr);
    let a = Akit::plugin(&pw_module(&[("proxy_on_response_body", trap())]), ONE);
    assert!(
        h2_get(a.addr, "/echo", &[]).is_none_or(|r| r.text() != "[initial body]"),
        "a failed response goes"
    );
    settled(a.addr);
}
