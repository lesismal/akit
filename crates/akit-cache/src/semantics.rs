//! RFC 9111's rules, as fit's `middleware/cache/semantics.go` has them: `Cache-Control`
//! directives, freshness lifetimes, `Vary`, and the preconditions a stored response answers.

use std::time::{Duration, SystemTime};

use akio_http::Headers;

/// The `Cache-Control` directives the cache reads, from a request or a response. A duration is
/// `None` when the directive is absent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Directives {
    pub(crate) max_age: Option<Duration>,
    pub(crate) s_maxage: Option<Duration>,
    pub(crate) max_stale: Option<Duration>,
    pub(crate) min_fresh: Option<Duration>,
    /// `max-stale` with no value: any staleness will do.
    pub(crate) max_stale_any: bool,
    pub(crate) no_store: bool,
    pub(crate) no_cache: bool,
    pub(crate) private: bool,
    pub(crate) public: bool,
    pub(crate) must_revalidate: bool,
    pub(crate) proxy_revalidate: bool,
    pub(crate) only_if_cached: bool,
    /// RFC 5861's `stale-while-revalidate` and `stale-if-error`.
    pub(crate) stale_while_revalidate: Option<Duration>,
    pub(crate) stale_if_error: Option<Duration>,
}

/// The bound on delta-seconds, as RFC 9111 section 1.2.2 has caches keep it: 2^31 seconds.
pub(crate) const MAX_DELTA: Duration = Duration::from_secs(1 << 31);

/// The directives of `Cache-Control` fields. A qualified `no-cache` or `private`, naming fields,
/// is taken as the unqualified one, as a cache that does not track fields must. A delta that does
/// not parse is taken as 0, which makes a response stale, as section 4.2.1 says to.
pub(crate) fn parse_directives<'a>(fields: impl Iterator<Item = &'a str>) -> Directives {
    let mut d = Directives::default();
    for field in fields {
        for member in field.split(',') {
            let (name, value) = member.split_once('=').unwrap_or((member, ""));
            let name = name.trim();
            let value = value.trim().trim_matches('"');
            let is = |n: &str| name.eq_ignore_ascii_case(n);
            if is("max-age") {
                d.max_age = Some(delta(value));
            } else if is("s-maxage") {
                d.s_maxage = Some(delta(value));
            } else if is("max-stale") {
                if value.is_empty() {
                    d.max_stale_any = true;
                } else {
                    d.max_stale = Some(delta(value));
                }
            } else if is("min-fresh") {
                d.min_fresh = Some(delta(value));
            } else if is("no-store") {
                d.no_store = true;
            } else if is("no-cache") {
                d.no_cache = true;
            } else if is("private") {
                d.private = true;
            } else if is("public") {
                d.public = true;
            } else if is("must-revalidate") {
                d.must_revalidate = true;
            } else if is("proxy-revalidate") {
                d.proxy_revalidate = true;
            } else if is("only-if-cached") {
                d.only_if_cached = true;
            } else if is("stale-while-revalidate") {
                d.stale_while_revalidate = Some(delta(value));
            } else if is("stale-if-error") {
                d.stale_if_error = Some(delta(value));
            }
        }
    }
    d
}

/// delta-seconds: 0 when it is not a number, at most [`MAX_DELTA`].
pub(crate) fn delta(s: &str) -> Duration {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Duration::ZERO;
    }
    match s.parse::<u64>() {
        Ok(n) => Duration::from_secs(n).min(MAX_DELTA),
        // Only digits: too large.
        Err(_) => MAX_DELTA,
    }
}

/// A request's directives: its `Cache-Control`, and `Pragma: no-cache` when it has no
/// `Cache-Control`, as section 5.4 allows for HTTP/1.0 clients.
pub(crate) fn request_directives(h: &Headers) -> Directives {
    let mut d = parse_directives(h.get_all("cache-control"));
    if h.get("cache-control").is_none()
        && h.get_all("pragma")
            .any(|v| contains_ignoring_case(v, "no-cache"))
    {
        d.no_cache = true;
    }
    d
}

/// Whether `haystack` holds `needle` (lower case), ignoring ASCII case.
fn contains_ignoring_case(haystack: &str, needle: &str) -> bool {
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

/// Whether a response of `status` may be given a heuristic lifetime, or one from the config,
/// when it has none of its own: those RFC 9110 section 15.1 calls heuristically cacheable, but
/// 204 and 206, whose bodies the cache does not keep.
pub(crate) fn heuristic(status: u16) -> bool {
    matches!(
        status,
        200 | 203 | 300 | 301 | 308 | 404 | 405 | 410 | 414 | 501
    )
}

/// Whether the cache keeps responses of `status` at all: the heuristic ones, and the temporary
/// redirects when they give a lifetime of their own.
pub(crate) fn storable_status(status: u16, explicit: bool) -> bool {
    heuristic(status) || explicit && matches!(status, 302 | 307)
}

/// The bound on a lifetime made from `Last-Modified`.
pub(crate) const MAX_HEURISTIC: Duration = Duration::from_secs(24 * 3600);

/// An HTTP-date in any of RFC 9110's three formats.
pub(crate) fn http_date(s: &str) -> Option<SystemTime> {
    akio_http::parse_http_date(s)
        .and_then(|secs| SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(secs)))
}

/// A response's freshness lifetime, as section 4.2.1 has a shared cache work it out, and whether
/// the response gave one itself. `date` is when it was made. One that gives none gets
/// `default_ttl`, when set, or a tenth of the time since it was last modified, at most a day.
pub(crate) fn freshness(
    h: &Headers,
    d: &Directives,
    status: u16,
    date: SystemTime,
    default_ttl: Option<Duration>,
) -> (Duration, bool) {
    if let Some(t) = d.s_maxage.or(d.max_age) {
        return (t, true);
    }
    if h.get("expires").is_some() {
        // The first one; one that is not a date has already expired, as "0" has.
        let t = h
            .get("expires")
            .and_then(http_date)
            .and_then(|t| t.duration_since(date).ok())
            .unwrap_or(Duration::ZERO);
        return (t, true);
    }
    if !heuristic(status) && !d.public {
        return (Duration::ZERO, false);
    }
    if let Some(t) = default_ttl {
        return (t, false);
    }
    let since = h
        .get("last-modified")
        .and_then(http_date)
        .and_then(|lm| date.duration_since(lm).ok())
        .filter(|t| !t.is_zero());
    match since {
        Some(t) => (t.checked_div(10).unwrap_or(t).min(MAX_HEURISTIC), false),
        None => (Duration::ZERO, false),
    }
}

/// The request fields a response's `Vary` names, in lower case, or `None` for `Vary: *`, which
/// no request matches.
pub(crate) fn vary_names(h: &Headers) -> Option<Vec<String>> {
    let mut names = Vec::new();
    for v in h.get_all("vary") {
        for name in v.split(',') {
            match name.trim() {
                "" => {}
                "*" => return None,
                n => names.push(n.to_ascii_lowercase()),
            }
        }
    }
    Some(names)
}

/// A request field's value as variants are told apart by: its lines joined, the space around
/// each list member taken out, so that `gzip, br` and `gzip,br` select the same variant.
pub(crate) fn vary_value(h: &Headers, name: &str) -> String {
    let mut lines = h.get_all(name);
    let Some(first) = lines.next() else {
        return String::new();
    };
    let mut rest = lines.peekable();
    if rest.peek().is_none() && !first.contains(", ") && first.trim() == first {
        return first.to_owned();
    }
    let mut out = String::with_capacity(first.len());
    for line in std::iter::once(first).chain(rest) {
        for member in line.split(',') {
            if !out.is_empty() {
                out.push(',');
            }
            out.push_str(member.trim());
        }
    }
    out
}

/// Whether a request's preconditions are met by a stored response with `etag` and
/// `last_modified`, so that it is answered 304: its `If-None-Match` lists the ETag, weakly
/// compared, or `*`; or, with no `If-None-Match`, its `If-Modified-Since` is no earlier than
/// `Last-Modified`.
pub(crate) fn not_modified(h: &Headers, etag: Option<&str>, last_modified: Option<&str>) -> bool {
    if h.get("if-none-match").is_some() {
        let Some(etag) = etag else { return false };
        return h.get_all("if-none-match").any(|line| {
            line.split(',').any(|m| {
                let m = m.trim();
                m == "*" || weak_equal(m, etag)
            })
        });
    }
    let (Some(since), Some(lm)) = (
        h.get("if-modified-since").and_then(http_date),
        last_modified.and_then(http_date),
    ) else {
        return false;
    };
    lm <= since
}

/// Entity tags compared weakly: their opaque parts match, whether either is weak or not.
pub(crate) fn weak_equal(a: &str, b: &str) -> bool {
    a.strip_prefix("W/").unwrap_or(a) == b.strip_prefix("W/").unwrap_or(b)
}

/// The fields a stored response is kept without: those of one connection, `Age` (worked out when
/// served), the length (the stored body gives it) and `Cache-Status`.
pub(crate) fn unstored(name: &str) -> bool {
    const UNSTORED: &[&str] = &[
        "connection",
        "keep-alive",
        "proxy-connection",
        "transfer-encoding",
        "upgrade",
        "te",
        "trailer",
        "content-length",
        "age",
        "cache-status",
    ];
    UNSTORED.iter().any(|u| name.eq_ignore_ascii_case(u))
}

/// The fields of a stored response a 304 made from it carries (RFC 9110 section 15.4.5).
pub(crate) const NOT_MODIFIED_FIELDS: &[&str] = &[
    "Cache-Control",
    "Content-Location",
    "Date",
    "ETag",
    "Expires",
    "Last-Modified",
    "Vary",
];

/// The fields a 304 does not change in the stored response it freshens (RFC 9111 section 3.2).
pub(crate) fn update_excluded(name: &str) -> bool {
    const EXCLUDED: &[&str] = &[
        "content-length",
        "content-encoding",
        "content-type",
        "content-range",
    ];
    EXCLUDED.iter().any(|u| name.eq_ignore_ascii_case(u))
}

/// Whether a request with `method` may change what its target holds, so that its success makes
/// stored responses out of date.
pub(crate) fn unsafe_method(method: &str) -> bool {
    !matches!(method, "GET" | "HEAD" | "OPTIONS" | "TRACE")
}

/// The most tags a response is kept with, and the longest tag: fit's `maxTags` and `maxTagLen`.
pub(crate) const MAX_TAGS: usize = 32;
pub(crate) const MAX_TAG_LEN: usize = 128;

/// A response's tags, by which it can be purged, as fit's `tagsOf`: the members of its
/// `Cache-Tag` fields, comma-separated, and of its `Surrogate-Key` fields, space-separated, each
/// once, at most [`MAX_TAGS`] of them, none longer than [`MAX_TAG_LEN`].
pub(crate) fn tags_of(h: &Headers) -> Box<[Box<str>]> {
    let mut tags: Vec<Box<str>> = Vec::new();
    let mut add = |tag: &str| {
        let tag = tag.trim();
        if !tag.is_empty()
            && tag.len() <= MAX_TAG_LEN
            && tags.len() < MAX_TAGS
            && !tags.iter().any(|t| &**t == tag)
        {
            tags.push(tag.into());
        }
    };
    for v in h.get_all("cache-tag") {
        v.split(',').for_each(&mut add);
    }
    for v in h.get_all("surrogate-key") {
        v.split_ascii_whitespace().for_each(&mut add);
    }
    tags.into_boxed_slice()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn fields(pairs: &[(&str, &str)]) -> Headers {
        let mut h = Headers::new();
        for (n, v) in pairs {
            h.append(n, v);
        }
        h
    }

    // fit's TestSemantics, its parts on directives and deltas.
    #[test]
    fn directives_and_deltas() {
        let d = parse_directives(
            [
                r#"Max-Age="60", s-maxage=x, no-cache="Set-Cookie""#,
                "private, max-stale, min-fresh=5, only-if-cached, PUBLIC",
                "stale-while-revalidate=30, Stale-If-Error=\"600\"",
            ]
            .into_iter(),
        );
        assert_eq!(d.stale_while_revalidate, Some(Duration::from_secs(30)));
        assert_eq!(d.stale_if_error, Some(Duration::from_secs(600)));
        assert_eq!(d.max_age, Some(Duration::from_secs(60)));
        assert_eq!(d.s_maxage, Some(Duration::ZERO), "a bad delta is 0");
        assert!(d.no_cache && d.private && d.public && d.only_if_cached && d.max_stale_any);
        assert_eq!(d.min_fresh, Some(Duration::from_secs(5)));
        assert_eq!(d.max_stale, None);
        assert_eq!(delta("99999999999999999999999"), MAX_DELTA);
        assert_eq!(delta("4294967296"), MAX_DELTA);
        assert_eq!(delta("-1"), Duration::ZERO);
        assert_eq!(delta(""), Duration::ZERO);
        assert_eq!(delta("10"), Duration::from_secs(10));
        let r = request_directives(&fields(&[("Pragma", "No-Cache")]));
        assert!(r.no_cache);
        let r = request_directives(&fields(&[
            ("Pragma", "no-cache"),
            ("Cache-Control", "max-age=1"),
        ]));
        assert!(!r.no_cache, "Pragma counts only without Cache-Control");
    }

    #[test]
    fn freshness_lifetimes() {
        let date = http_date("Mon, 02 Jan 2006 15:04:05 GMT").unwrap();
        let none = Directives::default();
        let h = fields(&[("Expires", "Mon, 02 Jan 2006 15:05:05 GMT")]);
        assert_eq!(
            freshness(&h, &none, 200, date, None),
            (Duration::from_secs(60), true)
        );
        let h = fields(&[("Expires", "0")]);
        assert_eq!(
            freshness(&h, &none, 200, date, None),
            (Duration::ZERO, true)
        );
        let h = fields(&[("Last-Modified", "Mon, 02 Jan 2006 15:02:25 GMT")]);
        assert_eq!(
            freshness(&h, &none, 200, date, None),
            (Duration::from_secs(10), false)
        );
        assert_eq!(
            freshness(&h, &none, 200, date, Some(Duration::from_secs(7))),
            (Duration::from_secs(7), false)
        );
        assert_eq!(
            freshness(&h, &none, 302, date, None),
            (Duration::ZERO, false)
        );
        let h = fields(&[("Last-Modified", "Mon, 02 Jan 2000 15:02:25 GMT")]);
        assert_eq!(
            freshness(&h, &none, 404, date, None),
            (MAX_HEURISTIC, false)
        );
        let d = parse_directives(["max-age=5, s-maxage=9"].into_iter());
        assert_eq!(
            freshness(&h, &d, 500, date, None),
            (Duration::from_secs(9), true)
        );
        assert!(storable_status(302, true) && !storable_status(302, false));
        assert!(!storable_status(500, true) && storable_status(404, false));
    }

    #[test]
    fn vary_and_preconditions() {
        assert_eq!(
            vary_names(&fields(&[
                ("Vary", "Accept-Encoding, , Origin"),
                ("Vary", "X")
            ])),
            Some(vec!["accept-encoding".into(), "origin".into(), "x".into()])
        );
        assert_eq!(vary_names(&fields(&[("Vary", "a, *")])), None);
        let h = fields(&[
            ("Accept-Encoding", "gzip, br"),
            ("Accept-Encoding", " zstd"),
        ]);
        assert_eq!(vary_value(&h, "accept-encoding"), "gzip,br,zstd");
        let h = fields(&[("Accept-Encoding", "gzip,br")]);
        assert_eq!(vary_value(&h, "accept-encoding"), "gzip,br");
        assert_eq!(vary_value(&h, "origin"), "");
        let lm = Some("Mon, 02 Jan 2006 15:04:05 GMT");
        let etag = Some(r#""v1""#);
        for (pairs, want) in [
            (vec![("If-None-Match", r#"W/"v1""#)], true),
            (vec![("If-None-Match", r#""x", "v1""#)], true),
            (vec![("If-None-Match", "*")], true),
            (
                vec![
                    ("If-None-Match", r#""v2""#),
                    ("If-Modified-Since", "Tue, 03 Jan 2006 15:04:05 GMT"),
                ],
                false,
            ),
            (
                vec![("If-Modified-Since", "Mon, 02 Jan 2006 15:04:05 GMT")],
                true,
            ),
            (
                vec![("If-Modified-Since", "Sunday, 01-Jan-06 15:04:05 GMT")],
                false,
            ),
            (vec![("If-Modified-Since", "garbage")], false),
            (vec![], false),
        ] {
            assert_eq!(not_modified(&fields(&pairs), etag, lm), want, "{pairs:?}");
        }
        assert!(!not_modified(&fields(&[("If-None-Match", "*")]), None, lm));
        assert!(unstored("Transfer-Encoding") && !unstored("ETag"));
        assert!(unsafe_method("POST") && !unsafe_method("OPTIONS"));
    }

    // fit's TestTagsOf.
    #[test]
    fn tags() {
        let long = "x".repeat(200);
        let h = fields(&[
            ("Cache-Tag", "a, b,, a"),
            ("Cache-Tag", &long),
            ("Surrogate-Key", "c  d b"),
        ]);
        let tags = tags_of(&h);
        let got: Vec<&str> = tags.iter().map(|t| &**t).collect();
        assert_eq!(got, ["a", "b", "c", "d"]);
        let many: Vec<String> = (0..40).map(|i| i.to_string()).collect();
        let h = fields(&[("Cache-Tag", &many.join(","))]);
        assert_eq!(tags_of(&h).len(), MAX_TAGS);
        assert!(tags_of(&Headers::new()).is_empty());
    }
}
