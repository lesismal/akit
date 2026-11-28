//! JSON values a proposal edits (fit's `assistant/jsontree.go`): [`Value`]s, whose objects keep
//! their members in the order they were written, so that a config a proposal changes keeps the
//! order its person wrote it in; JSON pointers (RFC 6901) into them, and the edits a step makes
//! at one: add, replace and remove.

use akit_config::value::Value;

/// The operations of a patch: add sets a member or inserts an element (at `-`, at the end),
/// replace sets what is there, remove removes it.
pub const OP_ADD: &str = "add";
/// See [`OP_ADD`].
pub const OP_REPLACE: &str = "replace";
/// See [`OP_ADD`].
pub const OP_REMOVE: &str = "remove";

/// The member `k` of an object.
pub fn get<'v>(o: &'v Value, k: &str) -> Option<&'v Value> {
    o.get(k)
}

/// Sets the member `k` of an object, at the end when it is new; nothing on another value.
pub fn set(o: &mut Value, k: &str, v: Value) {
    if let Value::Object(ms) = o {
        match ms.iter_mut().find(|(key, _)| key == k) {
            Some((_, x)) => *x = v,
            None => ms.push((k.to_owned(), v)),
        }
    }
}

/// Removes the member `k` of an object.
pub fn del(o: &mut Value, k: &str) {
    if let Value::Object(ms) = o {
        ms.retain(|(key, _)| key != k);
    }
}

/// Whether `a` and `b` are the same JSON value: members' order does not count, and numbers are
/// compared as written.
#[must_use]
pub fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.iter().any(|(k2, w)| k == k2 && equal(v, w)))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(v, w)| equal(v, w))
        }
        _ => a == b,
    }
}

/// The tokens of a JSON pointer, as `/routes/0/handle`; `""` is the whole value.
///
/// # Errors
///
/// A pointer that does not start with `/`.
pub fn parse_pointer(p: &str) -> Result<Vec<String>, String> {
    if p.is_empty() {
        return Ok(Vec::new());
    }
    let Some(rest) = p.strip_prefix('/') else {
        return Err(format!(
            "{} is not a JSON pointer: it starts with /",
            crate::json::quoted(p)
        ));
    };
    Ok(rest
        .split('/')
        .map(|s| s.replace("~1", "/").replace("~0", "~"))
        .collect())
}

/// `tokens` joined into a JSON pointer.
#[must_use]
pub fn pointer_of<S: AsRef<str>>(tokens: &[S]) -> String {
    let mut b = String::new();
    for t in tokens {
        b.push('/');
        b.push_str(&t.as_ref().replace('~', "~0").replace('/', "~1"));
    }
    b
}

/// An array's index token for an array of `n` elements; `end` allows `n` itself, an insertion at
/// the end.
fn index(tok: &str, n: usize, end: bool) -> Result<usize, String> {
    let i = tok
        .parse::<usize>()
        .ok()
        .filter(|i| i.to_string() == tok)
        .ok_or_else(|| format!("{} is not an array's index", crate::json::quoted(tok)))?;
    if i > n || (i == n && !end) {
        return Err(format!("index {i} is past the array's {n} elements"));
    }
    Ok(i)
}

/// The value at `tokens` in `root`.
#[must_use]
pub fn lookup<'v, S: AsRef<str>>(root: &'v Value, tokens: &[S]) -> Option<&'v Value> {
    let mut cur = root;
    for t in tokens {
        let t = t.as_ref();
        cur = match cur {
            Value::Object(_) => cur.get(t)?,
            Value::Array(a) => a.get(index(t, a.len(), false).ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

fn lookup_mut<'v>(root: &'v mut Value, tokens: &[String]) -> Option<&'v mut Value> {
    let mut cur = root;
    for t in tokens {
        cur = match cur {
            Value::Object(ms) => ms.iter_mut().find(|(k, _)| k == t).map(|(_, v)| v)?,
            Value::Array(a) => {
                let i = index(t, a.len(), false).ok()?;
                a.get_mut(i)?
            }
            _ => return None,
        };
    }
    Some(cur)
}

/// Makes the objects along `tokens` in `root` that are missing, the last an array when `array`.
fn make_path(root: &mut Value, tokens: &[String], array: bool) -> Result<(), String> {
    let mut cur = root;
    for (i, t) in tokens.iter().enumerate() {
        let Value::Object(ms) = cur else {
            return Err(format!(
                "{} is not an object",
                pointer_of(tokens.get(..i).unwrap_or_default())
            ));
        };
        let at = match ms.iter().position(|(k, _)| k == t) {
            Some(at) => at,
            None => {
                let next = if i.saturating_add(1) == tokens.len() && array {
                    Value::Array(Vec::new())
                } else {
                    Value::Object(Vec::new())
                };
                ms.push((t.clone(), next));
                ms.len().saturating_sub(1)
            }
        };
        let Some((_, next)) = ms.get_mut(at) else {
            return Err("the path could not be made".into());
        };
        cur = next;
    }
    Ok(())
}

/// Applies `op` at `tokens`, not empty, to `root`. With `create`, the objects and the array an
/// add's path goes through are made when the config leaves them out, as a reconcile's fix
/// expects. On an error, `root` may be changed only by the path made.
///
/// # Errors
///
/// What keeps the edit from being made, as fit words it.
pub fn patch(
    root: &mut Value,
    op: &str,
    tokens: &[String],
    v: Option<Value>,
    create: bool,
) -> Result<(), String> {
    let Some((last, parent_tokens)) = tokens.split_last() else {
        return Err(format!("{op} needs a path below the config's top"));
    };
    if lookup(root, parent_tokens).is_none() && create && op == OP_ADD {
        make_path(root, parent_tokens, last == "-")?;
    }
    let Some(parent) = lookup_mut(root, parent_tokens) else {
        return Err(format!(
            "the config has nothing at {}",
            pointer_of(parent_tokens)
        ));
    };
    match parent {
        Value::Object(ms) => {
            let at = ms.iter().position(|(k, _)| k == last);
            match (op, at) {
                (OP_ADD, Some(at)) => {
                    if let Some((_, x)) = ms.get_mut(at) {
                        *x = v.unwrap_or(Value::Null);
                    }
                }
                (OP_ADD, None) => ms.push((last.clone(), v.unwrap_or(Value::Null))),
                (_, None) => {
                    return Err(format!(
                        "the config has nothing at {} to {op}",
                        pointer_of(tokens)
                    ));
                }
                (OP_REPLACE, Some(at)) => {
                    if let Some((_, x)) = ms.get_mut(at) {
                        *x = v.unwrap_or(Value::Null);
                    }
                }
                (_, Some(at)) => {
                    ms.remove(at);
                }
            }
            Ok(())
        }
        Value::Array(a) => {
            if op == OP_ADD && last == "-" {
                a.push(v.unwrap_or(Value::Null));
            } else if op == OP_ADD {
                let i = index(last, a.len(), true)?;
                a.insert(i, v.unwrap_or(Value::Null));
            } else {
                let i = index(last, a.len(), false)?;
                if op == OP_REPLACE {
                    if let Some(x) = a.get_mut(i) {
                        *x = v.unwrap_or(Value::Null);
                    }
                } else {
                    a.remove(i);
                }
            }
            Ok(())
        }
        _ => Err(format!(
            "{} is neither an object nor an array",
            pointer_of(parent_tokens)
        )),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::json::compact;

    fn tree(s: &str) -> Value {
        akit_config::value::parse(s).unwrap()
    }

    fn tokens(p: &str) -> Vec<String> {
        parse_pointer(p).unwrap()
    }

    /// fit's `TestTree`: members kept in their order; equality without it; pointers; the edits
    /// and their refusals; paths made by a reconcile's add.
    #[test]
    fn edits() {
        let src = r#"{"b":1,"a":[1,2,{"x":"<&>","y":null,"z":true}],"c":{"d":"e~/f"},"n":1.50}"#;
        let v = tree(src);
        assert_eq!(compact(&v), src);
        assert!(equal(
            &tree(r#"{"a":1,"b":[1]}"#),
            &tree(r#"{"b":[1],"a":1}"#)
        ));
        for (a, b) in [
            (r#"{"a":1}"#, r#"{"b":1}"#),
            (r#"{"a":1}"#, r#"{"a":1,"b":2}"#),
            ("[1]", "[2]"),
            ("[1]", "{}"),
            ("{}", "[]"),
            ("[1]", "[1,2]"),
        ] {
            assert!(!equal(&tree(a), &tree(b)), "{a} {b}");
        }
        let t = tokens("/c/d~1x~0y");
        assert_eq!(t, ["c", "d/x~y"]);
        assert_eq!(pointer_of(&t), "/c/d~1x~0y");
        assert!(tokens("").is_empty());
        assert!(
            parse_pointer("routes")
                .unwrap_err()
                .contains("starts with /")
        );

        for (op, path, value, want) in [
            (OP_ADD, "/a/0", "0", r#""a":[0,1,2,"#),
            (OP_ADD, "/a/3", "9", ",9]"),
            (OP_ADD, "/a/-", "9", ",9]"),
            (OP_REPLACE, "/a/1", "7", r#""a":[1,7,"#),
            (OP_REMOVE, "/a/0", "", r#""a":[2,"#),
            (OP_ADD, "/new", r#""x""#, r#""new":"x""#),
            (OP_REPLACE, "/b", "2", r#"{"b":2,"#),
            (OP_REMOVE, "/c", "", r#"}],"n":1.50}"#),
            (OP_REMOVE, "/a/2/y", "", r#"{"x":"<&>","z":true}"#),
        ] {
            let mut got = v.clone();
            let val = (!value.is_empty()).then(|| tree(value));
            patch(&mut got, op, &tokens(path), val, false).unwrap();
            assert!(
                compact(&got).contains(want),
                "{op} {path}: {}",
                compact(&got)
            );
        }
        for (op, path) in [
            (OP_REPLACE, "/nope"),
            (OP_REMOVE, "/nope"),
            (OP_ADD, "/a/9"),
            (OP_ADD, "/a/01"),
            (OP_REPLACE, "/a/3"),
            (OP_REPLACE, "/a/-"),
            (OP_ADD, "/b/x"),
            (OP_ADD, "/x/y/z"),
            (OP_ADD, ""),
        ] {
            let mut got = v.clone();
            assert!(
                patch(&mut got, op, &tokens(path), Some(tree("1")), false).is_err(),
                "{op} {path}"
            );
        }
        // With create, an add makes the objects, and the array, it goes through; not through a
        // value that is neither.
        let mut got = v.clone();
        patch(&mut got, OP_ADD, &tokens("/p/q/-"), Some(tree("1")), true).unwrap();
        assert!(compact(&got).contains(r#""p":{"q":[1]}"#));
        let mut got = v.clone();
        assert!(patch(&mut got, OP_ADD, &tokens("/b/q/r"), Some(tree("1")), true).is_err());
        // An array at the top, and one in it.
        let mut arr = tree("[1,2]");
        patch(&mut arr, OP_REMOVE, &tokens("/0"), None, false).unwrap();
        assert_eq!(compact(&arr), "[2]");
        let mut nested = tree("[[1],[2]]");
        patch(&mut nested, OP_ADD, &tokens("/1/-"), Some(tree("3")), false).unwrap();
        assert_eq!(compact(&nested), "[[1],[2,3]]");
        assert!(lookup(&v, &["b", "x"]).is_none());
        assert!(lookup(&v, &["a", "x"]).is_none());
        let mut o = Value::Object(Vec::new());
        del(&mut o, "none");
    }
}
