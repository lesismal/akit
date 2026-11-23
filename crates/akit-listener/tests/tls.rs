//! TLS listeners (I09): certificates from files picked by SNI, ALPN choosing h2 or http/1.1, and
//! client authentication. On the backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use akio::{Backend, Engine, UringConfig};
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};
use support::certs::{Ca, dir};
use support::https::{client, get_h1, get_h2, leaf_der};

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

fn serve(json: &str) -> (Engine, SocketAddr) {
    let config = akit_config::from_json(json, None).unwrap();
    let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
    let cell = GraphCell::new(compile(&config, &registry, &Env::default()).unwrap());
    let e = akit_listener::engine(&cell, &options()).unwrap();
    e.start().unwrap();
    let addr = e.local_addrs()[0];
    (e, addr)
}

/// Certificates under one CA: `a` for a.example and its subdomains (first, the default), `b`
/// for b.example, and a client certificate.
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
        a: ca.leaf(&d, "a", &["a.example", "*.a.example"]),
        b: ca.leaf(&d, "b", &["b.example"]),
        client: ca.leaf(&d, "client", &["client.example"]),
        stranger: other.leaf(&d, "stranger", &["client.example"]),
    }
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

/// A config of one TLS listener with `protocols` (fit's default when empty) and `extra` in its
/// `tls`, serving `/hello`, and `/hello` on host b.example with another body.
fn config(p: &Pki, protocols: &str, extra: &str) -> String {
    let protocols = if protocols.is_empty() {
        String::new()
    } else {
        format!(r#""protocols": [{protocols}], "#)
    };
    format!(
        r#"{{
  "listeners": [{{"name": "secure", "address": "127.0.0.1:0", {protocols}"tls": {{
    "certificates": [{{"cert_file": "{}", "key_file": "{}"}}, {{"cert_file": "{}", "key_file": "{}"}}]{extra}}}}}],
  "routes": [
    {{"match": {{"path": ["/hello"]}}, "handler": {{"module": "respond", "params": {{"body": "hello over tls\n"}}}}}},
    {{"match": {{"host": ["b.example"], "path": ["/hello"]}}, "handler": {{"module": "respond", "params": {{"body": "hello b\n"}}}}}}
  ]
}}"#,
        s(&p.a.0),
        s(&p.a.1),
        s(&p.b.0),
        s(&p.b.1)
    )
}

#[test]
fn https_over_http1_and_h2_chosen_by_alpn() {
    let p = pki("listener-alpn");
    let (e, addr) = serve(&config(&p, "", ""));
    // fit's default protocols with tls: http/1.1 and h2, h2 first.
    for (offer, want) in [
        (&["http/1.1"][..], Some("http/1.1")),
        (&["h2", "http/1.1"][..], Some("h2")),
        // No ALPN: HTTP/1.1, as Go's server serves it.
        (&[][..], None),
    ] {
        if want == Some("h2") {
            let r = get_h2(addr, &client(&p.ca, offer, None), "a.example", "/hello");
            assert_eq!(r, Ok((200, "hello over tls\n".into())), "{offer:?}");
            continue;
        }
        let seen = get_h1(
            addr,
            &client(&p.ca, offer, None),
            "a.example",
            "a.example",
            "/hello",
        )
        .unwrap();
        assert_eq!(seen.alpn.as_deref(), want, "{offer:?}");
        assert_eq!(seen.status, "HTTP/1.1 200 OK");
        assert_eq!(seen.body, "hello over tls\n");
    }
    // Over HTTP/2, a route by host, as over HTTP/1.1.
    assert_eq!(
        get_h2(addr, &client(&p.ca, &["h2"], None), "b.example", "/hello"),
        Ok((200, "hello b\n".into()))
    );
    assert!(akit_listener::shutdown(&e, Duration::from_secs(5)));
}

#[test]
fn alpn_follows_the_listeners_protocols() {
    let p = pki("listener-protocols");
    // http/1.1 alone: a client offering h2 too gets HTTP/1.1.
    let (e, addr) = serve(&config(&p, r#""http/1.1""#, ""));
    let seen = get_h1(
        addr,
        &client(&p.ca, &["h2", "http/1.1"], None),
        "a.example",
        "a.example",
        "/hello",
    )
    .unwrap();
    assert_eq!(seen.alpn.as_deref(), Some("http/1.1"));
    assert!(akit_listener::shutdown(&e, Duration::from_secs(5)));

    // h2 alone: a client offering only http/1.1 is refused in the handshake
    // (no_application_protocol), as by Go's server; h2 is served.
    let (e, addr) = serve(&config(&p, r#""h2""#, ""));
    let r = get_h1(
        addr,
        &client(&p.ca, &["http/1.1"], None),
        "a.example",
        "a.example",
        "/hello",
    );
    assert!(r.is_err(), "{r:?}");
    assert_eq!(
        get_h2(addr, &client(&p.ca, &["h2"], None), "a.example", "/hello"),
        Ok((200, "hello over tls\n".into()))
    );
    assert!(akit_listener::shutdown(&e, Duration::from_secs(5)));
}

#[test]
fn sni_picks_the_certificate() {
    let p = pki("listener-sni");
    let (e, addr) = serve(&config(&p, "", ""));
    let c = client(&p.ca, &["http/1.1"], None);
    let (a, b) = (leaf_der(&p.a.0), leaf_der(&p.b.0));
    for (name, want) in [
        ("a.example", &a),
        ("b.example", &b),
        ("B.Example", &b),
        // By the first certificate's wildcard.
        ("x.a.example", &a),
    ] {
        let seen = get_h1(addr, &c, name, name, "/hello").unwrap();
        assert!(&seen.leaf == want, "{name}");
    }
    // A name no certificate holds gets the first (and fails the client's check of the name).
    let r = get_h1(addr, &c, "c.example", "c.example", "/hello");
    assert!(r.is_err(), "{r:?}");
    assert!(akit_listener::shutdown(&e, Duration::from_secs(5)));
}

#[test]
fn client_auth_require_and_verify_if_given() {
    let p = pki("listener-mtls");
    let ca = s(&p.ca);
    let get = |addr, cert: Option<&(PathBuf, PathBuf)>| {
        let c = client(
            &p.ca,
            &["http/1.1"],
            cert.map(|(c, k)| (c.as_path(), k.as_path())),
        );
        get_h1(addr, &c, "a.example", "a.example", "/hello").map(|s| s.status)
    };
    let ok = Ok("HTTP/1.1 200 OK".to_owned());

    let (e, addr) = serve(&config(
        &p,
        "",
        &format!(r#", "client_auth": {{"ca_files": ["{ca}"]}}"#),
    ));
    assert_eq!(get(addr, Some(&p.client)).map_err(|e| e.to_string()), ok);
    assert!(get(addr, None).is_err(), "require: no certificate");
    assert!(get(addr, Some(&p.stranger)).is_err(), "another CA's");
    assert!(akit_listener::shutdown(&e, Duration::from_secs(5)));

    let (e, addr) = serve(&config(
        &p,
        "",
        &format!(r#", "client_auth": {{"ca_files": ["{ca}"], "mode": "verify_if_given"}}"#),
    ));
    assert_eq!(get(addr, Some(&p.client)).map_err(|e| e.to_string()), ok);
    assert_eq!(get(addr, None).map_err(|e| e.to_string()), ok);
    assert!(get(addr, Some(&p.stranger)).is_err(), "another CA's");
    assert!(akit_listener::shutdown(&e, Duration::from_secs(5)));
}

#[test]
fn a_client_that_leaves_mid_handshake_does_not_disturb_others() {
    use std::io::Write;
    let p = pki("listener-cut");
    let (e, addr) = serve(&config(&p, "", ""));
    // Part of a ClientHello, then gone; and one that connects and says nothing.
    let mut cut = std::net::TcpStream::connect(addr).unwrap();
    cut.write_all(&[0x16, 0x03, 0x01, 0x02, 0x00, 0x01])
        .unwrap();
    drop(cut);
    let _silent = std::net::TcpStream::connect(addr).unwrap();
    // Not TLS at all: refused, the listener goes on.
    let mut plain = std::net::TcpStream::connect(addr).unwrap();
    plain
        .write_all(b"GET /hello HTTP/1.1\r\nHost: a\r\n\r\n")
        .unwrap();
    let seen = get_h1(
        addr,
        &client(&p.ca, &["http/1.1"], None),
        "a.example",
        "a.example",
        "/hello",
    )
    .unwrap();
    assert_eq!(seen.status, "HTTP/1.1 200 OK");
    drop(_silent);
    assert!(akit_listener::shutdown(&e, Duration::from_secs(5)));
}
