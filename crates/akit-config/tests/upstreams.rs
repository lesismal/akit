//! `upstreams` (I14): fit's fields, defaults and checks, as fit's `config.Upstream` and its
//! `Validate` have them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use akit_config::{
    Config, DEFAULT_CONNECT_TIMEOUT, DEFAULT_IDLE_TIMEOUT, DEFAULT_MAX_CONNS,
    DEFAULT_MAX_IDLE_CONNS, DEFAULT_MAX_PENDING, Errors, Source, from_json, from_sources,
};

fn ok(src: &str) -> Config {
    match from_json(src, Some("t.json")) {
        Ok(c) => c,
        Err(e) => panic!("unexpected errors:\n{e}"),
    }
}

fn errs(src: &str) -> Errors {
    from_json(src, Some("t.json")).expect_err("expected errors")
}

/// Each problem as `path: msg`.
fn problems(src: &str) -> Vec<String> {
    errs(src)
        .as_slice()
        .iter()
        .map(|e| format!("{}: {}", e.path, e.msg))
        .collect()
}

/// A config of one upstream with `fields` after its name.
fn one(fields: &str) -> String {
    format!(r#"{{"upstreams": [{{"name": "app", {fields}}}]}}"#)
}

#[test]
fn fit_defaults() {
    let c = ok(&one(r#""addresses": ["127.0.0.1:9000", "backend:80"]"#));
    let u = &c.upstreams[0];
    assert_eq!(u.name, "app");
    assert_eq!(u.addresses, ["127.0.0.1:9000", "backend:80"]);
    // fit's: 10000, 50000, 32, 60s, 10s.
    assert_eq!(
        (u.max_conns, u.max_pending, u.max_idle_conns),
        (
            DEFAULT_MAX_CONNS,
            DEFAULT_MAX_PENDING,
            DEFAULT_MAX_IDLE_CONNS
        )
    );
    assert_eq!((DEFAULT_MAX_CONNS, DEFAULT_MAX_PENDING), (10_000, 50_000));
    assert_eq!(DEFAULT_MAX_IDLE_CONNS, 32);
    assert_eq!(u.idle_timeout, DEFAULT_IDLE_TIMEOUT);
    assert_eq!(u.connect_timeout, DEFAULT_CONNECT_TIMEOUT);
    assert_eq!(DEFAULT_IDLE_TIMEOUT, Duration::from_secs(60));
    assert_eq!(DEFAULT_CONNECT_TIMEOUT, Duration::from_secs(10));
}

#[test]
fn limits_are_read() {
    let c = ok(&one(
        r#""addresses": ["[::1]:8080"], "protocol": "http/1.1", "max_conns": 2,
           "max_pending": 3, "max_idle_conns": 0, "idle_timeout": "1m30s",
           "connect_timeout": "250ms""#,
    ));
    let u = &c.upstreams[0];
    assert_eq!((u.max_conns, u.max_pending, u.max_idle_conns), (2, 3, 0));
    assert_eq!(u.idle_timeout, Duration::from_secs(90));
    assert_eq!(u.connect_timeout, Duration::from_millis(250));
    // 0 is the default for max_conns, max_pending and the timeouts, as in fit; max_idle_conns 0
    // keeps none.
    let c = ok(&one(
        r#""addresses": ["a:1"], "max_conns": 0, "max_pending": 0, "idle_timeout": "0s",
           "connect_timeout": "0""#,
    ));
    let u = &c.upstreams[0];
    assert_eq!((u.max_conns, u.max_pending), (10_000, 50_000));
    assert_eq!(
        (u.idle_timeout, u.connect_timeout),
        (DEFAULT_IDLE_TIMEOUT, DEFAULT_CONNECT_TIMEOUT)
    );
}

#[test]
fn fit_checks() {
    assert_eq!(
        problems(&one(r#""addresses": []"#)),
        ["upstreams[0].addresses: an upstream needs at least one address"]
    );
    assert_eq!(
        problems(r#"{"upstreams": [{"name": "app"}]}"#),
        ["upstreams[0].addresses: missing required field"]
    );
    assert_eq!(
        problems(
            r#"{"upstreams": [{"name": "app", "addresses": ["a:1"]}, {"name": "app", "addresses": ["b:1"]}]}"#
        ),
        [r#"upstreams[1].name: another upstream is already named "app""#]
    );
    assert_eq!(
        problems(r#"{"upstreams": [{"name": "-x", "addresses": ["a:1"]}]}"#),
        [
            r#"upstreams[0].name: "-x" is not a valid name: use letters, digits, '_', '.' and '-', starting with a letter or digit"#
        ]
    );
    assert_eq!(
        problems(&one(
            r#""addresses": ["http://a:1", "a", ":80", "a:0", "a:x", "a:1", "a:1", "::1:80"]"#
        )),
        [
            r#"upstreams[0].addresses[0]: http://a:1 is a URL; write the host and port alone, as in "10.0.0.1:8080""#,
            r#"upstreams[0].addresses[1]: "a" is not a host and port, as in "10.0.0.1:8080""#,
            r#"upstreams[0].addresses[2]: ":80" has no host"#,
            r#"upstreams[0].addresses[3]: "a:0" has no valid port"#,
            r#"upstreams[0].addresses[4]: "a:x" has no valid port"#,
            r#"upstreams[0].addresses[6]: "a:1" is listed twice"#,
            r#"upstreams[0].addresses[7]: "::1:80" is not a host and port, as in "10.0.0.1:8080""#,
        ]
    );
    assert_eq!(
        problems(&one(
            r#""addresses": ["a:1"], "max_conns": -1, "idle_timeout": "10", "connect_timeout": "-1s""#
        )),
        [
            "upstreams[0].max_conns: -1 is less than the minimum 0",
            r#"upstreams[0].idle_timeout: "10" is not a duration, such as "10s" or "1m30s""#,
            r#"upstreams[0].connect_timeout: "-1s" is negative"#,
        ]
    );
    assert_eq!(
        problems(&one(r#""addresses": ["a:1"], "protocol": "h3""#)),
        [r#"upstreams[0].protocol: "h3" is not one of "http/1.1", "h2c", "h2""#]
    );
    assert_eq!(
        problems(&one(r#""addresses": ["a:1"], "Max_Conns": 1"#)),
        [r#"upstreams[0].Max_Conns: unknown field (did you mean "max_conns"?)"#]
    );
}

#[test]
fn what_akit_lacks_is_not_supported_yet() {
    let e = errs(&one(
        r#""addresses": ["a:1"], "protocol": "h2c", "weights": [1], "health_check": {},
           "response_timeout": "1s""#,
    ));
    assert!(e.only_not_supported_yet(), "{e}");
    let paths: Vec<&str> = e.as_slice().iter().map(|e| e.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "upstreams[0].protocol",
            "upstreams[0].weights",
            "upstreams[0].health_check",
            "upstreams[0].response_timeout",
        ]
    );
}

#[test]
fn upstreams_are_joined_across_files() {
    let a = r#"{"upstreams": [{"name": "a", "addresses": ["a:1"]}]}"#;
    let b = r#"{"upstreams": [{"name": "b", "addresses": ["b:1"]}, {"name": "a", "addresses": ["c:1"]}]}"#;
    let srcs = [
        Source {
            name: Some("a.json"),
            format: akit_config::Format::Json,
            src: a,
        },
        Source {
            name: Some("b.json"),
            format: akit_config::Format::Json,
            src: b,
        },
    ];
    let e = from_sources(&srcs).unwrap_err();
    assert_eq!(e.as_slice().len(), 1, "{e}");
    assert_eq!(
        e.as_slice()[0].to_string(),
        r#"b.json:1:53: upstreams[1].name: another upstream is already named "a""#
    );
    let c = from_sources(&srcs[..1]).unwrap();
    assert_eq!(c.upstreams.len(), 1);
}
