//! `telemetry` (I64): read as fit reads it, with its defaults and fit's checks
//! (`config_test.go`'s "telemetry settings" cases, `TestParseHeaders`); given by one file only.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use akit_config::{
    DEFAULT_METRICS_INTERVAL, DEFAULT_TELEMETRY_ENDPOINT, DEFAULT_TELEMETRY_SERVICE_NAME,
    DEFAULT_TELEMETRY_TIMEOUT, DEFAULT_TRACES_INTERVAL, DEFAULT_TRACES_MAX_QUEUE_BYTES, Format,
    MetricsExport, Source, Telemetry, from_json, from_sources, parse_headers,
};

fn telemetry(doc: &str) -> Result<Option<Telemetry>, Vec<String>> {
    from_json(doc, Some("t.json"))
        .map(|c| c.telemetry)
        .map_err(|e| e.as_slice().iter().map(ToString::to_string).collect())
}

#[test]
fn defaults_and_values() {
    assert_eq!(from_json("{}", None).unwrap().telemetry, None);
    let t = telemetry(r#"{"telemetry": {"traces": {}}}"#)
        .unwrap()
        .unwrap();
    assert_eq!(t.endpoint, DEFAULT_TELEMETRY_ENDPOINT);
    assert_eq!(DEFAULT_TELEMETRY_ENDPOINT, "http://127.0.0.1:4318");
    assert_eq!(t.service_name, DEFAULT_TELEMETRY_SERVICE_NAME);
    assert_eq!(t.timeout, DEFAULT_TELEMETRY_TIMEOUT);
    let tr = t.traces.unwrap();
    assert_eq!(tr.sample_ratio, 0.0);
    assert_eq!(tr.max_queue_bytes, DEFAULT_TRACES_MAX_QUEUE_BYTES);
    assert_eq!(tr.interval, DEFAULT_TRACES_INTERVAL);
    assert_eq!(t.metrics, None);

    let t = telemetry(
        r#"{"telemetry": {"endpoint": "https://otel.example:4318", "headers_file": "h",
            "ca_file": "ca.pem", "cert_file": "c.pem", "key_file": "k.pem",
            "service_name": "edge", "timeout": "3s",
            "traces": {"sample_ratio": 0.25, "max_queue_bytes": 1024, "interval": "250ms"},
            "metrics": {"interval": "1m"}}}"#,
    )
    .unwrap()
    .unwrap();
    assert_eq!(t.endpoint, "https://otel.example:4318");
    assert_eq!(
        (&*t.headers_file, &*t.ca_file, &*t.cert_file, &*t.key_file),
        ("h", "ca.pem", "c.pem", "k.pem")
    );
    assert_eq!(
        (&*t.service_name, t.timeout),
        ("edge", Duration::from_secs(3))
    );
    let tr = t.traces.unwrap();
    assert_eq!(
        (tr.sample_ratio, tr.max_queue_bytes, tr.interval),
        (0.25, 1024, Duration::from_millis(250))
    );
    assert_eq!(
        t.metrics,
        Some(MetricsExport {
            interval: Duration::from_secs(60)
        })
    );
    let t = telemetry(r#"{"telemetry": {"metrics": {}, "timeout": "0s"}}"#)
        .unwrap()
        .unwrap();
    assert_eq!(t.metrics.unwrap().interval, DEFAULT_METRICS_INTERVAL);
    assert_eq!(t.timeout, DEFAULT_TELEMETRY_TIMEOUT);
    // An IPv6 host, in brackets.
    assert!(telemetry(r#"{"telemetry": {"endpoint": "http://[::1]:4318", "traces": {}}}"#).is_ok());
}

/// fit's "telemetry settings" cases: the same messages, at the same places.
#[test]
fn refused() {
    let e = telemetry(
        r#"{"listeners": [{"name": "web", "address": ":80"}], "telemetry": {"endpoint": "grpc://x:4317", "cert_file": "c"}}"#,
    )
    .unwrap_err();
    assert_eq!(
        e,
        [
            r#"t.json:1:78: telemetry.endpoint: "grpc://x:4317" is neither http://<host>:<port> nor https://<host>:<port>"#,
            "t.json:1:108: telemetry.cert_file: cert_file needs key_file",
            "t.json:1:65: telemetry: set traces, metrics or both: telemetry exports nothing without them",
        ]
    );
    let e = telemetry(
        r#"{"listeners": [{"name": "web", "address": ":80"}], "telemetry": {"traces": {"sample_ratio": 1.5}}}"#,
    )
    .unwrap_err();
    assert_eq!(
        e,
        ["t.json:1:93: telemetry.traces.sample_ratio: 1.5 is greater than the maximum 1"]
    );
    for (fields, want) in [
        (
            r#""endpoint": "http://x""#,
            r#"telemetry.endpoint: "http://x" is not a host and port alone"#,
        ),
        (
            r#""endpoint": "http://x:4318/v1""#,
            r#"telemetry.endpoint: "http://x:4318/v1" is not a host and port alone"#,
        ),
        (
            r#""endpoint": "http://x:0""#,
            r#"telemetry.endpoint: "http://x:0" is not a host and port alone"#,
        ),
        (
            r#""key_file": "k""#,
            "telemetry.key_file: key_file needs cert_file",
        ),
        (
            r#""traces": {"max_queue_bytes": -1}"#,
            "telemetry.traces.max_queue_bytes: -1 is less than the minimum 0",
        ),
        (
            r#""traces": {"interval": "soon"}"#,
            r#"telemetry.traces.interval: "soon" is not a duration"#,
        ),
        (
            r#""traces": {"bogus": 1}"#,
            "telemetry.traces.bogus: unknown field",
        ),
    ] {
        let e = telemetry(&format!(
            r#"{{"telemetry": {{"metrics": {{}}, {fields}}}}}"#
        ))
        .unwrap_err();
        assert_eq!(e.len(), 1, "{fields}: {e:?}");
        let got = e[0].splitn(4, ':').nth(3).unwrap_or("").trim_start();
        assert!(got.starts_with(want), "{fields}: {e:?}");
    }
}

/// As fit's other settings, `telemetry` is given by one file.
#[test]
fn one_file_only() {
    let a = r#"{"telemetry": {"traces": {}}}"#;
    let b = r#"{"telemetry": {"metrics": {}}}"#;
    let e = from_sources(&[
        Source {
            name: Some("a.json"),
            src: a,
            format: Format::Json,
        },
        Source {
            name: Some("b.json"),
            src: b,
            format: Format::Json,
        },
    ])
    .unwrap_err();
    let e: Vec<String> = e.as_slice().iter().map(ToString::to_string).collect();
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(
        e[0].starts_with("b.json:") && e[0].contains("also given in a.json"),
        "{e:?}"
    );
}

/// fit's `TestParseHeaders`.
#[test]
fn headers_file() {
    let h = parse_headers(
        "# comment\nX-Api-Key: s3cret\n\n  Authorization:Bearer a:b  \nX-Api-Key: two\n",
    )
    .unwrap();
    assert_eq!(
        h,
        [
            ("X-Api-Key".to_owned(), "s3cret".to_owned()),
            ("Authorization".to_owned(), "Bearer a:b".to_owned()),
            ("X-Api-Key".to_owned(), "two".to_owned()),
        ]
    );
    for bad in ["no colon", ": no name", "Bad Name: v"] {
        assert_eq!(parse_headers(&format!("Ok: 1\n{bad}")), Err(2), "{bad:?}");
    }
}
