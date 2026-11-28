//! A library's uploads (I149, fit's `medialib/upload.go`): files written into its `upload_dir`
//! in pieces, resumable after a cut connection, as tus's protocol has them: an upload begun with
//! its path and size, written at its offset, asked where it is at, given up. Its bytes go to a
//! temporary file in `upload_dir` (`.fit-upload-<id>.part`, beside a `.json` that keeps it
//! across restarts; fit's names, the files on disk being fit's), synced and renamed into place
//! once whole, and probed by the scan that follows.
//!
//! Bounded: `max_upload_bytes` an upload, `max_uploads` under way, `min_free_bytes` left on the
//! disk besides what those under way still need. Nothing touches the disk on an I/O thread: the
//! files are made, written and renamed on the library's pool, a write's body held (its reads)
//! while what was gathered of it, at most 512 KiB and one read more, is written. An upload not
//! written to for `upload_expiry` is given up at the start of a scan, which reads back those a
//! restart left; no timer runs.

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use akit_config::value::Value;

use super::paths::{FileError, clean_rel, disk_free, writable, write_atomic};
use super::{Library, Root};

/// The names of an upload's files in `upload_dir`, which a scan, looking at no name that starts
/// with '.', leaves alone.
const UPLOAD_PREFIX: &str = ".fit-upload-";
const UPLOAD_PART: &str = ".part";
const UPLOAD_META: &str = ".json";
/// The most of an upload's `.json` read back after a restart.
const MAX_UPLOAD_META: u64 = 4 << 10;
/// What of a body is gathered before it is written, on the pool, while the body is held.
const FLUSH_BYTES: usize = 512 << 10;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A library's `upload_dir` and the bounds of its uploads, compiled.
#[derive(Debug, Clone)]
pub(crate) struct UploadDir {
    pub(crate) root: Root,
    /// Its slash path under the root; "" for the root itself.
    pub(crate) rel: String,
    pub(crate) abs: PathBuf,
    pub(crate) max_bytes: u64,
    pub(crate) max: usize,
    pub(crate) min_free: u64,
    pub(crate) expiry: Duration,
}

impl UploadDir {
    /// `upload_dir` of `c`, in one of `roots`.
    pub(crate) fn compile(
        c: &akit_config::medialib::MediaLibrary,
        roots: &[Root],
    ) -> Result<Option<UploadDir>, String> {
        if c.upload_dir.is_empty() {
            return Ok(None);
        }
        let abs = akit_config::medialib::absolute(&c.upload_dir);
        let Some(root) = roots.iter().find(|r| super::files::inside(&r.abs, &abs)) else {
            return Err(format!(
                "upload_dir {:?} is not inside a root of the library",
                c.upload_dir
            ));
        };
        let rel = abs
            .strip_prefix(&root.abs)
            .map(|p| {
                p.components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .unwrap_or_default();
        Ok(Some(UploadDir {
            root: root.clone(),
            rel,
            abs,
            max_bytes: c.upload_bytes(),
            max: c.uploads(),
            min_free: c.min_free(),
            expiry: c.expiry(),
        }))
    }

    /// As `GET /media/library` shows it (fit's `UploadSettings`).
    pub(crate) fn to_value(&self) -> Value {
        let n = |v: u64| Value::Number(v.to_string());
        Value::Object(vec![
            ("root".to_owned(), Value::String(self.root.conf.clone())),
            ("dir".to_owned(), Value::String(self.rel.clone())),
            ("max_bytes".to_owned(), n(self.max_bytes)),
            ("max_uploads".to_owned(), n(self.max as u64)),
            ("min_free_bytes".to_owned(), n(self.min_free)),
            (
                "expiry".to_owned(),
                Value::Number(self.expiry.as_secs().to_string()),
            ),
        ])
    }
}

/// An upload to begin: its path under `upload_dir`, its size, and who asks.
#[derive(Debug, Clone)]
pub struct UploadSpec {
    /// Its slash path under `upload_dir`.
    pub path: String,
    /// Its bytes.
    pub size: i64,
    /// Who asks.
    pub user: String,
}

/// An upload under way, or the one just finished (`complete`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadInfo {
    /// Its ID.
    pub id: String,
    /// The root its file goes to, as the config writes it.
    pub root: String,
    /// Its file's path under the root.
    pub path: String,
    /// Its bytes.
    pub size: u64,
    /// Those written.
    pub offset: u64,
    /// Who began it.
    pub user: String,
    /// When it began, and was last written to.
    pub created: SystemTime,
    /// See [`created`](Self::created).
    pub updated: SystemTime,
    /// Being made, or written to.
    pub busy: bool,
    /// Whole and in place.
    pub complete: bool,
}

impl UploadInfo {
    /// As the Admin API writes it (fit's `UploadInfo`).
    #[must_use]
    pub fn to_value(&self) -> Value {
        let n = |v: u64| Value::Number(v.to_string());
        let mut m = vec![
            ("id", Value::String(self.id.clone())),
            ("root", Value::String(self.root.clone())),
            ("path", Value::String(self.path.clone())),
            ("size", n(self.size)),
            ("offset", n(self.offset)),
        ];
        if !self.user.is_empty() {
            m.push(("user", Value::String(self.user.clone())));
        }
        m.extend([
            ("created", super::catalog::time(self.created)),
            ("updated", super::catalog::time(self.updated)),
            ("busy", Value::Bool(self.busy)),
        ]);
        if self.complete {
            m.push(("complete", Value::Bool(true)));
        }
        Value::Object(m.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    }
}

/// An upload under way.
pub(crate) struct Upload {
    id: String,
    root: Root,
    /// The file's path under the root once whole.
    rel: String,
    /// `upload_dir`, its `.part` and `.json` in it.
    dir: PathBuf,
    size: u64,
    user: String,
    created: SystemTime,
    st: Mutex<UpState>,
}

struct UpState {
    offset: u64,
    updated: SystemTime,
    /// Being made, or written to.
    busy: bool,
    /// Cancelled or ended while busy: removed once not.
    gone: bool,
    f: Option<File>,
}

impl Upload {
    fn part(&self) -> PathBuf {
        self.dir
            .join(format!("{UPLOAD_PREFIX}{}{UPLOAD_PART}", self.id))
    }

    fn meta(&self) -> PathBuf {
        self.dir
            .join(format!("{UPLOAD_PREFIX}{}{UPLOAD_META}", self.id))
    }

    fn info_of(&self, st: &UpState) -> UploadInfo {
        UploadInfo {
            id: self.id.clone(),
            root: self.root.conf.clone(),
            path: self.rel.clone(),
            size: self.size,
            offset: st.offset,
            user: self.user.clone(),
            created: self.created,
            updated: st.updated,
            busy: st.busy,
            complete: false,
        }
    }

    fn info(&self) -> UploadInfo {
        self.info_of(&lock(&self.st))
    }
}

/// A library's uploads under way.
#[derive(Default)]
pub(crate) struct Uploads {
    m: HashMap<String, Arc<Upload>>,
    /// The `upload_dir` whose uploads were read back, after a restart.
    loaded: Option<PathBuf>,
}

/// An upload's `.json`, which keeps it across restarts (fit's `uploadMetaFile`).
fn meta_json(u: &Upload) -> String {
    let v = Value::Object(vec![
        ("id".to_owned(), Value::String(u.id.clone())),
        ("root".to_owned(), Value::String(u.root.conf.clone())),
        ("path".to_owned(), Value::String(u.rel.clone())),
        ("size".to_owned(), Value::Number(u.size.to_string())),
        ("user".to_owned(), Value::String(u.user.clone())),
        ("created".to_owned(), super::catalog::time(u.created)),
    ]);
    super::catalog::compact(&v)
}

impl Library {
    /// The library's `upload_dir` and its bounds, or `None` when it takes no uploads.
    pub(crate) fn upload_dir(&self) -> Option<UploadDir> {
        self.settings().up.clone()
    }

    /// The uploads under way, the oldest first.
    #[must_use]
    pub fn uploads(&self) -> Vec<UploadInfo> {
        let list: Vec<Arc<Upload>> = lock(&self.uploads).m.values().cloned().collect();
        let mut out: Vec<UploadInfo> = list
            .iter()
            .filter_map(|u| {
                let st = lock(&u.st);
                (!st.gone).then(|| u.info_of(&st))
            })
            .collect();
        out.sort_by_key(|u| u.created);
        out
    }

    /// The upload `id` under way.
    #[must_use]
    pub fn upload(&self, id: &str) -> Option<UploadInfo> {
        let u = self.lookup_upload(id)?;
        let st = lock(&u.st);
        (!st.gone).then(|| u.info_of(&st))
    }

    /// Waits until the upload `id` is not being made or written to and `f` holds of it,
    /// `limit` at most: for tests, which wait for what a write did. `None` for an upload not
    /// under way, or past the limit.
    #[doc(hidden)]
    pub fn wait_upload(
        &self,
        id: &str,
        limit: Duration,
        f: impl Fn(&UploadInfo) -> bool,
    ) -> Option<UploadInfo> {
        let deadline = std::time::Instant::now().checked_add(limit)?;
        let mut g = lock(&self.uploads);
        loop {
            let info = g.m.get(id).and_then(|u| {
                let st = lock(&u.st);
                (!st.gone).then(|| u.info_of(&st))
            });
            if let Some(i) = info.filter(|i| !i.busy && f(i)) {
                return Some(i);
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return None;
            }
            g = self
                .upload_changed
                .wait_timeout(g, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Tells those waiting for an upload that one changed.
    pub(crate) fn tell_uploads(&self) {
        let _g = lock(&self.uploads);
        self.upload_changed.notify_all();
    }

    fn lookup_upload(&self, id: &str) -> Option<Arc<Upload>> {
        lock(&self.uploads).m.get(id).cloned()
    }

    /// Begins an upload of `spec.size` bytes to `spec.path` under `upload_dir`: an empty
    /// temporary file in `upload_dir`, and its `.json`. Refused with [`FileError::NoUploads`]
    /// without `upload_dir`, [`FileError::BadPath`] or [`FileError::NotMedia`] for a path the
    /// library does not write or look at, [`FileError::UploadSize`] or [`FileError::TooLarge`]
    /// for a size of no bytes or past `max_upload_bytes`, [`FileError::TooMany`] past
    /// `max_uploads`, [`FileError::Exists`] for a file there already or being uploaded,
    /// [`FileError::Outside`] for a directory leading out of the root, and
    /// [`FileError::NoSpace`] when the disk would be left with less than `min_free_bytes`
    /// besides what the uploads under way still need. `done` is called on the pool, or at once
    /// with an error found before.
    pub fn create_upload(
        self: &Arc<Self>,
        spec: &UploadSpec,
        done: impl FnOnce(Result<UploadInfo, FileError>) + Send + 'static,
    ) {
        let up = self.upload_dir();
        let checked = (|| {
            if self.removed() {
                return Err(FileError::Removed);
            }
            let up = up.as_ref().ok_or(FileError::NoUploads)?;
            let rel = clean_rel(&spec.path, false)?;
            if super::scan::kind_of_name(&rel).is_none() {
                return Err(FileError::NotMedia);
            }
            if spec.size <= 0 {
                return Err(FileError::UploadSize);
            }
            let size = u64::try_from(spec.size).unwrap_or(0);
            if size > up.max_bytes {
                return Err(FileError::TooLarge(format!(
                    "{size} bytes, past max_upload_bytes, {}",
                    up.max_bytes
                )));
            }
            let rel = if up.rel.is_empty() {
                rel
            } else {
                format!("{}/{rel}", up.rel)
            };
            Ok((up.clone(), rel, size))
        })();
        let (up, rel, size) = match checked {
            Ok(v) => v,
            Err(e) => return done(Err(e)),
        };
        let mut idb = [0u8; 16];
        let _ = akio_crypto::rand::fill_random(&mut idb);
        let now = SystemTime::now();
        let u = Arc::new(Upload {
            id: super::catalog::hex(&idb),
            root: up.root.clone(),
            rel,
            dir: up.abs.clone(),
            size,
            user: spec.user.clone(),
            created: now,
            st: Mutex::new(UpState {
                offset: 0,
                updated: now,
                busy: true,
                gone: false,
                f: None,
            }),
        });
        // The upload takes its place at once, so that max_uploads and a path taken hold for
        // those asked for together; what the others still need of the disk is reckoned now.
        let mut need: u64 = 0;
        let taken = {
            let mut g = lock(&self.uploads);
            let mut n = 0usize;
            let mut err = None;
            for o in g.m.values() {
                let st = lock(&o.st);
                if !st.gone {
                    n = n.saturating_add(1);
                    need = need.saturating_add(o.size.saturating_sub(st.offset));
                    if o.rel == u.rel && o.root.abs == u.root.abs {
                        err = Some(FileError::Exists(format!("{} is being uploaded", u.rel)));
                    }
                }
            }
            if err.is_none() && n >= up.max {
                err = Some(FileError::TooMany(up.max));
            }
            if err.is_none() {
                g.m.insert(u.id.clone(), Arc::clone(&u));
            }
            err
        };
        if let Some(e) = taken {
            return done(Err(e));
        }
        let free = need.saturating_add(up.min_free);
        let me = Arc::clone(self);
        let fail_me = Arc::clone(self);
        let (u2, u3) = (Arc::clone(&u), Arc::clone(&u));
        type Done = Box<dyn FnOnce(Result<UploadInfo, FileError>) + Send>;
        let done: Arc<Mutex<Option<Done>>> = Arc::new(Mutex::new(Some(Box::new(done))));
        let tell = |d: &Arc<Mutex<Option<Done>>>| {
            let d = Arc::clone(d);
            move |r: Result<UploadInfo, FileError>| {
                if let Some(f) = lock(&d).take() {
                    f(r);
                }
            }
        };
        let (call, call2, refused) = (tell(&done), tell(&done), tell(&done));
        let run = Box::new(move || {
            if let Err(e) = make_upload(&u2, free) {
                let _ = std::fs::remove_file(u2.part());
                let _ = std::fs::remove_file(u2.meta());
                me.drop_upload(&u2);
                return call(Err(e));
            }
            let info = {
                let mut st = lock(&u2.st);
                st.busy = false;
                u2.info_of(&st)
            };
            me.tell_uploads();
            call(Ok(info));
        });
        let fail = Box::new(move || {
            fail_me.drop_upload(&u3);
            call2(Err(FileError::Busy));
        });
        if !self.submit(run, Some(fail)) {
            self.drop_upload(&u);
            refused(Err(FileError::Busy));
        }
    }

    /// Takes `u` out of the uploads under way.
    fn drop_upload(&self, u: &Arc<Upload>) {
        let mut g = lock(&self.uploads);
        if g.m.get(&u.id).is_some_and(|o| Arc::ptr_eq(o, u)) {
            g.m.remove(&u.id);
        }
        self.upload_changed.notify_all();
    }

    /// Removes `u`'s files and takes it out of the uploads under way. It blocks.
    fn remove_upload(&self, u: &Arc<Upload>) {
        let f = {
            let mut st = lock(&u.st);
            st.gone = true;
            st.f.take()
        };
        drop(f);
        let _ = std::fs::remove_file(u.part());
        let _ = std::fs::remove_file(u.meta());
        self.drop_upload(u);
    }

    /// Gives the upload `id` up, its temporary file removed on the pool; one being written to is
    /// removed once its write ends. `done` is called on the pool or at once.
    pub fn cancel_upload(
        self: &Arc<Self>,
        id: &str,
        done: impl FnOnce(Result<(), FileError>) + Send + 'static,
    ) {
        let Some(u) = self.lookup_upload(id) else {
            return done(Err(FileError::NoUpload));
        };
        let (gone, busy) = {
            let mut st = lock(&u.st);
            let was = (st.gone, st.busy);
            st.gone = true;
            was
        };
        if gone {
            return done(Err(FileError::NoUpload));
        }
        if busy {
            // The write ends it.
            return done(Ok(()));
        }
        let me = Arc::clone(self);
        type Done = Box<dyn FnOnce(Result<(), FileError>) + Send>;
        let done: Arc<Mutex<Option<Done>>> = Arc::new(Mutex::new(Some(Box::new(done))));
        let tell = |d: &Arc<Mutex<Option<Done>>>| {
            let d = Arc::clone(d);
            move || {
                if let Some(f) = lock(&d).take() {
                    f(Ok(()));
                }
            }
        };
        let (a, b, c) = (tell(&done), tell(&done), tell(&done));
        let run = Box::new(move || {
            me.remove_upload(&u);
            a();
        });
        // Otherwise the next scan's sweep removes it.
        if !self.submit(run, Some(Box::new(b))) {
            c();
        }
    }

    /// Begins a write to the upload `id` at `offset`, which must be where it is at: the
    /// [`Chunk`] to give its body to. [`FileError::NoUpload`], [`FileError::UploadBusy`] while
    /// another write runs, [`FileError::Offset`] for another offset. It never blocks.
    ///
    /// # Errors
    ///
    /// Those.
    pub fn write_upload(self: &Arc<Self>, id: &str, offset: u64) -> Result<Arc<Chunk>, FileError> {
        if self.removed() {
            return Err(FileError::Removed);
        }
        let u = self.lookup_upload(id).ok_or(FileError::NoUpload)?;
        {
            let mut st = lock(&u.st);
            if st.gone {
                return Err(FileError::NoUpload);
            }
            if st.busy {
                return Err(FileError::UploadBusy);
            }
            if offset != st.offset {
                return Err(FileError::Offset(st.offset));
            }
            st.busy = true;
        }
        Ok(Arc::new(Chunk {
            l: Arc::clone(self),
            u,
            st: Mutex::new(ChunkState {
                buf: Vec::new(),
                off: offset,
                busy: false,
                ended: false,
                fin: false,
                err: None,
            }),
            hooks: Mutex::new(None),
        }))
    }

    /// Closes the uploads' files as the library is removed; their files stay, to be taken up
    /// again with the library.
    pub(crate) fn close_uploads(&self) {
        let g = lock(&self.uploads);
        for u in g.m.values() {
            let mut st = lock(&u.st);
            if !st.busy {
                st.f = None;
            }
        }
    }

    /// At the start of each scan, on the scanner's thread: reads back the uploads of `up` a
    /// restart left, gives up those not written to for `upload_expiry`, and drops from memory
    /// those of an `upload_dir` no longer the library's. It blocks.
    pub(crate) fn sweep_uploads(&self, up: Option<&UploadDir>) {
        let now = SystemTime::now();
        let mut stale = Vec::new();
        let load = {
            let mut g = lock(&self.uploads);
            let mut drop_ids = Vec::new();
            for u in g.m.values() {
                let mut st = lock(&u.st);
                match up {
                    _ if st.busy => {}
                    Some(up) if u.dir == up.abs && u.root.abs == up.root.abs => {
                        let idle = now.duration_since(st.updated).unwrap_or_default();
                        if idle > up.expiry {
                            stale.push(Arc::clone(u));
                        }
                    }
                    _ => {
                        // Kept on disk, to be taken up again with its upload_dir.
                        st.f = None;
                        drop_ids.push(u.id.clone());
                    }
                }
            }
            for id in drop_ids {
                g.m.remove(&id);
            }
            let load = up.is_some_and(|up| g.loaded.as_deref() != Some(up.abs.as_path()));
            g.loaded = up.map(|u| u.abs.clone());
            load
        };
        for u in &stale {
            self.remove_upload(u);
        }
        if let Some(up) = up {
            self.read_uploads(up, load, now);
        }
    }

    /// Reads `upload_dir` for uploads' files: those of uploads not under way, when `load` is
    /// set, taken up again, and those left behind, older than `upload_expiry`, removed. It
    /// blocks.
    fn read_uploads(&self, up: &UploadDir, load: bool, now: SystemTime) {
        let Ok(rd) = std::fs::read_dir(&up.abs) else {
            return;
        };
        let mut metas: Vec<MetaFile> = Vec::new();
        let mut stale: Vec<String> = Vec::new();
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(rest) = name.strip_prefix(UPLOAD_PREFIX) else {
                continue;
            };
            if !e.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            let (id, is_meta) = match rest.strip_suffix(UPLOAD_META) {
                Some(id) => (id, true),
                None => match rest.strip_suffix(UPLOAD_PART) {
                    Some(id) => (id, false),
                    None => continue,
                },
            };
            if self.lookup_upload(id).is_some() {
                continue;
            }
            if e.metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| now.duration_since(t).unwrap_or_default() > up.expiry)
                && !stale.iter().any(|s| s == id)
            {
                stale.push(id.to_owned());
            }
            if is_meta && load {
                if let Some(m) = read_meta(&up.abs.join(&name), id) {
                    metas.push(m);
                }
            }
        }
        metas.sort_by_key(|m| m.created);
        for m in metas {
            if stale.contains(&m.id) {
                continue;
            }
            let part = up.abs.join(format!("{UPLOAD_PREFIX}{}{UPLOAD_PART}", m.id));
            let fi = std::fs::metadata(&part);
            let rel = clean_rel(&m.path, false);
            let ok = match (&fi, &rel) {
                (Ok(fi), Ok(rel)) => {
                    m.root == up.root.conf
                        && (up.rel.is_empty() || rel.starts_with(&format!("{}/", up.rel)))
                        && m.size > 0
                        && fi.len() <= m.size
                }
                _ => false,
            };
            let (Ok(fi), Ok(rel), true) = (fi, rel, ok) else {
                stale.push(m.id);
                continue;
            };
            let u = Arc::new(Upload {
                id: m.id.clone(),
                root: up.root.clone(),
                rel,
                dir: up.abs.clone(),
                size: m.size,
                user: m.user,
                created: m.created,
                st: Mutex::new(UpState {
                    offset: fi.len(),
                    updated: fi.modified().unwrap_or(now),
                    busy: false,
                    gone: false,
                    f: None,
                }),
            });
            let mut g = lock(&self.uploads);
            if g.m.len() < up.max {
                g.m.insert(m.id, u);
            } else {
                stale.push(m.id);
            }
        }
        for id in stale {
            let _ = std::fs::remove_file(up.abs.join(format!("{UPLOAD_PREFIX}{id}{UPLOAD_PART}")));
            let _ = std::fs::remove_file(up.abs.join(format!("{UPLOAD_PREFIX}{id}{UPLOAD_META}")));
        }
    }
}

/// An upload's `.json` as read back.
struct MetaFile {
    id: String,
    root: String,
    path: String,
    size: u64,
    user: String,
    created: SystemTime,
}

/// Reads an upload's `.json`, which must be `id`'s.
fn read_meta(p: &Path, id: &str) -> Option<MetaFile> {
    use std::io::Read as _;
    let f = File::open(p).ok()?;
    let mut data = Vec::new();
    f.take(MAX_UPLOAD_META.saturating_add(1))
        .read_to_end(&mut data)
        .ok()?;
    if data.len() as u64 > MAX_UPLOAD_META {
        return None;
    }
    let v = akit_config::value::parse(std::str::from_utf8(&data).ok()?).ok()?;
    let s = |k: &str| match v.get(k) {
        Some(Value::String(x)) => Some(x.clone()),
        _ => None,
    };
    let size = match v.get("size") {
        Some(Value::Number(n)) => n.parse::<u64>().ok()?,
        _ => return None,
    };
    let created = s("created")
        .and_then(|t| parse_time(&t))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let m = MetaFile {
        id: s("id")?,
        root: s("root")?,
        path: s("path")?,
        size,
        user: s("user").unwrap_or_default(),
        created,
    };
    (m.id == id).then_some(m)
}

/// An RFC 3339 time, as Go writes a `time.Time` (`2026-11-28T13:45:00.5+08:00`).
fn parse_time(t: &str) -> Option<SystemTime> {
    let num = |a: usize, b: usize| t.get(a..b)?.parse::<i64>().ok();
    if t.len() < 20 || t.as_bytes().get(10) != Some(&b'T') {
        return None;
    }
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, sec) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut rest = t.get(19..)?;
    let mut nanos: u64 = 0;
    if let Some(f) = rest.strip_prefix('.') {
        let n = f.find(|c: char| !c.is_ascii_digit()).unwrap_or(f.len());
        let digits = f.get(..n)?;
        let mut v = format!("{digits:0<9}");
        v.truncate(9);
        nanos = v.parse().ok()?;
        rest = f.get(n..)?;
    }
    let offset = match rest {
        "Z" => 0,
        o if o.len() == 6 => {
            let oh = o.get(1..3)?.parse::<i64>().ok()?;
            let om = o.get(4..6)?.parse::<i64>().ok()?;
            let v = oh
                .saturating_mul(3600)
                .saturating_add(om.saturating_mul(60));
            if o.starts_with('-') {
                v.saturating_neg()
            } else {
                v
            }
        }
        _ => return None,
    };
    let days =
        akit_observe::time::days_from_civil(y, u32::try_from(mo).ok()?, u32::try_from(d).ok()?);
    let secs = days
        .checked_mul(86_400)?
        .checked_add(
            h.saturating_mul(3600)
                .saturating_add(mi.saturating_mul(60))
                .saturating_add(sec),
        )?
        .checked_sub(offset)?;
    SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(u64::try_from(secs).ok()?))?
        .checked_add(Duration::from_nanos(nanos))
}

/// Makes `u`'s files, leaving `free` bytes free besides it. It blocks.
fn make_upload(u: &Upload, free: u64) -> Result<(), FileError> {
    let abs = writable(&u.root, &u.rel)?;
    if std::fs::symlink_metadata(&abs).is_ok() {
        return Err(FileError::Exists(u.rel.clone()));
    }
    std::fs::create_dir_all(&u.dir).map_err(|e| FileError::io(&e))?;
    if let Some(have) = disk_free(&u.dir) {
        if have < u.size.saturating_add(free) {
            return Err(FileError::NoSpace(format!(
                "{have} bytes free, {} needed besides min_free_bytes and the uploads under way",
                u.size
            )));
        }
    }
    let f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(u.part())
        .map_err(|e| FileError::io(&e))?;
    if let Err(e) = write_atomic(&u.meta(), meta_json(u).as_bytes()) {
        return Err(FileError::io(&e));
    }
    lock(&u.st).f = Some(f);
    Ok(())
}

/// What a write tells the request it reads the body of: hold (true) or let go (false) the
/// body's reads, and its end, once.
pub struct ChunkHooks {
    /// Holds or lets go the body's reads.
    pub hold: Box<dyn Fn(bool) + Send + Sync>,
    /// The end: the upload, `complete` once whole and in place, or the error.
    pub done: Box<dyn FnOnce(UploadInfo, Result<(), FileError>) + Send>,
}

/// A write to an upload: a request's body, from where the upload is at. Its bytes are gathered
/// as they come, and written on the library's pool, 512 KiB at a time, the body's reads held
/// meanwhile; a read already under way when they are held is gathered too.
pub struct Chunk {
    l: Arc<Library>,
    u: Arc<Upload>,
    st: Mutex<ChunkState>,
    hooks: Mutex<Option<ChunkHooks>>,
}

struct ChunkState {
    buf: Vec<u8>,
    /// Where `buf` goes in the file.
    off: u64,
    /// A write runs on the pool.
    busy: bool,
    ended: bool,
    /// The body's end, and its error.
    fin: bool,
    err: Option<String>,
}

impl Chunk {
    /// What the upload still takes from the write's offset.
    #[must_use]
    pub fn remaining(&self) -> u64 {
        self.u.size.saturating_sub(lock(&self.st).off)
    }

    /// The upload as the write found it.
    #[must_use]
    pub fn info(&self) -> UploadInfo {
        self.u.info()
    }

    /// Sets what the write tells its request.
    pub fn set_hooks(&self, hooks: ChunkHooks) {
        *lock(&self.hooks) = Some(hooks);
    }

    fn hold(&self, h: bool) {
        if let Some(k) = lock(&self.hooks).as_ref() {
            (k.hold)(h);
        }
    }

    /// Ends a write before its body with `err`, and gives the upload up, its file removed: for a
    /// body refused before it is read, as one longer than the upload.
    pub fn abort(self: &Arc<Self>, err: FileError) {
        let me = Arc::clone(self);
        let l = Arc::clone(&self.l);
        // Removing the files blocks: on the pool, or left to the next sweep.
        if !l.submit(Box::new(move || me.fail(err, true)), None) {
            self.fail(FileError::Busy, false);
        }
    }

    /// Takes a piece of the body: `data`, `fin` for the last, `err` for a body that ended
    /// without it. What is gathered is written on the pool, the reads held while it is. Never
    /// blocks.
    pub fn write(self: &Arc<Self>, data: &[u8], fin: bool, err: Option<String>) {
        let mut st = lock(&self.st);
        if st.ended {
            return;
        }
        let total = st
            .off
            .saturating_add(st.buf.len() as u64)
            .saturating_add(data.len() as u64);
        if total > self.u.size {
            drop(st);
            return self.abort(FileError::TooLarge(format!(
                "the body is longer than the upload's {} bytes",
                self.u.size
            )));
        }
        st.buf.extend_from_slice(data);
        st.fin |= fin;
        if err.is_some() {
            st.err = err;
        }
        if st.busy {
            // The write in flight takes it once it is done.
            return;
        }
        let last = st.fin || st.err.is_some();
        if !last && st.buf.len() < FLUSH_BYTES {
            return;
        }
        st.busy = true;
        drop(st);
        if !last {
            self.hold(true);
        }
        let me = Arc::clone(self);
        let me2 = Arc::clone(self);
        if !self.l.submit(
            Box::new(move || me.flush()),
            Some(Box::new(move || me2.fail(FileError::Busy, false))),
        ) {
            self.fail(FileError::Busy, false);
        }
    }

    /// Writes what was gathered, on the pool, and then ends the write, or lets the body go on.
    fn flush(self: &Arc<Self>) {
        loop {
            let (buf, off) = {
                let mut st = lock(&self.st);
                (std::mem::take(&mut st.buf), st.off)
            };
            if let Err(e) = self.write_at(&buf, off) {
                let remove = matches!(e, FileError::NoSpace(_));
                return self.fail(e, remove);
            }
            let (fin, err, more) = {
                let mut st = lock(&self.st);
                st.off = st.off.saturating_add(buf.len() as u64);
                (st.fin, st.err.clone(), st.buf.len())
            };
            let gone = {
                let mut u = lock(&self.u.st);
                u.offset = off.saturating_add(buf.len() as u64);
                u.updated = SystemTime::now();
                u.gone
            };
            if gone {
                return self.fail(FileError::NoUpload, true);
            }
            if let Some(e) = err {
                return self.fail(FileError::Io(e), false);
            }
            if more > 0 && (fin || more >= FLUSH_BYTES) {
                continue;
            }
            let at = lock(&self.st).off;
            if fin && more == 0 && at == self.u.size {
                return self.finish();
            }
            if fin && more == 0 {
                let info = self.u.info();
                return self.end(info, Ok(()));
            }
            lock(&self.st).busy = false;
            self.hold(false);
            return;
        }
    }

    /// Writes `buf` at `off` to the upload's file, opened when it is not yet, as after a
    /// restart. It blocks.
    fn write_at(&self, buf: &[u8], off: u64) -> Result<(), FileError> {
        use std::os::unix::fs::FileExt as _;
        let mut st = lock(&self.u.st);
        if st.f.is_none() {
            let f = std::fs::OpenOptions::new()
                .write(true)
                .open(self.u.part())
                .map_err(|e| FileError::io(&e))?;
            let len = f.metadata().map_err(|e| FileError::io(&e))?.len();
            if len < off {
                return Err(FileError::Changed(
                    "its temporary file is shorter than the upload's offset".to_owned(),
                ));
            }
            st.f = Some(f);
        }
        if buf.is_empty() {
            return Ok(());
        }
        let f = st.f.as_ref().ok_or(FileError::NoUpload)?;
        f.write_all_at(buf, off).map_err(|e| {
            if e.kind() == std::io::ErrorKind::StorageFull {
                FileError::NoSpace(e.to_string())
            } else {
                FileError::io(&e)
            }
        })
    }

    /// Renames the whole upload into place and asks for a scan, which probes it. It blocks.
    fn finish(self: &Arc<Self>) {
        let placed = {
            let _g = lock(&self.l.file_ops);
            place(&self.u)
        };
        if let Err(e) = placed {
            return self.fail(e, true);
        }
        let mut info = {
            let mut st = lock(&self.u.st);
            let info = self.u.info_of(&st);
            st.busy = false;
            st.gone = true;
            info
        };
        self.l.drop_upload(&self.u);
        info.busy = false;
        info.complete = true;
        self.l.rescan();
        self.end(info, Ok(()));
    }

    /// Ends the write with `err`; `remove` gives the upload up, its file removed.
    fn fail(self: &Arc<Self>, err: FileError, remove: bool) {
        let info = self.u.info();
        if remove {
            self.l.remove_upload(&self.u);
        }
        self.end(info, Err(err));
    }

    /// Ends the write, once: the upload is free for the next write, or removed if cancelled
    /// meanwhile, and the request is told.
    fn end(self: &Arc<Self>, mut info: UploadInfo, mut r: Result<(), FileError>) {
        {
            let mut st = lock(&self.st);
            if st.ended {
                return;
            }
            st.ended = true;
            st.buf = Vec::new();
        }
        let gone = {
            let mut u = lock(&self.u.st);
            u.busy = false;
            u.gone && !info.complete
        };
        if gone
            && !matches!(
                r,
                Err(FileError::TooLarge(_) | FileError::NoSpace(_) | FileError::NoUpload)
            )
        {
            // Cancelled while it was written to.
            let (l, u) = (Arc::clone(&self.l), Arc::clone(&self.u));
            let l2 = Arc::clone(&self.l);
            let u2 = Arc::clone(&self.u);
            if !self.l.submit(Box::new(move || l.remove_upload(&u)), None) {
                l2.drop_upload(&u2);
            }
            if r.is_ok() {
                r = Err(FileError::NoUpload);
            }
        }
        info.busy = false;
        self.l.tell_uploads();
        if let Some(k) = lock(&self.hooks).take() {
            (k.done)(info, r);
        }
    }
}

impl std::fmt::Debug for ChunkHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChunkHooks").finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Chunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Chunk")
            .field("upload", &self.u.id)
            .finish_non_exhaustive()
    }
}

/// Renames `u`'s whole file into place. It blocks.
fn place(u: &Upload) -> Result<(), FileError> {
    let f = lock(&u.st).f.take();
    if let Some(f) = f {
        f.sync_all().map_err(|e| FileError::io(&e))?;
    }
    let abs = writable(&u.root, &u.rel)?;
    if std::fs::symlink_metadata(&abs).is_ok() {
        return Err(FileError::Exists(u.rel.clone()));
    }
    if let Some(dir) = abs.parent() {
        std::fs::create_dir_all(dir).map_err(|e| FileError::io(&e))?;
    }
    std::fs::rename(u.part(), &abs).map_err(|e| FileError::io(&e))?;
    let _ = std::fs::remove_file(u.meta());
    Ok(())
}
