//! `acme` and `tls.acme` (I31): read as fit reads them, with fit's checks of the names; what akit
//! does not do yet (shared storage, on-demand TLS) refused as not supported yet;
//! `acme` given by one file only.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use akit_config::{
    Acme, Challenge, DEFAULT_ACME_DIRECTORY, DEFAULT_ACME_MAX_CONCURRENT,
    DEFAULT_ACME_REQUEST_TIMEOUT, DEFAULT_ACME_TIMEOUT, Format, Source, from_json, from_sources,
};

fn acme(fields: &str) -> Result<Option<Acme>, Vec<String>> {
    from_json(&format!(r#"{{"acme": {{{fields}}}}}"#), Some("t.json"))
        .map(|c| c.acme)
        .map_err(|e| e.as_slice().iter().map(ToString::to_string).collect())
}

/// A TLS listener whose `tls` is `tls`, and the top-level `acme` given by `top`.
fn listener(tls: &str, top: &str) -> Result<akit_config::Config, akit_config::Errors> {
    from_json(
        &format!(
            r#"{{"listeners": [{{"name": "web", "address": ":443", "tls": {{{tls}}}}}]{top}}}"#
        ),
        Some("t.json"),
    )
}

fn problems(tls: &str, top: &str) -> Vec<String> {
    listener(tls, top)
        .expect_err("expected errors")
        .as_slice()
        .iter()
        .map(|e| format!("{}: {}", e.path, e.msg))
        .collect()
}

#[test]
fn defaults_and_values() {
    assert_eq!(from_json("{}", None).unwrap().acme, None);
    let a = acme("").unwrap().unwrap();
    assert_eq!(a, Acme::default());
    assert_eq!(a.directory, DEFAULT_ACME_DIRECTORY);
    assert_eq!(
        DEFAULT_ACME_DIRECTORY,
        "https://acme-v02.api.letsencrypt.org/directory"
    );
    assert_eq!(
        (a.max_concurrent, a.timeout, a.request_timeout),
        (
            DEFAULT_ACME_MAX_CONCURRENT,
            DEFAULT_ACME_TIMEOUT,
            DEFAULT_ACME_REQUEST_TIMEOUT
        )
    );
    assert_eq!(
        (
            DEFAULT_ACME_MAX_CONCURRENT,
            DEFAULT_ACME_TIMEOUT,
            DEFAULT_ACME_REQUEST_TIMEOUT
        ),
        (2, Duration::from_secs(600), Duration::from_secs(30))
    );
    assert_eq!(a.challenges, [Challenge::Http01, Challenge::TlsAlpn01]);
    // 0 and empty are the defaults, as fit's.
    let a =
        acme(r#""directory": "", "max_concurrent": 0, "timeout": "0s", "request_timeout": "0s""#)
            .unwrap()
            .unwrap();
    assert_eq!(a, Acme::default());
    let a = acme(
        r#""directory": "https://127.0.0.1:14000/dir", "email": "ops@example.com",
           "ca_file": "ca.pem", "storage": "/var/lib/akit", "max_concurrent": 4,
           "timeout": "1m", "request_timeout": "5s", "challenges": ["http-01"]"#,
    )
    .unwrap()
    .unwrap();
    assert_eq!(a.directory, "https://127.0.0.1:14000/dir");
    assert_eq!(a.email, "ops@example.com");
    assert_eq!(a.ca_file, "ca.pem");
    assert_eq!(a.storage, "/var/lib/akit");
    assert_eq!(a.max_concurrent, 4);
    assert_eq!(a.timeout, Duration::from_secs(60));
    assert_eq!(a.request_timeout, Duration::from_secs(5));
    assert_eq!(a.challenges, [Challenge::Http01]);
}

#[test]
fn refused() {
    for (fields, want) in [
        (
            r#""directory": "ftp://ca/dir""#,
            r#"acme.directory: "ftp://ca/dir" is not an http or https URL"#,
        ),
        (
            r#""directory": "https://""#,
            r#"acme.directory: "https://" is not an http or https URL"#,
        ),
        (
            r#""challenges": ["dns-01"]"#,
            r#"acme.challenges[0]: "dns-01" is not one of "http-01", "tls-alpn-01""#,
        ),
        (
            r#""challenges": ["http-01", "http-01"]"#,
            r#"acme.challenges[1]: "http-01" is listed twice"#,
        ),
        (
            r#""max_concurrent": -1"#,
            "acme.max_concurrent: -1 is less than the minimum 0",
        ),
        (
            r#""timeout": "soon""#,
            r#"acme.timeout: "soon" is not a duration"#,
        ),
        (r#""bogus": 1"#, "acme.bogus: unknown field"),
    ] {
        let e = acme(fields).unwrap_err();
        assert_eq!(e.len(), 1, "{fields}: {e:?}");
        // Past the file and position, `t.json:1:<col>: `.
        let got = e[0].splitn(4, ':').nth(3).unwrap_or("").trim_start();
        assert!(got.starts_with(want), "{fields}: {e:?}");
        assert!(!e[0].contains("not supported yet"), "{fields}: {e:?}");
    }
}

#[test]
fn not_supported_yet() {
    for (fields, path) in [
        (r#""shared_storage": {}"#, "acme.shared_storage"),
        (r#""on_demand": {"ask": "http://x/"}"#, "acme.on_demand"),
    ] {
        let e = from_json(&format!(r#"{{"acme": {{{fields}}}}}"#), None).unwrap_err();
        assert!(e.only_not_supported_yet(), "{fields}: {e}");
        assert_eq!(e.as_slice()[0].path, path, "{fields}");
    }
}

#[test]
fn one_file_only() {
    let src = |name, src| Source {
        name: Some(name),
        format: Format::Json,
        src,
    };
    let e = from_sources(&[
        src("a.json", r#"{"acme": {}}"#),
        src("b.json", r#"{"acme": {}}"#),
    ])
    .unwrap_err();
    assert_eq!(e.as_slice().len(), 1);
    let msg = e.as_slice()[0].to_string();
    assert!(
        msg.starts_with("b.json:1:10: acme: also given in a.json"),
        "{msg}"
    );
}

#[test]
fn listener_names() {
    // Names alone, with no certificates: their certificates are obtained.
    let c = listener(r#""acme": ["a.example", "B.Example"]"#, "").unwrap();
    let tls = c.listeners[0].tls.as_ref().unwrap();
    assert!(tls.certificates.is_empty());
    assert_eq!(tls.acme, ["a.example", "B.Example"]);
    // With certificates from files too.
    let c = listener(
        r#""certificates": [{"cert_file": "a.pem", "key_file": "a.key"}], "acme": ["a.example"]"#,
        r#", "acme": {"email": "ops@example.com"}"#,
    )
    .unwrap();
    assert_eq!(c.listeners[0].tls.as_ref().unwrap().acme, ["a.example"]);
    assert_eq!(c.acme.unwrap().email, "ops@example.com");
}

#[test]
fn listener_names_as_fit_checks_them() {
    assert_eq!(
        problems(
            r#""acme": ["*.example.com", "10.0.0.1", "a.example", "A.example", "bad_name", ""]"#,
            ""
        ),
        [
            r#"listeners[0].tls.acme[0]: "*.example.com" is a wildcard, which the HTTP-01 challenge cannot obtain"#,
            r#"listeners[0].tls.acme[1]: "10.0.0.1" is an IP address; acme takes host names"#,
            r#"listeners[0].tls.acme[3]: "A.example" is listed twice"#,
            r#"listeners[0].tls.acme[4]: "bad_name" is not a valid host name"#,
            "listeners[0].tls.acme[5]: the host is empty",
        ]
    );
    assert_eq!(
        problems(r#""acme": [1]"#, ""),
        ["listeners[0].tls.acme[0]: expected a string, got a number"]
    );
}
