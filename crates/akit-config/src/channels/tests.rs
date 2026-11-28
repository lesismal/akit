//! fit's `config/channels_test.go`, ported for the channels (I145): what is read, its defaults,
//! and fit's refusals; push targets (I146); encryption (I147).

use super::*;
use crate::from_json;

const HEAD: &str = r#"{"listeners": [{"name": "web", "address": ":8080"}], "media_library": [{"name": "films", "roots": ["/srv/films"]}], "streaming": "#;

fn load(doc: &str) -> Result<crate::Config, String> {
    from_json(&format!("{HEAD}{doc}}}"), None).map_err(|e| e.to_string())
}

/// fit's `TestChannels`, the channels' part.
#[test]
fn channels_read() {
    let c = load(
        r#"{"channels": [
	  {"name": "tv/films", "library": "films", "items": ["films/bbb", "sintel"], "order": "shuffle", "loop": false,
	   "schedule": [{"at": "20:00", "item": "sintel"}, {"at": "mon,wed-fri 09:30:15", "item": "films/bbb"}, {"at": "2026-12-31 23:00", "item": "sintel"}],
	   "time_zone": "Asia/Shanghai", "rendition": "720p", "read_ahead": "5s", "max_buffer_bytes": 2097152},
	  {"name": "tv/news", "library": "films", "schedule": [{"at": "12:00", "item": "news"}]}]}"#,
    )
    .unwrap();
    let s = c.streaming.unwrap();
    let ch = &s.channels[0];
    assert_eq!(ch.zone(), "Asia/Shanghai");
    assert!(!ch.looped && ch.shuffle);
    assert_eq!(ch.read_ahead_or(), Duration::from_secs(5));
    assert_eq!(ch.buffer_bytes(), 2 << 20);
    assert_eq!(ch.rendition, "720p");
    assert_eq!(ch.schedule[0].at.clock, 20 * 3600);
    assert_eq!(ch.schedule[0].at.days, 0x7f);
    assert_eq!(ch.schedule[1].at.days, 0b0011_1010);
    assert_eq!(ch.schedule[1].at.clock, 9 * 3600 + 30 * 60 + 15);
    assert_eq!(ch.schedule[2].at.date, Some((2026, 12, 31)));
    let news = &s.channels[1];
    assert_eq!(news.zone(), "UTC");
    assert!(news.looped && !news.shuffle);
    assert_eq!(news.read_ahead_or(), DEFAULT_CHANNEL_READ_AHEAD);
    assert_eq!(news.buffer_bytes(), DEFAULT_CHANNEL_MAX_BUFFER_BYTES);
    assert!(!ch.encrypt && ch.rotation() == DEFAULT_CHANNEL_KEY_ROTATION);
    assert_eq!(
        StreamChannel {
            key_rotation: 3,
            ..StreamChannel::default()
        }
        .rotation(),
        3
    );
    // I147: encrypt, with the library's secret.
    let c = from_json(
        &format!(
            "{}{}}}",
            HEAD.replace(r#""roots": ["/srv/films"]"#, r#""roots": ["/srv/films"], "secret": "0123456789abcdef""#),
            r#"{"channels": [{"name": "x", "library": "films", "items": ["a"], "encrypt": true, "key_rotation": 5}]}"#
        ),
        None,
    )
    .unwrap();
    let ch = &c.streaming.unwrap().channels[0];
    assert!(ch.encrypt && ch.rotation() == 5);
}

/// fit's refusals of channels.
#[test]
fn channels_refused() {
    let long = format!(
        r#"{{"channels": [{{"name": "x", "library": "films", "items": ["{}"]}}]}}"#,
        "a".repeat(300)
    );
    for (doc, want) in [
        (
            r#"{"channels": [{"name": "/x", "library": "films", "items": ["a"]}]}"#,
            "not a stream's name",
        ),
        (
            r#"{"channels": [{"name": "x/../y", "library": "films", "items": ["a"]}]}"#,
            "not a stream's name",
        ),
        (
            r#"{"channels": [{"name": "x", "library": "films", "items": ["a"]}, {"name": "x", "library": "films", "items": ["a"]}]}"#,
            "already named",
        ),
        (
            r#"{"channels": [{"name": "x", "library": "none", "items": ["a"]}]}"#,
            r#"no media_library named "none""#,
        ),
        (
            r#"{"channels": [{"name": "x", "library": "films"}]}"#,
            "neither is set",
        ),
        (
            r#"{"channels": [{"name": "x", "library": "films", "items": [""]}]}"#,
            "may not be empty",
        ),
        (
            r#"{"channels": [{"name": "x", "library": "films", "items": ["/a"]}]}"#,
            "not an item's ID",
        ),
        (
            r#"{"channels": [{"name": "x", "library": "films", "items": ["a b"]}]}"#,
            "not an item's ID",
        ),
        (long.as_str(), "at most 256"),
        (
            r#"{"channels": [{"name": "x", "library": "films", "order": "random", "items": ["a"]}]}"#,
            "random",
        ),
        (
            r#"{"channels": [{"name": "x", "library": "films", "schedule": [{"at": "25:00", "item": "a"}]}]}"#,
            "is not a time",
        ),
        (
            r#"{"channels": [{"name": "x", "library": "films", "schedule": [{"at": "someday 10:00", "item": "a"}]}]}"#,
            "not a day of the week",
        ),
        (
            r#"{"channels": [{"name": "x", "library": "films", "schedule": [{"at": "mon-xyz 10:00", "item": "a"}]}]}"#,
            "not a day of the week",
        ),
        (
            r#"{"channels": [{"name": "x", "library": "films", "schedule": [{"at": "10:00", "item": ""}]}]}"#,
            "may not be empty",
        ),
        (
            r#"{"channels": [{"name": "x", "library": "films", "items": ["a"], "time_zone": "../etc"}]}"#,
            "not a time zone",
        ),
        (
            r#"{"channels": [{"name": "x", "library": "films", "items": ["a"], "read_ahead": "100ms"}]}"#,
            "500ms to 1m",
        ),
        (
            r#"{"channels": [{"name": "x", "library": "films", "items": ["a"], "max_buffer_bytes": 1000}]}"#,
            "at least 1048576",
        ),
        (
            r#"{"channels": [{"name": "x", "library": "films", "items": ["a"], "encrypt": true}]}"#,
            r#"set media_library "films"'s secret"#,
        ),
        (
            r#"{"channels": [{"name": "x", "library": "films", "items": ["a"], "key_rotation": 100001}]}"#,
            "key_rotation",
        ),
        ("{}", "nothing takes streams in"),
    ] {
        match load(doc) {
            Ok(_) => panic!("{doc}: loaded, want {want:?}"),
            Err(e) => assert!(e.contains(want), "{doc}: {e}, want {want:?}"),
        }
    }
    let many: Vec<String> = (0..=MAX_CHANNELS)
        .map(|i| format!(r#"{{"name": "c{i}", "library": "films", "items": ["a"]}}"#))
        .collect();
    let e = load(&format!(r#"{{"channels": [{}]}}"#, many.join(","))).unwrap_err();
    assert!(e.contains("at most 256"), "{e}");
}

/// fit's `TestChannelAt`, what is read; the times it falls on are akit-media's.
#[test]
fn channel_at() {
    let a = parse_channel_at("sat-mon 12:00").unwrap();
    assert_eq!((a.days, a.clock), (0b0100_0011, 12 * 3600));
    let a = parse_channel_at("FRI,sun 11:00").unwrap();
    assert_eq!(a.days, 0b0010_0001);
    let a = parse_channel_at("2026-11-07T09:00:30").unwrap();
    assert_eq!((a.date, a.clock), (Some((2026, 11, 7)), 9 * 3600 + 30));
    assert!(a.once());
    assert!(!parse_channel_at("10:00").unwrap().once());
    assert_eq!(parse_channel_at("9:05").unwrap().clock, 9 * 3600 + 5 * 60);
    for bad in [
        "",
        "10",
        "10:5",
        "24:00",
        "10:60",
        "2026-02-30 10:00",
        "mon",
        "mon- 10:00",
    ] {
        assert!(parse_channel_at(bad).is_err(), "{bad:?}");
    }
    assert!(valid_stream_path("a/b.c/_d~e-f"));
    for bad in ["", "a//b", ".a", &"a".repeat(300)] {
        assert!(!valid_stream_path(bad), "{bad:?}");
    }
    assert!(zone_name_form("America/Argentina/Buenos_Aires") && zone_name_form("Etc/GMT+8"));
    assert!(!zone_name_form("/etc") && !zone_name_form("a b"));
}

/// fit's `TestChannels`, the push targets' part, and `TestPushURL`.
#[test]
fn push_targets_read() {
    let c = load(
        r#"{"channels": [
	  {"name": "tv/films", "library": "films", "items": ["films/bbb"],
	   "push": [{"url": "rtmp://cdn.example.com/live/key"}, {"name": "srt-out", "url": "srt://203.0.113.5:9000?streamid=%23!::r=live/tv,m=publish&passphrase=0123456789&pbkeylen=32&latency=200&mode=caller"}]}],
	  "push": [{"stream": "live/cam", "url": "rtmp://ingest.example.com/app/key", "retry_min": "2s", "retry_max": "1m", "connect_timeout": "5s"},
	           {"stream": "tv/films", "url": "rtmp://a.example.com:1936/app/k2"}]}"#,
    )
    .unwrap();
    let s = c.streaming.unwrap();
    let names: Vec<String> = s
        .push_targets()
        .iter()
        .map(|t| format!("{}@{}@{}", t.push.name, t.push.stream, t.channel))
        .collect();
    assert_eq!(
        names.join(" "),
        "live/cam#1@live/cam@ tv/films#1@tv/films@ tv/films#2@tv/films@tv/films srt-out@tv/films@tv/films"
    );
    let t = s.push_targets();
    let p = &t[0].push;
    assert_eq!(
        (p.retry_min_or(), p.retry_max_or(), p.connect()),
        (
            Duration::from_secs(2),
            Duration::from_secs(60),
            Duration::from_secs(5)
        )
    );
    let q = &t[1].push;
    assert_eq!(
        (q.retry_min_or(), q.retry_max_or(), q.connect()),
        (
            DEFAULT_PUSH_RETRY_MIN,
            DEFAULT_PUSH_RETRY_MAX,
            DEFAULT_PUSH_CONNECT_TIMEOUT
        )
    );
    let low = StreamPush {
        retry_min: Duration::from_secs(60),
        retry_max: Duration::from_secs(1),
        ..StreamPush::default()
    };
    assert_eq!(low.retry_max_or(), Duration::from_secs(60));
    // The URL is a secret: Debug shows its scheme and host alone.
    assert!(!format!("{p:?}").contains("key"), "{p:?}");
    for (raw, want) in [
        ("rtmp://h/app/key?x=1", "rtmp://h:1935/app/…"),
        ("RTMP://h:1936/app/a/b", "rtmp://h:1936/app/…"),
        ("srt://h:9000?streamid=x", "srt://h:9000"),
    ] {
        assert_eq!(parse_push_url(raw).unwrap().redacted(), want, "{raw}");
    }
    let u = parse_push_url("srt://h:9000?streamid=s&passphrase=0123456789&pbkeylen=16&latency=300")
        .unwrap();
    assert_eq!(
        (
            u.stream_id.as_str(),
            u.passphrase.as_str(),
            u.pbkeylen,
            u.latency
        ),
        ("s", "0123456789", 16, Duration::from_millis(300))
    );
    assert_eq!(
        parse_push_url("srt://h:9000").unwrap().latency,
        DEFAULT_PUSH_SRT_LATENCY
    );
}

/// fit's refusals of push targets; none says the URL.
#[test]
fn push_targets_refused() {
    for (doc, want) in [
        (
            r#"{"channels": [{"name": "x", "library": "films", "items": ["a"], "push": [{"stream": "y", "url": "rtmp://h/a/b"}]}]}"#,
            "leave stream out",
        ),
        (
            r#"{"rtmp": {"listen": ":1935"}, "push": [{"url": "rtmp://h/a/b"}]}"#,
            "names the stream",
        ),
        (
            r#"{"rtmp": {"listen": ":1935"}, "push": [{"stream": "a b", "url": "rtmp://h/a/b"}]}"#,
            "not a stream's name",
        ),
        (
            r#"{"rtmp": {"listen": ":1935"}, "push": [{"name": "p", "stream": "a", "url": "rtmp://h/a/b"}, {"name": "p", "stream": "b", "url": "rtmp://h/a/b"}]}"#,
            "already named",
        ),
        (
            r#"{"rtmp": {"listen": ":1935"}, "push": [{"stream": "a", "url": "http://h/a/b"}]}"#,
            "rtmp://, rtmps:// or srt://",
        ),
        (
            r#"{"rtmp": {"listen": ":1935"}, "push": [{"stream": "a", "url": "rtmp:///a/b"}]}"#,
            "needs a host",
        ),
        (
            r#"{"rtmp": {"listen": ":1935"}, "push": [{"stream": "a", "url": "rtmp://h/a"}]}"#,
            "names an app and a stream",
        ),
        (
            r#"{"rtmp": {"listen": ":1935"}, "push": [{"stream": "a", "url": "rtmps://h/a/key"}]}"#,
            "not supported yet",
        ),
        (
            r#"{"rtmp": {"listen": ":1935"}, "push": [{"stream": "a", "url": "srt://h"}]}"#,
            "a host and a port",
        ),
        (
            r#"{"rtmp": {"listen": ":1935"}, "push": [{"stream": "a", "url": "srt://h:1/x"}]}"#,
            "has no path",
        ),
        (
            r#"{"rtmp": {"listen": ":1935"}, "push": [{"stream": "a", "url": "srt://h:1?foo=1"}]}"#,
            r#"not "foo""#,
        ),
        (
            r#"{"rtmp": {"listen": ":1935"}, "push": [{"stream": "a", "url": "srt://h:1?mode=listener"}]}"#,
            "mode may only be caller",
        ),
        (
            r#"{"rtmp": {"listen": ":1935"}, "push": [{"stream": "a", "url": "srt://h:1?passphrase=short"}]}"#,
            "10 to 79",
        ),
        (
            r#"{"rtmp": {"listen": ":1935"}, "push": [{"stream": "a", "url": "srt://h:1?pbkeylen=20"}]}"#,
            "16, 24 or 32",
        ),
        (
            r#"{"rtmp": {"listen": ":1935"}, "push": [{"stream": "a", "url": "srt://h:1?latency=5"}]}"#,
            "20 to 10000",
        ),
        (
            r#"{"rtmp": {"listen": ":1935"}, "push": [{"stream": "a", "url": "rtmp://h/a/b", "retry_min": "1m", "retry_max": "1s"}]}"#,
            "more than retry_max",
        ),
        (
            r#"{"rtmp": {"listen": ":1935"}, "push": [{"stream": "a", "url": "%%"}]}"#,
            "does not parse",
        ),
        (
            r#"{"push": [{"stream": "a", "url": "rtmp://h/a/b"}]}"#,
            "nothing takes streams in",
        ),
    ] {
        match load(doc) {
            Ok(_) => panic!("{doc}: loaded, want {want:?}"),
            Err(e) => {
                assert!(e.contains(want), "{doc}: {e}, want {want:?}");
                assert!(!e.contains("/key"), "an error holds the url: {e}");
            }
        }
    }
}
