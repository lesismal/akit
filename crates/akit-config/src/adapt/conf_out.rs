//! A JSON value written as a `.conf` file, byte for byte as fit's `adapter/fitconf.FromJSON`
//! writes it: field for field, each as its type in fit's config has it (the field inventory the
//! `.conf` reader types fields by), with the shorthands the reader takes (a listener's address
//! and protocols, an upstream's addresses and a tls's one certificate as arguments, numbers of
//! bytes in units), routes one by one rather than gathered into sites; then written again with
//! each block that fits in 80 columns on one line, as fit's `compact`.

use crate::conf::convert::{inventory_type, is_module, is_number};
use crate::conf::{Directive, Tok, directives};
use crate::error::join_field;
use crate::value::Value;

/// The widest a block written on one line may be, its indentation counted as four columns a tab.
const COMPACT_WIDTH: usize = 80;

/// The JSON document `src` written as a `.conf` file, as fit's `fitconf.FromJSON`.
///
/// # Errors
///
/// What makes `src` not JSON (`JSON: <why>`), not an object, or not something a `.conf` file
/// holds: a list of fewer than two values in a field of no known type, which would read back as
/// the value alone.
pub fn from_json(src: &str) -> Result<String, String> {
    let n = crate::json::parse(src).map_err(|e| format!("JSON: {}", e.msg))?;
    write(&Value::from_node(n))
}

/// `v`, a config, written as a `.conf` file.
pub(super) fn write(v: &Value) -> Result<String, String> {
    let Value::Object(obj) = v else {
        return Err("JSON: a config is an object".into());
    };
    let mut w = Writer { b: String::new() };
    w.object("", obj, 0, true)?;
    compact(&w.b)
}

/// A field's type, as fit's writer tells them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ty<'t> {
    /// No known type, or any value: written as it is.
    Generic,
    /// A module and its params.
    Module,
    /// An object of fields.
    Object,
    Map(&'t str),
    List(&'t str),
    /// A scalar: `string` says its kind is a string's, `int` an integer's.
    Scalar {
        string: bool,
        int: bool,
    },
}

/// The type of the field at inventory path `path`, which the inventory writes as `t`.
fn ty_of<'t>(path: &str, t: &'t str) -> Ty<'t> {
    match t {
        "object" if is_module(path) => Ty::Module,
        "object" => Ty::Object,
        "string" => Ty::Scalar {
            string: true,
            int: false,
        },
        "integer" | "duration" | "bytes|percent" => Ty::Scalar {
            string: false,
            int: true,
        },
        "number" | "boolean" => Ty::Scalar {
            string: false,
            int: false,
        },
        _ if t.starts_with("enum(") => Ty::Scalar {
            string: true,
            int: false,
        },
        _ => {
            if let Some(e) = t.strip_prefix("array<").and_then(|r| r.strip_suffix('>')) {
                Ty::List(e)
            } else if let Some(e) = t.strip_prefix("map<").and_then(|r| r.strip_suffix('>')) {
                Ty::Map(e)
            } else {
                Ty::Generic
            }
        }
    }
}

/// The names the top-level lists of objects take in a `.conf` file.
fn singular(list: &str) -> Option<&'static str> {
    Some(match list {
        "listeners" => "listener",
        "routes" => "route",
        "upstreams" => "upstream",
        "services" => "service",
        "providers" => "provider",
        _ => return None,
    })
}

/// The fields an element of a top-level list takes as arguments after its name.
fn shorthands(list: &str) -> &'static [&'static str] {
    match list {
        "listeners" => &["address", "protocols"],
        "upstreams" => &["addresses"],
        _ => &[],
    }
}

fn get<'v>(o: &'v [(String, Value)], k: &str) -> Option<&'v Value> {
    o.iter().find(|(key, _)| key == k).map(|(_, v)| v)
}

fn without(o: &[(String, Value)], k: &str) -> Vec<(String, Value)> {
    o.iter().filter(|(key, _)| key != k).cloned().collect()
}

fn describe(v: &Value) -> &'static str {
    match v {
        Value::Object(_) => "an object",
        Value::Array(_) => "a list",
        Value::String(_) => "a string",
        Value::Number(_) => "a number",
        Value::Bool(_) => "a boolean",
        Value::Null => "null",
    }
}

struct Writer {
    b: String,
}

impl Writer {
    fn line(&mut self, depth: usize, words: &[String]) {
        for _ in 0..depth {
            self.b.push('\t');
        }
        self.b.push_str(&words.join(" "));
        self.b.push('\n');
    }

    /// The members of `obj`, an object at inventory path `path`.
    fn object(
        &mut self,
        path: &str,
        obj: &[(String, Value)],
        depth: usize,
        root: bool,
    ) -> Result<(), String> {
        for (k, v) in obj {
            if *v == Value::Null {
                continue;
            }
            let fpath = join_field(path, k);
            match inventory_type(&fpath) {
                None => self.generic(k, v, depth)?,
                Some(t) => self.field(k, &fpath, ty_of(&fpath, t), true, v, depth, root)?,
            }
        }
        Ok(())
    }

    /// The member `k`, at inventory path `path`, of type `t`; `is_field` says it is a field of an
    /// object, not a map's value.
    #[allow(clippy::too_many_arguments)]
    fn field(
        &mut self,
        k: &str,
        path: &str,
        t: Ty<'_>,
        is_field: bool,
        v: &Value,
        depth: usize,
        root: bool,
    ) -> Result<(), String> {
        let depth1 = depth.saturating_add(1);
        match t {
            Ty::Generic => return self.generic(k, v, depth),
            Ty::Module => return self.module(k, path, v, depth),
            Ty::Object => {
                let Value::Object(o) = v else {
                    return self.generic(k, v, depth);
                };
                let is_tls = k == "tls"
                    && inventory_type(&format!("{path}.certificates")) == Some("array<object>");
                let (args, o) = if is_tls {
                    tls_shorthand(o)
                } else {
                    (Vec::new(), o.clone())
                };
                if !args.is_empty() && o.is_empty() {
                    let mut words = vec![word(k)];
                    words.extend(args);
                    self.line(depth, &words);
                    return Ok(());
                }
                let empty = o.is_empty();
                return self.block(k, args, depth, empty, |w| w.object(path, &o, depth1, false));
            }
            Ty::Map(elem) => {
                let Value::Object(o) = v else {
                    return self.generic(k, v, depth);
                };
                let epath = format!("{path}.*");
                let et = ty_of(&epath, elem);
                return self.block(k, Vec::new(), depth, o.is_empty(), |w| {
                    for (mk, mv) in o {
                        w.map_entry(mk, &epath, et, mv, depth1)?;
                    }
                    Ok(())
                });
            }
            Ty::List(elem) => {
                let Value::Array(a) = v else {
                    return self.generic(k, v, depth);
                };
                let epath = format!("{path}[]");
                if is_module(&epath) {
                    for e in a {
                        self.module(k, &epath, e, depth)?;
                    }
                    return Ok(());
                }
                match ty_of(&epath, elem) {
                    Ty::Object => return self.elements(k, path, &epath, a, depth, root),
                    Ty::Scalar { string, .. } => {
                        let words = scalars(a, !string).map_err(|e| format!("{k}: {e}"))?;
                        let mut line = vec![word(k)];
                        line.extend(words);
                        self.line(depth, &line);
                        return Ok(());
                    }
                    _ => {
                        return Err(format!(
                            "{k}: a list of {elem}, which a .conf file does not hold"
                        ));
                    }
                }
            }
            Ty::Scalar { string, int } => {
                if let Value::Number(n) = v {
                    if is_field && int && is_bytes_name(k) {
                        if let Ok(i) = n.parse::<i64>() {
                            self.line(depth, &[word(k), format_bytes(i)]);
                            return Ok(());
                        }
                    }
                }
                // A string where the field takes another kind is quoted, so that it reads back
                // a string, for the config's check to report.
                let s = scalar_word(v, !string).map_err(|e| format!("{k}: {e}"))?;
                self.line(depth, &[word(k), s]);
            }
        }
        Ok(())
    }

    /// The elements of the list of objects `k` (at `path`, its elements at `epath`): each a block,
    /// its name as the argument, and at the top the shorthand fields after it.
    fn elements(
        &mut self,
        k: &str,
        path: &str,
        epath: &str,
        a: &[Value],
        depth: usize,
        root: bool,
    ) -> Result<(), String> {
        let name = if root { singular(k).unwrap_or(k) } else { k };
        let named = inventory_type(&format!("{epath}.name")).is_some();
        for (i, e) in a.iter().enumerate() {
            let Value::Object(o) = e else {
                return Err(format!("{k}[{i}]: an object, not {}", describe(e)));
            };
            let mut args = Vec::new();
            let mut o = o.clone();
            if named {
                if let Some(Value::String(s)) = get(&o, "name") {
                    args.push(word(s));
                    o = without(&o, "name");
                }
            }
            if args.len() == 1 {
                let (more, rest) = shorthand_args(&o, shorthands(path));
                args.extend(more);
                o = rest;
            }
            if args.len() > 1 && o.is_empty() {
                let mut words = vec![name.to_owned()];
                words.extend(args);
                self.line(depth, &words);
                continue;
            }
            let empty = o.is_empty();
            let depth1 = depth.saturating_add(1);
            self.block(name, args, depth, empty, |w| {
                w.object(epath, &o, depth1, false)
            })?;
        }
        Ok(())
    }

    fn map_entry(
        &mut self,
        k: &str,
        epath: &str,
        t: Ty<'_>,
        v: &Value,
        depth: usize,
    ) -> Result<(), String> {
        if let (Value::Array(a), Ty::List(elem)) = (v, t) {
            if let Ty::Scalar { string, .. } = ty_of("", elem) {
                let words = scalars(a, !string).map_err(|e| format!("{k}: {e}"))?;
                let mut line = vec![word(k)];
                line.extend(words);
                self.line(depth, &line);
                return Ok(());
            }
        }
        self.field(k, epath, t, false, v, depth, false)
    }

    /// A module reference at `path`: `k`, the module's name, and its params.
    fn module(&mut self, k: &str, path: &str, v: &Value, depth: usize) -> Result<(), String> {
        let Value::Object(o) = v else {
            return Err(format!("{k}: a module is an object, not {}", describe(v)));
        };
        let Some(Value::String(name)) = get(o, "module") else {
            return Err(format!("{k}: a module needs its name, in module"));
        };
        if let Some((key, _)) = o.iter().find(|(key, _)| key != "module" && key != "params") {
            return Err(format!("{k}: a module has a module and params, not {key}"));
        }
        let params = match get(o, "params") {
            None | Some(Value::Null) => {
                self.line(depth, &[word(k), word(name)]);
                return Ok(());
            }
            Some(Value::Object(p)) => p,
            Some(p) => {
                return Err(format!(
                    "{k}: a module's params are an object, not {}",
                    describe(p)
                ));
            }
        };
        let mpath = format!("{path}<{name}>");
        let typed = inventory_type(&mpath).is_some();
        let depth1 = depth.saturating_add(1);
        self.block(k, vec![word(name)], depth, params.is_empty(), |w| {
            if typed {
                return w.object(&format!("{mpath}.params"), params, depth1, false);
            }
            for (pk, pv) in params {
                w.generic(pk, pv, depth1)?;
            }
            Ok(())
        })
    }

    /// `name` and `args`, then a block of what `body` writes, or `{}` when `empty`.
    fn block(
        &mut self,
        name: &str,
        args: Vec<String>,
        depth: usize,
        empty: bool,
        body: impl FnOnce(&mut Writer) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut words = vec![word(name)];
        words.extend(args);
        if empty {
            words.push("{}".into());
            self.line(depth, &words);
            return Ok(());
        }
        words.push("{".into());
        self.line(depth, &words);
        body(self)?;
        self.line(depth, &["}".to_owned()]);
        Ok(())
    }

    /// A member of no known type, as the reader reads one back: an object as a block, true with
    /// no arguments, and a list of two scalars or more as arguments.
    fn generic(&mut self, k: &str, v: &Value, depth: usize) -> Result<(), String> {
        match v {
            Value::Null => {
                self.line(depth, &[word(k), "null".into()]);
                return Ok(());
            }
            Value::Object(o) => {
                let depth1 = depth.saturating_add(1);
                return self.block(k, Vec::new(), depth, o.is_empty(), |w| {
                    for (mk, mv) in o {
                        w.generic(mk, mv, depth1)?;
                    }
                    Ok(())
                });
            }
            Value::Array(a) => {
                if a.len() < 2 {
                    return Err(format!(
                        "{k}: a list of fewer than two values reads back as one, in a field of no known type"
                    ));
                }
                let words = scalars(a, true).map_err(|e| format!("{k}: {e}"))?;
                let mut line = vec![word(k)];
                line.extend(words);
                self.line(depth, &line);
                return Ok(());
            }
            Value::Bool(true) => {
                self.line(depth, &[word(k)]);
                return Ok(());
            }
            _ => {}
        }
        let s = scalar_word(v, true).map_err(|e| format!("{k}: {e}"))?;
        self.line(depth, &[word(k), s]);
        Ok(())
    }
}

/// Whether a field named `k` is a number of bytes: its name ends in `bytes` or `_size`.
fn is_bytes_name(k: &str) -> bool {
    if k.ends_with(']') {
        return false;
    }
    let name = k.rsplit('.').next().unwrap_or(k);
    name.ends_with("bytes") || name.ends_with("_size")
}

/// `n` in the largest power of 1024 it is a whole number of, from KiB up, or as it is.
fn format_bytes(n: i64) -> String {
    for (unit, size) in [
        ("TiB", 1i64 << 40),
        ("GiB", 1 << 30),
        ("MiB", 1 << 20),
        ("KiB", 1 << 10),
    ] {
        if n > 0 && n.checked_rem(size) == Some(0) {
            return format!("{}{unit}", n.checked_div(size).unwrap_or(n));
        }
    }
    n.to_string()
}

/// A list's elements as arguments.
fn scalars(a: &[Value], typeless: bool) -> Result<Vec<String>, String> {
    a.iter().map(|e| scalar_word(e, typeless)).collect()
}

/// A scalar as an argument; `typeless` says the field's type is not known, or not a string's, so
/// that a string that reads as another kind is quoted.
fn scalar_word(v: &Value, typeless: bool) -> Result<String, String> {
    match v {
        Value::String(s) => {
            if typeless && (s == "true" || s == "false" || s == "null" || is_number(s)) {
                Ok(quote(s))
            } else {
                Ok(word(s))
            }
        }
        Value::Number(n) => Ok(n.clone()),
        Value::Bool(b) => Ok(if *b { "true" } else { "false" }.into()),
        _ => Err(format!("{} is not a value an argument holds", describe(v))),
    }
}

/// `s` as a word: as it is when it reads back so, else quoted.
fn word(s: &str) -> String {
    if s.is_empty()
        || s == "{"
        || s == "}"
        || s == "{}"
        || s.contains([' ', '\t', '\r', '\n', '"', '`'])
        || s.starts_with('#')
        || s.starts_with(';')
        || s.ends_with(';')
        || s.contains("{$")
    {
        return quote(s);
    }
    s.to_owned()
}

/// `s` in double quotes, its `{` escaped where `{$` would expand.
fn quote(s: &str) -> String {
    let mut b = String::with_capacity(s.len().saturating_add(2));
    b.push('"');
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => b.push_str("\\\\"),
            '"' => b.push_str("\\\""),
            '\n' => b.push_str("\\n"),
            '\t' => b.push_str("\\t"),
            '\r' => b.push_str("\\r"),
            '{' if chars.peek() == Some(&'$') => b.push_str("\\{"),
            c => b.push(c),
        }
    }
    b.push('"');
    b
}

/// The arguments an element of a top-level list takes for the shorthand `fields`, and the element
/// without them: each a string, the last a list of them, in order, as far as the element has them.
fn shorthand_args(o: &[(String, Value)], fields: &[&str]) -> (Vec<String>, Vec<(String, Value)>) {
    let mut args = Vec::new();
    let mut o = o.to_vec();
    for (i, name) in fields.iter().enumerate() {
        let Some(v) = get(&o, name) else {
            break;
        };
        let last = i.saturating_add(1) == fields.len();
        let words = match v {
            Value::String(s) if !last => vec![word(s)],
            Value::Array(a) if last && !a.is_empty() => {
                let mut ws = Vec::new();
                for e in a {
                    let Value::String(s) = e else {
                        return (args, o);
                    };
                    ws.push(word(s));
                }
                ws
            }
            _ => return (args, o),
        };
        args.extend(words);
        o = without(&o, name);
    }
    (args, o)
}

/// A tls's first certificate as the arguments of `tls <cert file> <key file>`, when it has nothing
/// else, and the tls without it.
fn tls_shorthand(o: &[(String, Value)]) -> (Vec<String>, Vec<(String, Value)>) {
    let none = || (Vec::new(), o.to_vec());
    let Some(Value::Array(certs)) = get(o, "certificates") else {
        return none();
    };
    let Some(Value::Object(first)) = certs.first() else {
        return none();
    };
    if first.len() != 2 {
        return none();
    }
    let (Some(Value::String(cert)), Some(Value::String(key))) =
        (get(first, "cert_file"), get(first, "key_file"))
    else {
        return none();
    };
    let mut rest = without(o, "certificates");
    if certs.len() > 1 {
        rest.insert(
            0,
            (
                "certificates".into(),
                Value::Array(certs.get(1..).unwrap_or_default().to_vec()),
            ),
        );
    }
    (vec![word(cert), word(key)], rest)
}

/// `src` written again with each block that fits within [`COMPACT_WIDTH`] on one line, its
/// directives parted with `;`. At the top, a blank line goes between directives but those of one
/// line of the same name.
fn compact(src: &str) -> Result<String, String> {
    let ds = directives(src).map_err(|e| e.msg)?;
    let mut b = String::new();
    write_compact(&mut b, &ds, 0);
    Ok(b)
}

fn write_compact(b: &mut String, ds: &[Directive], depth: usize) {
    let indent = "\t".repeat(depth);
    let mut prev_name = "";
    let mut prev_one_line = true;
    for (i, d) in ds.iter().enumerate() {
        let (one, ok) = inline(d);
        let ok = ok
            && (!d.has_block || depth.saturating_mul(4).saturating_add(one.len()) <= COMPACT_WIDTH);
        if depth == 0 && i > 0 && (!ok || !prev_one_line || d.name.text != prev_name) {
            b.push('\n');
        }
        prev_name = &d.name.text;
        prev_one_line = ok;
        if ok {
            b.push_str(&indent);
            b.push_str(&one);
            b.push('\n');
            continue;
        }
        b.push_str(&indent);
        b.push_str(&words(d).join(" "));
        b.push_str(" {\n");
        write_compact(b, &d.block, depth.saturating_add(1));
        b.push_str(&indent);
        b.push_str("}\n");
    }
}

/// A directive's name and arguments as written.
fn words(d: &Directive) -> Vec<String> {
    let mut ws = vec![token_word(&d.name)];
    ws.extend(d.args.iter().map(token_word));
    ws
}

/// `t` written so that it reads back as it was read: quoted if it was.
fn token_word(t: &Tok) -> String {
    if t.quoted {
        quote(&t.text)
    } else {
        t.text.clone()
    }
}

/// `d` on one line, or false when a block in it is wider than [`COMPACT_WIDTH`].
fn inline(d: &Directive) -> (String, bool) {
    let s = words(d).join(" ");
    if !d.has_block {
        return (s, true);
    }
    if d.block.is_empty() {
        return (s + " {}", true);
    }
    let mut parts = Vec::with_capacity(d.block.len());
    for e in &d.block {
        let (p, ok) = inline(e);
        if !ok || p.len() > COMPACT_WIDTH {
            return (String::new(), false);
        }
        parts.push(p);
    }
    (format!("{s} {{ {} }}", parts.join("; ")), true)
}
