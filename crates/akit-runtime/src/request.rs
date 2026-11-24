//! A request's header fields as a middleware changed them, for the handlers after it.
//!
//! akio's [`Context`] gives its request by shared reference only, so a middleware cannot change
//! the fields the request came with (akio API request A33). Until it can, a middleware that
//! changes them (`headers`, I40) keeps the changed fields in a [`Context::local`] of the request,
//! [`REQUEST_FIELDS`], and what reads the request's fields after it (the `proxy` handler, the
//! fields it sends upstream) reads them with [`request_headers`]. A request no middleware changed,
//! nearly every one, has no such local: its fields are read where they are, with no copy.
//!
//! The local is a string: the fields, each its name and value, each of those its length in
//! decimal, a `:` and its bytes. A stopgap, recorded in the progress files with A33.

use std::borrow::Cow;

use akio_http::{Context, Headers};

/// The [`Context::local`] that holds a request's header fields as a middleware changed them;
/// unset when none did. Read with [`request_headers`], written with [`set_request_headers`].
pub const REQUEST_FIELDS: &str = "akit.request_fields";

/// The request's header fields, as the middleware before the handler left them: the request's
/// own, borrowed, unless one changed them.
#[must_use]
pub fn request_headers(ctx: &Context) -> Cow<'_, Headers> {
    match ctx.local(REQUEST_FIELDS) {
        Some(s) => Cow::Owned(decode(s)),
        None => Cow::Borrowed(&ctx.req().headers),
    }
}

/// Makes `fields` the request's header fields for what reads them after this call through
/// [`request_headers`].
pub fn set_request_headers(ctx: &mut Context, fields: &Headers) {
    ctx.set_local(REQUEST_FIELDS, encode(fields));
}

fn encode(fields: &Headers) -> String {
    use std::fmt::Write as _;
    let bytes: usize = fields
        .iter()
        .map(|(n, v)| n.len().saturating_add(v.len()).saturating_add(12))
        .fold(0, usize::saturating_add);
    let mut s = String::with_capacity(bytes);
    for (n, v) in fields.iter() {
        // Writing to a String does not fail.
        let _ = write!(s, "{}:{n}{}:{v}", n.len(), v.len());
    }
    s
}

/// The fields `s` holds; what is malformed (no one but [`encode`] writes the local) ends them.
fn decode(s: &str) -> Headers {
    let mut fields = Headers::with_capacity(4, s.len());
    let mut rest = s;
    while !rest.is_empty() {
        let Some((name, r)) = take(rest) else { break };
        let Some((value, r)) = take(r) else { break };
        fields.append(name, value);
        rest = r;
    }
    fields
}

/// One length-prefixed string at the start of `s`, and what follows it.
fn take(s: &str) -> Option<(&str, &str)> {
    let (len, rest) = s.split_once(':')?;
    let len: usize = len.parse().ok()?;
    Some((rest.get(..len)?, rest.get(len..)?))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn fields_survive_the_local() {
        let mut h = Headers::new();
        h.append("X-A", "1");
        h.append("x-weird", "a:b 12:c");
        h.append("X-Empty", "");
        h.append("x-a", "2");
        let back = decode(&encode(&h));
        assert_eq!(back, h);
        assert_eq!(decode(&encode(&Headers::new())), Headers::new());
        // What is malformed ends the fields, without a panic.
        for bad in ["3:ab", "x:y", "1:a", "99999999999999999999:a", "2:ab1"] {
            let _ = decode(bad);
        }
        assert_eq!(decode("1:a1:b2:cd").len(), 1);
    }
}
