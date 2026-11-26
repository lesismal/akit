//! HTTP/3 listeners (I10): a TLS listener serving `h3` serves the same routes over QUIC on its
//! port in UDP, with the certificates its TCP side serves (SNI, client authentication), and its
//! HTTP/1.1 and HTTP/2 responses announce it with `Alt-Svc`, as fit's do. TCP on the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`); UDP on akio's UDP loop.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use akio::{Backend, Engine, UringConfig};
use akit_listener::Addrs;
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};
use support::certs::{Ca, dir};
use support::h3;
use support::https::{client, get_h1, get_h2_alt_svc};

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

fn serve(json: &str) -> (Engine, Vec<Addrs>) {
    let config = akit_config::from_json(json, None).unwrap();
    let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
    let cell = GraphCell::new(compile(&config, &registry, &Env::default()).unwrap());
    let e = akit_listener::engine(&cell, &options()).unwrap();
    e.start().unwrap();
    let addrs = akit_listener::addrs(&config.listeners, &e);
    (e, addrs)
}

/// Certificates under one CA: `a` for a.example (first, the default), `b` for b.example, and a
/// client certificate.
struct Pki {
    ca: PathBuf,
    a: (PathBuf, PathBuf),
    b: (PathBuf, PathBuf),
    client: (PathBuf, PathBuf),
    /// A client certificate from another CA.
    stranger: (PathBuf, PathBuf),
}

fn pki(name: &str) -> Pki {
    let d = dir(name);
    let ca = Ca::new("akit test ca");
    let other = Ca::new("another ca");
    Pki {
        ca: ca.write(&d, "ca.pem"),
        a: ca.leaf(&d, "a", &["a.example"]),
        b: ca.leaf(&d, "b", &["b.example"]),
        client: ca.leaf(&d, "client", &["client.example"]),
        stranger: other.leaf(&d, "stranger", &["client.example"]),
    }
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

/// A config of one TLS listener at `address` with `protocols` and `extra` in its `tls`, serving
/// `/hello`, and `/hello` on host b.example with another body.
fn config(p: &Pki, address: &str, protocols: &str, extra: &str) -> String {
    format!(
        r#"{{
  "listeners": [{{"name": "secure", "address": "{address}", "protocols": [{protocols}], "tls": {{
    "certificates": [{{"cert_file": "{}", "key_file": "{}"}}, {{"cert_file": "{}", "key_file": "{}"}}]{extra}}}}}],
  "routes": [
    {{"match": {{"path": ["/hello"]}}, "handler": {{"module": "respond", "params": {{"body": "hello\n"}}}}}},
    {{"match": {{"host": ["b.example"], "path": ["/hello"]}}, "handler": {{"module": "respond", "params": {{"body": "hello b\n"}}}}}}
  ]
}}"#,
        s(&p.a.0),
        s(&p.a.1),
        s(&p.b.0),
        s(&p.b.1)
    )
}

const ALL: &str = r#""http/1.1", "h2", "h3""#;

/// The I10 done-when: the same route over h3 as over h2, and `Alt-Svc` on h2 (and HTTP/1.1)
/// naming the UDP port, which is TCP's.
#[test]
fn the_same_route_over_h3_and_alt_svc_on_h2() {
    let p = pki("h3-route");
    let (e, addrs) = serve(&config(&p, "127.0.0.1:0", ALL, ""));
    let a = addrs[0];
    let (tcp, udp) = (a.tcp.unwrap(), a.udp.unwrap());
    // HTTP/3 goes on the port TCP got, the one Alt-Svc names.
    assert_eq!(tcp, udp);
    assert_eq!(a.addr(), Some(tcp));
    let want = format!("h3=\":{}\"; ma=86400", udp.port());
    assert_eq!(akit_listener::alt_svc(udp.port()), want);

    let (status, body, alt_svc) =
        get_h2_alt_svc(tcp, &client(&p.ca, &["h2"], None), "a.example", "/hello").unwrap();
    assert_eq!((status, body.as_str()), (200, "hello\n"));
    assert_eq!(alt_svc.as_deref(), Some(want.as_str()));
    // Over HTTP/1.1 too, and on a 404.
    let c1 = client(&p.ca, &["http/1.1"], None);
    let seen = get_h1(tcp, &c1, "a.example", "a.example", "/hello").unwrap();
    assert_eq!(seen.body, "hello\n");
    assert_eq!(seen.alt_svc.as_deref(), Some(want.as_str()));
    let seen = get_h1(tcp, &c1, "a.example", "a.example", "/nowhere").unwrap();
    assert_eq!(seen.status, "HTTP/1.1 404 Not Found");
    assert_eq!(seen.alt_svc.as_deref(), Some(want.as_str()));

    // The same route over HTTP/3; no Alt-Svc there.
    let seen = h3::get(udp, h3::client(&p.ca, None), "a.example", "/hello").unwrap();
    assert_eq!(
        seen,
        h3::Seen {
            alpn: Some("h3".into()),
            status: 200,
            alt_svc: None,
            body: "hello\n".into(),
        }
    );
    // A route by host, and the certificate by SNI (the client checks the name): b.example's.
    let seen = h3::get(udp, h3::client(&p.ca, None), "b.example", "/hello").unwrap();
    assert_eq!((seen.status, seen.body.as_str()), (200, "hello b\n"));
    // Canonical paths and 404, as over TCP.
    let seen = h3::get(udp, h3::client(&p.ca, None), "a.example", "/x/../hello").unwrap();
    assert_eq!((seen.status, seen.body.as_str()), (200, "hello\n"));
    let seen = h3::get(udp, h3::client(&p.ca, None), "a.example", "/nowhere").unwrap();
    assert_eq!(
        (seen.status, seen.body.as_str()),
        (404, "404 page not found\n")
    );
    // A name no certificate holds gets the first, which the client refuses.
    assert!(h3::get(udp, h3::client(&p.ca, None), "c.example", "/hello").is_err());
    assert!(akit_listener::shutdown(&e, Duration::from_secs(5)));
}

#[test]
fn h3_alone_listens_on_udp_alone() {
    let p = pki("h3-alone");
    let (e, addrs) = serve(&config(&p, "127.0.0.1:0", r#""h3""#, ""));
    let a = addrs[0];
    assert_eq!(a.tcp, None);
    let udp = a.udp.unwrap();
    // The listener's UDP socket alone, and after it, unbound, the TCP client thread an engine
    // without a TCP listener gets for the roles' dials (I100).
    let bound: Vec<_> = e
        .local_addrs()
        .into_iter()
        .filter(|a| a.port() != 0)
        .collect();
    assert_eq!(bound, [udp]);
    let seen = h3::get(udp, h3::client(&p.ca, None), "a.example", "/hello").unwrap();
    assert_eq!((seen.status, seen.body.as_str()), (200, "hello\n"));
    assert!(akit_listener::shutdown(&e, Duration::from_secs(5)));
}

#[test]
fn no_alt_svc_without_h3() {
    let p = pki("h3-none");
    let (e, addrs) = serve(&config(&p, "127.0.0.1:0", r#""http/1.1", "h2""#, ""));
    let a = addrs[0];
    assert_eq!(a.udp, None);
    let tcp = a.tcp.unwrap();
    let (status, _, alt_svc) =
        get_h2_alt_svc(tcp, &client(&p.ca, &["h2"], None), "a.example", "/hello").unwrap();
    assert_eq!((status, alt_svc), (200, None));
    // Nothing listens on UDP: the h3 client times out or is refused.
    assert!(h3::get(tcp, h3::client(&p.ca, None), "a.example", "/hello").is_err());
    assert!(akit_listener::shutdown(&e, Duration::from_secs(5)));
}

/// A fixed port: TCP and UDP on it, and Alt-Svc names it.
#[test]
fn a_fixed_port_serves_tcp_and_udp() {
    let p = pki("h3-fixed");
    // A port free for both, then given to the config.
    let port = loop {
        let t = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = t.local_addr().unwrap().port();
        if std::net::UdpSocket::bind(("127.0.0.1", port)).is_ok() {
            break port;
        }
    };
    let (e, addrs) = serve(&config(&p, &format!("127.0.0.1:{port}"), ALL, ""));
    let a = addrs[0];
    assert_eq!(a.tcp.unwrap().port(), port);
    assert_eq!(a.udp.unwrap().port(), port);
    let seen = get_h1(
        a.tcp.unwrap(),
        &client(&p.ca, &["http/1.1"], None),
        "a.example",
        "a.example",
        "/hello",
    )
    .unwrap();
    assert_eq!(seen.alt_svc, Some(format!("h3=\":{port}\"; ma=86400")));
    let seen = h3::get(
        a.udp.unwrap(),
        h3::client(&p.ca, None),
        "a.example",
        "/hello",
    )
    .unwrap();
    assert_eq!(seen.status, 200);
    assert!(akit_listener::shutdown(&e, Duration::from_secs(5)));
}

/// Client authentication holds over HTTP/3 as over TCP: the same authorities, the same mode.
#[test]
fn client_auth_over_h3() {
    let p = pki("h3-mtls");
    let ca = s(&p.ca);
    let get = |udp, cert: Option<&(PathBuf, PathBuf)>| {
        let c = h3::client(&p.ca, cert.map(|(c, k)| (c.as_path(), k.as_path())));
        h3::get(udp, c, "a.example", "/hello").map(|s| s.status)
    };

    let (e, addrs) = serve(&config(
        &p,
        "127.0.0.1:0",
        ALL,
        &format!(r#", "client_auth": {{"ca_files": ["{ca}"]}}"#),
    ));
    let udp = addrs[0].udp.unwrap();
    assert_eq!(get(udp, Some(&p.client)), Ok(200));
    assert!(get(udp, None).is_err(), "require: no certificate");
    assert!(get(udp, Some(&p.stranger)).is_err(), "another CA's");
    assert!(akit_listener::shutdown(&e, Duration::from_secs(5)));

    let (e, addrs) = serve(&config(
        &p,
        "127.0.0.1:0",
        ALL,
        &format!(r#", "client_auth": {{"ca_files": ["{ca}"], "mode": "verify_if_given"}}"#),
    ));
    let udp = addrs[0].udp.unwrap();
    assert_eq!(get(udp, Some(&p.client)), Ok(200));
    assert_eq!(get(udp, None), Ok(200));
    assert!(get(udp, Some(&p.stranger)).is_err(), "another CA's");
    assert!(akit_listener::shutdown(&e, Duration::from_secs(5)));
}

/// Datagrams that are not QUIC leave the listener serving.
#[test]
fn datagrams_that_are_not_quic_do_not_disturb_others() {
    let p = pki("h3-garbage");
    let (e, addrs) = serve(&config(&p, "127.0.0.1:0", ALL, ""));
    let udp = addrs[0].udp.unwrap();
    let junk = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    for d in [&b""[..], b"\x00", b"GET / HTTP/1.1\r\n\r\n", &[0xc0; 1200]] {
        junk.send_to(d, udp).unwrap();
    }
    let seen = h3::get(udp, h3::client(&p.ca, None), "a.example", "/hello").unwrap();
    assert_eq!(seen.status, 200);
    assert!(akit_listener::shutdown(&e, Duration::from_secs(5)));
}

/// Reload (I22) beside HTTP/3 (I10): a listener serving h3 has a UDP listener of its own in the
/// engine, so the listeners after it are still reported at their own addresses; a reload adds
/// one (its TCP side with akio's `add_listener`, A1, its HTTP/3 with `bind_udp`) and removes it
/// again, and removes one akit started with (akio's A22: a UDP listener removed at run time).
#[test]
fn reload_knows_h3_listeners() {
    use std::sync::Arc;

    use akit_listener::Listeners;

    let p = pki("h3-reload");
    let tls = |name: &str, protocols: &str| {
        format!(
            r#"{{"name": "{name}", "address": "127.0.0.1:0", "protocols": [{protocols}],
                "tls": {{"certificates": [{{"cert_file": "{}", "key_file": "{}"}}]}}}}"#,
            s(&p.a.0),
            s(&p.a.1)
        )
    };
    let json = |listeners: &[String]| {
        format!(
            r#"{{"listeners": [{}], "routes": [{{"handler": {{"module": "respond", "params": {{"body": "hi"}}}}}}]}}"#,
            listeners.join(",")
        )
    };
    let plain = r#"{"name": "plain", "address": "127.0.0.1:0"}"#.to_owned();
    let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
    let compiled = |j: &str| {
        let c = akit_config::from_json(j, None).unwrap();
        (compile(&c, &registry, &Env::default()).unwrap(), c)
    };
    let (g, config) = compiled(&json(&[tls("secure", ALL), plain.clone()]));
    let cell = GraphCell::new(g);
    let e = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
    e.start().unwrap();
    let addrs = akit_listener::addrs(&config.listeners, &e);
    let mut l = Listeners::new(Arc::clone(&e), cell.clone(), &options());
    let served: Vec<_> = l
        .served()
        .into_iter()
        .map(|s| (s.name, s.address))
        .collect();
    assert_eq!(
        served,
        [
            ("secure".to_owned(), addrs[0].tcp.unwrap()),
            ("plain".to_owned(), addrs[1].tcp.unwrap()),
        ]
    );
    // The plain listener really is there.
    let mut s = std::net::TcpStream::connect(addrs[1].tcp.unwrap()).unwrap();
    std::io::Write::write_all(
        &mut s,
        b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    )
    .unwrap();
    let mut resp = String::new();
    std::io::Read::read_to_string(&mut s, &mut resp).unwrap();
    assert!(resp.ends_with("hi"), "{resp}");

    // A listener serving h3 added by a reload: TCP added to the engine (A1), HTTP/3 bound on
    // the engine's UDP thread, on the port TCP got.
    let (g, _) = compiled(&json(&[
        tls("secure", ALL),
        plain.clone(),
        tls("more", ALL),
    ]));
    let r = l.reload(g).unwrap();
    assert_eq!((r.kept, r.added.len(), r.removed.len()), (2, 1, 0));
    let more = r.added[0].address;
    let seen = h3::get(more, h3::client(&p.ca, None), "a.example", "/").unwrap();
    assert_eq!((seen.status, seen.body.as_str()), (200, "hi"));
    let c1 = client(&p.ca, &["http/1.1"], None);
    let seen = get_h1(more, &c1, "a.example", "a.example", "/").unwrap();
    assert_eq!(seen.body, "hi");
    let want = akit_listener::alt_svc(more.port());
    assert_eq!(seen.alt_svc.as_deref(), Some(want.as_str()));

    // The h3 listener akit started with is removed (akio's A22), its TCP and UDP sides both.
    let secure = addrs[0];
    let (g, _) = compiled(&json(&[plain.clone(), tls("more", ALL)]));
    let r = l.reload(g).unwrap();
    assert_eq!((r.kept, r.added.len(), r.removed.len()), (2, 0, 1));
    assert_eq!(r.removed[0].name, "secure");
    assert!(std::net::TcpStream::connect(secure.tcp.unwrap()).is_err());
    wait_h3_gone(secure.udp.unwrap(), &p.ca);
    // The one a reload added stays, and goes with the next.
    let seen = h3::get(more, h3::client(&p.ca, None), "a.example", "/").unwrap();
    assert_eq!(seen.status, 200);
    let (g, _) = compiled(&json(&[plain]));
    let r = l.reload(g).unwrap();
    assert_eq!((r.kept, r.added.len(), r.removed.len()), (1, 0, 1));
    wait_h3_gone(more, &p.ca);
    e.shutdown();
    e.wait();
}

/// A port free in TCP and UDP both.
fn free_port() -> u16 {
    loop {
        let t = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = t.local_addr().unwrap().port();
        if std::net::UdpSocket::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
}

/// Waits for HTTP/3 at `udp` to answer no more, within 15 seconds: a removed listener's UDP
/// sockets close once its last peer is gone, or at its drain's end.
fn wait_h3_gone(udp: std::net::SocketAddr, ca: &Path) {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while h3::get(udp, h3::client(ca, None), "a.example", "/").is_ok() {
        assert!(
            std::time::Instant::now() < deadline,
            "{udp} still answers HTTP/3"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A handler that answers `/slow` a second after it came, and anything else at once, and says
/// when a `/slow` request came.
mod slow {
    use std::sync::mpsc::Sender;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use akio_http::{Context, Response};
    use akit_runtime::{Handler, HandlerParams, Info, Provision, ProvisionError, Provisioned};

    pub(crate) static CAME: Mutex<Option<Sender<()>>> = Mutex::new(None);

    struct Slow;

    impl akio_http::Service for Slow {
        fn call(&self, ctx: &mut Context) {
            if ctx.path() != "/slow" {
                return ctx.respond(Response::text(200, "hi"));
            }
            let Some(d) = ctx.detach() else { return };
            if let Some(tx) = CAME.lock().unwrap().as_ref() {
                let _ = tx.send(());
            }
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(1));
                d.respond(Response::text(200, "slow"));
            });
        }
    }

    impl Handler for Slow {}

    fn new(_: &Provision<'_, HandlerParams>) -> Result<Provisioned<dyn Handler>, ProvisionError> {
        Ok(Provisioned::new(Arc::new(Slow) as Arc<dyn Handler>))
    }

    pub(crate) static MODULES: &[Info<HandlerParams>] = &[Info::handler(
        "http.handlers.slow",
        "Answers /slow late.",
        new,
    )];
}

/// A reload that changes or removes a listener serving h3 drains its HTTP/3 as its TCP side
/// (akio's A22): the one akit started with replaced by another on its address, whose HTTP/3
/// serves once the old one's short drain (`UDP_REPLACED_DRAIN`) is over at the latest; that one
/// removed while a request is in flight on it, which is answered, the address answering no
/// more after.
#[test]
fn reload_drains_h3() {
    use std::sync::Arc;
    use std::sync::mpsc::channel;

    use akit_listener::Listeners;

    let p = pki("h3-drain");
    let port = free_port();
    let listener = |name: &str, body: &str| {
        format!(
            r#"{{"listeners": [{{"name": "{name}", "address": "127.0.0.1:{port}", "protocols": [{ALL}],
                "tls": {{"certificates": [{{"cert_file": "{}", "key_file": "{}"}}]}}}}],
              "routes": [{{"handler": {{"module": "{body}"}}}}]}}"#,
            s(&p.a.0),
            s(&p.a.1)
        )
    };
    let registry: Registry<HandlerParams> =
        Registry::new(&[akit_handler::MODULES, slow::MODULES]).unwrap();
    let compiled = |j: &str| {
        let c = akit_config::from_json(j, None).unwrap();
        compile(&c, &registry, &Env::default()).unwrap()
    };
    let cell = GraphCell::new(compiled(&listener("one", "slow")));
    let e = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
    e.start().unwrap();
    let mut l = Listeners::new(Arc::clone(&e), cell.clone(), &options());
    let udp: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

    let seen = h3::get(udp, h3::client(&p.ca, None), "a.example", "/").unwrap();
    assert_eq!(seen.body, "hi");
    // Changed (renamed), on the same address: the old one drains, the new one serves. The
    // kernel may hash a new peer to the old sockets while they drain, which drop it: tried
    // again until they are gone.
    let r = l.reload(compiled(&listener("two", "slow"))).unwrap();
    assert_eq!((r.kept, r.added.len(), r.removed.len()), (0, 1, 1));
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let seen = loop {
        match h3::get(udp, h3::client(&p.ca, None), "a.example", "/") {
            Ok(seen) => break seen,
            Err(e) => assert!(std::time::Instant::now() < deadline, "{e}"),
        }
    };
    assert_eq!(seen.body, "hi");

    // Removed, with nothing in its place, while a request is in flight on it: the request is
    // answered, then the address answers no more.
    let (tx, came) = channel();
    *slow::CAME.lock().unwrap() = Some(tx);
    let ca = p.ca.clone();
    let inflight =
        std::thread::spawn(move || h3::get(udp, h3::client(&ca, None), "a.example", "/slow"));
    came.recv_timeout(Duration::from_secs(10)).unwrap();
    let plain = r#"{"listeners": [{"name": "plain", "address": "127.0.0.1:0"}],
            "routes": [{"handler": {"module": "slow"}}]}"#;
    let r = l.reload(compiled(plain)).unwrap();
    assert_eq!((r.kept, r.added.len(), r.removed.len()), (0, 1, 1));
    let seen = inflight.join().unwrap().unwrap();
    assert_eq!((seen.status, seen.body.as_str()), (200, "slow"));
    wait_h3_gone(udp, &p.ca);
    *slow::CAME.lock().unwrap() = None;
    e.shutdown();
    e.wait();
}
