//! The pool the bcrypt checks of `password_hash` users run on, as fit's `mediaauth` pool: a
//! quarter of the CPUs (at least one), so that akio's I/O threads keep the rest, started with the
//! first check; at most [`QUEUE`] checks wait, and one that waited past [`MAX_WAIT`] is told it is
//! late, so that a flood of wrong passwords takes at most the pool, while tokens, plaintext
//! passwords and passwords remembered are checked at once meanwhile (rules 1 and 4).

use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// How many checks may wait, as fit's `queueSize`.
pub(crate) const QUEUE: usize = 256;
/// How long a check may wait before it is answered busy, as fit's `maxWait`.
pub(crate) const MAX_WAIT: Duration = Duration::from_secs(5);

/// A check: run with whether it waited past [`MAX_WAIT`] (or found no room).
pub(crate) type Job = Box<dyn FnOnce(bool) + Send>;

struct Queued {
    at: Instant,
    run: Job,
}

fn sender() -> Option<&'static SyncSender<Queued>> {
    static TX: OnceLock<Option<SyncSender<Queued>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Queued>(QUEUE);
        let rx = Arc::new(Mutex::new(rx));
        let n = std::thread::available_parallelism()
            .map_or(1, |n| n.get() / 4)
            .max(1);
        let mut started = 0usize;
        for i in 0..n {
            let rx = Arc::clone(&rx);
            let spawned = std::thread::Builder::new()
                .name(format!("akit-media-auth-{i}"))
                .spawn(move || {
                    loop {
                        // One worker waits on the queue at a time; the rest wait on the lock.
                        let job = rx.lock().unwrap_or_else(PoisonError::into_inner).recv();
                        let Ok(q) = job else { return };
                        (q.run)(q.at.elapsed() > MAX_WAIT);
                    }
                });
            if spawned.is_ok() {
                started = started.saturating_add(1);
            }
        }
        (started > 0).then_some(tx)
    })
    .as_ref()
}

/// Runs `job` on the pool; with no room, or no pool, it runs at once, told it is late.
pub(crate) fn submit(job: Job) {
    let q = Queued {
        at: Instant::now(),
        run: job,
    };
    match sender() {
        Some(tx) => match tx.try_send(q) {
            Ok(()) => {}
            Err(TrySendError::Full(q) | TrySendError::Disconnected(q)) => (q.run)(true),
        },
        None => (q.run)(true),
    }
}
