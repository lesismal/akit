//! fit's `llm/internal/webui` tests (`api_test.go`), ported: a router of the tests' own served
//! through a role console of akit-admin's, as a role's pages serve their methods.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize};
use std::sync::mpsc;
use std::time::Instant;

use akit_roles::pages::Consoles;

use super::*;
use crate::mesh::{self, Mesh, Router};
use crate::model::{CODE_INTERNAL, Identity, ROLE_HUB};
use crate::wire::Error;

/// A role's pages over `router`, served by a console on loopback.
struct Ui {
    addr: SocketAddr,
    _consoles: akit_admin::console::RoleConsoles,
    _mesh: Mesh,
    engine: Arc<akio::Engine>,
}

impl Drop for Ui {
    fn drop(&mut self) {
        self.engine.shutdown();
        self.engine.wait();
    }
}

fn start_ui(router: Router) -> Ui {
    let engine = akio::Engine::builder().client_threads(1).build().unwrap();
    engine.start().unwrap();
    let engine = Arc::new(engine);
    let m = Mesh::new(
        mesh::Config {
            identity: Arc::new(|| Identity {
                role: ROLE_HUB.to_owned(),
                id: "hub-test".to_owned(),
                name: "test".to_owned(),
                ..Identity::default()
            }),
            accept_roles: Vec::new(),
            engine: Arc::clone(&engine),
        },
        router,
    );
    let local = Local::new(m.downgrade());
    let pages = Arc::new(LlmPages::new("hub", local, m.downgrade()));
    pages.set_me(Arc::new(|| vec![("role", "\"test\"".to_owned())]));
    let hash = akio_crypto::bcrypt::hash(b"secret", akio_crypto::bcrypt::MIN_COST).unwrap();
    let doc = format!(
        r#"{{"llm": {{"hub": {{"console": {{"address": "127.0.0.1:0", "max_conns": 64, "users": [{{"username": "admin", "password_hash": "{hash}"}}]}}}}}}}}"#
    );
    let config = akit_config::from_json(&doc, None).unwrap();
    let mut consoles = akit_admin::console::RoleConsoles::new(&akit_admin::Options::default());
    consoles
        .apply("llm.hub", Some(&config), Some(pages))
        .unwrap();
    Ui {
        addr: consoles.addr("llm.hub").expect("the console's address"),
        _consoles: consoles,
        _mesh: m,
        engine,
    }
}

/// A logged-in browser: its session's headers.
struct Browser {
    addr: SocketAddr,
    auth: String,
}

fn send(addr: SocketAddr, method: &str, path: &str, headers: &str, body: &str) -> TcpStream {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    s
}

/// The status, head and body of a whole response, a chunked one decoded.
fn read_all(mut s: TcpStream) -> (u16, String, String) {
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let raw = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let status = head.split(' ').nth(1).unwrap_or("0").parse().unwrap_or(0);
    let chunked = head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked");
    let body = if chunked {
        dechunk(body)
    } else {
        body.to_owned()
    };
    (status, head.to_owned(), body)
}

fn dechunk(mut b: &str) -> String {
    let mut out = String::new();
    while let Some((size, rest)) = b.split_once("\r\n") {
        let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if n == 0 || rest.len() < n {
            break;
        }
        out.push_str(&rest[..n]);
        b = rest[n..].strip_prefix("\r\n").unwrap_or(&rest[n..]);
    }
    out
}

fn login(addr: SocketAddr) -> Browser {
    let (status, head, body) = read_all(send(
        addr,
        "POST",
        "/api/login",
        "",
        r#"{"username":"admin","password":"secret"}"#,
    ));
    assert_eq!(status, 200, "{body}");
    let session = head
        .lines()
        .find_map(|l| l.strip_prefix("Set-Cookie: "))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let csrf = body
        .split("\"csrf\":\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    Browser {
        addr,
        auth: format!("Cookie: {session}\r\nX-Akit-Csrf: {csrf}\r\n"),
    }
}

impl Browser {
    fn open(&self, method: &str, path: &str, body: &str) -> TcpStream {
        send(self.addr, method, path, &self.auth, body)
    }

    fn call(&self, method: &str, path: &str, body: &str) -> (u16, String, String) {
        read_all(self.open(method, path, body))
    }
}

/// Server-Sent Events: name (`""` for data) and data.
fn events(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let (mut name, mut data) = (String::new(), Vec::<String>::new());
    for line in body.split('\n') {
        if line.is_empty() {
            if !name.is_empty() || !data.is_empty() {
                out.push((std::mem::take(&mut name), data.join("\n")));
                data.clear();
            }
        } else if let Some(n) = line.strip_prefix("event: ") {
            name = n.to_owned();
        } else if let Some(d) = line.strip_prefix("data: ") {
            data.push(d.to_owned());
        }
    }
    out
}

fn ev(n: &str, d: &str) -> (String, String) {
    (n.to_owned(), d.to_owned())
}

/// fit's `TestRPC`.
#[test]
fn rpc() {
    let mut r = Router::new();
    r.handle("/t/echo", &[], Duration::ZERO, |_, req, a| {
        let v = akit_config::value::parse(std::str::from_utf8(req).unwrap()).unwrap();
        match v.get("say") {
            None => a.ok(r#"{"say":""}"#.to_owned()),
            Some(akit_config::value::Value::String(s)) => a.ok(format!(r#"{{"say":{s:?}}}"#)),
            Some(_) => a.err(Error::code(CODE_BAD_REQUEST, "say is not a string")),
        }
    });
    r.handle("/t/list", &[], Duration::ZERO, |_, _, a| {
        a.ok("[1]".to_owned());
    });
    r.handle("/t/conflict", &[], Duration::ZERO, |_, _, a| {
        a.err(Error::code(CODE_CONFLICT, "taken"));
    });
    r.handle("/t/timeout", &[], Duration::from_millis(100), |_, _, a| {
        // Never answered: the method's timeout does.
        std::mem::forget(a);
    });
    let ui = start_ui(r);
    let b = login(ui.addr);
    for (method, path, body, status, want) in [
        ("GET", "/api/me", "", 200, r#""role":"test""#),
        ("GET", "/api/me", "", 200, r#""user":"admin""#),
        ("GET", "/api/me", "", 200, r#""identity":{"role":"hub""#),
        (
            "POST",
            "/api/rpc/t/echo",
            r#"{"say":"hi"}"#,
            200,
            r#"{"say":"hi"}"#,
        ),
        ("POST", "/api/rpc/t/echo", "  ", 200, r#"{"say":""}"#),
        ("POST", "/api/rpc/t/echo", "{", 400, r#""code":1000"#),
        (
            "POST",
            "/api/rpc/t/echo",
            r#"{"say":1}"#,
            400,
            r#""code":1000"#,
        ),
        ("POST", "/api/rpc/t/list", "", 200, "[1]"),
        (
            "POST",
            "/api/rpc/t/conflict",
            "",
            409,
            r#""error":"taken","code":3001"#,
        ),
        (
            "POST",
            "/api/rpc/t/timeout",
            "",
            504,
            r#""error":"timeout""#,
        ),
        ("POST", "/api/rpc/t/none", "", 404, "no such endpoint"),
        ("GET", "/api/rpc/t/list", "", 404, "no such endpoint"),
        ("POST", "/api/m/llm.hub/rpc/t/list", "", 200, "[1]"),
    ] {
        let (st, _, got) = b.call(method, path, body);
        assert!(
            st == status && got.contains(want),
            "{path} {body:?}: {st} {got}"
        );
    }
}

/// fit's `TestStatusOf`.
#[test]
fn status_of_codes() {
    for (code, want) in [
        (CODE_OK, 200),
        (CODE_BAD_REQUEST, 400),
        (CODE_UNAUTHORIZED, 401),
        (CODE_FORBIDDEN, 403),
        (CODE_NOT_FOUND, 404),
        (CODE_CONFLICT, 409),
        (CODE_RATE_LIMITED, 429),
        (CODE_UNAVAILABLE, 503),
        (CODE_INTERNAL, 500),
        (12345, 500),
    ] {
        assert_eq!(status_of(code), want, "{code}");
    }
}

/// fit's `TestStream` and `TestSSEEnds`' empty result.
#[test]
fn stream() {
    let mut r = Router::new();
    r.handle_stream("/s/ok", &[], |_, _, st| {
        for f in ["a", "", "x\ny"] {
            assert_eq!(st.try_send(f.as_bytes()), Ok(true));
        }
        st.end(Ok(br#"{"n":3}"#.to_vec()));
    });
    r.handle_stream("/s/refuse", &[], |_, _, st| {
        st.end(Err(Error::code(CODE_FORBIDDEN, "not you")));
    });
    r.handle_stream("/s/late", &[], |_, _, st| {
        assert_eq!(st.try_send(br#""one""#), Ok(true));
        st.end(Err(Error::code(CODE_CONFLICT, "gone wrong")));
    });
    r.handle_stream("/s/empty", &[], |_, _, st| st.end(Ok(Vec::new())));
    let ui = start_ui(r);
    let b = login(ui.addr);

    let (status, head, body) = b.call("POST", "/api/stream/s/ok", "");
    assert_eq!(status, 200, "{head}");
    assert!(
        head.contains("Content-Type: text/event-stream; charset=utf-8"),
        "{head}"
    );
    assert_eq!(
        events(&body),
        [
            ev("", "a"),
            ev("", "null"),
            ev("", "x\ny"),
            ev("end", r#"{"n":3}"#)
        ]
    );
    let (status, _, body) = b.call("POST", "/api/stream/s/refuse", "");
    assert!(
        status == 403 && body.contains(r#""error":"not you""#),
        "refused before any frame: {status} {body}"
    );
    let (_, _, body) = b.call("POST", "/api/stream/s/late", "");
    assert_eq!(
        events(&body),
        [
            ev("", r#""one""#),
            ev("error", r#"{"code":3001,"message":"gone wrong"}"#)
        ]
    );
    let (status, _, body) = b.call("POST", "/api/stream/s/ok", "{");
    assert!(
        status == 400 && body.contains("not JSON"),
        "bad body: {status} {body}"
    );
    let (status, _, body) = b.call("POST", "/api/stream/s/empty", "");
    assert_eq!(status, 200);
    assert_eq!(events(&body), [ev("end", "null")]);
}

/// fit's `TestStreamLimit`: 16 streams at once, one more refused; a browser gone cancels its
/// stream, which frees its place.
#[test]
fn stream_limit() {
    let open = Arc::new(AtomicUsize::new(0));
    let mut r = Router::new();
    let o = Arc::clone(&open);
    r.handle_stream("/s/wait", &[], move |_, _, st| {
        o.fetch_add(1, Ordering::SeqCst);
        let o2 = Arc::clone(&o);
        st.on_finish(move |_| {
            o2.fetch_sub(1, Ordering::SeqCst);
        });
        assert_eq!(st.try_send(b"1"), Ok(true));
    });
    let ui = start_ui(r);
    let b = login(ui.addr);
    let mut conns = Vec::new();
    for i in 0..MAX_STREAMS {
        let mut s = b.open("POST", "/api/stream/s/wait", "");
        let mut got = Vec::new();
        let mut buf = [0u8; 1024];
        while !String::from_utf8_lossy(&got).contains("data: 1") {
            let n = s.read(&mut buf).unwrap();
            assert!(n > 0, "stream {i} ended");
            got.extend_from_slice(&buf[..n]);
        }
        assert!(got.starts_with(b"HTTP/1.1 200"), "stream {i}");
        conns.push(s);
    }
    let (status, _, body) = b.call("POST", "/api/stream/s/wait", "");
    assert!(
        status == 429 && body.contains(r#""code":4000"#),
        "one too many: {status} {body}"
    );
    drop(conns);
    let deadline = Instant::now() + Duration::from_secs(10);
    while open.load(Ordering::SeqCst) != 0 {
        assert!(
            Instant::now() < deadline,
            "{} streams still open",
            open.load(Ordering::SeqCst)
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    loop {
        let mut s = b.open("POST", "/api/stream/s/wait", "");
        let mut head = [0u8; 12];
        s.read_exact(&mut head).unwrap();
        if &head == b"HTTP/1.1 200" {
            break;
        }
        assert!(Instant::now() < deadline, "no place freed");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// fit's `bigStream`: `total` bytes in frames of 64 KiB, sent as the stream's credit allows.
struct Big {
    total: usize,
    sent: AtomicUsize,
    ended: Mutex<Option<mpsc::Sender<Option<Error>>>>,
}

fn pump(big: Arc<Big>, st: Stream) {
    let frame = vec![b'z'; 64 << 10];
    loop {
        if big.sent.load(Ordering::SeqCst) >= big.total {
            st.end(Ok(br#""done""#.to_vec()));
            return;
        }
        match st.try_send(&frame) {
            Ok(true) => {
                big.sent.fetch_add(frame.len(), Ordering::SeqCst);
            }
            Ok(false) => {
                let (b2, s2) = (Arc::clone(&big), st.clone());
                if st.on_credit(move || pump(b2, s2)) {
                    return;
                }
            }
            Err(_) => return,
        }
    }
}

/// Waits until the big stream stops sending: held back, or done.
fn stalled(big: &Big) {
    let (mut last, mut still) = (usize::MAX, 0);
    while still < 6 && big.sent.load(Ordering::SeqCst) < big.total {
        std::thread::sleep(Duration::from_millis(50));
        let n = big.sent.load(Ordering::SeqCst);
        if n == last {
            still += 1;
        } else {
            (last, still) = (n, 0);
        }
    }
}

/// fit's `TestStreamSlowBrowser`: a browser that reads only once the stream is held back gets
/// it all; one that goes away while it is held back cancels it.
#[test]
fn stream_slow_browser() {
    let (tx, rx) = mpsc::channel();
    let big = Arc::new(Big {
        total: 32 << 20,
        sent: AtomicUsize::new(0),
        ended: Mutex::new(Some(tx)),
    });
    let mut r = Router::new();
    let b2 = Arc::clone(&big);
    r.handle_stream("/s/big", &[], move |_, _, st| {
        let tx = lock(&b2.ended).clone();
        st.on_finish(move |e| {
            if let Some(tx) = tx {
                let _ = tx.send(e.cloned());
            }
        });
        pump(Arc::clone(&b2), st);
    });
    let ui = start_ui(r);
    let b = login(ui.addr);

    let s = b.open("POST", "/api/stream/s/big", "");
    stalled(&big);
    let held = big.sent.load(Ordering::SeqCst) < big.total;
    let (status, _, body) = read_all(s);
    assert_eq!(status, 200);
    let mut n = 0;
    let mut end = String::new();
    for (name, data) in events(&body) {
        if name.is_empty() {
            n += data.len();
        } else {
            end = data;
        }
    }
    assert_eq!((n, end.as_str()), (big.total, r#""done""#), "held: {held}");
    // The responder's own end, a good one, is `StreamEnded` on its side.
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(10)).unwrap(),
        Some(Error::StreamEnded)
    );

    big.sent.store(0, Ordering::SeqCst);
    let s = b.open("POST", "/api/stream/s/big", "");
    stalled(&big);
    let held = big.sent.load(Ordering::SeqCst) < big.total;
    drop(s);
    let err = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("the stream went on after its browser went away");
    assert!(
        !held || err != Some(Error::StreamEnded),
        "a stream held back ended well after its browser went away: {err:?}"
    );
}

/// A stream held back longer than the stall timeout is canceled (fit's write deadline).
#[test]
fn a_stalled_browser_cancels_its_stream() {
    let ended = Arc::new(AtomicBool::new(false));
    let sent = Arc::new(AtomicI64::new(0));
    let mut r = Router::new();
    let (e2, s2) = (Arc::clone(&ended), Arc::clone(&sent));
    let big = Arc::new(Big {
        total: 256 << 20,
        sent: AtomicUsize::new(0),
        ended: Mutex::new(None),
    });
    r.handle_stream("/s/big", &[], move |_, _, st| {
        let e3 = Arc::clone(&e2);
        st.on_finish(move |_| e3.store(true, Ordering::SeqCst));
        s2.store(1, Ordering::SeqCst);
        pump(Arc::clone(&big), st);
    });
    let engine = akio::Engine::builder().client_threads(1).build().unwrap();
    engine.start().unwrap();
    let engine = Arc::new(engine);
    let m = Mesh::new(
        mesh::Config {
            identity: Arc::new(Identity::default),
            accept_roles: Vec::new(),
            engine: Arc::clone(&engine),
        },
        r,
    );
    let pages = Arc::new(
        LlmPages::new("node", Local::new(m.downgrade()), m.downgrade())
            .with_stall(Duration::from_millis(300)),
    );
    let hash = akio_crypto::bcrypt::hash(b"secret", akio_crypto::bcrypt::MIN_COST).unwrap();
    let doc = format!(
        r#"{{"llm": {{"node": {{"console": {{"address": "127.0.0.1:0", "users": [{{"username": "admin", "password_hash": "{hash}"}}]}}}}}}}}"#
    );
    let config = akit_config::from_json(&doc, None).unwrap();
    let mut consoles = akit_admin::console::RoleConsoles::new(&akit_admin::Options::default());
    consoles
        .apply("llm.node", Some(&config), Some(pages))
        .unwrap();
    let b = login(consoles.addr("llm.node").expect("the console's address"));
    // Never read: the stream is held back, then canceled at the stall timeout.
    let s = b.open("POST", "/api/stream/s/big", "");
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ended.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "the stalled stream went on");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(sent.load(Ordering::SeqCst), 1);
    drop(s);
    consoles.stop();
    engine.shutdown();
    engine.wait();
}

#[test]
fn events_are_written_line_by_line() {
    assert_eq!(event("", b"a\r\nb"), b"data: a\ndata: b\n\n");
    assert_eq!(event("end", b"null"), b"event: end\ndata: null\n\n");
}
