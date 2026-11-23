//! fit's header tests for its proxy (`handler/proxy/proxy_test.go` and `framed_test.go`), ported:
//! the request and response forwarded, hop-by-hop fields dropped both ways, the forwarding fields
//! from clients trusted and not, `Host` passed on or rewritten, 502 when the upstream cannot be
//! reached, HEAD, many requests at once, and the params' errors. Over HTTP/1.1 and h2c, to an
//! HTTP/1.1 upstream (akit's only kind so far), on the backend `AKIO_TEST_BACKEND` names (epoll by
//! default, `uring`).
//!
//! The upstreams are plain threads on std sockets that report the request head exactly as it
//! arrived, so that the tests see the fields' order and case.
//!
//! `forwarding`, `request_fields_in_order` and `host_header` check the `Host` the upstream sees
//! and that no `User-Agent` is made up, which needs akio API request A11: akio's client at the
//! pinned commit drops a request's own `Host` and sends an empty `User-Agent` as it is. They are
//! ignored until the pin moves to an akio with A11; run them with `--ignored` against akio's A11
//! branch (a `[patch]` given on the command line, never committed).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::Duration;

use akio::{Backend, Engine, UringConfig};
use akio_http::client::ClientRequest;
use akio_http::h2client::{H2Client, H2Options};
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

fn registry() -> Registry<HandlerParams> {
    Registry::new(&[akit_handler::MODULES, akit_proxy::MODULES]).unwrap()
}

/// fit's `config1`: one listener for HTTP/1.1 and h2c, the upstream `app` at `addr`, and a route
/// to the proxy with `params`.
fn config1(params: &str, addr: &str) -> String {
    format!(
        r#"{{
  "listeners": [{{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}}],
  "upstreams": [{{"name": "app", "addresses": ["{addr}"]}}],
  "routes": [{{"handler": {{"module": "proxy", "params": {params}}}}}]
}}"#
    )
}

/// akit serving `json`, as `akit run` does: the graph with its pools, then the engine, then the
/// pools given it.
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
}

impl Drop for Akit {
    fn drop(&mut self) {
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
    }
}

/// A request as an upstream saw it: its head as it arrived, and its body.
#[derive(Debug)]
struct Seen {
    head: String,
    body: Vec<u8>,
}

impl Seen {
    fn request_line(&self) -> &str {
        self.head.lines().next().unwrap_or("")
    }

    /// The values of the field `name`, in any case, as they arrived.
    fn values(&self, name: &str) -> Vec<&str> {
        field_values(&self.head, name)
    }

    fn one(&self, name: &str) -> Option<&str> {
        self.values(name).first().copied()
    }
}

fn field_values<'a>(head: &'a str, name: &str) -> Vec<&'a str> {
    head.lines()
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .filter(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim())
        .collect()
}

/// An upstream on its own threads: it reads each request on a connection (its body framed by
/// `Content-Length`, as akio's client frames it), sends what it saw on the channel it returns, and
/// answers with `reply(seen)`.
fn upstream(
    reply: impl Fn(&Seen) -> Vec<u8> + Send + Sync + 'static,
) -> (SocketAddr, Receiver<Seen>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let (tx, rx) = channel();
    let reply = Arc::new(reply);
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            let (tx, reply): (Sender<Seen>, _) = (tx.clone(), reply.clone());
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
                    let length = field_values(&head, "content-length")
                        .first()
                        .map_or(0, |v| v.parse::<usize>().unwrap());
                    let mut body = vec![0; length];
                    if r.read_exact(&mut body).is_err() {
                        return;
                    }
                    let seen = Seen { head, body };
                    let out = reply(&seen);
                    let _ = tx.send(seen);
                    if w.write_all(&out).is_err() {
                        return;
                    }
                }
            });
        }
    });
    (addr, rx)
}

/// fit's echo backend: 201, with `X-Backend: echo`, the request's head and body as its body.
fn echo() -> (SocketAddr, Receiver<Seen>) {
    upstream(|seen| {
        let mut body = seen.head.clone().into_bytes();
        body.extend_from_slice(&seen.body);
        let mut out = format!(
            "HTTP/1.1 201 Created\r\nContent-Type: text/plain\r\nX-Backend: echo\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(&body);
        out
    })
}

/// A backend that answers every request with `reply`.
fn fixed(reply: &'static str) -> (SocketAddr, Receiver<Seen>) {
    upstream(move |_| reply.as_bytes().to_vec())
}

/// A response to the client: its status line, its head, and its body.
#[derive(Debug)]
struct Got {
    status: u16,
    head: String,
    body: Vec<u8>,
}

impl Got {
    fn values(&self, name: &str) -> Vec<&str> {
        field_values(&self.head, name)
    }
}

/// Sends `request`, as it is, over HTTP/1.1 and reads the response (framed by its
/// `Content-Length`; `head` says it has no body).
fn h1(addr: SocketAddr, request: &[u8], head: bool) -> Got {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(request).unwrap();
    let mut r = BufReader::new(s);
    let mut text = String::new();
    loop {
        let mut line = String::new();
        assert!(r.read_line(&mut line).unwrap() > 0, "cut off: {text:?}");
        text.push_str(&line);
        if line == "\r\n" {
            break;
        }
    }
    let status = text
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("status line in {text:?}"));
    let length = if head {
        0
    } else {
        field_values(&text, "content-length")
            .first()
            .map_or(0, |v| v.parse().unwrap())
    };
    let mut body = vec![0; length];
    r.read_exact(&mut body).unwrap();
    Got {
        status,
        head: text,
        body,
    }
}

/// A request over h2c (HTTP/2 with prior knowledge), on an engine of its own.
fn h2c(addr: SocketAddr, req: ClientRequest) -> Got {
    let client = Engine::builder().client_threads(1).build().unwrap();
    client.start().unwrap();
    let c = H2Client::dial_blocking(&client, addr, None, H2Options::default()).unwrap();
    let r = c.send(req.timeout(Duration::from_secs(10))).wait().unwrap();
    let head = r
        .headers
        .iter()
        .map(|(n, v)| format!("{n}: {v}\r\n"))
        .collect::<String>();
    client.shutdown();
    Got {
        status: r.status,
        head: format!("HTTP/2 {}\r\n{head}\r\n", r.status),
        body: r.body,
    }
}

fn recv(rx: &Receiver<Seen>) -> Seen {
    rx.recv_timeout(Duration::from_secs(10))
        .expect("the upstream saw no request")
}

/// fit's `TestForwarding`: method, target, `Host`, fields (given twice too), body, and the
/// upstream's status and fields back; over HTTP/1.1 and h2c. No body and no `User-Agent`: none is
/// made up.
#[test]
#[ignore = "needs akio A11 (a client request's own Host and User-Agent): run with --ignored against akio's A11 branch"]
fn forwarding() {
    let (up, rx) = echo();
    let akit = Akit::start(&config1(r#"{"upstream": "app"}"#, &up.to_string()));
    let body = "payload ".repeat(1000);
    let check = |got: &Got, seen: &Seen, host: &str, ua: &str| {
        assert_eq!(got.status, 201, "{got:?}");
        assert_eq!(got.values("x-backend"), ["echo"]);
        assert_eq!(got.values("content-type"), ["text/plain"]);
        assert_eq!(
            seen.request_line(),
            "PUT /a%2Fb/c?x=1&y=%20z HTTP/1.1\r\n".trim_end()
        );
        assert_eq!(seen.one("host"), Some(host));
        assert_eq!(seen.body, body.as_bytes());
        assert_eq!(seen.values("x-custom"), ["kept"]);
        assert_eq!(seen.values("x-multi"), ["1", "2"]);
        assert_eq!(seen.values("user-agent"), [ua]);
        assert_eq!(seen.values("content-length"), [body.len().to_string()]);
        // The upstream's head and body came back.
        assert!(
            got.body
                .starts_with(b"PUT /a%2Fb/c?x=1&y=%20z HTTP/1.1\r\n")
        );
    };

    let req = format!(
        "PUT /a%2Fb/c?x=1&y=%20z HTTP/1.1\r\nHost: app.example.com\r\nUser-Agent: test/1.1\r\n\
         X-Custom: kept\r\nX-Multi: 1\r\nX-Multi: 2\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let got = h1(akit.addr, req.as_bytes(), false);
    check(&got, &recv(&rx), "app.example.com", "test/1.1");

    let req = ClientRequest::new(
        akio_http::Method::Put,
        &format!("http://{}/a%2Fb/c?x=1&y=%20z", akit.addr),
    )
    .header("User-Agent", "test/2")
    .header("X-Custom", "kept")
    .header("X-Multi", "1")
    .header("X-Multi", "2")
    .body(body.clone().into_bytes());
    let got = h2c(akit.addr, req);
    check(&got, &recv(&rx), &akit.addr.to_string(), "test/2");

    let got = h1(akit.addr, b"GET / HTTP/1.1\r\nHost: a\r\n\r\n", false);
    assert_eq!(got.status, 201);
    let seen = recv(&rx);
    assert!(seen.body.is_empty());
    assert!(seen.values("content-length").is_empty(), "{seen:?}");
    assert!(seen.values("user-agent").is_empty(), "{seen:?}");
}

/// fit's `TestHopByHopRequest`.
#[test]
fn hop_by_hop_request() {
    let (up, rx) = echo();
    let akit = Akit::start(&config1(r#"{"upstream": "app"}"#, &up.to_string()));
    h1(
        akit.addr,
        b"GET / HTTP/1.1\r\nHost: a\r\nConnection: X-Hop, keep-alive\r\nX-Hop: 1\r\n\
          Keep-Alive: timeout=5\r\nProxy-Connection: keep-alive\r\nProxy-Authorization: Basic eDp5\r\n\
          Te: trailers\r\nUpgrade: foo\r\nX-End: kept\r\n\r\n",
        false,
    );
    let seen = recv(&rx);
    for name in [
        "X-Hop",
        "Keep-Alive",
        "Proxy-Connection",
        "Proxy-Authorization",
        "Te",
        "Upgrade",
    ] {
        assert!(seen.values(name).is_empty(), "{name} reached the upstream");
    }
    assert_eq!(seen.values("x-end"), ["kept"]);
    // The connection to akit's upstream is its own: kept alive, not the client's.
    assert!(
        seen.values("connection").is_empty() || seen.values("connection") == ["keep-alive"],
        "{seen:?}"
    );
}

/// fit's `TestHopByHopResponse` and `TestFramedResponseConnectionFields`: over HTTP/1.1 and h2c.
#[test]
fn hop_by_hop_response() {
    let (up, _rx) = fixed(
        "HTTP/1.1 200 OK\r\nConnection: X-Hop\r\nX-Hop: 1\r\nKeep-Alive: timeout=5\r\n\
         Proxy-Authenticate: Basic\r\nTrailer: X-T\r\nUpgrade: foo\r\nX-End: kept\r\n\
         Connection: x-a\r\nConnection: X-B, close\r\nx-a: 1\r\nX-b: 2\r\nContent-Length: 2\r\n\r\nok",
    );
    let akit = Akit::start(&config1(r#"{"upstream": "app"}"#, &up.to_string()));
    let h2 = h2c(
        akit.addr,
        ClientRequest::get(&format!("http://{}/", akit.addr)),
    );
    for got in [
        h1(akit.addr, b"GET / HTTP/1.1\r\nHost: a\r\n\r\n", false),
        h2,
    ] {
        assert_eq!((got.status, got.body.as_slice()), (200, &b"ok"[..]));
        assert_eq!(got.values("x-end"), ["kept"], "{got:?}");
        for name in [
            "X-Hop",
            "Keep-Alive",
            "Proxy-Authenticate",
            "Trailer",
            "Upgrade",
            "x-a",
            "x-b",
        ] {
            assert!(got.values(name).is_empty(), "{name} reached the client");
        }
        // Only akit's own framing of the connection, never the upstream's options.
        for v in got.values("connection") {
            assert!(v.eq_ignore_ascii_case("keep-alive"), "{got:?}");
        }
    }
}

/// fit's `TestFramedRequestHeader`: the client's fields go on in their order and case, without
/// the hop-by-hop ones, those `Connection` names, `Content-Length` (akio's client writes its own,
/// last) and the client's forwarding fields, replaced by the proxy's own after them.
#[test]
#[ignore = "needs akio A11 (a client request's own Host and User-Agent): run with --ignored against akio's A11 branch"]
fn request_fields_in_order() {
    let (up, rx) = echo();
    let akit = Akit::start(&config1(r#"{"upstream": "app"}"#, &up.to_string()));
    h1(
        akit.addr,
        b"POST /p?q=1 HTTP/1.1\r\nHost: app.example\r\nzz-first: 1\r\n\
          connection: x-hop, keep-alive\r\nx-hop: 1\r\nkeep-alive: 5\r\nTE: trailers\r\nproxy-authorization: x\r\n\
          x-forwarded-for: 203.0.113.9\r\nForwarded: for=203.0.113.9\r\nX-Forwarded-Host: evil\r\nx-multi: 1\r\n\
          Aa-Last: 2\r\nx-multi: 2\r\ncontent-length: 3\r\n\r\nabc",
        false,
    );
    let seen = recv(&rx);
    assert_eq!(
        seen.head,
        "POST /p?q=1 HTTP/1.1\r\nHost: app.example\r\nzz-first: 1\r\nx-multi: 1\r\n\
         Aa-Last: 2\r\nx-multi: 2\r\nX-Forwarded-For: 127.0.0.1\r\nX-Forwarded-Proto: http\r\n\
         X-Forwarded-Host: app.example\r\nForwarded: for=127.0.0.1;host=app.example;proto=http\r\n\
         Content-Length: 3\r\n\r\n"
    );
    assert_eq!(seen.body, b"abc");
}

/// fit's `TestFramedResponseHeader`: the upstream's fields come back in their order and case,
/// with its `Date`, without the hop-by-hop ones and those `Connection` names, framed afresh.
#[test]
fn response_fields_in_order() {
    let (up, _rx) = fixed(
        "HTTP/1.1 200 OK\r\nzz: 1\r\nDate: Mon, 02 Jan 2006 15:04:05 GMT\r\n\
         Connection: x-hop, keep-alive\r\nx-hop: 1\r\nKeep-Alive: timeout=5\r\nContent-Length: 2\r\nAa: 2\r\n\r\nok",
    );
    let akit = Akit::start(&config1(r#"{"upstream": "app"}"#, &up.to_string()));
    let got = h1(
        akit.addr,
        b"GET / HTTP/1.1\r\nHost: app.example\r\n\r\n",
        false,
    );
    let fields: Vec<&str> = got
        .head
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .filter(|l| {
            let n = l.split(':').next().unwrap_or("");
            !n.eq_ignore_ascii_case("content-length") && !n.eq_ignore_ascii_case("connection")
        })
        .collect();
    assert_eq!(
        fields,
        ["zz: 1", "Date: Mon, 02 Jan 2006 15:04:05 GMT", "Aa: 2"],
        "{got:?}"
    );
    assert_eq!(got.values("content-length"), ["2"]);
    assert_eq!(got.body, b"ok");
}

fn forwarding_fields(seen: &Seen) -> [String; 4] {
    [
        "X-Forwarded-For",
        "X-Forwarded-Proto",
        "X-Forwarded-Host",
        "Forwarded",
    ]
    .map(|n| seen.values(n).join(" | "))
}

/// fit's `TestForwardedUntrusted`.
#[test]
fn forwarded_untrusted() {
    let (up, rx) = echo();
    let akit = Akit::start(&config1(r#"{"upstream": "app"}"#, &up.to_string()));
    h1(
        akit.addr,
        b"GET / HTTP/1.1\r\nHost: site.example\r\nX-Forwarded-For: 203.0.113.9\r\n\
          X-Forwarded-Proto: https\r\nX-Forwarded-Host: evil.example\r\nForwarded: for=203.0.113.9\r\n\r\n",
        false,
    );
    assert_eq!(
        forwarding_fields(&recv(&rx)),
        [
            "127.0.0.1",
            "http",
            "site.example",
            "for=127.0.0.1;host=site.example;proto=http"
        ]
    );
}

/// fit's `TestForwardedTrusted`: a trusted proxy's fields are added to, or kept; one that sends
/// none gets them set.
#[test]
fn forwarded_trusted() {
    let (up, rx) = echo();
    let akit = Akit::start(&config1(
        r#"{"upstream": "app", "trusted_proxies": ["10.0.0.0/8", "127.0.0.1"]}"#,
        &up.to_string(),
    ));
    h1(
        akit.addr,
        b"GET / HTTP/1.1\r\nHost: site.example\r\nX-Forwarded-For: 203.0.113.9\r\n\
          X-Forwarded-Proto: https\r\nX-Forwarded-Host: public.example\r\n\
          Forwarded: for=203.0.113.9;proto=https\r\n\r\n",
        false,
    );
    assert_eq!(
        forwarding_fields(&recv(&rx)),
        [
            "203.0.113.9, 127.0.0.1",
            "https",
            "public.example",
            "for=203.0.113.9;proto=https, for=127.0.0.1;host=site.example;proto=http"
        ]
    );
    h1(
        akit.addr,
        b"GET / HTTP/1.1\r\nHost: site.example\r\n\r\n",
        false,
    );
    assert_eq!(
        forwarding_fields(&recv(&rx)),
        [
            "127.0.0.1",
            "http",
            "site.example",
            "for=127.0.0.1;host=site.example;proto=http"
        ]
    );
}

/// fit's `TestHostHeader` and `TestFramedHostHeader`: `host_header` is sent, the client's in
/// `X-Forwarded-Host`, and the body goes with it.
#[test]
#[ignore = "needs akio A11 (a client request's own Host and User-Agent): run with --ignored against akio's A11 branch"]
fn host_header() {
    let (up, rx) = echo();
    let akit = Akit::start(&config1(
        r#"{"upstream": "app", "host_header": "internal.svc:8080"}"#,
        &up.to_string(),
    ));
    h1(
        akit.addr,
        b"PUT /x HTTP/1.1\r\nHost: site.example\r\nzz: 1\r\nContent-Length: 2\r\n\r\nhi",
        false,
    );
    let seen = recv(&rx);
    assert!(
        seen.head
            .starts_with("PUT /x HTTP/1.1\r\nHost: internal.svc:8080\r\nzz: 1\r\n"),
        "{seen:?}"
    );
    assert_eq!(seen.values("x-forwarded-host"), ["site.example"]);
    assert_eq!(seen.body, b"hi");
}

/// fit's `TestBadGateway`: an upstream that refuses connections is answered 502, over HTTP/1.1
/// and h2c.
#[test]
fn bad_gateway() {
    let addr = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let akit = Akit::start(&config1(r#"{"upstream": "app"}"#, &addr.to_string()));
    let h2 = h2c(
        akit.addr,
        ClientRequest::get(&format!("http://{}/", akit.addr)),
    );
    for got in [
        h1(akit.addr, b"GET / HTTP/1.1\r\nHost: a\r\n\r\n", false),
        h2,
    ] {
        assert_eq!(got.status, 502, "{got:?}");
        assert_eq!(got.body, b"502 Bad Gateway\n");
    }
}

/// fit's `TestManyRequests`: 64 at once, half over HTTP/1.1 and half over h2c.
#[test]
fn many_requests() {
    let (up, _rx) = echo();
    let akit = Akit::start(&config1(r#"{"upstream": "app"}"#, &up.to_string()));
    let addr = akit.addr;
    let threads: Vec<_> = (0..64)
        .map(|i| {
            thread::spawn(move || {
                let got = if i % 2 == 1 {
                    h2c(
                        addr,
                        ClientRequest::post(&format!("http://{addr}/n/{i}"), i.to_string().into()),
                    )
                } else {
                    let body = i.to_string();
                    let req = format!(
                        "POST /n/{i} HTTP/1.1\r\nHost: a\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    h1(addr, req.as_bytes(), false)
                };
                assert_eq!(got.status, 201, "request {i}: {got:?}");
                let text = String::from_utf8(got.body).unwrap();
                assert!(
                    text.starts_with(&format!("POST /n/{i} HTTP/1.1\r\n"))
                        && text.ends_with(&format!("\r\n\r\n{i}")),
                    "request {i}: the upstream saw {text:?}"
                );
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
}

/// fit's `TestHead`: the upstream's `Content-Length` comes back on a response to HEAD, over
/// HTTP/1.1 and h2c.
#[test]
fn head() {
    let (up, _rx) = upstream(|seen| {
        if seen.request_line().starts_with("HEAD ") {
            b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n".to_vec()
        } else {
            b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello world".to_vec()
        }
    });
    let akit = Akit::start(&config1(r#"{"upstream": "app"}"#, &up.to_string()));
    let got = h1(akit.addr, b"HEAD / HTTP/1.1\r\nHost: a\r\n\r\n", true);
    assert_eq!(
        (got.status, got.values("content-length")),
        (200, vec!["11"])
    );
    let got = h2c(
        akit.addr,
        ClientRequest::new(akio_http::Method::Head, &format!("http://{}/", akit.addr)),
    );
    assert_eq!(
        (got.status, got.values("content-length")),
        (200, vec!["11"])
    );
    // And the next request on the same connection is still served.
    let mut s = TcpStream::connect(akit.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(
        b"HEAD / HTTP/1.1\r\nHost: a\r\n\r\nGET / HTTP/1.1\r\nHost: a\r\nConnection: close\r\n\r\n",
    )
    .unwrap();
    let mut all = String::new();
    s.read_to_string(&mut all).unwrap();
    assert_eq!(all.matches("HTTP/1.1 200").count(), 2, "{all:?}");
    assert!(all.ends_with("hello world"), "{all:?}");
}

/// fit's `TestConfigErrors`, through akit-config (the params) and `compile` (the upstream's name).
#[test]
fn config_errors() {
    for (params, want) in [
        (
            r#"{"upstream": "nope"}"#,
            r#"routes[0].handler.params.upstream: there is no upstream named "nope""#,
        ),
        (
            r#"{"upstream": "app", "trusted_proxies": ["10.0.0.0/8", "nope"]}"#,
            r#"routes[0].handler.params.trusted_proxies[1]: "nope" is not an IP address or a network, as in "10.0.0.1" or "10.0.0.0/8""#,
        ),
        (
            r#"{"upstream": "app", "host_header": "a b"}"#,
            r#"routes[0].handler.params.host_header: "a b" is not a host"#,
        ),
        (
            r#"{}"#,
            "routes[0].handler.params.upstream: upstream or upstreams is needed",
        ),
        (
            r#"{"upstream": "app", "timeout": "60s"}"#,
            "routes[0].handler.params.timeout: not supported yet",
        ),
    ] {
        let json = config1(params, "127.0.0.1:1");
        let err = match akit_config::from_json(&json, None) {
            Err(e) => e.to_string(),
            Ok(c) => compile(&c, &registry(), &Env::default())
                .map(|_| ())
                .unwrap_err()
                .to_string(),
        };
        assert!(err.contains(want), "{params}: {err}\nwant {want}");
    }
    // No params at all.
    let json = r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}],
        "routes": [{"handler": {"module": "proxy"}}]}"#;
    let err = akit_config::from_json(json, None).unwrap_err().to_string();
    assert!(
        err.contains("routes[0].handler.params.upstream: upstream or upstreams is needed"),
        "{err}"
    );
}

/// The canonical path the route matched is what goes upstream, not the path sent (fit's
/// listener canonicalizes it before its proxy sends it).
#[test]
fn canonical_path() {
    let (up, rx) = echo();
    let akit = Akit::start(&config1(r#"{"upstream": "app"}"#, &up.to_string()));
    h1(
        akit.addr,
        b"GET /a/./b//c/../d?x=/../y HTTP/1.1\r\nHost: a\r\n\r\n",
        false,
    );
    assert_eq!(recv(&rx).request_line(), "GET /a/b/d?x=/../y HTTP/1.1");
}
