//! The `proxy` handler's params (I15): what is read, fit's checks and messages, what is not
//! supported yet, and trusted proxies parsed as fit's `parsePrefix` parses them.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::net::IpAddr;

use akit_config::{Handler, Network, from_json};

fn config(params: &str) -> String {
    format!(
        r#"{{"listeners": [{{"name": "web", "address": ":8080"}}],
            "upstreams": [{{"name": "app", "addresses": ["127.0.0.1:9000"]}}],
            "routes": [{{"handler": {{"module": "proxy", "params": {params}}}}}]}}"#
    )
}

#[test]
fn reads() {
    let c = from_json(
        &config(
            r#"{"upstream": "app", "trusted_proxies": ["10.1.2.3/8", "::1", "::ffff:192.0.2.1"],
                "host_header": "internal.svc:8080", "timeout": "1m30s"}"#,
        ),
        None,
    )
    .unwrap();
    let Handler::Proxy(p) = &c.routes[0].handler else {
        unreachable!()
    };
    assert_eq!(p.upstream, "app");
    assert_eq!(p.host_header, "internal.svc:8080");
    let nets: Vec<String> = p.trusted_proxies.iter().map(ToString::to_string).collect();
    assert_eq!(nets, ["10.0.0.0/8", "::1/128", "192.0.2.1/32"]);
    assert_eq!(p.timeout, Some(std::time::Duration::from_secs(90)));
    // Left out, or 0: none (I17).
    for params in [
        r#"{"upstream": "app"}"#,
        r#"{"upstream": "app", "timeout": "0s"}"#,
    ] {
        let c = from_json(&config(params), None).unwrap();
        let Handler::Proxy(p) = &c.routes[0].handler else {
            unreachable!()
        };
        assert_eq!(p.timeout, None, "{params}");
    }
}

#[test]
fn tunnel_idle_timeout() {
    // I49: fit's 10 minutes when left out or 0.
    for (params, want) in [
        (r#"{"upstream": "app"}"#, 600_000),
        (
            r#"{"upstream": "app", "tunnel_idle_timeout": "0s"}"#,
            600_000,
        ),
        (
            r#"{"upstream": "app", "tunnel_idle_timeout": "300ms"}"#,
            300,
        ),
    ] {
        let c = from_json(&config(params), None).unwrap();
        let Handler::Proxy(p) = &c.routes[0].handler else {
            unreachable!()
        };
        assert_eq!(p.tunnel_idle_timeout.as_millis(), want, "{params}");
    }
    assert_eq!(
        akit_config::DEFAULT_TUNNEL_IDLE_TIMEOUT,
        std::time::Duration::from_secs(600)
    );
    let e = from_json(
        &config(r#"{"upstream": "app", "tunnel_idle_timeout": "soon"}"#),
        None,
    )
    .unwrap_err()
    .to_string();
    assert!(
        e.contains(r#"routes[0].handler.params.tunnel_idle_timeout: "soon" is not a duration"#),
        "{e}"
    );
}

#[test]
fn errors() {
    for (params, want) in [
        (
            r#"{"upstream": "app", "trusted_proxies": ["10.0.0.0/8", "nope"]}"#,
            r#"routes[0].handler.params.trusted_proxies[1]: "nope" is not an IP address or a network, as in "10.0.0.1" or "10.0.0.0/8""#,
        ),
        (
            r#"{"upstream": "app", "host_header": "a/b"}"#,
            r#"routes[0].handler.params.host_header: "a/b" is not a host"#,
        ),
        (
            r#"{"trusted_proxies": []}"#,
            "routes[0].handler.params.upstream: upstream or upstreams is needed",
        ),
        (
            r#"{"upstream": "app", "timeout": "soon"}"#,
            r#"routes[0].handler.params.timeout: "soon" is not a duration"#,
        ),
        (
            r#"{"upstream": "app", "nope": 1}"#,
            "routes[0].handler.params.nope",
        ),
    ] {
        let e = from_json(&config(params), None).unwrap_err().to_string();
        assert!(e.contains(want), "{params}: {e}");
    }
}

#[test]
fn not_supported_yet() {
    for field in [
        r#""upstreams": [{"upstream": "app", "weight": 1}]"#,
        r#""failover": "app""#,
        r#""mirror": {"upstream": "app"}"#,
        r#""retry": {}"#,
        r#""splice_uploads": false"#,
    ] {
        let name = field.split('"').nth(1).unwrap();
        let params = if name == "upstreams" {
            format!("{{{field}}}")
        } else {
            format!(r#"{{"upstream": "app", {field}}}"#)
        };
        let e = from_json(&config(&params), None).unwrap_err();
        assert!(e.only_not_supported_yet(), "{name}: {e}");
        assert_eq!(e.as_slice().len(), 1, "{name}: {e}");
        assert_eq!(
            e.as_slice()[0].path,
            format!("routes[0].handler.params.{name}")
        );
    }
}

#[test]
fn networks() {
    let ip = |s: &str| s.parse::<IpAddr>().unwrap();
    for bad in [
        "",
        "10.0.0.0/",
        "10.0.0.0/33",
        "10.0.0.0/08",
        "10.0.0.0/+8",
        "::/129",
        "10.0.0",
        "a/8",
    ] {
        assert_eq!(Network::parse(bad), None, "{bad}");
    }
    let n = Network::parse("10.0.0.0/8").unwrap();
    assert!(n.contains(ip("10.255.0.1")));
    assert!(!n.contains(ip("11.0.0.1")));
    // IPv4 networks hold no IPv6 address, mapped or not, as in Go: callers unmap first.
    assert!(!n.contains(ip("::ffff:10.0.0.1")));
    let all = Network::parse("0.0.0.0/0").unwrap();
    assert!(all.contains(ip("192.0.2.1")) && !all.contains(ip("::1")));
    let v6 = Network::parse("2001:db8::/32").unwrap();
    assert!(v6.contains(ip("2001:db8:1::1")) && !v6.contains(ip("2001:db9::1")));
    // An address alone, IPv4-mapped, is taken as IPv4; a mapped network stays IPv6.
    assert_eq!(
        Network::parse("::ffff:10.0.0.1").unwrap().to_string(),
        "10.0.0.1/32"
    );
    assert_eq!(
        Network::parse("::ffff:10.0.0.0/104").unwrap().to_string(),
        "::ffff:10.0.0.0/104"
    );
}
