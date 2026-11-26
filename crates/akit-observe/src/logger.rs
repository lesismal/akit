//! akit's own log (I82): what akit says about itself, as opposed to the access log's line for
//! each request, as fit's `logs.Root`.
//!
//! Messages come from loggers, named as fit names them ([`Logger`]): `server`, `tls`,
//! `provider` and the rest, and `engine`, which takes what akio writes (its `akio::log` records,
//! [`bridge_akio`]). Each logger has a least level, which a reload may change, and a format and
//! an output of its own or the log's: fit's (fib's) `line`, `text` and `json`, each carrying the
//! logger's name as `logger`, the time in the log's zone. A message below its logger's level is
//! not formatted at all. With no output set, debug and info go to standard output and warn and
//! error to standard error; a file is written through the sink [`Files`] keeps for its path,
//! shared with the access log when it writes there too (I81).
//!
//! The process has one log, [`install`]ed by `akit run`; until then, and after
//! [`Root::close`], a message of info or above is written to standard error as `akit: <message>`,
//! as fit's default logger writes `fit: <message>`. A message is written by the thread that logs
//! it: akit logs about starting, reloading, certificates and providers, not about each request.

use std::fmt::{self, Write as _};
use std::io::{self, Write};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use akit_config::LogFormat;

use crate::files::{Files, LogFile};
use crate::format::{json_string, logfmt};
use crate::time::{self, Stamp};
use crate::zone::Zone;

pub use akit_config::LogLevel as Level;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// fit's loggers, in fit's order (`config.Loggers`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum Logger {
    Server,
    Admin,
    Tls,
    Provider,
    Telemetry,
    AccessLog,
    Plugin,
    Fastcgi,
    Engine,
    Llm,
    Tunnel,
    Webrtc,
    Sync,
    Bittorrent,
}

impl Logger {
    /// Every logger, in fit's order: [`akit_config::LOGGERS`]'s.
    pub const ALL: [Logger; 14] = [
        Logger::Server,
        Logger::Admin,
        Logger::Tls,
        Logger::Provider,
        Logger::Telemetry,
        Logger::AccessLog,
        Logger::Plugin,
        Logger::Fastcgi,
        Logger::Engine,
        Logger::Llm,
        Logger::Tunnel,
        Logger::Webrtc,
        Logger::Sync,
        Logger::Bittorrent,
    ];

    /// The logger's name, as a config gives it.
    #[must_use]
    pub fn name(self) -> &'static str {
        akit_config::LOGGERS
            .get(self.index())
            .copied()
            .unwrap_or("engine")
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// Where one logger's messages go.
enum Out {
    /// Debug and info to standard output, the rest to standard error.
    Split,
    Stdout,
    Stderr,
    /// The file the log's sink of this index writes.
    File(usize),
}

struct State {
    level: AtomicU8,
    format: LogFormat,
    out: Out,
}

fn level_byte(l: Level) -> u8 {
    match l {
        Level::Debug => 0,
        Level::Info => 1,
        Level::Warn => 2,
        Level::Error => 3,
    }
}

/// A log: where its loggers' messages go. Safe for concurrent use.
pub struct Root {
    loggers: Vec<State>,
    zone: RwLock<(Arc<Zone>, String)>,
    files: Files,
    /// The file sinks the log writes through, by cleaned path; emptied when it closes.
    sinks: Mutex<Vec<(String, Arc<LogFile>)>>,
}

impl fmt::Debug for Root {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Root")
            .field("zone", &self.zone_name())
            .finish_non_exhaustive()
    }
}

impl Root {
    /// The log `c` describes (fit's defaults when `None`), its times in `zone` (the one
    /// `time_zone` names, loaded by the caller), its files opened through `files`. Opens files:
    /// control plane only.
    ///
    /// # Errors
    ///
    /// When a file cannot be opened, or is open with other rotation settings.
    pub fn new(c: Option<&akit_config::Log>, zone: Zone, files: &Files) -> io::Result<Root> {
        let default = akit_config::Log::default();
        let c = c.unwrap_or(&default);
        let mut sinks: Vec<(String, Arc<LogFile>)> = Vec::new();
        let mut loggers = Vec::with_capacity(Logger::ALL.len());
        for l in Logger::ALL {
            let name = l.name();
            let out = match c.output_of(name) {
                "" => Out::Split,
                "stdout" => Out::Stdout,
                "stderr" => Out::Stderr,
                path => {
                    let key = akit_config::output_path(path);
                    let i = match sinks.iter().position(|(k, _)| *k == key) {
                        Some(i) => i,
                        None => {
                            let f = files.open(
                                path,
                                akit_config::rotation(c.rotate_of(name)),
                                zone.clone(),
                            )?;
                            sinks.push((key, Arc::new(f)));
                            sinks.len().saturating_sub(1)
                        }
                    };
                    Out::File(i)
                }
            };
            loggers.push(State {
                level: AtomicU8::new(level_byte(c.level_of(name))),
                format: c.format_of(name),
                out,
            });
        }
        let zone_name = akit_config::zone_name(&c.time_zone);
        Ok(Root {
            loggers,
            zone: RwLock::new((Arc::new(zone), zone_name)),
            files: files.clone(),
            sinks: Mutex::new(sinks),
        })
    }

    /// The set of files the log writes through, shared with the access log.
    #[must_use]
    pub fn files(&self) -> &Files {
        &self.files
    }

    /// The name of the zone the log writes its times in: `Local`, `UTC` or an IANA name.
    #[must_use]
    pub fn zone_name(&self) -> String {
        self.zone
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .1
            .clone()
    }

    /// Whether `logger` writes a message at `level`.
    #[must_use]
    pub fn enabled(&self, logger: Logger, level: Level) -> bool {
        self.loggers
            .get(logger.index())
            .is_some_and(|s| level_byte(level) >= s.level.load(Ordering::Relaxed))
    }

    /// The least level `logger` writes.
    #[must_use]
    pub fn level(&self, logger: Logger) -> Level {
        match self
            .loggers
            .get(logger.index())
            .map_or(1, |s| s.level.load(Ordering::Relaxed))
        {
            0 => Level::Debug,
            1 => Level::Info,
            2 => Level::Warn,
            _ => Level::Error,
        }
    }

    /// Has each logger write from the level `c` gives it on, as a reload does.
    pub fn set_levels(&self, c: Option<&akit_config::Log>) {
        let default = akit_config::Log::default();
        let c = c.unwrap_or(&default);
        for (l, s) in Logger::ALL.iter().zip(&self.loggers) {
            s.level
                .store(level_byte(c.level_of(l.name())), Ordering::Relaxed);
        }
        sync_akio_level(self);
    }

    /// Has the log write its times in `zone`, named by `time_zone` as written, from now on, and
    /// its files turn in it.
    pub fn set_zone(&self, zone: Zone, time_zone: &str) {
        for (_, f) in lock(&self.sinks).iter() {
            f.set_zone(zone.clone());
        }
        *self.zone.write().unwrap_or_else(PoisonError::into_inner) =
            (Arc::new(zone), akit_config::zone_name(time_zone));
    }

    /// The file sinks the log writes through, by their cleaned paths.
    #[must_use]
    pub fn sinks(&self) -> Vec<(String, Arc<LogFile>)> {
        lock(&self.sinks).clone()
    }

    /// Lets go of the log's files, closing those no other log writes. Messages logged afterwards
    /// go to standard error. Waits for the files' writers: control plane only.
    pub fn close(&self) {
        let sinks = std::mem::take(&mut *lock(&self.sinks));
        drop(sinks);
    }

    /// Writes `args`, a message of `logger`'s at `level`, if its level lets it.
    pub fn write(&self, logger: Logger, level: Level, args: fmt::Arguments<'_>) {
        self.write_kv(logger, level, args, &[]);
    }

    /// Writes `args` with the attributes `kvs` after the logger's name, if its level lets it.
    pub fn write_kv(
        &self,
        logger: Logger,
        level: Level,
        args: fmt::Arguments<'_>,
        kvs: &[(&str, Attr<'_>)],
    ) {
        if !self.enabled(logger, level) {
            return;
        }
        let Some(s) = self.loggers.get(logger.index()) else {
            return;
        };
        let now = SystemTime::now();
        let zone = Arc::clone(&self.zone.read().unwrap_or_else(PoisonError::into_inner).0);
        let mut b = Vec::with_capacity(256);
        format(
            &mut b,
            s.format,
            now,
            &zone,
            level,
            args,
            logger.name(),
            kvs,
        );
        match &s.out {
            Out::File(i) => {
                let f = lock(&self.sinks).get(*i).map(|(_, f)| Arc::clone(f));
                match f {
                    // A full queue drops the message and counts it, as it does the access
                    // log's.
                    Some(f) => {
                        let _ = f.write_owned(b);
                    }
                    // Closed: standard error, as fit's.
                    None => write_std(&mut io::stderr().lock(), &b),
                }
            }
            Out::Stderr => write_std(&mut io::stderr().lock(), &b),
            Out::Stdout => write_std(&mut io::stdout().lock(), &b),
            Out::Split if level >= Level::Warn => write_std(&mut io::stderr().lock(), &b),
            Out::Split => write_std(&mut io::stdout().lock(), &b),
        }
    }
}

fn write_std(w: &mut dyn Write, b: &[u8]) {
    let _ = w.write_all(b).and_then(|()| w.flush());
}

/// The level's name as fib's handlers write it.
fn level_name(l: Level) -> &'static str {
    match l {
        Level::Debug => "DEBUG",
        Level::Info => "INFO",
        Level::Warn => "WARN",
        Level::Error => "ERROR",
    }
}

/// Appends `s` with its control characters but tabs escaped, as fib's `appendEscaped`.
fn escaped(b: &mut Vec<u8>, s: &str) {
    for &c in s.as_bytes() {
        if (c < b' ' && c != b'\t') || c == 0x7f {
            b.extend_from_slice(format!("\\x{c:02x}").as_bytes());
        } else {
            b.push(c);
        }
    }
}

/// Appends `secs` and `nanos` since the epoch at `offset` as Go's `time.RFC3339Nano`: the
/// fraction's trailing zeros dropped, none when it is zero, as log/slog's JSON writes a time.
fn rfc3339_nano(b: &mut Vec<u8>, secs: i64, nanos: u32, offset: i32) {
    let start = b.len();
    time::rfc3339(b, Stamp { secs, millis: 0 }, offset);
    // `…:05.000Z` or `…:05.000+08:00`: the fraction goes where `.000` is.
    let frac_at = start.saturating_add(19);
    let tail: Vec<u8> = b
        .get(frac_at.saturating_add(4)..)
        .unwrap_or_default()
        .to_vec();
    b.truncate(frac_at);
    if nanos > 0 {
        let mut digits = format!("{nanos:09}");
        while digits.ends_with('0') {
            digits.pop();
        }
        b.push(b'.');
        b.extend_from_slice(digits.as_bytes());
    }
    b.extend_from_slice(&tail);
}

/// An attribute's value: a string, or a number or boolean, which `json` writes bare.
#[derive(Clone, Copy)]
pub enum Attr<'a> {
    /// Quoted in `json`.
    Str(&'a dyn fmt::Display),
    /// A number or a boolean, bare in `json`.
    Bare(&'a dyn fmt::Display),
}

impl fmt::Debug for Attr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Attr::Str(v) | Attr::Bare(v) => write!(f, "{v}"),
        }
    }
}

/// Appends one message in `format`: fib's `line`, or log/slog's `text` or `json`.
#[allow(clippy::too_many_arguments)]
fn format(
    b: &mut Vec<u8>,
    format: LogFormat,
    now: SystemTime,
    zone: &Zone,
    level: Level,
    args: fmt::Arguments<'_>,
    logger: &str,
    kvs: &[(&str, Attr<'_>)],
) {
    let t = Stamp::of(now);
    let nanos = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let offset = zone.offset(t.secs);
    let msg = match args.as_str() {
        Some(s) => std::borrow::Cow::Borrowed(s),
        None => std::borrow::Cow::Owned(args.to_string()),
    };
    let mut value = String::new();
    match format {
        LogFormat::Line => {
            time::line(b, t, offset);
            b.push(b' ');
            let l = level_name(level);
            b.extend_from_slice(l.as_bytes());
            for _ in l.len()..6 {
                b.push(b' ');
            }
            escaped(b, &msg);
            b.extend_from_slice(b" logger=");
            logfmt(b, logger);
            for (k, v) in kvs {
                value.clear();
                let _ = write!(value, "{v:?}");
                b.push(b' ');
                logfmt(b, k);
                b.push(b'=');
                logfmt(b, &value);
            }
        }
        LogFormat::Text => {
            b.extend_from_slice(b"time=");
            time::rfc3339(b, t, offset);
            b.extend_from_slice(b" level=");
            b.extend_from_slice(level_name(level).as_bytes());
            b.extend_from_slice(b" msg=");
            logfmt(b, &msg);
            b.extend_from_slice(b" logger=");
            logfmt(b, logger);
            for (k, v) in kvs {
                value.clear();
                let _ = write!(value, "{v:?}");
                b.push(b' ');
                logfmt(b, k);
                b.push(b'=');
                logfmt(b, &value);
            }
        }
        LogFormat::Json => {
            b.extend_from_slice(b"{\"time\":\"");
            rfc3339_nano(b, t.secs, nanos, offset);
            b.extend_from_slice(b"\",\"level\":\"");
            b.extend_from_slice(level_name(level).as_bytes());
            b.extend_from_slice(b"\",\"msg\":");
            json_string(b, msg.as_bytes());
            b.extend_from_slice(b",\"logger\":");
            json_string(b, logger.as_bytes());
            for (k, v) in kvs {
                value.clear();
                let _ = write!(value, "{v:?}");
                b.push(b',');
                json_string(b, k.as_bytes());
                b.push(b':');
                match v {
                    Attr::Bare(_) => b.extend_from_slice(value.as_bytes()),
                    Attr::Str(_) => json_string(b, value.as_bytes()),
                }
            }
            b.push(b'}');
        }
    }
    b.push(b'\n');
}

/// The process's log, once `akit run` has installed one.
static ROOT: RwLock<Option<Arc<Root>>> = RwLock::new(None);

/// Makes `root` the process's log: every logger's messages go there from now on.
pub fn install(root: Arc<Root>) {
    sync_akio_level(&root);
    *ROOT.write().unwrap_or_else(PoisonError::into_inner) = Some(root);
}

/// The process's log, if one is installed.
#[must_use]
pub fn installed() -> Option<Arc<Root>> {
    ROOT.read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Takes the process's log out: messages go to standard error again.
pub fn uninstall() -> Option<Arc<Root>> {
    ROOT.write().unwrap_or_else(PoisonError::into_inner).take()
}

/// Writes `args`, a message of `logger`'s at `level`: to the process's log, or, with none, to
/// standard error as `akit: <message>` when the level is info or above.
pub fn log(logger: Logger, level: Level, args: fmt::Arguments<'_>) {
    let root = ROOT.read().unwrap_or_else(PoisonError::into_inner);
    match root.as_ref() {
        Some(r) => r.write(logger, level, args),
        None if level >= Level::Info => eprintln!("akit: {args}"),
        None => {}
    }
}

/// Whether a message of `logger`'s at `level` would be written.
#[must_use]
pub fn enabled(logger: Logger, level: Level) -> bool {
    match ROOT.read().unwrap_or_else(PoisonError::into_inner).as_ref() {
        Some(r) => r.enabled(logger, level),
        None => level >= Level::Info,
    }
}

/// A line callback, as akit's crates take one (`Arc<dyn Fn(&str)>`), that writes each line as a
/// message of `logger`'s at `level`, after `prefix`.
#[must_use]
pub fn lines(
    logger: Logger,
    level: Level,
    prefix: &'static str,
) -> Arc<dyn Fn(&str) + Send + Sync> {
    Arc::new(move |line: &str| log(logger, level, format_args!("{prefix}{line}")))
}

/// Writes a message of a logger's at `error`, formatted as `format!` does:
/// `akit_observe::error!(Tls, "{name}: {e}")`.
#[macro_export]
macro_rules! error {
    ($logger:ident, $($arg:tt)+) => {
        $crate::logger::log($crate::logger::Logger::$logger, $crate::logger::Level::Error, format_args!($($arg)+))
    };
}

/// Writes a message of a logger's at `warn`: `akit_observe::warn!(Server, "...")`.
#[macro_export]
macro_rules! warn {
    ($logger:ident, $($arg:tt)+) => {
        $crate::logger::log($crate::logger::Logger::$logger, $crate::logger::Level::Warn, format_args!($($arg)+))
    };
}

/// Writes a message of a logger's at `info`: `akit_observe::info!(Server, "...")`.
#[macro_export]
macro_rules! info {
    ($logger:ident, $($arg:tt)+) => {
        $crate::logger::log($crate::logger::Logger::$logger, $crate::logger::Level::Info, format_args!($($arg)+))
    };
}

/// Writes a message of a logger's at `debug`: `akit_observe::debug!(Engine, "...")`.
#[macro_export]
macro_rules! debug {
    ($logger:ident, $($arg:tt)+) => {
        $crate::logger::log($crate::logger::Logger::$logger, $crate::logger::Level::Debug, format_args!($($arg)+))
    };
}

/// akio's records, as the `engine` logger's messages: akio's `log` facade writes here once
/// [`bridge_akio`] has installed it. akio's info records (its engines starting and stopping) are
/// written at debug, as fib leaves its own engine lines off unless asked (`logStatus`); warn and
/// error keep their levels.
struct AkioBridge;

impl akio::log::Logger for AkioBridge {
    fn enabled(&self, m: &akio::log::Metadata<'_>) -> bool {
        enabled(Logger::Engine, from_akio(m.level()))
    }

    fn log(&self, r: &akio::log::Record<'_>) {
        let kvs: Vec<(&str, akio::log::Value<'_>)> = r.key_values().iter().collect();
        let shown: Vec<(&str, Attr<'_>)> = kvs
            .iter()
            .map(|(k, v)| {
                let bare = v.to_str().is_none()
                    && (v.to_u64().is_some()
                        || v.to_i64().is_some()
                        || v.to_f64().is_some_and(f64::is_finite)
                        || v.to_bool().is_some());
                let d = v as &dyn fmt::Display;
                (*k, if bare { Attr::Bare(d) } else { Attr::Str(d) })
            })
            .collect();
        let level = from_akio(r.level());
        let root = ROOT.read().unwrap_or_else(PoisonError::into_inner);
        match root.as_ref() {
            Some(root) => root.write_kv(Logger::Engine, level, *r.args(), &shown),
            None if level >= Level::Info => eprintln!("{}: {}", r.target(), r.args()),
            None => {}
        }
    }
}

fn from_akio(l: akio::log::Level) -> Level {
    match l {
        akio::log::Level::Error => Level::Error,
        akio::log::Level::Warn => Level::Warn,
        akio::log::Level::Info | akio::log::Level::Debug | akio::log::Level::Trace => Level::Debug,
    }
}

/// akio's maximum level, the `engine` logger's: its records below are never formatted.
fn sync_akio_level(root: &Root) {
    if akio::log::logger().is_none() {
        return;
    }
    // akio's info and debug records are the engine's debug (see `AkioBridge`).
    akio::log::set_max_level(match root.level(Logger::Engine) {
        Level::Debug => akio::log::LevelFilter::Debug,
        Level::Info | Level::Warn => akio::log::LevelFilter::Warn,
        Level::Error => akio::log::LevelFilter::Error,
    });
}

/// Has akio's records written as the `engine` logger's, at its level: once a process (akio
/// takes one logger); a second call does nothing.
pub fn bridge_akio() {
    if akio::log::set_boxed_logger(Box::new(AkioBridge)).is_ok() {
        match installed() {
            Some(r) => sync_akio_level(&r),
            None => akio::log::set_max_level(akio::log::LevelFilter::Warn),
        }
    }
}

#[cfg(test)]
mod tests;
