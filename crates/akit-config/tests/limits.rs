//! `limits` (I16, I17b): `max_body_bytes` read as fit reads it, carried by every listener, and
//! `write_stall_timeout`; its other fields refused as not supported yet; given by one file only.
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

#[test]
fn other_fields_not_supported_yet() {
    let e = limits(
        r#""max_body_bytes": 1, "max_pending_bytes": "25%", "max_header_bytes": 1, "max_conns": 1,
           "max_write_queue_bytes": 1, "write_stall_timeout": "1s""#,
    )
    .unwrap_err();
    assert!(e.only_not_supported_yet(), "{e}");
    let mut paths: Vec<&str> = e.as_slice().iter().map(|e| e.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(
        paths,
        [
            "limits.max_conns",
            "limits.max_header_bytes",
            "limits.max_pending_bytes",
            "limits.max_write_queue_bytes",
        ]
    );
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
