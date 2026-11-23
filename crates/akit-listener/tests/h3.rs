//! HTTP/3 listeners (I10): a TLS listener serving `h3` serves the same routes over QUIC on its
//! port in UDP, with the certificates its TCP side serves (SNI, client authentication), and its
//! HTTP/1.1 and HTTP/2 responses announce it with `Alt-Svc`, as fit's do. TCP on the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`); UDP on akio's UDP loop.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
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
    assert_eq!(e.local_addrs(), [udp]);
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
