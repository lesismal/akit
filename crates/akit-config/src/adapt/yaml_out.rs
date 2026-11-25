//! A JSON value written as YAML, byte for byte as fit's `adapter/yaml.FromJSON` writes it: each
//! value a go-yaml node (a string with a line break in the literal style, `<<` quoted, a number
//! tagged `!!int` or `!!float`), written by go-yaml's encoder (v3.0.5) with two spaces of
//! indentation and no line width.
//!
//! This is the part of go-yaml's emitter (`emitterc.go`) and resolver (`resolve.go`) those nodes
//! reach: block mappings and sequences, empty ones in the flow style, and the four scalar styles
//! with the analysis that picks one, kept as go-yaml has them so that the output is the same in
//! the corners too (a key longer than 128 bytes written after `?`, a string that would read as a
//! number or a date quoted, a character outside the Basic Multilingual Plane escaped).
//!
//! Lengths and columns here are bounded by the document's length, so the arithmetic cannot
//! overflow.
#![allow(clippy::arithmetic_side_effects)]

use crate::value::Value;

/// The JSON document `src` written as YAML, as fit's `yaml.FromJSON`.
///
/// # Errors
///
/// What makes `src` not JSON, as `JSON: <why>`.
pub fn from_json(src: &str) -> Result<String, String> {
    let n = crate::json::parse(src).map_err(|e| format!("JSON: {}", e.msg))?;
    Ok(write(&Value::from_node(n)))
}

/// `v` written as YAML.
pub(super) fn write(v: &Value) -> String {
    let mut evs = Vec::new();
    events(v, &mut evs);
    let mut e = Emitter::new(&evs);
    e.run();
    e.out
}

/// The tags go-yaml's resolver gives a plain scalar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tag {
    Null,
    Bool,
    Int,
    Float,
    Str,
    Timestamp,
    Merge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    Plain,
    Single,
    Double,
    Literal,
}

#[derive(Debug)]
enum Ev {
    MapStart,
    MapEnd,
    SeqStart,
    SeqEnd,
    Scalar {
        value: String,
        /// The tag's suffix after `!!` when it is written (`int`, `float`), none when implicit.
        tag: Option<&'static str>,
        style: Style,
    },
}

/// The events go-yaml's encoder emits for `v`, as fit's `FromJSON` makes its nodes.
fn events(v: &Value, out: &mut Vec<Ev>) {
    match v {
        Value::Object(ms) => {
            out.push(Ev::MapStart);
            for (k, v) in ms {
                out.push(string(k, true));
                events(v, out);
            }
            out.push(Ev::MapEnd);
        }
        Value::Array(a) => {
            out.push(Ev::SeqStart);
            for v in a {
                events(v, out);
            }
            out.push(Ev::SeqEnd);
        }
        Value::String(s) => out.push(string(s, false)),
        Value::Number(n) => {
            let (tag, name) = if n.contains(['.', 'e', 'E']) {
                (Tag::Float, "float")
            } else {
                (Tag::Int, "int")
            };
            out.push(Ev::Scalar {
                value: n.clone(),
                tag: (resolve(n) != tag).then_some(name),
                style: Style::Plain,
            });
        }
        Value::Bool(b) => out.push(Ev::Scalar {
            value: if *b { "true" } else { "false" }.into(),
            tag: None,
            style: Style::Plain,
        }),
        Value::Null => out.push(Ev::Scalar {
            value: "null".into(),
            tag: None,
            style: Style::Plain,
        }),
    }
}

/// A string node: a key, or a value, which fit writes in the literal style when it holds a line
/// break; `<<`, either, double-quoted, which plain would be a merge key. Then as go-yaml's
/// encoder: a string that would read back as another kind is double-quoted.
fn string(s: &str, key: bool) -> Ev {
    let node_style = if !key && s.contains('\n') {
        Some(Style::Literal)
    } else if s == "<<" {
        Some(Style::Double)
    } else {
        None
    };
    let style = match node_style {
        Some(st) => st,
        None if s.contains('\n') => Style::Literal,
        None if resolve(s) != Tag::Str => Style::Double,
        None => Style::Plain,
    };
    Ev::Scalar {
        value: s.to_owned(),
        tag: None,
        style,
    }
}

// --- go-yaml's resolver ---

/// The tag go-yaml's resolver gives `s` written plain.
fn resolve(s: &str) -> Tag {
    let hint = match s.as_bytes().first() {
        None => b'N',
        Some(b'+' | b'-') => b'S',
        Some(b'0'..=b'9') => b'D',
        Some(b'y' | b'Y' | b'n' | b'N' | b't' | b'T' | b'f' | b'F' | b'o' | b'O' | b'~') => b'M',
        Some(b'.') => b'.',
        Some(_) => return Tag::Str,
    };
    match s {
        "true" | "True" | "TRUE" | "false" | "False" | "FALSE" => return Tag::Bool,
        "" | "~" | "null" | "Null" | "NULL" => return Tag::Null,
        ".nan" | ".NaN" | ".NAN" | ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF"
        | "-.inf" | "-.Inf" | "-.INF" => return Tag::Float,
        "<<" => return Tag::Merge,
        _ => {}
    }
    match hint {
        b'.' => {
            if go_float_dot(s) {
                return Tag::Float;
            }
        }
        b'D' | b'S' => {
            if timestamp(s) {
                return Tag::Timestamp;
            }
            let plain: String = s.chars().filter(|c| *c != '_').collect();
            if parse_int(&plain, 0).is_some() || parse_uint(&plain, 0).is_some() {
                return Tag::Int;
            }
            if yaml_float(&plain) && plain.parse::<f64>().is_ok_and(f64::is_finite) {
                return Tag::Float;
            }
            if let Some(rest) = plain.strip_prefix("0b") {
                if parse_int(rest, 2).is_some() || parse_uint(rest, 2).is_some() {
                    return Tag::Int;
                }
            } else if let Some(rest) = plain.strip_prefix("-0b") {
                if parse_int(&format!("-{rest}"), 2).is_some() {
                    return Tag::Int;
                }
            }
            if let Some(rest) = plain.strip_prefix("0o") {
                if parse_int(rest, 8).is_some() || parse_uint(rest, 8).is_some() {
                    return Tag::Int;
                }
            } else if let Some(rest) = plain.strip_prefix("-0o") {
                if parse_int(&format!("-{rest}"), 8).is_some() {
                    return Tag::Int;
                }
            }
        }
        _ => {}
    }
    Tag::Str
}

/// Go's `strconv.ParseUint(s, base, 64)`, with no underscores: base 0 reads a `0x`, `0o` or `0b`
/// prefix, or a leading `0` as octal.
fn parse_uint(s: &str, base: u32) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    let (base, digits) = if base == 0 {
        let b = s.as_bytes();
        match (b.first(), b.get(1).map(u8::to_ascii_lowercase)) {
            (Some(b'0'), Some(p @ (b'b' | b'o' | b'x'))) if b.len() >= 3 => {
                let base = match p {
                    b'b' => 2,
                    b'o' => 8,
                    _ => 16,
                };
                (base, s.get(2..)?)
            }
            (Some(b'0'), _) => (8, s.get(1..)?),
            _ => (10, s),
        }
    } else {
        (base, s)
    };
    let mut n: u64 = 0;
    for c in digits.chars() {
        let d = c.to_digit(36)?;
        if d >= base {
            return None;
        }
        n = n.checked_mul(u64::from(base))?.checked_add(u64::from(d))?;
    }
    Some(n)
}

/// Go's `strconv.ParseInt(s, base, 64)`, with no underscores.
fn parse_int(s: &str, base: u32) -> Option<i64> {
    let (neg, rest) = match s.as_bytes().first() {
        Some(b'+') => (false, s.get(1..)?),
        Some(b'-') => (true, s.get(1..)?),
        _ => (false, s),
    };
    let u = parse_uint(rest, base)?;
    if neg {
        if u > 1 << 63 {
            return None;
        }
        Some(0i64.wrapping_sub_unsigned(u))
    } else {
        i64::try_from(u).ok()
    }
}

/// go-yaml's `yamlStyleFloat`: `^[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?$`.
fn yaml_float(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = usize::from(matches!(b.first(), Some(b'+' | b'-')));
    let digits = |i: &mut usize| {
        let start = *i;
        while b.get(*i).is_some_and(u8::is_ascii_digit) {
            *i += 1;
        }
        *i > start
    };
    if b.get(i) == Some(&b'.') {
        i += 1;
        if !digits(&mut i) {
            return false;
        }
    } else {
        if !digits(&mut i) {
            return false;
        }
        if b.get(i) == Some(&b'.') {
            i += 1;
            digits(&mut i);
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        if !digits(&mut i) {
            return false;
        }
    }
    i == b.len()
}

/// Whether Go's `strconv.ParseFloat` reads `s`, which starts with `.`, as a finite number: digits
/// after the point, an exponent, underscores only between digits.
fn go_float_dot(s: &str) -> bool {
    // Go's underscoreOK: an underscore only between two digits.
    let mut saw = b'^';
    for c in s.bytes() {
        if c.is_ascii_digit() {
            saw = b'0';
        } else if c == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
        } else {
            if saw == b'_' {
                return false;
            }
            saw = b'!';
        }
    }
    if saw == b'_' {
        return false;
    }
    let plain: String = s.chars().filter(|c| *c != '_').collect();
    let b = plain.as_bytes();
    let mut i = 1;
    let digits = |i: &mut usize| {
        let start = *i;
        while b.get(*i).is_some_and(u8::is_ascii_digit) {
            *i += 1;
        }
        *i > start
    };
    if !digits(&mut i) {
        return false;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        if !digits(&mut i) {
            return false;
        }
    }
    i == b.len() && plain.parse::<f64>().is_ok_and(f64::is_finite)
}

/// Whether go-yaml reads `s` as a timestamp: four digits and a `-`, then one of its layouts as
/// Go's `time.Parse` reads them.
fn timestamp(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 5 || !b.iter().take(4).all(u8::is_ascii_digit) || b.get(4) != Some(&b'-') {
        return false;
    }
    [
        "2006-1-2T15:4:5.999999999Z07:00",
        "2006-1-2t15:4:5.999999999Z07:00",
        "2006-1-2 15:4:5.999999999",
        "2006-1-2",
    ]
    .iter()
    .any(|l| go_time_parse(l, s))
}

/// One or two digits, as Go's `getnum` with `fixed` false; two with `fixed`.
fn getnum(s: &[u8], fixed: bool) -> Option<(u32, &[u8])> {
    let d0 = *s.first().filter(|c| c.is_ascii_digit())?;
    match s.get(1).filter(|c| c.is_ascii_digit()) {
        Some(d1) => Some((
            u32::from(d0 - b'0') * 10 + u32::from(d1 - b'0'),
            s.get(2..)?,
        )),
        None if fixed => None,
        None => Some((u32::from(d0 - b'0'), s.get(1..)?)),
    }
}

/// Go's `time.Parse(layout, value)` succeeding, for go-yaml's four layouts: their elements
/// `2006`, `1`, `2`, `15`, `4`, `5`, `.999999999` and `Z07:00`, and literal text.
fn go_time_parse(layout: &str, value: &str) -> bool {
    const ELEMS: &[&str] = &["2006", ".999999999", "Z07:00", "15", "1", "2", "4", "5"];
    let mut l = layout.as_bytes();
    let mut v = value.as_bytes();
    let (mut year, mut month, mut day) = (0u32, 0u32, 0u32);
    while !l.is_empty() {
        let Some(e) = ELEMS.iter().find(|e| l.starts_with(e.as_bytes())) else {
            // Literal text: a space matches one or more spaces.
            let c = l.first().copied().unwrap_or_default();
            if c == b' ' {
                if v.first().is_some_and(|x| *x != b' ') {
                    return false;
                }
                while l.first() == Some(&b' ') {
                    l = l.get(1..).unwrap_or_default();
                }
                while v.first() == Some(&b' ') {
                    v = v.get(1..).unwrap_or_default();
                }
                continue;
            }
            if v.first() != Some(&c) {
                return false;
            }
            l = l.get(1..).unwrap_or_default();
            v = v.get(1..).unwrap_or_default();
            continue;
        };
        l = l.get(e.len()..).unwrap_or_default();
        match *e {
            "2006" => {
                if v.len() < 4 || !v.iter().take(4).all(u8::is_ascii_digit) {
                    return false;
                }
                year = v
                    .iter()
                    .take(4)
                    .fold(0, |n, d| n * 10 + u32::from(d - b'0'));
                v = v.get(4..).unwrap_or_default();
            }
            "1" | "2" | "15" | "4" | "5" => {
                let Some((n, rest)) = getnum(v, false) else {
                    return false;
                };
                v = rest;
                match *e {
                    "1" if n == 0 || n > 12 => return false,
                    "1" => month = n,
                    "2" => day = n,
                    "15" if n >= 24 => return false,
                    "4" | "5" if n >= 60 => return false,
                    _ => {}
                }
            }
            ".999999999" => {
                if v.len() >= 2
                    && matches!(v.first(), Some(b'.' | b','))
                    && v.get(1).is_some_and(u8::is_ascii_digit)
                {
                    let n = v.iter().skip(1).take_while(|c| c.is_ascii_digit()).count();
                    v = v.get(1 + n..).unwrap_or_default();
                }
            }
            _ => {
                // Z07:00
                if v.first() == Some(&b'Z') {
                    v = v.get(1..).unwrap_or_default();
                    continue;
                }
                if v.len() < 6 || v.get(3) != Some(&b':') {
                    return false;
                }
                let (Some((hr, _)), Some((mm, _))) = (
                    getnum(v.get(1..3).unwrap_or_default(), true),
                    getnum(v.get(4..6).unwrap_or_default(), true),
                ) else {
                    return false;
                };
                if hr > 24 || mm > 60 || !matches!(v.first(), Some(b'+' | b'-')) {
                    return false;
                }
                v = v.get(6..).unwrap_or_default();
            }
        }
    }
    if !v.is_empty() {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    day >= 1 && day <= days
}

// --- go-yaml's emitter ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    DocumentContent,
    DocumentEnd,
    FlowSequenceFirstItem,
    FlowMappingFirstKey,
    BlockSequenceFirstItem,
    BlockSequenceItem,
    BlockMappingFirstKey,
    BlockMappingKey,
    BlockMappingSimpleValue,
    BlockMappingValue,
}

/// What go-yaml's `yaml_emitter_analyze_scalar` finds in a scalar.
#[derive(Debug, Default, Clone, Copy)]
struct Analysis {
    multiline: bool,
    flow_plain_allowed: bool,
    block_plain_allowed: bool,
    single_quoted_allowed: bool,
    block_allowed: bool,
}

const BEST_INDENT: i64 = 2;

struct Emitter<'a> {
    evs: &'a [Ev],
    out: String,
    state: State,
    states: Vec<State>,
    indent: i64,
    indents: Vec<i64>,
    flow_level: u32,
    column: i64,
    whitespace: bool,
    indention: bool,
    root_context: bool,
    simple_key_context: bool,
    analysis: Analysis,
    style: Style,
}

impl<'a> Emitter<'a> {
    fn new(evs: &'a [Ev]) -> Emitter<'a> {
        Emitter {
            evs,
            out: String::new(),
            state: State::DocumentContent,
            states: Vec::new(),
            indent: -1,
            indents: Vec::new(),
            flow_level: 0,
            column: 0,
            whitespace: true,
            indention: true,
            root_context: false,
            simple_key_context: false,
            analysis: Analysis::default(),
            style: Style::Plain,
        }
    }

    /// The stream: one implicit document, its node's events, the document's end.
    fn run(&mut self) {
        for i in 0..self.evs.len() {
            self.analyze(i);
            self.dispatch(i);
        }
        // The document's end: no `...`, as the encoder's are implicit, nor at the stream's end,
        // as its Close clears open_ended first.
        self.write_indent();
    }

    fn dispatch(&mut self, i: usize) {
        match self.state {
            State::DocumentContent => {
                self.states.push(State::DocumentEnd);
                self.emit_node(i, true, false);
            }
            // Nothing follows the root node.
            State::DocumentEnd => {}
            State::FlowSequenceFirstItem => self.flow_end(b']'),
            State::FlowMappingFirstKey => self.flow_end(b'}'),
            State::BlockSequenceFirstItem => self.block_sequence_item(i, true),
            State::BlockSequenceItem => self.block_sequence_item(i, false),
            State::BlockMappingFirstKey => self.block_mapping_key(i, true),
            State::BlockMappingKey => self.block_mapping_key(i, false),
            State::BlockMappingSimpleValue => self.block_mapping_value(i, true),
            State::BlockMappingValue => self.block_mapping_value(i, false),
        }
    }

    fn pop_state(&mut self) {
        self.state = self.states.pop().unwrap_or(State::DocumentEnd);
    }

    fn pop_indent(&mut self) {
        self.indent = self.indents.pop().unwrap_or(-1);
    }

    fn increase_indent(&mut self, flow: bool) {
        self.indents.push(self.indent);
        if self.indent < 0 {
            self.indent = if flow { BEST_INDENT } else { 0 };
        } else if self.states.last() == Some(&State::BlockSequenceItem) {
            self.indent += 2;
        } else {
            self.indent = BEST_INDENT * ((self.indent + BEST_INDENT) / BEST_INDENT);
        }
    }

    /// An empty flow collection, the only ones written: its opening and closing indicators.
    fn flow_end(&mut self, close: u8) {
        // The opening indicator, the indent and the flow level were taken when it started.
        self.flow_level = self.flow_level.saturating_sub(1);
        self.pop_indent();
        if close == b']' && self.column == 0 {
            self.write_indent();
        }
        self.write_indicator(if close == b']' { "]" } else { "}" }, false, false, false);
        self.pop_state();
    }

    fn flow_start(&mut self, open: &str, state: State) {
        self.write_indicator(open, true, true, false);
        self.increase_indent(true);
        self.flow_level += 1;
        self.state = state;
    }

    fn block_sequence_item(&mut self, i: usize, first: bool) {
        if first {
            self.increase_indent(false);
        }
        if matches!(self.evs.get(i), Some(Ev::SeqEnd)) {
            self.pop_indent();
            self.pop_state();
            return;
        }
        self.write_indent();
        self.write_indicator("-", true, false, true);
        self.states.push(State::BlockSequenceItem);
        self.emit_node(i, false, false);
    }

    fn block_mapping_key(&mut self, i: usize, first: bool) {
        if first {
            self.increase_indent(false);
        }
        if matches!(self.evs.get(i), Some(Ev::MapEnd)) {
            self.pop_indent();
            self.pop_state();
            return;
        }
        self.write_indent();
        if self.check_simple_key(i) {
            self.states.push(State::BlockMappingSimpleValue);
            self.emit_node(i, false, true);
        } else {
            self.write_indicator("?", true, false, true);
            self.states.push(State::BlockMappingValue);
            self.emit_node(i, false, false);
        }
    }

    fn block_mapping_value(&mut self, i: usize, simple: bool) {
        if simple {
            self.write_indicator(":", false, false, false);
        } else {
            self.write_indent();
            self.write_indicator(":", true, false, true);
        }
        self.states.push(State::BlockMappingKey);
        self.emit_node(i, false, false);
    }

    fn emit_node(&mut self, i: usize, root: bool, simple_key: bool) {
        self.root_context = root;
        self.simple_key_context = simple_key;
        match self.evs.get(i) {
            Some(Ev::Scalar { value, tag, style }) => self.emit_scalar(value, *tag, *style),
            Some(Ev::SeqStart) => {
                if self.flow_level > 0 || matches!(self.evs.get(i + 1), Some(Ev::SeqEnd)) {
                    self.flow_start("[", State::FlowSequenceFirstItem);
                } else {
                    self.state = State::BlockSequenceFirstItem;
                }
            }
            Some(Ev::MapStart) => {
                if self.flow_level > 0 || matches!(self.evs.get(i + 1), Some(Ev::MapEnd)) {
                    self.flow_start("{", State::FlowMappingFirstKey);
                } else {
                    self.state = State::BlockMappingFirstKey;
                }
            }
            _ => {}
        }
    }

    /// Whether the event at `i` can be written as a simple key: not on several lines, and at
    /// most 128 bytes with its tag.
    fn check_simple_key(&self, i: usize) -> bool {
        match self.evs.get(i) {
            Some(Ev::Scalar { value, tag, .. }) => {
                if self.analysis.multiline {
                    return false;
                }
                let tag_len = tag.map_or(0, |t| 2 + t.len());
                tag_len + value.len() <= 128
            }
            Some(Ev::SeqStart) => matches!(self.evs.get(i + 1), Some(Ev::SeqEnd)),
            Some(Ev::MapStart) => matches!(self.evs.get(i + 1), Some(Ev::MapEnd)),
            _ => false,
        }
    }

    fn analyze(&mut self, i: usize) {
        if let Some(Ev::Scalar { value, .. }) = self.evs.get(i) {
            self.analysis = analyze_scalar(value.as_bytes());
        }
    }

    fn emit_scalar(&mut self, value: &str, tag: Option<&'static str>, style: Style) {
        self.select_style(value, tag, style);
        if let Some(t) = tag {
            if !self.whitespace {
                self.put(b' ');
            }
            self.write_str("!!");
            self.write_str(t);
            self.whitespace = false;
            self.indention = false;
        }
        self.increase_indent(true);
        let v = value.as_bytes();
        match self.style {
            Style::Plain => self.write_plain(v),
            Style::Single => self.write_single(v),
            Style::Double => self.write_double(v),
            Style::Literal => self.write_literal(v),
        }
        self.pop_indent();
        self.pop_state();
    }

    fn select_style(&mut self, value: &str, tag: Option<&'static str>, style: Style) {
        let a = self.analysis;
        let mut style = style;
        if self.simple_key_context && a.multiline {
            style = Style::Double;
        }
        if style == Style::Plain {
            if self.flow_level > 0 && !a.flow_plain_allowed
                || self.flow_level == 0 && !a.block_plain_allowed
            {
                style = Style::Single;
            }
            if value.is_empty() && (self.flow_level > 0 || self.simple_key_context) {
                style = Style::Single;
            }
        }
        if style == Style::Single && !a.single_quoted_allowed {
            style = Style::Double;
        }
        if style == Style::Literal
            && (!a.block_allowed || self.flow_level > 0 || self.simple_key_context)
        {
            style = Style::Double;
        }
        let _ = tag;
        self.style = style;
    }

    // --- writing ---

    fn put(&mut self, c: u8) {
        self.out.push(char::from(c));
        self.column += 1;
    }

    fn put_break(&mut self) {
        self.out.push('\n');
        self.column = 0;
        self.indention = true;
    }

    /// Writes the character starting at byte `i` of `v`, and returns the byte after it.
    fn write(&mut self, v: &[u8], i: usize) -> usize {
        let w = width(v.get(i).copied().unwrap_or_default()).max(1);
        if let Some(s) = v.get(i..i + w).and_then(|c| std::str::from_utf8(c).ok()) {
            self.out.push_str(s);
        }
        self.column += 1;
        i + w
    }

    fn write_str(&mut self, s: &str) {
        let v = s.as_bytes();
        let mut i = 0;
        while i < v.len() {
            i = self.write(v, i);
        }
    }

    fn write_break(&mut self, v: &[u8], i: usize) -> usize {
        if v.get(i) == Some(&b'\n') {
            self.put_break();
            i + 1
        } else {
            let next = self.write(v, i);
            self.column = 0;
            self.indention = true;
            next
        }
    }

    fn write_indent(&mut self) {
        let indent = self.indent.max(0);
        if !self.indention || self.column > indent || (self.column == indent && !self.whitespace) {
            self.put_break();
        }
        while self.column < indent {
            self.put(b' ');
        }
        self.whitespace = true;
    }

    fn write_indicator(&mut self, ind: &str, need_ws: bool, is_ws: bool, is_indention: bool) {
        if need_ws && !self.whitespace {
            self.put(b' ');
        }
        self.write_str(ind);
        self.whitespace = is_ws;
        self.indention = self.indention && is_indention;
    }

    fn write_plain(&mut self, v: &[u8]) {
        if !v.is_empty() && !self.whitespace {
            self.put(b' ');
        }
        let mut breaks = false;
        let mut i = 0;
        while i < v.len() {
            if v.get(i) == Some(&b' ') {
                i = self.write(v, i);
            } else if is_break(v, i) {
                if !breaks && v.get(i) == Some(&b'\n') {
                    self.put_break();
                }
                i = self.write_break(v, i);
                breaks = true;
            } else {
                if breaks {
                    self.write_indent();
                }
                i = self.write(v, i);
                self.indention = false;
                breaks = false;
            }
        }
        if !v.is_empty() {
            self.whitespace = false;
        }
        self.indention = false;
        let _ = self.root_context;
    }

    fn write_single(&mut self, v: &[u8]) {
        self.write_indicator("'", true, false, false);
        let mut breaks = false;
        let mut i = 0;
        while i < v.len() {
            if v.get(i) == Some(&b' ') {
                i = self.write(v, i);
            } else if is_break(v, i) {
                if !breaks && v.get(i) == Some(&b'\n') {
                    self.put_break();
                }
                i = self.write_break(v, i);
                breaks = true;
            } else {
                if breaks {
                    self.write_indent();
                }
                if v.get(i) == Some(&b'\'') {
                    self.put(b'\'');
                }
                i = self.write(v, i);
                self.indention = false;
                breaks = false;
            }
        }
        self.write_indicator("'", false, false, false);
        self.whitespace = false;
        self.indention = false;
    }

    fn write_double(&mut self, v: &[u8]) {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        self.write_indicator("\"", true, false, false);
        // go-yaml's is_bom looks at the value's first bytes, wherever it is.
        let bom = v.starts_with(&[0xEF, 0xBB, 0xBF]);
        let mut i = 0;
        while i < v.len() {
            let c = v.get(i).copied().unwrap_or_default();
            if !is_printable(v, i) || bom || is_break(v, i) || c == b'"' || c == b'\\' {
                let w = width(c).max(1);
                let ch = v
                    .get(i..i + w)
                    .and_then(|s| std::str::from_utf8(s).ok())
                    .and_then(|s| s.chars().next())
                    .map_or(0, u32::from);
                i += w;
                self.put(b'\\');
                let simple = match ch {
                    0x00 => Some(b'0'),
                    0x07 => Some(b'a'),
                    0x08 => Some(b'b'),
                    0x09 => Some(b't'),
                    0x0A => Some(b'n'),
                    0x0B => Some(b'v'),
                    0x0C => Some(b'f'),
                    0x0D => Some(b'r'),
                    0x1B => Some(b'e'),
                    0x22 => Some(b'"'),
                    0x5C => Some(b'\\'),
                    0x85 => Some(b'N'),
                    0xA0 => Some(b'_'),
                    0x2028 => Some(b'L'),
                    0x2029 => Some(b'P'),
                    _ => None,
                };
                if let Some(s) = simple {
                    self.put(s);
                } else {
                    let (p, digits) = if ch <= 0xFF {
                        (b'x', 2)
                    } else if ch <= 0xFFFF {
                        (b'u', 4)
                    } else {
                        (b'U', 8)
                    };
                    self.put(p);
                    for k in (0..digits).rev() {
                        let d = (ch >> (k * 4)) & 0xF;
                        self.put(HEX.get(d as usize).copied().unwrap_or(b'0'));
                    }
                }
            } else {
                i = self.write(v, i);
            }
        }
        self.write_indicator("\"", false, false, false);
        self.whitespace = false;
        self.indention = false;
    }

    fn write_literal(&mut self, v: &[u8]) {
        self.write_indicator("|", true, false, false);
        if v.first() == Some(&b' ') || is_break(v, 0) {
            self.write_indicator("2", false, false, false);
        }
        let chomp = if v.is_empty() {
            Some("-")
        } else {
            let last = char_start(v, v.len() - 1);
            if !is_break(v, last) {
                Some("-")
            } else if last == 0 || is_break(v, char_start(v, last - 1)) {
                Some("+")
            } else {
                None
            }
        };
        if let Some(c) = chomp {
            self.write_indicator(c, false, false, false);
        }
        self.put_break();
        self.whitespace = true;
        let mut breaks = true;
        let mut i = 0;
        while i < v.len() {
            if is_break(v, i) {
                i = self.write_break(v, i);
                breaks = true;
            } else {
                if breaks {
                    self.write_indent();
                }
                i = self.write(v, i);
                self.indention = false;
                breaks = false;
            }
        }
    }
}

/// The byte length of the UTF-8 character starting with `b`.
fn width(b: u8) -> usize {
    if b & 0x80 == 0 {
        1
    } else if b & 0xE0 == 0xC0 {
        2
    } else if b & 0xF0 == 0xE0 {
        3
    } else if b & 0xF8 == 0xF0 {
        4
    } else {
        0
    }
}

/// The start of the character holding byte `i`.
fn char_start(v: &[u8], mut i: usize) -> usize {
    while i > 0 && v.get(i).is_some_and(|b| b & 0xC0 == 0x80) {
        i -= 1;
    }
    i
}

fn at(v: &[u8], i: usize) -> u8 {
    v.get(i).copied().unwrap_or_default()
}

fn is_printable(v: &[u8], i: usize) -> bool {
    let (b0, b1, b2) = (at(v, i), at(v, i + 1), at(v, i + 2));
    b0 == 0x0A
        || (0x20..=0x7E).contains(&b0)
        || (b0 == 0xC2 && b1 >= 0xA0)
        || (b0 > 0xC2 && b0 < 0xED)
        || (b0 == 0xED && b1 < 0xA0)
        || b0 == 0xEE
        || (b0 == 0xEF
            && !(b1 == 0xBB && b2 == 0xBF)
            && !(b1 == 0xBF && (b2 == 0xBE || b2 == 0xBF)))
}

fn is_break(v: &[u8], i: usize) -> bool {
    let (b0, b1, b2) = (at(v, i), at(v, i + 1), at(v, i + 2));
    b0 == b'\r'
        || b0 == b'\n'
        || (b0 == 0xC2 && b1 == 0x85)
        || (b0 == 0xE2 && b1 == 0x80 && (b2 == 0xA8 || b2 == 0xA9))
}

fn is_blank(v: &[u8], i: usize) -> bool {
    matches!(v.get(i), Some(b' ' | b'\t'))
}

/// go-yaml's `yaml_emitter_analyze_scalar`.
fn analyze_scalar(v: &[u8]) -> Analysis {
    if v.is_empty() {
        return Analysis {
            multiline: false,
            flow_plain_allowed: false,
            block_plain_allowed: true,
            single_quoted_allowed: true,
            block_allowed: false,
        };
    }
    let mut block_indicators = false;
    let mut flow_indicators = false;
    let mut line_breaks = false;
    let mut special_characters = false;
    let mut tab_characters = false;
    let mut leading_space = false;
    let mut leading_break = false;
    let mut trailing_space = false;
    let mut trailing_break = false;
    let mut break_space = false;
    let mut space_break = false;
    let mut previous_space = false;
    let mut previous_break = false;
    if v.starts_with(b"---") || v.starts_with(b"...") {
        block_indicators = true;
        flow_indicators = true;
    }
    let mut preceded_by_whitespace = true;
    let mut i = 0;
    while i < v.len() {
        let w = width(at(v, i)).max(1);
        let followed_by_whitespace = i + w >= v.len() || is_blank(v, i + w);
        let c = at(v, i);
        if i == 0 {
            match c {
                b'#' | b',' | b'[' | b']' | b'{' | b'}' | b'&' | b'*' | b'!' | b'|' | b'>'
                | b'\'' | b'"' | b'%' | b'@' | b'`' => {
                    flow_indicators = true;
                    block_indicators = true;
                }
                b'?' | b':' => {
                    flow_indicators = true;
                    if followed_by_whitespace {
                        block_indicators = true;
                    }
                }
                b'-' if followed_by_whitespace => {
                    flow_indicators = true;
                    block_indicators = true;
                }
                _ => {}
            }
        } else {
            match c {
                b',' | b'?' | b'[' | b']' | b'{' | b'}' => flow_indicators = true,
                b':' => {
                    flow_indicators = true;
                    if followed_by_whitespace {
                        block_indicators = true;
                    }
                }
                b'#' if preceded_by_whitespace => {
                    flow_indicators = true;
                    block_indicators = true;
                }
                _ => {}
            }
        }
        if c == b'\t' {
            tab_characters = true;
        } else if !is_printable(v, i) {
            special_characters = true;
        }
        if c == b' ' {
            if i == 0 {
                leading_space = true;
            }
            if i + w == v.len() {
                trailing_space = true;
            }
            if previous_break {
                break_space = true;
            }
            previous_space = true;
            previous_break = false;
        } else if is_break(v, i) {
            line_breaks = true;
            if i == 0 {
                leading_break = true;
            }
            if i + w == v.len() {
                trailing_break = true;
            }
            if previous_space {
                space_break = true;
            }
            previous_space = false;
            previous_break = true;
        } else {
            previous_space = false;
            previous_break = false;
        }
        preceded_by_whitespace = is_blank(v, i) || is_break(v, i) || c == 0;
        i += w;
    }
    let mut a = Analysis {
        multiline: line_breaks,
        flow_plain_allowed: true,
        block_plain_allowed: true,
        single_quoted_allowed: true,
        block_allowed: true,
    };
    if leading_space || leading_break || trailing_space || trailing_break {
        a.flow_plain_allowed = false;
        a.block_plain_allowed = false;
    }
    if trailing_space {
        a.block_allowed = false;
    }
    if break_space {
        a.flow_plain_allowed = false;
        a.block_plain_allowed = false;
        a.single_quoted_allowed = false;
    }
    if space_break || tab_characters || special_characters {
        a.flow_plain_allowed = false;
        a.block_plain_allowed = false;
        a.single_quoted_allowed = false;
    }
    if space_break || special_characters {
        a.block_allowed = false;
    }
    if line_breaks {
        a.flow_plain_allowed = false;
        a.block_plain_allowed = false;
    }
    if flow_indicators {
        a.flow_plain_allowed = false;
    }
    if block_indicators {
        a.block_plain_allowed = false;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::{Tag, resolve};

    #[test]
    fn resolver() {
        for (s, t) in [
            ("", Tag::Null),
            ("true", Tag::Bool),
            ("yes", Tag::Str),
            ("1", Tag::Int),
            ("0x1F", Tag::Int),
            ("08", Tag::Float),
            ("1_000", Tag::Int),
            ("1e3", Tag::Float),
            ("2e308", Tag::Str),
            (".5", Tag::Float),
            (".5_0", Tag::Float),
            ("._5", Tag::Str),
            ("0b-1", Tag::Int),
            ("2024-01-02", Tag::Timestamp),
            ("2024-02-30", Tag::Str),
            ("2024-01-02  10:00:00", Tag::Timestamp),
            ("<<", Tag::Str),
            ("~", Tag::Null),
        ] {
            assert_eq!(resolve(s), t, "{s:?}");
        }
    }
}
