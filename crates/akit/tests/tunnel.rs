//! The tunnel through `akit run` (I101) and `akit token` (I100): one akit runs the Admin API and a
//! tunnel hub; `akit token create` issues a token for the node and one for the proxy through the
//! Admin API; a second akit runs the node and the proxy with them; a connection to the proxy's
//! forward reaches a service behind the node. `akit token list` shows the members bound, and
//! `akit token revoke` closes the proxy's links.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn free() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

/// An echo service.
fn echo() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let a = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            std::thread::spawn(move || {
                let mut b = [0u8; 16 << 10];
                while let Ok(n) = s.read(&mut b) {
                    if n == 0 || s.write_all(&b[..n]).is_err() {
                        break;
                    }
                }
            });
        }
    });
    a
}

struct Akit(Child);

impl Drop for Akit {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `akit run -c path`, once it prints `ready`.
fn run(path: &Path, ready: &str) -> Akit {
    let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["run", "-shutdown-timeout", "2s", "-c"])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let out = BufReader::new(child.stdout.take().unwrap());
    let ready = ready.to_owned();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in out.lines() {
            let Ok(line) = line else { break };
            if line.starts_with(&ready) {
                let _ = tx.send(());
            }
        }
    });
    rx.recv_timeout(Duration::from_secs(20))
        .expect("akit run did not start");
    Akit(child)
}

/// `akit token <args>`: its exit status and standard output.
fn token(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_akit"))
        .arg("token")
        .args(args)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// Connects through the forward until a tunnel echoes.
fn through(fwd: SocketAddr, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if let Ok(mut s) = TcpStream::connect(fwd) {
            s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let data = vec![9u8; 256 << 10];
            let mut w = s.try_clone().unwrap();
            let d = data.clone();
            std::thread::spawn(move || {
                let _ = w.write_all(&d);
            });
            let mut got = vec![0u8; data.len()];
            if s.read_exact(&mut got).is_ok() && got == data {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

#[test]
fn a_tunnel_through_akit_run_with_tokens_from_akit_token() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("tunnel-e2e");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (api, hub, fwd, svc) = (free(), free(), free(), echo());
    let hub_path = dir.join("hub.json");
    std::fs::write(
        &hub_path,
        format!(
            r#"{{"admin": {{"address": "{api}"}},
  "tunnel": {{"hub": {{"name": "hub", "listeners": [{{"protocol": "tcp", "address": "{hub}"}}]}}}}}}"#
        ),
    )
    .unwrap();
    let _hub = run(&hub_path, "akit: admin API on");
    let c = hub_path.to_str().unwrap();
    let (ok, node_token) = token(&["create", "-c", c, "-role", "tunnel.hub", "-name", "lab"]);
    assert!(ok && node_token.starts_with("akitt1."), "{node_token}");
    let (ok, proxy_token) = token(&[
        "create",
        "-c",
        c,
        "-role",
        "tunnel.hub",
        "-name",
        "alice",
        "-ttl",
        "30d",
    ]);
    assert!(ok, "{proxy_token}");
    let (node_token, proxy_token) = (node_token.trim(), proxy_token.trim());

    let peers_path = dir.join("peers.json");
    let up = |tok: &str| {
        format!(
            r#"{{"name": "hub", "endpoints": [{{"address": "{hub}"}}], "token": "{tok}", "retry_interval": "200ms", "pool_size": 2}}"#
        )
    };
    std::fs::write(
        &peers_path,
        format!(
            r#"{{"tunnel": {{
  "node": {{"name": "lab", "hubs": [{}], "services": [{{"name": "echo", "address": "{svc}"}}]}},
  "proxy": {{"name": "alice", "hubs": [{}], "forwards": [{{"listen": "{fwd}", "node": "lab", "service": "echo"}}]}}}}}}"#,
            up(node_token),
            up(proxy_token)
        ),
    )
    .unwrap();
    let _peers = run(&peers_path, "akit: running tunnel.proxy");
    assert!(
        through(fwd, Duration::from_secs(20)),
        "no tunnel through the hub"
    );
    // The members are bound to their tokens, and listed with their links.
    let (ok, list) = token(&["list", "-c", c, "-role", "tunnel.hub"]);
    assert!(ok, "{list}");
    assert!(
        list.contains("members node/lab") && list.contains("members proxy/alice"),
        "{list}"
    );
    // Revoked: the proxy's links close, and no tunnel opens again.
    let (ok, json) = token(&["list", "-c", c, "-role", "tunnel.hub", "-json"]);
    assert!(ok);
    let v = akit_config::value::parse(&json).unwrap();
    let akit_config::value::Value::Array(tokens) = v.get("tokens").unwrap() else {
        panic!("{json}")
    };
    let alice = tokens
        .iter()
        .find(|t| t.get("name") == Some(&akit_config::value::Value::String("alice".into())))
        .unwrap();
    let akit_config::value::Value::String(id) = alice.get("id").unwrap() else {
        panic!()
    };
    let (ok, out) = token(&["revoke", "-c", c, "-role", "tunnel.hub", "-id", id]);
    assert!(ok, "{out}");
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        !through(fwd, Duration::from_secs(2)),
        "a tunnel opened with a revoked token"
    );
}
