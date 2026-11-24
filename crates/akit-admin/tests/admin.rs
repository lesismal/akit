//! The Admin API (I26), with fit's `admin/admin_test.go` ported: loads applied through I22's
//! reload (`Listeners::reload`) by a control thread, read back whole, by pointer and by version;
//! what is refused, and how (bad configs, wrong types, bodies too large, web pages, stale
//! ETags); history; concurrent loads; slow and cut clients; the connection limit. On the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`). Not ported: fit's token and Unix socket
//! tests (I27, akio API request A23).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use akio::{Backend, UringConfig};
use akit_admin::{
    Applied, ApplyError, Configs, Job, ListenerChange, Options, SOURCE_ADMIN, SOURCE_FILE, Server,
    VERSION_HEADER,
};
use akit_listener::Listeners;
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

/// A config with one listener on a free port, answering `body`: fit's `doc`.
fn doc(body: &str) -> String {
    format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
  "routes": [{{"handler": {{"module": "respond", "params": {{"body": "{body}"}}}}}}]}}"#
    )
}

fn admin(f: impl FnOnce(&mut akit_config::Admin)) -> akit_config::Admin {
    let mut a = akit_config::Admin {
        address: "127.0.0.1:0".into(),
        ..akit_config::Admin::default()
    };
    f(&mut a);
    a
}

/// A running server with its configs, its Admin API and the control thread that applies loads,
/// as `akit run` has them.
struct Fixture {
    configs: Configs,
    api: Server,
    /// The listener `web`, kept by every load of `doc`.
    web: SocketAddr,
    jobs: mpsc::SyncSender<Option<Job>>,
    control: Option<JoinHandle<()>>,
    engine: Arc<akio::Engine>,
}

/// Applies `text` as the control thread of `akit run` does (its `Runner::apply_text`).
fn apply(
    text: &str,
    listeners: &mut Listeners,
    first: Option<&akit_config::Admin>,
) -> Result<Applied, ApplyError> {
    let config = akit_config::from_json(text, None).map_err(ApplyError::Config)?;
    let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
    let graph = compile(&config, &registry, &Env::default()).map_err(ApplyError::Config)?;
    let r = listeners
        .reload(graph)
        .map_err(|e| ApplyError::Failed(e.to_string()))?;
    let change = |l: &akit_listener::Served| ListenerChange {
        name: l.name.clone(),
        address: l.address.to_string(),
        protocols: l.protocols.iter().map(|p| p.as_str().to_owned()).collect(),
    };
    Ok(Applied {
        added: r.added.iter().map(change).collect(),
        removed: r.removed.iter().map(change).collect(),
        warnings: akit_admin::admin_warning(first, config.admin.as_ref())
            .into_iter()
            .collect(),
    })
}

fn start(doc: &str, settings: akit_config::Admin) -> Fixture {
    start_with(doc, settings, Options::default())
}

fn start_with(doc: &str, settings: akit_config::Admin, mut opts: Options) -> Fixture {
    let config = akit_config::from_json(doc, None).unwrap();
    let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
    let cell = GraphCell::new(compile(&config, &registry, &Env::default()).unwrap());
    let mut lo = akit_listener::Options::default();
    lo.backend = backend();
    lo.shutdown_timeout = Duration::from_secs(5);
    let engine = Arc::new(akit_listener::engine(&cell, &lo).unwrap());
    engine.start().unwrap();
    let mut listeners = Listeners::new(Arc::clone(&engine), cell.clone(), &lo);
    let web = listeners.served()[0].address;

    let configs = Configs::new(
        akit_config::value::standardize(doc).unwrap(),
        SOURCE_FILE,
        settings.history,
    );
    let (jobs, rx) = mpsc::sync_channel::<Option<Job>>(64);
    let tx = jobs.clone();
    let submit: akit_admin::Submit = Box::new(move |job| match tx.try_send(Some(job)) {
        Ok(()) => Ok(()),
        Err(mpsc::TrySendError::Full(Some(j)) | mpsc::TrySendError::Disconnected(Some(j))) => {
            Err(j)
        }
        Err(_) => Ok(()),
    });
    opts.backend = backend();
    let api = Server::start(&settings, configs.clone(), submit, &opts).unwrap();
    let cs = configs.clone();
    let first = config.admin.clone();
    let control = std::thread::spawn(move || {
        while let Ok(Some(job)) = rx.recv() {
            job.run(&cs, |text| apply(text, &mut listeners, first.as_ref()));
            cell.collect();
        }
    });
    Fixture {
        configs,
        api,
        web,
        jobs,
        control: Some(control),
        engine,
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.api.shutdown();
        let _ = self.jobs.send(None);
        if let Some(c) = self.control.take() {
            let _ = c.join();
        }
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
    }
}

/// A response: its status, header fields and body.
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

fn parse(raw: &str) -> Resp {
    let (head, body) = raw.split_once("\r\n\r\n").expect(raw);
    let mut lines = head.lines();
    let status = lines.next().unwrap().split(' ').nth(1).unwrap();
    Resp {
        status: status.parse().unwrap(),
        headers: lines
            .filter_map(|l| l.split_once(':'))
            .map(|(n, v)| (n.to_owned(), v.trim().to_owned()))
            .collect(),
        body: body.to_owned(),
    }
}

/// One request on a connection of its own, `Connection: close`; `Host` is the API's address
/// unless `headers` gives one.
fn request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Resp {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\n");
    if !headers.iter().any(|(n, _)| n.eq_ignore_ascii_case("host")) {
        req.push_str(&format!("Host: {addr}\r\n"));
    }
    for (n, v) in headers {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    if let Some(b) = body {
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).unwrap();
    if let Some(b) = body {
        // A server that answers before the whole body is sent may close first.
        let _ = s.write_all(b.as_bytes());
    }
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    parse(&String::from_utf8_lossy(&raw))
}

/// One response read from `s` by its `Content-Length`, the connection left open.
fn read_response(s: &mut TcpStream) -> Resp {
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = s.read(&mut buf).unwrap();
        assert!(n > 0, "closed before a whole response: {raw:?}");
        raw.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&raw).into_owned();
        if let Some((_, body)) = text.split_once("\r\n\r\n") {
            let r = parse(&text);
            let len: usize = r.header("Content-Length").unwrap().parse().unwrap();
            if body.len() >= len {
                return r;
            }
        }
    }
}

impl Fixture {
    fn get(&self, path: &str) -> Resp {
        request(self.api.addr(), "GET", path, &[], None)
    }

    fn load(&self, config: &str) -> Resp {
        self.load_with(config, &[])
    }

    fn load_with(&self, config: &str, headers: &[(&str, &str)]) -> Resp {
        let mut h = vec![("Content-Type", "application/json")];
        h.extend_from_slice(headers);
        request(self.api.addr(), "POST", "/load", &h, Some(config))
    }

    /// What the listener `web` answers.
    fn served(&self) -> String {
        request(self.web, "GET", "/", &[("Host", "x")], None).body
    }
}

#[test]
fn load_and_read_back() {
    let f = start(&doc("v1"), admin(|_| {}));
    let r = f.get("/config");
    assert_eq!(
        (r.status, r.body.as_str(), r.header(VERSION_HEADER)),
        (200, doc("v1").as_str(), Some("1"))
    );
    assert_eq!(r.header("Content-Type"), Some("application/json"));
    let etag1 = r.header("ETag").unwrap().to_owned();
    assert_eq!(etag1, akit_admin::etag_of(doc("v1").as_bytes()));

    let r = f.load(&doc("v2"));
    let etag2 = r.header("ETag").unwrap().to_owned();
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.header(VERSION_HEADER), Some("2"));
    assert_eq!(r.body, format!("{{\"version\":2,\"etag\":{etag2:?}}}\n"));
    assert_eq!(f.served(), "v2");

    let r = f.get("/config");
    assert_eq!(
        (r.body.as_str(), r.header(VERSION_HEADER)),
        (doc("v2").as_str(), Some("2"))
    );
    assert_eq!(f.get("/versions/1").body, doc("v1"));
    assert_eq!(f.get("/versions/7").status, 404);
    assert_eq!(f.get("/versions/x").status, 404);
    let r = f.get("/load");
    assert_eq!((r.status, r.header("Allow")), (405, Some("POST")));
    assert_eq!(f.get("/nothing").status, 404);

    // If-None-Match: 304 while the config is the one named.
    let r = request(
        f.api.addr(),
        "GET",
        "/config",
        &[("If-None-Match", &etag2)],
        None,
    );
    assert_eq!((r.status, r.body.as_str()), (304, ""));
    assert_eq!(r.header(VERSION_HEADER), Some("2"));
    let r = request(
        f.api.addr(),
        "GET",
        "/config",
        &[("If-None-Match", &etag1)],
        None,
    );
    assert_eq!(r.status, 200);
    // HEAD answers as GET, without the body.
    let r = request(f.api.addr(), "HEAD", "/config", &[], None);
    assert_eq!((r.status, r.body.as_str()), (200, ""));

    // A value by JSON pointer, written as fit writes it.
    let r = f.get("/config/listeners/0");
    assert_eq!(r.status, 200);
    assert_eq!(
        r.body,
        "{\n  \"name\": \"web\",\n  \"address\": \"127.0.0.1:0\"\n}\n"
    );
    assert_eq!(
        f.get("/config/routes/0/handler/params/body").body,
        "\"v2\"\n"
    );
    assert_eq!(f.get("/config/").body, doc("v2"));
    let r = f.get("/config/upstreams");
    assert_eq!(r.status, 404);
    assert!(
        r.body.contains(r#"there is no member \"upstreams\""#),
        "{}",
        r.body
    );
    assert_eq!(f.get("/config/listeners/9").status, 404);
    assert_eq!(f.get("/config/listeners/~2").status, 400);
}

/// A config with comments and trailing commas is kept as standard JSON.
#[test]
fn load_with_comments() {
    let f = start(&doc("v1"), admin(|_| {}));
    let src = "// v2\n{\"listeners\": [{\"name\": \"web\", \"address\": \"127.0.0.1:0\",},], /* routes */\n \"routes\": [{\"handler\": {\"module\": \"respond\", \"params\": {\"body\": \"v2\"}}}],}";
    let r = f.load(src);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(f.served(), "v2");
    let body = f.get("/config").body;
    assert!(akit_config::value::parse(&body).is_ok());
    assert!(!body.contains("//") && !body.contains("/*"), "{body}");
    assert_eq!(body.len(), src.len());
    let r = f.load("{/* open");
    assert_eq!(r.status, 400);
    assert!(r.body.contains("does not end"), "{}", r.body);
}

#[test]
fn loads_refused() {
    let f = start(&doc("v1"), admin(|a| a.max_config_bytes = 1024));
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let taken_addr = taken.local_addr().unwrap();
    let cases: Vec<(&str, &str, String, u16, String)> = vec![
        (
            "invalid",
            "application/json",
            r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0", "bogus": 1}]}"#.into(),
            400,
            r#"{"error":"the config was not applied; the running config stays","errors":[{"path":"listeners[0].bogus","line":1,"column":58,"message":"unknown field"}]}"#.into(),
        ),
        (
            "not compiled",
            "application/json",
            r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}], "routes": [{"handler": {"module": "nope"}}]}"#.into(),
            400,
            // akit's message (akit-config checks the module's namespace): fit says "there is no
            // module \"nope\"".
            r#"there is no handler module \"nope\""#.into(),
        ),
        (
            "not bound",
            "application/json",
            format!(
                r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}, {{"name": "b", "address": "{taken_addr}"}}]}}"#
            ),
            500,
            "ddress already in use".into(),
        ),
        ("not JSON", "text/plain", doc("v2"), 415, "application/json".into()),
        (
            "too large",
            "application/json",
            " ".repeat(1025) + &doc("v2"),
            413,
            "the body is larger than max_config_bytes, 1024".into(),
        ),
        (
            "not JSON at all",
            "application/json; charset=utf-8",
            "{\"listeners\": [".into(),
            400,
            "the document ends early".into(),
        ),
    ];
    for (name, ct, body, status, want) in cases {
        let r = request(
            f.api.addr(),
            "POST",
            "/load",
            &[("Content-Type", ct)],
            Some(&body),
        );
        assert_eq!(r.status, status, "{name}: {r:?}");
        assert!(r.body.contains(&want), "{name}: {}", r.body);
        assert_eq!(r.header("Content-Type"), Some("application/json"), "{name}");
    }
    let r = f.load(r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0", "bogus": 1}]}"#);
    assert_eq!(
        r.body,
        "{\"error\":\"the config was not applied; the running config stays\",\"errors\":[{\"path\":\"listeners[0].bogus\",\"line\":1,\"column\":58,\"message\":\"unknown field\"}]}\n"
    );

    // A body too large that does not say its length is cut off at the limit.
    let mut s = TcpStream::connect(f.api.addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let chunk = " ".repeat(2048);
    write!(
        s,
        "POST /load HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{chunk}\r\n",
        chunk.len()
    )
    .unwrap();
    let _ = write!(s, "0\r\n\r\n");
    // The rest of the body is read and dropped, and the connection kept (akio API request A24
    // would close it, as fit does): the answer is read by its length.
    let r = read_response(&mut s);
    assert_eq!(r.status, 413, "{r:?}");
    assert!(r.body.contains("max_config_bytes, 1024"), "{}", r.body);

    // Endpoints and options that come with I27.
    for (method, path) in [
        ("PUT", "/config"),
        ("PATCH", "/config/listeners/0"),
        ("DELETE", "/config/routes/0"),
        ("POST", "/reload"),
        ("POST", "/rollback/1"),
    ] {
        let r = request(f.api.addr(), method, path, &[], None);
        assert_eq!(r.status, 501, "{method} {path}");
        assert!(r.body.contains("not supported yet"), "{}", r.body);
    }
    let r = request(
        f.api.addr(),
        "POST",
        "/load?dry_run=1",
        &[("Content-Type", "application/json")],
        Some(&doc("v2")),
    );
    assert_eq!(r.status, 501);
    let r = request(
        f.api.addr(),
        "POST",
        "/load?dry_run=maybe",
        &[("Content-Type", "application/json")],
        Some(&doc("v2")),
    );
    assert_eq!(r.status, 400);

    assert_eq!(f.served(), "v1");
    assert_eq!(f.configs.current().number, 1);
    assert_eq!(f.configs.versions().len(), 1);
    drop(taken);
}

/// If-Match: a load is applied only while the config it names runs.
#[test]
fn if_match() {
    let f = start(&doc("v1"), admin(|_| {}));
    let etag1 = f.get("/config").header("ETag").unwrap().to_owned();
    assert_eq!(f.load_with(&doc("v2"), &[("If-Match", &etag1)]).status, 200);
    // etag1 is stale now.
    let r = f.load_with(&doc("v3"), &[("If-Match", &etag1)]);
    assert_eq!(r.status, 412);
    assert!(r.body.contains("has changed since the ETag"), "{}", r.body);
    assert_eq!(f.served(), "v2");
    assert_eq!(f.load_with(&doc("v3"), &[("If-Match", "*")]).status, 200);
    assert_eq!(f.served(), "v3");
}

/// What a web page could send: a request with Origin, and one to a name that resolves to
/// 127.0.0.1 after DNS rebinding.
#[test]
fn browsers_refused() {
    let f = start(&doc("v1"), admin(|_| {}));
    let own = format!("http://{}", f.api.addr());
    for (method, host, origin, status) in [
        ("GET", "", "", 200),
        ("GET", "localhost:2019", "", 200),
        ("GET", "[::1]:2019", "", 200),
        ("GET", "evil.example.com:2019", "", 403),
        ("GET", "", "https://evil.example.com", 403),
        ("GET", "", "null", 403),
        // A read from the API's own origin passes; a write never does.
        ("GET", "", own.as_str(), 200),
        ("POST", "", own.as_str(), 403),
    ] {
        let mut h = Vec::new();
        if !host.is_empty() {
            h.push(("Host", host));
        }
        if !origin.is_empty() {
            h.push(("Origin", origin));
        }
        let path = if method == "GET" { "/config" } else { "/load" };
        h.push(("Content-Type", "application/json"));
        let body = (method == "POST").then(|| doc("v2"));
        let r = request(f.api.addr(), method, path, &h, body.as_deref());
        assert_eq!(
            r.status, status,
            "{method} Host {host:?} Origin {origin:?}: {r:?}"
        );
    }
    assert_eq!(f.served(), "v1");
}

#[test]
fn history() {
    let f = start(&doc("v1"), admin(|a| a.history = 2));
    for body in ["v2", "v3"] {
        assert_eq!(f.load(&doc(body)).status, 200);
    }
    let r = f.get("/versions");
    assert_eq!(r.status, 200);
    let v = akit_config::value::parse(&r.body).unwrap();
    use akit_config::value::Value::{Array, Number, String as Str};
    assert_eq!(v.get("current"), Some(&Number("3".into())));
    let Array(vs) = v.get("versions").unwrap() else {
        panic!("{}", r.body)
    };
    assert_eq!(vs.len(), 2);
    let field = |i: usize, k: &str| vs[i].get(k).unwrap().clone();
    assert_eq!(field(0, "version"), Number("2".into()));
    assert_eq!(field(1, "source"), Str(SOURCE_ADMIN.into()));
    assert_eq!(field(1, "bytes"), Number(doc("v3").len().to_string()));
    assert_eq!(
        field(1, "etag"),
        Str(akit_admin::etag_of(doc("v3").as_bytes()))
    );
    let Str(applied) = field(1, "applied") else {
        panic!()
    };
    assert!(applied.ends_with('Z') && applied.len() == 24, "{applied}");
    assert_eq!(f.get("/versions/1").status, 404);
    assert_eq!(f.get("/versions/2").body, doc("v2"));
}

/// Configs from many clients at once: each applied whole, one after another, with a version of
/// its own.
#[test]
fn concurrent_loads() {
    let f = Arc::new(start(&doc("v0"), admin(|a| a.history = 100)));
    const N: usize = 8;
    let handles: Vec<_> = (0..N)
        .map(|i| {
            let f = Arc::clone(&f);
            std::thread::spawn(move || {
                let r = f.load(&doc(&format!("c{i}")));
                assert_eq!(r.status, 200, "{r:?}");
                r.header(VERSION_HEADER).unwrap().to_owned()
            })
        })
        .collect();
    let mut versions: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    versions.sort();
    versions.dedup();
    assert_eq!(versions.len(), N, "a version given twice: {versions:?}");
    let cur = f.configs.current();
    assert_eq!(cur.number, N as u64 + 1);
    assert_eq!(f.configs.versions().len(), N + 1);
    let want = cur
        .data()
        .split("\"body\": \"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    assert_eq!(f.served(), want);
}

/// A client that stops sending its config is cut off when the body timeout passes, and one whose
/// connection is cut midway applies nothing; neither holds the API up for others.
#[test]
fn slow_and_cut_clients() {
    let mut opts = Options::default();
    opts.body_timeout = Duration::from_millis(500);
    let f = start_with(&doc("v1"), admin(|_| {}), opts);
    let partial = || {
        let mut c = TcpStream::connect(f.api.addr()).unwrap();
        let d = doc("v2");
        write!(
            c,
            "POST /load HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            d.len(),
            &d[..20]
        )
        .unwrap();
        c
    };
    let mut slow = partial();
    let cut = partial();
    cut.shutdown(Shutdown::Both).unwrap();
    drop(cut);

    // Others are served meanwhile.
    assert_eq!(f.get("/config").status, 200);
    slow.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let began = Instant::now();
    let mut raw = Vec::new();
    match slow.read_to_end(&mut raw) {
        Ok(_) => {
            if !raw.is_empty() {
                let r = parse(&String::from_utf8_lossy(&raw));
                assert!(matches!(r.status, 400 | 408), "slow client: {r:?}");
            }
        }
        Err(e) => assert!(
            !matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ),
            "the slow client was not cut off"
        ),
    }
    assert!(
        began.elapsed() < Duration::from_secs(3),
        "cut off after {:?}",
        began.elapsed()
    );
    assert_eq!(f.served(), "v1");
    assert_eq!(f.configs.current().number, 1);
}

/// Connections past max_conns are closed at once, and once some go, new ones are served again.
#[test]
fn connection_limit() {
    let f = start(&doc("v1"), admin(|a| a.max_conns = 2));
    let mut held = Vec::new();
    for _ in 0..2 {
        let mut c = TcpStream::connect(f.api.addr()).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        // Accepted once it answers; kept alive.
        write!(c, "GET /versions HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        let mut buf = [0u8; 4096];
        let n = c.read(&mut buf).unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));
        held.push(c);
    }
    let mut extra = TcpStream::connect(f.api.addr()).unwrap();
    extra
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let _ = write!(extra, "GET /config HTTP/1.1\r\nHost: localhost\r\n\r\n");
    match extra.read(&mut [0u8; 1]) {
        Ok(0) => {}
        Ok(n) => panic!("a connection past the limit was served: {n} bytes"),
        Err(e) => assert!(
            !matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ),
            "a connection past the limit was left open"
        ),
    }
    drop(extra);
    drop(held);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let ok = TcpStream::connect(f.api.addr()).ok().and_then(|mut s| {
            s.set_read_timeout(Some(Duration::from_secs(1))).ok()?;
            write!(
                s,
                "GET /config HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
            )
            .ok()?;
            let mut raw = String::new();
            s.read_to_string(&mut raw).ok()?;
            raw.starts_with("HTTP/1.1 200").then_some(())
        });
        if ok.is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "not served once the connections went"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// What a load changed: listeners opened and closed, and the change to admin, which takes a
/// restart.
#[test]
fn listeners_and_warnings() {
    let f = start(&doc("v1"), admin(|_| {}));
    let two = r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}, {"name": "extra", "address": "127.0.0.1:0"}],
  "admin": {"address": "127.0.0.1:1"}}"#;
    let r = f.load(two);
    assert_eq!(r.status, 200, "{r:?}");
    let v = akit_config::value::parse(&r.body).unwrap();
    let added = v.get("listeners_added").unwrap().to_json();
    assert!(added.contains("\"name\": \"extra\""), "{added}");
    assert!(added.contains("\"http/1.1\""), "{added}");
    assert!(v.get("listeners_removed").is_none());
    assert_eq!(
        v.get("warnings").unwrap().to_json(),
        "[\n  \"admin changed; the change takes effect when akit restarts\"\n]\n"
    );
    let r = f.load(&doc("v3"));
    let v = akit_config::value::parse(&r.body).unwrap();
    assert!(
        v.get("listeners_removed")
            .unwrap()
            .to_json()
            .contains("\"extra\"")
    );
    assert!(v.get("warnings").is_none(), "{}", r.body);
    assert_eq!(f.served(), "v3");
}
