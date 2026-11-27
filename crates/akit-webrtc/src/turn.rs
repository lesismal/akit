//! The TURN server, `webrtc.turn` (I111, fit's `webrtc/turn`): akio-turn's `TurnServer` on the
//! listeners' engine, with the member's users, secret, relay addresses, quotas and peer rules.
//!
//! The server listens on a UDP listener of the engine for each address of `listen`, on a TCP one
//! for each of `listen_tcp` and on a TLS one for each of `listen_tls` (akio-turn's `stream()`,
//! under akio-tls with the certificates from files); each allocation's relayed socket is bound by
//! akio-turn on the engine. What a reload may change in place is changed in place, as fit's: the
//! users and the secret (the server's `AuthFn` reads them), the peer rules (its peer filter reads
//! them on every request and datagram), the access (the listeners' accept filters, set again by
//! every apply) and the certificates (read on every handshake). A change of what akio-turn's
//! server is made with starts a new server: beside the old one, which keeps what it holds for
//! `-shutdown-timeout` and then closes (a drain), or, when the two share an address, after the
//! old one has closed.
//!
//! On akio's A256-A258 (I112): the relayed sockets bound in `port_min`..`port_max`, nonces made
//! with `nonce_secret`, a stream's messages bounded by `max_stream_message`; every relayed datagram
//! counted against its username's `user_bandwidth` (fit's limiter, [`limiter`]); the server's
//! events counted and logged; the quotas, caps and lifetimes set on the running server; a server a
//! reload replaced drained (new allocations refused with 508); the server's own addresses reached
//! on its relayed ports and on those `allowed_peers` lists.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, Weak};
use std::time::Duration;

use akio::tls::{CertAlert, CertDone, CertResolver, ClientHelloInfo};
use akio::{Listener, ListenerId};
use akio_turn::{AuthFn, FreeReason, TurnConfig, TurnEvent, TurnEventKind, TurnStats};
use akit_config::webrtc::{PeerRule, TurnServer as Member};
use akit_config::{Config, Tls, TlsVersion};
use akit_observe::metrics::TextWriter;
use akit_roles::host::{Host, Info, MetricsFn, Role};
use akit_tls::ListenerTls;

/// `webrtc.turn`.
pub const TURN: Info = Info {
    id: "webrtc.turn",
    set: |c| c.webrtc.as_ref().is_some_and(|w| w.turn.is_some()),
    new,
};

/// Sent as `SOFTWARE` in the server's responses: akit's (fit's `fit`, decision 039).
pub const SOFTWARE: &str = "akit";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn read<T: Clone>(l: &RwLock<T>) -> T {
    l.read().unwrap_or_else(PoisonError::into_inner).clone()
}

fn write<T>(l: &RwLock<T>, v: T) {
    *l.write().unwrap_or_else(PoisonError::into_inner) = v;
}

/// A local address to listen on: `:port` every address, `localhost` the loopback one.
pub(crate) fn listen_addr(a: &str) -> Option<SocketAddr> {
    if let Ok(s) = a.parse() {
        return Some(s);
    }
    let (host, port) = a.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let ip = match host.trim_start_matches('[').trim_end_matches(']') {
        "" => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        h if h.eq_ignore_ascii_case("localhost") => IpAddr::V4(Ipv4Addr::LOCALHOST),
        h => h.parse().ok()?,
    };
    Some(SocketAddr::new(ip, port))
}

fn unmap(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    }
}

/// The policy of a role's own `access`, then the config's global one.
pub(crate) fn policy(
    own: Option<&akit_config::Access>,
    global: Option<&akit_config::Access>,
) -> Result<Option<Arc<akit_access::Policy>>, String> {
    let err = |es: Vec<akit_access::Error>| {
        es.iter()
            .map(|e| format!("access.{}: {}", e.field, e.msg))
            .collect::<Vec<_>>()
            .join("; ")
    };
    let own = akit_access::compile(own, None).map_err(err)?;
    let global = akit_access::compile(global, None).map_err(err)?;
    Ok(akit_access::Policy::chain([own.as_ref(), global.as_ref()]))
}

/// The credentials the servers' `AuthFn` reads, swapped whole by an apply.
struct Creds {
    realm: String,
    keys: HashMap<String, [u8; 16]>,
    secret: Option<AuthFn>,
}

impl Creds {
    fn of(m: &Member) -> Creds {
        let keys = m
            .users
            .iter()
            .filter_map(|u| {
                let key = if u.password.is_empty() {
                    u.key_bytes()?
                } else {
                    akio_turn::stun::long_term_key(&u.name, &m.realm, &u.password)
                };
                Some((u.name.clone(), key))
            })
            .collect();
        Creds {
            realm: m.realm.clone(),
            keys,
            secret: (!m.secret.is_empty()).then(|| akio_turn::ephemeral_auth(m.secret.as_bytes())),
        }
    }

    /// A user of the config's, then the secret's time-limited ones, in the server's realm.
    fn key(&self, user: &str, realm: &str) -> Option<[u8; 16]> {
        if realm != self.realm {
            return None;
        }
        if let Some(k) = self.keys.get(user) {
            return Some(*k);
        }
        self.secret.as_ref().and_then(|s| s(user, realm))
    }
}

/// The peer rules the servers' peer filter reads, swapped whole by an apply.
#[derive(Default)]
struct Rules {
    allowed: Vec<PeerRule>,
    denied: Vec<PeerRule>,
}

/// What every server of the role shares, read on the I/O threads.
struct Shared {
    creds: RwLock<Arc<Creds>>,
    rules: RwLock<Arc<Rules>>,
    /// The servers' own addresses, `relay_ip` and `public_ip` of every server running and
    /// draining: peers there are reached on the ports `allowed_peers` names alone.
    own: RwLock<Arc<Vec<IpAddr>>>,
    certs: RwLock<Option<Arc<ListenerTls>>>,
    /// The servers running and draining, by their ID, for the metrics.
    live: Mutex<Vec<(u64, akio_turn::TurnServer)>>,
    /// The next server's ID.
    next: AtomicU64,
    /// What the servers no longer running counted, which the counters carry on from.
    retired: Mutex<TurnStats>,
    restarts: AtomicU64,
    /// Each username's relayed bytes a second each way, `user_bandwidth`.
    limiter: crate::limiter::Limiter,
    /// `user_bandwidth`, read for every relayed datagram.
    rate: AtomicU64,
    /// Datagrams dropped past a username's bandwidth.
    dropped: AtomicU64,
    /// akio-turn's events, counted.
    ev: Events,
}

/// fit's `refusedCodes`: the codes `akit_turn_refused_total` names; any other is `other`.
const REFUSED_CODES: [u16; 9] = [400, 403, 420, 437, 441, 442, 443, 486, 508];

/// akio-turn's events, counted for `/metrics`.
#[derive(Default)]
struct Events {
    allocated: AtomicU64,
    refreshed: AtomicU64,
    auth_failed: AtomicU64,
    /// By [`FreeReason`]: deleted, expired, closed.
    freed: [AtomicU64; 3],
    /// By [`REFUSED_CODES`], then other.
    refused: [AtomicU64; 10],
}

impl Shared {
    fn new() -> Shared {
        Shared {
            creds: RwLock::new(Arc::new(Creds {
                realm: String::new(),
                keys: HashMap::new(),
                secret: None,
            })),
            rules: RwLock::new(Arc::new(Rules::default())),
            own: RwLock::new(Arc::new(Vec::new())),
            certs: RwLock::new(None),
            live: Mutex::new(Vec::new()),
            next: AtomicU64::new(0),
            retired: Mutex::new(TurnStats::default()),
            restarts: AtomicU64::new(0),
            limiter: crate::limiter::Limiter::default(),
            rate: AtomicU64::new(akit_config::webrtc::DEFAULT_TURN_USER_BANDWIDTH),
            dropped: AtomicU64::new(0),
            ev: Events::default(),
        }
    }

    /// fit's `peerAllowed`, port-aware: a permission is asked with port 0, a datagram and a
    /// channel with the peer's port. The servers' own addresses, `relay_ip` and `public_ip`, are
    /// reached on the ports of the servers' relayed addresses (so two clients relay to each
    /// other) and on those `allowed_peers` names with them, never on the others, akit's own
    /// listeners among them; other peers go by `allowed_peers` first, then `denied_peers` and
    /// akio-turn's denial of internal addresses.
    fn peer_allowed(&self, peer: SocketAddr) -> bool {
        let ip = unmap(peer.ip());
        let port = peer.port();
        let r = read(&self.rules);
        if read(&self.own).contains(&ip) {
            if port == 0 {
                return true;
            }
            let at = SocketAddr::new(ip, port);
            return lock(&self.live).iter().any(|(_, s)| s.holds_relay(at))
                || r.allowed.iter().any(|a| a.port == port && a.contains(ip));
        }
        if r.allowed
            .iter()
            .any(|a| a.contains(ip) && (a.port == 0 || port == 0 || a.port == port))
        {
            return true;
        }
        if r.denied
            .iter()
            .any(|d| d.contains(ip) && (d.port == 0 || d.port == port))
        {
            return false;
        }
        !akio_turn::denied_by_default(ip)
    }

    fn set_own(&self, running: &[&Running]) {
        let mut own = Vec::new();
        for r in running {
            own.extend(r.member.relay_ip.map(unmap));
            own.extend(r.member.public_ip.map(unmap));
        }
        own.sort_unstable();
        own.dedup();
        write(&self.own, Arc::new(own));
    }

    fn stats(&self) -> (TurnStats, usize) {
        let live = lock(&self.live).clone();
        let mut st = *lock(&self.retired);
        for (_, s) in &live {
            add(&mut st, &s.stats());
        }
        (st, live.len())
    }

    fn retire(&self, id: u64, server: &akio_turn::TurnServer) {
        let st = server.stats();
        lock(&self.live).retain(|(i, _)| *i != id);
        let mut r = lock(&self.retired);
        add(&mut r, &st);
        // The gauges: the retired server's are gone.
        r.allocations = 0;
        r.pending = 0;
        r.permissions = 0;
        r.channels = 0;
    }
}

/// Adds `st`'s counters to `into`'s, saturating (akio-turn's `AddAssign` wraps on overflow).
fn add(into: &mut TurnStats, st: &TurnStats) {
    into.allocations = into.allocations.saturating_add(st.allocations);
    into.pending = into.pending.saturating_add(st.pending);
    into.permissions = into.permissions.saturating_add(st.permissions);
    into.channels = into.channels.saturating_add(st.channels);
    into.dropped = into.dropped.saturating_add(st.dropped);
    into.to_peer_bytes = into.to_peer_bytes.saturating_add(st.to_peer_bytes);
    into.to_client_bytes = into.to_client_bytes.saturating_add(st.to_client_bytes);
}

impl Shared {
    /// `on_relay`: counts a datagram against its username's bandwidth, dropping it past it.
    fn relay(&self, user: &str, to_peer: bool, n: usize) -> bool {
        let rate = self.rate.load(Ordering::Relaxed);
        if self
            .limiter
            .allow(user, to_peer, n, rate, std::time::Instant::now())
        {
            true
        } else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            false
        }
    }

    /// `on_event`: counts the event, and logs it at debug. It runs after the answer, with no
    /// lock of akio-turn's held; a line is written only when the `webrtc` logger is at debug.
    fn event(&self, e: &TurnEvent) {
        let m = &self.ev;
        let relay = e.relay.map(|r| r.to_string()).unwrap_or_default();
        match &e.kind {
            TurnEventKind::Allocated => {
                m.allocated.fetch_add(1, Ordering::Relaxed);
                akit_observe::debug!(
                    Webrtc,
                    "webrtc.turn: allocated {relay} for {:?}, from {}, for {:?}",
                    e.user,
                    e.client,
                    e.lifetime
                );
            }
            TurnEventKind::Refreshed => {
                m.refreshed.fetch_add(1, Ordering::Relaxed);
                akit_observe::debug!(
                    Webrtc,
                    "webrtc.turn: refreshed {relay} for {:?}, for {:?}",
                    e.user,
                    e.lifetime
                );
            }
            TurnEventKind::Freed {
                reason,
                to_peer,
                to_client,
            } => {
                let i = match reason {
                    FreeReason::Deleted => 0,
                    FreeReason::Expired => 1,
                    _ => 2,
                };
                if let Some(c) = m.freed.get(i) {
                    c.fetch_add(1, Ordering::Relaxed);
                }
                akit_observe::debug!(
                    Webrtc,
                    "webrtc.turn: freed {relay} of {:?}, {reason}, having relayed {} bytes to \
                     peers and {} to the client",
                    e.user,
                    to_peer.bytes,
                    to_client.bytes
                );
            }
            TurnEventKind::Refused { method, code } => {
                let i = REFUSED_CODES
                    .iter()
                    .position(|c| c == code)
                    .unwrap_or(REFUSED_CODES.len());
                if let Some(c) = m.refused.get(i) {
                    c.fetch_add(1, Ordering::Relaxed);
                }
                akit_observe::debug!(
                    Webrtc,
                    "webrtc.turn: refused a {method:?} of {:?} from {} with {code}",
                    e.user,
                    e.client
                );
            }
            TurnEventKind::AuthFailed { code, .. } => {
                m.auth_failed.fetch_add(1, Ordering::Relaxed);
                akit_observe::debug!(
                    Webrtc,
                    "webrtc.turn: credentials of {:?} from {} failed with {code}",
                    e.user,
                    e.client
                );
            }
            _ => {}
        }
    }
}

/// The lifetimes, quotas and caps of `m`, which a reload sets on the running server.
fn limits(server: &akio_turn::TurnServer, m: &Member) -> akio_turn::TurnLimits {
    let mut l = server.limits();
    l.default_lifetime = m.lifetime();
    l.max_lifetime = m.longest_lifetime().max(m.lifetime());
    l.nonce_lifetime = m.nonce_life();
    l.max_allocations = m.max_allocs();
    l.max_allocations_per_user = m.max_allocs_per_user();
    l.max_permissions = m.max_perms();
    l.max_channels = m.max_chans();
    l
}

/// The certificates of `listen_tls`, chosen per handshake from those the last apply loaded.
struct Certs(Weak<Shared>);

impl CertResolver for Certs {
    fn resolve(&self, hello: &ClientHelloInfo<'_>, done: CertDone) {
        match self.0.upgrade().and_then(|s| read(&s.certs)) {
            Some(t) => t.resolve(hello, done),
            None => done.fail(CertAlert::HandshakeFailure),
        }
    }
}

/// An akio-turn server serving one member's static settings.
struct Running {
    id: u64,
    key: String,
    member: Member,
    server: akio_turn::TurnServer,
    listeners: Vec<ListenerId>,
    udp: Vec<SocketAddr>,
    tcp: Vec<SocketAddr>,
    tls: Vec<SocketAddr>,
}

impl Running {
    /// Whether it listens on `address` over TCP when `stream`, UDP otherwise, as the member
    /// writes it or as it is bound.
    fn holds(&self, address: &str, stream: bool) -> bool {
        if address.ends_with(":0") {
            return false; // a port of the kernel's, never this server's
        }
        let (written, bound): (Vec<&String>, Vec<&SocketAddr>) = if stream {
            (
                self.member
                    .listen_tcp
                    .iter()
                    .chain(&self.member.listen_tls)
                    .collect(),
                self.tcp.iter().chain(&self.tls).collect(),
            )
        } else {
            (
                self.member.listen.iter().collect(),
                self.udp.iter().collect(),
            )
        };
        written.iter().any(|w| *w == address)
            || listen_addr(address).is_some_and(|a| bound.contains(&&a))
    }

    fn shares_addr(&self, m: &Member) -> bool {
        m.listen.iter().any(|a| self.holds(a, false))
            || m.listen_tcp
                .iter()
                .chain(&m.listen_tls)
                .any(|a| self.holds(a, true))
    }

    fn describe(&self) -> String {
        let list = |v: &[SocketAddr]| {
            v.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut s = list(&self.udp);
        if !self.tcp.is_empty() {
            s.push_str(", tcp ");
            s.push_str(&list(&self.tcp));
        }
        if !self.tls.is_empty() {
            s.push_str(", tls ");
            s.push_str(&list(&self.tls));
        }
        s
    }
}

/// What identifies an akio-turn server's settings: two members with the same key are served by
/// one server, their other settings changed in place (the credentials, peer rules, access,
/// certificates, bandwidth, quotas, caps and lifetimes).
fn static_key(m: &Member) -> String {
    format!(
        "{:?} {:?} {:?} {:?} {:?} {:?} {}-{} {:?} {} {:?} {}",
        m.listen,
        m.listen_tcp,
        m.listen_tls,
        m.realm,
        m.relay_ip,
        m.public_ip,
        m.port_min,
        m.port_max,
        m.nonce_secret,
        m.stream_message(),
        m.stream_idle(),
        m.mptcp,
    )
}

fn relay_from(m: &Member) -> String {
    let mut s = m.relay_ip.map(|i| i.to_string()).unwrap_or_default();
    if let Some(p) = m.public_ip {
        s.push_str(&format!(" ({p} to clients)"));
    }
    s
}

/// The role.
struct Turn {
    host: Host,
    shared: Arc<Shared>,
    cur: Option<Running>,
    /// The servers a reload replaced, draining until their timer closes them.
    draining: Arc<Mutex<Vec<Running>>>,
    access: Option<Arc<akit_access::Policy>>,
}

fn new(host: Host) -> Result<Box<dyn Role>, String> {
    Ok(Box::new(Turn {
        host,
        shared: Arc::new(Shared::new()),
        cur: None,
        draining: Arc::new(Mutex::new(Vec::new())),
        access: None,
    }))
}

impl Turn {
    fn filter(&self) -> Option<impl Fn(&SocketAddr) -> bool + Send + Sync + 'static> {
        let p = self.access.clone()?;
        Some(move |peer: &SocketAddr| p.allows(peer.ip()))
    }

    /// Sets the access's filter on every listener of `r`.
    fn refilter(&self, r: &Running) {
        for id in &r.listeners {
            let _ = match self.filter() {
                Some(f) => self.host.engine.set_listener_filter(*id, f),
                None => self.host.engine.clear_listener_filter(*id),
            };
        }
    }

    fn start(&self, m: &Member, key: String) -> Result<Running, String> {
        let shared = Arc::downgrade(&self.shared);
        let auth: AuthFn = Arc::new(move |user: &str, realm: &str| {
            shared
                .upgrade()
                .and_then(|s| read(&s.creds).key(user, realm))
        });
        let relay_ip = m.relay_ip.ok_or("relay_ip is not an IP address")?;
        let mut tc = TurnConfig::new(&m.realm, auth, unmap(relay_ip))
            .lifetimes(m.lifetime(), m.longest_lifetime())
            // fit's server relays UDP alone: TCP allocations (RFC 6062) are refused with 442.
            .tcp_relays(false);
        if let Some(p) = m.public_ip {
            tc = tc.public_ip(unmap(p));
        }
        let shared = Arc::downgrade(&self.shared);
        tc = tc.peer_allowed(move |peer| shared.upgrade().is_some_and(|s| s.peer_allowed(peer)));
        if m.port_max != 0 {
            tc = tc.relay_ports(m.port_min, m.port_max);
        }
        if !m.nonce_secret.is_empty() {
            tc = tc.nonce_secret(m.nonce_secret.as_bytes());
        }
        tc = tc.max_stream_message(m.stream_message());
        let shared = Arc::downgrade(&self.shared);
        tc = tc.on_relay(move |user, to_peer, n| {
            shared.upgrade().is_none_or(|s| s.relay(user, to_peer, n))
        });
        let shared = Arc::downgrade(&self.shared);
        tc = tc.on_event(move |e| {
            if let Some(s) = shared.upgrade() {
                s.event(e);
            }
        });
        tc.software = Some(SOFTWARE.to_owned());
        tc.nonce_lifetime = m.nonce_life();
        tc.max_allocations = m.max_allocs();
        tc.max_allocations_per_user = m.max_allocs_per_user();
        tc.max_permissions = m.max_perms();
        tc.max_channels = m.max_chans();
        tc.stream_allocate_timeout = m.stream_idle();
        let server = akio_turn::TurnServer::new(tc);
        server.set_engine(&self.host.engine);
        let id = self.shared.next.fetch_add(1, Ordering::Relaxed);
        let mut r = Running {
            id,
            key,
            member: m.clone(),
            server: server.clone(),
            listeners: Vec::new(),
            udp: Vec::new(),
            tcp: Vec::new(),
            tls: Vec::new(),
        };
        // Live before it serves, so that the peer checks see its addresses.
        lock(&self.shared.live).push((id, server.clone()));
        let tls = if m.listen_tls.is_empty() {
            None
        } else {
            let cfg = ListenerTls::load(&tls_of(m), &[])
                .and_then(|t| t.server_configs(Arc::new(Certs(Arc::downgrade(&self.shared)))))
                .map_err(|e| format!("tls: {e}"));
            match cfg {
                Ok(c) => Some(c.tcp),
                Err(e) => {
                    self.close(&r);
                    return Err(e);
                }
            }
        };
        let kinds: [(&Vec<String>, u8); 3] =
            [(&m.listen, 0), (&m.listen_tcp, 1), (&m.listen_tls, 2)];
        for (list, kind) in kinds {
            for a in list {
                let Some(addr) = listen_addr(a) else {
                    self.close(&r);
                    return Err(format!("{a:?} is not an address to listen on"));
                };
                let mut l = match (kind, &tls) {
                    (0, _) => Listener::udp(addr).serve(server.clone()),
                    (1, _) => Listener::tcp(addr).serve(server.stream()),
                    (_, Some(t)) => Listener::tcp(addr)
                        .serve(akio::tls::Tls::server(t.clone(), server.stream())),
                    (_, None) => continue,
                };
                if let Some(f) = self.filter() {
                    l = l.filter(f);
                }
                let what = ["listen", "listen_tcp", "listen_tls"]
                    .get(usize::from(kind))
                    .copied()
                    .unwrap_or("listen");
                match self.host.engine.add_listener(l) {
                    Ok(id) => {
                        r.listeners.push(id);
                        let bound = self
                            .host
                            .engine
                            .listeners()
                            .into_iter()
                            .find(|i| i.id == id)
                            .map_or(addr, |i| i.local_addr);
                        match kind {
                            0 => r.udp.push(bound),
                            1 => r.tcp.push(bound),
                            _ => r.tls.push(bound),
                        }
                    }
                    Err(e) => {
                        self.close(&r);
                        return Err(format!("{what} on {a}: {e}"));
                    }
                }
            }
        }
        Ok(r)
    }

    /// Closes `r` at once: its listeners removed, its allocations freed.
    fn close(&self, r: &Running) {
        for id in &r.listeners {
            let _ = self.host.engine.shutdown_listener(*id, Duration::ZERO);
        }
        r.server.close_all();
        self.shared.retire(r.id, &r.server);
    }

    /// Keeps `r` serving what it holds beside its successor for the drain, then closes it.
    fn drain(&self, r: Running) {
        let drain = self.host.drain;
        let addrs = r.describe();
        let id = r.id;
        let allocations = r.server.allocations();
        // New allocations refused with 508 while those held run out.
        r.server.drain();
        lock(&self.draining).push(r);
        let draining = Arc::downgrade(&self.draining);
        let shared = Arc::downgrade(&self.shared);
        let engine = Arc::downgrade(&self.host.engine);
        akit_observe::info!(
            Webrtc,
            "webrtc.turn: the server on {addrs} drains its {allocations} allocations for up to {drain:?}"
        );
        let _ = akio::timer::after_send(drain, move || {
            let (Some(d), Some(s), Some(e)) =
                (draining.upgrade(), shared.upgrade(), engine.upgrade())
            else {
                return;
            };
            let gone: Vec<Running> = {
                let mut d = lock(&d);
                let (old, keep): (Vec<_>, Vec<_>) = d.drain(..).partition(|r| r.id == id);
                *d = keep;
                old
            };
            for r in gone {
                for id in &r.listeners {
                    let _ = e.shutdown_listener(*id, Duration::ZERO);
                }
                r.server.close_all();
                s.retire(r.id, &r.server);
                akit_observe::info!(
                    Webrtc,
                    "webrtc.turn: the server on {addrs} drained; its allocations left ended"
                );
            }
        });
    }

    fn set_own(&self) {
        let d = lock(&self.draining);
        let mut all: Vec<&Running> = d.iter().collect();
        if let Some(c) = &self.cur {
            all.push(c);
        }
        self.shared.set_own(&all);
    }
}

/// `webrtc.turn.tls` as a listener's TLS, its certificates alone, for akit-tls to load.
fn tls_of(m: &Member) -> Tls {
    Tls {
        certificates: m
            .tls
            .as_ref()
            .map(|t| t.certificates.clone())
            .unwrap_or_default(),
        client_auth: None,
        max_version: TlsVersion::Tls13,
        acme: Vec::new(),
        on_demand: false,
    }
}

impl Role for Turn {
    fn apply(&mut self, config: &Config) -> Result<(), String> {
        let Some(m) = config.webrtc.as_ref().and_then(|w| w.turn.clone()) else {
            return Ok(());
        };
        let access = policy(m.access.as_ref(), config.access.as_ref())?;
        let certs = if m.listen_tls.is_empty() {
            None
        } else {
            Some(Arc::new(
                ListenerTls::load(&tls_of(&m), &[]).map_err(|e| format!("tls: {e}"))?,
            ))
        };
        let rules = Arc::new(Rules {
            allowed: m.allowed_peers.clone(),
            denied: m.denied_peers.clone(),
        });
        let creds = Arc::new(Creds::of(&m));
        let key = static_key(&m);
        self.shared.rate.store(m.bandwidth(), Ordering::Relaxed);
        if let Some(cur) = &self.cur {
            if cur.key == key {
                cur.server.set_limits(limits(&cur.server, &m));
                write(&self.shared.creds, creds);
                write(&self.shared.rules, rules);
                write(&self.shared.certs, certs);
                self.access = access;
                if let Some(cur) = &self.cur {
                    self.refilter(cur);
                }
                if let Some(cur) = &mut self.cur {
                    cur.member = m;
                }
                let d = lock(&self.draining);
                for r in d.iter() {
                    self.refilter(r);
                }
                return Ok(());
            }
        }
        for w in stopgap_warnings(&m) {
            akit_observe::warn!(Webrtc, "webrtc.turn: {w}");
        }
        // A server draining on an address the new one needs ends now.
        let ending: Vec<Running> = {
            let mut d = lock(&self.draining);
            let (end, keep): (Vec<_>, Vec<_>) = d.drain(..).partition(|r| r.shares_addr(&m));
            *d = keep;
            end
        };
        for r in &ending {
            self.close(r);
        }
        let (prev_creds, prev_rules, prev_certs, prev_access) = (
            read(&self.shared.creds),
            read(&self.shared.rules),
            read(&self.shared.certs),
            self.access.clone(),
        );
        write(&self.shared.creds, creds);
        write(&self.shared.rules, rules);
        write(&self.shared.certs, certs);
        self.access = access;
        let old = self.cur.take();
        let restore = |t: &mut Turn| {
            write(&t.shared.creds, prev_creds.clone());
            write(&t.shared.rules, prev_rules.clone());
            write(&t.shared.certs, prev_certs.clone());
            t.access = prev_access.clone();
        };
        match old {
            Some(old) if old.shares_addr(&m) => {
                // The new server needs the old one's addresses: close it first, and start it
                // again if the new one cannot start.
                self.close(&old);
                match self.start(&m, key) {
                    Ok(next) => {
                        self.shared.restarts.fetch_add(1, Ordering::Relaxed);
                        akit_observe::warn!(
                            Webrtc,
                            "webrtc.turn: listens on {}, relaying from {}; the server started \
                             again on the same addresses for a change of its realm, relay \
                             addresses, port range, nonce secret, Multipath TCP or stream bounds, \
                             ending its allocations",
                            next.describe(),
                            relay_from(&m)
                        );
                        self.cur = Some(next);
                    }
                    Err(e) => {
                        restore(self);
                        match self.start(&old.member, old.key.clone()) {
                            Ok(again) => self.cur = Some(again),
                            Err(e2) => akit_observe::error!(
                                Webrtc,
                                "webrtc.turn: the server as it was could not start again \
                                 either: {e2}"
                            ),
                        }
                        self.set_own();
                        return Err(e);
                    }
                }
            }
            old => match self.start(&m, key) {
                Ok(next) => {
                    match old {
                        None => akit_observe::info!(
                            Webrtc,
                            "webrtc.turn: listens on {}, relaying from {}",
                            next.describe(),
                            relay_from(&m)
                        ),
                        Some(old) => {
                            self.shared.restarts.fetch_add(1, Ordering::Relaxed);
                            akit_observe::info!(
                                Webrtc,
                                "webrtc.turn: listens on {}, relaying from {}",
                                next.describe(),
                                relay_from(&m)
                            );
                            self.refilter(&old);
                            self.drain(old);
                        }
                    }
                    self.cur = Some(next);
                }
                Err(e) => {
                    restore(self);
                    self.cur = old;
                    self.set_own();
                    return Err(e);
                }
            },
        }
        self.set_own();
        Ok(())
    }

    fn stop(&mut self, _grace: Duration) {
        if let Some(cur) = self.cur.take() {
            self.close(&cur);
        }
        let d: Vec<Running> = lock(&self.draining).drain(..).collect();
        for r in &d {
            self.close(r);
        }
    }

    fn files(&self) -> Vec<(String, [u8; 32])> {
        read(&self.shared.certs)
            .map(|t| t.files().iter().map(|(p, s)| (p.clone(), *s)).collect())
            .unwrap_or_default()
    }

    fn metrics(&self) -> Option<MetricsFn> {
        let shared = Arc::downgrade(&self.shared);
        Some(Arc::new(move |out: &mut Vec<u8>| {
            if let Some(s) = shared.upgrade() {
                write_metrics(&s, out);
            }
        }))
    }
}

/// What a member sets that akit does not do, said when its server starts.
fn stopgap_warnings(m: &Member) -> Vec<String> {
    if m.mptcp {
        vec!["mptcp: listen_tcp and listen_tls take plain TCP".to_owned()]
    } else {
        Vec::new()
    }
}

fn write_metrics(s: &Shared, out: &mut Vec<u8>) {
    let (st, servers) = s.stats();
    let n = |v: usize| u64::try_from(v).unwrap_or(u64::MAX);
    let load = |a: &AtomicU64| a.load(Ordering::Relaxed);
    let mut w = TextWriter::new(std::mem::take(out));
    for (name, help, v) in [
        (
            "akit_turn_allocations",
            "Allocations the TURN server holds, those of a server draining and expired ones the \
             next sweep frees included.",
            st.allocations,
        ),
        (
            "akit_turn_pending_allocations",
            "Allocations waiting for their relayed address to be bound.",
            st.pending,
        ),
        (
            "akit_turn_permissions",
            "Permissions the allocations hold.",
            st.permissions,
        ),
        (
            "akit_turn_channels",
            "Channels the allocations have bound.",
            st.channels,
        ),
        (
            "akit_turn_draining_servers",
            "TURN servers a reload replaced that drain their allocations.",
            servers.saturating_sub(1),
        ),
    ] {
        w.family(name, "gauge", help);
        w.sample_u64(name, n(v), &[]);
    }
    w.family(
        "akit_turn_relayed_bytes_total",
        "counter",
        "Bytes the TURN server relayed, by direction: to peers, from clients, or from peers, to \
         clients.",
    );
    w.sample_u64(
        "akit_turn_relayed_bytes_total",
        st.to_peer_bytes,
        &[("direction", "to_peer")],
    );
    w.sample_u64(
        "akit_turn_relayed_bytes_total",
        st.to_client_bytes,
        &[("direction", "from_peer")],
    );
    w.family(
        "akit_turn_dropped_total",
        "counter",
        "Datagrams the TURN server dropped, by reason: a username past its user_bandwidth.",
    );
    w.sample_u64(
        "akit_turn_dropped_total",
        load(&s.dropped),
        &[("reason", "bandwidth")],
    );
    let counters = [
        (
            "akit_turn_unrelayed_total",
            "Datagrams the TURN server did nothing with: malformed, without an allocation, a \
             permission or a channel, refused by the peer rules, or past user_bandwidth.",
            st.dropped,
        ),
        (
            "akit_turn_allocated_total",
            "Allocations made.",
            load(&s.ev.allocated),
        ),
        (
            "akit_turn_refreshed_total",
            "Allocations refreshed.",
            load(&s.ev.refreshed),
        ),
    ];
    for (name, help, v) in counters {
        w.family(name, "counter", help);
        w.sample_u64(name, v, &[]);
    }
    w.family(
        "akit_turn_freed_total",
        "counter",
        "Allocations ended, by reason: a client's delete, its lifetime run out, or its server or \
         TCP or TLS connection closed.",
    );
    for (i, why) in ["deleted", "expired", "closed"].iter().enumerate() {
        let v = s.ev.freed.get(i).map_or(0, load);
        w.sample_u64("akit_turn_freed_total", v, &[("reason", why)]);
    }
    w.family(
        "akit_turn_refused_total",
        "counter",
        "Requests refused after their credentials passed, by error code: 403 a peer the rules \
         refuse, 486 max_allocations_per_user, 508 max_allocations, max_permissions, \
         max_channels, the relay port range or a drain, and the rest.",
    );
    for (i, c) in s.ev.refused.iter().enumerate() {
        let code = REFUSED_CODES
            .get(i)
            .map_or_else(|| "other".to_owned(), ToString::to_string);
        w.sample_u64("akit_turn_refused_total", load(c), &[("code", &code)]);
    }
    let forbidden = REFUSED_CODES
        .iter()
        .position(|c| *c == 403)
        .and_then(|i| s.ev.refused.get(i))
        .map_or(0, load);
    for (name, help, v) in [
        (
            "akit_turn_auth_failures_total",
            "Requests whose credentials failed: a wrong user, password or realm, or incomplete \
             ones.",
            load(&s.ev.auth_failed),
        ),
        (
            "akit_turn_peers_refused_total",
            "Permissions and channels refused for a peer the rules refuse: denied_peers, the \
             default denial of internal addresses, or akit's own address on a port that is not a \
             relayed one.",
            forbidden,
        ),
        (
            "akit_turn_restarts_total",
            "Times a config's change of the TURN server's addresses, realm, relay addresses, \
             port range, nonce secret, Multipath TCP or stream bounds started a new server.",
            load(&s.restarts),
        ),
    ] {
        w.family(name, "counter", help);
        w.sample_u64(name, v, &[]);
    }
    *out = w.out;
}

#[cfg(test)]
mod tests;
