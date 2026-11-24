//! `admin` (I26): the Admin API's address, history and limits read as fit reads them, with fit's
//! check of the address; the rest of admin refused as not supported yet; given by one file only.
//! And the config as the API keeps it (`load_files`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::PathBuf;

use akit_config::{
    Admin, DEFAULT_ADMIN_ADDRESS, DEFAULT_ADMIN_HISTORY, DEFAULT_ADMIN_MAX_CONFIG_BYTES,
    DEFAULT_ADMIN_MAX_CONNS, from_json, load_files,
};

fn admin(fields: &str) -> Result<Option<Admin>, Vec<String>> {
    from_json(&format!(r#"{{"admin": {{{fields}}}}}"#), Some("t.json"))
        .map(|c| c.admin)
        .map_err(|e| e.as_slice().iter().map(ToString::to_string).collect())
}

#[test]
fn defaults_and_values() {
    assert_eq!(from_json("{}", None).unwrap().admin, None);
    let a = admin("").unwrap().unwrap();
    assert_eq!(a, Admin::default());
    assert_eq!(
        (
            a.address.as_str(),
            a.history,
            a.max_config_bytes,
            a.max_conns
        ),
        (
            DEFAULT_ADMIN_ADDRESS,
            DEFAULT_ADMIN_HISTORY,
            DEFAULT_ADMIN_MAX_CONFIG_BYTES,
            DEFAULT_ADMIN_MAX_CONNS
        )
    );
    assert_eq!(
        (DEFAULT_ADMIN_ADDRESS, DEFAULT_ADMIN_HISTORY),
        ("127.0.0.1:2019", 10)
    );
    assert_eq!(
        (DEFAULT_ADMIN_MAX_CONFIG_BYTES, DEFAULT_ADMIN_MAX_CONNS),
        (4 << 20, 16)
    );
    // 0 is the default, as fit's.
    let a = admin(r#""address": "", "history": 0, "max_config_bytes": 0, "max_conns": 0"#)
        .unwrap()
        .unwrap();
    assert_eq!(a, Admin::default());
    let a =
        admin(r#""address": "[::1]:2020", "history": 3, "max_config_bytes": 1024, "max_conns": 2"#)
            .unwrap()
            .unwrap();
    assert_eq!(
        (
            a.address.as_str(),
            a.history,
            a.max_config_bytes,
            a.max_conns
        ),
        ("[::1]:2020", 3, 1024, 2)
    );
    // The token (I27): a file, read when akit starts; none when left out or empty.
    let a = admin(r#""token_file": "/etc/akit/token""#)
        .unwrap()
        .unwrap();
    assert_eq!(a.token_file.as_deref(), Some("/etc/akit/token"));
    let a = admin(r#""token_file": """#).unwrap().unwrap();
    assert_eq!(a, Admin::default());
    // Off loopback, a token is not enough: the API is not served over TLS yet.
    let e = admin(r#""address": "0.0.0.0:2019", "token_file": "t""#).unwrap_err();
    assert!(
        e[0].contains("does not serve the Admin API over TLS yet"),
        "{e:?}"
    );
    for addr in ["localhost:2019", "127.0.0.2:1", "LOCALHOST:0"] {
        assert!(admin(&format!(r#""address": "{addr}""#)).is_ok(), "{addr}");
    }
}

#[test]
fn refused() {
    let e = admin(r#""address": "0.0.0.0:2019""#).unwrap_err();
    assert_eq!(e.len(), 1);
    assert!(
        e[0].starts_with(r#"t.json:1:23: admin.address: "0.0.0.0:2019" is not a loopback address"#),
        "{e:?}"
    );
    for (fields, want) in [
        (r#""address": ":2019""#, "is not a loopback address"),
        (r#""address": "2019""#, "is not a host and port"),
        (
            r#""history": -1"#,
            "admin.history: -1 is less than the minimum 0",
        ),
        (
            r#""max_conns": "2""#,
            "admin.max_conns: expected an integer",
        ),
        (
            r#""address": "unix//run/akit.sock""#,
            "admin.address: not supported yet: a Unix socket (akio API request A23)",
        ),
        (r#""token_file": 1"#, "admin.token_file: expected a string"),
        (r#""cert_file": "c""#, "admin.cert_file: not supported yet"),
        (r#""console": {}"#, "admin.console: not supported yet"),
        (r#""bogus": 1"#, "admin.bogus: unknown field"),
    ] {
        let e = admin(fields).unwrap_err();
        assert!(e.iter().any(|m| m.contains(want)), "{fields}: {e:?}");
    }
    assert!(
        from_json(r#"{"admin": []}"#, None).unwrap_err().as_slice()[0]
            .msg
            .contains("expected an object")
    );
}

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("admin")
        .join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn one_file_only() {
    let d = dir("two");
    fs::write(d.join("a.json"), r#"{"admin": {}}"#).unwrap();
    fs::write(d.join("b.json"), r#"{"admin": {"history": 2}}"#).unwrap();
    let e = akit_config::from_files(&[d]).unwrap_err();
    assert!(
        e.as_slice()[0].to_string().contains("admin: also given in"),
        "{e:?}"
    );
}

/// What the Admin API keeps of a config read from files: a lone JSON file as written, comments
/// blanked out; several files as one document.
#[test]
fn as_the_api_keeps_it() {
    let d = dir("kept");
    let one = d.join("one.json");
    let text = "{\"listeners\": [{\"name\": \"a\", \"address\": \":1\"},], // a\n}";
    fs::write(&one, text).unwrap();
    let (c, json) = load_files(&[&one]).unwrap();
    assert_eq!(c.listeners.len(), 1);
    assert_eq!(
        json,
        "{\"listeners\": [{\"name\": \"a\", \"address\": \":1\"} ]      \n}"
    );

    let more = d.join("more");
    fs::create_dir_all(&more).unwrap();
    fs::write(
        more.join("a.json"),
        r#"{"listeners": [{"name": "a", "address": ":1"}]}"#,
    )
    .unwrap();
    fs::write(
        more.join("b.yaml"),
        "admin:\n  history: 2\nlisteners:\n  - name: b\n    address: \":2\"\n",
    )
    .unwrap();
    let (c, json) = load_files(&[&more]).unwrap();
    assert_eq!(c.admin.unwrap().history, 2);
    let back = from_json(&json, None).unwrap();
    assert_eq!(back.listeners.len(), 2);
    assert_eq!(back.admin.unwrap().history, 2);

    // A config with errors gives them, and no JSON.
    fs::write(&one, "{\"listeners\": 1}").unwrap();
    assert!(load_files(&[&one]).is_err());
}
