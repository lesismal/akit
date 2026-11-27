//! M3's first exit criterion (I56): no middleware, nor handler, blocks an akio I/O thread. fit's
//! `test/workers` ported onto akit's listener, on the backend `AKIO_TEST_BACKEND` names (epoll by
//! default, `uring`).
//!
//! Each module akit registers serves a request whose answer waits on something slow: the
//! upstream its route proxies to (every middleware wraps a `proxy` to an upstream that answers
//! [`SLOW`] later), the auth service `forward_auth` asks, the FastCGI application, the tunnel's
//! far end, the bcrypt check of `basic_auth` on akio's pool. Where fit times `ServeHTTP`, akit
//! has akio time every callback on its I/O threads: the engine's slow-callback detector, set to
//! [`QUICK`], records any that runs longer, and a module that waited on the slow part would
//! hold its callback, and its thread, for as long. The answer still comes only once the slow part
//! gave it (but for the modules that answer themselves: `redirect`, `respond`, `static`).
//! [`every_module_covered`] fails for a module registered without a case, so a new one cannot
//! skip the check.
//!
//! Each module's benchmark, with nothing slow behind it, is `benches/modules.rs`.
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
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use akio::{Backend, Engine, UringConfig};
use akio_crypto::base64::{STANDARD as B64, URL_SAFE_NO_PAD as B64URL};
use akio_crypto::hash::Sha256;
use akio_crypto::mac::Hmac;
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};

/// How long the slow part of each case takes to answer, and the longest an I/O thread's
/// callback may run: far apart, so that a loaded machine does not blur them (fit's).
const SLOW: Duration = Duration::from_millis(600);
const QUICK: Duration = Duration::from_millis(200);
const WAIT: Duration = Duration::from_secs(20);

/// The JWT secret, 32 bytes or more.
const SECRET: &str = "a secret of the test's, 32 bytes or more";
/// `pw`, bcrypt at cost 4 (htpasswd -nbB -C 4).
const HASH: &str = "$2y$04$n.ddWQ.hfkNRsx7o3Y1w3eongqIP2oVF8Ka9OEQQwsoVnWY6O.jlW";

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

// ---- What the cases wait on ---------------------------------------------------------------------

/// Serves each connection `serve`d on a thread of its own.
fn server(serve: impl Fn(TcpStream) + Send + Sync + 'static) -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let serve = Arc::new(serve);
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            let serve = serve.clone();
            thread::spawn(move || serve(s));
        }
    });
    addr
}

/// An HTTP/1.1 upstream that answers each request `delay` after it came, with 2000 bytes not
/// to be kept by a cache.
fn http_upstream(delay: Duration) -> SocketAddr {
    server(move |s| {
        let mut w = s.try_clone().unwrap();
        let mut r = BufReader::new(s);
        loop {
            let mut len = 0usize;
            let mut line = String::new();
            loop {
                line.clear();
                if r.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                if line == "\r\n" {
                    break;
                }
                if let Some((n, v)) = line.split_once(':') {
                    if n.eq_ignore_ascii_case("content-length") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut body = vec![0; len];
            if r.read_exact(&mut body).is_err() {
                return;
            }
            thread::sleep(delay);
            let body = "late ".repeat(400);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nCache-Control: no-store\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            if w.write_all(resp.as_bytes()).is_err() {
                return;
            }
        }
    })
}

/// A FastCGI application that answers each request `delay` after its body ended.
fn fastcgi_app(delay: Duration) -> SocketAddr {
    fn record(t: u8, id: u16, content: &[u8]) -> Vec<u8> {
        let mut r = vec![1, t];
        r.extend_from_slice(&id.to_be_bytes());
        r.extend_from_slice(&(content.len() as u16).to_be_bytes());
        r.extend_from_slice(&[0, 0]);
        r.extend_from_slice(content);
        r
    }
    server(move |s| {
        let mut w = s.try_clone().unwrap();
        let mut r = BufReader::new(s);
        loop {
            let mut head = [0u8; 8];
            if r.read_exact(&mut head).is_err() {
                return;
            }
            let (t, id) = (head[1], u16::from_be_bytes([head[2], head[3]]));
            let len = usize::from(u16::from_be_bytes([head[4], head[5]])) + usize::from(head[6]);
            let mut content = vec![0; len];
            if r.read_exact(&mut content).is_err() {
                return;
            }
            // The end of the request's body (an empty FCGI_STDIN): answer it.
            if t == 5 && len == 0 {
                thread::sleep(delay);
                let mut out = record(6, id, b"Content-Type: text/plain\r\n\r\nok");
                out.extend(record(6, id, b""));
                out.extend(record(3, id, &[0; 8]));
                if w.write_all(&out).is_err() {
                    return;
                }
            }
        }
    })
}

/// A TCP server that echoes what it reads, `delay` after it read it.
fn slow_echo(delay: Duration) -> SocketAddr {
    server(move |mut s| {
        let mut buf = [0u8; 1024];
        loop {
            let n = match s.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            thread::sleep(delay);
            if s.write_all(&buf[..n]).is_err() {
                return;
            }
        }
    })
}

/// A directory of the test's own, with `name` holding `bytes`; its path.
fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "akit-workers-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    path
}

/// An HS256 token for `claims`, signed with [`SECRET`].
fn hs256(claims: &str) -> String {
    let signed = format!(
        "{}.{}",
        B64URL.encode(r#"{"alg":"HS256","typ":"JWT"}"#),
        B64URL.encode(claims)
    );
    let mac = Hmac::<Sha256>::mac(SECRET.as_bytes(), signed.as_bytes());
    format!("{signed}.{}", B64URL.encode(mac.as_ref()))
}

/// What the cases need: the slow upstream, auth service, FastCGI application and tunnel end,
/// a static root, the JWT secret's file and a token, fit's test plugin.
struct World {
    app: SocketAddr,
    auth: SocketAddr,
    fcgi: SocketAddr,
    echo: SocketAddr,
    root: PathBuf,
    secret: PathBuf,
    token: String,
    plugin: PathBuf,
}

impl World {
    fn new(delay: Duration) -> World {
        let index = temp_file("api/index.html", "static ".repeat(1000).as_bytes());
        let root = index.parent().unwrap().parent().unwrap().to_owned();
        let exp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        let gz = std::fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../akit-plugin/tests/guests/headers-akit.wasm.gz"),
        )
        .unwrap();
        let wasm = akio_deflate::decompress(akio_deflate::Format::Gzip, &gz, 64 << 20).unwrap();
        World {
            app: http_upstream(delay),
            auth: http_upstream(delay),
            fcgi: fastcgi_app(delay),
            echo: slow_echo(delay),
            root,
            secret: temp_file("secret", SECRET.as_bytes()),
            token: hs256(&format!(r#"{{"sub":"me","exp":{exp}}}"#)),
            plugin: temp_file("headers.wasm", &wasm),
        }
    }
}

// ---- The cases ----------------------------------------------------------------------------------

/// A module's case: the route's middleware and handler, the fields its request carries and the
/// status it gets. `inline` says the module answers without what is slow, as a redirect does.
struct Case {
    module: &'static str,
    middleware: String,
    handler: String,
    fields: Vec<(&'static str, String)>,
    status: u16,
    inline: bool,
}

/// The cases, every middleware wrapping a `proxy` to the upstream `app`.
fn cases(w: &World) -> Vec<Case> {
    let proxy = r#"{"module": "proxy", "params": {"upstream": "app"}}"#.to_owned();
    let mw = |module: &'static str, params: &str| Case {
        module,
        middleware: format!(r#"{{"module": "{module}", "params": {params}}}"#),
        handler: proxy.clone(),
        fields: Vec::new(),
        status: 200,
        inline: false,
    };
    let handler = |module: &'static str, handler: String, status: u16, inline: bool| Case {
        module,
        middleware: String::new(),
        handler,
        fields: Vec::new(),
        status,
        inline,
    };
    let mut basic = mw(
        "basic_auth",
        &format!(r#"{{"users": [{{"username": "me", "password_hash": "{HASH}"}}]}}"#),
    );
    basic
        .fields
        .push(("Authorization", format!("Basic {}", B64.encode("me:pw"))));
    let mut jwt = mw(
        "jwt",
        &format!(
            r#"{{"hmac_secret_file": "{}", "algorithms": ["HS256"]}}"#,
            w.secret.display()
        ),
    );
    jwt.fields
        .push(("Authorization", format!("Bearer {}", w.token)));
    let mut compress = mw("compress", "{}");
    compress.fields.push(("Accept-Encoding", "gzip".into()));
    let mut redirect = mw("redirect", r#"{"to": "/elsewhere"}"#);
    (redirect.status, redirect.inline) = (302, true);
    vec![
        mw("rewrite", r#"{"strip_prefix": ["/api"]}"#),
        redirect,
        mw(
            "headers",
            r#"{"request": {"set": {"X-A": "1"}}, "response": {"set": {"X-B": "2"}}}"#,
        ),
        mw("rate_limit", r#"{"rate": 1000000000, "burst": 1000000000}"#),
        mw("concurrency_limit", r#"{"max": 1000000}"#),
        mw("body_limit", r#"{"max_bytes": 1000}"#),
        basic,
        mw("client_cert", r#"{"optional": true}"#),
        // The auth service is the slow part, and the handler answers at once.
        Case {
            middleware: r#"{"module": "forward_auth", "params": {"upstream": "auth"}}"#.into(),
            handler: r#"{"module": "respond", "params": {"body": "in"}}"#.into(),
            ..mw("forward_auth", "{}")
        },
        jwt,
        mw("circuit_breaker", "{}"),
        compress,
        // The upstream's response is not kept (no-store): every request goes through to it.
        mw("cache", r#"{"zone": "workers"}"#),
        // fit's test plugin sets a field and passes the request on, from akio's pool.
        mw("wasm", &format!(r#"{{"path": "{}"}}"#, w.plugin.display())),
        handler("proxy", proxy.clone(), 200, false),
        handler(
            "fastcgi",
            format!(
                r#"{{"module": "fastcgi", "params": {{"address": "{}", "root": "/srv"}}}}"#,
                w.fcgi
            ),
            200,
            false,
        ),
        handler(
            "static",
            format!(
                r#"{{"module": "static", "params": {{"root": "{}"}}}}"#,
                w.root.display()
            ),
            200,
            true,
        ),
        handler(
            "respond",
            r#"{"module": "respond", "params": {"body": "x"}}"#.into(),
            200,
            true,
        ),
        // I113: whip and whep answer a request that is not an offer at once (405); webrtc_proxy
        // answers 503 while its role does not run (I114), and passes others on to a media server.
        handler("whip", r#"{"module": "whip"}"#.into(), 405, true),
        handler("whep", r#"{"module": "whep"}"#.into(), 405, true),
        handler(
            "webrtc_proxy",
            r#"{"module": "webrtc_proxy", "params": {"upstream": "app"}}"#.into(),
            503,
            true,
        ),
        // I151: bt_tracker answers 503 at once while its role does not run; with it, an
        // announce is a lookup and a shard lock (akit-bittorrent's tests).
        handler(
            "bt_tracker",
            r#"{"module": "bt_tracker"}"#.into(),
            503,
            true,
        ),
        // A CONNECT to the slow echo: the tunnel opens at once, its bytes come back late.
        handler(
            "forward_proxy",
            format!(
                r#"{{"module": "forward_proxy", "params": {{"clients": ["127.0.0.0/8"], "allow": ["127.0.0.0/8"], "ports": [{}]}}}}"#,
                w.echo.port()
            ),
            200,
            false,
        ),
    ]
}

/// The config serving each case on a host of its own ([`host`]), `forward_proxy` on
/// CONNECT requests.
fn config(w: &World, cases: &[Case]) -> String {
    let routes: Vec<String> = cases
        .iter()
        .map(|c| {
            let mw = if c.middleware.is_empty() {
                "[]".to_owned()
            } else {
                format!("[{}]", c.middleware)
            };
            let m = if c.module == "forward_proxy" {
                r#"{"method": ["CONNECT"]}"#.to_owned()
            } else {
                format!(r#"{{"host": ["{}"]}}"#, host(c.module))
            };
            format!(
                r#"{{"match": {m}, "middleware": {mw}, "handler": {}}}"#,
                c.handler
            )
        })
        .collect();
    format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1"]}}],
  "upstreams": [{{"name": "app", "addresses": ["{}"]}}, {{"name": "auth", "addresses": ["{}"]}}],
  "routes": [{}]}}"#,
        w.app,
        w.auth,
        routes.join(",\n")
    )
}

/// The host name of `module`'s route: `<module>.test`, `_` written `-`.
fn host(module: &str) -> String {
    format!("{}.test", module.replace('_', "-"))
}

/// akit serving `json` with every module akit's binary registers.
struct Akit {
    engine: Arc<Engine>,
    addr: SocketAddr,
    _cell: GraphCell,
}

impl Akit {
    fn start(json: &str) -> Akit {
        let config = akit_config::from_json(json, None).unwrap_or_else(|e| panic!("{json}: {e}"));
        let registry: Registry<HandlerParams> = Registry::new(akit::STANDARD).unwrap();
        let slot = EngineSlot::new();
        let cell = GraphCell::new(compile(&config, &registry, &Env::new(slot.clone())).unwrap());
        let engine = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
        engine
            .telemetry()
            .live()
            .set_slow_callback_threshold(Some(QUICK));
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

/// Sends `c`'s request to akit at `addr` and waits for the whole answer; its status.
fn request(addr: SocketAddr, c: &Case) -> u16 {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(WAIT)).unwrap();
    if c.module == "forward_proxy" {
        let target = format!("127.0.0.1:{}", c.handler_port());
        write!(s, "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        let status = line.split(' ').nth(1).unwrap().parse().unwrap();
        while line != "\r\n" {
            line.clear();
            assert!(r.read_line(&mut line).unwrap() > 0, "the head ended early");
        }
        s.write_all(b"ping").unwrap();
        let mut back = [0u8; 4];
        r.read_exact(&mut back).unwrap();
        assert_eq!(&back, b"ping");
        return status;
    }
    let mut req = format!(
        "GET /api/index.html HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
        host(c.module)
    );
    for (n, v) in &c.fields {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    let text = String::from_utf8_lossy(&out);
    text.split(' ')
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("{}: no answer: {text:?}", c.module))
}

impl Case {
    /// The port of `forward_proxy`'s allowed destination.
    fn handler_port(&self) -> u16 {
        let at = self.handler.find(r#""ports": ["#).unwrap() + 10;
        self.handler[at..]
            .split(']')
            .next()
            .unwrap()
            .parse()
            .unwrap()
    }
}

/// fit's `TestEveryModuleCovered`: a middleware or handler akit's binary registers with no case
/// here fails.
#[test]
fn every_module_covered() {
    let w = World {
        app: "127.0.0.1:1".parse().unwrap(),
        auth: "127.0.0.1:1".parse().unwrap(),
        fcgi: "127.0.0.1:1".parse().unwrap(),
        echo: "127.0.0.1:1".parse().unwrap(),
        root: PathBuf::new(),
        secret: PathBuf::new(),
        token: String::new(),
        plugin: PathBuf::new(),
    };
    let have: Vec<&str> = cases(&w).iter().map(|c| c.module).collect();
    let registry: Registry<HandlerParams> = Registry::new(akit::STANDARD).unwrap();
    let mut missing = Vec::new();
    for ns in ["http.middleware", "http.handlers"] {
        for info in registry.namespace(ns) {
            if !have.contains(&info.name()) {
                missing.push(info.id);
            }
        }
    }
    assert!(
        missing.is_empty(),
        "modules with no case: {missing:?}; add one to cases"
    );
    assert_eq!(registry.namespace("http.middleware").count(), 14);
}

/// fit's `TestNoModuleBlocks`: each case served twice, the first warming it up; no callback
/// on an I/O thread runs past [`QUICK`] meanwhile, and the answer, unless the module gives it
/// itself, comes once the slow part gave it.
#[test]
fn no_module_blocks() {
    let w = World::new(SLOW);
    let cases = cases(&w);
    let akit = Akit::start(&config(&w, &cases));
    for c in &cases {
        for i in 0..2 {
            let start = Instant::now();
            let status = request(akit.addr, c);
            let answered = start.elapsed();
            assert_eq!(status, c.status, "{}: status", c.module);
            let slow = akit.engine.slow_callbacks();
            assert!(
                slow.is_empty(),
                "{} (request {i}): callbacks on an I/O thread past {QUICK:?}: {slow:?}",
                c.module
            );
            if i == 1 && !c.inline {
                assert!(
                    answered >= SLOW * 3 / 4,
                    "{}: answered after {answered:?}, before the slow part ({SLOW:?}) could have",
                    c.module
                );
            }
        }
    }
    assert_eq!(akit.engine.metrics().total.slow_callbacks, 0);
}
