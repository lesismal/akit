//! A config converted between its formats, as fit's `fit adapt` converts it (I73): its files as
//! one JSON document, as fit's adapter gives it ([`document`], fit's `adapter.Set`), written as
//! JSON, YAML or fit's own `.conf` format ([`Document::write`]), each byte for byte as fit writes
//! it:
//!
//! - JSON as Go's `json.Indent` writes the document with two spaces, which keeps each string and
//!   number as the document has it: a lone JSON file's as written, a YAML or `.conf` file's as
//!   Go's `json.Marshal` writes them.
//! - YAML as fit's `adapter/yaml.FromJSON`: go-yaml's encoder with two spaces ([`yaml_out`]).
//! - `.conf` as fit's `adapter/fitconf.FromJSON`: each field as its type in fit's config has it,
//!   with the shorthands fit's reader takes, and a block that fits on a line on one
//!   ([`conf_out`]).
//!
//! Converting is control plane code: it reads whole files, never on an I/O thread.

mod conf_out;
mod yaml_out;

use crate::error::{Error, Errors};
use crate::json::{self, Kind, Node, ParseError, position};
use crate::value::{Value, write_go_string};
use crate::{Format, Source};

pub use conf_out::from_json as conf_from_json;
pub use yaml_out::from_json as yaml_from_json;

/// What a config is written as: fit's `-to` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// JSON, with two spaces of indentation (`json`).
    Json,
    /// YAML (`yaml`).
    Yaml,
    /// fit's own format, `.conf` (`fit`).
    Conf,
}

impl Target {
    /// The target fit's `-to` names: `json`, `yaml` or `fit`.
    #[must_use]
    pub fn named(name: &str) -> Option<Target> {
        match name {
            "json" => Some(Target::Json),
            "yaml" => Some(Target::Yaml),
            "fit" => Some(Target::Conf),
            _ => None,
        }
    }

    /// The target a file of format `f` is written in again.
    #[must_use]
    pub fn of(f: Format) -> Target {
        match f {
            Format::Json => Target::Json,
            Format::Yaml => Target::Yaml,
            Format::Conf => Target::Conf,
        }
    }
}

/// A JSON value with its strings and numbers as a document writes them, escapes and all: what
/// Go's `json.Indent` keeps.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Raw {
    /// A string, a number, `true`, `false` or `null`, as written.
    Token(String),
    Array(Vec<Raw>),
    /// An object: each key as written, quotes and all, and its value.
    Object(Vec<(String, Raw)>),
}

/// A config's files as one JSON document, as fit's adapter gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    raw: Raw,
    /// What follows the value in a lone JSON file: white space, and comments blanked out, which
    /// Go's `json.Indent` keeps.
    trailing: String,
}

/// The files `srcs` as one JSON document, as fit's adapter gives it: a lone file as it reads it
/// (a JSON file's strings and numbers as written), several merged as fit merges them, the
/// members of each file's top object in order, a list given by several files joined in the
/// files' order. Meant for files [`from_sources`] reads with no problem other than fields akit
/// does not support yet.
///
/// # Errors
///
/// What makes a file not parse, or a member other than a list given by two files, naming its
/// file.
///
/// [`from_sources`]: crate::from_sources
pub fn document(srcs: &[Source<'_>]) -> Result<Document, Errors> {
    let mut parts = Vec::with_capacity(srcs.len());
    for s in srcs {
        let parsed = match s.format {
            Format::Json => json::parse(s.src).map(|n| (raw_json(s.src, &n), n)),
            Format::Yaml => crate::yaml::parse(s.src).map(|n| (raw_marshal(&n), n)),
            Format::Conf => crate::conf::parse(s.src, &|name| std::env::var(name).ok())
                .map(|n| (raw_marshal(&n), n)),
        };
        let (raw, node) = parsed.map_err(|e| file_error(s, &e))?;
        parts.push((s, raw, node));
    }
    if let [(s, raw, node)] = parts.as_slice() {
        let trailing = if s.format == Format::Json {
            // What follows the value: white space and comments, the comments blanked out, as
            // fit's config.Standardize leaves them.
            let end = value_end(s.src, node);
            blank_comments(s.src.get(end..).unwrap_or_default())
        } else {
            String::new()
        };
        return Ok(Document {
            raw: raw.clone(),
            trailing,
        });
    }
    merge(&parts)
}

/// The files merged, as fit's `Set.merge`: a member whose value is a list (or null), other than
/// `schema_version`, joined across the files; any other given by one file only, but
/// `schema_version`, which the files may each give if they agree.
fn merge(parts: &[(&Source<'_>, Raw, Node)]) -> Result<Document, Errors> {
    struct Member {
        key: String,
        list: Option<Vec<Raw>>,
        value: Option<Raw>,
        /// The file that gave it first.
        file: usize,
    }
    let mut members: Vec<Member> = Vec::new();
    for (i, (s, raw, node)) in parts.iter().enumerate() {
        let (Raw::Object(ms), Kind::Object(nms)) = (raw, &node.kind) else {
            return Err(file_error(
                s,
                &ParseError {
                    off: node.off,
                    msg: "a config is an object".into(),
                },
            ));
        };
        for ((_, v), nm) in ms.iter().zip(nms) {
            let key = nm.key.clone();
            let is_list =
                key != "schema_version" && matches!(nm.value.kind, Kind::Array(_) | Kind::Null);
            let have = members.iter_mut().find(|m| m.key == key);
            let conflict = |first: usize| {
                let msg = if key == "schema_version" {
                    format!("differs from the one in {}", name(parts, first))
                } else {
                    format!(
                        "also given in {}: only the lists at the top of a config (listeners, routes, upstreams, services, providers) may be split across files",
                        name(parts, first)
                    )
                };
                let (line, col) = position(s.src, nm.key_off);
                Errors(vec![Error {
                    file: s.name.map(str::to_owned),
                    path: key.clone(),
                    line,
                    col,
                    msg,
                }])
            };
            match have {
                Some(m) if is_list => match &mut m.list {
                    Some(l) => {
                        if let Raw::Array(more) = v {
                            l.extend(more.iter().cloned());
                        }
                    }
                    None => return Err(conflict(m.file)),
                },
                Some(m) => {
                    if key != "schema_version" || m.value.as_ref() != Some(v) {
                        return Err(conflict(m.file));
                    }
                }
                None => members.push(Member {
                    key: key.clone(),
                    list: is_list.then(|| match v {
                        Raw::Array(a) => a.clone(),
                        _ => Vec::new(),
                    }),
                    value: (!is_list).then(|| v.clone()),
                    file: i,
                }),
            }
        }
    }
    let raw = Raw::Object(
        members
            .into_iter()
            .map(|m| {
                let mut k = String::new();
                write_go_string(&mut k, &m.key, true);
                let v = match (m.list, m.value) {
                    (Some(l), _) => Raw::Array(l),
                    (None, Some(v)) => v,
                    (None, None) => Raw::Token("null".into()),
                };
                (k, v)
            })
            .collect(),
    );
    Ok(Document {
        raw,
        trailing: String::new(),
    })
}

/// Whether `src`, a file in format `f`, holds a comment, which writing it again loses: `//` or
/// `/* */` in JSON, `#` in YAML and `.conf`. A file that does not parse holds none.
#[must_use]
pub fn has_comments(src: &str, f: Format) -> bool {
    match f {
        Format::Json => {
            let b = src.as_bytes();
            let mut i = 0usize;
            while let Some(&c) = b.get(i) {
                match c {
                    b'"' => i = i.saturating_add(string_token(src, i).len()),
                    b'/' if matches!(b.get(i.saturating_add(1)), Some(b'/' | b'*')) => {
                        return true;
                    }
                    _ => i = i.saturating_add(1),
                }
            }
            false
        }
        Format::Yaml => crate::yaml::parse_noting_comments(src).is_ok_and(|(_, c)| c),
        Format::Conf => crate::conf::has_comment(src),
    }
}

fn name(parts: &[(&Source<'_>, Raw, Node)], i: usize) -> String {
    parts
        .get(i)
        .and_then(|(s, _, _)| s.name)
        .unwrap_or_default()
        .to_owned()
}

fn file_error(s: &Source<'_>, e: &ParseError) -> Errors {
    let (line, col) = position(s.src, e.off);
    Errors(vec![Error {
        file: s.name.map(str::to_owned),
        path: String::new(),
        line,
        col,
        msg: e.msg.clone(),
    }])
}

impl Document {
    /// The document written as `to`, byte for byte as fit's `fit adapt -to` writes it.
    ///
    /// # Errors
    ///
    /// Why it cannot be written as `to`: a `.conf` file cannot hold every JSON document (a list
    /// of fewer than two values in a field of no known type reads back as the value alone).
    pub fn write(&self, to: Target) -> Result<String, String> {
        match to {
            Target::Json => {
                let mut b = String::new();
                indent(&mut b, &self.raw, 0);
                b.push_str(&self.trailing);
                b.push('\n');
                Ok(b)
            }
            Target::Yaml => Ok(yaml_out::write(&self.value())),
            Target::Conf => conf_out::write(&self.value()),
        }
    }

    /// The document as a value, its strings read, as fit's writers decode it.
    fn value(&self) -> Value {
        value_of(&self.raw)
    }
}

fn value_of(r: &Raw) -> Value {
    match r {
        Raw::Token(t) => match json::parse(t) {
            Ok(n) => Value::from_node(n),
            // Tokens come from a parsed document.
            Err(_) => Value::Null,
        },
        Raw::Array(a) => Value::Array(a.iter().map(value_of).collect()),
        Raw::Object(ms) => Value::Object(
            ms.iter()
                .map(|(k, v)| {
                    let key = match value_of(&Raw::Token(k.clone())) {
                        Value::String(s) => s,
                        _ => String::new(),
                    };
                    (key, value_of(v))
                })
                .collect(),
        ),
    }
}

/// Writes `r` as Go's `json.Indent` does with no prefix and two spaces: each element on a line of
/// its own, `"key": value`, an empty array or object as `[]` or `{}`.
fn indent(b: &mut String, r: &Raw, depth: usize) {
    let pad = |b: &mut String, depth: usize| {
        for _ in 0..depth {
            b.push_str("  ");
        }
    };
    let inner = depth.saturating_add(1);
    match r {
        Raw::Token(t) => b.push_str(t),
        Raw::Array(a) if a.is_empty() => b.push_str("[]"),
        Raw::Object(ms) if ms.is_empty() => b.push_str("{}"),
        Raw::Array(a) => {
            b.push('[');
            for (i, v) in a.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                b.push('\n');
                pad(b, inner);
                indent(b, v, inner);
            }
            b.push('\n');
            pad(b, depth);
            b.push(']');
        }
        Raw::Object(ms) => {
            b.push('{');
            for (i, (k, v)) in ms.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                b.push('\n');
                pad(b, inner);
                b.push_str(k);
                b.push_str(": ");
                indent(b, v, inner);
            }
            b.push('\n');
            pad(b, depth);
            b.push('}');
        }
    }
}

/// A JSON document's value with its strings and numbers as `src` writes them.
fn raw_json(src: &str, n: &Node) -> Raw {
    match &n.kind {
        Kind::Array(a) => Raw::Array(a.iter().map(|e| raw_json(src, e)).collect()),
        Kind::Object(ms) => Raw::Object(
            ms.iter()
                .map(|m| (string_token(src, m.key_off), raw_json(src, &m.value)))
                .collect(),
        ),
        Kind::Str(_) => Raw::Token(string_token(src, n.off)),
        Kind::Number(s) => Raw::Token(s.clone()),
        Kind::Bool(v) => Raw::Token(if *v { "true" } else { "false" }.into()),
        Kind::Null => Raw::Token("null".into()),
    }
}

/// A YAML or `.conf` file's value as fit's adapters write it: each string as Go's `json.Marshal`
/// escapes it, each number as the adapter gives it.
fn raw_marshal(n: &Node) -> Raw {
    let string = |s: &str| {
        let mut b = String::new();
        write_go_string(&mut b, s, true);
        b
    };
    match &n.kind {
        Kind::Array(a) => Raw::Array(a.iter().map(raw_marshal).collect()),
        Kind::Object(ms) => Raw::Object(
            ms.iter()
                .map(|m| (string(&m.key), raw_marshal(&m.value)))
                .collect(),
        ),
        Kind::Str(s) => Raw::Token(string(s)),
        Kind::Number(s) => Raw::Token(s.clone()),
        Kind::Bool(v) => Raw::Token(if *v { "true" } else { "false" }.into()),
        Kind::Null => Raw::Token("null".into()),
    }
}

/// The string token starting at `off` in `src`, quotes and all.
fn string_token(src: &str, off: usize) -> String {
    let b = src.as_bytes();
    let mut i = off.saturating_add(1);
    while let Some(&c) = b.get(i) {
        match c {
            b'\\' => i = i.saturating_add(2),
            b'"' => {
                i = i.saturating_add(1);
                break;
            }
            _ => i = i.saturating_add(1),
        }
    }
    src.get(off..i.min(b.len())).unwrap_or_default().to_owned()
}

/// The offset just after the value `n` in `src`.
fn value_end(src: &str, n: &Node) -> usize {
    match &n.kind {
        Kind::Str(_) => n.off.saturating_add(string_token(src, n.off).len()),
        Kind::Number(s) => n.off.saturating_add(s.len()),
        Kind::Bool(true) | Kind::Null => n.off.saturating_add(4),
        Kind::Bool(false) => n.off.saturating_add(5),
        Kind::Array(_) | Kind::Object(_) => {
            // The closing bracket: the first after the last element that is not in a comment.
            let last = match &n.kind {
                Kind::Array(a) => a.last().map(|e| value_end(src, e)),
                Kind::Object(ms) => ms.last().map(|m| value_end(src, &m.value)),
                _ => None,
            };
            let b = src.as_bytes();
            let mut i = last.unwrap_or_else(|| n.off.saturating_add(1));
            while let Some(&c) = b.get(i) {
                match c {
                    b']' | b'}' => return i.saturating_add(1),
                    b'/' if b.get(i.saturating_add(1)) == Some(&b'/') => {
                        while b.get(i).is_some_and(|c| *c != b'\n') {
                            i = i.saturating_add(1);
                        }
                    }
                    b'/' if b.get(i.saturating_add(1)) == Some(&b'*') => {
                        i = i.saturating_add(2);
                        while i < b.len()
                            && !(b.get(i) == Some(&b'*')
                                && b.get(i.saturating_add(1)) == Some(&b'/'))
                        {
                            i = i.saturating_add(1);
                        }
                        i = i.saturating_add(2);
                    }
                    _ => i = i.saturating_add(1),
                }
            }
            b.len()
        }
    }
}

/// White space and comments after a JSON value, each comment blanked out to spaces, its line
/// breaks kept, as fit's `config.Standardize` leaves them.
fn blank_comments(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut in_line = false;
    let mut in_block = false;
    let mut i = 0usize;
    while let Some(&c) = b.get(i) {
        let next = b.get(i.saturating_add(1)).copied();
        if in_block {
            if c == b'*' && next == Some(b'/') {
                out.push_str("  ");
                in_block = false;
                i = i.saturating_add(2);
                continue;
            }
        } else if !in_line && c == b'/' && next == Some(b'/') {
            in_line = true;
        } else if !in_line && c == b'/' && next == Some(b'*') {
            out.push_str("  ");
            in_block = true;
            i = i.saturating_add(2);
            continue;
        }
        if c == b'\n' {
            in_line = false;
        }
        if (in_line || in_block) && c != b'\n' && c != b'\r' {
            // A comment's character, which may take several bytes: one space a byte, as
            // Standardize writes it.
            out.push(' ');
        } else if c.is_ascii() {
            out.push(char::from(c));
        }
        i = i.saturating_add(1);
    }
    out
}

#[cfg(test)]
mod tests;
