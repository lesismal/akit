//! Router lookups, v0 (I05's scan of the routes, `v0.rs`) against v1 (I11's maps), at a few route
//! counts: fit's `BenchmarkMatch` table (each route a host of its own with a prefix and an exact
//! path, and a `*.` catch-all), looked up for a route in the middle, for the catch-all, and for a
//! host no route has; and the one-route config the benchmark scenarios serve.
//!
//! `cargo bench -p akit-router --bench lookup`, on the benchmark host only (rule 23).

#![allow(
    missing_docs,
    unreachable_pub,
    dead_code,
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::cast_precision_loss,
    clippy::indexing_slicing
)]

mod v0;

use std::hint::black_box;
use std::time::Instant;

const LOOKUPS: u32 = 5_000_000;

/// fit's `BenchmarkMatch` routes, `n` of them and the catch-all: (hosts, paths, value).
fn table(n: usize) -> Vec<(Vec<String>, Vec<String>, usize)> {
    let mut t: Vec<_> = (0..n)
        .map(|i| {
            (
                vec![format!("h{i}.example.com")],
                vec![format!("/api/v{i}/*"), format!("/p{i}")],
                i,
            )
        })
        .collect();
    t.push((vec!["*.example.com".into()], vec!["/*".into()], usize::MAX));
    t
}

/// Nanoseconds per call of `f`, the best of 3.
fn ns(mut f: impl FnMut()) -> f64 {
    (0..3)
        .map(|_| {
            let t = Instant::now();
            for _ in 0..LOOKUPS {
                f();
            }
            t.elapsed().as_nanos() as f64 / f64::from(LOOKUPS)
        })
        .fold(f64::INFINITY, f64::min)
}

fn main() {
    println!("lookups per run: {LOOKUPS}, best of 3");
    println!(
        "{:<48} {:>8} {:>8} {:>7}",
        "ns per lookup", "v0", "v1", "v0/v1"
    );
    let row = |name: String, a: f64, b: f64| {
        println!("{name:<48} {a:>8.1} {b:>8.1} {:>6.1}x", a / b);
    };

    // The benchmark scenarios' config: one route, no host, one exact path.
    let r0 = v0::Router::new(vec![v0::Route {
        hosts: vec![],
        paths: vec!["/".into()],
        priority: 0,
        value: 0usize,
    }]);
    let r1 = akit_router::Router::new(vec![akit_router::Route {
        hosts: vec![],
        paths: vec!["/".into()],
        priority: 0,
        value: 0usize,
    }]);
    let (h, p) = ("127.0.0.1:8080", "/");
    assert_eq!(r0.find(h, p), r1.find(h, p));
    row(
        "1 route, \"/\"".into(),
        ns(|| {
            black_box(r0.find(black_box(h), black_box(p)));
        }),
        ns(|| {
            black_box(r1.find(black_box(h), black_box(p)));
        }),
    );

    for n in [10, 100, 1000] {
        let t = table(n);
        let r0 = v0::Router::new(
            t.iter()
                .map(|(hosts, paths, v)| v0::Route {
                    hosts: hosts.clone(),
                    paths: paths.clone(),
                    priority: 0,
                    value: *v,
                })
                .collect(),
        );
        let r1 = akit_router::Router::new(
            t.iter()
                .map(|(hosts, paths, v)| akit_router::Route {
                    hosts: hosts.clone(),
                    paths: paths.clone(),
                    priority: 0,
                    value: *v,
                })
                .collect(),
        );
        let mid = n / 2;
        let lookups = [
            (
                "middle route",
                format!("H{mid}.example.com:8080"),
                format!("/api/v{mid}/users/1"),
            ),
            ("catch-all", "www.example.com".into(), "/x/y".into()),
            ("no route", "other.org".into(), "/api/v1/users/1".into()),
        ];
        for (what, host, path) in &lookups {
            let (h, p) = (host.as_str(), path.as_str());
            assert_eq!(r0.find(h, p), r1.find(h, p), "{what}");
            row(
                format!("{n} routes + catch-all, {what}"),
                ns(|| {
                    black_box(r0.find(black_box(h), black_box(p)));
                }),
                ns(|| {
                    black_box(r1.find(black_box(h), black_box(p)));
                }),
            );
        }
    }
}
