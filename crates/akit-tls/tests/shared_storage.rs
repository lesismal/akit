//! The shared storage (I62), as fit's `sharedstore/store_test.go` `TestStores`: two stores of
//! each kind on one fake server (Redis, etcd's gateway, Consul), as two instances on two hosts,
//! share keys and take leased locks in turn; a lock held past its TTL is renewed, and one left
//! behind by a holder that stopped is taken once its TTL passes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use akit_config::{
    ClientTlsFiles, ConsulStorage, EtcdStorage, RedisStorage, SharedStorage, SharedStorageKind,
};
use akit_tls::{MAX_VALUE_BYTES, SharedStore, Storage, StorageError};
use support::stores::{FakeHttp, FakeRedis, consul_semantics, etcd_semantics};

/// The lock TTL of these tests: short, so that a lock left behind runs out soon.
const TTL: Duration = Duration::from_secs(2);

fn later() -> Instant {
    Instant::now() + Duration::from_secs(10)
}

fn config(kind: SharedStorageKind) -> SharedStorage {
    SharedStorage {
        kind,
        prefix: "akit/".into(),
        lock_ttl: TTL,
        request_timeout: Duration::from_secs(5),
    }
}

fn open(c: &SharedStorage) -> Arc<SharedStore> {
    let s = SharedStore::new(c)
        .unwrap()
        .lock_poll(Duration::from_millis(50));
    Arc::new(s)
}

fn redis(addr: &str) -> SharedStorageKind {
    SharedStorageKind::Redis(RedisStorage {
        address: addr.into(),
        username: String::new(),
        password_file: String::new(),
        db: 0,
        tls: false,
        tls_files: ClientTlsFiles::default(),
    })
}

fn run(c: &SharedStorage, kind: &str) {
    let (a, b) = (open(c), open(c));
    assert_eq!(a.kind(), kind);
    assert!(a.shared());
    keys(&a, &b);
    locks(&a, &b);
    a.close();
    b.close();
}

#[test]
fn redis_shares_keys_and_locks() {
    let r = FakeRedis::semantic();
    run(&config(redis(&r.addr.to_string())), "redis");
}

#[test]
fn etcd_shares_keys_and_locks() {
    let e = FakeHttp::start(etcd_semantics());
    run(
        &config(SharedStorageKind::Etcd(EtcdStorage {
            endpoints: vec![e.url()],
            username: String::new(),
            password_file: String::new(),
            tls: ClientTlsFiles::default(),
        })),
        "etcd",
    );
}

#[test]
fn consul_shares_keys_and_locks() {
    let c = FakeHttp::start(consul_semantics());
    run(
        &config(SharedStorageKind::Consul(ConsulStorage {
            endpoint: c.url(),
            datacenter: String::new(),
            token_file: String::new(),
            tls: ClientTlsFiles::default(),
        })),
        "consul",
    );
}

/// fit's `testKeys`.
fn keys(a: &SharedStore, b: &SharedStore) {
    assert!(matches!(
        a.load("certificates/ca/none.pem", later()),
        Err(StorageError::NotFound)
    ));
    for (k, v) in [
        ("certificates/ca/a.pem", "A"),
        ("certificates/ca/b.pem", "B"),
        ("acme/ca/account.pem", "K"),
        ("a?b/c", "glob"),
        ("aXb/c", "not"),
    ] {
        a.store(k, v.as_bytes(), later()).unwrap();
    }
    a.store("certificates/ca/a.pem", b"A2", later()).unwrap();
    assert_eq!(b.load("certificates/ca/a.pem", later()).unwrap(), b"A2");
    // A lock is not a key.
    a.lock("issue/ca/a", later()).unwrap();
    assert_eq!(
        b.list("", later()).unwrap(),
        [
            "a?b/c",
            "aXb/c",
            "acme/ca/account.pem",
            "certificates/ca/a.pem",
            "certificates/ca/b.pem"
        ]
    );
    a.unlock("issue/ca/a").unwrap();
    assert_eq!(
        b.list("certificates/", later()).unwrap(),
        ["certificates/ca/a.pem", "certificates/ca/b.pem"]
    );
    assert_eq!(b.list("a?b", later()).unwrap(), ["a?b/c"]);
    assert!(b.list("nothing", later()).unwrap().is_empty());
    b.delete("certificates/ca/b.pem", later()).unwrap();
    b.delete("certificates/ca/b.pem", later()).unwrap();
    assert!(matches!(
        a.load("certificates/ca/b.pem", later()),
        Err(StorageError::NotFound)
    ));
    for bad in ["", "/a", "a//b", "a/../b", "./a", "a\\b"] {
        assert!(
            matches!(a.store(bad, b"", later()), Err(StorageError::BadKey(_))),
            "{bad:?}"
        );
    }
    let big = vec![0u8; usize::try_from(MAX_VALUE_BYTES).unwrap() + 1];
    assert!(matches!(
        a.store("big", &big, later()),
        Err(StorageError::TooLarge(_))
    ));
    // A deadline passed already.
    assert!(matches!(
        a.load("certificates/ca/a.pem", Instant::now()),
        Err(StorageError::Timeout)
    ));
}

/// fit's `testLocks`.
fn locks(a: &Arc<SharedStore>, b: &Arc<SharedStore>) {
    // Eight holders over the two stores take the lock in turn, never two at once.
    let holding = Arc::new(AtomicUsize::new(0));
    let most = Arc::new(AtomicUsize::new(0));
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let s = if i % 2 == 1 {
                Arc::clone(b)
            } else {
                Arc::clone(a)
            };
            let (holding, most) = (Arc::clone(&holding), Arc::clone(&most));
            std::thread::spawn(move || {
                s.lock("issue/ca/turns", Instant::now() + Duration::from_secs(60))
                    .unwrap();
                let n = holding.fetch_add(1, Ordering::SeqCst) + 1;
                most.fetch_max(n, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(20));
                holding.fetch_sub(1, Ordering::SeqCst);
                s.unlock("issue/ca/turns").unwrap();
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(most.load(Ordering::SeqCst), 1);
    assert!(matches!(
        a.unlock("issue/ca/turns"),
        Err(StorageError::NotHeld(_))
    ));

    // A lock held past its TTL is renewed, and waited for; given back, it is taken.
    a.lock("issue/ca/long", later()).unwrap();
    assert!(matches!(
        b.lock("issue/ca/long", Instant::now() + TTL + TTL / 2),
        Err(StorageError::Timeout)
    ));
    let b2 = Arc::clone(b);
    let got = std::thread::spawn(move || {
        b2.lock("issue/ca/long", Instant::now() + Duration::from_secs(60))
    });
    std::thread::sleep(Duration::from_millis(100));
    a.unlock("issue/ca/long").unwrap();
    got.join().unwrap().unwrap();
    b.unlock("issue/ca/long").unwrap();

    // A holder that stops renewing without giving it back loses the lock once its TTL passes.
    a.lock("issue/ca/left", later()).unwrap();
    a.abandon_for_tests("issue/ca/left");
    let start = Instant::now();
    b.lock(
        "issue/ca/left",
        Instant::now() + 3 * TTL + Duration::from_secs(5),
    )
    .unwrap();
    eprintln!("a lock left behind was taken after {:?}", start.elapsed());
    b.unlock("issue/ca/left").unwrap();
}
