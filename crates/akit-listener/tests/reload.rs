//! Reloading (I22): `Listeners::reload` keeps the unchanged listeners, adds the new ones to the
//! engine, drains the removed ones (akio's runtime listeners, A1), those akit started with as the
//! rest, and refuses what it cannot do with nothing changed. On the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
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
    let l = Listeners::new(Arc::clone(&e), cell.clone(), &options());
    (cell, e, l)
}

fn stop(e: &akio::Engine, l: Listeners) {
    drop(l);
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

    // The added listener changed at the same address: replaced, with the new settings.
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

    // Removed: it stops listening, and an idle connection it accepted is drained (closed).
    let mut open = TcpStream::connect(extra).unwrap();
    open.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    open.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut buf = [0u8; 512];
    assert!(open.read(&mut buf).unwrap() > 0);
    let r = l.reload(graph(&config(WEB, "v4"))).unwrap();
    assert_eq!((r.kept, r.added.len(), r.removed.len()), (1, 0, 1));
    assert_eq!(l.served().len(), 1);
    assert_eq!(get(web), ok("v4"));
    assert_eq!(open.read(&mut buf).unwrap(), 0);
    wait_closed(extra);
    // The address can be listened on again.
    drop(TcpListener::bind(extra).unwrap());
    cell.collect();
    assert_eq!(cell.retiring(), 0);
    stop(&e, l);
}

/// Waits until nothing listens at `addr` any more (each I/O thread closes its socket at its
/// next loop iteration).
fn wait_closed(addr: SocketAddr) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while TcpStream::connect(addr).is_ok() {
        assert!(std::time::Instant::now() < deadline, "{addr} still listens");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn the_listeners_akit_started_with_change_and_go() {
    let (cell, e, mut l) = serve(&config(WEB, "v1"));
    let web = l.served()[0].address;

    // Changed at its address (h2c added): replaced, the address served throughout.
    let r = l
        .reload(graph(&config(
            &format!(r#"{{"name": "web", "address": "{web}", "protocols": ["http/1.1", "h2c"]}}"#),
            "v2",
        )))
        .unwrap();
    assert_eq!((r.kept, r.added.len(), r.removed.len()), (0, 1, 1));
    assert_eq!((r.added[0].name.as_str(), r.added[0].address), ("web", web));
    assert_eq!(r.removed[0].address, web);
    assert_eq!(get(web), ok("v2"));
    cell.collect();

    // Removed, another listener in its place.
    let r = l
        .reload(graph(&config(
            r#"{"name": "other", "address": "127.0.0.1:0"}"#,
            "v3",
        )))
        .unwrap();
    assert_eq!((r.kept, r.added.len(), r.removed.len()), (0, 1, 1));
    let other = r.added[0].address;
    assert_eq!(get(other), ok("v3"));
    wait_closed(web);
    assert_eq!(l.served().len(), 1);
    cell.collect();
    assert_eq!(cell.retiring(), 0);
    stop(&e, l);
}

#[test]
fn what_cannot_be_served_is_refused_with_nothing_changed() {
    let (cell, e, mut l) = serve(&config(WEB, "v1"));
    let web = l.served()[0].address;
    let epoch = cell.epoch();

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
            assert!(msg.contains(&busy_addr.to_string()), "{msg}");
        }
        other => panic!("{other:?}"),
    }
    wait_closed(free);
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

/// `Listeners::plan` (I27's dry runs): what a reload would open and close, with nothing changed;
/// a new listener's taken address is refused as a reload refuses it, one a running listener holds
/// is not tried.
#[test]
fn a_plan_changes_nothing() {
    let old = r#"{"name": "old", "address": "127.0.0.1:0", "protocols": ["http/1.1"]}"#;
    let (cell, e, l) = serve(&config(&format!("{WEB}, {old}"), "v1"));
    let served = l.served();
    let before = e.listeners().len();

    let r = l
        .plan(&graph(&config(
            &format!(r#"{WEB}, {{"name": "new", "address": "127.0.0.1:0"}}"#),
            "v2",
        )))
        .unwrap();
    assert_eq!(r.kept, 1);
    assert_eq!(r.added.len(), 1);
    assert_eq!(
        (r.added[0].name.as_str(), r.added[0].address.to_string()),
        ("new", "127.0.0.1:0".to_owned())
    );
    assert_eq!(r.removed.len(), 1);
    assert_eq!(r.removed[0], served[1]);
    assert_eq!((l.served(), e.listeners().len()), (served.clone(), before));
    assert_eq!(get(served[0].address), ok("v1"));

    // A taken address, refused as reload refuses it.
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = taken.local_addr().unwrap();
    let clash = config(
        &format!(r#"{WEB}, {{"name": "clash", "address": "{addr}"}}"#),
        "v2",
    );
    let planned = l.plan(&graph(&clash)).unwrap_err();
    assert!(matches!(planned, ReloadError::Listener(_)), "{planned}");
    assert!(planned.to_string().contains("listener clash"), "{planned}");
    drop(taken);
    // The running listener's own address, changed: not tried, since a reload makes room on it.
    let moved = config(
        &format!(
            r#"{WEB}, {{"name": "old", "address": "{}"}}"#,
            served[1].address
        ),
        "v2",
    );
    let r = l.plan(&graph(&moved)).unwrap();
    assert_eq!((r.added.len(), r.removed.len()), (1, 1));
    assert_eq!(cell.retiring(), 0);
    stop(&e, l);
}
