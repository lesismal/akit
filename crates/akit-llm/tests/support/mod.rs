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

/// The roles of `doc` run on one engine, as `akit run` runs them, with a control thread that
/// applies their edits of their members as `akit run`'s does (refusing them with `fail`'s
/// message while it is set); `doc` is the config they change.
pub struct Roles {
    pub engine: Arc<akio::Engine>,
    pub roles: Arc<Mutex<akit_roles::host::Manager>>,
    pub doc: Arc<Mutex<akit_config::value::Value>>,
    pub fail: Arc<Mutex<Option<String>>>,
}

impl Roles {
    pub fn start(doc: &str) -> Roles {
        let config = akit_config::from_json(doc, None).unwrap_or_else(|e| panic!("{doc}\n{e}"));
        let engine = akio::Engine::builder().client_threads(2).build().unwrap();
        engine.start().unwrap();
        let engine = Arc::new(engine);
        let (tx, rx) = std::sync::mpsc::channel::<akit_roles::host::Edit>();
        let tx = Mutex::new(tx);
        let editor: akit_roles::host::Editor = Arc::new(move |edit| {
            let _ = tx.lock().unwrap().send(edit);
            Ok(())
        });
        let mut roles = akit_roles::host::Manager::new(
            &[
                akit_llm::hub::HUB,
                akit_llm::node::NODE,
                akit_llm::proxy::PROXY,
            ],
            Arc::clone(&engine),
            Some(editor),
        );
        // The roles' consoles, as `akit run` serves them (I109c).
        roles.set_consoles(Box::new(akit_admin::console::RoleConsoles::new(
            &akit_admin::Options::default(),
        )));
        let errs = roles.apply(&config);
        assert!(errs.is_empty(), "{errs:?}");
        let roles = Arc::new(Mutex::new(roles));
        let value = Arc::new(Mutex::new(akit_config::value::parse(doc).unwrap()));
        let fail: Arc<Mutex<Option<String>>> = Arc::default();
        let (control, v2, f2) = (Arc::clone(&roles), Arc::clone(&value), Arc::clone(&fail));
        std::thread::spawn(move || {
            while let Ok(edit) = rx.recv() {
                if let Some(msg) = f2.lock().unwrap().clone() {
                    (edit.done)(Err(msg));
                    continue;
                }
                let mut v = v2.lock().unwrap().clone();
                match edit.apply_to(&mut v) {
                    Ok(done) => match akit_config::from_json(&v.to_json(), None) {
                        Ok(config) => {
                            let errs = control.lock().unwrap().apply(&config);
                            if errs.is_empty() {
                                *v2.lock().unwrap() = v;
                                done(Ok(Vec::new()));
                            } else {
                                done(Err(errs.join("; ")));
                            }
                        }
                        Err(e) => done(Err(e.to_string())),
                    },
                    Err((done, err)) => done(Err(err)),
                }
            }
        });
        Roles {
            engine,
            roles,
            doc: value,
            fail,
        }
    }

    /// Role `id`'s methods, as its own pages call them.
    pub fn local(&self, id: &str) -> akit_llm::local::Local {
        let api = self.roles.lock().unwrap().api(id).unwrap();
        api.downcast_ref::<akit_llm::local::Local>()
            .unwrap()
            .clone()
    }

    /// Member `path` (`llm.node`) of the config as the roles changed it, as JSON.
    pub fn member(&self, path: &str) -> String {
        let v = self.doc.lock().unwrap();
        let mut cur = &*v;
        for p in path.split('.') {
            cur = cur.get(p).unwrap();
        }
        akit_llm::json::write(cur)
    }
}

impl Drop for Roles {
    fn drop(&mut self) {
        self.roles.lock().unwrap().stop(Duration::ZERO);
        self.engine.shutdown();
        self.engine.wait();
    }
}

/// A call to a role's own method, its answer's JSON.
pub fn call(
    l: &akit_llm::local::Local,
    method: &str,
    req: &str,
) -> Result<String, akit_llm::wire::Error> {
    l.call_wait(method, req, Duration::from_secs(30))
        .map(|b| String::from_utf8(b).unwrap())
}

/// The code of an error, 0 for none.
pub fn code<T>(r: &Result<T, akit_llm::wire::Error>) -> i64 {
    match r {
        Err(akit_llm::wire::Error::Remote { code, .. }) => *code,
        Err(_) => -1,
        Ok(_) => 0,
    }
}

/// Reads a stream to its end: its frames, and its result or error.
pub fn read_stream(
    st: &akit_llm::wire::Stream,
) -> (Vec<Vec<u8>>, Result<Vec<u8>, akit_llm::wire::Error>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let s2 = st.clone();
    st.on_data(move |ev| {
        let _ = tx.send(ev);
    });
    let mut frames = Vec::new();
    loop {
        match rx
            .recv_timeout(Duration::from_secs(30))
            .expect("the stream's frames")
        {
            akit_llm::wire::Event::Data(d) => {
                s2.release(d.len());
                frames.push(d);
            }
            akit_llm::wire::Event::End(r) => {
                return (frames, r.map(|()| s2.result().unwrap_or_default()));
            }
        }
    }
}
