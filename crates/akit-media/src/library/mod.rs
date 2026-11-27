//! fit's media library (I141, fit's `medialib`): the named libraries of the config's
//! `media_library`, each a set of directories scanned into a catalog of items, a title each with
//! its renditions, audio, subtitles and poster, for the handlers that serve them (`vod`, I142)
//! and the channels that play them (I145).
//!
//! Nothing here blocks an akio I/O thread. A library scans on a thread of its own, one scan at a
//! time, when it is made, every `scan_interval`, when a reload changes what it scans, and when
//! asked ([`Library::rescan`]); its probes run on at most `max_probes` threads of the scan's own.
//! What serving reads (a parsed movie, a subtitle, a file's bytes) runs on the library's bounded
//! file pool ([`Pool`]), and calls back from there. The catalog, the movies kept and the work
//! queued are each bounded.
//!
//! Libraries are found by name per engine, as `media_auth`'s policies are: a [`Library`] stays
//! the same across reloads and changes in place, so that whoever holds one sees what the config
//! now says. The role `media_library` ([`MEDIA_LIBRARY`]) moves them with every config applied.

pub mod admin;
pub mod catalog;
pub mod files;
pub mod pool;
mod scan;
mod tables;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed, Ordering::SeqCst};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use akio_media::mp4::Limits;
use akit_config::Config;
use akit_config::medialib::{MediaLibrary, path_match};
use akit_roles::host::{Host, Info, MetricsFn, Role};

pub use catalog::{Catalog, File, Item, Problem, Rendition, Subtitle, Track};
pub use files::{Handle, to_webvtt};
pub use pool::{Pool, PoolStats};
pub use scan::MAX_DEPTH;
pub use tables::{MAX_WAITERS, MovieResult, TableStats};

use crate::swap::Swap;

/// The errors of a library's methods (fit's `medialib` errors).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The library was removed by a reload, or its role stopped.
    Removed,
    /// The library's file pool refused the work, its queue full, or gave it up past
    /// `file_timeout`.
    Busy,
    /// The file is not as the catalog has it: changed, replaced or removed since it was
    /// scanned. The next scan lists it as it is.
    Changed,
    /// The file leads out of its root, as a symbolic link does, without `follow_symlinks`.
    Outside,
    /// The rendition or subtitle is listed with an error of its own.
    NotServed,
    /// Past the library's bounds: a movie larger than all of `max_table_bytes`.
    TooLarge,
    /// The file does not parse as what the catalog says it is.
    Malformed(String),
    /// The file could not be read.
    Io(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Removed => f.write_str("the library was removed"),
            Error::Busy => f.write_str("the library's file pool is busy"),
            Error::Changed => f.write_str("the file changed since it was scanned"),
            Error::Outside => f.write_str("the file leads out of its root"),
            Error::NotServed => f.write_str("the file is listed with an error and not served"),
            Error::TooLarge => f.write_str("past the library's bounds"),
            Error::Malformed(e) => write!(f, "the file is not served: {e}"),
            Error::Io(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for Error {}

/// A root of a library: as the config writes it, and absolute.
#[derive(Debug, Clone)]
pub(crate) struct Root {
    pub(crate) conf: String,
    pub(crate) abs: PathBuf,
}

/// A `files` or `exclude` pattern: one without '/' matches a name, one with '/' a path under the
/// root, `**` matching any run of directories.
#[derive(Debug, Clone)]
pub(crate) struct Pattern {
    src: String,
    parts: Option<Vec<String>>,
}

impl Pattern {
    fn new(p: &str) -> Pattern {
        Pattern {
            src: p.to_owned(),
            parts: p
                .contains('/')
                .then(|| p.split('/').map(str::to_owned).collect()),
        }
    }

    /// Whether the pattern matches `rel`, a slash path under a root.
    pub(crate) fn matches(&self, rel: &str) -> bool {
        match &self.parts {
            None => {
                let base = rel.rsplit('/').next().unwrap_or(rel);
                path_match(&self.src, base).unwrap_or(false)
            }
            Some(parts) => {
                let names: Vec<&str> = rel.split('/').collect();
                match_parts(parts, &names)
            }
        }
    }
}

fn match_parts(pat: &[String], name: &[&str]) -> bool {
    let Some((first, rest)) = pat.split_first() else {
        return name.is_empty();
    };
    if first == "**" {
        // ** takes none of the names, or one more.
        return (0..=name.len()).any(|i| name.get(i..).is_some_and(|n| match_parts(rest, n)));
    }
    match name.split_first() {
        Some((n, more)) => path_match(first, n).unwrap_or(false) && match_parts(rest, more),
        None => false,
    }
}

/// A library's config, compiled: immutable, and replaced whole by a reload.
#[derive(Debug, Clone)]
pub(crate) struct Settings {
    pub(crate) roots: Vec<Root>,
    pub(crate) include: Vec<Pattern>,
    pub(crate) exclude: Vec<Pattern>,
    pub(crate) follow: bool,
    pub(crate) interval: Duration,
    pub(crate) max_items: usize,
    pub(crate) max_files: usize,
    pub(crate) catalog_bytes: u64,
    pub(crate) probes: usize,
    pub(crate) sidecar_bytes: u64,
    pub(crate) subtitle_bytes: u64,
    pub(crate) table_bytes: u64,
    pub(crate) workers: usize,
    pub(crate) queue: usize,
    pub(crate) timeout: Duration,
}

impl Settings {
    fn compile(c: &MediaLibrary) -> Result<Settings, String> {
        if c.roots.is_empty() {
            return Err("no roots".to_owned());
        }
        let mut s = Settings {
            roots: Vec::new(),
            include: Vec::new(),
            exclude: Vec::new(),
            follow: c.follow_symlinks,
            interval: c.interval(),
            max_items: c.items(),
            max_files: c.files_bound(),
            catalog_bytes: c.catalog_bytes(),
            probes: c.probes(),
            sidecar_bytes: c.sidecar_bytes(),
            subtitle_bytes: c.subtitle_bytes(),
            table_bytes: c.table_bytes(),
            workers: c.workers(),
            queue: c.queue(),
            timeout: c.timeout(),
        };
        for r in &c.roots {
            if r.trim().is_empty() {
                return Err(format!("root {r:?}: not a path"));
            }
            s.roots.push(Root {
                conf: r.clone(),
                abs: akit_config::medialib::absolute(r),
            });
        }
        for (list, to) in [(&c.files, &mut s.include), (&c.exclude, &mut s.exclude)] {
            for p in list {
                if !akit_config::medialib::valid_path_pattern(p) {
                    return Err(format!("{p:?} is not a pattern"));
                }
                to.push(Pattern::new(p));
            }
        }
        Ok(s)
    }

    /// What of the settings a scan depends on: a reload that changes it scans again at once.
    fn scan_key(&self) -> String {
        let mut b = String::new();
        for r in &self.roots {
            b.push_str(&r.abs.to_string_lossy());
            b.push('\0');
        }
        for l in [&self.include, &self.exclude] {
            b.push('\u{1}');
            for p in l {
                b.push_str(&p.src);
                b.push('\0');
            }
        }
        b.push_str(&format!(
            "{} {} {} {} {} {}",
            self.follow,
            self.max_items,
            self.max_files,
            self.catalog_bytes,
            self.sidecar_bytes,
            self.subtitle_bytes
        ));
        b
    }

    /// Whether the file at `rel` is looked at.
    pub(crate) fn included(&self, rel: &str) -> bool {
        (self.include.is_empty() || self.include.iter().any(|p| p.matches(rel)))
            && !self.exclude.iter().any(|p| p.matches(rel))
    }
}

/// What a library's scans are doing, and have done.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// A scan runs.
    pub scanning: bool,
    /// Another is asked for after it.
    pub pending: bool,
    /// The scans ended.
    pub scans: u64,
    /// The last scan's start.
    pub started: Option<SystemTime>,
    /// Its end; `None` while it runs.
    pub ended: Option<SystemTime>,
    /// The files and directories the scan has looked at.
    pub files: usize,
    /// The probes of its media files it needs, a file probed again once when it changed while
    /// probed.
    pub probe: usize,
    /// Those done.
    pub probed: usize,
    /// The files unchanged since the last scan, not probed again.
    pub reused: usize,
}

/// What a library has counted.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    pub(crate) scans: AtomicU64,
    pub(crate) probed: AtomicU64,
    pub(crate) probe_errors: AtomicU64,
}

/// What wakes a library's scanner.
#[derive(Debug, Default)]
struct Wake {
    /// A scan asked for; one waits at most.
    kick: bool,
    /// `scan_interval` changed.
    retime: bool,
    stop: bool,
}

/// A hook a test runs before each probe.
pub(crate) type Hook = Arc<dyn Fn() + Send + Sync>;

/// A named media library: its roots, scanned into a [`Catalog`], and the file pool its files
/// are read on. Its methods never block.
pub struct Library {
    name: String,
    set: Swap<Settings>,
    cat: Swap<Catalog>,
    pool: Mutex<Pool>,
    tables: tables::Tables,
    removed: AtomicBool,
    wake: Mutex<Wake>,
    woken: Condvar,
    pub(crate) status: Mutex<Status>,
    /// Told whenever the status changes.
    changed: Condvar,
    /// The library's own changes of its files, and the items they took out of the catalog
    /// (I149); kept here so that a scan begun before a change does not list what it removed.
    edits: Mutex<(u64, HashMap<String, u64>)>,
    pub(crate) counters: Counters,
    hook: Mutex<Option<Hook>>,
    scanner: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Library {
    fn new(name: &str, s: Settings) -> Arc<Library> {
        let pool = Pool::new(s.workers, s.queue, s.timeout, &pool_name(name));
        let l = Arc::new(Library {
            name: name.to_owned(),
            set: Swap::default(),
            cat: Swap::default(),
            pool: Mutex::new(pool),
            tables: tables::Tables::new(s.table_bytes),
            removed: AtomicBool::new(false),
            wake: Mutex::new(Wake::default()),
            woken: Condvar::new(),
            status: Mutex::new(Status::default()),
            changed: Condvar::new(),
            edits: Mutex::new((0, HashMap::new())),
            counters: Counters::default(),
            hook: Mutex::new(None),
            scanner: Mutex::new(None),
        });
        l.set.set(Some(Arc::new(s)));
        l.cat.set(Some(Arc::new(Catalog::default())));
        let me = Arc::clone(&l);
        let spawned = std::thread::Builder::new()
            .name(format!("akit-medialib-{}", short(name)))
            .spawn(move || me.run());
        if let Ok(h) = spawned {
            *lock(&l.scanner) = Some(h);
        }
        l.rescan();
        l
    }

    /// The library's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether a reload removed the library.
    #[must_use]
    pub fn removed(&self) -> bool {
        self.removed.load(SeqCst)
    }

    /// The catalog of the library's last scan, empty before the first. It changes with each
    /// scan; what it returns does not.
    #[must_use]
    pub fn catalog(&self) -> Arc<Catalog> {
        self.cat.get().unwrap_or_default()
    }

    /// Runs `f` with the catalog as this thread last saw it, refreshed when a scan replaced it:
    /// for the I/O threads, which take no lock to read it.
    pub fn with_catalog<R>(&self, f: impl FnOnce(&Catalog) -> R) -> R {
        self.cat.with(|c| match c {
            Some(c) => f(c),
            None => f(&Catalog::default()),
        })
    }

    /// The item of the catalog whose ID is `id`.
    #[must_use]
    pub fn item(&self, id: &str) -> Option<Arc<Item>> {
        self.with_catalog(|c| c.item(id).cloned())
    }

    /// The library's file pool. It is replaced when a reload changes its bounds; the one
    /// replaced runs what it has queued.
    #[must_use]
    pub fn pool(&self) -> Pool {
        lock(&self.pool).clone()
    }

    /// What the library's movies take.
    #[must_use]
    pub fn tables(&self) -> TableStats {
        self.tables.stats()
    }

    /// What the library's scans are doing.
    #[must_use]
    pub fn status(&self) -> Status {
        let mut st = lock(&self.status).clone();
        st.pending = lock(&self.wake).kick;
        st
    }

    /// The library's roots, as the config writes them.
    #[must_use]
    pub fn roots(&self) -> Vec<String> {
        self.set
            .get()
            .map(|s| s.roots.iter().map(|r| r.conf.clone()).collect())
            .unwrap_or_default()
    }

    /// Asks for a scan of the library, and returns at once: false when one is already waiting
    /// to run, which will see what is there by then, or the library is removed.
    pub fn rescan(&self) -> bool {
        if self.removed() {
            return false;
        }
        let mut w = lock(&self.wake);
        if w.kick {
            return false;
        }
        w.kick = true;
        drop(w);
        self.woken.notify_all();
        self.tell();
        true
    }

    /// Tells those waiting for the status that it changed.
    pub(crate) fn tell(&self) {
        let _g = lock(&self.status);
        self.changed.notify_all();
    }

    /// Waits until `f` holds of the library's status, `limit` at most: the status then, or
    /// `None` past the limit.
    pub fn wait_status(&self, limit: Duration, f: impl Fn(&Status) -> bool) -> Option<Status> {
        let deadline = std::time::Instant::now().checked_add(limit)?;
        let mut g = lock(&self.status);
        loop {
            let mut st = g.clone();
            st.pending = lock(&self.wake).kick;
            if f(&st) {
                return Some(st);
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return None;
            }
            g = self
                .changed
                .wait_timeout(g, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn settings(&self) -> Arc<Settings> {
        self.set.get().unwrap_or_else(|| {
            Arc::new(Settings {
                roots: Vec::new(),
                include: Vec::new(),
                exclude: Vec::new(),
                follow: false,
                interval: akit_config::medialib::DEFAULT_SCAN_INTERVAL,
                max_items: 0,
                max_files: 0,
                catalog_bytes: 0,
                probes: 1,
                sidecar_bytes: 0,
                subtitle_bytes: 0,
                table_bytes: 0,
                workers: 1,
                queue: 1,
                timeout: Duration::ZERO,
            })
        })
    }

    /// The scanner: it scans when asked and every `scan_interval`, one scan at a time, until
    /// the library is removed.
    fn run(&self) {
        loop {
            let interval = self.settings().interval;
            let mut w = lock(&self.wake);
            let mut timed_out = false;
            if !w.kick && !w.stop && !w.retime {
                let (g, t) = self
                    .woken
                    .wait_timeout(w, interval)
                    .unwrap_or_else(PoisonError::into_inner);
                w = g;
                timed_out = t.timed_out();
            }
            if w.stop {
                return;
            }
            if w.retime && !w.kick {
                w.retime = false;
                continue;
            }
            if !w.kick && !timed_out {
                // Woken for nothing: wait again.
                continue;
            }
            w.kick = false;
            w.retime = false;
            drop(w);
            self.scan();
        }
    }

    /// Whether the library was removed, for a scan to stop.
    pub(crate) fn stopped(&self) -> bool {
        lock(&self.wake).stop
    }

    pub(crate) fn probe_hook(&self) -> Option<Hook> {
        lock(&self.hook).clone()
    }

    /// Scans the library into a new catalog, which replaces the one before once it is whole.
    fn scan(&self) {
        let s = self.settings();
        let started = SystemTime::now();
        {
            let mut st = lock(&self.status);
            *st = Status {
                scanning: true,
                scans: st.scans,
                started: Some(started),
                ..Status::default()
            };
            self.changed.notify_all();
        }
        let edits = lock(&self.edits).0;
        let prev = self.catalog();
        let cat = scan::Scanner::new(self, &s).run(&prev);
        let ended = SystemTime::now();
        {
            let mut st = lock(&self.status);
            st.scanning = false;
            st.ended = Some(ended);
            if cat.is_none() {
                self.changed.notify_all();
            }
        }
        let Some(mut cat) = cat else { return };
        cat.scanned = Some(ended);
        {
            // What the library's own changes took out while the scan ran stays out.
            let mut e = lock(&self.edits);
            let gone: Vec<String> =
                e.1.iter()
                    .filter(|(_, at)| **at > edits)
                    .map(|(id, _)| id.clone())
                    .collect();
            e.1.retain(|_, at| *at > edits);
            if !gone.is_empty() {
                cat = cat.without(&gone);
            }
            self.cat.set(Some(Arc::new(cat)));
        }
        let cat = self.catalog();
        let keep: HashSet<tables::Key> = cat
            .items
            .iter()
            .flat_map(|it| it.renditions.iter().chain(&it.audio))
            .map(key_of)
            .collect();
        self.tables.keep(&keep);
        self.counters.scans.fetch_add(1, Relaxed);
        {
            // The scan is counted once its catalog is in place.
            let mut st = lock(&self.status);
            st.scans = st.scans.saturating_add(1);
            self.changed.notify_all();
        }
        if !cat.truncated.is_empty() {
            akit_observe::warn!(
                Server,
                "media_library {}: the catalog lacks items: {}",
                self.name,
                cat.truncated
            );
        }
    }

    /// Moves the library to `s`, compiled from a config applied.
    fn apply(&self, s: Settings) {
        let old = self.settings();
        if old.workers != s.workers || old.queue != s.queue || old.timeout != s.timeout {
            let p = Pool::new(s.workers, s.queue, s.timeout, &pool_name(&self.name));
            let was = std::mem::replace(&mut *lock(&self.pool), p);
            was.close();
        }
        self.tables.resize(s.table_bytes);
        let retime = old.interval != s.interval;
        let rescan = old.scan_key() != s.scan_key();
        self.set.set(Some(Arc::new(s)));
        if retime {
            lock(&self.wake).retime = true;
            self.woken.notify_all();
        }
        if rescan {
            self.rescan();
        }
    }

    /// Ends the library: its scanner stops at its next step, its pool once it has run what it
    /// has queued, and its methods fail with [`Error::Removed`].
    fn remove(&self) {
        if self.removed.swap(true, SeqCst) {
            return;
        }
        lock(&self.wake).stop = true;
        self.woken.notify_all();
        self.tell();
        self.pool().close();
        self.tables.clear();
    }

    /// Waits for the scanner of a removed library to end, and its pool's threads, `limit` at
    /// most for the pool: for tests.
    #[doc(hidden)]
    pub fn join(&self, limit: Duration) -> bool {
        let h = lock(&self.scanner).take();
        let ok = h.is_none_or(|h| h.join().is_ok());
        ok && self.pool().wait(limit)
    }

    /// Runs `run` on the library's pool, or calls `fail` with [`Error::Removed`] once the
    /// library is removed, or [`Error::Busy`] when the pool does not take it or gives it up.
    fn on_pool(&self, run: pool::Run, fail: impl FnOnce(Error) + Send + Clone + 'static) {
        if self.removed() {
            return fail(Error::Removed);
        }
        let expired = fail.clone();
        if !self
            .pool()
            .submit(run, Some(Box::new(move || expired(Error::Busy))))
        {
            fail(Error::Busy);
        }
    }

    /// Opens `f`, a file of the catalog, at once: for work already on the library's pool. A
    /// file not as the catalog has it is [`Error::Changed`], and its movies are dropped.
    ///
    /// # Errors
    ///
    /// [`Error::Removed`], [`Error::Changed`], [`Error::Outside`], or the open's.
    pub fn open_file(&self, f: &File) -> Result<Handle, Error> {
        if self.removed() {
            return Err(Error::Removed);
        }
        let r = files::open_file(f, self.settings().follow);
        if r.as_ref().is_err_and(|e| *e == Error::Changed) {
            self.tables.drop_file(&f.abs);
        }
        r
    }

    /// Opens `f` on the library's pool and calls `cb` there with it, or with the error; `cb` is
    /// called once, at once on the caller's thread when the pool does not take the job.
    pub fn open(
        self: &Arc<Self>,
        f: &File,
        cb: impl FnOnce(Result<Handle, Error>) + Send + 'static,
    ) {
        let me = Arc::clone(self);
        let f = f.clone();
        let cb = Arc::new(Mutex::new(Some(cb)));
        let fail_cb = Arc::clone(&cb);
        self.on_pool(
            Box::new(move || {
                if let Some(cb) = lock(&cb).take() {
                    cb(me.open_file(&f));
                }
            }),
            move |e| {
                if let Some(cb) = lock(&fail_cb).take() {
                    cb(Err(e));
                }
            },
        );
    }

    /// Submits `run` to the library's pool: work that reads its files. False when the pool
    /// refuses it or the library is removed; `expire` runs in place of `run` for a job that
    /// waited past `file_timeout`.
    pub fn submit(&self, run: pool::Run, expire: Option<pool::Run>) -> bool {
        !self.removed() && self.pool().submit(run, expire)
    }

    /// Reads the subtitle `s` on the library's pool as WebVTT, an SRT file turned into it, and
    /// calls `cb` there with it, or with the error; at once on the caller's thread when the
    /// pool refuses the read.
    pub fn subtitle(
        self: &Arc<Self>,
        s: &Subtitle,
        cb: impl FnOnce(Result<Vec<u8>, Error>) + Send + 'static,
    ) {
        if !s.error.is_empty() {
            return cb(Err(Error::NotServed));
        }
        let format = s.format.clone();
        self.open(&s.file, move |h| {
            let r = h.and_then(|h| {
                let len = usize::try_from(h.size).map_err(|_| Error::TooLarge)?;
                let data = h.read_exact(0, len).map_err(|e| Error::Io(e.to_string()))?;
                to_webvtt(&format, &data).map_err(Error::Malformed)
            });
            cb(r);
        });
    }

    /// Gets the parsed movie of `r` and calls `cb` with it, or with the error: at once, on the
    /// caller's thread, when the library keeps it or it fails at once ([`Error::NotServed`] for
    /// a rendition listed with an error, [`Error::TooLarge`], [`Error::Busy`],
    /// [`Error::Removed`]); otherwise from the library's pool once it is read. A movie is read
    /// once for every call that waits for it, and kept, bounded by `max_table_bytes`, until its
    /// file changes or leaves the catalog. It is immutable, and shared.
    pub fn movie(self: &Arc<Self>, r: &Rendition, cb: impl FnOnce(MovieResult) + Send + 'static) {
        if self.removed() {
            return cb(Err(Error::Removed));
        }
        if r.moov.is_none() || !r.error.is_empty() {
            return cb(Err(Error::NotServed));
        }
        let key = key_of(r);
        let need = tables::reckon(r.table_bytes(), 0, r.tracks.len());
        let (begin, back) = self.tables.begin(&key, need, Box::new(cb));
        let id = match begin {
            tables::Begin::Kept(m) => {
                if let Some(cb) = back {
                    cb(Ok(m));
                }
                return;
            }
            tables::Begin::Refused(e) => {
                if let Some(cb) = back {
                    cb(Err(e));
                }
                return;
            }
            tables::Begin::Waiting => return,
            tables::Begin::Read(id) => id,
        };
        let me = Arc::clone(self);
        let r = r.clone();
        let fail_me = Arc::clone(self);
        let fail_key = key.clone();
        self.on_pool(
            Box::new(move || {
                let res = me
                    .open_file(&r.file)
                    .and_then(|h| tables::read_movie(&h, &r, &Limits::default()).map(Arc::new));
                me.tables.done(&key, id, res);
            }),
            move |e| fail_me.tables.done(&fail_key, id, Err(e)),
        );
    }

    #[cfg(test)]
    pub(crate) fn set_probe_hook(&self, h: Option<Hook>) {
        *lock(&self.hook) = h;
    }

    #[cfg(test)]
    pub(crate) fn set_pool(&self, p: Pool) -> Pool {
        std::mem::replace(&mut *lock(&self.pool), p)
    }

    #[cfg(test)]
    pub(crate) fn resize_tables(&self, max: u64) {
        self.tables.resize(max);
    }
}

fn key_of(r: &Rendition) -> tables::Key {
    tables::Key {
        abs: r.file.abs.clone(),
        size: r.file.size,
        modified: r.file.modified,
    }
}

fn short(name: &str) -> String {
    name.chars().take(8).collect()
}

fn pool_name(name: &str) -> String {
    format!("akit-medialib-{}-pool", short(name))
}

// ---- the registry ----

type Libraries = BTreeMap<String, Arc<Library>>;

fn registry() -> &'static Mutex<HashMap<usize, Libraries>> {
    static REG: std::sync::LazyLock<Mutex<HashMap<usize, Libraries>>> =
        std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
    &REG
}

/// The registry's key of an engine, as `media_auth`'s.
#[must_use]
pub fn key(engine: &Arc<akio::Engine>) -> usize {
    crate::auth::registry::key(engine)
}

/// The library named `name` of the engine `key`, or `None` when the running config has none of
/// that name.
#[must_use]
pub fn lookup(key: usize, name: &str) -> Option<Arc<Library>> {
    lock(registry()).get(&key)?.get(name).cloned()
}

/// The libraries of the engine `key`, sorted by name.
#[must_use]
pub fn libraries(key: usize) -> Vec<Arc<Library>> {
    lock(registry())
        .get(&key)
        .map(|m| m.values().cloned().collect())
        .unwrap_or_default()
}

/// Moves the engine `key`'s libraries to `list`: each changed in place, or made and scanned, and
/// those `list` leaves out removed, so that whoever still holds one is refused with
/// [`Error::Removed`]. A library whose roots, patterns or bounds change is scanned again.
///
/// # Errors
///
/// A library that does not compile, which changes nothing.
pub fn apply(key: usize, list: &[MediaLibrary]) -> Result<(), String> {
    let mut sets: BTreeMap<&str, Settings> = BTreeMap::new();
    let mut errs = Vec::new();
    for c in list {
        if sets.contains_key(c.name.as_str()) {
            errs.push(format!("{:?} is listed twice", c.name));
            continue;
        }
        match Settings::compile(c) {
            Ok(s) => {
                sets.insert(&c.name, s);
            }
            Err(e) => errs.push(format!("{}: {e}", c.name)),
        }
    }
    if !errs.is_empty() {
        return Err(errs.join("; "));
    }
    let mut reg = lock(registry());
    let libs = reg.remove(&key).unwrap_or_default();
    for (name, l) in &libs {
        if !sets.contains_key(name.as_str()) {
            l.remove();
        }
    }
    let mut next = Libraries::new();
    for (name, s) in sets {
        match libs.get(name) {
            Some(l) => {
                l.apply(s);
                next.insert(name.to_owned(), Arc::clone(l));
            }
            None => {
                next.insert(name.to_owned(), Library::new(name, s));
            }
        }
    }
    if !next.is_empty() {
        reg.insert(key, next);
    }
    Ok(())
}

/// Removes every library of the engine `key`.
pub fn remove(key: usize) {
    let libs = lock(registry()).remove(&key).unwrap_or_default();
    for l in libs.values() {
        l.remove();
    }
}

// ---- the role ----

/// The role `media_library`, which `akit run` registers before the handlers that serve its
/// libraries.
pub const MEDIA_LIBRARY: Info = Info {
    id: "media_library",
    set: |c| !c.media_library.is_empty(),
    new: |host| Ok(Box::new(LibraryRole::new(&host))),
};

/// The role.
#[derive(Debug)]
pub struct LibraryRole {
    key: usize,
}

impl LibraryRole {
    fn new(host: &Host) -> LibraryRole {
        LibraryRole {
            key: key(&host.engine),
        }
    }
}

impl Role for LibraryRole {
    fn apply(&mut self, config: &Config) -> Result<(), String> {
        apply(self.key, &config.media_library)
    }

    fn stop(&mut self, _grace: Duration) {
        remove(self.key);
    }

    fn metrics(&self) -> Option<MetricsFn> {
        let key = self.key;
        Some(Arc::new(move |out: &mut Vec<u8>| {
            let mut w = akit_observe::metrics::TextWriter::new(std::mem::take(out));
            write_metrics(&mut w, &libraries(key));
            *out = w.out;
        }))
    }
}

/// The libraries' metrics, as fit's, named akit's (decision 037).
pub fn write_metrics(w: &mut dyn akit_observe::MetricWriter, libs: &[Arc<Library>]) {
    w.family(
        "akit_media_library_items",
        "gauge",
        "Items of a library's catalog, by whether they can be served (ok) or are listed with an error.",
    );
    for l in libs {
        let cat = l.catalog();
        let errs = cat.items.iter().filter(|it| !it.error.is_empty()).count() as u64;
        let all = cat.items.len() as u64;
        w.sample_u64(
            "akit_media_library_items",
            all.saturating_sub(errs),
            &[("library", l.name()), ("state", "ok")],
        );
        w.sample_u64(
            "akit_media_library_items",
            errs,
            &[("library", l.name()), ("state", "error")],
        );
    }
    w.family(
        "akit_media_library_catalog_bytes",
        "gauge",
        "The memory a library's catalog takes, as reckoned for max_catalog_bytes.",
    );
    for l in libs {
        w.sample_u64(
            "akit_media_library_catalog_bytes",
            l.catalog().bytes,
            &[("library", l.name())],
        );
    }
    w.family(
        "akit_media_library_scans_total",
        "counter",
        "Scans of a library ended.",
    );
    for l in libs {
        w.sample_u64(
            "akit_media_library_scans_total",
            l.counters.scans.load(Relaxed),
            &[("library", l.name())],
        );
    }
    w.family(
        "akit_media_library_probes_total",
        "counter",
        "Media files a library's scans probed, by whether the probe found an error.",
    );
    for l in libs {
        let errs = l.counters.probe_errors.load(Relaxed);
        let all = l.counters.probed.load(Relaxed);
        w.sample_u64(
            "akit_media_library_probes_total",
            all.saturating_sub(errs),
            &[("library", l.name()), ("result", "ok")],
        );
        w.sample_u64(
            "akit_media_library_probes_total",
            errs,
            &[("library", l.name()), ("result", "error")],
        );
    }
    w.family(
        "akit_media_library_table_bytes",
        "gauge",
        "The memory of the parsed movies a library keeps, bounded by max_table_bytes.",
    );
    for l in libs {
        w.sample_u64(
            "akit_media_library_table_bytes",
            l.tables().bytes,
            &[("library", l.name())],
        );
    }
    w.family(
        "akit_media_library_table_lookups_total",
        "counter",
        "Parsed movies a library was asked for, by whether it kept the movie (hit) or not (miss): read from the file, a read under way waited for, or refused.",
    );
    for l in libs {
        let ts = l.tables();
        w.sample_u64(
            "akit_media_library_table_lookups_total",
            ts.hits,
            &[("library", l.name()), ("result", "hit")],
        );
        w.sample_u64(
            "akit_media_library_table_lookups_total",
            ts.misses,
            &[("library", l.name()), ("result", "miss")],
        );
    }
    w.family(
        "akit_media_library_pool_jobs",
        "gauge",
        "Jobs of a library's file pool, by whether they wait (queued) or run (busy).",
    );
    for l in libs {
        let st = l.pool().stats();
        w.sample_u64(
            "akit_media_library_pool_jobs",
            st.queued as u64,
            &[("library", l.name()), ("state", "queued")],
        );
        w.sample_u64(
            "akit_media_library_pool_jobs",
            st.busy as u64,
            &[("library", l.name()), ("state", "busy")],
        );
    }
    w.family(
        "akit_media_library_pool_refused_total",
        "counter",
        "Jobs a library's file pool refused, its queue full (refused), or gave up after file_timeout in its queue (expired), each a request answered 503.",
    );
    for l in libs {
        let st = l.pool().stats();
        w.sample_u64(
            "akit_media_library_pool_refused_total",
            st.refused,
            &[("library", l.name()), ("reason", "refused")],
        );
        w.sample_u64(
            "akit_media_library_pool_refused_total",
            st.expired,
            &[("library", l.name()), ("reason", "expired")],
        );
    }
}
