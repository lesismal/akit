//! Fuzz entry points (feature `fuzzing`; not a public API): the `fuzz/` cargo-fuzz project calls
//! them, and the tests below replay `fuzz/corpus/<target>` on stable. What picks an upstream's
//! address from a request: the hash policies' keys and the sticky cookie.
#![allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::panic,
    clippy::expect_used
)]

use std::net::IpAddr;
use std::time::Duration;

use akio_http::Headers;
use akit_config::{LoadBalancing, StickyCookie};

use crate::balance::{Balancer, Keys};
use crate::sticky::{Sticky, sticky_id};

const POLICIES: [LoadBalancing; 9] = [
    LoadBalancing::RoundRobin,
    LoadBalancing::WeightedRoundRobin,
    LoadBalancing::LeastConn,
    LoadBalancing::Random,
    LoadBalancing::P2c,
    LoadBalancing::IpHash,
    LoadBalancing::UriHash,
    LoadBalancing::HeaderHash,
    LoadBalancing::CookieHash,
];

/// `upstream_keys`: `data[0]` picks the policy, `data[1]` how many addresses (1 to 8),
/// `data[2]` which are up, `data[3..11]` their weights; then NUL-separated: the path, the query,
/// a `Cookie` field, an `X-Key` field and the client's address. The address picked is one of
/// the upstream's, and up when any is; the sticky cookie set for an address names it back.
pub fn upstream_keys(data: &[u8]) {
    if data.len() < 11 {
        return;
    }
    let policy = POLICIES[usize::from(data[0]) % POLICIES.len()];
    let n = usize::from(data[1] % 8) + 1;
    let up_mask = data[2];
    let weights: Vec<u32> = data[3..3 + n].iter().map(|w| u32::from(w % 16)).collect();
    let Ok(text) = std::str::from_utf8(&data[11..]) else {
        return;
    };
    let mut p = text.split('\0');
    let mut next = || p.next().unwrap_or("");
    let (path, query, cookie, key, peer) = (next(), next(), next(), next(), next());
    let addresses: Vec<String> = (0..n).map(|i| format!("10.0.0.{i}:80")).collect();
    let hash_key = if policy == LoadBalancing::CookieHash {
        "session"
    } else {
        "X-Key"
    };
    let b = Balancer::keyed(policy, &addresses, &weights, hash_key);
    let mut h = Headers::new();
    h.append("Cookie", cookie);
    h.append("X-Key", key);
    let keys = Keys {
        peer: peer.parse::<IpAddr>().ok(),
        path,
        query,
        headers: Some(&h),
    };
    let k = b.key(&keys);
    let up = |i: usize| up_mask & (1 << i) != 0;
    let any_up = (0..n).any(up);
    for _ in 0..3 {
        let i = b.pick_keyed(k, |i| i * 7 % 3, up);
        assert!(i < n, "picked {i} of {n}");
        assert!(!any_up || up(i), "{policy:?} picked {i}, which is down");
    }
    if policy.is_hash() && k.is_some() {
        let all = |_| true;
        assert_eq!(b.pick_keyed(k, |_| 0, all), b.pick_keyed(k, |_| 0, all));
    }

    let s = Sticky::new(
        &StickyCookie {
            name: "akit_sticky".into(),
            path: String::new(),
            max_age: Some(Duration::from_secs(60)),
            secure: false,
        },
        &addresses,
    );
    if let Some(i) = s.backend(&h) {
        assert!(i < n);
        assert!(cookie.contains(&sticky_id(&addresses[i])));
        assert_eq!(s.set_cookie(Some(&h), i), None);
    }
    for (i, a) in addresses.iter().enumerate() {
        let mut c = Headers::new();
        c.append("Cookie", format!("a=b; akit_sticky=\"{}\"", sticky_id(a)));
        assert_eq!(s.backend(&c), Some(i));
        let set = s.set_cookie(None, i).unwrap();
        assert!(
            set.starts_with(&format!("akit_sticky={}", sticky_id(a))),
            "{set}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `f` on every file of `fuzz/corpus/<target>`.
    fn replay(target: &str, f: fn(&[u8])) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fuzz/corpus")
            .join(target);
        let mut n = 0;
        for e in std::fs::read_dir(&dir).unwrap() {
            f(&std::fs::read(e.unwrap().path()).unwrap());
            n += 1;
        }
        assert!(n > 0, "{} is empty", dir.display());
    }

    #[test]
    fn corpus_upstream_keys() {
        replay("upstream_keys", upstream_keys);
    }
}
