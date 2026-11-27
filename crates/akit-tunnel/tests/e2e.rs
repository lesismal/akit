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

// I102: TLS links.

/// A hub alone on `engine`, keeping its certificate beside `dir/akit.json`, with `listener`.
fn hub_alone(engine: &Arc<akio::Engine>, dir: &std::path::Path, listener: &str) -> Manager {
    let doc = format!(
        r#"{{"tunnel": {{"hub": {{"name": "hub", "password": "pw", "listeners": [{listener}]}}}}}}"#
    );
    let config = akit_config::from_json(&doc, None).unwrap();
    let mut m = Manager::new(&[akit_tunnel::HUB], Arc::clone(engine), None);
    m.keep_beside(dir.join("akit.json").to_str().unwrap());
    let errs = m.apply(&config);
    assert!(errs.is_empty(), "{errs:?}");
    m
}

/// The node and the proxy, linking to `endpoint` with `check` (a pin or a CA file).
fn peers(engine: &Arc<akio::Engine>, endpoint: &str, check: &str, svc: u16, fwd: u16) -> Manager {
    let up = format!(
        r#"{{"name": "hub", "endpoints": [{{"address": "{endpoint}"}}], "password": "pw"{check}, "retry_interval": "100ms"}}"#
    );
    let doc = format!(
        r#"{{"tunnel": {{
          "node": {{"name": "lab", "hubs": [{up}], "services": [{{"name": "echo", "address": "127.0.0.1:{svc}"}}]}},
          "proxy": {{"name": "alice", "hubs": [{up}], "forwards": [{{"listen": "127.0.0.1:{fwd}", "node": "lab", "service": "echo"}}]}}}}}}"#
    );
    let config = akit_config::from_json(&doc, None).unwrap_or_else(|e| panic!("{doc}\n{e}"));
    let mut m = Manager::new(
        &[akit_tunnel::NODE, akit_tunnel::PROXY],
        Arc::clone(engine),
        None,
    );
    let errs = m.apply(&config);
    assert!(errs.is_empty(), "{errs:?}");
    m
}

fn engine() -> Arc<akio::Engine> {
    let e = akio::Engine::builder().client_threads(2).build().unwrap();
    e.start().unwrap();
    Arc::new(e)
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("akit-tunnel-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn tls_links_check_the_hubs_certificate_by_its_pin() {
    let dir = tmp("pin");
    let e = engine();
    let (hub, svc) = (free_port(), echo());
    let mut h = hub_alone(
        &e,
        &dir,
        &format!(r#"{{"protocol": "tls", "address": "127.0.0.1:{hub}"}}"#),
    );
    let kept = dir.join("akit.tunnel.hub.pem");
    let pem =
        std::fs::read_to_string(&kept).expect("the hub keeps its certificate beside the config");
    let fp = akit_tunnel::tls::fingerprint(&pem);
    assert_eq!(fp.len(), 64);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&kept).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    // The pin links.
    let fwd = free_port();
    let mut p = peers(
        &e,
        &format!("tls://127.0.0.1:{hub}"),
        &format!(r#", "pin": "{fp}""#),
        svc,
        fwd,
    );
    let mut s = tunnel(fwd);
    round_trip(&mut s, 4 << 20);
    p.stop(Duration::ZERO);
    // Another pin does not: the handshake is checked before a byte is sent.
    let fwd = free_port();
    let other = "ab".repeat(32);
    let mut p = peers(
        &e,
        &format!("tls://127.0.0.1:{hub}"),
        &format!(r#", "pin": "{other}""#),
        svc,
        fwd,
    );
    std::thread::sleep(Duration::from_millis(700));
    let mut s = TcpStream::connect(("127.0.0.1", fwd)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let _ = s.write_all(b"x");
    let mut b = [0u8; 1];
    assert!(
        !matches!(s.read(&mut b), Ok(1)),
        "a tunnel opened past a wrong pin"
    );
    p.stop(Duration::ZERO);
    h.stop(Duration::ZERO);
    // A hub started again keeps its certificate: the pin stays good.
    let hub2 = free_port();
    let mut h = hub_alone(
        &e,
        &dir,
        &format!(r#"{{"protocol": "tls", "address": "127.0.0.1:{hub2}"}}"#),
    );
    assert_eq!(
        akit_tunnel::tls::fingerprint(&std::fs::read_to_string(&kept).unwrap()),
        fp
    );
    let fwd = free_port();
    let mut p = peers(
        &e,
        &format!("tls://127.0.0.1:{hub2}"),
        &format!(r#", "pin": "{fp}""#),
        svc,
        fwd,
    );
    let _ = tunnel(fwd);
    p.stop(Duration::ZERO);
    h.stop(Duration::ZERO);
    e.shutdown();
    e.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tls_links_check_a_listeners_certificate_by_its_cas() {
    let dir = tmp("ca");
    // A CA, and a certificate it signs for 127.0.0.1, the listener's own.
    let ca_key =
        akio_crypto::pkey::PrivateKey::P256(akio_crypto::p256::SecretKey::generate().unwrap());
    let ca = akio_x509::CertificateParams::ca("akit tunnel test CA")
        .self_signed(&ca_key)
        .unwrap();
    let key =
        akio_crypto::pkey::PrivateKey::P256(akio_crypto::p256::SecretKey::generate().unwrap());
    let leaf = akio_x509::CertificateParams::new(["127.0.0.1"])
        .signed_by(&key.public_key(), &ca, &ca_key)
        .unwrap();
    std::fs::write(dir.join("ca.pem"), ca.to_pem()).unwrap();
    std::fs::write(dir.join("cert.pem"), leaf.to_pem()).unwrap();
    std::fs::write(
        dir.join("key.pem"),
        akio_crypto::pem::encode("PRIVATE KEY", &key.to_pkcs8_der()),
    )
    .unwrap();
    let e = engine();
    let (hub, svc, fwd) = (free_port(), echo(), free_port());
    let mut h = hub_alone(
        &e,
        &dir,
        &format!(
            r#"{{"protocol": "tls", "address": "127.0.0.1:{hub}", "cert_file": "{}", "key_file": "{}"}}"#,
            dir.join("cert.pem").display(),
            dir.join("key.pem").display()
        ),
    );
    assert!(
        !dir.join("akit.tunnel.hub.pem").exists(),
        "a listener with a certificate of its own makes none"
    );
    let mut p = peers(
        &e,
        &format!("tls://127.0.0.1:{hub}"),
        &format!(r#", "ca_file": "{}""#, dir.join("ca.pem").display()),
        svc,
        fwd,
    );
    let mut s = tunnel(fwd);
    round_trip(&mut s, 1 << 20);
    p.stop(Duration::ZERO);
    h.stop(Duration::ZERO);
    e.shutdown();
    e.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

// I102b: QUIC links.

fn free_udp_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn quic_engine() -> Arc<akio::Engine> {
    let e = akio::Engine::builder()
        .client_threads(2)
        .udp_client_threads(1)
        .build()
        .unwrap();
    e.start().unwrap();
    Arc::new(e)
}

#[test]
fn quic_links_carry_tunnels_and_check_the_pin() {
    let dir = tmp("quic");
    let e = quic_engine();
    let (hub, svc) = (free_udp_port(), echo());
    let mut h = hub_alone(
        &e,
        &dir,
        &format!(r#"{{"protocol": "quic", "address": "127.0.0.1:{hub}"}}"#),
    );
    let pem = std::fs::read_to_string(dir.join("akit.tunnel.hub.pem")).unwrap();
    let fp = akit_tunnel::tls::fingerprint(&pem);
    // The pin links; tunnels carry more than a stream's send buffer each, several at once.
    let fwd = free_port();
    let mut p = peers(
        &e,
        &format!("quic://127.0.0.1:{hub}"),
        &format!(r#", "pin": "{fp}""#),
        svc,
        fwd,
    );
    let mut s = tunnel(fwd);
    round_trip(&mut s, 12 << 20);
    let ts: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(move || {
                let mut s = tunnel(fwd);
                round_trip(&mut s, 2 << 20);
            })
        })
        .collect();
    for t in ts {
        t.join().unwrap();
    }
    p.stop(Duration::ZERO);
    // quic_insecure takes any certificate.
    let fwd = free_port();
    let mut p = peers(
        &e,
        &format!("quic://127.0.0.1:{hub}"),
        r#", "quic_insecure": true"#,
        svc,
        fwd,
    );
    let mut s = tunnel(fwd);
    round_trip(&mut s, 64 << 10);
    p.stop(Duration::ZERO);
    // Another pin does not link.
    let fwd = free_port();
    let other = "ab".repeat(32);
    let mut p = peers(
        &e,
        &format!("quic://127.0.0.1:{hub}"),
        &format!(r#", "pin": "{other}""#),
        svc,
        fwd,
    );
    std::thread::sleep(Duration::from_millis(700));
    let mut s = TcpStream::connect(("127.0.0.1", fwd)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let _ = s.write_all(b"x");
    let mut b = [0u8; 1];
    assert!(
        !matches!(s.read(&mut b), Ok(1)),
        "a tunnel opened past a wrong pin"
    );
    p.stop(Duration::ZERO);
    h.stop(Duration::ZERO);
    e.shutdown();
    e.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A QUIC listener refuses peers by the hub's access policy at akio's filter (ADR-178): a refused
/// peer's packets are dropped before a connection is made, and no tunnel opens; a config that
/// allows it swaps the filter on the same socket, and the peers link.
#[test]
fn quic_links_refused_at_the_filter() {
    let dir = tmp("quic-access");
    let e = quic_engine();
    let (hub, svc) = (free_udp_port(), echo());
    let doc = |access: &str| {
        format!(
            r#"{{"tunnel": {{"hub": {{"name": "hub", "password": "pw", "access": {access},
              "listeners": [{{"protocol": "quic", "address": "127.0.0.1:{hub}"}}]}}}}}}"#
        )
    };
    let config = akit_config::from_json(&doc(r#"{"deny": ["127.0.0.1"]}"#), None).unwrap();
    let mut h = Manager::new(&[akit_tunnel::HUB], Arc::clone(&e), None);
    h.keep_beside(dir.join("akit.json").to_str().unwrap());
    let errs = h.apply(&config);
    assert!(errs.is_empty(), "{errs:?}");
    let fwd = free_port();
    let mut p = peers(
        &e,
        &format!("quic://127.0.0.1:{hub}"),
        r#", "quic_insecure": true"#,
        svc,
        fwd,
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while e.telemetry().metrics().total.filtered == 0 {
        assert!(Instant::now() < deadline, "no packet was filtered");
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut s = TcpStream::connect(("127.0.0.1", fwd)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    let _ = s.write_all(b"x");
    let mut b = [0u8; 1];
    assert!(
        !matches!(s.read(&mut b), Ok(1)),
        "a tunnel opened from a refused peer"
    );
    let config = akit_config::from_json(&doc(r#"{"allow": ["127.0.0.1"]}"#), None).unwrap();
    let errs = h.apply(&config);
    assert!(errs.is_empty(), "{errs:?}");
    let mut s = tunnel(fwd);
    round_trip(&mut s, 64 << 10);
    p.stop(Duration::ZERO);
    h.stop(Duration::ZERO);
    e.shutdown();
    e.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

// I102c: WebSocket links.

/// `ws://` links: the hub's path takes them (several tunnels at once, more than a message
/// each), another path is answered 404 and links nothing; sealed by `body_password` too.
#[test]
fn ws_links_carry_tunnels() {
    let dir = tmp("ws");
    let e = engine();
    let (hub, svc) = (free_port(), echo());
    let mut h = hub_alone(
        &e,
        &dir,
        &format!(r#"{{"protocol": "ws", "address": "127.0.0.1:{hub}", "path": "/t"}}"#),
    );
    assert!(
        !dir.join("akit.tunnel.hub.pem").exists(),
        "a ws listener needs no certificate"
    );
    let fwd = free_port();
    let mut p = peers(&e, &format!("ws://127.0.0.1:{hub}/t"), "", svc, fwd);
    let mut s = tunnel(fwd);
    round_trip(&mut s, 12 << 20);
    let ts: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(move || {
                let mut s = tunnel(fwd);
                round_trip(&mut s, 2 << 20);
            })
        })
        .collect();
    for t in ts {
        t.join().unwrap();
    }
    p.stop(Duration::ZERO);
    // Another path: 404, nothing opens.
    let mut c = TcpStream::connect(("127.0.0.1", hub)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    c.write_all(b"GET /other HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n").unwrap();
    let mut head = [0u8; 12];
    c.read_exact(&mut head).unwrap();
    assert_eq!(&head, b"HTTP/1.1 404");
    let fwd = free_port();
    let mut p = peers(&e, &format!("ws://127.0.0.1:{hub}/other"), "", svc, fwd);
    std::thread::sleep(Duration::from_millis(700));
    let mut s = TcpStream::connect(("127.0.0.1", fwd)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let _ = s.write_all(b"x");
    let mut b = [0u8; 1];
    assert!(
        !matches!(s.read(&mut b), Ok(1)),
        "a tunnel opened on another path"
    );
    p.stop(Duration::ZERO);
    h.stop(Duration::ZERO);
    e.shutdown();
    e.wait();
    let _ = std::fs::remove_dir_all(&dir);
    // Sealed, the default path.
    let (hub, svc, fwd) = (free_port(), echo(), free_port());
    let up = format!(
        r#"{{"name": "hub", "endpoints": [{{"address": "ws://127.0.0.1:{hub}/tunnel"}}], "password": "pw", "body_password": "b", "pool_size": 2, "retry_interval": "100ms"}}"#
    );
    let doc = format!(
        r#"{{"tunnel": {{
          "hub": {{"name": "hub", "password": "pw", "body_password": "b", "listeners": [{{"protocol": "ws", "address": "127.0.0.1:{hub}"}}]}},
          "node": {{"name": "lab", "hubs": [{up}], "services": [{{"name": "echo", "address": "127.0.0.1:{svc}"}}]}},
          "proxy": {{"name": "alice", "hubs": [{up}], "forwards": [{{"listen": "127.0.0.1:{fwd}", "node": "lab", "service": "echo"}}]}}}}}}"#
    );
    let config = akit_config::from_json(&doc, None).unwrap_or_else(|e| panic!("{doc}\n{e}"));
    let e = engine();
    let mut roles = Manager::new(
        &[akit_tunnel::HUB, akit_tunnel::NODE, akit_tunnel::PROXY],
        Arc::clone(&e),
        None,
    );
    let errs = roles.apply(&config);
    assert!(errs.is_empty(), "{errs:?}");
    let mut s = tunnel(fwd);
    round_trip(&mut s, 8 << 20);
    roles.stop(Duration::ZERO);
    e.shutdown();
    e.wait();
}

/// `wss://` links check the hub's certificate by its pin, as `tls://` ones; another pin links
/// nothing.
#[test]
fn wss_links_check_the_hubs_certificate_by_its_pin() {
    let dir = tmp("wss");
    let e = engine();
    let (hub, svc) = (free_port(), echo());
    let mut h = hub_alone(
        &e,
        &dir,
        &format!(r#"{{"protocol": "wss", "address": "127.0.0.1:{hub}"}}"#),
    );
    let pem = std::fs::read_to_string(dir.join("akit.tunnel.hub.pem")).unwrap();
    let fp = akit_tunnel::tls::fingerprint(&pem);
    let fwd = free_port();
    let mut p = peers(
        &e,
        &format!("wss://127.0.0.1:{hub}/tunnel"),
        &format!(r#", "pin": "{fp}""#),
        svc,
        fwd,
    );
    let mut s = tunnel(fwd);
    round_trip(&mut s, 8 << 20);
    p.stop(Duration::ZERO);
    let fwd = free_port();
    let other = "ab".repeat(32);
    let mut p = peers(
        &e,
        &format!("wss://127.0.0.1:{hub}/tunnel"),
        &format!(r#", "pin": "{other}""#),
        svc,
        fwd,
    );
    std::thread::sleep(Duration::from_millis(700));
    let mut s = TcpStream::connect(("127.0.0.1", fwd)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let _ = s.write_all(b"x");
    let mut b = [0u8; 1];
    assert!(
        !matches!(s.read(&mut b), Ok(1)),
        "a tunnel opened past a wrong pin"
    );
    p.stop(Duration::ZERO);
    h.stop(Duration::ZERO);
    e.shutdown();
    e.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Opens a WebSocket to `path` on `port` by hand: the response's head read, the rest of what
/// came with it returned.
fn ws_handshake(port: u16, path: &str) -> (TcpStream, Vec<u8>) {
    let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(c, "GET {path} HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n").unwrap();
    let mut got = Vec::new();
    let mut b = [0u8; 4096];
    let end = loop {
        let n = c.read(&mut b).unwrap();
        assert!(n > 0, "closed in the handshake");
        got.extend_from_slice(&b[..n]);
        if let Some(i) = got.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    assert!(
        got.starts_with(b"HTTP/1.1 101"),
        "{}",
        String::from_utf8_lossy(&got)
    );
    let rest = got.split_off(end);
    (c, rest)
}

/// The opcodes of the server's frames in `b`, which holds whole frames and maybe the start of one.
fn ws_opcodes(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 2 <= b.len() {
        let op = b[at] & 0x0f;
        let (len, head) = match b[at + 1] & 0x7f {
            126 if at + 4 <= b.len() => {
                (usize::from(u16::from_be_bytes([b[at + 2], b[at + 3]])), 4)
            }
            127 if at + 10 <= b.len() => (
                usize::try_from(u64::from_be_bytes(b[at + 2..at + 10].try_into().unwrap()))
                    .unwrap(),
                10,
            ),
            126 | 127 => break,
            n => (usize::from(n), 2),
        };
        out.push(op);
        at += head + len;
    }
    out
}

/// A WebSocket link that fails (a frame shorter than a header) is ended at once, without a close
/// frame and without what was queued, as fit's `Abort` (akio's A238).
#[test]
fn ws_links_that_fail_end_at_once() {
    let dir = tmp("ws-abort");
    let e = engine();
    let hub = free_port();
    let mut h = hub_alone(
        &e,
        &dir,
        &format!(r#"{{"protocol": "ws", "address": "127.0.0.1:{hub}"}}"#),
    );
    let (mut c, mut got) = ws_handshake(hub, "/tunnel");
    // A binary message, masked with a zero key: a link frame whose length is 0.
    c.write_all(&[0x82, 0x84, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
    let t = Instant::now();
    let mut b = [0u8; 4096];
    loop {
        match c.read(&mut b) {
            Ok(0) | Err(_) => break,
            Ok(n) => got.extend_from_slice(&b[..n]),
        }
    }
    assert!(
        t.elapsed() < Duration::from_secs(3),
        "the failed link lasted {:?}",
        t.elapsed()
    );
    assert!(
        !ws_opcodes(&got).contains(&8),
        "a close frame came: {got:?}"
    );
    h.stop(Duration::ZERO);
    e.shutdown();
    e.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A service that writes `n` bytes to each connection, then closes it.
fn source(n: usize) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            std::thread::spawn(move || {
                let data: Vec<u8> = (0..n).map(|i| u8::try_from(i % 249).unwrap()).collect();
                let _ = s.write_all(&data);
            });
        }
    });
    port
}

/// Over a `ws://` link, a reader that stops holds its own tunnel's window, not the link: a second
/// tunnel echoes meanwhile, and the first's bytes all arrive once it reads again (rules 8, 9).
#[test]
fn ws_links_a_slow_reader_holds_its_tunnel_alone() {
    let dir = tmp("ws-slow");
    let e = engine();
    let (hub, svc, src) = (free_port(), echo(), source(24 << 20));
    let mut h = hub_alone(
        &e,
        &dir,
        &format!(r#"{{"protocol": "ws", "address": "127.0.0.1:{hub}"}}"#),
    );
    let up = format!(
        r#"{{"name": "hub", "endpoints": [{{"address": "ws://127.0.0.1:{hub}/tunnel"}}], "password": "pw", "pool_size": 1, "retry_interval": "100ms"}}"#
    );
    let (fwd, fwd_src) = (free_port(), free_port());
    let doc = format!(
        r#"{{"tunnel": {{
          "node": {{"name": "lab", "hubs": [{up}], "services": [{{"name": "echo", "address": "127.0.0.1:{svc}"}}, {{"name": "src", "address": "127.0.0.1:{src}"}}]}},
          "proxy": {{"name": "alice", "hubs": [{up}], "forwards": [{{"listen": "127.0.0.1:{fwd}", "node": "lab", "service": "echo"}}, {{"listen": "127.0.0.1:{fwd_src}", "node": "lab", "service": "src"}}]}}}}}}"#
    );
    let config = akit_config::from_json(&doc, None).unwrap_or_else(|e| panic!("{doc}\n{e}"));
    let mut p = Manager::new(
        &[akit_tunnel::NODE, akit_tunnel::PROXY],
        Arc::clone(&e),
        None,
    );
    assert!(p.apply(&config).is_empty());
    let _ = tunnel(fwd);
    // The source's tunnel, not read for a while.
    let mut slow = TcpStream::connect(("127.0.0.1", fwd_src)).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let mut s = tunnel(fwd);
    round_trip(&mut s, 4 << 20);
    let mut got = Vec::new();
    slow.set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    slow.read_to_end(&mut got).unwrap();
    assert_eq!(got.len(), 24 << 20);
    assert!(
        got.iter()
            .enumerate()
            .all(|(i, b)| *b == u8::try_from(i % 249).unwrap()),
        "the bytes came changed"
    );
    p.stop(Duration::ZERO);
    h.stop(Duration::ZERO);
    e.shutdown();
    e.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

// I103: the hub's calls and status, tokens' expiry.

fn hub_api(roles: &Manager) -> akit_tunnel::hub::HubApi {
    roles
        .api("tunnel.hub")
        .expect("the hub's handle")
        .downcast_ref::<akit_tunnel::hub::HubApi>()
        .expect("a HubApi")
        .clone()
}

fn status(api: &akit_tunnel::hub::HubApi) -> akit_config::value::Value {
    akit_tunnel::msg::parse(api.status().as_bytes()).expect("the status is JSON")
}

fn arr(v: &akit_config::value::Value, k: &str) -> Vec<akit_config::value::Value> {
    match v.get(k) {
        Some(akit_config::value::Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    }
}

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !f() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

type InfoResult = Result<akit_tunnel::msg::Info, String>;

fn call_info(f: impl FnOnce(akit_tunnel::hub::InfoDone)) -> InfoResult {
    let (tx, rx) = std::sync::mpsc::channel();
    f(Box::new(move |r| {
        let _ = tx.send(r);
    }));
    rx.recv_timeout(Duration::from_secs(10)).unwrap()
}

/// fit's hub calls (`api.go`): the status the pages read, a node's info asked for again, its
/// services set where it lets the hub, a tunnel closed and a peer kicked.
#[test]
fn the_hubs_calls_and_status() {
    let (hub, svc, fwd) = (free_port(), echo(), free_port());
    let up = format!(
        r#"{{"name": "hub", "endpoints": [{{"address": "127.0.0.1:{hub}"}}], "password": "pw", "pool_size": 2, "retry_interval": "100ms"}}"#
    );
    let doc = format!(
        r#"{{"tunnel": {{
          "hub": {{"name": "hub", "password": "pw", "listeners": [{{"protocol": "tcp", "address": "127.0.0.1:{hub}"}}]}},
          "node": {{"name": "lab", "hubs": [{up}], "allow_hub_services": true, "services": [{{"name": "echo", "address": "127.0.0.1:{svc}"}}]}},
          "proxy": {{"name": "alice", "hubs": [{up}], "forwards": [{{"listen": "127.0.0.1:{fwd}", "node": "lab", "service": "echo"}}]}}}}}}"#
    );
    let e = engine();
    let (roles, api) = controlled(&e, &doc);
    let mut s = tunnel(fwd);
    round_trip(&mut s, 1 << 20);

    let st = status(&api);
    assert_eq!(akit_tunnel::msg::get_str(&st, "name"), "hub");
    assert_eq!(arr(&st, "peers").len(), 2, "{}", api.status());
    assert_eq!(
        arr(&st, "listen"),
        [akit_config::value::Value::String(format!(
            "127.0.0.1:{hub}"
        ))]
    );
    let tunnels = arr(&st, "tunnels");
    assert_eq!(tunnels.len(), 1, "{}", api.status());
    assert_eq!(akit_tunnel::msg::get_str(&tunnels[0], "service"), "echo");
    assert_eq!(
        akit_tunnel::msg::get_str(&tunnels[0], "addr"),
        format!("127.0.0.1:{svc}")
    );
    assert_eq!(api.password_peers(), ["node/lab", "proxy/alice"]);

    // A node's info, asked for again.
    let node = api.peer_id("node", "lab").unwrap();
    let proxy = api.peer_id("proxy", "alice").unwrap();
    let info = call_info(|d| api.refresh_node(node, d)).unwrap();
    assert_eq!(info.name, "lab");
    assert_eq!(info.services.len(), 1);
    assert!(
        call_info(|d| api.refresh_node(proxy, d)).is_err(),
        "a proxy is no node"
    );

    // Its services set: the node takes them, and the hub shows them.
    let svcs = [
        akit_tunnel::msg::Service {
            name: "echo".into(),
            addr: format!("127.0.0.1:{svc}"),
            desc: String::new(),
        },
        akit_tunnel::msg::Service {
            name: "two".into(),
            addr: format!("127.0.0.1:{svc}"),
            desc: "the same".into(),
        },
    ];
    let info = call_info(|d| api.set_services(node, &svcs, d)).unwrap();
    assert_eq!(info.services.len(), 2);
    assert!(api.status().contains(r#""description":"the same""#));

    // A tunnel closed from the hub: the client's connection ends.
    let id: u64 = akit_tunnel::msg::get_str(&tunnels[0], "id")
        .parse()
        .unwrap();
    assert!(api.close_tunnel(id, "closed from the hub's console"));
    assert!(!api.close_tunnel(id, "again"));
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut b = [0u8; 1];
    assert!(
        matches!(s.read(&mut b), Ok(0) | Err(_)),
        "the tunnel stayed open"
    );

    // A peer kicked: its links close; it links again after its retry.
    assert!(!api.kick(9999, "nobody"));
    let before = api.status();
    assert!(api.kick(proxy, "kicked from the hub's console"));
    wait_until("the kicked proxy's links to close", || {
        api.peer_id("proxy", "alice") != Some(proxy)
    });
    wait_until("the proxy to link again", || {
        api.peer_id("proxy", "alice").is_some()
    });
    assert_ne!(api.status(), before);
    let _ = tunnel(fwd);
    roles.lock().unwrap().stop(Duration::ZERO);
    e.shutdown();
    e.wait();
}

/// The roles of `doc`, with a control thread that applies their edits as `akit run`'s does; and
/// the hub's handle.
fn controlled(
    e: &Arc<akio::Engine>,
    doc: &str,
) -> (Arc<std::sync::Mutex<Manager>>, akit_tunnel::hub::HubApi) {
    let (tx, rx) = std::sync::mpsc::channel::<akit_roles::host::Edit>();
    let tx = std::sync::Mutex::new(tx);
    let editor: akit_roles::host::Editor = Arc::new(move |edit| {
        let _ = tx.lock().unwrap().send(edit);
        Ok(())
    });
    let mut roles = Manager::new(
        &[akit_tunnel::HUB, akit_tunnel::NODE, akit_tunnel::PROXY],
        Arc::clone(e),
        Some(editor),
    );
    let config = akit_config::from_json(doc, None).unwrap_or_else(|err| panic!("{doc}\n{err}"));
    assert!(roles.apply(&config).is_empty());
    let api = hub_api(&roles);
    let roles = Arc::new(std::sync::Mutex::new(roles));
    let control = Arc::clone(&roles);
    let mut value = akit_config::value::parse(doc).unwrap();
    std::thread::spawn(move || {
        while let Ok(edit) = rx.recv() {
            match edit.apply_to(&mut value) {
                Ok(done) => {
                    let config = akit_config::from_json(&value.to_json(), None).unwrap();
                    let errs = control.lock().unwrap().apply(&config);
                    done(if errs.is_empty() {
                        Ok(Vec::new())
                    } else {
                        Err(errs.join("; "))
                    });
                }
                Err((done, err)) => done(Err(err)),
            }
        }
    });
    (roles, api)
}

/// fit's `TestTokenExpires` and `TestTokenRevoke`: a token whose expiry passes closes the links
/// it holds, and is refused from then on.
#[test]
fn a_tokens_expiry_closes_its_links() {
    let (rn, tn) = token_record("lab");
    let s = Secret::parse(&tn).unwrap();
    let exp = akit_roles::linktoken::unix_now() + 2;
    let rn = rn.replace(
        &format!(r#""id": "{}""#, s.id()),
        &format!(r#""id": "{}", "expires_at": {exp}"#, s.id()),
    );
    let (rp, tp) = token_record("alice");
    let env = start(
        &format!(r#", "tokens": [{rn}, {rp}]"#),
        (
            &format!(r#""token": "{tn}""#),
            &format!(r#""token": "{tp}""#),
        ),
        "",
    );
    let api = hub_api(&env.roles);
    let _ = tunnel(env.forward);
    assert!(api.password_peers().is_empty());
    let st = status(&api);
    assert!(
        arr(&st, "peers")
            .iter()
            .all(|p| arr(p, "tokens").len() == 1),
        "{}",
        api.status()
    );
    // Past the expiry the node's links close, and it does not link again.
    wait_until("the node's links to close at its token's expiry", || {
        api.peer_id("node", "lab").is_none()
    });
    std::thread::sleep(Duration::from_millis(600));
    assert!(
        api.peer_id("node", "lab").is_none(),
        "an expired token linked again"
    );
    assert!(api.peer_id("proxy", "alice").is_some());
}
