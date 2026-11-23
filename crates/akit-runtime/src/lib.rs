#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's module registry, config compilation, and the immutable runtime graph with its atomic swap.
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

/// The akio commit akit is built against (rule 22: akio is pinned by commit hash).
pub const AKIO_REV: &str = "50a823155a067ce60cfc424490548f0c318d4c09";
