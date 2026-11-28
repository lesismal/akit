//! fit's `config/channels_test.go`, ported for the channels (I145): what is read, its defaults,
//! and fit's refusals; push targets (I146) and encryption (I147) are not supported yet.

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
            "not supported yet",
        ),
        (
            r#"{"channels": [{"name": "x", "library": "films", "items": ["a"], "push": [{"url": "rtmp://h/a/b"}]}]}"#,
            "not supported yet",
        ),
        (
            r#"{"rtmp": {"listen": ":1935"}, "push": [{"stream": "a", "url": "rtmp://h/a/b"}]}"#,
            "not supported yet",
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
