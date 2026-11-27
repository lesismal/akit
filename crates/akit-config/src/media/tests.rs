//! fit's `config/mediaauth_test.go` and `config/streaming_test.go`, ported (I120). What fit reads
//! there and akit does not yet (a policy's `keys` and `url_signing`, the role's `channels` and
//! `push`, RTSP's `auth_methods` and `realm`) is refused as not supported yet.

use super::*;
use crate::from_json;

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

/// fit's `TestMediaAuth`.
#[test]
fn media_auth() {
    let sum = "ab".repeat(32);
    let good = format!(
        r#"{{"listeners": [{{"name": "w", "address": ":8080"}}], "media_auth": [
		{{"name": "live", "secret": "0123456789abcdef", "public": ["play"], "token_ttl": "1h",
		 "tokens": [{{"name": "obs", "token": "0123456789abcdef-x", "publish": ["live/*"]}},
		            {{"token_sha256": "{sum}", "play": ["*"]}}],
		 "users": [{{"name": "alice", "password": "pw", "publish": ["alice-*"], "play": ["*"]}},
		           {{"name": "bob", "password_hash": "$2a$10$abcdefghijklmnopqrstuuJ5VrFz8Z1G0gq8x0bq4d8bGHT3fYz1m"}}]}},
		{{"name": "open", "public": ["publish", "play"]}}]}}"#
    );
    let c = loads(&good);
    let m = c.media_auth.iter().find(|p| p.name == "live").unwrap();
    assert_eq!(m.ttl(), Duration::from_secs(3600));
    assert_eq!(m.tokens[1].hash_bytes(), Some([0xab; 32]));
    assert_eq!(m.tokens[0].hash_bytes(), None);
    assert!(m.is_public(MediaAction::Play) && !m.is_public(MediaAction::Publish));
    let open = c.media_auth.iter().find(|p| p.name == "open").unwrap();
    assert_eq!(open.ttl(), DEFAULT_MEDIA_TOKEN_TTL);
    assert!(!c.media_auth.iter().any(|p| p.name == "nope"));
    assert!(valid_media_pattern("live/*"));
    for bad in ["", "a b", &"a".repeat(257)] {
        assert!(!valid_media_pattern(bad), "{bad:?}");
    }
    // A policy whose secret is a file lets someone in, and cannot also set secret; the file
    // itself is read when the policy is compiled, not here.
    let c = loads(
        r#"{"listeners": [{"name": "w", "address": ":8080"}], "media_auth": [{"name": "f", "secret_file": "/tmp/akit-media-secret"}]}"#,
    );
    assert_eq!(c.media_auth[0].secret_file, "/tmp/akit-media-secret");
    let doc = |policy: &str| {
        format!(
            r#"{{"listeners": [{{"name": "w", "address": ":8080"}}], "media_auth": [{policy}]}}"#
        )
    };
    for (bad, want) in [
        (r#"{"name": "a"}"#, "lets no one in"),
        (r#"{"name": "-a", "public": ["play"]}"#, "not a valid name"),
        (
            r#"{"name": "a", "public": ["play"]}, {"name": "a", "public": ["play"]}"#,
            "already named",
        ),
        (r#"{"name": "a", "public": ["watch"]}"#, "watch"),
        (
            r#"{"name": "a", "public": ["play", "play"]}"#,
            "listed twice",
        ),
        (r#"{"name": "a", "secret": "short"}"#, "at least 16"),
        (
            r#"{"name": "a", "secret": "0123456789abcdef", "secret_file": "/tmp/akit-secret"}"#,
            "set secret or secret_file",
        ),
        (
            r#"{"name": "a", "tokens": [{"play": ["*"]}]}"#,
            "set token or token_sha256",
        ),
        (
            r#"{"name": "a", "tokens": [{"token": "0123456789abcdef", "token_sha256": "00"}]}"#,
            "set token or token_sha256",
        ),
        (
            r#"{"name": "a", "tokens": [{"token": "short"}]}"#,
            "at least 16",
        ),
        (
            r#"{"name": "a", "tokens": [{"token_sha256": "abc"}]}"#,
            "64 hex digits",
        ),
        (
            r#"{"name": "a", "tokens": [{"name": "x", "token": "0123456789abcdef"}, {"name": "x", "token": "0123456789abcdeg"}]}"#,
            "another token is already named",
        ),
        (
            r#"{"name": "a", "tokens": [{"token": "0123456789abcdef", "publish": ["a b"]}]}"#,
            "not a stream pattern",
        ),
        (
            r#"{"name": "a", "tokens": [{"token": "0123456789abcdef", "play": [""]}]}"#,
            "not a stream pattern",
        ),
        (
            r#"{"name": "a", "users": [{"name": "a:b", "password": "p"}]}"#,
            "not a user name",
        ),
        (
            r#"{"name": "a", "users": [{"name": "u", "password": "p"}, {"name": "u", "password": "q"}]}"#,
            "already named",
        ),
        (
            r#"{"name": "a", "users": [{"name": "u"}]}"#,
            "set password or password_hash",
        ),
        (
            r#"{"name": "a", "users": [{"name": "u", "password_hash": "plain"}]}"#,
            "not a bcrypt hash",
        ),
        // fit's I119 and the media library's track.
        (
            r#"{"name": "a", "keys": [{"key": "k1", "secret": "0123456789abcdef"}]}"#,
            "not supported yet",
        ),
        (
            r#"{"name": "a", "secret": "0123456789abcdef", "url_signing": {}}"#,
            "not supported yet",
        ),
    ] {
        refused(&doc(bad), want);
    }
}

/// fit's `TestStreaming`.
#[test]
fn streaming() {
    let good = r#"{"media_auth": [{"name": "live", "public": ["play"]}], "streaming": {
		"max_streams": 8, "rtmp": {"listen": ":1935", "auth": "live", "idle_timeout": "5s"},
		"rtsp": {"listen": ":8554", "auth": "live"},
		"srt": {"listen": ":8890", "passphrase": "0123456789", "latency": "200ms"},
		"webrtc": {"listen": ":8188", "ice_servers": [{"urls": ["stun:stun.example.com"]}]}}}"#;
    let c = loads(good);
    let s = c.streaming.as_ref().unwrap();
    let (rtmp, rtsp, srt, w) = (
        s.rtmp.as_ref().unwrap(),
        s.rtsp.as_ref().unwrap(),
        s.srt.as_ref().unwrap(),
        s.webrtc.as_ref().unwrap(),
    );
    assert_eq!(s.streams(), 8);
    assert_eq!(s.players(), DEFAULT_STREAM_MAX_PLAYERS);
    assert_eq!(s.gop_cache(), DEFAULT_STREAM_GOP_CACHE_BYTES);
    assert_eq!(rtmp.idle(), Duration::from_secs(5));
    assert_eq!(rtmp.handshake(), DEFAULT_RTMP_HANDSHAKE_TIMEOUT);
    assert_eq!(rtmp.conns(), DEFAULT_STREAM_MAX_CONNS);
    assert_eq!(rtmp.auth, "live");
    assert_eq!(rtsp.session(), DEFAULT_RTSP_SESSION_TIMEOUT);
    assert_eq!(rtsp.conns(), DEFAULT_STREAM_MAX_CONNS);
    assert_eq!(srt.latency_or(), Duration::from_millis(200));
    assert_eq!(srt.peer_idle(), DEFAULT_SRT_PEER_IDLE_TIMEOUT);
    assert_eq!(srt.conns(), DEFAULT_STREAM_MAX_CONNS);
    assert_eq!(w.sessions(), DEFAULT_STREAM_WEBRTC_MAX_SESSIONS);
    assert_eq!(w.connect(), DEFAULT_STREAM_WEBRTC_CONNECT_TIMEOUT);
    assert_eq!(w.offer_bytes(), DEFAULT_STREAM_WEBRTC_MAX_OFFER_BYTES);
    assert_eq!(w.ice_servers[0].urls, ["stun:stun.example.com"]);
    let doc = |streaming: &str| {
        format!(
            r#"{{"media_auth": [{{"name": "live", "public": ["play"]}}], "streaming": {streaming}}}"#
        )
    };
    for (bad, want) in [
        ("{}", "nothing takes streams in"),
        (
            r#"{"rtmp": {"listen": ":1935", "auth": "nope"}}"#,
            r#"no media_auth policy named "nope""#,
        ),
        (
            r#"{"rtsp": {"listen": ":8554", "auth": "nope"}}"#,
            r#"no media_auth policy named "nope""#,
        ),
        (
            r#"{"srt": {"listen": ":8890", "auth": "nope"}}"#,
            r#"no media_auth policy named "nope""#,
        ),
        (r#"{"rtmp": {"listen": "1935"}}"#, "not a host and port"),
        (
            r#"{"rtmp": {"listen": ":1935"}, "rtsp": {"listen": ":1935"}}"#,
            "streaming.rtmp listens on TCP :1935 already",
        ),
        (
            r#"{"srt": {"listen": ":9000"}, "webrtc": {"listen": ":9000"}}"#,
            "streaming.srt listens on UDP :9000 already",
        ),
        (
            r#"{"srt": {"listen": ":8890", "passphrase": "short"}}"#,
            "10 to 79",
        ),
        (
            r#"{"webrtc": {"listen": ":8188", "public_ips": ["nope"]}}"#,
            "nope",
        ),
        (
            r#"{"webrtc": {"listen": ":8188", "public_ips": ["0.0.0.0"]}}"#,
            "wildcard",
        ),
        (
            r#"{"webrtc": {"listen": ":8188", "ice_servers": [{"urls": ["http://x"]}]}}"#,
            "not a stun:",
        ),
        (
            r#"{"webrtc": {"listen": ":8188", "ice_servers": [{"urls": ["stun:x"], "username": "a\"b"}]}}"#,
            "double quote",
        ),
        (
            r#"{"webrtc": {"listen": ":8188", "max_offer_bytes": 100000}}"#,
            "65536",
        ),
        (r#"{"rtmp": {}}"#, "listen"),
        // Not run by akit yet: Multipath TCP, RTMP's message bound (akio A263), RTSP's Digest
        // and Basic (akio A264), and the media library's channels and push targets.
        (
            r#"{"rtmp": {"listen": ":1935", "mptcp": true}}"#,
            "not supported yet",
        ),
        (
            r#"{"rtmp": {"listen": ":1935", "max_message_bytes": 1024}}"#,
            "not supported yet",
        ),
        (
            r#"{"rtsp": {"listen": ":8554", "auth_methods": ["digest"]}}"#,
            "not supported yet",
        ),
        (
            r#"{"rtsp": {"listen": ":8554", "realm": "cams"}}"#,
            "not supported yet",
        ),
        (
            r#"{"rtsp": {"listen": ":8554", "idle_timeout": "5s"}}"#,
            "not supported yet",
        ),
        (r#"{"channels": []}"#, "not supported yet"),
    ] {
        refused(&doc(bad), want);
    }
}

/// The `media_auth` list and `streaming` come from one file each, and the policies a role names
/// may be in another.
#[test]
fn across_files() {
    use crate::{Format, Source, from_sources};
    let files = |a: &'static str, b: &'static str| {
        from_sources(&[
            Source {
                name: Some("a.json"),
                format: Format::Json,
                src: a,
            },
            Source {
                name: Some("b.json"),
                format: Format::Json,
                src: b,
            },
        ])
    };
    let a = r#"{"media_auth": [{"name": "live", "public": ["play"]}]}"#;
    let b = r#"{"streaming": {"rtmp": {"listen": ":1935", "auth": "live"}}}"#;
    let c = files(a, b).unwrap();
    assert_eq!(c.media_auth.len(), 1);
    let e = files(a, a).unwrap_err();
    assert!(e.to_string().contains("also given in a.json"), "{e}");
}

#[test]
fn ice_urls() {
    for good in [
        "stun:stun.example.com",
        "TURN:t.example.com:3478?transport=tcp",
    ] {
        assert!(ice_url(good), "{good}");
    }
    for bad in ["stun:", "stun://x", "http://x", "stun:a b", "x"] {
        assert!(!ice_url(bad), "{bad}");
    }
    assert!(valid_user_name("alice") && !valid_user_name("") && !valid_user_name("a\tb"));
}
