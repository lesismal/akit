#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! fit's media auth and streaming role on akio-media (the Streaming and media auth track,
//! I120–I126; decision 040): `media_auth`'s policies ([`auth`]) and the role that applies them,
//! with the checks of HTTP requests ([`http`]); the streaming role ([`streaming`]) and its handlers
//! ([`handlers`]: `httpflv`, `hls`, `stream_whip` and `stream_whep`); the media pages
//! ([`pages`]: `media_pages`).
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` (I120–I126) and
//! decision 040.

pub mod auth;
mod deadline;
pub mod handlers;
pub mod http;
pub mod library;
pub mod pages;
mod pool;
pub mod role;
mod rtc;
pub mod streaming;
mod swap;
mod tokens;
pub mod vod;

pub use handlers::MODULES;
