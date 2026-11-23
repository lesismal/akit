#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! The fit config compatibility gate ([decision 002], `docs/compatibility.md`).
//!
//! Every template in fit's `templates/` (each format as one file and as a `conf.d` directory)
//! and every config of fit-examples, pinned in `fixtures/` at the commits `fixtures/PINS` names,
//! goes through akit's validation. Each must either be valid or fail only with
//! "not supported yet: `<path>`" naming a field fit accepts (`fit-fields.txt`). Anything else
//! (a parse error, a wrong type, a refused field fit accepts under another name) fails the gate.
//!
//! A fixture in a format akit has no reader for yet (.conf until I23, YAML until I24, `conf.d`
//! directories until I23) is neither: it is an expected outcome, "format not supported yet",
//! listed with every other outcome in the checked-in report ([`REPORT`]), which the gate's run
//! must reproduce line for line.
//!
//! This crate enumerates the fixtures and judges the outcomes; the validation itself is a
//! function handed to [`run_gate`]. The gate's test (`tests/gate.rs`) hands it akit-config's
//! readers (iteration I07b).
//!
//! [decision 002]: https://github.com/lesismal/akit/blob/main/docs/decisions/002-config-compatibility.md

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The message prefix of a field akit does not support yet ([decision 002]).
///
/// [decision 002]: https://github.com/lesismal/akit/blob/main/docs/decisions/002-config-compatibility.md
pub const NOT_SUPPORTED_YET: &str = "not supported yet: ";

/// What the report says of a fixture in a format akit has no reader for yet
/// ([`Finding::NoReader`]).
pub const FORMAT_NOT_SUPPORTED_YET: &str = "format not supported yet: ";

/// The gate's checked-in report, relative to [`compat_dir`]: one line per outcome, as
/// [`Report::render`] writes it. The gate fails when its report differs from this file.
pub const REPORT: &str = "gate-report.txt";

/// The template formats: the directory under `templates/` and its single file.
pub const TEMPLATE_FORMATS: [(&str, &str); 3] = [
    ("conf", "fit.conf"),
    ("json", "fit.json"),
    ("yaml", "fit.yaml"),
];

/// Environment variables the examples read secrets from, which validation reads as `fit run`
/// does. fit-examples' own test sets them to stand-ins (`examples_test.go`, `secrets`).
/// akit-config expands no variable yet, and every example is a .conf file, which it cannot read
/// before I23; I23 hands the gate's reads these stand-ins (best through a lookup the reader
/// takes, not the process environment, which `cargo test` shares between threads).
/// `make compat-gate` sets them meanwhile.
pub const EXAMPLE_SECRETS: [&str; 15] = [
    "ES_PASSWORD",
    "KAFKA_PASSWORD",
    "MYSQL_PASSWORD",
    "PG_PASSWORD",
    "RABBITMQ_PASSWORD",
    "NATS_PASSWORD",
    "PULSAR_TOKEN",
    "MQTT_PASSWORD",
    "INFLUX_TOKEN",
    "TD_PASSWORD",
    "MSSQL_PASSWORD",
    "DORIS_PASSWORD",
    "OPENAI_API_KEY",
    "QDRANT_API_KEY",
    "MILVUS_TOKEN",
];

/// Examples that validation cannot check outside a user's machine, with fit-examples' reason
/// (`examples_test.go`, `noValidate`): fit only adapts them.
pub const NO_VALIDATE: [(&str, &str); 1] =
    [("https", "its certificate and key files are the user's own")];

/// The directory of this crate: `tests/compat`.
pub fn compat_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The pinned fixtures: `tests/compat/fixtures`.
pub fn fixtures_dir() -> PathBuf {
    compat_dir().join("fixtures")
}

/// Where a fixture comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A template of fit's as one file.
    Template,
    /// A template of fit's as a `conf.d` directory of files by kind.
    TemplateDir,
    /// An example of fit-examples, its `fit.conf`.
    Example,
}

/// One config the gate validates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fixture {
    /// A short name: `templates/conf/fit.conf`, `templates/yaml/conf.d`, `examples/hello`.
    pub name: String,
    /// The file or directory handed to validation, as `fit validate -c` takes it.
    pub path: PathBuf,
    /// Where it comes from.
    pub kind: Kind,
}

/// The fixtures in `dir` (normally [`fixtures_dir`]): templates first, then examples, by name.
pub fn fixtures_in(dir: &Path) -> io::Result<Vec<Fixture>> {
    let mut out = Vec::new();
    let templates = dir.join("fit").join("templates");
    for (format, file) in TEMPLATE_FORMATS {
        let path = templates.join(format).join(file);
        require(&path, false)?;
        out.push(Fixture {
            name: format!("templates/{format}/{file}"),
            path,
            kind: Kind::Template,
        });
        let path = templates.join(format).join("conf.d");
        require(&path, true)?;
        out.push(Fixture {
            name: format!("templates/{format}/conf.d"),
            path,
            kind: Kind::TemplateDir,
        });
    }
    let mut examples = Vec::new();
    for entry in fs::read_dir(dir.join("fit-examples"))? {
        let entry = entry?;
        let conf = entry.path().join("fit.conf");
        if entry.file_type()?.is_dir() && conf.is_file() {
            let name = entry.file_name().to_string_lossy().into_owned();
            examples.push(Fixture {
                name: format!("examples/{name}"),
                path: conf,
                kind: Kind::Example,
            });
        }
    }
    examples.sort_by(|a, b| a.name.cmp(&b.name));
    out.extend(examples);
    Ok(out)
}

/// The pinned fixtures: [`fixtures_in`] of [`fixtures_dir`].
pub fn fixtures() -> io::Result<Vec<Fixture>> {
    fixtures_in(&fixtures_dir())
}

fn require(path: &Path, dir: bool) -> io::Result<()> {
    let ok = if dir { path.is_dir() } else { path.is_file() };
    if ok {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("missing fixture {}", path.display()),
        ))
    }
}

/// The commits the fixtures were copied from (`fixtures/PINS`), by repository.
pub fn pins() -> io::Result<BTreeMap<String, String>> {
    let text = fs::read_to_string(fixtures_dir().join("PINS"))?;
    Ok(text
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect())
}

/// One line of `fit-fields.txt`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    /// The path, in the inventory's notation (`routes[].handler<respond>.params.status`).
    pub path: String,
    /// The config's top-level key, or `<namespace>/<module>` for a module's params.
    pub section: String,
    /// The type, as the inventory writes it.
    pub ty: String,
    /// `required`, `deprecated`, both comma-separated, or `-`.
    pub flags: String,
}

/// Parses `fit-fields.txt`; a malformed line is an error naming it.
pub fn parse_fields(text: &str) -> Result<Vec<Field>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        let [path, section, ty, flags] = cols.as_slice() else {
            return Err(format!(
                "fit-fields.txt:{}: {} columns, want 4",
                n.saturating_add(1),
                cols.len()
            ));
        };
        out.push(Field {
            path: (*path).to_owned(),
            section: (*section).to_owned(),
            ty: (*ty).to_owned(),
            flags: (*flags).to_owned(),
        });
    }
    Ok(out)
}

/// The field inventory, `tests/compat/fit-fields.txt`.
pub fn fields() -> io::Result<Vec<Field>> {
    let text = fs::read_to_string(compat_dir().join("fit-fields.txt"))?;
    parse_fields(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// The paths of `supported.txt`: the fields akit supports.
pub fn supported() -> io::Result<BTreeSet<String>> {
    let text = fs::read_to_string(compat_dir().join("supported.txt"))?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect())
}

/// Splits a path into segments, `[n]` and `<module>` written as part of their segment.
fn segments(path: &str) -> Vec<String> {
    path.split('.').map(str::to_owned).collect()
}

/// A segment with array indices made `[]` and module tags dropped: `handler<respond>` and
/// `handler` match, `routes[3]` and `routes[]` match.
fn normal(seg: &str) -> String {
    let mut out = String::with_capacity(seg.len());
    let mut chars = seg.chars();
    while let Some(c) = chars.next() {
        match c {
            '<' => {
                for c in chars.by_ref() {
                    if c == '>' {
                        break;
                    }
                }
            }
            '[' => {
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                }
                out.push_str("[]");
            }
            _ => out.push(c),
        }
    }
    out
}

/// Whether a path a validator names matches an inventory path: the validator may write array
/// indices (`routes[0]`), map keys where the inventory has `*` (`log.loggers.engine`), and the
/// module tag or not (`routes[0].handler.params.body` matches
/// `routes[].handler<respond>.params.body`).
pub fn path_matches(inventory: &str, named: &str) -> bool {
    let a = segments(inventory);
    let b = segments(named);
    a.len() == b.len()
        && a.iter()
            .zip(&b)
            .all(|(p, q)| p == "*" || normal(p) == normal(q))
}

/// The inventory path a validator's path names, as [`path_matches`] matches them; when several
/// match (a path without a module tag matches each module's), the one with the same module tags
/// first, then the first in the inventory. A path to an element of an array of values names the
/// array (`listeners[0].protocols[1]`, a value fit accepts that akit does not support yet such as
/// the `h3` protocol, is `listeners[].protocols`). `None` when no field of the inventory matches.
pub fn inventory_path<'a>(inventory: &'a [Field], named: &str) -> Option<&'a str> {
    field_path(inventory, named).or_else(|| {
        let array = named.strip_suffix(']')?.rsplit_once('[')?.0;
        field_path(inventory, array).filter(|p| {
            inventory
                .iter()
                .any(|f| f.path == *p && f.ty.starts_with("array<") && !f.ty.contains("object"))
        })
    })
}

fn field_path<'a>(inventory: &'a [Field], named: &str) -> Option<&'a str> {
    let tags = |path: &str| -> Vec<String> {
        segments(path)
            .iter()
            .map(|s| {
                s.split_once('<')
                    .map(|(_, t)| t.to_owned())
                    .unwrap_or_default()
            })
            .collect()
    };
    let want = tags(named);
    let mut matching = inventory.iter().filter(|f| path_matches(&f.path, named));
    let first = matching.next()?;
    if tags(&first.path) == want {
        return Some(&first.path);
    }
    Some(
        matching
            .find(|f| tags(&f.path) == want)
            .map_or(first.path.as_str(), |f| f.path.as_str()),
    )
}

/// What a validator found wrong with a config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Finding {
    /// "not supported yet: `<path>`": a field akit does not support yet.
    Unsupported(String),
    /// A value akit does not support yet, of a field it reads (one of fit's protocols in
    /// `listeners[].protocols`), or a behaviour of fit's that the field's absence asks for (no
    /// `listeners`, which fit serves over HTTPS by default): `what` says which. Reported as
    /// `<field>: <what>`, so that it is not taken for the field itself being refused.
    UnsupportedValue {
        /// Where, as the validator names it.
        path: String,
        /// What is not supported yet.
        what: String,
    },
    /// Any other error.
    Error(String),
    /// The config is in a format akit has no reader for yet: YAML until I24 (the .conf format
    /// and `conf.d` directories are read since I23). Neither a pass nor a failure by itself: the gate's checked-in report ([`REPORT`]) lists each such fixture, so a fixture
    /// that starts being read, or stops, changes the report.
    NoReader(String),
}

impl Finding {
    /// Reads a validator's error message: [`Finding::Unsupported`] when it is
    /// "not supported yet: `<path>`" (backticks optional), [`Finding::Error`] otherwise. The
    /// message may be prefixed by a location (`file:line: `), as fit's are.
    pub fn from_message(msg: &str) -> Self {
        match msg.find(NOT_SUPPORTED_YET) {
            Some(i) => {
                let rest = msg
                    .get(i.saturating_add(NOT_SUPPORTED_YET.len())..)
                    .unwrap_or("");
                Self::Unsupported(rest.trim().trim_matches('`').to_owned())
            }
            None => Self::Error(msg.to_owned()),
        }
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(path) => write!(f, "{NOT_SUPPORTED_YET}{path}"),
            Self::UnsupportedValue { path, what } => {
                write!(f, "{NOT_SUPPORTED_YET}{path}: {what}")
            }
            Self::Error(msg) => f.write_str(msg),
            Self::NoReader(what) => write!(f, "{FORMAT_NOT_SUPPORTED_YET}{what}"),
        }
    }
}

/// How one fixture fared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Valid.
    Valid,
    /// Refused only for fields fit accepts and akit does not support yet: passes the gate. The
    /// fields are written as the inventory writes them (`routes[].handler<proxy>`).
    Unsupported(BTreeSet<String>),
    /// In a format akit has no reader for yet, with which ([`Finding::NoReader`]).
    NoReader(String),
    /// Anything else: fails the gate, with why.
    Failed(Vec<String>),
    /// Not validated, with fit-examples' reason ([`NO_VALIDATE`]).
    Skipped(&'static str),
}

/// The gate's result: each fixture's outcome, by name.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Each fixture's outcome, by fixture name.
    pub outcomes: BTreeMap<String, Outcome>,
}

impl Report {
    /// The fixtures that fail the gate, with why.
    pub fn failures(&self) -> Vec<(&str, &[String])> {
        self.outcomes
            .iter()
            .filter_map(|(name, o)| match o {
                Outcome::Failed(why) => Some((name.as_str(), why.as_slice())),
                _ => None,
            })
            .collect()
    }

    /// Every field refused as not supported yet, across the fixtures.
    pub fn unsupported(&self) -> BTreeSet<&str> {
        self.outcomes
            .values()
            .filter_map(|o| match o {
                Outcome::Unsupported(paths) => Some(paths.iter().map(String::as_str)),
                _ => None,
            })
            .flatten()
            .collect()
    }
}

impl Report {
    /// The report as checked in ([`REPORT`]): one line per outcome, `<fixture>: <outcome>`,
    /// and one line per field for a fixture refused as not supported yet, sorted, so that a
    /// change of any outcome is a changed line. Failures are written too, as `FAILED: <why>`,
    /// though a report with failures is never checked in.
    #[must_use]
    pub fn render(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (name, o) in &self.outcomes {
            match o {
                Outcome::Valid => out.push(format!("{name}: valid")),
                Outcome::Unsupported(paths) => {
                    out.extend(
                        paths
                            .iter()
                            .map(|p| format!("{name}: {NOT_SUPPORTED_YET}{p}")),
                    );
                }
                Outcome::NoReader(what) => {
                    out.push(format!("{name}: {FORMAT_NOT_SUPPORTED_YET}{what}"));
                }
                Outcome::Skipped(why) => out.push(format!("{name}: skipped: {why}")),
                Outcome::Failed(why) => {
                    out.extend(why.iter().map(|w| format!("{name}: FAILED: {w}")));
                }
            }
        }
        out
    }

    /// How this report differs from a checked-in one (`#` lines and blank lines ignored): each
    /// line only the checked-in report has, `- <line>`, then each only this one has, `+ <line>`.
    /// Empty when they agree.
    #[must_use]
    pub fn diff(&self, checked_in: &str) -> Vec<String> {
        let want: BTreeSet<&str> = checked_in
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        let rendered = self.render();
        let got: BTreeSet<&str> = rendered.iter().map(String::as_str).collect();
        want.difference(&got)
            .map(|l| format!("- {l}"))
            .chain(got.difference(&want).map(|l| format!("+ {l}")))
            .collect()
    }

    /// Counts of the outcomes, for the summary line: valid, not supported yet, no reader yet,
    /// skipped, failed.
    #[must_use]
    pub fn counts(&self) -> [usize; 5] {
        let mut c = [0usize; 5];
        for o in self.outcomes.values() {
            let i = match o {
                Outcome::Valid => 0,
                Outcome::Unsupported(_) => 1,
                Outcome::NoReader(_) => 2,
                Outcome::Skipped(_) => 3,
                Outcome::Failed(_) => 4,
            };
            if let Some(n) = c.get_mut(i) {
                *n = n.saturating_add(1);
            }
        }
        c
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for line in self.render() {
            writeln!(f, "{line}")?;
        }
        Ok(())
    }
}

/// Runs `validate` on every fixture and judges each outcome against the inventory: a fixture
/// passes when it is valid, or when every finding is "not supported yet" naming a field of
/// `inventory`. `validate` takes a file or a directory, as `fit validate -c` does.
pub fn run_gate<V>(fixtures: &[Fixture], inventory: &[Field], mut validate: V) -> Report
where
    V: FnMut(&Path) -> Result<(), Vec<Finding>>,
{
    let mut report = Report::default();
    for fx in fixtures {
        let skip = NO_VALIDATE.iter().find(|(n, _)| {
            fx.kind == Kind::Example && fx.name.strip_prefix("examples/") == Some(*n)
        });
        let outcome = match skip {
            Some((_, why)) => Outcome::Skipped(why),
            None => match validate(&fx.path) {
                Ok(()) => Outcome::Valid,
                Err(findings) => judge(findings, inventory),
            },
        };
        report.outcomes.insert(fx.name.clone(), outcome);
    }
    report
}

fn judge(findings: Vec<Finding>, inventory: &[Field]) -> Outcome {
    if findings.is_empty() {
        return Outcome::Failed(vec!["refused with no finding".to_owned()]);
    }
    let mut paths = BTreeSet::new();
    let mut why = Vec::new();
    let mut no_reader = None;
    for f in findings {
        match f {
            Finding::Unsupported(p) => match inventory_path(inventory, &p) {
                Some(i) => {
                    paths.insert(i.to_owned());
                }
                None => why.push(format!("{NOT_SUPPORTED_YET}{p}: not a field fit accepts")),
            },
            Finding::UnsupportedValue { path, what } => match inventory_path(inventory, &path) {
                Some(i) => {
                    paths.insert(format!("{i}: {what}"));
                }
                None => why.push(format!(
                    "{NOT_SUPPORTED_YET}{path}: {what}: not a field fit accepts"
                )),
            },
            Finding::Error(e) => why.push(e),
            Finding::NoReader(what) => no_reader = Some(what),
        }
    }
    if let Some(what) = no_reader {
        if why.is_empty() && paths.is_empty() {
            return Outcome::NoReader(what);
        }
        why.push(format!(
            "{FORMAT_NOT_SUPPORTED_YET}{what}, beside other findings"
        ));
    }
    if why.is_empty() {
        Outcome::Unsupported(paths)
    } else {
        Outcome::Failed(why)
    }
}
