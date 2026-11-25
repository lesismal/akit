//! The `rewrite`, `redirect` and `headers` middleware through akit's listener on akio, with fit's
//! tests (`middleware/rewrite/rewrite_test.go`, `middleware/headers/headers_test.go`) ported, on
//! the backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
//!
//! Each route's handler is a test `echo` (registered as `respond`, whose params it takes): it
//! answers with `respond`'s status and fields, and a body of the path it was given (decoded and
//! escaped, as fit's test echoes `URL.Path` and `URL.EscapedPath()`) and the request's fields as
//! the middleware left them. Others go through the `proxy` handler to a plain upstream, which
//! reports the request as it arrived.
//!
//! The rewrite's `regexp` and `replacement`, and the groups of the redirect's regexp in its `to`,
//! run on akio-regex's matches and captures (akio API request A32).
//! fit's `{scheme}` over TLS (`https`) comes from akit-listener's `TLS` local, tested with the TLS
//! listeners; here it is `http`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::Duration;

use akio::{Backend, Engine, UringConfig};
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, HandlerParams, Info, Registry, compile};

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

/// The test handler.
mod echo {
    use std::sync::Arc;

    use akio_http::{Context, Response};
    use akit_runtime::{Handler, HandlerParams, Info, Provision, ProvisionError, Provisioned};

    struct Echo(akit_config::Respond);

    impl akio_http::Service for Echo {
        fn call(&self, ctx: &mut Context) {
            let escaped = ctx.path().to_owned();
            let decoded = akit_router::canonicalize(&escaped)
                .map(|c| String::from_utf8_lossy(&c.decoded).into_owned())
                .unwrap_or_default();
            let mut body = format!("{decoded} {escaped}\n");
            for (n, v) in ctx.req().headers.iter() {
                body.push_str(&format!("{n}: {v}\n"));
            }
            let mut resp = Response::new(self.0.status);
            for (n, v) in &self.0.headers {
                for v in v.split(',') {
                    resp.headers.append(n, v);
                }
            }
            resp.body = body.into_bytes();
            ctx.respond(resp);
        }
    }

    impl Handler for Echo {}

    fn new(p: &Provision<'_, HandlerParams>) -> Result<Provisioned<dyn Handler>, ProvisionError> {
        let HandlerParams::Respond(r) = p.params() else {
            return Err(ProvisionError::new("a respond handler only"));
        };
        Ok(Provisioned::new(
            Arc::new(Echo(r.clone())) as Arc<dyn Handler>
        ))
    }

    /// `respond`'s status and headers (a value with commas is that many fields) are the
    /// response's; its body is ignored.
    pub(crate) static MODULES: &[Info<HandlerParams>] =
        &[Info::handler("http.handlers.respond", "Echoes.", new)];
}

fn registry(handlers: &'static [Info<HandlerParams>]) -> Registry<HandlerParams> {
    Registry::new(&[handlers, akit_middleware::MODULES, akit_proxy::MODULES]).unwrap()
}

/// akit serving `json`, as `akit run` does.
struct Akit {
    engine: Arc<Engine>,
    addr: SocketAddr,
    _cell: GraphCell,
}

impl Akit {
    fn start(json: &str, handlers: &'static [Info<HandlerParams>]) -> Akit {
        let config = akit_config::from_json(json, None).unwrap_or_else(|e| panic!("{json}: {e}"));
        let slot = EngineSlot::new();
        let cell =
            GraphCell::new(compile(&config, &registry(handlers), &Env::new(slot.clone())).unwrap());
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

    /// A route to the echo, with `middleware` (a JSON array) and the echo's `respond` params.
    fn echo(middleware: &str, respond: &str) -> Akit {
        Akit::start(
            &format!(
                r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
                    "routes": [{{"middleware": {middleware},
                                 "handler": {{"module": "respond", "params": {respond}}}}}]}}"#
            ),
            echo::MODULES,
        )
    }

    /// A route to the proxy for the upstream at `up`, with `middleware`.
    fn proxy(middleware: &str, up: SocketAddr) -> Akit {
        Akit::start(
            &format!(
                r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
                    "upstreams": [{{"name": "app", "addresses": ["{up}"]}}],
                    "routes": [{{"middleware": {middleware},
                                 "handler": {{"module": "proxy", "params": {{"upstream": "app"}}}}}}]}}"#
            ),
            akit_handler::MODULES,
        )
    }
}

impl Drop for Akit {
    fn drop(&mut self) {
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
    }
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
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn text(&self) -> &str {
        std::str::from_utf8(&self.body).unwrap()
    }

    /// The first line of the echo's body: the path it was given, decoded and escaped.
    fn path(&self) -> &str {
        self.text().lines().next().unwrap_or("")
    }

    /// The fields of the request the echo saw (Host and Connection aside), as fit's test renders
    /// them: sorted names, each with its values joined by commas.
    fn request_fields(&self) -> String {
        fields(
            self.text()
                .lines()
                .skip(1)
                .filter_map(|l| l.split_once(": ")),
        )
    }

    /// The response's fields, those akio adds to every response aside.
    fn response_fields(&self) -> String {
        fields(
            self.headers
                .iter()
                .map(|(n, v)| (n.as_str(), v.as_str()))
                .filter(|(n, _)| {
                    !["content-length", "date", "connection"]
                        .iter()
                        .any(|x| n.eq_ignore_ascii_case(x))
                }),
        )
    }
}

fn fields<'a>(it: impl Iterator<Item = (&'a str, &'a str)>) -> String {
    let mut m: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (n, v) in it {
        if n.eq_ignore_ascii_case("host") || n.eq_ignore_ascii_case("connection") {
            continue;
        }
        m.entry(n).or_default().push(v);
    }
    let mut s = String::new();
    for (n, vs) in m {
        let _ = writeln!(s, "{n}: {}", vs.join(","));
    }
    s
}

/// One request on a connection of its own, the response read to the connection's close.
fn request(addr: SocketAddr, method: &str, target: &str, headers: &[(&str, &str)]) -> Resp {
    request_with_body(addr, method, target, headers, b"")
}

fn request_with_body(
    addr: SocketAddr,
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Resp {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut req = format!("{method} {target} HTTP/1.1\r\nConnection: close\r\n");
    if !headers.iter().any(|(n, _)| n.eq_ignore_ascii_case("host")) {
        req.push_str("Host: example.com\r\n");
    }
    for (n, v) in headers {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    if !body.is_empty() {
        req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).unwrap();
    s.write_all(body).unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).unwrap();
    parse(&raw)
}

fn parse(raw: &[u8]) -> Resp {
    let end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("no response head in {:?}", String::from_utf8_lossy(raw)));
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

/// A request head and body as an upstream saw them.
#[derive(Debug)]
struct Seen {
    head: String,
    body: Vec<u8>,
}

impl Seen {
    fn request_line(&self) -> &str {
        self.head.lines().next().unwrap_or("")
    }

    fn values(&self, name: &str) -> Vec<&str> {
        self.head
            .lines()
            .skip(1)
            .filter_map(|l| l.split_once(':'))
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim())
            .collect()
    }
}

/// An upstream on its own threads: it reports each request it reads (the body framed by
/// `Content-Length`) and answers `200 up` with `X-Powered-By: php`.
fn upstream() -> (SocketAddr, Receiver<Seen>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let (tx, rx) = channel();
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            let tx = tx.clone();
            thread::spawn(move || {
                let mut w = s.try_clone().unwrap();
                let mut r = BufReader::new(s);
                loop {
                    let mut head = String::new();
                    loop {
                        let mut line = String::new();
                        match r.read_line(&mut line) {
                            Ok(0) | Err(_) => return,
                            Ok(_) => {}
                        }
                        head.push_str(&line);
                        if line == "\r\n" {
                            break;
                        }
                    }
                    let seen = Seen {
                        head,
                        body: Vec::new(),
                    };
                    let length = seen
                        .values("content-length")
                        .first()
                        .map_or(0, |v| v.parse::<usize>().unwrap());
                    let mut body = vec![0; length];
                    if r.read_exact(&mut body).is_err() {
                        return;
                    }
                    let _ = tx.send(Seen { body, ..seen });
                    let reply = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nX-Powered-By: php\r\nContent-Length: 2\r\n\r\nup";
                    if w.write_all(reply.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
    (addr, rx)
}

/// The errors akit-config gives `json`, each "path: message".
fn config_errors(json: &str) -> String {
    match akit_config::from_json(json, None) {
        Ok(_) => String::new(),
        Err(e) => e.to_string(),
    }
}

/// A config of one route with the middleware `name` and `params`.
fn with_middleware(name: &str, params: &str) -> String {
    format!(
        r#"{{"listeners": [{{"name": "web", "address": ":8080"}}],
            "routes": [{{"middleware": [{{"module": "{name}", "params": {params}}}],
                         "handler": {{"module": "respond"}}}}]}}"#
    )
}

// fit's TestRewrite.
#[test]
fn rewrite() {
    for (name, params, cases) in [
        (
            "strip prefix",
            r#"{"strip_prefix": ["/api/v1", "/api"]}"#,
            &[
                ("/api/users", "/users /users"),
                ("/api/v1/users", "/users /users"),
                ("/api", "/ /"),
                ("/api/", "/ /"),
                ("/apix", "/apix /apix"),
                ("/other/api", "/other/api /other/api"),
                ("/api/a%2Fb", "/a/b /a/b"),
            ][..],
        ),
        (
            "strip a prefix ending in /",
            r#"{"strip_prefix": ["/static/"]}"#,
            &[
                ("/static/css/a.css", "/css/a.css /css/a.css"),
                ("/static", "/ /"),
            ][..],
        ),
        (
            "regexp",
            r#"{"regexp": "^/users/([0-9]+)$", "replacement": "/u?id=$1"}"#,
            &[
                ("/users/42", "/u?id=42 /u%3Fid=42"),
                ("/users/x", "/users/x /users/x"),
            ][..],
        ),
        (
            "regexp, named groups, every match",
            r#"{"regexp": "(?P<word>[a-z]+)-", "replacement": "${word}_"}"#,
            &[("/a-b-c", "/a_b_c /a_b_c")][..],
        ),
        (
            "a replacement that drops the leading /",
            r#"{"regexp": "^/old", "replacement": ""}"#,
            &[("/old", "/ /"), ("/oldx", "/x /x")][..],
        ),
        (
            "regexp on the decoded path",
            r#"{"regexp": "^/a b/(.*)$", "replacement": "/c/$1"}"#,
            &[("/a%20b/x%2Fy", "/c/x/y /c/x/y")][..],
        ),
        (
            "add prefix",
            r#"{"add_prefix": "/v2"}"#,
            &[("/users", "/v2/users /v2/users"), ("/", "/v2/ /v2/")][..],
        ),
        (
            "strip and add, in order",
            r#"{"strip_prefix": ["/api"], "add_prefix": "/internal"}"#,
            &[("/api/v1/users", "/internal/v1/users /internal/v1/users")][..],
        ),
        (
            "all three, in order",
            r#"{"strip_prefix": ["/api"], "regexp": "^/v1/", "replacement": "/", "add_prefix": "/internal"}"#,
            &[("/api/v1/users", "/internal/users /internal/users")][..],
        ),
        (
            "escaped afresh",
            r#"{"strip_prefix": ["/api"]}"#,
            &[
                ("/api/a%20b", "/a b /a%20b"),
                ("/api/a%21b", "/a!b /a%21b"),
                ("/api/a!b", "/a!b /a%21b"),
            ][..],
        ),
    ] {
        let akit = Akit::echo(
            &format!(r#"[{{"module": "rewrite", "params": {params}}}]"#),
            "{}",
        );
        for (target, want) in cases {
            let r = request(akit.addr, "GET", target, &[]);
            assert_eq!(r.status, 200, "{name} {target}");
            assert_eq!(r.path(), *want, "{name} {target}");
        }
    }
}

// A path the regexp rewrites goes upstream too.
#[test]
fn regexp_rewritten_path_goes_upstream() {
    let (up, seen) = upstream();
    let akit = Akit::proxy(
        r#"[{"module": "rewrite", "params": {"regexp": "^/users/(?P<id>[0-9]+)$", "replacement": "/v2/user/${id}"}}]"#,
        up,
    );
    let r = request(akit.addr, "GET", "/users/7?x=1", &[]);
    assert_eq!((r.status, r.text()), (200, "up"));
    let s = seen.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(s.request_line(), "GET /v2/user/7?x=1 HTTP/1.1");
}

// The rewritten path is what the proxy sends upstream, with the query as it came.
#[test]
fn rewritten_path_goes_upstream() {
    let (up, seen) = upstream();
    let akit = Akit::proxy(
        r#"[{"module": "rewrite", "params": {"strip_prefix": ["/api"], "add_prefix": "/v2"}}]"#,
        up,
    );
    let r = request(akit.addr, "GET", "/api/users/7?x=1&y", &[]);
    assert_eq!((r.status, r.text()), (200, "up"));
    let s = seen.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(s.request_line(), "GET /v2/users/7?x=1&y HTTP/1.1");
}

// Middleware run in order: a rewrite then a redirect sees the rewritten path.
#[test]
fn middleware_in_order() {
    let akit = Akit::echo(
        r#"[{"module": "rewrite", "params": {"strip_prefix": ["/old"]}},
            {"module": "redirect", "params": {"to": "/new{path}", "regexp": "^/a"}}]"#,
        "{}",
    );
    let r = request(akit.addr, "GET", "/old/a/b", &[]);
    assert_eq!((r.status, r.header("location")), (302, Some("/new/a/b")));
    let r = request(akit.addr, "GET", "/old/b", &[]);
    assert_eq!((r.status, r.path()), (200, "/b /b"));
}

// fit's TestRedirect.
#[test]
fn redirect() {
    for (name, params, target, status, location) in [
        (
            "every request",
            r#"{"to": "https://{host}{uri}"}"#,
            "/a/b?x=1",
            302,
            "https://example.com/a/b?x=1",
        ),
        (
            "permanent",
            r#"{"to": "https://{host}{uri}", "status": 308}"#,
            "/a",
            308,
            "https://example.com/a",
        ),
        (
            "placeholders",
            r#"{"to": "{scheme}://new.example{path}#{query}"}"#,
            "/a%20b?q",
            302,
            "http://new.example/a%20b#q",
        ),
        (
            "a fixed URL",
            r#"{"to": "/elsewhere", "status": 301}"#,
            "/x",
            301,
            "/elsewhere",
        ),
        (
            "regexp matched",
            r#"{"to": "/new{uri}", "regexp": "^/old/(.*)$"}"#,
            "/old/page?x",
            302,
            "/new/old/page?x",
        ),
        (
            "regexp not matched",
            r#"{"to": "/new", "regexp": "^/old/(.*)$"}"#,
            "/other",
            200,
            "",
        ),
        (
            "a group",
            r#"{"to": "/new/$1{query}", "regexp": "^/old/(.*)$"}"#,
            "/old/page?x",
            302,
            "/new/pagex",
        ),
        (
            "a group, not matched",
            r#"{"to": "/new/$1", "regexp": "^/old/(.*)$"}"#,
            "/other",
            200,
            "",
        ),
        (
            "groups stay escaped",
            r#"{"to": "/new/$1", "regexp": "^/old/(.*)$"}"#,
            "/old/a%20b%2Fc",
            302,
            "/new/a%20b%2Fc",
        ),
        (
            "named groups",
            r#"{"to": "https://{host}/u/${id}?from=$id", "regexp": "^/users/(?P<id>[0-9]+)$"}"#,
            "/users/42",
            302,
            "https://example.com/u/42?from=42",
        ),
        (
            "an unknown group is empty",
            r#"{"to": "/x/$2$name/${1}y", "regexp": "^/(.*)$"}"#,
            "/a",
            302,
            "/x//ay",
        ),
        (
            "regexp matches escaped",
            r#"{"to": "/new", "regexp": "^/a%20b$"}"#,
            "/a%20b",
            302,
            "/new",
        ),
        (
            "$$ is $",
            r#"{"to": "/a$$b/$", "regexp": "^/"}"#,
            "/x",
            302,
            "/a$b/$",
        ),
        // Open redirects: what the client sent makes a URL on another host.
        (
            "path made canonical",
            r#"{"to": "{path}"}"#,
            "//evil.com/x",
            302,
            "/evil.com/x",
        ),
        (
            "query makes //",
            r#"{"to": "/{query}"}"#,
            "/a?/evil.com",
            400,
            "",
        ),
        (
            "query makes /backslash",
            r#"{"to": "/{query}"}"#,
            r"/a?\evil.com",
            400,
            "",
        ),
        (
            "tab between",
            r#"{"to": "/{query}"}"#,
            "/a?%09/evil.com",
            302,
            "/%09/evil.com",
        ),
        // A `\` next to a `/` makes an empty segment, which akit-listener refuses before any
        // route, as fit's listener does; inside a segment it stays, escaped as Go escapes it.
        (
            "raw backslash",
            r#"{"to": "{path}", "regexp": "^/go/"}"#,
            r"/go/\evil.com",
            400,
            "",
        ),
        (
            "raw backslash in a segment",
            r#"{"to": "{path}", "regexp": "^/go/"}"#,
            r"/go/x\y",
            302,
            "/go/x%5Cy",
        ),
        (
            "escaped backslash in a segment",
            r#"{"to": "{path}", "regexp": "^/go/"}"#,
            "/go/x%5Cy",
            302,
            "/go/x%5Cy",
        ),
        // fit's "/go//evil.com" is made canonical by akit-listener, as "path made canonical".
        (
            "group made canonical",
            r#"{"to": "/$1", "regexp": "^/go/(.*)$"}"#,
            "/go//evil.com",
            302,
            "/evil.com",
        ),
        (
            "group makes //",
            r#"{"to": "/$1", "regexp": "^(/[a-z.]+)$"}"#,
            "/evil.com",
            400,
            "",
        ),
        (
            "group makes /backslash",
            r#"{"to": "/$1", "regexp": "^/go/(.*)$"}"#,
            "/go/%5Cevil.com",
            400,
            "",
        ),
        (
            "group makes a scheme",
            r#"{"to": "$1", "regexp": "^/go/(.*)$"}"#,
            "/go/https:/evil.com",
            400,
            "",
        ),
        (
            "group makes a path",
            r#"{"to": "$1", "regexp": "^/go(/.*)$"}"#,
            "/go/a:b",
            302,
            "/a:b",
        ),
        (
            "group makes a relative path",
            r#"{"to": "$1", "regexp": "^/go/(.*)$"}"#,
            "/go/page",
            302,
            "page",
        ),
        (
            "group on another host",
            r#"{"to": "//cdn.example/$1", "regexp": "^/s/(.*)$"}"#,
            "/s/x.js",
            302,
            "//cdn.example/x.js",
        ),
        (
            "to starting with $ makes a scheme",
            r#"{"to": "$$https://{host}", "regexp": "^/"}"#,
            "/x",
            400,
            "",
        ),
        (
            "to on another host",
            r#"{"to": "//cdn.example{path}", "regexp": "^/s/"}"#,
            "/s/x.js",
            302,
            "//cdn.example/s/x.js",
        ),
    ] {
        let akit = Akit::echo(
            &format!(r#"[{{"module": "redirect", "params": {params}}}]"#),
            "{}",
        );
        let r = request(akit.addr, "GET", target, &[]);
        assert_eq!(
            (r.status, r.header("location").unwrap_or("")),
            (status, location),
            "{name}: {}",
            r.text()
        );
        if status == 400 && !matches!(name, "raw backslash" | "group makes /backslash") {
            assert_eq!(r.text(), "400 bad redirect target\n", "{name}");
        }
    }
}

// fit's TestRedirectHeaderIsItsOwn: each response has its own Location.
#[test]
fn redirect_header_is_its_own() {
    let akit = Akit::echo(r#"[{"module": "redirect", "params": {"to": "/x"}}]"#, "{}");
    for _ in 0..3 {
        let r = request(akit.addr, "GET", "/", &[]);
        assert_eq!((r.status, r.header("location")), (302, Some("/x")));
        assert_eq!(r.header("content-length"), Some("0"));
    }
}

// fit's TestHeaders: the request's and the response's fields, twice (values set are not shared
// between requests).
#[test]
fn headers() {
    for (name, params, sent, written, want_request, want_response) in [
        (
            "set replaces, add appends, delete removes",
            r#"{"request": {"set": {"x-a": "1"}, "add": {"X-B": ["2", "3"]}, "delete": ["X-C"]}}"#,
            &[
                ("X-A", "old"),
                ("X-A", "older"),
                ("X-B", "1"),
                ("X-C", "gone"),
                ("X-D", "kept"),
            ][..],
            r#"{"X-A": "old,older", "X-B": "1", "X-C": "gone", "X-D": "kept"}"#,
            "X-A: 1\nX-B: 1,2,3\nX-D: kept\n",
            "X-A: old,older\nX-B: 1\nX-C: gone\nX-D: kept\n",
        ),
        (
            "the response",
            r#"{"response": {"set": {"Server": "fit", "Cache-Control": "no-store"}, "add": {"Vary": ["Accept"]}, "delete": ["X-Powered-By"]}}"#,
            &[("X-Powered-By", "php"), ("Vary", "Origin")][..],
            r#"{"X-Powered-By": "php", "Vary": "Origin"}"#,
            "Vary: Origin\nX-Powered-By: php\n",
            "Cache-Control: no-store\nServer: fit\nVary: Origin,Accept\n",
        ),
        (
            "deletion by prefix, then set",
            r#"{"response": {"delete": ["x-debug-*"], "set": {"X-Debug-Kept": "yes"}}}"#,
            &[("X-Debug-A", "1"), ("X-Debug-B", "2"), ("X-Debugger", "3")][..],
            r#"{"X-Debug-A": "1", "X-Debug-B": "2", "X-Debugger": "3"}"#,
            "X-Debug-A: 1\nX-Debug-B: 2\nX-Debugger: 3\n",
            "X-Debug-Kept: yes\nX-Debugger: 3\n",
        ),
        (
            "delete everything",
            r#"{"response": {"delete": ["*"]}}"#,
            &[("X-A", "1"), ("Content-Type", "text/plain")][..],
            r#"{"X-A": "1", "Content-Type": "text/plain"}"#,
            "Content-Type: text/plain\nX-A: 1\n",
            "",
        ),
    ] {
        let akit = Akit::echo(
            &format!(r#"[{{"module": "headers", "params": {params}}}]"#),
            &format!(r#"{{"headers": {written}}}"#),
        );
        for _ in 0..2 {
            let r = request(akit.addr, "GET", "/", sent);
            assert_eq!(r.status, 200, "{name}");
            assert_eq!(r.request_fields(), want_request, "{name}: request");
            assert_eq!(r.response_fields(), want_response, "{name}: response");
        }
    }
}

// fit's TestResponseOfAnyKind: responses answered at once, with any status, and later (a proxied
// one, from a completion callback) all get the response's changes.
#[test]
fn response_of_any_kind() {
    let mw = r#"[{"module": "headers", "params": {"response": {"set": {"X-Fit": "1"}, "delete": ["X-Powered-By"]}}}]"#;
    let akit = Akit::echo(mw, r#"{"status": 404}"#);
    let r = request(akit.addr, "GET", "/", &[]);
    assert_eq!((r.status, r.header("x-fit")), (404, Some("1")));
    let (up, _seen) = upstream();
    let akit = Akit::proxy(mw, up);
    let r = request(akit.addr, "GET", "/", &[]);
    assert_eq!((r.status, r.text()), (200, "up"));
    assert_eq!(r.header("x-fit"), Some("1"));
    assert_eq!(r.header("x-powered-by"), None);
    // The 404 akit answers when no route matches has none: middleware belong to their route.
    let akit = Akit::start(
        r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}],
            "routes": [{"match": {"path": ["/a"]}, "middleware": [{"module": "headers", "params": {"response": {"set": {"X-Fit": "1"}}}}],
                        "handler": {"module": "respond"}}]}"#,
        echo::MODULES,
    );
    let r = request(akit.addr, "GET", "/b", &[]);
    assert_eq!((r.status, r.header("x-fit")), (404, None));
}

// The request's changed fields are the ones the proxy sends upstream, with a large body sent
// whole, behind a rewrite.
#[test]
fn request_fields_go_upstream() {
    let (up, seen) = upstream();
    let akit = Akit::proxy(
        r#"[{"module": "headers", "params": {"request": {"set": {"X-Env": "prod"}, "add": {"X-Tag": ["a", "b"]}, "delete": ["X-Secret", "X-Debug-*"]}}},
            {"module": "rewrite", "params": {"strip_prefix": ["/api"]}}]"#,
        up,
    );
    let body: Vec<u8> = (0..1 << 20).map(|i: u32| (i % 251) as u8).collect();
    let r = request_with_body(
        akit.addr,
        "POST",
        "/api/upload",
        &[
            ("X-Env", "dev"),
            ("X-Secret", "s"),
            ("X-Debug-Id", "1"),
            ("X-Tag", "z"),
            ("X-Kept", "k"),
        ],
        &body,
    );
    assert_eq!((r.status, r.text()), (200, "up"));
    let s = seen.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(s.request_line(), "POST /upload HTTP/1.1");
    assert_eq!(s.values("x-env"), ["prod"]);
    assert_eq!(s.values("x-tag"), ["z", "a", "b"]);
    assert_eq!(s.values("x-kept"), ["k"]);
    assert!(s.values("x-secret").is_empty() && s.values("x-debug-id").is_empty());
    assert_eq!(s.body.len(), body.len());
    assert!(s.body == body);
}

// A client that leaves before its answer, behind every middleware, leaves akit serving.
#[test]
fn client_gone_midway() {
    let (up, _seen) = upstream();
    let akit = Akit::proxy(
        r#"[{"module": "headers", "params": {"request": {"set": {"X-A": "1"}}, "response": {"set": {"X-B": "2"}}}},
            {"module": "rewrite", "params": {"add_prefix": "/v2"}}]"#,
        up,
    );
    for _ in 0..20 {
        let mut s = TcpStream::connect(akit.addr).unwrap();
        s.write_all(b"POST /x HTTP/1.1\r\nHost: a\r\nContent-Length: 100\r\n\r\nhalf")
            .unwrap();
        drop(s);
    }
    let r = request(akit.addr, "GET", "/x", &[]);
    assert_eq!((r.status, r.header("x-b")), (200, Some("2")));
}

// fit's TestParamErrors, of all three, and what akit does not support yet.
#[test]
fn param_errors() {
    for (name, params, want) in [
        (
            "rewrite",
            "{}",
            "routes[0].middleware[0].params: strip_prefix, regexp or add_prefix is needed",
        ),
        (
            "rewrite",
            r#"{"strip_prefix": ["api"]}"#,
            r#"routes[0].middleware[0].params.strip_prefix: "api" does not start with '/'"#,
        ),
        (
            "rewrite",
            r#"{"add_prefix": "/v2/"}"#,
            r#""/v2/" does not start with '/', or ends with one"#,
        ),
        (
            "rewrite",
            r#"{"regexp": "("}"#,
            r#"routes[0].middleware[0].params.regexp: "(" is not a valid regular expression"#,
        ),
        (
            "rewrite",
            r#"{"replacement": "/x"}"#,
            "routes[0].middleware[0].params.replacement: a replacement needs a regexp",
        ),
        (
            "rewrite",
            r#"{"strip_prefix": ["/a"], "replacement": "/x"}"#,
            "a replacement needs a regexp",
        ),
        (
            "redirect",
            "{}",
            "routes[0].middleware[0].params.to: missing required field",
        ),
        (
            "redirect",
            r#"{"to": "/x", "status": 200}"#,
            "200 is not a redirect status",
        ),
        ("redirect", r#"{"to": "/x\r\nX-Evil: 1"}"#, "line break"),
        (
            "redirect",
            r#"{"to": "/x", "regexp": "["}"#,
            r#"routes[0].middleware[0].params.regexp: "[" is not a valid regular expression"#,
        ),
        ("headers", "{}", "request or response changes are needed"),
        (
            "headers",
            r#"{"request": {}}"#,
            "request or response changes are needed",
        ),
        (
            "headers",
            r#"{"request": {"set": {"Bad Name": "x"}}}"#,
            r#"routes[0].middleware[0].params.request: "Bad Name" is not a valid header name"#,
        ),
        (
            "headers",
            r#"{"response": {"add": {"X-A": ["a\nb"]}}}"#,
            "the value of X-A holds a line break or NUL",
        ),
        (
            "headers",
            r#"{"response": {"delete": ["X:*"]}}"#,
            r#""X:*" is not a valid header name or prefix"#,
        ),
        (
            "headers",
            r#"{"response": {"delete": [""]}}"#,
            r#""" is not a valid header name"#,
        ),
        (
            "headers",
            r#"{"request": {"set": {"X-A": "a\u0000b"}}}"#,
            "holds a line break or NUL",
        ),
        ("headers", r#"{"request": {"sett": {}}}"#, "unknown field"),
    ] {
        let errors = config_errors(&with_middleware(name, params));
        assert!(
            errors.contains(want),
            "{name} {params}: {errors:?}; want {want:?}"
        );
        assert!(
            !errors.contains("not supported yet"),
            "{name} {params}: {errors}"
        );
    }
    // A middleware the config model does not know is the registry's to find (I70), as fit's
    // `module.Load` finds it, with fit's message.
    let config = akit_config::from_json(
        r#"{"listeners": [{"name": "web", "address": ":8080"}],
            "routes": [{"middleware": [{"module": "nope"}], "handler": {"module": "respond"}}]}"#,
        None,
    )
    .unwrap();
    let Err(errors) = compile(
        &config,
        &registry(echo::MODULES),
        &Env::new(EngineSlot::new()),
    ) else {
        panic!("a middleware the registry does not have compiled");
    };
    let errors = errors.to_string();
    assert!(
        errors.contains(
            r#"routes[0].middleware[0].module: there is no module "nope" in http.middleware (it has "basic_auth","#
        ),
        "{errors}"
    );
}

// What fit has and akit does not yet: refused as not supported yet, naming the field.
#[test]
fn not_supported_yet() {
    for (name, params, want) in [
        // Every middleware of fit's is read (I71); Proxy-Wasm's params of `wasm` are I72's.
        (
            "wasm",
            r#"{"path": "p.wasm", "vm_config": "v"}"#,
            "routes[0].middleware[0].params.vm_config: not supported yet",
        ),
        (
            "wasm",
            r#"{"path": "p.wasm", "properties": []}"#,
            "routes[0].middleware[0].params.properties: not supported yet",
        ),
    ] {
        let config = with_middleware(name, params);
        let e = akit_config::from_json(&config, None).unwrap_err();
        assert!(e.only_not_supported_yet(), "{name} {params}: {e}");
        assert!(
            e.to_string().contains(want),
            "{name} {params}: {e}; want {want:?}"
        );
    }
    // Without a regexp, `$1` is text.
    akit_config::from_json(&with_middleware("redirect", r#"{"to": "/a$1"}"#), None).unwrap();
}
