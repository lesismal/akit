//! Stand-ins of the model APIs, for tests only (the feature `stand-ins`, and this crate's own
//! tests): never a real API, never a real key ([decision 044](../../../docs/decisions/044-assistant.md), 9).
//!
//! [`Server`] is a plain HTTP/1.1 server on a port of 127.0.0.1 the system chooses, a thread a
//! connection, one request a connection, whose handler writes the answer as a test wants it:
//! whole, streamed a chunk at a time, held, cut. [`fake_api`] is fit's `fakeapi` ported: a fake of
//! every kind of API the assistant calls, which checks each request as the vendor's API would
//! take it (path, key header, the body's shape: system prompt, tools, a tool's result going back)
//! and answers with an answer recorded from that API (fit's `recorded` files, in
//! `testdata/recorded`), streamed a piece at a time. Each kind's conversation is two turns: the
//! first answer calls the tool `get_status` with `{"listener": "web"}`, and once the request
//! carries that call's result, the second answers "The listener web is up."
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    missing_docs
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use akit_config::value::Value;

/// A request the server took.
#[derive(Debug, Clone, Default)]
pub struct Req {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Req {
    /// The header `name`, any case.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The query's parameter `name`.
    #[must_use]
    pub fn param(&self, name: &str) -> Option<&str> {
        self.query
            .split('&')
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v)
    }

    /// The body as JSON.
    #[must_use]
    pub fn json(&self) -> Option<Value> {
        akit_config::value::parse(std::str::from_utf8(&self.body).ok()?).ok()
    }
}

/// A connection's answer, as the handler writes it.
#[derive(Debug)]
pub struct Conn {
    s: TcpStream,
    chunked: bool,
}

impl Conn {
    /// The head of a streamed answer: chunked, the connection closed after it.
    pub fn head(&mut self, status: u16, headers: &[(&str, &str)]) {
        let mut h =
            format!("HTTP/1.1 {status} X\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n");
        for (n, v) in headers {
            h.push_str(&format!("{n}: {v}\r\n"));
        }
        h.push_str("\r\n");
        let _ = self.s.write_all(h.as_bytes());
        self.chunked = true;
    }

    /// A chunk of the answer, sent at once; false once the client has gone.
    pub fn chunk(&mut self, data: &[u8]) -> bool {
        if data.is_empty() {
            return true;
        }
        let mut b = format!("{:x}\r\n", data.len()).into_bytes();
        b.extend_from_slice(data);
        b.extend_from_slice(b"\r\n");
        self.s.write_all(&b).is_ok()
    }

    /// The end of a streamed answer.
    pub fn end(&mut self) {
        if self.chunked {
            let _ = self.s.write_all(b"0\r\n\r\n");
            self.chunked = false;
        }
    }

    /// A whole answer.
    pub fn whole(&mut self, status: u16, content_type: &str, body: &[u8]) {
        let h = format!(
            "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = self.s.write_all(h.as_bytes());
        let _ = self.s.write_all(body);
    }

    /// Bytes as they are.
    pub fn raw(&mut self, b: &[u8]) {
        let _ = self.s.write_all(b);
    }

    /// Waits up to `max` for the client to close the connection; whether it did.
    pub fn wait_gone(&mut self, max: Duration) -> bool {
        let deadline = Instant::now() + max;
        let mut buf = [0u8; 512];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            let _ = self
                .s
                .set_read_timeout(Some(left.min(Duration::from_millis(200))));
            match self.s.read(&mut buf) {
                Ok(0) => return true,
                Ok(_) => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => return true,
            }
        }
    }

    /// Closes the connection at once, whatever was sent: a streamed answer is left without its
    /// end.
    pub fn cut(&mut self) {
        self.chunked = false;
        let _ = self.s.shutdown(std::net::Shutdown::Both);
    }
}

/// Answers a request.
pub type Handler = Arc<dyn Fn(&Req, &mut Conn) + Send + Sync>;

/// A stand-in server; stopped when dropped.
pub struct Server {
    addr: SocketAddr,
    reqs: Arc<Mutex<Vec<Req>>>,
    stop: Arc<AtomicBool>,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server").field("addr", &self.addr).finish()
    }
}

impl Server {
    /// A server of `h`, on a port of 127.0.0.1 the system chooses.
    #[must_use]
    pub fn start(h: impl Fn(&Req, &mut Conn) + Send + Sync + 'static) -> Server {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let reqs = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let h: Handler = Arc::new(h);
        let (r, st) = (Arc::clone(&reqs), Arc::clone(&stop));
        std::thread::spawn(move || {
            for s in l.incoming() {
                if st.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(s) = s else { continue };
                let (h, r) = (Arc::clone(&h), Arc::clone(&r));
                std::thread::spawn(move || serve(s, &h, &r));
            }
        });
        Server { addr, reqs, stop }
    }

    /// `http://127.0.0.1:<port>`.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Its address.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The requests taken so far.
    #[must_use]
    pub fn requests(&self) -> Vec<Req> {
        self.reqs.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wakes the accept loop, which then sees the flag.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_secs(1));
    }
}

fn serve(s: TcpStream, h: &Handler, reqs: &Mutex<Vec<Req>>) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(10)));
    let Ok(rs) = s.try_clone() else { return };
    let mut r = BufReader::new(rs);
    let mut line = String::new();
    if r.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let target = parts.next().unwrap_or("").to_owned();
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_owned(), q.to_owned()),
        None => (target.clone(), String::new()),
    };
    let mut headers = Vec::new();
    loop {
        let mut l = String::new();
        if r.read_line(&mut l).unwrap_or(0) == 0 {
            return;
        }
        let l = l.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((n, v)) = l.split_once(':') {
            headers.push((n.trim().to_owned(), v.trim().to_owned()));
        }
    }
    let mut req = Req {
        method,
        path,
        query,
        headers,
        body: Vec::new(),
    };
    if req
        .header("Transfer-Encoding")
        .is_some_and(|v| v.contains("chunked"))
    {
        loop {
            let mut sz = String::new();
            if r.read_line(&mut sz).unwrap_or(0) == 0 {
                return;
            }
            let Ok(n) = usize::from_str_radix(sz.trim(), 16) else {
                return;
            };
            let mut c = vec![0u8; n.min(8 << 20) + 2];
            if r.read_exact(&mut c).is_err() {
                return;
            }
            if n == 0 {
                break;
            }
            req.body.extend_from_slice(&c[..n]);
        }
    } else {
        let len: usize = req
            .header("Content-Length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let mut body = vec![0u8; len.min(8 << 20)];
        if r.read_exact(&mut body).is_err() {
            return;
        }
        req.body = body;
    }
    reqs.lock().unwrap().push(req.clone());
    let mut c = Conn { s, chunked: false };
    h(&req, &mut c);
    c.end();
}

/// The kinds of API [`fake_api`] serves.
pub const KINDS: [&str; 8] = [
    "anthropic",
    "openai",
    "openai_responses",
    "gemini",
    "azure_openai",
    "ollama",
    "openai_compatible",
    "llm_proxy",
];

/// The base URL a provider of `kind` has on the fake at `url`.
#[must_use]
pub fn base_url(url: &str, kind: &str) -> String {
    match kind {
        "gemini" => format!("{url}/v1beta"),
        "azure_openai" | "ollama" => url.to_owned(),
        _ => format!("{url}/v1"),
    }
}

fn recorded(name: &str) -> &'static str {
    match name {
        "anthropic-1.sse" => include_str!("../testdata/recorded/anthropic-1.sse"),
        "anthropic-2.sse" => include_str!("../testdata/recorded/anthropic-2.sse"),
        "chat-1.sse" => include_str!("../testdata/recorded/chat-1.sse"),
        "chat-2.sse" => include_str!("../testdata/recorded/chat-2.sse"),
        "gemini-1.sse" => include_str!("../testdata/recorded/gemini-1.sse"),
        "gemini-2.sse" => include_str!("../testdata/recorded/gemini-2.sse"),
        "ollama-1.ndjson" => include_str!("../testdata/recorded/ollama-1.ndjson"),
        "ollama-2.ndjson" => include_str!("../testdata/recorded/ollama-2.ndjson"),
        "responses-1.sse" => include_str!("../testdata/recorded/responses-1.sse"),
        "responses-2.sse" => include_str!("../testdata/recorded/responses-2.sse"),
        _ => "",
    }
}

/// The value at `path` in `v`: keys and indexes, as fit's `fakeapi.Get`.
#[must_use]
pub fn get<'v>(v: &'v Value, path: &[&str]) -> Option<&'v Value> {
    let mut v = v;
    for p in path {
        v = match (v, p.parse::<usize>()) {
            (Value::Array(a), Ok(i)) => a.get(i)?,
            (Value::Array(a), Err(_)) if *p == "-1" => a.last()?,
            _ => v.get(p)?,
        };
    }
    Some(v)
}

fn get_str<'v>(v: &'v Value, path: &[&str]) -> &'v str {
    match get(v, path) {
        Some(Value::String(s)) => s,
        _ => "",
    }
}

/// The first element of the list at `path` whose member `key` is the string `value`.
#[must_use]
pub fn find<'v>(v: &'v Value, key: &str, value: &str, path: &[&str]) -> Option<&'v Value> {
    match get(v, path)? {
        Value::Array(a) => a
            .iter()
            .find(|e| matches!(e.get(key), Some(Value::String(s)) if s == value)),
        _ => None,
    }
}

fn fail(c: &mut Conn, status: u16, msg: &str) {
    let mut b = String::from("{\"error\":{\"message\":");
    akit_config::value::write_string(&mut b, msg);
    b.push_str(",\"type\":\"invalid_request_error\"}}");
    c.whole(status, "application/json", b.as_bytes());
}

/// fit's `fakeapi`: every kind of model API, its key `key` in its own header (none for Ollama).
#[must_use]
pub fn fake_api(key: &str) -> Server {
    let key = key.to_owned();
    Server::start(move |r, c| fake(&key, r, c))
}

fn fake(key: &str, r: &Req, c: &mut Conn) {
    if r.method == "GET" {
        return models(key, r, c);
    }
    let p = r.path.as_str();
    let (kind, key_header, prefix) = if p == "/v1/chat/completions" {
        ("chat", "Authorization", "Bearer ")
    } else if p.starts_with("/openai/deployments/") && p.ends_with("/chat/completions") {
        if r.param("api-version").is_none_or(str::is_empty) {
            return fail(c, 400, "api-version is missing");
        }
        ("chat", "Api-Key", "")
    } else if p == "/v1/responses" {
        ("responses", "Authorization", "Bearer ")
    } else if p == "/v1/messages" {
        if r.header("Anthropic-Version").is_none() {
            return fail(c, 400, "anthropic-version is missing");
        }
        ("anthropic", "X-Api-Key", "")
    } else if p.starts_with("/v1beta/models/") && p.ends_with(":streamGenerateContent") {
        if r.param("alt") != Some("sse") {
            return fail(c, 400, "alt=sse is missing");
        }
        ("gemini", "X-Goog-Api-Key", "")
    } else if p == "/api/chat" {
        ("ollama", "", "")
    } else {
        return fail(c, 404, &format!("no such endpoint: {p}"));
    };
    if !key_header.is_empty() && r.header(key_header) != Some(&format!("{prefix}{key}")) {
        // As the APIs do, the key is said back: the client must not pass it on.
        let got = r.header(key_header).unwrap_or("");
        return fail(
            c,
            401,
            &format!(
                "Incorrect API key provided: {}",
                got.strip_prefix(prefix).unwrap_or(got)
            ),
        );
    }
    let Some(body) = r.json() else {
        return fail(c, 400, "the body is not JSON");
    };
    match check(kind, &body) {
        Ok(turn) => replay(c, kind, turn),
        Err(msg) => fail(c, 400, msg),
    }
}

/// Writes the recorded answer of `kind`'s `turn`, an event, or a line, at a time.
fn replay(c: &mut Conn, kind: &str, turn: u8) {
    let (name, ctype, sep) = if kind == "ollama" {
        (
            format!("ollama-{turn}.ndjson"),
            "application/x-ndjson",
            "\n",
        )
    } else {
        (format!("{kind}-{turn}.sse"), "text/event-stream", "\n\n")
    };
    c.head(200, &[("Content-Type", ctype)]);
    for piece in recorded(&name).split_inclusive(sep) {
        if !c.chunk(piece.as_bytes()) {
            return;
        }
    }
}

fn models(key: &str, r: &Req, c: &mut Conn) {
    match r.path.as_str() {
        "/api/tags" => c.whole(
            200,
            "application/json",
            br#"{"models":[{"name":"qwen-test","model":"qwen-test","size":1}]}"#,
        ),
        "/v1beta/models" => {
            if r.header("X-Goog-Api-Key") != Some(key) {
                return fail(c, 403, "API key not valid. Please pass a valid API key.");
            }
            c.whole(
                200,
                "application/json",
                br#"{"models":[{"name":"models/gemini-test","inputTokenLimit":1048576,"supportedGenerationMethods":["generateContent","countTokens"]},{"name":"models/embedding-test","supportedGenerationMethods":["embedContent"]}]}"#,
            );
        }
        "/v1/models" | "/openai/models" => {
            let auth = r.header("Authorization") == Some(&format!("Bearer {key}"))
                || r.header("X-Api-Key") == Some(key)
                || r.header("Api-Key") == Some(key);
            if !auth {
                return fail(c, 401, "Incorrect API key provided");
            }
            c.whole(
                200,
                "application/json",
                br#"{"object":"list","data":[{"id":"gpt-test","object":"model"},{"id":"claude-test","type":"model"}]}"#,
            );
        }
        p => fail(c, 404, &format!("no such endpoint: {p}")),
    }
}

/// Which turn `body` is, or what is wrong with it as `kind`'s API would take it.
fn check(kind: &str, body: &Value) -> Result<u8, &'static str> {
    if body.get("tools").is_none() {
        // A call without tools, as a provider's test makes, is answered with the second turn's
        // text.
        return Ok(2);
    }
    let is = |path: &[&str], want: &str| get_str(body, path) == want;
    match kind {
        "chat" => {
            if !is(&["messages", "0", "role"], "system")
                || get(body, &["stream"]) != Some(&Value::Bool(true))
                || !is(&["tools", "0", "function", "name"], "get_status")
            {
                return Err(
                    "a chat call has its system message first, streams, and lists get_status",
                );
            }
            let Some(tool) = find(body, "role", "tool", &["messages"]) else {
                return Ok(1);
            };
            let asst = find(body, "role", "assistant", &["messages"]);
            let args = asst.map_or("", |a| {
                get_str(a, &["tool_calls", "0", "function", "arguments"])
            });
            if get_str(tool, &["tool_call_id"]) != "call_abc"
                || asst.map_or("", |a| get_str(a, &["tool_calls", "0", "id"])) != "call_abc"
                || args != r#"{"listener":"web"}"#
            {
                return Err("the tool's result does not answer the call");
            }
        }
        "responses" => {
            if get(body, &["instructions"]).is_none()
                || get(body, &["store"]) != Some(&Value::Bool(false))
                || !is(&["tools", "0", "name"], "get_status")
            {
                return Err("a responses call has instructions, store false, and lists get_status");
            }
            let Some(out) = find(body, "type", "function_call_output", &["input"]) else {
                return Ok(1);
            };
            let call = find(body, "type", "function_call", &["input"]);
            if get_str(out, &["call_id"]) != "call_r1"
                || call.map_or("", |c| get_str(c, &["call_id"])) != "call_r1"
            {
                return Err("the tool's result does not answer the call");
            }
        }
        "anthropic" => {
            if get(body, &["system"]).is_none()
                || get(body, &["max_tokens"]).is_none()
                || get(body, &["tools", "0", "input_schema"]).is_none()
            {
                return Err("a messages call has system, max_tokens and the tools' input_schema");
            }
            let last = get(body, &["messages", "-1"]);
            let Some(res) = last.and_then(|l| find(l, "type", "tool_result", &["content"])) else {
                return Ok(1);
            };
            let asst = find(body, "role", "assistant", &["messages"]);
            let tool_use = asst.and_then(|a| find(a, "type", "tool_use", &["content"]));
            let thinking = asst.and_then(|a| find(a, "type", "thinking", &["content"]));
            if get_str(res, &["tool_use_id"]) != "toolu_01"
                || tool_use.map_or("", |t| get_str(t, &["id"])) != "toolu_01"
                || thinking.map_or("", |t| get_str(t, &["signature"])) != "EqQBCgIYAhIM"
            {
                return Err(
                    "the tool's result does not answer the call, or the thinking block is not sent back",
                );
            }
        }
        "gemini" => {
            if get(body, &["systemInstruction", "parts", "0", "text"]).is_none()
                || !is(
                    &["tools", "0", "functionDeclarations", "0", "name"],
                    "get_status",
                )
            {
                return Err("a generateContent call has systemInstruction and declares get_status");
            }
            let (mut resp, mut call) = (None, None);
            if let Some(Value::Array(contents)) = get(body, &["contents"]) {
                for c in contents {
                    if let Some(Value::Array(parts)) = c.get("parts") {
                        for p in parts {
                            if let Some(v) = p.get("functionResponse") {
                                resp = Some(v);
                            }
                            if p.get("functionCall").is_some() {
                                call = Some(p);
                            }
                        }
                    }
                }
            }
            let Some(resp) = resp else { return Ok(1) };
            if get_str(resp, &["name"]) != "get_status"
                || call.map_or("", |c| get_str(c, &["thoughtSignature"])) != "CiQBVKhc7"
                || resp.get("response").is_none()
            {
                return Err("the function's response does not answer the call");
            }
        }
        "ollama" => {
            if !is(&["messages", "0", "role"], "system")
                || !is(&["tools", "0", "function", "name"], "get_status")
            {
                return Err("a chat call has its system message first and lists get_status");
            }
            let Some(res) = find(body, "role", "tool", &["messages"]) else {
                return Ok(1);
            };
            let asst = find(body, "role", "assistant", &["messages"]);
            if get_str(res, &["tool_name"]) != "get_status"
                || asst.map_or("", |a| {
                    get_str(a, &["tool_calls", "0", "function", "arguments", "listener"])
                }) != "web"
            {
                return Err("the tool's result does not answer the call");
            }
        }
        _ => {}
    }
    Ok(2)
}

/// A turn of a scripted model: a text answer, or tool calls (name, arguments JSON), each answer
/// as Chat Completions streams it.
#[derive(Debug, Clone)]
pub enum Turn {
    /// An answer of text, streamed in pieces of at most 16 bytes.
    Text(String),
    /// Tool calls, by name and arguments.
    Tools(Vec<(String, String)>),
    /// An answer of `n` bytes of text, in pieces of 64 KiB, for the bounds and backpressure.
    Big(usize),
}

/// A model speaking Chat Completions (an `openai_compatible` provider at its `/v1`) that answers
/// each call with the next of `turns`, the last again once they are spent; its requests are
/// kept, for tests to look for secrets in.
#[must_use]
pub fn scripted(turns: Vec<Turn>) -> Server {
    let next = Arc::new(Mutex::new(0usize));
    Server::start(move |r, c| {
        if r.method == "GET" {
            return c.whole(
                200,
                "application/json",
                br#"{"object":"list","data":[{"id":"m"}]}"#,
            );
        }
        let i = {
            let mut n = next.lock().unwrap();
            let i = (*n).min(turns.len().saturating_sub(1));
            *n += 1;
            i
        };
        let Some(turn) = turns.get(i) else {
            return fail(c, 500, "no turn scripted");
        };
        c.head(200, &[("Content-Type", "text/event-stream")]);
        let usage = r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#;
        match turn {
            Turn::Text(t) => {
                let chars: Vec<char> = t.chars().collect();
                for piece in chars.chunks(16) {
                    let s: String = piece.iter().collect();
                    let mut b = String::from("{\"choices\":[{\"delta\":{\"content\":");
                    akit_config::value::write_string(&mut b, &s);
                    b.push_str("}}]}");
                    if !c.chunk(format!("data: {b}\n\n").as_bytes()) {
                        return;
                    }
                }
            }
            Turn::Tools(calls) => {
                for (k, (name, args)) in calls.iter().enumerate() {
                    let mut a = String::new();
                    akit_config::value::write_string(&mut a, args);
                    let ev = format!(
                        "{{\"choices\":[{{\"delta\":{{\"tool_calls\":[{{\"index\":{k},\"id\":\"call_{i}_{k}\",\"function\":{{\"name\":\"{name}\",\"arguments\":{a}}}}}]}}}}]}}"
                    );
                    c.chunk(format!("data: {ev}\n\n").as_bytes());
                }
            }
            Turn::Big(n) => {
                let piece = format!(
                    "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{}\"}}}}]}}\n\n",
                    "z".repeat(64 << 10)
                );
                for _ in 0..n.div_ceil(64 << 10) {
                    if !c.chunk(piece.as_bytes()) {
                        return;
                    }
                }
            }
        }
        c.chunk(
            format!(
                "data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\ndata: {usage}\n\ndata: [DONE]\n\n"
            )
            .as_bytes(),
        );
    })
}
