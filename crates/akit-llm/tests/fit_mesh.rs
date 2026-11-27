//! akit's mesh linked with fit's own roles (decision 038): an akit node dials a fit hub, a fit
//! node dials an akit hub, with the shared password, with tokens each listener issued, and over
//! TLS with a pin. Needs fit's binary: `AKIT_FIT_BIN=/path/to/fit cargo test -p akit-llm --test
//! fit_mesh -- --ignored`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use akit_config::roles::{LinkToken, llm_peer_roles};
use akit_llm::json;
use akit_llm::mesh::{Config, Limits, Mesh, Router, fingerprint, version};
use akit_llm::model::{DialReq, Identity, ListenConfig, ROLE_HUB, ROLE_NODE};
use akit_roles::linktoken::Secret;

fn fit_bin() -> String {
    std::env::var("AKIT_FIT_BIN").expect("AKIT_FIT_BIN: the path of fit's binary")
}

/// A port free now, for a listener of fit's binary, whose config must name it before it starts:
/// another process may take it before fit binds it.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn until(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// fit running a config, killed when dropped.
struct Fit(Child, tempdir::Dir);

impl Drop for Fit {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

mod tempdir {
    /// A directory removed when dropped.
    pub(crate) struct Dir(pub(crate) std::path::PathBuf);
    impl Dir {
        pub(crate) fn new(tag: &str) -> Dir {
            let p = std::env::temp_dir().join(format!(
                "akit-llm-fit-{tag}-{}-{}",
                std::process::id(),
                akit_roles::linktoken::unix_now()
            ));
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn fit(tag: &str, config: &str) -> Fit {
    let dir = tempdir::Dir::new(tag);
    let path = dir.0.join("fit.json");
    std::fs::write(&path, config).unwrap();
    let log = std::fs::File::create(dir.0.join("fit.log")).unwrap();
    let child = Command::new(fit_bin())
        .args(["run", "-c"])
        .arg(&path)
        .stdout(Stdio::null())
        .stderr(log)
        .spawn()
        .unwrap();
    Fit(child, dir)
}

impl Fit {
    fn log(&self) -> String {
        std::fs::read_to_string(self.1.0.join("fit.log")).unwrap_or_default()
    }
}

fn engine() -> Arc<akio::Engine> {
    let e = akio::Engine::builder().client_threads(2).build().unwrap();
    e.start().unwrap();
    Arc::new(e)
}

fn mesh(e: &Arc<akio::Engine>, role: &'static str, id: &str) -> Mesh {
    let id = id.to_owned();
    let m = Mesh::new(
        Config {
            identity: Arc::new(move || Identity {
                role: role.to_owned(),
                id: id.clone(),
                name: id.clone(),
                hostname: "akit-test".to_owned(),
                version: version(),
                pid: i64::from(std::process::id()),
                ..Identity::default()
            }),
            accept_roles: llm_peer_roles(role)
                .iter()
                .map(|r| (*r).to_owned())
                .collect(),
            engine: Arc::clone(e),
        },
        Router::new(),
    );
    m.set_limits(Limits {
        retry: Duration::from_millis(300),
        ..Limits::default()
    });
    m
}

/// A self-signed certificate for localhost and its key, in files under `dir`.
fn cert_files(dir: &std::path::Path) -> (String, String, String) {
    let key = akio_crypto::p256::SecretKey::generate().unwrap();
    let key = akio_crypto::pkey::PrivateKey::P256(key);
    let mut p = akio_x509::CertificateParams::new(vec!["localhost", "127.0.0.1"]);
    let now = akit_roles::linktoken::unix_now();
    p.not_before = now - 3600;
    p.not_after = now + 86_400;
    let c = p.self_signed(&key).unwrap();
    let (cert, key) = (
        c.to_pem(),
        akio_crypto::pem::encode("PRIVATE KEY", &key.to_pkcs8_der()),
    );
    let (cf, kf) = (dir.join("cert.pem"), dir.join("key.pem"));
    std::fs::write(&cf, &cert).unwrap();
    std::fs::write(&kf, &key).unwrap();
    (
        cf.to_string_lossy().into_owned(),
        kf.to_string_lossy().into_owned(),
        fingerprint(&cert),
    )
}

fn token_rec(s: &Secret) -> String {
    json::Obj::new()
        .str("id", s.id())
        .str("name", "akit")
        .str("hash", &s.hash())
        .end()
}

fn fit_token(s: &Secret) -> String {
    s.token().replacen("akitt1.", "fitt1.", 1)
}

#[test]
#[ignore = "needs fit's binary in AKIT_FIT_BIN"]
fn akit_node_links_to_fit_hub() {
    let e = engine();
    let (p1, p2) = (free_port(), free_port());
    let tok = Secret::new().unwrap();
    let dir = tempdir::Dir::new("certs");
    let (cf, kf, fp) = cert_files(&dir.0);
    // A fit hub: one listener with the shared password and a token it issued; another over TLS.
    let hub = fit(
        "hub",
        &format!(
            r#"{{"llm": {{"hub": {{"id": "hub-fit00000000000001", "name": "fit-hub",
              "listen": {{"address": "127.0.0.1:{p1}", "password": "pw"}},
              "tokens": [{}]}}}}}}"#,
            token_rec(&tok)
        ),
    );
    let hub_tls = fit(
        "hubtls",
        &format!(
            r#"{{"llm": {{"hub": {{"id": "hub-fit00000000000002", "name": "fit-hub-tls",
              "listen": {{"address": "127.0.0.1:{p2}", "password": "pw", "cert_file": "{cf}", "key_file": "{kf}"}}}}}}}}"#
        ),
    );
    let node = mesh(&e, ROLE_NODE, "node-akit000000000001");
    node.dial(&DialReq {
        addr: format!("127.0.0.1:{p1}"),
        role: ROLE_HUB.to_owned(),
        password: "pw".to_owned(),
        ..DialReq::default()
    })
    .unwrap();
    until("the akit node to say hello to fit's hub", || {
        node.online("hub-fit00000000000001")
    });
    let p = &node.peers()[0];
    assert_eq!(p.identity.role, "hub");
    assert!(p.identity.version.starts_with("fit"), "{p:?}");
    until("fit's hub to log the akit node", || {
        hub.log().contains("node-akit000000000001")
    });
    // With the token fit's hub issued.
    let n2 = mesh(&e, ROLE_NODE, "node-akit000000000002");
    n2.dial(&DialReq {
        addr: format!("127.0.0.1:{p1}"),
        role: ROLE_HUB.to_owned(),
        token: tok.token(),
        ..DialReq::default()
    })
    .unwrap();
    until("the token's link", || n2.online("hub-fit00000000000001"));
    // Over TLS, pinned.
    let n3 = mesh(&e, ROLE_NODE, "node-akit000000000003");
    n3.dial(&DialReq {
        addr: format!("127.0.0.1:{p2}"),
        role: ROLE_HUB.to_owned(),
        password: "pw".to_owned(),
        pin: fp.clone(),
        ..DialReq::default()
    })
    .unwrap();
    until("the TLS link", || n3.online("hub-fit00000000000002"));
    // A wrong pin links nothing; a wrong password neither.
    let n4 = mesh(&e, ROLE_NODE, "node-akit000000000004");
    n4.dial(&DialReq {
        addr: format!("127.0.0.1:{p2}"),
        role: ROLE_HUB.to_owned(),
        password: "pw".to_owned(),
        pin: "11".repeat(32),
        ..DialReq::default()
    })
    .unwrap();
    let n5 = mesh(&e, ROLE_NODE, "node-akit000000000005");
    n5.dial(&DialReq {
        addr: format!("127.0.0.1:{p1}"),
        role: ROLE_HUB.to_owned(),
        password: "nope".to_owned(),
        ..DialReq::default()
    })
    .unwrap();
    until("the failures", || {
        !n4.targets()[0].error.is_empty() && n5.targets()[0].error.contains("authentication")
    });
    assert!(!n4.online("hub-fit00000000000002") && !n5.online("hub-fit00000000000001"));
    for m in [&node, &n2, &n3, &n4, &n5] {
        m.stop();
    }
    drop((hub, hub_tls));
    e.shutdown();
}

#[test]
#[ignore = "needs fit's binary in AKIT_FIT_BIN"]
fn fit_node_links_to_akit_hub() {
    let e = engine();
    let hub = mesh(&e, ROLE_HUB, "hub-akit000000000001");
    let updates: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let u = Arc::clone(&updates);
    hub.on_push("/node/update", &[ROLE_NODE], move |caller, msg| {
        let v = json::parse(msg).unwrap();
        u.lock()
            .unwrap()
            .push((caller.id.clone(), json::get_str(&v, "id")));
    });
    let tok = Secret::new().unwrap();
    hub.tokens().set(&[LinkToken {
        id: tok.id().to_owned(),
        name: "fit".to_owned(),
        hash: tok.hash(),
        ..LinkToken::default()
    }]);
    let dir = tempdir::Dir::new("certs2");
    let (cf, kf, fp) = cert_files(&dir.0);
    hub.set_listen(&ListenConfig {
        addr: "127.0.0.1:0".to_owned(),
        node_password: "np".to_owned(),
        ..ListenConfig::default()
    })
    .unwrap();
    let addr = hub.listen_addr();
    let hub_tls = mesh(&e, ROLE_HUB, "hub-akit000000000002");
    hub_tls
        .set_listen(&ListenConfig {
            addr: "127.0.0.1:0".to_owned(),
            password: "pw".to_owned(),
            cert_file: cf,
            key_file: kf,
            ..ListenConfig::default()
        })
        .unwrap();
    let tls_addr = hub_tls.listen_addr();
    assert_eq!(hub_tls.listen_state().fingerprint, fp);
    let n1 = fit(
        "node1",
        &format!(
            r#"{{"llm": {{"node": {{"id": "node-fit0000000000001", "name": "fit-node",
              "peers": [{{"address": "{addr}", "password": "np", "role": "hub"}}]}}}}}}"#
        ),
    );
    until("fit's node to say hello to the akit hub", || {
        hub.online("node-fit0000000000001")
    });
    let p = hub.peers();
    assert_eq!(p[0].identity.role, "node");
    assert!(p[0].shared_password);
    until("fit's node to push its info", || {
        updates
            .lock()
            .unwrap()
            .iter()
            .any(|(c, id)| c == "node-fit0000000000001" && id == "node-fit0000000000001")
    });
    // With a token the akit hub issued, written as fit reads it.
    let n2 = fit(
        "node2",
        &format!(
            r#"{{"llm": {{"node": {{"id": "node-fit0000000000002",
              "peers": [{{"address": "{addr}", "token": "{}", "role": "hub"}}]}}}}}}"#,
            fit_token(&tok)
        ),
    );
    until("the token's link", || hub.online("node-fit0000000000002"));
    assert!(
        hub.peers()
            .iter()
            .any(|p| p.token_id == tok.id() && p.identity.id == "node-fit0000000000002")
    );
    // Over TLS, pinned by fit.
    let n3 = fit(
        "node3",
        &format!(
            r#"{{"llm": {{"node": {{"id": "node-fit0000000000003",
              "peers": [{{"address": "{tls_addr}", "password": "pw", "role": "hub", "pin": "{fp}"}}]}}}}}}"#
        ),
    );
    until("the TLS link", || hub_tls.online("node-fit0000000000003"));
    // A calling hub: fit's node answers its info.
    let (tx, rx) = mpsc::channel();
    hub.call(
        "node-fit0000000000001",
        "/node/info",
        "{}",
        Duration::ZERO,
        move |r| {
            let _ = tx.send(r);
        },
    );
    let info = rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    let v = json::parse(&info).unwrap();
    assert_eq!(json::get_str(&v, "id"), "node-fit0000000000001");
    drop((n1, n2, n3));
    until("the links gone", || hub.peers().is_empty());
    hub.stop();
    hub_tls.stop();
    e.shutdown();
}
