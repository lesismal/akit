//! HTTPS by default (I33): fit's `TestEffective`, and its checks of configs that lean on it
//! (config_test.go, more_test.go), ported.

#![allow(
    clippy::panic,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing
)]

use akit_config::{AutoHttps, Config, Errors, Protocol, from_json};

fn ok(src: &str) -> Config {
    match from_json(src, Some("t.json")) {
        Ok(c) => c,
        Err(e) => panic!("unexpected errors:\n{e}"),
    }
}

fn errs(src: &str) -> Errors {
    from_json(src, Some("t.json")).expect_err("expected errors")
}

/// Each problem as `line:col: path: msg` (or `path: msg`) without the file name.
fn lines(src: &str) -> Vec<String> {
    errs(src)
        .as_slice()
        .iter()
        .map(|e| {
            e.to_string()
                .trim_start_matches("t.json:")
                .trim_start()
                .to_owned()
        })
        .collect()
}

const ROUTES: &str = r#""routes": [
    {"match": {"host": ["A.example", "localhost", "*.w.example"]}, "handler": {"module": "respond"}},
    {"listeners": ["other"], "match": {"host": ["b.example"]}, "handler": {"module": "respond"}},
    {"match": {"host": ["a.example", "c.example"]}, "handler": {"module": "respond"}}
  ]"#;

fn acme(c: &Config, i: usize) -> Vec<&str> {
    c.listeners[i]
        .tls
        .as_ref()
        .unwrap()
        .acme
        .iter()
        .map(String::as_str)
        .collect()
}

/// Listeners written out are served as written, but for an empty `tls`, which gets the host
/// names of the routes it serves.
#[test]
fn written_listeners_are_kept() {
    let c = ok(&format!(
        r#"{{"listeners": [
    {{"name": "web", "address": ":8443", "tls": {{}}}},
    {{"name": "other", "address": ":9443", "tls": {{"acme": []}}}},
    {{"name": "files", "address": ":10443", "tls": {{"acme": ["z.example"]}}}},
    {{"name": "plain", "address": ":8080"}}
  ], {ROUTES}}}"#
    ));
    assert_eq!(c.listeners.len(), 4);
    assert_eq!(acme(&c, 0), ["a.example", "c.example"]);
    assert_eq!(acme(&c, 1), ["a.example", "b.example", "c.example"]);
    assert_eq!(acme(&c, 2), ["z.example"]);
    assert!(c.listeners[3].tls.is_none());
    assert!(c.listeners.iter().all(|l| l.redirect_https == 0));
    assert_eq!(c.auto_https, None);
}

/// A config that writes none is served on `https` and `http`, which routes may name.
#[test]
fn none_written_makes_https_and_http() {
    let routes = ROUTES.replace(r#""listeners": ["other"]"#, r#""listeners": ["https"]"#);
    let routes = routes.trim_end_matches(']');
    let c = ok(&format!(
        r#"{{"acme": {{"directory": "https://ca.example/dir"}}, {routes},
    {{"listeners": ["https"], "match": {{"host": ["d.example"]}}, "handler": {{"module": "respond"}}}}]}}"#
    ));
    let ls = &c.listeners;
    assert_eq!(ls.len(), 2);
    assert_eq!(
        (ls[0].name.as_str(), ls[0].address.as_str()),
        ("https", ":443")
    );
    assert_eq!(
        (ls[1].name.as_str(), ls[1].address.as_str()),
        ("http", ":80")
    );
    assert_eq!(
        acme(&c, 0),
        ["a.example", "b.example", "c.example", "d.example"]
    );
    assert_eq!(ls[0].protocols, [Protocol::Http11, Protocol::H2]);
    assert_eq!(ls[0].redirect_https, 0);
    assert!(ls[1].tls.is_none());
    assert_eq!(ls[1].protocols, [Protocol::Http11]);
    assert_eq!(ls[1].redirect_https, 443);
}

/// Other ports, and no redirects.
#[test]
fn ports_and_no_redirects() {
    let routes = ROUTES.replace(r#""listeners": ["other"]"#, r#""listeners": ["https"]"#);
    let c = ok(&format!(
        r#"{{"auto_https": {{"https_port": 8443, "http_port": 8080}}, {routes}}}"#
    ));
    let ls = &c.listeners;
    assert_eq!(
        (ls.len(), ls[0].address.as_str(), ls[1].address.as_str()),
        (2, ":8443", ":8080")
    );
    assert_eq!(ls[1].redirect_https, 8443);
    assert_eq!(
        c.auto_https,
        Some(AutoHttps {
            disable: false,
            disable_redirects: false,
            http_port: 8080,
            https_port: 8443,
        })
    );

    let c = ok(&format!(
        r#"{{"auto_https": {{"disable_redirects": true}}, {routes}}}"#
    ));
    assert_eq!(c.listeners.len(), 1);
    assert_eq!(c.listeners[0].name, "https");

    // 0 is the default.
    let c = ok(&format!(
        r#"{{"auto_https": {{"https_port": 0, "http_port": 0}}, {routes}}}"#
    ));
    assert_eq!(c.listeners[0].address, ":443");
    assert_eq!(c.listeners[1].address, ":80");
}

/// fit's checks of a config that leans on HTTPS by default (config_test.go, more_test.go).
#[test]
fn checks() {
    for (doc, want) in [
        (
            r#"{"listeners": [], "routes": [{"handler": {"module": "respond"}}]}"#,
            "listeners: at least one listener is needed, or a route that matches a host name for HTTPS by default",
        ),
        (
            r#"{"routes": [{"match": {"host": ["localhost", "*.a.example", "10.0.0.1", "intranet"]}, "handler": {"module": "respond"}}]}"#,
            "listeners: at least one listener is needed, or a route that matches a host name for HTTPS by default",
        ),
        (
            r#"{"auto_https": {"disable": true}, "routes": [{"match": {"host": ["a.example"]}, "handler": {"module": "respond"}}]}"#,
            "listeners: at least one listener is needed",
        ),
        (
            r#"{"auto_https": {"http_port": 8443, "https_port": 8443}, "routes": [{"match": {"host": ["a.example"]}, "handler": {"module": "respond"}}]}"#,
            "1:30: auto_https.http_port: the HTTP port is the HTTPS port, 8443",
        ),
        (
            r#"{"listeners": [{"name": "web", "address": ":443", "tls": {}}], "routes": [{"match": {"host": ["localhost"]}, "handler": {"module": "respond"}}]}"#,
            "1:58: listeners[0].tls: a certificate, a name for acme or on_demand is needed, or a route on this listener that matches a host name for HTTPS by default",
        ),
        (
            r#"{"auto_https": {"disable": true}, "listeners": [{"name": "web", "address": ":443", "tls": {}}]}"#,
            "1:91: listeners[0].tls: a certificate, a name for acme or on_demand is needed",
        ),
        (
            r#"{"auto_https": {"https_port": 70000}, "routes": [{"match": {"host": ["a.example"]}, "handler": {"module": "respond"}}]}"#,
            "1:31: auto_https.https_port: 70000 is greater than the maximum 65535",
        ),
        (
            r#"{"auto_https": {"disable": 1, "redirect": true}}"#,
            "1:28: auto_https.disable: expected a boolean, got a number",
        ),
    ] {
        let got = lines(doc);
        assert_eq!(
            got.first().map(String::as_str),
            Some(want),
            "{doc}: {got:?}"
        );
    }

    // A route on a listener HTTPS by default does not make.
    let e = errs(
        r#"{"routes": [{"listeners": ["web"], "match": {"host": ["a.example"]}, "handler": {"module": "respond"}}]}"#,
    );
    assert_eq!(e.as_slice().len(), 1, "{e}");
    assert_eq!(e.as_slice()[0].path, "routes[0].listeners[0]");
    assert_eq!(e.as_slice()[0].msg, "there is no listener named \"web\"");

    // A misspelt field.
    assert!(
        errs(r#"{"auto_https": {"disabled": true}}"#)
            .to_string()
            .contains("auto_https.disabled: unknown field")
    );
}

/// A config with no listeners and no routes is left alone: akit started with only the Admin API.
#[test]
fn nothing_to_serve_is_left_alone() {
    assert!(ok("{}").listeners.is_empty());
    assert!(
        ok(r#"{"auto_https": {"disable": true}}"#)
            .listeners
            .is_empty()
    );
}
