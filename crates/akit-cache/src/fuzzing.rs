//! Fuzz entry points (feature `fuzzing`; not a public API): the `fuzz/` cargo-fuzz project calls
//! them, and the tests below replay `fuzz/corpus/<target>` on stable. fit's `FuzzDirectives`
//! (`middleware/cache/fuzz_test.go`), and the disk tier's file format, which a damaged or
//! foreign file in the cache's directory feeds.
#![allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::panic,
    clippy::expect_used
)]

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use akio_http::Headers;

use crate::cache::{hit_response, same_origin, usable};
use crate::disk;
use crate::purge::url_key;
use crate::semantics::{
    self, MAX_DELTA, MAX_HEURISTIC, MAX_TAG_LEN, MAX_TAGS, parse_directives, tags_of, vary_names,
};
use crate::store::Entry;

/// The `n`th NUL-separated part of `text`, empty past the last.
fn part(parts: &[&str], n: usize) -> String {
    parts.get(n).copied().unwrap_or("").to_owned()
}

/// `cache_semantics`: RFC 9111's parsing on any fields. `data[..12]`: the response's status, the
/// stored response's age and lifetime in seconds, and flags; then NUL-separated: the response's
/// `Cache-Control`, `Vary`, `Cache-Tag`, `Surrogate-Key`, `Expires`, `Last-Modified`, `ETag`,
/// `Location`; the request's `Cache-Control`, `Pragma`, `If-None-Match`, `If-Modified-Since`,
/// its path, and a URL to purge.
///
/// Every duration is bounded, tags are few and short, `Vary` names come in lower case, a
/// location the cache invalidates is a path on its own origin with no dot segments left, and a
/// purge key is an `http` or `https` key with no byte a request target cannot carry.
pub fn cache_semantics(data: &[u8]) {
    if data.len() < 12 {
        return;
    }
    let status = u16::from_be_bytes([data[0], data[1]]);
    let age = Duration::from_secs(u64::from(u32::from_be_bytes([
        data[2], data[3], data[4], data[5],
    ])));
    let lifetime = Duration::from_secs(u64::from(u32::from_be_bytes([
        data[6], data[7], data[8], data[9],
    ])));
    let flags = data[10];
    let Ok(text) = std::str::from_utf8(&data[12..]) else {
        return;
    };
    let p: Vec<&str> = text.split('\0').collect();
    let cc = part(&p, 0);

    let d = parse_directives(std::iter::once(cc.as_str()));
    for v in [
        d.max_age,
        d.s_maxage,
        d.max_stale,
        d.min_fresh,
        d.stale_while_revalidate,
        d.stale_if_error,
    ]
    .into_iter()
    .flatten()
    {
        assert!(v <= MAX_DELTA, "{cc:?}: {v:?}");
    }
    for member in cc.split([',', '=']) {
        assert!(semantics::delta(member.trim()) <= MAX_DELTA);
    }

    let mut resp = Headers::new();
    for (i, name) in [
        "cache-control",
        "vary",
        "cache-tag",
        "surrogate-key",
        "expires",
        "last-modified",
        "etag",
        "location",
    ]
    .into_iter()
    .enumerate()
    {
        let v = part(&p, i);
        if !v.is_empty() || flags & 1 != 0 {
            resp.append(name, v);
        }
    }
    let mut req = Headers::new();
    for (i, name) in [
        "cache-control",
        "pragma",
        "if-none-match",
        "if-modified-since",
    ]
    .into_iter()
    .enumerate()
    {
        let v = part(&p, 8 + i);
        if !v.is_empty() {
            req.append(name, v);
        }
    }
    // A request's path, in origin form, as the listener makes every target (I77).
    let path = match part(&p, 12) {
        p if p.starts_with('/') => p,
        p => format!("/{p}"),
    };
    let purge = part(&p, 13);

    let tags = tags_of(&resp);
    assert!(tags.len() <= MAX_TAGS, "{} tags", tags.len());
    for (i, t) in tags.iter().enumerate() {
        assert!(!t.is_empty() && t.len() <= MAX_TAG_LEN && t.trim() == &**t);
        assert!(!tags[..i].contains(t), "{t} twice");
    }
    match vary_names(&resp) {
        Some(names) => {
            for n in &names {
                assert_eq!(*n, n.to_ascii_lowercase());
                let _ = semantics::vary_value(&req, n);
            }
        }
        None => assert!(
            resp.get_all("vary")
                .any(|v| v.split(',').any(|m| m.trim() == "*"))
        ),
    }

    let date = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let ttl = (flags & 2 != 0).then_some(lifetime);
    let (fresh, explicit) = semantics::freshness(&resp, &d, status, date, ttl);
    if !explicit && ttl.is_none() {
        assert!(fresh <= MAX_HEURISTIC, "a heuristic lifetime of {fresh:?}");
    }
    if let Some(t) = d.s_maxage.or(d.max_age) {
        assert_eq!((fresh, explicit), (t, true));
    }
    let _ = semantics::storable_status(status, explicit);

    let rd = semantics::request_directives(&req);
    let now = date + age;
    let e = Entry {
        status,
        headers: resp.clone(),
        body: Arc::from(&b"body"[..]),
        vary: Vec::new(),
        response_time: date,
        initial_age: Duration::ZERO,
        lifetime,
        no_stale: d.must_revalidate || d.proxy_revalidate || d.s_maxage.is_some(),
        swr: d.stale_while_revalidate.unwrap_or_default(),
        sie: d.stale_if_error.unwrap_or_default(),
        etag: resp.get("etag").map(str::to_owned),
        last_modified: resp.get("last-modified").map(str::to_owned),
        tags,
        size: 0,
    };
    let _ = usable(&e, &rd, now);
    let _ = e.stale_for(now, lifetime);
    let r = hit_response(&e, &req, age, (flags & 4 != 0).then_some("stale"));
    assert!(r.status == e.status || r.status == 304 && e.status == 200);
    assert_eq!(
        r.headers.get("age"),
        Some(age.as_secs().to_string().as_str())
    );
    assert!(
        r.headers
            .get("cache-status")
            .is_some_and(|v| v.starts_with("fit; hit; ttl="))
    );

    let base = if flags & 8 != 0 {
        "https://example.com"
    } else {
        "http://Example.com:8080"
    };
    if let Some(target) = resp
        .get("location")
        .and_then(|v| same_origin(base, &path, v))
    {
        assert!(target.starts_with('/'), "{target:?}");
        let p = target.split('?').next().unwrap_or("");
        assert!(
            !p.split('/').any(|s| s == "." || s == ".."),
            "dot segments left in {target:?}"
        );
    }

    for prefix in [false, true] {
        if let Ok(k) = url_key(&purge, prefix) {
            assert!(
                k.starts_with("http://") || k.starts_with("https://"),
                "{k:?}"
            );
            assert!(!k.bytes().any(|b| b <= b' ' || b == 0x7f), "{k:?}");
            let (_, rest) = k.split_once("://").unwrap();
            let host = rest.split(['/', '?']).next().unwrap_or("");
            assert!(!host.is_empty(), "{purge:?}: {k:?}");
            assert_eq!(host, host.to_ascii_lowercase());
        }
    }
}

/// What an entry holds, to compare two.
#[allow(clippy::type_complexity)]
fn fields_of(
    e: &Entry,
) -> (
    u16,
    Vec<(String, String)>,
    Vec<(String, String)>,
    [Duration; 5],
    bool,
    Option<String>,
    Option<String>,
    Box<[Box<str>]>,
) {
    (
        e.status,
        e.headers
            .iter()
            .map(|(n, v)| (n.to_owned(), v.to_owned()))
            .collect(),
        e.vary.clone(),
        [
            e.response_time
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default(),
            e.initial_age,
            e.lifetime,
            e.swr,
            e.sie,
        ],
        e.no_stale,
        e.etag.clone(),
        e.last_modified.clone(),
        e.tags.clone(),
    )
}

/// `cache_disk`: the disk tier's files. `data[0]`'s low bit picks the direction: a file of any
/// bytes, which, when it reads, writes back to a file that reads the same; or an entry made of
/// the strings in `data[1..]` (NUL-separated: key, then field names and values, then a varied
/// name and value), which reads back as written.
pub fn cache_disk(data: &[u8]) {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    if mode & 1 == 0 {
        let Some((mut e, key, body_at)) = disk::decode(rest) else {
            return;
        };
        assert!(body_at <= rest.len());
        e.body = Arc::from(&rest[body_at..]);
        let mut file = disk::encode(&key, &e).expect("a file that read writes back");
        file.extend_from_slice(&e.body);
        let (again, key2, at2) = disk::decode(&file).expect("what was written reads");
        assert_eq!(key, key2);
        assert_eq!(at2, body_at);
        assert_eq!(fields_of(&e), fields_of(&again));
        return;
    }
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    let p: Vec<&str> = text.split('\0').collect();
    let key = p[0];
    let mut headers = Headers::new();
    let mut it = p[1..].chunks_exact(2);
    for nv in &mut it {
        headers.append(nv[0], nv[1]);
    }
    let vary = match it.remainder() {
        [v] => vec![("accept-encoding".to_owned(), (*v).to_owned())],
        _ => Vec::new(),
    };
    let secs = |i: usize| Duration::from_secs(u64::from(mode) << i);
    let e = Entry {
        status: u16::from(mode) * 3,
        etag: headers.get("etag").map(str::to_owned),
        last_modified: headers.get("last-modified").map(str::to_owned),
        tags: tags_of(&headers),
        headers,
        body: Arc::from(rest),
        vary,
        response_time: SystemTime::UNIX_EPOCH + secs(20),
        initial_age: secs(1),
        lifetime: secs(2),
        no_stale: mode & 2 != 0,
        swr: secs(3),
        sie: secs(4),
        size: 0,
    };
    let Ok(mut file) = disk::encode(key, &e) else {
        return;
    };
    file.extend_from_slice(&e.body);
    let (again, k, at) = disk::decode(&file).expect("what was written reads");
    assert_eq!(k, key);
    assert_eq!(&file[at..], rest);
    assert_eq!(fields_of(&e), fields_of(&again));
    let name = disk::file_name(key, &e.vary);
    assert_eq!(name.len(), 43, "{name}");
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
    fn corpus_cache_semantics() {
        replay("cache_semantics", cache_semantics);
    }

    #[test]
    fn corpus_cache_disk() {
        replay("cache_disk", cache_disk);
    }
}
