//! Directives into the JSON tree, as fit's `adapter/fitconf/convert.go`: each directive is a field
//! of the block it is in, its value written as the field's type has it. A scalar takes one
//! argument, a list of scalars its arguments, an object a block, a map a block of keys, a list of
//! objects a block for each element, and a module its name and a block of its params.
//!
//! The types are fit's own, from its field inventory (`tests/compat/fit-fields.txt`, generated
//! from fit's JSON Schema at the commit the compatibility gate pins), so that every field fit has,
//! whether akit reads it yet or not, converts as fit converts it; the decoder then reads it or
//! refuses it by name. A directive no field takes is converted as it is written, for the decoder
//! to name as an unknown field.

use std::collections::HashMap;
use std::sync::OnceLock;

use super::{Directive, MAX_DEPTH, Tok, err};
use crate::error::join_field;
use crate::json::{Kind, Member, Node, ParseError};

/// fit's field inventory: path, section, type and flags, tab-separated, one field a line.
const INVENTORY: &str = include_str!("../../../../tests/compat/fit-fields.txt");

/// The fields akit has that fit does not, with their types as the inventory would write them:
/// akit's extensions, each with its decision note.
const EXTENSIONS: &[(&str, &str)] = &[
    // I51b, decision 010: the eBPF sockmap relay of a listener's tcp services.
    ("listeners[].sockmap", "boolean"),
];

/// Each field's type, by its inventory path (`a.b` a field, `a[]` a list's elements, `a.*` a map's
/// values, `a<m>` the object whose module is `m`): fit's, and akit's [`EXTENSIONS`].
fn types() -> &'static HashMap<&'static str, &'static str> {
    static TYPES: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TYPES.get_or_init(|| {
        INVENTORY
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| {
                let mut cols = l.split('\t');
                let path = cols.next()?;
                let _section = cols.next()?;
                Some((path, cols.next()?))
            })
            .chain(EXTENSIONS.iter().copied())
            .collect()
    })
}

/// A field's type, as the inventory writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ty<'t> {
    Str,
    Int,
    Num,
    Bool,
    Obj,
    List(&'t str),
    Map(&'t str),
    /// Any value: converted as written.
    Any,
}

fn ty(t: &str) -> Ty<'_> {
    match t {
        "string" | "duration" => Ty::Str,
        // A number of bytes or a share of memory ("50%"): an integer's kind in fit's types.
        "integer" | "bytes|percent" => Ty::Int,
        "number" => Ty::Num,
        "boolean" => Ty::Bool,
        "object" => Ty::Obj,
        _ if t.starts_with("enum(") => Ty::Str,
        _ => {
            if let Some(e) = t.strip_prefix("array<").and_then(|r| r.strip_suffix('>')) {
                Ty::List(e)
            } else if let Some(e) = t.strip_prefix("map<").and_then(|r| r.strip_suffix('>')) {
                Ty::Map(e)
            } else {
                Ty::Any
            }
        }
    }
}

/// The type fit's field inventory gives the field at `path`, as it writes it (`string`,
/// `array<object>`, ...), for the `.conf` writer (I73).
pub(crate) fn inventory_type(path: &str) -> Option<&'static str> {
    types().get(path).copied()
}

fn type_at(path: &str) -> Option<Ty<'static>> {
    types().get(path).map(|t| ty(t))
}

/// Whether the object at `path` names a module, whose params depend on it.
pub(crate) fn is_module(path: &str) -> bool {
    types()
        .get(format!("{path}.module").as_str())
        .is_some_and(|t| t.starts_with("enum("))
}

/// The names the top-level lists of objects take in a `.conf` file, one directive an element.
fn plural(name: &str) -> Option<&'static str> {
    Some(match name {
        "listener" => "listeners",
        "route" => "routes",
        "upstream" => "upstreams",
        "service" => "services",
        "provider" => "providers",
        _ => return None,
    })
}

/// The fields an element of a top-level list takes as arguments after its name, in order; the
/// last, a list, takes every argument left: `listener web :8080 http/1.1 h2c`.
fn shorthands(list: &str) -> Option<&'static [&'static str]> {
    match list {
        "listeners" => Some(&["address", "protocols"]),
        "upstreams" => Some(&["addresses"]),
        _ => None,
    }
}

fn node(off: usize, kind: Kind) -> Node {
    Node { off, kind }
}

fn string(t: &Tok) -> Node {
    node(t.off, Kind::Str(t.text.clone()))
}

fn has(members: &[Member], key: &str) -> bool {
    members.iter().any(|m| m.key == key)
}

/// The whole config.
pub(super) fn root(ds: Vec<Directive>) -> Result<Node, ParseError> {
    object("", ds, true, 0)
}

/// The directives of a block as the members of the object at inventory path `path`; `root` says
/// it is the whole config.
fn object(path: &str, ds: Vec<Directive>, root: bool, off: usize) -> Result<Node, ParseError> {
    let mut members: Vec<Member> = Vec::new();
    for d in ds {
        let mut name = d.name.text.clone();
        if root {
            if let Some(p) = plural(&name) {
                name = p.to_owned();
            }
        }
        let fpath = join_field(path, &name);
        match type_at(&fpath) {
            None => {
                if has(&members, &name) {
                    return Err(err(d.name.off, format!("{} is given twice", d.name.text)));
                }
                let key_off = d.name.off;
                let v = generic(&d, 0)?;
                members.push(Member {
                    key: name,
                    key_off,
                    value: v,
                });
            }
            Some(Ty::List(elem)) => {
                let i = match members.iter().position(|m| m.key == name) {
                    Some(i) => i,
                    None => {
                        members.push(Member {
                            key: name,
                            key_off: d.name.off,
                            value: node(d.name.off, Kind::Array(Vec::new())),
                        });
                        members.len().saturating_sub(1)
                    }
                };
                let Some(Member {
                    value:
                        Node {
                            kind: Kind::Array(arr),
                            ..
                        },
                    ..
                }) = members.get_mut(i)
                else {
                    return Err(err(d.name.off, format!("{} is given twice", d.name.text)));
                };
                append_to(arr, &fpath, ty(elem), d)?;
            }
            Some(t) => {
                if has(&members, &name) {
                    return Err(err(
                        d.name.off,
                        format!("{} is given twice; it takes one value", d.name.text),
                    ));
                }
                let key_off = d.name.off;
                let v = value(t, &fpath, &name, d, false)?;
                members.push(Member {
                    key: name,
                    key_off,
                    value: v,
                });
            }
        }
    }
    Ok(node(off, Kind::Object(members)))
}

/// Adds what directive `d` gives the list at `path`, whose elements are of type `elem`, to `arr`.
fn append_to(
    arr: &mut Vec<Node>,
    path: &str,
    elem: Ty<'_>,
    d: Directive,
) -> Result<(), ParseError> {
    let epath = format!("{path}[]");
    if is_module(&epath) {
        arr.push(module(&epath, d)?);
        return Ok(());
    }
    match elem {
        Ty::Obj => {
            let named = types().contains_key(format!("{epath}.name").as_str());
            let short = shorthands(path);
            if d.args.len() > 1 && short.is_none() || d.args.len() == 1 && !named {
                let what = if named {
                    "one argument, its name,"
                } else {
                    "no arguments"
                };
                return Err(err(
                    d.name.off,
                    format!("{} takes {what} and a block", d.name.text),
                ));
            }
            let off = d.name.off;
            let mut args = d.args;
            let block = if args.len() > 1 {
                let rest = args.split_off(1);
                shorthand(&d.name, rest, short.unwrap_or_default(), d.block)?
            } else {
                d.block
            };
            let mut v = object(&epath, block, false, off)?;
            if let (Some(a), Kind::Object(members)) = (args.first(), &mut v.kind) {
                if has(members, "name") {
                    return Err(err(
                        a.off,
                        "the name is given twice, as the argument and in the block",
                    ));
                }
                members.insert(
                    0,
                    Member {
                        key: "name".into(),
                        key_off: a.off,
                        value: string(a),
                    },
                );
            }
            arr.push(v);
        }
        Ty::List(_) | Ty::Map(_) | Ty::Any => arr.push(generic(&d, 0)?),
        _ => {
            if d.has_block {
                return Err(err(
                    d.name.off,
                    format!("{} takes a list of values, not a block", d.name.text),
                ));
            }
            arr.extend(d.args.iter().map(|a| scalar(elem, a)));
        }
    }
    Ok(())
}

/// The block of a directive named `name` with the fields its arguments after its name give, by
/// the shorthand `fields`: each one argument, but the last, a list, which takes those left.
fn shorthand(
    name: &Tok,
    mut args: Vec<Tok>,
    fields: &[&str],
    block: Vec<Directive>,
) -> Result<Vec<Directive>, ParseError> {
    let mut out = Vec::new();
    for (i, f) in fields.iter().enumerate() {
        let Some(first) = args.first().map(|a| a.off) else {
            break;
        };
        if block.iter().any(|b| b.name.text == *f) {
            return Err(err(
                first,
                format!("{f} is given twice, as an argument and in the block"),
            ));
        }
        let n = if i.saturating_add(1) == fields.len() {
            args.len()
        } else {
            1
        };
        let rest = args.split_off(n);
        out.push(Directive {
            name: Tok::made(f, first),
            args,
            block: Vec::new(),
            has_block: false,
        });
        args = rest;
    }
    if let Some(a) = args.first() {
        return Err(err(
            a.off,
            format!(
                "{} takes its name, then {}, then a block",
                name.text,
                fields.join(" and ")
            ),
        ));
    }
    out.extend(block);
    Ok(out)
}

/// Directive `d` as a value of type `t`, which is not a list, of the field at `path` named
/// `name`; `in_map` says it is a map's value.
fn value(
    t: Ty<'_>,
    path: &str,
    name: &str,
    d: Directive,
    in_map: bool,
) -> Result<Node, ParseError> {
    match t {
        Ty::Any | Ty::List(_) => return generic(&d, 0),
        Ty::Obj if is_module(path) => return module(path, d),
        Ty::Obj => {
            let mut block = d.block;
            let is_tls = name == "tls"
                && types()
                    .get(format!("{path}.certificates").as_str())
                    .is_some_and(|t| *t == "array<object>");
            match d.args.as_slice() {
                // tls <cert file> <key file> is a certificate, before any the block gives.
                [cert, key] if is_tls => {
                    let c = Directive {
                        name: Tok::made("certificates", cert.off),
                        args: Vec::new(),
                        block: vec![
                            Directive {
                                name: Tok::made("cert_file", cert.off),
                                args: vec![cert.clone()],
                                block: Vec::new(),
                                has_block: false,
                            },
                            Directive {
                                name: Tok::made("key_file", key.off),
                                args: vec![key.clone()],
                                block: Vec::new(),
                                has_block: false,
                            },
                        ],
                        has_block: true,
                    };
                    block.insert(0, c);
                }
                [a, ..] if is_tls => {
                    return Err(err(
                        a.off,
                        "tls takes a certificate's file and its key's, as in tls cert.pem key.pem, or a block, or both",
                    ));
                }
                [a, ..] => {
                    return Err(err(
                        a.off,
                        format!("{} takes a block, not arguments", d.name.text),
                    ));
                }
                [] => {}
            }
            return object(path, block, false, d.name.off);
        }
        Ty::Map(elem) => {
            if let Some(a) = d.args.first() {
                return Err(err(
                    a.off,
                    format!(
                        "{} takes a block of keys and their values, not arguments",
                        d.name.text
                    ),
                ));
            }
            return map_block(ty(elem), &format!("{path}.*"), d.block, d.name.off);
        }
        Ty::Str | Ty::Int | Ty::Num | Ty::Bool => {}
    }
    if d.has_block {
        return Err(err(
            d.name.off,
            format!("{} takes a value, not a block", d.name.text),
        ));
    }
    match d.args.as_slice() {
        [] if t == Ty::Bool => Ok(node(d.name.off, Kind::Bool(true))),
        [] => Err(err(d.name.off, format!("{} takes a value", d.name.text))),
        [a] => {
            if !a.quoted
                && t == Ty::Int
                && !in_map
                && (name.ends_with("bytes") || name.ends_with("_size"))
            {
                if let Some(n) = parse_bytes(&a.text) {
                    return Ok(node(a.off, Kind::Number(n.to_string())));
                }
            }
            Ok(scalar(t, a))
        }
        [_, b, ..] => Err(err(
            b.off,
            format!(
                "{} takes one value; quote one that holds spaces",
                d.name.text
            ),
        )),
    }
}

/// A block of keys and their values, of type `t` (inventory path `path`), as an object.
fn map_block(t: Ty<'_>, path: &str, ds: Vec<Directive>, off: usize) -> Result<Node, ParseError> {
    let mut members: Vec<Member> = Vec::new();
    for d in ds {
        let k = d.name.text.clone();
        if has(&members, &k) {
            return Err(err(d.name.off, format!("{k:?} is a key twice")));
        }
        let key_off = d.name.off;
        let v = match t {
            Ty::List(e) if !matches!(ty(e), Ty::Obj | Ty::Any) => {
                if d.has_block {
                    return Err(err(
                        d.name.off,
                        format!("{k:?} takes a list of values, not a block"),
                    ));
                }
                let e = ty(e);
                node(
                    d.name.off,
                    Kind::Array(d.args.iter().map(|a| scalar(e, a)).collect()),
                )
            }
            _ => value(t, path, &k, d, true)?,
        };
        members.push(Member {
            key: k,
            key_off,
            value: v,
        });
    }
    Ok(node(off, Kind::Object(members)))
}

/// A module reference at `path`: the module's name as the argument, then its params in a block,
/// as the module's params have them.
fn module(path: &str, d: Directive) -> Result<Node, ParseError> {
    let [name] = d.args.as_slice() else {
        let names = types()
            .get(format!("{path}.module").as_str())
            .and_then(|t| t.strip_prefix("enum("))
            .and_then(|t| t.strip_suffix(')'))
            .unwrap_or_default()
            .replace('|', ", ");
        return Err(err(
            d.name.off,
            format!(
                "{} takes the name of a module, one of {names}, then its params in a block",
                d.name.text
            ),
        ));
    };
    let mut members = vec![Member {
        key: "module".into(),
        key_off: name.off,
        value: string(name),
    }];
    if d.has_block {
        let mpath = format!("{path}<{}>", name.text);
        let params = if types().contains_key(mpath.as_str()) {
            object(&format!("{mpath}.params"), d.block, false, d.name.off)?
        } else {
            // The decoder says there is no such module.
            generic(
                &Directive {
                    name: d.name.clone(),
                    args: Vec::new(),
                    block: d.block,
                    has_block: true,
                },
                0,
            )?
        };
        members.push(Member {
            key: "params".into(),
            key_off: d.name.off,
            value: params,
        });
    }
    Ok(node(d.name.off, Kind::Object(members)))
}

/// A directive of no known type: a block is an object of its directives, no arguments true, one
/// argument a scalar and several a list of them.
fn generic(d: &Directive, depth: usize) -> Result<Node, ParseError> {
    if depth > MAX_DEPTH {
        return Err(err(d.name.off, "nested too deep"));
    }
    if d.has_block {
        if let Some(a) = d.args.first() {
            return Err(err(
                a.off,
                format!("{} takes either arguments or a block", d.name.text),
            ));
        }
        let mut members: Vec<Member> = Vec::new();
        for e in &d.block {
            if has(&members, &e.name.text) {
                return Err(err(e.name.off, format!("{} is given twice", e.name.text)));
            }
            members.push(Member {
                key: e.name.text.clone(),
                key_off: e.name.off,
                value: generic(e, depth.saturating_add(1))?,
            });
        }
        return Ok(node(d.name.off, Kind::Object(members)));
    }
    Ok(match d.args.as_slice() {
        [] => node(d.name.off, Kind::Bool(true)),
        [a] => infer(a),
        args => node(d.name.off, Kind::Array(args.iter().map(infer).collect())),
    })
}

/// An argument as a value of scalar type `t`; one that is not what `t` takes is kept as a string,
/// for the decoder to report at its place.
fn scalar(t: Ty<'_>, a: &Tok) -> Node {
    if !a.quoted {
        match t {
            Ty::Bool if a.text == "true" || a.text == "false" => {
                return node(a.off, Kind::Bool(a.text == "true"));
            }
            Ty::Int | Ty::Num if is_number(&a.text) => {
                return node(a.off, Kind::Number(a.text.clone()));
            }
            _ => {}
        }
    }
    string(a)
}

/// An argument of no known type: a number, `true`, `false` or `null` as JSON has them, unless
/// quoted; anything else a string.
fn infer(a: &Tok) -> Node {
    if !a.quoted {
        match a.text.as_str() {
            "true" => return node(a.off, Kind::Bool(true)),
            "false" => return node(a.off, Kind::Bool(false)),
            "null" => return node(a.off, Kind::Null),
            s if is_number(s) => return node(a.off, Kind::Number(a.text.clone())),
            _ => {}
        }
    }
    string(a)
}

/// Whether `s` is a JSON number, at most 64 bytes long, that a float64 holds, as fit's `isNumber`
/// reads it into one: `1e400` is not.
pub(crate) fn is_number(s: &str) -> bool {
    if s.is_empty() || s.len() > 64 {
        return false;
    }
    let b = s.as_bytes();
    let mut i = usize::from(b.first() == Some(&b'-'));
    let digits = |i: &mut usize| {
        let start = *i;
        while b.get(*i).is_some_and(u8::is_ascii_digit) {
            *i = i.saturating_add(1);
        }
        *i > start
    };
    match b.get(i) {
        Some(b'0') => i = i.saturating_add(1),
        Some(b'1'..=b'9') => {
            digits(&mut i);
        }
        _ => return false,
    }
    if b.get(i) == Some(&b'.') {
        i = i.saturating_add(1);
        if !digits(&mut i) {
            return false;
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i = i.saturating_add(1);
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i = i.saturating_add(1);
        }
        if !digits(&mut i) {
            return false;
        }
    }
    i == b.len() && s.parse::<f64>().is_ok_and(f64::is_finite)
}

/// A whole number of bytes in a unit, as `16MiB` or `2gb`: powers of 1024 and of 1000, the case of
/// the letters not minded. A number alone is not one.
fn parse_bytes(s: &str) -> Option<i64> {
    const UNITS: &[(&str, i64)] = &[
        ("kib", 1 << 10),
        ("mib", 1 << 20),
        ("gib", 1 << 30),
        ("tib", 1 << 40),
        ("kb", 1_000),
        ("mb", 1_000_000),
        ("gb", 1_000_000_000),
        ("tb", 1_000_000_000_000),
        ("b", 1),
    ];
    let lower = s.to_ascii_lowercase();
    for (unit, n) in UNITS {
        let Some(num) = lower.strip_suffix(unit) else {
            continue;
        };
        if num.is_empty() {
            continue;
        }
        let v: i64 = num.parse().ok()?;
        if v < 0 {
            return None;
        }
        return v.checked_mul(*n);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        for ok in ["0", "-1", "1.5", "2e3", "-0.5E-2", "10", "1e308", "1e-400"] {
            assert!(is_number(ok), "{ok}");
        }
        for bad in [
            "", "-", "01", "1.", ".5", "1e", "+1", "0x10", "1/2", "NaN", "1e400",
        ] {
            assert!(!is_number(bad), "{bad}");
        }
    }

    #[test]
    fn bytes() {
        assert_eq!(parse_bytes("16MiB"), Some(16 << 20));
        assert_eq!(parse_bytes("2gb"), Some(2_000_000_000));
        assert_eq!(parse_bytes("7B"), Some(7));
        assert_eq!(parse_bytes("+1KiB"), Some(1024));
        assert_eq!(parse_bytes("1024"), None);
        assert_eq!(parse_bytes("MiB"), None);
        assert_eq!(parse_bytes("-1KiB"), None);
        assert_eq!(parse_bytes("9999999999TiB"), None);
        assert_eq!(parse_bytes("1.5MiB"), None);
    }

    #[test]
    fn inventory_types() {
        assert_eq!(type_at("listeners"), Some(Ty::List("object")));
        assert_eq!(type_at("routes[].priority"), Some(Ty::Int));
        assert_eq!(
            type_at("routes[].handler<respond>.params.headers"),
            Some(Ty::Map("string"))
        );
        assert!(is_module("routes[].handler"));
        assert!(is_module("routes[].middleware[]"));
        assert!(!is_module("routes[].match"));
    }
}
