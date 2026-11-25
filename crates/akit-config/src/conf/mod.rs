//! fit's own config format, `.conf` (fit's decision 021, `docs/reference/fit.md`): blocks of
//! directives, each a field of the config, read into the same tree the JSON reader gives, so that
//! one decoder reads both and a `.conf` file says exactly what its JSON twin says.
//!
//! As fit's `adapter/fitconf`: the lexer and parser here give the file's directives as written;
//! [`reader`] reads `def` and `use`, and expands each `site` into its routes; [`convert`] turns
//! the directives into the JSON tree, each value typed by its field in fit's config types (fit's
//! field inventory, pinned with the compatibility gate's fixtures). Every node keeps the byte
//! offset in the `.conf` text it was written at, so the decoder's problems point at the line and
//! column of the `.conf` file, as fit's do.
//!
//! What fit reads and akit does not yet: `include` and `import`, which read other files from a
//! `.conf` file, are refused as not supported yet.

// Offsets are bounded by the input's length, and counts by MAX_TOKENS, so the arithmetic in the
// lexer and parser below cannot overflow.
#![allow(clippy::arithmetic_side_effects)]

pub(crate) mod convert;
mod reader;

use crate::json::{Node, ParseError};

/// The most words (and ends of directives) a config may hold, its defs' uses counted, as fit's.
pub(crate) const MAX_TOKENS: usize = 1 << 20;
/// How deep blocks may nest, as fit's.
pub(crate) const MAX_DEPTH: usize = 64;

/// Looks up an environment variable, for `{$NAME}`.
pub(crate) type Env<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Reads a `.conf` config into the JSON tree its JSON twin gives, `{$NAME}` read with `env`.
pub(crate) fn parse(src: &str, env: Env<'_>) -> Result<Node, ParseError> {
    let items = lex(src, env)?;
    let words = items.len();
    let ds = parse_items(items)?;
    let ds = reader::Reader::new(words).splice(ds, true)?;
    let ds = reader::expand_sites(ds)?;
    convert::root(ds)
}

/// The directives `src` holds as written, as fit's `parse`: no `def`, `use` or `site` read, and
/// `{$NAME}` read as empty, for writing a file again (`akit adapt`, I73).
pub(crate) fn directives(src: &str) -> Result<Vec<Directive>, ParseError> {
    parse_items(lex(src, &|_| None)?)
}

/// Whether `src` holds a comment, as the lexer reads one: a `#` beginning a word, to the end of
/// its line. Writing the file again loses it (`akit fmt`, I73).
pub(crate) fn has_comment(src: &str) -> bool {
    let b = src.as_bytes();
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        match c {
            b'#' => return true,
            b'\n' | b';' | b' ' | b'\t' | b'\r' => i += 1,
            b'"' => {
                i += 1;
                while let Some(&c) = b.get(i) {
                    i += 1;
                    match c {
                        b'\\' => i += 1,
                        b'"' => break,
                        _ => {}
                    }
                }
            }
            b'`' => i = find(b, i + 1, b'`').map_or(b.len(), |e| e + 1),
            _ => {
                while b
                    .get(i)
                    .is_some_and(|c| !matches!(c, b' ' | b'\t' | b'\r' | b'\n'))
                {
                    i += 1;
                }
            }
        }
    }
    false
}

/// A word of the file: a directive's name or one of its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Tok {
    pub(crate) text: String,
    /// Written in quotes or backquotes: always a string, never a brace or a keyword.
    pub(crate) quoted: bool,
    /// The byte offset it starts at.
    pub(crate) off: usize,
}

impl Tok {
    /// A word made by expanding a shorthand, at `off`.
    pub(crate) fn made(text: &str, off: usize) -> Tok {
        Tok {
            text: text.to_owned(),
            quoted: false,
            off,
        }
    }
}

/// A directive: a name, its arguments, and the block after them, when there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Directive {
    pub(crate) name: Tok,
    pub(crate) args: Vec<Tok>,
    pub(crate) block: Vec<Directive>,
    pub(crate) has_block: bool,
}

pub(crate) fn err(off: usize, msg: impl Into<String>) -> ParseError {
    ParseError {
        off,
        msg: msg.into(),
    }
}

/// What the lexer gives the parser.
#[derive(Debug)]
enum Item {
    Word(Tok),
    /// The end of a directive: a new line or a `;`.
    End,
    Open(usize),
    Close(usize),
}

/// Splits `src` into items, as fit's lexer: a `#` beginning a word comments to the end of the
/// line; `"double quotes"` hold spaces and the escapes `\"`, `\\`, `\n`, `\t`, `\r` and `\{`;
/// `` `backquotes` `` hold anything as it is; `{` and `}` alone are braces, `{}` an empty block;
/// a `;` ending a word ends the directive; `{$NAME}` and `{$NAME:default}`, outside backquotes,
/// are the environment variable's value.
fn lex(src: &str, env: Env<'_>) -> Result<Vec<Item>, ParseError> {
    let b = src.as_bytes();
    let mut items: Vec<Item> = Vec::new();
    let push = |items: &mut Vec<Item>, it: Item, off: usize| {
        if items.len() >= MAX_TOKENS {
            return Err(err(
                off,
                format!("the file holds more than {MAX_TOKENS} words"),
            ));
        }
        items.push(it);
        Ok(())
    };
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        match c {
            b'\n' | b';' => {
                push(&mut items, Item::End, i)?;
                i += 1;
            }
            b' ' | b'\t' | b'\r' => i += 1,
            b'#' => {
                i = find(b, i, b'\n').unwrap_or(b.len());
            }
            b'"' => {
                let start = i;
                let mut out: Vec<u8> = Vec::new();
                let mut j = i + 1;
                loop {
                    match b.get(j) {
                        None => return Err(err(start, "a quote is not closed")),
                        Some(b'"') => break,
                        Some(b'{') if b.get(j + 1) == Some(&b'$') => {
                            let Some(end) = find(b, j, b'}') else {
                                return Err(err(start, "{$ is not closed with }"));
                            };
                            let v = expand_env(src.get(j..=end).unwrap_or_default(), start, env)?;
                            out.extend_from_slice(v.as_bytes());
                            j = end + 1;
                        }
                        Some(b'\\') if j + 1 < b.len() => {
                            j += 1;
                            match b.get(j).copied().unwrap_or_default() {
                                e @ (b'"' | b'\\' | b'{') => out.push(e),
                                b'n' => out.push(b'\n'),
                                b't' => out.push(b'\t'),
                                b'r' => out.push(b'\r'),
                                e => {
                                    out.push(b'\\');
                                    out.push(e);
                                }
                            }
                            j += 1;
                        }
                        Some(&x) => {
                            out.push(x);
                            j += 1;
                        }
                    }
                }
                i = j + 1;
                // Bytes copied from valid UTF-8, split only at ASCII: still valid.
                let text = String::from_utf8_lossy(&out).into_owned();
                push_quoted(&mut items, &push, text, start, b, i)?;
            }
            b'`' => {
                let start = i;
                let Some(end) = find(b, i + 1, b'`') else {
                    return Err(err(start, "a backquote is not closed"));
                };
                let text = src.get(i + 1..end).unwrap_or_default().to_owned();
                i = end + 1;
                push_quoted(&mut items, &push, text, start, b, i)?;
            }
            _ => {
                let start = i;
                let mut j = i;
                while b
                    .get(j)
                    .is_some_and(|c| !matches!(c, b' ' | b'\t' | b'\r' | b'\n'))
                {
                    j += 1;
                }
                let mut word = src.get(i..j).unwrap_or_default();
                i = j;
                let mut end = false;
                if word.len() > 1 {
                    if let Some(w) = word.strip_suffix(';') {
                        word = w;
                        end = true;
                    }
                }
                match word {
                    "{" => push(&mut items, Item::Open(start), start)?,
                    "}" => push(&mut items, Item::Close(start), start)?,
                    "{}" => {
                        push(&mut items, Item::Open(start), start)?;
                        push(&mut items, Item::Close(start + 1), start)?;
                    }
                    _ => {
                        let text = expand_env(word, start, env)?;
                        push(
                            &mut items,
                            Item::Word(Tok {
                                text,
                                quoted: false,
                                off: start,
                            }),
                            start,
                        )?;
                    }
                }
                if end {
                    push(&mut items, Item::End, start)?;
                }
            }
        }
    }
    Ok(items)
}

type Push = dyn Fn(&mut Vec<Item>, Item, usize) -> Result<(), ParseError>;

/// Pushes a quoted word; what follows it, at `next`, must end it.
fn push_quoted(
    items: &mut Vec<Item>,
    push: &Push,
    text: String,
    start: usize,
    b: &[u8],
    next: usize,
) -> Result<(), ParseError> {
    push(
        items,
        Item::Word(Tok {
            text,
            quoted: true,
            off: start,
        }),
        start,
    )?;
    if b.get(next)
        .is_some_and(|c| !matches!(c, b' ' | b'\t' | b'\r' | b'\n' | b';'))
    {
        return Err(err(
            next,
            "a quoted word must end at a space, a new line or a ;",
        ));
    }
    Ok(())
}

/// The offset of the first `c` in `b` at or after `from`.
fn find(b: &[u8], from: usize, c: u8) -> Option<usize> {
    b.get(from..)?
        .iter()
        .position(|&x| x == c)
        .map(|p| from + p)
}

/// `s` with `{$NAME}` and `{$NAME:default}` replaced; `off` is where `s` was written.
fn expand_env(s: &str, off: usize, env: Env<'_>) -> Result<String, ParseError> {
    if !s.contains("{$") {
        return Ok(s.to_owned());
    }
    let mut out = String::new();
    let mut s = s;
    loop {
        let Some(i) = s.find("{$") else {
            out.push_str(s);
            return Ok(out);
        };
        let Some(j) = s.get(i..).and_then(|r| r.find('}')).map(|j| i + j) else {
            return Err(err(off, "{$ is not closed with }"));
        };
        let inner = s.get(i + 2..j).unwrap_or_default();
        let (name, default) = match inner.split_once(':') {
            Some((n, d)) => (n, Some(d)),
            None => (inner, None),
        };
        if name.is_empty() {
            return Err(err(off, "{$} names no environment variable"));
        }
        let v = env(name).or_else(|| default.map(str::to_owned));
        out.push_str(s.get(..i).unwrap_or_default());
        out.push_str(&v.unwrap_or_default());
        s = s.get(j + 1..).unwrap_or_default();
    }
}

/// Parses a file's items into its directives.
fn parse_items(items: Vec<Item>) -> Result<Vec<Directive>, ParseError> {
    let mut p = Parser {
        items: items.into_iter().peekable(),
    };
    let ds = p.block(0)?;
    if let Some(it) = p.items.next() {
        let off = match it {
            Item::Word(t) => t.off,
            Item::Open(o) | Item::Close(o) => o,
            Item::End => 0,
        };
        return Err(err(off, "a } closes no block"));
    }
    Ok(ds)
}

struct Parser {
    items: std::iter::Peekable<std::vec::IntoIter<Item>>,
}

impl Parser {
    /// Parses directives up to a `}` or the end, which it does not consume.
    fn block(&mut self, depth: usize) -> Result<Vec<Directive>, ParseError> {
        let mut ds = Vec::new();
        while let Some(it) = self.items.peek() {
            let name = match it {
                Item::End => {
                    self.items.next();
                    continue;
                }
                Item::Close(_) => return Ok(ds),
                Item::Open(o) => return Err(err(*o, "a block needs a directive before its {")),
                Item::Word(t) => t.clone(),
            };
            self.items.next();
            let mut d = Directive {
                name,
                args: Vec::new(),
                block: Vec::new(),
                has_block: false,
            };
            while let Some(it) = self.items.peek() {
                match it {
                    Item::Word(t) => {
                        d.args.push(t.clone());
                        self.items.next();
                    }
                    Item::End | Item::Close(_) => break,
                    Item::Open(o) => {
                        let open = *o;
                        if depth + 1 > MAX_DEPTH {
                            return Err(err(
                                open,
                                format!("blocks nested more than {MAX_DEPTH} deep"),
                            ));
                        }
                        self.items.next();
                        d.block = self.block(depth + 1)?;
                        d.has_block = true;
                        if self.items.next().is_none() {
                            return Err(err(open, "this { is not closed"));
                        }
                        if let Some(Item::Word(t)) = self.items.peek() {
                            return Err(err(
                                t.off,
                                "a block's } ends its directive: begin a new line or use ;",
                            ));
                        }
                        break;
                    }
                }
            }
            ds.push(d);
        }
        Ok(ds)
    }
}

#[cfg(test)]
mod tests;
