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
//! GET  /api/workbench              {"user", "modules": ["admin"]}
//! GET  /api/node                   the Admin API's GET /node
//! GET  /api/m/admin/users          {"user", "store", "users"}
//! GET  /api/m/admin/schema         fit's config schema (I91)
//! POST /api/m/admin/account/password, /api/m/admin/users/password
//!                                  a password changed: 409 without admin.store (I93)
//! *    /api/m/admin/<path>         the Admin API's endpoint at /<path>, called in process
//! GET  /<file>                     fit's workbench, its files as fit ships them (I91)
//! ```
//!
//! Every request but `POST /api/login` and `/api/logout` needs a session; every one but a read
//! carries its CSRF token in `X-Fit-Csrf`; one whose `Origin` is not the console's own is
//! refused. Answers carry fit's headers: a Content-Security-Policy of the pages' own files,
//! `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, `nosniff`, HSTS over TLS, and
//! `Cache-Control: no-store` on the API. A password is checked on the console's login pool (one
//! thread, 16 waiting), never on the I/O thread; the Admin API's changes go to the control thread
//! as its own do.
//!
//! fit closes a connection without a login 5 s after it was accepted, and past `max_conns` closes
//! the oldest such connection to make room; akio-http does not let a handler do either (akio's
//! A232). Meanwhile the console's connections are bounded as the Admin API's: `max_conns` at
//! accept, akio's head (10 s), body (30 s) and idle (2 min) timeouts, and an answer to a request
//! without a valid session closes its connection.

use std::fmt::Write as _;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use akio::tls::Tls;
use akio::{AnyAcceptor, Config as EngineConfig, Engine, Listener, Pool, PoolConfig};
use akio_http::{BodyMode, Context, Detached, HttpConfig, Method, Request, Response};
use akit_config::value::{Value, write_string};
use akit_observe::logger::{Level, Logger};

use super::login::{self, Logins, Options as LoginOptions, Session};
use crate::server::{Options, Shared, VERSION_HEADER, dispatch, error, json, takes_config};

/// The header every request but a read carries its session's CSRF token in: fit's.
pub const CSRF_HEADER: &str = "X-Fit-Csrf";

/// The session cookie's name before its port: fit's, so that fit's scripts work unchanged.
pub const COOKIE: &str = "fit_console";

/// The pages' Content-Security-Policy: their own files and origin only, fit's `csp`.
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
     connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// The largest login body, whatever the console's bound: 4 KiB, as fit's.
pub const MAX_LOGIN_BYTES: usize = 4 << 10;

/// The largest body of a request other than a config: none is held past it.
const OTHER_BODY_BYTES: usize = 64 << 10;

/// How long a connection may go between requests: fit's `IdleTimeout`.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(2 * 60);

/// The login pool: one check at a time, 16 waiting, then 503, as fit's.
const LOGIN_WORKERS: usize = 1;
const LOGIN_QUEUE: usize = 16;

/// The module the console serves: the config's own, fit's `AdminModule`.
pub const ADMIN_MODULE: &str = "admin";

/// What a password change answers without a store: fit's `errNoConsoleStore`, in akit's words.
const NO_STORE: &str = "the console users are not saved: akit has no admin.store; passwords \
     change only with a store";

/// The console, served.
pub(crate) struct Console {
    engine: Engine,
    addr: SocketAddr,
    secure: bool,
}

impl std::fmt::Debug for Console {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Console")
            .field("addr", &self.addr)
            .field("secure", &self.secure)
            .finish_non_exhaustive()
    }
}

/// What every request of the console reads.
struct Pages {
    /// The Admin API's endpoints, its configs applied as the console's.
    api: Arc<Shared>,
    logins: Arc<Logins>,
    /// `fit_console_<port>`, once the port is known.
    cookie: OnceLock<String>,
    secure: bool,
    login_pool: Arc<Pool>,
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
        let addr = resolve(&c.address)?;
        let secure = c.cert_file.is_some();
        let tls = match (&c.cert_file, &c.key_file) {
            (Some(cert), Some(key)) => Some(
                akit_tls::server_config(
                    &akit_config::Tls {
                        certificates: vec![akit_config::Certificate {
                            cert_file: cert.clone(),
                            key_file: key.clone(),
                        }],
                        client_auth: None,
                        max_version: akit_config::TlsVersion::Tls13,
                        acme: Vec::new(),
                        on_demand: false,
                    },
                    &[akit_config::Protocol::Http11],
                )
                .map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidInput, format!("console: {e}"))
                })?,
            ),
            _ => None,
        };
        let pages = Arc::new(Pages {
            api: Arc::new(api),
            logins: Arc::new(Logins::new(&LoginOptions::of(c))),
            cookie: OnceLock::new(),
            secure,
            login_pool: pool(LOGIN_WORKERS, LOGIN_QUEUE),
        });
        let mut http = HttpConfig::default();
        // HTTP/1.1 alone, as the Admin API: one request at a time on each connection max_conns
        // counts.
        http.h2 = None;
        http.max_body_bytes = OTHER_BODY_BYTES;
        http.head_timeout = Some(Duration::from_secs(10));
        http.body_timeout = Some(Duration::from_secs(30));
        http.keep_alive_timeout = Some(IDLE_TIMEOUT);
        let mut cfg = EngineConfig::default();
        cfg.max_connections = c.max_conns;
        cfg.shutdown_timeout = Duration::from_secs(1);
        let service = akio_http::serve_with(http, Service(Arc::clone(&pages)));
        let acceptor = match tls {
            Some(t) => AnyAcceptor::new(Tls::server(t, service)),
            None => AnyAcceptor::new(service),
        };
        let listener = match &opts.console_access {
            // Refused at accept, before anything is read (fit's Binding.Filter).
            Some(p) => Listener::tcp(addr).filter(super::accept_filter(p)),
            None => Listener::tcp(addr),
        };
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
        let _ = pages.cookie.set(login::cookie_name(COOKIE, addr.port()));
        Ok(Console {
            engine,
            addr,
            secure,
        })
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

/// `{"error": msg}`, closing the connection: an answer to a client without a valid session, which
/// is not kept (the stopgap for akio's A232).
fn refuse(status: u16, msg: &str) -> Response {
    error(status, msg).header("Connection", "close")
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

/// `{"user", "csrf"}`, a session as fit's `sessionBody` writes it.
fn session_body(s: &Session) -> String {
    format!(
        "{{\"user\":{},\"csrf\":{}}}\n",
        quoted(&s.user),
        quoted(&s.csrf)
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
    fn cookie(&self) -> &str {
        self.cookie.get().map_or(COOKIE, String::as_str)
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
        ctx.add_response_header("Content-Security-Policy", CSP);
        ctx.add_response_header("X-Content-Type-Options", "nosniff");
        ctx.add_response_header("X-Frame-Options", "DENY");
        ctx.add_response_header("Referrer-Policy", "no-referrer");
        if self.secure {
            ctx.add_response_header("Strict-Transport-Security", "max-age=31536000");
        }
        if let Some(o) = ctx.header("Origin") {
            if o != self.origin(ctx) {
                return ctx.respond(refuse(403, "requests from other web pages are refused"));
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
            (Method::Post, "/api/login") => return self.login(ctx),
            (Method::Post, "/api/logout") => return self.logout(ctx),
            _ => {}
        }
        let token = ctx
            .header("Cookie")
            .and_then(|h| cookie(h, self.cookie()))
            .unwrap_or("")
            .to_owned();
        let Some(s) = self.logins.lookup(&token) else {
            return ctx.respond(refuse(401, "log in first"));
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
        if method == Method::Get {
            match path.as_str() {
                "/api/session" => return ctx.respond(json(200, session_body(&s))),
                "/api/workbench" => {
                    return ctx.respond(json(
                        200,
                        format!(
                            "{{\"user\":{},\"modules\":[\"{ADMIN_MODULE}\"]}}\n",
                            quoted(&s.user)
                        ),
                    ));
                }
                _ => {}
            }
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
            (Method::Post, "/account/password" | "/users/password") => {
                return ctx.respond(error(409, NO_STORE));
            }
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
        dispatch(&self.api, ctx);
    }

    /// `GET /api/m/admin/users`: who is logged in, whether a store keeps the passwords (never
    /// yet: I93), and every user who may log in.
    fn users(&self, s: &Session) -> Response {
        let mut b = format!("{{\"user\":{},\"store\":false,\"users\":[", quoted(&s.user));
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
    fn login(self: &Arc<Self>, ctx: &mut Context) {
        let json_body = ctx.header("Content-Type").is_some_and(|t| {
            t.split(';')
                .next()
                .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"))
        });
        if !json_body {
            return ctx.respond(refuse(415, "the body must be sent as application/json"));
        }
        if ctx.body().len() > MAX_LOGIN_BYTES {
            return ctx.respond(refuse(413, "a login's body is 4 KiB at most"));
        }
        let parsed = std::str::from_utf8(ctx.body())
            .ok()
            .and_then(|t| akit_config::value::parse(t).ok());
        let field = |v: &Value, k: &str| match v.get(k) {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        };
        let Some((user, password)) = parsed
            .as_ref()
            .and_then(|v| Some((field(v, "username")?, field(v, "password")?)))
        else {
            return ctx.respond(refuse(400, "the body must be {\"username\", \"password\"}"));
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
                return d.respond(refuse(401, "the user name or password is wrong"));
            }
            ticket.passed();
            let Some((token, s)) = me.logins.start(&user) else {
                return d.respond(refuse(500, "no randomness for a session"));
            };
            log(
                Level::Info,
                format_args!("console: {user:?} logged in from {addr}"),
            );
            d.respond(json(200, session_body(&s)).header("Set-Cookie", me.set_cookie(&token)));
        });
    }

    /// `POST /api/logout`: the session ends, its cookie with it; 204.
    fn logout(&self, ctx: &mut Context) {
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
        ctx.respond(
            Response::new(204)
                .header("Set-Cookie", self.set_cookie(""))
                .header("Connection", "close"),
        );
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
    let secs = r.wait.as_secs().saturating_add(1);
    let mut b = String::from("{\"error\":");
    write_string(&mut b, msg);
    let _ = writeln!(b, ",\"retry_after\":{secs}}}");
    json(429, b)
        .header("Retry-After", secs.to_string())
        .header("Connection", "close")
}
