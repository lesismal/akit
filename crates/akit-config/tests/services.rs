//! `services` (I51): fit's layer 4 services, their fields, defaults and checks, as fit's
//! `config.Service` and its `Validate` have them, and the `tcp` and `udp` protocols of the
//! listeners they take.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use akit_config::{
    Config, DEFAULT_SERVICE_IDLE_TIMEOUT, DEFAULT_SERVICE_MAX_CONNS,
    DEFAULT_SERVICE_UDP_IDLE_TIMEOUT, Protocol, from_json,
};

fn ok(src: &str) -> Config {
    match from_json(src, Some("t.json")) {
        Ok(c) => c,
        Err(e) => panic!("unexpected errors:\n{e}"),
    }
}

/// Each problem as `path: msg`.
fn problems(src: &str) -> Vec<String> {
    from_json(src, Some("t.json"))
        .expect_err("expected errors")
        .as_slice()
        .iter()
        .map(|e| format!("{}: {}", e.path, e.msg))
        .collect()
}

/// A config with `listeners`, an upstream `app`, and `services`.
fn doc(listeners: &str, services: &str) -> String {
    format!(
        r#"{{"listeners": [{listeners}],
            "upstreams": [{{"name": "app", "addresses": ["127.0.0.1:9000"]}}],
            "services": [{services}]}}"#
    )
}

const TCP: &str = r#"{"name": "t", "address": ":7000", "protocols": ["tcp"]}"#;
const UDP: &str = r#"{"name": "u", "address": ":7001", "protocols": ["udp"]}"#;

#[test]
fn fields_and_defaults() {
    let c = ok(&doc(
        &format!("{TCP}, {UDP}"),
        r#"{"name": "a", "listeners": ["t"], "upstream": "app"},
           {"name": "b", "listeners": ["u"], "upstream": "app", "idle_timeout": "30s", "max_conns": 5}"#,
    ));
    assert_eq!(c.listeners[0].protocols, [Protocol::Tcp]);
    assert_eq!(c.listeners[1].protocols, [Protocol::Udp]);
    let (a, b) = (&c.services[0], &c.services[1]);
    assert_eq!((a.name.as_str(), a.upstream.as_str()), ("a", "app"));
    assert_eq!(a.listeners, ["t"]);
    assert_eq!(a.idle_timeout, None);
    assert_eq!(a.idle(), DEFAULT_SERVICE_IDLE_TIMEOUT);
    assert_eq!(a.udp_idle(), DEFAULT_SERVICE_UDP_IDLE_TIMEOUT);
    assert_eq!(a.max_conns, DEFAULT_SERVICE_MAX_CONNS);
    assert_eq!(DEFAULT_SERVICE_IDLE_TIMEOUT, Duration::from_secs(600));
    assert_eq!(DEFAULT_SERVICE_UDP_IDLE_TIMEOUT, Duration::from_secs(60));
    assert_eq!(b.idle(), Duration::from_secs(30));
    assert_eq!(b.udp_idle(), Duration::from_secs(30));
    assert_eq!(b.max_conns, 5);
    // 0 means the defaults, as fit reads them.
    let c = ok(&doc(
        TCP,
        r#"{"name": "a", "listeners": ["t"], "upstream": "app", "idle_timeout": "0s", "max_conns": 0}"#,
    ));
    assert_eq!(c.services[0].idle_timeout, None);
    assert_eq!(c.services[0].max_conns, DEFAULT_SERVICE_MAX_CONNS);
    // A service may take several tcp listeners, and a routes-only config is unaffected.
    let c = ok(&doc(
        &format!(r#"{TCP}, {{"name": "t2", "address": ":7002", "protocols": ["tcp"]}}"#),
        r#"{"name": "a", "listeners": ["t", "t2"], "upstream": "app"}"#,
    ));
    assert_eq!(c.services[0].listeners, ["t", "t2"]);
}

#[test]
fn checks() {
    let cases: &[(&str, &str, &[&str])] = &[
        (
            TCP,
            r#"{"name": "a", "listeners": [], "upstream": "app"}"#,
            &[
                "services[0].listeners: a service needs at least one tcp or udp listener",
                "listeners[0].protocols: no service takes this tcp listener's connections: list it in a service's listeners",
            ],
        ),
        (
            TCP,
            r#"{"name": "a", "listeners": ["t"], "upstream": "nope"}"#,
            &[r#"services[0].upstream: there is no upstream named "nope""#],
        ),
        (
            TCP,
            r#"{"name": "a", "listeners": ["t", "x"], "upstream": "app"}"#,
            &[r#"services[0].listeners[1]: there is no listener named "x""#],
        ),
        (
            &format!(r#"{TCP}, {{"name": "h", "address": ":80"}}"#),
            r#"{"name": "a", "listeners": ["t", "h"], "upstream": "app"}"#,
            &[
                r#"services[0].listeners[1]: "h" is not a tcp or udp listener: a service takes the connections of listeners whose protocols include tcp, or are ["udp"]"#,
            ],
        ),
        (
            TCP,
            r#"{"name": "a", "listeners": ["t"], "upstream": "app"},
               {"name": "a", "listeners": ["t"], "upstream": "app"}"#,
            &[
                r#"services[1].name: another service is already named "a""#,
                r#"services[1].listeners[0]: listener "t" is already service "a"'s: services that share a tcp listener take the connections of their server_names, and only one of them may have none"#,
            ],
        ),
        (
            UDP,
            r#"{"name": "a", "listeners": ["u"], "upstream": "app"},
               {"name": "b", "listeners": ["u"], "upstream": "app"}"#,
            &[r#"services[1].listeners[0]: listener "u" is already service "a"'s"#],
        ),
        (
            &format!("{TCP}, {UDP}"),
            r#"{"name": "a b", "listeners": ["t"], "upstream": "app"}"#,
            &[
                r#"services[0].name: "a b" is not a valid name: use letters, digits, '_', '.' and '-', starting with a letter or digit"#,
                "listeners[1].protocols: no service takes this udp listener's connections: list it in a service's listeners",
            ],
        ),
        (
            r#"{"name": "u", "address": ":7001", "protocols": ["udp", "h3"], "tls": {"certificates": [{"cert_file": "c", "key_file": "k"}]}}"#,
            r#"{"name": "a", "listeners": ["u"], "upstream": "app"}"#,
            &[
                "listeners[0].protocols[0]: a udp listener serves UDP alone: list udp without other protocols",
            ],
        ),
        (
            r#"{"name": "t", "address": ":7000", "protocols": ["tcp"], "tls": {"certificates": [{"cert_file": "c", "key_file": "k"}]}}"#,
            r#"{"name": "a", "listeners": ["t"], "upstream": "app"}"#,
            &[
                "listeners[0].tls: tls serves the listener's HTTP, and this one serves none: tcp and udp are proxied as they are, TLS and all",
            ],
        ),
        (
            TCP,
            r#"{"name": "a", "listeners": ["t"], "upstream": "app", "server_names": ["x.example"], "proxy_protocol": "v1", "access": {}}"#,
            &[
                "services[0].server_names: not supported yet",
                "services[0].access: not supported yet",
                "services[0].proxy_protocol: not supported yet",
            ],
        ),
        (
            r#"{"name": "t", "address": ":7000", "protocols": ["http/1.1", "tcp"]}"#,
            r#"{"name": "a", "listeners": ["t"], "upstream": "app"}"#,
            &[
                "listeners[0].protocols[1]: not supported yet: tcp beside other protocols on one port",
            ],
        ),
    ];
    let mut wrong = Vec::new();
    for (listeners, services, want) in cases {
        let mut got = problems(&doc(listeners, services));
        let mut want: Vec<String> = want.iter().map(|s| (*s).to_owned()).collect();
        got.sort();
        want.sort();
        if got != want {
            wrong.push(format!(
                "{listeners} {services}:\n  got  {got:?}\n  want {want:?}"
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn routes_on_a_layer_4_listener() {
    let src = format!(
        r#"{{"listeners": [{TCP}],
            "upstreams": [{{"name": "app", "addresses": ["127.0.0.1:9000"]}}],
            "services": [{{"name": "a", "listeners": ["t"], "upstream": "app"}}],
            "routes": [{{"listeners": ["t"], "handler": {{"module": "respond"}}}}]}}"#
    );
    assert_eq!(
        problems(&src),
        [
            r#"routes[0].listeners[0]: "t" is a tcp listener, which a service serves, not routes: it serves no HTTP"#
        ]
    );
}
