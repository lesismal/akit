//! fit's `config/webrtc_test.go`, the WebRTC cases of `bindings_test.go` and `more_test.go`, and
//! the handlers' params (`webrtc/media/handler.go`, `proxyhttp.go`), ported (I110).

use super::*;
use crate::{Handler, from_json};

fn refused(doc: &str, want: &str) {
    match from_json(doc, None) {
        Ok(_) => panic!("{doc:.200}: loaded, want {want:?}"),
        Err(e) => {
            let s = e.to_string();
            assert!(s.contains(want), "{doc:.200}: {s}, want {want:?}");
        }
    }
}

fn loads(doc: &str) -> crate::Config {
    match from_json(doc, None) {
        Ok(c) => c,
        Err(e) => panic!("{doc:.200}: refused: {e}"),
    }
}

/// fit's `TestWebRTCMembers`.
#[test]
fn members() {
    let good = r#"{"webrtc": {"turn": {"listen": [":3478", "[::1]:3479"], "realm": "example.com",
        "users": [{"name": "alice", "password": "pw"}, {"name": "bob", "key": "0x0123456789abcdef0123456789ABCDEF"}],
        "secret": "s", "relay_ip": "10.0.0.5", "public_ip": "203.0.113.5", "port_min": 49152, "port_max": 49999,
        "allowed_peers": ["10.1.0.0/16", "10.0.0.5", "10.0.0.5:8189", "[fd00::1]:9"], "denied_peers": ["198.51.100.0/24"], "default_lifetime": "5m",
        "listen_tcp": [":3478"], "listen_tls": [":5349"], "tls": {"certificates": [{"cert_file": "c.pem", "key_file": "k.pem"}]},
        "max_permissions": 8, "stream_idle_timeout": "5s"}}}"#;
    let c = loads(good);
    let t = c.webrtc.as_ref().and_then(|w| w.turn.as_ref()).unwrap();
    assert_eq!(t.max_allocs(), DEFAULT_TURN_MAX_ALLOCATIONS);
    assert_eq!(
        t.max_allocs_per_user(),
        DEFAULT_TURN_MAX_ALLOCATIONS_PER_USER
    );
    assert_eq!(t.bandwidth(), DEFAULT_TURN_USER_BANDWIDTH);
    assert_eq!(
        t.users[1].key_bytes(),
        Some([
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab,
            0xcd, 0xef
        ])
    );
    assert_eq!(t.max_perms(), 8);
    assert_eq!(t.max_chans(), DEFAULT_TURN_MAX_CHANNELS);
    assert_eq!(t.stream_message(), DEFAULT_TURN_MAX_STREAM_MESSAGE);
    assert_eq!(t.stream_idle(), Duration::from_secs(5));
    assert_eq!(t.lifetime(), Duration::from_secs(300));
    assert_eq!((t.port_min, t.port_max), (49152, 49999));
    assert_eq!(t.allowed_peers.len(), 4);
    assert_eq!(t.allowed_peers[2].port, 8189);
    for (s, bits, port) in [
        ("10.0.0.5", 32, 0),
        ("10.0.0.0/8", 8, 0),
        ("10.0.0.5:8189", 32, 8189),
        ("[::1]:9", 128, 9),
    ] {
        let r = PeerRule::parse(s).unwrap_or_else(|| panic!("{s}"));
        assert_eq!((r.bits, r.port), (bits, port), "{s}");
    }
    assert!(PeerRule::parse("10.0.0.5:0").is_none(), "port 0 parsed");
    assert!(PeerRule::parse("10.0.0.0/33").is_none());
    let r = PeerRule::parse("10.1.0.0/16").unwrap();
    assert!(r.contains("10.1.200.3".parse().unwrap()));
    assert!(r.contains("::ffff:10.1.0.9".parse().unwrap()));
    assert!(!r.contains("10.2.0.1".parse().unwrap()));
    assert!(!r.contains("::1".parse().unwrap()));

    let turn = |fields: &str| {
        format!(
            r#"{{"webrtc": {{"turn": {{"listen": [":3478"], "realm": "r", "relay_ip": "10.0.0.5", {fields}}}}}}}"#
        )
    };
    for (doc, want) in [
        (turn(r#""users": []"#), "no one could log in"),
        (turn(r#""secret": "s", "realm": " ""#), "duplicate"),
        (
            r#"{"webrtc": {"turn": {"listen": [], "realm": "r", "relay_ip": "10.0.0.5", "secret": "s"}}}"#.into(),
            "at least one address",
        ),
        (
            r#"{"webrtc": {"turn": {"listen": [":3478", ":3478"], "realm": "r", "relay_ip": "10.0.0.5", "secret": "s"}}}"#.into(),
            "webrtc.turn.listen[0] listens on UDP :3478 already",
        ),
        (
            r#"{"webrtc": {"turn": {"listen": ["nope"], "realm": "r", "relay_ip": "10.0.0.5", "secret": "s"}}}"#.into(),
            "not a host and port",
        ),
        (
            r#"{"webrtc": {"turn": {"listen": [":3478"], "realm": " ", "relay_ip": "10.0.0.5", "secret": "s"}}}"#.into(),
            "realm is empty",
        ),
        (
            turn(r#""users": [{"name": "a", "password": "p"}, {"name": "a", "password": "q"}]"#),
            "already named",
        ),
        (turn(r#""users": [{"name": "a"}]"#), "set password or key"),
        (
            turn(r#""users": [{"name": "a", "password": "p", "key": "00"}]"#),
            "set password or key",
        ),
        (turn(r#""users": [{"name": "a", "key": "xyz"}]"#), "32 hex digits"),
        (
            r#"{"webrtc": {"turn": {"listen": [":3478"], "realm": "r", "relay_ip": "0.0.0.0", "secret": "s"}}}"#.into(),
            "wildcard",
        ),
        (
            r#"{"webrtc": {"turn": {"listen": [":3478"], "realm": "r", "relay_ip": "host", "secret": "s"}}}"#.into(),
            "not an IP address",
        ),
        (
            turn(r#""secret": "s", "public_ip": "2001:db8::1""#),
            "relay_ip's family",
        ),
        (turn(r#""secret": "s", "port_min": 5000"#), "together"),
        (
            turn(r#""secret": "s", "port_min": 6000, "port_max": 5000"#),
            "above port_max",
        ),
        (
            turn(r#""secret": "s", "max_lifetime": "1m""#),
            "below default_lifetime",
        ),
        (
            turn(r#""secret": "s", "allowed_peers": ["10.0.0.0/33"]"#),
            "not an address, an address and port, or a prefix",
        ),
        (
            turn(r#""secret": "s", "denied_peers": ["x"]"#),
            "not an address, an address and port, or a prefix",
        ),
        (
            turn(r#""secret": "s", "listen_tcp": ["nope"]"#),
            "not a host and port",
        ),
        (
            turn(
                r#""secret": "s", "listen_tcp": [":3478"], "listen_tls": [":3478"], "tls": {"certificates": [{"cert_file": "c", "key_file": "k"}]}"#,
            ),
            "webrtc.turn.listen_tcp[0] listens on TCP :3478 already",
        ),
        (
            turn(r#""secret": "s", "listen_tls": [":5349"]"#),
            "listen_tls needs certificates",
        ),
        (
            turn(r#""secret": "s", "tls": {"certificates": [{"cert_file": "c", "key_file": "k"}]}"#),
            "tls is for listen_tls",
        ),
        (
            turn(
                r#""secret": "s", "listen_tls": [":5349"], "tls": {"certificates": [{"cert_file": "c", "key_file": "k"}], "acme": ["a.example"]}"#,
            ),
            "unknown field",
        ),
        (
            turn(r#""secret": "s", "max_stream_message": 100"#),
            "too small for a datagram",
        ),
        (
            turn(r#""secret": "s", "max_channels": -1"#),
            "less than the minimum",
        ),
        (turn(r#""secret": "s", "port_max": 70000, "port_min": 1"#), "greater than the maximum"),
        // akit's own: a field fit does not have.
        (turn(r#""secret": "s", "nope": 1"#), "unknown field"),
    ] {
        refused(&doc, want);
    }
}

/// fit's `TestWebRTCMedia`.
#[test]
fn media() {
    let good = r#"{"webrtc": {"turn": {"listen": [":3478"], "realm": "r", "relay_ip": "10.0.0.5", "secret": "s"},
        "media": {"listen": ":8189", "public_ips": ["203.0.113.5"], "codecs": ["opus", "vp8", "H264"],
        "ice_servers": [{"urls": ["stun:stun.example.com:3478"]}, {"urls": ["turn:t.example.com?transport=udp"], "username": "u", "credential": "p"}],
        "turn_urls": ["turn:turn.example.com:3478"], "connect_timeout": "10s"}}}"#;
    let c = loads(good);
    let m = c.webrtc.as_ref().and_then(|w| w.media.as_ref()).unwrap();
    assert_eq!(m.max_pcs(), DEFAULT_MEDIA_MAX_PEER_CONNECTIONS);
    assert_eq!(m.max_stream_count(), DEFAULT_MEDIA_MAX_STREAMS);
    assert_eq!(m.max_viewer_count(), DEFAULT_MEDIA_MAX_VIEWERS);
    assert_eq!(m.bitrate(), DEFAULT_MEDIA_MAX_BITRATE);
    assert_eq!(m.sdp_bytes(), DEFAULT_MEDIA_MAX_SDP_BYTES);
    assert_eq!(m.connect(), Duration::from_secs(10));
    assert_eq!(m.ice(), DEFAULT_MEDIA_ICE_TIMEOUT);
    assert_eq!(m.key_frames(), DEFAULT_MEDIA_KEY_FRAME_INTERVAL);
    assert_eq!(m.credential_ttl(), DEFAULT_MEDIA_TURN_CREDENTIAL_TTL);
    assert_eq!(m.ice_servers.len(), 2);
    loads(r#"{"webrtc": {"media": {"listen": "127.0.0.1:0"}}}"#);
    let media =
        |fields: &str| format!(r#"{{"webrtc": {{"media": {{"listen": ":8189", {fields}}}}}}}"#);
    for (doc, want) in [
        (
            r#"{"webrtc": {"media": {"listen": "nope"}}}"#.to_owned(),
            "not a host and port",
        ),
        (media(r#""public_ips": ["x"]"#), "not an IP address"),
        (media(r#""public_ips": ["::"]"#), "wildcard"),
        (
            media(r#""ice_servers": [{"urls": []}]"#),
            "at least one URL",
        ),
        (
            media(r#""ice_servers": [{"urls": ["http://x"]}]"#),
            "not a stun:",
        ),
        (
            media(r#""ice_servers": [{"urls": ["stun:x"], "username": "a\"b"}]"#),
            "double quote",
        ),
        (
            media(r#""turn_urls": ["stun:x"]"#),
            "not a turn: or turns: URL",
        ),
        (media(r#""turn_urls": ["turn:x"]"#), "webrtc.turn's secret"),
        (media(r#""codecs": ["h265"]"#), "not one of"),
        (media(r#""max_viewers": -1"#), "less than the minimum"),
        (
            r#"{"webrtc": {"media": {}}}"#.to_owned(),
            "missing required field",
        ),
    ] {
        refused(&doc, want);
    }
}

/// fit's `TestWebRTCProxy`.
#[test]
fn proxy() {
    let c = loads(
        r#"{"webrtc": {"proxy": {"listen": ":8190", "public_ips": ["203.0.113.5"], "idle_timeout": "10s",
        "relay_ip": "10.0.0.5", "port_min": 40000, "port_max": 40099}}}"#,
    );
    let p = c.webrtc.as_ref().and_then(|w| w.proxy.as_ref()).unwrap();
    assert_eq!(p.sessions(), DEFAULT_PROXY_MAX_SESSIONS);
    assert_eq!(p.relays(), DEFAULT_PROXY_MAX_RELAYS);
    assert_eq!(p.session_idle(), DEFAULT_PROXY_SESSION_TIMEOUT);
    assert_eq!(p.idle(), Duration::from_secs(10));
    assert_eq!(p.relay_ip, Some("10.0.0.5".parse().unwrap()));
    assert_eq!((p.port_min, p.port_max), (40000, 40099));
    assert_eq!(WebRtcProxy::default().relay_ip, None);
    for (doc, want) in [
        (
            r#"{"webrtc": {"proxy": {"listen": "nope"}}}"#,
            "not a host and port",
        ),
        (
            r#"{"webrtc": {"proxy": {"listen": ":8190", "public_ips": ["0.0.0.0"]}}}"#,
            "wildcard",
        ),
        (
            r#"{"webrtc": {"media": {"listen": ":8189"}, "proxy": {"listen": ":8189"}}}"#,
            "webrtc.media listens on UDP :8189 already",
        ),
        (
            r#"{"webrtc": {"proxy": {"listen": ":8190", "max_relays": -1}}}"#,
            "less than the minimum",
        ),
        (
            r#"{"webrtc": {"proxy": {"listen": ":8190", "relay_ip": "x"}}}"#,
            "not an IP address",
        ),
        (
            r#"{"webrtc": {"proxy": {"listen": ":8190", "relay_ip": "::"}}}"#,
            "wildcard",
        ),
        (
            r#"{"webrtc": {"proxy": {"listen": ":8190", "port_max": 5000}}}"#,
            "together",
        ),
        (
            r#"{"webrtc": {"proxy": {"listen": ":8190", "port_min": 6000, "port_max": 5000}}}"#,
            "above port_max",
        ),
        (
            r#"{"webrtc": {"proxy": {"listen": ":8190", "port_min": 6000, "port_max": 70000}}}"#,
            "greater than the maximum",
        ),
    ] {
        refused(doc, want);
    }
}

/// fit's `more_test.go`: each role's `access` is checked, at its own path.
#[test]
fn access_checked() {
    for (doc, want) in [
        (
            r#"{"webrtc": {"turn": {"listen": [":3478"], "realm": "r", "secret": "s", "relay_ip": "127.0.0.1", "access": {"allow": ["10.0.0.0/33"]}}}}"#,
            "webrtc.turn.access.allow[0]",
        ),
        (
            r#"{"webrtc": {"media": {"listen": ":8189", "access": {"deny": ["::1/129"]}}}}"#,
            "webrtc.media.access.deny[0]",
        ),
        (
            r#"{"webrtc": {"proxy": {"listen": ":8190", "access": {"deny_files": [" "]}}}}"#,
            "webrtc.proxy.access.deny_files[0]",
        ),
    ] {
        refused(doc, want);
    }
}

/// The WebRTC roles' addresses join the listeners' in one check: a listener and the TURN server
/// on one UDP port clash, a TCP listener and a UDP role do not (fit's `bindings_test.go`).
#[test]
fn binds_with_listeners() {
    let doc = r#"{"listeners": [{"name": "dns", "address": ":3478", "protocols": ["udp"]}],
        "services": [{"name": "s", "listeners": ["dns"], "upstream": "u"}], "upstreams": [{"name": "u", "addresses": ["127.0.0.1:9"]}],
        "webrtc": {"turn": {"listen": [":3478"], "realm": "r", "secret": "s", "relay_ip": "10.0.0.5"}}}"#;
    refused(doc, "listener \"dns\" listens on UDP :3478 already");
    loads(
        r#"{"listeners": [{"name": "web", "address": ":8189"}], "routes": [{"handler": {"module": "respond"}}],
        "webrtc": {"media": {"listen": ":8189"}}}"#,
    );
}

fn handler(doc: &str) -> Handler {
    loads(doc).routes.into_iter().next().unwrap().handler
}

/// The handlers' params: `whip` and `whep` take a stream's name, `webrtc_proxy` an upstream and
/// its media addresses; `whip`'s and `whep`'s `auth` names a policy of the config (I126),
/// `webrtc_proxy`'s is not supported yet (I114).
#[test]
fn handlers() {
    let route = |module: &str, params: &str| {
        format!(
            r#"{{"listeners": [{{"name": "web", "address": ":8080"}}], "upstreams": [{{"name": "m", "addresses": ["10.0.0.2:8080"]}}], "routes": [{{"handler": {{"module": "{module}"{params}}}}}]}}"#
        )
    };
    assert_eq!(
        handler(&route("whip", "")),
        Handler::Whip(WhipWhep::default())
    );
    assert_eq!(
        handler(&route("whep", r#", "params": {"stream": "cam-1.a_b~"}"#)),
        Handler::Whep(WhipWhep {
            stream: "cam-1.a_b~".into(),
            ..WhipWhep::default()
        })
    );
    let Handler::WebRtcProxy(p) = handler(&route(
        "webrtc_proxy",
        r#", "params": {"upstream": "m", "media_addrs": {"10.0.0.2:8080": "10.0.0.2:8189", "10.0.0.3:8080": "10.0.0.3"}, "timeout": "3s", "max_sdp_bytes": 1000, "action": "play"}"#,
    )) else {
        panic!("not webrtc_proxy");
    };
    assert_eq!(p.upstream, "m");
    assert_eq!(
        p.media_addrs,
        vec![
            ("10.0.0.2:8080".into(), "10.0.0.2:8189".parse().unwrap()),
            ("10.0.0.3:8080".into(), "10.0.0.3:0".parse().unwrap()),
        ]
    );
    assert_eq!(p.timeout(), Duration::from_secs(3));
    assert_eq!(p.sdp_bytes(), 1000);
    assert_eq!(p.action, Some(MediaAction::Play));
    let Handler::WebRtcProxy(p) =
        handler(&route("webrtc_proxy", r#", "params": {"upstream": "m"}"#))
    else {
        panic!("not webrtc_proxy");
    };
    assert_eq!(p.timeout(), DEFAULT_WEBRTC_PROXY_TIMEOUT);
    assert_eq!(p.sdp_bytes(), DEFAULT_MEDIA_MAX_SDP_BYTES);
    for (doc, want) in [
        (
            route("whip", r#", "params": {"stream": "a/b"}"#),
            "is not a name of 1 to 128 letters",
        ),
        (
            route(
                "whep",
                &format!(r#", "params": {{"stream": "{}"}}"#, "a".repeat(129)),
            ),
            "is not a name of 1 to 128 letters",
        ),
        (
            route("whip", r#", "params": {"auth": "cams"}"#),
            r#"routes[0].handler.params.auth: there is no media_auth policy named "cams""#,
        ),
        (
            route(
                "webrtc_proxy",
                r#", "params": {"upstream": "m", "auth": "cams", "action": "publish"}"#,
            ),
            "routes[0].handler.params.auth: not supported yet",
        ),
        (
            route("webrtc_proxy", ""),
            "routes[0].handler.params.upstream: missing required field",
        ),
        (
            route(
                "webrtc_proxy",
                r#", "params": {"upstream": "m", "action": "watch"}"#,
            ),
            "is not one of \"publish\", \"play\"",
        ),
        (
            route(
                "webrtc_proxy",
                r#", "params": {"upstream": "m", "media_addrs": {"10.0.0.2:8080": "media"}}"#,
            ),
            "is not an IP address, or an IP and a port",
        ),
        (
            route(
                "webrtc_proxy",
                r#", "params": {"upstream": "m", "media_addrs": {"10.0.0.2:8080": "0.0.0.0"}}"#,
            ),
            "is not an IP address, or an IP and a port",
        ),
        (
            route(
                "webrtc_proxy",
                r#", "params": {"upstream": "m", "max_sdp_bytes": -1}"#,
            ),
            "less than the minimum",
        ),
        (route("whep", r#", "params": {"nope": 1}"#), "unknown field"),
    ] {
        refused(&doc, want);
    }
}

#[test]
fn urls_and_names() {
    for good in [
        "stun:stun.example.com",
        "TURN:t:3478?transport=tcp",
        "turns:[::1]:5349",
    ] {
        assert!(ice_url(good), "{good}");
    }
    for bad in ["stun:", "stun://x", "http:x", "turn:a b", "stun"] {
        assert!(!ice_url(bad), "{bad}");
    }
    assert!(link_safe("user-1:pw"));
    assert!(!link_safe("a\\b") && !link_safe("a\u{7f}"));
    assert!(stream_name("cam1") && !stream_name("") && !stream_name("a%2Fb"));
    assert_eq!(TurnUser::default().key_bytes(), None);
}
