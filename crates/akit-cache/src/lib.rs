#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's HTTP response cache (RFC 9111): memory and disk tiers.
//!
//! [`MODULES`] has the `cache` middleware (I67), as fit's `middleware/cache`: responses kept by
//! RFC 9111's rules in a bounded memory tier shared by the caches of a zone, and answered from,
//! with `Cache-Control`, `Vary`, validators and `Age`. I68 adds the disk tier, the cache lock and
//! the stale windows; I69 purging by key and tag.
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

mod cache;
mod semantics;
mod store;

use akit_runtime::{HandlerParams, Info};

/// The middleware modules of this crate, for [`akit_runtime::Registry::new`].
pub static MODULES: &[Info<HandlerParams>] = &[Info::middleware(
    "http.middleware.cache",
    "Keeps responses by RFC 9111's rules, in memory, shared by the routes of a zone, and answers from them.",
    cache::new,
)];

/// The cache's clock, which tests hold still.
mod clock {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, SystemTime};

    /// The time tests hold the clock at, in nanoseconds since the Unix epoch; 0 when not held.
    static HELD: AtomicU64 = AtomicU64::new(0);

    /// The time: one relaxed load besides the system clock.
    pub(crate) fn now() -> SystemTime {
        match HELD.load(Ordering::Relaxed) {
            0 => SystemTime::now(),
            n => SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_nanos(n))
                .unwrap_or(SystemTime::UNIX_EPOCH),
        }
    }

    pub(crate) fn hold(at: Option<SystemTime>) {
        let n = at
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |d| {
                u64::try_from(d.as_nanos()).unwrap_or(u64::MAX).max(1)
            });
        HELD.store(n, Ordering::Relaxed);
    }
}

/// Holds the cache's clock at `at`, or lets it go with `None`: for tests, as fit's `clockAt`.
/// Every cache in the process reads it.
#[doc(hidden)]
pub fn hold_clock(at: Option<std::time::SystemTime>) {
    clock::hold(at);
}

/// How many keys the zone `name` holds, its size in bytes, and the bytes responses being stored
/// hold so far; `None` when no cache uses the zone. For tests and diagnostics.
#[doc(hidden)]
#[must_use]
pub fn zone_stats(name: &str) -> Option<(usize, u64, u64)> {
    store::zone_stats(name)
}
