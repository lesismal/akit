//! ACME Renewal Information (ARI, RFC 9773), as fit's `tlsmgr/ari.go`: the CA suggests, for each
//! certificate, a window to renew it in, which may come earlier than its lifetime says when the
//! CA needs it replaced. The manager asks after each run that leaves a name with a certificate,
//! and again when the CA's `Retry-After` says; the certificate is due at a time picked at random
//! in the window, or at once when the window has begun, and by its lifetime when the CA has no
//! ARI or does not answer.

use std::time::{Duration, Instant};

use akio_crypto::base64::URL_SAFE_NO_PAD;

/// The least a manager waits before it asks about a certificate again, by default.
pub(crate) const ARI_CHECK_MIN: Duration = Duration::from_secs(60);
/// The most.
pub(crate) const ARI_CHECK_MAX: Duration = Duration::from_secs(24 * 3600);
/// The wait when the CA does not say.
pub(crate) const ARI_CHECK_DEFAULT: Duration = Duration::from_secs(6 * 3600);
/// The largest answer read (rule 11).
pub(crate) const ARI_MAX_BYTES: usize = 64 << 10;

/// What a manager knows from ARI about a name's certificate.
#[derive(Debug, Clone)]
pub(crate) struct AriState {
    /// The serial number of the certificate it is about.
    pub(crate) serial: Vec<u8>,
    /// When the certificate is due (Unix seconds), picked in the window; `None` when the CA
    /// suggested none.
    pub(crate) renew_at: Option<i64>,
    /// When to ask again.
    pub(crate) check_at: Instant,
}

impl AriState {
    /// Whether it says the certificate with serial number `serial` is due at `now` (Unix
    /// seconds).
    pub(crate) fn due(&self, serial: &[u8], now: i64) -> bool {
        self.serial == serial && self.renew_at.is_some_and(|t| now >= t)
    }
}

/// A certificate's ARI identifier: its authority key identifier and its serial number's DER
/// contents, each in base64url without padding, joined by a dot; `None` for a certificate
/// without an authority key identifier.
pub(crate) fn cert_id(aki: Option<&[u8]>, serial: &[u8]) -> Option<String> {
    let aki = aki.filter(|a| !a.is_empty())?;
    let serial: Vec<u8> = if serial.first().is_none_or(|b| b & 0x80 != 0) {
        // The DER contents of a positive INTEGER whose top bit is set.
        std::iter::once(0).chain(serial.iter().copied()).collect()
    } else {
        serial.to_vec()
    };
    Some(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(aki),
        URL_SAFE_NO_PAD.encode(&serial)
    ))
}

/// How long a `Retry-After` field says to wait, in seconds or as an HTTP date, kept between
/// `least` and `most`; [`ARI_CHECK_DEFAULT`] when it says nothing that parses. `now` is in Unix
/// seconds.
pub(crate) fn retry_after(v: Option<&str>, now: i64, least: Duration, most: Duration) -> Duration {
    let v = v.map(str::trim).unwrap_or("");
    let d = if let Ok(s) = v.parse::<u64>() {
        Duration::from_secs(s)
    } else if let Some(t) = http_date(v) {
        Duration::from_secs(u64::try_from(t.saturating_sub(now)).unwrap_or(0))
    } else {
        ARI_CHECK_DEFAULT
    };
    d.max(least).min(most.max(least))
}

/// When in the window `start`..`end` (Unix seconds) a certificate is due: `now` once it has
/// begun, a time picked at random in it before then; `None` for a window that is not one.
pub(crate) fn pick(start: i64, end: i64, now: i64) -> Option<i64> {
    if end <= start {
        return None;
    }
    if now >= start {
        return Some(now);
    }
    let mut r = [0u8; 8];
    // Without randomness every instance picks the window's start.
    let _ = akio_crypto::rand::fill_random(&mut r);
    let span = u64::try_from(end.saturating_sub(start)).unwrap_or(0);
    let off = u64::from_le_bytes(r).checked_rem(span).unwrap_or(0);
    Some(start.saturating_add(i64::try_from(off).unwrap_or(0)))
}

// The operands below are range-checked first (months, days, hours, minutes, seconds, offsets
// of two digits, the year of era within 0..400); what is not, the year, goes through checked
// operations.
#[allow(clippy::arithmetic_side_effects)]
/// Days from 1970-01-01 to the date `y`-`m`-`d` of the proleptic Gregorian calendar.
fn days_from_civil(y: i64, m: i64, d: i64) -> Option<i64> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = if m <= 2 { y.checked_sub(1)? } else { y };
    let era = y.checked_div_euclid(400)?;
    let yoe = y.checked_sub(era.checked_mul(400)?)?;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era.checked_mul(146_097)?
        .checked_add(doe)?
        .checked_sub(719_468)
}

/// Unix seconds of a date and a time of day.
#[allow(clippy::arithmetic_side_effects)]
fn unix(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64) -> Option<i64> {
    if !(0..24).contains(&h) || !(0..60).contains(&mi) || !(0..=60).contains(&s) {
        return None;
    }
    days_from_civil(y, mo, d)?
        .checked_mul(86_400)?
        .checked_add(h * 3600 + mi * 60 + s)
}

/// The number of the ASCII digits `s`, which must all be digits.
fn num(s: &str) -> Option<i64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// An RFC 3339 timestamp (`2026-10-20T12:00:00Z`, fractions and offsets allowed), in Unix
/// seconds, fractions dropped.
pub(crate) fn rfc3339(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, rest) = s.split_at_checked(10)?;
    let rest = rest.strip_prefix(['T', 't', ' '])?;
    let (y, mo, d) = (
        num(date.get(0..4)?)?,
        num(date.get(5..7)?)?,
        num(date.get(8..10)?)?,
    );
    if date.get(4..5)? != "-" || date.get(7..8)? != "-" {
        return None;
    }
    let (time, zone) = rest.split_at_checked(8)?;
    let (h, mi, sec) = (
        num(time.get(0..2)?)?,
        num(time.get(3..5)?)?,
        num(time.get(6..8)?)?,
    );
    if time.get(2..3)? != ":" || time.get(5..6)? != ":" {
        return None;
    }
    let zone = match zone.strip_prefix('.') {
        Some(z) => z.trim_start_matches(|c: char| c.is_ascii_digit()),
        None => zone,
    };
    let offset = match zone {
        "Z" | "z" => 0,
        z => {
            let sign = match z.get(0..1)? {
                "+" => 1,
                "-" => -1,
                _ => return None,
            };
            if z.len() != 6 || z.get(3..4)? != ":" {
                return None;
            }
            let (h, m) = (num(z.get(1..3)?)?, num(z.get(4..6)?)?);
            // Two digits each.
            #[allow(clippy::arithmetic_side_effects)]
            let off = sign * (h * 3600 + m * 60);
            off
        }
    };
    unix(y, mo, d, h, mi, sec)?.checked_sub(offset)
}

/// An HTTP date in its preferred form (IMF-fixdate: `Tue, 20 Oct 2026 14:00:00 GMT`), in Unix
/// seconds.
pub(crate) fn http_date(s: &str) -> Option<i64> {
    let mut f = s.split_ascii_whitespace();
    let (_day, d, mon, y, time, gmt) = (
        f.next()?,
        f.next()?,
        f.next()?,
        f.next()?,
        f.next()?,
        f.next()?,
    );
    if gmt != "GMT" || f.next().is_some() {
        return None;
    }
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mo = MONTHS.iter().position(|m| *m == mon)?;
    let mut t = time.split(':');
    let (h, mi, sec) = (num(t.next()?)?, num(t.next()?)?, num(t.next()?)?);
    unix(
        num(y)?,
        i64::try_from(mo).ok()?.checked_add(1)?,
        num(d)?,
        h,
        mi,
        sec,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// fit's `TestCertID`: RFC 9773's example.
    #[test]
    fn cert_ids_as_rfc_9773() {
        let aki = [
            0x69, 0x88, 0x5b, 0x6b, 0x87, 0x46, 0x40, 0x41, 0xe1, 0xb3, 0x7b, 0x84, 0x7b, 0xa0,
            0xae, 0x2c, 0xde, 0x01, 0xc8, 0xd4,
        ];
        assert_eq!(
            cert_id(Some(&aki), &[0x00, 0x87, 0x65, 0x43, 0x21]).as_deref(),
            Some("aYhba4dGQEHhs3uEe6CuLN4ByNQ.AIdlQyE")
        );
        assert!(
            cert_id(Some(&aki), &[0x01, 0x02])
                .unwrap_or_default()
                .ends_with(".AQI")
        );
        assert_eq!(cert_id(None, &[1]), None);
        assert_eq!(cert_id(Some(&[]), &[1]), None);
    }

    /// fit's `TestRetryAfter`.
    #[test]
    fn retry_after_as_fit() {
        let now = rfc3339("2026-10-20T12:00:00Z").unwrap_or(0);
        for (v, want) in [
            (None, ARI_CHECK_DEFAULT),
            (Some("nonsense"), ARI_CHECK_DEFAULT),
            (Some("3600"), Duration::from_secs(3600)),
            (Some("1"), ARI_CHECK_MIN),
            (Some("9999999"), ARI_CHECK_MAX),
            (
                Some("Tue, 20 Oct 2026 14:00:00 GMT"),
                Duration::from_secs(7200),
            ),
        ] {
            assert_eq!(
                retry_after(v, now, ARI_CHECK_MIN, ARI_CHECK_MAX),
                want,
                "{v:?}"
            );
        }
    }

    #[test]
    fn timestamps() {
        assert_eq!(rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(rfc3339("2026-10-20T12:00:00Z"), Some(1_792_497_600));
        assert_eq!(
            rfc3339("2026-10-20T12:00:00.123456789Z"),
            Some(1_792_497_600)
        );
        assert_eq!(rfc3339("2026-10-20T14:00:00+02:00"), Some(1_792_497_600));
        assert_eq!(rfc3339("2000-02-29T00:00:00-00:30"), Some(951_784_200));
        for bad in [
            "",
            "2026-10-20",
            "2026-13-20T12:00:00Z",
            "2026-10-20T25:00:00Z",
            "2026-10-20T12:00:00",
            "2026/10/20T12:00:00Z",
            "2026-10-20T12:00:00+0200",
            "２０２６-10-20T12:00:00Z",
        ] {
            assert_eq!(rfc3339(bad), None, "{bad}");
        }
        assert_eq!(
            http_date("Tue, 20 Oct 2026 12:00:00 GMT"),
            Some(1_792_497_600)
        );
        assert_eq!(http_date("Tue, 20 Oct 2026 12:00:00 UTC"), None);
        assert_eq!(http_date("Tue, 20 Foo 2026 12:00:00 GMT"), None);
    }

    #[test]
    fn windows() {
        assert_eq!(pick(100, 200, 150), Some(150));
        assert_eq!(pick(100, 200, 300), Some(300));
        assert_eq!(pick(100, 100, 0), None);
        assert_eq!(pick(200, 100, 0), None);
        for _ in 0..100 {
            let t = pick(100, 200, 0).unwrap_or(0);
            assert!((100..200).contains(&t), "{t}");
        }
    }
}
