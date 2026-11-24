//! A request's line in fit's formats: `json`, `text` and `line`, with the same fields in the same
//! order, as fit's `observe/accesslog.go` writes them.

use std::net::IpAddr;
use std::time::Duration;

use crate::time::{self, Stamp};

/// How lines are written: `access_log.format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    /// A JSON object a line.
    #[default]
    Json,
    /// `time=… level=… msg=access` and the fields as logfmt.
    Text,
    /// `2006-01-02 15:04:05.000 INFO  access` and the fields as logfmt.
    Line,
}

/// What a request's line says: borrowed from the request and response it is written for.
#[derive(Debug, Clone, Copy)]
pub struct Record<'a> {
    /// The listener the request came in on.
    pub listener: &'a str,
    /// The route that took it; empty (and left out of the line) when none did.
    pub route: &'a str,
    /// The client's address.
    pub client: IpAddr,
    /// Who the request was authenticated as; empty (and left out) when it was not.
    pub user: &'a str,
    /// `HTTP/1.1`, `HTTP/2.0`, …, as Go's `Request.Proto`.
    pub proto: &'a str,
    /// The method.
    pub method: &'a str,
    /// The `Host` the request named.
    pub host: &'a str,
    /// The request target as sent.
    pub uri: &'a str,
    /// The response's status.
    pub status: u16,
    /// The bytes of the request's body.
    pub bytes_in: u64,
    /// The bytes of the response's body.
    pub bytes_out: u64,
    /// From the request's dispatch to its response.
    pub duration: Duration,
    /// The upstream address it was proxied to, and how long the upstream took; `None` (and left
    /// out) for a request that was not proxied.
    pub upstream: Option<(&'a str, Duration)>,
    /// The `User-Agent` field.
    pub user_agent: &'a str,
    /// The `Referer` field.
    pub referer: &'a str,
}

/// The level of a request answered with `status`, as fib's package log writes levels.
fn level(status: u16) -> &'static str {
    match status {
        500.. => "ERROR",
        400.. => "WARN",
        _ => "INFO",
    }
}

/// Appends the line for `r` at time `t`, `offset` seconds east of UTC, in `format`.
pub fn append(b: &mut Vec<u8>, format: Format, r: &Record<'_>, t: Stamp, offset: i32) {
    match format {
        Format::Json => {
            b.extend_from_slice(b"{\"time\":\"");
            time::rfc3339(b, t, offset);
            b.push(b'"');
            json_fields(b, r);
        }
        Format::Text => {
            b.extend_from_slice(b"time=");
            time::rfc3339(b, t, offset);
            b.extend_from_slice(b" level=");
            b.extend_from_slice(level(r.status).as_bytes());
            b.extend_from_slice(b" msg=access");
            text_fields(b, r);
        }
        Format::Line => {
            time::line(b, t, offset);
            b.push(b' ');
            let l = level(r.status);
            b.extend_from_slice(l.as_bytes());
            // Padded to five, then a space.
            for _ in l.len()..6 {
                b.push(b' ');
            }
            b.extend_from_slice(b"access");
            text_fields(b, r);
        }
    }
}

fn json_fields(b: &mut Vec<u8>, r: &Record<'_>) {
    let field = |b: &mut Vec<u8>, k: &str, v: &str| {
        b.extend_from_slice(b",\"");
        b.extend_from_slice(k.as_bytes());
        b.extend_from_slice(b"\":");
        json_string(b, v.as_bytes());
    };
    field(b, "listener", r.listener);
    if !r.route.is_empty() {
        field(b, "route", r.route);
    }
    b.extend_from_slice(b",\"client\":\"");
    b.extend_from_slice(r.client.to_string().as_bytes());
    b.push(b'"');
    if !r.user.is_empty() {
        field(b, "user", r.user);
    }
    field(b, "proto", r.proto);
    field(b, "method", r.method);
    field(b, "host", r.host);
    field(b, "uri", r.uri);
    numbers(b, r, b",\"", b"\":");
    if let Some((addr, took)) = r.upstream {
        field(b, "upstream", addr);
        b.extend_from_slice(b",\"upstream_duration\":");
        time::seconds(b, took);
    }
    field(b, "user_agent", r.user_agent);
    field(b, "referer", r.referer);
    b.extend_from_slice(b"}\n");
}

/// `status`, `bytes_in`, `bytes_out` and `duration`, each after `pre` and its name and `mid`.
fn numbers(b: &mut Vec<u8>, r: &Record<'_>, pre: &[u8], mid: &[u8]) {
    for (k, v) in [
        ("status", u64::from(r.status)),
        ("bytes_in", r.bytes_in),
        ("bytes_out", r.bytes_out),
    ] {
        b.extend_from_slice(pre);
        b.extend_from_slice(k.as_bytes());
        b.extend_from_slice(mid);
        b.extend_from_slice(v.to_string().as_bytes());
    }
    b.extend_from_slice(pre);
    b.extend_from_slice(b"duration");
    b.extend_from_slice(mid);
    time::seconds(b, r.duration);
}

fn text_fields(b: &mut Vec<u8>, r: &Record<'_>) {
    let field = |b: &mut Vec<u8>, k: &str, v: &str| {
        b.push(b' ');
        b.extend_from_slice(k.as_bytes());
        b.push(b'=');
        logfmt(b, v);
    };
    field(b, "listener", r.listener);
    if !r.route.is_empty() {
        field(b, "route", r.route);
    }
    b.extend_from_slice(b" client=");
    b.extend_from_slice(r.client.to_string().as_bytes());
    if !r.user.is_empty() {
        field(b, "user", r.user);
    }
    field(b, "proto", r.proto);
    field(b, "method", r.method);
    field(b, "host", r.host);
    field(b, "uri", r.uri);
    numbers(b, r, b" ", b"=");
    if let Some((addr, took)) = r.upstream {
        field(b, "upstream", addr);
        b.extend_from_slice(b" upstream_duration=");
        time::seconds(b, took);
    }
    field(b, "user_agent", r.user_agent);
    field(b, "referer", r.referer);
    b.push(b'\n');
}

/// The lowercase hex digit of `n`'s low four bits.
fn hex(n: u32) -> u8 {
    char::from_digit(n & 0xf, 16)
        .and_then(|c| u8::try_from(c).ok())
        .unwrap_or(b'0')
}

/// The character `s` starts with and its length in bytes, or `None` and 1 when its first byte
/// does not start a valid UTF-8 character (or `s` is empty).
fn first_char(s: &[u8]) -> (Option<char>, usize) {
    for n in 1..=s.len().min(4) {
        if let Some(Ok(st)) = s.get(..n).map(std::str::from_utf8) {
            return (st.chars().next(), n);
        }
    }
    (None, 1)
}

/// Appends `s` as a JSON string, as fit's `appendString`: printable ASCII as it is but the quote
/// and the backslash, `\n`, `\r` and `\t` escaped so, other control bytes as `\u00XX`, valid
/// UTF-8 as it is, and each byte that is not valid UTF-8 as U+FFFD, so that every line is valid
/// JSON whatever a client sent.
pub fn json_string(b: &mut Vec<u8>, s: &[u8]) {
    b.push(b'"');
    let mut rest = s;
    loop {
        let plain = rest
            .iter()
            .take_while(|&&c| (0x20..0x80).contains(&c) && c != b'"' && c != b'\\')
            .count();
        let (head, tail) = rest.split_at_checked(plain).unwrap_or((rest, &[]));
        b.extend_from_slice(head);
        rest = tail;
        if rest.is_empty() {
            break;
        }
        let (c, n) = first_char(rest);
        match c {
            Some(c @ ('"' | '\\')) => b.extend_from_slice(&[b'\\', c as u8]),
            Some('\n') => b.extend_from_slice(b"\\n"),
            Some('\r') => b.extend_from_slice(b"\\r"),
            Some('\t') => b.extend_from_slice(b"\\t"),
            Some(c) if c.is_ascii() => {
                let v = u32::from(c);
                b.extend_from_slice(&[b'\\', b'u', b'0', b'0', hex(v >> 4), hex(v)]);
            }
            Some(c) => b.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
            None => b.extend_from_slice("\u{fffd}".as_bytes()),
        }
        rest = rest.get(n..).unwrap_or_default();
    }
    b.push(b'"');
}

/// Appends `s` as a logfmt value, as fit's `appendLogfmt`: as it is when it is printable ASCII
/// without the space, the quote and the equals sign, or quoted, as Go's `strconv.Quote` quotes
/// it, when it is empty or has another byte, so that no control character, nor a byte that is not
/// valid UTF-8, is written as it is.
pub fn logfmt(b: &mut Vec<u8>, s: &str) {
    let bare = !s.is_empty()
        && s.bytes()
            .all(|c| (0x21..0x7f).contains(&c) && c != b'"' && c != b'=');
    if bare {
        b.extend_from_slice(s.as_bytes());
    } else {
        go_quote(b, s.as_bytes());
    }
}

/// Appends `s` quoted as Go's `strconv.Quote` quotes it: `\a \b \f \n \r \t \v \\ \"` escaped
/// so, other ASCII control bytes and each byte that is not valid UTF-8 as `\xXX`, and characters
/// past ASCII as they are unless they are not printable (C1 controls, the separators U+2028 and
/// U+2029, the byte order mark and other format characters), which are written `\uXXXX`.
pub fn go_quote(b: &mut Vec<u8>, s: &[u8]) {
    b.push(b'"');
    let mut rest = s;
    while let Some(&byte) = rest.first() {
        let (c, n) = first_char(rest);
        match c {
            Some('\u{7}') => b.extend_from_slice(b"\\a"),
            Some('\u{8}') => b.extend_from_slice(b"\\b"),
            Some('\u{c}') => b.extend_from_slice(b"\\f"),
            Some('\n') => b.extend_from_slice(b"\\n"),
            Some('\r') => b.extend_from_slice(b"\\r"),
            Some('\t') => b.extend_from_slice(b"\\t"),
            Some('\u{b}') => b.extend_from_slice(b"\\v"),
            Some(c @ ('\\' | '"')) => b.extend_from_slice(&[b'\\', c as u8]),
            Some(c @ ' '..='~') => b.push(c as u8),
            Some(c) if c.is_ascii() => hex_byte(b, byte),
            Some(c) if printable(c) => b.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
            Some(c) => {
                let v = u32::from(c);
                if v <= 0xffff {
                    b.extend_from_slice(b"\\u");
                    for shift in [12, 8, 4, 0] {
                        b.push(hex(v >> shift));
                    }
                } else {
                    b.extend_from_slice(b"\\U");
                    for shift in [28, 24, 20, 16, 12, 8, 4, 0] {
                        b.push(hex(v >> shift));
                    }
                }
            }
            None => hex_byte(b, byte),
        }
        rest = rest.get(n..).unwrap_or_default();
    }
    b.push(b'"');
}

fn hex_byte(b: &mut Vec<u8>, c: u8) {
    let v = u32::from(c);
    b.extend_from_slice(&[b'\\', b'x', hex(v >> 4), hex(v)]);
}

/// Whether Go's `unicode.IsPrint` holds for `c` (past ASCII), as near as std can tell without
/// Unicode's tables: not a control, a separator other than the space, nor a format character
/// among the common ones.
fn printable(c: char) -> bool {
    !(c.is_control()
        || c.is_whitespace()
        || matches!(
            c,
            '\u{ad}'
                | '\u{600}'..='\u{605}'
                | '\u{61c}'
                | '\u{6dd}'
                | '\u{70f}'
                | '\u{180e}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{feff}'
                | '\u{fff9}'..='\u{fffb}'
                | '\u{e000}'..='\u{f8ff}'
                | '\u{f0000}'..
        ))
}

#[cfg(test)]
mod tests {
    use super::{go_quote, json_string, logfmt};

    fn s(f: impl FnOnce(&mut Vec<u8>)) -> String {
        let mut b = Vec::new();
        f(&mut b);
        String::from_utf8(b).unwrap_or_else(|e| format!("not UTF-8: {e}"))
    }

    #[test]
    fn json_strings() {
        for (input, want) in [
            (&b""[..], r#""""#),
            (b"/a?b=c", r#""/a?b=c""#),
            (b"q\"b\\", r#""q\"b\\""#),
            (b"\n\r\t\x01\x7f", "\"\\n\\r\\t\\u0001\x7f\""),
            ("中文".as_bytes(), "\"中文\""),
            (b"a\xffb\xe4\xb8", "\"a\u{fffd}b\u{fffd}\u{fffd}\""),
        ] {
            assert_eq!(s(|b| json_string(b, input)), want, "{input:?}");
        }
    }

    #[test]
    fn logfmt_values() {
        for (input, want) in [
            ("", r#""""#),
            ("curl/8.9.1", "curl/8.9.1"),
            ("a b", r#""a b""#),
            ("k=v", r#""k=v""#),
            ("say \"hi\"", r#""say \"hi\"""#),
            (
                "\x07\x08\x0c\n\r\t\x0b\x01\x7f",
                r#""\a\b\f\n\r\t\v\x01\x7f""#,
            ),
            ("中", "\"中\""),
            ("\u{85}\u{2028}\u{feff}", "\"\\u0085\\u2028\\ufeff\""),
        ] {
            assert_eq!(s(|b| logfmt(b, input)), want, "{input:?}");
        }
        assert_eq!(s(|b| go_quote(b, b"a\xffb")), r#""a\xffb""#);
    }
}
