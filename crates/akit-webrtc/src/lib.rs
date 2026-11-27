#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! fit's WebRTC on akio (I110-I114, fit's `webrtc`, decision 039): the TURN server
//! (`webrtc.turn`), the media server of the `whip` and `whep` handlers (`webrtc.media`), and the
//! relay of the `webrtc_proxy` handler (`webrtc.proxy`).
//!
//! I110 reads the config: the three handlers are here so that a config with them compiles, and
//! answer 503 as fit's do while their role does not run; `akit run` refuses them until their
//! iterations serve them (I113, I114).
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` (I110-I114).

mod handler;

use akit_runtime::{HandlerParams, Info};

/// The handler modules of this crate, for [`akit_runtime::Registry::new`].
pub static MODULES: &[Info<HandlerParams>] = &[
    Info::handler(
        "http.handlers.whip",
        "Takes WebRTC streams published with WHIP (RFC 9725) into akit's media server, webrtc.media, by name.",
        handler::whip,
    ),
    Info::handler(
        "http.handlers.whep",
        "Plays the streams of akit's media server, webrtc.media, to viewers with WHEP (draft-ietf-wish-whep), by name.",
        handler::whep,
    ),
    Info::handler(
        "http.handlers.webrtc_proxy",
        "Proxies WHIP and WHEP to the media servers of an upstream, and their peers' media through webrtc.proxy's one UDP port.",
        handler::webrtc_proxy,
    ),
];
