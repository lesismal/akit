//! The Admin API's HTTP side: akio-http on an engine of its own, one I/O thread (decision 006).
//!
//! Reads (`GET /config`, `/versions`) are answered on the I/O thread from the history, which it
//! holds only to clone an `Arc`. A load's body is streamed in under `max_config_bytes`, then the
//! request is detached and handed, as a [`Job`], to the control thread through `submit`; that
//! thread applies it (binding listeners and reading files, which block) and answers it.

use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use akio::{Backend, Config as EngineConfig, Engine, Listener};
use akio_http::{BodyEvent, BodyMode, Context, Detached, HttpConfig, Method, Request, Response};
use akit_config::Errors;
use akit_config::value::{Value, write_string};

use crate::configs::{Applied, ApplyError, Configs, Outcome, Refused, SOURCE_ADMIN, Version};
use crate::configs::{ListenerChange, etag_matches};
use crate::pointer;

/// The header that carries the version of the config a response is about: fit's name, so that
/// fit's clients read it.
pub const VERSION_HEADER: &str = "Fit-Config-Version";

/// What the control thread is given: `submit` hands it a [`Job`], and gives it back when it
/// cannot be taken now (its queue is full), to be answered 503. Called on the API's I/O thread:
/// it must not block.
pub type Submit = Box<dyn Fn(Job) -> Result<(), Job> + Send + Sync>;

/// The Admin API's settings beside the config's: its timeouts and backend.
#[derive(Debug, Clone)]
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
}

impl Default for Options {
    fn default() -> Options {
        Options {
            backend: Backend::default(),
            head_timeout: Duration::from_secs(10),
            body_timeout: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(60),
        }
    }
}

/// The largest body of a request other than a load: none is read.
const OTHER_BODY_BYTES: usize = 64 << 10;

/// A load waiting for the control thread: its config, its `If-Match`, and the request to answer.
pub struct Job(Box<Load>);

struct Load {
    data: Vec<u8>,
    if_match: Option<String>,
    reply: Detached,
}

impl fmt::Debug for Job {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Job")
            .field("bytes", &self.0.data.len())
            .field("if_match", &self.0.if_match)
            .finish_non_exhaustive()
    }
}

impl Job {
    /// Applies the config through `configs` with `apply` (which loads, compiles and serves it:
    /// the control thread's) and answers the request as fit's API answers it. Returns what was
    /// applied, for the caller to report, or `None` when nothing was.
    pub fn run(
        self,
        configs: &Configs,
        apply: impl FnOnce(&str) -> Result<Applied, ApplyError>,
    ) -> Option<Outcome> {
        let Load {
            data,
            if_match,
            reply,
        } = *self.0;
        let text = match String::from_utf8(data) {
            Ok(t) => t,
            Err(_) => {
                reply.respond(error(400, "the config is not UTF-8"));
                return None;
            }
        };
        let std = match akit_config::value::standardize(&text) {
            Ok(s) => s,
            Err(errs) => {
                reply.respond(not_applied(&errs));
                return None;
            }
        };
        match configs.update(SOURCE_ADMIN, if_match.as_deref(), std, apply) {
            Ok(o) => {
                reply.respond(
                    json(200, loaded(&o))
                        .header("ETag", &o.etag)
                        .header(VERSION_HEADER, o.version.to_string()),
                );
                Some(o)
            }
            Err(Refused::Changed) => {
                reply.respond(error(
                    412,
                    "the config has changed since the ETag given in If-Match",
                ));
                None
            }
            Err(Refused::Apply(ApplyError::Config(errs))) => {
                reply.respond(not_applied(&errs));
                None
            }
            Err(Refused::Apply(ApplyError::Failed(msg))) => {
                reply.respond(error(
                    500,
                    &format!("the config was not applied; the running config stays: {msg}"),
                ));
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
    /// Listens where `settings` says and serves the Admin API for `configs`, handing loads to
    /// the control thread with `submit`. On the control plane: it binds a socket.
    ///
    /// # Errors
    ///
    /// When the address does not parse or cannot be bound.
    pub fn start(
        settings: &akit_config::Admin,
        configs: Configs,
        submit: Submit,
        opts: &Options,
    ) -> io::Result<Server> {
        let addr = resolve(&settings.address)?;
        let api = Api(Arc::new(Shared {
            configs,
            submit,
            max_body: settings.max_config_bytes,
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

struct Shared {
    configs: Configs,
    submit: Submit,
    max_body: usize,
}

struct Api(Arc<Shared>);

fn is_load(req: &Request) -> bool {
    req.method == Method::Post && req.path() == "/load"
}

impl akio_http::Service for Api {
    fn call(&self, ctx: &mut Context) {
        if let Some(resp) = guard(ctx) {
            ctx.respond(resp);
            return;
        }
        let path = ctx.path().to_owned();
        let method = ctx.method().clone();
        let read = matches!(method, Method::Get | Method::Head);
        let resp = match path.as_str() {
            "/load" if method == Method::Post => return load(&self.0, ctx),
            "/load" => not_allowed("POST"),
            "/versions" if read => versions(&self.0.configs),
            "/versions" => not_allowed("GET, HEAD"),
            "/reload" => not_yet("POST /reload"),
            p if p.starts_with("/rollback/") => not_yet("POST /rollback"),
            p if p.starts_with("/versions/") => {
                if read {
                    version(&self.0.configs, p.get("/versions/".len()..).unwrap_or(""))
                } else {
                    not_allowed("GET, HEAD")
                }
            }
            p if p == "/config" || p.starts_with("/config/") => match method {
                Method::Get | Method::Head => get_config(ctx, &self.0.configs, &path),
                Method::Put | Method::Patch | Method::Delete => {
                    not_yet(&format!("{} /config", method.as_str()))
                }
                _ => not_allowed("GET, HEAD"),
            },
            _ => error(404, "there is no such endpoint"),
        };
        ctx.respond(resp);
    }

    fn body_mode(&self, req: &Request) -> BodyMode {
        if is_load(req) {
            BodyMode::Stream
        } else {
            BodyMode::Buffered
        }
    }
}

/// What must not reach the API, refused as fit refuses it: a request from a web page, which a
/// browser marks with `Origin` (but a read from the API's own origin), and one whose `Host` is not
/// a loopback name, as a web page's is after DNS rebinding. (Without a token, I27, the API
/// listens on loopback only.)
fn guard(ctx: &Context) -> Option<Response> {
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
    if !loopback_host(host) {
        return Some(error(403, "the Host field must name a loopback host"));
    }
    None
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

/// `POST /load`: the body, JSON within `max_config_bytes`, streamed in and handed to the control
/// thread.
fn load(shared: &Arc<Shared>, ctx: &mut Context) {
    match ctx.req().query_param("dry_run") {
        None | Some("" | "0" | "false") => {}
        Some("1" | "true") => return ctx.respond(not_yet("dry_run")),
        Some(other) => {
            let msg = format!("dry_run is 1 or true, 0 or false, not {other:?}");
            return ctx.respond(error(400, &msg));
        }
    }
    let json_type = ctx.header("Content-Type").is_some_and(|t| {
        t.split(';')
            .next()
            .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"))
    });
    if !json_type {
        return ctx.respond(error(415, "the body must be sent as application/json"));
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
    let if_match = ctx.header("If-Match").map(str::to_owned);
    // The body as it arrives, within max: bounded by the length sent, when one is.
    let mut data = Vec::with_capacity(
        declared
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0)
            .min(max),
    );
    let shared = Arc::clone(shared);
    let mut over = false;
    let mut if_match = Some(if_match);
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
                let Some(reply) = ctx.detach() else { return };
                let job = Job(Box::new(Load {
                    data: std::mem::take(&mut data),
                    if_match: if_match.take().flatten(),
                    reply,
                }));
                if let Err(job) = (shared.submit)(job) {
                    job.refuse(503, "akit is busy applying other configs: try again");
                }
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
    let rest = path.get("/config".len()..).unwrap_or("");
    if rest.is_empty() || rest == "/" {
        return config(&v);
    }
    let Some(decoded) = akio_http::message::percent_decode(rest) else {
        return error(
            400,
            &format!("{rest:?} is not a JSON pointer: a bad escape"),
        );
    };
    let found = pointer::parse(&decoded).and_then(|tokens| {
        let root = akit_config::value::parse(v.data())
            .map_err(|_| pointer::Problem::new(500, "the running config is not JSON".into()))?;
        pointer::lookup(&root, &tokens).map(Value::to_json)
    });
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

/// A load's answer: its version and ETag, the listeners it opened and closed, its warnings.
fn loaded(o: &Outcome) -> String {
    let mut b = format!("{{\"version\":{},\"etag\":", o.version);
    write_string(&mut b, &o.etag);
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

/// 501 for what fit's API does and akit's does not yet: edits, rollback, reload and dry runs
/// come with I27.
fn not_yet(what: &str) -> Response {
    error(
        501,
        &format!("{what} is not supported yet (the Admin API's edits, I27)"),
    )
}

#[cfg(test)]
mod tests {
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
