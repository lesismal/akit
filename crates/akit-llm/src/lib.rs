#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! fit's LLM feature on akio (I104-I107, fit's `llm`, decision 038): the mesh a hub, its nodes
//! and its proxies link with ([`wire`], [`mesh`]), byte for byte fit's, so that akit's roles and
//! fit's link with each other.
//!
//! Everything runs on the listeners' akio engine, driven by callbacks: a link is read in its
//! WebSocket's `on_message`, a call is answered whenever its handler is ready, a stream's data is
//! handed on as it comes and given back as it is consumed; timers run on akio's timer thread.
//! Each stream sends at most a window its reader has not given back, and a link whose send queue
//! passes its high water holds its streams and, for a while, its reads (rules 8 and 9).
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` (I104-I109).

pub mod json;
pub mod mesh;
pub mod model;
pub mod wire;
