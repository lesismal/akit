#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! The manager of other akits (I132, fit's package `cluster`, its I141): one akit, with
//! `admin.cluster` set, keeps the URLs and an administrator's credentials of others, its
//! members, polls each one's state, and serves them to the console's Cluster pages as the
//! console module `cluster`, at `/api/m/cluster/` ([decision 041](../../../docs/decisions/041-cluster-management.md)).
//!
//! The members are kept in the store's section `cluster` ([`member::SECTION`]), fit's document,
//! with their passwords and tokens, encrypted with the rest of the store and never answered,
//! logged or written anywhere else; what akit does not use of the document yet (groups,
//! templates, rollouts, links, roles) is kept as it was read. A member is reached through its
//! console, logged in with a user name and a password, or its Admin API, with its token; its
//! certificate is verified by the system's CAs, a CA PEM of its own, or a pinned fingerprint.
//! Members may be akits or fits: what they answer is fit's protocol.
//!
//! Nothing here blocks an I/O thread (rule 1): the calls to members go out on akio's
//! asynchronous HTTP client, one a member, each within its timeout and its bound on answers;
//! the poller, a thread of its own, starts at most `max_concurrency` polls at once and waits
//! between them; what writes the store runs on a pool of the manager's own, one write at a time.
//!
//! ```text
//! GET    /api/nodes                    the members, their credentials as whether they are set,
//!                                      with the members' ETag
//! POST   /api/nodes                    a member added (If-Match the members' ETag)
//! GET    /api/nodes/<name>             a member and its state
//! PUT    /api/nodes/<name>             a member changed, or renamed; an empty password or token
//!                                      keeps the one kept
//! DELETE /api/nodes/<name>             a member removed
//! POST   /api/nodes/<name>/test        the member as kept, or as the body would change it,
//!                                      tested: reached, logged in, its GET /node read, its
//!                                      certificate shown
//! GET    /api/nodes/<name>/read/<what> the member's node, status, versions or check, read
//!                                      through the manager
//! POST   /api/test                     a member not added yet tested
//! GET    /api/state                    every member's state
//! GET    /api/audit                    the audit log, filtered by user, node and action
//! GET    /api/me                       the user's roles: before I139, a cluster-wide admin
//! ```

pub mod json;
pub mod member;
pub mod node;
pub mod transport;

mod api;
pub mod groups;
pub mod ops;
pub mod pass;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use akio::tls::RootCerts;
use akio::{Pool, PoolConfig};
use akit_config::cluster::Cluster;
use akit_config::value::Value;

use crate::member::{Document, Member, Read, SECTION};
use crate::node::{Log, Node};
use crate::transport::{Limits, Transport};

/// The console module the manager is served as.
pub const MODULE_ID: &str = "cluster";
/// How many tests of members run at once; one more is answered 429.
pub const MAX_TESTS: usize = 4;
/// How many reads of members through the manager run at once; one more is answered 429.
pub const MAX_READS: usize = 8;
/// How long [`Manager::close`] waits for the members' sessions to be logged out.
pub const CLOSE_WAIT: Duration = Duration::from_secs(2);
/// The writes of the document waiting for the pool; one more is answered 503.
pub const MAX_WRITES_WAITING: usize = 16;

/// Keeps sections beside the config, as the encrypted store does
/// (`akit_admin::store::Store::section`, `set_section`). Its writes are called on the manager's
/// own pool, never on an I/O thread.
pub trait Store: Send + Sync {
    /// The section `name`, a JSON value, or `None`.
    fn section(&self, name: &str) -> Option<Vec<u8>>;

    /// Keeps `data` as the section `name`.
    ///
    /// # Errors
    ///
    /// Why it could not be written.
    fn set_section(&self, name: &str, data: &[u8]) -> Result<(), String>;
}

/// What a [`Manager`] needs.
pub struct Options {
    /// The engine the calls to members go out on.
    pub engine: Arc<akio::Engine>,
    /// `admin.cluster`.
    pub settings: Cluster,
    /// The store the members are kept in.
    pub store: Arc<dyn Store>,
    /// Says what users changed and how members' states changed; never a credential.
    pub log: Log,
    /// The system's CAs, read once; `None` reads them now (on the caller's thread, which must
    /// not be an I/O thread).
    pub system_roots: Option<RootCerts>,
    /// The manager's own config schema, as its console serves it: a member whose `GET /node`
    /// gives another digest opens read-only through the node switcher (I134).
    pub schema: &'static [u8],
}

impl std::fmt::Debug for Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Options")
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

/// Why a change was refused, and the status it is answered with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A member refused for a field: 400, `{"field", "error"}`.
    Field(member::FieldError),
    /// What was named is not there: 404.
    NotFound(String),
    /// What the document holds refuses it, or the document cannot be written: 409.
    Conflict(String),
    /// Too much under way: 429.
    Busy(String),
    /// It failed: 500.
    Failed(String),
}

impl Refusal {
    /// The answer that refuses it.
    #[must_use]
    pub fn response(&self) -> akio_http::Response {
        use akit_roles::pages::{fail, json as answer};
        match self {
            Refusal::Field(f) => answer(400, f.json()),
            Refusal::NotFound(m) => fail(404, m),
            Refusal::Conflict(m) => fail(409, m),
            Refusal::Busy(m) => fail(429, m),
            Refusal::Failed(m) => fail(500, m),
        }
    }
}

/// The document and what goes with it, under the manager's lock.
struct Doc {
    doc: Document,
    /// Why the document cannot be written, or empty.
    read_only: String,
    /// Counts the changes made since the manager started, its ETag.
    rev: u64,
}

pub(crate) struct Inner {
    o: Options,
    interval: Duration,
    lim: Limits,
    max_nodes: usize,
    concurrency: usize,
    system: RootCerts,
    epoch: Instant,
    doc: Mutex<Doc>,
    nodes: Mutex<HashMap<String, Arc<Node>>>,
    pool: Arc<Pool>,
    /// The polls under way, and the poller waiting for one to end.
    sem: (Mutex<usize>, Condvar),
    wake: Mutex<Option<SyncSender<()>>>,
    poller: Mutex<Option<JoinHandle<()>>>,
    closed: AtomicBool,
    pub(crate) tests: AtomicUsize,
    pub(crate) reads: AtomicUsize,
    /// The node switcher's calls (I134), and the writes waiting for their audit entries.
    pub(crate) pass: Mutex<pass::Hub>,
    /// The operations on many members (I135).
    pub(crate) ops: Mutex<ops::Ops>,
    pub(crate) pass_held: AtomicUsize,
    /// The manager's schema and its digest, `sha256:` and hex.
    pub(crate) schema: &'static [u8],
    pub(crate) schema_digest: String,
}

/// The manager of an akit's members. Cheap to clone.
#[derive(Clone)]
pub struct Manager(pub(crate) Arc<Inner>);

impl std::fmt::Debug for Manager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Manager")
            .field("settings", &self.0.o.settings)
            .finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Now, as fit writes a member's times: RFC 3339 with nanoseconds, in UTC, unquoted.
fn now_text() -> String {
    json::time(SystemTime::now()).trim_matches('"').to_owned()
}

impl Manager {
    /// The manager `o.settings` turns on, with the members the store keeps, polling them. A
    /// section that is not the manager's document, one a newer manager wrote, or one with roles
    /// leaves the manager read-only, which the log says, rather than written over. Reads the
    /// store and, without `o.system_roots`, the system's CAs: the control thread's.
    ///
    /// # Errors
    ///
    /// The poller's thread could not start.
    pub fn new(o: Options) -> std::io::Result<Manager> {
        let s = &o.settings;
        let system = match &o.system_roots {
            Some(r) => r.clone(),
            None => RootCerts::system().unwrap_or_else(|_| RootCerts::empty()),
        };
        let (doc, read_only) = match Document::read(o.store.section(SECTION).as_deref()) {
            Read::Ok(d) => (d, String::new()),
            Read::ReadOnly(d, why) => (d, why),
            Read::Bad(why) => (
                Document::default(),
                format!("{why}; it is left as it is, and no member can be added until it is fixed"),
            ),
        };
        if !read_only.is_empty() {
            (o.log)(format!("cluster: {read_only}"));
        }
        let mut pc = PoolConfig::default();
        pc.min_threads = 0;
        pc.max_threads = 1;
        pc.queue_capacity = MAX_WRITES_WAITING;
        let inner = Arc::new(Inner {
            interval: s.interval(),
            lim: Limits {
                timeout: s.call_timeout(),
                max_bytes: usize::try_from(s.response_bytes()).unwrap_or(usize::MAX),
            },
            max_nodes: s.nodes(),
            concurrency: s.concurrency().max(1),
            system,
            epoch: Instant::now(),
            doc: Mutex::new(Doc {
                doc,
                read_only,
                rev: 0,
            }),
            nodes: Mutex::new(HashMap::new()),
            pool: Pool::new(pc),
            sem: (Mutex::new(0), Condvar::new()),
            wake: Mutex::new(None),
            poller: Mutex::new(None),
            closed: AtomicBool::new(false),
            tests: AtomicUsize::new(0),
            reads: AtomicUsize::new(0),
            pass: Mutex::new(pass::Hub::default()),
            ops: Mutex::new(ops::Ops::default()),
            pass_held: AtomicUsize::new(0),
            schema: o.schema,
            schema_digest: format!(
                "sha256:{}",
                akit_roles::linktoken::hex(&akio_crypto::hash::Sha256::digest(o.schema))
            ),
            o,
        });
        let m = Manager(inner);
        m.sync();
        let (tx, rx) = mpsc::sync_channel(1);
        *lock(&m.0.wake) = Some(tx);
        let me = Arc::clone(&m.0);
        let h = std::thread::Builder::new()
            .name("akit-cluster-poller".to_owned())
            .spawn(move || run(&me, &rx))?;
        *lock(&m.0.poller) = Some(h);
        Ok(m)
    }

    /// The console module the manager is served as.
    #[must_use]
    pub fn module(&self) -> akit_roles::pages::ModuleRef {
        Arc::new(api::Module(self.clone()))
    }

    /// Stops the poller, logs the members' consoles out, waiting for that [`CLOSE_WAIT`] at most,
    /// and closes their connections. Call it before the engine's shutdown.
    pub fn close(&self) {
        if self.0.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        lock(&self.0.wake).take();
        self.0.sem.1.notify_all();
        if let Some(h) = lock(&self.0.poller).take() {
            let _ = h.join();
        }
        let nodes: Vec<Arc<Node>> = lock(&self.0.nodes).drain().map(|(_, n)| n).collect();
        let (tx, rx) = mpsc::channel::<()>();
        for n in &nodes {
            let tx = tx.clone();
            n.shutdown(Box::new(move || {
                let _ = tx.send(());
            }));
        }
        drop(tx);
        let deadline = Instant::now().checked_add(CLOSE_WAIT);
        for _ in &nodes {
            let left = deadline.map_or(Duration::ZERO, |d| {
                d.saturating_duration_since(Instant::now())
            });
            if rx.recv_timeout(left).is_err() {
                break;
            }
        }
        for n in &nodes {
            if let Some(t) = n.transport() {
                t.close();
            }
        }
    }

    /// The members, as the store keeps them, by name.
    #[must_use]
    pub fn members(&self) -> Vec<Member> {
        let mut ms = lock(&self.0.doc).doc.members.clone();
        ms.sort_by(|a, b| a.name.cmp(&b.name));
        ms
    }

    /// A copy of the document.
    pub(crate) fn doc(&self) -> Document {
        lock(&self.0.doc).doc.clone()
    }

    /// The members' ETag: the changes made since the manager started.
    #[must_use]
    pub fn etag(&self) -> String {
        format!("\"{}\"", lock(&self.0.doc).rev)
    }

    /// Why the document cannot be written, or empty.
    #[must_use]
    pub fn read_only(&self) -> String {
        lock(&self.0.doc).read_only.clone()
    }

    /// Every member's state as the API answers it, by name, as a JSON list.
    #[must_use]
    pub fn states_json(&self) -> String {
        let members = self.members();
        let nodes = lock(&self.0.nodes).clone();
        let mut b = String::from("[");
        for (i, m) in members.iter().enumerate() {
            if i > 0 {
                b.push(',');
            }
            b.push_str(&self.state_json(m, nodes.get(&m.name)));
        }
        b.push(']');
        b
    }

    fn state_json(&self, m: &Member, n: Option<&Arc<Node>>) -> String {
        let groups = groups::groups_of(&lock(&self.0.doc).doc, &m.name, &m.labels);
        if let Some(n) = n {
            return n.state().json(m, &groups);
        }
        // A member kept in the store that does not check is never polled.
        let mut st = n_state_unchecked();
        st.error = Some(transport::Problem::new(
            transport::KIND_BAD_ANSWER,
            "the member kept in the store is not valid: edit it",
        ));
        st.json(m, &groups)
    }

    /// The node of member `name`.
    pub(crate) fn node(&self, name: &str) -> Option<Arc<Node>> {
        lock(&self.0.nodes).get(name).cloned()
    }

    /// A node for `m`, a member checked, not kept: a test's.
    pub(crate) fn new_node(&self, m: &Member) -> Result<Arc<Node>, String> {
        let t = if m.access == member::ACCESS_LINK {
            None
        } else {
            Some(Arc::new(Transport::new(
                &self.0.o.engine,
                m,
                self.0.lim,
                &self.0.system,
            )?))
        };
        Ok(Arc::new(Node::new(
            m.clone(),
            t,
            self.0.interval,
            Arc::clone(&self.0.o.log),
        )))
    }

    /// Makes the nodes those of the document's members: a member whose link is unchanged keeps
    /// its node, its session and its state; one changed or removed has its node shut down.
    fn sync(&self) {
        if self.0.closed.load(Ordering::Acquire) {
            return;
        }
        let members = lock(&self.0.doc).doc.members.clone();
        let mut nodes = lock(&self.0.nodes);
        let mut keep = HashMap::new();
        for m in &members {
            if let Some(n) = nodes.remove(&m.name).filter(|n| n.spec.same_link(m)) {
                keep.insert(m.name.clone(), n);
                continue;
            }
            if let Err(e) = m.check() {
                (self.0.o.log)(format!(
                    "cluster: the member {:?} kept in the store is not polled: {e}",
                    m.name
                ));
                continue;
            }
            match self.new_node(m) {
                Ok(n) => {
                    keep.insert(m.name.clone(), n);
                }
                Err(e) => (self.0.o.log)(format!(
                    "cluster: the member {:?} kept in the store is not polled: {e}",
                    m.name
                )),
            }
        }
        for (_, n) in nodes.drain() {
            n.shutdown(Box::new(|| {}));
        }
        *nodes = keep;
        drop(nodes);
        self.poke();
    }

    /// Has the poller look for members due now.
    pub(crate) fn poke(&self) {
        if let Some(tx) = lock(&self.0.wake).as_ref() {
            let _ = tx.try_send(());
        }
    }

    /// Runs `f` on a copy of the document and writes it to the store with the audit entry `f`
    /// returns, as `user`, then uses it: fit's `Manager.change`. It writes a file: on the
    /// manager's pool.
    pub(crate) fn change<T>(
        &self,
        user: &str,
        f: impl FnOnce(&mut Document) -> Result<(Vec<(String, Value)>, T), Refusal>,
    ) -> Result<T, Refusal> {
        let mut d = {
            let g = lock(&self.0.doc);
            if !g.read_only.is_empty() {
                return Err(Refusal::Conflict(g.read_only.clone()));
            }
            g.doc.clone()
        };
        let (entry, out) = f(&mut d)?;
        let before = d.revision;
        d.revision = d.revision.saturating_add(1);
        let mut e = vec![
            ("at".to_owned(), Value::String(now_text())),
            ("user".to_owned(), Value::String(user.to_owned())),
        ];
        e.extend(entry);
        if !e.iter().any(|(k, _)| k == "outcome") {
            e.push(("outcome".to_owned(), Value::String("ok".to_owned())));
        }
        e.push(("before".to_owned(), Value::String(format!("r{before}"))));
        e.push((
            "after".to_owned(),
            Value::String(format!("r{}", d.revision)),
        ));
        d.append_audit(Value::Object(e));
        self.0
            .o
            .store
            .set_section(SECTION, &d.marshal())
            .map_err(|e| Refusal::Failed(format!("saving the members: {e}")))?;
        {
            let mut g = lock(&self.0.doc);
            g.doc = d;
            g.rev = g.rev.saturating_add(1);
        }
        self.sync();
        Ok(out)
    }

    /// Appends `entry` to the audit log without changing the members' ETag, on the pool: a
    /// test's (fit's `record`). A failed write is logged.
    pub(crate) fn record(&self, user: &str, entry: Vec<(String, Value)>) {
        self.record_then(user, entry, Box::new(|| {}));
    }

    /// [`record`](Manager::record), calling `done` once the entry is written, or lost.
    pub(crate) fn record_then(
        &self,
        user: &str,
        entry: Vec<(String, Value)>,
        done: Box<dyn FnOnce() + Send>,
    ) {
        let me = self.clone();
        let user = user.to_owned();
        let done = Arc::new(Mutex::new(Some(done)));
        let done2 = Arc::clone(&done);
        let job = Box::new(move || {
            let finish = || {
                if let Some(f) = lock(&done2).take() {
                    f();
                }
            };
            let mut d = {
                let g = lock(&me.0.doc);
                if !g.read_only.is_empty() {
                    drop(g);
                    return finish();
                }
                g.doc.clone()
            };
            let mut e = vec![
                ("at".to_owned(), Value::String(now_text())),
                ("user".to_owned(), Value::String(user)),
            ];
            e.extend(entry);
            d.append_audit(Value::Object(e));
            match me.0.o.store.set_section(SECTION, &d.marshal()) {
                Ok(()) => lock(&me.0.doc).doc.audit = d.audit,
                Err(e) => (me.0.o.log)(format!("cluster: the audit log: {e}")),
            }
            finish();
        });
        if self.0.pool.try_execute(job).is_err() {
            (self.0.o.log)(
                "cluster: the audit log: too many writes waiting; an entry is lost".to_owned(),
            );
            if let Some(f) = lock(&done).take() {
                f();
            }
        }
    }

    /// Runs `job` on the manager's pool: `false` when too many wait.
    pub(crate) fn on_pool(&self, job: Box<dyn FnOnce() + Send>) -> bool {
        self.0.pool.try_execute(job).is_ok()
    }

    /// The settings' bounds and timings.
    pub(crate) fn interval(&self) -> Duration {
        self.0.interval
    }

    pub(crate) fn limits(&self) -> Limits {
        self.0.lim
    }

    pub(crate) fn max_nodes(&self) -> usize {
        self.0.max_nodes
    }

    pub(crate) fn log(&self, msg: String) {
        (self.0.o.log)(msg);
    }

    /// The audit log, oldest first.
    #[must_use]
    pub fn audit(&self) -> Vec<Value> {
        lock(&self.0.doc).doc.audit.clone()
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.0.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Starts a poll of each member due, and returns how long until the next one is; `None`
    /// once the manager stops.
    fn poll_due(&self) -> Option<Duration> {
        let nodes: Vec<Arc<Node>> = lock(&self.0.nodes).values().cloned().collect();
        let interval_ms = json::millis(self.0.interval);
        let mut wait = self.0.interval;
        for n in nodes {
            let now = self.now_ms();
            let next = n.next.load(Ordering::Acquire);
            if next > now {
                wait = wait.min(Duration::from_millis(next.saturating_sub(now)));
                continue;
            }
            if n.closed.load(Ordering::Acquire) || n.busy.swap(true, Ordering::AcqRel) {
                continue;
            }
            // A slot among max_concurrency, waited for: this is the poller's own thread.
            {
                let (m, cv) = &self.0.sem;
                let mut held = lock(m);
                while *held >= self.0.concurrency {
                    if self.0.closed.load(Ordering::Acquire) {
                        n.busy.store(false, Ordering::Release);
                        return None;
                    }
                    held = cv.wait(held).unwrap_or_else(PoisonError::into_inner);
                }
                *held = held.saturating_add(1);
            }
            if self.0.closed.load(Ordering::Acquire) {
                self.release();
                n.busy.store(false, Ordering::Release);
                return None;
            }
            n.next
                .store(self.now_ms().saturating_add(interval_ms), Ordering::Release);
            let me = self.clone();
            let node = Arc::clone(&n);
            n.poll(Box::new(move || {
                node.busy.store(false, Ordering::Release);
                me.release();
                if me.now_ms() >= node.next.load(Ordering::Acquire) {
                    me.poke();
                }
            }));
        }
        Some(wait)
    }

    fn release(&self) {
        let (m, cv) = &self.0.sem;
        let mut held = lock(m);
        *held = held.saturating_sub(1);
        cv.notify_all();
    }
}

fn n_state_unchecked() -> node::State {
    node::State {
        status: node::STATUS_ERROR,
        ..node::State::new()
    }
}

/// The poller: polls the members, each every interval, at most `max_concurrency` at once, until
/// the manager stops. Waiting between polls is the logic of an interval (rule: timers only where
/// the logic is about time).
fn run(inner: &Arc<Inner>, wake: &mpsc::Receiver<()>) {
    let m = Manager(Arc::clone(inner));
    loop {
        if inner.closed.load(Ordering::Acquire) {
            return;
        }
        let Some(wait) = m.poll_due() else { return };
        match wake.recv_timeout(wait) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}
