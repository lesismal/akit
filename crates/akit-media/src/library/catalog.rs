//! What a scan found in a library (fit's `medialib/catalog.go`): its items, a title each with
//! the files of its renditions, audio, subtitles and poster, and the problems the scan met.
//! A catalog is immutable: a scan makes a new one, and whoever holds the old one reads it on.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use akio_crypto::hash::Sha256;
use akit_config::value::Value;

/// The most problems a catalog lists; the rest are counted.
pub const MAX_PROBLEMS: usize = 256;
/// The longest an item's ID may be.
pub const MAX_ID_LENGTH: usize = 256;

/// What a scan found: its items, sorted by ID, and the problems it met.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    /// The items, sorted by ID.
    pub items: Vec<Arc<Item>>,
    /// What the scan could not list, at most [`MAX_PROBLEMS`].
    pub problems: Vec<Problem>,
    /// The problems past [`MAX_PROBLEMS`], counted.
    pub more_problems: usize,
    /// Why the catalog lacks items the roots hold, past a bound; empty when it lacks none.
    pub truncated: String,
    /// The memory the catalog takes, as reckoned for `max_catalog_bytes`.
    pub bytes: u64,
    /// When the scan that made it ended; `None` before the first.
    pub scanned: Option<SystemTime>,
    pub(crate) by_id: HashMap<String, Arc<Item>>,
}

impl Catalog {
    /// The item whose ID is `id`.
    #[must_use]
    pub fn item(&self, id: &str) -> Option<&Arc<Item>> {
        self.by_id.get(id)
    }

    /// A copy without the items of `ids`.
    #[must_use]
    pub(crate) fn without(&self, ids: &[String]) -> Catalog {
        let mut out = self.clone();
        out.items.retain(|it| !ids.contains(&it.id));
        out.by_id.retain(|id, _| !ids.contains(id));
        out.bytes = out.items.iter().map(|it| it.bytes).sum();
        out
    }
}

/// A file or directory a scan could not list, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The root it was found under, as the config writes it; empty for none.
    pub root: String,
    /// Its path under the root.
    pub path: String,
    /// What is wrong.
    pub error: String,
}

/// A file of a library: where it is, and its size and time when it was scanned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    /// The root it was found under, as the config writes it.
    pub root: String,
    /// Its path under the root, with '/'.
    pub path: String,
    /// Its size, as the scan found it.
    pub size: u64,
    /// Its time, as the scan found it.
    pub modified: SystemTime,
    /// The path it is opened by.
    pub(crate) abs: PathBuf,
    /// The root's path with its links resolved.
    pub(crate) real: PathBuf,
}

/// An entry of a catalog: a title, with the files of its renditions, audio, subtitles and
/// poster.
#[derive(Debug, Clone)]
pub struct Item {
    /// The sidecar's `id`, or one made from the path under the root.
    pub id: String,
    /// The sidecar's title, or the file's name.
    pub title: String,
    /// The root, as the config writes it.
    pub root: String,
    /// The path under it of the item's files, less their rendition label and extension.
    pub path: String,
    /// The longest of its renditions'.
    pub duration: Duration,
    /// Its files' bytes.
    pub size: u64,
    /// The latest of its files' times.
    pub modified: SystemTime,
    /// Its files with video.
    pub renditions: Vec<Rendition>,
    /// Its files with audio alone.
    pub audio: Vec<Rendition>,
    /// Its subtitles.
    pub subtitles: Vec<Subtitle>,
    /// Its poster.
    pub poster: Option<File>,
    /// Its sidecar.
    pub sidecar: Option<File>,
    /// Why it cannot be served, as none of its files probed right; empty when it can be.
    pub error: String,
    pub(crate) bytes: u64,
    /// Its ID is its sidecar's.
    pub(crate) named: bool,
}

/// A media file of an item, probed.
#[derive(Debug, Clone)]
pub struct Rendition {
    /// The file.
    pub file: File,
    /// Its label (`1080p`), or the sidecar's name.
    pub name: String,
    /// The sidecar's language, or its audio's.
    pub language: String,
    /// The audio a player picks first, as the sidecar says.
    pub default: bool,
    /// The file's duration.
    pub duration: Duration,
    /// Its bytes over its duration, in bits a second.
    pub bitrate: u64,
    /// Its first video track's width.
    pub width: u32,
    /// Its first video track's height.
    pub height: u32,
    /// The RFC 6381 codecs of its tracks, as a playlist's `CODECS` writes them.
    pub codecs: String,
    /// Its tracks.
    pub tracks: Vec<Track>,
    /// Why it cannot be served; empty when it can be.
    pub error: String,
    /// Where its `moov` is, `(offset, size)`, for a file that probed right.
    pub(crate) moov: Option<(u64, u64)>,
    /// An error that may pass: the file was changing.
    pub(crate) transient: bool,
}

impl Rendition {
    /// The memory its parsed movie takes, as the probe found it: its tracks' sample indexes.
    #[must_use]
    pub fn table_bytes(&self) -> u64 {
        self.tracks.iter().map(|t| t.table_bytes).sum()
    }

    /// Whether it has a video track of a codec the library serves.
    #[must_use]
    pub fn has_video(&self) -> bool {
        self.tracks
            .iter()
            .any(|t| t.kind == KIND_VIDEO && t.codec != "unknown")
    }
}

/// A track of a rendition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    /// Its index among the file's tracks.
    pub index: usize,
    /// Its `track_ID`.
    pub id: u32,
    /// `video`, `audio`, `subtitle` or `other`.
    pub kind: &'static str,
    /// `h264`, `h265`, `av1`, `vp8`, `vp9`, `aac`, `opus`, `mp3`, `pcmu`, `pcma`, or `unknown`.
    pub codec: &'static str,
    /// Its RFC 6381 codecs.
    pub codec_string: String,
    /// Its language.
    pub language: String,
    /// Its picture's width.
    pub width: u32,
    /// Its picture's height.
    pub height: u32,
    /// Its sample rate.
    pub sample_rate: u32,
    /// Its channels.
    pub channels: u8,
    /// When it starts to play, after its edit list's empty edits.
    pub start: Duration,
    /// How long it plays.
    pub duration: Duration,
    /// Its samples.
    pub samples: u32,
    /// The memory its sample index takes.
    pub table_bytes: u64,
}

/// A subtitle file of an item, `.vtt` or `.srt`, served as WebVTT.
#[derive(Debug, Clone)]
pub struct Subtitle {
    /// The file.
    pub file: File,
    /// `vtt` or `srt`, as the file is written.
    pub format: String,
    /// From the sidecar or the file's name (`bbb.en.srt`).
    pub language: String,
    /// From the sidecar or the file's name, or the language when neither says.
    pub name: String,
    /// From the sidecar or the file's name.
    pub default: bool,
    /// From the sidecar or the file's name (`bbb.en.forced.srt`).
    pub forced: bool,
    /// Why it cannot be served, as past `max_subtitle_bytes`.
    pub error: String,
}

/// The kinds of a track.
pub const KIND_VIDEO: &str = "video";
/// See [`KIND_VIDEO`].
pub const KIND_AUDIO: &str = "audio";
/// See [`KIND_VIDEO`].
pub const KIND_SUBTITLE: &str = "subtitle";
/// See [`KIND_VIDEO`].
pub const KIND_OTHER: &str = "other";

/// The kind of a track by its handler.
pub(crate) fn kind_of(handler: [u8; 4]) -> &'static str {
    match &handler {
        b"vide" => KIND_VIDEO,
        b"soun" => KIND_AUDIO,
        b"subt" | b"text" | b"sbtl" => KIND_SUBTITLE,
        _ => KIND_OTHER,
    }
}

/// A codec's name in the catalog.
pub(crate) fn codec_name(c: akio_media::av::Codec) -> &'static str {
    use akio_media::av::Codec;
    match c {
        Codec::H264 => "h264",
        Codec::H265 => "h265",
        Codec::Av1 => "av1",
        Codec::Vp8 => "vp8",
        Codec::Vp9 => "vp9",
        Codec::Aac => "aac",
        Codec::Opus => "opus",
        Codec::Mp3 => "mp3",
        Codec::Pcmu => "pcmu",
        Codec::Pcma => "pcma",
        _ => "unknown",
    }
}

/// Whether `part` may be a part of an ID: 1 to 128 letters, digits, '.', '_', '~' and '-', not
/// starting with '.'.
fn id_part(part: &str) -> bool {
    let b = part.as_bytes();
    let ok = |c: &u8| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'~' | b'-' | b'.');
    match b.split_first() {
        Some((first, rest)) => b.len() <= 128 && *first != b'.' && ok(first) && rest.iter().all(ok),
        None => false,
    }
}

/// Whether `id` is an item's ID (fit's `ValidID`): parts of [`id_part`] joined by '/', 256
/// bytes at most; a path of a URL as it is, and a stream name a `media_auth` pattern matches.
#[must_use]
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID_LENGTH && id.split('/').all(id_part)
}

/// The ID of an item at `rel`, a slash path under its root without its extension, when its
/// sidecar names none (fit's `pathID`): `rel` itself when it is an ID, and otherwise `rel` with
/// every run of what an ID may not hold made '-', and 8 hex digits of its SHA-256 after it, so
/// that two paths never make one ID and a path makes the same ID every scan.
#[must_use]
pub fn path_id(rel: &str) -> String {
    if valid_id(rel) {
        return rel.to_owned();
    }
    let parts: Vec<String> = rel
        .split('/')
        .map(|p| {
            let mut b = String::new();
            let mut dash = false;
            for c in p.chars() {
                let keep = c.is_ascii_alphanumeric()
                    || c == '_'
                    || c == '~'
                    || (c == '.' && !b.is_empty());
                if keep {
                    b.push(c);
                    dash = false;
                } else if !dash && !b.is_empty() {
                    b.push('-');
                    dash = true;
                }
            }
            let mut s = b.trim_end_matches(['-', '.']).to_owned();
            if s.len() > 64 {
                s.truncate(64);
                s = s.trim_end_matches(['-', '.']).to_owned();
            }
            if s.is_empty() {
                s.push('x');
            }
            s
        })
        .collect();
    let sum = Sha256::digest(rel.as_bytes());
    let id = format!(
        "{}-{}",
        parts.join("/"),
        hex(sum.get(..4).unwrap_or_default())
    );
    if valid_id(&id) {
        return id;
    }
    // Too long, or too deep: the hash alone of what is past the first part.
    format!(
        "{}/{}",
        parts.first().map_or("x", String::as_str),
        hex(sum.get(..8).unwrap_or_default())
    )
}

pub(crate) fn hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(b.len().saturating_mul(2));
    for x in b {
        let _ = write!(s, "{x:02x}");
    }
    s
}

// What an item's parts are reckoned to take, beside their strings and codec configurations,
// as fit's: headers, maps and pointers, rounded up.
const ITEM_BYTES: u64 = 512;
const RENDITION_BYTES: u64 = 384;
const TRACK_BYTES: u64 = 256;
const SUBTITLE_BYTES: u64 = 192;
const FILE_BYTES: u64 = 128;

fn len(s: &str) -> u64 {
    s.len() as u64
}

fn path_len(p: &std::path::Path) -> u64 {
    p.as_os_str().len() as u64
}

impl Item {
    /// Sets the memory the item is reckoned to take (fit's `reckon`), and returns it.
    pub(crate) fn reckon(&mut self) -> u64 {
        let file = |f: &File| {
            FILE_BYTES
                .saturating_add(len(&f.root))
                .saturating_add(len(&f.path))
                .saturating_add(path_len(&f.abs))
                .saturating_add(path_len(&f.real))
        };
        let mut n = ITEM_BYTES
            .saturating_add(len(&self.id).saturating_mul(2))
            .saturating_add(len(&self.title))
            .saturating_add(len(&self.root))
            .saturating_add(len(&self.path))
            .saturating_add(len(&self.error));
        for r in self.renditions.iter().chain(&self.audio) {
            n = n
                .saturating_add(RENDITION_BYTES)
                .saturating_add(file(&r.file))
                .saturating_add(len(&r.name))
                .saturating_add(len(&r.language))
                .saturating_add(len(&r.codecs))
                .saturating_add(len(&r.error));
            for t in &r.tracks {
                n = n
                    .saturating_add(TRACK_BYTES)
                    .saturating_add(len(&t.codec_string))
                    .saturating_add(len(&t.language));
            }
        }
        for s in &self.subtitles {
            n = n
                .saturating_add(SUBTITLE_BYTES)
                .saturating_add(file(&s.file))
                .saturating_add(len(&s.language))
                .saturating_add(len(&s.name))
                .saturating_add(len(&s.error));
        }
        for f in self.poster.iter().chain(&self.sidecar) {
            n = n.saturating_add(file(f));
        }
        self.bytes = n;
        n
    }

    /// The files of the item: its renditions, audio, subtitles, poster and sidecar.
    pub fn files(&self) -> impl Iterator<Item = &File> {
        self.renditions
            .iter()
            .chain(&self.audio)
            .map(|r| &r.file)
            .chain(self.subtitles.iter().map(|s| &s.file))
            .chain(self.poster.iter())
            .chain(self.sidecar.iter())
    }
}

// ---- as the Admin API shows them ----

/// `v` as compact JSON, as Go's `json.Marshal` writes it (fit's Admin API's answers).
#[must_use]
pub fn compact(v: &Value) -> String {
    let mut out = String::new();
    write_compact(v, &mut out);
    out
}

fn write_compact(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(n),
        Value::String(s) => akit_config::value::write_string(out, s),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_compact(x, out);
            }
            out.push(']');
        }
        Value::Object(m) => {
            out.push('{');
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                akit_config::value::write_string(out, k);
                out.push(':');
                write_compact(x, out);
            }
            out.push('}');
        }
    }
}

fn s(v: &str) -> Value {
    Value::String(v.to_owned())
}

fn n(v: u64) -> Value {
    Value::Number(v.to_string())
}

/// A duration in seconds, as Go's `Duration.Seconds()` writes it in JSON.
pub(crate) fn secs(d: Duration) -> Value {
    Value::Number(seconds_text(d))
}

/// `d` in seconds as the shortest decimal that reads back the same, as Go writes a float64.
pub(crate) fn seconds_text(d: Duration) -> String {
    let f = d.as_secs_f64();
    if f.fract() == 0.0 && f < 1e15 {
        format!("{f:.0}")
    } else {
        format!("{f}")
    }
}

/// A time as RFC 3339 with nanoseconds, in UTC.
pub(crate) fn time(t: SystemTime) -> Value {
    let (secs, nanos) = match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => (
            i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
            d.subsec_nanos(),
        ),
        Err(_) => (0, 0),
    };
    let mut b = Vec::new();
    akit_observe::time::rfc3339_nano(&mut b, secs, nanos, 0);
    Value::String(String::from_utf8_lossy(&b).into_owned())
}

fn obj(members: Vec<(&str, Value)>) -> Value {
    Value::Object(
        members
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
    )
}

fn file_members(f: &File) -> Vec<(&'static str, Value)> {
    vec![
        ("root", s(&f.root)),
        ("path", s(&f.path)),
        ("size", n(f.size)),
        ("modified", time(f.modified)),
    ]
}

impl File {
    /// The file as the Admin API shows it.
    #[must_use]
    pub fn to_value(&self) -> Value {
        obj(file_members(self))
    }
}

impl Track {
    /// The track as the Admin API shows it.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut m = vec![
            ("index", n(self.index as u64)),
            ("id", n(u64::from(self.id))),
            ("kind", s(self.kind)),
            ("codec", s(self.codec)),
        ];
        if !self.codec_string.is_empty() {
            m.push(("codec_string", s(&self.codec_string)));
        }
        if !self.language.is_empty() {
            m.push(("language", s(&self.language)));
        }
        for (k, v) in [
            ("width", self.width),
            ("height", self.height),
            ("sample_rate", self.sample_rate),
            ("channels", u32::from(self.channels)),
        ] {
            if v != 0 {
                m.push((k, n(u64::from(v))));
            }
        }
        m.extend([
            ("start", secs(self.start)),
            ("duration", secs(self.duration)),
            ("samples", n(u64::from(self.samples))),
            ("table_bytes", n(self.table_bytes)),
        ]);
        obj(m)
    }
}

impl Rendition {
    /// The rendition as the Admin API shows it.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut m = file_members(&self.file);
        m.push(("name", s(&self.name)));
        if !self.language.is_empty() {
            m.push(("language", s(&self.language)));
        }
        if self.default {
            m.push(("default", Value::Bool(true)));
        }
        m.extend([
            ("duration", secs(self.duration)),
            ("bitrate", n(self.bitrate)),
        ]);
        if self.width != 0 {
            m.push(("width", n(u64::from(self.width))));
        }
        if self.height != 0 {
            m.push(("height", n(u64::from(self.height))));
        }
        m.extend([
            ("codecs", s(&self.codecs)),
            (
                "tracks",
                Value::Array(self.tracks.iter().map(Track::to_value).collect()),
            ),
        ]);
        if !self.error.is_empty() {
            m.push(("error", s(&self.error)));
        }
        obj(m)
    }
}

impl Subtitle {
    /// The subtitle as the Admin API shows it.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut m = file_members(&self.file);
        m.push(("format", s(&self.format)));
        for (k, v) in [
            ("language", &self.language),
            ("name", &self.name),
            ("error", &self.error),
        ] {
            if !v.is_empty() {
                m.push((k, s(v)));
            }
        }
        for (k, v) in [("default", self.default), ("forced", self.forced)] {
            if v {
                m.push((k, Value::Bool(true)));
            }
        }
        obj(m)
    }
}

impl Item {
    /// The item as the Admin API shows it, its durations in seconds.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut m = vec![
            ("id", s(&self.id)),
            ("title", s(&self.title)),
            ("root", s(&self.root)),
            ("path", s(&self.path)),
            ("duration", secs(self.duration)),
            ("size", n(self.size)),
            ("modified", time(self.modified)),
            (
                "renditions",
                Value::Array(self.renditions.iter().map(Rendition::to_value).collect()),
            ),
            (
                "audio",
                Value::Array(self.audio.iter().map(Rendition::to_value).collect()),
            ),
            (
                "subtitles",
                Value::Array(self.subtitles.iter().map(Subtitle::to_value).collect()),
            ),
        ];
        if let Some(p) = &self.poster {
            m.push(("poster", p.to_value()));
        }
        if let Some(p) = &self.sidecar {
            m.push(("sidecar", p.to_value()));
        }
        if !self.error.is_empty() {
            m.push(("error", s(&self.error)));
        }
        obj(m)
    }
}

impl Problem {
    /// The problem as the Admin API shows it.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut m = Vec::new();
        if !self.root.is_empty() {
            m.push(("root", s(&self.root)));
        }
        if !self.path.is_empty() {
            m.push(("path", s(&self.path)));
        }
        m.push(("error", s(&self.error)));
        obj(m)
    }
}
