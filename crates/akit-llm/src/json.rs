//! The little JSON the mesh speaks: compact objects written as Go's `encoding/json` writes them
//! (members in the order given, strings escaped as Go escapes them, HTML escaping off), and
//! values read with akit-config's parser.

use akit_config::value::{Value, write_string};

/// A JSON object being written, compact.
#[derive(Debug, Default)]
pub struct Obj {
    b: String,
    any: bool,
}

impl Obj {
    /// An empty object.
    #[must_use]
    pub fn new() -> Obj {
        Obj {
            b: String::from("{"),
            any: false,
        }
    }

    fn key(&mut self, k: &str) {
        if self.any {
            self.b.push(',');
        }
        self.any = true;
        write_string(&mut self.b, k);
        self.b.push(':');
    }

    /// Member `k`, a string.
    #[must_use]
    pub fn str(mut self, k: &str, v: &str) -> Obj {
        self.key(k);
        write_string(&mut self.b, v);
        self
    }

    /// Member `k`, a string left out when empty (Go's `omitempty`).
    #[must_use]
    pub fn str_omit(self, k: &str, v: &str) -> Obj {
        if v.is_empty() { self } else { self.str(k, v) }
    }

    /// Member `k`, an integer.
    #[must_use]
    pub fn int(mut self, k: &str, v: i64) -> Obj {
        self.key(k);
        self.b.push_str(&v.to_string());
        self
    }

    /// Member `k`, an integer left out when 0.
    #[must_use]
    pub fn int_omit(self, k: &str, v: i64) -> Obj {
        if v == 0 { self } else { self.int(k, v) }
    }

    /// Member `k`, a number as Go writes a float64.
    #[must_use]
    pub fn float(mut self, k: &str, v: f64) -> Obj {
        self.key(k);
        self.b.push_str(&float(v));
        self
    }

    /// Member `k`, a boolean.
    #[must_use]
    pub fn bool(mut self, k: &str, v: bool) -> Obj {
        self.key(k);
        self.b.push_str(if v { "true" } else { "false" });
        self
    }

    /// Member `k`, a boolean left out when false.
    #[must_use]
    pub fn bool_omit(self, k: &str, v: bool) -> Obj {
        if v { self.bool(k, v) } else { self }
    }

    /// Member `k`, JSON already written.
    #[must_use]
    pub fn raw(mut self, k: &str, json: &str) -> Obj {
        self.key(k);
        self.b.push_str(json);
        self
    }

    /// Member `k`, an array of strings.
    #[must_use]
    pub fn strs(mut self, k: &str, v: &[String]) -> Obj {
        self.key(k);
        self.b.push_str(&strs(v));
        self
    }

    /// The object, written.
    #[must_use]
    pub fn end(mut self) -> String {
        self.b.push('}');
        self.b
    }
}

/// A string as JSON.
#[must_use]
pub fn string(s: &str) -> String {
    let mut b = String::new();
    write_string(&mut b, s);
    b
}

/// Strings as a JSON array (`null` for none, as Go writes a nil slice).
#[must_use]
pub fn strs(v: &[String]) -> String {
    let mut b = String::from("[");
    for (i, s) in v.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        write_string(&mut b, s);
    }
    b.push(']');
    b
}

/// JSON values written already, as an array.
#[must_use]
pub fn array<I: IntoIterator<Item = String>>(items: I) -> String {
    let mut b = String::from("[");
    for (i, s) in items.into_iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        b.push_str(&s);
    }
    b.push(']');
    b
}

/// A float64 as Go's `encoding/json` writes it: the shortest form, no exponent in the usual
/// range, `0` for NaN and the infinities (which Go refuses).
#[must_use]
pub fn float(v: f64) -> String {
    if !v.is_finite() {
        return "0".to_owned();
    }
    let s = format!("{v}");
    s.strip_suffix(".0").map_or(s.clone(), str::to_owned)
}

/// Parses `b`, which must be JSON (an empty payload or `null` is `Value::Null`).
///
/// # Errors
///
/// When `b` is not JSON.
pub fn parse(b: &[u8]) -> Result<Value, String> {
    let s = std::str::from_utf8(b).map_err(|e| e.to_string())?;
    if s.trim().is_empty() {
        return Ok(Value::Null);
    }
    akit_config::value::parse(s).map_err(|e| e.to_string())
}

/// Member `k` of an object as a string, `""` when it is not one.
#[must_use]
pub fn get_str(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// Member `k` of an object as an integer, 0 when it is not one.
#[must_use]
pub fn get_int(v: &Value, k: &str) -> i64 {
    v.get(k).map_or(0, int_of)
}

/// A value as an integer, 0 when it is not one (a float's integer part).
#[must_use]
pub fn int_of(v: &Value) -> i64 {
    match v {
        Value::Number(n) => n.parse::<i64>().ok().unwrap_or_else(|| {
            n.parse::<f64>()
                .ok()
                .filter(|f| f.is_finite())
                .map_or(0, |f| {
                    #[allow(clippy::cast_possible_truncation)]
                    let i = f as i64;
                    i
                })
        }),
        _ => 0,
    }
}

/// Member `k` of an object as a float, 0 when it is not one.
#[must_use]
pub fn get_float(v: &Value, k: &str) -> f64 {
    match v.get(k) {
        Some(Value::Number(n)) => n.parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Member `k` of an object as a boolean, false when it is not one.
#[must_use]
pub fn get_bool(v: &Value, k: &str) -> bool {
    matches!(v.get(k), Some(Value::Bool(true)))
}

/// Member `k` of an object as an array, empty when it is not one.
#[must_use]
pub fn get_arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    match v.get(k) {
        Some(Value::Array(a)) => a,
        _ => &[],
    }
}

/// Member `k` of an object as strings, the elements that are not strings left out.
#[must_use]
pub fn get_strs(v: &Value, k: &str) -> Vec<String> {
    get_arr(v, k)
        .iter()
        .filter_map(|e| match e {
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

/// A value written back as compact JSON.
#[must_use]
pub fn write(v: &Value) -> String {
    let mut b = String::new();
    write_into(&mut b, v);
    b
}

fn write_into(b: &mut String, v: &Value) {
    match v {
        Value::Null => b.push_str("null"),
        Value::Bool(t) => b.push_str(if *t { "true" } else { "false" }),
        Value::Number(n) => b.push_str(n),
        Value::String(s) => write_string(b, s),
        Value::Array(a) => {
            b.push('[');
            for (i, e) in a.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                write_into(b, e);
            }
            b.push(']');
        }
        Value::Object(ms) => {
            b.push('{');
            for (i, (k, e)) in ms.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                write_string(b, k);
                b.push(':');
                write_into(b, e);
            }
            b.push('}');
        }
    }
}

/// A method's request, read as fit's `decodeReq` reads one into its struct: an empty payload or
/// `null` is an empty request; anything but an object, or a member of the wrong type, is a bad
/// request (`bad request: ...`).
#[derive(Debug)]
pub struct Req(Value);

/// A bad request's error.
fn bad(msg: impl std::fmt::Display) -> crate::wire::Error {
    crate::mesh::bad_request(format!("bad request: {msg}"))
}

impl Req {
    /// Reads `b`.
    ///
    /// # Errors
    ///
    /// Not JSON, or not an object.
    pub fn parse(b: &[u8]) -> Result<Req, crate::wire::Error> {
        match parse(b).map_err(bad)? {
            Value::Null => Ok(Req(Value::Object(Vec::new()))),
            v @ Value::Object(_) => Ok(Req(v)),
            _ => Err(bad("the request is not an object")),
        }
    }

    /// The request as a value.
    #[must_use]
    pub fn value(&self) -> &Value {
        &self.0
    }

    fn member(&self, k: &str) -> Option<&Value> {
        self.0.get(k).filter(|v| **v != Value::Null)
    }

    /// Member `k`, a string, `""` when left out.
    ///
    /// # Errors
    ///
    /// Not a string.
    pub fn str(&self, k: &str) -> Result<String, crate::wire::Error> {
        match self.member(k) {
            None => Ok(String::new()),
            Some(Value::String(s)) => Ok(s.clone()),
            Some(_) => Err(bad(format!("{k} is not a string"))),
        }
    }

    /// Member `k`, an integer, 0 when left out.
    ///
    /// # Errors
    ///
    /// Not an integer.
    pub fn int(&self, k: &str) -> Result<i64, crate::wire::Error> {
        match self.member(k) {
            None => Ok(0),
            Some(Value::Number(n)) => n
                .parse::<i64>()
                .map_err(|_| bad(format!("{k} is not an integer"))),
            Some(_) => Err(bad(format!("{k} is not a number"))),
        }
    }

    /// Member `k`, a boolean, false when left out.
    ///
    /// # Errors
    ///
    /// Not a boolean.
    pub fn bool(&self, k: &str) -> Result<bool, crate::wire::Error> {
        match self.member(k) {
            None => Ok(false),
            Some(Value::Bool(b)) => Ok(*b),
            Some(_) => Err(bad(format!("{k} is not a boolean"))),
        }
    }

    /// Member `k`, an object of strings, empty when left out.
    ///
    /// # Errors
    ///
    /// Not an object of strings.
    pub fn str_map(&self, k: &str) -> Result<Vec<(String, String)>, crate::wire::Error> {
        match self.member(k) {
            None => Ok(Vec::new()),
            Some(Value::Object(ms)) => ms
                .iter()
                .map(|(mk, v)| match v {
                    Value::String(s) => Ok((mk.clone(), s.clone())),
                    _ => Err(bad(format!("{k}.{mk} is not a string"))),
                })
                .collect(),
            Some(_) => Err(bad(format!("{k} is not an object"))),
        }
    }

    /// Member `k` written back as JSON, `""` when left out.
    #[must_use]
    pub fn raw(&self, k: &str) -> String {
        self.member(k).map(write).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn objects_and_values() {
        let o = Obj::new()
            .str("role", "hub")
            .int("pid", 7)
            .str_omit("http_url", "")
            .bool("ok", true)
            .float("avg", 1.5)
            .float("n", 2.0)
            .end();
        assert_eq!(o, r#"{"role":"hub","pid":7,"ok":true,"avg":1.5,"n":2}"#);
        let v = parse(o.as_bytes()).unwrap();
        assert_eq!(get_str(&v, "role"), "hub");
        assert_eq!(get_int(&v, "pid"), 7);
        assert!(get_bool(&v, "ok"));
        assert!((get_float(&v, "avg") - 1.5).abs() < 1e-9);
        assert_eq!(write(&v), o);
        assert_eq!(parse(b"").unwrap(), Value::Null);
        assert!(parse(b"{").is_err());
    }
}
