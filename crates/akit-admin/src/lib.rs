#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's Admin API (I26), with fit's paths and payloads (fit's `admin` package, decision 005),
//! served by akio-http on an engine of its own (akit's decision 006):
//!
//! ```text
//! GET  /config[/<pointer>]  the running config, as it was written, or the value a JSON
//!                           pointer names in it; ETag, If-None-Match
//! POST /load                replace the running config with the body (application/json,
//!                           within max_config_bytes), If-Match checked
//! GET  /versions            the configs kept, oldest first
//! GET  /versions/{n}        the config applied as version n
//! GET  /metrics             the metrics scrape Options::metrics writes (I37), when set
//! ```
//!
//! [`Configs`] keeps the configs applied, numbered, as written, and is the one path every config
//! takes to the running server, from the files (SIGHUP) or the API. The API answers reads on its
//! I/O thread; a load is handed, as a [`Job`], to the control thread, which applies it (through
//! the same reload as SIGHUP, `akit_listener::Listeners::reload`) and answers it.
//!
//! Edits by pointer, rollback, `POST /reload`, dry runs and the token come with I27; a Unix socket
//! with akio API request A23; the file provider with I29.

mod configs;
mod pointer;
mod server;

pub use configs::{
    Applied, ApplyError, Configs, ListenerChange, Outcome, Refused, SOURCE_ADMIN,
    SOURCE_CERTIFICATES, SOURCE_FILE, Version, etag_matches, etag_of,
};
pub use server::{Job, METRICS_CONTENT_TYPE, Metrics, Options, Server, Submit, VERSION_HEADER};

/// The warning a config gets when its `admin` differs from the one akit started with: the API
/// is not moved until a restart, as in fit.
#[must_use]
pub fn admin_warning(
    first: Option<&akit_config::Admin>,
    next: Option<&akit_config::Admin>,
) -> Option<String> {
    (first != next).then(|| "admin changed; the change takes effect when akit restarts".to_owned())
}
