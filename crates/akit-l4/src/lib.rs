#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's layer 4 proxying: TCP and UDP, TLS routed by SNI, PROXY protocol, and the forward
//! proxy's rules (I55, [`forward`]).
//!
//! So far (I51): the config's `services`, as fit's `l4` package proxies them. Each connection a
//! `tcp` listener accepts goes to an address of its service's upstream, and what either side
//! sends is passed to the other as it comes, untouched. A `udp` listener's sessions go the same
//! way: akio makes each address that sends the listener a datagram a connection of its own
//! ([`akio::Listener::udp`]), whose session dials a UDP socket of its own to an upstream address
//! ([`akio::Engine::dial_udp`]), and each datagram either side sends is sent on as one datagram.
//!
//! Nothing waits, as rules 1 and 8 ask. The upstream's connection is dialed through akio's
//! engine, asynchronously (a host name resolved off the I/O thread by [`akio::dns`]), while the
//! client's reads are held; each side's data is sent on from the callback that received it; and a
//! side's reads are held while the other has more than [`WATERMARK`] of it unsent, until that has
//! drained ([`akio::Conn::on_drain`]). Both connections are marked relayed
//! ([`akio::Conn::set_relayed`]): the relay paces each by the other. UDP has no backpressure: a
//! datagram that cannot be sent is dropped, as the network would drop it, and so are those past
//! [`WATERMARK`] that arrive before the upstream's socket is open.
//!
//! The address is picked by the upstream's `load_balancing` (`ip_hash` by the client's address),
//! out of rotation for its `max_fails` and health checks as for HTTP; a connect that fails is
//! tried on the next address picked, up to [`MAX_ATTEMPTS`] addresses, and counts against the
//! address. A UDP session the upstream refuses (an ICMP port unreachable) counts against it too,
//! and ends: the client's next datagram starts another.
//!
//! A side that ends ends the other once what it sent has gone. A connection or session that
//! carries nothing either way for the service's idle timeout is closed (10 minutes for TCP and 1
//! for UDP by default): akio closes it, on its own I/O thread, each side of the relay having the
//! service's idle timeout with the data it sends counted too ([`akio::Conn::set_idle_timeout`],
//! [`akio::Conn::set_idle_counts_writes`]). So is one still open when the graph it started on has
//! been replaced for the drain timeout ([`Service::drain`]). At most `max_conns` are proxied at once by a
//! service; one past it is closed as soon as it is accepted (a UDP session's datagrams dropped).
//!
//! I52: services may share a `tcp` listener, each taking the TLS connections whose ClientHello
//! asks for one of its `server_names` ([`Entry`]), which it proxies as they are, without
//! terminating their TLS; the one without `server_names` takes the rest. The ClientHello is read
//! with akio-mux's parser ([`akio_mux::tls::parse_client_hello`]), up to [`MAX_CLIENT_HELLO`]
//! bytes and within [`OPENING_TIMEOUT`], before a service takes the connection, and is sent on
//! to the upstream with what follows. A listener with `proxy_protocol` reads the PROXY header of
//! its trusted proxies' connections first (akio-mux's `ProxyProtocol`, put in front by
//! akit-listener): the client it names is the connection's, which `ip_hash` picks by, and a
//! service with `proxy_protocol` sends its upstream a header of its own naming that client and
//! the address it connected to ([`akio_mux::proxy::ProxyHeader`]).
//!
//! I51b: a listener with `sockmap` (akit's own field, decision 010) has its TCP connections
//! spliced in the kernel by akio's eBPF sockmap relay as their upstream's connection opens, where
//! the relay loads (Linux 6.15+, with the capabilities it needs), and relayed as above where it
//! does not; a spliced connection's idle timeout and closes follow its sockets' TCP counters
//! ([`sockmap`]). Its bytes never reach akio, so its idle timeout is the service's to keep: the
//! service sweeps its spliced connections on akio's timer thread ([`akio::timer::after_send`])
//! while it has any, reading their counters ([`akio::ebpf::Tunnel::moved`]).
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use akit_upstream::Pool;

mod entry;
pub mod forward;
#[cfg(any(test, feature = "fuzzing"))]
#[doc(hidden)]
pub mod fuzzing;
mod relay;
pub mod sockmap;

pub use akio_mux::proxy::Version as ProxyVersion;
pub use entry::Entry;
pub use relay::{Client, ClientAcceptor};

/// The most bytes read for a ClientHello on a listener whose services pick by server name (fit's
/// `MaxClientHello`): one with large key shares spans several records, but none in use comes
/// near it. A hello still incomplete past it goes to the fallback.
pub const MAX_CLIENT_HELLO: usize = 64 << 10;

/// How long a connection may take to send what is read before a service takes it: its
/// ClientHello, and its PROXY header (fit's `OpeningTimeout`).
pub const OPENING_TIMEOUT: Duration = Duration::from_secs(10);

/// How much may wait to go to one side before the other side's reads are held; also how much a
/// UDP session keeps of what its client sends before its upstream socket is open. fit's.
pub const WATERMARK: usize = 64 << 10;

/// The most addresses one connection's dial is tried on.
pub const MAX_ATTEMPTS: usize = 3;

/// The shards of a service's open connections, so that connections opening and closing on
/// different I/O threads rarely meet on one lock.
const SHARDS: usize = 16;

/// The shortest and the longest wait between two sweeps of a service's spliced connections: a
/// quarter of its idle timeout, within these.
const MIN_SWEEP: Duration = Duration::from_millis(10);
const MAX_SWEEP: Duration = Duration::from_secs(1);

/// A layer 4 service, compiled from the config's: it proxies the connections and sessions its
/// listeners hand it ([`ClientAcceptor`]) to its upstream's pool. Cheap to clone: clones share
/// it.
#[derive(Clone)]
pub struct Service(Arc<Inner>);

pub(crate) struct Inner {
    name: String,
    pub(crate) pool: Pool,
    idle: Duration,
    udp_idle: Duration,
    max_conns: usize,
    /// The PROXY protocol version of the header sent first on each upstream connection.
    pub(crate) proxy: Option<ProxyVersion>,
    /// The addresses one connection is tried on: [`MAX_ATTEMPTS`], or fewer when the upstream
    /// has fewer.
    pub(crate) attempts: usize,
    /// Connections and sessions proxied now.
    conns: AtomicUsize,
    /// The open ones, by their client connection's ID, for the drain and the sweep.
    open: [Mutex<HashMap<u64, Arc<relay::Relay>>>; SHARDS],
    /// What [`Inner::now`] counts from.
    epoch: Instant,
    /// A sweep of the spliced connections is scheduled.
    sweeping: AtomicBool,
    /// [`Service::drain`] was called: connections that come after are closed at once.
    drained: AtomicBool,
    /// Connections spliced in the kernel so far (I51b).
    pub(crate) spliced: AtomicU64,
}

impl fmt::Debug for Service {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Service")
            .field("name", &self.0.name)
            .field("upstream", &self.0.pool.name())
            .field("conns", &self.conns())
            .finish_non_exhaustive()
    }
}

impl Service {
    /// The service `c`, whose upstream is `pool`, which it dials through.
    #[must_use]
    pub fn new(c: &akit_config::Service, pool: Pool) -> Service {
        let attempts = MAX_ATTEMPTS.min(pool.addresses().len()).max(1);
        Service(Arc::new(Inner {
            name: c.name.clone(),
            pool,
            idle: c.idle(),
            udp_idle: c.udp_idle(),
            max_conns: c.max_conns,
            proxy: c.proxy_protocol.map(|v| match v {
                akit_config::ProxyVersion::V1 => ProxyVersion::V1,
                akit_config::ProxyVersion::V2 => ProxyVersion::V2,
            }),
            attempts,
            conns: AtomicUsize::new(0),
            open: std::array::from_fn(|_| Mutex::new(HashMap::new())),
            epoch: Instant::now(),
            sweeping: AtomicBool::new(false),
            drained: AtomicBool::new(false),
            spliced: AtomicU64::new(0),
        }))
    }

    /// Its name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.0.name
    }

    /// Its upstream's pool.
    #[must_use]
    pub fn pool(&self) -> &Pool {
        &self.0.pool
    }

    /// The idle timeout of its TCP connections.
    #[must_use]
    pub fn idle_timeout(&self) -> Duration {
        self.0.idle
    }

    /// The idle timeout of its UDP sessions.
    #[must_use]
    pub fn udp_idle_timeout(&self) -> Duration {
        self.0.udp_idle
    }

    /// The PROXY protocol version of the header it sends its upstream first, if any.
    #[must_use]
    pub fn proxy_protocol(&self) -> Option<ProxyVersion> {
        self.0.proxy
    }

    /// Whether `other` is this service (a clone of it).
    #[must_use]
    pub fn same(&self, other: &Service) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// How many connections and UDP sessions it proxies now.
    #[must_use]
    pub fn conns(&self) -> usize {
        self.0.conns.load(Ordering::SeqCst)
    }

    /// How many of its connections have been spliced in the kernel (`sockmap`, I51b), those
    /// closed since too.
    #[must_use]
    pub fn spliced(&self) -> u64 {
        self.0.spliced.load(Ordering::Relaxed)
    }

    /// How many of the connections it proxies now are spliced in the kernel.
    #[must_use]
    pub fn spliced_now(&self) -> usize {
        self.0.all().iter().filter(|r| r.is_spliced()).count()
    }

    /// Closes the connections and sessions it still proxies, both sides at once, and any that
    /// comes to it later: for once the graph it is part of has been replaced for the drain
    /// timeout, as fit's `Service.Drain`. Never blocks.
    pub fn drain(&self) {
        self.0.drained.store(true, Ordering::SeqCst);
        for r in self.0.all() {
            r.abandon();
        }
    }
}

impl Inner {
    /// Milliseconds since the service was made: what each connection's last activity is told in.
    pub(crate) fn now(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn shard(&self, id: u64) -> MutexGuard<'_, HashMap<u64, Arc<relay::Relay>>> {
        let i = usize::try_from(id.checked_rem(SHARDS as u64).unwrap_or(0)).unwrap_or(0);
        self.open
            .get(i)
            .unwrap_or(&self.open[0])
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes a place under `max_conns`; false when there is none, or the service was drained.
    pub(crate) fn admit(&self) -> bool {
        if self.drained.load(Ordering::SeqCst) {
            return false;
        }
        let mut cur = self.conns.load(Ordering::SeqCst);
        loop {
            if cur >= self.max_conns {
                return false;
            }
            match self.conns.compare_exchange_weak(
                cur,
                cur.saturating_add(1),
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return true,
                Err(now) => cur = now,
            }
        }
    }

    /// Gives back a place [`admit`](Self::admit) took.
    pub(crate) fn leave(&self) {
        self.conns.fetch_sub(1, Ordering::SeqCst);
    }

    /// Has akio close `c`, a side of a relay (a UDP session's when `udp`), once neither side
    /// sent anything for the service's idle timeout: the data it sends counts as activity, as
    /// what it receives does. Where akio cannot (DPDK, TUN) the engine's timeout applies.
    pub(crate) fn idle_timeout<H: akio::Handler>(&self, c: &akio::Conn<H>, udp: bool) {
        let idle = if udp { self.udp_idle } else { self.idle };
        let _ = c.set_idle_timeout(Some(idle));
        let _ = c.set_idle_counts_writes(true);
    }

    /// Records `r` among the open connections.
    pub(crate) fn add(&self, r: &Arc<relay::Relay>) {
        self.shard(r.id).insert(r.id, Arc::clone(r));
        // A drain that ran between `admit` and here did not see it.
        if self.drained.load(Ordering::SeqCst) {
            r.abandon();
        }
    }

    pub(crate) fn remove(&self, id: u64) {
        self.shard(id).remove(&id);
    }

    /// The open connections, taken out of the locks.
    fn all(&self) -> Vec<Arc<relay::Relay>> {
        let mut v = Vec::new();
        for s in &self.open {
            v.extend(
                s.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .values()
                    .cloned(),
            );
        }
        v
    }

    /// How long a sweep waits for the next.
    fn sweep_every(&self) -> Duration {
        self.idle
            .checked_div(4)
            .unwrap_or(MIN_SWEEP)
            .clamp(MIN_SWEEP, MAX_SWEEP)
    }

    /// Schedules a sweep of the spliced connections, unless one is: for a connection just
    /// spliced, whose idle timeout the sweep keeps from then on.
    pub(crate) fn arm_sweep(self: &Arc<Self>) {
        if self.sweeping.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = Arc::downgrade(self);
        if akio::timer::after_send(self.sweep_every(), move || sweep(&me)).is_err() {
            // The timers are full: the next connection tries again.
            self.sweeping.store(false, Ordering::SeqCst);
        }
    }
}

/// Closes the service's spliced connections idle for its timeout, and schedules the next sweep
/// while any is open. On akio's timer thread: it only reads the spliced connections' counters
/// and closes connections, each of which returns at once; the locks are not held meanwhile.
/// Connections relayed in userspace are akio's to close when idle.
fn sweep(me: &Weak<Inner>) {
    let Some(s) = me.upgrade() else {
        return;
    };
    let now = s.now();
    let limit = u64::try_from(s.idle.as_millis()).unwrap_or(u64::MAX);
    let spliced: Vec<_> = s.all().into_iter().filter(|r| r.is_spliced()).collect();
    for r in &spliced {
        // Its bytes never reach akio: its sockets' counters say whether it is active.
        r.probe();
        if now.saturating_sub(r.last()) >= limit {
            r.abandon();
        }
    }
    // Counted after the flag is down: a connection spliced meanwhile either saw it down and
    // scheduled the sweep itself, or is counted here.
    s.sweeping.store(false, Ordering::SeqCst);
    if s.all().iter().any(|r| r.is_spliced()) {
        s.arm_sweep();
    }
}
