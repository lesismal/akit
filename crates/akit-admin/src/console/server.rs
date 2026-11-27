//! The console's server (I90), as fit's `app/console` and `admin/console.go`: akio-http on an
//! engine of its own, one I/O thread, as the Admin API's (decision 006), over TLS when
//! `cert_file` is set; its connections checked at accept against `admin.console.access`, then the
//! global access, set again by every config applied.
//!
//! ```text
//! POST /api/login                  {"username", "password"}: a session's cookie and
//!                                  {"user", "csrf"}; 401 for a wrong password, 429 for a
//!                                  lockout, with Retry-After and retry_after
//! POST /api/logout                 ends the session; 204
//! GET  /api/session                {"user", "csrf"}
//! GET  /api/workbench              {"user", "modules": ["admin", ...]}: the config's own module
//!                                  and the running roles' (I109)
//! GET  /api/node                   the Admin API's GET /node
//! GET  /api/m/admin/users          {"user", "store", "users"}
//! GET  /api/m/admin/schema         fit's config schema (I91)
//! POST /api/m/admin/account/password
//!                                  {"current_password", "password"}: the user's own password
//!                                  changed, kept in the store; 409 without one (I93b)
//! POST /api/m/admin/users/password {"username", "password"}: another user's password set,
//!                                  kept in the store; 409 without one (I93b)
//! *    /api/m/admin/<path>         the Admin API's endpoint at /<path>, called in process
//! *    /api/m/<role>/<path>        a running role's module (I109), as `tunnel.hub`: its pages'
//!                                  API, at /api/<path>
//! GET  /<file>                     fit's workbench, its files as fit ships them (I91)
//! ```
//!
//! A role's own `console` (I109, [`super::roles`]) is the same server with the role's module in
//! place of the Admin API's: at `/api/<path>` and `/api/m/<role>/<path>`, its cookie
//! `akit_<role>_<port>` (`akit_tunnel_hub_7100`), its bodies 1 MiB at most, as fit's.
//!
//! Every request but `POST /api/login` and `/api/logout` needs a session; every one but a read
//! carries its CSRF token in `X-Akit-Csrf`; one whose `Origin` is not the console's own is
//! refused. A session whose password is temporary (`akit run -admin`, I93c) reads who it is and
//! changes the password, and does nothing else until it has; past its time, the temporary
//! password logs in no more. Answers carry fit's headers: a Content-Security-Policy of the pages' own files,
//! `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, `nosniff`, HSTS over TLS, and
//! `Cache-Control: no-store` on the API. A password is checked on the console's login pool (one
//! thread, 16 waiting), never on the I/O thread; the Admin API's changes go to the control thread
//! as its own do.
//!
//! Its connections are fit's (`limitHandler`, on akio's connection handles and accept hooks,
//! A232): one that has not sent a request with a valid session (a login that succeeds counts) is
//! closed [`UNAUTH_TIMEOUT`] after it was accepted, whatever it sends meanwhile, and kept that long
//! between requests; one that has is kept [`IDLE_TIMEOUT`] between requests, until it logs out or
//! its session ends, when it is back on the first rule. A request being served when its
//! connection's time runs out is answered, with `Connection: close`, within as long again. Past
//! `max_conns`, or past the share of it unauthenticated connections may hold, a new connection
//! closes the oldest unauthenticated one of the client holding the most ([`super::slots`]);
//! akio's head (10 s) and body (30 s) timeouts bound every request.

// A refusal is the answer itself, made once per request: its size does not matter.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use akio::{AnyAcceptor, Config as EngineConfig, ConnHandle, Engine, Listener, Pool, PoolConfig};
use akio_http::{BodyMode, Context, Detached, HttpConfig, Method, Request, Response};
use akit_config::value::{Value, write_string};
use akit_observe::logger::{Level, Logger};
use akit_roles::pages::{ModuleRef, Modules};

use super::login::{self, Logins, Options as LoginOptions, Session};
use super::slots::Slots;
use crate::certs::Reloader;
use crate::server::{Options, Shared, VERSION_HEADER, dispatch, error, json, takes_config};

/// The header every request but a read carries its session's CSRF token in: fit's `X-Fit-Csrf`,
/// renamed (decision 037), as the pages send it.
pub const CSRF_HEADER: &str = "X-Akit-Csrf";

/// The session cookie's name before its port: fit's `fit_console`, renamed (decision 037).
pub const COOKIE: &str = "akit_console";

/// The pages' Content-Security-Policy: their own files and origin only, fit's `csp`.
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
     connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// The largest login body, whatever the console's bound: 4 KiB, as fit's.
pub const MAX_LOGIN_BYTES: usize = 4 << 10;

/// The largest body of a request other than a config: none is held past it.
const OTHER_BODY_BYTES: usize = 64 << 10;

/// The largest body of a request to a role's console: fit's `MaxBodyBytes`, 1 MiB.
pub const ROLE_BODY_BYTES: usize = 1 << 20;

/// How long a connection with a valid session may go between requests: fit's `IdleTimeout`.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(2 * 60);

/// How long after it was accepted a connection without a valid session is closed, whatever it
/// sends meanwhile, and how long it may go between requests: fit's `UnauthIdleTimeout`.
pub const UNAUTH_TIMEOUT: Duration = Duration::from_secs(5);

/// The login pool: one check at a time, 16 waiting, then 503, as fit's.
const LOGIN_WORKERS: usize = 1;
const LOGIN_QUEUE: usize = 16;

/// The module the console serves: the config's own, fit's `AdminModule`.
pub const ADMIN_MODULE: &str = "admin";

/// What a password change answers without a store: fit's `errNoConsoleStore`, in akit's words.
pub(crate) const NO_STORE: &str = "the console users are not saved: akit has no admin.store; \
     passwords change only with a store";

/// What a temporary password past its time is told: fit's, in akit's words.
const EXPIRED: &str =
    "has expired without being changed; restart akit with -admin to set a new one in time";

/// The requests a session whose password is temporary may make, beside `GET /api/session` and
/// `/api/workbench`: fit's `PasswordRoutes`.
const PASSWORD_ROUTES: [(Method, &str); 2] = [
    (Method::Get, "/api/m/admin/users"),
    (Method::Post, "/api/m/admin/account/password"),
];

/// The console, served.
pub(crate) struct Console {
    engine: Engine,
    addr: SocketAddr,
    secure: bool,
    certs: Option<Reloader>,
}

impl std::fmt::Debug for Console {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Console")
            .field("addr", &self.addr)
            .field("secure", &self.secure)
            .finish_non_exhaustive()
    }
}

/// What a console serves beside its logins and files.
enum Kind {
    /// `admin.console`: the Admin API's endpoints, its configs applied as the console's, as the
    /// config's own module (`admin`), and the running roles' modules beside it (I109).
    Admin(Arc<Shared>, Option<Modules>),
    /// A role's own console (I109): the role's module, at `/api/` and at `/api/m/<id>/`, as
    /// fit's role consoles serve it.
    Role(ModuleRef),
}

/// What every request of the console reads.
struct Pages {
    kind: Kind,
    logins: Arc<Logins>,
    /// The cookie's name before its port: [`COOKIE`], or `akit_<role>` (`akit_tunnel_hub`).
    cookie_prefix: String,
    /// `akit_console_<port>`, once the port is known.
    cookie: OnceLock<String>,
    secure: bool,
    login_pool: Arc<Pool>,
    conns: Arc<Conns>,
}

/// The console's connections, fit's `limitHandler`: counted by [`Slots`], each closed at its
/// deadline unless a request with a valid session comes first.
struct Conns {
    slots: Slots,
    /// The connections open, by id: when they close while they have not authenticated.
    open: Mutex<HashMap<u64, Instant>>,
    /// [`UNAUTH_TIMEOUT`], or a test's.
    unauth: Duration,
    /// How long past its time a connection is kept while a request is served on it: `unauth`,
    /// or a test's.
    grace: Duration,
}

impl Conns {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Instant>> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A connection accepted (akio's `on_accept`, on the I/O thread): counted, unauthenticated,
    /// and closed `unauth` from now unless it authenticates; closed at once when its client
    /// holds its share already or every connection has authenticated. It may close the oldest
    /// unauthenticated connection to take its slot.
    fn accepted(&self, h: &ConnHandle) {
        let evict = h.clone();
        if !self
            .slots
            .open(h.id(), h.peer_addr().ip(), move || evict.close())
        {
            h.close();
            return;
        }
        let until = after(Instant::now(), self.unauth);
        let _ = h.close_at(until);
        self.lock().insert(h.id(), until);
    }

    /// A connection closed (akio's `on_close`): its slot freed.
    fn closed(&self, h: &ConnHandle) {
        self.lock().remove(&h.id());
        self.slots.release(h.id());
    }

    /// A request begun on its connection (fit's `limitHandler.begin`): one without a valid
    /// session is not cut while it is served, for as long again as its time (fit's grace); its
    /// response says `Connection: close` when that time ran out meanwhile, and otherwise sets
    /// the deadline back.
    fn begin(self: &Arc<Self>, ctx: &mut Context) -> Option<ConnHandle> {
        let h = ctx.connection()?;
        let id = h.id();
        if !self.slots.authenticated(id) {
            if let Some(until) = self.lock().get(&id).copied() {
                let _ = h.close_at(after(until.max(Instant::now()), self.grace));
            }
        }
        let me = Arc::clone(self);
        let handle = h.clone();
        ctx.add_response_filter(move |resp, _| {
            if me.slots.authenticated(id) {
                return;
            }
            let Some(until) = me.lock().get(&id).copied() else {
                return;
            };
            if Instant::now() >= until {
                resp.headers.set("Connection", "close");
            } else {
                let _ = handle.close_at(until);
            }
        });
        Some(h)
    }

    /// Whether the request on `h` came with a valid session (fit's `Console.authenticated`): one
    /// that did keeps its connection from now on, [`IDLE_TIMEOUT`] between requests (akio-http's
    /// keep-alive timeout), and is no longer closed for a newer one; one that did not, on a
    /// connection a session had kept, puts it back on the rule for unauthenticated ones, closed
    /// `unauth` from now unless a valid session comes first.
    fn authenticated(&self, h: Option<&ConnHandle>, valid: bool) {
        let Some(h) = h else { return };
        let id = h.id();
        if valid {
            h.cancel_close();
            self.slots.authenticate(id);
            return;
        }
        if self.slots.unauthenticate(id) {
            let until = after(Instant::now(), self.unauth);
            let _ = h.close_at(until);
            if let Some(u) = self.lock().get_mut(&id) {
                *u = until;
            }
        }
    }
}

/// `d` after `at` (`at` itself past what an `Instant` holds, which a timeout never comes near).
fn after(at: Instant, d: Duration) -> Instant {
    at.checked_add(d).unwrap_or(at)
}

fn pool(threads: usize, queue: usize) -> Arc<Pool> {
    let mut cfg = PoolConfig::default();
    cfg.min_threads = 0;
    cfg.max_threads = threads;
    cfg.queue_capacity = queue;
    Pool::new(cfg)
}

/// `address` as a socket address: `localhost` is 127.0.0.1.
fn resolve(address: &str) -> io::Result<SocketAddr> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("console: {address:?} is not a host and port"),
        )
    };
    let (host, port) = address.rsplit_once(':').ok_or_else(invalid)?;
    let port: u16 = port.parse().map_err(|_| invalid())?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") {
        return Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port));
    }
    if host.is_empty() {
        return Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port));
    }
    let ip: IpAddr = host.parse().map_err(|_| invalid())?;
    Ok(SocketAddr::new(ip, port))
}

impl Console {
    /// Listens where `c` says and serves the console, calling the Admin API's endpoints through
    /// `api`. On the control plane: it reads the certificate's files, hashes the dummy password
    /// and binds a socket.
    pub(crate) fn start(
        c: &akit_config::Console,
        api: Shared,
        opts: &Options,
    ) -> io::Result<Console> {
        // A console started while a user's password is temporary asks for it to be changed.
        let temporary = api.configs.temporary_password();
        let modules = opts.modules.clone();
        Console::start_kind(
            c,
            Kind::Admin(Arc::new(api), modules),
            COOKIE.to_owned(),
            OTHER_BODY_BYTES,
            temporary,
            opts,
        )
    }

    /// Listens where `c`, a role's `console`, says and serves the role's pages: its `module`
    /// (I109). On the control plane, as [`Console::start`].
    pub(crate) fn start_role(
        c: &akit_config::Console,
        module: ModuleRef,
        opts: &Options,
    ) -> io::Result<Console> {
        let cookie = format!("akit_{}", module.id().replace('.', "_"));
        Console::start_kind(c, Kind::Role(module), cookie, ROLE_BODY_BYTES, None, opts)
    }

    fn start_kind(
        c: &akit_config::Console,
        kind: Kind,
        cookie_prefix: String,
        max_body: usize,
        temporary: Option<(String, std::time::SystemTime)>,
        opts: &Options,
    ) -> io::Result<Console> {
        let addr = resolve(&c.address)?;
        let secure = c.cert_file.is_some();
        let tls = match (&c.cert_file, &c.key_file) {
            (Some(cert), Some(key)) => Some(akit_config::Tls {
                certificates: vec![akit_config::Certificate {
                    cert_file: cert.clone(),
                    key_file: key.clone(),
                }],
                client_auth: None,
                max_version: akit_config::TlsVersion::Tls13,
                acme: Vec::new(),
                on_demand: false,
            }),
            _ => None,
        };
        let conns = Arc::new(Conns {
            slots: Slots::new(c.max_conns),
            open: Mutex::new(HashMap::new()),
            unauth: opts.console_unauth_timeout,
            grace: opts.console_grace.unwrap_or(opts.console_unauth_timeout),
        });
        let logins = Logins::new(&LoginOptions::of(c));
        if let Some((user, until)) = temporary {
            logins.set_temporary(&user, until);
        }
        let pages = Arc::new(Pages {
            kind,
            logins: Arc::new(logins),
            cookie_prefix,
            cookie: OnceLock::new(),
            secure,
            login_pool: pool(LOGIN_WORKERS, LOGIN_QUEUE),
            conns: Arc::clone(&conns),
        });
        let mut http = HttpConfig::default();
        // HTTP/1.1 alone, as the Admin API: one request at a time on each connection max_conns
        // counts.
        http.h2 = None;
        http.max_body_bytes = max_body;
        http.head_timeout = Some(Duration::from_secs(10));
        http.body_timeout = Some(Duration::from_secs(30));
        // How long a connection is kept between requests; one without a valid session is closed
        // at its deadline (`Conns`) before.
        http.keep_alive_timeout = Some(opts.console_idle_timeout);
        let mut cfg = EngineConfig::default();
        // `max_conns` is the slots' (`Conns`), which close an older connection for a new one;
        // the engine's bound leaves room for those being closed.
        cfg.max_connections = c.max_conns.saturating_mul(2);
        cfg.shutdown_timeout = Duration::from_secs(1);
        let service = akio_http::serve_with(http, Service(Arc::clone(&pages)));
        // The certificate's files read again when they change (I94).
        let (acceptor, certs) = match tls {
            Some(t) => {
                // What its messages start with: `console` for admin.console's, the role's ID
                // for a role's (I109b).
                let name = match &pages.kind {
                    Kind::Admin(..) => "console",
                    Kind::Role(m) => m.id(),
                };
                let (r, swap) = Reloader::start(name, t, service)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
                (AnyAcceptor::new(swap), Some(r))
            }
            None => (AnyAcceptor::new(service), None),
        };
        let listener = match &opts.console_access {
            // Refused at accept, before anything is read (fit's Binding.Filter).
            Some(p) => Listener::tcp(addr).filter(super::accept_filter(p)),
            None => Listener::tcp(addr),
        };
        // Counted as they are accepted, before TLS (akio's A232).
        let (a, b) = (Arc::clone(&conns), conns);
        let listener = listener
            .on_accept(move |h| a.accepted(h))
            .on_close(move |h| b.closed(h));
        let engine = Engine::builder()
            .config(cfg)
            .listen(
                listener
                    .backend(opts.backend.clone())
                    .threads(1)
                    .serve_any(acceptor),
            )
            .build()
            .map_err(|e| io::Error::new(e.kind(), format!("console: {}: {e}", c.address)))?;
        engine
            .start()
            .map_err(|e| io::Error::new(e.kind(), format!("console: {}: {e}", c.address)))?;
        let addr = engine.local_addrs().first().copied().unwrap_or(addr);
        let _ = pages
            .cookie
            .set(login::cookie_name(&pages.cookie_prefix, addr.port()));
        Ok(Console {
            engine,
            addr,
            secure,
            certs,
        })
    }

    /// The certificate's files, when it serves TLS (I94).
    pub(crate) fn certs(&self) -> Option<&Reloader> {
        self.certs.as_ref()
    }

    /// Has the console serve the clients `access` serves from now on: a connection already open
    /// is not checked again.
    pub(crate) fn set_access(&self, access: Option<&Arc<akit_access::Policy>>) {
        let Some(id) = self.engine.listeners().first().map(|l| l.id) else {
            return;
        };
        // Only `NotFound` (the console shut down) can come back.
        let _ = match access {
            Some(p) => self.engine.set_listener_filter(id, super::accept_filter(p)),
            None => self.engine.clear_listener_filter(id),
        };
    }

    /// Where the console listens.
    pub(crate) fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Where the console listens, as a URL.
    pub(crate) fn url(&self) -> String {
        let scheme = if self.secure { "https" } else { "http" };
        format!("{scheme}://{}/", self.addr)
    }

    /// Stops the console: no more connections, those open closed.
    pub(crate) fn shutdown(&self) {
        self.engine.shutdown();
        self.engine.wait();
    }
}

struct Service(Arc<Pages>);

/// The Admin API's path a console path calls: `/api/m/admin/<p>` is `/<p>`.
fn admin_path(path: &str) -> Option<&str> {
    path.strip_prefix("/api/m/admin")
        .filter(|rest| rest.starts_with('/'))
}

impl akio_http::Service for Service {
    fn body_mode(&self, req: &Request) -> BodyMode {
        if let Kind::Role(_) = self.0.kind {
            return BodyMode::Buffered;
        }
        // A config's body streams in within max_config_bytes, as the Admin API's does.
        match admin_path(req.path()) {
            Some(p) if takes_config(&req.method, p) => BodyMode::Stream,
            _ => BodyMode::Buffered,
        }
    }

    fn call(&self, ctx: &mut Context) {
        self.0.serve(ctx);
    }
}

/// The value of cookie `name` in `header`, a `Cookie` field.
fn cookie<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header
        .split(';')
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// The client's address as the logins count it.
fn client(ctx: &Context) -> String {
    ctx.peer().ip().to_string()
}

fn log(level: Level, args: std::fmt::Arguments<'_>) {
    akit_observe::logger::log(Logger::Admin, level, args);
}

/// A string as JSON.
fn quoted(s: &str) -> String {
    let mut b = String::new();
    write_string(&mut b, s);
    b
}

/// `t` as RFC 3339 in UTC, as Go's `time.RFC3339`.
fn rfc3339(t: std::time::SystemTime) -> String {
    let secs = t
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    akit_roles::linktoken::rfc3339(secs)
}

/// `{"user", "csrf"}`, a session as fit's `sessionBody` writes it; with
/// `"must_change_password": true` and `password_expires` for a session whose password is
/// temporary.
fn session_body(s: &Session) -> String {
    let mut b = format!(
        "{{\"user\":{},\"csrf\":{}",
        quoted(&s.user),
        quoted(&s.csrf)
    );
    if s.must_change {
        b.push_str(",\"must_change_password\":true");
        if let Some(t) = s.expires {
            b.push_str(",\"password_expires\":");
            b.push_str(&quoted(&rfc3339(t)));
        }
    }
    b.push_str("}\n");
    b
}

/// A JSON body's string member `k`; `None` when it is not one.
fn field(v: &Value, k: &str) -> Option<String> {
    match v.get(k) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// The JSON object a console request sent as `application/json`, as fit's `Request.Decode`:
/// 415 for another type, 400 for a body that is not an object.
fn decoded(ctx: &Context) -> Result<Value, Response> {
    let json_body = ctx.header("Content-Type").is_some_and(|t| {
        t.split(';')
            .next()
            .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"))
    });
    if !json_body {
        return Err(error(415, "the body must be sent as application/json"));
    }
    let not_json = |why: &str| error(400, &format!("the body is not the JSON expected: {why}"));
    let text = std::str::from_utf8(ctx.body()).map_err(|_| not_json("not UTF-8"))?;
    match akit_config::value::parse(text) {
        Ok(v @ Value::Object(_)) => Ok(v),
        Ok(_) => Err(not_json("not an object")),
        Err(e) => Err(not_json(
            &e.as_slice()
                .first()
                .map_or_else(String::new, |e| e.msg.clone()),
        )),
    }
}

/// Why `password` cannot be a console password, or `None` when it can: bcrypt takes 72 bytes at
/// most. fit's `passwordProblem`.
pub(crate) fn password_problem(password: &str) -> Option<&'static str> {
    if password.is_empty() {
        Some("the new password is empty")
    } else if password.len() > 72 {
        Some("the new password is longer than bcrypt's 72 bytes")
    } else {
        None
    }
}

/// A password check refused before it is made: 429, as fit's `refuseChecks`, with how long to
/// wait in `Retry-After` and `retry_after`.
fn refused_checks(r: &login::Refusal) -> Response {
    let msg = if r.busy {
        "another password check from this address is under way; try again"
    } else if r.slowed {
        "too many failed password checks of this user; try again later"
    } else {
        "too many failed password checks from this address; try again later"
    };
    retry(r.wait, msg)
}

/// `{"changed": true, ...}`, a password change's answer: `user` or `username` its member.
fn changed(member: &str, who: &str) -> Response {
    json(
        200,
        format!("{{\"changed\":true,\"{member}\":{}}}\n", quoted(who)),
    )
}

/// Answers `d` with work run on `pool`: 503 at once when the pool's queue is full.
fn run_on(pool: &Pool, d: Detached, work: impl FnOnce(Detached) + Send + 'static) {
    let cell = Arc::new(Mutex::new(Some(d)));
    let taken = Arc::clone(&cell);
    let job = Box::new(move || {
        let d = taken.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(d) = d {
            work(d);
        }
    });
    if pool.try_execute(job).is_err() {
        let d = cell.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(d) = d {
            d.respond(error(503, "the console is busy; try again"));
        }
    }
}

impl Pages {
    /// Whether a store keeps the console users: `admin.console`'s, with a store.
    fn users_stored(&self) -> bool {
        match &self.kind {
            Kind::Admin(api, _) => api.configs.users_stored(),
            Kind::Role(_) => false,
        }
    }

    /// The IDs of the modules the console serves, for `GET /api/workbench`.
    fn module_ids(&self) -> Vec<&'static str> {
        match &self.kind {
            Kind::Admin(_, ms) => {
                let mut ids = vec![ADMIN_MODULE];
                if let Some(ms) = ms {
                    ids.extend(ms.ids());
                }
                ids
            }
            Kind::Role(m) => vec![m.id()],
        }
    }

    /// The module `id`, other than `admin`, the console serves.
    fn module(&self, id: &str) -> Option<ModuleRef> {
        match &self.kind {
            Kind::Admin(_, ms) => ms.as_ref().and_then(|ms| ms.get(id)),
            Kind::Role(m) => (m.id() == id).then(|| Arc::clone(m)),
        }
    }

    fn cookie(&self) -> &str {
        self.cookie
            .get()
            .map_or(&self.cookie_prefix, String::as_str)
    }

    fn origin(&self, ctx: &Context) -> String {
        let scheme = if self.secure { "https" } else { "http" };
        format!("{scheme}://{}", ctx.header("Host").unwrap_or(""))
    }

    /// The `Set-Cookie` field of `token`, or, empty, the one that ends it.
    fn set_cookie(&self, token: &str) -> String {
        let mut c = format!(
            "{}={token}; Path=/; HttpOnly; SameSite=Strict",
            self.cookie()
        );
        if token.is_empty() {
            c.push_str("; Max-Age=0");
        }
        if self.secure {
            c.push_str("; Secure");
        }
        c
    }

    fn serve(self: &Arc<Self>, ctx: &mut Context) {
        let conn = self.conns.begin(ctx);
        ctx.add_response_header("Content-Security-Policy", CSP);
        ctx.add_response_header("X-Content-Type-Options", "nosniff");
        ctx.add_response_header("X-Frame-Options", "DENY");
        ctx.add_response_header("Referrer-Policy", "no-referrer");
        if self.secure {
            ctx.add_response_header("Strict-Transport-Security", "max-age=31536000");
        }
        if let Some(o) = ctx.header("Origin") {
            if o != self.origin(ctx) {
                return ctx.respond(error(403, "requests from other web pages are refused"));
            }
        }
        let path = ctx.path().to_owned();
        if !path.starts_with("/api/") {
            ctx.add_response_header("Cache-Control", "no-cache");
            return ctx.respond(super::pages::serve(ctx.method(), &path));
        }
        ctx.add_response_header("Cache-Control", "no-store");
        let method = ctx.method().clone();
        match (&method, path.as_str()) {
            (Method::Post, "/api/login") => return self.login(ctx, conn),
            (Method::Post, "/api/logout") => return self.logout(ctx, conn.as_ref()),
            _ => {}
        }
        let token = ctx
            .header("Cookie")
            .and_then(|h| cookie(h, self.cookie()))
            .unwrap_or("")
            .to_owned();
        let Some(s) = self.logins.lookup(&token) else {
            self.conns.authenticated(conn.as_ref(), false);
            return ctx.respond(error(401, "log in first"));
        };
        let read = matches!(method, Method::Get | Method::Head);
        if !read
            && !ctx
                .header(CSRF_HEADER)
                .is_some_and(|got| login::csrf_ok(&s, got))
        {
            return ctx.respond(error(
                403,
                &format!("the request lacks the session's {CSRF_HEADER} token"),
            ));
        }
        self.conns.authenticated(conn.as_ref(), true);
        if s.must_change && !password_route(&method, &path) {
            return ctx.respond(json(
                403,
                "{\"error\":\"change the temporary password first\",\"must_change_password\":true}\n"
                    .to_owned(),
            ));
        }
        if method == Method::Get {
            match path.as_str() {
                "/api/session" => return ctx.respond(json(200, session_body(&s))),
                "/api/workbench" => {
                    let ids: Vec<String> = self.module_ids().iter().map(|i| quoted(i)).collect();
                    return ctx.respond(json(
                        200,
                        format!(
                            "{{\"user\":{},\"modules\":[{}]}}\n",
                            quoted(&s.user),
                            ids.join(",")
                        ),
                    ));
                }
                _ => {}
            }
        }
        // A role's module (I109): at /api/m/<id>/, and on its own console at /api/ too.
        if let Some(rest) = path.strip_prefix("/api/m/") {
            let (id, sub) = rest.split_once('/').unwrap_or((rest, ""));
            if id != ADMIN_MODULE {
                return match self.module(id) {
                    Some(m) => m.serve(ctx, &s.user, &format!("/api/{sub}")),
                    None => ctx.respond(error(404, &format!("no module {} runs here", quoted(id)))),
                };
            }
        }
        if let Kind::Role(m) = &self.kind {
            if path.starts_with("/api/m/") {
                return ctx.respond(error(
                    404,
                    &format!("no module {} runs here", quoted(ADMIN_MODULE)),
                ));
            }
            return m.serve(ctx, &s.user, &path);
        }
        // GET /api/node: the Admin API's, beside /api/m/admin/node.
        let target = if path == "/api/node" {
            Some("/node".to_owned())
        } else {
            admin_path(&path).map(str::to_owned)
        };
        let Some(sub) = target else {
            let msg = match path.strip_prefix("/api/m/") {
                Some(rest) => format!(
                    "no module {} runs here",
                    quoted(rest.split('/').next().unwrap_or(""))
                ),
                None => "no such endpoint".to_owned(),
            };
            return ctx.respond(error(404, &msg));
        };
        match (&method, sub.as_str()) {
            (Method::Get, "/users") => return ctx.respond(self.users(&s)),
            // fit's config schema, which the pages' forms are made from (I91).
            (Method::Get | Method::Head, "/schema") => {
                return ctx.respond(Response::with_body(
                    200,
                    "application/schema+json",
                    super::pages::SCHEMA.to_vec(),
                ));
            }
            (Method::Post, "/account/password") => return self.account_password(ctx, &s, &token),
            (Method::Post, "/users/password") => return self.user_password(ctx, &s),
            _ => {}
        }
        self.call_api(ctx, &s.user, &sub);
    }

    /// Calls the Admin API's endpoint at `sub` with the request, in process, behind the
    /// session instead of the token; says what a user changed.
    fn call_api(&self, ctx: &mut Context, user: &str, sub: &str) {
        let method = ctx.method().clone();
        let query = ctx.req().query().map(str::to_owned);
        if !matches!(method, Method::Get | Method::Head) {
            let dry = query.as_deref().is_some_and(|q| {
                q.split('&')
                    .any(|kv| kv == "dry_run=1" || kv == "dry_run=true")
            });
            let (user, what) = (user.to_owned(), format!("{} {sub}", method.as_str()));
            // Logged as the answer goes, so that whoever has it finds the change in the log.
            ctx.add_response_filter(move |resp, _| {
                let status = resp.status;
                let version = resp
                    .headers
                    .get(VERSION_HEADER)
                    .map(str::to_owned)
                    .filter(|_| status < 300);
                match (dry, version) {
                    (true, _) => log(
                        Level::Info,
                        format_args!(
                            "console: {user:?}: {what}?dry_run=1: {status}, checked and not applied"
                        ),
                    ),
                    (false, Some(v)) => log(
                        Level::Info,
                        format_args!("console: {user:?}: {what} applied config version {v}"),
                    ),
                    (false, None) => {
                        log(
                            Level::Info,
                            format_args!("console: {user:?}: {what}: {status}"),
                        );
                    }
                }
            });
        }
        if let Some(req) = ctx.req_mut() {
            req.target = match query {
                Some(q) => format!("{sub}?{q}"),
                None => sub.to_owned(),
            };
        }
        if let Kind::Admin(api, _) = &self.kind {
            dispatch(api, ctx);
        }
    }

    /// `GET /api/m/admin/users`: who is logged in, whether a store keeps the passwords, and
    /// every user who may log in.
    fn users(&self, s: &Session) -> Response {
        let mut b = format!(
            "{{\"user\":{},\"store\":{},\"users\":[",
            quoted(&s.user),
            self.users_stored()
        );
        for (i, u) in self.logins.users().iter().enumerate() {
            if i > 0 {
                b.push(',');
            }
            b.push_str(&quoted(u));
        }
        b.push_str("]}\n");
        json(200, b)
    }

    /// `POST /api/login`: checked on the login pool, one from an address at a time, not while
    /// it is locked out or its user slowed.
    fn login(self: &Arc<Self>, ctx: &mut Context, conn: Option<ConnHandle>) {
        let json_body = ctx.header("Content-Type").is_some_and(|t| {
            t.split(';')
                .next()
                .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"))
        });
        if !json_body {
            return ctx.respond(error(415, "the body must be sent as application/json"));
        }
        if ctx.body().len() > MAX_LOGIN_BYTES {
            return ctx.respond(error(413, "a login's body is 4 KiB at most"));
        }
        let parsed = std::str::from_utf8(ctx.body())
            .ok()
            .and_then(|t| akit_config::value::parse(t).ok());
        let Some((user, password)) = parsed
            .as_ref()
            .and_then(|v| Some((field(v, "username")?, field(v, "password")?)))
        else {
            return ctx.respond(error(400, "the body must be {\"username\", \"password\"}"));
        };
        let addr = client(ctx);
        let ticket = match self.logins.admit_user(&addr, &user) {
            Ok(t) => t,
            Err(r) => return ctx.respond(refused(r)),
        };
        let Some(d) = ctx.detach() else { return };
        let me = Arc::clone(self);
        run_on(&self.login_pool, d, move |d| {
            let wait = ticket.locked_out();
            if !wait.is_zero() {
                return d.respond(refused(login::Refusal {
                    wait,
                    busy: false,
                    slowed: false,
                }));
            }
            let ok = me.logins.check(&user, &password);
            if !ok {
                ticket.done(true);
                log(
                    Level::Warn,
                    format_args!("console: a failed login as {user:?} from {addr}"),
                );
                return d.respond(error(401, "the user name or password is wrong"));
            }
            ticket.passed();
            if me.logins.expired(&user) {
                log(
                    Level::Warn,
                    format_args!(
                        "console: a login as {user:?} from {addr} with its temporary password, which has expired"
                    ),
                );
                let mut b = String::from("{\"error\":");
                write_string(
                    &mut b,
                    &format!("the temporary password of {} {EXPIRED}", quoted(&user)),
                );
                b.push_str(",\"password_expired\":true}\n");
                return d.respond(json(401, b));
            }
            let Some((token, s)) = me.logins.start(&user) else {
                return d.respond(error(500, "no randomness for a session"));
            };
            me.conns.authenticated(conn.as_ref(), true);
            log(
                Level::Info,
                format_args!("console: {user:?} logged in from {addr}"),
            );
            d.respond(json(200, session_body(&s)).header("Set-Cookie", me.set_cookie(&token)));
        });
    }

    /// Whether a password may change now, from `addr`: 429 while a lockout holds the address,
    /// 409 when no store keeps the console users. fit's `passwordReady`.
    fn password_ready(&self, addr: &str) -> Result<(), Response> {
        let wait = self.logins.locked_out(addr);
        if !wait.is_zero() {
            return Err(refused_checks(&login::Refusal {
                wait,
                busy: false,
                slowed: false,
            }));
        }
        if !self.users_stored() {
            return Err(error(409, NO_STORE));
        }
        Ok(())
    }

    /// `POST /api/m/admin/account/password`: a user changes its own password, giving the current
    /// one, checked as a login's is (one from an address at a time, each failure counted). The
    /// new one is hashed and saved to the store on the login pool, since bcrypt and the disk
    /// block; the user's other sessions end. fit's `accountPassword`.
    fn account_password(self: &Arc<Self>, ctx: &mut Context, s: &Session, token: &str) {
        let body = match decoded(ctx) {
            Ok(b) => b,
            Err(r) => return ctx.respond(r),
        };
        let current = field(&body, "current_password").unwrap_or_default();
        let password = field(&body, "password").unwrap_or_default();
        if current.is_empty() {
            return ctx.respond(error(
                400,
                "the body must hold the current password and the new password",
            ));
        }
        if let Some(msg) = password_problem(&password) {
            return ctx.respond(error(400, msg));
        }
        if self.logins.temporary(&s.user).is_some() {
            if self.logins.expired(&s.user) {
                return ctx.respond(error(403, &format!("the temporary password {EXPIRED}")));
            }
            if password == current {
                return ctx.respond(error(400, "the new password must not be the temporary one"));
            }
        }
        let addr = client(ctx);
        if let Err(r) = self.password_ready(&addr) {
            return ctx.respond(r);
        }
        let ticket = match self.logins.admit_user(&addr, &s.user) {
            Ok(t) => t,
            Err(r) => return ctx.respond(refused_checks(&r)),
        };
        let Some(d) = ctx.detach() else { return };
        let me = Arc::clone(self);
        let (user, token) = (s.user.clone(), token.to_owned());
        run_on(&self.login_pool, d, move |d| {
            let wait = ticket.locked_out();
            if !wait.is_zero() {
                return d.respond(refused_checks(&login::Refusal {
                    wait,
                    busy: false,
                    slowed: false,
                }));
            }
            if me.logins.check(&user, &current) {
                ticket.passed();
            } else {
                ticket.done(true);
                let wait = me.logins.locked_out(&addr);
                if !wait.is_zero() {
                    return d.respond(refused_checks(&login::Refusal {
                        wait,
                        busy: false,
                        slowed: false,
                    }));
                }
                return d.respond(error(403, "the current password is wrong"));
            }
            match me.save_password(&user, &password) {
                Ok(()) => {
                    me.logins.end_user_sessions(&user, &token);
                    log(
                        Level::Info,
                        format_args!("console: {user:?} changed the password of {user:?}"),
                    );
                    d.respond(changed("user", &user));
                }
                Err(r) => d.respond(r),
            }
        });
    }

    /// `POST /api/m/admin/users/password`: a console user sets another user's password; every
    /// session of the other user ends. All console users are admins: no role is asked for.
    /// fit's `userPassword`.
    fn user_password(self: &Arc<Self>, ctx: &mut Context, s: &Session) {
        let body = match decoded(ctx) {
            Ok(b) => b,
            Err(r) => return ctx.respond(r),
        };
        let target = field(&body, "username").unwrap_or_default();
        let password = field(&body, "password").unwrap_or_default();
        if target.is_empty() {
            return ctx.respond(error(
                400,
                "the body must hold the user name and the new password",
            ));
        }
        if let Some(msg) = password_problem(&password) {
            return ctx.respond(error(400, msg));
        }
        if !self.logins.has_user(&target) {
            return ctx.respond(error(
                400,
                &format!("there is no console user named {}", quoted(&target)),
            ));
        }
        if let Err(r) = self.password_ready(&client(ctx)) {
            return ctx.respond(r);
        }
        let Some(d) = ctx.detach() else { return };
        let me = Arc::clone(self);
        let caller = s.user.clone();
        run_on(&self.login_pool, d, move |d| {
            match me.save_password(&target, &password) {
                Ok(()) => {
                    me.logins.end_user_sessions(&target, "");
                    log(
                        Level::Info,
                        format_args!("console: {caller:?} set the password of {target:?}"),
                    );
                    d.respond(changed("username", &target));
                }
                Err(r) => d.respond(r),
            }
        });
    }

    /// Hashes `password` and keeps it as `user`'s, in the store, then in the logins: on the
    /// login pool. A save that fails changes nothing.
    fn save_password(&self, user: &str, password: &str) -> Result<(), Response> {
        // password_problem kept it to bcrypt's 72 bytes.
        let hash =
            akio_crypto::bcrypt::hash(password.as_bytes(), akio_crypto::bcrypt::DEFAULT_COST)
                .map_err(|e| error(500, &format!("hashing the password: {e}")))?;
        let Kind::Admin(api, _) = &self.kind else {
            return Err(error(409, NO_STORE));
        };
        api.configs
            .set_console_password(user, &hash)
            .map_err(|e| error(500, &format!("saving the password: {e}")))?;
        self.logins.set_password(user, &hash);
        Ok(())
    }

    /// `POST /api/logout`: the session ends, its cookie with it; 204.
    fn logout(&self, ctx: &mut Context, conn: Option<&ConnHandle>) {
        let token = ctx
            .header("Cookie")
            .and_then(|h| cookie(h, self.cookie()))
            .unwrap_or("")
            .to_owned();
        if let Some(s) = self.logins.end(&token) {
            log(
                Level::Info,
                format_args!("console: {:?} logged out", s.user),
            );
        }
        // Logged out, the connection is an unauthenticated one again.
        self.conns.authenticated(conn, false);
        ctx.respond(Response::new(204).header("Set-Cookie", self.set_cookie("")));
    }
}

/// A login refused before its check: 429, with how long to wait in `Retry-After` and in the
/// body's `retry_after`, its whole seconds and one more, as fit's `Refuse`.
fn refused(r: login::Refusal) -> Response {
    let msg = if r.busy {
        "another login from this address is being checked; try again"
    } else if r.slowed {
        "too many failed logins as this user; try again later"
    } else {
        "too many failed logins from this address; try again later"
    };
    retry(r.wait, msg)
}

/// 429 with `msg`, and how long to wait, `wait`'s whole seconds and one more, in `Retry-After`
/// and the body's `retry_after`, as fit's `Refuse`.
fn retry(wait: Duration, msg: &str) -> Response {
    let secs = wait.as_secs().saturating_add(1);
    let mut b = String::from("{\"error\":");
    write_string(&mut b, msg);
    let _ = writeln!(b, ",\"retry_after\":{secs}}}");
    json(429, b).header("Retry-After", secs.to_string())
}

/// Whether a session whose password is temporary may make `method` on `path`.
fn password_route(method: &Method, path: &str) -> bool {
    (*method == Method::Get && (path == "/api/session" || path == "/api/workbench"))
        || PASSWORD_ROUTES
            .iter()
            .any(|(m, p)| m == method && *p == path)
}
