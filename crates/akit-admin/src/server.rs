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
use std::sync::Arc;
use std::time::Duration;

use akio::{Backend, Config as EngineConfig, Engine, Listener};
use akio_http::{BodyEvent, BodyMode, Context, Detached, HttpConfig, Method, Request, Response};
use akit_config::Errors;
use akit_config::value::{Value, write_string};

use crate::configs::{Applied, ApplyError, Configs, Mode, Outcome, Refused, SOURCE_ADMIN, Version};
use crate::configs::{ListenerChange, SOURCE_ROLLBACK, etag_matches};
use crate::pointer::{self, Problem};

/// The header that carries the version of the config a response is about: fit's name, so that
/// fit's clients read it.
pub const VERSION_HEADER: &str = "Fit-Config-Version";

/// The header set, to `1`, on the answer of a dry run that passed: the config was checked, not
/// applied, and [`VERSION_HEADER`] and `ETag` are still the running config's. fit's name.
pub const DRY_RUN_HEADER: &str = "Fit-Dry-Run";

/// What the control thread is given: `submit` hands it a [`Job`], and gives it back when it
/// cannot be taken now (its queue is full), to be answered 503. Called on the API's I/O thread:
/// it must not block.
pub type Submit = Box<dyn Fn(Job) -> Result<(), Job> + Send + Sync>;

/// What writes the metrics scrape `GET /metrics` answers with (I37's `Scrape::write`), called
/// on the API's I/O thread: it must not block.
pub type Metrics = Arc<dyn Fn(&mut Vec<u8>) + Send + Sync>;

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
}

impl fmt::Debug for Options {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Options")
            .field("backend", &self.backend)
            .field("head_timeout", &self.head_timeout)
            .field("body_timeout", &self.body_timeout)
            .field("idle_timeout", &self.idle_timeout)
            .field("metrics", &self.metrics.is_some())
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
    if_match: Option<String>,
    mode: Mode,
    reply: Detached,
}

/// What a [`Job`] changes.
enum Op {
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
            if_match,
            mode,
            reply,
        } = *self.0;
        let if_match = if_match.as_deref();
        let r = match op {
            Op::Load(data) => utf8(data).and_then(|text| {
                configs.change(SOURCE_ADMIN, if_match, mode, |_| whole(&text), apply)
            }),
            // A whole config, kept as written; the body must be JSON before anything is compared.
            Op::Put(tokens, data) if tokens.is_empty() => utf8(data).and_then(|text| {
                body_value(&text)?;
                configs.change(SOURCE_ADMIN, if_match, mode, |_| whole(&text), apply)
            }),
            Op::Put(tokens, data) => utf8(data).and_then(|text| {
                let v = body_value(&text)?;
                configs.change(
                    SOURCE_ADMIN,
                    if_match,
                    mode,
                    |cur| edited(cur, |root| pointer::put(root, &tokens, v)),
                    apply,
                )
            }),
            Op::Patch(tokens, data) => utf8(data).and_then(|text| {
                let v = body_value(&text)?;
                configs.change(
                    SOURCE_ADMIN,
                    if_match,
                    mode,
                    |cur| {
                        edited(cur, |root| {
                            pointer::lookup_mut(root, &tokens)
                                .map(|target| pointer::merge_patch(target, v))
                        })
                    },
                    apply,
                )
            }),
            Op::Delete(tokens) => configs.change(
                SOURCE_ADMIN,
                if_match,
                mode,
                |cur| edited(cur, |root| pointer::remove(root, &tokens)),
                apply,
            ),
            Op::Rollback(n) => configs.change(
                SOURCE_ROLLBACK,
                if_match,
                mode,
                |_| match configs.version(n) {
                    Some(v) => Ok(v.data().to_owned()),
                    None => Err(Refused::Request {
                        status: 404,
                        msg: format!("version {n} is not kept"),
                    }),
                },
                apply,
            ),
            Op::Reload => configs.change(
                SOURCE_ADMIN,
                if_match,
                mode,
                |cur| Ok(cur.data().to_owned()),
                apply,
            ),
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
}

impl fmt::Debug for Server {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Server")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

/// `address`, a loopback host and port as the config checked it: `localhost` is 127.0.0.1.
fn resolve(address: &str) -> io::Result<SocketAddr> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{address:?} is not a loopback host and port"),
        )
    };
    if let Some((host, port)) = address.rsplit_once(':') {
        if host.eq_ignore_ascii_case("localhost") {
            let port = port.parse().map_err(|_| invalid())?;
            return Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port));
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
        let addr = resolve(&settings.address)?;
        let api = Api(Arc::new(Shared {
            configs,
            submit,
            max_body: settings.max_config_bytes,
            metrics: opts.metrics.clone(),
            token,
        }));
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
        let engine = Engine::builder()
            .config(cfg)
            .listen(
                Listener::tcp(addr)
                    .backend(opts.backend.clone())
                    .threads(1)
                    .serve(akio_http::serve_with(http, api)),
            )
            .build()?;
        engine.start()?;
        let addr = engine.local_addrs().first().copied().unwrap_or(addr);
        Ok(Server { engine, addr })
    }

    /// Where the API listens.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Stops the API: no more connections, those open closed once their requests are answered
    /// (a load waiting for the control thread is answered when its [`Job`] is dropped: its
    /// connection closes).
    pub fn shutdown(&self) {
        self.engine.shutdown();
        self.engine.wait();
    }
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

struct Shared {
    configs: Configs,
    submit: Submit,
    max_body: usize,
    metrics: Option<Metrics>,
    /// The token every request must carry, from `token_file`; none asked for without it.
    token: Option<Vec<u8>>,
}

struct Api(Arc<Shared>);

/// Whether `req` sends a config or a value of one: its body is streamed in, within
/// `max_config_bytes`.
fn takes_config(req: &Request) -> bool {
    let path = req.path();
    match req.method {
        Method::Post => path == "/load",
        Method::Put | Method::Patch => path == "/config" || path.starts_with("/config/"),
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
fn mode_of(ctx: &Context) -> Result<Mode, Problem> {
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
        if let Some(resp) = guard(ctx, self.0.token.as_deref()) {
            ctx.respond(resp);
            return;
        }
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

    fn body_mode(&self, req: &Request) -> BodyMode {
        if takes_config(req) {
            BodyMode::Stream
        } else {
            BodyMode::Buffered
        }
    }
}

/// Hands `op` to the control thread, the request detached to be answered there; 503 when it
/// cannot take it now.
fn submit(shared: &Shared, ctx: &mut Context, op: Op, mode: Mode) {
    let if_match = ctx.header("If-Match").map(str::to_owned);
    let Some(reply) = ctx.detach() else { return };
    let job = Job(Box::new(Change {
        op,
        if_match,
        mode,
        reply,
    }));
    if let Err(job) = (shared.submit)(job) {
        job.refuse(503, "akit is busy applying other configs: try again");
    }
}

/// What must not reach the API, refused as fit refuses it: with a token asked for, a request
/// without it (401); a request from a web page, which a browser marks with `Origin` (but a read
/// from the API's own origin); and, without a token, one whose `Host` is not a loopback name, as
/// a web page's is after DNS rebinding.
fn guard(ctx: &Context, token: Option<&[u8]>) -> Option<Response> {
    if let Some(token) = token {
        let valid = ctx
            .header("Authorization")
            .and_then(|a| a.strip_prefix("Bearer "))
            .is_some_and(|got| same(got.as_bytes(), token));
        if !valid {
            // fit closes the connection too, so that a client cannot try again on it: akio-http
            // cannot close a connection after a response yet (akio API request A24).
            return Some(
                error(401, "a valid bearer token is needed")
                    .header("WWW-Authenticate", "Bearer realm=\"akit admin\""),
            );
        }
    }
    let host = ctx.header("Host").unwrap_or("");
    if let Some(origin) = ctx.header("Origin") {
        let same_origin_read = matches!(ctx.method(), Method::Get | Method::Head)
            && origin
                .strip_prefix("http://")
                .is_some_and(|h| !h.is_empty() && h.eq_ignore_ascii_case(host));
        if !same_origin_read {
            return Some(error(403, "requests from web pages are refused"));
        }
    }
    if token.is_none() && !loopback_host(host) {
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
    // What more the client sends is read and dropped, within the body timeout: akio-http cannot
    // close a connection after a response that refuses its body yet (akio API request A24).
    let too_large = move || {
        error(
            413,
            &format!("the body is larger than max_config_bytes, {max}"),
        )
    };
    let declared = ctx
        .header("Content-Length")
        .and_then(|l| l.trim().parse::<u64>().ok());
    if declared.is_some_and(|n| n > u64::try_from(max).unwrap_or(u64::MAX)) {
        return ctx.respond(too_large());
    }
    // The body as it arrives, within max: bounded by the length sent, when one is.
    let mut data = Vec::with_capacity(
        declared
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0)
            .min(max),
    );
    let shared = Arc::clone(shared);
    let mut over = false;
    let mut tokens = Some(tokens);
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
                let data = std::mem::take(&mut data);
                let op = match tokens.take().flatten() {
                    None => Op::Load(data),
                    Some(t) if patch => Op::Patch(t, data),
                    Some(t) => Op::Put(t, data),
                };
                submit(&shared, ctx, op, mode);
            }
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
        .and_then(|root| pointer::lookup(&root, &tokens).map(Value::to_json));
    match found {
        Ok(body) => json(200, body)
            .header("ETag", &v.etag)
            .header(VERSION_HEADER, v.number.to_string()),
        Err(p) => error(p.status, &p.msg),
    }
}

/// A config as it was written, with its ETag and version.
fn config(v: &Version) -> Response {
    Response::with_body(200, "application/json", v.data().as_bytes().to_vec())
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
        write_string(&mut b, v.source);
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

fn json(status: u16, body: String) -> Response {
    Response::with_body(status, "application/json", body.into_bytes())
}

/// `{"error": msg}`, as fit's `writeError`.
fn error(status: u16, msg: &str) -> Response {
    let mut b = String::from("{\"error\":");
    write_string(&mut b, msg);
    b.push_str("}\n");
    json(status, b)
}

fn not_allowed(allow: &str) -> Response {
    error(405, "method not allowed").header("Allow", allow)
}

#[cfg(test)]
mod tests {
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
