#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's handlers: respond, static files, redirects, FastCGI, the forward proxy.
//!
//! [`MODULES`] lists them for the registry. So far: `respond` (I05), `static` (I12), `fastcgi`
//! (I75) and `forward_proxy` (I55).
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

mod fastcgi;
mod forward_proxy;
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
    Info::handler(
        "http.handlers.forward_proxy",
        "A forward proxy for HTTP CONNECT, with access rules for clients and destinations.",
        forward_proxy::new,
    ),
];
