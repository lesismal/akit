//! A value the control thread swaps whole and the I/O threads read on every check, as fit's
//! `atomic.Pointer`: each thread keeps its own `Arc` of the value with the epoch it saw, and takes
//! the lock only when the epoch moved (rules 12 and 13: a check touches no lock and no reference
//! count another thread touches). A thread's copies of swaps that are gone are dropped the next
//! time it misses.

use std::any::Any;
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, Mutex, PoisonError, Weak};

/// Swap IDs, to tell swaps apart in a thread's cache.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

type Shared = Arc<dyn Any + Send + Sync>;

/// One thread's copy of one swap's value.
struct Entry {
    id: u64,
    epoch: u64,
    value: Option<Shared>,
    /// Alive while the swap is.
    alive: Weak<()>,
}

thread_local! {
    static CACHE: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
}

/// A value swapped whole; `None` until it is first set.
pub(crate) struct Swap<T: Send + Sync + 'static> {
    id: u64,
    epoch: AtomicU64,
    cur: Mutex<Option<Arc<T>>>,
    alive: Arc<()>,
}

impl<T: Send + Sync + 'static> std::fmt::Debug for Swap<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Swap")
            .field("epoch", &self.epoch.load(SeqCst))
            .finish_non_exhaustive()
    }
}

impl<T: Send + Sync + 'static> Default for Swap<T> {
    fn default() -> Self {
        Swap {
            id: NEXT_ID.fetch_add(1, SeqCst),
            epoch: AtomicU64::new(0),
            cur: Mutex::new(None),
            alive: Arc::new(()),
        }
    }
}

impl<T: Send + Sync + 'static> Swap<T> {
    /// Sets the value; readers see it from their next read.
    pub(crate) fn set(&self, v: Option<Arc<T>>) {
        let mut g = self.cur.lock().unwrap_or_else(PoisonError::into_inner);
        *g = v;
        // Under the lock: a reader that sees the new epoch takes the value under it too.
        self.epoch.fetch_add(1, SeqCst);
    }

    /// The value, for the control plane: a clone of the `Arc`.
    pub(crate) fn get(&self) -> Option<Arc<T>> {
        self.cur
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Runs `f` with the value as this thread last saw it, refreshed when it was swapped since.
    /// `f` may read other swaps; one that reads this one again sees it fresh, through the lock.
    pub(crate) fn with<R>(&self, f: impl FnOnce(Option<&T>) -> R) -> R {
        let epoch = self.epoch.load(SeqCst);
        // Taken out of the cache while `f` runs, so that `f` may use the cache.
        let entry = CACHE.with(|c| {
            let mut c = c.borrow_mut();
            let i = c.iter().position(|e| e.id == self.id)?;
            Some(c.swap_remove(i))
        });
        let entry = match entry {
            Some(e) if e.epoch == epoch => e,
            _ => {
                let g = self.cur.lock().unwrap_or_else(PoisonError::into_inner);
                let value = g.clone().map(|v| v as Shared);
                let epoch = self.epoch.load(SeqCst);
                drop(g);
                CACHE.with(|c| c.borrow_mut().retain(|e| e.alive.strong_count() > 0));
                Entry {
                    id: self.id,
                    epoch,
                    value,
                    alive: Arc::downgrade(&self.alive),
                }
            }
        };
        let r = f(entry.value.as_deref().and_then(|v| v.downcast_ref::<T>()));
        CACHE.with(|c| {
            let mut c = c.borrow_mut();
            // A read of this swap inside `f` cached it again: the later epoch stays.
            match c.iter_mut().find(|e| e.id == entry.id) {
                Some(e) if e.epoch >= entry.epoch => {}
                Some(e) => *e = entry,
                None => c.push(entry),
            }
        });
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swapped_values_are_seen() {
        let s: Swap<u32> = Swap::default();
        assert_eq!(s.with(|v| v.copied()), None);
        s.set(Some(Arc::new(1)));
        assert_eq!(s.with(|v| v.copied()), Some(1));
        s.set(Some(Arc::new(2)));
        assert_eq!(s.with(|v| v.copied()), Some(2));
        // Nested reads of the same swap and of another.
        let t: Swap<u32> = Swap::default();
        t.set(Some(Arc::new(7)));
        let got = s.with(|a| t.with(|b| s.with(|c| (a.copied(), b.copied(), c.copied()))));
        assert_eq!(got, (Some(2), Some(7), Some(2)));
        // Another thread sees the value as it is.
        let s = Arc::new(s);
        let s2 = Arc::clone(&s);
        assert_eq!(
            std::thread::spawn(move || s2.with(|v| v.copied()))
                .join()
                .unwrap(),
            Some(2)
        );
        // A swap dropped leaves no copy behind once another misses.
        drop(t);
        let u: Swap<u32> = Swap::default();
        u.set(Some(Arc::new(3)));
        assert_eq!(u.with(|v| v.copied()), Some(3));
        let ids: Vec<u64> = CACHE.with(|c| c.borrow().iter().map(|e| e.id).collect());
        assert_eq!(ids.len(), 2, "{ids:?}");
    }
}
