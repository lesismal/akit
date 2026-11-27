//! The configs applied, numbered, kept as written: the history the Admin API reads, and the one
//! step every config takes to the running server, from the files (SIGHUP), the API or a provider.
//!
//! A config's *base* is what the files and the API write; what providers provide (I57) is merged
//! into it, by fit's rules (`akit_provider::merge`), each time a config is applied: the base
//! config is kept as written, with its ETag, and the merged one, the *effective* config, is what
//! runs, with where each part of it came from.

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use akio_crypto::hash::Sha256;
use akit_config::Errors;
use akit_provider::{Merged, Part, Shadowed, Source};

use crate::bootstrap::{SOURCE_STORE, admin_in_store, has_admin, with_admin};
use crate::store::{Sealer, User};

/// A config read from the files (at start and on SIGHUP).
pub const SOURCE_FILE: &str = "file";
/// A config loaded through the Admin API.
pub const SOURCE_ADMIN: &str = "admin";
/// A config applied again by `POST /rollback/{n}` (I27), as fit's `admin.SourceRollback`.
pub const SOURCE_ROLLBACK: &str = "rollback";
/// The running config applied again because the certificate files it loaded changed
/// (`run -watch-certs`, I30), as fit's `admin.SourceCertificates`.
pub const SOURCE_CERTIFICATES: &str = "certificates";

/// The names of the base config's sources, which no provider may take.
const BASE_SOURCES: [&str; 5] = [
    SOURCE_FILE,
    SOURCE_ADMIN,
    SOURCE_ROLLBACK,
    SOURCE_CERTIFICATES,
    SOURCE_STORE,
];

/// What writes a base config, standard JSON, to the store (I93b): the store's write, sealed,
/// synced and renamed. Called on the control thread, under the lock that orders changes.
pub type SaveConfig = Box<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// What writes the console users whose passwords changed beside the config in the store
/// (I93b). Called on the console's login pool, never an I/O thread.
pub type SaveUsers = Box<dyn Fn(&[User]) -> Result<(), String> + Send + Sync>;

/// The store the base configs applied are kept in (I93b, fit's `UseStore`).
struct Kept {
    /// The bootstrap file's `admin`, as JSON, put into every config as it loads; `None` when it
    /// has none.
    admin: Option<Arc<str>>,
    save: SaveConfig,
    /// The SHA-256 of what was written last.
    saved: [u8; 32],
}

impl fmt::Debug for Kept {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Kept")
            .field("admin", &self.admin.is_some())
            .finish_non_exhaustive()
    }
}

/// The console users whose passwords changed since akit started, and what keeps them (I93b).
#[derive(Default)]
struct Users {
    list: Vec<User>,
    save: Option<SaveUsers>,
}

/// A config that was applied.
#[derive(Debug)]
pub struct Version {
    /// Its number: 1 for the config akit started with, one more for each config applied since.
    pub number: u64,
    /// When it was applied.
    pub applied: SystemTime,
    /// Where it came from: [`SOURCE_FILE`], [`SOURCE_ADMIN`], [`SOURCE_ROLLBACK`],
    /// [`SOURCE_CERTIFICATES`], or the name of the provider whose part changed.
    pub source: Arc<str>,
    /// A strong entity tag of its base config as written ([`etag_of`]), which `If-Match`
    /// compares.
    pub etag: String,
    data: Arc<str>,
    /// Where `data` came from: the source of the last change to the base config, or of the
    /// version a rollback took it from.
    base_source: Arc<str>,
    effective: Arc<str>,
    sources: Vec<Source>,
    shadowed: Vec<Shadowed>,
}

impl Version {
    /// The base config as it was written, without what providers provide: standard JSON
    /// (comments and trailing commas blanked out).
    #[must_use]
    pub fn data(&self) -> &str {
        &self.data
    }

    /// Where the base config came from: [`SOURCE_FILE`] or [`SOURCE_ADMIN`], or, after a
    /// rollback, the source of the version rolled back to.
    #[must_use]
    pub fn base_source(&self) -> &str {
        &self.base_source
    }

    /// The config that ran: the base config with what providers provided merged into it, the
    /// base config itself when they provided nothing.
    #[must_use]
    pub fn effective(&self) -> &str {
        &self.effective
    }

    /// Where each member of the effective config came from, element by element in its arrays.
    #[must_use]
    pub fn sources(&self) -> &[Source] {
        &self.sources
    }

    /// What providers provided and the effective config leaves out, because a source before
    /// them took its name.
    #[must_use]
    pub fn shadowed(&self) -> &[Shadowed] {
        &self.shadowed
    }
}

/// The running config as it runs, and where its parts came from: `GET /config/effective`.
#[derive(Debug, Clone)]
pub struct Effective {
    /// The running version.
    pub version: Arc<Version>,
    /// The sources, the base config's first, from the one that wins to the one that loses.
    pub precedence: Vec<Arc<str>>,
}

/// A listener a config opened or closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenerChange {
    /// Its name.
    pub name: String,
    /// Where it listens.
    pub address: String,
    /// Its protocols.
    pub protocols: Vec<String>,
}

/// What applying a config changed, as the caller's apply step reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// The listeners opened, the changed ones included.
    pub added: Vec<ListenerChange>,
    /// The listeners closed, the changed ones included.
    pub removed: Vec<ListenerChange>,
    /// Changes in the config that take effect only when akit restarts.
    pub warnings: Vec<String>,
}

/// Why the apply step refused a config; the running config stays.
#[derive(Debug)]
pub enum ApplyError {
    /// The config has errors (it does not load or does not compile): the API answers 400.
    Config(Errors),
    /// It could not be served (a listener's address is in use, ...): the API answers 500.
    Failed(String),
}

/// Whether a config is applied or only checked: a dry run (`?dry_run=1`) takes the path an apply
/// takes, and changes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Applied: served, and kept as a new version.
    Apply,
    /// Checked as an apply would check it, and not applied.
    DryRun,
}

/// Why [`Configs::update`] or [`Configs::change`] applied nothing.
#[derive(Debug)]
pub enum Refused {
    /// `If-Match` names a config that is no longer the running one: the API answers 412.
    Changed,
    /// The change cannot be made (a pointer to nothing, a version no longer kept, ...): the API
    /// answers with the status and message.
    Request {
        /// The status: 400 or 404.
        status: u16,
        /// What is wrong.
        msg: String,
    },
    /// The apply step refused it.
    Apply(ApplyError),
}

/// A config applied, or checked by a dry run.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// Its version; for a dry run, the running config's.
    pub version: u64,
    /// Its ETag; for a dry run, the running config's.
    pub etag: String,
    /// Where it came from.
    pub source: Arc<str>,
    /// What it changed; for a dry run, what applying it would change.
    pub applied: Applied,
    /// Whether it was only checked ([`Mode::DryRun`]): nothing changed.
    pub dry_run: bool,
    /// For a dry run, the ETag the config checked would have once applied; empty otherwise.
    pub new_etag: String,
    /// For a dry run, whether the config checked is the running one, byte for byte; for a
    /// provider's part ([`Configs::provide`]), that it is the provider's last, not applied
    /// again.
    pub unchanged: bool,
}

/// The providers, in their order of precedence, with what each provided last that was applied;
/// and the store the base configs are kept in, when there is one.
#[derive(Debug, Default)]
struct Providers {
    parts: Vec<(Arc<str>, Option<Arc<Part>>)>,
    kept: Option<Kept>,
}

impl Providers {
    /// The bootstrap file's `admin`, when a store keeps the config and the file has one.
    fn admin(&self) -> Option<&str> {
        self.kept.as_ref().and_then(|k| k.admin.as_deref())
    }
}

impl Providers {
    /// The parts applied, in order of precedence; with `with`, a provider's new part in place of
    /// its last.
    fn parts(&self, with: Option<(usize, &Arc<Part>)>) -> Vec<Arc<Part>> {
        self.parts
            .iter()
            .enumerate()
            .filter_map(|(i, (_, p))| match with {
                Some((j, new)) if i == j => Some(Arc::clone(new)),
                _ => p.clone(),
            })
            .collect()
    }
}

#[derive(Debug)]
struct Inner {
    keep: usize,
    /// Oldest first; the last is the running config. Held for a push or a clone only: the API's
    /// I/O thread reads it.
    history: Mutex<VecDeque<Arc<Version>>>,
    /// Held for a whole change, so that configs are applied one after another and `If-Match`
    /// is compared with the config each one replaces; with the providers and their parts. Never
    /// taken on an I/O thread.
    applying: Mutex<Providers>,
    /// The providers' names, in their order of precedence, for `GET /config/effective` on the
    /// API's I/O thread: held for a clone only.
    names: Mutex<Vec<Arc<str>>>,
    /// The console users whose passwords changed (I93b): held while they are written, on the
    /// console's pool. `stored` says whether a store keeps them, for the I/O thread.
    users: Mutex<Users>,
    stored: AtomicBool,
    /// The store's key and password, for the Admin API's exports and imports (I93b): held for a
    /// clone only.
    sealer: Mutex<Option<Sealer>>,
    /// The console user whose password is temporary (`akit run -admin`, I93c), and until when:
    /// held for a clone only, on every request of the Admin API.
    temporary: Mutex<Option<(String, SystemTime)>>,
    /// Who this akit is, for `GET /node` (I131): held for a clone only.
    node: Mutex<Arc<crate::node::Node>>,
    /// Whether the console runs, which `GET /node` lists (I131).
    console: AtomicBool,
    /// What a manager of many owns here (I136).
    managed: Mutex<crate::managed::State>,
}

impl fmt::Debug for Users {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Users")
            .field("list", &self.list.len())
            .field("save", &self.save.is_some())
            .finish()
    }
}

/// The configs applied to a running akit, the running one last, and the one path every new
/// config takes ([`change`](Configs::change), [`provide`](Configs::provide)). Cloned, it is the
/// same history.
#[derive(Debug, Clone)]
pub struct Configs {
    inner: Arc<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The ETag of `data`, a config as written: a strong entity tag of the first 96 bits of its
/// SHA-256, as fit's.
#[must_use]
pub fn etag_of(data: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut h = Sha256::new();
    h.update(data);
    let sum = h.finalize();
    let mut s = String::with_capacity(26);
    s.push('"');
    for b in sum.iter().take(12) {
        for nibble in [b >> 4, b & 0xf] {
            if let Some(c) = HEX.get(usize::from(nibble)) {
                s.push(char::from(*c));
            }
        }
    }
    s.push('"');
    s
}

/// Whether an `If-Match` or `If-None-Match` field's value names `etag`: `*`, or a list of entity
/// tags one of which is `etag`, compared strongly, as fit's.
#[must_use]
pub fn etag_matches(field: &str, etag: &str) -> bool {
    field
        .split(',')
        .map(str::trim)
        .any(|t| t == "*" || t == etag)
}

/// Whether `name` may name a provider, as fit's: lower-case letters, digits, `_`, `.` and `-`,
/// starting with a letter or digit, at most 63.
fn provider_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= 63
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && bytes.all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b'-')
        })
}

/// Merges `parts` into `data`, the base config from `base_source`, and applies (or checks) the
/// result with `apply`, its errors mapped to where they were written: in the base config, or in
/// what a provider provided.
///
/// With a store, `admin` is the bootstrap file's, added to the config as its last member, so that
/// every position before it stays where it was (fit's `WithAdmin`).
fn merged_apply(
    data: &Arc<str>,
    base_source: &Arc<str>,
    parts: &[Arc<Part>],
    admin: Option<&str>,
    mode: Mode,
    apply: impl FnOnce(&str, Mode) -> Result<Applied, ApplyError>,
) -> Result<(Applied, Merged), Refused> {
    let merged = match akit_provider::merge(data, base_source, parts) {
        Ok(m) => m,
        Err(errs) if !parts.is_empty() => return Err(Refused::Apply(ApplyError::Config(errs))),
        // With nothing to merge, a base config that cannot take parts is loaded as it is, for
        // its own errors.
        Err(_) => Merged::plain(Arc::clone(data)),
    };
    let applied = match admin {
        Some(a) => apply(&with_admin(merged.data(), a), mode),
        None => apply(merged.data(), mode),
    };
    match applied {
        Ok(applied) => Ok((applied, merged)),
        Err(ApplyError::Config(errs)) => {
            Err(Refused::Apply(ApplyError::Config(merged.errors(errs))))
        }
        Err(e) => Err(Refused::Apply(e)),
    }
}

impl Configs {
    /// The history of a server running `data`, version 1 from `source`, keeping `keep` configs
    /// (at least one), the running one included. `data` is standard JSON
    /// ([`akit_config::value::standardize`], [`akit_config::load_files`]).
    #[must_use]
    pub fn new(data: String, source: &str, keep: usize) -> Configs {
        let data: Arc<str> = Arc::from(data);
        let source: Arc<str> = Arc::from(source);
        let merged = akit_provider::merge(&data, &source, &[])
            .unwrap_or_else(|_| Merged::plain(Arc::clone(&data)));
        let (effective, sources, shadowed) = merged.into_parts();
        let first = Arc::new(Version {
            number: 1,
            applied: SystemTime::now(),
            source: Arc::clone(&source),
            etag: etag_of(data.as_bytes()),
            data,
            base_source: source,
            effective,
            sources,
            shadowed,
        });
        let keep = keep.max(1);
        let mut history = VecDeque::with_capacity(keep);
        history.push_back(first);
        Configs {
            inner: Arc::new(Inner {
                keep,
                history: Mutex::new(history),
                applying: Mutex::new(Providers::default()),
                names: Mutex::new(Vec::new()),
                users: Mutex::new(Users::default()),
                stored: AtomicBool::new(false),
                sealer: Mutex::new(None),
                temporary: Mutex::new(None),
                node: Mutex::new(Arc::new(crate::node::Node::new())),
                console: AtomicBool::new(false),
                managed: Mutex::new(crate::managed::State::default()),
            }),
        }
    }

    /// Keeps the base configs applied from here on with `save`, which writes one to the store,
    /// and puts `admin`, the bootstrap file's `admin` as JSON, into every config as it loads; the
    /// base configs are then without `admin`, and one that sets it is refused. The running config
    /// is taken to be saved already. fit's `UseStore`.
    pub fn use_store(&self, admin: Option<String>, save: SaveConfig) {
        let mut p = lock(&self.inner.applying);
        let saved = Sha256::digest(self.current().data.as_bytes());
        p.kept = Some(Kept {
            admin: admin.map(Arc::from),
            save,
            saved,
        });
    }

    /// Gives the console users whose passwords changed since akit started, and `save`, which
    /// writes a new list beside the config in the store. fit's `UseConsoleUsers`.
    pub fn use_console_users(&self, users: Vec<User>, save: SaveUsers) {
        let mut u = lock(&self.inner.users);
        u.list = users;
        u.save = Some(save);
        self.inner.stored.store(true, Ordering::Release);
    }

    /// The console users whose passwords changed since akit started, and whether a store keeps
    /// them.
    #[must_use]
    pub fn console_users(&self) -> (Vec<User>, bool) {
        let u = lock(&self.inner.users);
        (u.list.clone(), u.save.is_some())
    }

    /// Whether a store keeps the console users: what the console's pages are told, read without
    /// waiting on a write (an I/O thread's read).
    #[must_use]
    pub fn users_stored(&self) -> bool {
        self.inner.stored.load(Ordering::Acquire)
    }

    /// Writes `users` beside the config, then keeps them. Writes the store: never on an I/O
    /// thread.
    ///
    /// # Errors
    ///
    /// No store keeps the users, or the write failed: the old list stays.
    pub fn set_console_users(&self, users: Vec<User>) -> Result<(), String> {
        let mut u = lock(&self.inner.users);
        let Some(save) = &u.save else {
            return Err("admin: no store keeps the console users".to_owned());
        };
        save(&users)?;
        u.list = users;
        Ok(())
    }

    /// Writes `hash` as `username`'s console password hash beside the config in the store,
    /// replacing the user's own or adding it, the read and the write one step, so that two
    /// changes at once do not lose one; a temporary password of the user's is one no more. fit's
    /// `SetConsolePassword`. Writes the store: never on an I/O thread.
    ///
    /// # Errors
    ///
    /// No store keeps the users, or the write failed: nothing changes.
    pub fn set_console_password(&self, username: &str, hash: &str) -> Result<(), String> {
        let mut u = lock(&self.inner.users);
        let Some(save) = &u.save else {
            return Err("admin: no store keeps the console users".to_owned());
        };
        let mut out = u.list.clone();
        match out.iter_mut().find(|x| x.username == username) {
            Some(x) => hash.clone_into(&mut x.password_hash),
            None => out.push(User {
                username: username.to_owned(),
                password_hash: hash.to_owned(),
            }),
        }
        save(&out)?;
        u.list = out;
        drop(u);
        let mut t = lock(&self.inner.temporary);
        if t.as_ref().is_some_and(|(user, _)| user == username) {
            *t = None;
        }
        Ok(())
    }

    /// Gives the store's key and password, which seal and open the `fitstore` files the Admin
    /// API exports and imports without a password of their own. fit's `UseSealer`.
    pub fn use_sealer(&self, sealer: Sealer) {
        *lock(&self.inner.sealer) = Some(sealer);
    }

    /// Says who this akit is, for `GET /node` (I131): what `node` leaves empty keeps what was
    /// there (an ID kept nowhere, the host's name), as fit's `UseNode`.
    pub fn use_node(&self, node: crate::node::Node) {
        let mut n = lock(&self.inner.node);
        *n = Arc::new(n.merged(node));
    }

    /// Who this akit is.
    #[must_use]
    pub fn node(&self) -> Arc<crate::node::Node> {
        Arc::clone(&lock(&self.inner.node))
    }

    /// Says whether the console runs, which `GET /node` lists as the capability `console`.
    pub fn set_console_running(&self, running: bool) {
        self.inner.console.store(running, Ordering::Relaxed);
    }

    /// What a manager of many owns here, and its writer (I136).
    pub(crate) fn managed_state(&self) -> &Mutex<crate::managed::State> {
        &self.inner.managed
    }

    /// Whether the console runs.
    #[must_use]
    pub fn console_running(&self) -> bool {
        self.inner.console.load(Ordering::Relaxed)
    }

    /// The store's key and password [`use_sealer`](Configs::use_sealer) gave.
    #[must_use]
    pub fn sealer(&self) -> Option<Sealer> {
        lock(&self.inner.sealer).clone()
    }

    /// Says that the console user `user`'s password is a temporary one, good until `until`
    /// (`akit run -admin`, I93c): the console asks for it to be changed first, and the Admin
    /// API refuses every request until it is. fit's `UseTemporaryPassword`.
    pub fn use_temporary_password(&self, user: &str, until: SystemTime) {
        *lock(&self.inner.temporary) = Some((user.to_owned(), until));
    }

    /// The console user whose password is temporary, and until when.
    #[must_use]
    pub fn temporary_password(&self) -> Option<(String, SystemTime)> {
        lock(&self.inner.temporary).clone()
    }

    /// The running config.
    #[must_use]
    pub fn current(&self) -> Arc<Version> {
        let h = lock(&self.inner.history);
        // Never empty: `new` puts one in, `record` only adds before it drops.
        match h.back() {
            Some(v) => Arc::clone(v),
            None => unreachable_empty(),
        }
    }

    /// The configs kept, oldest first.
    #[must_use]
    pub fn versions(&self) -> Vec<Arc<Version>> {
        lock(&self.inner.history).iter().cloned().collect()
    }

    /// The config applied as version `n`, while it is kept.
    #[must_use]
    pub fn version(&self, n: u64) -> Option<Arc<Version>> {
        lock(&self.inner.history)
            .iter()
            .find(|v| v.number == n)
            .cloned()
    }

    /// The running config as it runs, with the sources in their order of precedence. Takes
    /// two short locks, one after the other: callable on an I/O thread.
    #[must_use]
    pub fn effective(&self) -> Effective {
        let version = self.current();
        let mut precedence = vec![Arc::clone(&version.base_source)];
        precedence.extend(lock(&self.inner.names).iter().cloned());
        Effective {
            version,
            precedence,
        }
    }

    /// Applies `data`, a config as standard JSON from `source`, with `apply`, which loads,
    /// compiles and serves it (the caller's: on the control plane, never an I/O thread), and on
    /// success keeps it as a new version. With `if_match`, an `If-Match` field's value, nothing is
    /// applied unless it names the running config. Configs are applied one at a time.
    ///
    /// # Errors
    ///
    /// [`Refused::Changed`] when `if_match` names another config; [`Refused::Apply`] with what
    /// `apply` refused. The running config stays.
    pub fn update(
        &self,
        source: &str,
        if_match: Option<&str>,
        data: String,
        apply: impl FnOnce(&str) -> Result<Applied, ApplyError>,
    ) -> Result<Outcome, Refused> {
        self.change(
            source,
            if_match,
            Mode::Apply,
            |_| Ok(data),
            |text, _| apply(text),
        )
    }

    /// Applies the config `edit` makes from the running one, as [`update`](Configs::update) does,
    /// with no other config applied between the two: edits from many writers at once each start
    /// from the one before, and none is lost, as fit's `Configs.Update`. `edit` gives the new base
    /// config, standard JSON, whose source is then `source`; what providers provide is merged
    /// into it before `apply` is given it, and `apply`'s errors are mapped to where they were
    /// written. `If-Match` is checked first, under the same lock, so that `edit` is not called
    /// when it is stale. With [`Mode::DryRun`], `apply` is asked only to check the config, and
    /// the outcome is the running config's version and ETag, with the ETag the config would
    /// have.
    ///
    /// # Errors
    ///
    /// [`Refused::Changed`] when `if_match` names another config; what `edit` refuses, as it
    /// refuses it; [`Refused::Apply`] with what merging or `apply` refused. The running config
    /// stays.
    pub fn change(
        &self,
        source: &str,
        if_match: Option<&str>,
        mode: Mode,
        edit: impl FnOnce(&Version) -> Result<String, Refused>,
        apply: impl FnOnce(&str, Mode) -> Result<Applied, ApplyError>,
    ) -> Result<Outcome, Refused> {
        let source: Arc<str> = Arc::from(source);
        let base = Arc::clone(&source);
        self.run(
            source,
            if_match,
            mode,
            |cur| Ok((Arc::from(edit(cur)?), base)),
            apply,
        )
    }

    /// Applies the running base config again, as a new version from `source`, its base source
    /// kept: `POST /reload`, and the certificate files' reload, as fit's `Configs.Reload`.
    ///
    /// # Errors
    ///
    /// As [`change`](Configs::change).
    pub fn reapply(
        &self,
        source: &str,
        if_match: Option<&str>,
        mode: Mode,
        apply: impl FnOnce(&str, Mode) -> Result<Applied, ApplyError>,
    ) -> Result<Outcome, Refused> {
        self.run(
            Arc::from(source),
            if_match,
            mode,
            |cur| Ok((Arc::clone(&cur.data), Arc::clone(&cur.base_source))),
            apply,
        )
    }

    /// Applies again the base config applied as version `n`, as a new version from
    /// [`SOURCE_ROLLBACK`], with what providers provide now: `POST /rollback/{n}`.
    ///
    /// # Errors
    ///
    /// [`Refused::Request`] with 404 when version `n` is no longer kept; otherwise as
    /// [`change`](Configs::change).
    pub fn rollback(
        &self,
        n: u64,
        if_match: Option<&str>,
        mode: Mode,
        apply: impl FnOnce(&str, Mode) -> Result<Applied, ApplyError>,
    ) -> Result<Outcome, Refused> {
        self.run(
            Arc::from(SOURCE_ROLLBACK),
            if_match,
            mode,
            |_| match self.version(n) {
                Some(v) => Ok((Arc::clone(&v.data), Arc::clone(&v.base_source))),
                None => Err(Refused::Request {
                    status: 404,
                    msg: format!("version {n} is not kept"),
                }),
            },
            apply,
        )
    }

    /// Adds a provider, named `name`, whose parts [`provide`](Configs::provide) applies. A
    /// provider added earlier wins over those added later.
    ///
    /// # Errors
    ///
    /// When `name` is not a valid name (lower-case letters, digits, `_`, `.` and `-`, starting
    /// with a letter or digit, at most 63), names a source of the base config (`file`, `admin`,
    /// `rollback`, `certificates`, `store`), or is added already.
    pub fn add_provider(&self, name: &str) -> Result<(), String> {
        if !provider_name(name) {
            return Err(format!(
                "admin: provider name {name:?}: lower-case letters, digits, '.', '_' and '-', starting with a letter or digit, at most 63"
            ));
        }
        if BASE_SOURCES.contains(&name) {
            return Err(format!(
                "admin: provider name {name:?} is a source of the base config"
            ));
        }
        let mut providers = lock(&self.inner.applying);
        if providers.parts.iter().any(|(n, _)| &**n == name) {
            return Err(format!("admin: provider {name:?} is added already"));
        }
        let name: Arc<str> = Arc::from(name);
        providers.parts.push((Arc::clone(&name), None));
        lock(&self.inner.names).push(name);
        Ok(())
    }

    /// Applies `data`, all that the provider `name` provides now (`akit_provider::parse` reads
    /// it), merged into the running base config, with `apply`, as a new version whose source is
    /// `name`; the base config, and its ETag, stay. Data the same as the provider's last is not
    /// applied again (the outcome is the running version, `unchanged`). Empty data provides
    /// nothing: what the provider provided is taken away.
    ///
    /// # Errors
    ///
    /// [`Refused::Apply`] with the part's errors, at their positions in `data`, or with what
    /// `apply` refused, mapped to where it was written; [`Refused::Request`] with 404 when no
    /// provider `name` was added. The running config stays, and so does what the provider
    /// provided before.
    pub fn provide(
        &self,
        name: &str,
        data: &[u8],
        apply: impl FnOnce(&str, Mode) -> Result<Applied, ApplyError>,
    ) -> Result<Outcome, Refused> {
        let part = Arc::new(
            akit_provider::parse(name, data).map_err(|e| Refused::Apply(ApplyError::Config(e)))?,
        );
        let mut providers = lock(&self.inner.applying);
        let Some(i) = providers.parts.iter().position(|(n, _)| &**n == name) else {
            return Err(Refused::Request {
                status: 404,
                msg: format!("no provider {name:?} was added"),
            });
        };
        let cur = self.current();
        let Some((source, old)) = providers.parts.get(i) else {
            return Err(Refused::Request {
                status: 404,
                msg: format!("no provider {name:?} was added"),
            });
        };
        let source = Arc::clone(source);
        if old.as_ref().is_some_and(|o| o.data() == part.data()) {
            return Ok(Outcome {
                version: cur.number,
                etag: cur.etag.clone(),
                source,
                applied: Applied::default(),
                dry_run: false,
                new_etag: String::new(),
                unchanged: true,
            });
        }
        let parts = providers.parts(Some((i, &part)));
        let (applied, merged) = merged_apply(
            &cur.data,
            &cur.base_source,
            &parts,
            providers.admin(),
            Mode::Apply,
            apply,
        )?;
        if let Some(slot) = providers.parts.get_mut(i) {
            slot.1 = Some(part);
        }
        let v = self.record(
            cur.number.saturating_add(1),
            source,
            Arc::clone(&cur.data),
            Arc::clone(&cur.base_source),
            cur.etag.clone(),
            merged,
        );
        Ok(Outcome {
            version: v.number,
            etag: v.etag.clone(),
            source: Arc::clone(&v.source),
            applied,
            dry_run: false,
            new_etag: String::new(),
            unchanged: false,
        })
    }

    /// The one path of every change but a provider's: `edit` gives the new base config and its
    /// base source from the running version.
    fn run(
        &self,
        source: Arc<str>,
        if_match: Option<&str>,
        mode: Mode,
        edit: impl FnOnce(&Version) -> Result<(Arc<str>, Arc<str>), Refused>,
        apply: impl FnOnce(&str, Mode) -> Result<Applied, ApplyError>,
    ) -> Result<Outcome, Refused> {
        let mut providers = lock(&self.inner.applying);
        let cur = self.current();
        if if_match.is_some_and(|m| !etag_matches(m, &cur.etag)) {
            return Err(Refused::Changed);
        }
        let (data, base_source) = edit(&cur)?;
        // With a store, admin comes from the bootstrap file, never from a config sent.
        if providers.admin().is_some() && has_admin(&data) {
            return Err(Refused::Apply(ApplyError::Config(admin_in_store())));
        }
        let parts = providers.parts(None);
        let (mut applied, merged) =
            merged_apply(&data, &base_source, &parts, providers.admin(), mode, apply)?;
        let etag = etag_of(data.as_bytes());
        if mode == Mode::DryRun {
            return Ok(Outcome {
                version: cur.number,
                etag: cur.etag.clone(),
                source,
                applied,
                dry_run: true,
                unchanged: etag == cur.etag,
                new_etag: etag,
            });
        }
        // Kept in the store, unless it holds this config already; one that cannot be written
        // runs, and is said unsaved, as fit's.
        if let Some(k) = &mut providers.kept {
            let sum = Sha256::digest(data.as_bytes());
            if sum != k.saved {
                match (k.save)(&data) {
                    Ok(()) => k.saved = sum,
                    Err(e) => applied.warnings.push(format!(
                        "the config runs, but was not saved to the store: {e}"
                    )),
                }
            }
        }
        let v = self.record(
            cur.number.saturating_add(1),
            source,
            data,
            base_source,
            etag,
            merged,
        );
        Ok(Outcome {
            version: v.number,
            etag: v.etag.clone(),
            source: Arc::clone(&v.source),
            applied,
            dry_run: false,
            new_etag: String::new(),
            unchanged: false,
        })
    }

    /// Keeps a version applied, dropping the oldest past `keep`.
    fn record(
        &self,
        number: u64,
        source: Arc<str>,
        data: Arc<str>,
        base_source: Arc<str>,
        etag: String,
        merged: Merged,
    ) -> Arc<Version> {
        let (effective, sources, shadowed) = merged.into_parts();
        let v = Arc::new(Version {
            number,
            applied: SystemTime::now(),
            source,
            etag,
            data,
            base_source,
            effective,
            sources,
            shadowed,
        });
        let mut h = lock(&self.inner.history);
        h.push_back(Arc::clone(&v));
        while h.len() > self.inner.keep {
            h.pop_front();
        }
        v
    }
}

#[cold]
fn unreachable_empty() -> Arc<Version> {
    // The history always holds the running config; this is never reached. Rather than panic on
    // an I/O thread, an empty config stands in.
    let empty: Arc<str> = Arc::from("");
    let source: Arc<str> = Arc::from(SOURCE_FILE);
    Arc::new(Version {
        number: 0,
        applied: SystemTime::UNIX_EPOCH,
        source: Arc::clone(&source),
        etag: etag_of(b""),
        data: Arc::clone(&empty),
        base_source: source,
        effective: empty,
        sources: Vec::new(),
        shadowed: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::{
        Applied, ApplyError, Configs, Mode, Refused, SOURCE_ADMIN, SOURCE_ROLLBACK, etag_matches,
        etag_of,
    };

    /// A dry run checks and keeps nothing; an edit's refusal applies nothing; a stale If-Match
    /// calls neither the edit nor the apply.
    #[test]
    fn dry_runs_and_edits() {
        let cs = Configs::new("{}".into(), super::SOURCE_FILE, 4);
        let first = cs.current();
        let o = cs
            .change(
                SOURCE_ADMIN,
                Some(&first.etag),
                Mode::DryRun,
                |_| Ok("{\"a\": 1}".into()),
                |_, mode| {
                    assert_eq!(mode, Mode::DryRun);
                    Ok(Applied::default())
                },
            )
            .unwrap();
        assert!(o.dry_run && !o.unchanged);
        assert_eq!((o.version, o.etag.as_str()), (1, first.etag.as_str()));
        assert_eq!(o.new_etag, etag_of(b"{\"a\": 1}"));
        let o = cs
            .change(
                SOURCE_ADMIN,
                None,
                Mode::DryRun,
                |cur| Ok(cur.data().to_owned()),
                |_, _| Ok(Applied::default()),
            )
            .unwrap();
        assert!(o.unchanged && o.new_etag == first.etag);
        assert_eq!(cs.versions().len(), 1);

        let r = cs.change(
            SOURCE_ADMIN,
            None,
            Mode::Apply,
            |_| {
                Err(Refused::Request {
                    status: 404,
                    msg: "no".into(),
                })
            },
            |_, _| unreachable!(),
        );
        assert!(matches!(r, Err(Refused::Request { status: 404, .. })));
        let r = cs.change(
            SOURCE_ADMIN,
            Some("\"stale\""),
            Mode::DryRun,
            |_| unreachable!(),
            |_, _| unreachable!(),
        );
        assert!(matches!(r, Err(Refused::Changed)));

        // The edit sees the running config, and its result is kept.
        let o = cs
            .change(
                SOURCE_ROLLBACK,
                None,
                Mode::Apply,
                |cur| Ok(format!("[{}]", cur.data())),
                |_, _| Ok(Applied::default()),
            )
            .unwrap();
        assert_eq!(
            (o.version, &*o.source, o.dry_run),
            (2, SOURCE_ROLLBACK, false)
        );
        assert_eq!(cs.current().data(), "[{}]");
        assert_eq!(&*cs.current().source, SOURCE_ROLLBACK);
    }

    #[test]
    fn etags() {
        let e = etag_of(b"{}");
        assert_eq!(e.len(), 26);
        assert!(e.starts_with('"') && e.ends_with('"'));
        // SHA-256 of "{}": 44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a.
        assert_eq!(e, "\"44136fa355b3678a1146ad16\"");
        assert!(etag_matches("*", &e));
        assert!(etag_matches(&format!("\"x\", {e}"), &e));
        assert!(!etag_matches("\"x\"", &e));
        assert!(!etag_matches(&format!("W/{e}"), &e));
    }

    #[test]
    fn versions_are_numbered_and_kept() {
        let cs = Configs::new("{}".into(), super::SOURCE_FILE, 2);
        assert_eq!(cs.current().number, 1);
        for i in 0..3 {
            let o = cs
                .update(SOURCE_ADMIN, None, format!("{{\"v\": {i}}}"), |_| {
                    Ok(Applied::default())
                })
                .unwrap();
            assert_eq!(o.version, i + 2);
        }
        let vs: Vec<u64> = cs.versions().iter().map(|v| v.number).collect();
        assert_eq!(vs, [3, 4]);
        assert!(cs.version(2).is_none());
        assert_eq!(cs.version(4).unwrap().data(), "{\"v\": 2}");

        // A stale If-Match, or a refused config, applies nothing.
        let stale = etag_of(b"{}");
        assert!(matches!(
            cs.update(SOURCE_ADMIN, Some(&stale), "{}".into(), |_| unreachable!()),
            Err(Refused::Changed)
        ));
        assert!(matches!(
            cs.update(SOURCE_ADMIN, None, "{}".into(), |_| Err(
                ApplyError::Failed("no".into())
            )),
            Err(Refused::Apply(ApplyError::Failed(_)))
        ));
        assert_eq!(cs.current().number, 4);
        let cur = cs.current().etag.clone();
        assert!(
            cs.update(SOURCE_ADMIN, Some(&cur), "{}".into(), |_| Ok(
                Applied::default()
            ))
            .is_ok()
        );
    }
}
