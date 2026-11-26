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
