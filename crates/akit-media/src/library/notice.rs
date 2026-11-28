//! A notice of something that changed, for those that wait for it rather than try again later:
//! a library's file pool with room again, its catalog replaced. No timer polls the library.
//!
//! A waiter reads the notice's count of changes before it tries; when what it tried is refused, it
//! hands [`Notice::when`] what it read and a callback, which is called at the next change, or
//! at once (`when` answers true, keeping nothing) when a change came since it read. A change
//! counts before it takes the waiters, under their lock, so that none is missed.

use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::sync::{Mutex, PoisonError};

/// A callback a notice keeps.
pub type Told = Box<dyn FnOnce() + Send>;

/// A notice: its changes counted, and those waiting for the next.
#[derive(Default)]
pub struct Notice {
    changes: AtomicU64,
    waiters: Mutex<Vec<Told>>,
}

impl std::fmt::Debug for Notice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Notice")
            .field("changes", &self.changes.load(SeqCst))
            .finish_non_exhaustive()
    }
}

impl Notice {
    /// The changes so far: read before trying what may be refused.
    #[must_use]
    pub fn seen(&self) -> u64 {
        self.changes.load(SeqCst)
    }

    /// Keeps `told` for the next change, or answers true, keeping nothing, when one came since
    /// `seen`: try again now.
    pub fn when(&self, seen: u64, told: Told) -> bool {
        let mut w = self.waiters.lock().unwrap_or_else(PoisonError::into_inner);
        if self.changes.load(SeqCst) != seen {
            return true;
        }
        w.push(told);
        false
    }

    /// A change: those waiting are told, on this thread, with no lock held.
    pub fn fire(&self) {
        self.changes.fetch_add(1, SeqCst);
        let waiters = {
            let mut w = self.waiters.lock().unwrap_or_else(PoisonError::into_inner);
            if w.is_empty() {
                return;
            }
            std::mem::take(&mut *w)
        };
        for t in waiters {
            t();
        }
    }

    /// How many wait: for tests.
    #[doc(hidden)]
    pub fn waiting(&self) -> usize {
        self.waiters
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn told_once_and_never_missed() {
        let n = Notice::default();
        let hits = Arc::new(AtomicUsize::new(0));
        let seen = n.seen();
        let h = Arc::clone(&hits);
        assert!(!n.when(
            seen,
            Box::new(move || {
                h.fetch_add(1, SeqCst);
            })
        ));
        assert_eq!(n.waiting(), 1);
        n.fire();
        n.fire();
        assert_eq!((hits.load(SeqCst), n.waiting()), (1, 0));
        // A change since what was seen: try again now, nothing kept.
        assert!(n.when(seen, Box::new(|| unreachable!())));
        assert_eq!(n.waiting(), 0);
    }
}
