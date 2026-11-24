//! The forward proxy (I55): who may use a listener's `socks` or the `forward_proxy` handler's HTTP
//! CONNECT, and where they may be connected to, as fit's `l4.Rules` and `internal/netguard`
//! decide it, on akio-socks's SOCKS server and CONNECT tunnels.
//!
//! A destination is decided twice. Before anything is dialed, by what the client asked for
//! ([`Rules::dest`]): its port must be one of `ports`; an address must not be in `deny`, must be
//! in `allow` when `allow` has entries, and must not be internal (below); a host name must not be
//! one `deny` names, and must be one `allow` names, or `allow` must have networks that its
//! address may fall in. Then, once a connection is open and before anything is sent on it
//! ([`Rules::resolved`], akio-socks's `ProxyConfig::connected`, akio API request A132), by the
//! address the name resolved to for that connection: `deny`'s networks refuse it, and, for a name
//! `allow` does not hold, only `allow`'s networks let it through. Whatever the name, the address
//! is refused when it is internal: loopback, link-local (the clouds' metadata services among
//! them), unspecified, multicast, another metadata address, or the host's own, which a
//! connection shows by coming from the address it goes to; only an `allow` network inside that
//! range (or naming the host's address alone) lets it through, as `127.0.0.0/8` does and
//! `0.0.0.0/0` does not. Deciding the address once connected keeps DNS from answering one way
//! for the check and another for the dial.
//!
//! The SOCKS server serves CONNECT, SOCKS5 without authentication, SOCKS4 and 4a, to the
//! clients `clients` holds; BIND and UDP ASSOCIATE are refused. Its handshake must be done within
//! [`OPENING_TIMEOUT`](crate::OPENING_TIMEOUT) plus the connect timeout, the connect within
//! `connect_timeout` (A133), and a tunnel that carries nothing either way for `idle_timeout` is
//! closed. Name resolution runs off the I/O threads; each side's reads are held while the other
//! has more than akio-socks's watermark (256 KiB) unsent.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use akio::EngineRef;
use akio_socks::{Command, Dest, Destinations, ProxyConfig, Socks};
use akit_config::{Destination, ForwardProxy, Network};

/// What [`Rules::dest`] says of a destination before it is dialed (fit's `verdict`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Refused.
    Refused,
    /// An address the rules allow: only whether it is the host's own is left to decide once
    /// connected.
    Allowed,
    /// A host name whose address still decides: a `deny` network may hold it, or only an
    /// `allow` network may let it through.
    Unresolved,
    /// A host name `allow` holds (or any, without `allow`), which only `deny`'s networks and
    /// the refusal of internal addresses may still refuse.
    ResolvedAllowed,
}

/// Host names, wildcard suffixes and networks: a forward proxy's `allow` or `deny`.
#[derive(Debug, Default)]
struct Dests {
    names: Vec<String>,
    suffixes: Vec<String>,
    nets: Vec<Network>,
}

impl Dests {
    fn new(d: &[Destination]) -> Dests {
        let mut out = Dests::default();
        for x in d {
            match x {
                Destination::Network(n) => out.nets.push(*n),
                Destination::Name(n) => out.names.push(n.clone()),
                Destination::Suffix(s) => out.suffixes.push(s.clone()),
            }
        }
        out
    }

    fn is_empty(&self) -> bool {
        self.names.is_empty() && self.suffixes.is_empty() && self.nets.is_empty()
    }

    /// Whether it holds `host`, a name in lower case without a trailing dot.
    fn name(&self, host: &str) -> bool {
        self.names.iter().any(|n| n == host) || self.suffixes.iter().any(|s| host.ends_with(s))
    }

    /// Whether one of its networks holds `ip`.
    fn addr(&self, ip: IpAddr) -> bool {
        let ip = unmap(ip);
        self.nets.iter().any(|n| n.contains(ip))
    }
}

/// Who may use a forward proxy and where it may connect them, compiled from a listener's `socks`
/// or the `forward_proxy` handler's params (fit's `l4.Rules`).
#[derive(Debug)]
pub struct Rules {
    clients: Vec<Network>,
    allow: Dests,
    deny: Dests,
    ports: Vec<u16>,
    connect: Duration,
    idle: Duration,
}

impl Rules {
    /// The rules of `c`, which akit-config has checked.
    #[must_use]
    pub fn new(c: &ForwardProxy) -> Arc<Rules> {
        Arc::new(Rules {
            clients: c.clients.clone(),
            allow: Dests::new(&c.allow),
            deny: Dests::new(&c.deny),
            ports: c.ports.clone(),
            connect: c.connect_timeout,
            idle: c.idle_timeout,
        })
    }

    /// How long resolving and connecting to a destination may take.
    #[must_use]
    pub fn connect_timeout(&self) -> Duration {
        self.connect
    }

    /// How long a tunnel may carry nothing either way.
    #[must_use]
    pub fn idle_timeout(&self) -> Duration {
        self.idle
    }

    /// Whether a client at `ip` may use the proxy.
    #[must_use]
    pub fn client(&self, ip: IpAddr) -> bool {
        let ip = unmap(ip);
        self.clients.iter().any(|n| n.contains(ip))
    }

    /// What the rules say of the destination `d` before it is dialed.
    #[must_use]
    pub fn dest(&self, d: &Dest) -> Verdict {
        match d {
            Dest::Addr(a) => self.dest_of(None, Some(a.ip()), a.port()),
            Dest::Name(host, port) => self.dest_of(Some(host), None, *port),
        }
    }

    /// The same for `host` and `port` as an HTTP CONNECT's target names them: `host` an IP
    /// address (an IPv6 one without its brackets) or a name.
    #[must_use]
    pub fn dest_host(&self, host: &str, port: u16) -> Verdict {
        match host.parse::<IpAddr>() {
            Ok(ip) => self.dest_of(None, Some(ip), port),
            Err(_) => self.dest_of(Some(host), None, port),
        }
    }

    fn dest_of(&self, name: Option<&str>, ip: Option<IpAddr>, port: u16) -> Verdict {
        if !self.ports.is_empty() && !self.ports.contains(&port) {
            return Verdict::Refused;
        }
        if let Some(ip) = ip {
            return if self.deny.addr(ip)
                || (!self.allow.is_empty() && !self.allow.addr(ip))
                || refused(ip, None, &self.allow.nets)
            {
                Verdict::Refused
            } else {
                Verdict::Allowed
            };
        }
        let host = name
            .unwrap_or_default()
            .trim_end_matches('.')
            .to_ascii_lowercase();
        // An IPv6 address with a zone, or anything else no name is.
        if host.is_empty() || host.contains([':', '%', '/']) {
            return Verdict::Refused;
        }
        if self.deny.name(&host) {
            Verdict::Refused
        } else if self.allow.is_empty() || self.allow.name(&host) {
            Verdict::ResolvedAllowed
        } else if self.allow.nets.is_empty() {
            Verdict::Refused
        } else {
            Verdict::Unresolved
        }
    }

    /// Whether a connection to `remote`, opened from `local` (when known), may carry a tunnel
    /// whose destination `dest` gave verdict `v`.
    #[must_use]
    pub fn resolved(&self, v: Verdict, remote: IpAddr, local: Option<IpAddr>) -> bool {
        let remote = unmap(remote);
        match v {
            Verdict::Refused => return false,
            Verdict::Allowed => {}
            Verdict::Unresolved | Verdict::ResolvedAllowed => {
                if self.deny.addr(remote) || (v == Verdict::Unresolved && !self.allow.addr(remote))
                {
                    return false;
                }
            }
        }
        !refused(remote, local, &self.allow.nets)
    }

    /// akio-socks's config of these rules: no destination filtered before the dial (the access
    /// hook and [`Rules::resolved`] decide), the connect and idle timeouts, and a handshake
    /// timeout of [`OPENING_TIMEOUT`](crate::OPENING_TIMEOUT) plus the connect timeout.
    #[must_use]
    pub fn proxy_config(self: &Arc<Rules>) -> ProxyConfig {
        let mut c = ProxyConfig::default();
        c.destinations = Destinations::Any;
        c.handshake_timeout = Some(crate::OPENING_TIMEOUT.saturating_add(self.connect));
        c.connect_timeout = Some(self.connect);
        c.idle_timeout = Some(self.idle);
        let rules = Arc::clone(self);
        c.connected = Some(Arc::new(
            move |d: &Dest, remote: SocketAddr, local: SocketAddr| {
                rules.resolved(rules.dest(d), remote.ip(), Some(local.ip()))
            },
        ));
        c
    }

    /// The SOCKS server of a listener's `socks`, dialing through `engine`: CONNECT, without
    /// authentication, for the clients and to the destinations the rules allow.
    #[must_use]
    pub fn socks(self: &Arc<Rules>, engine: EngineRef) -> Socks {
        let rules = Arc::clone(self);
        Socks::new(engine)
            .config(self.proxy_config())
            .allow(move |r| {
                r.command() == Command::Connect
                    && rules.client(r.client().ip())
                    && rules.dest(r.dest()) != Verdict::Refused
            })
    }
}

/// The ranges refused unless an `allow` network inside them names the address (fit's
/// `netguard.ranges`).
const INTERNAL: [(IpAddr, u8); 11] = [
    // "This network": a connect to 0.0.0.0 reaches the host itself.
    (IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 8),
    (IpAddr::V4(Ipv4Addr::new(127, 0, 0, 0)), 8),
    // Link-local, with the metadata service of most clouds at 169.254.169.254.
    (IpAddr::V4(Ipv4Addr::new(169, 254, 0, 0)), 16),
    (IpAddr::V4(Ipv4Addr::new(224, 0, 0, 0)), 4),
    (IpAddr::V4(Ipv4Addr::BROADCAST), 32),
    // Alibaba Cloud's metadata service.
    (IpAddr::V4(Ipv4Addr::new(100, 100, 100, 200)), 32),
    (IpAddr::V6(Ipv6Addr::UNSPECIFIED), 128),
    (IpAddr::V6(Ipv6Addr::LOCALHOST), 128),
    (IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0)), 10),
    (IpAddr::V6(Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0)), 8),
    // AWS's metadata service over IPv6.
    (
        IpAddr::V6(Ipv6Addr::new(0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254)),
        128,
    ),
];

/// The internal range `ip` is in, if any (fit's `netguard.Internal`), its prefix length with it.
#[must_use]
pub fn internal(ip: IpAddr) -> Option<Network> {
    let ip = unmap(ip);
    INTERNAL
        .iter()
        .map(|&(addr, bits)| Network { addr, bits })
        .find(|n| n.contains(ip))
}

/// Whether a connection to `remote`, opened from `local` (when known), goes where the config must
/// name to let it (fit's `netguard.Refused`): into an internal range, unless `allow` has a
/// network inside that range holding `remote`; or to the host's own address, `remote` being
/// `local`, unless `allow` has `remote` alone.
#[must_use]
pub fn refused(remote: IpAddr, local: Option<IpAddr>, allow: &[Network]) -> bool {
    let remote = unmap(remote);
    let bits = if let Some(n) = internal(remote) {
        n.bits
    } else if local.is_some_and(|l| unmap(l) == remote) {
        if remote.is_ipv4() { 32 } else { 128 }
    } else {
        return false;
    };
    !allow.iter().any(|n| n.bits >= bits && n.contains(remote))
}

/// `ip`, an IPv4-mapped IPv6 address taken as the IPv4 address it maps.
fn unmap(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        IpAddr::V4(_) => ip,
    }
}

#[cfg(test)]
mod tests {
    //! fit's `l4/forward_test.go` and `internal/netguard/netguard_test.go`.
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;
    use akit_config::DEFAULT_FORWARD_CONNECT_TIMEOUT;

    fn rules(clients: &[&str], allow: &[&str], deny: &[&str], ports: &[u16]) -> Arc<Rules> {
        let dests = |v: &[&str]| {
            v.iter()
                .map(|s| match Network::parse(s) {
                    Some(n) => Destination::Network(n),
                    None => match s.to_ascii_lowercase().strip_prefix('*') {
                        Some(rest) => Destination::Suffix(rest.to_owned()),
                        None => Destination::Name(s.to_ascii_lowercase()),
                    },
                })
                .collect()
        };
        Rules::new(&ForwardProxy {
            clients: clients.iter().map(|s| Network::parse(s).unwrap()).collect(),
            allow: dests(allow),
            deny: dests(deny),
            ports: ports.to_vec(),
            connect_timeout: DEFAULT_FORWARD_CONNECT_TIMEOUT,
            idle_timeout: Duration::from_secs(300),
        })
    }

    /// fit's `dest` of a "host:port" as a CONNECT names it: one that does not split is refused.
    fn dest(r: &Rules, hostport: &str) -> Verdict {
        let Some((host, port)) = hostport.rsplit_once(':') else {
            return Verdict::Refused;
        };
        let Ok(port) = port.parse::<u16>() else {
            return Verdict::Refused;
        };
        r.dest_host(host.trim_start_matches('[').trim_end_matches(']'), port)
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn rules_dest() {
        let r = rules(
            &["10.0.0.0/8", "::1"],
            &["example.com", "*.example.org", "192.0.2.0/24"],
            &["bad.example.org", "192.0.2.66"],
            &[443, 8443],
        );
        for (d, want) in [
            // deny's range may still hold the address an allowed name has.
            ("example.com:443", Verdict::ResolvedAllowed),
            ("EXAMPLE.com.:443", Verdict::ResolvedAllowed),
            ("example.com:80", Verdict::Refused),
            ("www.example.com:443", Verdict::Unresolved), // only a range can allow it
            ("a.b.example.org:443", Verdict::ResolvedAllowed),
            ("example.org:443", Verdict::Unresolved),
            ("bad.example.org:443", Verdict::Refused),
            ("192.0.2.1:8443", Verdict::Allowed),
            ("192.0.2.66:443", Verdict::Refused),
            ("198.51.100.1:443", Verdict::Refused),
            ("[fe80::1%eth0]:443", Verdict::Refused),
            ("nohost", Verdict::Refused),
            ("example.com:x", Verdict::Refused),
        ] {
            assert_eq!(dest(&r, d), want, "{d}");
        }
        for (a, want) in [
            ("192.0.2.9", true),
            ("192.0.2.66", false),
            ("198.51.100.1", false),
        ] {
            assert_eq!(r.resolved(Verdict::Unresolved, ip(a), None), want, "{a}");
        }
        for (a, want) in [
            ("10.1.2.3", true),
            ("::ffff:10.1.2.3", true),
            ("::1", true),
            ("11.0.0.1", false),
        ] {
            assert_eq!(r.client(ip(a)), want, "{a}");
        }
        // As SOCKS asks: by name and by address.
        assert_eq!(
            r.dest(&Dest::Name("Example.COM".into(), 443)),
            Verdict::ResolvedAllowed
        );
        assert_eq!(
            r.dest(&Dest::Addr("192.0.2.1:443".parse().unwrap())),
            Verdict::Allowed
        );

        // Without allow, everything deny does not hold; a deny range makes a name wait for its
        // address.
        let open = rules(&["0.0.0.0/0"], &[], &["127.0.0.0/8"], &[]);
        for (d, want) in [
            ("example.com:1", Verdict::ResolvedAllowed),
            ("127.0.0.1:1", Verdict::Refused),
            ("10.0.0.1:1", Verdict::Allowed),
        ] {
            assert_eq!(dest(&open, d), want, "open: {d}");
        }
        assert!(!open.resolved(Verdict::ResolvedAllowed, ip("127.0.0.1"), None));
        assert!(open.resolved(Verdict::ResolvedAllowed, ip("10.0.0.1"), None));
    }

    /// With allow and deny left out, what a client would reach the gateway's host or the cloud's
    /// metadata service through is refused: the Admin API on 127.0.0.1:2019 among them, by
    /// address, by a name that resolves there and by the host's own address; an allow range
    /// inside one of those ranges lets it through, and a wider one does not.
    #[test]
    fn rules_internal() {
        let open = rules(&["0.0.0.0/0"], &[], &[], &[]);
        for d in [
            "127.0.0.1:2019",
            "[::1]:2019",
            "[::ffff:127.0.0.1]:2019",
            "0.0.0.0:2019",
            "[::]:2019",
            "169.254.169.254:80",
            "[fe80::1]:80",
            "224.0.0.1:80",
            "[ff02::1]:80",
            "100.100.100.200:80",
            "[fd00:ec2::254]:80",
        ] {
            assert_eq!(dest(&open, d), Verdict::Refused, "{d}");
        }
        for d in ["10.0.0.1:443", "192.0.2.1:443", "[2001:db8::1]:443"] {
            assert_eq!(dest(&open, d), Verdict::Allowed, "{d}");
        }
        let me = ip("10.0.0.5");
        for (remote, local, want) in [
            ("127.0.0.1", Some("127.0.0.1"), false), // localhost resolved
            ("::ffff:169.254.169.254", Some("10.0.0.5"), false), // a metadata name resolved
            ("10.0.0.5", Some("10.0.0.5"), false),   // the host's own address
            ("10.0.0.6", Some("10.0.0.5"), true),
            ("192.0.2.1", None, true),
        ] {
            for v in [Verdict::Allowed, Verdict::ResolvedAllowed] {
                assert_eq!(
                    open.resolved(v, ip(remote), local.map(ip)),
                    want,
                    "{v:?} {remote} from {local:?}"
                );
            }
        }

        // An allow range inside an internal range, or the host's address alone, lets it
        // through; one wider does not.
        let opt_in = rules(
            &["0.0.0.0/0"],
            &[
                "0.0.0.0/0",
                "::/0",
                "127.0.0.0/8",
                "169.254.169.254",
                "10.0.0.5",
            ],
            &[],
            &[],
        );
        for (d, want) in [
            ("127.0.0.1:2019", Verdict::Allowed),
            ("169.254.169.254:80", Verdict::Allowed),
            ("169.254.1.1:80", Verdict::Refused),
            ("[::1]:2019", Verdict::Refused),
            ("0.0.0.0:80", Verdict::Refused),
            ("example.com:443", Verdict::Unresolved),
        ] {
            assert_eq!(dest(&opt_in, d), want, "opted in: {d}");
        }
        assert!(opt_in.resolved(Verdict::Unresolved, ip("127.0.0.1"), Some(ip("127.0.0.1"))));
        assert!(opt_in.resolved(Verdict::Allowed, me, Some(me)));
        assert!(!opt_in.resolved(Verdict::Unresolved, ip("::1"), None));
        let wide = rules(&["0.0.0.0/0"], &["10.0.0.0/8"], &[], &[]);
        assert!(!wide.resolved(Verdict::Unresolved, me, Some(me)));
        assert!(wide.resolved(Verdict::Unresolved, ip("10.0.0.6"), Some(me)));
    }

    #[test]
    fn netguard_internal_and_refused() {
        for (a, want) in [
            ("127.0.0.1", "127.0.0.0/8"),
            ("::ffff:127.0.0.1", "127.0.0.0/8"),
            ("0.0.0.0", "0.0.0.0/8"),
            ("169.254.169.254", "169.254.0.0/16"),
            ("100.100.100.200", "100.100.100.200/32"),
            ("224.0.0.1", "224.0.0.0/4"),
            ("255.255.255.255", "255.255.255.255/32"),
            ("::", "::/128"),
            ("::1", "::1/128"),
            ("fe80::1", "fe80::/10"),
            ("ff02::1", "ff00::/8"),
            ("fd00:ec2::254", "fd00:ec2::254/128"),
        ] {
            assert_eq!(
                internal(ip(a)).map(|n| n.to_string()).as_deref(),
                Some(want),
                "{a}"
            );
        }
        for a in [
            "10.0.0.1",
            "192.168.1.1",
            "100.100.100.201",
            "8.8.8.8",
            "2001:db8::1",
            "fd00:ec2::253",
        ] {
            assert_eq!(internal(ip(a)), None, "{a}");
        }
        let nets =
            |v: &[&str]| -> Vec<Network> { v.iter().map(|s| Network::parse(s).unwrap()).collect() };
        let me = Some(ip("10.0.0.5"));
        for (remote, local, allow, want) in [
            ("127.0.0.1", Some(ip("127.0.0.1")), nets(&[]), true),
            ("::ffff:169.254.169.254", me, nets(&[]), true),
            ("8.8.8.8", me, nets(&[]), false),
            ("10.0.0.5", me, nets(&[]), true),
            ("::ffff:10.0.0.5", me, nets(&[]), true),
            ("10.0.0.5", None, nets(&[]), false),
            (
                "127.0.0.1",
                Some(ip("127.0.0.1")),
                nets(&["127.0.0.0/8"]),
                false,
            ),
            (
                "127.0.0.1",
                Some(ip("127.0.0.1")),
                nets(&["127.0.0.1/32"]),
                false,
            ),
            (
                "127.0.0.1",
                Some(ip("127.0.0.1")),
                nets(&["0.0.0.0/0", "::/0"]),
                true,
            ),
            ("169.254.169.254", me, nets(&["169.254.169.254/32"]), false),
            ("10.0.0.5", me, nets(&["10.0.0.0/8"]), true),
            ("10.0.0.5", me, nets(&["10.0.0.5"]), false),
        ] {
            assert_eq!(
                refused(ip(remote), local, &allow),
                want,
                "{remote} from {local:?} {allow:?}"
            );
        }
    }
}
