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

/// A config, compiled: immutable, shared by every I/O thread, its modules provisioned.
pub struct Graph {
    listeners: Vec<Listener>,
    routes: Vec<CompiledRoute>,
    /// Per listener, in the order of `listeners`: its routes' router, to indices of `routes`.
    routers: Vec<akit_router::Router<usize>>,
    /// Per listener, in the order of `listeners`: its TLS, loaded from its files, when it has
    /// `tls` (its certificates, served per handshake through [`crate::ListenerCerts`]).
    tls: Vec<Option<akit_tls::ListenerTls>>,
    /// The SHA-256 of every certificate, key and authority file its listeners loaded, by path:
    /// what `run -watch-certs` compares the files with (I30).
    cert_files: std::collections::BTreeMap<String, akit_tls::Sum>,
    /// The upstreams' pools, in the config's order.
    upstreams: Vec<Pool>,
    /// The access log its requests' lines go to: the env's, when the config has `access_log`.
    access_log: Option<akit_observe::AccessLog>,
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
        let i = self.listener_index(name)?;
        Some(self.tls.get(i)?.as_ref()?.certs())
    }

    /// The SHA-256 of every certificate, key and authority file the listeners loaded, by path.
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

    /// The access log the requests this graph serves are written to, if any (I21).
    #[must_use]
    pub fn access_log(&self) -> Option<&akit_observe::AccessLog> {
        self.access_log.as_ref()
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
        let i = *self.routers.get(listener)?.find_request(r)?;
        self.routes.get(i)
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
    let upstreams: Vec<Pool> = config
        .upstreams
        .iter()
        .map(|u| Pool::new(u, env.engine().clone()))
        .collect();
    if !upstreams.is_empty() {
        // Recorded first, so cleaned up last.
        let pools = upstreams.clone();
        modules.push_cleanup(UPSTREAM_POOLS, move || pools.iter().for_each(Pool::close));
    }
    let env = &env.with_upstreams(upstreams.clone());
    let mut routes = Vec::with_capacity(config.routes.len());
    let mut errors = Vec::new();
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
            akit_tls::ListenerTls::load(t, &l.protocols)
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
    let cert_files = tls
        .iter()
        .flatten()
        .flat_map(|t| t.files().iter().map(|(p, s)| (p.clone(), *s)))
        .collect();
    if !errors.is_empty() {
        // `modules` drops here, cleaning up what was provisioned.
        return Err(Errors(errors));
    }
    // Built here, not per request (rule 12). A route with no listeners serves on every one.
    let routers = config
        .listeners
        .iter()
        .map(|l| {
            akit_router::Router::new(
                routes
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.listeners.is_empty() || r.listeners.contains(&l.name))
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
    Ok(Graph {
        listeners: config.listeners.clone(),
        routes,
        routers,
        tls,
        cert_files,
        upstreams,
        access_log: config.access_log.as_ref().and(env.access_log().cloned()),
        modules: Mutex::new(Some(modules)),
        holds: AtomicUsize::new(0),
        orphaned: AtomicBool::new(false),
    })
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
