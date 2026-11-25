//! Shared challenges, as fit's `tlsmgr/shared_test.go`: lookups of HTTP-01 answers and
//! TLS-ALPN-01 certificates in a shared storage, bounded in all and per client; the marker of
//! challenges in progress that gates them; and a listener that answers another instance's
//! TLS-ALPN-01 challenge, what is in memory first.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use akio::tls::CertifiedKey;

use super::{GATE_FRESH, MAX_CLIENT_LOOKUPS, MAX_MISSES, MAX_SHARED_LOOKUPS, MISS_FOR, nanos};
use crate::acme::{Acme, after, alpn_cert_pem};
use crate::storage::{FileStorage, Storage, StorageError};

/// A [`FileStorage`] that says it is shared between hosts, as a shared store does, whose loads
/// may be held and whose stores may fail (fit's `sharedStorage` and `faultyStorage`).
#[derive(Debug)]
struct SharedFiles {
    files: FileStorage,
    held: Mutex<bool>,
    released: Condvar,
    /// Loads of challenges, and of the marker.
    loads: AtomicUsize,
    marker_loads: AtomicUsize,
    fail_stores: AtomicBool,
}

impl SharedFiles {
    fn new(name: &str) -> Arc<SharedFiles> {
        let root =
            std::env::temp_dir().join(format!("akit-acme-shared-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Arc::new(SharedFiles {
            files: FileStorage::new(root),
            held: Mutex::new(false),
            released: Condvar::new(),
            loads: AtomicUsize::new(0),
            marker_loads: AtomicUsize::new(0),
            fail_stores: AtomicBool::new(false),
        })
    }

    fn hold(&self, on: bool) {
        *self.held.lock().unwrap() = on;
        self.released.notify_all();
    }
}

impl Drop for SharedFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.files.root());
    }
}

impl Storage for SharedFiles {
    fn load(&self, key: &str, deadline: Instant) -> Result<Vec<u8>, StorageError> {
        if key.ends_with("/challenges-in-progress") {
            self.marker_loads.fetch_add(1, Ordering::SeqCst);
        } else {
            self.loads.fetch_add(1, Ordering::SeqCst);
        }
        let mut held = self.held.lock().unwrap();
        while *held {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(StorageError::Timeout);
            }
            held = self.released.wait_timeout(held, left).unwrap().0;
        }
        drop(held);
        self.files.load(key, deadline)
    }
    fn store(&self, key: &str, value: &[u8], deadline: Instant) -> Result<(), StorageError> {
        if self.fail_stores.load(Ordering::SeqCst) {
            return Err(StorageError::Io(std::io::Error::other("fault")));
        }
        self.files.store(key, value, deadline)
    }
    fn delete(&self, key: &str, deadline: Instant) -> Result<(), StorageError> {
        self.files.delete(key, deadline)
    }
    fn list(&self, prefix: &str, deadline: Instant) -> Result<Vec<String>, StorageError> {
        self.files.list(prefix, deadline)
    }
    fn lock(&self, name: &str, deadline: Instant) -> Result<(), StorageError> {
        self.files.lock(name, deadline)
    }
    fn unlock(&self, name: &str) -> Result<(), StorageError> {
        self.files.unlock(name)
    }
    fn shared(&self) -> bool {
        true
    }
}

/// What a manager logged.
#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<String>>>);

impl Lines {
    fn count(&self, s: &str) -> usize {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.contains(s))
            .count()
    }
}

fn shared_acme(st: &Arc<SharedFiles>, lines: &Lines) -> Acme {
    let a = Acme::with_storage(
        &akit_config::Acme {
            directory: "https://127.0.0.1:1/dir".into(),
            ..akit_config::Acme::default()
        },
        Arc::clone(st) as Arc<dyn Storage>,
    )
    .unwrap();
    let l = lines.clone();
    a.set_log(Arc::new(move |line: &str| {
        l.0.lock().unwrap().push(line.to_owned());
    }));
    a
}

fn later() -> Instant {
    after(Duration::from_secs(10))
}

/// Calls `lookup_http_challenge` for `client` and waits for its answer.
fn lookup_from(a: &Acme, token: &str, client: Option<IpAddr>) -> Option<String> {
    let (tx, rx) = mpsc::sync_channel(1);
    a.lookup_http_challenge(token, client, Box::new(move |r| tx.send(r).unwrap()));
    rx.recv_timeout(Duration::from_secs(10)).expect("no answer")
}

fn lookup(a: &Acme, token: &str) -> Option<String> {
    lookup_from(a, token, Some("192.0.2.1".parse().unwrap()))
}

fn lookup_alpn(a: &Acme, name: &str) -> Option<CertifiedKey> {
    let (tx, rx) = mpsc::sync_channel(1);
    a.lookup_alpn_challenge(name, Box::new(move |r| tx.send(r).unwrap()));
    rx.recv_timeout(Duration::from_secs(10)).expect("no answer")
}

/// Waits for `f`, at most five seconds.
fn eventually(what: &str, f: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !f() {
        assert!(Instant::now() < end, "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Moves the manager's clock `d` on.
fn advance(a: &Acme, d: Duration) {
    a.inner.gate.offset.fetch_add(nanos(d), Ordering::SeqCst);
}

fn lookups(a: &Acme) -> usize {
    a.inner.gate.lookups.load(Ordering::SeqCst)
}

/// fit's `TestLookupHTTPChallenge`.
#[test]
fn http_challenges_looked_up() {
    // Without a shared storage, nothing is looked up.
    let plain = Acme::with_storage(
        &akit_config::Acme {
            directory: "https://127.0.0.1:1/dir".into(),
            ..akit_config::Acme::default()
        },
        Arc::new(FileStorage::new(
            std::env::temp_dir().join("akit-tls-unused"),
        )),
    )
    .unwrap();
    assert!(!plain.shared());
    assert_eq!(lookup(&plain, "token"), None);
    plain.close();

    let lines = Lines::default();
    let st = SharedFiles::new("http");
    let a = shared_acme(&st, &lines);
    assert!(a.shared());
    a.write_marker(later());
    st.store(
        &a.http_challenge_key("tok_en-1"),
        b"tok_en-1.thumb",
        later(),
    )
    .unwrap();
    assert_eq!(lookup(&a, "tok_en-1").as_deref(), Some("tok_en-1.thumb"));
    for bad in ["none", "../../certificates/x", "a/b", "", &"a".repeat(257)] {
        assert_eq!(lookup(&a, bad), None, "{bad:?}");
    }

    // Past MAX_SHARED_LOOKUPS under way, a lookup is answered at once, as one with no challenge.
    st.hold(true);
    let (tx, rx) = mpsc::channel();
    for _ in 0..MAX_SHARED_LOOKUPS {
        // No address, so no client's bound.
        let tx = tx.clone();
        a.lookup_http_challenge("tok_en-1", None, Box::new(move |r| tx.send(r).unwrap()));
    }
    eventually("every slot taken", || lookups(&a) == MAX_SHARED_LOOKUPS);
    let called = Arc::new(AtomicBool::new(false));
    let c = Arc::clone(&called);
    a.lookup_http_challenge(
        "tok_en-1",
        None,
        Box::new(move |r| c.store(r.is_none(), Ordering::SeqCst)),
    );
    assert!(
        called.load(Ordering::SeqCst),
        "a lookup past the bound waited"
    );
    // One from an address gives its client's slot back.
    called.store(false, Ordering::SeqCst);
    let ip: IpAddr = "192.0.2.7".parse().unwrap();
    let c = Arc::clone(&called);
    a.lookup_http_challenge(
        "tok_en-1",
        Some(ip),
        Box::new(move |r| c.store(r.is_none(), Ordering::SeqCst)),
    );
    assert!(called.load(Ordering::SeqCst));
    assert_eq!(
        a.inner.gate.clients[a.client_slot(ip)].load(Ordering::SeqCst),
        0
    );
    st.hold(false);
    for _ in 0..MAX_SHARED_LOOKUPS {
        let r = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(
            r.as_deref(),
            Some("tok_en-1.thumb"),
            "a held lookup found nothing"
        );
    }
    eventually("the slots back", || lookups(&a) == 0);
    a.close();
}

/// fit's `TestCertsSharedALPN`: a listener answers another instance's TLS-ALPN-01 challenge
/// from the storage, for a name it lists; what is in memory first.
#[test]
fn alpn_challenges_from_the_storage() {
    use crate::certs::Hello;
    use crate::{ListenerTls, Pick, Protocol, Tls};

    let lines = Lines::default();
    let st = SharedFiles::new("alpn");
    let a = shared_acme(&st, &lines);
    a.write_marker(later());
    let tls = Tls {
        certificates: Vec::new(),
        client_auth: None,
        max_version: akit_config::TlsVersion::Tls13,
        acme: vec!["a.akit.test".into(), "b.akit.test".into()],
        on_demand: false,
    };
    let t = ListenerTls::load_with(&tls, &[Protocol::Http11], Some(&a)).unwrap();
    let acme_only: &[&[u8]] = &[crate::ACME_TLS_ALPN.as_bytes()];
    let pick = |name: &str| {
        t.pick(&Hello {
            name: Some(name),
            schemes: &[0x0403],
            suites: &[],
            tls13: true,
            alpn: acme_only,
        })
    };
    // No certificate in memory: looked up, which finds none yet.
    assert!(matches!(pick("A.akit.test."), Pick::SharedAlpn(n) if n == "a.akit.test"));
    assert!(lookup_alpn(&a, "a.akit.test").is_none());
    let (challenge, pem) = alpn_cert_pem("a.akit.test", "tok.thumb").unwrap();
    for name in ["a.akit.test", "c.akit.test"] {
        st.store(&a.alpn_challenge_key(name), pem.as_bytes(), later())
            .unwrap();
    }
    // A name looked up in vain is not looked up again for MISS_FOR, which the instance with the
    // challenge waits out before the CA checks it.
    assert!(lookup_alpn(&a, "a.akit.test").is_none());
    advance(&a, MISS_FOR);
    let leaf = |k: &CertifiedKey| k.chain_der().next().map(<[u8]>::to_vec);
    let got = lookup_alpn(&a, "a.akit.test").expect("another instance's challenge");
    assert_eq!(leaf(&got), leaf(&challenge));
    // Not for a name the listener does not list.
    assert!(matches!(pick("c.akit.test"), Pick::Key(None)));
    // What is in memory comes first.
    let (mine, _) = alpn_cert_pem("a.akit.test", "mine").unwrap();
    a.set_alpn("a.akit.test", Some(mine.clone()));
    assert!(matches!(pick("a.akit.test"), Pick::Key(Some(k)) if leaf(&k) == leaf(&mine)));
    // A name that would make another key is not looked up.
    assert!(lookup_alpn(&a, "a.akit.test/../x").is_none());
    a.close();
}

/// fit's `TestSharedGate`: flooded, while no instance has a challenge in progress, with
/// handshakes offering acme-tls/1 alone and requests for HTTP-01 tokens, the storage is asked
/// for none of them, only for the marker, at most once each GATE_FRESH. Once another instance
/// marks a challenge in progress, it is looked up within GATE_FRESH.
#[test]
fn the_gate_keeps_lookups_off_the_storage() {
    let lines = Lines::default();
    let st = SharedFiles::new("gate");
    let (a, b) = (shared_acme(&st, &lines), shared_acme(&st, &lines));
    for i in 0..1000 {
        assert!(lookup_alpn(&a, &format!("r{i}.akit.test")).is_none());
        let client: IpAddr = format!("198.51.100.{}", i % 250).parse().unwrap();
        assert_eq!(lookup_from(&a, &format!("tok{i}"), Some(client)), None);
    }
    let (n, m) = (
        st.loads.load(Ordering::SeqCst),
        st.marker_loads.load(Ordering::SeqCst),
    );
    assert!(n == 0 && m <= 2, "{n} challenges and {m} markers loaded");

    // b marks a challenge in progress and keeps its answer: a, which read the marker lately,
    // finds it once GATE_FRESH has passed.
    let marker = b.mark_in_progress(later());
    st.store(&a.http_challenge_key("tok-b"), b"tok-b.thumb", later())
        .unwrap();
    assert_eq!(
        lookup(&a, "tok-b"),
        None,
        "the marker read again within GATE_FRESH"
    );
    advance(&a, GATE_FRESH);
    assert_eq!(lookup(&a, "tok-b").as_deref(), Some("tok-b.thumb"));
    // While it is, every lookup asks the storage, within the bounds.
    let before = st.marker_loads.load(Ordering::SeqCst);
    for _ in 0..10 {
        lookup(&a, "tok-none");
    }
    assert_eq!(
        st.marker_loads.load(Ordering::SeqCst),
        before,
        "a good marker read again"
    );
    drop(marker);
    // A marker that does not parse is as none.
    st.store(&b.marker_key(), b"soon", later()).unwrap();
    let fresh = shared_acme(&st, &lines);
    assert_eq!(lookup(&fresh, "tok-b"), None);
    for x in [a, b, fresh] {
        x.close();
    }
}

/// fit's `TestSharedALPNBound`: past MAX_SHARED_LOOKUPS, a handshake gets no certificate at
/// once; the names looked up in vain are not looked up again within MISS_FOR, and the misses
/// remembered are bounded.
#[test]
fn alpn_lookups_bounded() {
    let lines = Lines::default();
    let st = SharedFiles::new("alpn-bound");
    let a = shared_acme(&st, &lines);
    a.write_marker(later());
    st.hold(true);
    let (tx, rx) = mpsc::channel();
    for i in 0..MAX_SHARED_LOOKUPS {
        let tx = tx.clone();
        a.lookup_alpn_challenge(
            &format!("h{i}.akit.test"),
            Box::new(move |r| tx.send(r.is_some()).unwrap()),
        );
    }
    eventually("every slot taken", || lookups(&a) == MAX_SHARED_LOOKUPS);
    let answered = Arc::new(AtomicBool::new(false));
    let c = Arc::clone(&answered);
    a.lookup_alpn_challenge(
        "past.akit.test",
        Box::new(move |r| c.store(r.is_none(), Ordering::SeqCst)),
    );
    assert!(
        answered.load(Ordering::SeqCst),
        "a handshake past the bound waited"
    );
    st.hold(false);
    for _ in 0..MAX_SHARED_LOOKUPS {
        assert!(!rx.recv_timeout(Duration::from_secs(10)).unwrap());
    }
    let before = st.loads.load(Ordering::SeqCst);
    for i in 0..MAX_SHARED_LOOKUPS {
        assert!(lookup_alpn(&a, &format!("h{i}.akit.test")).is_none());
    }
    assert_eq!(
        st.loads.load(Ordering::SeqCst),
        before,
        "looked up again at once"
    );
    for i in 0..MAX_MISSES + 10 {
        a.miss(&format!("m{i}.akit.test"), 0);
    }
    assert!(a.inner.gate.misses.lock().unwrap().len() <= MAX_MISSES);
    a.close();
}

/// fit's `TestSharedLookupPerClient`: one client past MAX_CLIENT_LOOKUPS gets its 404 at once,
/// and the slots left are another client's, the CA's among them.
#[test]
fn lookups_bounded_per_client() {
    let lines = Lines::default();
    let st = SharedFiles::new("per-client");
    let a = shared_acme(&st, &lines);
    a.write_marker(later());
    st.store(&a.http_challenge_key("real"), b"real.thumb", later())
        .unwrap();
    st.hold(true);
    let (tx, rx) = mpsc::channel();
    for i in 0..MAX_CLIENT_LOOKUPS {
        // One host: the addresses of one IPv6 /64 count as one.
        let client: IpAddr = format!("2001:db8::{}", i + 1).parse().unwrap();
        let tx = tx.clone();
        a.lookup_http_challenge(
            "flood",
            Some(client),
            Box::new(move |r| tx.send(r).unwrap()),
        );
    }
    eventually("the client's slots taken", || {
        lookups(&a) == MAX_CLIENT_LOOKUPS
    });
    let refused = Arc::new(AtomicBool::new(false));
    let c = Arc::clone(&refused);
    a.lookup_http_challenge(
        "flood",
        Some("2001:db8::99".parse().unwrap()),
        Box::new(move |r| c.store(r.is_none(), Ordering::SeqCst)),
    );
    assert!(refused.load(Ordering::SeqCst));
    assert_eq!(lookups(&a), MAX_CLIENT_LOOKUPS);
    // Another client, whose counter is not the flooding one's.
    let flood = a.client_slot("2001:db8::1".parse().unwrap());
    let other = (0..=255)
        .map(|i| format!("203.0.113.{i}").parse::<IpAddr>().unwrap())
        .find(|c| a.client_slot(*c) != flood)
        .unwrap();
    let (ctx, crx) = mpsc::sync_channel(1);
    a.lookup_http_challenge("real", Some(other), Box::new(move |r| ctx.send(r).unwrap()));
    st.hold(false);
    assert_eq!(
        crx.recv_timeout(Duration::from_secs(10))
            .unwrap()
            .as_deref(),
        Some("real.thumb")
    );
    for _ in 0..MAX_CLIENT_LOOKUPS {
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
    }
    // An IPv4 client mapped into IPv6 is the same client.
    assert_eq!(
        a.client_slot("::ffff:192.0.2.7".parse().unwrap()),
        a.client_slot("192.0.2.7".parse().unwrap())
    );
    eventually("the counters back to zero", || {
        lookups(&a) == 0
            && a.inner
                .gate
                .clients
                .iter()
                .all(|c| c.load(Ordering::SeqCst) == 0)
    });
    a.close();
}

/// fit's `TestMarkInProgress`: the marker is kept while a challenge is in progress; a failure to
/// write it is reported.
#[test]
fn the_marker_is_kept_and_its_failures_reported() {
    let lines = Lines::default();
    let st = SharedFiles::new("marker");
    let a = shared_acme(&st, &lines);
    let m = a.mark_in_progress(later());
    let data = st.files.load(&a.marker_key(), later()).unwrap();
    let until: i64 = std::str::from_utf8(&data).unwrap().parse().unwrap();
    assert!(until > a.now_nanos());
    drop(m);
    // Its deadline passed, the refresher ends too (and the marker is not written).
    drop(a.mark_in_progress(Instant::now()));
    st.fail_stores.store(true, Ordering::SeqCst);
    a.write_marker(later());
    let failed = "keeping the marker of challenges in progress in the shared storage: akit-tls: storage: fault";
    assert_eq!(lines.count(failed), 1, "{:?}", lines.0.lock().unwrap());
    a.close();
}

/// fit's `TestReadGateWhileReading`: while one read of the marker is under way, another reads
/// nothing and takes a challenge to be in progress.
#[test]
fn the_gate_read_once_at_a_time() {
    let lines = Lines::default();
    let st = SharedFiles::new("reading");
    let a = shared_acme(&st, &lines);
    a.inner.gate.reading.store(true, Ordering::SeqCst);
    assert!(a.read_gate(later()));
    assert_eq!(st.marker_loads.load(Ordering::SeqCst), 0);
    a.inner.gate.reading.store(false, Ordering::SeqCst);
    // With no marker written, a read finds no challenge in progress.
    assert!(!a.read_gate(later()));
    assert_eq!(st.marker_loads.load(Ordering::SeqCst), 1);
    a.close();
}

/// A challenge's certificate as the storage keeps it reads back.
#[test]
fn alpn_certificates_kept_as_pem() {
    let (k, pem) = alpn_cert_pem("a.akit.test", "t.thumb").unwrap();
    let back = super::parse_alpn(pem.as_bytes()).unwrap();
    assert_eq!(back.chain_der().next(), k.chain_der().next());
    assert!(super::parse_alpn(b"not PEM").is_err());
    assert!(super::name_ok("a-1.akit.test"));
    for bad in ["", "A.akit.test", "a/b", "a_b", &"a".repeat(254)] {
        assert!(!super::name_ok(bad), "{bad:?}");
    }
}
