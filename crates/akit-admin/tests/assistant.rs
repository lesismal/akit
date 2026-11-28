//! The console's assistant (I160), with the settings' cases of fit's `assistant/assistant_test.go`
//! (`TestSettingsView`, `TestSettingsPut`, `TestSettingsFromStore`): the module `ai` served only
//! with `admin.assistant`, its settings read and saved into the store's section `assistant`, the
//! keys never answered, a stale version refused, settings that do not load refused with their
//! paths, and a section as fit writes it read. On the backend `AKIO_TEST_BACKEND` names.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use akio::{Backend, UringConfig};
use akit_admin::{Configs, Options, SOURCE_FILE, Server};
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

/// The store's sections, in memory.
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

const KEY: &str = "sk-test-0123456789abcdef";

/// The engine the model calls go out on, as `akit run` gives the listeners'.
fn engine() -> Arc<akio::Engine> {
    static E: std::sync::OnceLock<Arc<akio::Engine>> = std::sync::OnceLock::new();
    Arc::clone(E.get_or_init(|| {
        let e = akio::Engine::builder().client_threads(1).build().unwrap();
        e.start().unwrap();
        Arc::new(e)
    }))
}

/// A console with `admin.assistant` set to `assistant` (none when empty), its store `store`.
fn start(assistant: &str, store: Option<Arc<Sections>>) -> (Server, SocketAddr) {
    start_full(assistant, store, "", None)
}

/// [`start`], with `top` more top-level members of the config, and where `llm.proxy`'s API is
/// bound.
fn start_full(
    assistant: &str,
    store: Option<Arc<Sections>>,
    top: &str,
    proxy: Option<akit_assistant::ProxyAddr>,
) -> (Server, SocketAddr) {
    let console = format!(
        r#""console": {{"address": "127.0.0.1:0", "users": [{{"username": "ada", "password_hash": "{}"}}, {{"username": "bob", "password_hash": "{}"}}]}}"#,
        akio_crypto::bcrypt::hash(b"pw", akio_crypto::bcrypt::MIN_COST).unwrap(),
        akio_crypto::bcrypt::hash(b"pw", akio_crypto::bcrypt::MIN_COST).unwrap()
    );
    let more = if assistant.is_empty() {
        String::new()
    } else {
        format!(r#", "assistant": {assistant}"#)
    };
    let text = format!(r#"{{"admin": {{"address": "127.0.0.1:0", {console}{more}}}{top}}}"#);
    let config = akit_config::from_json(&text, None).unwrap();
    let settings = config.admin.clone().unwrap();
    let configs = Configs::new(
        akit_config::value::standardize(&text).unwrap(),
        SOURCE_FILE,
        10,
    );
    let submit: akit_admin::Submit = Box::new(Err);
    let mut opts = Options::default();
    opts.backend = Some(backend());
    opts.assistant_store = store.map(|s| s as Arc<dyn akit_assistant::Store>);
    opts.assistant_engine = Some(engine());
    opts.assistant_proxy_addr = proxy;
    let api = Server::start(&settings, configs, submit, &opts).unwrap();
    let url = api.console_url().unwrap();
    let addr = url
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .parse()
        .unwrap();
    (api, addr)
}

struct Resp {
    status: u16,
    body: String,
    set_cookie: Option<String>,
}

/// A logged-in browser.
struct Browser {
    addr: SocketAddr,
    cookie: String,
    csrf: String,
}

impl Browser {
    fn login(addr: SocketAddr, user: &str) -> Browser {
        let mut b = Browser {
            addr,
            cookie: String::new(),
            csrf: String::new(),
        };
        let r = b.send(
            "POST",
            "/api/login",
            Some(&format!(r#"{{"username": "{user}", "password": "pw"}}"#)),
        );
        assert_eq!(r.status, 200, "{}", r.body);
        let set = r.set_cookie.unwrap();
        b.cookie = set.split(';').next().unwrap().to_owned();
        let Some(Value::String(c)) = parse(&r.body).unwrap().get("csrf").cloned() else {
            panic!("{}", r.body)
        };
        b.csrf = c;
        b
    }

    fn send(&self, method: &str, path: &str, body: Option<&str>) -> Resp {
        let mut s = TcpStream::connect(self.addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut req = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
            self.addr
        );
        if !self.cookie.is_empty() {
            req.push_str(&format!("Cookie: {}\r\n", self.cookie));
        }
        if !self.csrf.is_empty() && method != "GET" {
            req.push_str(&format!("X-Akit-Csrf: {}\r\n", self.csrf));
        }
        if let Some(b) = body {
            req.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                b.len()
            ));
        }
        req.push_str("\r\n");
        req.push_str(body.unwrap_or(""));
        s.write_all(req.as_bytes()).unwrap();
        let mut raw = Vec::new();
        let _ = s.read_to_end(&mut raw);
        let raw = String::from_utf8_lossy(&raw).into_owned();
        let (head, body) = raw.split_once("\r\n\r\n").expect(&raw);
        let status = head.split(' ').nth(1).unwrap().parse().unwrap();
        let set_cookie = head
            .lines()
            .find_map(|l| l.strip_prefix("Set-Cookie: "))
            .map(str::to_owned);
        Resp {
            status,
            body: body.to_owned(),
            set_cookie,
        }
    }

    fn get(&self, path: &str) -> Resp {
        self.send("GET", path, None)
    }

    /// A provider's test, `provider` as the page sends it.
    fn test(&self, provider: &str, key: &str, more: &str) -> Resp {
        self.send(
            "POST",
            "/api/m/ai/providers/test",
            Some(&format!(
                r#"{{"provider": {provider}, "key": "{key}"{more}}}"#
            )),
        )
    }
}

fn json(r: &Resp) -> Value {
    parse(&r.body).unwrap_or_else(|_| panic!("{}", r.body))
}

fn str_at<'v>(v: &'v Value, path: &[&str]) -> &'v str {
    let mut v = v;
    for k in path {
        v = v.get(k).unwrap_or_else(|| panic!("no {k}"));
    }
    match v {
        Value::String(s) => s,
        other => panic!("{other:?}"),
    }
}

fn arr<'v>(v: &'v Value, k: &str) -> &'v [Value] {
    match v.get(k) {
        Some(Value::Array(a)) => a,
        other => panic!("{k}: {other:?}"),
    }
}

/// A provider whose key is in the environment variable `PATH`, which every test process has, so
/// that no test sets the environment.
const SETTINGS: &str = r#"{"providers": [{"name": "local", "kind": "openai_compatible", "base_url": "http://127.0.0.1:9/v1", "api_key_env": "PATH", "models": [{"name": "m"}]}], "users": [{"username": "ada", "level": "apply"}]}"#;

/// Without `admin.assistant` there is no module `ai`; with it, the workbench lists it after the
/// config's own and its settings are those of the config, unsaved without a store.
#[test]
fn served_only_when_set() {
    let (api, addr) = start("", None);
    let b = Browser::login(addr, "ada");
    let w = b.get("/api/workbench");
    assert!(!w.body.contains("\"ai\""), "{}", w.body);
    assert_eq!(b.get("/api/m/ai/settings").status, 404);
    api.shutdown();

    let (api, addr) = start(SETTINGS, None);
    let b = Browser::login(addr, "ada");
    let w = b.get("/api/workbench");
    assert!(w.body.contains(r#""modules":["admin","ai"]"#), "{}", w.body);
    let r = b.get("/api/m/ai/settings");
    assert_eq!(r.status, 200, "{}", r.body);
    let v = json(&r);
    assert_eq!(str_at(&v, &["source"]), "config");
    assert_eq!(v.get("store"), Some(&Value::Bool(false)));
    assert_eq!(str_at(&v, &["version"]), "0");
    assert_eq!(arr(v.get("settings").unwrap(), "providers").len(), 1);
    // Without a store nothing is saved.
    let r = b.send("PUT", "/api/m/ai/settings", Some(r#"{"settings": {}}"#));
    assert_eq!(r.status, 409, "{}", r.body);
    assert!(r.body.contains("admin.store"), "{}", r.body);
    assert_eq!(b.send("POST", "/api/m/ai/settings", Some("{}")).status, 405);
    assert_eq!(b.get("/api/m/ai/nothing").status, 404);
    // Not without a session.
    let anon = Browser {
        addr,
        cookie: String::new(),
        csrf: String::new(),
    };
    assert_eq!(anon.get("/api/m/ai/settings").status, 401);
    api.shutdown();
}

/// The settings answer the config's settings, kinds, levels, users and defaults; a key from its
/// environment variable shows masked, never whole.
#[test]
fn settings_view() {
    let path = std::env::var("PATH").unwrap();
    assert!(path.chars().count() >= 16, "PATH is too short for the test");
    let (api, addr) = start(SETTINGS, None);
    let b = Browser::login(addr, "bob");
    let r = b.get("/api/m/ai/settings");
    assert_eq!(r.status, 200);
    assert!(!r.body.contains(&path), "a key answered: {}", r.body);
    let v = json(&r);
    let local = v.get("keys").unwrap().get("local").unwrap();
    assert_eq!(local.get("set"), Some(&Value::Bool(true)));
    assert_eq!(str_at(local, &["source"]), "env");
    let tail: String = path.chars().skip(path.chars().count() - 4).collect();
    assert_eq!(str_at(local, &["masked"]), format!("••••{tail}"));
    assert_eq!(str_at(local, &["env"]), "PATH");
    assert_eq!(
        str_at(&v, &["defaults", "anthropic"]),
        "https://api.anthropic.com/v1"
    );
    assert_eq!(str_at(&v, &["defaults", "llm_proxy"]), "");
    assert_eq!(arr(&v, "kinds").len(), 8);
    assert_eq!(arr(&v, "levels").len(), 5);
    assert_eq!(arr(&v, "users").len(), 2);
    assert_eq!(v.get("llm_proxy"), Some(&Value::Null));
    api.shutdown();
}

fn section(store: &Sections) -> Value {
    let raw = akit_assistant::Store::section(store, "assistant").unwrap();
    parse(std::str::from_utf8(&raw).unwrap()).unwrap()
}

/// fit's `TestSettingsPut`: the settings saved whole into the store with the keys, used at once
/// and after a restart; the key never answered; a stale version, settings that do not load and a
/// key for no provider refused, the store unchanged.
#[test]
fn settings_put() {
    let store = Arc::new(Sections::default());
    let (api, addr) = start(SETTINGS, Some(Arc::clone(&store)));
    let b = Browser::login(addr, "ada");
    let v = json(&b.get("/api/m/ai/settings"));
    assert_eq!(v.get("store"), Some(&Value::Bool(true)));
    let new = r#"{"providers": [{"name": "local", "kind": "openai_compatible", "base_url": "http://127.0.0.1:9/v1", "models": [{"name": "m2"}]}, {"name": "claude", "kind": "anthropic"}], "default_level": "off"}"#;
    let r = b.send(
        "PUT",
        "/api/m/ai/settings",
        Some(&format!(
            r#"{{"settings": {new}, "keys": {{"claude": "{KEY}"}}, "version": "0"}}"#
        )),
    );
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(!r.body.contains(KEY), "a key answered: {}", r.body);
    let v = json(&r);
    assert_eq!(str_at(&v, &["source"]), "store");
    assert_eq!(str_at(&v, &["version"]), "1");
    assert_eq!(str_at(&v, &["keys", "claude", "source"]), "store");
    assert_eq!(str_at(&v, &["keys", "claude", "masked"]), "••••cdef");
    assert_eq!(
        v.get("keys").unwrap().get("local").unwrap().get("set"),
        Some(&Value::Bool(false))
    );
    // The store holds fit's document: the settings as sent, the key.
    let sec = section(&store);
    assert_eq!(str_at(&sec, &["keys", "claude"]), KEY);
    assert_eq!(str_at(&sec, &["settings", "default_level"]), "off");
    // The page read version 0, which is no longer the one in use.
    let r = b.send(
        "PUT",
        "/api/m/ai/settings",
        Some(&format!(r#"{{"settings": {new}, "version": "0"}}"#)),
    );
    assert_eq!(r.status, 412, "{}", r.body);
    // Settings that do not load: each error at its path; nothing saved.
    let r = b.send(
        "PUT",
        "/api/m/ai/settings",
        Some(r#"{"settings": {"providers": [{"name": "x", "kind": "nope"}], "users": [{"username": "eve", "level": "read"}]}}"#),
    );
    assert_eq!(r.status, 400, "{}", r.body);
    let v = json(&r);
    assert_eq!(str_at(&v, &["error"]), "the settings are not valid");
    let paths: Vec<&str> = arr(&v, "errors")
        .iter()
        .map(|e| str_at(e, &["path"]))
        .collect();
    assert!(
        paths.contains(&"admin.assistant.providers[0].kind")
            && paths.contains(&"admin.assistant.users[0].username"),
        "{paths:?}"
    );
    let r = b.send(
        "PUT",
        "/api/m/ai/settings",
        Some(&format!(
            r#"{{"settings": {new}, "keys": {{"gone": "k"}}}}"#
        )),
    );
    assert_eq!(r.status, 400, "{}", r.body);
    assert_eq!(str_at(&section(&store), &["keys", "claude"]), KEY);
    // A restart reads them from the store.
    api.shutdown();
    let (api, addr) = start(SETTINGS, Some(Arc::clone(&store)));
    let b = Browser::login(addr, "ada");
    let v = json(&b.get("/api/m/ai/settings"));
    assert_eq!(str_at(&v, &["source"]), "store");
    assert_eq!(str_at(&v, &["settings", "default_level"]), "off");
    assert_eq!(str_at(&v, &["keys", "claude", "source"]), "store");
    // An empty key removes the one kept; one not named is kept.
    let r = b.send(
        "PUT",
        "/api/m/ai/settings",
        Some(&format!(
            r#"{{"settings": {new}, "keys": {{"claude": ""}}}}"#
        )),
    );
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(section(&store).get("keys").is_none());
    api.shutdown();
}

/// fit's `TestSettingsFromStore`: a section as fit's assistant writes it opens with its settings
/// and keys; one whose settings no longer load, or that is not the assistant's, leaves the
/// config's in use.
#[test]
fn settings_from_store() {
    // What fit's `save` writes (`json.Marshal(kept{...})`): the settings as the page sent them,
    // compact, and the keys by provider.
    let fit = format!(
        r#"{{"settings":{{"providers":[{{"name":"o","kind":"ollama","models":[{{"name":"llama3"}}]}}],"users":[{{"username":"bob","level":"propose"}}]}},"keys":{{"o":"{KEY}"}}}}"#
    );
    let store = Arc::new(Sections::default());
    akit_assistant::Store::set_section(&*store, "assistant", fit.as_bytes()).unwrap();
    let (api, addr) = start(SETTINGS, Some(Arc::clone(&store)));
    let b = Browser::login(addr, "ada");
    let v = json(&b.get("/api/m/ai/settings"));
    assert_eq!(str_at(&v, &["source"]), "store");
    let users = arr(v.get("settings").unwrap(), "users");
    assert_eq!(str_at(&users[0], &["username"]), "bob");
    assert_eq!(str_at(&v, &["keys", "o", "source"]), "store");
    assert_eq!(str_at(&v, &["keys", "o", "masked"]), "••••cdef");
    api.shutdown();
    // Settings that no longer load (a user who is no console user): the config's are used, the
    // keys still read, the section left as it is.
    let stale = format!(
        r#"{{"settings":{{"users":[{{"username":"eve","level":"read"}}]}},"keys":{{"local":"{KEY}"}}}}"#
    );
    akit_assistant::Store::set_section(&*store, "assistant", stale.as_bytes()).unwrap();
    let (api, addr) = start(SETTINGS, Some(Arc::clone(&store)));
    let b = Browser::login(addr, "ada");
    let v = json(&b.get("/api/m/ai/settings"));
    assert_eq!(str_at(&v, &["source"]), "config");
    assert_eq!(str_at(&v, &["keys", "local", "source"]), "store");
    assert_eq!(section(&store), parse(&stale).unwrap());
    api.shutdown();
    // A section that is not the assistant's.
    akit_assistant::Store::set_section(&*store, "assistant", b"[1, 2]").unwrap();
    let (api, addr) = start(SETTINGS, Some(Arc::clone(&store)));
    let b = Browser::login(addr, "ada");
    let v = json(&b.get("/api/m/ai/settings"));
    assert_eq!(str_at(&v, &["source"]), "config");
    assert_eq!(str_at(&v, &["keys", "local", "source"]), "env");
    api.shutdown();
}

/// The section between akit and fit's own code: with `AKIT_ASSISTANT_DIR=<dir>`, akit's Settings
/// page saves settings and a key and writes the section to `<dir>/akit.section`, for
/// `tests/fitassistant/akit_assistant_test.go` to open in fit's package `assistant` (at the gate's
/// pin), which writes back `<dir>/fit.section`; run again, this test opens fit's.
#[test]
fn fit_interop() {
    let Ok(dir) = std::env::var("AKIT_ASSISTANT_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let fit = dir.join("fit.section");
    if let Ok(data) = std::fs::read(&fit) {
        let store = Arc::new(Sections::default());
        akit_assistant::Store::set_section(&*store, "assistant", &data).unwrap();
        let (api, addr) = start(SETTINGS, Some(Arc::clone(&store)));
        let b = Browser::login(addr, "ada");
        let r = b.get("/api/m/ai/settings");
        assert!(!r.body.contains(KEY), "{}", r.body);
        let v = json(&r);
        assert_eq!(str_at(&v, &["source"]), "store");
        assert_eq!(str_at(&v, &["keys", "o", "source"]), "store");
        assert_eq!(str_at(&v, &["keys", "o", "masked"]), "••••cdef");
        let users = arr(v.get("settings").unwrap(), "users");
        assert_eq!(str_at(&users[0], &["level"]), "propose");
        api.shutdown();
        return;
    }
    let store = Arc::new(Sections::default());
    let (api, addr) = start(SETTINGS, Some(Arc::clone(&store)));
    let b = Browser::login(addr, "ada");
    let r = b.send(
        "PUT",
        "/api/m/ai/settings",
        Some(&format!(
            r#"{{"settings": {{"providers": [{{"name": "claude", "kind": "anthropic", "models": [{{"name": "claude-x"}}]}}], "default_level": "off"}}, "keys": {{"claude": "{KEY}"}}}}"#
        )),
    );
    assert_eq!(r.status, 200, "{}", r.body);
    let sec = akit_assistant::Store::section(&*store, "assistant").unwrap();
    std::fs::write(dir.join("akit.section"), sec).unwrap();
    api.shutdown();
}

/// fit's `TestProviderTest` (I161): a provider, not saved, tried: its model list, then a short
/// call to a model, against a stand-in of its API; the key typed goes only to the API, never
/// into the answer; a wrong key fails both, said without the key; the list alone with
/// `models_only`; a provider that is not one is refused with its errors.
#[test]
fn provider_test() {
    let fake = akit_assistant::standin::fake_api(KEY);
    let (api, addr) = start(SETTINGS, None);
    let b = Browser::login(addr, "ada");
    let base = akit_assistant::standin::base_url(&fake.url(), "openai_compatible");
    let provider = format!(r#"{{"name": "t", "kind": "openai_compatible", "base_url": "{base}"}}"#);
    let r = b.test(&provider, KEY, "");
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(!r.body.contains(KEY), "{}", r.body);
    let v = json(&r);
    assert_eq!(v.get("ok"), Some(&Value::Bool(true)), "{}", r.body);
    assert_eq!(arr(&v, "models").len(), 2);
    assert_eq!(str_at(&v, &["model"]), "gpt-test");
    assert_eq!(str_at(&v, &["reply"]), "The listener web is up.");
    assert!(v.get("usage").is_some() && v.get("call_ms").is_some());
    // The call went with the key, as the API's own header.
    let calls = fake.requests();
    let chat = calls.iter().find(|q| q.method == "POST").unwrap();
    assert_eq!(
        chat.header("Authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
    // The list alone.
    let v = json(&b.test(&provider, KEY, r#", "models_only": true"#));
    assert_eq!(v.get("ok"), Some(&Value::Bool(true)));
    assert!(v.get("model").is_none() && v.get("reply").is_none());
    // A wrong key: the API says it back, akit does not.
    let wrong = "sk-wrong-0123456789abcdef";
    let r = b.test(&provider, wrong, r#", "model": "gpt-test""#);
    assert!(!r.body.contains(wrong), "{}", r.body);
    let v = json(&r);
    assert_eq!(v.get("ok"), Some(&Value::Bool(false)));
    assert!(str_at(&v, &["models_error"]).contains("401"), "{}", r.body);
    assert!(str_at(&v, &["call_error"]).contains("***"), "{}", r.body);
    // Not a provider.
    let r = b.test(r#"{"name": "-x", "kind": "nope"}"#, "", "");
    assert_eq!(r.status, 400, "{}", r.body);
    assert!(
        r.body.contains("admin.assistant.providers[0].kind"),
        "{}",
        r.body
    );
    // An llm_proxy provider with no llm.proxy running says so.
    let v = json(&b.test(r#"{"name": "mesh", "kind": "llm_proxy"}"#, "k", ""));
    assert!(
        str_at(&v, &["models_error"]).contains("no llm.proxy"),
        "{v:?}"
    );
    api.shutdown();
}

/// At most four tests at once (fit's `MaxTests`): a fifth is refused while four wait on an API
/// that does not answer, and a browser that leaves ends its test, its call cancelled.
#[test]
fn provider_tests_bounded() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let gone = Arc::new(AtomicUsize::new(0));
    let g2 = Arc::clone(&gone);
    let slow = akit_assistant::standin::Server::start(move |_, c| {
        if c.wait_gone(Duration::from_secs(10)) {
            g2.fetch_add(1, Ordering::SeqCst);
        }
    });
    let (api, addr) = start(SETTINGS, None);
    let b = Browser::login(addr, "ada");
    let provider = format!(
        r#"{{"name": "t", "kind": "openai_compatible", "base_url": "{}/v1"}}"#,
        slow.url()
    );
    let body = format!(r#"{{"provider": {provider}, "key": ""}}"#);
    let mut held = Vec::new();
    for _ in 0..4 {
        let mut s = TcpStream::connect(addr).unwrap();
        let req = format!(
            "POST /api/m/ai/providers/test HTTP/1.1\r\nHost: {addr}\r\nCookie: {}\r\nX-Akit-Csrf: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            b.cookie,
            b.csrf,
            body.len()
        );
        s.write_all(req.as_bytes()).unwrap();
        held.push(s);
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while slow.requests().len() < 4 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(slow.requests().len(), 4);
    let r = b.test(&provider, "", "");
    assert_eq!(r.status, 429, "{}", r.body);
    // The browsers leave: their calls are cancelled, and the slots come back.
    drop(held);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while gone.load(Ordering::SeqCst) < 4 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        gone.load(Ordering::SeqCst),
        4,
        "the calls were not cancelled"
    );
    let fake = akit_assistant::standin::fake_api(KEY);
    let base = akit_assistant::standin::base_url(&fake.url(), "openai_compatible");
    let r = b.test(
        &format!(r#"{{"name": "t", "kind": "openai_compatible", "base_url": "{base}"}}"#),
        KEY,
        r#", "models_only": true"#,
    );
    assert_eq!(r.status, 200, "{}", r.body);
    api.shutdown();
}

/// An `llm_proxy` provider is called on loopback at the port `llm.proxy`'s API is bound to,
/// whatever its config's address says (port 0 here), as an OpenAI-compatible API.
#[test]
fn llm_proxy_resolved() {
    let fake = akit_assistant::standin::fake_api(KEY);
    let bound = fake.addr();
    let (api, addr) = start_full(
        SETTINGS,
        None,
        r#", "llm": {"proxy": {"api": {"address": "127.0.0.1:0"}, "peers": [{"address": "127.0.0.1:1", "password": "p", "role": "hub"}]}}"#,
        Some(Arc::new(move || Some(bound))),
    );
    let b = Browser::login(addr, "ada");
    let v = json(&b.get("/api/m/ai/settings"));
    assert_eq!(str_at(&v, &["llm_proxy", "address"]), "127.0.0.1:0");
    let r = b.test(
        r#"{"name": "mesh", "kind": "llm_proxy", "models": [{"name": "gpt-test"}]}"#,
        KEY,
        "",
    );
    let v = json(&r);
    assert_eq!(v.get("ok"), Some(&Value::Bool(true)), "{}", r.body);
    assert_eq!(str_at(&v, &["reply"]), "The listener web is up.");
    assert!(
        fake.requests()
            .iter()
            .any(|q| q.path == "/v1/chat/completions")
    );
    api.shutdown();
}
