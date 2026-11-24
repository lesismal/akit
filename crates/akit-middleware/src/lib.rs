#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's built-in middlewares: rewrites, headers, limits, authentication, compression and the rest of fit's.
//!
//! [`MODULES`] lists them for the registry. So far: `rewrite`, `redirect` and `headers` (I40), as
//! fit's `middleware/rewrite` and `middleware/headers`; `rate_limit`, `concurrency_limit` and
//! `body_limit` (I41), as fit's `middleware/limit`; `basic_auth` and `client_cert` (I42), as fit's
//! `middleware/auth`; `forward_auth` (I43), as fit's `middleware/auth` too, which asks an auth
//! service on akit-upstream's pool, the request suspended meanwhile; `circuit_breaker` (I45), as fit's
//! `middleware/breaker`; `compress` (I47), as fit's `middleware/compress`, which has akio encode
//! streamed bodies write by write. Each is compiled from its params when the
//! config is, and wraps its route's handler once, then (rule 12); per request it runs inline on the
//! I/O thread and never blocks.
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

mod auth;
mod breaker;
mod compress;
mod forward;
mod headers;
mod limit;
mod redirect;
mod rewrite;
mod url;

use akit_runtime::{HandlerParams, Info};

/// The middleware modules of this crate, for [`akit_runtime::Registry::new`].
pub static MODULES: &[Info<HandlerParams>] = &[
    Info::middleware(
        "http.middleware.basic_auth",
        "Asks for a user name and password (Basic authentication), checked against bcrypt hashes.",
        auth::new_basic,
    ),
    Info::middleware(
        "http.middleware.body_limit",
        "Limits the size of request bodies, answering 413 past it.",
        limit::new_body,
    ),
    Info::middleware(
        "http.middleware.circuit_breaker",
        "Stops sending a route's requests on while too many fail or are slow, then lets a few through to try again.",
        breaker::new,
    ),
    Info::middleware(
        "http.middleware.client_cert",
        "Asks for a client certificate, verified by the listener's TLS (mTLS).",
        auth::new_client_cert,
    ),
    Info::middleware(
        "http.middleware.compress",
        "Compresses responses with zstd, brotli or gzip as they are written.",
        compress::new,
    ),
    Info::middleware(
        "http.middleware.concurrency_limit",
        "Limits how many of a route's requests are under way at once.",
        limit::new_concurrency,
    ),
    Info::middleware(
        "http.middleware.forward_auth",
        "Asks an auth service whether a request may go on, and passes fields of its answer on.",
        forward::new,
    ),
    Info::middleware(
        "http.middleware.headers",
        "Sets, adds and deletes request and response header fields.",
        headers::new,
    ),
    Info::middleware(
        "http.middleware.rate_limit",
        "Limits the rate of requests, by client address, header field or route, allowing a burst.",
        limit::new_rate,
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
