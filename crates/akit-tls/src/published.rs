//! A value the control plane replaces whole and the I/O threads read without a lock: what ACME
//! has obtained, read by every handshake, and its HTTP-01 answers.
//!
//! As akit-runtime's graph cell does for the graph (rules 12 and 13), each thread keeps its own
//! `Arc` of the value with the epoch it saw, and takes the lock only when the epoch moved: a read
//! is an atomic load of a line written once per change, and touches no reference count other
//! threads touch.

use std::any::Any;
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

/// IDs, to tell values apart in a thread's cache.
static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

struct Shared<T> {
    id: usize,
    epoch: AtomicU64,
    value: Mutex<Arc<T>>,
}

/// One thread's copy of one value: its ID, the epoch it was taken at, the value, and whether the
/// value is still published (an entry of a dropped one is pruned).
struct Entry {
    id: usize,
    epoch: u64,
    value: Arc<dyn Any + Send + Sync>,
    alive: Weak<dyn Any + Send + Sync>,
}

thread_local! {
    static CACHE: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
}

/// A value replaced whole by [`set`](Self::set) and read by [`with`](Self::with).
pub(crate) struct Published<T> {
    shared: Arc<Shared<T>>,
}

impl<T: Send + Sync + 'static> Published<T> {
    pub(crate) fn new(v: T) -> Published<T> {
        Published {
            shared: Arc::new(Shared {
                id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                epoch: AtomicU64::new(0),
                value: Mutex::new(Arc::new(v)),
            }),
        }
    }

    /// The current value, for the control plane.
    pub(crate) fn get(&self) -> Arc<T> {
        Arc::clone(
            &self
                .shared
                .value
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    /// Publishes `v` in place of the current value.
    pub(crate) fn set(&self, v: T) {
        let mut cur = self
            .shared
            .value
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *cur = Arc::new(v);
        // Under the lock, so that a reader that takes it sees the value with its epoch.
        self.shared.epoch.fetch_add(1, Ordering::Release);
    }

    /// Calls `f` with the current value, from this thread's copy, refreshed if it is stale. A
    /// read nested in `f` takes the lock instead; on a thread whose thread-locals are being torn
    /// down, `f` is not called and `R::default()` is returned (akit reads these from no
    /// thread-local destructor).
    pub(crate) fn with<R: Default>(&self, f: impl FnOnce(&T) -> R) -> R {
        let epoch = self.shared.epoch.load(Ordering::Acquire);
        let id = self.shared.id;
        let r = CACHE.try_with(move |c| {
            let Ok(mut cache) = c.try_borrow_mut() else {
                return Err(f);
            };
            let pos = match cache.iter().position(|e| e.id == id) {
                Some(i) if cache.get(i).is_some_and(|e| e.epoch == epoch) => i,
                found => {
                    let (epoch, value) = {
                        let v = self
                            .shared
                            .value
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner);
                        (self.shared.epoch.load(Ordering::Acquire), Arc::clone(&v))
                    };
                    let shared: Arc<dyn Any + Send + Sync> = self.shared.clone();
                    let entry = Entry {
                        id,
                        epoch,
                        value,
                        alive: Arc::downgrade(&shared),
                    };
                    match found {
                        Some(i) => {
                            if let Some(e) = cache.get_mut(i) {
                                *e = entry;
                            }
                            i
                        }
                        None => {
                            // Values dropped since are pruned when another is added.
                            cache.retain(|e| e.alive.strong_count() > 0);
                            cache.push(entry);
                            cache.len().saturating_sub(1)
                        }
                    }
                }
            };
            match cache.get(pos).and_then(|e| e.value.downcast_ref::<T>()) {
                Some(v) => Ok(f(v)),
                None => Err(f),
            }
        });
        match r {
            Ok(Ok(r)) => r,
            Ok(Err(f)) => f(&self.get()),
            Err(_) => R::default(),
        }
    }
}

impl<T> std::fmt::Debug for Published<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Published")
            .field("epoch", &self.shared.epoch.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_follow_sets_on_every_thread() {
        let p = Arc::new(Published::new(1u32));
        assert_eq!(p.with(|v| *v), 1);
        p.set(2);
        assert_eq!(p.with(|v| *v), 2);
        let q = Arc::clone(&p);
        std::thread::spawn(move || {
            assert_eq!(q.with(|v| *v), 2);
            q.set(3);
            assert_eq!(q.with(|v| *v), 3);
        })
        .join()
        .unwrap();
        assert_eq!(p.with(|v| *v), 3);
        assert_eq!(*p.get(), 3);
        // Two values on one thread, and one dropped.
        let other = Published::new(String::from("x"));
        assert_eq!(other.with(String::clone), "x");
        // Nested: from the lock.
        assert_eq!(p.with(|a| other.with(|b| format!("{a}{b}"))), "3x");
        drop(other);
        let third = Published::new(7u8);
        assert_eq!(third.with(|v| *v), 7);
        assert_eq!(p.with(|v| *v), 3);
    }
}
