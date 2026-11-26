//! The Docker provider (I58), fit's `provider/docker`: it turns the labels of running containers
//! into routes, upstreams and services, and follows the daemon's events to keep them up to date.
//!
//! It speaks the Engine API over plain HTTP, on a Unix socket or TCP, with unversioned paths
//! (fit's decision 010), on akio's HTTP client: the event stream is read, and the listings
//! collected, by callbacks on akio's I/O threads, which only parse what arrived and hand it on.
//! A thread of the provider's own keeps the time (the debounce after events, the refresh, the
//! backoff after a failure) and sends the parts; it waits on nothing an I/O thread holds.
//!
//! The labels, under a prefix (`akit` by default, fit's `fit` renamed), are fit's
//! ([`crate::labels`]), with these of the container's own:
//!
//! ```text
//! akit.enable=true                take the container (needed unless expose_all)
//! akit.name=<name>                the upstream's name, shared by replicas; the Compose project
//!                                and service ("<project>-<service>") or the container's name
//!                                when left out
//! akit.port=<port>                the container's port; its only TCP port when left out
//! akit.network=<network>          the network whose address is used
//! ```
//!
//! Replicas, the containers of one name, add their addresses to one upstream; its routes and
//! services are read from the first by container name.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use akio::Engine;
use akio_http::Headers;
use akio_http::client::{
    CancelToken, Client, ClientError, ClientOptions, ClientRequest, RedirectPolicy, ResponseBody,
    ResponseHandler, ResponseHead,
};
use akit_config::value::{self, Value};
use akit_config::{DockerEndpoint, DockerProvider};

use crate::labels::{self, Group};
use crate::run::{Provider, Update};

/// The most bytes of a listing of the containers.
pub(crate) const MAX_LIST_BYTES: usize = 16 << 20;
/// The most bytes of one event.
pub(crate) const MAX_EVENT_BYTES: usize = 1 << 20;
/// The most bytes of an error's answer reported.
const MAX_ERROR_BYTES: usize = 4 << 10;

/// The provider's timings: how long it waits after an event for more before listing the
/// containers again, and the bounds of the wait before trying the daemon again after it failed.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timings {
    pub(crate) debounce: Duration,
    pub(crate) min_backoff: Duration,
    pub(crate) max_backoff: Duration,
}

impl Default for Timings {
    fn default() -> Timings {
        Timings {
            debounce: Duration::from_millis(200),
            min_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(30),
        }
    }
}

/// Where the provider's reports go: the containers left out, the daemon failing.
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// The provider kinds of this module, for [`crate::Kinds`].
pub const KINDS: &[(&str, crate::Make<akit_config::Provider>)] = &[("docker", make)];

fn make(name: &str, pc: &akit_config::Provider) -> Result<(Box<dyn Provider>, String), String> {
    match &pc.kind {
        akit_config::ProviderKind::Docker(d) => {
            let watching = format!("watching Docker at {}", dial(&d.endpoint));
            let log: Log = crate::watch::stderr_log();
            Ok((Box::new(Docker::new(name, d.clone(), log)), watching))
        }
        #[allow(unreachable_patterns)]
        _ => Err(format!("provider {name}: not a docker provider")),
    }
}

/// fit's `network:address` of an endpoint.
fn dial(ep: &DockerEndpoint) -> String {
    match ep {
        DockerEndpoint::Unix(p) => format!("unix:{}", p.display()),
        DockerEndpoint::Tcp(a) => format!("tcp:{a}"),
    }
}

/// A Docker provider.
pub struct Docker {
    name: String,
    opts: DockerProvider,
    log: Log,
    timings: Timings,
    /// The engine its requests run on; one of its own, made at start, when not given.
    engine: Option<Arc<Engine>>,
    running: Option<Running>,
}

impl std::fmt::Debug for Docker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Docker")
            .field("name", &self.name)
            .field("opts", &self.opts)
            .finish_non_exhaustive()
    }
}

struct Running {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    /// The engine made for it, shut down when it stops.
    own: Option<Arc<Engine>>,
}

impl Docker {
    /// A Docker provider named `name`, with the settings `opts`; `log` hears what it reports.
    #[must_use]
    pub fn new(name: &str, opts: DockerProvider, log: Log) -> Docker {
        Docker {
            name: name.to_owned(),
            opts,
            log,
            timings: Timings::default(),
            engine: None,
            running: None,
        }
    }

    /// Runs its requests on `engine` rather than on an engine of its own.
    #[must_use]
    pub fn with_engine(mut self, engine: Arc<Engine>) -> Docker {
        self.engine = Some(engine);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_timings(mut self, t: Timings) -> Docker {
        self.timings = t;
        self
    }
}

impl Provider for Docker {
    fn name(&self) -> &str {
        &self.name
    }

    fn start(&mut self, update: Update) -> io::Result<()> {
        if self.running.is_some() {
            return Ok(());
        }
        let (engine, own) = match &self.engine {
            Some(e) => (Arc::clone(e), None),
            None => {
                let mut cfg = akio::Config::default();
                cfg.shutdown_timeout = Duration::from_secs(1);
                let e = Engine::builder()
                    .client_threads(1)
                    .config(cfg)
                    .build()
                    .map_err(|e| io::Error::other(format!("docker's client: {e}")))?;
                e.start()
                    .map_err(|e| io::Error::other(format!("docker's client: {e}")))?;
                let e = Arc::new(e);
                (Arc::clone(&e), Some(e))
            }
        };
        let shared = Arc::new(Shared::default());
        let mut w = Watcher {
            name: self.name.clone(),
            prefix: format!("{}.", self.opts.label_prefix),
            client: client(&engine, &self.opts),
            base: match &self.opts.endpoint {
                DockerEndpoint::Unix(_) => "http://docker".to_owned(),
                DockerEndpoint::Tcp(a) => format!("http://{a}"),
            },
            opts: self.opts.clone(),
            timings: self.timings,
            log: Arc::clone(&self.log),
            shared: Arc::clone(&shared),
            update,
            last: None,
            problems: Vec::new(),
        };
        let thread = std::thread::Builder::new()
            .name(format!("akit-provider-{}", self.name))
            .spawn(move || w.run());
        let thread = match thread {
            Ok(t) => t,
            Err(e) => {
                if let Some(e) = &own {
                    e.shutdown();
                }
                return Err(e);
            }
        };
        self.running = Some(Running {
            shared,
            thread: Some(thread),
            own,
        });
        Ok(())
    }

    fn stop(&mut self) {
        let Some(mut r) = self.running.take() else {
            return;
        };
        {
            let mut st = r.shared.lock();
            st.stop = true;
            for t in st.tokens.drain(..) {
                t.cancel();
            }
        }
        r.shared.cv.notify_all();
        if let Some(t) = r.thread.take() {
            let _ = t.join();
        }
        if let Some(e) = r.own.take() {
            e.shutdown();
            e.wait();
        }
    }
}

impl Drop for Docker {
    fn drop(&mut self) {
        self.stop();
    }
}

fn client(engine: &Arc<Engine>, opts: &DockerProvider) -> Client {
    let mut o = ClientOptions::default();
    o.max_head_bytes = 64 << 10;
    o.max_idle_per_host = 2;
    o.max_idle = 2;
    o.idle_timeout = Some(Duration::from_secs(30));
    // fit's ResponseHeaderTimeout: for the listings and the event stream alike.
    o.response_header_timeout = Some(opts.request_timeout);
    let c = Client::new(Arc::clone(engine))
        .with_options(o)
        .with_redirects(RedirectPolicy::none());
    match &opts.endpoint {
        DockerEndpoint::Unix(p) => c.with_unix_socket(Some(p.clone())),
        DockerEndpoint::Tcp(_) => c,
    }
}

/// `d` from now; a day from now when `d` is past what an `Instant` holds.
fn after(d: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(d)
        .or_else(|| now.checked_add(Duration::from_secs(86_400)))
        .unwrap_or(now)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the I/O threads' callbacks tell the provider's thread, for the watch `epoch`.
#[derive(Default)]
struct Inbox {
    stop: bool,
    /// The watch the callbacks belong to; those of an older one are dropped.
    epoch: u64,
    /// The event stream's head came: `Ok`, or why it failed.
    opened: Option<Result<(), String>>,
    /// An event that may change what the containers provide came.
    changed: bool,
    /// The event stream ended, and why.
    ended: Option<String>,
    /// A listing came: the body, or why it failed.
    listed: Option<Result<Vec<u8>, String>>,
    /// The requests under way, cancelled when the watch ends or the provider stops.
    tokens: Vec<CancelToken>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<Inbox>,
    cv: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inbox> {
        lock(&self.state)
    }

    /// Changes the inbox for the watch `epoch`, if it is still the current one, and wakes the
    /// provider's thread. Never blocks for long: callable on an I/O thread.
    fn tell(&self, epoch: u64, f: impl FnOnce(&mut Inbox)) {
        {
            let mut st = self.lock();
            if st.epoch != epoch {
                return;
            }
            f(&mut st);
        }
        self.cv.notify_all();
    }
}

/// The provider's thread: fit's `Provider.Run`.
struct Watcher {
    name: String,
    prefix: String,
    client: Client,
    base: String,
    opts: DockerProvider,
    timings: Timings,
    log: Log,
    shared: Arc<Shared>,
    update: Update,
    /// The part sent last.
    last: Option<Vec<u8>>,
    /// The problems reported at the last listing.
    problems: Vec<String>,
}

/// Why a watch ended: the provider stopped, or the daemon failed (after a listing succeeded or
/// not).
enum End {
    Stopped,
    Failed { listed: bool, err: String },
}

impl Watcher {
    /// Follows the daemon until the provider stops. When the daemon cannot be reached, the last
    /// part stays, and the daemon is tried again, from `min_backoff` to `max_backoff` apart.
    fn run(&mut self) {
        let mut backoff = self.timings.min_backoff;
        loop {
            let (listed, err) = match self.watch() {
                End::Stopped => return,
                End::Failed { listed, err } => (listed, err),
            };
            if listed {
                backoff = self.timings.min_backoff;
            }
            (self.log)(&format!(
                "provider {}: {err}; trying again in {}",
                self.name,
                akit_config::duration::format(backoff)
            ));
            if self.sleep(after(backoff)) {
                return;
            }
            backoff = backoff.saturating_mul(2).min(self.timings.max_backoff);
        }
    }

    /// Waits until `until`; whether the provider stopped meanwhile.
    fn sleep(&self, until: Instant) -> bool {
        let mut st = self.shared.lock();
        loop {
            if st.stop {
                return true;
            }
            let now = Instant::now();
            if now >= until {
                return false;
            }
            st = self
                .shared
                .cv
                .wait_timeout(st, until.saturating_duration_since(now))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Starts a watch: a new generation, with nothing of the last one's left.
    fn begin(&self) -> Option<u64> {
        let mut st = self.shared.lock();
        if st.stop {
            return None;
        }
        st.epoch = st.epoch.wrapping_add(1);
        st.opened = None;
        st.changed = false;
        st.ended = None;
        st.listed = None;
        for t in st.tokens.drain(..) {
            t.cancel();
        }
        Some(st.epoch)
    }

    /// A token for a request of the watch `epoch`, cancelled with it; `None` once stopped.
    fn token(&self, epoch: u64) -> Option<CancelToken> {
        let mut st = self.shared.lock();
        if st.stop || st.epoch != epoch {
            return None;
        }
        let t = CancelToken::new();
        // The tokens of finished requests go when the next is made: at most a few wait.
        st.tokens.retain(|t| !t.is_cancelled());
        st.tokens.push(t.clone());
        Some(t)
    }

    fn url(&self, path: &str, filters: &str) -> String {
        let mut u = format!("{}{path}", self.base);
        if !filters.is_empty() {
            u.push_str("?filters=");
            query_escape(&mut u, filters);
        }
        u
    }

    /// Follows the daemon's events, and lists the containers at the start, after events and
    /// every refresh interval, until the daemon fails or the provider stops: fit's `watch`.
    fn watch(&mut self) -> End {
        let Some(epoch) = self.begin() else {
            return End::Stopped;
        };
        let end = self.watch_epoch(epoch);
        // Whatever ended it, its requests end too.
        let mut st = self.shared.lock();
        for t in st.tokens.drain(..) {
            t.cancel();
        }
        drop(st);
        end
    }

    fn watch_epoch(&mut self, epoch: u64) -> End {
        // The events first, so that no change between the listing and them is missed.
        let Some(token) = self.token(epoch) else {
            return End::Stopped;
        };
        let req = ClientRequest::get(&self.url("/events", r#"{"type":["container"]}"#))
            .cancel_with(&token);
        self.client
            .request_with(req, Events::new(Arc::clone(&self.shared), epoch));
        let opened = self.wait(|st| st.opened.take());
        match opened {
            None => return End::Stopped,
            Some(Err(err)) => {
                return End::Failed { listed: false, err };
            }
            Some(Ok(())) => {}
        }
        if let Err(err) = self.refresh(epoch, true) {
            return err.unwrap_or(End::Stopped).listed(false);
        }
        let mut tick = after(self.opts.refresh_interval);
        let mut fire: Option<Instant> = None;
        loop {
            let deadline = fire.map_or(tick, |f| f.min(tick));
            enum Woke {
                Stop,
                Ended(String),
                Changed,
                Time,
            }
            let woke = {
                let mut st = self.shared.lock();
                loop {
                    if st.stop {
                        break Woke::Stop;
                    }
                    if let Some(e) = st.ended.take() {
                        break Woke::Ended(e);
                    }
                    if std::mem::take(&mut st.changed) {
                        break Woke::Changed;
                    }
                    let now = Instant::now();
                    if now >= deadline {
                        break Woke::Time;
                    }
                    st = self
                        .shared
                        .cv
                        .wait_timeout(st, deadline.saturating_duration_since(now))
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
            };
            match woke {
                Woke::Stop => return End::Stopped,
                Woke::Ended(e) => {
                    return End::Failed {
                        listed: true,
                        err: format!("events: {e}"),
                    };
                }
                Woke::Changed => {
                    if fire.is_none() {
                        fire = Some(after(self.timings.debounce));
                    }
                }
                Woke::Time => {
                    let now = Instant::now();
                    if fire.is_some_and(|f| now >= f) {
                        fire = None;
                        if let Err(e) = self.refresh(epoch, false) {
                            return e.unwrap_or(End::Stopped).listed(true);
                        }
                    }
                    if now >= tick {
                        tick = after(self.opts.refresh_interval);
                        if let Err(e) = self.refresh(epoch, true) {
                            return e.unwrap_or(End::Stopped).listed(true);
                        }
                    }
                }
            }
        }
    }

    /// Waits for what `take` takes from the inbox; `None` when the provider stops first.
    fn wait<T>(&self, mut take: impl FnMut(&mut Inbox) -> Option<T>) -> Option<T> {
        let mut st = self.shared.lock();
        loop {
            if st.stop {
                return None;
            }
            if let Some(v) = take(&mut st) {
                return Some(v);
            }
            st = self
                .shared
                .cv
                .wait(st)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Lists the containers and sends what they provide, when it changed or `always` is set.
    /// `Err(None)` when the provider stopped meanwhile.
    fn refresh(&mut self, epoch: u64, always: bool) -> Result<(), Option<End>> {
        let filters = if self.opts.expose_all {
            String::new()
        } else {
            let mut f = String::from(r#"{"label":["#);
            value::write_string(&mut f, &format!("{}enable=true", self.prefix));
            f.push_str("]}");
            f
        };
        let Some(token) = self.token(epoch) else {
            return Err(None);
        };
        let req = ClientRequest::get(&self.url("/containers/json", &filters))
            .timeout(self.opts.request_timeout)
            .cancel_with(&token);
        self.client.request_with(
            req,
            Collect::new(
                Arc::clone(&self.shared),
                epoch,
                "/containers/json",
                MAX_LIST_BYTES,
            ),
        );
        let listed = self.wait(|st| st.listed.take()).ok_or(None)?;
        let fail = |err: String| Some(End::Failed { listed: false, err });
        let data = listed.map_err(fail)?;
        let cs = parse_list(&data).map_err(|e| fail(format!("GET /containers/json: {e}")))?;
        let (part, problems) = translate(cs, &self.opts);
        for msg in &problems {
            if !self.problems.contains(msg) {
                (self.log)(&format!("provider {}: {msg}", self.name));
            }
        }
        self.problems = problems;
        if always || self.last.as_ref() != Some(&part) {
            self.last = Some(part.clone());
            self.update.send(part);
        }
        Ok(())
    }
}

impl End {
    /// A failure, saying whether a listing had succeeded before it.
    fn listed(self, listed: bool) -> End {
        match self {
            End::Failed { err, .. } => End::Failed { listed, err },
            End::Stopped => End::Stopped,
        }
    }
}

/// Appends `s` escaped as Go's `url.QueryEscape` escapes it.
fn query_escape(b: &mut String, s: &str) {
    for c in s.bytes() {
        match c {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                b.push(char::from(c));
            }
            b' ' => b.push('+'),
            _ => {
                let _ = write!(b, "%{c:02X}");
            }
        }
    }
}

/// The event stream, read on an I/O thread: one JSON object a line, each that may change what
/// the containers provide (any of a container's but its `exec_` ones, which health checks make)
/// marks the inbox changed. fit's `readEvents`.
struct Events {
    shared: Arc<Shared>,
    epoch: u64,
    /// The line being read, at most [`MAX_EVENT_BYTES`]; an error answer's body, at most
    /// [`MAX_ERROR_BYTES`].
    line: Vec<u8>,
    /// The head came, and was 200.
    opened: bool,
    /// The status line of an answer other than 200.
    refused: Option<String>,
    /// The stream was given up: nothing more is told.
    done: bool,
    body: Option<ResponseBody>,
}

impl Events {
    fn new(shared: Arc<Shared>, epoch: u64) -> Events {
        Events {
            shared,
            epoch,
            line: Vec::new(),
            opened: false,
            refused: None,
            done: false,
            body: None,
        }
    }

    /// Gives the stream up, telling why: as the reason it could not be opened, or the one it
    /// ended for.
    fn fail(&mut self, e: String) {
        if std::mem::replace(&mut self.done, true) {
            return;
        }
        let opened = self.opened;
        self.shared.tell(self.epoch, |st| {
            if opened {
                st.ended = Some(e);
            } else {
                st.opened = Some(Err(e));
            }
        });
        if let Some(b) = self.body.take() {
            b.cancel();
        }
    }
}

impl ResponseHandler for Events {
    fn on_head(&mut self, head: ResponseHead, body: ResponseBody) {
        self.body = Some(body);
        if head.status != 200 {
            // The answer's body says why: read it, then fail.
            self.refused = Some(format!("GET /events: {} {}", head.status, head.reason));
            return;
        }
        self.opened = true;
        self.shared.tell(self.epoch, |st| st.opened = Some(Ok(())));
    }

    fn on_data(&mut self, chunk: &[u8]) {
        if self.done {
            return;
        }
        if self.refused.is_some() {
            let room = MAX_ERROR_BYTES.saturating_sub(self.line.len());
            self.line
                .extend_from_slice(chunk.get(..room.min(chunk.len())).unwrap_or_default());
            return;
        }
        let mut changed = false;
        for piece in chunk.split_inclusive(|&b| b == b'\n') {
            let (data, whole) = match piece.strip_suffix(b"\n") {
                Some(d) => (d, true),
                None => (piece, false),
            };
            if self.line.len().saturating_add(data.len()) > MAX_EVENT_BYTES {
                return self.fail(format!("an event larger than {MAX_EVENT_BYTES} bytes"));
            }
            self.line.extend_from_slice(data);
            if !whole {
                continue;
            }
            let line = std::mem::take(&mut self.line);
            match event_changes(&line) {
                Ok(c) => changed |= c,
                Err(e) => return self.fail(e),
            }
        }
        if changed {
            self.shared.tell(self.epoch, |st| st.changed = true);
        }
    }

    fn on_end(&mut self, _trailers: &Headers) {
        match self.refused.take() {
            Some(head) => {
                let msg = error_message(&head, &self.line);
                self.fail(msg);
            }
            None => self.fail("the daemon ended the event stream".to_owned()),
        }
    }

    fn on_error(&mut self, e: ClientError) {
        let msg = match self.refused.take() {
            Some(head) => error_message(&head, &self.line),
            None if self.opened => e.to_string(),
            None => format!("GET /events: {e}"),
        };
        self.fail(msg);
    }
}

/// The message of an answer other than 200: its status and its body, trimmed.
fn error_message(head: &str, body: &[u8]) -> String {
    let body = String::from_utf8_lossy(body);
    format!("{head}: {}", body.trim())
}

/// Whether the event `line` may change what the containers provide; an error when it is not
/// JSON.
pub(crate) fn event_changes(line: &[u8]) -> Result<bool, String> {
    let text =
        std::str::from_utf8(line).map_err(|_| "an event that is not JSON: not UTF-8".to_owned())?;
    if text.trim().is_empty() {
        return Ok(false);
    }
    let v = value::parse(text).map_err(|e| format!("an event that is not JSON: {e}"))?;
    let s = |k: &str| match v.get(k) {
        Some(Value::String(s)) => s.as_str(),
        _ => "",
    };
    let (ty, action) = (s("Type"), s("Action"));
    Ok(!(!ty.is_empty() && ty != "container" || action.starts_with("exec_")))
}

/// A response collected whole, at most `limit` bytes, read on an I/O thread: a listing.
struct Collect {
    shared: Arc<Shared>,
    epoch: u64,
    path: &'static str,
    limit: usize,
    /// The status line, when it is not 200.
    status: Option<String>,
    body: Vec<u8>,
    handle: Option<ResponseBody>,
    /// It answered: nothing more is told.
    done: bool,
}

impl Collect {
    fn new(shared: Arc<Shared>, epoch: u64, path: &'static str, limit: usize) -> Collect {
        Collect {
            shared,
            epoch,
            path,
            limit,
            status: None,
            body: Vec::new(),
            handle: None,
            done: false,
        }
    }

    fn answer(&mut self, r: Result<Vec<u8>, String>) {
        if std::mem::replace(&mut self.done, true) {
            return;
        }
        if let Some(h) = self.handle.take() {
            if r.is_err() {
                h.cancel();
            }
        }
        self.shared.tell(self.epoch, |st| st.listed = Some(r));
    }

    /// An answer other than 200: its status and what came of its body.
    fn refused(&mut self) -> Option<String> {
        let s = self.status.take()?;
        Some(error_message(
            &format!("GET {}: {s}", self.path),
            &self.body,
        ))
    }
}

impl ResponseHandler for Collect {
    fn on_head(&mut self, head: ResponseHead, body: ResponseBody) {
        if head.status != 200 {
            self.status = Some(format!("{} {}", head.status, head.reason));
        }
        self.handle = Some(body);
    }

    fn on_data(&mut self, chunk: &[u8]) {
        if self.done {
            return;
        }
        if self.status.is_some() {
            let room = MAX_ERROR_BYTES.saturating_sub(self.body.len());
            self.body
                .extend_from_slice(chunk.get(..room.min(chunk.len())).unwrap_or_default());
            return;
        }
        if self.body.len().saturating_add(chunk.len()) > self.limit {
            let msg = format!(
                "GET {}: the list is larger than {} bytes",
                self.path, self.limit
            );
            return self.answer(Err(msg));
        }
        self.body.extend_from_slice(chunk);
    }

    fn on_end(&mut self, _trailers: &Headers) {
        let r = match self.refused() {
            Some(e) => Err(e),
            None => Ok(std::mem::take(&mut self.body)),
        };
        self.answer(r);
    }

    fn on_error(&mut self, e: ClientError) {
        let msg = self
            .refused()
            .unwrap_or_else(|| format!("GET {}: {e}", self.path));
        self.answer(Err(msg));
    }
}

/// What the daemon's list says of a running container.
#[derive(Debug, Clone, Default)]
pub(crate) struct Container {
    pub(crate) id: String,
    pub(crate) names: Vec<String>,
    pub(crate) state: String,
    pub(crate) status: String,
    pub(crate) labels: BTreeMap<String, String>,
    pub(crate) ports: Vec<Port>,
    pub(crate) network_mode: String,
    /// Its networks by name: the IPv4 address and the global IPv6 address on each.
    pub(crate) networks: BTreeMap<String, (String, String)>,
}

/// A port of a container, as the list gives it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Port {
    pub(crate) ip: String,
    pub(crate) private: u16,
    pub(crate) public: u16,
    pub(crate) ty: String,
}

impl Container {
    pub(crate) fn name(&self) -> &str {
        match self.names.first() {
            Some(n) => n.strip_prefix('/').unwrap_or(n),
            None => &self.id,
        }
    }
}

/// Reads a listing: a JSON array of containers. What is not as the daemon writes it is left
/// out of the container.
pub(crate) fn parse_list(data: &[u8]) -> Result<Vec<Container>, String> {
    let text =
        std::str::from_utf8(data).map_err(|e| format!("not UTF-8 at byte {}", e.valid_up_to()))?;
    let v = value::parse(text).map_err(|e| e.to_string())?;
    let Value::Array(items) = v else {
        return Err("the list is not a JSON array".to_owned());
    };
    Ok(items.iter().map(container).collect())
}

fn string(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn port_number(v: Option<&Value>) -> u16 {
    match v {
        Some(Value::Number(n)) => n.parse().unwrap_or(0),
        _ => 0,
    }
}

fn container(v: &Value) -> Container {
    let mut c = Container {
        id: string(v.get("Id")),
        state: string(v.get("State")),
        status: string(v.get("Status")),
        network_mode: string(v.get("HostConfig").and_then(|h| h.get("NetworkMode"))),
        ..Container::default()
    };
    if let Some(Value::Array(ns)) = v.get("Names") {
        c.names = ns
            .iter()
            .filter_map(|n| match n {
                Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
    }
    if let Some(Value::Object(ls)) = v.get("Labels") {
        for (k, l) in ls {
            if let Value::String(s) = l {
                c.labels.insert(k.clone(), s.clone());
            }
        }
    }
    if let Some(Value::Array(ps)) = v.get("Ports") {
        c.ports = ps
            .iter()
            .map(|p| Port {
                ip: string(p.get("IP")),
                private: port_number(p.get("PrivatePort")),
                public: port_number(p.get("PublicPort")),
                ty: string(p.get("Type")),
            })
            .collect();
    }
    if let Some(Value::Object(ns)) = v.get("NetworkSettings").and_then(|n| n.get("Networks")) {
        for (name, n) in ns {
            c.networks.insert(
                name.clone(),
                (
                    string(n.get("IPAddress")),
                    string(n.get("GlobalIPv6Address")),
                ),
            );
        }
    }
    c
}

/// Turns containers into a part, and says why it left out the containers it did. The part is
/// the same for the same containers, so that an unchanged one is seen to be.
pub(crate) fn translate(mut cs: Vec<Container>, opts: &DockerProvider) -> (Vec<u8>, Vec<String>) {
    let prefix = format!("{}.", opts.label_prefix);
    cs.sort_by(|a, b| a.name().cmp(b.name()));
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let mut problems = Vec::new();
    let mut taken = 0usize;
    for c in &cs {
        let mut problem = |msg: String| problems.push(format!("container {}: {msg}", c.name()));
        let enable = c.labels.get(&format!("{prefix}enable")).map(String::as_str);
        if enable == Some("false") || !opts.expose_all && enable != Some("true") {
            continue;
        }
        if !c.state.is_empty() && c.state != "running" {
            continue;
        }
        if c.status.contains("(unhealthy)") || c.status.contains("(health: starting)") {
            continue; // back once it is healthy
        }
        if taken == opts.max_containers {
            problem(format!(
                "left out: max_containers, {}, are taken",
                opts.max_containers
            ));
            continue;
        }
        let name = group_name(c, &prefix);
        if !labels::valid_name(&name) {
            problem(format!(
                "{name:?} is not a valid name for an upstream: set {prefix}name"
            ));
            continue;
        }
        let l = match parse_labels(c, &prefix) {
            Ok(l) => l,
            Err(e) => {
                problem(e);
                continue;
            }
        };
        let network = c
            .labels
            .get(&format!("{prefix}network"))
            .filter(|n| !n.is_empty())
            .unwrap_or(&opts.network);
        let addr = match address(c, l.port, network, opts.use_published_ports) {
            Ok(a) => a,
            Err(e) => {
                problem(e);
                continue;
            }
        };
        // The group's first container sets its labels; the group is made only once a
        // container is taken, so one left out leaves none.
        let set = groups.get(&name).map_or(&l, |g| &g.set);
        let mut svc_addrs: BTreeMap<String, String> = BTreeMap::new();
        for (key, s) in &set.services {
            if s.port == 0 {
                continue;
            }
            match address(c, s.port, network, opts.use_published_ports) {
                Ok(a) => {
                    svc_addrs.insert(key.clone(), a);
                }
                Err(e) => problem(format!("service {key}: {e}")),
            }
        }
        if svc_addrs.len() != set.ports() {
            continue;
        }
        let g = groups.entry(name).or_insert_with(|| Group {
            set: l,
            addrs: Vec::new(),
            svc_addrs: BTreeMap::new(),
        });
        g.addrs.push(addr);
        for (key, a) in svc_addrs {
            g.svc_addrs.entry(key).or_default().push(a);
        }
        taken = taken.saturating_add(1);
    }
    (labels::part(&groups), problems)
}

/// The name of `c`'s upstream: its name label, or its Compose project and service, or its own
/// name.
fn group_name(c: &Container, prefix: &str) -> String {
    if let Some(n) = c
        .labels
        .get(&format!("{prefix}name"))
        .filter(|n| !n.is_empty())
    {
        return n.clone();
    }
    let get = |k: &str| c.labels.get(k).map_or("", String::as_str);
    let (project, service) = (
        get("com.docker.compose.project"),
        get("com.docker.compose.service"),
    );
    if !project.is_empty() && !service.is_empty() {
        return format!("{project}-{service}");
    }
    c.name().to_owned()
}

/// Reads `c`'s labels under `prefix`.
fn parse_labels(c: &Container, prefix: &str) -> Result<labels::Set, String> {
    let kind = labels::Kind {
        label: "label",
        owner: "container",
    };
    let mut l = labels::parse(&c.labels, prefix, &["enable", "name", "network"], kind)?;
    if l.port == 0 {
        let mut ports: Vec<u16> = Vec::new();
        for p in &c.ports {
            if p.ty == "tcp" && p.private != 0 && !ports.contains(&p.private) {
                ports.push(p.private);
            }
        }
        match ports.as_slice() {
            [p] => l.port = *p,
            [] => return Err(format!("it exposes no TCP port: set {prefix}port")),
            _ => {
                ports.sort_unstable();
                let list: Vec<String> = ports.iter().map(u16::to_string).collect();
                return Err(format!(
                    "it exposes TCP ports [{}]: set {prefix}port",
                    list.join(" ")
                ));
            }
        }
    }
    Ok(l)
}

/// Go's `net.JoinHostPort`.
fn join_host_port(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Where `c`'s `port` is reached: its address on `network` (or its only network, or the first
/// by name), the host's loopback for a container on the host's network, or, with `published`,
/// the host port it is published on.
fn address(c: &Container, port: u16, network: &str, published: bool) -> Result<String, String> {
    if published {
        for p in &c.ports {
            if p.ty != "tcp" || p.private != port || p.public == 0 {
                continue;
            }
            let ip = match p.ip.as_str() {
                "" | "0.0.0.0" => "127.0.0.1",
                "::" => "::1",
                ip => ip,
            };
            return Ok(join_host_port(ip, p.public));
        }
        return Err(format!("port {port} is not published on the host"));
    }
    if c.network_mode == "host" {
        return Ok(join_host_port("127.0.0.1", port));
    }
    if !network.is_empty() {
        let Some((v4, v6)) = c.networks.get(network) else {
            return Err(format!("it is not on network {network:?}"));
        };
        return join_ip(v4, v6, port, network);
    }
    c.networks
        .iter()
        .find_map(|(name, (v4, v6))| join_ip(v4, v6, port, name).ok())
        .ok_or_else(|| "it has no address on any network".to_owned())
}

fn join_ip(v4: &str, v6: &str, port: u16, network: &str) -> Result<String, String> {
    let ip = if v4.is_empty() { v6 } else { v4 };
    if ip.is_empty() {
        return Err(format!("it has no address on network {network:?}"));
    }
    Ok(join_host_port(ip, port))
}

#[cfg(test)]
mod tests;
