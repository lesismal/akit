#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's listeners on akio: TCP, UDP, TLS and QUIC entry points, and protocol splitting with akio-mux.
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.
