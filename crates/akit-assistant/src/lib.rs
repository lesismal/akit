#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! The console's assistant (I160–I169, fit's package `assistant`,
//! [decision 044](../../../docs/decisions/044-assistant.md)): a chat panel beside the console's
//! pages that reads what akit runs, explains it and drafts changes a person applies. The console
//! serves it as the module `ai`, at `/api/m/ai/`, only while `admin.assistant` is set: off by
//! default, nothing is sent anywhere.
//!
//! The settings are `admin.assistant`'s until the Settings page saves, and then the store's: its
//! section `assistant`, fit's document `{"settings", "keys"}`, keeps them with the providers' API
//! keys, encrypted with the rest of the store. A key is written there, or read from the
//! environment variable a provider names, and never handed back: the pages see whether one is
//! set, where from, and its last four characters ([`settings`]).
//!
//! ```text
//! GET  /api/settings        the settings in use, as written; the keys as whether they are set;
//!                           the kinds, levels, console users, llm.proxy's API and each kind's
//!                           base URL
//! PUT  /api/settings        {"settings", "keys", "version"}: the settings whole, saved in the
//!                           store with the keys named (an empty one removed); 409 without a
//!                           store, 412 for a version not the one in use, 400 with each error at
//!                           its path
//! ```
//!
//! Nothing here blocks an I/O thread (rule 1): what writes the store runs on a pool of the
//! assistant's own, one write at a time.

pub mod json;
pub mod settings;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use akio::{Pool, PoolConfig};
use akio_http::{Context, Detached, Method, Response};
use akit_config::assistant::{Assistant as Settings, Level};
use akit_roles::pages::{self, Module, ModuleRef};

use crate::settings::{Put, Secret, State};

/// The console module the assistant is served as: fit's `ModuleID`.
pub const MODULE_ID: &str = "ai";
/// The store's section the settings and keys are kept in: fit's `Section`.
pub const SECTION: &str = "assistant";
/// The writes of the store waiting for the pool; one more is answered 503.
pub const MAX_WRITES_WAITING: usize = 16;

/// What a save answers without a store: fit's, in akit's words.
pub const NO_STORE: &str = "the assistant's settings are not saved: akit has no admin.store, so \
     they are admin.assistant's in the config file, and the keys come from api_key_env";

/// Keeps sections beside the config, as the encrypted store does
/// (`akit_admin::store::Store::section`, `set_section`). Its writes are called on the
/// assistant's own pool, never on an I/O thread.
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

/// Says what users changed: never a key.
pub type Log = Arc<dyn Fn(String) + Send + Sync>;

/// The running config, as JSON: whose `llm.proxy` an `llm_proxy` provider calls.
pub type Running = Arc<dyn Fn() -> String + Send + Sync>;

/// What an [`Assistant`] needs.
pub struct Options {
    /// `admin.assistant` as akit started with it, which turned the assistant on.
    pub settings: Settings,
    /// `admin.console`, whose users the settings name. Both change only at a restart, as all of
    /// `admin` does.
    pub console: Option<akit_config::Console>,
    /// The store the settings and keys are kept in; without one, the settings are the config's
    /// and the pages cannot save them.
    pub store: Option<Arc<dyn Store>>,
    /// The running config, whose `llm.proxy` an `llm_proxy` provider calls.
    pub running: Option<Running>,
    /// Says what users changed.
    pub log: Log,
}

impl std::fmt::Debug for Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Options")
            .field("providers", &self.settings.providers.len())
            .field("store", &self.store.is_some())
            .finish_non_exhaustive()
    }
}

/// The assistant of a console. Cheap to clone.
#[derive(Clone)]
pub struct Assistant(Arc<Inner>);

impl std::fmt::Debug for Assistant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Assistant").finish_non_exhaustive()
    }
}

struct Inner {
    o: Options,
    state: Mutex<State>,
    /// Where the store's writes run.
    pool: Arc<Pool>,
}

impl Assistant {
    /// The assistant `o.settings` turns on, with the settings and keys the store keeps when it
    /// keeps some. Settings in the store that no longer load, as after an upgrade that changed
    /// them, are left there and `o.settings` used; the log says so. Reads the store: call it on
    /// the control plane.
    #[must_use]
    pub fn new(o: Options) -> Assistant {
        let raw = if o.settings.raw.is_empty() {
            "{}".to_owned()
        } else {
            o.settings.raw.clone()
        };
        let mut state = State {
            raw,
            settings: Arc::new(o.settings.clone()),
            keys: BTreeMap::new(),
            from_store: false,
            rev: 0,
        };
        if let Some(sec) = o.store.as_ref().and_then(|s| s.section(SECTION)) {
            match settings::read_section(&sec) {
                Err(why) => (o.log)(format!(
                    "assistant: the store's section {SECTION:?} is not the assistant's ({why}); \
                     it is left as it is, and admin.assistant is used"
                )),
                Ok(kept) => {
                    state.keys = kept.keys;
                    if let Some(raw) = kept.settings {
                        match akit_config::assistant::load(&raw, o.console.as_ref()) {
                            Ok(s) => {
                                state.raw = raw;
                                state.settings = Arc::new(s);
                                state.from_store = true;
                            }
                            Err(e) => (o.log)(format!(
                                "assistant: the settings kept in the store do not load, so \
                                 admin.assistant is used until they are saved again: {e}"
                            )),
                        }
                    }
                }
            }
        }
        let mut cfg = PoolConfig::default();
        cfg.min_threads = 0;
        cfg.max_threads = 1;
        cfg.queue_capacity = MAX_WRITES_WAITING;
        Assistant(Arc::new(Inner {
            o,
            state: Mutex::new(state),
            pool: Pool::new(cfg),
        }))
    }

    /// The console module the assistant is served as.
    #[must_use]
    pub fn module(&self) -> ModuleRef {
        Arc::new(self.clone())
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The settings in use.
    #[must_use]
    pub fn settings(&self) -> Arc<Settings> {
        Arc::clone(&self.state().settings)
    }

    /// What the console user `user` may have the assistant do, under the settings in use.
    #[must_use]
    pub fn level(&self, user: &str) -> Level {
        self.settings().level(user)
    }

    /// The key of the provider `name` and where it is from, under the settings in use: for a
    /// model call, never for an answer. Reads the environment.
    #[must_use]
    pub fn key(&self, name: &str) -> Option<(Secret, settings::KeySource)> {
        let st = self.state();
        let p = st.settings.provider(name)?;
        settings::key_of(p, &st.keys)
    }

    fn console_users(&self) -> Vec<String> {
        self.0
            .o
            .console
            .as_ref()
            .map(|c| c.users.iter().map(|u| u.username.clone()).collect())
            .unwrap_or_default()
    }

    fn proxy_api(&self) -> Option<settings::ProxyApi> {
        let running = self.0.o.running.as_ref()?;
        settings::proxy_api(&running())
    }

    /// `GET /api/settings`'s answer.
    #[must_use]
    pub fn view(&self) -> String {
        let st = self.state().clone();
        settings::view(
            &st,
            self.0.o.store.is_some(),
            &self.console_users(),
            self.proxy_api().as_ref(),
        )
    }

    /// `PUT /api/settings`: checked as the config's settings would be, then written to the store
    /// with the keys, on the pool.
    fn put(&self, ctx: &mut Context, user: &str) {
        if self.0.o.store.is_none() {
            return ctx.respond(pages::fail(409, NO_STORE));
        }
        let body = match pages::decode(ctx) {
            Ok(b) => b,
            Err(r) => return ctx.respond(r),
        };
        let put = match settings::check_put(&body.0, self.0.o.console.as_ref()) {
            Ok(p) => p,
            Err((status, body)) => return ctx.respond(pages::json(status, body)),
        };
        let Some(d) = ctx.detach() else { return };
        let me = self.clone();
        let user = user.to_owned();
        self.run(d, move |d| match me.save(&user, put) {
            Ok(()) => d.respond(pages::json(200, me.view())),
            Err((status, msg)) => d.respond(pages::fail(status, &msg)),
        });
    }

    /// Writes `put`'s settings to the store with the keys it changes, and uses them from then on.
    /// It writes a file: on the pool.
    fn save(&self, user: &str, put: Put) -> Result<(), (u16, String)> {
        let Some(store) = &self.0.o.store else {
            return Err((409, NO_STORE.to_owned()));
        };
        let mut st = self.state();
        if !put.version.is_empty() && put.version != st.rev.to_string() {
            return Err((
                412,
                "the settings changed since the page read them; read them again".to_owned(),
            ));
        }
        let (keys, changed) = settings::merged_keys(&st.keys, &put);
        let sec = settings::section(&put.raw, &keys);
        store
            .set_section(SECTION, sec.as_bytes())
            .map_err(|e| (500, format!("saving the settings: {e}")))?;
        let providers = put.settings.providers.len();
        st.raw = put.raw;
        st.settings = Arc::new(put.settings);
        st.keys = keys;
        st.from_store = true;
        st.rev = st.rev.saturating_add(1);
        drop(st);
        (self.0.o.log)(format!(
            "console: {user:?} saved the assistant's settings: {providers} providers; keys \
             changed for [{}]",
            changed.join(" ")
        ));
        Ok(())
    }

    /// Answers `d` with `work` run on the pool: 503 at once when its queue is full.
    fn run(&self, d: Detached, work: impl FnOnce(Detached) + Send + 'static) {
        let cell = Arc::new(Mutex::new(Some(d)));
        let taken = Arc::clone(&cell);
        let job = Box::new(move || {
            let d = taken.lock().unwrap_or_else(PoisonError::into_inner).take();
            if let Some(d) = d {
                work(d);
            }
        });
        if self.0.pool.try_execute(job).is_err() {
            let d = cell.lock().unwrap_or_else(PoisonError::into_inner).take();
            if let Some(d) = d {
                d.respond(pages::fail(503, "the assistant is busy; try again"));
            }
        }
    }
}

impl Module for Assistant {
    fn id(&self) -> &'static str {
        MODULE_ID
    }

    fn serve(&self, ctx: &mut Context, user: &str, path: &str) {
        let method = ctx.method().clone();
        match (path, &method) {
            ("/api/settings", Method::Get | Method::Head) => {
                ctx.respond(pages::json(200, self.view()));
            }
            ("/api/settings", Method::Put) => self.put(ctx, user),
            ("/api/settings", _) => ctx.respond(not_allowed("GET, PUT")),
            _ => ctx.respond(pages::no_endpoint()),
        }
    }
}

fn not_allowed(allow: &str) -> Response {
    pages::fail(405, "method not allowed").header("Allow", allow)
}
