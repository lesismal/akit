//! The `rewrite` middleware, as fit's `middleware/rewrite` `Rewrite`: the request's path changed
//! before the handler sees it, the route having been picked by the path as it came.
//!
//! The path is changed in the request's target ([`Context::req_mut`], akio ADR-126), its query
//! kept: the `proxy` handler sends the new path upstream, the `static` handler looks it up. The
//! access log writes the target the request had before (fit's `RequestURI`). As fit's, the rewrite works on the decoded path and writes the
//! new one escaped afresh (`/api/a%2Fb` stripped of `/api` is `/a/b`). The `regexp`, compiled
//! with the config, replaces each of its matches in the decoded path with `replacement` (Go's
//! `ReplaceAllString`, on akio-regex's matches and groups: akio API request A32).

use std::{borrow::Cow, sync::Arc};

use akio_http::{BodyMode, Context, Request};
use akio_regex::Regex;
use akit_runtime::{
    Handler, HandlerParams, Middleware, Provision, ProvisionError, Provisioned, set_request_path,
};

use crate::url;

/// The rewrite, compiled.
#[derive(Debug, Clone)]
pub(crate) struct Rewrite {
    /// The prefixes to strip, each without a trailing `/`, in the order written.
    strip: Arc<[Box<[u8]>]>,
    /// The regexp whose matches are replaced, and what replaces them.
    replace: Option<(Arc<Regex>, Arc<str>)>,
    /// The prefix to add; empty for none.
    add: Arc<[u8]>,
}

pub(crate) fn new(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Middleware>, ProvisionError> {
    let HandlerParams::Rewrite(r) = p.params() else {
        return Err(ProvisionError::new(
            "the rewrite middleware was given another's params",
        ));
    };
    let replace = if r.regexp.is_empty() {
        None
    } else {
        let re = akit_router::regexp(r.regexp.as_bytes())
            .map_err(|e| ProvisionError::field("regexp", e))?;
        Some((Arc::new(re), r.replacement.as_str().into()))
    };
    Ok(Provisioned::new(Arc::new(Rewrite {
        replace,
        strip: r
            .strip_prefix
            .iter()
            .map(|s| s.strip_suffix('/').unwrap_or(s).as_bytes().into())
            .collect(),
        add: r.add_prefix.as_bytes().into(),
    }) as Arc<dyn Middleware>))
}

impl Rewrite {
    /// A rewrite that replaces `re`'s matches with `with`, and strips and adds nothing.
    #[cfg(any(test, feature = "fuzzing"))]
    pub(crate) fn with_regexp(re: Regex, with: &str) -> Rewrite {
        Rewrite {
            strip: Arc::from(Vec::new()),
            replace: Some((Arc::new(re), with.into())),
            add: Arc::from(Vec::new()),
        }
    }

    /// `p`, a decoded path, rewritten; `None` when it is left as it is.
    pub(crate) fn path(&self, p: &[u8]) -> Option<Vec<u8>> {
        let mut rest = p;
        for prefix in self.strip.iter() {
            if let Some(r) = cut_segments(p, prefix) {
                rest = r;
                break;
            }
        }
        let stripped = rest.len() != p.len();
        let replaced = match &self.replace {
            Some((re, with)) => replace(re, with, rest),
            None => None,
        };
        if !stripped && replaced.is_none() && self.add.is_empty() {
            return None;
        }
        let rest = replaced.as_deref().unwrap_or(rest);
        let mut out =
            Vec::with_capacity(self.add.len().saturating_add(rest.len()).saturating_add(1));
        out.extend_from_slice(&self.add);
        // As fit's: what the strip leaves, or the replacement makes, starts with a `/`.
        if rest.first() != Some(&b'/') {
            out.push(b'/');
        }
        out.extend_from_slice(rest);
        (out != p).then_some(out)
    }
}

/// `p` (a decoded path, perhaps not UTF-8) with each match of `re` replaced with `with`; `None`
/// when nothing matched. Go's regexp reads each byte of an invalid UTF-8 sequence as a character
/// of its own (U+FFFD) and keeps the byte where it copies the text: here such a byte stands, while
/// the regexp runs, for a character of a private use plane ([`stand_in_base`]), and is put back
/// after.
fn replace(re: &Regex, with: &str, p: &[u8]) -> Option<Vec<u8>> {
    if let Ok(text) = std::str::from_utf8(p) {
        return match re.replace_all(text, with) {
            Cow::Borrowed(_) => None,
            Cow::Owned(s) => Some(s.into_bytes()),
        };
    }
    // No block free for the stand-ins: only a path made to hold every one can do that; it is
    // left as it is rather than have its characters taken for its bytes.
    let base = stand_in_base(p, with)?;
    let mut text = String::with_capacity(p.len().saturating_mul(2));
    for chunk in p.utf8_chunks() {
        text.push_str(chunk.valid());
        for &b in chunk.invalid() {
            text.push(char::from_u32(base | u32::from(b)).unwrap_or('\u{FFFD}'));
        }
    }
    let Cow::Owned(out) = re.replace_all(&text, with) else {
        return None;
    };
    let mut bytes = Vec::with_capacity(out.len());
    for c in out.chars() {
        match u32::from(c)
            .checked_sub(base)
            .and_then(|b| u8::try_from(b).ok())
        {
            Some(b) if b >= 0x80 => bytes.push(b),
            _ => bytes.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
        }
    }
    Some(bytes)
}

/// Where the stand-ins of `p`'s invalid bytes go: a block of private use planes 15 and 16,
/// `base | 0x80` to `base | 0xFF`, that no character of `p` or `with` is in, so that every
/// stand-in the replacement holds is one and is put back as its byte (a path holding a
/// character of the block, U+10FF80 say, keeps it). The highest free one, U+10FF80's when no
/// character of the planes is there; `None` when none is free.
fn stand_in_base(p: &[u8], with: &str) -> Option<u32> {
    const FIRST: u32 = 0xF_0000;
    const BLOCKS: usize = 0x200;
    let mut used = [false; BLOCKS];
    let mut mark = |c: char| {
        let u = u32::from(c);
        if u >= FIRST && u & 0x80 != 0 {
            if let Some(slot) = usize::try_from(u.saturating_sub(FIRST) >> 8)
                .ok()
                .and_then(|i| used.get_mut(i))
            {
                *slot = true;
            }
        }
    };
    for chunk in p.utf8_chunks() {
        chunk.valid().chars().for_each(&mut mark);
    }
    with.chars().for_each(&mut mark);
    let free = used.iter().rposition(|u| !u)?;
    u32::try_from(free)
        .ok()
        .and_then(|i| FIRST.checked_add(i << 8))
}

/// What is left of `p` without `prefix` (which has no trailing `/`), when `p` is `prefix` or goes
/// on with a `/` after it: fit's `cutSegments`. An empty rest stands for `/`.
fn cut_segments<'p>(p: &'p [u8], prefix: &[u8]) -> Option<&'p [u8]> {
    let rest = p.strip_prefix(prefix)?;
    (rest.is_empty() || rest.first() == Some(&b'/')).then_some(rest)
}

impl Middleware for Rewrite {
    fn wrap(&self, next: Arc<dyn Handler>) -> Arc<dyn Handler> {
        Arc::new(Rewritten {
            rw: self.clone(),
            next,
        })
    }

    fn changes_target(&self) -> bool {
        true
    }
}

/// A handler behind a rewrite.
struct Rewritten {
    rw: Rewrite,
    next: Arc<dyn Handler>,
}

impl akio_http::Service for Rewritten {
    fn call(&self, ctx: &mut Context) {
        // The path the routes matched is canonical (akit-listener answers 400 to one that has
        // none), so decoding it again cannot fail; were it to, the path is left alone.
        let new = url::decode(ctx.path().as_bytes())
            .and_then(|p| self.rw.path(&p))
            .map(|p| url::escape_path(&p));
        if let Some(p) = new {
            set_request_path(ctx, &p);
        }
        self.next.call(ctx);
    }

    fn body_mode(&self, req: &Request) -> BodyMode {
        self.next.body_mode(req)
    }

    fn streams_bodies(&self) -> bool {
        self.next.streams_bodies()
    }
}

impl Handler for Rewritten {}

#[cfg(test)]
mod tests {
    use super::*;

    fn rw(strip: &[&str], add: &str) -> Rewrite {
        Rewrite {
            replace: None,
            strip: strip
                .iter()
                .map(|s| s.strip_suffix('/').unwrap_or(s).as_bytes().into())
                .collect(),
            add: add.as_bytes().into(),
        }
    }

    fn apply(r: &Rewrite, p: &str) -> String {
        String::from_utf8(
            r.path(p.as_bytes())
                .unwrap_or_else(|| p.as_bytes().to_vec()),
        )
        .unwrap_or_default()
    }

    #[test]
    fn paths_as_fit_rewrites_them() {
        let r = rw(&["/api/v1", "/api"], "");
        for (p, want) in [
            ("/api/users", "/users"),
            ("/api/v1/users", "/users"),
            ("/api", "/"),
            ("/api/", "/"),
            ("/apix", "/apix"),
            ("/other/api", "/other/api"),
            ("/api/a/b", "/a/b"),
        ] {
            assert_eq!(apply(&r, p), want, "{p}");
        }
        let r = rw(&["/static/"], "");
        assert_eq!(apply(&r, "/static/css/a.css"), "/css/a.css");
        assert_eq!(apply(&r, "/static"), "/");
        let r = rw(&[], "/v2");
        assert_eq!(apply(&r, "/users"), "/v2/users");
        assert_eq!(apply(&r, "/"), "/v2/");
        let r = rw(&["/api"], "/internal");
        assert_eq!(apply(&r, "/api/v1/users"), "/internal/v1/users");
        assert_eq!(apply(&r, "/api"), "/internal/");
        // Unchanged: no new path.
        assert!(rw(&["/api"], "").path(b"/x").is_none());
        assert!(rw(&["/"], "").path(b"/x").is_none());
    }

    fn rx(strip: &[&str], re: &str, with: &str, add: &str) -> Rewrite {
        let mut r = rw(strip, add);
        r.replace = Some((
            Arc::new(akit_router::regexp(re.as_bytes()).unwrap_or_else(|e| panic!("{e}"))),
            with.into(),
        ));
        r
    }

    #[test]
    fn regexps_as_fit_replaces_them() {
        let r = rx(&[], "^/old/(.*)$", "/new/$1", "");
        assert_eq!(apply(&r, "/old/a/b"), "/new/a/b");
        assert_eq!(apply(&r, "/other"), "/other");
        let r = rx(&[], "^/users/(?P<id>[0-9]+)$", "/u/${id}/profile", "");
        assert_eq!(apply(&r, "/users/42"), "/u/42/profile");
        assert_eq!(apply(&r, "/users/x"), "/users/x");
        // Every match is replaced; `$$` is a `$`; `$1x` names the group `1x` (empty).
        assert_eq!(apply(&rx(&[], "a", "b", ""), "/aaa/a"), "/bbb/b");
        assert_eq!(apply(&rx(&[], "(x)", "$$$1", ""), "/x"), "/$x");
        assert_eq!(apply(&rx(&[], "(x)", "$1x", ""), "/x"), "/");
        // What the replacement makes gets a leading `/`.
        assert_eq!(apply(&rx(&[], "^/v1", "", ""), "/v1/users"), "/users");
        assert_eq!(apply(&rx(&[], "^/", "", ""), "/users"), "/users");
        assert_eq!(apply(&rx(&[], "^/.*", "", ""), "/users"), "/");
        // Order: strip, replace, add.
        let r = rx(&["/api"], "^/v([0-9]+)/", "/version$1/", "/internal");
        assert_eq!(apply(&r, "/api/v2/x"), "/internal/version2/x");
        assert_eq!(apply(&r, "/v2/x"), "/internal/version2/x");
        // Empty matches, as Go's ReplaceAllString.
        assert_eq!(apply(&rx(&[], "x*", "-", ""), "/ab"), "/-/-a-b-");
        // Unicode in the decoded path.
        assert_eq!(apply(&rx(&[], "é+", "e", ""), "/caféé"), "/cafe");
    }

    #[test]
    fn bytes_that_are_not_utf8_are_kept() {
        let r = rx(&[], "^/a/(.)(.*)$", "/b/$2$1", "");
        assert_eq!(r.path(b"/a/\xffz\xfe"), Some(b"/b/z\xfe\xff".to_vec()));
        // `.` takes each invalid byte for one character, as Go's regexp does.
        let r = rx(&[], "^/(.)$", "/one", "");
        assert_eq!(r.path(b"/\xff"), Some(b"/one".to_vec()));
        assert!(r.path(b"/\xff\xff").is_none());
        // Nothing matched: no new path.
        assert!(rx(&[], "q", "r", "").path(b"/\xff").is_none());
    }

    /// Found by fuzzing (I77, `middleware_fields`): a path with an invalid byte and a character
    /// of the block its stand-ins used to take (U+10FF80 to U+10FFFF) came out with that
    /// character turned into a byte (`/\u{10FF80}\xff` into `/\x80\xff`). The stand-ins now take
    /// a block neither the path nor the replacement uses.
    #[test]
    fn characters_of_the_stand_ins_block_are_kept() {
        let mut p = "/\u{10FF80}x\u{10FFFF}".as_bytes().to_vec();
        p.push(0xff);
        let same = rx(&[], "(?s)(.)", "$1", "");
        assert!(same.path(&p).is_none(), "rewritten as itself");
        let r = rx(&[], "x", "\u{10FF81}", "");
        let mut want = "/\u{10FF80}\u{10FF81}\u{10FFFF}".as_bytes().to_vec();
        want.push(0xff);
        assert_eq!(r.path(&p), Some(want));
        // Every block taken: left as it is, not corrupted.
        let mut all = String::from("/");
        for i in 0..0x200u32 {
            all.push(char::from_u32(0xF_0080 + (i << 8)).unwrap());
        }
        let mut p = all.clone().into_bytes();
        p.push(0xff);
        assert_eq!(stand_in_base(&p, ""), None);
        assert!(same.path(&p).is_none());
        assert_eq!(stand_in_base(b"/\xff", ""), Some(0x10_FF00));
    }
}
