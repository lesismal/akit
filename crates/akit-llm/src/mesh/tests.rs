//! fit's `mesh_test.go`, `edge_test.go`, `roles_test.go`, `token_test.go`, `drain_test.go` and
//! `tls_test.go`, ported: roles' meshes linked over akio-ws on one engine.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use akit_config::roles::{LinkToken, llm_peer_roles};
use akit_roles::linktoken::Secret;

use super::*;
use crate::model::{DialReq, ListenConfig, ROLE_HUB, ROLE_NODE, ROLE_PROXY};
use crate::wire::conn::tests::engine;
use crate::wire::{Event, Stream};

const WAIT: Duration = Duration::from_secs(10);

fn until(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn router() -> Router {
    let mut r = Router::new();
    r.handle(
        "/echo",
        &[ROLE_HUB, ROLE_NODE, ROLE_PROXY],
        Duration::ZERO,
        |caller, req, a| {
            a.ok(format!(
                r#"{{"from":{},"req":{}}}"#,
                crate::json::string(&caller.id),
                if req.is_empty() {
                    "null"
                } else {
                    std::str::from_utf8(req).unwrap()
                }
            ));
        },
    );
    r.handle("/hub-only", &[ROLE_HUB], Duration::ZERO, |_, _, a| {
        a.ok("1".into());
    });
    r.handle(
        "/never",
        &[ROLE_HUB, ROLE_NODE],
        Duration::from_millis(200),
        |_, _, a| {
            // Never answered: the method's timeout does.
            std::mem::forget(a);
        },
    );
    r.handle_stream(
        "/count",
        &[ROLE_HUB, ROLE_PROXY, ROLE_NODE],
        |_, req, st| {
            let n: usize = std::str::from_utf8(req).unwrap().parse().unwrap();
            for i in 0..n {
                assert_eq!(st.try_send(format!("{i}").as_bytes()), Ok(true));
            }
            st.end(Ok(b"\"done\"".to_vec()));
        },
    );
    r
}

fn mesh(e: &Arc<Engine>, role: &'static str, id: &str) -> Mesh {
    let id = id.to_owned();
    let m = Mesh::new(
        Config {
            identity: Arc::new(move || Identity {
                role: role.to_owned(),
                id: id.clone(),
                name: id.clone(),
                version: version(),
                ..Identity::default()
            }),
            accept_roles: llm_peer_roles(role)
                .iter()
                .map(|r| (*r).to_owned())
                .collect(),
            engine: Arc::clone(e),
        },
        router(),
    );
    m.set_limits(Limits {
        retry: Duration::from_millis(200),
        ..Limits::default()
    });
    m
}

fn listen_on(m: &Mesh, l: ListenConfig) -> String {
    m.set_listen(&ListenConfig {
        addr: "127.0.0.1:0".to_owned(),
        ..l
    })
    .unwrap();
    m.listen_addr()
}

fn peer(addr: &str, role: &str, password: &str) -> DialReq {
    DialReq {
        addr: addr.to_owned(),
        role: role.to_owned(),
        password: password.to_owned(),
        ..DialReq::default()
    }
}

fn call(m: &Mesh, id: &str, method: &str, req: &str) -> Result<String, Error> {
    let (tx, rx) = mpsc::channel();
    m.call(id, method, req, Duration::ZERO, move |r| {
        let _ = tx.send(r);
    });
    rx.recv_timeout(WAIT)
        .unwrap()
        .map(|b| String::from_utf8(b).unwrap())
}

fn read_stream(st: &Stream) -> (Vec<String>, Result<(), Error>) {
    let (tx, rx) = mpsc::channel();
    let got = Mutex::new(Vec::new());
    let s = st.clone();
    let tx = Mutex::new(Some(tx));
    st.on_data(move |ev| match ev {
        Event::Data(d) => {
            s.release(d.len());
            got.lock().unwrap().push(String::from_utf8(d).unwrap());
        }
        Event::End(r) => {
            if let Some(t) = tx.lock().unwrap().take() {
                let _ = t.send((std::mem::take(&mut *got.lock().unwrap()), r));
            }
        }
    });
    rx.recv_timeout(WAIT).unwrap()
}

#[test]
fn both_directions_calls_streams_and_pushes() {
    let e = engine();
    let hub = mesh(&e, ROLE_HUB, "hub-1");
    let node = mesh(&e, ROLE_NODE, "node-1");
    let (ptx, prx) = mpsc::channel::<(String, String)>();
    let ptx = Mutex::new(ptx);
    hub.on_push("/node/update", &[ROLE_NODE], move |caller, msg| {
        let _ = ptx
            .lock()
            .unwrap()
            .send((caller.id.clone(), String::from_utf8(msg.to_vec()).unwrap()));
    });
    let ups = Arc::new(AtomicUsize::new(0));
    let u = Arc::clone(&ups);
    hub.on_peer_up(move |p| {
        assert_eq!(p.identity.role, ROLE_NODE);
        u.fetch_add(1, Ordering::Relaxed);
    });
    let addr = listen_on(
        &hub,
        ListenConfig {
            node_password: "np".to_owned(),
            ..ListenConfig::default()
        },
    );
    node.dial(&peer(&addr, ROLE_HUB, "np")).unwrap();
    until("the hello", || node.online("hub-1") && hub.online("node-1"));
    assert_eq!(ups.load(Ordering::Relaxed), 1);
    // Both ends call each other on the one link.
    assert_eq!(
        call(&node, "hub-1", "/echo", r#"{"a":1}"#).unwrap(),
        r#"{"from":"node-1","req":{"a":1}}"#
    );
    assert_eq!(
        call(&hub, "node-1", "/echo", "").unwrap(),
        r#"{"from":"hub-1","req":{}}"#
    );
    // A method's roles.
    assert_eq!(call(&hub, "node-1", "/hub-only", "").unwrap(), "1");
    assert_eq!(
        call(&node, "hub-1", "/hub-only", "")
            .unwrap_err()
            .code_msg(),
        (CODE_FORBIDDEN, "node may not call /hub-only".to_owned())
    );
    assert_eq!(
        call(&node, "hub-1", "/nope", "").unwrap_err().code_msg().0,
        crate::model::CODE_NOT_FOUND
    );
    assert_eq!(
        call(&node, "hub-1", "/never", "").unwrap_err().code_msg(),
        (crate::model::CODE_INTERNAL, "timeout".to_owned())
    );
    assert_eq!(call(&node, "nobody", "/echo", "").unwrap_err(), offline());
    // A stream each way.
    let st = node.open_stream("hub-1", "/count", "3").unwrap();
    let (got, r) = read_stream(&st);
    assert_eq!((got, r), (vec!["0".into(), "1".into(), "2".into()], Ok(())));
    assert_eq!(st.result().unwrap(), b"\"done\"");
    let st = hub.open_stream("node-1", "/count", "2").unwrap();
    assert_eq!(read_stream(&st).0.len(), 2);
    // Pushes, in order.
    for i in 0..20 {
        node.push("hub-1", "/node/update", &format!("{i}")).unwrap();
    }
    for i in 0..20 {
        assert_eq!(
            prx.recv_timeout(WAIT).unwrap(),
            ("node-1".into(), format!("{i}"))
        );
    }
    // What the pages show.
    let peers = hub.peers();
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].direction, "in");
    assert!(peers[0].shared_password);
    assert!(peers[0].link.bytes_in > 0);
    let t = node.targets();
    assert!(
        t[0].connected && t[0].peer.as_ref().unwrap().id == "hub-1",
        "{t:?}"
    );
    assert_eq!(hub.peers_of(ROLE_NODE)[0].id, "node-1");
    // A kicked outbound link dials again.
    assert_eq!(hub.kick("node-1"), 1);
    until("the link again", || {
        hub.online("node-1") && node.online("hub-1")
    });
    assert_eq!(ups.load(Ordering::Relaxed), 2);
    node.stop();
    hub.stop();
    e.shutdown();
}

#[test]
fn roles_and_passwords_refused() {
    let e = engine();
    let hub = mesh(&e, ROLE_HUB, "hub-1");
    let addr = listen_on(
        &hub,
        ListenConfig {
            node_password: "np".to_owned(),
            proxy_password: "pp".to_owned(),
            ..ListenConfig::default()
        },
    );
    // A proxy that holds the node's password does not say hello as a proxy.
    let proxy = mesh(&e, ROLE_PROXY, "proxy-1");
    proxy.dial(&peer(&addr, ROLE_HUB, "np")).unwrap();
    until("the refusal", || {
        proxy.targets()[0]
            .error
            .contains("is not the proxy password")
    });
    assert!(!proxy.online("hub-1"));
    // A wrong password fails the handshake.
    let node = mesh(&e, ROLE_NODE, "node-1");
    node.dial(&peer(&addr, ROLE_HUB, "nope")).unwrap();
    until("the auth failure", || {
        node.targets()[0].error.contains("authentication")
    });
    // A peer of the role expected, and of another role.
    let n2 = mesh(&e, ROLE_NODE, "node-2");
    assert!(
        n2.dial(&peer(&addr, ROLE_NODE, "np")).is_err(),
        "its own role"
    );
    assert!(n2.dial(&peer(&addr, "local", "np")).is_err());
    assert!(
        n2.dial(&peer("10.0.0.1:1", ROLE_HUB, "np")).is_err(),
        "TLS off loopback"
    );
    assert!(n2.dial(&peer(&addr, ROLE_HUB, "")).is_err(), "no password");
    n2.dial(&peer(&addr, ROLE_PROXY, "np")).unwrap();
    until("the role refused", || {
        n2.targets()[0].error.contains("configured as a proxy peer")
    });
    // The same ID as another role: refused.
    let twin = mesh(&e, ROLE_NODE, "node-ok");
    twin.dial(&peer(&addr, ROLE_HUB, "np")).unwrap();
    until("the node", || hub.online("node-ok"));
    let p2 = mesh(&e, ROLE_PROXY, "node-ok");
    p2.dial(&peer(&addr, ROLE_HUB, "pp")).unwrap();
    until("the conflict", || {
        p2.targets()[0]
            .error
            .contains("connected already as a node")
    });
    // A listener off loopback needs TLS.
    let h2 = mesh(&e, ROLE_HUB, "hub-2");
    assert!(
        h2.set_listen(&ListenConfig {
            addr: "0.0.0.0:0".to_owned(),
            password: "x".to_owned(),
            ..ListenConfig::default()
        })
        .is_err()
    );
    for m in [&hub, &proxy, &node, &n2, &twin, &p2] {
        m.stop();
    }
    e.shutdown();
}

#[test]
fn tokens_bind_their_members() {
    let e = engine();
    let hub = mesh(&e, ROLE_HUB, "hub-1");
    let (a, b) = (Secret::new().unwrap(), Secret::new().unwrap());
    let rec = |s: &Secret| LinkToken {
        id: s.id().to_owned(),
        name: "t".to_owned(),
        hash: s.hash(),
        ..LinkToken::default()
    };
    hub.tokens().set(&[rec(&a), rec(&b)]);
    let addr = listen_on(&hub, ListenConfig::default());
    let tok = |addr: &str, s: &Secret| DialReq {
        token: s.token(),
        ..peer(addr, ROLE_HUB, "")
    };
    let node = mesh(&e, ROLE_NODE, "node-1");
    node.dial(&tok(&addr, &a)).unwrap();
    until("the token's link", || hub.online("node-1"));
    let p = &hub.peers()[0];
    assert_eq!(p.token_id, a.id());
    assert!(!p.shared_password);
    assert_eq!(node.targets()[0].token_id, a.id());
    // Another token cannot take the ID over.
    let thief = mesh(&e, ROLE_NODE, "node-1");
    thief.dial(&tok(&addr, &b)).unwrap();
    until("the refusal", || {
        thief.targets()[0].error.contains("bound") || thief.targets()[0].error.contains("token")
    });
    // Revoked: its links close.
    hub.tokens().set(&[rec(&b)]);
    until("the link closed", || !hub.online("node-1"));
    for m in [&hub, &node, &thief] {
        m.stop();
    }
    e.shutdown();
}

#[test]
fn a_pool_of_links_and_drain() {
    let e = engine();
    let hub = mesh(&e, ROLE_HUB, "hub-1");
    let node = mesh(&e, ROLE_NODE, "node-1");
    let haddr = listen_on(
        &hub,
        ListenConfig {
            password: "pw".to_owned(),
            ..ListenConfig::default()
        },
    );
    let naddr = listen_on(
        &node,
        ListenConfig {
            password: "pw".to_owned(),
            ..ListenConfig::default()
        },
    );
    node.dial(&peer(&haddr, ROLE_HUB, "pw")).unwrap();
    hub.dial(&peer(&naddr, ROLE_NODE, "pw")).unwrap();
    until("two links", || {
        hub.peers().len() == 2 && node.peers().len() == 2
    });
    // Calls go over both.
    for _ in 0..10 {
        call(&hub, "node-1", "/echo", "").unwrap();
    }
    let p = hub.peers();
    assert!(p.iter().all(|c| c.link.bytes_out > 0), "{p:?}");
    // The node drains: the hub stops picking its links, and the node's stream under way ends.
    let st = hub.open_stream("node-1", "/count", "1").unwrap();
    let r = read_stream(&st);
    assert_eq!(r.1, Ok(()));
    let n2 = node.clone();
    let t = std::thread::spawn(move || n2.shutdown(Duration::from_secs(2)));
    until("the hub sees the drain", || !hub.online("node-1"));
    assert_eq!(call(&hub, "node-1", "/echo", "").unwrap_err(), offline());
    t.join().unwrap();
    until("the links gone", || hub.peers().is_empty());
    hub.stop();
    e.shutdown();
}

#[test]
fn a_link_without_hello_is_closed() {
    let e = engine();
    let hub = mesh(&e, ROLE_HUB, "hub-1");
    hub.set_hello_wait(Duration::from_millis(300));
    let addr = listen_on(
        &hub,
        ListenConfig {
            password: "pw".to_owned(),
            ..ListenConfig::default()
        },
    );
    let (tx, rx) = mpsc::channel();
    let cfg = wire::DialConfig {
        session: wire::Options {
            dialer: true,
            ..wire::Options::default()
        },
        timing: wire::conn::Timing::default(),
        token: None,
        password: "pw".to_owned(),
        tls: None,
        server_name: None,
        timeout: WAIT,
        handlers: Arc::new(|_| Arc::new(NoMesh)),
        on_close: Some(Arc::new(move |_, _| {
            let _ = tx.send(());
        })),
    };
    let (ctx, crx) = mpsc::channel();
    let _ = wire::conn::dial(&e, &format!("ws://{addr}/ws"), cfg, move |r| {
        let _ = ctx.send(r.is_ok());
    });
    assert!(crx.recv_timeout(WAIT).unwrap());
    rx.recv_timeout(WAIT).unwrap();
    hub.stop();
    e.shutdown();
}

/// A self-signed certificate for localhost: its PEM and its key's.
fn cert() -> (String, String) {
    let key = akio_crypto::p256::SecretKey::generate().unwrap();
    let key = akio_crypto::pkey::PrivateKey::P256(key);
    let mut p = akio_x509::CertificateParams::new(vec!["localhost", "127.0.0.1"]);
    let now = akit_roles::linktoken::unix_now();
    p.not_before = now - 3600;
    p.not_after = now + 86_400;
    let c = p.self_signed(&key).unwrap();
    (
        c.to_pem(),
        akio_crypto::pem::encode("PRIVATE KEY", &key.to_pkcs8_der()),
    )
}

#[test]
fn tls_with_a_pin() {
    let e = engine();
    let hub = mesh(&e, ROLE_HUB, "hub-1");
    let (c, k) = cert();
    let addr = listen_on(
        &hub,
        ListenConfig {
            password: "pw".to_owned(),
            cert: c.clone(),
            key: k,
            ..ListenConfig::default()
        },
    );
    let fp = hub.listen_state().fingerprint;
    assert_eq!(fp, fingerprint(&c));
    assert_eq!(fp.len(), 64);
    let node = mesh(&e, ROLE_NODE, "node-1");
    node.dial(&DialReq {
        pin: fp.clone(),
        ..peer(&addr, ROLE_HUB, "pw")
    })
    .unwrap();
    until("the TLS link", || hub.online("node-1"));
    assert!(node.targets()[0].tls);
    // A wrong pin links nothing.
    let n2 = mesh(&e, ROLE_NODE, "node-2");
    n2.dial(&DialReq {
        pin: "00".repeat(32),
        ..peer(&addr, ROLE_HUB, "pw")
    })
    .unwrap();
    until("the failure", || !n2.targets()[0].error.is_empty());
    assert!(!hub.online("node-2"));
    // Plain ws:// to a TLS listener fails too.
    let n3 = mesh(&e, ROLE_NODE, "node-3");
    n3.dial(&peer(&addr, ROLE_HUB, "pw")).unwrap();
    until("the failure", || !n3.targets()[0].error.is_empty());
    for m in [&hub, &node, &n2, &n3] {
        m.stop();
    }
    e.shutdown();
}

/// fit's `TestListenerCerts` for the mesh: a listener's `cert_file` read again at the next
/// configuration when its files changed, served in place (its links stay), reported for
/// `-watch-certs`; files that do not load leave the running certificate.
#[test]
fn cert_files_read_again() {
    let e = engine();
    let dir = std::env::temp_dir().join(format!(
        "akit-llm-mesh-certs-{}-{}",
        std::process::id(),
        akit_roles::linktoken::unix_now()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let (cf, kf) = (dir.join("c.pem"), dir.join("k.pem"));
    let (c1, k1) = cert();
    std::fs::write(&cf, &c1).unwrap();
    std::fs::write(&kf, &k1).unwrap();
    let hub = mesh(&e, ROLE_HUB, "hub-1");
    let l = ListenConfig {
        addr: "127.0.0.1:0".to_owned(),
        password: "pw".to_owned(),
        cert_file: cf.to_string_lossy().into_owned(),
        key_file: kf.to_string_lossy().into_owned(),
        ..ListenConfig::default()
    };
    hub.configure(&l, &[]).unwrap();
    let addr = hub.listen_addr();
    let l = ListenConfig {
        addr: addr.clone(),
        ..l
    };
    hub.configure(&l, &[]).unwrap(); // the address bound, as the config gives it
    let fp1 = hub.listen_state().fingerprint;
    assert_eq!(fp1, fingerprint(&c1));
    let files = hub.files();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].1, akio_crypto::hash::Sha256::digest(c1.as_bytes()));
    let n1 = mesh(&e, ROLE_NODE, "node-1");
    n1.dial(&DialReq {
        pin: fp1.clone(),
        ..peer(&addr, ROLE_HUB, "pw")
    })
    .unwrap();
    until("the first link", || hub.online("node-1"));
    let before = hub.peers()[0].conn_id;
    // A key not yet written: the running certificate stays.
    let (c2, k2) = cert();
    std::fs::write(&cf, &c2).unwrap();
    hub.configure(&l, &[]).unwrap();
    assert_eq!(hub.listen_state().fingerprint, fp1);
    assert_eq!(
        hub.files()[0].1,
        akio_crypto::hash::Sha256::digest(c2.as_bytes())
    );
    // Both written: served from now on, in place.
    std::fs::write(&kf, &k2).unwrap();
    hub.configure(&l, &[]).unwrap();
    let fp2 = hub.listen_state().fingerprint;
    assert_eq!(fp2, fingerprint(&c2));
    assert_eq!(hub.peers()[0].conn_id, before, "the link stayed");
    let n2 = mesh(&e, ROLE_NODE, "node-2");
    n2.dial(&DialReq {
        pin: fp2,
        ..peer(&addr, ROLE_HUB, "pw")
    })
    .unwrap();
    until("a link to the new certificate", || hub.online("node-2"));
    // The old pin no longer matches a new link.
    let n3 = mesh(&e, ROLE_NODE, "node-3");
    n3.dial(&DialReq {
        pin: fp1,
        ..peer(&addr, ROLE_HUB, "pw")
    })
    .unwrap();
    until("the old pin refused", || !n3.targets()[0].error.is_empty());
    assert!(!hub.online("node-3"));
    // Files gone: reported as zeros, the certificate kept.
    std::fs::remove_dir_all(&dir).unwrap();
    hub.configure(&l, &[]).unwrap();
    assert_eq!(hub.files()[0].1, [0u8; 32]);
    assert!(hub.online("node-1") && hub.online("node-2"));
    for m in [&hub, &n1, &n2, &n3] {
        m.stop();
    }
    e.shutdown();
}

#[test]
fn configure_moves_listener_and_targets() {
    let e = engine();
    let hub = mesh(&e, ROLE_HUB, "hub-1");
    let node = mesh(&e, ROLE_NODE, "node-1");
    let l = ListenConfig {
        addr: "127.0.0.1:0".to_owned(),
        password: "pw".to_owned(),
        ..ListenConfig::default()
    };
    hub.configure(&l, &[]).unwrap();
    let addr = hub.listen_addr();
    let peers = vec![peer(&addr, ROLE_HUB, "pw")];
    node.configure(&ListenConfig::default(), &peers).unwrap();
    until("the link", || hub.online("node-1"));
    // The same again changes nothing: the link stays.
    let before = hub.peers()[0].conn_id;
    node.configure(&ListenConfig::default(), &peers).unwrap();
    hub.configure(&l, &[]).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(hub.peers()[0].conn_id, before);
    // The target dropped: the link closes.
    node.configure(&ListenConfig::default(), &[]).unwrap();
    until("the link gone", || !hub.online("node-1"));
    assert!(node.targets().is_empty());
    // A bad target is reported, the rest applied.
    assert!(
        node.configure(&ListenConfig::default(), &[peer("nohost", ROLE_HUB, "pw")])
            .is_err()
    );
    hub.configure(&ListenConfig::default(), &[]).unwrap();
    assert!(hub.listen_addr().is_empty());
    hub.stop();
    node.stop();
    e.shutdown();
}
