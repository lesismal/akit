#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's roles beside the gateway, fit's `app` (I99, I100): the link tokens the tunnel's hub and
//! the LLM roles' listeners issue ([`linktoken`], fit's `internal/linktoken`, fit's decision 035),
//! and the host that starts, moves and stops the roles on every config applied.
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` (I98–I109) and
//! decision 038.

pub mod linktoken;
