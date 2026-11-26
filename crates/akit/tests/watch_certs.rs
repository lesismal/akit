//! End to end: `akit run` watches the certificate files its config loaded (`-watch-certs`, on by
//! default, as fit's) and applies the config again when they change (I30): the listener is kept
//! and its next handshakes get the new certificate; a key that does not match its certificate
//! leaves the running one until it is fixed; `-watch-certs=false` turns it off. The Admin API's
//! certificate files are watched too, and served to its new connections without a restart (I94).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod support;

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use support::certs::{Ca, dir};
use support::https::{client, get_h1, leaf_der};

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

fn s(p: &Path) -> String {
    p.display().to_string()
}

fn config(d: &Path, a: &(PathBuf, PathBuf)) -> PathBuf {
    let json = format!(
        r#"{{"listeners": [{{"name": "https", "address": "127.0.0.1:0", "protocols": ["http/1.1"],
              "tls": {{"certificates": [{{"cert_file": "{}", "key_file": "{}"}}]}}}}],
            "routes": [{{"handler": {{"module": "respond", "params": {{"body": "hello"}}}}}}]}}"#,
        s(&a.0),
        s(&a.1)
    );
    let p = d.join("fit.json");
    std::fs::write(&p, json).unwrap();
    p
}

/// Starts `akit run` on `config` with `flags`; its listener's address.
fn start(config: &Path, flags: &[&str]) -> (Akit, SocketAddr) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
        .arg("run")
        .args(flags)
        .arg("-c")
        .arg(config)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (tx, out) = mpsc::channel();
    for r in [
        Box::new(child.stdout.take().unwrap()) as Box<dyn std::io::Read + Send>,
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
}

/// The certificate a full handshake gets.
fn leaf(addr: SocketAddr, roots: &Path) -> Vec<u8> {
    let seen = get_h1(
        addr,
        &client(roots, &["http/1.1"], None),
        "a.example",
        "a.example",
        "/",
    )
    .unwrap();
    assert_eq!(seen.body, "hello");
    seen.leaf
}

/// Waits until a full handshake gets `want`.
fn wait_leaf(addr: SocketAddr, roots: &Path, want: &[u8]) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while leaf(addr, roots) != want {
        assert!(
            Instant::now() < deadline,
            "the certificate was not replaced"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn renewed_files_are_served_without_a_restart() {
    let d = dir("e2e-watch-certs");
    let ca = Ca::new("akit e2e ca");
    let roots = ca.write(&d, "ca.pem");
    let a = ca.leaf(&d, "a", &["a.example"]);
    let (akit, addr) = start(&config(&d, &a), &["-watch-interval", "50ms"]);
    assert_eq!(leaf(addr, &roots), leaf_der(&a.0));

    // Renewed in place: applied again once the files settle, the listener kept.
    ca.leaf(&d, "a", &["a.example"]);
    let lines = akit.until("akit: reloaded");
    assert_eq!(
        lines[0], "akit: certificate files changed; reloading",
        "{lines:?}"
    );
    assert!(
        !lines
            .iter()
            .any(|l| l.contains("removed") || l.contains("listener")),
        "{lines:?}"
    );
    wait_leaf(addr, &roots, &leaf_der(&a.0));

    // A certificate whose key is not yet written: the reload fails, once, and the running
    // certificate stays; once the key is written, applied.
    let served = leaf_der(&a.0);
    let next = d.join("next");
    std::fs::create_dir_all(&next).unwrap();
    let (c, k) = ca.leaf(&next, "a", &["a.example"]);
    std::fs::copy(&c, &a.0).unwrap();
    let lines = akit.until("akit: reload failed");
    assert_eq!(
        lines[0], "akit: certificate files changed; reloading",
        "{lines:?}"
    );
    // Why, for the running config applied again.
    let why = akit.until("the running config: ").pop().unwrap();
    assert!(why.contains("does not match"), "{why}");
    assert_eq!(leaf(addr, &roots), served);
    akit.quiet(Duration::from_millis(1000));
    std::fs::copy(&k, &a.1).unwrap();
    akit.until("akit: reloaded");
    wait_leaf(addr, &roots, &leaf_der(&c));
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn watch_certs_false_leaves_the_files_alone() {
    let d = dir("e2e-watch-certs-off");
    let ca = Ca::new("akit e2e ca");
    let roots = ca.write(&d, "ca.pem");
    let a = ca.leaf(&d, "a", &["a.example"]);
    let first = leaf_der(&a.0);
    let (akit, addr) = start(
        &config(&d, &a),
        &["-watch-certs=false", "-watch-interval", "50ms"],
    );
    ca.leaf(&d, "a", &["a.example"]);
    akit.quiet(Duration::from_millis(1000));
    assert_eq!(leaf(addr, &roots), first);
    std::fs::remove_dir_all(&d).unwrap();
}

/// Waits until a full handshake with the Admin API at `addr` gets `want`.
fn wait_admin_leaf(addr: SocketAddr, roots: &Path, want: &[u8]) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let seen = get_h1(
            addr,
            &client(roots, &["http/1.1"], None),
            "admin.example",
            // The loopback API without a token hears only its own host names.
            "localhost",
            "/versions",
        )
        .unwrap();
        assert!(seen.status.contains(" 200 "), "{}", seen.status);
        if seen.leaf == want {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the Admin API's certificate was not replaced"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn admin_certificate_renewed_without_a_restart() {
    let d = dir("e2e-watch-certs-admin");
    let ca = Ca::new("akit e2e ca");
    let roots = ca.write(&d, "ca.pem");
    let a = ca.leaf(&d, "a", &["a.example"]);
    let admin = ca.leaf(&d, "admin", &["admin.example"]);
    let p = config(&d, &a);
    let json = std::fs::read_to_string(&p).unwrap().replacen(
        '{',
        &format!(
            r#"{{"admin": {{"address": "127.0.0.1:0", "cert_file": "{}", "key_file": "{}"}},"#,
            s(&admin.0),
            s(&admin.1)
        ),
        1,
    );
    std::fs::write(&p, json).unwrap();
    let (akit, _) = start(&p, &["-watch-interval", "50ms"]);
    let line = akit.until("akit: admin API on ").pop().unwrap();
    let api: SocketAddr = line.rsplit(' ').next().unwrap().parse().unwrap();
    wait_admin_leaf(api, &roots, &leaf_der(&admin.0));

    // Renewed in place: served to new connections once the files settle.
    ca.leaf(&d, "admin", &["admin.example"]);
    let renewed = leaf_der(&admin.0);
    wait_admin_leaf(api, &roots, &renewed);

    // A certificate whose key is not yet written: the running one stays; once the key is
    // written, served.
    let next = d.join("next");
    std::fs::create_dir_all(&next).unwrap();
    let (c, k) = ca.leaf(&next, "admin", &["admin.example"]);
    std::fs::copy(&c, &admin.0).unwrap();
    std::thread::sleep(Duration::from_millis(1000));
    wait_admin_leaf(api, &roots, &renewed);
    std::fs::copy(&k, &admin.1).unwrap();
    wait_admin_leaf(api, &roots, &leaf_der(&c));
    drop(akit);
    std::fs::remove_dir_all(&d).unwrap();
}
