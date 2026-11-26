//! Log files (I81, decision 033): a sink per path, which every log writing there shares, written
//! by a thread of its own, as fit's `logs.Files` and `logs.File`.
//!
//! A write copies into the sink's queue and returns: past [`QUEUE_BYTES`] (or 256 buffers) it is
//! refused and counted, never waited for, so no logger, request or I/O thread waits for the disk
//! (rules 1 and 11). The writer rotates the file before a write that would take it past
//! `max_bytes` (a buffer larger than that gets a file of its own) and at the first write in a
//! later period of `interval`, periods counted from midnight in the log's zone, whichever comes
//! first; a rotated file is named `<path>.<yyyy-mm-ddThh-mm-ss.mmm>`, the rotation's time in UTC,
//! with `-1`, `-2` when the name is taken and `.gz` once compressed. Compression runs on a thread
//! of the set's own, started with the first one, a queue of 16; a failure keeps the plain file.
//! Retention, at open, after each rotation and every ten minutes, removes the rotated files
//! older than `max_age`, then those past `max_files`, then the oldest until they total at most
//! `max_total_bytes`: only the names a rotation makes from that path, never the open file.
//!
//! No name planted in the log's directory leads a sink to write, read or remove another file: a
//! file is opened only when it is a regular file with one link, and is the file the name held
//! when it was looked at (`O_NONBLOCK`, so a FIFO cannot hold the open); a new file and a
//! compression's temporary file are created exclusively.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use akit_config::Rotation;

use crate::time;
use crate::zone::Zone;

/// The most bytes a sink holds that its writer has not written: past it a write is refused, and
/// counted, rather than waited for (fit's `QueueBytes`).
pub const QUEUE_BYTES: usize = 1 << 20;
/// The most buffers a sink's queue holds, whatever their size.
const QUEUE_MESSAGES: usize = 256;
/// How often a sink removes the rotated files past their age even though nothing rotated.
pub const SWEEP_EVERY: Duration = Duration::from_secs(600);
/// The compressions waiting at most, and how long one may wait before it is given up.
const COMPRESS_QUEUE: usize = 16;
const COMPRESS_TIMEOUT: Duration = Duration::from_secs(60);
/// The most rotated names a sweep looks at.
const MAX_ROTATED_NAMES: usize = 10_000;
/// The most names a rotation tries, the time's and its `-1`, `-2` and so on.
const MAX_BACKUP_NAMES: usize = 1000;
/// How long a size or interval rotation that failed waits before it is tried again.
const ROTATE_RETRY: Duration = Duration::from_secs(60);
/// The mode a log file is created with: its owner writes it, its group may read it, others may
/// not (access logs hold addresses, URLs and user names). A file already there keeps its mode; a
/// file a rotation starts takes the mode of the one it replaces.
const FILE_MODE: u32 = 0o640;
/// How long closing a sink waits for the compressions of its files.
const CLOSE_WAIT: Duration = Duration::from_secs(30);
/// What compression reads at a time.
const CHUNK: usize = 64 << 10;

/// What tells a sink the time: the system's clock, or a test's.
pub type Clock = Arc<dyn Fn() -> SystemTime + Send + Sync>;
/// What is told a sink's failures: a line for akit's log.
pub type Report = Arc<dyn Fn(&str) + Send + Sync>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Why a write was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The sink is closed.
    Closed,
    /// Its queue is full: the write is counted as dropped.
    Full,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Refused::Closed => "the log file is closed",
            Refused::Full => "the log file's queue is full",
        })
    }
}

impl std::error::Error for Refused {}

/// The set of log files the process has open: one sink a path, shared by every log writing
/// there. Cloning it is cheap: the clones are the same set.
#[derive(Clone)]
pub struct Files(Arc<FilesInner>);

struct FilesInner {
    sinks: Mutex<HashMap<String, Arc<Sink>>>,
    common: Arc<Common>,
}

/// What the sinks of a set share.
struct Common {
    clock: Clock,
    sweep_every: Duration,
    report: Report,
    /// The compression thread's queue, made with the first compression.
    compressor: OnceLock<Option<SyncSender<Job>>>,
}

impl fmt::Debug for Files {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Files")
            .field("open", &lock(&self.0.sinks).len())
            .finish_non_exhaustive()
    }
}

impl Default for Files {
    fn default() -> Files {
        Files::new()
    }
}

impl Files {
    /// An empty set, on the system's clock, its failures written by the `engine` logger at warn,
    /// as fit's.
    #[must_use]
    pub fn new() -> Files {
        Files::with(
            Arc::new(SystemTime::now),
            SWEEP_EVERY,
            crate::logger::lines(
                crate::logger::Logger::Engine,
                crate::logger::Level::Warn,
                "",
            ),
        )
    }

    /// An empty set on `clock`, sweeping every `sweep_every`, its sinks' failures told to
    /// `report`.
    #[must_use]
    pub fn with(clock: Clock, sweep_every: Duration, report: Report) -> Files {
        Files(Arc::new(FilesInner {
            sinks: Mutex::new(HashMap::new()),
            common: Arc::new(Common {
                clock,
                sweep_every,
                report,
                compressor: OnceLock::new(),
            }),
        }))
    }

    /// The sink of the file at `path`, opened if no log writes there yet, rotated as `rotation`
    /// says (none: one file that grows) and its periods counted in `zone`. Opens a file and looks
    /// at its directory: control plane only.
    ///
    /// # Errors
    ///
    /// When the file cannot be opened, or is open with other rotation settings: a file has one.
    pub fn open(&self, path: &str, rotation: Option<Rotation>, zone: Zone) -> io::Result<LogFile> {
        let key = akit_config::output_path(path);
        let mut sinks = lock(&self.0.sinks);
        if let Some(s) = sinks.get(&key) {
            if *lock(&s.rot) != rotation {
                return Err(io::Error::other(format!(
                    "log file {path:?} is open with other rotation settings: a file has one rotate"
                )));
            }
            s.refs.fetch_add(1, Ordering::Relaxed);
            return Ok(LogFile {
                sink: Arc::clone(s),
                files: self.clone(),
            });
        }
        let s = Sink::open(&key, rotation, zone, Arc::clone(&self.0.common))?;
        sinks.insert(key, Arc::clone(&s));
        Ok(LogFile {
            sink: s,
            files: self.clone(),
        })
    }

    /// The sink of `path`, when one is open.
    #[must_use]
    pub fn get(&self, path: &str) -> Option<LogFile> {
        let key = akit_config::output_path(path);
        let sinks = lock(&self.0.sinks);
        let s = sinks.get(&key)?;
        s.refs.fetch_add(1, Ordering::Relaxed);
        Some(LogFile {
            sink: Arc::clone(s),
            files: self.clone(),
        })
    }

    /// What each open sink holds, by path.
    #[must_use]
    pub fn list(&self) -> Vec<Info> {
        let sinks: Vec<Arc<Sink>> = lock(&self.0.sinks).values().cloned().collect();
        let mut infos: Vec<Info> = sinks.iter().map(|s| s.info()).collect();
        infos.sort_by(|a, b| a.path.cmp(&b.path));
        infos
    }

    /// Lets go of one reference to `s`, closing it when it was the last.
    fn release(&self, s: &Arc<Sink>) {
        let last = {
            let mut sinks = lock(&self.0.sinks);
            let last = s.refs.fetch_sub(1, Ordering::Relaxed) <= 1;
            if last {
                sinks.remove(&s.path);
            }
            last
        };
        if last {
            s.close();
        }
    }
}

/// One log's hold on a sink: writes go to the sink's queue. The sink closes, once what it
/// queued is written, when its last holder is dropped (which waits for its writer: control
/// plane only).
pub struct LogFile {
    sink: Arc<Sink>,
    files: Files,
}

impl fmt::Debug for LogFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LogFile")
            .field("path", &self.sink.path)
            .finish_non_exhaustive()
    }
}

impl Drop for LogFile {
    fn drop(&mut self) {
        self.files.release(&self.sink);
    }
}

impl LogFile {
    /// The file's path, cleaned.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.sink.path
    }

    /// Queues a copy of `p` to be written. Never waits for the disk.
    ///
    /// # Errors
    ///
    /// [`Refused::Full`] when the queue has no room (counted as dropped), [`Refused::Closed`]
    /// once the sink is closed.
    pub fn write(&self, p: &[u8]) -> Result<(), Refused> {
        if p.is_empty() {
            return Ok(());
        }
        self.write_owned(p.to_vec()).map_err(|(r, _)| r)
    }

    /// Queues `b` to be written, without a copy. Never waits for the disk.
    ///
    /// # Errors
    ///
    /// As [`write`](LogFile::write), with `b` handed back.
    pub fn write_owned(&self, b: Vec<u8>) -> Result<(), (Refused, Vec<u8>)> {
        self.sink.write(b)
    }

    /// Has the file rotated and kept as `rotation` says from its next write on, as a reload does.
    pub fn set_rotation(&self, rotation: Option<Rotation>) {
        *lock(&self.sink.rot) = rotation;
    }

    /// Has the file's periods counted in `zone` from now on.
    pub fn set_zone(&self, zone: Zone) {
        *lock(&self.sink.zone) = Arc::new(zone);
    }

    /// The rotation settings it has now.
    #[must_use]
    pub fn rotation(&self) -> Option<Rotation> {
        *lock(&self.sink.rot)
    }

    /// Rotates the file now, after what is queued is written. Waits for the writer: control plane
    /// only.
    ///
    /// # Errors
    ///
    /// When the rotation fails, or the sink is closed.
    pub fn rotate_now(&self) -> Result<(), String> {
        self.sink.control(Ctl::Rotate).map(|_| ())
    }

    /// Removes the rotated files past the retention settings now, or with `dry_run` only says
    /// which it would. Waits for the writer: control plane only.
    ///
    /// # Errors
    ///
    /// When the directory cannot be read, or the sink is closed.
    pub fn cleanup_now(&self, dry_run: bool) -> Result<Vec<Removed>, String> {
        self.sink.control(Ctl::Cleanup(dry_run))
    }

    /// Waits until what was queued before is written. Control plane only.
    ///
    /// # Errors
    ///
    /// When the sink is closed.
    pub fn sync(&self) -> Result<(), String> {
        self.sink.control(Ctl::Sync).map(|_| ())
    }

    /// What the sink holds now, its files listed from its directory. Reads the directory:
    /// control plane only.
    #[must_use]
    pub fn info(&self) -> Info {
        self.sink.info()
    }

    /// The writes refused since it was opened.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.sink.dropped.load(Ordering::Relaxed)
    }
}

/// What the writer is sent.
enum Msg {
    Buf(Vec<u8>),
    Ctl(Ctl, mpsc::Sender<Result<Vec<Removed>, String>>),
    Stop,
}

/// An operation the writer runs, in its turn after what was queued before.
#[derive(Debug, Clone, Copy)]
enum Ctl {
    Rotate,
    Cleanup(bool),
    Sync,
}

/// A rotated file a cleanup removed, or would remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    /// Its name in the log's directory.
    pub name: String,
    /// Its size.
    pub size: u64,
    /// When it was last written.
    pub modified: SystemTime,
}

/// A sink as the Admin API shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    /// The file's path, cleaned.
    pub path: String,
    /// The bytes the open file holds.
    pub current_size: u64,
    /// The writes refused since it was opened.
    pub dropped: u64,
    /// What its last failure was, empty when none.
    pub last_error: String,
    /// The open file first, then the rotated ones, newest first.
    pub files: Vec<FileInfo>,
}

/// One file of a sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    /// Its name in the log's directory.
    pub name: String,
    /// Its size.
    pub size: u64,
    /// When it was last written; `None` when it could not be looked at.
    pub modified: Option<SystemTime>,
    /// Whether it is the open file.
    pub current: bool,
}

/// A log file's sink: its queue and the settings its writer reads.
struct Sink {
    path: String,
    dir: PathBuf,
    base: String,
    rot: Mutex<Option<Rotation>>,
    zone: Mutex<Arc<Zone>>,
    common: Arc<Common>,
    tx: SyncSender<Msg>,
    /// Whether the sink is closed: a write holds it for reading while it queues, so the queue is
    /// never stopped under it.
    closed: RwLock<bool>,
    /// Bytes queued and not yet written.
    queued: AtomicUsize,
    dropped: AtomicU64,
    last_err: Mutex<String>,
    /// The rotated files being compressed, which a sweep leaves alone.
    compressing: Mutex<HashSet<PathBuf>>,
    /// The size of the open file, as the writer last wrote it.
    size: AtomicU64,
    writer: Mutex<Option<JoinHandle<()>>>,
    /// The logs holding it, counted under the set's lock.
    refs: AtomicUsize,
}

impl Sink {
    fn open(
        path: &str,
        rot: Option<Rotation>,
        zone: Zone,
        common: Arc<Common>,
    ) -> io::Result<Arc<Sink>> {
        let p = Path::new(path);
        let dir = match p.parent() {
            Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let base = p
            .file_name()
            .map(|b| b.to_string_lossy().into_owned())
            .ok_or_else(|| io::Error::other(format!("log file {path:?} names no file")))?;
        let (file, meta) = open_append(p, FILE_MODE)?;
        if let Ok(m) = fs::metadata(&dir) {
            let mode = m.permissions().mode() & 0o777;
            if mode & 0o022 != 0 {
                (common.report)(&format!(
                    "log {path}: its directory {} is writable by users other than its owner ({mode:o}); \
                     akit does not follow links there, but such a user can still remove or rename the log's files",
                    dir.display()
                ));
            }
        }
        let (tx, rx) = mpsc::sync_channel(QUEUE_MESSAGES);
        let zone = Arc::new(zone);
        let sink = Arc::new(Sink {
            path: path.to_owned(),
            dir,
            base,
            rot: Mutex::new(rot),
            zone: Mutex::new(Arc::clone(&zone)),
            common,
            tx,
            closed: RwLock::new(false),
            queued: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
            last_err: Mutex::new(String::new()),
            compressing: Mutex::new(HashSet::new()),
            size: AtomicU64::new(meta.len()),
            writer: Mutex::new(None),
            refs: AtomicUsize::new(1),
        });
        let mut w = Writer {
            sink: Arc::clone(&sink),
            file: Some(file),
            size: meta.len(),
            period_start: None,
            retry_at: None,
        };
        if let Some(r) = rot.filter(|r| !r.interval.is_zero()) {
            w.period_start = Some(period_start((sink.common.clock)(), r.interval, &zone));
        }
        let handle = std::thread::Builder::new()
            .name("akit-log-file".into())
            .spawn(move || w.run(&rx))?;
        *lock(&sink.writer) = Some(handle);
        Ok(sink)
    }

    fn write(&self, b: Vec<u8>) -> Result<(), (Refused, Vec<u8>)> {
        let closed = self.closed.read().unwrap_or_else(PoisonError::into_inner);
        if *closed {
            return Err((Refused::Closed, b));
        }
        let len = b.len();
        if self.queued.load(Ordering::Relaxed).saturating_add(len) > QUEUE_BYTES {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return Err((Refused::Full, b));
        }
        self.queued.fetch_add(len, Ordering::Relaxed);
        match self.tx.try_send(Msg::Buf(b)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(m) | TrySendError::Disconnected(m)) => {
                self.queued.fetch_sub(len, Ordering::Relaxed);
                self.dropped.fetch_add(1, Ordering::Relaxed);
                let b = match m {
                    Msg::Buf(b) => b,
                    _ => Vec::new(),
                };
                Err((Refused::Full, b))
            }
        }
    }

    fn control(&self, c: Ctl) -> Result<Vec<Removed>, String> {
        let (reply, rx) = mpsc::channel();
        {
            let closed = self.closed.read().unwrap_or_else(PoisonError::into_inner);
            if *closed {
                return Err(Refused::Closed.to_string());
            }
            // The writer is the only receiver, and running: this waits only for it to make room.
            self.tx
                .send(Msg::Ctl(c, reply))
                .map_err(|_| Refused::Closed.to_string())?;
        }
        rx.recv().map_err(|_| Refused::Closed.to_string())?
    }

    /// Writes what is queued, closes the file and stops the writer. Waits for it.
    fn close(&self) {
        {
            let mut closed = self.closed.write().unwrap_or_else(PoisonError::into_inner);
            if *closed {
                return;
            }
            *closed = true;
            let _ = self.tx.send(Msg::Stop);
        }
        if let Some(h) = lock(&self.writer).take() {
            let _ = h.join();
        }
        // The compressions of its rotated files finish first, within a bound, so a stop does not
        // leave a temporary file behind.
        let until = after(CLOSE_WAIT);
        while !lock(&self.compressing).is_empty() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Remembers `msg` as the sink's last failure and says it.
    fn fail(&self, msg: String) {
        (self.common.report)(&format!("log {}: {msg}", self.path));
        *lock(&self.last_err) = msg;
    }

    fn rotation(&self) -> Option<Rotation> {
        *lock(&self.rot)
    }

    fn zone(&self) -> Arc<Zone> {
        Arc::clone(&lock(&self.zone))
    }

    /// Whether `name` is one a rotation made from this file.
    fn is_rotated(&self, name: &str) -> bool {
        name.strip_prefix(self.base.as_str())
            .and_then(|n| n.strip_prefix('.'))
            .is_some_and(rotated_suffix)
    }

    fn is_compressing(&self, name: &str) -> bool {
        lock(&self.compressing).contains(&self.dir.join(name))
    }

    fn info(&self) -> Info {
        let mut info = Info {
            path: self.path.clone(),
            current_size: self.size.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            last_error: lock(&self.last_err).clone(),
            files: Vec::new(),
        };
        let mut rotated: Vec<FileInfo> = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for e in entries.flatten().take(MAX_ROTATED_NAMES) {
                let name = e.file_name().to_string_lossy().into_owned();
                if !self.is_rotated(&name) || self.is_compressing(&name) {
                    continue;
                }
                let Ok(m) = e.metadata() else { continue };
                rotated.push(FileInfo {
                    name,
                    size: m.len(),
                    modified: m.modified().ok(),
                    current: false,
                });
            }
        }
        rotated.sort_by_key(|a| std::cmp::Reverse(a.modified));
        let current = match fs::symlink_metadata(&self.path) {
            Ok(m) => FileInfo {
                name: self.base.clone(),
                size: m.len(),
                modified: m.modified().ok(),
                current: true,
            },
            Err(_) => FileInfo {
                name: self.base.clone(),
                size: info.current_size,
                modified: None,
                current: true,
            },
        };
        info.files.push(current);
        info.files.extend(rotated);
        info
    }
}

/// The writer thread's state: the open file and what it knows of it.
struct Writer {
    sink: Arc<Sink>,
    file: Option<fs::File>,
    size: u64,
    /// The start of the period the open file is in, in nanoseconds since the epoch.
    period_start: Option<i128>,
    /// When a size or interval rotation that failed is tried again.
    retry_at: Option<SystemTime>,
}

impl Writer {
    fn now(&self) -> SystemTime {
        (self.sink.common.clock)()
    }

    fn run(mut self, rx: &Receiver<Msg>) {
        let every = self.sink.common.sweep_every;
        let _ = self.retention(false);
        let mut next = after(every);
        loop {
            let wait = next.saturating_duration_since(Instant::now());
            match rx.recv_timeout(wait) {
                Ok(Msg::Buf(b)) => {
                    self.sink.queued.fetch_sub(b.len(), Ordering::Relaxed);
                    self.write_buffer(&b);
                }
                Ok(Msg::Ctl(c, reply)) => {
                    let r = match c {
                        Ctl::Rotate => {
                            let now = self.now();
                            self.rotate(now).map(|()| Vec::new())
                        }
                        Ctl::Cleanup(dry_run) => self.retention(dry_run),
                        Ctl::Sync => Ok(Vec::new()),
                    };
                    let _ = reply.send(r);
                }
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => {
                    // Buffers sent before the stop came before it: what is left are refused.
                    self.close_file();
                    return;
                }
                Err(RecvTimeoutError::Timeout) => {
                    let _ = self.retention(false);
                    next = after(every);
                }
            }
        }
    }

    fn close_file(&mut self) {
        if let Some(f) = self.file.take() {
            if let Err(e) = f.sync_all() {
                self.sink.fail(format!("close: {e}"));
            }
        }
    }

    /// Writes one buffer, rotating the file first when the write would pass its size or a period
    /// ended.
    fn write_buffer(&mut self, b: &[u8]) {
        let now = self.now();
        if self.file.is_some() && self.need_rotate(b.len(), now) {
            let _ = self.rotate(now);
        }
        let Some(f) = self.file.as_mut() else {
            self.sink.dropped.fetch_add(1, Ordering::Relaxed);
            self.sink.fail("write: the file is not open".to_owned());
            return;
        };
        let r = f.write_all(b);
        self.size = self.size.saturating_add(b.len() as u64);
        self.sink.size.store(self.size, Ordering::Relaxed);
        if let Err(e) = r {
            self.sink.dropped.fetch_add(1, Ordering::Relaxed);
            self.sink.fail(format!("write: {e}"));
        }
    }

    /// Whether a write of `n` bytes at `now` rotates the file first.
    fn need_rotate(&mut self, n: usize, now: SystemTime) -> bool {
        let Some(rot) = self.sink.rotation() else {
            return false;
        };
        if self.retry_at.is_some_and(|t| now < t) {
            return false;
        }
        if rot.max_bytes > 0 && self.size > 0 && self.size.saturating_add(n as u64) > rot.max_bytes
        {
            return true;
        }
        if !rot.interval.is_zero() {
            let start = period_start(now, rot.interval, &self.sink.zone());
            match self.period_start {
                None => self.period_start = Some(start),
                Some(s) if s != start => return true,
                Some(_) => {}
            }
        }
        false
    }

    /// Renames the open file out of the way, starts a new one and, when asked, compresses the
    /// old one. A rotation that fails keeps the file it has, says why, and is not tried again by
    /// size or interval for a minute.
    fn rotate(&mut self, now: SystemTime) -> Result<(), String> {
        let Some(rot) = self.sink.rotation() else {
            return Ok(());
        };
        if self.file.is_none() {
            return Ok(());
        }
        if self.size == 0 {
            // Nothing to rotate yet: the period starts again here.
            if !rot.interval.is_zero() {
                self.period_start = Some(period_start(now, rot.interval, &self.sink.zone()));
            }
            return Ok(());
        }
        let retry = |w: &mut Writer, msg: String| {
            w.sink.fail(format!("rotate: {msg}"));
            w.retry_at = now.checked_add(ROTATE_RETRY);
            Err(msg)
        };
        let backup = match self.backup_name(now) {
            Ok(b) => b,
            Err(e) => return retry(self, e),
        };
        // The file as it is held: the new file takes its mode, and the compression reads this
        // file and no other a name may lead to.
        let held = self.file.as_ref().and_then(|f| f.metadata().ok());
        let mode = held
            .as_ref()
            .map_or(FILE_MODE, |m| m.permissions().mode() & 0o777);
        drop(self.file.take());
        let path = PathBuf::from(&self.sink.path);
        if let Err(e) = fs::rename(&path, &backup) {
            // Keep appending to the file that could not be renamed.
            match open_append(&path, mode) {
                Ok((f, m)) => {
                    self.file = Some(f);
                    self.size = m.len();
                }
                Err(e) => self.sink.fail(format!("open: {e}")),
            }
            return retry(self, e.to_string());
        }
        self.size = 0;
        self.sink.size.store(0, Ordering::Relaxed);
        if rot.compress {
            match held {
                Some(h) => compress_later(&self.sink, backup, &h),
                None => self.sink.fail(format!(
                    "compress {}: the file rotated could not be looked at",
                    backup.display()
                )),
            }
        }
        match open_append(&path, mode) {
            Ok((f, m)) => {
                self.file = Some(f);
                self.size = m.len();
                self.sink.size.store(self.size, Ordering::Relaxed);
            }
            Err(e) => {
                let msg = format!("open: {e}");
                self.sink.fail(msg.clone());
                return Err(msg);
            }
        }
        self.retry_at = None;
        if !rot.interval.is_zero() {
            self.period_start = Some(period_start(now, rot.interval, &self.sink.zone()));
        }
        let _ = self.retention(false);
        Ok(())
    }

    /// The name the open file is renamed to: the path, a dot and the time in UTC, with `-1`,
    /// `-2` and so on when the name is taken.
    fn backup_name(&self, now: SystemTime) -> Result<PathBuf, String> {
        let stamp = utc_stamp(now);
        for i in 0..MAX_BACKUP_NAMES {
            let mut name = format!("{}.{stamp}", self.sink.base);
            if i > 0 {
                name.push('-');
                name.push_str(&i.to_string());
            }
            let p = self.sink.dir.join(&name);
            if name_free(&p).map_err(|e| e.to_string())? {
                return Ok(p);
            }
        }
        Err(format!(
            "the {MAX_BACKUP_NAMES} names after {}.{stamp} are taken",
            self.sink.base
        ))
    }

    /// Removes the rotated files past the age, count and total size, oldest first, and returns
    /// them; with `dry_run` only returns them.
    fn retention(&mut self, dry_run: bool) -> Result<Vec<Removed>, String> {
        let Some(rot) = self.sink.rotation() else {
            return Ok(Vec::new());
        };
        if rot.max_age.is_zero() && rot.max_files == 0 && rot.max_total_bytes == 0 {
            return Ok(Vec::new());
        }
        let entries = match fs::read_dir(&self.sink.dir) {
            Ok(e) => e,
            Err(e) => {
                let msg = format!("list {}: {e}", self.sink.dir.display());
                self.sink.fail(msg.clone());
                return Err(msg);
            }
        };
        let mut rotated: Vec<Removed> = Vec::new();
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name == self.sink.base || !self.sink.is_rotated(&name) {
                continue;
            }
            if self.sink.is_compressing(&name) {
                continue;
            }
            let Ok(m) = e.metadata() else { continue };
            rotated.push(Removed {
                name,
                size: m.len(),
                modified: m.modified().unwrap_or(UNIX_EPOCH),
            });
            if rotated.len() > MAX_ROTATED_NAMES {
                break;
            }
        }
        // Newest first.
        rotated.sort_by_key(|a| std::cmp::Reverse(a.modified));
        let now = self.now();
        let mut remove = Vec::new();
        let mut kept = Vec::with_capacity(rotated.len());
        for r in rotated {
            let age = now.duration_since(r.modified).unwrap_or_default();
            if !rot.max_age.is_zero() && age > rot.max_age {
                remove.push(r);
            } else {
                kept.push(r);
            }
        }
        if let Ok(max) = usize::try_from(rot.max_files) {
            if max > 0 && kept.len() > max {
                remove.extend(kept.split_off(max));
            }
        }
        if rot.max_total_bytes > 0 {
            let mut total: u64 = kept.iter().map(|r| r.size).fold(0, u64::saturating_add);
            while total > rot.max_total_bytes {
                let Some(last) = kept.pop() else { break };
                total = total.saturating_sub(last.size);
                remove.push(last);
            }
        }
        if !dry_run {
            for r in &remove {
                if let Err(e) = fs::remove_file(self.sink.dir.join(&r.name)) {
                    if e.kind() != io::ErrorKind::NotFound {
                        self.sink.fail(format!("remove {}: {e}", r.name));
                    }
                }
            }
        }
        Ok(remove)
    }
}

/// The instant `d` from now, or now when that is past what an instant holds.
fn after(d: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(d).unwrap_or(now)
}

/// Whether `s` is what follows a rotated file's name and its dot: the rotation's time in UTC, a
/// number when that name was taken, `.gz` when compressed, as in
/// `2026-11-05T04-00-00.000-1.gz`.
fn rotated_suffix(s: &str) -> bool {
    let s = s.strip_suffix(".gz").unwrap_or(s);
    let b = s.as_bytes();
    let pattern = b"dddd-dd-ddTdd-dd-dd.ddd";
    let (Some(head), Some(tail)) = (b.get(..pattern.len()), b.get(pattern.len()..)) else {
        return false;
    };
    let head_ok = head.iter().zip(pattern).all(|(c, p)| match p {
        b'd' => c.is_ascii_digit(),
        p => c == p,
    });
    let tail_ok = match tail.split_first() {
        None => true,
        Some((b'-', n)) => !n.is_empty() && n.iter().all(u8::is_ascii_digit),
        Some(_) => false,
    };
    head_ok && tail_ok
}

/// `now` in UTC as a rotated file's name has it: `2026-11-05T04-00-00.000`.
fn utc_stamp(now: SystemTime) -> String {
    let t = time::Stamp::of(now);
    let mut b = Vec::with_capacity(23);
    time::rfc3339(&mut b, t, 0);
    // `2026-11-05T04:00:00.000Z`: colons to dashes, the zone dropped.
    b.pop();
    for c in &mut b {
        if *c == b':' {
            *c = b'-';
        }
    }
    String::from_utf8(b).unwrap_or_default()
}

/// Whether neither a rotated file nor what its compression writes is at `name`.
fn name_free(name: &Path) -> io::Result<bool> {
    let s = name.as_os_str().to_string_lossy();
    for n in [s.to_string(), format!("{s}.gz"), format!("{s}.gz.tmp")] {
        match fs::symlink_metadata(&n) {
            Ok(_) => return Ok(false),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

/// Refuses what a log may not be written to or compressed from: not a regular file, or one
/// with another hard link, which a link planted in the directory to a file elsewhere would have.
fn check_file(m: &fs::Metadata) -> io::Result<()> {
    if !m.file_type().is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    if m.nlink() > 1 {
        return Err(io::Error::other(format!(
            "{} hard links, where a log file has one",
            m.nlink()
        )));
    }
    Ok(())
}

/// Opens the regular file at `path` as `opts` says, only when it is the file the name held when
/// looked at without following a link, so that no link at the name leads akit elsewhere.
fn open_existing(path: &Path, opts: &OpenOptions) -> io::Result<Option<(fs::File, fs::Metadata)>> {
    let before = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    check_file(&before).map_err(|e| io::Error::other(format!("open {}: {e}", path.display())))?;
    let f = match opts.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let m = f.metadata()?;
    check_file(&m).map_err(|e| io::Error::other(format!("open {}: {e}", path.display())))?;
    if (m.dev(), m.ino()) != (before.dev(), before.ino()) {
        return Err(io::Error::other(format!(
            "open {}: the name changed while it was opened",
            path.display()
        )));
    }
    Ok(Some((f, m)))
}

/// Opens `path` to be appended to, created with `mode` when missing.
fn open_append(path: &Path, mode: u32) -> io::Result<(fs::File, fs::Metadata)> {
    for _ in 0..3 {
        let mut existing = OpenOptions::new();
        existing
            .append(true)
            .custom_flags(akio_sys::ffi::O_NONBLOCK);
        if let Some(r) = open_existing(path, &existing)? {
            return Ok(r);
        }
        // Created exclusively: a link put at the name meanwhile is not followed.
        match OpenOptions::new()
            .append(true)
            .create_new(true)
            .mode(mode)
            .open(path)
        {
            Ok(f) => {
                let m = f.metadata()?;
                return Ok((f, m));
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other(format!(
        "open {}: the name keeps changing",
        path.display()
    )))
}

/// A rotated file to compress.
struct Job {
    sink: Arc<Sink>,
    name: PathBuf,
    dev: u64,
    ino: u64,
    mode: u32,
    queued: Instant,
}

/// Queues the gzip of `name`, the file rotated as `held` was, on the set's compression thread;
/// a full queue keeps the plain file.
fn compress_later(sink: &Arc<Sink>, name: PathBuf, held: &fs::Metadata) {
    lock(&sink.compressing).insert(name.clone());
    let job = Job {
        sink: Arc::clone(sink),
        name,
        dev: held.dev(),
        ino: held.ino(),
        mode: held.permissions().mode() & 0o777,
        queued: Instant::now(),
    };
    let tx = sink.common.compressor.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Job>(COMPRESS_QUEUE);
        std::thread::Builder::new()
            .name("akit-log-gzip".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    if job.queued.elapsed() > COMPRESS_TIMEOUT {
                        job.sink.fail(format!(
                            "compress {}: it waited too long; the file is kept as it is",
                            job.name.display()
                        ));
                    } else if let Err(e) = compress(&job) {
                        let _ = fs::remove_file(gz_name(&job.name, ".gz.tmp"));
                        job.sink
                            .fail(format!("compress {}: {e}", job.name.display()));
                    }
                    lock(&job.sink.compressing).remove(&job.name);
                }
            })
            .ok()
            .map(|_| tx)
    });
    let refused = match tx {
        Some(tx) => tx.try_send(job).err().map(|e| match e {
            TrySendError::Full(j) | TrySendError::Disconnected(j) => j,
        }),
        None => Some(job),
    };
    if let Some(job) = refused {
        job.sink.fail(format!(
            "compress {}: the compression queue is full; the file is kept as it is",
            job.name.display()
        ));
        lock(&job.sink.compressing).remove(&job.name);
    }
}

fn gz_name(name: &Path, suffix: &str) -> PathBuf {
    let mut s = name.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Writes `job.name`'s gzip to `<name>.gz` and removes the plain file. Reads the name only when
/// it still holds the file rotated, and writes only a file it creates.
fn compress(job: &Job) -> io::Result<()> {
    let mut opts = OpenOptions::new();
    opts.read(true).custom_flags(akio_sys::ffi::O_NONBLOCK);
    let (mut input, m) = open_existing(&job.name, &opts)?
        .ok_or_else(|| io::Error::other("the file rotated is gone"))?;
    if (m.dev(), m.ino()) != (job.dev, job.ino) {
        return Err(io::Error::other(
            "the name no longer holds the file rotated",
        ));
    }
    let tmp = gz_name(&job.name, ".gz.tmp");
    // Exclusive: a file already there, a link included, is not written through.
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    out.set_permissions(fs::Permissions::from_mode(job.mode))?;
    let mut z =
        akio_deflate::Deflater::new(akio_deflate::Format::Gzip, akio_deflate::Level::DEFAULT);
    let mut buf = vec![0u8; CHUNK];
    let mut packed = Vec::with_capacity(CHUNK);
    loop {
        let n = input.read(&mut buf)?;
        let Some(chunk) = buf.get(..n) else { break };
        let flush = if n == 0 {
            akio_deflate::Flush::Finish
        } else {
            akio_deflate::Flush::None
        };
        z.compress(chunk, &mut packed, flush);
        out.write_all(&packed)?;
        packed.clear();
        if n == 0 {
            break;
        }
    }
    out.sync_all()?;
    drop(out);
    fs::rename(&tmp, gz_name(&job.name, ".gz"))?;
    match fs::remove_file(&job.name) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The start, in nanoseconds since the epoch, of the period of length `d` that `now` is in, in
/// `zone`: midnight of its day, plus whole periods from there, so `24h` turns at midnight and
/// `1h` on the hour; a period of whole days groups days from 1970-01-01 by civil date (fit's
/// `periodStart`). `d` is at least a second, and under a day or a whole number of days, as the
/// config's checks ensure.
#[must_use]
#[allow(clippy::arithmetic_side_effects)] // Every value is clamped to MAX_SECS first.
pub fn period_start(now: SystemTime, d: Duration, zone: &Zone) -> i128 {
    const NS: i128 = 1_000_000_000;
    const DAY: i64 = 86_400;
    let t = time::Stamp::of(now);
    let secs = time::clamp(t.secs);
    let now_ns = i128::from(secs) * NS
        + i128::from(
            now.duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos()),
        );
    let wall = secs + i64::from(zone.offset(secs));
    let day = wall.div_euclid(DAY);
    // The instant the wall clock reads 00:00 of `day` in the zone.
    let midnight = |day: i64| -> i128 {
        let w = day * DAY;
        let guess = w - i64::from(zone.offset(w));
        i128::from(w - i64::from(zone.offset(guess))) * NS
    };
    let d_ns = i128::try_from(d.as_nanos()).unwrap_or(i128::MAX).max(NS);
    let day_ns = i128::from(DAY) * NS;
    if d_ns >= day_ns {
        let days = i64::try_from(d_ns / day_ns).unwrap_or(1).max(1);
        return midnight(day.div_euclid(days) * days);
    }
    let m = midnight(day);
    let since = (now_ns - m).max(0);
    m + since / d_ns * d_ns
}

#[cfg(test)]
mod tests;
