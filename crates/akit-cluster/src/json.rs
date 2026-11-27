//! JSON as the manager reads and writes it: [`Value`]s read with akit's reader, written compact
//! as Go's `encoding/json` writes them, members in their order; and times as fit writes them.

use std::time::{SystemTime, UNIX_EPOCH};

use akit_config::value::{Value, write_string};

/// `v` written compact, members in their order, numbers as written.
#[must_use]
pub fn compact(v: &Value) -> String {
    let mut b = String::new();
    write(&mut b, v);
    b
}

/// Appends `v`, compact.
pub fn write(b: &mut String, v: &Value) {
    match v {
        Value::Null => b.push_str("null"),
        Value::Bool(t) => b.push_str(if *t { "true" } else { "false" }),
        Value::Number(n) => b.push_str(n),
        Value::String(s) => write_string(b, s),
        Value::Array(a) => {
            b.push('[');
            for (i, v) in a.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                write(b, v);
            }
            b.push(']');
        }
        Value::Object(ms) => {
            b.push('{');
            for (i, (k, v)) in ms.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                write_string(b, k);
                b.push(':');
                write(b, v);
            }
            b.push('}');
        }
    }
}

/// `s` as a JSON string.
#[must_use]
pub fn quoted(s: &str) -> String {
    let mut b = String::new();
    write_string(&mut b, s);
    b
}

/// Parses `data`, JSON; `None` when it is not.
#[must_use]
pub fn parse(data: &[u8]) -> Option<Value> {
    std::str::from_utf8(data)
        .ok()
        .and_then(|t| akit_config::value::parse(t).ok())
}

/// String member `key` of `v`, `""` when it is not one.
#[must_use]
pub fn str_of<'v>(v: &'v Value, key: &str) -> &'v str {
    match v.get(key) {
        Some(Value::String(s)) => s,
        _ => "",
    }
}

/// Integer member `key` of `v`, 0 when it is not one.
#[must_use]
pub fn int_of(v: &Value, key: &str) -> i64 {
    match v.get(key) {
        Some(Value::Number(n)) => n.parse().unwrap_or(0),
        _ => 0,
    }
}

/// Unsigned integer member `key` of `v`, 0 when it is not one.
#[must_use]
pub fn uint_of(v: &Value, key: &str) -> u64 {
    match v.get(key) {
        Some(Value::Number(n)) => n.parse().unwrap_or(0),
        _ => 0,
    }
}

/// Boolean member `key` of `v`, false when it is not one.
#[must_use]
pub fn bool_of(v: &Value, key: &str) -> bool {
    matches!(v.get(key), Some(Value::Bool(true)))
}

/// The strings of array member `key` of `v`; what is not a string is left out.
#[must_use]
pub fn strings_of(v: &Value, key: &str) -> Vec<String> {
    match v.get(key) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| match x {
                Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A list of strings as JSON.
#[must_use]
pub fn string_list(ss: &[String]) -> String {
    let mut b = String::from("[");
    for (i, s) in ss.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        write_string(&mut b, s);
    }
    b.push(']');
    b
}

/// `t` as Go's `time.Time` writes itself in JSON: RFC 3339 in UTC, with the nanoseconds when
/// there are any, quoted.
#[must_use]
pub fn time(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let mut v = Vec::new();
    akit_observe::time::rfc3339_nano(
        &mut v,
        i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        d.subsec_nanos(),
        0,
    );
    quoted(&String::from_utf8_lossy(&v))
}

/// Milliseconds of a duration, as fit's `Milliseconds()`.
#[must_use]
pub fn millis(d: std::time::Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// A JSON object being written: members appended in order.
#[derive(Debug, Default)]
pub struct Obj(String);

impl Obj {
    /// An empty object.
    #[must_use]
    pub fn new() -> Obj {
        Obj(String::from("{"))
    }

    fn key(&mut self, k: &str) {
        if self.0.len() > 1 {
            self.0.push(',');
        }
        write_string(&mut self.0, k);
        self.0.push(':');
    }

    /// Member `k`, raw JSON.
    #[must_use]
    pub fn raw(mut self, k: &str, json: &str) -> Obj {
        self.key(k);
        self.0.push_str(json);
        self
    }

    /// Member `k`, a string.
    #[must_use]
    pub fn str(mut self, k: &str, v: &str) -> Obj {
        self.key(k);
        write_string(&mut self.0, v);
        self
    }

    /// Member `k`, a string, left out when empty (Go's `omitempty`).
    #[must_use]
    pub fn str_omit(self, k: &str, v: &str) -> Obj {
        if v.is_empty() { self } else { self.str(k, v) }
    }

    /// Member `k`, a number.
    #[must_use]
    pub fn num(self, k: &str, v: impl std::fmt::Display) -> Obj {
        let s = v.to_string();
        self.raw(k, &s)
    }

    /// Member `k`, a boolean.
    #[must_use]
    pub fn bool(self, k: &str, v: bool) -> Obj {
        self.raw(k, if v { "true" } else { "false" })
    }

    /// Member `k`, raw JSON, left out when `None`.
    #[must_use]
    pub fn opt(self, k: &str, json: Option<&str>) -> Obj {
        match json {
            Some(j) => self.raw(k, j),
            None => self,
        }
    }

    /// The object, closed.
    #[must_use]
    pub fn end(mut self) -> String {
        self.0.push('}');
        self.0
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn writes_compact() {
        let v = parse(br#"{"a": [1, "x", null, true], "b": {"c": 2.50}}"#).unwrap();
        assert_eq!(compact(&v), r#"{"a":[1,"x",null,true],"b":{"c":2.50}}"#);
        let o = Obj::new()
            .str("s", "q\"")
            .num("n", 3)
            .bool("t", true)
            .str_omit("e", "")
            .opt("o", None)
            .end();
        assert_eq!(o, r#"{"s":"q\"","n":3,"t":true}"#);
        let t = UNIX_EPOCH + std::time::Duration::new(1_793_581_323, 5);
        assert_eq!(time(t), "\"2026-11-02T01:02:03.000000005Z\"");
        assert_eq!(
            time(UNIX_EPOCH + std::time::Duration::from_secs(1_793_581_323)),
            "\"2026-11-02T01:02:03Z\""
        );
    }
}
