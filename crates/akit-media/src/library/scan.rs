//! A scan of a library (fit's `medialib/scan.go`): its roots walked, the files grouped into
//! items by sidecar or by name, the media files probed with akio-media's MP4 reader, and the
//! catalog built, each step bounded. A scan runs on the library's scanner thread; its probes on
//! at most `max_probes` threads of its own, so that the file pool is left to the files served
//! (decision 043).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::{Duration, SystemTime};

use akio_media::mp4::{Limits, Locator, Movie, Mp4Error};

use super::catalog::{
    Catalog, File, Item, KIND_AUDIO, KIND_VIDEO, MAX_PROBLEMS, Problem, Rendition, Subtitle, Track,
    codec_name, kind_of, path_id,
};
use super::files::{Sidecar, inside, modified, open_file, read_sidecar};
use super::{Error, Library, Settings, Status};

/// How deep under its root a scan walks directories.
pub const MAX_DEPTH: usize = 32;
/// The bytes of a file's start read to find its `moov`.
const HEAD_READ: usize = 64 << 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Media,
    Subtitle,
    Image,
    Sidecar,
}

pub(crate) fn kind_of_name(name: &str) -> Option<Kind> {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)?;
    match ext.as_str() {
        "mp4" | "m4v" | "m4a" | "mov" => Some(Kind::Media),
        "vtt" | "srt" => Some(Kind::Subtitle),
        "jpg" | "jpeg" | "png" | "webp" => Some(Kind::Image),
        "json" => Some(Kind::Sidecar),
        _ => None,
    }
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Media => "media file",
        Kind::Subtitle => "subtitle",
        Kind::Image => "picture",
        Kind::Sidecar => "sidecar",
    }
}

/// A file the scan found.
#[derive(Debug)]
struct Entry {
    root: usize,
    real: PathBuf,
    rel: String,
    abs: PathBuf,
    size: u64,
    modified: SystemTime,
    kind: Kind,
    claimed: bool,
}

/// A file of a group: a rendition, audio, or a subtitle, with what the sidecar or its name
/// says of it.
#[derive(Debug, Default)]
struct Member {
    entry: usize,
    name: String,
    language: String,
    default: bool,
    forced: bool,
    audio: bool,
    probed: Option<Rendition>,
}

/// The files of an item, before they are probed.
#[derive(Debug, Default)]
struct Group {
    root: usize,
    dir: String,
    base: String,
    side: Option<(Sidecar, usize)>,
    media: Vec<Member>,
    subs: Vec<Member>,
    poster: Option<usize>,
}

impl Group {
    fn path(&self) -> String {
        join(&self.dir, &self.base)
    }
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// A slash path's directory and its name without its extension.
fn split(rel: &str) -> (&str, &str) {
    let (dir, name) = rel.rsplit_once('/').unwrap_or(("", rel));
    let stem = match name.rfind('.') {
        Some(i) => name.get(..i).unwrap_or(name),
        None => name,
    };
    (dir, stem)
}

/// Whether `label`, the last part of a media file's name before its extension, names a
/// rendition (fit's `renditionLabel`): `720p`, `1080p60`, `4k`, `800k`, `800kbps`, `audio-fr`.
fn rendition_label(label: &str) -> bool {
    let digits = label.bytes().take_while(u8::is_ascii_digit).count();
    let rest = label.get(digits..).unwrap_or("");
    if (3..=4).contains(&digits) {
        if let Some(fps) = rest.strip_prefix('p') {
            return fps.len() <= 3 && fps.bytes().all(|b| b.is_ascii_digit());
        }
    }
    if digits > 0 && (rest == "k" || rest == "kbps") {
        return true;
    }
    match label.strip_prefix("audio") {
        Some(r) => {
            r.is_empty()
                || r.strip_prefix('-').is_some_and(|r| {
                    r.split('-')
                        .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric()))
                })
        }
        None => false,
    }
}

/// Whether `part` of a subtitle's name names its language (fit's `subtitleLanguage`): `en`,
/// `pt-BR`, `zho`.
fn subtitle_language(part: &str) -> bool {
    let (lang, region) = part.split_once('-').unwrap_or((part, ""));
    (2..=3).contains(&lang.len())
        && lang.bytes().all(|b| b.is_ascii_lowercase())
        && (!part.contains('-')
            || ((2..=8).contains(&region.len())
                && region.bytes().all(|b| b.is_ascii_alphanumeric())))
}

/// A media file's stem split into the item's base and the rendition's label.
fn split_label(stem: &str) -> (&str, &str) {
    if let Some(i) = stem.rfind('.') {
        if i > 0 {
            let label = stem.get(i.saturating_add(1)..).unwrap_or("");
            if rendition_label(label) {
                return (stem.get(..i).unwrap_or(stem), label);
            }
        }
    }
    (stem, "")
}

/// The language an audio rendition's label names: `fr` of `audio-fr`.
fn label_language(label: &str) -> String {
    label
        .strip_prefix("audio-")
        .map(|rest| rest.split('-').next().unwrap_or(""))
        .filter(|first| subtitle_language(first))
        .unwrap_or("")
        .to_owned()
}

/// The path under its root of `file`, a sidecar's slash path relative to its directory `dir`;
/// `None` when it leads out of `dir` or is not a plain relative path.
fn under_dir(dir: &str, file: &str) -> Option<String> {
    if file.is_empty() || file.starts_with('/') || file.contains(['\\', '\0']) {
        return None;
    }
    let clean = akit_config::medialib::clean(Path::new(file));
    let clean = clean.to_str()?;
    if clean == "." || clean == ".." || clean.starts_with("../") {
        return None;
    }
    Some(join(dir, clean))
}

/// One scan of a library.
pub(crate) struct Scanner<'l> {
    lib: &'l Library,
    s: &'l Settings,
    entries: Vec<Entry>,
    files: usize,
    problems: Vec<Problem>,
    more: usize,
    trunc: String,
}

struct DirTask {
    abs: PathBuf,
    real: PathBuf,
    rel: String,
    depth: usize,
}

impl<'l> Scanner<'l> {
    pub(crate) fn new(lib: &'l Library, s: &'l Settings) -> Scanner<'l> {
        Scanner {
            lib,
            s,
            entries: Vec::new(),
            files: 0,
            problems: Vec::new(),
            more: 0,
            trunc: String::new(),
        }
    }

    fn problem(&mut self, root: Option<usize>, rel: &str, error: String) {
        if self.problems.len() >= MAX_PROBLEMS {
            self.more = self.more.saturating_add(1);
            return;
        }
        let root = root
            .and_then(|r| self.s.roots.get(r))
            .map(|r| r.conf.clone())
            .unwrap_or_default();
        self.problems.push(Problem {
            root,
            path: rel.to_owned(),
            error,
        });
    }

    fn truncate(&mut self, why: String) {
        if self.trunc.is_empty() {
            self.trunc = why;
        }
    }

    fn progress(&self, f: impl FnOnce(&mut Status)) {
        let mut st = self
            .lib
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut st);
    }

    /// Makes the catalog, from `prev`'s probes of the files unchanged; `None` when the library
    /// is removed meanwhile.
    pub(crate) fn run(mut self, prev: &Catalog) -> Option<Catalog> {
        for r in 0..self.s.roots.len() {
            self.walk(r);
            if self.lib.stopped() {
                return None;
            }
        }
        let files = self.files;
        self.progress(|st| st.files = files);
        let sides = self.read_sidecars();
        let mut groups = self.group(sides);
        if !self.probe(&mut groups, prev) {
            return None;
        }
        Some(self.build(groups))
    }

    /// Lists the files of root `r` that the library looks at.
    fn walk(&mut self, r: usize) {
        let Some(root) = self.s.roots.get(r) else {
            return;
        };
        let abs = root.abs.clone();
        let real = match fs::canonicalize(&abs).and_then(|real| {
            if fs::metadata(&real)?.is_dir() {
                Ok(real)
            } else {
                Err(std::io::Error::other("not a directory"))
            }
        }) {
            Ok(real) => real,
            Err(e) => {
                self.problem(Some(r), "", format!("the root: {e}"));
                return;
            }
        };
        if !self.trunc.is_empty() {
            return;
        }
        let mut stack = vec![DirTask {
            abs,
            real: real.clone(),
            rel: String::new(),
            depth: 0,
        }];
        let mut visited: HashSet<PathBuf> = HashSet::from([real.clone()]);
        while let Some(d) = stack.pop() {
            if !self.trunc.is_empty() || self.lib.stopped() {
                return;
            }
            let Some(names) = self.read_dir(r, &d) else {
                return;
            };
            for (name, ft) in names {
                if name.starts_with('.') {
                    continue;
                }
                let rel = join(&d.rel, &name);
                let abs = d.abs.join(&name);
                let (target, meta) = if ft.is_symlink() {
                    let got = fs::canonicalize(&abs).and_then(|t| fs::metadata(&t).map(|m| (t, m)));
                    match got {
                        Err(_) => {
                            self.problem(
                                Some(r),
                                &rel,
                                "a symbolic link that leads nowhere".into(),
                            );
                            continue;
                        }
                        Ok((t, _)) if !self.s.follow && !inside(&real, &t) => {
                            self.problem(
                                Some(r),
                                &rel,
                                "a symbolic link out of the root, refused: follow_symlinks follows it"
                                    .into(),
                            );
                            continue;
                        }
                        Ok(tm) => tm,
                    }
                } else {
                    match fs::symlink_metadata(&abs) {
                        Ok(m) => (d.real.join(&name), m),
                        // Removed since it was listed.
                        Err(_) => continue,
                    }
                };
                if meta.is_dir() {
                    let excluded = self.s.exclude.iter().any(|p| p.matches(&rel));
                    if excluded || visited.contains(&target) {
                        continue;
                    }
                    if d.depth.saturating_add(1) > MAX_DEPTH {
                        self.problem(
                            Some(r),
                            &rel,
                            format!(
                                "deeper than {MAX_DEPTH} directories under the root: not walked"
                            ),
                        );
                        continue;
                    }
                    visited.insert(target.clone());
                    stack.push(DirTask {
                        abs,
                        real: target,
                        rel,
                        depth: d.depth.saturating_add(1),
                    });
                } else if meta.is_file() {
                    if let Some(kind) = kind_of_name(&name) {
                        if self.s.included(&rel) {
                            self.entries.push(Entry {
                                root: r,
                                real: real.clone(),
                                rel,
                                abs,
                                size: meta.len(),
                                modified: modified(&meta),
                                kind,
                                claimed: false,
                            });
                        }
                    }
                }
            }
        }
    }

    /// `d`'s entries, sorted by name, each counted against `max_files`; `None` once it is
    /// passed, which ends the walk.
    fn read_dir(&mut self, r: usize, d: &DirTask) -> Option<Vec<(String, fs::FileType)>> {
        let it = match fs::read_dir(&d.abs) {
            Ok(it) => it,
            Err(e) => {
                self.problem(Some(r), &d.rel, e.to_string());
                return Some(Vec::new());
            }
        };
        let mut out = Vec::new();
        for e in it {
            self.files = self.files.saturating_add(1);
            if self.files > self.s.max_files {
                let conf = self
                    .s
                    .roots
                    .get(r)
                    .map(|r| r.conf.clone())
                    .unwrap_or_default();
                self.truncate(format!(
                    "more than max_files, {}, files and directories: the scan stopped at {}",
                    self.s.max_files,
                    join(&conf, &d.rel).trim_end_matches('/')
                ));
                return None;
            }
            match e {
                Ok(e) => {
                    let Ok(ft) = e.file_type() else { continue };
                    let Ok(name) = e.file_name().into_string() else {
                        continue;
                    };
                    out.push((name, ft));
                }
                Err(e) => {
                    self.problem(Some(r), &d.rel, e.to_string());
                    break;
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Some(out)
    }

    fn file(&self, i: usize) -> Option<File> {
        let e = self.entries.get(i)?;
        let root = self.s.roots.get(e.root)?;
        Some(File {
            root: root.conf.clone(),
            path: e.rel.clone(),
            size: e.size,
            modified: e.modified,
            abs: e.abs.clone(),
            real: e.real.clone(),
        })
    }

    /// Reads and parses the sidecars found, each bounded by `max_sidecar_bytes` and all
    /// together by `max_catalog_bytes`; one that does not parse is a problem.
    fn read_sidecars(&mut self) -> HashMap<usize, Sidecar> {
        let mut total = 0u64;
        let mut list = Vec::new();
        let mut late: Vec<(usize, String, String)> = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            if e.kind != Kind::Sidecar {
                continue;
            }
            if e.size > self.s.sidecar_bytes {
                late.push((
                    e.root,
                    e.rel.clone(),
                    format!(
                        "a sidecar of {} bytes, past max_sidecar_bytes, {}: left out",
                        e.size, self.s.sidecar_bytes
                    ),
                ));
            } else if total.saturating_add(e.size) > self.s.catalog_bytes {
                if self.trunc.is_empty() {
                    let conf = self
                        .s
                        .roots
                        .get(e.root)
                        .map(|r| r.conf.as_str())
                        .unwrap_or("");
                    self.trunc = format!(
                        "the sidecars pass max_catalog_bytes, {}: {} and those after it are left out",
                        self.s.catalog_bytes,
                        join(conf, &e.rel)
                    );
                }
            } else {
                total = total.saturating_add(e.size);
                list.push(i);
            }
        }
        for (r, rel, msg) in late {
            self.problem(Some(r), &rel, msg);
        }
        let mut out = HashMap::new();
        for i in list {
            let Some(f) = self.file(i) else { continue };
            match read_sidecar(&f, self.s.follow) {
                Ok(sd) => {
                    out.insert(i, sd);
                }
                Err(e) => {
                    let (r, rel) = self
                        .entries
                        .get(i)
                        .map(|e| (e.root, e.rel.clone()))
                        .unwrap_or_default();
                    self.problem(Some(r), &rel, format!("the sidecar: {e}"));
                }
            }
        }
        out
    }

    /// Gathers the files found into items: what sidecars name, then media files by their names'
    /// base and rendition label, then subtitles and posters named after an item's base.
    fn group(&mut self, mut sides: HashMap<usize, Sidecar>) -> Vec<Group> {
        let mut by_rel: HashMap<(usize, String), usize> = HashMap::new();
        for (i, e) in self.entries.iter().enumerate() {
            by_rel.insert((e.root, e.rel.clone()), i);
        }
        let mut groups: Vec<Group> = Vec::new();
        let mut index: HashMap<(usize, String, String), usize> = HashMap::new();
        let mut get = |groups: &mut Vec<Group>, root: usize, dir: &str, base: &str| -> usize {
            *index
                .entry((root, dir.to_owned(), base.to_owned()))
                .or_insert_with(|| {
                    groups.push(Group {
                        root,
                        dir: dir.to_owned(),
                        base: base.to_owned(),
                        ..Group::default()
                    });
                    groups.len().saturating_sub(1)
                })
        };
        // Sidecars first, so that the files they name are theirs.
        for i in 0..self.entries.len() {
            let Some(sd) = sides.remove(&i) else { continue };
            let Some((root, rel)) = self.entries.get(i).map(|e| (e.root, e.rel.clone())) else {
                continue;
            };
            let (dir, base) = split(&rel);
            let (dir, base) = (dir.to_owned(), base.to_owned());
            let g = get(&mut groups, root, &dir, &base);
            let mut problems = Vec::new();
            let mut claim = |field: &str, file: &str, want: Kind, entries: &mut Vec<Entry>| {
                let Some(target) = under_dir(&dir, file) else {
                    problems.push(format!(
                        "{field} {file:?} leads out of the sidecar's directory"
                    ));
                    return None;
                };
                let Some(&j) = by_rel.get(&(root, target)) else {
                    problems.push(format!(
                        "{field} {file:?} is not a file the library looks at"
                    ));
                    return None;
                };
                let e = entries.get_mut(j)?;
                if e.kind != want {
                    problems.push(format!("{field} {file:?} is not a {}", kind_name(want)));
                    return None;
                }
                if e.claimed {
                    problems.push(format!("{field} {file:?} is named by a sidecar already"));
                    return None;
                }
                e.claimed = true;
                Some(j)
            };
            let mut media = Vec::new();
            for (audio, list) in [(false, &sd.renditions), (true, &sd.audio)] {
                for m in list {
                    if let Some(j) = claim("file", &m.file, Kind::Media, &mut self.entries) {
                        media.push(Member {
                            entry: j,
                            name: m.name.clone(),
                            language: m.language.clone(),
                            default: m.default,
                            audio,
                            ..Member::default()
                        });
                    }
                }
            }
            let mut subs = Vec::new();
            for s in &sd.subtitles {
                if let Some(j) = claim("file", &s.file, Kind::Subtitle, &mut self.entries) {
                    subs.push(Member {
                        entry: j,
                        name: s.name.clone(),
                        language: s.language.clone(),
                        default: s.default,
                        forced: s.forced,
                        ..Member::default()
                    });
                }
            }
            let poster = if sd.poster.is_empty() {
                None
            } else {
                claim("poster", &sd.poster, Kind::Image, &mut self.entries)
            };
            for p in problems {
                self.problem(Some(root), &rel, p);
            }
            if let Some(g) = groups.get_mut(g) {
                g.side = Some((sd, i));
                g.media.extend(media);
                g.subs.extend(subs);
                if poster.is_some() {
                    g.poster = poster;
                }
            }
        }
        for i in 0..self.entries.len() {
            let Some(e) = self.entries.get(i) else {
                continue;
            };
            if e.kind != Kind::Media || e.claimed {
                continue;
            }
            let (dir, stem) = split(&e.rel);
            let (base, label) = split_label(stem);
            let root = e.root;
            let (dir, base, label) = (dir.to_owned(), base.to_owned(), label.to_owned());
            let g = get(&mut groups, root, &dir, &base);
            if let Some(g) = groups.get_mut(g) {
                g.media.push(Member {
                    entry: i,
                    audio: label.starts_with("audio"),
                    language: label_language(&label),
                    name: label,
                    ..Member::default()
                });
            }
        }
        let mut by_dir: HashMap<(usize, String), Vec<usize>> = HashMap::new();
        for (gi, g) in groups.iter().enumerate() {
            by_dir.entry((g.root, g.dir.clone())).or_default().push(gi);
        }
        for i in 0..self.entries.len() {
            let Some(e) = self.entries.get(i) else {
                continue;
            };
            if e.claimed || !matches!(e.kind, Kind::Subtitle | Kind::Image) {
                continue;
            }
            let (dir, stem) = split(&e.rel);
            let image = e.kind == Kind::Image;
            let mut best: Option<usize> = None;
            for &gi in by_dir.get(&(e.root, dir.to_owned())).into_iter().flatten() {
                let Some(c) = groups.get(gi) else { continue };
                let named = stem == c.base
                    || stem.starts_with(&format!("{}.", c.base))
                    || (image && stem == format!("{}-poster", c.base));
                let longer = best
                    .and_then(|b| groups.get(b))
                    .is_none_or(|b| c.base.len() > b.base.len());
                if named && longer {
                    best = Some(gi);
                }
            }
            let Some(g) = best.and_then(|b| groups.get_mut(b)) else {
                continue;
            };
            if image {
                let poster_name = stem == g.base
                    || stem == format!("{}.poster", g.base)
                    || stem == format!("{}-poster", g.base);
                if g.poster.is_none() && poster_name {
                    g.poster = Some(i);
                }
                continue;
            }
            let mut m = Member {
                entry: i,
                ..Member::default()
            };
            let rest = stem.get(g.base.len()..).unwrap_or("");
            for part in rest.strip_prefix('.').unwrap_or(rest).split('.') {
                match part {
                    "" => {}
                    "forced" => m.forced = true,
                    "default" => m.default = true,
                    p if m.language.is_empty() && subtitle_language(p) => {
                        p.clone_into(&mut m.language);
                    }
                    p => p.clone_into(&mut m.name),
                }
            }
            g.subs.push(m);
        }
        groups
    }

    /// Probes the media files of `groups`, those unchanged since `prev` reused as `prev` found
    /// them; a file that changes while it is probed is probed again, once, and listed with that
    /// error if it changes again. `false` when the library is removed meanwhile.
    fn probe(&mut self, groups: &mut [Group], prev: &Catalog) -> bool {
        let mut old: HashMap<&Path, &Rendition> = HashMap::new();
        for it in &prev.items {
            for r in it.renditions.iter().chain(&it.audio) {
                if !r.transient {
                    old.insert(&r.file.abs, r);
                }
            }
        }
        let mut todo: Vec<(usize, usize)> = Vec::new();
        let mut reused = 0usize;
        for (gi, g) in groups.iter_mut().enumerate() {
            for (mi, m) in g.media.iter_mut().enumerate() {
                let Some(e) = self.entries.get(m.entry) else {
                    continue;
                };
                let conf = self
                    .s
                    .roots
                    .get(e.root)
                    .map(|r| r.conf.as_str())
                    .unwrap_or("");
                match old.get(e.abs.as_path()) {
                    Some(o)
                        if o.file.size == e.size
                            && o.file.modified == e.modified
                            && o.file.root == conf
                            && o.file.path == e.rel =>
                    {
                        m.probed = Some((*o).clone());
                        reused = reused.saturating_add(1);
                    }
                    _ => todo.push((gi, mi)),
                }
            }
        }
        let n = todo.len();
        self.progress(|st| {
            st.probe = n;
            st.reused = reused;
        });
        for round in 0..2 {
            if todo.is_empty() {
                break;
            }
            if round > 0 {
                let n = todo.len();
                self.progress(|st| st.probe = st.probe.saturating_add(n));
            }
            let files: Vec<Option<File>> = todo
                .iter()
                .map(|&(gi, mi)| {
                    let e = groups.get(gi)?.media.get(mi)?.entry;
                    self.file(e)
                })
                .collect();
            let results = self.probe_all(&files);
            if self.lib.stopped() {
                return false;
            }
            let mut again = Vec::new();
            for ((gi, mi), res) in todo.into_iter().zip(results) {
                let Some(m) = groups.get_mut(gi).and_then(|g| g.media.get_mut(mi)) else {
                    continue;
                };
                let Some(r) = res else { continue };
                if r.transient && round == 0 {
                    // As it is now, for the second try.
                    if let Some(e) = self.entries.get_mut(m.entry) {
                        if let Ok(meta) = fs::metadata(&e.abs) {
                            e.size = meta.len();
                            e.modified = modified(&meta);
                        }
                    }
                    again.push((gi, mi));
                }
                m.probed = Some(r);
            }
            todo = again;
        }
        true
    }

    /// Probes `files` on at most `max_probes` threads.
    fn probe_all(&self, files: &[Option<File>]) -> Vec<Option<Rendition>> {
        let out: Vec<Mutex<Option<Rendition>>> = files.iter().map(|_| Mutex::new(None)).collect();
        let next = AtomicUsize::new(0);
        let threads = self.s.probes.min(files.len()).max(1);
        let limits = Limits::default();
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Relaxed);
                        let Some(f) = files.get(i) else { return };
                        if self.lib.stopped() {
                            return;
                        }
                        let Some(f) = f else { continue };
                        if let Some(h) = self.lib.probe_hook() {
                            h();
                        }
                        let r = probe_file(f, self.s.follow, &limits);
                        self.lib.counters.probed.fetch_add(1, Relaxed);
                        if !r.error.is_empty() {
                            self.lib.counters.probe_errors.fetch_add(1, Relaxed);
                        }
                        self.progress(|st| st.probed = st.probed.saturating_add(1));
                        if let Some(slot) = out.get(i) {
                            *slot
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(r);
                        }
                    }
                });
            }
        });
        out.into_iter()
            .map(|m| {
                m.into_inner()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
            })
            .collect()
    }

    /// The catalog of `groups`: their items in the order of the roots and their paths, an ID
    /// taken twice left out, until `max_items` or `max_catalog_bytes`, then sorted by ID.
    fn build(mut self, mut groups: Vec<Group>) -> Catalog {
        groups.sort_by(|a, b| a.root.cmp(&b.root).then_with(|| a.path().cmp(&b.path())));
        let mut cat = Catalog::default();
        let mut taken: HashMap<String, String> = HashMap::new();
        for g in groups {
            let conf = self
                .s
                .roots
                .get(g.root)
                .map(|r| r.conf.clone())
                .unwrap_or_default();
            let mut it = self.item(g);
            let at = join(&conf, &it.path);
            if let Some(w) = taken.get(&it.id) {
                let msg = format!("the ID {:?} is taken by {w}: left out", it.id);
                let (root, path) = (self.root_index(&it.root), it.path.clone());
                self.problem(root, &path, msg);
                continue;
            }
            if cat.items.len() >= self.s.max_items {
                self.truncate(format!(
                    "more than max_items, {}, items: {at} and those after it are left out",
                    self.s.max_items
                ));
                break;
            }
            let n = it.reckon();
            if cat.bytes.saturating_add(n) > self.s.catalog_bytes {
                self.truncate(format!(
                    "the items pass max_catalog_bytes, {}: {at} and those after it are left out",
                    self.s.catalog_bytes
                ));
                break;
            }
            cat.bytes = cat.bytes.saturating_add(n);
            taken.insert(it.id.clone(), at);
            let it = std::sync::Arc::new(it);
            cat.by_id.insert(it.id.clone(), std::sync::Arc::clone(&it));
            cat.items.push(it);
        }
        cat.items.sort_by(|a, b| a.id.cmp(&b.id));
        cat.problems = self.problems;
        cat.more_problems = self.more;
        cat.truncated = self.trunc;
        cat
    }

    fn root_index(&self, conf: &str) -> Option<usize> {
        self.s.roots.iter().position(|r| r.conf == conf)
    }

    /// The item of `g`, its files probed.
    fn item(&self, g: Group) -> Item {
        let conf = self
            .s
            .roots
            .get(g.root)
            .map(|r| r.conf.clone())
            .unwrap_or_default();
        let path = g.path();
        let mut it = Item {
            id: String::new(),
            title: g.base.clone(),
            root: conf,
            path,
            duration: Duration::ZERO,
            size: 0,
            modified: SystemTime::UNIX_EPOCH,
            renditions: Vec::new(),
            audio: Vec::new(),
            subtitles: Vec::new(),
            poster: None,
            sidecar: None,
            error: String::new(),
            bytes: 0,
            named: false,
        };
        if let Some((sd, i)) = &g.side {
            it.id.clone_from(&sd.id);
            if !sd.title.is_empty() {
                it.title.clone_from(&sd.title);
            }
            it.named = !sd.id.is_empty();
            it.sidecar = self.file(*i);
        }
        if it.id.is_empty() {
            it.id = path_id(&it.path);
        }
        let add = |it: &mut Item, f: &File| {
            it.size = it.size.saturating_add(f.size);
            if f.modified > it.modified {
                it.modified = f.modified;
            }
        };
        let mut first_err = String::new();
        for m in g.media {
            let Some(mut r) = m.probed else { continue };
            r.name = m.name;
            r.default = m.default;
            r.language = if m.language.is_empty() {
                first_language(&r)
            } else {
                m.language
            };
            add(&mut it, &r.file.clone());
            if !r.error.is_empty() && first_err.is_empty() {
                first_err.clone_from(&r.error);
            }
            if m.audio || (r.error.is_empty() && !r.has_video()) {
                it.audio.push(r);
            } else {
                it.renditions.push(r);
            }
        }
        let mut ok = false;
        for list in [&it.renditions, &it.audio] {
            for r in list {
                if r.error.is_empty() && !ok {
                    it.duration = it.duration.max(r.duration);
                }
            }
            ok = ok || list.iter().any(|r| r.error.is_empty());
        }
        if !ok {
            it.error = if first_err.is_empty() {
                "no media file: the sidecar names none, and none is named after it".to_owned()
            } else {
                first_err
            };
        }
        for m in g.subs {
            let Some(f) = self.file(m.entry) else {
                continue;
            };
            let format = Path::new(&f.path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let mut s = Subtitle {
                format,
                name: if m.name.is_empty() {
                    m.language.clone()
                } else {
                    m.name
                },
                language: m.language,
                default: m.default,
                forced: m.forced,
                error: String::new(),
                file: f,
            };
            if s.file.size > self.s.subtitle_bytes {
                s.error = format!(
                    "{} bytes, past max_subtitle_bytes, {}",
                    s.file.size, self.s.subtitle_bytes
                );
            }
            add(&mut it, &s.file.clone());
            it.subtitles.push(s);
        }
        if let Some(p) = g.poster.and_then(|p| self.file(p)) {
            add(&mut it, &p);
            it.poster = Some(p);
        }
        if let Some(s) = it.sidecar.clone() {
            add(&mut it, &s);
        }
        it
    }
}

fn first_language(r: &Rendition) -> String {
    r.tracks
        .iter()
        .find(|t| t.kind == KIND_AUDIO && !t.language.is_empty() && t.language != "und")
        .map(|t| t.language.clone())
        .unwrap_or_default()
}

/// What an error of the MP4 reader means for the file (fit's `probeError`).
fn probe_error(e: &Mp4Error) -> String {
    match e {
        Mp4Error::NoMoov => "not an MP4 file, or one cut short before its moov box",
        Mp4Error::Fragmented => "a fragmented MP4 file, which the library does not serve",
        Mp4Error::Limit(_) => "an MP4 file past the reader's limits",
        _ => "a malformed MP4 file",
    }
    .to_owned()
}

const CHANGED: &str = "the file changed while it was probed; it is probed again at the next scan";

/// Probes the media file `f`: its `moov` found and parsed through its root's checks, and its
/// tracks told (fit's `probeFile`).
pub(crate) fn probe_file(f: &File, follow: bool, limits: &Limits) -> Rendition {
    let mut r = Rendition {
        file: f.clone(),
        name: String::new(),
        language: String::new(),
        default: false,
        duration: Duration::ZERO,
        bitrate: 0,
        width: 0,
        height: 0,
        codecs: String::new(),
        tracks: Vec::new(),
        error: String::new(),
        moov: None,
        transient: false,
    };
    let h = match open_file(f, follow) {
        Ok(h) => h,
        Err(Error::Changed) => {
            r.error = CHANGED.to_owned();
            r.transient = true;
            return r;
        }
        Err(e) => {
            r.error = e.to_string();
            return r;
        }
    };
    let parsed = (|| -> Result<Movie, String> {
        let mut loc = Locator::new(h.size, limits);
        let at = loop {
            let (off, want) = loc.next_read();
            let len = HEAD_READ
                .max(want)
                .min(usize::try_from(h.size.saturating_sub(off)).unwrap_or(HEAD_READ));
            let mut buf = vec![0u8; len];
            let got = h.read_at(&mut buf, off).map_err(|e| e.to_string())?;
            buf.truncate(got);
            if let Some(at) = loc.feed(&buf).map_err(|e| probe_error(&e))? {
                break at;
            }
        };
        let len = usize::try_from(at.size)
            .map_err(|_| "an MP4 file past the reader's limits".to_owned())?;
        let moov = h.read_exact(at.offset, len).map_err(|e| e.to_string())?;
        Movie::parse(&moov, &at, limits).map_err(|e| probe_error(&e))
    })();
    // Unchanged while it was read: as it was opened.
    let changed = match h.file().metadata() {
        Ok(m) => m.len() != f.size || modified(&m) != f.modified,
        Err(_) => true,
    };
    if changed {
        r.error = CHANGED.to_owned();
        r.transient = true;
        return r;
    }
    match parsed {
        Ok(m) => describe(&mut r, &m),
        Err(e) => r.error = e,
    }
    r
}

/// Fills `r` from `m`, the file parsed (fit's `describe`).
fn describe(r: &mut Rendition, m: &Movie) {
    let mut codecs = Vec::new();
    let mut playable = false;
    let mut end = Duration::ZERO;
    for (i, t) in m.tracks.iter().enumerate() {
        let kind = kind_of(t.handler);
        let (start, duration) = presented(m, t);
        let mut tr = Track {
            index: i,
            id: t.id,
            kind,
            codec: codec_name(t.track.codec),
            // fit's (fib's) spelling: an H.264 profile and level in lower-case hex.
            codec_string: t
                .track
                .codec_string()
                .map(|c| {
                    if c.starts_with("avc1.") {
                        c.to_ascii_lowercase()
                    } else {
                        c
                    }
                })
                .unwrap_or_default(),
            language: t.language.clone(),
            width: 0,
            height: 0,
            sample_rate: t.track.sample_rate,
            channels: t.track.channels,
            start,
            duration,
            samples: t.index.len(),
            table_bytes: t.index.heap_bytes() as u64,
        };
        end = end.max(start.saturating_add(duration));
        if kind == KIND_VIDEO {
            tr.width = t.track.width;
            tr.height = t.track.height;
            if r.width == 0 {
                r.width = tr.width;
                r.height = tr.height;
            }
        }
        let c = t.track.codec;
        if (c.is_video() && kind == KIND_VIDEO) || (c.is_audio() && kind == KIND_AUDIO) {
            playable = true;
            if !tr.codec_string.is_empty() {
                codecs.push(tr.codec_string.clone());
            }
        }
        r.tracks.push(tr);
    }
    if !playable {
        r.error =
            "no H.264, H.265 or AV1 video, nor AAC, Opus or MP3 audio, which the library serves"
                .to_owned();
        return;
    }
    r.duration = nanos(m.duration_time());
    if r.duration.is_zero() {
        r.duration = end;
    }
    let secs = r.duration.as_secs_f64();
    if secs > 0.0 {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        {
            r.bitrate = (r.file.size as f64 * 8.0 / secs) as u64;
        }
    }
    r.codecs = codecs.join(",");
    r.moov = Some((m.moov_offset, m.moov_size));
}

fn nanos(t: akio_media::av::Time) -> Duration {
    Duration::from_nanos(u64::try_from(t).unwrap_or(0))
}

/// When `t` starts to play and for how long, as its edit list says (fit's `presented`): after
/// the empty edits at its start, for the movie's time of the edits that play; its media's
/// duration without an edit list.
fn presented(m: &Movie, t: &akio_media::mp4::Mp4Track) -> (Duration, Duration) {
    let ticks = |v: u64, scale: u32| {
        let v = i64::try_from(v).unwrap_or(i64::MAX);
        nanos(akio_media::av::from_ticks(v, scale))
    };
    if t.edits.is_empty() || m.timescale == 0 {
        return (Duration::ZERO, ticks(t.duration, t.timescale));
    }
    let (mut empty, mut play) = (0u64, 0u64);
    for e in &t.edits {
        if e.media_time < 0 && play == 0 {
            empty = empty.saturating_add(e.duration);
        } else if e.media_time >= 0 {
            play = play.saturating_add(e.duration);
        }
    }
    (ticks(empty, m.timescale), ticks(play, m.timescale))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        for good in [
            "720p",
            "1080p",
            "1080p60",
            "4k",
            "800k",
            "800kbps",
            "audio",
            "audio-fr",
            "audio-pt-BR",
        ] {
            assert!(rendition_label(good), "{good}");
        }
        for bad in [
            "72p",
            "12345p",
            "1080p1234",
            "k",
            "kbps",
            "audiox",
            "audio-",
            "en",
        ] {
            assert!(!rendition_label(bad), "{bad}");
        }
        assert_eq!(split_label("bbb.720p"), ("bbb", "720p"));
        assert_eq!(split_label("bbb.en"), ("bbb.en", ""));
        assert_eq!(split_label(".720p"), (".720p", ""));
        assert_eq!(label_language("audio-fr"), "fr");
        assert_eq!(label_language("audio-fr-sdh"), "fr");
        assert_eq!(label_language("audio-main"), "");
        for good in ["en", "zho", "pt-BR", "zh-Hans"] {
            assert!(subtitle_language(good), "{good}");
        }
        for bad in ["e", "engl", "EN", "pt-", "pt-B", "default"] {
            assert!(!subtitle_language(bad), "{bad}");
        }
        assert_eq!(
            under_dir("films", "a/../b.mp4").as_deref(),
            Some("films/b.mp4")
        );
        assert_eq!(under_dir("", "b.mp4").as_deref(), Some("b.mp4"));
        for bad in ["", "/etc/passwd", "../x", "x\\y", "..", ".", "a/../.."] {
            assert_eq!(under_dir("films", bad), None, "{bad}");
        }
    }
}
