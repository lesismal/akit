//! The Admin API (I26, I27), with fit's `admin/admin_test.go`, `edit_test.go` and
//! `dryrun_test.go` ported: loads and edits applied through I22's reload (`Listeners::reload`) by
//! a control thread, read back whole, by pointer and by version; what is refused, and how (bad
//! configs, wrong types, bodies too large, web pages, stale ETags, missing tokens); edits by
//! pointer; concurrent writers; rollback; reload; dry runs; history; concurrent loads; slow and
//! cut clients; the connection limit. On the backend `AKIO_TEST_BACKEND` names (epoll by default,
//! `uring`); fit's Unix socket test (akio API request A23). Not ported: fit's console, store and
//! `/config/text` cases (not in akit).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use akio::{Backend, UringConfig};
use akit_admin::{
    Applied, ApplyError, Configs, DRY_RUN_HEADER, Job, ListenerChange, Mode, Options, SOURCE_ADMIN,
    SOURCE_FILE, SOURCE_ROLLBACK, Server, VERSION_HEADER, etag_of,
};
use akit_config::value::Value;
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

/// Applies `text`, or checks it, as the control thread of `akit run` does (its
/// `Runner::apply_text`).
fn apply(
    text: &str,
    mode: Mode,
    listeners: &mut Listeners,
    first: Option<&akit_config::Admin>,
) -> Result<Applied, ApplyError> {
    let config = akit_config::from_json(text, None).map_err(ApplyError::Config)?;
    let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
    let graph = compile(&config, &registry, &Env::default()).map_err(ApplyError::Config)?;
    let r = match mode {
        Mode::Apply => listeners.reload(graph),
        Mode::DryRun => listeners.plan(&graph),
    }
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
    let mut config = akit_config::from_json(doc, None).unwrap();
    // Two I/O threads, not one a core: the tests run at once, and each thread holds an epoll or
    // io_uring and an eventfd. Only the engine's: what is applied and read back is the text, and
    // a reload keeps the threads akit started with.
    config.engine.event_loops = 2;
    let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
    let cell = GraphCell::new(compile(&config, &registry, &Env::default()).unwrap());
    let mut lo = akit_listener::Options::default();
    lo.backend = Some(backend());
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
    opts.backend = Some(backend());
    let api = Server::start(&settings, configs.clone(), submit, &opts).unwrap();
    let cs = configs.clone();
    let first = config.admin.clone();
    let control = std::thread::spawn(move || {
        while let Ok(Some(job)) = rx.recv() {
            job.run(&cs, |text, mode| {
                apply(text, mode, &mut listeners, first.as_ref())
            });
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

    /// A request with a body of `content_type` when it is not empty, and `If-Match` when
    /// `if_match` is not empty: fit's `send`.
    fn send(
        &self,
        method: &str,
        path: &str,
        content_type: &str,
        body: &str,
        if_match: &str,
    ) -> Resp {
        let mut h = Vec::new();
        if !content_type.is_empty() {
            h.push(("Content-Type", content_type));
        }
        if !if_match.is_empty() {
            h.push(("If-Match", if_match));
        }
        let body = (!body.is_empty() || !content_type.is_empty()).then_some(body);
        request(self.api.addr(), method, path, &h, body)
    }

    fn request_with(&self, method: &str, path: &str, headers: &[(&str, &str)]) -> Resp {
        request(self.api.addr(), method, path, headers, None)
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
    assert_eq!(etag1, etag_of(doc("v1").as_bytes()));

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
            r#"there is no module \"nope\""#.into(),
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
    // One write, which the server reads whole: what it refuses is not left unread, which could
    // make the close a reset that overtakes the answer.
    let head = format!(
        "POST /load HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{chunk}\r\n",
        chunk.len()
    );
    s.write_all(head.as_bytes()).unwrap();
    // Answered, and the connection closed rather than the rest of the body read, as fit's.
    let r = read_response(&mut s);
    assert_eq!(r.status, 413, "{r:?}");
    assert!(r.body.contains("max_config_bytes, 1024"), "{}", r.body);
    assert_eq!(r.header("Connection"), Some("close"), "{r:?}");
    let mut rest = Vec::new();
    let _ = s.read_to_end(&mut rest);
    assert!(rest.is_empty(), "{rest:?}");

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
    assert_eq!(field(1, "etag"), Str(etag_of(doc("v3").as_bytes())));
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

/// GET /metrics answers with the scrape `Options::metrics` writes (I37's), 404 without one.
#[test]
fn metrics() {
    let f = start(&doc("v1"), admin(|_| {}));
    assert_eq!(f.get("/metrics").status, 404);
    let mut opts = Options::default();
    opts.metrics = Some(Arc::new(|out: &mut Vec<u8>| {
        out.extend_from_slice(b"akit_config_version 1\n");
    }));
    let f = start_with(&doc("v1"), admin(|_| {}), opts);
    let r = f.get("/metrics");
    assert_eq!(
        (r.status, r.body.as_str()),
        (200, "akit_config_version 1\n")
    );
    assert_eq!(
        r.header("Content-Type"),
        Some(akit_admin::METRICS_CONTENT_TYPE)
    );
    assert_eq!(
        request(f.api.addr(), "POST", "/metrics", &[], None).status,
        405
    );
}

/// fit's `TestEditByPointer`: a value read, set, merged, appended and removed by pointer; the
/// config edited is written out again, a whole config put is kept as written.
#[test]
fn edit_by_pointer() {
    let f = start(&doc("v1"), admin(|_| {}));
    let r = f.get("/config/routes/0/handler/params/body");
    assert_eq!((r.status, r.body.as_str()), (200, "\"v1\"\n"), "{r:?}");
    assert_eq!(r.header("ETag"), Some(f.configs.current().etag.as_str()));
    assert_eq!(f.get("/config/routes/3").status, 404);

    let r = f.send(
        "PUT",
        "/config/routes/0/handler/params/body",
        "application/json",
        r#""v2""#,
        "",
    );
    assert_eq!(
        (r.status, r.header(VERSION_HEADER)),
        (200, Some("2")),
        "{r:?}"
    );
    assert_eq!(f.served(), "v2");
    let r = f.send(
        "PATCH",
        "/config/routes/0/handler/params",
        "application/merge-patch+json",
        r#"{"body": "v3", "status": 201}"#,
        "",
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(f.served(), "v3");
    let r = f.send(
        "PUT",
        "/config/routes/-",
        "application/json",
        r#"{"name": "second", "handler": {"module": "respond", "params": {"body": "v4"}}}"#,
        "",
    );
    assert_eq!(r.status, 200, "{r:?}");
    let r = f.send("DELETE", "/config/routes/0", "", "", "");
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(f.served(), "v4");
    assert_eq!(
        f.get("/config").body,
        r#"{
  "listeners": [
    {
      "name": "web",
      "address": "127.0.0.1:0"
    }
  ],
  "routes": [
    {
      "name": "second",
      "handler": {
        "module": "respond",
        "params": {
          "body": "v4"
        }
      }
    }
  ]
}
"#
    );
    // A whole config put is kept as written.
    let r = f.send("PUT", "/config", "application/json", &doc("v5"), "");
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(f.get("/config").body, doc("v5"));
    assert_eq!(f.configs.current().number, 6);
}

/// fit's `TestEditsRefused`: edits that leave the config invalid, or cannot be made, change
/// nothing.
#[test]
fn edits_refused() {
    let f = start(&doc("v1"), admin(|_| {}));
    for (method, path, ct, body, status, want) in [
        (
            "PATCH",
            "/config/listeners/0",
            "application/merge-patch+json",
            r#"{"address": "bogus"}"#,
            400,
            r#"{"path":"listeners[0].address","line":5,"column":18,"message":"#,
        ),
        (
            "PUT",
            "/config/routes/0/handler/module",
            "application/json",
            r#""nope""#,
            400,
            r#"there is no module \"nope\""#,
        ),
        (
            "DELETE",
            "/config/listeners",
            "",
            "",
            400,
            r#""path":"listeners""#,
        ),
        (
            "PUT",
            "/config/routes/0/bogus",
            "application/json",
            "1",
            400,
            r#""path":"routes[0].bogus","line":16,"column":7,"message":"unknown field""#,
        ),
        (
            "PUT",
            "/config/routes/0",
            "application/json",
            "{",
            400,
            "the body is not JSON",
        ),
        (
            "PUT",
            "/config",
            "application/json",
            "{",
            400,
            "the body is not JSON",
        ),
        (
            "PUT",
            "/config/nope/x",
            "application/json",
            "1",
            404,
            r#"there is no member \"nope\""#,
        ),
        (
            "PATCH",
            "/config/routes/5",
            "application/json",
            "{}",
            404,
            "index 5 is past the end",
        ),
        (
            "PATCH",
            "/config/routes/0",
            "text/plain",
            "{}",
            415,
            "application/merge-patch+json or application/json",
        ),
        (
            "DELETE",
            "/config",
            "",
            "",
            400,
            "the config itself cannot be deleted",
        ),
        (
            "DELETE",
            "/config/",
            "",
            "",
            400,
            "the config itself cannot be deleted",
        ),
        (
            "PUT",
            "/config/routes/0",
            "text/plain",
            "{}",
            415,
            "application/json",
        ),
        ("GET", "/config/a~2b", "", "", 400, "not a JSON pointer"),
        (
            "PUT",
            "/config/a~2b",
            "application/json",
            "1",
            400,
            "not a JSON pointer",
        ),
        (
            "POST",
            "/config",
            "application/json",
            "{}",
            405,
            "method not allowed",
        ),
        ("GET", "/rollback/1", "", "", 405, "method not allowed"),
        ("GET", "/reload", "", "", 405, "method not allowed"),
        ("POST", "/rollback/x", "", "", 404, "versions are numbers"),
        ("POST", "/rollback/7", "", "", 404, "version 7 is not kept"),
    ] {
        let r = f.send(method, path, ct, body, "");
        assert_eq!(r.status, status, "{method} {path} {body}: {r:?}");
        assert!(r.body.contains(want), "{method} {path} {body}: {}", r.body);
    }
    let v = f.configs.current();
    assert_eq!((v.number, v.data()), (1, doc("v1").as_str()));
    assert_eq!(f.served(), "v1");
    // A body too large is refused before it is read whole.
    let f = start(&doc("v1"), admin(|a| a.max_config_bytes = 64));
    let r = f.send(
        "PUT",
        "/config/routes/0/name",
        "application/json",
        &format!("\"{}\"", "x".repeat(100)),
        "",
    );
    assert_eq!(r.status, 413, "{r:?}");
}

/// fit's `TestIfMatch`: a stale ETag refuses every kind of change; `*` and a list name the
/// running config.
#[test]
fn if_match_every_change() {
    let f = start(&doc("v1"), admin(|_| {}));
    let etag = f.configs.current().etag.clone();
    let r = f.request_with("GET", "/config", &[("If-None-Match", &etag)]);
    assert_eq!(r.status, 304);
    let r = f.send(
        "PUT",
        "/config/routes/0/handler/params/body",
        "application/json",
        r#""v2""#,
        &etag,
    );
    assert_eq!(r.status, 200, "{r:?}");
    let next = r.header("ETag").unwrap().to_owned();
    assert!(next != etag && next == f.configs.current().etag);
    for (method, path, body) in [
        ("PUT", "/config/routes/0/handler/params/body", r#""v3""#),
        ("PATCH", "/config/routes/0", "{}"),
        ("DELETE", "/config/routes/0", ""),
        ("POST", "/load", &doc("v3")),
        ("POST", "/rollback/1", ""),
        ("POST", "/reload", ""),
        ("PUT", "/config?dry_run=1", &doc("v3")),
    ] {
        let r = f.send(method, path, "application/json", body, &etag);
        assert_eq!(r.status, 412, "{method} {path}: {r:?}");
        assert!(r.body.contains("If-Match"), "{}", r.body);
    }
    assert_eq!(
        f.send("POST", "/load", "application/json", &doc("v3"), "*")
            .status,
        200
    );
    let list = format!("\"other\", {}", f.configs.current().etag);
    assert_eq!(
        f.send("POST", "/load", "application/json", &doc("v4"), &list)
            .status,
        200
    );
    assert_eq!(f.configs.current().number, 4);
    assert_eq!(f.served(), "v4");
}

/// fit's `TestConcurrentWriters`: writers that read the config, change it and write it back with
/// If-Match each either apply their change on top of all the others or are told to read again;
/// writers without If-Match lose nothing either, since each change is made to the config before
/// it.
#[test]
fn concurrent_writers() {
    let f = Arc::new(start(
        &doc("v0"),
        admin(|a| {
            a.history = 100;
            a.max_conns = 64;
        }),
    ));
    const N: usize = 8;
    let conflicts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut threads = Vec::new();
    for i in 0..N {
        let (f1, conflicts) = (Arc::clone(&f), Arc::clone(&conflicts));
        threads.push(std::thread::spawn(move || {
            loop {
                let r = f1.get("/config");
                let etag = r.header("ETag").unwrap().to_owned();
                let current = akit_config::value::parse(&r.body).unwrap();
                let Some(Value::Array(mut routes)) = current.get("routes").cloned() else {
                    panic!("no routes: {}", r.body);
                };
                routes.push(
                    akit_config::value::parse(&format!(
                        r#"{{"name": "a{i}", "priority": -1, "handler": {{"module": "respond"}}}}"#
                    ))
                    .unwrap(),
                );
                let body = Value::Array(routes).to_json();
                let r = f1.send("PUT", "/config/routes", "application/json", &body, &etag);
                match r.status {
                    200 => return,
                    412 => {
                        conflicts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    _ => panic!("writer {i}: {r:?}"),
                }
            }
        }));
        let f2 = Arc::clone(&f);
        threads.push(std::thread::spawn(move || {
            let r = f2.send(
                "PUT",
                "/config/routes/-",
                "application/json",
                &format!(
                    r#"{{"name": "b{i}", "priority": -1, "handler": {{"module": "respond"}}}}"#
                ),
                "",
            );
            assert_eq!(r.status, 200, "appender {i}: {r:?}");
        }));
    }
    for t in threads {
        t.join().unwrap();
    }
    let current = f.get("/config").body;
    for i in 0..N {
        for name in [format!("a{i}"), format!("b{i}")] {
            assert_eq!(
                current.matches(&format!("\"name\": \"{name}\"")).count(),
                1,
                "route {name} is not in the config once: {current}"
            );
        }
    }
    assert_eq!(f.configs.current().number, 2 * N as u64 + 1);
    eprintln!(
        "{} conflicts",
        conflicts.load(std::sync::atomic::Ordering::Relaxed)
    );
}

/// fit's `TestRollback`: an earlier version's config applied again as a new version, while it is
/// kept.
#[test]
fn rollback() {
    let f = start(&doc("v1"), admin(|a| a.history = 3));
    for body in ["v2", "v3"] {
        assert_eq!(f.load(&doc(body)).status, 200);
    }
    let r = f.send("POST", "/rollback/1", "", "", "");
    assert_eq!(
        (r.status, r.header(VERSION_HEADER)),
        (200, Some("4")),
        "{r:?}"
    );
    assert_eq!(f.served(), "v1");
    let cur = f.configs.current();
    // Version 1 has left the history of three, but its config is back.
    assert!(f.configs.version(1).is_none());
    assert_eq!(
        (cur.data(), &*cur.source),
        (doc("v1").as_str(), SOURCE_ROLLBACK)
    );
    // Its ETag is version 1's again, now the last of those kept.
    assert_eq!(cur.etag, f.configs.versions()[2].etag);
    assert_eq!(cur.etag, etag_of(doc("v1").as_bytes()));
    assert!(f.configs.version(2).is_some());
    let r = f.send("POST", "/rollback/1", "", "", "");
    assert_eq!(r.status, 404);
    assert!(r.body.contains("version 1 is not kept"), "{}", r.body);
    // A rollback to the version it follows.
    let r = f.send("POST", "/rollback/3", "", "", &cur.etag);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(f.served(), "v3");
    let vs = f.get("/versions").body;
    let v = akit_config::value::parse(&vs).unwrap();
    let Some(Value::Array(kept)) = v.get("versions") else {
        panic!("{vs}")
    };
    assert_eq!(kept.len(), 3, "{vs}");
    let last = &kept[2];
    assert_eq!(
        last.get("version"),
        Some(&Value::Number("5".into())),
        "{vs}"
    );
    assert_eq!(
        last.get("source"),
        Some(&Value::String(SOURCE_ROLLBACK.into())),
        "{vs}"
    );
}

/// fit's `TestReload`: `POST /reload` applies the running config again as a new version; a stale
/// If-Match is refused, and a dry run applies nothing.
#[test]
fn reload() {
    let f = start(&doc("v1"), admin(|_| {}));
    let cur = f.configs.current();
    let r = f.send("POST", "/reload", "", "", "");
    assert_eq!(
        (r.status, r.header(VERSION_HEADER)),
        (200, Some("2")),
        "{r:?}"
    );
    assert!(r.body.contains(r#""version":2"#), "{}", r.body);
    assert_eq!(f.served(), "v1");
    let now = f.configs.current();
    assert_eq!(
        (now.etag.as_str(), &*now.source),
        (cur.etag.as_str(), SOURCE_ADMIN)
    );
    assert_eq!(f.send("POST", "/reload", "", "", "\"stale\"").status, 412);
    let r = f.send("POST", "/reload?dry_run=1", "", "", &now.etag);
    assert_eq!(
        (r.status, r.header(DRY_RUN_HEADER)),
        (200, Some("1")),
        "{r:?}"
    );
    assert!(r.body.contains(r#""unchanged":true"#), "{}", r.body);
    assert_eq!(f.configs.current().number, 2);
    assert_eq!(f.send("POST", "/reload?dry_run=x", "", "", "").status, 400);
}

/// fit's `TestDryRun`: `?dry_run=1` checks a change through the whole apply path and answers as
/// an apply would, with the same errors, leaving the version, the config and the listeners as
/// they were.
#[test]
fn dry_run() {
    let f = start(&doc("one"), admin(|_| {}));
    let v = f.configs.current();
    let unchanged = |what: &str| {
        let cur = f.configs.current();
        assert_eq!((cur.number, f.configs.versions().len()), (1, 1), "{what}");
        assert_eq!(f.served(), "one", "{what}");
    };

    // A whole config that loads: answered with the running version and ETag, the ETag it would
    // have, and the listener it would add.
    let two = r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}, {"name": "more", "address": "127.0.0.1:0"}],
  "routes": [{"handler": {"module": "respond", "params": {"body": "two"}}}], "admin": {}}"#;
    let r = f.send("PUT", "/config?dry_run=1", "application/json", two, &v.etag);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.header(DRY_RUN_HEADER), Some("1"));
    assert_eq!(r.header("ETag"), Some(v.etag.as_str()));
    assert_eq!(r.header(VERSION_HEADER), Some("1"));
    let a = akit_config::value::parse(&r.body).unwrap();
    assert_eq!(a.get("dry_run"), Some(&Value::Bool(true)), "{}", r.body);
    assert_eq!(a.get("version"), Some(&Value::Number("1".into())));
    assert_eq!(a.get("etag"), Some(&Value::String(v.etag.clone())));
    let Some(Value::String(new_etag)) = a.get("new_etag") else {
        panic!("{}", r.body)
    };
    assert_ne!(new_etag, &v.etag);
    assert!(a.get("unchanged").is_none(), "{}", r.body);
    assert!(
        r.body
            .contains(r#""listeners_added":[{"name":"more","address":"127.0.0.1:0","#),
        "{}",
        r.body
    );
    assert!(!r.body.contains("listeners_removed"), "{}", r.body);
    assert!(r.body.contains("admin changed"), "{}", r.body);
    unchanged("PUT");

    // The running config itself.
    let r = f.send(
        "POST",
        "/load?dry_run=true",
        "application/json",
        &doc("one"),
        "",
    );
    assert!(r.body.contains(r#""unchanged":true"#), "{}", r.body);
    assert!(
        r.body
            .contains(&format!(r#""new_etag":"{}""#, v.etag.replace('"', "\\\""))),
        "{}",
        r.body
    );

    // A config that does not load, or does not compile, is refused as an apply refuses it, word
    // for word. (fit's first case, `{"listeners": []}`, is left out: akit-config takes a config
    // without listeners.)
    for bad in [
        r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}], "routes": [{"handler": {"module": "nope"}}]}"#,
        r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}], "routes": [{"handler": {"module": "respond", "params": {"status": "x"}}}]}"#,
        r#"{"listeners": ["#,
    ] {
        let dry = f.send("PUT", "/config?dry_run=1", "application/json", bad, "");
        unchanged("a bad config, checked");
        let applied = f.send("PUT", "/config", "application/json", bad, "");
        assert_eq!(dry.status, 400, "{bad}: {dry:?}");
        assert_eq!(
            (dry.status, &dry.body),
            (applied.status, &applied.body),
            "{bad}"
        );
        assert_eq!(dry.header(DRY_RUN_HEADER), None);
    }
    // An address another socket holds: refused as the apply's bind is.
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let clash = format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}, {{"name": "clash", "address": "{}"}}],
  "routes": [{{"handler": {{"module": "respond", "params": {{"body": "two"}}}}}}]}}"#,
        taken.local_addr().unwrap()
    );
    let dry = f.send("PUT", "/config?dry_run=1", "application/json", &clash, "");
    let applied = f.send("PUT", "/config", "application/json", &clash, "");
    assert_eq!(dry.status, 500, "{dry:?}");
    assert_eq!((dry.status, &dry.body), (applied.status, &applied.body));
    assert!(dry.body.contains("listener clash"), "{}", dry.body);
    drop(taken);
    unchanged("a taken address");
    // A dry_run that is neither yes nor no.
    for (method, path) in [
        ("PUT", "/config"),
        ("PATCH", "/config"),
        ("DELETE", "/config/routes/0"),
        ("POST", "/load"),
        ("POST", "/reload"),
    ] {
        let r = f.send(
            method,
            &format!("{path}?dry_run=maybe"),
            "application/json",
            "{}",
            "",
        );
        assert_eq!(r.status, 400, "{method} {path}: {r:?}");
        assert!(r.body.contains("dry_run"), "{}", r.body);
    }

    // PATCH, PUT and DELETE, checked and not applied.
    for (method, path, ct, body) in [
        (
            "PATCH",
            "/config/routes/0/handler/params?dry_run=1",
            "application/merge-patch+json",
            r#"{"body": "patched"}"#,
        ),
        (
            "PUT",
            "/config/routes/-?dry_run=1",
            "application/json",
            r#"{"handler": {"module": "respond"}}"#,
        ),
        ("DELETE", "/config/routes/0?dry_run=1", "", ""),
    ] {
        let r = f.send(method, path, ct, body, &v.etag);
        assert_eq!(
            (r.status, r.header(DRY_RUN_HEADER)),
            (200, Some("1")),
            "{method} {path}: {r:?}"
        );
        assert!(
            r.body.contains(r#""dry_run":true"#) && !r.body.contains("unchanged"),
            "{}",
            r.body
        );
        unchanged(path);
    }

    // Then the same change applied: the dry run's new ETag is the one it gets.
    let r = f.send(
        "PUT",
        "/config?dry_run=1",
        "application/json",
        &doc("three"),
        &v.etag,
    );
    let a = akit_config::value::parse(&r.body).unwrap();
    let Some(Value::String(new_etag)) = a.get("new_etag") else {
        panic!("{}", r.body)
    };
    let r = f.send("PUT", "/config", "application/json", &doc("three"), &v.etag);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.header("ETag"), Some(new_etag.as_str()));
    assert_eq!(f.served(), "three");
}

/// fit's `TestDryRunRemovesListener`: a dry run lists the listeners an apply would take away, and
/// frees nothing.
#[test]
fn dry_run_removes_listener() {
    let f = start(
        r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}, {"name": "old", "address": "127.0.0.1:0", "protocols": ["http/1.1"]}],
  "routes": [{"handler": {"module": "respond", "params": {"body": "one"}}}]}"#,
        admin(|_| {}),
    );
    let old = f.engine.listeners();
    let r = f.send(
        "PUT",
        "/config?dry_run=1",
        "application/json",
        &doc("one"),
        "",
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert!(
        r.body.contains(r#""listeners_removed":[{"name":"old","#),
        "{}",
        r.body
    );
    assert!(!r.body.contains("listeners_added"), "{}", r.body);
    assert_eq!(f.engine.listeners().len(), old.len());
}

/// fit's `TestToken`: with `token_file`, every request carries the token, and the Host field may
/// be anything; an empty token file is refused.
#[test]
fn token() {
    let file = std::env::temp_dir().join(format!("akit-admin-token-{}", std::process::id()));
    std::fs::write(&file, "s3cret\n").unwrap();
    let path = file.to_str().unwrap().to_owned();
    let f = start(&doc("v1"), admin(|a| a.token_file = Some(path.clone())));
    for (auth, status) in [
        ("", 401),
        ("Bearer wrong", 401),
        ("Basic s3cret", 401),
        ("Bearer s3cre", 401),
        ("Bearer s3cret", 200),
    ] {
        let mut h = vec![("Host", "akit.example.com")];
        if !auth.is_empty() {
            h.push(("Authorization", auth));
        }
        let r = request(f.api.addr(), "GET", "/config", &h, None);
        assert_eq!(r.status, status, "Authorization {auth:?}: {r:?}");
        if status == 401 {
            assert!(
                r.header("WWW-Authenticate")
                    .is_some_and(|v| v.starts_with("Bearer "))
            );
            assert!(
                r.body.contains("a valid bearer token is needed"),
                "{}",
                r.body
            );
        }
    }
    // A connection that asks without it is closed after the 401, as fit's: the request
    // pipelined behind it, with the token, is not served.
    let mut s = TcpStream::connect(f.api.addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(
        b"GET /config HTTP/1.1\r\nHost: localhost\r\n\r\n\
          GET /config HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer s3cret\r\n\r\n",
    )
    .unwrap();
    let r = read_response(&mut s);
    assert_eq!(r.status, 401, "{r:?}");
    assert_eq!(r.header("Connection"), Some("close"), "{r:?}");
    let mut rest = Vec::new();
    let _ = s.read_to_end(&mut rest);
    assert!(rest.is_empty(), "{}", String::from_utf8_lossy(&rest));
    // Every endpoint asks for it; with it, a change is applied.
    let r = request(f.api.addr(), "POST", "/rollback/1", &[], None);
    assert_eq!(r.status, 401);
    let r = request(
        f.api.addr(),
        "POST",
        "/load",
        &[
            ("Content-Type", "application/json"),
            ("Authorization", "Bearer s3cret"),
        ],
        Some(&doc("v2")),
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(f.served(), "v2");
    // A web page is still refused, token or not.
    let r = request(
        f.api.addr(),
        "GET",
        "/config",
        &[
            ("Authorization", "Bearer s3cret"),
            ("Origin", "https://evil.example.com"),
        ],
        None,
    );
    assert_eq!(r.status, 403);

    std::fs::write(&file, " \n").unwrap();
    let submit: akit_admin::Submit = Box::new(|_| Ok(()));
    let settings = admin(|a| a.token_file = Some(path.clone()));
    let e = Server::start(&settings, f.configs.clone(), submit, &Options::default()).unwrap_err();
    assert!(e.to_string().contains("is empty"), "{e}");
    std::fs::remove_file(&file).unwrap();
    let submit: akit_admin::Submit = Box::new(|_| Ok(()));
    let e = Server::start(&settings, f.configs.clone(), submit, &Options::default()).unwrap_err();
    assert!(e.to_string().contains("token_file"), "{e}");
}

/// I80b (the threat model's L2): a secret written inline is answered as `[redacted]` by every
/// read of the config, whole, by pointer, by version and as it runs; sent back as `[redacted]`
/// with a change, it is kept; where no secret is kept, `[redacted]` is refused.
#[test]
fn secrets_redacted() {
    let cfg = r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}],
  "routes": [{"handler": {"module": "respond", "params": {"body": "v1",
    "headers": {"Authorization": "Bearer s3cret-value", "X-Plain": "seen"}}}}]}"#;
    let f = start(cfg, admin(|_| {}));
    let headers = "/config/routes/0/handler/params/headers";
    for path in [
        "/config",
        "/versions/1",
        "/config/effective",
        headers,
        &format!("{headers}/Authorization"),
    ] {
        let r = f.get(path);
        assert_eq!(r.status, 200, "{path}: {r:?}");
        assert!(!r.body.contains("s3cret-value"), "{path}: {}", r.body);
        assert!(r.body.contains("[redacted]"), "{path}: {}", r.body);
    }
    assert!(f.get("/config").body.contains("\"seen\""));
    let site = |want: &str| {
        let r = request(f.web, "GET", "/", &[("Host", "x")], None);
        assert_eq!(r.header("Authorization"), Some(want), "{r:?}");
        r
    };
    site("Bearer s3cret-value");

    // What GET answered, sent back changed, keeps the secret.
    let got = f.get("/config");
    let etag = got.header("ETag").unwrap().to_owned();
    let r = f.load_with(
        &got.body.replace("\"v1\"", "\"v2\""),
        &[("If-Match", etag.as_str())],
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(site("Bearer s3cret-value").body, "v2");
    assert!(f.configs.current().data().contains("Bearer s3cret-value"));
    // So do a value put at a pointer and a merge patch.
    let r = f.send(
        "PUT",
        headers,
        "application/json",
        r#"{"Authorization": "[redacted]", "X-Plain": "again"}"#,
        "",
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(site("Bearer s3cret-value").header("X-Plain"), Some("again"));
    let r = f.send(
        "PATCH",
        "/config/routes/0/handler/params",
        "application/merge-patch+json",
        r#"{"headers": {"Authorization": "[redacted]"}, "body": "v3"}"#,
        "",
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(site("Bearer s3cret-value").body, "v3");
    // Where no secret is kept, it is refused, and nothing changes.
    for (method, path, body) in [
        ("PUT", format!("{headers}/Cookie"), "\"[redacted]\""),
        ("PATCH", headers.to_owned(), r#"{"Cookie": "[redacted]"}"#),
    ] {
        let r = f.send(method, &path, "application/json", body, "");
        assert_eq!(r.status, 400, "{method} {path}: {r:?}");
        assert!(r.body.contains("no secret is kept there"), "{}", r.body);
    }
    let r = f.load(&doc("[redacted]").replace("\"body\"", "\"token\""));
    assert_eq!(r.status, 400, "{r:?}");
    assert_eq!(site("Bearer s3cret-value").body, "v3");
    // A new secret replaces the old.
    let r = f.send(
        "PUT",
        &format!("{headers}/Authorization"),
        "application/json",
        "\"Bearer other\"",
        "",
    );
    assert_eq!(r.status, 200, "{r:?}");
    site("Bearer other");
}

/// An edit's body under, at and over `max_config_bytes`, and one cut off midway: only a whole
/// body within the limit is applied.
#[test]
fn edit_limits_and_cut_clients() {
    let at = doc("v2");
    let f = start(&doc("v1"), admin(|a| a.max_config_bytes = at.len()));
    let r = f.send("PUT", "/config", "application/json", &at, "");
    assert_eq!(r.status, 200, "at the limit: {r:?}");
    let r = f.send("PATCH", "/config", "application/json", "{}", "");
    assert_eq!(r.status, 200, "under the limit: {r:?}");
    let over = format!(" {at}");
    let r = f.send("PUT", "/config", "application/json", &over, "");
    assert_eq!(r.status, 413, "over the limit: {r:?}");
    assert_eq!(f.configs.current().number, 3);

    // Cut off midway: nothing is applied, and the API goes on.
    let mut s = TcpStream::connect(f.api.addr()).unwrap();
    write!(
        s,
        "PUT /config HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        at.len(),
        &at[..at.len() / 2]
    )
    .unwrap();
    s.shutdown(Shutdown::Both).unwrap();
    drop(s);
    let r = f.send(
        "PATCH",
        "/config/routes/0/handler/params",
        "application/json",
        r#"{"body": "v3"}"#,
        "",
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(f.served(), "v3");
    assert_eq!(f.configs.current().number, 4);
}

/// fit's `TestUnixSocket`: on `unix/<path>` the API listens on a socket made 0600, a stale one
/// left by a process that is gone replaced, `Host` not checked; one in use is refused, and the
/// file goes with the API.
#[test]
fn unix_socket() {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    // A short path: a socket's path is limited to about 100 bytes.
    let path = std::path::PathBuf::from(format!("/tmp/akit-admin-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    // A socket left by a process that is gone is replaced.
    drop(UnixListener::bind(&path).unwrap());
    let settings = admin(|a| a.address = format!("unix/{}", path.display()));
    let f = start(&doc("v1"), settings.clone());
    assert_eq!(f.api.address(), format!("unix/{}", path.display()));
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    // Over the socket, the Host field is not checked.
    let mut s = UnixStream::connect(&path).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(b"GET /config HTTP/1.1\r\nHost: fit.example.com\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let r = parse(&String::from_utf8_lossy(&raw));
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.body, doc("v1"));

    // One that is in use is not.
    let (submit, _rx) = {
        let (tx, rx) = mpsc::sync_channel::<Job>(1);
        let s: akit_admin::Submit = Box::new(move |job| {
            tx.try_send(job).map_err(|e| match e {
                mpsc::TrySendError::Full(j) | mpsc::TrySendError::Disconnected(j) => j,
            })
        });
        (s, rx)
    };
    let e = Server::start(&settings, f.configs.clone(), submit, &Options::default()).unwrap_err();
    assert!(e.to_string().contains("in use"), "{e}");
    assert!(path.exists(), "the socket in use is left alone");
    drop(f);
    assert!(!path.exists(), "the socket goes with the API");
}

/// The items of the Admin API v1 as frozen (tests/compat/v1/admin-api.txt) and marked since
/// (additions.txt): `added` ones in, `retired` ones out.
fn frozen_v1() -> BTreeSet<Vec<String>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/compat/v1");
    let items = |text: &str| -> Vec<Vec<String>> {
        text.lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| l.split('\t').map(str::to_owned).collect())
            .collect()
    };
    let mut out: BTreeSet<Vec<String>> =
        items(&std::fs::read_to_string(dir.join("admin-api.txt")).unwrap())
            .into_iter()
            .collect();
    for line in items(&std::fs::read_to_string(dir.join("additions.txt")).unwrap()) {
        match (
            line.first().map(String::as_str),
            line.get(1).map(String::as_str),
        ) {
            (Some("admin"), Some("added")) => {
                out.insert(line[2..].to_vec());
            }
            (Some("admin"), Some("retired")) => {
                out.remove(&line[2..]);
            }
            _ => {}
        }
    }
    out
}

/// Whether `path` is one of `pattern`'s: `{n}` a number, `{path...}` one segment or more.
fn path_matches(pattern: &str, path: &str) -> bool {
    let mut p = pattern.split('/');
    let mut c = path.split('/');
    loop {
        match (p.next(), c.next()) {
            (None, None) => return true,
            (Some("{path...}" | "{id...}"), Some(seg)) => return !seg.is_empty(),
            // I100's /tokens/{member}/{id}: one segment each.
            (Some("{member}" | "{id}" | "{name}"), Some(seg)) => {
                if seg.is_empty() {
                    return false;
                }
            }
            (Some("{n}"), Some(seg)) => {
                if seg.is_empty() || !seg.bytes().all(|b| b.is_ascii_digit()) {
                    return false;
                }
            }
            (Some(a), Some(b)) if a == b => {}
            _ => return false,
        }
    }
}

/// The members of `v`, as `a`, `a[].b`, under `prefix`.
fn members(prefix: &str, v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::Object(ms) => {
            for (k, v) in ms {
                let name = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                out.insert(name.clone());
                members(&name, v, out);
            }
        }
        Value::Array(items) => {
            for v in items {
                members(&format!("{prefix}[]"), v, out);
            }
        }
        _ => {}
    }
}

/// I79, the v1 freeze: the Admin API serves exactly what tests/compat/v1 holds (its v1 baseline
/// and the additions marked since): every endpoint frozen is served, no method or path past them
/// is, and the header fields, the query parameter and the members of the answers to a change,
/// to an error and to `GET /versions` are those frozen. A change not marked fails it: a
/// compatible addition is marked in additions.txt; anything else waits for a v2.
#[test]
fn v1_surface_is_frozen() {
    // The endpoints other crates serve through `Options::endpoints` (I157) are held to the file
    // by those crates' own tests: akit-bittorrent's `admin_surface_is_frozen` for
    // `/bittorrent/`.
    let frozen: BTreeSet<Vec<String>> = frozen_v1()
        .into_iter()
        .filter(|i| {
            !(matches!(i[0].as_str(), "endpoint" | "uncovered")
                && i.get(2).is_some_and(|p| p.starts_with("/bittorrent/")))
        })
        .collect();
    let mut seen: BTreeSet<Vec<String>> = BTreeSet::new();
    let item = |parts: &[&str]| parts.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();

    let mut opts = Options::default();
    opts.metrics = Some(Arc::new(|out: &mut Vec<u8>| {
        out.extend_from_slice(b"akit_config_version 1\n");
    }));
    // I141: the media libraries' endpoints, as akit run registers them (on an engine without
    // libraries).
    opts.endpoints.push((
        akit_media::library::admin::PREFIX,
        Arc::new(|ctx: &mut akio_http::Context| akit_media::library::admin::serve(usize::MAX, ctx)),
    ));
    let f = start_with(&doc("v1"), admin(|_| {}), opts);

    // Endpoints: every method on every path frozen, each path's numbers and pointers filled in.
    let patterns: BTreeSet<&str> = frozen
        .iter()
        .filter(|i| matches!(i[0].as_str(), "endpoint" | "uncovered"))
        .map(|i| i[2].as_str())
        .collect();
    for pattern in &patterns {
        let path = pattern
            .replace("{path...}", "listeners/0/name")
            .replace("{n}", "1")
            .replace("{member}", "tunnel.hub")
            .replace("{id}", "0123456789abcdef")
            .replace("{name}", "films")
            .replace("{id...}", "films/bbb");
        let mut get_served = false;
        for method in ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"] {
            let r = request(f.api.addr(), method, &path, &[], None);
            let served = match method {
                // A HEAD's 404 has no body to tell an endpoint's from a missing one.
                "HEAD" => get_served && r.status != 405,
                _ => {
                    !(r.status == 405
                        || r.status == 404 && r.body.contains("there is no such endpoint"))
                }
            };
            if method == "GET" {
                get_served = served;
            }
            if !served {
                continue;
            }
            let mut matched = false;
            for i in frozen.iter().filter(|i| i[1] == method) {
                if matches!(i[0].as_str(), "endpoint" | "uncovered") && path_matches(&i[2], &path) {
                    seen.insert(i.clone());
                    matched = true;
                }
            }
            if !matched {
                seen.insert(item(&["endpoint", method, pattern]));
            }
        }
    }
    // A path the dispatcher names is one frozen, or the stem of one.
    let src = include_str!("../src/server.rs");
    let call = src
        .split_once("fn serve(&self, ctx: &mut Context)")
        .and_then(|(_, rest)| rest.split_once("pub(crate) fn submit("))
        .map(|(body, _)| body)
        .unwrap();
    for lit in call.split('"').skip(1).step_by(2) {
        if lit.starts_with('/') {
            assert!(
                patterns
                    .iter()
                    .any(|p| *p == lit || lit.ends_with('/') && p.starts_with(lit)),
                "the Admin API serves {lit}, which tests/compat/v1 does not hold: mark it added"
            );
        }
    }

    // Header fields and the query parameter.
    let r = f.get("/config");
    let etag = r.header("ETag").unwrap().to_owned();
    if r.header(VERSION_HEADER).is_some() {
        seen.insert(item(&["header", "ETag"]));
        seen.insert(item(&["header", VERSION_HEADER]));
    }
    if f.request_with("GET", "/config", &[("If-None-Match", &etag)])
        .status
        == 304
    {
        seen.insert(item(&["header", "If-None-Match"]));
    }
    if f.load_with(&doc("v1"), &[("If-Match", "\"stale\"")]).status == 412 {
        seen.insert(item(&["header", "If-Match"]));
    }
    if request(f.api.addr(), "POST", "/metrics", &[], None)
        .header("Allow")
        .is_some()
    {
        seen.insert(item(&["header", "Allow"]));
    }
    let r = request(
        f.api.addr(),
        "POST",
        "/load?dry_run=1",
        &[("Content-Type", "application/json")],
        Some(&doc("v1")),
    );
    if r.status == 200 && r.header(DRY_RUN_HEADER) == Some("1") {
        seen.insert(item(&["query", "dry_run"]));
        seen.insert(item(&["header", DRY_RUN_HEADER]));
    }

    // The members of a change's answer: a dry run that changes nothing, listeners added with a
    // warning, then removed.
    let mut change = BTreeSet::new();
    members(
        "",
        &akit_config::value::parse(&r.body).unwrap(),
        &mut change,
    );
    let two = r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}, {"name": "extra", "address": "127.0.0.1:0"}],
  "admin": {"address": "127.0.0.1:1"}}"#;
    for config in [two, &doc("v3")] {
        let r = f.load(config);
        assert_eq!(r.status, 200, "{r:?}");
        members(
            "",
            &akit_config::value::parse(&r.body).unwrap(),
            &mut change,
        );
    }
    // An error's.
    let mut error = BTreeSet::new();
    let r = f.load(r#"{"listeners": 1}"#);
    assert_eq!(r.status, 400, "{r:?}");
    members("", &akit_config::value::parse(&r.body).unwrap(), &mut error);
    // GET /versions'.
    let mut versions = BTreeSet::new();
    members(
        "",
        &akit_config::value::parse(&f.get("/versions").body).unwrap(),
        &mut versions,
    );
    for (answer, keys) in [("change", change), ("error", error), ("versions", versions)] {
        for k in keys {
            seen.insert(item(&["member", answer, &k]));
        }
    }
    drop(f);

    // The bearer token.
    let file = std::env::temp_dir().join(format!("akit-admin-v1-token-{}", std::process::id()));
    std::fs::write(&file, "s3cret\n").unwrap();
    let path = file.to_str().unwrap().to_owned();
    let f = start(&doc("v1"), admin(|a| a.token_file = Some(path.clone())));
    if f.get("/config").header("WWW-Authenticate").is_some() {
        seen.insert(item(&["header", "WWW-Authenticate"]));
    }
    if f.request_with("GET", "/config", &[("Authorization", "Bearer s3cret")])
        .status
        == 200
    {
        seen.insert(item(&["header", "Authorization"]));
    }
    drop(f);
    let _ = std::fs::remove_file(&file);

    let new: Vec<String> = seen.difference(&frozen).map(|i| i.join("\t")).collect();
    let gone: Vec<String> = frozen.difference(&seen).map(|i| i.join("\t")).collect();
    assert!(
        new.is_empty() && gone.is_empty(),
        "the Admin API differs from its v1 surface (docs/compatibility.md).\n\
         Not marked (a compatible addition is marked in tests/compat/v1/additions.txt as \
         `admin<TAB>added<TAB><item>`):\n  {}\n\
         Gone, which breaks v1:\n  {}",
        new.join("\n  "),
        gone.join("\n  ")
    );
}

/// `admin.access` follows every config applied (I87): a change to it alone is not one that takes
/// a restart; a change to the rest of `admin` still is.
#[test]
fn admin_access_takes_no_restart() {
    let first = akit_config::Admin::default();
    let with_access = akit_config::Admin {
        access: Some(akit_config::Access {
            deny: vec!["192.0.2.1".into()],
            ..akit_config::Access::default()
        }),
        ..akit_config::Admin::default()
    };
    assert_eq!(
        akit_admin::admin_warning(Some(&first), Some(&with_access)),
        None
    );
    let moved = akit_config::Admin {
        history: 3,
        ..with_access
    };
    assert!(akit_admin::admin_warning(Some(&first), Some(&moved)).is_some());
}

// I91: the config as text (fit's admin/text_test.go and transfer_test.go, ported).

const TEXT_FRAGMENT: &str = r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}],
  "routes": [{"name": "r1", "handler": {"module": "respond", "params": {"body": "v1"}}}],
  "upstreams": [{"name": "app", "addresses": ["127.0.0.1:9000"]}],
  "log": {"level": "debug"}}"#;

fn same_json(what: &str, want: &str, got: &str) {
    let (w, g) = (
        akit_config::value::parse(want).unwrap(),
        akit_config::value::parse(got).unwrap_or_else(|e| panic!("{what}: {got:?}: {e:?}")),
    );
    assert_eq!(w, g, "{what}: {got}");
}

/// The first error of a refused text: its path, line and message.
fn first_error(body: &str) -> (String, u64, String) {
    let v = akit_config::value::parse(body).unwrap();
    let Some(Value::Array(errs)) = v.get("errors") else {
        panic!("{body}")
    };
    let e = &errs[0];
    let s = |k: &str| match e.get(k) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    let line = match e.get("line") {
        Some(Value::Number(n)) => n.parse().unwrap(),
        _ => 0,
    };
    (s("path"), line, s("message"))
}

fn member(f: &Fixture, name: &str) -> Option<String> {
    let v = akit_config::value::parse(&f.get("/config").body).unwrap();
    v.get(name).map(Value::to_json)
}

#[test]
fn text_render_parse() {
    let f = start(&doc("v1"), admin(|_| {}));
    for (format, ct) in [
        ("conf", "text/plain"),
        ("yaml", "application/yaml"),
        ("json", "application/json"),
    ] {
        let r = f.send(
            "POST",
            &format!("/config/render?format={format}"),
            "application/json",
            TEXT_FRAGMENT,
            "",
        );
        assert_eq!(r.status, 200, "{format}: {r:?}");
        assert_eq!(r.header("Content-Type"), Some("text/plain; charset=utf-8"));
        let p = f.send(
            "POST",
            &format!("/config/parse?format={format}"),
            ct,
            &r.body,
            "",
        );
        assert_eq!(p.status, 200, "{format}: {p:?}");
        assert_eq!(p.header("Content-Type"), Some("application/json"));
        same_json(
            &format!("the fragment after {format}"),
            TEXT_FRAGMENT,
            &p.body,
        );
        // The empty config.
        let r = f.send(
            "POST",
            &format!("/config/render?format={format}"),
            "application/json",
            "{}",
            "",
        );
        assert_eq!(r.status, 200);
        let p = f.send(
            "POST",
            &format!("/config/parse?format={format}"),
            "text/plain",
            &r.body,
            "",
        );
        assert_eq!(p.body.trim(), "{}", "{format}");
    }
    for (query, body, want) in [
        ("", TEXT_FRAGMENT, "conf, yaml or json"),
        ("format=toml", TEXT_FRAGMENT, "conf, yaml or json"),
        ("format=json", "[1]", "a config is a JSON object"),
        ("format=conf", "{", "not a config"),
    ] {
        let r = f.send(
            "POST",
            &format!("/config/render?{query}"),
            "application/json",
            body,
            "",
        );
        assert_eq!(r.status, 400, "{query} {body}");
        assert!(r.body.contains(want), "{query}: {}", r.body);
    }
}

#[test]
fn text_parse_errors() {
    let f = start(&doc("v1"), admin(|_| {}));
    for (format, body) in [
        ("conf", "listener web {\n\taddress :8080\n"),
        ("json", r#"{"listeners": [}"#),
    ] {
        let r = f.send(
            "POST",
            &format!("/config/parse?format={format}"),
            "text/plain",
            body,
            "",
        );
        assert_eq!(r.status, 400, "{format}: {r:?}");
        assert!(
            r.body.contains(r#""error":"the text does not parse""#),
            "{}",
            r.body
        );
        assert!(r.body.contains(r#""line":"#), "{}", r.body);
    }
    // Comments and trailing commas, as fit's JSON takes them.
    let r = f.send(
        "POST",
        "/config/parse?format=json",
        "text/plain",
        "{\n  \"listeners\": [ // the web listener\n    {\"name\": \"web\", \"address\": \"127.0.0.1:0\"},\n  ],\n}\n",
        "",
    );
    assert_eq!(r.status, 200, "{r:?}");
    let r = f.send(
        "POST",
        "/config/parse?format=yaml",
        "text/plain",
        "- a\n- b\n",
        "",
    );
    assert_eq!(r.status, 400);
    assert!(r.body.contains("not a config object"), "{}", r.body);
}

#[test]
fn text_part_save() {
    let f = start(&doc("v1"), admin(|_| {}));
    let etag = f.configs.current().etag.clone();
    let r = f.send(
        "PUT",
        "/config/text?format=conf&members=listeners",
        "text/plain",
        "listener web {\n\taddress 127.0.0.1:0\n\tprotocols http/1.1\n}\n",
        &etag,
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.header(VERSION_HEADER), Some("2"));
    same_json(
        "listeners",
        r#"[{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1"]}]"#,
        &member(&f, "listeners").unwrap(),
    );
    same_json(
        "routes kept",
        r#"[{"handler": {"module": "respond", "params": {"body": "v1"}}}]"#,
        &member(&f, "routes").unwrap(),
    );
    let r = f.send(
        "PUT",
        "/config/text?format=yaml&members=upstreams",
        "text/plain",
        "upstreams:\n  - name: app\n    addresses:\n      - 127.0.0.1:9000\n",
        "",
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert!(member(&f, "upstreams").is_some());
    // A member the text leaves out is removed.
    let r = f.send(
        "PUT",
        "/config/text?format=yaml&members=upstreams",
        "text/plain",
        "# no upstreams\n",
        "",
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(member(&f, "upstreams"), None);
    let r = f.send(
        "PUT",
        "/config/text?format=json&members=routes",
        "text/plain",
        r#"{"routes": [{"name": "r2", "handler": {"module": "respond", "params": {"body": "v2"}}}]}"#,
        &f.configs.current().etag,
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(f.served(), "v2");
    let before = f.configs.current().number;
    let r = f.send(
        "PUT",
        "/config/text?format=conf&members=listeners",
        "text/plain",
        "listener web 127.0.0.1:0\nroute r3 { handler respond { body v9 } }\n",
        "",
    );
    assert_eq!(r.status, 400);
    assert!(
        r.body
            .contains(r#"the text sets \"routes\", which this page does not edit"#),
        "{}",
        r.body
    );
    let r = f.send(
        "PUT",
        "/config/text?format=json&members=admin,listeners",
        "text/plain",
        r#"{"listeners": []}"#,
        "",
    );
    assert_eq!(r.status, 400);
    assert!(r.body.contains("admin cannot be edited this way"));
    // An invalid listener: its error placed in the text.
    let r = f.send(
        "PUT",
        "/config/text?format=conf&members=listeners",
        "text/plain",
        "listener web {\n\taddress bogus\n}\n",
        "",
    );
    assert_eq!(r.status, 400, "{r:?}");
    assert!(
        r.body
            .contains("the config was not applied; the running config stays")
    );
    let (path, line, _) = first_error(&r.body);
    assert_eq!(
        (path.as_str(), line),
        ("listeners[0].address", 2),
        "{}",
        r.body
    );
    assert_eq!(f.configs.current().number, before);
    assert_eq!(f.served(), "v2");
    let r = f.send(
        "PUT",
        "/config/text?format=json&members=listeners",
        "text/plain",
        r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}]}"#,
        "\"stale\"",
    );
    assert_eq!(r.status, 412, "{r:?}");
    for query in ["", "format=toml"] {
        let r = f.send(
            "PUT",
            &format!("/config/text?{query}"),
            "text/plain",
            "{}",
            "",
        );
        assert_eq!(r.status, 400);
        assert!(r.body.contains("conf, yaml or json"));
    }
    // A text is never a store's file: that is an import's (I93b).
    let r = f.send("PUT", "/config/text?format=store", "text/plain", "{}", "");
    assert_eq!(r.status, 400);
    assert!(r.body.contains("conf, yaml or json"), "{}", r.body);
}

#[test]
fn text_whole_config() {
    let f = start(&doc("v1"), admin(|_| {}));
    let r = f.send(
        "PUT",
        "/config/text?format=conf",
        "text/plain",
        "listener web {\n\taddress bogus\n}\n\nroute r1 { handler respond { body v1 } }\n",
        "",
    );
    assert_eq!(r.status, 400, "{r:?}");
    let (path, line, _) = first_error(&r.body);
    assert_eq!(
        (path.as_str(), line),
        ("listeners[0].address", 2),
        "{}",
        r.body
    );
    assert_eq!(f.served(), "v1");
    let r = f.send(
        "PUT",
        "/config/text?format=conf",
        "text/plain",
        "listener web 127.0.0.1:0\n\nroute r1 {\n\thandler respond {\n\t\tbody v3\n\t}\n}\n",
        "",
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(f.served(), "v3");
    same_json(
        "the whole config",
        r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}], "routes": [{"name": "r1", "handler": {"module": "respond", "params": {"body": "v3"}}}]}"#,
        &f.get("/config").body,
    );
    // A dry run checks, and applies nothing.
    let r = f.send(
        "PUT",
        "/config/text?format=json&dry_run=1",
        "text/plain",
        &doc("v4"),
        "",
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(f.served(), "v3");
}

#[test]
fn export_and_import() {
    let f = start(&doc("v1"), admin(|_| {}));
    for format in ["conf", "yaml", "json"] {
        let r = f.send(
            "POST",
            "/config/export",
            "application/json",
            &format!(r#"{{"format": "{format}"}}"#),
            "",
        );
        assert_eq!(r.status, 200, "{format}: {r:?}");
        let cd = r.header("Content-Disposition").unwrap();
        assert!(
            cd.starts_with(r#"attachment; filename="akit-config-v"#)
                && cd.ends_with(&format!(".{format}\"")),
            "{cd}"
        );
        // Imported back: the same config, as a new version.
        let i = f.send(
            "POST",
            &format!("/config/import?format={format}"),
            "text/plain",
            &r.body,
            "",
        );
        assert_eq!(i.status, 200, "{format}: {i:?}");
        assert_eq!(f.served(), "v1");
    }
    let r = f.send(
        "POST",
        "/config/export",
        "application/json",
        r#"{"format": "json", "members": ["routes"]}"#,
        "",
    );
    assert_eq!(r.status, 200);
    assert!(r.body.contains("\"routes\"") && !r.body.contains("\"listeners\""));
    assert!(
        r.header("Content-Disposition")
            .unwrap()
            .contains("akit-routes-v")
    );
    let r = f.send(
        "POST",
        "/config/import?format=json&members=routes",
        "text/plain",
        r#"{"routes": [{"handler": {"module": "respond", "params": {"body": "v5"}}}]}"#,
        "",
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(f.served(), "v5");
    for (body, want) in [
        (r#"{"format": "toml"}"#, "conf, yaml, json or store"),
        // Without a store, a store's export needs a password of its own (I93b).
        (
            r#"{"format": "store"}"#,
            "give the password to seal the export with",
        ),
        (
            r#"{"format": "json", "members": ["admin"]}"#,
            "admin is not exported",
        ),
        ("[", "the body must be"),
    ] {
        let r = f.send("POST", "/config/export", "application/json", body, "");
        assert_eq!(r.status, 400, "{body}: {r:?}");
        assert!(r.body.contains(want), "{body}: {}", r.body);
    }
}

// I92: the Admin API over TLS (fit's admin/tls_test.go: TestTLS, TestTLSClientCA,
// TestTLSStartErrors).

struct Pki {
    dir: std::path::PathBuf,
    ca: akio_x509::Certificate,
    ca_key: akio_crypto::pkey::PrivateKey,
}

fn new_key() -> akio_crypto::pkey::PrivateKey {
    akio_crypto::pkey::PrivateKey::P256(akio_crypto::p256::SecretKey::generate().unwrap())
}

impl Pki {
    fn new(name: &str) -> Pki {
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("admin-tls")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ca_key = new_key();
        let ca = akio_x509::CertificateParams::ca("admin test ca")
            .self_signed(&ca_key)
            .unwrap();
        std::fs::write(dir.join("ca.pem"), ca.to_pem()).unwrap();
        Pki { dir, ca, ca_key }
    }

    /// A leaf the CA signs for `names`: its certificate and key files.
    fn leaf(&self, stem: &str, names: &[&str]) -> (String, String) {
        let key = new_key();
        let cert = akio_x509::CertificateParams::new(names)
            .signed_by(&key.public_key(), &self.ca, &self.ca_key)
            .unwrap();
        let (c, k) = (
            self.dir.join(format!("{stem}.pem")),
            self.dir.join(format!("{stem}.key")),
        );
        std::fs::write(&c, cert.to_pem()).unwrap();
        std::fs::write(&k, key.to_pkcs8_pem()).unwrap();
        (
            c.to_string_lossy().into_owned(),
            k.to_string_lossy().into_owned(),
        )
    }

    fn client(&self, cert: Option<(&str, &str)>) -> akio::tls::ClientConfig {
        let mut roots = akio::tls::RootCerts::empty();
        roots
            .add_pem(&std::fs::read(self.dir.join("ca.pem")).unwrap())
            .unwrap();
        let mut b = akio::tls::ClientConfig::builder().roots(roots);
        if let Some((c, k)) = cert {
            b = b.client_cert(
                akio::tls::Certificate::from_pem(&std::fs::read(c).unwrap()).unwrap(),
                akio::tls::PrivateKey::from_pem(&std::fs::read(k).unwrap()).unwrap(),
            );
        }
        b.build().unwrap()
    }
}

/// A GET over TLS: the status line, or why the exchange failed.
fn tls_get(addr: SocketAddr, cfg: &akio::tls::ClientConfig, path: &str) -> Result<String, String> {
    let tcp = TcpStream::connect(addr).map_err(|e| e.to_string())?;
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut s = akio::tls::stream::Stream::client(cfg, "admin.akit.test", tcp)
        .map_err(|e| e.to_string())?;
    s.handshake().map_err(|e| e.to_string())?;
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: admin.akit.test\r\nAuthorization: Bearer t0ken\r\nConnection: close\r\n\r\n"
    )
    .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    s.read_to_end(&mut out).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out).into_owned();
    Ok(text.lines().next().unwrap_or("").to_owned())
}

#[test]
fn tls() {
    let pki = Pki::new("tls");
    let (cert, key) = pki.leaf("api", &["admin.akit.test"]);
    let token = pki.dir.join("token");
    std::fs::write(&token, "t0ken\n").unwrap();
    let f = start(
        &doc("v1"),
        admin(|a| {
            a.address = "0.0.0.0:0".into();
            a.cert_file = Some(cert.clone());
            a.key_file = Some(key.clone());
            a.token_file = Some(token.to_string_lossy().into_owned());
        }),
    );
    let addr = SocketAddr::from(([127, 0, 0, 1], f.api.addr().port()));
    assert_eq!(
        tls_get(addr, &pki.client(None), "/versions").unwrap(),
        "HTTP/1.1 200 OK"
    );
    // In the clear, nothing is answered.
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let _ = s.write_all(b"GET /versions HTTP/1.1\r\nHost: x\r\n\r\n");
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf);
    assert!(
        !buf.starts_with(b"HTTP/1.1 200"),
        "{}",
        String::from_utf8_lossy(&buf)
    );
}

#[test]
fn tls_client_ca() {
    let pki = Pki::new("client-ca");
    let (cert, key) = pki.leaf("api", &["admin.akit.test"]);
    let (ccert, ckey) = pki.leaf("client", &["client.akit.test"]);
    let token = pki.dir.join("token");
    std::fs::write(&token, "t0ken").unwrap();
    let f = start(
        &doc("v1"),
        admin(|a| {
            a.cert_file = Some(cert.clone());
            a.key_file = Some(key.clone());
            a.client_ca_file = Some(pki.dir.join("ca.pem").to_string_lossy().into_owned());
            a.token_file = Some(token.to_string_lossy().into_owned());
        }),
    );
    let addr = f.api.addr();
    assert!(
        tls_get(addr, &pki.client(None), "/versions").is_err(),
        "a client without a certificate was heard"
    );
    assert_eq!(
        tls_get(addr, &pki.client(Some((&ccert, &ckey))), "/versions").unwrap(),
        "HTTP/1.1 200 OK"
    );
    // A certificate another CA signed is refused too.
    let other = Pki::new("client-ca-other");
    let (ocert, okey) = other.leaf("client", &["client.akit.test"]);
    assert!(tls_get(addr, &pki.client(Some((&ocert, &okey))), "/versions").is_err());
}

#[test]
fn tls_start_errors() {
    let pki = Pki::new("start-errors");
    let (cert, key) = pki.leaf("api", &["admin.akit.test"]);
    for (c, k, ca) in [
        ("missing.pem".to_owned(), key.clone(), None),
        (cert.clone(), "missing.key".to_owned(), None),
        (key.clone(), key.clone(), None),
        (cert.clone(), key.clone(), Some("missing-ca.pem".to_owned())),
    ] {
        let settings = admin(|a| {
            a.cert_file = Some(c.clone());
            a.key_file = Some(k.clone());
            a.client_ca_file = ca.clone();
        });
        let configs = Configs::new(doc("v1"), SOURCE_FILE, 2);
        let submit: akit_admin::Submit = Box::new(|_| Ok(()));
        let mut opts = Options::default();
        opts.backend = Some(backend());
        let e = Server::start(&settings, configs, submit, &opts).unwrap_err();
        assert!(e.to_string().starts_with("admin: "), "{c} {k}: {e}");
    }
}

// I94: the certificate files read again when they change (fit's admin/tls_test.go:
// TestTLSReload).

/// The leaf a TLS server at `addr` presents to a new connection, as DER: a client of its own each
/// time, whose handshake resumes no session (a resumed one is sent no certificate).
fn served_leaf(addr: SocketAddr, cfg: &akio::tls::ClientConfig) -> Vec<u8> {
    let tcp = TcpStream::connect(addr).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    tcp.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut s = akio::tls::stream::Stream::client(cfg, "admin.akit.test", tcp).unwrap();
    s.handshake().unwrap();
    s.conn().peer_certificates()[0].clone()
}

/// The first certificate of the PEM file `path`, as DER.
fn leaf_der(path: &str) -> Vec<u8> {
    akio_x509::Certificate::from_pem(&std::fs::read(path).unwrap()).unwrap()[0]
        .as_der()
        .to_vec()
}

/// Copies `from` over `to` and dates it `secs` from now: a file system with coarse modification
/// times may not tell the change otherwise, and the certificates' sizes may be the same.
fn replace(from: &str, to: &str, secs: u64) {
    std::fs::copy(from, to).unwrap();
    std::fs::File::options()
        .write(true)
        .open(to)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(secs))
        .unwrap();
}

#[test]
fn tls_reload() {
    let pki = Pki::new("reload");
    let (cert, key) = pki.leaf("api", &["admin.akit.test"]);
    let token = pki.dir.join("token");
    std::fs::write(&token, "t0ken").unwrap();
    let f = start(
        &doc("v1"),
        admin(|a| {
            a.cert_file = Some(cert.clone());
            a.key_file = Some(key.clone());
            a.token_file = Some(token.to_string_lossy().into_owned());
        }),
    );
    let addr = f.api.addr();
    let client = pki.client(None);
    let first = served_leaf(addr, &pki.client(None));
    assert_eq!(first, leaf_der(&cert));
    // Nothing changed: nothing read again.
    assert!(f.api.reload_certs().is_empty());

    // Renewed outside akit, and read again (SIGHUP): new connections get it.
    let (renewed, renewed_key) = pki.leaf("renewed", &["admin.akit.test"]);
    replace(&renewed, &cert, 10);
    replace(&renewed_key, &key, 10);
    assert_eq!(
        f.api.reload_certs(),
        vec![akit_admin::CertReload::Reloaded("admin")]
    );
    let got = served_leaf(addr, &pki.client(None));
    assert_eq!(got, leaf_der(&renewed));
    assert_ne!(got, first);
    assert_eq!(
        tls_get(addr, &client, "/versions").unwrap(),
        "HTTP/1.1 200 OK"
    );

    // A key that does not load, through the watcher (-watch-certs): the renewed certificate
    // stays, and the warning says why; the same files are not tried again.
    std::fs::write(&key, "not a key").unwrap();
    let now = Instant::now();
    assert!(f.api.watch_certs(now).is_empty(), "the first look settles");
    let r = f.api.watch_certs(now + Duration::from_secs(1));
    match r.as_slice() {
        [akit_admin::CertReload::Failed(e)] => {
            assert!(e.starts_with("admin: "), "{e}");
            assert!(e.contains("api.key"), "{e}");
            assert!(e.ends_with("; the running certificate stays"), "{e}");
        }
        r => panic!("{r:?}"),
    }
    assert_eq!(served_leaf(addr, &pki.client(None)), leaf_der(&renewed));
    assert!(f.api.watch_certs(now + Duration::from_secs(2)).is_empty());
    assert_eq!(served_leaf(addr, &pki.client(None)), leaf_der(&renewed));

    // Mended: loaded once the files have settled for the debounce time.
    let (third, third_key) = pki.leaf("third", &["admin.akit.test"]);
    replace(&third, &cert, 20);
    replace(&third_key, &key, 20);
    assert!(f.api.watch_certs(now + Duration::from_secs(3)).is_empty());
    assert!(
        f.api
            .watch_certs(now + Duration::from_millis(3200))
            .is_empty(),
        "loaded before it settled"
    );
    assert_eq!(
        f.api.watch_certs(now + Duration::from_secs(4)),
        vec![akit_admin::CertReload::Reloaded("admin")]
    );
    assert_eq!(served_leaf(addr, &pki.client(None)), leaf_der(&third));
    assert!(f.api.watch_certs(now + Duration::from_secs(5)).is_empty());
}

#[test]
fn tls_reload_client_ca() {
    let pki = Pki::new("reload-client-ca");
    let other = Pki::new("reload-client-ca-other");
    let (cert, key) = pki.leaf("api", &["admin.akit.test"]);
    let (ccert, ckey) = pki.leaf("client", &["client.akit.test"]);
    let (ocert, okey) = other.leaf("client", &["client.akit.test"]);
    let token = pki.dir.join("token");
    std::fs::write(&token, "t0ken").unwrap();
    let ca = pki.dir.join("clients.pem");
    std::fs::copy(pki.dir.join("ca.pem"), &ca).unwrap();
    let ca = ca.to_string_lossy().into_owned();
    let f = start(
        &doc("v1"),
        admin(|a| {
            a.cert_file = Some(cert.clone());
            a.key_file = Some(key.clone());
            a.client_ca_file = Some(ca.clone());
            a.token_file = Some(token.to_string_lossy().into_owned());
        }),
    );
    let addr = f.api.addr();
    let ours = pki.client(Some((&ccert, &ckey)));
    let theirs = pki.client(Some((&ocert, &okey)));
    assert_eq!(
        tls_get(addr, &ours, "/versions").unwrap(),
        "HTTP/1.1 200 OK"
    );
    assert!(tls_get(addr, &theirs, "/versions").is_err());

    // The client CA replaced: the other CA's clients are heard, and ours no more.
    replace(&other.dir.join("ca.pem").to_string_lossy(), &ca, 10);
    assert_eq!(
        f.api.reload_certs(),
        vec![akit_admin::CertReload::Reloaded("admin")]
    );
    assert_eq!(
        tls_get(addr, &theirs, "/versions").unwrap(),
        "HTTP/1.1 200 OK"
    );
    assert!(tls_get(addr, &ours, "/versions").is_err());

    // A client CA file with no certificate: the running one stays.
    std::fs::write(&ca, "no certificate here").unwrap();
    match f.api.reload_certs().as_slice() {
        [akit_admin::CertReload::Failed(e)] => assert!(e.contains("clients.pem"), "{e}"),
        r => panic!("{r:?}"),
    }
    assert_eq!(
        tls_get(addr, &theirs, "/versions").unwrap(),
        "HTTP/1.1 200 OK"
    );
}

// I100: the link tokens (fit's admin/tokens.go and its tests).

/// A config with a tunnel hub, whose tokens the API manages.
fn hub_doc() -> String {
    r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}],
  "routes": [{"handler": {"module": "respond", "params": {"body": "v1"}}}],
  "tunnel": {"hub": {"name": "hub"}}}"#
        .to_owned()
}

#[test]
fn tokens_issued_listed_changed_and_revoked() {
    let f = start(&hub_doc(), admin(|_| {}));
    // Issued: shown once, its hash in the config.
    let r = f.send(
        "POST",
        "/tokens/tunnel.hub",
        "application/json",
        r#"{"name": " lab ", "note": "the lab", "ttl": "1h"}"#,
        "",
    );
    assert_eq!(r.status, 200, "{r:?}");
    let v = akit_config::value::parse(&r.body).unwrap();
    let Some(Value::String(token)) = v.get("token") else {
        panic!("{r:?}")
    };
    assert!(token.starts_with("akitt1."), "{token}");
    let Some(Value::String(id)) = v.get("id") else {
        panic!("{r:?}")
    };
    assert_eq!(v.get("name"), Some(&Value::String("lab".into())));
    let secret = akit_roles::linktoken::Secret::parse(token).unwrap();
    // The Admin API redacts the tokens' records, as fit's secretName rule takes "tokens" (I80b).
    assert!(f.get("/config").body.contains("[redacted]"));
    let config = f.configs.current().data().to_owned();
    assert!(config.contains(&secret.hash()), "{config}");
    assert!(!config.contains(token.as_str()), "the token itself is kept");
    // Listed, one member's and every one's.
    let r = f.get("/tokens/tunnel.hub");
    assert_eq!(r.status, 200);
    assert!(
        r.body.contains(id.as_str()) && !r.body.contains("hash"),
        "{r:?}"
    );
    let r = f.get("/tokens");
    assert!(r.body.starts_with(r#"{"roles":{"tunnel.hub":[{"#), "{r:?}");
    // Its expiry changed, then never.
    let r = f.send(
        "PATCH",
        &format!("/tokens/tunnel.hub/{id}"),
        "application/json",
        r#"{"expires_at": 4102444800}"#,
        "",
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert!(r.body.contains(r#""expires_at":4102444800"#), "{r:?}");
    let r = f.send(
        "PATCH",
        &format!("/tokens/tunnel.hub/{id}"),
        "application/json",
        "{}",
        "",
    );
    assert!(r.status == 200 && !r.body.contains("expires_at"), "{r:?}");
    // Refused: a bad name, a past expiry, an unknown token, a member that issues none, one the
    // config does not set.
    for (method, path, body, status) in [
        (
            "POST",
            "/tokens/tunnel.hub".to_owned(),
            r#"{"name": ""}"#,
            400,
        ),
        (
            "POST",
            "/tokens/tunnel.hub".to_owned(),
            r#"{"name": "a", "expires_at": 5}"#,
            400,
        ),
        (
            "POST",
            "/tokens/tunnel.hub".to_owned(),
            r#"{"name": 1}"#,
            400,
        ),
        (
            "PATCH",
            "/tokens/tunnel.hub/0123456789abcdef".to_owned(),
            "{}",
            404,
        ),
        (
            "POST",
            "/tokens/tunnel.node".to_owned(),
            r#"{"name": "a"}"#,
            404,
        ),
        (
            "POST",
            "/tokens/llm.hub".to_owned(),
            r#"{"name": "a"}"#,
            404,
        ),
    ] {
        let r = f.send(method, &path, "application/json", body, "");
        assert_eq!(r.status, status, "{method} {path} {body}: {r:?}");
    }
    assert_eq!(f.get("/tokens/llm.hub").status, 404);
    assert_eq!(f.get("/tokens/tunnel.hub/x/y").status, 404);
    assert_eq!(f.request_with("PUT", "/tokens", &[]).status, 405);
    // Revoked: gone from the config.
    let r = request(
        f.api.addr(),
        "DELETE",
        &format!("/tokens/tunnel.hub/{id}"),
        &[],
        None,
    );
    assert_eq!((r.status, r.body.as_str()), (200, r#"{"ok":true}"#));
    assert!(!f.configs.current().data().contains(&secret.hash()));
    assert_eq!(f.get("/tokens/tunnel.hub").body, r#"{"tokens":[]}"#);
}
