//! I55: fit's forward proxy, a listener's `socks` and the `forward_proxy` handler's params
//! (`config.ForwardProxy`), their fields, defaults and checks, ported from fit's
//! `config/config_test.go` ("listeners of many protocols, and socks").
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use akit_config::{
    Config, DEFAULT_FORWARD_CONNECT_TIMEOUT, DEFAULT_FORWARD_IDLE_TIMEOUT, Destination,
    ForwardProxy, Handler, Network, Protocol, from_json,
};

fn ok(src: &str) -> Config {
    match from_json(src, Some("t.json")) {
        Ok(c) => c,
        Err(e) => panic!("unexpected errors:\n{e}"),
    }
}

/// Each problem as fit prints it, `line:col: path: msg`.
fn problems(src: &str) -> Vec<String> {
    from_json(src, None)
        .expect_err("expected errors")
        .as_slice()
        .iter()
        .map(ToString::to_string)
        .collect()
}

#[test]
fn listeners_of_many_protocols_and_socks() {
    let doc = r#"{"listeners": [
  {"name": "one", "address": ":443", "protocols": ["http/1.1", "h2", "tcp", "socks"], "tls": {"certificates": [{"cert_file": "c", "key_file": "k"}]}, "socks": {"clients": ["10.0.0.0/8"]}},
  {"name": "nosocks", "address": ":1080", "protocols": ["socks"]},
  {"name": "extra", "address": ":1081", "socks": {"clients": [], "allow": ["x..y", "*.ok.example", "10.1.0.0/16", "nope/8"], "deny": ["1.2.3.4"], "ports": [0, 443, 70000]}},
  {"name": "s", "address": ":1082", "protocols": ["socks"], "socks": {"clients": ["bad"]}}
], "upstreams": [{"name": "a", "addresses": ["a:1"]}],
"services": [{"name": "pass", "listeners": ["one"], "upstream": "a", "server_names": ["a.example.com"]}],
"routes": [{"listeners": ["one", "s"], "handler": {"module": "respond"}}]}"#;
    assert_eq!(
        problems(doc),
        [
            r#"3:57: listeners[1].protocols[0]: socks needs the listener's socks, which says who may use it"#,
            r#"4:50: listeners[2].socks: socks is for a listener whose protocols include socks"#,
            r#"4:62: listeners[2].socks.clients: at least one address is needed"#,
            r#"4:76: listeners[2].socks.allow[0]: "x..y" is not a host name, an IP address or a CIDR range"#,
            r#"4:115: listeners[2].socks.allow[3]: "nope/8" is not a host name, an IP address or a CIDR range"#,
            r#"4:157: listeners[2].socks.ports[0]: 0 is not a port"#,
            r#"4:165: listeners[2].socks.ports[2]: 70000 is not a port"#,
            r#"5:83: listeners[3].socks.clients[0]: "bad" is not an IP address or CIDR range"#,
            // fit points at the listener's name in the route (8:34); akit at the route, as for
            // its tcp and udp listeners.
            r#"8:12: routes[0].listeners[1]: "s" is a socks listener, not routes: it serves no HTTP"#,
        ]
    );
}

#[test]
fn socks_fields_and_defaults() {
    let c = ok(r#"{"listeners": [
        {"name": "s", "address": ":1080", "protocols": ["socks"], "socks": {
            "clients": ["127.0.0.1", "10.0.0.0/8"],
            "allow": ["Example.com", "*.Example.org", "192.0.2.0/24"],
            "deny": ["169.254.169.254"], "ports": [80, 443],
            "connect_timeout": "3s", "idle_timeout": "1m"}},
        {"name": "d", "address": ":1081", "protocols": ["socks"], "socks": {"clients": ["::1"]}}]}"#);
    let l = &c.listeners[0];
    assert_eq!(l.protocols, [Protocol::Socks]);
    assert!(!Protocol::Socks.is_http());
    let s = l.socks.as_ref().unwrap();
    assert_eq!(
        s.clients,
        [
            Network::parse("127.0.0.1").unwrap(),
            Network::parse("10.0.0.0/8").unwrap()
        ]
    );
    assert_eq!(
        s.allow,
        [
            Destination::Name("example.com".into()),
            Destination::Suffix(".example.org".into()),
            Destination::Network(Network::parse("192.0.2.0/24").unwrap()),
        ]
    );
    assert_eq!(
        s.deny,
        [Destination::Network(
            Network::parse("169.254.169.254").unwrap()
        )]
    );
    assert_eq!(s.ports, [80, 443]);
    assert_eq!(s.connect_timeout, Duration::from_secs(3));
    assert_eq!(s.idle_timeout, Duration::from_secs(60));
    let d = c.listeners[1].socks.as_ref().unwrap();
    assert!(d.allow.is_empty() && d.deny.is_empty() && d.ports.is_empty());
    assert_eq!(d.connect_timeout, DEFAULT_FORWARD_CONNECT_TIMEOUT);
    assert_eq!(d.idle_timeout, DEFAULT_FORWARD_IDLE_TIMEOUT);
    assert_eq!(DEFAULT_FORWARD_CONNECT_TIMEOUT, Duration::from_secs(10));
    assert_eq!(DEFAULT_FORWARD_IDLE_TIMEOUT, Duration::from_secs(300));
    // Beside HTTP and tcp on one port, and with a PROXY header.
    let c = ok(
        r#"{"listeners": [{"name": "m", "address": ":80", "protocols": ["http/1.1", "tcp", "socks"],
        "proxy_protocol": {"trusted": ["10.0.0.1"]}, "socks": {"clients": ["0.0.0.0/0"]}}],
        "upstreams": [{"name": "a", "addresses": ["a:1"]}],
        "services": [{"name": "raw", "listeners": ["m"], "upstream": "a"}]}"#,
    );
    assert_eq!(
        c.listeners[0].protocols,
        [Protocol::Http11, Protocol::Tcp, Protocol::Socks]
    );
}

#[test]
fn socks_listener_misuse() {
    let p = problems(
        r#"{"listeners": [
  {"name": "s", "address": ":1080", "protocols": ["socks", "udp"], "socks": {"clients": ["::1"], "allow": ["[::1]", "**.a.example", "a.example."], "idle_timeout": "x"}},
  {"name": "t", "address": ":1081", "protocols": ["socks"], "socks": {"clients": ["::1"]}, "tls": {"certificates": [{"cert_file": "c", "key_file": "k"}]}}
], "upstreams": [{"name": "a", "addresses": ["a:1"]}],
"services": [{"name": "x", "listeners": ["t"], "upstream": "a"}]}"#,
    );
    let want = [
        r#"listeners[0].protocols[1]: a udp listener serves UDP alone"#,
        r#"listeners[0].socks.allow[0]: "[::1]" is not a host name"#,
        r#"listeners[0].socks.allow[1]: "**.a.example" is not a host name"#,
        r#"listeners[0].socks.allow[2]: "a.example." is not a host name"#,
        r#"listeners[0].socks.idle_timeout"#,
        r#"listeners[1].tls: tls serves the listener's HTTP, and this one serves none"#,
        r#"services[0].listeners[0]: "t" is not a tcp or udp listener"#,
        r#"listeners[0].protocols: no service takes this udp listener's connections"#,
    ];
    for w in want {
        assert!(p.iter().any(|e| e.contains(w)), "no {w:?} in {p:#?}");
    }
    assert_eq!(p.len(), want.len(), "{p:#?}");
}

#[test]
fn forward_proxy_params() {
    let c = ok(r#"{"listeners": [{"name": "p", "address": ":3128"}],
        "routes": [{"match": {"method": ["CONNECT"]}, "handler": {"module": "forward_proxy", "params": {
            "clients": ["127.0.0.1/32", "10.0.0.0/8"], "deny": ["169.254.169.254", "10.0.0.0/8"],
            "ports": [443], "connect_timeout": "10s", "idle_timeout": "5m"}}}]}"#);
    let Handler::ForwardProxy(f) = &c.routes[0].handler else {
        panic!("{:?}", c.routes[0].handler);
    };
    assert_eq!(
        *f,
        ForwardProxy {
            clients: vec![
                Network::parse("127.0.0.1").unwrap(),
                Network::parse("10.0.0.0/8").unwrap()
            ],
            allow: vec![],
            deny: vec![
                Destination::Network(Network::parse("169.254.169.254").unwrap()),
                Destination::Network(Network::parse("10.0.0.0/8").unwrap()),
            ],
            ports: vec![443],
            connect_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(300),
        }
    );
    assert_eq!(c.routes[0].handler.module(), "forward_proxy");
    let p = problems(
        r#"{"listeners": [{"name": "p", "address": ":3128"}],
            "routes": [{"handler": {"module": "forward_proxy"}},
            {"handler": {"module": "forward_proxy", "params": {"clients": ["x"], "nope": 1}}}]}"#,
    );
    assert_eq!(p.len(), 3, "{p:#?}");
    assert!(p[0].contains("routes[0].handler.params.clients: missing required field"));
    assert!(p[1].contains(r#"routes[1].handler.params.clients[0]: "x" is not an IP address"#));
    assert!(p[2].contains("routes[1].handler.params.nope: unknown field"));
}
