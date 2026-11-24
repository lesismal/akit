//! Load balancing (I19): fit's balancer tests (`upstream/balance_test.go`), ported. Each thread
//! keeps its own turn, starting where its slot starts, so a sequence fit checks from its first
//! pick is checked here as a rotation of fit's; the shares are fit's, exactly. Then the pool:
//! `pick` follows the upstream's `load_balancing` and `weights`, and `least_conn` reads the
//! requests under way to each address.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, Listener, UringConfig};
use akio_http::client::ClientRequest;
use akit_upstream::balance::MAX_SCHEDULE;
use akit_upstream::{Balancer, EngineSlot, LoadBalancing, Pool};

const ADDRS: &[u8] = b"abcdefghijklmnopqrstuvwxyz";

/// `n` picks of `b` with every address idle, as letters.
fn seq(b: &Balancer, n: usize) -> String {
    (0..n).map(|_| ADDRS[b.pick(|_| 0)] as char).collect()
}

/// How often each address came up in `n` picks with every address idle.
fn counts(b: &Balancer, n: usize) -> Vec<usize> {
    let mut got = vec![0; b.len()];
    for _ in 0..n {
        got[b.pick(|_| 0)] += 1;
    }
    got
}

/// Whether `s` is a rotation of `round` repeated: the same sequence started elsewhere.
fn rotation_of(s: &str, round: &str) -> bool {
    let twice = round.repeat(s.len() / round.len() + 2);
    (0..round.len()).any(|i| twice[i..].starts_with(s))
}

#[test]
fn round_robin() {
    let b = Balancer::new(LoadBalancing::RoundRobin, &[1, 1, 1]);
    let s = seq(&b, 6);
    assert!(rotation_of(&s, "abc"), "sequence {s}");
    assert_eq!(counts(&b, 3000), [1000, 1000, 1000]);
}

#[test]
fn weighted_round_robin() {
    let b = Balancer::new(LoadBalancing::WeightedRoundRobin, &[5, 1, 1]);
    // Smooth: nginx's sequence for 5, 1, 1, rather than five a's in a row.
    let s = seq(&b, 7);
    assert!(rotation_of(&s, "aabacaa"), "sequence {s}");
    assert_eq!(counts(&b, 7000), [5000, 1000, 1000]);
    let b = Balancer::new(LoadBalancing::WeightedRoundRobin, &[3, 2]);
    assert_eq!(counts(&b, 5000), [3000, 2000]);
}

#[test]
fn weighted_round_robin_schedule() {
    // Weights with a common divisor make the sequence of their quotients: 10, 2, 2 as 5, 1, 1.
    let b = Balancer::new(LoadBalancing::WeightedRoundRobin, &[10, 2, 2]);
    let s = seq(&b, 14);
    assert!(rotation_of(&s, "aabacaa"), "sequence {s}");
    // Too many weights to work out ahead: picked on the slot's running weights, still smooth
    // and still by weight.
    let weights: Vec<u32> = (0..100).map(|i| 997 - i % 2).collect();
    let total: usize = weights.iter().map(|&w| w as usize).sum();
    assert!(total > MAX_SCHEDULE);
    let b = Balancer::new(LoadBalancing::WeightedRoundRobin, &weights);
    let got = counts(&b, total);
    for (i, &w) in weights.iter().enumerate() {
        assert_eq!(got[i], w as usize, "address {i} of weight {w}");
    }
}

/// `threads` threads each picking `n` times; the counts of all of them.
fn concurrent(b: &Arc<Balancer>, threads: usize, n: usize) -> Vec<usize> {
    let got = Arc::new(Mutex::new(vec![0; b.len()]));
    let hs: Vec<_> = (0..threads)
        .map(|_| {
            let (b, got) = (b.clone(), got.clone());
            thread::spawn(move || {
                let mine = counts(&b, n);
                let mut g = got.lock().unwrap();
                for (t, m) in g.iter_mut().zip(mine) {
                    *t += m;
                }
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    Arc::try_unwrap(got).unwrap().into_inner().unwrap()
}

#[test]
fn weighted_round_robin_concurrent() {
    // Every pick takes its own entry of its slot's sequence, so the shares are exact over whole
    // rounds whatever the interleaving, and whichever threads share a slot.
    let b = Arc::new(Balancer::new(LoadBalancing::WeightedRoundRobin, &[5, 1, 1]));
    assert_eq!(concurrent(&b, 8, 7000), [40000, 8000, 8000]);
    let b = Arc::new(Balancer::new(LoadBalancing::RoundRobin, &[1, 1, 1]));
    assert_eq!(concurrent(&b, 8, 3000), [8000, 8000, 8000]);
}

#[test]
fn least_conn() {
    let b = Balancer::new(LoadBalancing::LeastConn, &[1, 1, 1]);
    // Idle addresses share the load.
    assert_eq!(counts(&b, 300), [100, 100, 100]);
    // Busy ones are passed over until the rest catch up. (No pick here is a tie: which of two
    // tied addresses wins depends on the slot the picking thread lands on, so on the CPU it
    // runs on.)
    let mut active = [5usize, 3, 0];
    for _ in 0..3 {
        let p = b.pick(|i| active[i]);
        assert_eq!(p, 2, "picked {p} with {active:?}");
        active[p] += 1;
    }
    // Now b has 3 and c 4: b next.
    active[2] += 1;
    assert_eq!(b.pick(|i| active[i]), 1);

    // Weighted: a of weight 2 takes twice the requests under way.
    let b = Balancer::new(LoadBalancing::LeastConn, &[2, 1]);
    let mut active = [0usize; 2];
    for _ in 0..300 {
        let p = b.pick(|i| active[i]);
        active[p] += 1;
    }
    assert_eq!(active, [200, 100]);
}

#[test]
fn pick_concurrent() {
    // Every policy, from 8 threads at once: every pick is an address, and the shares of the
    // turn-taking policies hold.
    for policy in [
        LoadBalancing::RoundRobin,
        LoadBalancing::WeightedRoundRobin,
        LoadBalancing::LeastConn,
    ] {
        let weights: &[u32] = if policy == LoadBalancing::RoundRobin {
            &[1, 1, 1]
        } else {
            &[3, 2, 1]
        };
        let b = Arc::new(Balancer::new(policy, weights));
        let got = concurrent(&b, 8, 6000);
        assert_eq!(got.iter().sum::<usize>(), 48000, "{policy:?}");
        match policy {
            LoadBalancing::RoundRobin => assert_eq!(got, [16000; 3]),
            LoadBalancing::WeightedRoundRobin => assert_eq!(got, [24000, 16000, 8000]),
            // Idle, least_conn takes the addresses in turn, as round robin.
            _ => assert_eq!(got, [16000; 3]),
        }
    }
}

#[test]
fn one_address() {
    for policy in [
        LoadBalancing::RoundRobin,
        LoadBalancing::WeightedRoundRobin,
        LoadBalancing::LeastConn,
    ] {
        let b = Balancer::new(policy, &[7]);
        assert_eq!(counts(&b, 10), [10]);
    }
}

/// An upstream config: `addresses`, then `fields`.
fn upstream(addresses: &[String], fields: &str) -> akit_config::Upstream {
    let list: Vec<String> = addresses.iter().map(|a| format!("{a:?}")).collect();
    let json = format!(
        r#"{{"upstreams": [{{"name": "app", "addresses": [{}]{}{fields}}}]}}"#,
        list.join(", "),
        if fields.is_empty() { "" } else { ", " }
    );
    akit_config::from_json(&json, None)
        .unwrap()
        .upstreams
        .remove(0)
}

fn names(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("127.0.0.1:{}", 9001 + i)).collect()
}

/// How often each address came up in `n` of the pool's picks.
fn pool_counts(p: &Pool, n: usize) -> Vec<usize> {
    let mut got = vec![0; p.addresses().len()];
    for _ in 0..n {
        got[p.pick()] += 1;
    }
    got
}

#[test]
fn pool_follows_the_config() {
    // The default: round robin.
    let p = Pool::new(&upstream(&names(3), ""), EngineSlot::new());
    assert_eq!(p.load_balancing(), LoadBalancing::RoundRobin);
    assert_eq!(pool_counts(&p, 300), [100, 100, 100]);
    // Weights given: weighted round robin, by them.
    let p = Pool::new(
        &upstream(&names(3), r#""weights": [5, 1, 1]"#),
        EngineSlot::new(),
    );
    assert_eq!(p.load_balancing(), LoadBalancing::WeightedRoundRobin);
    assert_eq!(p.weight(0), 5);
    assert_eq!(pool_counts(&p, 700), [500, 100, 100]);
    // Weights all 1: round robin, as fit's default.
    let p = Pool::new(
        &upstream(&names(2), r#""weights": [1, 1]"#),
        EngineSlot::new(),
    );
    assert_eq!(p.load_balancing(), LoadBalancing::RoundRobin);
    // least_conn, idle: in turn.
    let p = Pool::new(
        &upstream(
            &names(2),
            r#""load_balancing": "least_conn", "weights": [2, 1]"#,
        ),
        EngineSlot::new(),
    );
    assert_eq!(p.load_balancing(), LoadBalancing::LeastConn);
    assert_eq!(pool_counts(&p, 10), [5, 5]);
}

/// An engine with one listener (answering 404), whose threads host the pools' connections.
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
                .serve(akio_http::serve(|ctx: &mut akio_http::Context| {
                    ctx.text(404, "");
                })),
        )
        .build()
        .unwrap();
    e.start().unwrap();
    Arc::new(e)
}

/// A blocking HTTP/1.1 upstream, a thread per connection, that holds every response until it is
/// let go, and counts the requests it has read.
struct Held {
    addr: SocketAddr,
    held: AtomicUsize,
    gate: (Mutex<bool>, Condvar),
}

impl Held {
    fn start() -> Arc<Held> {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let u = Arc::new(Held {
            addr: l.local_addr().unwrap(),
            held: AtomicUsize::new(0),
            gate: (Mutex::new(false), Condvar::new()),
        });
        let u2 = u.clone();
        thread::spawn(move || {
            for s in l.incoming() {
                let Ok(s) = s else { return };
                let u3 = u2.clone();
                thread::spawn(move || u3.serve(s));
            }
        });
        u
    }

    fn serve(&self, mut s: TcpStream) {
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
            self.held.fetch_add(1, Ordering::SeqCst);
            {
                let (open, cv) = &self.gate;
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
fn pool_least_conn_reads_the_requests_under_way() {
    let e = engine();
    let ups = [Held::start(), Held::start()];
    let addrs: Vec<String> = ups.iter().map(|u| u.addr.to_string()).collect();
    let pool = Pool::new(
        &upstream(
            &addrs,
            r#""load_balancing": "least_conn", "weights": [2, 1]"#,
        ),
        EngineSlot::with(e.clone()),
    );
    let (tx, rx) = channel();
    // Each request is held, so each pick sees those before it under way: a of weight 2 takes
    // twice b's.
    for i in 0..9 {
        let tx = tx.clone();
        let b = pool.pick();
        pool.send(b, ClientRequest::get(&format!("/n/{i}")), move |r| {
            let _ = tx.send(r.is_ok());
        });
    }
    eventually("9 requests held", || {
        ups[0].held.load(Ordering::SeqCst) + ups[1].held.load(Ordering::SeqCst) == 9
    });
    assert_eq!(ups[0].held.load(Ordering::SeqCst), 6);
    assert_eq!(ups[1].held.load(Ordering::SeqCst), 3);
    assert_eq!(pool.load(0).unwrap().busy, 6);
    assert_eq!(pool.load(1).unwrap().busy, 3);
    for u in &ups {
        u.release();
    }
    for _ in 0..9 {
        assert!(rx.recv_timeout(Duration::from_secs(10)).unwrap());
    }
    // Done, they no longer count: the next picks share the addresses by weight again.
    eventually("slots given back", || {
        pool.load(0).unwrap().busy + pool.load(1).unwrap().busy == 0
    });
    let mut by: HashMap<usize, usize> = HashMap::new();
    for _ in 0..30 {
        *by.entry(pool.pick()).or_default() += 1;
    }
    assert_eq!(by.values().sum::<usize>(), 30);
    e.shutdown();
}
