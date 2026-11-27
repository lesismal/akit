//! fit's `TestPeerAllowed`, without a network: the peer rules' order and ports, and the servers'
//! own addresses (I111).
#![allow(clippy::unwrap_used)]

use super::*;

fn rule(s: &str) -> PeerRule {
    PeerRule::parse(s).unwrap()
}

#[test]
fn peer_allowed() {
    let s = Shared::new();
    write(
        &s.rules,
        Arc::new(Rules {
            allowed: ["10.0.0.0/8", "10.0.0.5:8189", "192.168.1.1:5000"]
                .map(rule)
                .to_vec(),
            denied: ["8.8.8.8:53", "9.9.9.0/24"].map(rule).to_vec(),
        }),
    );
    write(
        &s.own,
        Arc::new(vec![
            "10.0.0.5".parse().unwrap(),
            "203.0.113.5".parse().unwrap(),
        ]),
    );
    for (peer, want) in [
        ("10.0.0.5:0", true),        // a permission on the own address
        ("10.0.0.5:8080", false),    // not a listed port, whatever 10/8 says
        ("203.0.113.5:8080", false), // the public address alike
        ("10.0.0.5:8189", true),     // listed with its port
        ("[::ffff:10.0.0.5]:8189", true),
        ("10.1.2.3:80", true),   // by the prefix
        ("192.168.1.1:0", true), // a port of it is allowed
        ("192.168.1.1:5000", true),
        ("192.168.1.1:5001", false), // denied by default
        ("192.168.1.2:0", false),
        ("8.8.8.8:53", false),
        ("8.8.8.8:54", true),
        ("8.8.8.8:0", true),
        ("9.9.9.9:0", false),
    ] {
        assert_eq!(s.peer_allowed(peer.parse().unwrap()), want, "{peer}");
    }
}

#[test]
fn listen_addrs() {
    assert_eq!(listen_addr(":3478"), Some("[::]:3478".parse().unwrap()));
    assert_eq!(
        listen_addr("localhost:3478"),
        Some("127.0.0.1:3478".parse().unwrap())
    );
    assert_eq!(listen_addr("[::1]:1"), Some("[::1]:1".parse().unwrap()));
    assert_eq!(listen_addr("turn.example.com:3478"), None);
}

#[test]
fn credentials() {
    let m = akit_config::from_json(
        r#"{"webrtc": {"turn": {"listen": [":3478"], "realm": "r", "relay_ip": "10.0.0.5", "secret": "s",
            "users": [{"name": "a", "password": "p"}, {"name": "b", "key": "000102030405060708090a0b0c0d0e0f"}]}}}"#,
        None,
    )
    .unwrap()
    .webrtc
    .unwrap()
    .turn
    .unwrap();
    let c = Creds::of(&m);
    assert_eq!(
        c.key("a", "r"),
        Some(akio_turn::stun::long_term_key("a", "r", "p"))
    );
    assert_eq!(c.key("b", "r").unwrap()[15], 15);
    assert_eq!(c.key("a", "other"), None);
    let (u, p) = akio_turn::ephemeral_credentials(b"s", "x", Duration::from_secs(60));
    assert_eq!(
        c.key(&u, "r"),
        Some(akio_turn::stun::long_term_key(&u, "r", &p))
    );
    assert_eq!(c.key("nobody", "r"), None);
}
