//! What the library writes, and where (I149, fit's `medialib/paths.go`): names valid on every
//! system akit and fit run on, paths under a root within bounds, and never a file out of its
//! root, whatever `follow_symlinks` says. And the errors of a library's files managed.

use std::io;
use std::path::{Path, PathBuf};

use super::Root;
use super::files::inside;

/// The bounds of a path the library writes.
const MAX_NAME_BYTES: usize = 255;
const MAX_PATH_BYTES: usize = 1024;

/// The errors of a library's files managed: uploads, and changes of its files (fit's
/// `medialib` errors of I139).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileError {
    /// The library was removed.
    Removed,
    /// The library's file pool refused the work, or gave it up.
    Busy,
    /// The library has no `upload_dir`.
    NoUploads,
    /// No such upload under way: never begun, finished, cancelled, expired, or given up.
    NoUpload,
    /// Another request writes to the upload.
    UploadBusy,
    /// The write does not start where the upload is at, which it says.
    Offset(u64),
    /// `max_uploads` uploads are under way.
    TooMany(usize),
    /// Not enough disk space, as it says.
    NoSpace(String),
    /// An upload's size is 0.
    UploadSize,
    /// Past a bound, as it says.
    TooLarge(String),
    /// Not a path the library writes.
    BadPath,
    /// Not a media file, subtitle, picture or sidecar.
    NotMedia,
    /// A file is there already, or the ID is taken, as it says.
    Exists(String),
    /// The path leads out of its root.
    Outside,
    /// The file changed since it was scanned, as it says.
    Changed(String),
    /// No such item in the catalog.
    NoItem,
    /// The library has no such root.
    NoRoot(String),
    /// A rename or move to where the item is.
    NoChange,
    /// A sidecar the library would not read, as it says.
    BadSidecar(String),
    /// A new name the library would read as something else, as it says.
    BadName(String),
    /// The disk said no.
    Io(String),
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileError::Removed => f.write_str("the library was removed"),
            FileError::Busy => f.write_str("the library's file pool is busy"),
            FileError::NoUploads => {
                f.write_str("the library takes no uploads: it has no upload_dir")
            }
            FileError::NoUpload => f.write_str("no such upload under way"),
            FileError::UploadBusy => f.write_str("another request writes to the upload"),
            FileError::Offset(at) => write!(
                f,
                "the write does not start where the upload is at: it is at {at}"
            ),
            FileError::TooMany(n) => write!(f, "max_uploads uploads are under way: {n}"),
            FileError::NoSpace(m) => write!(f, "not enough disk space: {m}"),
            FileError::UploadSize => f.write_str("an upload's size is 1 byte or more"),
            FileError::TooLarge(m) => write!(f, "past the library's bounds: {m}"),
            FileError::BadPath => f.write_str("not a path the library writes"),
            FileError::NotMedia => f.write_str("not a media file, subtitle, picture or sidecar"),
            FileError::Exists(m) => write!(f, "a file is there already, or the ID is taken: {m}"),
            FileError::Outside => f.write_str("the file leads out of its root"),
            FileError::Changed(m) => write!(f, "the file changed since it was scanned: {m}"),
            FileError::NoItem => f.write_str("no such item in the catalog"),
            FileError::NoRoot(r) => {
                write!(
                    f,
                    "no such item in the catalog: the library has no root {r:?}"
                )
            }
            FileError::NoChange => f.write_str("the item is there already"),
            FileError::BadSidecar(m) => write!(f, "not a sidecar the library reads: {m}"),
            FileError::BadName(m) => write!(f, "not a path the library writes: {m}"),
            FileError::Io(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for FileError {}

impl FileError {
    /// The HTTP status the Admin API answers it with (fit's `fileStatus`).
    #[must_use]
    pub fn status(&self) -> u16 {
        match self {
            FileError::NoItem
            | FileError::NoRoot(_)
            | FileError::NoUpload
            | FileError::NoUploads
            | FileError::Removed => 404,
            FileError::BadPath
            | FileError::BadName(_)
            | FileError::NotMedia
            | FileError::UploadSize
            | FileError::NoChange
            | FileError::BadSidecar(_) => 400,
            FileError::Outside => 403,
            FileError::Exists(_)
            | FileError::Changed(_)
            | FileError::Offset(_)
            | FileError::UploadBusy => 409,
            FileError::TooLarge(_) => 413,
            FileError::TooMany(_) => 429,
            FileError::Busy => 503,
            FileError::NoSpace(_) => 507,
            FileError::Io(_) => 500,
        }
    }

    pub(crate) fn io(e: &io::Error) -> FileError {
        FileError::Io(e.to_string())
    }
}

/// Whether `name` is a name of a file or directory the library writes, the same on every system
/// akit and fit run on (fit's `validName`).
pub(crate) fn valid_name(name: &str) -> bool {
    if name.is_empty()
        || name.len() > MAX_NAME_BYTES
        || name.starts_with('.')
        || name.ends_with(' ')
        || name.ends_with('.')
    {
        return false;
    }
    if name
        .chars()
        .any(|c| c < ' ' || c == '\u{7f}' || "/\\:*?\"<>|".contains(c))
    {
        return false;
    }
    let stem = name.split('.').next().unwrap_or("");
    let upper = stem.trim_end_matches(' ').to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return false;
    }
    let u = stem.to_ascii_uppercase();
    !(u.len() == 4
        && (u.starts_with("COM") || u.starts_with("LPT"))
        && u.as_bytes().get(3).is_some_and(u8::is_ascii_digit))
}

/// `p`, a slash path under a root the library writes, cleaned of a trailing '/' (fit's
/// `cleanRel`): each of its parts a valid name, at most [`super::MAX_DEPTH`] parts and 1,024
/// bytes. "" is a root itself, taken when `dir` is set.
pub(crate) fn clean_rel(p: &str, dir: bool) -> Result<String, FileError> {
    let p = p.strip_suffix('/').unwrap_or(p);
    if p.is_empty() {
        return if dir {
            Ok(String::new())
        } else {
            Err(FileError::BadPath)
        };
    }
    if p.len() > MAX_PATH_BYTES || p.matches('/').count() >= super::MAX_DEPTH {
        return Err(FileError::BadPath);
    }
    if !p.split('/').all(valid_name) {
        return Err(FileError::BadPath);
    }
    Ok(p.to_owned())
}

/// The path `rel`, a slash path, opens under `r`.
pub(crate) fn path_of(r: &Root, rel: &str) -> PathBuf {
    let mut out = r.abs.clone();
    for part in rel.split('/').filter(|s| !s.is_empty()) {
        out.push(part);
    }
    out
}

/// Checks that the directory of `rel` under `r`, as far as it exists, lies inside `r` once its
/// symbolic links are resolved, whatever `follow_symlinks` says: the library never writes out of
/// its roots (fit's `writable`). `rel`'s path, or [`FileError::Outside`]. It blocks.
pub(crate) fn writable(r: &Root, rel: &str) -> Result<PathBuf, FileError> {
    let real = std::fs::canonicalize(&r.abs).map_err(|e| FileError::io(&e))?;
    let abs = path_of(r, rel);
    let mut dir = abs.parent().map(Path::to_path_buf).unwrap_or_default();
    loop {
        match std::fs::canonicalize(&dir) {
            Ok(t) => {
                if !inside(&real, &t) {
                    return Err(FileError::Outside);
                }
                return Ok(abs);
            }
            Err(e) => {
                let parent = dir.parent().map(Path::to_path_buf);
                match parent {
                    Some(p)
                        if e.kind() == io::ErrorKind::NotFound
                            && p != dir
                            && inside(&r.abs, &p) =>
                    {
                        dir = p;
                    }
                    _ => return Err(FileError::io(&e)),
                }
            }
        }
    }
}

/// Writes `data` to `abs` through a temporary file beside it, which a scan does not look at
/// (`.akit-tmp-`). It blocks.
pub(crate) fn write_atomic(abs: &Path, data: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    let mut rnd = [0u8; 8];
    let _ = akio_crypto::rand::fill_random(&mut rnd);
    let dir = abs.parent().unwrap_or_else(|| Path::new("."));
    let tmp = dir.join(format!(".akit-tmp-{}", super::catalog::hex(&rnd)));
    let r = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, abs)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

/// The bytes free to the process on the disk of `dir`, or `None` when it cannot be told.
pub(crate) fn disk_free(dir: &Path) -> Option<u64> {
    akio_sys::fs::space(dir).ok().map(|s| s.available)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// fit's `TestValidName` and `TestCleanRel`.
    #[test]
    fn names_and_paths() {
        for ok in ["a", "bbb.mp4", "a b.mp4", "Ünïcode.mkv", "console.mp4"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in [
            "", ".hidden", "a.", "a ", "a/b", "a\\b", "a:b", "a*b", "a?b", "a\"b", "a<b", "a>b",
            "a|b", "a\u{1}b", "CON", "con.mp4", "Nul", "COM1.mp4", "lpt9", "CONIN$",
        ] {
            assert!(!valid_name(bad), "{bad:?}");
        }
        assert_eq!(clean_rel("films/a.mp4", false).unwrap(), "films/a.mp4");
        assert_eq!(clean_rel("films/", true).unwrap(), "films");
        assert_eq!(clean_rel("", true).unwrap(), "");
        for bad in [
            "",
            "/a",
            "a//b",
            "../a",
            "a/../b",
            "a/./b",
            &"x/".repeat(40),
        ] {
            assert!(clean_rel(bad, false).is_err(), "{bad:?}");
        }
        assert!(clean_rel(&"a".repeat(1025), false).is_err());
    }
}
