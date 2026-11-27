//! The role's parts that need no engine.

use super::*;

#[test]
fn addresses() {
    assert_eq!(listen_addr(":1935"), Some("[::]:1935".parse().unwrap()));
    assert_eq!(
        listen_addr("localhost:1935"),
        Some("127.0.0.1:1935".parse().unwrap())
    );
    assert_eq!(listen_addr("nope"), None);
    // fit's `sameAddress`: the old listener goes first when the two may take the same port.
    assert!(same_address(":1935", "127.0.0.1:1935"));
    assert!(same_address("0.0.0.0:1935", "[::]:1935"));
    assert!(same_address("localhost:1935", "127.0.0.1:1935"));
    assert!(!same_address("127.0.0.1:1935", "127.0.0.2:1935"));
    assert!(!same_address(":1935", ":1936"));
    assert!(!same_address(":0", ":0"));
}

/// fit's `srtConnect`'s credentials.
#[test]
fn srt_ids() {
    let id = |s: &str| StreamId::parse(s).unwrap();
    let (a, c) = srt_credentials(&id("#!::r=live/cam,m=publish,s=T"));
    assert_eq!((a, c.token.as_str()), (Action::Publish, "T"));
    let (a, c) = srt_credentials(&id("#!::r=live/cam,u=alice,s=pw"));
    assert_eq!(
        (a, c.user.as_str(), c.password.as_str()),
        (Action::Play, "alice", "pw")
    );
    let (_, c) = srt_credentials(&id("live/cam?user=bob&pass=x"));
    assert_eq!((c.user.as_str(), c.password.as_str()), ("bob", "x"));
    assert!(srt_credentials(&id("live/cam")).1.is_empty());
}

#[test]
fn guards() {
    let open = Guard::default();
    assert_eq!(
        open.allow(Action::Publish, "x", &Credentials::default()),
        Ok(())
    );
    // A policy the running config does not have refuses everyone.
    let missing = Guard {
        name: "gone".into(),
        policy: None,
    };
    assert_eq!(
        missing.allow(Action::Play, "x", &Credentials::default()),
        Err(Refusal::Unavailable)
    );
    let shared = Shared::default();
    shared.auth[RTMP].set(Some(Arc::new(missing)));
    let info = PeerInfo::new("rtmp", None, "");
    assert!(shared.hook(Action::Play, "x", &info).is_err());
    // Another protocol's clients were checked before the hub was asked.
    assert!(
        shared
            .hook(Action::Publish, "x", &PeerInfo::new("http-flv", None, ""))
            .is_ok()
    );
    assert_eq!(shared.meters.refused[RTMP][2].load(SeqCst), 1);
    assert_eq!(shared.meters.published.load(SeqCst), 1);
}
