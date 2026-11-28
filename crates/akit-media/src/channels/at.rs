//! The times a schedule's slot falls on (fit's `ChannelAt.Prev` and `Next`), in a channel's time
//! zone: a time of day on the days it is on, as written across a change of the clocks, or a
//! date's, once.

use akit_config::channels::ChannelAt;
use akit_observe::Zone;
use akit_observe::time::{civil, days_from_civil};

const DAY: i64 = 86_400;
const NANOS: i64 = 1_000_000_000;

/// The time, in seconds since 1970, of the local time `local` (seconds since 1970 on the zone's
/// clock), as Go's `time.Date` takes it: the offset of the time it would be at the first
/// guess, or, when that one is not the zone's offset then, the offset then.
fn utc_of(zone: &Zone, local: i64) -> i64 {
    let first = i64::from(zone.offset(local));
    let guess = local.saturating_sub(first);
    let then = i64::from(zone.offset(guess));
    if then == first {
        guess
    } else {
        local.saturating_sub(then)
    }
}

/// The slot's time of day on day `days` (since 1970) of the zone's calendar, in seconds since
/// 1970.
fn on_day(at: &ChannelAt, zone: &Zone, days: i64) -> i64 {
    utc_of(
        zone,
        days.saturating_mul(DAY).saturating_add(i64::from(at.clock)),
    )
}

/// The day of the zone's calendar `t` (seconds since 1970) is on.
fn day_of(zone: &Zone, t: i64) -> i64 {
    t.saturating_add(i64::from(zone.offset(t))).div_euclid(DAY)
}

/// Whether the slot is on day `days`: its weekday's bit (1970-01-01 a Thursday).
fn is_on(at: &ChannelAt, days: i64) -> bool {
    let wd = days.saturating_add(4).rem_euclid(7);
    at.days & (1u8 << wd) != 0
}

fn date_time(at: &ChannelAt, zone: &Zone, (y, m, d): (i64, u32, u32)) -> i64 {
    on_day(at, zone, days_from_civil(y, m, d))
}

/// The slot's latest start at or before `t`, both in nanoseconds since 1970; `None` for a date
/// after `t`, or none within the last week.
#[must_use]
pub(crate) fn prev(at: &ChannelAt, zone: &Zone, t: i64) -> Option<i64> {
    let secs = t.div_euclid(NANOS);
    if let Some(date) = at.date {
        let s = date_time(at, zone, date).saturating_mul(NANOS);
        return (s <= t).then_some(s);
    }
    let today = day_of(zone, secs);
    (0..8).find_map(|i| {
        let day = today.saturating_sub(i);
        let s = on_day(at, zone, day).saturating_mul(NANOS);
        (is_on(at, day) && s <= t).then_some(s)
    })
}

/// The slot's first start after `t`, both in nanoseconds since 1970; `None` for a date at or
/// before `t`.
#[must_use]
pub(crate) fn next(at: &ChannelAt, zone: &Zone, t: i64) -> Option<i64> {
    let secs = t.div_euclid(NANOS);
    if let Some(date) = at.date {
        let s = date_time(at, zone, date).saturating_mul(NANOS);
        return (s > t).then_some(s);
    }
    let today = day_of(zone, secs);
    (0..9).find_map(|i| {
        let day = today.saturating_add(i);
        let s = on_day(at, zone, day).saturating_mul(NANOS);
        (is_on(at, day) && s > t).then_some(s)
    })
}

/// The date and time of `t` (nanoseconds since 1970) in `zone`, as RFC 3339 with the zone's
/// offset, for the Admin API.
#[must_use]
pub(crate) fn rfc3339(zone: &Zone, t: i64) -> String {
    let secs = t.div_euclid(NANOS);
    let off = zone.offset(secs);
    let local = secs.saturating_add(i64::from(off));
    let (y, m, d) = civil(local.div_euclid(DAY));
    let s = local.rem_euclid(DAY);
    let mut out = format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        s / 3600,
        s / 60 % 60,
        s % 60
    );
    let ns = t.rem_euclid(NANOS);
    if ns != 0 {
        let frac = format!("{ns:09}");
        out.push('.');
        out.push_str(frac.trim_end_matches('0'));
    }
    if off == 0 {
        out.push('Z');
    } else {
        let a = off.unsigned_abs();
        out.push_str(&format!(
            "{}{:02}:{:02}",
            if off < 0 { '-' } else { '+' },
            a / 3600,
            a / 60 % 60
        ));
    }
    out
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects, clippy::unwrap_used)]
mod tests {
    use super::*;
    use akit_config::channels::parse_channel_at;

    fn at(y: i64, m: u32, d: u32, h: u32, mi: u32, s: u32, zone: &Zone) -> i64 {
        utc_of(
            zone,
            days_from_civil(y, m, d) * DAY + i64::from(h * 3600 + mi * 60 + s),
        ) * NANOS
    }

    /// fit's `TestChannelAt`.
    #[test]
    fn channel_at() {
        let (Ok(sh), Ok(ny)) = (Zone::load("Asia/Shanghai"), Zone::load("America/New_York")) else {
            // A host without zoneinfo files: nothing to check against.
            return;
        };
        // A Friday, 10:00 in Shanghai.
        let now = at(2026, 11, 6, 10, 0, 0, &sh);
        let day = DAY * NANOS;
        type Case<'a> = (&'a str, &'a Zone, Option<i64>, Option<i64>);
        let cases: Vec<Case<'_>> = vec![
            (
                "09:30",
                &sh,
                Some(at(2026, 11, 6, 9, 30, 0, &sh)),
                Some(at(2026, 11, 7, 9, 30, 0, &sh)),
            ),
            ("10:00:00", &sh, Some(now), Some(now + day)),
            (
                "mon 08:00",
                &sh,
                Some(at(2026, 11, 2, 8, 0, 0, &sh)),
                Some(at(2026, 11, 9, 8, 0, 0, &sh)),
            ),
            (
                "sat-mon 12:00",
                &sh,
                Some(at(2026, 11, 2, 12, 0, 0, &sh)),
                Some(at(2026, 11, 7, 12, 0, 0, &sh)),
            ),
            (
                "FRI,sun 11:00",
                &sh,
                Some(at(2026, 11, 1, 11, 0, 0, &sh)),
                Some(at(2026, 11, 6, 11, 0, 0, &sh)),
            ),
            (
                "2026-11-06 09:00",
                &sh,
                Some(at(2026, 11, 6, 9, 0, 0, &sh)),
                None,
            ),
            (
                "2026-11-07T09:00:30",
                &sh,
                None,
                Some(at(2026, 11, 7, 9, 0, 30, &sh)),
            ),
            // New York leaves summer time on 2026-11-01 at 02:00: 01:30 is the time of day as
            // written.
            (
                "01:30",
                &ny,
                Some(at(2026, 11, 5, 1, 30, 0, &ny)),
                Some(at(2026, 11, 6, 1, 30, 0, &ny)),
            ),
        ];
        for (text, zone, p, n) in cases {
            let a = parse_channel_at(text).unwrap();
            assert_eq!(prev(&a, zone, now), p, "{text}: prev");
            assert_eq!(next(&a, zone, now), n, "{text}: next");
        }
        // 2026-11-05 01:30 in New York is 06:30 UTC (EST, -5).
        assert_eq!(
            at(2026, 11, 5, 1, 30, 0, &ny),
            (days_from_civil(2026, 11, 5) * DAY + 6 * 3600 + 30 * 60) * NANOS
        );
        let none = ChannelAt::default();
        assert_eq!(prev(&none, &sh, now), None);
        assert_eq!(next(&none, &sh, now), None);
        assert_eq!(rfc3339(&sh, now), "2026-11-06T10:00:00+08:00");
        assert_eq!(
            rfc3339(&Zone::utc(), now + 5_000_000),
            "2026-11-06T02:00:00.005Z"
        );
    }
}
