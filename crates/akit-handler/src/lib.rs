#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's handlers: respond, static files, redirects, FastCGI.
//!
//! [`MODULES`] lists them for the registry. So far: `respond` (I05).
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

mod respond;

use akit_runtime::{HandlerParams, Info};

/// The handler modules of this crate, for [`akit_runtime::Registry::new`].
pub static MODULES: &[Info<HandlerParams>] = &[Info::handler(
    "http.handlers.respond",
    "Answers with a fixed status, header fields and body.",
    respond::new,
)];
