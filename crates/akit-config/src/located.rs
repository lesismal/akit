//! A YAML document read with the line each of its values and keys is at (I164): what a skill's
//! front matter is checked against, each refusal placed at its line as fit's `yaml.Node` places
//! it. Scalars are resolved as the config's YAML resolves them (YAML 1.2's core schema).

use crate::Error;
use crate::json::{Kind, Node, position};

/// A value, and the 1-based line it is at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    /// The line.
    pub line: u32,
    /// The value.
    pub value: Value,
}

/// What a [`Located`] holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// `null`, or nothing.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number, as written.
    Number(String),
    /// A string.
    String(String),
    /// A sequence.
    Array(Vec<Located>),
    /// A mapping: its keys, each with its line, and their values, in their order.
    Object(Vec<(String, u32, Located)>),
}

/// A YAML document read: its value, and the line of its first alias when it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Yaml {
    /// The value; an empty document, or one of comments alone, is an empty mapping.
    pub value: Located,
    /// The line of the first alias (`*name`).
    pub alias: Option<u32>,
}

/// Reads `src`, one YAML document.
///
/// # Errors
///
/// What makes `src` not YAML, at its line and column; a key given twice is one.
pub fn yaml(src: &str) -> Result<Yaml, Error> {
    let (node, alias) = crate::yaml::parse_noting_aliases(src).map_err(|e| {
        let (line, col) = position(src, e.off);
        Error {
            file: None,
            path: String::new(),
            line,
            col,
            msg: e.msg,
        }
    })?;
    Ok(Yaml {
        value: located(src, &node),
        alias: alias.map(|off| position(src, off).0),
    })
}

fn located(src: &str, n: &Node) -> Located {
    let value = match &n.kind {
        Kind::Null => Value::Null,
        Kind::Bool(b) => Value::Bool(*b),
        Kind::Number(s) => Value::Number(s.clone()),
        Kind::Str(s) => Value::String(s.clone()),
        Kind::Array(a) => Value::Array(a.iter().map(|x| located(src, x)).collect()),
        Kind::Object(ms) => Value::Object(
            ms.iter()
                .map(|m| {
                    (
                        m.key.clone(),
                        position(src, m.key_off).0,
                        located(src, &m.value),
                    )
                })
                .collect(),
        ),
    };
    Located {
        line: position(src, n.off).0,
        value,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_aliases() {
        let y = yaml("name: a\ntools: [x, y]\nn: 3\nr: true\nt:\nd: &v w\ne: *v\n").unwrap();
        let Value::Object(ms) = &y.value.value else {
            panic!("{y:?}")
        };
        assert_eq!(ms[0].0, "name");
        assert_eq!((ms[1].1, ms[1].2.line), (2, 2));
        let Value::Array(items) = &ms[1].2.value else {
            panic!()
        };
        assert_eq!(items[1].value, Value::String("y".into()));
        assert_eq!(ms[2].2.value, Value::Number("3".into()));
        assert_eq!(ms[3].2.value, Value::Bool(true));
        assert_eq!(ms[4].2.value, Value::Null);
        assert_eq!(y.alias, Some(7));
        assert_eq!(
            yaml("# c\n").unwrap().value.value,
            Value::Object(Vec::new())
        );
        let e = yaml("a: 1\na: 2\n").unwrap_err();
        assert_eq!(e.line, 2);
    }
}
