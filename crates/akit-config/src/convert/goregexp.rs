//! Go's `regexp.Compile`, for what fit's converters ask of it: whether a pattern compiles, and
//! when it does not, Go's error, word for word (`error parsing regexp: missing closing ):
//! `(``), which fit's warnings quote. It follows Go's `regexp/syntax` parser with the `Perl`
//! flags step by step, keeping of the parse tree only what decides its errors: the repetitions,
//! whose nested counts Go bounds at 1000, and the tree's height. Go's bound on a program's size
//! (`expression too large`) is not checked; a pattern that big is not met in a config.

/// The Unicode class names Go's `\p{...}` takes, as Go's `canonicalName` writes them (Go
/// 1.27's tables, which fit's pinned toolchain has).
const UNICODE_NAMES: &[&str] = &[
    "Adlam",
    "Ahom",
    "Anatolianhieroglyphs",
    "Any",
    "Arabic",
    "Armenian",
    "Ascii",
    "Assigned",
    "Avestan",
    "Balinese",
    "Bamum",
    "Bassavah",
    "Batak",
    "Bengali",
    "Beriaerfe",
    "Bhaiksuki",
    "Bopomofo",
    "Brahmi",
    "Braille",
    "Buginese",
    "Buhid",
    "C",
    "Canadianaboriginal",
    "Carian",
    "Casedletter",
    "Caucasianalbanian",
    "Cc",
    "Cf",
    "Chakma",
    "Cham",
    "Cherokee",
    "Chorasmian",
    "Closepunctuation",
    "Cn",
    "Cntrl",
    "Co",
    "Combiningmark",
    "Common",
    "Connectorpunctuation",
    "Control",
    "Coptic",
    "Cs",
    "Cuneiform",
    "Currencysymbol",
    "Cypriot",
    "Cyprominoan",
    "Cyrillic",
    "Dashpunctuation",
    "Decimalnumber",
    "Deseret",
    "Devanagari",
    "Digit",
    "Divesakuru",
    "Dogra",
    "Duployan",
    "Egyptianhieroglyphs",
    "Elbasan",
    "Elymaic",
    "Enclosingmark",
    "Ethiopic",
    "Finalpunctuation",
    "Format",
    "Garay",
    "Georgian",
    "Glagolitic",
    "Gothic",
    "Grantha",
    "Greek",
    "Gujarati",
    "Gunjalagondi",
    "Gurmukhi",
    "Gurungkhema",
    "Han",
    "Hangul",
    "Hanifirohingya",
    "Hanunoo",
    "Hatran",
    "Hebrew",
    "Hiragana",
    "Imperialaramaic",
    "Inherited",
    "Initialpunctuation",
    "Inscriptionalpahlavi",
    "Inscriptionalparthian",
    "Javanese",
    "Kaithi",
    "Kannada",
    "Katakana",
    "Kawi",
    "Kayahli",
    "Kharoshthi",
    "Khitansmallscript",
    "Khmer",
    "Khojki",
    "Khudawadi",
    "Kiratrai",
    "L",
    "Lao",
    "Latin",
    "Lc",
    "Lepcha",
    "Letter",
    "Letternumber",
    "Limbu",
    "Lineara",
    "Linearb",
    "Lineseparator",
    "Lisu",
    "Ll",
    "Lm",
    "Lo",
    "Lowercaseletter",
    "Lt",
    "Lu",
    "Lycian",
    "Lydian",
    "M",
    "Mahajani",
    "Makasar",
    "Malayalam",
    "Mandaic",
    "Manichaean",
    "Marchen",
    "Mark",
    "Masaramgondi",
    "Mathsymbol",
    "Mc",
    "Me",
    "Medefaidrin",
    "Meeteimayek",
    "Mendekikakui",
    "Meroiticcursive",
    "Meroitichieroglyphs",
    "Miao",
    "Mn",
    "Modi",
    "Modifierletter",
    "Modifiersymbol",
    "Mongolian",
    "Mro",
    "Multani",
    "Myanmar",
    "N",
    "Nabataean",
    "Nagmundari",
    "Nandinagari",
    "Nd",
    "Newa",
    "Newtailue",
    "Nko",
    "Nl",
    "No",
    "Nonspacingmark",
    "Number",
    "Nushu",
    "Nyiakengpuachuehmong",
    "Ogham",
    "Olchiki",
    "Oldhungarian",
    "Olditalic",
    "Oldnortharabian",
    "Oldpermic",
    "Oldpersian",
    "Oldsogdian",
    "Oldsoutharabian",
    "Oldturkic",
    "Olduyghur",
    "Olonal",
    "Openpunctuation",
    "Oriya",
    "Osage",
    "Osmanya",
    "Other",
    "Otherletter",
    "Othernumber",
    "Otherpunctuation",
    "Othersymbol",
    "P",
    "Pahawhhmong",
    "Palmyrene",
    "Paragraphseparator",
    "Paucinhau",
    "Pc",
    "Pd",
    "Pe",
    "Pf",
    "Phagspa",
    "Phoenician",
    "Pi",
    "Po",
    "Privateuse",
    "Ps",
    "Psalterpahlavi",
    "Punct",
    "Punctuation",
    "Rejang",
    "Runic",
    "S",
    "Samaritan",
    "Saurashtra",
    "Sc",
    "Separator",
    "Sharada",
    "Shavian",
    "Siddham",
    "Sidetic",
    "Signwriting",
    "Sinhala",
    "Sk",
    "Sm",
    "So",
    "Sogdian",
    "Sorasompeng",
    "Soyombo",
    "Spaceseparator",
    "Spacingmark",
    "Sundanese",
    "Sunuwar",
    "Surrogate",
    "Sylotinagri",
    "Symbol",
    "Syriac",
    "Tagalog",
    "Tagbanwa",
    "Taile",
    "Taitham",
    "Taiviet",
    "Taiyo",
    "Takri",
    "Tamil",
    "Tangsa",
    "Tangut",
    "Telugu",
    "Thaana",
    "Thai",
    "Tibetan",
    "Tifinagh",
    "Tirhuta",
    "Titlecaseletter",
    "Todhri",
    "Tolongsiki",
    "Toto",
    "Tulutigalari",
    "Ugaritic",
    "Unassigned",
    "Uppercaseletter",
    "Vai",
    "Vithkuqi",
    "Wancho",
    "Warangciti",
    "Yezidi",
    "Yi",
    "Z",
    "Zanabazarsquare",
    "Zl",
    "Zp",
    "Zs",
];

/// The POSIX classes Go takes in a class, `[:alpha:]`, and their negations.
const POSIX_NAMES: &[&str] = &[
    "alnum", "alpha", "ascii", "blank", "cntrl", "digit", "graph", "lower", "print", "punct",
    "space", "upper", "word", "xdigit",
];

/// How high Go lets a parse tree be.
const MAX_HEIGHT: usize = 1000;

/// What Go's parser keeps of a regexp, for its errors.
#[derive(Debug, Clone)]
enum Node {
    Leaf,
    /// `{min,max}`: max -1 for none.
    Repeat {
        min: i64,
        max: i64,
        sub: Box<Node>,
    },
    /// A capture, a `*`, `+` or `?`: one level of the tree over its sub.
    Wrap(Box<Node>),
    /// A concatenation or an alternation of several.
    Many(Vec<Node>),
}

impl Node {
    fn height(&self) -> usize {
        match self {
            Node::Leaf => 1,
            Node::Repeat { sub, .. } | Node::Wrap(sub) => sub.height().saturating_add(1),
            Node::Many(v) => v
                .iter()
                .map(Node::height)
                .max()
                .unwrap_or(0)
                .saturating_add(1),
        }
    }
}

/// Go's `repeatIsValid`: the nested repetitions' counts multiplied stay within `n`.
fn repeat_is_valid(re: &Node, mut n: i64) -> bool {
    if let Node::Repeat { min, max, .. } = re {
        let mut m = *max;
        if m == 0 {
            return true;
        }
        if m < 0 {
            m = *min;
        }
        if m > n {
            return false;
        }
        if m > 0 {
            n /= m;
        }
    }
    match re {
        Node::Leaf => true,
        Node::Repeat { sub, .. } | Node::Wrap(sub) => repeat_is_valid(sub, n),
        Node::Many(v) => v.iter().all(|s| repeat_is_valid(s, n)),
    }
}

/// A group being parsed: its alternatives so far, and the items of the one being read.
struct Frame {
    capture: bool,
    alts: Vec<Node>,
    items: Vec<Node>,
}

fn concat(items: Vec<Node>) -> Node {
    match items.len() {
        0 => Node::Leaf,
        1 => items.into_iter().next().unwrap_or(Node::Leaf),
        _ => Node::Many(items),
    }
}

fn err(code: &str, expr: &str) -> String {
    format!("error parsing regexp: {code}: `{expr}`")
}

/// The text of `s` that was taken to leave `rest`.
fn taken<'a>(s: &'a str, rest: &str) -> &'a str {
    s.get(..s.len().saturating_sub(rest.len())).unwrap_or(s)
}

/// Whether `re` compiles with Go's `regexp.Compile`, or Go's error.
pub(crate) fn check(re: &str) -> Result<(), String> {
    let mut stack = vec![Frame {
        capture: false,
        alts: Vec::new(),
        items: Vec::new(),
    }];
    let mut t = re;
    let mut last_repeat: &str = "";
    while let Some(c) = t.chars().next() {
        let mut repeat: &str = "";
        match c {
            '(' => {
                if t.as_bytes().get(1) == Some(&b'?') {
                    let (rest, group) = perl_flags(t)?;
                    if let Some(capture) = group {
                        stack.push(Frame {
                            capture,
                            alts: Vec::new(),
                            items: Vec::new(),
                        });
                    }
                    t = rest;
                } else {
                    stack.push(Frame {
                        capture: true,
                        alts: Vec::new(),
                        items: Vec::new(),
                    });
                    t = &t[1..];
                }
            }
            '|' => {
                if let Some(f) = stack.last_mut() {
                    let items = std::mem::take(&mut f.items);
                    f.alts.push(concat(items));
                }
                t = &t[1..];
            }
            ')' => {
                if stack.len() < 2 {
                    return Err(err("unexpected )", re));
                }
                let Some(f) = stack.pop() else {
                    return Err(err("unexpected )", re));
                };
                let mut alts = f.alts;
                alts.push(concat(f.items));
                let node = concat(alts);
                let node = if f.capture {
                    Node::Wrap(Box::new(node))
                } else {
                    node
                };
                if let Some(p) = stack.last_mut() {
                    p.items.push(node);
                }
                t = &t[1..];
            }
            '[' => {
                t = class(t)?;
                push(&mut stack, Node::Leaf);
            }
            '*' | '+' | '?' => {
                let before = t;
                let after = repeat_op(&mut stack, None, before, &t[1..], last_repeat)?;
                repeat = before;
                t = after;
            }
            '{' => {
                let before = t;
                match parse_repeat(t) {
                    None => {
                        push(&mut stack, Node::Leaf);
                        t = &t[1..];
                    }
                    Some((min, max, after)) => {
                        if !(0..=1000).contains(&min) || max > 1000 || max >= 0 && min > max {
                            return Err(err("invalid repeat count", taken(before, after)));
                        }
                        let after =
                            repeat_op(&mut stack, Some((min, max)), before, after, last_repeat)?;
                        repeat = before;
                        t = after;
                    }
                }
            }
            '\\' => {
                t = escape_atom(&mut stack, t)?;
            }
            _ => {
                push(&mut stack, Node::Leaf);
                t = &t[c.len_utf8()..];
            }
        }
        last_repeat = repeat;
    }
    if stack.len() != 1 {
        return Err(err("missing closing )", re));
    }
    let f = stack.pop().map_or(Node::Leaf, |f| {
        let mut alts = f.alts;
        alts.push(concat(f.items));
        concat(alts)
    });
    if f.height() > MAX_HEIGHT {
        return Err(err("expression nests too deeply", re));
    }
    Ok(())
}

fn push(stack: &mut [Frame], n: Node) {
    if let Some(f) = stack.last_mut() {
        f.items.push(n);
    }
}

/// Go's `repeat`: the item on top repeated, `count` for `{min,max}` and `None` for `*`, `+`
/// and `?`. Returns what follows.
fn repeat_op<'a>(
    stack: &mut [Frame],
    count: Option<(i64, i64)>,
    before: &'a str,
    mut after: &'a str,
    last_repeat: &'a str,
) -> Result<&'a str, String> {
    if let Some(rest) = after.strip_prefix('?') {
        after = rest;
    }
    if !last_repeat.is_empty() {
        return Err(err(
            "invalid nested repetition operator",
            taken(last_repeat, after),
        ));
    }
    let Some(top) = stack.last_mut().and_then(|f| f.items.pop()) else {
        return Err(err(
            "missing argument to repetition operator",
            taken(before, after),
        ));
    };
    let node = match count {
        None => Node::Wrap(Box::new(top)),
        Some((min, max)) => Node::Repeat {
            min,
            max,
            sub: Box::new(top),
        },
    };
    if let Some((min, max)) = count
        && (min >= 2 || max >= 2)
        && !repeat_is_valid(&node, 1000)
    {
        return Err(err("invalid repeat count", taken(before, after)));
    }
    push(stack, node);
    Ok(after)
}

/// Go's `parseInt`: decimal digits, no leading zero; -1 for a number too big.
fn parse_int(s: &str) -> Option<(i64, &str)> {
    let b = s.as_bytes();
    if !b.first().is_some_and(u8::is_ascii_digit) {
        return None;
    }
    if b.len() >= 2 && b.first() == Some(&b'0') && b.get(1).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let end = b
        .iter()
        .position(|c| !c.is_ascii_digit())
        .unwrap_or(b.len());
    let mut n: i64 = 0;
    for &d in b.get(..end).unwrap_or_default() {
        if n >= 100_000_000 {
            n = -1;
            break;
        }
        n = n * 10 + i64::from(d - b'0');
    }
    Some((n, s.get(end..).unwrap_or_default()))
}

/// Go's `parseRepeat`: `{min}`, `{min,}` (max -1) or `{min,max}`; min -1 when too big.
fn parse_repeat(s: &str) -> Option<(i64, i64, &str)> {
    let s = s.strip_prefix('{')?;
    let (mut min, s) = parse_int(s)?;
    let max;
    let s = if let Some(s) = s.strip_prefix(',') {
        if s.is_empty() {
            return None;
        }
        if let Some(rest) = s.strip_prefix('}') {
            max = -1;
            return Some((min, max, rest));
        }
        let (m, s) = parse_int(s)?;
        max = m;
        if max < 0 {
            min = -1;
        }
        s
    } else {
        if s.is_empty() {
            return None;
        }
        max = min;
        s
    };
    let rest = s.strip_prefix('}')?;
    Some((min, max, rest))
}

/// Go's `parsePerlFlags`, at `(?`: the rest, and the group it opens, if any (true for a
/// capture).
fn perl_flags(s: &str) -> Result<(&str, Option<bool>), String> {
    let b = s.as_bytes();
    let starts_with_p = b.len() > 4 && b.get(2) == Some(&b'P') && b.get(3) == Some(&b'<');
    let starts_with_name = b.len() > 3 && b.get(2) == Some(&b'<');
    if starts_with_p || starts_with_name {
        let start = if starts_with_name { 3 } else { 4 };
        let Some(end) = s.find('>') else {
            return Err(err("invalid named capture", s));
        };
        let capture = &s[..=end];
        let name = s.get(start..end).unwrap_or_default();
        if name.is_empty() || !name.bytes().all(|c| c == b'_' || c.is_ascii_alphanumeric()) {
            return Err(err("invalid named capture", capture));
        }
        return Ok((&s[end + 1..], Some(true)));
    }
    let mut t = &s[2..];
    let mut negated = false;
    let mut saw_flag = false;
    while let Some(c) = t.chars().next() {
        t = &t[c.len_utf8()..];
        match c {
            'i' | 'm' | 's' | 'U' => saw_flag = true,
            '-' => {
                if negated {
                    break;
                }
                negated = true;
                saw_flag = false;
            }
            ':' | ')' => {
                if negated && !saw_flag {
                    break;
                }
                return Ok((t, (c == ':').then_some(false)));
            }
            _ => break,
        }
    }
    Err(err("invalid or unsupported Perl syntax", taken(s, t)))
}

/// An escape outside a class: an assertion, `\Q...\E`, a class, or a character.
fn escape_atom<'a>(stack: &mut [Frame], t: &'a str) -> Result<&'a str, String> {
    if let Some(&c) = t.as_bytes().get(1) {
        match c {
            b'A' | b'b' | b'B' | b'z' => {
                push(stack, Node::Leaf);
                return Ok(&t[2..]);
            }
            b'C' => return Err(err("invalid escape sequence", &t[..2])),
            b'Q' => {
                let body = &t[2..];
                let (lit, rest) = body.split_once(r"\E").unwrap_or((body, ""));
                for _ in lit.chars() {
                    push(stack, Node::Leaf);
                }
                return Ok(rest);
            }
            _ => {}
        }
    }
    if let Some(rest) = unicode_class(t)? {
        push(stack, Node::Leaf);
        return Ok(rest);
    }
    if let Some(rest) = perl_class(t) {
        push(stack, Node::Leaf);
        return Ok(rest);
    }
    let rest = escape(t)?;
    push(stack, Node::Leaf);
    Ok(rest)
}

/// `\d`, `\s`, `\w` and their negations.
fn perl_class(t: &str) -> Option<&str> {
    let b = t.as_bytes();
    (b.first() == Some(&b'\\') && matches!(b.get(1), Some(b'd' | b'D' | b's' | b'S' | b'w' | b'W')))
        .then(|| &t[2..])
}

/// Go's `canonicalName`.
fn canonical_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut first = true;
    for c in name.bytes() {
        match c {
            b'_' | b'-' | b' ' => continue,
            c if first => {
                first = false;
                out.push(char::from(c.to_ascii_uppercase()));
            }
            c => out.push(char::from(c.to_ascii_lowercase())),
        }
    }
    out
}

/// Go's `parseUnicodeClass`: `\pL`, `\p{Name}`, `\P...`, committed once `\p` or `\P` is seen.
fn unicode_class(s: &str) -> Result<Option<&str>, String> {
    let b = s.as_bytes();
    if b.first() != Some(&b'\\') || !matches!(b.get(1), Some(b'p' | b'P')) {
        return Ok(None);
    }
    let t = &s[2..];
    let c = t.chars().next();
    let (seq, name, rest) = if c == Some('{') {
        let Some(end) = s.find('}') else {
            return Err(err("invalid character class range", s));
        };
        (&s[..=end], s.get(3..end).unwrap_or_default(), &s[end + 1..])
    } else {
        let n = c.map_or(0, char::len_utf8);
        let seq = &s[..2 + n];
        (seq, &seq[2..], &t[n..])
    };
    let name = name.strip_prefix('^').unwrap_or(name);
    if UNICODE_NAMES.contains(&canonical_name(name).as_str()) {
        Ok(Some(rest))
    } else {
        Err(err("invalid character class range", seq))
    }
}

/// Go's `parseEscape`: an escaped character; returns the rune and what follows.
fn escape_char(s: &str) -> Result<(u32, &str), String> {
    let t = &s[1..];
    let Some(c) = t.chars().next() else {
        return Err(err("trailing backslash at end of expression", ""));
    };
    let mut t = &t[c.len_utf8()..];
    let bad = |t: &str| err("invalid escape sequence", taken(s, t));
    match c {
        '1'..='7' | '0' => {
            if c != '0'
                && !t
                    .as_bytes()
                    .first()
                    .is_some_and(|d| (b'0'..=b'7').contains(d))
            {
                return Err(bad(t));
            }
            let mut r = c as u32 - '0' as u32;
            for _ in 1..3 {
                match t.as_bytes().first() {
                    Some(&d) if (b'0'..=b'7').contains(&d) => {
                        r = r * 8 + u32::from(d - b'0');
                        t = &t[1..];
                    }
                    _ => break,
                }
            }
            Ok((r, t))
        }
        'x' => {
            let Some(c) = t.chars().next() else {
                return Err(bad(t));
            };
            t = &t[c.len_utf8()..];
            if c == '{' {
                let mut n = 0;
                let mut r: u32 = 0;
                loop {
                    let Some(c) = t.chars().next() else {
                        return Err(bad(t));
                    };
                    t = &t[c.len_utf8()..];
                    if c == '}' {
                        break;
                    }
                    let Some(v) = c.to_digit(16) else {
                        return Err(bad(t));
                    };
                    r = r.saturating_mul(16).saturating_add(v);
                    if r > 0x10_FFFF {
                        return Err(bad(t));
                    }
                    n += 1;
                }
                if n == 0 {
                    return Err(bad(t));
                }
                return Ok((r, t));
            }
            let x = c.to_digit(16);
            let Some(c2) = t.chars().next() else {
                // Go reads the end of the text as an invalid rune, not a hex digit.
                return Err(bad(t));
            };
            t = &t[c2.len_utf8()..];
            match (x, c2.to_digit(16)) {
                (Some(x), Some(y)) => Ok((x * 16 + y, t)),
                _ => Err(bad(t)),
            }
        }
        'a' => Ok((7, t)),
        'f' => Ok((12, t)),
        'n' => Ok((10, t)),
        'r' => Ok((13, t)),
        't' => Ok((9, t)),
        'v' => Ok((11, t)),
        c if c.is_ascii() && !c.is_ascii_alphanumeric() => Ok((c as u32, t)),
        _ => Err(bad(t)),
    }
}

fn escape(s: &str) -> Result<&str, String> {
    escape_char(s).map(|(_, t)| t)
}

/// Go's `parseClass`, at `[`: what follows the class.
fn class(s: &str) -> Result<&str, String> {
    let mut t = &s[1..];
    if let Some(rest) = t.strip_prefix('^') {
        t = rest;
    }
    let mut first = true;
    while t.is_empty() || !t.starts_with(']') || first {
        first = false;
        if t.len() > 2 && t.starts_with("[:") {
            if let Some(i) = t[2..].find(":]") {
                let name = &t[..i + 4];
                let inner = &name[2..name.len() - 2];
                let inner = inner.strip_prefix('^').unwrap_or(inner);
                if !POSIX_NAMES.contains(&inner) {
                    return Err(err("invalid character class range", name));
                }
                t = &t[i + 4..];
                continue;
            }
        }
        if let Some(rest) = unicode_class(t)? {
            t = rest;
            continue;
        }
        if let Some(rest) = perl_class(t) {
            t = rest;
            continue;
        }
        let rng = t;
        let (lo, rest) = class_char(t, s)?;
        t = rest;
        if t.len() >= 2 && t.starts_with('-') && !t[1..].starts_with(']') {
            let (hi, rest) = class_char(&t[1..], s)?;
            t = rest;
            if hi < lo {
                return Err(err("invalid character class range", taken(rng, t)));
            }
        }
    }
    Ok(&t[1..])
}

/// Go's `parseClassChar`.
fn class_char<'a>(s: &'a str, whole: &str) -> Result<(u32, &'a str), String> {
    let Some(c) = s.chars().next() else {
        return Err(err("missing closing ]", whole));
    };
    if c == '\\' {
        return escape_char(s);
    }
    Ok((c as u32, &s[c.len_utf8()..]))
}

#[cfg(test)]
mod tests {
    use super::check;

    #[test]
    fn errors_as_go() {
        for (re, want) in [
            ("(", "missing closing ): `(`"),
            ("a)", "unexpected ): `a)`"),
            ("[a", "missing closing ]: `[a`"),
            ("*", "missing argument to repetition operator: `*`"),
            ("a**", "invalid nested repetition operator: `**`"),
            ("a*?+", "invalid nested repetition operator: `*?+`"),
            ("a{1001}", "invalid repeat count: `{1001}`"),
            ("(a{10}){101}", "invalid repeat count: `{101}`"),
            (r"\q", "invalid escape sequence: `\\q`"),
            (r"\8", "invalid escape sequence: `\\8`"),
            ("^/(?!private)", "invalid or unsupported Perl syntax: `(?!`"),
            ("(?<=a)", "invalid named capture: `(?<=a)`"),
            ("(?P<>a)", "invalid named capture: `(?P<>`"),
            ("[z-a]", "invalid character class range: `z-a`"),
            ("[[:foo:]]", "invalid character class range: `[:foo:]`"),
            (r"\p{Foo}", "invalid character class range: `\\p{Foo}`"),
            ("\\", "trailing backslash at end of expression: ``"),
            ("x{2}{3}?", "invalid nested repetition operator: `{2}{3}?`"),
            (r"\xA", "invalid escape sequence: `\\xA`"),
            (r"\x", "invalid escape sequence: `\\x`"),
            ("(?i", "invalid or unsupported Perl syntax: `(?i`"),
            ("(?-)", "invalid or unsupported Perl syntax: `(?-)`"),
            (r"[a-\d]", "invalid escape sequence: `\\d`"),
            ("(?P<n", "invalid named capture: `(?P<n`"),
            (r"\p", "invalid character class range: `\\p`"),
            ("a|*", "missing argument to repetition operator: `*`"),
            ("(?i)*", "missing argument to repetition operator: `*`"),
        ] {
            assert_eq!(
                check(re),
                Err(format!("error parsing regexp: {want}")),
                "{re}"
            );
        }
        for re in [
            r"^/users/(?<id>\d+)$",
            r"(?i)\.php$",
            "a{,5}",
            r"\pZ",
            "(|)*",
            "^*",
            r"\p{Greek}\pL\P{^Han}[\p{L}[:^alpha:]]",
            "[]a]",
            r"\Qa.b",
            "(?i)(?-s:a)",
            r"[\d-z]",
            r"\x{10FFFF}\x41\101\0",
        ] {
            assert_eq!(check(re), Ok(()), "{re}");
        }
    }
}
