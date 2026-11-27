//! Deadlines for requests a handler answers later (I142's `vod` `timeout`): one thread, started
//! with the first deadline, runs each job whose time has come, unless it was cancelled. A job
//! must be quick and must not block. A stopgap until akio gives a handler a timer on its engine
//! (akio request A296); the logic is about time, so a timer is what it needs (rules.md).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

type Job = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct State {
    /// By when, then by ID.
    due: BTreeMap<(Instant, u64), ()>,
    jobs: HashMap<u64, (Instant, Job)>,
    next: u64,
}

struct Timer {
    st: Mutex<State>,
    woken: Condvar,
}

fn timer() -> &'static Timer {
    static T: OnceLock<&'static Timer> = OnceLock::new();
    T.get_or_init(|| {
        let t: &'static Timer = Box::leak(Box::new(Timer {
            st: Mutex::new(State::default()),
            woken: Condvar::new(),
        }));
        let _ = std::thread::Builder::new()
            .name("akit-media-deadlines".into())
            .spawn(move || run(t));
        t
    })
}

fn run(t: &Timer) {
    let mut st = t.st.lock().unwrap_or_else(PoisonError::into_inner);
    loop {
        let now = Instant::now();
        let first = st.due.keys().next().copied();
        match first {
            Some((at, id)) if at <= now => {
                st.due.remove(&(at, id));
                if let Some((_, job)) = st.jobs.remove(&id) {
                    drop(st);
                    job();
                    st = t.st.lock().unwrap_or_else(PoisonError::into_inner);
                }
            }
            Some((at, _)) => {
                st = t
                    .woken
                    .wait_timeout(st, at.saturating_duration_since(now))
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
            None => st = t.woken.wait(st).unwrap_or_else(PoisonError::into_inner),
        }
    }
}

/// A deadline set: cancel it with [`cancel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Deadline(u64);

/// Runs `job` after `d`, on the deadlines' thread, unless [`cancel`]led first.
pub(crate) fn after(d: Duration, job: impl FnOnce() + Send + 'static) -> Deadline {
    let t = timer();
    let at = Instant::now().checked_add(d).unwrap_or_else(Instant::now);
    let mut st = t.st.lock().unwrap_or_else(PoisonError::into_inner);
    st.next = st.next.saturating_add(1);
    let id = st.next;
    st.due.insert((at, id), ());
    st.jobs.insert(id, (at, Box::new(job)));
    drop(st);
    t.woken.notify_one();
    Deadline(id)
}

/// Cancels `d`: its job will not run, unless it runs already.
pub(crate) fn cancel(d: Deadline) {
    let t = timer();
    let mut st = t.st.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((at, _)) = st.jobs.remove(&d.0) {
        st.due.remove(&(at, d.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn runs_unless_cancelled() {
        let (tx, rx) = mpsc::channel();
        let t2 = tx.clone();
        let c = after(Duration::from_millis(5), move || {
            t2.send("cancelled").unwrap();
        });
        cancel(c);
        after(Duration::from_millis(10), move || {
            tx.send("ran").unwrap();
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(20)).unwrap(), "ran");
        assert!(rx.try_recv().is_err());
    }
}
