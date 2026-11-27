//! The proxy's SDP and URL rewriting without a network (fit's `proxy_test.go`'s `TestRewriteSDP`
//! and its helpers').
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;

const ANSWER: &str = "v=0\r\no=- 1 1 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\nc=IN IP4 10.0.0.2\r\n\
a=ice-ufrag:uf1\r\na=ice-pwd:pw1\r\n\
m=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:0\r\n\
a=candidate:1 1 udp 2130706431 fe80::1 8189 typ host\r\n\
a=candidate:2 1 udp 1694498815 203.0.113.9 40000 typ srflx raddr 10.0.0.2 rport 8189\r\n\
a=candidate:3 1 udp 2130706430 10.0.0.2 8189 typ host\r\n\
a=candidate:4 2 udp 2130706430 10.0.0.2 8190 typ host\r\n\
a=end-of-candidates\r\n\
m=video 0 UDP/TLS/RTP/SAVPF 96\r\na=mid:1\r\n";

#[test]
fn rewrite() {
    let proxy: Vec<SocketAddr> = vec!["198.51.100.1:8190".parse().unwrap()];
    let (sdp, creds) = rewrite_sdp(ANSWER.as_bytes(), &proxy, None).unwrap();
    let sdp = String::from_utf8(sdp).unwrap();
    assert_eq!(
        creds,
        [Cred {
            ufrag: "uf1".into(),
            pwd: "pw1".into(),
            media: Some("10.0.0.2:8189".parse().unwrap()),
        }]
    );
    let cands: Vec<&str> = sdp
        .lines()
        .filter(|l| l.starts_with("a=candidate:"))
        .collect();
    assert_eq!(
        cands,
        ["a=candidate:1 1 udp 2130706431 198.51.100.1 8190 typ host"]
    );
    assert!(sdp.contains("c=IN IP4 198.51.100.1\r\n"));
    // The refused section gets none.
    assert_eq!(sdp.matches("a=end-of-candidates").count(), 1);
    // media_addrs: with a port, as it is; an IP alone takes the candidate's port.
    let (_, c) = rewrite_sdp(
        ANSWER.as_bytes(),
        &proxy,
        Some("10.9.9.9:7000".parse().unwrap()),
    )
    .unwrap();
    assert_eq!(c[0].media, Some("10.9.9.9:7000".parse().unwrap()));
    let (_, c) = rewrite_sdp(
        ANSWER.as_bytes(),
        &proxy,
        Some("10.9.9.9:0".parse().unwrap()),
    )
    .unwrap();
    assert_eq!(c[0].media, Some("10.9.9.9:8189".parse().unwrap()));
    // Without a host candidate, the server-reflexive one.
    let srflx: String = ANSWER
        .lines()
        .filter(|l| !l.contains("typ host"))
        .map(|l| format!("{l}\r\n"))
        .collect();
    let (_, c) = rewrite_sdp(srflx.as_bytes(), &proxy, None).unwrap();
    assert_eq!(c[0].media, Some("203.0.113.9:40000".parse().unwrap()));
    assert!(rewrite_sdp(ANSWER.as_bytes(), &[], None).is_err());
}

#[test]
fn strip_and_paths() {
    let s = strip_candidates(ANSWER.as_bytes());
    let s = String::from_utf8(s).unwrap();
    assert!(!s.contains("a=candidate") && !s.contains("end-of-candidates"));
    assert!(s.contains("a=ice-ufrag:uf1\r\n"));
    assert_eq!(
        upstream_path("/whip/cam", "/whip/cam/abc").as_deref(),
        Some("/whip/cam/abc")
    );
    assert_eq!(
        upstream_path("/whip/cam", "http://10.0.0.2:8080/s/1?x=y").as_deref(),
        Some("/s/1?x=y")
    );
    assert_eq!(
        upstream_path("/whip/cam", "cam/abc").as_deref(),
        Some("/whip/cam/abc")
    );
    assert_eq!(upstream_path("/whip/cam", " "), None);
    assert_eq!(
        split_links(r#"<a,b>; rel="x, y", <c>"#),
        [r#"<a,b>; rel="x, y""#, " <c>"]
    );
    let mut h = Headers::with_capacity(2, 0);
    h.append(
        "Link",
        r#"<stun:s.example>; rel="ice-server", </m/s1/sse>; rel="urn:ietf:params:whep:ext:core:server-sent-events""#,
    );
    h.append("Link", "</other/x>; rel=\"x\"");
    assert_eq!(
        rewrite_links(&h, "/m/s1", "/w/abc"),
        [
            r#"<stun:s.example>; rel="ice-server""#,
            r#"</w/abc/sse>; rel="urn:ietf:params:whep:ext:core:server-sent-events""#
        ]
    );
}
