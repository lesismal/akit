//! The disk tier, as fit's `middleware/cache/disk.go`: one file a response, and an index of them
//! in memory, so that a request learns whether the disk has its response without touching the
//! disk. Every file operation runs on the disk pool ([`submit`]), a bounded pool of akit's own
//! (akio's [`akio::Pool`]) apart from akio's logical pool, and never on an I/O thread (rules.md
//! rule 1): a request whose response is on disk is suspended while it is read.
//!
//! A file is a head of 12 bytes (the magic `AKTC`, a version, and the length of what follows),
//! the response's metadata (its key, status, times, fields and the request fields it varies on),
//! then its body. Files are written to `tmp` and renamed into place, so a file is whole or not
//! there; what `tmp` holds when the tier opens, a write cut short, is cleared, and the index is
//! read back from the files, on the pool, while requests go on finding less on disk.
//!
//! What waits is bounded: the pool's queue ([`QUEUE`]), the bytes of responses waiting to be
//! written ([`MAX_PENDING_WRITES`]), a file's metadata ([`MAX_META`]), and the index
//! (`max_entries`, `max_bytes`).

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant, SystemTime};

use akio_http::Headers;
use akio_pool::{Pool, PoolConfig};
use akit_config::CacheDisk;

use crate::store::{Entry, MAX_VARIANTS, TagIndex};

/// How long a job may wait for the pool, and a request for a response read from disk, before
/// it goes on without: fit's `diskTimeout`.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
/// The jobs that may wait for the pool.
const QUEUE: usize = 4096;
/// The bytes of responses waiting to be written; past it a response is kept in memory alone.
pub(crate) const MAX_PENDING_WRITES: u64 = 64 << 20;
/// The most a file's metadata may take.
const MAX_META: usize = 1 << 20;

const MAGIC: &[u8; 4] = b"AKTC";
const VERSION: u8 = 1;
const HEAD: usize = 12;

/// The disk pool's timeout, in milliseconds, which tests shorten.
static TIMEOUT_MS: AtomicU64 = AtomicU64::new(5000);

pub(crate) fn timeout() -> Duration {
    Duration::from_millis(TIMEOUT_MS.load(Ordering::Relaxed))
}

pub(crate) fn set_timeout(d: Option<Duration>) {
    let d = d.unwrap_or(DEFAULT_TIMEOUT);
    TIMEOUT_MS.store(
        u64::try_from(d.as_millis()).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
}

/// Tests hold the pool's jobs, as a disk that hangs would.
static HELD: AtomicBool = AtomicBool::new(false);
static HOLD: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

pub(crate) fn hold(on: bool) {
    let (m, cv) = &HOLD;
    *m.lock().unwrap_or_else(PoisonError::into_inner) = on;
    HELD.store(on, Ordering::SeqCst);
    cv.notify_all();
}

fn wait_while_held() {
    if !HELD.load(Ordering::SeqCst) {
        return;
    }
    let (m, cv) = &HOLD;
    let mut held = m.lock().unwrap_or_else(PoisonError::into_inner);
    while *held {
        held = cv.wait(held).unwrap_or_else(PoisonError::into_inner);
    }
}

thread_local! {
    /// Set on the disk pool's threads.
    static ON_POOL: Cell<bool> = const { Cell::new(false) };
}

/// File operations made off the disk pool: none, as tests check.
static OFF_POOL: AtomicU64 = AtomicU64::new(0);

pub(crate) fn off_pool() -> u64 {
    OFF_POOL.load(Ordering::SeqCst)
}

/// Counts a file operation made off the pool.
fn on_pool() {
    if !ON_POOL.with(Cell::get) {
        OFF_POOL.fetch_add(1, Ordering::SeqCst);
    }
}

/// The pool every disk tier reads and writes its files on: up to 4 threads a CPU, at least 16,
/// while the disk keeps them waiting, and [`QUEUE`] jobs waiting.
fn pool() -> &'static Arc<Pool> {
    static POOL: OnceLock<Arc<Pool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let cpus = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
        let mut cfg = PoolConfig::default();
        cfg.max_threads = cpus.saturating_mul(4).max(16);
        cfg.queue_capacity = QUEUE;
        Pool::new(cfg)
    })
}

/// Runs `run` on the disk pool, or `expire` there instead when it waited past [`timeout`].
/// False, both dropped unrun, when the pool's queue is full.
pub(crate) fn submit(
    run: impl FnOnce() + Send + 'static,
    expire: impl FnOnce() + Send + 'static,
) -> bool {
    let queued = Instant::now();
    pool()
        .try_execute(Box::new(move || {
            ON_POOL.with(|p| p.set(true));
            wait_while_held();
            if queued.elapsed() > timeout() {
                expire();
            } else {
                run();
            }
        }))
        .is_ok()
}

/// `path` as the tier names its directory: `.` segments and a trailing `/` gone.
pub(crate) fn clean(path: &str) -> PathBuf {
    Path::new(path)
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect()
}

/// A response on disk: what deciding to serve it takes, in `meta` (its fields and body stay on
/// disk), and its file.
#[derive(Debug)]
pub(crate) struct DiskEntry {
    pub(crate) meta: Arc<Entry>,
    /// Relative to the directory: `ab/ab…`.
    pub(crate) file: String,
    pub(crate) size: u64,
}

/// A key's variants on disk; `removed`, when not 0, the seq of its removal: a write queued
/// before it is not kept.
#[derive(Debug, Default)]
struct Item {
    variants: Vec<Arc<DiskEntry>>,
    removed: u64,
    /// Its place in the order, the least recently used first.
    tick: u64,
}

#[derive(Debug, Default)]
struct State {
    items: HashMap<Arc<str>, Item>,
    order: BTreeMap<u64, Arc<str>>,
    tick: u64,
    size: u64,
    count: u64,
    /// Files whose entries are gone, to be removed on the pool.
    trash: Vec<String>,
    draining: bool,
    /// The keys of the responses on disk by tag.
    tags: TagIndex,
    /// The seq of the last clear: a write queued before it is not kept.
    cleared: u64,
}

impl State {
    /// Marks `key` used, making its item if there is none.
    fn touch(&mut self, key: &str) -> &mut Item {
        self.tick = self.tick.wrapping_add(1);
        let tick = self.tick;
        let k: Arc<str> = match self.items.get_key_value(key) {
            Some((k, it)) => {
                let k = Arc::clone(k);
                self.order.remove(&it.tick);
                k
            }
            None => {
                let k: Arc<str> = Arc::from(key);
                self.items.insert(Arc::clone(&k), Item::default());
                k
            }
        };
        self.order.insert(tick, Arc::clone(&k));
        let it = self.items.entry(k).or_default();
        it.tick = tick;
        it
    }

    /// Drops `de`, `key`'s entry, from the counts and the tags, and trashes its file.
    fn forget(&mut self, key: &str, de: &DiskEntry) {
        self.size = self.size.saturating_sub(de.size);
        self.count = self.count.saturating_sub(1);
        self.tags.remove(key, &de.meta.tags);
        self.trash.push(de.file.clone());
    }

    /// Drops the least recently used keys past the bounds. A key counts as an entry while it is
    /// in the index, as a removed one is.
    fn evict(&mut self, max_bytes: u64, max_entries: u64) {
        while self.size > max_bytes || self.items.len() as u64 > max_entries {
            let Some((_, k)) = self.order.pop_first() else {
                return;
            };
            if let Some(it) = self.items.remove(&k) {
                for de in &it.variants {
                    self.forget(&k, de);
                }
            }
        }
    }
}

/// A zone's disk tier.
#[derive(Debug)]
pub(crate) struct Disk {
    dir: PathBuf,
    max_bytes: AtomicU64,
    max_entries: AtomicU64,
    /// What responses waiting to be written hold.
    pub(crate) pending: AtomicU64,
    /// Orders writes and removals, so that a write queued before its key was removed does not
    /// bring the response back.
    seq: AtomicU64,
    /// The index has been read back from the directory.
    ready: AtomicBool,
    st: Mutex<State>,
    me: Weak<Disk>,
}

impl Disk {
    /// The tier in `dir`, made if missing, its index read back on the pool. Runs when a config
    /// is compiled.
    pub(crate) fn open(dir: PathBuf, cfg: &CacheDisk) -> io::Result<Arc<Disk>> {
        fs::create_dir_all(dir.join("tmp"))?;
        let d = Arc::new_cyclic(|me| Disk {
            dir,
            max_bytes: AtomicU64::new(0),
            max_entries: AtomicU64::new(0),
            pending: AtomicU64::new(0),
            seq: AtomicU64::new(0),
            ready: AtomicBool::new(false),
            st: Mutex::new(State::default()),
            me: me.clone(),
        });
        d.set_limits(cfg);
        let (run, done) = (Arc::clone(&d), Arc::clone(&d));
        // Requests do not wait for it: until it is done they find less on disk.
        if !submit(move || run.rebuild(), move || done.rebuild()) {
            d.ready.store(true, Ordering::Release);
        }
        Ok(d)
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    pub(crate) fn set_limits(&self, cfg: &CacheDisk) {
        self.max_bytes.store(cfg.max_bytes, Ordering::Relaxed);
        self.max_entries.store(cfg.max_entries, Ordering::Relaxed);
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn evict(&self, st: &mut State) {
        st.evict(
            self.max_bytes.load(Ordering::Relaxed),
            self.max_entries.load(Ordering::Relaxed),
        );
    }

    /// How many responses the tier holds, and their size.
    pub(crate) fn stats(&self) -> (u64, u64) {
        let st = self.lock();
        (st.count, st.size)
    }

    /// The entry under `key` that answers a request with fields `h`, if any; marks the key used.
    pub(crate) fn get(&self, key: &str, h: &Headers) -> Option<Arc<DiskEntry>> {
        let mut st = self.lock();
        if !st.items.contains_key(key) {
            return None;
        }
        let it = st.touch(key);
        it.variants
            .iter()
            .rev()
            .find(|de| de.meta.matches(h))
            .cloned()
    }

    /// Queues `e`, `key`'s response, to be written. It finds no room when the pool's queue is
    /// full or too much waits to be written: it is kept in memory alone.
    pub(crate) fn write(&self, key: &str, e: &Arc<Entry>) {
        let Some(me) = self.me.upgrade() else { return };
        let n = e.body.len() as u64;
        if self
            .pending
            .fetch_add(n, Ordering::AcqRel)
            .saturating_add(n)
            > MAX_PENDING_WRITES
        {
            self.pending.fetch_sub(n, Ordering::AcqRel);
            return;
        }
        let seq = self.seq.fetch_add(1, Ordering::AcqRel).saturating_add(1);
        let pending = Pending(Arc::clone(&me), n);
        let (key, e) = (key.to_owned(), Arc::clone(e));
        submit(
            move || {
                let _pending = pending;
                if let Ok(de) = me.write_file(&key, &e, seq) {
                    me.add(&key, de, seq);
                }
                me.drain();
            },
            || {},
        );
    }

    /// Drops `key`'s responses; reports whether it had any. Their files go on the pool, and a
    /// write of `key` queued before now is not kept.
    pub(crate) fn remove(&self, key: &str) -> bool {
        let seq = self.seq.fetch_add(1, Ordering::AcqRel).saturating_add(1);
        let found;
        {
            let mut st = self.lock();
            let it = st.touch(key);
            let gone = std::mem::take(&mut it.variants);
            it.removed = seq;
            found = !gone.is_empty();
            for de in &gone {
                st.forget(key, de);
            }
            self.evict(&mut st);
        }
        self.drain_later();
        found
    }

    /// Drops every response on disk; returns how many keys had one. Their files go on the
    /// pool, and a write queued before now is not kept, nor a file read back after now.
    pub(crate) fn clear(&self) -> usize {
        let seq = self.seq.fetch_add(1, Ordering::AcqRel).saturating_add(1);
        let n;
        {
            let mut st = self.lock();
            let items = std::mem::take(&mut st.items);
            st.order.clear();
            n = items.values().filter(|it| !it.variants.is_empty()).count();
            for (k, it) in &items {
                for de in &it.variants {
                    st.forget(k, de);
                }
            }
            st.cleared = seq;
        }
        self.drain_later();
        n
    }

    /// Adds the keys of the responses on disk tagged `tag` to `out`.
    pub(crate) fn tagged(&self, tag: &str, out: &mut Vec<Arc<str>>) {
        self.lock().tags.keys(tag, out);
    }

    /// Adds the keys with responses on disk that start with any of `prefixes` to `out`, under the
    /// index's lock for as long as reading its keys takes.
    pub(crate) fn keys_with(&self, prefixes: &[String], out: &mut Vec<Arc<str>>) {
        let st = self.lock();
        out.extend(
            st.items
                .iter()
                .filter(|(k, it)| {
                    !it.variants.is_empty() && prefixes.iter().any(|p| k.starts_with(p.as_str()))
                })
                .map(|(k, _)| Arc::clone(k)),
        );
    }

    /// Puts `de` in the index under `key`, in place of the variant it answers for, unless `key`
    /// was removed or the tier cleared after the write was queued, at `seq` (0 for a file read
    /// back, which loses to any removal or clear).
    pub(crate) fn add(&self, key: &str, de: DiskEntry, seq: u64) {
        let mut st = self.lock();
        let cleared = st.cleared;
        let it = st.touch(key);
        if seq < it.removed || (seq < cleared || (seq == 0 && cleared != 0)) {
            // A file read back may be the one a later write put in place: that one stays.
            if !it.variants.iter().any(|v| v.file == de.file) {
                st.trash.push(de.file);
            }
            return;
        }
        let mut gone = Vec::new();
        if let Some(i) = it
            .variants
            .iter()
            .position(|v| v.meta.same_variant(&de.meta))
        {
            if it
                .variants
                .get(i)
                .is_some_and(|v| v.meta.response_time > de.meta.response_time)
            {
                // A newer response is there already: `de`, read back, was its file before it.
                return;
            }
            let v = it.variants.remove(i);
            let same_file = v.file == de.file;
            gone.push((v, same_file));
        }
        if it.variants.len() >= MAX_VARIANTS && !it.variants.is_empty() {
            gone.push((it.variants.remove(0), false));
        }
        let size = de.size;
        let de = Arc::new(de);
        it.variants.push(Arc::clone(&de));
        let k = st
            .items
            .get_key_value(key)
            .map_or_else(|| Arc::from(key), |(k, _)| Arc::clone(k));
        for (v, same_file) in gone {
            st.size = st.size.saturating_sub(v.size);
            st.count = st.count.saturating_sub(1);
            st.tags.remove(key, &v.meta.tags);
            if !same_file {
                st.trash.push(v.file.clone());
            }
        }
        st.tags.add(&k, &de.meta.tags);
        st.size = st.size.saturating_add(size);
        st.count = st.count.saturating_add(1);
        self.evict(&mut st);
    }

    /// Removes `de`, `key`'s entry, which could not be read back.
    pub(crate) fn drop_entry(&self, key: &str, de: &Arc<DiskEntry>) {
        {
            let mut st = self.lock();
            let mut gone = None;
            if let Some(it) = st.items.get_mut(key) {
                if let Some(i) = it.variants.iter().position(|v| Arc::ptr_eq(v, de)) {
                    gone = Some(it.variants.remove(i));
                }
            }
            if let Some(v) = gone {
                st.forget(key, &v);
            }
        }
        self.drain_later();
    }

    /// Has the trash emptied on the pool, unless that is under way; if the pool is full, the
    /// next job empties it.
    fn drain_later(&self) {
        {
            let mut st = self.lock();
            if st.draining || st.trash.is_empty() {
                return;
            }
            st.draining = true;
        }
        let Some(me) = self.me.upgrade() else { return };
        let again = Arc::clone(&me);
        if !submit(move || me.drain(), move || again.drain()) {
            self.lock().draining = false;
        }
    }

    /// Removes the trashed files. Runs on the pool.
    pub(crate) fn drain(&self) {
        loop {
            let files = {
                let mut st = self.lock();
                let files = std::mem::take(&mut st.trash);
                if files.is_empty() {
                    st.draining = false;
                    return;
                }
                files
            };
            for f in files {
                on_pool();
                let _ = fs::remove_file(self.dir.join(f));
            }
        }
    }

    /// Writes `key`'s response `e` to its file, through a file in `tmp` renamed into place.
    /// Runs on the pool.
    pub(crate) fn write_file(&self, key: &str, e: &Entry, seq: u64) -> io::Result<DiskEntry> {
        on_pool();
        let mut head = encode(key, e)?;
        let name = file_name(key, &e.vary);
        let full = self.dir.join(&name);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = self.dir.join("tmp").join(format!(
            "w{seq}-{}",
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos())
        ));
        let written = (|| {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&head)?;
            f.write_all(&e.body)?;
            drop(f);
            fs::rename(&tmp, &full)
        })();
        if let Err(err) = written {
            let _ = fs::remove_file(&tmp);
            return Err(err);
        }
        let size = (head.len() as u64).saturating_add(e.body.len() as u64);
        head.clear();
        Ok(DiskEntry {
            meta: Arc::new(meta_only(e)),
            file: name,
            size,
        })
    }

    /// Reads `de`'s response back, whole. Runs on the pool.
    pub(crate) fn read(&self, de: &DiskEntry) -> io::Result<Entry> {
        on_pool();
        let mut f = fs::File::open(self.dir.join(&de.file))?;
        if f.metadata()?.len() != de.size {
            return Err(broken("a file of another size"));
        }
        let mut data = Vec::with_capacity(usize::try_from(de.size).unwrap_or(0));
        f.read_to_end(&mut data)?;
        let (mut e, key, body_at) =
            decode(&data).ok_or_else(|| broken("not a file of the cache's"))?;
        drop(key);
        e.body = Arc::from(data.get(body_at..).unwrap_or_default());
        Ok(e)
    }

    /// Reads the index back from the directory: every file's metadata, the files that are not
    /// whole or not the cache's removed, and what `tmp` holds, a write cut short, cleared. Runs
    /// on the pool.
    fn rebuild(&self) {
        on_pool();
        if let Ok(rd) = fs::read_dir(self.dir.join("tmp")) {
            for e in rd.flatten() {
                let _ = fs::remove_file(e.path());
            }
        }
        if let Ok(subs) = fs::read_dir(&self.dir) {
            for sub in subs.flatten() {
                let sname = sub.file_name().to_string_lossy().into_owned();
                if sname.len() != 2 || !sub.file_type().is_ok_and(|t| t.is_dir()) {
                    continue;
                }
                let Ok(files) = fs::read_dir(sub.path()) else {
                    continue;
                };
                for f in files.flatten() {
                    let fname = f.file_name().to_string_lossy().into_owned();
                    let name = format!("{sname}/{fname}");
                    let path = f.path();
                    match read_meta(&path) {
                        Some((key, e, size))
                            if fname.starts_with(&sname) && file_name(&key, &e.vary) == name =>
                        {
                            let de = DiskEntry {
                                meta: Arc::new(e),
                                file: name,
                                size,
                            };
                            self.add(&key, de, 0);
                        }
                        _ => {
                            let _ = fs::remove_file(&path);
                        }
                    }
                }
            }
        }
        self.drain();
        self.ready.store(true, Ordering::Release);
    }
}

/// Gives back what a queued write holds in `pending`, however the job ends.
struct Pending(Arc<Disk>, u64);

impl Drop for Pending {
    fn drop(&mut self) {
        self.0.pending.fetch_sub(self.1, Ordering::AcqRel);
    }
}

fn broken(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("cache: {what}"))
}

/// The file of `key`'s variant with `vary`, relative to the directory: `ab/ab…`, from the
/// SHA-256 of both.
pub(crate) fn file_name(key: &str, vary: &[(String, String)]) -> String {
    let mut input = Vec::with_capacity(key.len().saturating_add(64));
    input.extend_from_slice(key.as_bytes());
    for (n, v) in vary {
        input.push(0);
        input.extend_from_slice(n.as_bytes());
        input.push(0);
        input.extend_from_slice(v.as_bytes());
    }
    let sum = akio_crypto::hash::Sha256::digest(&input);
    let mut hex = String::with_capacity(43);
    for b in sum.iter().take(20) {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
    }
    let dir = hex.get(..2).unwrap_or("00").to_owned();
    format!("{dir}/{hex}")
}

/// `e` without its fields and body, for the index; its validators stay.
fn meta_only(e: &Entry) -> Entry {
    Entry {
        status: e.status,
        headers: Headers::new(),
        body: Arc::from(&[][..]),
        vary: e.vary.clone(),
        response_time: e.response_time,
        initial_age: e.initial_age,
        lifetime: e.lifetime,
        no_stale: e.no_stale,
        swr: e.swr,
        sie: e.sie,
        etag: e.etag.clone(),
        last_modified: e.last_modified.clone(),
        tags: e.tags.clone(),
        size: 0,
    }
}

fn nanos(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&u32::try_from(b.len()).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(b);
}

/// The head and metadata of `key`'s response `e`; its body follows them in the file.
fn encode(key: &str, e: &Entry) -> io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(256);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&[VERSION, 0, 0, 0, 0, 0, 0, 0]);
    put_bytes(&mut out, key.as_bytes());
    out.extend_from_slice(&e.status.to_be_bytes());
    let since = e
        .response_time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    for d in [since, e.initial_age, e.lifetime, e.swr, e.sie] {
        out.extend_from_slice(&nanos(d).to_be_bytes());
    }
    out.push(u8::from(e.no_stale));
    out.extend_from_slice(&u32::try_from(e.vary.len()).unwrap_or(0).to_be_bytes());
    for (n, v) in &e.vary {
        put_bytes(&mut out, n.as_bytes());
        put_bytes(&mut out, v.as_bytes());
    }
    out.extend_from_slice(&u32::try_from(e.headers.len()).unwrap_or(0).to_be_bytes());
    for (n, v) in e.headers.iter() {
        put_bytes(&mut out, n.as_bytes());
        put_bytes(&mut out, v.as_bytes());
    }
    out.extend_from_slice(&(e.body.len() as u64).to_be_bytes());
    let meta = out.len().saturating_sub(HEAD);
    if meta > MAX_META {
        return Err(broken(
            "the response's fields are too large to keep on disk",
        ));
    }
    let n = u32::try_from(meta).unwrap_or(u32::MAX).to_be_bytes();
    if let Some(slot) = out.get_mut(8..HEAD) {
        slot.copy_from_slice(&n);
    }
    Ok(out)
}

/// Reads what [`encode`] wrote.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (a, b) = (self.0.get(..n)?, self.0.get(n..)?);
        self.0 = b;
        Some(a)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1)?.first().copied()
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_be_bytes(self.take(8)?.try_into().ok()?))
    }

    fn str(&mut self) -> Option<&'a str> {
        let n = usize::try_from(self.u32()?).ok()?;
        std::str::from_utf8(self.take(n)?).ok()
    }
}

/// The length of the metadata a file's `head` announces, if it is a head of the cache's.
fn head_len(head: &[u8]) -> Option<usize> {
    if head.get(..4)? != MAGIC || *head.get(4)? != VERSION {
        return None;
    }
    let n = usize::try_from(u32::from_be_bytes(head.get(8..HEAD)?.try_into().ok()?)).ok()?;
    (n <= MAX_META).then_some(n)
}

/// The entry `meta` (a file's metadata, without its head) records, without its body, its key,
/// and the body's length.
fn decode_meta(meta: &[u8]) -> Option<(Entry, String, u64)> {
    let mut r = Reader(meta);
    let key = r.str()?.to_owned();
    let status = r.u16()?;
    let since = Duration::from_nanos(r.u64()?);
    let [initial_age, lifetime, swr, sie] =
        [r.u64()?, r.u64()?, r.u64()?, r.u64()?].map(Duration::from_nanos);
    let no_stale = r.u8()? != 0;
    let nv = r.u32()?;
    let mut vary = Vec::new();
    for _ in 0..nv {
        let (n, v) = (r.str()?, r.str()?);
        vary.push((n.to_owned(), v.to_owned()));
    }
    let nh = r.u32()?;
    let mut headers = Headers::new();
    for _ in 0..nh {
        let (n, v) = (r.str()?, r.str()?);
        headers.append(n, v);
    }
    let body_len = r.u64()?;
    if !r.0.is_empty() {
        return None;
    }
    let etag = headers.get("etag").map(str::to_owned);
    let last_modified = headers.get("last-modified").map(str::to_owned);
    let tags = crate::semantics::tags_of(&headers);
    let e = Entry {
        status,
        headers,
        body: Arc::from(&[][..]),
        vary,
        response_time: SystemTime::UNIX_EPOCH.checked_add(since)?,
        initial_age,
        lifetime,
        no_stale,
        swr,
        sie,
        etag,
        last_modified,
        tags,
        size: 0,
    };
    Some((e, key, body_len))
}

/// A whole file: its entry (no body), its key, and where its body starts.
fn decode(data: &[u8]) -> Option<(Entry, String, usize)> {
    let n = head_len(data)?;
    let end = HEAD.checked_add(n)?;
    let (e, key, body_len) = decode_meta(data.get(HEAD..end)?)?;
    (data.len().checked_sub(end)? as u64 == body_len).then_some((e, key, end))
}

/// A file's key, its entry without fields or body, for the index, and its size; `None` for one
/// that is not a whole file of the cache's.
fn read_meta(path: &Path) -> Option<(String, Entry, u64)> {
    let mut f = fs::File::open(path).ok()?;
    let size = f.metadata().ok()?.len();
    let mut head = [0u8; HEAD];
    f.read_exact(&mut head).ok()?;
    let n = head_len(&head)?;
    let mut meta = vec![0u8; n];
    f.read_exact(&mut meta).ok()?;
    let (e, key, body_len) = decode_meta(&meta)?;
    let whole = (HEAD as u64)
        .checked_add(n as u64)
        .and_then(|m| m.checked_add(body_len))
        == Some(size);
    whole.then(|| (key, meta_only(&e), size))
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        clippy::cast_possible_truncation
    )]

    use super::*;

    fn entry(body: &[u8], at: SystemTime, vary: &str) -> Entry {
        let mut headers = Headers::new();
        headers.append("Content-Type", "text/plain");
        headers.append("ETag", "\"e\"");
        Entry {
            status: 200,
            headers,
            body: Arc::from(body),
            vary: if vary.is_empty() {
                Vec::new()
            } else {
                vec![("a".into(), vary.into())]
            },
            response_time: at,
            initial_age: Duration::from_secs(3),
            lifetime: Duration::from_secs(60),
            no_stale: true,
            swr: Duration::from_secs(7),
            sie: Duration::from_secs(9),
            etag: Some("\"e\"".into()),
            last_modified: None,
            tags: Box::default(),
            size: 0,
        }
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "akit-cache-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn cfg(max_bytes: u64, max_entries: u64) -> CacheDisk {
        CacheDisk {
            path: String::new(),
            max_bytes,
            max_entries,
        }
    }

    fn wait(what: &str, f: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !f() {
            assert!(Instant::now() < deadline, "no {what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Runs `f` on the pool and waits for it.
    fn on<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        assert!(submit(move || drop(tx.send(f())), || {}));
        rx.recv_timeout(Duration::from_secs(5)).unwrap()
    }

    // fit's TestParseFile: what a file records comes back, and a file cut short or not the
    // cache's is refused.
    #[test]
    fn files() {
        let e = entry(b"body", SystemTime::now(), "x");
        let mut data = encode("http://k/", &e).unwrap();
        data.extend_from_slice(&e.body);
        let (back, key, at) = decode(&data).unwrap();
        assert_eq!(key, "http://k/");
        assert_eq!(&data[at..], b"body");
        assert_eq!(
            (
                back.status,
                back.vary.clone(),
                back.lifetime,
                back.swr,
                back.sie,
                back.no_stale
            ),
            (200, e.vary.clone(), e.lifetime, e.swr, e.sie, true)
        );
        assert_eq!(back.response_time, e.response_time);
        assert_eq!(back.etag.as_deref(), Some("\"e\""));
        assert_eq!(back.headers.get("content-type"), Some("text/plain"));
        assert!(decode(&data[..data.len() - 1]).is_none(), "cut short");
        let mut longer = data.clone();
        longer.push(0);
        assert!(decode(&longer).is_none(), "too long");
        let mut other = data.clone();
        other[0] = b'F';
        assert!(decode(&other).is_none(), "not the cache's");
        assert!(decode(&data[..5]).is_none());
        let mut big = data.clone();
        big[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(decode(&big).is_none(), "metadata too large");
        assert_ne!(file_name("k", &[]), file_name("k", &e.vary));
        assert_eq!(file_name("k", &[]).len(), 43);
    }

    // fit's TestDiskAdd: an older response does not replace a newer one, a variant's new file
    // trashes its old one, and past the variants' bound the oldest goes.
    #[test]
    fn add() {
        let dir = temp("add");
        let d = Disk::open(dir.clone(), &cfg(1 << 30, 1000)).unwrap();
        wait("the index", || d.is_ready());
        let now = SystemTime::now();
        let de = |file: &str, at: SystemTime, vary: &str| DiskEntry {
            meta: Arc::new(entry(b"", at, vary)),
            file: file.into(),
            size: 10,
        };
        let none = Headers::new();
        d.add("k", de("new", now, ""), 0);
        d.add("k", de("old", now - Duration::from_secs(1), ""), 0);
        assert_eq!(d.get("k", &none).unwrap().file, "new");
        d.add("k", de("newer", now + Duration::from_secs(1), ""), 0);
        assert_eq!(d.lock().trash, vec!["new".to_owned()]);
        d.lock().trash.clear();
        for i in 0..=MAX_VARIANTS {
            let v = char::from(b'a' + i as u8).to_string();
            d.add("v", de(&format!("v{v}"), now, &v), 0);
        }
        assert_eq!(d.stats().0, 1 + MAX_VARIANTS as u64);
        let mut h = Headers::new();
        h.append("A", "a");
        assert!(d.get("v", &h).is_none(), "the oldest variant kept");
        // A write queued before a removal is dropped when it lands.
        let seq = d.seq.fetch_add(1, Ordering::AcqRel) + 1;
        d.remove("r");
        d.add("r", de("late", now, ""), seq);
        assert!(d.get("r", &none).is_none());
        d.lock().trash.clear();
        let _ = fs::remove_dir_all(&dir);
    }

    // fit's TestDiskBounds: max_entries and max_bytes, the least recently used going, and
    // their files with them.
    #[test]
    fn bounds() {
        let dir = temp("bounds");
        let d = Disk::open(dir.clone(), &cfg(1 << 20, 10)).unwrap();
        wait("the index", || d.is_ready());
        let body = vec![7u8; 10 << 10];
        for i in 0..30 {
            d.write(
                &format!("/k{i}"),
                &Arc::new(entry(&body, SystemTime::now(), "")),
            );
            wait("a write", || d.pending.load(Ordering::SeqCst) == 0);
        }
        let dd = Arc::clone(&d);
        on(move || dd.drain());
        let files = || {
            let mut n = 0;
            for sub in fs::read_dir(&dir).unwrap().flatten() {
                if sub.file_name() != "tmp" {
                    n += fs::read_dir(sub.path()).unwrap().count();
                }
            }
            n
        };
        let none = Headers::new();
        assert_eq!((d.stats().0, files()), (10, 10));
        assert!(d.get("/k29", &none).is_some() && d.get("/k0", &none).is_none());
        d.set_limits(&cfg(50 << 10, 100));
        d.write("/big", &Arc::new(entry(&body, SystemTime::now(), "")));
        wait("a write", || d.pending.load(Ordering::SeqCst) == 0);
        let dd = Arc::clone(&d);
        on(move || dd.drain());
        let (n, size) = d.stats();
        assert!(
            size <= 50 << 10 && n == 4 && files() == 4,
            "{n} {size} {}",
            files()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    // fit's TestDiskFiles and TestDiskBrokenFiles, in part: the index read back keeps the whole
    // files of the cache's, and removes the rest and what tmp holds.
    #[test]
    fn rebuild() {
        let dir = temp("rebuild");
        let d = Disk::open(dir.clone(), &cfg(1 << 30, 1000)).unwrap();
        wait("the index", || d.is_ready());
        let e = Arc::new(entry(b"hello", SystemTime::now(), ""));
        d.write("http://a/", &e);
        d.write("http://b/", &e);
        wait("the writes", || d.stats().0 == 2);
        let a = d.get("http://a/", &Headers::new()).unwrap();
        let dd = Arc::clone(&d);
        let aa = Arc::clone(&a);
        let back = on(move || dd.read(&aa)).unwrap();
        assert_eq!(&*back.body, b"hello");
        assert_eq!(back.headers.get("etag"), Some("\"e\""));
        drop(d);
        // One cut short; junk beside it, and in tmp.
        let b = file_name("http://b/", &[]);
        let data = fs::read(dir.join(&b)).unwrap();
        fs::write(dir.join(&b), &data[..data.len() - 3]).unwrap();
        let junk = dir.join(&a.file[..2]).join(format!("{}junk", &a.file[..2]));
        fs::write(&junk, b"not a cache file").unwrap();
        fs::write(dir.join("tmp/w1"), b"half").unwrap();
        let d = Disk::open(dir.clone(), &cfg(1 << 30, 1000)).unwrap();
        wait("the index", || d.is_ready());
        assert_eq!(d.stats().0, 1);
        assert!(d.get("http://a/", &Headers::new()).is_some());
        for f in [dir.join(&b), junk, dir.join("tmp/w1")] {
            assert!(!f.exists(), "{} was kept", f.display());
        }
        // A file gone from under its entry, or of another size, is not read.
        let a = d.get("http://a/", &Headers::new()).unwrap();
        fs::write(dir.join(&a.file), b"x").unwrap();
        let (dd, aa) = (Arc::clone(&d), Arc::clone(&a));
        assert!(on(move || dd.read(&aa)).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
