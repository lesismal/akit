//! The Kubernetes provider, fit's `provider/kubernetes`: the Gateway API's Gateways and routes
//! (I60), whose status it writes back, and Ingresses (I59), served as listeners, routes,
//! services and upstreams.
//!
//! It talks to the API server with a client of its own on akio's (fit's decision 011): it lists
//! each kind it reads, in pages, at the first of its versions the server serves, then watches it
//! from the list's resource version, over HTTP/2 when the server offers it. The listeners of the
//! Gateways of the GatewayClasses whose controllerName is `controller_name` are listeners of
//! akit's, and their HTTPRoutes, GRPCRoutes, TLSRoutes, TCPRoutes and UDPRoutes routes and
//! services of akit's ([`gateway`]); the status of each class, Gateway, listener and route is
//! written back as the API says ([`status`]), on the provider's thread, one write at a time.
//! With `ingress`, each Ingress of an IngressClass whose controller is `controller_name` (or
//! naming none, when such a class is the default) is a set of routes on akit's listener of
//! `ingress.http_port` and, for the hosts of its `tls`, of `ingress.https_port`. The certificates
//! of the Secrets HTTPS listeners and Ingresses name are written to files; each Service port they
//! send to is an upstream of its ready endpoints or its cluster IP. Ingresses' annotations under
//! `fit.lesismal.github.io/` ask for redirects to HTTPS, rewrites, h2c upstreams, rate and body
//! limits, source ranges and timeouts, as fit reads them. What it leaves out is reported once.
//!
//! It runs in the control plane, on a thread of its own, its requests answered on an akio
//! engine of its own, never on the listeners' I/O threads.

mod client;
mod gateway;
mod kubeconfig;
mod status;
mod translate;
mod types;
mod watch;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use akio::Engine;
use akio_http::client::CancelToken;
use akio_http::h2client::H2Client;
use akit_config::value::{self, Value};
use akit_config::{KubernetesProvider, ProviderKind};

use crate::run::{Make, Provider, Update};
use client::{Api, Failure, PAGE_SIZE, Ready};
pub use kubeconfig::Env;
use status::{StatusUpdate, object_path};
use translate::{Snapshot, Translator};
use types::{Object, Shape, decode};
use watch::{Signal, Store, Watch, lock};

/// The kinds of provider this module makes, for [`crate::Kinds`].
pub const KINDS: &[(&str, Make<akit_config::Provider>)] = &[("kubernetes", make)];

fn make(name: &str, c: &akit_config::Provider) -> Result<(Box<dyn Provider>, String), String> {
    #[allow(unreachable_patterns)]
    match &c.kind {
        ProviderKind::Kubernetes(k) => Ok((
            Box::new(Kubernetes::new(name, k.clone())),
            format!(
                "watching Kubernetes for GatewayClasses of {}",
                k.controller_name
            ),
        )),
        other => Err(format!(
            "provider {name}: not a kubernetes provider: {}",
            other.name()
        )),
    }
}

/// How the provider says what happens: a line, its name in front.
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// The provider's timings; fields so that tests can shorten them.
#[derive(Debug, Clone, Copy)]
pub struct Timings {
    /// How long the provider waits after a change for more, before translating what it holds.
    pub debounce: Duration,
    /// How often the part is sent whether or not it changed, so that one refused because of
    /// the rest of the config is tried again.
    pub resync: Duration,
    /// The first wait before listing a kind again after a failure, doubled up to
    /// `max_backoff`; a kind not served is looked for again every `max_backoff`.
    pub min_backoff: Duration,
    /// See `min_backoff`.
    pub max_backoff: Duration,
}

impl Default for Timings {
    fn default() -> Timings {
        Timings {
            debounce: Duration::from_millis(200),
            resync: Duration::from_secs(30),
            min_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(30),
        }
    }
}

/// A kind the provider watches.
#[derive(Debug, Clone, Copy)]
struct Kind {
    /// The resource's plural, for messages, and the kind of the statuses written.
    name: &'static str,
    /// Its path on the API server, across namespaces.
    path: &'static str,
    /// The paths of the kind's other versions, tried in turn when the API server does not serve
    /// `path`.
    older: &'static [&'static str],
    /// What narrows what is listed and watched, a query's.
    query: &'static str,
    shape: Shape,
}

const fn kind(name: &'static str, path: &'static str, shape: Shape) -> Kind {
    Kind {
        name,
        path,
        older: &[],
        query: "",
        shape,
    }
}

/// The kinds the provider watches. TLSRoutes, TCPRoutes and UDPRoutes are in the Gateway API's
/// experimental channel. Only the Secrets of TLS certificates, which HTTPS listeners and
/// Ingresses name.
const GATEWAY_KINDS: [Kind; 12] = [
    kind("namespaces", "/api/v1/namespaces", Shape::Namespace),
    kind(
        "gatewayclasses",
        "/apis/gateway.networking.k8s.io/v1/gatewayclasses",
        Shape::GatewayClass,
    ),
    kind(
        "gateways",
        "/apis/gateway.networking.k8s.io/v1/gateways",
        Shape::Gateway,
    ),
    kind(
        "httproutes",
        "/apis/gateway.networking.k8s.io/v1/httproutes",
        Shape::HttpRoute,
    ),
    kind(
        "grpcroutes",
        "/apis/gateway.networking.k8s.io/v1/grpcroutes",
        Shape::GrpcRoute,
    ),
    Kind {
        older: &[
            "/apis/gateway.networking.k8s.io/v1alpha3/tlsroutes",
            "/apis/gateway.networking.k8s.io/v1alpha2/tlsroutes",
        ],
        ..kind(
            "tlsroutes",
            "/apis/gateway.networking.k8s.io/v1/tlsroutes",
            Shape::TlsRoute,
        )
    },
    kind(
        "tcproutes",
        "/apis/gateway.networking.k8s.io/v1alpha2/tcproutes",
        Shape::TcpRoute,
    ),
    kind(
        "udproutes",
        "/apis/gateway.networking.k8s.io/v1alpha2/udproutes",
        Shape::UdpRoute,
    ),
    Kind {
        older: &["/apis/gateway.networking.k8s.io/v1/referencegrants"],
        ..kind(
            "referencegrants",
            "/apis/gateway.networking.k8s.io/v1beta1/referencegrants",
            Shape::Grant,
        )
    },
    kind("services", "/api/v1/services", Shape::Service),
    kind(
        "endpointslices",
        "/apis/discovery.k8s.io/v1/endpointslices",
        Shape::Slice,
    ),
    Kind {
        query: "fieldSelector=type%3Dkubernetes.io%2Ftls",
        ..kind("secrets", "/api/v1/secrets", Shape::Secret)
    },
];

/// The kinds watched for Ingresses too, with `ingress`.
const INGRESS_KINDS: [Kind; 2] = [
    kind(
        "ingresses",
        "/apis/networking.k8s.io/v1/ingresses",
        Shape::Ingress,
    ),
    kind(
        "ingressclasses",
        "/apis/networking.k8s.io/v1/ingressclasses",
        Shape::IngressClass,
    ),
];

/// The kinds a provider of `opts` watches: Ingresses and IngressClasses only with `ingress`.
fn watched(opts: &KubernetesProvider) -> Vec<Kind> {
    let mut v = GATEWAY_KINDS.to_vec();
    if opts.ingress.is_some() {
        v.extend(INGRESS_KINDS);
    }
    v
}

/// What wakes the provider's thread.
pub(crate) enum Msg {
    /// An object changed.
    Changed,
    /// Stop.
    Stop,
    /// The HTTP/2 connection was made, or not.
    Dialed(Result<H2Client, io::Error>),
    /// A page of a kind's list came, or did not.
    Page {
        kind: usize,
        seq: u64,
        r: Result<Vec<u8>, Failure>,
    },
    /// A kind's watch ended.
    WatchEnded {
        kind: usize,
        seq: u64,
        r: Result<(), Failure>,
    },
    /// A status was written, or not.
    StatusWritten {
        kind: &'static str,
        r: Result<(), Failure>,
    },
}

impl fmt::Debug for Msg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Msg::Changed => "Changed",
            Msg::Stop => "Stop",
            Msg::Dialed(_) => "Dialed",
            Msg::Page { .. } => "Page",
            Msg::WatchEnded { .. } => "WatchEnded",
            Msg::StatusWritten { .. } => "StatusWritten",
        })
    }
}

/// A Kubernetes provider.
pub struct Kubernetes {
    name: String,
    opts: KubernetesProvider,
    env: Env,
    timings: Timings,
    log: Log,
    running: Option<(SyncSender<Msg>, Arc<AtomicBool>, JoinHandle<()>)>,
}

impl fmt::Debug for Kubernetes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Kubernetes")
            .field("name", &self.name)
            .field("opts", &self.opts)
            .finish_non_exhaustive()
    }
}

impl Kubernetes {
    /// A provider named `name`, with the settings `opts`, the process's environment, fit's
    /// timings, and saying what happens on stderr.
    #[must_use]
    pub fn new(name: &str, opts: KubernetesProvider) -> Kubernetes {
        Kubernetes {
            name: name.to_owned(),
            opts,
            env: Env::of_process(),
            timings: Timings::default(),
            log: Arc::new(|line: &str| eprintln!("akit: {line}")),
            running: None,
        }
    }

    /// Finds the cluster in `env` rather than the process's environment.
    #[must_use]
    pub fn with_env(mut self, env: Env) -> Kubernetes {
        self.env = env;
        self
    }

    /// Takes `timings` rather than fit's.
    #[must_use]
    pub fn with_timings(mut self, timings: Timings) -> Kubernetes {
        self.timings = timings;
        self
    }

    /// Says what happens through `log`.
    #[must_use]
    pub fn with_log(mut self, log: Log) -> Kubernetes {
        self.log = log;
        self
    }
}

impl Provider for Kubernetes {
    fn name(&self) -> &str {
        &self.name
    }

    fn start(&mut self, update: Update) -> io::Result<()> {
        if self.running.is_some() {
            return Ok(());
        }
        // One message for each kind's request in flight, the dial's, the status write's, a
        // change and the stop.
        let n = watched(&self.opts).len();
        let (tx, rx) = mpsc::sync_channel(n.saturating_add(5));
        let stopped = Arc::new(AtomicBool::new(false));
        let run = Run {
            name: self.name.clone(),
            prefix: format!("provider {}: ", self.name),
            opts: self.opts.clone(),
            env: self.env.clone(),
            t: self.timings,
            log: Arc::clone(&self.log),
            update,
            signal: Arc::new(Signal {
                tx: tx.clone(),
                changed: AtomicBool::new(false),
            }),
            stopped: Arc::clone(&stopped),
            last: None,
            problems: BTreeSet::new(),
            cert_dir: None,
            own_dir: false,
            writes: VecDeque::new(),
            writing: None,
            failing: BTreeSet::new(),
        };
        let h = std::thread::Builder::new()
            .name(format!("akit-provider-{}", self.name))
            .spawn(move || run.run(&rx))?;
        self.running = Some((tx, stopped, h));
        Ok(())
    }

    fn stop(&mut self) {
        if let Some((tx, stopped, h)) = self.running.take() {
            stopped.store(true, Ordering::Release);
            // The queue has room for it: a stop is sent once.
            let _ = tx.try_send(Msg::Stop);
            let _ = h.join();
        }
    }
}

impl Drop for Kubernetes {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A kind's state on the provider's thread.
struct KindState {
    kind: Kind,
    /// Which of its paths is listed and watched: 0 for `kind.path`, else one of `kind.older`.
    at: usize,
    store: Arc<Mutex<Store>>,
    /// Counts the requests made, so that the end of one given up is passed over.
    seq: u64,
    cancel: Option<CancelToken>,
    phase: Phase,
    backoff: Duration,
    /// Reported: the kind is not served; objects left out past `max_objects`.
    missing: bool,
    full: bool,
}

enum Phase {
    /// To list, or watch from a resource version, at a time.
    Wait {
        at: Instant,
        next: Next,
    },
    /// Pages being read: the objects so far, and whether more were left out.
    Listing {
        objs: BTreeMap<String, Arc<Object>>,
        truncated: bool,
    },
    Watching,
}

enum Next {
    List,
    Watch(String),
}

/// The provider's thread.
struct Run {
    name: String,
    prefix: String,
    opts: KubernetesProvider,
    env: Env,
    t: Timings,
    log: Log,
    update: Update,
    signal: Arc<Signal>,
    stopped: Arc<AtomicBool>,
    /// The last part sent.
    last: Option<Vec<u8>>,
    /// What was reported, so that it is reported once, until it goes and comes again.
    problems: BTreeSet<String>,
    /// Where certificate files are written; `None` when there is nowhere.
    cert_dir: Option<PathBuf>,
    /// The directory is the provider's own, removed when it stops.
    own_dir: bool,
    /// The statuses still to write, of the last set made, and the write in flight; one at a
    /// time, so that a slow API server holds up no part.
    writes: VecDeque<StatusUpdate>,
    writing: Option<CancelToken>,
    /// The kinds whose last write failed, and was reported.
    failing: BTreeSet<&'static str>,
}

impl KindState {
    /// The path listed and watched.
    fn path(&self) -> &'static str {
        match self.at.checked_sub(1) {
            None => self.kind.path,
            Some(i) => self.kind.older.get(i).copied().unwrap_or(self.kind.path),
        }
    }
}

impl Run {
    fn log(&self, msg: &str) {
        (self.log)(&format!("{}{msg}", self.prefix));
    }

    /// Waits for `d` or a stop; whether it was a stop.
    fn sleep(&self, rx: &Receiver<Msg>, d: Duration) -> bool {
        let until = Instant::now().checked_add(d);
        loop {
            if self.stopped.load(Ordering::Acquire) {
                return true;
            }
            let left = until.map_or(d, |u| u.saturating_duration_since(Instant::now()));
            if left.is_zero() {
                return false;
            }
            match rx.recv_timeout(left) {
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => return true,
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => return false,
            }
        }
    }

    fn run(mut self, rx: &Receiver<Msg>) {
        // Until the cluster can be found, nothing new is sent, so the last part stays.
        let (api, engine) = loop {
            let r = kubeconfig::load(&self.opts, &self.env).and_then(|rc| {
                for w in rc.exposed() {
                    self.log(&w);
                }
                let server = rc.server.clone();
                let r#where = rc.r#where.clone();
                let mut cfg = akio::Config::default();
                cfg.shutdown_timeout = Duration::from_secs(1);
                cfg.connect_timeout = Some(self.opts.request_timeout);
                let engine = Engine::builder()
                    .client_threads(1)
                    .config(cfg)
                    .build()
                    .map_err(|e| format!("the client's engine: {e}"))?;
                engine
                    .start()
                    .map_err(|e| format!("the client's engine: {e}"))?;
                let engine = Arc::new(engine);
                match Api::new(Arc::clone(&engine), rc, self.opts.request_timeout) {
                    Ok(api) => Ok((api, engine, server, r#where)),
                    Err(e) => {
                        engine.shutdown();
                        engine.wait();
                        Err(e)
                    }
                }
            });
            match r {
                Ok((api, engine, server, r#where)) => {
                    self.log(&format!("watching the cluster at {server}, by {where}"));
                    break (api, engine);
                }
                Err(e) => {
                    self.log(&format!(
                        "{e}; trying again in {}",
                        akit_config::duration::format(self.t.max_backoff)
                    ));
                    if self.sleep(rx, self.t.max_backoff) {
                        return;
                    }
                }
            }
        };
        self.cert_dir();
        self.follow(rx, api);
        engine.shutdown();
        engine.wait();
        if self.own_dir {
            if let Some(d) = &self.cert_dir {
                let _ = std::fs::remove_dir_all(d);
            }
        }
    }

    /// Makes the directory certificates are written to.
    fn cert_dir(&mut self) {
        if self.opts.certificates_dir.is_empty() {
            let nanos = SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos());
            let d = std::env::temp_dir().join(format!(
                "akit-{}-{}-{nanos:08x}",
                self.name,
                std::process::id()
            ));
            match make_dir(&d, false) {
                Ok(()) => {
                    self.cert_dir = Some(d);
                    self.own_dir = true;
                }
                Err(e) => self.log(&format!(
                    "no directory for the certificates of HTTPS listeners: {e}; they are not served"
                )),
            }
        } else {
            let d = PathBuf::from(&self.opts.certificates_dir);
            match make_dir(&d, true) {
                Ok(()) => self.cert_dir = Some(d),
                Err(e) => self.log(&format!(
                    "certificates_dir: {e}; HTTPS listeners are not served"
                )),
            }
        }
    }

    /// Follows the cluster until stopped: each kind listed, then watched, listed again when
    /// its watch fails or is too old; the part sent once every kind is listed, after each change
    /// (once no more come for `debounce`), and every `resync`.
    fn follow(&mut self, rx: &Receiver<Msg>, mut api: Api) {
        let now = Instant::now();
        let mut kinds: Vec<KindState> = watched(&self.opts)
            .into_iter()
            .map(|k| KindState {
                kind: k,
                at: 0,
                store: Arc::default(),
                seq: 0,
                cancel: None,
                phase: Phase::Wait {
                    at: now,
                    next: Next::List,
                },
                backoff: self.t.min_backoff,
                missing: false,
                full: false,
            })
            .collect();
        let mut fire: Option<Instant> = None;
        let mut resync = now.checked_add(self.t.resync);
        // A failed dial is made again after its backoff.
        let mut dial_after: Option<Instant> = None;
        let mut dial_backoff = self.t.min_backoff;
        loop {
            let now = Instant::now();
            // Start what is due, once the client is ready; while it is being connected, the
            // outcome wakes this thread.
            let mut dialing = false;
            let due = api_state(&kinds);
            if dial_after.is_none_or(|t| now >= t) && due.is_some_and(|at| at <= now) {
                let signal = Arc::clone(&self.signal);
                match api.poll(move |r| signal.post(Msg::Dialed(r))) {
                    Ready::Yes => {
                        dial_after = None;
                        for i in 0..kinds.len() {
                            self.start(&mut api, &mut kinds, i, now);
                        }
                    }
                    Ready::Dialing => dialing = true,
                }
            }
            let due = if dialing {
                None
            } else {
                api_state(&kinds).map(|at| dial_after.map_or(at, |d| d.max(at)))
            };
            let deadline = [fire, resync, due].into_iter().flatten().min();
            let wait = deadline.map_or(Duration::from_secs(3600), |d| {
                d.saturating_duration_since(Instant::now())
            });
            let msg = match rx.recv_timeout(wait) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => Some(Msg::Stop),
            };
            if self.stopped.load(Ordering::Acquire) {
                break;
            }
            let now = Instant::now();
            match msg {
                Some(Msg::Stop) => break,
                Some(Msg::Changed) => {
                    self.signal.changed.store(false, Ordering::Release);
                    if fire.is_none() {
                        fire = now.checked_add(self.t.debounce);
                    }
                }
                Some(Msg::Dialed(r)) => match api.dialed(r) {
                    Ok(()) => dial_backoff = self.t.min_backoff,
                    Err(e) => {
                        self.log(&format!(
                            "{e}; trying again in {}",
                            akit_config::duration::format(dial_backoff)
                        ));
                        dial_after = now.checked_add(dial_backoff);
                        dial_backoff = dial_backoff.saturating_mul(2).min(self.t.max_backoff);
                    }
                },
                Some(Msg::Page { kind, seq, r }) => {
                    if kinds.get(kind).is_some_and(|k| k.seq == seq)
                        && self.page(&mut api, &mut kinds, kind, r, now)
                    {
                        fire.get_or_insert(now);
                    }
                }
                Some(Msg::WatchEnded { kind, seq, r }) => {
                    if let Some(k) = kinds.get_mut(kind).filter(|k| k.seq == seq) {
                        self.watch_ended(k, r, now);
                    }
                }
                Some(Msg::StatusWritten { kind, r }) => {
                    self.writing = None;
                    self.written(kind, r);
                    self.write_next(&mut api, &kinds);
                }
                None => {}
            }
            if fire.is_some_and(|f| now >= f) {
                fire = None;
                self.send(&mut api, &mut kinds, false);
            }
            if resync.is_some_and(|r| now >= r) {
                resync = now.checked_add(self.t.resync);
                self.send(&mut api, &mut kinds, true);
            }
        }
        for k in &mut kinds {
            if let Some(c) = k.cancel.take() {
                c.cancel();
            }
        }
        if let Some(c) = self.writing.take() {
            c.cancel();
        }
    }

    /// Starts writing the next status, when none is being written.
    fn write_next(&mut self, api: &mut Api, kinds: &[KindState]) {
        if self.writing.is_some() {
            return;
        }
        let Some(u) = self.writes.pop_front() else {
            return;
        };
        let Some(k) = kinds.iter().find(|k| k.kind.name == u.kind) else {
            return;
        };
        let path = object_path(k.path(), &u.ns, &u.name);
        let cancel = CancelToken::new();
        self.writing = Some(cancel.clone());
        let (signal, kind) = (Arc::clone(&self.signal), u.kind);
        api.patch_status(&path, u.patch, &cancel, move |r| {
            signal.post(Msg::StatusWritten { kind, r });
        });
    }

    /// A status write ended: one refused because the object changed since, or is gone, is
    /// left, since the change comes through its watch and makes another set; a failure is
    /// reported once for its kind, and its recovery.
    fn written(&mut self, kind: &'static str, r: Result<(), Failure>) {
        match r {
            Ok(()) | Err(Failure::Conflict | Failure::NotFound) => {
                if self.failing.remove(kind) {
                    self.log(&format!("the status of {kind} is written again"));
                }
            }
            Err(e) => {
                if self.failing.insert(kind) {
                    self.log(&format!(
                        "writing the status of {kind}: {e}; trying again with each change and every {}",
                        akit_config::duration::format(self.t.resync)
                    ));
                }
            }
        }
    }

    /// Starts kind `i`'s list or watch, when it is due.
    fn start(&self, api: &mut Api, kinds: &mut [KindState], i: usize, now: Instant) {
        let Some(k) = kinds.get_mut(i) else {
            return;
        };
        let next = match &mut k.phase {
            Phase::Wait { at, next } if *at <= now => std::mem::replace(next, Next::List),
            _ => return,
        };
        k.seq = k.seq.wrapping_add(1);
        let cancel = CancelToken::new();
        k.cancel = Some(cancel.clone());
        match next {
            Next::List => {
                k.at = 0;
                k.phase = Phase::Listing {
                    objs: BTreeMap::new(),
                    truncated: false,
                };
                self.get_page(api, k, i, "");
            }
            Next::Watch(rv) => {
                k.phase = Phase::Watching;
                let w = Watch::new(
                    i,
                    k.seq,
                    k.kind.name,
                    k.path().to_owned(),
                    k.kind.shape,
                    Arc::clone(&k.store),
                    self.opts.max_objects,
                    Arc::clone(&self.signal),
                );
                api.watch(k.path(), k.kind.query, &rv, &cancel, w);
            }
        }
    }

    fn get_page(&self, api: &mut Api, k: &KindState, i: usize, cont: &str) {
        let mut q = format!("limit={PAGE_SIZE}");
        if !k.kind.query.is_empty() {
            q.push('&');
            q.push_str(k.kind.query);
        }
        if !cont.is_empty() {
            q.push_str("&continue=");
            q.push_str(&client::escape(cont));
        }
        let (signal, seq) = (Arc::clone(&self.signal), k.seq);
        let cancel = k.cancel.clone().unwrap_or_default();
        api.get(k.path(), &q, &cancel, move |r| {
            signal.post(Msg::Page { kind: i, seq, r });
        });
    }

    /// A page of kind `i`'s list: the next asked for, or the store replaced with them all and
    /// the watch made. Whether the part is to be sent at once.
    fn page(
        &self,
        api: &mut Api,
        kinds: &mut [KindState],
        i: usize,
        r: Result<Vec<u8>, Failure>,
        now: Instant,
    ) -> bool {
        let Some(k) = kinds.get_mut(i) else {
            return false;
        };
        let first_page = matches!(&k.phase, Phase::Listing { objs, .. } if objs.is_empty());
        let body = match r {
            Ok(b) => b,
            // The next of its versions, if it has one.
            Err(Failure::NotFound) if first_page && k.at < k.kind.older.len() => {
                k.at = k.at.saturating_add(1);
                self.get_page(api, k, i, "");
                return false;
            }
            Err(Failure::NotFound) => {
                if !k.missing {
                    k.missing = true;
                    self.log(&format!(
                        "{} are not served by the API server (is the Gateway API installed?); looking again every {}",
                        k.kind.name,
                        akit_config::duration::format(self.t.max_backoff)
                    ));
                }
                replace(k, BTreeMap::new(), String::new(), false);
                self.signal.changed();
                k.phase = Phase::Wait {
                    at: now.checked_add(self.t.max_backoff).unwrap_or(now),
                    next: Next::List,
                };
                return false;
            }
            Err(Failure::Gone) if matches!(&k.phase, Phase::Listing { objs, .. } if !objs.is_empty()) =>
            {
                // The continue token expired: start over.
                k.phase = Phase::Listing {
                    objs: BTreeMap::new(),
                    truncated: false,
                };
                self.get_page(api, k, i, "");
                return false;
            }
            Err(e) => {
                self.failed(k, &format!("listing {}: {e}", k.kind.name), now);
                return false;
            }
        };
        let page = match parse_page(&body, k.kind) {
            Ok(p) => p,
            Err(e) => {
                self.failed(k, &format!("listing {}: {e}", k.kind.name), now);
                return false;
            }
        };
        let Phase::Listing { objs, truncated } = &mut k.phase else {
            return false;
        };
        for (key, o) in page.items {
            if objs.len() >= self.opts.max_objects {
                *truncated = true;
                break;
            }
            objs.insert(key, Arc::new(o));
        }
        if !page.cont.is_empty() && !*truncated {
            self.get_page(api, k, i, &page.cont);
            return false;
        }
        let (objs, truncated) = (std::mem::take(objs), *truncated);
        replace(k, objs, page.rv.clone(), truncated);
        if k.missing {
            k.missing = false;
            self.log(&format!("{} are served now", k.kind.name));
        }
        k.backoff = self.t.min_backoff;
        k.phase = Phase::Wait {
            at: now,
            next: Next::Watch(page.rv),
        };
        // The first time every kind is listed, the part goes at once.
        let first = kinds.iter().all(|k| lock(&k.store).synced) && self.last.is_none();
        if !first {
            self.signal.changed();
        }
        first
    }

    /// Kind `k`'s request failed: said, and listed again after its backoff.
    fn failed(&self, k: &mut KindState, msg: &str, now: Instant) {
        self.log(&format!(
            "{msg}; trying again in {}",
            akit_config::duration::format(k.backoff)
        ));
        k.phase = Phase::Wait {
            at: now.checked_add(k.backoff).unwrap_or(now),
            next: Next::List,
        };
        k.backoff = k.backoff.saturating_mul(2).min(self.t.max_backoff);
    }

    /// Kind `k`'s watch ended: made again from where it was when the server ended it in time,
    /// the kind listed again when it was too old, and after its backoff when it failed.
    fn watch_ended(&self, k: &mut KindState, r: Result<(), Failure>, now: Instant) {
        // A watch that failed on an event is still open: it is closed.
        if let Some(c) = k.cancel.take() {
            c.cancel();
        }
        let full = lock(&k.store).full;
        if full && !k.full {
            self.report_full(k);
        }
        k.full = full;
        match r {
            Ok(()) => {
                let rv = lock(&k.store).rv.clone();
                k.phase = Phase::Wait {
                    at: now,
                    next: Next::Watch(rv),
                };
            }
            Err(Failure::Gone) => {
                k.phase = Phase::Wait {
                    at: now,
                    next: Next::List,
                };
            }
            Err(e) => {
                self.log(&format!(
                    "watching {}: {e}; listing them again in {}",
                    k.kind.name,
                    akit_config::duration::format(k.backoff)
                ));
                k.phase = Phase::Wait {
                    at: now.checked_add(k.backoff).unwrap_or(now),
                    next: Next::List,
                };
                k.backoff = k.backoff.saturating_mul(2).min(self.t.max_backoff);
            }
        }
    }

    fn report_full(&self, k: &KindState) {
        self.log(&format!(
            "more than max_objects, {}, {}: the rest are left out",
            self.opts.max_objects, k.kind.name
        ));
    }

    /// Translates what the stores hold and sends it, when it changed or `always` is set, once
    /// every kind has been listed; then writes the statuses that changed.
    fn send(&mut self, api: &mut Api, kinds: &mut [KindState], always: bool) {
        let mut snap = Snapshot::default();
        for k in kinds.iter_mut() {
            let st = lock(&k.store);
            if !st.synced {
                return;
            }
            let objs: Vec<Arc<Object>> = st.objs.values().cloned().collect();
            let full = st.full;
            drop(st);
            if full && !k.full {
                self.report_full(k);
            }
            k.full = full;
            *match k.kind.shape {
                Shape::IngressClass => &mut snap.ingress_classes,
                Shape::Ingress => &mut snap.ingresses,
                Shape::Service => &mut snap.services,
                Shape::Slice => &mut snap.slices,
                Shape::Secret => &mut snap.secrets,
                Shape::Namespace => &mut snap.namespaces,
                Shape::GatewayClass => &mut snap.gateway_classes,
                Shape::Gateway => &mut snap.gateways,
                Shape::HttpRoute => &mut snap.http_routes,
                Shape::GrpcRoute => &mut snap.grpc_routes,
                Shape::TlsRoute => &mut snap.tls_routes,
                Shape::TcpRoute => &mut snap.tcp_routes,
                Shape::UdpRoute => &mut snap.udp_routes,
                Shape::Grant => &mut snap.grants,
            } = objs;
        }
        let dir = self.cert_dir.as_ref().map(|d| d.display().to_string());
        let mut t = Translator::new(&self.name, &self.opts, dir);
        let part = t.run(&snap);
        let mut reported = BTreeSet::new();
        for msg in std::mem::take(&mut t.problems) {
            if !self.problems.contains(&msg) {
                self.log(&msg);
            }
            reported.insert(msg);
        }
        self.problems = reported;
        if let Err(e) = write_certs(&t.certs) {
            self.log(&format!(
                "writing certificates: {e}; the change is not sent"
            ));
            return;
        }
        if always || self.last.as_ref() != Some(&part) {
            self.last = Some(part.clone());
            self.update.send(part);
        }
        if let Some(d) = &self.cert_dir {
            remove_certs(d, &t.certs);
        }
        // The last set made replaces what was still to write of the one before.
        self.writes = t.statuses(SystemTime::now()).into();
        self.write_next(api, kinds);
    }
}

/// The earliest time a kind waits for.
fn api_state(kinds: &[KindState]) -> Option<Instant> {
    kinds
        .iter()
        .filter_map(|k| match k.phase {
            Phase::Wait { at, .. } => Some(at),
            _ => None,
        })
        .min()
}

fn replace(k: &mut KindState, objs: BTreeMap<String, Arc<Object>>, rv: String, full: bool) {
    let mut st = lock(&k.store);
    st.objs = objs;
    st.synced = true;
    st.rv = rv;
    st.full = full;
}

/// A page of a list.
struct Page {
    items: Vec<(String, Object)>,
    rv: String,
    cont: String,
}

fn parse_page(body: &[u8], k: Kind) -> Result<Page, String> {
    let text = std::str::from_utf8(body).map_err(|_| "not UTF-8".to_owned())?;
    let v = value::parse(text).map_err(|e| e.to_string())?;
    let m = v.get("metadata").unwrap_or(&Value::Null);
    let s = |k: &str| match m.get(k) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    let mut items = Vec::new();
    if let Some(Value::Array(a)) = v.get("items") {
        for it in a {
            let o = decode(k.shape, it).map_err(|e| format!("{}: {e}", k.name))?;
            items.push((o.meta().key(), o));
        }
    }
    Ok(Page {
        items,
        rv: s("resourceVersion"),
        cont: s("continue"),
    })
}

/// Makes `d`, readable by akit's user alone; with `all`, its parents too, and it may be there.
fn make_dir(d: &Path, all: bool) -> io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(all);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        b.mode(0o700);
    }
    b.create(d)
}

/// The name of a certificate file the provider writes: `<namespace>.<secret>.<hash>.pem`.
fn is_cert_file(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".pem") else {
        return false;
    };
    let Some((rest, hash)) = stem.rsplit_once('.') else {
        return false;
    };
    let Some((ns, secret)) = rest.split_once('.') else {
        return false;
    };
    hash.len() == 12
        && hash
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        && !ns.is_empty()
        && ns
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        && !secret.is_empty()
        && secret
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'.')
}

/// Writes the certificate files `certs`, by path, that are not there yet: a file's name holds
/// a hash of its contents, so one there is the same. Each is written whole, then renamed into
/// place.
fn write_certs(certs: &BTreeMap<String, Vec<u8>>) -> io::Result<()> {
    for (path, data) in certs {
        let p = Path::new(path);
        if p.exists() {
            continue;
        }
        let tmp = PathBuf::from(format!("{path}.tmp"));
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            o.mode(0o600);
        }
        let r = o.open(&tmp).and_then(|mut f| {
            use std::io::Write as _;
            f.write_all(data)?;
            f.sync_all()
        });
        if let Err(e) = r.and_then(|()| std::fs::rename(&tmp, p)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    }
    Ok(())
}

/// Removes the certificate files in `dir` that `certs` no longer holds.
fn remove_certs(dir: &Path, certs: &BTreeMap<String, Vec<u8>>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let path = dir.join(name);
        if is_cert_file(name) && !certs.contains_key(&path.display().to_string()) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The `provider_kubernetes` fuzz target (feature `fuzzing`; I77): [`crate::fuzzing`].
#[cfg(any(test, feature = "fuzzing"))]
pub(crate) mod fuzz {
    #![allow(
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        clippy::panic
    )]

    use std::sync::Arc;
    use std::time::Duration;

    use akit_config::{KubernetesIngress, KubernetesProvider, value};

    use super::translate::{Snapshot, Translator};
    use super::types::{Object, Shape, decode};
    use super::{GATEWAY_KINDS, INGRESS_KINDS, parse_page};

    const SHAPES: [Shape; 14] = [
        Shape::IngressClass,
        Shape::Ingress,
        Shape::Service,
        Shape::Slice,
        Shape::Secret,
        Shape::Namespace,
        Shape::GatewayClass,
        Shape::Gateway,
        Shape::HttpRoute,
        Shape::GrpcRoute,
        Shape::TlsRoute,
        Shape::TcpRoute,
        Shape::UdpRoute,
        Shape::Grant,
    ];

    /// The cluster every input starts from: fit's GatewayClass and IngressClass, a Gateway
    /// with an HTTP, a wildcard and an HTTPS listener, and a Service with endpoints.
    fn world() -> Snapshot {
        let o = |shape: Shape, doc: &str| -> Arc<Object> {
            Arc::new(
                decode(shape, &value::parse(doc).unwrap_or(value::Value::Null))
                    .unwrap_or_else(|e| panic!("{doc}: {e}")),
            )
        };
        Snapshot {
            namespaces: vec![o(
                Shape::Namespace,
                r#"{"metadata": {"name": "default", "labels": {"gateway": "yes"}}}"#,
            )],
            gateway_classes: vec![o(
                Shape::GatewayClass,
                r#"{"metadata": {"name": "fit"}, "spec": {"controllerName": "lesismal.github.io/fit"}}"#,
            )],
            gateways: vec![o(
                Shape::Gateway,
                r#"{"metadata": {"namespace": "default", "name": "gw"}, "spec": {"gatewayClassName": "fit", "listeners": [
  {"name": "http", "port": 80, "protocol": "HTTP", "allowedRoutes": {"namespaces": {"from": "All"}}},
  {"name": "web", "port": 8080, "protocol": "HTTP", "hostname": "*.example.com"},
  {"name": "tls", "port": 443, "protocol": "HTTPS", "tls": {"certificateRefs": [{"name": "cert"}]}},
  {"name": "tcp", "port": 9000, "protocol": "TCP"}]}}"#,
            )],
            ingress_classes: vec![o(
                Shape::IngressClass,
                r#"{"metadata": {"name": "fit", "annotations": {"ingressclass.kubernetes.io/is-default-class": "true"}}, "spec": {"controller": "lesismal.github.io/fit"}}"#,
            )],
            services: vec![o(
                Shape::Service,
                r#"{"metadata": {"namespace": "default", "name": "web"}, "spec": {"clusterIP": "10.96.0.1", "ports": [{"name": "http", "port": 80}]}}"#,
            )],
            slices: vec![o(
                Shape::Slice,
                r#"{"metadata": {"namespace": "default", "name": "web-abc", "labels": {"kubernetes.io/service-name": "web"}},
  "addressType": "IPv4", "endpoints": [{"addresses": ["10.0.0.1"], "conditions": {"ready": true}}], "ports": [{"name": "http", "protocol": "TCP", "port": 8000}]}"#,
            )],
            ..Snapshot::default()
        }
    }

    /// `provider_kubernetes`: what the API server sends the Kubernetes provider (I59, I60).
    /// `data[0]` picks the options; the rest is NUL-separated documents, each a byte that picks
    /// its kind and then its JSON: a list page (read as the API server's pages are) when the
    /// option bit says so, else an object added to the cluster. Whatever the objects, the
    /// provider writes a part the merge reads.
    pub(crate) fn kubernetes(data: &[u8]) {
        let Some((&flags, rest)) = data.split_first() else {
            return;
        };
        let opts = KubernetesProvider {
            kubeconfig: String::new(),
            context: String::new(),
            controller_name: akit_config::DEFAULT_KUBERNETES_CONTROLLER_NAME.to_owned(),
            namespaces: if flags & 2 != 0 {
                vec!["default".into()]
            } else {
                Vec::new()
            },
            listen_host: String::new(),
            addresses: Vec::new(),
            backends: if flags & 4 != 0 {
                akit_config::KubernetesBackends::ClusterIp
            } else {
                akit_config::KubernetesBackends::Endpoints
            },
            max_objects: 10_000,
            request_timeout: Duration::from_secs(10),
            ingress: (flags & 8 != 0).then_some(KubernetesIngress {
                http_port: 80,
                https_port: 443,
            }),
            certificates_dir: String::new(),
        };
        let mut s = world();
        for doc in rest.split(|&b| b == 0).take(64) {
            let Some((&k, json)) = doc.split_first() else {
                continue;
            };
            if flags & 1 != 0 {
                let kinds: Vec<_> = GATEWAY_KINDS.iter().chain(INGRESS_KINDS.iter()).collect();
                let kind = kinds[usize::from(k) % kinds.len()];
                if let Ok(page) = parse_page(json, *kind) {
                    for (_, o) in page.items {
                        add(&mut s, kind.shape, Arc::new(o));
                    }
                }
                continue;
            }
            let shape = SHAPES[usize::from(k) % SHAPES.len()];
            let Some(v) = std::str::from_utf8(json)
                .ok()
                .and_then(|t| value::parse(t).ok())
            else {
                continue;
            };
            if let Ok(o) = decode(shape, &v) {
                add(&mut s, shape, Arc::new(o));
            }
        }
        let mut t = Translator::new("kubernetes", &opts, None);
        let part = t.run(&s);
        if let Err(e) = crate::merge::parse("kubernetes", &part) {
            panic!(
                "the part does not read: {e}\n{}",
                String::from_utf8_lossy(&part)
            );
        }
    }

    fn add(s: &mut Snapshot, shape: Shape, o: Arc<Object>) {
        let list = match shape {
            Shape::IngressClass => &mut s.ingress_classes,
            Shape::Ingress => &mut s.ingresses,
            Shape::Service => &mut s.services,
            Shape::Slice => &mut s.slices,
            Shape::Secret => &mut s.secrets,
            Shape::Namespace => &mut s.namespaces,
            Shape::GatewayClass => &mut s.gateway_classes,
            Shape::Gateway => &mut s.gateways,
            Shape::HttpRoute => &mut s.http_routes,
            Shape::GrpcRoute => &mut s.grpc_routes,
            Shape::TlsRoute => &mut s.tls_routes,
            Shape::TcpRoute => &mut s.tcp_routes,
            Shape::UdpRoute => &mut s.udp_routes,
            Shape::Grant => &mut s.grants,
        };
        list.push(o);
    }
}

#[cfg(test)]
mod gateway_tests;
#[cfg(test)]
mod tests;
