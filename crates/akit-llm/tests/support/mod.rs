//! What the LLM roles' end-to-end tests share: a fake OpenAI-compatible upstream, an agent's
//! HTTP calls, and the roles run as `akit run` runs them.
#![allow(
    dead_code,
    unreachable_pub,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

pub fn until(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// What the upstream saw of one call.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Seen {
    pub fn header(&self, k: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(h, _)| h.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    }
}

/// A fake OpenAI-compatible upstream: `GET /v1/models`, and `POST /v1/chat/completions`,
/// which answers JSON with the body's length and model, or, with `"stream":true`, three server
/// events and `[DONE]`, chunked, 50 ms apart.
pub struct Upstream {
    pub port: u16,
    pub seen: Arc<Mutex<Vec<Seen>>>,
}

fn read_request(r: &mut BufReader<TcpStream>) -> Option<Seen> {
    let mut line = String::new();
    if r.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();
    let mut headers = Vec::new();
    loop {
        let mut l = String::new();
        r.read_line(&mut l).ok()?;
        let l = l.trim_end();
        if l.is_empty() {
            break;
        }
        let (k, v) = l.split_once(':')?;
        headers.push((k.trim().to_owned(), v.trim().to_owned()));
    }
    let get = |k: &str| {
        headers
            .iter()
            .find(|(h, _): &&(String, String)| h.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.clone())
    };
    let mut body = Vec::new();
    if let Some(n) = get("content-length") {
        body.resize(n.parse().ok()?, 0);
        r.read_exact(&mut body).ok()?;
    } else if get("transfer-encoding").is_some_and(|v| v.contains("chunked")) {
        loop {
            let mut sz = String::new();
            r.read_line(&mut sz).ok()?;
            let n = usize::from_str_radix(sz.trim(), 16).ok()?;
            let mut c = vec![0u8; n + 2];
            r.read_exact(&mut c).ok()?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&c[..n]);
        }
    }
    Some(Seen {
        method,
        path,
        headers,
        body,
    })
}

fn serve(s: TcpStream, seen: &Arc<Mutex<Vec<Seen>>>) {
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut w = s.try_clone().unwrap();
    let mut r = BufReader::new(s);
    while let Some(req) = read_request(&mut r) {
        seen.lock().unwrap().push(req.clone());
        let text = String::from_utf8_lossy(&req.body).into_owned();
        let model = akit_llm::json::parse(&req.body)
            .map(|v| akit_llm::json::get_str(&v, "model"))
            .unwrap_or_default();
        if req.method == "GET" {
            let b = r#"{"object":"list","data":[]}"#;
            let _ = write!(
                w,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
                b.len()
            );
            continue;
        }
        if text.contains("\"stream\":true") {
            let _ = write!(
                w,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n"
            );
            for i in 0..3 {
                let ev = format!("data: {{\"i\":{i},\"model\":\"{model}\"}}\n\n");
                let _ = write!(w, "{:x}\r\n{ev}\r\n", ev.len());
                let _ = w.flush();
                std::thread::sleep(Duration::from_millis(50));
            }
            let ev = "data: [DONE]\n\n";
            let _ = write!(w, "{:x}\r\n{ev}\r\n0\r\n\r\n", ev.len());
            continue;
        }
        let b = format!(
            r#"{{"id":"x","object":"chat.completion","model":"{model}","bytes":{}}}"#,
            req.body.len()
        );
        let _ = write!(
            w,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        );
    }
}

impl Upstream {
    pub fn start() -> Upstream {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let s2 = Arc::clone(&seen);
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(s) = s else { break };
                let seen = Arc::clone(&s2);
                std::thread::spawn(move || serve(s, &seen));
            }
        });
        Upstream { port, seen }
    }

    pub fn last_post(&self) -> Seen {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|s| s.method == "POST")
            .cloned()
            .unwrap()
    }
}

/// An agent's answer.
#[derive(Debug)]
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// The body's pieces as they came (a chunked body's chunks).
    pub chunks: usize,
}

impl Answer {
    pub fn header(&self, k: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(h, _)| h.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// An agent's HTTP/1.1 call to the proxy's API.
pub fn agent(port: u16, method: &str, path: &str, key: &str, body: &[u8]) -> Answer {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let auth = if key.is_empty() {
        String::new()
    } else {
        format!("Authorization: Bearer {key}\r\n")
    };
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: x\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .unwrap();
    s.write_all(body).unwrap();
    let mut r = BufReader::new(s);
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let status: u16 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    let mut headers = Vec::new();
    loop {
        let mut l = String::new();
        r.read_line(&mut l).unwrap();
        let l = l.trim_end();
        if l.is_empty() {
            break;
        }
        let (k, v) = l.split_once(':').unwrap();
        headers.push((k.trim().to_owned(), v.trim().to_owned()));
    }
    let get = |k: &str| {
        headers
            .iter()
            .find(|(h, _): &&(String, String)| h.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.clone())
    };
    let mut body = Vec::new();
    let mut chunks = 0;
    if let Some(n) = get("content-length") {
        body.resize(n.parse().unwrap(), 0);
        r.read_exact(&mut body).unwrap();
        chunks = 1;
    } else if get("transfer-encoding").is_some_and(|v| v.contains("chunked")) {
        loop {
            let mut sz = String::new();
            r.read_line(&mut sz).unwrap();
            let n = usize::from_str_radix(sz.trim(), 16).unwrap();
            let mut c = vec![0u8; n + 2];
            r.read_exact(&mut c).unwrap();
            if n == 0 {
                break;
            }
            chunks += 1;
            body.extend_from_slice(&c[..n]);
        }
    } else {
        r.read_to_end(&mut body).unwrap();
    }
    Answer {
        status,
        headers,
        body,
        chunks,
    }
}

/// The SHA-256 of a key, in hex, as the config keeps it.
pub fn key_hash(key: &str) -> String {
    akit_roles::linktoken::hex(&akio_crypto::hash::Sha256::digest(key.as_bytes()))
}

/// The roles of `doc` run on one engine, as `akit run` runs them.
pub struct Roles {
    pub engine: Arc<akio::Engine>,
    pub roles: akit_roles::host::Manager,
}

impl Roles {
    pub fn start(doc: &str) -> Roles {
        let config = akit_config::from_json(doc, None).unwrap_or_else(|e| panic!("{doc}\n{e}"));
        let engine = akio::Engine::builder().client_threads(2).build().unwrap();
        engine.start().unwrap();
        let engine = Arc::new(engine);
        let mut roles = akit_roles::host::Manager::new(
            &[
                akit_llm::hub::HUB,
                akit_llm::node::NODE,
                akit_llm::proxy::PROXY,
            ],
            Arc::clone(&engine),
            None,
        );
        let errs = roles.apply(&config);
        assert!(errs.is_empty(), "{errs:?}");
        Roles { engine, roles }
    }
}

impl Drop for Roles {
    fn drop(&mut self) {
        self.roles.stop(Duration::ZERO);
        self.engine.shutdown();
        self.engine.wait();
    }
}
