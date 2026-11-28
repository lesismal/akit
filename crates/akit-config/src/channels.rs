//! The streaming role's channels (I145, fit's `config/channels.go`): items of a media library
//! played into the hub as live streams, in order, shuffled, looped or on a schedule. Their push
//! targets (I146) and their HLS encryption (I147) are refused as not supported yet.
//!
//! A schedule's times are read here (`at`), and the time zone's name checked for its form; the
//! zone itself is loaded, from the system's zoneinfo files, when the role applies the config,
//! which reports a zone it does not know (decision 043).

use std::time::Duration;

use crate::decode::{Decoder, Obj};
use crate::error::{join_field, join_index};
use crate::json::Node;
use crate::roles::{bool_opt, list, required, text, text_at};

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
];
// push: I146; encrypt and key_rotation: I147.
const CHANNEL_FIT_ONLY: &[&str] = &["push", "encrypt", "key_rotation"];
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

#[cfg(test)]
mod tests;
