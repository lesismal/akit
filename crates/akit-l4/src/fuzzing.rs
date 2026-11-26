//! Fuzz entry points (feature `fuzzing`; not a public API): the `fuzz/` cargo-fuzz project calls
//! them, and the tests below replay `fuzz/corpus/<target>` on stable. fit's `FuzzReadSNI`
//! (`l4/codec_test.go`) as akit reads a ClientHello to pick a service, and the forward proxy's
//! rules on any destination a CONNECT or SOCKS request names. The PROXY protocol and ClientHello
//! parsers themselves are akio-mux's, fuzzed in akio (`proxy_header`, `mux_tls_hello`).
#![allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use akio_mux::tls::{HelloError, parse_client_hello};
use akit_config::{Destination, ForwardProxy, Network};
use akit_upstream::{EngineSlot, Pool};

use crate::forward::{Rules, Verdict, internal, refused};
use crate::relay::Opening;
use crate::{Entry, MAX_CLIENT_HELLO, Service};

/// The services of fit's `TestEntryPick`: `a` for `A.example.com`, `w` for `*.w.example.com` and
/// `x.example.com`, and `f`, the fallback.
fn entry() -> &'static Entry {
    static ENTRY: OnceLock<Entry> = OnceLock::new();
    ENTRY.get_or_init(|| {
        let config = akit_config::from_json(
            r#"{
  "listeners": [{"name": "t", "address": "127.0.0.1:0", "protocols": ["tcp"]}],
  "upstreams": [{"name": "up", "addresses": ["127.0.0.1:1"]}],
  "services": [
    {"name": "a", "listeners": ["t"], "upstream": "up", "server_names": ["A.example.com"]},
    {"name": "w", "listeners": ["t"], "upstream": "up", "server_names": ["*.w.example.com", "x.example.com"]},
    {"name": "f", "listeners": ["t"], "upstream": "up"}
  ]
}"#,
            None,
        )
        .expect("the entry's config");
        let pool = Pool::new(&config.upstreams[0], EngineSlot::new());
        let services: Vec<(Service, Vec<String>)> = config
            .services
            .iter()
            .map(|s| (Service::new(s, pool.clone()), s.server_names.clone()))
            .collect();
        Entry::of(services.iter().map(|(s, n)| (s.clone(), n.as_slice())))
    })
}

/// `l4_hello`: a TCP listener's connection, its ClientHello read as it arrives in pieces
/// (`data[0]` splits the reads), goes to the service its server name picks, or the fallback once
/// it is not a hello or is longer than [`MAX_CLIENT_HELLO`]: the same service however the bytes
/// were split. The services' names compare without case or a trailing dot.
pub fn l4_hello(data: &[u8]) {
    let Some((&split, wire)) = data.split_first() else {
        return;
    };
    let entry = entry();
    let mut o = Opening {
        entry: entry.clone(),
        buf: Vec::new(),
        waiting: Arc::new(Mutex::new(None)),
        timer: None,
    };
    let step = usize::from(split).max(1);
    let mut picked = None;
    let mut fed = 0;
    for chunk in wire.chunks(step) {
        fed += chunk.len();
        if let Some(s) = o.read(chunk) {
            picked = Some(s.map(|s| s.name().to_owned()));
            break;
        }
    }
    let seen = &wire[..fed];
    let want = match parse_client_hello(seen) {
        Ok(h) => Some(h.server_name().map(str::to_owned)),
        Err(HelloError::Incomplete) if seen.len() < MAX_CLIENT_HELLO => None,
        Err(_) => Some(None),
    };
    let want = want.map(|name| entry.pick(name.as_deref()).map(|s| s.name().to_owned()));
    assert_eq!(picked, want, "{fed} bytes read in pieces of {step}");
    assert!(
        o.buf.len() < MAX_CLIENT_HELLO + step,
        "{} bytes kept",
        o.buf.len()
    );

    if let Ok(name) = std::str::from_utf8(wire) {
        let got = entry.named(name).map(Service::name);
        assert_eq!(
            entry.named(&name.to_ascii_uppercase()).map(Service::name),
            got
        );
        if !name.ends_with('.') {
            assert_eq!(entry.named(&format!("{name}.")).map(Service::name), got);
        }
        if let Some(rest) = name.strip_suffix(".w.example.com") {
            if !rest.is_empty() && !rest.contains('.') {
                assert_eq!(got, Some("w"), "{name:?}");
            }
        }
    }
}

/// The rules of a forward proxy, picked by `bits`: an `allow` list of nothing, names, an
/// internal network or a public one; a `deny` list; and a list of ports.
fn rules(bits: u8) -> Arc<Rules> {
    let net = |s: &str| Network::parse(s).expect("a network");
    let allow = match bits & 3 {
        0 => Vec::new(),
        1 => vec![
            Destination::Name("example.com".into()),
            Destination::Suffix(".example.org".into()),
        ],
        2 => vec![Destination::Network(net("10.1.0.0/16"))],
        _ => vec![
            Destination::Network(net("203.0.113.0/24")),
            Destination::Network(net("127.0.0.1/32")),
        ],
    };
    let deny = if bits & 4 != 0 {
        vec![
            Destination::Name("blocked.example.com".into()),
            Destination::Network(net("198.51.100.0/24")),
        ]
    } else {
        Vec::new()
    };
    Rules::new(&ForwardProxy {
        clients: vec![net("0.0.0.0/0")],
        allow,
        deny,
        ports: if bits & 8 != 0 {
            vec![443, 80]
        } else {
            Vec::new()
        },
        connect_timeout: Duration::from_secs(1),
        idle_timeout: Duration::from_secs(1),
    })
}

/// The allow networks of [`rules`]'s `bits`.
fn allow_nets(bits: u8) -> Vec<Network> {
    match bits & 3 {
        2 => vec![Network::parse("10.1.0.0/16").unwrap()],
        3 => vec![
            Network::parse("203.0.113.0/24").unwrap(),
            Network::parse("127.0.0.1/32").unwrap(),
        ],
        _ => Vec::new(),
    }
}

/// `l4_forward`: the forward proxy's rules (I55) on any destination a CONNECT or SOCKS request
/// names, and any address it resolves to. `data[0]` picks the rules, `data[1..3]` the port,
/// `data[3..19]` the address connected to (IPv4 when its first ten bytes are zero) and
/// `data[19..35]` the local one; the rest is the host named. An internal address, or the host's
/// own, is never let through unless `allow` holds it; a denied one never is.
pub fn l4_forward(data: &[u8]) {
    if data.len() < 35 {
        return;
    }
    let bits = data[0];
    let port = u16::from_be_bytes([data[1], data[2]]);
    let addr = |b: &[u8]| -> IpAddr {
        if b[..10].iter().all(|&x| x == 0) {
            IpAddr::V4(Ipv4Addr::new(b[12], b[13], b[14], b[15]))
        } else {
            IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(b).unwrap()))
        }
    };
    let remote = addr(&data[3..19]);
    let local = addr(&data[19..35]);
    let Ok(host) = std::str::from_utf8(&data[35..]) else {
        return;
    };
    let r = rules(bits);
    let allow = allow_nets(bits);
    let v = r.dest_host(host, port);
    if bits & 8 != 0 && port != 443 && port != 80 {
        assert_eq!(v, Verdict::Refused, "port {port}");
    }
    match host.parse::<IpAddr>() {
        Ok(ip) => {
            assert!(
                matches!(v, Verdict::Refused | Verdict::Allowed),
                "{host:?}: {v:?}"
            );
            if refused(ip, None, &allow) {
                assert_eq!(v, Verdict::Refused, "{host:?} is internal");
            }
            if bits & 4 != 0
                && Network::parse("198.51.100.0/24")
                    .unwrap()
                    .contains(unmap(ip))
            {
                assert_eq!(v, Verdict::Refused, "{host:?} is denied");
            }
        }
        Err(_) => {
            assert_ne!(v, Verdict::Allowed, "{host:?}");
            let h = host.trim_end_matches('.').to_ascii_lowercase();
            if h.is_empty() || h.contains([':', '%', '/']) {
                assert_eq!(v, Verdict::Refused, "{host:?}");
            }
            if bits & 4 != 0 && h == "blocked.example.com" {
                assert_eq!(v, Verdict::Refused, "{host:?} is denied");
            }
        }
    }
    let ok = r.resolved(v, remote, Some(local));
    if ok {
        assert_ne!(v, Verdict::Refused);
        let remote = unmap(remote);
        if let Some(n) = internal(remote) {
            assert!(
                allow.iter().any(|a| a.bits >= n.bits && a.contains(remote)),
                "{remote} is internal ({n}) and was let through"
            );
        }
        if unmap(local) == remote {
            let full = if remote.is_ipv4() { 32 } else { 128 };
            assert!(
                allow.iter().any(|a| a.bits >= full && a.contains(remote)),
                "{remote}, the host's own, was let through"
            );
        }
        if bits & 4 != 0 && v != Verdict::Allowed {
            // An address named was checked before it was dialed; a name's, once resolved.
            assert!(!Network::parse("198.51.100.0/24").unwrap().contains(remote));
        }
    }
    let _ = r.client(remote);
}

fn unmap(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        IpAddr::V4(_) => ip,
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
    fn corpus_l4_hello() {
        replay("l4_hello", l4_hello);
    }

    #[test]
    fn corpus_l4_forward() {
        replay("l4_forward", l4_forward);
    }
}
