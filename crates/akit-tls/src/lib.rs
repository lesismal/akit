#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's certificate management: files, ACME, storage, OCSP stapling, on-demand TLS.
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.
