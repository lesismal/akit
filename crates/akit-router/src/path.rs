//! Request paths in their canonical form, as fit's listener puts them (`listener/path.go`).

use std::borrow::Cow;

/// A request's path in its canonical form: what the routes match, and what a handler should go by
/// (the upstream a proxy forwards to, a static file), so that both see the same path.
///
/// The canonical path has no dot-segments, removed as RFC 3986 section 5.2.4 says, `%2E` counting
/// as `.`, and no empty segments: `//` is one `/`. Only a literal `/` separates segments: `%2F`
/// and `%5C` stay as they came, inside their segment, since an API may take a name with a slash
/// in one segment (GitLab's `group%2Fproject`). Case and `;` are left alone, as nginx leaves them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Canonical<'a> {
    /// The path escaped, as a request target writes it: the one sent, unless something was
    /// removed from it.
    pub escaped: Cow<'a, str>,
    /// The path percent-decoded: what the routes match (fit's `URL.Path`). Not always UTF-8.
    pub decoded: Cow<'a, [u8]>,
}

impl Canonical<'_> {
    /// Whether the canonical path differs from the one sent (dot or empty segments were
    /// removed).
    #[must_use]
    pub fn changed(&self) -> bool {
        matches!(self.escaped, Cow::Owned(_))
    }
}

/// `path`, a request target's path (without its query), in its canonical form; `None` for a path
/// that is refused, which fit answers `400 bad request path`: one whose `..` would climb above
/// the root, one with a malformed escape, and one that, decoded, has an empty or dot-segment
/// that came encoded (`%2F`, `%5C`, or `\` around it), as no client has reason to send one and a
/// backend that decodes it, or takes `\` for `/`, would see another path than the routes did.
///
/// A path with nothing to decode and nothing to remove, nearly every one, is only scanned, without
/// allocating. A path that does not start with `/` (`*`) is left as it is.
#[must_use]
pub fn canonicalize(path: &str) -> Option<Canonical<'_>> {
    if !path.starts_with('/') {
        return Some(Canonical {
            escaped: Cow::Borrowed(path),
            decoded: Cow::Borrowed(path.as_bytes()),
        });
    }
    let decoded = percent_decode(path.as_bytes())?;
    if !has_dot_or_empty_segment(&decoded) {
        return Some(Canonical {
            escaped: Cow::Borrowed(path),
            decoded,
        });
    }
    let clean = remove_dot_segments(path)?;
    let decoded = percent_decode(clean.as_bytes())?.into_owned();
    if has_dot_or_empty_segment(&decoded) {
        // What is left came encoded: `%2F`, `%5C` or a `\`.
        return None;
    }
    Some(Canonical {
        escaped: Cow::Owned(clean),
        decoded: Cow::Owned(decoded),
    })
}

/// `s` percent-decoded, borrowed when it has no escape; `None` when an escape is malformed, as Go's
/// URL parser refuses it.
fn percent_decode(s: &[u8]) -> Option<Cow<'_, [u8]>> {
    if !s.contains(&b'%') {
        return Some(Cow::Borrowed(s));
    }
    let mut out = Vec::with_capacity(s.len());
    let mut it = s.iter();
    while let Some(&c) = it.next() {
        if c == b'%' {
            let hi = it.next().and_then(|&h| hex(h))?;
            let lo = it.next().and_then(|&l| hex(l))?;
            out.push(hi.wrapping_shl(4) | lo);
        } else {
            out.push(c);
        }
    }
    Some(Cow::Owned(out))
}

fn hex(c: u8) -> Option<u8> {
    char::from(c)
        .to_digit(16)
        .and_then(|d| u8::try_from(d).ok())
}

/// Whether `p`, a path that starts with `/`, has a segment that is empty (but the last, after a
/// trailing slash), `.` or `..`, `\` separating segments as well as `/`.
fn has_dot_or_empty_segment(p: &[u8]) -> bool {
    let sep = |c: Option<&u8>| matches!(c, Some(b'/' | b'\\'));
    p.iter().enumerate().any(|(i, c)| {
        if *c != b'/' && *c != b'\\' {
            return false;
        }
        let rest = p.get(i.saturating_add(1)..).unwrap_or_default();
        match rest {
            [] => false,
            [b'/' | b'\\', ..] | [b'.'] | [b'.', b'.'] => true,
            [b'.', c, ..] if sep(Some(c)) => true,
            [b'.', b'.', c, ..] => sep(Some(c)),
            _ => false,
        }
    })
}

/// `escaped`, an escaped path that starts with `/`, without its dot-segments, in which `%2E`
/// counts as `.`, and its empty segments but a trailing one; `None` when a `..` would climb above
/// the root. A dot-segment that ends the path leaves a trailing slash, as RFC 3986 has it:
/// `/a/b/..` is `/a/`.
fn remove_dot_segments(escaped: &str) -> Option<String> {
    let mut out = String::with_capacity(escaped.len());
    out.push('/');
    let rest = escaped.get(1..).unwrap_or_default();
    let mut segs = rest.split('/').peekable();
    while let Some(seg) = segs.next() {
        let more = segs.peek().is_some();
        match dot_segment(seg) {
            1 => {}
            2 => {
                if out.len() == 1 {
                    return None;
                }
                // `out` ends with the `/` after the segment to pop.
                out.pop();
                let cut = out.rfind('/').map_or(1, |i| i.saturating_add(1));
                out.truncate(cut);
            }
            _ => {
                if !seg.is_empty() {
                    out.push_str(seg);
                    if more {
                        out.push('/');
                    }
                }
            }
        }
    }
    Some(out)
}

/// 1 for the segment `.`, 2 for `..`, either with its dots written `%2E` or `%2e`, and 0 for any
/// other.
fn dot_segment(seg: &str) -> u8 {
    let mut s = seg.as_bytes();
    let mut n = 0u8;
    loop {
        s = match s {
            [] => return n,
            _ if n == 2 => return 0,
            [b'.', rest @ ..] | [b'%', b'2', b'e' | b'E', rest @ ..] => rest,
            _ => return 0,
        };
        n = n.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::canonicalize;

    /// fit's `TestCanonicalize`: the target, then the path (`URL.Path`) after, and the request
    /// target after (the escaped path and the query); refused when the path is empty.
    #[test]
    fn fits_canonicalize() {
        for (target, path, uri) in [
            // Left as they are.
            ("/", "/", "/"),
            ("/a/b", "/a/b", "/a/b"),
            ("/a/b/", "/a/b/", "/a/b/"),
            ("/a/b?x=/../y", "/a/b", "/a/b?x=/../y"),
            (
                "/.well-known/acme-challenge/t",
                "/.well-known/acme-challenge/t",
                "/.well-known/acme-challenge/t",
            ),
            ("/a/.b/..c/...", "/a/.b/..c/...", "/a/.b/..c/..."),
            ("/a/%2e%2ex", "/a/..x", "/a/%2e%2ex"),
            (
                "/group%2Fproject/x",
                "/group/project/x",
                "/group%2Fproject/x",
            ),
            ("/a%5Cb", "/a\\b", "/a%5Cb"),
            ("/a;b/..;/c", "/a;b/..;/c", "/a;b/..;/c"),
            ("/ADMIN/x", "/ADMIN/x", "/ADMIN/x"),
            // Dot-segments and empty segments removed.
            ("/x/../admin/secret", "/admin/secret", "/admin/secret"),
            ("/x/%2e%2e/admin/secret", "/admin/secret", "/admin/secret"),
            (
                "/x/%2E./admin/secret?q=1",
                "/admin/secret",
                "/admin/secret?q=1",
            ),
            ("/./admin", "/admin", "/admin"),
            ("//admin/secret", "/admin/secret", "/admin/secret"),
            ("/admin//secret///", "/admin/secret/", "/admin/secret/"),
            ("/a/b/..", "/a/", "/a/"),
            ("/a/b/.", "/a/b/", "/a/b/"),
            ("/a/..", "/", "/"),
            ("/a/b/../../c", "/c", "/c"),
            ("/a/../b%20c/%2F/../d", "/b c/d", "/b%20c/d"),
            ("/x/../a%2Fb", "/a/b", "/a%2Fb"),
            ("/x/..?", "/", "/?"),
            ("/x/./...", "/x/...", "/x/..."),
            // Refused.
            ("/..", "", ""),
            ("/../admin", "", ""),
            ("/a/../../admin", "", ""),
            ("/%2e%2e/admin", "", ""),
            ("/x%2F..%2Fadmin", "", ""),
            ("/x%2F%2Fadmin", "", ""),
            ("/%2Fadmin", "", ""),
            ("/x%5C..%5Cadmin", "", ""),
            ("/x\\..\\admin", "", ""),
            ("/\\evil.com", "", ""),
            // Not in fit's table: Go's URL parser refuses a malformed escape.
            ("/a%2", "", ""),
            ("/a%zz", "", ""),
        ] {
            let (p, query) = match target.split_once('?') {
                Some((p, q)) => (p, Some(q)),
                None => (target, None),
            };
            let got = canonicalize(p);
            if path.is_empty() {
                assert!(got.is_none(), "{target}: {got:?}, want refused");
                continue;
            }
            let got = got.unwrap_or_else(|| panic!("{target}: refused"));
            let got_uri = match query {
                Some(q) => format!("{}?{q}", got.escaped),
                None => got.escaped.to_string(),
            };
            assert_eq!(
                (&*got.decoded, got_uri.as_str()),
                (path.as_bytes(), uri),
                "{target}"
            );
            assert_eq!(got.changed(), got.escaped != p, "{target}");
        }
        // Paths not to canonicalize.
        for p in ["*", ""] {
            let c = canonicalize(p).unwrap();
            assert_eq!((&*c.escaped, &*c.decoded), (p, p.as_bytes()));
        }
    }

    #[test]
    fn non_utf8_escapes() {
        let c = canonicalize("/a%FF/./b").unwrap();
        assert_eq!(&*c.escaped, "/a%FF/b");
        assert_eq!(&*c.decoded, b"/a\xff/b");
    }
}
