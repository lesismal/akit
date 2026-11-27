#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! fit's BitTorrent roles on akio-bittorrent (the BitTorrent track, I150–I159, decision 042):
//! `bittorrent.tracker` ([`tracker`], I151), answering announces and scrapes over HTTP through the
//! routes' `bt_tracker` handler and over UDP on its own `listen`, on akio-bittorrent's
//! `TrackerServer`; and `bittorrent.client` ([`client`], I152), downloading and seeding the
//! torrents kept in its `data_dir` on akio-bittorrent's `Client`.
//!
//! [`MODULES`] lists the handlers for akit's registry; the roles are [`akit_roles::host::Info`]s
//! the binary registers.
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

pub mod client;
pub mod tracker;

use akit_runtime::{HandlerParams, Info};

/// The handler modules of this crate, for [`akit_runtime::Registry::new`]: `bt_tracker` (I151).
pub static MODULES: &[Info<HandlerParams>] = &[Info::handler(
    "http.handlers.bt_tracker",
    "Serves the bittorrent.tracker role's HTTP announces and scrapes (BEP 3, 23, 7 and 48): a request whose path ends in announce is an announce, and one ending in scrape a scrape, whatever comes before. Refusals are answered 200 with a failure reason; 503 while the role does not run.",
    tracker::new_handler,
)];

/// The `bittorrent.client` role.
pub use client::CLIENT;
/// The `bittorrent.tracker` role.
pub use tracker::TRACKER;
