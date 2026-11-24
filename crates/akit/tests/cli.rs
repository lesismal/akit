//! End to end: the commands, flags and exit codes (I06): `validate`, several `-c` and
//! directories, and `run`'s graceful shutdown on SIGTERM and SIGINT.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use akio_sys::process::{SIGINT, SIGTERM, kill};

const LISTENER: &str = r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}]}"#;
const ROUTE: &str = r#"{
  "routes": [{"listeners": ["web"], "match": {"path": ["/hello"]},
              "handler": {"module": "respond", "params": {"body": "hello\n"}}}]
}"#;

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("cli")
        .join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn write(d: &Path, name: &str, text: &str) -> PathBuf {
    let p = d.join(name);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, text).unwrap();
    p
}

fn akit(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(args)
        .output()
        .unwrap()
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn validate_merges_files_and_reports_errors() {
    let d = dir("validate");
    let a = write(&d, "a.json", LISTENER);
    let b = write(&d, "b.json", ROUTE);
    let out = akit(&["validate", "-c", s(&a), "-c", s(&b)]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("{}, {}: valid\n", a.display(), b.display())
    );

    // The route alone: its listener is in no file.
    let out = akit(&["validate", "-c", s(&b)]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(
        err.contains(&format!(
            "{}:2:14: routes[0].listeners[0]: there is no listener named \"web\"",
            b.display()
        )),
        "{err}"
    );
    assert!(out.stdout.is_empty());

    // A directory is its files, merged.
    let out = akit(&["validate", &format!("-c={}", d.display())]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // fit.json by default, which is not here.
    let out = Command::new(env!("CARGO_BIN_EXE_akit"))
        .arg("validate")
        .current_dir(&d)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .starts_with("fit.json: ")
    );

    // fit's own .conf format (I23), its problems at their line and column.
    let conf = write(
        &d,
        "fit.conf",
        "listener web :8080\nroute { handler respond { status 204 } }\n",
    );
    let out = akit(&["validate", "-c", s(&conf)]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bad = write(&d, "bad.conf", "listener web {\n\taddress nowhere\n}\n");
    let out = akit(&["validate", "-c", s(&bad)]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .starts_with(&format!("{}:2:10: listeners[0].address: ", bad.display()))
    );
}

#[test]
fn usage_errors_exit_2() {
    for args in [
        &[][..],
        &["nope"],
        &["run", "-x"],
        &["run", "-c"],
        &["run", "extra"],
        &["run", "-shutdown-timeout", "soon"],
        &["run", "-watch-interval", "soon"],
        &["run", "-watch-interval", "0s"],
        &["run", "-admin"],
        &["validate", "-shutdown-timeout=1s"],
        &["version", "-x"],
    ] {
        assert_eq!(akit(args).status.code(), Some(2), "{args:?}");
    }
    assert_eq!(akit(&["help"]).status.code(), Some(0));
    let v = akit(&["version"]);
    assert_eq!(v.status.code(), Some(0));
    assert!(String::from_utf8(v.stdout).unwrap().starts_with("akit "));
}

/// The running binary, killed if a test fails before it exits.
struct Running(Option<Child>, SocketAddr);

impl Running {
    fn start(args: &[&str]) -> Running {
        let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        assert!(line.starts_with("akit: listener \"web\" on "), "{line:?}");
        let addr = line.split(' ').nth(4).unwrap().parse().unwrap();
        // Put the rest of stdout back for `stop` to read.
        child.stdout = None;
        let rest = std::thread::spawn(move || {
            let mut s = String::new();
            out.read_to_string(&mut s).unwrap();
            s
        });
        STDOUT.with(|r| *r.borrow_mut() = Some(rest));
        Running(Some(child), addr)
    }

    /// Sends `sig` and returns the exit code, stdout after the listener line, and stderr.
    fn stop(mut self, sig: i32) -> (Option<i32>, String, String) {
        let mut child = self.0.take().unwrap();
        kill(child.id(), sig).unwrap();
        let status = child.wait().unwrap();
        let mut err = String::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut err)
            .unwrap();
        let out = STDOUT.with(|r| r.borrow_mut().take().unwrap().join().unwrap());
        (status.code(), out, err)
    }
}

thread_local! {
    static STDOUT: std::cell::RefCell<Option<std::thread::JoinHandle<String>>> =
        const { std::cell::RefCell::new(None) };
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(c) = &mut self.0 {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn get(addr: SocketAddr, path: &str) -> String {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    resp
}

#[test]
fn run_merges_files_and_stops_on_sigterm() {
    let d = dir("run");
    let a = write(&d, "a.json", LISTENER);
    let b = write(&d, "b.json", ROUTE);
    let r = Running::start(&["run", "-c", s(&a), "-c", s(&b), "-shutdown-timeout", "5s"]);
    assert!(get(r.1, "/hello").ends_with("\r\n\r\nhello\n"));
    // An idle keep-alive connection does not hold the shutdown up.
    let mut idle = TcpStream::connect(r.1).unwrap();
    idle.write_all(b"GET /hello HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    idle.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut buf = [0u8; 256];
    assert!(idle.read(&mut buf).unwrap() > 0);
    let start = std::time::Instant::now();
    let (code, out, err) = r.stop(SIGTERM);
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(out, "akit: shutting down\nakit: stopped\n");
    assert!(start.elapsed() < Duration::from_secs(4));
    // The idle connection was closed.
    assert_eq!(idle.read(&mut buf).unwrap(), 0);
}

#[test]
fn run_stops_on_sigint_from_a_directory() {
    let d = dir("run-dir");
    write(&d, "a.json", LISTENER);
    write(&d, "routes/b.json", ROUTE);
    let r = Running::start(&["run", "-c", s(&d)]);
    assert!(get(r.1, "/hello").ends_with("hello\n"));
    let (code, out, _) = r.stop(SIGINT);
    assert_eq!(code, Some(0));
    assert_eq!(out, "akit: shutting down\nakit: stopped\n");
}
