//! Access lists on layer 4 services and the forward proxy (I86; fit's `listener/access_test.go`
//! `TestAccessService`): a `tcp` listener of one service decides at accept by the service's
//! policy; one that tells protocols apart, or whose services pick by server name, decides once it
//! knows the service; a `udp` listener's refused clients' datagrams are dropped at akio's filter
//! (ADR-178, A220); a listener's `socks` checks its clients at their request when the listener decides later. On
//! the backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation
)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

use akio::{Backend, UringConfig};
use akit_listener::Listeners;
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};

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

/// `json` served, the slot filled so that services dial: the engine, its listeners and the
/// address of the first.
fn serve(json: &str) -> (Arc<akio::Engine>, Listeners, SocketAddr) {
    let (e, l, addr, _) = serve_slot(json);
    (e, l, addr)
}

fn stop(e: &akio::Engine, l: Listeners) {
    drop(l);
    assert!(akit_listener::shutdown(e, Duration::from_secs(5)));
}

/// A TCP echo server, on threads of its own, for as long as the test runs.
fn tcp_echo() -> SocketAddr {
    let up = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = up.local_addr().unwrap();
    std::thread::spawn(move || {
        for c in up.incoming() {
            let Ok(mut c) = c else { return };
            std::thread::spawn(move || {
                let mut r = c.try_clone().unwrap();
                let _ = std::io::copy(&mut r, &mut c);
            });
        }
    });
    addr
}

/// Whether `first` sent on a new connection to `addr` comes back.
fn echoes_with(addr: SocketAddr, first: &[u8]) -> bool {
    let Ok(mut c) = TcpStream::connect(addr) else {
        return false;
    };
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    if c.write_all(first).is_err() {
        return false;
    }
    let mut buf = vec![0u8; first.len()];
    c.read_exact(&mut buf).is_ok() && buf == first
}

fn echoes(addr: SocketAddr) -> bool {
    echoes_with(addr, b"ping")
}

/// The status of an HTTP/1.1 GET of `/` on a connection of its own; `None` when it is closed
/// with no answer.
fn status(addr: SocketAddr) -> Option<u16> {
    let mut s = TcpStream::connect(addr).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let _ = s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    let mut resp = Vec::new();
    let _ = s.read_to_end(&mut resp);
    let resp = String::from_utf8_lossy(&resp);
    resp.lines().next()?.split(' ').nth(1)?.parse().ok()
}

/// fit's `TestAccessService`: a `tcp` listener, alone or beside HTTP, with the global access and
/// the service's own.
#[test]
fn services() {
    let up = tcp_echo();
    let doc = |global: &str, service: &str, protocols: &str| {
        format!(
            r#"{{"access": {global},
  "listeners": [{{"name": "echo", "address": "127.0.0.1:0", "protocols": [{protocols}]}}],
  "upstreams": [{{"name": "echo", "addresses": ["{up}"]}}],
  "routes": [{{"handler": {{"module": "respond", "params": {{"body": "hello"}}}}}}],
  "services": [{{"name": "echo", "listeners": ["echo"], "upstream": "echo"{service}}}]}}"#
        )
    };
    const TCP: &str = r#""tcp""#;
    const MIXED: &str = r#""http/1.1", "tcp""#;
    for (name, global, service, protocols, echo, http) in [
        (
            "global deny",
            r#"{"deny": ["127.0.0.1"]}"#,
            "",
            TCP,
            false,
            None,
        ),
        (
            "service overrides",
            r#"{"deny": ["127.0.0.1"]}"#,
            r#", "access": {"allow": ["127.0.0.1"]}"#,
            TCP,
            true,
            None,
        ),
        (
            "service deny",
            "{}",
            r#", "access": {"deny": ["127.0.0.0/8"]}"#,
            TCP,
            false,
            None,
        ),
        // A listener that tells protocols apart decides after it has.
        (
            "mixed service overrides",
            r#"{"deny": ["127.0.0.1"]}"#,
            r#", "access": {"allow": ["127.0.0.1"]}"#,
            MIXED,
            true,
            Some(403),
        ),
        (
            "mixed service deny",
            "{}",
            r#", "access": {"deny": ["127.0.0.1"]}"#,
            MIXED,
            false,
            Some(200),
        ),
        (
            "mixed global deny",
            r#"{"deny": ["127.0.0.1"]}"#,
            "",
            MIXED,
            false,
            None,
        ),
    ] {
        let (e, l, addr) = serve(&doc(global, service, protocols));
        assert_eq!(echoes(addr), echo, "{name}");
        if protocols == MIXED {
            assert_eq!(status(addr), http, "{name}: HTTP");
        }
        stop(&e, l);
    }
}

/// Services picked by server name decide once the ClientHello names theirs, and a listener
/// taking PROXY headers once the header names the client.
#[test]
fn services_by_name_and_proxy_headers() {
    let up = tcp_echo();
    let json = format!(
        r#"{{"access": {{"deny": ["127.0.0.1"]}},
  "listeners": [{{"name": "tls", "address": "127.0.0.1:0", "protocols": ["tcp"]}}],
  "upstreams": [{{"name": "echo", "addresses": ["{up}"]}}],
  "services": [
    {{"name": "a", "listeners": ["tls"], "upstream": "echo", "server_names": ["a.example"], "access": {{"allow": ["127.0.0.1"]}}}},
    {{"name": "rest", "listeners": ["tls"], "upstream": "echo"}}]}}"#
    );
    let (e, l, addr) = serve(&json);
    // A ClientHello naming a.example goes to `a`, whose own list serves it; another to `rest`,
    // which goes by the global deny.
    assert!(echoes_with(addr, &client_hello("a.example")), "a");
    assert!(!echoes_with(addr, &client_hello("b.example")), "rest");
    assert!(!echoes(addr), "not TLS: rest");
    stop(&e, l);

    let json = format!(
        r#"{{"access": {{"deny": ["192.0.2.1"]}},
  "listeners": [{{"name": "pp", "address": "127.0.0.1:0", "protocols": ["tcp"], "proxy_protocol": {{"trusted": ["127.0.0.1"]}}}}],
  "upstreams": [{{"name": "echo", "addresses": ["{up}"]}}],
  "services": [{{"name": "echo", "listeners": ["pp"], "upstream": "echo"}}]}}"#
    );
    let (e, l, addr) = serve(&json);
    for (client, want) in [("192.0.2.1", false), ("192.0.2.2", true)] {
        let mut first = format!("PROXY TCP4 {client} 127.0.0.1 40000 80\r\n").into_bytes();
        let n = first.len();
        first.extend_from_slice(b"ping");
        let got = (|| {
            let mut c = TcpStream::connect(addr).ok()?;
            c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            c.write_all(&first).ok()?;
            let mut buf = [0u8; 4];
            c.read_exact(&mut buf).ok()?;
            Some(buf == *b"ping")
        })()
        .unwrap_or(false);
        assert_eq!(got, want, "{client} ({n} bytes of header)");
    }
    stop(&e, l);
}

/// A TLS 1.3 ClientHello asking for `name`, as short as akio-mux reads one: enough for the entry
/// to pick by it. The echo upstream sends it back unchanged.
fn client_hello(name: &str) -> Vec<u8> {
    let sni = {
        let n = name.as_bytes();
        let mut v = Vec::new();
        let list_len = 3 + n.len();
        v.extend_from_slice(&[0x00, 0x00]); // server_name
        v.extend_from_slice(&((list_len + 2) as u16).to_be_bytes());
        v.extend_from_slice(&(list_len as u16).to_be_bytes());
        v.push(0); // host_name
        v.extend_from_slice(&(n.len() as u16).to_be_bytes());
        v.extend_from_slice(n);
        v
    };
    let mut body = Vec::new();
    body.extend_from_slice(&[0x03, 0x03]);
    body.extend_from_slice(&[0u8; 32]);
    body.push(0); // session id
    body.extend_from_slice(&[0x00, 0x02, 0x13, 0x01]);
    body.extend_from_slice(&[0x01, 0x00]);
    body.extend_from_slice(&(sni.len() as u16).to_be_bytes());
    body.extend_from_slice(&sni);
    let mut hs = vec![0x01];
    hs.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
    hs.extend_from_slice(&body);
    let mut rec = vec![0x16, 0x03, 0x01];
    rec.extend_from_slice(&(hs.len() as u16).to_be_bytes());
    rec.extend_from_slice(&hs);
    rec
}

/// A `udp` listener refuses a client's datagrams at akio's filter (ADR-178, A220), by the policy
/// of the service that takes its sessions: they are dropped before a session is made, and get no
/// answer; a served client's do. A reload swaps the filter of the socket it keeps.
#[test]
fn udp_sessions() {
    let up = UdpSocket::bind("127.0.0.1:0").unwrap();
    let up_addr = up.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut buf = [0u8; 1500];
        while let Ok((n, from)) = up.recv_from(&mut buf) {
            let _ = up.send_to(&buf[..n], from);
        }
    });
    let json = |access: &str, service: &str| {
        format!(
            r#"{{"access": {access},
  "listeners": [{{"name": "dns", "address": "127.0.0.1:0", "protocols": ["udp"]}}],
  "upstreams": [{{"name": "dns", "addresses": ["{up_addr}"]}}],
  "services": [{{"name": "dns", "listeners": ["dns"], "upstream": "dns"{service}}}]}}"#
        )
    };
    let c = UdpSocket::bind("127.0.0.1:0").unwrap();
    c.set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let echoes = |addr: SocketAddr| {
        for _ in 0..10 {
            c.send_to(b"ping", addr).unwrap();
            let mut buf = [0u8; 16];
            if let Ok((n, _)) = c.recv_from(&mut buf) {
                return &buf[..n] == b"ping";
            }
        }
        false
    };
    let deny = r#"{"deny": ["127.0.0.1"]}"#;
    let allow = r#"{"allow": ["127.0.0.1"]}"#;
    for (access, service, want) in [
        (deny, "", false),
        (allow, "", true),
        // The service's own list decides before the global one.
        (deny, r#", "access": {"allow": ["127.0.0.1"]}"#, true),
        (allow, r#", "access": {"deny": ["127.0.0.1"]}"#, false),
    ] {
        let (e, l, addr) = serve(&json(access, service));
        assert_eq!(echoes(addr), want, "{access} {service}");
        let m = e.telemetry().metrics();
        if want {
            assert_eq!(m.total.filtered, 0, "{access} {service}");
        } else {
            // Every datagram dropped at the filter: no session was made for the client.
            assert!(m.total.filtered > 0, "{access} {service}: nothing filtered");
            assert_eq!(
                m.total.accepted, 0,
                "{access} {service}: a session was made"
            );
        }
        stop(&e, l);
    }

    // A reload keeping the listener swaps its socket's filter.
    let (e, mut l, addr, slot) = serve_slot(&json("{}", ""));
    assert!(echoes(addr));
    let r = l.reload(graph(&json(deny, ""), &slot)).unwrap();
    assert_eq!(r.kept, 1, "the reload bound the listener anew");
    // A new client: the one served above keeps its session, as an accepted connection is kept.
    let c2 = UdpSocket::bind("127.0.0.1:0").unwrap();
    c2.set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    c2.send_to(b"ping", addr).unwrap();
    let mut buf = [0u8; 16];
    assert!(
        c2.recv_from(&mut buf).is_err(),
        "served after a reload denied it"
    );
    let r = l.reload(graph(&json(allow, ""), &slot)).unwrap();
    assert_eq!(r.kept, 1);
    let mut got = false;
    for _ in 0..10 {
        c2.send_to(b"ping", addr).unwrap();
        if let Ok((n, _)) = c2.recv_from(&mut buf) {
            got = &buf[..n] == b"ping";
            break;
        }
    }
    assert!(got, "refused after a reload allowed it");
    stop(&e, l);
}

/// `json` compiled with `slot` as the upstreams' engine.
fn graph(json: &str, slot: &EngineSlot) -> akit_runtime::Graph {
    let config = akit_config::from_json(json, None).unwrap();
    let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
    compile(&config, &registry, &Env::new(slot.clone())).unwrap()
}

/// [`serve`], with the slot, for reloads.
fn serve_slot(json: &str) -> (Arc<akio::Engine>, Listeners, SocketAddr, EngineSlot) {
    let slot = EngineSlot::new();
    let cell = GraphCell::new(graph(json, &slot));
    let e = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
    e.start().unwrap();
    assert!(slot.set(Arc::clone(&e)));
    let l = Listeners::new(Arc::clone(&e), cell, &options());
    let addr = l.served()[0].address;
    (e, l, addr, slot)
}

/// A listener's `socks` beside HTTP, whose route has a list of its own: the listener decides
/// later, and SOCKS checks its clients at their request.
#[test]
fn socks_beside_http() {
    let up = tcp_echo();
    let json = |global: &str| {
        format!(
            r#"{{"access": {global},
  "listeners": [{{"name": "mix", "address": "127.0.0.1:0", "protocols": ["http/1.1", "socks"],
    "socks": {{"clients": ["127.0.0.1"], "allow": ["127.0.0.1"]}}}}],
  "routes": [{{"access": {{"allow": ["127.0.0.1"]}}, "handler": {{"module": "respond", "params": {{"body": "hello"}}}}}}]}}"#
        )
    };
    for (global, want) in [(r#"{"deny": ["127.0.0.1"]}"#, false), ("{}", true)] {
        let (e, l, addr) = serve(&json(global));
        // The route's own list serves HTTP whatever the global one says.
        assert_eq!(status(addr), Some(200), "{global}: HTTP");
        assert_eq!(socks_echoes(addr, up), want, "{global}: SOCKS");
        stop(&e, l);
    }
}

/// Whether a SOCKS5 CONNECT through `proxy` to `to` is granted and echoes.
fn socks_echoes(proxy: SocketAddr, to: SocketAddr) -> bool {
    let Ok(mut c) = TcpStream::connect(proxy) else {
        return false;
    };
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut reply = [0u8; 2];
    if c.write_all(&[5, 1, 0]).is_err() || c.read_exact(&mut reply).is_err() {
        return false;
    }
    let SocketAddr::V4(v4) = to else { return false };
    let mut req = vec![5, 1, 0, 1];
    req.extend_from_slice(&v4.ip().octets());
    req.extend_from_slice(&v4.port().to_be_bytes());
    let mut head = [0u8; 10];
    if c.write_all(&req).is_err() || c.read_exact(&mut head).is_err() || head[1] != 0 {
        return false;
    }
    echoes_on(&mut c)
}

fn echoes_on(c: &mut TcpStream) -> bool {
    let mut buf = [0u8; 4];
    c.write_all(b"ping").is_ok() && c.read_exact(&mut buf).is_ok() && buf == *b"ping"
}
