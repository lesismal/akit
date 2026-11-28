//! Playback URLs that expire (I144, fit's `mediaauth/urlsign.go` and `sessions.go`): a URL signed
//! for one viewer and one resource, the path prefix of an item of `vod` (`/vod/<library>/<id>/`)
//! or of a stream (`/hls/live/cam/`, `/live/cam/` for `/live/cam.flv`), its grant in its query,
//!
//! ```text
//! ?exp=<unix seconds>&kid=<key>&ip=<address or network>&sid=<session>&sub=<subject>&sig=<MAC>
//! ```
//!
//! `kid`, `ip` and `sid` when there are, and `sig`, the first 16 bytes of an HMAC-SHA256 over the
//! resource and the fields, in base64url, under a key derived from the policy's secret or the
//! keyring's key `kid`. The format is fit's byte for byte (decision 043): a URL fit signs is taken
//! by akit, and one akit signs by fit, given the same secret and policy name. A playlist fetched
//! with a URL hands what it names a grant of its own (`g=1`), living `play_grace` past the later
//! of the URL's expiry and the item's end, in the query of every URI or as the cookie
//! `akit_media_grant` (fit's `fit_media_grant`, decision 037).
//!
//! A check runs on the I/O thread with no callback (rule 1): the MAC, the expiry, the address,
//! `Referer` and `Origin`, then the session's entry in a table of the policy's own, in shards by
//! subject, each under a lock held for a few comparisons, and never past `max_tracked_sessions`.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use akio_crypto::hash::Sha256;
use akio_crypto::mac::Hmac;
use akit_config::media::{UrlSigning, addr_or_prefix};

/// The cookie a playlist hands its grant on in, with `url_signing`'s `cookie`.
pub const GRANT_COOKIE: &str = "akit_media_grant";

const SIG_BYTES: usize = 16;
const SIG_CHARS: usize = 22;
const MAX_SUBJECT_BYTES: usize = 128;
/// The longest a session's ID may be.
pub const MAX_SESSION_BYTES: usize = 64;
const MAX_RESOURCE_BYTES: usize = 1024;

/// Why a URL, or a grant, was refused: 403 with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlReason {
    /// Past its expiry.
    Expired,
    /// Another resource, a field changed, a key rotated or removed, or one that does not parse.
    Signature,
    /// Another viewer's address.
    Address,
    /// A `Referer` not in `referers`.
    Referer,
    /// An `Origin` not in `origins`.
    Origin,
    /// A new session past `max_sessions`.
    SessionLimit,
    /// A session `on_limit` ended.
    SessionEnded,
}

/// The reasons, in the order the counts keep them.
pub const URL_REASONS: [UrlReason; 7] = [
    UrlReason::Expired,
    UrlReason::Signature,
    UrlReason::Address,
    UrlReason::Referer,
    UrlReason::Origin,
    UrlReason::SessionLimit,
    UrlReason::SessionEnded,
];

impl UrlReason {
    /// Its name, as fit's `URLReasons`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            UrlReason::Expired => "expired",
            UrlReason::Signature => "signature",
            UrlReason::Address => "address",
            UrlReason::Referer => "referer",
            UrlReason::Origin => "origin",
            UrlReason::SessionLimit => "session_limit",
            UrlReason::SessionEnded => "session_ended",
        }
    }

    fn index(self) -> usize {
        URL_REASONS.iter().position(|r| *r == self).unwrap_or(0)
    }
}

/// A policy's `url_signing`, compiled.
#[derive(Debug, Clone)]
pub(crate) struct UrlConfig {
    pub(crate) ttl: Duration,
    grace: Duration,
    pub(crate) idle: Duration,
    subnet: bool,
    pub(crate) cookie: bool,
    max_sessions: usize,
    end_oldest: bool,
    tracked: usize,
    referers: Vec<String>,
    origins: Vec<String>,
    pub(crate) trusted: Vec<(IpAddr, u8)>,
}

impl UrlConfig {
    pub(crate) fn compile(u: &UrlSigning) -> Result<UrlConfig, String> {
        let mut trusted = Vec::new();
        for s in &u.trusted_proxies {
            trusted.push(addr_or_prefix(s).ok_or_else(|| {
                format!("url_signing.trusted_proxies: {s:?} is not an IP address or a network")
            })?);
        }
        Ok(UrlConfig {
            ttl: u.url_ttl(),
            grace: u.grace(),
            idle: u.idle(),
            subnet: u.subnet,
            cookie: u.cookie,
            max_sessions: u.max_sessions,
            end_oldest: u.end_oldest,
            tracked: u.tracked(),
            referers: u.referers.clone(),
            origins: u.origins.clone(),
            trusted,
        })
    }
}

/// The key URLs are signed with, derived from a signing key (fit's `newURLKey`).
pub(crate) fn url_key(sign_key: &[u8]) -> Vec<u8> {
    let mut m = Hmac::<Sha256>::new(sign_key);
    m.update(b"fit media url\x00");
    m.finalize().to_vec()
}

/// The fields of a signed URL or of a grant.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grant {
    pub(crate) exp: i64,
    pub(crate) kid: String,
    pub(crate) ip: String,
    pub(crate) sid: String,
    pub(crate) sub: String,
    pub(crate) child: bool,
    pub(crate) sig: String,
}

fn b64(b: &[u8]) -> String {
    akio_crypto::base64::URL_SAFE_NO_PAD.encode(b)
}

impl Grant {
    /// The MAC of the grant for `resource` under `key`, in base64url (fit's `urlKey.mac`).
    pub(crate) fn mac(&self, key: &[u8], resource: &str) -> String {
        let mut m = Hmac::<Sha256>::new(key);
        m.update(b"fit-url-1\n");
        m.update(resource.as_bytes());
        m.update(b"\n");
        m.update(self.exp.to_string().as_bytes());
        for f in [&self.kid, &self.ip, &self.sid, &self.sub] {
            m.update(b"\n");
            m.update(f.as_bytes());
        }
        m.update(if self.child { b"\n1" } else { b"\n0" });
        b64(m.finalize().get(..SIG_BYTES).unwrap_or_default())
    }

    /// The grant as a query, without a leading '?' (fit's `appendQuery`).
    #[must_use]
    pub fn query(&self) -> String {
        let mut b = format!("exp={}", self.exp);
        for (k, v) in [("kid", &self.kid), ("ip", &self.ip), ("sid", &self.sid)] {
            if !v.is_empty() {
                b.push('&');
                b.push_str(k);
                b.push('=');
                b.push_str(v);
            }
        }
        b.push_str("&sub=");
        b.push_str(&crate::handlers::query_escape(&self.sub));
        if self.child {
            b.push_str("&g=1");
        }
        b.push_str("&sig=");
        b.push_str(&self.sig);
        b
    }

    /// A grant read from `q`, a query or a cookie's value of the same form (fit's `parse`): `None`
    /// when it lacks a field, has one twice, or has one that is not as a grant's may be.
    #[must_use]
    pub fn parse(q: &str) -> Option<Grant> {
        let mut g = Grant::default();
        let mut seen = 0u8;
        for kv in q.split('&') {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            let bit = match k {
                "exp" => 1,
                "kid" => 2,
                "ip" => 4,
                "sid" => 8,
                "sub" => 16,
                "g" => 32,
                "sig" => 64,
                _ => continue,
            };
            if seen & bit != 0 {
                return None;
            }
            seen |= bit;
            let v = if v.contains(['%', '+']) {
                crate::auth::unescape(v)?
            } else {
                v.to_owned()
            };
            match bit {
                1 => g.exp = v.parse::<i64>().ok().filter(|n| *n > 0)?,
                2 => g.kid = v,
                4 => g.ip = v,
                8 => g.sid = v,
                16 => g.sub = v,
                32 => {
                    if v != "1" {
                        return None;
                    }
                    g.child = true;
                }
                _ => g.sig = v,
            }
        }
        let ok = seen & (1 | 16 | 64) == 1 | 16 | 64
            && valid_subject(&g.sub)
            && g.sig.len() == SIG_CHARS
            && (g.sid.is_empty() || valid_session(&g.sid))
            && (g.kid.is_empty() || akit_config::media::valid_key_name(&g.kid))
            && g.ip.len() <= 64;
        ok.then_some(g)
    }
}

/// Whether `s` is a subject a grant may carry: 1 to 128 bytes with no control character.
#[must_use]
pub fn valid_subject(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_SUBJECT_BYTES && !s.bytes().any(|b| b < 0x20 || b == 0x7f)
}

/// Whether `s` is a playback session's ID: 1 to 64 letters, digits, '.', '_', '~' and '-'.
#[must_use]
pub fn valid_session(s: &str) -> bool {
    akit_config::media::valid_key_name(s) && s.len() <= MAX_SESSION_BYTES
}

fn valid_path_chars(p: &str) -> bool {
    p.bytes()
        .all(|c| c.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/".contains(&c))
}

/// Whether `r` is a resource a URL may be signed for: a path of plain segments ending in '/'.
#[must_use]
pub fn valid_resource(r: &str) -> bool {
    r.len() >= 3
        && r.len() <= MAX_RESOURCE_BYTES
        && r.starts_with('/')
        && r.ends_with('/')
        && valid_path_chars(r)
        && r.get(1..r.len().saturating_sub(1)).is_some_and(|mid| {
            mid.split('/')
                .all(|s| !s.is_empty() && s != "." && s != "..")
        })
}

/// The resource a URL of `path` is signed for (fit's `ResourceOf`): the path up to its last '/',
/// or for an HTTP-FLV stream the path without `.flv` and with '/'.
#[must_use]
pub fn resource_of(path: &str) -> Option<String> {
    if path.len() > MAX_RESOURCE_BYTES || !path.starts_with('/') || path.ends_with('/') {
        return None;
    }
    let res = match path.strip_suffix(".flv") {
        Some(s) if !s.ends_with('/') => format!("{s}/"),
        _ => {
            let i = path.rfind('/')?;
            path.get(..=i)?.to_owned()
        }
    };
    (valid_resource(&res) && valid_path_chars(path)).then_some(res)
}

/// The time now in seconds since 1970.
pub(crate) fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// The time now in nanoseconds since 1970.
pub(crate) fn now_nanos() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
}

/// The address bound into a URL for `a`: the address, or its /24 or /64 with `subnet`.
pub(crate) fn bind(a: IpAddr, subnet: bool) -> String {
    let a = match a {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    };
    if !subnet {
        return a.to_string();
    }
    match a {
        IpAddr::V4(v) => {
            let o = v.octets();
            format!("{}.{}.{}.0/24", o[0], o[1], o[2])
        }
        IpAddr::V6(v) => {
            let mut s = v.segments();
            for x in s.iter_mut().skip(4) {
                *x = 0;
            }
            format!("{}/64", std::net::Ipv6Addr::from(s))
        }
    }
}

fn in_prefix(a: IpAddr, p: IpAddr, bits: u8) -> bool {
    match (a, p) {
        (IpAddr::V4(a), IpAddr::V4(p)) => {
            let m = u32::MAX
                .checked_shl(32u32.saturating_sub(u32::from(bits)))
                .unwrap_or(0);
            u32::from(a) & m == u32::from(p) & m
        }
        (IpAddr::V6(a), IpAddr::V6(p)) => {
            let m = u128::MAX
                .checked_shl(128u32.saturating_sub(u32::from(bits)))
                .unwrap_or(0);
            u128::from(a) & m == u128::from(p) & m
        }
        _ => false,
    }
}

/// Whether `a` is the address or in the network `bound` (fit's `boundTo`).
fn bound_to(bound: &str, a: Option<IpAddr>) -> bool {
    let Some(a) = a else { return false };
    match addr_or_prefix(bound) {
        Some((p, bits)) if bound.contains('/') => in_prefix(a, p, bits),
        Some((p, _)) => p == a,
        None => false,
    }
}

/// The origin of a URL, as a `Referer`'s: what is before the first '/' after `://`.
fn origin_of(u: &str) -> &str {
    let Some(i) = u.find("://") else { return u };
    let after = i.saturating_add(3);
    match u.get(after..).and_then(|r| r.find('/')) {
        Some(j) => u.get(..after.saturating_add(j)).unwrap_or(u),
        None => u,
    }
}

fn match_origin(patterns: &[String], origin: &str) -> bool {
    if origin.len() > 256 {
        return false;
    }
    let lower = origin.to_ascii_lowercase();
    patterns.iter().any(|p| crate::auth::matches(p, &lower))
}

/// The viewer's address: the connection's peer, or, when that is a trusted proxy, the last
/// address of `X-Forwarded-For` that is not one (fit's `clientIP`).
pub(crate) fn client_ip(
    peer: Option<IpAddr>,
    xff: &[&str],
    trusted: &[(IpAddr, u8)],
) -> Option<IpAddr> {
    let canon = |a: IpAddr| match a {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    };
    let is_trusted = |a: IpAddr| trusted.iter().any(|(p, b)| in_prefix(a, *p, *b));
    let mut ip = peer.map(canon)?;
    if trusted.is_empty() || !is_trusted(ip) {
        return Some(ip);
    }
    for line in xff.iter().rev() {
        for item in line.rsplit(',') {
            let Ok(a) = item.trim().parse::<IpAddr>() else {
                return Some(ip);
            };
            let a = canon(a);
            if !is_trusted(a) {
                return Some(a);
            }
            ip = a;
        }
    }
    Some(ip)
}

/// What of a request a URL's check reads.
pub(crate) struct Seen<'a> {
    pub(crate) peer: Option<IpAddr>,
    pub(crate) xff: Vec<&'a str>,
    pub(crate) referer: Option<&'a str>,
    pub(crate) origin: Option<&'a str>,
}

/// Checks `g` for `resource` with `key` (the URL key of the key it names): its MAC, expiry,
/// address, `Referer` and `Origin` (fit's `state.verify`).
pub(crate) fn verify(
    c: &UrlConfig,
    key: &[u8],
    g: &Grant,
    resource: &str,
    seen: &Seen<'_>,
    now: i64,
) -> Result<(), UrlReason> {
    let want = g.mac(key, resource);
    if g.sig.len() != SIG_CHARS || !akio_crypto::ct_eq(want.as_bytes(), g.sig.as_bytes()) {
        return Err(UrlReason::Signature);
    }
    if now >= g.exp {
        return Err(UrlReason::Expired);
    }
    if !g.ip.is_empty() && !bound_to(&g.ip, client_ip(seen.peer, &seen.xff, &c.trusted)) {
        return Err(UrlReason::Address);
    }
    if !c.referers.is_empty() {
        if let Some(r) = seen.referer.filter(|r| !r.is_empty()) {
            if !match_origin(&c.referers, origin_of(r)) {
                return Err(UrlReason::Referer);
            }
        }
    }
    if !c.origins.is_empty() {
        if let Some(o) = seen.origin.filter(|o| !o.is_empty()) {
            if !match_origin(&c.origins, o) {
                return Err(UrlReason::Origin);
            }
        }
    }
    Ok(())
}

/// The child grant a playlist hands on (fit's `PassOn`): living `play_grace` past the later of
/// the URL's expiry and now plus `length`; a child is handed on unchanged.
pub(crate) fn child(
    c: &UrlConfig,
    key: &[u8],
    g: &Grant,
    resource: &str,
    length: Duration,
    now: i64,
) -> Grant {
    if g.child {
        return g.clone();
    }
    let mut out = g.clone();
    let length = i64::try_from(length.as_secs()).unwrap_or(i64::MAX);
    let grace = i64::try_from(c.grace.as_secs()).unwrap_or(i64::MAX);
    out.exp = g.exp.max(now.saturating_add(length)).saturating_add(grace);
    out.child = true;
    out.sig = out.mac(key, resource);
    out
}

/// The cookie of `g` on `resource`'s path (fit's `cookie`).
pub(crate) fn cookie(g: &Grant, resource: &str, tls: bool, now: i64) -> String {
    let age = g.exp.saturating_sub(now).max(1);
    let same = if tls {
        "Secure; SameSite=None"
    } else {
        "SameSite=Lax"
    };
    format!(
        "{GRANT_COOKIE}={}; Path={resource}; Max-Age={age}; HttpOnly; {same}",
        g.query()
    )
}

/// A grant for `resource` signed for a viewer (fit's `SignURL`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn sign(
    c: &UrlConfig,
    key: &[u8],
    kid: &str,
    resource: &str,
    subject: &str,
    address: Option<IpAddr>,
    session: &str,
    exp: i64,
) -> Grant {
    let mut g = Grant {
        exp,
        kid: kid.to_owned(),
        ip: address.map(|a| bind(a, c.subnet)).unwrap_or_default(),
        sid: session.to_owned(),
        sub: subject.to_owned(),
        child: false,
        sig: String::new(),
    };
    g.sig = g.mac(key, resource);
    g
}

// ---- sessions ----

const SHARDS: usize = 64;
/// How often, at most, a table at its bound is swept whole.
const SWEEP_EVERY_NANOS: u64 = 1_000_000_000;

#[derive(Debug, Clone)]
struct Session {
    sid: String,
    resource: String,
    address: String,
    start: i64,
    last: i64,
    /// The last of its grants' expiries, in seconds.
    keep: i64,
    ended: bool,
}

impl Session {
    fn over(&self, now: i64, idle: Duration) -> bool {
        if self.ended {
            return self.keep <= now / 1_000_000_000;
        }
        let idle = i64::try_from(idle.as_nanos()).unwrap_or(i64::MAX);
        self.last.saturating_add(idle) < now
    }
}

/// A playback session, as the Admin API lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionView {
    /// Its subject.
    pub subject: String,
    /// Its ID.
    pub id: String,
    /// Its resource.
    pub resource: String,
    /// The address it started from.
    pub address: String,
    /// When it started and last fetched, in nanoseconds since 1970.
    pub start: i64,
    /// See [`start`](Self::start).
    pub last: i64,
    /// Ended by `on_limit`.
    pub ended: bool,
}

/// A policy's playback sessions, kept across reloads (fit's `sessionTable`).
#[derive(Debug)]
pub(crate) struct Sessions {
    shards: Vec<Mutex<HashMap<String, Vec<Session>>>>,
    total: AtomicU64,
    swept: AtomicU64,
    pub(crate) refused: [AtomicU64; 7],
}

impl Default for Sessions {
    fn default() -> Sessions {
        Sessions {
            shards: (0..SHARDS).map(|_| Mutex::new(HashMap::new())).collect(),
            total: AtomicU64::new(0),
            swept: AtomicU64::new(0),
            refused: Default::default(),
        }
    }
}

fn shard_of(subject: &str) -> usize {
    let h = Sha256::digest(subject.as_bytes());
    usize::from(h.first().copied().unwrap_or(0)) % SHARDS
}

/// Why a session is not let in: a URL refusal, or the table full (503).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TouchError {
    Url(UrlReason),
    Full,
}

impl Sessions {
    fn shard(&self, subject: &str) -> std::sync::MutexGuard<'_, HashMap<String, Vec<Session>>> {
        // shard_of is below SHARDS, the number of shards.
        #[allow(clippy::indexing_slicing)]
        let sh = &self.shards[shard_of(subject)];
        sh.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn prune(&self, l: &mut Vec<Session>, now: i64, idle: Duration) {
        let n = l.len();
        l.retain(|s| !s.over(now, idle));
        let gone = n.saturating_sub(l.len()) as u64;
        if gone > 0 {
            // Each session is counted once, when added: the total holds these.
            self.total.fetch_sub(gone, Relaxed);
        }
    }

    /// Counts a refusal by its reason.
    pub(crate) fn count(&self, r: UrlReason) {
        if let Some(c) = self.refused.get(r.index()) {
            c.fetch_add(1, Relaxed);
        }
    }

    /// Records a request of session `sid` of `subject`, for `resource`, from `address`, with a
    /// grant good until `exp`, at `now` in nanoseconds (fit's `touch`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn touch(
        &self,
        c: &UrlConfig,
        subject: &str,
        sid: &str,
        resource: &str,
        address: &str,
        exp: i64,
        now: i64,
    ) -> Result<(), TouchError> {
        let mut sh = self.shard(subject);
        let mut have = false;
        if let Some(l) = sh.get_mut(subject) {
            self.prune(l, now, c.idle);
            if let Some(s) = l.iter_mut().find(|s| s.sid == sid) {
                s.keep = s.keep.max(exp);
                if s.ended {
                    return Err(TouchError::Url(UrlReason::SessionEnded));
                }
                s.last = now;
                return Ok(());
            }
            have = !l.is_empty();
        }
        if !have {
            // Every session that holds a subject is counted in the total, so that the subjects
            // too are bounded.
            sh.remove(subject);
        }
        if c.max_sessions > 0 {
            if let Some(l) = sh.get_mut(subject) {
                let playing = l.iter().filter(|s| !s.ended).count();
                if playing >= c.max_sessions {
                    if !c.end_oldest {
                        return Err(TouchError::Url(UrlReason::SessionLimit));
                    }
                    if let Some(s) = l.iter_mut().filter(|s| !s.ended).min_by_key(|s| s.start) {
                        s.ended = true;
                    }
                }
            }
        }
        if self.total.load(Relaxed) >= c.tracked as u64 {
            drop(sh);
            self.sweep(now, c.idle);
            sh = self.shard(subject);
            if self.total.load(Relaxed) >= c.tracked as u64 {
                return Err(TouchError::Full);
            }
        }
        sh.entry(subject.to_owned()).or_default().push(Session {
            sid: sid.to_owned(),
            resource: resource.to_owned(),
            address: address.to_owned(),
            start: now,
            last: now,
            keep: exp,
            ended: false,
        });
        self.total.fetch_add(1, Relaxed);
        Ok(())
    }

    /// Drops every session that is over, and every subject left with none, at most once a
    /// [`SWEEP_EVERY_NANOS`]. No shard's lock is held.
    fn sweep(&self, now: i64, idle: Duration) {
        let last = self.swept.load(Relaxed);
        let at = u64::try_from(now).unwrap_or(0);
        if last != 0 && at.saturating_sub(last) < SWEEP_EVERY_NANOS
            || self
                .swept
                .compare_exchange(last, at, Relaxed, Relaxed)
                .is_err()
        {
            return;
        }
        for sh in &self.shards {
            let mut m = sh.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            m.retain(|_, l| {
                self.prune(l, now, idle);
                !l.is_empty()
            });
        }
    }

    /// Records that session `sid` of `subject` was handed a grant good until `exp`.
    pub(crate) fn extend(&self, subject: &str, sid: &str, exp: i64) {
        let mut sh = self.shard(subject);
        if let Some(s) = sh
            .get_mut(subject)
            .and_then(|l| l.iter_mut().find(|s| s.sid == sid))
        {
            s.keep = s.keep.max(exp);
        }
    }

    /// Ends session `sid` of `subject`: whether it was playing.
    pub(crate) fn end(&self, subject: &str, sid: &str) -> bool {
        let mut sh = self.shard(subject);
        match sh
            .get_mut(subject)
            .and_then(|l| l.iter_mut().find(|s| s.sid == sid && !s.ended))
        {
            Some(s) => {
                s.ended = true;
                true
            }
            None => false,
        }
    }

    /// The number of sessions tracked.
    #[cfg(test)]
    pub(crate) fn tracked(&self) -> u64 {
        self.total.load(Relaxed)
    }

    /// The sessions that are not over at `n`, by subject, start and ID.
    pub(crate) fn list(&self, idle: Duration, n: i64) -> Vec<SessionView> {
        let mut out = Vec::new();
        for sh in &self.shards {
            let m = sh.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            for (subject, l) in m.iter() {
                for s in l.iter().filter(|s| !s.over(n, idle)) {
                    out.push(SessionView {
                        subject: subject.clone(),
                        id: s.sid.clone(),
                        resource: s.resource.clone(),
                        address: s.address.clone(),
                        start: s.start,
                        last: s.last,
                        ended: s.ended,
                    });
                }
            }
        }
        out.sort_by(|a, b| {
            a.subject
                .cmp(&b.subject)
                .then(a.start.cmp(&b.start))
                .then_with(|| a.id.cmp(&b.id))
        });
        out
    }
}

#[cfg(test)]
mod tests;
