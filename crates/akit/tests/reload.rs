//! End to end: `akit run` reloads its config on SIGHUP (I22) while clients send requests, and
//! none of them fails: the routes change, a listener is added and removed, a broken config and
//! one that removes a listener akit started with leave the running config.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use akio_sys::process::{SIGHUP, SIGTERM, kill};

/// A config: the `web` listener, `/hello` answering `body`, and the `extra` listener when asked.
fn config(body: &str, extra: bool) -> String {
    let extra = if extra {
        r#", {"name": "extra", "address": "127.0.0.1:0"}"#
    } else {
        ""
    };
    format!(
        r#"{{
  "listeners": [{{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}}{extra}],
  "routes": [
    {{"match": {{"path": ["/hello"]}}, "handler": {{"module": "respond", "params": {{"body": "{body}"}}}}}}
  ]
}}"#
    )
}

/// The running binary, killed if the test fails before it exits.
struct Akit {
    child: Child,
    out: mpsc::Receiver<String>,
    err: mpsc::Receiver<String>,
}

impl Drop for Akit {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn lines<R: Read + Send + 'static>(r: R) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(r).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

impl Akit {
    fn start(path: &PathBuf) -> Akit {
        let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
            .args(["run", "-shutdown-timeout", "5s", "-c"])
            .arg(path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let out: ChildStdout = child.stdout.take().unwrap();
        let err: ChildStderr = child.stderr.take().unwrap();
        Akit {
            child,
            out: lines(out),
            err: lines(err),
        }
    }

    fn signal(&self, sig: i32) {
        kill(self.child.id(), sig).unwrap();
    }

    /// stdout's lines up to and including the first that starts with `last`.
    fn out_until(&self, last: &str) -> Vec<String> {
        until(&self.out, last)
    }

    fn err_until(&self, last: &str) -> Vec<String> {
        until(&self.err, last)
    }
}

fn until(rx: &mpsc::Receiver<String>, last: &str) -> Vec<String> {
    let mut v = Vec::new();
    loop {
        let line = rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|e| panic!("waiting for {last:?} after {v:?}: {e}"));
        let done = line.starts_with(last);
        v.push(line);
        if done {
            return v;
        }
    }
}

/// `akit: listener "NAME" on ADDR (...)`: the name and the address.
fn listener(line: &str) -> (String, SocketAddr) {
    let mut words = line.split(' ');
    assert_eq!(words.next(), Some("akit:"), "{line}");
    assert_eq!(words.next(), Some("listener"), "{line}");
    let name = words.next().unwrap().trim_matches('"').to_owned();
    assert_eq!(words.next(), Some("on"), "{line}");
    (name, words.next().unwrap().parse().unwrap())
}

/// One keep-alive request on `s`; the body, or why it failed.
fn get(s: &mut BufReader<TcpStream>, close: bool) -> Result<String, String> {
    let conn = if close { "close" } else { "keep-alive" };
    write!(
        s.get_mut(),
        "GET /hello HTTP/1.1\r\nHost: x\r\nConnection: {conn}\r\n\r\n"
    )
    .map_err(|e| format!("write: {e}"))?;
    let mut status = String::new();
    s.read_line(&mut status).map_err(|e| format!("read: {e}"))?;
    if !status.starts_with("HTTP/1.1 200 ") {
        return Err(format!("status {status:?}"));
    }
    let mut len = None;
    loop {
        let mut h = String::new();
        s.read_line(&mut h).map_err(|e| format!("read: {e}"))?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((n, v)) = h.split_once(':') {
            if n.eq_ignore_ascii_case("content-length") {
                len = v.trim().parse::<usize>().ok();
            }
        }
    }
    let mut body = vec![0; len.ok_or("no content-length")?];
    s.read_exact(&mut body).map_err(|e| format!("body: {e}"))?;
    String::from_utf8(body).map_err(|e| e.to_string())
}

fn connect(addr: SocketAddr) -> Result<BufReader<TcpStream>, String> {
    let s = TcpStream::connect(addr).map_err(|e| format!("connect: {e}"))?;
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    Ok(BufReader::new(s))
}

/// What the clients saw.
#[derive(Default)]
struct Seen {
    ok: AtomicUsize,
    failed: Mutex<Vec<String>>,
    last: Mutex<String>,
}

/// A client loop: keep-alive connections of 20 requests each (the last closing it), until
/// `stop`.
fn client(addr: SocketAddr, seen: Arc<Seen>, stop: Arc<AtomicBool>) {
    while !stop.load(SeqCst) {
        let mut s = match connect(addr) {
            Ok(s) => s,
            Err(e) => {
                seen.failed.lock().unwrap().push(e);
                continue;
            }
        };
        for i in 0..20 {
            match get(&mut s, i == 19) {
                Ok(body) => {
                    seen.ok.fetch_add(1, SeqCst);
                    *seen.last.lock().unwrap() = body;
                }
                Err(e) => {
                    seen.failed.lock().unwrap().push(e);
                    break;
                }
            }
        }
    }
}

/// Waits until the clients have seen `body`, and some more requests after it.
fn wait_for(seen: &Seen, body: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while *seen.last.lock().unwrap() != body {
        assert!(Instant::now() < deadline, "the clients never saw {body:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
    let n = seen.ok.load(SeqCst);
    while seen.ok.load(SeqCst) < n + 100 {
        assert!(Instant::now() < deadline, "the clients stopped");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn reloads_under_traffic_with_no_failed_request() {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("reload.json");
    std::fs::write(&path, config("v1", false)).unwrap();
    let akit = Akit::start(&path);
    let (name, web) = listener(&akit.out_until("akit: listener")[0]);
    assert_eq!(name, "web");

    let seen = Arc::new(Seen::default());
    let stop = Arc::new(AtomicBool::new(false));
    let clients: Vec<_> = (0..4)
        .map(|_| {
            let (seen, stop) = (Arc::clone(&seen), Arc::clone(&stop));
            std::thread::spawn(move || client(web, seen, stop))
        })
        .collect();
    wait_for(&seen, "v1");

    // New routes and a new listener.
    std::fs::write(&path, config("v2", true)).unwrap();
    akit.signal(SIGHUP);
    let out = akit.out_until("akit: reloaded");
    assert_eq!(out[0], format!("akit: reloading {}", path.display()));
    let (name, extra) = listener(&out[1]);
    assert_eq!(name, "extra");
    assert_eq!(out.len(), 3, "{out:?}");
    wait_for(&seen, "v2");
    assert_eq!(get(&mut connect(extra).unwrap(), true).unwrap(), "v2");

    // A broken config: the running one stays.
    std::fs::write(&path, "{\"listeners\": [").unwrap();
    akit.signal(SIGHUP);
    let err = akit.err_until("akit: reload failed; the running config stays");
    assert_eq!(err.len(), 1, "{err:?}");
    let err = akit.err.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(err.starts_with(&format!("{}:", path.display())), "{err}");
    assert_eq!(
        akit.out_until("akit: reloading"),
        [format!("akit: reloading {}", path.display())]
    );
    assert_eq!(get(&mut connect(extra).unwrap(), true).unwrap(), "v2");

    // A config without the listener akit started with: refused until akio can close one (A1).
    std::fs::write(
        &path,
        r#"{"listeners": [{"name": "other", "address": "127.0.0.1:0"}]}"#,
    )
    .unwrap();
    akit.signal(SIGHUP);
    let err = akit.err_until("akit: reload failed; the running config stays");
    assert!(err[0].contains("listener \"web\" on"), "{err:?}");
    assert!(err[0].contains("A1"), "{err:?}");
    akit.out_until("akit: reloading");

    // The added listener removed, the routes changed again.
    std::fs::write(&path, config("v3", false)).unwrap();
    akit.signal(SIGHUP);
    let out = akit.out_until("akit: reloaded");
    assert_eq!(
        out[1],
        format!("akit: listener \"extra\" on {extra} removed"),
        "{out:?}"
    );
    assert_eq!(out.len(), 3, "{out:?}");
    wait_for(&seen, "v3");
    assert!(TcpStream::connect(extra).is_err(), "extra still listens");

    // Again, many times, as fast as signals come.
    for i in 0..20 {
        std::fs::write(&path, config(&format!("v{}", 4 + i), i % 2 == 0)).unwrap();
        akit.signal(SIGHUP);
        akit.out_until("akit: reloaded");
    }
    wait_for(&seen, "v23");

    stop.store(true, SeqCst);
    for c in clients {
        c.join().unwrap();
    }
    let failed = seen.failed.lock().unwrap();
    assert!(failed.is_empty(), "failed requests: {failed:?}");
    assert!(seen.ok.load(SeqCst) >= 400, "{}", seen.ok.load(SeqCst));

    akit.signal(SIGTERM);
    let mut akit = akit;
    let status = akit.child.wait().unwrap();
    assert_eq!(status.code(), Some(0));
    let _ = std::fs::remove_file(&path);
}
