#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's reverse proxy for HTTP, WebSocket and gRPC, with streamed bodies and linked backpressure.
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.
