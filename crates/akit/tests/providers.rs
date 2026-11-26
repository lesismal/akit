//! End to end: `akit run` with a `consul_kv` provider (I61) follows the keys under its prefix in
//! a fake Consul, with blocking queries: the routes a key holds are served, and served anew when
//! it changes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

use akio_crypto::base64::STANDARD;
use akio_sys::process::{SIGTERM, kill};

/// A directory of the test's own, removed when it ends.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let d = std::env::temp_dir().join(format!("akit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The running binary, killed if the test fails before it exits.
struct Akit {
    child: Child,
    out: mpsc::Receiver<String>,
}

impl Drop for Akit {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Akit {
    fn start(config: &Path) -> (Akit, SocketAddr) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
            .args(["run", "-shutdown-timeout", "5s", "-c"])
            .arg(config)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, out) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let a = Akit { child, out };
        let line = a.until("akit: listener ");
        let addr = line.split(' ').nth(4).unwrap().parse().unwrap();
        (a, addr)
    }

    fn until(&self, prefix: &str) -> String {
        loop {
            let line = self
                .out
                .recv_timeout(Duration::from_secs(10))
                .unwrap_or_else(|e| panic!("waiting for {prefix:?}: {e}"));
            if line.starts_with(prefix) {
                return line;
            }
        }
    }

    fn stop(mut self) {
        kill(self.child.id(), SIGTERM).unwrap();
        self.until("akit: stopped");
        assert!(self.child.wait().unwrap().success());
    }
}

#[derive(Default)]
struct Kv {
    index: u64,
    keys: BTreeMap<String, String>,
}

/// A fake Consul's KV store: `GET /v1/kv/<prefix>?recurse=true`, as a blocking query when an
/// index is given.
struct FakeConsul {
    st: Arc<(Mutex<Kv>, Condvar)>,
    addr: SocketAddr,
}

impl FakeConsul {
    fn start() -> FakeConsul {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let st = Arc::new((
            Mutex::new(Kv {
                index: 10,
                ..Kv::default()
            }),
            Condvar::new(),
        ));
        let s = Arc::clone(&st);
        std::thread::spawn(move || {
            for c in l.incoming() {
                let Ok(c) = c else { break };
                let s = Arc::clone(&s);
                std::thread::spawn(move || serve(c, &s));
            }
        });
        FakeConsul { st, addr }
    }

    fn put(&self, key: &str, value: &str) {
        let mut kv = self.st.0.lock().unwrap();
        kv.keys.insert(key.to_owned(), value.to_owned());
        kv.index += 1;
        self.st.1.notify_all();
    }
}

fn serve(s: TcpStream, st: &(Mutex<Kv>, Condvar)) {
    let mut w = s.try_clone().unwrap();
    let mut r = BufReader::new(s);
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let target = line.split(' ').nth(1).unwrap_or("").to_owned();
        loop {
            let mut h = String::new();
            if r.read_line(&mut h).unwrap_or(0) == 0 {
                return;
            }
            if h.trim_end().is_empty() {
                break;
            }
        }
        let (path, query) = target.split_once('?').unwrap_or((&target, ""));
        let index: Option<u64> = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("index="))
            .map(|i| i.parse().unwrap());
        let mut kv = st.0.lock().unwrap();
        if let Some(i) = index {
            let until = Instant::now() + Duration::from_secs(5);
            while kv.index <= i && Instant::now() < until {
                kv = st.1.wait_timeout(kv, Duration::from_millis(100)).unwrap().0;
            }
        }
        let prefix = path.strip_prefix("/v1/kv/").unwrap_or("");
        let entries: Vec<String> = kv
            .keys
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| format!(r#"{{"Key":"{k}","Value":"{}"}}"#, STANDARD.encode(v)))
            .collect();
        let (status, body) = if entries.is_empty() {
            ("404 Not Found", String::new())
        } else {
            ("200 OK", format!("[{}]", entries.join(",")))
        };
        let resp = format!(
            "HTTP/1.1 {status}\r\nX-Consul-Index: {}\r\nContent-Length: {}\r\n\r\n{body}",
            kv.index,
            body.len()
        );
        drop(kv);
        if w.write_all(resp.as_bytes()).is_err() {
            return;
        }
    }
}

/// `GET path` from akit: the status and the body.
fn get(addr: SocketAddr, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let status = out.split(' ').nth(1).unwrap().parse().unwrap();
    let body = out.split_once("\r\n\r\n").map_or("", |(_, b)| b).to_owned();
    (status, body)
}

fn route(body: &str) -> String {
    format!(
        r#"{{"routes": [{{"name": "app", "match": {{"path": ["/app"]}}, "handler": {{"module": "respond", "params": {{"body": "{body}"}}}}}}]}}"#
    )
}

#[test]
fn consul_kv_routes_follow_the_keys() {
    let consul = FakeConsul::start();
    consul.put("akit/app", &route("one"));
    consul.put("other/x", &route("other"));
    let dir = Dir::new("providers");
    let config = dir.0.join("akit.json");
    std::fs::write(
        &config,
        format!(
            r#"{{
  "listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
  "routes": [{{"name": "base", "match": {{"path": ["/base"]}}, "handler": {{"module": "respond", "params": {{"body": "base"}}}}}}],
  "providers": [{{"name": "kv", "consul_kv": {{"endpoint": "http://{}", "refresh_interval": "1h"}}}}]
}}"#,
            consul.addr
        ),
    )
    .unwrap();
    let (akit, addr) = Akit::start(&config);
    akit.until(&format!(
        r#"akit: provider kv watching Consul's keys under "akit/" at http://{}"#,
        consul.addr
    ));
    akit.until("akit: applied what provider kv provides");
    assert_eq!(get(addr, "/app"), (200, "one".to_owned()));
    assert_eq!(get(addr, "/base"), (200, "base".to_owned()));

    consul.put("akit/app", &route("two"));
    akit.until("akit: applied what provider kv provides");
    assert_eq!(get(addr, "/app"), (200, "two".to_owned()));
    akit.stop();
}
