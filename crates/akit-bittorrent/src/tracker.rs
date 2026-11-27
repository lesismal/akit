//! The `bittorrent.tracker` role (I151, fit's `bittorrent/tracker`): akio-bittorrent's
//! `TrackerServer` over HTTP, through the routes whose handler is `bt_tracker`, and over UDP, on
//! the role's own `listen`, both on one bounded store in memory, on the listeners' engine.
//!
//! Nothing here blocks an I/O thread: an announce or a scrape is served on the thread that read
//! it, by the store, which takes a short lock of its shard; the allow-list and the access are
//! lookups in what the apply compiled. The allow-list's files are read by the apply, on the
//! control thread.
//!
//! What a request reads is the role's [`Snap`], published on every apply to a board keyed by the
//! engine (fit keys its roles by engine too), and read through a cache of each thread's that is
//! refreshed only when the board changed: no lock and no shared reference count per request.
//!
//! A reload changes in place the allow-list, the access, the answers (`interval`,
//! `min_interval`, `max_numwant`, `max_scrape`) and the console, keeping the swarms. A change of
//! the store's bounds (`max_torrents`, `max_peers_per_torrent`, `peer_ttl`) makes a new store,
//! its swarms starting empty (clients announce again within their interval), until akio's A285
//! sets them in place. A `listen` change moves the UDP binding; a port that cannot be bound
//! leaves the one before. The UDP connection IDs are keyed by a secret the role keeps across
//! reloads, not restarts.

mod handler;
pub(crate) use handler::new as new_handler;
pub use handler::service;
pub(crate) mod pages;
mod serve;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use akio_bittorrent::tracker::{TrackerConfig, TrackerServer};
use akit_config::Config;
use akit_config::bittorrent::{BitTorrentTracker, MAX_BT_TRACKER_ALLOW, parse_info_hash};
use akit_roles::host::{Host, Info, Role};

/// The role's ID, and its member's path in the config.
pub const ID: &str = "bittorrent.tracker";

/// The `bittorrent.tracker` role, for the binary's list of roles.
pub const TRACKER: Info = Info {
    id: ID,
    set: is_set,
    new: new_role,
};

fn is_set(c: &Config) -> bool {
    c.bittorrent.as_ref().is_some_and(|b| b.tracker.is_some())
}

#[allow(clippy::unnecessary_wraps)]
fn new_role(h: Host) -> Result<Box<dyn Role>, String> {
    Ok(Box::new(Tracker::new(h)))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The reasons a request is refused, counted in the statistics.
pub(crate) const REFUSED_UNREGISTERED: usize = 0;
// 1, `passkey`, counts a private tracker's refusals, which come with `auth` (I159).
pub(crate) const REFUSED_ACCESS: usize = 2;
const REFUSAL_NAMES: [&str; 3] = ["unregistered", "passkey", "access"];

/// What the role counts, across reloads.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    pub(crate) announces: AtomicU64,
    pub(crate) scrapes: AtomicU64,
    pub(crate) refused: [AtomicU64; 3],
}

impl Counters {
    pub(crate) fn refuse(&self, why: usize) {
        if let Some(c) = self.refused.get(why) {
            c.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The allow-list: the info hashes tracked, in the order first listed.
#[derive(Debug, Default)]
pub(crate) struct Allow {
    set: HashSet<[u8; 20]>,
    pub(crate) list: Vec<[u8; 20]>,
}

impl Allow {
    fn add(&mut self, h: [u8; 20]) -> Result<(), String> {
        if self.set.contains(&h) {
            return Ok(());
        }
        if self.list.len() >= MAX_BT_TRACKER_ALLOW {
            return Err(format!("more than {MAX_BT_TRACKER_ALLOW} info hashes"));
        }
        self.set.insert(h);
        self.list.push(h);
        Ok(())
    }

    pub(crate) fn has(&self, h: &[u8; 20]) -> bool {
        self.set.contains(h)
    }
}

/// What a request is served from: the store and what the apply compiled.
pub(crate) struct Snap {
    pub(crate) server: TrackerServer,
    pub(crate) allow: Option<Arc<Allow>>,
    pub(crate) policy: Option<Arc<akit_access::Policy>>,
    /// Seconds.
    pub(crate) interval: u32,
    /// Seconds; 0, none.
    pub(crate) min_interval: u32,
    pub(crate) numwant: usize,
    pub(crate) scrape: usize,
    pub(crate) counters: Arc<Counters>,
    pub(crate) secret: Arc<[u8; 32]>,
    /// Where UDP is answered, for the statistics.
    pub(crate) udp: Option<SocketAddr>,
    pub(crate) max_torrents: usize,
}

impl fmt::Debug for Snap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Snap")
            .field("interval", &self.interval)
            .field("udp", &self.udp)
            .finish_non_exhaustive()
    }
}

/// The running trackers' snaps, by engine.
type Board = Vec<(usize, Arc<Snap>)>;

// The board of the running trackers' snaps, by engine, and its epoch.
static EPOCH: AtomicU64 = AtomicU64::new(1);
static BOARD: Mutex<Board> = Mutex::new(Vec::new());

thread_local! {
    /// The board as this thread last read it, and the epoch it read it at.
    static SEEN: RefCell<(u64, Board)> = const { RefCell::new((0, Vec::new())) };
}

/// The key of `engine` on the board.
pub(crate) fn key_of(engine: &Arc<akio::Engine>) -> usize {
    Arc::as_ptr(engine).addr()
}

/// Puts `snap` on the board for `key`, or takes it off.
fn publish(key: usize, snap: Option<Arc<Snap>>) {
    let mut b = lock(&BOARD);
    b.retain(|(k, _)| *k != key);
    if let Some(s) = snap {
        b.push((key, s));
    }
    drop(b);
    EPOCH.fetch_add(1, Ordering::SeqCst);
}

/// Runs `f` with the snap of the tracker on the engine `key`, `None` while none runs there.
pub(crate) fn with_snap<R>(key: usize, mut f: impl FnMut(Option<&Snap>) -> R) -> R {
    let now = EPOCH.load(Ordering::SeqCst);
    let cached = SEEN.try_with(|c| {
        let mut c = c.try_borrow_mut().ok()?;
        if c.0 != now {
            c.1 = lock(&BOARD).clone();
            c.0 = now;
        }
        let s = c.1.iter().find(|(k, _)| *k == key).map(|(_, s)| &**s);
        Some(f(s))
    });
    if let Ok(Some(r)) = cached {
        return r;
    }
    // The thread's cache is not at hand (its destructor ran, or a call inside one): the board.
    let s = lock(&BOARD)
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, s)| Arc::clone(s));
    f(s.as_deref())
}

/// The store's bounds, which make a new store when they change (until akio's A285).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StoreKey {
    torrents: usize,
    per_torrent: usize,
    ttl: Duration,
}

impl StoreKey {
    fn of(cfg: &BitTorrentTracker) -> StoreKey {
        StoreKey {
            torrents: cfg.torrents(),
            per_torrent: cfg.peers_per_torrent(),
            ttl: cfg.ttl(),
        }
    }

    fn server(self) -> TrackerServer {
        let mut c = TrackerConfig::default();
        c.max_swarms = self.torrents;
        c.max_peers_per_swarm = self.per_torrent;
        c.peer_ttl = self.ttl;
        // The role bounds each announce's peers itself (max_numwant, 50 by default, a UDP
        // answer's 1,400 bytes) before asking the store: the store's bound is the most the
        // config may set.
        c.max_peers = akit_config::bittorrent::MAX_BT_TRACKER_NUMWANT;
        TrackerServer::new(c)
    }
}

/// The UDP binding.
struct Udp {
    listen: String,
    binding: akio::UdpBinding,
    addr: SocketAddr,
}

/// The running `bittorrent.tracker` role.
pub struct Tracker {
    host: Host,
    key: usize,
    counters: Arc<Counters>,
    secret: Arc<[u8; 32]>,
    store: Option<(StoreKey, TrackerServer)>,
    udp: Option<Udp>,
    files: Vec<(String, [u8; 32])>,
    pages: Arc<pages::Pages>,
    stopped: bool,
}

impl fmt::Debug for Tracker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tracker")
            .field("udp", &self.udp.as_ref().map(|u| u.addr))
            .finish_non_exhaustive()
    }
}

/// A local address to listen on: `:port` every address (IPv6 and IPv4), `localhost` the loopback
/// one; a host name is not resolved here.
fn listen_addr(a: &str) -> Option<SocketAddr> {
    if let Ok(s) = a.parse() {
        return Some(s);
    }
    let (host, port) = a.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let ip = match host.trim_start_matches('[').trim_end_matches(']') {
        "" => std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
        h if h.eq_ignore_ascii_case("localhost") => {
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        }
        h => h.parse().ok()?,
    };
    Some(SocketAddr::new(ip, port))
}

/// The policy of the role's own `access`, then the config's global one; the files they read,
/// with their SHA-256, into `files`.
fn policy(
    own: Option<&akit_config::Access>,
    global: Option<&akit_config::Access>,
    files: &mut BTreeMap<String, akit_access::Sum>,
) -> Result<Option<Arc<akit_access::Policy>>, String> {
    let err = |es: Vec<akit_access::Error>| {
        es.iter()
            .map(|e| format!("access.{}: {}", e.field, e.msg))
            .collect::<Vec<_>>()
            .join("; ")
    };
    let own = akit_access::compile(own, Some(files)).map_err(err)?;
    let global = akit_access::compile(global, None).map_err(err)?;
    Ok(akit_access::Policy::chain([own.as_ref(), global.as_ref()]))
}

/// Reads the allow-list, inline and from its files, as fit's `loadAllow`; `None` tracks every
/// torrent. The files read go into `files` with their SHA-256. Reads files: the control thread's.
fn load_allow(
    cfg: &BitTorrentTracker,
    files: &mut BTreeMap<String, akit_access::Sum>,
) -> Result<Option<Arc<Allow>>, String> {
    if cfg.allow.is_empty() && cfg.allow_files.is_empty() {
        return Ok(None);
    }
    let mut a = Allow::default();
    for (i, s) in cfg.allow.iter().enumerate() {
        let h =
            parse_info_hash(s).ok_or_else(|| format!("allow[{i}]: {s:?} is not an info hash"))?;
        a.add(h)?;
    }
    for path in &cfg.allow_files {
        read_allow_file(&mut a, path, files)?;
    }
    Ok(Some(Arc::new(a)))
}

fn read_allow_file(
    a: &mut Allow,
    path: &str,
    files: &mut BTreeMap<String, akit_access::Sum>,
) -> Result<(), String> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let max = akit_access::MAX_FILE_BYTES;
    if f.metadata().is_ok_and(|m| m.len() > max) {
        return Err(format!("{path} is larger than {max} bytes"));
    }
    let mut buf = Vec::new();
    f.take(max.saturating_add(1))
        .read_to_end(&mut buf)
        .map_err(|e| format!("{path}: {e}"))?;
    if u64::try_from(buf.len()).map_or(true, |n| n > max) {
        return Err(format!("{path} is larger than {max} bytes"));
    }
    files.insert(path.to_owned(), akio_crypto::hash::Sha256::digest(&buf));
    let text = String::from_utf8_lossy(&buf);
    for (i, line) in text.lines().enumerate() {
        let n = i.saturating_add(1);
        if line.len() > 64 << 10 {
            return Err(format!("{path}:{n}: the line is longer than 64 KiB"));
        }
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let h = parse_info_hash(line).ok_or_else(|| {
            format!("{path}:{n}: {line:?} is not an info hash: 40 hex digits (v1) or 64 (v2)")
        })?;
        a.add(h).map_err(|e| format!("{path}:{n}: {e}"))?;
    }
    Ok(())
}

fn secs(d: Duration) -> u32 {
    u32::try_from(d.as_secs()).unwrap_or(u32::MAX)
}

impl Tracker {
    fn new(host: Host) -> Tracker {
        let mut secret = [0u8; 32];
        // A secret that could not be made random is still a secret of this process: the
        // engine's address and the time.
        if akio_crypto::rand::fill_random(&mut secret).is_err() {
            let seed = format!(
                "{:?}{:?}",
                std::time::SystemTime::now(),
                Arc::as_ptr(&host.engine)
            );
            secret = akio_crypto::hash::Sha256::digest(seed.as_bytes());
        }
        let key = key_of(&host.engine);
        Tracker {
            pages: Arc::new(pages::Pages::new(key)),
            host,
            key,
            counters: Arc::new(Counters::default()),
            secret: Arc::new(secret),
            store: None,
            udp: None,
            files: Vec::new(),
            stopped: false,
        }
    }

    /// Binds UDP on `addr` with `policy` at akio's filter.
    fn bind(
        &self,
        addr: SocketAddr,
        policy: Option<&Arc<akit_access::Policy>>,
    ) -> Result<akio::UdpBinding, String> {
        let acceptor = serve::UdpAcceptor { key: self.key };
        let bound = match policy {
            Some(p) => {
                let p = Arc::clone(p);
                self.host
                    .engine
                    .bind_udp_filtered(addr, acceptor, move |peer: &SocketAddr| p.allows(peer.ip()))
            }
            None => self.host.engine.bind_udp(addr, acceptor),
        };
        bound.map_err(|e| e.to_string())
    }

    /// Moves the UDP binding to `listen`, `""` for none, its filter `policy`. A new address that
    /// cannot be bound leaves the binding before, as fit's `applyListen`.
    fn move_listen(
        &mut self,
        listen: &str,
        policy: Option<&Arc<akit_access::Policy>>,
    ) -> Result<(), String> {
        if let Some(u) = &self.udp {
            if u.listen == listen {
                let set = match policy {
                    Some(p) => {
                        let p = Arc::clone(p);
                        u.binding
                            .set_filter(move |peer: &SocketAddr| p.allows(peer.ip()))
                    }
                    None => u.binding.clear_filter(),
                };
                return set.map_err(|e| format!("listen on {listen}: {e}"));
            }
        }
        if listen.is_empty() {
            if let Some(u) = self.udp.take() {
                u.binding.close();
            }
            return Ok(());
        }
        let addr = listen_addr(listen)
            .ok_or_else(|| format!("listen on {listen}: not an address to bind"))?;
        // akio binds with SO_REUSEPORT: the old binding's own port is freed first, else both
        // would take its datagrams; it is bound again where it was when the new one fails.
        let same_port = self
            .udp
            .as_ref()
            .is_some_and(|u| u.addr.port() == addr.port());
        if same_port {
            let Some(old) = self.udp.take() else {
                return Ok(());
            };
            old.binding.close();
            match self.bind(addr, policy) {
                Ok(b) => {
                    self.udp = Some(Udp {
                        listen: listen.to_owned(),
                        addr: b.local_addr(),
                        binding: b,
                    });
                    Ok(())
                }
                Err(e) => {
                    if let Ok(b) = self.bind(old.addr, policy) {
                        self.udp = Some(Udp {
                            listen: old.listen,
                            addr: b.local_addr(),
                            binding: b,
                        });
                    }
                    Err(format!("listen on {listen}: {e}"))
                }
            }
        } else {
            let b = self
                .bind(addr, policy)
                .map_err(|e| format!("listen on {listen}: {e}"))?;
            if let Some(old) = self.udp.take() {
                old.binding.close();
            }
            self.udp = Some(Udp {
                listen: listen.to_owned(),
                addr: b.local_addr(),
                binding: b,
            });
            Ok(())
        }
    }
}

impl Role for Tracker {
    fn apply(&mut self, config: &Config) -> Result<(), String> {
        let Some(cfg) = config.bittorrent.as_ref().and_then(|b| b.tracker.as_ref()) else {
            return Err("no bittorrent.tracker member".to_owned());
        };
        if self.stopped {
            return Err("the role was stopped".to_owned());
        }
        let mut files = BTreeMap::new();
        let policy = policy(cfg.access.as_ref(), config.access.as_ref(), &mut files)?;
        let allow = load_allow(cfg, &mut files).map_err(|e| format!("allow-list: {e}"))?;
        let key = StoreKey::of(cfg);
        // A new store, when the role starts or its bounds changed, made before anything changes.
        let fresh = match &self.store {
            Some((k, _)) if *k == key => None,
            _ => Some(key.server()),
        };
        self.move_listen(&cfg.listen, policy.as_ref())?;
        if let Some(s) = fresh {
            self.store = Some((key, s));
        }
        let Some((_, server)) = &self.store else {
            return Err("no store".to_owned());
        };
        let snap = Snap {
            server: server.clone(),
            allow,
            policy,
            interval: secs(cfg.interval_or()),
            min_interval: secs(cfg.min_interval),
            numwant: cfg.numwant(),
            scrape: cfg.scrape(),
            counters: Arc::clone(&self.counters),
            secret: Arc::clone(&self.secret),
            udp: self.udp.as_ref().map(|u| u.addr),
            max_torrents: cfg.torrents(),
        };
        publish(self.key, Some(Arc::new(snap)));
        self.files = files.into_iter().collect();
        Ok(())
    }

    fn stop(&mut self, _grace: Duration) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        if let Some(u) = self.udp.take() {
            u.binding.close();
        }
        self.store = None;
        publish(self.key, None);
    }

    fn files(&self) -> Vec<(String, [u8; 32])> {
        self.files.clone()
    }

    fn module(&self) -> Option<akit_roles::pages::ModuleRef> {
        Some(Arc::clone(&self.pages) as akit_roles::pages::ModuleRef)
    }

    fn api(&self) -> Option<akit_roles::host::Api> {
        Some(Arc::clone(&self.pages) as akit_roles::host::Api)
    }

    fn metrics(&self) -> Option<akit_roles::host::MetricsFn> {
        let key = self.key;
        Some(Arc::new(move |out: &mut Vec<u8>| write_metrics(key, out)))
    }
}

/// The tracker's metrics (I157, fit's `tracker/metrics.go`): the torrents its store keeps and
/// their bound, the announces and scrapes served, the requests refused by reason, and the
/// allow-list's size; its peers and their bound once akio's store counts them (A285). Read from
/// its counters and snapshot, never a lock a request waits for; nothing while it does not run.
fn write_metrics(key: usize, out: &mut Vec<u8>) {
    let Some(st) = stats_of(key) else { return };
    let n = |v: usize| u64::try_from(v).unwrap_or(u64::MAX);
    let mut w = akit_observe::metrics::TextWriter::new(std::mem::take(out));
    for (name, help, v) in [
        (
            "akit_bittorrent_tracker_torrents",
            "Torrents the tracker's store keeps, with the peers it has not looked at since they expired.",
            n(st.torrents),
        ),
        (
            "akit_bittorrent_tracker_max_torrents",
            "The tracker's max_torrents.",
            n(st.max_torrents),
        ),
        (
            "akit_bittorrent_tracker_allowed",
            "Info hashes the tracker's allow-list holds; 0 without one.",
            n(st.allowed),
        ),
    ] {
        w.family(name, "gauge", help);
        w.sample_u64(name, v, &[]);
    }
    for (name, help, v) in [
        (
            "akit_bittorrent_tracker_announces_total",
            "Announces the tracker served since the role started, over HTTP and UDP.",
            st.announces,
        ),
        (
            "akit_bittorrent_tracker_scrapes_total",
            "Scrapes the tracker served since the role started, over HTTP and UDP.",
            st.scrapes,
        ),
    ] {
        w.family(name, "counter", help);
        w.sample_u64(name, v, &[]);
    }
    w.family(
        "akit_bittorrent_tracker_refused_total",
        "counter",
        "Requests the tracker refused since the role started, by reason: unregistered (an info hash the allow-list lacks, counted per hash), passkey and access.",
    );
    for (reason, v) in &st.refused {
        w.sample_u64(
            "akit_bittorrent_tracker_refused_total",
            *v,
            &[("reason", reason)],
        );
    }
    *out = w.out;
}

impl Drop for Tracker {
    fn drop(&mut self) {
        self.stop(Duration::ZERO);
    }
}

/// The tracker's statistics, fit's `tracker.Stats` but for what akio's store cannot count yet
/// (its peers and their bound, akio's A285).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stats {
    /// Torrents tracked now.
    pub torrents: usize,
    /// Announces served.
    pub announces: u64,
    /// Scrapes served.
    pub scrapes: u64,
    /// Refusals by reason: `unregistered`, `passkey`, `access`.
    pub refused: Vec<(&'static str, u64)>,
    /// Whether the tracker takes passkeys (never yet: `auth` is not supported yet).
    pub private: bool,
    /// The info hashes of the allow-list; 0 tracks every torrent.
    pub allowed: usize,
    /// Where UDP is answered, if anywhere.
    pub udp: Option<SocketAddr>,
    /// `max_torrents`.
    pub max_torrents: usize,
}

/// A torrent's statistics, fit's `tracker.TorrentStats`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentStats {
    /// The info hash, in hex.
    pub info_hash: String,
    /// Peers that have every piece.
    pub seeders: u32,
    /// The others.
    pub leechers: u32,
    /// Completed downloads announced while the torrent was tracked.
    pub completed: u32,
    /// Whether the store holds the torrent.
    pub tracked: bool,
}

/// The statistics of the tracker running on `engine`, `None` while none runs there.
#[must_use]
pub fn stats(engine: &Arc<akio::Engine>) -> Option<Stats> {
    stats_of(key_of(engine))
}

pub(crate) fn stats_of(key: usize) -> Option<Stats> {
    with_snap(key, |s| {
        let s = s?;
        let c = &s.counters;
        Some(Stats {
            torrents: s.server.swarms(),
            announces: c.announces.load(Ordering::Relaxed),
            scrapes: c.scrapes.load(Ordering::Relaxed),
            refused: REFUSAL_NAMES
                .iter()
                .zip(&c.refused)
                .map(|(n, v)| (*n, v.load(Ordering::Relaxed)))
                .collect(),
            private: false,
            allowed: s.allow.as_ref().map_or(0, |a| a.list.len()),
            udp: s.udp,
            max_torrents: s.max_torrents,
        })
    })
}

/// The statistics of torrent `h` of the tracker on `key`.
pub(crate) fn torrent_of(key: usize, h: [u8; 20]) -> Option<TorrentStats> {
    with_snap(key, |s| {
        let s = s?;
        Some(serve::torrent_stats(&s.server, h))
    })
}

/// Where the tracker on `engine` answers UDP, if it runs and does.
#[must_use]
pub fn udp_addr(engine: &Arc<akio::Engine>) -> Option<SocketAddr> {
    with_snap(key_of(engine), |s| s.and_then(|s| s.udp))
}

/// The answer of the console's `pages` to a GET of `path` (`path` with its query).
#[cfg(test)]
fn tests_serve(pages: &pages::Pages, path: &str) -> akio_http::Response {
    let bare = path.split('?').next().unwrap_or(path);
    pages.answer(true, path, bare)
}

#[cfg(test)]
mod tests;
