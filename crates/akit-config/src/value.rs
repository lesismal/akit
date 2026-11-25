//! A config as a JSON value, for the Admin API (I26): kept as written, read back whole or by a
//! path, and written out again as fit writes it.
//!
//! - [`standardize`] turns a JSON config with comments and trailing commas into standard JSON,
//!   blanking them out so that every other byte, and every position, stays where it was, as fit's
//!   `config.Standardize`.
//! - [`json_of`] is a config's files as one JSON document, as fit's adapter gives it: a lone JSON
//!   file as it was written (standardized), several files or a YAML or `.conf` one merged and
//!   written out.
//! - [`Value`] is a parsed JSON value, members in their order and numbers as written;
//!   [`Value::to_json`] writes it as fit's Admin API does: two spaces to a level.

use crate::error::{Error, Errors};
use crate::json::{self, Kind, Node, position};
use crate::{Format, Source};

/// A JSON value: an object's members in their order, a number as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number, as written.
    Number(String),
    /// A string.
    String(String),
    /// An array.
    Array(Vec<Value>),
    /// An object: its members in their order, no key twice.
    Object(Vec<(String, Value)>),
}

impl Value {
    pub(crate) fn from_node(n: Node) -> Value {
        match n.kind {
            Kind::Null => Value::Null,
            Kind::Bool(b) => Value::Bool(b),
            Kind::Number(s) => Value::Number(s),
            Kind::Str(s) => Value::String(s),
            Kind::Array(a) => Value::Array(a.into_iter().map(Value::from_node).collect()),
            Kind::Object(ms) => Value::Object(
                ms.into_iter()
                    .map(|m| (m.key, Value::from_node(m.value)))
                    .collect(),
            ),
        }
    }

    /// The member `key` of an object.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(ms) => ms.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The value written as fit's Admin API writes a config it edited: two spaces to a level,
    /// members in their order, numbers as written, ending with a newline.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut b = String::new();
        self.write(&mut b, 0);
        b.push('\n');
        b
    }

    fn write(&self, b: &mut String, depth: usize) {
        let indent = |b: &mut String, depth: usize| {
            for _ in 0..depth {
                b.push_str("  ");
            }
        };
        match self {
            Value::Null => b.push_str("null"),
            Value::Bool(v) => b.push_str(if *v { "true" } else { "false" }),
            Value::Number(n) => b.push_str(n),
            Value::String(s) => write_string(b, s),
            Value::Array(a) if a.is_empty() => b.push_str("[]"),
            Value::Object(ms) if ms.is_empty() => b.push_str("{}"),
            Value::Array(a) => {
                b.push_str("[\n");
                for (i, v) in a.iter().enumerate() {
                    indent(b, depth.saturating_add(1));
                    v.write(b, depth.saturating_add(1));
                    if i.saturating_add(1) < a.len() {
                        b.push(',');
                    }
                    b.push('\n');
                }
                indent(b, depth);
                b.push(']');
            }
            Value::Object(ms) => {
                b.push_str("{\n");
                for (i, (k, v)) in ms.iter().enumerate() {
                    indent(b, depth.saturating_add(1));
                    write_string(b, k);
                    b.push_str(": ");
                    v.write(b, depth.saturating_add(1));
                    if i.saturating_add(1) < ms.len() {
                        b.push(',');
                    }
                    b.push('\n');
                }
                indent(b, depth);
                b.push('}');
            }
        }
    }
}

/// Appends `s` as a JSON string, escaped as Go's `encoding/json` escapes it with HTML escaping
/// off: `"`, `\`, the control characters, U+2028 and U+2029.
pub fn write_string(b: &mut String, s: &str) {
    write_go_string(b, s, false);
}

/// Appends `s` as a JSON string as Go's `encoding/json` writes it; `html` says with its HTML
/// escaping on, as `json.Marshal` has it: `<`, `>` and `&` as `\u003c`, `\u003e` and `\u0026`.
pub(crate) fn write_go_string(b: &mut String, s: &str, html: bool) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    b.push('"');
    for c in s.chars() {
        match c {
            '<' if html => b.push_str("\\u003c"),
            '>' if html => b.push_str("\\u003e"),
            '&' if html => b.push_str("\\u0026"),
            '"' => b.push_str("\\\""),
            '\\' => b.push_str("\\\\"),
            '\n' => b.push_str("\\n"),
            '\r' => b.push_str("\\r"),
            '\t' => b.push_str("\\t"),
            '\u{8}' => b.push_str("\\b"),
            '\u{c}' => b.push_str("\\f"),
            '\u{2028}' => b.push_str("\\u2028"),
            '\u{2029}' => b.push_str("\\u2029"),
            c if u32::from(c) < 0x20 => {
                let n = u32::from(c);
                b.push_str("\\u00");
                for nibble in [n >> 4, n & 0xf] {
                    if let Some(h) = usize::try_from(nibble).ok().and_then(|i| HEX.get(i)) {
                        b.push(char::from(*h));
                    }
                }
            }
            c => b.push(c),
        }
    }
    b.push('"');
}

/// The problem a JSON document `src` has, as an [`Errors`] with its position; `file` names it.
fn parse_error(src: &str, file: Option<&str>, e: &json::ParseError) -> Errors {
    let (line, col) = position(src, e.off);
    Errors(vec![Error {
        file: file.map(str::to_owned),
        path: String::new(),
        line,
        col,
        msg: e.msg.clone(),
    }])
}

/// Parses `src`, JSON with comments and trailing commas allowed, as one [`Value`].
///
/// # Errors
///
/// What makes `src` not JSON, at its position.
pub fn parse(src: &str) -> Result<Value, Errors> {
    json::parse(src)
        .map(Value::from_node)
        .map_err(|e| parse_error(src, None, &e))
}

/// `src`, a JSON config with comments and trailing commas allowed, as standard JSON: each
/// comment and each trailing comma replaced by spaces (a comment's line breaks kept), so that
/// every other byte stays where it was, as fit's `config.Standardize`.
///
/// # Errors
///
/// What makes `src` not JSON, at its position.
pub fn standardize(src: &str) -> Result<String, Errors> {
    json::parse(src).map_err(|e| parse_error(src, None, &e))?;
    let b = src.as_bytes();
    let mut out = b.to_vec();
    let mut i = 0usize;
    while let Some(&c) = b.get(i) {
        match c {
            b'"' => i = string_end(b, i),
            b'/' if b.get(i.saturating_add(1)) == Some(&b'/') => {
                while let Some(&c) = b.get(i).filter(|c| **c != b'\n') {
                    blank(&mut out, i, c);
                    i = i.saturating_add(1);
                }
            }
            b'/' if b.get(i.saturating_add(1)) == Some(&b'*') => {
                let end = block_end(b, i);
                while i < end {
                    if let Some(&c) = b.get(i) {
                        blank(&mut out, i, c);
                    }
                    i = i.saturating_add(1);
                }
            }
            b',' => {
                if matches!(b.get(skip_blank(b, i.saturating_add(1))), Some(b']' | b'}')) {
                    blank(&mut out, i, c);
                }
                i = i.saturating_add(1);
            }
            _ => i = i.saturating_add(1),
        }
    }
    // Only ASCII bytes were replaced, by spaces: still UTF-8.
    String::from_utf8(out).map_err(|_| {
        Errors(vec![Error {
            file: None,
            path: String::new(),
            line: 0,
            col: 0,
            msg: "the config is not UTF-8".into(),
        }])
    })
}

/// Replaces byte `i` by a space unless it is a line break.
fn blank(out: &mut [u8], i: usize, c: u8) {
    if c != b'\n' && c != b'\r' {
        if let Some(o) = out.get_mut(i) {
            *o = b' ';
        }
    }
}

/// The index after the string starting at `i` (its opening quote).
fn string_end(b: &[u8], mut i: usize) -> usize {
    i = i.saturating_add(1);
    while let Some(&c) = b.get(i) {
        match c {
            b'\\' => i = i.saturating_add(2),
            b'"' => return i.saturating_add(1),
            _ => i = i.saturating_add(1),
        }
    }
    i
}

/// The index after the `/* */` comment starting at `i`.
fn block_end(b: &[u8], i: usize) -> usize {
    let mut j = i.saturating_add(2);
    while j < b.len() {
        if b.get(j) == Some(&b'*') && b.get(j.saturating_add(1)) == Some(&b'/') {
            return j.saturating_add(2);
        }
        j = j.saturating_add(1);
    }
    b.len()
}

/// The index of the first byte from `i` on that is neither white space nor in a comment.
fn skip_blank(b: &[u8], mut i: usize) -> usize {
    loop {
        match b.get(i) {
            Some(b' ' | b'\t' | b'\n' | b'\r') => i = i.saturating_add(1),
            Some(b'/') if b.get(i.saturating_add(1)) == Some(&b'/') => {
                while b.get(i).is_some_and(|c| *c != b'\n') {
                    i = i.saturating_add(1);
                }
            }
            Some(b'/') if b.get(i.saturating_add(1)) == Some(&b'*') => i = block_end(b, i),
            _ => return i,
        }
    }
}

/// The members of a config's top object that several files may each give, joined in the files'
/// order, as fit's adapter merges them.
const LISTS: &[&str] = &["listeners", "routes", "upstreams", "services", "providers"];

/// A config's files as one JSON document, as fit's adapter gives it to its Admin API: a lone JSON
/// file as written, [`standardize`]d; several files, or a YAML or `.conf` one, as one object
/// written by [`Value::to_json`], the lists at the top joined in the files' order and every
/// other member taken from the first file that gives it. Meant for files [`from_sources`]
/// read without a problem.
///
/// # Errors
///
/// What makes a file not parse, naming it.
///
/// [`from_sources`]: crate::from_sources
pub fn json_of(srcs: &[Source<'_>]) -> Result<String, Errors> {
    if let [one] = srcs {
        if one.format == Format::Json {
            return standardize(one.src).map_err(|mut e| {
                for err in &mut e.0 {
                    err.file = one.name.map(str::to_owned);
                }
                e
            });
        }
    }
    let mut merged: Vec<(String, Value)> = Vec::new();
    for s in srcs {
        let node = match s.format {
            Format::Json => json::parse(s.src),
            Format::Yaml => crate::yaml::parse(s.src),
            Format::Conf => crate::conf::parse(s.src, &|name| std::env::var(name).ok()),
        }
        .map_err(|e| parse_error(s.src, s.name, &e))?;
        let Value::Object(members) = Value::from_node(node) else {
            continue;
        };
        for (k, v) in members {
            match (merged.iter_mut().find(|(m, _)| *m == k), v) {
                (Some((_, Value::Array(have))), Value::Array(more))
                    if LISTS.contains(&k.as_str()) =>
                {
                    have.extend(more);
                }
                (Some(_), _) => {}
                (None, v) => merged.push((k, v)),
            }
        }
    }
    Ok(Value::Object(merged).to_json())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{Value, json_of, parse, standardize};
    use crate::{Format, Source};

    #[test]
    fn standardize_blanks_comments_and_trailing_commas() {
        let src = "// v2\n{\"a\": [1, 2,], /* b\n */ \"s\": \"x // y, /* z */ ,]\",}";
        let std = standardize(src).unwrap();
        assert_eq!(std.len(), src.len());
        assert_eq!(
            std,
            "     \n{\"a\": [1, 2 ],     \n    \"s\": \"x // y, /* z */ ,]\" }"
        );
        assert!(
            standardize("{/* open").unwrap_err().0[0]
                .msg
                .contains("does not end")
        );
        // A comma before a comment and then the end is a trailing one too.
        assert_eq!(standardize("[1, // x\n]").unwrap(), "[1      \n]");
    }

    #[test]
    fn written_as_fit_writes_it() {
        let v =
            parse(r#"{"a": [1, {"b": null}], "e": {}, "f": [], "s": "q\"\\\n\u0001<>"}"#).unwrap();
        assert_eq!(
            v.to_json(),
            "{\n  \"a\": [\n    1,\n    {\n      \"b\": null\n    }\n  ],\n  \"e\": {},\n  \"f\": [],\n  \"s\": \"q\\\"\\\\\\n\\u0001<>\"\n}\n"
        );
        assert_eq!(v.get("e"), Some(&Value::Object(vec![])));
        assert_eq!(parse("1.50e3").unwrap().to_json(), "1.50e3\n");
    }

    #[test]
    fn files_as_one_document() {
        let one = |src, format| Source {
            name: Some("a"),
            format,
            src,
        };
        assert_eq!(
            json_of(&[one("{\"listeners\": [], } // x", Format::Json)]).unwrap(),
            "{\"listeners\": []  }     "
        );
        let got = json_of(&[
            one(
                r#"{"listeners": [{"name": "a"}], "limits": {}}"#,
                Format::Json,
            ),
            one("listeners:\n  - name: b\nroutes: []\n", Format::Yaml),
        ])
        .unwrap();
        assert_eq!(
            got,
            "{\n  \"listeners\": [\n    {\n      \"name\": \"a\"\n    },\n    {\n      \"name\": \"b\"\n    }\n  ],\n  \"limits\": {},\n  \"routes\": []\n}\n"
        );
        let err = json_of(&[one("{", Format::Json), one("{", Format::Json)]).unwrap_err();
        assert_eq!(err.0[0].file.as_deref(), Some("a"));
    }
}
