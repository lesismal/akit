//! `limits` (I16, I17b, I79): `max_body_bytes` and `max_header_bytes` read as fit reads them,
//! carried by every listener, `write_stall_timeout`, `max_conns`, `max_write_queue_bytes` and
//! (I79b) `max_pending_bytes`; given by one file only.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::PathBuf;

use akit_config::{DEFAULT_MAX_BODY_BYTES, from_files, from_json};

const LISTENERS: &str =
    r#""listeners": [{"name": "a", "address": ":1"}, {"name": "b", "address": ":2"}]"#;

fn limits(fields: &str) -> Result<akit_config::Config, akit_config::Errors> {
    from_json(
        &format!(r#"{{{LISTENERS}, "limits": {{{fields}}}}}"#),
        Some("t.json"),
    )
}

#[test]
fn max_body_bytes() {
    // fit's default when left out, or 0, or with no limits at all.
    for fields in ["", r#""max_body_bytes": 0"#] {
        let c = limits(fields).unwrap();
        assert_eq!(c.limits.max_body_bytes, DEFAULT_MAX_BODY_BYTES);
        assert_eq!(DEFAULT_MAX_BODY_BYTES, 16 << 20);
    }
    let c = from_json(&format!("{{{LISTENERS}}}"), None).unwrap();
    assert!(
        c.listeners
            .iter()
            .all(|l| l.max_body_bytes == DEFAULT_MAX_BODY_BYTES)
    );
    // Every listener carries it, as fit's runtime.Listener does.
    let c = limits(r#""max_body_bytes": 100000"#).unwrap();
    assert_eq!(c.limits.max_body_bytes, 100_000);
    assert!(c.listeners.iter().all(|l| l.max_body_bytes == 100_000));
    // fit's int64, at least 0.
    let e = limits(r#""max_body_bytes": -1"#).unwrap_err();
    assert_eq!(
        e.to_string(),
        "t.json:1:110: limits.max_body_bytes: -1 is less than the minimum 0"
    );
    let e = limits(r#""max_body_bytes": "lots""#).unwrap_err();
    assert!(!e.only_not_supported_yet(), "{e}");
}

/// `write_stall_timeout` (I17b): fit's 30s when left out or 0.
#[test]
fn write_stall_timeout() {
    use std::time::Duration;
    for fields in ["", r#""write_stall_timeout": "0s""#] {
        let c = limits(fields).unwrap();
        assert_eq!(c.limits.write_stall_timeout, Duration::from_secs(30));
    }
    assert_eq!(
        akit_config::DEFAULT_WRITE_STALL_TIMEOUT,
        Duration::from_secs(30)
    );
    let c = limits(r#""write_stall_timeout": "250ms""#).unwrap();
    assert_eq!(c.limits.write_stall_timeout, Duration::from_millis(250));
    let e = limits(r#""write_stall_timeout": "-1s""#).unwrap_err();
    assert!(!e.only_not_supported_yet(), "{e}");
    assert!(
        e.to_string().contains("limits.write_stall_timeout: "),
        "{e}"
    );
}

/// `max_header_bytes`, `max_conns` and `max_write_queue_bytes` (I79): fit's defaults when left
/// out or 0; `max_header_bytes` carried by every listener, as `max_body_bytes` is.
#[test]
fn header_conns_and_queue() {
    use akit_config::{
        DEFAULT_LIMITS_MAX_CONNS, DEFAULT_MAX_HEADER_BYTES, DEFAULT_MAX_WRITE_QUEUE_BYTES,
    };
    assert_eq!(DEFAULT_MAX_HEADER_BYTES, 1 << 20);
    assert_eq!(DEFAULT_LIMITS_MAX_CONNS, 100_000);
    assert_eq!(DEFAULT_MAX_WRITE_QUEUE_BYTES, 32 << 20);
    for fields in [
        "",
        r#""max_header_bytes": 0, "max_conns": 0, "max_write_queue_bytes": 0"#,
    ] {
        let c = limits(fields).unwrap();
        assert_eq!(c.limits.max_header_bytes, DEFAULT_MAX_HEADER_BYTES);
        assert_eq!(c.limits.max_conns, DEFAULT_LIMITS_MAX_CONNS);
        assert_eq!(
            c.limits.max_write_queue_bytes,
            DEFAULT_MAX_WRITE_QUEUE_BYTES
        );
        assert!(
            c.listeners
                .iter()
                .all(|l| l.max_header_bytes == DEFAULT_MAX_HEADER_BYTES)
        );
    }
    let c = limits(r#""max_header_bytes": 4096, "max_conns": 7, "max_write_queue_bytes": 65536"#)
        .unwrap();
    assert_eq!(c.limits.max_header_bytes, 4096);
    assert!(c.listeners.iter().all(|l| l.max_header_bytes == 4096));
    assert_eq!(c.limits.max_conns, 7);
    assert_eq!(c.limits.max_write_queue_bytes, 65536);
    for key in ["max_header_bytes", "max_conns", "max_write_queue_bytes"] {
        let e = limits(&format!(r#""{key}": -1"#)).unwrap_err();
        assert!(!e.only_not_supported_yet(), "{e}");
        assert!(
            e.to_string()
                .contains(&format!("limits.{key}: -1 is less than the minimum 0")),
            "{e}"
        );
    }
}

/// `max_pending_bytes` (I79b): a number of bytes or a share of memory, fit's `TestMemory` and
/// `TestLimits`; half the memory when left out or 0.
#[test]
fn max_pending_bytes() {
    use akit_config::{DEFAULT_MAX_PENDING_BYTES, FALLBACK_MAX_PENDING_BYTES, Memory};
    assert_eq!(DEFAULT_MAX_PENDING_BYTES, Memory::Share(5000));
    assert_eq!(FALLBACK_MAX_PENDING_BYTES, 256 << 20);
    for fields in ["", r#""max_pending_bytes": 0"#] {
        let c = limits(fields).unwrap();
        assert_eq!(c.limits.max_pending_bytes, DEFAULT_MAX_PENDING_BYTES);
    }
    let total = 64u64 << 30;
    for (v, want, bytes) in [
        ("1048576", Memory::Bytes(1 << 20), 1 << 20),
        (r#""25%""#, Memory::Share(2500), total / 4),
        (r#""12.5%""#, Memory::Share(1250), total / 8),
        (r#""100%""#, Memory::Share(10000), total),
        (r#""12.345%""#, Memory::Share(1235), 8_486_855_376),
    ] {
        let c = limits(&format!(r#""max_pending_bytes": {v}"#)).unwrap();
        let m = c.limits.max_pending_bytes;
        assert_eq!(m, want, "{v}");
        assert_eq!(m.bytes(Some(total), 9), bytes, "{v}");
        // A share where the memory cannot be read is the fallback; bytes are bytes.
        let unread = if matches!(m, Memory::Share(_)) {
            9
        } else {
            bytes
        };
        assert_eq!(m.bytes(None, 9), unread, "{v}");
        // Written back as a config writes it, the share rounded to hundredths of a percent.
        let written = if v == r#""12.345%""# {
            "12.35%"
        } else {
            v.trim_matches('"')
        };
        assert_eq!(m.to_string(), written, "{v}");
    }
    // A share is a byte at least.
    assert_eq!(Memory::Share(1).bytes(Some(10), 9), 1);
    for (v, msg) in [
        (r#""0%""#, r#""0%" is not more than 0% and at most 100%"#),
        (r#""0.001%""#, "not more than 0% and at most 100%"),
        (r#""101%""#, "not more than 0% and at most 100%"),
        (r#""NaN%""#, "not more than 0% and at most 100%"),
        (r#""50""#, r#""50" is not a share of memory, such as "50%""#),
        (r#"" 5%""#, "is not a share of memory"),
        (r#""half""#, "is not a share of memory"),
        ("-1", "-1 is not a number of bytes, 0 or more"),
        ("1.5", "is not a number of bytes, 0 or more"),
        (
            "true",
            "expected a number of bytes or a share of memory, got",
        ),
    ] {
        let e = limits(&format!(r#""max_pending_bytes": {v}"#)).unwrap_err();
        assert!(!e.only_not_supported_yet(), "{v}: {e}");
        let e = e.to_string();
        assert!(
            e.contains("limits.max_pending_bytes: ") && e.contains(msg),
            "{v}: {e}, want {msg:?}"
        );
    }
}

#[test]
fn every_field_is_read() {
    let c = limits(
        r#""max_body_bytes": 1, "max_pending_bytes": "25%", "max_header_bytes": 1, "max_conns": 1,
           "max_write_queue_bytes": 1, "write_stall_timeout": "1s""#,
    )
    .unwrap();
    assert_eq!(c.limits.max_pending_bytes, akit_config::Memory::Share(2500));
    // A field fit does not have is wrong, not "not supported yet".
    let e = limits(r#""max_bodies": 1"#).unwrap_err();
    assert!(!e.only_not_supported_yet(), "{e}");
}

#[test]
fn one_file_only() {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("limits");
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    let a = d.join("a.json");
    let b = d.join("b.json");
    let c = d.join("c.json");
    fs::write(
        &a,
        format!(r#"{{{LISTENERS}, "limits": {{"max_body_bytes": 5}}}}"#),
    )
    .unwrap();
    fs::write(&b, r#"{"limits": {"max_body_bytes": 5}}"#).unwrap();
    fs::write(&c, r#"{"routes": []}"#).unwrap();
    // Given by one file, it holds for the listeners of every file.
    let cfg = from_files(&[&c, &a]).unwrap();
    assert!(cfg.listeners.iter().all(|l| l.max_body_bytes == 5));
    // Given by two, even alike, it is refused in the second, as fit's adapter.Set refuses it.
    let e = from_files(&[&a, &b]).unwrap_err();
    assert_eq!(e.as_slice().len(), 1, "{e}");
    assert_eq!(
        e.as_slice()[0].to_string(),
        format!(
            "{}:1:12: limits: also given in {}: only the lists at the top of a config \
             (listeners, routes, upstreams, services, providers) may be split across files",
            b.display(),
            a.display()
        )
    );
    let _ = fs::remove_dir_all(&d);
}
