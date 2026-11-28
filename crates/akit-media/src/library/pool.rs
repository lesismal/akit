//! A library's file pool (fit's `medialib/pool.go`): the threads its served files are read on,
//! so that a slow disk holds up those and never an akio I/O thread (rules 1 and 4). It is bounded
//! twice: by its workers, all the disk is asked to do at once, and by its queue, past which a job
//! is refused rather than kept; a job that waited past the timeout is given up.
//!
//! The threads are started as jobs come, up to the bound, and stay until the pool is closed.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// A piece of file work.
pub type Run = Box<dyn FnOnce() + Send>;

struct Job {
    run: Run,
    /// Called in place of `run` for a job that waited past the pool's timeout.
    expire: Option<Run>,
    queued: Instant,
}

#[derive(Default)]
struct State {
    queue: VecDeque<Job>,
    threads: usize,
    idle: usize,
    busy: usize,
    closed: bool,
}

struct Inner {
    state: Mutex<State>,
    more: Condvar,
    ended: Condvar,
    workers: usize,
    queue: usize,
    timeout: Duration,
    refused: AtomicU64,
    expired: AtomicU64,
    name: String,
}

/// What a pool is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PoolStats {
    /// Its bound of workers.
    pub workers: usize,
    /// Its bound of jobs waiting.
    pub queue: usize,
    /// The jobs waiting.
    pub queued: usize,
    /// The jobs running.
    pub busy: usize,
    /// The jobs refused, its queue full or the pool closed.
    pub refused: u64,
    /// The jobs that waited past its timeout.
    pub expired: u64,
}

/// A library's file pool.
#[derive(Clone)]
pub struct Pool(Arc<Inner>);

impl std::fmt::Debug for Pool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pool")
            .field("stats", &self.stats())
            .finish()
    }
}

impl Pool {
    /// A pool of at most `workers` threads (at least one), `queue` jobs waiting (at least one),
    /// each taken up within `timeout` of being queued; its threads are named after `name`.
    #[must_use]
    pub fn new(workers: usize, queue: usize, timeout: Duration, name: &str) -> Pool {
        Pool(Arc::new(Inner {
            state: Mutex::new(State::default()),
            more: Condvar::new(),
            ended: Condvar::new(),
            workers: workers.max(1),
            queue: queue.max(1),
            timeout,
            refused: AtomicU64::new(0),
            expired: AtomicU64::new(0),
            name: name.to_owned(),
        }))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queues `run`, or reports false, queuing nothing, when the queue is full or the pool is
    /// closed. It never waits. `expire`, if given, runs in place of `run` for a job that waited
    /// past the pool's timeout. A job queued runs, or expires, even when the pool is closed
    /// after it is queued.
    pub fn submit(&self, run: Run, expire: Option<Run>) -> bool {
        let mut st = self.lock();
        if st.closed || st.queue.len() >= self.0.queue {
            drop(st);
            self.0.refused.fetch_add(1, Relaxed);
            return false;
        }
        st.queue.push_back(Job {
            run,
            expire,
            queued: Instant::now(),
        });
        if st.idle == 0 && st.threads < self.0.workers {
            let n = st.threads;
            let me = self.clone();
            let spawned = std::thread::Builder::new()
                .name(format!("{}-{n}", self.0.name))
                .spawn(move || me.work());
            if spawned.is_ok() {
                st.threads = st.threads.saturating_add(1);
            }
        }
        drop(st);
        self.0.more.notify_one();
        true
    }

    fn work(&self) {
        let mut st = self.lock();
        loop {
            if let Some(job) = st.queue.pop_front() {
                if job.queued.elapsed() > self.0.timeout {
                    drop(st);
                    self.0.expired.fetch_add(1, Relaxed);
                    if let Some(e) = job.expire {
                        e();
                    }
                    st = self.lock();
                    continue;
                }
                st.busy = st.busy.saturating_add(1);
                drop(st);
                (job.run)();
                st = self.lock();
                st.busy = st.busy.saturating_sub(1);
                if st.busy == 0 && st.queue.is_empty() {
                    self.0.ended.notify_all();
                }
                continue;
            }
            if st.closed {
                st.threads = st.threads.saturating_sub(1);
                if st.threads == 0 {
                    self.0.ended.notify_all();
                }
                return;
            }
            st.idle = st.idle.saturating_add(1);
            st = self.0.more.wait(st).unwrap_or_else(PoisonError::into_inner);
            st.idle = st.idle.saturating_sub(1);
        }
    }

    /// Refuses jobs from now on; the threads end once those queued have run. It does not wait
    /// for them: [`wait`](Self::wait) does.
    pub fn close(&self) {
        let mut st = self.lock();
        st.closed = true;
        let none = st.threads == 0;
        drop(st);
        if none {
            // Nothing will take what is queued: expire it here.
            let jobs: Vec<Job> = self.lock().queue.drain(..).collect();
            for j in jobs {
                self.0.expired.fetch_add(1, Relaxed);
                if let Some(e) = j.expire {
                    e();
                }
            }
        }
        self.0.more.notify_all();
    }

    /// Waits until a closed pool's threads have ended, `limit` at most; whether they had.
    pub fn wait(&self, limit: Duration) -> bool {
        let deadline = Instant::now().checked_add(limit);
        let mut st = self.lock();
        while st.threads > 0 {
            let left = deadline.map_or(limit, |d| d.saturating_duration_since(Instant::now()));
            if left.is_zero() {
                return false;
            }
            st = self
                .0
                .ended
                .wait_timeout(st, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }

    /// Waits until the pool has no job queued or running, `limit` at most; whether it had none.
    pub fn wait_idle(&self, limit: Duration) -> bool {
        let deadline = Instant::now().checked_add(limit);
        let mut st = self.lock();
        while st.busy > 0 || !st.queue.is_empty() {
            let left = deadline.map_or(limit, |d| d.saturating_duration_since(Instant::now()));
            if left.is_zero() {
                return false;
            }
            st = self
                .0
                .ended
                .wait_timeout(st, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }

    /// What the pool is doing.
    #[must_use]
    pub fn stats(&self) -> PoolStats {
        let st = self.lock();
        PoolStats {
            workers: self.0.workers,
            queue: self.0.queue,
            queued: st.queue.len(),
            busy: st.busy,
            refused: self.0.refused.load(Relaxed),
            expired: self.0.expired.load(Relaxed),
        }
    }

    /// Whether `other` is this pool.
    #[must_use]
    pub fn same(&self, other: &Pool) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// fit's `TestPool`: a full queue refuses, a job past the timeout expires, and a closed pool
    /// refuses and runs what it had queued.
    #[test]
    fn bounded() {
        let p = Pool::new(1, 1, Duration::from_secs(60), "t");
        let (release_tx, release) = mpsc::channel::<()>();
        let (ran_tx, ran) = mpsc::channel::<&str>();
        let r1 = ran_tx.clone();
        assert!(p.submit(
            Box::new(move || {
                r1.send("first").unwrap();
                let _ = release.recv();
            }),
            None
        ));
        assert_eq!(ran.recv().unwrap(), "first");
        let r2 = ran_tx.clone();
        assert!(p.submit(Box::new(move || r2.send("queued").unwrap()), None));
        assert!(!p.submit(Box::new(|| {}), None), "a full queue took a job");
        let st = p.stats();
        assert_eq!(
            (st.workers, st.queue, st.queued, st.busy, st.refused),
            (1, 1, 1, 1, 1),
            "{st:?}"
        );
        p.close();
        p.close();
        assert!(!p.submit(Box::new(|| {}), None), "a closed pool took a job");
        release_tx.send(()).unwrap();
        assert!(p.wait(Duration::from_secs(20)));
        assert_eq!(ran.recv().unwrap(), "queued");

        let q = Pool::new(0, 0, Duration::ZERO, "t");
        let (tx, expired) = mpsc::channel::<bool>();
        let (a, b) = (tx.clone(), tx);
        // A job that waits at all is past a timeout of zero.
        q.submit(
            Box::new(move || a.send(false).unwrap()),
            Some(Box::new(move || b.send(true).unwrap())),
        );
        // Either the thread took it before any time passed, or it expired.
        let first = expired.recv_timeout(Duration::from_secs(20)).unwrap();
        q.close();
        assert!(q.wait(Duration::from_secs(20)));
        let st = q.stats();
        assert_eq!((st.workers, st.queue), (1, 1), "{st:?}");
        assert_eq!(st.expired, u64::from(first), "{st:?}");
    }
}
