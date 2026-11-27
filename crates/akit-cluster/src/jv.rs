//! JSON values as the templates handle them (fit's `cluster/jsonv.go` and `internal/managed`):
//! members read and set in their order, the elements of the named lists found by `name`, every
//! string walked with the object members above it, and the canonical form and SHA-256 two
//! managers and their members compare a part by.

use akit_config::value::Value;

pub use akit_config::cluster::{
    MANAGED_LISTS as LISTS, UNOWNABLE, check_managed_part as check_part,
    is_managed_list as is_list, managed_display as display,
};

/// Member `key` of object `v`.
#[must_use]
pub fn get<'v>(v: &'v Value, key: &str) -> Option<&'v Value> {
    v.get(key)
}

/// Sets member `key` of object `v` to `x`, in its place or at the end.
pub fn set(v: &mut Value, key: &str, x: Value) {
    if let Value::Object(ms) = v {
        match ms.iter_mut().find(|(k, _)| k == key) {
            Some((_, old)) => *old = x,
            None => ms.push((key.to_owned(), x)),
        }
    }
}

/// An object's `name`, or empty.
#[must_use]
pub fn name_of(v: &Value) -> &str {
    match v.get("name") {
        Some(Value::String(s)) => s,
        _ => "",
    }
}

/// The value of part `member` (`name`) in `cfg`: fit's `lookupJV`.
#[must_use]
pub fn lookup<'v>(cfg: &'v Value, member: &str, name: &str) -> Option<&'v Value> {
    akit_config::cluster::managed_lookup(cfg, member, name)
}

/// Calls `f` on every string of `v` with the object members above it, replacing it with what `f`
/// returns; stops at the first error.
///
/// # Errors
///
/// What `f` returned.
#[allow(clippy::type_complexity)]
pub fn walk_strings(
    v: &mut Value,
    path: &mut Vec<String>,
    f: &mut dyn FnMut(&[String], &str) -> Result<String, String>,
) -> Result<(), String> {
    match v {
        Value::String(s) => {
            *s = f(path, s)?;
        }
        Value::Array(items) => {
            for it in items {
                walk_strings(it, path, f)?;
            }
        }
        Value::Object(ms) => {
            for (k, x) in ms {
                path.push(k.clone());
                let r = walk_strings(x, path, f);
                path.pop();
                r?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// `v` in its canonical form, fit's `managed.Canonical`.
#[must_use]
pub fn canonical(v: &Value) -> String {
    akit_config::cluster::canonical_json(v)
}

/// The lower-case hex SHA-256 of `v`'s canonical form: fit's `managed.Hash`.
#[must_use]
pub fn hash(v: &Value) -> String {
    akit_config::cluster::managed_hash(v)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn canonical_as_go_writes_it() {
        let v = crate::json::parse(br#"{"b": [1.0, "x<y"], "a": {"d": true, "c": null}}"#).unwrap();
        assert_eq!(
            canonical(&v),
            r#"{"a":{"c":null,"d":true},"b":[1.0,"x<y"]}"#
        );
        // fit's managed.Hash of the same, as Go's encoding/json gives it (computed with Go).
        assert_eq!(
            hash(&v),
            "865ce1af88c20bf2032cb01a1b1222b8c736e2725da056894ec9fdfbc80de9c3"
        );
        let s = crate::json::parse("{\"s\": \"\u{e9}\\u2028\\\"\\\\\"}".as_bytes()).unwrap();
        assert_eq!(canonical(&s), "{\"s\":\"\u{e9}\\u2028\\\"\\\\\"}");
        assert_eq!(
            hash(&s),
            "0e5ae6bcfa21bb31b81111244df530a152bd481aba3c9d0ab060cf0befda2d04"
        );
        assert_eq!(
            hash(&crate::json::parse(br#"{"x": 1}"#).unwrap()),
            "5041bf1f713df204784353e82f6a4a535931cb64f1f4b4a5aeaffcb720918b22"
        );
    }

    #[test]
    fn parts() {
        assert!(check_part("log", "").is_none());
        assert!(check_part("routes", "api").is_none());
        assert!(check_part("routes", "").is_some());
        assert!(check_part("log", "x").is_some());
        assert!(check_part("admin", "").is_some());
        assert!(check_part("Log", "").is_some());
        let cfg = crate::json::parse(br#"{"routes": [{"name": "a", "x": 1}, {"y": 2}]}"#).unwrap();
        assert_eq!(
            lookup(&cfg, "routes", "a").map(canonical).as_deref(),
            Some(r#"{"name":"a","x":1}"#)
        );
        assert!(lookup(&cfg, "routes", "b").is_none());
        assert_eq!(display("routes", "a"), "routes[a]");
    }
}
