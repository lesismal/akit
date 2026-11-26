#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's Admin API (I26, I27), with fit's paths and payloads (fit's `admin` package, decision 005),
//! served by akio-http on an engine of its own (akit's decision 006):
//!
//! ```text
//! GET    /config[/<pointer>]  the running base config, as it was written, or the value a JSON
//!                             pointer names in it; ETag, If-None-Match
//! GET    /config/effective    the config as it runs, with what providers provide merged into
//!                             it, and where each part came from (I57)
//! PUT    /config[/<pointer>]  set that value to the body; "-" appends to an array (I27)
//! PATCH  /config[/<pointer>]  merge the body into it, a JSON merge patch (RFC 7396) (I27)
//! DELETE /config/<pointer>    remove it (I27)
//! POST   /load                replace the running config with the body
//! POST   /rollback/{n}        apply version n's config again, as a new version (I27)
//! POST   /reload              apply the running config again, as a new version (I27)
//! GET    /versions            the configs kept, oldest first
//! GET    /versions/{n}        the config applied as version n
//! GET    /metrics             the metrics scrape Options::metrics writes (I37), when set
//! GET    /status              what akit is doing now: listeners, connections, upstreams and
//!                             their backends' health, certificates, requests (I63)
//! GET    /cache               what each cache zone keeps (I69)
//! POST   /cache/purge         remove what a cache zone keeps, by URL, URL prefix, tag, or all
//!                             (I69)
//! GET    /logs                every file output of akit's log and the access log, with its
//!                             loggers, format, zone, rotation and files (I83)
//! POST   /logs/rotate         rotate one output now: {"path"} (I83)
//! POST   /logs/cleanup        remove what its retention says, or with dry_run report it:
//!                             {"path", "dry_run"} (I83)
//! GET    /dashboard/          read-only web pages of the GET endpoints, unless
//!                             admin.disable_dashboard (I63)
//! POST   /config/render       a config fragment written as .conf, YAML or JSON (I91)
//! POST   /config/parse        the JSON of such a text (I91)
//! PUT    /config/text         a text applied as the config, or as ?members= of it (I91)
//! POST   /config/export       the config, or members of it, as a file (I91)
//! POST   /config/import       a file applied as the config, or as ?members= of it (I91)
//! ```
//!
//! With `admin.console` (I89–I91, [`console`]), the console serves the same endpoints under
//! `/api/m/admin/`, behind its logins, beside fit's workbench pages.
//!
//! Bodies are JSON (`application/json`; for `PATCH`, `application/merge-patch+json` too), within
//! `max_config_bytes`. Every change takes `If-Match`, checked against the running config under the
//! lock that orders changes, so that concurrent writers lose nothing and a stale one gets 412;
//! all but a rollback take `?dry_run=1`, which checks the change as an apply would and applies
//! nothing. With `token_file`, every request must carry `Authorization: Bearer <token>` (I27).
//!
//! [`Configs`] keeps the configs applied, numbered, as written, and is the one path every config
//! takes to the running server, from the files (SIGHUP) or the API. The API answers reads on its
//! I/O thread; a load is handed, as a [`Job`], to the control thread, which applies it (through
//! the same reload as SIGHUP, `akit_listener::Listeners::reload`) and answers it; so are edits,
//! rollbacks and reloads, each made from the running config under the history's lock.
//!
//! The file provider (I29, [`Watcher`]) applies a config's files each time they change, through
//! the same [`Configs`]. Other providers (I57, `akit_provider`) send parts, which [`Configs`]
//! merges into the base config (the one the files and the API write) each time a config is
//! applied, by fit's rules: [`Configs::provide`] applies a provider's new part, as a new version
//! whose source is the provider; `GET /config`, its pointers, ETags and rollbacks stay about the
//! base config, as in fit.
//!
//! On `unix/<path>` the API listens on a Unix socket only akit's user reaches (0600; a stale one
//! replaced, one in use refused; no `Host` check), as fit's. With `cert_file` and `key_file` it is
//! served over TLS (I92), with `client_ca_file` to clients with a certificate only; an address off
//! loopback needs TLS and a token. Its certificate files, and the console's, are read again when
//! they change ([`Server::watch_certs`], [`Server::reload_certs`], I94).
//!
//! [`store`] is fit's encrypted store, its `fitstore` file byte for byte (I93).

mod cache;
mod certs;
mod configs;
pub mod console;
mod dashboard;
#[cfg(any(test, feature = "fuzzing"))]
#[doc(hidden)]
pub mod fuzzing;
mod logs;
mod pointer;
mod redact;
mod server;
mod status;
pub mod store;
mod text;
pub mod tokens;
mod watch;

pub use certs::CertReload;
pub use configs::{
    Applied, ApplyError, Configs, Effective, ListenerChange, Mode, Outcome, Refused, SOURCE_ADMIN,
    SOURCE_CERTIFICATES, SOURCE_FILE, SOURCE_ROLLBACK, Version, etag_matches, etag_of,
};
pub use dashboard::DASHBOARD_CSP;
pub use server::{
    DRY_RUN_HEADER, Job, Live, METRICS_CONTENT_TYPE, Metrics, Options, Server, Submit,
    VERSION_HEADER,
};
pub use watch::{
    DEFAULT_WATCH_DEBOUNCE, DEFAULT_WATCH_INTERVAL, DEFAULT_WATCH_MAX_BYTES, Expand, WatchError,
    WatchOptions, WatchReport, Watcher,
};

/// The warning a config gets when its `admin` differs from the one akit started with: the API
/// is not moved until a restart, as in fit.
#[must_use]
pub fn admin_warning(
    first: Option<&akit_config::Admin>,
    next: Option<&akit_config::Admin>,
) -> Option<String> {
    // `access` and the console's `access` follow every config applied (I87, I90), the rest
    // takes a restart, as fit's.
    let without_access = |a: Option<&akit_config::Admin>| {
        a.map(|a| akit_config::Admin {
            access: None,
            console: a.console.as_ref().map(|c| akit_config::Console {
                access: None,
                ..c.clone()
            }),
            ..a.clone()
        })
    };
    (without_access(first) != without_access(next))
        .then(|| "admin changed; the change takes effect when akit restarts".to_owned())
}
