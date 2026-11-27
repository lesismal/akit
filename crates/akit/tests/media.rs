//! fit's media auth and streaming role through akit's command line (I121–I124): `akit
//! media-token` signs a token the policy takes, and refuses what fit's refuses; the role against
//! ffmpeg (RTMP, RTSP, SRT, HTTP-FLV, HLS and WHIP) and fit's binary.
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
            "has neither secret, secret_file nor keys",
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

// ---- I122: the streaming role through `akit run`, against ffmpeg and fit's binary ----

use std::io::{BufRead, BufReader};
use std::net::{TcpListener, UdpSocket};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_tcp() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn free_udp() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Whether `tool` runs, and, for ffmpeg, has `protocol`.
fn have(tool: &str, protocol: &str) -> bool {
    let Ok(out) = Command::new(tool)
        .args(["-hide_banner", "-protocols"])
        .output()
    else {
        return false;
    };
    protocol.is_empty()
        || String::from_utf8_lossy(&out.stdout)
            .lines()
            .any(|l| l.trim() == protocol)
}

/// `akit run -c path` (or fit's, `bin run -c path`), once it says the role runs (akit), or its
/// RTMP port takes connections (fit).
fn serve(bin: &str, path: &std::path::Path, rtmp: u16) -> Killed {
    let mut child = Command::new(bin)
        .args(["run", "-c"])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let out = BufReader::new(child.stdout.take().unwrap());
    std::thread::spawn(move || for _ in out.lines() {});
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::net::TcpStream::connect(("127.0.0.1", rtmp)).is_err() {
        assert!(Instant::now() < deadline, "{bin} did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    Killed(child)
}

/// ffmpeg publishing a test pattern, H.264 and AAC, in real time for `secs`, to `url` as
/// `format`.
fn publish(url: &str, format: &str, secs: u32, extra: &[&str]) -> Child {
    Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-re"])
        .args(["-f", "lavfi", "-i", "testsrc=size=320x240:rate=25"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=44100"])
        .args(["-t", &secs.to_string()])
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
        ])
        .args(["-g", "25", "-pix_fmt", "yuv420p", "-c:a", "aac"])
        .args(extra)
        .args(["-f", format, url])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

/// Whether ffprobe finds a video stream at `url`, tried until `within` passes.
fn plays(url: &str, extra: &[&str], within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        let out = Command::new("ffprobe")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-rw_timeout",
                "5000000",
            ])
            .args(extra)
            .args(["-show_entries", "stream=codec_name", "-of", "csv=p=0", url])
            .output()
            .unwrap();
        if String::from_utf8_lossy(&out.stdout).contains("h264") {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

/// Waits for `c` to exit and says whether it succeeded, killing it past `within`.
fn exits_ok(c: &mut Child, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if let Ok(Some(s)) = c.try_wait() {
            return s.success();
        }
        if Instant::now() > deadline {
            let _ = c.kill();
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// fit's `scripts/streaming-interop.sh`, the part a host's ffmpeg can run: ffmpeg publishes over
/// RTMP, RTSP and SRT (when its build has libsrt) with the token and is refused without; ffprobe
/// plays each back. The same against fit's binary when `AKIT_FIT_BIN` names it: akit and fit
/// answer the same commands the same way. Skipped without ffmpeg, as in CI.
#[test]
fn ffmpeg_publishes_and_plays() {
    if !have("ffmpeg", "rtmp") || !have("ffprobe", "") {
        eprintln!("ffmpeg or ffprobe with RTMP not found: skipped");
        return;
    }
    let srt = have("ffmpeg", "srt");
    let mut servers = vec![("akit".to_owned(), env!("CARGO_BIN_EXE_akit").to_owned())];
    if let Ok(fit) = std::env::var("AKIT_FIT_BIN") {
        servers.push(("fit".to_owned(), fit));
    }
    for (name, bin) in servers {
        let d = dir(&format!("ffmpeg-{name}"));
        let (rtmp, rtsp, srtp, web) = (free_tcp(), free_tcp(), free_udp(), free_tcp());
        let cfg = d.join("akit.json");
        std::fs::write(
            &cfg,
            format!(
                r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:{web}"}}],
                  "routes": [{{"match": {{"path": ["/hls/*"]}}, "handler": {{"module": "hls", "params": {{"prefix": "/hls/", "auth": "live", "target_duration": "1s"}}}}}},
                             {{"match": {{"path": ["/live/*"]}}, "handler": {{"module": "httpflv", "params": {{"auth": "live"}}}}}}],
                  "media_auth": [{{"name": "live", "public": ["play"],
                  "tokens": [{{"name": "studio", "token": "0123456789abcdef-publish", "publish": ["live/*"]}}]}}],
                  "streaming": {{"rtmp": {{"listen": "127.0.0.1:{rtmp}", "auth": "live"}},
                                 "rtsp": {{"listen": "127.0.0.1:{rtsp}", "auth": "live"}},
                                 "srt": {{"listen": "127.0.0.1:{srtp}", "auth": "live"}}}}}}"#
            ),
        )
        .unwrap();
        let _server = serve(&bin, &cfg, rtmp);
        eprintln!("{name}: ffmpeg against {bin} (srt: {srt})");
        let t = "0123456789abcdef-publish";
        // RTMP: refused without the token, published with it, played by anyone.
        let mut bad = publish(&format!("rtmp://127.0.0.1:{rtmp}/live/a"), "flv", 2, &[]);
        assert!(
            !exits_ok(&mut bad, Duration::from_secs(30)),
            "{name}: rtmp without a token"
        );
        let mut good = publish(
            &format!("rtmp://127.0.0.1:{rtmp}/live/a?token={t}"),
            "flv",
            8,
            &[],
        );
        assert!(
            plays(
                &format!("rtmp://127.0.0.1:{rtmp}/live/a"),
                &[],
                Duration::from_secs(20)
            ),
            "{name}: rtmp play"
        );
        assert!(
            plays(
                &format!("rtsp://127.0.0.1:{rtsp}/live/a"),
                &["-rtsp_transport", "tcp"],
                Duration::from_secs(20)
            ),
            "{name}: rtsp play of an rtmp stream"
        );
        // HTTP-FLV and HLS (fMP4, and MPEG-TS) of it.
        for url in [
            format!("http://127.0.0.1:{web}/live/a.flv"),
            format!("http://127.0.0.1:{web}/hls/live/a/index.m3u8"),
            format!("http://127.0.0.1:{web}/hls/live/a/index.m3u8?format=ts"),
        ] {
            assert!(plays(&url, &[], Duration::from_secs(20)), "{name}: {url}");
        }
        let _ = good.kill();
        let _ = good.wait();
        // RTSP: the token in the URL's query. ffmpeg's SETUP after ANNOUNCE names the track
        // after the query (`.../live/b?token=T/streamid=0`), which fit takes and akio-media's
        // RTSP server answers 404 until akio A264: the same command fails on akit for now.
        let mut bad = publish(
            &format!("rtsp://127.0.0.1:{rtsp}/live/b"),
            "rtsp",
            2,
            &["-rtsp_transport", "tcp"],
        );
        assert!(
            !exits_ok(&mut bad, Duration::from_secs(30)),
            "{name}: rtsp without a token"
        );
        let mut good = publish(
            &format!("rtsp://127.0.0.1:{rtsp}/live/b?token={t}"),
            "rtsp",
            8,
            &["-rtsp_transport", "tcp"],
        );
        let played = plays(
            &format!("rtmp://127.0.0.1:{rtmp}/live/b"),
            &[],
            Duration::from_secs(if name == "fit" { 20 } else { 5 }),
        );
        assert_eq!(
            played,
            name == "fit",
            "{name}: rtmp play of an rtsp stream published with a query token"
        );
        let _ = good.kill();
        let _ = good.wait();
        if srt {
            let url = |sid: &str| format!("srt://127.0.0.1:{srtp}?streamid={sid}&latency=200000");
            let mut bad = publish(&url("#!::r=live/c,m=publish"), "mpegts", 2, &[]);
            assert!(
                !exits_ok(&mut bad, Duration::from_secs(30)),
                "{name}: srt without a token"
            );
            let mut good = publish(
                &url(&format!("#!::r=live/c,m=publish,s={t}")),
                "mpegts",
                8,
                &[],
            );
            assert!(
                plays(
                    &format!("rtmp://127.0.0.1:{rtmp}/live/c"),
                    &[],
                    Duration::from_secs(20)
                ),
                "{name}: rtmp play of an srt stream"
            );
            assert!(
                plays(&url("live/c"), &[], Duration::from_secs(20)),
                "{name}: srt play"
            );
            let _ = good.kill();
            let _ = good.wait();
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// Whether ffmpeg has the muxer `name`.
fn have_muxer(name: &str) -> bool {
    Command::new("ffmpeg")
        .args(["-hide_banner", "-muxers"])
        .output()
        .is_ok_and(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(|l| l.split_whitespace().nth(1) == Some(name))
        })
}

/// ffmpeg's WHIP muxer publishing to `stream_whip` on the role's WebRTC port (I124): refused
/// without the token, published with it (H.264 without B-frames, and Opus), and played back over
/// RTMP and HLS. The same against fit's binary when `AKIT_FIT_BIN` names it. Skipped without an
/// ffmpeg that has the WHIP muxer.
#[test]
fn ffmpeg_publishes_over_whip() {
    if !have_muxer("whip") || !have("ffprobe", "") {
        eprintln!("ffmpeg's WHIP muxer or ffprobe not found: skipped");
        return;
    }
    let mut servers = vec![("akit".to_owned(), env!("CARGO_BIN_EXE_akit").to_owned())];
    if let Ok(fit) = std::env::var("AKIT_FIT_BIN") {
        servers.push(("fit".to_owned(), fit));
    }
    let t = "0123456789abcdef-publish";
    let whip = |url: &str, token: Option<&str>, secs: u32| {
        let mut c = Command::new("ffmpeg");
        c.args(["-hide_banner", "-loglevel", "error", "-re"])
            .args(["-f", "lavfi", "-i", "testsrc=size=320x240:rate=25"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"])
            .args(["-t", &secs.to_string()])
            .args([
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-tune",
                "zerolatency",
            ])
            .args([
                "-profile:v",
                "baseline",
                "-bf",
                "0",
                "-g",
                "25",
                "-pix_fmt",
                "yuv420p",
            ])
            .args(["-c:a", "libopus", "-ar", "48000", "-ac", "2"]);
        if let Some(t) = token {
            c.args(["-authorization", t]);
        }
        c.args(["-f", "whip", url])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    };
    for (name, bin) in servers {
        let d = dir(&format!("whip-{name}"));
        let (rtmp, rtc, web) = (free_tcp(), free_udp(), free_tcp());
        let cfg = d.join("akit.json");
        std::fs::write(
            &cfg,
            format!(
                r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:{web}"}}],
                  "routes": [{{"match": {{"path": ["/whip/*"]}}, "handler": {{"module": "stream_whip", "params": {{"auth": "live"}}}}}},
                             {{"match": {{"path": ["/hls/*"]}}, "handler": {{"module": "hls", "params": {{"prefix": "/hls/", "auth": "live", "target_duration": "1s"}}}}}}],
                  "media_auth": [{{"name": "live", "public": ["play"],
                  "tokens": [{{"name": "studio", "token": "{t}", "publish": ["live/*"]}}]}}],
                  "streaming": {{"rtmp": {{"listen": "127.0.0.1:{rtmp}", "auth": "live"}},
                                 "webrtc": {{"listen": "127.0.0.1:{rtc}"}}}}}}"#
            ),
        )
        .unwrap();
        let _server = serve(&bin, &cfg, rtmp);
        eprintln!("{name}: ffmpeg's WHIP muxer against {bin}");
        let url = format!("http://127.0.0.1:{web}/whip/live/w");
        let mut bad = whip(&url, None, 2);
        assert!(
            !exits_ok(&mut bad, Duration::from_secs(30)),
            "{name}: whip without a token"
        );
        let mut good = whip(&url, Some(t), 10);
        assert!(
            plays(
                &format!("rtmp://127.0.0.1:{rtmp}/live/w"),
                &[],
                Duration::from_secs(20)
            ),
            "{name}: rtmp play of a whip stream"
        );
        let hls = format!("http://127.0.0.1:{web}/hls/live/w/index.m3u8");
        assert!(
            plays(&hls, &[], Duration::from_secs(20)),
            "{name}: hls play of a whip stream"
        );
        let _ = good.kill();
        let _ = good.wait();
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// The role's metrics on the Admin API's `/metrics`, with akit's (I122).
#[test]
fn metrics_served() {
    use std::io::{Read, Write};
    let d = dir("metrics");
    let (rtmp, admin) = (free_tcp(), free_tcp());
    let cfg = d.join("akit.json");
    std::fs::write(
        &cfg,
        format!(
            r#"{{"admin": {{"address": "127.0.0.1:{admin}"}},
              "streaming": {{"rtmp": {{"listen": "127.0.0.1:{rtmp}"}}}}}}"#
        ),
    )
    .unwrap();
    let _akit = serve(env!("CARGO_BIN_EXE_akit"), &cfg, rtmp);
    let deadline = Instant::now() + Duration::from_secs(20);
    let body = loop {
        if let Ok(s) = std::net::TcpStream::connect(("127.0.0.1", admin)) {
            s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            // The Admin API takes its own address as the host (DNS rebinding).
            (&s).write_all(
                format!("GET /metrics HTTP/1.1\r\nHost: 127.0.0.1:{admin}\r\n\r\n").as_bytes(),
            )
            .unwrap();
            let mut r = BufReader::new(s);
            let mut status = String::new();
            r.read_line(&mut status).unwrap();
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0u8; len];
            r.read_exact(&mut body).unwrap();
            if status.starts_with("HTTP/1.1 200") {
                break String::from_utf8(body).unwrap();
            }
        }
        assert!(Instant::now() < deadline, "no /metrics");
        std::thread::sleep(Duration::from_millis(50));
    };
    for want in [
        "akit_streaming_streams{state=\"publishing\"} 0",
        "akit_streaming_published_total 0",
        "akit_streaming_auth_refused_total{protocol=\"srt\",reason=\"forbidden\"} 0",
        "akit_streaming_restarts_total 0",
    ] {
        assert!(body.contains(want), "{want}\n{body}");
    }
    let _ = std::fs::remove_dir_all(&d);
}
