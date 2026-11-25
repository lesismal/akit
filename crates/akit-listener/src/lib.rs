#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's listeners on akio: TCP, UDP, TLS and QUIC entry points, and protocol splitting with
//! akio-mux.
//!
//! So far (I05): HTTP/1.1 and h2c listeners from the graph's config, each an akio TCP listener
//! serving a [`Dispatcher`], which routes every request through the current graph of a
//! [`GraphCell`] and answers 404 when no route matches. I09: a listener with `tls` serves HTTPS,
//! its HTTP acceptor wrapped in akio-tls with the configuration the graph loaded (certificates
//! by SNI, ALPN choosing h2 or http/1.1, client authentication); since I30 its certificates are
//! chosen per handshake by an [`akit_runtime::ListenerCerts`], from the current graph. Since I11 the routes go by the
//! request's canonical path, as fit's listener does: dot and empty segments removed, a path
//! climbing above the root or hiding a separator in an escape answered `400 bad request path`.
//! I10: a TLS listener serving `h3` serves HTTP/3 on akio-h3, over QUIC on its port in UDP, with
//! the certificates its TCP connections serve; its HTTP/1.1 and HTTP/2 responses announce it with
//! `Alt-Svc`, as fit's do.
//!
//! I21: a graph with an access log has every request's line written once its response is done,
//! by a response filter the [`Dispatcher`] registers: it runs on the thread that sends the
//! response (the I/O thread, or the one a detached handler answers from), and builds the line
//! there, from the request and the response ([`access_record`]).
//!
//! I22: [`Listeners::reload`] publishes a new graph and moves the listeners to its (see
//! [`reload`]).
//!
//! I37: a graph whose config has `metrics` has each request counted by listener and route
//! (`fit_http_requests_total`, `fit_http_request_duration_seconds`): one answered before its
//! handler returns (an inline handler's, the 404, the 400) once the call returns, with no hook;
//! any other by a response filter, the access log's when there is one. [`metrics::Scrape`] writes
//! every metric, in fit's names, for the Admin API's `/metrics`.
//!
//! I16: each listener takes request bodies up to the config's `limits.max_body_bytes` (fit's
//! 16 MiB by default) over HTTP/1.1, HTTP/2 and HTTP/3, and answers a larger one 413. A request
//! whose route's handler takes its body as it arrives (`body_mode`, the proxy's) gets it so,
//! with [`MAX_BODY_BYTES`] telling the handler the limit, which it holds the body to; one that
//! declares a length past it is answered 413 before its handler runs, as before.
//!
//! I17b: the config's `limits.write_stall_timeout` (fit's 30s by default) is the engine's write
//! timeout: a connection whose queued output makes no progress for that long is closed, between
//! one and two of it after its last progress (akio checks it about once a timeout). A reload
//! applies a new one to the connections already open ([`Listeners::reload`]). Unlike fit's,
//! which resets an HTTP/2 or HTTP/3 stream alone, it closes the connection, and it does not
//! bound HTTP/3 (akio's write timeout is TCP's).
//!
//! I51: a `tcp` listener is an akio TCP listener, and a `udp` one an akio UDP listener (each
//! client address a connection), whose connections go to the service the current graph has for
//! the listener, through akit-l4's [`akit_l4::ClientAcceptor`]. A reload that keeps such a
//! listener hands its new connections to the new graph's service, and closes those the old
//! graph's services still proxy once the drain timeout ([`Options::shutdown_timeout`]) has
//! passed, as fit's does.
//!
//! I52: a listener with `proxy_protocol`, of `tcp` or of HTTP over TCP, reads the PROXY header
//! of its trusted proxies' connections before anything else (before TLS), with akio-mux's
//! `ProxyProtocol`; the client it names is the relay's, or the request's. A `tcp` listener's
//! services may pick its connections by their ClientHello's server name (akit-l4's
//! [`akit_l4::Entry`]).
//!
//! I53: a listener that serves `tcp` beside HTTP tells them apart on its port by what a client
//! sends first, with akio-mux ([`sniff`]), as fit's does: a TLS ClientHello whose server name a
//! service of the listener takes goes to that service, passed through; any other TLS is served
//! HTTPS when the listener has `tls`, or else goes to the service without `server_names`; HTTP
//! (HTTP/1.1, and with `h2c` HTTP/2's preface), on a listener in the clear, is served; and
//! whatever else goes to the service without `server_names`, or is closed. A client that has not
//! sent enough to tell within [`akit_l4::OPENING_TIMEOUT`] is closed. A PROXY header comes before
//! all of it.
//!
//! I55: a listener serving `socks` serves akio-socks's SOCKS server with its `socks` rules
//! (akit-l4's [`akit_l4::forward`]), alone or beside HTTP and `tcp`, where a SOCKS5 or SOCKS4
//! greeting goes to it.
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

use std::borrow::Cow;
use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv6Addr, SocketAddr, TcpListener, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use akio::tls::Tls;
use akio::{AnyAcceptor, Backend, Config as EngineConfig, Engine, Listener as EngineListener};
use akio_http::{BodyMode, Context, HttpConfig, Method, Request, Response, Version};
use akit_config::{Listener, Protocol};
use akit_observe::metrics::Shard as MetricsShard;
use akit_observe::{AccessLog, Name, Record, SpanEnd, Trace, Tracer};
use akit_runtime::GraphCell;

pub mod metrics;
pub mod reload;
mod status;
pub use reload::{Listeners, ReloadError, Reloaded, Served, ServedView};

/// fit's answer when no route matches.
const NOT_FOUND: &str = "404 page not found\n";

/// fit's answer to a path that has no canonical form ([`akit_router::canonicalize`]).
const BAD_REQUEST_PATH: &str = "400 bad request path\n";

pub use akit_runtime::{MAX_BODY_BYTES, TLS, TRACEPARENT};

/// The akio service of one listener: each request goes to the route the current graph picks for
/// it on this listener, or is answered 404. Runs on the I/O thread, and only reads the graph.
pub struct Dispatcher {
    cell: GraphCell,
    listener: String,
    /// The listener's name, as each logged request carries it to its line.
    log_name: Name,
    tls: bool,
    /// The `Alt-Svc` value every response gets, for the TCP side of a listener that also serves
    /// HTTP/3; made once, when the listener is set up.
    alt_svc: Option<String>,
    /// The listener's `max_body_bytes`, as [`MAX_BODY_BYTES`] gives it to a handler that takes
    /// its body as it arrives.
    max_body_bytes: u64,
}

impl Dispatcher {
    /// The dispatcher of the listener named `listener`.
    #[must_use]
    pub fn new(cell: GraphCell, listener: impl Into<String>) -> Dispatcher {
        let listener = listener.into();
        Dispatcher {
            cell,
            log_name: Name::new(&listener),
            listener,
            tls: false,
            alt_svc: None,
            max_body_bytes: akit_config::DEFAULT_MAX_BODY_BYTES,
        }
    }

    /// The same, for a listener that serves TLS: its requests carry [`TLS`].
    #[must_use]
    pub fn tls(mut self, tls: bool) -> Dispatcher {
        self.tls = tls;
        self
    }

    /// The listener's `max_body_bytes`: a request body streamed to its handler is held to it
    /// ([`MAX_BODY_BYTES`]), and one that declares a length past it is answered 413 by akio
    /// before its handler runs. fit's 16 MiB unless set.
    #[must_use]
    pub fn max_body_bytes(mut self, n: u64) -> Dispatcher {
        self.max_body_bytes = n;
        self
    }

    /// Adds `Alt-Svc: <value>` to every response, as fit's dispatcher does on a listener that also
    /// serves HTTP/3 ([`alt_svc`]).
    #[must_use]
    pub fn with_alt_svc(mut self, value: impl Into<String>) -> Dispatcher {
        self.alt_svc = Some(value.into());
        self
    }
}

/// The `Alt-Svc` value announcing HTTP/3 on `port` of the same host, as fib's `http3.AltSvc`
/// writes it: `h3=":443"; ma=86400`.
#[must_use]
pub fn alt_svc(port: u16) -> String {
    format!("h3=\":{port}\"; ma=86400")
}

impl fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dispatcher")
            .field("listener", &self.listener)
            .field("alt_svc", &self.alt_svc)
            .finish_non_exhaustive()
    }
}

impl Dispatcher {
    /// Has `log` write the request's line, `count` count it (I37), and `traced` record its span
    /// (I64), once its response is done, from the thread that sends it, as fit's `logRequest`:
    /// one hook for the three. A request whose response is never sent (its connection went
    /// first) has no line, is not counted and has no span.
    ///
    /// The hook is a boxed closure, one allocation a request, until akio can call a listener's
    /// observer with a few words of the request's own (akio API request A29).
    fn track(
        &self,
        ctx: &mut Context,
        log: Option<&AccessLog>,
        count: Option<(Arc<MetricsShard>, u32)>,
        route: &str,
        traced: Option<(Tracer, Trace)>,
    ) {
        let start = Instant::now();
        if log.is_none() && traced.is_none() {
            if let Some((m, slot)) = count {
                ctx.add_response_filter(move |resp: &mut Response, _: &Request| {
                    m.observe(slot, resp.status, start.elapsed());
                });
            }
            return;
        }
        let shard = log.map(AccessLog::shard);
        // The target as it is now, canonical: a `rewrite` middleware changes the request's path
        // for the handlers, while the line writes the target fit's `RequestURI` keeps.
        let uri = shard.as_ref().map(|_| ctx.req().target.clone());
        let listener = self.log_name.clone();
        let route = Name::new(route);
        let tls = self.tls;
        // When the span began, in the time it is exported in: read for a traced request alone.
        let began = traced.as_ref().map(|_| SystemTime::now());
        ctx.add_response_filter(move |resp: &mut Response, req: &Request| {
            let took = start.elapsed();
            akit_observe::upstream(|upstream| {
                let mut r =
                    access_record(listener.as_str(), route.as_str(), took, upstream, resp, req);
                if let Some(uri) = &uri {
                    r.uri = uri;
                }
                if let Some(shard) = &shard {
                    shard.log(&r);
                }
                if let (Some((tracer, t)), Some(began)) = (&traced, began) {
                    tracer.end(t, &span_end(&r, req, tls, began));
                }
            });
            if let Some((m, slot)) = count {
                m.observe(slot, resp.status, took);
            }
        });
    }
}

/// The span of the request `r` records, as fit's `appendSpan` fills it: begun at `began`, ended
/// `r.duration` later.
fn span_end<'a>(r: &Record<'a>, req: &'a Request, tls: bool, began: SystemTime) -> SpanEnd<'a> {
    SpanEnd {
        method: r.method,
        route: r.route,
        path: req.path(),
        tls,
        host: r.host,
        client: r.client,
        version: match req.version {
            Version::Http10 => "1.0",
            Version::Http2 => "2",
            Version::Http3 => "3",
            _ => "1.1",
        },
        status: r.status,
        size: r.bytes_out,
        listener: r.listener,
        upstream: r.upstream.map_or("", |(a, _)| a),
        start: began,
        end: began.checked_add(r.duration).unwrap_or(began),
    }
}

/// Whether the current graph's tracer traces the request of `ctx`, by its `traceparent` (one
/// field of that name, or none), and if so its place in the trace, and the tracer; the upstream
/// is then sent the `traceparent` naming its span ([`TRACEPARENT`]). A field lookup for a request
/// not traced.
fn sample(g: &akit_runtime::Graph, ctx: &mut Context) -> Option<(Tracer, Trace)> {
    let tracer = g.tracer()?;
    let t = {
        let mut fields = ctx.req().headers.get_all("traceparent");
        let first = fields.next();
        let traceparent = if fields.next().is_some() { None } else { first };
        tracer.sample(traceparent)?
    };
    ctx.set_local(TRACEPARENT, t.traceparent());
    Some((tracer.clone(), t))
}

/// The access log's record of `req` answered with `resp`, as fit's `observe` fills its fields:
/// the client's address (an IPv4 address mapped into IPv6 written as IPv4, as Go has it), the
/// protocol as Go's `Request.Proto`, the `Host` (the target's authority for an absolute-form
/// target without one), the target as sent, the request body's bytes (its `Content-Length` for a
/// body not read whole), and the response body's: none for `HEAD` and for a status that has no
/// body, the file's for a file, the `Content-Length` for a streamed one (its bytes are not known
/// when its head is sent).
#[must_use]
pub fn access_record<'a>(
    listener: &'a str,
    route: &'a str,
    duration: Duration,
    upstream: Option<(&'a str, Duration)>,
    resp: &'a Response,
    req: &'a Request,
) -> Record<'a> {
    let content_length = |h: &akio_http::Headers| {
        h.get("content-length")
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(0)
    };
    let bytes_in = if req.body.is_empty() {
        content_length(&req.headers)
    } else {
        req.body.len() as u64
    };
    let bytes_out = if req.method == Method::Head || matches!(resp.status, 100..=199 | 204 | 304) {
        0
    } else if let Some(f) = &resp.file {
        f.len
    } else if resp.body.is_empty() {
        content_length(&resp.headers)
    } else {
        resp.body.len() as u64
    };
    let ip = req.peer.ip();
    let client = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        IpAddr::V4(_) => ip,
    };
    let host = req.headers.get("host").unwrap_or_else(|| {
        req.target
            .split_once("://")
            .map(|(_, rest)| rest.split(['/', '?']).next().unwrap_or(""))
            .unwrap_or("")
    });
    Record {
        listener,
        route,
        client,
        user: "",
        proto: match req.version {
            Version::Http10 => "HTTP/1.0",
            Version::Http11 => "HTTP/1.1",
            Version::Http2 => "HTTP/2.0",
            Version::Http3 => "HTTP/3.0",
            _ => "HTTP/1.1",
        },
        method: req.method.as_str(),
        host,
        uri: &req.target,
        status: resp.status,
        bytes_in,
        bytes_out,
        duration,
        upstream,
        user_agent: req.headers.get("user-agent").unwrap_or(""),
        referer: req.headers.get("referer").unwrap_or(""),
    }
}

impl akio_http::Service for Dispatcher {
    fn call(&self, ctx: &mut Context) {
        // On every response, the 400 and 404 below included, as fit adds it before routing.
        if let Some(v) = &self.alt_svc {
            ctx.add_response_header("alt-svc", v);
        }
        self.cell.with(|g| {
            let log = g.access_log();
            let listener = g.listener_index(&self.listener);
            // The registry, when the graph counts requests (I37).
            let metrics = g.metrics();
            // Whether it is traced (I64), whatever is made of it, as fit samples before routing.
            let traced = sample(g, ctx);
            // The routes go by the canonical path, as fit's do; a path that has none is answered
            // 400, and counted with the requests no route takes.
            let (route, changed) = {
                let Some(path) = akit_router::canonicalize(ctx.path()) else {
                    let slot = metrics.zip(listener.and_then(|i| g.request_slot(i, None)));
                    if log.is_some() || traced.is_some() {
                        self.track(ctx, log, None, "", traced);
                    }
                    let start = Instant::now();
                    ctx.text(400, BAD_REQUEST_PATH);
                    if let Some((m, slot)) = slot {
                        m.observe(slot, 400, start.elapsed());
                    }
                    return;
                };
                if !self.tls {
                    // The CA checks an HTTP-01 challenge in the clear; the answer is in memory
                    // (I31). A token with no challenge under way goes to the routes, as fit's.
                    if let Some(answer) = g.acme().and_then(|a| acme_answer(a, &path.decoded)) {
                        ctx.bytes(200, "text/plain", answer.into_bytes());
                        return;
                    }
                    // With a shared storage (I62), another instance's, looked up off this
                    // thread: the request is answered later, 404 when there is none, as fit's.
                    if let Some((a, token)) = g.acme().filter(|a| a.shared()).and_then(|a| {
                        path.decoded
                            .strip_prefix(akit_tls::ACME_CHALLENGE_PREFIX.as_bytes())
                            .and_then(|t| std::str::from_utf8(t).ok())
                            .map(|t| (a, t.to_owned()))
                    }) {
                        let client = ctx.req().peer.ip();
                        let Some(reply) = ctx.detach() else { return };
                        a.lookup_http_challenge(
                            &token,
                            Some(client),
                            Box::new(move |answer| {
                                reply.respond(match answer {
                                    Some(a) => {
                                        Response::with_body(200, "text/plain", a.into_bytes())
                                    }
                                    None => Response::text(404, NOT_FOUND),
                                });
                            }),
                        );
                        return;
                    }
                }
                let req = RouterRequest {
                    req: ctx.req(),
                    path: &path.decoded,
                };
                let route = listener.and_then(|i| g.route_request_at(i, &req));
                let changed = match path.escaped {
                    Cow::Owned(p) => Some(p),
                    Cow::Borrowed(_) => None,
                };
                (route, changed)
            };
            if let Some(p) = changed {
                // What the handlers and the access log go by, as fit's listener changes the
                // request's path and `RequestURI` (akio ADR-126).
                akit_runtime::set_request_path(ctx, &p);
            }
            if self.tls {
                ctx.set_local(TLS, "");
            }
            if ctx.body_reads().is_some() {
                // Its body streams (`body_mode`): the handler holds it to the limit.
                ctx.set_local(MAX_BODY_BYTES, self.max_body_bytes.to_string());
            }
            let slot = metrics.zip(listener.and_then(|i| g.request_slot(i, route.map(|(i, _)| i))));
            // A request answered before its handler returns (fit's inline ones, and the 404) is
            // counted then, with no hook; any other is counted by the hook on its response.
            let inline = route.is_none_or(|(_, r)| r.handler.inline());
            // A request logged or traced has one hook for its line, its span and its count.
            let hook = log.is_some() || traced.is_some();
            let hooked = match slot {
                Some((m, slot)) if !inline || hook => m.shard().map(|s| (s, slot)),
                _ => None,
            };
            let start = Instant::now();
            if hook {
                self.track(
                    ctx,
                    log,
                    hooked,
                    route.and_then(|(_, r)| r.name.as_deref()).unwrap_or(""),
                    traced,
                );
                match route {
                    Some((_, r)) => akit_observe::dispatch_logged(|| r.handler.call(ctx)),
                    None => unmatched(ctx, redirect_port(g, listener)),
                }
            } else {
                if hooked.is_some() {
                    self.track(ctx, None, hooked, "", None);
                }
                match route {
                    Some((_, r)) => r.handler.call(ctx),
                    None => unmatched(ctx, redirect_port(g, listener)),
                }
                if let (true, Some((m, slot)), Some(status)) = (inline, slot, ctx.status()) {
                    m.observe(slot, status, start.elapsed());
                }
            }
        });
    }

    /// The route's handler's: a request whose route takes its body as it arrives (the proxy's,
    /// I16) gets it so, unless it declares a length past `max_body_bytes`, which is then read
    /// as buffered bodies are, and answered 413 by akio. Others are buffered (akio's default),
    /// within the listener's `max_body_bytes` (`http_config`).
    ///
    /// The route is looked up here and again in [`call`](Self::call), on the graph current each
    /// time; a reload in between can hand a streamed body to a route that buffers, which then
    /// sees it empty.
    fn body_mode(&self, req: &Request) -> BodyMode {
        self.cell.with(|g| {
            let Some(path) = akit_router::canonicalize(req.path()) else {
                return BodyMode::Buffered;
            };
            let rr = RouterRequest {
                req,
                path: &path.decoded,
            };
            let Some(route) = g
                .listener_index(&self.listener)
                .and_then(|i| g.route_request(i, &rr))
            else {
                return BodyMode::Buffered;
            };
            match route.handler.body_mode(req) {
                BodyMode::Stream
                    if declared_length(req).is_some_and(|n| n > self.max_body_bytes) =>
                {
                    BodyMode::Buffered
                }
                mode => mode,
            }
        })
    }
}

/// The HTTPS port the listener at `listener` redirects to when HTTPS by default made it (I33),
/// or 0: read only for a request no route takes.
fn redirect_port(g: &akit_runtime::Graph, listener: Option<usize>) -> u16 {
    listener
        .and_then(|i| g.listeners().get(i))
        .map_or(0, |l| l.redirect_https)
}

/// The answer to a request no route takes: a redirect to HTTPS on `redirect` when it is not 0
/// (the listener HTTPS by default made), 404 otherwise.
fn unmatched(ctx: &mut Context, redirect: u16) {
    if redirect == 0 {
        ctx.text(404, NOT_FOUND);
        return;
    }
    let req = ctx.req();
    let location = https_location(req.headers.get("host"), &req.target, redirect);
    match location {
        Some(l) => {
            let mut resp = Response::new(308);
            resp.headers.set("Location", l);
            ctx.respond(resp);
        }
        None => ctx.text(400, NO_HOST),
    }
}

/// fit's answer to a request to redirect that names no host.
const NO_HOST: &str = "400 no host to redirect to\n";

/// Where a request in the clear to `host` (its `Host` field, if any) for `target` is redirected
/// on HTTPS port `port`, as fit's `redirectHTTPS`: the same host without its port, and the same
/// target; `None` when there is no host. A target in absolute form gives its host when there is
/// no `Host`, and its path and query.
fn https_location(host: Option<&str>, target: &str, port: u16) -> Option<String> {
    // A target in absolute form: its authority and the rest.
    let (authority, rest) = match target.split_once("://") {
        Some((_, after)) if !target.starts_with('/') => {
            let end = after.find(['/', '?']).unwrap_or(after.len());
            (after.get(..end), after.get(end..).unwrap_or(""))
        }
        _ => (None, target),
    };
    let host = host.filter(|h| !h.is_empty()).or(authority).unwrap_or("");
    let host = strip_port(host);
    if host.is_empty() {
        return None;
    }
    let mut location =
        String::with_capacity(host.len().saturating_add(rest.len()).saturating_add(16));
    location.push_str("https://");
    if host.contains(':') && !host.starts_with('[') {
        // An IPv6 address, from a host and port.
        location.push('[');
        location.push_str(host);
        location.push(']');
    } else {
        location.push_str(host);
    }
    if port != 443 {
        location.push(':');
        location.push_str(&port.to_string());
    }
    if !rest.starts_with('/') {
        location.push('/');
    }
    location.push_str(rest);
    Some(location)
}

/// `host` without its port, as Go's `net.SplitHostPort` takes one apart (and `host` as it is
/// when it has none): `[::1]:80` gives `::1`, `a.example:80` `a.example`.
fn strip_port(host: &str) -> &str {
    if let Some(inner) = host.strip_prefix('[') {
        return match inner.split_once("]:") {
            Some((h, _)) => h,
            None => host,
        };
    }
    match host.split_once(':') {
        Some((h, p)) if !p.contains(':') => h,
        _ => host,
    }
}

/// The answer to the HTTP-01 challenge a request for the canonical `path` asks for, if `path` is
/// under [`akit_tls::ACME_CHALLENGE_PREFIX`] and its token's challenge is under way.
fn acme_answer(acme: &akit_tls::Acme, path: &[u8]) -> Option<String> {
    let token = path.strip_prefix(akit_tls::ACME_CHALLENGE_PREFIX.as_bytes())?;
    acme.http_challenge(std::str::from_utf8(token).ok()?)
}

/// The body length `req` declares (`Content-Length`), if any.
fn declared_length(req: &Request) -> Option<u64> {
    req.headers
        .get("content-length")
        .and_then(|v| v.trim().parse().ok())
}

/// A request as the router's matchers read it: akio's request, with the canonical path.
struct RouterRequest<'a> {
    req: &'a Request,
    path: &'a [u8],
}

impl akit_router::Request for RouterRequest<'_> {
    fn method(&self) -> &str {
        self.req.method.as_str()
    }
    fn host(&self) -> &str {
        self.req.headers.get("host").unwrap_or("")
    }
    fn path(&self) -> &[u8] {
        self.path
    }
    fn query(&self) -> &str {
        self.req.query().unwrap_or("")
    }
    fn remote_ip(&self) -> Option<IpAddr> {
        Some(self.req.peer.ip())
    }
    fn any_header(&self, name: &str, f: impl FnMut(&str) -> bool) -> bool {
        self.req.headers.get_all(name).any(f)
    }
}

/// Why the listeners could not be set up.
#[derive(Debug)]
pub enum Error {
    /// A listener's address does not resolve.
    Address {
        /// The listener's name.
        listener: String,
        /// Its address.
        address: String,
        /// Why.
        error: io::Error,
    },
    /// A listener asks for what akit does not support yet.
    NotSupportedYet {
        /// The listener's name.
        listener: String,
        /// What.
        what: String,
    },
    /// akio could not build or start the engine (an address in use, say).
    Engine(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Address {
                listener,
                address,
                error,
            } => write!(f, "listener {listener}: address {address:?}: {error}"),
            Error::NotSupportedYet { listener, what } => {
                write!(f, "listener {listener}: not supported yet: {what}")
            }
            Error::Engine(e) => write!(f, "listening: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Address { error, .. } | Error::Engine(error) => Some(error),
            Error::NotSupportedYet { .. } => None,
        }
    }
}

/// Where a listener's `address` listens: `":8080"` on every address, IPv6 and IPv4 (akio binds
/// `[::]` dual-stack), as Go's `net.Listen` does; a host name resolved, its first address taken.
/// Resolving may block: call it on the control plane.
///
/// # Errors
///
/// When the address does not resolve.
pub fn resolve(address: &str) -> io::Result<SocketAddr> {
    if let Some(port) = address.strip_prefix(':') {
        let port = port
            .parse::<u16>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "not a port"))?;
        return Ok(SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), port));
    }
    // "[::1]:80", "127.0.0.1:80" and "localhost:80" as std takes them.
    address
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "resolves to no address"))
}

/// The akio HTTP settings of a listener serving `protocols`.
fn http_config(l: &Listener) -> Result<HttpConfig, Error> {
    let h1 = l.protocols.contains(&Protocol::Http11);
    let h2c = l.protocols.contains(&Protocol::H2c);
    // Over TLS, ALPN picks h2 (akit_tls::alpn): a client that negotiates it sends HTTP/2's
    // preface. One that offers no ALPN is served HTTP/1.1 even by an h2-only listener, as Go's
    // server serves it.
    let h2 = l.protocols.contains(&Protocol::H2);
    if h2c && !h1 {
        // fit serves HTTP/2 alone then; akio's server always takes HTTP/1 as well.
        return Err(Error::NotSupportedYet {
            listener: l.name.clone(),
            what: "protocols h2c without http/1.1".into(),
        });
    }
    let mut cfg = HttpConfig::default();
    cfg.max_body_bytes = body_limit(l);
    match &mut cfg.h2 {
        Some(h2cfg) if h2c || h2 => h2cfg.max_body_bytes = cfg.max_body_bytes,
        _ => cfg.h2 = None,
    }
    Ok(cfg)
}

/// The listener's `limits.max_body_bytes` for akio, which answers a body past it 413 (one that
/// declares its length, at once; a chunked one, once it passes it), as fit's listener does.
fn body_limit(l: &Listener) -> usize {
    usize::try_from(l.max_body_bytes).unwrap_or(usize::MAX)
}

/// The akio HTTP/3 settings of a listener serving `h3`.
fn h3_config(l: &Listener) -> akio_h3::H3Config {
    let mut cfg = akio_h3::H3Config::default();
    cfg.max_body_bytes = body_limit(l);
    cfg
}

/// How the listeners are served.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Options {
    /// The akio backend of every listener (epoll, kqueue on macOS, by default).
    pub backend: Backend,
    /// How long [`shutdown`] waits for requests in flight before closing their connections:
    /// fit's `-shutdown-timeout`, 30 s by default.
    pub shutdown_timeout: Duration,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            backend: Backend::default(),
            shutdown_timeout: Duration::from_secs(30),
        }
    }
}

/// Whether a listener serves HTTP over TCP: HTTP/1.1, h2c or h2.
fn serves_tcp(l: &Listener) -> bool {
    l.protocols
        .iter()
        .any(|p| matches!(p, Protocol::Http11 | Protocol::H2c | Protocol::H2))
}

/// Whether a listener serves HTTP/3, over QUIC in UDP.
fn serves_h3(l: &Listener) -> bool {
    l.protocols.contains(&Protocol::H3)
}

/// Whether a listener's connections go to a layer 4 service, over TCP (I51).
fn serves_l4_tcp(l: &Listener) -> bool {
    l.protocols.contains(&Protocol::Tcp)
}

/// Whether a listener's sessions go to a layer 4 service, over UDP (I51).
fn serves_l4_udp(l: &Listener) -> bool {
    l.protocols.contains(&Protocol::Udp)
}

/// Whether a listener serves the `socks` forward proxy (I55).
fn serves_socks(l: &Listener) -> bool {
    l.protocols.contains(&Protocol::Socks)
}

/// Whether a listener has a TCP socket in the engine: for HTTP over TCP, a service, or SOCKS.
fn tcp_side(l: &Listener) -> bool {
    serves_tcp(l) || serves_l4_tcp(l) || serves_socks(l)
}

/// Whether a listener has a UDP socket in the engine: for HTTP/3, or a service.
fn udp_side(l: &Listener) -> bool {
    serves_h3(l) || serves_l4_udp(l)
}

/// The acceptor of a `tcp` or `udp` listener: each connection to a service the cell's current
/// graph has for it (I51), by the server name of its ClientHello (I52).
fn l4_acceptor(
    cell: &GraphCell,
    l: &Listener,
) -> akit_l4::ClientAcceptor<impl Fn() -> Option<akit_l4::Entry> + Send + Sync + 'static> {
    let cell = cell.clone();
    let name = l.name.clone();
    let find = move || cell.with(|g| g.listener_index(&name).and_then(|i| g.entry(i)).cloned());
    if serves_l4_udp(l) {
        akit_l4::ClientAcceptor::udp(find)
    } else {
        akit_l4::ClientAcceptor::tcp(find)
    }
}

/// The SOCKS server of a listener serving `socks` (I55): its `socks` rules, fixed for as long as
/// the listener runs (a reload that changes them binds the listener anew, as fit's does), its
/// tunnels dialed through the engine the graph's slot is filled with once `akit run` has built
/// it. `None` for a listener without `socks`, which akit-config refuses for one serving it.
fn socks_acceptor(cell: &GraphCell, l: &Listener) -> Option<akio_socks::Socks> {
    let rules = akit_l4::forward::Rules::new(l.socks.as_ref()?);
    let engine = akio::EngineRef::new();
    let set = engine.clone();
    cell.with(|g| g.engine().when_set(move |e| set.set(e)));
    Some(rules.socks(engine))
}

/// How long akio keeps a UDP peer that sends nothing: at least as long as the longest idle
/// timeout of the graph's services, which close their sessions themselves. A stopgap for akio
/// API request A78 (an idle timeout per connection): akio's is the engine's.
fn udp_idle(g: &akit_runtime::Graph, now: Duration) -> Duration {
    g.services()
        .iter()
        .map(akit_l4::Service::udp_idle_timeout)
        .fold(now, Duration::max)
}

/// How many ports [`engine`] tries for listeners at port 0 that serve HTTP/3 beside TCP, whose
/// UDP port is the one TCP gets and may be taken (fit's `maxPortZeroBinds`).
const MAX_PORT_ZERO_BINDS: usize = 8;

/// Where one listener listens, as [`addrs`] gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Addrs {
    /// Its TCP address, for a listener serving HTTP/1.1, h2c or h2.
    pub tcp: Option<SocketAddr>,
    /// Its UDP address, where HTTP/3 listens, for a listener serving h3: the same port as TCP's.
    pub udp: Option<SocketAddr>,
}

impl Addrs {
    /// The address to report: TCP's, or UDP's for a listener serving HTTP/3 alone (fit's
    /// `Addr.Addr`).
    #[must_use]
    pub fn addr(&self) -> Option<SocketAddr> {
        self.tcp.or(self.udp)
    }
}

/// Where each of `listeners` (the graph's [`engine`] was built from) listens on `engine`, in
/// their order.
#[must_use]
pub fn addrs(listeners: &[Listener], engine: &Engine) -> Vec<Addrs> {
    let mut bound = engine.local_addrs().into_iter();
    listeners
        .iter()
        .map(|l| Addrs {
            tcp: if tcp_side(l) { bound.next() } else { None },
            udp: if udp_side(l) { bound.next() } else { None },
        })
        .collect()
}

/// An akio engine serving every listener of the cell's current graph, each with its own
/// [`Dispatcher`], and hosting the connections of its upstream pools (on the last listener's
/// threads); built, not started. A listener serving `h3` serves HTTP/3 over UDP on the port its
/// TCP side listens on, which the `Alt-Svc` of its TCP responses names. [`addrs`] gives where
/// each listens, in the graph's order ([`Engine::local_addrs`] lists each listener's TCP address,
/// then its UDP one).
///
/// # Errors
///
/// When an address does not resolve, a listener asks for what akit does not support yet, or akio
/// cannot build the engine (an address in use).
pub fn engine(cell: &GraphCell, opts: &Options) -> Result<Engine, Error> {
    let mut last = None;
    for _ in 0..MAX_PORT_ZERO_BINDS {
        match build(cell, opts) {
            // A listener at port 0 serving h3 beside TCP: the port probed free for both was
            // taken before akio bound it. Another one, as fit's bind tries.
            Err((Error::Engine(e), true)) if e.kind() == io::ErrorKind::AddrInUse => {
                last = Some(e);
            }
            Ok(e) => return Ok(e),
            Err((e, _)) => return Err(e),
        }
    }
    Err(Error::Engine(
        last.unwrap_or_else(|| io::ErrorKind::AddrInUse.into()),
    ))
}

/// [`engine`]'s one try; the error says too whether a port was probed for a listener at port 0.
fn build(cell: &GraphCell, opts: &Options) -> Result<Engine, (Error, bool)> {
    type Loaded = (Listener, Option<akit_tls::ServerConfigs>);
    // Each TLS listener's configurations, its certificates served from the current graph per
    // handshake (I30), so that a reload keeping the listener changes them.
    let listeners: Vec<Loaded> = cell
        .with(|g| {
            g.listeners()
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    let tls = akit_runtime::tls::server_configs(cell, g, i, false).transpose()?;
                    Ok((l.clone(), tls))
                })
                .collect::<Result<_, akit_tls::Error>>()
        })
        .map_err(|e| (tls_error(&e), false))?;
    let mut probed = false;
    let mut cfg = EngineConfig::default();
    cfg.shutdown_timeout = opts.shutdown_timeout;
    // The upstream pools dial on this engine. akio's client honours an upstream's
    // connect_timeout only for a name that resolves to several addresses, and dials a lone one
    // with the engine's (akio API request A12): until then the engine's is the shortest of the
    // upstreams', which is each one's own when they agree (fit's default, 10s, when none sets it).
    if let Some(t) = cell.with(|g| g.upstreams().iter().map(|p| p.connect_timeout()).min()) {
        cfg.connect_timeout = Some(t);
    }
    // fit's limits.write_stall_timeout is akio's write timeout: a connection whose queued output
    // makes no progress for it is closed (a reload changes it, `Listeners::reload`).
    cfg.write_timeout = Some(cell.with(akit_runtime::Graph::write_stall_timeout));
    cfg.udp_idle_timeout = cell.with(|g| udp_idle(g, cfg.udp_idle_timeout));
    let mut b = Engine::builder().config(cfg);
    for (l, tls) in &listeners {
        let fail = |e: Error| (e, false);
        let mut addr = resolve(&l.address).map_err(|error| {
            fail(Error::Address {
                listener: l.name.clone(),
                address: l.address.clone(),
                error,
            })
        })?;
        let (tcp, h3) = (tcp_side(l), serves_h3(l));
        if tcp && h3 && addr.port() == 0 {
            // HTTP/3 goes on the port TCP gets, the one Alt-Svc names: a port free for both.
            probed = true;
            let port = free_port(addr.ip()).map_err(|e| (Error::Engine(e), true))?;
            addr.set_port(port);
        }
        if tcp {
            let listener = EngineListener::tcp(addr).backend(opts.backend.clone());
            b = b.listen(
                tcp_listener(
                    cell,
                    l,
                    tls.as_ref().map(|c| &c.tcp),
                    listener,
                    h3.then_some(addr.port()),
                )
                .map_err(fail)?,
            );
        }
        if h3 {
            // Per-peer UDP listeners run on akio's epoll (kqueue) UDP loop whatever the TCP
            // backend: the backend is left to akio.
            b = b.listen(EngineListener::udp(addr).serve(
                h3_acceptor(cell, l, tls.as_ref().and_then(|c| c.h3.as_ref())).map_err(fail)?,
            ));
        }
        if serves_l4_udp(l) {
            b = b.listen(EngineListener::udp(addr).serve(l4_acceptor(cell, l)));
        }
    }
    // The HTTP/3 of a listener a reload adds is bound on the engine's UDP threads
    // (`Engine::bind_udp`, I22): those of the last listener serving h3, or one of their own when
    // none does. (akio adds TCP listeners at run time, A1, not UDP ones.)
    // UDP services dial their upstreams there too (I51).
    if !listeners.iter().any(|(l, _)| udp_side(l)) {
        b = b.udp_client_threads(1);
    }
    b.build().map_err(|e| (Error::Engine(e), probed))
}

/// The TCP side of listener `l` on `listener` (its address and backend set): HTTP/1.1 and h2c,
/// or HTTPS with `tls` (the graph's, loaded), its `tcp` service and its `socks` forward proxy,
/// told apart by [`sniff`] when it serves more than one of them; `h3_port` names the port of its
/// HTTP/3, which its responses' `Alt-Svc` advertises.
fn tcp_listener(
    cell: &GraphCell,
    l: &Listener,
    tls: Option<&akio::tls::ServerConfig>,
    listener: EngineListener,
    h3_port: Option<u16>,
) -> Result<EngineListener, Error> {
    let web = if serves_tcp(l) {
        let mut d = Dispatcher::new(cell.clone(), l.name.clone())
            .tls(l.tls.is_some())
            .max_body_bytes(l.max_body_bytes);
        if let Some(port) = h3_port {
            d = d.with_alt_svc(alt_svc(port));
        }
        let http = akio_http::serve_with(http_config(l)?, d);
        Some(match (&l.tls, tls) {
            (None, _) => AnyAcceptor::new(http),
            (Some(_), Some(cfg)) => AnyAcceptor::new(Tls::server(cfg.clone(), http)),
            // The graph loads every listener's tls, or is not compiled.
            (Some(_), None) => return Err(not_loaded(l)),
        })
    } else {
        None
    };
    let socks = if serves_socks(l) {
        Some(socks_acceptor(cell, l).ok_or_else(|| not_loaded(l))?)
    } else {
        None
    };
    let acc = match (web, socks) {
        (Some(web), None) if !serves_l4_tcp(l) => web,
        (None, Some(socks)) if !serves_l4_tcp(l) => AnyAcceptor::new(socks),
        (None, None) => AnyAcceptor::new(l4_acceptor(cell, l)),
        (web, socks) => sniff(cell, l, web, socks).into_any(),
    };
    // The PROXY header comes before TLS, and before the sniffing.
    Ok(serve_tcp(l, listener, acc))
}

/// The acceptor of a listener serving more than one of HTTP, `tcp` and `socks` (I53, I55; fit's
/// `sniff`), which tells them apart by what a client sends first with akio-mux, `web` being its
/// HTTP acceptor (HTTPS with `tls`) and `socks` its SOCKS server:
///
/// - with `tls`, a ClientHello whose server name a service of the listener takes (in the current
///   graph) goes to the listener's services, which pass it through to that one (akit-l4's
///   [`akit_l4::ClientAcceptor`], which reads the hello again), and any other TLS to `web`;
///   up to [`akit_l4::MAX_CLIENT_HELLO`] bytes are read for the hello;
/// - in the clear, TLS goes to the listener's services, which pick by its name (I52), HTTP/1.1
///   (and HTTP/2's preface with `h2c`) to `web`;
/// - a SOCKS5 or SOCKS4 greeting to `socks`;
/// - whatever else to the listener's services, whose service without `server_names` takes it,
///   or closes it when there is none (or the listener serves no `tcp`).
///
/// A client that has not sent enough to tell within [`akit_l4::OPENING_TIMEOUT`] is closed.
fn sniff(
    cell: &GraphCell,
    l: &Listener,
    web: Option<AnyAcceptor>,
    socks: Option<akio_socks::Socks>,
) -> akio_mux::Mux {
    use akio_mux::{Decision, Mux, Rule};
    let mut mux = Mux::new().sniff_timeout(Some(akit_l4::OPENING_TIMEOUT));
    match (&l.tls, web) {
        (Some(_), Some(web)) => {
            let (c, name) = (cell.clone(), l.name.clone());
            let named = move |h: &akio_mux::ClientHello<'_>| {
                h.server_name().is_some_and(|n| {
                    c.with(|g| {
                        g.listener_index(&name)
                            .and_then(|i| g.entry(i))
                            .is_some_and(|e| e.named(n).is_some())
                    })
                })
            };
            mux = mux
                .route(Rule::TlsHello(Arc::new(named)), l4_acceptor(cell, l))
                .max_sniff_bytes(akit_l4::MAX_CLIENT_HELLO)
                .route_any(Rule::Tls, web);
        }
        (_, web) => {
            mux = mux.route(Rule::Tls, l4_acceptor(cell, l));
            if let Some(web) = web {
                let http = if l.protocols.contains(&Protocol::H2c) {
                    Rule::Http
                } else {
                    Rule::Http1
                };
                mux = mux.route_any(http, web);
            }
        }
    }
    if let Some(socks) = socks {
        // SOCKS5 or SOCKS4 (and 4a): one server takes both.
        let greeting = |b: &[u8]| match (Rule::Socks5.decide(b), Rule::Socks4.decide(b)) {
            (Decision::Yes, _) | (_, Decision::Yes) => Decision::Yes,
            (Decision::NeedMore, _) | (_, Decision::NeedMore) => Decision::NeedMore,
            _ => Decision::No,
        };
        mux = mux.route(Rule::Custom(Arc::new(greeting)), socks);
    }
    mux.fallback(l4_acceptor(cell, l))
}

/// The most bytes of a PROXY header a listener reads (fit's `MaxProxyHeader`): a v1 line is at
/// most 107, and a v2 header's TLVs are left at what a proxy sends in practice.
pub const MAX_PROXY_HEADER: usize = 4096;

/// `listener` (TCP) serving `acc`, behind akio-mux's PROXY protocol layer when listener `l` has
/// `proxy_protocol` (I52): a connection from one of its trusted proxies must begin with a header,
/// of version 1 or 2, of at most [`MAX_PROXY_HEADER`] bytes within
/// [`akit_l4::OPENING_TIMEOUT`], or is closed, and is served as from the client it names (the
/// request's peer: akio API request A105); one from elsewhere is served as it is.
fn serve_tcp(l: &Listener, listener: EngineListener, acc: AnyAcceptor) -> EngineListener {
    let Some(pp) = l.proxy_protocol.clone() else {
        return listener.serve_any(acc);
    };
    listener.serve_any(
        akio_mux::proxy::ProxyProtocol::new_any(acc)
            .trusted(move |a| pp.trusts(a.ip()))
            .header_timeout(Some(akit_l4::OPENING_TIMEOUT))
            .max_header_bytes(MAX_PROXY_HEADER),
    )
}

/// The HTTP/3 acceptor of listener `l`, with its h3 TLS (the graph's, loaded).
fn h3_acceptor(
    cell: &GraphCell,
    l: &Listener,
    h3_tls: Option<&akio::tls::ServerConfig>,
) -> Result<impl akio::Acceptor, Error> {
    // akit-config refuses h3 without tls; the graph loads its h3 TLS with the rest.
    let Some(tls) = h3_tls else {
        return Err(not_loaded(l));
    };
    let quic = akio_quic::server_config_from(tls)
        .map_err(|e| Error::Engine(io::Error::other(e.to_string())))?;
    Ok(akio_h3::serve_with(
        quic,
        h3_config(l),
        // HTTP/3 is always over TLS.
        Dispatcher::new(cell.clone(), l.name.clone())
            .tls(true)
            .max_body_bytes(l.max_body_bytes),
    ))
}

/// A listener's TLS configuration refused by akio-tls (the graph built one when it was
/// compiled, so this is not expected).
fn tls_error(e: &akit_tls::Error) -> Error {
    Error::Engine(io::Error::other(e.to_string()))
}

fn not_loaded(l: &Listener) -> Error {
    Error::Engine(io::Error::other(format!(
        "listener {}: its tls was not loaded",
        l.name
    )))
}

/// A port on `ip` free for TCP and for UDP at once, for a listener at port 0 serving HTTP/3 on
/// the port TCP gets: TCP's free port, unless UDP's is taken, as fit's bind tries them. Binds
/// and closes sockets: control plane only. The port may still be taken before akio binds it;
/// [`engine`] then tries again.
fn free_port(ip: IpAddr) -> io::Result<u16> {
    let mut last = io::Error::from(io::ErrorKind::AddrInUse);
    for _ in 0..MAX_PORT_ZERO_BINDS {
        let tcp = TcpListener::bind(SocketAddr::new(ip, 0))?;
        let port = tcp.local_addr()?.port();
        match UdpSocket::bind(SocketAddr::new(ip, port)) {
            Ok(_) => return Ok(port),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Shuts `engine` down gracefully and waits until it has: the listeners stop accepting, idle
/// connections are closed, and a connection with requests in flight is closed once their
/// responses are sent (an HTTP/2 one gets `GOAWAY`), or when the engine's shutdown timeout
/// ([`Options::shutdown_timeout`], `timeout` here) passes. Returns false when it passed with
/// connections still open, which akio then closed.
pub fn shutdown(engine: &Engine, timeout: Duration) -> bool {
    let open = engine.telemetry().open_connections();
    let start = Instant::now();
    engine.shutdown();
    engine.wait();
    // akio waits as long as the timeout only for connections it then closes.
    open == 0 || start.elapsed() < timeout
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::net::SocketAddr;

    #[test]
    fn addresses() {
        assert_eq!(
            super::resolve(":8080").unwrap(),
            "[::]:8080".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            super::resolve("127.0.0.1:0").unwrap(),
            "127.0.0.1:0".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            super::resolve("[::1]:80").unwrap(),
            "[::1]:80".parse::<SocketAddr>().unwrap()
        );
        assert!(super::resolve("localhost:80").unwrap().ip().is_loopback());
        assert!(super::resolve(":x").is_err());
    }

    /// fit's `TestRedirectEdges` and the redirects of `TestAutoHTTPSRedirect`: the host without
    /// its port, an IPv6 one in brackets, the target kept, a target in absolute form taken apart,
    /// and no host refused.
    #[test]
    fn https_locations() {
        let l = super::https_location;
        for (host, target, port, want) in [
            (
                Some("a.example"),
                "/x?y=1",
                8443,
                Some("https://a.example:8443/x?y=1"),
            ),
            (
                Some("A.example:8080"),
                "/form",
                8443,
                Some("https://A.example:8443/form"),
            ),
            (Some("a.example:80"), "/", 443, Some("https://a.example/")),
            (Some("[::1]"), "/", 8443, Some("https://[::1]:8443/")),
            (Some("[::1]:8080"), "/x", 8443, Some("https://[::1]:8443/x")),
            (Some("::1"), "/x", 443, Some("https://[::1]/x")),
            (
                Some("other.example"),
                "/.well-known/acme-challenge/none",
                443,
                Some("https://other.example/.well-known/acme-challenge/none"),
            ),
            (
                Some("a.example"),
                "http://a.example/y?z=1",
                8443,
                Some("https://a.example:8443/y?z=1"),
            ),
            (
                None,
                "http://b.example:8080?q",
                443,
                Some("https://b.example/?q"),
            ),
            (None, "http://b.example", 443, Some("https://b.example/")),
            (None, "/x", 8443, None),
            (Some(""), "/x", 8443, None),
        ] {
            assert_eq!(l(host, target, port).as_deref(), want, "{host:?} {target}");
        }
    }
}
