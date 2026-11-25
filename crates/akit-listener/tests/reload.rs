//! Reloading (I22, I28; fit's `listener/reload_test.go`): `Listeners::reload` keeps the unchanged
//! listeners, their connections served the new graph, adds the new ones to the
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

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

/// A keep-alive HTTP/1.1 connection, its requests sent one at a time.
struct Keep(BufReader<TcpStream>);

impl Keep {
    fn new(addr: SocketAddr) -> Result<Keep, String> {
        let s = TcpStream::connect(addr).map_err(|e| format!("connect {addr}: {e}"))?;
        s.set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| e.to_string())?;
        Ok(Keep(BufReader::new(s)))
    }

    /// A GET of `/`: the status line and the body.
    fn get(&mut self) -> Result<(String, String), String> {
        let s = self.0.get_mut();
        write!(s, "GET / HTTP/1.1\r\nHost: x\r\n\r\n").map_err(|e| format!("write: {e}"))?;
        let mut status = String::new();
        self.0
            .read_line(&mut status)
            .map_err(|e| format!("read: {e}"))?;
        if status.is_empty() {
            return Err("closed".into());
        }
        let mut length = 0usize;
        loop {
            let mut line = String::new();
            self.0
                .read_line(&mut line)
                .map_err(|e| format!("read: {e}"))?;
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':') {
                if k.eq_ignore_ascii_case("content-length") {
                    length = v.trim().parse().map_err(|_| line.to_owned())?;
                }
            }
        }
        let mut body = vec![0u8; length];
        self.0
            .read_exact(&mut body)
            .map_err(|e| format!("read body: {e}"))?;
        Ok((
            status.trim_end().to_owned(),
            String::from_utf8_lossy(&body).into_owned(),
        ))
    }
}

#[test]
fn unchanged_listeners_are_kept_and_new_ones_opened_and_closed() {
    let (cell, e, mut l) = serve(&config(WEB, "v1"));
    let web = l.served()[0].address;
    assert_eq!(get(web), ok("v1"));
    // A keep-alive connection to web, open across the reloads.
    let mut keep = Keep::new(web).unwrap();
    assert_eq!(keep.get().unwrap(), ok("v1"));

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
    assert_eq!(keep.get().unwrap(), ok("v2"));
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
    assert_eq!(keep.get().unwrap(), ok("v4"));
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
    let deadline = Instant::now() + Duration::from_secs(5);
    while TcpStream::connect(addr).is_ok() {
        assert!(Instant::now() < deadline, "{addr} still listens");
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

    // h2c alone is not supported yet: the config reader refuses it (I25), so no graph with it
    // reaches a reload.
    let errs = akit_config::from_json(
        &config(
            &format!(r#"{WEB}, {{"name": "c", "address": "127.0.0.1:0", "protocols": ["h2c"]}}"#),
            "v2",
        ),
        None,
    )
    .unwrap_err();
    assert!(errs.only_not_supported_yet(), "{errs}");

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

/// I28's test: listeners added and removed again and again while clients use a listener every
/// reload keeps, over keep-alive connections opened before and during the reloads and over a new
/// connection per request: no request fails, each answers from a graph published by then, and
/// each listener added serves until it is removed.
#[test]
fn listeners_added_and_removed_under_traffic_fail_no_request() {
    let (cell, e, mut l) = serve(&config(WEB, "v0"));
    let web = l.served()[0].address;
    let stop_clients = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicUsize::new(0));
    let failures = Arc::new(Mutex::new(Vec::<String>::new()));
    let clients: Vec<_> = (0..4)
        .map(|n| {
            let (stop_clients, done, failures) = (
                Arc::clone(&stop_clients),
                Arc::clone(&done),
                Arc::clone(&failures),
            );
            std::thread::spawn(move || {
                let mut keep = None;
                let mut last = 0u32;
                while !stop_clients.load(Ordering::SeqCst) {
                    // Half the clients keep one connection; the others reconnect now and then.
                    let reuse = n % 2 == 0 || done.load(Ordering::SeqCst) % 7 != 0;
                    if !reuse || keep.is_none() {
                        keep = Some(Keep::new(web));
                    }
                    let r = match keep.as_mut() {
                        Some(Ok(k)) => k.get(),
                        Some(Err(e)) => Err(e.clone()),
                        None => Err("no connection".into()),
                    };
                    match r {
                        Ok((status, body)) if status == "HTTP/1.1 200 OK" => {
                            // The versions only go up.
                            match body.strip_prefix('v').and_then(|v| v.parse::<u32>().ok()) {
                                Some(v) if v >= last => last = v,
                                _ => failures
                                    .lock()
                                    .unwrap()
                                    .push(format!("{body} after v{last}")),
                            }
                        }
                        Ok(other) => failures.lock().unwrap().push(format!("{other:?}")),
                        Err(e) => {
                            failures.lock().unwrap().push(e);
                            keep = None;
                        }
                    }
                    done.fetch_add(1, Ordering::SeqCst);
                }
            })
        })
        .collect();
    let wait_for = |n: usize| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while done.load(Ordering::SeqCst) < n {
            assert!(Instant::now() < deadline, "the clients made no progress");
            std::thread::sleep(Duration::from_millis(2));
        }
    };
    wait_for(20);

    for round in 1..=10u32 {
        let extra = format!(
            r#"{{"name": "extra{round}", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}}"#
        );
        // Added beside web, then removed again with web kept, the routes changed each time.
        let r = l
            .reload(graph(&config(
                &format!("{WEB}, {extra}"),
                &format!("v{}", round * 2 - 1),
            )))
            .unwrap();
        assert_eq!((r.kept, r.added.len(), r.removed.len()), (1, 1, 0));
        let added = r.added[0].address;
        assert_eq!(get(added), ok(&format!("v{}", round * 2 - 1)));
        wait_for(done.load(Ordering::SeqCst) + 10);
        let r = l
            .reload(graph(&config(WEB, &format!("v{}", round * 2))))
            .unwrap();
        assert_eq!((r.kept, r.added.len(), r.removed.len()), (1, 0, 1));
        assert_eq!(r.removed[0].address, added);
        assert_eq!(l.served().len(), 1);
        assert_eq!(l.served()[0].address, web);
        wait_for(done.load(Ordering::SeqCst) + 10);
        cell.collect();
    }
    stop_clients.store(true, Ordering::SeqCst);
    for c in clients {
        c.join().unwrap();
    }
    assert!(
        failures.lock().unwrap().is_empty(),
        "{:?}",
        failures.lock().unwrap()
    );
    assert_eq!(get(web), ok("v20"));
    cell.collect();
    assert_eq!(cell.retiring(), 0);
    stop(&e, l);
}

/// `limits.write_stall_timeout` (I17b) is the engine's write timeout: fit's 30s when the config
/// sets none, and what a reload sets from then on, for the connections already open too.
#[test]
fn a_reload_changes_the_write_stall_timeout() {
    let (cell, e, mut l) = serve(&config(WEB, "one"));
    let live = || e.telemetry().live().write_timeout();
    assert_eq!(live(), Some(Duration::from_secs(30)));
    let with_stall = |t: &str| {
        config(WEB, "two").replacen(
            "\"listeners\"",
            &format!(r#""limits": {{"write_stall_timeout": "{t}"}}, "listeners""#),
            1,
        )
    };
    l.reload(graph(&with_stall("250ms"))).unwrap();
    cell.collect();
    assert_eq!(live(), Some(Duration::from_millis(250)));
    let addr = l.served()[0].address;
    assert_eq!(get(addr), ok("two"));
    l.reload(graph(&config(WEB, "three"))).unwrap();
    cell.collect();
    assert_eq!(live(), Some(Duration::from_secs(30)));
    stop(&e, l);
}
