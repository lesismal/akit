//! The time channels go by (fit's `Clock`): the wall clock, its timers on akit-media's deadline
//! thread until akio gives a handler a timer of its engine (A296); or a test's, so that a
//! schedule and a channel's pace are tested on a clock the test moves rather than on time.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use akio_media::av::Time;

/// A call [`Clock::after`] arranged.
pub trait Timer: Send {
    /// Stops the call, unless it runs already.
    fn stop(&self);
}

/// The time a manager's channels go by.
pub trait Clock: Send + Sync + 'static {
    /// The time now, in nanoseconds since 1970.
    fn now(&self) -> i64;
    /// Calls `f` once `d` nanoseconds have passed, never on the caller's thread.
    fn after(&self, d: Time, f: Box<dyn FnOnce() + Send>) -> Box<dyn Timer>;
}

/// The wall clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct WallClock;

struct WallTimer(crate::deadline::Deadline);

impl Timer for WallTimer {
    fn stop(&self) {
        crate::deadline::cancel(self.0);
    }
}

impl Clock for WallClock {
    fn now(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
    }

    fn after(&self, d: Time, f: Box<dyn FnOnce() + Send>) -> Box<dyn Timer> {
        let d = Duration::from_nanos(u64::try_from(d).unwrap_or(0));
        Box::new(WallTimer(crate::deadline::after(d, f)))
    }
}
