#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's WASM plugins, run off the I/O threads.
//!
//! I71: the `wasm` middleware ([`MODULES`]), as fit's `plugin/wasm` with fit's own plugin ABI
//! (`fit_on_request` and the host module `fit`): a WebAssembly plugin is loaded when the config is
//! compiled, on akio-wasm (akio ADR-125; [decision 008](../../docs/decisions/008-wasm-runtime.md)),
//! and run for each request of its routes on a pool of instances of its own
//! (`akio_wasm::InstancePool`), each bounded in time and memory, never on an I/O thread; the
//! request waits suspended meanwhile. Proxy-Wasm filters are I72's.
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
mod plugin;
mod wasm;

use akit_runtime::{HandlerParams, Info};

/// The plugin modules of this crate, for [`akit_runtime::Registry::new`].
pub static MODULES: &[Info<HandlerParams>] = &[Info::middleware(
    "http.middleware.wasm",
    "Runs a WebAssembly plugin of fit's interface on each request, on a pool of its own.",
    wasm::new,
)];
