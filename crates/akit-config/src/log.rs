//! The log files' settings (I81): `rotate`, how a log file is rotated and its rotated files
//! removed, as fit's `config.Rotate` (decision 033, fit's decision 034).

use std::time::Duration;

use crate::decode::Decoder;
use crate::json::{Kind, Node};

/// fit's default `max_bytes` and `max_total_bytes`: 1 GiB.
pub const DEFAULT_ROTATE_MAX_BYTES: u64 = 1 << 30;
/// fit's default `max_total_bytes`: 1 GiB.
pub const DEFAULT_ROTATE_MAX_TOTAL_BYTES: u64 = 1 << 30;
/// fit's default `max_age`: 720h, 30 days.
pub const DEFAULT_ROTATE_MAX_AGE: Duration = Duration::from_secs(720 * 3600);

const DAY: Duration = Duration::from_secs(86_400);

/// `rotate`: how a log file is rotated and how its rotated files are removed, as fit's
/// `config.Rotate`. The object being there turns rotation on; a field left out (`None`) takes its
/// default, and 0 turns that limit off. A file with no `rotate` is written as one file for as long
/// as akit runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rotate {
    /// `max_bytes`: how large a file may grow before it is rotated; 1 GiB when left out, 0 off.
    pub max_bytes: Option<u64>,
    /// `interval`: how long a file is written before it is rotated, at the next period boundary
    /// of the log's zone; zero (left out) off.
    pub interval: Duration,
    /// `max_age`: how long a rotated file is kept after it was last written; 720h when left
    /// out, 0 off.
    pub max_age: Option<Duration>,
    /// `max_files`: the most rotated files kept, newest first; none or 0 has no count limit.
    pub max_files: Option<u64>,
    /// `max_total_bytes`: the most the rotated files may total; 1 GiB when left out, 0 off.
    pub max_total_bytes: Option<u64>,
    /// `compress`: gzip a file when it is rotated; on when left out.
    pub compress: Option<bool>,
}

/// A `rotate` with its defaults filled in: what a file sink rotates and keeps by. Two outputs of
/// one path need settings that are equal (fit's `Rotate.Equal`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rotation {
    /// Rotate before a write that would take the file past this many bytes; 0 off.
    pub max_bytes: u64,
    /// Rotate at the first write in a later period of this length; zero off.
    pub interval: Duration,
    /// Remove rotated files last written longer ago than this; zero off.
    pub max_age: Duration,
    /// Keep at most this many rotated files; 0 off.
    pub max_files: u64,
    /// Keep the rotated files to at most this many bytes in all; 0 off.
    pub max_total_bytes: u64,
    /// Gzip a rotated file.
    pub compress: bool,
}

impl Rotation {
    /// Whether the settings bound anything at all.
    #[must_use]
    pub fn bounded(&self) -> bool {
        self.max_bytes > 0
            || !self.interval.is_zero()
            || !self.max_age.is_zero()
            || self.max_files > 0
            || self.max_total_bytes > 0
    }
}

impl Rotate {
    /// The settings, with fit's defaults for what is left out.
    #[must_use]
    pub fn resolved(&self) -> Rotation {
        Rotation {
            max_bytes: self.max_bytes.unwrap_or(DEFAULT_ROTATE_MAX_BYTES),
            interval: self.interval,
            max_age: self.max_age.unwrap_or(DEFAULT_ROTATE_MAX_AGE),
            max_files: self.max_files.unwrap_or(0),
            max_total_bytes: self
                .max_total_bytes
                .unwrap_or(DEFAULT_ROTATE_MAX_TOTAL_BYTES),
            compress: self.compress.unwrap_or(true),
        }
    }
}

/// The resolved settings of an output's `rotate`, `None` for none, as fit's `rotateOf`.
#[must_use]
pub fn rotation(r: Option<&Rotate>) -> Option<Rotation> {
    r.map(Rotate::resolved)
}

/// Whether two outputs' `rotate`s rotate and keep files alike (fit's `Rotate.Equal`): both
/// absent, or both there and equal once their defaults are filled in.
#[must_use]
pub fn same_rotation(a: Option<&Rotate>, b: Option<&Rotate>) -> bool {
    rotation(a) == rotation(b)
}

const ROTATE_FIELDS: &[&str] = &[
    "max_bytes",
    "interval",
    "max_age",
    "max_files",
    "max_total_bytes",
    "compress",
];

/// `rotate`, at `path`, with fit's checks (`checkRotate`): a negative limit, an `interval` under a
/// second, and one over 24h that is not a whole number of days are refused.
pub(crate) fn rotate(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Rotate> {
    let mut o = d.object(n, path)?;
    let bytes = |d: &mut Decoder<'_>, o: &mut crate::decode::Obj<'_>, key: &str| {
        o.take(key)
            .and_then(|v| d.int(v, &o.at(key), 0, i64::MAX))
            .and_then(|n| u64::try_from(n).ok())
    };
    let max_bytes = bytes(d, &mut o, "max_bytes");
    let max_files = bytes(d, &mut o, "max_files");
    let max_total_bytes = bytes(d, &mut o, "max_total_bytes");
    let interval = match o.take("interval") {
        None => Duration::ZERO,
        Some(v) => {
            let p = o.at("interval");
            let i = d.duration(v, &p).unwrap_or_default();
            if !i.is_zero() && i < Duration::from_secs(1) {
                d.error(
                    v.off,
                    &p,
                    format!(
                        "{} is too short: a file rotates every second or more",
                        crate::duration::format(i)
                    ),
                );
            } else if i > DAY && i.as_nanos().checked_rem(DAY.as_nanos()) != Some(0) {
                d.error(
                    v.off,
                    &p,
                    format!(
                        "{} is over 24h but not a whole number of days",
                        crate::duration::format(i)
                    ),
                );
            }
            i
        }
    };
    let max_age = o
        .take("max_age")
        .and_then(|v| d.duration(v, &o.at("max_age")));
    let compress = match o.take("compress") {
        None => None,
        Some(v) => match &v.kind {
            Kind::Bool(b) => Some(*b),
            other => {
                d.error(
                    v.off,
                    &o.at("compress"),
                    format!("expected a boolean, got {}", other.name()),
                );
                None
            }
        },
    };
    o.finish(d, ROTATE_FIELDS, &[]);
    Some(Rotate {
        max_bytes,
        interval,
        max_age,
        max_files,
        max_total_bytes,
        compress,
    })
}

/// Whether a log's `output` names a file, rather than standard output, standard error or
/// nothing (fit's `fileOutput`).
#[must_use]
pub fn is_file_output(out: &str) -> bool {
    !matches!(out, "" | "stdout" | "stderr")
}

/// The cleaned path of a file output, the key two outputs sharing one file are compared by, as
/// Go's `filepath.Clean`: repeated and trailing slashes, `.` and `..` resolved lexically.
#[must_use]
pub fn output_path(out: &str) -> String {
    let rooted = out.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for p in out.split('/') {
        match p {
            "" | "." => {}
            ".." => match parts.last() {
                Some(&last) if last != ".." => {
                    parts.pop();
                }
                _ if rooted => {}
                _ => parts.push(".."),
            },
            p => parts.push(p),
        }
    }
    let joined = parts.join("/");
    match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_owned(),
        (false, false) => joined,
    }
}

/// The name a `time_zone` stands for, as Go's `Location.String` names the zone fit loads for it:
/// `Local` for `local` or empty, `UTC` for `utc` (either in any case), else the name trimmed.
/// Two logs writing one file need the same.
#[must_use]
pub fn zone_name(tz: &str) -> String {
    let t = tz.trim();
    match t.to_ascii_lowercase().as_str() {
        "" | "local" => "Local".to_owned(),
        "utc" => "UTC".to_owned(),
        _ => t.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Rotate, output_path, same_rotation, zone_name};

    #[test]
    fn cleaned_paths() {
        for (p, want) in [
            ("/var/log/a.log", "/var/log/a.log"),
            ("/var//log/./a.log", "/var/log/a.log"),
            ("/var/log/x/../a.log", "/var/log/a.log"),
            ("logs/a.log", "logs/a.log"),
            ("./logs/a.log", "logs/a.log"),
            ("../a.log", "../a.log"),
            ("/../a.log", "/a.log"),
            ("a/..", "."),
        ] {
            assert_eq!(output_path(p), want, "{p}");
        }
    }

    #[test]
    fn equal_settings() {
        let explicit = Rotate {
            max_bytes: Some(1 << 30),
            compress: Some(true),
            ..Rotate::default()
        };
        assert!(same_rotation(Some(&Rotate::default()), Some(&explicit)));
        assert!(same_rotation(None, None));
        assert!(!same_rotation(None, Some(&Rotate::default())));
        let other = Rotate {
            max_files: Some(3),
            ..Rotate::default()
        };
        assert!(!same_rotation(Some(&Rotate::default()), Some(&other)));
        assert_eq!(zone_name(" UTC "), "UTC");
        assert_eq!(zone_name(""), zone_name("LOCAL"));
        assert_eq!(zone_name("Asia/Shanghai"), "Asia/Shanghai");
    }
}

/// fit's loggers, in fit's order (`config.Loggers`): the names `log.levels` and `log.loggers`
/// may give. akit writes through the first nine; the roles' (`llm`, `tunnel`, `webrtc`,
/// `sync`, `bittorrent`) are accepted and take their settings, for the tracks that bring them.
pub const LOGGERS: &[&str] = &[
    "server",
    "admin",
    "tls",
    "provider",
    "telemetry",
    "access_log",
    "plugin",
    "fastcgi",
    "engine",
    "llm",
    "tunnel",
    "webrtc",
    "sync",
    "bittorrent",
];

/// A level of akit's own log: `debug`, `info`, `warn` or `error`, least first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum LogLevel {
    /// `debug`.
    Debug,
    /// `info`, the default.
    #[default]
    Info,
    /// `warn`.
    Warn,
    /// `error`.
    Error,
}

impl LogLevel {
    /// The level a config names, if it is one.
    #[must_use]
    pub fn parse(s: &str) -> Option<LogLevel> {
        Some(match s {
            "debug" => LogLevel::Debug,
            "info" => LogLevel::Info,
            "warn" => LogLevel::Warn,
            "error" => LogLevel::Error,
            _ => return None,
        })
    }
}

/// `log`: akit's own log, what akit says about itself, as fit's `config.Log` (I82). Each message
/// comes from a logger and is written when its level is at least the logger's, in the format and
/// to the output of that logger, or of the log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Log {
    /// `level`: the least level of every logger `levels` and `loggers` do not set one for; info
    /// when left out.
    pub level: Option<LogLevel>,
    /// `levels`: some loggers' least levels, by name, in the order written.
    pub levels: Vec<(String, LogLevel)>,
    /// `output`: `"stdout"`, `"stderr"`, or a file's path, of every logger `loggers` does not
    /// set one for; empty (left out) splits debug and info to standard output and the rest to
    /// standard error.
    pub output: String,
    /// `format`: how messages are written; `line` when left out.
    pub format: Option<crate::LogFormat>,
    /// `time_zone`, as written: `local` (or empty), `utc`, or an IANA zone name.
    pub time_zone: String,
    /// `rotate`: how `output`'s file is rotated and kept; none when left out.
    pub rotate: Option<Rotate>,
    /// `loggers`: some loggers' own settings, by name, in the order written.
    pub loggers: Vec<(String, Logger)>,
}

/// One of `log.loggers`: what it leaves out is the log's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Logger {
    /// `level`.
    pub level: Option<LogLevel>,
    /// `output`, empty when left out.
    pub output: String,
    /// `format`.
    pub format: Option<crate::LogFormat>,
    /// `rotate`: its file's, when `output` names a file of its own.
    pub rotate: Option<Rotate>,
}

/// One file the log writes to, with the loggers writing there and its rotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOutput {
    /// The output as written.
    pub path: String,
    /// The loggers writing there, in fit's order.
    pub loggers: Vec<&'static str>,
    /// Its `rotate`.
    pub rotate: Option<Rotate>,
}

impl Log {
    fn logger(&self, name: &str) -> Option<&Logger> {
        self.loggers.iter().find(|(n, _)| n == name).map(|(_, l)| l)
    }

    /// The least level `logger` writes (fit's `LevelOf`): its own, or `levels`', or `level`, or
    /// info.
    #[must_use]
    pub fn level_of(&self, logger: &str) -> LogLevel {
        if let Some(l) = self.logger(logger).and_then(|l| l.level) {
            return l;
        }
        if let Some((_, l)) = self.levels.iter().find(|(n, _)| n == logger) {
            return *l;
        }
        self.level.unwrap_or_default()
    }

    /// Where `logger` writes (fit's `OutputOf`): its own output, or the log's; empty for the
    /// split between standard output and error.
    #[must_use]
    pub fn output_of(&self, logger: &str) -> &str {
        match self.logger(logger) {
            Some(l) if !l.output.is_empty() => &l.output,
            _ => &self.output,
        }
    }

    /// How `logger` writes (fit's `FormatOf`): its own format, or the log's, or `line`.
    #[must_use]
    pub fn format_of(&self, logger: &str) -> crate::LogFormat {
        self.logger(logger)
            .and_then(|l| l.format)
            .or(self.format)
            .unwrap_or(crate::LogFormat::Line)
    }

    /// How `logger`'s file is rotated (fit's `RotateOf`): its own `rotate` when it writes a file
    /// of its own, else the log's.
    #[must_use]
    pub fn rotate_of(&self, logger: &str) -> Option<&Rotate> {
        match self.logger(logger) {
            Some(l) if is_file_output(&l.output) && l.output != self.output => l.rotate.as_ref(),
            _ => self.rotate.as_ref(),
        }
    }

    /// The files the log writes to, each with its loggers and rotation (fit's `FileOutputs`).
    #[must_use]
    pub fn file_outputs(&self) -> Vec<FileOutput> {
        let mut outs: Vec<FileOutput> = Vec::new();
        for &name in LOGGERS {
            let out = self.output_of(name);
            if !is_file_output(out) {
                continue;
            }
            let key = output_path(out);
            match outs.iter_mut().find(|o| output_path(&o.path) == key) {
                Some(o) => o.loggers.push(name),
                None => outs.push(FileOutput {
                    path: out.to_owned(),
                    loggers: vec![name],
                    rotate: self.rotate_of(name).cloned(),
                }),
            }
        }
        outs
    }
}

const LOG_FIELDS: &[&str] = &[
    "level",
    "levels",
    "output",
    "format",
    "time_zone",
    "rotate",
    "loggers",
];
const LOGGER_FIELDS: &[&str] = &["level", "output", "format", "rotate"];

fn level_field(d: &mut Decoder<'_>, o: &mut crate::decode::Obj<'_>) -> Option<LogLevel> {
    let v = o.take("level")?;
    let p = o.at("level");
    let s = d.string(v, &p)?;
    let l = LogLevel::parse(&s);
    if l.is_none() {
        d.error(
            v.off,
            &p,
            format!("{s:?} is not one of \"debug\", \"info\", \"warn\", \"error\""),
        );
    }
    l
}

fn format_field(d: &mut Decoder<'_>, o: &mut crate::decode::Obj<'_>) -> Option<crate::LogFormat> {
    let v = o.take("format")?;
    let p = o.at("format");
    match d.string(v, &p)?.as_str() {
        "line" => Some(crate::LogFormat::Line),
        "text" => Some(crate::LogFormat::Text),
        "json" => Some(crate::LogFormat::Json),
        other => {
            d.error(
                v.off,
                &p,
                format!("{other:?} is not one of \"line\", \"text\", \"json\""),
            );
            None
        }
    }
}

fn no_logger(name: &str) -> String {
    format!(
        "no logger is named {name:?}: the loggers are {}",
        LOGGERS.join(", ")
    )
}

/// `log`, at `path`, with fit's checks (`checkLog`): an unknown logger, a level that is not one,
/// a logger's level given in `levels` too, `rotate`'s checks, and loggers sharing a file with
/// other rotation settings. Whether `time_zone` names a zone is checked when the log is opened,
/// as the access log's is.
pub(crate) fn log(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Log> {
    let mut o = d.object(n, path)?;
    let level = level_field(d, &mut o);
    let mut levels = Vec::new();
    if let Some(v) = o.take("levels") {
        let p = o.at("levels");
        for (off, name, value) in d.string_map(v, &p) {
            let at = crate::error::join_key(&p, &name);
            if !LOGGERS.contains(&name.as_str()) {
                d.error(off, &at, no_logger(&name));
                continue;
            }
            match LogLevel::parse(&value) {
                Some(l) => levels.push((name, l)),
                None => d.error(
                    off,
                    &at,
                    format!("{value:?} is not a level: debug, info, warn or error"),
                ),
            }
        }
    }
    let output = o
        .take("output")
        .and_then(|v| d.string(v, &o.at("output")))
        .unwrap_or_default();
    let format = format_field(d, &mut o);
    let time_zone = o
        .take("time_zone")
        .and_then(|v| d.string(v, &o.at("time_zone")))
        .unwrap_or_default();
    let rotate = o.take("rotate").and_then(|v| rotate(d, v, &o.at("rotate")));
    let mut loggers: Vec<(String, Logger, usize)> = Vec::new();
    if let Some(v) = o.take("loggers") {
        let p = o.at("loggers");
        match &v.kind {
            Kind::Object(members) => {
                for m in members {
                    let at = crate::error::join_key(&p, &m.key);
                    if !LOGGERS.contains(&m.key.as_str()) {
                        d.error(m.key_off, &at, no_logger(&m.key));
                        continue;
                    }
                    let Some(mut lo) = d.object(&m.value, &at) else {
                        continue;
                    };
                    let level = level_field(d, &mut lo);
                    if level.is_some() && levels.iter().any(|(n, _)| *n == m.key) {
                        d.error(
                            m.value.off,
                            &crate::error::join_field(&at, "level"),
                            format!(
                                "logger {} has a level in levels too: set it in one place",
                                m.key
                            ),
                        );
                    }
                    let output = lo
                        .take("output")
                        .and_then(|v| d.string(v, &lo.at("output")))
                        .unwrap_or_default();
                    let format = format_field(d, &mut lo);
                    let own_rotate = lo
                        .take("rotate")
                        .and_then(|v| self::rotate(d, v, &lo.at("rotate")));
                    lo.finish(d, LOGGER_FIELDS, &[]);
                    loggers.push((
                        m.key.clone(),
                        Logger {
                            level,
                            output,
                            format,
                            rotate: own_rotate,
                        },
                        m.value.off,
                    ));
                }
            }
            other => d.error(
                v.off,
                &p,
                format!("expected an object, got {}", other.name()),
            ),
        }
    }
    o.finish(d, LOG_FIELDS, &[]);
    // The outputs sharing a file need one rotation: two loggers writing to one file cannot
    // each rotate it on their own (fit's note, in its order: the log's output, then the loggers
    // by name).
    let mut shared: Vec<(String, Option<&Rotate>)> = Vec::new();
    if is_file_output(&output) {
        shared.push((output_path(&output), rotate.as_ref()));
    }
    let mut by_name: Vec<&(String, Logger, usize)> = loggers.iter().collect();
    by_name.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, l, off) in by_name {
        if !is_file_output(&l.output) {
            continue;
        }
        let key = output_path(&l.output);
        match shared.iter().find(|(k, _)| *k == key) {
            Some((_, prev)) => {
                if !same_rotation(*prev, l.rotate.as_ref()) {
                    let at = crate::error::join_field(
                        &crate::error::join_key(&o_path(path, "loggers"), name),
                        "output",
                    );
                    d.error(
                        *off,
                        &at,
                        format!(
                            "logger {name} writes the log's other loggers' file {:?} with other rotation settings: a file has one rotate",
                            l.output
                        ),
                    );
                }
            }
            None => shared.push((key, l.rotate.as_ref())),
        }
    }
    Some(Log {
        level,
        levels,
        output,
        format,
        time_zone,
        rotate,
        loggers: loggers.into_iter().map(|(n, l, _)| (n, l)).collect(),
    })
}

fn o_path(path: &str, key: &str) -> String {
    crate::error::join_field(path, key)
}

/// Whether the access log writing `access`'s output shares a file of `log`'s with other rotation
/// settings or another zone (fit's `checkLogFileSharing`): the problems, each with its field.
#[must_use]
pub fn sharing_problems(
    log: Option<&Log>,
    access: &crate::AccessLog,
) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let Some(log) = log else { return out };
    if !is_file_output(&access.output) {
        return out;
    }
    let key = output_path(&access.output);
    for f in log.file_outputs() {
        if output_path(&f.path) != key {
            continue;
        }
        if !same_rotation(f.rotate.as_ref(), access.rotate.as_ref()) {
            out.push((
                "rotate",
                format!(
                    "the access log writes the log's file {:?} with other rotation settings: a file has one rotate",
                    access.output
                ),
            ));
        }
        let (a, l) = (zone_name(&access.time_zone), zone_name(&log.time_zone));
        if a != l {
            out.push((
                "time_zone",
                format!(
                    "the access log writes the log's file {:?} in {a}, not the log's {l}: a file turns at one boundary",
                    access.output
                ),
            ));
        }
    }
    out
}
