//! The certificate manager (I30): certificates chosen per handshake (wildcards in any of them),
//! and replaced by a reload under traffic without a failed request or a dropped connection. On
//! the backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod support;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use akio::tls::stream::Stream;
use akio::{Backend, UringConfig};
use akit_listener::Listeners;
use akit_runtime::{Env, Graph, GraphCell, HandlerParams, Registry, compile};
use support::certs::{Ca, dir};
use support::https::{client, get_h1, leaf_der};

fn options() -> akit_listener::Options {
    let mut o = akit_listener::Options::default();
    o.backend = match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    };
    o.shutdown_timeout = Duration::from_secs(5);
    o
}

fn graph(json: &str) -> Graph {
    let config = akit_config::from_json(json, None).unwrap();
    let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
    compile(&config, &registry, &Env::default()).unwrap()
}

fn serve(json: &str) -> (GraphCell, Arc<akio::Engine>, Listeners) {
    let cell = GraphCell::new(graph(json));
    let e = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
    e.start().unwrap();
    let l = Listeners::new(Arc::clone(&e), cell.clone(), &options());
    (cell, e, l)
}

fn stop(e: &akio::Engine, l: Listeners) {
    drop(l);
    assert!(akit_listener::shutdown(e, Duration::from_secs(5)));
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

/// One TLS listener with the certificates `certs` and `extra` in its `tls`, answering `/` with
/// "hello".
fn config(certs: &[&(PathBuf, PathBuf)], extra: &str) -> String {
    let certs: Vec<String> = certs
        .iter()
        .map(|(c, k)| format!(r#"{{"cert_file": "{}", "key_file": "{}"}}"#, s(c), s(k)))
        .collect();
    format!(
        r#"{{"listeners": [{{"name": "secure", "address": "127.0.0.1:0", "protocols": ["http/1.1"],
              "tls": {{"certificates": [{}]{extra}}}}}],
            "routes": [{{"handler": {{"module": "respond", "params": {{"body": "hello"}}}}}}]}}"#,
        certs.join(", ")
    )
}

/// A wildcard in a certificate other than the first picks it (akio's resolver, A2): fit's
/// choice, the first certificate for the name in the order given.
#[test]
fn a_wildcard_in_any_certificate_picks_it() {
    let d = dir("certs-wildcard");
    let ca = Ca::new("akit test ca");
    let roots = ca.write(&d, "ca.pem");
    let a = ca.leaf(&d, "a", &["a.example"]);
    let w = ca.leaf(&d, "w", &["*.w.example", "b.example"]);
    let x = ca.leaf(&d, "x", &["x.w.example"]);
    let (_, e, l) = serve(&config(&[&a, &w, &x], ""));
    let addr = l.served()[0].address;
    let c = client(&roots, &["http/1.1"], None);
    for (name, want) in [
        ("a.example", &a),
        ("y.w.example", &w),
        ("B.example", &w),
        // The wildcard came first, and serves the name too.
        ("x.w.example", &w),
    ] {
        let seen = get_h1(addr, &c, name, name, "/").unwrap();
        assert!(seen.leaf == leaf_der(&want.0), "{name}");
        assert_eq!(seen.body, "hello");
    }
    // One label deep only: a deeper name gets the first, which the client refuses.
    assert!(get_h1(addr, &c, "z.y.w.example", "z.y.w.example", "/").is_err());
    stop(&e, l);
    std::fs::remove_dir_all(&d).unwrap();
}

/// An HTTPS/1.1 connection kept open: requests sent on it one at a time.
struct Open(Stream<TcpStream>);

impl Open {
    fn new(addr: SocketAddr, ca: &Path) -> (Open, Vec<u8>) {
        let tcp = TcpStream::connect(addr).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut s = Stream::client(&client(ca, &["http/1.1"], None), "a.example", tcp).unwrap();
        s.handshake().unwrap();
        let leaf = s.conn().peer_certificates()[0].clone();
        (Open(s), leaf)
    }

    /// A GET of `/`; the response's body, "hello".
    fn get(&mut self) -> String {
        write!(self.0, "GET / HTTP/1.1\r\nHost: a.example\r\n\r\n").unwrap();
        self.0.flush().unwrap();
        let mut resp = Vec::new();
        let mut buf = [0u8; 1024];
        while !resp.ends_with(b"hello") {
            let n = self.0.read(&mut buf).unwrap();
            assert!(n > 0, "closed: {:?}", String::from_utf8_lossy(&resp));
            resp.extend_from_slice(&buf[..n]);
        }
        let resp = String::from_utf8(resp).unwrap();
        assert!(resp.starts_with("HTTP/1.1 200 OK"), "{resp}");
        resp.rsplit("\r\n\r\n").next().unwrap().to_owned()
    }
}

/// A certificate replaced while clients make requests: the reload keeps the listener (its
/// socket and its connections), the handshakes after it get the new certificate, and no request
/// fails or connection drops.
#[test]
fn a_certificate_replaced_under_traffic() {
    let d = dir("certs-replaced");
    let ca = Ca::new("akit test ca");
    let roots = ca.write(&d, "ca.pem");
    let a = ca.leaf(&d, "a", &["a.example"]);
    let old = leaf_der(&a.0);
    let json = config(&[&a], "");
    let (cell, e, mut l) = serve(&json);
    let addr = l.served()[0].address;
    // A connection made before the reload, and kept across it.
    let (mut open, leaf) = Open::new(addr, &roots);
    assert_eq!(leaf, old);
    assert_eq!(open.get(), "hello");

    // Clients making new connections, each with a full handshake, all along.
    let stop_clients = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicUsize::new(0));
    let failures = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let clients: Vec<_> = (0..4)
        .map(|_| {
            let (stop_clients, done, failures, seen) = (
                Arc::clone(&stop_clients),
                Arc::clone(&done),
                Arc::clone(&failures),
                Arc::clone(&seen),
            );
            let roots = roots.clone();
            std::thread::spawn(move || {
                while !stop_clients.load(Ordering::SeqCst) {
                    // A client of its own each time: a full handshake, not a resumed one (which
                    // sends no certificate).
                    let c = client(&roots, &["http/1.1"], None);
                    match get_h1(addr, &c, "a.example", "a.example", "/") {
                        Ok(s) if s.body == "hello" => {
                            let mut seen = seen.lock().unwrap();
                            if !seen.contains(&s.leaf) {
                                seen.push(s.leaf);
                            }
                        }
                        Ok(s) => failures.lock().unwrap().push(format!("{s:?}")),
                        Err(e) => failures.lock().unwrap().push(e.to_string()),
                    }
                    done.fetch_add(1, Ordering::SeqCst);
                }
            })
        })
        .collect();
    let wait_for = |n: usize| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while done.load(Ordering::SeqCst) < n {
            assert!(Instant::now() < deadline, "the clients made no progress");
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    wait_for(20);

    // Renewed: the same files, a new key and certificate. The reload keeps the listener.
    let renewed = ca.leaf(&d, "a", &["a.example"]);
    assert_eq!(renewed, a);
    let new = leaf_der(&a.0);
    assert_ne!(new, old);
    let r = l.reload(graph(&json)).unwrap();
    assert_eq!((r.kept, r.added.len(), r.removed.len()), (1, 0, 0));
    assert_eq!(l.served()[0].address, addr);
    let after = done.load(Ordering::SeqCst);
    wait_for(after + 20);
    stop_clients.store(true, Ordering::SeqCst);
    for c in clients {
        c.join().unwrap();
    }
    assert!(
        failures.lock().unwrap().is_empty(),
        "{:?}",
        failures.lock().unwrap()
    );
    assert_eq!(*seen.lock().unwrap(), [old.clone(), new.clone()]);
    // The next handshake gets the new certificate; the connection made before still serves.
    assert_eq!(
        get_h1(
            addr,
            &client(&roots, &[], None),
            "a.example",
            "a.example",
            "/"
        )
        .unwrap()
        .leaf,
        new
    );
    assert_eq!(open.get(), "hello");
    // The old graph is retired: the manager reads the current one.
    cell.collect();
    assert_eq!(cell.retiring(), 0);
    assert_eq!(
        get_h1(
            addr,
            &client(&roots, &[], None),
            "a.example",
            "a.example",
            "/"
        )
        .unwrap()
        .leaf,
        new
    );
    drop(open);
    stop(&e, l);
    std::fs::remove_dir_all(&d).unwrap();
}

/// A configuration holds its client authorities: when their files change, a reload opens the
/// listener anew (beside the old one, which drains), rather than keeping it.
#[test]
fn new_client_authorities_reopen_the_listener() {
    let d = dir("certs-authorities");
    let ca = Ca::new("akit test ca");
    let roots = ca.write(&d, "ca.pem");
    let a = ca.leaf(&d, "a", &["a.example"]);
    let auth = format!(
        r#", "client_auth": {{"ca_files": ["{}"], "mode": "verify_if_given"}}"#,
        s(&roots)
    );
    let json = config(&[&a], &auth);
    let (cell, e, mut l) = serve(&json);
    // The same files: kept.
    let r = l.reload(graph(&json)).unwrap();
    assert_eq!((r.kept, r.added.len(), r.removed.len()), (1, 0, 0));
    // Another authority in the same file: opened anew.
    Ca::new("another ca").write(&d, "ca.pem");
    let r = l.reload(graph(&json)).unwrap();
    assert_eq!((r.kept, r.added.len(), r.removed.len()), (0, 1, 1));
    // The server's certificate still chains to the first CA, which the client trusts.
    let addr = l.served()[0].address;
    let first = d.join("first-ca.pem");
    ca.write(&d, "first-ca.pem");
    let seen = get_h1(
        addr,
        &client(&first, &[], None),
        "a.example",
        "a.example",
        "/",
    )
    .unwrap();
    assert_eq!(seen.body, "hello");
    cell.collect();
    stop(&e, l);
    std::fs::remove_dir_all(&d).unwrap();
}
