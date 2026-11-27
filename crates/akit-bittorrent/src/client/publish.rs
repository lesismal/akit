//! Publishing directories as torrents (I156, fit's `bittorrent/publish.go`, `publish_walk.go`):
//! each `publish` entry's directory built into hybrid torrents (a torrent of each file, or one of
//! the whole directory) by akio-bittorrent's `Builder`, on a thread of the entry's own (at most
//! `max_hashing` building at once), never on an I/O thread, and seeded where the files are
//! through storage that refuses writes, added complete (`AddOptions::seed`) so that what was just
//! hashed is not checked again.
//!
//! The disk is followed by walking `dir` every `check_interval`, a look at the disk with no
//! event to wait on (as static files follow it): a file is told by its size, modification and
//! status change times, inode and device; a new or changed one is built once the walk before saw
//! it the same, so that a file being copied is not hashed half way; one changed or removed stops
//! its torrent at once. What was built is kept in `data_dir/publish/<name>/` (`index.json` with
//! each torrent's files and their stamps, `<id>.torrent`), so that a restart neither builds nor
//! checks a file that did not change. Names starting with a dot and symbolic links are left out.
//!
//! The published torrents are not the role's list: not in `torrents.json`, not counted by
//! `max_torrents`, seeding whatever the seed limits say. The `bt_publish` handler serves their
//! index, .torrent files and magnet links.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};

use akio_bittorrent::client::{
    AddOptions, Client, FileStamp, FileStorage, SeedLimits, Storage, Torrent,
};
use akio_bittorrent::metainfo::{Builder, MetaInfo, TorrentVersion};
use akit_config::bittorrent::{BitTorrentPublish, path_pattern_matches};
use akit_config::value::{self, Value};

use super::{lock, state::write_atomic};

/// The most problems an entry reports.
const MAX_PROBLEMS: usize = 20;

/// A file's stamp: size, modification and status change times (nanoseconds), inode, device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Stamp {
    size: u64,
    mtime: i128,
    ctime: i128,
    ino: u64,
    dev: u64,
}

fn stamp_of(m: &std::fs::Metadata) -> Stamp {
    let ns = |t: std::io::Result<std::time::SystemTime>| {
        t.ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| i128::try_from(d.as_nanos()).unwrap_or(0))
    };
    #[cfg(unix)]
    let (ctime, ino, dev) = {
        use std::os::unix::fs::MetadataExt;
        (
            i128::from(m.ctime())
                .saturating_mul(1_000_000_000)
                .saturating_add(i128::from(m.ctime_nsec())),
            m.ino(),
            m.dev(),
        )
    };
    #[cfg(not(unix))]
    let (ctime, ino, dev) = (0, 0, 0);
    Stamp {
        size: m.len(),
        mtime: ns(m.modified()),
        ctime,
        ino,
        dev,
    }
}

/// A file found by a walk: its path under `dir`, `/`-separated, and its stamp.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Found {
    rel: String,
    stamp: Stamp,
}

/// A torrent the entry publishes, as the index and the handler show it.
#[derive(Debug, Clone)]
pub struct Published {
    /// Its v1 info hash, in hex.
    pub id: String,
    /// The file's path under `dir`, `""` for a whole directory.
    pub path: String,
    /// The torrent's name.
    pub name: String,
    /// Its size in bytes.
    pub size: u64,
    /// Its files.
    pub files: usize,
    /// Its v2 info hash, in hex.
    pub info_hash_v2: String,
    /// Its magnet link.
    pub magnet: String,
    /// When it was built, RFC 3339.
    pub built: String,
    /// `seeding`, or `error`.
    pub state: String,
    /// What went wrong.
    pub error: String,
    /// The .torrent file.
    pub torrent: Arc<Vec<u8>>,
    files_seen: Vec<Found>,
    handle: Option<Torrent>,
}

/// An entry's state, which the handler reads.
#[derive(Debug, Clone, Default)]
pub struct EntryStatus {
    /// The entry's name.
    pub name: String,
    /// Its directory.
    pub dir: String,
    /// `per_file` or `whole_dir`.
    pub mode: String,
    /// Its web seed.
    pub web_seed: String,
    /// The torrents it publishes, by path.
    pub torrents: Vec<Published>,
    /// Files waiting to settle or be built.
    pub pending: usize,
    /// When the directory was last walked, RFC 3339.
    pub scanned: String,
    /// What was left out and why, at most 20.
    pub problems: Vec<String>,
    /// Why the directory could not be walked.
    pub error: String,
}

/// A running entry.
struct Running {
    cfg: BitTorrentPublish,
    status: Arc<Mutex<EntryStatus>>,
    stop: Arc<AtomicBool>,
    tx: SyncSender<()>,
}

/// The entries of a client.
#[derive(Default)]
pub(crate) struct Publisher {
    running: Vec<Running>,
}

impl std::fmt::Debug for Publisher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Publisher")
            .field("entries", &self.running.len())
            .finish()
    }
}

/// Writes refused: published files are never written to.
struct ReadOnly(FileStorage);

impl Storage for ReadOnly {
    fn read(&self, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
        self.0.read(offset, len)
    }

    fn write(&self, _offset: u64, data: &[u8]) -> std::io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "a published file is not written to",
        ))
    }

    fn may_have_data(&self) -> bool {
        true
    }

    fn file_stamps(&self) -> Option<Vec<Option<FileStamp>>> {
        self.0.file_stamps()
    }
}

/// The smallest power of two of at least 16 KiB that makes fewer than 2,048 pieces of `size`,
/// at most 16 MiB (fit's).
fn piece_length(size: u64) -> u32 {
    let mut l: u32 = 16 << 10;
    while l < 16 << 20 && size.checked_div(u64::from(l)).unwrap_or(0) >= 2048 {
        l = l.saturating_mul(2);
    }
    l
}

/// `s` with what a URL's path may not hold escaped.
fn escape_path(s: &str) -> String {
    use std::fmt::Write;
    s.bytes().fold(String::new(), |mut o, b| {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            o.push(char::from(b));
        } else {
            let _ = write!(o, "%{b:02X}");
        }
        o
    })
}

/// What one walk found: the files published, what was left out, whether the walk was whole.
struct Walk {
    files: Vec<Found>,
    left_out: bool,
    problems: Vec<String>,
}

fn walk(cfg: &BitTorrentPublish, dir: &Path) -> Result<Walk, String> {
    let max_entries = cfg.files_max().saturating_mul(10).saturating_add(10_000);
    let mut w = Walk {
        files: Vec::new(),
        left_out: false,
        problems: Vec::new(),
    };
    let mut seen = 0usize;
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    let top = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    drop(top);
    while let Some((d, rel)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            w.left_out = true;
            w.problems.push(format!("{}: not read", d.display()));
            continue;
        };
        for e in rd.flatten() {
            seen = seen.saturating_add(1);
            if seen > max_entries {
                w.left_out = true;
                w.problems.push(format!(
                    "more than {max_entries} entries walked: the rest left out"
                ));
                return Ok(w);
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let path = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            let Ok(m) = std::fs::symlink_metadata(e.path()) else {
                continue;
            };
            if name.starts_with('.') || m.file_type().is_symlink() {
                w.left_out = true;
                continue;
            }
            if cfg.exclude.iter().any(|p| path_pattern_matches(p, &path)) {
                w.left_out = true;
                continue;
            }
            if m.is_dir() {
                stack.push((e.path(), path));
            } else if m.is_file() {
                if !cfg.files.is_empty()
                    && !cfg.files.iter().any(|p| path_pattern_matches(p, &path))
                {
                    w.left_out = true;
                    continue;
                }
                w.files.push(Found {
                    rel: path,
                    stamp: stamp_of(&m),
                });
            }
        }
    }
    w.files.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(w)
}

/// A unit to publish: a file, or the whole directory (path `""`), and its files.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Unit {
    path: String,
    files: Vec<Found>,
}

/// The units of a walk under the entry's bounds, and the problems.
fn units(cfg: &BitTorrentPublish, w: Walk) -> (Vec<Unit>, Vec<String>) {
    let mut problems = w.problems;
    let (max_files, max_bytes) = (cfg.files_max(), cfg.total_bytes());
    if cfg.whole_dir {
        let total: u64 = w.files.iter().map(|f| f.stamp.size).sum();
        if w.left_out {
            // akio's Builder takes a directory whole: one with files left out (dotfiles,
            // symbolic links, files or exclude) cannot be built as fit builds it.
            problems.push(
                "whole_dir: the directory has files left out, which akio-bittorrent's builder of a \
                 directory would put in: not published"
                    .to_owned(),
            );
            return (Vec::new(), problems);
        }
        if w.files.len() > max_files || total > max_bytes || w.files.is_empty() {
            problems.push(format!(
                "whole_dir: {} files of {total} bytes, past max_files {max_files} or max_total_bytes {max_bytes}, or none: not published",
                w.files.len()
            ));
            return (Vec::new(), problems);
        }
        return (
            vec![Unit {
                path: String::new(),
                files: w.files,
            }],
            problems,
        );
    }
    let mut out = Vec::new();
    let mut total = 0u64;
    for f in w.files {
        if f.stamp.size == 0 {
            continue;
        }
        if out.len() >= max_files || total.saturating_add(f.stamp.size) > max_bytes {
            problems.push(format!(
                "{}: past max_files or max_total_bytes, left out",
                f.rel
            ));
            continue;
        }
        total = total.saturating_add(f.stamp.size);
        out.push(Unit {
            path: f.rel.clone(),
            files: vec![f],
        });
    }
    (out, problems)
}

/// Builds the torrent of `u` in entry `cfg` whose directory is `dir`.
fn build(cfg: &BitTorrentPublish, dir: &Path, u: &Unit) -> Result<MetaInfo, String> {
    let size: u64 = u.files.iter().map(|f| f.stamp.size).sum();
    let pl = if cfg.piece_length > 0 {
        u32::try_from(cfg.piece_length).unwrap_or(16 << 20)
    } else {
        piece_length(size)
    };
    let (name, path) = if u.path.is_empty() {
        let n = dir
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        (n, dir.to_path_buf())
    } else {
        let n = u.path.rsplit('/').next().unwrap_or(&u.path).to_owned();
        (n, dir.join(&u.path))
    };
    let mut b = Builder::new(name)
        .piece_length(pl)
        .version(TorrentVersion::Hybrid)
        .private(cfg.private);
    for a in &cfg.announce {
        b = b.tracker(a.clone());
    }
    if !cfg.web_seed.is_empty() {
        let ws = if u.path.is_empty() {
            // Clients ask the URL's parent for <name>/<path>.
            let t = cfg.web_seed.trim_end_matches('/');
            format!("{}/", t.rsplit_once('/').map_or(t, |(p, _)| p))
        } else {
            let base = if cfg.web_seed.ends_with('/') {
                cfg.web_seed.clone()
            } else {
                format!("{}/", cfg.web_seed)
            };
            format!("{base}{}", escape_path(&u.path))
        };
        b = b.web_seed(ws);
    }
    let mi = b.build_path(&path).map_err(|e| e.to_string())?;
    // As a client reads it back.
    MetaInfo::parse(&mi.to_bytes()).map_err(|e| e.to_string())
}

fn now_rfc3339() -> String {
    super::now_rfc3339()
}

/// The entry's state directory and the key its index is kept under.
fn key(cfg: &BitTorrentPublish) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        cfg.dir,
        cfg.whole_dir,
        cfg.web_seed,
        cfg.announce.join(","),
        cfg.private,
        cfg.piece_length
    )
}

fn stamp_json(f: &Found) -> Value {
    let n = |v: String| Value::Number(v);
    Value::Object(vec![
        ("path".into(), Value::String(f.rel.clone())),
        ("size".into(), n(f.stamp.size.to_string())),
        ("mtime".into(), n(f.stamp.mtime.to_string())),
        ("ctime".into(), n(f.stamp.ctime.to_string())),
        ("ino".into(), n(f.stamp.ino.to_string())),
        ("dev".into(), n(f.stamp.dev.to_string())),
    ])
}

fn found_of(v: &Value) -> Option<Found> {
    let num = |k: &str| match v.get(k) {
        Some(Value::Number(n)) => n.parse::<i128>().ok(),
        _ => None,
    };
    let Some(Value::String(rel)) = v.get("path") else {
        return None;
    };
    Some(Found {
        rel: rel.clone(),
        stamp: Stamp {
            size: u64::try_from(num("size")?).ok()?,
            mtime: num("mtime")?,
            ctime: num("ctime")?,
            ino: u64::try_from(num("ino")?).ok()?,
            dev: u64::try_from(num("dev")?).ok()?,
        },
    })
}

/// Writes the entry's index: each torrent's ID, path and files' stamps.
fn save_index(state: &Path, k: &str, torrents: &[Published]) {
    let list = torrents
        .iter()
        .filter(|t| t.state != "error")
        .map(|t| {
            Value::Object(vec![
                ("id".into(), Value::String(t.id.clone())),
                ("path".into(), Value::String(t.path.clone())),
                ("built".into(), Value::String(t.built.clone())),
                (
                    "files".into(),
                    Value::Array(t.files_seen.iter().map(stamp_json).collect()),
                ),
            ])
        })
        .collect();
    let doc = Value::Object(vec![
        ("key".into(), Value::String(k.to_owned())),
        ("torrents".into(), Value::Array(list)),
    ]);
    let _ = write_atomic(&state.join("index.json"), doc.to_json().as_bytes());
}

/// What the index kept under key `k`: by path, the torrent's files, .torrent and build time.
fn load_index(state: &Path, k: &str) -> HashMap<String, (Vec<Found>, MetaInfo, Vec<u8>, String)> {
    let mut out = HashMap::new();
    let Ok(text) = std::fs::read_to_string(state.join("index.json")) else {
        return out;
    };
    let Ok(v) = value::parse(&text) else {
        return out;
    };
    if v.get("key") != Some(&Value::String(k.to_owned())) {
        return out;
    }
    let Some(Value::Array(list)) = v.get("torrents") else {
        return out;
    };
    for t in list {
        let (Some(Value::String(id)), Some(Value::String(path)), Some(Value::Array(files))) =
            (t.get("id"), t.get("path"), t.get("files"))
        else {
            continue;
        };
        let built = match t.get("built") {
            Some(Value::String(b)) => b.clone(),
            _ => String::new(),
        };
        let Ok(bytes) = std::fs::read(state.join(format!("{id}.torrent"))) else {
            continue;
        };
        let Ok(mi) = MetaInfo::parse(&bytes) else {
            continue;
        };
        let found: Option<Vec<Found>> = files.iter().map(found_of).collect();
        if let Some(found) = found {
            out.insert(path.clone(), (found, mi, bytes, built));
        }
    }
    out
}

/// Adds `mi`, whose files are below `dir` (as `FileStorage` lays them out), to `client`,
/// complete and read only, seeding for ever.
fn seed(client: &Client, dir: &Path, mi: &MetaInfo) -> Result<Torrent, String> {
    let mut o = AddOptions::default();
    o.seed = true;
    o.seed_limits = Some(SeedLimits::default());
    o.storage = Some(Arc::new(ReadOnly(FileStorage::new(dir, &mi.info))));
    client.add_torrent(mi.clone(), o).map_err(|e| e.to_string())
}

fn hex(b: &[u8]) -> String {
    super::hex(b)
}

/// An entry's thread: a walk every `check_interval` until stopped.
#[allow(clippy::too_many_lines)]
fn run_entry(
    cfg: BitTorrentPublish,
    client: Client,
    state: PathBuf,
    status: Arc<Mutex<EntryStatus>>,
    stop: Arc<AtomicBool>,
    rx: Receiver<()>,
) {
    let dir = std::path::absolute(&cfg.dir).unwrap_or_else(|_| PathBuf::from(&cfg.dir));
    let k = key(&cfg);
    let _ = std::fs::create_dir_all(&state);
    let mut kept = load_index(&state, &k);
    let mut previous: HashMap<String, Vec<Found>> = HashMap::new();
    // Builds that failed, by path, with the files they were tried on: not tried again until
    // those change.
    let mut failed: HashMap<String, Vec<Found>> = HashMap::new();
    let mut current: Vec<Published> = Vec::new();
    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let mut problems = Vec::new();
        let walked = walk(&cfg, &dir);
        let mut pending = 0usize;
        let mut changed = false;
        match walked {
            Err(e) => {
                lock(&status).error = e;
            }
            Ok(w) => {
                let (us, p) = units(&cfg, w);
                problems.extend(p);
                let want: HashMap<&str, &Unit> = us.iter().map(|u| (u.path.as_str(), u)).collect();
                // Torrents whose unit went or changed stop at once.
                current.retain(|t| {
                    let keep = want
                        .get(t.path.as_str())
                        .is_some_and(|u| u.files == t.files_seen);
                    if !keep {
                        if let Some(h) = &t.handle {
                            client.remove(&h.info_hash());
                        }
                        let _ = std::fs::remove_file(state.join(format!("{}.torrent", t.id)));
                        changed = true;
                    }
                    keep
                });
                let mut to_build = Vec::new();
                for u in &us {
                    if current.iter().any(|t| t.path == u.path) {
                        continue;
                    }
                    // Restored from the index, its files unchanged: seeded without a build.
                    if let Some((files, mi, bytes, built)) = kept.remove(&u.path) {
                        if files == u.files {
                            current
                                .push(published(&client, &dir, u, mi, bytes, built, &state, false));
                            changed = true;
                            continue;
                        }
                    }
                    if failed.get(&u.path) == Some(&u.files) {
                        continue;
                    }
                    if previous.get(&u.path) == Some(&u.files) {
                        to_build.push(u.clone());
                    } else {
                        pending = pending.saturating_add(1);
                    }
                }
                // At most max_hashing builds at once, each on a thread of its own.
                for batch in to_build.chunks(cfg.hashing()) {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    let results: Vec<(Unit, Result<MetaInfo, String>)> = std::thread::scope(|s| {
                        let hs: Vec<_> = batch
                            .iter()
                            .map(|u| (u.clone(), s.spawn(|| build(&cfg, &dir, u))))
                            .collect();
                        hs.into_iter()
                            .map(|(u, h)| {
                                (
                                    u,
                                    h.join()
                                        .unwrap_or_else(|_| Err("the build failed".to_owned())),
                                )
                            })
                            .collect()
                    });
                    for (u, r) in results {
                        match r {
                            Ok(mi) => {
                                let bytes = mi.to_bytes();
                                current.push(published(
                                    &client,
                                    &dir,
                                    &u,
                                    mi,
                                    bytes,
                                    now_rfc3339(),
                                    &state,
                                    true,
                                ));
                                changed = true;
                            }
                            Err(e) => {
                                problems.push(format!(
                                    "{}: {e}",
                                    if u.path.is_empty() { "." } else { &u.path }
                                ));
                                failed.insert(u.path.clone(), u.files.clone());
                            }
                        }
                    }
                }
                previous = us.into_iter().map(|u| (u.path, u.files)).collect();
                failed.retain(|p, _| previous.contains_key(p));
                let mut st = lock(&status);
                st.error.clear();
            }
        }
        current.sort_by(|a, b| a.path.cmp(&b.path));
        if changed {
            save_index(&state, &k, &current);
        }
        {
            let mut st = lock(&status);
            st.torrents.clone_from(&current);
            st.pending = pending;
            st.scanned = now_rfc3339();
            problems.truncate(MAX_PROBLEMS);
            st.problems = problems;
        }
        match rx.recv_timeout(cfg.interval()) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
    for t in &current {
        if let Some(h) = &t.handle {
            client.remove(&h.info_hash());
        }
    }
}

/// The published torrent of `u`, added to `client`; its .torrent written when `fresh`.
#[allow(clippy::too_many_arguments)]
fn published(
    client: &Client,
    dir: &Path,
    u: &Unit,
    mi: MetaInfo,
    bytes: Vec<u8>,
    built: String,
    state: &Path,
    fresh: bool,
) -> Published {
    let id = hex(&mi.info_hash.0);
    if fresh {
        let _ = write_atomic(&state.join(format!("{id}.torrent")), &bytes);
    }
    let storage_dir = if u.path.is_empty() {
        dir.parent()
            .map_or_else(|| dir.to_path_buf(), Path::to_path_buf)
    } else {
        dir.join(&u.path)
            .parent()
            .map_or_else(|| dir.to_path_buf(), Path::to_path_buf)
    };
    let (handle, state_s, error) = match seed(client, &storage_dir, &mi) {
        Ok(t) => (Some(t), "seeding", String::new()),
        Err(e) => (None, "error", e),
    };
    Published {
        id,
        path: u.path.clone(),
        name: mi.info.name.clone(),
        size: mi.info.total_length,
        files: mi.info.files.iter().filter(|f| !f.padding).count(),
        info_hash_v2: mi.info_hash_v2.map(|h| h.to_hex()).unwrap_or_default(),
        magnet: mi.magnet().to_string(),
        built,
        state: state_s.to_owned(),
        error,
        torrent: Arc::new(bytes),
        files_seen: u.files.clone(),
        handle,
    }
}

impl Publisher {
    /// Moves the entries to `want`: those that changed or went stopped (a removed one's state
    /// removed), new ones started, the rest kept.
    pub(crate) fn apply(&mut self, want: &[BitTorrentPublish], client: &Client, data_dir: &Path) {
        let root = data_dir.join("publish");
        self.running.retain(|r| {
            let keep = want.contains(&r.cfg);
            if !keep {
                r.stop.store(true, Ordering::Release);
                let _ = r.tx.try_send(());
                if !want.iter().any(|w| w.name == r.cfg.name) {
                    let _ = std::fs::remove_dir_all(root.join(&r.cfg.name));
                }
            }
            keep
        });
        for w in want {
            if self.running.iter().any(|r| r.cfg == *w) {
                continue;
            }
            let status = Arc::new(Mutex::new(EntryStatus {
                name: w.name.clone(),
                dir: w.dir.clone(),
                mode: if w.whole_dir { "whole_dir" } else { "per_file" }.to_owned(),
                web_seed: w.web_seed.clone(),
                ..EntryStatus::default()
            }));
            let stop = Arc::new(AtomicBool::new(false));
            let (tx, rx) = sync_channel(1);
            let (c, cl, st, sp, state) = (
                w.clone(),
                client.clone(),
                Arc::clone(&status),
                Arc::clone(&stop),
                root.join(&w.name),
            );
            // A thread of the entry's own; never joined: a build under way (akio's Builder
            // cannot be stopped) ends on its own, its result dropped.
            let _ = std::thread::Builder::new()
                .name(format!("akit-bt-publish-{}", w.name))
                .spawn(move || run_entry(c, cl, state, st, sp, rx));
            self.running.push(Running {
                cfg: w.clone(),
                status,
                stop,
                tx,
            });
        }
    }

    /// Stops every entry.
    pub(crate) fn stop(&mut self) {
        for r in self.running.drain(..) {
            r.stop.store(true, Ordering::Release);
            let _ = r.tx.try_send(());
        }
    }

    /// The entries' states, by the config's order.
    pub(crate) fn statuses(&self) -> Vec<Arc<Mutex<EntryStatus>>> {
        self.running.iter().map(|r| Arc::clone(&r.status)).collect()
    }
}
