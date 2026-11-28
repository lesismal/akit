//! How akit's TCP and Unix-domain stream listeners are scheduled: on akio's event loops, each
//! connection on one I/O thread for its whole life (akio's ADR-001), not on akio's default work
//! stealing (ADR-104), unless `AKIO_DEFAULT_SCHEDULING` asks otherwise.
//!
//! Under work stealing akit's linked backpressure no longer bounds what a streamed relay holds:
//! akit-proxy's `stream_download` test saw resident memory rise 51–59 MiB moving 64 MiB through
//! a held downstream, where event loops keep it within the test's bound (akio API request A303,
//! decision 91 in `docs/decisions-pending.md`). akit keeps event loops until akio's work
//! stealing honours paused reads and full writes.

use std::sync::OnceLock;

use akio::{Listener, Scheduling};

/// `l` with akit's scheduling: [`Scheduling::EventLoops`]. When `AKIO_DEFAULT_SCHEDULING` is set
/// (to `event-loops` or `work-stealing`), `l` is left unchosen and akio's default follows the
/// variable, so work stealing can still be tried. For TCP and Unix-domain stream listeners; a
/// UDP listener has event loops anyway.
#[must_use]
pub fn scheduled(l: Listener) -> Listener {
    if left_to_akio() {
        l
    } else {
        l.scheduling(Scheduling::EventLoops)
    }
}

/// Whether `AKIO_DEFAULT_SCHEDULING` is set, read once.
fn left_to_akio() -> bool {
    static SET: OnceLock<bool> = OnceLock::new();
    *SET.get_or_init(|| std::env::var_os("AKIO_DEFAULT_SCHEDULING").is_some_and(|v| !v.is_empty()))
}
