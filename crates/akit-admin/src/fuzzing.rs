//! Fuzz entry points (feature `fuzzing`; not a public API): the `fuzz/` cargo-fuzz project calls
//! them, and the tests below replay `fuzz/corpus/<target>` on stable. The Admin API's JSON
//! pointers (`/config/...` paths) and merge patches, which its clients send.
#![allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::panic,
    clippy::expect_used
)]

use akit_config::value::{self, Value};

use crate::pointer;

/// `admin_pointer`: NUL-separated, a JSON pointer, a document and a value. A pointer that
/// parses writes back as itself; a value put where it points is found there; a merge patch
/// (the value) applied twice changes nothing the first did not.
pub fn admin_pointer(data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let mut p = text.splitn(3, '\0');
    let (ptr, doc, val) = (
        p.next().unwrap_or(""),
        p.next().unwrap_or(""),
        p.next().unwrap_or(""),
    );
    let (Ok(mut doc), Ok(v)) = (value::parse(doc), value::parse(val)) else {
        return;
    };
    let mut patched = doc.clone();
    pointer::merge_patch(&mut patched, v.clone());
    let mut twice = patched.clone();
    pointer::merge_patch(&mut twice, v.clone());
    assert_eq!(twice, patched, "a merge patch applied twice");

    let Ok(tokens) = pointer::parse(ptr) else {
        return;
    };
    assert_eq!(
        pointer::string(&tokens),
        ptr,
        "the pointer writes back otherwise"
    );
    let _ = pointer::lookup(&doc, &tokens);
    // `-` appends to an array; in an object it names a member as any token does (RFC 6901).
    let appends = tokens.last().is_some_and(|t| t == "-")
        && matches!(
            pointer::lookup(&doc, &tokens[..tokens.len() - 1]),
            Ok(Value::Array(_))
        );
    if pointer::put(&mut doc, &tokens, v.clone()).is_ok() {
        if appends {
            let parent = &tokens[..tokens.len() - 1];
            match pointer::lookup(&doc, parent) {
                Ok(Value::Array(a)) => assert_eq!(a.last(), Some(&v)),
                other => panic!("appended to {other:?}"),
            }
        } else {
            match pointer::lookup(&doc, &tokens) {
                Ok(found) => assert!(
                    *found == v || has_duplicate_keys(&doc),
                    "put {v:?} at {ptr:?}, found {found:?}"
                ),
                Err(e) => panic!("put at {ptr:?}, then not found: {e:?}"),
            }
            if !tokens.is_empty() {
                assert!(pointer::remove(&mut doc, &tokens).is_ok());
            }
        }
    }
}

/// Whether an object in `v` names a member twice, which a lookup finds the first of.
fn has_duplicate_keys(v: &Value) -> bool {
    match v {
        Value::Object(ms) => {
            ms.iter()
                .enumerate()
                .any(|(i, (k, _))| ms.iter().skip(i + 1).any(|(k2, _)| k2 == k))
                || ms.iter().any(|(_, x)| has_duplicate_keys(x))
        }
        Value::Array(a) => a.iter().any(has_duplicate_keys),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `f` on every file of `fuzz/corpus/<target>`.
    fn replay(target: &str, f: fn(&[u8])) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fuzz/corpus")
            .join(target);
        let mut n = 0;
        for e in std::fs::read_dir(&dir).unwrap() {
            f(&std::fs::read(e.unwrap().path()).unwrap());
            n += 1;
        }
        assert!(n > 0, "{} is empty", dir.display());
    }

    #[test]
    fn corpus_admin_pointer() {
        replay("admin_pointer", admin_pointer);
    }
}
