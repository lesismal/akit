//! akit's own log: fit's formats, levels, zones and files (fit's `logs/zone_test.go` and
//! `logs_test.go`).
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::time::{Duration, UNIX_EPOCH};

use akit_config::{Log, LogFormat, LogLevel};

use super::{Attr, Level, Logger, Root, format, rfc3339_nano};
use crate::files::Files;
use crate::zone::Zone;

fn line(f: LogFormat, zone: &Zone, msg: &str) -> String {
    let mut b = Vec::new();
    // 2026-11-05 03:59:59.123456789 UTC.
    let t = UNIX_EPOCH + Duration::new(1_793_851_199, 123_456_789);
    format(
        &mut b,
        f,
        t,
        zone,
        Level::Warn,
        format_args!("{msg}"),
        "tls",
        &[("conn", Attr::Bare(&7u64)), ("peer", Attr::Str(&"a b"))],
    );
    String::from_utf8(b).unwrap()
}

#[test]
fn formats_in_the_zone() {
    let utc = Zone::utc();
    let east = Zone::fixed("east", 8 * 3600);
    assert_eq!(
        line(LogFormat::Line, &utc, "certificate obtained"),
        "2026-11-05 03:59:59.123 WARN  certificate obtained logger=tls conn=7 peer=\"a b\"\n"
    );
    assert_eq!(
        line(LogFormat::Line, &east, "a\nb"),
        "2026-11-05 11:59:59.123 WARN  a\\x0ab logger=tls conn=7 peer=\"a b\"\n"
    );
    assert_eq!(
        line(LogFormat::Text, &east, "two words"),
        "time=2026-11-05T11:59:59.123+08:00 level=WARN msg=\"two words\" logger=tls conn=7 peer=\"a b\"\n"
    );
    assert_eq!(
        line(LogFormat::Json, &utc, "say \"hi\""),
        "{\"time\":\"2026-11-05T03:59:59.123456789Z\",\"level\":\"WARN\",\"msg\":\"say \\\"hi\\\"\",\"logger\":\"tls\",\"conn\":7,\"peer\":\"a b\"}\n"
    );
    let mut b = Vec::new();
    rfc3339_nano(&mut b, 1_793_851_199, 500_000_000, -3600);
    assert_eq!(String::from_utf8(b).unwrap(), "2026-11-05T02:59:59.5-01:00");
    let mut b = Vec::new();
    rfc3339_nano(&mut b, 1_793_851_199, 0, 0);
    assert_eq!(String::from_utf8(b).unwrap(), "2026-11-05T03:59:59Z");
}

fn config(json: &str) -> Log {
    akit_config::from_json(
        &format!(r#"{{"listeners": [{{"name": "w", "address": ":1"}}], "log": {json}}}"#),
        None,
    )
    .unwrap()
    .log
    .unwrap()
}

#[test]
fn levels_and_files() {
    let dir = std::env::temp_dir().join(format!("akit-logger-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("akit.log");
    let c = config(&format!(
        r#"{{"level": "warn", "levels": {{"tls": "debug"}}, "format": "json",
            "loggers": {{"engine": {{"output": "{}", "format": "line"}}}}}}"#,
        file.display()
    ));
    let files = Files::new();
    let root = Root::new(Some(&c), Zone::utc(), &files).unwrap();
    assert!(root.enabled(Logger::Tls, Level::Debug));
    assert!(!root.enabled(Logger::Server, Level::Info));
    assert!(root.enabled(Logger::Server, Level::Warn));
    assert_eq!(root.level(Logger::Engine), LogLevel::Warn);
    root.write(Logger::Engine, Level::Info, format_args!("not written"));
    root.write(Logger::Engine, Level::Error, format_args!("written"));
    let sinks = root.sinks();
    assert_eq!(sinks.len(), 1);
    sinks[0].1.sync().unwrap();
    drop(sinks);
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.ends_with(" ERROR written logger=engine\n"), "{text}");
    assert_eq!(text.lines().count(), 1);
    // A reload's levels.
    root.set_levels(Some(&config(r#"{"level": "debug"}"#)));
    assert!(root.enabled(Logger::Server, Level::Debug));
    // Closed: the file is let go of.
    root.close();
    assert!(files.list().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_shared_with_the_access_log() {
    let dir = std::env::temp_dir().join(format!("akit-logger-shared-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("all.log");
    let c = config(&format!(
        r#"{{"output": "{}", "rotate": {{}}}}"#,
        file.display()
    ));
    let files = Files::new();
    let root = Root::new(Some(&c), Zone::utc(), &files).unwrap();
    // The access log's open of the same path shares the sink; other settings are refused.
    let access = files
        .open(
            file.to_str().unwrap(),
            akit_config::rotation(c.rotate.as_ref()),
            Zone::utc(),
        )
        .unwrap();
    assert_eq!(files.list().len(), 1);
    assert!(
        files
            .open(file.to_str().unwrap(), None, Zone::utc())
            .is_err()
    );
    root.write(Logger::Server, Level::Info, format_args!("from the log"));
    access.write(b"from the access log\n").unwrap();
    access.sync().unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    assert_eq!(text.lines().count(), 2, "{text}");
    root.close();
    assert_eq!(files.list().len(), 1, "the access log still holds it");
    drop(access);
    assert!(files.list().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
