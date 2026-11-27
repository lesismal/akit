//! The media server's pieces without a network (I113): the SDP it reads, fit's `pickCodecs`,
//! the `Link` headers and the bitrate bucket.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;

const OFFER: &str = "v=0\r\no=- 1 1 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\n\
m=audio 9 UDP/TLS/RTP/SAVPF 0 111\r\na=mid:0\r\na=sendonly\r\na=rtpmap:0 PCMU/8000\r\na=rtpmap:111 opus/48000/2\r\n\
m=video 9 UDP/TLS/RTP/SAVPF 102 96 98\r\na=mid:1\r\na=sendonly\r\na=rtpmap:102 H264/90000\r\na=rtpmap:96 VP8/90000\r\na=rtpmap:98 VP9/90000\r\n\
m=video 0 UDP/TLS/RTP/SAVPF 96\r\na=mid:2\r\na=rtpmap:96 VP8/90000\r\n";

#[test]
fn picks() {
    let all: Vec<String> = akit_config::webrtc::MEDIA_CODECS
        .iter()
        .map(|c| (*c).to_owned())
        .collect();
    assert_eq!(pick_codecs(OFFER, &all), [Codec::Opus, Codec::H264]);
    assert_eq!(
        pick_codecs(OFFER, &["vp8".into(), "opus".into()]),
        [Codec::Opus, Codec::Vp8]
    );
    assert!(pick_codecs(OFFER, &["AV1".into()]).is_empty());
    // A section that only receives sends nothing to pick.
    let recv = OFFER.replace("a=sendonly", "a=recvonly");
    assert!(pick_codecs(&recv, &all).is_empty());
    assert!(answers(OFFER, &[Direction::SendOnly]));
    assert!(!answers(&recv, &[Direction::SendOnly]));
    assert_eq!(sections(OFFER).len(), 3);
    assert!(sections(OFFER)[2].port_zero);
}

#[test]
fn links() {
    let core = Arc::new_cyclic(|me| Core {
        base: Instant::now(),
        set: RwLock::new(Arc::new(Settings::of(
            &WebRtc::default(),
            &WebRtcMedia::default(),
        ))),
        st: Mutex::new(State::default()),
        access: Mutex::new(None),
        m: Meters::default(),
        me: me.clone(),
    });
    assert!(core.links("0123456789abcdef0123456789abcdef").is_empty());
    let c = akit_config::from_json(
        r#"{"webrtc": {"turn": {"listen": [":3478"], "realm": "r", "relay_ip": "10.0.0.5", "secret": "s"},
            "media": {"listen": ":8189", "ice_servers": [{"urls": ["stun:a:3478", "stun:b"]}, {"urls": ["turn:t"], "username": "u", "credential": "p"}],
            "turn_urls": ["turn:mine:3478"]}}}"#,
        None,
    )
    .unwrap();
    let w = c.webrtc.unwrap();
    write(
        &core.set,
        Arc::new(Settings::of(&w, w.media.as_ref().unwrap())),
    );
    let l = core.links("0123456789abcdef0123456789abcdef");
    assert_eq!(l.len(), 4);
    assert_eq!(l[0], r#"<stun:a:3478>; rel="ice-server""#);
    assert_eq!(
        l[2],
        r#"<turn:t>; rel="ice-server"; username="u"; credential="p"; credential-type="password""#
    );
    // Time-limited credentials of webrtc.turn's secret, for the session.
    assert!(l[3].starts_with("<turn:mine:3478>; rel=\"ice-server\"; username=\""));
    assert!(l[3].contains(":akit-0123456789abcdef\"; credential=\""));
    assert_eq!(core.addrs(), Vec::<SocketAddr>::new());
}

#[test]
fn bitrate_bucket() {
    let b = Mutex::new((0.0, None));
    let t0 = Instant::now();
    // 8000 b/s: 1000 bytes a second, 2000 of burst.
    assert!(allow(&b, 2000, 8000, t0));
    assert!(!allow(&b, 1, 8000, t0));
    assert!(allow(&b, 500, 8000, t0 + Duration::from_millis(500)));
    assert!(!allow(&b, 100, 8000, t0 + Duration::from_millis(500)));
}
