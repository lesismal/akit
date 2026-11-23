//! A strict YAML reader that keeps where each value is (decision 005), for configs written in
//! YAML as fit reads them (fit's decision 019, `adapter/yaml`): the YAML says what the JSON
//! would, key for key. A mapping is an object, a sequence an array, and a scalar the JSON value
//! its YAML 1.2 tag says (core schema, as fit's YAML library resolves it): a string unless it is
//! a number, a boolean (`true`/`false`) or null. Anchors, aliases and merge keys (`<<`) are
//! resolved, within bounds; a key given twice, a key that is not a scalar, a tag fit's config
//! does not know and a second document are refused, as fit refuses them.
//!
//! Reading is two passes: [`Parser`] turns the text into a tree of YAML nodes (anchors shared,
//! not copied), and [`Conv`] turns that tree into the [`Node`] tree the JSON reader produces,
//! expanding aliases and merge keys under [`MAX_NODES`] and [`MAX_EXPANDED`], and resolving
//! scalars.
//!
//! What fit's library accepts and this reader does not, each refused with an error rather than
//! read differently: `%TAG` directives and named tag handles (`!e!x`), a lone carriage return as
//! a line break, collections nested more than [`MAX_DEPTH`] deep (fit: 256), and two oddities of libyaml's (a tab between `-` and a comment, an empty explicit
//! key in a flow sequence). `tools/yaml-diff` checks the reader against fit's.
//!
//! Offsets are bounded by the input's length, so the position arithmetic below cannot overflow.
#![allow(clippy::arithmetic_side_effects)]

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::json::{Kind, MAX_DEPTH, Member, Node, ParseError};

/// The most nodes a document may expand to through its aliases and merge keys, as fit's.
pub(crate) const MAX_NODES: usize = 1 << 20;

/// The most bytes of scalars a document may expand to through its aliases, as fit's bound on
/// the JSON it writes.
pub(crate) const MAX_EXPANDED: usize = 64 << 20;

/// How deep aliases may nest once expanded, as fit's.
const MAX_EXPAND_DEPTH: usize = 256;

/// Parses `src` as one YAML document into the tree the JSON reader produces. An empty
/// document, or one of comments alone, is an empty object, as fit reads it.
pub(crate) fn parse(src: &str) -> Result<Node, ParseError> {
    check_chars(src)?;
    let bom = if src.starts_with('\u{feff}') { 3 } else { 0 };
    let mut p = Parser {
        s: src,
        b: src.as_bytes(),
        i: 0,
        bom,
        depth: 0,
        flow: 0,
        line: Cell::new((bom, bom)),
        flow_parent: -1,
        anchors: HashMap::new(),
    };
    let Some(root) = p.document()? else {
        return Ok(Node {
            off: 0,
            kind: Kind::Object(Vec::new()),
        });
    };
    Conv { nodes: 0, bytes: 0 }.value(&root, 0)
}

/// Refuses what YAML does not allow in a stream: control characters other than tab, line feed
/// and carriage return (as part of a CRLF line break), and the non-characters.
fn check_chars(src: &str) -> Result<(), ParseError> {
    let b = src.as_bytes();
    for (off, c) in src.char_indices() {
        let bad = match c {
            '\t' | '\n' => false,
            '\r' => b.get(off + 1) != Some(&b'\n'),
            '\u{85}' => false,
            c if c < ' ' => true,
            '\u{7f}'..='\u{9f}' => true,
            '\u{fffe}' | '\u{ffff}' => true,
            _ => false,
        };
        if bad {
            let msg = if c == '\r' {
                "invalid YAML: a carriage return not followed by a line feed".to_owned()
            } else {
                format!("invalid YAML: the control character {c:?}")
            };
            return Err(ParseError { off, msg });
        }
    }
    Ok(())
}

/// A YAML node as written: its offset, its tag if it has one, and what it holds.
#[derive(Debug)]
struct Y {
    off: usize,
    /// The tag in its short form (`!!str`, `!custom`), with where it is written.
    tag: Option<(String, usize)>,
    v: Yv,
}

#[derive(Debug)]
enum Yv {
    /// A scalar's text, and whether it was plain (unquoted, not a block scalar), which alone is
    /// resolved to a number, a boolean or null.
    Scalar(String, bool),
    Seq(Vec<Rc<Y>>),
    Map(Vec<(Rc<Y>, Rc<Y>)>),
    Alias(Rc<Y>),
}

impl Y {
    fn empty(off: usize) -> Rc<Y> {
        Rc::new(Y {
            off,
            tag: None,
            v: Yv::Scalar(String::new(), true),
        })
    }
}

/// A node's anchor and tag, read before it.
#[derive(Default)]
struct Props {
    anchor: Option<String>,
    tag: Option<(String, usize)>,
    off: Option<usize>,
}

impl Props {
    fn is_empty(&self) -> bool {
        self.off.is_none()
    }
}

/// Where a block node is read: what may start it on the line it shares with its parent.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    /// The document's root.
    Doc,
    /// A mapping's value, after `key:`.
    MapValue,
    /// A sequence's entry, after `-`.
    SeqEntry,
}

struct Parser<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
    /// The length of a byte order mark the stream starts with, which no column counts.
    bom: usize,
    depth: usize,
    /// How many flow collections the position is in.
    flow: usize,
    /// The last position [`Parser::line_start`] was asked about, and its answer.
    line: Cell<(usize, usize)>,
    /// The indentation of the block node the outermost flow collection is in.
    flow_parent: isize,
    /// Each anchor's node, with where its properties start.
    anchors: HashMap<String, (usize, Rc<Y>)>,
}

fn blank(c: Option<u8>) -> bool {
    matches!(c, None | Some(b' ' | b'\t' | b'\n' | b'\r'))
}

fn flow_indicator(c: u8) -> bool {
    matches!(c, b',' | b'[' | b']' | b'{' | b'}')
}

impl Parser<'_> {
    fn err(&self, msg: impl Into<String>) -> ParseError {
        self.err_at(self.i, msg)
    }

    fn err_at(&self, off: usize, msg: impl Into<String>) -> ParseError {
        ParseError {
            off,
            msg: format!("invalid YAML: {}", msg.into()),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn at(&self, k: usize) -> Option<u8> {
        self.b.get(self.i + k).copied()
    }

    fn eof(&self) -> bool {
        self.i >= self.b.len()
    }

    /// Where the current line starts. The last answer is kept, so that reading a long line
    /// token by token stays linear.
    fn line_start(&self) -> usize {
        let (at, start) = self.line.get();
        let found = if (start..at).contains(&self.i) {
            start
        } else if self.i >= at {
            self.b
                .get(at..self.i)
                .and_then(|s| s.iter().rposition(|&c| c == b'\n'))
                .map_or(start, |n| at + n + 1)
        } else {
            self.s
                .get(..self.i)
                .and_then(|s| s.rfind('\n'))
                .map_or(self.bom, |n| n + 1)
        };
        self.line.set((self.i, found));
        found
    }

    /// The column of the current position, counted in bytes: only compared where what comes
    /// before it on its line is spaces and ASCII indicators.
    fn col(&self) -> usize {
        self.i - self.line_start()
    }

    /// Whether only white space comes before the current position on its line.
    fn first_on_line(&self) -> bool {
        self.b
            .get(self.line_start()..self.i)
            .is_some_and(|s| s.iter().all(|&c| c == b' ' || c == b'\t'))
    }

    /// Whether a document marker (`---` or `...`, then a blank) starts at the current position,
    /// at the start of a line.
    fn marker(&self) -> bool {
        self.col() == 0
            && matches!(self.b.get(self.i..self.i + 3), Some(b"---") | Some(b"..."))
            && blank(self.at(3))
    }

    fn line_break(&mut self) -> bool {
        match self.peek() {
            Some(b'\n') => {
                self.i += 1;
                true
            }
            Some(b'\r') if self.at(1) == Some(b'\n') => {
                self.i += 2;
                true
            }
            _ => false,
        }
    }

    fn skip_to_eol(&mut self) {
        while let Some(c) = self.peek() {
            if c == b'\n' || c == b'\r' {
                break;
            }
            self.i += 1;
        }
    }

    /// Skips spaces and tabs on the current line, and a comment after them. As fit's YAML
    /// library, a block refuses a tab in indentation and after a `-` or `?` indicator.
    fn skip_line_space(&mut self) -> Result<(), ParseError> {
        let tabs_ok = self.flow > 0 || !self.after_indicator();
        let line_start = self.i == self.line_start();
        let mut tab = None;
        while let Some(c @ (b' ' | b'\t')) = self.peek() {
            if c == b'\t' && tab.is_none() {
                tab = Some(self.i);
            }
            self.i += 1;
        }
        if let Some(t) = tab.filter(|_| !tabs_ok) {
            // fit's library lets a comment follow a tab after an indicator, and start a line
            // with a tab after a comment line.
            let comment_ok = !line_start || self.after_comment_line();
            if !(self.peek() == Some(b'#') && comment_ok) {
                return Err(self.err_at(
                    t,
                    "a tab where only spaces may be (in indentation, or after '-', '?' or ':')",
                ));
            }
        }
        // Between tokens, '#' starts a comment.
        if self.peek() == Some(b'#') {
            self.skip_to_eol();
        }
        Ok(())
    }

    /// Whether the line before the current one holds a comment alone.
    fn after_comment_line(&self) -> bool {
        let ls = self.line_start();
        let Some(prev) = ls.checked_sub(1).and_then(|end| self.s.get(..end)) else {
            return false;
        };
        let line = prev.rsplit('\n').next().unwrap_or_default();
        line.trim_start_matches([' ', '\t']).starts_with('#')
    }

    /// Whether the position is at the start of a line, right after a `-` or `?` indicator, or
    /// after the `:` of an explicit key's value at the start of a line.
    fn after_indicator(&self) -> bool {
        let before = self.b.get(self.line_start()..self.i).unwrap_or_default();
        match before {
            [] => true,
            [.., p, b'-' | b'?'] => matches!(p, b' ' | b'\t'),
            [b'-' | b'?' | b':'] => true,
            _ => false,
        }
    }

    /// Skips white space, comments and line breaks to the next token, and refuses a tab in the
    /// indentation before it.
    fn skip_ws(&mut self) -> Result<(), ParseError> {
        loop {
            self.skip_line_space()?;
            if !self.line_break() {
                return Ok(());
            }
        }
    }

    /// Enters a collection, at most [`MAX_DEPTH`] deep, as the JSON reader: fit's bound is 256,
    /// which no config comes near, and half of it keeps the recursion's stack small.
    fn nest(&mut self) -> Result<(), ParseError> {
        if self.depth >= MAX_DEPTH {
            return Err(self.err(format!("nested deeper than {MAX_DEPTH}")));
        }
        self.depth += 1;
        Ok(())
    }

    /// The stream: directives, at most one document, and nothing after it but comments.
    fn document(&mut self) -> Result<Option<Rc<Y>>, ParseError> {
        self.i = self.bom;
        self.skip_ws()?;
        let mut directives = false;
        while self.peek() == Some(b'%') && self.col() == 0 {
            let start = self.i;
            self.skip_to_eol();
            let line = self.s.get(start..self.i).unwrap_or_default();
            let mut words = line.split_whitespace();
            match words.next() {
                Some("%YAML") => {
                    let v = words.next().unwrap_or_default();
                    if !v.starts_with("1.") {
                        return Err(self.err_at(start, format!("YAML version {v:?}")));
                    }
                }
                Some("%TAG") => {
                    return Err(ParseError {
                        off: start,
                        msg: format!("{}: %TAG directives", crate::NOT_SUPPORTED_YET),
                    });
                }
                _ => {} // reserved directives are ignored, as YAML says
            }
            directives = true;
            self.skip_ws()?;
        }
        let mut started = false;
        if self.marker() && self.b.get(self.i..self.i + 3) == Some(b"---") {
            self.i += 3;
            started = true;
        } else if directives {
            return Err(self.err("expected '---' after the directives"));
        }
        let root = self.block_node(-1, Ctx::Doc)?;
        self.skip_ws()?;
        if self.marker() && self.b.get(self.i..self.i + 3) == Some(b"...") {
            if root.is_none() && !started {
                return Err(self.err("a document end marker ('...') with no document"));
            }
            while self.marker() && self.b.get(self.i..self.i + 3) == Some(b"...") {
                self.i += 3;
                self.skip_ws()?;
            }
        }
        if !self.eof() {
            if self.marker() || self.peek() == Some(b'%') {
                return Err(ParseError {
                    off: self.i,
                    msg: "the YAML holds more than one document".into(),
                });
            }
            return Err(self.err("unexpected content after the document's root (bad indentation?)"));
        }
        Ok(root)
    }

    /// A block node after its parent's indicator (`key:`, `-`, the document start): on the same
    /// line or on the lines after, indented more than `parent`.
    fn block_node(&mut self, parent: isize, ctx: Ctx) -> Result<Option<Rc<Y>>, ParseError> {
        let empty_at = self.i;
        self.skip_ws()?;
        // Properties on lines of their own are the node's; those on the line of its content are
        // the content's (a key's, when the content is a mapping).
        let mut outer = Props::default();
        let mut inner = Props::default();
        let mut start_col = self.col();
        // Whether the line's first token (properties included) starts the line.
        let mut line_fresh = false;
        loop {
            if self.eof() || self.marker() {
                let props = self.merge(outer, inner)?;
                return Ok(self.empty_with(props, empty_at));
            }
            let fresh = self.first_on_line();
            let c = self.col();
            if fresh {
                start_col = c;
                line_fresh = true;
            }
            if fresh && to_isize(c) <= parent {
                let props = self.merge(outer, inner)?;
                // A mapping's value may be a sequence at the key's column; a mapping's value and
                // a sequence's entry may be a block scalar at it.
                if ctx != Ctx::Doc && to_isize(c) == parent {
                    if ctx == Ctx::MapValue && self.seq_indicator() {
                        return self.finish(props, |p| p.block_seq(c)).map(Some);
                    }
                    if matches!(self.peek(), Some(b'|' | b'>')) {
                        return self.finish(props, |p| p.block_scalar(parent)).map(Some);
                    }
                }
                return Ok(self.empty_with(props, empty_at));
            }
            // A block collection starts on a line of its own, or after a sequence's `-`.
            let block_ok = line_fresh || ctx == Ctx::SeqEntry;
            match self.peek() {
                Some(b'&' | b'!') => {
                    self.props(&mut inner)?;
                    self.skip_line_space()?;
                    if matches!(self.peek(), None | Some(b'\n' | b'\r')) {
                        self.skip_ws()?;
                        outer = self.merge(outer, std::mem::take(&mut inner))?;
                    }
                }
                _ => {
                    return self
                        .node_at(parent, block_ok, start_col, outer, inner)
                        .map(Some);
                }
            }
        }
    }

    /// Properties read in two places, for one node.
    fn merge(&self, a: Props, b: Props) -> Result<Props, ParseError> {
        if a.anchor.is_some() && b.anchor.is_some() {
            return Err(self.err_at(b.off.unwrap_or(self.i), "a node with two anchors"));
        }
        if a.tag.is_some() && b.tag.is_some() {
            return Err(self.err_at(b.off.unwrap_or(self.i), "a node with two tags"));
        }
        Ok(Props {
            anchor: a.anchor.or(b.anchor),
            tag: a.tag.or(b.tag),
            off: a.off.or(b.off),
        })
    }

    /// An empty node, null, with the properties read before it.
    fn empty_with(&mut self, props: Props, off: usize) -> Option<Rc<Y>> {
        if props.is_empty() {
            return None;
        }
        let y = Rc::new(Y {
            off: props.off.unwrap_or(off),
            tag: props.tag,
            v: Yv::Scalar(String::new(), true),
        });
        if let Some(a) = props.anchor {
            self.anchor(a, &y);
        }
        Some(y)
    }

    /// Names a finished node by its anchor. As fit's library names a node when it starts, an
    /// anchor given again inside the node it names keeps the inner node.
    fn anchor(&mut self, name: String, y: &Rc<Y>) {
        match self.anchors.get(&name) {
            Some((start, _)) if *start > y.off => {}
            _ => {
                self.anchors.insert(name, (y.off, Rc::clone(y)));
            }
        }
    }

    /// Reads a node with `f` and gives it the properties.
    fn finish(
        &mut self,
        props: Props,
        f: impl FnOnce(&mut Self) -> Result<Y, ParseError>,
    ) -> Result<Rc<Y>, ParseError> {
        let mut y = f(self)?;
        if let Some(off) = props.off {
            y.off = off;
        }
        if props.tag.is_some() {
            y.tag = props.tag;
        }
        let y = Rc::new(y);
        if let Some(a) = props.anchor {
            self.anchor(a, &y);
        }
        Ok(y)
    }

    /// Whether a ':' that makes what is before it a key follows, after spaces on this line;
    /// the position moves to it when it does.
    fn colon_ahead(&mut self) -> bool {
        let save = self.i;
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.i += 1;
        }
        if self.peek() == Some(b':') && blank(self.at(1)) {
            true
        } else {
            self.i = save;
            false
        }
    }

    fn seq_indicator(&self) -> bool {
        self.peek() == Some(b'-') && blank(self.at(1))
    }

    /// An anchor (`&a`) or a tag (`!!str`, `!x`, `!<tag:yaml.org,2002:str>`).
    fn props(&mut self, props: &mut Props) -> Result<(), ParseError> {
        let start = self.i;
        props.off.get_or_insert(start);
        if self.peek() == Some(b'&') {
            if props.anchor.is_some() {
                return Err(self.err("a node with two anchors"));
            }
            self.i += 1;
            props.anchor = Some(self.name("an anchor")?);
            return Ok(());
        }
        if props.tag.is_some() {
            return Err(self.err("a node with two tags"));
        }
        let tag = if self.at(1) == Some(b'<') {
            self.i += 2;
            let s = self.i;
            while matches!(self.peek(), Some(c) if c != b'>' && !blank(Some(c))) {
                self.i += 1;
            }
            if self.peek() != Some(b'>') {
                return Err(self.err_at(start, "a verbatim tag that does not end"));
            }
            let uri = self.s.get(s..self.i).unwrap_or_default().to_owned();
            self.i += 1;
            match uri.strip_prefix("tag:yaml.org,2002:") {
                Some(rest) => format!("!!{rest}"),
                None => uri,
            }
        } else {
            let s = self.i;
            self.i += 1;
            while let Some(c) = self.peek() {
                if c == b'%' {
                    // A URI escape: two hex digits.
                    let hex = self.b.get(self.i + 1..self.i + 3);
                    if !hex.is_some_and(|h| h.iter().all(u8::is_ascii_hexdigit)) {
                        return Err(self.err("a '%' in a tag not followed by two hex digits"));
                    }
                    self.i += 3;
                } else if c.is_ascii_alphanumeric() || b"-_;/?:@&=+$,.!~*'()[]".contains(&c) {
                    self.i += 1;
                } else {
                    break;
                }
            }
            let t = self.s.get(s..self.i).unwrap_or_default().to_owned();
            // A named handle: '!', letters, digits, '-' or '_', then '!'.
            let named = t
                .get(1..)
                .and_then(|r| r.split_once('!'))
                .is_some_and(|(h, _)| {
                    !h.is_empty()
                        && h.bytes()
                            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
                });
            if named {
                return Err(ParseError {
                    off: start,
                    msg: format!("{}: named tag handles ({t})", crate::NOT_SUPPORTED_YET),
                });
            }
            t
        };
        if !blank(self.peek()) {
            return Err(self.err("expected a space after a tag"));
        }
        if tag == "!!" {
            return Err(self.err_at(start, "a tag with no name"));
        }
        // "!" alone is the non-specific tag: the node is read as it would be with none.
        props.tag = Some((tag, start));
        Ok(())
    }

    /// An anchor's or alias's name: up to a blank or a flow indicator.
    /// As fit's library reads them: letters, digits, `-` and `_`, then a blank or one of
    /// `?:,]}%@` and a backquote.
    fn name(&mut self, what: &str) -> Result<String, ParseError> {
        let s = self.i;
        while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            self.i += 1;
        }
        let end_ok = blank(self.peek())
            || matches!(
                self.peek(),
                Some(b'?' | b':' | b',' | b']' | b'}' | b'%' | b'@' | b'`')
            );
        if self.i == s || !end_ok {
            return Err(self.err(format!(
                "{what}'s name is letters, digits, '-' and '_' only"
            )));
        }
        Ok(self.s.get(s..self.i).unwrap_or_default().to_owned())
    }

    fn alias(&mut self) -> Result<Y, ParseError> {
        let off = self.i;
        self.i += 1;
        let name = self.name("an alias")?;
        match self.anchors.get(&name) {
            Some((_, t)) => Ok(Y {
                off,
                tag: None,
                v: Yv::Alias(Rc::clone(t)),
            }),
            None => Err(self.err_at(off, format!("unknown anchor {name:?} referenced"))),
        }
    }

    /// A node starting at the current token, at column `start_col` with its properties. A
    /// block collection may start here when `block_ok`.
    fn node_at(
        &mut self,
        parent: isize,
        block_ok: bool,
        start_col: usize,
        outer: Props,
        inner: Props,
    ) -> Result<Rc<Y>, ParseError> {
        let c = self.col();
        // An empty key with an anchor or a tag (`&a: 1`), as fit's library reads it.
        if self.peek() == Some(b':') && blank(self.at(1)) && !inner.is_empty() && block_ok {
            let off = inner.off.unwrap_or(self.i);
            return self.map_from_key(start_col, outer, inner, off, String::new(), true);
        }
        let structural = match self.peek() {
            Some(b'-' | b'?') => blank(self.at(1)),
            Some(b'*' | b'|' | b'>' | b'[' | b'{') => true,
            _ => false,
        };
        if !structural {
            return self.scalar_or_map(parent, block_ok, start_col, outer, inner);
        }
        let inner_props = !inner.is_empty();
        let props = self.merge(outer, inner)?;
        match self.peek() {
            Some(b'-') if blank(self.at(1)) => {
                if !block_ok || inner_props {
                    return Err(self.err("a block sequence is not allowed here"));
                }
                self.finish(props, |p| p.block_seq(c))
            }
            Some(b'?') if blank(self.at(1)) => {
                if !block_ok || inner_props {
                    return Err(self.err("an explicit key is not allowed here"));
                }
                let key = self.explicit_key(c)?;
                self.finish(props, |p| p.block_map_from(c, key, true))
            }
            Some(b'*') => {
                if !props.is_empty() {
                    return Err(self.err("an alias cannot have an anchor or a tag"));
                }
                let y = self.alias()?;
                self.skip_line_space()?;
                if self.peek() == Some(b':') && blank(self.at(1)) {
                    return Err(self.err_at(y.off, "a key must be a string"));
                }
                Ok(Rc::new(y))
            }
            Some(b'|' | b'>') => self.finish(props, |p| p.block_scalar(parent)),
            Some(b'[' | b'{') => {
                self.flow_parent = parent;
                let y = self.finish(props, Self::flow_collection)?;
                self.skip_line_space()?;
                if self.peek() == Some(b':') && blank(self.at(1)) {
                    return Err(self.err_at(y.off, "a key must be a string"));
                }
                Ok(y)
            }
            _ => Err(self.err("unexpected character")),
        }
    }

    /// A quoted or plain scalar, or the block mapping it is the first key of.
    fn scalar_or_map(
        &mut self,
        parent: isize,
        block_ok: bool,
        start_col: usize,
        outer: Props,
        inner: Props,
    ) -> Result<Rc<Y>, ParseError> {
        let off = self.i;
        let quoted = matches!(self.peek(), Some(b'"' | b'\''));
        let text = if quoted {
            self.quoted()?
        } else {
            self.plain_line(false)?
        };
        if self.colon_ahead() {
            if !block_ok {
                return Err(self.err("a mapping is not allowed here (a ':' after a value)"));
            }
            if self.b.get(off..self.i).is_some_and(|k| k.contains(&b'\n')) {
                return Err(self.err_at(off, "a key on more than one line"));
            }
            return self.map_from_key(start_col, outer, inner, off, text, !quoted);
        }
        let text = if quoted {
            text
        } else {
            self.plain_more(text, parent, false)?
        };
        let props = self.merge(outer, inner)?;
        Ok(self.scalar_with(props, off, text, !quoted))
    }

    /// A block mapping whose first key has been read: the properties on the key's line are
    /// the key's, those on lines before it the mapping's.
    fn map_from_key(
        &mut self,
        n: usize,
        outer: Props,
        inner: Props,
        off: usize,
        text: String,
        plain: bool,
    ) -> Result<Rc<Y>, ParseError> {
        let key = self.scalar_with(inner, off, text, plain);
        self.finish(outer, |p| p.block_map(n, key))
    }

    fn scalar_with(&mut self, props: Props, off: usize, text: String, plain: bool) -> Rc<Y> {
        let y = Rc::new(Y {
            off: props.off.unwrap_or(off),
            tag: props.tag,
            v: Yv::Scalar(text, plain),
        });
        if let Some(a) = props.anchor {
            self.anchor(a, &y);
        }
        y
    }

    /// A block mapping at column `n`, its first key read; the current position is at or before
    /// the ':' after it.
    fn block_map(&mut self, n: usize, first: Rc<Y>) -> Result<Y, ParseError> {
        self.block_map_from(n, first, false)
    }

    /// A block mapping at column `n` whose first key, `explicit` (after `?`) or not, has been
    /// read.
    fn block_map_from(&mut self, n: usize, first: Rc<Y>, explicit: bool) -> Result<Y, ParseError> {
        self.nest()?;
        let off = first.off;
        let mut entries = Vec::new();
        let (mut key, mut explicit) = (first, explicit);
        loop {
            let value_off;
            let value = if explicit {
                // The value, if any, follows on a line of its own at the mapping's column.
                self.skip_ws()?;
                value_off = self.i;
                if !self.eof()
                    && !self.marker()
                    && self.first_on_line()
                    && self.col() == n
                    && self.peek() == Some(b':')
                    && blank(self.at(1))
                {
                    self.i += 1;
                    // As after a sequence's '-', a block collection may start on this line.
                    self.block_node(to_isize(n), Ctx::SeqEntry)?
                } else {
                    None
                }
            } else {
                self.skip_line_space()?;
                if !(self.peek() == Some(b':') && blank(self.at(1))) {
                    return Err(self.err("expected ':' after a key"));
                }
                self.i += 1;
                value_off = self.i;
                self.block_node(to_isize(n), Ctx::MapValue)?
            };
            entries.push((key, value.unwrap_or_else(|| Y::empty(value_off))));
            self.skip_ws()?;
            if self.eof() || self.marker() {
                break;
            }
            if !self.first_on_line() {
                return Err(self.err("expected the end of the line after a value"));
            }
            let c = self.col();
            if c < n {
                break;
            }
            if c > n {
                return Err(self.err("bad indentation of a mapping entry"));
            }
            explicit = self.peek() == Some(b'?') && blank(self.at(1));
            key = if explicit {
                self.explicit_key(n)?
            } else {
                self.block_key()?
            };
        }
        self.depth -= 1;
        Ok(Y {
            off,
            tag: None,
            v: Yv::Map(entries),
        })
    }

    /// An explicit key (`? key`), of a block mapping at column `n`.
    fn explicit_key(&mut self, n: usize) -> Result<Rc<Y>, ParseError> {
        self.i += 1; // ?
        let off = self.i;
        Ok(self
            .block_node(to_isize(n), Ctx::SeqEntry)?
            .unwrap_or_else(|| Y::empty(off)))
    }

    /// A key of a block mapping after its first, at the mapping's column.
    fn block_key(&mut self) -> Result<Rc<Y>, ParseError> {
        let mut props = Props::default();
        while matches!(self.peek(), Some(b'&' | b'!')) {
            self.props(&mut props)?;
            self.skip_line_space()?;
            if matches!(self.peek(), None | Some(b'\n' | b'\r')) {
                return Err(self.err("expected a key after its anchor or tag"));
            }
        }
        let off = self.i;
        if self.peek() == Some(b':') && blank(self.at(1)) && !props.is_empty() {
            let off = props.off.unwrap_or(off);
            return Ok(self.scalar_with(props, off, String::new(), true));
        }
        match self.peek() {
            Some(b'-') if blank(self.at(1)) => {
                Err(self.err("a sequence entry where a mapping's key was expected"))
            }
            Some(b'*') => Err(self.err("a key must be a string")),
            Some(b'[' | b'{') => Err(self.err("a key must be a string")),
            Some(b'|' | b'>') => Err(self.err("a block scalar cannot be a key")),
            Some(b'"' | b'\'') => {
                let text = self.quoted()?;
                if !self.colon_ahead() {
                    return Err(self.err("expected ':' after a key"));
                }
                if self.b.get(off..self.i).is_some_and(|k| k.contains(&b'\n')) {
                    return Err(self.err_at(off, "a key on more than one line"));
                }
                Ok(self.scalar_with(props, off, text, false))
            }
            _ => {
                let text = self.plain_line(false)?;
                if !self.colon_ahead() {
                    return Err(self.err_at(off, "expected 'key: value' (no ':' after this)"));
                }
                Ok(self.scalar_with(props, off, text, true))
            }
        }
    }

    /// A block sequence at column `n`; the current position is at its first `-`.
    fn block_seq(&mut self, n: usize) -> Result<Y, ParseError> {
        self.nest()?;
        let off = self.i;
        let mut items = Vec::new();
        loop {
            self.i += 1; // -
            let entry_off = self.i;
            let item = self
                .block_node(to_isize(n), Ctx::SeqEntry)?
                .unwrap_or_else(|| Y::empty(entry_off));
            items.push(item);
            self.skip_ws()?;
            if self.eof() || self.marker() {
                break;
            }
            if !self.first_on_line() {
                return Err(self.err("expected the end of the line after a value"));
            }
            let c = self.col();
            if c < n || (c == n && !self.seq_indicator()) {
                break;
            }
            if c > n {
                return Err(self.err("bad indentation of a sequence entry"));
            }
        }
        self.depth -= 1;
        Ok(Y {
            off,
            tag: None,
            v: Yv::Seq(items),
        })
    }

    /// The first line of a plain scalar, up to a ': ', a ' #', the end of the line or, in a
    /// flow collection, a flow indicator.
    fn plain_line(&mut self, flow: bool) -> Result<String, ParseError> {
        let start = self.i;
        let c = self.peek();
        let indicator = matches!(
            c,
            Some(
                b'-' | b'?'
                    | b':'
                    | b','
                    | b'['
                    | b']'
                    | b'{'
                    | b'}'
                    | b'#'
                    | b'&'
                    | b'*'
                    | b'!'
                    | b'|'
                    | b'>'
                    | b'\''
                    | b'"'
                    | b'%'
                    | b'@'
                    | b'`'
            )
        );
        if indicator {
            let next = self.at(1);
            // As fit's library: '-' before a non-blank, and '?' and ':' too outside a flow
            // collection.
            let safe = match c {
                Some(b'-') => !blank(next),
                Some(b'?' | b':') => !flow && !blank(next),
                _ => false,
            };
            if !safe {
                return Err(self.err(format!(
                    "a value cannot start with {:?}",
                    self.s
                        .get(self.i..)
                        .and_then(|r| r.chars().next())
                        .unwrap_or(' ')
                )));
            }
        }
        self.plain_run(flow);
        Ok(self
            .s
            .get(start..self.i)
            .unwrap_or_default()
            .trim_end()
            .to_owned())
    }

    /// Moves past the plain characters of one line, leaving the trailing spaces.
    fn plain_run(&mut self, flow: bool) {
        let mut end = self.i;
        while let Some(c) = self.peek() {
            match c {
                b'\n' | b'\r' => break,
                b':' if blank(self.at(1)) => break,
                b'?' if flow => break,
                b'#' if self.i > 0 && matches!(self.b.get(self.i - 1), Some(b' ' | b'\t')) => {
                    break;
                }
                c if flow && flow_indicator(c) => break,
                b' ' | b'\t' => self.i += 1,
                _ => {
                    self.i += 1;
                    end = self.i;
                }
            }
        }
        self.i = end;
    }

    /// The lines that continue a plain scalar: in a block, those indented more than `parent`;
    /// folded, a line break a space and each empty line a line feed.
    fn plain_more(
        &mut self,
        mut text: String,
        parent: isize,
        flow: bool,
    ) -> Result<String, ParseError> {
        loop {
            let save = self.i;
            while matches!(self.peek(), Some(b' ' | b'\t')) {
                self.i += 1;
            }
            if !self.line_break() {
                self.i = save;
                return Ok(text);
            }
            let mut empties = 0;
            loop {
                while let Some(c @ (b' ' | b'\t')) = self.peek() {
                    // As fit's library: a tab in the indentation a block's value needs.
                    let indent = if flow { self.flow_parent } else { parent };
                    if c == b'\t' && to_isize(self.col()) <= indent {
                        return Err(self.err("a tab in indentation"));
                    }
                    self.i += 1;
                }
                if self.line_break() {
                    empties += 1;
                } else {
                    break;
                }
            }
            let stop = match self.peek() {
                None | Some(b'#') => true,
                Some(c) if flow && (flow_indicator(c) || c == b'?') => true,
                Some(b':') if flow && blank(self.at(1)) => true,
                _ => self.marker_at_line() || (!flow && to_isize(self.col()) <= parent),
            };
            if stop {
                // As fit's library, the white space before what ends the scalar is read.
                if flow {
                    self.i = save;
                }
                return Ok(text);
            }
            if empties == 0 {
                text.push(' ');
            } else {
                text.extend(std::iter::repeat_n('\n', empties));
            }
            let s = self.i;
            self.plain_run(flow);
            text.push_str(self.s.get(s..self.i).unwrap_or_default());
            if !flow && self.peek() == Some(b':') && blank(self.at(1)) {
                return Err(
                    self.err("a key on more than one line, or a ':' in a value on several lines")
                );
            }
        }
    }

    /// Whether the current line starts with a document marker.
    fn marker_at_line(&self) -> bool {
        let ls = self.line_start();
        matches!(self.b.get(ls..ls + 3), Some(b"---") | Some(b"..."))
            && blank(self.b.get(ls + 3).copied())
    }

    /// A single- or double-quoted scalar, folded across lines as YAML folds them.
    fn quoted(&mut self) -> Result<String, ParseError> {
        let start = self.i;
        let double = self.peek() == Some(b'"');
        self.i += 1;
        let mut out = String::new();
        loop {
            let run = self.i;
            while let Some(c) = self.peek() {
                if matches!(c, b'\'' | b'"' | b'\\' | b' ' | b'\t' | b'\n' | b'\r') {
                    break;
                }
                self.i += 1;
            }
            out.push_str(self.s.get(run..self.i).unwrap_or_default());
            match self.peek() {
                None => return Err(self.err_at(start, "a quoted string that does not end")),
                Some(b'"') if double => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\'') if !double => {
                    if self.at(1) == Some(b'\'') {
                        out.push('\'');
                        self.i += 2;
                    } else {
                        self.i += 1;
                        return Ok(out);
                    }
                }
                Some(b'\\') if double => {
                    self.i += 1;
                    if matches!(self.peek(), Some(b'\n' | b'\r')) {
                        // An escaped line break: the lines join with nothing between them,
                        // but for the empty lines after it.
                        self.line_break();
                        let empties = self.fold_breaks(start)?;
                        out.extend(std::iter::repeat_n('\n', empties));
                    } else {
                        out.push(self.escape()?);
                    }
                }
                Some(b' ' | b'\t') => {
                    let ws = self.i;
                    while matches!(self.peek(), Some(b' ' | b'\t')) {
                        self.i += 1;
                    }
                    if !matches!(self.peek(), Some(b'\n' | b'\r')) {
                        out.push_str(self.s.get(ws..self.i).unwrap_or_default());
                    }
                }
                Some(b'\n' | b'\r') => {
                    self.line_break();
                    let empties = self.fold_breaks(start)?;
                    if empties == 0 {
                        out.push(' ');
                    } else {
                        out.extend(std::iter::repeat_n('\n', empties));
                    }
                }
                Some(c) => {
                    // The other quote, or a backslash in a single-quoted string.
                    out.push(char::from(c));
                    self.i += 1;
                }
            }
        }
    }

    /// Skips the empty lines and the indentation after a line break in a quoted string,
    /// returning how many empty lines there were.
    fn fold_breaks(&mut self, start: usize) -> Result<usize, ParseError> {
        let mut empties = 0;
        loop {
            while matches!(self.peek(), Some(b' ' | b'\t')) {
                self.i += 1;
            }
            if self.marker_at_line() && self.col() <= 3 {
                return Err(self.err_at(start, "a document marker inside a quoted string"));
            }
            if self.line_break() {
                empties += 1;
            } else {
                return Ok(empties);
            }
        }
    }

    fn escape(&mut self) -> Result<char, ParseError> {
        let at = self.i - 1;
        let c = self.peek();
        self.i += 1;
        let hex = |p: &mut Self, n: usize| -> Result<char, ParseError> {
            let digits =
                p.s.get(p.i..p.i + n)
                    .filter(|s| s.bytes().all(|b| b.is_ascii_hexdigit()))
                    .ok_or_else(|| p.err_at(at, format!("an escape that needs {n} hex digits")))?;
            p.i += n;
            u32::from_str_radix(digits, 16)
                .ok()
                .and_then(char::from_u32)
                .ok_or_else(|| p.err_at(at, "an escape that is not a Unicode character"))
        };
        Ok(match c {
            Some(b'0') => '\0',
            Some(b'a') => '\u{7}',
            Some(b'b') => '\u{8}',
            Some(b't' | b'\t') => '\t',
            Some(b'n') => '\n',
            Some(b'v') => '\u{b}',
            Some(b'f') => '\u{c}',
            Some(b'r') => '\r',
            Some(b'e') => '\u{1b}',
            Some(b' ') => ' ',
            Some(b'"') => '"',
            Some(b'\'') => '\'',
            Some(b'/') => '/',
            Some(b'\\') => '\\',
            Some(b'N') => '\u{85}',
            Some(b'_') => '\u{a0}',
            Some(b'L') => '\u{2028}',
            Some(b'P') => '\u{2029}',
            Some(b'x') => hex(self, 2)?,
            Some(b'u') => hex(self, 4)?,
            Some(b'U') => hex(self, 8)?,
            _ => return Err(self.err_at(at, "an unknown escape in a string")),
        })
    }

    /// A literal (`|`) or folded (`>`) block scalar, its content indented more than `parent`.
    fn block_scalar(&mut self, parent: isize) -> Result<Y, ParseError> {
        let off = self.i;
        let literal = self.peek() == Some(b'|');
        self.i += 1;
        let mut chomp = 0i8; // -1 strip, 0 clip, 1 keep
        let mut increment = 0usize;
        for _ in 0..2 {
            match self.peek() {
                Some(b'-') if chomp == 0 => {
                    chomp = -1;
                    self.i += 1;
                }
                Some(b'+') if chomp == 0 => {
                    chomp = 1;
                    self.i += 1;
                }
                Some(d @ b'1'..=b'9') if increment == 0 => {
                    increment = usize::from(d - b'0');
                    self.i += 1;
                }
                Some(b'0') => return Err(self.err("a block scalar's indentation cannot be 0")),
                _ => {}
            }
        }
        if !blank(self.peek()) && self.peek() != Some(b'#') {
            return Err(self.err("expected the end of the line after a block scalar's header"));
        }
        self.skip_line_space()?;
        if !self.eof() && !self.line_break() {
            return Err(self.err("expected the end of the line after a block scalar's header"));
        }
        let min = usize::try_from(parent + 1).unwrap_or(0).max(1);
        let mut indent = if increment > 0 {
            usize::try_from(parent).map_or(increment, |p| p + increment)
        } else {
            0
        };
        // The line breaks of empty lines not yet written.
        let mut breaks = String::new();
        if indent == 0 {
            // The first line with content sets the indentation.
            let mut max_empty = 0usize;
            loop {
                let ls = self.i;
                while self.peek() == Some(b' ') {
                    self.i += 1;
                }
                if self.peek() == Some(b'\t') {
                    return Err(self.err("a tab in a block scalar's indentation"));
                }
                if self.eof() {
                    break;
                }
                let width = self.i - ls;
                if self.line_break() {
                    max_empty = max_empty.max(width);
                    breaks.push('\n');
                    continue;
                }
                // As fit's library: the most indented of these lines, the empty ones too.
                indent = (self.i - ls).max(max_empty).max(min);
                self.i = ls;
                break;
            }
            if indent == 0 {
                indent = min;
            }
        }
        let mut out = String::new();
        let mut first = true;
        let mut leading_blank = false;
        let mut last_break = false;
        loop {
            let ls = self.i;
            while self.peek() == Some(b' ') && self.i - ls < indent {
                self.i += 1;
            }
            if self.eof() {
                break;
            }
            if self.i - ls < indent {
                if self.line_break() {
                    breaks.push('\n');
                    continue;
                }
                if self.peek() == Some(b'\t') {
                    return Err(self.err("a tab in a block scalar's indentation"));
                }
                // Less indented: the scalar has ended.
                self.i = ls;
                break;
            }
            if self.line_break() {
                breaks.push('\n');
                continue;
            }
            let trailing_blank = matches!(self.peek(), Some(b' ' | b'\t'));
            if !first {
                if !literal && last_break && !leading_blank && !trailing_blank {
                    if breaks.is_empty() {
                        out.push(' ');
                    }
                } else if last_break {
                    out.push('\n');
                }
            }
            out.push_str(&breaks);
            breaks.clear();
            leading_blank = trailing_blank;
            let s = self.i;
            self.skip_to_eol();
            out.push_str(self.s.get(s..self.i).unwrap_or_default());
            first = false;
            last_break = self.line_break();
            if !last_break {
                break;
            }
        }
        match chomp {
            -1 => {}
            0 => {
                if !first && last_break {
                    out.push('\n');
                }
            }
            _ => {
                if last_break {
                    out.push('\n');
                }
                out.push_str(&breaks);
            }
        }
        Ok(Y {
            off,
            tag: None,
            v: Yv::Scalar(out, false),
        })
    }

    /// A flow sequence (`[a, b]`) or mapping (`{a: 1}`), across lines.
    fn flow_collection(&mut self) -> Result<Y, ParseError> {
        self.nest()?;
        self.flow += 1;
        let off = self.i;
        let seq = self.peek() == Some(b'[');
        let close = if seq { b']' } else { b'}' };
        self.i += 1;
        let mut items = Vec::new();
        let mut entries = Vec::new();
        loop {
            self.flow_ws()?;
            match self.peek() {
                Some(c) if c == close => {
                    self.i += 1;
                    break;
                }
                Some(b',') => return Err(self.err("a comma with no value before it")),
                None => {
                    return Err(self.err_at(
                        off,
                        if seq {
                            "did not find expected ',' or ']' (a sequence that does not end)"
                        } else {
                            "did not find expected ',' or '}' (a mapping that does not end)"
                        },
                    ));
                }
                _ => {}
            }
            // As fit's library: in a flow collection, '?' always starts an explicit key.
            let explicit = self.peek() == Some(b'?');
            let (key, json_like) = if explicit {
                self.i += 1;
                let k_off = self.i;
                self.flow_ws()?;
                match self.peek() {
                    Some(c) if c == b',' || c == b':' || c == close => {
                        if seq {
                            return Err(self.err("expected a key after '?'"));
                        }
                        (Y::empty(k_off), false)
                    }
                    _ => self.flow_node()?,
                }
            } else {
                self.flow_node()?
            };
            self.flow_ws()?;
            let pair = self.peek() == Some(b':')
                && (json_like
                    || explicit
                    || blank(self.at(1))
                    || self.at(1).is_some_and(flow_indicator));
            if pair
                && !explicit
                && self
                    .b
                    .get(key.off..self.i)
                    .is_some_and(|k| k.contains(&b'\n'))
            {
                return Err(self.err_at(key.off, "a key on more than one line"));
            }
            let value = if pair {
                self.i += 1;
                let v_off = self.i;
                self.flow_ws()?;
                if matches!(self.peek(), Some(b',')) || self.peek() == Some(close) {
                    Some(Y::empty(v_off))
                } else {
                    Some(self.flow_node()?.0)
                }
            } else {
                None
            };
            let value = match value {
                None if explicit && seq => Some(Y::empty(self.i)),
                v => v,
            };
            if seq {
                match value {
                    Some(v) => items.push(Rc::new(Y {
                        off: key.off,
                        tag: None,
                        v: Yv::Map(vec![(key, v)]),
                    })),
                    None => items.push(key),
                }
            } else {
                let v = value.unwrap_or_else(|| Y::empty(self.i));
                entries.push((key, v));
            }
            self.flow_ws()?;
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(c) if c == close => {}
                None => continue, // reported above
                Some(_) => {
                    return Err(self.err(if seq {
                        "did not find expected ',' or ']'"
                    } else {
                        "did not find expected ',' or '}'"
                    }));
                }
            }
        }
        self.depth -= 1;
        self.flow -= 1;
        Ok(Y {
            off,
            tag: None,
            v: if seq {
                Yv::Seq(items)
            } else {
                Yv::Map(entries)
            },
        })
    }

    /// White space, comments and line breaks in a flow collection.
    fn flow_ws(&mut self) -> Result<(), ParseError> {
        loop {
            self.skip_line_space()?;
            if !self.line_break() {
                break;
            }
            if self.marker() {
                return Err(self.err("a document marker inside a flow collection"));
            }
        }
        Ok(())
    }

    /// A node inside a flow collection, and whether it is one after which ':' needs no space
    /// (a quoted scalar or a flow collection).
    fn flow_node(&mut self) -> Result<(Rc<Y>, bool), ParseError> {
        let mut props = Props::default();
        while matches!(self.peek(), Some(b'&' | b'!')) {
            self.props(&mut props)?;
            self.flow_ws()?;
        }
        let off = self.i;
        match self.peek() {
            Some(b'*') => {
                if !props.is_empty() {
                    return Err(self.err("an alias cannot have an anchor or a tag"));
                }
                Ok((Rc::new(self.alias()?), false))
            }
            Some(b'[' | b'{') => Ok((self.finish(props, Self::flow_collection)?, true)),
            Some(b'"' | b'\'') => {
                let text = self.quoted()?;
                Ok((self.scalar_with(props, off, text, false), true))
            }
            Some(b',' | b']' | b'}' | b':') | None => {
                if props.is_empty() {
                    return Err(self.err("expected a value"));
                }
                // An empty key with an anchor or a tag takes a ':' right after it, as JSON's do.
                Ok((
                    self.empty_with(props, off).unwrap_or_else(|| Y::empty(off)),
                    true,
                ))
            }
            Some(b'|' | b'>') => Err(self.err("a block scalar inside a flow collection")),
            _ => {
                let text = self.plain_line(true)?;
                let text = self.plain_more(text, -1, true)?;
                Ok((self.scalar_with(props, off, text, true), false))
            }
        }
    }
}

fn to_isize(n: usize) -> isize {
    isize::try_from(n).unwrap_or(isize::MAX)
}

/// What a scalar is, by its tag or, plain and untagged, by its text.
#[derive(Debug, PartialEq)]
enum Resolved {
    Null,
    Bool(bool),
    Int(String),
    Float(f64),
    Str,
    Timestamp,
    Merge,
}

impl Resolved {
    fn tag(&self) -> &'static str {
        match self {
            Resolved::Null => "!!null",
            Resolved::Bool(_) => "!!bool",
            Resolved::Int(_) => "!!int",
            Resolved::Float(_) => "!!float",
            Resolved::Str => "!!str",
            Resolved::Timestamp => "!!timestamp",
            Resolved::Merge => "!!merge",
        }
    }
}

/// Resolves a plain scalar as fit's YAML library does (YAML 1.2's core schema, with its
/// additions: `0o`/`0b`/`0x` and YAML 1.1 octals, `_` between digits, timestamps).
fn resolve(s: &str) -> Resolved {
    match s {
        "" | "~" | "null" | "Null" | "NULL" => return Resolved::Null,
        "true" | "True" | "TRUE" => return Resolved::Bool(true),
        "false" | "False" | "FALSE" => return Resolved::Bool(false),
        ".nan" | ".NaN" | ".NAN" => return Resolved::Float(f64::NAN),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => {
            return Resolved::Float(f64::INFINITY);
        }
        "-.inf" | "-.Inf" | "-.INF" => return Resolved::Float(f64::NEG_INFINITY),
        "<<" => return Resolved::Merge,
        _ => {}
    }
    match s.as_bytes().first() {
        // Go's ParseFloat takes '_' between digits.
        Some(b'.') if underscores_between_digits(s) => match parse_float(&s.replace('_', "")) {
            Some(f) => Resolved::Float(f),
            None => Resolved::Str,
        },
        Some(b'0'..=b'9' | b'+' | b'-') => {
            if is_timestamp(s) {
                return Resolved::Timestamp;
            }
            let plain: String = s.chars().filter(|&c| c != '_').collect();
            if let Some(i) = parse_int(&plain) {
                return Resolved::Int(i);
            }
            if yaml_float(&plain) {
                if let Some(f) = parse_float(&plain) {
                    return Resolved::Float(f);
                }
            }
            Resolved::Str
        }
        _ => Resolved::Str,
    }
}

/// Whether each '_' in `s` is between two digits, as Go's number syntax allows them.
fn underscores_between_digits(s: &str) -> bool {
    let b = s.as_bytes();
    b.iter().enumerate().all(|(i, &c)| {
        c != b'_'
            || (i
                .checked_sub(1)
                .and_then(|p| b.get(p))
                .is_some_and(u8::is_ascii_digit)
                && b.get(i + 1).is_some_and(u8::is_ascii_digit))
    })
}

/// An integer as Go's `strconv.ParseInt(s, 0, 64)` or `ParseUint` reads it (a sign, then
/// `0x`, `0o`, `0b` or a leading `0` for octal), in decimal; `None` past 64 bits.
fn parse_int(s: &str) -> Option<String> {
    let (neg, body) = match s.as_bytes().first() {
        Some(b'-') => (true, s.get(1..)?),
        Some(b'+') => (false, s.get(1..)?),
        _ => (false, s),
    };
    let lower = body.get(..2).map(str::to_ascii_lowercase);
    let (radix, digits) = match lower.as_deref() {
        Some("0x") => (16, body.get(2..)?),
        Some("0o") => (8, body.get(2..)?),
        Some("0b") => (2, body.get(2..)?),
        _ if body.len() > 1 && body.starts_with('0') => (8, body.get(1..)?),
        _ => (10, body),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let v = u64::from_str_radix(digits, radix).ok()?;
    if neg {
        if v > 1u64 << 63 {
            return None;
        }
        if v == 0 {
            return Some("0".into());
        }
        return Some(format!("-{v}"));
    }
    Some(v.to_string())
}

/// YAML's float syntax, as fit's library checks it: `[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?`.
fn yaml_float(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if matches!(b.first(), Some(b'-' | b'+')) {
        i += 1;
    }
    let digits = |i: &mut usize| {
        let s = *i;
        while b.get(*i).is_some_and(u8::is_ascii_digit) {
            *i += 1;
        }
        *i > s
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
        if matches!(b.get(i), Some(b'-' | b'+')) {
            i += 1;
        }
        if !digits(&mut i) {
            return false;
        }
    }
    i == b.len()
}

/// A decimal float as Go's `strconv.ParseFloat` reads one (the forms [`yaml_float`] and a
/// leading '.' allow); `None` when it is not one.
fn parse_float(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    if body.is_empty()
        || !body
            .bytes()
            .next()
            .is_some_and(|c| c.is_ascii_digit() || c == b'.')
    {
        return None;
    }
    if !b
        .iter()
        .all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-'))
    {
        return None;
    }
    let f: f64 = s.parse().ok()?;
    // Go's ParseFloat refuses what overflows; Rust's parse gives infinity.
    if f.is_infinite() {
        return None;
    }
    Some(f)
}

/// Whether `s` is a timestamp as fit's library reads one: `2006-1-2`, or that with a time
/// after `T`, `t` or a space (`15:4:5`, a fraction; a zone `Z` or `±07:00` after `T`/`t`, none
/// after a space).
fn is_timestamp(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    let num = |i: &mut usize, min: usize, max: usize| -> Option<u32> {
        let s0 = *i;
        while *i - s0 < max && b.get(*i).is_some_and(u8::is_ascii_digit) {
            *i += 1;
        }
        if *i - s0 < min {
            return None;
        }
        std::str::from_utf8(b.get(s0..*i)?).ok()?.parse().ok()
    };
    let expect = |i: &mut usize, c: u8| -> bool {
        if b.get(*i) == Some(&c) {
            *i += 1;
            true
        } else {
            false
        }
    };
    let Some(year) = num(&mut i, 4, 4) else {
        return false;
    };
    if !expect(&mut i, b'-') {
        return false;
    }
    let Some(month) = num(&mut i, 1, 2) else {
        return false;
    };
    if !expect(&mut i, b'-') {
        return false;
    }
    let Some(day) = num(&mut i, 1, 2) else {
        return false;
    };
    if !(1..=12).contains(&month) || day < 1 || day > days_in(year, month) {
        return false;
    }
    if i == b.len() {
        return true;
    }
    let zoned = match b.get(i) {
        Some(b'T' | b't') => true,
        Some(b' ') => false,
        _ => return false,
    };
    i += 1;
    let (Some(h), true, Some(m), true, Some(sec)) = (
        num(&mut i, 1, 2),
        expect(&mut i, b':'),
        num(&mut i, 1, 2),
        expect(&mut i, b':'),
        num(&mut i, 1, 2),
    ) else {
        return false;
    };
    if h > 23 || m > 59 || sec > 59 {
        return false;
    }
    if expect(&mut i, b'.') && num(&mut i, 1, 9).is_none() {
        return false;
    }
    if !zoned {
        return i == b.len();
    }
    match b.get(i) {
        Some(b'Z') => i + 1 == b.len(),
        Some(b'+' | b'-') => {
            i += 1;
            let ok = num(&mut i, 2, 2).is_some_and(|h| h <= 23)
                && expect(&mut i, b':')
                && num(&mut i, 2, 2).is_some_and(|m| m <= 59);
            ok && i == b.len()
        }
        _ => false,
    }
}

fn days_in(year: u32, month: u32) -> u32 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// A float as Go's `strconv.FormatFloat(f, 'g', -1, 64)` writes it, which is what fit's
/// adapter hands its JSON decoder.
fn go_float(f: f64) -> String {
    let f = if f == 0.0 { 0.0 } else { f };
    // Rust's `{:e}` gives the shortest digits that read back the same, as Go's -1 precision.
    let e = format!("{f:e}");
    let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let (neg, mant) = match mant.strip_prefix('-') {
        Some(m) => (true, m),
        None => (false, mant),
    };
    let digits: String = mant.chars().filter(char::is_ascii_digit).collect();
    let sign = if neg { "-" } else { "" };
    // Go's shortest 'g' form: the exponent form below 1e-4 and from 1e6 on.
    if !(-4..6).contains(&exp) {
        return go_exp(sign, &digits, exp);
    }
    if exp < 0 {
        let zeros = "0".repeat(usize::try_from(-exp - 1).unwrap_or(0));
        return format!("{sign}0.{zeros}{digits}");
    }
    let point = usize::try_from(exp + 1).unwrap_or(0);
    match (digits.get(..point), digits.get(point..)) {
        (Some(int), Some(frac)) if !frac.is_empty() => format!("{sign}{int}.{frac}"),
        _ => {
            let zeros = "0".repeat(point.saturating_sub(digits.len()));
            format!("{sign}{digits}{zeros}")
        }
    }
}

fn go_exp(sign: &str, digits: &str, exp: i32) -> String {
    let (first, rest) = digits.split_at(1.min(digits.len()));
    let dot = if rest.is_empty() { "" } else { "." };
    let es = if exp < 0 { '-' } else { '+' };
    format!("{sign}{first}{dot}{rest}e{es}{:02}", exp.unsigned_abs())
}

/// Turns the YAML tree into the JSON reader's, as fit's adapter turns it into JSON.
struct Conv {
    nodes: usize,
    bytes: usize,
}

impl Conv {
    fn count(&mut self, off: usize) -> Result<(), ParseError> {
        self.nodes += 1;
        if self.nodes > MAX_NODES || self.bytes > MAX_EXPANDED {
            return Err(ParseError {
                off,
                msg: "the document expands to too much, through its aliases".into(),
            });
        }
        Ok(())
    }

    fn value(&mut self, y: &Y, depth: usize) -> Result<Node, ParseError> {
        self.count(y.off)?;
        if depth > MAX_EXPAND_DEPTH {
            return Err(ParseError {
                off: y.off,
                msg: "nested too deep".into(),
            });
        }
        let kind = match &y.v {
            Yv::Alias(t) => {
                let mut n = self.value(t, depth + 1)?;
                n.off = y.off;
                return Ok(n);
            }
            Yv::Seq(items) => {
                let mut out = Vec::with_capacity(items.len());
                for e in items {
                    out.push(self.value(e, depth + 1)?);
                }
                Kind::Array(out)
            }
            Yv::Map(_) => {
                let ms = self.members(y, depth)?;
                let mut out = Vec::with_capacity(ms.len());
                for (key, key_off, v) in ms {
                    self.bytes += key.len();
                    out.push(Member {
                        key,
                        key_off,
                        value: self.value(&v, depth + 1)?,
                    });
                }
                Kind::Object(out)
            }
            Yv::Scalar(text, plain) => self.scalar(y, text, *plain)?,
        };
        Ok(Node { off: y.off, kind })
    }

    /// A mapping's members in order, those merge keys bring in after, unless a key of its own
    /// or an earlier merge has them.
    #[allow(clippy::type_complexity)]
    fn members(&mut self, y: &Y, depth: usize) -> Result<Vec<(String, usize, Rc<Y>)>, ParseError> {
        if depth > MAX_EXPAND_DEPTH {
            return Err(ParseError {
                off: y.off,
                msg: "nested too deep".into(),
            });
        }
        let Yv::Map(entries) = &y.v else {
            return Ok(Vec::new());
        };
        let mut own = Vec::new();
        let mut merged = Vec::new();
        let mut seen = HashSet::new();
        for (k, v) in entries {
            self.count(k.off)?;
            if is_merge_key(k) {
                let src = deref(v);
                let sources: Vec<&Rc<Y>> = match &src.v {
                    Yv::Map(_) => vec![src],
                    Yv::Seq(items) => items.iter().map(deref).collect(),
                    _ => return Err(merge_error(v.off)),
                };
                for s in sources {
                    if !matches!(s.v, Yv::Map(_)) {
                        return Err(merge_error(s.off));
                    }
                    let ms = self.members(s, depth + 1)?;
                    for m in ms {
                        self.count(m.1)?;
                        merged.push(m);
                    }
                }
                continue;
            }
            let key = match &k.v {
                Yv::Scalar(text, plain) if key_tag_ok(k, text, *plain) => text.clone(),
                _ => {
                    return Err(ParseError {
                        off: k.off,
                        msg: "a key must be a string".into(),
                    });
                }
            };
            if !seen.insert(key.clone()) {
                return Err(ParseError {
                    off: k.off,
                    msg: format!("{key:?} is a key twice"),
                });
            }
            own.push((key, k.off, Rc::clone(v)));
        }
        for m in merged {
            if seen.insert(m.0.clone()) {
                own.push(m);
            }
        }
        Ok(own)
    }

    fn scalar(&mut self, y: &Y, text: &str, plain: bool) -> Result<Kind, ParseError> {
        self.bytes += text.len();
        let err = |msg: String| ParseError { off: y.off, msg };
        let resolved = match tag_of(y) {
            Some(t) => match t {
                "!!str" | "!!timestamp" | "!!binary" => Resolved::Str,
                "!!null" => Resolved::Null,
                "!!bool" => match resolve(text) {
                    b @ Resolved::Bool(_) => b,
                    _ => return Err(err(format!("{text:?} is not a boolean (!!bool)"))),
                },
                "!!int" => match resolve(text) {
                    i @ Resolved::Int(_) => i,
                    _ => return Err(err(format!("{text:?} is not a number JSON can hold"))),
                },
                "!!float" => match resolve(text) {
                    Resolved::Int(i) => Resolved::Float(i.parse().unwrap_or(f64::NAN)),
                    f @ Resolved::Float(_) => f,
                    _ => return Err(err(format!("{text:?} is not a float (!!float)"))),
                },
                other => {
                    return Err(err(format!(
                        "the tag {other} is not one fit's config knows"
                    )));
                }
            },
            None if plain => resolve(text),
            None => Resolved::Str,
        };
        Ok(match resolved {
            Resolved::Null => Kind::Null,
            Resolved::Bool(b) => Kind::Bool(b),
            Resolved::Int(i) => Kind::Number(i),
            Resolved::Float(f) => {
                if !f.is_finite() {
                    return Err(err(format!("{text} is not a number JSON can hold")));
                }
                Kind::Number(go_float(f))
            }
            Resolved::Str | Resolved::Timestamp => Kind::Str(text.to_owned()),
            Resolved::Merge => {
                return Err(err("the tag !!merge is not one fit's config knows".into()));
            }
        })
    }
}

fn merge_error(off: usize) -> ParseError {
    ParseError {
        off,
        msg: "a merge key (<<) needs a mapping or a list of them".into(),
    }
}

fn deref(y: &Rc<Y>) -> &Rc<Y> {
    let mut y = y;
    for _ in 0..MAX_EXPAND_DEPTH {
        match &y.v {
            Yv::Alias(t) => y = t,
            _ => break,
        }
    }
    y
}

/// A node's tag, but for the non-specific `!`, which leaves the node read as with none.
fn tag_of(y: &Y) -> Option<&str> {
    y.tag
        .as_ref()
        .map(|(t, _)| t.as_str())
        .filter(|t| *t != "!")
}

fn is_merge_key(k: &Y) -> bool {
    match (&k.v, tag_of(k)) {
        (Yv::Scalar(_, _), Some(t)) => t == "!!merge",
        (Yv::Scalar(text, true), None) => text == "<<",
        _ => false,
    }
}

/// Whether a scalar key's tag is one fit takes a key's text from: a string, a number, a
/// boolean or null.
fn key_tag_ok(k: &Y, text: &str, plain: bool) -> bool {
    let tag = match tag_of(k) {
        Some(t) => t,
        None if plain => resolve(text).tag(),
        None => "!!str",
    };
    matches!(tag, "!!str" | "!!int" | "!!bool" | "!!float" | "!!null")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tree as compact JSON, for comparing with what fit's adapter writes.
    fn json(n: &Node) -> String {
        match &n.kind {
            Kind::Null => "null".into(),
            Kind::Bool(b) => b.to_string(),
            Kind::Number(s) => s.clone(),
            Kind::Str(s) => format!("{s:?}"),
            Kind::Array(items) => {
                let v: Vec<String> = items.iter().map(json).collect();
                format!("[{}]", v.join(","))
            }
            Kind::Object(ms) => {
                let v: Vec<String> = ms
                    .iter()
                    .map(|m| format!("{:?}:{}", m.key, json(&m.value)))
                    .collect();
                format!("{{{}}}", v.join(","))
            }
        }
    }

    fn ok(src: &str) -> String {
        match parse(src) {
            Ok(n) => json(&n),
            Err(e) => panic!("{src:?}: {}", e.msg),
        }
    }

    fn err(src: &str) -> String {
        match parse(src) {
            Ok(n) => panic!("{src:?}: read as {}", json(&n)),
            Err(e) => e.msg,
        }
    }

    #[test]
    fn fits_cases() {
        // fit's adapter/yaml TestYAML.
        for (y, j) in [
            (
                "a: 1\nb: 1.5\nc: true\nd: null\ne: ~\nf: hello\ng: '1'\nh: \"true\"\n",
                r#"{"a":1,"b":1.5,"c":true,"d":null,"e":null,"f":"hello","g":"1","h":"true"}"#,
            ),
            (
                "a: yes\nb: on\nc: 30s\nd: 0x1F\ne: 1e3\n",
                r#"{"a":"yes","b":"on","c":"30s","d":31,"e":1000}"#,
            ),
            (
                "list:\n  - a\n  - {b: 2}\n  - [1, 2]\n",
                r#"{"list":["a",{"b":2},[1,2]]}"#,
            ),
            (
                "base: &b {x: 1, y: 2}\nuse: *b\nmerged:\n  <<: *b\n  y: 3\n",
                r#"{"base":{"x":1,"y":2},"use":{"x":1,"y":2},"merged":{"y":3,"x":1}}"#,
            ),
            (
                "a: &a {p: 1}\nb: &b {q: 2}\nc:\n  <<: [*a, *b]\n  r: 3\n",
                r#"{"a":{"p":1},"b":{"q":2},"c":{"r":3,"p":1,"q":2}}"#,
            ),
            ("200: ok\n", r#"{"200":"ok"}"#),
            (
                "text: |\n  line one\n  line two\n",
                r#"{"text":"line one\nline two\n"}"#,
            ),
        ] {
            assert_eq!(ok(y), j, "{y:?}");
        }
        for y in ["", "\n", "# a: 1\n# b: 2\n", "---\n", "--- # c\n...\n"] {
            assert_eq!(ok(y), "{}", "{y:?}");
        }
    }

    #[test]
    fn fits_errors() {
        let mut bomb = "a: &a [x, x, x, x, x, x, x, x, x, x]\n".to_owned();
        let mut prev = "a".to_owned();
        for n in ["b", "c", "d", "e", "f", "g", "h"] {
            let refs = vec![format!("*{prev}"); 10].join(", ");
            bomb += &format!("{n}: &{n} [{refs}]\n");
            prev = n.to_owned();
        }
        for (y, want) in [
            ("a: 1\n---\nb: 2\n", "more than one document"),
            ("[a]: 1\n", "a key must be a string"),
            ("a: 1\na: 2\n", "\"a\" is a key twice"),
            ("a: .inf\n", "not a number JSON can hold"),
            ("a: !custom x\n", "the tag !custom"),
            ("a: [1\n", "did not find expected"),
            ("<<: 1\n", "a merge key"),
            (bomb.as_str(), "expands to too much"),
            ("a: !!bool maybe\n", "not a boolean"),
            (
                "a: !!int 99999999999999999999999\n",
                "is not a number JSON can hold",
            ),
            ("a: !!int x\n", "is not a number JSON can hold"),
            ("a: !!float x\n", "not a float"),
            ("a:\n  <<: [1]\n", "a merge key (<<) needs a mapping"),
            ("a:\n  <<: {b: 1, b: 2}\n", "\"b\" is a key twice"),
            ("a: <<\n", "!!merge"),
        ] {
            let e = err(y);
            assert!(e.contains(want), "{y:?}: {e}, want {want:?}");
        }
        let deep = format!("{}{}", "[".repeat(300), "]".repeat(300));
        assert!(err(&deep).contains("nested deeper"));
        // As deep as the JSON reader, in blocks and flows.
        let block = |n: usize| -> String {
            (0..n)
                .map(|i| format!("{}k:\n", " ".repeat(i)))
                .collect::<String>()
                + &" ".repeat(n)
                + "v: 1\n"
        };
        assert!(parse(&block(MAX_DEPTH - 1)).is_ok());
        assert!(err(&block(MAX_DEPTH)).contains("nested deeper"));
        let flow = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
        assert!(parse(&flow(MAX_DEPTH)).is_ok());
        assert!(err(&flow(MAX_DEPTH + 1)).contains("nested deeper"));
    }

    #[test]
    fn scalars() {
        for (y, j) in [
            (
                "a: 0o17\nb: 017\nc: 0b101\nd: -0x10\ne: 1_000\nf: +5\ng: 08",
                r#"{"a":15,"b":15,"c":5,"d":-16,"e":1000,"f":5,"g":8}"#,
            ),
            (
                "a: 18446744073709551615\nb: 99999999999999999999\nc: -0\nd: -0.0",
                r#"{"a":18446744073709551615,"b":1e+20,"c":0,"d":0}"#,
            ),
            (
                "a: 1.0\nb: .5\nc: 1e6\nd: 123456.7\ne: 0.0001\nf: 0.00001\ng: 1234567.0",
                r#"{"a":1,"b":0.5,"c":1e+06,"d":123456.7,"e":0.0001,"f":1e-05,"g":1.234567e+06}"#,
            ),
            (
                "a: 2001-12-14\nb: 2001-12-14t21:59:43.10-05:00\nc: 12:30\nd: :8080",
                r#"{"a":"2001-12-14","b":"2001-12-14t21:59:43.10-05:00","c":"12:30","d":":8080"}"#,
            ),
            (
                "a: !!str 12\nb: !!float 1\nc: !!null x\nd: !!binary aGk=\ne: ! 12\nf: !<tag:yaml.org,2002:str> 1",
                r#"{"a":"12","b":1,"c":null,"d":"aGk=","e":12,"f":"1"}"#,
            ),
            ("a: x # c\nb: x#y\nc: -x\nd: a: b", ""),
        ] {
            if j.is_empty() {
                assert!(err(y).contains("not allowed here"), "{y:?}");
            } else {
                assert_eq!(ok(y), j, "{y:?}");
            }
        }
        assert_eq!(
            ok("a: x # c\nb: x#y\nc: -x\nd: 'it''s'\n"),
            r#"{"a":"x","b":"x#y","c":"-x","d":"it's"}"#
        );
        assert_eq!(
            ok(r#"a: "\x41\u00e9\U0001F600\t\"\\ \/""#),
            "{\"a\":\"Aé😀\\t\\\"\\\\ /\"}"
        );
        // Keys are their text, as written.
        assert_eq!(
            ok("1.0: a\ntrue: b\n~: c\n0x10: d\n"),
            r#"{"1.0":"a","true":"b","~":"c","0x10":"d"}"#
        );
        assert!(err("2001-01-01: x\n").contains("a key must be a string"));
        assert_eq!(ok("? a\n: b\n? c\nd: e\n"), r#"{"a":"b","c":null,"d":"e"}"#);
        assert_eq!(ok("{? a: 1, ? b}"), r#"{"a":1,"b":null}"#);
        assert!(err("? [a]\n: b\n").contains("a key must be a string"));
    }

    #[test]
    fn folding() {
        assert_eq!(
            ok("a: one\n  two\n\n  three\n"),
            r#"{"a":"one two\nthree"}"#
        );
        assert_eq!(
            ok("a: \"one\n  two \\\n  three\n\n  four\"\n"),
            r#"{"a":"one two three\nfour"}"#
        );
        assert_eq!(ok("a: 'one\n\n\n  two'\n"), r#"{"a":"one\n\ntwo"}"#);
        assert_eq!(
            ok("a: >\n  one\n  two\n\n  three\n    more\n  four\n"),
            r#"{"a":"one two\nthree\n  more\nfour\n"}"#
        );
        assert_eq!(ok("a: |-\n  x\n\n"), r#"{"a":"x"}"#);
        assert_eq!(ok("a: |+\n  x\n\n"), r#"{"a":"x\n\n"}"#);
        assert_eq!(ok("a: |2\n   x\n  y\n"), r#"{"a":" x\ny\n"}"#);
        assert_eq!(ok("a:\n  - |\n    x\n  - y\n"), r#"{"a":["x\n","y"]}"#);
        assert_eq!(
            ok("a: |\n\n  x\n  # not a comment\nb: 1"),
            r#"{"a":"\nx\n# not a comment\n","b":1}"#
        );
        assert_eq!(ok("- |\n x\n"), r#"["x\n"]"#);
        assert!(err("a: |\n    \n  x\n").contains("bad indentation"));
    }

    #[test]
    fn structure() {
        assert_eq!(ok("a:\n- 1\n- 2\nb: 3\n"), r#"{"a":[1,2],"b":3}"#);
        assert_eq!(
            ok("- a: 1\n  b: 2\n- - x\n  - y\n-\n  c: 3\n- \n"),
            r#"[{"a":1,"b":2},["x","y"],{"c":3},null]"#
        );
        assert_eq!(ok("a:\nb:\n  c:\n"), r#"{"a":null,"b":{"c":null}}"#);
        assert_eq!(
            ok("x: &a\n  k: v\ny: *a\nz: &s s\nw: *s\n"),
            r#"{"x":{"k":"v"},"y":{"k":"v"},"z":"s","w":"s"}"#
        );
        assert_eq!(ok("- &k a: 1\n- *k\n"), r#"[{"a":1},"a"]"#);
        assert_eq!(
            ok("a: !!map\n  b: 1\nc: !!seq\n- 1\n"),
            r#"{"a":{"b":1},"c":[1]}"#
        );
        assert_eq!(
            ok("{a: [1, 2,], \"b\":3, c, d: }"),
            r#"{"a":[1,2],"b":3,"c":null,"d":null}"#
        );
        assert_eq!(
            ok("[a: 1, b, {c: d}, [e]]"),
            r#"[{"a":1},"b",{"c":"d"},["e"]]"#
        );
        assert_eq!(
            ok("[a b,\n  c\n  d, http://x:1/y]"),
            r#"["a b","c d","http://x:1/y"]"#
        );
        assert_eq!(ok("%YAML 1.2\n---\na: 1\n...\n# end\n"), r#"{"a":1}"#);
        assert_eq!(ok("\u{feff}a: 1\r\nb: 2\r\n"), r#"{"a":1,"b":2}"#);
        assert_eq!(ok("--- 5\n"), "5");
        for (y, want) in [
            ("a: 1\n b: 2\n", "more than one line"),
            ("a:\n  b: 1\n c: 2\n", "bad indentation"),
            ("a: 1\nb\n", "expected 'key: value'"),
            ("a:\n  - 1\n - 2\n", "bad indentation"),
            ("a: *x\n", "unknown anchor"),
            ("a: &x *y\n", "alias cannot"),
            ("a: [1, 2] 3\n", "end of the line"),
            ("a: \"x\n", "does not end"),
            ("\ta: 1\n", "a tab where only spaces"),
            ("- \ta\n", "a tab where only spaces"),
            ("a: 1\n\t", "a tab"),
            ("a:\n\tb: 1\n", "a tab where only spaces"),
            ("a: b\n- c\n", "a sequence entry where"),
            ("- a\nb: c\n", "unexpected content"),
            ("%TAG ! tag:x,2000:\n---\na: 1\n", "not supported yet"),
            ("a: !e!x 1\n", "not supported yet"),
            ("a: [1,,2]\n", "comma"),
            ("a: \u{1}\n", "control character"),
            ("a: 1\rb: 2\n", "carriage return"),
            ("a: @x\n", "cannot start"),
            ("a: x\n  b: y\n", "more than one line"),
            ("*a: 1\n", "unknown anchor"),
            ("a: &x 1\n*x : 2\n", "a key must be a string"),
            ("--- a\n--- b\n", "more than one document"),
            ("a: |0\n x\n", "cannot be 0"),
        ] {
            let e = err(y);
            assert!(e.contains(want), "{y:?}: {e}, want {want:?}");
        }
    }

    #[test]
    fn positions() {
        use crate::json::position;
        let src = "listeners:\n  - name: web\n    address: \":8080\"\n    protocols: [http/1.1, h3]\nroutes:\n  - handler:\n      module: respond\n      params:\n        status: 9999\nx: *y";
        let n = parse(src.trim_end_matches("\nx: *y")).unwrap();
        let Kind::Object(top) = &n.kind else { panic!() };
        let Kind::Array(ls) = &top[0].value.kind else {
            panic!()
        };
        let Kind::Object(l) = &ls[0].kind else {
            panic!()
        };
        let Kind::Array(ps) = &l[2].value.kind else {
            panic!()
        };
        assert_eq!(position(src, ps[1].off), (4, 27));
        assert_eq!(position(src, l[1].key_off), (3, 5));
        let e = parse(src).unwrap_err();
        assert_eq!(position(src, e.off), (10, 4));
        let e = parse("a: 1\nb:\n  c: 1\n  c: 2\n").unwrap_err();
        assert_eq!(position("a: 1\nb:\n  c: 1\n  c: 2\n", e.off), (4, 3));
        // An alias's value is where the alias is.
        let src = "a: &x 1\nb: *x\n";
        let n = parse(src).unwrap();
        let Kind::Object(m) = &n.kind else { panic!() };
        assert_eq!(position(src, m[1].value.off), (2, 4));
    }

    /// fit's YAML templates hold, value for value, what their JSON twins hold: the whole trees,
    /// not only the fields the model reads.
    #[test]
    fn templates_are_their_json_twins() {
        let t = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/compat/fixtures/fit/templates");
        let mut files = vec![("yaml/fit.yaml".to_owned(), "json/fit.json".to_owned())];
        let mut stack = vec![t.join("yaml/conf.d")];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let rel = p.strip_prefix(t.join("yaml")).unwrap().to_owned();
                    let json = std::path::Path::new("json").join(rel.with_extension("json"));
                    files.push((
                        format!("yaml/{}", rel.display()),
                        json.display().to_string(),
                    ));
                }
            }
        }
        assert!(files.len() >= 10, "{files:?}");
        let mut written_out = 0;
        for (y, j) in files {
            let ys = std::fs::read_to_string(t.join(&y)).unwrap();
            let js = std::fs::read_to_string(t.join(&j)).unwrap();
            let yn = parse(&ys).unwrap_or_else(|e| panic!("{y}: {e:?}"));
            let jn = crate::json::parse(&js).unwrap();
            assert_eq!(json(&yn), json(&jn), "{y}");
            // And with what is commented out written out: every field fit has, in YAML.
            let (yu, ju) = (uncomment(&ys, true), uncomment(&js, false));
            match (parse(&yu), crate::json::parse(&ju)) {
                (Ok(yn), Ok(jn)) => {
                    // A block whose fields are all prose once written out is `{}` in JSON and
                    // empty, null, in YAML.
                    let (ys, js) = (
                        json(&yn).replace("{}", "null"),
                        json(&jn).replace("{}", "null"),
                    );
                    assert_eq!(ys, js, "{y}, written out");
                    written_out += 1;
                }
                (y_r, j_r) => eprintln!("{y} written out: {:?} / {:?}", y_r.err(), j_r.err()),
            }
        }
        assert!(written_out >= 6, "only {written_out} templates written out");
    }

    /// The template with its commented-out fields written out: one level of comments taken off
    /// each line that holds YAML (`yaml`) or JSON, not prose.
    fn uncomment(src: &str, yaml: bool) -> String {
        let mark = if yaml { "# " } else { "// " };
        let code = |t: &str| {
            if t.starts_with(mark) || t.starts_with(['[', ']', '{', '}', '"']) {
                return true;
            }
            if !yaml {
                return false;
            }
            if t == "-" || t.starts_with("- ") || t.starts_with('|') {
                return true;
            }
            // `key:` and a value, quoted, a collection or one word; not prose with a colon.
            let key = t.split_once(':').map(|(k, rest)| {
                let v = rest.trim();
                let value = v.is_empty()
                    || v.starts_with(['"', '\'', '[', '{', '|', '>', '&', '*', '#'])
                    || !v.contains(' ');
                (k, (rest.is_empty() || rest.starts_with(' ')) && value)
            });
            matches!(key, Some((k, true)) if !k.is_empty()
                && k.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_'./-".contains(&c)))
        };
        let mut out = String::new();
        for line in src.lines() {
            let t = line.trim_start_matches(' ');
            let indent = &line[..line.len() - t.len()];
            match t.strip_prefix(mark).filter(|r| code(r)) {
                Some(rest) => {
                    out.push_str(indent);
                    out.push_str(rest);
                }
                None => out.push_str(line),
            }
            out.push('\n');
        }
        out
    }

    /// The cases fit and akit read differently, each with both verdicts: `cases` are documents,
    /// `fit` fit's verdicts on them, `OK <canonical JSON>` or `ERR...`.
    fn differences<'a>(cases: impl Iterator<Item = (String, &'a str)>) -> (usize, Vec<String>) {
        let mut n = 0;
        let mut out = Vec::new();
        for (src, want) in cases {
            n += 1;
            let got = match parse(&src) {
                Ok(node) => format!("OK {}", canonical(&node)),
                Err(e) => format!("ERR {}", e.msg),
            };
            // An explicit empty document is null to fit, an empty config: akit reads it as {}.
            let same = if want == "OK null" {
                got == "OK {}" || got == want
            } else if want.starts_with("OK") {
                got == want
            } else {
                got.starts_with("ERR")
            };
            if !same {
                out.push(format!("{src:?}\n  fit:  {want}\n  akit: {got}\n"));
            }
        }
        (n, out)
    }

    fn unhex(h: &str) -> Option<String> {
        let bytes: Option<Vec<u8>> = (0..h.len() / 2)
            .map(|i| u8::from_str_radix(h.get(2 * i..2 * i + 2)?, 16).ok())
            .collect();
        String::from_utf8(bytes?).ok()
    }

    /// fit's verdicts on a sample of the differential check's corpus, checked in.
    #[test]
    fn fits_verdicts() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/yaml-fit.txt");
        let text = std::fs::read_to_string(path).unwrap();
        let cases = text.lines().filter(|l| !l.starts_with('#')).map(|l| {
            let (h, v) = l.split_once(' ').unwrap();
            (unhex(h).unwrap(), v)
        });
        let (n, diff) = differences(cases);
        assert!(n > 400, "only {n} cases");
        assert!(diff.is_empty(), "{}", diff.join(""));
    }

    /// The differential check against fit's adapter, run by `tools/yaml-diff/run.sh`:
    /// `AKIT_YAML_DIFF=<dir>` names a directory with `cases.hex` (one hex-encoded document a
    /// line) and `fit.txt` (fit's verdict on each); the cases read differently are written to
    /// `<dir>/akit.diff`.
    #[test]
    #[ignore = "needs fit's verdicts, made with Go by tools/yaml-diff/run.sh"]
    fn differential_against_fit() {
        let Ok(dir) = std::env::var("AKIT_YAML_DIFF") else {
            return;
        };
        let dir = std::path::Path::new(&dir);
        let cases = std::fs::read_to_string(dir.join("cases.hex")).unwrap();
        let fit = std::fs::read_to_string(dir.join("fit.txt")).unwrap();
        let pairs = cases
            .lines()
            .zip(fit.lines())
            .filter_map(|(c, f)| Some((unhex(c)?, f)));
        let (n, diff) = differences(pairs);
        std::fs::write(dir.join("akit.diff"), diff.concat()).unwrap();
        eprintln!("{n} cases, {} read differently", diff.len());
    }

    fn hex(s: &str) -> String {
        s.bytes().map(|b| format!("{b:02x}")).collect()
    }

    /// The tree as the differential check's Go side writes it.
    fn canonical(n: &Node) -> String {
        match &n.kind {
            Kind::Null => "null".into(),
            Kind::Bool(b) => b.to_string(),
            Kind::Number(s) => s.clone(),
            Kind::Str(s) => format!("s{}", hex(s)),
            Kind::Array(items) => {
                let v: Vec<String> = items.iter().map(canonical).collect();
                format!("[{}]", v.join(","))
            }
            Kind::Object(ms) => {
                let v: Vec<String> = ms
                    .iter()
                    .map(|m| format!("k{}:{}", hex(&m.key), canonical(&m.value)))
                    .collect();
                format!("{{{}}}", v.join(","))
            }
        }
    }

    /// A long line, or many short ones, is read in linear time.
    #[test]
    fn long_lines_are_linear() {
        let n = 200_000;
        let flow = format!("a: [{}x]\n", "x, ".repeat(n));
        let t = std::time::Instant::now();
        let v = parse(&flow).unwrap();
        let Kind::Object(m) = &v.kind else { panic!() };
        let Kind::Array(items) = &m[0].value.kind else {
            panic!()
        };
        assert_eq!(items.len(), n + 1);
        let block = "- x\n".repeat(n);
        assert!(parse(&block).is_ok());
        let map: String = (0..n).map(|i| format!("k{i}: v\n")).collect();
        assert!(parse(&map).is_ok());
        assert!(
            t.elapsed() < std::time::Duration::from_secs(20),
            "{:?}",
            t.elapsed()
        );
    }

    #[test]
    fn floats_as_go_writes_them() {
        for (f, s) in [
            (1.0, "1"),
            (1.5, "1.5"),
            (100000.0, "100000"),
            (1e6, "1e+06"),
            (1.5e300, "1.5e+300"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (-2.5, "-2.5"),
            (1e20, "1e+20"),
            (123456789.0, "1.23456789e+08"),
            (0.1 + 0.2, "0.30000000000000004"),
        ] {
            assert_eq!(go_float(f), s, "{f}");
        }
    }

    #[test]
    fn garbage_never_panics() {
        let src = "a: &x {b: [1, 'c', \"d\\n\"]}\nl:\n  - k: |\n      t\n  - *x\n  - <<: *x\n    e: >-\n      f\n";
        for n in 0..=src.len() {
            if let Some(p) = src.get(..n) {
                let _ = parse(p);
            }
        }
        let bytes = src.as_bytes();
        for i in 0..bytes.len() {
            for &b in b"\"'{}[],:-?&*!|>#\n \t%@`\\" {
                let mut v = bytes.to_vec();
                v[i] = b;
                if let Ok(s) = std::str::from_utf8(&v) {
                    let _ = parse(s);
                }
            }
        }
    }
}
