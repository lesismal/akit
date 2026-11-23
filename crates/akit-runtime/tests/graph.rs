//! The runtime graph (I04): compiling a config, publishing graphs, reading them per request on
//! several threads (and on akio's I/O threads), and retiring every old graph exactly once, never
//! while a request runs on it or a hold keeps it.

// Test helpers outside #[test] functions may panic like the tests themselves.
#![allow(
    clippy::panic,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

use akio_http::Context;
use akit_config::{Config, Handler as HandlerConfig, from_json};
use akit_runtime::{
    Env, Graph, GraphCell, Handler, HandlerParams, Info, Provision, ProvisionError, Provisioned,
    Registry, compile,
};

/// How many times each body's module was cleaned up, across the tests of this binary (bodies are
/// unique per test).
static CLEANED: Mutex<Option<HashMap<String, usize>>> = Mutex::new(None);

fn cleaned(body: &str) -> usize {
    CLEANED
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(body).copied())
        .unwrap_or(0)
}

/// A stand-in for I05's `respond`: answers with its body, and counts its cleanup.
struct Respond {
    body: String,
}

impl akio_http::Service for Respond {
    fn call(&self, ctx: &mut Context) {
        ctx.text(200, self.body.clone());
    }
}

impl Handler for Respond {}

fn new_respond(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    let HandlerConfig::Respond(r) = p.params() else {
        return Err(ProvisionError::new("not respond"));
    };
    if r.status == 418 {
        return Err(ProvisionError::field("status", "418 is refused here"));
    }
    let body = r.body.clone();
    let key = body.clone();
    Ok(
        Provisioned::new(Arc::new(Respond { body }) as Arc<dyn Handler>).on_cleanup(move || {
            *CLEANED
                .lock()
                .unwrap()
                .get_or_insert_with(HashMap::new)
                .entry(key)
                .or_insert(0) += 1;
        }),
    )
}

static MODULES: &[Info<HandlerParams>] = &[Info::handler(
    "http.handlers.respond",
    "A test respond.",
    new_respond,
)];

fn registry() -> Registry<HandlerParams> {
    Registry::new(&[MODULES]).unwrap()
}

fn config(bodies: &[&str]) -> Config {
    let routes: Vec<String> = bodies
        .iter()
        .map(|b| {
            format!(r#"{{"name": "r-{b}", "match": {{"path": ["/{b}"]}}, "handler": {{"module": "respond", "params": {{"body": "{b}"}}}}}}"#)
        })
        .collect();
    from_json(
        &format!(
            r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}], "routes": [{}]}}"#,
            routes.join(",")
        ),
        None,
    )
    .unwrap()
}

fn graph(bodies: &[&str]) -> Graph {
    compile(&config(bodies), &registry(), &Env::default()).unwrap()
}

fn first_body(g: &Graph) -> String {
    g.routes()[0].name.clone().unwrap()
}

#[test]
fn compiles_routes_and_modules() {
    let g = graph(&["c1a", "c1b"]);
    assert_eq!(g.listeners()[0].name, "web");
    assert_eq!(g.routes().len(), 2);
    assert_eq!(g.routes()[1].path, ["/c1b"]);
    assert_eq!(
        g.module_ids(),
        ["http.handlers.respond", "http.handlers.respond"]
    );
    assert!(!g.is_retired());
    drop(g);
    // Never published: the modules are cleaned up when the graph drops.
    assert_eq!((cleaned("c1a"), cleaned("c1b")), (1, 1));
}

#[test]
fn a_failed_compile_cleans_up_and_names_the_field() {
    let cfg = from_json(
        r#"{"listeners": [{"name": "web", "address": ":0"}], "routes": [
            {"handler": {"module": "respond", "params": {"body": "c2a"}}},
            {"handler": {"module": "respond", "params": {"status": 418}}}]}"#,
        None,
    )
    .unwrap();
    let e = compile(&cfg, &registry(), &Env::default()).unwrap_err();
    assert_eq!(e.as_slice().len(), 1);
    assert_eq!(e.as_slice()[0].path, "routes[1].handler.params.status");
    assert_eq!(
        e.to_string(),
        "routes[1].handler.params.status: 418 is refused here"
    );
    assert_eq!(
        cleaned("c2a"),
        1,
        "the route provisioned before the failure"
    );

    let empty: Registry<HandlerParams> = Registry::new(&[]).unwrap();
    let e = compile(&config(&["c2b"]), &empty, &Env::default()).unwrap_err();
    assert_eq!(
        e.to_string(),
        "routes[0].handler: there is no module \"respond\" in http.handlers"
    );
}

#[test]
fn swap_publishes_and_collect_retires() {
    let cell = GraphCell::new(graph(&["s1"]));
    assert_eq!(cell.with(first_body), "r-s1");
    assert_eq!(cell.swap(graph(&["s2"])), 1);
    assert_eq!(cell.with(first_body), "r-s2");
    assert_eq!(cell.retiring(), 1);
    assert_eq!(cell.collect(), 1);
    assert_eq!((cell.retiring(), cleaned("s1"), cleaned("s2")), (0, 1, 0));
    assert_eq!(cell.collect(), 0);
    drop(cell);
    assert_eq!(
        cleaned("s2"),
        1,
        "the current graph is retired with its cell"
    );
}

#[test]
fn a_hold_keeps_its_graph() {
    let cell = GraphCell::new(graph(&["h1"]));
    let hold = cell.hold();
    cell.swap(graph(&["h2"]));
    assert_eq!(cell.collect(), 0);
    assert_eq!(first_body(&hold), "r-h1");
    assert!(!hold.is_retired());
    drop(hold);
    assert_eq!(cell.collect(), 1);
    assert_eq!(cleaned("h1"), 1);
    // A hold that outlives its cell: the graph is cleaned up when the hold drops.
    let hold = cell.hold();
    drop(cell);
    assert_eq!(cleaned("h2"), 0);
    drop(hold);
    assert_eq!(cleaned("h2"), 1);
}

#[test]
fn a_request_running_on_the_old_graph_keeps_it() {
    let cell = GraphCell::new(graph(&["r1"]));
    let entered = Arc::new(Barrier::new(2));
    let swapped = Arc::new(Barrier::new(2));
    let t = {
        let (cell, entered, swapped) = (cell.clone(), Arc::clone(&entered), Arc::clone(&swapped));
        std::thread::spawn(move || {
            cell.with(|g| {
                entered.wait();
                swapped.wait(); // the swap and a collect happen here
                assert!(!g.is_retired());
                // A nested read in the same request stays on the graph it started on.
                assert_eq!(cell.with(first_body), "r-r1");
                first_body(g)
            })
        })
    };
    entered.wait();
    cell.swap(graph(&["r2"]));
    assert_eq!(cell.collect(), 0, "a thread is inside the old graph");
    swapped.wait();
    assert_eq!(t.join().unwrap(), "r-r1");
    assert_eq!(cell.collect(), 1);
    assert_eq!(cleaned("r1"), 1);
}

#[test]
fn an_idle_thread_does_not_pin_the_old_graph() {
    let cell = GraphCell::new(graph(&["i1"]));
    let done = Arc::new(Barrier::new(2));
    let park = Arc::new(Barrier::new(2));
    let t = {
        let (cell, done, park) = (cell.clone(), Arc::clone(&done), Arc::clone(&park));
        std::thread::spawn(move || {
            assert_eq!(cell.with(first_body), "r-i1"); // cached on this thread
            done.wait();
            park.wait(); // idle while the graph is replaced
            cell.with(first_body)
        })
    };
    done.wait();
    cell.swap(graph(&["i2"]));
    assert_eq!(cell.collect(), 1, "the idle thread is outside");
    assert_eq!(cleaned("i1"), 1);
    park.wait();
    assert_eq!(t.join().unwrap(), "r-i2");
}

/// Many threads read in a loop while graphs are swapped and collected: no read ever sees a
/// retired graph, and every graph is cleaned up exactly once.
#[test]
fn swaps_under_concurrent_reads() {
    const THREADS: usize = 8;
    const SWAPS: usize = 300;
    let name = |i: usize| format!("cc{i}");
    let cell = GraphCell::new(graph(&[&name(0)]));
    let stop = Arc::new(AtomicBool::new(false));
    let reads = Arc::new(AtomicUsize::new(0));
    let readers: Vec<_> = (0..THREADS)
        .map(|_| {
            let (cell, stop, reads) = (cell.clone(), Arc::clone(&stop), Arc::clone(&reads));
            std::thread::spawn(move || {
                let mut n = 0usize;
                while !stop.load(Ordering::Relaxed) {
                    cell.with(|g| assert!(!g.is_retired(), "read a retired graph"));
                    if n % 64 == 0 {
                        let h = cell.hold();
                        assert!(!h.is_retired());
                    }
                    n += 1;
                }
                reads.fetch_add(n, Ordering::Relaxed);
            })
        })
        .collect();
    for i in 1..=SWAPS {
        cell.swap(graph(&[&name(i)]));
        cell.collect();
    }
    stop.store(true, Ordering::Relaxed);
    for r in readers {
        r.join().unwrap();
    }
    while cell.retiring() > 0 {
        cell.collect();
        std::thread::sleep(Duration::from_millis(1));
    }
    for i in 0..SWAPS {
        assert_eq!(cleaned(&name(i)), 1, "graph {i}");
    }
    assert_eq!(cleaned(&name(SWAPS)), 0, "the current graph");
    drop(cell);
    assert_eq!(cleaned(&name(SWAPS)), 1);
    assert!(reads.load(Ordering::Relaxed) > THREADS);
}

/// Requests on akio's I/O threads read the cell; a swap changes what the next requests get, and
/// the old graph is retired once they are done.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn swaps_while_akio_serves() {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    use akio::{Backend, Config as EngineConfig, Engine, Listener, UringConfig};

    /// The akio service: the route of the current graph, on whichever I/O thread runs it.
    struct Current(GraphCell);
    impl akio_http::Service for Current {
        fn call(&self, ctx: &mut Context) {
            self.0.with(|g| g.routes()[0].handler.call(ctx));
        }
    }

    let backend = match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    };
    let cell = GraphCell::new(graph(&["ak1"]));
    let e = Engine::builder()
        .config({
            let mut v = EngineConfig::default();
            v.shutdown_timeout = Duration::from_millis(500);
            v
        })
        .listen(
            Listener::tcp("127.0.0.1:0".parse().unwrap())
                .backend(backend)
                .threads(4)
                .serve(akio_http::serve(Current(cell.clone()))),
        )
        .build()
        .unwrap();
    e.start().unwrap();
    let addr = e.local_addrs()[0];
    let get = || {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp).unwrap();
        resp
    };
    // Enough connections to reach every I/O thread.
    for _ in 0..16 {
        assert!(get().ends_with("ak1"));
    }
    cell.swap(graph(&["ak2"]));
    for _ in 0..16 {
        assert!(get().ends_with("ak2"));
    }
    // Every I/O thread is idle between requests: the old graph retires.
    assert_eq!(cell.collect(), 1);
    assert_eq!(cleaned("ak1"), 1);
    e.shutdown();
    e.wait();
    drop(e);
    drop(cell);
    assert_eq!(cleaned("ak2"), 1);
}
