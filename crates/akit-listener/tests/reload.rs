//! Reloading (I22): `Listeners::reload` keeps the unchanged listeners, opens the new ones,
//! closes the removed ones, and refuses what it cannot do with nothing changed. On the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use akio::{Backend, UringConfig};
use akit_listener::{Listeners, ReloadError};
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

/// Listeners from `listeners` (JSON), each answering `/` with `body` on every listener.
fn config(listeners: &str, body: &str) -> String {
    format!(
        r#"{{"listeners": [{listeners}],
            "routes": [{{"handler": {{"module": "respond", "params": {{"body": "{body}"}}}}}}]}}"#
    )
}

const WEB: &str = r#"{"name": "web", "address": "127.0.0.1:0"}"#;

/// Serves `json`: the engine started, its listeners.
fn serve(json: &str) -> (GraphCell, Arc<akio::Engine>, Listeners) {
    let cell = GraphCell::new(graph(json));
    let e = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
    e.start().unwrap();
    let l = Listeners::new(Arc::clone(&e), cell.clone());
    (cell, e, l)
}

fn stop(e: &akio::Engine, mut l: Listeners) {
    l.close();
    assert!(akit_listener::shutdown(e, Duration::from_secs(5)));
}

/// An HTTP/1.1 GET of `/`, with `Connection: close`; the status line and the body.
fn get(addr: SocketAddr) -> (String, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(s, "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    let (head, body) = resp.split_once("\r\n\r\n").unwrap();
    (head.lines().next().unwrap().to_owned(), body.to_owned())
}

fn ok(body: &str) -> (String, String) {
    ("HTTP/1.1 200 OK".into(), body.into())
}

#[test]
fn unchanged_listeners_are_kept_and_new_ones_opened_and_closed() {
    let (cell, e, mut l) = serve(&config(WEB, "v1"));
    let web = l.served()[0].address;
    assert_eq!(get(web), ok("v1"));

    // The routes change, a listener is added.
    let r = l
        .reload(graph(&config(
            &format!(r#"{WEB}, {{"name": "extra", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}}"#),
            "v2",
        )))
        .unwrap();
    assert_eq!(r.kept, 1);
    assert!(r.removed.is_empty());
    assert_eq!(r.added.len(), 1);
    assert_eq!(r.added[0].name, "extra");
    let extra = r.added[0].address;
    assert_eq!(l.served().len(), 2);
    assert_eq!(get(web), ok("v2"));
    assert_eq!(get(extra), ok("v2"));
    // The old graph is retired once no request uses it.
    assert_eq!(cell.retiring(), 1);
    cell.collect();
    assert_eq!(cell.retiring(), 0);

    // The added listener changed at the same address: it keeps its socket, with the new
    // settings.
    let r = l
        .reload(graph(&config(
            &format!(r#"{WEB}, {{"name": "extra2", "address": "{extra}"}}"#),
            "v3",
        )))
        .unwrap();
    assert_eq!((r.kept, r.added.len(), r.removed.len()), (1, 1, 1));
    assert_eq!(r.removed[0].name, "extra");
    assert_eq!(
        (r.added[0].name.as_str(), r.added[0].address),
        ("extra2", extra)
    );
    assert_eq!(get(extra), ok("v3"));

    // Removed: its socket closed, a connection it accepted kept until it ends.
    let mut open = TcpStream::connect(extra).unwrap();
    open.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    open.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut buf = [0u8; 512];
    assert!(open.read(&mut buf).unwrap() > 0);
    let r = l.reload(graph(&config(WEB, "v4"))).unwrap();
    assert_eq!((r.kept, r.added.len(), r.removed.len()), (1, 0, 1));
    assert_eq!(l.served().len(), 1);
    assert!(TcpStream::connect(extra).is_err());
    assert_eq!(get(web), ok("v4"));
    // The address can be listened on again.
    drop(TcpListener::bind(extra).unwrap());
    drop(open);
    cell.collect();
    assert_eq!(cell.retiring(), 0);
    stop(&e, l);
}

#[test]
fn what_cannot_be_served_is_refused_with_nothing_changed() {
    let (cell, e, mut l) = serve(&config(WEB, "v1"));
    let web = l.served()[0].address;
    let epoch = cell.epoch();

    // A listener akit started with, removed or changed: needs A1.
    for listeners in [
        r#"{"name": "other", "address": "127.0.0.1:0"}"#.to_owned(),
        r#"{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}"#.to_owned(),
    ] {
        match l.reload(graph(&config(&listeners, "v2"))) {
            Err(ReloadError::Fixed { listener, address }) => {
                assert_eq!((listener.as_str(), address), ("web", web));
            }
            other => panic!("{other:?}"),
        }
    }

    // An address in use: the listeners bound before it are closed again.
    let busy = TcpListener::bind("127.0.0.1:0").unwrap();
    let busy_addr = busy.local_addr().unwrap();
    let free = {
        let s = TcpListener::bind("127.0.0.1:0").unwrap();
        s.local_addr().unwrap()
    };
    let r = l.reload(graph(&config(
        &format!(
            r#"{WEB}, {{"name": "a", "address": "{free}"}}, {{"name": "b", "address": "{busy_addr}"}}"#
        ),
        "v2",
    )));
    match r {
        Err(e @ ReloadError::Listener(_)) => {
            let msg = e.to_string();
            assert!(msg.starts_with("listener b: address"), "{msg}");
        }
        other => panic!("{other:?}"),
    }
    assert!(TcpStream::connect(free).is_err());
    drop(busy);

    // h2c alone is not supported yet.
    let r = l.reload(graph(&config(
        &format!(r#"{WEB}, {{"name": "c", "address": "127.0.0.1:0", "protocols": ["h2c"]}}"#),
        "v2",
    )));
    assert!(matches!(r, Err(ReloadError::Listener(_))), "{r:?}");

    assert_eq!(cell.epoch(), epoch);
    assert_eq!(cell.retiring(), 0);
    assert_eq!(l.served().len(), 1);
    assert_eq!(get(web), ok("v1"));
    stop(&e, l);
}
