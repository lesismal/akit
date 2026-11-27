//! The host of the roles (I100, fit's `app`): it starts each role a config sets, moves the running
//! ones to every config applied, and stops those a config leaves out, on the control thread, after
//! the listeners. A role's failure is a warning: the rest of the config runs.
//!
//! A role adds its bindings to the listeners' akio engine ([`Host::engine`]) and dials on it; it
//! changes its own member of the config through [`Host::edit`], which the control thread applies as
//! any config. The roles akit runs are the [`Info`]s the binary registers: a member set whose role
//! is not one of them is refused when the config is compiled ([`unknown`]).

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use akit_config::Config;
use akit_config::value::Value;

use crate::linktoken::{Registry, View};
use crate::pages::{Consoles, Modules};

/// A running role.
pub trait Role: Send {
    /// Moves the role to `config`, the config being applied, which sets its member. The first
    /// call starts it. Configs are applied one at a time, on the control thread; it may bind
    /// addresses and start dials, and returns once they are under way. An error leaves the role
    /// as it was, and is reported with the config's warnings. It must not wait for an edit of its
    /// own, which waits for the apply.
    ///
    /// # Errors
    ///
    /// Why the role could not move to `config`.
    fn apply(&mut self, config: &Config) -> Result<(), String>;

    /// Ends the role: its bindings are removed and its connections closed, waiting for what is
    /// under way `grace` at most.
    fn stop(&mut self, grace: Duration);

    /// The registry of the tokens the role's listener issued, for a role that issues them.
    fn tokens(&self) -> Option<Registry> {
        None
    }

    /// What the role's pages call (a hub's status, its calls on peers and tunnels): a handle of
    /// the role's own type, which the caller downcasts (`akit_tunnel::hub::HubApi`). Its methods
    /// return at once. `None` for a role without one.
    fn api(&self) -> Option<Api> {
        None
    }

    /// The files the role read when a config was last applied to it, by path, with the SHA-256
    /// of what it read (zeros for a file it could not read): its listeners' certificates and
    /// keys. `akit run -watch-certs` applies the running config again once one of them changes,
    /// as for the listeners' own (fit's `Info.CertFiles`). None by default.
    fn files(&self) -> Vec<(String, [u8; 32])> {
        Vec::new()
    }

    /// The role's module of the pages (I109, fit's `Paged`): its API, which its own `console`
    /// and `admin.console` serve. `None` for a role without pages.
    fn module(&self) -> Option<crate::pages::ModuleRef> {
        None
    }

    /// What the role counts, for `/metrics` (fit's `app.Metered`): a writer the Admin API's
    /// thread calls on every scrape, appending the role's families in Prometheus's text format.
    /// It must not block. `None` for a role without metrics.
    fn metrics(&self) -> Option<MetricsFn> {
        None
    }
}

/// A role's metrics ([`Role::metrics`]): appends its families to the scrape, in Prometheus's
/// text format.
pub type MetricsFn = Arc<dyn Fn(&mut Vec<u8>) + Send + Sync>;

/// The running roles' metrics, shared with the Admin API's `/metrics`.
pub type Meters = Arc<Mutex<Vec<MetricsFn>>>;

/// Appends the metrics of every running role to `out`.
pub fn write_meters(meters: &Meters, out: &mut Vec<u8>) {
    let all = meters
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    for m in all {
        m(out);
    }
}

/// A role's handle for its pages ([`Role::api`]).
pub type Api = Arc<dyn std::any::Any + Send + Sync>;

/// A role akit can run.
#[derive(Clone, Copy)]
pub struct Info {
    /// The path of the role's member, its parts joined with dots: `tunnel.hub`.
    pub id: &'static str,
    /// Whether a config sets the member.
    pub set: fn(&Config) -> bool,
    /// The role, not started yet: its first apply starts it.
    pub new: fn(Host) -> Result<Box<dyn Role>, String>,
}

impl fmt::Debug for Info {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Info").field("id", &self.id).finish()
    }
}

/// Every role fit has, with whether `c` sets its member: the members a config may set.
#[must_use]
pub fn members(c: &Config) -> Vec<(&'static str, bool)> {
    let t = c.tunnel.as_ref();
    let l = c.llm.as_ref();
    let w = c.webrtc.as_ref();
    let b = c.bittorrent.as_ref();
    vec![
        ("bittorrent.client", b.is_some_and(|b| b.client.is_some())),
        ("bittorrent.tracker", b.is_some_and(|b| b.tracker.is_some())),
        ("llm.hub", l.is_some_and(|l| l.hub.is_some())),
        ("llm.node", l.is_some_and(|l| l.node.is_some())),
        ("llm.proxy", l.is_some_and(|l| l.proxy.is_some())),
        ("tunnel.hub", t.is_some_and(|t| t.hub.is_some())),
        ("tunnel.node", t.is_some_and(|t| t.node.is_some())),
        ("tunnel.proxy", t.is_some_and(|t| t.proxy.is_some())),
        ("webrtc.media", w.is_some_and(|w| w.media.is_some())),
        ("webrtc.proxy", w.is_some_and(|w| w.proxy.is_some())),
        ("webrtc.turn", w.is_some_and(|w| w.turn.is_some())),
    ]
}

/// The members `c` sets whose roles are not among `infos`: what akit does not run yet, which a
/// config must not say it does.
#[must_use]
pub fn unknown(c: &Config, infos: &[Info]) -> Vec<&'static str> {
    members(c)
        .into_iter()
        .filter(|(id, set)| *set && !infos.iter().any(|i| i.id == *id))
        .map(|(id, _)| id)
        .collect()
}

/// A change of a role's member: given the member as it is now, as a JSON value (`Null` when the
/// config leaves it out), it changes it in place, or says why not.
pub type Change = Box<dyn FnOnce(&mut Value) -> Result<(), String> + Send>;

/// What a change's caller is told: the warnings applying it gave, or why it was not applied.
pub type Done = Box<dyn FnOnce(Result<Vec<String>, String>) + Send>;

/// An edit of a role's member, for the control thread to apply.
pub struct Edit {
    /// The role's ID, the path of its member: `tunnel.hub`.
    pub id: &'static str,
    /// The change.
    pub change: Change,
    /// What its caller is told.
    pub done: Done,
}

impl fmt::Debug for Edit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Edit")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Edit {
    /// Makes the change to `root`, a whole config, at the member's path, creating the member's
    /// object when the change gives the member a value and the config left it out.
    ///
    /// # Errors
    ///
    /// The change's error; a config whose top is not an object.
    pub fn apply_to(self, root: &mut Value) -> Result<Done, (Done, String)> {
        let Edit { id, change, done } = self;
        let Some((a, b)) = id.split_once('.') else {
            return Err((done, format!("{id} is not a member's path")));
        };
        let Value::Object(top) = root else {
            return Err((done, "the config is not an object".to_owned()));
        };
        if !top.iter().any(|(k, _)| k == a) {
            top.push((a.to_owned(), Value::Object(Vec::new())));
        }
        let Some((_, Value::Object(role))) = top.iter_mut().find(|(k, _)| k == a) else {
            return Err((done, format!("{a} is not an object")));
        };
        let mut member = role
            .iter()
            .find(|(k, _)| k == b)
            .map_or(Value::Null, |(_, v)| v.clone());
        if let Err(e) = change(&mut member) {
            return Err((done, e));
        }
        role.retain(|(k, _)| k != b);
        if member != Value::Null {
            role.push((b.to_owned(), member));
        }
        Ok(done)
    }
}

/// Hands an edit to the control thread; gives it back when the control thread cannot take it now
/// (too many wait). Called from any thread: it must not block.
pub type Editor = Arc<dyn Fn(Edit) -> Result<(), Edit> + Send + Sync>;

/// What a role is given: the engine it shares, and the way to change its own member.
#[derive(Clone)]
pub struct Host {
    /// The role's ID: `tunnel.hub`.
    pub id: &'static str,
    /// The listeners' engine, which the role adds its bindings to and dials on.
    pub engine: Arc<akio::Engine>,
    /// How long a role may let what a server it replaced holds run out: `run`'s
    /// `-shutdown-timeout`.
    pub drain: Duration,
    /// The prefix of the files the role keeps beside the config (`/etc/akit/akit` for
    /// `/etc/akit/akit.json`), or `None` when nothing is kept there.
    pub kept: Option<String>,
    editor: Option<Editor>,
}

impl fmt::Debug for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Host")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Host {
    /// The file the role keeps what it made of `name` in, such as a self-signed certificate whose
    /// fingerprint its peers pin, beside the config: `<prefix>.<id>.<name>`.
    #[must_use]
    pub fn kept_file(&self, name: &str) -> Option<String> {
        self.kept
            .as_ref()
            .map(|p| format!("{p}.{}.{name}", self.id))
    }

    /// Changes the role's member of the running config with `change`, applied by the control
    /// thread as a config from the role's pages; `done` is told how it went, there. It returns at
    /// once.
    pub fn edit(&self, change: Change, done: Done) {
        let edit = Edit {
            id: self.id,
            change,
            done,
        };
        match &self.editor {
            None => (edit.done)(Err("the config cannot be changed here".to_owned())),
            Some(e) => {
                if let Err(edit) = e(edit) {
                    (edit.done)(Err(crate::pages::BUSY.to_owned()));
                }
            }
        }
    }
}

/// The prefix of the names of the files kept beside the config at `path` (decision 038's
/// `akit.tunnel.hub.pem`): the config file's path without its extension, as `/etc/akit/akit` for
/// `/etc/akit/akit.json`, or `akit` in a directory of config files; `None` for `""` or `-`,
/// standard input, beside which nothing is kept. Reads the file system: the control thread's.
#[must_use]
pub fn beside_prefix(path: &str) -> Option<String> {
    if path.is_empty() || path == "-" {
        return None;
    }
    let p = std::path::absolute(path).unwrap_or_else(|_| path.into());
    if p.is_dir() {
        return Some(p.join("akit").to_string_lossy().into_owned());
    }
    Some(p.with_extension("").to_string_lossy().into_owned())
}

/// The roles' token registries, shared with the Admin API, which reads them on its own thread.
pub type Registries = Arc<Mutex<HashMap<&'static str, Registry>>>;

/// Runs the roles a config sets.
pub struct Manager {
    infos: Vec<Info>,
    engine: Arc<akio::Engine>,
    editor: Option<Editor>,
    drain: Duration,
    kept: Option<String>,
    running: BTreeMap<&'static str, Box<dyn Role>>,
    registries: Registries,
    modules: Modules,
    consoles: Option<Box<dyn Consoles>>,
    meters: Meters,
}

impl fmt::Debug for Manager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Manager")
            .field("running", &self.running.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl Manager {
    /// A manager of `infos`' roles, on `engine`, changing the config through `editor`. It runs no
    /// role until [`Manager::apply`].
    #[must_use]
    pub fn new(infos: &[Info], engine: Arc<akio::Engine>, editor: Option<Editor>) -> Manager {
        Manager {
            infos: infos.to_vec(),
            engine,
            editor,
            drain: Duration::from_secs(30),
            kept: None,
            running: BTreeMap::new(),
            registries: Arc::new(Mutex::new(HashMap::new())),
            modules: Modules::default(),
            consoles: None,
            meters: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Has `consoles` serve the roles' own consoles (I109): each moved with its role.
    pub fn set_consoles(&mut self, consoles: Box<dyn Consoles>) {
        self.consoles = Some(consoles);
    }

    /// The running roles' modules of the pages (I109), which `admin.console` serves; kept up to
    /// date by every apply.
    #[must_use]
    pub fn modules(&self) -> Modules {
        self.modules.clone()
    }

    /// Moves role `id`'s console to `c`; its error, as a role's.
    fn console(&mut self, id: &'static str, c: Option<&Config>, errs: &mut Vec<String>) {
        let module = c
            .and_then(|_| self.running.get(id))
            .and_then(|r| r.module());
        if let Some(cs) = self.consoles.as_mut() {
            if let Err(e) = cs.apply(id, c, module) {
                errs.push(format!("{id}: console: {e}"));
            }
        }
    }

    /// Looks at the roles' consoles' certificate files once (`-watch-certs`, every interval, on
    /// the control thread; [`Consoles::watch_certs`]).
    pub fn watch_console_certs(&mut self, now: std::time::Instant) {
        if let Some(cs) = self.consoles.as_mut() {
            cs.watch_certs(now);
        }
    }

    /// Where role `id`'s console listens, when it runs ([`Consoles::addr`]).
    #[must_use]
    pub fn console_addr(&self, id: &str) -> Option<std::net::SocketAddr> {
        self.consoles.as_ref().and_then(|cs| cs.addr(id))
    }

    /// Reads the roles' consoles' certificate files now (SIGHUP; [`Consoles::reload_certs`]).
    pub fn reload_console_certs(&mut self) {
        if let Some(cs) = self.consoles.as_mut() {
            cs.reload_certs();
        }
    }

    /// Sets how long a role may let what a server it replaced holds run out.
    pub fn set_drain(&mut self, d: Duration) {
        self.drain = d;
    }

    /// Has the roles keep what they make beside the config at `path` ([`beside_prefix`]).
    pub fn keep_beside(&mut self, path: &str) {
        self.kept = beside_prefix(path);
    }

    fn host(&self, id: &'static str) -> Host {
        Host {
            id,
            engine: Arc::clone(&self.engine),
            drain: self.drain,
            kept: self.kept.clone(),
            editor: self.editor.clone(),
        }
    }

    /// Moves the roles to `c`: it starts each role `c` sets that is not running, applies `c` to
    /// each running one, and stops each `c` leaves out. Each role's error is returned; the roles
    /// without one run `c`.
    pub fn apply(&mut self, c: &Config) -> Vec<String> {
        let mut errs = Vec::new();
        for info in self.infos.clone() {
            let set = (info.set)(c);
            if !set {
                if let Some(mut r) = self.running.remove(info.id) {
                    self.console(info.id, None, &mut errs);
                    r.stop(Duration::ZERO);
                }
                continue;
            }
            match self.running.get_mut(info.id) {
                Some(r) => {
                    if let Err(e) = r.apply(c) {
                        errs.push(format!("{}: {e}; it runs as it did", info.id));
                    }
                    self.console(info.id, Some(c), &mut errs);
                }
                None => {
                    let mut r = match (info.new)(self.host(info.id)) {
                        Ok(r) => r,
                        Err(e) => {
                            errs.push(format!("{}: {e}", info.id));
                            continue;
                        }
                    };
                    if let Err(e) = r.apply(c) {
                        r.stop(Duration::ZERO);
                        errs.push(format!("{}: {e}", info.id));
                        continue;
                    }
                    self.running.insert(info.id, r);
                    self.console(info.id, Some(c), &mut errs);
                }
            }
        }
        self.note_registries();
        errs
    }

    fn note_registries(&self) {
        let mut g = self
            .registries
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        g.clear();
        for (id, r) in &self.running {
            if let Some(reg) = r.tokens() {
                g.insert(id, reg);
            }
        }
        drop(g);
        self.modules
            .set(self.running.values().filter_map(|r| r.module()).collect());
        let mut m = self.meters.lock().unwrap_or_else(PoisonError::into_inner);
        m.clear();
        m.extend(self.running.values().filter_map(|r| r.metrics()));
    }

    /// The IDs of the roles running, sorted.
    #[must_use]
    pub fn running(&self) -> Vec<&'static str> {
        self.running.keys().copied().collect()
    }

    /// The handle of the running role `id` for its pages ([`Role::api`]).
    #[must_use]
    pub fn api(&self, id: &str) -> Option<Api> {
        self.running.get(id).and_then(|r| r.api())
    }

    /// The files the running roles read when the config was last applied ([`Role::files`]), with
    /// their SHA-256, for `-watch-certs` to apply the config again when one changes.
    #[must_use]
    pub fn files(&self) -> Vec<(String, [u8; 32])> {
        self.running.values().flat_map(|r| r.files()).collect()
    }

    /// The running roles' metrics, for the Admin API's `/metrics`.
    #[must_use]
    pub fn meters(&self) -> Meters {
        Arc::clone(&self.meters)
    }

    /// The running roles' token registries, for the Admin API.
    #[must_use]
    pub fn registries(&self) -> Registries {
        Arc::clone(&self.registries)
    }

    /// Stops every role, waiting for each `grace` at most. Stop them before the engine.
    pub fn stop(&mut self, grace: Duration) {
        let mut errs = Vec::new();
        for id in self.running() {
            self.console(id, None, &mut errs);
        }
        for (_, mut r) in std::mem::take(&mut self.running) {
            r.stop(grace);
        }
        self.note_registries();
    }
}

/// The views of the tokens of the running role `id`, from `registries`.
#[must_use]
pub fn token_views(registries: &Registries, id: &str) -> Option<Vec<View>> {
    let g = registries.lock().unwrap_or_else(PoisonError::into_inner);
    g.get(id).map(Registry::list)
}

#[cfg(test)]
mod tests;
