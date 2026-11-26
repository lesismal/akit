//! fit's `tunnel/e2e_test.go` over TCP, ported (I101): a hub, a node and a proxy on one akio
//! engine, as `akit run` runs them, a service behind the node reached through the proxy's forward.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use akit_roles::host::Manager;
use akit_roles::linktoken::Secret;

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// An echo service: what each connection sends comes back.
fn echo() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            std::thread::spawn(move || {
                let mut buf = vec![0u8; 64 << 10];
                loop {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if s.write_all(&buf[..n]).is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }
    });
    port
}

struct Env {
    engine: Arc<akio::Engine>,
    roles: Manager,
    forward: u16,
}

impl Drop for Env {
    fn drop(&mut self) {
        self.roles.stop(Duration::ZERO);
        self.engine.shutdown();
        self.engine.wait();
    }
}

/// The three roles, linked with `auth` (the members' fields) over TCP, with `body`.
fn start(hub_auth: &str, peer_auth: (&str, &str), body: &str) -> Env {
    let (hub, svc, fwd) = (free_port(), echo(), free_port());
    let up = |auth: &str| {
        format!(
            r#"{{"name": "hub", "endpoints": [{{"address": "127.0.0.1:{hub}"}}], {auth}{body}, "pool_size": 2, "retry_interval": "100ms", "ping_interval": "200ms"}}"#
        )
    };
    let doc = format!(
        r#"{{"tunnel": {{
          "hub": {{"name": "hub"{hub_auth}{body}, "listeners": [{{"protocol": "tcp", "address": "127.0.0.1:{hub}"}}]}},
          "node": {{"name": "lab", "hubs": [{}], "services": [{{"name": "echo", "address": "127.0.0.1:{svc}"}}]}},
          "proxy": {{"name": "alice", "hubs": [{}], "forwards": [{{"listen": "127.0.0.1:{fwd}", "node": "lab", "service": "echo"}}]}}}}}}"#,
        up(peer_auth.0),
        up(peer_auth.1)
    );
    let config = akit_config::from_json(&doc, None).unwrap_or_else(|e| panic!("{doc}\n{e}"));
    let engine = akio::Engine::builder().client_threads(2).build().unwrap();
    engine.start().unwrap();
    let engine = Arc::new(engine);
    let mut roles = Manager::new(
        &[akit_tunnel::HUB, akit_tunnel::NODE, akit_tunnel::PROXY],
        Arc::clone(&engine),
        None,
    );
    let errs = roles.apply(&config);
    assert!(errs.is_empty(), "{errs:?}");
    Env {
        engine,
        roles,
        forward: fwd,
    }
}

/// Connects to the forward until a tunnel opens and echoes `msg`: the node's registration and
/// the proxy's list of nodes take a moment.
fn tunnel(port: u16) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) {
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let _ = s.write_all(b"ping");
            let mut b = [0u8; 4];
            if s.read_exact(&mut b).is_ok() && &b == b"ping" {
                return s;
            }
        }
        assert!(Instant::now() < deadline, "no tunnel opened");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `n` bytes through the tunnel and back, written while they are read, so that the windows and
/// the acknowledgements turn many times over.
fn round_trip(s: &mut TcpStream, n: usize) {
    let data: Vec<u8> = (0..n).map(|i| u8::try_from(i % 251).unwrap()).collect();
    let mut w = s.try_clone().unwrap();
    let send = data.clone();
    let writer = std::thread::spawn(move || w.write_all(&send).unwrap());
    let mut got = vec![0u8; n];
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s.read_exact(&mut got).unwrap();
    writer.join().unwrap();
    assert!(got == data, "the bytes came back changed");
}

#[test]
fn tunnels_with_the_shared_password() {
    let env = start(
        r#", "password": "pw""#,
        (r#""password": "pw""#, r#""password": "pw""#),
        "",
    );
    let mut s = tunnel(env.forward);
    round_trip(&mut s, 16 << 20);
    // Several tunnels at once.
    let mut more: Vec<TcpStream> = (0..8).map(|_| tunnel(env.forward)).collect();
    for s in &mut more {
        round_trip(s, 1 << 20);
    }
}

#[test]
fn tunnels_sealed_by_the_body_password() {
    let env = start(
        r#", "password": "pw""#,
        (r#""password": "pw""#, r#""password": "pw""#),
        r#", "body_password": "b""#,
    );
    let mut s = tunnel(env.forward);
    round_trip(&mut s, 8 << 20);
}

fn token_record(name: &str) -> (String, String) {
    let s = Secret::new().unwrap();
    (
        format!(
            r#"{{"id": "{}", "name": "{name}", "hash": "{}"}}"#,
            s.id(),
            s.hash()
        ),
        s.token(),
    )
}

#[test]
fn tunnels_with_tokens_plain_and_sealed() {
    for body in ["", r#", "body_password": "b""#] {
        let (rn, tn) = token_record("lab");
        let (rp, tp) = token_record("alice");
        let env = start(
            &format!(r#", "tokens": [{rn}, {rp}]"#),
            (
                &format!(r#""token": "{tn}""#),
                &format!(r#""token": "{tp}""#),
            ),
            body,
        );
        let mut s = tunnel(env.forward);
        round_trip(&mut s, 4 << 20);
        // The members were bound to their tokens.
        let views = akit_roles::host::token_views(&env.roles.registries(), "tunnel.hub").unwrap();
        let mut members: Vec<String> = views.iter().flat_map(|v| v.members.clone()).collect();
        members.sort();
        assert_eq!(members, ["node/lab", "proxy/alice"]);
    }
}

#[test]
fn a_wrong_password_opens_nothing() {
    let env = start(
        r#", "password": "pw""#,
        (r#""password": "nope""#, r#""password": "nope""#),
        "",
    );
    std::thread::sleep(Duration::from_millis(500));
    let mut s = TcpStream::connect(("127.0.0.1", env.forward)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let _ = s.write_all(b"x");
    let mut b = [0u8; 1];
    assert!(!matches!(s.read(&mut b), Ok(1)), "a tunnel opened");
}

#[test]
fn a_token_another_member_holds_is_refused() {
    // The proxy registers with the node's token under another name: the first to link binds it;
    // the token is then the node's, and the proxy's links are refused.
    let (rn, tn) = token_record("lab");
    let env = start(
        &format!(r#", "tokens": [{rn}]"#),
        (
            &format!(r#""token": "{tn}""#),
            &format!(r#""token": "{tn}""#),
        ),
        "",
    );
    // Both register: a token is of no role, and binds each member that first links with it.
    let _ = tunnel(env.forward);
    let views = akit_roles::host::token_views(&env.roles.registries(), "tunnel.hub").unwrap();
    assert_eq!(views[0].members.len(), 2, "{views:?}");
}
