//! fit's expression language (`router/expr.go`): matchers combined with `&&`, `||`, `!` and
//! parentheses, parsed once, when the config is compiled.

use std::fmt;

use crate::matcher::{Error, Kind, Matcher, Request, one_field, parse_prefix, regexp};

/// The longest expression, so that a config cannot make the parser recurse without end (fit's
/// `maxExpr`).
pub const MAX_EXPR: usize = 64 << 10;

/// The deepest nesting of `!` and parentheses (fit's `maxDepth`).
pub const MAX_DEPTH: usize = 100;

/// A parsed expression: matchers combined with `&&` (and), `||` (or), `!` (not) and
/// parentheses, `&&` binding tighter than `||`, as in Go:
///
/// ```text
/// Host("example.com", "*.example.com") && (PathPrefix("/api/") || Method("POST"))
/// ```
///
/// Its matchers take string arguments, quoted as in Go, with `"..."` or `` `...` ``:
///
/// - `Host(host...)`: one of the hosts, exact or a `*.` wildcard for one label, ignoring case,
///   a port and a trailing dot;
/// - `Path(path...)`: one of the paths, exactly;
/// - `PathPrefix(prefix...)`: a path that starts with one of the prefixes;
/// - `PathRegexp(expr...)`: [`Matcher::path_regexp`];
/// - `Method(method...)`: [`Matcher::methods`];
/// - `Header(name, value...)`: a header named `name`, with one of the values, or with any value
///   when none are given ([`Matcher::headers`]);
/// - `Query(key, value...)`: a query parameter, alike ([`Matcher::query`]);
/// - `RemoteIP(net...)`: [`Matcher::remote_ip`].
///
/// Its [`Display`](fmt::Display) prints it back in a canonical form, with only the parentheses
/// its structure needs. Evaluating it does not allocate.
#[derive(Debug, Clone)]
pub struct Expr(Node);

#[derive(Debug, Clone)]
enum Node {
    /// A chain of `&&`, flattened from the left: the first element is never an `And`; a later
    /// one may be, one written in parentheses on the right.
    And(Box<[Node]>),
    /// Likewise for `||`.
    Or(Box<[Node]>),
    Not(Box<Node>),
    Call(Box<Call>),
}

/// A matcher with the call it was written as.
#[derive(Debug, Clone)]
struct Call {
    name: &'static str,
    args: Box<[Box<[u8]>]>,
    m: Matcher,
}

impl Expr {
    /// Parses `s` and builds its matchers.
    ///
    /// # Errors
    ///
    /// fit's messages: where (`at byte N`, 1-based) and what is wrong.
    pub fn parse(s: &str) -> Result<Expr, Error> {
        if s.len() > MAX_EXPR {
            return Err(Error(format!(
                "the expression is longer than {MAX_EXPR} bytes"
            )));
        }
        let mut p = Parser {
            s: s.as_bytes(),
            i: 0,
            tok: Token::default(),
            back: false,
            depth: 0,
        };
        let e = p.or()?;
        let t = p.next()?;
        if t.kind != Tok::Eof {
            return Err(p.error(format_args!("expected && or ||, found {t}")));
        }
        Ok(Expr(e))
    }

    /// Whether `r` matches.
    #[must_use]
    pub fn matches<R: Request + ?Sized>(&self, r: &R) -> bool {
        self.0.matches(r)
    }
}

impl Node {
    fn matches<R: Request + ?Sized>(&self, r: &R) -> bool {
        match self {
            Node::And(es) => es.iter().all(|e| e.matches(r)),
            Node::Or(es) => es.iter().any(|e| e.matches(r)),
            Node::Not(e) => !e.matches(r),
            Node::Call(c) => c.m.matches(r),
        }
    }

    fn is_and(&self) -> bool {
        matches!(self, Node::And(_))
    }

    fn is_or(&self) -> bool {
        matches!(self, Node::Or(_))
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Writes `e`, in parentheses when `paren`.
fn paren(f: &mut fmt::Formatter<'_>, e: &Node, paren: bool) -> fmt::Result {
    if paren {
        write!(f, "({e})")
    } else {
        write!(f, "{e}")
    }
}

impl fmt::Display for Node {
    /// fit's `String`: `&&` binds tighter than `||`, and both group to the left.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Node::And(es) => {
                for (i, e) in es.iter().enumerate() {
                    if i == 0 {
                        paren(f, e, e.is_or())?;
                    } else {
                        f.write_str(" && ")?;
                        paren(f, e, e.is_or() || e.is_and())?;
                    }
                }
                Ok(())
            }
            Node::Or(es) => {
                for (i, e) in es.iter().enumerate() {
                    if i == 0 {
                        write!(f, "{e}")?;
                    } else {
                        f.write_str(" || ")?;
                        paren(f, e, e.is_or())?;
                    }
                }
                Ok(())
            }
            Node::Not(e) => {
                f.write_str("!")?;
                paren(f, e, e.is_or() || e.is_and())
            }
            Node::Call(c) => {
                write!(f, "{}(", c.name)?;
                for (i, a) in c.args.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    f.write_str(&go_quote(a))?;
                }
                f.write_str(")")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tok {
    #[default]
    Eof,
    Ident,
    Str,
    And,
    Or,
    Not,
    LParen,
    RParen,
    Comma,
}

#[derive(Debug, Clone, Default)]
struct Token {
    kind: Tok,
    /// An identifier, or a string's value.
    text: Vec<u8>,
    pos: usize,
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            Tok::Eof => f.write_str("the end"),
            Tok::Ident => f.write_str(&String::from_utf8_lossy(&self.text)),
            Tok::Str => f.write_str(&go_quote(&self.text)),
            Tok::And => f.write_str("\"&&\""),
            Tok::Or => f.write_str("\"||\""),
            Tok::Not => f.write_str("\"!\""),
            Tok::LParen => f.write_str("\"(\""),
            Tok::RParen => f.write_str("\")\""),
            Tok::Comma => f.write_str("\",\""),
        }
    }
}

struct Parser<'s> {
    s: &'s [u8],
    i: usize,
    tok: Token,
    /// `next` returns `tok` again.
    back: bool,
    depth: usize,
}

impl Parser<'_> {
    fn error(&self, msg: fmt::Arguments<'_>) -> Error {
        Error(format!("at byte {}: {msg}", self.tok.pos.saturating_add(1)))
    }

    fn at(&self, i: usize) -> Option<u8> {
        self.s.get(i).copied()
    }

    /// Reads the next token into `tok` and returns it.
    fn next(&mut self) -> Result<Token, Error> {
        if self.back {
            self.back = false;
            return Ok(self.tok.clone());
        }
        while self.at(self.i).is_some_and(|c| b" \t\r\n".contains(&c)) {
            self.i = self.i.saturating_add(1);
        }
        let start = self.i;
        self.tok = Token {
            pos: start,
            ..Token::default()
        };
        let Some(c) = self.at(self.i) else {
            return Ok(self.tok.clone());
        };
        match c {
            b'&' | b'|' => {
                if self.at(self.i.saturating_add(1)) != Some(c) {
                    let c = char::from(c);
                    return Err(self.error(format_args!("expected {c}{c}")));
                }
                self.i = self.i.saturating_add(2);
                self.tok.kind = if c == b'&' { Tok::And } else { Tok::Or };
            }
            b'!' | b'(' | b')' | b',' => {
                self.i = self.i.saturating_add(1);
                self.tok.kind = match c {
                    b'!' => Tok::Not,
                    b'(' => Tok::LParen,
                    b')' => Tok::RParen,
                    _ => Tok::Comma,
                };
            }
            b'"' | b'`' => {
                let mut end = self.i.saturating_add(1);
                while let Some(b) = self.at(end) {
                    if b == c || b == b'\n' {
                        break;
                    }
                    if c == b'"' && b == b'\\' {
                        end = end.saturating_add(1);
                    }
                    end = end.saturating_add(1);
                }
                if self.at(end) != Some(c) {
                    return Err(self.error(format_args!("the string is not closed")));
                }
                let lit = self.s.get(self.i..=end).unwrap_or_default();
                let Some(v) = unquote(lit) else {
                    return Err(self.error(format_args!(
                        "{} is not a valid string",
                        String::from_utf8_lossy(lit)
                    )));
                };
                self.i = end.saturating_add(1);
                self.tok.kind = Tok::Str;
                self.tok.text = v;
            }
            c if is_letter(c) => {
                while self
                    .at(self.i)
                    .is_some_and(|c| is_letter(c) || c.is_ascii_digit())
                {
                    self.i = self.i.saturating_add(1);
                }
                self.tok.kind = Tok::Ident;
                self.tok.text = self.s.get(start..self.i).unwrap_or_default().to_vec();
            }
            c => {
                return Err(self.error(format_args!("unexpected {}", go_quote_rune(char::from(c)))));
            }
        }
        Ok(self.tok.clone())
    }

    /// `a || b || ...`
    fn or(&mut self) -> Result<Node, Error> {
        let mut e = self.and()?;
        loop {
            if self.next()?.kind != Tok::Or {
                self.back = true;
                return Ok(e);
            }
            let b = self.and()?;
            e = match e {
                Node::Or(es) => Node::Or(push(es, b)),
                e => Node::Or(Box::new([e, b])),
            };
        }
    }

    /// `a && b && ...`
    fn and(&mut self) -> Result<Node, Error> {
        let mut e = self.unary()?;
        loop {
            if self.next()?.kind != Tok::And {
                self.back = true;
                return Ok(e);
            }
            let b = self.unary()?;
            e = match e {
                Node::And(es) => Node::And(push(es, b)),
                e => Node::And(Box::new([e, b])),
            };
        }
    }

    /// `!e`, `(e)` and calls.
    fn unary(&mut self) -> Result<Node, Error> {
        self.depth = self.depth.saturating_add(1);
        let r = self.unary_inner();
        self.depth = self.depth.saturating_sub(1);
        r
    }

    fn unary_inner(&mut self) -> Result<Node, Error> {
        if self.depth > MAX_DEPTH {
            return Err(self.error(format_args!("the expression nests deeper than {MAX_DEPTH}")));
        }
        let t = self.next()?;
        match t.kind {
            Tok::Not => Ok(Node::Not(Box::new(self.unary()?))),
            Tok::LParen => {
                let e = self.or()?;
                self.expect(Tok::RParen, "expected ) or an operator")?;
                Ok(e)
            }
            Tok::Ident => self.call(&t),
            _ => Err(self.error(format_args!("expected a matcher, ! or (, found {t}"))),
        }
    }

    fn expect(&mut self, kind: Tok, msg: &str) -> Result<(), Error> {
        let t = self.next()?;
        if t.kind != kind {
            return Err(self.error(format_args!("{msg}, found {t}")));
        }
        Ok(())
    }

    /// Parses the arguments of the matcher `t` names, and builds it.
    fn call(&mut self, t: &Token) -> Result<Node, Error> {
        let Some(name) = MATCHERS.iter().copied().find(|n| n.as_bytes() == t.text) else {
            return Err(self.error(format_args!(
                "there is no matcher {}",
                String::from_utf8_lossy(&t.text)
            )));
        };
        self.expect(Tok::LParen, &format!("expected ( after {name}"))?;
        let mut args: Vec<Box<[u8]>> = Vec::new();
        loop {
            let a = self.next()?;
            if a.kind == Tok::RParen && args.is_empty() {
                break;
            }
            if a.kind != Tok::Str {
                return Err(self.error(format_args!("expected a string, found {a}")));
            }
            args.push(a.text.into());
            let sep = self.next()?;
            if sep.kind == Tok::RParen {
                break;
            }
            if sep.kind != Tok::Comma {
                return Err(self.error(format_args!("expected , or ), found {sep}")));
            }
        }
        let at = t.pos.saturating_add(1);
        if args.is_empty() {
            return Err(Error(format!(
                "at byte {at}: {name} needs at least one argument"
            )));
        }
        let m = build(name, &args).map_err(|e| Error(format!("at byte {at}: {name}: {e}")))?;
        Ok(Node::Call(Box::new(Call {
            name,
            args: args.into(),
            m,
        })))
    }
}

fn push(es: Box<[Node]>, e: Node) -> Box<[Node]> {
    let mut v = es.into_vec();
    v.push(e);
    v.into()
}

fn is_letter(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

/// The matchers an expression may call.
const MATCHERS: [&str; 8] = [
    "Host",
    "Path",
    "PathPrefix",
    "PathRegexp",
    "Method",
    "Header",
    "Query",
    "RemoteIP",
];

/// Builds matcher `name` from its arguments, of which there is at least one.
fn build(name: &str, args: &[Box<[u8]>]) -> Result<Matcher, Error> {
    let kind = match name {
        "Host" => host_call(args)?,
        "Path" => {
            check_paths(args)?;
            Kind::Path(args.into())
        }
        "PathPrefix" => {
            check_paths(args)?;
            Kind::PathPrefix(args.into())
        }
        "PathRegexp" => Kind::PathRegexp(args.iter().map(|a| regexp(a)).collect::<Result<_, _>>()?),
        "Method" => Kind::Methods(args.into()),
        "Header" | "Query" => {
            let (first, rest) = match args.split_first() {
                Some((first, rest)) if !first.is_empty() => (first, rest),
                _ => {
                    return Err(Error(
                        if name == "Header" {
                            "the header's name is empty"
                        } else {
                            "the query parameter's name is empty"
                        }
                        .to_owned(),
                    ));
                }
            };
            let f = one_field(first, rest);
            if name == "Header" {
                Kind::Headers(f)
            } else {
                Kind::Query(f)
            }
        }
        _ => {
            // RemoteIP
            let nets = args
                .iter()
                .map(|a| match std::str::from_utf8(a) {
                    Ok(s) => parse_prefix(s),
                    Err(_) => Err(Error(format!(
                        "{} is not an IP address or a CIDR prefix",
                        go_quote(a)
                    ))),
                })
                .collect::<Result<_, _>>()?;
            Kind::RemoteIp(nets)
        }
    };
    Ok(Matcher(kind))
}

/// `Host`'s matcher: exact hosts and `*.` wildcards, lower case.
fn host_call(args: &[Box<[u8]>]) -> Result<Kind, Error> {
    let mut exact = Vec::new();
    let mut wild = Vec::new();
    for h in args {
        let h = trim_brackets(h);
        let h = h.strip_suffix(b".").unwrap_or(h).to_ascii_lowercase();
        let bad = || {
            Error(format!(
                "{} is not a host or a \"*.\" wildcard",
                go_quote(&h)
            ))
        };
        if let Some(suffix) = h.strip_prefix(b"*") {
            if !suffix.starts_with(b".") || suffix.len() < 2 || suffix.contains(&b'*') {
                return Err(bad());
            }
            wild.push(suffix.into());
            continue;
        }
        if h.is_empty() || h.contains(&b'*') {
            return Err(bad());
        }
        exact.push(h.into());
    }
    Ok(Kind::Host {
        exact: exact.into(),
        wild: wild.into(),
    })
}

/// Go's `strings.Trim(h, "[]")`.
fn trim_brackets(h: &[u8]) -> &[u8] {
    let is = |c: &u8| *c == b'[' || *c == b']';
    let start = h.iter().position(|c| !is(c)).unwrap_or(h.len());
    let end = h
        .iter()
        .rposition(|c| !is(c))
        .map_or(start, |e| e.saturating_add(1));
    h.get(start..end).unwrap_or_default()
}

fn check_paths(args: &[Box<[u8]>]) -> Result<(), Error> {
    match args.iter().find(|a| !a.starts_with(b"/")) {
        Some(a) => Err(Error(format!("{} does not start with '/'", go_quote(a)))),
        None => Ok(()),
    }
}

/// Go's `strconv.Unquote` of a `"..."` or `` `...` `` literal; `None` when it is not valid.
fn unquote(lit: &[u8]) -> Option<Vec<u8>> {
    let (&q, rest) = lit.split_first()?;
    let (&q2, body) = rest.split_last()?;
    if q != q2 {
        return None;
    }
    if q == b'`' {
        if body.contains(&b'`') {
            return None;
        }
        // Go drops carriage returns from raw strings.
        return Some(body.iter().copied().filter(|&c| c != b'\r').collect());
    }
    if body.contains(&b'\n') {
        return None;
    }
    let mut out = Vec::with_capacity(body.len());
    let mut i = body.iter().copied();
    while let Some(c) = i.next() {
        if c == b'"' {
            return None;
        }
        if c != b'\\' {
            out.push(c);
            continue;
        }
        let e = i.next()?;
        let simple = match e {
            b'a' => Some(0x07),
            b'b' => Some(0x08),
            b'f' => Some(0x0c),
            b'n' => Some(b'\n'),
            b'r' => Some(b'\r'),
            b't' => Some(b'\t'),
            b'v' => Some(0x0b),
            b'\\' => Some(b'\\'),
            b'"' => Some(b'"'),
            _ => None,
        };
        if let Some(b) = simple {
            out.push(b);
            continue;
        }
        match e {
            b'0'..=b'7' => {
                let mut v = u32::from(e.wrapping_sub(b'0'));
                for _ in 0..2 {
                    let d = i.next().filter(|d| (b'0'..=b'7').contains(d))?;
                    v = v
                        .wrapping_mul(8)
                        .wrapping_add(u32::from(d.wrapping_sub(b'0')));
                }
                out.push(u8::try_from(v).ok()?);
            }
            b'x' | b'u' | b'U' => {
                let n = match e {
                    b'x' => 2,
                    b'u' => 4,
                    _ => 8,
                };
                let mut v: u32 = 0;
                for _ in 0..n {
                    let d = i.next().and_then(|d| char::from(d).to_digit(16))?;
                    v = v.wrapping_mul(16).wrapping_add(d);
                }
                if e == b'x' {
                    out.push(u8::try_from(v).ok()?);
                } else {
                    let c = char::from_u32(v)?;
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
            }
            _ => return None,
        }
    }
    Some(out)
}

/// Go's `strconv.Quote` of `s`, whose bytes need not be UTF-8: printable characters as they
/// are, the rest escaped.
pub(crate) fn go_quote(s: &[u8]) -> String {
    let mut out = String::with_capacity(s.len().saturating_add(2));
    out.push('"');
    for chunk in s.utf8_chunks() {
        for c in chunk.valid().chars() {
            escape(&mut out, c, '"');
        }
        for b in chunk.invalid() {
            out.push_str(&format!("\\x{b:02x}"));
        }
    }
    out.push('"');
    out
}

/// Go's `strconv.QuoteRune`.
fn go_quote_rune(c: char) -> String {
    let mut out = String::from("'");
    escape(&mut out, c, '\'');
    out.push('\'');
    out
}

/// Writes `c` as Go's quoting does inside quotes `q`.
fn escape(out: &mut String, c: char, q: char) {
    match c {
        '\x07' => out.push_str("\\a"),
        '\x08' => out.push_str("\\b"),
        '\x0c' => out.push_str("\\f"),
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        '\x0b' => out.push_str("\\v"),
        '\\' => out.push_str("\\\\"),
        c if c == q => {
            out.push('\\');
            out.push(c);
        }
        c if u32::from(c) < 0x20 || c == '\x7f' => {
            out.push_str(&format!("\\x{:02x}", u32::from(c)));
        }
        c if !printable(c) => {
            if u32::from(c) < 0x1_0000 {
                out.push_str(&format!("\\u{:04x}", u32::from(c)));
            } else {
                out.push_str(&format!("\\U{:08x}", u32::from(c)));
            }
        }
        c => out.push(c),
    }
}

/// Go's `unicode.IsPrint`, approximately: Rust's standard library has no Unicode categories,
/// so this takes control characters, spaces other than U+0020, and the format characters and
/// noncharacters commonly met as not printable. Either way the quoted text unquotes to `c`.
fn printable(c: char) -> bool {
    !(c.is_control()
        || (c.is_whitespace() && c != ' ')
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
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{206f}'
                | '\u{e000}'..='\u{f8ff}'
                | '\u{feff}'
                | '\u{fff9}'..='\u{fffb}'
                | '\u{fffe}'
                | '\u{ffff}'
        ))
}
