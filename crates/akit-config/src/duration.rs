//! Go's durations, as fit's config writes them (`config.Duration`).

use std::time::Duration;

/// Parses `s` as Go's `time.ParseDuration` does: an optional sign, then one or more decimal
/// numbers, each with a unit (`ns`, `us`, `µs`, `ms`, `s`, `m`, `h`), as `"300ms"`, `"1.5h"` or
/// `"2h45m"`; `"0"` alone needs no unit. `None` when it is not a duration (or does not fit in Go's
/// 64-bit nanoseconds); `Some(Err(()))` when it is a negative one.
#[must_use]
pub fn parse(s: &str) -> Option<Result<Duration, ()>> {
    let (negative, mut rest) = match s.as_bytes().first() {
        Some(b'-') => (true, s.get(1..)?),
        Some(b'+') => (false, s.get(1..)?),
        _ => (false, s),
    };
    if rest == "0" {
        return Some(Ok(Duration::ZERO));
    }
    if rest.is_empty() {
        return None;
    }
    let mut total: u128 = 0;
    while !rest.is_empty() {
        let int_len = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let (int, after) = rest.split_at(int_len);
        let (frac, after) = match after.strip_prefix('.') {
            Some(f) => {
                let n = f.find(|c: char| !c.is_ascii_digit()).unwrap_or(f.len());
                f.split_at(n)
            }
            None => ("", after),
        };
        if int.is_empty() && frac.is_empty() {
            return None;
        }
        let unit_len = after
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(after.len());
        let (unit, next) = after.split_at(unit_len);
        let scale: u128 = match unit {
            "ns" => 1,
            "us" | "µs" | "μs" => 1_000,
            "ms" => 1_000_000,
            "s" => 1_000_000_000,
            "m" => 60_000_000_000,
            "h" => 3_600_000_000_000,
            _ => return None,
        };
        let whole: u128 = if int.is_empty() {
            0
        } else {
            int.parse::<u128>().ok()?
        };
        let mut part = whole.checked_mul(scale)?;
        // The fraction, digit by digit, as Go keeps it: truncated to the nanosecond.
        let mut place = scale;
        for b in frac.bytes() {
            place /= 10;
            if place == 0 {
                break;
            }
            part = part.checked_add(u128::from(b.wrapping_sub(b'0')).checked_mul(place)?)?;
        }
        total = total.checked_add(part)?;
        if total > i64::MAX as u128 {
            return None;
        }
        rest = next;
    }
    let nanos = u64::try_from(total).ok()?;
    if negative && nanos > 0 {
        return Some(Err(()));
    }
    Some(Ok(Duration::from_nanos(nanos)))
}

/// `d` as Go prints a duration: `30s`, `1m30s`, `1h0m0s`, `500ms`, `1.5s`.
#[must_use]
pub fn format(d: Duration) -> String {
    let secs = d.as_secs();
    let nanos = d.subsec_nanos();
    if secs == 0 {
        let (n, unit, scale) = match nanos {
            0 => return "0s".to_owned(),
            n if n >= 1_000_000 => (n, "ms", 1_000_000),
            n if n >= 1_000 => (n, "\u{b5}s", 1_000),
            n => return format!("{n}ns"),
        };
        return format!(
            "{}{unit}",
            decimal(
                u64::from(n.checked_div(scale).unwrap_or(0)),
                n.checked_rem(scale).unwrap_or(0),
                scale,
            )
        );
    }
    let s_part = decimal(secs % 60, nanos, 1_000_000_000);
    let (h, m) = (secs / 3600, (secs / 60) % 60);
    if h > 0 {
        format!("{h}h{m}m{s_part}s")
    } else if m > 0 {
        format!("{m}m{s_part}s")
    } else {
        format!("{s_part}s")
    }
}

/// `whole.frac` with `frac` in units of `1/scale`, trailing zeros dropped.
fn decimal(whole: u64, frac: u32, scale: u32) -> String {
    if frac == 0 {
        return whole.to_string();
    }
    let width = scale.to_string().len().saturating_sub(1);
    let f = format!("{frac:0width$}");
    format!("{whole}.{}", f.trim_end_matches('0'))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{format, parse};

    #[test]
    fn go_formats() {
        for (d, want) in [
            (Duration::ZERO, "0s"),
            (Duration::from_nanos(7), "7ns"),
            (Duration::from_micros(1500), "1.5ms"),
            (Duration::from_millis(100), "100ms"),
            (Duration::from_secs(5), "5s"),
            (Duration::from_millis(2500), "2.5s"),
            (Duration::from_secs(90), "1m30s"),
            (Duration::from_secs(3600), "1h0m0s"),
        ] {
            assert_eq!(format(d), want);
        }
    }

    #[test]
    fn go_durations() {
        let ok = |s: &str, d: Duration| assert_eq!(parse(s), Some(Ok(d)), "{s}");
        ok("0", Duration::ZERO);
        ok("10s", Duration::from_secs(10));
        ok("1m30s", Duration::from_secs(90));
        ok("250ms", Duration::from_millis(250));
        ok("1.5h", Duration::from_secs(5400));
        ok(".5s", Duration::from_millis(500));
        ok("1.s", Duration::from_secs(1));
        ok("3µs", Duration::from_micros(3));
        ok("2h45m", Duration::from_secs(9900));
        ok("+1s", Duration::from_secs(1));
        ok("-0s", Duration::ZERO);
        assert_eq!(parse("-1s"), Some(Err(())));
        for bad in [
            "",
            "1",
            "s",
            "1x",
            "1.2.3s",
            ".s",
            "-",
            "9999999999h",
            "1 s",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }
}
