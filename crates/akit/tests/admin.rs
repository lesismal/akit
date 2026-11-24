//! End to end: `akit run` with `admin` (I26) serves the Admin API, applies a config loaded
//! through it by the same path as SIGHUP's reload, and numbers both in one history.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use akio_sys::process::{SIGHUP, SIGTERM, kill};

/// A free port on 127.0.0.1, for a listener the configs name.
fn free() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

fn config(web: SocketAddr, api: SocketAddr, body: &str) -> String {
    format!(
        r#"{{
  // The Admin API, on loopback.
  "admin": {{"address": "{api}", "history": 5}},
  "listeners": [{{"name": "web", "address": "{web}"}}],
  "routes": [{{"handler": {{"module": "respond", "params": {{"body": "{body}"}}}}}}],
}}"#
    )
}

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

/// One request, `Connection: close`: the status, the version header and the body.
fn request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, String, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    if let Some(b) = body {
        req.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ));
    } else {
        req.push_str("\r\n");
    }
    s.write_all(req.as_bytes()).unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    let version = head
        .lines()
        .find_map(|l| l.strip_prefix("Fit-Config-Version: "))
        .unwrap_or("")
        .to_owned();
    (status, version, body.to_owned())
}

#[test]
fn loads_through_the_admin_api_and_sighup_share_one_history() {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("admin.json");
    let (web, api) = (free(), free());
    let first = config(web, api, "v1");
    std::fs::write(&path, &first).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["run", "-shutdown-timeout", "5s", "-c"])
        .arg(&path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
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
    let akit = Akit { child, out };
    let lines = until(&akit.out, "akit: admin API on");
    assert_eq!(lines.last().unwrap(), &format!("akit: admin API on {api}"));

    // The config as written, comments blanked out.
    let (status, version, body) = request(api, "GET", "/config", None);
    assert_eq!((status, version.as_str()), (200, "1"));
    assert_eq!(body.len(), first.len());
    assert!(
        body.contains("\"history\": 5") && !body.contains("//"),
        "{body}"
    );
    assert_eq!(request(web, "GET", "/", None).2, "v1");

    // A load: applied by the control thread, as a reload.
    let (status, version, body) = request(api, "POST", "/load", Some(&config(web, api, "v2")));
    assert_eq!((status, version.as_str()), (200, "2"), "{body}");
    assert_eq!(
        until(&akit.out, "akit: loaded"),
        ["akit: loaded a config from the admin API (config version 2)"]
    );
    assert_eq!(request(web, "GET", "/", None).2, "v2");

    // SIGHUP: the file again, as version 3.
    akit_signal(&akit, SIGHUP);
    let lines = until(&akit.out, "akit: reloaded");
    assert_eq!(
        lines.last().unwrap(),
        &format!("akit: reloaded {} (config version 3)", path.display())
    );
    assert_eq!(request(web, "GET", "/", None).2, "v1");
    let (_, version, body) = request(api, "GET", "/versions", None);
    assert_eq!(version, "");
    let sources: Vec<&str> = body
        .split("\"source\":\"")
        .skip(1)
        .map(|s| s.split('"').next().unwrap())
        .collect();
    assert_eq!(sources, ["file", "admin", "file"], "{body}");
    assert!(body.starts_with("{\"current\":3,"), "{body}");

    akit_signal(&akit, SIGTERM);
    until(&akit.out, "akit: stopped");
}

fn akit_signal(akit: &Akit, sig: i32) {
    kill(akit.child.id(), sig).unwrap();
}
