//! A `fastcgi` handler's connections to its application (fit's `pool.go`): dialed through akio's
//! engine, one request on each at a time, idle ones kept for the next. Their number is bounded
//! (`max_conns`), as are the idle ones kept (`max_idle_conns`) and the requests that wait for one
//! when all are busy (`max_pending`; past it a request is refused). Connecting has its timeout
//! (`connect_timeout`), and an idle connection is closed after `idle_timeout`.
//!
//! Every deadline of the handler's (its exchanges', its idle connections', its dials') is checked
//! by one sweep every [`TICK`], a job on akio's timer thread ([`akio::timer::after_send`]) that
//! runs while the handler has anything to watch: one job a
//! handler, whatever its rate of requests, not one a request in a table all share.
//!
//! Nothing here blocks: a host name is resolved off the I/O threads by akio's resolver, and a
//! request that finds no connection free waits in the queue, as a callback. Locks: the pool's,
//! then a connection's, never the other way; neither is held while calling a waiter, which may
//! send on the connection it gets.

use std::collections::VecDeque;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use akio::{Acceptor, Conn};
use akit_config::FastCgiAddress;
use akit_upstream::EngineSlot;

use super::exchange::Exchange;
use super::record::Reader;

/// Why a request got no connection.
#[derive(Debug)]
pub(crate) enum DialError {
    /// `max_pending` requests already wait: answered 503.
    Busy,
    /// The handler's config is gone: answered 503.
    Closed,
    /// Connecting took longer than `connect_timeout`: answered 504.
    Timeout,
    /// Connecting failed: answered 502 (504 for a timeout).
    Io(io::Error),
}

/// What a request waiting for a connection is called with, once.
pub(crate) type Waiter = Box<dyn FnOnce(Result<Arc<FcgiConn>, DialError>) + Send>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the pool dials and how many it keeps.
#[derive(Debug)]
pub(crate) struct Limits {
    pub(crate) address: FastCgiAddress,
    pub(crate) connect_timeout: Duration,
    pub(crate) idle_timeout: Duration,
    pub(crate) max_conns: usize,
    pub(crate) max_idle: usize,
    pub(crate) max_pending: usize,
}

/// How often the handler's deadlines are checked.
pub(crate) const TICK: Duration = Duration::from_millis(100);

#[derive(Default)]
struct PoolState {
    /// Connections dialing, busy or idle.
    open: usize,
    /// The idle connections, and since when.
    idle: Vec<(Arc<FcgiConn>, Instant)>,
    waiting: VecDeque<Waiter>,
    closed: bool,
    /// The connections being dialed, and when they time out.
    dialing: Vec<(Weak<FcgiConn>, Instant)>,
    /// The exchanges under way, whose deadlines the sweep checks.
    exchanges: Vec<Weak<Exchange>>,
    /// The sweep's job is pending.
    ticking: bool,
}

/// A handler's connections.
pub(crate) struct Pool {
    limits: Limits,
    engine: EngineSlot,
    st: Mutex<PoolState>,
    me: Weak<Pool>,
}

impl std::fmt::Debug for Pool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pool")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl Pool {
    pub(crate) fn new(limits: Limits, engine: EngineSlot) -> Arc<Pool> {
        Arc::new_cyclic(|me| Pool {
            limits,
            engine,
            st: Mutex::new(PoolState::default()),
            me: me.clone(),
        })
    }

    /// Hands a connection to `w`: an idle one at once, a new one once it is dialed, or one
    /// another request gives back. `w` runs once, maybe on this thread, never under the pool's
    /// lock. With `idle` false, the idle connections are passed over (a request sent again after
    /// its connection went stale: the others may have gone with it).
    pub(crate) fn take(&self, w: Waiter, idle: bool) {
        let mut st = lock(&self.st);
        if st.closed {
            drop(st);
            return w(Err(DialError::Closed));
        }
        if idle {
            while let Some((c, _)) = st.idle.pop() {
                if c.is_open() {
                    drop(st);
                    return w(Ok(c));
                }
            }
        }
        if st.open >= self.limits.max_conns {
            if st.waiting.len() >= self.limits.max_pending {
                drop(st);
                return w(Err(DialError::Busy));
            }
            st.waiting.push_back(w);
            return;
        }
        st.open = st.open.saturating_add(1);
        drop(st);
        self.dial(w);
    }

    /// Opens a connection for `w`, which counts already among the open ones.
    fn dial(&self, w: Waiter) {
        let fc = Arc::new(FcgiConn {
            pool: self.me.clone(),
            conn: OnceLock::new(),
            counted: AtomicBool::new(true),
            st: Mutex::new(ConnState {
                dial: Some(w),
                ..ConnState::default()
            }),
        });
        let Some(engine) = self.engine.get().cloned() else {
            return fc.dial_failed(DialError::Closed);
        };
        {
            let now = Instant::now();
            let mut st = lock(&self.st);
            st.dialing.push((
                Arc::downgrade(&fc),
                now.checked_add(self.limits.connect_timeout).unwrap_or(now),
            ));
            // Without the sweep, akio's own connect timeout still bounds it.
            let _ = self.sweep(&mut st);
        }
        let acceptor = Dialed(fc.clone());
        let on_error = {
            let fc = fc.clone();
            move |e: io::Error| fc.dial_failed(DialError::Io(e))
        };
        match &self.limits.address {
            FastCgiAddress::Tcp { host, port } => {
                let mut he = akio::dns::HappyEyeballs::default();
                he.connect_timeout = self.limits.connect_timeout;
                match host.parse::<IpAddr>() {
                    Ok(ip) => he.dial(
                        &engine,
                        vec![SocketAddr::new(ip, *port)],
                        acceptor,
                        on_error,
                    ),
                    Err(_) => he.connect(&engine, host, *port, acceptor, on_error),
                }
            }
            FastCgiAddress::Unix(path) => {
                engine.dial_unix(std::path::Path::new(path), acceptor, on_error);
            }
        }
    }

    /// Gives a connection back after a request: to a request waiting for one, to the idle set
    /// (closed after `idle_timeout`), or closed.
    pub(crate) fn put(&self, c: Arc<FcgiConn>) {
        let mut st = lock(&self.st);
        if st.closed {
            drop(st);
            return c.close();
        }
        if let Some(w) = st.waiting.pop_front() {
            drop(st);
            return w(Ok(c));
        }
        if st.idle.len() >= self.limits.max_idle {
            drop(st);
            return c.close();
        }
        st.idle.push((c, Instant::now()));
        if !self.sweep(&mut st) {
            // No sweep to close it later: not kept.
            if let Some((c, _)) = st.idle.pop() {
                drop(st);
                c.close();
            }
        }
    }

    /// Has the sweep run, if it does not already; false when it cannot be scheduled. `st` is
    /// the pool's state, locked.
    fn sweep(&self, st: &mut PoolState) -> bool {
        if st.ticking {
            return true;
        }
        let pool = self.me.clone();
        st.ticking = akio::timer::after_send(TICK, move || {
            if let Some(p) = pool.upgrade() {
                p.tick();
            }
        })
        .is_ok();
        st.ticking
    }

    /// Has the sweep watch `x`'s deadlines; false when it cannot.
    pub(crate) fn watch(&self, x: &Arc<Exchange>) -> bool {
        let mut st = lock(&self.st);
        st.exchanges.push(Arc::downgrade(x));
        if self.sweep(&mut st) {
            return true;
        }
        st.exchanges.pop();
        false
    }

    /// The sweep: exchanges past a deadline fail, idle connections past `idle_timeout` close,
    /// dials past `connect_timeout` fail. It runs again while anything is left to watch.
    fn tick(&self) {
        let now = Instant::now();
        let (exchanges, expired, dials) = {
            let mut st = lock(&self.st);
            st.ticking = false;
            st.exchanges
                .retain(|w| w.upgrade().is_some_and(|x| !x.finished()));
            let exchanges: Vec<Arc<Exchange>> =
                st.exchanges.iter().filter_map(Weak::upgrade).collect();
            let idle_timeout = self.limits.idle_timeout;
            let mut expired = Vec::new();
            st.idle.retain(|(c, since)| {
                let keep = now.saturating_duration_since(*since) < idle_timeout;
                if !keep {
                    expired.push(c.clone());
                }
                keep
            });
            let mut dials = Vec::new();
            st.dialing.retain(|(w, deadline)| match w.upgrade() {
                Some(c) if c.dialing() => {
                    if now >= *deadline {
                        dials.push(c);
                        false
                    } else {
                        true
                    }
                }
                _ => false,
            });
            (exchanges, expired, dials)
        };
        for x in &exchanges {
            x.check(now);
        }
        for c in expired {
            c.close();
        }
        for c in dials {
            c.dial_failed(DialError::Timeout);
        }
        let mut st = lock(&self.st);
        st.exchanges
            .retain(|w| w.upgrade().is_some_and(|x| !x.finished()));
        if !st.exchanges.is_empty() || !st.idle.is_empty() || !st.dialing.is_empty() {
            let _ = self.sweep(&mut st);
        }
    }

    /// Counts a connection closed, or never opened: a request waiting for one dials in its
    /// place. Once a connection.
    fn lost(&self, c: &Arc<FcgiConn>) {
        if !c.counted.swap(false, Ordering::AcqRel) {
            return;
        }
        let mut st = lock(&self.st);
        if let Some(i) = st.idle.iter().position(|(ic, _)| Arc::ptr_eq(ic, c)) {
            st.idle.swap_remove(i);
        }
        if !st.closed {
            if let Some(w) = st.waiting.pop_front() {
                drop(st);
                return self.dial(w);
            }
        }
        st.open = st.open.saturating_sub(1);
    }

    /// Closes the idle connections and fails the requests waiting: the handler's config is gone.
    pub(crate) fn close(&self) {
        let (idle, waiting) = {
            let mut st = lock(&self.st);
            st.closed = true;
            (
                std::mem::take(&mut st.idle),
                std::mem::take(&mut st.waiting),
            )
        };
        for (c, _) in idle {
            c.close();
        }
        for w in waiting {
            w(Err(DialError::Closed));
        }
    }
}

#[derive(Default)]
struct ConnState {
    /// The request the connection carries, or none while idle.
    x: Option<Arc<Exchange>>,
    /// The requests the connection has carried.
    served: u32,
    /// The application sent something for the request the connection carries.
    heard: bool,
    closed: bool,
    /// Who waits for the connection while it is dialed.
    dial: Option<Waiter>,
}

/// A connection to the application, and the request on it.
pub(crate) struct FcgiConn {
    pool: Weak<Pool>,
    conn: OnceLock<Conn<Link>>,
    /// Counted among the pool's open connections: cleared once, when it is lost.
    counted: AtomicBool,
    st: Mutex<ConnState>,
}

impl std::fmt::Debug for FcgiConn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FcgiConn").finish_non_exhaustive()
    }
}

impl FcgiConn {
    /// Dialing failed or took too long: the waiter hears it, if it still waits.
    fn dial_failed(self: &Arc<Self>, e: DialError) {
        let w = {
            let mut st = lock(&self.st);
            let w = st.dial.take();
            if w.is_some() {
                st.closed = true;
            }
            w
        };
        let Some(w) = w else { return };
        if let Some(p) = self.pool.upgrade() {
            p.lost(self);
        }
        w(Err(e));
    }

    /// Whether the connection has not closed.
    fn is_open(&self) -> bool {
        !lock(&self.st).closed
    }

    /// Whether the connection is still being dialed.
    fn dialing(&self) -> bool {
        lock(&self.st).dial.is_some()
    }

    /// Gives the connection to `x`.
    pub(crate) fn start(&self, x: Arc<Exchange>) {
        let mut st = lock(&self.st);
        st.x = Some(x);
        st.served = st.served.saturating_add(1);
        st.heard = false;
    }

    /// Whether the connection carried a request before the one it carries and the application
    /// has sent nothing for this one: an idle connection the application closed, maybe before
    /// it read the request.
    pub(crate) fn stale(&self) -> bool {
        let st = lock(&self.st);
        st.served > 1 && !st.heard
    }

    /// Takes the request `x` off the connection, which goes back to the pool when `reuse` says
    /// it can carry another, else is closed. Nothing when `x` is not the connection's request.
    pub(crate) fn release(self: &Arc<Self>, x: &Arc<Exchange>, reuse: bool) {
        let closed = {
            let mut st = lock(&self.st);
            if !st.x.as_ref().is_some_and(|y| Arc::ptr_eq(y, x)) {
                return;
            }
            st.x = None;
            st.closed
        };
        if closed {
            return;
        }
        match (reuse, self.pool.upgrade()) {
            (true, Some(p)) => p.put(self.clone()),
            _ => self.close(),
        }
    }

    /// Gives back a connection a request got but no longer wants (it ended meanwhile).
    pub(crate) fn give_back(self: &Arc<Self>) {
        match self.pool.upgrade() {
            Some(p) => p.put(self.clone()),
            None => self.close(),
        }
    }

    pub(crate) fn send(&self, b: Vec<u8>) -> akio::Result<()> {
        match self.conn.get() {
            Some(c) => c.send_owned(b),
            None => Err(akio::Error::Closed),
        }
    }

    pub(crate) fn queued_bytes(&self) -> usize {
        self.conn.get().map_or(0, Conn::queued_bytes)
    }

    /// Calls `f` once what is queued is written, or the connection closed.
    pub(crate) fn on_drain(&self, f: impl FnOnce() + Send + 'static) {
        match self.conn.get() {
            Some(c) => c.on_drain(f),
            None => f(),
        }
    }

    pub(crate) fn hold_reads(&self, hold: bool) {
        if let Some(c) = self.conn.get() {
            c.hold_reads(hold);
        }
    }

    pub(crate) fn close(&self) {
        if let Some(c) = self.conn.get() {
            c.close();
        }
    }
}

/// The acceptor of a connection being dialed.
pub(crate) struct Dialed(Arc<FcgiConn>);

impl Acceptor for Dialed {
    type Handler = Link;

    fn on_open(&self, c: &Conn<Link>) -> Link {
        let fc = self.0.clone();
        let _ = fc.conn.set(c.clone());
        let w = lock(&fc.st).dial.take();
        match w {
            Some(w) => w(Ok(fc.clone())),
            // Given up (its connect timeout passed): not wanted.
            None => c.close(),
        }
        Link {
            fc,
            reader: Reader::default(),
        }
    }
}

/// A connection's handler on its I/O thread: reads the records the application sends, for the
/// request on the connection; anything while idle breaks the protocol.
pub(crate) struct Link {
    fc: Arc<FcgiConn>,
    reader: Reader,
}

impl akio::Handler for Link {
    fn on_data(&mut self, c: &Conn<Self>, data: &[u8]) {
        let x = {
            let mut st = lock(&self.fc.st);
            st.heard = true;
            st.x.clone()
        };
        let Some(x) = x else {
            return c.close();
        };
        let fc = self.fc.clone();
        super::exchange::records(&x, &fc, &mut self.reader, c, data);
    }

    fn on_close(&mut self, _c: &Conn<Self>, err: Option<akio::Error>) {
        let (x, stale) = {
            let mut st = lock(&self.fc.st);
            st.closed = true;
            (st.x.take(), st.served > 1 && !st.heard)
        };
        if let Some(p) = self.fc.pool.upgrade() {
            p.lost(&self.fc);
        }
        if let Some(x) = x {
            let why = err.map_or_else(
                || "the application closed the connection".to_owned(),
                |e| e.to_string(),
            );
            x.lost(&self.fc, why, stale);
        }
    }
}
