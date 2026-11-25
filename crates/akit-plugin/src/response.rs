//! A Proxy-Wasm plugin's response callbacks (fit's `plugin/wasm/response.go`, decision 023), on
//! akio's held heads and body filters (akio ADR-128, akio API requests A166 and A167).
//!
//! A plugin that exports any of them follows the response of each request it lets go on: the
//! request's context stays open on the instance that made it, and the response's callbacks run
//! as jobs queued for that instance (`InstancePool::submit_to`), before new requests, never on
//! the thread writing the response.
//!
//! - The response's head is held while `proxy_on_response_headers` runs
//!   (`Context::hold_response_head`): the plugin reads and changes its fields, or answers with a
//!   local response, which akio sends in the response's place (fib could not: fit fails such a
//!   response).
//! - Its body goes through `proxy_on_response_body` as it is written (`Context::add_body_filter`),
//!   what is written meanwhile held by akio; the last call carries the trailer, which
//!   `proxy_on_response_trailers` is given when there is one. A plugin that pauses the body holds
//!   it, up to `max_body_bytes`, and is called again with all it holds as more comes; one that
//!   pauses a header or a trailer goes on.
//! - A callback that fails (a trap, its deadline, a local response once the head has gone, the
//!   body held past `max_body_bytes`), or that cannot run (its instance made again, the pool
//!   closed), fails the response (its HTTP/1 connection closed, its stream reset), or, with
//!   `on_error: skip`, lets the rest of it go on without the plugin.
//! - Once the body has ended, or the response has, the context is done, logged and deleted on its
//!   instance: at the end of the last callback, or by the job [`Follow`] queues when it is
//!   dropped, with the last of the request's hooks.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use akio_http::{BodyChunk, BodyFilter, Context, FilterDone, Headers, HeldHead, Method, Response};
use akio_wasm::{JobError, Pooled};
use akit_config::WasmOnError;

use crate::abi::{Host, Op, apply};
use crate::plugin::Plugin;
use crate::proxywasm::{ACTION_PAUSE, Ticket, close_context, instance, invoke};

/// A followed request: its context on its instance, and what its response's hooks share.
pub(crate) struct Follow {
    plugin: Arc<Plugin>,
    ticket: Ticket,
    st: Mutex<FollowState>,
}

impl std::fmt::Debug for Follow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Follow")
            .field("ticket", &self.ticket)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
struct FollowState {
    /// `on_error: skip` left the plugin out of the rest of the response.
    skip: bool,
    /// The context is closed on its instance.
    closed: bool,
    /// The response's status, once its head is settled.
    status: u16,
    /// The body the plugin holds, and what came since, between its calls.
    buf: Vec<u8>,
    /// The body's trailer, between its last part's arrival and its call.
    trailers: Option<Headers>,
}

impl Follow {
    /// Follows the request whose context is `ticket`, counted by `plugin` until it is dropped.
    pub(crate) fn new(plugin: Arc<Plugin>, ticket: Ticket) -> Arc<Follow> {
        plugin.followed(true);
        Arc::new(Follow {
            plugin,
            ticket,
            st: Mutex::new(FollowState::default()),
        })
    }

    fn lock(&self) -> MutexGuard<'_, FollowState> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What follows a callback that failed or could not run: with `on_error: skip` the plugin
    /// is left out of the rest of the response (`true`, it goes on); with `fail` it fails.
    fn failed(&self) -> bool {
        if self.plugin.on_error == WasmOnError::Skip {
            self.lock().skip = true;
            true
        } else {
            false
        }
    }

    /// Notes that the job just run closed the context.
    fn closed(&self) {
        self.lock().closed = true;
    }

    /// Queues `job` for the instance that has the context; `done` gets its result, or, when it
    /// cannot be queued (its instance made again, the pool closed), an error at once.
    fn submit<R: 'static>(
        &self,
        job: impl FnOnce(&mut Pooled<Host>) -> R + Send + 'static,
        done: impl FnOnce(Result<R, JobError>) + Send + 'static,
    ) {
        if let Err(refused) = self.plugin.pool.submit_to(self.ticket.0, job, done) {
            (refused.done)(Err(JobError::Remade));
        }
    }

    /// Registers the hooks that run the plugin's response callbacks on `ctx`, after the changes
    /// to the response's fields `ops` the request's callbacks made.
    pub(crate) fn hook(self: Arc<Follow>, ctx: &mut Context, ops: Vec<Op>) {
        let Some(pw) = self.plugin.pw else {
            return;
        };
        let f = Arc::clone(&self);
        ctx.add_response_filter(move |resp: &mut Response, _| {
            apply(&ops, &mut resp.headers);
            f.lock().status = resp.status;
        });
        if pw.head {
            let f = Arc::clone(&self);
            ctx.hold_response_head(pw.hold_timeout, move |head, req| {
                on_head(f, head, req.method == Method::Head);
            });
        }
        if pw.body {
            ctx.add_body_filter(Body(self));
        }
    }
}

impl Drop for Follow {
    fn drop(&mut self) {
        if !self.lock().closed {
            let id = self.ticket.1;
            // Queued for the instance that has the context: never full (its queue holds four
            // calls for each stream it may follow), refused only once the instance, and the
            // context with it, are gone.
            let _ = self.plugin.pool.submit_to(
                self.ticket.0,
                move |p: &mut Pooled<Host>| drop(close_context(p, id)),
                |_| {},
            );
        }
        self.plugin.followed(false);
    }
}

/// Whether a response of `status` carries a body.
fn body_allowed(status: u16) -> bool {
    status >= 200 && status != 204 && status != 304
}

/// What `proxy_on_response_headers` decided.
#[derive(Debug)]
enum HeadOut {
    /// The head goes on, with these fields.
    Go(Headers),
    /// The plugin answered: this goes in the response's place.
    Answer(Response),
    /// The call failed, or could not be made.
    Failed,
}

/// The held head of a followed response: queued for the plugin's instance, and let go, replaced
/// or failed as the plugin decides.
fn on_head(f: Arc<Follow>, head: HeldHead, head_request: bool) {
    let status = head.status();
    let skip = {
        let mut st = f.lock();
        st.status = status;
        st.skip || st.closed
    };
    if skip {
        head.resume();
        return;
    }
    let headers = head.headers().clone();
    let end = !body_allowed(status) || head_request;
    let job_f = Arc::clone(&f);
    let done_f = Arc::clone(&f);
    f.submit(
        move |p: &mut Pooled<Host>| response_headers(p, &job_f, status, headers, end),
        move |r| {
            let mut head = head;
            match r {
                Ok(HeadOut::Go(h)) => {
                    *head.headers_mut() = h;
                    head.resume();
                }
                Ok(HeadOut::Answer(resp)) => head.respond(resp),
                Ok(HeadOut::Failed) | Err(_) => {
                    if done_f.failed() {
                        head.resume();
                    } else {
                        head.fail();
                    }
                }
            }
        },
    );
}

/// Runs `proxy_on_response_headers` for `f`'s context, with the response's `status` and its
/// fields `headers`, which it may change.
fn response_headers(
    p: &mut Pooled<Host>,
    f: &Follow,
    status: u16,
    headers: Headers,
    end: bool,
) -> HeadOut {
    let id = f.ticket.1;
    let filtered = f.plugin.pw.is_some_and(|pw| pw.body);
    let Some(pw) = instance(p) else {
        return HeadOut::Failed;
    };
    let Some(e) = pw.x.response_headers else {
        return HeadOut::Go(headers);
    };
    let Some(s) = pw.streams.get_mut(&id) else {
        // Its instance was made again since.
        return HeadOut::Failed;
    };
    let n = u32::try_from(headers.len().saturating_add(1)).unwrap_or(u32::MAX);
    s.status = status;
    s.ops.clear();
    s.head = Some(headers);
    let r = invoke(p, e, id, &[n, u32::from(end)]);
    let (head, answer, ops) = match instance(p).and_then(|pw| pw.streams.get_mut(&id)) {
        Some(s) => (
            s.head.take().unwrap_or_default(),
            s.answer.take(),
            std::mem::take(&mut s.ops),
        ),
        None => (Headers::new(), None, Vec::new()),
    };
    if r.is_err() {
        close(p, f);
        return HeadOut::Failed;
    }
    if let Some((status, body)) = answer {
        close(p, f);
        return HeadOut::Answer(local_response(status, body, &ops));
    }
    if end || !filtered {
        close(p, f);
    }
    HeadOut::Go(head)
}

/// Closes `f`'s context on its instance, done, logged and deleted, once.
fn close(p: &mut Pooled<Host>, f: &Follow) {
    let _ = close_context(p, f.ticket.1);
    f.closed();
}

/// A plugin's local response: `status`, `body`, and the fields `ops` set, as fit's.
pub(crate) fn local_response(status: u16, body: Vec<u8>, ops: &[Op]) -> Response {
    let mut resp = Response::new(status);
    apply(ops, &mut resp.headers);
    if !body.is_empty() && resp.headers.get("content-type").is_none() {
        resp.headers
            .append("Content-Type", "text/plain; charset=utf-8");
    }
    resp.body = body;
    resp
}

/// What a body callback decided.
#[derive(Debug)]
enum BodyOut {
    /// This goes on, with these trailers at the end.
    Out(Vec<u8>, Headers),
    /// The plugin holds the body: nothing goes on yet.
    Held,
    /// The call failed, or could not be made: what the plugin held is back in the follow's state.
    Failed,
}

/// A followed request's response body, through the plugin.
struct Body(Arc<Follow>);

impl BodyFilter for Body {
    fn filter(&mut self, chunk: BodyChunk, done: FilterDone) {
        let f = &self.0;
        let BodyChunk {
            data,
            end,
            trailers,
            ..
        } = chunk;
        let max = f.plugin.pw.map_or(u64::MAX, |pw| pw.max_body_bytes);
        let mut st = f.lock();
        if st.buf.is_empty() {
            st.buf = data;
        } else {
            st.buf.extend_from_slice(&data);
        }
        if st.skip {
            let out = std::mem::take(&mut st.buf);
            drop(st);
            done.done(out, trailers);
            return;
        }
        if u64::try_from(st.buf.len()).unwrap_or(u64::MAX) > max {
            // The plugin held more of the body than max_body_bytes.
            drop(st);
            if f.failed() {
                let out = std::mem::take(&mut f.lock().buf);
                done.done(out, trailers);
            } else {
                done.fail();
            }
            return;
        }
        if end {
            st.trailers = Some(trailers);
        }
        drop(st);
        let job_f = Arc::clone(f);
        let done_f = Arc::clone(f);
        f.submit(
            move |p: &mut Pooled<Host>| response_body(p, &job_f, end),
            move |r| match r {
                Ok(BodyOut::Out(data, trailers)) => done.done(data, trailers),
                Ok(BodyOut::Held) => done.done(Vec::new(), Headers::new()),
                Ok(BodyOut::Failed) | Err(_) => {
                    if done_f.failed() {
                        let mut st = done_f.lock();
                        let out = std::mem::take(&mut st.buf);
                        let trailers = st.trailers.take().unwrap_or_default();
                        drop(st);
                        done.done(out, trailers);
                    } else {
                        done.fail();
                    }
                }
            },
        );
    }
}

/// Runs `proxy_on_response_body` for `f`'s context with the body it holds and what came since,
/// and at the end `proxy_on_response_trailers`, and hands on what the plugin lets go.
fn response_body(p: &mut Pooled<Host>, f: &Follow, end: bool) -> BodyOut {
    let id = f.ticket.1;
    let (mut buf, mut trailers, status) = {
        let mut st = f.lock();
        let trailers = if end {
            st.trailers.take().unwrap_or_default()
        } else {
            Headers::new()
        };
        (std::mem::take(&mut st.buf), trailers, st.status)
    };
    // What failed hands back what the plugin held, for on_error: skip to send on.
    let back = |buf: Vec<u8>, trailers: Headers| {
        let mut st = f.lock();
        st.buf = buf;
        if end {
            st.trailers = Some(trailers);
        }
        BodyOut::Failed
    };
    let x = match instance(p) {
        Some(pw) if pw.streams.contains_key(&id) => pw.x,
        _ => return back(buf, trailers),
    };
    if let Some(e) = x.response_body {
        let n = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        let eos = end && trailers.is_empty();
        if let Some(s) = instance(p).and_then(|pw| pw.streams.get_mut(&id)) {
            s.status = status;
            s.body = std::mem::take(&mut buf);
            s.in_body = true;
            if end {
                s.trailers = Some(std::mem::take(&mut trailers));
            }
        }
        let r = invoke(p, e, id, &[n, u32::from(eos)]);
        let mut answered = false;
        if let Some(s) = instance(p).and_then(|pw| pw.streams.get_mut(&id)) {
            buf = std::mem::take(&mut s.body);
            s.in_body = false;
            if end {
                trailers = s.trailers.take().unwrap_or_default();
            }
            // A local response once the response has begun: akio cannot replace it.
            answered = s.answer.take().is_some();
        }
        if r.is_err() || answered {
            close(p, f);
            return back(buf, trailers);
        }
        if r == Ok(ACTION_PAUSE) && !end {
            // The plugin holds the body, and sees it with more next time.
            f.lock().buf = buf;
            return BodyOut::Held;
        }
    }
    if end && !trailers.is_empty() {
        if let Some(e) = x.response_trailers {
            let n = u32::try_from(trailers.len()).unwrap_or(u32::MAX);
            if let Some(s) = instance(p).and_then(|pw| pw.streams.get_mut(&id)) {
                s.status = status;
                s.trailers = Some(std::mem::take(&mut trailers));
            }
            let r = invoke(p, e, id, &[n]);
            let mut answered = false;
            if let Some(s) = instance(p).and_then(|pw| pw.streams.get_mut(&id)) {
                trailers = s.trailers.take().unwrap_or_default();
                answered = s.answer.take().is_some();
            }
            if r.is_err() || answered {
                close(p, f);
                return back(buf, trailers);
            }
        }
    }
    if end {
        close(p, f);
    }
    BodyOut::Out(buf, trailers)
}
