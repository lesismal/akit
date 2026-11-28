//! JSON as the assistant reads and writes it: [`Value`]s read with akit's reader, written compact
//! as Go's `encoding/json` writes them, members in their order and numbers as written.

use akit_config::value::{Value, write_string};

/// `v` written compact.
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

/// Strings as a JSON array.
#[must_use]
pub fn strings<S: AsRef<str>>(items: &[S]) -> String {
    let mut b = String::from("[");
    for (i, s) in items.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        write_string(&mut b, s.as_ref());
    }
    b.push(']');
    b
}

/// String member `key` of `v`, `""` when it is not one.
#[must_use]
pub fn str_of<'v>(v: &'v Value, key: &str) -> &'v str {
    match v.get(key) {
        Some(Value::String(s)) => s,
        _ => "",
    }
}

/// A JSON object written member by member, in order.
#[derive(Debug)]
pub struct Obj(String);

impl Default for Obj {
    fn default() -> Obj {
        Obj::new()
    }
}

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

    /// The object, closed.
    #[must_use]
    pub fn end(mut self) -> String {
        self.0.push('}');
        self.0
    }
}
