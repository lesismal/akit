//! `media_pages` (I125) through akit's listener, as `akit run` serves it: fit's
//! `handler/mediapages` tests ported (`pages_test.go`, `chat_test.go`): the pages, their assets
//! and the config API; the login and its cookie; the room API with its events, limits and member
//! TTL; a slow watcher sent only the latest state and cut after `stall_timeout`; a reload that
//! keeps the rooms; the chat's text and files, its history and its bounds.
//!
//! Every wait is on what the test can see, with a deadline.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use akio::Engine;
use akio_crypto::base64::STANDARD as B64;
use akio_crypto::bcrypt;
use akit_config::value::{Value, parse};
use akit_roles::host::Manager;
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, Registry, compile};

const T: Duration = Duration::from_secs(15);
const TOKEN: &str = "0123456789abcdef-token";

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + T;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A config with media_pages on /media/*, its params `params`, and a policy "live": ann may
/// publish and play room1's streams, viv (a bcrypt user) may play them only, and a static token
/// may do anything.
fn conf(params: &str) -> String {
    static HASH: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| bcrypt::hash(b"viv-password", bcrypt::MIN_COST).unwrap());
    format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
          "routes": [{{"match": {{"path": ["/media", "/media/*"]}}, "handler": {{"module": "media_pages", "params": {params}}}}}],
          "media_auth": [{{"name": "live",
            "users": [
              {{"name": "ann", "password": "ann-password", "publish": ["room1-*"], "play": ["room1-*"]}},
              {{"name": "viv", "password_hash": "{}", "play": ["room1-*"]}}],
            "tokens": [{{"name": "all", "token": "{TOKEN}", "publish": ["*"], "play": ["*"]}}]}}]}}"#,
        *HASH
    )
}

struct Akit {
    engine: Arc<Engine>,
    roles: Manager,
    cell: GraphCell,
    registry: Registry<akit_runtime::HandlerParams>,
    slot: EngineSlot,
    addr: SocketAddr,
}

impl Drop for Akit {
    fn drop(&mut self) {
        self.roles.stop(Duration::ZERO);
        self.engine.shutdown();
    }
}

fn start(json: &str) -> Akit {
    let config = akit_config::from_json(json, None).unwrap_or_else(|e| panic!("{json}: {e}"));
    let slot = EngineSlot::new();
    let registry = Registry::new(&[akit_media::MODULES]).unwrap();
    let cell = GraphCell::new(compile(&config, &registry, &Env::new(slot.clone())).unwrap());
    let engine =
        Arc::new(akit_listener::engine(&cell, &akit_listener::Options::default()).unwrap());
    engine.start().unwrap();
    assert!(slot.set(Arc::clone(&engine)));
    let addr = engine.local_addrs()[0];
    let mut roles = Manager::new(&[akit_media::role::MEDIA_AUTH], Arc::clone(&engine), None);
    let errs = roles.apply(&config);
    assert!(errs.is_empty(), "{errs:?}");
    Akit {
        engine,
        roles,
        cell,
        registry,
        slot,
        addr,
    }
}

impl Akit {
    fn reload(&mut self, json: &str) {
        let config = akit_config::from_json(json, None).unwrap();
        let errs = self.roles.apply(&config);
        assert!(errs.is_empty(), "{errs:?}");
        let g = compile(&config, &self.registry, &Env::new(self.slot.clone())).unwrap();
        self.cell.swap(g);
        self.cell.collect();
    }

    fn stats(&self) -> Option<akit_media::pages::Stats> {
        akit_media::pages::stats(akit_media::auth::registry::key(&self.engine), "/media")
    }

    fn join(&self, room: &str, body: &str, fields: &[(&str, &str)]) -> (u16, Value) {
        let r = http(
            self.addr,
            "POST",
            &format!("/media/api/rooms/{room}/join"),
            fields,
            body.as_bytes(),
        );
        (r.status, parse(&r.text()).unwrap_or(Value::Null))
    }

    /// A member's key, joined with `fields`' credentials.
    fn key(&self, room: &str, name: &str, fields: &[(&str, &str)]) -> String {
        let (code, v) = self.join(room, &format!(r#"{{"name": "{name}"}}"#), fields);
        assert_eq!(code, 200, "{name} joins {room}: {v:?}");
        s(&v, "key")
    }
}

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

fn http(
    addr: SocketAddr,
    method: &str,
    target: &str,
    fields: &[(&str, &str)],
    body: &[u8],
) -> Resp {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(T)).unwrap();
    let mut req = format!("{method} {target} HTTP/1.1\r\nHost: akit\r\nConnection: close\r\n");
    for (n, v) in fields {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    req.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    s.write_all(req.as_bytes()).unwrap();
    s.write_all(body).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let split = out
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(out.len());
    let head = String::from_utf8_lossy(&out[..split]).into_owned();
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Resp {
        status,
        headers,
        body: out.get(split + 4..).unwrap_or_default().to_vec(),
    }
}

fn get(a: &Akit, target: &str, fields: &[(&str, &str)]) -> Resp {
    http(a.addr, "GET", target, fields, b"")
}

fn post(a: &Akit, target: &str, body: &str, fields: &[(&str, &str)]) -> Resp {
    http(a.addr, "POST", target, fields, body.as_bytes())
}

fn basic(user: &str, pw: &str) -> String {
    format!("Basic {}", B64.encode(format!("{user}:{pw}")))
}

fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn b(v: &Value, k: &str) -> Option<bool> {
    match v.get(k) {
        Some(Value::Bool(b)) => Some(*b),
        _ => None,
    }
}

fn n(v: &Value, k: &str) -> i64 {
    match v.get(k) {
        Some(Value::Number(n)) => n.parse().unwrap_or(-1),
        _ => -1,
    }
}

/// One server-sent event.
#[derive(Debug)]
struct Event {
    name: String,
    data: Value,
}

/// A room's event stream, read on a thread of its own.
struct Events {
    rx: Receiver<Event>,
    sock: TcpStream,
}

impl Drop for Events {
    fn drop(&mut self) {
        let _ = self.sock.shutdown(std::net::Shutdown::Both);
    }
}

fn follow(a: &Akit, room: &str, fields: &[(&str, &str)]) -> Events {
    let mut sock = TcpStream::connect(a.addr).unwrap();
    let mut req = format!("GET /media/api/rooms/{room}/events HTTP/1.1\r\nHost: akit\r\n");
    for (n, v) in fields {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    req.push_str("\r\n");
    sock.write_all(req.as_bytes()).unwrap();
    let mut r = BufReader::new(sock.try_clone().unwrap());
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    assert!(line.contains(" 200 "), "events: {line}");
    loop {
        let mut l = String::new();
        r.read_line(&mut l).unwrap();
        if l.trim_end().is_empty() {
            break;
        }
        if l.to_ascii_lowercase().starts_with("content-type:") {
            assert!(l.contains("text/event-stream"), "{l}");
        }
    }
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let (mut name, mut data) = (String::new(), String::new());
        loop {
            let mut l = String::new();
            match r.read_line(&mut l) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let l = l.trim_end_matches(['\r', '\n']);
            // The chunked framing's size lines and empty lines.
            if let Some(v) = l.strip_prefix("event: ") {
                name = v.to_owned();
            } else if let Some(v) = l.strip_prefix("data: ") {
                data = v.to_owned();
            } else if l.is_empty() && !name.is_empty() {
                let ev = Event {
                    name: std::mem::take(&mut name),
                    data: parse(&std::mem::take(&mut data)).unwrap_or(Value::Null),
                };
                if tx.send(ev).is_err() {
                    return;
                }
            }
        }
    });
    Events { rx, sock }
}

impl Events {
    /// The next event `f` takes, the others skipped.
    fn wait(&self, what: &str, mut f: impl FnMut(&Event) -> bool) -> Event {
        let deadline = Instant::now() + T;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let e = self
                .rx
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
            if f(&e) {
                return e;
            }
        }
    }

    /// The next room snapshot whose members' names are `names`, comma-joined.
    fn room(&self, what: &str, names: &str) -> Vec<Value> {
        let e = self.wait(what, |e| e.name == "room" && member_names(&e.data) == names);
        members(&e.data)
    }
}

fn members(v: &Value) -> Vec<Value> {
    match v.get("members") {
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    }
}

fn member_names(v: &Value) -> String {
    members(v)
        .iter()
        .map(|m| s(m, "name"))
        .collect::<Vec<_>>()
        .join(",")
}

/// The pages, their assets and the config API (fit's `TestPages`).
#[test]
fn pages() {
    let a = start(&conf(
        r#"{"auth": "live", "login": true, "title": "Our media", "hls": "/hls/{stream}/index.m3u8",
            "whep": "https://media.example.com/whep/{stream}",
            "links": [{"name": "RTMP", "url": "rtmp://{host}/live/{stream}?token={token}"}],
            "ice_servers": [{"urls": ["stun:stun.example.com:3478"]}]}"#,
    ));
    let r = get(&a, "/media?room=x", &[]);
    assert_eq!(r.status, 308);
    assert_eq!(r.header("location"), Some("/media/?room=x"));
    for p in ["/media/", "/media/call", "/media/publish", "/media/play"] {
        let r = get(&a, p, &[]);
        let csp = r.header("content-security-policy").unwrap_or("");
        let body = r.text();
        assert_eq!(r.status, 200, "{p}");
        assert!(body.contains(r#"<base href="/media/">"#), "{p}");
        assert!(body.contains("<title>Our media</title>"), "{p}");
        assert!(
            csp.contains("connect-src 'self' https://media.example.com"),
            "{csp}"
        );
        assert!(csp.contains("script-src 'self'"), "{csp}");
    }
    let r = get(&a, "/media/app.js", &[]);
    let etag = r.header("etag").unwrap().to_owned();
    assert_eq!(r.status, 200);
    assert!(
        r.header("content-type")
            .unwrap()
            .starts_with("text/javascript")
    );
    assert!(r.text().contains("replaceTrack"));
    assert!(r.text().contains("X-Akit-Member"));
    assert_eq!(
        get(&a, "/media/app.js", &[("If-None-Match", &etag)]).status,
        304
    );
    let r = get(&a, "/media/app.css", &[]);
    assert!(r.status == 200 && r.header("content-type").unwrap().starts_with("text/css"));
    assert_eq!(get(&a, "/media/vendor/hls.min.js", &[]).status, 200);
    for p in [
        "/media/nothing",
        "/media/api/nothing",
        "/media/api/rooms/bad%20name/join",
        "/media/web/app.js",
    ] {
        assert_eq!(get(&a, p, &[]).status, 404, "{p}");
    }
    assert_eq!(post(&a, "/media/", "x", &[]).status, 405);
    let r = get(&a, "/media/api/config", &[]);
    assert_eq!(r.status, 200);
    let c = parse(&r.text()).unwrap();
    assert_eq!(s(&c, "title"), "Our media");
    assert_eq!(s(&c, "whip"), "/whip/{stream}");
    assert_eq!(s(&c, "whep"), "https://media.example.com/whep/{stream}");
    assert_eq!((b(&c, "login"), b(&c, "auth")), (Some(true), Some(true)));
    let public = c.get("public").unwrap();
    assert_eq!(b(public, "play"), Some(false));
    assert!(matches!(c.get("links"), Some(Value::Array(l)) if l.len() == 1));
    assert!(matches!(c.get("iceServers"), Some(Value::Array(l)) if l.len() == 1));
    let room = c.get("room").unwrap();
    assert_eq!((n(room, "maxMembers"), n(room, "heartbeatMs")), (16, 10000));
    let call = c.get("call").unwrap();
    assert_eq!((b(call, "mic"), b(call, "cam")), (Some(false), Some(false)));
    let chat = c.get("chat").unwrap();
    assert_eq!(
        (b(chat, "enabled"), n(chat, "maxFileBytes")),
        (Some(true), 5 << 20)
    );
    assert_eq!(post(&a, "/media/api/config", "{}", &[]).status, 405);
}

/// The login answers a right password with a token as a cookie, and the token lets its holder
/// into the room API (fit's `TestLogin`).
#[test]
fn login() {
    let a = start(&conf(r#"{"auth": "live", "login": true}"#));
    let login = "/media/api/login";
    assert_eq!(
        post(&a, login, r#"{"user": "ann", "password": "wrong"}"#, &[]).status,
        401
    );
    assert_eq!(
        post(&a, login, r#"{"user": "nobody", "password": "x"}"#, &[]).status,
        401
    );
    assert_eq!(post(&a, login, "not json", &[]).status, 400);
    assert_eq!(get(&a, login, &[]).status, 405);
    let r = post(
        &a,
        login,
        r#"{"user": "ann", "password": "ann-password"}"#,
        &[],
    );
    assert_eq!(r.status, 200, "{}", r.text());
    let v = parse(&r.text()).unwrap();
    let token = s(&v, "token");
    assert!(token.starts_with("fm1."), "{token}");
    assert_eq!(s(&v, "subject"), "ann");
    assert!(matches!(v.get("publish"), Some(Value::Array(p)) if p.len() == 1));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!(n(&v, "expires") >= now + 11 * 3600);
    let ck = r.header("set-cookie").unwrap().to_owned();
    for want in [
        format!("akit_media_token={token}"),
        "Path=/".into(),
        "HttpOnly".into(),
        "SameSite=Strict".into(),
        "Max-Age=".into(),
    ] {
        assert!(ck.contains(&want), "{ck} lacks {want}");
    }
    assert!(!ck.contains("Secure"), "Secure without TLS: {ck}");
    // The cookie, and the token as a bearer token, let ann in.
    let cookie = format!("akit_media_token={token}");
    let (code, _) = a.join(
        "room1",
        r#"{"name": "ann", "publishing": true}"#,
        &[("Cookie", &cookie)],
    );
    assert_eq!(code, 200);
    let bearer = format!("Bearer {token}");
    let (code, _) = a.join("room2", r#"{"name": "ann"}"#, &[("Authorization", &bearer)]);
    assert_eq!(code, 403);
    // A bcrypt user, checked on the pool.
    let r = post(
        &a,
        login,
        r#"{"user": "viv", "password": "viv-password"}"#,
        &[],
    );
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.text().contains(r#""subject":"viv""#) && r.text().contains(r#""publish":[]"#));
    let r = post(&a, "/media/api/logout", "{}", &[]);
    assert!(r.status == 200 && r.header("set-cookie").unwrap().contains("Max-Age=0"));
    let big = format!(r#"{{"user": "ann", "password": "{}"}}"#, "x".repeat(2000));
    assert_eq!(post(&a, login, &big, &[]).status, 413);
    // Without login, there is no login.
    let g = start(&conf(r#"{"auth": "live"}"#));
    let r = post(
        &g,
        login,
        r#"{"user": "ann", "password": "ann-password"}"#,
        &[],
    );
    assert_eq!(r.status, 404);
}

/// The params are checked (fit's `TestParams` and `TestLoginWithoutUsers`).
#[test]
fn params() {
    for (params, want) in [
        (r#"{"prefix": "media"}"#, "prefix"),
        (r#"{"prefix": "/a b"}"#, "prefix"),
        (r#"{"login": true}"#, "login"),
        (r#"{"auth": "nope"}"#, "no media_auth policy"),
        (r#"{"whip": "/whip/"}"#, "{stream}"),
        (r#"{"whep": "ftp://x/{stream}"}"#, "neither"),
        (r#"{"hls": "/x/{stream}\"><script>"}"#, "{stream}"),
        (r#"{"title": "<b>"}"#, "title"),
        (r#"{"links": [{"name": "x", "url": "a\"b"}]}"#, "link"),
        (r#"{"member_ttl": "1s"}"#, "member_ttl"),
        (r#"{"max_file_bytes": 1023}"#, "at least 1024"),
        (r#"{"max_file_bytes": 33554433}"#, "at most 33554432"),
        (r#"{"libraries": ["x"]}"#, "there is no media_library named"),
        (r#"{"max_members": 257}"#, "max_members"),
    ] {
        let err = match akit_config::from_json(&conf(params), None) {
            Err(e) => e.to_string(),
            Ok(c) => {
                let registry = Registry::new(&[akit_media::MODULES]).unwrap();
                match compile(&c, &registry, &Env::new(EngineSlot::new())) {
                    Err(e) => e.to_string(),
                    Ok(_) => String::new(),
                }
            }
        };
        assert!(err.contains(want), "{params}: {err:?}, want {want:?}");
    }
    // A login with a policy of no users.
    let doc = r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}],
        "media_auth": [{"name": "p", "public": ["play"]}],
        "routes": [{"handler": {"module": "media_pages", "params": {"auth": "p", "login": true}}}]}"#;
    let c = akit_config::from_json(doc, None).unwrap();
    let registry = Registry::new(&[akit_media::MODULES]).unwrap();
    let err = compile(&c, &registry, &Env::new(EngineSlot::new()))
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(err.contains("has no users"), "{err}");
}

/// Members join, announce their switches and leave, and the watchers of the room see each
/// change; the room API checks the policy (fit's `TestRoom`).
#[test]
fn room() {
    let a = start(&conf(r#"{"auth": "live", "max_members": 3}"#));
    let ann = basic("ann", "ann-password");
    let viv = basic("viv", "viv-password");
    let bearer = format!("Bearer {TOKEN}");
    let r = get(&a, "/media/api/rooms/room1/events", &[]);
    assert!(r.status == 401 && r.header("www-authenticate").is_some());
    assert_eq!(a.join("room1", r#"{"name": "ann"}"#, &[]).0, 401);
    let r = get(
        &a,
        "/media/api/rooms/room9/events",
        &[("Authorization", &ann)],
    );
    assert_eq!(r.status, 403);

    let w = follow(&a, "room1", &[("Authorization", &viv)]);
    w.room("the empty room", "");
    let (code, v) = a.join(
        "room1",
        r#"{"name": "ann", "audio": true, "video": true, "audioDevice": true, "videoDevice": false}"#,
        &[("Authorization", &ann)],
    );
    assert_eq!(code, 200, "{v:?}");
    assert_eq!(
        (b(&v, "publish"), s(&v, "stream")),
        (Some(true), "room1-ann".into())
    );
    let ann_key = s(&v, "key");
    assert_eq!(ann_key.len(), 32);
    let m = w.room("ann", "ann");
    assert_eq!(
        (
            b(&m[0], "audio"),
            b(&m[0], "video"),
            b(&m[0], "audioDevice")
        ),
        (Some(true), Some(true), Some(true))
    );
    assert_eq!(
        (b(&m[0], "videoDevice"), b(&m[0], "publishing")),
        (Some(false), Some(false))
    );
    // viv may play, not publish.
    let (code, v) = a.join("room1", r#"{"name": "viv"}"#, &[("Authorization", &viv)]);
    assert!(
        code == 200 && b(&v, "publish") == Some(false),
        "{code} {v:?}"
    );
    let viv_key = s(&v, "key");
    let (code, _) = a.join(
        "room1",
        r#"{"name": "viv2", "publishing": true}"#,
        &[("Authorization", &viv)],
    );
    assert_eq!(code, 403);
    let r = post(
        &a,
        "/media/api/rooms/room1/members/viv",
        &format!(r#"{{"key": "{viv_key}", "publishing": true}}"#),
        &[],
    );
    assert_eq!(r.status, 403);
    w.room("ann and viv", "ann,viv");
    // ann switches her microphone off and publishes.
    let r = post(
        &a,
        "/media/api/rooms/room1/members/ann",
        &format!(r#"{{"key": "{ann_key}", "audio": false, "video": true, "publishing": true}}"#),
        &[],
    );
    assert_eq!(r.status, 200);
    w.wait("ann's microphone off", |e| {
        let m = members(&e.data);
        e.name == "room"
            && m.len() == 2
            && b(&m[0], "audio") == Some(false)
            && b(&m[0], "publishing") == Some(true)
    });
    let r = post(
        &a,
        "/media/api/rooms/room1/members/ann",
        r#"{"key": "nope"}"#,
        &[],
    );
    assert_eq!(r.status, 404);
    // The name is ann's: 409 for another, and ann's own key takes it back.
    let auth = [("Authorization", bearer.as_str())];
    assert_eq!(a.join("room1", r#"{"name": "ann"}"#, &auth).0, 409);
    let (code, v) = a.join(
        "room1",
        &format!(r#"{{"name": "ann", "key": "{ann_key}", "audio": true}}"#),
        &auth,
    );
    assert!(code == 200 && s(&v, "key") == ann_key, "{code} {v:?}");
    assert_eq!(a.join("room1", r#"{"name": "third"}"#, &auth).0, 200);
    assert_eq!(a.join("room1", r#"{"name": "fourth"}"#, &auth).0, 503);
    assert_eq!(a.join("room1", r#"{"name": "bad name"}"#, &auth).0, 400);
    let big = format!(r#"{{"name": "x", "pad": "{}"}}"#, "x".repeat(2000));
    assert_eq!(a.join("room1", &big, &auth).0, 413);
    // ann leaves.
    let leave = format!(r#"{{"name": "ann", "key": "{ann_key}"}}"#);
    assert_eq!(
        post(&a, "/media/api/rooms/room1/leave", &leave, &[]).status,
        200
    );
    assert_eq!(
        post(&a, "/media/api/rooms/room1/leave", &leave, &[]).status,
        404
    );
    w.room("ann gone", "viv,third");
    assert_eq!(get(&a, "/media/api/rooms/room1/join", &auth).status, 405);
}

/// A member without a heartbeat leaves after member_ttl; a room with no one is forgotten;
/// max_rooms bounds the rooms (fit's `TestRoomTTLAndLimits`).
#[test]
fn room_ttl_and_limits() {
    let a = start(&conf(
        r#"{"max_rooms": 1, "max_members": 1, "member_ttl": "3s"}"#,
    ));
    assert_eq!(a.join("a", r#"{"name": "x"}"#, &[]).0, 200);
    assert_eq!(a.join("b", r#"{"name": "x"}"#, &[]).0, 503);
    assert_eq!(get(&a, "/media/api/rooms/b/events", &[]).status, 503);
    let w = follow(&a, "a", &[]);
    w.room("x", "x");
    // Two watchers a member: one more is refused.
    let w2 = follow(&a, "a", &[]);
    assert_eq!(get(&a, "/media/api/rooms/a/events", &[]).status, 503);
    drop(w2);
    let start = Instant::now();
    w.room("x gone quiet", "");
    assert!(
        start.elapsed() >= Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
    drop(w);
    wait_for("room a forgotten", || {
        a.stats().is_some_and(|s| s.rooms == 0)
    });
    assert_eq!(a.join("b", r#"{"name": "x"}"#, &[]).0, 200);
}

/// A watcher that goes is forgotten; one that does not take its events is cut off after
/// stall_timeout, while the room goes on for the rest (fit's `TestWatcherCutAndSlow`).
#[test]
fn watcher_cut_and_slow() {
    let a = start(&conf(r#"{"stall_timeout": "1s", "max_members": 256}"#));
    let w = follow(&a, "r", &[]);
    w.wait("the room", |e| e.name == "room");
    drop(w);
    wait_for("the cut watcher forgotten", || {
        a.stats().is_some_and(|s| s.watchers == 0)
    });
    // A watcher that reads nothing, with a small receive buffer.
    let mut slow = TcpStream::connect(a.addr).unwrap();
    slow.write_all(b"GET /media/api/rooms/r/events HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    wait_for("the slow watcher", || {
        a.stats().is_some_and(|s| s.watchers == 1)
    });
    let good = follow(&a, "r", &[]);
    let name = "n".repeat(60);
    let mut keys = Vec::new();
    for i in 0..100u8 {
        let n = format!(
            "{name}{}{}",
            char::from(b'A' + i / 26),
            char::from(b'a' + i % 26)
        );
        let (code, v) = a.join("r", &format!(r#"{{"name": "{n}"}}"#), &[]);
        assert_eq!(code, 200);
        keys.push((n, s(&v, "key")));
    }
    // The members change state until the slow watcher is cut off, however much the kernel's
    // buffers take first (Linux's grow to megabytes), then each ends with its microphone off.
    let addr = a.addr;
    let cut = Arc::new(AtomicBool::new(false));
    let threads: Vec<_> = keys
        .into_iter()
        .map(|(n, k)| {
            let cut = Arc::clone(&cut);
            std::thread::spawn(move || {
                let update = |audio: bool| {
                    let r = http(
                        addr,
                        "POST",
                        &format!("/media/api/rooms/r/members/{n}"),
                        &[],
                        format!(r#"{{"key": "{k}", "audio": {audio}}}"#).as_bytes(),
                    );
                    assert_eq!(r.status, 200);
                };
                let mut j = 0u32;
                while !cut.load(Ordering::SeqCst) && j < 5000 {
                    update(j % 2 == 0);
                    j += 1;
                }
                update(false);
            })
        })
        .collect();
    wait_for("the slow watcher cut off", || {
        a.stats().is_some_and(|s| s.watchers == 1)
    });
    cut.store(true, Ordering::SeqCst);
    for t in threads {
        t.join().unwrap();
    }
    good.wait("every member's last state", |e| {
        let m = members(&e.data);
        e.name == "room" && m.len() == 100 && m.iter().all(|x| b(x, "audio") == Some(false))
    });
    assert_eq!(a.stats().unwrap().watchers, 1);
    drop(slow);
}

/// A reload keeps the rooms and their members, and takes the new settings; the last config gone,
/// the rooms go (fit's `TestReload`; akit's streams follow the room on under the new config, with
/// no drain).
#[test]
fn reload() {
    let mut a = start(&conf("{}"));
    assert_eq!(a.join("r", r#"{"name": "x"}"#, &[]).0, 200);
    let w = follow(&a, "r", &[]);
    w.room("x", "x");
    a.reload(&conf(r#"{"max_members": 2}"#));
    let w2 = follow(&a, "r", &[]);
    w2.room("x, kept", "x");
    assert_eq!(a.join("r", r#"{"name": "y"}"#, &[]).0, 200);
    w.room("y, on the stream from before the reload", "x,y");
    assert_eq!(a.join("r", r#"{"name": "z"}"#, &[]).0, 503);
    drop((w, w2));
    a.reload(r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}]}"#);
    wait_for("the rooms closed", || a.stats().is_none());
}

/// api/config's call and chat settings (fit's `TestPagesConfigDefaults`).
#[test]
fn config_defaults() {
    let read = |a: &Akit| parse(&get(a, "/media/api/config", &[]).text()).unwrap();
    let c = read(&start(&conf("{}")));
    let (call, chat) = (c.get("call").unwrap(), c.get("chat").unwrap());
    assert_eq!((b(call, "mic"), b(call, "cam")), (Some(false), Some(false)));
    assert_eq!(
        (b(chat, "enabled"), n(chat, "maxFileBytes")),
        (Some(true), 5 << 20)
    );
    let c = read(&start(&conf(
        r#"{"mic_default": true, "cam_default": true, "max_file_bytes": 2048}"#,
    )));
    let (call, chat) = (c.get("call").unwrap(), c.get("chat").unwrap());
    assert_eq!((b(call, "mic"), b(call, "cam")), (Some(true), Some(true)));
    assert_eq!(n(chat, "maxFileBytes"), 2048);
    let c = read(&start(&conf(r#"{"chat": false}"#)));
    let chat = c.get("chat").unwrap();
    assert_eq!(
        (b(chat, "enabled"), n(chat, "maxFileBytes")),
        (Some(false), 5 << 20)
    );
}

/// pages: false serves none of the pages, while the login and the room API stay (fit's
/// `TestPagesOff`).
#[test]
fn pages_off() {
    let a = start(&conf(r#"{"auth": "live", "login": true, "pages": false}"#));
    for p in [
        "/media",
        "/media/",
        "/media/call",
        "/media/app.js",
        "/media/app.css",
        "/media/nothing",
    ] {
        assert_eq!(get(&a, p, &[]).status, 404, "{p}");
    }
    assert_eq!(get(&a, "/media/api/config", &[]).status, 200);
    let r = post(
        &a,
        "/media/api/login",
        r#"{"user": "ann", "password": "ann-password"}"#,
        &[],
    );
    assert_eq!(r.status, 200);
    let ann = basic("ann", "ann-password");
    assert_eq!(
        a.join("room1", r#"{"name": "ann"}"#, &[("Authorization", &ann)])
            .0,
        200
    );
}

fn chat_fields<'f>(auth: &'f str, member: &'f str, key: &'f str) -> [(&'f str, &'f str); 3] {
    [
        ("Authorization", auth),
        ("X-Akit-Member", member),
        ("X-Akit-Room-Key", key),
    ]
}

fn say(a: &Akit, room: &str, member: &str, key: &str, text: &str, auth: &str) -> (u16, Value) {
    let r = post(
        a,
        &format!("/media/api/rooms/{room}/chat"),
        &format!(r#"{{"text": {}}}"#, akit_media::pages::quote_json(text)),
        &chat_fields(auth, member, key),
    );
    (r.status, parse(&r.text()).unwrap_or(Value::Null))
}

fn file(
    a: &Akit,
    room: &str,
    (member, key, auth): (&str, &str, &str),
    name: &str,
    ctype: &str,
    data: &[u8],
) -> (u16, Value) {
    let mut fields = chat_fields(auth, member, key).to_vec();
    if !ctype.is_empty() {
        fields.push(("Content-Type", ctype));
    }
    let q: String = name
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    let r = http(
        a.addr,
        "POST",
        &format!("/media/api/rooms/{room}/files?file={q}"),
        &fields,
        data,
    );
    (r.status, parse(&r.text()).unwrap_or(Value::Null))
}

/// A text message reaches the members over the events, and a late watcher's history holds it;
/// a wrong key or name is no such member (fit's `TestChatText` and `TestChatBounds`).
#[test]
fn chat_text() {
    let a = start(&conf(r#"{"auth": "live"}"#));
    let (ann, viv) = (basic("ann", "ann-password"), basic("viv", "viv-password"));
    let ann_key = a.key("room1", "ann", &[("Authorization", &ann)]);
    let viv_key = a.key("room1", "viv", &[("Authorization", &viv)]);
    let wa = follow(&a, "room1", &[("Authorization", &ann)]);
    let first = wa.wait("ann's first event", |_| true);
    assert_eq!(first.name, "history");
    assert!(matches!(first.data.get("messages"), Some(Value::Array(m)) if m.is_empty()));
    let (code, m) = say(&a, "room1", "ann", &ann_key, "hi there", &ann);
    assert_eq!(code, 200, "{m:?}");
    assert_eq!(
        (s(&m, "id").len(), s(&m, "from"), s(&m, "text")),
        (32, "ann".into(), "hi there".into())
    );
    let id = s(&m, "id");
    let got = wa.wait("ann gets the message", |e| {
        e.name == "chat" && s(&e.data, "id") == id
    });
    assert_eq!(s(&got.data, "text"), "hi there");
    let late = follow(
        &a,
        "room1",
        &[("Authorization", &format!("Bearer {TOKEN}"))],
    );
    let h = late.wait("the late watcher's history", |e| e.name == "history");
    assert!(
        matches!(h.data.get("messages"), Some(Value::Array(m)) if m.len() == 1 && s(&m[0], "id") == id)
    );
    assert_eq!(say(&a, "room1", "ann", "wrong", "x", &ann).0, 404);
    assert_eq!(say(&a, "room1", "viv", &ann_key, "x", &viv).0, 404);
    assert_eq!(say(&a, "room1", "ann", &viv_key, "x", &ann).0, 404);
    assert_eq!(say(&a, "room9", "ann", &ann_key, "x", &ann).0, 403);
    // The bounds of a message.
    let fields = chat_fields(&ann, "ann", &ann_key);
    for (body, want) in [
        ("".to_owned(), 400),
        (r#"{"text": ""}"#.to_owned(), 400),
        (r#"{"text": "   "}"#.to_owned(), 400),
        ("not json".to_owned(), 400),
        (r#"{"text": 5}"#.to_owned(), 400),
        (format!(r#"{{"text": "{}"}}"#, "x".repeat(2000)), 413),
    ] {
        let r = post(&a, "/media/api/rooms/room1/chat", &body, &fields);
        assert_eq!(r.status, want, "{body:.60}");
    }
    let bad = http(
        a.addr,
        "POST",
        "/media/api/rooms/room1/chat",
        &fields,
        b"{\"text\": \"\xff\xfe\"}",
    );
    assert_eq!(bad.status, 400);
    let (code, m) = say(&a, "room1", "ann", &ann_key, "你好, media!", &ann);
    assert!(code == 200 && s(&m, "text") == "你好, media!");
}

/// With the chat off, its API is not there and the stream has no history (fit's `TestChatOff`).
#[test]
fn chat_off() {
    let a = start(&conf(r#"{"auth": "live", "chat": false}"#));
    let ann = basic("ann", "ann-password");
    let key = a.key("room1", "ann", &[("Authorization", &ann)]);
    assert_eq!(say(&a, "room1", "ann", &key, "hi", &ann).0, 404);
    assert_eq!(
        file(&a, "room1", ("ann", &key, &ann), "a.bin", "", b"x").0,
        404
    );
    let w = follow(&a, "room1", &[("Authorization", &ann)]);
    let first = w.wait("the first event", |_| true);
    assert_eq!(first.name, "room");
}

/// A file's bytes go in and come back out with its name and type; the name is sanitized; the
/// bounds (fit's `TestChatFile`, `TestChatFileTooLarge` and `TestChatHistoryCap`).
#[test]
fn chat_file() {
    let a = start(&conf(r#"{"auth": "live", "max_file_bytes": 1024}"#));
    let ann = basic("ann", "ann-password");
    let bearer = format!("Bearer {TOKEN}");
    let key = a.key("room1", "ann", &[("Authorization", &ann)]);
    let who = ("ann", key.as_str(), ann.as_str());
    let data = b"hello file\n";
    let (code, m) = file(&a, "room1", who, "a b.png", "image/png", data);
    assert_eq!(code, 200, "{m:?}");
    let f = m.get("file").unwrap();
    assert_eq!(
        (s(f, "name"), n(f, "size"), s(f, "type")),
        ("a b.png".into(), 11, "image/png".into())
    );
    let id = s(&m, "id");
    let r = get(
        &a,
        &format!("/media/api/rooms/room1/files/{id}"),
        &[("Authorization", &ann)],
    );
    assert_eq!((r.status, r.body.as_slice()), (200, &data[..]));
    assert_eq!(r.header("content-type"), Some("image/png"));
    assert!(r.header("content-disposition").unwrap().contains("a b.png"));
    assert_eq!(r.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(r.header("cache-control"), Some("no-store"));
    let (code, m) = file(&a, "room1", who, "../../a\tb.txt", "", b"x");
    assert_eq!(code, 200);
    let f = m.get("file").unwrap();
    assert_eq!(
        (s(f, "name"), s(f, "type")),
        (".._.._a_b.txt".into(), "application/octet-stream".into())
    );
    assert_eq!(file(&a, "room1", who, "", "text/plain", b"x").0, 400);
    assert_eq!(
        file(&a, "room1", who, &"n".repeat(201), "text/plain", b"x").0,
        400
    );
    assert_eq!(
        file(&a, "room1", ("ann", "wrong", &ann), "a.bin", "", b"x").0,
        404
    );
    let zero = "0".repeat(32);
    let auth = [("Authorization", bearer.as_str())];
    assert_eq!(
        get(&a, &format!("/media/api/rooms/room1/files/{zero}"), &auth).status,
        404
    );
    assert_eq!(
        get(&a, &format!("/media/api/rooms/room2/files/{id}"), &auth).status,
        404
    );
    // max_file_bytes.
    assert_eq!(file(&a, "room1", who, "big.bin", "", &[b'x'; 2000]).0, 413);
    assert_eq!(file(&a, "room1", who, "ok.bin", "", &[b'y'; 1024]).0, 200);
    // The history keeps 64 messages, the oldest dropped.
    let mut ids = Vec::new();
    for i in 0..70 {
        let (code, m) = say(&a, "room1", "ann", &key, &format!("m{i}"), &ann);
        assert_eq!(code, 200);
        ids.push(s(&m, "id"));
    }
    let w = follow(&a, "room1", &[("Authorization", &ann)]);
    let h = w.wait("the capped history", |e| e.name == "history");
    let Some(Value::Array(msgs)) = h.data.get("messages") else {
        panic!("{:?}", h.data)
    };
    assert_eq!(msgs.len(), 64);
    assert_eq!(s(&msgs[0], "id"), ids[6]);
    assert_eq!(s(&msgs[63], "id"), ids[69]);
}

/// Adding files past the hub's bound drops the oldest file messages across the rooms, whose
/// downloads then 404, while a room forgotten drops its bytes (fit's `TestChatFileEviction`).
#[test]
fn chat_file_eviction() {
    akit_media::pages::set_chat_file_bytes(2500);
    let a = start(&conf(r#"{"auth": "live", "member_ttl": "3s"}"#));
    let bearer = format!("Bearer {TOKEN}");
    let auth = [("Authorization", bearer.as_str())];
    let k1 = a.key("room1", "ann", &auth);
    let k2 = a.key("room2", "ann", &auth);
    let one = |c: u8| vec![c; 1000];
    let (_, fa) = file(&a, "room1", ("ann", &k1, &bearer), "a.bin", "", &one(b'a'));
    let (code, fb) = file(&a, "room2", ("ann", &k2, &bearer), "b.bin", "", &one(b'b'));
    assert_eq!(code, 200);
    assert_eq!(a.stats().unwrap().file_bytes, 2000);
    let (code, fc) = file(&a, "room1", ("ann", &k1, &bearer), "c.bin", "", &one(b'c'));
    assert_eq!(code, 200);
    assert_eq!(a.stats().unwrap().file_bytes, 2000);
    let url = |room: &str, m: &Value| format!("/media/api/rooms/{room}/files/{}", s(m, "id"));
    assert_eq!(get(&a, &url("room1", &fa), &auth).status, 404);
    assert_eq!(get(&a, &url("room2", &fb), &auth).status, 200);
    assert_eq!(get(&a, &url("room1", &fc), &auth).status, 200);
    let leave = format!(r#"{{"name": "ann", "key": "{k1}"}}"#);
    assert_eq!(
        post(&a, "/media/api/rooms/room1/leave", &leave, &[]).status,
        200
    );
    wait_for("room1 forgotten", || {
        a.stats().is_some_and(|s| s.rooms == 1)
    });
    assert_eq!(a.stats().unwrap().file_bytes, 1000);
    akit_media::pages::set_chat_file_bytes(64 << 20);
}
