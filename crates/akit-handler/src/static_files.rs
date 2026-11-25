//! `static`: the files under a root directory, as fit's `handler/static`, on akio-http's
//! [`Files`].
//!
//! `Files` does the serving: `ETag` and `Last-Modified`, conditional requests (`If-Match`,
//! `If-None-Match`, `If-Modified-Since`, `If-Unmodified-Since`) and ranges (`If-Range`, several
//! ranges as `multipart/byteranges`), the `Content-Type` by extension, the directory's index
//! file, a file's `.br`, `.zst` or `.gz` twin for a client that accepts it (fit's
//! `precompressed`), small files kept in memory and larger ones sent from disk (`sendfile` on
//! plain HTTP/1, from a kept handle). Only a file it knows and
//! trusts is answered on the I/O thread; every look at the disk (`stat`, `open`, reading a file
//! in) runs on akio's file pool, bounded in threads and queue, and a request past the queue is
//! answered 503 (rules.md rule 4).
//!
//! `Files` also does fit's `try_files` (`Files::try_paths`, with what the candidates resolved a
//! path to kept and trusted as a file is), `browse` (`Files::list_directories`, fit's page, read
//! on the pool) and the choice of a twin by the client's q values among the encodings
//! `precompressed` lists (`Files::twins`).
//!
//! What this module adds before `Files` sees a request is fit's: only `GET` and `HEAD`, others
//! answered 405 with `Allow: GET, HEAD`; the canonical path the route matched (akit-listener's
//! `CANONICAL_PATH`, dot and empty segments removed, as fit's listener gives its handler) with
//! `strip_prefix` cut from it, a path without the prefix answered 404, the rest given to
//! `Files::serve`; a path with a `..` segment, a NUL or a backslash, escaped or not, refused with
//! 400, so no request names a file outside the root; and, with `dotfiles` `deny` (the default), a
//! path with a segment starting with a dot answered 404 but for `/.well-known/` at the root. These
//! look at the path alone, without allocating; the path is copied for `Files::serve` only with a
//! prefix or a path that was not canonical. With `deny`, `Files::hide` passes over `try_files`
//! candidates that such a segment names, as fit's handler does.
//!
//! `Files` also answers as fit's handler (akio ADR-124): `index` tried in order, a directory
//! asked for without its slash redirected (but with `try_files`), a file asked for with one 404,
//! 403 for a file that may not be read, only regular files opened, Go's content types (the
//! system's `mime.types` included) with `default_type` or else the type sniffed from the first
//! 512 bytes, fit's short error bodies, listings and redirects naming the whole path below
//! `strip_prefix`, at most `max_cache_bytes` kept in memory, and a request the file pool has not
//! answered within fit's 10 s answered 503.
//!
//! Where `Files` still differs from fit's handler, `docs/progress.md` says (I12, I13).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use akio_http::files::{Files, MimeTypes};
use akio_http::{Context, Method, Response};
use akit_config::Dotfiles;
use akit_runtime::{
    CANONICAL_PATH, Handler, HandlerParams, Provision, ProvisionError, Provisioned,
};

/// The handler: fit's checks, then `Files`.
struct Static {
    files: Files,
    /// `strip_prefix`, empty for none.
    strip_prefix: String,
    /// `try_files` given as an empty list, which finds nothing: every request is answered 404.
    find_nothing: bool,
    hide_dotfiles: bool,
    bad_request: Response,
    not_found: Response,
    method_not_allowed: Response,
}

impl akio_http::Service for Static {
    fn call(&self, ctx: &mut Context) {
        if !matches!(ctx.method(), Method::Get | Method::Head) {
            ctx.respond(self.method_not_allowed.clone());
            return;
        }
        // The canonical path, what the route matched (akit-listener's, when it differs from the
        // path sent: dot and empty segments removed, as fit's listener removes them before its
        // handler sees the path).
        let canonical = ctx.local(CANONICAL_PATH);
        let as_sent = canonical.is_none();
        let full = canonical.unwrap_or_else(|| ctx.path());
        let Some(path) = full.strip_prefix(self.strip_prefix.as_str()) else {
            ctx.respond(self.not_found.clone());
            return;
        };
        match scan(path) {
            Scan::Unsafe => ctx.respond(self.bad_request.clone()),
            Scan::Hidden if self.hide_dotfiles => ctx.respond(self.not_found.clone()),
            Scan::Hidden | Scan::Plain if self.find_nothing => ctx.respond(self.not_found.clone()),
            Scan::Hidden | Scan::Plain if as_sent && self.strip_prefix.is_empty() => {
                self.files.call(ctx);
            }
            Scan::Hidden | Scan::Plain => {
                // The path is borrowed from the request, which `Files::serve_prefixed` takes
                // alongside it: a copy, for a handler with a prefix or a path that was not
                // canonical only. Listings and redirects name the whole canonical path.
                let rest = path.to_owned();
                self.files.serve_prefixed(ctx, &self.strip_prefix, &rest);
            }
        }
    }
}

impl Handler for Static {}

pub(crate) fn new(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    let HandlerParams::Static(s) = p.params() else {
        return Err(ProvisionError::new(
            "the static handler's params are not static's",
        ));
    };
    // Provisioning runs where the config is compiled, off the I/O threads: the root is looked at
    // once here, as fit's Provision does.
    let root = std::path::absolute(&s.root).map_err(|e| ProvisionError::field("root", e))?;
    let md = std::fs::metadata(&root).map_err(|e| ProvisionError::field("root", e))?;
    if !md.is_dir() {
        return Err(ProvisionError::field(
            "root",
            format!("{} is not a directory", root.display()),
        ));
    }
    Ok(Provisioned::new(Arc::new(Static {
        files: files(root, s),
        strip_prefix: s.strip_prefix.clone(),
        find_nothing: s.try_files.as_ref().is_some_and(Vec::is_empty),
        hide_dotfiles: s.dotfiles == Dotfiles::Deny,
        bad_request: error(400, "400 Bad Request\n"),
        not_found: error(404, "404 Not Found\n"),
        method_not_allowed: error(405, "405 Method Not Allowed\n").header("Allow", "GET, HEAD"),
    }) as Arc<dyn Handler>))
}

/// The `Files` for the params. fit's handler follows changes on Linux through a watch of the
/// directories under the root, and trusts what it knows for `check_interval` elsewhere and with
/// a `check_interval` of 0 looks at the disk for every request: `Files` is set up alike.
fn files(root: PathBuf, s: &akit_config::Static) -> Files {
    let check: Duration = s.check_interval;
    let twins: Vec<&str> = s.precompressed.iter().map(|e| e.name()).collect();
    let copies: Vec<&str> = s.compress.iter().map(|e| e.name()).collect();
    let tries: Vec<&str> = s.try_files.iter().flatten().map(String::as_str).collect();
    let index: Vec<&str> = s.index.iter().map(String::as_str).collect();
    let mut files = Files::new(root)
        .index_files(&index)
        // fit redirects a directory asked for without its slash, but with try_files, whose
        // candidates say whether they name one.
        .redirect_directories(s.try_files.is_none())
        // Go's types, with the system's mime.types (read here, where the config is compiled).
        .mime_types(MimeTypes::system())
        .default_type(s.default_type.as_deref())
        .sniff(true)
        .error_response(|status| error(status, &fit_text(status)))
        .max_cache_bytes(s.max_cache_bytes)
        .disk_timeout(Some(FILE_TIMEOUT))
        .max_cached_bytes(s.max_file_bytes)
        .check_interval(check)
        .watch(cfg!(target_os = "linux") && !check.is_zero())
        .twins(&twins)
        .compress(&copies)
        .try_paths(&tries)
        .list_directories(if s.browse { s.browse_max_entries } else { 0 });
    if s.dotfiles == Dotfiles::Deny {
        files = files.hide(|path| scan_decoded(path) != Scan::Plain);
    }
    files
}

/// fit's `DefaultFileTimeout`: how long a request waits on the file pool before it is answered
/// 503.
const FILE_TIMEOUT: Duration = Duration::from_secs(10);

/// fit's short text body for `status`: `"404 Not Found\n"`.
fn fit_text(status: u16) -> String {
    let reason = match status {
        301 => "Moved Permanently",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    };
    format!("{status} {reason}\n")
}

/// One of fit's short text answers, built once; each request sends a copy.
fn error(status: u16, body: &str) -> Response {
    Response::with_body(
        status,
        "text/plain; charset=utf-8",
        body.as_bytes().to_vec(),
    )
}

/// What a request path holds, as fit's `dotfile.Scan` says it of the decoded path.
#[derive(Debug, PartialEq, Eq)]
enum Scan {
    /// A `..` segment, a NUL or a backslash, or an escape that is not one: what could name a
    /// file outside the root, or another file on some systems. Answered 400.
    Unsafe,
    /// A segment starting with a dot (not `.` itself, nor `.well-known` as the first segment).
    Hidden,
    /// Neither.
    Plain,
}

/// fit's directory for ACME's HTTP-01 challenges and `security.txt`, which `dotfiles` `deny`
/// serves when it is the path's first segment.
const WELL_KNOWN: &[u8] = b".well-known";

/// Looks at the segments of `raw`, a request path as sent, percent-decoding as it goes, in one
/// pass and without allocating. The whole path is looked at: `Unsafe` wins over `Hidden`.
fn scan(raw: &str) -> Scan {
    let mut s = Scanner::default();
    let mut bytes = raw.bytes();
    while let Some(c) = bytes.next() {
        let mut b = c;
        if b == b'%' {
            let hex = |c: Option<u8>| c.and_then(|c| char::from(c).to_digit(16));
            let (Some(hi), Some(lo)) = (hex(bytes.next()), hex(bytes.next())) else {
                return Scan::Unsafe;
            };
            // Two hex digits make a byte, at most 255.
            let Some(v) = hi
                .checked_mul(16)
                .and_then(|h| h.checked_add(lo))
                .and_then(|v| u8::try_from(v).ok())
            else {
                return Scan::Unsafe;
            };
            b = v;
        }
        s.byte(b);
    }
    s.end_segment();
    if s.unsafe_ {
        Scan::Unsafe
    } else if s.hidden {
        Scan::Hidden
    } else {
        Scan::Plain
    }
}

/// [`scan`] of a path already percent-decoded, as `Files::hide` is given: a `%` is a byte as
/// any other.
fn scan_decoded(path: &str) -> Scan {
    let mut s = Scanner::default();
    for b in path.bytes() {
        s.byte(b);
    }
    s.end_segment();
    if s.unsafe_ {
        Scan::Unsafe
    } else if s.hidden {
        Scan::Hidden
    } else {
        Scan::Plain
    }
}

/// [`scan`]'s state: what it found so far, and the segment being read (its decoded length and
/// first bytes, which tell a dot segment and `.well-known` from the rest).
#[derive(Default)]
struct Scanner {
    unsafe_: bool,
    hidden: bool,
    /// A segment with a name was read.
    named: bool,
    len: usize,
    head: [u8; WELL_KNOWN.len()],
}

impl Scanner {
    fn byte(&mut self, b: u8) {
        match b {
            0 | b'\\' => self.unsafe_ = true,
            b'/' => self.end_segment(),
            _ => {
                if let Some(slot) = self.head.get_mut(self.len) {
                    *slot = b;
                }
                self.len = self.len.saturating_add(1);
            }
        }
    }

    fn end_segment(&mut self) {
        let len = self.len;
        self.len = 0;
        let head = self.head.get(..len.min(WELL_KNOWN.len())).unwrap_or(&[]);
        if len == 0 || head == b"." {
            // No name: the directory the path is at.
            return;
        }
        if head.first() == Some(&b'.') {
            if head == b".." {
                self.unsafe_ = true;
            } else if self.named || len != WELL_KNOWN.len() || head != WELL_KNOWN {
                self.hidden = true;
            }
        }
        self.named = true;
    }
}

#[cfg(test)]
mod tests {
    use super::{Scan, scan, scan_decoded};

    #[test]
    fn decoded_paths() {
        // What Files::hide is given is decoded already: a % is a byte as any other.
        assert_eq!(scan_decoded("/a/%2e%2e/b"), Scan::Plain);
        assert_eq!(scan_decoded("/a%"), Scan::Plain);
        assert_eq!(scan_decoded("/.env"), Scan::Hidden);
        assert_eq!(scan_decoded("/a/.well-known/x"), Scan::Hidden);
        assert_eq!(scan_decoded("/.well-known/x"), Scan::Plain);
        assert_eq!(scan_decoded("/a/../b"), Scan::Unsafe);
    }

    #[test]
    fn traversal_is_unsafe() {
        // fit's TestTraversal's paths, as sent.
        for p in [
            "/../secret.txt",
            "/a/../../secret.txt",
            "/%2e%2e/secret.txt",
            "/%2E%2e/secret.txt",
            "/..%2fsecret.txt",
            "/..%5csecret.txt",
            "/ok.txt%00",
            "/..",
            "/x/../ok.txt",
            "/a\\b",
            "/a/..",
            "..",
            // An escape that is not one.
            "/a%2",
            "/a%zz",
            "/%",
        ] {
            assert_eq!(scan(p), Scan::Unsafe, "{p}");
        }
        // Unsafe wins over hidden.
        assert_eq!(scan("/.git/../x"), Scan::Unsafe);
    }

    #[test]
    fn dotfiles_are_hidden() {
        for p in [
            "/..dots",
            "/.env",
            "/.git/config",
            "/a/.htpasswd",
            "/%2egit/config",
            "/a/.well-known/x",
            "/.well-knownx/a",
            "/.well-know",
        ] {
            assert_eq!(scan(p), Scan::Hidden, "{p}");
        }
    }

    #[test]
    fn plain_paths() {
        for p in [
            "/",
            "",
            "/hello.txt",
            "//hello.txt",
            "/./hello.txt",
            "/docs/a%20b.txt",
            "/a.b/c..d/e.",
            "/.well-known/acme-challenge/x",
            "/.well-known",
            "/%2ewell-known/x",
            "/a/./b/",
            "/caf%C3%A9",
        ] {
            assert_eq!(scan(p), Scan::Plain, "{p}");
        }
    }
}
