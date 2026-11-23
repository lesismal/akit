//! The upstream pools on the listeners' engine (I14): their connections are dialed on its
//! threads, and an upstream's `connect_timeout` bounds a connect that is never answered. On the
//! backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::mpsc::channel;
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, UringConfig};
use akio_http::client::ClientRequest;
use akit_runtime::upstream::{EngineSlot, Error};
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

/// `akit run`'s order: the graph with its pools, then the engine from it, then the slot filled.
fn serve(upstream: &str) -> (Arc<akio::Engine>, GraphCell) {
    let json = format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
            "routes": [{{"handler": {{"module": "respond"}}}}],
            "upstreams": [{upstream}]}}"#
    );
    let config = akit_config::from_json(&json, None).unwrap();
    let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
    let slot = EngineSlot::new();
    let cell = GraphCell::new(compile(&config, &registry, &Env::new(slot.clone())).unwrap());
    let e = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
    e.start().unwrap();
    assert!(slot.set(e.clone()));
    (e, cell)
}

#[test]
fn pools_dial_on_the_listeners_engine() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { return };
            thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let mut head = Vec::new();
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(buf.get(..n).unwrap()),
                    }
                }
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nup");
            });
        }
    });
    let (engine, cell) = serve(&format!(r#"{{"name": "app", "addresses": ["{addr}"]}}"#));
    let pool = cell.with(|g| g.upstream("app").unwrap().clone());
    let (tx, rx) = channel();
    pool.request(ClientRequest::get("/"), move |r| {
        let _ = tx.send(r);
    });
    let resp = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
    assert_eq!((resp.status, resp.body.as_slice()), (200, &b"up"[..]));
    assert!(akit_listener::shutdown(&engine, Duration::from_secs(5)));
}

/// An address where connection attempts go unanswered, and what keeps it so; `None` where this
/// host has none. A listener that never accepts, its accept queue filled, drops further SYNs on
/// Linux; failing that, a blackhole route.
fn unanswered() -> Option<(SocketAddr, Vec<TcpStream>, Option<TcpListener>)> {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let mut held = Vec::new();
    for _ in 0..1024 {
        match TcpStream::connect_timeout(&addr, Duration::from_millis(200)) {
            Ok(s) => held.push(s),
            Err(e) if e.kind() == ErrorKind::TimedOut => return Some((addr, held, Some(l))),
            Err(_) => break,
        }
    }
    drop(held);
    let target: SocketAddr = "10.255.255.1:81".parse().unwrap();
    match TcpStream::connect_timeout(&target, Duration::from_millis(300)) {
        Err(e) if e.kind() == ErrorKind::TimedOut => Some((target, Vec::new(), None)),
        _ => None,
    }
}

#[test]
fn connect_timeout() {
    let Some((target, _held, _listener)) = unanswered() else {
        eprintln!("skipped: no address here leaves connection attempts unanswered");
        return;
    };
    let (engine, cell) = serve(&format!(
        r#"{{"name": "app", "addresses": ["{target}"], "connect_timeout": "300ms"}}"#
    ));
    let pool = cell.with(|g| g.upstream("app").unwrap().clone());
    let (tx, rx) = channel();
    let start = Instant::now();
    pool.request(ClientRequest::get("/"), move |r| {
        let _ = tx.send(r);
    });
    let r = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let took = start.elapsed();
    let e = r.unwrap_err();
    assert!(
        matches!(e, Error::Client(_)) && e.is_upstream_failure(),
        "{e}"
    );
    // Its connect_timeout, not akio's 10s default.
    assert!(took >= Duration::from_millis(250), "took {took:?}");
    assert!(took < Duration::from_secs(5), "took {took:?}");
    drop(engine);
}
