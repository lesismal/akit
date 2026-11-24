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

/// With `metrics` (I37): the Admin API serves the scrape at `GET /metrics`, in Prometheus's text
/// format, with the requests counted and every config applied after the first counted, from
/// SIGHUP and from the Admin API alike, a refused load included.
#[test]
fn the_admin_api_serves_the_metrics() {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("admin-metrics.json");
    let (web, api) = (free(), free());
    let doc = |body: &str| {
        config(web, api, body).replacen("\"listeners\"", "\"metrics\": {},\n  \"listeners\"", 1)
    };
    std::fs::write(&path, doc("v1")).unwrap();
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
    until(&akit.out, "akit: admin API on");
    assert_eq!(request(web, "GET", "/", None).2, "v1");
    assert_eq!(request(api, "POST", "/load", Some(&doc("v2"))).0, 200);
    until(&akit.out, "akit: loaded");
    assert_eq!(
        request(api, "POST", "/load", Some("{\"listeners\": 1}")).0,
        400
    );
    akit_signal(&akit, SIGHUP);
    until(&akit.out, "akit: reloaded");
    assert_eq!(request(web, "GET", "/", None).2, "v1");

    let mut s = TcpStream::connect(api).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "GET /metrics HTTP/1.1\r\nHost: {api}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(
        head.to_ascii_lowercase()
            .contains("content-type: text/plain; version=0.0.4; charset=utf-8"),
        "{head}"
    );
    for line in [
        "fit_config_reloads_total{result=\"success\"} 2",
        "fit_config_reloads_total{result=\"failure\"} 1",
        "fit_config_version 3",
        "fit_http_requests_total{listener=\"web\",route=\"#0\",code=\"2xx\"} 2",
    ] {
        assert!(body.lines().any(|l| l == line), "no {line:?} in:\n{body}");
    }

    akit_signal(&akit, SIGTERM);
    until(&akit.out, "akit: stopped");
}

/// One request with `headers`, `Connection: close`: the status, the head and the body.
fn call(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> (u16, String, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    for (n, v) in headers {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    let b = body.unwrap_or("");
    if body.is_some() {
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");
    req.push_str(b);
    s.write_all(req.as_bytes()).unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, head.to_owned(), body.to_owned())
}

/// I27 through `akit run`: with `token_file` every request carries the token; an edit by pointer
/// is applied, a dry run checks a change (its new listener tried, not opened) and applies
/// nothing, and a rollback applies the first config again, each numbered in the one history.
#[test]
fn edits_dry_runs_and_rollbacks_behind_the_token() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let (path, token) = (dir.join("admin-edits.json"), dir.join("admin-edits.token"));
    std::fs::write(&token, "s3cret\n").unwrap();
    let (web, api, extra) = (free(), free(), free());
    let first = config(web, api, "v1").replacen(
        "\"history\": 5",
        &format!(
            "\"history\": 5, \"token_file\": {:?}",
            token.to_str().unwrap()
        ),
        1,
    );
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
    until(&akit.out, "akit: admin API on");
    let auth = ("Authorization", "Bearer s3cret");
    let json = ("Content-Type", "application/json");

    assert_eq!(call(api, "GET", "/config", &[], None).0, 401);
    let (status, head, body) = call(
        api,
        "PATCH",
        "/config/routes/0/handler/params",
        &[auth, json],
        Some(r#"{"body": "v2"}"#),
    );
    assert_eq!(status, 200, "{head}\n{body}");
    assert_eq!(
        until(&akit.out, "akit: loaded"),
        ["akit: loaded a config from the admin API (config version 2)"]
    );
    assert_eq!(request(web, "GET", "/", None).2, "v2");

    // A dry run: the listener it would add is listed, and not opened.
    let listener = format!(r#"{{"name": "extra", "address": "{extra}"}}"#);
    let (status, head, body) = call(
        api,
        "PUT",
        "/config/listeners/-?dry_run=1",
        &[auth, json],
        Some(&listener),
    );
    assert_eq!(status, 200, "{head}\n{body}");
    assert!(
        head.contains("Fit-Dry-Run: 1") && head.contains("Fit-Config-Version: 2"),
        "{head}"
    );
    assert!(
        body.contains(&format!(
            r#""listeners_added":[{{"name":"extra","address":"{extra}""#
        )),
        "{body}"
    );
    assert!(TcpStream::connect(extra).is_err());

    let (status, head, body) = call(api, "POST", "/rollback/1", &[auth], None);
    assert_eq!(status, 200, "{head}\n{body}");
    assert_eq!(
        until(&akit.out, "akit: loaded"),
        ["akit: loaded a config from the admin API (config version 3)"]
    );
    assert_eq!(request(web, "GET", "/", None).2, "v1");
    let (_, _, body) = call(api, "GET", "/versions", &[auth], None);
    let sources: Vec<&str> = body
        .split("\"source\":\"")
        .skip(1)
        .map(|s| s.split('"').next().unwrap())
        .collect();
    assert_eq!(sources, ["file", "admin", "rollback"], "{body}");
    let (_, _, now) = call(api, "GET", "/config", &[auth], None);
    // The first config as it was kept, comments blanked out.
    assert_eq!(now.len(), first.len());
    assert!(
        now.contains(r#""body": "v1""#) && !now.contains("//"),
        "{now}"
    );

    akit_signal(&akit, SIGTERM);
    until(&akit.out, "akit: stopped");
    let _ = std::fs::remove_file(&token);
}
