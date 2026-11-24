//! `providers` (I58): read as fit reads them, with their defaults and fit's checks
//! (`config_test.go`'s provider cases); the kinds not supported yet refused as such.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;

use akit_config::{
    DEFAULT_DOCKER_LABEL_PREFIX, DEFAULT_DOCKER_MAX_CONTAINERS, DEFAULT_DOCKER_REFRESH_INTERVAL,
    DEFAULT_DOCKER_REQUEST_TIMEOUT, DockerEndpoint, DockerProvider, Provider, ProviderKind,
    from_json,
};

const L: &str = r#""listeners": [{"name": "web", "address": ":80"}]"#;

fn providers(ps: &str) -> Result<Vec<Provider>, Vec<String>> {
    from_json(&format!(r#"{{{L}, "providers": {ps}}}"#), None)
        .map(|c| c.providers)
        .map_err(|e| e.as_slice().iter().map(ToString::to_string).collect())
}

fn docker(p: &Provider) -> &DockerProvider {
    match &p.kind {
        ProviderKind::Docker(d) => d,
        other => panic!("not docker: {}", other.name()),
    }
}

#[test]
fn defaults_and_values() {
    assert!(from_json("{}", None).unwrap().providers.is_empty());
    let ps = providers(r#"[{"docker": {}}]"#).unwrap();
    assert_eq!(ps[0].name, "docker");
    let d = docker(&ps[0]);
    assert_eq!(
        d.endpoint,
        DockerEndpoint::Unix(PathBuf::from("/var/run/docker.sock"))
    );
    assert_eq!(d.label_prefix, DEFAULT_DOCKER_LABEL_PREFIX);
    assert_eq!(d.max_containers, DEFAULT_DOCKER_MAX_CONTAINERS);
    assert_eq!(d.refresh_interval, DEFAULT_DOCKER_REFRESH_INTERVAL);
    assert_eq!(d.request_timeout, DEFAULT_DOCKER_REQUEST_TIMEOUT);
    assert!(!d.expose_all && !d.use_published_ports && d.network.is_empty());

    let ps = providers(
        r#"[{"name": "local", "docker": {"endpoint": "unix:///run/d.sock", "expose_all": true,
            "network": "web", "use_published_ports": true, "label_prefix": "app",
            "max_containers": 0, "refresh_interval": "0s", "request_timeout": "3s"}},
           {"name": "remote", "docker": {"endpoint": "tcp://[::1]:2375"}}]"#,
    )
    .unwrap();
    let d = docker(&ps[0]);
    assert_eq!(ps[0].name, "local");
    assert_eq!(
        d.endpoint,
        DockerEndpoint::Unix(PathBuf::from("/run/d.sock"))
    );
    assert!(d.expose_all && d.use_published_ports);
    assert_eq!(
        (d.network.as_str(), d.label_prefix.as_str()),
        ("web", "app")
    );
    // 0 is the default, as in fit.
    assert_eq!(d.max_containers, DEFAULT_DOCKER_MAX_CONTAINERS);
    assert_eq!(d.refresh_interval, DEFAULT_DOCKER_REFRESH_INTERVAL);
    assert_eq!(d.request_timeout, std::time::Duration::from_secs(3));
    assert_eq!(
        docker(&ps[1]).endpoint,
        DockerEndpoint::Tcp("[::1]:2375".into())
    );
}

/// fit's `config_test.go` provider cases.
#[test]
fn checks() {
    for (ps, want) in [
        (
            "[{}]",
            "providers[0]: set exactly one kind of provider: docker, consul, consul_kv, etcd or kubernetes",
        ),
        (
            r#"[{"docker": {}}, {"docker": {"endpoint": "tcp://127.0.0.1:2375"}}]"#,
            r#"providers[1].name: another provider is already named "docker": give one a name"#,
        ),
        (
            r#"[{"name": "file", "docker": {}}]"#,
            r#"providers[0].name: "file" names a source of the base config: pick another name"#,
        ),
        (
            r#"[{"name": "Bad", "docker": {}}]"#,
            r#"providers[0].name: "Bad" is not a valid name: use lower-case letters, digits, '_', '.' and '-', starting with a letter or digit, at most 63"#,
        ),
        (
            r#"[{"docker": {"endpoint": "http://x"}}]"#,
            r#"providers[0].docker.endpoint: "http://x" is neither unix://<path> nor tcp://<host>:<port>"#,
        ),
        (
            r#"[{"docker": {"endpoint": "tcp://nohost"}}]"#,
            r#"providers[0].docker.endpoint: "tcp://nohost" has no host and port"#,
        ),
        (
            r#"[{"docker": {"endpoint": "unix://"}}]"#,
            r#"providers[0].docker.endpoint: "unix://" has no path"#,
        ),
        (
            r#"[{"docker": {"label_prefix": "-x"}}]"#,
            r#"providers[0].docker.label_prefix: "-x" is not a valid prefix: use letters, digits, '_', '.' and '-', starting with a letter or digit"#,
        ),
        (
            r#"[{"docker": {}, "kubernetes": {}}]"#,
            "providers[0]: set exactly one kind of provider: docker, consul, consul_kv, etcd or kubernetes",
        ),
        (
            r#"[{"docker": {"max_containers": -1}}]"#,
            "providers[0].docker.max_containers: -1 is less than the minimum 0",
        ),
        (
            r#"[{"docker": {"colour": "red"}}]"#,
            "providers[0].docker.colour: unknown field",
        ),
    ] {
        let errs = providers(ps).unwrap_err();
        assert!(
            errs.iter().any(|e| e.ends_with(want)),
            "{ps}: {errs:?}, want {want:?}"
        );
    }
}

/// The kinds of I61 are refused as not supported yet, naming the field; a kubernetes provider
/// without `ingress` too (the Gateway API, I60).
#[test]
fn other_kinds_not_supported_yet() {
    for kind in ["consul", "consul_kv", "etcd", "kubernetes"] {
        let errs = providers(&format!(r#"[{{"{kind}": {{}}}}]"#)).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains(&format!("providers[0].{kind}"))
                    && e.contains("not supported yet")),
            "{kind}: {errs:?}"
        );
        assert!(
            !errs.iter().any(|e| e.contains("exactly one kind")),
            "{kind}: {errs:?}"
        );
    }
}

/// Providers may be split across files, and their names are unique across them.
#[test]
fn across_files() {
    use akit_config::{Format, Source, from_sources};
    let a = r#"{"providers": [{"name": "a", "docker": {}}]}"#;
    let b = r#"{"providers": [{"name": "b", "docker": {}}, {"name": "a", "docker": {}}]}"#;
    let srcs = [
        Source {
            name: Some("a.json"),
            src: a,
            format: Format::Json,
        },
        Source {
            name: Some("b.json"),
            src: b,
            format: Format::Json,
        },
    ];
    let errs: Vec<String> = from_sources(&srcs)
        .unwrap_err()
        .as_slice()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(
        errs[0].starts_with("b.json:")
            && errs[0].ends_with(
                r#"providers[1].name: another provider is already named "a": give one a name"#
            ),
        "{errs:?}"
    );
}
