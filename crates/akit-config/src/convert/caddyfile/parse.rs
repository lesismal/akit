//! The Caddyfile's syntax, as fit's `adapter/caddyfile` reads it: tokens (words, quoted strings,
//! backquoted strings, heredocs), `{$NAME}` placeholders from the environment, snippets and their
//! imports, and server blocks of directives.

use std::collections::HashMap;

use crate::convert::gostd::atoi;

/// A word of a Caddyfile: a bare word, a quoted string, a heredoc, or one of the braces.
#[derive(Debug, Clone, Default)]
struct Token {
    text: String,
    quoted: bool,
    line: i64,
    /// A line ended between the token before it and this one.
    nl: bool,
}

impl Token {
    fn brace(&self, b: &str) -> bool {
        !self.quoted && self.text == b
    }
}

/// One line of a block, with the block it opens: a site's directive, a directive's
/// subdirective, or a global option.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Directive {
    pub(crate) name: String,
    pub(crate) args: Vec<String>,
    pub(crate) line: i64,
    pub(crate) block: Vec<Directive>,
    pub(crate) is_block: bool,
}

/// A block at the top of a Caddyfile: the global options (no keys), or a site, with its
/// addresses as keys.
#[derive(Debug, Clone, Default)]
pub(crate) struct ServerBlock {
    pub(crate) keys: Vec<String>,
    pub(crate) line: i64,
    pub(crate) dirs: Vec<Directive>,
}

/// The bounds on what a Caddyfile may hold.
pub(crate) const MAX_TOKENS: usize = 200_000;
const MAX_NESTING: usize = 64;
const MAX_IMPORTS: usize = 1000;

fn count_lines(s: &[u8]) -> i64 {
    i64::try_from(s.iter().filter(|&&c| c == b'\n').count()).unwrap_or(i64::MAX)
}

/// Splits a Caddyfile into its tokens.
fn lex(src: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<Vec<Token>, String> {
    let b = src.as_bytes();
    let mut toks = Vec::new();
    let (mut line, mut nl) = (1i64, true);
    let mut i = 0usize;
    while let Some(&c) = b.get(i) {
        match c {
            b'\n' => {
                line += 1;
                nl = true;
                i += 1;
                continue;
            }
            b' ' | b'\t' | b'\r' => {
                i += 1;
                continue;
            }
            b'\\'
                if b.get(i + 1) == Some(&b'\n')
                    || b.get(i + 1) == Some(&b'\r') && b.get(i + 2) == Some(&b'\n') =>
            {
                // A line continued on the next.
                while b.get(i).is_some_and(|&c| c != b'\n') {
                    i += 1;
                }
                line += 1;
                i += 1;
                continue;
            }
            b'#' => {
                while b.get(i).is_some_and(|&c| c != b'\n') {
                    i += 1;
                }
                continue;
            }
            _ => {}
        }
        if toks.len() >= MAX_TOKENS {
            return Err(format!("line {line}: more than {MAX_TOKENS} tokens"));
        }
        let mut t = Token {
            line,
            nl,
            ..Token::default()
        };
        nl = false;
        let rest = src.get(i..).unwrap_or_default();
        if c == b'"' {
            i += 1;
            let mut s = Vec::new();
            loop {
                let Some(&d) = b.get(i) else {
                    return Err(format!(
                        "line {}: a quoted string that does not end",
                        t.line
                    ));
                };
                i += 1;
                if d == b'"' {
                    break;
                }
                let mut d = d;
                if d == b'\\' && b.get(i) == Some(&b'"') {
                    d = b'"';
                    i += 1;
                }
                if d == b'\n' {
                    line += 1;
                }
                s.push(d);
            }
            t.text = String::from_utf8_lossy(&s).into_owned();
            t.quoted = true;
        } else if c == b'`' {
            let Some(end) = rest.get(1..).and_then(|r| r.find('`')) else {
                return Err(format!(
                    "line {}: a quoted string that does not end",
                    t.line
                ));
            };
            t.text = rest.get(1..1 + end).unwrap_or_default().to_owned();
            t.quoted = true;
            line += count_lines(t.text.as_bytes());
            i += end + 2;
        } else if rest.starts_with("<<") && heredoc_marker(rest.get(2..).unwrap_or_default()) {
            let (text, n, lines) = heredoc(rest, line)?;
            t.text = text;
            t.quoted = true;
            line += lines;
            i += n;
        } else {
            let start = i;
            while b
                .get(i)
                .is_some_and(|&d| !matches!(d, b' ' | b'\t' | b'\r' | b'\n'))
            {
                i += 1;
            }
            t.text = String::from_utf8_lossy(b.get(start..i).unwrap_or_default()).into_owned();
        }
        t.text = expand_env(&t.text, env);
        toks.push(t);
    }
    Ok(toks)
}

/// `^[A-Za-z0-9_-]+\r?\n`.
fn heredoc_marker(s: &str) -> bool {
    let b = s.as_bytes();
    let n = b
        .iter()
        .position(|c| !(c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')))
        .unwrap_or(b.len());
    if n == 0 {
        return false;
    }
    let r = b.get(n..).unwrap_or_default();
    r.starts_with(b"\n") || r.starts_with(b"\r\n")
}

/// Reads a heredoc, `<<MARKER`, the lines after it, and MARKER on a line of its own, whose
/// indentation is taken off every line: the text, how many bytes it took and how many lines.
fn heredoc(src: &str, line: i64) -> Result<(String, usize, i64), String> {
    let eol = src.find('\n').unwrap_or(src.len());
    let marker = src.get(2..eol).unwrap_or_default().trim_end_matches('\r');
    let lines: Vec<&str> = src.get(eol + 1..).unwrap_or_default().split('\n').collect();
    for (n, l) in lines.iter().enumerate() {
        let l = l.trim_end_matches('\r');
        let indent = l.trim_end_matches([' ', '\t']);
        if !indent.ends_with(marker) || indent.trim_start_matches([' ', '\t']) != marker {
            continue;
        }
        let indent = indent
            .get(..indent.len() - marker.len())
            .unwrap_or_default();
        let mut body = Vec::with_capacity(n);
        for b in lines.get(..n).unwrap_or_default() {
            let b = b.trim_end_matches('\r');
            if !b.is_empty() && !b.starts_with(indent) {
                return Err(format!(
                    "line {line}: a heredoc line that is not indented as its closing {marker}"
                ));
            }
            body.push(b.strip_prefix(indent).unwrap_or(b));
        }
        let mut size = eol + 1;
        for b in lines.get(..=n).unwrap_or_default() {
            size += b.len() + 1;
        }
        size -= 1;
        let size = size.min(src.len());
        let lines = i64::try_from(n + 1).unwrap_or(i64::MAX);
        return Ok((body.join("\n"), size, lines));
    }
    Err(format!(
        "line {line}: a heredoc that does not end with {marker}"
    ))
}

/// The tokens of `src`: their texts, lines, and whether a line ended before each.
#[cfg(test)]
pub(crate) fn lex_texts(
    src: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Vec<(String, i64, bool)>, String> {
    Ok(lex(src, env)?
        .into_iter()
        .map(|t| (t.text, t.line, t.nl))
        .collect())
}

/// Replaces `{$NAME}` and `{$NAME:default}` with the environment's values, as Caddy does when
/// it reads a Caddyfile.
fn expand_env(s: &str, env: &dyn Fn(&str) -> Option<String>) -> String {
    if !s.contains("{$") {
        return s.to_owned();
    }
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    let mut copied = 0usize;
    while i < b.len() {
        if let Some((end, name, default)) = env_placeholder(s, i) {
            out.push_str(s.get(copied..i).unwrap_or_default());
            out.push_str(&env(name).unwrap_or_else(|| default.to_owned()));
            i = end;
            copied = end;
        } else {
            i += 1;
        }
    }
    out.push_str(s.get(copied..).unwrap_or_default());
    out
}

/// `\{\$([A-Za-z_][A-Za-z0-9_]*)(?::([^}]*))?\}` at `i`: its end, the name and the default.
fn env_placeholder(s: &str, i: usize) -> Option<(usize, &str, &str)> {
    let b = s.as_bytes();
    if b.get(i) != Some(&b'{') || b.get(i + 1) != Some(&b'$') {
        return None;
    }
    let start = i + 2;
    if !b
        .get(start)
        .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
    {
        return None;
    }
    let mut j = start + 1;
    while b
        .get(j)
        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
    {
        j += 1;
    }
    let name = s.get(start..j)?;
    match b.get(j) {
        Some(b'}') => Some((j + 1, name, "")),
        Some(b':') => {
            let close = s.get(j + 1..)?.find('}')?;
            Some((j + 2 + close, name, s.get(j + 1..j + 1 + close)?))
        }
        _ => None,
    }
}

/// Reads a Caddyfile into its server blocks, with snippets imported, and the warnings reading
/// it gave.
pub(crate) fn parse(
    src: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<(Vec<ServerBlock>, Vec<String>), String> {
    let toks = lex(src, env)?;
    let mut p = Parser::default();
    let toks = p.take_snippets(toks)?;
    let toks = p.expand(toks, 0)?;
    p.toks = toks;
    let blocks = p.blocks()?;
    Ok((blocks, p.warnings))
}

#[derive(Default)]
struct Parser {
    toks: Vec<Token>,
    pos: usize,
    snippets: HashMap<String, Vec<Token>>,
    imports: usize,
    warnings: Vec<String>,
}

/// The index of the `}` that closes the `{` at `open`.
fn match_brace(toks: &[Token], open: usize) -> Result<usize, String> {
    let mut depth = 0usize;
    for (i, t) in toks.iter().enumerate().skip(open) {
        if t.brace("{") {
            depth += 1;
            if depth > MAX_NESTING {
                return Err(format!("line {}: blocks nested too deep", t.line));
            }
        } else if t.brace("}") {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Ok(i);
            }
        }
    }
    Err(format!(
        "line {}: a block that does not end",
        toks.get(open).map_or(0, |t| t.line)
    ))
}

/// Whether `s` is `{args[N]}`, `{args[:]}` or `{args.N}` at `i`: its end and the index (`None`
/// for all of them).
fn snippet_arg(s: &str, i: usize) -> Option<(usize, Option<&str>)> {
    let r = s.get(i..)?.strip_prefix("{args")?;
    let base = i + 5;
    let digits = |t: &str| t.bytes().take_while(u8::is_ascii_digit).count();
    if let Some(t) = r.strip_prefix('[') {
        if let Some(t2) = t.strip_prefix(":]}") {
            let _ = t2;
            return Some((base + 4, None));
        }
        let n = digits(t);
        if n > 0 && t.get(n..)?.starts_with("]}") {
            return Some((base + 1 + n + 2, Some(t.get(..n)?)));
        }
        return None;
    }
    if let Some(t) = r.strip_prefix('.') {
        let n = digits(t);
        if n > 0 && t.get(n..)?.starts_with('}') {
            return Some((base + 1 + n + 1, Some(t.get(..n)?)));
        }
    }
    None
}

/// `text` with each `{args...}` replaced with the import's arguments.
fn replace_args(text: &str, args: &[String]) -> String {
    if !text.contains("{args") {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let (mut i, mut copied) = (0usize, 0usize);
    while i < text.len() {
        if let Some((end, idx)) = snippet_arg(text, i) {
            out.push_str(text.get(copied..i).unwrap_or_default());
            match idx {
                None => out.push_str(&args.join(" ")),
                Some(n) => {
                    let idx = match atoi(n) {
                        Ok(n) | Err(Some(n)) => n,
                        Err(None) => 0,
                    };
                    if let Some(a) = usize::try_from(idx).ok().and_then(|i| args.get(i)) {
                        out.push_str(a);
                    }
                }
            }
            i = end;
            copied = end;
        } else {
            i += 1;
        }
    }
    out.push_str(text.get(copied..).unwrap_or_default());
    out
}

impl Parser {
    fn warn(&mut self, line: i64, msg: String) {
        self.warnings.push(format!("line {line}: {msg}"));
    }

    /// Takes the snippets, `(name) { ... }` at the top, out of `toks`.
    fn take_snippets(&mut self, toks: Vec<Token>) -> Result<Vec<Token>, String> {
        let mut out = Vec::with_capacity(toks.len());
        let mut i = 0usize;
        while let Some(t) = toks.get(i) {
            let tb = t.text.as_bytes();
            if t.nl
                && !t.quoted
                && tb.len() > 2
                && tb.first() == Some(&b'(')
                && tb.last() == Some(&b')')
                && toks.get(i + 1).is_some_and(|n| n.brace("{") && !n.nl)
            {
                let end = match_brace(&toks, i + 1)?;
                let name = t
                    .text
                    .get(1..t.text.len() - 1)
                    .unwrap_or_default()
                    .to_owned();
                if self.snippets.contains_key(&name) {
                    return Err(format!("line {}: snippet {name} is defined twice", t.line));
                }
                self.snippets
                    .insert(name, toks.get(i + 2..end).unwrap_or_default().to_vec());
                i = end + 1;
                continue;
            }
            // Only blocks at the top hold snippets: skip over the others.
            if t.brace("{") {
                let end = match_brace(&toks, i)?;
                out.extend_from_slice(toks.get(i..=end).unwrap_or_default());
                i = end + 1;
                continue;
            }
            out.push(t.clone());
            i += 1;
        }
        Ok(out)
    }

    /// Replaces each import of a snippet with the snippet's tokens, its `{args[N]}` replaced
    /// with the import's arguments.
    fn expand(&mut self, mut toks: Vec<Token>, depth: usize) -> Result<Vec<Token>, String> {
        if depth > 10 {
            return Err("snippets import each other too deep".into());
        }
        let mut out: Vec<Token> = Vec::new();
        let mut i = 0usize;
        while i < toks.len() {
            let Some(t) = toks.get(i).cloned() else { break };
            let line_start = t.nl || i > 0 && toks.get(i - 1).is_some_and(|p| p.brace("{"));
            if !line_start || t.quoted || t.text != "import" {
                out.push(t);
                i += 1;
                continue;
            }
            let mut args = Vec::new();
            let mut j = i + 1;
            while let Some(a) = toks.get(j) {
                if a.nl || a.brace("{") || a.brace("}") {
                    break;
                }
                args.push(a.text.clone());
                j += 1;
            }
            i = j;
            let Some(first) = args.first() else {
                return Err(format!("line {}: import names no snippet", t.line));
            };
            let Some(body) = self.snippets.get(first).cloned() else {
                self.warn(t.line, format!("import {first}: skipped: importing files is not converted; only snippets defined in the Caddyfile are"));
                continue;
            };
            self.imports += 1;
            if self.imports > MAX_IMPORTS {
                return Err(format!("line {}: more than {MAX_IMPORTS} imports", t.line));
            }
            let args = args.get(1..).unwrap_or_default();
            let mut sub: Vec<Token> = body
                .into_iter()
                .map(|mut b| {
                    b.text = replace_args(&b.text, args);
                    b
                })
                .collect();
            if let Some(f) = sub.first_mut() {
                f.nl = true;
            }
            if out.len() + sub.len() > MAX_TOKENS {
                return Err(format!(
                    "line {}: more than {MAX_TOKENS} tokens once snippets are imported",
                    t.line
                ));
            }
            let sub = self.expand(sub, depth + 1)?;
            out.extend(sub);
            // What follows the import starts a line of its own.
            if let Some(n) = toks.get_mut(j) {
                n.nl = true;
            }
        }
        Ok(out)
    }

    fn peek(&self) -> Option<&Token> {
        self.toks.get(self.pos)
    }

    /// Reads the server blocks: the global options, if they come first, and the sites. A
    /// Caddyfile of one site may leave out its braces.
    fn blocks(&mut self) -> Result<Vec<ServerBlock>, String> {
        let mut out: Vec<ServerBlock> = Vec::new();
        loop {
            let Some(t) = self.peek().cloned() else {
                return Ok(out);
            };
            let mut b = ServerBlock {
                line: t.line,
                ..ServerBlock::default()
            };
            if t.brace("{") {
                if !out.is_empty() {
                    return Err(format!(
                        "line {}: a block with no site address; global options come first",
                        t.line
                    ));
                }
                self.pos += 1;
                b.dirs = self.directives(1)?;
                out.push(b);
                continue;
            }
            if t.brace("}") {
                return Err(format!("line {}: an unexpected \"}}\"", t.line));
            }
            // The keys: the addresses on the line, and on the lines after it while one ends
            // with a comma.
            let mut more = false;
            let mut first = true;
            while let Some(t) = self.peek() {
                if t.brace("{") || t.brace("}") || !first && t.nl && !more {
                    break;
                }
                let text = t.text.clone();
                self.pos += 1;
                more = text.ends_with(',');
                for k in text.split(',') {
                    if !k.is_empty() {
                        b.keys.push(k.to_owned());
                    }
                }
                first = false;
            }
            if self.peek().is_some_and(|t| t.brace("{") && !t.nl) {
                self.pos += 1;
                b.dirs = self.directives(1)?;
                out.push(b);
                continue;
            }
            // A site without braces: the rest of the Caddyfile is its own.
            if out.iter().any(|o| !o.keys.is_empty()) {
                return Err(format!(
                    "line {}: site {} has no braces, which only a Caddyfile of one site may leave out",
                    b.line,
                    b.keys.join(", ")
                ));
            }
            b.dirs = self.directives(0)?;
            out.push(b);
            return Ok(out);
        }
    }

    /// Reads the lines of a block, up to the `}` that closes it, or the end of the Caddyfile
    /// at depth 0.
    fn directives(&mut self, depth: usize) -> Result<Vec<Directive>, String> {
        if depth > MAX_NESTING {
            return Err("blocks nested too deep".into());
        }
        let mut ds = Vec::new();
        loop {
            let Some(t) = self.peek().cloned() else {
                if depth > 0 {
                    let line = self.toks.last().map_or(1, |t| t.line);
                    return Err(format!("line {line}: a block that does not end"));
                }
                return Ok(ds);
            };
            self.pos += 1;
            if t.brace("}") {
                if depth == 0 {
                    return Err(format!("line {}: an unexpected \"}}\"", t.line));
                }
                return Ok(ds);
            }
            if t.brace("{") {
                return Err(format!(
                    "line {}: a block with no directive before it",
                    t.line
                ));
            }
            let mut d = Directive {
                name: t.text,
                line: t.line,
                ..Directive::default()
            };
            while let Some(a) = self.peek().cloned() {
                if a.nl || a.brace("}") {
                    break;
                }
                self.pos += 1;
                if a.brace("{") {
                    d.is_block = true;
                    d.block = self.directives(depth + 1)?;
                    break;
                }
                d.args.push(a.text);
            }
            ds.push(d);
        }
    }
}
