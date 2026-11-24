//! The shared storage's parts, as fit's `sharedstore/fake_test.go` and `store_test.go`
//! (`TestReadReply`): RESP2 replies, Redis over TLS with a user and a database, Redis' dial
//! errors and replies of every shape, a pool whose connections are all busy, etcd's endpoints,
//! answers and authentication, Consul's, and the store's own errors and locks on a fake backend.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::type_complexity
)]

/// A CA and the leaves it signs, as PEM files (tests/support/certs.rs, which needs a target
/// directory only integration tests have).
mod certs {
    use std::path::{Path, PathBuf};

    use akio_crypto::pkey::PrivateKey;
    use akio_x509::{Certificate, CertificateParams};

    fn new_key() -> PrivateKey {
        PrivateKey::P256(akio_crypto::p256::SecretKey::generate().unwrap())
    }

    pub(super) struct Ca {
        cert: Certificate,
        key: PrivateKey,
    }

    impl Ca {
        pub(super) fn new(name: &str) -> Ca {
            let key = new_key();
            let cert = CertificateParams::ca(name).self_signed(&key).unwrap();
            Ca { cert, key }
        }

        pub(super) fn write(&self, dir: &Path, file: &str) -> PathBuf {
            let p = dir.join(file);
            std::fs::write(&p, self.cert.to_pem()).unwrap();
            p
        }

        pub(super) fn leaf(&self, dir: &Path, stem: &str, names: &[&str]) -> (PathBuf, PathBuf) {
            let key = new_key();
            let cert = CertificateParams::new(names)
                .signed_by(&key.public_key(), &self.cert, &self.key)
                .unwrap();
            let (c, k) = (
                dir.join(format!("{stem}.pem")),
                dir.join(format!("{stem}.key")),
            );
            std::fs::write(&c, cert.to_pem()).unwrap();
            std::fs::write(&k, key.to_pkcs8_pem()).unwrap();
            (c, k)
        }
    }
}
#[path = "../../tests/support/stores.rs"]
mod stores;

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use akio::tls::{Certificate, PrivateKey, ServerConfig};
use akit_config::{
    ClientTlsFiles, ConsulStorage, EtcdStorage, RedisStorage, SharedStorage, SharedStorageKind,
};

use super::consul::ConsulBackend;
use super::etcd::{EtcdBackend, range_end};
use super::redis::{RedisBackend, RedisError, Reply, parse};
use super::{Backend, CallError, Lease, MAX_KEYS, SharedStore};
use crate::storage::{MAX_VALUE_BYTES, Storage, StorageError};
use stores::{FakeHttp, FakeRedis, HttpReply, Replies, TempDir, array, bulk, closed_addr};

fn later() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

fn redis_opts(addr: &str) -> RedisStorage {
    RedisStorage {
        address: addr.into(),
        username: String::new(),
        password_file: String::new(),
        db: 0,
        tls: false,
        tls_files: ClientTlsFiles::default(),
    }
}

fn etcd_opts(endpoints: &[String]) -> EtcdStorage {
    EtcdStorage {
        endpoints: endpoints.to_vec(),
        username: String::new(),
        password_file: String::new(),
        tls: ClientTlsFiles::default(),
    }
}

fn consul_opts(endpoint: &str) -> ConsulStorage {
    ConsulStorage {
        endpoint: endpoint.into(),
        datacenter: String::new(),
        token_file: String::new(),
        tls: ClientTlsFiles::default(),
    }
}

fn shared(kind: SharedStorageKind, ttl: Duration) -> SharedStorage {
    SharedStorage {
        kind,
        prefix: "p/".into(),
        lock_ttl: ttl,
        request_timeout: Duration::from_secs(5),
    }
}

#[test]
fn replies_as_fit_reads_them() {
    let ok = |s: &str| parse(s.as_bytes(), 0);
    let some = |r: Reply, n: usize| Ok(Some((r, n)));
    assert_eq!(ok("+OK\r\n"), some(Reply::Simple("OK".into()), 5));
    assert_eq!(
        ok("-ERR wrong\r\n"),
        Err(RedisError::Answered("ERR wrong".into()))
    );
    assert_eq!(ok(":42\r\n"), some(Reply::Int(42), 5));
    assert_eq!(ok("$3\r\nabc\r\n"), some(Reply::Bulk(b"abc".to_vec()), 9));
    assert_eq!(ok("$-1\r\n"), some(Reply::Nil, 5));
    assert_eq!(ok("*-1\r\n"), some(Reply::Nil, 5));
    let s = "*2\r\n$1\r\n0\r\n*1\r\n$1\r\nk\r\n";
    assert_eq!(
        ok(s),
        some(
            Reply::Array(vec![
                Reply::Bulk(b"0".to_vec()),
                Reply::Array(vec![Reply::Bulk(b"k".to_vec())])
            ]),
            s.len()
        )
    );
    // An error inside an array is kept as one.
    let s = "*2\r\n-ERR a\r\n:1\r\n";
    assert_eq!(
        ok(s),
        some(
            Reply::Array(vec![Reply::Error("ERR a".into()), Reply::Int(1)]),
            s.len()
        )
    );
    // Not all there yet.
    for part in ["", "$5\r\nab", "*1\r\n$5\r\nab", "+OK"] {
        assert_eq!(ok(part), Ok(None), "{part:?}");
    }
    for (s, want) in [
        ("*1\r\n*1\r\n*1\r\n:1\r\n", "array"),
        (&*format!("${}\r\n", MAX_VALUE_BYTES + 1), "bulk string"),
        (&*format!("*{}\r\n", MAX_KEYS + 1), "array"),
        ("?\r\n", "type"),
        ("+OK\n", "not RESP"),
        (":x\r\n", "integer"),
    ] {
        match ok(s) {
            Err(RedisError::Conn(e)) if e.contains(want) => {}
            r => panic!("{s:?}: {r:?}, want an error with {want:?}"),
        }
    }
}

/// A server that cuts each connection as soon as it is made.
fn cutting_server() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for c in l.incoming() {
            drop(c);
        }
    });
    addr
}

#[test]
fn redis_tls_configs_refused() {
    let d = TempDir::new("redis-tls-config");
    let ca = certs::Ca::new("test CA");
    let (cert, _) = ca.leaf(&d.0, "leaf", &["127.0.0.1"]);
    let not_pem = d.file("ca.pem", "not a certificate");
    let none = d.0.join("none.pem").display().to_string();
    let cert = cert.display().to_string();
    for (name, files) in [
        (
            "no CA file",
            ClientTlsFiles {
                ca_file: none.clone(),
                ..ClientTlsFiles::default()
            },
        ),
        (
            "CA not PEM",
            ClientTlsFiles {
                ca_file: not_pem.clone(),
                ..ClientTlsFiles::default()
            },
        ),
        (
            "no key",
            ClientTlsFiles {
                ca_file: String::new(),
                cert_file: cert.clone(),
                key_file: none.clone(),
            },
        ),
    ] {
        let mut o = redis_opts("127.0.0.1:1");
        o.tls = true;
        o.tls_files = files;
        assert!(
            RedisBackend::new(&o, Duration::from_secs(1)).is_err(),
            "{name}"
        );
    }
    // The store names the kind of a store that cannot be made.
    let mut o = redis_opts("127.0.0.1:1");
    o.tls = true;
    o.tls_files.ca_file = not_pem;
    let e = SharedStore::new(&shared(
        SharedStorageKind::Redis(o),
        Duration::from_secs(30),
    ))
    .unwrap_err();
    assert!(e.contains("shared_storage.redis"), "{e}");
}

/// fit's `TestRedisTLS`: over TLS with a client certificate, authenticating as a user and
/// selecting a database.
#[test]
fn redis_over_tls_with_a_user_and_a_database() {
    let d = TempDir::new("redis-tls");
    let ca = certs::Ca::new("test CA");
    let ca_file = ca.write(&d.0, "ca.pem");
    let (cert, key) = ca.leaf(&d.0, "leaf", &["127.0.0.1"]);
    let chain = Certificate::from_pem(&std::fs::read(&cert).unwrap()).unwrap();
    let k = PrivateKey::from_pem(&std::fs::read(&key).unwrap()).unwrap();
    let tls = ServerConfig::builder(chain, k).build().unwrap();
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let kv: Arc<Mutex<std::collections::HashMap<String, String>>> = Arc::default();
    let (s2, kv2) = (Arc::clone(&seen), Arc::clone(&kv));
    let r = FakeRedis::start(
        Some(tls),
        Arc::new(move |args: &[String]| {
            s2.lock().unwrap().push(args.join(" "));
            match args[0].as_str() {
                "AUTH" | "SELECT" => "+OK\r\n".into(),
                "SET" => {
                    kv2.lock().unwrap().insert(args[1].clone(), args[2].clone());
                    "+OK\r\n".into()
                }
                "GET" => kv2
                    .lock()
                    .unwrap()
                    .get(&args[1])
                    .map_or("$-1\r\n".into(), |v| bulk(v)),
                _ => "-ERR unknown\r\n".into(),
            }
        }),
    );
    let pw = d.file("password", "pw\r\n");
    let mut o = redis_opts(&r.addr.to_string());
    o.tls = true;
    o.tls_files = ClientTlsFiles {
        ca_file: ca_file.display().to_string(),
        cert_file: cert.display().to_string(),
        key_file: key.display().to_string(),
    };
    o.username = "u".into();
    o.password_file = pw;
    o.db = 3;
    let s = SharedStore::new(&shared(
        SharedStorageKind::Redis(o),
        Duration::from_secs(30),
    ))
    .unwrap();
    s.store("a/b", b"v", later()).unwrap();
    assert_eq!(s.load("a/b", later()).unwrap(), b"v");
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen[..2], ["AUTH u pw", "SELECT 3"], "{seen:?}");
    assert_eq!(seen[2], "SET p/a/b v");
    s.close();
}

#[test]
fn redis_dial_errors() {
    let ok = || -> stores::RedisHandle { Arc::new(|_: &[String]| "+OK\r\n".to_owned()) };
    let refuse = |cmd: &'static str| -> stores::RedisHandle {
        Arc::new(move |args: &[String]| {
            if args[0] == cmd {
                "-ERR no\r\n".to_owned()
            } else {
                "+OK\r\n".to_owned()
            }
        })
    };
    let d = TempDir::new("redis-dial");
    let pw = d.file("password", "pw\n");
    let (fine, auth, select) = (
        FakeRedis::start(None, ok()),
        FakeRedis::start(None, refuse("AUTH")),
        FakeRedis::start(None, refuse("SELECT")),
    );
    let cut = cutting_server();
    let mut cases = Vec::new();
    cases.push(("nothing listens", redis_opts(&closed_addr()), ""));
    let mut o = redis_opts(&cut);
    o.tls = true;
    cases.push(("TLS handshake", o, ""));
    let mut o = redis_opts(&fine.addr.to_string());
    o.password_file = d.0.join("none").display().to_string();
    cases.push(("no password", o, "none"));
    let mut o = redis_opts(&auth.addr.to_string());
    o.password_file = pw;
    cases.push(("AUTH refused", o, "AUTH"));
    let mut o = redis_opts(&select.addr.to_string());
    o.db = 1;
    cases.push(("SELECT refused", o, "SELECT"));
    cases.push(("not a host and port", redis_opts("nohost"), "host and port"));
    for (name, o, want) in cases {
        let r = RedisBackend::new(&o, Duration::from_secs(5)).unwrap();
        match r.get("k", later()) {
            Err(CallError::Failed(e)) if e.contains(want) => {}
            other => panic!("{name}: {other:?}"),
        }
        assert_eq!(r.counts(), (0, 0), "{name}: a connection kept");
        r.close();
    }
}

/// fit's `TestRedisReplies`.
#[test]
fn redis_replies_of_every_shape() {
    let replies: Arc<Mutex<std::collections::HashMap<String, String>>> = Arc::default();
    let r2 = Arc::clone(&replies);
    let fake = FakeRedis::start(
        None,
        Arc::new(move |args: &[String]| {
            let m = r2.lock().unwrap();
            let k = if args[0] == "EVAL" {
                format!("EVAL {}", args[3])
            } else {
                args[0].clone()
            };
            m.get(&k).cloned().unwrap_or_default()
        }),
    );
    let answer = |cmd: &str, reply: &str| {
        replies.lock().unwrap().insert(cmd.into(), reply.into());
    };
    let r = RedisBackend::new(&redis_opts(&fake.addr.to_string()), Duration::from_secs(5)).unwrap();

    // An error Redis answers keeps the connection.
    answer("PING", "+PONG\r\n");
    assert_eq!(r.run(&[b"PING"], later()), Ok(Reply::Simple("PONG".into())));
    answer("GET", "-ERR wrong\r\n");
    assert!(matches!(r.get("k", later()), Err(CallError::Failed(e)) if e == "ERR wrong"));
    assert_eq!(r.counts(), (1, 1));
    answer("GET", ":1\r\n");
    assert!(matches!(r.get("k", later()), Err(CallError::Failed(e)) if e.contains("integer")));
    // A cut connection is not kept.
    answer("GET", "");
    assert!(r.get("k", later()).is_err());
    assert_eq!(r.counts(), (0, 0));

    // SCAN: errors, a reply of another shape, pages, and too many keys.
    answer("SCAN", "-ERR scan\r\n");
    assert!(r.list("p/", later()).is_err());
    answer("SCAN", "+OK\r\n");
    assert!(matches!(r.list("p/", later()), Err(e) if e.contains("cursor")));
    let paged = FakeRedis::start(
        None,
        Arc::new(|args: &[String]| {
            if args[1] == "0" {
                array(&[bulk("7"), array(&[bulk("p/a"), ":1\r\n".into()])])
            } else {
                array(&[bulk("0"), array(&[bulk("p/b")])])
            }
        }),
    );
    let r2 =
        RedisBackend::new(&redis_opts(&paged.addr.to_string()), Duration::from_secs(5)).unwrap();
    assert_eq!(r2.list("p/", later()).unwrap(), ["p/a", "p/b"]);
    let page = array(&[bulk("1"), array(&vec![bulk("k"); MAX_KEYS / 2 + 1])]);
    let many = FakeRedis::start(None, Arc::new(move |_: &[String]| page.clone()));
    let r3 =
        RedisBackend::new(&redis_opts(&many.addr.to_string()), Duration::from_secs(5)).unwrap();
    assert!(matches!(r3.list("p/", later()), Err(e) if e.contains("more than")));

    // Locks: an error, held by another, taken, renewed, lost and given back.
    answer("SET", "-ERR set\r\n");
    assert!(r.try_lock("l", Duration::from_secs(1), later()).is_err());
    answer("SET", "$-1\r\n");
    assert!(matches!(
        r.try_lock("l", Duration::from_secs(1), later()),
        Ok(None)
    ));
    answer("SET", "+OK\r\n");
    let l = r
        .try_lock("l", Duration::from_secs(1), later())
        .unwrap()
        .unwrap();
    answer("EVAL l", ":1\r\n");
    l.renew(later()).unwrap();
    answer("EVAL l", ":0\r\n");
    assert!(matches!(l.renew(later()), Err(e) if e.contains("lost")));
    answer("EVAL l", "-ERR eval\r\n");
    assert!(l.renew(later()).is_err());
    answer("EVAL l", ":1\r\n");
    l.release(later()).unwrap();
    for x in [&r, &r2, &r3] {
        x.close();
    }
}

/// fit's `TestRedisConnFaults`: every connection busy, a command waits for one within its
/// deadline.
#[test]
fn redis_with_every_connection_busy() {
    let r = RedisBackend::new(&redis_opts(&closed_addr()), Duration::from_secs(1)).unwrap();
    r.fill();
    let start = Instant::now();
    let e = r
        .run(&[b"PING"], Instant::now() + Duration::from_millis(50))
        .unwrap_err();
    assert!(e.text().contains("busy"));
    assert!(start.elapsed() < Duration::from_secs(1));
}

fn etcd(endpoints: &[String]) -> EtcdBackend {
    EtcdBackend::new(&etcd_opts(endpoints)).unwrap()
}

/// fit's `TestEtcdEndpoints`.
#[test]
fn etcd_endpoints_in_turn() {
    let mut o = etcd_opts(&["http://127.0.0.1:1".into()]);
    o.tls.ca_file = "/nonexistent/ca.pem".into();
    assert!(EtcdBackend::new(&o).is_err());
    let rs = Arc::new(Replies::default());
    rs.set("/v3/kv/range", 200, r#"{"kvs":[{"value":"dg=="}]}"#);
    let srv = rs.serve();
    // The first endpoint cannot be reached; the second is used, now and after.
    let e = etcd(&[
        format!("http://{}", closed_addr()),
        format!("{}/", srv.url()),
    ]);
    for _ in 0..2 {
        assert_eq!(e.get("k", later()).unwrap(), b"v");
    }
    assert_eq!(e.inner().next(), 1);
    // An answer that is not 200 is not tried elsewhere.
    rs.set("/v3/kv/put", 500, "boom");
    let err = e.put("k", b"v", later()).unwrap_err();
    assert!(err.contains("500") && err.contains("boom"), "{err}");
    assert_eq!(e.inner().next(), 1);
    // No endpoint can be reached.
    let e2 = etcd(&[
        format!("http://{}", closed_addr()),
        format!("http://{}", closed_addr()),
    ]);
    assert!(e2.del("k", later()).is_err());
    // An endpoint that is not a URL.
    let e3 = etcd(&["http://a b".into()]);
    assert!(e3.del("k", later()).is_err());
}

/// fit's `TestEtcdAnswers`.
#[test]
fn etcd_answers() {
    let rs = Arc::new(Replies::default());
    let srv = rs.serve();
    let e = etcd(&[srv.url()]);
    for (name, reply, want) in [
        ("empty", HttpReply::Full(200, String::new()), "empty answer"),
        (
            "not JSON",
            HttpReply::Full(200, "{\n".into()),
            "POST /v3/kv/range",
        ),
        ("cut short", HttpReply::Cut, "POST /v3/kv/range"),
    ] {
        let reply = Mutex::new(Some(reply));
        rs.set_fn("/v3/kv/range", move |_| {
            reply
                .lock()
                .unwrap()
                .take()
                .unwrap_or(HttpReply::Full(500, "again".into()))
        });
        match e.get("k", later()) {
            Err(CallError::Failed(m)) if m.contains(want) => {}
            r => panic!("{name}: {r:?}"),
        }
    }
    rs.set("/v3/kv/range", 200, "{}");
    assert!(matches!(e.get("k", later()), Err(CallError::NotFound)));

    // list
    rs.set("/v3/kv/range", 500, "");
    assert!(e.list("p/", later()).is_err());
    rs.set(
        "/v3/kv/range",
        200,
        r#"{"kvs":[{"key":"cC9h"}],"more":true}"#,
    );
    assert!(matches!(e.list("p/", later()), Err(m) if m.contains("more than")));
    rs.set("/v3/kv/range", 200, r#"{"kvs":[{"key":"cC9h"}]}"#);
    assert_eq!(e.list("p/", later()).unwrap(), ["p/a"]);
    for (from, to) in [
        (&b"a\xff"[..], &b"b"[..]),
        (b"\xff\xff", b"\x00"),
        (b"ab", b"ac"),
    ] {
        assert_eq!(range_end(from), to, "{from:?}");
    }

    // Locks.
    rs.set("/v3/lease/grant", 500, "");
    assert!(e.try_lock("l", Duration::from_secs(1), later()).is_err());
    rs.set("/v3/lease/grant", 200, r#"{"error":"too many leases"}"#);
    assert!(
        matches!(e.try_lock("l", Duration::from_secs(1), later()), Err(m) if m.contains("too many leases"))
    );
    rs.set("/v3/lease/grant", 200, r#"{"ID":"7"}"#);
    let revoked = Arc::new(AtomicUsize::new(0));
    let r2 = Arc::clone(&revoked);
    rs.set_fn("/v3/lease/revoke", move |_| {
        r2.fetch_add(1, Ordering::SeqCst);
        HttpReply::Full(200, "{}".into())
    });
    rs.set("/v3/kv/txn", 500, "");
    assert!(e.try_lock("l", Duration::from_secs(1), later()).is_err());
    assert_eq!(revoked.load(Ordering::SeqCst), 1);
    rs.set("/v3/kv/txn", 200, r#"{"succeeded":false}"#);
    assert!(matches!(
        e.try_lock("l", Duration::from_secs(1), later()),
        Ok(None)
    ));
    assert_eq!(revoked.load(Ordering::SeqCst), 2);
    rs.set("/v3/kv/txn", 200, r#"{"succeeded":true}"#);
    let l = e
        .try_lock("l", Duration::from_secs(1), later())
        .unwrap()
        .unwrap();
    rs.set("/v3/lease/keepalive", 500, "");
    assert!(l.renew(later()).is_err());
    rs.set(
        "/v3/lease/keepalive",
        200,
        "{\"result\":{\"TTL\":\"0\"}}\n{\"result\":{\"TTL\":\"0\"}}",
    );
    assert!(matches!(l.renew(later()), Err(m) if m.contains("lost")));
    rs.set("/v3/lease/keepalive", 200, r#"{"result":{"TTL":"3"}}"#);
    l.renew(later()).unwrap();
    rs.set(
        "/v3/lease/revoke",
        404,
        r#"{"error":"etcdserver: requested lease not found"}"#,
    );
    l.release(later()).unwrap();
    rs.set("/v3/lease/revoke", 500, "boom");
    assert!(l.release(later()).is_err());
}

/// fit's `TestEtcdAuth`.
#[test]
fn etcd_authenticates() {
    let rs = Arc::new(Replies::default());
    let tokens = Arc::new(AtomicUsize::new(0));
    let t2 = Arc::clone(&tokens);
    rs.set_fn("/v3/auth/authenticate", move |req| {
        if !String::from_utf8_lossy(&req.body).contains(r#""password":"pw""#) {
            return HttpReply::Full(401, "authentication failed".into());
        }
        HttpReply::Full(
            200,
            format!(
                r#"{{"token":"T{}"}}"#,
                t2.fetch_add(1, Ordering::SeqCst) + 1
            ),
        )
    });
    // The first token is refused as if it had expired.
    rs.set_fn("/v3/kv/put", |req| {
        if req.header("Authorization") == "T1" {
            return HttpReply::Full(401, "etcdserver: invalid auth token".into());
        }
        HttpReply::Full(200, "{}".into())
    });
    let srv = rs.serve();
    let d = TempDir::new("etcd-auth");
    let mut o = etcd_opts(&[srv.url()]);
    o.username = "u".into();
    o.password_file = d.file("pw", "pw\n");
    let e = EtcdBackend::new(&o).unwrap();
    e.put("k", b"v", later()).unwrap();
    assert!(rs.last().ends_with(" T2"), "{}", rs.last());
    assert_eq!(e.inner().token(), "T2");
    // A password refused, and one that cannot be read.
    o.password_file = d.file("pw2", "wrong");
    let bad = EtcdBackend::new(&o).unwrap();
    assert!(matches!(bad.put("k", b"", later()), Err(m) if m.contains("authentication failed")));
    o.password_file = d.0.join("none").display().to_string();
    let none = EtcdBackend::new(&o).unwrap();
    assert!(matches!(none.put("k", b"", later()), Err(m) if m.contains("none")));
}

/// fit's `TestConsul`.
#[test]
fn consul_answers() {
    let d = TempDir::new("consul");
    let mut o = consul_opts("http://127.0.0.1:1");
    o.token_file = d.0.join("none").display().to_string();
    assert!(ConsulBackend::new(&o).is_err());
    assert!(
        SharedStore::new(&shared(
            SharedStorageKind::Consul(o),
            Duration::from_secs(30)
        ))
        .is_err()
    );
    let mut o = consul_opts("http://127.0.0.1:1");
    o.tls.ca_file = d.0.join("none").display().to_string();
    assert!(ConsulBackend::new(&o).is_err());

    let rs = Arc::new(Replies::default());
    let srv = rs.serve();
    let mut o = consul_opts(&format!("{}/", srv.url()));
    o.datacenter = "dc2".into();
    o.token_file = d.file("token", " tok\n");
    let b = ConsulBackend::new(&o).unwrap();

    rs.set("/v1/kv/p/a", 200, "v");
    assert_eq!(b.get("p/a", later()).unwrap(), b"v");
    assert_eq!(rs.last(), "GET /v1/kv/p/a?dc=dc2&raw= tok");
    rs.set("/v1/kv/p/a", 500, "boom");
    assert!(matches!(b.get("p/a", later()), Err(CallError::Failed(m)) if m.contains("boom")));
    rs.set("/v1/kv/p/a", 200, "false");
    assert!(matches!(b.put("p/a", b"", later()), Err(m) if m.contains("false")));

    // list
    rs.set("/v1/kv/p/", 200, "nope");
    assert!(b.list("p/", later()).is_err());
    let many = vec!["\"k\""; MAX_KEYS + 1].join(",");
    rs.set("/v1/kv/p/", 200, &format!("[{many}]"));
    assert!(matches!(b.list("p/", later()), Err(m) if m.contains("more than")));
    rs.set("/v1/kv/p/", 404, "");
    assert!(b.list("p/", later()).unwrap().is_empty());

    // Locks.
    let destroyed = Arc::new(AtomicUsize::new(0));
    let d2 = Arc::clone(&destroyed);
    rs.set_fn("/v1/session/destroy/s1", move |_| {
        d2.fetch_add(1, Ordering::SeqCst);
        HttpReply::Full(200, "true".into())
    });
    rs.set("/v1/session/create", 500, "");
    assert!(b.try_lock("p/l", Duration::from_secs(10), later()).is_err());
    rs.set("/v1/session/create", 200, "{}");
    assert!(
        matches!(b.try_lock("p/l", Duration::from_secs(10), later()), Err(m) if m.contains("no session"))
    );
    rs.set("/v1/session/create", 200, r#"{"ID":"s1"}"#);
    rs.set("/v1/kv/p/l", 500, "");
    assert!(b.try_lock("p/l", Duration::from_secs(10), later()).is_err());
    assert_eq!(destroyed.load(Ordering::SeqCst), 1);
    rs.set("/v1/kv/p/l", 200, "false");
    assert!(matches!(
        b.try_lock("p/l", Duration::from_secs(10), later()),
        Ok(None)
    ));
    assert_eq!(destroyed.load(Ordering::SeqCst), 2);
    rs.set("/v1/kv/p/l", 200, "true");
    let l = b
        .try_lock("p/l", Duration::from_secs(10), later())
        .unwrap()
        .unwrap();
    assert!(
        rs.last().starts_with("PUT /v1/kv/p/l?acquire=s1&dc=dc2"),
        "{}",
        rs.last()
    );
    assert!(matches!(l.renew(later()), Err(m) if m.contains("lost")));
    rs.set("/v1/session/renew/s1", 200, "[]");
    l.renew(later()).unwrap();
    l.release(later()).unwrap();

    let bad = ConsulBackend::new(&consul_opts("http://a b")).unwrap();
    assert!(bad.del("k", later()).is_err());
}

/// A backend whose calls fail with `err`, and whose locks are taken with `lock`.
struct FakeBackend {
    err: String,
    lock: Option<Box<dyn Fn() -> Option<Box<dyn Lease>> + Send + Sync>>,
    closed: Arc<AtomicBool>,
}

impl Backend for FakeBackend {
    fn get(&self, _: &str, _: Instant) -> Result<Vec<u8>, CallError> {
        Err(CallError::Failed(self.err.clone()))
    }
    fn put(&self, _: &str, _: &[u8], _: Instant) -> Result<(), String> {
        Err(self.err.clone())
    }
    fn del(&self, _: &str, _: Instant) -> Result<(), String> {
        Err(self.err.clone())
    }
    fn list(&self, _: &str, _: Instant) -> Result<Vec<String>, String> {
        Err(self.err.clone())
    }
    fn try_lock(&self, _: &str, _: Duration, _: Instant) -> Result<Option<Box<dyn Lease>>, String> {
        match &self.lock {
            Some(f) => Ok(f()),
            None => Err(self.err.clone()),
        }
    }
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

#[derive(Clone, Default)]
struct FakeLease {
    err: Option<String>,
    released: Arc<AtomicUsize>,
}

impl Lease for FakeLease {
    fn renew(&self, _: Instant) -> Result<(), String> {
        self.err.clone().map_or(Ok(()), Err)
    }
    fn release(&self, _: Instant) -> Result<(), String> {
        self.released.fetch_add(1, Ordering::SeqCst);
        self.err.clone().map_or(Ok(()), Err)
    }
}

fn fake_store(b: FakeBackend, ttl: Duration) -> SharedStore {
    SharedStore::with_backend(
        Box::new(b),
        "fake",
        &shared(SharedStorageKind::Redis(redis_opts("x:1")), ttl),
        Duration::from_millis(20),
    )
}

/// fit's `TestStoreErrors`.
#[test]
fn store_errors() {
    let s = fake_store(
        FakeBackend {
            err: "boom".into(),
            lock: None,
            closed: Arc::default(),
        },
        Duration::from_secs(60),
    );
    let boom = |e: StorageError| matches!(&e, StorageError::Io(io) if io.to_string().contains("fake: ") && io.to_string().contains("boom"));
    assert!(matches!(
        s.load("/a", later()),
        Err(StorageError::BadKey(_))
    ));
    assert!(boom(s.load("a", later()).unwrap_err()));
    assert!(boom(s.store("a", b"", later()).unwrap_err()));
    assert!(matches!(
        s.delete("a/../b", later()),
        Err(StorageError::BadKey(_))
    ));
    assert!(boom(s.delete("a", later()).unwrap_err()));
    assert!(matches!(
        s.list("a/./b", later()),
        Err(StorageError::BadKey(_))
    ));
    assert!(boom(s.list("a", later()).unwrap_err()));
    assert!(matches!(
        s.lock("a//b", later()),
        Err(StorageError::BadKey(_))
    ));
    assert!(boom(s.lock("a", later()).unwrap_err()));
    assert!(matches!(s.unlock("a"), Err(StorageError::NotHeld(_))));
    // An etcd store is made without reaching etcd.
    let s = SharedStore::new(&shared(
        SharedStorageKind::Etcd(etcd_opts(&["http://127.0.0.1:2379".into()])),
        Duration::from_secs(30),
    ))
    .unwrap();
    assert_eq!(s.kind(), "etcd");
    s.close();
}

/// fit's `TestStoreLocks`.
#[test]
fn store_locks() {
    // A lock whose renewals fail is reported, and one that cannot be given back says so.
    let l = FakeLease {
        err: Some("lost".into()),
        released: Arc::default(),
    };
    let l2 = l.clone();
    let s = fake_store(
        FakeBackend {
            err: String::new(),
            lock: Some(Box::new(move || {
                Some(Box::new(l2.clone()) as Box<dyn Lease>)
            })),
            closed: Arc::default(),
        },
        Duration::from_millis(30),
    );
    let (tx, rx) = std::sync::mpsc::sync_channel::<String>(1);
    s.set_log(Arc::new(move |line: &str| {
        let _ = tx.try_send(line.to_owned());
    }));
    s.lock("a/b", later()).unwrap();
    let msg = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(msg.contains("renewing lock a/b: lost"), "{msg}");
    let e = s.unlock("a/b").unwrap_err();
    assert!(e.to_string().contains("giving back"), "{e}");

    // Close gives back the locks held.
    let l = FakeLease::default();
    let l2 = l.clone();
    let closed = Arc::new(AtomicBool::new(false));
    let s2 = fake_store(
        FakeBackend {
            err: String::new(),
            lock: Some(Box::new(move || {
                Some(Box::new(l2.clone()) as Box<dyn Lease>)
            })),
            closed: Arc::clone(&closed),
        },
        Duration::from_secs(60),
    );
    for name in ["x", "y"] {
        s2.lock(name, later()).unwrap();
    }
    s2.close();
    assert_eq!(l.released.load(Ordering::SeqCst), 2);
    assert!(closed.load(Ordering::SeqCst));
    assert!(matches!(s2.unlock("x"), Err(StorageError::NotHeld(_))));

    // A lock held by this store already is waited for, here until the deadline.
    let s3 = fake_store(
        FakeBackend {
            err: String::new(),
            lock: Some(Box::new(|| {
                Some(Box::new(FakeLease::default()) as Box<dyn Lease>)
            })),
            closed: Arc::default(),
        },
        Duration::from_secs(60),
    );
    s3.lock("z", later()).unwrap();
    assert!(matches!(
        s3.lock("z", Instant::now() + Duration::from_millis(100)),
        Err(StorageError::Timeout)
    ));
    s3.unlock("z").unwrap();
}

/// The fake HTTP server is torn down with its test.
#[test]
fn fakes_stop() {
    let srv = FakeHttp::start(Arc::new(|_: &stores::HttpReq| {
        HttpReply::Full(200, String::new())
    }));
    drop(srv);
}
