//! The module registry: what maps a config's module name (`respond`, `proxy`, …) to the module's
//! constructor, and the provisioning and cleanup of what it builds.
//!
//! A module has a namespaced ID, such as `http.handlers.respond`. A config names a module by the
//! last part of its ID; the namespace comes from where the name appears (a route's handler is
//! looked up in [`HANDLERS`], its middleware in [`MIDDLEWARE`]), as in fit.
//!
//! Modules are registered at compile time: a crate lists the modules it ships in a `static` slice
//! of [`Info`], and the binary builds one [`Registry`] from the slices it links. There is no global
//! mutable state: the registry is a value, passed to what compiles a config.
//!
//! A module's constructor decodes its params and provisions it in one step, and may fail. What it
//! builds may need cleaning up when the config it belongs to is released: the constructor says so
//! with [`Provisioned::on_cleanup`]. Every module loaded for one config is recorded in a
//! [`Modules`], which runs the cleanups in the reverse of the order the modules were provisioned,
//! exactly once, when it is released or dropped; a config that fails to compile drops its
//! `Modules`, cleaning up what was provisioned before the failure.

use std::{fmt, sync::Arc};

use akit_upstream::{EngineSlot, Pool};

use crate::traits::{Handler, Middleware};

/// The namespace of the modules that answer requests: [`Handler`]s.
pub const HANDLERS: &str = "http.handlers";

/// The namespace of the modules that wrap a route's handler: [`Middleware`].
pub const MIDDLEWARE: &str = "http.middleware";

/// What a module is, which its namespace says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Kind {
    /// A [`Handler`], in [`HANDLERS`].
    Handler,
    /// A [`Middleware`], in [`MIDDLEWARE`].
    Middleware,
}

impl Kind {
    /// The namespace modules of this kind are registered in.
    pub const fn namespace(self) -> &'static str {
        match self {
            Kind::Handler => HANDLERS,
            Kind::Middleware => MIDDLEWARE,
        }
    }

    /// The kind's name, as errors say it.
    pub const fn noun(self) -> &'static str {
        match self {
            Kind::Handler => "handler",
            Kind::Middleware => "middleware",
        }
    }
}

/// What the config a module belongs to provides it with, besides its own params, as fit's
/// `module.Env`.
///
/// So far (I14): the engine connections are dialed through, and the config's upstream pools, by
/// name. What compiles a config gives an `Env` with the engine ([`Env::new`]); [`compile`]
/// (crate::compile) adds the pools it makes from the config's `upstreams` before it provisions
/// the modules, which keep the pools they use ([`Env::upstream`]).
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Env {
    engine: EngineSlot,
    upstreams: Vec<Pool>,
    access_log: Option<akit_observe::AccessLog>,
    metrics: Option<akit_observe::metrics::Metrics>,
}

impl Env {
    /// The env of a config whose pools dial through the engine `engine` holds once it is filled.
    /// [`Env::default`] has an empty slot: its pools fail every request, as for `akit validate`.
    #[must_use]
    pub fn new(engine: EngineSlot) -> Env {
        Env {
            engine,
            upstreams: Vec::new(),
            access_log: None,
            metrics: None,
        }
    }

    /// The same, with the process's access log, which a graph compiled from a config with
    /// `access_log` writes its requests' lines to (I21). The log outlives the graphs: it is
    /// opened once, when akit starts, as fit's.
    #[must_use]
    pub fn with_access_log(mut self, log: Option<akit_observe::AccessLog>) -> Env {
        self.access_log = log;
        self
    }

    /// The same, with the process's metrics registry (I37): a graph compiled with it shares its
    /// upstreams' counters with the other graphs that have them, and, from a config with
    /// `metrics`, counts its requests in it. Made once, when akit starts, as fit's.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Option<akit_observe::metrics::Metrics>) -> Env {
        self.metrics = metrics;
        self
    }

    /// The process's metrics registry, if it has one.
    #[must_use]
    pub fn metrics(&self) -> Option<&akit_observe::metrics::Metrics> {
        self.metrics.as_ref()
    }

    /// The process's access log, if it has one.
    #[must_use]
    pub fn access_log(&self) -> Option<&akit_observe::AccessLog> {
        self.access_log.as_ref()
    }

    /// Where the engine connections are dialed through is, or will be.
    #[must_use]
    pub fn engine(&self) -> &EngineSlot {
        &self.engine
    }

    /// The pool of the upstream named `name`.
    #[must_use]
    pub fn upstream(&self, name: &str) -> Option<&Pool> {
        self.upstreams.iter().find(|p| p.name() == name)
    }

    /// The same engine, and `upstreams`.
    pub(crate) fn with_upstreams(&self, upstreams: Vec<Pool>) -> Env {
        Env {
            engine: self.engine.clone(),
            upstreams,
            access_log: self.access_log.clone(),
            metrics: self.metrics.clone(),
        }
    }
}

/// What a module's constructor is given: its params, as the config wrote them, and its config's
/// [`Env`]. As fit's `module.Context`.
///
/// `P` is the type of a module's params. akit-runtime does not know the config model: I04 sets
/// `P` to akit-config's params value, which modules decode strictly and whose errors carry the
/// path and position of what is wrong.
pub struct Provision<'a, P> {
    id: &'static str,
    params: &'a P,
    env: &'a Env,
}

impl<'a, P> Provision<'a, P> {
    /// The ID of the module being provisioned, as `http.handlers.respond`.
    pub fn id(&self) -> &'static str {
        self.id
    }

    /// The module's params, as the config wrote them.
    pub fn params(&self) -> &'a P {
        self.params
    }

    /// What the module's config provides it with.
    pub fn env(&self) -> &'a Env {
        self.env
    }
}

impl<P> fmt::Debug for Provision<'_, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Provision")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// What runs when the config a module belongs to is released.
type Cleanup = Box<dyn FnOnce() + Send + Sync>;

/// A provisioned module: what serves requests, and what to run when its config is released.
///
/// As fit's `Provisioner` and `Cleaner` together.
pub struct Provisioned<T: ?Sized> {
    value: Arc<T>,
    cleanup: Option<Cleanup>,
}

impl<T: ?Sized> Provisioned<T> {
    /// A module with nothing to clean up.
    pub fn new(value: Arc<T>) -> Provisioned<T> {
        Provisioned {
            value,
            cleanup: None,
        }
    }

    /// Runs `cleanup` once, when the config the module belongs to is released: after the last
    /// request and connection holding the config are done, or when the config fails to compile.
    ///
    /// It may run on an I/O thread (whoever drops the last hold on the config), so it never
    /// blocks: what can only block (closing a file on a slow disk, flushing a cache) is handed to
    /// akio's logical pool with `spawn_blocking`.
    #[must_use]
    pub fn on_cleanup(mut self, cleanup: impl FnOnce() + Send + Sync + 'static) -> Provisioned<T> {
        self.cleanup = Some(Box::new(cleanup));
        self
    }
}

impl<T: ?Sized> fmt::Debug for Provisioned<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Provisioned")
            .field("cleanup", &self.cleanup.is_some())
            .finish_non_exhaustive()
    }
}

/// A constructor: decodes a module's params and provisions it.
pub type New<P, T> = fn(&Provision<'_, P>) -> Result<Provisioned<T>, ProvisionError>;

/// A module's constructor, by kind.
#[non_exhaustive]
pub enum Constructor<P> {
    /// Builds a [`Handler`].
    Handler(New<P, dyn Handler>),
    /// Builds a [`Middleware`].
    Middleware(New<P, dyn Middleware>),
}

impl<P> Constructor<P> {
    /// The kind of module this builds.
    pub const fn kind(&self) -> Kind {
        match self {
            Constructor::Handler(_) => Kind::Handler,
            Constructor::Middleware(_) => Kind::Middleware,
        }
    }
}

impl<P> Clone for Constructor<P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P> Copy for Constructor<P> {}

impl<P> fmt::Debug for Constructor<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Constructor::{:?}", self.kind())
    }
}

/// Describes a module, as fit's `module.Info`.
pub struct Info<P> {
    /// The module's namespaced ID, as `http.handlers.respond`: lowercase words of letters, digits
    /// and `_`, each starting with a letter, joined by dots. Its namespace is its
    /// [kind](Constructor::kind)'s.
    pub id: &'static str,
    /// What the module does, in a sentence or two, for the config reference.
    pub doc: &'static str,
    /// Builds the module from its params.
    pub new: Constructor<P>,
}

impl<P> Info<P> {
    /// A handler module.
    pub const fn handler(id: &'static str, doc: &'static str, new: New<P, dyn Handler>) -> Self {
        Info {
            id,
            doc,
            new: Constructor::Handler(new),
        }
    }

    /// A middleware module.
    pub const fn middleware(
        id: &'static str,
        doc: &'static str,
        new: New<P, dyn Middleware>,
    ) -> Self {
        Info {
            id,
            doc,
            new: Constructor::Middleware(new),
        }
    }

    /// The namespace part of the ID, as `http.handlers`.
    pub fn namespace(&self) -> &'static str {
        self.id.rsplit_once('.').map_or("", |(ns, _)| ns)
    }

    /// The last part of the ID, by which a config names the module.
    pub fn name(&self) -> &'static str {
        self.id.rsplit_once('.').map_or(self.id, |(_, name)| name)
    }
}

impl<P> fmt::Debug for Info<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Info")
            .field("id", &self.id)
            .field("kind", &self.new.kind())
            .finish_non_exhaustive()
    }
}

/// Whether `id` is a well-formed module ID: at least two dot-separated words, each of lowercase
/// letters, digits and `_`, starting with a letter.
fn well_formed(id: &str) -> bool {
    let mut words = 0usize;
    for word in id.split('.') {
        let mut bytes = word.bytes();
        if !bytes.next().is_some_and(|b| b.is_ascii_lowercase()) {
            return false;
        }
        if !bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
            return false;
        }
        words = words.saturating_add(1);
    }
    words >= 2
}

/// A mistake in the modules given to [`Registry::new`]: in the modules' code, not in a config.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RegistryError {
    /// An ID that is not well formed.
    Malformed(&'static str),
    /// An ID registered twice.
    Duplicate(&'static str),
    /// An ID whose namespace is not its constructor's kind's.
    Namespace {
        /// The module's ID.
        id: &'static str,
        /// What its constructor builds.
        kind: Kind,
    },
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::Malformed(id) => write!(f, "module: malformed ID {id:?}"),
            RegistryError::Duplicate(id) => write!(f, "module: {id} is registered twice"),
            RegistryError::Namespace { id, kind } => write!(
                f,
                "module: {id} is a {}, which belongs in {}",
                kind.noun(),
                kind.namespace()
            ),
        }
    }
}

impl std::error::Error for RegistryError {}

/// A module's provisioning failed: its params are wrong beyond what decoding them finds, or what
/// it builds from them could not be built.
///
/// Its message says what is wrong, as fit's; whoever loaded the module puts it at the path of the
/// module's params (and of [`field`](Self::field) in them, when there is one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvisionError {
    field: Option<Box<str>>,
    message: Box<str>,
}

impl ProvisionError {
    /// An error about the module's params as a whole.
    pub fn new(message: impl fmt::Display) -> ProvisionError {
        ProvisionError {
            field: None,
            message: message.to_string().into(),
        }
    }

    /// An error about the param named `field`, as fit's `Context.Errorf(field, …)`.
    pub fn field(field: &str, message: impl fmt::Display) -> ProvisionError {
        ProvisionError {
            field: Some(field.into()),
            message: message.to_string().into(),
        }
    }

    /// The param the error is about, if it is about one.
    pub fn field_name(&self) -> Option<&str> {
        self.field.as_deref()
    }

    /// What is wrong.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ProvisionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProvisionError {}

/// Loading a module from a config failed.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LoadError {
    /// The config names a module its namespace does not have.
    Unknown {
        /// The namespace it was looked up in.
        namespace: &'static str,
        /// The name the config gave.
        name: Box<str>,
        /// The names the namespace has, sorted.
        known: Vec<&'static str>,
    },
    /// The module's constructor failed.
    Provision {
        /// The module's ID.
        id: &'static str,
        /// What went wrong.
        error: ProvisionError,
    },
}

impl fmt::Display for LoadError {
    /// As fit's: `there is no module "nope" in http.handlers (it has "proxy", "respond")`, and a
    /// provisioning error's own message.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Unknown {
                namespace,
                name,
                known,
            } => {
                write!(f, "there is no module {name:?} in {namespace}")?;
                for (i, k) in known.iter().enumerate() {
                    f.write_str(if i == 0 { " (it has " } else { ", " })?;
                    write!(f, "{k:?}")?;
                }
                if !known.is_empty() {
                    f.write_str(")")?;
                }
                Ok(())
            }
            LoadError::Provision { error, .. } => fmt::Display::fmt(error, f),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LoadError::Provision { error, .. } => Some(error),
            LoadError::Unknown { .. } => None,
        }
    }
}

/// One module loaded for a config.
struct Loaded {
    id: &'static str,
    cleanup: Option<Cleanup>,
}

/// The modules loaded for one config, in the order they were provisioned, and their cleanups.
///
/// Releasing it (or dropping it) cleans the modules up in the reverse of that order, so a module is
/// cleaned up before any it was built on, each exactly once. A config that fails to compile drops
/// its `Modules`, which cleans up what was provisioned before the failure; one that compiles keeps
/// it in its graph (I04) until the graph is released.
#[derive(Default)]
pub struct Modules {
    loaded: Vec<Loaded>,
}

impl Modules {
    /// No modules.
    pub fn new() -> Modules {
        Modules::default()
    }

    /// How many modules were loaded.
    pub fn len(&self) -> usize {
        self.loaded.len()
    }

    /// Whether none were.
    pub fn is_empty(&self) -> bool {
        self.loaded.is_empty()
    }

    /// The IDs of the modules loaded, in the order they were provisioned.
    pub fn ids(&self) -> impl ExactSizeIterator<Item = &'static str> + '_ {
        self.loaded.iter().map(|l| l.id)
    }

    /// Cleans the modules up, last provisioned first.
    pub fn release(self) {
        drop(self);
    }

    /// Records what is not a module but is cleaned up with them, as the config's upstream pools
    /// are: `cleanup` runs, in the same reverse order, under `id`.
    pub(crate) fn push_cleanup(
        &mut self,
        id: &'static str,
        cleanup: impl FnOnce() + Send + Sync + 'static,
    ) {
        self.loaded.push(Loaded {
            id,
            cleanup: Some(Box::new(cleanup)),
        });
    }

    fn push<T: ?Sized>(&mut self, id: &'static str, p: Provisioned<T>) -> Arc<T> {
        self.loaded.push(Loaded {
            id,
            cleanup: p.cleanup,
        });
        p.value
    }
}

impl Drop for Modules {
    fn drop(&mut self) {
        while let Some(l) = self.loaded.pop() {
            if let Some(cleanup) = l.cleanup {
                cleanup();
            }
        }
    }
}

impl fmt::Debug for Modules {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.ids()).finish()
    }
}

/// The modules a binary links, by ID: built once from the `static` lists of the crates that ship
/// them, then only read.
pub struct Registry<P: 'static> {
    /// Sorted by ID.
    modules: Vec<&'static Info<P>>,
}

impl<P: 'static> Registry<P> {
    /// A registry of the modules in `sets`.
    ///
    /// Fails on a malformed ID, an ID given twice, or an ID outside its kind's namespace: mistakes
    /// in the modules' code, which a test of the binary's registry catches.
    pub fn new(sets: &[&'static [Info<P>]]) -> Result<Registry<P>, RegistryError> {
        let mut modules: Vec<&'static Info<P>> = Vec::new();
        for info in sets.iter().flat_map(|set| set.iter()) {
            if !well_formed(info.id) {
                return Err(RegistryError::Malformed(info.id));
            }
            let kind = info.new.kind();
            if info.namespace() != kind.namespace() {
                return Err(RegistryError::Namespace { id: info.id, kind });
            }
            modules.push(info);
        }
        modules.sort_unstable_by_key(|i| i.id);
        if let Some(w) = modules
            .windows(2)
            .find(|w| w.first().map(|i| i.id) == w.get(1).map(|i| i.id))
        {
            return Err(RegistryError::Duplicate(w.first().map_or("", |i| i.id)));
        }
        Ok(Registry { modules })
    }

    /// The module registered with `id`.
    pub fn lookup(&self, id: &str) -> Option<&'static Info<P>> {
        self.modules
            .binary_search_by(|i| i.id.cmp(id))
            .ok()
            .and_then(|at| self.modules.get(at).copied())
    }

    /// The modules registered directly in namespace `ns`, sorted by ID.
    pub fn namespace<'a>(&'a self, ns: &'a str) -> impl Iterator<Item = &'static Info<P>> + 'a {
        self.modules
            .iter()
            .copied()
            .filter(move |i| i.namespace() == ns)
    }

    /// Every module, sorted by ID.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &'static Info<P>> + '_ {
        self.modules.iter().copied()
    }

    /// Loads the handler a config names `name`, provisioned from `params` with `env`, and records
    /// it in `modules`.
    pub fn handler(
        &self,
        name: &str,
        params: &P,
        env: &Env,
        modules: &mut Modules,
    ) -> Result<Arc<dyn Handler>, LoadError> {
        let info = self.find(Kind::Handler, name)?;
        match info.new {
            Constructor::Handler(new) => provision(info, new, params, env, modules),
            Constructor::Middleware(_) => Err(self.unknown(Kind::Handler, name)),
        }
    }

    /// Loads the middleware a config names `name`, provisioned from `params` with `env`, and
    /// records it in `modules`.
    pub fn middleware(
        &self,
        name: &str,
        params: &P,
        env: &Env,
        modules: &mut Modules,
    ) -> Result<Arc<dyn Middleware>, LoadError> {
        let info = self.find(Kind::Middleware, name)?;
        match info.new {
            Constructor::Middleware(new) => provision(info, new, params, env, modules),
            Constructor::Handler(_) => Err(self.unknown(Kind::Middleware, name)),
        }
    }

    fn find(&self, kind: Kind, name: &str) -> Result<&'static Info<P>, LoadError> {
        let ns = kind.namespace();
        self.modules
            .iter()
            .copied()
            .find(|i| i.namespace() == ns && i.name() == name)
            .ok_or_else(|| self.unknown(kind, name))
    }

    fn unknown(&self, kind: Kind, name: &str) -> LoadError {
        let namespace = kind.namespace();
        LoadError::Unknown {
            namespace,
            name: name.into(),
            known: self.namespace(namespace).map(|i| i.name()).collect(),
        }
    }
}

impl<P> fmt::Debug for Registry<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.modules.iter().map(|i| i.id))
            .finish()
    }
}

fn provision<P, T: ?Sized>(
    info: &'static Info<P>,
    new: New<P, T>,
    params: &P,
    env: &Env,
    modules: &mut Modules,
) -> Result<Arc<T>, LoadError> {
    let p = new(&Provision {
        id: info.id,
        params,
        env,
    })
    .map_err(|error| LoadError::Provision { id: info.id, error })?;
    Ok(modules.push(info.id, p))
}
