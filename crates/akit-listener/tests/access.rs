//! Access lists on listeners and routes (I85; fit's `listener/access_test.go`): a refused client
//! is closed as it is accepted when the listener's own list and the global one are all there is
//! (akio's accept filter, A3), and answered 403 when a route of the listener has a list of its
//! own, or the listener takes PROXY headers; a reload changes the policy on the same binding.
//! HTTP/3's peers are refused at akio's filter on its UDP socket (ADR-178, A220), its requests
//! one by one when a route has a list of its own. TCP on the backend `AKIO_TEST_BACKEND` names
//! (epoll by default, `uring`).
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
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use akio::{Backend, UringConfig};
use akit_listener::Listeners;
use akit_runtime::{Env, Graph, GraphCell, HandlerParams, Registry, compile};

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

/// Serves `json`: the engine started, its listeners.
fn serve(json: &str) -> (Arc<akio::Engine>, Listeners) {
    let cell = GraphCell::new(graph(json));
    let e = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
    e.start().unwrap();
    let l = Listeners::new(Arc::clone(&e), cell, &options());
    (e, l)
}

fn stop(e: &akio::Engine, l: Listeners) {
    drop(l);
    assert!(akit_listener::shutdown(e, Duration::from_secs(5)));
}

fn addr(l: &Listeners) -> SocketAddr {
    l.served()[0].address
}

/// What a request on a connection of its own gets: `None` when the connection is closed with no
/// answer, as one refused at accept is; else the status.
fn send(addr: SocketAddr, request: &str) -> Option<u16> {
    let mut s = TcpStream::connect(addr).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    // A connection closed at accept may refuse the write, or take it and answer nothing.
    let _ = s.write_all(request.as_bytes());
    let mut resp = Vec::new();
    let _ = s.read_to_end(&mut resp);
    let resp = String::from_utf8_lossy(&resp);
    let line = resp.lines().next()?;
    line.split(' ').nth(1)?.parse().ok()
}

fn get(addr: SocketAddr, path: &str) -> Option<u16> {
    send(
        addr,
        &format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"),
    )
}

/// fit's `accessDoc`: one listener, a route `/open` and a route for the rest, with the given
/// accesses (JSON objects, or empty for none).
fn doc(global: &str, listener: &str, route: &str) -> String {
    let field = |v: &str| {
        if v.is_empty() {
            String::new()
        } else {
            format!(r#", "access": {v}"#)
        }
    };
    format!(
        r#"{{"schema_version": 1{}, "listeners": [{{"name": "web", "address": "127.0.0.1:0"{}}}],
  "routes": [
    {{"match": {{"path": ["/open"]}}, "handler": {{"module": "respond", "params": {{"body": "open"}}}}{}}},
    {{"handler": {{"module": "respond", "params": {{"body": "hello"}}}}}}
  ]}}"#,
        field(global),
        field(listener),
        field(route)
    )
}

#[test]
fn refused_at_accept() {
    for (name, global, listener, refused) in [
        ("none", "", "", false),
        ("global deny", r#"{"deny": ["127.0.0.1"]}"#, "", true),
        (
            "global allow elsewhere",
            r#"{"allow": ["10.0.0.0/8"]}"#,
            "",
            true,
        ),
        ("global allow", r#"{"allow": ["127.0.0.0/8"]}"#, "", false),
        (
            "listener overrides global deny",
            r#"{"deny": ["127.0.0.0/8"]}"#,
            r#"{"allow": ["127.0.0.1"]}"#,
            false,
        ),
        (
            "listener deny overrides global allow",
            r#"{"allow": ["127.0.0.0/8"]}"#,
            r#"{"deny": ["127.0.0.1"]}"#,
            true,
        ),
        (
            "listener no say, global decides",
            r#"{"deny": ["127.0.0.1"]}"#,
            r#"{"deny": ["10.0.0.1"]}"#,
            true,
        ),
    ] {
        let (e, l) = serve(&doc(global, listener, ""));
        let got = get(addr(&l), "/");
        assert_eq!(got.is_none(), refused, "{name}: {got:?}");
        // The listener keeps accepting what it serves after refusing.
        if refused {
            assert_eq!(get(addr(&l), "/"), None, "{name}, again");
        } else {
            assert_eq!(got, Some(200), "{name}");
        }
        // Refused at accept: no route's handler ran, no request was read.
        stop(&e, l);
    }
}

#[test]
fn route_decides_for_its_requests() {
    // The route's own access decides for its requests; the others go by the listener's and the
    // global, checked per request now.
    let (e, l) = serve(&doc(
        r#"{"deny": ["127.0.0.0/8"]}"#,
        "",
        r#"{"allow": ["127.0.0.1"]}"#,
    ));
    assert_eq!(get(addr(&l), "/open"), Some(200));
    assert_eq!(get(addr(&l), "/other"), Some(403));
    // A path with no canonical form is refused before it is looked at, as fit's.
    assert_eq!(get(addr(&l), "/../x"), Some(403));
    stop(&e, l);

    let (e, l) = serve(&doc(
        r#"{"allow": ["127.0.0.0/8"]}"#,
        "",
        r#"{"deny": ["127.0.0.1"]}"#,
    ));
    assert_eq!(get(addr(&l), "/open"), Some(403), "denied by its route");
    assert_eq!(get(addr(&l), "/other"), Some(200));
    assert_eq!(get(addr(&l), "/../x"), Some(400));
    // A body the refused request carries is not waited for; the answer is fit's.
    let mut s = TcpStream::connect(addr(&l)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "POST /open HTTP/1.1\r\nHost: x\r\nContent-Length: 1000000\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut head = [0u8; 256];
    let n = s.read(&mut head).unwrap();
    let head = String::from_utf8_lossy(&head[..n]).into_owned();
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    assert!(head.contains("text/plain"), "{head}");
    drop(s);
    let mut s = TcpStream::connect(addr(&l)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "GET /open HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    assert!(resp.ends_with("\r\n\r\n403 forbidden\n"), "{resp}");
    stop(&e, l);
}

#[test]
fn follows_reload() {
    let (e, mut l) = serve(&doc("", "", ""));
    let a = addr(&l);
    assert_eq!(get(a, "/"), Some(200), "refused before any access");
    let mut reload = |json: &str| {
        let r = l.reload(graph(json)).unwrap();
        assert_eq!(r.kept, 1, "the reload bound the listener anew");
        assert_eq!(addr(&l), a);
    };
    reload(&doc(r#"{"deny": ["127.0.0.1"]}"#, "", ""));
    assert_eq!(get(a, "/"), None, "served after a reload denied it");
    // A route of its own: decided per request now, so the accept serves it.
    reload(&doc(
        r#"{"deny": ["127.0.0.1"]}"#,
        "",
        r#"{"allow": ["127.0.0.1"]}"#,
    ));
    assert_eq!(get(a, "/open"), Some(200));
    assert_eq!(get(a, "/"), Some(403));
    // The listener's own, at accept again.
    reload(&doc("", r#"{"allow": ["10.0.0.0/8"]}"#, ""));
    assert_eq!(get(a, "/open"), None);
    reload(&doc("", "", ""));
    assert_eq!(get(a, "/"), Some(200), "after access is gone");
    stop(&e, l);
}

#[test]
fn proxy_protocol_names_the_address() {
    let (e, l) = serve(
        r#"{
  "access": {"deny": ["192.0.2.1"]},
  "listeners": [{"name": "web", "address": "127.0.0.1:0", "proxy_protocol": {"trusted": ["127.0.0.1"]}}],
  "routes": [{"handler": {"module": "respond", "params": {"body": "hello"}}}]}"#,
    );
    for (client, want) in [("192.0.2.1", 403), ("192.0.2.2", 200)] {
        let got = send(
            addr(&l),
            &format!(
                "PROXY TCP4 {client} 127.0.0.1 40000 80\r\nGET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
            ),
        );
        assert_eq!(got, Some(want), "{client}");
    }
    stop(&e, l);
}

/// List files: read when the config is compiled, their sums kept with the certificates' for
/// `-watch-certs`, and a file that cannot be read failing the config.
#[test]
fn list_files() {
    let d = support::certs::dir("access-files");
    let deny = d.join("deny.txt");
    std::fs::write(&deny, "# ours\n127.0.0.0/8 ; loopback\n").unwrap();
    let json = doc(
        &format!(r#"{{"deny_files": [{:?}]}}"#, deny.display().to_string()),
        "",
        "",
    );
    let g = graph(&json);
    assert!(
        g.cert_files().contains_key(&deny.display().to_string()),
        "the list file is not watched"
    );
    drop(g);
    let (e, l) = serve(&json);
    assert_eq!(get(addr(&l), "/"), None);
    stop(&e, l);

    let config = akit_config::from_json(
        &doc(r#"{"allow_files": ["/nonexistent/allow.txt"]}"#, "", ""),
        None,
    )
    .unwrap();
    let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
    let err = compile(&config, &registry, &Env::default()).unwrap_err();
    let err = err.to_string();
    assert!(err.contains("access.allow_files[0]"), "{err}");
    let _ = std::fs::remove_dir_all(&d);
}

/// HTTP/3's peers refused at akio's filter (ADR-178, A220), before a QUIC connection is made:
/// a refused client's handshake goes unanswered, as its TCP side's connection is closed at
/// accept; with a route that has a list of its own, its requests are decided one by one (403).
/// A reload swaps the filter of the UDP socket it keeps, and one a reload binds takes it from its
/// first datagram.
#[test]
fn h3_peers_are_filtered() {
    use support::certs::{Ca, dir};
    let d = dir("access-h3");
    let ca = Ca::new("akit test ca");
    let ca_file = ca.write(&d, "ca.pem");
    let (cert, key) = ca.leaf(&d, "a", &["a.example"]);
    let s = |p: &Path| p.display().to_string();
    let json = |address: &str, access: &str, route: &str| {
        format!(
            r#"{{"access": {access},
  "listeners": [{{"name": "secure", "address": "{address}", "protocols": ["http/1.1", "h3"],
    "tls": {{"certificates": [{{"cert_file": "{}", "key_file": "{}"}}]}}}}],
  "routes": [{{"handler": {{"module": "respond", "params": {{"body": "hello"}}}}{route}}}]}}"#,
            s(&cert),
            s(&key)
        )
    };
    let filtered = |e: &akio::Engine| e.telemetry().metrics().total.filtered;
    // What an HTTP/3 GET of `/` from 127.0.0.1 gets: its status, `None` with no answer.
    let h3 = |udp: SocketAddr| {
        support::h3::get(udp, support::h3::client(&ca_file, None), "a.example", "/")
            .ok()
            .map(|seen| seen.status)
    };
    let deny = r#"{"deny": ["127.0.0.1"]}"#;
    let allow = r#"{"allow": ["127.0.0.1"]}"#;
    let own = r#", "access": {"allow": ["127.0.0.1"]}"#;
    let own_deny = r#", "access": {"deny": ["127.0.0.1"]}"#;
    for (access, route, want) in [
        (deny, "", None),
        (allow, "", Some(200)),
        // A route of its own: the peer is served, the request decided.
        (deny, own, Some(200)),
        (allow, own_deny, Some(403)),
    ] {
        let (e, l) = serve(&json("127.0.0.1:0", access, route));
        let config = akit_config::from_json(&json("127.0.0.1:0", access, route), None).unwrap();
        let a = akit_listener::addrs(&config.listeners, &e)[0];
        let before = filtered(&e);
        assert_eq!(h3(a.udp.unwrap()), want, "{access} {route}");
        if want.is_none() {
            assert!(filtered(&e) > before, "{access}: no datagram was filtered");
            // Its TCP side refuses at accept.
            assert_eq!(get(a.tcp.unwrap(), "/"), None);
        }
        stop(&e, l);
    }

    // A reload swaps the kept socket's filter.
    let (e, mut l) = serve(&json("127.0.0.1:0", "{}", ""));
    let config = akit_config::from_json(&json("127.0.0.1:0", "{}", ""), None).unwrap();
    let a = akit_listener::addrs(&config.listeners, &e)[0].udp.unwrap();
    assert_eq!(h3(a), Some(200));
    let r = l.reload(graph(&json("127.0.0.1:0", deny, ""))).unwrap();
    assert_eq!(r.kept, 1, "the reload bound the listener anew");
    assert_eq!(h3(a), None, "served after a reload denied it");
    let r = l.reload(graph(&json("127.0.0.1:0", allow, ""))).unwrap();
    assert_eq!(r.kept, 1);
    assert_eq!(h3(a), Some(200), "refused after a reload allowed it");
    // A listener a reload binds (another address) refuses from its first datagram.
    let other = format!("127.0.0.1:{}", free_udp_port());
    let r = l.reload(graph(&json(&other, deny, ""))).unwrap();
    assert_eq!(r.added.len(), 1);
    let moved: SocketAddr = other.parse().unwrap();
    let before = filtered(&e);
    assert_eq!(
        h3(moved),
        None,
        "a socket bound by a reload served a denied peer"
    );
    assert!(filtered(&e) > before, "no datagram was filtered");
    stop(&e, l);
    let _ = std::fs::remove_dir_all(&d);
}

/// A port free for both TCP and UDP on 127.0.0.1, for a listener a reload adds.
fn free_udp_port() -> u16 {
    loop {
        let t = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = t.local_addr().unwrap().port();
        if std::net::UdpSocket::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
}
