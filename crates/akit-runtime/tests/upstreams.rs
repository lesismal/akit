//! Upstream pools in the graph (I14): one per upstream of the config, given to the modules
//! through their `Env`, and closed when the graph is retired, draining the requests in flight.
//! The pools dial through an engine on the backend `AKIO_TEST_BACKEND` names (epoll by default,
//! `uring`).
#![allow(
    clippy::panic,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, Listener, UringConfig};
use akio_http::Context;
use akio_http::client::ClientRequest;
use akit_config::{Config, Handler as HandlerConfig, from_json};
use akit_runtime::upstream::{EngineSlot, Error, Pool};
use akit_runtime::{
    Env, GraphCell, Handler, HandlerParams, Info, Provision, ProvisionError, Provisioned, Registry,
    UPSTREAM_POOLS, compile,
};

/// The pool the `grab` handler took from its env, by test.
static GRABBED: Mutex<Vec<(String, Pool)>> = Mutex::new(Vec::new());

struct Nothing;

impl akio_http::Service for Nothing {
    fn call(&self, ctx: &mut Context) {
        ctx.text(200, "");
    }
}

impl Handler for Nothing {}

/// A stand-in for I15's proxy: `respond` whose body names an upstream it keeps from its env (and
/// whose status 418 fails its provisioning).
fn new_grab(p: &Provision<'_, HandlerParams>) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    let HandlerConfig::Respond(r) = p.params();
    if r.status == 418 {
        return Err(ProvisionError::field("status", "418 is refused here"));
    }
    let (test, name) = r.body.split_once('/').unwrap();
    let pool = p
        .env()
        .upstream(name)
        .ok_or_else(|| ProvisionError::field("body", format!("no upstream {name:?}")))?;
    GRABBED
        .lock()
        .unwrap()
        .push((test.to_owned(), pool.clone()));
    Ok(Provisioned::new(Arc::new(Nothing) as Arc<dyn Handler>))
}

static MODULES: &[Info<HandlerParams>] = &[Info::handler(
    "http.handlers.respond",
    "Takes an upstream's pool from its env.",
    new_grab,
)];

fn registry() -> Registry<HandlerParams> {
    Registry::new(&[MODULES]).unwrap()
}

fn grabbed(test: &str) -> Pool {
    GRABBED
        .lock()
        .unwrap()
        .iter()
        .find(|(t, _)| t == test)
        .map(|(_, p)| p.clone())
        .unwrap()
}

/// A config with the upstreams `upstreams` (JSON objects) and a route per entry of `routes`
/// (`(body, status)`).
fn config(upstreams: &[String], routes: &[(&str, u16)]) -> Config {
    let routes: Vec<String> = routes
        .iter()
        .map(|(b, s)| {
            format!(r#"{{"handler": {{"module": "respond", "params": {{"status": {s}, "body": "{b}"}}}}}}"#)
        })
        .collect();
    from_json(
        &format!(
            r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}], "routes": [{}], "upstreams": [{}]}}"#,
            routes.join(","),
            upstreams.join(",")
        ),
        None,
    )
    .unwrap()
}

fn engine() -> Arc<Engine> {
    let backend = match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    };
    let e = Engine::builder()
        .listen(
            Listener::tcp("127.0.0.1:0".parse().unwrap())
                .backend(backend)
                .serve(akio_http::serve(|ctx: &mut Context| ctx.text(404, ""))),
        )
        .build()
        .unwrap();
    e.start().unwrap();
    Arc::new(e)
}

/// A blocking HTTP/1.1 upstream that holds every response until let go.
struct Held {
    addr: SocketAddr,
    held: AtomicUsize,
    gate: (Mutex<bool>, Condvar),
}

impl Held {
    fn start() -> Arc<Held> {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let h = Arc::new(Held {
            addr: l.local_addr().unwrap(),
            held: AtomicUsize::new(0),
            gate: (Mutex::new(false), Condvar::new()),
        });
        let h2 = h.clone();
        thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { return };
                let h3 = h2.clone();
                thread::spawn(move || {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                            match s.read(&mut chunk) {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                            }
                        }
                        let end = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
                        buf.drain(..end);
                        h3.held.fetch_add(1, Ordering::SeqCst);
                        {
                            let (open, cv) = &h3.gate;
                            let mut g = open.lock().unwrap();
                            while !*g {
                                g = cv.wait(g).unwrap();
                            }
                        }
                        if s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                            .is_err()
                        {
                            return;
                        }
                    }
                });
            }
        });
        h
    }

    fn release(&self) {
        let (open, cv) = &self.gate;
        *open.lock().unwrap() = true;
        cv.notify_all();
    }
}

fn eventually(what: &str, f: impl Fn() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < Duration::from_secs(10), "never: {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn pools_are_compiled_and_given_to_modules() {
    let c = config(
        &[
            r#"{"name": "app", "addresses": ["127.0.0.1:1", "127.0.0.1:2"], "max_conns": 3}"#
                .into(),
            r#"{"name": "api", "addresses": ["127.0.0.1:3"]}"#.into(),
        ],
        &[("compiled/api", 200)],
    );
    let g = compile(&c, &registry(), &Env::default()).unwrap();
    let names: Vec<&str> = g.upstreams().iter().map(Pool::name).collect();
    assert_eq!(names, ["app", "api"]);
    let app = g.upstream("app").unwrap();
    assert_eq!(
        app.addresses().collect::<Vec<_>>(),
        ["127.0.0.1:1", "127.0.0.1:2"]
    );
    assert!(g.upstream("nope").is_none());
    // The handler got the same pool from its env.
    assert_eq!(grabbed("compiled").name(), "api");
    // The pools are cleaned up after the modules that use them.
    assert_eq!(g.module_ids(), [UPSTREAM_POOLS, "http.handlers.respond"]);
    // Without an engine (as `akit validate`), a request fails at once.
    let (tx, rx) = channel();
    app.request(ClientRequest::get("/"), move |r| {
        let _ = tx.send(r);
    });
    assert!(matches!(rx.recv().unwrap(), Err(Error::NoEngine)));
    // A config without upstreams records none.
    let g = compile(&config(&[], &[]), &registry(), &Env::default()).unwrap();
    assert!(g.upstreams().is_empty() && g.module_ids().is_empty());
    // A handler naming an upstream that is not there fails provisioning.
    let e = compile(
        &config(&[], &[("missing/app", 200)]),
        &registry(),
        &Env::default(),
    )
    .unwrap_err();
    assert_eq!(e.as_slice()[0].path, "routes[0].handler.params.body");
}

#[test]
fn a_config_that_fails_closes_its_pools() {
    let c = config(
        &[r#"{"name": "app", "addresses": ["127.0.0.1:1"]}"#.into()],
        &[("failed/app", 200), ("x/app", 418)],
    );
    assert!(compile(&c, &registry(), &Env::default()).is_err());
    let pool = grabbed("failed");
    assert!(pool.is_closing() && pool.is_closed());
}

#[test]
fn a_retired_graph_drains_its_pools() {
    let up = Held::start();
    let slot = EngineSlot::new();
    let env = Env::new(slot.clone());
    let c = config(
        &[format!(
            r#"{{"name": "app", "addresses": ["{}"], "max_conns": 2}}"#,
            up.addr
        )],
        &[("drained/app", 200)],
    );
    let cell = GraphCell::new(compile(&c, &registry(), &env).unwrap());
    // The engine comes after the graph, as in `akit run`.
    assert!(slot.set(engine()));
    let pool = cell.with(|g| g.upstream("app").unwrap().clone());
    let (tx, rx) = channel();
    for i in 0..6 {
        let tx = tx.clone();
        pool.request(ClientRequest::get(&format!("/{i}")), move |r| {
            let _ = tx.send((i, r));
        });
    }
    eventually("two held, four waiting", || {
        up.held.load(Ordering::SeqCst) == 2 && pool.load(0).unwrap().waiting == 4
    });
    // A new graph, without the upstream: the old one is retired at once (no request runs on
    // it), which closes its pool...
    cell.swap(compile(&config(&[], &[]), &registry(), &env).unwrap());
    assert_eq!(cell.collect(), 1);
    assert!(pool.is_closing() && !pool.is_closed());
    let (tx2, late) = channel();
    pool.request(ClientRequest::get("/late"), move |r| {
        let _ = tx2.send(r);
    });
    assert!(matches!(
        late.recv_timeout(Duration::from_secs(5)).unwrap(),
        Err(Error::Closed)
    ));
    // ...which drains: the six sent before finish.
    up.release();
    for _ in 0..6 {
        let (i, r) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let resp = r.unwrap_or_else(|e| panic!("{i}: {e}"));
        assert_eq!((resp.status, resp.body.as_slice()), (200, &b"ok"[..]));
    }
    eventually("the pool closed", || pool.is_closed());
    assert_eq!(pool.in_flight(), 0);
}
