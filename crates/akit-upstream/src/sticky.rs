//! Sticky sessions by cookie (I36), as fit's `sticky`: a request whose cookie names an address
//! that is up goes there, and a response to any other sets the cookie naming the address it went
//! to. The cookie's value is [`sticky_id`] of the address, the same in every akit and fit with the
//! same addresses, so that a client keeps its address across instances and reloads.
//!
//! Everything a pick reads is made when the config is compiled (rule 12): the ids sorted for a
//! binary search, and each address's whole `Set-Cookie` value. A pick parses the cookie's value
//! and searches; it never allocates.

use std::fmt::Write as _;

use akio_crypto::hash::Sha256;
use akio_http::Headers;
use akit_config::StickyCookie;

use crate::balance::cookie_value;

/// An upstream's sticky cookie: the address each value names, and the `Set-Cookie` value that
/// names each address.
#[derive(Debug)]
pub struct Sticky {
    name: Box<str>,
    /// Each address's id, by index.
    ids: Box<[u64]>,
    /// The ids and their addresses' indexes, sorted by id.
    by_id: Box<[(u64, usize)]>,
    /// Each address's `Set-Cookie` value, by index.
    set_cookie: Box<[Box<str>]>,
}

impl Sticky {
    /// The sticky cookie `c` for `addresses`.
    #[must_use]
    pub fn new(c: &StickyCookie, addresses: &[impl AsRef<str>]) -> Sticky {
        let name = if c.name.is_empty() {
            akit_config::DEFAULT_STICKY_COOKIE_NAME
        } else {
            &c.name
        };
        let path = if c.path.is_empty() { "/" } else { &c.path };
        let mut attrs = format!("; Path={path}");
        if let Some(age) = c.max_age.filter(|t| !t.is_zero()) {
            let _ = write!(attrs, "; Max-Age={}", age.as_secs());
        }
        attrs.push_str("; HttpOnly; SameSite=Lax");
        if c.secure {
            attrs.push_str("; Secure");
        }
        let ids: Box<[u64]> = addresses.iter().map(|a| id(a.as_ref())).collect();
        let mut by_id: Vec<(u64, usize)> = ids.iter().copied().zip(0..).collect();
        by_id.sort_unstable();
        let set_cookie = ids
            .iter()
            .map(|&i| format!("{name}={i:016x}{attrs}").into_boxed_str())
            .collect();
        Sticky {
            name: name.into(),
            ids,
            by_id: by_id.into_boxed_slice(),
            set_cookie,
        }
    }

    /// The cookie's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The index of the address the request's cookie names, whether it is up or not; `None`
    /// without the cookie, or for a value that names no address.
    #[must_use]
    pub fn backend(&self, headers: &Headers) -> Option<usize> {
        let id = parse_id(cookie_value(headers, &self.name)?)?;
        let i = self.by_id.partition_point(|&(x, _)| x < id);
        self.by_id
            .get(i)
            .filter(|&&(x, _)| x == id)
            .map(|&(_, b)| b)
    }

    /// The `Set-Cookie` value a response to a request with `headers`, which went to the address
    /// at `backend`, carries: the cookie naming the address, unless the request's names it
    /// already.
    #[must_use]
    pub fn set_cookie(&self, headers: Option<&Headers>, backend: usize) -> Option<&str> {
        let id = *self.ids.get(backend)?;
        let named = headers
            .and_then(|h| cookie_value(h, &self.name))
            .and_then(parse_id);
        if named == Some(id) {
            return None;
        }
        self.set_cookie.get(backend).map(|s| &**s)
    }
}

/// The sticky cookie's value for the address `address`, as fit's `StickyID`: the first 64 bits
/// of its SHA-256, in lowercase hex.
#[must_use]
pub fn sticky_id(address: &str) -> String {
    format!("{:016x}", id(address))
}

fn id(address: &str) -> u64 {
    let sum = Sha256::digest(address.as_bytes());
    let mut first = [0u8; 8];
    for (d, s) in first.iter_mut().zip(sum.iter()) {
        *d = *s;
    }
    u64::from_be_bytes(first)
}

/// A cookie value as fit's ids are written, 16 lowercase hex digits, as a number.
fn parse_id(v: &str) -> Option<u64> {
    if v.len() != 16 || !v.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    u64::from_str_radix(v, 16).ok()
}
