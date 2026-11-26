//! Time zones, for the times access log lines are written in: UTC, the local zone, or an IANA
//! zone, read from the system's zoneinfo files (TZif, RFC 8536) with the POSIX `TZ` rule of their
//! footer for the times past their last transition, as Go's `time.LoadLocation` reads them.
//!
//! A zone is loaded once, on the control plane (it reads files); finding the offset of a time is
//! a binary search, or the rule's arithmetic, and never touches the disk.
//!
//! Times are clamped to [`crate::time::MAX_SECS`] of the epoch, offsets and a rule's times are
//! bounded by its parser (a week of seconds at most), and counts read from a file by its size:
//! the arithmetic below cannot overflow.
#![allow(clippy::arithmetic_side_effects)]

use std::fmt;
use std::path::Path;

/// A time zone: the offset from UTC of every instant.
#[derive(Clone, PartialEq, Eq)]
pub struct Zone {
    name: String,
    /// Transition times, in seconds since the epoch, ascending.
    times: Vec<i64>,
    /// The type in effect from each transition on, an index of `types`.
    idx: Vec<u8>,
    /// Each type's offset, in seconds east of UTC, and whether it is daylight time.
    types: Vec<(i32, bool)>,
    /// The rule for the times past the last transition (or for every time, with none).
    rule: Option<Rule>,
}

impl fmt::Debug for Zone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Zone")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Why a zone could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneError(String);

impl fmt::Display for ZoneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} is not a time zone: local, utc or an IANA name such as Asia/Shanghai",
            self.0
        )
    }
}

impl std::error::Error for ZoneError {}

/// The directories zoneinfo files are looked for in, as Go's `platformZoneSources`.
const ZONE_DIRS: &[&str] = &[
    "/usr/share/zoneinfo/",
    "/usr/share/lib/zoneinfo/",
    "/usr/lib/locale/TZ/",
    "/etc/zoneinfo/",
];

/// The largest zoneinfo file read: real ones are a few kilobytes.
const MAX_TZIF_BYTES: u64 = 256 << 10;

impl Zone {
    /// UTC.
    #[must_use]
    pub fn utc() -> Zone {
        Zone {
            name: "UTC".into(),
            times: Vec::new(),
            idx: Vec::new(),
            types: vec![(0, false)],
            rule: None,
        }
    }

    /// The zone a config's `time_zone` names, as fit's `config.LoadTimeZone`: `local` or empty
    /// for the local zone, `utc` for UTC (either in any case, spaces around ignored), or an IANA
    /// name such as `Asia/Shanghai`. Reads files: call it on the control plane.
    ///
    /// # Errors
    ///
    /// When the name is none of those, or its zoneinfo file is not found or not valid.
    pub fn load(name: &str) -> Result<Zone, ZoneError> {
        let trimmed = name.trim();
        match trimmed.to_ascii_lowercase().as_str() {
            "" | "local" => return Ok(Zone::local()),
            "utc" => return Ok(Zone::utc()),
            _ => {}
        }
        Zone::named(trimmed).ok_or_else(|| ZoneError(name.to_owned()))
    }

    /// The local zone, as Go's `time.Local`: the zone `TZ` names (a zoneinfo name, a path after
    /// `:` or `/`, or a POSIX rule; UTC when set and empty), or `/etc/localtime`'s; UTC when
    /// none can be read.
    #[must_use]
    pub fn local() -> Zone {
        let zone = match std::env::var("TZ") {
            Ok(tz) if tz.is_empty() => Some(Zone::utc()),
            Ok(tz) => {
                let tz = tz.strip_prefix(':').unwrap_or(&tz);
                if tz.starts_with('/') {
                    read_tzif(Path::new(tz), tz)
                } else if tz == "UTC" {
                    Some(Zone::utc())
                } else {
                    Zone::named(tz).or_else(|| {
                        Rule::parse(tz).map(|rule| Zone {
                            name: tz.to_owned(),
                            times: Vec::new(),
                            idx: Vec::new(),
                            types: vec![(rule.std, false)],
                            rule: Some(rule),
                        })
                    })
                }
            }
            Err(_) => read_tzif(Path::new("/etc/localtime"), "Local"),
        };
        let mut zone = zone.unwrap_or_else(Zone::utc);
        zone.name = "Local".into();
        zone
    }

    /// The zone of the IANA name `name`, from the first zoneinfo directory that has it.
    fn named(name: &str) -> Option<Zone> {
        if name == "UTC" {
            return Some(Zone::utc());
        }
        // A name, never a path out of the directory.
        let ok = name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'+' | b'.'))
            && !name.starts_with('/')
            && !name
                .split('/')
                .any(|s| s.is_empty() || s == "." || s == "..");
        if !ok {
            return None;
        }
        ZONE_DIRS
            .iter()
            .find_map(|dir| read_tzif(&Path::new(dir).join(name), name))
    }

    /// A zone `offset` seconds east of UTC at every instant, named `name`, as Go's
    /// `time.FixedZone`: for tests and fixed offsets.
    #[must_use]
    pub fn fixed(name: &str, offset: i32) -> Zone {
        Zone {
            name: name.to_owned(),
            times: Vec::new(),
            idx: Vec::new(),
            types: vec![(offset, false)],
            rule: None,
        }
    }

    /// The zone of a TZif file's bytes, named `name`.
    #[must_use]
    pub fn from_tzif(name: &str, data: &[u8]) -> Option<Zone> {
        parse_tzif(name, data)
    }

    /// The zone's name: `UTC`, `Local`, or the IANA name it was loaded by.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether the zone is UTC at every instant.
    #[must_use]
    pub fn is_utc(&self) -> bool {
        self.rule.is_none() && self.types.iter().all(|t| t.0 == 0)
    }

    /// The offset from UTC, in seconds east, at `t` seconds since the epoch.
    #[must_use]
    pub fn offset(&self, t: i64) -> i32 {
        let t = crate::time::clamp(t);
        let Some(&last) = self.times.last() else {
            return match &self.rule {
                Some(r) => r.offset(t),
                None => self.first_std(),
            };
        };
        if t >= last {
            if let Some(r) = &self.rule {
                return r.offset(t);
            }
        }
        // The index of the last transition at or before t.
        let n = self.times.partition_point(|&x| x <= t);
        if n == 0 {
            return self.first_std();
        }
        self.idx
            .get(n - 1)
            .and_then(|&i| self.types.get(usize::from(i)))
            .map_or(0, |t| t.0)
    }

    /// The offset of the times before the first transition: the first standard time type, as
    /// Go's `lookupFirstZone` (roughly).
    fn first_std(&self) -> i32 {
        self.types
            .iter()
            .find(|t| !t.1)
            .or(self.types.first())
            .map_or(0, |t| t.0)
    }
}

fn read_tzif(path: &Path, name: &str) -> Option<Zone> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    let mut data = Vec::new();
    f.take(MAX_TZIF_BYTES + 1).read_to_end(&mut data).ok()?;
    if data.len() as u64 > MAX_TZIF_BYTES {
        return None;
    }
    parse_tzif(name, &data)
}

/// A cursor over a TZif file's bytes.
struct Bytes<'a>(&'a [u8]);

impl<'a> Bytes<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (a, b) = self.0.split_at_checked(n)?;
        self.0 = b;
        Some(a)
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }

    fn u32(&mut self) -> Option<u32> {
        self.array().map(u32::from_be_bytes)
    }

    fn count(&mut self) -> Option<usize> {
        let n = usize::try_from(self.u32()?).ok()?;
        // Real files have a few hundred of each; this bounds a corrupt one.
        (n <= 1 << 16).then_some(n)
    }
}

/// The counts of a TZif header.
struct Header {
    isut: usize,
    isstd: usize,
    leap: usize,
    time: usize,
    typ: usize,
    chars: usize,
}

fn header(b: &mut Bytes<'_>) -> Option<(u8, Header)> {
    if b.take(4)? != b"TZif" {
        return None;
    }
    let version = *b.take(1)?.first()?;
    b.take(15)?;
    Some((
        version,
        Header {
            isut: b.count()?,
            isstd: b.count()?,
            leap: b.count()?,
            time: b.count()?,
            typ: b.count()?,
            chars: b.count()?,
        },
    ))
}

fn parse_tzif(name: &str, data: &[u8]) -> Option<Zone> {
    let mut b = Bytes(data);
    let (version, mut h) = header(&mut b)?;
    let mut wide = false;
    if version >= b'2' {
        // Skip the version 1 data for the 64-bit data after it.
        let v1 = h.time * 5 + h.typ * 6 + h.chars + h.leap * 8 + h.isstd + h.isut;
        b.take(v1)?;
        h = header(&mut b)?.1;
        wide = true;
    }
    if h.typ == 0 {
        return None;
    }
    let mut times = Vec::with_capacity(h.time);
    for _ in 0..h.time {
        let t = if wide {
            i64::from_be_bytes(b.array()?)
        } else {
            i64::from(i32::from_be_bytes(b.array()?))
        };
        times.push(t);
    }
    let idx = b.take(h.time)?.to_vec();
    let mut types = Vec::with_capacity(h.typ);
    for _ in 0..h.typ {
        let off = i32::from_be_bytes(b.array()?);
        let [isdst, _] = b.array::<2>()?;
        types.push((off, isdst != 0));
    }
    if idx.iter().any(|&i| usize::from(i) >= types.len()) {
        return None;
    }
    b.take(h.chars + h.leap * if wide { 12 } else { 8 } + h.isstd + h.isut)?;
    let rule = if wide {
        // The footer: "\n<POSIX TZ>\n".
        b.0.strip_prefix(b"\n")
            .and_then(|r| r.split(|&c| c == b'\n').next())
            .and_then(|r| std::str::from_utf8(r).ok())
            .filter(|r| !r.is_empty())
            .and_then(Rule::parse)
    } else {
        None
    };
    Some(Zone {
        name: name.to_owned(),
        times,
        idx,
        types,
        rule,
    })
}

/// A POSIX `TZ` rule: a standard offset and, optionally, daylight time between two dates of
/// every year.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Rule {
    /// Standard time's offset, seconds east of UTC.
    std: i32,
    /// Daylight time's, with when it starts and ends; `None` for a zone without.
    dst: Option<(i32, Date, Date)>,
}

/// A date of a rule, and the local time of day the change happens at, in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Date {
    kind: DateKind,
    time: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DateKind {
    /// `Jn`: day 1 to 365, February 29 never counted.
    Julian(u16),
    /// `n`: day 0 to 365, February 29 counted.
    Day(u16),
    /// `Mm.w.d`: day `d` (0 Sunday) of week `w` (1 to 5, 5 the last) of month `m`.
    Month(u8, u8, u8),
}

/// A cursor over a rule's text.
struct Text<'a>(&'a [u8]);

impl Text<'_> {
    fn peek(&self) -> Option<u8> {
        self.0.first().copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.0 = self.0.get(1..).unwrap_or_default();
            true
        } else {
            false
        }
    }

    fn num(&mut self, max: u32) -> Option<u32> {
        let n = self.0.iter().take_while(|c| c.is_ascii_digit()).count();
        if n == 0 {
            return None;
        }
        let mut v: u32 = 0;
        for &c in self.0.get(..n)? {
            v = v
                .checked_mul(10)?
                .checked_add(u32::from(c.checked_sub(b'0')?))?;
        }
        self.0 = self.0.get(n..)?;
        (v <= max).then_some(v)
    }

    /// A zone abbreviation: `<...>`, or three or more letters.
    fn name(&mut self) -> Option<()> {
        if self.eat(b'<') {
            let n = self.0.iter().position(|&c| c == b'>')?;
            self.0 = self.0.get(n + 1..)?;
            return Some(());
        }
        let n = self
            .0
            .iter()
            .take_while(|c| c.is_ascii_alphabetic())
            .count();
        if n < 3 {
            return None;
        }
        self.0 = self.0.get(n..)?;
        Some(())
    }

    /// `[+-]hh[:mm[:ss]]`, in seconds; hours up to `max_h`.
    fn time(&mut self, max_h: u32) -> Option<i32> {
        let neg = if self.eat(b'-') {
            true
        } else {
            self.eat(b'+');
            false
        };
        let mut s = self.num(max_h)? * 3600;
        if self.eat(b':') {
            s += self.num(59)? * 60;
            if self.eat(b':') {
                s += self.num(59)?;
            }
        }
        let s = i32::try_from(s).ok()?;
        Some(if neg { -s } else { s })
    }

    fn date(&mut self) -> Option<Date> {
        let kind = if self.eat(b'J') {
            DateKind::Julian(u16::try_from(self.num(365)?).ok().filter(|&d| d >= 1)?)
        } else if self.eat(b'M') {
            let m = u8::try_from(self.num(12)?).ok().filter(|&m| m >= 1)?;
            if !self.eat(b'.') {
                return None;
            }
            let w = u8::try_from(self.num(5)?).ok().filter(|&w| w >= 1)?;
            if !self.eat(b'.') {
                return None;
            }
            let d = u8::try_from(self.num(6)?).ok()?;
            DateKind::Month(m, w, d)
        } else {
            DateKind::Day(u16::try_from(self.num(365)?).ok()?)
        };
        let time = if self.eat(b'/') {
            self.time(167)?
        } else {
            2 * 3600
        };
        Some(Date { kind, time })
    }
}

impl Rule {
    fn parse(s: &str) -> Option<Rule> {
        let mut t = Text(s.as_bytes());
        t.name()?;
        // POSIX offsets are west of UTC: the sign is the other way round.
        let std = -t.time(24)?;
        if t.peek().is_none() {
            return Some(Rule { std, dst: None });
        }
        t.name()?;
        let dst_off = match t.peek() {
            Some(b',') | None => std + 3600,
            Some(_) => -t.time(24)?,
        };
        let (start, end) = if t.eat(b',') {
            let start = t.date()?;
            if !t.eat(b',') {
                return None;
            }
            (start, t.date()?)
        } else {
            // The US rules, as Go's tzset takes a rule without dates.
            (
                Date {
                    kind: DateKind::Month(3, 2, 0),
                    time: 7200,
                },
                Date {
                    kind: DateKind::Month(11, 1, 0),
                    time: 7200,
                },
            )
        };
        if t.peek().is_some() {
            return None;
        }
        Some(Rule {
            std,
            dst: Some((dst_off, start, end)),
        })
    }

    fn offset(&self, t: i64) -> i32 {
        let Some((dst, start, end)) = self.dst else {
            return self.std;
        };
        let (year, _, _) =
            crate::time::civil(t.saturating_add(i64::from(self.std)).div_euclid(86_400));
        let year_start = crate::time::days_from_civil(year, 1, 1).saturating_mul(86_400);
        // Each change is at a local time: the start's in standard time, the end's in daylight.
        let s = year_start + start.secs(year) - i64::from(self.std);
        let e = year_start + end.secs(year) - i64::from(dst);
        let in_dst = if s < e {
            s <= t && t < e
        } else {
            // The southern hemisphere: daylight time spans the new year.
            !(e <= t && t < s)
        };
        if in_dst { dst } else { self.std }
    }
}

impl Date {
    /// Seconds from the start of `year` to this date's change.
    fn secs(self, year: i64) -> i64 {
        let leap = is_leap(year);
        let day: i64 = match self.kind {
            DateKind::Julian(d) => {
                let d = i64::from(d) - 1;
                if leap && d >= 59 { d + 1 } else { d }
            }
            DateKind::Day(d) => i64::from(d),
            DateKind::Month(m, w, wd) => {
                let first = crate::time::days_from_civil(year, u32::from(m), 1);
                let jan1 = crate::time::days_from_civil(year, 1, 1);
                // 1970-01-01 was a Thursday (4).
                let first_wd = (first + 4).rem_euclid(7);
                let mut mday =
                    1 + (i64::from(wd) - first_wd).rem_euclid(7) + 7 * (i64::from(w) - 1);
                let len = i64::from(month_len(year, m));
                while mday > len {
                    mday -= 7;
                }
                first - jan1 + mday - 1
            }
        };
        day * 86_400 + i64::from(self.time)
    }
}

fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

fn month_len(y: i64, m: u8) -> u8 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{Rule, Zone};
    use crate::time::days_from_civil;

    fn at(y: i64, m: u32, d: u32, h: i64, min: i64) -> i64 {
        days_from_civil(y, m, d) * 86_400 + h * 3600 + min * 60
    }

    #[test]
    fn rules() {
        let ny = Rule::parse("EST5EDT,M3.2.0,M11.1.0").unwrap();
        // 2026: DST from March 8 07:00 UTC to November 1 06:00 UTC.
        assert_eq!(ny.offset(at(2026, 3, 8, 6, 59)), -5 * 3600);
        assert_eq!(ny.offset(at(2026, 3, 8, 7, 0)), -4 * 3600);
        assert_eq!(ny.offset(at(2026, 11, 1, 5, 59)), -4 * 3600);
        assert_eq!(ny.offset(at(2026, 11, 1, 6, 0)), -5 * 3600);
        let sh = Rule::parse("CST-8").unwrap();
        assert_eq!(sh.offset(0), 8 * 3600);
        let named = Rule::parse("<+0530>-5:30").unwrap();
        assert_eq!(named.offset(0), 5 * 3600 + 1800);
        // Sydney: daylight time over the new year.
        let syd = Rule::parse("AEST-10AEDT,M10.1.0,M4.1.0/3").unwrap();
        assert_eq!(syd.offset(at(2026, 1, 15, 0, 0)), 11 * 3600);
        assert_eq!(syd.offset(at(2026, 6, 15, 0, 0)), 10 * 3600);
        // Without dates: the US rules.
        let us = Rule::parse("PST8PDT").unwrap();
        assert_eq!(us.offset(at(2026, 7, 1, 0, 0)), -7 * 3600);
        for bad in [
            "",
            "E5",
            "EST",
            "EST5EDT,M3.2.0",
            "EST5EDT,M13.1.0,M11.1.0",
            "EST5x",
        ] {
            assert!(Rule::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn names() {
        assert!(Zone::load("utc").unwrap().is_utc());
        assert!(Zone::load(" UTC ").unwrap().is_utc());
        assert_eq!(Zone::load("").unwrap().name(), "Local");
        assert_eq!(Zone::load("Local").unwrap().name(), "Local");
        for bad in [
            "Nowhere/City",
            "../etc/passwd",
            "/etc/localtime",
            "a//b",
            "x y",
        ] {
            let e = Zone::load(bad).unwrap_err().to_string();
            assert!(e.contains("is not a time zone"), "{e}");
        }
    }

    /// A TZif v2 file of two transitions, its footer a rule.
    fn tzif() -> Vec<u8> {
        let mut v = Vec::new();
        let header = |v: &mut Vec<u8>, times: u32, types: u32, chars: u32| {
            v.extend_from_slice(b"TZif2");
            v.extend_from_slice(&[0; 15]);
            for n in [0, 0, 0, times, types, chars] {
                v.extend_from_slice(&u32::to_be_bytes(n));
            }
        };
        // Version 1 data: no transitions, one type.
        header(&mut v, 0, 1, 4);
        v.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        v.extend_from_slice(b"UTC\0");
        header(&mut v, 2, 2, 8);
        v.extend_from_slice(&i64::to_be_bytes(1000));
        v.extend_from_slice(&i64::to_be_bytes(2000));
        v.extend_from_slice(&[0, 1]);
        v.extend_from_slice(&i32::to_be_bytes(3600));
        v.extend_from_slice(&[1, 0]);
        v.extend_from_slice(&i32::to_be_bytes(0));
        v.extend_from_slice(&[0, 4]);
        v.extend_from_slice(b"ONE\0TWO\0");
        v.extend_from_slice(b"\n<+02>-2\n");
        v
    }

    #[test]
    fn tzif_files() {
        let z = Zone::from_tzif("Test/Zone", &tzif()).unwrap();
        assert_eq!(z.name(), "Test/Zone");
        // Before the first transition: the first standard type.
        assert_eq!(z.offset(0), 0);
        assert_eq!(z.offset(1000), 3600);
        assert_eq!(z.offset(1999), 3600);
        // From the last on: the footer's rule.
        assert_eq!(z.offset(2000), 7200);
        assert_eq!(z.offset(i64::MAX / 2), 7200);
        // Truncated anywhere, or a type index out of range: not a zone.
        let good = tzif();
        for n in 0..good.len() - 9 {
            let _ = Zone::from_tzif("t", &good[..n]);
        }
        let mut bad = tzif();
        let i = bad.len() - 9 - 8 - 6 * 2 - 2;
        bad[i] = 9;
        assert!(Zone::from_tzif("t", &bad).is_none());
    }

    #[test]
    fn system_zones() {
        // Where the system has zoneinfo files (macOS and most Linux images).
        if std::path::Path::new("/usr/share/zoneinfo/Asia/Shanghai").exists() {
            let z = Zone::load("Asia/Shanghai").unwrap();
            assert_eq!(z.offset(at(2026, 10, 19, 7, 39)), 8 * 3600);
        }
        if std::path::Path::new("/usr/share/zoneinfo/America/New_York").exists() {
            let z = Zone::load("America/New_York").unwrap();
            assert_eq!(z.offset(at(2026, 7, 1, 0, 0)), -4 * 3600);
            assert_eq!(z.offset(at(2026, 1, 1, 0, 0)), -5 * 3600);
            assert_eq!(z.offset(at(2040, 7, 1, 0, 0)), -4 * 3600);
            assert_eq!(z.offset(at(1990, 1, 1, 0, 0)), -5 * 3600);
        }
    }
}
