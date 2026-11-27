//! Load balancing v2 (I36): fit's tests of its `random`, `p2c`, hash and sticky cookie policies
//! (`upstream/hash_test.go`, `upstream/edges_test.go`), ported, and the ring checked against
//! fit's: the same keys go to the same addresses as in fit.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::cell::RefCell;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::mpsc::channel;
use std::time::Duration;

use akio::{Backend, Engine, Listener, UringConfig};
use akio_http::Headers;
use akio_http::client::ClientRequest;
use akit_upstream::{Balancer, EngineSlot, Keys, LoadBalancing, Pool, Sticky, sticky_id};

/// Whether `got` is within `tolerance` (a fraction) of `want`.
fn within(got: usize, want: usize, tolerance: f64) -> bool {
    (got as f64 - want as f64).abs() <= tolerance * want as f64
}

const LETTERS: [&str; 4] = ["a", "b", "c", "d"];

/// A balancer of `policy` over addresses `a`, `b`, … of `weights`.
fn balancer(policy: LoadBalancing, weights: &[u32], key: &str) -> Balancer {
    Balancer::keyed(policy, &LETTERS[..weights.len()], weights, key)
}

/// Which addresses are down, for the picks' `up`.
struct Down(RefCell<Vec<bool>>);

impl Down {
    fn none(n: usize) -> Down {
        Down(RefCell::new(vec![false; n]))
    }
    fn set(&self, i: usize, down: bool) {
        self.0.borrow_mut()[i] = down;
    }
    fn up(&self) -> impl Fn(usize) -> bool + '_ {
        move |i| !self.0.borrow()[i]
    }
}

/// How often each address came up in `n` picks without a key, every address idle.
fn counts(b: &Balancer, n: usize, down: &Down) -> Vec<usize> {
    let mut got = vec![0; b.len()];
    for _ in 0..n {
        got[b.pick_up(|_| 0, down.up())] += 1;
    }
    got
}

#[test]
fn random() {
    let none = Down::none(3);
    let got = counts(
        &balancer(LoadBalancing::Random, &[1, 1, 1], ""),
        30000,
        &none,
    );
    assert!(
        got.iter().all(|&g| within(g, 10000, 0.05)),
        "random: {got:?}"
    );
    let got = counts(&balancer(LoadBalancing::Random, &[3, 1], ""), 40000, &none);
    assert!(
        within(got[0], 30000, 0.03) && within(got[1], 10000, 0.08),
        "random by weight 3 and 1: {got:?}"
    );
}

#[test]
fn p2c() {
    let none = Down::none(3);
    let b = balancer(LoadBalancing::P2c, &[1, 1, 1], "");
    // The busiest is never the less busy of two.
    let active = RefCell::new(vec![10usize, 0, 0]);
    let mut got = [0usize; 3];
    for _ in 0..3000 {
        got[b.pick_up(|i| active.borrow()[i], none.up())] += 1;
    }
    assert!(got[0] == 0 && within(got[1], 1500, 0.1), "a busy: {got:?}");
    // Requests that stay keep the addresses level.
    active.borrow_mut()[0] = 0;
    for _ in 0..3000 {
        let p = b.pick_up(|i| active.borrow()[i], none.up());
        active.borrow_mut()[p] += 1;
    }
    let a = active.borrow().clone();
    assert!(a.iter().all(|&x| within(x, 1000, 0.02)), "under way: {a:?}");
    // By weight: a of weight 3 carries three times what b does.
    let b = balancer(LoadBalancing::P2c, &[3, 1], "");
    let active = RefCell::new(vec![0usize; 2]);
    for _ in 0..4000 {
        let p = b.pick_up(|i| active.borrow()[i], none.up());
        active.borrow_mut()[p] += 1;
    }
    assert!(
        within(active.borrow()[0], 3000, 0.01),
        "by weight: {active:?}"
    );
    let one = balancer(LoadBalancing::P2c, &[1], "");
    assert_eq!(counts(&one, 10, &Down::none(1)), [10]);
}

#[test]
fn p2c_one_up() {
    let down = Down::none(3);
    let b = balancer(LoadBalancing::P2c, &[1, 1, 1], "");
    down.set(0, true);
    down.set(2, true);
    assert_eq!(counts(&b, 1000, &down), [0, 1000, 0]);
}

#[test]
fn picks_skip_down() {
    for policy in [LoadBalancing::Random, LoadBalancing::P2c] {
        let down = Down::none(3);
        let b = balancer(policy, &[1, 1, 1], "");
        down.set(1, true);
        let got = counts(&b, 300, &down);
        assert!(
            got[1] == 0 && got[0] + got[2] == 300,
            "{policy:?} with b down: {got:?}"
        );
        down.set(0, true);
        down.set(2, true);
        let got = counts(&b, 300, &down);
        assert!(
            got.iter().all(|&g| g > 0),
            "{policy:?} with all down: {got:?}"
        );
    }
    let down = Down::none(2);
    let b = balancer(LoadBalancing::Random, &[3, 1], "");
    down.set(0, true);
    assert_eq!(
        counts(&b, 100, &down),
        [0, 100],
        "weighted random with a down"
    );
    // fit's TestWeightedRandomAllDown.
    down.set(1, true);
    let got = counts(&b, 400, &down);
    assert!(got[0] > 0 && got[1] > 0, "all down: {got:?}");
}

const HASHES: [LoadBalancing; 4] = [
    LoadBalancing::IpHash,
    LoadBalancing::UriHash,
    LoadBalancing::HeaderHash,
    LoadBalancing::CookieHash,
];

fn hash_key(policy: LoadBalancing) -> &'static str {
    match policy {
        LoadBalancing::HeaderHash => "x-user",
        LoadBalancing::CookieHash => "user",
        _ => "",
    }
}

/// The parts of a request whose key, for `policy`, is `key` (fit's `keyed`): an owned request,
/// lent as [`Keys`].
struct Req {
    peer: IpAddr,
    path: String,
    query: String,
    headers: Headers,
}

impl Req {
    fn keyed(policy: LoadBalancing, key: &str) -> Req {
        let mut r = Req {
            peer: "192.0.2.1".parse().unwrap(),
            path: "/".into(),
            query: String::new(),
            headers: Headers::new(),
        };
        match policy {
            LoadBalancing::IpHash => r.peer = key.parse().unwrap(),
            LoadBalancing::UriHash => (r.path, r.query) = (format!("/{key}"), "q=1".into()),
            LoadBalancing::HeaderHash => r.headers.append("X-User", key),
            LoadBalancing::CookieHash => {
                r.headers.append("Cookie", format!("a=b; user={key}; c=d"));
            }
            _ => {}
        }
        r
    }

    fn keys(&self) -> Keys<'_> {
        Keys {
            peer: Some(self.peer),
            path: &self.path,
            query: &self.query,
            headers: Some(&self.headers),
        }
    }
}

fn key_of(policy: LoadBalancing, i: usize) -> String {
    if policy == LoadBalancing::IpHash {
        format!("10.{}.{}.{}", i >> 16 & 255, i >> 8 & 255, i & 255)
    } else {
        format!("key{i}")
    }
}

/// `b`'s pick for the request whose key is `key`, with `down`.
fn pick(b: &Balancer, policy: LoadBalancing, key: &str, down: &Down) -> usize {
    let r = Req::keyed(policy, key);
    b.pick_keyed(b.key(&r.keys()), |_| 0, down.up())
}

#[test]
fn hash_distribution() {
    for policy in HASHES {
        let none = Down::none(4);
        let b = balancer(policy, &[1, 1, 1, 1], hash_key(policy));
        let mut got = [0usize; 4];
        for i in 0..40000 {
            got[pick(&b, policy, &key_of(policy, i), &none)] += 1;
        }
        assert!(
            got.iter().all(|&g| within(g, 10000, 0.2)),
            "{policy:?}: {got:?}"
        );
        let b = balancer(policy, &[2, 1, 1], hash_key(policy));
        let mut got = [0usize; 3];
        for i in 0..40000 {
            got[pick(&b, policy, &key_of(policy, i), &none)] += 1;
        }
        assert!(
            within(got[0], 20000, 0.15),
            "{policy:?} by weight 2, 1, 1: {got:?}"
        );
    }
}

/// fit's TestHashStable: the same key goes to the same address; adding an address moves only
/// keys to it, about its share of them; an address down moves only its own keys, and they come
/// back with it.
#[test]
fn hash_stable() {
    for policy in HASHES {
        let b3 = balancer(policy, &[1, 1, 1], hash_key(policy));
        let b4 = balancer(policy, &[1, 1, 1, 1], hash_key(policy));
        let (none3, none4) = (Down::none(3), Down::none(4));
        const N: usize = 20000;
        let mut before = vec![0; N];
        let mut moved = 0;
        for (i, was) in before.iter_mut().enumerate() {
            let k = key_of(policy, i);
            *was = pick(&b3, policy, &k, &none3);
            assert_eq!(pick(&b3, policy, &k, &none3), *was, "{policy:?}: key {i}");
            let after = pick(&b4, policy, &k, &none4);
            if after != *was {
                assert_eq!(after, 3, "{policy:?}: key {i} moved from {was} to {after}");
                moved += 1;
            }
        }
        assert!(
            within(moved, N / 4, 0.3),
            "{policy:?}: {moved} of {N} keys moved"
        );

        let down = Down::none(3);
        down.set(1, true);
        for (i, &was) in before.iter().enumerate() {
            let got = pick(&b3, policy, &key_of(policy, i), &down);
            assert!(
                got != 1 && (was == 1 || got == was),
                "{policy:?} with b down: key {i} went to {got}, before to {was}"
            );
        }
        down.set(1, false);
        for (i, &was) in before.iter().enumerate() {
            assert_eq!(pick(&b3, policy, &key_of(policy, i), &down), was);
        }
    }
}

#[test]
fn hash_keys() {
    let b = balancer(LoadBalancing::IpHash, &[1; 4], "");
    let none = Down::none(4);
    let by_ip = |ip: &str| {
        let keys = Keys {
            peer: Some(ip.parse().unwrap()),
            ..Keys::default()
        };
        b.pick_keyed(b.key(&keys), |_| 0, none.up())
    };
    // The port does not count (Keys has none), and an IPv4 address mapped into IPv6 is the
    // IPv4 address, as fit sees it.
    assert_eq!(by_ip("10.0.0.1"), by_ip("::ffff:10.0.0.1"));
    assert_eq!(
        b.key(&Keys {
            peer: Some("10.0.0.1".parse().unwrap()),
            ..Keys::default()
        }),
        b.key(&Keys {
            peer: Some("::ffff:10.0.0.1".parse().unwrap()),
            ..Keys::default()
        })
    );
    // uri_hash counts the query.
    let u = balancer(LoadBalancing::UriHash, &[1; 4], "");
    let diff = (0..20)
        .filter(|i| {
            let (q1, q2) = (format!("a={i}"), format!("a={}", i + 100));
            let k1 = Keys {
                path: "/p",
                query: &q1,
                ..Keys::default()
            };
            let k2 = Keys {
                path: "/p",
                query: &q2,
                ..Keys::default()
            };
            u.key(&k1) != u.key(&k2)
        })
        .count();
    assert_eq!(diff, 20, "uri_hash ignores the query");

    // Cookies: fit's cookieValue.
    let mut h = Headers::new();
    h.append("Cookie", r#"a=1; user="x"; b=2"#);
    h.append("Cookie", "c=3");
    let c = balancer(LoadBalancing::CookieHash, &[1; 4], "user");
    let cookie = |name: &str| {
        let b = balancer(LoadBalancing::CookieHash, &[1; 4], name);
        b.key(&Keys {
            headers: Some(&h),
            ..Keys::default()
        })
    };
    let hashed = |v: &str| {
        let mut h = Headers::new();
        h.append("Cookie", format!("k={v}"));
        balancer(LoadBalancing::CookieHash, &[1; 4], "k").key(&Keys {
            headers: Some(&h),
            ..Keys::default()
        })
    };
    for (name, want) in [("a", "1"), ("user", "x"), ("b", "2"), ("c", "3")] {
        assert_eq!(cookie(name), hashed(want), "cookie {name}");
    }
    assert_eq!(cookie("d"), None);
    assert_eq!(cookie("us"), None);
    drop(c);

    // A request without its key goes in turn.
    for policy in [LoadBalancing::HeaderHash, LoadBalancing::CookieHash] {
        let b = balancer(policy, &[1, 1, 1], hash_key(policy));
        let none = Down::none(3);
        let mut got = [0usize; 3];
        for _ in 0..300 {
            got[pick(&b, policy, "", &none)] += 1;
        }
        assert_eq!(got, [100, 100, 100], "{policy:?} without a key");
        // And without a request at all.
        assert!(b.pick_up(|_| 0, none.up()) < 3);
        assert_eq!(b.key(&Keys::default()), None);
    }
    // header_hash's header is found whatever its case, as fit's canonical key.
    let b = balancer(LoadBalancing::HeaderHash, &[1; 4], "X-USER");
    let r = Req::keyed(LoadBalancing::HeaderHash, "ann");
    let lower = balancer(LoadBalancing::HeaderHash, &[1; 4], "x-user");
    assert_eq!(b.key(&r.keys()), lower.key(&r.keys()));
    assert!(b.key(&r.keys()).is_some());
}

/// fit's own picks for the same keys (a scratch program on fit's `upstream` package at
/// 7daee573): addresses 10.0.0.1:80 to 10.0.0.4:80 of weights 1, 2, 1, 3.
#[test]
fn keys_land_where_fits_do() {
    let addrs = ["10.0.0.1:80", "10.0.0.2:80", "10.0.0.3:80", "10.0.0.4:80"];
    let weights = [1, 2, 1, 3];
    let none = Down::none(4);
    let ip = Balancer::keyed(LoadBalancing::IpHash, &addrs, &weights, "");
    let uri = Balancer::keyed(LoadBalancing::UriHash, &addrs, &weights, "");
    let hdr = Balancer::keyed(LoadBalancing::HeaderHash, &addrs, &weights, "x-user");
    let (mut a, mut b, mut c) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..24 {
        let peer = Keys {
            peer: Some(format!("192.0.2.{i}").parse().unwrap()),
            ..Keys::default()
        };
        a.push(ip.pick_keyed(ip.key(&peer), |_| 0, none.up()));
        let path = format!("/p/{i}");
        let target = Keys {
            path: &path,
            query: "q=1",
            ..Keys::default()
        };
        b.push(uri.pick_keyed(uri.key(&target), |_| 0, none.up()));
        let mut h = Headers::new();
        h.append("X-User", format!("user{i}"));
        let field = Keys {
            headers: Some(&h),
            ..Keys::default()
        };
        c.push(hdr.pick_keyed(hdr.key(&field), |_| 0, none.up()));
    }
    assert_eq!(
        a,
        [
            2, 1, 1, 2, 0, 3, 3, 1, 2, 3, 1, 3, 3, 1, 3, 1, 0, 3, 3, 1, 1, 1, 3, 1
        ]
    );
    assert_eq!(
        b,
        [
            3, 3, 3, 3, 3, 1, 3, 3, 3, 1, 3, 3, 2, 3, 1, 3, 3, 3, 2, 2, 2, 3, 1, 3
        ]
    );
    assert_eq!(
        c,
        [
            1, 3, 1, 1, 1, 0, 3, 3, 1, 1, 3, 2, 1, 2, 3, 1, 1, 2, 0, 3, 2, 2, 2, 0
        ]
    );
    let v6 = Keys {
        peer: Some("2001:db8::1".parse().unwrap()),
        ..Keys::default()
    };
    assert_eq!(ip.pick_keyed(ip.key(&v6), |_| 0, none.up()), 1);
    assert_eq!(sticky_id("10.0.0.1:80"), "e0f2af3b3dabfb17");
}

/// fit's TestHashRingEdges: a ring whose addresses share a name, with every address down.
#[test]
fn hash_ring_edges() {
    let ring = Balancer::keyed(LoadBalancing::IpHash, &["a", "a"], &[1, 1], "");
    let down = Down::none(2);
    down.set(0, true);
    down.set(1, true);
    let keys = Keys {
        peer: Some("192.0.2.1".parse().unwrap()),
        ..Keys::default()
    };
    assert!(ring.pick_keyed(ring.key(&keys), |_| 0, down.up()) < 2);
    let uri = balancer(LoadBalancing::UriHash, &[1, 1], "");
    assert!(uri.pick_up(|_| 0, Down::none(2).up()) < 2);
    // One address: every key goes to it.
    let one = balancer(LoadBalancing::UriHash, &[5], "");
    assert_eq!(pick(&one, LoadBalancing::UriHash, "x", &Down::none(1)), 0);
}

#[test]
fn ring_size_is_bounded() {
    use akit_upstream::balance::{MAX_RING_POINTS, POINTS_PER_WEIGHT};
    assert_eq!(POINTS_PER_WEIGHT, 160);
    // 10,000 addresses of weight 1000 would be 1.6 billion points in fit's ring; akit's holds
    // at most MAX_RING_POINTS, and still spreads keys over every address by weight.
    let names: Vec<String> = (0..2000)
        .map(|i| format!("10.0.{}.{}:80", i / 256, i % 256))
        .collect();
    let weights: Vec<u32> = (0..2000).map(|i| if i < 1000 { 1000 } else { 1 }).collect();
    let b = Balancer::keyed(LoadBalancing::UriHash, &names, &weights, "");
    let none = Down::none(2000);
    let mut heavy = 0;
    for i in 0..10000 {
        let path = format!("/{i}");
        let k = Keys {
            path: &path,
            ..Keys::default()
        };
        if b.pick_keyed(b.key(&k), |_| 0, none.up()) < 1000 {
            heavy += 1;
        }
    }
    // The heavy half weighs 1000 times the light half.
    assert!(heavy > 9900, "{heavy} of 10000 keys to the heavy addresses");
    assert_eq!(MAX_RING_POINTS, 1 << 20);
}

#[test]
fn sticky_cookie_values() {
    let c = akit_config::from_json(
        r#"{"upstreams": [{"name": "app", "addresses": ["a:1", "b:1"],
            "sticky_cookie": {"name": "s", "path": "/app", "secure": true, "max_age": "1h"}}]}"#,
        None,
    )
    .unwrap();
    let u = &c.upstreams[0];
    let s = Sticky::new(u.sticky_cookie.as_ref().unwrap(), &u.addresses);
    assert_eq!(s.name(), "s");
    assert_eq!(
        s.set_cookie(None, 1).unwrap(),
        format!(
            "s={}; Path=/app; Max-Age=3600; HttpOnly; SameSite=Lax; Secure",
            sticky_id("b:1")
        )
    );
    let mut h = Headers::new();
    h.append("Cookie", format!("x=1; s={}", sticky_id("b:1")));
    assert_eq!(s.backend(&h), Some(1));
    // The request's cookie names the address already: none set; another: it is set.
    assert_eq!(s.set_cookie(Some(&h), 1), None);
    assert!(
        s.set_cookie(Some(&h), 0)
            .unwrap()
            .starts_with(&format!("s={};", sticky_id("a:1")))
    );
    // Values that name nothing.
    for v in [
        "nothing",
        "0000",
        "",
        &sticky_id("b:1").to_uppercase(),
        &sticky_id("c:1"),
    ] {
        let mut h = Headers::new();
        h.append("Cookie", format!("s={v}"));
        assert_eq!(s.backend(&h), None, "{v:?}");
    }
    // fit's TestStickyNilRequest: no request, nothing picked.
    assert_eq!(s.backend(&Headers::new()), None);
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

/// An address nothing listens on: port 1 (tcpmux), which nothing serves and no test binds. A
/// port picked by binding port 0 and letting it go may be taken by another process meanwhile.
fn closed_address() -> String {
    "127.0.0.1:1".to_owned()
}

/// fit's TestSticky, on the pool: without a cookie, in turn, each answer naming its address;
/// with it, always there and no cookie set again; its address down, another and a cookie that
/// names it; back once it is up; a value that names nothing goes by the policy.
#[test]
fn pool_sticky() {
    let e = engine();
    // b is closed: one failure takes it out of rotation.
    let addrs = vec!["127.0.0.1:9".to_owned(), closed_address()];
    let pool = Pool::new(
        &upstream(
            &addrs,
            r#""max_fails": 1, "fail_timeout": "1h",
               "sticky_cookie": {"name": "s", "path": "/app", "secure": true}"#,
        ),
        EngineSlot::with(e.clone()),
    );
    assert!(pool.is_sticky());
    let req = |cookie: &str| {
        let mut h = Headers::new();
        if !cookie.is_empty() {
            h.append("Cookie", cookie);
        }
        h
    };
    let pick = |h: &Headers| {
        let keys = Keys {
            headers: Some(h),
            ..Keys::default()
        };
        let b = pool.pick_for(&keys);
        (b, pool.set_cookie(&keys, b).map(str::to_owned))
    };
    let (first, set) = pick(&req(""));
    assert_eq!(
        set.unwrap(),
        format!(
            "s={}; Path=/app; HttpOnly; SameSite=Lax; Secure",
            sticky_id(&addrs[first])
        )
    );
    assert_ne!(
        pick(&req("")).0,
        first,
        "requests without a cookie did not go in turn"
    );

    let to = |b: usize| req(&format!("x=1; s={}", sticky_id(&addrs[b])));
    for _ in 0..10 {
        assert_eq!(pick(&to(1)), (1, None));
        assert_eq!(pick(&to(0)), (0, None));
    }
    // b down (a request to it fails): a request whose cookie names it goes to a, and is told so.
    let (tx, rx) = channel();
    pool.send(1, ClientRequest::get("/"), move |r| {
        let _ = tx.send(r.is_ok());
    });
    assert!(!rx.recv_timeout(Duration::from_secs(10)).unwrap());
    assert!(pool.is_down(1));
    let (other, set) = pick(&to(1));
    assert_eq!(other, 0);
    assert!(
        set.unwrap()
            .starts_with(&format!("s={};", sticky_id(&addrs[0])))
    );
    // A value that names nothing goes by the policy, and gets a cookie.
    let (b, set) = pick(&req("s=nothing"));
    assert_eq!(b, 0, "b is down");
    assert!(set.is_some());
    e.shutdown();
}

/// The pool hashes by the config's policy and key.
#[test]
fn pool_hashes() {
    let addrs: Vec<String> = (0..4).map(|i| format!("10.0.0.{}:80", i + 1)).collect();
    let pool = Pool::new(
        &upstream(
            &addrs,
            r#""load_balancing": "header_hash", "hash_key": "x-user", "weights": [1, 2, 1, 3]"#,
        ),
        EngineSlot::new(),
    );
    assert_eq!(pool.load_balancing(), LoadBalancing::HeaderHash);
    assert!(!pool.is_sticky());
    let mut got = Vec::new();
    for i in 0..24 {
        let mut h = Headers::new();
        h.append("x-user", format!("user{i}"));
        let keys = Keys {
            headers: Some(&h),
            ..Keys::default()
        };
        got.push(pool.pick_for(&keys));
        assert_eq!(pool.set_cookie(&keys, 0), None);
    }
    // fit's picks, as keys_land_where_fits_do has them.
    assert_eq!(
        got,
        [
            1, 3, 1, 1, 1, 0, 3, 3, 1, 1, 3, 2, 1, 2, 3, 1, 1, 2, 0, 3, 2, 2, 2, 0
        ]
    );
    // Without a request, in turn.
    let mut by = [0usize; 4];
    for _ in 0..400 {
        by[pool.pick()] += 1;
    }
    assert_eq!(by, [100; 4]);
}
