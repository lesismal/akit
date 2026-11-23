//! The per-request read of the graph (rule 13): `GraphCell::with` against the alternative it
//! replaces, cloning a shared `Arc<Graph>` per request, on one thread and on many, where the
//! shared reference count's cache line bounces between cores.
//!
//! `cargo bench -p akit-runtime --bench graph_read`, on the benchmark host only (rule 23).

#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::indexing_slicing
)]

use std::hint::black_box;
use std::sync::{Arc, Barrier, Mutex};
use std::time::{Duration, Instant};

use akit_config::from_json;
use akit_runtime::{Env, Graph, GraphCell, Registry, compile};

const READS: u64 = 20_000_000;

fn graph() -> Graph {
    let cfg = from_json(r#"{"listeners": [{"name": "web", "address": ":0"}]}"#, None).unwrap();
    compile(&cfg, &Registry::new(&[]).unwrap(), &Env::default()).unwrap()
}

/// Nanoseconds per read, with `threads` threads each doing `READS` reads.
fn run(threads: usize, read: impl Fn() + Send + Sync + 'static) -> f64 {
    let read = Arc::new(read);
    let start = Arc::new(Barrier::new(threads + 1));
    let handles: Vec<_> = (0..threads)
        .map(|_| {
            let (read, start) = (Arc::clone(&read), Arc::clone(&start));
            std::thread::spawn(move || {
                read(); // warm the thread's cache
                start.wait();
                let t = Instant::now();
                for _ in 0..READS {
                    read();
                }
                t.elapsed()
            })
        })
        .collect();
    start.wait();
    let total: Duration = handles.into_iter().map(|h| h.join().unwrap()).sum();
    total.as_nanos() as f64 / (READS as f64 * threads as f64)
}

fn main() {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    println!("reads per thread: {READS}; cores: {cores}");
    println!(
        "{:<34} {:>8} {:>8}",
        "ns per read",
        "1 thread",
        format!("{cores} threads")
    );
    for (name, make) in [
        ("GraphCell::with", 0u8),
        ("Arc<Graph> clone + drop (shared)", 1),
        ("Mutex<Arc<Graph>> lock + clone", 2),
    ] {
        let mut row = Vec::new();
        for threads in [1, cores] {
            let ns = match make {
                0 => {
                    let cell = GraphCell::new(graph());
                    run(threads, move || {
                        black_box(cell.with(|g| g.routes().len()));
                    })
                }
                1 => {
                    let g = Arc::new(graph());
                    run(threads, move || drop(black_box(Arc::clone(&g))))
                }
                _ => {
                    let g = Arc::new(Mutex::new(Arc::new(graph())));
                    run(threads, move || drop(black_box(g.lock().unwrap().clone())))
                }
            };
            row.push(ns);
        }
        println!("{name:<34} {:>8.2} {:>8.2}", row[0], row[1]);
    }
}
