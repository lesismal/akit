//! The assistant's chat (I162), with the cases of fit's `assistant/chat_test.go`: a question
//! answered from the tools, which read the Admin API in process; no secret in what reaches the
//! model; `ask_user` and its choice; the limit of tool calls; a result over its bound cut; a chat
//! stopped; conversations kept per user, apart, across a restart; and akit's linked backpressure,
//! a browser that does not read holding the model back. Against a scripted stand-in of a model
//! on port 0, never a real one.
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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use akio::{Backend, UringConfig};
use akit_admin::{Configs, Options, SOURCE_FILE, Server};
use akit_assistant::standin::{Server as Standin, Turn, scripted};
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

const PROVIDER_KEY: &str = "sk-chat-provider-0123456789";
const ROUTE_PASSWORD_HASH: &str = "$2b$04$abcdefghijklmnopqrstuuG2rmOVKzOv2Hd2t1Gc9i1dK1Szy/7Ri";

/// `akit run`'s console with an assistant calling `model` (a stand-in at its URL), `more` more
/// members of `admin.assistant`, a route with a secret, and the store `store`.
fn start(model: &str, more: &str, store: Option<Arc<Sections>>) -> (Server, SocketAddr) {
    start_with(model, r#"[{"name": "m"}]"#, more, store)
}

/// The access log's file of the config [`start_top`] gives, and akit's own log installed, once.
fn logs() -> &'static std::path::Path {
    static P: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        let d = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("assistant-chat");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let files = akit_observe::Files::new();
        let c = akit_config::Log {
            output: d.join("akit.log").to_string_lossy().into_owned(),
            ..akit_config::Log::default()
        };
        let root =
            akit_observe::logger::Root::new(Some(&c), akit_observe::Zone::utc(), &files).unwrap();
        akit_observe::logger::install(Arc::new(root));
        let access = d.join("access.log");
        std::fs::write(
            &access,
            "127.0.0.1 - - \"GET /app HTTP/1.1\" 200 2 route=app\n10.0.0.9 - - \"GET /app HTTP/1.1\" 401 0 route=app authorization=Bearer abcdefgh123456\n",
        )
        .unwrap();
        access
    })
}

/// [`start`], the provider's models `models`.
fn start_with(
    model: &str,
    models: &str,
    more: &str,
    store: Option<Arc<Sections>>,
) -> (Server, SocketAddr) {
    start_top(model, models, more, "", store)
}

/// [`start_with`], `top` more top-level members of the config.
fn start_top(
    model: &str,
    models: &str,
    more: &str,
    top: &str,
    store: Option<Arc<Sections>>,
) -> (Server, SocketAddr) {
    let hash = akio_crypto::bcrypt::hash(b"pw", akio_crypto::bcrypt::MIN_COST).unwrap();
    let text = format!(
        r#"{{"admin": {{"address": "127.0.0.1:0",
            "console": {{"address": "127.0.0.1:0", "users": [{{"username": "ada", "password_hash": "{hash}"}}, {{"username": "bob", "password_hash": "{hash}"}}]}},
            "assistant": {{"providers": [{{"name": "local", "kind": "openai_compatible", "base_url": "{model}/v1", "models": {models}}}],
              "users": [{{"username": "ada", "level": "apply"}}]{more}}}}},
          "listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],{top}
          "routes": [{{"name": "app", "match": {{"path": ["/app"]}},
            "middleware": [{{"module": "basic_auth", "params": {{"users": [{{"username": "u", "password_hash": "{ROUTE_PASSWORD_HASH}"}}]}}}}],
            "handler": {{"module": "respond", "params": {{"body": "ok"}}}}}}]}}"#
    );
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
    opts.assistant_engine = Some(engine());
    opts.assistant_store = store.map(|s| s as Arc<dyn akit_assistant::Store>);
    let api = Server::start(&settings, configs, submit, &opts).unwrap();
    let url = api.console_url().unwrap();
    let addr = url
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .parse()
        .unwrap();
    (api, addr)
}

struct Browser {
    addr: SocketAddr,
    cookie: String,
    csrf: String,
}

fn request(b: &Browser, method: &str, path: &str, body: Option<&str>) -> String {
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
    req
}

/// A whole answer: its status and body, dechunked.
fn send(b: &Browser, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut s = TcpStream::connect(b.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
    s.write_all(request(b, method, path, body).as_bytes())
        .unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    split(&raw)
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
            out.push_str(rest.get(..n.min(rest.len())).unwrap_or(""));
            break;
        }
        out.push_str(&rest[..n]);
        s = rest.get(n + 2..).unwrap_or("");
    }
    out
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

/// A chat's events: each SSE message's event name (empty for `message`) and its data.
fn events(body: &str) -> Vec<(String, Value)> {
    body.split("\n\n")
        .filter_map(|m| {
            let mut name = String::new();
            let mut data = None;
            for l in m.lines() {
                if let Some(n) = l.strip_prefix("event: ") {
                    name = n.to_owned();
                } else if let Some(d) = l.strip_prefix("data: ") {
                    data = parse(d).ok();
                }
            }
            data.map(|d| (name, d))
        })
        .collect()
}

fn typ(v: &Value) -> &str {
    match v.get("type") {
        Some(Value::String(s)) => s,
        _ => "",
    }
}

fn s<'v>(v: &'v Value, k: &str) -> &'v str {
    match v.get(k) {
        Some(Value::String(s)) => s,
        _ => "",
    }
}

fn chat(b: &Browser, body: &str) -> Vec<(String, Value)> {
    let (status, text) = send(b, "POST", "/api/m/ai/chat", Some(body));
    assert_eq!(status, 200, "{text}");
    events(&text)
}

fn last(evs: &[(String, Value)]) -> (&str, &Value) {
    let (n, v) = evs.last().unwrap();
    (n.as_str(), v)
}

/// fit's `TestChatAnswersFromTools`: the model calls a read tool, which reads the Admin API in
/// process; its result goes back fenced as data, and the answer streams; the events come in
/// fit's order; the conversation is kept and listed; no secret reaches the model.
#[test]
fn answers_from_tools() {
    let model = scripted(vec![
        Turn::Tools(vec![
            (
                "config_get".into(),
                r#"{"members": ["routes", "admin"]}"#.into(),
            ),
            ("status_get".into(), "{}".into()),
        ]),
        Turn::Text("The route app answers ok.".into()),
    ]);
    let (api, addr) = start(&model.url(), "", None);
    let b = login(addr, "ada");
    let evs = chat(&b, r#"{"message": "What does /app answer?", "lang": "en"}"#);
    let types: Vec<&str> = evs.iter().map(|(_, v)| typ(v)).collect();
    assert_eq!(types[0], "start", "{types:?}");
    assert!(
        types.contains(&"tool_call") && types.contains(&"tool_result"),
        "{types:?}"
    );
    assert!(types.contains(&"usage"), "{types:?}");
    let (name, end) = last(&evs);
    assert_eq!(
        (name, typ(end), s(end, "reason")),
        ("end", "done", "answer")
    );
    let conv = s(&evs[0].1, "conversation").to_owned();
    let text: String = evs
        .iter()
        .filter(|(_, v)| typ(v) == "text")
        .map(|(_, v)| s(v, "text").to_owned())
        .collect();
    assert_eq!(text, "The route app answers ok.");
    let result = evs
        .iter()
        .find(|(_, v)| typ(v) == "tool_result" && s(v, "name") == "config_get")
        .unwrap();
    assert!(
        s(&result.1, "content").contains("\"app\""),
        "{:?}",
        result.1
    );
    // What reached the model: the tool's result fenced, no secret.
    let reqs = model.requests();
    assert_eq!(reqs.len(), 2);
    let second = String::from_utf8_lossy(&reqs[1].body).into_owned();
    assert!(
        second.contains("<tool_output tool=\\\"config_get\\\">"),
        "{second}"
    );
    assert!(second.contains("What does /app answer?"));
    for r in &reqs {
        let body = String::from_utf8_lossy(&r.body);
        for secret in [
            ROUTE_PASSWORD_HASH,
            "$2b$04$",
            PROVIDER_KEY,
            "password_hash\":\"$",
        ] {
            assert!(!body.contains(secret), "{secret} reached the model");
        }
        assert!(
            !body.contains("\"assistant\":{"),
            "admin.assistant reached the model"
        );
    }
    // Kept and listed.
    let (st, list) = send(&b, "GET", "/api/m/ai/conversations", None);
    assert_eq!(st, 200);
    assert!(list.contains(&conv), "{list}");
    let (st, one) = send(&b, "GET", &format!("/api/m/ai/conversations/{conv}"), None);
    assert_eq!(st, 200);
    let v = parse(&one).unwrap();
    let Some(Value::Array(msgs)) = v.get("messages") else {
        panic!("{one}")
    };
    assert_eq!(msgs.len(), 5, "{one}"); // user, calls, two results, answer
    // The tools' list says what this user may call.
    let (_, tools) = send(&b, "GET", "/api/m/ai/tools", None);
    assert!(tools.contains(r#""name":"config_get""#) && tools.contains(r#""level":"apply""#));
    api.shutdown();
}

/// fit's `TestChatAsk`: `ask_user` ends the answer with its question, and the choice comes back
/// to the model as the call's result.
#[test]
fn ask_user() {
    let model = scripted(vec![
        Turn::Tools(vec![(
            "ask_user".into(),
            r#"{"question": "Which port?", "options": ["8080", "9090"]}"#.into(),
        )]),
        Turn::Text("Port 9090 it is.".into()),
    ]);
    let (api, addr) = start(&model.url(), "", None);
    let b = login(addr, "ada");
    let evs = chat(&b, r#"{"message": "Add a listener."}"#);
    let ask = evs.iter().find(|(_, v)| typ(v) == "ask").unwrap();
    assert_eq!(s(&ask.1, "question"), "Which port?");
    assert_eq!(s(last(&evs).1, "reason"), "ask");
    let conv = s(&evs[0].1, "conversation").to_owned();
    let id = s(&ask.1, "id").to_owned();
    // A choice not offered is refused.
    let (st, _) = send(
        &b,
        "POST",
        "/api/m/ai/chat",
        Some(&format!(
            r#"{{"conversation": "{conv}", "answer": {{"id": "{id}", "choice": "1"}}}}"#
        )),
    );
    assert_eq!(st, 400);
    let evs = chat(
        &b,
        &format!(r#"{{"conversation": "{conv}", "answer": {{"id": "{id}", "choice": "9090"}}}}"#),
    );
    assert_eq!(s(last(&evs).1, "reason"), "answer");
    let body = String::from_utf8_lossy(&model.requests()[1].body).into_owned();
    assert!(body.contains("9090") && body.contains(&id), "{body}");
    api.shutdown();
}

/// fit's `TestChatLimits`: past `max_tool_calls` the calls are refused and the answer ends with
/// the reason; a tool's result past `max_tool_result_bytes` is cut and says so; a message past
/// its bound, or with no model to call, is refused.
#[test]
fn limits() {
    let model = scripted(vec![Turn::Tools(vec![
        ("config_get".into(), "{}".into()),
        ("status_get".into(), "{}".into()),
        ("cache_zones".into(), "{}".into()),
    ])]);
    let (api, addr) = start(
        &model.url(),
        r#", "limits": {"max_tool_calls": 2, "max_tool_result_bytes": 200}"#,
        None,
    );
    let b = login(addr, "ada");
    let evs = chat(&b, r#"{"message": "Look around."}"#);
    assert_eq!(s(last(&evs).1, "reason"), "max_tool_calls");
    let results: Vec<&Value> = evs
        .iter()
        .filter(|(_, v)| typ(v) == "tool_result")
        .map(|(_, v)| v)
        .collect();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].get("cut"), Some(&Value::Bool(true)));
    assert!(s(results[0], "content").contains("[cut: the result is"));
    assert!(s(results[2], "content").contains("limit of 2 tool calls"));
    let big = format!(r#"{{"message": "{}"}}"#, "x".repeat(40 << 10));
    assert_eq!(send(&b, "POST", "/api/m/ai/chat", Some(&big)).0, 400);
    assert_eq!(
        send(&b, "POST", "/api/m/ai/chat", Some(r#"{"retry": true}"#)).0,
        400
    );
    api.shutdown();
}

/// fit's `TestChatStop` and akit's linked backpressure (decision row 102): while the browser
/// reads nothing, the model's answer is held once the events waiting pass 64 KiB, so the
/// stand-in cannot send it all; read, it comes whole; a chat stopped ends with `stopped`, what
/// came of its answer kept.
#[test]
fn backpressure_and_stop() {
    let sent = Arc::new(AtomicUsize::new(0));
    let s2 = Arc::clone(&sent);
    let total = 384; // pieces of 64 KiB: 24 MiB, past what sockets' buffers hold
    let model = Standin::start(move |r, c| {
        if r.method == "GET" {
            return c.whole(200, "application/json", b"{}");
        }
        c.head(200, &[("Content-Type", "text/event-stream")]);
        let piece = format!(
            "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{}\"}}}}]}}\n\n",
            "z".repeat(64 << 10)
        );
        for _ in 0..total {
            if !c.chunk(piece.as_bytes()) {
                return;
            }
            s2.fetch_add(1, Ordering::SeqCst);
        }
        c.chunk(
            b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        );
    });
    let (api, addr) = start(
        &model.url(),
        r#", "limits": {"max_stream_bytes": 33554432}"#,
        None,
    );
    let b = login(addr, "ada");
    let mut conn = TcpStream::connect(addr).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    conn.write_all(request(&b, "POST", "/api/m/ai/chat", Some(r#"{"message": "big"}"#)).as_bytes())
        .unwrap();
    // Nothing read: the model is held back.
    let start = Instant::now();
    let mut seen = 0;
    while start.elapsed() < Duration::from_secs(2) {
        seen = sent.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        seen > 0 && seen < total - 64,
        "the model sent {seen} of {total} pieces"
    );
    let mut raw = Vec::new();
    let _ = conn.read_to_end(&mut raw);
    let (status, body) = split(&raw);
    assert_eq!(status, 200);
    let evs = events(&body);
    let text: usize = evs
        .iter()
        .filter(|(_, v)| typ(v) == "text")
        .map(|(_, v)| s_len(v))
        .sum();
    assert_eq!(text, total * (64 << 10));
    assert_eq!(s(last(&evs).1, "reason"), "answer");

    // Stopped while the model is held.
    sent.store(0, Ordering::SeqCst);
    let mut conn = TcpStream::connect(addr).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    conn.write_all(
        request(
            &b,
            "POST",
            "/api/m/ai/chat",
            Some(r#"{"message": "again"}"#),
        )
        .as_bytes(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while sent.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let (_, list) = send(&b, "GET", "/api/m/ai/conversations", None);
    let v = parse(&list).unwrap();
    let Some(Value::Array(convs)) = v.get("conversations") else {
        panic!("{list}")
    };
    let busy = convs
        .iter()
        .find(|c| c.get("busy") == Some(&Value::Bool(true)))
        .unwrap();
    let id = s(busy, "id").to_owned();
    let (st, stopped) = send(
        &b,
        "POST",
        "/api/m/ai/chat/stop",
        Some(&format!(r#"{{"conversation": "{id}"}}"#)),
    );
    assert_eq!((st, stopped.trim()), (200, r#"{"stopped":true}"#));
    let mut raw = Vec::new();
    let _ = conn.read_to_end(&mut raw);
    let evs = events(&split(&raw).1);
    assert_eq!(s(last(&evs).1, "reason"), "stopped");
    let (_, one) = send(&b, "GET", &format!("/api/m/ai/conversations/{id}"), None);
    assert!(
        one.contains(r#""stopped":true"#),
        "{}",
        &one[..one.len().min(300)]
    );
    api.shutdown();
}

fn s_len(v: &Value) -> usize {
    s(v, "text").len()
}

/// fit's `TestConversationsKept`: each user's conversations are their own, kept in the store's
/// sections and read again after a restart; one deleted is gone.
#[test]
fn conversations_kept_apart() {
    let model = scripted(vec![Turn::Text("Hello.".into())]);
    let store = Arc::new(Sections::default());
    let (api, addr) = start(&model.url(), "", Some(Arc::clone(&store)));
    let (ada, bob) = (login(addr, "ada"), login(addr, "bob"));
    let a = s(
        &chat(&ada, r#"{"message": "hi from ada"}"#)[0].1,
        "conversation",
    )
    .to_owned();
    let b = s(
        &chat(&bob, r#"{"message": "hi from bob"}"#)[0].1,
        "conversation",
    )
    .to_owned();
    assert_eq!(
        send(&bob, "GET", &format!("/api/m/ai/conversations/{a}"), None).0,
        404
    );
    // Written to the store, a section a user.
    let deadline = Instant::now() + Duration::from_secs(5);
    while store.0.lock().unwrap().len() < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let names: Vec<String> = store.0.lock().unwrap().keys().cloned().collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names.iter().all(|n| n.starts_with("assistant.chats.")));
    api.shutdown();
    let (api, addr) = start(&model.url(), "", Some(Arc::clone(&store)));
    let ada = login(addr, "ada");
    let (_, list) = send(&ada, "GET", "/api/m/ai/conversations", None);
    assert!(list.contains(&a) && !list.contains(&b), "{list}");
    assert!(list.contains(r#""kept":true"#));
    let (st, _) = send(
        &ada,
        "DELETE",
        &format!("/api/m/ai/conversations/{a}"),
        None,
    );
    assert_eq!(st, 200);
    let (_, list) = send(&ada, "GET", "/api/m/ai/conversations", None);
    assert!(!list.contains(&a), "{list}");
    api.shutdown();
}

/// fit's `TestChatSummarizes`: a conversation past its model's window has its early turns
/// summarized by the model, the summary kept and sent in their place.
#[test]
fn summarizes() {
    let model = scripted(vec![
        Turn::Text("First answer.".into()),
        Turn::Text("SUMMARY: the person asked about routes.".into()),
        Turn::Text("Second answer.".into()),
    ]);
    let (api, addr) = start_with(
        &model.url(),
        r#"[{"name": "m", "context": 2500, "tools": false}]"#,
        "",
        None,
    );
    let b = login(addr, "ada");
    let long = "word ".repeat(480);
    let evs = chat(&b, &format!(r#"{{"message": "{long}one"}}"#));
    assert_eq!(s(last(&evs).1, "reason"), "answer");
    let conv = s(&evs[0].1, "conversation").to_owned();
    let evs = chat(
        &b,
        &format!(r#"{{"conversation": "{conv}", "message": "{long}two"}}"#),
    );
    let sum = evs.iter().find(|(_, v)| typ(v) == "summary").unwrap();
    assert_eq!(sum.1.get("folded"), Some(&Value::Number("2".into())));
    assert_eq!(s(last(&evs).1, "reason"), "answer");
    let reqs = model.requests();
    // The first answer, the summary, the second answer.
    assert_eq!(reqs.len(), 3);
    let summary_call = String::from_utf8_lossy(&reqs[1].body).into_owned();
    assert!(
        summary_call.contains("You summarize a conversation"),
        "{summary_call}"
    );
    let second = String::from_utf8_lossy(&reqs[2].body).into_owned();
    assert!(
        second.contains("<summary>\\nSUMMARY: the person asked about routes."),
        "{second}"
    );
    assert!(
        !second.contains("word one"),
        "the early turn was sent again"
    );
    api.shutdown();
}

/// fit's `TestLogTools` (I162b): `log_tail` reads akit's own log's latest messages and
/// `access_log_sample` the end of the access log's file, each only with its `data` setting on
/// (without it the model is told there is no such tool), redacted; `adapt_convert` converts nginx.
#[test]
fn logs_and_converter() {
    let access = logs();
    akit_observe::logger::log(
        akit_observe::logger::Logger::Tls,
        akit_observe::logger::Level::Warn,
        format_args!("chat-logs-test: certificate for a.example expires soon"),
    );
    let top = format!(
        r#""access_log": {{"output": "{}", "format": "text"}},"#,
        access.display()
    );
    let model = scripted(vec![
        Turn::Tools(vec![
            ("log_tail".into(), r#"{"contains": "chat-logs-test", "level": "warn"}"#.into()),
            ("access_log_sample".into(), r#"{"status": 401}"#.into()),
            (
                "adapt_convert".into(),
                r#"{"from": "nginx", "text": "http { server { listen 8080; location / { return 200 hi; } } }"}"#.into(),
            ),
        ]),
        Turn::Text("Done.".into()),
    ]);
    let (api, addr) = start_top(
        &model.url(),
        r#"[{"name": "m"}]"#,
        r#", "data": {"send_logs": true, "send_access_log": true}"#,
        &top,
        None,
    );
    let b = login(addr, "ada");
    let evs = chat(&b, r#"{"message": "What went wrong?"}"#);
    let result = |name: &str| {
        evs.iter()
            .find(|(_, v)| typ(v) == "tool_result" && s(v, "name") == name)
            .map(|(_, v)| (s(v, "content").to_owned(), v.get("error").is_some()))
            .unwrap()
    };
    let (logs, err) = result("log_tail");
    assert!(
        !err && logs.contains("warn tls: chat-logs-test: certificate"),
        "{logs}"
    );
    let (lines, err) = result("access_log_sample");
    assert!(
        !err && lines.contains("10.0.0.9") && !lines.contains("127.0.0.1 -"),
        "{lines}"
    );
    assert!(
        !lines.contains("abcdefgh123456"),
        "a token in the access log reached the model"
    );
    let (conv, err) = result("adapt_convert");
    assert!(
        !err && conv.contains("\"config\"") && conv.contains("8080"),
        "{conv}"
    );
    api.shutdown();

    // With the settings off, the tools are not offered.
    let model = scripted(vec![
        Turn::Tools(vec![("log_tail".into(), "{}".into())]),
        Turn::Text("Done.".into()),
    ]);
    let (api, addr) = start_top(&model.url(), r#"[{"name": "m"}]"#, "", &top, None);
    let b = login(addr, "ada");
    let evs = chat(&b, r#"{"message": "Logs?"}"#);
    let r = evs.iter().find(|(_, v)| typ(v) == "tool_result").unwrap();
    assert!(
        s(&r.1, "content").contains("there is no tool log_tail"),
        "{:?}",
        r.1
    );
    let (_, tools) = send(&b, "GET", "/api/m/ai/tools", None);
    assert!(tools.contains("data.send_logs is off"), "{tools}");
    api.shutdown();
}

/// The skills through the console (I164): the built-ins listed, a skill added (kept in the
/// store's section), run by `/name` with its prompt in the system prompt and its tools only, and
/// the model's own `skill_load` of a built-in.
#[test]
fn skills_through_the_console() {
    let model = scripted(vec![
        Turn::Text("All bound.".into()),
        Turn::Tools(vec![(
            "skill_load".into(),
            r#"{"name": "diagnose"}"#.into(),
        )]),
        Turn::Text("Loaded.".into()),
    ]);
    let store = Arc::new(Sections::default());
    let (api, addr) = start(&model.url(), "", Some(Arc::clone(&store)));
    let b = login(addr, "ada");
    let (status, list) = send(&b, "GET", "/api/m/ai/skills", None);
    assert!(
        status == 200
            && list.contains(r#""name":"route-builder""#)
            && list.contains(r#""can_change":true"#),
        "{status} {list}"
    );
    let src = "---\nname: ops-check\ntitle: Ops check\ndescription: The morning check.\ntools: [status_get]\n---\nRead the status and say whether the listeners are bound.\n";
    let body = format!(r#"{{"source": {}}}"#, akit_assistant::json::quoted(src));
    let (status, out) = send(&b, "POST", "/api/m/ai/skills", Some(&body));
    assert_eq!(status, 200, "{out}");
    let kept = store
        .0
        .lock()
        .unwrap()
        .get("assistant.skills")
        .cloned()
        .unwrap_or_default();
    assert!(String::from_utf8_lossy(&kept).contains("ops-check"));

    let evs = chat(&b, r#"{"message": "/ops-check is everything bound?"}"#);
    assert_eq!(s(&evs[0].1, "skill"), "ops-check", "{evs:?}");
    let first = model
        .requests()
        .into_iter()
        .find(|r| r.method == "POST")
        .unwrap();
    let sent = String::from_utf8_lossy(&first.body).into_owned();
    assert!(
        sent.contains("The current skill is ops-check")
            && sent.contains(r#""name":"status_get""#)
            && !sent.contains(r#""name":"config_get""#),
        "{sent}"
    );

    let evs = chat(&b, r#"{"message": "why does /app answer 401?"}"#);
    assert!(
        evs.iter()
            .any(|(_, v)| typ(v) == "skill" && s(v, "skill") == "diagnose"),
        "{evs:?}"
    );
    api.shutdown();
}
