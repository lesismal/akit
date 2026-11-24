//! A fake HTTP/1.1 server for the providers' tests, as fit's use `httptest`: each connection on
//! a thread of its own, its requests handled one after another by a handler that may block (a
//! blocking query) or stream its answer (a watch).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::watch::{Log, Timings};

/// A request as the fake read it.
#[derive(Debug, Clone)]
pub(crate) struct Req {
    pub(crate) method: String,
    /// The path, unescaped as written.
    pub(crate) path: String,
    /// The query, as written, without the `?`.
    pub(crate) query: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
}

impl Req {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The value of the query's `key`, percent-decoded.
    pub(crate) fn param(&self, key: &str) -> Option<String> {
        self.query.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (k == key).then(|| unescape(v))
        })
    }

    /// The path and query, as Go's `URL.RequestURI`.
    pub(crate) fn uri(&self) -> String {
        if self.query.is_empty() {
            self.path.clone()
        } else {
            format!("{}?{}", self.path, self.query)
        }
    }
}

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let h = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                out.push(u8::from_str_radix(h, 16).unwrap_or(b'?'));
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Writes an answer.
pub(crate) struct Resp {
    stream: TcpStream,
    headers: Vec<(String, String)>,
    streaming: bool,
}

impl Resp {
    pub(crate) fn header(&mut self, k: &str, v: &str) {
        self.headers.push((k.to_owned(), v.to_owned()));
    }

    fn head(&mut self, status: u16, extra: &str) -> io::Result<()> {
        let reason = crate::api::status_line(status, "");
        let mut h = format!("HTTP/1.1 {reason}\r\n");
        for (k, v) in &self.headers {
            h.push_str(&format!("{k}: {v}\r\n"));
        }
        h.push_str(extra);
        h.push_str("\r\n");
        self.stream.write_all(h.as_bytes())
    }

    /// The whole answer.
    pub(crate) fn send(mut self, status: u16, body: &[u8]) {
        let extra = format!("Content-Length: {}\r\n", body.len());
        if self.head(status, &extra).is_ok() {
            let _ = self.stream.write_all(body);
        }
    }

    /// Starts a chunked answer; [`chunk`](Self::chunk) writes its pieces, dropping it ends it.
    pub(crate) fn stream(&mut self, status: u16) -> io::Result<()> {
        self.streaming = true;
        self.head(status, "Transfer-Encoding: chunked\r\n")
    }

    pub(crate) fn chunk(&mut self, data: &[u8]) -> io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        self.stream
            .write_all(format!("{:x}\r\n", data.len()).as_bytes())?;
        self.stream.write_all(data)?;
        self.stream.write_all(b"\r\n")?;
        self.stream.flush()
    }
}

impl Drop for Resp {
    fn drop(&mut self) {
        if self.streaming {
            let _ = self.stream.write_all(b"0\r\n\r\n");
        }
    }
}

type Handler = dyn Fn(&Req, Resp) + Send + Sync;

/// A fake server, closed when dropped.
pub(crate) struct Server {
    pub(crate) url: String,
    closed: Arc<AtomicBool>,
    conns: Arc<Mutex<Vec<TcpStream>>>,
    addr: std::net::SocketAddr,
}

impl Server {
    pub(crate) fn start(h: impl Fn(&Req, Resp) + Send + Sync + 'static) -> Server {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let closed = Arc::new(AtomicBool::new(false));
        let conns: Arc<Mutex<Vec<TcpStream>>> = Arc::new(Mutex::new(Vec::new()));
        let h: Arc<Handler> = Arc::new(h);
        {
            let (closed, conns) = (Arc::clone(&closed), Arc::clone(&conns));
            std::thread::spawn(move || {
                for s in l.incoming() {
                    if closed.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(s) = s else { continue };
                    conns.lock().unwrap().push(s.try_clone().unwrap());
                    let h = Arc::clone(&h);
                    std::thread::spawn(move || serve(&s, &*h));
                }
            });
        }
        Server {
            url: format!("http://{addr}"),
            closed,
            conns,
            addr,
        }
    }

    /// Closes the connections open, as httptest's `CloseClientConnections`.
    pub(crate) fn close_conns(&self) {
        for c in self.conns.lock().unwrap().drain(..) {
            let _ = c.shutdown(Shutdown::Both);
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        self.close_conns();
    }
}

fn serve(s: &TcpStream, h: &Handler) {
    let mut r = BufReader::new(s.try_clone().unwrap());
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let mut parts = line.split_whitespace();
        let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
            return;
        };
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
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
            if let Some((k, v)) = l.split_once(':') {
                headers.push((k.trim().to_owned(), v.trim().to_owned()));
            }
        }
        let len: usize = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.parse().ok())
            .unwrap_or(0);
        let mut body = vec![0; len];
        if r.read_exact(&mut body).is_err() {
            return;
        }
        let req = Req {
            method: method.to_owned(),
            path: path.to_owned(),
            query: query.to_owned(),
            headers,
            body,
        };
        let resp = Resp {
            stream: s.try_clone().unwrap(),
            headers: Vec::new(),
            streaming: false,
        };
        h(&req, resp);
    }
}

/// The lines a provider logged.
#[derive(Clone, Default)]
pub(crate) struct Lines(Arc<Mutex<Vec<String>>>);

impl Lines {
    pub(crate) fn log(&self) -> Log {
        let l = Arc::clone(&self.0);
        Arc::new(move |s: &str| l.lock().unwrap().push(s.to_owned()))
    }

    pub(crate) fn all(&self) -> String {
        self.0.lock().unwrap().join("\n")
    }

    /// Waits for a line holding `want`.
    pub(crate) fn wait(&self, want: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !self.all().contains(want) {
            assert!(
                std::time::Instant::now() < deadline,
                "logs lack {want:?}:\n{}",
                self.all()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// fit's `shorten`: the follower's timings for tests.
pub(crate) fn short() -> Timings {
    Timings {
        debounce: Duration::from_millis(20),
        min_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(50),
    }
}

/// Parts a provider sent, as they come.
pub(crate) struct Parts {
    pub(crate) rx: std::sync::mpsc::Receiver<Vec<u8>>,
    pub(crate) ups: crate::Updates,
}

impl Parts {
    /// An update for the provider `name`, whose parts arrive here.
    pub(crate) fn new(name: &str) -> (Parts, crate::Update) {
        let (tx, rx) = std::sync::mpsc::channel();
        let cell: Arc<Mutex<Option<crate::Updates>>> = Arc::new(Mutex::new(None));
        let c = Arc::clone(&cell);
        let ups = crate::Updates::new(move || {
            if let Some(u) = c.lock().unwrap().as_ref() {
                while let Some((_, data)) = u.take() {
                    let _ = tx.send(data);
                }
            }
        });
        *cell.lock().unwrap() = Some(ups.clone());
        let u = ups.update(name);
        (Parts { rx, ups }, u)
    }

    /// The next part `ok` takes, passing over others (the follower sends one again every
    /// refresh interval).
    pub(crate) fn next(&self, ok: impl Fn(&[u8]) -> bool) -> Vec<u8> {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut last = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(p) if ok(&p) => return p,
                Ok(p) => last = p,
                Err(_) => panic!("no such part; the last: {}", String::from_utf8_lossy(&last)),
            }
        }
    }

    /// Waits for the part `want`.
    pub(crate) fn want(&self, want: &str) {
        self.next(|p| p == want.as_bytes());
    }
}
