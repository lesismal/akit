//! Requests spread over upstreams by weight, and the failover (I46): fit's `pickPool`, as
//! `handler/proxy/split_test.go` checks it end to end (akit-proxy's tests do too).
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use akit_upstream::split::chance;
use akit_upstream::{EngineSlot, Pool, Split};

fn pool(name: &str, fields: &str) -> Pool {
    let json =
        format!(r#"{{"upstreams": [{{"name": "{name}", "addresses": ["127.0.0.1:1"]{fields}}}]}}"#);
    let u = akit_config::from_json(&json, None)
        .unwrap()
        .upstreams
        .remove(0);
    Pool::new(&u, EngineSlot::new())
}

/// How many of `n` picks went to each upstream, by name.
fn counts(s: &Split, n: usize) -> std::collections::BTreeMap<String, usize> {
    let mut got = std::collections::BTreeMap::new();
    for _ in 0..n {
        *got.entry(s.pick().name().to_owned()).or_default() += 1;
    }
    got
}

#[test]
fn by_weight() {
    // TestSplitWeights: 3 to 1, 800 picks; the bounds are about five deviations out.
    let s = Split::new([(pool("a", ""), 3), (pool("b", ""), 1)], None).unwrap();
    let got = counts(&s, 800);
    let a = got["a"];
    assert_eq!(a + got["b"], 800);
    assert!((540..=660).contains(&a), "{got:?}");
    // Three ways, 1:2:7, over more picks.
    let s = Split::new(
        [(pool("a", ""), 1), (pool("b", ""), 2), (pool("c", ""), 7)],
        None,
    )
    .unwrap();
    let got = counts(&s, 10_000);
    for (name, want) in [("a", 1000), ("b", 2000), ("c", 7000)] {
        let n = got[name];
        assert!(n.abs_diff(want) < 300, "{name}: {got:?}");
    }
    // One upstream: always it, the others never drawn.
    let s = Split::new([(pool("a", ""), 5)], None).unwrap();
    assert_eq!(counts(&s, 100)["a"], 100);
    assert_eq!(Split::one(pool("a", "")).pick().name(), "a");
    assert!(Split::new(std::iter::empty(), None).is_none());
    // The pools, the failover last.
    let s = Split::new(
        [(pool("a", ""), 1), (pool("b", ""), 1)],
        Some(pool("c", "")),
    )
    .unwrap();
    let names: Vec<&str> = s.pools().map(Pool::name).collect();
    assert_eq!(names, ["a", "b", "c"]);
}

#[test]
fn failover() {
    // TestSplitFailover: a's one address out of rotation (max_fails 1), b's in: b.
    let a = pool("a", r#", "max_fails": 1, "fail_timeout": "1m""#);
    let b = pool("b", r#", "max_fails": 1, "fail_timeout": "1m""#);
    let s = Split::new([(a.clone(), 1)], Some(b.clone())).unwrap();
    assert_eq!(s.pick().name(), "a");
    let x = a.stream_pick("127.0.0.1".parse().unwrap());
    a.stream_done(x, true);
    assert!(!a.available());
    assert_eq!(s.pick().name(), "b");
    // Both out: the chosen one still, as a last resort.
    let x = b.stream_pick("127.0.0.1".parse().unwrap());
    b.stream_done(x, true);
    assert_eq!(s.pick().name(), "a");
    // Without a failover, the chosen one whatever its state.
    let s = Split::new([(a, 1)], None).unwrap();
    assert_eq!(s.pick().name(), "a");
}

#[test]
fn failover_after_the_weights() {
    // The weights choose first; the failover stands in for the chosen one alone.
    let a = pool("a", r#", "max_fails": 1, "fail_timeout": "1m""#);
    let x = a.stream_pick("127.0.0.1".parse().unwrap());
    a.stream_done(x, true);
    let s = Split::new([(a, 1), (pool("b", ""), 1)], Some(pool("c", ""))).unwrap();
    let got = counts(&s, 2000);
    assert_eq!(got.get("a"), None, "{got:?}");
    assert_eq!(got["b"] + got["c"], 2000);
    assert!(got["b"].abs_diff(1000) < 200, "{got:?}");
}

#[test]
fn chances() {
    assert!((0..1000).all(|_| chance(100.0)));
    assert!((0..1000).all(|_| chance(150.0)));
    assert!(!(0..1000).any(|_| chance(0.0)));
    assert!(!(0..1000).any(|_| chance(-1.0)));
    // TestMirrorPercent's share: about 25 of 100, over more draws.
    let n = (0..10_000).filter(|_| chance(25.0)).count();
    assert!(n.abs_diff(2500) < 250, "{n}");
}
