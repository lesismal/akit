//! fit's mesh (`llm/internal/mesh`): the links between a hub, its nodes and its proxies, on
//! [`wire`](crate::wire) sessions over WebSockets. A role listens (its `listen`) and dials its
//! `peers`; whichever side dials says hello first ([`METHOD_HELLO`]), and both learn each other's
//! identity (role and stable ID). From then on both ends serve their methods, streams and pushes
//! on the link, each method taking the caller roles it names ([`Router`]).
//!
//! A peer's links, whichever side dialed them, are its pool: calls go round robin over those not
//! draining. A dial target is dialed again after its link is lost, after a backoff with jitter;
//! one whose peer refused the hello waits longer. Everything is driven by callbacks and akio's
//! timers: no thread waits on a link.

mod dial;
mod listen;
pub mod router;

pub use dial::{DIAL_TIMEOUT, backoff, refused_backoff};
pub use listen::{ListenState, fingerprint, validate_addr};
pub use router::{Answer, DEFAULT_TIMEOUT, Router};

use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, RwLock, Weak};
use std::time::{Duration, Instant};

use akio::Engine;
use akio::timer::after_send;
use akit_roles::linktoken::{Registry, Use};

use crate::model::{
    CODE_BAD_REQUEST, CODE_FORBIDDEN, CODE_UNAUTHORIZED, CODE_UNAVAILABLE, DialTarget, Identity,
    LinkStats, METHOD_HELLO, PeerConn, ROLE_LOCAL,
};
use crate::wire::{self, Error, Reply, Request, Session, Stream};

use dial::Target;
use listen::Lst;

/// How long an incoming link may stay without a hello.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(15);
/// The inbound links of a listener, by default.
pub const DEFAULT_MAX_CONNS: usize = 10_000;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn unix_now() -> i64 {
    akit_roles::linktoken::unix_now()
}

/// The peer is offline: no usable link.
#[must_use]
pub fn offline() -> Error {
    Error::code(CODE_UNAVAILABLE, "peer is offline")
}

/// A bad request.
#[must_use]
pub fn bad_request(msg: impl Into<String>) -> Error {
    Error::code(CODE_BAD_REQUEST, msg)
}

/// The version a role says hello with (decision 038: akit's).
#[must_use]
pub fn version() -> String {
    format!("akit {}", env!("CARGO_PKG_VERSION"))
}

/// The bounds of a mesh's links.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The heartbeat's interval.
    pub ping: Duration,
    /// The longest wait between two dials of an unreachable target.
    pub retry: Duration,
    /// How long a ping may wait for its pong.
    pub slow: Duration,
    /// The listener's inbound links.
    pub max_conns: usize,
    /// The calls and streams a peer may have running here per link.
    pub max_inflight: usize,
    /// A stream's idle timeout.
    pub stream_idle: Duration,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            ping: Duration::from_secs(15),
            retry: Duration::from_secs(5),
            slow: wire::conn::DEFAULT_SLOW_TIMEOUT,
            max_conns: DEFAULT_MAX_CONNS,
            max_inflight: wire::session::DEFAULT_MAX_INBOUND,
            stream_idle: wire::session::DEFAULT_STREAM_IDLE,
        }
    }
}

impl Limits {
    /// A role's `limits`, its defaults filled in.
    #[must_use]
    pub fn of(l: Option<&akit_config::roles::LlmLimits>) -> Limits {
        use akit_config::roles::LlmLimits as L;
        Limits {
            ping: L::ping(l),
            retry: L::retry(l),
            slow: L::slow(l),
            max_conns: L::conns(l),
            max_inflight: L::inflight(l),
            stream_idle: L::idle(l),
        }
    }

    fn session(&self) -> wire::Options {
        wire::Options {
            max_inbound: self.max_inflight,
            stream_idle: self.stream_idle,
            ..wire::Options::default()
        }
    }

    fn timing(&self) -> wire::conn::Timing {
        wire::conn::Timing {
            ping: self.ping,
            slow: self.slow,
        }
    }
}

/// What a mesh is: whose it is and whom it links with.
#[derive(Clone)]
pub struct Config {
    /// The role's identity, asked at every hello.
    pub identity: Arc<dyn Fn() -> Identity + Send + Sync>,
    /// The peer roles it links with, both ways; its own is always refused.
    pub accept_roles: Vec<String>,
    /// The engine its listener is bound on and its links are dialed on.
    pub engine: Arc<Engine>,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("accept_roles", &self.accept_roles)
            .finish_non_exhaustive()
    }
}

/// A push's handler: the peer that sent it, the JSON.
pub type PushFn = Arc<dyn Fn(&Identity, &[u8]) + Send + Sync>;
type PeerFn = Arc<dyn Fn(&PeerConn) + Send + Sync>;

struct PushRoute {
    roles: Vec<String>,
    f: PushFn,
}

#[derive(Default)]
struct CState {
    registered: bool,
    closed: bool,
    peer: PeerConn,
    token_use: Option<Use>,
}

/// One link of the mesh, inbound or outbound.
pub(crate) struct MConn {
    wc: Arc<wire::Conn>,
    id: i64,
    out: bool,
    remote: String,
    target: Option<Arc<Target>>,
    st: Mutex<CState>,
}

impl MConn {
    fn live(&self) -> bool {
        !self.wc.session().is_closed()
    }

    fn usable(&self) -> bool {
        self.live() && !self.wc.session().draining()
    }

    fn dir(&self) -> &'static str {
        if self.out { "out" } else { "in" }
    }

    fn peer(&self) -> Option<Identity> {
        let g = lock(&self.st);
        g.registered.then(|| g.peer.identity.clone())
    }
}

#[derive(Default)]
struct St {
    /// Every link, by its wire link's address.
    conns: HashMap<usize, Arc<MConn>>,
    /// The links that said hello, by peer ID.
    by_id: HashMap<String, Vec<Arc<MConn>>>,
    targets: HashMap<String, Arc<Target>>,
}

fn key(wc: &Arc<wire::Conn>) -> usize {
    Arc::as_ptr(wc) as usize
}

pub(crate) struct Inner {
    me: Weak<Inner>,
    cfg: Config,
    limits: Mutex<Limits>,
    router: Router,
    pushes: RwLock<HashMap<String, PushRoute>>,
    on_up: RwLock<Vec<PeerFn>>,
    on_down: RwLock<Vec<PeerFn>>,
    st: Mutex<St>,
    lst: Mutex<Lst>,
    tokens: Registry,
    draining: AtomicBool,
    stopped: AtomicBool,
    round: AtomicU64,
    next_conn: AtomicI64,
    hello_wait: Mutex<Duration>,
    filter: Mutex<Option<Arc<akit_access::Policy>>>,
    quiet: (Mutex<()>, Condvar),
}

/// One role's side of the mesh. Cheap to clone.
#[derive(Clone)]
pub struct Mesh(Arc<Inner>);

impl fmt::Debug for Mesh {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mesh")
            .field("accept_roles", &self.0.cfg.accept_roles)
            .finish_non_exhaustive()
    }
}

impl Mesh {
    /// A mesh serving `router`'s methods; it neither listens nor dials until
    /// [`Mesh::configure`].
    #[must_use]
    pub fn new(cfg: Config, router: Router) -> Mesh {
        Mesh(Arc::new_cyclic(|me| Inner {
            me: me.clone(),
            cfg,
            limits: Mutex::new(Limits::default()),
            router,
            pushes: RwLock::new(HashMap::new()),
            on_up: RwLock::new(Vec::new()),
            on_down: RwLock::new(Vec::new()),
            st: Mutex::new(St::default()),
            lst: Mutex::new(Lst::default()),
            tokens: Registry::default(),
            draining: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            round: AtomicU64::new(0),
            next_conn: AtomicI64::new(0),
            hello_wait: Mutex::new(HELLO_TIMEOUT),
            filter: Mutex::new(None),
            quiet: (Mutex::new(()), Condvar::new()),
        }))
    }

    /// The tokens the listener issued, and the links that use them.
    #[must_use]
    pub fn tokens(&self) -> Registry {
        self.0.tokens.clone()
    }

    /// This role's identity.
    #[must_use]
    pub fn identity(&self) -> Identity {
        (self.0.cfg.identity)()
    }

    /// The router.
    #[must_use]
    pub fn router(&self) -> &Router {
        &self.0.router
    }

    /// Sets the bounds of the links made from now on; a listener whose bounds changed starts
    /// again at the next [`Mesh::configure`].
    pub fn set_limits(&self, l: Limits) {
        *lock(&self.0.limits) = l;
    }

    /// The bounds.
    #[must_use]
    pub fn limits(&self) -> Limits {
        *lock(&self.0.limits)
    }

    /// Shortens how long an incoming link may go without a hello (tests).
    pub fn set_hello_wait(&self, d: Duration) {
        *lock(&self.0.hello_wait) = d;
    }

    /// Sets the access policy the listener takes connections by: for those it accepts from now
    /// on.
    pub fn set_filter(&self, p: Option<Arc<akit_access::Policy>>) {
        *lock(&self.0.filter) = p.clone();
        self.0.refilter(p);
    }

    /// Runs `f` for the pushes of `method` from peers of `roles`; others are dropped.
    pub fn on_push(
        &self,
        method: &str,
        roles: &[&str],
        f: impl Fn(&Identity, &[u8]) + Send + Sync + 'static,
    ) {
        self.0
            .pushes
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                method.to_owned(),
                PushRoute {
                    roles: roles.iter().map(|r| (*r).to_owned()).collect(),
                    f: Arc::new(f),
                },
            );
    }

    /// Runs `f` when a link says hello (on the link's thread; it must not block).
    pub fn on_peer_up(&self, f: impl Fn(&PeerConn) + Send + Sync + 'static) {
        self.0
            .on_up
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::new(f));
    }

    /// Runs `f` when a link that said hello is lost.
    pub fn on_peer_down(&self, f: impl Fn(&PeerConn) + Send + Sync + 'static) {
        self.0
            .on_down
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::new(f));
    }

    /// Moves the mesh to `listen` and `peers`: the listener starts, changes or stops (an empty
    /// address: none); the targets not in `peers` are dropped, those in it added or changed.
    /// What did not change, its links among it, is left alone. A listener that cannot bind is
    /// logged and kept in [`Mesh::listen_state`], so that the role still runs. Binds sockets
    /// and reads files: the control thread's.
    ///
    /// # Errors
    ///
    /// The listener's failure and the targets skipped as invalid, joined.
    pub fn configure(
        &self,
        listen: &crate::model::ListenConfig,
        peers: &[crate::model::DialReq],
    ) -> Result<(), String> {
        let mut errs = Vec::new();
        if let Err(e) = self.set_listen(listen) {
            akit_observe::error!(Llm, "mesh: listener not started: {}: {e}", listen.addr);
            errs.push(format!("listen {}: {e}", listen.addr));
        }
        let mut keep = Vec::new();
        for p in peers {
            match self.dial(p) {
                Ok(()) => {
                    if let Ok(a) = validate_addr(&p.addr, true) {
                        keep.push(a);
                    }
                }
                Err(e) => errs.push(format!("dial target {}: {e}", p.addr)),
            }
        }
        let drop: Vec<String> = lock(&self.0.st)
            .targets
            .keys()
            .filter(|a| !keep.contains(a))
            .cloned()
            .collect();
        for a in drop {
            self.undial(&a);
        }
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs.join("; "))
        }
    }

    /// Stops the mesh: every link and the listener closed, no dial again.
    pub fn stop(&self) {
        if self.0.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        let targets: Vec<Arc<Target>> = lock(&self.0.st).targets.drain().map(|(_, t)| t).collect();
        for t in targets {
            self.0.stop_target(&t);
        }
        self.0.close_conns(|_| true);
        self.0.stop_listener();
    }

    /// Drains the mesh, then stops it: the listener refuses new links, every link is told to
    /// drain, and the calls and streams under way get `grace` to end. Waits: the control
    /// thread's.
    pub fn shutdown(&self, grace: Duration) {
        if !self.0.draining.swap(true, Ordering::AcqRel) {
            let conns: Vec<Arc<MConn>> = lock(&self.0.st).conns.values().cloned().collect();
            for c in &conns {
                c.wc.session().drain();
            }
            akit_observe::info!(
                Llm,
                "mesh draining: {} links, {} calls and streams under way",
                conns.len(),
                self.inflight()
            );
            let deadline = Instant::now()
                .checked_add(grace)
                .unwrap_or_else(Instant::now);
            let (m, cv) = &self.0.quiet;
            let mut g = lock(m);
            while self.inflight() > 0 {
                let now = Instant::now();
                if now >= deadline {
                    akit_observe::warn!(
                        Llm,
                        "mesh: shutdown grace expired, closing with {} under way",
                        self.inflight()
                    );
                    break;
                }
                g = cv
                    .wait_timeout(g, deadline.saturating_duration_since(now))
                    .map_or_else(|e| e.into_inner().0, |r| r.0);
            }
        }
        self.stop();
    }

    /// The calls and streams under way on every link.
    #[must_use]
    pub fn inflight(&self) -> usize {
        lock(&self.0.st)
            .conns
            .values()
            .map(|c| c.wc.session().inflight())
            .sum()
    }

    fn pick(&self, id: &str) -> Option<Arc<MConn>> {
        let list = lock(&self.0.st).by_id.get(id).cloned()?;
        let n = list.len();
        if n == 0 {
            return None;
        }
        let start = usize::try_from(self.0.round.fetch_add(1, Ordering::Relaxed)).unwrap_or(0);
        (0..n)
            .filter_map(|i| list.get(start.wrapping_add(i).checked_rem(n).unwrap_or(0)))
            .find(|c| c.usable())
            .cloned()
    }

    /// Whether peer `id` has a usable link (live, not draining).
    #[must_use]
    pub fn online(&self, id: &str) -> bool {
        self.pick(id).is_some()
    }

    /// Calls `method` on peer `id` with JSON `req`; `done` gets the reply's JSON or the error,
    /// once (zero `timeout`: [`DEFAULT_TIMEOUT`]).
    pub fn call(
        &self,
        id: &str,
        method: &str,
        req: &str,
        timeout: Duration,
        done: impl FnOnce(Result<Vec<u8>, Error>) + Send + 'static,
    ) {
        let Some(c) = self.pick(id) else {
            return done(Err(offline()));
        };
        let timeout = if timeout.is_zero() {
            DEFAULT_TIMEOUT
        } else {
            timeout
        };
        let req = if req.is_empty() { "{}" } else { req };
        c.wc.session().call(method, req.as_bytes(), timeout, done);
    }

    /// Opens the stream method `method` on peer `id` with JSON `req`.
    ///
    /// # Errors
    ///
    /// [`offline`]; the link's failure.
    pub fn open_stream(&self, id: &str, method: &str, req: &str) -> Result<Stream, Error> {
        let c = self.pick(id).ok_or_else(offline)?;
        c.wc.session().open_stream(method, req.as_bytes())
    }

    /// Pushes JSON `msg` of `method` to peer `id`.
    ///
    /// # Errors
    ///
    /// [`offline`]; the link's failure.
    pub fn push(&self, id: &str, method: &str, msg: &str) -> Result<(), Error> {
        let c = self.pick(id).ok_or_else(offline)?;
        c.wc.session().push(method, msg.as_bytes())
    }

    /// One entry per live link that said hello, by role, name and link.
    #[must_use]
    pub fn peers(&self) -> Vec<PeerConn> {
        let list: Vec<Arc<MConn>> = lock(&self.0.st).by_id.values().flatten().cloned().collect();
        let mut out: Vec<PeerConn> = list
            .iter()
            .map(|c| {
                let mut p = lock(&c.st).peer.clone();
                let s = c.wc.session();
                p.draining = s.draining();
                let (i, o) = s.traffic();
                p.link = LinkStats {
                    bytes_in: i,
                    bytes_out: o,
                    rtt: s.rtt(),
                };
                p
            })
            .collect();
        out.sort_by(|a, b| {
            (&a.identity.role, &a.identity.name, a.conn_id).cmp(&(
                &b.identity.role,
                &b.identity.name,
                b.conn_id,
            ))
        });
        out
    }

    /// The peers of `role` linked, each once.
    #[must_use]
    pub fn peers_of(&self, role: &str) -> Vec<Identity> {
        let mut seen = Vec::new();
        let mut out = Vec::new();
        for p in self.peers() {
            if p.identity.role == role && !seen.contains(&p.identity.id) {
                seen.push(p.identity.id.clone());
                out.push(p.identity);
            }
        }
        out
    }

    /// Closes every link of peer `id`, and says how many; outbound ones are dialed again.
    pub fn kick(&self, id: &str) -> usize {
        let list = lock(&self.0.st).by_id.get(id).cloned().unwrap_or_default();
        for c in &list {
            c.wc.close();
        }
        list.len()
    }

    /// The dial targets and their state, without passwords and tokens.
    #[must_use]
    pub fn targets(&self) -> Vec<DialTarget> {
        let list: Vec<Arc<Target>> = lock(&self.0.st).targets.values().cloned().collect();
        let mut out: Vec<DialTarget> = list.iter().map(|t| t.view()).collect();
        out.sort_by(|a, b| a.addr.cmp(&b.addr));
        out
    }
}

impl Inner {
    fn notify_quiet(&self) {
        if self.draining.load(Ordering::Acquire) {
            let (m, cv) = &self.quiet;
            let _g = lock(m);
            cv.notify_all();
        }
    }

    fn close_conns(&self, f: impl Fn(&MConn) -> bool) {
        let list: Vec<Arc<MConn>> = lock(&self.st)
            .conns
            .values()
            .filter(|c| f(c))
            .cloned()
            .collect();
        for c in list {
            c.wc.close();
            self.disconnected(&c.wc, None);
        }
    }

    /// Records a new authenticated link, before any frame of it is handled.
    fn connected(&self, wc: &Arc<wire::Conn>, t: Option<Arc<Target>>) -> Arc<MConn> {
        let c = Arc::new(MConn {
            wc: Arc::clone(wc),
            id: self
                .next_conn
                .fetch_add(1, Ordering::Relaxed)
                .saturating_add(1),
            out: t.is_some(),
            remote: t
                .as_ref()
                .map_or_else(|| wc.remote().to_owned(), |t| t.addr.clone()),
            target: t,
            st: Mutex::new(CState::default()),
        });
        if let Some(me) = self.me.upgrade() {
            let q = Arc::downgrade(&me);
            wc.session().set_quiet(Arc::new(move || {
                if let Some(m) = q.upgrade() {
                    m.notify_quiet();
                }
            }));
        }
        lock(&self.st).conns.insert(key(wc), Arc::clone(&c));
        c
    }

    fn conn_of(&self, wc: &Arc<wire::Conn>) -> Option<Arc<MConn>> {
        lock(&self.st).conns.get(&key(wc)).cloned()
    }

    /// Closes an incoming link that does not say hello in time.
    fn await_hello(&self, wc: &Arc<wire::Conn>) {
        let Some(c) = self.conn_of(wc) else { return };
        let wait = *lock(&self.hello_wait);
        let _ = after_send(wait, move || {
            let stale = {
                let g = lock(&c.st);
                !g.registered && !g.closed
            };
            if stale {
                akit_observe::warn!(
                    Llm,
                    "mesh: closing a connection that did not say hello: remote={}",
                    c.remote
                );
                c.wc.close();
            }
        });
    }

    /// Forgets a closed link; idempotent.
    fn disconnected(&self, wc: &Arc<wire::Conn>, err: Option<&Error>) {
        let (c, was, peer) = {
            let mut st = lock(&self.st);
            let Some(c) = st.conns.remove(&key(wc)) else {
                return;
            };
            let (was, peer, token_use) = {
                let mut g = lock(&c.st);
                let was = g.registered;
                g.registered = false;
                g.closed = true;
                (was, g.peer.clone(), g.token_use.take())
            };
            drop(token_use);
            if was {
                unindex(&mut st, &c, &peer.identity.id);
            }
            (c, was, peer)
        };
        self.notify_quiet();
        if was {
            let reason = match err {
                Some(e) if *e != Error::Closed => format!(" reason={e}"),
                _ => String::new(),
            };
            akit_observe::info!(
                Llm,
                "mesh peer disconnected: role={} id={} dir={} remote={}{reason}",
                peer.identity.role,
                peer.identity.id,
                c.dir(),
                peer.remote
            );
            let fns = self
                .on_down
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            for f in fns {
                f(&peer);
            }
        }
    }

    /// Validates the identity a peer announced: a role taken, not this role's, not this role.
    fn check_peer(&self, id: &Identity) -> Result<(), String> {
        if id.id.is_empty() || id.role.is_empty() {
            return Err("missing role or id".to_owned());
        }
        let me = (self.cfg.identity)();
        if id.id == me.id {
            return Err("connected to itself".to_owned());
        }
        if id.role == me.role {
            return Err(format!(
                "role {} is not accepted by {}: peers of the same role never connect",
                id.role, me.role
            ));
        }
        if id.role == ROLE_LOCAL || !self.cfg.accept_roles.contains(&id.role) {
            return Err(format!("role {} is not accepted by {}", id.role, me.role));
        }
        Ok(())
    }

    /// Records the peer of `c`, checked already.
    fn register(&self, c: &Arc<MConn>, id: Identity) -> Result<(), String> {
        let mut peer = PeerConn {
            identity: id.clone(),
            conn_id: c.id,
            direction: c.dir().to_owned(),
            remote: c.remote.clone(),
            connected_at: unix_now(),
            ..PeerConn::default()
        };
        match &c.target {
            Some(t) => peer.target.clone_from(&t.addr),
            None => {
                peer.token_id = c.wc.token().to_owned();
                peer.shared_password = c.wc.token().is_empty();
            }
        }
        {
            let mut st = lock(&self.st);
            // An ID is one process's: one another link holds under another role is someone
            // claiming it.
            let other = st.by_id.get(&id.id).and_then(|l| {
                l.iter()
                    .find(|o| !Arc::ptr_eq(o, c))
                    .map(|o| lock(&o.st).peer.identity.role.clone())
            });
            if let Some(r) = other.filter(|r| *r != id.role) {
                return Err(format!("id {} is connected already as a {r}", id.id));
            }
            let mut g = lock(&c.st);
            if g.closed {
                return Err(Error::Closed.to_string());
            }
            if g.registered {
                let old = g.peer.identity.id.clone();
                drop(g);
                unindex(&mut st, c, &old);
                g = lock(&c.st);
            }
            g.registered = true;
            g.peer = peer.clone();
            drop(g);
            st.by_id
                .entry(id.id.clone())
                .or_default()
                .push(Arc::clone(c));
        }
        if self.draining.load(Ordering::Acquire) {
            c.wc.session().drain();
        }
        akit_observe::info!(
            Llm,
            "mesh peer connected: role={} id={} name={} dir={} remote={}",
            id.role,
            id.id,
            id.name,
            c.dir(),
            peer.remote
        );
        let fns = self
            .on_up
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        for f in fns {
            f(&peer);
        }
        Ok(())
    }

    /// Serves the hello of an incoming link.
    fn handle_hello(&self, c: &Arc<MConn>, payload: &[u8]) -> Result<String, Error> {
        let id = Identity::parse(payload).map_err(|_| bad_request("bad hello"))?;
        let mut r = self.check_peer(&id);
        if r.is_ok() && !c.wc.roles().contains(&id.role) {
            // The password this link proved is not the role's.
            r = Err(format!(
                "role {}: the password this link proved is not the {} password of this listener",
                id.role, id.role
            ));
        }
        if r.is_ok() && !c.wc.token().is_empty() {
            r = self.admit_token(c, &id.id);
        }
        if r.is_ok() {
            r = self.register(c, id.clone());
        }
        match r {
            Ok(()) => Ok((self.cfg.identity)().to_json()),
            Err(e) => {
                akit_observe::warn!(
                    Llm,
                    "mesh: hello refused: role={} id={} remote={} error={e}",
                    id.role,
                    id.id,
                    c.remote
                );
                // Closed once the refusal has gone.
                let wc = Arc::clone(&c.wc);
                let _ = after_send(Duration::from_secs(1), move || wc.close());
                Err(Error::code(CODE_FORBIDDEN, e))
            }
        }
    }

    /// Records `c`, which proved its token, as the link of peer `id`: the token still good, and
    /// `id` bound to no other token.
    fn admit_token(&self, c: &Arc<MConn>, id: &str) -> Result<(), String> {
        let wc = Arc::downgrade(&c.wc);
        let u = self
            .tokens
            .admit(
                c.wc.token(),
                id,
                &c.remote,
                Some(Box::new(move || {
                    if let Some(w) = wc.upgrade() {
                        w.close();
                    }
                })),
            )
            .map_err(|e| e.to_string())?;
        let mut g = lock(&c.st);
        if g.closed {
            return Err(Error::Closed.to_string());
        }
        g.token_use = Some(u);
        Ok(())
    }

    /// The identity of the peer of `c`, once it said hello.
    fn caller(c: &MConn) -> Result<Identity, Error> {
        c.peer()
            .ok_or_else(|| Error::code(CODE_UNAUTHORIZED, "hello first"))
    }
}

fn unindex(st: &mut St, c: &Arc<MConn>, id: &str) {
    if let Some(list) = st.by_id.get_mut(id) {
        list.retain(|o| !Arc::ptr_eq(o, c));
        if list.is_empty() {
            st.by_id.remove(id);
        }
    }
}

/// Serves what a peer sends on one link.
struct Handler {
    m: Weak<Inner>,
    c: Weak<MConn>,
}

impl wire::Handler for Handler {
    fn request(&self, _s: &Session, req: Request, reply: Reply) {
        let (Some(m), Some(c)) = (self.m.upgrade(), self.c.upgrade()) else {
            return reply.answer(Err(Error::Closed));
        };
        if req.method == METHOD_HELLO {
            if c.out {
                return reply.answer(Err(Error::code(
                    CODE_FORBIDDEN,
                    "hello is for incoming connections",
                )));
            }
            let r = m.handle_hello(&c, &req.payload);
            return reply.answer(r.map(String::into_bytes));
        }
        match Inner::caller(&c) {
            Ok(caller) => m.router.invoke(&caller, &req.method, &req.payload, reply),
            Err(e) => reply.answer(Err(e)),
        }
    }

    fn stream(&self, _s: &Session, st: Stream) {
        let (Some(m), Some(c)) = (self.m.upgrade(), self.c.upgrade()) else {
            return st.end(Err(Error::Closed));
        };
        match Inner::caller(&c) {
            Ok(caller) => m.router.invoke_stream(&caller, st),
            Err(e) => st.end(Err(e)),
        }
    }

    fn push(&self, _s: &Session, msg: Request) {
        let (Some(m), Some(c)) = (self.m.upgrade(), self.c.upgrade()) else {
            return;
        };
        let Ok(caller) = Inner::caller(&c) else {
            akit_observe::warn!(
                Llm,
                "mesh: push before hello dropped: method={} remote={}",
                msg.method,
                c.remote
            );
            return;
        };
        let route = m
            .pushes
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&msg.method)
            .map(|r| (r.roles.clone(), Arc::clone(&r.f)));
        match route {
            None => akit_observe::warn!(
                Llm,
                "mesh: push of an unknown method dropped: method={} from={}",
                msg.method,
                caller.id
            ),
            Some((roles, _)) if !roles.contains(&caller.role) => akit_observe::warn!(
                Llm,
                "mesh: push refused: method={} role={} from={}",
                msg.method,
                caller.role,
                caller.id
            ),
            Some((_, f)) => f(&caller, &msg.payload),
        }
    }

    fn drain(&self, _s: &Session) {
        if let Some(c) = self.c.upgrade() {
            let p = lock(&c.st).peer.identity.clone();
            akit_observe::info!(
                Llm,
                "mesh peer draining: no new calls on this connection: role={} id={} dir={} remote={}",
                p.role,
                p.id,
                c.dir(),
                c.remote
            );
        }
    }
}

impl Inner {
    /// The handler of a new link: its mesh record made first.
    fn handler_of(
        self: &Arc<Self>,
        wc: &Arc<wire::Conn>,
        t: Option<Arc<Target>>,
    ) -> Arc<dyn wire::Handler> {
        let c = self.connected(wc, t);
        Arc::new(Handler {
            m: Arc::downgrade(self),
            c: Arc::downgrade(&c),
        })
    }

    fn peer_addr_filter(&self) -> Option<wire::conn::PeerFilter> {
        let p = lock(&self.filter).clone()?;
        Some(Arc::new(move |a: &SocketAddr| p.allows(a.ip())))
    }
}

/// The handler of a link whose mesh is gone.
pub(crate) struct NoMesh;
impl wire::Handler for NoMesh {}

pub(crate) fn dial_closed() -> NoMesh {
    NoMesh
}

#[cfg(test)]
mod tests;
