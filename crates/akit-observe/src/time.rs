//! Dates from seconds since the epoch, and the times lines are written with, formatted without
//! a layout: RFC 3339 with milliseconds (`2026-10-19T07:39:56.123Z`, `…+08:00`), and fit's
//! `line` format's `2026-10-19 15:39:56.123`.
//!
//! Seconds are clamped to [`MAX_SECS`] either side of the epoch (35 million years) before any
//! date arithmetic, which bounds every day, year and second count below far from overflowing.
#![allow(clippy::arithmetic_side_effects)]

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How far from the epoch, in seconds, a time is taken as it is; further ones are clamped.
pub const MAX_SECS: i64 = 1 << 50;

/// `secs` within [`MAX_SECS`] of the epoch.
#[must_use]
pub fn clamp(secs: i64) -> i64 {
    secs.clamp(-MAX_SECS, MAX_SECS)
}

/// The days since 1970-01-01 of a date of the proleptic Gregorian calendar (Howard Hinnant's
/// `days_from_civil`).
#[must_use]
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(m);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of `days` since 1970-01-01: year, month (1 to 12), day (1 to 31).
#[must_use]
pub fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    // Both are in range by construction.
    (
        y,
        u32::try_from(m).unwrap_or(1),
        u32::try_from(d).unwrap_or(1),
    )
}

/// A time as seconds and milliseconds since the epoch (negative seconds before it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    /// Whole seconds since the epoch.
    pub secs: i64,
    /// Milliseconds past them, 0 to 999.
    pub millis: u32,
}

impl Stamp {
    /// `t` as seconds and milliseconds since the epoch.
    #[must_use]
    pub fn of(t: SystemTime) -> Stamp {
        match t.duration_since(UNIX_EPOCH) {
            Ok(d) => Stamp {
                secs: i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
                millis: d.subsec_millis(),
            },
            Err(e) => {
                let d = e.duration();
                let mut secs = -i64::try_from(d.as_secs()).unwrap_or(i64::MAX);
                let mut millis = 0;
                let sub = d.subsec_nanos();
                if sub > 0 {
                    secs -= 1;
                    millis = (1_000_000_000 - sub) / 1_000_000;
                }
                Stamp { secs, millis }
            }
        }
    }
}

/// The ASCII digit of `n % 10`.
fn digit(n: u32) -> u8 {
    // Below 10, so it fits.
    b'0' + u8::try_from(n % 10).unwrap_or(0)
}

fn two(b: &mut Vec<u8>, n: u32) {
    b.push(digit(n / 10));
    b.push(digit(n));
}

/// Appends the local date and time of `secs` (already shifted by the zone's offset):
/// `2006-01-02<sep>15:04:05`.
fn date_time(b: &mut Vec<u8>, secs: i64, sep: u8) {
    let secs = clamp(secs);
    let (y, m, d) = civil(secs.div_euclid(86_400));
    let tod = u32::try_from(secs.rem_euclid(86_400)).unwrap_or(0);
    match u32::try_from(y) {
        Ok(y) if y <= 9999 => {
            two(b, y / 100);
            two(b, y % 100);
        }
        _ => b.extend_from_slice(y.to_string().as_bytes()),
    }
    b.push(b'-');
    two(b, m);
    b.push(b'-');
    two(b, d);
    b.push(sep);
    two(b, tod / 3600);
    b.push(b':');
    two(b, tod / 60 % 60);
    b.push(b':');
    two(b, tod % 60);
}

fn millis(b: &mut Vec<u8>, ms: u32) {
    b.push(b'.');
    b.push(digit(ms / 100));
    two(b, ms % 100);
}

/// Appends `t` as RFC 3339 with milliseconds at `offset` seconds east of UTC, as Go's
/// `2006-01-02T15:04:05.000Z07:00`: `Z` for UTC, else `±hh:mm`.
pub fn rfc3339(b: &mut Vec<u8>, t: Stamp, offset: i32) {
    date_time(b, t.secs.saturating_add(i64::from(offset)), b'T');
    millis(b, t.millis);
    if offset == 0 {
        b.push(b'Z');
        return;
    }
    b.push(if offset < 0 { b'-' } else { b'+' });
    let o = offset.unsigned_abs() / 60;
    two(b, o / 60);
    b.push(b':');
    two(b, o % 60);
}

/// Appends `secs` and `nanos` since the epoch at `offset` as Go's `time.RFC3339Nano`: the
/// fraction's trailing zeros dropped, none when it is zero, as log/slog's JSON writes a time.
pub fn rfc3339_nano(b: &mut Vec<u8>, secs: i64, nanos: u32, offset: i32) {
    let start = b.len();
    rfc3339(b, Stamp { secs, millis: 0 }, offset);
    // `…:05.000Z` or `…:05.000+08:00`: the fraction goes where `.000` is.
    let frac_at = start.saturating_add(19);
    let tail: Vec<u8> = b
        .get(frac_at.saturating_add(4)..)
        .unwrap_or_default()
        .to_vec();
    b.truncate(frac_at);
    if nanos > 0 {
        let mut digits = format!("{nanos:09}");
        while digits.ends_with('0') {
            digits.pop();
        }
        b.push(b'.');
        b.extend_from_slice(digits.as_bytes());
    }
    b.extend_from_slice(&tail);
}

/// Appends `t` at `offset` seconds east of UTC as fit's `line` format writes it:
/// `2006-01-02 15:04:05.000`.
pub fn line(b: &mut Vec<u8>, t: Stamp, offset: i32) {
    date_time(b, t.secs.saturating_add(i64::from(offset)), b' ');
    millis(b, t.millis);
}

/// Appends `d` in seconds with six decimals, rounded to the nearest microsecond, as fit's
/// `appendSeconds`: `0.000412`.
pub fn seconds(b: &mut Vec<u8>, d: Duration) {
    let us = d.as_nanos().saturating_add(500) / 1000;
    b.extend_from_slice((us / 1_000_000).to_string().as_bytes());
    let frac = u32::try_from(us % 1_000_000).unwrap_or(0);
    b.push(b'.');
    for div in [100_000, 10_000, 1000, 100, 10, 1] {
        b.push(digit(frac / div));
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::{Stamp, civil, days_from_civil};

    fn s(f: impl FnOnce(&mut Vec<u8>)) -> String {
        let mut b = Vec::new();
        f(&mut b);
        String::from_utf8(b).unwrap_or_default()
    }

    #[test]
    fn dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil(0), (1970, 1, 1));
        for days in [-800_000, -1, 59, 60, 11_016, 20_745, 2_932_896] {
            let (y, m, d) = civil(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(civil(days_from_civil(2024, 2, 29)), (2024, 2, 29));
    }

    #[test]
    fn times() {
        // 2026-10-19T07:39:56.123Z
        let t = UNIX_EPOCH + Duration::from_millis(1_792_395_596_123);
        let st = Stamp::of(t);
        assert_eq!(s(|b| super::rfc3339(b, st, 0)), "2026-10-19T07:39:56.123Z");
        assert_eq!(
            s(|b| super::rfc3339(b, st, 8 * 3600)),
            "2026-10-19T15:39:56.123+08:00"
        );
        assert_eq!(
            s(|b| super::rfc3339(b, st, -(3 * 3600 + 1800))),
            "2026-10-19T04:09:56.123-03:30"
        );
        assert_eq!(
            s(|b| super::line(b, st, 8 * 3600)),
            "2026-10-19 15:39:56.123"
        );
        let before = Stamp::of(UNIX_EPOCH - Duration::from_millis(1));
        assert_eq!(
            s(|b| super::rfc3339(b, before, 0)),
            "1969-12-31T23:59:59.999Z"
        );
    }

    #[test]
    fn durations() {
        for (d, want) in [
            (Duration::ZERO, "0.000000"),
            (Duration::from_nanos(412_499), "0.000412"),
            (Duration::from_nanos(412_500), "0.000413"),
            (Duration::from_millis(1500), "1.500000"),
            (Duration::from_secs(90061), "90061.000000"),
        ] {
            assert_eq!(s(|b| super::seconds(b, d)), want);
        }
    }
}
