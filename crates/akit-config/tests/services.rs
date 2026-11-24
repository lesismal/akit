//! `services` (I51): fit's layer 4 services, their fields, defaults and checks, as fit's
//! `config.Service` and its `Validate` have them, and the `tcp` and `udp` protocols of the
//! listeners they take; I52: services' `server_names` and `proxy_protocol`, and listeners'
//! `proxy_protocol`, as fit's `config` checks them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use akit_config::{
    Config, DEFAULT_SERVICE_IDLE_TIMEOUT, DEFAULT_SERVICE_MAX_CONNS,
    DEFAULT_SERVICE_UDP_IDLE_TIMEOUT, Network, Protocol, ProxyVersion, from_json,
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
            r#"{"name": "a", "listeners": ["t"], "upstream": "app", "access": {}}"#,
            &["services[0].access: not supported yet"],
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

/// Each problem of `doc(listeners, services)`, sorted, against `want`, for each case.
fn check_cases(cases: &[(&str, &str, &[&str])]) {
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
fn server_names_and_proxy_protocol() {
    // Three services share a tcp listener: two by name, one without (fit's sniConfig).
    let c = ok(&doc(
        r#"{"name": "t", "address": ":7000", "protocols": ["tcp"],
            "proxy_protocol": {"trusted": ["127.0.0.0/8", "::1", "::ffff:10.0.0.1"]}}"#,
        r#"{"name": "a", "listeners": ["t"], "upstream": "app", "server_names": ["A.Example.com"], "proxy_protocol": "v1"},
           {"name": "w", "listeners": ["t"], "upstream": "app", "server_names": ["*.w.example.com"], "proxy_protocol": "v2"},
           {"name": "f", "listeners": ["t"], "upstream": "app", "server_names": [], "proxy_protocol": ""}"#,
    ));
    let (a, w, f) = (&c.services[0], &c.services[1], &c.services[2]);
    assert_eq!(a.server_names, ["a.example.com"]);
    assert_eq!(w.server_names, ["*.w.example.com"]);
    assert!(f.server_names.is_empty());
    assert_eq!(
        (a.proxy_protocol, w.proxy_protocol, f.proxy_protocol),
        (Some(ProxyVersion::V1), Some(ProxyVersion::V2), None)
    );
    let pp = c.listeners[0].proxy_protocol.as_ref().unwrap();
    assert_eq!(
        pp.trusted,
        [
            Network::parse("127.0.0.0/8").unwrap(),
            Network::parse("::1").unwrap(),
            Network::parse("10.0.0.1").unwrap(),
        ]
    );
    for (ip, want) in [
        ("127.9.9.9", true),
        ("::1", true),
        ("10.0.0.1", true),
        ("::ffff:127.0.0.1", true),
        ("10.0.0.2", false),
        ("::2", false),
    ] {
        assert_eq!(pp.trusts(ip.parse().unwrap()), want, "{ip}");
    }
    // A listener of HTTP takes PROXY headers too.
    let c = ok(
        r#"{"listeners": [{"name": "web", "address": ":80", "proxy_protocol": {"trusted": ["10.0.0.0/8"]}}]}"#,
    );
    assert!(c.listeners[0].proxy_protocol.is_some());

    let named = |n: &str, names: &str| {
        format!(
            r#"{{"name": "{n}", "listeners": ["t"], "upstream": "app", "server_names": [{names}]}}"#
        )
    };
    let tcp_pp = |pp: &str| {
        format!(
            r#"{{"name": "t", "address": ":7000", "protocols": ["tcp"], "proxy_protocol": {pp}}}"#
        )
    };
    check_cases(&[
        // Two by name and the fallback: fine; a name twice on one listener is not.
        (
            TCP,
            &format!(
                "{}, {}",
                named("a", r#""a.example.com""#),
                named("b", r#""A.example.com", "b.example.com""#)
            ),
            &[
                r#"services[1].server_names[0]: "a.example.com" is already service "a"'s on listener "t""#,
            ],
        ),
        (
            TCP,
            &format!(
                "{}, {}",
                named("a", r#""a.example.com""#),
                r#"{"name": "f", "listeners": ["t"], "upstream": "app"}, {"name": "g", "listeners": ["t"], "upstream": "app"}"#
            ),
            &[
                r#"services[2].listeners[0]: listener "t" is already service "f"'s: services that share a tcp listener take the connections of their server_names, and only one of them may have none"#,
            ],
        ),
        (
            TCP,
            &named(
                "a",
                r#""x.example", "X.example", "10.0.0.1", "bad name", "**.example.com", "*.""#,
            ),
            &[
                r#"services[0].server_names[1]: "X.example" is listed twice"#,
                r#"services[0].server_names[2]: "10.0.0.1" is an IP address; a ClientHello names host names"#,
                r#"services[0].server_names[3]: "bad name" is not a valid host name"#,
                r#"services[0].server_names[4]: "**.example.com": a server name's wildcard is "*.", one label, as in certificates"#,
                r#"services[0].server_names[5]: the host is empty"#,
            ],
        ),
        (
            UDP,
            r#"{"name": "a", "listeners": ["u"], "upstream": "app", "server_names": ["a.example"], "proxy_protocol": "v2"}"#,
            &[
                "services[0].server_names: server_names pick a service by the TLS ClientHello of a tcp connection; a service with a udp listener takes none",
                "services[0].proxy_protocol: the PROXY protocol is sent on tcp connections; a service with a udp listener sends none",
            ],
        ),
        (
            TCP,
            r#"{"name": "a", "listeners": ["t"], "upstream": "app", "proxy_protocol": "v3"}"#,
            &[r#"services[0].proxy_protocol: "v3" is not one of "v1", "v2""#],
        ),
        (
            &tcp_pp(r#"{"trusted": []}"#),
            r#"{"name": "a", "listeners": ["t"], "upstream": "app"}"#,
            &["listeners[0].proxy_protocol.trusted: at least one address is needed"],
        ),
        (
            &tcp_pp(r#"{"trusted": ["10.0.0.0/33", "x"], "other": 1}"#),
            r#"{"name": "a", "listeners": ["t"], "upstream": "app"}"#,
            &[
                r#"listeners[0].proxy_protocol.trusted[0]: "10.0.0.0/33" is not an IP address or CIDR range"#,
                r#"listeners[0].proxy_protocol.trusted[1]: "x" is not an IP address or CIDR range"#,
                r#"listeners[0].proxy_protocol.other: unknown field"#,
            ],
        ),
        (
            &tcp_pp("{}"),
            r#"{"name": "a", "listeners": ["t"], "upstream": "app"}"#,
            &[r#"listeners[0].proxy_protocol.trusted: missing required field"#],
        ),
        (
            r#"{"name": "u", "address": ":7001", "protocols": ["udp"], "proxy_protocol": {"trusted": ["10.0.0.1"]}}"#,
            r#"{"name": "a", "listeners": ["u"], "upstream": "app"}"#,
            &[
                "listeners[0].proxy_protocol: a PROXY protocol header comes at the start of a TCP connection; this listener takes UDP alone",
            ],
        ),
    ]);
}

/// I53: `tcp` beside HTTP on one listener (fit's "mixed" listener), in the clear and with tls,
/// beside h3 too: a service takes its connections, and routes its HTTP.
#[test]
fn tcp_beside_other_protocols() {
    for listener in [
        r#"{"name": "t", "address": ":7000", "protocols": ["http/1.1", "h2c", "tcp"]}"#,
        r#"{"name": "t", "address": ":7000", "protocols": ["http/1.1", "h2", "h3", "tcp"],
            "tls": {"certificates": [{"cert_file": "c", "key_file": "k"}]},
            "proxy_protocol": {"trusted": ["10.0.0.0/8"]}}"#,
    ] {
        let src = format!(
            r#"{{"listeners": [{listener}],
                "upstreams": [{{"name": "app", "addresses": ["127.0.0.1:9000"]}}],
                "services": [{{"name": "a", "listeners": ["t"], "upstream": "app"}},
                             {{"name": "b", "listeners": ["t"], "upstream": "app", "server_names": ["b.example"]}}],
                "routes": [{{"listeners": ["t"], "handler": {{"module": "respond"}}}}]}}"#
        );
        let c = ok(&src);
        assert!(
            c.listeners[0].protocols.contains(&Protocol::Tcp),
            "{listener}"
        );
        assert!(c.listeners[0].protocols.len() > 2, "{listener}");
    }
}
