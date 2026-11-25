//! What the converters take from Go's standard library, as fit's converters use it, so that
//! they write what fit's write byte for byte: `encoding/json`'s `Marshal` ([`J`]),
//! `strconv`'s `Quote`, `Atoi`, `ParseFloat` and `FormatFloat`, `net`'s `SplitHostPort` and
//! `JoinHostPort`, `strings.NewReplacer` and `strings.Trim` with a cutset.

use std::collections::BTreeMap;

use crate::value::write_go_string;

/// A value as Go's `json.Marshal` writes it: a struct's fields in their order ([`J::Obj`]), a
/// map's sorted by key ([`J::Map`]).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum J {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Arr(Vec<J>),
    /// A struct: its fields in order, those `omitempty` leaves out already left out.
    Obj(Vec<(&'static str, J)>),
    /// A map: its keys in byte order, as Go sorts them.
    Map(BTreeMap<String, J>),
}

/// A Go `map[string]any`.
pub(crate) type Params = BTreeMap<String, J>;

impl J {
    pub(crate) fn str(s: &str) -> J {
        J::Str(s.to_owned())
    }

    pub(crate) fn strs<S: AsRef<str>>(v: &[S]) -> J {
        J::Arr(v.iter().map(|s| J::Str(s.as_ref().to_owned())).collect())
    }

    /// The value as `json.Marshal` writes it, or its error: a float that is not a number or is
    /// infinite has no JSON.
    pub(crate) fn marshal(&self) -> Result<String, String> {
        let mut b = String::new();
        self.write(&mut b)?;
        Ok(b)
    }

    fn write(&self, b: &mut String) -> Result<(), String> {
        match self {
            J::Null => b.push_str("null"),
            J::Bool(v) => b.push_str(if *v { "true" } else { "false" }),
            J::Int(n) => b.push_str(&n.to_string()),
            J::Float(f) => b.push_str(&json_float(*f)?),
            J::Str(s) => write_go_string(b, s, true),
            J::Arr(a) => {
                b.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        b.push(',');
                    }
                    v.write(b)?;
                }
                b.push(']');
            }
            J::Obj(ms) => {
                b.push('{');
                for (i, (k, v)) in ms.iter().enumerate() {
                    if i > 0 {
                        b.push(',');
                    }
                    write_go_string(b, k, true);
                    b.push(':');
                    v.write(b)?;
                }
                b.push('}');
            }
            J::Map(ms) => {
                b.push('{');
                for (i, (k, v)) in ms.iter().enumerate() {
                    if i > 0 {
                        b.push(',');
                    }
                    write_go_string(b, k, true);
                    b.push(':');
                    v.write(b)?;
                }
                b.push('}');
            }
        }
        Ok(())
    }
}

/// A struct being written: its fields, `omitempty` ones left out when empty.
#[derive(Default)]
pub(crate) struct Fields(Vec<(&'static str, J)>);

impl Fields {
    pub(crate) fn new() -> Fields {
        Fields(Vec::new())
    }

    /// A field always written.
    pub(crate) fn put(mut self, k: &'static str, v: J) -> Fields {
        self.0.push((k, v));
        self
    }

    /// A string field with `omitempty`.
    pub(crate) fn str(self, k: &'static str, v: &str) -> Fields {
        if v.is_empty() {
            self
        } else {
            self.put(k, J::str(v))
        }
    }

    /// A list of strings with `omitempty`.
    pub(crate) fn strs<S: AsRef<str>>(self, k: &'static str, v: &[S]) -> Fields {
        if v.is_empty() {
            self
        } else {
            self.put(k, J::strs(v))
        }
    }

    /// A number with `omitempty`.
    pub(crate) fn int(self, k: &'static str, v: i64) -> Fields {
        if v == 0 { self } else { self.put(k, J::Int(v)) }
    }

    /// A boolean with `omitempty`.
    pub(crate) fn bool(self, k: &'static str, v: bool) -> Fields {
        if v { self.put(k, J::Bool(true)) } else { self }
    }

    /// A pointer, a list or a map with `omitempty`: written when there is one.
    pub(crate) fn opt(self, k: &'static str, v: Option<J>) -> Fields {
        match v {
            Some(v) => self.put(k, v),
            None => self,
        }
    }

    /// A map of params with `omitempty`.
    pub(crate) fn params(self, k: &'static str, v: &Params) -> Fields {
        if v.is_empty() {
            self
        } else {
            self.put(k, J::Map(v.clone()))
        }
    }

    pub(crate) fn done(self) -> J {
        J::Obj(self.0)
    }
}

/// A float as `encoding/json` writes it: Go's shortest form, positional between 1e-6 and 1e21,
/// with an exponent outside (`1e-7`, `1e+21`).
fn json_float(f: f64) -> Result<String, String> {
    if f.is_nan() {
        return Err("json: unsupported value: NaN".into());
    }
    if f.is_infinite() {
        let s = if f > 0.0 { "+Inf" } else { "-Inf" };
        return Err(format!("json: unsupported value: {s}"));
    }
    let abs = f.abs();
    if abs != 0.0 && !(1e-6..1e21).contains(&abs) {
        let e = format!("{f:e}");
        let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
        let exp: i32 = exp.parse().unwrap_or(0);
        return Ok(if exp < 0 {
            format!("{mant}e-{}", exp.unsigned_abs())
        } else {
            format!("{mant}e+{exp:02}")
        });
    }
    Ok(format!("{f}"))
}

/// Go's `strconv.FormatFloat(f, 'f', -1, 64)`.
pub(crate) fn format_float(f: f64) -> String {
    if f.is_nan() {
        "NaN".into()
    } else if f.is_infinite() {
        if f > 0.0 {
            "+Inf".into()
        } else {
            "-Inf".into()
        }
    } else {
        format!("{f}")
    }
}

/// Go's `strconv.Quote`.
pub(crate) fn quote(s: &str) -> String {
    akit_router::go_quote(s.as_bytes())
}

/// Go's `strconv.Atoi` on a 64-bit machine: an optional sign and decimal digits. Past the
/// range, Go returns the bound with an error: `Err(bound)`.
pub(crate) fn atoi(s: &str) -> Result<i64, Option<i64>> {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(None);
    }
    s.parse::<i64>().map_err(|_| {
        Some(if s.starts_with('-') {
            i64::MIN
        } else {
            i64::MAX
        })
    })
}

/// `strconv.Atoi(s)` with its value read only when there is no error.
pub(crate) fn atoi_ok(s: &str) -> Option<i64> {
    atoi(s).ok()
}

/// Go's `strconv.ParseFloat(s, 64)`.
pub(crate) fn parse_float(s: &str) -> Option<f64> {
    let (neg, body) = match s.as_bytes().first() {
        Some(b'-') => (true, s.get(1..)?),
        Some(b'+') => (false, s.get(1..)?),
        _ => (false, s),
    };
    let lower = body.to_ascii_lowercase();
    if lower == "inf" || lower == "infinity" {
        return Some(if neg {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        });
    }
    if lower == "nan" {
        return (s.len() == 3).then_some(f64::NAN);
    }
    if !underscore_ok(s) {
        return None;
    }
    let clean: String = body.chars().filter(|c| *c != '_').collect();
    let v = if let Some(hex) = clean
        .strip_prefix("0x")
        .or_else(|| clean.strip_prefix("0X"))
    {
        parse_hex_float(hex)?
    } else {
        if clean.is_empty()
            || !clean
                .bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
        {
            return None;
        }
        let v: f64 = clean.parse().ok()?;
        if v.is_infinite() {
            return None;
        }
        v
    };
    Some(if neg { -v } else { v })
}

/// A hexadecimal float after its `0x`: digits, an optional point, and a `p` exponent, which Go
/// requires.
fn parse_hex_float(s: &str) -> Option<f64> {
    let (mant, exp) = s.split_once(['p', 'P'])?;
    let exp: i32 = exp.parse().ok()?;
    let mut v = 0f64;
    let mut frac_digits = 0i32;
    let mut seen_point = false;
    let mut digits = 0;
    for c in mant.chars() {
        if c == '.' && !seen_point {
            seen_point = true;
            continue;
        }
        let d = c.to_digit(16)?;
        v = v * 16.0 + f64::from(d);
        digits += 1;
        if seen_point {
            frac_digits = frac_digits.saturating_add(1);
        }
    }
    if digits == 0 {
        return None;
    }
    let v = v * 2f64.powi(exp.saturating_sub(frac_digits.saturating_mul(4)));
    (!v.is_infinite()).then_some(v)
}

/// Go's `underscoreOK`: underscores only between digits, or after a base prefix.
fn underscore_ok(s: &str) -> bool {
    let b = s.as_bytes();
    let b = b
        .strip_prefix(b"-")
        .or_else(|| b.strip_prefix(b"+"))
        .unwrap_or(b);
    let mut saw = b'^';
    let mut i = 0;
    let mut hex = false;
    if b.len() >= 2
        && b.first() == Some(&b'0')
        && matches!(
            b.get(1).map(u8::to_ascii_lowercase),
            Some(b'b' | b'o' | b'x')
        )
    {
        i = 2;
        saw = b'0';
        hex = b.get(1).map(u8::to_ascii_lowercase) == Some(b'x');
    }
    while let Some(&c) = b.get(i) {
        i += 1;
        let l = c.to_ascii_lowercase();
        if c.is_ascii_digit() || hex && (b'a'..=b'f').contains(&l) {
            saw = b'0';
            continue;
        }
        if c == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
            continue;
        }
        if saw == b'_' {
            return false;
        }
        saw = b'!';
    }
    saw != b'_'
}

/// Go's `net.SplitHostPort`.
pub(crate) fn split_host_port(hostport: &str) -> Option<(&str, &str)> {
    let i = hostport.rfind(':')?;
    let (host, j, k) = if hostport.starts_with('[') {
        let end = hostport.find(']')?;
        if end + 1 != i {
            return None;
        }
        (hostport.get(1..end)?, 1, end + 1)
    } else {
        let host = hostport.get(..i)?;
        if host.contains(':') {
            return None;
        }
        (host, 0, 0)
    };
    if hostport.get(j..)?.contains('[') || hostport.get(k..)?.contains(']') {
        return None;
    }
    Some((host, hostport.get(i + 1..)?))
}

/// Go's `net.JoinHostPort`.
pub(crate) fn join_host_port(host: &str, port: &str) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Go's `strings.NewReplacer(pairs...).Replace(s)`: at each place, the first pair (in their
/// order) whose old text is there is replaced; the replacements do not overlap.
pub(crate) fn replace(s: &str, pairs: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    'outer: while i < s.len() {
        let rest = s.get(i..).unwrap_or_default();
        for (old, new) in pairs {
            if !old.is_empty() && rest.starts_with(old) {
                out.push_str(new);
                i += old.len();
                continue 'outer;
            }
        }
        let Some(c) = rest.chars().next() else { break };
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// Go's `strings.TrimLeft(s, cutset)` for an ASCII cutset.
pub(crate) fn trim_left<'a>(s: &'a str, cutset: &str) -> &'a str {
    s.trim_start_matches(|c| cutset.contains(c))
}

/// Go's `strings.TrimRight(s, cutset)` for an ASCII cutset.
pub(crate) fn trim_right<'a>(s: &'a str, cutset: &str) -> &'a str {
    s.trim_end_matches(|c| cutset.contains(c))
}

/// Go's `strings.Trim(s, cutset)` for an ASCII cutset.
pub(crate) fn trim<'a>(s: &'a str, cutset: &str) -> &'a str {
    trim_right(trim_left(s, cutset), cutset)
}

/// Go's `base64.StdEncoding.DecodeString`: padded, line breaks ignored.
pub(crate) fn base64_std(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(u32::from(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        }))
    }
    let b: Vec<u8> = s.bytes().filter(|c| *c != b'\r' && *c != b'\n').collect();
    if b.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 4 * 3);
    for (i, q) in b.chunks(4).enumerate() {
        let last = (i + 1) * 4 == b.len();
        let pad = q.iter().rev().take_while(|c| **c == b'=').count();
        if pad > 2 || pad > 0 && !last {
            return None;
        }
        let mut n = 0u32;
        for (j, &c) in q.iter().enumerate() {
            let v = if j >= 4 - pad { 0 } else { val(c)? };
            n = n << 6 | v;
        }
        let bytes = n.to_be_bytes();
        out.extend_from_slice(bytes.get(1..4 - pad).unwrap_or_default());
    }
    Some(out)
}

/// Bytes as Go's `encoding/json` writes a string of them: each byte of an invalid sequence as
/// U+FFFD.
pub(crate) fn lossy(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len());
    for chunk in b.utf8_chunks() {
        out.push_str(chunk.valid());
        for _ in chunk.invalid() {
            out.push('\u{fffd}');
        }
    }
    out
}

/// Go's `regexp.QuoteMeta`.
pub(crate) fn quote_meta(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if r"\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The line a warning, `line N: ...`, is about; 0 when it says none, as fit's `warningLine`.
pub(crate) fn warning_line(w: &str) -> i64 {
    let head = w.split(':').next().unwrap_or_default();
    atoi_ok(head.strip_prefix("line ").unwrap_or(head)).unwrap_or(0)
}

/// Sorts warnings by their lines, keeping the order of those on one line.
pub(crate) fn sort_warnings(ws: &mut [String]) {
    ws.sort_by_key(|w| warning_line(w));
}

/// A route name made of `site` and `hint`, `hint` trimmed of `cut`, with what route names may
/// not hold replaced, at most 60 bytes, unique among `names`, as fit's converters make one.
pub(crate) fn route_name(
    names: &mut std::collections::HashSet<String>,
    site: &str,
    hint: &str,
    cut: &str,
) -> String {
    let mut b = String::new();
    for c in format!("{site}-{}", trim(hint, cut)).chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
            b.push(c);
        } else {
            b.push('_');
        }
    }
    let mut name = trim_right(trim_left(&b, "._-"), "_-").to_owned();
    if name.is_empty() {
        name = "route".into();
    }
    name.truncate(60);
    let mut unique = name.clone();
    let mut i = 2u64;
    while names.contains(&unique) {
        unique = format!("{name}-{i}");
        i = i.saturating_add(1);
    }
    names.insert(unique.clone());
    unique
}

/// Go's `strings.NewReplacer(".", "_", ":", "_", "[", "", "]", "")` and, with `slash`, `"/"` to
/// `"_"`: an address as a name.
pub(crate) fn addr_name(s: &str, slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '.' | ':' => out.push('_'),
            '/' if slash => out.push('_'),
            '[' | ']' => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_as_go() {
        let cases = [
            (1.0 / 60.0, "0.016666666666666666"),
            (1e-7, "1e-7"),
            (1e21, "1e+21"),
            (123_456_789e15, "1.23456789e+23"),
            (0.5, "0.5"),
            (5.0, "5"),
            (2.5e-10, "2.5e-10"),
        ];
        for (f, want) in cases {
            assert_eq!(json_float(f).unwrap(), want);
        }
        assert_eq!(
            J::Float(f64::NAN).marshal().unwrap_err(),
            "json: unsupported value: NaN"
        );
    }

    #[test]
    fn parse_float_as_go() {
        assert_eq!(parse_float("1e-400"), Some(0.0));
        assert_eq!(parse_float("+nan"), None);
        assert!(parse_float("nan").unwrap().is_nan());
        assert_eq!(parse_float("1_0"), Some(10.0));
        assert_eq!(parse_float("_1"), None);
        assert_eq!(parse_float("0x1p-2"), Some(0.25));
        assert_eq!(parse_float("0x1"), None);
        assert_eq!(parse_float("+Infinity"), Some(f64::INFINITY));
        assert_eq!(parse_float("1e400"), None);
        assert_eq!(parse_float("5."), Some(5.0));
        assert_eq!(parse_float(".5"), Some(0.5));
    }

    #[test]
    fn atoi_as_go() {
        assert_eq!(atoi("+5"), Ok(5));
        assert_eq!(atoi("99999999999999999999"), Err(Some(i64::MAX)));
        assert_eq!(atoi("-99999999999999999999"), Err(Some(i64::MIN)));
        assert_eq!(atoi("0x10"), Err(None));
        assert_eq!(atoi("1_0"), Err(None));
    }

    #[test]
    fn host_port_as_go() {
        assert_eq!(split_host_port("a:1"), Some(("a", "1")));
        assert_eq!(split_host_port("[::1]:80"), Some(("::1", "80")));
        assert_eq!(split_host_port(":80"), Some(("", "80")));
        assert_eq!(split_host_port("::1"), None);
        assert_eq!(split_host_port("a"), None);
        assert_eq!(split_host_port("[::1]"), None);
        assert_eq!(split_host_port("[a]b:1"), None);
        assert_eq!(join_host_port("::1", "80"), "[::1]:80");
    }

    #[test]
    fn base64_as_go() {
        assert_eq!(base64_std("JDJh").as_deref(), Some(&b"$2a"[..]));
        assert_eq!(base64_std("YQ==").as_deref(), Some(&b"a"[..]));
        assert_eq!(base64_std("YQ"), None);
        assert_eq!(base64_std("Y=Q="), None);
        assert_eq!(base64_std("YQ==\nYQ=="), None);
        assert_eq!(lossy(b"a\xe2\x82b"), "a\u{fffd}\u{fffd}b");
    }

    #[test]
    fn replacer_as_go() {
        let pairs = [
            ("$request_uri", "{uri}"),
            ("$uri", "{path}"),
            ("$host", "{host}"),
        ];
        assert_eq!(
            replace("$request_uri$host$uri", &pairs),
            "{uri}{host}{path}"
        );
    }
}
