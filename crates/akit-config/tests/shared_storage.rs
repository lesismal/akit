//! `acme.shared_storage` (I62), as fit's `config.SharedStorage`: its defaults, each kind's
//! fields, and fit's checks (`config/config_test.go` and `config/more_test.go`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use akit_config::{
    ClientTlsFiles, ConsulStorage, DEFAULT_REDIS_ADDRESS, DEFAULT_SHARED_STORAGE_LOCK_TTL,
    DEFAULT_SHARED_STORAGE_PREFIX, DEFAULT_SHARED_STORAGE_TIMEOUT, EtcdStorage, RedisStorage,
    SharedStorage, SharedStorageKind, from_json,
};

const WEB: &str = r#""listeners": [{"name": "web", "address": ":80"}]"#;

fn shared(fields: &str) -> SharedStorage {
    from_json(
        &format!(r#"{{{WEB}, "acme": {{"shared_storage": {{{fields}}}}}}}"#),
        Some("t.json"),
    )
    .unwrap_or_else(|e| panic!("{fields}: {e}"))
    .acme
    .unwrap()
    .shared_storage
    .unwrap()
}

/// The errors of `doc`, each as `<line>:<col>: <path>: <message>`, sorted.
fn errors(doc: &str) -> Vec<String> {
    let mut v: Vec<String> = from_json(doc, None)
        .expect_err("expected errors")
        .as_slice()
        .iter()
        .map(ToString::to_string)
        .collect();
    v.sort();
    v
}

#[test]
fn defaults() {
    let s = shared(r#""redis": {}"#);
    assert_eq!(
        s,
        SharedStorage {
            kind: SharedStorageKind::Redis(RedisStorage {
                address: DEFAULT_REDIS_ADDRESS.into(),
                username: String::new(),
                password_file: String::new(),
                db: 0,
                tls: false,
                tls_files: ClientTlsFiles::default(),
            }),
            prefix: DEFAULT_SHARED_STORAGE_PREFIX.into(),
            lock_ttl: DEFAULT_SHARED_STORAGE_LOCK_TTL,
            request_timeout: DEFAULT_SHARED_STORAGE_TIMEOUT,
        }
    );
    assert_eq!(
        (
            DEFAULT_SHARED_STORAGE_PREFIX,
            DEFAULT_SHARED_STORAGE_LOCK_TTL,
            DEFAULT_SHARED_STORAGE_TIMEOUT
        ),
        ("akit/", Duration::from_secs(30), Duration::from_secs(10))
    );
    assert_eq!(s.kind.name(), "redis");
    let s = shared(r#""etcd": {}"#);
    assert_eq!(
        s.kind,
        SharedStorageKind::Etcd(EtcdStorage {
            endpoints: vec!["http://127.0.0.1:2379".into()],
            username: String::new(),
            password_file: String::new(),
            tls: ClientTlsFiles::default(),
        })
    );
    let s = shared(r#""consul": {}, "prefix": "", "lock_ttl": "0s""#);
    assert_eq!(
        s.kind,
        SharedStorageKind::Consul(ConsulStorage {
            endpoint: "http://127.0.0.1:8500".into(),
            datacenter: String::new(),
            token_file: String::new(),
            tls: ClientTlsFiles::default(),
        })
    );
    assert_eq!(
        (s.prefix.as_str(), s.lock_ttl),
        ("akit/", DEFAULT_SHARED_STORAGE_LOCK_TTL)
    );
}

#[test]
fn every_field() {
    let s = shared(
        r#""redis": {"address": "[::1]:6380", "username": "u", "password_file": "/p",
            "db": 3, "tls": true, "ca_file": "/ca", "cert_file": "/c", "key_file": "/k"},
           "prefix": "akit/", "lock_ttl": "1m", "request_timeout": "3s""#,
    );
    assert_eq!(
        s,
        SharedStorage {
            kind: SharedStorageKind::Redis(RedisStorage {
                address: "[::1]:6380".into(),
                username: "u".into(),
                password_file: "/p".into(),
                db: 3,
                tls: true,
                tls_files: ClientTlsFiles {
                    ca_file: "/ca".into(),
                    cert_file: "/c".into(),
                    key_file: "/k".into(),
                },
            }),
            prefix: "akit/".into(),
            lock_ttl: Duration::from_secs(60),
            request_timeout: Duration::from_secs(3),
        }
    );
    let s = shared(
        r#""etcd": {"endpoints": ["https://a:2379", "http://b:2379"], "username": "u",
            "password_file": "/p", "ca_file": "/ca", "cert_file": "/c", "key_file": "/k"}"#,
    );
    let SharedStorageKind::Etcd(e) = s.kind else {
        panic!("{s:?}")
    };
    assert_eq!(e.endpoints, ["https://a:2379", "http://b:2379"]);
    assert_eq!((e.username.as_str(), e.password_file.as_str()), ("u", "/p"));
    assert_eq!(e.tls.cert_file, "/c");
    let s = shared(
        r#""consul": {"endpoint": "https://c:8501", "datacenter": "dc2", "token_file": "/t",
            "ca_file": "/ca"}"#,
    );
    let SharedStorageKind::Consul(c) = s.kind else {
        panic!("{s:?}")
    };
    assert_eq!(
        (
            c.endpoint.as_str(),
            c.datacenter.as_str(),
            c.token_file.as_str(),
            c.tls.ca_file.as_str()
        ),
        ("https://c:8501", "dc2", "/t", "/ca")
    );
}

/// fit's `config_test.go` cases, at fit's positions.
#[test]
fn checks_as_fit() {
    assert_eq!(
        errors(
            r#"{"listeners": [{"name": "web", "address": ":80"}], "acme": {"storage": "/d", "shared_storage": {"redis": {}, "etcd": {}}}}"#
        ),
        [
            "1:72: acme.storage: storage and shared_storage are two places to keep certificates: set one",
            "1:96: acme.shared_storage: set exactly one kind of shared storage: redis, etcd or consul",
        ]
    );
    assert_eq!(
        errors(
            r#"{"listeners": [{"name": "web", "address": ":80"}], "acme": {"shared_storage": {"redis": {"address": "nohost", "username": "u", "ca_file": "ca.pem"}, "lock_ttl": "5s"}}}"#
        ),
        [
            r#"1:101: acme.shared_storage.redis.address: "nohost" is not a host and port"#,
            "1:123: acme.shared_storage.redis.username: username needs password_file",
            "1:139: acme.shared_storage.redis.ca_file: ca_file needs tls",
            "1:162: acme.shared_storage.lock_ttl: 5s is too short: at least 10s, which Consul's sessions need, and more than a renewal takes",
        ]
    );
    assert_eq!(
        errors(
            r#"{"listeners": [{"name": "web", "address": ":80"}], "acme": {"shared_storage": {"etcd": {"endpoints": ["tcp://x:1"], "password_file": "p"}}}}"#
        ),
        [
            r#"1:103: acme.shared_storage.etcd.endpoints[0]: "tcp://x:1" is neither http://<host>:<port> nor https://<host>:<port>"#,
            "1:134: acme.shared_storage.etcd.password_file: password_file needs username",
        ]
    );
    assert_eq!(
        errors(
            r#"{"listeners": [{"name": "web", "address": ":80"}], "acme": {"shared_storage": {"consul": {"endpoint": "http://x", "cert_file": "c"}}}}"#
        ),
        [
            r#"1:103: acme.shared_storage.consul.endpoint: "http://x" is not a host and port alone"#,
            "1:128: acme.shared_storage.consul.cert_file: cert_file needs key_file",
        ]
    );
}

/// fit's `more_test.go` cases, by path and message.
#[test]
fn more_checks_as_fit() {
    for (doc, want) in [
        (
            r#""redis": {"cert_file": "c", "key_file": "k"}"#,
            "acme.shared_storage.redis.cert_file: cert_file needs tls",
        ),
        (
            r#""etcd": {"password_file": "p"}"#,
            "acme.shared_storage.etcd.password_file: password_file needs username",
        ),
        (
            r#""etcd": {"username": "u"}"#,
            "acme.shared_storage.etcd.username: username needs password_file",
        ),
        (
            "",
            "acme.shared_storage: set exactly one kind of shared storage: redis, etcd or consul",
        ),
        (r#""redis": {"db": -1}"#, "acme.shared_storage.redis.db: "),
        (
            r#""redis": {}, "bogus": 1"#,
            "acme.shared_storage.bogus: unknown field",
        ),
    ] {
        let e = errors(&format!(
            r#"{{{WEB}, "acme": {{"shared_storage": {{{doc}}}}}}}"#
        ));
        assert_eq!(e.len(), 1, "{doc}: {e:?}");
        // Past the position, `<line>:<col>: `.
        let got = e[0].split_once(": ").map(|(_, rest)| rest);
        assert!(got.is_some_and(|g| g.starts_with(want)), "{doc}: {e:?}");
    }
}
