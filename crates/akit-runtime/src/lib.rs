#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's module registry, config compilation, and the immutable runtime graph with its atomic swap.
//!
//! - [`traits`]: what the data plane is built from (listener, matcher, router, middleware,
//!   handler, service, upstream), on akio's callback model.
//! - [`module`]: the registry mapping a config's module names to constructors, and the
//!   provisioning and cleanup of the modules a config loads.
//! - [`compile`] and [`Graph`]: a config compiled into what the data plane reads, and
//!   [`GraphCell`], which publishes graphs to the I/O threads, gives each request the current one
//!   without a shared reference count, and retires the old ones (I04).
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

mod cell;
mod graph;
pub mod module;
pub mod request;
pub mod traits;

/// The upstream pools (I14), which [`Env`] and [`Graph`] hold.
pub use akit_upstream as upstream;
pub use cell::{GraphCell, Hold};
pub use graph::{CompiledRoute, Graph, HandlerParams, UPSTREAM_POOLS, compile};

pub use module::{Env, Info, LoadError, Modules, Provision, ProvisionError, Provisioned, Registry};
pub use request::{REQUEST_FIELDS, request_headers, set_request_headers};
pub use traits::{
    Handler, HandlerService, Listener, Matcher, Middleware, Router, Service, Upstream,
};

/// The [`akio_http::Context::local`] that holds a request's canonical path, escaped, when it
/// differs from the path sent (dot or empty segments were removed): what the routes matched, and
/// what handlers go by (akio's request target cannot be rewritten: akio API request A33).
/// akit-listener sets it; it is unset for nearly every request, whose path is canonical already.
/// A middleware that rewrites the path (`rewrite`, I40) sets it to the new path, which the
/// middleware and handler after it go by.
pub const CANONICAL_PATH: &str = "akit.canonical_path";

/// The [`akio_http::Context::local`] set (to `""`) on a request that came over TLS, by
/// akit-listener; unset in the clear. akio's context does not say yet.
pub const TLS: &str = "akit.tls";

/// The akio commit akit is built against (rule 22: akio is pinned by commit hash).
pub const AKIO_REV: &str = "a9a620ab2db9635caee32fc625da51ae255ff591";
