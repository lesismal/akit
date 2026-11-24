#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's built-in middlewares: rewrites, headers, limits, authentication, compression and the rest of fit's.
//!
//! [`MODULES`] lists them for the registry. So far (I40): `rewrite`, `redirect` and `headers`, as
//! fit's `middleware/rewrite` and `middleware/headers`. Each is compiled from its params when the
//! config is, and wraps its route's handler once, then (rule 12); per request it runs inline on the
//! I/O thread and never blocks.
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

mod headers;
mod redirect;
mod rewrite;
mod url;

use akit_runtime::{HandlerParams, Info};

/// The middleware modules of this crate, for [`akit_runtime::Registry::new`].
pub static MODULES: &[Info<HandlerParams>] = &[
    Info::middleware(
        "http.middleware.headers",
        "Sets, adds and deletes request and response header fields.",
        headers::new,
    ),
    Info::middleware(
        "http.middleware.redirect",
        "Redirects requests whose path a regular expression matches to a URL made from it.",
        redirect::new,
    ),
    Info::middleware(
        "http.middleware.rewrite",
        "Rewrites the request's path: strips or adds a prefix.",
        rewrite::new,
    ),
];
