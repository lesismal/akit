//! An OCSP responder for a test CA, whose answers a test sets: fit's `tlsmgr` test responder.
//! A thread per connection, in the clear, one request each.
#![allow(
    dead_code,
    unreachable_pub,
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use akio_x509::{Certificate, OcspResponseParams, RevocationReason, RevocationStatus};

use super::certs::Ca;

/// What the responder answers.
pub struct Settings {
    /// By serial number; good when not set.
    pub revoked: Vec<Vec<u8>>,
    /// Of a response, in seconds; none given when 0.
    pub validity: i64,
    /// Answer 500.
    pub fail: bool,
    /// Requests wait while it is set.
    pub hold: bool,
}

pub struct Responder {
    pub ca: Arc<Ca>,
    /// Its URL.
    pub url: String,
    leaves: Arc<Mutex<HashMap<Vec<u8>, Certificate>>>,
    settings: Arc<(Mutex<Settings>, Condvar)>,
    pub requests: Arc<AtomicUsize>,
    pub in_flight: Arc<AtomicUsize>,
    pub most_in_flight: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl Drop for Responder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.release();
    }
}

fn now() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap()
}

impl Responder {
    pub fn start() -> Responder {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/ocsp", l.local_addr().unwrap());
        let r = Responder {
            ca: Arc::new(Ca::new("akit test CA")),
            url,
            leaves: Arc::default(),
            settings: Arc::new((
                Mutex::new(Settings {
                    revoked: Vec::new(),
                    validity: 3600,
                    fail: false,
                    hold: false,
                }),
                Condvar::new(),
            )),
            requests: Arc::default(),
            in_flight: Arc::default(),
            most_in_flight: Arc::default(),
            stop: Arc::default(),
        };
        let (ca, leaves, settings) = (r.ca.clone(), r.leaves.clone(), r.settings.clone());
        let (requests, in_flight, most) = (
            r.requests.clone(),
            r.in_flight.clone(),
            r.most_in_flight.clone(),
        );
        let stop = r.stop.clone();
        l.set_nonblocking(true).unwrap();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                match l.accept() {
                    Ok((c, _)) => {
                        let (ca, leaves, settings) = (ca.clone(), leaves.clone(), settings.clone());
                        let (requests, in_flight, most, stop) = (
                            requests.clone(),
                            in_flight.clone(),
                            most.clone(),
                            stop.clone(),
                        );
                        std::thread::spawn(move || {
                            let n = in_flight.fetch_add(1, Ordering::AcqRel) + 1;
                            most.fetch_max(n, Ordering::AcqRel);
                            requests.fetch_add(1, Ordering::AcqRel);
                            serve(c, &ca, &leaves, &settings, &stop);
                            in_flight.fetch_sub(1, Ordering::AcqRel);
                        });
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        r
    }

    /// Has it answer about `leaf`, which its CA issued.
    pub fn know(&self, leaf: &Certificate) {
        self.leaves
            .lock()
            .unwrap()
            .insert(leaf.serial().to_vec(), leaf.clone());
    }

    pub fn set(&self, f: impl FnOnce(&mut Settings)) {
        f(&mut self.settings.0.lock().unwrap());
        self.settings.1.notify_all();
    }

    /// Lets held requests go.
    pub fn release(&self) {
        self.set(|s| s.hold = false);
    }
}

fn serve(
    mut c: TcpStream,
    ca: &Ca,
    leaves: &Mutex<HashMap<Vec<u8>, Certificate>>,
    settings: &(Mutex<Settings>, Condvar),
    stop: &AtomicBool,
) {
    c.set_nonblocking(false).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let (head_end, len) = loop {
        let Ok(n) = c.read(&mut chunk) else { return };
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..i]).to_ascii_lowercase();
            let len = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .map_or(0, |v| v.trim().parse::<usize>().unwrap());
            break (i + 4, len);
        }
    };
    while buf.len() < head_end + len {
        let Ok(n) = c.read(&mut chunk) else { return };
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = &buf[head_end..head_end + len];
    let (fail, validity, revoked) = {
        let mut s = settings.0.lock().unwrap();
        while s.hold && !stop.load(Ordering::Acquire) {
            s = settings
                .1
                .wait_timeout(s, Duration::from_millis(50))
                .unwrap()
                .0;
        }
        (s.fail, s.validity, s.revoked.clone())
    };
    if stop.load(Ordering::Acquire) {
        return;
    }
    // The leaf whose serial number the request holds.
    let leaf = leaves.lock().unwrap().iter().find_map(|(serial, leaf)| {
        let mut int = vec![0x02, u8::try_from(serial.len()).unwrap()];
        int.extend_from_slice(serial);
        body.windows(int.len())
            .any(|w| w == int)
            .then(|| leaf.clone())
    });
    let (status, out) = match (fail, leaf) {
        (true, _) => ("500 Internal Server Error", b"down".to_vec()),
        (false, None) => ("404 Not Found", b"unknown".to_vec()),
        (false, Some(leaf)) => {
            let t = now();
            let mut p = OcspResponseParams::new(if revoked.contains(&leaf.serial().to_vec()) {
                RevocationStatus::Revoked {
                    time: t - 60,
                    reason: Some(RevocationReason::KeyCompromise),
                }
            } else {
                RevocationStatus::Good
            });
            p.this_update = t;
            p.next_update = (validity > 0).then_some(t + validity);
            (
                "200 OK",
                p.sign(&leaf, &ca.cert, &ca.cert, ca.key()).unwrap(),
            )
        }
    };
    let head = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/ocsp-response\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        out.len()
    );
    let _ = c.write_all(head.as_bytes());
    let _ = c.write_all(&out);
}
