#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's observability: access and error logs, metrics, tracing.
//!
//! I21: the access log, as fit's `observe.AccessLog` ([`AccessLog`]): a line for each request,
//! in fit's `json`, `text` or `line` format ([`Format`]) and with its fields ([`Record`]), built
//! on the thread that finished the request's response, buffered per thread and written by a
//! thread of the log's own; a full buffer drops lines and counts them rather than block (rules 1
//! and 11). Times are written in the zone `time_zone` names ([`Zone`]: UTC, the local zone, or an
//! IANA zone from the system's zoneinfo files).
//!
//! The listener builds each request's [`Record`] from its request and response (akit-listener's
//! `Dispatcher`); a handler that knows more tells it on the thread that sends the response, as
//! the proxy does with [`with_upstream`].
//!
//! I37: [`metrics`], fit's Prometheus metrics: requests by listener and route, upstreams and
//! their backends, config reloads, counted with atomic adds alone on the request path and
//! written in Prometheus's text format.
//!
//! I64: OpenTelemetry, fit's: [`Tracer`] samples requests by their W3C `traceparent` and the
//! sample ratio, and records a server span for each one sampled, encoded as OTLP JSON on the
//! thread that sends the response into a bounded, sharded buffer that a thread of its own exports
//! ([`trace`]); [`MetricsExporter`] exports the metrics a [`MetricWriter`] is given every
//! interval ([`otlp`]). Both post over OTLP/HTTP through [`Otlp`], with akio's HTTP client on an
//! engine of its own, never from an I/O thread.
//!
//! Depends on std, akit-config and akio (the exporters' HTTP client).
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

mod format;
mod log;
pub mod metrics;
pub mod otlp;
pub mod time;
pub mod trace;
mod zone;

use std::cell::{Cell, RefCell};
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::time::Duration;

pub use format::{Format, Record, append as append_line, go_quote, json_string, logfmt};
pub use log::{
    AccessLog, DEFAULT_BUFFER_BYTES, DEFAULT_FLUSH_INTERVAL, FLUSH_BYTES, MAX_SHARDS, Options,
    Shard,
};
pub use otlp::{MetricWriter, MetricsExporter, Otlp, OtlpWriter};
pub use trace::{SpanEnd, Trace, Tracer};
pub use zone::{Zone, ZoneError};

/// Opens where an access log's `output` says lines go: standard output for `"stdout"` (or
/// empty), standard error for `"stderr"`, else the file of that path, created if need be and
/// appended to. Control plane only: it opens a file.
///
/// # Errors
///
/// When the file cannot be opened.
pub fn open_output(output: &str) -> io::Result<Box<dyn Write + Send>> {
    Ok(match output {
        "" | "stdout" => Box::new(io::stdout()),
        "stderr" => Box::new(io::stderr()),
        path => Box::new(OpenOptions::new().create(true).append(true).open(path)?),
    })
}

thread_local! {
    /// The upstream of the response being sent on this thread: [`with_upstream`]'s.
    static UPSTREAM: RefCell<(bool, String, Duration)> =
        const { RefCell::new((false, String::new(), Duration::ZERO)) };
    /// Whether the request being dispatched on this thread is logged: [`logged`].
    static LOGGED: Cell<bool> = const { Cell::new(false) };
}

/// Runs `f`, which sends a proxied request's response, with the upstream it was proxied to,
/// `addr`, and how long the upstream took, `took`, known to the access log's line for that
/// response ([`upstream`]): the line is built on the thread that sends the response, inside `f`.
/// Allocates nothing once this thread has held an address as long.
pub fn with_upstream<R>(addr: &str, took: Duration, f: impl FnOnce() -> R) -> R {
    let set = UPSTREAM.with(|u| {
        u.try_borrow_mut().is_ok_and(|mut u| {
            u.0 = true;
            u.1.clear();
            u.1.push_str(addr);
            u.2 = took;
            true
        })
    });
    let r = f();
    if set {
        UPSTREAM.with(|u| {
            if let Ok(mut u) = u.try_borrow_mut() {
                u.0 = false;
            }
        });
    }
    r
}

/// Calls `f` with the upstream [`with_upstream`] set for the response being sent on this thread,
/// if any.
pub fn upstream<R>(f: impl FnOnce(Option<(&str, Duration)>) -> R) -> R {
    UPSTREAM.with(|u| match u.try_borrow() {
        Ok(u) if u.0 => f(Some((u.1.as_str(), u.2))),
        _ => f(None),
    })
}

/// Runs `f`, a request's handler, with [`logged`] true on this thread: the access log writes the
/// request's line, so a handler that answers later keeps what the line needs (the upstream's
/// address) and only then.
pub fn dispatch_logged<R>(f: impl FnOnce() -> R) -> R {
    let was = LOGGED.with(|l| l.replace(true));
    let r = f();
    LOGGED.with(|l| l.set(was));
    r
}

/// Whether the request whose handler runs on this thread now has an access log line
/// ([`dispatch_logged`]).
#[must_use]
pub fn logged() -> bool {
    LOGGED.with(Cell::get)
}

/// A short string kept inline, without an allocation of its own, when it fits: a listener's or
/// a route's name, carried with a request until its line is written.
#[derive(Clone)]
pub enum Name {
    /// Up to [`Name::INLINE`] bytes, in place.
    Inline(u8, [u8; Name::INLINE]),
    /// A longer one.
    Heap(Box<str>),
}

impl Name {
    /// The most bytes kept inline.
    pub const INLINE: usize = 30;

    /// `s`, inline when it fits.
    #[must_use]
    pub fn new(s: &str) -> Name {
        let b = s.as_bytes();
        let mut buf = [0; Name::INLINE];
        match (buf.get_mut(..b.len()), u8::try_from(b.len())) {
            (Some(dst), Ok(n)) => {
                dst.copy_from_slice(b);
                Name::Inline(n, buf)
            }
            _ => Name::Heap(s.into()),
        }
    }

    /// The string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Name::Inline(n, buf) => buf
                .get(..usize::from(*n))
                .and_then(|b| std::str::from_utf8(b).ok())
                .unwrap_or_default(),
            Name::Heap(s) => s,
        }
    }
}

impl std::fmt::Debug for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_str().fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Name, dispatch_logged, logged, upstream, with_upstream};

    #[test]
    fn names() {
        for s in [
            "",
            "web",
            "a-route-name-of-thirty-bytes!!",
            "a-route-name-of-thirty-one-bytes",
        ] {
            assert_eq!(Name::new(s).as_str(), s);
        }
        assert!(matches!(Name::new(&"x".repeat(31)), Name::Heap(_)));
    }

    #[test]
    fn upstreams() {
        assert_eq!(upstream(|u| u.map(|(a, _)| a.to_owned())), None);
        let got = with_upstream("10.0.0.1:80", Duration::from_millis(3), || {
            upstream(|u| u.map(|(a, d)| (a.to_owned(), d)))
        });
        assert_eq!(
            got,
            Some(("10.0.0.1:80".to_owned(), Duration::from_millis(3)))
        );
        assert_eq!(upstream(|u| u.map(|(a, _)| a.to_owned())), None);
        assert!(!logged());
        assert!(dispatch_logged(logged));
        assert!(!logged());
    }
}
