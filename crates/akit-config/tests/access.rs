//! `access` (I85; fit's `config.Access` and its `check`): the global allow and deny lists, a
//! listener's and a route's, their entries checked as fit's router parses them and their files'
//! paths not empty; the files are read when the config is compiled (akit-access).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use akit_config::{Access, from_conf, from_json, from_yaml};

fn problems(src: &str) -> Vec<String> {
    from_json(src, Some("t.json"))
        .expect_err("expected errors")
        .as_slice()
        .iter()
        .map(|e| format!("{}: {}", e.path, e.msg))
        .collect()
}

const ROUTES: &str = r#""listeners": [{"name": "web", "address": ":80"}], "routes": [{"handler": {"module": "respond"}}]"#;

#[test]
fn read_at_every_level() {
    let c = from_json(
        r#"{"access": {"allow": ["10.0.0.0/8"], "deny": ["10.1.0.0/16", " 192.0.2.1 "],
                       "allow_files": ["a.txt"], "deny_files": ["d.txt", "e.p2p"]},
            "listeners": [{"name": "web", "address": ":8080", "access": {"deny": ["2001:db8::/32"]}}],
            "routes": [{"access": {"allow": ["::1"]}, "handler": {"module": "respond"}}]}"#,
        None,
    )
    .unwrap();
    assert_eq!(
        c.access,
        Some(Access {
            allow: vec!["10.0.0.0/8".into()],
            deny: vec!["10.1.0.0/16".into(), " 192.0.2.1 ".into()],
            allow_files: vec!["a.txt".into()],
            deny_files: vec!["d.txt".into(), "e.p2p".into()],
        })
    );
    assert_eq!(
        c.access.as_ref().unwrap().files().collect::<Vec<_>>(),
        ["a.txt", "d.txt", "e.p2p"]
    );
    assert_eq!(
        c.listeners[0].access.as_ref().unwrap().deny,
        ["2001:db8::/32"]
    );
    assert_eq!(c.routes[0].access.as_ref().unwrap().allow, ["::1"]);
    // A service's (I86).
    let c = from_json(
        r#"{"listeners": [{"name": "t", "address": ":7000", "protocols": ["tcp"]}],
            "upstreams": [{"name": "app", "addresses": ["127.0.0.1:9000"]}],
            "services": [{"name": "a", "listeners": ["t"], "upstream": "app", "access": {"allow": ["10.0.0.0/8"], "deny_files": ["d.txt"]}}]}"#,
        None,
    )
    .unwrap();
    let a = c.services[0].access.as_ref().unwrap();
    assert_eq!(
        (a.allow.as_slice(), a.deny_files.as_slice()),
        (&["10.0.0.0/8".to_owned()][..], &["d.txt".to_owned()][..])
    );
    // Empty, and left out.
    let c = from_json(&format!(r#"{{"access": {{}}, {ROUTES}}}"#), None).unwrap();
    assert_eq!(c.access, Some(Access::default()));
    let c = from_json(&format!("{{{ROUTES}}}"), None).unwrap();
    assert_eq!(c.access, None);
}

#[test]
fn entries_are_checked() {
    assert_eq!(
        problems(&format!(
            r#"{{"access": {{"allow": ["10.0.0.0/33", "10.0.0.1"], "deny": ["x", "::ffff:10.0.0.0/104"],
                 "allow_files": [" "], "deny_files": [""]}}, {ROUTES}}}"#
        )),
        [
            r#"access.allow[0]: "10.0.0.0/33" is not an IP address or a CIDR prefix"#,
            r#"access.deny[0]: "x" is not an IP address or a CIDR prefix"#,
            r#"access.deny[1]: "::ffff:10.0.0.0/104" is an IPv4 prefix written in IPv6: write it in IPv4"#,
            "access.allow_files[0]: the path is empty",
            "access.deny_files[0]: the path is empty",
        ]
    );
    assert_eq!(
        problems(
            r#"{"listeners": [{"name": "web", "address": ":80", "access": {"allow": "10.0.0.0/8"}}]}"#
        ),
        ["listeners[0].access.allow: expected an array, got a string"]
    );
    assert_eq!(
        problems(
            r#"{"listeners": [{"name": "web", "address": ":80"}], "routes": [{"access": {"allows": []}, "handler": {"module": "respond"}}]}"#
        ),
        ["routes[0].access.allows: unknown field"]
    );
}

#[test]
fn conf_and_yaml() {
    let conf = from_conf(
        "access { deny 203.0.113.0/24; deny_files lists/deny.txt }\n\
         listener web :8080 { access { allow 10.0.0.0/8 192.168.0.0/16 } }\n\
         route { access { allow 203.0.113.7 }; handler respond { body hi } }\n",
        Some("t.conf"),
    )
    .unwrap();
    let yaml = from_yaml(
        "access:\n  deny: [203.0.113.0/24]\n  deny_files: [lists/deny.txt]\n\
         listeners:\n  - name: web\n    address: :8080\n    access: {allow: [10.0.0.0/8, 192.168.0.0/16]}\n\
         routes:\n  - access: {allow: [203.0.113.7]}\n    handler: {module: respond, params: {body: hi}}\n",
        Some("t.yaml"),
    )
    .unwrap();
    for c in [&conf, &yaml] {
        let a = c.access.as_ref().unwrap();
        assert_eq!(
            (a.deny.as_slice(), a.deny_files.as_slice()),
            (
                &["203.0.113.0/24".to_owned()][..],
                &["lists/deny.txt".to_owned()][..]
            )
        );
        assert_eq!(
            c.listeners[0].access.as_ref().unwrap().allow,
            ["10.0.0.0/8", "192.168.0.0/16"]
        );
        assert_eq!(c.routes[0].access.as_ref().unwrap().allow, ["203.0.113.7"]);
    }
}

/// The global access is one file's, as the other objects at the top are.
#[test]
fn given_by_one_file() {
    let d = std::env::temp_dir().join(format!("akit-config-access-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let a = d.join("a.json");
    let b = d.join("b.json");
    std::fs::write(
        &a,
        format!(r#"{{"access": {{"deny": ["10.0.0.1"]}}, {ROUTES}}}"#),
    )
    .unwrap();
    std::fs::write(&b, r#"{"access": {"deny": ["10.0.0.2"]}}"#).unwrap();
    let e = akit_config::from_files(&[&a, &b]).unwrap_err().to_string();
    let _ = std::fs::remove_dir_all(&d);
    assert!(e.contains("access: also given in"), "{e}");
}
