//! The `wasm` middleware through akit's listener on akio, on the backend `AKIO_TEST_BACKEND`
//! names (epoll by default, `uring`), with fit's tests (`plugin/wasm/wasm_test.go`,
//! `plugin_test.go`) ported.
//!
//! The plugin is fit's own test plugin, `testdata/headers`, built with Go for wasip1 with the ABI's
//! names renamed as akit names them (decision 037; `tests/guests/headers-akit.wasm.gz`, made by
//! `tools/plugin-guest.sh`; see `tests/guests/README.md`): it edits the request's and the response's fields, answers `/deny` itself, loops on
//! `/loop`, panics on `/panic` and grows its memory by 64 MiB on `/grow`. Plugins that misbehave
//! in ways a compiler would not let them are encoded by the tests (`akit_plugin::encode`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, UringConfig};
use akit_plugin::encode::{self, Func};
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
    o.shutdown_timeout = Duration::from_secs(5);
    o
}

/// The test handler: fit's `echo`, which answers with the request's fields the tests look at.
mod echo {
    use std::sync::Arc;

    use akio_http::{Context, Response};
    use akit_runtime::{Handler, HandlerParams, Info, Provision, ProvisionError, Provisioned};

    struct Echo;

    impl akio_http::Service for Echo {
        fn call(&self, ctx: &mut Context) {
            let h = &ctx.req().headers;
            let get = |n: &str| h.get(n).unwrap_or("").to_owned();
            let body = format!(
                "plugin={} secret={} before={} body={}",
                get("X-Plugin"),
                get("X-Secret"),
                get("X-Before"),
                ctx.req().body.len()
            );
            ctx.respond(Response::with_body(200, "text/plain", body.into_bytes()));
        }
    }

    impl Handler for Echo {}

    fn new(_: &Provision<'_, HandlerParams>) -> Result<Provisioned<dyn Handler>, ProvisionError> {
        Ok(Provisioned::new(Arc::new(Echo) as Arc<dyn Handler>))
    }

    pub(crate) static MODULES: &[Info<HandlerParams>] =
        &[Info::handler("http.handlers.respond", "Echoes.", new)];
}

fn registry() -> Registry<HandlerParams> {
    Registry::new(&[
        echo::MODULES,
        akit_plugin::MODULES,
        akit_middleware::MODULES,
    ])
    .unwrap()
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

/// A committed guest of `tests/guests`, inflated.
fn guest(name: &str) -> Vec<u8> {
    let gz = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/guests")
            .join(name),
    )
    .unwrap();
    akio_deflate::decompress(akio_deflate::Format::Gzip, &gz, 64 << 20).unwrap()
}

/// fit's test plugin, built for akit's ABI, inflated once.
fn headers_plugin() -> &'static str {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| temp_file("headers.wasm", &guest("headers-akit.wasm.gz")))
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

    /// A route to the echo behind `middleware`, a JSON array.
    fn with(middleware: &str) -> Akit {
        Akit::start(&config(middleware))
    }

    /// A route to the echo behind the `wasm` middleware running `path` with `params` besides
    /// (JSON members, each followed by a comma, or nothing).
    fn plugin(path: &str, params: &str) -> Akit {
        Akit::with(&wasm(path, params))
    }
}

impl Drop for Akit {
    fn drop(&mut self) {
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
    }
}

fn config(middleware: &str) -> String {
    format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
            "routes": [{{"middleware": {middleware},
                         "handler": {{"module": "respond"}}}}]}}"#
    )
}

fn wasm(path: &str, params: &str) -> String {
    format!(r#"[{{"module": "wasm", "params": {{{params} "path": "{path}"}}}}]"#)
}

/// A response as the client saw it.
#[derive(Debug)]
struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.all(name).into_iter().next()
    }

    fn all(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .collect()
    }

    fn text(&self) -> &str {
        std::str::from_utf8(&self.body).unwrap()
    }
}

/// The request on a connection of its own, written whole.
fn send(addr: SocketAddr, target: &str, headers: &[(&str, &str)], body: &[u8]) -> TcpStream {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut req = format!("GET {target} HTTP/1.1\r\nConnection: close\r\nHost: example.com\r\n");
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

/// The response on `s`, read to the connection's close.
fn read(mut s: TcpStream) -> Resp {
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).unwrap();
    let end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("no response head in {:?}", String::from_utf8_lossy(&raw)));
    let head = std::str::from_utf8(&raw[..end]).unwrap();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .map(|l| {
            let (n, v) = l.split_once(':').unwrap();
            (n.to_owned(), v.trim().to_owned())
        })
        .collect();
    Resp {
        status,
        headers,
        body: raw[end + 4..].to_vec(),
    }
}

fn get(addr: SocketAddr, target: &str, headers: &[(&str, &str)]) -> Resp {
    read(send(addr, target, headers, b""))
}

/// fit's `TestHeaders`: the plugin sets a request field from its config, deletes another, and
/// sets and adds response fields, on each instance and again.
#[test]
fn headers() {
    let a = Akit::plugin(headers_plugin(), r#""config": "from the config","#);
    for _ in 0..3 {
        let r = get(a.addr, "/a", &[("X-Secret", "s")]);
        assert_eq!(
            (r.status, r.text()),
            (200, "plugin=from the config secret= before= body=0")
        );
        assert_eq!(r.header("X-Served-By"), Some("wasm GET /a"));
        assert_eq!(r.all("X-Count"), ["one", "two"]);
    }
}

/// The plugin's changes to the request's fields come after an earlier middleware's, which it
/// sees, and both reach the handler.
#[test]
fn after_the_headers_middleware() {
    let a = Akit::with(&format!(
        r#"[{{"module": "headers", "params": {{"request": {{"set": {{"X-Before": "b", "X-Secret": "t"}}}}}}}},
            {}]"#,
        wasm(headers_plugin(), r#""config": "c","#)
            .trim_start_matches('[')
            .trim_end_matches(']')
    ));
    let r = get(a.addr, "/a", &[]);
    assert_eq!(
        (r.status, r.text()),
        (200, "plugin=c secret= before=b body=0")
    );
}

/// fit's `TestRespond`: the plugin answers the request itself, with the response fields it set.
#[test]
fn respond() {
    let a = Akit::plugin(headers_plugin(), "");
    let r = get(a.addr, "/deny", &[("X-Deny", "you")]);
    assert_eq!((r.status, r.text()), (403, "denied: you"));
    assert_eq!(r.header("X-Denied-By"), Some("plugin"));
    assert_eq!(r.header("Content-Type"), Some("text/plain; charset=utf-8"));
}

/// fit's `TestFailures`: a call that runs past its timeout, one that traps and one that asks for
/// more memory than it may have each fail the request, or let it go on with `on_error: skip`;
/// the instance it stopped is made again for the next request.
#[test]
fn failures() {
    for on_error in ["fail", "skip"] {
        let a = Akit::plugin(
            headers_plugin(),
            &format!(
                r#""instances": 1, "timeout": "50ms", "max_memory_bytes": 67108864, "on_error": "{on_error}","#
            ),
        );
        for path in ["/loop", "/panic", "/grow"] {
            let started = Instant::now();
            let r = get(a.addr, path, &[]);
            let want = if on_error == "skip" { 200 } else { 500 };
            assert_eq!(r.status, want, "{on_error} {path}: {}", r.text());
            if on_error == "fail" {
                assert_eq!(r.text(), "500 Internal Server Error\n");
            } else {
                // Without the plugin's changes.
                assert_eq!(r.header("X-Served-By"), None);
            }
            if path == "/loop" {
                assert!(
                    started.elapsed() < Duration::from_secs(5),
                    "stopped after {:?}",
                    started.elapsed()
                );
            }
            let r = get(a.addr, "/a", &[]);
            assert_eq!(r.status, 200, "{on_error}: after {path}");
            assert!(
                r.header("X-Served-By").is_some(),
                "{on_error}: after {path}"
            );
        }
    }
}

/// fit's `TestNeverBlocks`: the call runs on the pool, and the queue is bounded: past it a
/// request is answered 503 at once.
#[test]
fn never_blocks() {
    let a = Akit::plugin(
        headers_plugin(),
        r#""instances": 1, "queue": 1, "timeout": "500ms", "wait_timeout": "5s","#,
    );
    let first = send(a.addr, "/loop", &[], b"");
    // The first is taken by the instance before the second comes.
    thread::sleep(Duration::from_millis(200));
    let second = send(a.addr, "/loop", &[], b"");
    thread::sleep(Duration::from_millis(50));
    let started = Instant::now();
    let r = get(a.addr, "/a", &[]);
    assert_eq!(
        (r.status, r.text()),
        (503, "503 Service Unavailable\n"),
        "past the queue"
    );
    assert!(started.elapsed() < Duration::from_millis(400), "it waited");
    for (i, s) in [first, second].into_iter().enumerate() {
        assert_eq!(read(s).status, 500, "{i}");
    }
    // The instance serves again.
    assert_eq!(get(a.addr, "/a", &[]).status, 200);
}

/// A request that waits for an instance longer than `wait_timeout` is answered 503.
#[test]
fn wait_timeout() {
    let a = Akit::plugin(
        headers_plugin(),
        r#""instances": 1, "timeout": "400ms", "wait_timeout": "50ms","#,
    );
    let busy = send(a.addr, "/loop", &[], b"");
    thread::sleep(Duration::from_millis(100));
    assert_eq!(get(a.addr, "/a", &[]).status, 503);
    assert_eq!(read(busy).status, 500);
}

/// Clients that go while their requests wait (connections cut midway), and a client that sends
/// its request slowly, leave the plugin serving.
#[test]
fn clients_cut_and_slow() {
    let a = Akit::plugin(headers_plugin(), r#""instances": 1, "timeout": "300ms","#);
    let busy = send(a.addr, "/loop", &[], b"");
    thread::sleep(Duration::from_millis(50));
    for _ in 0..4 {
        drop(send(a.addr, "/a", &[], b""));
    }
    // A head cut in its middle.
    let mut s = TcpStream::connect(a.addr).unwrap();
    s.write_all(b"GET /a HTTP/1.1\r\nHost: exa").unwrap();
    drop(s);
    assert_eq!(read(busy).status, 500);
    // A head written a few bytes at a time.
    let mut s = TcpStream::connect(a.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    for chunk in b"GET /a HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n".chunks(7) {
        s.write_all(chunk).unwrap();
        thread::sleep(Duration::from_millis(5));
    }
    let r = read(s);
    assert_eq!(r.status, 200);
    assert_eq!(r.header("X-Served-By"), Some("wasm GET /a"));
}

/// A request's body comes whole to the handler behind the plugin, within the listener's limit.
#[test]
fn bodies() {
    let a = Akit::start(
        &config(&wasm(headers_plugin(), r#""config": "c","#)).replacen(
            '{',
            r#"{"limits": {"max_body_bytes": 1048576}, "#,
            1,
        ),
    );
    let body = vec![b'x'; 512 << 10];
    let r = read(send(a.addr, "/a", &[], &body));
    assert_eq!(
        (r.status, r.text()),
        (200, "plugin=c secret= before= body=524288")
    );
    // Past the limit, refused on its declared length, before the plugin.
    let over = ((1 << 20) + 1).to_string();
    let r = get(a.addr, "/a", &[("Content-Length", &over)]);
    assert_eq!(r.status, 413);
}

/// fit's `TestCalls` (akit's ABI): `akit_on_request` that stops without an answer is answered 200
/// with nothing; one that goes on reaches the handler; a request that waited past a
/// `wait_timeout` of 1ns is answered 503.
#[test]
fn calls() {
    let module = |action: i32| {
        encode::Module {
            funcs: vec![Func::export(
                "akit_on_request",
                0,
                1,
                encode::i32_const(action),
            )],
            ..encode::Module::default()
        }
        .encode()
    };
    for (name, code, params, status, body) in [
        ("stops without an answer", module(1), "", 200, ""),
        (
            "goes on",
            module(0),
            "",
            200,
            "plugin= secret= before= body=0",
        ),
        (
            "waited too long",
            module(0),
            r#""wait_timeout": "1ns","#,
            503,
            "503 Service Unavailable\n",
        ),
    ] {
        let a = Akit::plugin(
            &temp_file("call.wasm", &code),
            &format!(r#""instances": 1, {params}"#),
        );
        for _ in 0..2 {
            let r = get(a.addr, "/", &[]);
            assert_eq!((r.status, r.text()), (status, body), "{name}");
        }
    }
}

/// The config's errors, as fit's `TestParams` and `TestLoadErrors` (akit's ABI) check them.
#[test]
fn errors() {
    let trap = vec![encode::OP_UNREACHABLE];
    let init_traps = encode::Module {
        funcs: vec![
            Func::export("akit_on_request", 0, 1, encode::i32_const(0)),
            Func::export("_initialize", 0, 0, trap),
        ],
        ..encode::Module::default()
    }
    .encode();
    let no_exports = encode::Module::default().encode();
    let proxy_wasm = encode::Module {
        funcs: vec![Func::export("proxy_abi_version_0_2_1", 0, 0, Vec::new())],
        ..encode::Module::default()
    }
    .encode();
    let not_wasm = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    for (path, params, want) in [
        ("/nonexistent.wasm".to_owned(), "", "open /nonexistent.wasm"),
        (
            not_wasm.to_str().unwrap().to_owned(),
            "",
            "compiling the plugin",
        ),
        (
            temp_file("init.wasm", &init_traps),
            r#""instances": 2,"#,
            "unreachable",
        ),
        (
            temp_file("none.wasm", &no_exports),
            "",
            "exports neither akit_on_request",
        ),
        // fit's own build of the same plugin: fit's names are not akit's ABI (decision 037).
        (
            temp_file("headers-fit.wasm", &guest("headers-fit.wasm.gz")),
            "",
            "exports neither akit_on_request",
        ),
        (
            temp_file("pw.wasm", &proxy_wasm),
            "",
            "the Proxy-Wasm plugin does not export proxy_on_context_create",
        ),
    ] {
        let json = config(&wasm(&path, params));
        let config = akit_config::from_json(&json, None).unwrap();
        let err = compile(&config, &registry(), &Env::new(EngineSlot::new()))
            .err()
            .unwrap_or_else(|| panic!("{path}: compiled"))
            .to_string();
        assert!(err.contains(want), "{path}: {err}, want {want:?}");
    }
    for (params, want) in [
        (r#""on_error": "ignore","#, r#""ignore" is not one of"#),
        (r#""response": "body","#, r#""body" is not one of"#),
        (r#""instances": -1,"#, "instances"),
    ] {
        let err = akit_config::from_json(&config(&wasm("x.wasm", params)), None)
            .unwrap_err()
            .to_string();
        assert!(err.contains(want), "{params}: {err}, want {want:?}");
    }
}
