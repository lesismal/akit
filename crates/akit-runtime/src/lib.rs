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
pub mod traits;

pub use cell::{GraphCell, Hold};
pub use graph::{CompiledRoute, Graph, HandlerParams, compile};

pub use module::{Env, Info, LoadError, Modules, Provision, ProvisionError, Provisioned, Registry};
pub use traits::{
    Handler, HandlerService, Listener, Matcher, Middleware, Router, Service, Upstream,
};

/// The akio commit akit is built against (rule 22: akio is pinned by commit hash).
pub const AKIO_REV: &str = "50a823155a067ce60cfc424490548f0c318d4c09";
