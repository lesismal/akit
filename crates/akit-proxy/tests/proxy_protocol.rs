//! A listener of HTTP behind a load balancer (I52; fit's `l4.TestProxyProtocolHTTP` and
//! `TestProxyProtocolHTTPEdges`): the client a trusted PROXY header names is the request's, as
//! the upstream's `X-Forwarded-For` shows (akio API request A105). On the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use akio::{Backend, Engine, UringConfig};
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
    o.shutdown_timeout = Duration::from_secs(2);
    o
}

struct Akit {
    engine: Arc<Engine>,
    addr: SocketAddr,
    _cell: GraphCell,
}

impl Akit {
    fn start(json: &str) -> Akit {
        let config = akit_config::from_json(json, None).unwrap();
        let registry: Registry<HandlerParams> =
            Registry::new(&[akit_handler::MODULES, akit_proxy::MODULES]).unwrap();
        let slot = EngineSlot::new();
        let cell = GraphCell::new(compile(&config, &registry, &Env::new(slot.clone())).unwrap());
        let engine = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
        engine.start().unwrap();
        assert!(slot.set(engine.clone()));
        let addr = *engine.local_addrs().first().unwrap();
        Akit {
            engine,
            addr,
            _cell: cell,
        }
    }

    fn dial(&self) -> TcpStream {
        let c = TcpStream::connect_timeout(&self.addr, Duration::from_secs(5)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c
    }
}

impl Drop for Akit {
    fn drop(&mut self) {
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
    }
}

/// A TCP server serving each connection with `serve` on a thread of its own.
fn backend(serve: impl Fn(TcpStream) + Send + Sync + 'static) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let serve = Arc::new(serve);
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            let serve = Arc::clone(&serve);
            thread::spawn(move || serve(s));
        }
    });
    addr
}

/// Checks that akit closes `c` within 5 seconds: its read ends without data.
fn expect_closed(c: &mut TcpStream) {
    match c.read(&mut [0u8; 1]) {
        Ok(0) => {}
        Err(e) if !matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
        r => panic!("read {r:?}; want the connection closed"),
    }
}

/// A listener of HTTP takes PROXY headers: the client they name is the request's, as the
/// upstream's X-Forwarded-For shows, and an untrusted client's own address is (fit's
/// `TestProxyProtocolHTTP` and `TestProxyProtocolHTTPEdges`).
#[test]
fn proxy_protocol_http() {
    let app = backend(|s| {
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut r = std::io::BufReader::new(s.try_clone().unwrap());
        let mut w = s;
        loop {
            let mut xff = String::new();
            let mut line = String::new();
            loop {
                line.clear();
                if std::io::BufRead::read_line(&mut r, &mut line).unwrap_or(0) == 0 {
                    return;
                }
                if line == "\r\n" {
                    break;
                }
                if let Some((k, v)) = line.split_once(':') {
                    if k.eq_ignore_ascii_case("x-forwarded-for") {
                        xff = v.trim().to_owned();
                    }
                }
            }
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{xff}",
                xff.len()
            );
            if w.write_all(resp.as_bytes()).is_err() {
                return;
            }
        }
    });
    let doc = |trusted: &str| {
        format!(
            r#"{{
  "listeners": [{{"name": "web", "address": "127.0.0.1:0", "proxy_protocol": {{"trusted": ["{trusted}"]}}}}],
  "upstreams": [{{"name": "app", "addresses": [{app:?}]}}],
  "routes": [{{"handler": {{"module": "proxy", "params": {{"upstream": "app"}}}}}}]
}}"#
        )
    };
    let get = |a: &Akit, header: &[u8], split: bool| -> String {
        let mut c = a.dial();
        let wire = [
            header,
            b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
        ]
        .concat();
        if split {
            c.write_all(&wire[..10]).unwrap();
            thread::sleep(Duration::from_millis(20));
            c.write_all(&wire[10..]).unwrap();
        } else {
            c.write_all(&wire).unwrap();
        }
        let mut out = String::new();
        let _ = c.read_to_string(&mut out);
        match out.split_once("\r\n\r\n") {
            Some((head, body)) if head.starts_with("HTTP/1.1 200") => body.to_owned(),
            _ => panic!("response {out:?}"),
        }
    };
    let a = Akit::start(&doc("127.0.0.1"));
    for split in [false, true] {
        assert_eq!(
            get(
                &a,
                b"PROXY TCP4 203.0.113.9 198.51.100.1 5555 80\r\n",
                split
            ),
            "203.0.113.9"
        );
    }
    let v2 = b"\r\n\r\n\0\r\nQUIT\n\x21\x21\x00\x24\
        \x20\x01\x0d\xb8\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x09\
        \x20\x01\x0d\xb8\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x01\x15\xb3\x00\x50";
    assert_eq!(get(&a, v2, false), "2001:db8::9");
    // A header naming no one: the connection's own address.
    assert_eq!(get(&a, b"PROXY UNKNOWN\r\n", false), "127.0.0.1");
    // A bad header, and none at all, from a trusted address: closed.
    for wire in [
        &b"PROXY BAD HEADER\r\n"[..],
        b"GET / HTTP/1.1\r\nHost: x\r\n\r\n",
    ] {
        let mut c = a.dial();
        c.write_all(wire).unwrap();
        expect_closed(&mut c);
    }
    drop(a);
    let a = Akit::start(&doc("10.0.0.0/8"));
    assert_eq!(get(&a, b"", false), "127.0.0.1");
}
