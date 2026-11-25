//! Fuzz entry points (feature `fuzzing`; not a public API): the `fuzz/` cargo-fuzz project calls
//! them, and the tests below replay `fuzz/corpus/<target>` on stable. What the middlewares read
//! from a request or a key server: tokens and their claims, JWK sets and PEM keys, credentials,
//! forwarding fields, `Accept-Encoding`, and paths to rewrite.
#![allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::panic,
    clippy::expect_used
)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use akio_http::Headers;
use akit_config::{Encoding, Network};

use crate::rewrite::Rewrite;
use crate::{auth, compress, jws, limit, url};

/// `jwt_token`: see the `jwt` middleware's [`crate::jwt::fuzz::jwt_token`].
pub fn jwt_token(data: &[u8]) {
    crate::jwt::fuzz::jwt_token(data);
}

/// `jwks_keys`: a JWK set as a key server sends it, and a PEM file of public keys, never make
/// the parsers panic; the keys taken verify nothing they were not made for. `data[0]`'s low bit
/// picks which; then a token is checked against what was read.
pub fn jwks_keys(data: &[u8]) {
    let Some((&which, rest)) = data.split_first() else {
        return;
    };
    let keys = if which & 1 == 0 {
        jws::parse_jwks(rest)
    } else {
        jws::parse_public_keys(rest)
    };
    let Ok(keys) = keys else { return };
    assert!(!keys.is_empty(), "a set with no key was taken");
    let set = jws::KeySet::new([&keys[..]]);
    for alg in jws::Alg::ALL {
        for k in &keys {
            if k.fits(alg) {
                assert!(
                    !k.verify(alg, b"e30.e30", b""),
                    "{alg:?} verified no signature"
                );
            }
        }
    }
    let _ = set.lookup("");
}

/// The rate limiter's client, walking `X-Forwarded-For` back from the peer as fit's `clientIP`
/// does: past the trusted proxies to the first address that is not one; an entry that does not
/// parse stops the walk at the trusted one after it.
fn walk_back(peer: SocketAddr, h: &Headers, trusted: &[Network]) -> IpAddr {
    let unmap = |ip: IpAddr| match ip {
        IpAddr::V6(a) => a.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        ip => ip,
    };
    let is_trusted = |ip: IpAddr| trusted.iter().any(|n| n.contains(ip));
    let mut cur = unmap(peer.ip());
    if !is_trusted(cur) {
        return cur;
    }
    let items: Vec<&str> = h
        .get_all("x-forwarded-for")
        .filter(|l| !l.is_empty())
        .flat_map(|l| l.split(','))
        .collect();
    for item in items.iter().rev() {
        let s = item.trim();
        let s = s.split_once('%').map_or(s, |(a, _)| a);
        match s.parse::<IpAddr>().ok().map(unmap) {
            None => return cur,
            Some(a) if is_trusted(a) => cur = a,
            Some(a) => return a,
        }
    }
    cur
}

/// `middleware_fields`: what middlewares read from a request's fields and path. `data[0]`
/// picks the trusted proxies; the rest, NUL-separated: an `Authorization` field, the
/// `X-Forwarded-For` fields (one a line), an `Accept-Encoding` field, and a path.
///
/// Basic credentials split at the first colon; the rate limiter's client is the one fit's walk
/// back finds; the encoding negotiated is one offered; a path decodes back from its escaped
/// form; and a rewrite that replaces a path's every character with itself leaves it as it is,
/// whatever bytes it holds.
pub fn middleware_fields(data: &[u8]) {
    let Some((&flags, rest)) = data.split_first() else {
        return;
    };
    // The fields are text; the path, decoded, any bytes.
    let mut p = rest.splitn(4, |&b| b == 0);
    let mut field = || std::str::from_utf8(p.next().unwrap_or_default()).ok();
    let (Some(authorization), Some(xff), Some(accept)) = (field(), field(), field()) else {
        return;
    };
    let path = p.next().unwrap_or_default();

    let mut h = Headers::new();
    h.append("Authorization", authorization);
    if let Some((user, _)) = auth::credentials(&h) {
        assert!(!user.contains(&b':'));
    }

    let mut h = Headers::new();
    for line in xff.split('\n') {
        h.append("X-Forwarded-For", line);
    }
    let trusted: Vec<Network> = match flags & 3 {
        0 => Vec::new(),
        1 => vec![Network::parse("10.0.0.0/8").unwrap()],
        2 => vec![
            Network::parse("10.0.0.0/8").unwrap(),
            Network::parse("::1/128").unwrap(),
        ],
        _ => vec![Network::parse("0.0.0.0/0").unwrap()],
    };
    let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 5555);
    assert_eq!(
        limit::client_ip(peer, &h, &trusted),
        walk_back(peer, &h, &trusted),
        "X-Forwarded-For {xff:?}"
    );

    let mut h = Headers::new();
    h.append("Accept-Encoding", accept);
    let offered = [Encoding::Zstd, Encoding::Br, Encoding::Gzip];
    for o in [&offered[..], &offered[2..], &[]] {
        if let Some(e) = compress::negotiate(&h, o) {
            assert!(o.contains(&e), "{e:?} was not offered");
        }
    }

    let escaped = url::escape_path(path);
    assert_eq!(url::decode(escaped.as_bytes()).as_deref(), Some(path));
    if let Ok(s) = std::str::from_utf8(path) {
        if let Some(d) = url::decode(path) {
            let e = url::escaped_path(s);
            assert_eq!(url::decode(e.as_bytes()), Some(d), "{s:?}: {e:?}");
        }
        let _ = url::canonical_key(s);
    }

    // A decoded path starts with a slash.
    if path.starts_with(b"/") {
        let same = Rewrite::with_regexp(akit_router::regexp(b"(?s)(.)").unwrap(), "$1");
        assert_eq!(
            same.path(path),
            None,
            "{:?} was rewritten",
            String::from_utf8_lossy(path)
        );
    }
    let r = Rewrite::with_regexp(akit_router::regexp(b"^/old/(.*)$").unwrap(), "/new/$1");
    if let Some(out) = r.path(path) {
        assert!(out.starts_with(b"/"));
    }
    let _ = Arc::new(r);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `f` on every file of `fuzz/corpus/<target>`.
    fn replay(target: &str, f: fn(&[u8])) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fuzz/corpus")
            .join(target);
        let mut n = 0;
        for e in std::fs::read_dir(&dir).unwrap() {
            f(&std::fs::read(e.unwrap().path()).unwrap());
            n += 1;
        }
        assert!(n > 0, "{} is empty", dir.display());
    }

    #[test]
    fn corpus_jwt_token() {
        replay("jwt_token", jwt_token);
    }

    #[test]
    fn corpus_jwks_keys() {
        replay("jwks_keys", jwks_keys);
    }

    #[test]
    fn corpus_middleware_fields() {
        replay("middleware_fields", middleware_fields);
    }
}
