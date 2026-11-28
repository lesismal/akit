//! A library's files changed (I149, fit's `medialib/manage.go`): an item renamed, moved or
//! deleted, every file of it; its sidecar read, or drafted from what its names group, and written;
//! a root's directories listed. Each change is worked out from the catalog, then done on the
//! library's pool, one at a time, each file checked as the catalog has it and written only inside
//! its root; a rename or move that fails part way moves back what it moved. The item leaves the
//! catalog at once, and stays out of a scan begun before; the scan asked for lists it as it is.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, MutexGuard, PoisonError};
use std::time::SystemTime;

use akit_config::value::Value;

use super::catalog::{File, Item, path_id, time};
use super::files::{Sidecar, SidecarFile, inside, parse_sidecar};
use super::paths::{FileError, clean_rel, path_of, valid_name, writable, write_atomic};
use super::scan::{Kind, kind_of_name, split_label, under_dir};
use super::{Library, Root};

/// The most directories [`Library::dirs`] lists.
pub const MAX_DIRS: usize = 2000;
/// The most files a [`SidecarView`] lists.
pub const MAX_SIDECAR_FILES: usize = 1000;
/// The bounds of a path the library writes, less what a label and an extension take.
const MAX_BASE_BYTES: usize = 255 - 32;

fn lock<T>(m: &std::sync::Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a change of a library's files did (fit's `Change`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The library.
    pub library: String,
    /// `rename`, `move`, `delete` or `sidecar`.
    pub op: &'static str,
    /// The ID of the item changed.
    pub item: String,
    /// Its ID once the scan that follows lists it; empty for one deleted.
    pub id: String,
    /// Its root, as the config writes it.
    pub root: String,
    /// Its files, by their paths under the root, from where to where: `from` alone for a file
    /// removed, `to` alone for one written.
    pub files: Vec<(String, String)>,
}

impl Change {
    /// As the Admin API writes it.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let s = |v: &str| Value::String(v.to_owned());
        let mut m = vec![("library", s(&self.library)), ("op", s(self.op))];
        if !self.item.is_empty() {
            m.push(("item", s(&self.item)));
        }
        if !self.id.is_empty() {
            m.push(("id", s(&self.id)));
        }
        m.push(("root", s(&self.root)));
        let files = self
            .files
            .iter()
            .map(|(from, to)| {
                let mut f = Vec::new();
                if !from.is_empty() {
                    f.push(("from".to_owned(), s(from)));
                }
                if !to.is_empty() {
                    f.push(("to".to_owned(), s(to)));
                }
                Value::Object(f)
            })
            .collect();
        m.push(("files", Value::Array(files)));
        Value::Object(m.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    }
}

/// A file a change moves: to `to` (its path under the root once done), or removes (`to`
/// empty).
struct Move {
    f: File,
    to: String,
    to_abs: PathBuf,
}

/// A change worked out from the catalog, before the disk is touched.
struct Plan {
    op: &'static str,
    item: Arc<Item>,
    root: Root,
    id: String,
    moves: Vec<Move>,
    /// The sidecar's path the item will have, for a rewrite.
    side_new: String,
}

/// What a change does once its files moved, undoing them all when it fails.
type After = Box<dyn FnOnce(&Plan) -> Result<Vec<(String, String)>, FileError> + Send>;

/// What a change tells its caller.
type Done = Box<dyn FnOnce(Result<Change, (FileError, Option<Box<Change>>)>) + Send>;

/// The slash path's directory, "" at the root.
fn rel_dir(p: &str) -> &str {
    p.rsplit_once('/').map_or("", |(d, _)| d)
}

/// `dir` and `name` joined as a slash path.
fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

fn base_of(p: &str) -> &str {
    p.rsplit_once('/').map_or(p, |(_, b)| b)
}

/// An item's sidecar as a sidecar file holds it: two spaces to a level, fit's field order,
/// empty fields left out, a newline at the end (fit's `encodeSidecar`).
pub(crate) fn encode_sidecar(sd: &Sidecar) -> String {
    let s = |v: &str| Value::String(v.to_owned());
    let file = |f: &SidecarFile, subtitle: bool| {
        let mut m = vec![("file".to_owned(), s(&f.file))];
        if !f.name.is_empty() {
            m.push(("name".to_owned(), s(&f.name)));
        }
        if !f.language.is_empty() {
            m.push(("language".to_owned(), s(&f.language)));
        }
        if f.default {
            m.push(("default".to_owned(), Value::Bool(true)));
        }
        if subtitle && f.forced {
            m.push(("forced".to_owned(), Value::Bool(true)));
        }
        Value::Object(m)
    };
    let mut m = Vec::new();
    for (k, v) in [("id", &sd.id), ("title", &sd.title), ("poster", &sd.poster)] {
        if !v.is_empty() {
            m.push((k.to_owned(), s(v)));
        }
    }
    for (k, list, subtitle) in [
        ("renditions", &sd.renditions, false),
        ("audio", &sd.audio, false),
        ("subtitles", &sd.subtitles, true),
    ] {
        if !list.is_empty() {
            m.push((
                k.to_owned(),
                Value::Array(list.iter().map(|f| file(f, subtitle)).collect()),
            ));
        }
    }
    Value::Object(m).to_json()
}

/// A sidecar as the Admin API writes it: as its file holds it, compact.
pub(crate) fn sidecar_value(sd: &Sidecar) -> Value {
    akit_config::value::parse(&encode_sidecar(sd)).unwrap_or(Value::Null)
}

/// An item's sidecar as the console edits it (fit's `SidecarView`).
#[derive(Debug, Clone)]
pub struct SidecarView {
    /// The item has a sidecar.
    pub exists: bool,
    /// Where it is, or would be, under the root.
    pub path: String,
    /// When it was written.
    pub modified: Option<SystemTime>,
    pub(crate) sidecar: Sidecar,
    /// The media files, subtitles and pictures under the item's directory, relative to it.
    pub files: Vec<String>,
}

impl SidecarView {
    /// As the Admin API writes it.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut m = vec![
            ("exists".to_owned(), Value::Bool(self.exists)),
            ("path".to_owned(), Value::String(self.path.clone())),
        ];
        if let Some(t) = self.modified {
            m.push(("modified".to_owned(), time(t)));
        }
        m.push(("sidecar".to_owned(), sidecar_value(&self.sidecar)));
        m.push((
            "files".to_owned(),
            Value::Array(self.files.iter().cloned().map(Value::String).collect()),
        ));
        Value::Object(m)
    }
}

/// A sidecar read from a request's JSON object, as a scan reads one.
///
/// # Errors
///
/// Why it is not a sidecar the library reads.
pub(crate) fn sidecar_from(v: &Value) -> Result<Sidecar, FileError> {
    let text = super::catalog::compact(v);
    parse_sidecar(text.as_bytes()).map_err(FileError::BadSidecar)
}

/// Checks a new name of an item: a name a scan does not read as a rendition's label, a poster's
/// or a file's (fit's `checkBase`).
fn check_base(name: &str) -> Result<(), FileError> {
    if !valid_name(name) || name.len() > MAX_BASE_BYTES {
        return Err(FileError::BadPath);
    }
    let (b, label) = split_label(name);
    if !label.is_empty() || b != name || name.ends_with("-poster") || kind_of_name(name).is_some() {
        return Err(FileError::BadName(format!(
            "{name:?} reads as a file's name, a rendition's label or a poster's"
        )));
    }
    Ok(())
}

/// Checks that `f` is as the catalog has it, and that its directory lies inside its root (fit's
/// `checkSource`). It blocks.
fn check_source(f: &File, follow: bool) -> Result<(), FileError> {
    let md = std::fs::symlink_metadata(&f.abs).map_err(|e| gone(&e))?;
    if !md.file_type().is_symlink()
        && (md.len() != f.size || super::files::modified(&md) != f.modified)
    {
        return Err(FileError::Changed(f.path.clone()));
    }
    let parent = f.abs.parent().unwrap_or_else(|| Path::new("/"));
    let dir = std::fs::canonicalize(parent).map_err(|e| gone(&e))?;
    if !inside(&f.real, &dir) && !follow {
        return Err(FileError::Outside);
    }
    Ok(())
}

fn gone(e: &std::io::Error) -> FileError {
    if e.kind() == std::io::ErrorKind::NotFound {
        FileError::Changed("it is gone".to_owned())
    } else {
        FileError::io(e)
    }
}

impl Library {
    /// The item `id` and its root.
    fn item_root(&self, id: &str) -> Result<(Arc<Item>, Root), FileError> {
        if self.removed() {
            return Err(FileError::Removed);
        }
        let it = self.item(id).ok_or(FileError::NoItem)?;
        let r = self
            .settings()
            .roots
            .iter()
            .find(|r| r.conf == it.root)
            .cloned()
            // A reload removed its root: the next scan drops it.
            .ok_or(FileError::NoItem)?;
        Ok((it, r))
    }

    /// The ID the item will have at `rel`: its sidecar's, or `rel`'s.
    fn id_after(it: &Item, rel: &str) -> String {
        if it.named {
            it.id.clone()
        } else {
            path_id(rel)
        }
    }

    /// Renames the item `id` to `name` in its directory: each of its files there named after it,
    /// `bbb.720p.mp4`, `bbb.en.srt`, `bbb.jpg`, `bbb.json`, renamed `name.720p.mp4` and so on;
    /// files its sidecar names by other names keep them, and the sidecar is written again where
    /// it names one renamed. The item keeps its ID when its sidecar names one. On the pool;
    /// `done` there, or at once with an error found before.
    pub fn rename_item(
        self: &Arc<Self>,
        id: &str,
        name: &str,
        done: impl FnOnce(Result<Change, (FileError, Option<Box<Change>>)>) + Send + 'static,
    ) {
        let (it, r) = match self
            .item_root(id)
            .and_then(|v| check_base(name).map(|()| v))
        {
            Ok(v) => v,
            Err(e) => return done(Err((e, None))),
        };
        let dir = rel_dir(&it.path).to_owned();
        let base = base_of(&it.path).to_owned();
        if name == base {
            return done(Err((FileError::NoChange, None)));
        }
        let mut moves = Vec::new();
        let mut renamed: HashMap<String, String> = HashMap::new();
        for f in it.files() {
            let mut to = f.path.clone();
            let fname = base_of(&f.path);
            if rel_dir(&f.path) == dir
                && (fname.starts_with(&format!("{base}."))
                    || fname.starts_with(&format!("{base}-poster.")))
            {
                to = join(
                    &dir,
                    &format!("{name}{}", fname.get(base.len()..).unwrap_or("")),
                );
                if let Err(e) = clean_rel(&to, false) {
                    return done(Err((e, None)));
                }
                renamed.insert(f.path.clone(), to.clone());
            }
            moves.push(Move {
                f: f.clone(),
                to,
                to_abs: PathBuf::new(),
            });
        }
        let side_new = it
            .sidecar
            .as_ref()
            .and_then(|s| renamed.get(&s.path).cloned())
            .unwrap_or_default();
        let p = Plan {
            op: "rename",
            id: Self::id_after(&it, &join(&dir, name)),
            item: Arc::clone(&it),
            root: r,
            moves,
            side_new,
        };
        let after: After = Box::new(move |p: &Plan| rewrite_refs(p, &renamed).map(|()| Vec::new()));
        self.change(p, Some(after), Box::new(done));
    }

    /// Moves the item `id`, every file of it, to `dir`, a slash path under its root, "" for the
    /// root itself, each file keeping its path relative to the item's directory, so that what its
    /// sidecar names stays named; the directories are made when missing.
    pub fn move_item(
        self: &Arc<Self>,
        id: &str,
        dir: &str,
        done: impl FnOnce(Result<Change, (FileError, Option<Box<Change>>)>) + Send + 'static,
    ) {
        let (it, r, dir) = match self
            .item_root(id)
            .and_then(|(it, r)| clean_rel(dir, true).map(|d| (it, r, d)))
        {
            Ok(v) => v,
            Err(e) => return done(Err((e, None))),
        };
        let from = rel_dir(&it.path).to_owned();
        if dir == from {
            return done(Err((FileError::NoChange, None)));
        }
        let mut moves = Vec::new();
        for f in it.files() {
            let rel = if from.is_empty() {
                f.path.clone()
            } else {
                f.path
                    .strip_prefix(&format!("{from}/"))
                    .unwrap_or(&f.path)
                    .to_owned()
            };
            let to = join(&dir, &rel);
            if let Err(e) = clean_rel(&to, false) {
                return done(Err((e, None)));
            }
            moves.push(Move {
                f: f.clone(),
                to,
                to_abs: PathBuf::new(),
            });
        }
        let p = Plan {
            op: "move",
            id: Self::id_after(&it, &join(&dir, base_of(&it.path))),
            item: Arc::clone(&it),
            root: r,
            moves,
            side_new: String::new(),
        };
        self.change(p, None, Box::new(done));
    }

    /// Removes every file of the item `id`: its renditions, audio, subtitles, poster and sidecar.
    /// A file that cannot be removed stops it, and the change says what was removed before.
    pub fn delete_item(
        self: &Arc<Self>,
        id: &str,
        done: impl FnOnce(Result<Change, (FileError, Option<Box<Change>>)>) + Send + 'static,
    ) {
        let (it, r) = match self.item_root(id) {
            Ok(v) => v,
            Err(e) => return done(Err((e, None))),
        };
        let moves = it
            .files()
            .map(|f| Move {
                f: f.clone(),
                to: String::new(),
                to_abs: PathBuf::new(),
            })
            .collect();
        let p = Plan {
            op: "delete",
            id: String::new(),
            item: Arc::clone(&it),
            root: r,
            moves,
            side_new: String::new(),
        };
        self.change(p, None, Box::new(done));
    }

    /// Does `p` on the pool, one change at a time, then takes the item out of the catalog and asks
    /// for a scan.
    fn change(self: &Arc<Self>, p: Plan, after: Option<After>, done: Done) {
        let me = Arc::clone(self);
        let slot = Arc::new(std::sync::Mutex::new(Some(done)));
        let (s1, s2, s3) = (Arc::clone(&slot), Arc::clone(&slot), slot);
        let tell = |s: &Arc<std::sync::Mutex<Option<Done>>>, r| {
            if let Some(d) = lock(s).take() {
                d(r);
            }
        };
        let run = Box::new(move || {
            let r = {
                let _g = lock(&me.file_ops);
                me.do_plan(p, after)
            };
            let changed = match &r {
                Ok(c) => !c.files.is_empty(),
                Err((_, Some(c))) => !c.files.is_empty(),
                Err(_) => false,
            };
            if changed {
                let id = match &r {
                    Ok(c) => c.item.clone(),
                    Err((_, Some(c))) => c.item.clone(),
                    Err(_) => String::new(),
                };
                me.forget(&id);
                me.rescan();
            }
            tell(&s1, r);
        });
        let expire = Box::new(move || tell(&s2, Err((FileError::Busy, None))));
        if !self.submit(run, Some(expire)) {
            tell(&s3, Err((FileError::Busy, None)));
        }
    }

    /// Does `p`. It blocks.
    fn do_plan(
        &self,
        mut p: Plan,
        after: Option<After>,
    ) -> Result<Change, (FileError, Option<Box<Change>>)> {
        if self.removed() {
            return Err((FileError::Removed, None));
        }
        let mut c = Change {
            library: self.name().to_owned(),
            op: p.op,
            item: p.item.id.clone(),
            id: p.id.clone(),
            root: p.root.conf.clone(),
            files: Vec::new(),
        };
        let follow = self.settings().follow;
        for m in &mut p.moves {
            if let Err(e) = check_source(&m.f, follow) {
                return Err((with_path(e, &m.f.path), None));
            }
            if m.to.is_empty() || m.to == m.f.path {
                continue;
            }
            m.to_abs = writable(&p.root, &m.to).map_err(|e| (with_path(e, &m.to), None))?;
            match std::fs::symlink_metadata(&m.to_abs) {
                Ok(there) => {
                    // The file itself, on a system that folds case, is renamed in place.
                    let same = std::fs::symlink_metadata(&m.f.abs).is_ok_and(|src| {
                        use std::os::unix::fs::MetadataExt as _;
                        src.dev() == there.dev() && src.ino() == there.ino()
                    });
                    if !same {
                        return Err((FileError::Exists(m.to.clone()), None));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err((FileError::io(&e), None)),
            }
        }
        if p.op == "delete" {
            for m in &p.moves {
                if let Err(e) = std::fs::remove_file(&m.f.abs) {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        return Err((
                            FileError::Io(format!("{}: {e}", m.f.path)),
                            Some(Box::new(c)),
                        ));
                    }
                }
                self.tables.drop_file(&m.f.abs);
                c.files.push((m.f.path.clone(), String::new()));
            }
            return Ok(c);
        }
        let mut done: Vec<&Move> = Vec::new();
        let undo = |done: &[&Move]| {
            for m in done.iter().rev() {
                let _ = std::fs::rename(&m.to_abs, &m.f.abs);
            }
        };
        for m in &p.moves {
            if m.to.is_empty() || m.to == m.f.path {
                continue;
            }
            let r = m
                .to_abs
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::rename(&m.f.abs, &m.to_abs));
            if let Err(e) = r {
                undo(&done);
                return Err((FileError::Io(format!("{}: {e}", m.f.path)), None));
            }
            done.push(m);
        }
        let mut extra = Vec::new();
        if let Some(a) = after {
            match a(&p) {
                Ok(x) => extra = x,
                Err(e) => {
                    undo(&done);
                    return Err((e, None));
                }
            }
        }
        for m in &done {
            self.tables.drop_file(&m.f.abs);
            c.files.push((m.f.path.clone(), m.to.clone()));
        }
        c.files.extend(extra);
        Ok(c)
    }

    /// Takes the item `id` out of the catalog at once, and its sample tables with it, until a scan
    /// that starts later lists it as it is.
    pub(crate) fn forget(&self, id: &str) {
        let mut e = lock(&self.edits);
        e.0 = e.0.saturating_add(1);
        let at = e.0;
        e.1.insert(id.to_owned(), at);
        let cat = self.catalog().without(&[id.to_owned()]);
        self.cat.set(Some(Arc::new(cat)));
        drop(e);
        self.replaced.fire();
    }

    /// Reads the sidecar of the item `id`, or drafts one of what its files are named, on the pool.
    pub fn read_sidecar(
        self: &Arc<Self>,
        id: &str,
        done: impl FnOnce(Result<SidecarView, FileError>) + Send + 'static,
    ) {
        let (it, r) = match self.item_root(id) {
            Ok(v) => v,
            Err(e) => return done(Err(e)),
        };
        let me = Arc::clone(self);
        let slot = Arc::new(std::sync::Mutex::new(Some(done)));
        let (s1, s2, s3) = (Arc::clone(&slot), Arc::clone(&slot), slot);
        let run = Box::new(move || {
            let v = me.read_sidecar_now(&it, &r);
            if let Some(d) = lock(&s1).take() {
                d(v);
            }
        });
        let expire = Box::new(move || {
            if let Some(d) = lock(&s2).take() {
                d(Err(FileError::Busy));
            }
        });
        if !self.submit(run, Some(expire)) {
            if let Some(d) = lock(&s3).take() {
                d(Err(FileError::Busy));
            }
        }
    }

    fn read_sidecar_now(&self, it: &Item, r: &Root) -> Result<SidecarView, FileError> {
        let s = self.settings();
        let dir = rel_dir(&it.path);
        let mut v = SidecarView {
            exists: false,
            path: join(dir, &format!("{}.json", base_of(&it.path))),
            modified: None,
            sidecar: Sidecar::default(),
            files: Vec::new(),
        };
        match &it.sidecar {
            Some(f) => {
                let h = super::files::open_file(f, s.follow)
                    .map_err(|e| FileError::Io(e.to_string()))?;
                if h.size > s.sidecar_bytes {
                    return Err(FileError::TooLarge(format!(
                        "{} bytes, past max_sidecar_bytes",
                        h.size
                    )));
                }
                let len = usize::try_from(h.size).unwrap_or(0);
                let data = h.read_exact(0, len).map_err(|e| FileError::io(&e))?;
                v.sidecar = parse_sidecar(&data).map_err(FileError::BadSidecar)?;
                v.exists = true;
                v.path.clone_from(&f.path);
                v.modified = Some(f.modified);
            }
            None => v.sidecar = draft_sidecar(it),
        }
        v.files = list_files(&path_of(r, dir), MAX_SIDECAR_FILES);
        Ok(v)
    }

    /// Writes `sd` as the sidecar of the item `id`, replacing the one it has, or as
    /// `<name>.json` beside its files: whole or not at all, on the pool. `sd` is checked as a
    /// scan reads it; `modified`, when given, is the time of the sidecar the change was made
    /// from, and one written since is refused with [`FileError::Changed`].
    pub(crate) fn write_sidecar(
        self: &Arc<Self>,
        id: &str,
        sd: &Sidecar,
        modified: Option<SystemTime>,
        done: impl FnOnce(Result<Change, (FileError, Option<Box<Change>>)>) + Send + 'static,
    ) {
        let (it, r) = match self.item_root(id) {
            Ok(v) => v,
            Err(e) => return done(Err((e, None))),
        };
        let data = encode_sidecar(sd);
        let s = self.settings();
        let checked = (|| {
            let parsed = parse_sidecar(data.as_bytes()).map_err(FileError::BadSidecar)?;
            if data.len() as u64 > s.sidecar_bytes {
                return Err(FileError::TooLarge(format!(
                    "{} bytes, past max_sidecar_bytes, {}",
                    data.len(),
                    s.sidecar_bytes
                )));
            }
            if !parsed.id.is_empty() && parsed.id != it.id {
                if let Some(other) = self.item(&parsed.id) {
                    return Err(FileError::Exists(format!(
                        "the item {} has the ID {:?}",
                        other.path, parsed.id
                    )));
                }
            }
            if let Some(m) = modified {
                if it.sidecar.as_ref().is_none_or(|f| f.modified != m) {
                    return Err(FileError::Changed(
                        "the sidecar was written since".to_owned(),
                    ));
                }
            }
            Ok(parsed)
        })();
        let parsed = match checked {
            Ok(p) => p,
            Err(e) => return done(Err((e, None))),
        };
        let target = it.sidecar.as_ref().map_or_else(
            || join(rel_dir(&it.path), &format!("{}.json", base_of(&it.path))),
            |f| f.path.clone(),
        );
        let new_id = if parsed.id.is_empty() {
            path_id(&it.path)
        } else {
            parsed.id.clone()
        };
        let follow = s.follow;
        let side = it.sidecar.clone();
        let root = r.clone();
        let after: After = Box::new(move |_p: &Plan| {
            check_refs(&root, rel_dir(&target), &parsed)?;
            let abs = writable(&root, &target)?;
            match &side {
                Some(f) => check_source(f, follow)?,
                None => {
                    if std::fs::symlink_metadata(&abs).is_ok() {
                        return Err(FileError::Exists(target.clone()));
                    }
                }
            }
            write_atomic(&abs, data.as_bytes()).map_err(|e| FileError::io(&e))?;
            Ok(vec![(String::new(), target)])
        });
        let p = Plan {
            op: "sidecar",
            id: new_id,
            item: Arc::clone(&it),
            root: r,
            moves: Vec::new(),
            side_new: String::new(),
        };
        self.change(p, Some(after), Box::new(done));
    }

    /// The directories under the root the config writes as `root`, as slash paths, sorted, those
    /// starting with '.' and those `exclude` leaves out skipped, links not followed, at most
    /// [`MAX_DIRS`]: where an item may move to. On the pool.
    pub fn dirs(
        self: &Arc<Self>,
        root: &str,
        done: impl FnOnce(Result<Vec<String>, FileError>) + Send + 'static,
    ) {
        let s = self.settings();
        if self.removed() {
            return done(Err(FileError::Removed));
        }
        let Some(r) = s.roots.iter().find(|r| r.conf == root).cloned() else {
            return done(Err(FileError::NoRoot(root.to_owned())));
        };
        let slot = Arc::new(std::sync::Mutex::new(Some(done)));
        let (s1, s2, s3) = (Arc::clone(&slot), Arc::clone(&slot), slot);
        let me = Arc::clone(self);
        let run = Box::new(move || {
            let s = me.settings();
            let mut out = Vec::new();
            walk_dirs(&r.abs, "", 0, &s, &mut out);
            out.sort();
            if let Some(d) = lock(&s1).take() {
                d(Ok(out));
            }
        });
        let expire = Box::new(move || {
            if let Some(d) = lock(&s2).take() {
                d(Err(FileError::Busy));
            }
        });
        if !self.submit(run, Some(expire)) {
            if let Some(d) = lock(&s3).take() {
                d(Err(FileError::Busy));
            }
        }
    }
}

/// The error of `e`, of the file at `path`.
fn with_path(e: FileError, path: &str) -> FileError {
    match e {
        FileError::Io(m) => FileError::Io(format!("{path}: {m}")),
        FileError::Changed(m) if m == "it is gone" => FileError::Changed(format!("{path}: {m}")),
        FileError::Changed(_) => FileError::Changed(path.to_owned()),
        other => other,
    }
}

fn walk_dirs(abs: &Path, rel: &str, depth: usize, s: &super::Settings, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(abs) else {
        return;
    };
    let mut ents: Vec<_> = rd.flatten().collect();
    ents.sort_by_key(std::fs::DirEntry::file_name);
    for e in ents {
        let name = e.file_name().to_string_lossy().into_owned();
        let p = join(rel, &name);
        let dir = e.file_type().is_ok_and(|t| t.is_dir());
        if out.len() >= MAX_DIRS
            || !dir
            || name.starts_with('.')
            || s.exclude.iter().any(|x| x.matches(&p))
        {
            continue;
        }
        out.push(p.clone());
        if depth.saturating_add(1) < super::MAX_DEPTH {
            walk_dirs(&e.path(), &p, depth.saturating_add(1), s, out);
        }
    }
}

/// The media files, subtitles and pictures under `dir`, at most `n`, as slash paths relative to
/// it, sorted, names starting with '.' skipped (fit's `listFiles`). It blocks.
fn list_files(dir: &Path, n: usize) -> Vec<String> {
    fn walk(abs: &Path, rel: &str, depth: usize, n: usize, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(abs) else {
            return;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if out.len() >= n || name.starts_with('.') {
                continue;
            }
            let p = join(rel, &name);
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() && depth < super::MAX_DEPTH {
                walk(&e.path(), &p, depth.saturating_add(1), n, out);
            } else if t.is_file()
                && matches!(
                    kind_of_name(&name),
                    Some(Kind::Media | Kind::Subtitle | Kind::Image)
                )
            {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, "", 0, n, &mut out);
    out.sort();
    out
}

/// The sidecar of what an item's files are named: its title and files with their names and
/// languages (fit's `draftSidecar`).
fn draft_sidecar(it: &Item) -> Sidecar {
    let dir = rel_dir(&it.path);
    let rel = |p: &str| {
        if dir.is_empty() {
            p.to_owned()
        } else {
            p.strip_prefix(&format!("{dir}/")).unwrap_or(p).to_owned()
        }
    };
    let file = |r: &super::catalog::Rendition| SidecarFile {
        file: rel(&r.file.path),
        name: r.name.clone(),
        language: r.language.clone(),
        default: r.default,
        forced: false,
    };
    Sidecar {
        id: String::new(),
        title: it.title.clone(),
        poster: it.poster.as_ref().map_or(String::new(), |p| rel(&p.path)),
        renditions: it.renditions.iter().map(file).collect(),
        audio: it.audio.iter().map(file).collect(),
        subtitles: it
            .subtitles
            .iter()
            .map(|s| SidecarFile {
                file: rel(&s.file.path),
                name: s.name.clone(),
                language: s.language.clone(),
                default: s.default,
                forced: s.forced,
            })
            .collect(),
    }
}

/// Writes the item's sidecar again, at its new path, the files renamed (by their old paths under
/// the root) named anew; nothing when it names none of them (fit's `rewriteRefs`). It blocks.
fn rewrite_refs(p: &Plan, renamed: &HashMap<String, String>) -> Result<(), FileError> {
    if p.side_new.is_empty() {
        return Ok(());
    }
    let abs = path_of(&p.root, &p.side_new);
    let data = std::fs::read(&abs).map_err(|e| FileError::io(&e))?;
    let mut sd = parse_sidecar(&data).map_err(FileError::BadSidecar)?;
    let from = rel_dir(&p.item.path).to_owned();
    let to = rel_dir(&p.side_new).to_owned();
    let mut changed = false;
    let mut fix = |r: &mut String| {
        if r.is_empty() {
            return;
        }
        if let Some(rel) = under_dir(&from, r) {
            if let Some(n) = renamed.get(&rel) {
                *r = if to.is_empty() {
                    n.clone()
                } else {
                    n.strip_prefix(&format!("{to}/")).unwrap_or(n).to_owned()
                };
                changed = true;
            }
        }
    };
    fix(&mut sd.poster);
    for f in sd
        .renditions
        .iter_mut()
        .chain(sd.audio.iter_mut())
        .chain(sd.subtitles.iter_mut())
    {
        fix(&mut f.file);
    }
    if !changed {
        return Ok(());
    }
    write_atomic(&abs, encode_sidecar(&sd).as_bytes()).map_err(|e| FileError::io(&e))
}

/// Checks that each file `sd` names, relative to `dir` under `r`, is there, inside `r`, and of
/// the kind it is named as (fit's `checkRefs`). It blocks.
fn check_refs(r: &Root, dir: &str, sd: &Sidecar) -> Result<(), FileError> {
    let real = std::fs::canonicalize(&r.abs).map_err(|e| FileError::io(&e))?;
    let check = |field: &str, file: &str, want: Kind| -> Result<(), FileError> {
        let rel = under_dir(dir, file).ok_or_else(|| {
            FileError::BadSidecar(format!(
                "{field} {file:?} leads out of the sidecar's directory"
            ))
        })?;
        if kind_of_name(&rel) != Some(want) {
            return Err(FileError::BadSidecar(format!(
                "{field} {file:?} is not a {}",
                super::scan::kind_name(want)
            )));
        }
        let t = std::fs::canonicalize(path_of(r, &rel))
            .map_err(|_| FileError::BadSidecar(format!("{field} {file:?} is not there")))?;
        let regular = std::fs::metadata(&t).is_ok_and(|m| m.is_file());
        if !regular || !inside(&real, &t) {
            return Err(FileError::BadSidecar(format!(
                "{field} {file:?} is not a file inside the root"
            )));
        }
        Ok(())
    };
    if !sd.poster.is_empty() {
        check("poster", &sd.poster, Kind::Image)?;
    }
    for f in sd.renditions.iter().chain(&sd.audio) {
        check("file", &f.file, Kind::Media)?;
    }
    for f in &sd.subtitles {
        check("file", &f.file, Kind::Subtitle)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bases() {
        assert!(check_base("sintel").is_ok());
        for bad in ["", "a.mp4", "bbb.720p", "a-poster", ".x", "a/b"] {
            assert!(check_base(bad).is_err(), "{bad:?}");
        }
    }
}
