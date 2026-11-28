//! The streaming role's channels (I145, fit's `config/channels.go`): items of a media library
//! played into the hub as live streams, in order, shuffled, looped or on a schedule; and its push
//! targets (I146), streams of the hub relayed to outside RTMP and SRT servers. A channel's HLS
//! encryption (I147) is refused as not supported yet, as is a push to `rtmps://` (akio API
//! request A297).
//!
//! A schedule's times are read here (`at`), and the time zone's name checked for its form; the
//! zone itself is loaded, from the system's zoneinfo files, when the role applies the config,
//! which reports a zone it does not know (decision 043).

use std::time::Duration;

use crate::decode::{Decoder, Obj};
use crate::error::{join_field, join_index};
use crate::json::Node;
use crate::roles::{bool_opt, duration, list, required, text, text_at};

/// fit's `DefaultChannelReadAhead`.
pub const DEFAULT_CHANNEL_READ_AHEAD: Duration = Duration::from_secs(3);
/// fit's `MinChannelReadAhead`.
pub const MIN_CHANNEL_READ_AHEAD: Duration = Duration::from_millis(500);
/// fit's `MaxChannelReadAhead`.
pub const MAX_CHANNEL_READ_AHEAD: Duration = Duration::from_secs(60);
/// fit's `DefaultChannelMaxBufferBytes`.
pub const DEFAULT_CHANNEL_MAX_BUFFER_BYTES: u64 = 16 << 20;
/// fit's `MinChannelMaxBufferBytes`.
pub const MIN_CHANNEL_MAX_BUFFER_BYTES: u64 = 1 << 20;
/// fit's `MaxChannelItems`.
pub const MAX_CHANNEL_ITEMS: usize = 10_000;
/// fit's `MaxChannelSlots`.
pub const MAX_CHANNEL_SLOTS: usize = 1000;
/// fit's `MaxChannels`.
pub const MAX_CHANNELS: usize = 256;
const MAX_CHANNEL_ITEM_LENGTH: usize = 256;

/// A channel of the streaming role (fit's `config.StreamChannel`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StreamChannel {
    /// `name`: the stream it publishes into the hub, as `tv/films`.
    pub name: String,
    /// `library`: the media library its items are of.
    pub library: String,
    /// `items`: the IDs it plays, in this order unless shuffled.
    pub items: Vec<String>,
    /// `order` is `shuffle`.
    pub shuffle: bool,
    /// `loop`: on unless `false`.
    pub looped: bool,
    /// `schedule`: items started at set times.
    pub schedule: Vec<ChannelSlot>,
    /// `time_zone`, as written: empty for UTC.
    pub time_zone: String,
    /// `rendition`: the rendition played of each item, by name; the best when empty.
    pub rendition: String,
    /// `read_ahead`: [`DEFAULT_CHANNEL_READ_AHEAD`] when zero.
    pub read_ahead: Duration,
    /// `max_buffer_bytes`: [`DEFAULT_CHANNEL_MAX_BUFFER_BYTES`] when 0.
    pub max_buffer_bytes: u64,
    /// `push` (I146): outside servers the channel is relayed to.
    pub push: Vec<StreamPush>,
}

impl StreamChannel {
    /// `read_ahead`, or its default.
    #[must_use]
    pub fn read_ahead_or(&self) -> Duration {
        if self.read_ahead.is_zero() {
            DEFAULT_CHANNEL_READ_AHEAD
        } else {
            self.read_ahead
        }
    }

    /// `max_buffer_bytes`, or its default.
    #[must_use]
    pub fn buffer_bytes(&self) -> u64 {
        if self.max_buffer_bytes == 0 {
            DEFAULT_CHANNEL_MAX_BUFFER_BYTES
        } else {
            self.max_buffer_bytes
        }
    }

    /// The name of `time_zone`'s zone: `UTC` when left out.
    #[must_use]
    pub fn zone(&self) -> &str {
        if self.time_zone.is_empty() {
            "UTC"
        } else {
            &self.time_zone
        }
    }
}

/// A slot of a channel's schedule (fit's `config.ChannelSlot`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChannelSlot {
    /// `at`, as written.
    pub at_text: String,
    /// `at`, read.
    pub at: ChannelAt,
    /// `item`: the item it plays from its start.
    pub item: String,
}

/// A schedule slot's `at`, read (fit's `config.ChannelAt`): a time of day, every day or on some
/// days of the week, or a date and time, once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChannelAt {
    /// Bit `d` set for each day of the week `d` (0 Sunday) the slot is on; all seven for every
    /// day, none for a date.
    pub days: u8,
    /// The time of day, in seconds from midnight.
    pub clock: u32,
    /// A date's year, month and day; `None` for a time of day.
    pub date: Option<(i64, u32, u32)>,
}

impl ChannelAt {
    /// Whether the slot is on a date, once.
    #[must_use]
    pub fn once(&self) -> bool {
        self.date.is_some()
    }
}

const WEEKDAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

/// Digits of `s`, exactly `n` of them, or 1 to `n` without `fixed`.
fn digits(s: &str, n: usize, fixed: bool) -> Option<u32> {
    let ok = if fixed {
        s.len() == n
    } else {
        (1..=n).contains(&s.len())
    };
    (ok && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

/// `HH:MM` or `HH:MM:SS`, as Go's layouts `15:04` and `15:04:05` take it.
fn clock_of(s: &str) -> Option<u32> {
    let mut parts = s.split(':');
    let h = digits(parts.next()?, 2, false)?;
    let m = digits(parts.next()?, 2, true)?;
    let sec = match parts.next() {
        Some(p) => digits(p, 2, true)?,
        None => 0,
    };
    (parts.next().is_none() && h < 24 && m < 60 && sec < 60).then(|| {
        h.saturating_mul(3600)
            .saturating_add(m.saturating_mul(60))
            .saturating_add(sec)
    })
}

/// Whether `d` is a day of month `m` of year `y`.
fn valid_date(y: i64, m: u32, d: u32) -> bool {
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&d)
}

/// A date and time, `2006-01-02 15:04[:05]` or with `T`.
fn date_of(s: &str) -> Option<ChannelAt> {
    let (date, time) = s.split_once(' ').or_else(|| s.split_once('T'))?;
    let mut p = date.split('-');
    let y = i64::from(digits(p.next()?, 4, true)?);
    let m = digits(p.next()?, 2, true)?;
    let d = digits(p.next()?, 2, true)?;
    if p.next().is_some() || !valid_date(y, m, d) {
        return None;
    }
    let clock =
        clock_of(time).filter(|_| time.len() >= 5 && time.as_bytes().get(2) == Some(&b':'))?;
    Some(ChannelAt {
        days: 0,
        clock,
        date: Some((y, m, d)),
    })
}

/// Reads a schedule slot's `at` (fit's `ParseChannelAt`).
///
/// # Errors
///
/// What is wrong with it, as fit says.
pub fn parse_channel_at(s: &str) -> Result<ChannelAt, String> {
    let s = s.trim();
    if let Some(at) = date_of(s) {
        return Ok(at);
    }
    let mut at = ChannelAt {
        days: 0x7f,
        ..ChannelAt::default()
    };
    let mut clock = s;
    if let Some((days, rest)) = s.split_once(' ') {
        clock = rest.trim();
        at.days = 0;
        for part in days.to_ascii_lowercase().split(',') {
            let (from, to) = match part.split_once('-') {
                Some((a, b)) => (a, Some(b)),
                None => (part, None),
            };
            let day = |n: &str| WEEKDAYS.iter().position(|w| *w == n);
            let (Some(a), b) = (day(from), to.map(day)) else {
                return Err(format!(
                    "{part:?} is not a day of the week: mon, tue, wed, thu, fri, sat, sun, or a range of them as mon-fri"
                ));
            };
            let b = match b {
                None => a,
                Some(Some(b)) => b,
                Some(None) => {
                    return Err(format!(
                        "{part:?} is not a day of the week: mon, tue, wed, thu, fri, sat, sun, or a range of them as mon-fri"
                    ));
                }
            };
            let mut d = a;
            loop {
                at.days |= 1 << d;
                if d == b {
                    break;
                }
                d = d.saturating_add(1) % 7;
            }
        }
    }
    match clock_of(clock) {
        Some(c) => {
            at.clock = c;
            Ok(at)
        }
        None => Err(format!(
            "{s:?} is not a time: HH:MM or HH:MM:SS, days of the week before it, or a date as 2026-12-31 23:00"
        )),
    }
}

/// Whether `p` is a channel's name (fit's `ValidStreamPath`): parts of letters, digits, '.',
/// '_', '~' and '-', not starting with '.', split at '/', 256 bytes at most.
#[must_use]
pub fn valid_stream_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 256
        && p.split('/').all(|part| {
            (1..=128).contains(&part.len())
                && !part.starts_with('.')
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'))
        })
}

/// Whether `tz` has the form of a time zone's name: `UTC`, `Local`, or an IANA name.
#[must_use]
pub fn zone_name_form(tz: &str) -> bool {
    !tz.is_empty()
        && tz.len() <= 64
        && !tz.starts_with('/')
        && !tz.split('/').any(|p| p.is_empty() || p == "." || p == "..")
        && tz
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'+' | b'-'))
}

/// What is wrong with a channel's item ID (fit's `checkItemID`); `None` when nothing.
fn item_problem(id: &str) -> Option<String> {
    if id.is_empty() {
        return Some("an item's ID may not be empty".into());
    }
    if id.len() > MAX_CHANNEL_ITEM_LENGTH {
        return Some(format!(
            "an item's ID is {} bytes: at most {MAX_CHANNEL_ITEM_LENGTH}",
            id.len()
        ));
    }
    if id.starts_with('/') || id.chars().any(|c| c <= ' ' || c == '\u{7f}') {
        return Some(format!(
            "{id:?} is not an item's ID: the library's IDs are paths without spaces, not starting with '/'"
        ));
    }
    None
}

const CHANNEL_FIELDS: &[&str] = &[
    "name",
    "library",
    "items",
    "order",
    "loop",
    "schedule",
    "time_zone",
    "rendition",
    "read_ahead",
    "max_buffer_bytes",
    // I146.
    "push",
];
// I147.
const CHANNEL_FIT_ONLY: &[&str] = &["encrypt", "key_rotation"];
const SLOT_FIELDS: &[&str] = &["at", "item"];

/// A channel's library, for the check once every file is read: its name, where it is written,
/// and its path.
#[derive(Debug, Clone)]
pub(crate) struct LibraryRef {
    pub(crate) name: String,
    pub(crate) off: usize,
    pub(crate) path: String,
}

/// Decodes `streaming.channels`, with fit's checks (`checkChannels`) but for the libraries'
/// names, which [`LibraryRef`]s carry to the check once every file is read.
pub(crate) fn channels(
    d: &mut Decoder<'_>,
    o: &mut Obj<'_>,
    libs: &mut Vec<LibraryRef>,
) -> Vec<StreamChannel> {
    let got = list(d, o, "channels", |d, n, p| channel(d, n, p, libs));
    if got.len() > MAX_CHANNELS {
        d.error(
            o.off,
            &o.at("channels"),
            format!("{} channels: at most {MAX_CHANNELS}", got.len()),
        );
    }
    let mut out: Vec<StreamChannel> = Vec::new();
    for (c, off, p) in got {
        if valid_stream_path(&c.name) && out.iter().any(|o| o.name == c.name) {
            d.error(
                off,
                &join_field(&p, "name"),
                format!("another channel is already named {:?}", c.name),
            );
        }
        out.push(c);
    }
    out
}

fn channel(
    d: &mut Decoder<'_>,
    n: &Node,
    path: &str,
    libs: &mut Vec<LibraryRef>,
) -> Option<StreamChannel> {
    let mut o = d.object(n, path)?;
    let (name, noff) = required(d, &mut o, "name");
    if noff != o.off && !valid_stream_path(&name) {
        d.error(
            noff,
            &o.at("name"),
            format!(
                "{name:?} is not a stream's name: parts of letters, digits, '.', '_', '~' and '-', split at '/'"
            ),
        );
    }
    let (library, loff) = required(d, &mut o, "library");
    if loff != o.off && !library.is_empty() {
        libs.push(LibraryRef {
            name: library.clone(),
            off: loff,
            path: o.at("library"),
        });
    }
    let mut c = StreamChannel {
        name,
        library,
        looped: true,
        ..StreamChannel::default()
    };
    if let Some(v) = o.take("items") {
        let p = o.at("items");
        let items = d.strings(v, &p);
        if items.len() > MAX_CHANNEL_ITEMS {
            d.error(
                v.off,
                &p,
                format!("{} items: at most {MAX_CHANNEL_ITEMS}", items.len()),
            );
        }
        for (i, (off, id)) in items.into_iter().enumerate() {
            if let Some(msg) = item_problem(&id) {
                d.error(off, &join_index(&p, i), msg);
            }
            c.items.push(id);
        }
    }
    if let Some((v, off)) = text_at(d, &mut o, "order") {
        match v.as_str() {
            "" | "sequence" => {}
            "shuffle" => c.shuffle = true,
            _ => d.error(
                off,
                &o.at("order"),
                format!("{v:?} is not one of \"sequence\", \"shuffle\""),
            ),
        }
    }
    if let Some(l) = bool_opt(d, &mut o, "loop") {
        c.looped = l;
    }
    let slots = list(d, &mut o, "schedule", slot);
    if slots.len() > MAX_CHANNEL_SLOTS {
        d.error(
            o.off,
            &o.at("schedule"),
            format!("{} slots: at most {MAX_CHANNEL_SLOTS}", slots.len()),
        );
    }
    c.schedule = slots.into_iter().map(|(s, _, _)| s).collect();
    if let Some((tz, off)) = text_at(d, &mut o, "time_zone") {
        if !tz.is_empty() && !zone_name_form(&tz) {
            d.error(
                off,
                &o.at("time_zone"),
                format!("{tz:?} is not a time zone: an IANA name, as Asia/Shanghai, or Local"),
            );
        }
        c.time_zone = tz;
    }
    c.rendition = text(d, &mut o, "rendition");
    if let Some(v) = o.take("read_ahead") {
        let off = v.off;
        if let Some(dur) = d.duration(v, &o.at("read_ahead")) {
            if !dur.is_zero() && !(MIN_CHANNEL_READ_AHEAD..=MAX_CHANNEL_READ_AHEAD).contains(&dur) {
                d.error(
                    off,
                    &o.at("read_ahead"),
                    format!(
                        "read_ahead is {}: 500ms to 1m0s",
                        crate::duration::format(dur)
                    ),
                );
            }
            c.read_ahead = dur;
        }
    }
    if let Some(v) = o.take("max_buffer_bytes") {
        let off = v.off;
        if let Some(b) = d
            .int(v, &o.at("max_buffer_bytes"), 0, i64::MAX)
            .and_then(|b| u64::try_from(b).ok())
        {
            if b != 0 && b < MIN_CHANNEL_MAX_BUFFER_BYTES {
                d.error(
                    off,
                    &o.at("max_buffer_bytes"),
                    format!("max_buffer_bytes is {b}: at least {MIN_CHANNEL_MAX_BUFFER_BYTES}"),
                );
            }
            c.max_buffer_bytes = b;
        }
    }
    let name = c.name.clone();
    for (mut p, off, path) in list(d, &mut o, "push", push) {
        if !p.stream.is_empty() && p.stream != name {
            d.error(
                off,
                &join_field(&path, "stream"),
                "a channel's push relays the channel: leave stream out",
            );
        }
        p.stream.clone_from(&name);
        p.at = Some((off, path));
        c.push.push(p);
    }
    if c.items.is_empty() && c.schedule.is_empty() {
        d.error(
            n.off,
            path,
            "a channel plays items, a schedule or both: neither is set",
        );
    }
    o.finish(d, CHANNEL_FIELDS, CHANNEL_FIT_ONLY);
    Some(c)
}

fn slot(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<ChannelSlot> {
    let mut o = d.object(n, path)?;
    let (at_text, aoff) = required(d, &mut o, "at");
    let mut at = ChannelAt::default();
    if aoff != o.off {
        match parse_channel_at(&at_text) {
            Ok(a) => at = a,
            Err(e) => d.error(aoff, &o.at("at"), e),
        }
    }
    let (item, ioff) = required(d, &mut o, "item");
    if ioff != o.off {
        if let Some(msg) = item_problem(&item) {
            d.error(ioff, &o.at("item"), msg);
        }
    }
    o.finish(d, SLOT_FIELDS, &[]);
    Some(ChannelSlot { at_text, at, item })
}

// ---- push targets (I146) ----

/// fit's `MaxStreamPushTargets`.
pub const MAX_STREAM_PUSH_TARGETS: usize = 256;
/// fit's `DefaultPushRetryMin`.
pub const DEFAULT_PUSH_RETRY_MIN: Duration = Duration::from_secs(1);
/// fit's `DefaultPushRetryMax`.
pub const DEFAULT_PUSH_RETRY_MAX: Duration = Duration::from_secs(30);
/// fit's `DefaultPushConnectTimeout`.
pub const DEFAULT_PUSH_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// fit's `DefaultPushSRTLatency`.
pub const DEFAULT_PUSH_SRT_LATENCY: Duration = Duration::from_millis(120);

/// An outside server a stream of the hub is relayed to (fit's `config.StreamPush`).
#[derive(Clone, Eq, Default)]
pub struct StreamPush {
    /// `name`: unique among the push targets; the stream's name and its place among the
    /// stream's targets when empty ([`push_targets`] fills it in).
    pub name: String,
    /// `stream`: the stream relayed; a channel's push relays the channel.
    pub stream: String,
    /// `url`: a secret, its stream key with it.
    pub url: String,
    /// `retry_min`: [`DEFAULT_PUSH_RETRY_MIN`] when zero.
    pub retry_min: Duration,
    /// `retry_max`: [`DEFAULT_PUSH_RETRY_MAX`] when zero.
    pub retry_max: Duration,
    /// `connect_timeout`: [`DEFAULT_PUSH_CONNECT_TIMEOUT`] when zero.
    pub connect_timeout: Duration,
    /// Where it is written, for the checks of the targets together.
    #[doc(hidden)]
    pub at: Option<(usize, String)>,
}

impl std::fmt::Debug for StreamPush {
    /// Without the URL, a secret: its scheme and host alone.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let url = parse_push_url(&self.url).map_or_else(|_| "…".to_owned(), |u| u.redacted());
        f.debug_struct("StreamPush")
            .field("name", &self.name)
            .field("stream", &self.stream)
            .field("url", &url)
            .finish_non_exhaustive()
    }
}

impl PartialEq for StreamPush {
    /// The same target, wherever it is written.
    fn eq(&self, o: &StreamPush) -> bool {
        self.name == o.name
            && self.stream == o.stream
            && self.url == o.url
            && self.retry_min == o.retry_min
            && self.retry_max == o.retry_max
            && self.connect_timeout == o.connect_timeout
    }
}

impl StreamPush {
    /// `retry_min`, or its default.
    #[must_use]
    pub fn retry_min_or(&self) -> Duration {
        if self.retry_min.is_zero() {
            DEFAULT_PUSH_RETRY_MIN
        } else {
            self.retry_min
        }
    }

    /// `retry_max`, or its default; never less than `retry_min`.
    #[must_use]
    pub fn retry_max_or(&self) -> Duration {
        let m = if self.retry_max.is_zero() {
            DEFAULT_PUSH_RETRY_MAX
        } else {
            self.retry_max
        };
        m.max(self.retry_min_or())
    }

    /// `connect_timeout`, or its default.
    #[must_use]
    pub fn connect(&self) -> Duration {
        if self.connect_timeout.is_zero() {
            DEFAULT_PUSH_CONNECT_TIMEOUT
        } else {
            self.connect_timeout
        }
    }
}

/// A push target as the role runs it (fit's `config.PushTarget`): from `streaming.push` or a
/// channel's, its name and stream filled in.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PushTarget {
    /// The target.
    pub push: StreamPush,
    /// The channel whose push it is; empty for `streaming.push`'s.
    pub channel: String,
}

/// Every push target of `push` (`streaming.push`) and `channels`, `push`'s first, then each
/// channel's, with its name and stream filled in: a target without a name is named after its
/// stream and its place among the stream's targets, from 1, as `tv/films#1` (fit's
/// `PushTargets`).
#[must_use]
pub fn push_targets(push: &[StreamPush], channels: &[StreamChannel]) -> Vec<PushTarget> {
    let mut out = Vec::new();
    let mut n: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut add = |mut p: StreamPush, channel: &str| {
        if !channel.is_empty() {
            channel.clone_into(&mut p.stream);
        }
        let k = n.entry(p.stream.clone()).or_insert(0);
        *k = k.saturating_add(1);
        if p.name.is_empty() {
            p.name = format!("{}#{k}", p.stream);
        }
        out.push(PushTarget {
            push: p,
            channel: channel.to_owned(),
        });
    };
    for p in push {
        add(p.clone(), "");
    }
    for c in channels {
        for p in &c.push {
            add(p.clone(), &c.name);
        }
    }
    out
}

/// A push target's URL taken apart (fit's `config.PushURL`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PushUrl {
    /// `rtmp` or `srt`.
    pub protocol: String,
    /// `host:port`, the default port filled in for RTMP.
    pub host: String,
    /// An RTMP URL's first path element.
    pub app: String,
    /// SRT's stream ID.
    pub stream_id: String,
    /// SRT's passphrase.
    pub passphrase: String,
    /// SRT's key length: 0, 16, 24 or 32.
    pub pbkeylen: u32,
    /// SRT's latency.
    pub latency: Duration,
}

impl PushUrl {
    /// The URL as the Admin API and the logs show it: its scheme, host and, for RTMP, app;
    /// never the stream key, nor SRT's stream ID and passphrase.
    #[must_use]
    pub fn redacted(&self) -> String {
        if self.app.is_empty() {
            format!("{}://{}", self.protocol, self.host)
        } else {
            format!("{}://{}/{}/…", self.protocol, self.host, self.app)
        }
    }
}

/// `s` with its %XX escapes decoded; `None` for a bad escape.
fn unescape(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0usize;
    while let Some(&c) = b.get(i) {
        if c == b'%' {
            let hex = s.get(i.saturating_add(1)..i.saturating_add(3))?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i = i.saturating_add(3);
        } else {
            out.push(if c == b'+' { b' ' } else { c });
            i = i.saturating_add(1);
        }
    }
    String::from_utf8(out).ok()
}

/// Takes a push target's `url` apart, or says what is wrong with it (fit's `ParsePushURL`); the
/// error never holds the URL, which is a secret.
///
/// # Errors
///
/// What is wrong with it.
pub fn parse_push_url(raw: &str) -> Result<PushUrl, String> {
    let bad = || "the url does not parse".to_owned();
    if raw.bytes().any(|b| b <= b' ' || b == 0x7f) || (raw.contains('%') && unescape(raw).is_none())
    {
        return Err(bad());
    }
    let Some((scheme, rest)) = raw.split_once("://") else {
        return Err("a push url is rtmp://, rtmps:// or srt://".into());
    };
    let protocol = scheme.to_ascii_lowercase();
    let (authority, rest) = match rest.find(['/', '?', '#']) {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let rest = rest.split('#').next().unwrap_or("");
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (host, port) = if let Some(r) = authority.strip_prefix('[') {
        let (h, p) = r.split_once(']').ok_or_else(bad)?;
        (h, p.strip_prefix(':').unwrap_or(""))
    } else {
        authority.rsplit_once(':').unwrap_or((authority, ""))
    };
    if !port.is_empty() && port.parse::<u16>().is_err() {
        return Err(bad());
    }
    let hostport = |p: &str| {
        if host.contains(':') {
            format!("[{host}]:{p}")
        } else {
            format!("{host}:{p}")
        }
    };
    let mut u = PushUrl {
        protocol: protocol.clone(),
        ..PushUrl::default()
    };
    match protocol.as_str() {
        "rtmp" | "rtmps" => {
            if host.is_empty() {
                return Err(format!("an {protocol} url needs a host"));
            }
            let p = path.trim_start_matches('/');
            let (app, stream) = p.split_once('/').unwrap_or((p, ""));
            if app.is_empty() || stream.is_empty() {
                return Err(format!(
                    "an {protocol} url names an app and a stream: {protocol}://host/app/stream"
                ));
            }
            if protocol == "rtmps" {
                return Err("rtmps:// is not supported yet: akio's RTMP client dials without TLS (akio API request A297)".into());
            }
            u.host = hostport(if port.is_empty() { "1935" } else { port });
            app.clone_into(&mut u.app);
        }
        "srt" => {
            if host.is_empty() || port.is_empty() {
                return Err("an srt url needs a host and a port, as srt://host:9000".into());
            }
            if !path.is_empty() && path != "/" {
                return Err("an srt url has no path: the stream goes in streamid".into());
            }
            u.host = hostport(port);
            u.latency = DEFAULT_PUSH_SRT_LATENCY;
            for kv in query.split('&').filter(|kv| !kv.is_empty()) {
                let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                let k = unescape(k).ok_or_else(bad)?;
                let v = unescape(v).ok_or_else(bad)?;
                match k.as_str() {
                    "streamid" => u.stream_id = v,
                    "passphrase" => u.passphrase = v,
                    "pbkeylen" => match v.parse::<u32>() {
                        Ok(n @ (16 | 24 | 32)) => u.pbkeylen = n,
                        _ => return Err("pbkeylen is 16, 24 or 32".into()),
                    },
                    "latency" => match v.parse::<u64>() {
                        Ok(ms) if (20..=10_000).contains(&ms) => {
                            u.latency = Duration::from_millis(ms);
                        }
                        _ => return Err("latency is in milliseconds, 20 to 10000".into()),
                    },
                    "mode" => {
                        if !v.is_empty() && v != "caller" {
                            return Err(
                                "an srt push calls its target: mode may only be caller".into()
                            );
                        }
                    }
                    other => {
                        return Err(format!(
                            "an srt url takes streamid, passphrase, pbkeylen, latency and mode=caller, not {other:?}"
                        ));
                    }
                }
            }
            let n = u.passphrase.chars().count();
            if n != 0 && !(10..=79).contains(&n) {
                return Err(format!(
                    "the passphrase is {n} characters: SRT takes 10 to 79"
                ));
            }
        }
        _ => return Err("a push url is rtmp://, rtmps:// or srt://".into()),
    }
    Ok(u)
}

const PUSH_FIELDS: &[&str] = &[
    "name",
    "stream",
    "url",
    "retry_min",
    "retry_max",
    "connect_timeout",
];

fn push(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<StreamPush> {
    let mut o = d.object(n, path)?;
    let p = StreamPush {
        name: text(d, &mut o, "name"),
        stream: text(d, &mut o, "stream"),
        url: required(d, &mut o, "url").0,
        retry_min: duration(d, &mut o, "retry_min"),
        retry_max: duration(d, &mut o, "retry_max"),
        connect_timeout: duration(d, &mut o, "connect_timeout"),
        at: None,
    };
    o.finish(d, PUSH_FIELDS, &[]);
    Some(p)
}

/// Decodes `streaming.push`.
pub(crate) fn pushes(d: &mut Decoder<'_>, o: &mut Obj<'_>) -> Vec<StreamPush> {
    list(d, o, "push", push)
        .into_iter()
        .map(|(mut p, off, path)| {
            p.at = Some((off, path));
            p
        })
        .collect()
}

/// fit's checks of the push targets together: their number, streams, names, URLs and retries.
pub(crate) fn check_pushes(
    d: &mut Decoder<'_>,
    at: (usize, &str),
    push: &[StreamPush],
    channels: &[StreamChannel],
) {
    let targets = push_targets(push, channels);
    if targets.len() > MAX_STREAM_PUSH_TARGETS {
        d.error(
            at.0,
            &join_field(at.1, "push"),
            format!(
                "{} push targets: at most {MAX_STREAM_PUSH_TARGETS}",
                targets.len()
            ),
        );
    }
    for (i, t) in targets.iter().enumerate() {
        let p = &t.push;
        let (off, path) = p.at.clone().unwrap_or((at.0, at.1.to_owned()));
        if t.channel.is_empty() {
            if p.stream.is_empty() {
                d.error(off, &path, "a push target names the stream it relays");
            } else if !valid_stream_path(&p.stream) {
                d.error(
                    off,
                    &join_field(&path, "stream"),
                    format!("{:?} is not a stream's name", p.stream),
                );
            }
        }
        if targets.iter().take(i).any(|o| o.push.name == p.name) {
            d.error(
                off,
                &join_field(&path, "name"),
                format!("another push target is already named {:?}", p.name),
            );
        }
        if let Err(e) = parse_push_url(&p.url) {
            d.error(off, &join_field(&path, "url"), e);
        }
        if !p.retry_min.is_zero() && !p.retry_max.is_zero() && p.retry_min > p.retry_max {
            d.error(
                off,
                &join_field(&path, "retry_min"),
                format!(
                    "retry_min is {}, more than retry_max's {}",
                    crate::duration::format(p.retry_min),
                    crate::duration::format(p.retry_max)
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests;
