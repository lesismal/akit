//! The `basic_auth` and `client_cert` middleware through akit's listener on akio, with fit's tests
//! (`middleware/auth/auth_test.go`) ported, on the backend `AKIO_TEST_BACKEND` names (epoll by
//! default, `uring`): passwords checked and remembered, refusals, the fields a client may not
//! forge in any spelling, `Authorization` kept or removed, a check that does not hold up the I/O
//! thread, a body sent with a password checked for the first time, client certificates over
//! HTTP/1.1, HTTP/2 and HTTP/3, and the params akit refuses.
//!
//! The route's handler is a test one, registered as `respond`: it answers with the fields the
//! middleware left (`X-User` and `X-Subject` as an application behind CGI would read them, every
//! spelling folded together) and the length of the body it got.
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
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, UringConfig};
use akio_crypto::bcrypt;
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, HandlerParams, Info, Registry, compile};
use support::certs::{Ca, dir};

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

/// The test handler.
mod echo {
    use std::sync::Arc;

    use akio_http::{Context, Response};
    use akit_runtime::{Handler, HandlerParams, Info, Provision, ProvisionError, Provisioned};

    /// What an application behind CGI reads as field `name`: the values of every field whose
    /// name folds to it, sorted, as the order it would see them in is not known.
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

    struct Echo;

    impl akio_http::Service for Echo {
        fn call(&self, ctx: &mut Context) {
            let body = {
                let h = &ctx.req().headers;
                format!(
                    "x-user={} x-subject={} authorization={} body={}",
                    as_cgi(h, "X-User"),
                    as_cgi(h, "X-Subject"),
                    h.get("authorization").unwrap_or(""),
                    ctx.body().len()
                )
            };
            ctx.respond(Response::text(200, body));
        }
    }

    impl Handler for Echo {}

    fn new(_: &Provision<'_, HandlerParams>) -> Result<Provisioned<dyn Handler>, ProvisionError> {
        Ok(Provisioned::new(Arc::new(Echo) as Arc<dyn Handler>))
    }

    pub(crate) static MODULES: &[Info<HandlerParams>] =
        &[Info::handler("http.handlers.respond", "Echoes.", new)];
}

/// akit serving `json`.
struct Akit {
    engine: Arc<Engine>,
    addrs: Vec<SocketAddr>,
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
        let addrs = engine.local_addrs();
        Akit {
            engine,
            addrs,
            _cell: cell,
        }
    }

    /// The echo handler behind `middleware`, on `/auth`; and with none on `/open`.
    fn echo(middleware: &str) -> Akit {
        Akit::start(
            &format!(
                r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
                    "routes": [
                      {{"match": {{"path": ["/open"]}}, "handler": {{"module": "respond"}}}},
                      {{"middleware": {middleware}, "handler": {{"module": "respond"}}}}]}}"#
            ),
            echo::MODULES,
        )
    }

    fn addr(&self) -> SocketAddr {
        self.addrs[0]
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

fn read_response(r: &mut impl BufRead) -> Resp {
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let status = line
        .split(' ')
        .nth(1)
        .unwrap_or_else(|| panic!("no status line: {line:?}"))
        .parse()
        .unwrap();
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let (n, v) = line.split_once(':').unwrap();
        headers.push((n.to_owned(), v.trim().to_owned()));
    }
    let mut resp = Resp {
        status,
        headers,
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

/// One request on a connection of its own, with `headers` and a body of `body` bytes.
fn exchange(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: usize,
) -> Resp {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: example.com\r\n");
    for (n, v) in headers {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    if body > 0 {
        req.push_str(&format!("Content-Length: {body}\r\n"));
    }
    req.push_str("\r\n");
    let mut w = s.try_clone().unwrap();
    w.write_all(req.as_bytes()).unwrap();
    w.write_all(&vec![b'x'; body]).unwrap();
    read_response(&mut BufReader::new(s))
}

fn basic(user: &str, password: &str) -> String {
    format!(
        "Basic {}",
        akio_crypto::base64::STANDARD.encode(format!("{user}:{password}"))
    )
}

/// A request for `/auth` with `user`'s password, when `user` is not empty, and `extra` fields.
fn auth_get(addr: SocketAddr, user: &str, password: &str, extra: &[(&str, &str)]) -> Resp {
    let auth = basic(user, password);
    let mut h: Vec<(&str, &str)> = extra.to_vec();
    if !user.is_empty() {
        h.push(("Authorization", &auth));
    }
    exchange(addr, "GET", "/auth", &h, 0)
}

/// The bcrypt hash of `password` at the lowest cost, which keeps the tests quick.
fn hash(password: &str) -> String {
    bcrypt::hash(password.as_bytes(), bcrypt::MIN_COST).unwrap()
}

/// fit's `spellings`: the fields a client might send to pass for `name` with an application that
/// folds `-` and `_`.
fn spellings(name: &str) -> Vec<String> {
    let under = name.replace('-', "_");
    vec![
        under.clone(),
        under.to_ascii_lowercase(),
        under.to_ascii_uppercase(),
        name.replacen('-', "_", 1),
    ]
}

// fit's TestBasic.
#[test]
fn basic_auth() {
    let akit = Akit::echo(&format!(
        r#"[{{"module": "basic_auth", "params": {{"realm": "staff", "user_header": "x-user",
            "users": [{{"username": "alice", "password_hash": "{}"}},
                      {{"username": "bob", "password_hash": "{}"}}]}}}}]"#,
        hash("wonderland"),
        hash("builder")
    ));
    let a = akit.addr();
    let refused = |user: &str, password: &str| {
        let r = auth_get(a, user, password, &[]);
        assert_eq!(r.status, 401, "{user:?} {password:?}: {}", r.body);
        assert_eq!(
            r.header("www-authenticate"),
            Some(r#"Basic realm="staff", charset="UTF-8""#)
        );
        assert_eq!(r.body, "Unauthorized\n");
    };
    let admitted = |user: &str, password: &str| {
        let r = auth_get(a, user, password, &[]);
        assert_eq!(
            (r.status, r.body.as_str()),
            (
                200,
                format!("x-user={user} x-subject= authorization= body=0").as_str()
            ),
            "{user:?} {password:?}"
        );
    };
    refused("", "");
    refused("alice", "");
    refused("alice", "Wonderland");
    refused("carol", "wonderland"); // unknown, though alice's password
    refused("bob", "wonderland"); // another user's password
    admitted("alice", "wonderland");
    admitted("alice", "wonderland"); // remembered: answered at once
    refused("alice", "wonderland!"); // another password is checked anew
    admitted("bob", "builder");
    admitted("alice", "wonderland"); // each user remembers their own
    // Another scheme, or a malformed one.
    let r = exchange(a, "GET", "/auth", &[("Authorization", "Bearer abc")], 0);
    assert_eq!(r.status, 401);
    // Unprotected routes are not asked.
    assert_eq!(exchange(a, "GET", "/open", &[], 0).status, 200);

    // A client may not say who it is in the user's field...
    assert_eq!(auth_get(a, "", "", &[("X-User", "alice")]).status, 401);
    let r = auth_get(a, "bob", "builder", &[("X-User", "alice")]);
    assert!(
        r.body.contains("x-user=bob "),
        "the client's own field kept: {}",
        r.body
    );
    // ...nor in a spelling an application behind CGI reads as the same field.
    for name in spellings("X-User") {
        let r = auth_get(a, "bob", "builder", &[(&name, "alice")]);
        assert!(r.body.contains("x-user=bob "), "{name}: {}", r.body);
    }
}

// fit's TestBasicForwardAuthorization.
#[test]
fn basic_auth_forward_authorization() {
    let akit = Akit::echo(&format!(
        r#"[{{"module": "basic_auth", "params": {{"forward_authorization": true,
            "users": [{{"username": "alice", "password_hash": "{}"}}]}}}}]"#,
        hash("pw")
    ));
    for _ in 0..2 {
        // Checked, then remembered.
        let r = auth_get(akit.addr(), "alice", "pw", &[]);
        assert!(
            r.body.ends_with(" authorization=Basic YWxpY2U6cHc= body=0"),
            "{} {}",
            r.status,
            r.body
        );
    }
    let r = auth_get(akit.addr(), "", "", &[]);
    assert_eq!(
        r.header("www-authenticate"),
        Some(r#"Basic realm="akit", charset="UTF-8""#),
        "the realm by default"
    );
}

// A slow check runs off the I/O thread: on one HTTP/2 connection, whose streams are all served by
// one I/O thread, requests that need no check are answered while it runs; the request goes on
// once it passes, and the password is remembered. The cost is the lowest from 8 whose hash takes
// 200 ms here (8 in a debug build, about a second; more in a release build, where cost 8 took
// less than the 20 ms the other requests are sent after, and the check passed before them).
#[test]
fn basic_auth_checks_off_the_io_thread() {
    use akio_http::client::ClientRequest;
    use akio_http::h2client::{H2Client, H2Options};
    use std::sync::mpsc;

    let mut cost = 8;
    let slow = loop {
        let t = Instant::now();
        let h = bcrypt::hash(b"slow", cost).unwrap();
        if t.elapsed() >= Duration::from_millis(200) || cost >= 14 {
            break h;
        }
        cost += 1;
    };
    let akit = Akit::start(
        &format!(
            r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}}],
                "routes": [
                  {{"match": {{"path": ["/open"]}}, "handler": {{"module": "respond"}}}},
                  {{"middleware": [{{"module": "basic_auth", "params": {{
                      "users": [{{"username": "alice", "password_hash": "{slow}"}}]}}}}],
                    "handler": {{"module": "respond"}}}}]}}"#
        ),
        echo::MODULES,
    );
    let a = akit.addr();
    let client = Engine::builder()
        .client_threads(1)
        .config({
            let mut v = akio::Config::default();
            v.shutdown_timeout = Duration::from_millis(200);
            v
        })
        .build()
        .unwrap();
    client.start().unwrap();
    let c = H2Client::dial_blocking(&client, a, None, H2Options::default()).unwrap();
    let (tx, rx) = mpsc::channel();
    let auth = basic("alice", "slow");
    let send = |path: &str, tx: mpsc::Sender<(String, u16)>, with_auth: bool| {
        let mut req = ClientRequest::get(&format!("http://{a}{path}"));
        if with_auth {
            req = req.header("authorization", &auth);
        }
        let path = path.to_owned();
        c.request(req, move |r| {
            let _ = tx.send((path, r.map_or(0, |r| r.status)));
        });
    };
    let started = Instant::now();
    send("/auth", tx.clone(), true);
    thread::sleep(Duration::from_millis(20));
    for _ in 0..5 {
        send("/open", tx.clone(), false);
    }
    let order: Vec<(String, u16)> = (0..6)
        .map(|_| rx.recv_timeout(Duration::from_secs(20)).unwrap())
        .collect();
    let check_took = started.elapsed();
    assert_eq!(
        order.last().unwrap(),
        &("/auth".to_owned(), 200),
        "{order:?}"
    );
    assert!(
        order[..5].iter().all(|(p, s)| p == "/open" && *s == 200),
        "{order:?}"
    );
    // Remembered now: let in without a check.
    let t = Instant::now();
    send("/auth", tx, true);
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(20)).unwrap(),
        ("/auth".to_owned(), 200)
    );
    assert!(
        t.elapsed() * 2 < check_took,
        "checked again: {:?} of {check_took:?}",
        t.elapsed()
    );
    drop(c);
    client.shutdown();
    client.wait();
}

/// An upstream on its own threads that answers each request with the length of its body.
fn counting_upstream() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            thread::spawn(move || {
                let mut w = s.try_clone().unwrap();
                let mut r = BufReader::new(s);
                loop {
                    let mut length = 0usize;
                    loop {
                        let mut line = String::new();
                        if r.read_line(&mut line).unwrap_or(0) == 0 {
                            return;
                        }
                        let line = line.trim_end();
                        if line.is_empty() {
                            break;
                        }
                        if let Some((n, v)) = line.split_once(':') {
                            if n.eq_ignore_ascii_case("content-length") {
                                length = v.trim().parse().unwrap();
                            }
                        }
                    }
                    let mut body = vec![0; length];
                    if r.read_exact(&mut body).is_err() {
                        return;
                    }
                    let n = length.to_string();
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{n}",
                        n.len()
                    );
                    if w.write_all(reply.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
    addr
}

// A body sent with a password checked for the first time goes with the request, through the
// proxy (whose large bodies stream): taken whole for the check, then streamed once remembered.
#[test]
fn basic_auth_with_a_body_through_the_proxy() {
    let up = counting_upstream();
    let akit = Akit::start(
        &format!(
            r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
                "upstreams": [{{"name": "app", "addresses": ["{up}"]}}],
                "routes": [{{"middleware": [{{"module": "basic_auth", "params": {{
                    "users": [{{"username": "alice", "password_hash": "{}"}}]}}}}],
                  "handler": {{"module": "proxy", "params": {{"upstream": "app"}}}}}}]}}"#,
            hash("pw")
        ),
        akit_handler::MODULES,
    );
    let auth = basic("alice", "pw");
    for size in [10, 300_000, 300_000, 5] {
        let r = exchange(
            akit.addr(),
            "POST",
            "/up",
            &[("Authorization", &auth)],
            size,
        );
        assert_eq!(
            (r.status, r.body.as_str()),
            (200, size.to_string().as_str())
        );
    }
    let wrong = basic("alice", "nope");
    let r = exchange(
        akit.addr(),
        "POST",
        "/up",
        &[("Authorization", &wrong)],
        300_000,
    );
    assert_eq!(r.status, 401);
}

// fit's TestBasicParams.
#[test]
fn basic_auth_params() {
    let good = hash("pw");
    for (name, params, want) in [
        (
            "basic_auth",
            r#"{"users": []}"#.to_owned(),
            "at least one user",
        ),
        (
            "basic_auth",
            format!(r#"{{"users": [{{"username": "a:b", "password_hash": "{good}"}}]}}"#),
            "not a user name",
        ),
        (
            "basic_auth",
            format!(r#"{{"users": [{{"username": "", "password_hash": "{good}"}}]}}"#),
            "not a user name",
        ),
        (
            "basic_auth",
            format!(
                r#"{{"users": [{{"username": "a", "password_hash": "{good}"}}, {{"username": "a", "password_hash": "{good}"}}]}}"#
            ),
            "listed twice",
        ),
        (
            "basic_auth",
            r#"{"users": [{"username": "a", "password_hash": "pw"}]}"#.to_owned(),
            "not a bcrypt hash",
        ),
        (
            "basic_auth",
            format!(
                r#"{{"realm": "say \"hi\"", "users": [{{"username": "a", "password_hash": "{good}"}}]}}"#
            ),
            "quote",
        ),
        (
            "basic_auth",
            format!(
                r#"{{"user_header": "x user", "users": [{{"username": "a", "password_hash": "{good}"}}]}}"#
            ),
            "not a field name",
        ),
        ("basic_auth", "{}".to_owned(), "users"),
        (
            "basic_auth",
            format!(
                r#"{{"forward_authorization": 1, "users": [{{"username": "a", "password_hash": "{good}"}}]}}"#
            ),
            "expected a boolean",
        ),
        (
            "basic_auth",
            format!(r#"{{"users": [{{"username": "a", "password": "{good}"}}]}}"#),
            "unknown field",
        ),
        (
            "client_cert",
            r#"{"subject_header": "a b"}"#.to_owned(),
            "not a field name",
        ),
        (
            "client_cert",
            r#"{"optional": "yes"}"#.to_owned(),
            "expected a boolean",
        ),
    ] {
        let json = format!(
            r#"{{"listeners": [{{"name": "web", "address": ":8080"}}],
                "routes": [{{"middleware": [{{"module": "{name}", "params": {params}}}],
                             "handler": {{"module": "respond"}}}}]}}"#
        );
        let errs = match akit_config::from_json(&json, None) {
            Ok(_) => String::new(),
            Err(e) => e.to_string(),
        };
        assert!(
            errs.contains(want),
            "{name} {params}: {errs:?}, want {want:?}"
        );
    }
    // Without params: basic_auth needs users; client_cert needs nothing.
    let json = r#"{"listeners": [{"name": "web", "address": ":8080"}],
        "routes": [{"middleware": [{"module": "basic_auth"}], "handler": {"module": "respond"}}]}"#;
    let errs = akit_config::from_json(json, None).unwrap_err().to_string();
    assert!(
        errs.contains("params.users: missing required field"),
        "{errs}"
    );
    let json = r#"{"listeners": [{"name": "web", "address": ":8080"}],
        "routes": [{"middleware": [{"module": "client_cert"}], "handler": {"module": "respond"}}]}"#;
    let c = akit_config::from_json(json, None).unwrap();
    assert_eq!(
        c.routes[0].middleware[0],
        akit_config::Handler::ClientCert(akit_config::ClientCert::default())
    );
    // Hashes htpasswd and Go make are taken, $2y$ and $2a$.
    for h in [
        "$2y$05$skiZk0SnUJNo2I6tAYj.FOJd7NKP44JQaF889.YEyBr7g/uf47whC",
        "$2a$04$5xt5UjNNUVWTqOgf9FDZm.bfWVyXg.U2as/rOFV.Ny5IQstc/jC26",
    ] {
        let json = format!(
            r#"{{"listeners": [{{"name": "web", "address": ":8080"}}],
                "routes": [{{"middleware": [{{"module": "basic_auth", "params": {{"users": [{{"username": "u", "password_hash": "{h}"}}]}}}}],
                             "handler": {{"module": "respond"}}}}]}}"#
        );
        let c = akit_config::from_json(&json, None).unwrap();
        let akit_config::Handler::BasicAuth(b) = &c.routes[0].middleware[0] else {
            panic!()
        };
        assert_eq!((b.realm.as_str(), b.forward_authorization), ("akit", false));
    }
}

// ---- client_cert ----

/// A CA, a server certificate for a.example, a client certificate (CN=client.example) and one
/// from another CA.
struct Pki {
    ca: PathBuf,
    server: (PathBuf, PathBuf),
    client: (PathBuf, PathBuf),
}

fn pki(name: &str) -> Pki {
    let d = dir(name);
    let ca = Ca::new("akit test ca");
    Pki {
        ca: ca.write(&d, "ca.pem"),
        server: ca.leaf(&d, "server", &["a.example"]),
        client: ca.leaf(&d, "client", &["client.example"]),
    }
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

/// akit with a TLS listener (h1, h2, h3) asking for client certificates if given, and a plain
/// one, serving `/cert` behind `client_cert` with `subject_header`, `/opt` behind an optional
/// one, and `/both` behind `basic_auth` and `client_cert`.
fn mtls(p: &Pki, password_hash: &str) -> Akit {
    Akit::start(
        &format!(
            r#"{{"listeners": [
                {{"name": "secure", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2", "h3"],
                  "tls": {{"certificates": [{{"cert_file": "{}", "key_file": "{}"}}],
                           "client_auth": {{"ca_files": ["{}"], "mode": "verify_if_given"}}}}}},
                {{"name": "plain", "address": "127.0.0.1:0"}}],
              "routes": [
                {{"match": {{"path": ["/cert"]}}, "middleware": [{{"module": "client_cert", "params": {{"subject_header": "x-subject"}}}}],
                  "handler": {{"module": "respond"}}}},
                {{"match": {{"path": ["/opt"]}}, "middleware": [{{"module": "client_cert", "params": {{"subject_header": "x-subject", "optional": true}}}}],
                  "handler": {{"module": "respond"}}}},
                {{"match": {{"path": ["/both"]}}, "middleware": [
                    {{"module": "basic_auth", "params": {{"user_header": "x-user", "users": [{{"username": "bob", "password_hash": "{password_hash}"}}]}}}},
                    {{"module": "client_cert", "params": {{"subject_header": "x-subject"}}}}],
                  "handler": {{"module": "respond"}}}}]}}"#,
            s(&p.server.0),
            s(&p.server.1),
            s(&p.ca)
        ),
        echo::MODULES,
    )
}

/// A GET over TLS with `headers`, with the client certificate when `cert`: the status and body.
fn tls_get(
    addr: SocketAddr,
    p: &Pki,
    cert: bool,
    path: &str,
    headers: &[(&str, &str)],
) -> (u16, String) {
    let cfg = support::https::client(
        &p.ca,
        &["http/1.1"],
        cert.then_some((p.client.0.as_path(), p.client.1.as_path())),
    );
    let tcp = TcpStream::connect(addr).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut st = akio::tls::stream::Stream::client(&cfg, "a.example", tcp).unwrap();
    let mut req = format!("GET {path} HTTP/1.1\r\nHost: a.example\r\nConnection: close\r\n");
    for (n, v) in headers {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    req.push_str("\r\n");
    st.write_all(req.as_bytes()).unwrap();
    st.flush().unwrap();
    let mut all = Vec::new();
    let _ = st.read_to_end(&mut all);
    let r = read_response(&mut BufReader::new(&all[..]));
    (r.status, r.body)
}

// fit's TestClientCert, and TestAuthenticated's cases of it.
#[test]
fn client_cert() {
    let p = pki("middleware-client-cert");
    let akit = mtls(&p, &hash("pw"));
    let (tls, plain) = (akit.addrs[0], akit.addrs[2]);
    let subject = "CN=client.example";
    let forged: Vec<String> = std::iter::once("X-Subject".to_owned())
        .chain(spellings("X-Subject"))
        .collect();
    let forged: Vec<(&str, &str)> = forged.iter().map(|n| (n.as_str(), "CN=forged")).collect();
    for (name, cert, path, want) in [
        (
            "verified",
            true,
            "/cert",
            (
                200,
                format!("x-user= x-subject={subject} authorization= body=0"),
            ),
        ),
        ("none", false, "/cert", (403, "Forbidden\n".to_owned())),
        (
            "optional, none",
            false,
            "/opt",
            (200, "x-user= x-subject= authorization= body=0".to_owned()),
        ),
        (
            "optional, verified",
            true,
            "/opt",
            (
                200,
                format!("x-user= x-subject={subject} authorization= body=0"),
            ),
        ),
    ] {
        assert_eq!(tls_get(tls, &p, cert, path, &forged), want, "{name}");
    }
    // In the clear: no certificate.
    let r = exchange(plain, "GET", "/cert", &[("X-Subject", "CN=forged")], 0);
    assert_eq!(
        (r.status, r.body.as_str()),
        (403, "Forbidden\n"),
        "in the clear"
    );

    // basic_auth's user stays who the request is, the subject in its field.
    let auth = basic("bob", "pw");
    assert_eq!(
        tls_get(tls, &p, true, "/both", &[("Authorization", &auth)]),
        (
            200,
            format!("x-user=bob x-subject={subject} authorization= body=0")
        )
    );
    assert_eq!(
        tls_get(tls, &p, false, "/both", &[("Authorization", &auth)]).0,
        403
    );
    assert_eq!(tls_get(tls, &p, true, "/both", &[]).0, 401);
}

// The certificate over HTTP/2 and HTTP/3 too.
#[test]
fn client_cert_over_h2_and_h3() {
    let p = pki("middleware-client-cert-h2-h3");
    let akit = mtls(&p, &hash("pw"));
    let (tls, udp) = (akit.addrs[0], akit.addrs[1]);
    let cert = Some((p.client.0.as_path(), p.client.1.as_path()));
    let want = "x-user= x-subject=CN=client.example authorization= body=0";
    let h2 = |cert| {
        support::https::get_h2(
            tls,
            &support::https::client(&p.ca, &["h2"], cert),
            "a.example",
            "/cert",
        )
    };
    assert_eq!(h2(cert), Ok((200, want.to_owned())));
    assert_eq!(h2(None), Ok((403, "Forbidden\n".to_owned())));
    let h3 = |cert| {
        support::h3::get(udp, support::h3::client(&p.ca, cert), "a.example", "/cert")
            .map(|s| (s.status, s.body))
    };
    assert_eq!(h3(cert), Ok((200, want.to_owned())));
    assert_eq!(h3(None), Ok((403, "Forbidden\n".to_owned())));
}
