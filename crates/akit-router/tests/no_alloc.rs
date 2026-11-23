//! A lookup does not allocate, nor does canonicalizing a path with nothing to remove: fit's
//! `TestMatchAllocs` and `TestCanonicalizeAllocs`, counted by a global allocator
//! (`stats_alloc`, so that akit has no `unsafe` of its own). Its own binary, without the test
//! harness, so that no other thread allocates while it counts.

#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects
)]

use std::alloc::System;
use std::hint::black_box;

use akit_router::{Route, Router, canonicalize};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

/// The allocations `f` makes, run 100 times.
fn allocs(mut f: impl FnMut()) -> usize {
    f(); // anything lazily set up on a first call
    let region = Region::new(GLOBAL);
    for _ in 0..100 {
        f();
    }
    region.change().allocations
}

fn route(hosts: &[&str], paths: &[&str], priority: i64, value: usize) -> Route<usize> {
    Route {
        hosts: hosts.iter().map(|s| (*s).to_owned()).collect(),
        paths: paths.iter().map(|s| (*s).to_owned()).collect(),
        priority,
        value,
    }
}

fn main() {
    let mut routes = vec![
        route(&["*.example.com"], &["/api/*"], 0, 0),
        route(&["example.com"], &["/hello", "/static/*"], 1, 1),
        route(&["**.example.org"], &["/deep/*"], 0, 2),
        route(&[], &["/*"], 0, 3),
    ];
    for i in 0..100 {
        routes.push(route(
            &[&format!("h{i}.example.net")],
            &[&format!("/p{i}/*")],
            0,
            4 + i,
        ));
    }
    let r = Router::new(routes);
    let mut failed = false;
    for (host, path, want) in [
        ("example.com", "/hello", Some(1)),
        ("EXAMPLE.com:8080", "/static/css/site.css", Some(1)),
        ("www.Example.com", "/api/v1", Some(0)),
        ("A.B.C.Example.ORG.", "/deep/x", Some(2)),
        ("h42.example.net", "/p42/x", Some(46)),
        ("[::1]:8080", "/nothing", Some(3)),
        ("x", "", None),
    ] {
        assert_eq!(r.find(host, path).copied(), want, "{host} {path}");
        let n = allocs(|| {
            black_box(r.find(black_box(host), black_box(path)));
        });
        println!("find({host:?}, {path:?}): {n} allocations in 100");
        failed |= n != 0;
    }
    for path in ["/api/v1/users/12345/profile.json", "/", "*"] {
        let n = allocs(|| {
            black_box(canonicalize(black_box(path)).unwrap());
        });
        println!("canonicalize({path:?}): {n} allocations in 100");
        failed |= n != 0;
    }
    // The count is real: a path with something to remove allocates.
    let n = allocs(|| {
        black_box(canonicalize(black_box("/a/../b")).unwrap());
    });
    assert!(n > 0, "the allocator counts nothing");
    if failed {
        panic!("a lookup allocated");
    }
    println!("no_alloc: ok");
}
