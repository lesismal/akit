//! fit's `access/access_test.go` and `access/lists_test.go`, ported.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation
)]

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;
use std::sync::Arc;

use akit_config::Access;

use crate::lists::{parse_line, parse_list};
use crate::{List, MAX_LINE_BYTES, Policy, Range, Set, compile};

fn list(allow: &[&str], deny: &[&str]) -> Option<Arc<List>> {
    compile(
        Some(&Access {
            allow: allow.iter().map(|s| (*s).to_owned()).collect(),
            deny: deny.iter().map(|s| (*s).to_owned()).collect(),
            ..Access::default()
        }),
        None,
    )
    .unwrap()
}

fn addr(s: &str) -> IpAddr {
    s.parse().unwrap()
}

/// A small xorshift generator: the tests' random prefixes and addresses, the same each run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A directory of the test's own, removed when the test ends.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let d = std::env::temp_dir().join(format!("akit-access-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }

    /// A file of `content` in it.
    fn file(&self, name: &str, content: &[u8]) -> String {
        let p = self.0.join(name);
        std::fs::write(&p, content).unwrap();
        p.display().to_string()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn chain(lists: &[&Option<Arc<List>>]) -> Option<Arc<Policy>> {
    Policy::chain(lists.iter().map(|l| l.as_ref()))
}

fn allows(p: &Option<Arc<Policy>>, a: &str) -> bool {
    crate::allows(p.as_deref(), addr(a))
}

#[test]
fn policy_layers() {
    let global = list(&[], &["203.0.113.0/24", "2001:db8:bad::/48"]);
    let route_allow = list(&["203.0.113.7", "10.0.0.0/8"], &[]);
    let route_deny = list(&[], &["198.51.100.1"]);
    let whitelist = list(&["192.0.2.0/24"], &[]);
    let fe80 = list(&[], &["fe80::/10"]);
    for (name, p, a, want) in [
        ("no policy serves all", None, "203.0.113.1", true),
        (
            "global deny",
            chain(&[&None, &global]),
            "203.0.113.1",
            false,
        ),
        (
            "global deny v6",
            chain(&[&global]),
            "2001:db8:bad::1",
            false,
        ),
        ("global no say", chain(&[&global]), "198.51.100.1", true),
        (
            "route allow overrides global deny",
            chain(&[&route_allow, &global]),
            "203.0.113.7",
            true,
        ),
        (
            "route allow list refuses others",
            chain(&[&route_allow, &global]),
            "198.51.100.9",
            false,
        ),
        (
            "route deny overrides global allow",
            chain(&[&route_deny, &whitelist]),
            "198.51.100.1",
            false,
        ),
        (
            "route no say falls to global whitelist",
            chain(&[&route_deny, &whitelist]),
            "198.51.100.2",
            false,
        ),
        (
            "route no say, global whitelist serves",
            chain(&[&route_deny, &whitelist]),
            "192.0.2.5",
            true,
        ),
        (
            "mapped v4 counts as v4",
            chain(&[&global]),
            "::ffff:203.0.113.1",
            false,
        ),
        ("link-local denied", chain(&[&fe80]), "fe80::1", false),
    ] {
        assert_eq!(allows(&p, a), want, "{name}: {a}");
    }
}

#[test]
fn chain_empty() {
    assert!(chain(&[&None, &list(&[], &[])]).is_none());
    assert!(compile(None, None).unwrap().is_none());
    assert!(compile(Some(&Access::default()), None).unwrap().is_none());
}

#[test]
fn deny_beats_allow_in_one_list() {
    let p = chain(&[&list(&["10.0.0.0/8"], &["10.1.0.0/16"])]);
    assert!(!allows(&p, "10.1.2.3"));
    assert!(allows(&p, "10.2.2.3"));
}

#[test]
fn edges() {
    let p = chain(&[&list(&[], &["0.0.0.0/0"])]);
    for a in ["0.0.0.0", "255.255.255.255", "1.2.3.4"] {
        assert!(!allows(&p, a), "{a} allowed by a 0.0.0.0/0 deny");
    }
    assert!(allows(&p, "::1"), "::1 refused by an IPv4 deny");
    let p = chain(&[&list(&[], &["::/0"])]);
    for a in [
        "::",
        "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
        "2001:db8::1",
    ] {
        assert!(!allows(&p, a), "{a} allowed by a ::/0 deny");
    }
    let p = chain(&[&list(
        &[],
        &[
            "255.255.255.255",
            "2001:db8::/64",
            "2001:db8:0:1::/64",
            "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
        ],
    )]);
    for (a, want) in [
        ("255.255.255.255", false),
        ("255.255.255.254", true),
        ("2001:db8::ffff", false),
        ("2001:db8:0:1::5", false),
        ("2001:db8:0:2::", true),
        ("2001:db7:ffff::", true),
        ("ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", false),
        ("ffff:ffff:ffff:ffff:ffff:ffff:ffff:fffe", true),
    ] {
        assert_eq!(allows(&p, a), want, "{a}");
    }
}

/// Whether the prefix `net`/`bits` holds `a`, scanned as a linear check would.
fn prefix_holds(net: IpAddr, bits: u8, a: IpAddr) -> bool {
    match (net, a) {
        (IpAddr::V4(n), IpAddr::V4(a)) => {
            let m = u32::MAX.checked_shl(32 - u32::from(bits)).unwrap_or(0);
            u32::from(n) & m == u32::from(a) & m
        }
        (IpAddr::V6(n), IpAddr::V6(a)) => {
            let m = u128::MAX.checked_shl(128 - u32::from(bits)).unwrap_or(0);
            u128::from(n) & m == u128::from(a) & m
        }
        _ => false,
    }
}

/// The merged ranges against a linear scan of the prefixes, on random prefixes and addresses.
#[test]
fn set_against_linear() {
    let mut r = Rng(0x1234_5678_9abc_def1);
    for round in 0..50 {
        let mut ps = Vec::new();
        for _ in 0..1 + r.below(40) {
            if r.below(2) == 0 {
                // A small space, so that ranges meet.
                let b: [u8; 4] = std::array::from_fn(|_| r.below(4) as u8);
                ps.push((IpAddr::V4(Ipv4Addr::from(b)), 16 + r.below(17) as u8));
            } else {
                let b: [u8; 16] = std::array::from_fn(|_| r.below(3) as u8);
                ps.push((IpAddr::V6(Ipv6Addr::from(b)), 100 + r.below(29) as u8));
            }
        }
        let s = Set::new(ps.iter().map(|&(a, b)| Range::of_prefix(a, b)).collect());
        for _ in 0..2000 {
            let a = if r.below(2) == 0 {
                IpAddr::V4(Ipv4Addr::from(std::array::from_fn::<u8, 4, _>(|_| {
                    r.below(4) as u8
                })))
            } else {
                IpAddr::V6(Ipv6Addr::from(std::array::from_fn::<u8, 16, _>(|_| {
                    r.below(3) as u8
                })))
            };
            let want = ps.iter().any(|&(n, b)| prefix_holds(n, b, a));
            assert_eq!(s.contains(a), want, "round {round}: {a}; prefixes {ps:?}");
        }
    }
}

#[test]
fn files() {
    let d = Dir::new("files");
    let content = "; a list\n203.0.113.0/24 ; SBL1\n\n  198.51.100.7  # one\n2001:db8::/32\n";
    let path = d.file("drop.txt", content.as_bytes());
    let mut sums = BTreeMap::new();
    let l = compile(
        Some(&Access {
            deny_files: vec![path.clone()],
            allow: vec!["198.51.100.0/24".into()],
            ..Access::default()
        }),
        Some(&mut sums),
    )
    .unwrap();
    assert_eq!(
        sums.get(&path),
        Some(&akio_crypto::hash::Sha256::digest(content.as_bytes())),
        "the file's sum is not recorded"
    );
    let p = chain(&[&l]);
    for (a, want) in [
        ("203.0.113.9", false),
        ("198.51.100.7", false),
        ("198.51.100.8", true),
        ("2001:db8::1", false),
        ("192.0.2.1", false),
    ] {
        assert_eq!(allows(&p, a), want, "{a}");
    }

    let bad = d.file("bad.txt", b"10.0.0.0/8\nnot-an-ip\n");
    let e = compile(
        Some(&Access {
            allow_files: vec![bad],
            ..Access::default()
        }),
        None,
    )
    .unwrap_err();
    assert!(e[0].msg.contains("bad.txt:2"), "{e:?}");
    assert_eq!(e[0].field, "allow_files[0]");
    let missing = compile(
        Some(&Access {
            deny_files: vec!["x".into(), d.0.join("missing").display().to_string()],
            ..Access::default()
        }),
        None,
    )
    .unwrap_err();
    assert_eq!(missing.len(), 2, "{missing:?}");
    assert_eq!(missing[1].field, "deny_files[1]");
}

#[test]
fn compile_errors() {
    let e = compile(
        Some(&Access {
            allow: vec!["10.0.0.0/33".into()],
            deny: vec!["x".into()],
            ..Access::default()
        }),
        None,
    )
    .unwrap_err();
    assert_eq!(e.len(), 2, "{e:?}");
    assert_eq!(
        (e[0].field.as_str(), e[1].field.as_str()),
        ("allow[0]", "deny[0]")
    );
}

#[test]
fn range_of() {
    for (p, bits, want) in [
        ("10.0.0.0", 8, "10.0.0.0-10.255.255.255"),
        ("10.1.2.3", 32, "10.1.2.3-10.1.2.3"),
        ("0.0.0.0", 0, "0.0.0.0-255.255.255.255"),
        ("10.1.2.3", 20, "10.1.0.0-10.1.15.255"),
        (
            "2001:db8::",
            32,
            "2001:db8::-2001:db8:ffff:ffff:ffff:ffff:ffff:ffff",
        ),
        ("::", 0, "::-ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff"),
        ("2001:db8::1", 128, "2001:db8::1-2001:db8::1"),
        ("::ffff:10.0.0.0", 104, "10.0.0.0-10.255.255.255"),
    ] {
        assert_eq!(
            Range::of_prefix(addr(p), bits).to_string(),
            want,
            "{p}/{bits}"
        );
    }
}

#[test]
fn parse_lines() {
    for (line, want, err) in [
        ("", "", false),
        ("   ", "", false),
        ("# a comment", "", false),
        ("; a comment", "", false),
        ("// a comment", "", false),
        ("203.0.113.7", "203.0.113.7-203.0.113.7", false),
        ("203.0.113.0/24 ; SBL", "203.0.113.0-203.0.113.255", false),
        ("2001:db8::/126 # x", "2001:db8::-2001:db8::3", false),
        ("203.0.113.1-203.0.113.9", "203.0.113.1-203.0.113.9", false),
        (
            "203.0.113.1 - 203.0.113.9",
            "203.0.113.1-203.0.113.9",
            false,
        ),
        (
            "2001:db8::1-2001:db8::ff",
            "2001:db8::1-2001:db8::ff",
            false,
        ),
        // ipfilter.dat, leading zeros, levels.
        (
            "001.002.003.004 - 001.002.003.255 , 000 , Some ISP, Inc",
            "1.2.3.4-1.2.3.255",
            false,
        ),
        ("1.2.3.4 - 1.2.3.5 , 100 , x", "1.2.3.4-1.2.3.5", false),
        (
            "1.2.3.4 - 1.2.3.5 , 127 , at the bound",
            "1.2.3.4-1.2.3.5",
            false,
        ),
        ("1.2.3.4 - 1.2.3.5 , 128 , allowed", "", false),
        ("1.2.3.4 - 1.2.3.5 , 255 , allowed", "", false),
        ("1.2.3.4 - 1.2.3.5 , 256 , x", "", true),
        ("1.2.3.4 - 1.2.3.5 , high , x", "", true),
        ("1.2.3.4 - 1.2.3.5,0", "1.2.3.4-1.2.3.5", false),
        // .p2p, names with colons, commas, dashes and '#'.
        ("Bad Corp:1.2.3.4-1.2.3.255", "1.2.3.4-1.2.3.255", false),
        (
            "Bad: Corp, Inc. #2:5.6.7.8-5.6.7.9",
            "5.6.7.8-5.6.7.9",
            false,
        ),
        ("Foo-Bar, Ltd:9.9.9.0-9.9.9.255", "9.9.9.0-9.9.9.255", false),
        (
            "Leading:010.000.000.001-010.000.000.002",
            "10.0.0.1-10.0.0.2",
            false,
        ),
        // Bad lines.
        ("not-an-ip", "", true),
        ("1.2.3.4-1.2.3", "", true),
        ("1.2.3.9-1.2.3.4", "", true),
        ("1.2.3.4-2001:db8::1", "", true),
        ("1.2.3.256", "", true),
        ("1.2.3.4 - 1.2.3.0 , 0 , backwards", "", true),
        ("name:1.2.3.4", "", true),
        ("1.2.3.0001-1.2.3.5", "", true),
        ("fe80::1%eth0-fe80::2", "", true),
    ] {
        match parse_line(line) {
            Ok(r) => {
                assert!(!err, "parse_line({line:?}) = {r:?}, want an error");
                assert_eq!(
                    r.map(|r| r.to_string()).unwrap_or_default(),
                    want,
                    "{line:?}"
                );
            }
            Err(e) => assert!(err, "parse_line({line:?}): {e}"),
        }
    }
}

/// The .p2p and .dat formats in `deny_files` and `allow_files`, mixed with the plain format,
/// through `compile` and a policy.
#[test]
fn p2p_and_dat_files() {
    let d = Dir::new("formats");
    let p2p = d.file(
        "level1.p2p",
        b"# PeerGuardian list\nSome Org:203.0.113.10-203.0.113.20\nOther, Inc:198.51.100.0-198.51.100.255\n\n",
    );
    let dat = d.file(
        "ipfilter.dat",
        b"// eMule\r\n192.000.002.000 - 192.000.002.127 , 000 , Blocked\r\n192.000.002.128 - 192.000.002.255 , 200 , Not blocked\r\n",
    );
    let mixed = d.file(
        "mixed.txt",
        b"10.0.0.0/8\n2001:db8::1-2001:db8::9\nX:172.16.0.1-172.16.0.1\n",
    );
    let l = compile(
        Some(&Access {
            deny_files: vec![p2p.clone(), dat, mixed],
            ..Access::default()
        }),
        None,
    )
    .unwrap();
    let p = chain(&[&l]);
    for (a, want) in [
        ("203.0.113.9", true),
        ("203.0.113.10", false),
        ("203.0.113.20", false),
        ("203.0.113.21", true),
        ("198.51.100.77", false),
        ("192.0.2.0", false),
        ("192.0.2.127", false),
        ("192.0.2.128", true),
        ("10.9.9.9", false),
        ("2001:db8::5", false),
        ("2001:db8::a", true),
        ("172.16.0.1", false),
        ("172.16.0.2", true),
        ("::ffff:203.0.113.15", false),
    ] {
        assert_eq!(allows(&p, a), want, "{a}");
    }

    // An allow list of .p2p ranges serves only them.
    let l = compile(
        Some(&Access {
            allow_files: vec![p2p],
            ..Access::default()
        }),
        None,
    )
    .unwrap();
    let p = chain(&[&l]);
    assert!(allows(&p, "203.0.113.15") && !allows(&p, "8.8.8.8"));

    // A bad line names its file and line.
    let bad = d.file(
        "bad.dat",
        b"1.2.3.4 - 1.2.3.5 , 0 , ok\n1.2.3.4 - 1.2.3.5 , x , bad level\n",
    );
    let e = compile(
        Some(&Access {
            deny_files: vec![bad],
            ..Access::default()
        }),
        None,
    )
    .unwrap_err();
    assert!(e[0].msg.contains("bad.dat:2"), "{e:?}");
    let mut long = "x".repeat(MAX_LINE_BYTES + 1).into_bytes();
    long.extend_from_slice(b":1.2.3.4-1.2.3.5\n");
    assert!(
        parse_list(&long, "long.p2p").is_err(),
        "a line past MAX_LINE_BYTES was read"
    );
    // A list whose last line has no newline, and a line that is not UTF-8 in a comment.
    let rs = parse_list(b"# caf\xe9\n10.0.0.1", "end.txt").unwrap();
    assert_eq!(rs.len(), 1);
}

/// Ranges against a linear check: a set made of random ranges holds what they hold.
#[test]
fn ranges_against_linear() {
    let mut r = Rng(0x0fed_cba9_8765_4321);
    for round in 0..50 {
        let mut rs = Vec::new();
        for _ in 0..1 + r.below(30) {
            let mut a = Ipv4Addr::new(10, 0, r.below(4) as u8, r.below(256) as u8);
            let mut b = Ipv4Addr::new(10, 0, r.below(4) as u8, r.below(256) as u8);
            if b < a {
                std::mem::swap(&mut a, &mut b);
            }
            rs.push(Range {
                lo: IpAddr::V4(a),
                hi: IpAddr::V4(b),
            });
        }
        let s = Set::new(rs.clone());
        for _ in 0..2000 {
            let a = IpAddr::V4(Ipv4Addr::new(10, 0, r.below(5) as u8, r.below(256) as u8));
            let want = rs.iter().any(|g| g.lo <= a && a <= g.hi);
            assert_eq!(s.contains(a), want, "round {round}: {a}");
        }
    }
}

/// A large list: a hundred thousand prefixes from a file, looked up.
#[test]
fn large_list() {
    let d = Dir::new("large");
    let mut text = String::new();
    for i in 0u32..100_000 {
        text.push_str(&format!(
            "{}.{}.{}.0/24\n",
            1 + ((i >> 16) & 0xff),
            (i >> 8) & 0xff,
            i & 0xff
        ));
    }
    let path = d.file("deny.txt", text.as_bytes());
    let l = compile(
        Some(&Access {
            deny_files: vec![path],
            ..Access::default()
        }),
        None,
    )
    .unwrap();
    let p = chain(&[&l]);
    assert!(allows(&p, "192.0.2.1"));
    assert!(!allows(&p, "1.0.0.7"));
    assert!(!allows(&p, "2.134.159.200"));
}
