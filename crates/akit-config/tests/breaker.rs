//! The `circuit_breaker` middleware's params (I45): its defaults, and fit's checks
//! (TestBreakerParams).
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::time::Duration;

use akit_config::{CircuitBreaker, Handler, from_json};

fn config(params: &str) -> String {
    format!(
        r#"{{"listeners": [{{"name": "web", "address": ":8080"}}],
            "routes": [{{"middleware": [{{"module": "circuit_breaker"{params}}}],
                        "handler": {{"module": "respond"}}}}]}}"#
    )
}

fn breaker(params: &str) -> CircuitBreaker {
    let c = from_json(&config(params), None).unwrap();
    let Handler::CircuitBreaker(b) = &c.routes[0].middleware[0] else {
        unreachable!()
    };
    b.clone()
}

#[test]
fn defaults() {
    let d = CircuitBreaker::default();
    assert_eq!(d.window, Duration::from_secs(10));
    assert_eq!(d.min_requests, 20);
    assert_eq!(d.error_ratio, 0.5);
    assert!(d.statuses.is_empty());
    assert_eq!(d.slow_threshold, None);
    assert_eq!(d.slow_ratio, 0.5);
    assert_eq!(d.open_duration, Duration::from_secs(30));
    assert_eq!(d.half_open_requests, 5);
    assert_eq!(breaker(""), d);
    assert_eq!(breaker(r#", "params": {}"#), d);
    // 0, as leaving it out, is the default.
    assert_eq!(
        breaker(
            r#", "params": {"window": "0s", "error_ratio": 0, "slow_ratio": 0,
                "open_duration": "0s", "slow_threshold": "0s"}"#
        ),
        d
    );
}

#[test]
fn reads() {
    let b = breaker(
        r#", "params": {"window": "1m", "min_requests": 3, "error_ratio": 0.25,
            "statuses": [429, 503], "slow_threshold": "2s", "slow_ratio": 1,
            "open_duration": "5s", "half_open_requests": 1}"#,
    );
    assert_eq!(b.window, Duration::from_secs(60));
    assert_eq!(b.min_requests, 3);
    assert_eq!(b.error_ratio, 0.25);
    assert_eq!(b.statuses, [429, 503]);
    assert_eq!(b.slow_threshold, Some(Duration::from_secs(2)));
    assert_eq!(b.slow_ratio, 1.0);
    assert_eq!(b.open_duration, Duration::from_secs(5));
    assert_eq!(b.half_open_requests, 1);
}

#[test]
fn errors() {
    let at = "routes[0].middleware[0].params";
    for (params, want) in [
        (r#"{"statuses": [600]}"#, "statuses[0]: 600 is not a status"),
        (r#"{"statuses": [99]}"#, "statuses[0]: 99 is not a status"),
        (
            r#"{"error_ratio": 1.5}"#,
            "error_ratio: 1.5 is greater than the maximum 1",
        ),
        (
            r#"{"slow_ratio": -1}"#,
            "slow_ratio: -1 is less than the minimum 0",
        ),
        (
            r#"{"min_requests": 0}"#,
            "min_requests: 0 is less than the minimum 1",
        ),
        (
            r#"{"half_open_requests": 0}"#,
            "half_open_requests: 0 is less than the minimum 1",
        ),
        (
            r#"{"window": "soon"}"#,
            r#"window: "soon" is not a duration"#,
        ),
        (r#"{"nope": 1}"#, "nope"),
    ] {
        let e = from_json(&config(&format!(r#", "params": {params}"#)), None)
            .unwrap_err()
            .to_string();
        assert!(e.contains(&format!("{at}.{want}")), "{params}: {e}");
    }
}
