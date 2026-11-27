//! The `forward_auth` middleware through akit's listener on akio, with fit's tests
//! (`middleware/auth/forward_test.go`) ported, on the backend `AKIO_TEST_BACKEND` names (epoll by
//! default, `uring`): a request let through with the auth service's fields, the fields a client may
//! not forge in any spelling, a refusal passed on with its body, one whose body is too large to,
//! an auth service that is slow (requests wait without holding the I/O thread), one that does not
//! answer in time, one that cannot be reached, a client that goes while its request waits, the
//! fields `request_headers` limits the auth service to, and the params akit refuses.
//!
//! The auth service is a test server on plain threads, which answers by the request's
//! `Authorization` field as fit's test service does. The route's handler is a test one,
//! registered as `respond`: it answers with the fields `forward_auth` copies, as an application
//! behind CGI would read them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, UringConfig};
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

/// The test handler: fit's `fieldsHandler`.
mod fields {
    use std::sync::Arc;

    use akio_http::{Context, Response};
    use akit_runtime::{Handler, HandlerParams, Info, Provision, ProvisionError, Provisioned};

    /// What an application behind CGI reads as field `name`: fit's `asCGI`.
    fn as_cgi(h: &akio_http::Headers, name: &str) -> String {
        let fold = |s: &str| s.replace('_', "-").to_ascii_lowercase();
        let mut values: Vec<&str> = h
            .iter()
            .filter(|(n, _)| fold(n) == fold(name))
            .map(|(_, v)| v)
            .collect();
        values.sort_unstable();
        values.join(",")
    }

    struct Fields;

    impl akio_http::Service for Fields {
        fn call(&self, ctx: &mut Context) {
            let body = {
                let h = &ctx.req().headers;
                format!(
                    "user={} seen={}",
                    as_cgi(h, "X-Auth-User"),
                    as_cgi(h, "X-Auth-Seen")
                )
            };
            ctx.respond(Response::text(200, body));
        }
    }

    impl Handler for Fields {}

    fn new(_: &Provision<'_, HandlerParams>) -> Result<Provisioned<dyn Handler>, ProvisionError> {
        Ok(Provisioned::new(Arc::new(Fields) as Arc<dyn Handler>))
    }

    pub(crate) static MODULES: &[Info<HandlerParams>] =
        &[Info::handler("http.handlers.respond", "Fields.", new)];
}

/// akit serving `json`.
struct Akit {
    engine: Arc<Engine>,
    addr: SocketAddr,
    _cell: GraphCell,
}

impl Akit {
    fn start(json: &str) -> Akit {
        let mut config =
            akit_config::from_json(json, None).unwrap_or_else(|e| panic!("{json}: {e}"));
        // Two I/O threads, not one a core: the tests run at once, each thread holds an epoll or
        // io_uring and an eventfd, and one a core on a 16-core host came close to the 1024
        // descriptors a process may open by default.
        config.engine.event_loops = 2;
        let slot = EngineSlot::new();
        let registry = Registry::new(&[
            fields::MODULES as &[Info<HandlerParams>],
            akit_middleware::MODULES,
            akit_proxy::MODULES,
        ])
        .unwrap();
        let cell = GraphCell::new(compile(&config, &registry, &Env::new(slot.clone())).unwrap());
        let engine = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
        engine.start().unwrap();
        assert!(slot.set(engine.clone()));
        let addr = engine.local_addrs()[0];
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

/// fit's `forwardDoc`, with `params` added to forward_auth's.
fn forward_doc(auth: SocketAddr, timeout: &str, params: &str) -> String {
    format!(
        r#"{{
  "listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
  "upstreams": [{{"name": "auth", "addresses": ["{auth}"]}}],
  "routes": [
    {{"match": {{"path": ["/fast"]}}, "handler": {{"module": "respond"}}}},
    {{"middleware": [{{"module": "forward_auth", "params": {{"upstream": "auth", "uri": "/check?app=1",
        "copy_headers": ["x-auth-user", "X-Auth-Seen"], "timeout": "{timeout}"{params}}}}}],
      "handler": {{"module": "respond"}}}}
  ]
}}"#
    )
}

// ---- the auth service ----

/// fit's `authService`: answers by the request's `Authorization` field.
struct AuthService {
    addr: SocketAddr,
    /// Requests whose client went before the answer.
    cancelled: Arc<AtomicUsize>,
}

/// A request head as the auth service read it: its request line's method and target, and its
/// fields.
struct Seen {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
}

impl Seen {
    fn get(&self, name: &str) -> &str {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map_or("", |(_, v)| v.as_str())
    }
}

fn read_head(r: &mut impl BufRead) -> Option<Seen> {
    let mut line = String::new();
    if r.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.split(' ');
    let method = parts.next()?.to_owned();
    let target = parts.next()?.to_owned();
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let (n, v) = line.split_once(':')?;
        headers.push((n.to_owned(), v.trim().to_owned()));
    }
    Some(Seen {
        method,
        target,
        headers,
    })
}

/// Waits `d` for the connection's client to go: true if it went first.
fn went_within(s: &TcpStream, d: Duration) -> bool {
    let end = Instant::now() + d;
    let mut buf = [0u8; 64];
    loop {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        s.set_read_timeout(Some(left)).unwrap();
        match (&*s).read(&mut buf) {
            Ok(0) => return true,
            Ok(_) => {}
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return false;
            }
            Err(_) => return true,
        }
    }
}

fn answer(
    w: &mut TcpStream,
    status: u16,
    fields: &[(&str, &str)],
    body: &[u8],
) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {status} X\r\n");
    for (n, v) in fields {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    if status != 204 {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    w.write_all(head.as_bytes())?;
    w.write_all(body)
}

fn serve_auth(s: TcpStream, cancelled: &AtomicUsize) {
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut w = s;
    while let Some(req) = read_head(&mut r) {
        let seen = [
            req.method.as_str(),
            &req.target,
            req.get("X-Forwarded-Method"),
            req.get("X-Forwarded-Proto"),
            req.get("X-Forwarded-Host"),
            req.get("X-Forwarded-Uri"),
            req.get("X-Forwarded-For"),
            req.get("X-Custom"),
        ]
        .join(" ");
        let ok = match req.get("Authorization") {
            "Bearer good" => answer(
                &mut w,
                200,
                &[
                    ("X-Auth-User", "alice"),
                    ("X-Auth-Seen", &seen),
                    ("X-Not-Copied", "1"),
                ],
                b"ignored",
            ),
            "Bearer nouser" => answer(&mut w, 204, &[], b""),
            auth @ ("Bearer slow" | "Bearer hang") => {
                let d = if auth == "Bearer hang" {
                    Duration::from_secs(10)
                } else {
                    Duration::from_millis(500)
                };
                if went_within(&w, d) {
                    cancelled.fetch_add(1, Ordering::SeqCst);
                    return;
                }
                w.set_read_timeout(None).unwrap();
                answer(&mut w, 200, &[("X-Auth-User", "slow")], b"")
            }
            "Bearer big" => answer(&mut w, 401, &[], &vec![b'x'; 200 << 10]),
            _ => answer(
                &mut w,
                302,
                &[("Location", "/login"), ("Set-Cookie", "next=/p")],
                b"go log in",
            ),
        };
        if ok.is_err() {
            return;
        }
    }
}

impl AuthService {
    fn start() -> AuthService {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let cancelled = Arc::new(AtomicUsize::new(0));
        let c = cancelled.clone();
        // Runs as long as the test process: each test has its own.
        thread::spawn(move || {
            for s in l.incoming().flatten() {
                let c = c.clone();
                thread::spawn(move || serve_auth(s, &c));
            }
        });
        AuthService { addr, cancelled }
    }
}

// ---- the client ----

/// A response as the client saw it.
#[derive(Debug)]
struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// One request on a connection of its own, with `auth` as its `Authorization` when not empty,
/// and `headers`.
fn fetch(addr: SocketAddr, method: &str, path: &str, auth: &str, headers: &[(&str, &str)]) -> Resp {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: example.com\r\n");
    if !auth.is_empty() {
        req.push_str(&format!("Authorization: {auth}\r\n"));
    }
    for (n, v) in headers {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    req.push_str("\r\n");
    (&s).write_all(req.as_bytes()).unwrap();
    let mut r = BufReader::new(s);
    let head = read_head(&mut r).expect("a response");
    let status = head.target.parse().unwrap();
    let mut resp = Resp {
        status,
        headers: head.headers,
        body: String::new(),
    };
    let length: usize = resp
        .header("content-length")
        .map_or(0, |v| v.parse().unwrap());
    let mut body = vec![0; length];
    r.read_exact(&mut body).unwrap();
    resp.body = String::from_utf8(body).unwrap();
    resp
}

/// fit's `spellings`.
fn spellings(name: &str) -> Vec<String> {
    let under = name.replace('-', "_");
    vec![
        under.clone(),
        under.to_ascii_lowercase(),
        under.to_ascii_uppercase(),
        name.replacen('-', "_", 1),
    ]
}

// fit's TestForwardAuth.
#[test]
fn forward_auth() {
    let a = AuthService::start();
    let akit = Akit::start(&forward_doc(a.addr, "300ms", ""));
    let base = akit.addr;

    let r = fetch(base, "POST", "/p?q=1", "Bearer good", &[("X-Custom", "c")]);
    assert_eq!(
        (r.status, r.body.as_str()),
        (
            200,
            "user=alice seen=GET /check?app=1 POST http example.com /p?q=1 127.0.0.1 c"
        ),
        "allowed"
    );

    // Fields the client sends in copy_headers' names are not believed.
    let r = fetch(
        base,
        "GET",
        "/p",
        "Bearer nouser",
        &[("X-Auth-User", "mallory")],
    );
    assert_eq!((r.status, r.body.as_str()), (200, "user= seen="));
    // Nor in a spelling an application behind CGI reads as the same field.
    for name in spellings("X-Auth-User") {
        let r = fetch(base, "GET", "/p", "Bearer nouser", &[(&name, "mallory")]);
        assert_eq!((r.status, r.body.as_str()), (200, "user= seen="), "{name}");
    }

    let r = fetch(base, "GET", "/p", "Bearer bad", &[]);
    assert_eq!(r.status, 302, "denied: {r:?}");
    assert_eq!(r.header("location"), Some("/login"));
    assert_eq!(r.header("set-cookie"), Some("next=/p"));
    assert_eq!(r.body, "go log in");

    let r = fetch(base, "GET", "/p", "Bearer big", &[]);
    assert_eq!(
        (r.status, r.body.len()),
        (401, 0),
        "denied with a large body"
    );

    let r = fetch(base, "GET", "/p", "Bearer hang", &[]);
    assert_eq!(r.status, 504, "past the timeout");

    // The route without forward_auth is not asked.
    assert_eq!(fetch(base, "GET", "/fast", "", &[]).status, 200);
}

// fit's TestForwardAuthUnreachable.
#[test]
fn forward_auth_unreachable() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    let akit = Akit::start(&forward_doc(addr, "5s", ""));
    let r = fetch(akit.addr, "GET", "/p", "Bearer good", &[]);
    assert_eq!(r.status, 502, "no auth service");
}

// fit's TestForwardAuthClientGone: the auth service's request is given up when the client goes.
#[test]
fn forward_auth_client_gone() {
    let a = AuthService::start();
    let akit = Akit::start(&forward_doc(a.addr, "30s", ""));
    let s = TcpStream::connect(akit.addr).unwrap();
    (&s).write_all(b"GET /p HTTP/1.1\r\nHost: example.com\r\nAuthorization: Bearer hang\r\n\r\n")
        .unwrap();
    thread::sleep(Duration::from_millis(300));
    drop(s);
    let deadline = Instant::now() + Duration::from_secs(5);
    while a.cancelled.load(Ordering::SeqCst) == 0 {
        assert!(
            Instant::now() < deadline,
            "the auth service's request was not given up"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

// fit's TestForwardAuthSlowService: many requests wait for a slow auth service at once, more than
// akio has I/O threads, and another route answers at once meanwhile: the waiting requests hold
// no thread.
#[test]
fn forward_auth_slow_service() {
    let a = AuthService::start();
    let akit = Akit::start(&forward_doc(a.addr, "10s", ""));
    let base = akit.addr;
    const WAITING: usize = 128;
    let start = Instant::now();
    let slow: Vec<_> = (0..WAITING)
        .map(|_| {
            thread::spawn(move || {
                let r = fetch(base, "GET", "/p", "Bearer slow", &[]);
                r.status == 200 && r.body == "user=slow seen="
            })
        })
        .collect();
    thread::sleep(Duration::from_millis(100)); // the slow ones are waiting
    for i in 0..10 {
        let t0 = Instant::now();
        let r = fetch(base, "GET", "/fast", "", &[]);
        let d = t0.elapsed();
        assert!(
            r.status == 200 && d < Duration::from_millis(250),
            "fast request {i}: {} after {d:?}",
            r.status
        );
    }
    let allowed = slow
        .into_iter()
        .map(|t| t.join().unwrap())
        .filter(|ok| *ok)
        .count();
    assert_eq!(allowed, WAITING, "slow requests let in");
    let d = start.elapsed();
    assert!(
        d < Duration::from_secs(5),
        "the slow requests took {d:?}: they were not waited for at once"
    );
}

/// With `request_headers`, the auth service gets those fields alone (and its own); a body that
/// streams waits for the check, and the handler is answered.
#[test]
fn forward_auth_request_headers() {
    let a = AuthService::start();
    let akit = Akit::start(&forward_doc(
        a.addr,
        "5s",
        r#", "request_headers": ["authorization"]"#,
    ));
    let r = fetch(akit.addr, "GET", "/p", "Bearer good", &[("X-Custom", "c")]);
    assert_eq!(
        (r.status, r.body.as_str()),
        (
            200,
            // The trailing empty X-Custom is trimmed with the field value.
            "user=alice seen=GET /check?app=1 GET http example.com /p 127.0.0.1"
        ),
    );
    // A request with a body of no declared length, which the handler would have stream.
    let s = TcpStream::connect(akit.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    (&s).write_all(
        b"POST /p HTTP/1.1\r\nHost: example.com\r\nAuthorization: Bearer nouser\r\n\
          Transfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
    )
    .unwrap();
    let mut r = BufReader::new(s);
    let head = read_head(&mut r).unwrap();
    assert_eq!(head.target, "200");
}

/// fit's `examples/forward-auth`: `forward_auth` in front of the `proxy` handler. The app gets
/// the auth service's field, and a body larger than the proxy streams.
#[test]
fn forward_auth_then_proxy() {
    let a = AuthService::start();
    let app = TcpListener::bind("127.0.0.1:0").unwrap();
    let app_addr = app.local_addr().unwrap();
    thread::spawn(move || {
        for s in app.incoming().flatten() {
            thread::spawn(move || {
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut w = s;
                while let Some(req) = read_head(&mut r) {
                    let n: usize = req.get("Content-Length").parse().unwrap_or(0);
                    let mut body = vec![0; n];
                    if r.read_exact(&mut body).is_err() {
                        return;
                    }
                    let text = format!("user={} body={n}", req.get("X-Auth-User"));
                    if answer(&mut w, 200, &[], text.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
    let akit = Akit::start(&format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
            "upstreams": [{{"name": "auth", "addresses": ["{}"]}}, {{"name": "app", "addresses": ["{app_addr}"]}}],
            "routes": [{{"middleware": [{{"module": "forward_auth", "params": {{"upstream": "auth",
                           "request_headers": ["Authorization"], "copy_headers": ["X-Auth-User"]}}}}],
                         "handler": {{"module": "proxy", "params": {{"upstream": "app"}}}}}}]}}"#,
        a.addr
    ));
    let r = fetch(akit.addr, "GET", "/p", "Bearer good", &[]);
    assert_eq!((r.status, r.body.as_str()), (200, "user=alice body=0"));
    let r = fetch(akit.addr, "GET", "/p", "Bearer bad", &[]);
    assert_eq!((r.status, r.body.as_str()), (302, "go log in"));
    // 200 KiB, past the proxy's stream threshold.
    let s = TcpStream::connect(akit.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let n = 200 << 10;
    let head = format!(
        "POST /p HTTP/1.1\r\nHost: example.com\r\nAuthorization: Bearer good\r\nContent-Length: {n}\r\n\r\n"
    );
    (&s).write_all(head.as_bytes()).unwrap();
    (&s).write_all(&vec![b'x'; n]).unwrap();
    let mut r = BufReader::new(s);
    let head = read_head(&mut r).unwrap();
    let len: usize = head.get("Content-Length").parse().unwrap();
    let mut body = vec![0; len];
    r.read_exact(&mut body).unwrap();
    assert_eq!(
        (head.target.as_str(), String::from_utf8(body).unwrap()),
        ("200", format!("user=alice body={n}"))
    );
}

/// A body that streams waits with the request while the auth service checks it (akio's
/// suspension with its reads held, ADR-159), then streams to the proxy's upstream, whole and in
/// order: the upstream has the request's head before the client sent the second half of its
/// body, which it would not if the body were read whole for the check.
#[test]
fn forward_auth_streams_the_body() {
    let a = AuthService::start();
    let app = TcpListener::bind("127.0.0.1:0").unwrap();
    let app_addr = app.local_addr().unwrap();
    let (heads, got_head) = std::sync::mpsc::channel::<()>();
    thread::spawn(move || {
        for s in app.incoming().flatten() {
            let heads = heads.clone();
            thread::spawn(move || {
                s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut w = s;
                while let Some(req) = read_head(&mut r) {
                    let _ = heads.send(());
                    let n: usize = req.get("Content-Length").parse().unwrap_or(0);
                    let mut body = vec![0; n];
                    if r.read_exact(&mut body).is_err() {
                        return;
                    }
                    let in_order = body.iter().enumerate().all(|(i, &b)| b == pattern(i));
                    let text = format!(
                        "user={} body={n} in_order={in_order}",
                        req.get("X-Auth-User")
                    );
                    if answer(&mut w, 200, &[], text.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
    let akit = Akit::start(&format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
            "upstreams": [{{"name": "auth", "addresses": ["{}"]}}, {{"name": "app", "addresses": ["{app_addr}"]}}],
            "routes": [{{"middleware": [{{"module": "forward_auth", "params": {{"upstream": "auth",
                           "request_headers": ["Authorization"], "copy_headers": ["X-Auth-User"]}}}}],
                         "handler": {{"module": "proxy", "params": {{"upstream": "app"}}}}}}]}}"#,
        a.addr
    ));
    // 1 MiB, past the proxy's stream threshold, the second half sent once the upstream has the
    // request's head.
    let n = 1 << 20;
    let body: Vec<u8> = (0..n).map(pattern).collect();
    let s = TcpStream::connect(akit.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.set_write_timeout(Some(Duration::from_secs(10))).unwrap();
    let head = format!(
        "POST /p HTTP/1.1\r\nHost: example.com\r\nAuthorization: Bearer good\r\nContent-Length: {n}\r\n\r\n"
    );
    (&s).write_all(head.as_bytes()).unwrap();
    let (first, rest) = body.split_at(n / 2);
    (&s).write_all(first).unwrap();
    got_head
        .recv_timeout(Duration::from_secs(10))
        .expect("the upstream never had the request before the whole body was sent");
    (&s).write_all(rest).unwrap();
    let mut r = BufReader::new(s);
    let head = read_head(&mut r).unwrap();
    let len: usize = head.get("Content-Length").parse().unwrap();
    let mut answer = vec![0; len];
    r.read_exact(&mut answer).unwrap();
    assert_eq!(
        (head.target.as_str(), String::from_utf8(answer).unwrap()),
        ("200", format!("user=alice body={n} in_order=true"))
    );
}

/// Byte `i` of a test body: a pattern a reordered or lost part breaks.
fn pattern(i: usize) -> u8 {
    u8::try_from(i % 251).unwrap()
}

// fit's TestForwardAuthParams, and akit-config's checks.
#[test]
fn forward_auth_params() {
    for (params, want) in [
        (r#"{"upstream": "auth", "uri": "check"}"#, "not a path"),
        (
            r#"{"upstream": "auth", "uri": "http://a/check"}"#,
            "not a path",
        ),
        (r#"{"upstream": "auth", "uri": "/a%zz"}"#, "not a path"),
        (
            r#"{"upstream": "auth", "request_headers": ["a b"]}"#,
            r#"request_headers[0]: "a b" is not a field name"#,
        ),
        (
            r#"{"upstream": "auth", "copy_headers": ["x:y"]}"#,
            r#"copy_headers[0]: "x:y" is not a field name"#,
        ),
        (
            r#"{"upstream": "auth", "timeout": "soon"}"#,
            "not a duration",
        ),
        (r#"{"uri": "/check"}"#, "upstream: missing required field"),
        (r#"{"upstream": "auth", "url": "/"}"#, "unknown field"),
    ] {
        let json = format!(
            r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
                "upstreams": [{{"name": "auth", "addresses": ["127.0.0.1:1"]}}],
                "routes": [{{"middleware": [{{"module": "forward_auth", "params": {params}}}],
                             "handler": {{"module": "respond"}}}}]}}"#
        );
        let errs = match akit_config::from_json(&json, None) {
            Ok(_) => String::new(),
            Err(e) => e.to_string(),
        };
        assert!(errs.contains(want), "{params}: {errs:?}, want {want:?}");
    }
    // Without params, and the defaults.
    let json = r#"{"listeners": [{"name": "web", "address": ":8080"}],
        "routes": [{"middleware": [{"module": "forward_auth"}], "handler": {"module": "respond"}}]}"#;
    let errs = akit_config::from_json(json, None).unwrap_err().to_string();
    assert!(
        errs.contains("params.upstream: missing required field"),
        "{errs}"
    );
    let json = r#"{"listeners": [{"name": "web", "address": ":8080"}],
        "routes": [{"middleware": [{"module": "forward_auth", "params": {"upstream": "auth"}}],
                    "handler": {"module": "respond"}}]}"#;
    let c = akit_config::from_json(json, None).unwrap();
    assert_eq!(
        c.routes[0].middleware[0],
        akit_config::Handler::ForwardAuth(akit_config::ForwardAuth {
            upstream: "auth".into(),
            uri: "/".into(),
            request_headers: Vec::new(),
            copy_headers: Vec::new(),
            timeout: akit_config::DEFAULT_FORWARD_AUTH_TIMEOUT,
        })
    );
    // An upstream the config does not have is refused when the middleware is provisioned.
    let json = r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}],
        "upstreams": [{"name": "auth", "addresses": ["127.0.0.1:1"]}],
        "routes": [{"middleware": [{"module": "forward_auth", "params": {"upstream": "nope"}}],
                    "handler": {"module": "respond"}}]}"#;
    let c = akit_config::from_json(json, None).unwrap();
    let registry = Registry::new(&[
        fields::MODULES as &[Info<HandlerParams>],
        akit_middleware::MODULES,
    ])
    .unwrap();
    let Err(err) = compile(&c, &registry, &Env::new(EngineSlot::new())) else {
        panic!("an unknown upstream was taken");
    };
    let err = err.to_string();
    assert!(
        err.contains(r#"there is no upstream named "nope""#),
        "{err}"
    );
}
