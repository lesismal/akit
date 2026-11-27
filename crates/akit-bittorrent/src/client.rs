//! The `bittorrent.client` role (I152, fit's `bittorrent/role.go`, `state.go`, `api.go`):
//! akio-bittorrent's `Client`, downloading and seeding the torrents kept in `data_dir`.
//!
//! The client runs on an engine of its own, which `Client::new` makes, until akio's A283 lets it
//! join the listeners' engine. Its disk reads, writes and hash checks run on its disk pool, never
//! on an I/O thread. The role's own work that touches files runs on the control thread (an
//! apply, which reads the state and makes the client), on the caller's thread (the API: adding,
//! removing, pausing, each change of the list saved at once, as fit's), and on the role's saver
//! thread, which every `save_interval` writes what the client's events left to save: a magnet
//! link's .torrent file once its metadata arrived, a completion, a torrent its seed limit paused.
//! The events themselves only note what happened, under a lock of their own: they may come on an
//! I/O thread, or inside a call of the role's.
//!
//! A reload changes the rates and seed limits in place. A change of `listen`, `encryption`,
//! `dht`, `pex`, `utp`, `lsd`, `disk_workers`, `max_conns_per_torrent` or `upload_slots` (the last
//! two until akio's A284) or of `data_dir` makes a new client: the state is saved and the client
//! closed (akio saves each torrent's resume data), and the new one starts from the state of its
//! `data_dir`, resuming without a check. A new client that cannot be made brings the old one back.

mod pages;
pub(crate) mod state;

use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use akio_bittorrent::client::{
    AddOptions, Client, ClientConfig, FileStorage, RateLimits, SeedLimits, Torrent, TorrentEvent,
    TorrentStatus,
};
use akio_bittorrent::magnet::Magnet;
use akio_bittorrent::metainfo::{InfoHash, MetaInfo};
use akio_bittorrent::mse::Policy;
use akit_config::Config;
use akit_config::bittorrent::{BitTorrentClient, BtEncryption};
use akit_roles::host::{Host, Info, Role};

use state::{Doc, Entry};

/// The role's ID, and its member's path in the config.
pub const ID: &str = "bittorrent.client";

/// The `bittorrent.client` role, for the binary's list of roles.
pub const CLIENT: Info = Info {
    id: ID,
    set: is_set,
    new: new_role,
};

fn is_set(c: &Config) -> bool {
    c.bittorrent.as_ref().is_some_and(|b| b.client.is_some())
}

#[allow(clippy::unnecessary_wraps)]
fn new_role(h: Host) -> Result<Box<dyn Role>, String> {
    Ok(Box::new(ClientRole::new(h)))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The peer ID's prefix: akit's (decision 037; decision row 75), in the Azureus style clients
/// show, then 12 random bytes.
pub const PEER_ID_PREFIX: &[u8; 8] = b"-AK0001-";

/// The public routers the DHT bootstraps from, as fit's client does: resolved off the control
/// thread, only when `dht` is on.
const DHT_ROUTERS: [&str; 3] = [
    "router.bittorrent.com:6881",
    "dht.transmissionbt.com:6881",
    "router.utorrent.com:6881",
];

/// Why an API call was refused, fit's errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// The client does not run (fit's `ErrNotRunning`).
    NotRunning,
    /// No such torrent (`ErrNotFound`).
    NotFound,
    /// The torrent is held already, under this ID.
    Exists(String),
    /// `max_torrents` torrents are held already (`ErrTooMany`).
    TooMany,
    /// Bad input (`ErrBadRequest`): why.
    BadRequest(String),
    /// The state could not be written: why.
    Io(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::NotRunning => f.write_str("bittorrent: the client does not run"),
            ApiError::NotFound => f.write_str("bittorrent: no such torrent"),
            ApiError::Exists(id) => write!(f, "bittorrent: torrent {id} is held already"),
            ApiError::TooMany => f.write_str("bittorrent: max_torrents torrents are held already"),
            ApiError::BadRequest(why) => write!(f, "bittorrent: bad request: {why}"),
            ApiError::Io(why) => write!(f, "bittorrent: {why}"),
        }
    }
}

impl std::error::Error for ApiError {}

/// What to add: a .torrent file's bytes or a magnet link.
#[derive(Debug, Clone)]
pub enum Source {
    /// A .torrent file.
    Torrent(Vec<u8>),
    /// A magnet link.
    Magnet(String),
}

/// How to add it.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct AddRequest {
    /// Added paused.
    pub paused: bool,
    /// Peers to dial, kept with it (at most 128).
    pub peers: Vec<SocketAddr>,
}

/// A torrent as the API reports it, fit's `bittorrent.Status` but for what later iterations add
/// (categories, limits, the queue, files).
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    /// Its ID: its v1 info hash in hex, or its v2 hash cut to 20 bytes.
    pub id: String,
    /// Its name, once known.
    pub name: String,
    /// Its magnet link.
    pub magnet: String,
    /// The directory it downloads into.
    pub dir: String,
    /// `metadata`, `checking`, `downloading`, `seeding`, `paused`, `stopped` or `error`.
    pub state: String,
    /// What went wrong, if anything.
    pub error: String,
    /// Paused by the API or its seed limit.
    pub paused: bool,
    /// When it was added, RFC 3339.
    pub added: String,
    /// It has every piece.
    pub complete: bool,
    /// Its size in bytes, once known.
    pub size: u64,
    /// Bytes had.
    pub completed: u64,
    /// `completed` over `size`, 0 to 1.
    pub progress: f64,
    /// Pieces in all and had.
    pub pieces: usize,
    /// Pieces had.
    pub pieces_have: usize,
    /// Payload bytes downloaded and uploaded, earlier sessions in its resume data included.
    pub downloaded: u64,
    /// Payload bytes uploaded.
    pub uploaded: u64,
    /// Connected peers, and those of them that have every piece.
    pub peers: usize,
    /// Connected seeders.
    pub seeders: usize,
    /// Pieces read back and checked when it started: 0 when it resumed without a check.
    pub pieces_checked: u64,
}

/// A torrent the role holds.
struct Rec {
    entry: Entry,
    /// Its .torrent, once known.
    mi: Option<MetaInfo>,
    /// Whether `torrents/<id>.torrent` holds it.
    mi_saved: bool,
    torrent: Option<Torrent>,
    err: String,
}

/// What makes a new client when it changes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ClientKey {
    listen: String,
    encryption: BtEncryption,
    dht: bool,
    pex: bool,
    utp: bool,
    lsd: bool,
    disk_threads: usize,
    data_dir: String,
    conns_per_torrent: usize,
    slots: usize,
}

impl ClientKey {
    fn of(c: &BitTorrentClient) -> ClientKey {
        let l = c.limits_or_default();
        ClientKey {
            listen: c.listen.clone(),
            encryption: c.encryption,
            dht: c.dht_on(),
            pex: c.pex_on(),
            utp: c.utp_on(),
            lsd: c.lsd,
            disk_threads: c.disk_threads(),
            data_dir: c.data_dir.clone(),
            conns_per_torrent: l.conns_per_torrent(),
            slots: l.slots(),
        }
    }
}

/// The running client and its torrents.
struct Run {
    key: ClientKey,
    cfg: BitTorrentClient,
    client: Client,
    data_dir: PathBuf,
    downloads: PathBuf,
    /// Where the client puts a magnet link's files: `download_dir` when it was made.
    client_dir: PathBuf,
    recs: Vec<Rec>,
    other: Vec<(String, akit_config::value::Value)>,
}

/// What a client's event noted for the saver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Note {
    Metadata,
    Completed,
    SeedLimit,
}

/// The most events noted between two saves; past it, the saver looks at every torrent.
const MAX_NOTES: usize = 4096;

/// What the role, its API and its saver share.
struct Shared {
    run: Mutex<Option<Run>>,
    notes: Mutex<Vec<(InfoHash, Note)>>,
    /// Notes were dropped past [`MAX_NOTES`]: every torrent is looked at.
    overflow: AtomicBool,
}

impl fmt::Debug for Shared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Shared").finish_non_exhaustive()
    }
}

/// The role's API: what the console's routes, the tests and (I157) the Admin API call. Cheap to
/// clone. Its calls do file I/O (the state saved): call them off I/O threads.
#[derive(Debug, Clone)]
pub struct ClientApi(Arc<Shared>);

enum SaverMsg {
    Interval(Duration),
    Stop,
}

/// The running `bittorrent.client` role.
pub struct ClientRole {
    host: Host,
    shared: Arc<Shared>,
    saver: Option<(Sender<SaverMsg>, std::thread::JoinHandle<()>)>,
    pages: Arc<pages::Pages>,
    stopped: bool,
}

impl fmt::Debug for ClientRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientRole")
            .field("id", &self.host.id)
            .finish_non_exhaustive()
    }
}

fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    akit_roles::linktoken::rfc3339(secs)
}

/// A local address to listen on: `:port` every address, IPv6 and IPv4; `localhost` the loopback
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

fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

fn rates(c: &BitTorrentClient) -> RateLimits {
    let l = c.limits_or_default();
    let r = |v: u64| (v > 0).then_some(v);
    RateLimits::new(r(l.upload_rate), r(l.download_rate))
}

fn seed_limits(c: &BitTorrentClient) -> SeedLimits {
    let l = c.limits_or_default();
    SeedLimits::new(
        (l.seed_ratio > 0.0).then_some(l.seed_ratio),
        (!l.seed_time.is_zero()).then_some(l.seed_time),
    )
}

fn peer_id() -> [u8; 20] {
    let mut id = [0u8; 20];
    if let Some(d) = id.get_mut(..8) {
        d.copy_from_slice(PEER_ID_PREFIX);
    }
    if let Some(d) = id.get_mut(8..) {
        let _ = akio_crypto::rand::fill_random(d);
    }
    id
}

/// The client `cfg` asks for, its events noted into `shared`.
fn make_client(cfg: &BitTorrentClient, shared: &Arc<Shared>) -> Result<Client, String> {
    let listen = listen_addr(&cfg.listen)
        .ok_or_else(|| format!("listen {}: not an address to bind", cfg.listen))?;
    let data_dir = absolute(Path::new(&cfg.data_dir));
    let downloads = absolute(&cfg.downloads());
    for d in [&data_dir, &downloads] {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let l = cfg.limits_or_default();
    let mut c = ClientConfig::default();
    c.listen = listen;
    c.data_dir = downloads;
    c.peer_id = Some(peer_id());
    c.encryption = match cfg.encryption {
        BtEncryption::Allow => Policy::Disabled,
        BtEncryption::Prefer => Policy::Prefer,
        BtEncryption::Require => Policy::Require,
    };
    c.utp = cfg.utp_on();
    c.dht = cfg.dht_on();
    c.pex = cfg.pex_on();
    c.lsd = cfg.lsd;
    c.upload_slots = l.slots();
    c.max_peers_per_torrent = l.conns_per_torrent();
    c.disk_threads = cfg.disk_threads();
    c.rate_limits = rates(cfg);
    c.seed_limits = seed_limits(cfg);
    c.resume_dir = Some(data_dir.join(state::RESUME_DIR));
    c.resume_interval = cfg.save();
    let weak = Arc::downgrade(shared);
    c.on_event = Some(Arc::new(move |ih: InfoHash, ev: &TorrentEvent| {
        let note = match ev {
            TorrentEvent::MetadataReceived => Note::Metadata,
            TorrentEvent::Completed => Note::Completed,
            TorrentEvent::SeedLimitReached => Note::SeedLimit,
            _ => return,
        };
        if let Some(s) = weak.upgrade() {
            let mut n = lock(&s.notes);
            if n.len() < MAX_NOTES {
                n.push((ih, note));
            } else {
                s.overflow.store(true, Ordering::Relaxed);
            }
        }
    }));
    let client = Client::new(c).map_err(|e| format!("listen on {}: {e}", cfg.listen))?;
    if cfg.dht_on() {
        // A router's name is resolved on a thread of its own, never the control thread's; the
        // client takes the addresses whenever they come.
        let cl = client.clone();
        let _ = std::thread::Builder::new()
            .name("akit-bt-dht".into())
            .spawn(move || {
                use std::net::ToSocketAddrs;
                let addrs: Vec<SocketAddr> = DHT_ROUTERS
                    .iter()
                    .filter_map(|r| r.to_socket_addrs().ok())
                    .flatten()
                    .collect();
                cl.dht_bootstrap(&addrs);
            });
    }
    Ok(client)
}

/// Starts `rec` in `client`: from its .torrent into its own directory, or from its magnet link
/// into the client's (akio's A286).
fn start(client: &Client, rec: &mut Rec, seed: SeedLimits) {
    let mut o = AddOptions::default();
    o.paused = rec.entry.paused;
    o.peers = rec
        .entry
        .peers
        .iter()
        .filter_map(|p| p.parse().ok())
        .collect();
    o.seed_limits = Some(seed);
    let added = match &rec.mi {
        Some(mi) => {
            o.storage = Some(Arc::new(FileStorage::new(&rec.entry.dir, &mi.info)));
            client.add_torrent(mi.clone(), o)
        }
        None if !rec.entry.magnet.is_empty() => client.add_magnet(&rec.entry.magnet, o),
        None => {
            rec.err = "neither its .torrent file nor a magnet link is kept".to_owned();
            return;
        }
    };
    match added {
        Ok(t) => {
            rec.torrent = Some(t);
            rec.err.clear();
        }
        Err(e) => rec.err = e.to_string(),
    }
}

/// The records of `doc`, their .torrent files read from `data_dir`.
fn records(doc: &Doc, data_dir: &Path) -> Vec<Rec> {
    doc.torrents
        .iter()
        .map(|e| {
            let path = state::torrent_path(data_dir, &e.id);
            let (mi, err) = match state::read_bounded(&path, state::MAX_TORRENT_BYTES) {
                Ok(b) => match MetaInfo::parse(&b) {
                    Ok(mi) => (Some(mi), String::new()),
                    Err(err) => (None, format!("{}: {err}", path.display())),
                },
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => (None, String::new()),
                Err(err) => (None, format!("{}: {err}", path.display())),
            };
            Rec {
                mi_saved: mi.is_some(),
                mi,
                entry: e.clone(),
                torrent: None,
                err,
            }
        })
        .collect()
}

impl Run {
    fn seed(&self) -> SeedLimits {
        seed_limits(&self.cfg)
    }

    fn doc(&self) -> Doc {
        Doc {
            torrents: self.recs.iter().map(|r| r.entry.clone()).collect(),
            other: self.other.clone(),
        }
    }

    /// Writes `torrents.json`, and the .torrent files not written yet.
    fn save(&mut self) -> Result<(), String> {
        let dir = self.data_dir.clone();
        for r in &mut self.recs {
            if r.mi.is_none() {
                r.mi = r.torrent.as_ref().and_then(Torrent::metainfo);
            }
            if let (Some(mi), false) = (&r.mi, r.mi_saved) {
                let path = state::torrent_path(&dir, &r.entry.id);
                state::write_atomic(&path, &mi.to_bytes())
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                r.mi_saved = true;
            }
            if r.entry.name.is_empty() {
                if let Some(n) = r.mi.as_ref().map(|m| m.info.name.clone()) {
                    r.entry.name = n;
                }
            }
        }
        let path = dir.join(state::STATE_FILE);
        state::write_atomic(&path, state::encode(&self.doc(), &now_rfc3339()).as_bytes())
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    fn find(&self, id: &str) -> Option<usize> {
        let id = id.to_ascii_lowercase();
        self.recs.iter().position(|r| r.entry.id == id)
    }

    fn status(&self, r: &Rec) -> Status {
        let e = &r.entry;
        let mut s = Status {
            id: e.id.clone(),
            name: e.name.clone(),
            magnet: e.magnet.clone(),
            dir: e.dir.clone(),
            state: "stopped".into(),
            error: r.err.clone(),
            paused: e.paused,
            added: e.added.clone(),
            complete: e.completed,
            size: 0,
            completed: 0,
            progress: 0.0,
            pieces: 0,
            pieces_have: 0,
            downloaded: 0,
            uploaded: 0,
            peers: 0,
            seeders: 0,
            pieces_checked: 0,
        };
        if let Some(mi) = &r.mi {
            if s.magnet.is_empty() {
                s.magnet = mi.magnet().to_string();
            }
            if s.name.is_empty() {
                s.name.clone_from(&mi.info.name);
            }
        }
        if !r.err.is_empty() {
            s.state = "error".into();
            return s;
        }
        let Some(t) = &r.torrent else {
            return s;
        };
        let st = t.stats();
        s.state = match st.status {
            TorrentStatus::FetchingMetadata => "metadata",
            TorrentStatus::Checking => "checking",
            TorrentStatus::Downloading => "downloading",
            TorrentStatus::Seeding => "seeding",
            TorrentStatus::Paused => "paused",
            _ => "error",
        }
        .into();
        if s.name.is_empty() {
            s.name = t.name().unwrap_or_default();
        }
        s.size = st.total_size;
        s.completed = st.total_size.saturating_sub(st.bytes_left);
        #[allow(clippy::cast_precision_loss)]
        if st.total_size > 0 {
            s.progress = s.completed as f64 / st.total_size as f64;
        }
        s.pieces = st.pieces_total;
        s.pieces_have = st.pieces_have;
        s.complete = s.complete || (st.pieces_total > 0 && st.pieces_have == st.pieces_total);
        s.downloaded = st.total_downloaded;
        s.uploaded = st.total_uploaded;
        s.peers = st.peers;
        s.seeders = st.seeders;
        s.pieces_checked = st.pieces_checked;
        s
    }

    /// Applies the events noted since: completions and seed limits into the entries. Whether
    /// the list changed.
    fn take_notes(&mut self, notes: Vec<(InfoHash, Note)>, all: bool) -> bool {
        let mut changed = false;
        for r in &mut self.recs {
            let Some(t) = &r.torrent else { continue };
            let ih = t.info_hash();
            let noted: Vec<Note> = notes
                .iter()
                .filter(|(h, _)| *h == ih)
                .map(|(_, n)| *n)
                .collect();
            let complete = t.is_complete();
            if (all || noted.contains(&Note::Completed)) && complete && !r.entry.completed {
                r.entry.completed = true;
                changed = true;
            }
            if (all || noted.contains(&Note::SeedLimit))
                && t.stats().status == TorrentStatus::Paused
                && !r.entry.paused
            {
                // Saved paused, so that a restart, whose counters start from what resume data
                // keeps, does not seed it past its limit (fit's decision 9).
                r.entry.paused = true;
                changed = true;
            }
            if (all || noted.contains(&Note::Metadata)) && !r.mi_saved {
                changed = true;
            }
        }
        changed
    }
}

impl Shared {
    /// The saver's look: what the events noted, saved.
    fn save_noted(&self) {
        let notes = std::mem::take(&mut *lock(&self.notes));
        let all = self.overflow.swap(false, Ordering::Relaxed);
        if notes.is_empty() && !all {
            return;
        }
        let mut g = lock(&self.run);
        if let Some(run) = g.as_mut() {
            if run.take_notes(notes, all) {
                let _ = run.save();
            }
        }
    }
}

fn saver(shared: &Arc<Shared>, rx: &Receiver<SaverMsg>, mut every: Duration) {
    loop {
        match rx.recv_timeout(every) {
            Ok(SaverMsg::Interval(d)) => every = d,
            Ok(SaverMsg::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => shared.save_noted(),
        }
    }
}

impl ClientRole {
    fn new(host: Host) -> ClientRole {
        let shared = Arc::new(Shared {
            run: Mutex::new(None),
            notes: Mutex::new(Vec::new()),
            overflow: AtomicBool::new(false),
        });
        ClientRole {
            pages: Arc::new(pages::Pages::new(ClientApi(Arc::clone(&shared)))),
            host,
            shared,
            saver: None,
            stopped: false,
        }
    }

    /// A new client of `cfg` started from `data_dir`'s state; the run that holds it.
    fn open(&self, cfg: &BitTorrentClient, doc: Doc) -> Result<Run, String> {
        let client = make_client(cfg, &self.shared)?;
        let data_dir = absolute(Path::new(&cfg.data_dir));
        let mut run = Run {
            key: ClientKey::of(cfg),
            cfg: cfg.clone(),
            client,
            downloads: absolute(&cfg.downloads()),
            client_dir: absolute(&cfg.downloads()),
            recs: records(&doc, &data_dir),
            other: doc.other,
            data_dir,
        };
        let seed = run.seed();
        for r in &mut run.recs {
            if r.err.is_empty() {
                start(&run.client, r, seed);
            }
        }
        Ok(run)
    }

    fn close(run: &mut Run) {
        let _ = run.save();
        run.client.close();
    }
}

impl Role for ClientRole {
    fn apply(&mut self, config: &Config) -> Result<(), String> {
        let Some(cfg) = config.bittorrent.as_ref().and_then(|b| b.client.as_ref()) else {
            return Err("no bittorrent.client member".to_owned());
        };
        if self.stopped {
            return Err("the role was stopped".to_owned());
        }
        let key = ClientKey::of(cfg);
        let mut g = lock(&self.shared.run);
        let same = g.as_ref().is_some_and(|r| r.key == key);
        if same {
            if let Some(run) = g.as_mut() {
                run.client.set_rate_limits(rates(cfg));
                let seed = seed_limits(cfg);
                for t in run.recs.iter().filter_map(|r| r.torrent.as_ref()) {
                    t.set_seed_limits(seed);
                }
                run.downloads = absolute(&cfg.downloads());
                run.cfg = cfg.clone();
            }
        } else {
            // The new directory's state first: one that does not read leaves all as it was.
            let data_dir = absolute(Path::new(&cfg.data_dir));
            let mut old = g.take();
            if let Some(o) = old.as_mut() {
                // Saved first, so that the same directory read again holds what runs now.
                let _ = o.save();
            }
            let doc = match state::load(&data_dir) {
                Ok(d) => d,
                Err(e) => {
                    *g = old;
                    return Err(e);
                }
            };
            if let Some(o) = old.as_mut() {
                ClientRole::close(o);
            }
            match self.open(cfg, doc) {
                Ok(run) => *g = Some(run),
                Err(e) => {
                    // The old client back, on its config and torrents.
                    if let Some(o) = old {
                        let back = state::load(&o.data_dir)
                            .map_err(|e2| {
                                format!("{e}; the client before could not be read back: {e2}")
                            })
                            .and_then(|doc| self.open(&o.cfg, doc));
                        match back {
                            Ok(run) => *g = Some(run),
                            Err(e2) => {
                                return Err(format!(
                                    "{e}; the client before did not start again: {e2}"
                                ));
                            }
                        }
                    }
                    return Err(e);
                }
            }
        }
        drop(g);
        match &self.saver {
            Some((tx, _)) => {
                let _ = tx.send(SaverMsg::Interval(cfg.save()));
            }
            None => {
                let (tx, rx) = channel();
                let shared = Arc::clone(&self.shared);
                let every = cfg.save();
                let h = std::thread::Builder::new()
                    .name("akit-bt-saver".into())
                    .spawn(move || saver(&shared, &rx, every))
                    .map_err(|e| format!("the saver thread: {e}"))?;
                self.saver = Some((tx, h));
            }
        }
        Ok(())
    }

    fn stop(&mut self, _grace: Duration) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        if let Some((tx, h)) = self.saver.take() {
            let _ = tx.send(SaverMsg::Stop);
            let _ = h.join();
        }
        self.shared.save_noted();
        if let Some(mut run) = lock(&self.shared.run).take() {
            ClientRole::close(&mut run);
        }
    }

    fn api(&self) -> Option<akit_roles::host::Api> {
        Some(Arc::new(ClientApi(Arc::clone(&self.shared))) as akit_roles::host::Api)
    }

    fn module(&self) -> Option<akit_roles::pages::ModuleRef> {
        Some(Arc::clone(&self.pages) as akit_roles::pages::ModuleRef)
    }
}

impl Drop for ClientRole {
    fn drop(&mut self) {
        self.stop(Duration::ZERO);
    }
}

fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

/// Removes the files of `mi` below `dir` that are there, and the torrent's own directory if it
/// is left empty.
fn delete_data(dir: &Path, mi: &MetaInfo) {
    let st = FileStorage::new(dir, &mi.info);
    for (i, f) in mi.info.files.iter().enumerate() {
        if !f.padding {
            let _ = std::fs::remove_file(st.path(i));
        }
    }
    if !mi.info.single_file {
        let _ = std::fs::remove_dir(dir.join(&mi.info.name));
    }
}

impl ClientApi {
    /// The API of the running role `api` (`akit_roles::host::Manager::api`), if it is this
    /// role's.
    #[must_use]
    pub fn of(api: &akit_roles::host::Api) -> Option<ClientApi> {
        api.downcast_ref::<ClientApi>().cloned()
    }

    fn with<R>(&self, f: impl FnOnce(&mut Run) -> Result<R, ApiError>) -> Result<R, ApiError> {
        let mut g = lock(&self.0.run);
        match g.as_mut() {
            Some(run) => f(run),
            None => Err(ApiError::NotRunning),
        }
    }

    /// Where the client listens.
    #[must_use]
    pub fn listen_addr(&self) -> Option<SocketAddr> {
        lock(&self.0.run).as_ref().map(|r| r.client.listen_addr())
    }

    /// Adds a torrent into `download_dir`, as fit's `Role.Add`; the list saved at once.
    ///
    /// # Errors
    ///
    /// Not running, bad input, held already, `max_torrents` reached, the state not written.
    pub fn add(&self, src: Source, req: AddRequest) -> Result<Status, ApiError> {
        let (mi, magnet) = match src {
            Source::Torrent(b) => {
                if u64::try_from(b.len()).map_or(true, |n| n > state::MAX_TORRENT_BYTES) {
                    return Err(ApiError::BadRequest(format!(
                        "a .torrent file larger than {} bytes",
                        state::MAX_TORRENT_BYTES
                    )));
                }
                let mi = MetaInfo::parse(&b).map_err(|e| ApiError::BadRequest(e.to_string()))?;
                (Some(mi), String::new())
            }
            Source::Magnet(m) => {
                if m.len() > state::MAX_MAGNET_BYTES {
                    return Err(ApiError::BadRequest(format!(
                        "a magnet link longer than {} bytes",
                        state::MAX_MAGNET_BYTES
                    )));
                }
                Magnet::parse(&m).map_err(|e| ApiError::BadRequest(e.to_string()))?;
                (None, m)
            }
        };
        self.with(|run| {
            if run.recs.len() >= run.cfg.torrents() {
                return Err(ApiError::TooMany);
            }
            // A magnet link downloads where the client was made to (akio's A286).
            let dir = if mi.is_some() {
                &run.downloads
            } else {
                &run.client_dir
            };
            let dir = dir.to_string_lossy().into_owned();
            let mut rec = Rec {
                entry: Entry {
                    id: String::new(),
                    name: mi.as_ref().map(|m| m.info.name.clone()).unwrap_or_default(),
                    magnet,
                    dir,
                    paused: req.paused,
                    added: now_rfc3339(),
                    completed: false,
                    peers: req
                        .peers
                        .iter()
                        .take(128)
                        .map(ToString::to_string)
                        .collect(),
                    other: Vec::new(),
                },
                mi,
                mi_saved: false,
                torrent: None,
                err: String::new(),
            };
            let seed = run.seed();
            start(&run.client, &mut rec, seed);
            let Some(t) = rec.torrent.clone() else {
                return Err(match &rec.err {
                    e if e.contains("already added") => ApiError::Exists(String::new()),
                    e => ApiError::BadRequest(e.clone()),
                });
            };
            rec.entry.id = hex(&t.info_hash().0);
            let id = rec.entry.id.clone();
            run.recs.push(rec);
            if let Err(e) = run.save() {
                // Not kept: the client lets it go again.
                if let Some(r) = run.recs.pop() {
                    if let Some(t) = &r.torrent {
                        run.client.remove(&t.info_hash());
                    }
                }
                return Err(ApiError::Io(e));
            }
            let i = run.find(&id).ok_or(ApiError::NotFound)?;
            run.recs
                .get(i)
                .map(|r| run.status(r))
                .ok_or(ApiError::NotFound)
        })
        .map_err(|e| match e {
            ApiError::Exists(id) if id.is_empty() => ApiError::Exists("of this info hash".into()),
            e => e,
        })
    }

    /// Removes torrent `id`, its files too with `delete_data`, and its .torrent and resume data.
    ///
    /// # Errors
    ///
    /// Not running, no such torrent, the state not written.
    pub fn remove(&self, id: &str, delete: bool) -> Result<(), ApiError> {
        self.with(|run| {
            let i = run.find(id).ok_or(ApiError::NotFound)?;
            let r = run.recs.remove(i);
            if let Some(t) = &r.torrent {
                run.client.remove(&t.info_hash());
            }
            let _ = std::fs::remove_file(state::torrent_path(&run.data_dir, &r.entry.id));
            let _ = std::fs::remove_file(state::resume_path(&run.data_dir, &r.entry.id));
            if delete {
                if let Some(mi) =
                    r.mi.as_ref()
                        .or(r.torrent.as_ref().and_then(Torrent::metainfo).as_ref())
                {
                    delete_data(Path::new(&r.entry.dir), mi);
                }
            }
            run.save().map_err(ApiError::Io)
        })
    }

    /// Pauses torrent `id`, or resumes it; saved at once.
    ///
    /// # Errors
    ///
    /// Not running, no such torrent, the state not written.
    pub fn set_paused(&self, id: &str, paused: bool) -> Result<Status, ApiError> {
        self.with(|run| {
            let i = run.find(id).ok_or(ApiError::NotFound)?;
            let r = run.recs.get_mut(i).ok_or(ApiError::NotFound)?;
            r.entry.paused = paused;
            if let Some(t) = &r.torrent {
                if paused {
                    t.pause();
                } else {
                    t.resume();
                }
            }
            run.save().map_err(ApiError::Io)?;
            run.recs
                .get(i)
                .map(|r| run.status(r))
                .ok_or(ApiError::NotFound)
        })
    }

    /// Every torrent, in the list's order.
    ///
    /// # Errors
    ///
    /// Not running.
    pub fn list(&self) -> Result<Vec<Status>, ApiError> {
        self.with(|run| Ok(run.recs.iter().map(|r| run.status(r)).collect()))
    }

    /// Torrent `id`.
    ///
    /// # Errors
    ///
    /// Not running, no such torrent.
    pub fn get(&self, id: &str) -> Result<Status, ApiError> {
        self.with(|run| {
            let i = run.find(id).ok_or(ApiError::NotFound)?;
            run.recs
                .get(i)
                .map(|r| run.status(r))
                .ok_or(ApiError::NotFound)
        })
    }

    /// Saves what the client's events noted now, as the saver does every `save_interval`.
    pub fn save_now(&self) {
        self.0.overflow.store(true, Ordering::Relaxed);
        self.0.save_noted();
    }
}

#[cfg(test)]
mod tests;
