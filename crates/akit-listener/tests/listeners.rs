//! Listeners from a config on akio: HTTP/1.1 and h2c, routing per listener, and 404. On the
//! backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use akio::{Backend, Engine, UringConfig};
use akio_http::client::ClientRequest;
use akio_http::h2client::{H2Client, H2Options};
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

/// Serves `json`; returns the engine and where each listener listens.
fn serve(json: &str) -> (Engine, Vec<SocketAddr>) {
    let config = akit_config::from_json(json, None).unwrap();
    let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
    let cell = GraphCell::new(compile(&config, &registry, &Env::default()).unwrap());
    let e = akit_listener::engine(&cell, &options()).unwrap();
    e.start().unwrap();
    let addrs = e.local_addrs();
    (e, addrs)
}

/// An HTTP/1.1 GET; the status line and the body.
fn get(addr: SocketAddr, host: &str, path: &str) -> (String, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    let (head, body) = resp.split_once("\r\n\r\n").unwrap();
    (head.lines().next().unwrap().to_owned(), body.to_owned())
}

/// An HTTP/2 GET with prior knowledge, in the clear; the status and the body.
fn get_h2c(addr: SocketAddr, path: &str) -> Result<(u16, String), String> {
    let client = Engine::builder().client_threads(1).build().unwrap();
    client.start().unwrap();
    let c = H2Client::dial_blocking(&client, addr, None, H2Options::default())
        .map_err(|e| e.to_string())?;
    let r = c
        .send(ClientRequest::get(&format!("http://{addr}{path}")).timeout(Duration::from_secs(5)))
        .wait()
        .map_err(|e| e.to_string())?;
    Ok((r.status, String::from_utf8(r.body).unwrap()))
}

const HELLO: &str = r#"{
  "listeners": [{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}],
  "routes": [
    {"name": "hello", "match": {"path": ["/hello"]},
     "handler": {"module": "respond", "params": {"body": "hello from fit\n"}}},
    {"name": "teapot", "match": {"host": ["tea.example"], "path": ["/hello"]},
     "handler": {"module": "respond", "params": {"status": 418, "headers": {"X-Tea": "1"}}}}
  ]
}"#;

#[test]
fn http1_and_h2c_with_404() {
    let (e, addrs) = serve(HELLO);
    let a = addrs[0];
    assert_eq!(
        get(a, "x", "/hello"),
        ("HTTP/1.1 200 OK".into(), "hello from fit\n".into())
    );
    // fit's answer when no route matches.
    assert_eq!(
        get(a, "x", "/nope"),
        (
            "HTTP/1.1 404 Not Found".into(),
            "404 page not found\n".into()
        )
    );
    // The more specific host wins.
    let (status, body) = get(a, "TEA.example:80", "/hello");
    assert!(status.starts_with("HTTP/1.1 418"), "{status}");
    assert_eq!(body, "");
    assert_eq!(
        get_h2c(a, "/hello").unwrap(),
        (200, "hello from fit\n".into())
    );
    assert_eq!(
        get_h2c(a, "/nope").unwrap(),
        (404, "404 page not found\n".into())
    );
    e.shutdown();
    e.wait();
}

#[test]
fn h2c_upgrade() {
    let (e, addrs) = serve(HELLO);
    let mut s = TcpStream::connect(addrs[0]).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(
        b"GET /hello HTTP/1.1\r\nHost: x\r\nConnection: Upgrade, HTTP2-Settings\r\n\
          Upgrade: h2c\r\nHTTP2-Settings: AAMAAABkAAQCAAAAAAIAAAAA\r\n\r\n",
    )
    .unwrap();
    let mut buf = [0u8; 64];
    let n = s.read(&mut buf).unwrap();
    let head = String::from_utf8_lossy(&buf[..n]);
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    e.shutdown();
    e.wait();
}

/// Without h2c (fit's default protocols), HTTP/2 is not served.
#[test]
fn http1_only_by_default() {
    let (e, addrs) = serve(
        r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}],
            "routes": [{"handler": {"module": "respond", "params": {"body": "one"}}}]}"#,
    );
    assert_eq!(get(addrs[0], "x", "/").1, "one");
    assert!(get_h2c(addrs[0], "/").is_err());
    e.shutdown();
    e.wait();
}

/// A route serves on the listeners it names, or on every one.
#[test]
fn routes_per_listener() {
    let (e, addrs) = serve(
        r#"{"listeners": [{"name": "a", "address": "127.0.0.1:0"},
                          {"name": "b", "address": "127.0.0.1:0"}],
            "routes": [
              {"listeners": ["a"], "match": {"path": ["/x"]},
               "handler": {"module": "respond", "params": {"body": "a"}}},
              {"handler": {"module": "respond", "params": {"body": "all"}}}]}"#,
    );
    assert_eq!(get(addrs[0], "h", "/x").1, "a");
    assert_eq!(get(addrs[1], "h", "/x").1, "all");
    assert_eq!(get(addrs[0], "h", "/y").1, "all");
    e.shutdown();
    e.wait();
}

/// fit's `TestProtocols` (I79b, akio's ADR-174): a listener of h2c alone serves HTTP/2 with prior
/// knowledge and nothing else, a listener of http/1.1 HTTP/1.1 alone.
#[test]
fn h2c_alone() {
    let (e, addrs) = serve(
        r#"{"listeners": [{"name": "h1", "address": "127.0.0.1:0"},
                          {"name": "h2c", "address": "127.0.0.1:0", "protocols": ["h2c"]}],
            "routes": [{"handler": {"module": "respond", "params": {"body": "ok"}}}]}"#,
    );
    let (h1, h2) = (addrs[0], addrs[1]);
    assert_eq!(get(h1, "h", "/").1, "ok");
    assert_eq!(get_h2c(h2, "/").unwrap(), (200, "ok".to_owned()));
    assert!(get_h2c(h1, "/").is_err(), "h2c on an http/1.1 listener");
    // HTTP/1.1 on the h2c listener: a GOAWAY, then closed; no HTTP/1.1 answer.
    let mut s = TcpStream::connect(h2).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(b"GET / HTTP/1.1\r\nHost: h\r\n\r\n").unwrap();
    let mut got = Vec::new();
    let _ = s.read_to_end(&mut got);
    assert!(!got.starts_with(b"HTTP/1.1"), "{got:?}");
    assert_eq!(got.get(3), Some(&7), "a GOAWAY frame: {got:?}");
    e.shutdown();
    e.wait();
}

/// A `respond` that answers 300 ms later, from another thread: a request in flight.
mod slow {
    use std::sync::Arc;
    use std::time::Duration;

    use akio_http::{Context, Response};
    use akit_runtime::{Handler, HandlerParams, Info, Provision, ProvisionError, Provisioned};

    struct Slow {
        delay: Duration,
        body: String,
    }

    impl akio_http::Service for Slow {
        fn call(&self, ctx: &mut Context) {
            let Some(d) = ctx.detach() else { return };
            let (delay, body) = (self.delay, self.body.clone());
            std::thread::spawn(move || {
                std::thread::sleep(delay);
                d.respond(Response::text(200, body));
            });
        }
    }

    impl Handler for Slow {}

    fn new(p: &Provision<'_, HandlerParams>) -> Result<Provisioned<dyn Handler>, ProvisionError> {
        let HandlerParams::Respond(r) = p.params() else {
            return Err(ProvisionError::new("a respond handler only"));
        };
        Ok(Provisioned::new(Arc::new(Slow {
            delay: Duration::from_millis(u64::from(r.status)),
            body: r.body.clone(),
        }) as Arc<dyn Handler>))
    }

    /// `respond`'s status is the delay in milliseconds.
    pub(crate) static MODULES: &[Info<HandlerParams>] = &[Info::handler(
        "http.handlers.respond",
        "Answers later.",
        new,
    )];
}

fn serve_slow(delay_ms: u16, shutdown_timeout: Duration) -> (Engine, SocketAddr) {
    let config = akit_config::from_json(
        &format!(
            r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
                "routes": [{{"handler": {{"module": "respond",
                             "params": {{"status": {delay_ms}, "body": "done"}}}}}}]}}"#
        ),
        None,
    )
    .unwrap();
    let registry: Registry<HandlerParams> = Registry::new(&[slow::MODULES]).unwrap();
    let cell = GraphCell::new(compile(&config, &registry, &Env::default()).unwrap());
    let mut o = options();
    o.shutdown_timeout = shutdown_timeout;
    let e = akit_listener::engine(&cell, &o).unwrap();
    e.start().unwrap();
    let a = e.local_addrs()[0];
    (e, a)
}

/// Sends a request and returns the connection, the request in flight.
fn send(addr: SocketAddr) -> TcpStream {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    s
}

#[test]
fn shutdown_lets_a_request_in_flight_finish() {
    let (e, addr) = serve_slow(300, Duration::from_secs(5));
    let mut s = send(addr);
    // An idle keep-alive connection is closed at once.
    let mut idle = TcpStream::connect(addr).unwrap();
    idle.set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let start = std::time::Instant::now();
    let in_time = std::thread::spawn(move || akit_listener::shutdown(&e, Duration::from_secs(5)));
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    assert!(resp.ends_with("done"), "{resp}");
    assert_eq!(idle.read(&mut [0u8; 16]).unwrap(), 0);
    assert!(in_time.join().unwrap());
    assert!(start.elapsed() < Duration::from_secs(3));
    // The listener no longer accepts.
    assert!(TcpStream::connect(addr).is_err());
}

#[test]
fn shutdown_closes_what_outlasts_its_timeout() {
    let (e, addr) = serve_slow(500, Duration::from_millis(100));
    let mut s = send(addr);
    std::thread::sleep(Duration::from_millis(100));
    let start = std::time::Instant::now();
    assert!(!akit_listener::shutdown(&e, Duration::from_millis(100)));
    assert!(start.elapsed() < Duration::from_millis(400));
    let mut resp = Vec::new();
    let _ = s.read_to_end(&mut resp);
    assert!(resp.is_empty(), "{}", String::from_utf8_lossy(&resp));
}

/// fit's `TestPathConfusionRoutes`, with `respond` for its handlers: paths spelled to miss a
/// protected route and reach the catch-all get the protected route, or 400.
#[test]
fn path_confusion() {
    let (e, addrs) = serve(
        r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}],
            "routes": [
              {"match": {"path": ["/admin/*"]},
               "handler": {"module": "respond", "params": {"status": 401, "body": "protected"}}},
              {"match": {"path": ["/php/admin/*"]},
               "handler": {"module": "respond", "params": {"status": 401, "body": "protected"}}},
              {"match": {"path": ["/php/*"]},
               "handler": {"module": "respond", "params": {"body": "php"}}},
              {"handler": {"module": "respond", "params": {"body": "open"}}}]}"#,
    );
    let a = addrs[0];
    for (target, status, body) in [
        ("/admin/secret", 401, "protected"),
        ("/x/../admin/secret", 401, "protected"),
        ("/x/%2e%2e/admin/secret", 401, "protected"),
        ("/x/.%2E/admin/secret", 401, "protected"),
        ("//admin/secret", 401, "protected"),
        ("/./admin/secret", 401, "protected"),
        ("/x%2F..%2Fadmin/secret", 400, "400 bad request path\n"),
        ("/%2Fadmin/secret", 400, "400 bad request path\n"),
        ("/x%5C..%5Cadmin/secret", 400, "400 bad request path\n"),
        ("/../admin/secret", 400, "400 bad request path\n"),
        ("/php/admin/panel.php", 401, "protected"),
        ("/php/x/../admin/panel.php", 401, "protected"),
        ("/php/x/%2e%2e/admin/panel.php", 401, "protected"),
        ("/php/a/../index.php", 200, "php"),
        ("/a/./b/../c//d?q=/../x", 200, "open"),
        // Routes match the decoded path, as fit's do.
        ("/%61dmin/secret", 401, "protected"),
        // A target in absolute form is routed by its path, as fit's (Go's) are: found by
        // fuzzing (I77), it went to the catch-all, which asked the upstream for that path.
        ("http://a/admin/secret", 401, "protected"),
        ("http://a/x/../admin/secret", 401, "protected"),
        ("https://user@a:8443/%61dmin/secret?x", 401, "protected"),
        ("HTTP://a/x/%2e%2e/admin/secret", 401, "protected"),
        ("http://a/../admin/secret", 400, "400 bad request path\n"),
        ("http://a", 200, "open"),
    ] {
        let (line, got) = get(a, "a", target);
        assert!(
            line.starts_with(&format!("HTTP/1.1 {status} ")),
            "{target}: {line}"
        );
        assert_eq!(got, body, "{target}");
        if target.starts_with('/') && !target.starts_with("//") {
            assert_eq!(
                get_h2c(a, target).unwrap(),
                (status, body.to_owned()),
                "h2c {target}"
            );
        }
    }
    e.shutdown();
    e.wait();
}

/// A target in absolute form names the host the request is for, which goes before its `Host`
/// field (RFC 9112 section 3.2.2), as Go's server, and so fit, takes it (I77).
#[test]
fn absolute_form_targets_name_their_host() {
    let (e, addrs) = serve(
        r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}],
            "routes": [
              {"match": {"host": ["b.example"]},
               "handler": {"module": "respond", "params": {"status": 401, "body": "b"}}},
              {"handler": {"module": "respond", "params": {"body": "open"}}}]}"#,
    );
    let a = addrs[0];
    for (host, target, status, body) in [
        ("a.example", "http://b.example/x", 401, "b"),
        ("a.example", "http://u:p@B.example:80/x", 401, "b"),
        ("b.example", "http://a.example/x", 200, "open"),
        ("b.example", "http:///x", 401, "b"),
        ("b.example", "/x", 401, "b"),
    ] {
        let (line, got) = get(a, host, target);
        assert!(
            line.starts_with(&format!("HTTP/1.1 {status} ")),
            "{host} {target}: {line}"
        );
        assert_eq!(got, body, "{host} {target}");
    }
    e.shutdown();
    e.wait();
}

/// A handler that answers with the path it should go by: the canonical one when it differs from
/// the one sent.
mod echo {
    use std::sync::Arc;

    use akio_http::Context;
    use akit_runtime::{Handler, HandlerParams, Info, Provision, ProvisionError, Provisioned};

    struct Echo;

    impl akio_http::Service for Echo {
        fn call(&self, ctx: &mut Context) {
            let p = ctx.path().to_owned();
            ctx.text(200, p);
        }
    }

    impl Handler for Echo {}

    fn new(_: &Provision<'_, HandlerParams>) -> Result<Provisioned<dyn Handler>, ProvisionError> {
        Ok(Provisioned::new(Arc::new(Echo) as Arc<dyn Handler>))
    }

    pub(crate) static MODULES: &[Info<HandlerParams>] = &[Info::handler(
        "http.handlers.respond",
        "Answers with the canonical path.",
        new,
    )];
}

#[test]
fn canonical_path_for_handlers() {
    let config = akit_config::from_json(
        r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}],
            "routes": [{"handler": {"module": "respond"}}]}"#,
        None,
    )
    .unwrap();
    let registry: Registry<HandlerParams> = Registry::new(&[echo::MODULES]).unwrap();
    let cell = GraphCell::new(compile(&config, &registry, &Env::default()).unwrap());
    let e = akit_listener::engine(&cell, &options()).unwrap();
    e.start().unwrap();
    let a = e.local_addrs()[0];
    for (target, want) in [
        ("/a/b?x=1", "/a/b"),
        ("/a/./b/../c//d?q=/../x", "/a/c/d"),
        ("/a/x/%2e%2e/b%2Fc", "/a/b%2Fc"),
    ] {
        assert_eq!(get(a, "h", target).1, want, "{target}");
    }
    e.shutdown();
    e.wait();
}

/// An HTTP/1.1 request with `head` (its method, target and extra header lines); the status
/// line's code and the body.
fn request(addr: SocketAddr, head: &str) -> (u16, String) {
    let (method, rest) = head.split_once(' ').unwrap();
    let (target, extra) = rest.split_once('\n').unwrap_or((rest, ""));
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let extra: String = extra.lines().map(|l| format!("{l}\r\n")).collect();
    write!(
        s,
        "{method} {target} HTTP/1.1\r\nHost: a.example\r\n{extra}Content-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    let (head, body) = resp.split_once("\r\n\r\n").unwrap();
    let code = head.split(' ').nth(1).unwrap().parse().unwrap();
    (code, body.to_owned())
}

/// I39: routes' method, header, query, remote IP, path regexp and expression matchers, on the
/// requests a listener serves.
#[test]
fn route_matchers() {
    let body =
        |b: &str| format!(r#""handler": {{"module": "respond", "params": {{"body": "{b}"}}}}"#);
    let json = format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}}],
            "routes": [
              {{"match": {{"path": ["/m"], "method": ["POST"]}}, {}}},
              {{"match": {{"path": ["/m"], "header": {{"X-Env": ["prod"]}}}}, {}}},
              {{"match": {{"path": ["/m"], "query": {{"v": ["2"]}}}}, {}}},
              {{"match": {{"path": ["/m"]}}, {}}},
              {{"match": {{"path": ["/ip"], "remote_ip": ["10.0.0.0/8"]}}, {}}},
              {{"match": {{"path": ["/ip"], "remote_ip": ["127.0.0.0/8", "::1"]}}, {}}},
              {{"match": {{"path_regexp": ["^/users/[0-9]+$"]}}, {}}},
              {{"priority": 1, "match": {{"expr": "Host(\"*.example\") && (PathPrefix(\"/api/\") || Method(\"DELETE\")) && !Header(\"X-Skip\")"}}, {}}}
            ]}}"#,
        body("post"),
        body("prod"),
        body("v2"),
        body("plain"),
        body("ten"),
        body("local"),
        body("user"),
        body("expr"),
    );
    let (e, addrs) = serve(&json);
    let a = addrs[0];
    for (head, code, want) in [
        ("POST /m", 200, "post"),
        ("GET /m\nX-Env: prod", 200, "prod"),
        ("GET /m\nX-Env: dev\nX-Env: prod", 200, "prod"),
        ("GET /m\nx-env: Prod", 200, "plain"),
        ("GET /m?v=2", 200, "v2"),
        ("GET /m?v=%32", 200, "v2"),
        ("GET /m?v=3", 200, "plain"),
        ("POST /m?v=2\nX-Env: prod", 200, "post"),
        ("GET /ip", 200, "local"),
        ("GET /users/42", 200, "user"),
        ("GET /users/42/x", 404, "404 page not found\n"),
        ("GET /users/%34%32", 200, "user"),
        ("GET /api/x", 200, "expr"),
        ("DELETE /m", 200, "expr"),
        ("DELETE /m\nX-Skip: 1", 200, "plain"),
        ("GET /api/x\nX-Skip: 1", 404, "404 page not found\n"),
    ] {
        assert_eq!(request(a, head), (code, want.to_owned()), "{head:?}");
    }
    // Over HTTP/2 too: the query and the method.
    assert_eq!(get_h2c(a, "/m?v=2").unwrap(), (200, "v2".to_owned()));
    assert_eq!(get_h2c(a, "/m").unwrap(), (200, "plain".to_owned()));
    e.shutdown();
    e.wait();
}

/// A POST of `size` bytes over HTTP/1.1, framed by its length or chunked; the status, or `None`
/// when the connection was ended before one came (a chunked body past the limit may be).
fn post(addr: SocketAddr, size: usize, chunked: bool) -> Option<u16> {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let framing = if chunked {
        "Transfer-Encoding: chunked".to_owned()
    } else {
        format!("Content-Length: {size}")
    };
    write!(
        s,
        "POST / HTTP/1.1\r\nHost: x\r\n{framing}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let body = vec![b'b'; size];
    // A body past the limit may be refused before it is all sent: stop writing then.
    let _ = if chunked {
        body.chunks(16 << 10)
            .try_for_each(|c| {
                write!(s, "{:x}\r\n", c.len())?;
                s.write_all(c)?;
                s.write_all(b"\r\n")
            })
            .and_then(|()| s.write_all(b"0\r\n\r\n"))
    } else {
        s.write_all(&body)
    };
    let mut resp = Vec::new();
    let _ = s.read_to_end(&mut resp);
    let status = String::from_utf8_lossy(resp.get(9..12)?).parse().ok()?;
    Some(status)
}

/// A POST of `size` bytes over HTTP/2 in the clear; its status.
fn post_h2c(addr: SocketAddr, size: usize) -> Result<u16, String> {
    let client = Engine::builder().client_threads(1).build().unwrap();
    client.start().unwrap();
    let c = H2Client::dial_blocking(&client, addr, None, H2Options::default())
        .map_err(|e| e.to_string())?;
    let r = c
        .send(
            ClientRequest::post(&format!("http://{addr}/"), vec![b'b'; size])
                .timeout(Duration::from_secs(5)),
        )
        .wait()
        .map_err(|e| e.to_string())?;
    Ok(r.status)
}

/// fit's `TestBodyLimit` (listener/limits_test.go): `limits.max_body_bytes` bounds every
/// request body a listener takes; one past it is answered 413, over HTTP/1.1 (framed by its
/// length or chunked) and h2c.
#[test]
fn body_limit() {
    let (e, addrs) = serve(
        r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}],
            "limits": {"max_body_bytes": 100000},
            "routes": [{"handler": {"module": "respond", "params": {"body": "ok"}}}]}"#,
    );
    let a = addrs[0];
    for (size, status) in [(1000, 200), (90_000, 200), (150_000, 413)] {
        assert_eq!(post(a, size, false), Some(status), "{size} bytes");
        let chunked = post(a, size, true);
        assert!(
            chunked == Some(status) || status == 413 && chunked.is_none(),
            "{size} bytes, chunked: {chunked:?}"
        );
        assert_eq!(post_h2c(a, size), Ok(status), "{size} bytes over h2c");
    }
    e.shutdown();
    e.wait();
}

/// Without `limits`, fit's default: 16 MiB (akio's own is 1 MiB).
#[test]
fn body_limit_default() {
    let (e, addrs) = serve(
        r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}],
            "routes": [{"handler": {"module": "respond", "params": {"body": "ok"}}}]}"#,
    );
    let a = addrs[0];
    assert_eq!(post(a, 2 << 20, false), Some(200));
    assert_eq!(post_h2c(a, 2 << 20), Ok(200));
    assert_eq!(post(a, (16 << 20) + 1, false), Some(413));
    e.shutdown();
    e.wait();
}
