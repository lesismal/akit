//! The loop the providers that follow a service's API share (Consul, Consul KV, etcd): fit's
//! `provider/internal/watch`, on akio's model.
//!
//! A [`Follower`] runs on a thread of its own in the control plane, as fit's runs on a
//! goroutine. It opens a [`Source`], starts its watch on another thread of its own, and reads the
//! source whole once the watch is in place, after changes (debounced) and every interval,
//! sending the part it makes when it changed (or always, on the interval). When the watch or a
//! read fails, the last part stays, and the source is tried again, from [`Timings::min_backoff`]
//! to [`Timings::max_backoff`] apart. Requests go through akio's HTTP client, whose callbacks
//! run on akio's I/O threads and never block: they hand their outcome to the thread that waits.
//!
//! Everything a follower waits on is a [`Stop`]: stopping the provider cancels the round under
//! way, its requests (through their [`CancelToken`]) and its sleeps, and the threads it started
//! are joined before the provider's `stop` returns.

use std::collections::HashSet;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use akio_http::client::CancelToken;

use crate::run::Update;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a provider reports: a line, without the `akit: ` its caller adds.
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// The `provider` logger (I82), at warn, as akit reports what its providers find: standard error
/// until akit's log is installed.
#[must_use]
pub fn stderr_log() -> Log {
    akit_observe::logger::lines(
        akit_observe::logger::Logger::Provider,
        akit_observe::logger::Level::Warn,
        "",
    )
}

/// A follower's timings: fit's `watch.Debounce`, `MinBackoff` and `MaxBackoff`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timings {
    /// How long a follower waits after a change for more, before reading the source again.
    pub(crate) debounce: Duration,
    /// The least wait before trying the source again after it failed.
    pub(crate) min_backoff: Duration,
    /// The most.
    pub(crate) max_backoff: Duration,
}

impl Default for Timings {
    fn default() -> Timings {
        Timings {
            debounce: Duration::from_millis(200),
            min_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(30),
        }
    }
}

type Hook = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct StopState {
    stopped: bool,
    children: Vec<Stop>,
    hooks: Vec<Hook>,
}

struct StopInner {
    state: Mutex<StopState>,
    cv: Condvar,
    token: CancelToken,
}

/// Stops what waits on it: its sleeps, its requests (their [`CancelToken`]), what was hooked
/// to it, and its children. Cheap to clone; usable from any thread, an akio I/O thread among
/// them (a short lock, no I/O).
#[derive(Clone)]
pub(crate) struct Stop(Arc<StopInner>);

impl std::fmt::Debug for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stop")
            .field("stopped", &self.is_stopped())
            .finish()
    }
}

impl Stop {
    pub(crate) fn new() -> Stop {
        Stop(Arc::new(StopInner {
            state: Mutex::new(StopState::default()),
            cv: Condvar::new(),
            token: CancelToken::new(),
        }))
    }

    /// Stops it, and its children; the hooks run on this thread.
    pub(crate) fn cancel(&self) {
        let (children, hooks) = {
            let mut st = lock(&self.0.state);
            if st.stopped {
                return;
            }
            st.stopped = true;
            (
                std::mem::take(&mut st.children),
                std::mem::take(&mut st.hooks),
            )
        };
        self.0.cv.notify_all();
        self.0.token.cancel();
        for c in children {
            c.cancel();
        }
        for h in hooks {
            h();
        }
    }

    pub(crate) fn is_stopped(&self) -> bool {
        lock(&self.0.state).stopped
    }

    /// The token the requests made under it are cancelled with.
    pub(crate) fn token(&self) -> &CancelToken {
        &self.0.token
    }

    /// Waits for `d`, or until stopped: whether it waited the whole of `d`.
    pub(crate) fn sleep(&self, d: Duration) -> bool {
        let until = Instant::now().checked_add(d);
        let mut st = lock(&self.0.state);
        loop {
            if st.stopped {
                return false;
            }
            let left = match until {
                Some(u) => u.saturating_duration_since(Instant::now()),
                None => Duration::from_secs(3600),
            };
            if left.is_zero() {
                return true;
            }
            st = self
                .0
                .cv
                .wait_timeout(st, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// A stop stopped with this one, or on its own. Born stopped when this one is.
    pub(crate) fn child(&self) -> Stop {
        let c = Stop::new();
        let mut st = lock(&self.0.state);
        if st.stopped {
            drop(st);
            c.cancel();
            return c;
        }
        // The children stopped already are let go: those kept are those live.
        st.children.retain(|c| !c.is_stopped());
        st.children.push(c.clone());
        c
    }

    /// Runs `f` when stopped: at once, here, when it is already.
    pub(crate) fn on_cancel(&self, f: impl FnOnce() + Send + 'static) {
        let mut st = lock(&self.0.state);
        if st.stopped {
            drop(st);
            f();
            return;
        }
        st.hooks.push(Box::new(f));
    }
}

/// What a watch tells its follower: that it is in place, and that the source changed. Both are
/// a short lock: callable on an akio I/O thread.
#[derive(Clone)]
pub(crate) struct Events(Arc<Signals>);

#[derive(Default)]
struct Sig {
    ready: bool,
    changed: bool,
    /// The watch ended, with its error.
    ended: Option<String>,
    stopped: bool,
}

#[derive(Default)]
struct Signals {
    sig: Mutex<Sig>,
    cv: Condvar,
}

impl Events {
    fn new() -> Events {
        Events(Arc::new(Signals::default()))
    }

    /// The watch is in place: no change after a read that follows is missed.
    pub(crate) fn ready(&self) {
        lock(&self.0.sig).ready = true;
        self.0.cv.notify_all();
    }

    /// The source changed (one pending stands for any number).
    pub(crate) fn changed(&self) {
        lock(&self.0.sig).changed = true;
        self.0.cv.notify_all();
    }

    fn ended(&self, err: String) {
        lock(&self.0.sig).ended.get_or_insert(err);
        self.0.cv.notify_all();
    }

    fn stopped(&self) {
        lock(&self.0.sig).stopped = true;
        self.0.cv.notify_all();
    }
}

/// What a follower follows: fit's `watch.Source`.
pub(crate) trait Source: Send + 'static {
    /// The connection a watch and its reads share.
    type Conn: Conn;

    /// Readies a connection to the source for a watch and the reads that go with it: a client
    /// with its files read. Blocks (it reads files): on the follower's thread.
    fn open(&mut self, stop: &Stop) -> Result<Arc<Self::Conn>, String>;
}

/// A source's connection, shared by its watch's thread and its follower's.
pub(crate) trait Conn: Send + Sync + 'static {
    /// Watches the source until `stop` or the watch fails, returning why it ended (`Ok` when it
    /// ended without an error). Calls `ev.ready()` once the watch is in place, and
    /// `ev.changed()` after each change from then on. Blocks: on a thread of its own.
    fn watch(&self, stop: &Stop, ev: &Events) -> Result<(), String>;

    /// Reads the source whole: the part it makes, and why it left out what it did. Blocks: on
    /// the follower's thread.
    fn read(&self, stop: &Stop) -> Result<(Vec<u8>, Vec<String>), String>;
}

/// Follows a source and sends the parts it makes: fit's `watch.Follower`.
pub(crate) struct Follower<S: Source> {
    /// The provider's name.
    pub(crate) name: String,
    /// How often the source is read whether or not it changed.
    pub(crate) interval: Duration,
    pub(crate) timings: Timings,
    pub(crate) log: Log,
    pub(crate) source: S,
    /// The part sent last.
    last: Option<Vec<u8>>,
    /// The problems reported at the last read.
    problems: HashSet<String>,
}

impl<S: Source> Follower<S> {
    pub(crate) fn new(name: &str, interval: Duration, log: Log, source: S) -> Follower<S> {
        Follower {
            name: name.to_owned(),
            interval,
            timings: Timings::default(),
            log,
            source,
            last: None,
            problems: HashSet::new(),
        }
    }

    /// Follows the source until `stop`, sending the part it makes through `update`: at once,
    /// after each change, and every interval whether or not it changed, so that a part refused
    /// because of the rest of the config is tried again. When the source cannot be reached,
    /// the last part stays, and the source is tried again, backing off.
    pub(crate) fn run(&mut self, stop: &Stop, update: &dyn Fn(Vec<u8>)) {
        let mut backoff = self.timings.min_backoff;
        loop {
            let round = stop.child();
            let (read, err) = self.follow(&round, update);
            round.cancel();
            if stop.is_stopped() {
                return;
            }
            if read {
                backoff = self.timings.min_backoff;
            }
            (self.log)(&format!(
                "provider {}: {err}; trying again in {}",
                self.name,
                akit_config::duration::format(backoff)
            ));
            if !stop.sleep(backoff) {
                return;
            }
            backoff = backoff.saturating_mul(2).min(self.timings.max_backoff);
        }
    }

    /// Watches the source, and reads it once the watch is in place, after changes and every
    /// interval, until the source fails or `round` is stopped. Says whether a read succeeded,
    /// and why it ended.
    fn follow(&mut self, round: &Stop, update: &dyn Fn(Vec<u8>)) -> (bool, String) {
        let conn = match self.source.open(round) {
            Ok(c) => c,
            Err(e) => return (false, e),
        };
        let ev = Events::new();
        {
            let ev = ev.clone();
            round.on_cancel(move || ev.stopped());
        }
        let watcher = {
            let (conn, round, ev) = (Arc::clone(&conn), round.clone(), ev.clone());
            std::thread::Builder::new()
                .name(format!("akit-provider-{}", self.name))
                .spawn(move || {
                    let r = conn.watch(&round, &ev);
                    ev.ended(match r {
                        Ok(()) => "the watch ended".to_owned(),
                        Err(e) => e,
                    });
                })
        };
        let watcher = match watcher {
            Ok(w) => w,
            Err(e) => return (false, format!("watch: {e}")),
        };
        let r = self.follow_watch(&conn, round, &ev, update);
        // The watch is done with the connection before it is let go.
        round.cancel();
        join(watcher);
        r
    }

    fn follow_watch(
        &mut self,
        conn: &Arc<S::Conn>,
        round: &Stop,
        ev: &Events,
        update: &dyn Fn(Vec<u8>),
    ) -> (bool, String) {
        // Until the watch is in place.
        {
            let mut sig = lock(&ev.0.sig);
            loop {
                if sig.stopped {
                    return (false, "stopped".to_owned());
                }
                if let Some(e) = sig.ended.take() {
                    return (false, format!("watch: {e}"));
                }
                if sig.ready {
                    // A change before the first read is in it.
                    sig.changed = false;
                    break;
                }
                sig = ev.0.cv.wait(sig).unwrap_or_else(PoisonError::into_inner);
            }
        }
        if let Err(e) = self.refresh(conn, round, update, true) {
            return (false, e);
        }
        let mut tick = Instant::now().checked_add(self.interval);
        let mut fire: Option<Instant> = None;
        loop {
            let what = {
                let mut sig = lock(&ev.0.sig);
                loop {
                    if sig.stopped {
                        return (true, "stopped".to_owned());
                    }
                    if let Some(e) = sig.ended.take() {
                        return (true, format!("watch: {e}"));
                    }
                    if std::mem::take(&mut sig.changed) && fire.is_none() {
                        fire = Instant::now().checked_add(self.timings.debounce);
                    }
                    let now = Instant::now();
                    if fire.is_some_and(|f| now >= f) {
                        fire = None;
                        break false;
                    }
                    if tick.is_some_and(|t| now >= t) {
                        tick = now.checked_add(self.interval);
                        break true;
                    }
                    let next = [fire, tick].into_iter().flatten().min();
                    sig = match next {
                        Some(n) => {
                            ev.0.cv
                                .wait_timeout(sig, n.saturating_duration_since(now))
                                .unwrap_or_else(PoisonError::into_inner)
                                .0
                        }
                        None => ev.0.cv.wait(sig).unwrap_or_else(PoisonError::into_inner),
                    };
                }
            };
            if let Err(e) = self.refresh(conn, round, update, what) {
                return (true, e);
            }
        }
    }

    /// Reads the source and sends the part it makes, when it changed or `always`, reporting
    /// each problem once while it lasts.
    fn refresh(
        &mut self,
        conn: &Arc<S::Conn>,
        round: &Stop,
        update: &dyn Fn(Vec<u8>),
        always: bool,
    ) -> Result<(), String> {
        let (part, problems) = conn.read(round)?;
        let mut reported = HashSet::with_capacity(problems.len());
        for msg in problems {
            if !self.problems.contains(&msg) {
                (self.log)(&format!("provider {}: {msg}", self.name));
            }
            reported.insert(msg);
        }
        self.problems = reported;
        if always || self.last.as_ref() != Some(&part) {
            self.last = Some(part.clone());
            update(part);
        }
        Ok(())
    }
}

fn join(h: JoinHandle<()>) {
    // A watch that panicked is a bug, reported by the panic itself.
    let _ = h.join();
}

/// A follower run on a thread of its own: what a provider holds while it watches.
pub(crate) struct Running {
    stop: Stop,
    thread: Option<JoinHandle<()>>,
}

impl Running {
    /// Starts `f` on a thread of its own, sending its parts through `update`.
    pub(crate) fn start<S: Source>(mut f: Follower<S>, update: Update) -> std::io::Result<Running> {
        let stop = Stop::new();
        let thread = {
            let stop = stop.clone();
            std::thread::Builder::new()
                .name(format!("akit-provider-{}", f.name))
                .spawn(move || f.run(&stop, &|part| update.send(part)))?
        };
        Ok(Running {
            stop,
            thread: Some(thread),
        })
    }

    /// Stops the follower and waits for its threads: no part is sent after it returns.
    pub(crate) fn stop(&mut self) {
        self.stop.cancel();
        if let Some(t) = self.thread.take() {
            join(t);
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests;
