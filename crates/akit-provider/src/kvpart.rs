//! A part made from the keys of a key-value store, as the Consul KV and etcd providers read
//! them: fit's `provider/internal/kvpart`. Each key under a prefix holds a part of its own, a
//! JSON object of listeners, routes, upstreams and services, and the part provided is theirs
//! together, in the order of their keys.

use akit_config::value::{self, Value};

use crate::merge::{KINDS, parse};

/// A key and its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) key: String,
    pub(crate) value: Vec<u8>,
}

/// Joins the parts `entries` hold, in the order of their keys, and says why it left out the
/// entries it did: those whose value is not a part, and those past the first `max` with a
/// value. Keys ending in `/`, which name folders, and empty values are passed over. The part is
/// the same for the same entries, so that an unchanged one is seen to be.
pub(crate) fn part(mut entries: Vec<Entry>, max: usize) -> (Vec<u8>, Vec<String>) {
    entries.sort_by(|a, b| a.key.cmp(&b.key));
    let mut kinds: [Vec<String>; 4] = Default::default();
    let mut problems = Vec::new();
    let mut taken = 0usize;
    for e in &entries {
        if e.key.ends_with('/') || e.value.trim_ascii().is_empty() {
            continue;
        }
        if taken == max {
            problems.push(format!(
                "key {}: left out: max_keys, {max}, are taken",
                e.key
            ));
            continue;
        }
        if let Err(errs) = parse("", &e.value) {
            let msg = errs
                .as_slice()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ");
            problems.push(format!("key {}: {msg}", e.key));
            continue;
        }
        // parse took it: UTF-8 JSON, an object of arrays of KINDS (comments blanked out).
        let doc = std::str::from_utf8(&e.value)
            .ok()
            .and_then(|t| value::standardize(t).ok())
            .and_then(|t| value::parse(&t).ok());
        let Some(Value::Object(members)) = doc else {
            problems.push(format!("key {}: not a JSON object", e.key));
            continue;
        };
        for (k, v) in members {
            let (Some(i), Value::Array(items)) = (KINDS.iter().position(|kind| *kind == k), v)
            else {
                continue;
            };
            if let Some(list) = kinds.get_mut(i) {
                for item in &items {
                    let mut b = String::new();
                    compact(item, &mut b);
                    list.push(b);
                }
            }
        }
        taken = taken.saturating_add(1);
    }
    let mut b = String::from("{");
    for (kind, items) in KINDS.iter().zip(&kinds) {
        if items.is_empty() {
            continue;
        }
        if b.len() > 1 {
            b.push(',');
        }
        b.push('"');
        b.push_str(kind);
        b.push_str("\":[");
        b.push_str(&items.join(","));
        b.push(']');
    }
    b.push('}');
    (b.into_bytes(), problems)
}

/// `v` as compact JSON: members in their order, numbers as written, as Go's `json.Compact`.
pub(crate) fn compact(v: &Value, b: &mut String) {
    match v {
        Value::Null => b.push_str("null"),
        Value::Bool(t) => b.push_str(if *t { "true" } else { "false" }),
        Value::Number(n) => b.push_str(n),
        Value::String(s) => value::write_string(b, s),
        Value::Array(items) => {
            b.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                compact(item, b);
            }
            b.push(']');
        }
        Value::Object(members) => {
            b.push('{');
            for (i, (k, v)) in members.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                value::write_string(b, k);
                b.push(':');
                compact(v, b);
            }
            b.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::{Entry, part};

    fn e(key: &str, value: &str) -> Entry {
        Entry {
            key: key.to_owned(),
            value: value.as_bytes().to_vec(),
        }
    }

    /// fit's `TestPart`.
    #[test]
    fn parts() {
        let entries = vec![
            e(
                "fit/b",
                r#"{"routes": [{"name": "b", "handler": {"module": "respond"}}], "upstreams": [{"name": "u2", "addresses": ["10.0.0.2:80"]}]}"#,
            ),
            e(
                "fit/a",
                "{\n  \"upstreams\": [{\"name\": \"u1\", \"addresses\": [\"10.0.0.1:80\"]}],\n  \"listeners\": [{\"name\": \"l\", \"address\": \":8080\"}]\n}",
            ),
            e("fit/", ""),                                // a folder
            e("fit/dir/", "junk"),                        // a folder, with a value
            e("fit/empty", "  \n"),                       // empty
            e("fit/c", r#"{"access_log": {}}"#),          // not a part
            e("fit/d", r#"{"routes": ["#),                // not JSON
            e("fit/e", r#"{"services": {"name": "x"}}"#), // not an array
        ];
        let (p, problems) = part(entries, 10);
        let want = concat!(
            r#"{"listeners":[{"name":"l","address":":8080"}],"#,
            r#""routes":[{"name":"b","handler":{"module":"respond"}}],"#,
            r#""upstreams":[{"name":"u1","addresses":["10.0.0.1:80"]},{"name":"u2","addresses":["10.0.0.2:80"]}]}"#
        );
        assert_eq!(String::from_utf8(p).unwrap(), want);
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(
            problems[0].starts_with("key fit/c: ")
                && problems[0].contains("only listeners, routes, upstreams and services"),
            "{problems:?}"
        );
        assert!(problems[1].starts_with("key fit/d: "), "{problems:?}");
        assert!(problems[2].starts_with("key fit/e: "), "{problems:?}");

        // Past max, the rest by key are left out.
        let (p, problems) = part(
            vec![
                e(
                    "k3",
                    r#"{"routes": [{"name": "c", "handler": {"module": "respond"}}]}"#,
                ),
                e(
                    "k1",
                    r#"{"routes": [{"name": "a", "handler": {"module": "respond"}}]}"#,
                ),
                e(
                    "k2",
                    r#"{"routes": [{"name": "b", "handler": {"module": "respond"}}]}"#,
                ),
            ],
            2,
        );
        let p = String::from_utf8(p).unwrap();
        assert!(
            p.contains(r#""a""#) && p.contains(r#""b""#) && !p.contains(r#""c""#),
            "{p}"
        );
        assert_eq!(problems, ["key k3: left out: max_keys, 2, are taken"]);

        let (p, problems) = part(Vec::new(), 10);
        assert_eq!((p.as_slice(), problems.len()), (&b"{}"[..], 0));
    }
}
