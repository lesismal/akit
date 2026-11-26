//! JSON pointers (RFC 6901) into a config, as fit's Admin API takes them after `/config`, and the
//! edits made there (I27): a value set or added ([`put`], `-` appending to an array), removed
//! ([`remove`]), or merged with a JSON merge patch (RFC 7396, [`merge_patch`]), as fit's
//! `admin/jsonptr.go`.

use akit_config::value::Value;

/// A request the API refuses: its status and message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Problem {
    pub(crate) status: u16,
    pub(crate) msg: String,
}

impl Problem {
    pub(crate) fn new(status: u16, msg: String) -> Problem {
        Problem { status, msg }
    }
}

/// The reference tokens of pointer `p`, `""` or starting with `/`.
pub(crate) fn parse(p: &str) -> Result<Vec<String>, Problem> {
    if p.is_empty() {
        return Ok(Vec::new());
    }
    let Some(rest) = p.strip_prefix('/') else {
        return Err(Problem::new(
            400,
            format!("{p:?} is not a JSON pointer: it must start with '/'"),
        ));
    };
    rest.split('/')
        .map(|t| {
            let mut out = String::with_capacity(t.len());
            let mut chars = t.chars();
            while let Some(c) = chars.next() {
                if c != '~' {
                    out.push(c);
                    continue;
                }
                match chars.next() {
                    Some('0') => out.push('~'),
                    Some('1') => out.push('/'),
                    _ => {
                        return Err(Problem::new(
                            400,
                            format!("{p:?} is not a JSON pointer: '~' must be followed by 0 or 1"),
                        ));
                    }
                }
            }
            Ok(out)
        })
        .collect()
}

/// `tokens` written back as a JSON pointer, for messages.
pub(crate) fn string(tokens: &[String]) -> String {
    let mut b = String::new();
    for t in tokens {
        b.push('/');
        b.push_str(&t.replace('~', "~0").replace('/', "~1"));
    }
    b
}

/// The array index token `t` names in an array of `n`, as fit's `index`: decimal, without a
/// leading zero, below `n`; `upto`, the pointer to it, is for the message.
fn index(t: &str, n: usize, upto: &[String]) -> Result<usize, Problem> {
    let not_index = || {
        Problem::new(
            404,
            format!("{}: {t:?} is not an array index", string(upto)),
        )
    };
    if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit()) || (t.len() > 1 && t.starts_with('0'))
    {
        return Err(not_index());
    }
    let j: usize = t.parse().map_err(|_| not_index())?;
    if j >= n {
        return Err(Problem::new(
            404,
            format!(
                "{}: index {j} is past the end of an array of {n}",
                string(upto)
            ),
        ));
    }
    Ok(j)
}

/// 404: what `tokens` point into is a scalar.
fn scalar(tokens: &[String], i: usize) -> Problem {
    Problem::new(
        404,
        format!(
            "{}: {} is neither an object nor an array",
            string(tokens.get(..=i).unwrap_or(tokens)),
            string(tokens.get(..i).unwrap_or_default())
        ),
    )
}

/// The value `tokens` point to in `root`, or 404 with fit's message.
pub(crate) fn lookup<'v>(root: &'v Value, tokens: &[String]) -> Result<&'v Value, Problem> {
    let mut v = root;
    for (i, t) in tokens.iter().enumerate() {
        let upto = tokens.get(..=i).unwrap_or(tokens);
        v = match v {
            Value::Object(_) => v.get(t).ok_or_else(|| {
                Problem::new(404, format!("{}: there is no member {t:?}", string(upto)))
            })?,
            Value::Array(a) => a
                .get(index(t, a.len(), upto)?)
                .ok_or_else(|| scalar(tokens, i))?,
            _ => return Err(scalar(tokens, i)),
        };
    }
    Ok(v)
}

/// [`lookup`], for a change.
pub(crate) fn lookup_mut<'v>(
    root: &'v mut Value,
    tokens: &[String],
) -> Result<&'v mut Value, Problem> {
    // The errors, with their messages, as `lookup` finds them; then the way down, which holds.
    lookup(root, tokens)?;
    let mut v = root;
    for (i, t) in tokens.iter().enumerate() {
        v = match v {
            Value::Object(ms) => ms.iter_mut().find(|(k, _)| k == t).map(|(_, v)| v),
            Value::Array(a) => t.parse::<usize>().ok().and_then(|j| a.get_mut(j)),
            _ => None,
        }
        .ok_or_else(|| scalar(tokens, i))?;
    }
    Ok(v)
}

/// Splits `tokens` into the pointer to the parent and the last token, if there is one.
fn split(tokens: &[String]) -> Option<(&[String], &str)> {
    tokens
        .split_last()
        .map(|(last, parent)| (parent, last.as_str()))
}

/// Sets the value `tokens` point to in `root`: it replaces a member or an element, adds a member
/// to an object, or, for the token `-`, appends to an array; no token replaces `root`.
pub(crate) fn put(root: &mut Value, tokens: &[String], v: Value) -> Result<(), Problem> {
    let Some((parent, last)) = split(tokens) else {
        *root = v;
        return Ok(());
    };
    match lookup_mut(root, parent)? {
        Value::Object(ms) => match ms.iter_mut().find(|(k, _)| k == last) {
            Some((_, old)) => *old = v,
            None => ms.push((last.to_owned(), v)),
        },
        Value::Array(a) if last == "-" => a.push(v),
        Value::Array(a) => {
            let j = index(last, a.len(), tokens)?;
            if let Some(old) = a.get_mut(j) {
                *old = v;
            }
        }
        _ => return Err(scalar(tokens, parent.len())),
    }
    Ok(())
}

/// Removes the member or element `tokens` point to in `root`.
pub(crate) fn remove(root: &mut Value, tokens: &[String]) -> Result<(), Problem> {
    let Some((parent, last)) = split(tokens) else {
        return Err(Problem::new(
            400,
            "the config itself cannot be deleted".into(),
        ));
    };
    match lookup_mut(root, parent)? {
        Value::Object(ms) => {
            let Some(j) = ms.iter().position(|(k, _)| k == last) else {
                return Err(Problem::new(
                    404,
                    format!("{}: there is no member {last:?}", string(tokens)),
                ));
            };
            ms.remove(j);
        }
        Value::Array(a) => {
            let j = index(last, a.len(), tokens)?;
            a.remove(j);
        }
        _ => return Err(scalar(tokens, parent.len())),
    }
    Ok(())
}

/// Merges `patch` into `target` as RFC 7396's JSON merge patch does: an object's members are
/// merged in, a `null` removes one, and anything else replaces the target whole.
pub(crate) fn merge_patch(target: &mut Value, patch: Value) {
    let Value::Object(members) = patch else {
        *target = patch;
        return;
    };
    if !matches!(target, Value::Object(_)) {
        *target = Value::Object(Vec::new());
    }
    let Value::Object(ms) = target else { return };
    for (k, v) in members {
        let at = ms.iter().position(|(key, _)| *key == k);
        match (v, at) {
            (Value::Null, Some(j)) => {
                ms.remove(j);
            }
            (Value::Null, None) => {}
            (v, Some(j)) => {
                if let Some((_, old)) = ms.get_mut(j) {
                    merge_patch(old, v);
                }
            }
            (v, None) => {
                let mut new = Value::Null;
                merge_patch(&mut new, v);
                ms.push((k, new));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{Problem, lookup, merge_patch, parse, put, remove, string};
    use akit_config::value::{self, Value};

    /// fit's `TestPointer`.
    #[test]
    fn pointers_round_trip() {
        for (p, want) in [
            ("/", vec![""]),
            ("/a/b", vec!["a", "b"]),
            ("/a~1b/c~0d/~01", vec!["a/b", "c~d", "~1"]),
        ] {
            let got = parse(p).unwrap();
            assert_eq!(got, want, "{p}");
            assert_eq!(string(&got), p);
        }
        for bad in ["a", "/a~", "/a~2"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    /// fit's `TestEditTree`: edits keep the order of members, numbers as written and characters;
    /// what cannot be done is refused with fit's messages.
    #[test]
    fn edit_tree() {
        let doc = r#"{"z": 1.50, "a": [1, 2, 3], "o": {"k": "<&>", "n": null}, "e": {}, "f": []}"#;
        let root = value::parse(doc).unwrap();
        assert_eq!(
            root.to_json(),
            "{\n  \"z\": 1.50,\n  \"a\": [\n    1,\n    2,\n    3\n  ],\n  \"o\": {\n    \"k\": \"<&>\",\n    \"n\": null\n  },\n  \"e\": {},\n  \"f\": []\n}\n"
        );
        let v = |s: &str| value::parse(s).unwrap();
        let tok = |p: &str| parse(p).unwrap();
        // Each edit, and the document it leaves written compactly in its members' order.
        let edit = |f: &dyn Fn(&mut Value) -> Result<(), Problem>| -> String {
            let mut r = value::parse(doc).unwrap();
            match f(&mut r) {
                Ok(()) => r.to_json().split_whitespace().collect::<String>(),
                Err(p) => format!("error {}: {}", p.status, p.msg),
            }
        };
        type Edit = Box<dyn Fn(&mut Value) -> Result<(), Problem>>;
        let cases: Vec<(&str, Edit, &str)> = vec![
            (
                "put member",
                Box::new(move |r| put(r, &tok("/o/k"), v(r#""x""#))),
                r#"{"z":1.50,"a":[1,2,3],"o":{"k":"x","n":null},"e":{},"f":[]}"#,
            ),
            (
                "add member",
                Box::new(move |r| put(r, &tok("/e/new"), v("true"))),
                r#"{"z":1.50,"a":[1,2,3],"o":{"k":"<&>","n":null},"e":{"new":true},"f":[]}"#,
            ),
            (
                "put element",
                Box::new(move |r| put(r, &tok("/a/1"), v("9"))),
                r#"{"z":1.50,"a":[1,9,3],"o":{"k":"<&>","n":null},"e":{},"f":[]}"#,
            ),
            (
                "append",
                Box::new(move |r| put(r, &tok("/f/-"), v(r#""x""#))),
                r#"{"z":1.50,"a":[1,2,3],"o":{"k":"<&>","n":null},"e":{},"f":["x"]}"#,
            ),
            (
                "past the end",
                Box::new(move |r| put(r, &tok("/a/3"), v("9"))),
                "error 404: /a/3: index 3 is past the end of an array of 3",
            ),
            (
                "leading zero",
                Box::new(move |r| lookup(r, &tok("/a/01")).map(|_| ())),
                r#"error 404: /a/01: "01" is not an array index"#,
            ),
            (
                "no parent",
                Box::new(move |r| put(r, &tok("/x/y"), v("1"))),
                r#"error 404: /x: there is no member "x""#,
            ),
            (
                "into a scalar",
                Box::new(move |r| put(r, &tok("/z/y"), v("1"))),
                "error 404: /z/y: /z is neither an object nor an array",
            ),
            (
                "remove member",
                Box::new(move |r| remove(r, &tok("/o"))),
                r#"{"z":1.50,"a":[1,2,3],"e":{},"f":[]}"#,
            ),
            (
                "remove element",
                Box::new(move |r| remove(r, &tok("/a/0"))),
                r#"{"z":1.50,"a":[2,3],"o":{"k":"<&>","n":null},"e":{},"f":[]}"#,
            ),
            (
                "remove missing",
                Box::new(move |r| remove(r, &tok("/q"))),
                r#"error 404: /q: there is no member "q""#,
            ),
            (
                "remove from a scalar",
                Box::new(move |r| remove(r, &tok("/z/q"))),
                "error 404: /z/q: /z is neither an object nor an array",
            ),
            (
                "remove root",
                Box::new(|r| remove(r, &[])),
                "error 400: the config itself cannot be deleted",
            ),
            ("put root", Box::new(move |r| put(r, &[], v("[]"))), "[]"),
            (
                "merge",
                Box::new(move |r| {
                    merge_patch(
                        r,
                        v(r#"{"o": {"k": null, "m": 2}, "z": [1], "new": {"a": null, "b": 1}}"#),
                    );
                    Ok(())
                }),
                r#"{"z":[1],"a":[1,2,3],"o":{"n":null,"m":2},"e":{},"f":[],"new":{"b":1}}"#,
            ),
            (
                "merge a scalar",
                Box::new(move |r| {
                    merge_patch(r, v("7"));
                    Ok(())
                }),
                "7",
            ),
        ];
        for (name, f, want) in &cases {
            assert_eq!(edit(f.as_ref()), *want, "{name}");
        }
        for bad in ["", "{", r#"{"a": 1, "a": 2}"#, "1 2"] {
            assert!(value::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn pointers() {
        assert_eq!(parse("").unwrap(), Vec::<String>::new());
        assert_eq!(parse("/a~1b/~0/0").unwrap(), ["a/b", "~", "0"]);
        assert_eq!(parse("/").unwrap(), [""]);
        assert_eq!(parse("a").unwrap_err().status, 400);
        assert!(
            parse("/a~2")
                .unwrap_err()
                .msg
                .contains("'~' must be followed")
        );

        let root = value::parse(r#"{"a/b": [10, {"c": true}], "n": 1}"#).unwrap();
        let get = |p: &str| lookup(&root, &parse(p).unwrap());
        assert_eq!(get("/a~1b/1/c").unwrap(), &Value::Bool(true));
        assert_eq!(get("").unwrap(), &root);
        for (p, want) in [
            ("/x", r#"/x: there is no member "x""#),
            (
                "/a~1b/2",
                "/a~1b/2: index 2 is past the end of an array of 2",
            ),
            ("/a~1b/01", r#"/a~1b/01: "01" is not an array index"#),
            ("/a~1b/-", r#"/a~1b/-: "-" is not an array index"#),
            ("/n/x", "/n/x: /n is neither an object nor an array"),
        ] {
            let e = get(p).unwrap_err();
            assert_eq!((e.status, e.msg.as_str()), (404, want), "{p}");
        }
    }
}
