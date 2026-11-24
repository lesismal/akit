//! `upstreams` (I14): fit's fields, defaults and checks, as fit's `config.Upstream` and its
//! `Validate` have them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use akit_config::{
    Config, DEFAULT_CONNECT_TIMEOUT, DEFAULT_IDLE_TIMEOUT, DEFAULT_MAX_CONNS,
    DEFAULT_MAX_IDLE_CONNS, DEFAULT_MAX_PENDING, Errors, LoadBalancing, Source, from_json,
    from_sources,
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
fn load_balancing() {
    // fit's default: round robin, or weighted round robin when an address weighs other than 1.
    let u = &ok(&one(r#""addresses": ["a:1", "b:1"]"#)).upstreams[0];
    assert_eq!(u.load_balancing, LoadBalancing::RoundRobin);
    assert!(u.weights.is_empty());
    assert_eq!((u.weight(0), u.weight(1)), (1, 1));
    let u = &ok(&one(r#""addresses": ["a:1", "b:1"], "weights": [3, 1]"#)).upstreams[0];
    assert_eq!(u.load_balancing, LoadBalancing::WeightedRoundRobin);
    assert_eq!(u.weights, [3, 1]);
    let u = &ok(&one(r#""addresses": ["a:1", "b:1"], "weights": [1, 1]"#)).upstreams[0];
    assert_eq!(u.load_balancing, LoadBalancing::RoundRobin);
    for (name, lb) in [
        ("round_robin", LoadBalancing::RoundRobin),
        ("weighted_round_robin", LoadBalancing::WeightedRoundRobin),
        ("least_conn", LoadBalancing::LeastConn),
    ] {
        let c = ok(&one(&format!(
            r#""addresses": ["a:1"], "load_balancing": "{name}""#
        )));
        assert_eq!(c.upstreams[0].load_balancing, lb);
        assert_eq!(lb.name(), name);
    }
    let u = &ok(&one(
        r#""addresses": ["a:1", "b:1"], "weights": [1000, 1], "load_balancing": "least_conn""#,
    ))
    .upstreams[0];
    assert_eq!(
        (u.load_balancing, u.weights.as_slice()),
        (LoadBalancing::LeastConn, &[1000, 1][..])
    );

    // fit's checks and messages.
    assert_eq!(
        problems(&one(r#""addresses": ["a:1", "b:1"], "weights": [2]"#)),
        ["upstreams[0].weights: 1 weights for 2 addresses; give one weight for each address"]
    );
    assert_eq!(
        problems(&one(
            r#""addresses": ["a:1"], "weights": [2], "load_balancing": "round_robin""#
        )),
        [
            "upstreams[0].load_balancing: round_robin takes no weights; use weighted_round_robin or least_conn"
        ]
    );
    assert_eq!(
        problems(&one(r#""addresses": ["a:1", "b:1"], "weights": [0, 1001]"#)),
        [
            "upstreams[0].weights[0]: 0 is less than the minimum 1",
            "upstreams[0].weights[1]: 1001 is greater than the maximum 1000",
        ]
    );
    assert_eq!(
        problems(&one(r#""addresses": ["a:1"], "load_balancing": "fastest""#)),
        [
            r#"upstreams[0].load_balancing: "fastest" is not one of "round_robin", "weighted_round_robin", "least_conn", "random", "p2c", "ip_hash", "uri_hash", "header_hash", "cookie_hash""#
        ]
    );
}

#[test]
fn load_balancing_v2() {
    for (name, lb) in [
        ("random", LoadBalancing::Random),
        ("p2c", LoadBalancing::P2c),
        ("ip_hash", LoadBalancing::IpHash),
        ("uri_hash", LoadBalancing::UriHash),
    ] {
        let u = &ok(&one(&format!(
            r#""addresses": ["a:1", "b:1"], "weights": [2, 1], "load_balancing": "{name}""#
        )))
        .upstreams[0];
        assert_eq!((u.load_balancing, u.hash_key.as_str()), (lb, ""));
        assert_eq!(lb.name(), name);
        assert_eq!(lb.is_hash(), name.ends_with("_hash"));
    }
    for (name, lb) in [
        ("header_hash", LoadBalancing::HeaderHash),
        ("cookie_hash", LoadBalancing::CookieHash),
    ] {
        let u = &ok(&one(&format!(
            r#""addresses": ["a:1"], "load_balancing": "{name}", "hash_key": "x-user""#
        )))
        .upstreams[0];
        assert_eq!((u.load_balancing, u.hash_key.as_str()), (lb, "x-user"));
        assert!(lb.is_hash());
    }
    assert!(
        ok(&one(r#""addresses": ["a:1"]"#)).upstreams[0]
            .sticky_cookie
            .is_none()
    );

    // fit's TestHashedPolicyConfig, with its messages.
    for (doc, want) in [
        (
            r#""addresses": ["a:1"], "load_balancing": "header_hash""#,
            "upstreams[0].hash_key: header_hash needs hash_key, the name of the header to hash",
        ),
        (
            r#""addresses": ["a:1"], "load_balancing": "cookie_hash", "hash_key": """#,
            "upstreams[0].hash_key: cookie_hash needs hash_key, the name of the cookie to hash",
        ),
        (
            r#""addresses": ["a:1"], "load_balancing": "cookie_hash", "hash_key": "a b""#,
            r#"upstreams[0].hash_key: "a b" is not a valid cookie name"#,
        ),
        (
            r#""addresses": ["a:1"], "hash_key": "x""#,
            "upstreams[0].hash_key: hash_key is for header_hash and cookie_hash",
        ),
        (
            r#""addresses": ["a:1"], "load_balancing": "p2c", "hash_key": "x""#,
            "upstreams[0].hash_key: hash_key is for header_hash and cookie_hash",
        ),
        (
            r#""addresses": ["a:1"], "sticky_cookie": {"name": "a;b", "path": "/"}"#,
            r#"upstreams[0].sticky_cookie.name: "a;b" is not a valid cookie name"#,
        ),
        (
            r#""addresses": ["a:1"], "sticky_cookie": {"path": "/a;b"}"#,
            r#"upstreams[0].sticky_cookie.path: "/a;b" is not a cookie path: it must start with '/' and hold no ';'"#,
        ),
        (
            r#""addresses": ["a:1"], "sticky_cookie": {"path": "x"}"#,
            r#"upstreams[0].sticky_cookie.path: "x" is not a cookie path: it must start with '/' and hold no ';'"#,
        ),
        (
            r#""addresses": ["a:1"], "sticky_cookie": {"secure": "yes"}"#,
            "upstreams[0].sticky_cookie.secure: expected a boolean, got a string",
        ),
    ] {
        assert_eq!(problems(&one(doc)), [want], "{doc}");
    }
    let e = errs(&one(
        r#""addresses": ["a:1"], "sticky_cookie": {"domain": "x"}"#,
    ));
    assert!(e.to_string().contains("sticky_cookie.domain"), "{e}");
}

#[test]
fn sticky_cookie() {
    let sticky = |fields: &str| {
        ok(&one(&format!(
            r#""addresses": ["a:1"], "sticky_cookie": {fields}"#
        )))
        .upstreams[0]
            .sticky_cookie
            .clone()
            .unwrap()
    };
    let s = sticky("{}");
    assert_eq!(
        (s.name.as_str(), s.path.as_str(), s.max_age, s.secure),
        (akit_config::DEFAULT_STICKY_COOKIE_NAME, "/", None, false)
    );
    assert_eq!(s.name, "fit_sticky");
    let s = sticky(r#"{"name": "s", "path": "/app", "max_age": "1h", "secure": true}"#);
    assert_eq!(
        (s.name.as_str(), s.path.as_str(), s.max_age, s.secure),
        ("s", "/app", Some(Duration::from_secs(3600)), true)
    );
    let s = sticky(r#"{"name": "", "path": "", "max_age": "0s"}"#);
    assert_eq!(
        (s.name.as_str(), s.path.as_str(), s.max_age),
        ("fit_sticky", "/", None)
    );
}

#[test]
fn what_akit_lacks_is_not_supported_yet() {
    let e = errs(&one(r#""addresses": ["a:1"],
           "response_idle_timeout": "1s""#));
    assert!(e.only_not_supported_yet(), "{e}");
    let paths: Vec<&str> = e.as_slice().iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, ["upstreams[0].response_idle_timeout",]);
}

/// I18: h2c and h2 upstreams.
#[test]
fn protocols() {
    use akit_config::UpstreamProtocol;
    for (name, want) in [
        ("http/1.1", UpstreamProtocol::Http11),
        ("h2c", UpstreamProtocol::H2c),
        ("h2", UpstreamProtocol::H2),
    ] {
        let c = from_json(
            &format!(
                r#"{{"upstreams": [{{"name": "a", "addresses": ["a:1"], "protocol": "{name}"}}]}}"#
            ),
            None,
        )
        .unwrap();
        assert_eq!(c.upstreams[0].protocol, want);
        assert_eq!(want.name(), name);
        assert_eq!(want.is_h2(), name != "http/1.1");
    }
    let c = from_json(
        r#"{"upstreams": [{"name": "a", "addresses": ["a:1"]}]}"#,
        None,
    )
    .unwrap();
    assert_eq!(c.upstreams[0].protocol, UpstreamProtocol::Http11);
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

/// `response_timeout` (I17): fit's 1h when left out or 0.
#[test]
fn response_timeout() {
    let u = &ok(&one(r#""addresses": ["a:1"]"#)).upstreams[0];
    assert_eq!(u.response_timeout, Duration::from_secs(3600));
    assert_eq!(
        akit_config::DEFAULT_RESPONSE_TIMEOUT,
        Duration::from_secs(3600)
    );
    let u = &ok(&one(r#""addresses": ["a:1"], "response_timeout": "2m""#)).upstreams[0];
    assert_eq!(u.response_timeout, Duration::from_secs(120));
    let u = &ok(&one(r#""addresses": ["a:1"], "response_timeout": "0s""#)).upstreams[0];
    assert_eq!(u.response_timeout, Duration::from_secs(3600));
    let p = problems(&one(r#""addresses": ["a:1"], "response_timeout": "-1s""#));
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].starts_with("upstreams[0].response_timeout: "), "{p:?}");
}

#[test]
fn passive_health_fields() {
    // fit's defaults: max_fails 1, fail_timeout 10s.
    let u = &ok(&one(r#""addresses": ["a:1"]"#)).upstreams[0];
    assert_eq!((u.max_fails, u.fail_timeout), (1, Duration::from_secs(10)));
    assert_eq!(
        (
            akit_config::DEFAULT_MAX_FAILS,
            akit_config::DEFAULT_FAIL_TIMEOUT
        ),
        (1, Duration::from_secs(10))
    );
    let u = &ok(&one(
        r#""addresses": ["a:1"], "max_fails": 3, "fail_timeout": "30s""#,
    ))
    .upstreams[0];
    assert_eq!((u.max_fails, u.fail_timeout), (3, Duration::from_secs(30)));
    // max_fails 0 keeps every address in rotation; fail_timeout 0 is the default.
    let u = &ok(&one(
        r#""addresses": ["a:1"], "max_fails": 0, "fail_timeout": "0s""#,
    ))
    .upstreams[0];
    assert_eq!((u.max_fails, u.fail_timeout), (0, Duration::from_secs(10)));
    // fit's min 0.
    let p = problems(&one(r#""addresses": ["a:1"], "max_fails": -1"#));
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].starts_with("upstreams[0].max_fails: "), "{p:?}");
}
