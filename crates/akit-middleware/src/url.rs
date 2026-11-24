//! Paths and header names as Go's `net/url` and `net/http` write them, which fit's middleware
//! goes by: a request's escaped path, a path escaped afresh, a canonical header name.

use std::borrow::Cow;

use akio_http::Context;
use akit_runtime::CANONICAL_PATH;

/// The request's path as the handler goes by it, escaped: the canonical path (akit-listener's,
/// or a `rewrite` middleware's) when it differs from the one sent, else the one sent.
pub(crate) fn current_path(ctx: &Context) -> &str {
    ctx.local(CANONICAL_PATH).unwrap_or_else(|| ctx.path())
}

/// `p`, an escaped path, as Go's `URL.EscapedPath` gives it: `p` itself when it is a valid
/// escaping, else `p` decoded and escaped afresh (`\` becomes `%5C`).
pub(crate) fn escaped_path(p: &str) -> Cow<'_, str> {
    if p.bytes().all(valid_encoded) {
        return Cow::Borrowed(p);
    }
    match decode(p.as_bytes()) {
        Some(d) => Cow::Owned(escape_path(&d)),
        None => Cow::Borrowed(p),
    }
}

/// `p` percent-decoded; `None` when an escape is malformed.
pub(crate) fn decode(p: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(p.len());
    let mut it = p.iter();
    while let Some(&c) = it.next() {
        if c == b'%' {
            let hi = it.next().and_then(|&h| hex(h))?;
            let lo = it.next().and_then(|&l| hex(l))?;
            out.push(hi.wrapping_shl(4) | lo);
        } else {
            out.push(c);
        }
    }
    Some(out)
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c.wrapping_sub(b'0')),
        b'a'..=b'f' => Some(c.wrapping_sub(b'a').wrapping_add(10)),
        b'A'..=b'F' => Some(c.wrapping_sub(b'A').wrapping_add(10)),
        _ => None,
    }
}

/// `p`, a decoded path, escaped as Go's `url.PathEscape`-like `escape(p, encodePath)` writes it,
/// which is how a Go `URL` whose path was set afresh is written: what is not unreserved, or one
/// of `$&+,/:;=@`, is `%XX`.
pub(crate) fn escape_path(p: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut s = String::with_capacity(p.len());
    for &c in p {
        if should_escape(c) {
            s.push('%');
            for nibble in [c >> 4, c & 15] {
                s.push(char::from(*HEX.get(usize::from(nibble)).unwrap_or(&b'0')));
            }
        } else {
            s.push(char::from(c));
        }
    }
    s
}

/// Go's `shouldEscape(c, encodePath)`.
fn should_escape(c: u8) -> bool {
    !(c.is_ascii_alphanumeric()
        || matches!(
            c,
            b'-' | b'_'
                | b'.'
                | b'~'
                | b'$'
                | b'&'
                | b'+'
                | b','
                | b'/'
                | b':'
                | b';'
                | b'='
                | b'@'
        ))
}

/// Go's `validEncoded(s, encodePath)`, a byte at a time.
fn valid_encoded(c: u8) -> bool {
    matches!(
        c,
        b'!' | b'$'
            | b'&'
            | b'\''
            | b'('
            | b')'
            | b'*'
            | b'+'
            | b','
            | b';'
            | b'='
            | b':'
            | b'@'
            | b'['
            | b']'
            | b'%'
    ) || !should_escape(c)
}

/// `name` as Go's `http.CanonicalHeaderKey` writes it: the first letter and each letter after a
/// `-` upper case, the rest lower case (`x-debug-id` is `X-Debug-Id`). Names are checked to be
/// tokens when the config is read.
pub(crate) fn canonical_key(name: &str) -> String {
    let mut upper = true;
    name.chars()
        .map(|c| {
            let out = if upper {
                c.to_ascii_uppercase()
            } else {
                c.to_ascii_lowercase()
            };
            upper = c == '-';
            out
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping_as_go() {
        assert_eq!(escape_path(b"/u?id=42"), "/u%3Fid=42");
        assert_eq!(escape_path(b"/a b/c"), "/a%20b/c");
        assert_eq!(escape_path(b"/a!b*(c)'"), "/a%21b%2A%28c%29%27");
        assert_eq!(escape_path(b"/$&+,:;=@-_.~"), "/$&+,:;=@-_.~");
        assert_eq!(escape_path(b"/\\x\xff"), "/%5Cx%FF");
        assert_eq!(escaped_path("/a%20b"), "/a%20b");
        assert_eq!(escaped_path("/a!b"), "/a!b");
        assert_eq!(escaped_path("/go/\\evil.com"), "/go/%5Cevil.com");
        assert_eq!(escaped_path("/a{b}"), "/a%7Bb%7D");
        assert_eq!(decode(b"/a%2Fb%zz"), None);
        assert_eq!(decode(b"/a%2fb").as_deref(), Some(&b"/a/b"[..]));
    }

    #[test]
    fn canonical_keys() {
        assert_eq!(canonical_key("x-debug-ID"), "X-Debug-Id");
        assert_eq!(canonical_key("SERVER"), "Server");
        assert_eq!(canonical_key("-a"), "-A");
    }
}
