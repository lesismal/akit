//! The runtime graph: a config compiled into what the data plane reads, every module provisioned
//! before the graph exists, nothing in it changed afterwards (rule 12).
//!
//! A graph is published, read and retired by a [`GraphCell`](crate::GraphCell). Its modules are
//! cleaned up exactly once: when the cell retires it, or, if it never was, when the last reference
//! to it is dropped.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use akit_config::{Config, Error, Errors, Listener};
use akit_upstream::Pool;

use crate::module::{Env, Modules, Registry};
use crate::traits::Handler;

/// The params a module is provisioned from: a route's handler or one of its middleware (I40), as
/// akit-config read it, its module named by [`akit_config::Handler::module`]. akit-config decodes
/// and checks each module's params, with their paths and positions, when the config is read
/// (I02); modules take the typed value.
pub type HandlerParams = akit_config::Handler;

/// A route, compiled.
pub struct CompiledRoute {
    /// The route's name, if it has one.
    pub name: Option<String>,
    /// The listeners it serves on; every HTTP listener when empty.
    pub listeners: Vec<String>,
    /// Its priority.
    pub priority: i64,
    /// `match.host`.
    pub host: Vec<String>,
    /// `match.path`.
    pub path: Vec<String>,
    /// The rest of `match`, built (I39): method, remote IP, header, query, path regexp and
    /// expression, the cheapest first, as fit's graph orders them.
    pub matchers: Vec<akit_router::Matcher>,
    /// The handler, its middleware chain built around it (I40), the first middleware outermost;
    /// provisioned.
    pub handler: Arc<dyn Handler>,
}

impl fmt::Debug for CompiledRoute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompiledRoute")
            .field("name", &self.name)
            .field("listeners", &self.listeners)
            .field("priority", &self.priority)
            .field("host", &self.host)
            .field("path", &self.path)
            .field("matchers", &self.matchers.len())
            .finish_non_exhaustive()
    }
}

/// Where a graph counts its requests (I37): the registry, and per listener the slot of each
/// route that serves on it (by route index) and of the requests no route takes, as fit's
/// `Listener.Requests` and `Unmatched`.
struct Requests {
    metrics: akit_observe::metrics::Metrics,
    /// Per listener: per route, its slot ([`NO_SLOT`] for a route not on the listener, or past
    /// the bound); then the unmatched requests' slot.
    slots: Vec<(Box<[u32]>, u32)>,
}

/// No slot: not counted.
pub const NO_SLOT: u32 = u32::MAX;

/// The name a route's requests are counted under: its own, or `#` and its place in the config's
/// routes for one without (fit's `metricsName`).
#[must_use]
pub fn metrics_name(name: Option<&str>, index: usize) -> String {
    match name {
        Some(n) if !n.is_empty() => n.to_owned(),
        _ => format!("#{index}"),
    }
}

/// A config, compiled: immutable, shared by every I/O thread, its modules provisioned.
pub struct Graph {
    listeners: Vec<Listener>,
    routes: Vec<CompiledRoute>,
    /// Per listener, in the order of `listeners`: its routes' router, to indices of `routes`.
    routers: Vec<akit_router::Router<usize>>,
    /// Per listener, in the order of `listeners`: its TLS, loaded from its files, when it has
    /// `tls` (its certificates, served per handshake through [`crate::ListenerCerts`]).
    tls: Vec<Option<akit_tls::ListenerTls>>,
    /// The SHA-256 of every certificate, key and authority file its listeners (I30) and its h2
    /// upstreams (I54) loaded, by path: what `run -watch-certs` compares the files with.
    cert_files: std::collections::BTreeMap<String, akit_tls::Sum>,
    /// The env's ACME manager, when it has one (I31): it outlives the graph, and once the graph
    /// is current it is told to manage `acme_names`.
    acme: Option<akit_tls::Acme>,
    /// The names the listeners list in `tls.acme`, each once, in the config's order.
    acme_names: Vec<String>,
    /// The upstreams' pools, in the config's order.
    upstreams: Vec<Pool>,
    /// The layer 4 services (I51), in the config's order.
    services: Vec<akit_l4::Service>,
    /// Per listener, in the order of `listeners`: the services that take its connections, by
    /// server name (I52), for a `tcp` or `udp` listener.
    l4: Vec<Option<akit_l4::Entry>>,
    /// The access log its requests' lines go to: the env's, when the config has `access_log`.
    access_log: Option<akit_observe::AccessLog>,
    /// The process's tracer, which samples its requests (I64).
    tracer: Option<akit_observe::Tracer>,
    /// Where its requests are counted, when the config has `metrics` (I37) or
    /// `telemetry.metrics` (I64).
    requests: Option<Requests>,
    /// The config's `limits.write_stall_timeout` (I17b).
    write_stall_timeout: std::time::Duration,
    /// Taken (and so cleaned up) once, by [`Graph::retire`] or on drop.
    modules: Mutex<Option<Modules>>,
    /// Requests and connections that keep this graph past their handler's call
    /// ([`Hold`](crate::Hold)); a graph is not retired while any does.
    pub(crate) holds: AtomicUsize,
    /// Set when its cell is gone: the last [`Hold`](crate::Hold) to drop retires it, since no
    /// `collect` will.
    pub(crate) orphaned: AtomicBool,
}

impl Graph {
    /// The config's `limits.write_stall_timeout`: how long a response may wait for its client to
    /// take what it was sent, which is akio's engine `write_timeout` (akit-listener sets it).
    #[must_use]
    pub fn write_stall_timeout(&self) -> std::time::Duration {
        self.write_stall_timeout
    }

    /// The listeners, as the config gives them.
    #[must_use]
    pub fn listeners(&self) -> &[Listener] {
        &self.listeners
    }

    /// The TLS of the listener at `listener` ([`listener_index`](Self::listener_index)), loaded
    /// from its files when the graph was compiled; `None` for one in the clear. Its
    /// configurations are built around a manager with [`akit_tls::ListenerTls::server_configs`]
    /// ([`crate::ListenerCerts`] for a listener that serves the current graph's certificates).
    #[must_use]
    pub fn tls(&self, listener: usize) -> Option<&akit_tls::ListenerTls> {
        self.tls.get(listener)?.as_ref()
    }

    /// The certificates of the listener named `name`, when it has `tls`: what a handshake on it
    /// is served from while this graph is current. Looks the listener up by name (there are
    /// few): once per full TLS handshake, not per request.
    #[must_use]
    pub fn certs(&self, name: &str) -> Option<&Arc<akit_tls::Certs>> {
        self.listener_tls(name).map(akit_tls::ListenerTls::certs)
    }

    /// The TLS of the listener named `name`, when it has `tls`: its certificates from files and
    /// from ACME ([`akit_tls::ListenerTls::get`]).
    #[must_use]
    pub fn listener_tls(&self, name: &str) -> Option<&akit_tls::ListenerTls> {
        let i = self.listener_index(name)?;
        self.tls.get(i)?.as_ref()
    }

    /// The ACME manager its listeners' `tls.acme` names are obtained by, and whose HTTP-01
    /// answers its listeners in the clear give (I31).
    #[must_use]
    pub fn acme(&self) -> Option<&akit_tls::Acme> {
        self.acme.as_ref()
    }

    /// The names its listeners list in `tls.acme`, each once: what the ACME manager is told to
    /// manage once the graph is current.
    #[must_use]
    pub fn acme_names(&self) -> &[String] {
        &self.acme_names
    }

    /// The SHA-256 of every certificate, key and authority file the listeners and the h2
    /// upstreams loaded, by path.
    #[must_use]
    pub fn cert_files(&self) -> &std::collections::BTreeMap<String, akit_tls::Sum> {
        &self.cert_files
    }

    /// The routes, in the config's order.
    #[must_use]
    pub fn routes(&self) -> &[CompiledRoute] {
        &self.routes
    }

    /// The upstreams' pools, in the config's order. Each is closed, and drains, when the graph is
    /// retired.
    #[must_use]
    pub fn upstreams(&self) -> &[Pool] {
        &self.upstreams
    }

    /// The layer 4 services (I51), in the config's order.
    #[must_use]
    pub fn services(&self) -> &[akit_l4::Service] {
        &self.services
    }

    /// The services that take the connections of the listener at `listener`, a `tcp` or `udp`
    /// one (I51), by the server names of their ClientHellos (I52).
    #[must_use]
    pub fn entry(&self, listener: usize) -> Option<&akit_l4::Entry> {
        self.l4.get(listener)?.as_ref()
    }

    /// The access log the requests this graph serves are written to, if any (I21).
    #[must_use]
    pub fn access_log(&self) -> Option<&akit_observe::AccessLog> {
        self.access_log.as_ref()
    }

    /// The tracer that samples its requests and records their spans, if akit traces (I64).
    #[must_use]
    pub fn tracer(&self) -> Option<&akit_observe::Tracer> {
        self.tracer.as_ref()
    }

    /// The metrics registry its requests are counted in, when the config has `metrics` (I37) or
    /// `telemetry.metrics` (I64).
    #[must_use]
    pub fn metrics(&self) -> Option<&akit_observe::metrics::Metrics> {
        self.requests.as_ref().map(|r| &r.metrics)
    }

    /// The slot the requests the route at `route` (an index of [`routes`](Self::routes), or
    /// `None` for those no route takes) answers on the listener at `listener` are counted in
    /// ([`akit_observe::metrics::Shard::observe`]); `None` when requests are not counted. Runs
    /// per request: it only reads.
    #[must_use]
    pub fn request_slot(&self, listener: usize, route: Option<usize>) -> Option<u32> {
        let (routes, unmatched) = self.requests.as_ref()?.slots.get(listener)?;
        let slot = match route {
            Some(i) => *routes.get(i)?,
            None => *unmatched,
        };
        (slot != NO_SLOT).then_some(slot)
    }

    /// The certificates the listener at `listener` serves, for the metrics: none for one in the
    /// clear.
    #[must_use]
    /// Parses each one: control plane only.
    pub fn certificates(&self, listener: usize) -> Vec<akit_tls::CertificateInfo> {
        self.tls
            .get(listener)
            .and_then(Option::as_ref)
            .map(|t| {
                t.certs()
                    .certificates()
                    .filter_map(akit_tls::CertificateInfo::of)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The pool of the upstream named `name`.
    #[must_use]
    pub fn upstream(&self, name: &str) -> Option<&Pool> {
        self.upstreams.iter().find(|p| p.name() == name)
    }

    /// The index of the listener named `name`.
    #[must_use]
    pub fn listener_index(&self, name: &str) -> Option<usize> {
        self.listeners.iter().position(|l| l.name == name)
    }

    /// The route that serves a request to `host` (the `Host` field) and `path` (canonical and
    /// percent-decoded: [`akit_router::Canonical::decoded`]) on the listener at `listener`
    /// ([`listener_index`](Self::listener_index)), as fit's router picks it; `None` when no route
    /// matches, or there is no such listener. Runs per request: it only reads, and does not
    /// allocate.
    #[must_use]
    pub fn route(
        &self,
        listener: usize,
        host: &str,
        path: impl AsRef<[u8]>,
    ) -> Option<&CompiledRoute> {
        let i = *self.routers.get(listener)?.find(host, path)?;
        self.routes.get(i)
    }

    /// The route that serves request `r` on the listener at `listener`: [`route`](Self::route)
    /// with the routes' other matchers (method, header, query, remote IP, path regexps,
    /// expressions) run on `r`. Runs per request: it only reads, and does not allocate.
    #[must_use]
    pub fn route_request<R: akit_router::Request + ?Sized>(
        &self,
        listener: usize,
        r: &R,
    ) -> Option<&CompiledRoute> {
        self.route_request_at(listener, r).map(|(_, route)| route)
    }

    /// [`route_request`](Self::route_request), with the route's index among
    /// [`routes`](Self::routes).
    #[must_use]
    pub fn route_request_at<R: akit_router::Request + ?Sized>(
        &self,
        listener: usize,
        r: &R,
    ) -> Option<(usize, &CompiledRoute)> {
        let i = *self.routers.get(listener)?.find_request(r)?;
        self.routes.get(i).map(|route| (i, route))
    }

    /// The IDs of the modules provisioned for this graph, in order; empty once it is retired.
    #[must_use]
    pub fn module_ids(&self) -> Vec<&'static str> {
        self.lock()
            .as_ref()
            .map(|m| m.ids().collect())
            .unwrap_or_default()
    }

    /// Whether the graph's modules have been cleaned up.
    #[must_use]
    pub fn is_retired(&self) -> bool {
        self.lock().is_none()
    }

    /// Cleans the modules up, if that has not been done; true when this call did it.
    pub(crate) fn retire(&self) -> bool {
        let modules = self.lock().take();
        modules.map(Modules::release).is_some()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Modules>> {
        // A cleanup that panicked poisons nothing worth refusing: the Option is still sound.
        self.modules.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl fmt::Debug for Graph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Graph")
            .field("listeners", &self.listeners)
            .field("routes", &self.routes)
            .field("upstreams", &self.upstreams)
            .field("modules", &self.module_ids())
            .field("holds", &self.holds.load(Ordering::Relaxed))
            .finish()
    }
}

/// The ID the upstream pools are recorded under among a graph's modules, whose cleanup closes
/// them.
pub const UPSTREAM_POOLS: &str = "upstream.pools";

/// The ID the graph's lease of the metrics registry is recorded under among its modules, whose
/// cleanup gives back its counters (I37).
pub const METRICS: &str = "metrics";

/// Compiles `config` into a graph: a pool made for each upstream (I14), dialing through `env`'s
/// engine, then every route's handler and middleware loaded from `registry` and provisioned with
/// `env` and the pools, the middleware chained around the handler once, here (rule 12). The pools are cleaned up after the handlers, which may use them: each is closed, and
/// drains its requests in flight, when the graph is retired (decision 004). Nothing is published: a config that fails leaves nothing behind (the modules
/// provisioned before the failure are cleaned up), and the running graph is untouched.
///
/// Errors carry the path of what failed, as akit-config's do, without a position (the config was
/// already read).
pub fn compile(
    config: &Config,
    registry: &Registry<HandlerParams>,
    env: &Env,
) -> Result<Graph, Errors> {
    let mut modules = Modules::new();
    // What the graph takes from the metrics registry, given back when it is retired (I37).
    let mut lease = env.metrics().map(akit_observe::metrics::Metrics::lease);
    let mut errors = Vec::new();
    // The files the upstreams' TLS was read from (I54), watched with the listeners'.
    let mut upstream_files = std::collections::BTreeMap::new();
    let upstreams: Vec<Pool> = config
        .upstreams
        .iter()
        .enumerate()
        .map(|(i, u)| {
            // An h2 upstream's CA and client certificate, read here as fit's `upstream.New`
            // reads them: a config whose files do not load fails, `validate` included.
            let tls = match akit_tls::UpstreamTls::load(u) {
                Ok(t) => t.map(|t| {
                    upstream_files.extend(t.files().iter().map(|(p, s)| (p.clone(), *s)));
                    t.config().clone()
                }),
                Err(e) => {
                    errors.push(Error {
                        file: None,
                        path: format!("upstreams[{i}].{}", e.path),
                        line: 0,
                        col: 0,
                        msg: e.msg,
                    });
                    None
                }
            };
            let pool = Pool::new_with_tls(u, env.engine().clone(), tls);
            match lease.as_mut() {
                Some(l) => {
                    let addrs: Vec<&str> = u.addresses.iter().map(String::as_str).collect();
                    pool.with_stats(l.upstream(&u.name, &addrs))
                }
                None => pool,
            }
        })
        .collect();
    if !upstreams.is_empty() {
        // Recorded first, so cleaned up last.
        let pools = upstreams.clone();
        modules.push_cleanup(UPSTREAM_POOLS, move || pools.iter().for_each(Pool::close));
    }
    let env = &env.with_upstreams(upstreams.clone());
    let mut routes = Vec::with_capacity(config.routes.len());
    for (i, r) in config.routes.iter().enumerate() {
        let handler = match registry.handler(r.handler.module(), &r.handler, env, &mut modules) {
            Ok(h) => h,
            Err(e) => {
                errors.push(load_error(format!("routes[{i}].handler"), &e));
                continue;
            }
        };
        let mut middleware = Vec::with_capacity(r.middleware.len());
        for (j, m) in r.middleware.iter().enumerate() {
            match registry.middleware(m.module(), m, env, &mut modules) {
                Ok(mw) => middleware.push(mw),
                Err(e) => errors.push(load_error(format!("routes[{i}].middleware[{j}]"), &e)),
            }
        }
        if middleware.len() != r.middleware.len() {
            continue;
        }
        let handler = crate::traits::chain(&middleware, handler);
        let (host, path) = r
            .matcher
            .as_ref()
            .map(|m| (m.host.clone(), m.path.clone()))
            .unwrap_or_default();
        let matchers = match r.matcher.as_ref().map(matchers).transpose() {
            Ok(ms) => ms.unwrap_or_default(),
            Err(e) => {
                // The config's checks build the same matchers: not reached with a checked one.
                errors.push(Error {
                    file: None,
                    path: format!("routes[{i}].match"),
                    line: 0,
                    col: 0,
                    msg: e.to_string(),
                });
                continue;
            }
        };
        routes.push(CompiledRoute {
            name: r.name.clone(),
            listeners: r.listeners.clone(),
            priority: r.priority,
            host,
            path,
            matchers,
            handler,
        });
    }
    // Each TLS listener's certificates, read from their files, as fit's graph loads them: a
    // config whose files do not load fails here, `validate` included.
    let mut tls = Vec::with_capacity(config.listeners.len());
    for (i, l) in config.listeners.iter().enumerate() {
        tls.push(l.tls.as_ref().and_then(|t| {
            akit_tls::ListenerTls::load_with(t, &l.protocols, env.acme())
                .map_err(|e| {
                    let mut path = format!("listeners[{i}].tls");
                    if !e.path.is_empty() {
                        path = format!("{path}.{}", e.path);
                    }
                    errors.push(Error {
                        file: None,
                        path,
                        line: 0,
                        col: 0,
                        msg: e.msg,
                    });
                })
                .ok()
        }));
    }
    let mut acme_names: Vec<String> = Vec::new();
    for n in tls
        .iter()
        .flatten()
        .flat_map(akit_tls::ListenerTls::acme_names)
    {
        if !acme_names.contains(n) {
            acme_names.push(n.clone());
        }
    }
    let mut cert_files: std::collections::BTreeMap<String, akit_tls::Sum> = tls
        .iter()
        .flatten()
        .flat_map(|t| t.files().iter().map(|(p, s)| (p.clone(), *s)))
        .collect();
    cert_files.extend(upstream_files);
    if !errors.is_empty() {
        // `modules` drops here, cleaning up what was provisioned, and `lease` gives back what it
        // took.
        return Err(Errors(errors));
    }
    // Each listener's slots, as fit's graph takes them: every route on it, and the requests no
    // route takes.
    // With `metrics`, or with `telemetry.metrics`, which exports the same counts (I64), as fit's.
    let counted = config.metrics.is_some()
        || config
            .telemetry
            .as_ref()
            .is_some_and(|t| t.metrics.is_some());
    let requests = match (counted, lease.as_mut()) {
        (true, Some(l)) => Some(Requests {
            metrics: l.metrics().clone(),
            slots: config
                .listeners
                .iter()
                .map(|li| {
                    let routes = routes
                        .iter()
                        .enumerate()
                        .map(|(i, r)| {
                            if serves(li, r) {
                                l.route(&li.name, &metrics_name(r.name.as_deref(), i))
                                    .unwrap_or(NO_SLOT)
                            } else {
                                NO_SLOT
                            }
                        })
                        .collect();
                    (routes, l.route(&li.name, "").unwrap_or(NO_SLOT))
                })
                .collect(),
        }),
        _ => None,
    };
    if let Some(l) = lease {
        modules.push_cleanup(METRICS, move || drop(l));
    }
    // Built here, not per request (rule 12). A route with no listeners serves on every one
    // ([`serves`]).
    let routers = config
        .listeners
        .iter()
        .map(|l| {
            akit_router::Router::new(
                routes
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| serves(l, r))
                    .map(|(i, r)| akit_router::Route {
                        hosts: r.host.clone(),
                        paths: r.path.clone(),
                        matchers: r.matchers.clone(),
                        priority: r.priority,
                        value: i,
                    })
                    .collect(),
            )
        })
        .collect();
    // Each service with its upstream's pool (the config's checks make sure it has one), and
    // each listener's service.
    let services: Vec<akit_l4::Service> = config
        .services
        .iter()
        .filter_map(|s| {
            let pool = upstreams.iter().find(|p| p.name() == s.upstream)?;
            Some(akit_l4::Service::new(s, pool.clone()))
        })
        .collect();
    let l4 = config
        .listeners
        .iter()
        .map(|l| {
            let mine: Vec<(akit_l4::Service, &[String])> = config
                .services
                .iter()
                .filter(|c| c.listeners.contains(&l.name))
                .filter_map(|c| {
                    let s = services.iter().find(|s| s.name() == c.name)?;
                    Some((s.clone(), c.server_names.as_slice()))
                })
                .collect();
            (!mine.is_empty()).then(|| akit_l4::Entry::of(mine))
        })
        .collect();
    Ok(Graph {
        listeners: config.listeners.clone(),
        routes,
        routers,
        tls,
        cert_files,
        acme: env.acme().cloned(),
        acme_names,
        upstreams,
        services,
        l4,
        access_log: config.access_log.as_ref().and(env.access_log().cloned()),
        tracer: env.tracer().cloned(),
        requests,
        write_stall_timeout: config.limits.write_stall_timeout,
        modules: Mutex::new(Some(modules)),
        holds: AtomicUsize::new(0),
        orphaned: AtomicBool::new(false),
    })
}

/// Whether the route `r` serves on the listener `l`: on every one when it names none, but for a
/// listener that redirects to HTTPS (`auto_https`, I33), which serves no routes.
fn serves(l: &Listener, r: &CompiledRoute) -> bool {
    l.redirect_https == 0 && (r.listeners.is_empty() || r.listeners.contains(&l.name))
}

/// The error of a module at `path` (a route's handler or one of its middleware) that failed to
/// load: at its params' field, when the error names one.
fn load_error(mut path: String, e: &crate::LoadError) -> Error {
    if let crate::LoadError::Provision { error, .. } = e {
        if let Some(field) = error.field_name() {
            path = format!("{path}.params.{field}");
        }
    }
    Error {
        file: None,
        path,
        line: 0,
        col: 0,
        msg: e.to_string(),
    }
}

/// The matchers `m` asks for besides hosts and paths, the cheapest first (fit's
/// `runtime/graph.go` `matchers`).
fn matchers(m: &akit_config::Match) -> Result<Vec<akit_router::Matcher>, akit_router::Error> {
    use akit_router::Matcher;
    let mut ms = Vec::new();
    if !m.method.is_empty() {
        ms.push(Matcher::methods(&m.method));
    }
    if !m.remote_ip.is_empty() {
        ms.push(Matcher::remote_ip(&m.remote_ip)?);
    }
    if !m.header.is_empty() {
        ms.push(Matcher::headers(&m.header));
    }
    if !m.query.is_empty() {
        ms.push(Matcher::query(&m.query));
    }
    if !m.path_regexp.is_empty() {
        ms.push(Matcher::path_regexp(&m.path_regexp)?);
    }
    if !m.expr.is_empty() {
        ms.push(Matcher::expr(&m.expr)?);
    }
    Ok(ms)
}
