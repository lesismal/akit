#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! fit's tunnel on akio (I101-I103, fit's `tunnel`, decision 038): a hub (`tunnel.hub`) that nodes
//! and proxies link out to, a node (`tunnel.node`) beside the TCP services it reaches, and a proxy
//! (`tunnel.proxy`) that listens on a local address per forward and carries each connection
//! through a hub to a node's service. The links speak fit's protocol byte for byte, so that akit's
//! roles and fit's link with each other.
//!
//! Everything runs on the listeners' akio engine, driven by callbacks: a link is read in its
//! connection's `on_data`; a call that waits (a node's dial of a service, a proxy's open of a
//! tunnel) is answered from the callback that ends the wait; timers run on akio's timer thread.
//! Each tunnel's end sends at most a window the other has not acknowledged and holds its TCP
//! connection's reads until it is acknowledged (rule 8): the windows bound what waits on a link,
//! which may queue fit's `LinkQueue` (akio's per-connection bound, A234).
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` (I101-I103).

pub mod crypt;
pub mod end;
pub mod hub;
pub mod link;
pub mod msg;
pub mod node;
pub mod proxy;
pub mod quic;
pub mod tls;
pub mod uplink;
pub mod ws;

use akit_roles::host::Info;

/// `tunnel.hub`.
pub const HUB: Info = Info {
    id: "tunnel.hub",
    set: |c| c.tunnel.as_ref().is_some_and(|t| t.hub.is_some()),
    new: hub::new,
};

/// `tunnel.node`.
pub const NODE: Info = Info {
    id: "tunnel.node",
    set: |c| c.tunnel.as_ref().is_some_and(|t| t.node.is_some()),
    new: node::new,
};

/// `tunnel.proxy`.
pub const PROXY: Info = Info {
    id: "tunnel.proxy",
    set: |c| c.tunnel.as_ref().is_some_and(|t| t.proxy.is_some()),
    new: proxy::new,
};
