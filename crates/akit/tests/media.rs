//! fit's media auth and streaming role through akit's command line (I121–I124): `akit
//! media-token` signs a token the policy takes, and refuses what fit's refuses.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::PathBuf;
use std::process::Command;

use akit_config::media::MediaAction;
use akit_media::auth::{Credentials, Policy};

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("akit-media-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// fit's `TestMediaToken` (`cli/mediatoken_test.go`).
#[test]
fn media_token() {
    let d = dir("token");
    let cfg = d.join("akit.json");
    let json = r#"{"media_auth": [
        {"name": "live", "secret": "0123456789abcdef-secret"},
        {"name": "nosecret", "public": ["play"]}]}"#;
    std::fs::write(&cfg, json).unwrap();
    let akit = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_akit"))
            .arg("media-token")
            .arg("-c")
            .arg(&cfg)
            .args(args)
            .output()
            .unwrap()
    };
    let out = akit(&[
        "-policy", "live", "-publish", "live/*", "-play", "*", "-ttl", "1h", "-subject", "obs",
    ]);
    assert!(out.status.success(), "{out:?}");
    let token = String::from_utf8(out.stdout.clone())
        .unwrap()
        .trim()
        .to_owned();
    assert!(token.starts_with("fm1."), "{token}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("akit media-token: good until "),
        "{out:?}"
    );
    let config = akit_config::from_json(json, None).unwrap();
    let p = Policy::from_config(&config.media_auth[0]).unwrap();
    let c = Credentials {
        token,
        ..Credentials::default()
    };
    assert_eq!(
        p.allow(MediaAction::Publish, "live/cam", &c),
        Ok("obs".into())
    );
    assert!(p.allow(MediaAction::Publish, "other", &c).is_err());
    for (args, code, want) in [
        (&["-publish", "x"][..], 2, "-policy is required"),
        (&["-policy", "live"][..], 2, "give -publish, -play or both"),
        (
            &["-policy", "nope", "-play", "*"][..],
            1,
            "no media_auth policy named \"nope\"",
        ),
        (
            &["-policy", "nosecret", "-play", "*"][..],
            1,
            "neither secret nor secret_file",
        ),
        (
            &["-policy", "live", "-play", "a b"][..],
            1,
            "not a stream pattern",
        ),
        (
            &["-policy", "live", "-play", "*", "-ttl", "x"][..],
            2,
            "-ttl",
        ),
    ] {
        let out = akit(args);
        assert_eq!(out.status.code(), Some(code), "{args:?}: {out:?}");
        let e = String::from_utf8_lossy(&out.stderr);
        assert!(e.contains(want), "{args:?}: {e}");
    }
    let _ = std::fs::remove_dir_all(&d);
}
