//! akit's TURN server against fib's TURN client, fit's own (I111): `tests/interop/fib-turn`, a
//! Go program on fib's `webrtc/turn` client, allocates over UDP, TCP and TLS with a password, a
//! key and the secret's credentials, and relays to an echoing peer by indication and by channel;
//! wrong credentials and a quota are refused with fit's codes. With `AKIT_FIT_BIN` too, the same
//! runs against fit's own TURN server, and the two must answer alike.
//!
//! Needs the probe's binary (`cd tests/interop/fib-turn && go build -o /tmp/fib-turn .`):
//! `AKIT_FIB_TURN=/tmp/fib-turn [AKIT_FIT_BIN=/path/to/fit] cargo test -p akit-webrtc --test
//! fib_turn -- --ignored`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::net::{SocketAddr, UdpSocket};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use akit_roles::host::Manager;

const REALM: &str = "example.com";
const SECRET: &str = "a-shared-secret";

/// A peer that answers each datagram with `echo:` and the datagram.
fn echo() -> SocketAddr {
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = s.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut b = [0u8; 4096];
        while let Ok((n, from)) = s.recv_from(&mut b) {
            let mut out = b"echo:".to_vec();
            out.extend_from_slice(&b[..n]);
            let _ = s.send_to(&out, from);
        }
    });
    addr
}

/// The member both servers run: UDP, TCP and TLS on the ports given (0: any), the peer allowed.
fn member(udp: u16, tcp: u16, tls: u16, cert: &str, key: &str, peer: SocketAddr) -> String {
    let bob: String = akio_turn::stun::long_term_key("bob", REALM, "bobpw")
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!(
        r#"{{"listen": ["127.0.0.1:{udp}"], "listen_tcp": ["127.0.0.1:{tcp}"], "listen_tls": ["127.0.0.1:{tls}"],
            "tls": {{"certificates": [{{"cert_file": "{cert}", "key_file": "{key}"}}]}},
            "realm": "{REALM}", "secret": "{SECRET}", "relay_ip": "127.0.0.1",
            "users": [{{"name": "alice", "password": "pw"}}, {{"name": "bob", "key": "{bob}"}}, {{"name": "q", "password": "pw"}}],
            "max_allocations_per_user": 4, "allowed_peers": ["{peer}"]}}"#
    )
}

/// What the probe printed, but the relayed address; and its exit code.
fn probe(args: &[&str]) -> (String, i32) {
    let bin = std::env::var("AKIT_FIB_TURN").expect("AKIT_FIB_TURN");
    let out = Command::new(bin).args(args).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.starts_with("relayed "))
        .collect::<Vec<_>>()
        .join("\n");
    (text, out.status.code().unwrap_or(-1))
}

/// The scenarios, each the probe's arguments, against the server at `udp`, `tcp` and `tls`.
fn scenarios(udp: u16, tcp: u16, tls: u16, peer: SocketAddr) -> Vec<(String, (String, i32))> {
    let peer = peer.to_string();
    let mut out = Vec::new();
    let mut run = |name: String, args: Vec<String>| {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        out.push((name, probe(&a)));
    };
    for (transport, port) in [("udp", udp), ("tcp", tcp), ("tls", tls)] {
        let server = format!("127.0.0.1:{port}");
        for (user, pass, secret) in [
            ("alice", "pw", ""),
            ("bob", "bobpw", ""),
            ("carol", "", SECRET),
        ] {
            for bind in [false, true] {
                let mut args = vec![
                    "-server".into(),
                    server.clone(),
                    "-transport".into(),
                    transport.into(),
                    "-user".into(),
                    user.into(),
                    "-pass".into(),
                    pass.into(),
                    "-peer".into(),
                    peer.clone(),
                ];
                if !secret.is_empty() {
                    args.extend(["-secret".into(), secret.into()]);
                }
                if bind {
                    args.push("-bind".into());
                }
                run(format!("{transport} {user} bind={bind}"), args);
            }
        }
        run(
            format!("{transport} wrong password"),
            [
                "-server",
                &server,
                "-transport",
                transport,
                "-user",
                "alice",
                "-pass",
                "nope",
            ]
            .map(String::from)
            .to_vec(),
        );
        run(
            format!("{transport} another secret"),
            [
                "-server",
                &server,
                "-transport",
                transport,
                "-user",
                "carol",
                "-secret",
                "not-the-secret",
            ]
            .map(String::from)
            .to_vec(),
        );
    }
    out
}

fn tls_files(dir: &std::path::Path) -> (String, String) {
    let key = akio_crypto::p256::SecretKey::generate().unwrap();
    let key = akio_crypto::pkey::PrivateKey::P256(key);
    let mut p = akio_x509::CertificateParams::new(vec!["localhost"]);
    p.not_before = 1_700_000_000;
    p.not_after = 4_000_000_000;
    let cert = p.self_signed(&key).unwrap();
    let (cf, kf) = (dir.join("c.pem"), dir.join("k.pem"));
    std::fs::write(&cf, cert.to_pem()).unwrap();
    std::fs::write(
        &kf,
        akio_crypto::pem::encode("PRIVATE KEY", &key.to_pkcs8_der()),
    )
    .unwrap();
    (cf.display().to_string(), kf.display().to_string())
}

#[path = "../../akit-tunnel/tests/support/fit.rs"]
mod fit;

#[test]
#[ignore = "needs AKIT_FIB_TURN, the probe's binary"]
fn fib_client_against_akit_and_fit() {
    let dir = std::env::temp_dir().join(format!("akit-webrtc-fib-turn-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (cert, key) = tls_files(&dir);
    let peer = echo();

    // akit's server, on an engine of its own, as akit run runs it, on port 0: where it listens
    // is asked of the engine, UDP, then TCP and TLS in the member's order.
    let engine = akio::Engine::builder()
        .client_threads(1)
        .udp_client_threads(2)
        .build()
        .unwrap();
    engine.start().unwrap();
    let engine = Arc::new(engine);
    let mut roles = Manager::new(&[akit_webrtc::TURN], Arc::clone(&engine), None);
    let doc = format!(
        r#"{{"webrtc": {{"turn": {}}}}}"#,
        member(0, 0, 0, &cert, &key, peer)
    );
    let errs = roles.apply(&akit_config::from_json(&doc, None).unwrap());
    assert!(errs.is_empty(), "{errs:?}");
    let ls = engine.listeners();
    let udp = ls.iter().find(|l| l.udp).unwrap().local_addr.port();
    let mut streams = ls.iter().filter(|l| !l.udp).map(|l| l.local_addr.port());
    let (tcp, tls) = (streams.next().unwrap(), streams.next().unwrap());
    let akit = scenarios(udp, tcp, tls, peer);
    for (name, (out, code)) in &akit {
        let refused = name.contains("wrong") || name.contains("another");
        if refused {
            assert_eq!((out.as_str(), *code), ("error 401", 2), "akit: {name}");
        } else {
            assert_eq!(
                (out.as_str(), *code),
                ("echo 0\necho 1\necho 2\nok", 0),
                "akit: {name}"
            );
        }
    }
    roles.stop(Duration::ZERO);
    engine.shutdown();
    engine.wait();

    // fit's, with the same member, answering alike.
    if let Ok(bin) = std::env::var("AKIT_FIT_BIN") {
        // fit's listeners on port 0, where they listen read from the line it logs.
        let run = fit::start(
            &bin,
            dir.join("fit"),
            &format!(
                r#"{{"webrtc": {{"turn": {}}}}}"#,
                member(0, 0, 0, &cert, &key, peer)
            ),
            |_| {},
        );
        let (udp, tcp, tls) = (
            run.port_in("listens on ", "listens on "),
            run.port_in("listens on ", ", tcp "),
            run.port_in("listens on ", ", tls "),
        );
        let fit = scenarios(udp, tcp, tls, peer);
        assert_eq!(akit, fit, "akit and fit answer fib's client differently");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
