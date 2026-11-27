//! Fake stores for the shared storage's tests (I62), as fit's `sharedstore/fake_test.go`: a
//! Redis server answering RESP2 (in the clear or over TLS) from a function of the command, and an
//! HTTP server answering from a function of the request; and, on them, stores with the semantics
//! the shared storage relies on (Redis' `SET NX PX` and the lock scripts, etcd's v3 gateway with
//! leases and transactions, Consul's KV store with sessions), for two instances to share.
//!
//! Used by akit-tls's unit and integration tests and akit-listener's, through `#[path]`.
#![allow(
    dead_code,
    unreachable_pub,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::type_complexity
)]

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use akio::tls::{ServerConfig, Tls};
use akio::{Acceptor, Conn, Engine, Handler, Listener};
use akio_crypto::base64::STANDARD;
use akit_config::value::{Value, parse};

/// An address nothing listens on: port 1 (tcpmux), which nothing serves and no test binds. A
/// port picked by binding port 0 and letting it go may be taken by another process meanwhile.
pub fn closed_addr() -> String {
    "127.0.0.1:1".to_owned()
}

/// A temporary directory of its own, removed when dropped.
pub struct TempDir(pub std::path::PathBuf);

impl TempDir {
    pub fn new(name: &str) -> TempDir {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!(
            "akit-{name}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        TempDir(d)
    }

    /// Writes `data` to `name` in it, and returns its path.
    pub fn file(&self, name: &str, data: &str) -> String {
        let p = self.0.join(name);
        std::fs::write(&p, data).unwrap();
        p.display().to_string()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------------------------------------
// Redis

/// What a fake Redis answers a command (its arguments) with, raw; empty cuts the connection.
pub type RedisHandle = Arc<dyn Fn(&[String]) -> String + Send + Sync>;

/// A bulk string.
pub fn bulk(s: &str) -> String {
    format!("${}\r\n{s}\r\n", s.len())
}

/// An array of replies.
pub fn array(items: &[String]) -> String {
    format!("*{}\r\n{}", items.len(), items.concat())
}

/// One command at the start of `buf`: its arguments and the bytes it took.
fn command(buf: &[u8]) -> Option<(Vec<String>, usize)> {
    let line_end = |from: usize| -> Option<usize> {
        buf.get(from..)?
            .windows(2)
            .position(|w| w == b"\r\n")
            .map(|p| from + p)
    };
    let e = line_end(0)?;
    let n: usize = std::str::from_utf8(buf.get(1..e)?).ok()?.parse().ok()?;
    let mut at = e + 2;
    let mut args = Vec::with_capacity(n);
    for _ in 0..n {
        let e = line_end(at)?;
        let len: usize = std::str::from_utf8(buf.get(at + 1..e)?)
            .ok()?
            .parse()
            .ok()?;
        let start = e + 2;
        let data = buf.get(start..start + len)?;
        buf.get(start + len..start + len + 2)?;
        args.push(String::from_utf8_lossy(data).into_owned());
        at = start + len + 2;
    }
    Some((args, at))
}

struct RedisConnHandler {
    buf: Vec<u8>,
    handle: RedisHandle,
}

impl Handler for RedisConnHandler {
    fn on_data(&mut self, c: &Conn<Self>, data: &[u8]) {
        self.buf.extend_from_slice(data);
        while let Some((args, took)) = command(&self.buf) {
            self.buf.drain(..took);
            let reply = (self.handle)(&args);
            if reply.is_empty() {
                c.close();
                return;
            }
            let _ = c.send(reply.as_bytes());
        }
    }
}

struct RedisAcceptor(RedisHandle);

impl Acceptor for RedisAcceptor {
    type Handler = RedisConnHandler;
    fn on_open(&self, _c: &Conn<RedisConnHandler>) -> RedisConnHandler {
        RedisConnHandler {
            buf: Vec::new(),
            handle: Arc::clone(&self.0),
        }
    }
}

/// A fake Redis on 127.0.0.1, on an akio engine of its own, stopped when dropped.
pub struct FakeRedis {
    pub addr: SocketAddr,
    engine: Engine,
}

impl FakeRedis {
    /// Serves RESP2, over TLS with `tls` when set, answering each command with `handle`.
    pub fn start(tls: Option<ServerConfig>, handle: RedisHandle) -> FakeRedis {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let l = match tls {
            Some(cfg) => Listener::tcp(addr)
                .threads(1)
                .serve(Tls::server(cfg, RedisAcceptor(handle))),
            None => Listener::tcp(addr).threads(1).serve(RedisAcceptor(handle)),
        };
        let mut cfg = akio::Config::default();
        cfg.shutdown_timeout = Duration::from_secs(1);
        let engine = Engine::builder().config(cfg).listen(l).build().unwrap();
        engine.start().unwrap();
        let addr = engine.local_addrs()[0];
        FakeRedis { addr, engine }
    }

    /// A Redis with the semantics the store relies on: keys with expiries, `SET NX PX`, `DEL`,
    /// `SCAN MATCH` of a prefix, and the lock scripts.
    pub fn semantic() -> FakeRedis {
        FakeRedis::start(None, redis_semantics())
    }
}

impl Drop for FakeRedis {
    fn drop(&mut self) {
        self.engine.shutdown();
        self.engine.wait();
    }
}

/// Whether `key` matches a `SCAN` pattern of an escaped prefix and `*`.
fn glob_prefix(pattern: &str, key: &str) -> bool {
    let mut prefix = String::new();
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => prefix.extend(chars.next()),
            '*' if chars.as_str().is_empty() => return key.starts_with(&prefix),
            c => prefix.push(c),
        }
    }
    key == prefix
}

/// The handler of [`FakeRedis::semantic`].
pub fn redis_semantics() -> RedisHandle {
    let kv: Arc<Mutex<HashMap<String, (String, Option<Instant>)>>> = Arc::default();
    Arc::new(move |args: &[String]| {
        let mut kv = kv.lock().unwrap();
        let now = Instant::now();
        kv.retain(|_, (_, exp)| exp.is_none_or(|e| e > now));
        let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
        match arg(0).to_ascii_uppercase().as_str() {
            "AUTH" | "SELECT" => "+OK\r\n".into(),
            "PING" => "+PONG\r\n".into(),
            "GET" => match kv.get(&arg(1)) {
                Some((v, _)) => bulk(v),
                None => "$-1\r\n".into(),
            },
            "SET" => {
                let (k, v) = (arg(1), arg(2));
                let rest: Vec<String> = args
                    .iter()
                    .skip(3)
                    .map(|a| a.to_ascii_uppercase())
                    .collect();
                let nx = rest.iter().any(|a| a == "NX");
                let px = rest
                    .iter()
                    .position(|a| a == "PX")
                    .and_then(|i| args.get(i + 4))
                    .and_then(|ms| ms.parse::<u64>().ok());
                if nx && kv.contains_key(&k) {
                    return "$-1\r\n".into();
                }
                kv.insert(k, (v, px.map(|ms| now + Duration::from_millis(ms))));
                "+OK\r\n".into()
            }
            "DEL" => format!(":{}\r\n", i32::from(kv.remove(&arg(1)).is_some())),
            "SCAN" => {
                let pattern = args
                    .iter()
                    .position(|a| a.eq_ignore_ascii_case("MATCH"))
                    .map_or("*".to_owned(), |i| arg(i + 1));
                let keys: Vec<String> = kv
                    .keys()
                    .filter(|k| glob_prefix(&pattern, k))
                    .map(|k| bulk(k))
                    .collect();
                array(&[bulk("0"), array(&keys)])
            }
            "EVAL" => {
                let (script, key, token) = (arg(1), arg(3), arg(4));
                let held = kv.get(&key).is_some_and(|(v, _)| *v == token);
                if !held {
                    return ":0\r\n".into();
                }
                if script.contains("pexpire") {
                    let ms: u64 = arg(5).parse().unwrap_or(0);
                    if let Some(e) = kv.get_mut(&key) {
                        e.1 = Some(now + Duration::from_millis(ms));
                    }
                } else {
                    kv.remove(&key);
                }
                ":1\r\n".into()
            }
            _ => "-ERR unknown command\r\n".into(),
        }
    })
}

// ---------------------------------------------------------------------------------------------
// HTTP

/// A request as the fake HTTP server got it.
#[derive(Debug, Clone)]
pub struct HttpReq {
    pub method: String,
    /// The path, percent-decoded.
    pub path: String,
    /// The query, as sent.
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpReq {
    pub fn header(&self, name: &str) -> &str {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map_or("", |(_, v)| v.as_str())
    }

    /// A query parameter, as sent.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.query.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (k == name).then_some(v)
        })
    }

    pub fn json(&self) -> Value {
        parse(std::str::from_utf8(&self.body).unwrap_or("{}")).unwrap_or(Value::Null)
    }
}

/// An answer of the fake HTTP server.
pub enum HttpReply {
    /// A status and a body.
    Full(u16, String),
    /// `HTTP/1.1 200 OK` with a body shorter than its length says, then the connection cut.
    Cut,
}

pub type HttpHandle = Arc<dyn Fn(&HttpReq) -> HttpReply + Send + Sync>;

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A fake HTTP/1.1 server on 127.0.0.1, a thread per connection, stopped when dropped.
pub struct FakeHttp {
    pub addr: SocketAddr,
    stop: Arc<AtomicBool>,
}

impl FakeHttp {
    pub fn start(handle: HttpHandle) -> FakeHttp {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&stop);
        std::thread::spawn(move || {
            for c in l.incoming() {
                if s.load(Ordering::Acquire) {
                    return;
                }
                let Ok(c) = c else { continue };
                let (h, s) = (Arc::clone(&handle), Arc::clone(&s));
                std::thread::spawn(move || serve_http(c, &h, &s));
            }
        });
        FakeHttp { addr, stop }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for FakeHttp {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.addr);
    }
}

fn serve_http(mut c: TcpStream, handle: &HttpHandle, stop: &AtomicBool) {
    c.set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let mut buf = Vec::new();
    loop {
        if stop.load(Ordering::Acquire) {
            return;
        }
        // A whole request in buf?
        if let Some(head_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
            let mut lines = head.split("\r\n");
            let first = lines.next().unwrap_or("");
            let mut parts = first.split(' ');
            let method = parts.next().unwrap_or("").to_owned();
            let target = parts.next().unwrap_or("").to_owned();
            let headers: Vec<(String, String)> = lines
                .filter_map(|l| l.split_once(':'))
                .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
                .collect();
            let len: usize = headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, v)| v.parse().ok())
                .unwrap_or(0);
            if buf.len() >= head_end + 4 + len {
                let body = buf[head_end + 4..head_end + 4 + len].to_vec();
                buf.drain(..head_end + 4 + len);
                let (path, query) = target.split_once('?').unwrap_or((&target, ""));
                let req = HttpReq {
                    method,
                    path: percent_decode(path),
                    query: query.to_owned(),
                    headers,
                    body,
                };
                match handle(&req) {
                    HttpReply::Full(status, body) => {
                        let out = format!(
                            "HTTP/1.1 {status} X\r\nContent-Length: {}\r\n\r\n{body}",
                            body.len()
                        );
                        if c.write_all(out.as_bytes()).is_err() {
                            return;
                        }
                    }
                    HttpReply::Cut => {
                        let _ = c.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n");
                        return;
                    }
                }
                continue;
            }
        }
        let mut b = [0u8; 8192];
        match c.read(&mut b) {
            Ok(0) => return,
            Ok(n) => buf.extend_from_slice(&b[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => return,
        }
    }
}

/// Answers each path with what is set for it, 404 otherwise, and records what it was asked,
/// as `METHOD target authorization-or-token` (fit's `replies`).
#[derive(Default)]
pub struct Replies {
    m: Mutex<HashMap<String, HttpHandle>>,
    pub seen: Mutex<Vec<String>>,
}

impl Replies {
    pub fn set(&self, path: &str, status: u16, body: &str) {
        let body = body.to_owned();
        self.set_fn(path, move |_| HttpReply::Full(status, body.clone()));
    }

    pub fn set_fn(&self, path: &str, f: impl Fn(&HttpReq) -> HttpReply + Send + Sync + 'static) {
        self.m.lock().unwrap().insert(path.to_owned(), Arc::new(f));
    }

    pub fn last(&self) -> String {
        self.seen
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap_or_default()
    }

    /// A server answering from it.
    pub fn serve(self: &Arc<Replies>) -> FakeHttp {
        let r = Arc::clone(self);
        FakeHttp::start(Arc::new(move |req: &HttpReq| {
            let target = if req.query.is_empty() {
                req.path.clone()
            } else {
                format!("{}?{}", req.path, req.query)
            };
            r.seen.lock().unwrap().push(format!(
                "{} {target} {}{}",
                req.method,
                req.header("Authorization"),
                req.header("X-Consul-Token")
            ));
            let f = r.m.lock().unwrap().get(&req.path).cloned();
            match f {
                Some(f) => f(req),
                None => HttpReply::Full(404, "404 page not found\n".into()),
            }
        }))
    }
}

// ---------------------------------------------------------------------------------------------
// etcd

#[derive(Default)]
struct Etcd {
    kv: BTreeMap<Vec<u8>, (Vec<u8>, i64)>,
    leases: HashMap<i64, (Duration, Instant)>,
    next: i64,
}

impl Etcd {
    fn expire(&mut self) {
        let now = Instant::now();
        let gone: Vec<i64> = self
            .leases
            .iter()
            .filter(|(_, (_, e))| *e <= now)
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            self.leases.remove(&id);
            self.kv.retain(|_, (_, l)| *l != id);
        }
    }
}

fn b64(v: Option<&Value>) -> Vec<u8> {
    match v {
        Some(Value::String(s)) => STANDARD.decode(s).unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn num(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::String(s) | Value::Number(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

/// A fake etcd v3 JSON gateway with the semantics the store relies on: ranges, puts, deletes,
/// leases that expire and take their keys with them, and create-revision transactions.
pub fn etcd_semantics() -> HttpHandle {
    let st: Arc<Mutex<Etcd>> = Arc::default();
    Arc::new(move |req: &HttpReq| {
        let mut st = st.lock().unwrap();
        st.expire();
        let v = req.json();
        let enc = |b: &[u8]| STANDARD.encode(b);
        match req.path.as_str() {
            "/v3/kv/range" => {
                let key = b64(v.get("key"));
                let end = v.get("range_end").map(|e| b64(Some(e)));
                let kvs: Vec<String> = st
                    .kv
                    .iter()
                    .filter(|(k, _)| match &end {
                        None => **k == key,
                        Some(e) if e.as_slice() == [0] => **k >= key,
                        Some(e) => **k >= key && *k < e,
                    })
                    .map(|(k, (val, _))| {
                        format!("{{\"key\":\"{}\",\"value\":\"{}\"}}", enc(k), enc(val))
                    })
                    .collect();
                if kvs.is_empty() {
                    return HttpReply::Full(200, "{}".into());
                }
                HttpReply::Full(200, format!("{{\"kvs\":[{}]}}", kvs.join(",")))
            }
            "/v3/kv/put" => {
                let lease = num(v.get("lease"));
                st.kv
                    .insert(b64(v.get("key")), (b64(v.get("value")), lease));
                HttpReply::Full(200, "{}".into())
            }
            "/v3/kv/deleterange" => {
                st.kv.remove(&b64(v.get("key")));
                HttpReply::Full(200, "{}".into())
            }
            "/v3/lease/grant" => {
                let ttl = Duration::from_secs(u64::try_from(num(v.get("TTL"))).unwrap_or(1).max(1));
                st.next += 1;
                let id = st.next;
                st.leases.insert(id, (ttl, Instant::now() + ttl));
                HttpReply::Full(
                    200,
                    format!("{{\"ID\":\"{id}\",\"TTL\":\"{}\"}}", ttl.as_secs()),
                )
            }
            "/v3/lease/keepalive" => {
                let id = num(v.get("ID"));
                match st.leases.get_mut(&id) {
                    Some((ttl, exp)) => {
                        *exp = Instant::now() + *ttl;
                        HttpReply::Full(
                            200,
                            format!(
                                "{{\"result\":{{\"ID\":\"{id}\",\"TTL\":\"{}\"}}}}\n",
                                ttl.as_secs()
                            ),
                        )
                    }
                    None => HttpReply::Full(200, format!("{{\"result\":{{\"ID\":\"{id}\"}}}}\n")),
                }
            }
            "/v3/lease/revoke" => {
                let id = num(v.get("ID"));
                if st.leases.remove(&id).is_none() {
                    return HttpReply::Full(
                        404,
                        "{\"error\":\"etcdserver: requested lease not found\"}".into(),
                    );
                }
                st.kv.retain(|_, (_, l)| *l != id);
                HttpReply::Full(200, "{}".into())
            }
            "/v3/kv/txn" => {
                let Some(Value::Array(cmp)) = v.get("compare") else {
                    return HttpReply::Full(400, "no compare".into());
                };
                let key = b64(cmp.first().and_then(|c| c.get("key")));
                if st.kv.contains_key(&key) {
                    return HttpReply::Full(200, "{\"succeeded\":false}".into());
                }
                if let Some(Value::Array(ops)) = v.get("success") {
                    for op in ops {
                        if let Some(p) = op.get("request_put") {
                            let lease = num(p.get("lease"));
                            st.kv
                                .insert(b64(p.get("key")), (b64(p.get("value")), lease));
                        }
                    }
                }
                HttpReply::Full(200, "{\"succeeded\":true}".into())
            }
            _ => HttpReply::Full(404, "404 page not found\n".into()),
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Consul

#[derive(Default)]
struct Consul {
    kv: BTreeMap<String, (Vec<u8>, Option<String>)>,
    sessions: HashMap<String, (Duration, Instant)>,
    next: u64,
}

impl Consul {
    fn expire(&mut self) {
        let now = Instant::now();
        let gone: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, (_, e))| *e <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in gone {
            self.destroy(&id);
        }
    }

    fn destroy(&mut self, id: &str) {
        self.sessions.remove(id);
        // Behavior "delete": the keys the session holds go with it.
        self.kv.retain(|_, (_, s)| s.as_deref() != Some(id));
    }
}

/// A fake Consul HTTP API with the semantics the store relies on: the KV store, its listing, and
/// sessions that expire and delete the keys they hold.
pub fn consul_semantics() -> HttpHandle {
    let st: Arc<Mutex<Consul>> = Arc::default();
    Arc::new(move |req: &HttpReq| {
        let mut st = st.lock().unwrap();
        st.expire();
        let ok = || HttpReply::Full(200, "true".into());
        if let Some(key) = req.path.strip_prefix("/v1/kv/") {
            let key = key.to_owned();
            return match req.method.as_str() {
                "GET" if req.param("keys").is_some() => {
                    let keys: Vec<String> = st
                        .kv
                        .keys()
                        .filter(|k| k.starts_with(&key))
                        .map(|k| format!("\"{k}\""))
                        .collect();
                    if keys.is_empty() {
                        return HttpReply::Full(404, String::new());
                    }
                    HttpReply::Full(200, format!("[{}]", keys.join(",")))
                }
                "GET" => match st.kv.get(&key) {
                    Some((v, _)) => HttpReply::Full(200, String::from_utf8_lossy(v).into_owned()),
                    None => HttpReply::Full(404, String::new()),
                },
                "PUT" => {
                    if let Some(s) = req.param("acquire") {
                        let s = s.to_owned();
                        if !st.sessions.contains_key(&s) {
                            return HttpReply::Full(500, "invalid session".into());
                        }
                        if st
                            .kv
                            .get(&key)
                            .is_some_and(|(_, h)| h.as_deref().is_some_and(|h| h != s))
                        {
                            return HttpReply::Full(200, "false".into());
                        }
                        st.kv.insert(key, (req.body.clone(), Some(s)));
                        return ok();
                    }
                    let held = st.kv.get(&key).and_then(|(_, h)| h.clone());
                    st.kv.insert(key, (req.body.clone(), held));
                    ok()
                }
                "DELETE" => {
                    st.kv.remove(&key);
                    ok()
                }
                _ => HttpReply::Full(405, String::new()),
            };
        }
        if req.path == "/v1/session/create" {
            let v = req.json();
            let ttl = match v.get("TTL") {
                Some(Value::String(s)) => s
                    .trim_end_matches('s')
                    .parse::<u64>()
                    .map_or(Duration::from_secs(10), Duration::from_secs),
                _ => Duration::from_secs(10),
            };
            st.next += 1;
            let id = format!("s-{}", st.next);
            st.sessions.insert(id.clone(), (ttl, Instant::now() + ttl));
            return HttpReply::Full(200, format!("{{\"ID\":\"{id}\"}}"));
        }
        if let Some(id) = req.path.strip_prefix("/v1/session/renew/") {
            return match st.sessions.get_mut(id) {
                Some((ttl, exp)) => {
                    *exp = Instant::now() + *ttl;
                    HttpReply::Full(200, format!("[{{\"ID\":\"{id}\"}}]"))
                }
                None => HttpReply::Full(404, String::new()),
            };
        }
        if let Some(id) = req.path.strip_prefix("/v1/session/destroy/") {
            let id = id.to_owned();
            st.destroy(&id);
            return ok();
        }
        HttpReply::Full(404, "404 page not found\n".into())
    })
}
