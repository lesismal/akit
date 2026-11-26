//! The pacing of a relay between two connections (fit's `tunnel/internal/common/pace.go`), for the
//! console socket and the web proxies (I103b): each side's reads are held while the other side's
//! backlog is past [`WATERMARK`], and let go once it has drained (rules 8 and 9). Both sides of
//! such a relay are relayed (`set_relayed`): their reads do not pause for their own output, which
//! the gate of the other side stands in for.

use std::sync::{Arc, Mutex, PoisonError};

use akio_http::ws::WsSender;

/// How much a connection a relay sends to may have queued before the relay stops reading the
/// other side: fit's `Watermark`.
pub const WATERMARK: usize = 256 << 10;

/// A connection whose reads a [`Gate`] holds, and that a relay sends to.
pub trait Side: Send + Sync {
    /// Holds (`true`) or lets go (`false`) its reads.
    fn hold_reads(&self, hold: bool);
    /// The bytes it has queued, not yet written.
    fn queued(&self) -> usize;
    /// Runs `f` once what it has queued is out, or once it closes.
    fn on_drain(&self, f: Box<dyn FnOnce() + Send>);
}

impl<H: akio::Handler> Side for akio::Conn<H> {
    fn hold_reads(&self, hold: bool) {
        akio::Conn::hold_reads(self, hold);
    }
    fn queued(&self) -> usize {
        self.queued_bytes()
    }
    fn on_drain(&self, f: Box<dyn FnOnce() + Send>) {
        akio::Conn::on_drain(self, f);
    }
}

impl Side for WsSender {
    fn hold_reads(&self, hold: bool) {
        WsSender::hold_reads(self, hold);
    }
    fn queued(&self) -> usize {
        self.queued_bytes()
    }
    fn on_drain(&self, f: Box<dyn FnOnce() + Send>) {
        WsSender::on_drain(self, f);
    }
}

/// Holds the reads of one side of a relay while the other side's backlog is past [`WATERMARK`],
/// and lets them go once it has drained.
pub struct Gate {
    reads: Box<dyn Side>,
    /// Waiting for the other side to drain.
    waiting: Mutex<bool>,
}

impl std::fmt::Debug for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gate").finish_non_exhaustive()
    }
}

impl Gate {
    /// The gate of `reads`' reads, which is marked relayed by its caller.
    #[must_use]
    pub fn new(reads: Box<dyn Side>) -> Arc<Gate> {
        Arc::new(Gate {
            reads,
            waiting: Mutex::new(false),
        })
    }

    /// Called after each send to `to` of what was read on the gate's side: holds the gate's reads
    /// while `to`'s backlog is past [`WATERMARK`], until it drains.
    pub fn pace(self: &Arc<Self>, to: &dyn Side) {
        {
            let mut w = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
            if *w || to.queued() <= WATERMARK {
                return;
            }
            *w = true;
            self.reads.hold_reads(true);
        }
        // Registered once the hold is on, so that a drain that comes at once lets it go.
        let me = Arc::clone(self);
        to.on_drain(Box::new(move || me.release()));
    }

    /// Holds the gate's reads, as while what was read waits for the other side to open.
    pub fn hold(&self) {
        self.reads.hold_reads(true);
    }

    /// Lets the gate's reads go.
    pub fn release(&self) {
        let mut w = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        *w = false;
        self.reads.hold_reads(false);
    }
}
