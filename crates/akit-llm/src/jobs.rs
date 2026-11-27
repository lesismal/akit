//! A node's jobs (fit's `llm/node/jobs.go`): its fixed actions and GPU settings run in the
//! background on a pool of their own, at most [`MAX_RUNNING`] at once, [`MAX_JOBS`] kept with the
//! last [`MAX_JOB_OUTPUT`](crate::model::MAX_JOB_OUTPUT) bytes of their output; their output
//! followed as it is written (`/node/job/follow`), a job canceled or past its timeout killed
//! with its process group.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::Duration;

use akio::timer::{SendTimer, after_send};

use crate::exec::{Command, Ended, Killer, OutBuf};
use crate::json::Obj;
use crate::model::CODE_NOT_FOUND;
use crate::wire::{Error, Stream};

/// A fixed action.
pub const KIND_ACTION: &str = "action";
/// A GPU tool installed.
pub const KIND_GPU_INSTALL: &str = "gpu-install";
/// A GPU setting.
pub const KIND_GPU_SET: &str = "gpu-set";

/// Running.
pub const JOB_RUNNING: &str = "running";
/// Ended well.
pub const JOB_DONE: &str = "done";
/// Ended badly.
pub const JOB_FAILED: &str = "failed";
/// Canceled.
pub const JOB_CANCELED: &str = "canceled";

/// The jobs kept: the ended ones are dropped first, the oldest first.
pub const MAX_JOBS: usize = 200;
/// The jobs running at once; one more is refused.
pub const MAX_RUNNING: usize = 8;

/// A follower's frames are this far apart at least, so that a chatty job goes in batches.
const FOLLOW_BATCH: Duration = Duration::from_millis(100);
/// A follower sends the job's state after this long without output, so that the stream's idle
/// timeout measures the link, not a quiet job.
const FOLLOW_KEEPALIVE: Duration = Duration::from_secs(30);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now() -> i64 {
    akit_roles::linktoken::unix_now()
}

/// A job (fit's `model.Job`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Job {
    /// Its number on the node.
    pub id: i64,
    /// [`KIND_ACTION`], [`KIND_GPU_INSTALL`] or [`KIND_GPU_SET`].
    pub kind: String,
    /// What it does.
    pub title: String,
    /// [`JOB_RUNNING`], [`JOB_DONE`], [`JOB_FAILED`] or [`JOB_CANCELED`].
    pub status: String,
    /// Its command's exit code, once it ended.
    pub exit_code: Option<i64>,
    /// Why it failed.
    pub error: String,
    /// Its output from `offset`.
    pub output: String,
    /// The absolute offset of `output`.
    pub offset: i64,
    /// The output's length so far.
    pub output_len: i64,
    /// When it started, in seconds since 1970.
    pub created_at: i64,
    /// When it ended.
    pub finished_at: i64,
}

impl Job {
    /// As JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        Obj::new()
            .int("id", self.id)
            .str("kind", &self.kind)
            .str("title", &self.title)
            .str("status", &self.status)
            .raw(
                "exit_code",
                &self
                    .exit_code
                    .map_or_else(|| "null".to_owned(), |c| c.to_string()),
            )
            .str_omit("error", &self.error)
            .str_omit("output", &self.output)
            .int("offset", self.offset)
            .int("output_len", self.output_len)
            .int("created_at", self.created_at)
            .int_omit("finished_at", self.finished_at)
            .end()
    }
}

/// What a job runs: given its output and its killer, it returns its exit code and why it
/// failed. It blocks, on the jobs' pool.
pub type Run = Box<dyn FnOnce(&OutBuf, &Killer) -> (i64, Result<(), String>) + Send>;

struct Entry {
    job: Job,
    out: OutBuf,
    kill: Killer,
    timer: Option<SendTimer>,
}

struct St {
    next_id: i64,
    list: BTreeMap<i64, Entry>,
}

struct Inner {
    pool: Arc<akio::Pool>,
    kill_pool: Arc<akio::Pool>,
    st: Mutex<St>,
    on_change: OnceLock<Arc<dyn Fn() + Send + Sync>>,
}

/// A node's jobs. Cheap to clone.
#[derive(Clone)]
pub struct Jobs(Arc<Inner>);

impl std::fmt::Debug for Jobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Jobs").finish_non_exhaustive()
    }
}

/// A pool of `threads` threads and `queue` jobs waiting, its threads made as needed.
#[must_use]
pub fn pool(name: &str, threads: usize, queue: usize) -> Arc<akio::Pool> {
    let mut cfg = akio::PoolConfig::default();
    cfg.min_threads = 0;
    cfg.max_threads = threads;
    cfg.queue_capacity = queue;
    name.clone_into(&mut cfg.name);
    akio::Pool::new(cfg)
}

/// A job's command run: `$ <command>`, then its output.
pub fn run_command(c: Command) -> Run {
    Box::new(move |out, kill| {
        out.write(format!("$ {}\n", c.display()).as_bytes());
        c.run(out, kill, Duration::ZERO)
    })
}

/// Commands run one after another, the first failure ending the job.
pub fn run_commands(cs: Vec<Command>) -> Run {
    Box::new(move |out, kill| {
        for c in cs {
            out.write(format!("$ {}\n", c.display()).as_bytes());
            let (code, r) = c.run(out, kill, Duration::ZERO);
            if r.is_err() {
                return (code, r);
            }
        }
        (0, Ok(()))
    })
}

impl Jobs {
    /// No jobs; they run on a pool of [`MAX_RUNNING`] threads, and their process groups'
    /// signals go on `kill_pool`.
    #[must_use]
    pub fn new(kill_pool: Arc<akio::Pool>) -> Jobs {
        Jobs(Arc::new(Inner {
            pool: pool("akit-llm-jobs", MAX_RUNNING, MAX_RUNNING),
            kill_pool,
            st: Mutex::new(St {
                next_id: 0,
                list: BTreeMap::new(),
            }),
            on_change: OnceLock::new(),
        }))
    }

    /// Runs `f` when a job starts or ends (set once, before the first job); it must not block.
    pub fn on_change(&self, f: impl Fn() + Send + Sync + 'static) {
        let _ = self.0.on_change.set(Arc::new(f));
    }

    fn changed(&self) {
        if let Some(f) = self.0.on_change.get() {
            f();
        }
    }

    fn running_locked(st: &St) -> usize {
        st.list
            .values()
            .filter(|e| e.job.status == JOB_RUNNING)
            .count()
    }

    /// The jobs running.
    #[must_use]
    pub fn running(&self) -> usize {
        Self::running_locked(&lock(&self.0.st))
    }

    /// Starts `run` in the background, killed after `timeout`; the job returned has no output
    /// yet.
    ///
    /// # Errors
    ///
    /// [`MAX_RUNNING`] jobs run already, or the pool took no more.
    pub fn start(
        &self,
        kind: &str,
        title: &str,
        timeout: Duration,
        run: Run,
    ) -> Result<Job, String> {
        let (job, out, kill) = {
            let mut st = lock(&self.0.st);
            if Self::running_locked(&st) >= MAX_RUNNING {
                return Err(format!(
                    "{MAX_RUNNING} jobs are running already; wait for one to end"
                ));
            }
            st.next_id = st.next_id.saturating_add(1);
            let job = Job {
                id: st.next_id,
                kind: kind.to_owned(),
                title: title.to_owned(),
                status: JOB_RUNNING.to_owned(),
                created_at: now(),
                ..Job::default()
            };
            let (out, kill) = (OutBuf::new(), Killer::new(Arc::clone(&self.0.kill_pool)));
            st.list.insert(
                job.id,
                Entry {
                    job: job.clone(),
                    out: out.clone(),
                    kill: kill.clone(),
                    timer: None,
                },
            );
            Self::prune_locked(&mut st);
            (job, out, kill)
        };
        let id = job.id;
        let me = Arc::downgrade(&self.0);
        let (out2, kill2) = (out.clone(), kill.clone());
        let r = self.0.pool.try_execute(Box::new(move || {
            let (code, r) = run(&out2, &kill2);
            let Some(inner) = me.upgrade() else { return };
            let timer = {
                let mut st = lock(&inner.st);
                st.list.get_mut(&id).and_then(|e| {
                    let j = &mut e.job;
                    j.exit_code = Some(code);
                    j.finished_at = now();
                    match (r, kill2.why()) {
                        (Ok(()), _) => JOB_DONE.clone_into(&mut j.status),
                        (Err(_), Some(Ended::Canceled)) => {
                            JOB_CANCELED.clone_into(&mut j.status);
                            "canceled".clone_into(&mut j.error);
                        }
                        (Err(_), Some(Ended::Timeout)) => {
                            JOB_FAILED.clone_into(&mut j.status);
                            "timeout".clone_into(&mut j.error);
                        }
                        (Err(e), None) => {
                            JOB_FAILED.clone_into(&mut j.status);
                            j.error = e;
                        }
                    }
                    e.timer.take()
                })
            };
            if let Some(t) = timer {
                t.cancel();
            }
            // Followers learn of the end once the state is set.
            out2.notify();
            Jobs(inner).changed();
        }));
        if r.is_err() {
            lock(&self.0.st).list.remove(&id);
            return Err("the jobs' pool is full; wait for a job to end".to_owned());
        }
        if !timeout.is_zero() {
            let k = kill.clone();
            let t = after_send(timeout, move || k.kill(Ended::Timeout)).ok();
            let mut st = lock(&self.0.st);
            match st.list.get_mut(&id) {
                Some(e) if e.job.status == JOB_RUNNING => e.timer = t,
                _ => {
                    if let Some(t) = t {
                        t.cancel();
                    }
                }
            }
        }
        self.changed();
        Ok(job)
    }

    fn prune_locked(st: &mut St) {
        if st.list.len() <= MAX_JOBS {
            return;
        }
        let done: Vec<i64> = st
            .list
            .iter()
            .filter(|(_, e)| e.job.status != JOB_RUNNING)
            .map(|(id, _)| *id)
            .collect();
        for id in done {
            if st.list.len() <= MAX_JOBS {
                break;
            }
            st.list.remove(&id);
        }
    }

    /// Job `id` with its output from absolute `offset`.
    #[must_use]
    pub fn get(&self, id: i64, offset: usize) -> Option<Job> {
        let (mut j, out) = {
            let st = lock(&self.0.st);
            let e = st.list.get(&id)?;
            (e.job.clone(), e.out.clone())
        };
        let (text, off, len) = out.from(offset);
        j.output = text;
        j.offset = i64::try_from(off).unwrap_or(i64::MAX);
        j.output_len = i64::try_from(len).unwrap_or(i64::MAX);
        Some(j)
    }

    /// Every job, newest first, without its output.
    #[must_use]
    pub fn list(&self) -> Vec<Job> {
        let st = lock(&self.0.st);
        st.list
            .values()
            .rev()
            .map(|e| {
                let mut j = e.job.clone();
                j.output_len = i64::try_from(e.out.len()).unwrap_or(i64::MAX);
                j
            })
            .collect()
    }

    /// Cancels running job `id` (its process group killed); false when it is unknown or ended.
    pub fn cancel(&self, id: i64) -> bool {
        let kill = {
            let st = lock(&self.0.st);
            match st.list.get(&id) {
                Some(e) if e.job.status == JOB_RUNNING => e.kill.clone(),
                _ => return false,
            }
        };
        kill.kill(Ended::Canceled);
        true
    }

    /// Cancels every running job.
    pub fn cancel_all(&self) {
        let running: Vec<i64> = lock(&self.0.st)
            .list
            .iter()
            .filter(|(_, e)| e.job.status == JOB_RUNNING)
            .map(|(id, _)| *id)
            .collect();
        for id in running {
            self.cancel(id);
        }
    }

    /// Follows job `id` on `st` from absolute `offset` (`/node/job/follow`): a frame, a job with
    /// its new output, as it is written, then the stream ends with the job, without output, once
    /// it ended and all of it was sent.
    pub fn follow(&self, id: i64, offset: usize, st: Stream) {
        let out = {
            let g = lock(&self.0.st);
            match g.list.get(&id) {
                Some(e) => e.out.clone(),
                None => return st.end(Err(not_found(id))),
            }
        };
        let f = Arc::new(Follow {
            jobs: Arc::downgrade(&self.0),
            id,
            out,
            st: st.clone(),
            s: Mutex::new(FSt {
                offset,
                first: true,
                last_status: String::new(),
                keep: None,
                done: false,
            }),
        });
        let f2 = Arc::clone(&f);
        st.on_finish(move |_| f2.stop());
        f.arm_keep();
        f.step();
    }
}

fn not_found(id: i64) -> Error {
    Error::code(CODE_NOT_FOUND, format!("job {id}: job not found"))
}

struct FSt {
    offset: usize,
    first: bool,
    last_status: String,
    keep: Option<SendTimer>,
    done: bool,
}

/// One follower of a job.
struct Follow {
    jobs: Weak<Inner>,
    id: i64,
    out: OutBuf,
    st: Stream,
    s: Mutex<FSt>,
}

impl Follow {
    fn stop(&self) {
        let t = {
            let mut s = lock(&self.s);
            s.done = true;
            s.keep.take()
        };
        if let Some(t) = t {
            t.cancel();
        }
    }

    /// Sends the job's state after [`FOLLOW_KEEPALIVE`] without a frame.
    fn arm_keep(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        let t = after_send(FOLLOW_KEEPALIVE, move || {
            if let Some(f) = me.upgrade() {
                lock(&f.s).first = true;
                f.arm_keep();
                f.step();
            }
        })
        .ok();
        let old = {
            let mut s = lock(&self.s);
            if s.done {
                drop(s);
                if let Some(t) = t {
                    t.cancel();
                }
                return;
            }
            std::mem::replace(&mut s.keep, t)
        };
        if let Some(t) = old {
            t.cancel();
        }
    }

    fn step(self: &Arc<Self>) {
        let mut s = lock(&self.s);
        loop {
            if s.done {
                return;
            }
            let epoch = self.out.generation();
            let Some(jobs) = self.jobs.upgrade() else {
                s.done = true;
                drop(s);
                return self.st.end(Err(Error::Closed));
            };
            let Some(mut snap) = Jobs(jobs).get(self.id, s.offset) else {
                s.done = true;
                drop(s);
                return self.st.end(Err(not_found(self.id)));
            };
            if s.first || !snap.output.is_empty() || snap.status != s.last_status {
                match self.st.try_send(snap.to_json().as_bytes()) {
                    Ok(true) => {
                        s.first = false;
                        s.last_status.clone_from(&snap.status);
                        drop(s);
                        self.arm_keep();
                        s = lock(&self.s);
                    }
                    Ok(false) => {
                        let me = Arc::clone(self);
                        if self.st.on_credit(move || me.step()) {
                            return;
                        }
                        continue; // credit came meanwhile
                    }
                    Err(_) => {
                        s.done = true;
                        return; // the stream's end says why
                    }
                }
            }
            s.offset = usize::try_from(snap.output_len).unwrap_or(0);
            if snap.status != JOB_RUNNING {
                s.done = true;
                let keep = s.keep.take();
                drop(s);
                if let Some(t) = keep {
                    t.cancel();
                }
                snap.output.clear();
                return self.st.end(Ok(snap.to_json().into_bytes()));
            }
            drop(s);
            // The next change, then a batch's wait.
            let me = Arc::downgrade(self);
            self.out.on_change(epoch, move || {
                let _ = after_send(FOLLOW_BATCH, move || {
                    if let Some(f) = me.upgrade() {
                        f.step();
                    }
                });
            });
            return;
        }
    }
}

#[cfg(test)]
mod tests;
