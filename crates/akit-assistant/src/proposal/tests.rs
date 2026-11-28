//! fit's `proposals_test.go`, `proposals_check_test.go`, `proposals_edge_test.go`,
//! `proposals_more_test.go`, `apply_test.go` and `audit_test.go`, ported: an assistant on a
//! stand-in of the Admin API (fit's `fakeConfigs`: versions with ETags, `If-Match`, `PUT
//! /config/text` by members, reconcile, rollback, the cache, render in the three formats), whose
//! changes are answered from another thread as the control thread answers them, and a scripted
//! stand-in of a model on port 0; its module served over HTTP as the console serves it, the user
//! named by a header.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::too_many_lines
)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use akio::{Engine, Listener};
use akio_http::Context;
use akit_config::value::{Value, parse};
use akit_roles::pages::Module;

use super::*;
use crate::audit::{self, Filter};
use crate::conv::Conversation;
use crate::standin::{self, Script, Turn};
use crate::{Api, ApiAnswer, Assistant, Options, Store};

/// A secret of the stand-in's config, which a proposal may carry back where it was and nowhere
/// else.
const SECRET: &str = "$2b$04$abcdefghijklmnopqrstuuG2rmOVKzOv2Hd2t1Gc9i1dK1Szy/7Ri";

/// The config the stand-in starts with: it loads, and holds a secret in a route's middleware.
fn base() -> String {
    format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
"upstreams": [{{"name": "app", "addresses": ["10.0.0.7:9000"]}}],
"routes": [{{"name": "api", "match": {{"path": ["/api/*"]}}, "middleware": [{{"module": "basic_auth", "params": {{"users": [{{"username": "u", "password_hash": "{SECRET}"}}]}}}}],
  "handler": {{"module": "proxy", "params": {{"upstream": "app"}}}}}}],
"admin": {{"address": "127.0.0.1:2019", "max_conns": 8}}}}"#
    )
}

fn etag_of(data: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut h);
    format!("\"{:012x}\"", h.finish() & 0xffff_ffff_ffff)
}

/// What the stand-in of the Admin API holds.
#[derive(Default)]
struct Cfgs {
    /// Version n is `versions[n-1]`.
    versions: Vec<String>,
    /// The changes asked for: method, target, If-Match.
    changes: Vec<String>,
    bodies: Vec<String>,
    check: String,
    /// When not 0, a change answers it.
    fail: u16,
    zones: Vec<String>,
    purges: Vec<String>,
    /// Every request answers 500.
    down: bool,
    /// These requests, "METHOD /path", answer 500.
    broken: Vec<String>,
    /// A purge of a zone fails, as when it went since the check.
    fail_purge: bool,
    /// The check's answer to a config, when not the config's own.
    validate: Option<fn(&str) -> String>,
}

/// The Admin API's stand-in, fit's `fakeConfigs`.
#[derive(Clone, Default)]
struct Fake(Arc<Mutex<Cfgs>>);

impl Fake {
    fn new(data: &str) -> Fake {
        Fake(Arc::new(Mutex::new(Cfgs {
            versions: vec![data.to_owned()],
            check: r#"{"differences":[]}"#.to_owned(),
            zones: vec!["main".to_owned()],
            ..Cfgs::default()
        })))
    }

    fn with<R>(&self, f: impl FnOnce(&mut Cfgs) -> R) -> R {
        f(&mut self.0.lock().unwrap())
    }

    fn current(&self) -> (u64, String) {
        self.with(|c| (c.versions.len() as u64, c.versions.last().unwrap().clone()))
    }

    /// `data` applied as a new version, as a change made elsewhere.
    fn set(&self, data: &str) {
        self.with(|c| c.versions.push(data.to_owned()));
    }

    fn changed(&self) -> Vec<String> {
        self.with(|c| c.changes.clone())
    }
}

fn answer(status: u16, body: &str) -> ApiAnswer {
    ApiAnswer {
        status,
        headers: vec![("Content-Type".into(), "application/json".into())],
        body: body.as_bytes().to_vec(),
    }
}

fn check_config(text: &str) -> String {
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

fn render(format: &str, body: &str) -> Result<String, String> {
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
            ("POST", "/config/validate") => match c.validate {
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
    fn changed_by(&self, method: &str, target: &str, if_match: &str, body: &str) -> ApiAnswer {
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
struct Mem {
    sec: Mutex<HashMap<String, Vec<u8>>>,
    fail: bool,
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

fn client_engine() -> Arc<Engine> {
    static E: OnceLock<Arc<Engine>> = OnceLock::new();
    Arc::clone(E.get_or_init(|| {
        let e = Engine::builder().client_threads(1).build().unwrap();
        e.start().unwrap();
        Arc::new(e)
    }))
}

/// The settings with `extra` (members after the provider, each with a comma before it) and the
/// console's users ada and bob, as `admin` loads them.
fn settings_of(
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

fn options(model: &str, extra: &str, api: Fake, store: Option<Arc<Mem>>, logs: &Logs) -> Options {
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

type Logs = Arc<Mutex<Vec<String>>>;

/// An assistant on a stand-in of the Admin API and of a model, served over HTTP.
struct Shared {
    a: Assistant,
    cfgs: Fake,
    sc: Script,
    model: standin::Server,
    store: Arc<Mem>,
    logs: Logs,
    addr: SocketAddr,
    _http: Engine,
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
struct Fx {
    sh: Arc<Shared>,
    user: &'static str,
}

impl std::ops::Deref for Fx {
    type Target = Shared;

    fn deref(&self) -> &Shared {
        &self.sh
    }
}

/// Serves `a`'s module over HTTP, the user named by `X-Test-User` (ada without it).
fn serve(
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

fn start_with(extra: &str, turns: Vec<Turn>, tune: impl FnOnce(&mut Options)) -> Fx {
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

fn start(extra: &str, turns: Vec<Turn>) -> Fx {
    start_with(extra, turns, |_| {})
}

fn tools(calls: &[(&str, &str)]) -> Turn {
    Turn::Tools(
        calls
            .iter()
            .map(|(n, a)| ((*n).to_owned(), (*a).to_owned()))
            .collect(),
    )
}

fn text(t: &str) -> Turn {
    Turn::Text(t.to_owned())
}

/// A chat's events: each SSE message's event name (empty for `message`) and its data.
struct Chat {
    events: Vec<(String, Value)>,
}

impl Chat {
    fn of(&self, typ: &str) -> Vec<&Value> {
        self.events
            .iter()
            .filter(|(n, d)| n.is_empty() && json::str_of(d, "type") == typ)
            .map(|(_, d)| d)
            .collect()
    }

    fn end(&self) -> Option<&Value> {
        self.events.iter().find(|(n, _)| n == "end").map(|(_, d)| d)
    }

    fn err(&self) -> Option<&Value> {
        self.events
            .iter()
            .find(|(n, _)| n == "error")
            .map(|(_, d)| d)
    }

    fn conv(&self) -> String {
        json::str_of(self.of("start")[0], "conversation").to_owned()
    }
}

fn s<'v>(v: &'v Value, k: &str) -> &'v str {
    json::str_of(v, k)
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

fn arr<'v>(v: &'v Value, k: &str) -> &'v [Value] {
    match v.get(k) {
        Some(Value::Array(a)) => a,
        _ => &[],
    }
}

fn strs(v: &Value, k: &str) -> Vec<String> {
    arr(v, k)
        .iter()
        .map(|x| match x {
            Value::String(s) => s.clone(),
            other => compact(other),
        })
        .collect()
}

/// `v` with its member `k` set to `x`.
fn with(v: &Value, k: &str, x: Value) -> Value {
    let mut v = v.clone();
    tree::set(&mut v, k, x);
    v
}

/// The system prompt of a request to the model, as Chat Completions carries it.
fn system(r: &standin::Req) -> String {
    let v = r.json().unwrap();
    arr(&v, "messages")
        .iter()
        .find(|m| s(m, "role") == "system")
        .map(|m| s(m, "content").to_owned())
        .unwrap_or_default()
}

/// Whether a request to the model offers the tool `name`.
fn has_tool(r: &standin::Req, name: &str) -> bool {
    let v = r.json().unwrap();
    arr(&v, "tools")
        .iter()
        .any(|t| t.get("function").is_some_and(|f| s(f, "name") == name))
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

impl Fx {
    /// The same assistant, as `user`'s browser.
    fn as_user(&self, user: &'static str) -> Fx {
        Fx {
            sh: Arc::clone(&self.sh),
            user,
        }
    }

    fn model_calls(&self) -> Vec<standin::Req> {
        self.model
            .requests()
            .into_iter()
            .filter(|r| r.method == "POST")
            .collect()
    }

    fn no_secrets(&self) {
        for r in self.model.requests() {
            assert!(
                !String::from_utf8_lossy(&r.body).contains(SECRET),
                "a secret reached the model"
            );
        }
    }

    fn send(&self, method: &str, path: &str, body: &str) -> (u16, String) {
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

    fn get(&self, path: &str) -> (u16, String) {
        self.send("GET", &format!("/api/m/ai{path}"), "")
    }

    fn post(&self, path: &str, body: &str) -> (u16, String) {
        self.send("POST", &format!("/api/m/ai{path}"), body)
    }

    fn chat(&self, body: &str) -> Chat {
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
    fn view(&self, conv: &str) -> (u16, Option<Value>, String) {
        let (status, body) = self.get(&format!("/api/proposals/{conv}"));
        let p = parse(&body)
            .ok()
            .and_then(|v| v.get("proposal").cloned())
            .filter(|p| *p != Value::Null);
        (status, p, body)
    }

    /// Confirms the proposal `v` shows.
    fn apply_it(&self, conv: &str, v: &Value) -> (u16, Value) {
        let body = Obj::new()
            .str("id", s(v, "id"))
            .num("revision", n(v, "revision"))
            .str("etag", s(v, "etag"))
            .str("user", s(v, "user"))
            .end();
        let (status, out) = self.post(&format!("/api/proposals/{conv}/apply"), &body);
        (status, parse(&out).unwrap_or(Value::Null))
    }

    fn audit(&self, query: &str) -> Vec<Value> {
        let (status, body) = self.get(&format!("/api/audit{query}"));
        assert_eq!(status, 200, "{body}");
        arr(&parse(&body).unwrap(), "entries").to_vec()
    }

    fn log_text(&self) -> String {
        self.logs.lock().unwrap().join("\n")
    }

    /// Has the user's chat make a proposal of `calls`, and returns its conversation and view.
    fn propose_in(&self, calls: &[(&str, &str)]) -> (String, Value) {
        self.sc.set(vec![tools(calls), text("Proposed.")]);
        let c = self.chat(r#"{"message": "change it"}"#);
        assert!(c.end().is_some(), "{:?}", c.err());
        let conv = c.conv();
        let (_, v, body) = self.view(&conv);
        (conv, v.unwrap_or_else(|| panic!("no proposal: {body}")))
    }
}

// ---- the propose tools called alone (fit's proposals_check_test.go, proposals_edge_test.go) ----

/// Calls the propose tool `tool` in ada's conversation `conv`, made when it is not there.
fn propose_tool(a: &Assistant, conv: &str, tool: &str, args: &str) -> Result<String, String> {
    propose_late(a, conv, tool, args, None)
}

fn propose_late(
    a: &Assistant,
    conv: &str,
    tool: &str,
    args: &str,
    expired: Option<&AtomicBool>,
) -> Result<String, String> {
    {
        let mut chats = a.chats();
        let u = a.user_chats(&mut chats, "ada");
        if u.get(conv).is_none() {
            u.convs.push(Conversation {
                id: conv.to_owned(),
                title: conv.to_owned(),
                ..Conversation::default()
            });
        }
    }
    let settings = a.settings();
    let call = Call {
        a,
        args: parse(args).unwrap_or(Value::Null),
        user: "ada",
        settings: &settings,
        conv,
        expired,
        chat: None,
    };
    (a.registry().get(tool).unwrap().run.unwrap())(&call)
}

fn want(a: &Assistant, conv: &str, tool: &str, args: &str, want: &str) {
    match propose_tool(a, conv, tool, args) {
        Err(e) => assert!(e.contains(want), "{tool} {args}: {e}, want {want:?}"),
        Ok(out) => panic!("{tool} {args}: {out}, want {want:?}"),
    }
}

fn must(a: &Assistant, conv: &str, tool: &str, args: &str) -> Value {
    let out = propose_tool(a, conv, tool, args).unwrap_or_else(|e| panic!("{tool} {args}: {e}"));
    parse(&out).unwrap()
}

fn pending_of(a: &Assistant, conv: &str) -> Option<Proposal> {
    a.proposal_of("ada", conv).flatten()
}

fn built_of(a: &Assistant, conv: &str) -> Built {
    let red = a.redactor(&a.settings());
    build(a, &red, &pending_of(a, conv).unwrap().steps).unwrap()
}

/// fit's `TestProposeChecks`: a step is refused, and left out of the proposal, when it touches
/// admin, writes a secret or a redacted value where the config has none, mixes kinds of config
/// steps, or is not what its tool takes; a redacted value written back where it was is the
/// config's own again.
#[test]
fn propose_checks() {
    let f = start("", vec![text("-")]);
    let (a, cfgs) = (&f.a, &f.cfgs);
    let c = "c1";
    let many_tags = format!(
        r#"{{"summary": "x", "tags": [{}"t"]}}"#,
        r#""t","#.repeat(100)
    );
    let long = format!(r#"{{"summary": "{}"}}"#, "x".repeat(400));
    for (tool, args, w) in [
        (
            CONFIG_PROPOSE,
            r#"{"summary": "x", "members": {"admin": {"address": ":1"}}}"#,
            "admin is out",
        ),
        (
            CONFIG_PROPOSE,
            r#"{"summary": "x", "ops": [{"op": "replace", "path": "/admin/address", "value": ":1"}]}"#,
            "admin is out",
        ),
        (
            CONFIG_PROPOSE,
            r#"{"summary": "x", "ops": [{"op": "replace", "path": "", "value": {}}]}"#,
            "the whole config",
        ),
        (
            CONFIG_PROPOSE,
            r#"{"summary": "x", "ops": [{"op": "move", "path": "/routes/0"}]}"#,
            "not \"move\"",
        ),
        (
            CONFIG_PROPOSE,
            r#"{"summary": "x", "ops": [{"op": "add", "path": "/routes/-"}]}"#,
            "needs a value",
        ),
        (
            CONFIG_PROPOSE,
            r#"{"summary": "x", "ops": [{"op": "add", "path": "routes"}]}"#,
            "starts with /",
        ),
        (
            CONFIG_PROPOSE,
            r#"{"summary": "x", "ops": [{"op": "replace", "path": "/nothing", "value": 1}]}"#,
            "nothing at /nothing",
        ),
        (
            CONFIG_PROPOSE,
            r#"{"summary": "x", "ops": [{"op": "remove", "path": "/routes/7"}]}"#,
            "past the array",
        ),
        (
            CONFIG_PROPOSE,
            r#"{"summary": "x", "ops": [{"op": "add", "path": "/routes/x/y", "value": 1}]}"#,
            "nothing at /routes/x",
        ),
        (
            CONFIG_PROPOSE,
            r#"{"summary": "x", "ops": [{"op": "add", "path": "/listeners/0/name/x", "value": 1}]}"#,
            "neither an object nor an array",
        ),
        (
            CONFIG_PROPOSE,
            r#"{"summary": "x", "members": [1]}"#,
            "not a JSON object",
        ),
        (
            CONFIG_PROPOSE,
            r#"{"summary": "x"}"#,
            "give members, ops or both",
        ),
        (CONFIG_PROPOSE, r#"{"ops": []}"#, "summary is 1 to"),
        (CONFIG_PROPOSE, &long, "summary is 1 to"),
        (CONFIG_PROPOSE, "[1]", "not what config_propose takes"),
        // A secret set, or a redacted value where the config has none.
        (
            CONFIG_PROPOSE,
            r#"{"summary": "x", "ops": [{"op": "replace", "path": "/routes/0/middleware/0/params/users/0/password_hash", "value": "mine"}]}"#,
            "holds a secret",
        ),
        (
            CONFIG_PROPOSE,
            r#"{"summary": "x", "ops": [{"op": "add", "path": "/routes/-", "value": {"middleware": [{"module": "headers", "params": {"token": "t-1"}}], "handler": {"module": "respond"}}}]}"#,
            "holds a secret",
        ),
        (
            CONFIG_PROPOSE,
            r#"{"summary": "x", "ops": [{"op": "add", "path": "/routes/-", "value": {"handler": {"module": "respond", "params": {"body": "[redacted]"}}}}]}"#,
            "a redacted value",
        ),
        (
            PURGE_PROPOSE,
            r#"{"summary": "x"}"#,
            "name urls, tags, or all",
        ),
        (
            PURGE_PROPOSE,
            r#"{"summary": "x", "urls": ["/relative"]}"#,
            "not an absolute",
        ),
        (
            PURGE_PROPOSE,
            r#"{"summary": "x", "zone": "nope", "all": true}"#,
            "no cache uses the zone",
        ),
        (PURGE_PROPOSE, &many_tags, "at most 100"),
        (ROLLBACK_PROPOSE, r#"{"summary": "x"}"#, "kept version"),
        (
            ROLLBACK_PROPOSE,
            r#"{"summary": "x", "version": 1}"#,
            "the one running",
        ),
        (
            ROLLBACK_PROPOSE,
            r#"{"summary": "x", "version": 9}"#,
            "answered 404",
        ),
        (
            RECONCILE_PROPOSE,
            r#"{"summary": "x", "fixes": []}"#,
            "at least one fix",
        ),
        (
            RECONCILE_PROPOSE,
            r#"{"summary": "x", "fixes": [{"id": "nope", "action": "x"}]}"#,
            "no difference \"nope\"",
        ),
    ] {
        want(a, c, tool, args, w);
    }
    assert!(pending_of(a, c).is_none(), "a refused step made a proposal");
    let settings = a.settings();
    let none = Call {
        a,
        args: parse("{}").unwrap(),
        user: "ada",
        settings: &settings,
        conv: "none",
        expired: None,
        chat: None,
    };
    assert_eq!(run_config(&none).unwrap_err(), NO_CONVERSATION);
    let big = format!(r#"{{"summary": "{}"}}"#, "x".repeat(MAX_BYTES));
    assert!(
        propose_tool(a, c, CONFIG_PROPOSE, &big)
            .unwrap_err()
            .contains("larger than")
    );

    // The route copied whole, its secret redacted as the model saw it: the secret is the
    // config's again, and a secret set to "" is allowed.
    let out = must(
        a,
        c,
        CONFIG_PROPOSE,
        r#"{"summary": "Rename the route", "members": {"routes": [{"name": "api2", "match": {"path": ["/api/*"]},
        "middleware": [{"module": "basic_auth", "params": {"users": [{"username": "u", "password_hash": "[redacted]"}]}}], "handler": {"module": "proxy", "params": {"upstream": "app"}}}]}}"#,
    );
    assert!(
        b(&out, "valid") && n(&out, "revision") == 1,
        "{}",
        compact(&out)
    );
    let after = compact(&built_of(a, c).after);
    assert!(
        after.contains(SECRET) && after.contains("\"api2\""),
        "{after}"
    );
    // The kinds of config steps do not mix; a purge goes with any.
    want(
        a,
        c,
        RECONCILE_PROPOSE,
        r#"{"summary": "x", "fixes": [{"id": "a", "action": "b"}]}"#,
        "cannot join",
    );
    want(
        a,
        c,
        ROLLBACK_PROPOSE,
        r#"{"summary": "x", "version": 1}"#,
        "cannot join",
    );
    must(
        a,
        c,
        PURGE_PROPOSE,
        r#"{"summary": "Purge all", "all": true}"#,
    );
    must(
        a,
        c,
        CONFIG_PROPOSE,
        r#"{"summary": "No secret", "ops": [{"op": "replace", "path": "/routes/0/middleware/0/params/users/0/password_hash", "value": ""}]}"#,
    );
    let p = pending_of(a, c).unwrap();
    assert_eq!((p.steps.len(), p.revision), (3, 3));

    // A step that leaves the whole invalid is kept, and the proposal cannot be applied until
    // mended; the config changed meanwhile, the proposal moves to it.
    cfgs.set(&base().replace("10.0.0.7", "10.0.0.9"));
    let out = must(
        a,
        c,
        CONFIG_PROPOSE,
        r#"{"summary": "Break it", "ops": [{"op": "remove", "path": "/routes/0/handler"}]}"#,
    );
    assert!(
        !b(&out, "valid")
            && out.get("errors").is_some()
            && out.get("rebased").is_some()
            && n(&out, "base_version") == 2,
        "{}",
        compact(&out)
    );
    let p = pending_of(a, c).unwrap();
    assert!(!p.valid && p.problems > 0 && p.base_version == 2);
    assert!(state(Some(&p), "").contains("does not check"));

    // Bounds: the steps of a proposal.
    let d = "c2";
    for i in 0..MAX_STEPS {
        must(
            a,
            d,
            PURGE_PROPOSE,
            &format!(r#"{{"summary": "purge {i}", "tags": ["t{i}"]}}"#),
        );
    }
    want(
        a,
        d,
        PURGE_PROPOSE,
        r#"{"summary": "one more", "all": true}"#,
        "has its 32 steps",
    );
    let bt = built_of(a, d);
    assert!(bt.valid && bt.kind.is_empty() && bt.purges.len() == MAX_STEPS);
    {
        let mut chats = a.chats();
        let p = a
            .user_chats(&mut chats, "ada")
            .get(d)
            .unwrap()
            .proposal
            .as_mut()
            .unwrap();
        p.steps.truncate(1);
        p.steps[0].args = " ".repeat(MAX_BYTES);
    }
    want(
        a,
        d,
        PURGE_PROPOSE,
        r#"{"summary": "one more", "all": true}"#,
        "would pass",
    );

    // A step past its timeout is not added.
    let late = AtomicBool::new(true);
    let e = propose_late(
        a,
        c,
        PURGE_PROPOSE,
        r#"{"summary": "late", "all": true}"#,
        Some(&late),
    )
    .unwrap_err();
    assert!(e.contains("took too long"), "{e}");

    // The model's view of a proposal without one, and of its outcome.
    assert_eq!(state(None, "applied."), "applied.");
    let checks = Proposal {
        valid: true,
        ..Proposal::default()
    };
    assert!(state(Some(&checks), "").contains("It checks"));
    let failed = Proposal {
        error: "no".into(),
        ..Proposal::default()
    };
    assert!(state(Some(&failed), "").contains("last apply failed: no"));
}

/// fit's `TestProposeReconcileAndRollback`: reconcile steps' fixes are checked against the
/// Consistency check and applied together as it does; a rollback is one step, refused when it
/// would change admin.
#[test]
fn propose_reconcile_and_rollback() {
    let f = start("", vec![text("-")]);
    let (a, cfgs) = (&f.a, &f.cfgs);
    cfgs.with(|c| {
        c.check = r#"{"differences": [
  {"id": "upstream:app", "fixes": [{"action": "update", "patch": {"op": "replace", "path": "/upstreams/0/addresses", "value": ["10.0.0.9:9000"]}}, {"action": "ignore"}]},
  {"id": "route:x", "fixes": [{"action": "add", "patch": {"op": "add", "path": "/extra/list/-", "value": {"n": 1}}}]},
  {"id": "route:y", "fixes": [{"action": "set", "patch": {"op": "add", "path": "/routes/0/name", "value": "y"}}]},
  {"id": "del:a", "fixes": [{"action": "delete", "patch": {"op": "delete", "path": "/upstreams/0/addresses/0"}}]},
  {"id": "del:b", "fixes": [{"action": "delete", "patch": {"op": "delete", "path": "/upstreams/0"}}]},
  {"id": "admin:x", "fixes": [{"action": "set", "patch": {"op": "replace", "path": "/admin/address", "value": ":1"}}]},
  {"id": "bad", "fixes": [{"action": "set", "patch": {"op": "add", "path": "/routes/0/name"}}, {"action": "nopath", "patch": {"op": "add", "path": "", "value": 1}}, {"action": "missing", "patch": {"op": "replace", "path": "/no/where", "value": 1}}]}
]}"#
        .to_owned();
    });
    let c = "r";
    want(
        a,
        c,
        RECONCILE_PROPOSE,
        r#"{"summary": "x", "fixes": [{"id": "upstream:app", "action": "nope"}]}"#,
        "offers no \"nope\"",
    );
    want(
        a,
        c,
        RECONCILE_PROPOSE,
        r#"{"summary": "x", "fixes": [{"id": "admin:x", "action": "set"}]}"#,
        "admin is out",
    );
    must(
        a,
        c,
        RECONCILE_PROPOSE,
        r#"{"summary": "Fix the pool", "fixes": [{"id": "upstream:app", "action": "update"}, {"id": "route:x", "action": "add"}]}"#,
    );
    want(
        a,
        c,
        RECONCILE_PROPOSE,
        r#"{"summary": "again", "fixes": [{"id": "upstream:app", "action": "update"}]}"#,
        "fixed by the proposal already",
    );
    want(
        a,
        c,
        CONFIG_PROPOSE,
        r#"{"summary": "x", "ops": [{"op": "remove", "path": "/routes/0"}]}"#,
        "cannot join",
    );
    must(
        a,
        c,
        RECONCILE_PROPOSE,
        r#"{"summary": "Name it", "fixes": [{"id": "route:y", "action": "set"}]}"#,
    );
    let bt = built_of(a, c);
    let after = compact(&bt.after);
    assert!(
        after.contains(r#""addresses":["10.0.0.9:9000"]"#)
            && after.contains(r#""extra":{"list":[{"n":1}]}"#)
            && bt.kind == KIND_RECONCILE,
        "{after} {:?}",
        bt.step_errs
    );

    // Deletes go deepest and highest first; a fix that does not apply is its step's error.
    let d = "r2";
    must(
        a,
        d,
        RECONCILE_PROPOSE,
        r#"{"summary": "Deletes", "fixes": [{"id": "del:b", "action": "delete"}, {"id": "del:a", "action": "delete"}]}"#,
    );
    let bt = built_of(a, d);
    assert!(
        bt.valid && compact(&bt.after).contains(r#""upstreams":[]"#),
        "{} {:?}",
        compact(&bt.after),
        bt.step_errs
    );
    for action in ["set", "nopath", "missing"] {
        let args =
            format!(r#"{{"summary": "bad", "fixes": [{{"id": "bad", "action": "{action}"}}]}}"#);
        assert!(
            propose_tool(a, &format!("r3{action}"), RECONCILE_PROPOSE, &args).is_err(),
            "{action}: a fix that does not apply"
        );
    }
    cfgs.with(|c| "not json".clone_into(&mut c.check));
    want(
        a,
        "r4",
        RECONCILE_PROPOSE,
        r#"{"summary": "x", "fixes": [{"id": "a", "action": "b"}]}"#,
        "could not be read",
    );

    // Rollback.
    cfgs.set(&base().replace("10.0.0.7", "10.0.0.9"));
    cfgs.set(&base().replace(r#""max_conns": 8"#, r#""max_conns": 9"#));
    cfgs.set("[1]");
    let r = "rb";
    // The running one does not read.
    want(
        a,
        r,
        ROLLBACK_PROPOSE,
        r#"{"summary": "x", "version": 1}"#,
        "not a JSON object",
    );
    cfgs.set(&base());
    want(
        a,
        r,
        ROLLBACK_PROPOSE,
        r#"{"summary": "x", "version": 3}"#,
        "another admin",
    );
    want(
        a,
        r,
        ROLLBACK_PROPOSE,
        r#"{"summary": "x", "version": 4}"#,
        "not a JSON object",
    );
    must(
        a,
        r,
        ROLLBACK_PROPOSE,
        r#"{"summary": "Back to 2", "version": 2}"#,
    );
    want(
        a,
        r,
        ROLLBACK_PROPOSE,
        r#"{"summary": "x", "version": 1}"#,
        "one rollback",
    );
    want(
        a,
        r,
        CONFIG_PROPOSE,
        r#"{"summary": "x", "ops": [{"op": "remove", "path": "/routes/0"}]}"#,
        "cannot join",
    );
    let bt = built_of(a, r);
    assert_eq!(
        (bt.rollback, bt.changed.clone()),
        (2, vec!["upstreams".to_owned()])
    );
}

/// fit's `TestProposeBuildFails`: a config the Admin API does not give is the tool's error.
#[test]
fn propose_build_fails() {
    let none = start_with("", vec![text("-")], |o| o.api = None);
    want(
        &none.a,
        "x",
        PURGE_PROPOSE,
        r#"{"summary": "x", "all": true}"#,
        "not reachable",
    );
    let arr = start_with("", vec![text("-")], |o| {
        o.api = Some(Arc::new(Fake::new("[1]")));
    });
    want(
        &arr.a,
        "x",
        PURGE_PROPOSE,
        r#"{"summary": "x", "all": true}"#,
        "not a JSON object",
    );
}

/// fit's `TestProposeEdges`: a member removed, a step that fails once the config moved, a check
/// whose errors are many, a step of no propose tool, a render that fails, and the cache's zones
/// not read.
#[test]
fn propose_edges() {
    let f = start("", vec![text("-")]);
    let (a, cfgs) = (&f.a, &f.cfgs);
    // A member removed; a secret in a member set whole is refused.
    let out = must(
        a,
        "m",
        CONFIG_PROPOSE,
        r#"{"summary": "No upstreams", "members": {"upstreams": null, "x_none": null}}"#,
    );
    assert_eq!(strs(&out, "changed"), ["upstreams"]);
    want(
        a,
        "m",
        CONFIG_PROPOSE,
        r#"{"summary": "x", "members": {"routes": [{"middleware": [{"module": "m", "params": {"secret": "new"}}], "handler": {"module": "respond"}}]}}"#,
        "holds a secret",
    );
    // A render that fails: the view says so in the diff's place.
    cfgs.with(|c| c.broken = vec!["POST /config/render".into()]);
    let (v, _, _) = a
        .view_proposal("ada", "m", &pending_of(a, "m").unwrap(), "ada")
        .unwrap();
    let v = parse(&v).unwrap();
    let diff = v.get("diff").unwrap();
    assert!(
        s(diff, "conf").contains("could not be rendered in conf"),
        "{}",
        s(diff, "conf")
    );
    assert!(s(diff, "json").contains("+++ proposed"));
    cfgs.with(|c| c.broken.clear());
    must(
        a,
        "r",
        CONFIG_PROPOSE,
        r#"{"summary": "x", "ops": [{"op": "add", "path": "/upstreams/0/addresses/-", "value": "render-fails"}]}"#,
    );
    let (v, _, _) = a
        .view_proposal("ada", "r", &pending_of(a, "r").unwrap(), "ada")
        .unwrap();
    assert!(
        s(parse(&v).unwrap().get("diff").unwrap(), "yaml")
            .contains("could not be rendered in yaml")
    );

    // A step that applied once fails when the config moves: the next step hears of it.
    must(
        a,
        "s",
        CONFIG_PROPOSE,
        r#"{"summary": "Drop the middleware", "ops": [{"op": "remove", "path": "/routes/0/middleware"}]}"#,
    );
    cfgs.set(r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}], "routes": [{"handler": {"module": "respond"}}]}"#);
    let out = must(
        a,
        "s",
        PURGE_PROPOSE,
        r#"{"summary": "Purge", "all": true}"#,
    );
    assert!(
        out.get("step_errors").and_then(|e| e.get("1")).is_some() && !b(&out, "valid"),
        "{}",
        compact(&out)
    );

    // The cache's zones not read.
    cfgs.with(|c| c.broken = vec!["GET /cache".into()]);
    want(
        a,
        "z",
        PURGE_PROPOSE,
        r#"{"summary": "x", "zone": "main", "all": true}"#,
        "zones could not be read",
    );
    cfgs.with(|c| c.broken.clear());

    // A step of no propose tool, as from a store an older akit wrote.
    let red = a.redactor(&a.settings());
    let st = Step {
        tool: "nope".into(),
        summary: "x".into(),
        args: r#"{"summary": "x"}"#.into(),
        at: 0,
    };
    let bt = build(a, &red, &[st]).unwrap();
    assert!(bt.step_errs[0] == "nope is not a propose tool" && !bt.valid);

    // A check with many errors lists 50, and says how many more.
    let many = start_with("", vec![text("-")], |o| {
        let api = Fake::new(&base());
        api.with(|c| {
            c.validate = Some(|_| {
                let items: Vec<String> = (0..60)
                    .map(|i| format!(r#"{{"path":"x{i}","msg":"bad"}}"#))
                    .collect();
                format!(r#"{{"valid":false,"errors":[{}]}}"#, items.join(","))
            });
        });
        o.api = Some(Arc::new(api));
    });
    let out = must(
        &many.a,
        "v",
        CONFIG_PROPOSE,
        r#"{"summary": "x", "ops": [{"op": "add", "path": "/upstreams/-", "value": {"name": "b"}}]}"#,
    );
    let errs = arr(&out, "errors");
    assert!(
        errs.len() == 51 && s(&errs[50], "msg").contains("and 10 more"),
        "{}",
        compact(&out)
    );

    // An apply or discard that finished after a step was added leaves the proposal the step made.
    let p = pending_of(a, "m").unwrap();
    a.set_proposal("ada", "m", &p.id, p.revision - 1, None, "applied");
    a.set_proposal("ada", "m", "other", p.revision, None, "applied");
    assert_eq!(pending_of(a, "m").unwrap().revision, p.revision);

    // What the log keeps of a proposal is bounded.
    let mut big = Proposal {
        id: "p".into(),
        ..Proposal::default()
    };
    for _ in 0..MAX_STEPS + 5 {
        big.steps.push(Step {
            tool: PURGE_PROPOSE.into(),
            summary: "s".repeat(500),
            ..Step::default()
        });
    }
    let ap = audit::Prop::of(&big, &vec![String::new(); 70]);
    assert!(ap.steps.len() == MAX_STEPS && ap.changed.len() == audit::MAX_NAMES);
    assert!(ap.steps[0].1.len() <= MAX_SUMMARY_BYTES + 4);
}

/// fit's `TestReconcileOrder`: the removals of several fixes go deepest and highest first, by
/// index or by name, and a fix that sets an element sets it in place.
#[test]
fn reconcile_order() {
    let f = start("", vec![text("-")]);
    let (a, cfgs) = (&f.a, &f.cfgs);
    cfgs.set(r#"{"listeners": [{"name": "a", "address": ":1"}, {"name": "b", "address": ":2"}, {"name": "c", "address": ":3"}],
  "routes": [{"handler": {"module": "respond"}}], "upstreams": [{"name": "u", "addresses": ["x", "y"]}]}"#);
    cfgs.with(|c| {
        c.check = r#"{"differences": [
  {"id": "l0", "fixes": [{"action": "delete", "patch": {"op": "delete", "path": "/listeners/0"}}]},
  {"id": "l2", "fixes": [{"action": "delete", "patch": {"op": "delete", "path": "/listeners/2"}}]},
  {"id": "u", "fixes": [{"action": "delete", "patch": {"op": "delete", "path": "/upstreams"}}]},
  {"id": "set", "fixes": [{"action": "set", "patch": {"op": "replace", "path": "/listeners/1/address", "value": ":9"}}]},
  {"id": "el", "fixes": [{"action": "set", "patch": {"op": "add", "path": "/upstreams/0/addresses/1", "value": "z"}}]}
]}"#
        .to_owned();
    });
    must(
        a,
        "o",
        RECONCILE_PROPOSE,
        r#"{"summary": "Fixes", "fixes": [{"id": "l0", "action": "delete"}, {"id": "u", "action": "delete"},
  {"id": "l2", "action": "delete"}, {"id": "set", "action": "set"}]}"#,
    );
    let bt = built_of(a, "o");
    assert_eq!(
        compact(&bt.after),
        r#"{"listeners":[{"name":"b","address":":9"}],"routes":[{"handler":{"module":"respond"}}]}"#,
        "{:?}",
        bt.step_errs
    );
    must(
        a,
        "e",
        RECONCILE_PROPOSE,
        r#"{"summary": "In place", "fixes": [{"id": "el", "action": "set"}]}"#,
    );
    assert!(compact(&built_of(a, "e").after).contains(r#""addresses":["x","z"]"#));
}

// ---- the panel (fit's proposals_test.go, apply_test.go, proposals_more_test.go) ----

/// fit's `TestProposeAndApply`: a change asked in words becomes one proposal of three steps,
/// checked and shown with its diff in three formats; nothing is applied until the confirm, and
/// then the steps go as one change with If-Match, a new version, its purge after it; the audit
/// log holds the chat and the apply, and the model hears of it.
#[test]
fn propose_and_apply() {
    let f = start(
        r#", "default_level": "apply""#,
        vec![
            tools(&[
                (
                    CONFIG_PROPOSE,
                    r#"{"summary": "Add a route for /health", "ops": [{"op": "add", "path": "/routes/0", "value": {"name": "health", "match": {"path": ["/health"]}, "handler": {"module": "respond", "params": {"body": "ok"}}}}]}"#,
                ),
                (
                    CONFIG_PROPOSE,
                    r#"{"summary": "A second backend", "members": {"upstreams": [{"name": "app", "addresses": ["10.0.0.7:9000", "10.0.0.8:9000"]}]}}"#,
                ),
                (
                    PURGE_PROPOSE,
                    r#"{"summary": "Drop the cached health page", "zone": "main", "urls": ["https://example.com/health"]}"#,
                ),
            ]),
            text("The proposal adds /health and a backend."),
            text("Done."),
        ],
    );
    let c = f.chat(r#"{"message": "add a /health route and a second backend 10.0.0.8:9000"}"#);
    assert_eq!(
        c.end().map(|e| s(e, "reason")),
        Some("answer"),
        "{:?}",
        c.err()
    );
    let conv = c.conv();
    let props = c.of("proposal");
    assert!(
        props.len() == 3
            && s(props[0], "id") == s(props[2], "id")
            && n(props[2], "revision") == 3
            && s(props[1], "name") == CONFIG_PROPOSE
            && s(props[0], "text") == "Add a route for /health",
        "{props:?} {:?}",
        c.of("tool_result")
    );
    let res = c.of("tool_result");
    assert!(
        !b(res[0], "error") && s(res[0], "content").contains(r#""valid":true"#),
        "{res:?}"
    );
    assert!(
        s(res[1], "content").contains(r#"+        \"10.0.0.8:9000\""#),
        "{}",
        s(res[1], "content")
    );
    assert!(f.cfgs.changed().is_empty(), "a proposal applied something");

    let (status, v, body) = f.view(&conv);
    let v = v.unwrap();
    assert!(
        status == 200
            && arr(&v, "steps").len() == 3
            && b(&v, "valid")
            && b(&v, "can_apply")
            && !b(&v, "stale")
            && s(&v, "kind") == KIND_PATCH
            && strs(&v, "changed") == ["upstreams", "routes"]
            && arr(&v, "purges").len() == 1
            && n(&v, "version") == 1,
        "{status} {body}"
    );
    for fmt in DIFF_FORMATS {
        let d = s(v.get("diff").unwrap(), fmt);
        assert!(
            d.contains("--- version 1\n+++ proposed\n") && d.contains("10.0.0.8:9000"),
            "{fmt} diff: {d}"
        );
    }
    assert!(!body.contains(SECRET), "the view: {body}");
    // Reading it changed nothing: no change, and the store's sections as they were.
    f.a.settle();
    let section = crate::conv::section_of("ada");
    let before = f.store.section(&section);
    f.view(&conv);
    f.a.settle();
    assert!(f.cfgs.changed().is_empty() && before == f.store.section(&section));

    // A confirm of another revision is refused.
    let (status, out) = f.apply_it(&conv, &with(&v, "revision", Value::Number("1".into())));
    assert!(
        status == 409 && s(&out, "outcome") == audit::REFUSED,
        "{status} {out:?}"
    );
    let (status, out) = f.apply_it(&conv, &v);
    let purged = arr(&out, "purged");
    assert!(
        status == 200
            && s(&out, "outcome") == audit::APPLIED
            && n(&out, "version") == 2
            && purged.len() == 1
            && n(&purged[0], "purged") == 2
            && strs(&out, "warnings") == ["a warning"],
        "{status} {}",
        compact(&out)
    );
    assert_eq!(
        f.cfgs.changed(),
        [format!(
            "PUT /config/text?format=json&members=upstreams%2Croutes {}",
            etag_of(&base())
        )]
    );
    let (_, data) = f.cfgs.current();
    assert!(
        data.contains(SECRET) && data.contains("\"health\"") && data.contains("10.0.0.8"),
        "applied: {data}"
    );
    assert!(f.view(&conv).1.is_none(), "the proposal is still pending");
    assert!(
        f.log_text().contains("\"ada\": ada applied the proposal"),
        "{}",
        f.log_text()
    );

    let e = f.audit("");
    assert!(
        e.len() == 3
            && s(&e[0], "kind") == audit::KIND_APPLY
            && n(&e[0], "version") == 2
            && s(&e[0], "outcome") == audit::APPLIED
            && arr(e[0].get("proposal").unwrap(), "steps").len() == 3
            && s(&e[2], "kind") == audit::KIND_CHAT
            && arr(&e[2], "calls").len() == 3
            && e[2].get("proposal").is_some()
            && s(&e[2], "outcome") == "answer"
            && s(&e[1], "outcome") == audit::REFUSED,
        "{e:?}"
    );

    // The model hears what became of it.
    f.chat(&format!(
        r#"{{"conversation": "{conv}", "message": "thanks"}}"#
    ));
    let reqs = f.model_calls();
    let last = system(reqs.last().unwrap());
    assert!(
        last.contains("as config version 2") && last.contains("Proposals: to change"),
        "{last}"
    );
    let second = system(&reqs[1]);
    assert!(
        second.contains("Pending proposal")
            && second.contains("3. cache_purge_propose: Drop the cached health page"),
        "{second}"
    );
    f.no_secrets();
}

/// fit's `TestApplyRefusals`: a proposal that does not check, one confirmed on an older config or
/// revision, or one whose conversation is answering, is not applied.
#[test]
fn apply_refusals() {
    let f = start(r#", "default_level": "apply""#, vec![text("-")]);
    // Invalid: a route without a handler.
    let (conv, v) = f.propose_in(&[(
        CONFIG_PROPOSE,
        r#"{"summary": "Break", "ops": [{"op": "remove", "path": "/routes/0/handler"}]}"#,
    )]);
    assert!(!b(&v, "valid") && !arr(&v, "errors").is_empty(), "{v:?}");
    let (status, out) = f.apply_it(&conv, &v);
    assert!(
        status == 422 && s(&out, "outcome") == audit::REFUSED && f.cfgs.changed().is_empty(),
        "{status} {out:?}"
    );

    // Stale: the config changed after the proposal was made.
    let (conv, v) = f.propose_in(&[(
        CONFIG_PROPOSE,
        r#"{"summary": "Second backend", "ops": [{"op": "add", "path": "/upstreams/0/addresses/-", "value": "10.0.0.8:9000"}]}"#,
    )]);
    f.cfgs
        .set(&base().replace(r#""max_conns": 8"#, r#""max_conns": 9"#));
    let now = f.view(&conv).1.unwrap();
    assert!(
        b(&now, "stale") && n(&now, "version") == 2 && n(&now, "base_version") == 1,
        "{now:?}"
    );
    let (status, out) = f.apply_it(&conv, &v);
    let p = out.get("proposal").cloned().unwrap_or(Value::Null);
    assert!(
        status == 412
            && s(&out, "outcome") == audit::STALE
            && n(&p, "version") == 2
            && n(&p, "revision") == n(&v, "revision") + 1
            && !b(&p, "stale")
            && s(&out, "error").contains("offered again on version 2"),
        "{status} {}",
        compact(&out)
    );
    assert!(f.cfgs.changed().is_empty(), "a stale proposal was applied");
    // The page that confirmed the old one is refused again; the one offered again applies.
    assert_eq!(f.apply_it(&conv, &v).0, 409);
    let (status, out) = f.apply_it(&conv, &p);
    assert!(status == 200 && n(&out, "version") == 3, "{status} {out:?}");
    let (_, data) = f.cfgs.current();
    assert!(
        data.contains(r#""max_conns":9"#) && data.contains("10.0.0.8"),
        "{data}"
    );

    // An Admin API that finds it stale at the last moment.
    let (conv, v) = f.propose_in(&[(
        CONFIG_PROPOSE,
        r#"{"summary": "Third", "ops": [{"op": "add", "path": "/upstreams/0/addresses/-", "value": "10.0.0.9:9000"}]}"#,
    )]);
    f.cfgs.with(|c| c.fail = 412);
    let (status, out) = f.apply_it(&conv, &v);
    assert!(
        status == 412 && s(&out, "outcome") == audit::STALE,
        "{status} {out:?}"
    );
    f.cfgs.with(|c| c.fail = 0);

    // Answering: the conversation may change its proposal.
    let v = f.view(&conv).1.unwrap();
    let busy = |on: bool| {
        let mut chats = f.a.chats();
        f.a.user_chats(&mut chats, "ada").get(&conv).unwrap().busy = on;
    };
    busy(true);
    let (status, body) = f.post(
        &format!("/api/proposals/{conv}/apply"),
        &format!(r#"{{"id": "{}", "revision": 1}}"#, s(&v, "id")),
    );
    assert!(
        status == 409 && body.contains("answering"),
        "{status} {body}"
    );
    busy(false);

    // Requests that name nothing.
    for (method, path, body, want) in [
        ("GET", "/api/m/ai/api/proposals/nope".to_owned(), "", 404),
        (
            "POST",
            "/api/m/ai/api/proposals/nope/apply".to_owned(),
            r#"{"id": "x"}"#,
            404,
        ),
        (
            "POST",
            format!("/api/m/ai/api/proposals/{conv}/apply"),
            r#"{"id": "other"}"#,
            409,
        ),
        (
            "POST",
            format!("/api/m/ai/api/proposals/{conv}/discard"),
            r#"{"id": "other"}"#,
            409,
        ),
        (
            "POST",
            format!("/api/m/ai/api/proposals/{conv}/apply"),
            "nope",
            400,
        ),
        ("PUT", format!("/api/m/ai/api/proposals/{conv}"), "{}", 405),
        (
            "POST",
            format!("/api/m/ai/api/proposals/{conv}/other"),
            "{}",
            405,
        ),
    ] {
        let (status, out) = f.send(method, &path, body);
        assert_eq!(status, want, "{method} {path}: {out}");
    }
    // A conversation without one.
    f.sc.set(vec![text("hello")]);
    let c = f.chat(r#"{"message": "hello"}"#);
    let (status, body) = f.get(&format!("/api/proposals/{}", c.conv()));
    assert!(
        status == 200 && body == r#"{"proposal":null}"#,
        "{status} {body}"
    );
}

/// fit's `TestApplyFailsWhole`: the config steps go as one change, so a change the Admin API
/// refuses applies none of them, nor the purges after them; the proposal stays, with why, and the
/// model hears of it.
#[test]
fn apply_fails_whole() {
    let f = start(r#", "default_level": "apply""#, vec![text("-")]);
    let (conv, v) = f.propose_in(&[
        (CONFIG_PROPOSE, r#"{"summary": "Move the listener", "ops": [{"op": "replace", "path": "/listeners/0/address", "value": "127.0.0.1:1"}]}"#),
        (CONFIG_PROPOSE, r#"{"summary": "A backend", "ops": [{"op": "add", "path": "/upstreams/0/addresses/-", "value": "10.0.0.8:9000"}]}"#),
        (PURGE_PROPOSE, r#"{"summary": "Purge", "all": true}"#),
    ]);
    assert!(b(&v, "valid") && arr(&v, "steps").len() == 3, "{v:?}");
    f.cfgs.with(|c| c.fail = 400);
    let (status, out) = f.apply_it(&conv, &v);
    let errs = arr(&out, "errors");
    assert!(
        status == 422
            && s(&out, "outcome") == audit::FAILED
            && errs.len() == 1
            && s(&errs[0], "message") == "address in use"
            && !s(out.get("proposal").unwrap(), "error").is_empty(),
        "{status} {}",
        compact(&out)
    );
    let (version, _) = f.cfgs.current();
    let purges = f.cfgs.with(|c| c.purges.len());
    assert!(version == 1 && purges == 0 && f.cfgs.changed().len() == 1);
    let e = f.audit("?kind=apply");
    assert!(
        e.len() == 1 && s(&e[0], "outcome") == audit::FAILED && !s(&e[0], "detail").is_empty(),
        "{e:?}"
    );
    f.sc.set(vec![text("ok")]);
    f.chat(&format!(
        r#"{{"conversation": "{conv}", "message": "why?"}}"#
    ));
    let reqs = f.model_calls();
    assert!(system(reqs.last().unwrap()).contains("Its last apply failed"));
}

/// fit's `TestApplyKinds`: reconcile steps go as one `POST /config/reconcile`, a rollback as `POST
/// /rollback/{n}`, both with If-Match; purges alone change no config and are never stale, and a
/// purge that fails after the config leaves the outcome partial.
#[test]
fn apply_kinds() {
    let f = start(r#", "default_level": "apply""#, vec![text("-")]);
    f.cfgs.with(|c| {
        c.check = r#"{"differences": [{"id": "admin:timeout", "fixes": [{"action": "reapply"}]},
  {"id": "pool", "fixes": [{"action": "update", "patch": {"op": "replace", "path": "/upstreams/0/addresses", "value": ["10.0.0.9:9000"]}}]}]}"#
            .to_owned();
    });
    let (conv, v) = f.propose_in(&[
        (
            RECONCILE_PROPOSE,
            r#"{"summary": "Reapply", "fixes": [{"id": "admin:timeout", "action": "reapply"}]}"#,
        ),
        (
            RECONCILE_PROPOSE,
            r#"{"summary": "Fix the pool", "fixes": [{"id": "pool", "action": "update"}]}"#,
        ),
        (
            PURGE_PROPOSE,
            r#"{"summary": "Purge main", "zone": "main", "all": true}"#,
        ),
    ]);
    assert!(
        s(&v, "kind") == KIND_RECONCILE && arr(&v, "fixes").len() == 2 && b(&v, "valid"),
        "{v:?}"
    );
    // The zone is gone by the time it is applied.
    f.cfgs.with(|c| c.zones.clear());
    let v = f.view(&conv).1.unwrap();
    assert!(
        !b(&v, "valid") && !s(&arr(&v, "steps")[2], "error").is_empty(),
        "{v:?}"
    );
    f.cfgs.with(|c| c.zones = vec!["main".into()]);
    let v = f.view(&conv).1.unwrap();
    let (status, out) = f.apply_it(&conv, &v);
    assert!(status == 200 && n(&out, "version") == 2, "{status} {out:?}");
    let body = f.cfgs.with(|c| c.bodies[0].clone());
    let got = f.cfgs.changed();
    assert!(
        got.len() == 1
            && got[0].starts_with("POST /config/reconcile \"")
            && body
                == r#"{"fixes":[{"id":"admin:timeout","action":"reapply"},{"id":"pool","action":"update"}]}"#,
        "{got:?} {body}"
    );

    // A rollback.
    let (conv, v) = f.propose_in(&[(
        ROLLBACK_PROPOSE,
        r#"{"summary": "Back to 1", "version": 1}"#,
    )]);
    assert!(
        n(&v, "rollback") == 1 && s(&v, "kind") == KIND_ROLLBACK,
        "{v:?}"
    );
    let (status, out) = f.apply_it(&conv, &v);
    assert!(status == 200 && n(&out, "version") == 3, "{status} {out:?}");
    let got = f.cfgs.changed();
    assert!(
        got.len() == 2 && got[1].starts_with("POST /rollback/1 "),
        "{got:?}"
    );

    // Purges alone: no config change, no ETag; one fails.
    let (conv, v) = f.propose_in(&[
        (
            PURGE_PROPOSE,
            r#"{"summary": "Purge main", "zone": "main", "tags": ["x"]}"#,
        ),
        (
            PURGE_PROPOSE,
            r#"{"summary": "Purge all zones", "all": true}"#,
        ),
    ]);
    f.cfgs.with(|c| c.fail_purge = true);
    f.cfgs.set(&base());
    let v = with(
        &with(&v, "etag", Value::String("old".into())),
        "revision",
        Value::Number("2".into()),
    );
    let (status, out) = f.apply_it(&conv, &v);
    let purged = arr(&out, "purged");
    assert!(
        status == 200
            && s(&out, "outcome") == audit::PARTIAL
            && n(&out, "version") == 0
            && purged.len() == 2
            && !s(&purged[0], "error").is_empty()
            && n(&purged[1], "purged") == 2,
        "{status} {}",
        compact(&out)
    );
    assert_eq!(f.cfgs.changed().len(), 2);
    let e = f.audit("?kind=apply&limit=1");
    assert!(
        e.len() == 1 && s(&e[0], "outcome") == audit::PARTIAL && !s(&e[0], "detail").is_empty(),
        "{e:?}"
    );
}

/// fit's `TestDiscard`: a discarded proposal, or one deleted with its conversation, is gone and
/// in the audit log; nothing else changes.
#[test]
fn discard() {
    let f = start(
        r#", "users": [{"username": "bob", "level": "off"}], "default_level": "propose""#,
        vec![text("-")],
    );
    let (conv, v) = f.propose_in(&[(PURGE_PROPOSE, r#"{"summary": "Purge", "all": true}"#)]);
    let id = s(&v, "id");
    let body = format!(r#"{{"id": "{id}"}}"#);
    let (status, out) = f
        .as_user("bob")
        .post(&format!("/api/proposals/{conv}/discard"), &body);
    assert_eq!(status, 403, "{out}");
    let (status, out) = f.post(&format!("/api/proposals/{conv}/discard"), &body);
    assert!(status == 200 && out.contains(id), "{status} {out}");
    assert!(f.view(&conv).1.is_none(), "still pending");
    let (conv2, _) = f.propose_in(&[(PURGE_PROPOSE, r#"{"summary": "Purge", "all": true}"#)]);
    let (status, out) = f.send(
        "DELETE",
        &format!("/api/m/ai/api/conversations/{conv2}"),
        "",
    );
    assert_eq!(status, 200, "{out}");
    let e = f.audit("?kind=discard");
    assert!(
        e.len() == 2
            && s(&e[0], "detail") == "deleted with its conversation"
            && s(&e[1], "outcome") == audit::DISCARDED
            && f.cfgs.changed().is_empty(),
        "{e:?}"
    );
    // The conversation says what became of it.
    let (status, out) = f.get(&format!("/api/conversations/{conv}"));
    assert!(
        status == 200
            && s(&parse(&out).unwrap(), "proposal_note").contains("ada discarded the proposal"),
        "{status} {out}"
    );
}

/// fit's `TestProposalViewFails`: an Admin API that does not give the config fails the view and
/// the apply with 502, applying nothing.
#[test]
fn proposal_view_fails() {
    let f = start(r#", "default_level": "apply""#, vec![text("-")]);
    let (conv, v) = f.propose_in(&[(PURGE_PROPOSE, r#"{"summary": "Purge", "all": true}"#)]);
    f.cfgs.with(|c| c.down = true);
    assert_eq!(f.view(&conv).0, 502);
    assert_eq!(f.apply_it(&conv, &v).0, 502);
}

/// fit's `TestProposeLevels`: a read or edit user gets no propose tools; a propose user proposes
/// but cannot apply, while a user who applies sees and applies others' proposals; a level lowered
/// during a chat stops its next call.
#[test]
fn propose_levels() {
    let add_route = r#"{"summary": "Add /health", "ops": [{"op": "add", "path": "/routes/-", "value": {"handler": {"module": "respond", "params": {"body": "ok"}}}}]}"#;
    let f = start(
        r#", "users": [{"username": "ada", "level": "apply"}, {"username": "bob", "level": "propose"}]"#,
        vec![tools(&[(CONFIG_PROPOSE, add_route)]), text("Proposed.")],
    );
    let bob = f.as_user("bob");
    let c = bob.chat(r#"{"message": "add /health"}"#);
    assert!(
        c.end().is_some() && c.of("proposal").len() == 1,
        "{:?}",
        c.err()
    );
    assert!(
        has_tool(&f.model_calls()[0], CONFIG_PROPOSE),
        "a propose user is not offered config_propose"
    );
    let conv = c.conv();
    let (status, v, body) = bob.view(&conv);
    let v = v.unwrap();
    assert!(
        status == 200 && !b(&v, "can_apply") && s(&v, "level") == "propose",
        "{status} {body}"
    );
    let (status, out) = bob.apply_it(&conv, &v);
    assert!(
        status == 403 && s(&out, "error").contains("only a user whose level is apply"),
        "{status} {out:?}"
    );
    assert!(f.cfgs.changed().is_empty(), "a propose user applied");
    // bob reads only his own; ada reads every user's.
    assert_eq!(bob.get("/api/proposals/x?user=ada").0, 403);
    assert_eq!(
        bob.post("/api/proposals/x/discard", r#"{"user": "ada", "id": "x"}"#)
            .0,
        403
    );
    let (status, out) = bob.get("/api/proposals");
    assert!(
        status == 200 && out.contains(r#""user":"bob""#) && out.contains("Add /health"),
        "{status} {out}"
    );
    let (status, list) = f.get("/api/proposals");
    assert!(
        status == 200 && list.contains(r#""user":"bob""#) && list.contains(r#""level":"apply""#),
        "{status} {list}"
    );
    let (status, v, body) = f.view(&format!("{conv}?user=bob"));
    let v = v.unwrap();
    assert!(
        status == 200 && b(&v, "can_apply") && s(&v, "user") == "bob",
        "{status} {body}"
    );
    let (status, out) = f.apply_it(&conv, &v);
    assert!(status == 200 && n(&out, "version") == 2, "{status} {out:?}");
    let e = f.audit("?kind=apply");
    assert!(
        e.len() == 2
            && s(&e[0], "user") == "ada"
            && s(&e[0], "owner") == "bob"
            && s(&e[1], "user") == "bob"
            && s(&e[1], "outcome") == audit::REFUSED,
        "{e:?}"
    );
    let e = f.audit("?user=bob&kind=chat");
    assert!(
        e.len() == 1 && s(&arr(&e[0], "calls")[0], "tool") == CONFIG_PROPOSE,
        "{e:?}"
    );

    // read and edit users are offered no propose tools.
    for level in ["read", "edit"] {
        let g = start(
            &format!(r#", "default_level": "{level}""#),
            vec![tools(&[(CONFIG_PROPOSE, add_route)]), text("no")],
        );
        let c = g.chat(r#"{"message": "add /health"}"#);
        let first = &g.model_calls()[0];
        assert!(
            c.end().is_some()
                && !has_tool(first, CONFIG_PROPOSE)
                && b(c.of("tool_result")[0], "error")
                && c.of("proposal").is_empty(),
            "{level}: {:?}",
            c.events
        );
        assert!(!system(first).contains("Proposals: to change"), "{level}");
    }

    // A level lowered while a chat runs, here as the model is called, after the chat took its
    // tools, stops the call that comes.
    let handle: Arc<OnceLock<Assistant>> = Arc::default();
    let h2 = Arc::clone(&handle);
    let calls = Arc::new(Mutex::new(0));
    let route = add_route.to_owned();
    let model = standin::Server::start(move |r, c| {
        if r.method != "POST" {
            return c.whole(200, "application/json", br#"{"data":[]}"#);
        }
        let k = {
            let mut n = calls.lock().unwrap();
            *n += 1;
            *n
        };
        let turn = if k == 1 {
            let a = h2.get().unwrap();
            let mut st = a.state();
            let mut lower = (*st.settings).clone();
            lower.default_level = Some(Level::Read);
            st.settings = Arc::new(lower);
            format!(
                r#"{{"choices":[{{"delta":{{"tool_calls":[{{"index":0,"id":"p","function":{{"name":"{CONFIG_PROPOSE}","arguments":{}}}}}]}}}}]}}"#,
                quoted(&route)
            )
        } else {
            r#"{"choices":[{"delta":{"content":"no"}}]}"#.to_owned()
        };
        c.head(200, &[("Content-Type", "text/event-stream")]);
        c.chunk(
            format!(
                "data: {turn}\n\ndata: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n"
            )
            .as_bytes(),
        );
    });
    let cfgs = Fake::new(&base());
    let logs = Logs::default();
    let store = Arc::new(Mem::default());
    let o = options(
        &model.url(),
        r#", "default_level": "propose""#,
        cfgs.clone(),
        Some(Arc::clone(&store)),
        &logs,
    );
    let a = Assistant::new(o);
    let _ = handle.set(a.clone());
    let h = serve(a, cfgs, Script::default(), model, store, logs);
    let c = h.chat(r#"{"message": "add /health"}"#);
    assert!(
        c.end().is_some()
            && s(c.of("tool_result")[0], "content").contains("level is read now")
            && c.of("proposal").is_empty(),
        "{:?}",
        c.events
    );
}

/// fit's `TestChatAuditOnError`: an answer that called tools and then failed is in the audit log
/// with its error.
#[test]
fn chat_audit_on_error() {
    let f = start("", vec![tools(&[("status_get", "{}")]), Turn::Status(400)]);
    let c = f.chat(r#"{"message": "status?"}"#);
    assert!(c.err().is_some(), "{:?}", c.end());
    let e = f.audit("");
    assert!(
        e.len() == 1
            && s(&e[0], "outcome") == crate::chat::REASON_ERROR
            && !s(&e[0], "detail").is_empty()
            && s(&arr(&e[0], "calls")[0], "tool") == "status_get",
        "{e:?}"
    );
}

/// fit's `TestProposalsList`: a user who applies sees every user's pending proposals, the latest
/// first, and discards another's.
#[test]
fn proposals_list() {
    let f = start(
        r#", "users": [{"username": "ada", "level": "apply"}, {"username": "bob", "level": "propose"}]"#,
        vec![text("-")],
    );
    let (conv1, _) = f.propose_in(&[(PURGE_PROPOSE, r#"{"summary": "First", "all": true}"#)]);
    let bob = f.as_user("bob");
    let (conv2, v2) = bob.propose_in(&[(PURGE_PROPOSE, r#"{"summary": "Second", "all": true}"#)]);
    let (status, body) = f.get("/api/proposals");
    let list = parse(&body).unwrap();
    let ps = arr(&list, "proposals");
    assert!(
        status == 200
            && ps.len() == 2
            && s(&ps[0], "conversation") == conv2
            && s(&ps[1], "conversation") == conv1,
        "{status} {body}"
    );
    let (status, out) = f.post(
        &format!("/api/proposals/{conv2}/discard"),
        &format!(r#"{{"user": "bob", "id": "{}"}}"#, s(&v2, "id")),
    );
    assert_eq!(status, 200, "{out}");
    let e = f.audit("?kind=discard");
    assert!(
        e.len() == 1 && s(&e[0], "owner") == "bob" && s(&e[0], "user") == "ada",
        "{e:?}"
    );
}

// ---- the audit log (fit's audit_test.go) ----

/// fit's `TestAuditLog`: entries are bounded by `limits.max_audit_entries`, numbered across
/// restarts, picked by user, conversation and kind and paged; the store keeps them, and without
/// one, or with one that cannot write, they are kept in memory.
#[test]
fn audit_log() {
    let f = start(r#", "limits": {"max_audit_entries": 3}"#, vec![text("-")]);
    let a = &f.a;
    let long = "x".repeat(4000);
    let calls: Vec<audit::Call> = (0..40)
        .map(|i| audit::Call {
            tool: format!("t{i}"),
            args: long.clone(),
            error: false,
        })
        .collect();
    for i in 0..5 {
        a.audit(audit::Entry {
            user: ["ada", "bob"][i % 2].into(),
            kind: audit::KIND_CHAT.into(),
            conversation: format!("c{i}"),
            outcome: "answer".into(),
            calls: calls.clone(),
            detail: long.clone(),
            ..audit::Entry::default()
        });
    }
    let (all, total) = a.audit_entries(&Filter::default());
    assert!(
        total == 3
            && all.len() == 3
            && all[0].seq == 5
            && all[2].seq == 3
            && all[0].source == audit::SOURCE_PANEL
            && all[0].at > 0
            && all[0].calls.len() == audit::MAX_CALLS
            && all[0].more_calls == 8
            && all[0].calls[0].args.len() <= audit::MAX_ARGS + 4
            && all[0].detail.len() <= audit::MAX_DETAIL + 4,
        "{total} {:?}",
        all[0].seq
    );
    let pick = |f: Filter| a.audit_entries(&f).0;
    let e = pick(Filter {
        user: "ada".into(),
        ..Filter::default()
    });
    assert!(e.len() == 2 && e[0].conversation == "c4");
    let e = pick(Filter {
        conversation: "c3".into(),
        ..Filter::default()
    });
    assert!(e.len() == 1 && e[0].user == "bob");
    let e = pick(Filter {
        before: 5,
        limit: 1,
        ..Filter::default()
    });
    assert!(e.len() == 1 && e[0].seq == 4);
    assert!(
        pick(Filter {
            kind: audit::KIND_APPLY.into(),
            limit: 9999,
            ..Filter::default()
        })
        .is_empty()
    );
    a.settle();
    let sec = String::from_utf8(f.store.section(audit::SECTION).unwrap()).unwrap();
    assert!(sec.contains(r#""seq":5"#), "{sec}");

    // A restart: the entries and their numbers hold, within a bound now smaller.
    let store = Arc::clone(&f.store);
    let g = start_with(
        r#", "limits": {"max_audit_entries": 2}"#,
        vec![text("-")],
        |o| o.store = Some(store as Arc<dyn Store>),
    );
    g.a.audit(audit::Entry {
        user: "ada".into(),
        kind: audit::KIND_DISCARD.into(),
        outcome: audit::DISCARDED.into(),
        ..audit::Entry::default()
    });
    let (e, total) = g.a.audit_entries(&Filter::default());
    assert!(
        total == 2 && e[0].seq == 6 && e[1].seq == 5,
        "{total} {e:?}"
    );

    // A section that does not load is started again; a store that cannot write keeps them in
    // memory.
    let bad = Arc::new(Mem {
        sec: Mutex::new([(audit::SECTION.to_owned(), b"[1]".to_vec())].into()),
        fail: true,
    });
    let h = start_with("", vec![text("-")], |o| {
        o.store = Some(bad as Arc<dyn Store>);
    });
    h.a.audit(audit::Entry {
        user: "ada".into(),
        kind: audit::KIND_CHAT.into(),
        ..audit::Entry::default()
    });
    h.a.settle();
    let t = h.log_text();
    assert!(
        t.contains("does not load") && t.contains("kept in memory only"),
        "{t}"
    );
    // Without a store.
    let d = start_with("", vec![text("-")], |o| o.store = None);
    d.a.audit(audit::Entry {
        user: "ada".into(),
        kind: audit::KIND_CHAT.into(),
        ..audit::Entry::default()
    });
    assert_eq!(d.a.audit_entries(&Filter::default()).0.len(), 1);
}

/// fit's `TestAuditEndpoint`: `GET /api/audit` answers the log, filtered, with the console's users
/// and its bound.
#[test]
fn audit_endpoint() {
    let f = start(r#", "limits": {"max_audit_entries": 10}"#, vec![text("-")]);
    f.a.audit(audit::Entry {
        user: "bob".into(),
        kind: audit::KIND_CHAT.into(),
        conversation: "x".into(),
        outcome: "answer".into(),
        ..audit::Entry::default()
    });
    f.a.audit(audit::Entry {
        user: "ada".into(),
        kind: audit::KIND_APPLY.into(),
        conversation: "y".into(),
        outcome: audit::APPLIED.into(),
        version: 4,
        ..audit::Entry::default()
    });
    let (status, body) = f.get("/api/audit?user=ada&limit=5");
    assert!(
        status == 200
            && body.contains(r#""version":4"#)
            && !body.contains(r#""user":"bob""#)
            && body.contains(r#""users":["ada","bob"]"#)
            && body.contains(r#""max":10"#)
            && body.contains(r#""kept":true"#)
            && body.contains(r#""total":2"#),
        "{status} {body}"
    );
    let e = f.audit("?conversation=x&before=2");
    assert!(e.len() == 1 && s(&e[0], "user") == "bob", "{e:?}");
}
