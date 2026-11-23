#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's layer 4 proxying: TCP and UDP, TLS routed by SNI, PROXY protocol.
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.
