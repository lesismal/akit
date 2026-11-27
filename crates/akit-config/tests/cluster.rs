//! Cluster management in the config (I130): `admin.node_id`, `admin.cluster` and
//! `admin.cluster_member` read and checked as fit's `config/cluster_test.go` checks them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use akit_config::cluster::{Cluster, MAX_CLUSTER_ADMINS};
use akit_config::{Config, from_json};

const STORE: &str = r#""store": {"path": "/x", "password": "p"}"#;
// fit's assistantConsole, with a hash that is one.
const CONSOLE: &str = r#""console": {"users": [{"username": "ada", "password_hash": "$2a$10$x"}]}"#;

fn load(doc: &str) -> Result<Config, String> {
    from_json(doc, None).map_err(|e| {
        e.as_slice()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    })
}

#[test]
fn cluster_values() {
    let c = load(&format!(
        r#"{{"admin": {{{STORE}, {CONSOLE}, "cluster": {{"poll_interval": "5s", "max_nodes": 10, "admins": ["ada"]}}}}}}"#
    ))
    .unwrap();
    let cl = c.admin.unwrap().cluster.unwrap();
    assert_eq!(
        cl,
        Cluster {
            poll_interval: Duration::from_secs(5),
            max_nodes: 10,
            admins: vec!["ada".to_owned()],
            ..Cluster::default()
        }
    );
    // The store may be one akit run finds beside the config, which akit run checks: the config
    // alone does not need admin.store.
    let c = load(&format!(r#"{{"admin": {{{CONSOLE}, "cluster": {{}}}}}}"#)).unwrap();
    assert_eq!(c.admin.unwrap().cluster, Some(Cluster::default()));
}

#[test]
fn cluster_errors() {
    // fit's TestClusterErrors, its tokens with akit's prefix.
    let fp = format!("{}AB", "AB:".repeat(31));
    let admins = vec![r#""a""#; MAX_CLUSTER_ADMINS + 1].join(",");
    let cases: Vec<(&str, String, Vec<&str>)> = vec![
        (
            "no console",
            format!(r#"{{"admin": {{{STORE}, "cluster": {{}}}}}}"#),
            vec!["admin.cluster: a manager's members are served in the console's workbench"],
        ),
        (
            "bounds",
            format!(
                r#"{{"admin": {{{STORE}, {CONSOLE}, "cluster": {{"poll_interval": "10ms", "timeout": "1ms", "max_response_bytes": 10}}}}}}"#
            ),
            vec![
                "poll_interval: 10ms is below 1s",
                "timeout: 1ms is below 1s",
                "max_response_bytes: 10 is below 16384",
            ],
        ),
        (
            "maximum",
            format!(
                r#"{{"admin": {{{STORE}, {CONSOLE}, "cluster": {{"max_concurrency": 1000}}}}}}"#
            ),
            vec!["max_concurrency: 1000 is greater than the maximum 256"],
        ),
        (
            "link",
            format!(
                r#"{{"admin": {{{STORE}, {CONSOLE}, "cluster": {{"link_address": "7444", "link_url": "https://x"}}}}}}"#
            ),
            vec![
                "link_address: \"7444\" is not a host and a port",
                "link_url: \"https://x\" is not a wss:// URL",
            ],
        ),
        (
            "link url alone",
            format!(
                r#"{{"admin": {{{STORE}, {CONSOLE}, "cluster": {{"link_url": "wss://m:7444/cluster/link"}}}}}}"#
            ),
            vec!["link_url: link_url names where members reach link_address: set link_address too"],
        ),
        (
            "member",
            r#"{"admin": {"cluster_member": {"manager": "ws://m/x", "fingerprint": "ab", "token": "akite1.x"}}}"#.to_owned(),
            vec![
                "cluster_member.manager: \"ws://m/x\" is not a wss:// URL",
                "cluster_member.fingerprint: \"ab\" is not the SHA-256",
                "cluster_member.token: not an enrollment token",
            ],
        ),
        (
            "member token twice",
            format!(
                r#"{{"admin": {{"cluster_member": {{"manager": "wss://m:1/cluster/link", "fingerprint": "{fp}", "token": "x", "token_file": "/t"}}}}}}"#
            ),
            vec!["cluster_member.token: token and token_file are two places for one token"],
        ),
        (
            "member required",
            r#"{"admin": {"cluster_member": {}}}"#.to_owned(),
            vec!["cluster_member.manager", "cluster_member.fingerprint"],
        ),
        (
            "admins",
            format!(
                r#"{{"admin": {{{STORE}, {CONSOLE}, "cluster": {{"admins": ["", "nobody", "nobody"]}}}}}}"#
            ),
            vec![
                "admin.cluster.admins[0]: an admin is a console user's name",
                "admins[1]: \"nobody\" is not a console user",
                "admins[2]: \"nobody\" is named twice",
            ],
        ),
        (
            "too many admins",
            format!(
                r#"{{"admin": {{{STORE}, {CONSOLE}, "cluster": {{"admins": [{admins}]}}}}}}"#
            ),
            vec!["admins: 65 admins, more than 64"],
        ),
        (
            "node id",
            r#"{"admin": {"node_id": "-x"}}"#.to_owned(),
            vec!["admin.node_id: \"-x\" is not a node ID"],
        ),
        (
            "unknown",
            format!(r#"{{"admin": {{{CONSOLE}, "cluster": {{"bogus": 1}}}}}}"#),
            vec!["admin.cluster.bogus: unknown field"],
        ),
    ];
    for (name, doc, want) in cases {
        let err = match load(&doc) {
            Ok(_) => panic!("{name}: loaded"),
            Err(e) => e,
        };
        for w in want {
            assert!(err.contains(w), "{name}: {err:?} lacks {w:?}");
        }
    }
}

#[test]
fn cluster_link_settings() {
    // fit's TestClusterLinkSettings.
    let doc = format!(
        r#"{{"admin": {{{STORE}, {CONSOLE}, "cluster": {{"link_address": ":7444", "link_url": "wss://m.example:7444/cluster/link"}},
"cluster_member": {{"manager": "wss://m.example:7444/cluster/link", "fingerprint": "{}", "token": "akite1.0123456789abcdef.{}"}}}}}}"#,
        "0a".repeat(32),
        "A".repeat(43)
    );
    let a = load(&doc).unwrap().admin.unwrap();
    assert_eq!(a.cluster.unwrap().link_address, ":7444");
    let m = a.cluster_member.unwrap();
    assert_eq!(m.manager, "wss://m.example:7444/cluster/link");
    // The token is never shown.
    assert!(!format!("{m:?}").contains("AAAA"));
    let a = load(r#"{"admin": {"node_id": "edge-1.example"}}"#)
        .unwrap()
        .admin
        .unwrap();
    assert_eq!(a.node_id.as_deref(), Some("edge-1.example"));
}
