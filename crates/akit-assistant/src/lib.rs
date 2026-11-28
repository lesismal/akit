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
//! POST /api/providers/test  {"provider", "key", "model", "models_only"}: the provider, saved or
//!                           not, tried: its model list (15 s), then a short call (30 s);
//!                           4 at once
//! POST /api/chat            {"message" | "retry" | "answer", "conversation", "page", "lang"}:
//!                           the chat, as Server-Sent Events ([`chat`])
//! POST /api/chat/stop       {"conversation"}: its chat stopped
//! GET  /api/tools           the tools, and which this user may call
//! GET  /api/conversations   the user's conversations, the latest first
//! GET  /api/conversations/<id>, DELETE  one, with the question it waits on; deleted
//! ```
//!
//! Nothing here blocks an I/O thread (rule 1): what writes the store runs on a pool of the
//! assistant's own, one write at a time.

pub mod chat;
pub mod conv;
mod endpoints;
pub mod json;
pub mod model;
pub mod prompt;
pub mod redact;
pub mod registry;
pub mod settings;
mod test;
pub mod tools;

pub use test::{CALL_WAIT, MAX_TESTS, MODELS_WAIT};
#[cfg(any(test, feature = "stand-ins"))]
pub mod standin;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

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

/// The threads the tools run on, and the calls waiting for one.
const TOOL_THREADS: usize = 4;
const TOOL_QUEUE: usize = 64;

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

/// An answer of the Admin API, called in process.
#[derive(Debug, Clone)]
pub struct ApiAnswer {
    /// Its status.
    pub status: u16,
    /// Its body.
    pub body: Vec<u8>,
}

/// The Admin API's reads, called in process (fit's `admincall.go`): what the read tools ask of
/// it, as the console's pages do. Called on the assistant's tools' pool, where it may block.
pub trait Api: Send + Sync {
    /// `method` on `target` (a path and its query) with `body`.
    fn call(&self, method: &str, target: &str, body: Vec<u8>) -> ApiAnswer;
}

/// Where `llm.proxy`'s agents' API listens, its port as bound; `None` while it does not run.
pub type ProxyAddr = Arc<dyn Fn() -> Option<std::net::SocketAddr> + Send + Sync>;

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
    /// The running engine whose connections the model calls take (the listeners'); without
    /// one, nothing is called.
    pub engine: Option<Arc<akio::Engine>>,
    /// The CAs an https provider's certificate is checked against: the system's, read once on
    /// the control plane; akio's own lazy read without them.
    pub roots: Option<akio::tls::RootCerts>,
    /// The model client's bounds.
    pub limits: model::Limits,
    /// Where `llm.proxy`'s API is bound, which an `llm_proxy` provider calls; the running
    /// config's address without it.
    pub proxy_addr: Option<ProxyAddr>,
    /// The Admin API, in process, which the read tools read; without it they say so.
    pub api: Option<Arc<dyn Api>>,
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
    /// The model client, with an engine.
    client: Option<model::Client>,
    /// The provider tests under way.
    tests: AtomicUsize,
    /// The tools the model may call.
    registry: registry::Registry,
    /// The users' conversations.
    chats: Mutex<conv::Chats>,
    /// The chats under way, by user and conversation.
    running: Mutex<HashMap<(String, String), Arc<chat::Chat>>>,
    /// The chats under way, against `max_concurrent_chats`.
    slots: AtomicUsize,
    /// Where the tools run.
    tools_pool: Arc<Pool>,
    /// When the assistant started.
    started: Instant,
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
        let client = o.engine.as_ref().map(|e| {
            let mut lim = o.limits;
            lim.max_stream_bytes = state.settings.limits.stream_bytes();
            model::Client::new(Arc::clone(e), lim, o.roots.clone())
        });
        let mut tc = PoolConfig::default();
        tc.min_threads = 0;
        tc.max_threads = TOOL_THREADS;
        tc.queue_capacity = TOOL_QUEUE;
        Assistant(Arc::new(Inner {
            o,
            state: Mutex::new(state),
            pool: Pool::new(cfg),
            client,
            tests: AtomicUsize::new(0),
            registry: registry::Registry::new(tools::read_tools()),
            chats: Mutex::new(conv::Chats::default()),
            running: Mutex::new(HashMap::new()),
            slots: AtomicUsize::new(0),
            tools_pool: Pool::new(tc),
            started: Instant::now(),
        }))
    }

    /// The model client, when the assistant has an engine to call with.
    #[must_use]
    pub fn client(&self) -> Option<&model::Client> {
        self.0.client.as_ref()
    }

    /// The tools the model may call.
    #[must_use]
    pub fn registry(&self) -> &registry::Registry {
        &self.0.registry
    }

    /// The redactor of what the tools hand a model under `s`: the providers' keys, from the
    /// store and the environment, and `data.redact`'s patterns.
    #[must_use]
    pub fn redactor(&self, s: &Settings) -> redact::Redactor {
        let mut literals: Vec<String> = self
            .state()
            .keys
            .values()
            .map(|k| k.expose().to_owned())
            .collect();
        for p in &s.providers {
            if !p.api_key_env.is_empty() {
                if let Ok(k) = std::env::var(&p.api_key_env) {
                    literals.push(k);
                }
            }
        }
        redact::Redactor::new(literals, &s.data.redact)
    }

    /// The Admin API's answer to `method` on `target`, in process: the body of a 2xx, or the
    /// error it gave. On the tools' pool.
    ///
    /// # Errors
    ///
    /// The API is not reachable here, or answered otherwise than 2xx.
    pub fn api_json(&self, method: &str, target: &str, body: Vec<u8>) -> Result<String, String> {
        let Some(api) = &self.0.o.api else {
            return Err("the Admin API is not reachable from the assistant here".to_owned());
        };
        let a = api.call(method, target, body);
        let text = String::from_utf8_lossy(&a.body).into_owned();
        if (200..300).contains(&a.status) {
            return Ok(text);
        }
        let msg = akit_config::value::parse(&text)
            .ok()
            .map(|v| json::str_of(&v, "error").to_owned())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| prompt::clip(&text, 512));
        Err(format!("the Admin API answered {}: {msg}", a.status))
    }

    fn chats(&self) -> MutexGuard<'_, conv::Chats> {
        self.0.chats.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `user`'s conversations, read from the store the first time.
    fn user_chats<'c>(&self, chats: &'c mut conv::Chats, user: &str) -> &'c mut conv::UserChats {
        let u = chats.users.entry(user.to_owned()).or_default();
        if !u.loaded {
            u.loaded = true;
            if let Some(data) = self
                .0
                .o
                .store
                .as_ref()
                .and_then(|s| s.section(&conv::section_of(user)))
            {
                match conv::UserChats::read(&data) {
                    Some(k) => *u = k,
                    None => (self.0.o.log)(format!(
                        "assistant: the conversations kept for {user:?} do not load, and are \
                         started again"
                    )),
                }
            }
        }
        u
    }

    /// Has `user`'s conversations written to the store, on the pool, after what waits.
    fn save_chats(&self, user: &str) {
        if self.0.o.store.is_none() {
            return;
        }
        {
            let mut chats = self.chats();
            chats.dirty.insert(user.to_owned());
            if chats.saving {
                return;
            }
            chats.saving = true;
        }
        let me = self.clone();
        let job = Box::new(move || me.save_dirty());
        if self.0.pool.try_execute(job).is_err() {
            // The pool is full: the next chat that ends writes it.
            self.chats().saving = false;
        }
    }

    fn save_dirty(&self) {
        loop {
            let (user, data) = {
                let mut chats = self.chats();
                let Some(user) = chats.dirty.pop_first() else {
                    chats.saving = false;
                    return;
                };
                let data = chats.users.get(&user).map(conv::UserChats::section);
                (user, data)
            };
            let (Some(data), Some(store)) = (data, &self.0.o.store) else {
                continue;
            };
            if let Err(e) = store.set_section(&conv::section_of(&user), data.as_bytes()) {
                (self.0.o.log)(format!(
                    "assistant: the conversations of {user:?} are kept in memory only: {e}"
                ));
            }
        }
    }

    fn take_slot(&self, max: u64) -> bool {
        let max = usize::try_from(max).unwrap_or(usize::MAX);
        let mut n = self.0.slots.load(Ordering::SeqCst);
        loop {
            if n >= max {
                return false;
            }
            match self.0.slots.compare_exchange_weak(
                n,
                n.saturating_add(1),
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return true,
                Err(now) => n = now,
            }
        }
    }

    fn give_slot(&self) {
        let mut n = self.0.slots.load(Ordering::SeqCst);
        while n > 0 {
            match self.0.slots.compare_exchange_weak(
                n,
                n.saturating_sub(1),
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return,
                Err(now) => n = now,
            }
        }
    }

    fn start_chat(&self, user: &str, conv: &str, c: &Arc<chat::Chat>) {
        self.0
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((user.to_owned(), conv.to_owned()), Arc::clone(c));
    }

    /// A chat ended: its slot given back.
    fn end_chat(&self, user: &str, conv: &str) {
        let gone = self
            .0
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&(user.to_owned(), conv.to_owned()));
        if gone.is_some() {
            self.give_slot();
        }
    }

    fn running(&self, user: &str, conv: &str) -> Option<Arc<chat::Chat>> {
        self.0
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&(user.to_owned(), conv.to_owned()))
            .cloned()
    }

    /// Closes the model client: calls after it fail.
    pub fn close(&self) {
        if let Some(c) = &self.0.client {
            c.close();
        }
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
            ("/api/providers/test", Method::Post) => self.test(ctx),
            ("/api/chat", Method::Post) => self.chat(ctx, user),
            ("/api/chat/stop", Method::Post) => self.stop_chat(ctx, user),
            ("/api/tools", Method::Get) => ctx.respond(pages::json(200, self.tools_view(user))),
            ("/api/conversations", Method::Get) => self.conversations(ctx, user),
            (p, Method::Get | Method::Delete) if p.starts_with("/api/conversations/") => {
                let id = p.trim_start_matches("/api/conversations/").to_owned();
                let delete = method == Method::Delete;
                self.conversation(ctx, user, &id, delete);
            }
            ("/api/providers/test", _) => ctx.respond(not_allowed("POST")),
            _ => ctx.respond(pages::no_endpoint()),
        }
    }
}

fn not_allowed(allow: &str) -> Response {
    pages::fail(405, "method not allowed").header("Allow", allow)
}
