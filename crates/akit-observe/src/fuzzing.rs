//! Fuzz entry points (feature `fuzzing`; not a public API): the `fuzz/` cargo-fuzz project calls
//! them, and the tests below replay `fuzz/corpus/<target>` on stable. What a client sends that
//! ends up in a log line or a trace, and the zone files the log's time zone is read from.
#![allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::panic,
    clippy::expect_used
)]

use std::fmt::Write as _;
use std::net::{IpAddr, Ipv6Addr};
use std::time::Duration;

use akit_config::value::{self, Value};

use crate::format::{Format, Record, append};
use crate::time::Stamp;
use crate::trace::parse_traceparent;
use crate::{Zone, go_quote, json_string, logfmt};

/// `observe_access_log`: an access log line, whatever the request's strings hold, is one line:
/// in JSON a JSON object whose strings read back as what was logged, in text and line formats free of control characters. `data[..16]`: the time (any
/// seconds, as a clock or a test may give) and the zone's offset; then NUL-separated: the
/// method, host, target, user agent, referer, user, route, upstream.
pub fn observe_access_log(data: &[u8]) {
    if data.len() < 16 {
        return;
    }
    let secs = i64::from_be_bytes(data[..8].try_into().unwrap());
    let millis = u32::from_be_bytes(data[8..12].try_into().unwrap()) % 1000;
    // Any offset a zone file holds.
    let offset = i32::from_be_bytes(data[12..16].try_into().unwrap());
    let Ok(text) = std::str::from_utf8(&data[16..]) else {
        return;
    };
    let p: Vec<&str> = text.split('\0').collect();
    let part = |i: usize| p.get(i).copied().unwrap_or("");
    let r = Record {
        listener: "web",
        route: part(6),
        client: IpAddr::V6(Ipv6Addr::LOCALHOST),
        user: part(5),
        proto: "HTTP/1.1",
        method: part(0),
        host: part(1),
        uri: part(2),
        status: u16::from(data[15]) * 3,
        bytes_in: u64::from(data[14]),
        bytes_out: u64::MAX,
        duration: Duration::from_nanos(u64::from_be_bytes(data[..8].try_into().unwrap())),
        upstream: (!part(7).is_empty()).then(|| (part(7), Duration::from_micros(1500))),
        user_agent: part(3),
        referer: part(4),
    };
    let t = Stamp { secs, millis };
    for format in [Format::Json, Format::Text, Format::Line] {
        let mut b = Vec::new();
        append(&mut b, format, &r, t, offset);
        let line = std::str::from_utf8(&b).unwrap_or_else(|e| panic!("{format:?}: {e}"));
        let line = line
            .strip_suffix('\n')
            .unwrap_or_else(|| panic!("{format:?}: {line:?} does not end its line"));
        assert!(
            !line.contains(['\n', '\r']),
            "{format:?}: a line break in {line:?}"
        );
        if format == Format::Json {
            match value::parse(line) {
                Ok(Value::Object(_)) => {}
                other => panic!("not a JSON object ({other:?}): {line}"),
            }
            let v = value::parse(line).unwrap();
            for (k, want) in [("uri", r.uri), ("user_agent", r.user_agent)] {
                if let Some(Value::String(s)) = v.get(k) {
                    assert_eq!(s, want, "{k}");
                }
            }
        } else {
            assert!(
                !line.chars().any(char::is_control),
                "{format:?}: a control character in {line:?}"
            );
        }
    }
}

/// `b` with each byte that is not UTF-8 replaced by U+FFFD, as Go's `utf8.DecodeRune` takes
/// them one at a time (Rust's lossy decoding takes a broken sequence as one).
fn lossy(b: &[u8]) -> String {
    let mut s = String::new();
    for chunk in b.utf8_chunks() {
        s.push_str(chunk.valid());
        for _ in chunk.invalid() {
            s.push('\u{fffd}');
        }
    }
    s
}

/// `observe_escapes`: the escapes the log writes any bytes with: a JSON string that reads back
/// as the bytes (each one that is not UTF-8 as U+FFFD), a Go-quoted one and a logfmt value with
/// no control character or space left bare; and a `traceparent` read writes back as itself.
pub fn observe_escapes(data: &[u8]) {
    let mut b = Vec::new();
    json_string(&mut b, data);
    let s = std::str::from_utf8(&b).unwrap();
    match value::parse(s) {
        Ok(Value::String(got)) => assert_eq!(got, lossy(data)),
        other => panic!("{s:?} reads as {other:?}"),
    }
    let mut q = Vec::new();
    go_quote(&mut q, data);
    let q = std::str::from_utf8(&q).unwrap();
    assert!(q.starts_with('"') && q.ends_with('"') && q.len() >= 2);
    assert!(!q.chars().any(char::is_control), "{q:?}");
    if let Ok(text) = std::str::from_utf8(data) {
        let mut l = Vec::new();
        logfmt(&mut l, text);
        let l = std::str::from_utf8(&l).unwrap();
        assert!(!l.is_empty() && !l.chars().any(char::is_control), "{l:?}");
        assert!(l.starts_with('"') || !l.contains([' ', '=', '"']), "{l:?}");

        if let Some((trace, parent, sampled)) = parse_traceparent(text) {
            assert_ne!(trace, [0; 16]);
            assert_ne!(parent, [0; 8]);
            let mut again = String::from("00-");
            for x in trace {
                let _ = write!(again, "{x:02x}");
            }
            again.push('-');
            for x in parent {
                let _ = write!(again, "{x:02x}");
            }
            again.push_str(if sampled { "-01" } else { "-00" });
            let (t2, p2, s2) = parse_traceparent(&again).unwrap();
            assert_eq!((t2, p2, s2), (trace, parent, sampled), "{text:?}");
        }
    }
}

/// `observe_tzif`: a TZif file of any bytes reads as a zone, or not, without a panic; a zone
/// read gives an offset for any instant, the same each time. (RFC 8536 only asks offsets to be
/// within a day and two hours; the log writes any, its times saturating.)
pub fn observe_tzif(data: &[u8]) {
    let Some(z) = Zone::from_tzif("Fuzz/Zone", data) else {
        return;
    };
    assert_eq!(z.name(), "Fuzz/Zone");
    for t in [
        i64::MIN,
        -(1 << 59),
        -2_208_988_800,
        0,
        1_700_000_000,
        4_102_444_800,
        1 << 40,
        i64::MAX,
    ] {
        assert_eq!(z.offset(t), z.offset(t), "at {t}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `f` on every file of `fuzz/corpus/<target>`.
    fn replay(target: &str, f: fn(&[u8])) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fuzz/corpus")
            .join(target);
        let mut n = 0;
        for e in std::fs::read_dir(&dir).unwrap() {
            f(&std::fs::read(e.unwrap().path()).unwrap());
            n += 1;
        }
        assert!(n > 0, "{} is empty", dir.display());
    }

    #[test]
    fn corpus_observe_access_log() {
        replay("observe_access_log", observe_access_log);
    }

    #[test]
    fn corpus_observe_escapes() {
        replay("observe_escapes", observe_escapes);
    }

    #[test]
    fn corpus_observe_tzif() {
        replay("observe_tzif", observe_tzif);
    }
}
