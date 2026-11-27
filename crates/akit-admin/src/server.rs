//! The Admin API's HTTP side: akio-http on an engine of its own, one I/O thread (decision 006).
//!
//! Reads (`GET /config`, `/versions`) are answered on the I/O thread from the history, which it
//! holds only to clone an `Arc`. A change's body (`POST /load`, `PUT` and `PATCH /config`) is
//! streamed in under `max_config_bytes`; then the request is detached and handed, as a [`Job`],
//! to the control thread through `submit`, as are `DELETE /config`, `POST /rollback/{n}` and
//! `POST /reload`. That thread makes the change from the running config, under the history's
//! lock, applies or checks it (binding listeners and reading files, which block) and answers it.

use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use akio::{AnyAcceptor, Backend, Config as EngineConfig, Engine, Listener};
use akio_http::{BodyEvent, BodyMode, Context, Detached, HttpConfig, Method, Request, Response};
use akit_config::Errors;
use akit_config::value::{Value, write_string};

use crate::certs::{CertReload, Reloader};
use crate::configs::{Applied, ApplyError, Configs, Mode, Outcome, Refused, SOURCE_ADMIN, Version};
use crate::configs::{Effective, ListenerChange, etag_matches};
use crate::dashboard;
use crate::pointer::{self, Problem};
use crate::redact;

/// The header that carries the version of the config a response is about: fit's
/// `Fit-Config-Version`, renamed (decision 037).
pub const VERSION_HEADER: &str = "Akit-Config-Version";

/// The header set, to `1`, on the answer of a dry run that passed: the config was checked, not
/// applied, and [`VERSION_HEADER`] and `ETag` are still the running config's. fit's `Fit-Dry-Run`,
/// renamed (decision 037).
pub const DRY_RUN_HEADER: &str = "Akit-Dry-Run";

/// What the control thread is given: `submit` hands it a [`Job`], and gives it back when it
/// cannot be taken now (its queue is full), to be answered 503. Called on the API's I/O thread:
/// it must not block.
pub type Submit = Box<dyn Fn(Job) -> Result<(), Job> + Send + Sync>;

/// What writes the metrics scrape `GET /metrics` answers with (I37's `Scrape::write`), called
/// on the API's I/O thread: it must not block.
pub type Metrics = Arc<dyn Fn(&mut Vec<u8>) + Send + Sync>;

/// What writes what akit is doing now, as the members of `GET /status`'s object after the running
/// config's version, applied time and source (I63's `Scrape::write_status`): comma-separated, with
/// no comma before the first or after the last. Called on the API's I/O thread: it must not block.
pub type Live = Arc<dyn Fn(&mut String) + Send + Sync>;

/// The content type of a Prometheus text scrape, as fit's `GET /metrics` sends it.
pub const METRICS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// The Admin API's settings beside the config's: its timeouts, backend and metrics.
#[derive(Clone)]
#[non_exhaustive]
pub struct Options {
    /// The akio backend of the API's listener.
    pub backend: Backend,
    /// How long a request's head may take to arrive: 10 s, as fit's.
    pub head_timeout: Duration,
    /// How long a request's body may go without a byte: 60 s, as fit's read timeout.
    pub body_timeout: Duration,
    /// How long a connection may wait for its next request: 60 s, as fit's.
    pub idle_timeout: Duration,
    /// The scrape `GET /metrics` answers with, as fit's Admin API serves it; 404 without one.
    pub metrics: Option<Metrics>,
    /// What `GET /status` says akit is doing now (I63); without it, nothing: no listeners, no
    /// upstreams, no certificates.
    pub status: Option<Live>,
    /// The access lists the API's connections are checked against at accept (I87): the running
    /// graph's `admin.access`, then its global access; every address served without it. Changed
    /// by [`Server::set_access`]. Not asked of a Unix socket's peers.
    pub access: Option<Arc<akit_access::Policy>>,
    /// The access lists the console's connections are checked against at accept (I90): the
    /// running graph's `admin.console.access`, then its global access; every address served
    /// without it. Changed by [`Server::set_console_access`].
    pub console_access: Option<Arc<akit_access::Policy>>,
    /// The log files akit writes (I81, I83), which `GET /logs` lists and `POST /logs/rotate` and
    /// `POST /logs/cleanup` act on; without it, the outputs are listed with no files.
    pub logs: Option<akit_observe::Files>,
    /// The running roles' link tokens, with their last uses and links (I100), which `GET
    /// /tokens` lists; without it, the tokens the running config holds.
    pub tokens: Option<crate::tokens::TokenViews>,
    /// The running roles' modules of the pages (I109), which the console serves beside the
    /// config's own; none without it.
    pub modules: Option<akit_roles::pages::Modules>,
    /// How long after it was accepted a console connection without a valid session is closed
    /// (fit's `UnauthIdleTimeout`, [`crate::console::UNAUTH_TIMEOUT`]); tests shorten it.
    pub console_unauth_timeout: Duration,
    /// How long a console connection with a valid session may go between requests (fit's
    /// `IdleTimeout`, [`crate::console::IDLE_TIMEOUT`]); tests shorten it.
    pub console_idle_timeout: Duration,
    /// How long past its time a console connection without a valid session is kept while a
    /// request is served on it (fit's grace); `None`: `console_unauth_timeout`, as fit's. Tests
    /// lengthen it.
    pub console_grace: Option<Duration>,
    /// What `GET /node` reads of the running akit (I131): the roles running and whether the
    /// running config counts requests; none and no without it.
    pub node: Option<crate::node::NodeView>,
    /// The cluster manager's console module, `cluster` (I132), which `admin.console` serves
    /// beside the config's own; none without it.
    pub cluster: Option<akit_roles::pages::ModuleRef>,
    /// The largest body the console takes for the cluster module's node switcher (I134): the
    /// manager's `max_response_bytes`; 0 without a manager.
    pub cluster_max_body: usize,
}

impl fmt::Debug for Options {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Options")
            .field("backend", &self.backend)
            .field("head_timeout", &self.head_timeout)
            .field("body_timeout", &self.body_timeout)
            .field("idle_timeout", &self.idle_timeout)
            .field("metrics", &self.metrics.is_some())
            .field("status", &self.status.is_some())
            .field("access", &self.access.is_some())
            .field("console_access", &self.console_access.is_some())
            .field("logs", &self.logs.is_some())
            .field("tokens", &self.tokens.is_some())
            .field("modules", &self.modules)
            .field("console_unauth_timeout", &self.console_unauth_timeout)
            .field("console_idle_timeout", &self.console_idle_timeout)
            .field("console_grace", &self.console_grace)
            .field("node", &self.node.is_some())
            .field("cluster", &self.cluster.is_some())
            .finish()
    }
}

impl Default for Options {
    fn default() -> Options {
        Options {
            backend: Backend::default(),
            head_timeout: Duration::from_secs(10),
            body_timeout: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(60),
            metrics: None,
            status: None,
            access: None,
            console_access: None,
            logs: None,
            tokens: None,
            modules: None,
            console_unauth_timeout: crate::console::UNAUTH_TIMEOUT,
            console_idle_timeout: crate::console::IDLE_TIMEOUT,
            console_grace: None,
            node: None,
            cluster: None,
            cluster_max_body: 0,
        }
    }
}

/// The largest body of a request other than a change: none is read.
const OTHER_BODY_BYTES: usize = 64 << 10;

/// A change waiting for the control thread: what to change, its `If-Match`, whether it is a dry
/// run, and the request to answer.
pub struct Job(Box<Change>);

struct Change {
    op: Op,
    /// Where the change comes from: [`SOURCE_ADMIN`], or the console's.
    source: &'static str,
    if_match: Option<String>,
    mode: Mode,
    reply: Detached,
}

/// What a [`Job`] changes.
pub(crate) enum Op {
    /// `POST /load`: a whole config, the body.
    Load(Vec<u8>),
    /// `PUT /config[/<pointer>]`: the value there set to the body; at the root, a whole config,
    /// kept as written.
    Put(Vec<String>, Vec<u8>),
    /// `PATCH /config[/<pointer>]`: the body merged into the value there.
    Patch(Vec<String>, Vec<u8>),
    /// `DELETE /config/<pointer>`: the value there removed.
    Delete(Vec<String>),
    /// `POST /rollback/{n}`: version `n`'s config applied again.
    Rollback(u64),
    /// `POST /reload`: the running config applied again.
    Reload,
    /// `PUT /config/text` and `POST /config/import` (I91): a text's config, whole or some of its
    /// members; with `import`, the running `admin` kept.
    Text {
        frag: Value,
        members: Vec<String>,
        import: bool,
        /// The text, and its format, which the errors of the config it makes are placed in.
        text: String,
        format: akit_config::Format,
    },
    /// The link tokens of a member (I100).
    Tokens(crate::tokens::TokenOp),
    /// `POST /config/import?format=store` (I93b): a `fitstore` file's config, opened, whole or
    /// some of its members, the running `admin` kept.
    Sealed { frag: Value, members: Vec<String> },
}

impl Op {
    fn name(&self) -> &'static str {
        match self {
            Op::Load(_) => "load",
            Op::Put(..) => "put",
            Op::Patch(..) => "patch",
            Op::Delete(_) => "delete",
            Op::Rollback(_) => "rollback",
            Op::Reload => "reload",
            Op::Text { .. } => "text",
            Op::Tokens(t) => t.name(),
            Op::Sealed { .. } => "import",
        }
    }
}

impl fmt::Debug for Job {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = match &self.0.op {
            Op::Load(b) | Op::Put(_, b) | Op::Patch(_, b) => b.len(),
            _ => 0,
        };
        f.debug_struct("Job")
            .field("op", &self.0.op.name())
            .field("bytes", &bytes)
            .field("if_match", &self.0.if_match)
            .field("mode", &self.0.mode)
            .finish_non_exhaustive()
    }
}

/// A body as text: 400 when it is not UTF-8.
fn utf8(data: Vec<u8>) -> Result<String, Refused> {
    String::from_utf8(data).map_err(|_| Refused::Request {
        status: 400,
        msg: "the body is not UTF-8".into(),
    })
}

/// A body as one JSON value: 400 when it is not, as fit's `decodeJSON`.
fn body_value(text: &str) -> Result<Value, Refused> {
    akit_config::value::parse(text).map_err(|errs| Refused::Request {
        status: 400,
        msg: format!(
            "the body is not JSON: {}",
            errs.as_slice().first().map_or("", |e| e.msg.as_str())
        ),
    })
}

/// A config sent whole, as standard JSON: 400 with its errors when it is not JSON.
fn whole(text: &str) -> Result<String, Refused> {
    akit_config::value::standardize(text).map_err(|errs| Refused::Apply(ApplyError::Config(errs)))
}

/// `new`, a value sent for the place `tokens` names, with the secrets it sends as
/// [`redact::REDACTED`] put back from `old`, what the running config holds there (I80b); 400 for
/// one with no secret kept at its place.
fn restored(new: &mut Value, old: Option<&Value>, tokens: &[String]) -> Result<(), Problem> {
    redact::restore(new, old, &pointer::string(tokens))
        .map(|_| ())
        .map_err(|msg| Problem::new(400, msg))
}

/// A whole config sent, `text`, with the secrets it sends as [`redact::REDACTED`] put back from
/// the running config `cur`: as it was written when it sends none, or else written out as fit
/// writes an edited config. Left for [`whole`] to refuse when it is not JSON.
fn unredacted(cur: &Version, text: String) -> Result<String, Refused> {
    if !text.contains(redact::REDACTED) {
        return Ok(text);
    }
    let (Ok(mut new), Ok(old)) = (
        akit_config::value::parse(&text),
        akit_config::value::parse(cur.data()),
    ) else {
        return Ok(text);
    };
    match redact::restore(&mut new, Some(&old), "") {
        Ok(true) => Ok(new.to_json()),
        Ok(false) => Ok(text),
        Err(msg) => Err(Refused::Request { status: 400, msg }),
    }
}

/// The running config `cur` with `change` made to it at `tokens`, written out as fit writes an
/// edited config.
fn edited(
    cur: &Version,
    change: impl FnOnce(&mut Value) -> Result<(), Problem>,
) -> Result<String, Refused> {
    let problem = |p: Problem| Refused::Request {
        status: p.status,
        msg: p.msg,
    };
    let mut root = akit_config::value::parse(cur.data()).map_err(|_| Refused::Request {
        status: 500,
        msg: "the running config is not JSON".into(),
    })?;
    change(&mut root).map_err(problem)?;
    Ok(root.to_json())
}

impl Job {
    /// Makes the change from the running config in `configs` and applies it with `apply` (which
    /// loads, compiles and serves a config, or with [`Mode::DryRun`] only checks it: the control
    /// thread's), and answers the request as fit's API answers it. Returns what was applied, for
    /// the caller to report, or `None` when nothing was (a dry run included).
    pub fn run(
        self,
        configs: &Configs,
        apply: impl FnOnce(&str, Mode) -> Result<Applied, ApplyError>,
    ) -> Option<Outcome> {
        let Change {
            op,
            source,
            if_match,
            mode,
            reply,
        } = *self.0;
        let if_match = if_match.as_deref();
        if let Op::Tokens(t) = op {
            let (resp, outcome) =
                crate::tokens::run(t, configs, source, |text, _| apply(text, Mode::Apply));
            reply.respond(resp);
            return outcome;
        }
        let r = match op {
            Op::Load(data) => utf8(data).and_then(|text| {
                configs.change(
                    source,
                    if_match,
                    mode,
                    |cur| whole(&unredacted(cur, text)?),
                    apply,
                )
            }),
            // A whole config, kept as written; the body must be JSON before anything is compared.
            Op::Put(tokens, data) if tokens.is_empty() => utf8(data).and_then(|text| {
                body_value(&text)?;
                configs.change(
                    source,
                    if_match,
                    mode,
                    |cur| whole(&unredacted(cur, text)?),
                    apply,
                )
            }),
            Op::Put(tokens, data) => utf8(data).and_then(|text| {
                let mut v = body_value(&text)?;
                configs.change(
                    source,
                    if_match,
                    mode,
                    |cur| {
                        edited(cur, |root| {
                            restored(&mut v, pointer::lookup(root, &tokens).ok(), &tokens)?;
                            pointer::put(root, &tokens, v)
                        })
                    },
                    apply,
                )
            }),
            Op::Patch(tokens, data) => utf8(data).and_then(|text| {
                let mut v = body_value(&text)?;
                configs.change(
                    source,
                    if_match,
                    mode,
                    |cur| {
                        edited(cur, |root| {
                            let target = pointer::lookup_mut(root, &tokens)?;
                            restored(&mut v, Some(target), &tokens)?;
                            pointer::merge_patch(target, v);
                            Ok(())
                        })
                    },
                    apply,
                )
            }),
            Op::Delete(tokens) => configs.change(
                source,
                if_match,
                mode,
                |cur| edited(cur, |root| pointer::remove(root, &tokens)),
                apply,
            ),
            Op::Rollback(n) => configs.rollback(n, if_match, mode, apply),
            Op::Reload => configs.reapply(source, if_match, mode, apply),
            Op::Text {
                frag,
                members,
                import,
                text,
                format,
            } => configs
                .change(
                    source,
                    if_match,
                    mode,
                    |cur| crate::text::edited(cur, frag, &members, import),
                    apply,
                )
                .map_err(|e| crate::text::placed(e, &text, format)),
            Op::Sealed { frag, members } => configs.change(
                source,
                if_match,
                mode,
                |cur| crate::text::edited(cur, frag, &members, true),
                apply,
            ),
            // Taken above.
            Op::Tokens(_) => return None,
        };
        match r {
            Ok(o) => {
                let mut resp = json(200, loaded(&o))
                    .header("ETag", &o.etag)
                    .header(VERSION_HEADER, o.version.to_string());
                if o.dry_run {
                    resp = resp.header(DRY_RUN_HEADER, "1");
                }
                reply.respond(resp);
                (!o.dry_run).then_some(o)
            }
            Err(e) => {
                reply.respond(refused(&e));
                None
            }
        }
    }

    /// Answers the request with `status` and `msg`, applying nothing: when akit is shutting
    /// down, or the control thread cannot take it.
    pub fn refuse(self, status: u16, msg: &str) {
        self.0.reply.respond(error(status, msg));
    }
}

/// The answer to a change refused, as fit's `writeUpdateError`.
pub(crate) fn refused_response(e: &Refused) -> Response {
    refused(e)
}

/// The answer to a change refused, as fit's `writeUpdateError`.
fn refused(e: &Refused) -> Response {
    match e {
        Refused::Changed => error(
            412,
            "the config has changed since the ETag given in If-Match",
        ),
        Refused::Request { status, msg } => error(*status, msg),
        Refused::Apply(ApplyError::Config(errs)) => not_applied(errs),
        Refused::Apply(ApplyError::Failed(msg)) => error(
            500,
            &format!("the config was not applied; the running config stays: {msg}"),
        ),
    }
}

/// The Admin API, served.
pub struct Server {
    engine: Engine,
    addr: SocketAddr,
    /// The Unix socket's path, when `address` is `unix/<path>` (A23): removed at shutdown.
    unix: Option<PathBuf>,
    /// The console, when `admin.console` is set (I90).
    console: Option<crate::console::server::Console>,
    /// The TLS certificate's files, read again when they change (I94).
    certs: Option<Reloader>,
}

impl fmt::Debug for Server {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Server")
            .field("addr", &self.addr)
            .field("unix", &self.unix)
            .finish_non_exhaustive()
    }
}

/// `address`, a loopback host and port as the config checked it: `localhost` is 127.0.0.1.
fn resolve(address: &str) -> io::Result<SocketAddr> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{address:?} is not a host and port"),
        )
    };
    if let Some((host, port)) = address.rsplit_once(':') {
        if host.eq_ignore_ascii_case("localhost") {
            let port = port.parse().map_err(|_| invalid())?;
            return Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port));
        }
        // Every address, off loopback with TLS and a token (I92).
        if host.is_empty() {
            let port = port.parse().map_err(|_| invalid())?;
            return Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port));
        }
    }
    address.parse().map_err(|_| invalid())
}

impl Server {
    /// Listens where `settings` says and serves the Admin API for `configs`, handing changes to
    /// the control thread with `submit`. With `token_file`, every request must carry the token it
    /// holds. On the control plane: it reads the token file and binds a socket.
    ///
    /// # Errors
    ///
    /// When the token file cannot be read or holds no token, or the address does not parse or
    /// cannot be bound.
    pub fn start(
        settings: &akit_config::Admin,
        configs: Configs,
        submit: Submit,
        opts: &Options,
    ) -> io::Result<Server> {
        let token = match &settings.token_file {
            None => None,
            Some(file) => Some(read_token(file)?),
        };
        // `unix/<path>` (A23): a socket only akit's user reaches, 0600, a stale one replaced and
        // one in use refused, as fit's; no Host check there.
        let unix = settings.address.strip_prefix("unix/").map(PathBuf::from);
        let addr = match &unix {
            Some(_) => akio::UNIX_ADDR,
            None => resolve(&settings.address)?,
        };
        let tls = settings
            .cert_file
            .as_ref()
            .map(|cert| api_tls(settings, cert));
        let shared = Shared {
            tls: tls.is_some(),
            unix: unix.is_some(),
            configs,
            submit: Arc::from(submit),
            source: SOURCE_ADMIN,
            max_body: settings.max_config_bytes,
            metrics: opts.metrics.clone(),
            status: opts.status.clone(),
            logs: opts.logs.clone(),
            tokens: opts.tokens.clone(),
            node: opts.node.clone(),
            dashboard: !settings.disable_dashboard,
            token,
            deriving: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        // The console (I90), when admin.console is set: the same endpoints, behind its logins,
        // its configs applied as its own source; bound first, so that an address it cannot have
        // stops akit before the API listens.
        let console = match &settings.console {
            Some(c) => Some(crate::console::server::Console::start(
                c,
                Shared {
                    source: crate::console::SOURCE_CONSOLE,
                    token: None,
                    dashboard: false,
                    unix: false,
                    tls: false,
                    // Room for the request's own JSON around a config, as fit's.
                    max_body: settings.max_config_bytes.saturating_add(64 << 10),
                    ..shared.clone()
                },
                opts,
            )?),
            None => None,
        };
        // `GET /node` lists the console when it runs (I131).
        shared.configs.set_console_running(console.is_some());
        let api = Api(Arc::new(shared));
        let mut http = HttpConfig::default();
        // HTTP/1.1 alone: one request at a time on each of the connections max_conns counts.
        http.h2 = None;
        http.max_body_bytes = OTHER_BODY_BYTES;
        http.head_timeout = Some(opts.head_timeout);
        http.body_timeout = Some(opts.body_timeout);
        http.keep_alive_timeout = Some(opts.idle_timeout);
        let mut cfg = EngineConfig::default();
        // Past it, a connection is closed as it is accepted.
        cfg.max_connections = settings.max_conns;
        cfg.shutdown_timeout = Duration::from_secs(1);
        let listener = match (&unix, &opts.access) {
            (Some(path), _) => Listener::unix(path).unix_mode(0o600),
            // Refused at accept, before anything is read (I87, akio's A3).
            (None, Some(p)) => Listener::tcp(addr).filter(accept_filter(p)),
            (None, None) => Listener::tcp(addr),
        };
        let service = akio_http::serve_with(http, api);
        // Over TLS with cert_file (I92), client certificates asked for with client_ca_file; the
        // files read again when they change (I94).
        let (acceptor, certs) = match tls {
            Some(t) => match Reloader::start("admin", t, service) {
                Ok((r, swap)) => (AnyAcceptor::new(swap), Some(r)),
                Err(e) => {
                    if let Some(c) = &console {
                        c.shutdown();
                    }
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, e));
                }
            },
            None => (AnyAcceptor::new(service), None),
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
            .map_err(|e| {
                if let Some(c) = &console {
                    c.shutdown();
                }
                match &unix {
                    Some(p) => io::Error::new(e.kind(), format!("{}: {e}", p.display())),
                    None => e,
                }
            })?;
        if let Err(e) = engine.start() {
            if let Some(c) = &console {
                c.shutdown();
            }
            return Err(e);
        }
        let addr = engine.local_addrs().first().copied().unwrap_or(addr);
        Ok(Server {
            engine,
            addr,
            unix,
            console,
            certs,
        })
    }

    /// Looks at the API's and the console's certificate files once (I94): `-watch-certs`, every
    /// interval, on the control thread, never an I/O thread (it reads files). Files that changed
    /// and have stayed as they are for the watcher's debounce time are loaded and served to new
    /// connections; files that do not load leave the running certificate, and are not tried
    /// again until they change. What happened is in the `admin` logger, and returned.
    pub fn watch_certs(&self, now: Instant) -> Vec<CertReload> {
        self.reloaders().filter_map(|r| r.tick(now)).collect()
    }

    /// Reads the API's and the console's certificate files now (SIGHUP, I94), and serves those
    /// that changed and load to new connections; as [`watch_certs`](Self::watch_certs)
    /// otherwise.
    pub fn reload_certs(&self) -> Vec<CertReload> {
        self.reloaders().filter_map(Reloader::reload).collect()
    }

    fn reloaders(&self) -> impl Iterator<Item = &Reloader> {
        self.certs
            .iter()
            .chain(self.console.as_ref().and_then(|c| c.certs()))
    }

    /// Has the API serve the clients `access` serves from now on (I87): the access lists of a
    /// config applied, `admin.access` then the global access; every client without it. A
    /// connection already open is not checked again. A Unix socket's peers are never checked.
    pub fn set_access(&self, access: Option<&Arc<akit_access::Policy>>) {
        if self.unix.is_some() {
            return;
        }
        let Some(id) = self.engine.listeners().first().map(|l| l.id) else {
            return;
        };
        // Only `NotFound` (the API shut down) can come back.
        let _ = match access {
            Some(p) => self.engine.set_listener_filter(id, accept_filter(p)),
            None => self.engine.clear_listener_filter(id),
        };
    }

    /// Has the console serve the clients `access` serves from now on (I90): the access lists of a
    /// config applied, `admin.console.access` then the global access; every client without it.
    /// Nothing without a console.
    pub fn set_console_access(&self, access: Option<&Arc<akit_access::Policy>>) {
        if let Some(c) = &self.console {
            c.set_access(access);
        }
    }

    /// Where the console listens, as a URL (`http://127.0.0.1:2020/`), when there is one.
    #[must_use]
    pub fn console_url(&self) -> Option<String> {
        self.console
            .as_ref()
            .map(crate::console::server::Console::url)
    }

    /// Where the API listens: [`akio::UNIX_ADDR`] on a Unix socket.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Where the API listens, as `address` names it: a host and port, or `unix/<path>`.
    #[must_use]
    pub fn address(&self) -> String {
        match &self.unix {
            Some(p) => format!("unix/{}", p.display()),
            None => self.addr.to_string(),
        }
    }

    /// Stops the API: no more connections, those open closed once their requests are answered
    /// (a load waiting for the control thread is answered when its [`Job`] is dropped: its
    /// connection closes).
    pub fn shutdown(&self) {
        if let Some(c) = &self.console {
            c.shutdown();
        }
        self.engine.shutdown();
        self.engine.wait();
        // The socket file goes with the API, as Go's listener removes it on close.
        if let Some(p) = &self.unix {
            use std::os::unix::fs::FileTypeExt;
            if std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_socket()) {
                let _ = std::fs::remove_file(p);
            }
        }
    }
}

/// The Admin API's TLS (I92): `cert` and `key_file`, TLS 1.2 and 1.3, every client's certificate
/// verified against `client_ca_file` when it is set. Read when akit starts, and again when the
/// files change (I94).
fn api_tls(settings: &akit_config::Admin, cert: &str) -> akit_config::Tls {
    akit_config::Tls {
        certificates: vec![akit_config::Certificate {
            cert_file: cert.to_owned(),
            key_file: settings.key_file.clone().unwrap_or_default(),
        }],
        client_auth: settings
            .client_ca_file
            .as_ref()
            .map(|ca| akit_config::ClientAuth {
                ca_files: vec![ca.clone()],
                mode: akit_config::ClientAuthMode::Require,
            }),
        max_version: akit_config::TlsVersion::Tls13,
        acme: Vec::new(),
        on_demand: false,
    }
}

/// The accept filter of `policy`: a connection from an address it refuses is closed as it is
/// accepted.
pub(crate) fn accept_filter(
    policy: &Arc<akit_access::Policy>,
) -> impl Fn(&SocketAddr) -> bool + Send + Sync + 'static {
    let p = Arc::clone(policy);
    move |peer: &SocketAddr| p.allows(peer.ip())
}

/// The token in `file`, as fit reads it: the file's content, trimmed of white space.
fn read_token(file: &str) -> io::Result<Vec<u8>> {
    let data = std::fs::read_to_string(file)
        .map_err(|e| io::Error::new(e.kind(), format!("token_file {file}: {e}")))?;
    let token = data.trim();
    if token.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("token_file {file} is empty"),
        ));
    }
    Ok(token.as_bytes().to_vec())
}

#[derive(Clone)]
pub(crate) struct Shared {
    pub(crate) configs: Configs,
    submit: Arc<dyn Fn(Job) -> Result<(), Job> + Send + Sync>,
    /// The source of the configs applied through it: [`SOURCE_ADMIN`], or the console's.
    pub(crate) source: &'static str,
    max_body: usize,
    metrics: Option<Metrics>,
    status: Option<Live>,
    /// The log files akit writes (I83).
    pub(crate) logs: Option<akit_observe::Files>,
    /// The running roles' link tokens (I100).
    pub(crate) tokens: Option<crate::tokens::TokenViews>,
    /// What `GET /node` reads of the running akit (I131).
    node: Option<crate::node::NodeView>,
    /// Whether the dashboard is served (I63): unless `disable_dashboard`.
    dashboard: bool,
    /// The token every request must carry, from `token_file`; none asked for without it.
    token: Option<Vec<u8>>,
    /// On a Unix socket (A23): only akit's user reaches it, so `Host` is not checked.
    unix: bool,
    /// Served over TLS (I92): the API's own origin is `https://`.
    tls: bool,
    /// Set while a key is derived for a `fitstore` file exported or imported (I93b), from the
    /// Admin API and the console alike: one at a time, the others refused.
    pub(crate) deriving: Arc<std::sync::atomic::AtomicBool>,
}

struct Api(Arc<Shared>);

/// Whether `req` sends a config or a value of one, or a purge: its body is streamed in, within
/// `max_config_bytes`.
pub(crate) fn takes_config(method: &Method, path: &str) -> bool {
    match method {
        Method::Post => {
            crate::tokens::takes_body(method, path)
                || path == "/load"
                || path == "/config/render"
                || path == "/config/parse"
                || path == "/config/export"
                || path == "/config/import"
                || path == "/cache/purge"
                || path == "/logs/rotate"
                || path == "/logs/cleanup"
        }
        Method::Put | Method::Patch => {
            path == "/config"
                || path.starts_with("/config/")
                || crate::tokens::takes_body(method, path)
        }
        _ => false,
    }
}

/// The JSON pointer a `/config[/<pointer>]` path names, percent-decoded: none (the root) for
/// `/config` and `/config/`, as fit's.
fn pointer_of(path: &str) -> Result<Vec<String>, Problem> {
    let rest = path.get("/config".len()..).unwrap_or("");
    if rest.is_empty() || rest == "/" {
        return Ok(Vec::new());
    }
    let Some(decoded) = akio_http::message::percent_decode(rest) else {
        return Err(Problem::new(
            400,
            format!("{rest:?} is not a JSON pointer: a bad escape"),
        ));
    };
    pointer::parse(&decoded)
}

/// Whether the request asks for a dry run: `?dry_run=1` or `true`; anything but those, `0`,
/// `false` or none is refused, as fit's `isDryRun`.
pub(crate) fn mode_of(ctx: &Context) -> Result<Mode, Problem> {
    match ctx.req().query_param("dry_run") {
        None | Some("" | "0" | "false") => Ok(Mode::Apply),
        Some("1" | "true") => Ok(Mode::DryRun),
        Some(other) => Err(Problem::new(
            400,
            format!("dry_run is 1 or true, 0 or false, not {other:?}"),
        )),
    }
}

/// The answer to a request refused before it is handed on.
fn problem(p: &Problem) -> Response {
    error(p.status, &p.msg)
}

const CONFIG_METHODS: &str = "GET, HEAD, PUT, PATCH, DELETE";

impl akio_http::Service for Api {
    fn call(&self, ctx: &mut Context) {
        // While a console user's password is the temporary one `akit run -admin` gave, nothing
        // is served (I93c); the console, which calls the endpoints past this, asks for it.
        if let Some((user, _)) = self.0.configs.temporary_password() {
            ctx.respond(error(
                403,
                &format!(
                    "the console user {user:?} still has its temporary password: change it on \
                     the console first"
                ),
            ));
            return;
        }
        if let Some(resp) = guard(ctx, self.0.token.as_deref(), self.0.unix, self.0.tls) {
            ctx.respond(resp);
            return;
        }
        dispatch(&self.0, ctx);
    }

    fn body_mode(&self, req: &Request) -> BodyMode {
        if takes_config(&req.method, req.path()) {
            BodyMode::Stream
        } else {
            BodyMode::Buffered
        }
    }
}

/// Answers a request of the API's endpoints, past its guard: the Admin API's own, or the
/// console's, which checks its sessions instead and calls this with the request's target
/// rewritten to the API's path (I90).
pub(crate) fn dispatch(shared: &Arc<Shared>, ctx: &mut Context) {
    let this = Api(Arc::clone(shared));
    this.serve(ctx);
}

impl Api {
    fn serve(&self, ctx: &mut Context) {
        let path = ctx.path().to_owned();
        let method = ctx.method().clone();
        let read = matches!(method, Method::Get | Method::Head);
        let post = method == Method::Post;
        let resp = match path.as_str() {
            "/load" if post => return change(&self.0, ctx, None),
            "/reload" if post => match mode_of(ctx) {
                Ok(mode) => return submit(&self.0, ctx, Op::Reload, mode),
                Err(p) => problem(&p),
            },
            "/load" | "/reload" => not_allowed("POST"),
            "/versions" if read => versions(&self.0.configs),
            "/metrics" if read => match &self.0.metrics {
                Some(write) => {
                    let mut out = Vec::new();
                    write(&mut out);
                    Response::with_body(200, METRICS_CONTENT_TYPE, out)
                }
                None => error(404, "there is no such endpoint"),
            },
            "/metrics" => not_allowed("GET, HEAD"),
            "/versions" => not_allowed("GET, HEAD"),
            "/status" if read => crate::status::status(&self.0.configs, self.0.status.as_ref()),
            "/status" => not_allowed("GET, HEAD"),
            // I131: who this akit is, for a manager of many, and the config's schema.
            "/node" if read => crate::node::answer(
                &self.0.configs,
                self.0.node.as_ref(),
                self.0.metrics.is_some(),
            ),
            "/node" => not_allowed("GET, HEAD"),
            "/node/managed" => return crate::managed::serve(&self.0.configs, ctx),
            "/schema" if read => crate::node::schema_answer(),
            "/schema" => not_allowed("GET, HEAD"),
            "/cache" if read => crate::cache::zones(),
            "/cache" => not_allowed("GET, HEAD"),
            "/cache/purge" if post => return crate::cache::purge(&self.0, ctx),
            "/cache/purge" => not_allowed("POST"),
            // I83: the log files, on the pool.
            "/logs" if read => return crate::logs::list(&self.0, ctx),
            "/logs" => not_allowed("GET, HEAD"),
            "/logs/rotate" if post => return crate::logs::operate(&self.0, ctx, true),
            "/logs/cleanup" if post => return crate::logs::operate(&self.0, ctx, false),
            "/logs/rotate" | "/logs/cleanup" => not_allowed("POST"),
            // I100: the link tokens of the tunnel's hub and the LLM roles.
            p if p == "/tokens" || p.starts_with("/tokens/") => {
                return crate::tokens::serve(&self.0, ctx, p);
            }
            // Its other methods edit the member `effective`, as fit's `PUT /config/{path...}`.
            p if self.0.dashboard && dashboard::is_dashboard(p) => {
                if read {
                    dashboard::serve(p)
                } else {
                    not_allowed("GET, HEAD")
                }
            }
            p if p.starts_with("/rollback/") => {
                if !post {
                    not_allowed("POST")
                } else if let Ok(n) = p.get("/rollback/".len()..).unwrap_or("").parse::<u64>() {
                    // fit's rollback takes no dry_run.
                    return submit(&self.0, ctx, Op::Rollback(n), Mode::Apply);
                } else {
                    error(404, "versions are numbers")
                }
            }
            p if p.starts_with("/versions/") => {
                if read {
                    version(&self.0.configs, p.get("/versions/".len()..).unwrap_or(""))
                } else {
                    not_allowed("GET, HEAD")
                }
            }
            "/config/effective" if read => effective(&self.0.configs.effective()),
            // I91: the config as text, for the console's raw editors and transfer page.
            "/config/render" if post => return crate::text::render(&self.0, ctx),
            "/config/parse" if post => return crate::text::parse(&self.0, ctx),
            "/config/export" if post => return crate::text::export(&self.0, ctx),
            "/config/import" if post => return crate::text::put(&self.0, ctx, true),
            "/config/text" if method == Method::Put => {
                return crate::text::put(&self.0, ctx, false);
            }
            "/config/render" | "/config/parse" | "/config/export" | "/config/import" => {
                not_allowed("POST")
            }
            "/config/text" => not_allowed("PUT"),
            p if p == "/config" || p.starts_with("/config/") => match method {
                Method::Get | Method::Head => get_config(ctx, &self.0.configs, &path),
                Method::Put | Method::Patch => match pointer_of(&path) {
                    Ok(tokens) => return change(&self.0, ctx, Some(tokens)),
                    Err(p) => problem(&p),
                },
                Method::Delete => match pointer_of(&path).and_then(|t| Ok((t, mode_of(ctx)?))) {
                    Ok((tokens, mode)) => {
                        return submit(&self.0, ctx, Op::Delete(tokens), mode);
                    }
                    Err(p) => problem(&p),
                },
                _ => not_allowed(CONFIG_METHODS),
            },
            _ => error(404, "there is no such endpoint"),
        };
        ctx.respond(resp);
    }
}

/// Hands `op` to the control thread, the request detached to be answered there; 503 when it
/// cannot take it now.
pub(crate) fn submit(shared: &Shared, ctx: &mut Context, op: Op, mode: Mode) {
    let if_match = ctx.header("If-Match").map(str::to_owned);
    let Some(reply) = ctx.detach() else { return };
    submit_detached(shared, reply, if_match, op, mode);
}

/// Hands `op` to the control thread, to answer `reply` there, as [`submit`] does for a request
/// detached already (a store's file opened on the pool, I93b).
pub(crate) fn submit_detached(
    shared: &Shared,
    reply: Detached,
    if_match: Option<String>,
    op: Op,
    mode: Mode,
) {
    let job = Job(Box::new(Change {
        op,
        source: shared.source,
        if_match,
        mode,
        reply,
    }));
    if let Err(job) = (shared.submit)(job) {
        job.refuse(503, "akit is busy applying other configs: try again");
    }
}

/// What must not reach the API, refused as fit refuses it: with a token asked for, a request
/// without it (401), but for the dashboard's files, which hold no data and ask for the token
/// themselves (I63); a request from a web page, which a browser marks with `Origin` (but a read
/// from the API's own origin); and, without a token, one whose `Host` is not a loopback name, as
/// a web page's is after DNS rebinding, unless it came over the Unix socket (`unix`), which no web
/// page reaches (fit's `loopback` is false there).
fn guard(ctx: &Context, token: Option<&[u8]>, unix: bool, tls: bool) -> Option<Response> {
    if let Some(token) = token {
        let valid = ctx
            .header("Authorization")
            .and_then(|a| a.strip_prefix("Bearer "))
            .is_some_and(|got| same(got.as_bytes(), token));
        let dashboard = matches!(ctx.method(), Method::Get | Method::Head)
            && dashboard::is_dashboard(ctx.path());
        if !valid && !dashboard {
            // Nothing more is taken on the connection, as fit's: a client cannot try again on
            // it, and the body it sends is not read (akio closes an HTTP/1 connection after a
            // response that says `Connection: close`).
            return Some(
                error(401, "a valid bearer token is needed")
                    .header("WWW-Authenticate", "Bearer realm=\"akit admin\"")
                    .header("Connection", "close"),
            );
        }
    }
    let host = ctx.header("Host").unwrap_or("");
    if let Some(origin) = ctx.header("Origin") {
        let same_origin_read = matches!(ctx.method(), Method::Get | Method::Head)
            && origin
                .strip_prefix(if tls { "https://" } else { "http://" })
                .is_some_and(|h| !h.is_empty() && h.eq_ignore_ascii_case(host));
        if !same_origin_read {
            return Some(error(403, "requests from web pages are refused"));
        }
    }
    if token.is_none() && !unix && !loopback_host(host) {
        return Some(error(403, "the Host field must name a loopback host"));
    }
    None
}

/// Whether `a` and `b` are the same bytes, in a time that does not depend on where they differ
/// (but on their lengths), as Go's `subtle.ConstantTimeCompare`.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Whether `hostport` names `localhost` or a loopback address, as fit's `loopbackHost`.
fn loopback_host(hostport: &str) -> bool {
    let host = match hostport.rsplit_once(':') {
        // A bare IPv6 address has colons and no port.
        Some((h, p))
            if p.bytes().all(|b| b.is_ascii_digit())
                && (!h.contains(':') || h.starts_with('[')) =>
        {
            h
        }
        _ => hostport,
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// `POST /load` (`tokens` none), `PUT` or `PATCH /config[/<pointer>]`: the body, JSON within
/// `max_config_bytes`, streamed in and handed to the control thread.
fn change(shared: &Arc<Shared>, ctx: &mut Context, tokens: Option<Vec<String>>) {
    let mode = match mode_of(ctx) {
        Ok(m) => m,
        Err(p) => return ctx.respond(problem(&p)),
    };
    let patch = ctx.method() == &Method::Patch;
    let types: &[&str] = if patch {
        &["application/merge-patch+json", "application/json"]
    } else {
        &["application/json"]
    };
    let shared2 = Arc::clone(shared);
    read_body(shared, ctx, types, move |ctx, data| {
        let op = match tokens {
            None => Op::Load(data),
            Some(t) if patch => Op::Patch(t, data),
            Some(t) => Op::Put(t, data),
        };
        submit(&shared2, ctx, op, mode);
    });
}

/// Streams in the body of a request sent as one of `types`, within `max_config_bytes`, and
/// hands it to `done`; 415 for another type, 413 past the bound, as fit's `readBody`.
pub(crate) fn read_body(
    shared: &Arc<Shared>,
    ctx: &mut Context,
    types: &'static [&'static str],
    done: impl FnOnce(&mut Context, Vec<u8>) + Send + 'static,
) {
    let typed = ctx.header("Content-Type").is_some_and(|t| {
        t.split(';')
            .next()
            .is_some_and(|m| types.iter().any(|w| m.trim().eq_ignore_ascii_case(w)))
    });
    if !typed {
        return ctx.respond(error(
            415,
            &format!("the body must be sent as {}", types.join(" or ")),
        ));
    }
    let max = shared.max_body;
    // The connection closes after it, the rest of the body not read, as Go's server closes one
    // whose body passed `MaxBytesReader`'s limit (akio honours `Connection: close`).
    let too_large = move || {
        error(
            413,
            &format!("the body is larger than max_config_bytes, {max}"),
        )
        .header("Connection", "close")
    };
    let declared = ctx
        .header("Content-Length")
        .and_then(|l| l.trim().parse::<u64>().ok());
    if declared.is_some_and(|n| n > u64::try_from(max).unwrap_or(u64::MAX)) {
        return ctx.respond(too_large());
    }
    // No body: nothing will arrive to end it.
    if declared == Some(0) || (declared.is_none() && ctx.header("Transfer-Encoding").is_none()) {
        return done(ctx, Vec::new());
    }
    // The body as it arrives, within max: bounded by the length sent, when one is.
    let mut data = Vec::with_capacity(
        declared
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0)
            .min(max),
    );
    let mut over = false;
    let mut done = Some(done);
    ctx.on_body(move |ctx, ev| {
        if over {
            return;
        }
        match ev {
            BodyEvent::Chunk(chunk) => {
                if data.len().saturating_add(chunk.len()) > max {
                    over = true;
                    data = Vec::new();
                    ctx.respond(too_large());
                } else {
                    data.extend_from_slice(chunk);
                }
            }
            BodyEvent::End(_) => {
                if let Some(done) = done.take() {
                    done(ctx, std::mem::take(&mut data));
                }
            }
            // Refused by a bound of akio's (`Context::limit_body`), which answered it.
            BodyEvent::TooLarge => over = true,
        }
    });
}

/// `GET /config[/<pointer>]`.
fn get_config(ctx: &Context, configs: &Configs, path: &str) -> Response {
    let v = configs.current();
    if ctx
        .header("If-None-Match")
        .is_some_and(|m| etag_matches(m, &v.etag))
    {
        return Response::new(304)
            .header("ETag", &v.etag)
            .header(VERSION_HEADER, v.number.to_string());
    }
    let tokens = match pointer_of(path) {
        Ok(t) => t,
        Err(p) => return problem(&p),
    };
    if tokens.is_empty() {
        return config(&v);
    }
    let found = akit_config::value::parse(v.data())
        .map_err(|_| Problem::new(500, "the running config is not JSON".into()))
        .and_then(|root| {
            let mut found = pointer::lookup(&root, &tokens)?.clone();
            redact::at_pointer(&mut found, &tokens);
            Ok(found.to_json())
        });
    match found {
        Ok(body) => json(200, body)
            .header("ETag", &v.etag)
            .header(VERSION_HEADER, v.number.to_string()),
        Err(p) => error(p.status, &p.msg),
    }
}

/// A config as it was written, its secrets redacted (I80b), with its ETag and version.
fn config(v: &Version) -> Response {
    let Some(data) = redact::config(v.data()) else {
        return error(500, "the running config is not JSON");
    };
    Response::with_body(200, "application/json", data.into_bytes())
        .header("ETag", &v.etag)
        .header(VERSION_HEADER, v.number.to_string())
}

/// `GET /versions`: the configs kept, oldest first.
fn versions(configs: &Configs) -> Response {
    let vs = configs.versions();
    let current = vs.last().map_or(0, |v| v.number);
    let mut b = format!("{{\"current\":{current},\"versions\":[");
    for (i, v) in vs.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        b.push_str(&format!("{{\"version\":{},\"applied\":", v.number));
        let mut t = Vec::new();
        akit_observe::time::rfc3339(&mut t, akit_observe::time::Stamp::of(v.applied), 0);
        write_string(&mut b, &String::from_utf8_lossy(&t));
        b.push_str(",\"source\":");
        write_string(&mut b, &v.source);
        b.push_str(&format!(",\"bytes\":{},\"etag\":", v.data().len()));
        write_string(&mut b, &v.etag);
        b.push('}');
    }
    b.push_str("]}\n");
    json(200, b)
}

/// `GET /versions/{n}`: the config applied as version `n`.
fn version(configs: &Configs, n: &str) -> Response {
    let Ok(n) = n.parse::<u64>() else {
        return error(404, "versions are numbers");
    };
    match configs.version(n) {
        Some(v) => config(&v),
        None => error(404, &format!("version {n} is not kept")),
    }
}

/// `GET /config/effective`: the running config as it runs, the base config with what providers
/// provide merged into it, and where each part came from, as fit's `Effective`: the version and
/// the base config's ETag, the sources in their order of precedence, the config (compact, as Go
/// writes a raw message), the source of each member and array element, and what was shadowed.
fn effective(e: &Effective) -> Response {
    let v = &e.version;
    let mut b = format!("{{\"version\":{},\"etag\":", v.number);
    write_string(&mut b, &v.etag);
    b.push_str(",\"precedence\":[");
    for (i, s) in e.precedence.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        write_string(&mut b, s);
    }
    b.push_str("],\"config\":");
    match redact::config(v.effective()) {
        Some(c) => compact(&mut b, &c),
        None => return error(500, "the running config is not JSON"),
    }
    b.push_str(",\"sources\":[");
    for (i, s) in v.sources().iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        b.push_str("{\"path\":");
        write_string(&mut b, &s.path);
        b.push_str(",\"source\":");
        write_string(&mut b, &s.source);
        if !s.from.is_empty() {
            b.push_str(",\"from\":");
            write_string(&mut b, &s.from);
        }
        b.push('}');
    }
    b.push(']');
    if !v.shadowed().is_empty() {
        b.push_str(",\"shadowed\":[");
        for (i, s) in v.shadowed().iter().enumerate() {
            if i > 0 {
                b.push(',');
            }
            b.push_str("{\"source\":");
            write_string(&mut b, &s.source);
            b.push_str(",\"from\":");
            write_string(&mut b, &s.from);
            b.push_str(",\"name\":");
            write_string(&mut b, &s.name);
            b.push_str(",\"by\":");
            write_string(&mut b, &s.by);
            b.push('}');
        }
        b.push(']');
    }
    b.push_str("}\n");
    json(200, b).header(VERSION_HEADER, v.number.to_string())
}

/// `json`, standard JSON, without the white space between its tokens.
fn compact(b: &mut String, json: &str) {
    let mut in_string = false;
    let mut escaped = false;
    for c in json.chars() {
        if in_string {
            b.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
            b.push(c);
        } else if !matches!(c, ' ' | '\t' | '\r' | '\n') {
            b.push(c);
        }
    }
}

/// A change's answer, as fit's: its version and ETag, the listeners it opened and closed, its
/// warnings; for a dry run, the running config's version and ETag and the ETag it would have.
fn loaded(o: &Outcome) -> String {
    let mut b = String::from("{");
    if o.dry_run {
        b.push_str("\"dry_run\":true,");
    }
    b.push_str(&format!("\"version\":{},\"etag\":", o.version));
    write_string(&mut b, &o.etag);
    if !o.new_etag.is_empty() {
        b.push_str(",\"new_etag\":");
        write_string(&mut b, &o.new_etag);
    }
    if o.unchanged {
        b.push_str(",\"unchanged\":true");
    }
    let lists = [
        ("listeners_added", &o.applied.added),
        ("listeners_removed", &o.applied.removed),
    ];
    for (name, ls) in lists.iter().filter(|(_, ls)| !ls.is_empty()) {
        b.push_str(&format!(",\"{name}\":["));
        for (i, l) in ls.iter().enumerate() {
            if i > 0 {
                b.push(',');
            }
            listener(&mut b, l);
        }
        b.push(']');
    }
    if !o.applied.warnings.is_empty() {
        b.push_str(",\"warnings\":");
        strings(&mut b, &o.applied.warnings);
    }
    b.push_str("}\n");
    b
}

fn listener(b: &mut String, l: &ListenerChange) {
    b.push_str("{\"name\":");
    write_string(b, &l.name);
    b.push_str(",\"address\":");
    write_string(b, &l.address);
    b.push_str(",\"protocols\":");
    strings(b, &l.protocols);
    b.push('}');
}

fn strings(b: &mut String, ss: &[String]) {
    b.push('[');
    for (i, s) in ss.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        write_string(b, s);
    }
    b.push(']');
}

/// 400 with a config's errors, as fit answers a config that does not load or compile.
fn not_applied(errs: &Errors) -> Response {
    let mut b = String::from(
        "{\"error\":\"the config was not applied; the running config stays\",\"errors\":[",
    );
    for (i, e) in errs.as_slice().iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        b.push('{');
        if !e.path.is_empty() {
            b.push_str("\"path\":");
            write_string(&mut b, &e.path);
            b.push(',');
        }
        if e.line > 0 {
            b.push_str(&format!("\"line\":{},\"column\":{},", e.line, e.col));
        }
        b.push_str("\"message\":");
        write_string(&mut b, &e.msg);
        b.push('}');
    }
    b.push_str("]}\n");
    json(400, b)
}

pub(crate) fn json(status: u16, body: String) -> Response {
    Response::with_body(status, "application/json", body.into_bytes())
}

/// `{"error": msg}`, as fit's `writeError`.
pub(crate) fn error(status: u16, msg: &str) -> Response {
    let mut b = String::from("{\"error\":");
    write_string(&mut b, msg);
    b.push_str("}\n");
    json(status, b)
}

pub(crate) fn not_allowed(allow: &str) -> Response {
    error(405, "method not allowed").header("Allow", allow)
}

#[cfg(test)]
mod tests {
    #[test]
    fn compacts() {
        let mut b = String::new();
        super::compact(&mut b, "{ \"a b\" : [1, \"x\\\" y\"],\n  \"c\": {} }");
        assert_eq!(b, r#"{"a b":[1,"x\" y"],"c":{}}"#);
    }

    #[test]
    fn tokens_compared() {
        assert!(super::same(b"s3cret", b"s3cret"));
        assert!(!super::same(b"s3cret", b"s3creT"));
        assert!(!super::same(b"s3cret", b"s3cre"));
        assert!(!super::same(b"", b"x"));
        assert!(super::same(b"", b""));
    }

    #[test]
    fn loopback_hosts() {
        for h in [
            "localhost",
            "LOCALHOST:2019",
            "127.0.0.1",
            "127.0.0.1:2019",
            "127.1.2.3:1",
            "[::1]:2019",
            "::1",
        ] {
            assert!(super::loopback_host(h), "{h}");
        }
        for h in [
            "",
            "evil.example.com:2019",
            "10.0.0.1:2019",
            "[::2]:1",
            ":2019",
        ] {
            assert!(!super::loopback_host(h), "{h}");
        }
    }
}
