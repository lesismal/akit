//! fit's `mesh.Router`: the methods a role serves, each with the caller roles it takes. The same
//! routes serve calls over the mesh and a role's own pages ([`ROLE_LOCAL`], always taken).

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use akio::timer::after_send;

use crate::model::{CODE_FORBIDDEN, CODE_NOT_FOUND, Identity, ROLE_LOCAL};
use crate::wire::{Error, Reply, Stream};

/// A method's bound, and a call's whose caller sets none.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// How a method answers: once, now or later, with JSON or an error; the method's timeout answers
/// `timeout` when it has not.
#[derive(Clone)]
pub struct Answer(Arc<Mutex<Option<Reply>>>);

impl fmt::Debug for Answer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Answer").finish_non_exhaustive()
    }
}

impl Answer {
    /// An answer through `reply`, bounded by `timeout`.
    #[must_use]
    pub fn new(reply: Reply, timeout: Duration) -> Answer {
        let a = Answer(Arc::new(Mutex::new(Some(reply))));
        if !timeout.is_zero() {
            let t = a.clone();
            let _ = after_send(timeout, move || t.answer(Err(Error::Timeout)));
        }
        a
    }

    /// Whether the caller gave up, or it was answered.
    #[must_use]
    pub fn done(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_none_or(Reply::canceled)
    }

    /// Answers with `r`: JSON, or an error, which crosses as fit's `wireErr` makes it.
    pub fn answer(&self, r: Result<String, Error>) {
        let reply = self.0.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(reply) = reply {
            reply.answer(r.map(String::into_bytes).map_err(|e| e.to_remote()));
        }
    }

    /// Answers with JSON.
    pub fn ok(&self, json: String) {
        self.answer(Ok(json));
    }

    /// Answers with an error.
    pub fn err(&self, e: Error) {
        self.answer(Err(e));
    }
}

/// A method: the caller, the request's JSON, the answer.
pub type CallFn = Arc<dyn Fn(&Identity, &[u8], Answer) + Send + Sync>;
/// A stream method: the caller, the open's JSON, the stream, which it ends with
/// [`Stream::end`].
pub type StreamFn = Arc<dyn Fn(&Identity, &[u8], Stream) + Send + Sync>;

#[derive(Clone)]
struct Route {
    roles: Vec<String>,
    timeout: Duration,
    call: Option<CallFn>,
    stream: Option<StreamFn>,
}

impl Route {
    fn allowed(&self, role: &str) -> bool {
        role == ROLE_LOCAL || self.roles.iter().any(|r| r == role)
    }
}

/// The methods a role serves.
#[derive(Default, Clone)]
pub struct Router {
    routes: HashMap<String, Route>,
    streams: HashMap<String, Route>,
}

impl fmt::Debug for Router {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut m: Vec<&String> = self.routes.keys().chain(self.streams.keys()).collect();
        m.sort();
        f.debug_struct("Router").field("methods", &m).finish()
    }
}

impl Router {
    /// An empty router.
    #[must_use]
    pub fn new() -> Router {
        Router::default()
    }

    /// Registers `method` for callers of `roles`, bounded by `timeout` (zero: the default).
    pub fn handle(
        &mut self,
        method: &str,
        roles: &[&str],
        timeout: Duration,
        f: impl Fn(&Identity, &[u8], Answer) + Send + Sync + 'static,
    ) {
        self.routes.insert(
            method.to_owned(),
            Route {
                roles: roles.iter().map(|r| (*r).to_owned()).collect(),
                timeout: if timeout.is_zero() {
                    DEFAULT_TIMEOUT
                } else {
                    timeout
                },
                call: Some(Arc::new(f)),
                stream: None,
            },
        );
    }

    /// Registers the stream method `method` for callers of `roles`.
    pub fn handle_stream(
        &mut self,
        method: &str,
        roles: &[&str],
        f: impl Fn(&Identity, &[u8], Stream) + Send + Sync + 'static,
    ) {
        self.streams.insert(
            method.to_owned(),
            Route {
                roles: roles.iter().map(|r| (*r).to_owned()).collect(),
                timeout: Duration::ZERO,
                call: None,
                stream: Some(Arc::new(f)),
            },
        );
    }

    /// The methods, sorted.
    #[must_use]
    pub fn methods(&self) -> Vec<String> {
        let mut m: Vec<String> = self.routes.keys().cloned().collect();
        m.sort();
        m
    }

    /// The stream methods, sorted.
    #[must_use]
    pub fn stream_methods(&self) -> Vec<String> {
        let mut m: Vec<String> = self.streams.keys().cloned().collect();
        m.sort();
        m
    }

    /// Runs `method` for `caller`, checking its role, bounded by its timeout.
    pub fn invoke(&self, caller: &Identity, method: &str, payload: &[u8], reply: Reply) {
        let Some(rt) = self.routes.get(method) else {
            return reply.answer(Err(Error::code(
                CODE_NOT_FOUND,
                format!("unknown method {method}"),
            )));
        };
        if !rt.allowed(&caller.role) {
            return reply.answer(Err(Error::code(
                CODE_FORBIDDEN,
                format!("{} may not call {method}", caller.role),
            )));
        }
        let a = Answer::new(reply, rt.timeout);
        if let Some(f) = &rt.call {
            f(caller, payload, a);
        }
    }

    /// Runs the stream method of `st` for `caller`, checking its role.
    pub fn invoke_stream(&self, caller: &Identity, st: Stream) {
        let method = st.method().to_owned();
        let Some(rt) = self.streams.get(&method) else {
            return st.end(Err(Error::code(
                CODE_NOT_FOUND,
                format!("unknown stream method {method}"),
            )));
        };
        if !rt.allowed(&caller.role) {
            return st.end(Err(Error::code(
                CODE_FORBIDDEN,
                format!("{} may not call {method}", caller.role),
            )));
        }
        if let Some(f) = &rt.stream {
            let p = st.payload().to_vec();
            f(caller, &p, st);
        }
    }
}
