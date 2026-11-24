//! The etcd provider: fit's `provider/etcd`. Each key under its prefix holds a part, a JSON
//! object of listeners, routes, upstreams and services, and it provides them together, in the
//! order of their keys.
//!
//! It speaks etcd's v3 API through the JSON gateway etcd serves beside gRPC on its client port,
//! as fit does (fit's decision 013), with akio's HTTP client: `POST /v3/kv/range` reads the keys,
//! `POST /v3/watch` follows them, its answer a stream of JSON objects, a line each, parsed as
//! they arrive on the client's I/O thread, and `POST /v3/auth/authenticate` gets a token when a
//! user is set. It runs in the control plane, on threads of its own ([`crate::watch`]).

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use akio_crypto::base64::STANDARD;
use akio_http::client::{ClientError, ClientRequest, ResponseBody, ResponseHandler, ResponseHead};
use akio_http::message::Headers;
use akit_config::value::Value;

use crate::api::{
    Api, ApiEngine, MAX_BODY_BYTES, json_string, parse_json, status_error, status_line,
};
use crate::kvpart::{self, Entry};
use crate::run::{Provider, Update};
use crate::watch::{Conn, Events, Follower, Log, Running, Source, Stop, Timings};

/// How long etcd may stay silent on a watch before it is given up: etcd sends a progress
/// notification, asked for with `progress_notify`, every ten minutes by default.
const WATCH_IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// The etcd provider.
pub struct Etcd {
    name: String,
    opts: akit_config::EtcdProvider,
    log: Log,
    timings: Timings,
    max_body: usize,
    running: Option<(Running, Arc<ApiEngine>)>,
}

impl std::fmt::Debug for Etcd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Etcd")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl Etcd {
    /// An etcd provider named `name`, with the settings `opts`; `log` hears what it reports: keys
    /// left out and etcd failing.
    #[must_use]
    pub fn new(name: &str, opts: &akit_config::EtcdProvider, log: Log) -> Etcd {
        Etcd {
            name: name.to_owned(),
            opts: opts.clone(),
            log,
            timings: Timings::default(),
            max_body: MAX_BODY_BYTES,
            running: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn set_timings(&mut self, t: Timings) {
        self.timings = t;
    }

    /// What it watches, as akit says it when it starts: fit's words.
    #[must_use]
    pub fn watching(opts: &akit_config::EtcdProvider) -> String {
        format!(
            "watching etcd's keys under {:?} at {}",
            opts.prefix,
            opts.endpoints.join(", ")
        )
    }
}

impl Provider for Etcd {
    fn name(&self) -> &str {
        &self.name
    }

    fn start(&mut self, update: Update) -> std::io::Result<()> {
        self.stop();
        let engine = Arc::new(ApiEngine::start(&self.name).map_err(std::io::Error::other)?);
        let source = EtcdSource {
            opts: self.opts.clone(),
            engine: Arc::clone(&engine),
            tries: 0,
            max_body: self.max_body,
        };
        let mut f = Follower::new(
            &self.name,
            self.opts.refresh_interval,
            Arc::clone(&self.log),
            source,
        );
        f.timings = self.timings;
        self.running = Some((Running::start(f, update)?, engine));
        Ok(())
    }

    fn stop(&mut self) {
        if let Some((mut r, engine)) = self.running.take() {
            r.stop();
            drop(engine);
        }
    }
}

impl Drop for Etcd {
    fn drop(&mut self) {
        self.stop();
    }
}

struct EtcdSource {
    opts: akit_config::EtcdProvider,
    engine: Arc<ApiEngine>,
    /// The watches started, to take the endpoints in turn.
    tries: usize,
    max_body: usize,
}

impl Source for EtcdSource {
    type Conn = EtcdConn;

    /// A client for the next endpoint, authenticated when a user is set.
    fn open(&mut self, stop: &Stop) -> Result<Arc<EtcdConn>, String> {
        let eps = &self.opts.endpoints;
        let ep = eps
            .get(self.tries.checked_rem(eps.len()).unwrap_or(0))
            .map_or(akit_config::DEFAULT_ETCD_ENDPOINT, String::as_str);
        let endpoint = ep.strip_suffix('/').unwrap_or(ep).to_owned();
        self.tries = self.tries.wrapping_add(1);
        let api = Api::open(&self.engine, &self.opts.tls, self.max_body)?;
        let mut conn = EtcdConn {
            api,
            endpoint,
            token: String::new(),
            prefix: self.opts.prefix.clone(),
            max_keys: self.opts.max_keys,
            timeout: self.opts.request_timeout,
        };
        if !self.opts.username.is_empty() {
            let path = &self.opts.password_file;
            let password = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            let body = format!(
                "{{\"name\":{},\"password\":{}}}",
                json_string(&self.opts.username),
                json_string(password.trim_end_matches(['\r', '\n']))
            );
            let out = conn.call("/v3/auth/authenticate", body, stop)?;
            conn.token = match out.get("token") {
                Some(Value::String(t)) => t.clone(),
                _ => {
                    return Err(format!(
                        "POST /v3/auth/authenticate at {}: the answer has no token",
                        conn.endpoint
                    ));
                }
            };
        }
        Ok(Arc::new(conn))
    }
}

struct EtcdConn {
    api: Api,
    endpoint: String,
    token: String,
    prefix: String,
    max_keys: usize,
    timeout: Duration,
}

impl EtcdConn {
    fn request(&self, path: &str, body: String) -> ClientRequest {
        let mut req = ClientRequest::post(&format!("{}{path}", self.endpoint), body.into_bytes())
            .header("Content-Type", "application/json");
        if !self.token.is_empty() {
            req = req.header("Authorization", &self.token);
        }
        req
    }

    /// Sends `body` to `path` and reads the answer, within the request timeout.
    fn call(&self, path: &str, body: String, stop: &Stop) -> Result<Value, String> {
        let what = format!("POST {path} at {}", self.endpoint);
        let r = self
            .api
            .call(self.request(path, body), self.timeout, stop, &what)?;
        if r.status != 200 {
            return Err(status_error(&r, &what));
        }
        parse_json(&r.body, &format!("POST {path}"))
    }

    /// The range of the keys under the prefix, as the gateway takes it: base64.
    fn key_range(&self) -> String {
        let p = self.prefix.as_bytes();
        format!(
            "\"key\":\"{}\",\"range_end\":\"{}\"",
            STANDARD.encode(p),
            STANDARD.encode(range_end(p))
        )
    }
}

/// The end of the range of the keys starting with `prefix`: the prefix with its last byte below
/// 0xff incremented and what follows it dropped, or `"\0"`, every key, when it has none.
fn range_end(prefix: &[u8]) -> Vec<u8> {
    let mut end = prefix.to_vec();
    while let Some(last) = end.pop() {
        if last < 0xff {
            end.push(last.saturating_add(1));
            return end;
        }
    }
    vec![0]
}

impl Conn for EtcdConn {
    /// Watches the keys under the prefix: the watch is in place once etcd says it is created,
    /// which must come within the request timeout.
    fn watch(&self, stop: &Stop, ev: &Events) -> Result<(), String> {
        // The watch's request alone, cancelled when it is not created in time.
        let stop = &stop.child();
        let done = Arc::new(Done::default());
        {
            let done = Arc::clone(&done);
            stop.on_cancel(move || done.finish(Err("stopped".to_owned())));
        }
        let body = format!(
            "{{\"create_request\":{{{},\"progress_notify\":true}}}}",
            self.key_range()
        );
        let what = format!("POST /v3/watch at {}", self.endpoint);
        let req = self
            .request("/v3/watch", body)
            .response_header_timeout(self.timeout)
            .response_idle_timeout(WATCH_IDLE_TIMEOUT)
            .cancel_with(stop.token());
        let handler = WatchHandler {
            what: what.clone(),
            api: self.api.clone(),
            lines: Lines::new(self.api.max_body),
            status: None,
            error_body: Vec::new(),
            started: false,
            ev: ev.clone(),
            done: Arc::clone(&done),
            body: None,
        };
        self.api.client.request_with(req, handler);
        // Created within the request timeout, then as long as it lasts.
        let created_by = Instant::now().checked_add(self.timeout);
        let r = done.wait(created_by);
        match r {
            Some(r) => r,
            None => {
                stop.cancel();
                Err(format!(
                    "{what}: the watch was not created within {}",
                    akit_config::duration::format(self.timeout)
                ))
            }
        }
    }

    /// Reads the keys under the prefix and their values, at most `max_keys` of them.
    fn read(&self, stop: &Stop) -> Result<(Vec<u8>, Vec<String>), String> {
        let body = format!("{{{},\"limit\":{}}}", self.key_range(), self.max_keys);
        let out = self.call("/v3/kv/range", body, stop)?;
        let mut entries = Vec::new();
        if let Some(Value::Array(kvs)) = out.get("kvs") {
            for kv in kvs {
                let field = |name: &str| -> Result<Vec<u8>, String> {
                    match kv.get(name) {
                        None | Some(Value::Null) => Ok(Vec::new()),
                        Some(Value::String(s)) => STANDARD.decode(s).map_err(|e| {
                            format!("POST /v3/kv/range: a {name} that is not base64: {e}")
                        }),
                        Some(_) => Err(format!("POST /v3/kv/range: a {name} that is not a string")),
                    }
                };
                entries.push(Entry {
                    key: String::from_utf8_lossy(&field("key")?).into_owned(),
                    value: field("value")?,
                });
            }
        }
        let (part, mut problems) = kvpart::part(entries, self.max_keys);
        if matches!(out.get("more"), Some(Value::Bool(true))) {
            problems.push(format!(
                "the keys past the first {} under {} are left out: max_keys",
                self.max_keys, self.prefix
            ));
        }
        Ok((part, problems))
    }
}

/// How a watch ended, once it has, and whether it was created: set from the client's I/O
/// thread or by a stop.
#[derive(Default)]
struct Done {
    st: Mutex<(Option<Result<(), String>>, bool)>,
    cv: Condvar,
}

impl Done {
    fn finish(&self, r: Result<(), String>) {
        self.st
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .0
            .get_or_insert(r);
        self.cv.notify_all();
    }

    fn created(&self) {
        self.st.lock().unwrap_or_else(PoisonError::into_inner).1 = true;
        self.cv.notify_all();
    }

    /// Waits for the end; `None` when `created_by` passes first and the watch was not created.
    fn wait(&self, created_by: Option<Instant>) -> Option<Result<(), String>> {
        let mut g = self.st.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(r) = g.0.take() {
                return Some(r);
            }
            g = match created_by {
                Some(by) if !g.1 => {
                    let left = by.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return None;
                    }
                    self.cv
                        .wait_timeout(g, left)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
                _ => self.cv.wait(g).unwrap_or_else(PoisonError::into_inner),
            };
        }
    }
}

/// Lines of a stream, each at most `max` bytes.
struct Lines {
    buf: Vec<u8>,
    max: usize,
}

impl Lines {
    fn new(max: usize) -> Lines {
        Lines {
            buf: Vec::new(),
            max,
        }
    }

    /// Takes `chunk`, calling `f` with each line it completes; stops at `f`'s first error.
    fn push(
        &mut self,
        mut chunk: &[u8],
        f: &mut dyn FnMut(&[u8]) -> Result<(), String>,
    ) -> Result<(), String> {
        while let Some(i) = chunk.iter().position(|&b| b == b'\n') {
            let (line, rest) = chunk.split_at(i);
            chunk = rest.get(1..).unwrap_or_default();
            if self.buf.is_empty() {
                f(line)?;
            } else {
                self.add(line)?;
                let line = std::mem::take(&mut self.buf);
                f(&line)?;
            }
        }
        self.add(chunk)
    }

    fn add(&mut self, b: &[u8]) -> Result<(), String> {
        if self.buf.len().saturating_add(b.len()) > self.max {
            return Err(format!("a watch message is larger than {} bytes", self.max));
        }
        self.buf.extend_from_slice(b);
        Ok(())
    }

    /// What is left at the end of the stream, a last line without its newline.
    fn finish(&mut self, f: &mut dyn FnMut(&[u8]) -> Result<(), String>) -> Result<(), String> {
        let line = std::mem::take(&mut self.buf);
        if line.is_empty() { Ok(()) } else { f(&line) }
    }
}

/// One message of a watch, a line: calls `ready` at the first, which says the watch is
/// created, and `changed` at each with events; an error ends the watch.
fn watch_message(
    line: &[u8],
    started: &mut bool,
    ready: &mut dyn FnMut(),
    changed: &mut dyn FnMut(),
) -> Result<(), String> {
    if line.trim_ascii().is_empty() {
        return Ok(());
    }
    let m = std::str::from_utf8(line)
        .ok()
        .and_then(|t| akit_config::value::parse(t).ok())
        .ok_or_else(|| "a watch message that is not JSON".to_owned())?;
    if let Some(e) = m.get("error") {
        let msg = match e.get("message") {
            Some(Value::String(s)) => s.as_str(),
            _ => "",
        };
        return Err(format!("the watch failed: {msg}"));
    }
    let Some(r) = m.get("result").filter(|r| !matches!(r, Value::Null)) else {
        return Ok(());
    };
    let flag = |k: &str| matches!(r.get(k), Some(Value::Bool(true)));
    let text = |k: &str| match r.get(k) {
        Some(Value::String(s) | Value::Number(s)) => s.clone(),
        _ => String::new(),
    };
    if flag("canceled") {
        let mut reason = text("cancel_reason");
        let compacted = text("compact_revision");
        if !compacted.is_empty() && compacted != "0" {
            reason.push_str(&format!(" (compacted at revision {compacted})"));
        }
        return Err(format!("etcd canceled the watch: {}", reason.trim()));
    }
    if !*started {
        if !flag("created") {
            return Err("the watch's first message does not say it is created".to_owned());
        }
        *started = true;
        ready();
        return Ok(());
    }
    if matches!(r.get("events"), Some(Value::Array(evs)) if !evs.is_empty()) {
        changed();
    }
    Ok(())
}

/// Receives a watch's answer as it arrives, on the client's I/O thread: never blocks.
struct WatchHandler {
    what: String,
    api: Api,
    lines: Lines,
    /// The answer's status, when it is not 200: its body is then read for the error.
    status: Option<(u16, String)>,
    error_body: Vec<u8>,
    started: bool,
    ev: Events,
    done: Arc<Done>,
    body: Option<ResponseBody>,
}

impl WatchHandler {
    fn fail(&mut self, e: String) {
        if let Some(b) = self.body.take() {
            b.cancel();
        }
        self.done.finish(Err(e));
    }
}

impl ResponseHandler for WatchHandler {
    fn on_head(&mut self, head: ResponseHead, body: ResponseBody) {
        if head.status != 200 {
            self.status = Some((head.status, head.reason));
        }
        self.body = Some(body);
    }

    fn on_data(&mut self, chunk: &[u8]) {
        if self.status.is_some() {
            let room = 4096usize.saturating_sub(self.error_body.len());
            self.error_body
                .extend_from_slice(chunk.get(..room).unwrap_or(chunk));
            return;
        }
        let (ev, done) = (self.ev.clone(), Arc::clone(&self.done));
        let started = &mut self.started;
        let r = self.lines.push(chunk, &mut |line| {
            watch_message(
                line,
                started,
                &mut || {
                    done.created();
                    ev.ready();
                },
                &mut || ev.changed(),
            )
        });
        if let Err(e) = r {
            self.fail(e);
        }
    }

    fn on_end(&mut self, _trailers: &Headers) {
        self.body = None;
        if let Some((status, reason)) = self.status.take() {
            let msg = format!(
                "{}: {}: {}",
                self.what,
                status_line(status, &reason),
                String::from_utf8_lossy(&self.error_body).trim()
            );
            self.done.finish(Err(msg));
            return;
        }
        let (ev, done) = (self.ev.clone(), Arc::clone(&self.done));
        let started = &mut self.started;
        let r = self.lines.finish(&mut |line| {
            watch_message(
                line,
                started,
                &mut || {
                    done.created();
                    ev.ready();
                },
                &mut || ev.changed(),
            )
        });
        self.done
            .finish(r.and(Err("etcd ended the watch".to_owned())));
    }

    fn on_error(&mut self, e: ClientError) {
        self.body = None;
        let msg = format!("{}: {}", self.what, self.api.error(&e));
        self.done.finish(Err(msg));
    }
}

#[cfg(test)]
mod tests;
