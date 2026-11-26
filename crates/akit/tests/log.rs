//! End to end: akit's own log (I82). `akit run` writes its loggers' messages in fit's formats
//! and the log's zone, to standard error by default and to a file the access log may share,
//! each line whole; `akit validate` refuses a zone that is none.
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

use akio_sys::process::{SIGTERM, kill};

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
        let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
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
