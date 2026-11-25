//! Turning a parsed JSON tree into the config's types: every problem is collected with its path
//! and position, and decoding goes on past it, so that one run reports them all, as fit does.
//!
//! An object's members are checked against two lists: the fields akit reads, and the fields fit
//! reads that akit does not support yet (decision 002). Any other member is an unknown field.

use crate::error::{Error, NOT_SUPPORTED_YET, join_field, join_index, join_key};
use crate::json::{Kind, Member, Node, position};

/// Collects the problems of one document.
#[derive(Debug)]
pub(crate) struct Decoder<'s> {
    src: &'s str,
    file: Option<String>,
    pub(crate) errors: Vec<Error>,
    /// What is read but said (fit's `Config.Warnings`): an old name of a renamed field.
    pub(crate) warnings: Vec<Error>,
}

impl<'s> Decoder<'s> {
    pub(crate) fn new(src: &'s str, file: Option<&str>) -> Decoder<'s> {
        Decoder {
            src,
            file: file.map(str::to_owned),
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// The file named in its problems.
    pub(crate) fn file(&self) -> Option<&str> {
        self.file.as_deref()
    }

    /// Records a problem at byte `off`, for the value at `path`.
    pub(crate) fn error(&mut self, off: usize, path: &str, msg: impl Into<String>) {
        let (line, col) = position(self.src, off);
        self.errors.push(Error {
            file: self.file.clone(),
            path: path.to_owned(),
            line,
            col,
            msg: msg.into(),
        });
    }

    /// Records that the field at `path`, its key at byte `off`, is the old name of `new`, a field
    /// renamed in schema version 1 (fit's `renamed` tag): read as `new`, and said, as fit says
    /// it.
    pub(crate) fn renamed(&mut self, off: usize, path: &str, new: &str) {
        let (line, col) = position(self.src, off);
        self.warnings.push(Error {
            file: self.file.clone(),
            path: path.to_owned(),
            line,
            col,
            msg: crate::error::renamed(new),
        });
    }

    /// Records that fit accepts what is at `path` and akit does not yet; `what` says more when
    /// the path alone does not (a value rather than a field).
    pub(crate) fn not_yet(&mut self, off: usize, path: &str, what: &str) {
        let msg = if what.is_empty() {
            NOT_SUPPORTED_YET.to_owned()
        } else {
            format!("{NOT_SUPPORTED_YET}: {what}")
        };
        self.error(off, path, msg);
    }

    fn expected(&mut self, n: &Node, path: &str, what: &str) {
        self.error(
            n.off,
            path,
            format!("expected {what}, got {}", n.kind.name()),
        );
    }

    /// The members of an object, to be taken one by one and then checked by [`Obj::finish`].
    pub(crate) fn object<'n>(&mut self, n: &'n Node, path: &str) -> Option<Obj<'n>> {
        match &n.kind {
            Kind::Object(members) => Some(Obj {
                off: n.off,
                path: path.to_owned(),
                members,
                used: vec![false; members.len()],
            }),
            _ => {
                self.expected(n, path, "an object");
                None
            }
        }
    }

    pub(crate) fn string(&mut self, n: &Node, path: &str) -> Option<String> {
        match &n.kind {
            Kind::Str(s) => Some(s.clone()),
            _ => {
                self.expected(n, path, "a string");
                None
            }
        }
    }

    /// An integer within `min..=max`, as fit's `min` and `max` tags bound it.
    pub(crate) fn int(&mut self, n: &Node, path: &str, min: i64, max: i64) -> Option<i64> {
        let Kind::Number(raw) = &n.kind else {
            self.expected(n, path, "an integer");
            return None;
        };
        let Ok(v) = raw.parse::<i64>() else {
            self.error(n.off, path, format!("{raw} is not an integer in range"));
            return None;
        };
        if v < min {
            self.error(n.off, path, format!("{v} is less than the minimum {min}"));
            return None;
        }
        if v > max {
            self.error(
                n.off,
                path,
                format!("{v} is greater than the maximum {max}"),
            );
            return None;
        }
        Some(v)
    }

    /// A number, as fit's `float64` fields take it: any JSON number.
    pub(crate) fn number(&mut self, n: &Node, path: &str) -> Option<f64> {
        let Kind::Number(raw) = &n.kind else {
            self.expected(n, path, "a number");
            return None;
        };
        match raw.parse::<f64>() {
            Ok(v) if v.is_finite() => Some(v),
            _ => {
                self.error(n.off, path, format!("{raw} is not a number in range"));
                None
            }
        }
    }

    /// A duration, written as a string Go's `time.ParseDuration` takes (`"10s"`, `"1m30s"`,
    /// `"250ms"`), not negative: fit's `config.Duration`.
    pub(crate) fn duration(&mut self, n: &Node, path: &str) -> Option<std::time::Duration> {
        let s = self.string(n, path)?;
        match crate::duration::parse(&s) {
            Some(Ok(d)) => Some(d),
            Some(Err(())) => {
                self.error(n.off, path, format!("{s:?} is negative"));
                None
            }
            None => {
                self.error(
                    n.off,
                    path,
                    format!("{s:?} is not a duration, such as \"10s\" or \"1m30s\""),
                );
                None
            }
        }
    }

    /// An array, each element read by `f` with its index's path.
    pub(crate) fn array<T>(
        &mut self,
        n: &Node,
        path: &str,
        mut f: impl FnMut(&mut Self, &Node, &str) -> Option<T>,
    ) -> Vec<T> {
        let Kind::Array(items) = &n.kind else {
            self.expected(n, path, "an array");
            return Vec::new();
        };
        let mut out = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            if let Some(v) = f(self, item, &join_index(path, i)) {
                out.push(v);
            }
        }
        out
    }

    /// An array of strings.
    pub(crate) fn strings(&mut self, n: &Node, path: &str) -> Vec<(usize, String)> {
        self.array(n, path, |d, item, p| {
            d.string(item, p).map(|s| (item.off, s))
        })
    }

    /// An object of string values, in the order written (keys are unique: the reader refuses a
    /// key given twice).
    pub(crate) fn string_map(&mut self, n: &Node, path: &str) -> Vec<(usize, String, String)> {
        let Kind::Object(members) = &n.kind else {
            self.expected(n, path, "an object");
            return Vec::new();
        };
        members
            .iter()
            .filter_map(|m| {
                let v = self.string(&m.value, &join_key(path, &m.key))?;
                Some((m.key_off, m.key.clone(), v))
            })
            .collect()
    }
}

/// An object being decoded: members are taken by name, and [`finish`](Obj::finish) reports the
/// ones nobody took.
#[derive(Debug)]
pub(crate) struct Obj<'n> {
    pub(crate) off: usize,
    pub(crate) path: String,
    members: &'n [Member],
    used: Vec<bool>,
}

impl<'n> Obj<'n> {
    /// The member named `key`, if present; it counts as read.
    pub(crate) fn take(&mut self, key: &str) -> Option<&'n Node> {
        let i = self.members.iter().position(|m| m.key == key)?;
        if let Some(u) = self.used.get_mut(i) {
            *u = true;
        }
        self.members.get(i).map(|m| &m.value)
    }

    /// Whether there is a member named `key`; it does not count as read.
    pub(crate) fn has(&self, key: &str) -> bool {
        self.members.iter().any(|m| m.key == key)
    }

    /// Where the key of the member named `key` is, if present: fit reports an old name there.
    pub(crate) fn key_off(&self, key: &str) -> Option<usize> {
        self.members
            .iter()
            .find(|m| m.key == key)
            .map(|m| m.key_off)
    }

    /// The member named `key`, or a "missing required field" problem.
    pub(crate) fn require(&mut self, d: &mut Decoder<'_>, key: &str) -> Option<&'n Node> {
        let v = self.take(key);
        if v.is_none() {
            d.error(
                self.off,
                &join_field(&self.path, key),
                "missing required field",
            );
        }
        v
    }

    /// The path of member `key`.
    pub(crate) fn at(&self, key: &str) -> String {
        join_field(&self.path, key)
    }

    /// Reports every member not taken: "not supported yet" for a field fit reads (`fit_only`),
    /// "unknown field" for the rest, suggesting a field of `known` that differs only in case.
    pub(crate) fn finish(self, d: &mut Decoder<'_>, known: &[&str], fit_only: &[&str]) {
        for (m, used) in self.members.iter().zip(&self.used) {
            if *used {
                continue;
            }
            let path = join_field(&self.path, &m.key);
            if fit_only.contains(&m.key.as_str()) {
                d.not_yet(m.key_off, &path, "");
            } else {
                let suggestion = known
                    .iter()
                    .chain(fit_only)
                    .find(|k| k.eq_ignore_ascii_case(&m.key))
                    .map(|k| format!(" (did you mean {k:?}?)"))
                    .unwrap_or_default();
                d.error(m.key_off, &path, format!("unknown field{suggestion}"));
            }
        }
    }
}
