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

use crate::module::{Env, Modules, Registry};
use crate::traits::Handler;

/// The params a handler module is provisioned from: the handler as akit-config read it, its
/// module named by [`akit_config::Handler::module`]. akit-config decodes and checks each module's
/// params, with their paths and positions, when the config is read (I02); modules take the typed
/// value.
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
    /// The handler, its middleware chain built around it (I40 on); provisioned.
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
            .finish_non_exhaustive()
    }
}

/// A config, compiled: immutable, shared by every I/O thread, its modules provisioned.
pub struct Graph {
    listeners: Vec<Listener>,
    routes: Vec<CompiledRoute>,
    /// Per listener, in the order of `listeners`: its routes' router, to indices of `routes`.
    routers: Vec<akit_router::Router<usize>>,
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

    /// The routes, in the config's order.
    #[must_use]
    pub fn routes(&self) -> &[CompiledRoute] {
        &self.routes
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
            .field("modules", &self.module_ids())
            .field("holds", &self.holds.load(Ordering::Relaxed))
            .finish()
    }
}

/// Compiles `config` into a graph: every route's handler loaded from `registry` and provisioned
/// with `env`. Nothing is published: a config that fails leaves nothing behind (the modules
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
    let mut routes = Vec::with_capacity(config.routes.len());
    let mut errors = Vec::new();
    for (i, r) in config.routes.iter().enumerate() {
        let handler = match registry.handler(r.handler.module(), &r.handler, env, &mut modules) {
            Ok(h) => h,
            Err(e) => {
                let mut path = format!("routes[{i}].handler");
                if let crate::LoadError::Provision { error, .. } = &e {
                    if let Some(field) = error.field_name() {
                        path = format!("{path}.params.{field}");
                    }
                }
                errors.push(Error {
                    file: None,
                    path,
                    line: 0,
                    col: 0,
                    msg: e.to_string(),
                });
                continue;
            }
        };
        let (host, path) = r
            .matcher
            .as_ref()
            .map(|m| (m.host.clone(), m.path.clone()))
            .unwrap_or_default();
        routes.push(CompiledRoute {
            name: r.name.clone(),
            listeners: r.listeners.clone(),
            priority: r.priority,
            host,
            path,
            handler,
        });
    }
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
        modules: Mutex::new(Some(modules)),
        holds: AtomicUsize::new(0),
        orphaned: AtomicBool::new(false),
    })
}
