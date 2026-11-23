//! Problems found in a config, each with where it was found, written as fit writes them:
//! `file:line:col: path: message`.

use std::fmt;

/// One problem found in a config, and where it was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// The file the problem is in, when the config was read from a file.
    pub file: Option<String>,
    /// Where in the document the problem is, as in `listeners[0].protocols[1]`; empty for the
    /// document as a whole.
    pub path: String,
    /// The 1-based line the problem was found at; 0 when there is no position.
    pub line: u32,
    /// The 1-based column, counted in characters; 0 when there is no position.
    pub col: u32,
    /// What is wrong.
    pub msg: String,
}

impl Error {
    /// Whether this problem is a field or value fit accepts that akit does not support yet
    /// (decision 002): the config is fit's, and only akit falls short.
    #[must_use]
    pub fn is_not_supported_yet(&self) -> bool {
        self.msg.starts_with(NOT_SUPPORTED_YET)
    }
}

/// How every "fit accepts this, akit does not yet" message starts.
pub const NOT_SUPPORTED_YET: &str = "not supported yet";

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(file) = &self.file {
            write!(f, "{file}:")?;
            if self.line == 0 {
                f.write_str(" ")?;
            }
        }
        if self.line > 0 {
            write!(f, "{}:{}: ", self.line, self.col)?;
        }
        if !self.path.is_empty() {
            write!(f, "{}: ", self.path)?;
        }
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

/// Every problem found in a config, in the order found. Never empty when returned as an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Errors(pub Vec<Error>);

impl Errors {
    /// The problems.
    #[must_use]
    pub fn as_slice(&self) -> &[Error] {
        &self.0
    }

    /// Whether every problem is a "not supported yet" one: a valid fit config that akit cannot
    /// run yet.
    #[must_use]
    pub fn only_not_supported_yet(&self) -> bool {
        !self.0.is_empty() && self.0.iter().all(Error::is_not_supported_yet)
    }
}

impl fmt::Display for Errors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, e) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str("\n")?;
            }
            write!(f, "{e}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Errors {}

/// `path.name`, or `name` at the top.
pub(crate) fn join_field(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_owned()
    } else {
        format!("{path}.{name}")
    }
}

/// `path[i]`.
pub(crate) fn join_index(path: &str, i: usize) -> String {
    format!("{path}[{i}]")
}

/// `path["key"]`, for a map's key.
pub(crate) fn join_key(path: &str, key: &str) -> String {
    format!("{path}[{key:?}]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_as_fit() {
        let e = Error {
            file: Some("fit.json".into()),
            path: "listeners[0].name".into(),
            line: 3,
            col: 7,
            msg: "missing required field".into(),
        };
        assert_eq!(
            e.to_string(),
            "fit.json:3:7: listeners[0].name: missing required field"
        );
        let e = Error {
            file: Some("fit.json".into()),
            path: String::new(),
            line: 0,
            col: 0,
            msg: "x".into(),
        };
        assert_eq!(e.to_string(), "fit.json: x");
        assert_eq!(join_key("h", "a b"), "h[\"a b\"]");
    }
}
