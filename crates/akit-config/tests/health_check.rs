//! `upstreams[].health_check` (I35): fit's fields, defaults and checks, as fit's
//! `config.HealthCheck` and its `Validate` have them (fit's `config_test.go`, "health checks" and
//! `TestHealthCheckDefaults`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use akit_config::{
    DEFAULT_HEALTH_INTERVAL, DEFAULT_HEALTH_THRESHOLD, DEFAULT_HEALTH_TIMEOUT, HealthCheck,
    HealthCheckType, from_json,
};

/// The health check of a config of one upstream with `check` as its `health_check`.
fn check(check: &str) -> HealthCheck {
    let src = format!(
        r#"{{"upstreams": [{{"name": "app", "addresses": ["a:1"], "health_check": {check}}}]}}"#
    );
    match from_json(&src, None) {
        Ok(c) => c.upstreams[0].health_check.clone().expect("a health check"),
        Err(e) => panic!("unexpected errors:\n{e}"),
    }
}

#[test]
fn fit_defaults() {
    let h = check("{}");
    assert_eq!(h.kind, HealthCheckType::Http);
    assert_eq!(h.path, "/");
    assert_eq!(h.host, None);
    assert_eq!(h.port, None);
    assert!(h.expect_status.is_empty());
    assert_eq!(h.interval, DEFAULT_HEALTH_INTERVAL);
    assert_eq!(h.interval, Duration::from_secs(5));
    assert_eq!(h.timeout, DEFAULT_HEALTH_TIMEOUT);
    assert_eq!(h.timeout, Duration::from_secs(2));
    assert_eq!(h.jitter, Duration::from_millis(500));
    assert_eq!((h.fails, h.passes), (2, 2));
    assert_eq!(DEFAULT_HEALTH_THRESHOLD, 2);
    // 0 is the default, as fit's orDefault takes it.
    let h =
        check(r#"{"interval": "0s", "timeout": "0s", "jitter": "0s", "fails": 0, "passes": 0}"#);
    assert_eq!(
        (h.interval, h.timeout, h.jitter, h.fails, h.passes),
        (
            Duration::from_secs(5),
            Duration::from_secs(2),
            Duration::from_millis(500),
            2,
            2
        )
    );
    // Without an upstream's health_check, no probes.
    let c = from_json(
        r#"{"upstreams": [{"name": "app", "addresses": ["a:1"]}]}"#,
        None,
    )
    .unwrap();
    assert_eq!(c.upstreams[0].health_check, None);
}

#[test]
fn set_fields() {
    let h = check(
        r#"{"path": "/healthz?full=1", "host": "app.internal", "port": 8081,
            "expect_status": [200, 204], "interval": "1s", "timeout": "1s", "jitter": "1ms",
            "fails": 1, "passes": 3}"#,
    );
    assert_eq!(h.path, "/healthz?full=1");
    assert_eq!(h.host.as_deref(), Some("app.internal"));
    assert_eq!(h.port, Some(8081));
    assert_eq!(h.expect_status, [200, 204]);
    assert_eq!(h.interval, Duration::from_secs(1));
    assert_eq!(h.timeout, Duration::from_secs(1));
    assert_eq!(h.jitter, Duration::from_millis(1));
    assert_eq!((h.fails, h.passes), (1, 3));
    assert!(h.expects(204) && !h.expects(200 + 1) && !h.expects(301));
    let h = check(r#"{"type": "tcp", "port": 9}"#);
    assert_eq!(h.kind, HealthCheckType::Tcp);
    assert_eq!(h.port, Some(9));
    // Without expect_status: 200 to 399.
    let h = check("{}");
    assert!(h.expects(200) && h.expects(399) && !h.expects(199) && !h.expects(400));
}

/// fit's `config_test.go` case "health checks", with its positions.
#[test]
fn fits_checks() {
    let doc = r#"{"listeners": [{"name": "web", "address": ":80"}], "upstreams": [
  {"name": "u", "addresses": ["a:1"], "health_check": {"type": "tcp", "path": "/h", "host": "h", "expect_status": [200]}},
  {"name": "v", "addresses": ["a:1"], "health_check": {"path": "h", "host": "*.h", "timeout": "6s", "jitter": "5s"}},
  {"name": "w", "addresses": ["a:1"], "health_check": {"interval": "100ms"}}
]}"#;
    let e = from_json(doc, None).expect_err("expected errors");
    let got: Vec<String> = e
        .as_slice()
        .iter()
        .filter(|e| e.path.contains("health_check"))
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        got,
        [
            "2:79: upstreams[0].health_check.path: path is for http health checks; a tcp one only opens a connection",
            "2:93: upstreams[0].health_check.host: host is for http health checks; a tcp one only opens a connection",
            "2:115: upstreams[0].health_check.expect_status: expect_status is for http health checks; a tcp one only opens a connection",
            r#"3:64: upstreams[1].health_check.path: "h" does not start with '/'"#,
            r#"3:77: upstreams[1].health_check.host: "*.h" is not a host name or IP address"#,
            "3:95: upstreams[1].health_check.timeout: the timeout, 6s, is longer than the interval, 5s",
            "3:111: upstreams[1].health_check.jitter: the jitter, 5s, is not less than the interval, 5s",
            "4:55: upstreams[2].health_check.timeout: the timeout, 2s, is longer than the interval, 100ms",
        ]
    );
}

#[test]
fn bad_values() {
    for (check, path, msg) in [
        (
            r#"{"type": "udp"}"#,
            "type",
            r#""udp" is not one of "http", "tcp""#,
        ),
        (r#"{"port": 0}"#, "port", "0 is less than the minimum 1"),
        (
            r#"{"port": 65536}"#,
            "port",
            "65536 is greater than the maximum 65535",
        ),
        (
            r#"{"expect_status": [99]}"#,
            "expect_status[0]",
            "99 is less than the minimum 100",
        ),
        (
            r#"{"expect_status": [600]}"#,
            "expect_status[0]",
            "600 is greater than the maximum 599",
        ),
        (r#"{"fails": -1}"#, "fails", "-1 is less than the minimum 0"),
        (
            r#"{"interval": "x"}"#,
            "interval",
            r#""x" is not a duration, such as "10s" or "1m30s""#,
        ),
        (r#"{"headers": {}}"#, "headers", "unknown field"),
        (
            r#"{"host": "a b"}"#,
            "host",
            r#""a b" is not a host name or IP address"#,
        ),
        (
            r#"{"jitter": "6s"}"#,
            "jitter",
            "the jitter, 6s, is not less than the interval, 5s",
        ),
    ] {
        let src = format!(
            r#"{{"upstreams": [{{"name": "app", "addresses": ["a:1"], "health_check": {check}}}]}}"#
        );
        let e = from_json(&src, None).expect_err(check);
        let got: Vec<(String, String)> = e
            .as_slice()
            .iter()
            .map(|e| (e.path.clone(), e.msg.clone()))
            .collect();
        assert_eq!(
            got,
            [(format!("upstreams[0].health_check.{path}"), msg.to_owned())],
            "{check}"
        );
    }
}
