//! TLS to `h2` upstreams (I54), as fit's `upstream/tls_test.go` and `TestProbeProtocols` test it:
//! a backend that requires a client certificate from its own authority is reached by requests and
//! probes with `tls_cert_file`, and refused without one; its certificate names `example.com`,
//! which `tls_server_name` asks for in place of the address, over HTTP/2 and over HTTP/1.1 when
//! the server chooses it in ALPN (akio's `Client::with_tls_server_name`, A108). The pool is made
//! as the graph makes it: its TLS loaded from the files by `akit_tls::UpstreamTls` and given to
//! `Pool::new_with_tls`. On the backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

#[path = "../../akit-listener/tests/support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, Listener, UringConfig};
use akio_http::client::ClientRequest;
use akio_http::{Context, Version};
use akit_runtime::upstream::{EngineSlot, Pool};
use akit_tls::UpstreamTls;

fn backend() -> Backend {
    match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    }
}

/// The engine whose client threads the pools dial through.
fn clients() -> Arc<Engine> {
    static E: OnceLock<Arc<Engine>> = OnceLock::new();
    E.get_or_init(|| {
        let e = Engine::builder().client_threads(2).build().unwrap();
        e.start().unwrap();
        Arc::new(e)
    })
    .clone()
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

/// The files of a test: the upstream's authority, the server's certificate (for `example.com`
/// only), and a client certificate the authority signed.
struct Files {
    ca: PathBuf,
    server: (PathBuf, PathBuf),
    client: (PathBuf, PathBuf),
}

fn files(name: &str) -> Files {
    let d = support::certs::dir(name);
    let ca = support::certs::Ca::new("upstream CA");
    Files {
        ca: ca.write(&d, "ca.pem"),
        server: ca.leaf(&d, "server", &["example.com"]),
        client: ca.leaf(&d, "client", &["akit"]),
    }
}

/// A backend over TLS offering `alpn`, requiring a client certificate from `f.ca`. It answers
/// with the request's version and whether the client certificate it verified is `f.client`'s,
/// and counts the requests to `/health`.
struct Up {
    engine: Arc<Engine>,
    probes: Arc<AtomicUsize>,
}

impl Up {
    fn start(f: &Files, alpn: &[&str]) -> Up {
        let mut roots = akio::tls::RootCerts::empty();
        roots.add_pem(&std::fs::read(&f.ca).unwrap()).unwrap();
        let scfg = akio::tls::ServerConfig::builder(
            akio::tls::Certificate::from_pem(&std::fs::read(&f.server.0).unwrap()).unwrap(),
            akio::tls::PrivateKey::from_pem(&std::fs::read(&f.server.1).unwrap()).unwrap(),
        )
        .alpn(alpn)
        .client_auth_required(roots)
        .build()
        .unwrap();
        let client = support::https::leaf_der(&f.client.0);
        let probes = Arc::new(AtomicUsize::new(0));
        let p = probes.clone();
        let service = akio_http::serve(move |ctx: &mut Context| {
            let req = ctx.req();
            let v = if req.version == Version::Http2 {
                "h2"
            } else {
                "h1"
            };
            let who = match req.peer_certificates.as_deref() {
                Some([leaf, ..]) if *leaf == client => "akit",
                Some(_) => "someone else",
                None => "nobody",
            };
            if req.target == "/health" {
                p.fetch_add(1, Ordering::SeqCst);
            }
            ctx.text(200, format!("{v} {who}"));
        });
        let e = Engine::builder()
            .listen(
                Listener::tcp("127.0.0.1:0".parse().unwrap())
                    .backend(backend())
                    .threads(2)
                    .serve(akio::tls::Tls::server(scfg, service)),
            )
            .build()
            .unwrap();
        e.start().unwrap();
        Up {
            engine: Arc::new(e),
            probes,
        }
    }

    fn addr(&self) -> String {
        self.engine.local_addrs()[0].to_string()
    }
}

impl Drop for Up {
    fn drop(&mut self) {
        self.engine.shutdown();
    }
}

/// The pool of an `h2` upstream at `addr` with `fields`, made as the graph makes it.
fn pool(addr: &str, fields: &str) -> Pool {
    let json = format!(
        r#"{{"upstreams": [{{"name": "app", "addresses": ["{addr}"], "protocol": "h2", "max_fails": 0{fields}}}]}}"#
    );
    let u = akit_config::from_json(&json, None)
        .unwrap()
        .upstreams
        .remove(0);
    let tls = UpstreamTls::load(&u).unwrap().map(|t| t.config().clone());
    Pool::new_with_tls(&u, EngineSlot::with(clients()), tls)
}

/// GET `path` through `pool`: the body, or the error.
fn get(pool: &Pool, path: &str) -> Result<String, String> {
    let (tx, rx) = channel();
    pool.request(ClientRequest::get(path), move |r| {
        let _ = tx.send(r.map(|r| String::from_utf8_lossy(&r.body).into_owned()));
    });
    rx.recv_timeout(Duration::from_secs(10))
        .unwrap()
        .map_err(|e| e.to_string())
}

/// Probes every 200 ms. Each that reaches the backend has passed its TLS handshake, client
/// certificate included (`probes_passed` counts them).
const PROBES: &str =
    r#", "health_check": {"path": "/health", "interval": "200ms", "timeout": "200ms", "fails": 3}"#;

/// The probes reached the backend a few times, and left the address in rotation.
fn probes_passed(up: &Up, pool: &Pool) {
    let start = Instant::now();
    while up.probes.load(Ordering::SeqCst) < 3 {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "no probes arrived"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !pool.is_probed_down(0),
        "the probes, with the same certificate, failed"
    );
}

/// fit's `TestClientCertificate`, over HTTP/2 (and its probes: fit's `TestProbeProtocols`, h2).
#[test]
fn client_certificate() {
    let f = files("akit-proxy-upstream-tls-cert");
    let up = Up::start(&f, &["h2", "http/1.1"]);
    let common = format!(
        r#", "tls_ca_file": "{}", "tls_server_name": "example.com""#,
        s(&f.ca)
    );
    let with_cert = format!(
        r#"{common}, "tls_cert_file": "{}", "tls_key_file": "{}"{PROBES}"#,
        s(&f.client.0),
        s(&f.client.1)
    );
    let p = pool(&up.addr(), &with_cert);
    assert_eq!(get(&p, "/").unwrap(), "h2 akit");
    probes_passed(&up, &p);
    p.close();

    // Without a client certificate, the server refuses the handshake.
    let p = pool(&up.addr(), &common);
    assert!(get(&p, "/").is_err());
    p.close();
}

/// Without `tls_server_name`, the certificate (for `example.com`) does not name the address
/// dialed; with another name, it does not either.
#[test]
fn server_name() {
    let f = files("akit-proxy-upstream-tls-name");
    let up = Up::start(&f, &["h2", "http/1.1"]);
    let cert = format!(
        r#", "tls_ca_file": "{}", "tls_cert_file": "{}", "tls_key_file": "{}""#,
        s(&f.ca),
        s(&f.client.0),
        s(&f.client.1)
    );
    for (name, ok) in [
        ("", false),
        (r#", "tls_server_name": "other.example""#, false),
        (r#", "tls_server_name": "example.com""#, true),
    ] {
        let p = pool(&up.addr(), &format!("{cert}{name}"));
        let got = get(&p, "/");
        assert_eq!(got.is_ok(), ok, "{name:?}: {got:?}");
        p.close();
    }
}

/// A server that chooses HTTP/1.1 in ALPN (I18b's fallback): the pool's HTTP/1 client and the
/// probes after it present the client certificate and ask for `tls_server_name` too.
#[test]
fn http1_fallback_keeps_the_name_and_the_certificate() {
    let f = files("akit-proxy-upstream-tls-h1");
    let up = Up::start(&f, &["http/1.1"]);
    let fields = format!(
        r#", "tls_ca_file": "{}", "tls_server_name": "example.com", "tls_cert_file": "{}", "tls_key_file": "{}"{PROBES}"#,
        s(&f.ca),
        s(&f.client.0),
        s(&f.client.1)
    );
    let p = pool(&up.addr(), &fields);
    for _ in 0..2 {
        assert_eq!(get(&p, "/").unwrap(), "h1 akit");
    }
    assert_eq!(p.h2_connections(0), Some((0, 0)));
    probes_passed(&up, &p);
    p.close();
    // Without the name, the HTTP/1 client checks the address and fails.
    let fields = format!(
        r#", "tls_ca_file": "{}", "tls_cert_file": "{}", "tls_key_file": "{}""#,
        s(&f.ca),
        s(&f.client.0),
        s(&f.client.1)
    );
    let p = pool(&up.addr(), &fields);
    assert!(get(&p, "/").is_err());
    p.close();
}
