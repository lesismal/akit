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
    // The dashboard (I63): served unless disabled.
    assert!(!Admin::default().disable_dashboard);
    let a = admin(r#""disable_dashboard": true"#).unwrap().unwrap();
    assert!(a.disable_dashboard);
    assert_eq!(
        admin(r#""disable_dashboard": false"#).unwrap().unwrap(),
        Admin::default()
    );
    // Off loopback, a token is not enough: TLS too (I92).
    let e = admin(r#""address": "0.0.0.0:2019", "token_file": "t""#).unwrap_err();
    assert!(e[0].contains("or set cert_file and key_file"), "{e:?}");
    let a = admin(
        r#""address": "0.0.0.0:2019", "token_file": "t", "cert_file": "c", "key_file": "k",
           "client_ca_file": "ca""#,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        (
            a.cert_file.as_deref(),
            a.key_file.as_deref(),
            a.client_ca_file.as_deref()
        ),
        (Some("c"), Some("k"), Some("ca"))
    );
    // TLS on loopback, without a token, is fine.
    assert!(admin(r#""cert_file": "c", "key_file": "k""#).is_ok());
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
            r#""address": "unix/""#,
            r#"admin.address: "unix/" has no path after "unix/""#,
        ),
        (r#""token_file": 1"#, "admin.token_file: expected a string"),
        (
            r#""disable_dashboard": "yes""#,
            "admin.disable_dashboard: expected a boolean",
        ),
        (
            r#""cert_file": "c""#,
            "admin: cert_file and key_file go together",
        ),
        (
            r#""client_ca_file": "ca""#,
            "admin.client_ca_file: client certificates are asked for over TLS",
        ),
        (
            r#""address": "unix//run/a.sock", "cert_file": "c", "key_file": "k""#,
            "a Unix socket is not served over TLS",
        ),
        (
            r#""address": ":2019""#,
            "or set token_file, cert_file and key_file",
        ),
        (r#""mptcp": true"#, "admin.mptcp: not supported yet"),
        (r#""store": {}"#, "admin.store: not supported yet"),
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

const HASH: &str = "$2b$04$9u7ktsc3kKzS8dTbCbQyvOCT6NsjXTrZfX9t/cP8cs6oBwVjtwHZC";

fn console(fields: &str) -> Result<akit_config::Console, Vec<String>> {
    admin(&format!(r#""console": {{{fields}}}"#)).map(|a| a.unwrap().console.unwrap())
}

#[test]
fn console_defaults_and_values() {
    use akit_config::{
        Console, ConsoleUser, DEFAULT_CONSOLE_MAX_CONNS, DEFAULT_CONSOLE_MAX_SESSIONS,
        DEFAULT_CONSOLE_SESSION_LIFETIME, DEFAULT_CONSOLE_SESSION_TIMEOUT,
    };
    use std::time::Duration;
    let users = format!(r#""users": [{{"username": "admin", "password_hash": "{HASH}"}}]"#);
    let c = console(&users).unwrap();
    assert_eq!(
        c,
        Console {
            users: vec![ConsoleUser {
                username: "admin".into(),
                password_hash: HASH.into(),
            }],
            ..Console::default()
        }
    );
    // fit's defaults.
    assert_eq!(c.address, "127.0.0.1:2020");
    assert_eq!(
        (
            DEFAULT_CONSOLE_SESSION_TIMEOUT,
            DEFAULT_CONSOLE_SESSION_LIFETIME,
            DEFAULT_CONSOLE_MAX_SESSIONS,
            DEFAULT_CONSOLE_MAX_CONNS
        ),
        (
            Duration::from_secs(1800),
            Duration::from_secs(43200),
            64,
            32
        )
    );
    // The hash is never in a Debug line.
    assert!(!format!("{c:?}").contains(HASH));
    let c = console(&format!(
        r#"{users}, "address": "0.0.0.0:2020", "cert_file": "c.pem", "key_file": "k.pem",
           "session_timeout": "5m", "session_lifetime": "1h", "max_sessions": 2, "max_conns": 3,
           "access": {{"allow": ["127.0.0.1"]}}"#
    ))
    .unwrap();
    assert_eq!(
        (
            c.address.as_str(),
            c.cert_file.as_deref(),
            c.key_file.as_deref(),
            c.session_timeout,
            c.session_lifetime,
            c.max_sessions,
            c.max_conns,
        ),
        (
            "0.0.0.0:2020",
            Some("c.pem"),
            Some("k.pem"),
            Duration::from_secs(300),
            Duration::from_secs(3600),
            2,
            3
        )
    );
    assert_eq!(c.access.unwrap().allow, vec!["127.0.0.1".to_owned()]);
    // 0 is the default, as fit's.
    let c = console(&format!(
        r#"{users}, "session_timeout": "0s", "max_sessions": 0, "max_conns": 0"#
    ))
    .unwrap();
    assert_eq!(
        (c.session_timeout, c.max_sessions, c.max_conns),
        (DEFAULT_CONSOLE_SESSION_TIMEOUT, 64, 32)
    );
    // A config of the console alone needs no listener, as fit's.
    assert!(from_json(&format!(r#"{{"admin": {{"console": {{{users}}}}}}}"#), None).is_ok());
}

#[test]
fn console_refused() {
    let user = format!(r#"{{"username": "admin", "password_hash": "{HASH}"}}"#);
    for (fields, want) in [
        (String::new(), "admin.console.users: missing required field"),
        (
            r#""users": []"#.to_owned(),
            "admin.console.users: list at least one user",
        ),
        (
            format!(r#""users": [{user}, {user}]"#),
            r#"admin.console.users[1].username: "admin" is listed twice"#,
        ),
        (
            r#""users": [{"username": "", "password_hash": "$2a$10$x"}]"#.to_owned(),
            "admin.console.users[0].username: the user name is empty",
        ),
        (
            r#""users": [{"username": "a", "password_hash": "secret"}]"#.to_owned(),
            "admin.console.users[0].password_hash: not a bcrypt hash: make one with akit hash-password",
        ),
        (
            r#""users": [{"username": "a"}]"#.to_owned(),
            "admin.console.users[0].password_hash: missing required field",
        ),
        (
            format!(r#""users": [{user}], "address": "0.0.0.0:2020""#),
            r#"admin.console.address: "0.0.0.0:2020" is not a loopback address: set cert_file and key_file"#,
        ),
        (
            format!(r#""users": [{user}], "cert_file": "c""#),
            "admin.console: cert_file and key_file go together",
        ),
        (
            format!(r#""users": [{user}], "address": "2020""#),
            "admin.console.address: ",
        ),
        (
            format!(r#""users": [{user}], "mptcp": true"#),
            "admin.console.mptcp: not supported yet",
        ),
        (
            format!(r#""users": [{user}], "session_timeout": "soon""#),
            "admin.console.session_timeout: \"soon\" is not a duration",
        ),
        (
            format!(r#""users": [{user}], "max_conns": -1"#),
            "admin.console.max_conns: -1 is less than the minimum 0",
        ),
        (
            format!(r#""users": [{user}], "access": {{"allow": ["x"]}}"#),
            "admin.console.access.allow[0]",
        ),
        (
            format!(r#""users": [{user}], "bogus": 1"#),
            "admin.console.bogus: unknown field",
        ),
    ] {
        let e = console(&fields).unwrap_err();
        assert!(e.iter().any(|m| m.contains(want)), "{fields}: {e:?}");
    }
    // Off loopback with TLS is fine; on loopback without it too.
    assert!(console(&format!(r#""users": [{user}], "address": "[::1]:2020""#)).is_ok());
    assert!(
        console(&format!(
            r#""users": [{user}], "address": ":2020", "cert_file": "c", "key_file": "k""#
        ))
        .is_ok()
    );
}
