//! fit's BitTorrent roles of the config (I150): `bittorrent.client`, which downloads and seeds
//! torrents kept in its `data_dir`, and `bittorrent.tracker`, which answers announces and scrapes,
//! with fit's fields, defaults, bounds and checks (`config/bittorrent.go`,
//! `config/bittorrent_tracker.go`).
//!
//! The members are read as strictly as the rest of the config. What is left out is kept empty
//! (a string), zero (a number, a duration) or `None`; the methods give fit's defaults for it.
//! What fit reads and akit does not run yet is refused as not supported yet, each field by its
//! iteration (the [plan](https://github.com/lesismal/akit/blob/main/docs/dev-plan.md#bittorrent)):
//! the client's `categories`, `watch_dir`, `url_timeout` and `alt_speed` (I153, I154), its
//! `access` and the limits akio-bittorrent does not set yet (akio's A284), `publish` (I156) and
//! `rss` (I159); the tracker's `auth` (a `media_auth` policy, which akit does not read yet),
//! `max_peers` (akio's A285) and `store` (I159).

use std::time::Duration;

use crate::decode::{Decoder, Obj};
use crate::error::join_field;
use crate::json::Node;
use crate::model::{Access, Console};
use crate::roles::{
    access_of, bool_opt, boolean, bytes, count, duration, list, role_console, text, text_at,
};

/// fit's `DefaultBTMaxConnsPerTorrent`: a torrent's peer connections at once.
pub const DEFAULT_BT_MAX_CONNS_PER_TORRENT: usize = 50;
/// fit's `DefaultBTUploadSlots`: the peers a torrent uploads to at once, beside one optimistic
/// unchoke.
pub const DEFAULT_BT_UPLOAD_SLOTS: usize = 4;
/// fit's `DefaultBTMaxTorrents`: the torrents the client holds.
pub const DEFAULT_BT_MAX_TORRENTS: usize = 1000;
/// fit's `MaxBTTorrents`: the most `max_torrents` may be, and the most torrents a state file
/// may list.
pub const MAX_BT_TORRENTS: usize = 100_000;
/// fit's `MaxBTDiskWorkers`: the most `disk_workers` may be.
pub const MAX_BT_DISK_WORKERS: usize = 64;
/// fit's `DefaultBTSaveInterval`: how often, at most, the client's state is saved.
pub const DEFAULT_BT_SAVE_INTERVAL: Duration = Duration::from_secs(5);
/// fit's `DefaultBTWatchInterval`: how often `watch_dir` is looked at.
pub const DEFAULT_BT_WATCH_INTERVAL: Duration = Duration::from_secs(10);
/// fit's `DefaultBTWatchMaxFiles`: the files of `watch_dir` taken in one look.
pub const DEFAULT_BT_WATCH_MAX_FILES: usize = 100;
/// fit's `MaxBTWatchMaxFiles`: the most `watch_max_files` may be.
pub const MAX_BT_WATCH_MAX_FILES: usize = 10_000;
/// fit's `DefaultBTURLTimeout`: how long the download of a .torrent file by URL may take.
pub const DEFAULT_BT_URL_TIMEOUT: Duration = Duration::from_secs(30);
/// fit's `DefaultBTMinFreeBytes`: the disk space a download leaves free.
pub const DEFAULT_BT_MIN_FREE_BYTES: u64 = 1 << 30;
/// fit's `MaxBTCategories`: the most categories.
pub const MAX_BT_CATEGORIES: usize = 1000;

/// fit's `DefaultBTTrackerInterval`: how often clients are told to announce.
pub const DEFAULT_BT_TRACKER_INTERVAL: Duration = Duration::from_secs(30 * 60);
/// fit's `DefaultBTTrackerPeerTTL`: how long a peer is kept after its last announce.
pub const DEFAULT_BT_TRACKER_PEER_TTL: Duration = Duration::from_secs(60 * 60);
/// fit's `DefaultBTTrackerMaxTorrents`: the torrents tracked at once.
pub const DEFAULT_BT_TRACKER_MAX_TORRENTS: usize = 100_000;
/// fit's `DefaultBTTrackerMaxPeersPerTorrent`: the peers kept of one torrent.
pub const DEFAULT_BT_TRACKER_MAX_PEERS_PER_TORRENT: usize = 10_000;
/// fit's `DefaultBTTrackerMaxNumWant`: the most peers an announce gets.
pub const DEFAULT_BT_TRACKER_MAX_NUMWANT: usize = 200;
/// fit's `DefaultBTTrackerMaxScrape`: the most torrents an HTTP scrape may name.
pub const DEFAULT_BT_TRACKER_MAX_SCRAPE: usize = 64;
/// fit's `MaxBTTrackerTorrents`: the most `max_torrents` may be.
pub const MAX_BT_TRACKER_TORRENTS: usize = 1 << 20;
/// fit's `MaxBTTrackerPeersPerTorrent`: the most `max_peers_per_torrent` may be.
pub const MAX_BT_TRACKER_PEERS_PER_TORRENT: usize = 1 << 16;
/// fit's `MaxBTTrackerNumWant`: the most `max_numwant` may be.
pub const MAX_BT_TRACKER_NUMWANT: usize = 1000;
/// fit's `MaxBTTrackerScrape`: the most `max_scrape` may be.
pub const MAX_BT_TRACKER_SCRAPE: usize = 1000;
/// fit's `MaxBTTrackerAllow`: the most info hashes the allow-list may hold, inline and from its
/// files together.
pub const MAX_BT_TRACKER_ALLOW: usize = 1 << 20;

/// How the client's peer connections use Message Stream Encryption (`encryption`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BtEncryption {
    /// `allow`: dial in plaintext first, accept both.
    Allow,
    /// `prefer`: dial encrypted, and again in plaintext when a peer refuses; accept both.
    #[default]
    Prefer,
    /// `require`: dial and accept only encrypted connections.
    Require,
}

impl BtEncryption {
    /// The policy's name, as the config writes it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            BtEncryption::Allow => "allow",
            BtEncryption::Prefer => "prefer",
            BtEncryption::Require => "require",
        }
    }
}

/// `bittorrent`: fit's BitTorrent roles.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BitTorrent {
    /// `client`: the BitTorrent client; none when left out.
    pub client: Option<BitTorrentClient>,
    /// `tracker`: the BitTorrent tracker; none when left out.
    pub tracker: Option<BitTorrentTracker>,
}

/// `bittorrent.client`: a client that downloads and seeds torrents kept in `data_dir`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BitTorrentClient {
    /// `listen`: the host and port peers reach the client at, TCP and UDP (uTP, the DHT and UDP
    /// trackers); port 0 picks one free for both. Required.
    pub listen: String,
    /// `data_dir`: where the client's state is kept. Required.
    pub data_dir: String,
    /// `download_dir`: where torrents added from now on download into; `<data_dir>/downloads`
    /// when left out ([`BitTorrentClient::downloads`]).
    pub download_dir: String,
    /// `encryption`: `prefer` when left out.
    pub encryption: BtEncryption,
    /// `dht`: on when left out ([`BitTorrentClient::dht_on`]).
    pub dht: Option<bool>,
    /// `pex`: on when left out ([`BitTorrentClient::pex_on`]).
    pub pex: Option<bool>,
    /// `utp`: on when left out ([`BitTorrentClient::utp_on`]).
    pub utp: Option<bool>,
    /// `lsd`: off by default.
    pub lsd: bool,
    /// `disk_workers`: the threads that read, write and hash; 0, the number of CPUs at most 8
    /// ([`BitTorrentClient::disk_threads`]).
    pub disk_workers: usize,
    /// `save_interval`: how often, at most, the state is saved; [`DEFAULT_BT_SAVE_INTERVAL`] when
    /// left out or 0.
    pub save_interval: Duration,
    /// `limits`: the client's rates and limits; the defaults when left out.
    pub limits: Option<BitTorrentLimits>,
    /// `console`: the client's pages; none when left out.
    pub console: Option<Console>,
    /// `categories` (I153): the categories a torrent may be added in.
    pub categories: Vec<BitTorrentCategory>,
    /// `watch_dir` (I153): a directory looked at for .torrent and .magnet files; none when left
    /// out.
    pub watch_dir: String,
    /// `watch_interval`: [`DEFAULT_BT_WATCH_INTERVAL`] when left out or 0.
    pub watch_interval: Duration,
    /// `watch_max_files`: [`DEFAULT_BT_WATCH_MAX_FILES`] when left out or 0.
    pub watch_max_files: usize,
    /// `url_timeout`: [`DEFAULT_BT_URL_TIMEOUT`] when left out or 0.
    pub url_timeout: Duration,
}

/// `bittorrent.client.categories[]` (I153): a category of torrents.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BitTorrentCategory {
    /// `name`: letters, digits, `_`, `.` and `-`. Required.
    pub name: String,
    /// `dir`: where its torrents download into, a relative one below `download_dir`;
    /// `<download_dir>/<name>` when left out.
    pub dir: String,
}

impl BitTorrentClient {
    /// The category named `name`.
    #[must_use]
    pub fn category(&self, name: &str) -> Option<&BitTorrentCategory> {
        self.categories.iter().find(|c| c.name == name)
    }

    /// The directory the torrents of `c` download into, fit's `CategoryDir`.
    #[must_use]
    pub fn category_dir(&self, c: &BitTorrentCategory) -> std::path::PathBuf {
        let d = c.dir.trim();
        if d.is_empty() {
            self.downloads().join(&c.name)
        } else if std::path::Path::new(d).is_absolute() {
            std::path::PathBuf::from(d)
        } else {
            self.downloads().join(d)
        }
    }

    /// `watch_interval`, or its default.
    #[must_use]
    pub fn watch(&self) -> Duration {
        dur_or(self.watch_interval, DEFAULT_BT_WATCH_INTERVAL)
    }

    /// `watch_max_files`, or its default.
    #[must_use]
    pub fn watch_files(&self) -> usize {
        or(self.watch_max_files, DEFAULT_BT_WATCH_MAX_FILES)
    }

    /// `url_timeout`, or its default.
    #[must_use]
    pub fn url_fetch_timeout(&self) -> Duration {
        dur_or(self.url_timeout, DEFAULT_BT_URL_TIMEOUT)
    }

    /// `download_dir`, or `<data_dir>/downloads`.
    #[must_use]
    pub fn downloads(&self) -> std::path::PathBuf {
        if self.download_dir.trim().is_empty() {
            std::path::Path::new(&self.data_dir).join("downloads")
        } else {
            std::path::PathBuf::from(&self.download_dir)
        }
    }

    /// Whether the DHT is on, as it is when left out.
    #[must_use]
    pub fn dht_on(&self) -> bool {
        self.dht.unwrap_or(true)
    }

    /// Whether peer exchange is on, as it is when left out.
    #[must_use]
    pub fn pex_on(&self) -> bool {
        self.pex.unwrap_or(true)
    }

    /// Whether uTP is on, as it is when left out.
    #[must_use]
    pub fn utp_on(&self) -> bool {
        self.utp.unwrap_or(true)
    }

    /// `save_interval`, or its default.
    #[must_use]
    pub fn save(&self) -> Duration {
        if self.save_interval.is_zero() {
            DEFAULT_BT_SAVE_INTERVAL
        } else {
            self.save_interval
        }
    }

    /// `disk_workers`, or the number of CPUs, at most 8.
    #[must_use]
    pub fn disk_threads(&self) -> usize {
        if self.disk_workers > 0 {
            return self.disk_workers;
        }
        std::thread::available_parallelism().map_or(1, |n| n.get().min(8))
    }

    /// `limits`, or the defaults.
    #[must_use]
    pub fn limits_or_default(&self) -> BitTorrentLimits {
        self.limits.clone().unwrap_or_default()
    }

    /// `limits.max_torrents`, or its default.
    #[must_use]
    pub fn torrents(&self) -> usize {
        self.limits_or_default().torrents()
    }
}

/// `bittorrent.client.limits`: the client's rates and limits.
#[derive(Debug, Clone, Default)]
pub struct BitTorrentLimits {
    /// `upload_rate`: bytes a second uploaded, all torrents together; 0, unlimited.
    pub upload_rate: u64,
    /// `download_rate`: bytes a second downloaded, all torrents together; 0, unlimited.
    pub download_rate: u64,
    /// `max_conns_per_torrent`: [`DEFAULT_BT_MAX_CONNS_PER_TORRENT`] when left out or 0.
    pub max_conns_per_torrent: usize,
    /// `upload_slots`: [`DEFAULT_BT_UPLOAD_SLOTS`] when left out or 0.
    pub upload_slots: usize,
    /// `seed_ratio`: pause a torrent once it uploaded this many times its size; 0, never.
    pub seed_ratio: f64,
    /// `seed_time`: pause a torrent once it seeded this long; 0, never.
    pub seed_time: Duration,
    /// `max_torrents`: [`DEFAULT_BT_MAX_TORRENTS`] when left out or 0.
    pub max_torrents: usize,
    /// `max_active_downloads` (I153): the most torrents downloading at once; 0, no queue.
    pub max_active_downloads: usize,
    /// `max_active_seeds` (I153): the most torrents seeding at once; 0, no queue.
    pub max_active_seeds: usize,
    /// `min_free_bytes` (I153): [`DEFAULT_BT_MIN_FREE_BYTES`] when left out or 0.
    pub min_free_bytes: u64,
}

impl PartialEq for BitTorrentLimits {
    fn eq(&self, o: &Self) -> bool {
        self.upload_rate == o.upload_rate
            && self.download_rate == o.download_rate
            && self.max_conns_per_torrent == o.max_conns_per_torrent
            && self.upload_slots == o.upload_slots
            && self.seed_ratio.to_bits() == o.seed_ratio.to_bits()
            && self.seed_time == o.seed_time
            && self.max_torrents == o.max_torrents
            && self.max_active_downloads == o.max_active_downloads
            && self.max_active_seeds == o.max_active_seeds
            && self.min_free_bytes == o.min_free_bytes
    }
}

impl Eq for BitTorrentLimits {}

impl BitTorrentLimits {
    /// `max_conns_per_torrent`, or its default.
    #[must_use]
    pub fn conns_per_torrent(&self) -> usize {
        or(self.max_conns_per_torrent, DEFAULT_BT_MAX_CONNS_PER_TORRENT)
    }

    /// `upload_slots`, or its default.
    #[must_use]
    pub fn slots(&self) -> usize {
        or(self.upload_slots, DEFAULT_BT_UPLOAD_SLOTS)
    }

    /// `max_torrents`, or its default.
    #[must_use]
    pub fn torrents(&self) -> usize {
        or(self.max_torrents, DEFAULT_BT_MAX_TORRENTS)
    }

    /// `min_free_bytes`, or its default.
    #[must_use]
    pub fn free_bytes(&self) -> u64 {
        if self.min_free_bytes == 0 {
            DEFAULT_BT_MIN_FREE_BYTES
        } else {
            self.min_free_bytes
        }
    }
}

/// `bittorrent.tracker`: a tracker over HTTP (the routes' `bt_tracker` handler) and UDP (its own
/// `listen`), its swarms in memory.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BitTorrentTracker {
    /// `listen`: where UDP announces and scrapes are answered; none when left out.
    pub listen: String,
    /// `interval`: [`DEFAULT_BT_TRACKER_INTERVAL`] when left out or 0.
    pub interval: Duration,
    /// `min_interval`: the HTTP answers' `min interval`; none when left out.
    pub min_interval: Duration,
    /// `peer_ttl`: [`DEFAULT_BT_TRACKER_PEER_TTL`] when left out or 0.
    pub peer_ttl: Duration,
    /// `max_torrents`: [`DEFAULT_BT_TRACKER_MAX_TORRENTS`] when left out or 0.
    pub max_torrents: usize,
    /// `max_peers_per_torrent`: [`DEFAULT_BT_TRACKER_MAX_PEERS_PER_TORRENT`] when left out or 0.
    pub max_peers_per_torrent: usize,
    /// `max_numwant`: [`DEFAULT_BT_TRACKER_MAX_NUMWANT`] when left out or 0.
    pub max_numwant: usize,
    /// `max_scrape`: [`DEFAULT_BT_TRACKER_MAX_SCRAPE`] when left out or 0.
    pub max_scrape: usize,
    /// `allow`: the info hashes tracked, in hex (40 or 64 digits).
    pub allow: Vec<String>,
    /// `allow_files`: files of more allowed info hashes, one a line.
    pub allow_files: Vec<String>,
    /// `access`: the client addresses served, before the global access.
    pub access: Option<Access>,
    /// `console`: the tracker's pages; none when left out.
    pub console: Option<Console>,
}

impl BitTorrentTracker {
    /// `interval`, or its default.
    #[must_use]
    pub fn interval_or(&self) -> Duration {
        dur_or(self.interval, DEFAULT_BT_TRACKER_INTERVAL)
    }

    /// `peer_ttl`, or its default.
    #[must_use]
    pub fn ttl(&self) -> Duration {
        dur_or(self.peer_ttl, DEFAULT_BT_TRACKER_PEER_TTL)
    }

    /// `max_torrents`, or its default.
    #[must_use]
    pub fn torrents(&self) -> usize {
        or(self.max_torrents, DEFAULT_BT_TRACKER_MAX_TORRENTS)
    }

    /// `max_peers_per_torrent`, or its default.
    #[must_use]
    pub fn peers_per_torrent(&self) -> usize {
        or(
            self.max_peers_per_torrent,
            DEFAULT_BT_TRACKER_MAX_PEERS_PER_TORRENT,
        )
    }

    /// `max_numwant`, or its default.
    #[must_use]
    pub fn numwant(&self) -> usize {
        or(self.max_numwant, DEFAULT_BT_TRACKER_MAX_NUMWANT)
    }

    /// `max_scrape`, or its default.
    #[must_use]
    pub fn scrape(&self) -> usize {
        or(self.max_scrape, DEFAULT_BT_TRACKER_MAX_SCRAPE)
    }

    /// `allow_files` and the access's list files, which the role has watched.
    #[must_use]
    pub fn files(&self) -> Vec<String> {
        let mut out = self.allow_files.clone();
        if let Some(a) = &self.access {
            out.extend(a.allow_files.iter().cloned());
            out.extend(a.deny_files.iter().cloned());
        }
        out
    }
}

/// An info hash of a tracker's allow-list, fit's `ParseInfoHash`: 40 hex digits, a v1 hash, or
/// 64, a v2 hash cut to its first 20 bytes as trackers key it (BEP 52). Spaces around it are
/// ignored.
#[must_use]
pub fn parse_info_hash(s: &str) -> Option<[u8; 20]> {
    let s = s.trim();
    if s.len() != 40 && s.len() != 64 {
        return None;
    }
    let b = s.as_bytes();
    // A 64-digit hash is hex throughout, though only its first 20 bytes are kept.
    if !b.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let mut out = [0u8; 20];
    for (o, pair) in out.iter_mut().zip(b.chunks_exact(2)) {
        *o = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(out)
}

fn or(v: usize, default: usize) -> usize {
    if v == 0 { default } else { v }
}

fn dur_or(v: Duration, default: Duration) -> Duration {
    if v.is_zero() { default } else { v }
}

// What akit reads, and what fit reads that akit does not yet, object by object.

const BITTORRENT_FIELDS: &[&str] = &["client", "tracker"];
const BT_CLIENT_FIELDS: &[&str] = &[
    "listen",
    "data_dir",
    "download_dir",
    "encryption",
    "dht",
    "pex",
    "utp",
    "lsd",
    "disk_workers",
    "save_interval",
    "limits",
    "console",
    // I153.
    "categories",
    "watch_dir",
    "watch_interval",
    "watch_max_files",
    "url_timeout",
];
// I153: categories, watch_dir and its bounds, url_timeout; I154: alt_speed and access (akio's
// A284); disk_queue_bytes, akio-bittorrent's bound of the blocks waiting for the disk (A284);
// I156: publish; I159: rss.
const BT_CLIENT_FIT_ONLY: &[&str] = &["disk_queue_bytes", "alt_speed", "access", "rss", "publish"];
const BT_LIMITS_FIELDS: &[&str] = &[
    "upload_rate",
    "download_rate",
    "max_conns_per_torrent",
    "upload_slots",
    "seed_ratio",
    "seed_time",
    "max_torrents",
    // I153.
    "max_active_downloads",
    "max_active_seeds",
    "min_free_bytes",
];
// The client's connections in all and dials at once (akio's A284); the queue and the disk's free
// space (I153).
const BT_LIMITS_FIT_ONLY: &[&str] = &["max_conns", "max_half_open"];
const BT_CATEGORY_FIELDS: &[&str] = &["name", "dir"];
// I154: a category's limits; a media library's rescan, once akit reads `media_library`.
const BT_CATEGORY_FIT_ONLY: &[&str] = &["limits", "media_library"];
const BT_TRACKER_FIELDS: &[&str] = &[
    "listen",
    "interval",
    "min_interval",
    "peer_ttl",
    "max_torrents",
    "max_peers_per_torrent",
    "max_numwant",
    "max_scrape",
    "allow",
    "allow_files",
    "access",
    "console",
];
// The store's bound of every torrent's peers (akio's A285); auth, a media_auth policy, which akit
// does not read yet; the store several instances share (I159).
const BT_TRACKER_FIT_ONLY: &[&str] = &["max_peers", "auth", "store"];

/// Decodes `bittorrent`.
pub(crate) fn bittorrent(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<BitTorrent> {
    let mut o = d.object(n, path)?;
    let client = o
        .take("client")
        .and_then(|v| client(d, v, &join_field(path, "client")));
    let tracker = o
        .take("tracker")
        .and_then(|v| tracker(d, v, &join_field(path, "tracker")));
    o.finish(d, BITTORRENT_FIELDS, &[]);
    Some(BitTorrent { client, tracker })
}

/// A listener's address: a host and port, as fit's `checkAddress`.
fn check_listen(d: &mut Decoder<'_>, o: &Obj<'_>, address: &str, off: usize) {
    if let Err(msg) = crate::model::check_address(address) {
        d.error(off, &o.at("listen"), msg);
    } else {
        d.address_at(&o.at("listen"), off);
    }
}

fn client(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<BitTorrentClient> {
    let mut o = d.object(n, path)?;
    let mut c = BitTorrentClient::default();
    if let Some(v) = o.require(d, "listen") {
        if let Some(s) = d.string(v, &o.at("listen")) {
            check_listen(d, &o, &s, v.off);
            c.listen = s;
        }
    }
    if let Some(v) = o.require(d, "data_dir") {
        if let Some(s) = d.string(v, &o.at("data_dir")) {
            if s.trim().is_empty() {
                d.error(v.off, &o.at("data_dir"), "data_dir may not be empty");
            }
            c.data_dir = s;
        }
    }
    c.download_dir = text(d, &mut o, "download_dir");
    if let Some((s, off)) = text_at(d, &mut o, "encryption") {
        c.encryption = match s.as_str() {
            "allow" => BtEncryption::Allow,
            "prefer" => BtEncryption::Prefer,
            "require" => BtEncryption::Require,
            _ => {
                d.error(
                    off,
                    &o.at("encryption"),
                    format!("{s:?} is not one of allow, prefer, require"),
                );
                BtEncryption::Prefer
            }
        };
    }
    c.dht = bool_opt(d, &mut o, "dht");
    c.pex = bool_opt(d, &mut o, "pex");
    c.utp = bool_opt(d, &mut o, "utp");
    c.lsd = boolean(d, &mut o, "lsd");
    c.disk_workers = count(d, &mut o, "disk_workers", MAX_BT_DISK_WORKERS as i64);
    c.save_interval = duration(d, &mut o, "save_interval");
    c.limits = o.take("limits").and_then(|v| limits(d, v, &o.at("limits")));
    c.console = role_console(d, &mut o);
    let cats = list(d, &mut o, "categories", category);
    if cats.len() > MAX_BT_CATEGORIES {
        d.error(
            o.key_off("categories").unwrap_or(o.off),
            &o.at("categories"),
            format!("at most {MAX_BT_CATEGORIES} categories"),
        );
    }
    for (i, (cat, off, p)) in cats.iter().enumerate() {
        if cats.iter().take(i).any(|(o, _, _)| o.name == cat.name) {
            d.error(
                *off,
                &join_field(p, "name"),
                format!("another category is already named {:?}", cat.name),
            );
        }
    }
    c.categories = cats.into_iter().map(|(c, _, _)| c).collect();
    c.watch_dir = text(d, &mut o, "watch_dir");
    c.watch_interval = duration(d, &mut o, "watch_interval");
    c.watch_max_files = count(d, &mut o, "watch_max_files", MAX_BT_WATCH_MAX_FILES as i64);
    c.url_timeout = duration(d, &mut o, "url_timeout");
    o.finish(d, BT_CLIENT_FIELDS, BT_CLIENT_FIT_ONLY);
    Some(c)
}

fn limits(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<BitTorrentLimits> {
    let mut o = d.object(n, path)?;
    let l = BitTorrentLimits {
        upload_rate: bytes(d, &mut o, "upload_rate"),
        download_rate: bytes(d, &mut o, "download_rate"),
        max_conns_per_torrent: count(d, &mut o, "max_conns_per_torrent", i64::from(i32::MAX)),
        upload_slots: count(d, &mut o, "upload_slots", i64::from(i32::MAX)),
        seed_ratio: o
            .take("seed_ratio")
            .and_then(|v| {
                let at = o.at("seed_ratio");
                let r = d.number(v, &at)?;
                if r < 0.0 {
                    d.error(v.off, &at, format!("{r} is less than 0"));
                    return None;
                }
                Some(r)
            })
            .unwrap_or(0.0),
        seed_time: duration(d, &mut o, "seed_time"),
        max_torrents: count(d, &mut o, "max_torrents", MAX_BT_TORRENTS as i64),
        max_active_downloads: count(d, &mut o, "max_active_downloads", i64::from(i32::MAX)),
        max_active_seeds: count(d, &mut o, "max_active_seeds", i64::from(i32::MAX)),
        min_free_bytes: bytes(d, &mut o, "min_free_bytes"),
    };
    o.finish(d, BT_LIMITS_FIELDS, BT_LIMITS_FIT_ONLY);
    Some(l)
}

fn category(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<BitTorrentCategory> {
    let mut o = d.object(n, path)?;
    let mut c = BitTorrentCategory::default();
    if let Some(v) = o.require(d, "name") {
        if let Some(s) = d.string(v, &o.at("name")) {
            if !crate::model::valid_name(&s) {
                d.error(v.off, &o.at("name"), crate::model::bad_name(&s));
            }
            c.name = s;
        }
    }
    if let Some((dir, off)) = text_at(d, &mut o, "dir") {
        let t = dir.trim();
        let p = std::path::Path::new(t);
        let local = p.components().all(|c| {
            matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        });
        if !t.is_empty() && !p.is_absolute() && !local {
            d.error(
                off,
                &o.at("dir"),
                format!("{dir:?} leads out of download_dir: a relative dir stays below it"),
            );
        }
        c.dir = dir;
    }
    o.finish(d, BT_CATEGORY_FIELDS, BT_CATEGORY_FIT_ONLY);
    Some(c)
}

fn tracker(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<BitTorrentTracker> {
    let mut o = d.object(n, path)?;
    let mut t = BitTorrentTracker::default();
    if let Some((s, off)) = text_at(d, &mut o, "listen") {
        if !s.is_empty() {
            check_listen(d, &o, &s, off);
        }
        t.listen = s;
    }
    let interval_off = o.key_off("interval").unwrap_or(o.off);
    t.interval = duration(d, &mut o, "interval");
    let min_off = o.key_off("min_interval");
    t.min_interval = duration(d, &mut o, "min_interval");
    let ttl_off = o.key_off("peer_ttl").unwrap_or(interval_off);
    t.peer_ttl = duration(d, &mut o, "peer_ttl");
    t.max_torrents = count(d, &mut o, "max_torrents", MAX_BT_TRACKER_TORRENTS as i64);
    t.max_peers_per_torrent = count(
        d,
        &mut o,
        "max_peers_per_torrent",
        MAX_BT_TRACKER_PEERS_PER_TORRENT as i64,
    );
    t.max_numwant = count(d, &mut o, "max_numwant", MAX_BT_TRACKER_NUMWANT as i64);
    t.max_scrape = count(d, &mut o, "max_scrape", MAX_BT_TRACKER_SCRAPE as i64);
    let allow = list(d, &mut o, "allow", |d, n, p| {
        let s = d.string(n, p)?;
        if parse_info_hash(&s).is_none() {
            d.error(
                n.off,
                p,
                format!("{s:?} is not an info hash: 40 hex digits (v1) or 64 (v2)"),
            );
        }
        Some(s)
    });
    if allow.len() > MAX_BT_TRACKER_ALLOW {
        d.error(
            o.key_off("allow").unwrap_or(o.off),
            &o.at("allow"),
            format!(
                "{} info hashes: at most {MAX_BT_TRACKER_ALLOW}",
                allow.len()
            ),
        );
    }
    t.allow = allow.into_iter().map(|(s, _, _)| s).collect();
    let files = list(d, &mut o, "allow_files", |d, n, p| {
        let s = d.string(n, p)?;
        if s.trim().is_empty() {
            d.error(n.off, p, "a file name may not be empty");
        }
        Some(s)
    });
    t.allow_files = files.into_iter().map(|(s, _, _)| s).collect();
    t.access = access_of(d, &mut o);
    t.console = role_console(d, &mut o);
    if let Some(off) = min_off {
        if t.min_interval > t.interval_or() {
            d.error(
                off,
                &o.at("min_interval"),
                format!(
                    "min_interval {} is longer than the interval, {}",
                    crate::duration::format(t.min_interval),
                    crate::duration::format(t.interval_or())
                ),
            );
        }
    }
    if t.ttl() <= t.interval_or() {
        d.error(
            ttl_off,
            &o.at("peer_ttl"),
            format!(
                "peer_ttl {} is not longer than the interval, {}: every peer would be forgotten \
                 before it announces again",
                crate::duration::format(t.ttl()),
                crate::duration::format(t.interval_or())
            ),
        );
    }
    o.finish(d, BT_TRACKER_FIELDS, BT_TRACKER_FIT_ONLY);
    Some(t)
}

#[cfg(test)]
mod tests;
