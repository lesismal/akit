//! The console's server (I90), with fit's `admin/console_test.go` (`TestConsoleLoginAndEdit`,
//! `TestConsoleLoginRefused`, `TestConsoleSessions`, `TestConsoleFailureTableBounded`) and the
//! cases of `app/console`'s tests that need neither the pages nor the store ported: logins,
//! lockouts, sessions' ends, CSRF tokens, other origins, fit's headers, TLS, access at accept and
//! after a config applied, a config changed through the console applied as its own source and
//! logged, and the certificate read again when its files change (I94). With a store (I93b),
//! fit's `admin/accounts_test.go`, `accounts_edge_test.go`, `transfer_test.go` and
//! `transfer_edge_test.go`: password changes and resets kept in the store, the temporary password
//! of `akit run -admin` (I93c), and the `store` format of exports and imports. On the backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
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
    start_with(c, opts, |_| {})
}

/// [`start`], with `prep` given the configs before the Admin API and the console start.
fn start_with(c: akit_config::Console, opts: Options, prep: impl FnOnce(&Configs)) -> Fixture {
    let _ = log_file();
    let text = doc("one");
    let mut config = akit_config::from_json(&text, None).unwrap();
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
    opts.backend = Some(backend());
    prep(&configs);
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

/// A self-signed certificate for `console.akit.test` and its key, written to `c` and `k`.
fn self_signed(c: &Path, k: &Path) {
    use akio_crypto::pkey::PrivateKey;
    let key = PrivateKey::P256(akio_crypto::p256::SecretKey::generate().unwrap());
    let cert = akio_x509::CertificateParams::new(["console.akit.test"])
        .self_signed(&key)
        .unwrap();
    std::fs::write(c, cert.to_pem()).unwrap();
    std::fs::write(k, key.to_pkcs8_pem()).unwrap();
}

/// The leaf the console presents to a new connection, as DER (a client of its own, which resumes
/// no session).
fn served_leaf(addr: SocketAddr) -> Vec<u8> {
    let cfg = ClientConfig::builder()
        .insecure_skip_verify()
        .build()
        .unwrap();
    let tcp = TcpStream::connect(addr).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    tcp.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut s = Stream::client(&cfg, "console.akit.test", tcp).unwrap();
    s.handshake().unwrap();
    s.conn().peer_certificates()[0].clone()
}

fn leaf_der(path: &Path) -> Vec<u8> {
    akio_x509::Certificate::from_pem(&std::fs::read(path).unwrap()).unwrap()[0]
        .as_der()
        .to_vec()
}

/// I94: the console's certificate read again when its files change (fit's TestTLSReload, for the
/// console): a renewed one served to new connections, one that does not load leaving the running
/// one, with a warning in the admin logger.
#[test]
fn tls_reload() {
    let d = dir("tls-reload");
    let (c, k) = (d.join("c.pem"), d.join("k.pem"));
    self_signed(&c, &k);
    let f = start(
        console(|con| {
            con.cert_file = Some(c.to_string_lossy().into_owned());
            con.key_file = Some(k.to_string_lossy().into_owned());
        }),
        Options::default(),
    );
    let first = served_leaf(f.addr);
    assert_eq!(first, leaf_der(&c));
    assert!(f.api.reload_certs().is_empty());

    // Renewed outside akit: written whole beside it, then renamed over it, as tools do.
    let (nc, nk) = (d.join("new.pem"), d.join("new.key"));
    self_signed(&nc, &nk);
    let renewed = leaf_der(&nc);
    std::fs::rename(&nc, &c).unwrap();
    std::fs::rename(&nk, &k).unwrap();
    let now = Instant::now();
    assert!(f.api.watch_certs(now).is_empty(), "the first look settles");
    assert_eq!(
        f.api.watch_certs(now + Duration::from_secs(1)),
        vec![akit_admin::CertReload::Reloaded("console")]
    );
    assert_eq!(served_leaf(f.addr), renewed);
    assert_ne!(renewed, first);
    assert!(logged("console: certificate files changed; reloaded"));

    // The certificate replaced and its key not yet: the renewed one stays.
    self_signed(&nc, &nk);
    std::fs::rename(&nc, &c).unwrap();
    match f.api.reload_certs().as_slice() {
        [akit_admin::CertReload::Failed(e)] => {
            assert!(e.starts_with("console: "), "{e}");
            assert!(e.ends_with("; the running certificate stays"), "{e}");
        }
        r => panic!("{r:?}"),
    }
    assert_eq!(served_leaf(f.addr), renewed);
    assert!(logged("the running certificate stays"));
    // Its key written: served.
    std::fs::rename(&nk, &k).unwrap();
    assert_eq!(
        f.api.reload_certs(),
        vec![akit_admin::CertReload::Reloaded("console")]
    );
    assert_eq!(served_leaf(f.addr), leaf_der(&c));
}

/// A role's module that answers nothing: for the role consoles' own behaviour.
struct NoPages;

impl akit_roles::pages::Module for NoPages {
    fn id(&self) -> &'static str {
        "tunnel.node"
    }

    fn serve(&self, ctx: &mut akio_http::Context, _user: &str, _path: &str) {
        ctx.respond(akit_roles::pages::no_endpoint());
    }
}

/// I109b: a role's console's certificate read again when its files change, as
/// `admin.console`'s (I94), the console kept: a renewed one served to new connections, one that
/// does not load leaving the running one, each named by its role.
#[test]
fn a_role_consoles_tls_reload() {
    use akit_roles::pages::Consoles;
    let d = dir("role-tls-reload");
    let (c, k) = (d.join("c.pem"), d.join("k.pem"));
    self_signed(&c, &k);
    let doc = format!(
        r#"{{"tunnel": {{"node": {{"console": {{"address": "127.0.0.1:0", "cert_file": "{}", "key_file": "{}", "users": [{{"username": "admin", "password_hash": "{}"}}]}}}}}}}}"#,
        c.display(),
        k.display(),
        hash("pw")
    );
    let config = akit_config::from_json(&doc, None).unwrap();
    let mut rc = akit_admin::console::RoleConsoles::new(&Options::default());
    let module: akit_roles::pages::ModuleRef = Arc::new(NoPages);
    rc.apply("tunnel.node", Some(&config), Some(module))
        .unwrap();
    let addr = rc.addr("tunnel.node").unwrap();
    let first = served_leaf(addr);
    assert_eq!(first, leaf_der(&c));
    assert!(rc.reload_certs().is_empty());

    let (nc, nk) = (d.join("new.pem"), d.join("new.key"));
    self_signed(&nc, &nk);
    let renewed = leaf_der(&nc);
    std::fs::rename(&nc, &c).unwrap();
    std::fs::rename(&nk, &k).unwrap();
    let now = Instant::now();
    assert!(rc.watch_certs(now).is_empty(), "the first look settles");
    assert_eq!(
        rc.watch_certs(now + Duration::from_secs(1)),
        vec![akit_admin::CertReload::Reloaded("tunnel.node")]
    );
    assert_eq!(rc.addr("tunnel.node"), Some(addr), "the console was kept");
    assert_eq!(served_leaf(addr), renewed);

    // The certificate replaced and its key not yet: the renewed one stays.
    self_signed(&nc, &nk);
    std::fs::rename(&nc, &c).unwrap();
    match rc.reload_certs().as_slice() {
        [akit_admin::CertReload::Failed(e)] => {
            assert!(e.starts_with("tunnel.node: "), "{e}");
        }
        r => panic!("{r:?}"),
    }
    assert_eq!(served_leaf(addr), renewed);
    // Its key written: served, through the trait the roles' manager calls.
    std::fs::rename(&nk, &k).unwrap();
    Consoles::reload_certs(&mut rc);
    assert_eq!(served_leaf(addr), leaf_der(&c));
    rc.stop();
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

/// fit's `TestNode` (I131): `GET /node` says who the akit is, what config runs and what the API
/// offers, with the running config's ETag and version in its headers; through the console too,
/// where it lists the console, and the store with one. `GET /schema` is the console's schema.
#[test]
fn node_and_schema() {
    use akit_config::value::{Value, parse as parse_json};
    let mut opts = Options::default();
    opts.node = Some(Arc::new(|| akit_admin::node::NodeLive {
        roles: vec!["tunnel.node".into(), "llm.hub".into()],
        counts_requests: true,
    }));
    let f = start(console(|_| {}), opts);
    let api = f.api.address().parse::<SocketAddr>().unwrap();
    let get = |path: &str| {
        let s = TcpStream::connect(api).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        exchange(s, &api.to_string(), "GET", path, &[], None)
    };
    let r = get("/node");
    assert_eq!(r.status, 200, "{r:?}");
    let n = parse_json(&r.body).unwrap();
    let s = |k: &str| match n.get(k) {
        Some(Value::String(s)) => s.clone(),
        other => panic!("{k}: {other:?}"),
    };
    let cur = f.configs.current();
    assert_eq!(s("config_etag"), cur.etag);
    assert_eq!(s("config_source"), SOURCE_FILE);
    assert_eq!(s("id_kept"), "memory");
    assert!(!s("id").is_empty() && !s("hostname").is_empty() && !s("os").is_empty());
    assert!(s("schema_digest").starts_with("sha256:"));
    assert!(!s("started").is_empty());
    assert_eq!(n.get("config_version"), Some(&Value::Number("1".into())));
    assert_eq!(n.get("schema_version"), Some(&Value::Number("1".into())));
    assert_eq!(r.header("ETag"), Some(cur.etag.as_str()));
    assert_eq!(r.header("Akit-Config-Version"), Some("1"));
    let list = |k: &str| match n.get(k) {
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| match v {
                Value::String(s) => s.clone(),
                _ => panic!("{v:?}"),
            })
            .collect::<Vec<_>>(),
        other => panic!("{k}: {other:?}"),
    };
    assert_eq!(list("roles"), ["llm.hub", "tunnel.node"]);
    let caps = list("capabilities");
    for c in [
        "node",
        "config.dry_run",
        "config.if_match",
        "config.text",
        "config.versions",
        "status.requests",
        "schema",
        "console",
        "config.check",
    ] {
        assert!(caps.iter().any(|x| x == c), "{caps:?} lacks {c}");
    }
    assert!(!caps.iter().any(|x| x == "store"));
    let mut sorted = caps.clone();
    sorted.sort();
    assert_eq!(caps, sorted);

    // Who the akit is, as akit run says; a change shows its version.
    let started = std::time::UNIX_EPOCH + Duration::from_secs(1_793_581_323);
    f.configs.use_node(akit_admin::node::Node {
        id: "node-a".into(),
        id_kept: akit_admin::node::ID_IN_FILE,
        hostname: String::new(),
        version: "9.9.9".into(),
        engine: "akio 0123456789ab".into(),
        started,
    });
    let mut b = Browser::new(&f);
    assert_eq!(b.login("ada", "pw"), 200);
    let r = b.send("PUT", "/api/m/admin/config", &[], Some(&doc("two")));
    assert_eq!(r.status, 200, "{r:?}");
    for path in ["/api/node", "/api/m/admin/node"] {
        let r = b.get(path);
        assert_eq!(r.status, 200, "{path}: {r:?}");
        let n = parse_json(&r.body).unwrap();
        let s = |k: &str| match n.get(k) {
            Some(Value::String(s)) => s.clone(),
            other => panic!("{k}: {other:?}"),
        };
        assert_eq!(
            (
                s("id"),
                s("id_kept"),
                s("akit_version"),
                s("fib_version"),
                s("started"),
                s("config_source")
            ),
            (
                "node-a".to_owned(),
                "file".to_owned(),
                "9.9.9".to_owned(),
                "akio 0123456789ab".to_owned(),
                "2026-11-02T01:02:03.000Z".to_owned(),
                "console".to_owned()
            ),
            "{path}"
        );
        assert_eq!(n.get("config_version"), Some(&Value::Number("2".into())));
    }
    let r = get("/schema");
    assert_eq!(r.status, 200);
    assert_eq!(r.header("Content-Type"), Some("application/schema+json"));
    assert_eq!(r.body, b.get("/api/m/admin/schema").body);
    assert_eq!(b.send("POST", "/api/node", &[], Some("{}")).status, 405);
}

// The console's connections (akio's A232; fit's `app/console/idle_test.go` and
// `TestLimitHandlerEvicts`).

/// A raw HTTP/1.1 connection to the console, so that a test knows which connection each request
/// goes on.
struct Raw {
    s: TcpStream,
    buf: Vec<u8>,
}

impl Raw {
    fn dial(f: &Fixture) -> Raw {
        let s = TcpStream::connect(f.addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        Raw { s, buf: Vec::new() }
    }

    /// Sends a request with header lines `extra` and `body`; its response, or `None` when the
    /// connection closed first.
    fn try_do(&mut self, method: &str, path: &str, body: &str, extra: &[&str]) -> Option<Resp> {
        let mut req = format!("{method} {path} HTTP/1.1\r\nHost: test\r\n");
        for h in extra {
            req.push_str(h);
            req.push_str("\r\n");
        }
        if !body.is_empty() {
            req.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                body.len()
            ));
        }
        req.push_str("\r\n");
        req.push_str(body);
        self.s.write_all(req.as_bytes()).ok()?;
        // The head, then its Content-Length's body.
        let head_end = loop {
            if let Some(i) = self.buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
            let mut b = [0u8; 4096];
            match self.s.read(&mut b) {
                Ok(0) | Err(_) => return None,
                Ok(n) => self.buf.extend_from_slice(&b[..n]),
            }
        };
        let head = String::from_utf8_lossy(&self.buf[..head_end]).into_owned();
        let len: usize = head
            .lines()
            .filter_map(|l| l.split_once(':'))
            .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
            .map_or(0, |(_, v)| v.trim().parse().unwrap());
        while self.buf.len() < head_end + len {
            let mut b = [0u8; 4096];
            match self.s.read(&mut b) {
                Ok(0) | Err(_) => return None,
                Ok(n) => self.buf.extend_from_slice(&b[..n]),
            }
        }
        let rest = self.buf.split_off(head_end + len);
        let raw = String::from_utf8_lossy(&self.buf).into_owned();
        self.buf = rest;
        Some(parse(&raw))
    }

    fn send(&mut self, method: &str, path: &str, body: &str, extra: &[&str]) -> Resp {
        self.try_do(method, path, body, extra)
            .unwrap_or_else(|| panic!("{method} {path}: the connection closed"))
    }

    /// Whether the console has closed the connection, waiting up to `within`.
    fn closed_within(&mut self, within: Duration) -> bool {
        self.s.set_read_timeout(Some(within)).unwrap();
        let mut b = [0u8; 1];
        let r = self.s.read(&mut b);
        self.s
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        match r {
            Ok(0) => true,
            Ok(_) => panic!("the console sent more on an idle connection"),
            Err(e) => !matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ),
        }
    }

    /// Asks `GET path` every `gap` until the console closes the connection; how long after
    /// `start` that was. Fails past `limit`.
    fn keeps_asking(
        &mut self,
        start: Instant,
        gap: Duration,
        limit: Duration,
        path: &str,
    ) -> Duration {
        while start.elapsed() < limit {
            if self.try_do("GET", path, "", &[]).is_none() {
                return start.elapsed();
            }
            std::thread::sleep(gap);
        }
        panic!(
            "the connection is still open {:?} on, asked every {gap:?}",
            start.elapsed()
        );
    }
}

/// The session cookie of a login's response, as a `Cookie` line.
fn cookie_line(r: &Resp) -> String {
    let set = r.header("Set-Cookie").unwrap();
    format!("Cookie: {}", set.split(';').next().unwrap())
}

fn timeouts(unauth: Duration, idle: Duration) -> Options {
    let mut o = Options::default();
    o.console_unauth_timeout = unauth;
    o.console_idle_timeout = idle;
    o
}

const LOGIN: &str = r#"{"username": "ada", "password": "pw"}"#;

/// fit's `TestConsoleIdleTimeouts`: a connection that has not sent a request with a valid session
/// is closed after the short timeout, whether it said nothing or made requests without one; one
/// that has, by logging in or by a request carrying the session, outlives it, request after
/// request, until it logs out.
#[test]
fn connections_idle_timeouts() {
    let (unauth, idle) = (Duration::from_secs(1), Duration::from_secs(10));
    let f = start(console(|_| {}), timeouts(unauth, idle));
    // Silent.
    assert!(Raw::dial(&f).closed_within(idle / 2), "a silent connection");
    // Without a session.
    let mut c = Raw::dial(&f);
    assert_eq!(c.send("GET", "/", "", &[]).status, 200);
    let forged = format!("Cookie: akit_console_{}=forged", f.addr.port());
    assert_eq!(c.send("GET", "/api/session", "", &[&forged]).status, 401);
    let wrong = r#"{"username": "ada", "password": "wrong"}"#;
    let r = c.try_do("POST", "/api/login", wrong, &[]);
    assert!(r.is_none_or(|r| r.status == 401));
    assert!(c.closed_within(idle / 2), "a connection without a session");
    // Logged in: past the short timeout, the login keeps the connection.
    let mut c = Raw::dial(&f);
    let r = c.send("POST", "/api/login", LOGIN, &[]);
    assert_eq!(r.status, 200);
    let cookie = cookie_line(&r);
    std::thread::sleep(3 * unauth);
    assert_eq!(c.send("GET", "/", "", &[]).status, 200, "after a pause");
    // With a session: twice past the short timeout, the second time after a request without
    // the session, which does not shorten it again.
    let mut c = Raw::dial(&f);
    assert_eq!(c.send("GET", "/api/session", "", &[&cookie]).status, 200);
    std::thread::sleep(3 * unauth);
    assert_eq!(c.send("GET", "/", "", &[]).status, 200);
    std::thread::sleep(3 * unauth);
    assert_eq!(c.send("POST", "/api/logout", "", &[&cookie]).status, 204);
    // Logged out, it is an unauthenticated connection again.
    assert!(c.closed_within(idle / 2), "after logging out");
}

/// fit's `TestConsoleUnauthBoundFromOpen`: a connection without a valid session is closed the
/// short timeout after it opened however often it asks; one whose session has ended since it
/// was kept for it is closed the short timeout after the request that found it gone.
#[test]
fn connections_bound_from_open() {
    let (unauth, idle) = (Duration::from_millis(600), Duration::from_secs(10));
    let f = start(console(|_| {}), timeouts(unauth, idle));
    for path in ["/", "/api/session"] {
        let start = Instant::now();
        let mut c = Raw::dial(&f);
        let held = c.keeps_asking(start, unauth / 3, 10 * unauth, path);
        assert!(held <= 3 * unauth, "{path}: closed only after {held:?}");
    }
    // The session ended.
    let mut c = Raw::dial(&f);
    let r = c.send("POST", "/api/login", LOGIN, &[]);
    assert_eq!(r.status, 200);
    let cookie = cookie_line(&r);
    std::thread::sleep(2 * unauth);
    assert_eq!(c.send("GET", "/api/session", "", &[&cookie]).status, 200);
    // Another tab logs out: the next request finds the session gone.
    assert_eq!(
        Raw::dial(&f)
            .send("POST", "/api/logout", "", &[&cookie])
            .status,
        204
    );
    let start = Instant::now();
    assert_eq!(c.send("GET", "/api/session", "", &[&cookie]).status, 401);
    let held = c.keeps_asking(start, unauth / 3, 10 * unauth, "/");
    assert!(
        held <= 3 * unauth,
        "the ended session: closed only after {held:?}"
    );
}

/// fit's `TestConsoleUnauthBoundServing`: an unauthenticated connection whose time runs out while
/// a request is served (a login whose check is slow) is not cut under it: the response comes,
/// saying `Connection: close`, and the connection closes after it; one whose request goes on
/// past the grace is closed anyway. The grace (fit's: as long as the short timeout) is set apart
/// here, so that how long a check takes on a loaded host does not decide the outcome.
#[test]
fn connections_answered_past_their_time() {
    // A cost whose check takes four times the short timeout here, or more, from the quickest of
    // three checks at cost 8 (each cost more doubles the time): a check measured on a loaded
    // host, and run on a quiet one, must still outlast the timeout and its grace (twice the
    // timeout, measured once, did not).
    let (unauth, idle) = (Duration::from_millis(300), Duration::from_secs(10));
    let quickest = (0..3)
        .map(|_| {
            let t = Instant::now();
            akio_crypto::bcrypt::hash(b"pw", 8).unwrap();
            t.elapsed()
        })
        .min()
        .unwrap();
    let mut cost = 8;
    let mut takes = quickest;
    while takes < 4 * unauth && cost < 16 {
        cost += 1;
        takes *= 2;
    }
    let slow = akio_crypto::bcrypt::hash(b"pw", cost).unwrap();
    let users = |c: &mut akit_config::Console| c.users[0].password_hash = slow.clone();
    let mut o = timeouts(unauth, idle);
    o.console_grace = Some(Duration::from_secs(20));
    let f = start(console(users), o);
    let mut c = Raw::dial(&f);
    // A wrong password: checked in full, the connection still without a session.
    let r = c.send(
        "POST",
        "/api/login",
        r#"{"username": "ada", "password": "wrong"}"#,
        &[],
    );
    assert_eq!(r.status, 401);
    assert_eq!(r.header("Connection"), Some("close"), "{r:?}");
    assert!(c.closed_within(idle / 2));
    drop(f);
    // Past the grace: closed under the request, with no answer.
    let mut o = timeouts(unauth, idle);
    o.console_grace = Some(Duration::from_millis(20));
    let f = start(console(users), o);
    let mut c = Raw::dial(&f);
    let r = c.try_do(
        "POST",
        "/api/login",
        r#"{"username": "ada", "password": "wrong"}"#,
        &[],
    );
    assert!(r.is_none(), "answered past the grace: {r:?}");
}

/// fit's `TestLimitHandlerEvicts`: with its connections taken, the console closes the oldest
/// unauthenticated one for a new connection, rather than refuse it; an authenticated one is
/// kept, and a new one is refused only when every connection has authenticated.
#[test]
fn connections_evicted_for_new_ones() {
    let f = start(console(|c| c.max_conns = 4), Options::default()); // 3 for unauthenticated ones
    let mut c1 = Raw::dial(&f);
    assert_eq!(c1.send("POST", "/api/login", LOGIN, &[]).status, 200);
    let mut cs: Vec<Raw> = (0..3).map(|_| Raw::dial(&f)).collect();
    for c in &mut cs {
        // Served: counted.
        assert_eq!(c.send("GET", "/", "", &[]).status, 200);
    }
    // Full, with 3 unauthenticated: the oldest of them goes.
    let mut c5 = Raw::dial(&f);
    assert_eq!(c5.send("GET", "/", "", &[]).status, 200);
    assert!(
        cs[0].closed_within(Duration::from_secs(5)),
        "not the oldest"
    );
    assert!(!cs[1].closed_within(Duration::from_millis(200)));
    assert!(!cs[2].closed_within(Duration::from_millis(200)));
    assert_eq!(
        c1.send("GET", "/", "", &[]).status,
        200,
        "an authenticated one"
    );
    // Every one authenticated: a new one is closed as it opens.
    for c in cs.iter_mut().skip(1).chain([&mut c5]) {
        assert_eq!(c.send("POST", "/api/login", LOGIN, &[]).status, 200);
    }
    let mut last = Raw::dial(&f);
    assert!(
        last.try_do("GET", "/", "", &[]).is_none(),
        "a connection past max_conns, all authenticated, was served"
    );
}

// --- With a store (I93b, I93c): fit's accounts, transfer and temporary-password tests. ---

/// Argon2id cheap enough for tests: the store's header records them.
const CHEAP: akit_admin::store::Params = akit_admin::store::Params {
    time: 1,
    memory_kib: 64,
    threads: 1,
};

const USERS: &str = "/api/m/admin/users";
const ACCOUNT: &str = "/api/m/admin/account/password";
const RESET: &str = "/api/m/admin/users/password";

type Kept = Arc<std::sync::Mutex<akit_admin::store::Store>>;

fn locked(k: &Kept) -> std::sync::MutexGuard<'_, akit_admin::store::Store> {
    k.lock().unwrap()
}

/// fit's `startStoreConsole`: a console of ada (`pw`) and bob (`bobpw`) whose configs keep the
/// password hashes changed from it beside the config in a store, as `akit run` does with
/// `admin.store`, its key sealing the exports; `prep` given the configs too, before the console
/// starts. The store, its path, and the bootstrap file's `admin` as JSON.
fn start_store(name: &str, prep: impl FnOnce(&Configs)) -> (Fixture, Kept, PathBuf, String) {
    let users = vec![
        akit_config::ConsoleUser {
            username: "ada".into(),
            password_hash: hash("pw"),
        },
        akit_config::ConsoleUser {
            username: "bob".into(),
            password_hash: hash("bobpw"),
        },
    ];
    let mut admin = String::from(r#"{"console": {"address": "127.0.0.1:0", "users": ["#);
    for (i, u) in users.iter().enumerate() {
        if i > 0 {
            admin.push(',');
        }
        admin.push_str(&format!(
            r#"{{"username": "{}", "password_hash": "{}"}}"#,
            u.username, u.password_hash
        ));
    }
    admin.push_str("]}}");
    let path = dir(name).join("akit.store");
    let mut st = akit_admin::store::Store::create_with(&path, "secret", CHEAP).unwrap();
    st.write(doc("one").into_bytes()).unwrap();
    let sealer = st.sealer().unwrap();
    let kept: Kept = Arc::new(std::sync::Mutex::new(st));
    let (a, b) = (Arc::clone(&kept), Arc::clone(&kept));
    let f = start_with(console(|c| c.users = users), Options::default(), |cs| {
        cs.use_store(
            None,
            Box::new(move |d: &str| {
                locked(&a)
                    .write(d.as_bytes().to_vec())
                    .map_err(|e| e.to_string())
            }),
        );
        cs.use_console_users(
            Vec::new(),
            Box::new(move |u: &[akit_admin::store::User]| {
                locked(&b).set_users(u.to_vec()).map_err(|e| e.to_string())
            }),
        );
        cs.use_sealer(sealer);
        prep(cs);
    });
    (f, kept, path, admin)
}

/// One exchange with the Admin API itself, its body bytes as they came.
fn api_call(
    f: &Fixture,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let addr = f.api.addr();
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    raw_exchange(&mut s, &addr.to_string(), method, path, headers, body)
}

/// One exchange on `s` with a body of bytes, and the response's body as bytes.
fn raw_exchange(
    s: &mut (impl Read + Write),
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (n, v) in headers {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).unwrap();
    let _ = s.write_all(body);
    let _ = s.flush();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let at = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("a head");
    let head = String::from_utf8_lossy(&raw[..at]).into_owned();
    let mut lines = head.lines();
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.to_owned(), v.trim().to_owned()))
        .collect();
    (status, headers, raw[at + 4..].to_vec())
}

impl Browser {
    /// A request with a body of bytes sent as `content_type`, the response's body as bytes.
    fn send_bytes(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> (u16, Vec<(String, String)>, Vec<u8>) {
        let mut h: Vec<(&str, &str)> = headers.to_vec();
        let cookie = format!("akit_console_{}={}", self.addr.port(), self.cookie);
        h.push(("Cookie", &cookie));
        h.push(("X-Akit-Csrf", &self.csrf));
        let mut tcp = TcpStream::connect(self.addr).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        raw_exchange(&mut tcp, &self.addr.to_string(), method, path, &h, body)
    }

    fn post(&self, path: &str, body: &str) -> Resp {
        self.send("POST", path, &[], Some(body))
    }
}

fn header<'a>(h: &'a [(String, String)], name: &str) -> Option<&'a str> {
    h.iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn verifies(hash: &str, pw: &str) -> bool {
    akio_crypto::bcrypt::Hash::parse(hash).is_ok_and(|h| h.verify(pw.as_bytes()))
}

/// fit's `TestConsolePasswordChange`: a user changes its own password; the wrong current password
/// is refused, the right one changes it, the user's other sessions end, and the store keeps the
/// hash, so that a restart logs in with it.
#[test]
fn password_change() {
    let (f, _kept, path, admin) = start_store("password-change", |_| {});
    let mut b = Browser::new(&f);
    assert_eq!(b.login("ada", "pw"), 200);
    let mut other = Browser::new(&f);
    assert_eq!(other.login("ada", "pw"), 200);
    let r = b.get(USERS);
    assert!(r.body.contains(r#""store":true"#), "{r:?}");

    // A request without the session's CSRF token changes nothing.
    let csrf = std::mem::take(&mut b.csrf);
    assert_eq!(
        b.post(ACCOUNT, r#"{"current_password": "pw", "password": "new"}"#)
            .status,
        403
    );
    b.csrf = csrf;
    // The body's fields, and a password bcrypt does not take, are refused before anything is
    // checked.
    let long = format!(
        r#"{{"current_password": "pw", "password": "{}"}}"#,
        "a".repeat(73)
    );
    for body in [
        r#"{"current_password": "pw"}"#,
        r#"{"password": "new"}"#,
        long.as_str(),
    ] {
        assert_eq!(b.post(ACCOUNT, body).status, 400, "{body:.60}");
    }
    // The wrong current password is refused, and the old one still logs in.
    let r = b.post(
        ACCOUNT,
        r#"{"current_password": "wrong", "password": "new"}"#,
    );
    assert_eq!(r.status, 403, "{r:?}");
    assert!(r.body.contains("the current password is wrong"));
    assert_eq!(Browser::new(&f).login("ada", "pw"), 200);

    // The right one changes it.
    let r = b.post(ACCOUNT, r#"{"current_password": "pw", "password": "new"}"#);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.body.trim(), r#"{"changed":true,"user":"ada"}"#);
    assert!(logged(r#"console: "ada" changed the password of "ada""#));
    // The user's other sessions end; the one that changed the password stays.
    assert_eq!(other.get("/api/session").status, 401);
    assert_eq!(b.get("/api/session").status, 200);
    assert_eq!(Browser::new(&f).login("ada", "pw"), 401);
    assert_eq!(Browser::new(&f).login("ada", "new"), 200);
    let (users, stored) = f.configs.console_users();
    assert!(stored);
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].username, "ada");
    assert!(verifies(&users[0].password_hash, "new"));

    // The store keeps the hash beside the config, and the bootstrap file's admin with the
    // store's users, as akit run makes it at a restart, logs in with the new password.
    let (opened, data) = akit_admin::store::Store::open(&path, "secret").unwrap();
    assert_eq!(String::from_utf8(data.unwrap()).unwrap(), doc("one"));
    let users = opened.users().to_vec();
    assert_eq!(users.len(), 1);
    assert!(verifies(&users[0].password_hash, "new"));
    let merged = akit_admin::bootstrap::with_console_users(&admin, &users);
    let restarted = akit_config::from_json(&format!(r#"{{"admin": {merged}}}"#), None).unwrap();
    let c = restarted.admin.unwrap().console.unwrap();
    let f2 = start(c, Options::default());
    assert_eq!(Browser::new(&f2).login("ada", "new"), 200);
    assert_eq!(Browser::new(&f2).login("bob", "bobpw"), 200);
}

/// fit's `TestConsolePasswordReset`: another user's password set; the target logs in with it, its
/// sessions end, and the store keeps the hash.
#[test]
fn password_reset() {
    let (f, _kept, path, _) = start_store("password-reset", |_| {});
    let mut b = Browser::new(&f);
    assert_eq!(b.login("ada", "pw"), 200);
    let mut bob = Browser::new(&f);
    assert_eq!(bob.login("bob", "bobpw"), 200);
    let r = b.post(RESET, r#"{"username": "nobody", "password": "x"}"#);
    assert_eq!(r.status, 400);
    assert!(
        r.body
            .contains(r#"there is no console user named \"nobody\""#),
        "{r:?}"
    );
    assert_eq!(b.post(RESET, r#"{"username": "bob"}"#).status, 400);
    let r = b.post(RESET, r#"{"username": "bob", "password": "newbob"}"#);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.body.trim(), r#"{"changed":true,"username":"bob"}"#);
    assert!(logged(r#"console: "ada" set the password of "bob""#));
    assert_eq!(bob.get("/api/session").status, 401);
    assert_eq!(Browser::new(&f).login("bob", "newbob"), 200);
    let (opened, _) = akit_admin::store::Store::open(&path, "secret").unwrap();
    let users = opened.users();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].username, "bob");
    assert!(verifies(&users[0].password_hash, "newbob"));
    assert_eq!(b.get("/api/session").status, 200);
}

/// fit's `TestConsolePasswordNoStore` and `TestSetConsoleUsersWithoutStore`: without a store a
/// change is refused, so that nothing is lost at the next restart.
#[test]
fn password_without_a_store() {
    let f = start(console(|_| {}), Options::default());
    let mut b = Browser::new(&f);
    assert_eq!(b.login("ada", "pw"), 200);
    for (path, body) in [
        (ACCOUNT, r#"{"current_password": "pw", "password": "new"}"#),
        (RESET, r#"{"username": "ada", "password": "new"}"#),
    ] {
        let r = b.post(path, body);
        assert_eq!(r.status, 409, "{path}: {r:?}");
        assert!(
            r.body.contains("passwords change only with a store"),
            "{r:?}"
        );
    }
    assert_eq!(Browser::new(&f).login("ada", "pw"), 200);
    assert!(
        f.configs
            .set_console_users(vec![akit_admin::store::User {
                username: "ada".into(),
                password_hash: "x".into(),
            }])
            .is_err()
    );
    let (users, stored) = f.configs.console_users();
    assert!(users.is_empty() && !stored);
}

/// fit's `TestConsolePasswordStoreFails` and `TestConsoleResetStoreFails`: a change the store
/// cannot keep leaves the login as it was.
#[test]
fn password_store_fails() {
    let (f, _kept, _, _) = start_store("password-fails", |_| {});
    f.configs.use_console_users(
        Vec::new(),
        Box::new(|_: &[akit_admin::store::User]| Err("the disk is full".to_owned())),
    );
    let mut b = Browser::new(&f);
    assert_eq!(b.login("ada", "pw"), 200);
    let r = b.post(ACCOUNT, r#"{"current_password": "pw", "password": "new"}"#);
    assert_eq!(r.status, 500, "{r:?}");
    assert!(r.body.contains("saving the password: the disk is full"));
    assert_eq!(Browser::new(&f).login("ada", "pw"), 200);
    assert_eq!(Browser::new(&f).login("ada", "new"), 401);
    let r = b.post(RESET, r#"{"username": "bob", "password": "new-bob"}"#);
    assert_eq!(r.status, 500, "{r:?}");
    assert!(r.body.contains("saving the password: the disk is full"));
    assert_eq!(Browser::new(&f).login("bob", "bobpw"), 200);
}

/// fit's `TestConsolePasswordLockout` and `TestConsolePasswordResetLockedOut`: the wrong current
/// password counts as a failed login and locks the address out of every password check, a
/// reset's included.
#[test]
fn password_lockout() {
    let (f, _kept, _, _) = start_store("password-lockout", |_| {});
    let mut b = Browser::new(&f);
    assert_eq!(b.login("ada", "pw"), 200);
    let wrong = r#"{"current_password": "wrong", "password": "new"}"#;
    for i in 0..akit_admin::console::login::MAX_FAILURES - 1 {
        assert_eq!(b.post(ACCOUNT, wrong).status, 403, "failure {}", i + 1);
    }
    let r = b.post(ACCOUNT, wrong);
    assert_eq!(r.status, 429, "{r:?}");
    assert!(r.header("Retry-After").is_some());
    assert_eq!(
        b.post(ACCOUNT, r#"{"current_password": "pw", "password": "new"}"#)
            .status,
        429
    );
    assert_eq!(
        b.post(RESET, r#"{"username": "bob", "password": "newbob"}"#)
            .status,
        429
    );
}

/// fit's `TestConsolePasswordBodies`: a change whose body is not the JSON it takes, or names no
/// user, is refused before anything is checked.
#[test]
fn password_bodies() {
    let (f, _kept, _, _) = start_store("password-bodies", |_| {});
    let mut b = Browser::new(&f);
    assert_eq!(b.login("ada", "pw"), 200);
    for (path, body, want) in [
        (
            ACCOUNT,
            r#"{"current_password": "#,
            "the body is not the JSON expected",
        ),
        (RESET, "[", "the body is not the JSON expected"),
        (
            RESET,
            r#"{"password": "new"}"#,
            "the user name and the new password",
        ),
    ] {
        let r = b.post(path, body);
        assert_eq!(r.status, 400, "{body}: {r:?}");
        assert!(r.body.contains(want), "{body}: {r:?}");
    }
    let r = b.send(
        "POST",
        RESET,
        &[("Content-Type", "text/plain")],
        Some(r#"{"username": "bob"}"#),
    );
    assert_eq!(r.status, 415, "{r:?}");
}

/// fit's `TestTemporaryPassword` and `TestConsoleStartsTemporary`: a session with the temporary
/// password `akit run -admin` gives may only change it, and the Admin API refuses everything
/// meanwhile.
#[test]
fn temporary_password() {
    let until = std::time::SystemTime::now() + Duration::from_secs(600);
    let (f, _kept, _, _) = start_store("temporary", |cs| cs.use_temporary_password("ada", until));
    let (status, _, body) = api_call(&f, "GET", "/config", &[], b"");
    assert_eq!(status, 403);
    assert!(String::from_utf8_lossy(&body).contains("temporary password"));
    let mut b = Browser::new(&f);
    let r = b.send(
        "POST",
        "/api/login",
        &[],
        Some(r#"{"username": "ada", "password": "pw"}"#),
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert!(r.body.contains(r#""must_change_password":true"#), "{r:?}");
    assert!(r.body.contains(r#""password_expires":""#), "{r:?}");
    assert_eq!(b.login("ada", "pw"), 200);
    for path in ["/api/m/admin/config", "/api/m/admin/schema"] {
        let r = b.get(path);
        assert_eq!(r.status, 403, "{path}");
        assert!(r.body.contains(r#""must_change_password":true"#), "{r:?}");
    }
    for path in ["/api/session", "/api/workbench", USERS] {
        assert_eq!(b.get(path).status, 200, "{path}");
    }
    let r = b.post(ACCOUNT, r#"{"current_password": "pw", "password": "pw"}"#);
    assert_eq!(r.status, 400);
    assert!(r.body.contains("must not be the temporary one"), "{r:?}");
    let r = b.post(
        ACCOUNT,
        r#"{"current_password": "pw", "password": "new-pw"}"#,
    );
    assert_eq!(r.status, 200, "{r:?}");
    // The same session goes on, with everything open; so does the API.
    let r = b.get("/api/session");
    assert!(r.status == 200 && !r.body.contains("must_change"), "{r:?}");
    assert_eq!(b.get("/api/m/admin/config").status, 200);
    assert!(f.configs.temporary_password().is_none());
    assert_eq!(api_call(&f, "GET", "/config", &[], b"").0, 200);
}

/// fit's `TestTemporaryPassword`, past its time: the login and the change are refused.
#[test]
fn temporary_password_expired() {
    let until = std::time::SystemTime::now() + Duration::from_secs(2);
    let (f, _kept, _, _) = start_store("temporary-expired", |cs| {
        cs.use_temporary_password("ada", until);
    });
    let mut b = Browser::new(&f);
    assert_eq!(b.login("ada", "pw"), 200);
    while std::time::SystemTime::now() < until + Duration::from_millis(100) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let r = Browser::new(&f).send(
        "POST",
        "/api/login",
        &[],
        Some(r#"{"username": "ada", "password": "pw"}"#),
    );
    assert_eq!(r.status, 401, "{r:?}");
    assert!(r.body.contains(r#""password_expired":true"#), "{r:?}");
    let r = b.post(
        ACCOUNT,
        r#"{"current_password": "pw", "password": "new-pw"}"#,
    );
    assert_eq!(r.status, 403, "{r:?}");
    assert!(r.body.contains("expired"), "{r:?}");
}

/// fit's `TestExportImport`: the config and a page's members exported in every format and
/// imported back; `fitstore` files sealed with the running store's key and with a password of
/// their own; every import kept in the store.
#[test]
fn export_and_import() {
    let (f, kept, _, _) = start_store("transfer", |_| {});
    let mut b = Browser::new(&f);
    assert_eq!(b.login("ada", "pw"), 200);
    const EXPORT: &str = "/api/m/admin/config/export";
    const IMPORT: &str = "/api/m/admin/config/import";
    let mut prev = "one".to_owned();
    for format in ["json", "yaml", "conf"] {
        let r = b.post(EXPORT, &format!(r#"{{"format": "{format}"}}"#));
        assert_eq!(r.status, 200, "{r:?}");
        assert!(r.body.contains(&prev), "{r:?}");
        assert!(
            r.header("Content-Disposition")
                .unwrap()
                .contains("akit-config-v")
        );
        let changed = r.body.replacen(&prev, format, 1);
        prev = format.to_owned();
        let r = b.send(
            "POST",
            &format!("{IMPORT}?format={format}"),
            &[("Content-Type", "text/plain")],
            Some(&changed),
        );
        assert_eq!(r.status, 200, "{r:?}\n{changed}");
        assert_eq!(served(&f), format);
    }
    let r = b.post(EXPORT, r#"{"format": "json", "members": ["routes"]}"#);
    assert_eq!(r.status, 200);
    assert!(!r.body.contains("listeners"));
    assert!(
        r.header("Content-Disposition")
            .unwrap()
            .contains("akit-routes-v")
    );
    assert_eq!(
        b.post(EXPORT, r#"{"format": "json", "members": ["admin"]}"#)
            .status,
        400
    );
    assert_eq!(b.post(EXPORT, r#"{"format": "toml"}"#).status, 400);

    // fitstore: with the running store's key, and with a password.
    for pw in ["", "export-pw"] {
        let (status, h, body) = b.send_bytes(
            "POST",
            EXPORT,
            &[("Content-Type", "application/json")],
            format!(r#"{{"format": "store", "password": "{pw}"}}"#).as_bytes(),
        );
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
        assert!(body.starts_with(b"FITSTORE"));
        assert_eq!(header(&h, "Content-Type"), Some("application/octet-stream"));
        let key = if pw.is_empty() { "secret" } else { pw };
        let (data, _) = akit_admin::store::unseal(&body, key).unwrap();
        assert!(String::from_utf8(data).unwrap().contains("routes"));
        let sealed =
            akit_admin::store::seal_with(key, doc(&format!("sealed-{pw}")).as_bytes(), &[], CHEAP)
                .unwrap();
        let mut hdr = vec![("Content-Type", "application/octet-stream")];
        if !pw.is_empty() {
            hdr.push(("Akit-Store-Password", pw));
        }
        let (status, _, res) =
            b.send_bytes("POST", &format!("{IMPORT}?format=store"), &hdr, &sealed);
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&res));
        assert_eq!(served(&f), format!("sealed-{pw}"));
    }
    let sealed = akit_admin::store::seal_with("other", doc("x").as_bytes(), &[], CHEAP).unwrap();
    let (status, _, res) = b.send_bytes(
        "POST",
        &format!("{IMPORT}?format=store"),
        &[("Content-Type", "application/octet-stream")],
        &sealed,
    );
    assert_eq!(status, 400);
    assert!(String::from_utf8_lossy(&res).contains("does not open"));
    // A config that does not load is not applied.
    let r = b.send(
        "POST",
        &format!("{IMPORT}?format=json"),
        &[("Content-Type", "text/plain")],
        Some(r#"{"listeners": [{"name": "web"}]}"#),
    );
    assert_eq!(r.status, 400, "{r:?}");
    // Every import was kept in the store.
    let saved = locked(&kept).read().unwrap();
    assert!(
        String::from_utf8(saved)
            .unwrap()
            .contains("sealed-export-pw")
    );
    // A whole export with the store's key holds the users whose passwords changed.
    let r = b.post(ACCOUNT, r#"{"current_password": "pw", "password": "new"}"#);
    assert_eq!(r.status, 200);
    let (_, _, body) = b.send_bytes(
        "POST",
        EXPORT,
        &[("Content-Type", "application/json")],
        br#"{"format": "store"}"#,
    );
    let (_, users) = akit_admin::store::unseal(&body, "secret").unwrap();
    assert_eq!(users.len(), 1);
    assert!(verifies(&users[0].password_hash, "new"));
}

/// fit's `TestImportStoreBounded`: a file whose header asks for a costly key derivation is
/// refused before deriving it, and a second import while one is being opened is answered 503,
/// rather than both deriving at once; akit runs without a store, so the file's password is
/// asked for.
#[test]
fn import_store_bounded() {
    let f = start(console(|_| {}), Options::default());
    let send = |file: &[u8], pw: Option<&str>| {
        let mut h = vec![("Content-Type", "application/octet-stream")];
        if let Some(p) = pw {
            h.push(("Akit-Store-Password", p));
        }
        api_call(&f, "POST", "/config/import?format=store", &h, file)
    };
    let sealed = akit_admin::store::seal_with("pw", doc("v2").as_bytes(), &[], CHEAP).unwrap();
    let (status, _, body) = send(&sealed, None);
    assert_eq!(status, 400);
    assert!(String::from_utf8_lossy(&body).contains("Akit-Store-Password"));
    // Past twice what akit writes with: refused before anything is derived.
    let mut costly = sealed.clone();
    costly[14..18].copy_from_slice(&(2 * (64 << 10) + 8u32).to_be_bytes());
    let (status, _, body) = send(&costly, Some("pw"));
    assert_eq!(status, 400);
    assert!(String::from_utf8_lossy(&body).contains("more than"));
    // One that takes a while to open, and another meanwhile.
    let slow = akit_admin::store::seal_with(
        "pw",
        doc("v3").as_bytes(),
        &[],
        akit_admin::store::Params {
            time: 6,
            memory_kib: 128 << 10,
            threads: 1,
        },
    )
    .unwrap();
    std::thread::scope(|s| {
        let first = s.spawn(|| send(&slow, Some("pw")));
        std::thread::sleep(Duration::from_millis(300));
        let (status, h, body) = send(&sealed, Some("pw"));
        assert_eq!(status, 503, "{}", String::from_utf8_lossy(&body));
        assert!(header(&h, "Retry-After").is_some());
        assert!(String::from_utf8_lossy(&body).contains("being opened"));
        assert_eq!(first.join().unwrap().0, 200);
    });
    assert_eq!(served(&f), "v3");
    let (status, _, _) = send(&sealed, Some("pw"));
    assert_eq!(status, 200);
    assert_eq!(served(&f), "v2");
    // Exports need a password of their own without a store.
    let (status, _, body) = api_call(
        &f,
        "POST",
        "/config/export",
        &[("Content-Type", "application/json")],
        br#"{"format": "store"}"#,
    );
    assert_eq!(status, 400);
    assert!(String::from_utf8_lossy(&body).contains("give the password"));
}

/// With a store, a config that sets `admin` is refused: it comes from the bootstrap file
/// (`akit run`'s `errAdminInStore`); one without is applied and kept in the store.
#[test]
fn store_keeps_admin_out() {
    let (f, kept, _, _) = start_store("admin-out", |_| {});
    // use_store with an admin: what akit run does.
    let k2 = Arc::clone(&kept);
    f.configs.use_store(
        Some(r#"{"address": "127.0.0.1:0"}"#.to_owned()),
        Box::new(move |d: &str| {
            locked(&k2)
                .write(d.as_bytes().to_vec())
                .map_err(|e| e.to_string())
        }),
    );
    let with_admin = r#"{"admin": {}, "listeners": [{"name": "web", "address": "127.0.0.1:0"}]}"#;
    let (status, _, body) = api_call(
        &f,
        "POST",
        "/load",
        &[("Content-Type", "application/json")],
        with_admin.as_bytes(),
    );
    assert_eq!(status, 400);
    assert!(
        String::from_utf8_lossy(&body)
            .contains("admin is set in the config file akit started with")
    );
    let (status, _, body) = api_call(
        &f,
        "POST",
        "/load",
        &[("Content-Type", "application/json")],
        doc("two").as_bytes(),
    );
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(served(&f), "two");
    assert_eq!(
        String::from_utf8(locked(&kept).read().unwrap()).unwrap(),
        doc("two")
    );
    // A store that cannot be written: the config runs, and is said unsaved.
    f.configs.use_store(
        Some("{}".to_owned()),
        Box::new(|_: &str| Err("the disk is full".to_owned())),
    );
    let (status, _, body) = api_call(
        &f,
        "POST",
        "/load",
        &[("Content-Type", "application/json")],
        doc("three").as_bytes(),
    );
    assert_eq!(status, 200);
    assert!(
        String::from_utf8_lossy(&body).contains("was not saved to the store: the disk is full")
    );
    assert_eq!(served(&f), "three");
}
