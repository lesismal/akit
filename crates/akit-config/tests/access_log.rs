//! `access_log` (I21): its fields with fit's defaults, in JSON, YAML and `.conf`; wrong values
//! with their paths; `rotate` refused as not supported yet; and given by one file only.
#![allow(clippy::panic, clippy::unwrap_used, clippy::indexing_slicing)]

use std::time::Duration;

use akit_config::{
    AccessLevel, AccessLog, DEFAULT_BUFFER_BYTES, DEFAULT_FLUSH_INTERVAL, Format, LogFormat,
    Source, from_conf, from_json, from_sources, from_yaml,
};

const BASE: &str = r#""listeners": [{"name": "web", "address": ":8080"}],
  "routes": [{"handler": {"module": "respond"}}]"#;

fn access_log(fields: &str) -> Result<Option<AccessLog>, Vec<String>> {
    from_json(&format!("{{{BASE}, \"access_log\": {{{fields}}}}}"), None)
        .map(|c| c.access_log)
        .map_err(|e| e.as_slice().iter().map(ToString::to_string).collect())
}

#[test]
fn defaults() {
    let a = access_log("").unwrap().unwrap();
    assert_eq!(
        a,
        AccessLog {
            output: "stdout".into(),
            flush_interval: DEFAULT_FLUSH_INTERVAL,
            buffer_bytes: DEFAULT_BUFFER_BYTES,
            level: AccessLevel::Info,
            format: LogFormat::Json,
            time_zone: String::new(),
        }
    );
    assert_eq!(a.min_status(), 0);
    // Zero stands for the default, as fit's.
    let a = access_log(r#""output": "", "flush_interval": "0s", "buffer_bytes": 0"#)
        .unwrap()
        .unwrap();
    assert_eq!(
        (a.output.as_str(), a.flush_interval, a.buffer_bytes),
        ("stdout", DEFAULT_FLUSH_INTERVAL, DEFAULT_BUFFER_BYTES)
    );
    assert!(
        from_json(&format!("{{{BASE}}}"), None)
            .unwrap()
            .access_log
            .is_none()
    );
}

#[test]
fn fields() {
    let a = access_log(
        r#""output": "/var/log/fit/access.log", "flush_interval": "250ms",
           "buffer_bytes": 65536, "level": "warn", "format": "line", "time_zone": "Asia/Shanghai""#,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        a,
        AccessLog {
            output: "/var/log/fit/access.log".into(),
            flush_interval: Duration::from_millis(250),
            buffer_bytes: 65536,
            level: AccessLevel::Warn,
            format: LogFormat::Line,
            time_zone: "Asia/Shanghai".into(),
        }
    );
    assert_eq!(a.min_status(), 400);
    let a = access_log(r#""level": "error", "format": "text""#)
        .unwrap()
        .unwrap();
    assert_eq!(
        (a.level, a.format, a.min_status()),
        (AccessLevel::Error, LogFormat::Text, 500)
    );
}

#[test]
fn errors() {
    for (fields, want) in [
        (
            r#""level": "debug""#,
            r#"access_log.level: "debug" is not one of "info", "warn", "error""#,
        ),
        (
            r#""format": "xml""#,
            r#"access_log.format: "xml" is not one of "json", "text", "line""#,
        ),
        (
            r#""buffer_bytes": -1"#,
            "access_log.buffer_bytes: -1 is less than the minimum 0",
        ),
        (
            r#""flush_interval": "1x""#,
            "access_log.flush_interval: \"1x\" is not a duration",
        ),
        (
            r#""output": 1"#,
            "access_log.output: expected a string, got",
        ),
        (
            r#""rotate": {"max_bytes": 1}"#,
            "access_log.rotate: not supported yet",
        ),
        (r#""outputs": "x""#, "access_log.outputs: unknown field"),
    ] {
        let e = access_log(fields).unwrap_err();
        assert_eq!(e.len(), 1, "{fields}: {e:?}");
        assert!(e[0].contains(want), "{fields}: {e:?}");
    }
}

#[test]
fn yaml_and_conf() {
    let y = from_yaml(
        "listeners:\n  - {name: web, address: \":8080\"}\nroutes:\n  - handler: {module: respond}\naccess_log:\n  output: stderr\n  format: text\n",
        None,
    )
    .unwrap();
    let a = y.access_log.unwrap();
    assert_eq!((a.output.as_str(), a.format), ("stderr", LogFormat::Text));
    let c = from_conf(
        "listener web :8080\nroute {\n\thandler respond\n}\naccess_log {\n\toutput stderr\n\tlevel error\n}\n",
        None,
    );
    match c {
        Ok(c) => {
            let a = c.access_log.unwrap();
            assert_eq!((a.output.as_str(), a.level), ("stderr", AccessLevel::Error));
        }
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn one_file_only() {
    let a = format!("{{{BASE}, \"access_log\": {{}}}}");
    let b = r#"{"access_log": {"format": "text"}}"#;
    let srcs = [
        Source {
            name: Some("a.json"),
            format: Format::Json,
            src: &a,
        },
        Source {
            name: Some("b.json"),
            format: Format::Json,
            src: b,
        },
    ];
    let e = from_sources(&srcs).unwrap_err();
    assert_eq!(e.as_slice().len(), 1, "{e}");
    assert_eq!(
        e.as_slice()[0].to_string(),
        "b.json:1:16: access_log: also given in a.json: only the lists at the top of a config (listeners, routes, upstreams, services, providers) may be split across files"
    );
}
