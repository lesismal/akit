//! Providers (I57), with fit's `admin/provider_test.go` ported: `TestProviders` (parts merged
//! into the base config and served, names shadowed, the effective config and where each part came
//! from, a broken part refused with its errors where it wrote them, loads, rollbacks and a base
//! config that breaks a provider's route) and `TestFileAndProviders` (the file provider and a
//! provider at once). A control thread applies the API's changes, the providers' parts (taken
//! from `akit_provider::Updates`) and the file's, as `akit run`'s does. On the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use akio::{Backend, UringConfig};
use akit_admin::{
    Applied, ApplyError, Configs, Job, ListenerChange, Mode, Options, Outcome, Refused,
    SOURCE_ADMIN, SOURCE_FILE, SOURCE_ROLLBACK, Server, VERSION_HEADER, WatchOptions, Watcher,
};
use akit_config::value::Value;
use akit_listener::Listeners;
use akit_provider::{Update, Updates};
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

/// A route for `path` answering `body`, named `name` when it is not empty, with `more` members
/// when it is not empty: fit's `route`.
fn route(name: &str, path: &str, body: &str, more: &str) -> String {
    let n = if name.is_empty() {
        String::new()
    } else {
        format!(r#""name": "{name}", "#)
    };
    let more = if more.is_empty() {
        String::new()
    } else {
        format!(", {more}")
    };
    format!(
        r#"{{{n}"match": {{"path": ["{path}"]}}, "handler": {{"module": "respond", "params": {{"body": "{body}"}}}}{more}}}"#
    )
}

/// A part with one route, as [`route`] makes it: fit's `fakeRoute`.
fn fake_route(name: &str, path: &str, body: &str) -> String {
    format!(r#"{{"routes": [{}]}}"#, route(name, path, body, ""))
}

/// Applies `text`, or checks it, as the control thread of `akit run` does.
fn apply(text: &str, mode: Mode, listeners: &mut Listeners) -> Result<Applied, ApplyError> {
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
        warnings: Vec::new(),
    })
}

/// What the control thread is asked to do.
enum Msg {
    Job(Job),
    /// Providers' parts wait.
    Provide,
    /// A provider's part applied at once, its outcome sent back.
    Now(String, Vec<u8>, mpsc::Sender<Result<Outcome, Refused>>),
    Stop,
}

/// A running server with its configs, its Admin API, the providers' updates and the control
/// thread that applies what changes, as `akit run` has them.
struct Fixture {
    configs: Configs,
    api: Server,
    updates: Updates,
    web: SocketAddr,
    tx: mpsc::SyncSender<Msg>,
    control: Option<JoinHandle<()>>,
    engine: Arc<akio::Engine>,
    /// What the control thread could not apply, as `akit run` would report it.
    refused: Arc<Mutex<Vec<String>>>,
}

/// Starts `doc`; with `watch`, the file it was read from is watched, as `akit run -watch` does.
fn start(doc: &str, watch: Option<PathBuf>) -> Fixture {
    let config = akit_config::from_json(doc, None).unwrap();
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
        16,
    );
    let (tx, rx) = mpsc::sync_channel::<Msg>(64);
    let t = tx.clone();
    let submit: akit_admin::Submit = Box::new(move |job| match t.try_send(Msg::Job(job)) {
        Ok(()) => Ok(()),
        Err(
            mpsc::TrySendError::Full(Msg::Job(j)) | mpsc::TrySendError::Disconnected(Msg::Job(j)),
        ) => Err(j),
        Err(_) => Ok(()),
    });
    let t = tx.clone();
    let updates = Updates::new(move || {
        let _ = t.try_send(Msg::Provide);
    });
    let mut opts = Options::default();
    opts.backend = Some(backend());
    let settings = akit_config::Admin {
        address: "127.0.0.1:0".into(),
        ..akit_config::Admin::default()
    };
    let api = Server::start(&settings, configs.clone(), submit, &opts).unwrap();
    let cs = configs.clone();
    let ups = updates.clone();
    let refused = Arc::new(Mutex::new(Vec::new()));
    let report = Arc::clone(&refused);
    let control = std::thread::spawn(move || {
        let mut watcher = watch.map(|path| {
            let paths = vec![path.display().to_string()];
            let files = akit_config::read_config_files(&paths).unwrap();
            let mut o = WatchOptions::default();
            o.debounce = Duration::from_millis(30);
            Watcher::new(&paths, &files, o)
        });
        loop {
            while let Some((name, data)) = ups.take() {
                if let Err(e) =
                    cs.provide(&name, &data, |text, mode| apply(text, mode, &mut listeners))
                {
                    report.lock().unwrap().push(format!("{name}: {e:?}"));
                }
            }
            if let Some(w) = &mut watcher {
                w.tick(Instant::now(), |files| {
                    let (_, json) = akit_config::load_config_files(files).unwrap();
                    cs.update(SOURCE_FILE, None, json, |text| {
                        apply(text, Mode::Apply, &mut listeners)
                    })
                    .map(|_| ())
                    .map_err(|e| report.lock().unwrap().push(format!("file: {e:?}")))
                });
            }
            match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(Msg::Job(job)) => {
                    job.run(&cs, |text, mode| apply(text, mode, &mut listeners));
                }
                Ok(Msg::Now(name, data, reply)) => {
                    let r =
                        cs.provide(&name, &data, |text, mode| apply(text, mode, &mut listeners));
                    let _ = reply.send(r);
                }
                Ok(Msg::Provide) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Ok(Msg::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            cell.collect();
        }
    });
    Fixture {
        configs,
        api,
        updates,
        web,
        tx,
        control: Some(control),
        engine,
        refused,
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.api.shutdown();
        self.updates.close();
        let _ = self.tx.send(Msg::Stop);
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

/// One request on a connection of its own, `Connection: close`.
fn request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Resp {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\nHost: {addr}\r\n");
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
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let raw = String::from_utf8_lossy(&raw);
    let (head, body) = raw.split_once("\r\n\r\n").expect(&raw);
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

impl Fixture {
    fn get(&self, path: &str) -> Resp {
        request(self.api.addr(), "GET", path, &[], None)
    }

    fn load(&self, config: &str) -> Resp {
        request(
            self.api.addr(),
            "POST",
            "/load",
            &[("Content-Type", "application/json")],
            Some(config),
        )
    }

    /// What the listener `web` answers at `path`: fit's `servedAt`.
    fn served_at(&self, path: &str) -> String {
        request(self.web, "GET", path, &[], None).body
    }

    /// A provider's part applied by the control thread, as `Configs.Provide` in fit's tests.
    fn provide(&self, name: &str, data: &str) -> Result<Outcome, Refused> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(Msg::Now(name.into(), data.as_bytes().to_vec(), tx))
            .unwrap();
        rx.recv_timeout(Duration::from_secs(10)).unwrap()
    }

    /// `GET /config/effective`, parsed.
    fn effective(&self) -> Value {
        let r = self.get("/config/effective");
        assert_eq!(r.status, 200, "{r:?}");
        assert!(r.header(VERSION_HEADER).is_some(), "{r:?}");
        assert_eq!(r.header("Content-Type"), Some("application/json"), "{r:?}");
        akit_config::value::parse(&r.body).unwrap_or_else(|e| panic!("{e:?}: {}", r.body))
    }

    /// Waits until `path` answers `want`: fit's `waitServed`.
    fn wait_served(&self, path: &str, want: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let got = self.served_at(path);
            if got == want {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{path} served {got:?}, want {want:?}; refused: {:?}",
                self.refused.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn string(v: &Value) -> &str {
    match v {
        Value::String(s) => s,
        other => panic!("not a string: {other:?}"),
    }
}

fn array(v: Option<&Value>) -> &[Value] {
    match v {
        Some(Value::Array(a)) => a,
        None => &[],
        other => panic!("not an array: {other:?}"),
    }
}

fn number(v: Option<&Value>) -> u64 {
    match v {
        Some(Value::Number(n)) => n.parse().unwrap(),
        other => panic!("not a number: {other:?}"),
    }
}

/// Whether the effective config `e` says `path` came from `source`, at `from` in it.
fn has_source(e: &Value, path: &str, source: &str, from: &str) -> bool {
    array(e.get("sources")).iter().any(|s| {
        s.get("path").map(string) == Some(path)
            && s.get("source").map(string) == Some(source)
            && s.get("from").map_or("", string) == from
    })
}

fn precedence(e: &Value) -> Vec<&str> {
    array(e.get("precedence")).iter().map(string).collect()
}

/// fit's `TestProviders`.
#[test]
fn providers() {
    let f = start(&doc("v1"), None);
    // Before any provider, the effective config is the base config, all from the file.
    let e = f.effective();
    assert_eq!(precedence(&e), [SOURCE_FILE]);
    assert!(has_source(&e, "/listeners/0", SOURCE_FILE, ""), "{e:?}");
    assert!(e.get("shadowed").is_none(), "{e:?}");

    for name in ["fake", "other"] {
        f.configs.add_provider(name).unwrap();
    }
    for name in [
        "fake",
        "file",
        "admin",
        "rollback",
        "Bad",
        "",
        "a/b",
        &"a".repeat(64),
    ] {
        assert!(f.configs.add_provider(name).is_err(), "{name:?} was taken");
    }
    assert!(matches!(
        f.provide("nope", "{}"),
        Err(Refused::Request { status: 404, .. })
    ));
    let etag = f.configs.current().etag.clone();

    // A provider's route is served beside the base config's.
    let fake = fake_route("fr", "/p", "from fake");
    let o = f.provide("fake", &fake).unwrap();
    assert_eq!(
        (o.version, o.etag.as_str(), &*o.source, o.unchanged),
        (2, etag.as_str(), "fake", false)
    );
    assert_eq!(f.served_at("/p"), "from fake");
    assert_eq!(f.served_at("/"), "v1");
    let r = f.get("/config");
    assert_eq!(
        (r.body.as_str(), r.header("ETag")),
        (doc("v1").as_str(), Some(etag.as_str())),
        "GET /config is not the base config"
    );

    // The other provider's route named as fake's is left out; its other route is served.
    let other = format!(
        r#"{{"routes": [{}, {}]}}"#,
        route("fr", "/p", "from other", ""),
        route("", "/q", "from other", r#""listeners": ["web"]"#)
    );
    f.provide("other", &other).unwrap();
    assert_eq!(f.served_at("/p"), "from fake");
    assert_eq!(f.served_at("/q"), "from other");
    let e = f.effective();
    assert_eq!(number(e.get("version")), 3);
    assert_eq!(e.get("etag").map(string), Some(etag.as_str()));
    assert_eq!(precedence(&e), [SOURCE_FILE, "fake", "other"]);
    for (path, source, from) in [
        ("/listeners/0", SOURCE_FILE, ""),
        ("/routes/0", SOURCE_FILE, ""),
        ("/routes/1", "fake", "/routes/0"),
        ("/routes/2", "other", "/routes/1"),
    ] {
        assert!(has_source(&e, path, source, from), "{path}: {e:?}");
    }
    let shadowed = array(e.get("shadowed"));
    assert_eq!(shadowed.len(), 1, "{e:?}");
    let s = &shadowed[0];
    assert_eq!(
        ["source", "from", "name", "by"].map(|k| s.get(k).map_or("", string)),
        ["other", "/routes/0", "fr", "fake"]
    );
    let config = e.get("config").unwrap();
    assert_eq!(
        (
            array(config.get("routes")).len(),
            array(config.get("listeners")).len()
        ),
        (3, 1),
        "{config:?}"
    );
    // The effective config loads as it is.
    akit_config::from_json(&config.to_json(), None).unwrap();

    // The same part again is not applied again.
    let o = f.provide("fake", &fake).unwrap();
    assert_eq!((o.version, o.unchanged), (3, true));

    // A broken part is refused with its errors where it wrote them, and what the provider
    // provided before stays.
    let Err(Refused::Apply(ApplyError::Config(errs))) =
        f.provide("fake", "{\"routes\": [{\"hanlder\": {}}]}")
    else {
        panic!("a broken part was applied")
    };
    let e = errs
        .as_slice()
        .iter()
        .find(|e| e.path.starts_with("routes[0].hanlder"))
        .unwrap_or_else(|| panic!("{errs:?}"));
    assert!(e.msg.ends_with("(provided by fake)"), "{e:?}");
    assert_eq!((e.line, e.col), (1, 14), "{e:?}");
    assert_eq!(f.served_at("/p"), "from fake");
    assert_eq!(f.configs.current().number, 3);

    // A config loaded through the API keeps what providers provide.
    let r = f.load(&doc("v2"));
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(f.served_at("/"), "v2");
    assert_eq!(f.served_at("/p"), "from fake");
    assert_eq!(f.served_at("/q"), "from other");
    let e = f.effective();
    assert_eq!(precedence(&e)[0], SOURCE_ADMIN);
    assert!(has_source(&e, "/routes/0", SOURCE_ADMIN, ""), "{e:?}");

    // A base config that breaks what a provider provides is refused, with the error in the
    // provider's part.
    let r = f.load(&doc("v3").replacen("\"web\"", "\"main\"", 1));
    assert_eq!(r.status, 400, "{r:?}");
    assert!(
        r.body.contains(r#""path":"routes[1].listeners[0]""#)
            && r.body.contains("(provided by other)"),
        "{}",
        r.body
    );

    // A rollback brings back the base config, and keeps what providers provide.
    let r = request(f.api.addr(), "POST", "/rollback/1", &[], None);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(f.served_at("/"), "v1");
    assert_eq!(f.served_at("/p"), "from fake");
    let e = f.effective();
    assert_eq!(precedence(&e)[0], SOURCE_FILE);
    assert_eq!(&*f.configs.current().source, SOURCE_ROLLBACK);
    // So does a reload, and the base config's source stays.
    let r = request(f.api.addr(), "POST", "/reload", &[], None);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(precedence(&f.effective())[0], SOURCE_FILE);
    assert_eq!(f.served_at("/q"), "from other");

    // A dry run checks the base config with what providers provide.
    let r = request(
        f.api.addr(),
        "POST",
        "/load?dry_run=1",
        &[("Content-Type", "application/json")],
        Some(&doc("v4").replacen("\"web\"", "\"main\"", 1)),
    );
    assert!(
        r.status == 400 && r.body.contains("(provided by other)"),
        "{r:?}"
    );

    // A provider that provides nothing any more takes its routes away.
    f.provide("fake", "").unwrap();
    assert_eq!(
        f.served_at("/p"),
        "from other",
        "once fake provides nothing, the other provider's route is no longer shadowed"
    );
    assert!(f.effective().get("shadowed").is_none());
}

/// fit's `TestFileAndProviders`: the file provider and a provider together: an edit to the file
/// and what the provider sends are merged, and the effective config says where each part came
/// from.
#[test]
fn file_and_providers() {
    let dir = std::env::temp_dir().join(format!("akit-providers-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("akit.json");
    std::fs::write(&path, doc("v1")).unwrap();
    let f = start(&doc("v1"), Some(path.clone()));
    f.configs.add_provider("fake").unwrap();
    let fake: Update = f.updates.update("fake");
    // Sent from a thread of the provider's own, as a provider's watch would.
    let send = |data: String| {
        let u = fake.clone();
        std::thread::spawn(move || u.send(data.into_bytes()))
            .join()
            .unwrap();
    };

    send(fake_route("", "/p", "from fake"));
    f.wait_served("/p", "from fake");
    std::fs::write(&path, doc("v2")).unwrap();
    f.wait_served("/", "v2");
    assert_eq!(f.served_at("/p"), "from fake");
    let e = f.effective();
    assert!(has_source(&e, "/routes/0", SOURCE_FILE, ""), "{e:?}");
    assert!(has_source(&e, "/routes/1", "fake", "/routes/0"), "{e:?}");
    assert!(has_source(&e, "/listeners/0", SOURCE_FILE, ""), "{e:?}");
    send(fake_route("", "/p", "changed"));
    f.wait_served("/p", "changed");
    assert!(f.refused.lock().unwrap().is_empty());
    drop(f);
    std::fs::remove_dir_all(&dir).unwrap();
}
