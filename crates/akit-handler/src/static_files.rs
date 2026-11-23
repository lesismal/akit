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
//! What this module adds before `Files` sees a request is fit's: only `GET` and `HEAD`, others
//! answered 405 with `Allow: GET, HEAD`; a path with a `..` segment, a NUL or a backslash,
//! escaped or not, refused with 400, so no request names a file outside the root; and, with
//! `dotfiles` `deny` (the default), a path with a segment starting with a dot answered 404 but
//! for `/.well-known/` at the root. These look at the request path alone, without allocating.
//!
//! Where `Files` differs from fit's handler, and the akio API requests that will close the gap,
//! `docs/progress.md` says (I12, I13). `try_files` and `browse` wait on requests A17 and A18:
//! akit-config refuses them as not supported yet.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use akio_http::files::Files;
use akio_http::{Context, Method, Response};
use akit_config::Dotfiles;
use akit_runtime::{Handler, HandlerParams, Provision, ProvisionError, Provisioned};

/// The handler: fit's checks, then `Files`.
struct Static {
    files: Files,
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
        match scan(ctx.path()) {
            Scan::Unsafe => ctx.respond(self.bad_request.clone()),
            Scan::Hidden if self.hide_dotfiles => ctx.respond(self.not_found.clone()),
            Scan::Hidden | Scan::Plain => self.files.call(ctx),
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
        files: files(root, &s.index, s.max_file_bytes, s.check_interval),
        hide_dotfiles: s.dotfiles == Dotfiles::Deny,
        bad_request: error(400, "400 Bad Request\n"),
        not_found: error(404, "404 Not Found\n"),
        method_not_allowed: error(405, "405 Method Not Allowed\n").header("Allow", "GET, HEAD"),
    }) as Arc<dyn Handler>))
}

/// The `Files` for the params. fit's handler follows changes on Linux through a watch of the
/// directories under the root, and trusts what it knows for `check_interval` elsewhere and with
/// a `check_interval` of 0 looks at the disk for every request: `Files` is set up alike.
fn files(root: PathBuf, index: &str, max_file_bytes: u64, check: Duration) -> Files {
    Files::new(root)
        .index(index)
        .max_cached_bytes(max_file_bytes)
        .check_interval(check)
        .watch(cfg!(target_os = "linux") && !check.is_zero())
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
    use super::{Scan, scan};

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
