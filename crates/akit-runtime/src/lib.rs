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
//! - [`ListenerCerts`]: a TLS listener's certificate manager, serving each handshake the current
//!   graph's certificates, so that a reload changes them without closing the listener (I30).
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

pub mod access;
mod cell;
mod graph;
pub mod module;
pub mod request;
pub mod tls;
pub mod traits;

/// akio's HTTP crate, at akit's akio commit: what a module's handler is built on
/// ([`Handler`] is an [`akio_http::Service`]). A module crate built in with `akit build` (I70)
/// takes it from here, so that its akio is always akit's.
pub use akio_http;
/// The config model, whose [`HandlerParams`] a module's constructor is given: a module akit's
/// model does not know gets [`config::Handler::Module`], its params as written (I70).
pub use akit_config as config;
/// The upstream pools (I14), which [`Env`] and [`Graph`] hold.
pub use akit_upstream as upstream;
pub use cell::{GraphCell, Hold};
pub use graph::{
    CompiledRoute, Graph, HandlerParams, METRICS, NO_SLOT, Previous, UPSTREAM_POOLS, compile,
    metrics_name,
};
pub use tls::ListenerCerts;

pub use module::{Env, Info, LoadError, Modules, Provision, ProvisionError, Provisioned, Registry};
pub use request::{set_request_headers, set_request_path, with_path};
pub use traits::{
    Handler, HandlerService, Listener, Matcher, Middleware, Router, Service, Upstream,
};

/// The [`akio_http::Context::local`] set (to `""`) on a request that came over TLS, by
/// akit-listener; unset in the clear. akio's context does not say yet.
pub const TLS: &str = "akit.tls";

/// The [`akio_http::Context::local`] that holds the `traceparent` field naming the span akit
/// began for a request it traces (I64): the proxy sends it to the upstream in place of the
/// client's. akit-listener sets it; unset for a request not traced.
pub const TRACEPARENT: &str = "akit.traceparent";

/// The akio commit akit is built against (rule 22: akio is pinned by commit hash).
pub const AKIO_REV: &str = "077a67101dea0b0bb8b655b04d79fc3b0b09ffde";
