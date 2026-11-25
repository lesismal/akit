#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's HTTP response cache (RFC 9111): memory and disk tiers.
//!
//! [`MODULES`] has the `cache` middleware, as fit's `middleware/cache`: responses kept by RFC
//! 9111's rules in a bounded memory tier shared by the caches of a zone, and answered from, with
//! `Cache-Control`, `Vary`, validators and `Age` (I67); a disk tier on a pool of its own, the
//! cache lock, and stale-while-revalidate and stale-if-error (I68); purging by key, URL prefix,
//! tag or zone, and what each zone keeps, for the Admin API's `POST /cache/purge` and `GET /cache`
//! ([`purge`], [`zones`]; I69).
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

mod cache;
mod disk;
#[cfg(any(test, feature = "fuzzing"))]
#[doc(hidden)]
pub mod fuzzing;
mod lock;
mod purge;
mod semantics;
mod store;

use akit_runtime::{HandlerParams, Info};

pub use purge::{PurgeError, Selection, ZoneStats, purge, zones, zones_json};

/// The middleware modules of this crate, for [`akit_runtime::Registry::new`].
pub static MODULES: &[Info<HandlerParams>] = &[Info::middleware(
    "http.middleware.cache",
    "Keeps responses by RFC 9111's rules, in memory and on disk, shared by the routes of a zone, and answers from them.",
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

/// How many responses zone `name`'s disk tier holds and their size, and whether its index has
/// been read back; `None` when no cache uses the zone or it has no disk. For tests.
#[doc(hidden)]
#[must_use]
pub fn disk_stats(name: &str) -> Option<(u64, u64, bool)> {
    let d = store::zone_disk(name)?;
    let (n, size) = d.stats();
    Some((n, size, d.is_ready()))
}

/// The bytes zone `name`'s disk tier has waiting to be written; for tests.
#[doc(hidden)]
#[must_use]
pub fn disk_pending(name: &str) -> Option<u64> {
    store::zone_disk(name).map(|d| d.pending.load(std::sync::atomic::Ordering::Acquire))
}

/// Holds every job of the disk pool before it starts, as a disk that hangs would, or lets them
/// go; for tests.
#[doc(hidden)]
pub fn hold_disk(on: bool) {
    disk::hold(on);
}

/// Sets how long a request waits for a response read from disk, and a job for the disk pool;
/// `None` for the default, 5s. For tests.
#[doc(hidden)]
pub fn set_disk_timeout(d: Option<std::time::Duration>) {
    disk::set_timeout(d);
}

/// How many file operations ran off the disk pool: none should. For tests.
#[doc(hidden)]
#[must_use]
pub fn disk_io_off_pool() -> u64 {
    disk::off_pool()
}

/// How many fetches zone `name` has under way, that requests wait for or in the background;
/// for tests.
#[doc(hidden)]
#[must_use]
pub fn fetches(name: &str) -> Option<(usize, u64)> {
    store::zone_fetches(name)
}

/// Drops what zone `name` keeps under `key` from memory alone, as if evicted, leaving the disk
/// tier's; for tests.
#[doc(hidden)]
pub fn evict_from_memory(name: &str, key: &str) {
    store::zone_evict(name, key);
}

/// The directory of zone `name`'s disk tier; for tests.
#[doc(hidden)]
#[must_use]
pub fn disk_path(name: &str) -> Option<std::path::PathBuf> {
    store::zone_disk(name).map(|d| d.dir().to_path_buf())
}
