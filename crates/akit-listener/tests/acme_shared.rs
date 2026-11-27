//! A shared storage (I62, `acme.shared_storage`) against Pebble, as fit's `TestSharedPebble` and
//! `test/e2e` `TestACMESharedStorage`: two managers on one Redis, as two instances on two hosts.
//! The CA reaches only the second, whose listener answers the first's challenge from the
//! storage (HTTP-01 in the clear, and TLS-ALPN-01 in a handshake); once the first has its
//! certificate, the second, asked for the same name, takes it from the storage: one certificate,
//! one issuance. On the backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`). Skipped
//! without Pebble's binaries (support/pebble.rs).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

#[path = "../../akit-tls/tests/support/stores.rs"]
mod stores;
mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use akio::{Backend, UringConfig};
use akit_config::{Challenge, ClientTlsFiles, RedisStorage, SharedStorage, SharedStorageKind};
use akit_listener::Listeners;
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};
use akit_tls::{Acme, SharedStore, Storage};
use stores::FakeRedis;
use support::certs::dir;
use support::pebble::{Pebble, free_port};

const NAME: &str = "shared.akit.test";

fn options() -> akit_listener::Options {
    let mut o = akit_listener::Options::default();
    o.backend = Some(match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    });
    o.shutdown_timeout = Duration::from_secs(5);
    o
}

fn storage(redis: &FakeRedis) -> SharedStorage {
    SharedStorage {
        kind: SharedStorageKind::Redis(RedisStorage {
            address: redis.addr.to_string(),
            username: String::new(),
            password_file: String::new(),
            db: 0,
            tls: false,
            tls_files: ClientTlsFiles::default(),
        }),
        prefix: "akit/".into(),
        lock_ttl: Duration::from_secs(10),
        request_timeout: Duration::from_secs(5),
    }
}

/// A manager on the shared storage, its log lines counted in `obtained`.
fn manager(
    p: &Pebble,
    redis: &FakeRedis,
    challenge: Challenge,
    obtained: &Arc<AtomicUsize>,
) -> Acme {
    let a = Acme::new(&akit_config::Acme {
        directory: p.directory.clone(),
        ca_file: p.ca_file.display().to_string(),
        timeout: Duration::from_secs(60),
        request_timeout: Duration::from_secs(10),
        challenges: vec![challenge],
        shared_storage: Some(storage(redis)),
        ..akit_config::Acme::default()
    })
    .unwrap();
    assert!(a.shared());
    let o = Arc::clone(obtained);
    a.set_log(Arc::new(move |line: &str| {
        eprintln!("acme: {line}");
        if line.contains("obtained a certificate") {
            o.fetch_add(1, Ordering::SeqCst);
        }
    }));
    a
}

/// A Redis with the store's semantics, counting the lookups of challenges.
fn redis(lookups: &Arc<AtomicUsize>) -> FakeRedis {
    let inner = stores::redis_semantics();
    let l = Arc::clone(lookups);
    FakeRedis::start(
        None,
        Arc::new(move |args: &[String]| {
            if args[0] == "GET" && args.get(1).is_some_and(|k| k.contains("/challenges/")) {
                l.fetch_add(1, Ordering::SeqCst);
            }
            inner(args)
        }),
    )
}

fn obtained(acme: &Acme, name: &str) -> akio::tls::CertifiedKey {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(k) = acme.certificate(name) {
            return k;
        }
        assert!(
            Instant::now() < deadline,
            "no certificate for {name}: {:?}",
            acme.last_error(name)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Listeners serving `json`'s config for `acme`.
fn serve(json: &str, acme: &Acme) -> (Arc<akio::Engine>, Listeners) {
    let config = akit_config::from_json(json, None).unwrap();
    let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
    let graph = compile(
        &config,
        &registry,
        &Env::default().with_acme(Some(acme.clone())),
    )
    .unwrap();
    let cell = GraphCell::new(graph);
    let engine = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
    engine.start().unwrap();
    let l = Listeners::new(Arc::clone(&engine), cell, &options());
    (engine, l)
}

fn run(challenge: Challenge) {
    let d = dir(match challenge {
        Challenge::Http01 => "acme-shared-http",
        Challenge::TlsAlpn01 => "acme-shared-alpn",
    });
    let (http_port, tls_port) = (free_port(), free_port());
    let Some(pebble) = Pebble::start_with(&d, http_port, tls_port, 7_776_000) else {
        return;
    };
    let lookups = Arc::new(AtomicUsize::new(0));
    let r = redis(&lookups);
    let issued = Arc::new(AtomicUsize::new(0));
    let (a, b) = (
        manager(&pebble, &r, challenge, &issued),
        manager(&pebble, &r, challenge, &issued),
    );
    // Only the second listens where the CA checks; the first has no listener at all.
    let json = match challenge {
        Challenge::Http01 => format!(
            r#"{{"listeners": [{{"name": "plain", "address": "127.0.0.1:{http_port}", "protocols": ["http/1.1"]}}],
                "routes": [{{"handler": {{"module": "respond", "params": {{"body": "routes"}}}}}}]}}"#
        ),
        Challenge::TlsAlpn01 => format!(
            r#"{{"listeners": [{{"name": "secure", "address": "127.0.0.1:{tls_port}", "protocols": ["http/1.1"],
                   "tls": {{"acme": ["{NAME}"]}}}}],
                "routes": [{{"handler": {{"module": "respond", "params": {{"body": "routes"}}}}}}]}}"#
        ),
    };
    let (engine, listeners) = serve(&json, &b);

    a.manage(&[NAME]);
    let first = obtained(&a, NAME);
    assert!(
        lookups.load(Ordering::SeqCst) > 0,
        "the CA's check was not answered from the shared storage"
    );
    b.manage(&[NAME]);
    let second = obtained(&b, NAME);
    assert_eq!(
        first.chain_der().next(),
        second.chain_der().next(),
        "the second obtained a certificate of its own"
    );
    assert_eq!(issued.load(Ordering::SeqCst), 1, "certificates obtained");
    // The challenge's answer is gone from the storage.
    let st = SharedStore::new(&storage(&r)).unwrap();
    let left = st
        .list("challenges", Instant::now() + Duration::from_secs(5))
        .unwrap();
    assert!(left.is_empty(), "challenges left in the storage: {left:?}");
    st.close();

    drop(listeners);
    assert!(akit_listener::shutdown(&engine, Duration::from_secs(5)));
    a.close();
    b.close();
    drop(pebble);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn two_instances_share_one_certificate_through_http_01() {
    run(Challenge::Http01);
}

#[test]
fn two_instances_share_one_certificate_through_tls_alpn_01() {
    run(Challenge::TlsAlpn01);
}

/// With a shared storage and no challenge in progress, a request for a challenge in the clear is
/// answered 404 at once, not routed, as fit's; with no Pebble needed.
#[test]
fn a_challenge_nobody_has_is_404() {
    let lookups = Arc::new(AtomicUsize::new(0));
    let r = redis(&lookups);
    let a = Acme::new(&akit_config::Acme {
        directory: "https://127.0.0.1:1/dir".into(),
        shared_storage: Some(storage(&r)),
        ..akit_config::Acme::default()
    })
    .unwrap();
    let (engine, listeners) = serve(
        r#"{"listeners": [{"name": "plain", "address": "127.0.0.1:0", "protocols": ["http/1.1"]}],
            "routes": [{"handler": {"module": "respond", "params": {"body": "routes"}}}]}"#,
        &a,
    );
    let addr = listeners.served()[0].address;
    let get = |path: &str| {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        write!(
            s,
            "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    };
    let r404 = get("/.well-known/acme-challenge/tok");
    assert!(r404.starts_with("HTTP/1.1 404"), "{r404}");
    assert!(get("/other").ends_with("routes"));
    // Another instance's challenge, once it marks one in progress.
    let st = SharedStore::new(&storage(&r)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let until = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        + 120_000_000_000;
    let ca = "127.0.0.1-1-dir";
    st.store(
        &format!("acme/{ca}/challenges-in-progress"),
        until.to_string().as_bytes(),
        deadline,
    )
    .unwrap();
    st.store(
        &format!("challenges/{ca}/http-01/tok"),
        b"tok.thumb",
        deadline,
    )
    .unwrap();
    // Within a second the marker is read again.
    let end = Instant::now() + Duration::from_secs(10);
    let seen = Mutex::new(String::new());
    loop {
        let r = get("/.well-known/acme-challenge/tok");
        if r.starts_with("HTTP/1.1 200") {
            assert!(r.ends_with("tok.thumb"), "{r}");
            break;
        }
        *seen.lock().unwrap() = r;
        assert!(Instant::now() < end, "{}", seen.lock().unwrap());
        std::thread::sleep(Duration::from_millis(200));
    }
    st.close();
    drop(listeners);
    assert!(akit_listener::shutdown(&engine, Duration::from_secs(5)));
    a.close();
}
