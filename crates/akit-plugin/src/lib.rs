#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's WASM plugins, run off the I/O threads.
//!
//! I71: the `wasm` middleware ([`MODULES`]), as fit's `plugin/wasm` with fit's own plugin ABI
//! (`fit_on_request` and the host module `fit`): a WebAssembly plugin is loaded when the config is
//! compiled, on akio-wasm (akio ADR-125; [decision 008](../../docs/decisions/008-wasm-runtime.md)),
//! and run for each request of its routes on a pool of instances of its own
//! (`akio_wasm::InstancePool`), each bounded in time and memory, never on an I/O thread; the
//! request waits suspended meanwhile.
//!
//! I72: Proxy-Wasm filters, the subset fit supports ([decision 011](../../docs/decisions/011-proxy-wasm.md)):
//! a request's context and its request headers, and the response's callbacks, its head held and
//! its body filtered on akio's hooks (akio ADR-128), all on the same pool.
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

mod abi;
#[doc(hidden)]
#[allow(
    missing_docs,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation
)]
pub mod encode;
#[cfg(any(test, feature = "fuzzing"))]
#[doc(hidden)]
pub mod fuzzing;
mod plugin;
mod proxywasm;
mod response;
mod wasm;

use akit_runtime::{HandlerParams, Info};

/// The plugin modules of this crate, for [`akit_runtime::Registry::new`].
pub static MODULES: &[Info<HandlerParams>] = &[Info::middleware(
    "http.middleware.wasm",
    "Runs a WebAssembly plugin, of fit's own interface or a Proxy-Wasm filter, on each request, on a pool of its own.",
    wasm::new,
)];
