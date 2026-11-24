//! What providers provide, merged into the base config: fit's `provider.Parse`, `provider.Merge`
//! and `Merged.Errors`, by the rules of fit's decision 009.
//!
//! - A provider sends a [`Part`]: a JSON object whose members are among [`KINDS`], each an array
//!   of objects, at most [`MAX_PART_BYTES`].
//! - [`merge`] adds the parts' elements, as written, after the base config's in each array,
//!   provider by provider in their order of precedence. A named element (a listener, upstream or
//!   service, or a route with a name) whose name the base config, or a provider before it,
//!   already uses is left out and recorded as [`Shadowed`]. The base config's text, and so its
//!   indexes and positions, is kept as written.
//! - [`Merged::errors`] maps the errors of the merged config back to where they were written:
//!   in the base config, or in a provider's part, saying `(provided by <name>)`.
//!
//! Offsets are bounded by the lengths of the texts, so the arithmetic below cannot overflow.
#![allow(clippy::arithmetic_side_effects)]

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use akit_config::value::{self, Value};
use akit_config::{Error, Errors};

/// The members of a config a part may hold: the arrays whose elements are merged.
pub const KINDS: [&str; 4] = ["listeners", "routes", "upstreams", "services"];

/// The largest part: one larger is refused. 16 MiB, as fit's `MaxPartBytes`.
pub const MAX_PART_BYTES: usize = 16 << 20;

/// What a provider provides, as [`parse`] read it.
#[derive(Debug)]
pub struct Part {
    provider: Arc<str>,
    data: String,
    doc: Doc,
}

impl Part {
    /// The provider it came from.
    #[must_use]
    pub fn provider(&self) -> &Arc<str> {
        &self.provider
    }

    /// The part as written: standard JSON, comments and trailing commas blanked out.
    #[must_use]
    pub fn data(&self) -> &str {
        &self.data
    }

    /// Its size as written.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.data.len()
    }
}

/// Reads `data`, what the provider `name` provides: a JSON object whose members are among
/// [`KINDS`], each an array of objects. Empty data (or white space) provides nothing. Comments
/// and trailing commas are allowed, as in a config file, and blanked out.
///
/// # Errors
///
/// What is wrong with it, with positions in `data`.
pub fn parse(name: &str, data: &[u8]) -> Result<Part, Errors> {
    if data.len() > MAX_PART_BYTES {
        return Err(error(
            "",
            0,
            "",
            format!("the part is larger than {MAX_PART_BYTES} bytes"),
        ));
    }
    let text = std::str::from_utf8(data).map_err(|e| {
        error(
            "",
            0,
            "",
            format!("the part is not UTF-8 (at byte {})", e.valid_up_to()),
        )
    })?;
    let text = if text.trim().is_empty() { "{}" } else { text };
    let data = value::standardize(text)?;
    let doc = scan(&data, true)?;
    Ok(Part {
        provider: Arc::from(name),
        data,
        doc,
    })
}

/// A config, or a part, scanned for what merging needs: its top-level members, where its object
/// ends, and where the arrays of [`KINDS`] and their elements are.
#[derive(Debug, Default)]
struct Doc {
    /// The top-level members, in order.
    keys: Vec<String>,
    /// The offset of the top-level object's `}`.
    end: usize,
    /// By the index of their kind in [`KINDS`]; `None` when left out.
    arrays: [Option<Array>; 4],
}

#[derive(Debug)]
struct Array {
    /// The offsets of `[` and `]`.
    open: usize,
    close: usize,
    elems: Vec<Element>,
}

#[derive(Debug)]
struct Element {
    /// Where it is in its document.
    start: usize,
    end: usize,
    /// Its name, when it has one that is not empty.
    name: Option<String>,
}

/// One problem at byte `off` of `data`.
fn error(data: &str, off: usize, path: impl Into<String>, msg: impl Into<String>) -> Errors {
    let (line, col) = position(data, off);
    Errors(vec![Error {
        file: None,
        path: path.into(),
        line,
        col,
        msg: msg.into(),
    }])
}

fn skip_space(b: &[u8], mut i: usize) -> usize {
    while matches!(b.get(i), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        i += 1;
    }
    i
}

/// The offset after the string that starts at `i`.
fn string_end(b: &[u8], mut i: usize) -> usize {
    i += 1;
    loop {
        match b.get(i) {
            None => return b.len(),
            Some(b'\\') => i += 2,
            Some(b'"') => return i + 1,
            Some(_) => i += 1,
        }
    }
}

/// The offset after the value that starts at `i`, in standard JSON.
fn value_end(b: &[u8], i: usize) -> usize {
    match b.get(i) {
        Some(b'"') => string_end(b, i),
        Some(b'{' | b'[') => {
            let mut depth = 0usize;
            let mut j = i;
            loop {
                match b.get(j) {
                    None => return b.len(),
                    Some(b'"') => {
                        j = string_end(b, j);
                        continue;
                    }
                    Some(b'{' | b'[') => depth += 1,
                    Some(b'}' | b']') => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            return j + 1;
                        }
                    }
                    Some(_) => {}
                }
                j += 1;
            }
        }
        _ => {
            let mut j = i;
            while let Some(c) = b.get(j) {
                if matches!(c, b',' | b']' | b'}' | b' ' | b'\t' | b'\r' | b'\n') {
                    break;
                }
                j += 1;
            }
            j.max(i + 1)
        }
    }
}

/// Scans `data`, standard JSON. A part may hold only [`KINDS`], and their elements must be
/// objects whose names, when given, are strings; a base config may hold anything, but the
/// members of [`KINDS`] it has must be arrays.
fn scan(data: &str, part: bool) -> Result<Doc, Errors> {
    let b = data.as_bytes();
    let not_json = |i: usize| error(data, i, "", "invalid JSON");
    let mut d = Doc::default();
    let mut i = skip_space(b, 0);
    if b.get(i) != Some(&b'{') {
        return Err(error(data, i, "", "must be a JSON object"));
    }
    i += 1;
    loop {
        i = skip_space(b, i);
        match b.get(i) {
            Some(b'}') => break,
            Some(b',') => i = skip_space(b, i + 1),
            Some(b'"') => {}
            _ => return Err(not_json(i)),
        }
        let key_end = string_end(b, i);
        let key = match data.get(i..key_end).map(value::parse) {
            Some(Ok(Value::String(k))) => k,
            _ => return Err(not_json(i)),
        };
        i = skip_space(b, key_end);
        if b.get(i) != Some(&b':') {
            return Err(not_json(i));
        }
        i = skip_space(b, i + 1);
        let kind = KINDS.iter().position(|k| *k == key);
        d.keys.push(key.clone());
        let Some(kind) = kind else {
            if part {
                return Err(error(
                    data,
                    key_end,
                    key,
                    "a provider may provide only listeners, routes, upstreams and services",
                ));
            }
            i = value_end(b, i);
            continue;
        };
        if b.get(i) != Some(&b'[') {
            return Err(error(
                data,
                key_end,
                key,
                "must be an array, for what providers provide to be merged into it",
            ));
        }
        let open = i;
        i += 1;
        let mut elems = Vec::new();
        loop {
            i = skip_space(b, i);
            match b.get(i) {
                Some(b']') => break,
                Some(b',') => i = skip_space(b, i + 1),
                None => return Err(not_json(i)),
                Some(_) => {}
            }
            let end = value_end(b, i);
            let path = format!("{key}[{}]", elems.len());
            elems.push(element(data, i, end, part, &path)?);
            i = end;
        }
        if let Some(slot) = d.arrays.get_mut(kind) {
            *slot = Some(Array {
                open,
                close: i,
                elems,
            });
        }
        i += 1;
    }
    d.end = i;
    if skip_space(b, i + 1) < b.len() {
        return Err(error(
            data,
            skip_space(b, i + 1),
            "",
            "has more after the top-level object",
        ));
    }
    Ok(d)
}

/// The element at `start..end` of `data`, at `path`.
fn element(
    data: &str,
    start: usize,
    end: usize,
    part: bool,
    path: &str,
) -> Result<Element, Errors> {
    let unnamed = Element {
        start,
        end,
        name: None,
    };
    let v = match data.get(start..end).map(value::parse) {
        Some(Ok(v @ Value::Object(_))) => v,
        _ if part => return Err(error(data, start, path, "must be an object")),
        _ => return Ok(unnamed),
    };
    match v.get("name") {
        Some(Value::String(n)) => Ok(Element {
            name: (!n.is_empty()).then(|| n.clone()),
            ..unnamed
        }),
        Some(_) if part => Err(error(
            data,
            start,
            format!("{path}.name"),
            "must be a string",
        )),
        _ => Ok(unnamed),
    }
}

/// Where an element, or a member, of a merged config came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// A JSON pointer to it in the merged config.
    pub path: String,
    /// The provider it came from, or the source of the base config (`file`, `admin`, ...).
    pub source: Arc<str>,
    /// A JSON pointer to it in what its provider provided; empty for the base config.
    pub from: String,
}

/// A provider's element left out of a merged config because its name was taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shadowed {
    /// The provider.
    pub source: Arc<str>,
    /// A JSON pointer to the element in what it provided.
    pub from: String,
    /// The name.
    pub name: String,
    /// The source whose element of that name is in the config.
    pub by: Arc<str>,
}

/// A span of the merged config and where it came from.
#[derive(Debug, Clone, Copy)]
struct Segment {
    /// In [`Merged::data`].
    start: usize,
    end: usize,
    /// The part (its index among the parts merged) it came from; `None` for the base config, or
    /// for the commas and keys merging writes.
    part: Option<usize>,
    /// Where it starts in the base config or in its part; `None` for what merging writes.
    from: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
struct Origin {
    /// Its part's index among the parts merged.
    part: usize,
    /// Its index in the part's array.
    index: usize,
}

/// A base config with parts merged into it.
#[derive(Debug)]
pub struct Merged {
    data: Arc<str>,
    sources: Vec<Source>,
    shadowed: Vec<Shadowed>,
    base: Option<(Arc<str>, Doc)>,
    parts: Vec<Arc<Part>>,
    segments: Vec<Segment>,
    /// The elements added of each kind, in order.
    origins: [Vec<Origin>; 4],
}

/// The JSON pointer to element `i` of `kind`.
fn pointer(kind: &str, i: usize) -> String {
    format!("/{kind}/{i}")
}

/// `key` as a JSON pointer's token.
fn token(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

impl Merged {
    /// `data`, a config nothing was merged into, and which was not scanned: its errors are left
    /// as they are, and it has no sources.
    #[must_use]
    pub fn plain(data: Arc<str>) -> Merged {
        Merged {
            data,
            sources: Vec::new(),
            shadowed: Vec::new(),
            base: None,
            parts: Vec::new(),
            segments: Vec::new(),
            origins: Default::default(),
        }
    }

    /// The merged config: the base config as written, with the parts' elements, as written,
    /// added to its arrays. The base config itself when nothing was added.
    #[must_use]
    pub fn data(&self) -> &Arc<str> {
        &self.data
    }

    /// Where each member of the merged config came from, element by element in its arrays, in
    /// the order of the merged config.
    #[must_use]
    pub fn sources(&self) -> &[Source] {
        &self.sources
    }

    /// What the parts provided and the merged config leaves out, because a source before them
    /// took its name.
    #[must_use]
    pub fn shadowed(&self) -> &[Shadowed] {
        &self.shadowed
    }

    /// Takes the sources and what was shadowed.
    #[must_use]
    pub fn into_parts(self) -> (Arc<str>, Vec<Source>, Vec<Shadowed>) {
        (self.data, self.sources, self.shadowed)
    }

    /// Maps `errs`, what loading or compiling [`data`](Merged::data) gave, to where they were
    /// written: an error in the base config gets its position there, and one in a provider's
    /// element gets its path and position in what that provider provided, and says which
    /// provider that was.
    #[must_use]
    pub fn errors(&self, errs: Errors) -> Errors {
        if self.base.is_none() {
            return errs;
        }
        Errors(errs.0.into_iter().map(|e| self.map_error(e)).collect())
    }

    fn map_error(&self, e: Error) -> Error {
        let Some((base, base_doc)) = &self.base else {
            return e;
        };
        let mut me = e;
        let mut seg = None;
        let mut off = 0;
        if me.line > 0 {
            off = offset(&self.data, me.line, me.col);
            seg = self
                .segments
                .binary_search_by(|s| {
                    if s.end <= off {
                        std::cmp::Ordering::Less
                    } else if s.start > off {
                        std::cmp::Ordering::Greater
                    } else {
                        std::cmp::Ordering::Equal
                    }
                })
                .ok()
                .and_then(|i| self.segments.get(i));
        }
        let mut part = None;
        if let Some((k, n, rest)) = element_path(&me.path) {
            let n = n.checked_sub(
                base_doc
                    .arrays
                    .get(k)
                    .and_then(Option::as_ref)
                    .map_or(0, |a| a.elems.len()),
            );
            if let Some(o) = n.and_then(|n| self.origins.get(k).and_then(|os| os.get(n))) {
                part = Some(o.part);
                let tail = me.path.get(rest..).unwrap_or("").to_owned();
                me.path = format!("{}[{}]{tail}", KINDS.get(k).copied().unwrap_or(""), o.index);
            }
        } else if let Some(s) = seg {
            part = s.part;
        }
        me.line = 0;
        me.col = 0;
        if let Some(s) = seg {
            if let Some(from) = s.from {
                let at = from + off.saturating_sub(s.start);
                match (part, s.part) {
                    (None, None) => (me.line, me.col) = position(base, at),
                    (Some(p), Some(sp)) if p == sp => {
                        if let Some(p) = self.parts.get(p) {
                            (me.line, me.col) = position(&p.data, at);
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(p) = part.and_then(|p| self.parts.get(p)) {
            let _ = write!(me.msg, " (provided by {})", p.provider);
        }
        me
    }
}

/// The kind, the index and the length of the prefix of a path that starts in an element of one
/// of [`KINDS`], as `routes[3]`.
fn element_path(path: &str) -> Option<(usize, usize, usize)> {
    KINDS.iter().enumerate().find_map(|(k, kind)| {
        let rest = path.strip_prefix(kind)?.strip_prefix('[')?;
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || rest.as_bytes().get(digits) != Some(&b']') {
            return None;
        }
        let n = rest.get(..digits)?.parse().ok()?;
        Some((k, n, kind.len() + digits + 2))
    })
}

/// Merges `parts` into `base`, the base config, whose source is `base_source`; parts come in
/// their order of precedence, the first winning. When nothing is added, the merged config is
/// `base` itself.
///
/// # Errors
///
/// When `base` is not a JSON object, or one of its members of [`KINDS`] is not an array.
pub fn merge(
    base: &Arc<str>,
    base_source: &Arc<str>,
    parts: &[Arc<Part>],
) -> Result<Merged, Errors> {
    // Standard JSON, as the base configs kept are, is kept as it is (and shared).
    let std = value::standardize(base)?;
    let base: Arc<str> = if *std == **base {
        Arc::clone(base)
    } else {
        Arc::from(std)
    };
    let bd = scan(&base, false)?;
    let mut m = Merged {
        data: Arc::clone(&base),
        sources: Vec::new(),
        shadowed: Vec::new(),
        base: None,
        parts: parts.to_vec(),
        segments: Vec::new(),
        origins: Default::default(),
    };

    // What each part adds, kind by kind, and what it leaves out.
    let mut picks: [Vec<(usize, usize)>; 4] = Default::default();
    for (k, kind_picks) in picks.iter_mut().enumerate() {
        let mut taken: HashMap<&str, &Arc<str>> = HashMap::new();
        if let Some(Some(a)) = bd.arrays.get(k) {
            for name in a.elems.iter().filter_map(|e| e.name.as_deref()) {
                taken.insert(name, base_source);
            }
        }
        for (pi, p) in parts.iter().enumerate() {
            let Some(Some(a)) = p.doc.arrays.get(k) else {
                continue;
            };
            for (i, e) in a.elems.iter().enumerate() {
                if let Some(name) = e.name.as_deref() {
                    match taken.get(name) {
                        Some(&by) if **by != *p.provider => {
                            m.shadowed.push(Shadowed {
                                source: Arc::clone(&p.provider),
                                from: pointer(KINDS.get(k).copied().unwrap_or(""), i),
                                name: name.to_owned(),
                                by: Arc::clone(by),
                            });
                            continue;
                        }
                        _ => {
                            taken.insert(name, &p.provider);
                        }
                    }
                }
                kind_picks.push((pi, i));
            }
        }
    }

    // Where the picks go into the base config: at the end of an array, after its last element
    // or inside it when empty, or, for a kind the base config leaves out, as a member added at
    // the end of the object.
    let mut ins: Vec<(usize, usize)> = Vec::new();
    for (k, kind_picks) in picks.iter().enumerate() {
        if kind_picks.is_empty() {
            continue;
        }
        let at = match bd.arrays.get(k).and_then(Option::as_ref) {
            Some(a) if a.elems.is_empty() => a.open + 1,
            Some(a) => a.close,
            None => bd.end,
        };
        ins.push((at, k));
    }
    ins.sort_by_key(|(at, _)| *at);

    if ins.is_empty() {
        m.segments.push(Segment {
            start: 0,
            end: base.len(),
            part: None,
            from: Some(0),
        });
    } else {
        let n = base.len() + parts.iter().map(|p| p.bytes()).sum::<usize>();
        let mut out = String::with_capacity(n);
        let mut from = 0usize;
        let mut members = bd.keys.len();
        let mut segments = Vec::new();
        let copy_base =
            |out: &mut String, segments: &mut Vec<Segment>, from: &mut usize, to: usize| {
                if to > *from {
                    segments.push(Segment {
                        start: out.len(),
                        end: out.len() + to - *from,
                        part: None,
                        from: Some(*from),
                    });
                    out.push_str(base.get(*from..to).unwrap_or(""));
                    *from = to;
                }
            };
        let glue = |out: &mut String, segments: &mut Vec<Segment>, s: &str| {
            segments.push(Segment {
                start: out.len(),
                end: out.len() + s.len(),
                part: None,
                from: None,
            });
            out.push_str(s);
        };
        for &(at, k) in &ins {
            copy_base(&mut out, &mut segments, &mut from, at);
            let a = bd.arrays.get(k).and_then(Option::as_ref);
            let mut comma = a.is_some_and(|a| !a.elems.is_empty());
            let kind = KINDS.get(k).copied().unwrap_or("");
            if a.is_none() {
                if members > 0 {
                    glue(&mut out, &mut segments, ",");
                }
                glue(&mut out, &mut segments, &format!("\"{kind}\":["));
                members += 1;
            }
            for &(pi, i) in picks.get(k).map_or(&[][..], Vec::as_slice) {
                let Some(p) = parts.get(pi) else { continue };
                let Some(e) = p
                    .doc
                    .arrays
                    .get(k)
                    .and_then(Option::as_ref)
                    .and_then(|a| a.elems.get(i))
                else {
                    continue;
                };
                if comma {
                    glue(&mut out, &mut segments, ",");
                }
                comma = true;
                let raw = p.data.get(e.start..e.end).unwrap_or("");
                segments.push(Segment {
                    start: out.len(),
                    end: out.len() + raw.len(),
                    part: Some(pi),
                    from: Some(e.start),
                });
                out.push_str(raw);
                if let Some(os) = m.origins.get_mut(k) {
                    os.push(Origin { part: pi, index: i });
                }
            }
            if a.is_none() {
                glue(&mut out, &mut segments, "]");
            }
        }
        copy_base(&mut out, &mut segments, &mut from, base.len());
        m.data = Arc::from(out);
        m.segments = segments;
    }

    // The sources, in the order of the merged config's members.
    let mut keys: Vec<&str> = bd.keys.iter().map(String::as_str).collect();
    for &(_, k) in &ins {
        if bd.arrays.get(k).is_some_and(Option::is_none) {
            keys.push(KINDS.get(k).copied().unwrap_or(""));
        }
    }
    for key in keys {
        let Some(k) = KINDS.iter().position(|x| *x == key) else {
            m.sources.push(Source {
                path: format!("/{}", token(key)),
                source: Arc::clone(base_source),
                from: String::new(),
            });
            continue;
        };
        let mut n = 0;
        if let Some(Some(a)) = bd.arrays.get(k) {
            for _ in &a.elems {
                m.sources.push(Source {
                    path: pointer(key, n),
                    source: Arc::clone(base_source),
                    from: String::new(),
                });
                n += 1;
            }
        }
        for o in m.origins.get(k).map_or(&[][..], Vec::as_slice) {
            if let Some(p) = parts.get(o.part) {
                m.sources.push(Source {
                    path: pointer(key, n),
                    source: Arc::clone(&p.provider),
                    from: pointer(key, o.index),
                });
            }
            n += 1;
        }
    }
    m.base = Some((base, bd));
    Ok(m)
}

/// The 1-based line and column, counted in characters, of byte `off` in `data`, as config
/// errors give them.
fn position(data: &str, off: usize) -> (u32, u32) {
    let mut off = off.min(data.len());
    while !data.is_char_boundary(off) {
        off -= 1;
    }
    let before = data.get(..off).unwrap_or("");
    let line = before.bytes().filter(|&b| b == b'\n').count() + 1;
    let start = before.rfind('\n').map_or(0, |n| n + 1);
    let col = before.get(start..).map_or(0, |s| s.chars().count()) + 1;
    (
        u32::try_from(line).unwrap_or(u32::MAX),
        u32::try_from(col).unwrap_or(u32::MAX),
    )
}

/// [`position`]'s inverse.
fn offset(data: &str, line: u32, col: u32) -> usize {
    let mut off = 0usize;
    for _ in 1..line {
        match data.get(off..).and_then(|s| s.find('\n')) {
            Some(i) => off += i + 1,
            None => return data.len(),
        }
    }
    let rest = data.get(off..).unwrap_or("");
    for (n, (i, c)) in rest.char_indices().enumerate() {
        if c == '\n' || n + 1 >= usize::try_from(col).unwrap_or(usize::MAX) {
            return off + i;
        }
    }
    data.len()
}

#[cfg(test)]
mod tests;
