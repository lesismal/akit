#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's handlers: respond, static files, redirects, FastCGI.
//!
//! [`MODULES`] lists them for the registry. So far: `respond` (I05), `static` (I12) and `fastcgi`
//! (I75).
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

mod fastcgi;
mod respond;
mod static_files;

use akit_runtime::{HandlerParams, Info};

/// The handler modules of this crate, for [`akit_runtime::Registry::new`].
pub static MODULES: &[Info<HandlerParams>] = &[
    Info::handler(
        "http.handlers.respond",
        "Answers with a fixed status, header fields and body.",
        respond::new,
    ),
    Info::handler(
        "http.handlers.static",
        "Serves files from a directory, with conditional and range requests and small files kept in memory.",
        static_files::new,
    ),
    Info::handler(
        "http.handlers.fastcgi",
        "Answers requests from a FastCGI application, as PHP-FPM is, over a bounded pool of connections, streaming both ways.",
        fastcgi::new,
    ),
];
