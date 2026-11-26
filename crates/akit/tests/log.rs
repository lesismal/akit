//! End to end: akit's own log (I82). `akit run` writes its loggers' messages in fit's formats
//! and the log's zone, to standard error by default and to a file the access log may share,
//! each line whole; `akit validate` refuses a zone that is none. I83: a reload changes the
//! levels, the zone and the rotation in place and warns of an output or format changed; the
//! Admin API lists the log files, rotates one and cleans one up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use akio_sys::process::{SIGHUP, SIGTERM, kill};

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

/// The running binary, its stdout and stderr lines.
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

fn lines(r: impl Read + Send + 'static) -> mpsc::Receiver<String> {
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

fn until(rx: &mpsc::Receiver<String>, pred: impl Fn(&str) -> bool) -> String {
    loop {
        let line = rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|e| panic!("waiting: {e}"));
        if pred(&line) {
            return line;
        }
    }
}

impl Akit {
    fn start(config: &Path) -> (Akit, SocketAddr) {
        Akit::start_tz(config, None)
    }

    /// With `TZ` set to `tz` for the local zone.
    fn start_tz(config: &Path, tz: Option<&str>) -> (Akit, SocketAddr) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_akit"));
        if let Some(tz) = tz {
            cmd.env("TZ", tz);
        }
        let mut child = cmd
            .args(["run", "-shutdown-timeout", "5s", "-c"])
            .arg(config)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let out = lines(child.stdout.take().unwrap());
        let err = lines(child.stderr.take().unwrap());
        let a = Akit { child, out, err };
        let line = until(&a.out, |l| l.starts_with("akit: listener "));
        let addr = line.split(' ').nth(4).unwrap().parse().unwrap();
        (a, addr)
    }

    fn stop(mut self) {
        kill(self.child.id(), SIGTERM).unwrap();
        until(&self.out, |l| l == "akit: stopped");
        assert!(self.child.wait().unwrap().success());
    }
}

fn get(addr: SocketAddr, path: &str) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: h\r\nConnection: close\r\n\r\n").as_bytes())
        .unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
}

fn write_config(dir: &Path, rest: &str) -> PathBuf {
    let path = dir.join("akit.json");
    std::fs::write(
        &path,
        format!(
            r#"{{
  "listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
  "routes": [{{"handler": {{"module": "respond", "params": {{"body": "hi"}}}}}}],
  {rest}
}}"#
        ),
    )
    .unwrap();
    path
}

#[test]
fn messages_go_to_standard_error_as_lines_by_default() {
    let dir = Dir::new("log-default");
    let access = dir.0.join("access.log");
    let cfg = write_config(
        &dir.0,
        &format!(r#""access_log": {{"output": "{}"}}"#, access.display()),
    );
    let (akit, _) = Akit::start(&cfg);
    let line = until(&akit.err, |l| l.contains("has no rotate"));
    // `2026-11-26 10:00:00.000 WARN  access log file … logger=server`, local time.
    assert_eq!(line.as_bytes()[10], b' ', "{line}");
    assert!(
        line.get(23..).unwrap().starts_with(&format!(
            " WARN  access log file {} has no rotate: it will grow without bound logger=server",
            access.display()
        )),
        "{line}"
    );
    akit.stop();
}

#[test]
fn a_file_shared_with_the_access_log_in_json_and_utc() {
    let dir = Dir::new("log-shared");
    let file = dir.0.join("all.log");
    let cfg = write_config(
        &dir.0,
        &format!(
            r#""log": {{"format": "json", "time_zone": "utc", "output": "{f}"}},
  "access_log": {{"output": "{f}", "time_zone": "UTC", "flush_interval": "10ms"}}"#,
            f = file.display()
        ),
    );
    let (akit, addr) = Akit::start(&cfg);
    for i in 0..20 {
        get(addr, &format!("/n{i}"));
    }
    akit.stop();
    let text = std::fs::read_to_string(&file).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let log: Vec<&&str> = lines
        .iter()
        .filter(|l| l.contains("\"logger\":\"server\""))
        .collect();
    assert_eq!(log.len(), 2, "{text}");
    for l in &log {
        assert!(l.starts_with("{\"time\":\""), "{l}");
        assert!(l.contains("Z\",\"level\":\"WARN\",\"msg\":"), "{l}");
        assert!(l.ends_with(",\"logger\":\"server\"}"), "{l}");
    }
    let access: Vec<&&str> = lines.iter().filter(|l| l.contains("\"uri\":")).collect();
    assert_eq!(access.len(), 20, "{text}");
    for l in &access {
        assert!(l.starts_with("{\"time\":\"") && l.ends_with('}'), "{l}");
    }
    assert_eq!(lines.len(), 22, "{text}");
}

#[test]
fn validate_refuses_an_unknown_zone() {
    let dir = Dir::new("log-validate");
    let cfg = write_config(&dir.0, r#""log": {"time_zone": "Mars/Olympus"}"#);
    let out = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["validate", "-c"])
        .arg(&cfg)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("log.time_zone"), "{err}");
    assert!(err.contains("Mars/Olympus"), "{err}");
}

/// The names of `dir`'s files starting with `base`, sorted.
fn names(dir: &Path, base: &str) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(base))
        .collect();
    v.sort();
    v
}

#[test]
fn reload_changes_levels_zone_and_rotation_in_place() {
    let dir = Dir::new("log-reload");
    let file = dir.0.join("akit.log");
    // The log and the access log share the file: one rotate and one zone for both.
    let both = |format: &str, rest: &str, zone: &str, rotate: &str| {
        format!(
            r#""log": {{"format": "{format}", "output": "{f}", "time_zone": "{zone}", "rotate": {rotate}{rest}}},
  "access_log": {{"output": "{f}", "time_zone": "{zone}", "flush_interval": "10ms", "rotate": {rotate}}}"#,
            f = file.display()
        )
    };
    let big = r#"{"max_bytes": 1048576, "compress": false}"#;
    let small = r#"{"max_bytes": 300, "compress": false}"#;
    let cfg = write_config(&dir.0, &both("json", "", "utc", big));
    // The local zone, eight hours east, for the reload to move to.
    let (akit, addr) = Akit::start_tz(&cfg, Some("XYZ-8"));
    // Debug for every logger, the local zone, and a file that turns at 300 bytes.
    write_config(
        &dir.0,
        &both("json", r#", "level": "debug""#, "local", small),
    );
    kill(akit.child.id(), SIGHUP).unwrap();
    until(&akit.out, |l| l.starts_with("akit: reloaded"));
    for i in 0..10 {
        get(addr, &format!("/n{i}"));
        std::thread::sleep(Duration::from_millis(20));
    }
    // A format changed takes a restart, and says so.
    write_config(
        &dir.0,
        &both("text", r#", "level": "debug""#, "local", small),
    );
    kill(akit.child.id(), SIGHUP).unwrap();
    until(&akit.err, |l| {
        l == "akit: log's outputs and formats take a restart; the running ones stay"
    });
    until(&akit.out, |l| l.starts_with("akit: reloaded"));
    akit.stop();
    // Rotated at 300 bytes; akio's engine line at debug from the reload on, in JSON (the format
    // stays) at +08:00; every request's line, at +08:00 too.
    let all = names(&dir.0, "akit.log");
    assert!(all.len() >= 3, "{all:?}");
    let (mut engine, mut access) = (0, 0);
    for n in &all {
        let text = std::fs::read_to_string(dir.0.join(n)).unwrap();
        for l in text.lines() {
            assert!(l.starts_with("{\"time\":\""), "{n}: {l}");
            if l.contains("\"logger\":\"engine\"") {
                engine += 1;
                assert!(l.contains("+08:00\",\"level\":\"DEBUG\""), "{l}");
            }
            if l.contains("\"uri\":\"/n") {
                access += 1;
                assert!(l.contains("+08:00\""), "{l}");
            }
        }
    }
    assert!(engine > 0, "{all:?}");
    assert_eq!(access, 10, "{all:?}");
}

/// A port nothing listens on now.
fn free_port() -> SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

/// Sends a request to the Admin API at `api` and returns the status and the body.
fn admin(api: SocketAddr, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut s = TcpStream::connect(api).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let body = body.unwrap_or("");
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    let status = out
        .split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let body = out.split_once("\r\n\r\n").map_or("", |(_, b)| b).to_owned();
    (status, body)
}

#[test]
fn the_admin_api_lists_rotates_and_cleans_up() {
    let dir = Dir::new("log-admin");
    let access = dir.0.join("access.log");
    let api = free_port();
    let keep = |n: u32| {
        format!(
            r#""admin": {{"address": "{api}"}},
  "access_log": {{"output": "{}", "flush_interval": "10ms",
                 "rotate": {{"max_bytes": 400, "max_files": {n}, "compress": false}}}}"#,
            access.display()
        )
    };
    let cfg = write_config(&dir.0, &keep(100));
    let (akit, addr) = Akit::start(&cfg);
    until(&akit.out, |l| l.starts_with("akit: admin API on"));
    for i in 0..12 {
        get(addr, &format!("/n{i}"));
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(100));
    let (status, body) = admin(api, "GET", "/logs", None);
    assert_eq!(status, 200, "{body}");
    assert!(body.starts_with("{\"outputs\":[{\"path\":"), "{body}");
    assert!(body.contains("\"loggers\":[\"access_log\"],\"access_log\":true,\"format\":\"json\",\"time_zone\":\"local\""), "{body}");
    assert!(body.contains("\"rotate\":{\"max_bytes\":400,\"interval_seconds\":0,\"max_age_seconds\":2592000,\"max_files\":100,\"max_total_bytes\":1073741824,\"compress\":false}"), "{body}");
    let before = names(&dir.0, "access.log");
    assert!(before.len() >= 3, "{before:?}");
    assert_eq!(
        body.matches("\"name\":").count(),
        before.len(),
        "{before:?} {body}"
    );
    assert!(body.contains("\"current\":true"), "{body}");

    // Rotate now: one more file.
    get(addr, "/last");
    std::thread::sleep(Duration::from_millis(100));
    let n = names(&dir.0, "access.log").len();
    let path = format!(r#"{{"path": "{}"}}"#, access.display());
    assert_eq!(
        admin(api, "POST", "/logs/rotate", Some(&path)),
        (200, "{}\n".to_owned())
    );
    assert_eq!(names(&dir.0, "access.log").len(), n + 1);

    // Keep one: a dry run reports, then the cleanup removes.
    write_config(&dir.0, &keep(1));
    kill(akit.child.id(), SIGHUP).unwrap();
    until(&akit.out, |l| l.starts_with("akit: reloaded"));
    let dry = format!(r#"{{"path": "{}", "dry_run": true}}"#, access.display());
    let (status, body) = admin(api, "POST", "/logs/cleanup", Some(&dry));
    assert_eq!(status, 200, "{body}");
    // The open file and the newest rotated one stay.
    let would = body.matches("\"name\":").count();
    assert_eq!(would, n - 1, "{body}");
    assert_eq!(names(&dir.0, "access.log").len(), n + 1);
    let (status, body) = admin(api, "POST", "/logs/cleanup", Some(&path));
    assert_eq!(status, 200, "{body}");
    assert_eq!(body.matches("\"name\":").count(), would, "{body}");
    assert_eq!(names(&dir.0, "access.log").len(), 2);

    // Only the running config's files.
    let (status, body) = admin(
        api,
        "POST",
        "/logs/rotate",
        Some(r#"{"path": "/etc/passwd"}"#),
    );
    assert_eq!(status, 400, "{body}");
    assert!(
        body.contains("is not a log file of the running config"),
        "{body}"
    );
    assert_eq!(admin(api, "POST", "/logs/cleanup", Some("{}")).0, 400);
    assert_eq!(admin(api, "GET", "/logs/rotate", None).0, 405);
    akit.stop();
}
