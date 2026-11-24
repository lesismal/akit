//! `providers` (I61): the `consul`, `consul_kv` and `etcd` kinds read as fit reads them, with
//! their defaults and fit's checks (`config_test.go`'s provider cases, `more_test.go`'s
//! `TestProviderChecks` and defaults).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use akit_config::{
    DEFAULT_CONSUL_ENDPOINT, DEFAULT_CONSUL_MAX_SERVICES, DEFAULT_CONSUL_TAG_PREFIX,
    DEFAULT_ETCD_ENDPOINT, DEFAULT_KV_MAX_KEYS, DEFAULT_KV_PREFIX, DEFAULT_WATCH_REFRESH_INTERVAL,
    DEFAULT_WATCH_REQUEST_TIMEOUT, Format, Provider, ProviderKind, Source, from_json, from_sources,
};

const WEB: &str = r#""listeners": [{"name": "web", "address": ":80"}]"#;

fn providers(doc: &str) -> Result<Vec<Provider>, Vec<String>> {
    from_json(doc, None)
        .map(|c| c.providers)
        .map_err(|e| e.as_slice().iter().map(ToString::to_string).collect())
}

fn errors(doc: &str) -> Vec<String> {
    providers(doc).expect_err("the config was taken")
}

#[test]
fn defaults() {
    let ps = providers(&format!(
        r#"{{{WEB}, "providers": [{{"consul": {{}}}}, {{"consul_kv": {{}}}}, {{"etcd": {{}}}}]}}"#
    ))
    .unwrap();
    assert_eq!(
        ps.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["consul", "consul_kv", "etcd"]
    );
    let ProviderKind::Consul(c) = &ps[0].kind else {
        panic!("{:?}", ps[0]);
    };
    assert_eq!(c.conn.endpoint, DEFAULT_CONSUL_ENDPOINT);
    assert_eq!(DEFAULT_CONSUL_ENDPOINT, "http://127.0.0.1:8500");
    assert_eq!(
        (&*c.tag_prefix, c.max_services, c.expose_all),
        (
            DEFAULT_CONSUL_TAG_PREFIX,
            DEFAULT_CONSUL_MAX_SERVICES,
            false
        )
    );
    assert_eq!(
        (c.conn.refresh_interval, c.conn.request_timeout),
        (
            DEFAULT_WATCH_REFRESH_INTERVAL,
            DEFAULT_WATCH_REQUEST_TIMEOUT
        )
    );
    assert_eq!(
        (
            DEFAULT_WATCH_REFRESH_INTERVAL,
            DEFAULT_WATCH_REQUEST_TIMEOUT
        ),
        (Duration::from_secs(30), Duration::from_secs(10))
    );
    let ProviderKind::ConsulKv(k) = &ps[1].kind else {
        panic!("{:?}", ps[1]);
    };
    assert_eq!(
        (&*k.conn.endpoint, &*k.prefix, k.max_keys),
        (
            DEFAULT_CONSUL_ENDPOINT,
            DEFAULT_KV_PREFIX,
            DEFAULT_KV_MAX_KEYS
        )
    );
    assert_eq!((DEFAULT_KV_PREFIX, DEFAULT_KV_MAX_KEYS), ("fit/", 1000));
    let ProviderKind::Etcd(e) = &ps[2].kind else {
        panic!("{:?}", ps[2]);
    };
    assert_eq!(e.endpoints, [DEFAULT_ETCD_ENDPOINT]);
    assert_eq!(DEFAULT_ETCD_ENDPOINT, "http://127.0.0.1:2379");
    assert_eq!(
        (&*e.prefix, e.max_keys),
        (DEFAULT_KV_PREFIX, DEFAULT_KV_MAX_KEYS)
    );
}

#[test]
fn values() {
    let ps = providers(&format!(
        r#"{{{WEB}, "providers": [
            {{"name": "cat", "consul": {{"endpoint": "https://consul.example:8501", "datacenter": "dc2",
              "token_file": "t", "ca_file": "ca.pem", "cert_file": "c.pem", "key_file": "k.pem",
              "expose_all": true, "tag_prefix": "app", "max_services": 5,
              "refresh_interval": "1m", "request_timeout": "3s"}}}},
            {{"name": "kv", "consul_kv": {{"endpoint": "http://10.0.0.1:8500", "prefix": "edge/", "max_keys": 7}}}},
            {{"name": "e", "etcd": {{"endpoints": ["http://10.0.0.71:2379", "https://10.0.0.72:2379"],
              "username": "fit", "password_file": "pw", "prefix": "x/", "max_keys": 0,
              "refresh_interval": "0s", "request_timeout": "2s"}}}}
        ]}}"#
    ))
    .unwrap();
    let ProviderKind::Consul(c) = &ps[0].kind else {
        panic!()
    };
    assert_eq!(ps[0].name, "cat");
    assert_eq!(
        (&*c.conn.endpoint, &*c.conn.datacenter, &*c.conn.token_file),
        ("https://consul.example:8501", "dc2", "t")
    );
    assert_eq!(
        (
            &*c.conn.tls.ca_file,
            &*c.conn.tls.cert_file,
            &*c.conn.tls.key_file
        ),
        ("ca.pem", "c.pem", "k.pem")
    );
    assert_eq!(
        (c.expose_all, &*c.tag_prefix, c.max_services),
        (true, "app", 5)
    );
    assert_eq!(
        (c.conn.refresh_interval, c.conn.request_timeout),
        (Duration::from_secs(60), Duration::from_secs(3))
    );
    let ProviderKind::ConsulKv(k) = &ps[1].kind else {
        panic!()
    };
    assert_eq!(
        (&*k.conn.endpoint, &*k.prefix, k.max_keys),
        ("http://10.0.0.1:8500", "edge/", 7)
    );
    let ProviderKind::Etcd(e) = &ps[2].kind else {
        panic!()
    };
    assert_eq!(
        e.endpoints,
        ["http://10.0.0.71:2379", "https://10.0.0.72:2379"]
    );
    assert_eq!(
        (&*e.username, &*e.password_file, &*e.prefix, e.max_keys),
        ("fit", "pw", "x/", DEFAULT_KV_MAX_KEYS)
    );
    assert_eq!(
        (e.refresh_interval, e.request_timeout),
        (DEFAULT_WATCH_REFRESH_INTERVAL, Duration::from_secs(2))
    );
}

/// fit's `config_test.go` cases about providers, at fit's positions, but for a name's problems:
/// fit reports them at the name's value (1:75 in its cases), akit at the provider's object.
#[test]
fn checks() {
    for (doc, want) in [
        (
            format!(r#"{{{WEB}, "providers": [{{}}]}}"#),
            vec![
                "1:66: providers[0]: set exactly one kind of provider: docker, consul, consul_kv, etcd or kubernetes",
            ],
        ),
        (
            format!(
                r#"{{{WEB}, "providers": [{{"consul": {{}}}}, {{"consul": {{"endpoint": "http://127.0.0.1:1"}}}}]}}"#
            ),
            vec![
                r#"1:82: providers[1].name: another provider is already named "consul": give one a name"#,
            ],
        ),
        (
            format!(r#"{{{WEB}, "providers": [{{"name": "file", "etcd": {{}}}}]}}"#),
            vec![
                r#"1:66: providers[0].name: "file" names a source of the base config: pick another name"#,
            ],
        ),
        (
            format!(r#"{{{WEB}, "providers": [{{"name": "Bad", "etcd": {{}}}}]}}"#),
            vec![
                r#"1:66: providers[0].name: "Bad" is not a valid name: use lower-case letters, digits, '_', '.' and '-', starting with a letter or digit, at most 63"#,
            ],
        ),
        (
            format!(r#"{{{WEB}, "providers": [{{"consul": {{}}, "etcd": {{}}}}]}}"#),
            vec![
                "1:66: providers[0]: set exactly one kind of provider: docker, consul, consul_kv, etcd or kubernetes",
            ],
        ),
        (
            format!(
                r#"{{{WEB}, "providers": [{{"consul": {{"endpoint": "tcp://x:1", "tag_prefix": "-x", "cert_file": "c.pem"}}}}]}}"#
            ),
            vec![
                r#"1:90: providers[0].consul.endpoint: "tcp://x:1" is neither http://<host>:<port> nor https://<host>:<port>"#,
                "1:136: providers[0].consul.cert_file: cert_file needs key_file",
                r#"1:117: providers[0].consul.tag_prefix: "-x" is not a valid prefix: use letters, digits, '_', '.' and '-', starting with a letter or digit"#,
            ],
        ),
        (
            format!(
                r#"{{{WEB}, "providers": [{{"consul_kv": {{"endpoint": "http://x:1/v1", "key_file": "k.pem"}}}}]}}"#
            ),
            vec![
                r#"1:93: providers[0].consul_kv.endpoint: "http://x:1/v1" is not a host and port alone"#,
                "1:122: providers[0].consul_kv.key_file: key_file needs cert_file",
            ],
        ),
        (
            format!(
                r#"{{{WEB}, "providers": [{{"etcd": {{"endpoints": ["http://ok:2379", "https://nohost"], "username": "u"}}}}]}}"#
            ),
            vec![
                r#"1:108: providers[0].etcd.endpoints[1]: "https://nohost" is not a host and port alone"#,
                "1:139: providers[0].etcd.username: username needs password_file",
            ],
        ),
        // more_test.go's TestProviderChecks.
        (
            format!(r#"{{{WEB}, "providers": [{{"etcd": {{"password_file": "p"}}}}]}}"#),
            vec!["providers[0].etcd.password_file: password_file needs username"],
        ),
    ] {
        let mut got = errors(&doc);
        let mut want: Vec<String> = want.into_iter().map(str::to_owned).collect();
        if !want[0].starts_with("1:") {
            // Without a position: the message alone.
            got = got
                .iter()
                .map(|e| e.split_once(": ").map_or(e.clone(), |(_, m)| m.to_owned()))
                .collect();
        }
        got.sort();
        want.sort();
        assert_eq!(got, want, "{doc}");
    }
}

/// A field the kinds do not have is unknown.
#[test]
fn unknown_fields() {
    let errs = errors(&format!(
        r#"{{{WEB}, "providers": [{{"etcd": {{"colour": 1}}}}]}}"#
    ));
    assert!(
        errs[0].contains("providers[0].etcd.colour: unknown field"),
        "{errs:?}"
    );
}

/// `providers` may be split across files, as fit's lists at the top; names are unique across
/// them.
#[test]
fn across_files() {
    let a = format!(r#"{{{WEB}, "providers": [{{"etcd": {{}}}}]}}"#);
    let b = r#"{"providers": [{"name": "kv", "consul_kv": {}}]}"#;
    let c = from_sources(&[
        Source {
            name: Some("a.json"),
            format: Format::Json,
            src: &a,
        },
        Source {
            name: Some("b.json"),
            format: Format::Json,
            src: b,
        },
    ])
    .unwrap();
    assert_eq!(
        c.providers
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["etcd", "kv"]
    );
    let b = r#"{"providers": [{"etcd": {}}]}"#;
    let errs = from_sources(&[
        Source {
            name: Some("a.json"),
            format: Format::Json,
            src: &a,
        },
        Source {
            name: Some("b.json"),
            format: Format::Json,
            src: b,
        },
    ])
    .unwrap_err();
    let errs: Vec<String> = errs.as_slice().iter().map(ToString::to_string).collect();
    assert_eq!(
        errs,
        [
            r#"b.json:1:16: providers[0].name: another provider is already named "etcd": give one a name"#
        ]
    );
}
