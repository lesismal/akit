//! nginx's config syntax, as fit's `adapter/nginx` reads it: directives, their arguments with
//! quotes taken off, and their blocks; OpenResty's `*_by_lua_block` blocks passed over as code.

/// One nginx directive: its name, its arguments, the line it starts on, and its block.
#[derive(Debug, Clone, Default)]
pub(crate) struct Directive {
    pub(crate) name: String,
    pub(crate) args: Vec<String>,
    pub(crate) line: i64,
    pub(crate) block: Vec<Directive>,
}

/// The bounds on what a config may hold.
pub(crate) const MAX_DIRECTIVES: usize = 100_000;
const MAX_NESTING: usize = 64;

/// Reads an nginx config into its directives.
pub(crate) fn parse(src: &str) -> Result<Vec<Directive>, String> {
    let mut p = Parser {
        src: src.as_bytes(),
        pos: 0,
        line: 1,
        count: 0,
    };
    let ds = p.block(0)?;
    if p.pos < p.src.len() {
        return Err(format!("line {}: an unexpected \"}}\"", p.line));
    }
    Ok(ds)
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
    line: i64,
    count: usize,
}

/// A word, a quoted string, or one of `;` `{` `}`.
struct Token {
    text: String,
    quoted: bool,
    line: i64,
}

impl Token {
    fn is(&self, s: &str) -> bool {
        !self.quoted && self.text == s
    }
}

fn errorf(line: i64, msg: &str) -> String {
    format!("line {line}: {msg}")
}

impl Parser<'_> {
    fn at(&self, i: usize) -> Option<u8> {
        self.src.get(i).copied()
    }

    fn text(&self, from: usize, to: usize) -> String {
        String::from_utf8_lossy(self.src.get(from..to).unwrap_or_default()).into_owned()
    }

    /// The next token, or `None` at the end.
    fn next(&mut self) -> Result<Option<Token>, String> {
        while let Some(c) = self.at(self.pos) {
            match c {
                b'\n' => {
                    self.line += 1;
                    self.pos += 1;
                }
                b' ' | b'\t' | b'\r' => self.pos += 1,
                b'#' => {
                    while self.at(self.pos).is_some_and(|c| c != b'\n') {
                        self.pos += 1;
                    }
                }
                b';' | b'{' | b'}' => {
                    self.pos += 1;
                    return Ok(Some(Token {
                        text: char::from(c).to_string(),
                        quoted: false,
                        line: self.line,
                    }));
                }
                b'"' | b'\'' => {
                    let start = self.line;
                    self.pos += 1;
                    let mut b = Vec::new();
                    loop {
                        let Some(mut d) = self.at(self.pos) else {
                            return Err(errorf(start, "a quoted string that does not end"));
                        };
                        self.pos += 1;
                        if d == c {
                            break;
                        }
                        if d == b'\\'
                            && let Some(e) = self.at(self.pos)
                        {
                            self.pos += 1;
                            d = match e {
                                b'"' | b'\'' | b'\\' => e,
                                b'n' => b'\n',
                                b't' => b'\t',
                                _ => {
                                    b.push(b'\\');
                                    e
                                }
                            };
                        }
                        if d == b'\n' {
                            self.line += 1;
                        }
                        b.push(d);
                    }
                    return Ok(Some(Token {
                        text: String::from_utf8_lossy(&b).into_owned(),
                        quoted: true,
                        line: start,
                    }));
                }
                _ => {
                    let start = self.pos;
                    while let Some(d) = self.at(self.pos) {
                        if matches!(d, b' ' | b'\t' | b'\r' | b'\n' | b';' | b'{' | b'}') {
                            break;
                        }
                        if d == b'\\' && self.pos + 1 < self.src.len() {
                            self.pos += 1;
                        }
                        // A variable in braces, ${name}, holds its brace.
                        if d == b'$'
                            && self.at(self.pos + 1) == Some(b'{')
                            && let Some(end) = self
                                .src
                                .get(self.pos..)
                                .and_then(|r| r.iter().position(|&x| x == b'}'))
                            && end > 0
                        {
                            self.pos += end;
                        }
                        self.pos += 1;
                    }
                    let end = self.pos.min(self.src.len());
                    return Ok(Some(Token {
                        text: self.text(start, end),
                        quoted: false,
                        line: self.line,
                    }));
                }
            }
        }
        Ok(None)
    }

    /// Passes over the Lua code of an OpenResty `*_by_lua_block` to the `}` that ends it: braces
    /// in Lua's strings, long brackets and comments do not count.
    fn skip_lua(&mut self, d: &Directive) -> Result<(), String> {
        let mut depth = 1usize;
        while let Some(c) = self.at(self.pos) {
            match c {
                b'\n' => {
                    self.line += 1;
                    self.pos += 1;
                }
                b'{' => {
                    depth += 1;
                    self.pos += 1;
                }
                b'}' => {
                    self.pos += 1;
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                b'-' if self
                    .src
                    .get(self.pos..)
                    .is_some_and(|r| r.starts_with(b"--")) =>
                {
                    self.pos += 2;
                    if self.long_bracket() {
                        continue;
                    }
                    while self.at(self.pos).is_some_and(|c| c != b'\n') {
                        self.pos += 1;
                    }
                }
                b'[' if self.long_bracket() => {}
                b'"' | b'\'' => {
                    self.pos += 1;
                    // A string ends at its quote, or, unended, at the line's end.
                    while let Some(x) = self.at(self.pos) {
                        if x == c || x == b'\n' {
                            break;
                        }
                        if x == b'\\' && self.at(self.pos + 1).is_some_and(|n| n != b'\n') {
                            self.pos += 1;
                        }
                        self.pos += 1;
                    }
                    if self.at(self.pos) == Some(c) {
                        self.pos += 1;
                    }
                }
                _ => self.pos += 1,
            }
        }
        Err(errorf(
            d.line,
            &format!("{}: a block of Lua that does not end", d.name),
        ))
    }

    /// Passes over a Lua long bracket, `[[...]]` or `[==[...]==]`, at the parser's place, and
    /// says whether there was one.
    fn long_bracket(&mut self) -> bool {
        let s = self.src.get(self.pos..).unwrap_or_default();
        if !s.starts_with(b"[") {
            return false;
        }
        let mut level = 1;
        while s.get(level) == Some(&b'=') {
            level += 1;
        }
        if s.get(level) != Some(&b'[') {
            return false;
        }
        let mut end = vec![b']'];
        end.extend(std::iter::repeat_n(b'=', level - 1));
        end.push(b']');
        let body_from = level + 1;
        let found = s
            .get(body_from..)
            .and_then(|r| r.windows(end.len()).position(|w| w == end.as_slice()));
        let Some(i) = found else {
            self.line += lines(s);
            self.pos = self.src.len();
            return true;
        };
        let len = body_from + i + end.len();
        self.line += lines(s.get(..len).unwrap_or_default());
        self.pos += len;
        true
    }

    /// Reads directives to the `}` that ends the block, or the end of the config at depth 0.
    fn block(&mut self, depth: usize) -> Result<Vec<Directive>, String> {
        if depth > MAX_NESTING {
            return Err(errorf(self.line, "blocks nested too deep"));
        }
        let mut ds = Vec::new();
        loop {
            let Some(t) = self.next()? else {
                if depth > 0 {
                    return Err(errorf(self.line, "a block that does not end"));
                }
                return Ok(ds);
            };
            if t.is("}") {
                if depth == 0 {
                    self.pos -= 1;
                }
                return Ok(ds);
            }
            if t.is(";") || t.is("{") {
                return Err(errorf(
                    t.line,
                    &format!(
                        "a {} with no directive before it",
                        super::gostd::quote(&t.text)
                    ),
                ));
            }
            self.count += 1;
            if self.count > MAX_DIRECTIVES {
                return Err(errorf(
                    t.line,
                    &format!("more than {MAX_DIRECTIVES} directives"),
                ));
            }
            let mut d = Directive {
                name: t.text,
                line: t.line,
                ..Directive::default()
            };
            loop {
                let Some(a) = self.next()? else {
                    return Err(errorf(
                        d.line,
                        &format!("{} does not end with \";\"", d.name),
                    ));
                };
                if a.is(";") {
                    break;
                }
                if a.is("{") {
                    if d.name.ends_with("_by_lua_block") {
                        // OpenResty's Lua: code, not directives, kept out.
                        self.skip_lua(&d)?;
                        break;
                    }
                    d.block = self.block(depth + 1)?;
                    break;
                }
                if a.is("}") {
                    return Err(errorf(
                        a.line,
                        &format!("{} does not end with \";\"", d.name),
                    ));
                }
                d.args.push(a.text);
            }
            ds.push(d);
        }
    }
}

fn lines(b: &[u8]) -> i64 {
    i64::try_from(b.iter().filter(|&&c| c == b'\n').count()).unwrap_or(i64::MAX)
}
