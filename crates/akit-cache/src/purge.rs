//! Purging, as fit's `middleware/cache/purge.go`: what a zone keeps, in memory and on disk,
//! removed by key (the absolute URLs requests name), by URL prefix, by tag (`Cache-Tag`,
//! `Surrogate-Key`), or all of it; and what each zone keeps. The Admin API's `POST /cache/purge`
//! and `GET /cache` (I69).
//!
//! A purge runs where it is called (the Admin API's I/O thread), never on a data plane thread:
//! it takes each shard's lock, and the disk index's, for as long as finding the keys takes (a
//! lookup for a tag; going through the keys for a prefix), as clearing them takes, and removes
//! nothing from the disk itself: the files go on the disk pool, as an eviction's do.

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use crate::store::{Store, zones_named};

/// What [`purge`] removes: the responses to `urls`, every variant of each; those whose URLs
/// start with any of `prefixes`; those tagged with any of `tags`; or, with `all`, everything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    /// Absolute `http` or `https` URLs, as requests name them.
    pub urls: Vec<String>,
    /// The starts of absolute `http` or `https` URLs: `https://example.com/img/`.
    pub prefixes: Vec<String>,
    /// Tags, as `Cache-Tag` (comma-separated) and `Surrogate-Key` (space-separated) name them.
    pub tags: Vec<String>,
    /// Everything the zone keeps.
    pub all: bool,
}

impl Selection {
    /// Whether it names nothing to remove.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.urls.is_empty() && self.prefixes.is_empty() && self.tags.is_empty() && !self.all
    }
}

/// Why [`purge`] removed nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PurgeError {
    /// No cache uses the zone named: fit's `ErrNoZone`, 404 at the Admin API.
    NoZone(String),
    /// A URL or a prefix is not one: 400 at the Admin API.
    Invalid(String),
}

impl fmt::Display for PurgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PurgeError::NoZone(z) => write!(f, "no cache uses the zone {z:?}"),
            PurgeError::Invalid(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for PurgeError {}

/// Removes what the zone named `zone` keeps, or every zone's for `""`, as `sel` selects: from
/// memory and from disk, where a write still queued is dropped too. Returns how many keys it
/// removed, as fit's `PurgeBy`.
///
/// # Errors
///
/// [`PurgeError::NoZone`] when no cache uses `zone`; [`PurgeError::Invalid`] for a URL or
/// prefix that is not an absolute `http` or `https` one (a prefix with user information or a
/// fragment is refused too). Nothing is removed then.
pub fn purge(zone: &str, sel: &Selection) -> Result<usize, PurgeError> {
    let keys = sel
        .urls
        .iter()
        .map(|u| url_key(u, false))
        .collect::<Result<Vec<_>, _>>()?;
    let prefixes = sel
        .prefixes
        .iter()
        .map(|p| url_key(p, true))
        .collect::<Result<Vec<_>, _>>()?;
    let stores = zones_named(zone);
    if !zone.is_empty() && stores.is_empty() {
        return Err(PurgeError::NoZone(zone.to_owned()));
    }
    let mut n = 0usize;
    for s in &stores {
        n = n.saturating_add(if sel.all {
            clear(s)
        } else {
            select(s, &keys, &prefixes, &sel.tags)
        });
    }
    Ok(n)
}

/// Drops everything `s` keeps, in memory and on disk; returns how many keys it held, in either.
fn clear(s: &Store) -> usize {
    let n = s.clear();
    match s.disk() {
        Some(d) => n.max(d.clear()),
        None => n,
    }
}

/// Drops what `s` keeps under `keys`, under keys starting with any of `prefixes`, and under keys
/// tagged with any of `tags`; returns how many keys it dropped.
fn select(s: &Store, keys: &[String], prefixes: &[String], tags: &[String]) -> usize {
    let disk = s.disk();
    let mut found: Vec<Arc<str>> = Vec::new();
    for tag in tags {
        s.tagged(tag, &mut found);
        if let Some(d) = &disk {
            d.tagged(tag, &mut found);
        }
    }
    if !prefixes.is_empty() {
        s.keys_with(prefixes, &mut found);
        if let Some(d) = &disk {
            d.keys_with(prefixes, &mut found);
        }
    }
    let mut purged: HashSet<&str> = HashSet::new();
    let candidates = found
        .iter()
        .map(|k| &**k)
        .chain(keys.iter().map(String::as_str));
    for k in candidates {
        if purged.contains(k) {
            continue;
        }
        let mut removed = s.remove(k);
        if let Some(d) = &disk {
            // Removed on disk even when it holds nothing there now: a write queued is dropped.
            removed |= d.remove(k);
        }
        if removed {
            purged.insert(k);
        }
    }
    purged.len()
}

/// The key of the response to a request for `u`, as fit's `urlKey` (or, with `prefix`, the
/// start of the keys of URLs starting with `u`, as fit's `prefixKey`): its scheme, its host in
/// lower case, and its path (`/` when it has none) and query as written.
pub(crate) fn url_key(u: &str, prefix: bool) -> Result<String, PurgeError> {
    let what = if prefix {
        "the start of an absolute http or https URL"
    } else {
        "an absolute http or https URL"
    };
    let bad = || PurgeError::Invalid(format!("{u:?} is not {what}"));
    let (scheme, rest) = u.split_once("://").ok_or_else(bad)?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(bad());
    }
    // A URL whose bytes a request target could not carry, or whose escapes are broken, is not one.
    if u.bytes().any(|b| b <= b' ' || b == 0x7f) || !escapes_valid(u) {
        return Err(bad());
    }
    let (rest, fragment) = match rest.split_once('#') {
        Some((r, _)) => (r, true),
        None => (rest, false),
    };
    if prefix && fragment {
        return Err(bad());
    }
    let at = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, target) = rest.split_at(at);
    let host = match authority.rsplit_once('@') {
        Some(_) if prefix => return Err(bad()),
        Some((_, h)) => h,
        None => authority,
    };
    if host.is_empty() || host.contains('%') {
        return Err(bad());
    }
    let mut key = String::with_capacity(scheme.len().saturating_add(u.len()));
    key.push_str(&scheme);
    key.push_str("://");
    key.extend(host.chars().map(|c| c.to_ascii_lowercase()));
    if !target.starts_with('/') {
        key.push('/');
    }
    key.push_str(target);
    Ok(key)
}

/// Whether every `%` in `u` starts an escape of two hex digits.
fn escapes_valid(u: &str) -> bool {
    let b = u.as_bytes();
    b.iter().enumerate().all(|(i, &c)| {
        c != b'%'
            || (b
                .get(i.saturating_add(1))
                .is_some_and(u8::is_ascii_hexdigit)
                && b.get(i.saturating_add(2))
                    .is_some_and(u8::is_ascii_hexdigit))
    })
}

/// What a zone keeps, as fit's `ZoneStats` and `GET /cache` show it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ZoneStats {
    /// The zone's name.
    pub zone: String,
    /// The keys in memory.
    pub keys: usize,
    /// What they take in memory, in bytes.
    pub bytes: u64,
    /// The responses on disk, when the zone has a disk tier.
    pub disk_entries: u64,
    /// What they take on disk, in bytes.
    pub disk_bytes: u64,
    /// The disk tier's directory.
    pub disk_path: Option<String>,
}

impl ZoneStats {
    /// As fit's `GET /cache` writes it: `{"zone":…,"keys":…,"bytes":…}`, with
    /// `disk_entries`, `disk_bytes` and `disk_path` when they are not zero or empty.
    pub fn write_json(&self, out: &mut String) {
        use std::fmt::Write as _;
        out.push_str("{\"zone\":");
        json_string(out, &self.zone);
        let _ = write!(out, ",\"keys\":{},\"bytes\":{}", self.keys, self.bytes);
        if self.disk_entries != 0 {
            let _ = write!(out, ",\"disk_entries\":{}", self.disk_entries);
        }
        if self.disk_bytes != 0 {
            let _ = write!(out, ",\"disk_bytes\":{}", self.disk_bytes);
        }
        if let Some(p) = self.disk_path.as_deref().filter(|p| !p.is_empty()) {
            out.push_str(",\"disk_path\":");
            json_string(out, p);
        }
        out.push('}');
    }
}

fn json_string(out: &mut String, s: &str) {
    use std::fmt::Write as _;
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// What each zone in use keeps, by name.
#[must_use]
pub fn zones() -> Vec<ZoneStats> {
    zones_named("")
        .iter()
        .map(|s| {
            let (keys, bytes) = s.stats();
            let disk = s.disk();
            let (disk_entries, disk_bytes) = disk.as_ref().map_or((0, 0), |d| d.stats());
            ZoneStats {
                zone: s.name().to_owned(),
                keys,
                bytes,
                disk_entries,
                disk_bytes,
                disk_path: disk.map(|d| d.dir().display().to_string()),
            }
        })
        .collect()
}

/// `zones()` as fit's `GET /cache` answers it: a JSON array.
#[must_use]
pub fn zones_json() -> String {
    let mut out = String::from("[");
    for (i, z) in zones().iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        z.write_json(&mut out);
    }
    out.push(']');
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn keys() {
        for (u, want) in [
            ("http://Example.COM/a?b=C", "http://example.com/a?b=C"),
            ("HTTPS://h:8443", "https://h:8443/"),
            ("http://h?x", "http://h/?x"),
            ("http://u@h/p#f", "http://h/p"),
            ("http://h/%41", "http://h/%41"),
        ] {
            assert_eq!(url_key(u, false).unwrap(), want, "{u}");
        }
        assert_eq!(url_key("http://H/img/", true).unwrap(), "http://h/img/");
        for u in [
            "/relative",
            "ftp://h/",
            "http:///p",
            "http://h/a b",
            "http://%zz",
        ] {
            assert!(url_key(u, false).is_err(), "{u}");
        }
        // fit's prefixes refused.
        for p in [
            "/relative",
            "ftp://h/",
            "http://u@h/",
            "http://h/#f",
            "http://%zz",
        ] {
            assert!(url_key(p, true).is_err(), "{p}");
        }
    }

    #[test]
    fn zone_json() {
        let mut out = String::new();
        ZoneStats {
            zone: "a\"b".into(),
            keys: 2,
            bytes: 10,
            disk_entries: 0,
            disk_bytes: 0,
            disk_path: None,
        }
        .write_json(&mut out);
        assert_eq!(out, r#"{"zone":"a\"b","keys":2,"bytes":10}"#);
        out.clear();
        ZoneStats {
            zone: "d".into(),
            keys: 0,
            bytes: 0,
            disk_entries: 1,
            disk_bytes: 5,
            disk_path: Some("/c".into()),
        }
        .write_json(&mut out);
        assert_eq!(
            out,
            r#"{"zone":"d","keys":0,"bytes":0,"disk_entries":1,"disk_bytes":5,"disk_path":"/c"}"#
        );
    }
}
