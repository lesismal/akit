#![forbid(unsafe_code)]
//! akit as a library: its command line, [`main`], with the module lists its registry is made from
//! given by the binary that calls it. akit's own binary (`src/bin/akit.rs`) gives [`STANDARD`]; one
//! [`build`] makes (`akit build`, I70, as fit's `fit build`) is the same with the `MODULES` of the
//! crates it was asked for besides, or fewer of akit's own. A module crate registers nothing by
//! itself: it exports `pub static MODULES: &[akit_runtime::Info<akit_runtime::HandlerParams>]`
//! at its root, which a build lists (decision 003); `docs/modules.md` says how to write one.
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

// The command line is `main.rs`, which other changes edit as the binary's root it was until I70.
#[path = "main.rs"]
mod cli;

pub mod build;

// What the command line's modules (`adapt.rs`) reach at the crate's root, where they were until I70.
use cli::{config_paths, flags};

use akit_runtime::{HandlerParams, Info};

pub use cli::main;

// akit's own module crates, which `STANDARD` lists and a build's generated `main` names through
// these, and the module traits and registry a module crate builds on.
pub use akit_bittorrent;
pub use akit_cache;
pub use akit_handler;
pub use akit_media;
pub use akit_middleware;
pub use akit_plugin;
pub use akit_proxy;
pub use akit_runtime;
pub use akit_webrtc;

/// akit's version: `AKIT_VERSION` when the build set it (scripts/dist.sh and the Dockerfile set the
/// release's tag, as fit's `-X main.version`), the crate's version otherwise.
pub const VERSION: &str = match option_env!("AKIT_VERSION") {
    Some(v) if !v.is_empty() => v,
    _ => env!("CARGO_PKG_VERSION"),
};

/// The module lists of akit's own crates, which akit's binary registers, as do the binaries a
/// build makes unless told to leave some out ([`build::STANDARD`] names them, in this order).
pub const STANDARD: &[&[Info<HandlerParams>]] = &[
    akit_handler::MODULES,
    akit_middleware::MODULES,
    akit_cache::MODULES,
    akit_plugin::MODULES,
    akit_proxy::MODULES,
    akit_webrtc::MODULES,
    // I151: bt_tracker.
    akit_bittorrent::MODULES,
    // I123: the streaming role's handlers.
    akit_media::MODULES,
];

/// The roles of fit's config that akit runs (I100: the host; the tunnel's and the LLM feature's
/// roles join it with their iterations, I101-I109). A config that sets another role's member is
/// refused by `run`, which `validate` still checks.
pub const ROLES: &[akit_roles::host::Info] = &[
    // I121: media_auth's policies, before the roles that name them.
    akit_media::role::MEDIA_AUTH,
    // I122: the streaming role.
    akit_media::streaming::STREAMING,
    // I101: the tunnel's roles.
    akit_tunnel::HUB,
    akit_tunnel::NODE,
    akit_tunnel::PROXY,
    // I105-I107: the LLM feature's roles.
    akit_llm::hub::HUB,
    akit_llm::node::NODE,
    akit_llm::proxy::PROXY,
    // I111: the TURN server; I113: the media server of whip and whep.
    akit_webrtc::TURN,
    akit_webrtc::MEDIA,
    // I114: the media proxy of webrtc_proxy.
    akit_webrtc::PROXY,
    // I151, I152: the BitTorrent tracker and client.
    akit_bittorrent::TRACKER,
    akit_bittorrent::CLIENT,
];
