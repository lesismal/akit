//! A strict JSON reader that keeps where each value is: JSON as RFC 8259 writes it, plus the two
//! things fit's JSON configs may hold, `//` and `/* */` comments and a trailing comma after the
//! last element of an array or object. An object with the same key twice is refused rather than
//! one of them silently kept, as fit refuses it.
//!
//! Offsets are bounded by the input's length, so the position arithmetic below cannot overflow.
#![allow(clippy::arithmetic_side_effects)]

/// How deep arrays and objects may nest. fit's configs nest a handful of levels; this bounds
/// the parser's recursion on hostile input.
pub const MAX_DEPTH: usize = 128;

/// A parsed value and the byte offset it starts at.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Node {
    pub(crate) off: usize,
    pub(crate) kind: Kind,
}

/// A JSON value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Kind {
    Null,
    Bool(bool),
    /// A number as written, checked against JSON's grammar; converted by the field that reads it.
    Number(String),
    Str(String),
    Array(Vec<Node>),
    Object(Vec<Member>),
}

/// One member of an object, with where its key is.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Member {
    pub(crate) key: String,
    pub(crate) key_off: usize,
    pub(crate) value: Node,
}

impl Kind {
    /// What the value is, for messages such as "expected an object, got a string".
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Kind::Null => "null",
            Kind::Bool(_) => "a boolean",
            Kind::Number(_) => "a number",
            Kind::Str(_) => "a string",
            Kind::Array(_) => "an array",
            Kind::Object(_) => "an object",
        }
    }
}

/// Where parsing failed and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParseError {
    pub(crate) off: usize,
    pub(crate) msg: String,
}

/// Parses `src` as one JSON value with comments and trailing commas allowed.
pub(crate) fn parse(src: &str) -> Result<Node, ParseError> {
    let mut p = Parser {
        b: src.as_bytes(),
        src,
        i: 0,
        depth: 0,
    };
    p.skip()?;
    if p.i >= p.b.len() {
        return Err(p.err("invalid JSON: the document is empty"));
    }
    let v = p.value()?;
    p.skip()?;
    if p.i < p.b.len() {
        return Err(p.err("invalid JSON: more after the value"));
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    src: &'a str,
    i: usize,
    depth: usize,
}

impl Parser<'_> {
    fn err(&self, msg: impl Into<String>) -> ParseError {
        ParseError {
            off: self.i,
            msg: msg.into(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    /// Skips white space and comments.
    fn skip(&mut self) -> Result<(), ParseError> {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\n' | b'\r') => self.i += 1,
                Some(b'/') => match self.b.get(self.i + 1) {
                    Some(b'/') => {
                        while let Some(c) = self.peek() {
                            if c == b'\n' {
                                break;
                            }
                            self.i += 1;
                        }
                    }
                    Some(b'*') => {
                        let start = self.i;
                        let rest = self.b.get(self.i + 2..).unwrap_or_default();
                        match rest.windows(2).position(|w| w == b"*/") {
                            Some(end) => self.i += 2 + end + 2,
                            None => {
                                return Err(ParseError {
                                    off: start,
                                    msg: "invalid JSON: a /* comment that does not end".into(),
                                });
                            }
                        }
                    }
                    _ => return Err(self.err("invalid JSON: unexpected '/'")),
                },
                _ => return Ok(()),
            }
        }
    }

    fn value(&mut self) -> Result<Node, ParseError> {
        let off = self.i;
        let kind = match self.peek() {
            Some(b'{') => self.nested(Self::object)?,
            Some(b'[') => self.nested(Self::array)?,
            Some(b'"') => Kind::Str(self.string()?),
            Some(b't') => self.word("true", Kind::Bool(true))?,
            Some(b'f') => self.word("false", Kind::Bool(false))?,
            Some(b'n') => self.word("null", Kind::Null)?,
            Some(b'-' | b'0'..=b'9') => Kind::Number(self.number()?),
            Some(c) => {
                return Err(self.err(format!(
                    "invalid JSON: unexpected {}",
                    describe(c, self.src.get(self.i..))
                )));
            }
            None => return Err(self.err("invalid JSON: the document ends early")),
        };
        Ok(Node { off, kind })
    }

    fn nested(&mut self, f: fn(&mut Self) -> Result<Kind, ParseError>) -> Result<Kind, ParseError> {
        if self.depth >= MAX_DEPTH {
            return Err(self.err(format!("invalid JSON: nested deeper than {MAX_DEPTH}")));
        }
        self.depth += 1;
        let r = f(self);
        self.depth -= 1;
        r
    }

    fn word(&mut self, w: &str, kind: Kind) -> Result<Kind, ParseError> {
        if self.b.get(self.i..self.i + w.len()) == Some(w.as_bytes()) {
            self.i += w.len();
            Ok(kind)
        } else {
            Err(self.err(format!(
                "invalid JSON: unexpected {}",
                describe(self.peek().unwrap_or(0), self.src.get(self.i..))
            )))
        }
    }

    fn array(&mut self) -> Result<Kind, ParseError> {
        self.i += 1; // [
        let mut items = Vec::new();
        loop {
            self.skip()?;
            match self.peek() {
                Some(b']') => {
                    self.i += 1;
                    return Ok(Kind::Array(items));
                }
                Some(b',') => return Err(self.err("invalid JSON: a comma with no value before it")),
                _ => {}
            }
            items.push(self.value()?);
            self.skip()?;
            match self.peek() {
                Some(b',') => self.i += 1, // a trailing comma is allowed: ']' may follow
                Some(b']') => {}
                None => return Err(self.err("invalid JSON: an array that does not end")),
                Some(_) => return Err(self.err("invalid JSON: expected ',' or ']' after a value")),
            }
        }
    }

    fn object(&mut self) -> Result<Kind, ParseError> {
        self.i += 1; // {
        let mut members: Vec<Member> = Vec::new();
        loop {
            self.skip()?;
            match self.peek() {
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Kind::Object(members));
                }
                Some(b'"') => {}
                Some(b',') => {
                    return Err(self.err("invalid JSON: a comma with no member before it"));
                }
                None => return Err(self.err("invalid JSON: an object that does not end")),
                Some(_) => return Err(self.err("invalid JSON: expected a key in double quotes")),
            }
            let key_off = self.i;
            let key = self.string()?;
            if members.iter().any(|m| m.key == key) {
                return Err(ParseError {
                    off: key_off,
                    msg: format!("duplicate key {key:?}"),
                });
            }
            self.skip()?;
            if self.peek() != Some(b':') {
                return Err(self.err("invalid JSON: expected ':' after a key"));
            }
            self.i += 1;
            self.skip()?;
            let value = self.value()?;
            members.push(Member {
                key,
                key_off,
                value,
            });
            self.skip()?;
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {}
                None => return Err(self.err("invalid JSON: an object that does not end")),
                Some(_) => return Err(self.err("invalid JSON: expected ',' or '}' after a value")),
            }
        }
    }

    fn string(&mut self) -> Result<String, ParseError> {
        let start = self.i;
        self.i += 1; // "
        let mut out = String::new();
        loop {
            // Copy the run of plain characters at once.
            let run = self.i;
            while let Some(c) = self.peek() {
                if c == b'"' || c == b'\\' || c < 0x20 {
                    break;
                }
                self.i += 1;
            }
            out.push_str(self.src.get(run..self.i).unwrap_or_default());
            match self.peek() {
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.i += 1;
                    let c = self.escape()?;
                    out.push(c);
                }
                Some(_) => return Err(self.err("invalid JSON: a control character in a string")),
                None => {
                    return Err(ParseError {
                        off: start,
                        msg: "invalid JSON: a string that does not end".into(),
                    });
                }
            }
        }
    }

    fn escape(&mut self) -> Result<char, ParseError> {
        let c = self.peek();
        self.i += 1;
        Ok(match c {
            Some(b'"') => '"',
            Some(b'\\') => '\\',
            Some(b'/') => '/',
            Some(b'b') => '\u{8}',
            Some(b'f') => '\u{c}',
            Some(b'n') => '\n',
            Some(b'r') => '\r',
            Some(b't') => '\t',
            Some(b'u') => {
                let hi = self.hex4()?;
                if (0xD800..0xDC00).contains(&hi) {
                    if self.b.get(self.i..self.i + 2) != Some(b"\\u") {
                        return Err(self.err("invalid JSON: a lone UTF-16 surrogate"));
                    }
                    self.i += 2;
                    let lo = self.hex4()?;
                    if !(0xDC00..0xE000).contains(&lo) {
                        return Err(self.err("invalid JSON: a lone UTF-16 surrogate"));
                    }
                    let cp = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                    char::from_u32(cp).ok_or_else(|| self.err("invalid JSON: a bad \\u escape"))?
                } else {
                    char::from_u32(hi)
                        .ok_or_else(|| self.err("invalid JSON: a lone UTF-16 surrogate"))?
                }
            }
            _ => {
                self.i -= 1;
                return Err(self.err("invalid JSON: an unknown escape in a string"));
            }
        })
    }

    fn hex4(&mut self) -> Result<u32, ParseError> {
        let digits = self
            .src
            .get(self.i..self.i + 4)
            .filter(|s| s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| self.err("invalid JSON: a \\u escape needs four hex digits"))?;
        self.i += 4;
        u32::from_str_radix(digits, 16).map_err(|_| self.err("invalid JSON: a bad \\u escape"))
    }

    fn number(&mut self) -> Result<String, ParseError> {
        let start = self.i;
        let digits = |p: &mut Self| {
            let s = p.i;
            while matches!(p.peek(), Some(b'0'..=b'9')) {
                p.i += 1;
            }
            p.i > s
        };
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        match self.peek() {
            Some(b'0') => {
                self.i += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(self.err("invalid JSON: a number with a leading zero"));
                }
            }
            Some(b'1'..=b'9') => {
                digits(self);
            }
            _ => return Err(self.err("invalid JSON: a '-' with no digits after it")),
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            if !digits(self) {
                return Err(self.err("invalid JSON: a '.' with no digits after it"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if !digits(self) {
                return Err(self.err("invalid JSON: an exponent with no digits"));
            }
        }
        Ok(self.src.get(start..self.i).unwrap_or_default().to_owned())
    }
}

/// The character at `rest`, quoted, for an "unexpected" message.
fn describe(c: u8, rest: Option<&str>) -> String {
    match rest.and_then(|r| r.chars().next()) {
        Some(ch) => format!("{ch:?}"),
        None => format!("byte 0x{c:02x}"),
    }
}

/// The 1-based line and column of byte `off` in `src`, the column counted in characters.
pub(crate) fn position(src: &str, off: usize) -> (u32, u32) {
    let before = src.get(..off.min(src.len())).unwrap_or(src);
    let line = before.bytes().filter(|&b| b == b'\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |n| n + 1);
    let col = before.get(line_start..).map_or(0, |s| s.chars().count()) + 1;
    (
        u32::try_from(line).unwrap_or(u32::MAX),
        u32::try_from(col).unwrap_or(u32::MAX),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(src: &str) -> String {
        parse(src).unwrap_err().msg
    }

    #[test]
    fn values() {
        let v = parse(r#"{"a": [1, -2.5e3, true, false, null, "xé😀\n"]}"#).unwrap();
        let Kind::Object(m) = v.kind else { panic!() };
        let Kind::Array(items) = &m[0].value.kind else {
            panic!()
        };
        assert_eq!(items[0].kind, Kind::Number("1".into()));
        assert_eq!(items[1].kind, Kind::Number("-2.5e3".into()));
        assert_eq!(items[5].kind, Kind::Str("xé😀\n".into()));
    }

    #[test]
    fn comments_and_trailing_commas() {
        let v =
            parse("// head\n{ /* a */ \"a\": [1, 2,], // tail\n \"b\": {\"c\": 1,}, }\n").unwrap();
        let Kind::Object(m) = v.kind else { panic!() };
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn refusals() {
        assert!(err("").contains("empty"));
        assert!(err("[,]").contains("comma"));
        assert!(err("{,}").contains("comma"));
        assert!(err("[1,,2]").contains("comma"));
        assert!(err("{\"a\":1 \"b\":2}").contains("expected ','"));
        assert!(err("01").contains("leading zero"));
        assert!(err("1.").contains("'.'"));
        assert!(err("1e").contains("exponent"));
        assert!(err("-").contains("'-'"));
        assert!(err("\"a").contains("does not end"));
        assert!(err("\"a\u{1}\"").contains("control"));
        assert!(err(r#""\x""#).contains("unknown escape"));
        assert!(err(r#""\ud800""#).contains("surrogate"));
        assert!(err(r#""\udc00""#).contains("surrogate"));
        assert!(err(r#""\u12""#).contains("four hex"));
        assert!(err("/* open").contains("does not end"));
        assert!(err("1 2").contains("more after"));
        assert!(err("tru").contains("unexpected"));
        assert!(err("{'a':1}").contains("double quotes"));
        assert!(err("[1").contains("does not end"));
        assert_eq!(err(r#"{"a":1,"a":2}"#), "duplicate key \"a\"");
    }

    #[test]
    fn depth_is_bounded() {
        let ok = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        assert!(parse(&ok).is_ok());
        let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 1), "]".repeat(MAX_DEPTH + 1));
        assert!(err(&deep).contains("nested deeper"));
    }

    #[test]
    fn positions() {
        let src = "{\n  \"é\": x\n}";
        let e = parse(src).unwrap_err();
        assert_eq!(position(src, e.off), (2, 8));
        assert_eq!(position(src, 0), (1, 1));
    }

    #[test]
    fn garbage_never_panics() {
        // A cheap stand-in for fuzzing on stable: every prefix and single-byte change of a
        // config either parses or is refused.
        let src = r#"{"listeners":[{"name":"web","address":":8080",},], /* c */ "routes":[]}"#;
        for n in 0..=src.len() {
            if let Some(p) = src.get(..n) {
                let _ = parse(p);
            }
        }
        let bytes = src.as_bytes();
        for i in 0..bytes.len() {
            for &b in b"\"{}[],/*\\0x" {
                let mut v = bytes.to_vec();
                v[i] = b;
                if let Ok(s) = std::str::from_utf8(&v) {
                    let _ = parse(s);
                }
            }
        }
    }
}
