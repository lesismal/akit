//! What the assistant's tests share (I163, I164): a stand-in of the Admin API (fit's
//! `fakeConfigs`: versions with ETags, `If-Match`, `PUT /config/text` by members, reconcile,
//! rollback, the cache, render in the three formats), whose changes are answered from another
//! thread as the control thread answers them; a scripted stand-in of a model on port 0; and the
//! assistant's module served over HTTP as the console serves it, the user named by a header.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    unreachable_pub
)]
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use akio::{Engine, Listener};
use akio_http::Context;
use akit_config::value::{Value, parse};
use akit_roles::pages::Module;

use crate::json::{self, Obj, compact, quoted};
use crate::proposal::VERSION_HEADER;
use crate::standin::{self, Script, Turn};
use crate::tree;
use crate::{Api, ApiAnswer, Assistant, Options, Store};

/// A secret of the stand-in's config, which a proposal may carry back where it was and nowhere
/// else.
pub(crate) const SECRET: &str = "$2b$04$abcdefghijklmnopqrstuuG2rmOVKzOv2Hd2t1Gc9i1dK1Szy/7Ri";

/// The config the stand-in starts with: it loads, and holds a secret in a route's middleware.
pub(crate) fn base() -> String {
    format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
"upstreams": [{{"name": "app", "addresses": ["10.0.0.7:9000"]}}],
"routes": [{{"name": "api", "match": {{"path": ["/api/*"]}}, "middleware": [{{"module": "basic_auth", "params": {{"users": [{{"username": "u", "password_hash": "{SECRET}"}}]}}}}],
  "handler": {{"module": "proxy", "params": {{"upstream": "app"}}}}}}],
"admin": {{"address": "127.0.0.1:2019", "max_conns": 8}}}}"#
    )
}

pub(crate) fn etag_of(data: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut h);
    format!("\"{:012x}\"", h.finish() & 0xffff_ffff_ffff)
}

/// What the stand-in of the Admin API holds.
#[derive(Default)]
pub(crate) struct Cfgs {
    /// Version n is `versions[n-1]`.
    pub(crate) versions: Vec<String>,
    /// The changes asked for: method, target, If-Match.
    pub(crate) changes: Vec<String>,
    pub(crate) bodies: Vec<String>,
    pub(crate) check: String,
    /// When not 0, a change answers it.
    pub(crate) fail: u16,
    pub(crate) zones: Vec<String>,
    pub(crate) purges: Vec<String>,
    /// Every request answers 500.
    pub(crate) down: bool,
    /// These requests, "METHOD /path", answer 500.
    pub(crate) broken: Vec<String>,
    /// A purge of a zone fails, as when it went since the check.
    pub(crate) fail_purge: bool,
    /// The check's answer to a config, when not the config's own.
    pub(crate) validate: Option<Check>,
    /// `GET /status`'s answer for a version and its config, when not the fixed one.
    pub(crate) status: Option<Status>,
}

/// The answer of `POST /config/validate` to a config.
pub(crate) type Check = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// The answer of `GET /status` to a version and its config.
pub(crate) type Status = Arc<dyn Fn(u64, &str) -> String + Send + Sync>;

/// The Admin API's stand-in, fit's `fakeConfigs`.
#[derive(Clone, Default)]
pub(crate) struct Fake(pub(crate) Arc<Mutex<Cfgs>>);

impl Fake {
    pub(crate) fn new(data: &str) -> Fake {
        Fake(Arc::new(Mutex::new(Cfgs {
            versions: vec![data.to_owned()],
            check: r#"{"differences":[]}"#.to_owned(),
            zones: vec!["main".to_owned()],
            ..Cfgs::default()
        })))
    }

    pub(crate) fn with<R>(&self, f: impl FnOnce(&mut Cfgs) -> R) -> R {
        f(&mut self.0.lock().unwrap())
    }

    pub(crate) fn current(&self) -> (u64, String) {
        self.with(|c| (c.versions.len() as u64, c.versions.last().unwrap().clone()))
    }

    /// `data` applied as a new version, as a change made elsewhere.
    pub(crate) fn set(&self, data: &str) {
        self.with(|c| c.versions.push(data.to_owned()));
    }

    pub(crate) fn changed(&self) -> Vec<String> {
        self.with(|c| c.changes.clone())
    }
}

pub(crate) fn answer(status: u16, body: &str) -> ApiAnswer {
    ApiAnswer {
        status,
        headers: vec![("Content-Type".into(), "application/json".into())],
        body: body.as_bytes().to_vec(),
    }
}

pub(crate) fn check_config(text: &str) -> String {
    match akit_config::from_json(text, None) {
        Ok(_) => r#"{"valid":true}"#.to_owned(),
        Err(errs) => {
            let items: Vec<String> = errs
                .0
                .iter()
                .map(|e| Obj::new().str("path", &e.path).str("msg", &e.msg).end())
                .collect();
            format!(r#"{{"valid":false,"errors":[{}]}}"#, items.join(","))
        }
    }
}

pub(crate) fn render(format: &str, body: &str) -> Result<String, String> {
    let target = if format == "conf" {
        akit_config::adapt::Target::Conf
    } else {
        akit_config::adapt::Target::Yaml
    };
    akit_config::adapt::document(&[akit_config::Source {
        name: None,
        format: akit_config::Format::Json,
        src: body,
    }])
    .map_err(|e| e.to_string())?
    .write(target)
}

impl Api for Fake {
    fn call(&self, method: &str, target: &str, body: Vec<u8>) -> ApiAnswer {
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let body = String::from_utf8(body).unwrap();
        let c = self.0.lock().unwrap();
        if c.down || c.broken.iter().any(|b| *b == format!("{method} {path}")) {
            return answer(500, "");
        }
        match (method, path) {
            ("GET", "/config") => {
                let data = c.versions.last().unwrap().clone();
                ApiAnswer {
                    status: 200,
                    headers: vec![
                        ("ETag".into(), etag_of(&data)),
                        (VERSION_HEADER.into(), c.versions.len().to_string()),
                    ],
                    body: data.into_bytes(),
                }
            }
            ("GET", p) if p.starts_with("/versions/") => {
                let n: usize = p["/versions/".len()..].parse().unwrap_or(0);
                match c.versions.get(n.wrapping_sub(1)) {
                    Some(v) => answer(200, v),
                    None => answer(404, r#"{"error":"not kept"}"#),
                }
            }
            ("GET", "/config/check") => answer(200, &c.check),
            // The schema the Admin API serves: the console's, fit's renamed.
            ("GET", "/schema") => answer(200, include_str!("../../akit-admin/console/schema.json")),
            ("GET", "/config/effective") => answer(
                200,
                &format!(
                    r#"{{"version":{},"config":{},"sources":[]}}"#,
                    c.versions.len(),
                    c.versions.last().unwrap()
                ),
            ),
            ("GET", "/status") if c.status.is_some() => {
                let f = c.status.as_ref().unwrap();
                answer(200, &f(c.versions.len() as u64, c.versions.last().unwrap()))
            }
            ("GET", "/status") => answer(
                200,
                r#"{"version":1,"listeners":[{"name":"web","address":"127.0.0.1:0"}],"upstreams":[{"name":"app","backends":[{"address":"10.0.0.7:9000","healthy":false,"in_flight":0}]}]}"#,
            ),
            ("GET", "/metrics") => answer(200, "akit_requests_total 1\n"),
            ("GET", "/cache") => {
                let zs: Vec<String> = c
                    .zones
                    .iter()
                    .map(|z| format!(r#"{{"zone":{},"keys":1}}"#, quoted(z)))
                    .collect();
                answer(200, &format!("[{}]", zs.join(",")))
            }
            ("POST", "/config/render") => {
                if body.contains("render-fails") {
                    return answer(400, r#"{"error":"the text cannot be rendered"}"#);
                }
                match render(query.trim_start_matches("format="), &body) {
                    Ok(t) => answer(200, &t),
                    Err(e) => answer(400, &format!(r#"{{"error":{}}}"#, quoted(&e))),
                }
            }
            ("POST", "/config/validate") => match &c.validate {
                Some(f) => answer(200, &f(&body)),
                None => answer(200, &check_config(&body)),
            },
            _ => answer(404, r#"{"error":"no such endpoint"}"#),
        }
    }

    fn change(
        &self,
        method: &str,
        target: &str,
        if_match: &str,
        body: Vec<u8>,
        done: Box<dyn FnOnce(ApiAnswer) + Send>,
    ) {
        let ans = self.changed_by(method, target, if_match, &String::from_utf8(body).unwrap());
        // Answered elsewhere, as the control thread answers a change.
        std::thread::spawn(move || done(ans));
    }
}

impl Fake {
    pub(crate) fn changed_by(
        &self,
        method: &str,
        target: &str,
        if_match: &str,
        body: &str,
    ) -> ApiAnswer {
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let mut c = self.0.lock().unwrap();
        if path == "/cache/purge" {
            c.purges.push(body.to_owned());
            let zone = parse(body)
                .ok()
                .map(|v| json::str_of(&v, "zone").to_owned())
                .unwrap_or_default();
            if !zone.is_empty() && (!c.zones.contains(&zone) || c.fail_purge) {
                return answer(404, r#"{"error":"no cache uses the zone"}"#);
            }
            return answer(200, r#"{"purged":2}"#);
        }
        let cur = c.versions.last().unwrap().clone();
        c.changes.push(format!("{method} {target} {if_match}"));
        c.bodies.push(body.to_owned());
        if if_match != etag_of(&cur) {
            return answer(412, r#"{"error":"the config changed"}"#);
        }
        if c.fail != 0 {
            return answer(
                c.fail,
                r#"{"error":"the config was not applied; the running config stays","errors":[{"path":"listeners[0].address","message":"address in use"}]}"#,
            );
        }
        let data = match (method, path) {
            ("PUT", "/config/text") => {
                let members = query
                    .split('&')
                    .find_map(|kv| kv.strip_prefix("members="))
                    .and_then(akio_http::message::percent_decode)
                    .unwrap_or_default();
                let mut base = parse(&cur).unwrap();
                let Ok(frag) = parse(body) else {
                    return answer(400, r#"{"error":"not JSON"}"#);
                };
                for m in members.split(',') {
                    match frag.get(m) {
                        Some(v) => tree::set(&mut base, m, v.clone()),
                        None => tree::del(&mut base, m),
                    }
                }
                compact(&base)
            }
            ("POST", "/config/reconcile") => cur.replacen("\"1h\"", "\"2h\"", 1),
            ("POST", p) if p.starts_with("/rollback/") => {
                let n: usize = p["/rollback/".len()..].parse().unwrap();
                c.versions[n - 1].clone()
            }
            _ => return answer(404, r#"{"error":"no such change"}"#),
        };
        if let Err(e) = akit_config::from_json(&data, None) {
            return answer(400, &format!(r#"{{"error":{}}}"#, quoted(&e.to_string())));
        }
        c.versions.push(data.clone());
        answer(
            200,
            &format!(
                r#"{{"version":{},"etag":{},"warnings":["a warning"]}}"#,
                c.versions.len(),
                quoted(&etag_of(&data))
            ),
        )
    }
}

/// The store's sections in memory; `fail` refuses every write.
#[derive(Default)]
pub(crate) struct Mem {
    pub(crate) sec: Mutex<HashMap<String, Vec<u8>>>,
    pub(crate) fail: bool,
}

impl Store for Mem {
    fn section(&self, name: &str) -> Option<Vec<u8>> {
        self.sec.lock().unwrap().get(name).cloned()
    }

    fn set_section(&self, name: &str, data: &[u8]) -> Result<(), String> {
        if self.fail {
            return Err("the store is read-only".into());
        }
        self.sec
            .lock()
            .unwrap()
            .insert(name.to_owned(), data.to_vec());
        Ok(())
    }
}

pub(crate) fn client_engine() -> Arc<Engine> {
    static E: OnceLock<Arc<Engine>> = OnceLock::new();
    Arc::clone(E.get_or_init(|| {
        let e = Engine::builder().client_threads(1).build().unwrap();
        e.start().unwrap();
        Arc::new(e)
    }))
}

/// The settings with `extra` (members after the provider, each with a comma before it) and the
/// console's users ada and bob, as `admin` loads them.
pub(crate) fn settings_of(
    model: &str,
    extra: &str,
) -> (akit_config::assistant::Assistant, akit_config::Console) {
    let hash = akio_crypto::bcrypt::hash(b"pw", akio_crypto::bcrypt::MIN_COST).unwrap();
    let text = format!(
        r#"{{"admin": {{"address": "127.0.0.1:0",
            "console": {{"address": "127.0.0.1:0", "users": [{{"username": "ada", "password_hash": "{hash}"}}, {{"username": "bob", "password_hash": "{hash}"}}]}},
            "assistant": {{"providers": [{{"name": "local", "kind": "openai_compatible", "base_url": "{model}/v1", "models": [{{"name": "m"}}]}}]{extra}}}}}}}"#
    );
    let c = akit_config::from_json(&text, None).unwrap();
    let admin = c.admin.unwrap();
    (admin.assistant.unwrap(), admin.console.unwrap())
}

pub(crate) fn options(
    model: &str,
    extra: &str,
    api: Fake,
    store: Option<Arc<Mem>>,
    logs: &Logs,
) -> Options {
    let (settings, console) = settings_of(model, extra);
    let l2 = Arc::clone(logs);
    Options {
        settings,
        console: Some(console),
        store: store.map(|s| s as Arc<dyn Store>),
        running: None,
        log: Arc::new(move |m| l2.lock().unwrap().push(m)),
        engine: Some(client_engine()),
        roots: None,
        limits: crate::model::Limits::default(),
        proxy_addr: None,
        api: Some(Arc::new(api)),
    }
}

pub(crate) type Logs = Arc<Mutex<Vec<String>>>;

/// An assistant on a stand-in of the Admin API and of a model, served over HTTP.
pub(crate) struct Shared {
    pub(crate) a: Assistant,
    pub(crate) cfgs: Fake,
    pub(crate) sc: Script,
    pub(crate) model: standin::Server,
    pub(crate) store: Arc<Mem>,
    pub(crate) logs: Logs,
    pub(crate) addr: SocketAddr,
    pub(crate) _http: Engine,
}

/// One I/O thread each, and stopped once the test is over: the tests run many at once, and a
/// ring of io_uring's each is a file.
impl Drop for Shared {
    fn drop(&mut self) {
        self._http.shutdown();
        self._http.wait();
    }
}

/// [`Shared`] as a user's browser.
pub(crate) struct Fx {
    pub(crate) sh: Arc<Shared>,
    pub(crate) user: &'static str,
}

impl std::ops::Deref for Fx {
    type Target = Shared;

    fn deref(&self) -> &Shared {
        &self.sh
    }
}

/// Serves `a`'s module over HTTP, the user named by `X-Test-User` (ada without it).
pub(crate) fn serve(
    a: Assistant,
    cfgs: Fake,
    sc: Script,
    model: standin::Server,
    store: Arc<Mem>,
    logs: Logs,
) -> Fx {
    let m = a.clone();
    let service = akio_http::serve(move |ctx: &mut Context| {
        let user = ctx.header("X-Test-User").unwrap_or("ada").to_owned();
        let path = ctx
            .path()
            .strip_prefix("/api/m/ai")
            .unwrap_or("")
            .to_owned();
        m.serve(ctx, &user, &path);
    });
    let http = Engine::builder()
        .io_threads(1)
        .listen(
            Listener::tcp("127.0.0.1:0".parse().unwrap())
                .threads(1)
                .serve(service),
        )
        .build()
        .unwrap();
    http.start().unwrap();
    let addr = http.local_addrs()[0];
    Fx {
        sh: Arc::new(Shared {
            a,
            cfgs,
            sc,
            model,
            store,
            logs,
            addr,
            _http: http,
        }),
        user: "ada",
    }
}

pub(crate) fn start_with(extra: &str, turns: Vec<Turn>, tune: impl FnOnce(&mut Options)) -> Fx {
    let (model, sc) = standin::script(turns);
    let cfgs = Fake::new(&base());
    let store = Arc::new(Mem::default());
    let logs = Logs::default();
    let mut o = options(
        &model.url(),
        extra,
        cfgs.clone(),
        Some(Arc::clone(&store)),
        &logs,
    );
    tune(&mut o);
    serve(Assistant::new(o), cfgs, sc, model, store, logs)
}

/// [`start_with`], on the Admin API's stand-in `cfgs`.
pub(crate) fn start_on(
    cfgs: Fake,
    extra: &str,
    turns: Vec<Turn>,
    tune: impl FnOnce(&mut Options),
) -> Fx {
    let (model, sc) = standin::script(turns);
    let store = Arc::new(Mem::default());
    let logs = Logs::default();
    let mut o = options(
        &model.url(),
        extra,
        cfgs.clone(),
        Some(Arc::clone(&store)),
        &logs,
    );
    tune(&mut o);
    serve(Assistant::new(o), cfgs, sc, model, store, logs)
}

pub(crate) fn start(extra: &str, turns: Vec<Turn>) -> Fx {
    start_with(extra, turns, |_| {})
}

pub(crate) fn calls(calls: &[(&str, &str)]) -> Turn {
    Turn::Tools(
        calls
            .iter()
            .map(|(n, a)| ((*n).to_owned(), (*a).to_owned()))
            .collect(),
    )
}

pub(crate) fn text(t: &str) -> Turn {
    Turn::Text(t.to_owned())
}

/// A chat's events: each SSE message's event name (empty for `message`) and its data.
pub(crate) struct Chat {
    pub(crate) events: Vec<(String, Value)>,
}

impl Chat {
    pub(crate) fn of(&self, typ: &str) -> Vec<&Value> {
        self.events
            .iter()
            .filter(|(n, d)| n.is_empty() && json::str_of(d, "type") == typ)
            .map(|(_, d)| d)
            .collect()
    }

    pub(crate) fn end(&self) -> Option<&Value> {
        self.events.iter().find(|(n, _)| n == "end").map(|(_, d)| d)
    }

    pub(crate) fn err(&self) -> Option<&Value> {
        self.events
            .iter()
            .find(|(n, _)| n == "error")
            .map(|(_, d)| d)
    }

    pub(crate) fn conv(&self) -> String {
        json::str_of(self.of("start")[0], "conversation").to_owned()
    }
}

pub(crate) fn s<'v>(v: &'v Value, k: &str) -> &'v str {
    json::str_of(v, k)
}

pub(crate) fn n(v: &Value, k: &str) -> u64 {
    match v.get(k) {
        Some(Value::Number(x)) => x.parse().unwrap(),
        _ => 0,
    }
}

pub(crate) fn b(v: &Value, k: &str) -> bool {
    matches!(v.get(k), Some(Value::Bool(true)))
}

pub(crate) fn arr<'v>(v: &'v Value, k: &str) -> &'v [Value] {
    match v.get(k) {
        Some(Value::Array(a)) => a,
        _ => &[],
    }
}

pub(crate) fn strs(v: &Value, k: &str) -> Vec<String> {
    arr(v, k)
        .iter()
        .map(|x| match x {
            Value::String(s) => s.clone(),
            other => compact(other),
        })
        .collect()
}

/// `v` with its member `k` set to `x`.
pub(crate) fn with(v: &Value, k: &str, x: Value) -> Value {
    let mut v = v.clone();
    tree::set(&mut v, k, x);
    v
}

/// The system prompt of a request to the model, as Chat Completions carries it.
pub(crate) fn system(r: &standin::Req) -> String {
    let v = r.json().unwrap();
    arr(&v, "messages")
        .iter()
        .find(|m| s(m, "role") == "system")
        .map(|m| s(m, "content").to_owned())
        .unwrap_or_default()
}

/// Whether a request to the model offers the tool `name`.
pub(crate) fn has_tool(r: &standin::Req, name: &str) -> bool {
    let v = r.json().unwrap();
    arr(&v, "tools")
        .iter()
        .any(|t| t.get("function").is_some_and(|f| s(f, "name") == name))
}

pub(crate) fn dechunk(mut s: &str) -> String {
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

impl Fx {
    /// The same assistant, as `user`'s browser.
    pub(crate) fn as_user(&self, user: &'static str) -> Fx {
        Fx {
            sh: Arc::clone(&self.sh),
            user,
        }
    }

    pub(crate) fn model_calls(&self) -> Vec<standin::Req> {
        self.model
            .requests()
            .into_iter()
            .filter(|r| r.method == "POST")
            .collect()
    }

    pub(crate) fn no_secrets(&self) {
        for r in self.model.requests() {
            assert!(
                !String::from_utf8_lossy(&r.body).contains(SECRET),
                "a secret reached the model"
            );
        }
    }

    pub(crate) fn send(&self, method: &str, path: &str, body: &str) -> (u16, String) {
        let mut st = TcpStream::connect(self.addr).unwrap();
        st.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        write!(
            st,
            "{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nX-Test-User: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            self.user,
            body.len()
        )
        .unwrap();
        let mut raw = Vec::new();
        let _ = st.read_to_end(&mut raw);
        let t = String::from_utf8_lossy(&raw).into_owned();
        let (head, body) = t.split_once("\r\n\r\n").expect(&t);
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

    pub(crate) fn get(&self, path: &str) -> (u16, String) {
        self.send("GET", &format!("/api/m/ai{path}"), "")
    }

    pub(crate) fn post(&self, path: &str, body: &str) -> (u16, String) {
        self.send("POST", &format!("/api/m/ai{path}"), body)
    }

    pub(crate) fn chat(&self, body: &str) -> Chat {
        let (status, body) = self.post("/api/chat", body);
        assert_eq!(status, 200, "{body}");
        let events = body
            .split("\n\n")
            .filter_map(|m| {
                let mut name = String::new();
                let mut data = None;
                for l in m.lines() {
                    if let Some(x) = l.strip_prefix("event: ") {
                        name = x.to_owned();
                    } else if let Some(d) = l.strip_prefix("data: ") {
                        data = parse(d).ok();
                    }
                }
                data.map(|d| (name, d))
            })
            .collect();
        Chat { events }
    }

    /// The conversation's proposal as the browser reads it (`conv` may carry `?user=`).
    pub(crate) fn view(&self, conv: &str) -> (u16, Option<Value>, String) {
        let (status, body) = self.get(&format!("/api/proposals/{conv}"));
        let p = parse(&body)
            .ok()
            .and_then(|v| v.get("proposal").cloned())
            .filter(|p| *p != Value::Null);
        (status, p, body)
    }

    /// Confirms the proposal `v` shows.
    pub(crate) fn apply_it(&self, conv: &str, v: &Value) -> (u16, Value) {
        let body = Obj::new()
            .str("id", s(v, "id"))
            .num("revision", n(v, "revision"))
            .str("etag", s(v, "etag"))
            .str("user", s(v, "user"))
            .end();
        let (status, out) = self.post(&format!("/api/proposals/{conv}/apply"), &body);
        (status, parse(&out).unwrap_or(Value::Null))
    }

    pub(crate) fn audit(&self, query: &str) -> Vec<Value> {
        let (status, body) = self.get(&format!("/api/audit{query}"));
        assert_eq!(status, 200, "{body}");
        arr(&parse(&body).unwrap(), "entries").to_vec()
    }

    pub(crate) fn log_text(&self) -> String {
        self.logs.lock().unwrap().join("\n")
    }

    /// Has the user's chat make a proposal of `steps`, and returns its conversation and view.
    pub(crate) fn propose_in(&self, steps: &[(&str, &str)]) -> (String, Value) {
        self.sc.set(vec![calls(steps), text("Proposed.")]);
        let c = self.chat(r#"{"message": "change it"}"#);
        assert!(c.end().is_some(), "{:?}", c.err());
        let conv = c.conv();
        let (_, v, body) = self.view(&conv);
        (conv, v.unwrap_or_else(|| panic!("no proposal: {body}")))
    }
}
