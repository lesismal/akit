//! The `wasm` middleware, as fit's `plugin/wasm` `WASM`, with akit's own ABI ([`crate::abi`]) or
//! Proxy-Wasm's ([`crate::proxywasm`], [`crate::response`]): a WebAssembly plugin run for each
//! request, on a pool of instances of its own.
//!
//! - On the I/O thread, the request is copied as the plugin sees it (method, decoded path,
//!   query, host, client address, fields), suspended (akio's `Context::suspend`, ADR-109) and
//!   handed to the plugin's [`InstancePool`] (akio-wasm, ADR-125), which never waits: a full
//!   queue is answered 503 at once.
//! - On a pool thread, `akit_on_request` (or a Proxy-Wasm filter's context made, and its
//!   `proxy_on_request_headers`) runs under the call's deadline (`timeout`) and the instance's
//!   memory bound (`max_memory_bytes`). A call past its deadline traps, and its instance is made
//!   again for the next request.
//! - The pool's completion resumes the request on that thread: the request's fields as the
//!   plugin changed them become the request's (akio's `Context::req_mut`, ADR-126); then the
//!   plugin's answer is sent, with the response fields it set, or the request goes on to the
//!   next middleware and the handler, the plugin's changes to the response's fields made by a
//!   response filter on whatever response is sent. A Proxy-Wasm filter that follows the response
//!   keeps the request's context open on its instance for it ([`crate::response`]), as many at
//!   once as `max_streams`, past which a request is answered 503.
//! - A request that waited past `wait_timeout`, or that the pool closed on, is answered 503; a
//!   call that trapped (a panic, memory past its bound, its deadline passed) or broke the ABI is
//!   answered 500, or goes on without the plugin with `on_error: skip`.
//! - A client that goes while its request waits leaves its call unrun.
//! - A request whose body streams waits suspended with its body's reads held (akio's ADR-159),
//!   and streams to the handler once the plugin let it through; the plugin is not handed the
//!   request's body (fit's subset), only whether it has one, as fit tells it (`Content-Length`
//!   above 0 or a `Transfer-Encoding`, or a body already read).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use akio_http::{BodyMode, Context, Headers, Method, Request, Response, Suspended, Version};
use akio_wasm::{JobError, Pooled, Refusal};
use akit_config::WasmOnError;
use akit_runtime::{
    Handler, HandlerParams, Middleware, Provision, ProvisionError, Provisioned, TLS,
    set_request_headers,
};

use crate::abi::{ACTION_STOP, Call, Host, Op, apply};
use crate::plugin::Plugin;
use crate::proxywasm::{self, PwRequest, Ticket};
use crate::response::{Follow, local_response};

pub(crate) fn new(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Middleware>, ProvisionError> {
    let HandlerParams::Wasm(w) = p.params() else {
        return Err(ProvisionError::new(
            "the wasm middleware was given another's params",
        ));
    };
    let plugin = Arc::new(Plugin::start(w).map_err(|e| ProvisionError::field("path", e))?);
    let closing = Arc::clone(&plugin);
    Ok(Provisioned::new(Arc::new(WasmMiddleware {
        plugin,
        on_error: w.on_error,
    }) as Arc<dyn Middleware>)
    .on_cleanup(move || closing.close_later()))
}

/// The `wasm` middleware, compiled.
#[derive(Debug)]
struct WasmMiddleware {
    plugin: Arc<Plugin>,
    on_error: WasmOnError,
}

impl Middleware for WasmMiddleware {
    fn wrap(&self, next: Arc<dyn Handler>) -> Arc<dyn Handler> {
        Arc::new(Plugged {
            plugin: Arc::clone(&self.plugin),
            on_error: self.on_error,
            next,
        })
    }

    /// A plugin may set the request's path (akit's ABI, Proxy-Wasm's `:path`).
    fn changes_target(&self) -> bool {
        true
    }
}

/// A route's handler behind the plugin.
struct Plugged {
    plugin: Arc<Plugin>,
    on_error: WasmOnError,
    next: Arc<dyn Handler>,
}

impl akio_http::Service for Plugged {
    fn call(&self, ctx: &mut Context) {
        let call = match self.plugin.pw {
            Some(pw) => Snapshot::Pw(snapshot_pw(ctx, pw.follows())),
            None => Snapshot::Fit(snapshot(ctx)),
        };
        let gone = Arc::new(AtomicBool::new(false));
        let g = Arc::clone(&gone);
        ctx.on_gone(move || g.store(true, Ordering::Release));
        let Some(request) = ctx.suspend() else {
            // A resumed request already taking its streamed body.
            ctx.respond(Response::new(500));
            return;
        };
        let next = Arc::clone(&self.next);
        let on_error = self.on_error;
        let plugin = Arc::clone(&self.plugin);
        let job = move |p: &mut Pooled<Host>| match call {
            Snapshot::Fit(call) => run(p, call, &gone),
            Snapshot::Pw(req) => proxywasm::run_request(p, req, &gone),
        };
        let done = move |r: Result<Outcome, JobError>| {
            finish(request, verdict(r, on_error), next, &plugin);
        };
        if let Err(refused) = self.plugin.pool.submit(job, done) {
            // Never waited for: answered here, on the I/O thread, at once.
            let why = match refused.reason {
                Refusal::Failed(e) => JobError::Failed(e),
                // A full queue, or a pool closing with its config: 503, as fit's.
                _ => JobError::Closed,
            };
            (refused.done)(Err(why));
        }
    }

    fn body_mode(&self, req: &Request) -> BodyMode {
        self.next.body_mode(req)
    }

    fn streams_bodies(&self) -> bool {
        self.next.streams_bodies()
    }
}

impl Handler for Plugged {}

/// The request as a plugin of either ABI sees it.
enum Snapshot {
    Fit(Call),
    Pw(PwRequest),
}

/// The request as a Proxy-Wasm plugin sees it, copied on the I/O thread; `follows` says the
/// plugin follows responses, which a tunnel's (`CONNECT`, an upgrade) is not.
fn snapshot_pw(ctx: &Context, follows: bool) -> PwRequest {
    let req = ctx.req();
    let mut headers = req.headers.clone();
    headers.remove("host");
    let tunnel = req.method == Method::Connect || req.headers.get("upgrade").is_some();
    PwRequest {
        method: req.method.as_str().to_owned(),
        target: req.target.clone(),
        host: req.headers.get("host").unwrap_or("").to_owned(),
        scheme: if ctx.local(TLS).is_some() {
            "https"
        } else {
            "http"
        },
        protocol: match req.version {
            Version::Http10 => "HTTP/1.0",
            Version::Http2 => "HTTP/2.0",
            Version::Http3 => "HTTP/3.0",
            _ => "HTTP/1.1",
        },
        remote: req.peer.to_string(),
        headers,
        follows: follows && !tunnel,
        has_body: has_body(req),
    }
}

/// Whether `req` has a body, as fit tells a Proxy-Wasm filter (`end_of_stream` its opposite):
/// one read already, or one its fields declare, since a body that streams is not read yet.
fn has_body(req: &Request) -> bool {
    !req.body.is_empty()
        || req.headers.get("transfer-encoding").is_some()
        || req
            .headers
            .get("content-length")
            .and_then(|v| v.trim().parse::<u64>().ok())
            .is_some_and(|n| n > 0)
}

/// The request as the plugin sees it, copied on the I/O thread.
fn snapshot(ctx: &Context) -> Call {
    let headers = ctx.req().headers.clone();
    let req = ctx.req();
    let escaped = ctx.path();
    let path = akio_http::message::percent_decode(escaped).unwrap_or_else(|| escaped.to_owned());
    Call {
        method: req.method.as_str().to_owned(),
        path,
        query: req.query().unwrap_or("").to_owned(),
        host: headers.get("host").unwrap_or("").to_owned(),
        remote: req.peer.to_string(),
        headers,
        ..Call::default()
    }
}

/// What a call left: the request's fields, method and target if it changed them, its changes
/// to the response's fields, its answer if it gave one, and, for a Proxy-Wasm plugin that
/// follows the response, where the request's context is open.
#[derive(Debug, Default)]
pub(crate) struct Ran {
    pub(crate) headers: Option<Headers>,
    pub(crate) method: Option<String>,
    pub(crate) target: Option<String>,
    pub(crate) ops: Vec<Op>,
    pub(crate) answer: Option<(u16, Vec<u8>)>,
    pub(crate) follow: Option<Ticket>,
}

/// How a job on the pool ended.
#[derive(Debug)]
pub(crate) enum Outcome {
    /// The client went while the request waited: the plugin was not called.
    Gone,
    /// The plugin ran.
    Ran(Ran),
    /// The call trapped, or broke the ABI.
    Failed,
    /// The instance follows as many responses as it may (`max_streams`).
    Busy,
}

/// Runs the plugin for `call` on the pool's instance `p`.
pub(crate) fn run(p: &mut Pooled<Host>, call: Call, gone: &AtomicBool) -> Outcome {
    if gone.load(Ordering::Acquire) {
        return Outcome::Gone;
    }
    let Some(f) = p.state().on_request else {
        return Outcome::Failed;
    };
    p.state_mut().call = Some(call);
    let r = p.call_typed(f, ());
    let Some(call) = p.state_mut().call.take() else {
        return Outcome::Failed;
    };
    let Ok(action) = r else {
        return Outcome::Failed;
    };
    let status = if call.status == 0 { 200 } else { call.status };
    let answer = (call.status != 0 || action == ACTION_STOP).then_some((status, call.body));
    Outcome::Ran(Ran {
        headers: (call.request_ops > 0).then_some(call.headers),
        ops: call.ops,
        answer,
        ..Ran::default()
    })
}

/// What becomes of the request once its job is done with.
#[derive(Debug)]
pub(crate) enum Verdict {
    /// Its client went: nothing to answer.
    Gone,
    /// The plugin's changes made, and its answer sent or the request sent on.
    Apply(Ran),
    /// Sent on without the plugin (`on_error: skip`).
    Skip,
    /// Answered with this status.
    Status(u16),
}

/// The verdict on a job's result, `on_error` deciding a failed call's.
pub(crate) fn verdict(r: Result<Outcome, JobError>, on_error: WasmOnError) -> Verdict {
    let failed = || match on_error {
        WasmOnError::Fail => Verdict::Status(500),
        WasmOnError::Skip => Verdict::Skip,
    };
    match r {
        Ok(Outcome::Gone) => Verdict::Gone,
        Ok(Outcome::Ran(ran)) => Verdict::Apply(ran),
        Ok(Outcome::Busy) => Verdict::Status(503),
        // A trap, a panic, or an instance that could not be made again.
        Ok(Outcome::Failed) | Err(JobError::Panicked | JobError::Failed(_) | JobError::Remade) => {
            failed()
        }
        // Waited past wait_timeout, or the pool closed with its config.
        Err(_) => Verdict::Status(503),
    }
}

/// Resumes the request with its verdict: on the pool's thread, which the next handlers then run
/// on; they never block (rule 1).
fn finish(request: Suspended, v: Verdict, next: Arc<dyn Handler>, plugin: &Arc<Plugin>) {
    match v {
        Verdict::Gone => drop(request),
        Verdict::Status(status) => request.resume(move |ctx| ctx.respond(plain(status))),
        Verdict::Skip => request.resume(move |ctx| next.call(ctx)),
        Verdict::Apply(mut ran) => {
            // Made here, so that a request whose client went before it resumes still has its
            // context closed, as the follow is dropped.
            let follow = ran
                .follow
                .take()
                .map(|ticket| Follow::new(Arc::clone(plugin), ticket));
            request.resume(move |ctx| apply_ran(ctx, ran, follow, &next));
        }
    }
}

/// Makes the plugin's changes to the request, and answers it or sends it on.
fn apply_ran(ctx: &mut Context, ran: Ran, follow: Option<Arc<Follow>>, next: &Arc<dyn Handler>) {
    if ran.method.is_some() || ran.target.is_some() {
        // Through akio's `Context::req_mut` (ADR-126), as the fields.
        if let Some(r) = ctx.req_mut() {
            if let Some(m) = ran.method {
                r.method = Method::parse(&m);
            }
            if let Some(t) = ran.target {
                r.target = t;
            }
        }
    }
    if let Some(h) = ran.headers {
        set_request_headers(ctx, h);
    }
    if let Some((status, body)) = ran.answer {
        ctx.respond(local_response(status, body, &ran.ops));
        return;
    }
    if let Some(f) = follow {
        f.hook(ctx, ran.ops);
    } else if !ran.ops.is_empty() {
        let ops = ran.ops;
        ctx.add_response_filter(move |resp: &mut Response, _: &Request| {
            apply(&ops, &mut resp.headers);
        });
    }
    next.call(ctx);
}

/// An answer of `status` with its reason as body, as fit's.
fn plain(status: u16) -> Response {
    let body = match status {
        503 => "503 Service Unavailable\n",
        _ => "500 Internal Server Error\n",
    };
    Response::with_body(
        status,
        "text/plain; charset=utf-8",
        body.as_bytes().to_vec(),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn verdicts() {
        use WasmOnError::{Fail, Skip};
        let ran = || Ok(Outcome::Ran(Ran::default()));
        assert!(matches!(verdict(ran(), Fail), Verdict::Apply(_)));
        assert!(matches!(verdict(Ok(Outcome::Gone), Fail), Verdict::Gone));
        // An instance following as many responses as it may: 503, as fit's TestProxyWasmMaxStreams.
        assert!(matches!(
            verdict(Ok(Outcome::Busy), Skip),
            Verdict::Status(503)
        ));
        assert!(matches!(
            verdict(Ok(Outcome::Failed), Fail),
            Verdict::Status(500)
        ));
        assert!(matches!(verdict(Ok(Outcome::Failed), Skip), Verdict::Skip));
        // An instance that could not be made again fails the request (fit's TestInstanceLost).
        let failed = JobError::Failed(Arc::new(akio_wasm::StartError::Panicked));
        assert!(matches!(
            verdict(Err(failed.clone()), Fail),
            Verdict::Status(500)
        ));
        assert!(matches!(verdict(Err(failed), Skip), Verdict::Skip));
        assert!(matches!(
            verdict(Err(JobError::Panicked), Fail),
            Verdict::Status(500)
        ));
        // Waited too long, or still queued as the pool closed (fit's TestCloseQueued): 503
        // whatever on_error says.
        for e in [JobError::Expired, JobError::Closed] {
            assert!(matches!(
                verdict(Err(e.clone()), Fail),
                Verdict::Status(503)
            ));
            assert!(matches!(verdict(Err(e), Skip), Verdict::Status(503)));
        }
    }

    fn params(instances: usize) -> akit_config::Wasm {
        akit_config::Wasm {
            path: "test.wasm".into(),
            config: String::new(),
            instances,
            queue: 4,
            timeout: std::time::Duration::from_secs(10),
            wait_timeout: std::time::Duration::from_secs(10),
            max_memory_bytes: 1 << 20,
            on_error: WasmOnError::Fail,
            vm_config: String::new(),
            properties: Vec::new(),
            response: akit_config::WasmResponse::All,
            max_streams: 4,
            max_body_bytes: 1 << 20,
        }
    }

    /// fit's `TestClientGone`: a request whose client left while it waited is dropped, without a
    /// call; one whose client is there is called (and here traps).
    #[test]
    fn a_client_gone_is_not_called() {
        use crate::encode::{Func, Module, OP_UNREACHABLE};
        let code = Module {
            funcs: vec![Func::export(
                crate::abi::EXPORT_ON_REQUEST,
                0,
                1,
                vec![OP_UNREACHABLE],
            )],
            ..Module::default()
        }
        .encode();
        let p = Plugin::start_code(&params(1), &code).unwrap();
        for (gone, want_traps) in [(true, 0), (false, 1)] {
            let (tx, rx) = std::sync::mpsc::channel();
            let flag = AtomicBool::new(gone);
            p.pool
                .submit(
                    move |pd: &mut Pooled<Host>| run(pd, Call::default(), &flag),
                    move |r| tx.send(r).unwrap(),
                )
                .unwrap();
            let r = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
            if gone {
                assert!(matches!(r, Ok(Outcome::Gone)), "{r:?}");
            } else {
                assert!(matches!(r, Ok(Outcome::Failed)), "{r:?}");
            }
            assert_eq!(p.pool.stats().traps.total(), want_traps);
        }
    }

    #[test]
    fn plain_answers() {
        let r = plain(503);
        assert_eq!(r.status, 503);
        assert_eq!(r.body, b"503 Service Unavailable\n");
        assert_eq!(plain(500).body, b"500 Internal Server Error\n");
    }
}
