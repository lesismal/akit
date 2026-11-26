//! A pick does not allocate, by any policy, with a key, a sticky cookie and addresses down: fit's
//! `TestPickAllocs` and `TestNewPoliciesAllocs`, counted by a global allocator (`stats_alloc`, so
//! that akit has no `unsafe` of its own). Its own binary, without the test harness, so that no
//! other thread allocates while it counts.

#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects
)]

use std::alloc::System;
use std::hint::black_box;

use akio_http::Headers;
use akit_upstream::{Balancer, EngineSlot, Keys, LoadBalancing, Pool, sticky_id};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

/// The allocations `f` makes, run 1000 times.
fn allocs(mut f: impl FnMut()) -> usize {
    f(); // anything lazily set up on a first call (the thread's slot and generator)
    let region = Region::new(GLOBAL);
    for _ in 0..1000 {
        f();
    }
    region.change().allocations
}

fn main() {
    let addrs = ["10.0.0.1:80", "10.0.0.2:80", "10.0.0.3:80", "10.0.0.4:80"];
    let mut headers = Headers::new();
    headers.append("X-User", "someone");
    headers.append("Cookie", "a=b; user=someone; c=d");
    let keys = Keys {
        peer: Some("2001:db8::1".parse().unwrap()),
        path: "/someone",
        query: "q=1",
        headers: Some(&headers),
    };
    for policy in [
        LoadBalancing::RoundRobin,
        LoadBalancing::WeightedRoundRobin,
        LoadBalancing::LeastConn,
        LoadBalancing::Random,
        LoadBalancing::P2c,
        LoadBalancing::IpHash,
        LoadBalancing::UriHash,
        LoadBalancing::HeaderHash,
        LoadBalancing::CookieHash,
    ] {
        let key = match policy {
            LoadBalancing::HeaderHash => "x-user",
            LoadBalancing::CookieHash => "user",
            _ => "",
        };
        let b = Balancer::keyed(policy, &addrs, &[3, 2, 1, 1], key);
        let n = allocs(|| {
            black_box(b.pick_keyed(b.key(black_box(&keys)), |i| i, |i| i != 1));
        });
        assert_eq!(n, 0, "{policy:?}: {n} allocations in 1000 picks");
    }

    // The pool, with a sticky cookie that names an address, and one that does not.
    let list: Vec<String> = addrs.iter().map(|a| format!("{a:?}")).collect();
    let json = format!(
        r#"{{"upstreams": [{{"name": "app", "addresses": [{}], "load_balancing": "ip_hash",
            "sticky_cookie": {{}}}}]}}"#,
        list.join(", ")
    );
    let u = akit_config::from_json(&json, None)
        .unwrap()
        .upstreams
        .remove(0);
    let pool = Pool::new(&u, EngineSlot::new());
    let mut named = Headers::new();
    named.append(
        "Cookie",
        format!("x=1; akit_sticky={}", sticky_id(addrs[2])),
    );
    for h in [&named, &headers] {
        let k = Keys {
            headers: Some(h),
            ..keys
        };
        let n = allocs(|| {
            let b = pool.pick_for(black_box(&k));
            black_box(pool.set_cookie(&k, b));
        });
        assert_eq!(n, 0, "pool: {n} allocations in 1000 picks");
    }
    // A split by weight with a failover (I46).
    let split =
        akit_upstream::Split::new([(pool.clone(), 3), (pool.clone(), 1)], Some(pool)).unwrap();
    let n = allocs(|| {
        black_box(split.pick());
        black_box(akit_upstream::split::chance(black_box(50.0)));
    });
    assert_eq!(n, 0, "split: {n} allocations in 1000 picks");
    println!("no_alloc: ok");
}
