#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! fit's media auth and streaming role on akio-media (the Streaming and media auth track,
//! I120–I126; decision 039): `media_auth`'s policies ([`auth`]) and the role that applies them,
//! with the checks of HTTP requests ([`http`]).
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` (I120–I126) and
//! decision 039.

pub mod auth;
pub mod http;
mod pool;
pub mod role;
pub mod streaming;
mod swap;
