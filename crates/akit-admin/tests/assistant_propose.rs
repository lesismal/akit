//! The assistant's proposals applied through the Admin API in process (I163): the steps a
//! scripted stand-in of a model proposes are checked on the running config and shown as a diff;
//! the apply goes to the control thread as the console's change, with `If-Match`, the secrets
//! the model saw redacted put back; a config changed meanwhile makes the proposal stale and
//! offered again; a rollback is applied as a new version; each apply is in the audit log.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use akio::{Backend, UringConfig};
use akit_admin::{Applied, ApplyError, Configs, Job, Options, SOURCE_FILE, Server};
use akit_assistant::standin::{Script, Turn, script};
use akit_config::value::{Value, parse};

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

fn engine() -> Arc<akio::Engine> {
    static E: std::sync::OnceLock<Arc<akio::Engine>> = std::sync::OnceLock::new();
    Arc::clone(E.get_or_init(|| {
        let e = akio::Engine::builder().client_threads(1).build().unwrap();
        e.start().unwrap();
        Arc::new(e)
    }))
}

#[derive(Default)]
struct Sections(Mutex<HashMap<String, Vec<u8>>>);

impl akit_assistant::Store for Sections {
    fn section(&self, name: &str) -> Option<Vec<u8>> {
        self.0.lock().unwrap().get(name).cloned()
    }

    fn set_section(&self, name: &str, data: &[u8]) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .insert(name.to_owned(), data.to_vec());
        Ok(())
    }
}

/// A route's password hash: the secret the model sees redacted, and the applied config keeps.
const ROUTE_PASSWORD_HASH: &str = "$2b$04$abcdefghijklmnopqrstuuG2rmOVKzOv2Hd2t1Gc9i1dK1Szy/7Ri";

/// The control thread's apply step: a config that loads is applied.
fn apply(text: &str) -> Result<Applied, ApplyError> {
    akit_config::from_json(text, None)
        .map(|_| Applied::default())
        .map_err(ApplyError::Config)
}

struct Fixture {
    api: Server,
    addr: SocketAddr,
    configs: Configs,
    sc: Script,
    _store: Arc<Sections>,
    jobs: mpsc::SyncSender<Option<Job>>,
    control: Option<std::thread::JoinHandle<()>>,
    _model: akit_assistant::standin::Server,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.api.shutdown();
        let _ = self.jobs.send(None);
        if let Some(c) = self.control.take() {
            let _ = c.join();
        }
    }
}

/// `akit run`'s console with an assistant whose model answers `turns`, ada's level apply, and a
/// control thread that applies the changes it is handed.
fn start(turns: Vec<Turn>) -> Fixture {
    let (model, sc) = script(turns);
    let hash = akio_crypto::bcrypt::hash(b"pw", akio_crypto::bcrypt::MIN_COST).unwrap();
    let text = format!(
        r#"{{"admin": {{"address": "127.0.0.1:0",
            "console": {{"address": "127.0.0.1:0", "users": [{{"username": "ada", "password_hash": "{hash}"}}]}},
            "assistant": {{"providers": [{{"name": "local", "kind": "openai_compatible", "base_url": "{}/v1", "models": [{{"name": "m"}}]}}],
              "users": [{{"username": "ada", "level": "apply"}}]}}}},
          "listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
          "upstreams": [{{"name": "app", "addresses": ["127.0.0.1:9"]}}],
          "routes": [{{"name": "app", "match": {{"path": ["/app"]}},
            "middleware": [{{"module": "basic_auth", "params": {{"users": [{{"username": "u", "password_hash": "{ROUTE_PASSWORD_HASH}"}}]}}}}],
            "handler": {{"module": "respond", "params": {{"body": "ok"}}}}}}]}}"#,
        model.url()
    );
    let config = akit_config::from_json(&text, None).unwrap();
    let settings = config.admin.clone().unwrap();
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
    let store = Arc::new(Sections::default());
    let mut opts = Options::default();
    opts.backend = Some(backend());
    opts.assistant_engine = Some(engine());
    opts.assistant_store = Some(Arc::clone(&store) as Arc<dyn akit_assistant::Store>);
    let api = Server::start(&settings, configs.clone(), submit, &opts).unwrap();
    let cs = configs.clone();
    let control = std::thread::spawn(move || {
        while let Ok(Some(job)) = rx.recv() {
            job.run(&cs, |text, _| apply(text));
        }
    });
    let url = api.console_url().unwrap();
    let addr = url
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .parse()
        .unwrap();
    Fixture {
        api,
        addr,
        configs,
        sc,
        _store: store,
        jobs,
        control: Some(control),
        _model: model,
    }
}

struct Browser {
    addr: SocketAddr,
    cookie: String,
    csrf: String,
}

fn split(raw: &[u8]) -> (u16, String) {
    let text = String::from_utf8_lossy(raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").expect(&text);
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    let body = if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        dechunk(body)
    } else {
        body.to_owned()
    };
    (status, body)
}

fn dechunk(mut s: &str) -> String {
    let mut out = String::new();
    while let Some((size, rest)) = s.split_once("\r\n") {
        let Ok(n) = usize::from_str_radix(size.trim(), 16) else {
            break;
        };
        if n == 0 || rest.len() < n {
            break;
        }
        out.push_str(&rest[..n]);
        s = rest.get(n + 2..).unwrap_or("");
    }
    out
}

fn send(b: &Browser, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut s = TcpStream::connect(b.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nCookie: {}\r\nX-Akit-Csrf: {}\r\n",
        b.addr, b.cookie, b.csrf
    );
    if let Some(body) = body {
        req.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    req.push_str("\r\n");
    req.push_str(body.unwrap_or(""));
    s.write_all(req.as_bytes()).unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    split(&raw)
}

fn login(addr: SocketAddr, user: &str) -> Browser {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let body = format!(r#"{{"username": "{user}", "password": "pw"}}"#);
    write!(
        s,
        "POST /api/login HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut raw = String::new();
    let _ = s.read_to_string(&mut raw);
    let cookie = raw
        .lines()
        .find_map(|l| l.strip_prefix("Set-Cookie: "))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let v = parse(raw.split_once("\r\n\r\n").unwrap().1).unwrap();
    let Some(Value::String(csrf)) = v.get("csrf") else {
        panic!("{raw}")
    };
    Browser {
        addr,
        cookie,
        csrf: csrf.clone(),
    }
}

fn s<'v>(v: &'v Value, k: &str) -> &'v str {
    match v.get(k) {
        Some(Value::String(s)) => s,
        _ => "",
    }
}

fn n(v: &Value, k: &str) -> u64 {
    match v.get(k) {
        Some(Value::Number(x)) => x.parse().unwrap(),
        _ => 0,
    }
}

fn b(v: &Value, k: &str) -> bool {
    matches!(v.get(k), Some(Value::Bool(true)))
}

/// Has ada's chat make a proposal of `calls`, and returns its conversation and view.
fn propose(f: &Fixture, br: &Browser, calls: &[(&str, &str)]) -> (String, Value) {
    f.sc.set(vec![
        Turn::Tools(
            calls
                .iter()
                .map(|(n, a)| ((*n).to_owned(), (*a).to_owned()))
                .collect(),
        ),
        Turn::Text("Proposed.".into()),
    ]);
    let (status, body) = send(
        br,
        "POST",
        "/api/m/ai/chat",
        Some(r#"{"message": "change it"}"#),
    );
    assert_eq!(status, 200, "{body}");
    let start = body
        .lines()
        .find_map(|l| l.strip_prefix("data: "))
        .map(|d| parse(d).unwrap())
        .unwrap();
    let conv = s(&start, "conversation").to_owned();
    assert!(body.contains("\"type\":\"proposal\""), "{body}");
    let (status, body) = send(br, "GET", &format!("/api/m/ai/proposals/{conv}"), None);
    assert_eq!(status, 200, "{body}");
    assert!(
        !body.contains(ROUTE_PASSWORD_HASH),
        "a secret in the view: {body}"
    );
    let v = parse(&body).unwrap().get("proposal").cloned().unwrap();
    (conv, v)
}

fn apply_it(br: &Browser, conv: &str, v: &Value) -> (u16, Value) {
    let body = format!(
        r#"{{"id": "{}", "revision": {}, "etag": {}}}"#,
        s(v, "id"),
        n(v, "revision"),
        akit_assistant::json::quoted(s(v, "etag"))
    );
    let (status, out) = send(
        br,
        "POST",
        &format!("/api/m/ai/proposals/{conv}/apply"),
        Some(&body),
    );
    (status, parse(&out).unwrap_or(Value::Null))
}

/// A proposal goes as the console's change through the control thread, with If-Match; the
/// route copied whole has its secret put back; a config changed since makes it stale, offered
/// again on the new version; a rollback is a new version; each apply is in the audit log.
#[test]
fn proposals_applied_in_process() {
    let f = start(vec![Turn::Text("-".into())]);
    let br = login(f.addr, "ada");

    // A route added, and the route there copied whole with its password hash as the model saw
    // it, redacted.
    let (conv, v) = propose(
        &f,
        &br,
        &[
            (
                "config_propose",
                r#"{"summary": "Add /health", "ops": [{"op": "add", "path": "/routes/-", "value": {"name": "health", "match": {"path": ["/health"]}, "handler": {"module": "respond", "params": {"body": "ok"}}}}]}"#,
            ),
            (
                "config_propose",
                r#"{"summary": "A second backend", "members": {"upstreams": [{"name": "app", "addresses": ["127.0.0.1:9", "127.0.0.1:10"]}]}}"#,
            ),
            (
                "cache_purge_propose",
                r#"{"summary": "Purge", "all": true}"#,
            ),
        ],
    );
    assert!(
        b(&v, "valid")
            && s(&v, "kind") == "patch"
            && n(&v, "version") == 1
            && !s(&v, "etag").is_empty(),
        "{v:?}"
    );
    let conf = s(v.get("diff").unwrap(), "conf");
    assert!(
        conf.contains("--- version 1") && conf.contains("/health"),
        "{conf}"
    );
    let (status, out) = apply_it(&br, &conv, &v);
    assert!(
        status == 200 && s(&out, "outcome") == "applied" && n(&out, "version") == 2,
        "{status} {out:?}"
    );
    let cur = f.configs.current();
    let data = cur.data();
    assert!(
        data.contains("\"health\"")
            && data.contains("127.0.0.1:10")
            && data.contains(ROUTE_PASSWORD_HASH),
        "{data}"
    );

    // Stale: the config changed after the proposal was checked.
    let (conv, v) = propose(
        &f,
        &br,
        &[(
            "config_propose",
            r#"{"summary": "A third backend", "ops": [{"op": "add", "path": "/upstreams/0/addresses/-", "value": "127.0.0.1:11"}]}"#,
        )],
    );
    let moved = data.replace("\"ok\"", "\"fine\"");
    f.configs.update(SOURCE_FILE, None, moved, apply).unwrap();
    let (status, out) = apply_it(&br, &conv, &v);
    let again = out.get("proposal").cloned().unwrap();
    assert!(
        status == 412 && s(&out, "outcome") == "stale" && n(&again, "version") == 3,
        "{status} {out:?}"
    );
    let (status, out) = apply_it(&br, &conv, &again);
    assert!(status == 200 && n(&out, "version") == 4, "{status} {out:?}");
    assert!(f.configs.current().data().contains("127.0.0.1:11"));

    // A rollback to the first version, as a new one.
    let (conv, v) = propose(
        &f,
        &br,
        &[(
            "rollback_propose",
            r#"{"summary": "Back to 1", "version": 1}"#,
        )],
    );
    assert!(
        s(&v, "kind") == "rollback" && n(&v, "rollback") == 1,
        "{v:?}"
    );
    let (status, out) = apply_it(&br, &conv, &v);
    assert!(status == 200 && n(&out, "version") == 5, "{status} {out:?}");
    let back = f.configs.current();
    assert!(!back.data().contains("\"health\"") && back.data().contains(ROUTE_PASSWORD_HASH));

    // The audit log: each apply, kept with the store.
    let (status, body) = send(&br, "GET", "/api/m/ai/audit?kind=apply", None);
    let log = parse(&body).unwrap();
    let Some(Value::Array(entries)) = log.get("entries") else {
        panic!("{body}")
    };
    let outcomes: Vec<&str> = entries.iter().map(|e| s(e, "outcome")).collect();
    assert!(
        status == 200 && outcomes == ["applied", "applied", "stale", "applied"],
        "{body}"
    );
    assert!(b(&log, "kept"), "{body}");
}
