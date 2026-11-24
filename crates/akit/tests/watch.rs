//! End to end: `akit run -watch` (I29, fit's file provider) applies its config each time the
//! files change, as fit's `TestServeWatch`, `TestWatch` and `TestConfigDirectory`: each edit is
//! served; SIGHUP applies an edit and the watcher does not apply it again; a broken edit is
//! reported and the running config stays until the next; a file removed is reported and applied
//! when it comes back; a directory's files added and taken out are applied, and a broken one is
//! reported by its name.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use akio_sys::process::{SIGHUP, SIGTERM, kill};

/// The running binary, killed when dropped, and its stdout and stderr lines.
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

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("watch")
        .join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

/// Writes `text` at `p`, its directories made.
fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// A config of one listener answering every request with `body`.
fn config(body: &str) -> String {
    format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
  "routes": [{{"handler": {{"module": "respond", "params": {{"body": "{body}"}}}}}}]}}"#
    )
}

/// Starts `akit run -watch -watch-interval 20ms` on `config`, and waits for it to watch; its
/// listener's address.
fn start(config: &Path) -> (Akit, SocketAddr) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["run", "-watch", "-watch-interval", "20ms", "-c"])
        .arg(config)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (tx, out) = mpsc::channel();
    for r in [
        Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>,
        Box::new(child.stderr.take().unwrap()),
    ] {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(r).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
    }
    let akit = Akit { child, out };
    let line = akit.until("akit: listener").pop().unwrap();
    let addr = line.split(' ').nth(4).unwrap().parse().unwrap();
    akit.until(&format!("akit: watching {}", s(config)));
    (akit, addr)
}

impl Akit {
    /// Its output lines up to and including the first that starts with `last`.
    fn until(&self, last: &str) -> Vec<String> {
        let mut v = Vec::new();
        loop {
            let line = self
                .out
                .recv_timeout(Duration::from_secs(10))
                .unwrap_or_else(|e| panic!("waiting for {last:?} after {v:?}: {e}"));
            let done = line.starts_with(last);
            v.push(line);
            if done {
                return v;
            }
        }
    }

    /// Nothing said for `d`.
    fn quiet(&self, d: Duration) {
        if let Ok(line) = self.out.recv_timeout(d) {
            panic!("said {line:?}");
        }
    }

    fn stop(mut self) {
        kill(self.child.id(), SIGTERM).unwrap();
        self.until("akit: stopped");
        assert_eq!(self.child.wait().unwrap().code(), Some(0));
    }
}

/// `GET path`: the status code and the body.
fn get(addr: SocketAddr, path: &str) -> (u16, String) {
    let mut c = TcpStream::connect(addr).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        c,
        "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut resp = String::new();
    c.read_to_string(&mut resp).unwrap();
    let code = resp.get(9..12).unwrap().parse().unwrap();
    let body = resp.split_once("\r\n\r\n").unwrap().1.to_owned();
    (code, body)
}

/// Waits until `GET path` answers `code` and `body`.
fn wait_get(addr: SocketAddr, path: &str, code: u16, body: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = get(addr, path);
        if got == (code, body.to_owned()) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{path}: {got:?}, want {code} {body:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn each_edit_is_applied() {
    let d = dir("edits");
    let cfg = d.join("fit.json");
    let name = s(&cfg);
    write(&cfg, &config("one"));
    let (akit, addr) = start(&cfg);
    assert_eq!(get(addr, "/"), (200, "one".into()));

    write(&cfg, &config("two"));
    let lines = akit.until(&format!("akit: reloaded {name} (config version 2)"));
    assert!(
        lines.contains(&format!("akit: {name} changed; reloading")),
        "{lines:?}"
    );
    assert_eq!(get(addr, "/"), (200, "two".into()));

    // SIGHUP applies an edit too, and the watcher does not apply it again.
    write(&cfg, &config("three"));
    kill(akit.child.id(), SIGHUP).unwrap();
    let lines = akit.until(&format!("akit: reloaded {name} (config version 3)"));
    assert!(
        lines.contains(&format!("akit: reloading {name}")),
        "{lines:?}"
    );
    akit.quiet(Duration::from_millis(1000));
    assert_eq!(get(addr, "/"), (200, "three".into()));

    // Broken: reported, once, and "three" stays.
    write(
        &cfg,
        r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}], "routes": [{"handler": {"module": "nope"}}]}"#,
    );
    let lines = akit.until("akit: reload failed; the running config stays");
    assert!(
        lines.contains(&format!("akit: {name} changed; reloading")),
        "{lines:?}"
    );
    let err = akit.until(&name);
    assert!(err.last().unwrap().contains("nope"), "{err:?}");
    akit.quiet(Duration::from_millis(500));
    assert_eq!(get(addr, "/"), (200, "three".into()));

    // Fixed: applied.
    write(&cfg, &config("four"));
    akit.until(&format!("akit: reloaded {name} (config version 4)"));
    assert_eq!(get(addr, "/"), (200, "four".into()));

    // A file that cannot be read is reported, once, and the config stays; when it comes back,
    // it is applied.
    std::fs::remove_file(&cfg).unwrap();
    akit.until(&format!("akit: watching {name}: "));
    akit.quiet(Duration::from_millis(500));
    assert_eq!(get(addr, "/"), (200, "four".into()));
    write(&cfg, &config("five"));
    akit.until(&format!("akit: reloaded {name} (config version 5)"));
    assert_eq!(get(addr, "/"), (200, "five".into()));

    // Touched, its contents the same: nothing applied.
    std::fs::File::options()
        .write(true)
        .open(&cfg)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(3600))
        .unwrap();
    akit.quiet(Duration::from_millis(1000));
    akit.stop();
}

/// A directory of config files in fit's formats: a file added is served, one taken out is no
/// longer, and a broken one is reported by its name while the running config stays.
#[test]
fn a_directory_s_files() {
    let d = dir("directory");
    write(
        &d.join("00-base.conf"),
        "listener web {\n\taddress 127.0.0.1:0\n\tprotocols http/1.1 h2c\n}\n",
    );
    let a = d.join("routes/a.conf");
    write(
        &a,
        "route a {\n\tmatch { path /a }\n\thandler respond { body a }\n}\n",
    );
    let (akit, addr) = start(&d);
    wait_get(addr, "/a", 200, "a");

    write(
        &d.join("routes/b.yaml"),
        "routes:\n  - match: {path: [/b]}\n    handler: {module: respond, params: {body: b}}\n",
    );
    wait_get(addr, "/b", 200, "b");

    std::fs::remove_file(&a).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while get(addr, "/a").0 != 404 {
        assert!(Instant::now() < deadline, "/a is still served");
        std::thread::sleep(Duration::from_millis(20));
    }

    let broken = d.join("routes/c.conf");
    write(&broken, "route c {\n\thandler\n}\n");
    akit.until("akit: reload failed; the running config stays");
    let err = akit.until(&format!("{}:2:", s(&broken)));
    assert!(err.last().unwrap().contains("handler"), "{err:?}");
    assert_eq!(get(addr, "/b"), (200, "b".into()));
    akit.stop();
}
