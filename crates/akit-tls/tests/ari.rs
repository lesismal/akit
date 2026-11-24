//! ACME Renewal Information (I32), fit's `tlsmgr/ari_test.go`: a CA whose suggested window has
//! begun has a certificate its lifetime says is far from due renewed at once, served meanwhile;
//! one whose window is to come has nothing renewed before it, and is asked again when its
//! `Retry-After` says. The CA serves a directory and ARI in the clear, and fails every other
//! request, so that a renewal shows as an order that fails.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod support;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use akio_x509::{Certificate, CertificateParams};
use akit_tls::{Acme, FileStorage, Storage};
use support::certs::{Ca, dir};

fn now() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap()
}

/// Unix seconds as RFC 3339.
fn rfc3339(t: i64) -> String {
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

#[derive(Default)]
struct Fake {
    window: Mutex<(i64, i64)>,
    retry_after: Mutex<Option<String>>,
    /// The certificate identifiers asked about.
    asked: Mutex<Vec<String>>,
    /// Requests for anything but the directory and ARI: a renewal started.
    others: AtomicUsize,
    stop: AtomicBool,
}

/// A CA in the clear; its directory's URL.
fn fake_ca(f: &Arc<Fake>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    l.set_nonblocking(true).unwrap();
    let (f, b) = (Arc::clone(f), base.clone());
    std::thread::spawn(move || {
        while !f.stop.load(Ordering::Acquire) {
            let Ok((mut c, _)) = l.accept() else {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            };
            c.set_nonblocking(false).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                match c.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            }
            let head = String::from_utf8_lossy(&buf).into_owned();
            let path = head.split_whitespace().nth(1).unwrap_or("").to_owned();
            let (status, extra, body) = if path == "/dir" {
                let body = format!(
                    r#"{{"newNonce": "{b}/nonce", "newAccount": "{b}/account", "newOrder": "{b}/order", "renewalInfo": "{b}/ari/"}}"#
                );
                ("200 OK", String::new(), body)
            } else if let Some(id) = path.strip_prefix("/ari/") {
                f.asked.lock().unwrap().push(id.to_owned());
                let (start, end) = *f.window.lock().unwrap();
                let extra = f
                    .retry_after
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|r| format!("retry-after: {r}\r\n"))
                    .unwrap_or_default();
                let body = format!(
                    r#"{{"suggestedWindow": {{"start": "{}", "end": "{}"}}}}"#,
                    rfc3339(start),
                    rfc3339(end)
                );
                ("200 OK", extra, body)
            } else {
                f.others.fetch_add(1, Ordering::AcqRel);
                (
                    "500 Internal Server Error",
                    String::new(),
                    "not here".into(),
                )
            };
            let _ = write!(
                c,
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    format!("{base}/dir")
}

/// A certificate for `name` a test CA issued, with the key identifier ARI names it by, stored
/// as the manager stores what it obtains.
fn store_issued(storage: &FileStorage, directory: &str, name: &str) -> Certificate {
    let ca = Ca::new("akit test CA");
    let key =
        akio_crypto::pkey::PrivateKey::P256(akio_crypto::p256::SecretKey::generate().unwrap());
    let leaf = ca.sign(&CertificateParams::new([name]), &key);
    assert!(leaf.authority_key_id().is_some());
    let pem = format!(
        "{}{}",
        leaf.to_pem(),
        akio_crypto::pem::encode("PRIVATE KEY", &key.to_pkcs8_der())
    );
    // The storage's key: the directory's host and path as fit's caKey makes them.
    let ca_key: String = directory
        .trim_start_matches("http://")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    storage
        .store(
            &format!("certificates/{ca_key}/{name}.pem"),
            pem.as_bytes(),
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
    leaf
}

fn manager(directory: &str, root: &std::path::Path) -> (Acme, FileStorage) {
    let a = Acme::new(&akit_config::Acme {
        directory: directory.to_owned(),
        storage: root.display().to_string(),
        timeout: Duration::from_secs(5),
        request_timeout: Duration::from_secs(5),
        ..akit_config::Acme::default()
    })
    .unwrap();
    a.set_log(Arc::new(|l: &str| eprintln!("acme: {l}")));
    a.set_backoff(Duration::from_secs(60), Duration::from_secs(60));
    (a, FileStorage::new(root))
}

fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < deadline, "no {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// fit's `TestARIWindowBegun`.
#[test]
fn a_window_begun_renews_at_once() {
    let d = dir("ari-begun");
    let fake = Arc::new(Fake::default());
    *fake.window.lock().unwrap() = (now() - 3600, now() + 3600);
    let directory = fake_ca(&fake);
    let (a, storage) = manager(&directory, &d);
    let leaf = store_issued(&storage, &directory, "ari.akit.test");
    a.manage(&["ari.akit.test"]);
    eventually("renewal started", || {
        fake.others.load(Ordering::Acquire) > 0
    });
    let served = a
        .certificate("ari.akit.test")
        .expect("the stored one served meanwhile");
    assert_eq!(served.chain_der().next().unwrap(), leaf.as_der());
    let asked = fake.asked.lock().unwrap().clone();
    let id = asked.first().expect("not asked");
    let b64 = |b: &[u8]| akio_crypto::base64::URL_SAFE_NO_PAD.encode(b);
    let want = format!(
        "{}.{}",
        b64(leaf.authority_key_id().unwrap()),
        b64(leaf.serial())
    );
    assert_eq!(id, &want);
    a.close();
    fake.stop.store(true, Ordering::Release);
    std::fs::remove_dir_all(&d).unwrap();
}

/// fit's `TestARIWindowLater`.
#[test]
fn a_window_to_come_is_waited_for_and_asked_again() {
    let d = dir("ari-later");
    let fake = Arc::new(Fake::default());
    *fake.window.lock().unwrap() = (now() + 24 * 3600, now() + 48 * 3600);
    *fake.retry_after.lock().unwrap() = Some("1".into());
    let directory = fake_ca(&fake);
    let (a, storage) = manager(&directory, &d);
    a.set_ari_bounds(Duration::from_millis(100), Duration::from_secs(24 * 3600));
    store_issued(&storage, &directory, "later.akit.test");
    a.manage(&["later.akit.test"]);
    eventually("asked again", || fake.asked.lock().unwrap().len() >= 3);
    assert_eq!(
        fake.others.load(Ordering::Acquire),
        0,
        "renewed before the window"
    );
    // Picked in the window.
    let secs =
        |t: SystemTime| i64::try_from(t.duration_since(UNIX_EPOCH).unwrap().as_secs()).unwrap();
    let at = secs(a.renewal_info("later.akit.test").expect("no time picked"));
    assert!(at >= now() + 23 * 3600 && at <= now() + 48 * 3600, "{at}");
    // Due when it is next asked, a second from now at most.
    assert!(secs(a.renew_at("later.akit.test").unwrap()) <= now() + 2);
    a.close();
    fake.stop.store(true, Ordering::Release);
    std::fs::remove_dir_all(&d).unwrap();
}
