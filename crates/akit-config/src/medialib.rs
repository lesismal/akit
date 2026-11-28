//! fit's `media_library` (I140, fit's `config/medialibrary.go`): named libraries of media files,
//! directories scanned into a catalog of items that the media handlers serve, with fit's fields,
//! defaults, bounds and checks.
//!
//! What is left out is kept empty, zero or false; the methods give fit's defaults for it. The
//! uploads' fields (`upload_dir`, `max_upload_bytes`, `max_uploads`, `min_free_bytes`,
//! `upload_expiry`) are refused as not supported yet until the media files are managed (I149).
//!
//! The patterns are Go's `path.Match` patterns split at '/', `**` spanning directories:
//! [`path_match`] is Go's matcher, so that a pattern fit takes matches the same names here.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use crate::decode::Decoder;
use crate::error::{join_field, join_index};
use crate::json::Node;
use crate::roles::{boolean, bytes, count, duration, text_at};

/// fit's `DefaultMediaScanInterval`.
pub const DEFAULT_SCAN_INTERVAL: Duration = Duration::from_secs(600);
/// The shortest `scan_interval` may be.
pub const MIN_SCAN_INTERVAL: Duration = Duration::from_secs(10);
/// fit's `DefaultMediaMaxItems`.
pub const DEFAULT_MAX_ITEMS: usize = 20_000;
/// fit's `DefaultMediaMaxFiles`.
pub const DEFAULT_MAX_FILES: usize = 200_000;
/// fit's `DefaultMediaMaxCatalogBytes`.
pub const DEFAULT_MAX_CATALOG_BYTES: u64 = 64 << 20;
/// fit's `DefaultMediaMaxProbes`.
pub const DEFAULT_MAX_PROBES: usize = 4;
/// fit's `DefaultMediaMaxSidecarBytes`.
pub const DEFAULT_MAX_SIDECAR_BYTES: u64 = 64 << 10;
/// The most `max_sidecar_bytes` may be.
pub const MAX_SIDECAR_BYTES: u64 = 1 << 20;
/// fit's `DefaultMediaMaxSubtitleBytes`.
pub const DEFAULT_MAX_SUBTITLE_BYTES: u64 = 2 << 20;
/// fit's `DefaultMediaMaxTableBytes`.
pub const DEFAULT_MAX_TABLE_BYTES: u64 = 128 << 20;
/// fit's `DefaultMediaFileQueue`.
pub const DEFAULT_FILE_QUEUE: usize = 4096;
/// fit's `DefaultMediaFileTimeout`.
pub const DEFAULT_FILE_TIMEOUT: Duration = Duration::from_secs(10);
/// The fewest file workers by default, and how many a CPU.
const MIN_FILE_WORKERS: usize = 16;
const FILE_WORKERS_PER_CPU: usize = 4;
/// The fewest bytes a library's secret may have (fit's `MinMediaSecret`).
pub const MIN_SECRET: usize = 16;

/// A named media library, as fit's `config.MediaLibrary`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MediaLibrary {
    /// `name`: letters, digits, '_', '.' and '-', starting with a letter or digit.
    pub name: String,
    /// `roots`: the directories scanned, none inside another.
    pub roots: Vec<String>,
    /// `files`: the files looked at, by pattern; every file when empty.
    pub files: Vec<String>,
    /// `exclude`: the files and directories left out, by pattern.
    pub exclude: Vec<String>,
    /// `follow_symlinks`: follow links that lead out of the roots.
    pub follow_symlinks: bool,
    /// `scan_interval`: [`DEFAULT_SCAN_INTERVAL`] when zero.
    pub scan_interval: Duration,
    /// `max_items`: [`DEFAULT_MAX_ITEMS`] when 0.
    pub max_items: usize,
    /// `max_files`: [`DEFAULT_MAX_FILES`] when 0.
    pub max_files: usize,
    /// `max_catalog_bytes`: [`DEFAULT_MAX_CATALOG_BYTES`] when 0.
    pub max_catalog_bytes: u64,
    /// `max_probes`: [`DEFAULT_MAX_PROBES`] when 0.
    pub max_probes: usize,
    /// `max_sidecar_bytes`: [`DEFAULT_MAX_SIDECAR_BYTES`] when 0, at most [`MAX_SIDECAR_BYTES`].
    pub max_sidecar_bytes: u64,
    /// `max_subtitle_bytes`: [`DEFAULT_MAX_SUBTITLE_BYTES`] when 0.
    pub max_subtitle_bytes: u64,
    /// `max_table_bytes`: [`DEFAULT_MAX_TABLE_BYTES`] when 0.
    pub max_table_bytes: u64,
    /// `file_workers`: 4 a CPU, at least 16, when 0.
    pub file_workers: usize,
    /// `file_queue`: [`DEFAULT_FILE_QUEUE`] when 0.
    pub file_queue: usize,
    /// `file_timeout`: [`DEFAULT_FILE_TIMEOUT`] when zero.
    pub file_timeout: Duration,
    /// `secret`: the secret the keys of encrypted HLS are derived from; empty for none.
    pub secret: String,
    /// `secret_file`: a file holding it, read each time the config is applied; empty for none.
    pub secret_file: String,
}

fn or<T: PartialEq + Default>(v: T, def: T) -> T {
    if v == T::default() { def } else { v }
}

impl MediaLibrary {
    /// `scan_interval`, or its default.
    #[must_use]
    pub fn interval(&self) -> Duration {
        or(self.scan_interval, DEFAULT_SCAN_INTERVAL)
    }

    /// `max_items`, or its default.
    #[must_use]
    pub fn items(&self) -> usize {
        or(self.max_items, DEFAULT_MAX_ITEMS)
    }

    /// `max_files`, or its default.
    #[must_use]
    pub fn files_bound(&self) -> usize {
        or(self.max_files, DEFAULT_MAX_FILES)
    }

    /// `max_catalog_bytes`, or its default.
    #[must_use]
    pub fn catalog_bytes(&self) -> u64 {
        or(self.max_catalog_bytes, DEFAULT_MAX_CATALOG_BYTES)
    }

    /// `max_probes`, or its default.
    #[must_use]
    pub fn probes(&self) -> usize {
        or(self.max_probes, DEFAULT_MAX_PROBES)
    }

    /// `max_sidecar_bytes`, or its default.
    #[must_use]
    pub fn sidecar_bytes(&self) -> u64 {
        or(self.max_sidecar_bytes, DEFAULT_MAX_SIDECAR_BYTES)
    }

    /// `max_subtitle_bytes`, or its default.
    #[must_use]
    pub fn subtitle_bytes(&self) -> u64 {
        or(self.max_subtitle_bytes, DEFAULT_MAX_SUBTITLE_BYTES)
    }

    /// `max_table_bytes`, or its default.
    #[must_use]
    pub fn table_bytes(&self) -> u64 {
        or(self.max_table_bytes, DEFAULT_MAX_TABLE_BYTES)
    }

    /// `file_workers`, or its default: 4 a CPU, and at least 16.
    #[must_use]
    pub fn workers(&self) -> usize {
        let cpus = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        or(
            self.file_workers,
            MIN_FILE_WORKERS.max(FILE_WORKERS_PER_CPU.saturating_mul(cpus)),
        )
    }

    /// `file_queue`, or its default.
    #[must_use]
    pub fn queue(&self) -> usize {
        or(self.file_queue, DEFAULT_FILE_QUEUE)
    }

    /// `file_timeout`, or its default.
    #[must_use]
    pub fn timeout(&self) -> Duration {
        or(self.file_timeout, DEFAULT_FILE_TIMEOUT)
    }

    /// Whether the library has a secret to derive the keys of encrypted HLS from.
    #[must_use]
    pub fn encryptable(&self) -> bool {
        !self.secret.is_empty() || !self.secret_file.is_empty()
    }
}

const MEDIA_LIBRARY_FIELDS: &[&str] = &[
    "name",
    "roots",
    "files",
    "exclude",
    "follow_symlinks",
    "scan_interval",
    "max_items",
    "max_files",
    "max_catalog_bytes",
    "max_probes",
    "max_sidecar_bytes",
    "max_subtitle_bytes",
    "max_table_bytes",
    "file_workers",
    "file_queue",
    "file_timeout",
    "secret",
    "secret_file",
];
// The uploads: the media files managed (I149).
const MEDIA_LIBRARY_FIT_ONLY: &[&str] = &[
    "upload_dir",
    "max_upload_bytes",
    "max_uploads",
    "min_free_bytes",
    "upload_expiry",
];

/// Decodes `media_library`, fit's checks of each library with it (`checkMediaLibrary`).
pub(crate) fn media_library(d: &mut Decoder<'_>, n: &Node, path: &str) -> Vec<MediaLibrary> {
    let got = d.array(n, path, |d, n, p| {
        library(d, n, p).map(|m| (m, n.off, p.to_owned()))
    });
    let mut seen: Vec<&str> = Vec::new();
    for (m, off, p) in &got {
        if crate::model::valid_name(&m.name) {
            if seen.contains(&m.name.as_str()) {
                d.error(
                    *off,
                    &join_field(p, "name"),
                    format!("another media_library is already named {:?}", m.name),
                );
            }
            seen.push(&m.name);
        }
    }
    got.into_iter().map(|(m, _, _)| m).collect()
}

fn library(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<MediaLibrary> {
    let mut o = d.object(n, path)?;
    let mut m = MediaLibrary::default();
    if let Some(v) = o.require(d, "name") {
        m.name = d.string(v, &o.at("name")).unwrap_or_default();
        if !crate::model::valid_name(&m.name) {
            d.error(
                v.off,
                &o.at("name"),
                format!(
                    "{:?} is not a valid name: use letters, digits, '_', '.' and '-', starting with a letter or digit",
                    m.name
                ),
            );
        }
    }
    if let Some(v) = o.require(d, "roots") {
        let p = o.at("roots");
        let roots = d.strings(v, &p);
        if roots.is_empty() && matches!(v.kind, crate::json::Kind::Array(_)) {
            d.error(v.off, &p, "a library needs at least one root");
        }
        let mut clean: Vec<Option<(PathBuf, &str)>> = Vec::with_capacity(roots.len());
        for (j, (off, r)) in roots.iter().enumerate() {
            if r.trim().is_empty() {
                d.error(*off, &join_index(&p, j), "a root may not be empty");
                clean.push(None);
                continue;
            }
            let c = absolute(r);
            for (other, conf) in clean.iter().flatten() {
                let msg = if *other == c {
                    format!("{r:?} is listed twice")
                } else if within(other, &c) || within(&c, other) {
                    format!(
                        "{conf:?} and {r:?} lie one inside the other: the files would be listed twice"
                    )
                } else {
                    continue;
                };
                d.error(*off, &join_index(&p, j), msg);
            }
            clean.push(Some((c, r.as_str())));
        }
        m.roots = roots.into_iter().map(|(_, r)| r).collect();
    }
    for (key, slot) in [("files", &mut m.files), ("exclude", &mut m.exclude)] {
        let Some(v) = o.take(key) else { continue };
        let p = o.at(key);
        for (j, (off, pat)) in d.strings(v, &p).into_iter().enumerate() {
            if !valid_path_pattern(&pat) {
                d.error(
                    off,
                    &join_index(&p, j),
                    format!(
                        "{pat:?} is not a pattern: names and *, ? and [...] as path.Match takes them, and ** for any run of directories, split at '/', not starting with one"
                    ),
                );
            }
            slot.push(pat);
        }
    }
    m.follow_symlinks = boolean(d, &mut o, "follow_symlinks");
    if let Some(v) = o.take("scan_interval") {
        if let Some(t) = d.duration(v, &o.at("scan_interval")) {
            if !t.is_zero() && t < MIN_SCAN_INTERVAL {
                d.error(
                    v.off,
                    &o.at("scan_interval"),
                    format!(
                        "scan_interval is {}: at least {}",
                        crate::duration::format(t),
                        crate::duration::format(MIN_SCAN_INTERVAL)
                    ),
                );
            }
            m.scan_interval = t;
        }
    }
    m.max_items = count(d, &mut o, "max_items", i64::MAX);
    m.max_files = count(d, &mut o, "max_files", i64::MAX);
    m.max_catalog_bytes = bytes(d, &mut o, "max_catalog_bytes");
    m.max_probes = count(d, &mut o, "max_probes", i64::MAX);
    if let Some(v) = o.take("max_sidecar_bytes") {
        let max = i64::try_from(MAX_SIDECAR_BYTES).unwrap_or(i64::MAX);
        m.max_sidecar_bytes = d
            .int(v, &o.at("max_sidecar_bytes"), 0, max)
            .and_then(|n| u64::try_from(n).ok())
            .unwrap_or(0);
    }
    m.max_subtitle_bytes = bytes(d, &mut o, "max_subtitle_bytes");
    m.max_table_bytes = bytes(d, &mut o, "max_table_bytes");
    m.file_workers = count(d, &mut o, "file_workers", i64::MAX);
    m.file_queue = count(d, &mut o, "file_queue", i64::MAX);
    m.file_timeout = duration(d, &mut o, "file_timeout");
    let secret_at = text_at(d, &mut o, "secret").map(|(s, off)| {
        m.secret = s;
        off
    });
    let secret_file = text_at(d, &mut o, "secret_file");
    if let Some((f, off)) = secret_file {
        if !f.is_empty() && !m.secret.is_empty() {
            d.error(
                off,
                &o.at("secret_file"),
                "set secret or secret_file, one of the two",
            );
        }
        m.secret_file = f;
    }
    if m.secret_file.is_empty() && !m.secret.is_empty() && m.secret.len() < MIN_SECRET {
        d.error(
            secret_at.unwrap_or(o.off),
            &o.at("secret"),
            format!(
                "the secret is {} bytes: use at least {MIN_SECRET}",
                m.secret.len()
            ),
        );
    }
    o.finish(d, MEDIA_LIBRARY_FIELDS, MEDIA_LIBRARY_FIT_ONLY);
    Some(m)
}

/// Whether `p` is a pattern of a library's `files` or `exclude` (fit's `ValidMediaPathPattern`):
/// not empty, not starting with '/', its parts split at '/' each `**` or a pattern Go's
/// `path.Match` takes.
#[must_use]
pub fn valid_path_pattern(p: &str) -> bool {
    !p.is_empty()
        && !p.starts_with('/')
        && p.split('/')
            .all(|part| part == "**" || (!part.is_empty() && path_match(part, "").is_ok()))
}

/// A pattern Go's `path.Match` refuses (`ErrBadPattern`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadPattern;

/// Go's `path.Match`: whether `name` matches the shell pattern `pattern`, where `*` matches any
/// run of characters but '/', `?` one character but '/', `[...]` a class (with ranges and `^`),
/// and `\` escapes; [`BadPattern`] for a malformed pattern, which is reported even when the name
/// does not match.
///
/// # Errors
///
/// [`BadPattern`] for a malformed pattern.
pub fn path_match(pattern: &str, name: &str) -> Result<bool, BadPattern> {
    let mut pattern = pattern.as_bytes();
    let mut name = name.as_bytes();
    'pattern: while !pattern.is_empty() {
        let (star, chunk, rest) = scan_chunk(pattern);
        pattern = rest;
        if star && chunk.is_empty() {
            // A trailing * matches the rest unless it holds a '/'.
            return Ok(!name.contains(&b'/'));
        }
        let (t, ok) = match_chunk(chunk, name)?;
        if ok && (t.is_empty() || !pattern.is_empty()) {
            name = t;
            continue;
        }
        if star {
            // A match skipping one byte more each time, never a '/'.
            let mut after = name;
            while let Some((&b, tail)) = after.split_first() {
                if b == b'/' {
                    break;
                }
                after = tail;
                let (t, ok) = match_chunk(chunk, after)?;
                if ok {
                    if pattern.is_empty() && !t.is_empty() {
                        continue;
                    }
                    name = t;
                    continue 'pattern;
                }
            }
        }
        // The rest of the pattern must still be well formed.
        while !pattern.is_empty() {
            let (_, chunk, rest) = scan_chunk(pattern);
            pattern = rest;
            match_chunk(chunk, b"")?;
        }
        return Ok(false);
    }
    Ok(name.is_empty())
}

/// Go's `scanChunk`: whether stars come before the next chunk, the chunk, and the rest.
fn scan_chunk(pattern: &[u8]) -> (bool, &[u8], &[u8]) {
    let mut rest = pattern;
    let mut star = false;
    while let Some((b'*', tail)) = rest.split_first() {
        star = true;
        rest = tail;
    }
    let all = rest;
    let mut in_range = false;
    let mut len = 0usize;
    let mut scan = all;
    while let Some((&b, tail)) = scan.split_first() {
        match b {
            b'\\' => {
                if let Some((_, after)) = tail.split_first() {
                    len = len.saturating_add(1);
                    scan = after;
                    len = len.saturating_add(1);
                    continue;
                }
            }
            b'[' => in_range = true,
            b']' => in_range = false,
            b'*' if !in_range => break,
            _ => {}
        }
        len = len.saturating_add(1);
        scan = tail;
    }
    let (chunk, rest) = all.split_at(len.min(all.len()));
    (star, chunk, rest)
}

/// The first character of `b` and what follows it, as Go's `utf8.DecodeRune`: U+FFFD and one
/// byte for a byte that does not start one; `None` for no byte.
fn rune(b: &[u8]) -> Option<(char, &[u8])> {
    for w in 1..=b.len().min(4) {
        let (head, tail) = b.split_at(w);
        if let Some(c) = std::str::from_utf8(head)
            .ok()
            .and_then(|s| s.chars().next())
        {
            return Some((c, tail));
        }
    }
    b.split_first().map(|(_, tail)| ('\u{fffd}', tail))
}

/// Go's `matchChunk`: what of `s` is left after `chunk` matches its start, and whether it did;
/// the chunk is checked to its end once the match failed.
fn match_chunk<'s>(chunk: &[u8], s: &'s [u8]) -> Result<(&'s [u8], bool), BadPattern> {
    let mut chunk = chunk;
    let mut s = s;
    let mut failed = false;
    while let Some((&c, after)) = chunk.split_first() {
        if !failed && s.is_empty() {
            failed = true;
        }
        match c {
            b'[' => {
                let mut r = '\u{fffd}';
                if !failed {
                    if let Some((sc, tail)) = rune(s) {
                        r = sc;
                        s = tail;
                    }
                }
                chunk = after;
                let negated = chunk.first() == Some(&b'^');
                if let (true, Some((_, tail))) = (negated, chunk.split_first()) {
                    chunk = tail;
                }
                let mut matched = false;
                let mut ranges = 0usize;
                loop {
                    if let (Some((b']', tail)), true) = (chunk.split_first(), ranges > 0) {
                        chunk = tail;
                        break;
                    }
                    let (lo, rest) = get_esc(chunk)?;
                    chunk = rest;
                    let mut hi = lo;
                    if let Some((b'-', tail)) = chunk.split_first() {
                        let (h, rest) = get_esc(tail)?;
                        hi = h;
                        chunk = rest;
                    }
                    if lo <= r && r <= hi {
                        matched = true;
                    }
                    ranges = ranges.saturating_add(1);
                }
                if matched == negated {
                    failed = true;
                }
            }
            b'?' => {
                if !failed {
                    if s.first() == Some(&b'/') {
                        failed = true;
                    }
                    s = rune(s).map_or(s, |(_, tail)| tail);
                }
                chunk = after;
            }
            _ => {
                let (lit, rest) = if c == b'\\' {
                    after.split_first().ok_or(BadPattern)?
                } else {
                    (&c, after)
                };
                if !failed {
                    match s.split_first() {
                        Some((b, tail)) if b == lit => s = tail,
                        _ => failed = true,
                    }
                }
                chunk = rest;
            }
        }
    }
    if failed {
        Ok((&[], false))
    } else {
        Ok((s, true))
    }
}

/// Go's `getEsc`: a character of a class, escaped or not, and the rest of the chunk.
fn get_esc(chunk: &[u8]) -> Result<(char, &[u8]), BadPattern> {
    let chunk = match chunk.split_first() {
        None | Some((b'-' | b']', _)) => return Err(BadPattern),
        Some((b'\\', tail)) => tail,
        Some(_) => chunk,
    };
    let (c, rest) = rune(chunk).ok_or(BadPattern)?;
    let invalid = c == '\u{fffd}' && chunk.len().saturating_sub(rest.len()) == 1;
    if invalid || rest.is_empty() {
        return Err(BadPattern);
    }
    Ok((c, rest))
}

/// `p` made absolute against the working directory and cleaned of `.` and `..` lexically, as Go's
/// `filepath.Abs` does; `p` cleaned as it is when the working directory cannot be read.
#[must_use]
pub fn absolute(p: &str) -> PathBuf {
    let path = Path::new(p);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
    };
    clean(&joined)
}

/// `p` cleaned lexically, as Go's `filepath.Clean`: `.` dropped, `..` taking the part before it
/// (none above the root).
#[must_use]
pub fn clean(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                let up = matches!(out.components().next_back(), Some(Component::ParentDir));
                if up || (out.as_os_str().is_empty() && !out.has_root()) {
                    out.push("..");
                } else {
                    // At the root, `..` is the root.
                    out.pop();
                }
            }
            other => out.push(other),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// Whether `p` lies inside `dir`, both clean absolute paths, and is not `dir` itself (fit's
/// `within`).
#[must_use]
pub fn within(dir: &Path, p: &Path) -> bool {
    p != dir && p.starts_with(dir)
}

/// fit's `vod` defaults and bounds (`handler/vod`).
pub const DEFAULT_SEGMENT_DURATION: Duration = Duration::from_secs(4);
/// See [`DEFAULT_SEGMENT_DURATION`].
pub const MIN_SEGMENT_DURATION: Duration = Duration::from_secs(1);
/// See [`DEFAULT_SEGMENT_DURATION`].
pub const MAX_SEGMENT_DURATION: Duration = Duration::from_secs(60);
/// See [`DEFAULT_SEGMENT_DURATION`].
pub const DEFAULT_MAX_CACHE_BYTES: u64 = 256 << 20;
/// See [`DEFAULT_SEGMENT_DURATION`].
pub const DEFAULT_MAX_SEGMENT_BYTES: u64 = 32 << 20;
/// See [`DEFAULT_SEGMENT_DURATION`].
pub const MIN_MAX_SEGMENT_BYTES: u64 = 64 << 10;
/// See [`DEFAULT_SEGMENT_DURATION`].
pub const MAX_MAX_SEGMENT_BYTES: u64 = 1 << 30;
/// See [`DEFAULT_SEGMENT_DURATION`].
pub const DEFAULT_VOD_TIMEOUT: Duration = Duration::from_secs(30);

/// The `vod` handler's segments: fMP4 or MPEG-TS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Container {
    /// `fmp4`, the default.
    #[default]
    Fmp4,
    /// `ts`.
    Ts,
}

/// The `vod` handler's params, as fit's `vod.VOD` (I142): a media library's items as video on
/// demand over HLS, and the files themselves with ranges.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Vod {
    /// `library`: the `media_library` whose items are served.
    pub library: String,
    /// `prefix`: the part of the path before an item's ID, ending in '/'; `/vod/<library>/` when
    /// left out.
    pub prefix: String,
    /// `auth`: the `media_auth` policy every request must satisfy, action `play` on the stream
    /// `vod/<library>/<id>`; empty for none.
    pub auth: String,
    /// `container`: what `index.m3u8` lists.
    pub container: Container,
    /// `segment_duration`: [`DEFAULT_SEGMENT_DURATION`] when zero.
    pub segment_duration: Duration,
    /// `max_cache_bytes`: [`DEFAULT_MAX_CACHE_BYTES`] when 0.
    pub max_cache_bytes: u64,
    /// `max_segment_bytes`: the smaller of [`DEFAULT_MAX_SEGMENT_BYTES`] and a quarter of the
    /// cache when 0.
    pub max_segment_bytes: u64,
    /// `cache_control`: sent as `Cache-Control`; none when empty.
    pub cache_control: String,
    /// `cors`: sent as `Access-Control-Allow-Origin`; none when empty.
    pub cors: String,
    /// `timeout`: [`DEFAULT_VOD_TIMEOUT`] when zero.
    pub timeout: Duration,
    /// `encrypt` (I147): AES-128 segments, each item under a key of its own derived from the
    /// library's secret.
    pub encrypt: bool,
    /// The policy `auth` names, from the config.
    pub policy: Option<crate::media::MediaAuth>,
}

impl Vod {
    /// `segment_duration`, or its default.
    #[must_use]
    pub fn segment(&self) -> Duration {
        or(self.segment_duration, DEFAULT_SEGMENT_DURATION)
    }

    /// `max_cache_bytes`, or its default.
    #[must_use]
    pub fn cache_bytes(&self) -> u64 {
        or(self.max_cache_bytes, DEFAULT_MAX_CACHE_BYTES)
    }

    /// `max_segment_bytes`, or its default.
    #[must_use]
    pub fn segment_bytes(&self) -> u64 {
        or(
            self.max_segment_bytes,
            DEFAULT_MAX_SEGMENT_BYTES.min(self.cache_bytes() / 4),
        )
    }

    /// `timeout`, or its default.
    #[must_use]
    pub fn wait(&self) -> Duration {
        or(self.timeout, DEFAULT_VOD_TIMEOUT)
    }

    /// `prefix`, or its default.
    #[must_use]
    pub fn prefix_or(&self) -> String {
        if self.prefix.is_empty() {
            format!("/vod/{}/", self.library)
        } else if self.prefix.ends_with('/') {
            self.prefix.clone()
        } else {
            format!("{}/", self.prefix)
        }
    }
}

const VOD_FIELDS: &[&str] = &[
    "library",
    "prefix",
    "auth",
    "container",
    "segment_duration",
    "max_cache_bytes",
    "max_segment_bytes",
    "cache_control",
    "cors",
    "timeout",
    // I147.
    "encrypt",
];
const VOD_FIT_ONLY: &[&str] = &[];

/// Decodes the `vod` handler's params, with fit's checks (`VOD.Provision`); the library it names
/// is checked once every file is read.
pub(crate) fn vod(
    d: &mut Decoder<'_>,
    params: Option<&Node>,
    off: usize,
    path: &str,
) -> Option<Vod> {
    let Some(n) = params else {
        d.error(off, &join_field(path, "library"), "missing required field");
        return None;
    };
    let mut o = d.object(n, path)?;
    let mut v = Vod::default();
    if let Some(x) = o.require(d, "library") {
        v.library = d.string(x, &o.at("library")).unwrap_or_default();
        if !crate::model::valid_name(&v.library) {
            d.error(
                x.off,
                &o.at("library"),
                format!("{:?} is not a library's name", v.library),
            );
        }
    }
    if let Some((p, poff)) = text_at(d, &mut o, "prefix") {
        if !p.is_empty() && !p.starts_with('/') {
            d.error(
                poff,
                &o.at("prefix"),
                format!("{p:?} does not start with /"),
            );
        }
        v.prefix = p;
    }
    if let Some((a, _)) = text_at(d, &mut o, "auth") {
        v.auth = a;
    }
    if let Some((c, coff)) = text_at(d, &mut o, "container") {
        match c.as_str() {
            "" | "fmp4" => {}
            "ts" => v.container = Container::Ts,
            _ => d.error(
                coff,
                &o.at("container"),
                format!("{c:?} is not one of \"fmp4\", \"ts\""),
            ),
        }
    }
    if let Some(x) = o.take("segment_duration") {
        let p = o.at("segment_duration");
        if let Some(t) = d.duration(x, &p) {
            if !t.is_zero() && !(MIN_SEGMENT_DURATION..=MAX_SEGMENT_DURATION).contains(&t) {
                d.error(
                    x.off,
                    &p,
                    format!(
                        "{} is not between {} and {}",
                        crate::duration::format(t),
                        crate::duration::format(MIN_SEGMENT_DURATION),
                        crate::duration::format(MAX_SEGMENT_DURATION)
                    ),
                );
            }
            v.segment_duration = t;
        }
    }
    v.max_cache_bytes = bytes(d, &mut o, "max_cache_bytes");
    let seg_at = o.take("max_segment_bytes").map(|x| {
        v.max_segment_bytes = d
            .int(x, &o.at("max_segment_bytes"), 0, i64::MAX)
            .and_then(|n| u64::try_from(n).ok())
            .unwrap_or(0);
        x.off
    });
    let seg = v.segment_bytes();
    let quarter = v.cache_bytes() / 4;
    if !(MIN_MAX_SEGMENT_BYTES..=MAX_MAX_SEGMENT_BYTES).contains(&seg) || seg > quarter {
        d.error(
            seg_at.unwrap_or(o.off),
            &o.at("max_segment_bytes"),
            format!(
                "{seg} is not between 64 KiB and the smaller of 1 GiB and a quarter of max_cache_bytes ({quarter})"
            ),
        );
    }
    for (key, slot) in [
        ("cache_control", &mut v.cache_control),
        ("cors", &mut v.cors),
    ] {
        if let Some((s, soff)) = text_at(d, &mut o, key) {
            if s.contains(['\r', '\n']) {
                d.error(
                    soff,
                    &o.at(key),
                    "cache_control and cors may not hold a line break",
                );
            }
            *slot = s;
        }
    }
    v.timeout = duration(d, &mut o, "timeout");
    if let Some(x) = o.take("encrypt") {
        let p = o.at("encrypt");
        match &x.kind {
            crate::json::Kind::Bool(b) => v.encrypt = *b,
            other => d.error(
                x.off,
                &p,
                format!("expected a boolean, got {}", other.name()),
            ),
        }
        if v.encrypt && v.auth.is_empty() {
            d.error(
                x.off,
                &p,
                "encrypt needs auth: the key is served only to a request its policy lets play the item",
            );
        }
    }
    o.finish(d, VOD_FIELDS, VOD_FIT_ONLY);
    Some(v)
}

#[cfg(test)]
mod tests;
