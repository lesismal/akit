//! The console's server (I90), with fit's `admin/console_test.go` (`TestConsoleLoginAndEdit`,
//! `TestConsoleLoginRefused`, `TestConsoleSessions`, `TestConsoleFailureTableBounded`) and the
//! cases of `app/console`'s tests that need neither the pages nor the store ported: logins,
//! lockouts, sessions' ends, CSRF tokens, other origins, fit's headers, TLS, access at accept and
//! after a config applied, and a config changed through the console applied as its own source and
//! logged. On the backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use akio::tls::stream::Stream;
use akio::tls::{ClientConfig, RootCerts};
use akio::{Backend, UringConfig};
use akit_admin::{Applied, ApplyError, Configs, Job, Mode, Options, SOURCE_FILE, Server};
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

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("console")
        .join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The `admin` logger's lines, written to a file this process's log holds.
fn log_file() -> &'static Path {
    static LOG: OnceLock<PathBuf> = OnceLock::new();
    LOG.get_or_init(|| {
        let d = dir("log");
        let p = d.join("admin.log");
        let c = akit_config::Log {
            output: p.to_string_lossy().into_owned(),
            ..akit_config::Log::default()
        };
        let files = akit_observe::Files::new();
        let root =
            akit_observe::logger::Root::new(Some(&c), akit_observe::Zone::utc(), &files).unwrap();
        akit_observe::logger::install(Arc::new(root));
        p
    })
}

/// Waits for a line holding `want` in the log, as its file's thread writes it.
fn logged(want: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let text = std::fs::read_to_string(log_file()).unwrap_or_default();
        if text.contains(want) {
            return true;
        }
        if Instant::now() > deadline {
            eprintln!("log: {text}");
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn doc(body: &str) -> String {
    format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
  "routes": [{{"handler": {{"module": "respond", "params": {{"body": "{body}"}}}}}}]}}"#
    )
}

fn hash(pw: &str) -> String {
    akio_crypto::bcrypt::hash(pw.as_bytes(), akio_crypto::bcrypt::MIN_COST).unwrap()
}

fn console(f: impl FnOnce(&mut akit_config::Console)) -> akit_config::Console {
    let mut c = akit_config::Console {
        address: "127.0.0.1:0".into(),
        users: vec![akit_config::ConsoleUser {
            username: "ada".into(),
            password_hash: hash("pw"),
        }],
        ..akit_config::Console::default()
    };
    f(&mut c);
    c
}

/// A running server, its Admin API and console, and the control thread that applies changes.
struct Fixture {
    configs: Configs,
    api: Server,
    addr: SocketAddr,
    web: SocketAddr,
    jobs: mpsc::SyncSender<Option<Job>>,
    control: Option<JoinHandle<()>>,
    engine: Arc<akio::Engine>,
}

fn start(c: akit_config::Console, opts: Options) -> Fixture {
    let _ = log_file();
    let text = doc("one");
    let config = akit_config::from_json(&text, None).unwrap();
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
        akit_config::value::standardize(&text).unwrap(),
        SOURCE_FILE,
        10,
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
    let settings = akit_config::Admin {
        address: "127.0.0.1:0".into(),
        console: Some(c),
        ..akit_config::Admin::default()
    };
    let mut opts = opts;
    opts.backend = backend();
    let api = Server::start(&settings, configs.clone(), submit, &opts).unwrap();
    let url = api.console_url().unwrap();
    let addr = url
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .parse()
        .unwrap();
    let cs = configs.clone();
    let control = std::thread::spawn(move || {
        while let Ok(Some(job)) = rx.recv() {
            job.run(&cs, |text, mode| {
                let config = akit_config::from_json(text, None).map_err(ApplyError::Config)?;
                let registry: Registry<HandlerParams> =
                    Registry::new(&[akit_handler::MODULES]).unwrap();
                let graph =
                    compile(&config, &registry, &Env::default()).map_err(ApplyError::Config)?;
                match mode {
                    Mode::Apply => listeners.reload(graph),
                    Mode::DryRun => listeners.plan(&graph),
                }
                .map_err(|e| ApplyError::Failed(e.to_string()))?;
                Ok(Applied::default())
            });
            cell.collect();
        }
    });
    Fixture {
        configs,
        api,
        addr,
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

/// One exchange on `s`: the request, `Connection: close`, and the response to its end.
fn exchange(
    mut s: impl Read + Write,
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Resp {
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (n, v) in headers {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    if let Some(b) = body {
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).unwrap();
    if let Some(b) = body {
        let _ = s.write_all(b.as_bytes());
    }
    let _ = s.flush();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    parse(&String::from_utf8_lossy(&raw))
}

/// A browser: its session's cookie and CSRF token, over TLS with `tls`.
struct Browser {
    addr: SocketAddr,
    tls: Option<ClientConfig>,
    cookie: String,
    csrf: String,
}

impl Browser {
    fn new(f: &Fixture) -> Browser {
        Browser {
            addr: f.addr,
            tls: None,
            cookie: String::new(),
            csrf: String::new(),
        }
    }

    fn origin(&self) -> String {
        let scheme = if self.tls.is_some() { "https" } else { "http" };
        format!("{scheme}://{}", self.addr)
    }

    fn send(&self, method: &str, path: &str, headers: &[(&str, &str)], body: Option<&str>) -> Resp {
        let mut h: Vec<(&str, &str)> = headers.to_vec();
        let cookie = format!("akit_console_{}={}", self.addr.port(), self.cookie);
        if !self.cookie.is_empty() {
            h.push(("Cookie", &cookie));
        }
        if !self.csrf.is_empty() && method != "GET" {
            h.push(("X-Akit-Csrf", &self.csrf));
        }
        if body.is_some() && !h.iter().any(|(n, _)| *n == "Content-Type") {
            h.push(("Content-Type", "application/json"));
        }
        let tcp = TcpStream::connect(self.addr).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let host = self.addr.to_string();
        match &self.tls {
            None => exchange(tcp, &host, method, path, &h, body),
            Some(cfg) => {
                let mut s = Stream::client(cfg, "console.akit.test", tcp).unwrap();
                s.handshake().unwrap();
                exchange(s, &host, method, path, &h, body)
            }
        }
    }

    fn get(&self, path: &str) -> Resp {
        self.send("GET", path, &[], None)
    }

    /// Logs in, keeping the session's cookie and token; the status.
    fn login(&mut self, user: &str, pw: &str) -> u16 {
        let r = self.send(
            "POST",
            "/api/login",
            &[],
            Some(&format!(r#"{{"username": "{user}", "password": "{pw}"}}"#)),
        );
        if r.status == 200 {
            let set = r.header("Set-Cookie").unwrap();
            let (_, v) = set.split(';').next().unwrap().split_once('=').unwrap();
            self.cookie = v.to_owned();
            let body = akit_config::value::parse(&r.body).unwrap();
            let Some(akit_config::value::Value::String(c)) = body.get("csrf") else {
                panic!("{}", r.body)
            };
            self.csrf = c.clone();
        }
        r.status
    }
}

fn served(f: &Fixture) -> String {
    exchange(
        TcpStream::connect(f.web).unwrap(),
        "x",
        "GET",
        "/",
        &[],
        None,
    )
    .body
}

#[test]
fn login_and_edit() {
    let f = start(console(|_| {}), Options::default());
    let mut b = Browser::new(&f);
    // fit's headers on every answer; the API's not cached.
    let r = b.get("/api/session");
    assert_eq!(r.status, 401, "{r:?}");
    assert!(
        r.header("Content-Security-Policy")
            .unwrap()
            .contains("frame-ancestors 'none'")
    );
    assert_eq!(r.header("X-Frame-Options"), Some("DENY"));
    assert_eq!(r.header("Referrer-Policy"), Some("no-referrer"));
    assert_eq!(r.header("X-Content-Type-Options"), Some("nosniff"));
    assert_eq!(r.header("Strict-Transport-Security"), None);
    for path in [
        "/api/session",
        "/api/workbench",
        "/api/m/admin/config",
        "/api/m/admin/users",
    ] {
        assert_eq!(b.get(path).status, 401, "{path} without a login");
    }
    assert_eq!(b.login("ada", "wrong"), 401);
    assert_eq!(b.login("nobody", "pw"), 401);
    assert_eq!(b.login("ada", "pw"), 200);
    assert!(!b.csrf.is_empty());
    assert!(logged(r#"console: "ada" logged in from 127.0.0.1"#));
    assert!(logged(
        r#"console: a failed login as "nobody" from 127.0.0.1"#
    ));

    assert_eq!(
        b.get("/api/session").body.trim(),
        format!(r#"{{"user":"ada","csrf":"{}"}}"#, b.csrf)
    );
    assert_eq!(
        b.get("/api/workbench").body.trim(),
        r#"{"user":"ada","modules":["admin"]}"#
    );
    assert_eq!(
        b.get("/api/m/admin/users").body.trim(),
        r#"{"user":"ada","store":false,"users":["ada"]}"#
    );
    let r = b.get("/api/m/admin/config");
    assert_eq!(r.status, 200, "{r:?}");
    assert!(r.body.contains("\"one\""));
    assert_eq!(r.header("Cache-Control"), Some("no-store"));
    let etag = r.header("ETag").unwrap().to_owned();
    // The Admin API's other reads, through the session.
    assert_eq!(b.get("/api/m/admin/versions").status, 200);
    assert!(b.get("/api/m/admin/status").body.contains("\"version\""));

    // A change needs the CSRF token.
    let csrf = std::mem::take(&mut b.csrf);
    let r = b.send(
        "PUT",
        "/api/m/admin/config",
        &[("If-Match", &etag)],
        Some(&doc("two")),
    );
    assert_eq!(r.status, 403, "{r:?}");
    b.csrf = csrf;
    let r = b.send(
        "PUT",
        "/api/m/admin/config",
        &[("If-Match", &etag)],
        Some(&doc("two")),
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(served(&f), "two");
    assert_eq!(&*f.configs.current().source, "console");
    assert!(logged(
        r#"console: "ada": PUT /config applied config version 2"#
    ));
    // A dry run applies nothing, and says so.
    let r = b.send(
        "PUT",
        "/api/m/admin/config?dry_run=1",
        &[],
        Some(&doc("three")),
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(served(&f), "two");
    assert!(logged(
        r#"console: "ada": PUT /config?dry_run=1: 200, checked and not applied"#
    ));
    // A stale ETag, and a config with errors.
    let r = b.send(
        "PUT",
        "/api/m/admin/config",
        &[("If-Match", &etag)],
        Some(&doc("three")),
    );
    assert_eq!(r.status, 412, "{r:?}");
    let r = b.send(
        "PUT",
        "/api/m/admin/config",
        &[],
        Some(r#"{"listeners": [{"name": "web"}]}"#),
    );
    assert_eq!(r.status, 400, "{r:?}");
    assert!(logged(r#"console: "ada": PUT /config: 400"#));
    // A rollback, by the console.
    let r = b.send("POST", "/api/m/admin/rollback/1", &[], None);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(served(&f), "one");

    // Other web pages.
    let r = b.send(
        "GET",
        "/api/m/admin/config",
        &[("Origin", "http://evil.example")],
        None,
    );
    assert_eq!(r.status, 403);
    let own = b.origin();
    assert_eq!(
        b.send("GET", "/api/m/admin/config", &[("Origin", &own)], None)
            .status,
        200
    );
    let r = b.send(
        "POST",
        "/api/login",
        &[("Origin", "http://evil.example")],
        Some("{}"),
    );
    assert_eq!(r.status, 403);

    // A password changes only with a store (I93).
    let r = b.send(
        "POST",
        "/api/m/admin/account/password",
        &[],
        Some(r#"{"current_password": "pw", "password": "new"}"#),
    );
    assert_eq!(r.status, 409, "{r:?}");
    assert!(r.body.contains("admin.store"));

    // Paths that are no endpoint.
    assert!(
        b.get("/api/m/llm.hub/x")
            .body
            .contains(r#"no module \"llm.hub\" runs here"#)
    );
    assert_eq!(b.get("/api/nothing").status, 404);
    assert_eq!(b.get("/api/m/admin/nothing").status, 404);

    let r = b.send("POST", "/api/logout", &[], Some("{}"));
    assert_eq!(r.status, 204, "{r:?}");
    assert!(r.header("Set-Cookie").unwrap().contains("Max-Age=0"));
    assert!(logged(r#"console: "ada" logged out"#));
    assert_eq!(b.get("/api/session").status, 401);
}

#[test]
fn logins_refused() {
    let f = start(console(|_| {}), Options::default());
    let mut b = Browser::new(&f);
    let r = b.send(
        "POST",
        "/api/login",
        &[("Content-Type", "application/x-www-form-urlencoded")],
        Some("username=ada&password=pw"),
    );
    assert_eq!(r.status, 415);
    let big = format!(
        r#"{{"username": "ada", "password": "{}"}}"#,
        "x".repeat(5000)
    );
    assert_eq!(b.send("POST", "/api/login", &[], Some(&big)).status, 413);
    assert_eq!(b.send("POST", "/api/login", &[], Some("[1]")).status, 400);
    for _ in 0..akit_admin::console::login::MAX_FAILURES {
        assert_eq!(b.login("ada", "wrong"), 401);
    }
    let r = b.send(
        "POST",
        "/api/login",
        &[],
        Some(r#"{"username": "ada", "password": "pw"}"#),
    );
    assert_eq!(r.status, 429, "{r:?}");
    // Its whole seconds and one more, as fit's: 15 minutes, less what the test took.
    let secs: u64 = r.header("Retry-After").unwrap().parse().unwrap();
    assert!((890..=901).contains(&secs), "{secs}");
    assert!(
        r.body.contains(&format!(r#""retry_after":{secs}"#)),
        "{}",
        r.body
    );
    assert!(r.body.contains("too many failed logins from this address"));
}

#[test]
fn sessions_end() {
    let f = start(
        console(|c| {
            c.session_timeout = Duration::from_secs(1);
            c.max_sessions = 2;
        }),
        Options::default(),
    );
    let mut b = Browser::new(&f);
    assert_eq!(b.login("ada", "pw"), 200);
    assert_eq!(b.get("/api/session").status, 200);
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(b.get("/api/session").status, 401, "an idle session lives");
    let (mut first, mut second, mut third) = (Browser::new(&f), Browser::new(&f), Browser::new(&f));
    first.login("ada", "pw");
    second.login("ada", "pw");
    third.login("ada", "pw");
    assert_eq!(
        first.get("/api/session").status,
        401,
        "the oldest past max_sessions"
    );
    assert_eq!(second.get("/api/session").status, 200);
    assert_eq!(third.get("/api/session").status, 200);
    first.cookie = "made-up".into();
    assert_eq!(first.get("/api/session").status, 401);
}

#[test]
fn over_tls() {
    use akio_crypto::pkey::PrivateKey;
    use akio_x509::CertificateParams;
    let d = dir("tls");
    let key = PrivateKey::P256(akio_crypto::p256::SecretKey::generate().unwrap());
    let cert = CertificateParams::new(["console.akit.test"])
        .self_signed(&key)
        .unwrap();
    let (c, k) = (d.join("c.pem"), d.join("k.pem"));
    std::fs::write(&c, cert.to_pem()).unwrap();
    std::fs::write(&k, key.to_pkcs8_pem()).unwrap();
    let f = start(
        console(|con| {
            con.cert_file = Some(c.to_string_lossy().into_owned());
            con.key_file = Some(k.to_string_lossy().into_owned());
        }),
        Options::default(),
    );
    assert!(f.api.console_url().unwrap().starts_with("https://"));
    let mut roots = RootCerts::empty();
    roots.add_pem(&std::fs::read(&c).unwrap()).unwrap();
    let mut b = Browser::new(&f);
    b.tls = Some(ClientConfig::builder().roots(roots).build().unwrap());
    let r = b.get("/api/session");
    assert_eq!(r.status, 401);
    assert_eq!(
        r.header("Strict-Transport-Security"),
        Some("max-age=31536000")
    );
    let r = b.send(
        "POST",
        "/api/login",
        &[],
        Some(r#"{"username": "ada", "password": "pw"}"#),
    );
    assert_eq!(r.status, 200);
    assert!(r.header("Set-Cookie").unwrap().ends_with("; Secure"));
    assert!(
        r.header("Set-Cookie")
            .unwrap()
            .contains("HttpOnly; SameSite=Strict")
    );
    assert_eq!(b.login("ada", "pw"), 200);
    assert_eq!(b.get("/api/m/admin/config").status, 200);
    // An origin in the clear is not the console's.
    let r = b.send(
        "GET",
        "/api/session",
        &[("Origin", &format!("http://{}", f.addr))],
        None,
    );
    assert_eq!(r.status, 403);
}

#[test]
fn access_at_accept() {
    let list = akit_access::compile(
        Some(&akit_config::Access {
            allow: vec!["192.0.2.1".into()],
            ..akit_config::Access::default()
        }),
        None,
    )
    .unwrap();
    let allow_other = akit_access::Policy::chain([list.as_ref()]).unwrap();
    let mut opts = Options::default();
    opts.console_access = Some(Arc::clone(&allow_other));
    let f = start(console(|_| {}), opts);
    // Closed as it is accepted: nothing comes back.
    let mut s = TcpStream::connect(f.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let _ = s.write_all(b"GET /api/session HTTP/1.1\r\nHost: x\r\n\r\n");
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf);
    assert!(buf.is_empty(), "{}", String::from_utf8_lossy(&buf));
    // A config applied that lets everyone in.
    f.api.set_console_access(None);
    assert_eq!(Browser::new(&f).get("/api/session").status, 401);
    f.api.set_console_access(Some(&allow_other));
    let mut s = TcpStream::connect(f.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let _ = s.write_all(b"GET /api/session HTTP/1.1\r\nHost: x\r\n\r\n");
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf);
    assert!(buf.is_empty());
}

#[test]
fn failure_table_bounded() {
    // fit's TestConsoleFailureTableBounded, on the logins themselves.
    let l = Arc::new(akit_admin::console::login::Logins::new(
        &akit_admin::console::login::Options {
            users: vec![("ada".into(), hash("pw"))],
            idle: Duration::from_secs(60),
            lifetime: Duration::from_secs(60),
            max_sessions: 1,
        },
    ));
    for i in 0..akit_admin::console::login::MAX_ADDRESSES + 10 {
        l.failed(&format!("10.0.{}.{}", i / 256, i % 256));
    }
    assert!(l.addresses() <= akit_admin::console::login::MAX_ADDRESSES);
}

/// I91: fit's workbench, its files as fit ships them, and fit's schema behind a login.
#[test]
fn pages_and_schema() {
    let f = start(console(|_| {}), Options::default());
    let mut b = Browser::new(&f);
    let r = b.get("/");
    assert_eq!(r.status, 200);
    assert_eq!(r.header("Content-Type"), Some("text/html; charset=utf-8"));
    assert_eq!(r.header("Cache-Control"), Some("no-cache"));
    assert!(
        r.header("Content-Security-Policy")
            .unwrap()
            .contains("script-src 'self'")
    );
    let web = Path::new(env!("CARGO_MANIFEST_DIR")).join("console/web");
    assert_eq!(
        r.body,
        std::fs::read_to_string(web.join("index.html")).unwrap()
    );
    for (path, ct) in [
        ("/core/workbench.js", "text/javascript; charset=utf-8"),
        ("/core/ui.css", "text/css; charset=utf-8"),
        ("/gateway/logs.js", "text/javascript; charset=utf-8"),
    ] {
        let r = b.get(path);
        assert_eq!(
            (r.status, r.header("Content-Type")),
            (200, Some(ct)),
            "{path}"
        );
        assert_eq!(
            r.body,
            std::fs::read_to_string(web.join(path.trim_start_matches('/'))).unwrap(),
            "{path}"
        );
    }
    assert_eq!(b.get("/nothing.js").status, 404);
    assert_eq!(b.send("POST", "/", &[], Some("{}")).status, 405);
    assert_eq!(b.get("/api/m/admin/schema").status, 401);
    assert_eq!(b.login("ada", "pw"), 200);
    let r = b.get("/api/m/admin/schema");
    assert_eq!(r.status, 200);
    assert_eq!(r.header("Content-Type"), Some("application/schema+json"));
    assert!(r.body.contains("\"$defs\"") && r.body.contains("\"console\""));
    // The raw editors' endpoints, through the console.
    let r = b.send(
        "POST",
        "/api/m/admin/config/render?format=conf",
        &[],
        Some(r#"{"log": {"level": "debug"}}"#),
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert!(r.body.contains("log"), "{}", r.body);
}
