//! A library's files opened, its sidecars parsed and its subtitles turned into WebVTT (fit's
//! `medialib/open.go`, `sidecar.go` and `subtitle.go`). Everything here blocks: it runs on the
//! library's file pool or its scanner.

use std::fs;
use std::io;
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::time::SystemTime;

use akit_config::value::Value;

use super::Error;
use super::catalog::{File, valid_id};

/// A file of a library opened for reading, as the catalog has it.
#[derive(Debug)]
pub struct Handle {
    file: fs::File,
    /// The file's size.
    pub size: u64,
}

impl Handle {
    /// Reads `buf.len()` bytes at `off`, fewer only at the end of the file.
    ///
    /// # Errors
    ///
    /// The read's.
    pub fn read_at(&self, buf: &mut [u8], off: u64) -> io::Result<usize> {
        let mut done = 0usize;
        while let Some(rest) = buf.get_mut(done..) {
            if rest.is_empty() {
                break;
            }
            let at = off.saturating_add(done as u64);
            match self.file.read_at(rest, at) {
                Ok(0) => break,
                Ok(n) => done = done.saturating_add(n),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(done)
    }

    /// `len` bytes at `off`, read whole.
    ///
    /// # Errors
    ///
    /// The read's, or `UnexpectedEof` for a file shorter than that.
    pub fn read_exact(&self, off: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut b = vec![0u8; len];
        if self.read_at(&mut b, off)? != len {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        Ok(b)
    }

    /// The file, for a handler that sends a range of it as it is.
    #[must_use]
    pub fn file(&self) -> &fs::File {
        &self.file
    }

    /// The file, given up.
    #[must_use]
    pub fn into_file(self) -> fs::File {
        self.file
    }
}

/// Whether `p` is `dir` or lies inside it, both clean absolute paths (fit's `inside`).
pub(crate) fn inside(dir: &Path, p: &Path) -> bool {
    p.starts_with(dir)
}

fn not_found(e: io::Error) -> Error {
    if e.kind() == io::ErrorKind::NotFound {
        Error::Changed
    } else {
        Error::Io(e.to_string())
    }
}

/// Opens `f`: refused with [`Error::Outside`] when its path now leads out of its root and
/// `follow` is false, with [`Error::Changed`] when it is gone or is not as the catalog has it, of
/// the same size and time (fit's `openFile`).
pub(crate) fn open_file(f: &File, follow: bool) -> Result<Handle, Error> {
    let target = fs::canonicalize(&f.abs).map_err(not_found)?;
    if !follow && !inside(&f.real, &target) {
        return Err(Error::Outside);
    }
    let file = fs::File::open(&f.abs).map_err(not_found)?;
    let meta = file.metadata().map_err(|e| Error::Io(e.to_string()))?;
    if !meta.is_file() {
        return Err(Error::Io("not a regular file".to_owned()));
    }
    if meta.len() != f.size || modified(&meta) != f.modified {
        return Err(Error::Changed);
    }
    Ok(Handle {
        file,
        size: meta.len(),
    })
}

/// A file's time, the epoch when the system does not give it.
pub(crate) fn modified(m: &fs::Metadata) -> SystemTime {
    m.modified().unwrap_or(SystemTime::UNIX_EPOCH)
}

/// An item's `<name>.json` (fit's `Sidecar`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Sidecar {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) poster: String,
    pub(crate) renditions: Vec<SidecarFile>,
    pub(crate) audio: Vec<SidecarFile>,
    pub(crate) subtitles: Vec<SidecarFile>,
}

/// A rendition, an audio file or a subtitle a sidecar names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SidecarFile {
    pub(crate) file: String,
    pub(crate) name: String,
    pub(crate) language: String,
    pub(crate) default: bool,
    /// Subtitles only.
    pub(crate) forced: bool,
}

/// The longest a sidecar's title, name or language may be.
const MAX_SIDECAR_TEXT: usize = 1024;

/// Reads and parses the sidecar `f`, whose size the scan bounds.
pub(crate) fn read_sidecar(f: &File, follow: bool) -> Result<Sidecar, String> {
    let h = open_file(f, follow).map_err(|e| e.to_string())?;
    let len = usize::try_from(h.size).map_err(|_| "too large".to_owned())?;
    let data = h.read_exact(0, len).map_err(|e| e.to_string())?;
    parse_sidecar(&data)
}

/// Parses a sidecar strictly, as fit's `parseSidecar`: an unknown field, a value of another
/// type, or a text that is too long, is an error, its message Go's where fit's tests read it.
pub(crate) fn parse_sidecar(data: &[u8]) -> Result<Sidecar, String> {
    let text = std::str::from_utf8(data).map_err(|_| "not UTF-8".to_owned())?;
    let v = match akit_config::value::parse(text) {
        Ok(v) => v,
        Err(errs) => {
            // Go's decoder reads one value and says so of what follows it.
            if let Some(end) = first_value_end(text) {
                if !text.get(end..).unwrap_or("").trim().is_empty() {
                    return Err("more than one JSON value".to_owned());
                }
            }
            return Err(errs
                .as_slice()
                .first()
                .map_or_else(|| "not JSON".to_owned(), |e| e.msg.clone()));
        }
    };
    let Value::Object(members) = v else {
        return Err("json: cannot unmarshal into a sidecar: not an object".to_owned());
    };
    let mut sd = Sidecar::default();
    for (k, v) in members {
        match k.as_str() {
            "id" => sd.id = string(&k, v)?,
            "title" => sd.title = string(&k, v)?,
            "poster" => sd.poster = string(&k, v)?,
            "renditions" => sd.renditions = files(&k, v, false)?,
            "audio" => sd.audio = files(&k, v, false)?,
            "subtitles" => sd.subtitles = files(&k, v, true)?,
            other => return Err(format!("json: unknown field {other:?}")),
        }
    }
    if !sd.id.is_empty() && !valid_id(&sd.id) {
        return Err(format!(
            "id {:?} is not an item's ID: parts of 1 to 128 letters, digits, '.', '_', '~' and '-', not starting with '.', joined by '/'",
            sd.id
        ));
    }
    let texts = std::iter::once(&sd.title).chain(
        sd.renditions
            .iter()
            .chain(&sd.audio)
            .chain(&sd.subtitles)
            .flat_map(|f| [&f.name, &f.language]),
    );
    for t in texts {
        if t.len() > MAX_SIDECAR_TEXT {
            return Err(format!(
                "a title, name or language is not UTF-8, or longer than {MAX_SIDECAR_TEXT} bytes"
            ));
        }
    }
    Ok(sd)
}

/// Where the first JSON value of `text` ends, when it is an object or array whose brackets
/// close: so that `{} {}` is told from a value that does not parse.
fn first_value_end(text: &str) -> Option<usize> {
    let start = text.find(|c: char| !c.is_whitespace())?;
    let mut depth = 0usize;
    let mut in_str = false;
    let mut esc = false;
    for (i, c) in text.char_indices().skip_while(|(i, _)| *i < start) {
        if in_str {
            match c {
                _ if esc => esc = false,
                '\\' => esc = true,
                '"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' | '[' => depth = depth.saturating_add(1),
            '}' | ']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i.saturating_add(1));
                }
            }
            _ if depth == 0 => return None,
            _ => {}
        }
    }
    None
}

fn string(k: &str, v: Value) -> Result<String, String> {
    match v {
        Value::String(s) => Ok(s),
        Value::Null => Ok(String::new()),
        _ => Err(format!(
            "json: cannot unmarshal into Go struct field Sidecar.{k} of type string"
        )),
    }
}

fn boolean(k: &str, v: Value) -> Result<bool, String> {
    match v {
        Value::Bool(b) => Ok(b),
        Value::Null => Ok(false),
        _ => Err(format!(
            "json: cannot unmarshal into Go struct field {k} of type bool"
        )),
    }
}

fn files(k: &str, v: Value, subtitle: bool) -> Result<Vec<SidecarFile>, String> {
    let list = match v {
        Value::Array(list) => list,
        Value::Null => return Ok(Vec::new()),
        _ => {
            return Err(format!(
                "json: cannot unmarshal into Go struct field Sidecar.{k} of type array"
            ));
        }
    };
    let mut out = Vec::with_capacity(list.len());
    for v in list {
        let Value::Object(members) = v else {
            return Err(format!(
                "json: cannot unmarshal into Go struct field Sidecar.{k} of type object"
            ));
        };
        let mut f = SidecarFile::default();
        for (mk, mv) in members {
            match mk.as_str() {
                "file" => f.file = string(&mk, mv)?,
                "name" => f.name = string(&mk, mv)?,
                "language" => f.language = string(&mk, mv)?,
                "default" => f.default = boolean(&mk, mv)?,
                "forced" if subtitle => f.forced = boolean(&mk, mv)?,
                other => return Err(format!("json: unknown field {other:?}")),
            }
        }
        out.push(f);
    }
    Ok(out)
}

const BOM: &[u8] = b"\xef\xbb\xbf";

/// `data`, a subtitle file of `format` `vtt` or `srt`, as WebVTT (fit's `ToWebVTT`): a WebVTT
/// file as it is, without a byte order mark, and an SRT file turned into one, its times written
/// as WebVTT writes them and what SRT adds after them left out.
///
/// # Errors
///
/// What makes it neither.
pub fn to_webvtt(format: &str, data: &[u8]) -> Result<Vec<u8>, String> {
    let data = data.strip_prefix(BOM).unwrap_or(data);
    match format {
        "vtt" => {
            let next = data.get(6);
            if !data.starts_with(b"WEBVTT") || next.is_some_and(|b| !b" \t\r\n".contains(b)) {
                return Err("not a WebVTT file: it does not start with WEBVTT".to_owned());
            }
            Ok(data.to_vec())
        }
        "srt" => srt_to_vtt(data),
        other => Err(format!("subtitles of format {other:?}")),
    }
}

/// An SRT cue's times, as fit's `srtTime` matches them: `h:m:s,ms --> h:m:s,ms` (or with dots),
/// spaces around; the parts' numbers.
fn srt_times(line: &str) -> Option<([&str; 4], [&str; 4])> {
    let (a, b) = line.split_once("-->")?;
    let first = srt_time(a.trim_start(), true)?;
    let second = srt_time(b.trim_start(), false)?;
    Some((first, second))
}

/// One time: up to 3 digits of hours, 2 of minutes and seconds, 3 of milliseconds; the first
/// must end the text but for spaces, the second may be followed by anything.
fn srt_time(s: &str, whole: bool) -> Option<[&str; 4]> {
    let mut out = [""; 4];
    let mut rest = s;
    for (i, (max, sep)) in [(3usize, ':'), (2, ':'), (2, ','), (3, ' ')]
        .into_iter()
        .enumerate()
    {
        let n = rest.bytes().take_while(u8::is_ascii_digit).count().min(max);
        if n == 0 {
            return None;
        }
        let (digits, after) = rest.split_at(n);
        *out.get_mut(i)? = digits;
        rest = after;
        if i < 2 {
            rest = rest.strip_prefix(sep)?;
        } else if i == 2 {
            rest = rest.strip_prefix([',', '.'])?;
        }
    }
    if whole && !rest.trim().is_empty() {
        return None;
    }
    Some(out)
}

fn vtt_time(p: [&str; 4]) -> String {
    let n = |s: &str| s.parse::<u32>().unwrap_or(0);
    let mut ms = n(p[3]);
    // Milliseconds written with fewer than three digits are tenths or hundredths.
    for _ in p[3].len()..3 {
        ms = ms.saturating_mul(10);
    }
    format!("{:02}:{:02}:{:02}.{ms:03}", n(p[0]), n(p[1]), n(p[2]))
}

fn srt_to_vtt(data: &[u8]) -> Result<Vec<u8>, String> {
    let text = String::from_utf8_lossy(data)
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut out =
        String::with_capacity(text.len().saturating_add(text.len() / 8).saturating_add(16));
    out.push_str("WEBVTT\n\n");
    let mut cues = 0usize;
    for line in text.split_inclusive('\n') {
        let line = line.strip_suffix('\n').unwrap_or(line);
        if let Some((a, b)) = srt_times(line) {
            cues = cues.saturating_add(1);
            out.push_str(&vtt_time(a));
            out.push_str(" --> ");
            out.push_str(&vtt_time(b));
            out.push('\n');
            continue;
        }
        if cues == 0 {
            // The first cue's number, and what is before it.
            continue;
        }
        // A cue's text may not hold -->, which would start a cue.
        out.push_str(&line.replace("-->", "--&gt;"));
        out.push('\n');
    }
    if cues == 0 {
        return Err("not an SRT file: it has no cue".to_owned());
    }
    Ok(out.into_bytes())
}
