//! gRPC calls (I50), as fit's `handler/proxy/grpc.go` proxies them.
//!
//! A gRPC call is an HTTP/2 or HTTP/3 `POST` whose `Content-Type` is `application/grpc` or one of
//! its subtypes (`application/grpc+proto`; `application/grpc-web` is not one), and the proxy
//! passes it on as it does any request: its messages stream both ways as they come, each side
//! paced by the other, and the upstream's trailers, with `grpc-status` and `grpc-message`, follow
//! its response. A call needs an `h2` or `h2c` upstream. What the proxy adds:
//!
//! - its body streams to the proxy from its head, whatever length it declares, so that a client
//!   that waits for an answer to its first message is answered;
//! - `TE: trailers` goes upstream, as gRPC servers ask of their clients;
//! - `grpc-timeout` bounds the call at the proxy too, as the route's `timeout` does, and the
//!   upstream is told the shorter of the two;
//! - the response is never collected to be sent whole: a trailers-only response (its status in
//!   its head) is passed on as one, any other as it arrives;
//! - a call the proxy cannot complete is answered as gRPC answers: before its response began, a
//!   trailers-only response, status 200 with `grpc-status` and `grpc-message`; on a response cut
//!   short, those trailers. Not an HTTP status or a reset its client would not read.

use std::time::Duration;

use akio_http::{Headers, Method, Request, Response, Version};

/// `DEADLINE_EXCEEDED`.
pub(crate) const DEADLINE_EXCEEDED: u8 = 4;
/// `RESOURCE_EXHAUSTED`.
pub(crate) const RESOURCE_EXHAUSTED: u8 = 8;
/// `INTERNAL`.
pub(crate) const INTERNAL: u8 = 13;
/// `UNAVAILABLE`.
pub(crate) const UNAVAILABLE: u8 = 14;

/// The `Content-Type` of `req` when it is a gRPC call; `None` when it is not one.
pub(crate) fn call(req: &Request) -> Option<&str> {
    if !matches!(req.version, Version::Http2 | Version::Http3) || req.method != Method::Post {
        return None;
    }
    let mut types = req.headers.get_all("content-type");
    let ct = types.next()?;
    (types.next().is_none() && content_type(ct)).then_some(ct)
}

/// Whether `ct` is `application/grpc` or one of its subtypes, as `application/grpc+proto`;
/// `application/grpc-web` is not.
pub(crate) fn content_type(ct: &str) -> bool {
    const PREFIX: &str = "application/grpc";
    let (Some(head), Some(rest)) = (ct.get(..PREFIX.len()), ct.get(PREFIX.len()..)) else {
        return false;
    };
    head.eq_ignore_ascii_case(PREFIX)
        && (rest.is_empty() || rest.starts_with('+') || rest.starts_with(';'))
}

/// A `grpc-timeout` value: at most eight digits and a unit.
pub(crate) fn parse_timeout(v: &str) -> Option<Duration> {
    if !v.is_ascii() || v.len() < 2 || v.len() > 9 {
        return None;
    }
    let (digits, unit) = v.split_at(v.len().saturating_sub(1));
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u64 = digits.parse().ok()?;
    Some(match unit {
        "n" => Duration::from_nanos(n),
        "u" => Duration::from_micros(n),
        "m" => Duration::from_millis(n),
        "S" => Duration::from_secs(n),
        "M" => Duration::from_secs(n.saturating_mul(60)),
        "H" => Duration::from_secs(n.saturating_mul(3600)),
        _ => return None,
    })
}

/// `d` as `grpc-timeout` carries it, rounded up to the finest unit that holds it in eight digits.
pub(crate) fn encode_timeout(d: Duration) -> String {
    const MAX: u128 = 99_999_999;
    const UNITS: [(char, u128); 6] = [
        ('n', 1),
        ('u', 1_000),
        ('m', 1_000_000),
        ('S', 1_000_000_000),
        ('M', 60_000_000_000),
        ('H', 3_600_000_000_000),
    ];
    let nanos = d.as_nanos();
    for (unit, size) in UNITS {
        let v = nanos.div_ceil(size).max(1);
        if v <= MAX {
            return format!("{v}{unit}");
        }
    }
    format!("{MAX}H")
}

/// The gRPC status of a call the proxy would have answered with the HTTP status `status`, as
/// gRPC maps them.
pub(crate) fn code(status: u16) -> u8 {
    match status {
        504 | 408 => DEADLINE_EXCEEDED,
        413 => RESOURCE_EXHAUSTED,
        400 => INTERNAL,
        _ => UNAVAILABLE,
    }
}

/// The `grpc-message` of a code the proxy answers with.
pub(crate) fn message(code: u8) -> &'static str {
    match code {
        DEADLINE_EXCEEDED => "akit: the call took too long",
        RESOURCE_EXHAUSTED => "akit: the request is too large",
        INTERNAL => "akit: the request was malformed",
        _ => "akit: the upstream is unavailable",
    }
}

/// The answer to a call of `content_type` that got no response, for the HTTP status `status`:
/// a trailers-only response.
pub(crate) fn fail(status: u16, content_type: &str) -> Response {
    let code = code(status);
    let mut r = Response::new(200);
    r.headers.append("Content-Type", content_type);
    r.headers.append("Grpc-Status", code.to_string());
    r.headers.append("Grpc-Message", message(code));
    r
}

/// The trailers that end a call whose response failed after it began: `DEADLINE_EXCEEDED` when
/// its time ran out, else `UNAVAILABLE`.
pub(crate) fn broken(timed_out: bool) -> Headers {
    let code = if timed_out {
        DEADLINE_EXCEEDED
    } else {
        UNAVAILABLE
    };
    let mut t = Headers::new();
    t.append("Grpc-Status", code.to_string());
    t.append("Grpc-Message", message(code));
    t
}

/// How long the call `req` may take, and the `grpc-timeout` the upstream is told when it is not
/// the client's own: the route's `timeout`, or the call's `grpc-timeout` when that is shorter
/// (fit's `callTimeout`). Never zero: akio takes a zero timeout as none.
pub(crate) fn deadline(
    headers: &Headers,
    route: Option<Duration>,
) -> (Option<Duration>, Option<String>) {
    let mut values = headers.get_all("grpc-timeout");
    let call = match (values.next(), values.next()) {
        (Some(v), None) => parse_timeout(v.trim()),
        _ => None,
    };
    let least = Duration::from_millis(1);
    match (call, route) {
        (Some(d), r) if r.is_none_or(|r| d <= r) => (Some(d.max(least)), None),
        (_, Some(r)) => (Some(r.max(least)), Some(encode_timeout(r))),
        (_, None) => (None, None),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::time::Duration;

    use super::*;

    /// fit's `TestGRPCHelpers`.
    #[test]
    fn helpers() {
        for ct in [
            "application/grpc",
            "application/grpc+proto",
            "Application/GRPC+json",
            "application/grpc;charset=x",
        ] {
            assert!(content_type(ct), "{ct} is gRPC's");
        }
        for ct in [
            "application/grpc-web",
            "application/grpcx",
            "application/json",
            "",
            "application/grp",
        ] {
            assert!(!content_type(ct), "{ct} is not gRPC's");
        }
        for (v, want) in [
            ("300m", Some(Duration::from_millis(300))),
            ("5S", Some(Duration::from_secs(5))),
            ("1H", Some(Duration::from_secs(3600))),
            ("2M", Some(Duration::from_secs(120))),
            ("7u", Some(Duration::from_micros(7))),
            ("99999999n", Some(Duration::from_nanos(99_999_999))),
            ("99999999H", Some(Duration::from_secs(99_999_999 * 3600))),
            ("m", None),
            ("123456789S", None),
            ("10x", None),
            ("-1S", None),
            ("+1S", None),
            ("1é", None),
        ] {
            assert_eq!(parse_timeout(v), want, "{v}");
        }
        for (d, want) in [
            (Duration::from_secs(2), "2000000u"),
            (Duration::from_nanos(1), "1n"),
            (Duration::ZERO, "1n"),
            (Duration::from_secs(200 * 3600), "720000S"),
            (Duration::MAX, "99999999H"),
        ] {
            assert_eq!(encode_timeout(d), want, "{d:?}");
            if d < Duration::from_secs(99_999_999 * 3600) {
                assert!(parse_timeout(&encode_timeout(d)).unwrap() >= d, "{d:?}");
            }
        }
        assert_eq!(code(504), DEADLINE_EXCEEDED);
        assert_eq!(code(408), DEADLINE_EXCEEDED);
        assert_eq!(code(413), RESOURCE_EXHAUSTED);
        assert_eq!(code(400), INTERNAL);
        assert_eq!(code(502), UNAVAILABLE);
        assert_eq!(code(503), UNAVAILABLE);
    }

    #[test]
    fn failures() {
        let r = fail(504, "application/grpc+json");
        assert_eq!(r.status, 200);
        assert!(r.body.is_empty());
        assert_eq!(r.headers.get("content-type"), Some("application/grpc+json"));
        assert_eq!(r.headers.get("grpc-status"), Some("4"));
        assert_eq!(
            r.headers.get("grpc-message"),
            Some("akit: the call took too long")
        );
        let t = broken(false);
        assert_eq!(t.get("grpc-status"), Some("14"));
        assert_eq!(
            t.get("grpc-message"),
            Some("akit: the upstream is unavailable")
        );
        assert_eq!(broken(true).get("grpc-status"), Some("4"));
    }

    #[test]
    fn deadlines() {
        let with = |v: &[&str]| {
            let mut h = Headers::new();
            for v in v {
                h.append("grpc-timeout", *v);
            }
            h
        };
        let s = Duration::from_secs;
        // The call's own, shorter than the route's or alone: the client's field goes on.
        assert_eq!(deadline(&with(&["1S"]), Some(s(2))), (Some(s(1)), None));
        assert_eq!(deadline(&with(&["1S"]), None), (Some(s(1)), None));
        // The route's, shorter: the upstream is told it.
        assert_eq!(
            deadline(&with(&["5S"]), Some(s(2))),
            (Some(s(2)), Some("2000000u".to_owned()))
        );
        assert_eq!(
            deadline(&with(&[]), Some(s(2))),
            (Some(s(2)), Some("2000000u".to_owned()))
        );
        // A malformed or repeated field is none.
        assert_eq!(
            deadline(&with(&["5x"]), Some(s(2))),
            (Some(s(2)), Some("2000000u".to_owned()))
        );
        assert_eq!(deadline(&with(&["1S", "2S"]), None), (None, None));
        assert_eq!(deadline(&with(&[]), None), (None, None));
        // Never zero.
        assert_eq!(
            deadline(&with(&["0m"]), None),
            (Some(Duration::from_millis(1)), None)
        );
    }
}
