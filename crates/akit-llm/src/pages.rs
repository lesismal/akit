//! An LLM role's module of the pages (I109c, fit's `llm/internal/webui`): what its `console` and
//! `admin.console` serve at `/api/m/llm.<role>/`, and its own console at `/api/` too.
//!
//! ```text
//! GET  /api/me               {"identity", "user"}, and a proxy's "api_url"
//! POST /api/rpc<method>      a method of the role's router, answered with its result as JSON,
//!                            or {"error", "code"} with the HTTP status of the code
//! POST /api/stream<method>   a stream method, answered with Server-Sent Events: a data event
//!                            per frame, "end" with the result, "error" with {code, message}
//! ```
//!
//! A method runs as the role's own pages call it ([`Local`], the caller `local`), through the
//! same code as over the mesh; it never blocks the console's thread: its answer comes through a
//! callback. A stream's frames are written as they come and given back to the stream only once
//! the browser has taken them ([`WATERMARK`]), so that a slow browser holds back its own stream
//! and nothing piles up; a wait for it is bounded by [`STALL_TIMEOUT`]. At most [`MAX_STREAMS`]
//! streams are followed at once.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use akio_http::{BodyStream, Context, Detached, Headers, Method, Response};
use akit_roles::pages::{Module, json, no_endpoint, quoted};

use crate::local::Local;
use crate::mesh::WeakMesh;
use crate::model::{
    CODE_BAD_REQUEST, CODE_CONFLICT, CODE_FORBIDDEN, CODE_INTERNAL, CODE_NOT_FOUND, CODE_OK,
    CODE_RATE_LIMITED, CODE_UNAUTHORIZED, CODE_UNAVAILABLE, METHOD_PEERS, METHOD_STATUS,
};
use crate::wire::{Error, Event, Stream};

/// How many streams a role's pages follow at once: fit's `MaxStreams`.
pub const MAX_STREAMS: usize = 16;

/// How long a stream waits for the browser to take its events: fit's `StallTimeout`.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// The bytes written to a browser and not yet sent past which a stream's frames are held back
/// until they are.
pub const WATERMARK: usize = 64 << 10;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What `GET /api/me` adds beside the identity and the user: members as JSON, by name.
pub type MeFn = Arc<dyn Fn() -> Vec<(&'static str, String)> + Send + Sync>;

/// An LLM role's module.
pub struct LlmPages {
    id: &'static str,
    local: Local,
    mesh: WeakMesh,
    me: OnceLock<MeFn>,
    streams: Arc<AtomicUsize>,
    /// [`STALL_TIMEOUT`], or a test's.
    stall: Duration,
}

impl std::fmt::Debug for LlmPages {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmPages")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// The HTTP status of a business code: fit's `StatusOf`.
#[must_use]
pub fn status_of(code: i64) -> u16 {
    match code {
        CODE_OK => 200,
        CODE_BAD_REQUEST => 400,
        CODE_UNAUTHORIZED => 401,
        CODE_FORBIDDEN => 403,
        CODE_NOT_FOUND => 404,
        CODE_CONFLICT => 409,
        CODE_RATE_LIMITED => 429,
        CODE_UNAVAILABLE => 503,
        _ => 500,
    }
}

/// `{"error", "code"}` for `e`, with the status of its code (fit's `Fail`): a timeout 504, the
/// method's own (which crosses the pipe as an internal error, `timeout`) as the call's.
#[must_use]
pub fn failure(e: &Error) -> Response {
    let (code, msg) = e.code_msg();
    let status = if code == CODE_INTERNAL && msg == "timeout" {
        504
    } else {
        status_of(code)
    };
    json(
        status,
        format!("{{\"error\":{},\"code\":{code}}}\n", quoted(&msg)),
    )
}

/// One Server-Sent Event: its name, when not a data event, and `data`'s lines.
#[must_use]
pub fn event(name: &str, data: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(data.len().saturating_add(16));
    if !name.is_empty() {
        b.extend_from_slice(b"event: ");
        b.extend_from_slice(name.as_bytes());
        b.push(b'\n');
    }
    for line in data.split(|c| *c == b'\n') {
        b.extend_from_slice(b"data: ");
        b.extend_from_slice(line.strip_suffix(b"\r").unwrap_or(line));
        b.push(b'\n');
    }
    b.push(b'\n');
    b
}

impl LlmPages {
    /// The module of role `role` (`hub`), its methods called through `local`.
    #[must_use]
    pub fn new(role: &str, local: Local, mesh: WeakMesh) -> LlmPages {
        LlmPages {
            id: match role {
                "hub" => "llm.hub",
                "node" => "llm.node",
                _ => "llm.proxy",
            },
            local,
            mesh,
            me: OnceLock::new(),
            streams: Arc::default(),
            stall: STALL_TIMEOUT,
        }
    }

    /// Has `GET /api/me` add what `f` gives (a proxy's `api_url`); set once.
    pub fn set_me(&self, f: MeFn) {
        let _ = self.me.set(f);
    }

    /// The module with a stall timeout of its own: tests'.
    #[doc(hidden)]
    #[must_use]
    pub fn with_stall(mut self, d: Duration) -> LlmPages {
        self.stall = d;
        self
    }

    fn me(&self, user: &str) -> Response {
        let mut ms: Vec<(&'static str, String)> = self.me.get().map(|f| f()).unwrap_or_default();
        let identity = self
            .mesh
            .upgrade()
            .map_or_else(|| "null".to_owned(), |m| m.identity().to_json());
        ms.push(("identity", identity));
        ms.push(("user", quoted(user)));
        // As Go writes a map: its keys in order.
        ms.sort_by(|a, b| a.0.cmp(b.0));
        let body: Vec<String> = ms
            .iter()
            .map(|(k, v)| format!("{}:{v}", quoted(k)))
            .collect();
        json(200, format!("{{{}}}\n", body.join(",")))
    }

    /// The request's JSON, `""` for none; `None` once refused (fit's `body`).
    fn body(ctx: &mut Context) -> Option<String> {
        let b = ctx.body();
        let text = std::str::from_utf8(b).ok().map(str::trim);
        match text {
            Some("") => Some(String::new()),
            Some(t) if akit_config::value::parse(t).is_ok() => Some(t.to_owned()),
            _ => {
                ctx.respond(json(
                    400,
                    format!("{{\"error\":\"the body is not JSON\",\"code\":{CODE_BAD_REQUEST}}}\n"),
                ));
                None
            }
        }
    }

    fn rpc(&self, ctx: &mut Context, user: &str, method: &str) {
        let Some(req) = Self::body(ctx) else { return };
        let Some(d) = ctx.detach() else { return };
        let (user, m) = (user.to_owned(), method.to_owned());
        self.local
            .call(method, &req, Duration::ZERO, move |r| match r {
                Ok(data) => {
                    if m != METHOD_PEERS
                        && m != METHOD_STATUS
                        && !m.ends_with("/list")
                        && !m.ends_with("/get")
                    {
                        akit_observe::info!(Llm, "console: {user:?}: {m}");
                    }
                    let data = if data.is_empty() {
                        b"null".to_vec()
                    } else {
                        data
                    };
                    d.respond(Response::with_body(200, "application/json", data));
                }
                Err(e) => d.respond(failure(&e)),
            });
    }

    fn stream(&self, ctx: &mut Context, method: &str) {
        let Some(req) = Self::body(ctx) else { return };
        if self.streams.fetch_add(1, Ordering::AcqRel) >= MAX_STREAMS {
            self.streams.fetch_sub(1, Ordering::AcqRel);
            return ctx.respond(failure(&Error::code(
                CODE_RATE_LIMITED,
                "too many streams open; close one",
            )));
        }
        let st = match self.local.open_stream(method, &req) {
            Ok(st) => st,
            Err(e) => {
                self.streams.fetch_sub(1, Ordering::AcqRel);
                return ctx.respond(failure(&e));
            }
        };
        let Some(mut d) = ctx.detach() else {
            self.streams.fetch_sub(1, Ordering::AcqRel);
            st.cancel();
            return;
        };
        let gone = st.clone();
        d.on_gone(move || gone.cancel());
        let sse = Arc::new(Sse {
            st: st.clone(),
            out: Mutex::new(Out::Waiting(d)),
            state: Mutex::new(SseState::default()),
            streams: Arc::clone(&self.streams),
            stall: self.stall,
        });
        st.on_data(move |ev| sse.event(ev));
    }
}

/// Where a stream's events go: the request, until its response starts, then its body.
enum Out {
    Waiting(Detached),
    Body(BodyStream),
    Done,
}

#[derive(Default)]
struct SseState {
    ended: bool,
    /// Bytes of frames not given back, waiting for the browser.
    held: usize,
    waiting: bool,
    stall: Option<akio::timer::SendTimer>,
}

/// One stream's events to the browser (fit's `sse`). `out` and `state` are never held together
/// but in [`Sse::drained`], which takes `state` only: the body's drain may run it at once, inside
/// `out`.
struct Sse {
    st: Stream,
    out: Mutex<Out>,
    state: Mutex<SseState>,
    streams: Arc<AtomicUsize>,
    stall: Duration,
}

fn sse_headers() -> Headers {
    let mut h = Headers::new();
    h.set("Content-Type", "text/event-stream; charset=utf-8");
    h.set("X-Accel-Buffering", "no");
    h
}

impl Sse {
    /// The body, the response started when it was not.
    fn started(out: &mut Out) -> Option<&mut BodyStream> {
        if let Out::Waiting(_) = out {
            let Out::Waiting(d) = std::mem::replace(out, Out::Done) else {
                return None;
            };
            if let Some(b) = d.stream_response(200, sse_headers()) {
                *out = Out::Body(b);
            }
        }
        match out {
            Out::Body(b) => Some(b),
            _ => None,
        }
    }

    fn event(self: &Arc<Self>, ev: Event) {
        match ev {
            Event::Data(p) => self.frame(&p),
            Event::End(r) => self.finish(r),
        }
    }

    fn frame(self: &Arc<Self>, p: &[u8]) {
        let n = p.len();
        if lock(&self.state).ended {
            self.st.release(n);
            return;
        }
        let data: &[u8] = if p.is_empty() { b"null" } else { p };
        let mut out = lock(&self.out);
        let Some(body) = Self::started(&mut out) else {
            drop(out);
            self.st.cancel();
            return;
        };
        if body.write(&event("", data)).is_err() {
            drop(out);
            self.st.cancel();
            return;
        }
        let above = body.queued_bytes() > WATERMARK;
        let wait = {
            let mut s = lock(&self.state);
            if !above && !s.waiting {
                drop(s);
                drop(out);
                self.st.release(n);
                return;
            }
            s.held = s.held.saturating_add(n);
            if s.waiting {
                false
            } else {
                s.waiting = true;
                let me = Arc::downgrade(self);
                s.stall = akio::timer::after_send(self.stall, move || {
                    if let Some(me) = me.upgrade() {
                        me.st.cancel();
                    }
                })
                .ok();
                true
            }
        };
        if wait {
            let me = Arc::downgrade(self);
            body.on_drain(move || {
                if let Some(me) = me.upgrade() {
                    me.drained();
                }
            });
        }
    }

    /// The browser took what was written: the frames held go back to the stream.
    fn drained(&self) {
        let held = {
            let mut s = lock(&self.state);
            s.waiting = false;
            if let Some(t) = s.stall.take() {
                t.cancel();
            }
            std::mem::take(&mut s.held)
        };
        self.st.release(held);
    }

    fn finish(&self, r: Result<(), Error>) {
        {
            let mut s = lock(&self.state);
            if s.ended {
                return;
            }
            s.ended = true;
            if let Some(t) = s.stall.take() {
                t.cancel();
            }
        }
        self.streams.fetch_sub(1, Ordering::AcqRel);
        let mut out = lock(&self.out);
        let ev = match r {
            Ok(()) => {
                let res = self
                    .st
                    .result()
                    .filter(|r| !r.iter().all(u8::is_ascii_whitespace))
                    .unwrap_or_else(|| b"null".to_vec());
                event("end", &res)
            }
            Err(e) => {
                if let Out::Waiting(_) = &*out {
                    if let Out::Waiting(d) = std::mem::replace(&mut *out, Out::Done) {
                        d.respond(failure(&e));
                    }
                    return;
                }
                let (code, msg) = e.code_msg();
                event(
                    "error",
                    format!("{{\"code\":{code},\"message\":{}}}", quoted(&msg)).as_bytes(),
                )
            }
        };
        if Self::started(&mut out).is_some() {
            if let Out::Body(b) = std::mem::replace(&mut *out, Out::Done) {
                let _ = b.write(&ev);
                b.finish();
            }
        }
    }
}

impl Module for LlmPages {
    fn id(&self) -> &'static str {
        self.id
    }

    fn serve(&self, ctx: &mut Context, user: &str, path: &str) {
        let method = ctx.method().clone();
        if method == Method::Get && path == "/api/me" {
            return ctx.respond(self.me(user));
        }
        if method != Method::Post {
            return ctx.respond(no_endpoint());
        }
        let (calls, streams) = self.local.methods();
        if let Some(m) = path.strip_prefix("/api/rpc") {
            if calls.iter().any(|c| c == m) {
                return self.rpc(ctx, user, m);
            }
        } else if let Some(m) = path.strip_prefix("/api/stream") {
            if streams.iter().any(|c| c == m) {
                return self.stream(ctx, m);
            }
        }
        ctx.respond(no_endpoint());
    }
}

#[cfg(test)]
mod tests;
