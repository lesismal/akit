//! The file system storage and its locks, as fit's `tlsmgr` storage tests.
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

use akit_tls::{FileStorage, MAX_VALUE_BYTES, Storage, StorageError};
use support::certs::dir;

fn later() -> Instant {
    Instant::now() + Duration::from_secs(10)
}

#[test]
fn stores_loads_lists_and_deletes() {
    let root = dir("storage");
    let s = FileStorage::new(&root);
    assert!(matches!(
        s.load("certs/a.example/cert.pem", later()),
        Err(StorageError::NotFound)
    ));
    for (k, v) in [
        ("certs/a.example/cert.pem", "a cert"),
        ("certs/a.example/key.pem", "a key"),
        ("certs/b.example/cert.pem", "b cert"),
        ("accounts/acme/key.pem", "account"),
    ] {
        s.store(k, v.as_bytes(), later()).unwrap();
    }
    s.store("certs/a.example/cert.pem", b"a cert, renewed", later())
        .unwrap();
    assert_eq!(
        s.load("certs/a.example/cert.pem", later()).unwrap(),
        b"a cert, renewed"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode();
        assert_eq!(mode(&root.join("certs/a.example/key.pem")) & 0o077, 0);
        assert_eq!(mode(&root.join("certs/a.example")) & 0o077, 0);
    }

    // A lock held does not show in list.
    s.lock("issue/a.example", later()).unwrap();
    let list = |p: &str| s.list(p, later()).unwrap();
    assert_eq!(
        list(""),
        [
            "accounts/acme/key.pem",
            "certs/a.example/cert.pem",
            "certs/a.example/key.pem",
            "certs/b.example/cert.pem"
        ]
    );
    assert_eq!(
        list("certs"),
        [
            "certs/a.example/cert.pem",
            "certs/a.example/key.pem",
            "certs/b.example/cert.pem"
        ]
    );
    assert_eq!(
        list("/certs/a.example/"),
        ["certs/a.example/cert.pem", "certs/a.example/key.pem"]
    );
    assert!(list("missing").is_empty());
    s.unlock("issue/a.example").unwrap();

    s.delete("certs/b.example/cert.pem", later()).unwrap();
    s.delete("certs/b.example/cert.pem", later()).unwrap();
    assert!(matches!(
        s.load("certs/b.example/cert.pem", later()),
        Err(StorageError::NotFound)
    ));

    for key in [
        "", "/abs", "a/../b", "a//b", "./a", "a/.", "a\\b", "..", "a\0",
    ] {
        let e = s.store(key, b"", later()).unwrap_err();
        assert!(e.to_string().contains("bad storage key"), "{key:?}: {e}");
        assert!(s.load(key, later()).is_err(), "{key:?}");
    }
    let big = vec![0u8; usize::try_from(MAX_VALUE_BYTES + 1).unwrap()];
    assert!(matches!(
        s.store("big", &big, later()),
        Err(StorageError::TooLarge(_))
    ));
    std::fs::write(root.join("big"), &big).unwrap();
    assert!(matches!(
        s.load("big", later()),
        Err(StorageError::TooLarge(_))
    ));
    // A deadline passed: nothing is done.
    assert!(matches!(
        s.store("x", b"", Instant::now()),
        Err(StorageError::Timeout)
    ));
    assert!(matches!(s.load("x", later()), Err(StorageError::NotFound)));
    std::fs::remove_dir_all(&root).unwrap();
}

/// Holders in two storages on the same directory, as two instances of akit would be, take a lock
/// in turn.
#[test]
fn a_lock_is_held_by_one_at_a_time() {
    let root = dir("storage-lock");
    let poll = Duration::from_millis(5);
    let stores = [
        Arc::new(FileStorage::new(&root).lock_poll(poll)),
        Arc::new(FileStorage::new(&root).lock_poll(poll)),
    ];
    let inside = Arc::new(AtomicUsize::new(0));
    let overlaps = Arc::new(AtomicUsize::new(0));
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let s = Arc::clone(&stores[i % 2]);
            let (inside, overlaps) = (Arc::clone(&inside), Arc::clone(&overlaps));
            std::thread::spawn(move || {
                for _ in 0..5 {
                    s.lock("issue/a.example", later()).unwrap();
                    if inside.fetch_add(1, Ordering::SeqCst) > 0 {
                        overlaps.fetch_add(1, Ordering::SeqCst);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                    inside.fetch_sub(1, Ordering::SeqCst);
                    s.unlock("issue/a.example").unwrap();
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(overlaps.load(Ordering::SeqCst), 0);
    assert!(matches!(
        stores[0].unlock("issue/a.example"),
        Err(StorageError::NotHeld(_))
    ));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_fresh_lock_is_waited_for() {
    let root = dir("storage-wait");
    let store = || {
        FileStorage::new(&root)
            .lock_refresh(Duration::from_millis(20))
            .lock_stale(Duration::from_secs(1))
            .lock_poll(Duration::from_millis(5))
    };
    let (a, b) = (store(), Arc::new(store()));
    a.lock("x", later()).unwrap();
    // A holder that keeps its lock fresh keeps it past the stale time.
    let short = Instant::now() + Duration::from_millis(2500);
    assert!(matches!(b.lock("x", short), Err(StorageError::Timeout)));
    // Once given back, the waiter gets it.
    let waiter = {
        let b = Arc::clone(&b);
        std::thread::spawn(move || b.lock("x", later()))
    };
    std::thread::sleep(Duration::from_millis(20));
    a.unlock("x").unwrap();
    waiter.join().unwrap().unwrap();
    b.unlock("x").unwrap();
    std::fs::remove_dir_all(&root).unwrap();
}

/// A holder that stopped without giving its lock back loses it once it goes stale.
#[test]
fn a_stale_lock_is_taken_over() {
    let root = dir("storage-stale");
    let s = FileStorage::new(&root)
        .lock_stale(Duration::from_millis(200))
        .lock_poll(Duration::from_millis(5));
    let file = root.join("locks/x.lock");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "1\n").unwrap();
    let start = Instant::now();
    s.lock("x", later()).unwrap();
    assert!(
        start.elapsed() >= Duration::from_millis(150),
        "a fresh lock was taken over after {:?}",
        start.elapsed()
    );
    s.unlock("x").unwrap();
    assert!(!file.exists(), "lock file left");
    // A name with slashes is one file.
    s.lock("issue/a.example", later()).unwrap();
    assert!(root.join("locks/issue_a.example.lock").exists());
    s.unlock("issue/a.example").unwrap();
    std::fs::remove_dir_all(&root).unwrap();
}

/// I80b (the threat model's L3): a storage directory left open to others, and the directories
/// under it written into, are tightened to 0700 by a store or a lock; a file stored replaces the
/// old one whole.
#[cfg(unix)]
#[test]
fn an_open_storage_directory_is_tightened() {
    use std::os::unix::fs::PermissionsExt;
    let root = dir("storage-open");
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    let open = |p: &std::path::Path| {
        std::fs::create_dir_all(p).unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    open(&root);
    open(&root.join("certs/a.example"));
    open(&root.join("other"));
    let s = FileStorage::new(&root);
    s.store("certs/a.example/key.pem", b"a key", later())
        .unwrap();
    assert_eq!(mode(&root), 0o700);
    assert_eq!(mode(&root.join("certs")), 0o700);
    assert_eq!(mode(&root.join("certs/a.example")), 0o700);
    assert_eq!(mode(&root.join("certs/a.example/key.pem")), 0o600);
    // A directory it does not write into is left as it is.
    assert_eq!(mode(&root.join("other")), 0o755);
    s.store("certs/a.example/key.pem", b"a key, renewed", later())
        .unwrap();
    assert_eq!(
        s.load("certs/a.example/key.pem", later()).unwrap(),
        b"a key, renewed"
    );
    let names: Vec<String> = std::fs::read_dir(root.join("certs/a.example"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["key.pem"], "no file left beside it");
    // A lock tightens its directory too.
    open(&root.join("locks"));
    s.lock("issue/a.example", later()).unwrap();
    assert_eq!(mode(&root.join("locks")), 0o700);
    s.unlock("issue/a.example").unwrap();
}
