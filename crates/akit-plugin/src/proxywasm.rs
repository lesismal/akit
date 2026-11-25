//! Proxy-Wasm (fit's `plugin/wasm/proxywasm.go`): the ABI Envoy and others run HTTP filters
//! through, v0.2.x, the subset fit supports, as host functions of akio-wasm's [`Linker`].
//!
//! A plugin is a Proxy-Wasm filter when it exports `proxy_abi_version_0_2_0` or
//! `proxy_abi_version_0_2_1`. Each instance starts its root context (id 1) with the `vm_config`
//! and the `config` when it is made ([`start`]), and fails to load if the plugin refuses either.
//! Each request is a context of its own: created, its request headers handed to the plugin
//! ([`run_request`]), then done, logged and deleted, so that the plugin forgets it, unless the
//! plugin follows the response ([`crate::response`]). What the plugin decides then goes: a local
//! response it sent, or the request on, with the changes it made to the request's headers
//! (pseudo-headers `:method`, `:path`, `:authority` included) and to the response's, which apply
//! when its header is settled. The response's callbacks read and change the response's header,
//! body and trailer. Calls out (`proxy_http_call`), shared data and queues, ticks, foreign
//! functions, the request's body and trailer, and setting properties answer `Unimplemented` or
//! `NotFound`.
//!
//! The contexts an instance keeps live in its store's state ([`PwInstance::streams`]), so that an
//! instance made again after a trap starts with none, as fit's does.
//!
//! Where fit stores what a plugin gives it, akit also refuses (`BadArgument`) a field name that is
//! not an HTTP token and a value holding a line break or NUL, which would split the header akio
//! writes, a name or value that is not UTF-8, a `:path` that is not a path, and fails the call
//! past [`MAX_CHANGES`] changes to a header map in one callback (rule 11).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use akio_http::Headers;
use akio_wasm::{
    Caller, Func, FuncType, InstanceId, LinkError, Linker, Pooled, Trap, ValType, Value,
};

use crate::abi::{self, Host, MAX_BODY, MAX_FIELD, MAX_OPS, Op, abi, apply, canonical, is_tchar};
use crate::wasm::{Outcome, Ran};

/// The exports that make a module a Proxy-Wasm filter.
pub(crate) const VERSIONS: [&str; 2] = ["proxy_abi_version_0_2_1", "proxy_abi_version_0_2_0"];

pub(crate) const ON_CONTEXT_CREATE: &str = "proxy_on_context_create";
pub(crate) const ON_VM_START: &str = "proxy_on_vm_start";
pub(crate) const ON_CONFIGURE: &str = "proxy_on_configure";
pub(crate) const ON_REQUEST_HEADERS: &str = "proxy_on_request_headers";
pub(crate) const ON_DONE: &str = "proxy_on_done";
pub(crate) const ON_LOG: &str = "proxy_on_log";
pub(crate) const ON_DELETE: &str = "proxy_on_delete";
pub(crate) const ALLOCATE: &str = "proxy_on_memory_allocate";
pub(crate) const ON_RESPONSE_HEADERS: &str = "proxy_on_response_headers";
pub(crate) const ON_RESPONSE_BODY: &str = "proxy_on_response_body";
pub(crate) const ON_RESPONSE_TRAILERS: &str = "proxy_on_response_trailers";

/// The exports a Proxy-Wasm plugin must have.
pub(crate) const REQUIRED: [&str; 6] = [
    ON_CONTEXT_CREATE,
    ON_REQUEST_HEADERS,
    ON_DONE,
    ON_LOG,
    ON_DELETE,
    ALLOCATE,
];

/// The module Proxy-Wasm's host functions are imported from.
const ENV: &str = "env";

// Proxy-Wasm's statuses.
pub(crate) const OK: u32 = 0;
pub(crate) const NOT_FOUND: u32 = 1;
pub(crate) const BAD_ARGUMENT: u32 = 2;
pub(crate) const UNIMPLEMENTED: u32 = 12;

// Its map types.
pub(crate) const REQUEST_HEADERS: u32 = 0;
pub(crate) const RESPONSE_HEADERS: u32 = 2;
pub(crate) const RESPONSE_TRAILERS: u32 = 3;

// Its buffer types.
pub(crate) const RESPONSE_BODY: u32 = 1;
pub(crate) const VM_CONFIGURATION: u32 = 6;
pub(crate) const PLUGIN_CONFIGURATION: u32 = 7;

/// The action that pauses a stream.
pub(crate) const ACTION_PAUSE: i32 = 1;

/// The root context's id; requests' contexts are numbered from the next.
pub(crate) const ROOT_CONTEXT: u32 = 1;
/// The most metrics an instance defines.
pub(crate) const MAX_METRICS: usize = 256;
/// The most changes one callback makes to the request's header, or to the response's header or
/// trailer while it has them (akit's bound; fit sets none).
pub(crate) const MAX_CHANGES: usize = 1024;

/// What a Proxy-Wasm plugin's instances share, from its params.
#[derive(Debug, Default)]
pub(crate) struct PwShared {
    /// `vm_config`, the buffer `proxy_on_vm_start` reads.
    pub(crate) vm_config: String,
    /// `properties`, each path's segments joined by NUL.
    pub(crate) properties: Vec<(String, String)>,
    /// `max_body_bytes`: how much of a response's body the plugin may hold or make.
    pub(crate) max_body_bytes: u64,
    /// `max_streams`: the responses an instance follows at once.
    pub(crate) max_streams: usize,
}

/// A function a Proxy-Wasm plugin exports, and how many results it returns (none or one).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Export {
    f: Func,
    results: usize,
}

/// The functions an instance of a Proxy-Wasm plugin exports.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Exports {
    context_create: Export,
    request_headers: Export,
    done: Export,
    log: Export,
    delete: Export,
    allocate: Export,
    pub(crate) response_headers: Option<Export>,
    pub(crate) response_body: Option<Export>,
    pub(crate) response_trailers: Option<Export>,
}

/// A Proxy-Wasm instance's own: its exports, the next request's context id, its metrics, and the
/// contexts it keeps.
#[derive(Debug)]
pub(crate) struct PwInstance {
    pub(crate) x: Exports,
    next_id: u32,
    metrics: Vec<i64>,
    metric_names: HashMap<String, u32>,
    /// The requests' contexts open: the one a request's call serves, and those whose responses
    /// the plugin follows (at most `max_streams` between calls).
    pub(crate) streams: HashMap<u32, Stream>,
    /// The context the call under way serves, which the host functions act on.
    pub(crate) current: Option<u32>,
}

impl PwInstance {
    fn new(x: Exports) -> PwInstance {
        PwInstance {
            x,
            next_id: ROOT_CONTEXT.saturating_add(1),
            metrics: Vec::new(),
            metric_names: HashMap::new(),
            streams: HashMap::new(),
            current: None,
        }
    }

    /// The next request's context id: never the root's, nor one still open.
    fn next_id(&mut self) -> u32 {
        loop {
            let id = self.next_id;
            self.next_id = match id.checked_add(1) {
                Some(n) => n,
                None => ROOT_CONTEXT.saturating_add(1),
            };
            if id > ROOT_CONTEXT && !self.streams.contains_key(&id) {
                return id;
            }
        }
    }
}

/// A request as a Proxy-Wasm plugin sees and changes it: copied from the request on its I/O
/// thread, and handed back with the plugin's changes once the call returns.
#[derive(Debug, Clone, Default)]
pub(crate) struct PwRequest {
    /// `:method`.
    pub(crate) method: String,
    /// `:path`: the request's target, its query included (Go's `RequestURI`).
    pub(crate) target: String,
    /// `:authority`: the request's `Host`.
    pub(crate) host: String,
    /// `:scheme`.
    pub(crate) scheme: &'static str,
    /// `request.protocol`, as Go's `Proto`.
    pub(crate) protocol: &'static str,
    /// `source.address`: the client's address and port.
    pub(crate) remote: String,
    /// The request's fields, `Host` aside.
    pub(crate) headers: Headers,
    /// Whether the request is followed past its headers when the plugin lets it go on: it is not
    /// a tunnel's (`CONNECT`, an upgrade), and the plugin exports response callbacks it is given.
    pub(crate) follows: bool,
    /// Whether it has a body (`end_of_stream` is its opposite).
    pub(crate) has_body: bool,
}

impl PwRequest {
    /// The request's path, decoded, without its query (Go's `URL.Path`).
    fn url_path(&self) -> String {
        let path = self
            .target
            .split_once('?')
            .map_or(&*self.target, |(p, _)| p);
        akio_http::message::percent_decode(path).unwrap_or_else(|| path.to_owned())
    }

    /// The request's query, without its `?`.
    fn query(&self) -> &str {
        self.target.split_once('?').map_or("", |(_, q)| q)
    }

    /// Its pseudo-header `name`.
    fn pseudo(&self, name: &str) -> Option<&str> {
        match name {
            ":method" => Some(&self.method),
            ":path" => Some(&self.target),
            ":authority" => Some(&self.host),
            ":scheme" => Some(self.scheme),
            _ => None,
        }
    }

    /// Its headers as Proxy-Wasm has them: its pseudo-headers, then its fields, in lower case.
    fn pairs(&self) -> Vec<(String, String)> {
        let mut pairs = Vec::with_capacity(self.headers.len().saturating_add(4));
        for name in [":method", ":path", ":authority", ":scheme"] {
            pairs.push((name.to_owned(), self.pseudo(name).unwrap_or("").to_owned()));
        }
        field_pairs(&mut pairs, &self.headers);
        pairs
    }
}

/// A request's context on its instance: the request, and the response as far as the plugin has
/// seen it.
#[derive(Debug, Default)]
pub(crate) struct Stream {
    pub(crate) req: PwRequest,
    /// The request's pseudo-headers changed (`:method`, `:path`, `:authority`).
    pseudo_changed: bool,
    /// Its fields changed.
    fields_changed: bool,
    /// Changes the callback under way made to a header map, bounded by [`MAX_CHANGES`].
    changes: usize,
    /// The plugin's changes to the response's fields, before there is one.
    pub(crate) ops: Vec<Op>,
    /// The plugin's local response: its status and body (its fields are in `ops`).
    pub(crate) answer: Option<(u16, Vec<u8>)>,
    /// The response's status once its header is settled, 0 before.
    pub(crate) status: u16,
    /// The response's header while `proxy_on_response_headers` runs.
    pub(crate) head: Option<Headers>,
    /// The response's trailer, at the end of its body.
    pub(crate) trailers: Option<Headers>,
    /// The response's body as the plugin has it, while `proxy_on_response_body` runs.
    pub(crate) body: Vec<u8>,
    /// The body is the plugin's to read and change.
    pub(crate) in_body: bool,
}

impl Stream {
    pub(crate) fn new(req: PwRequest) -> Stream {
        Stream {
            req,
            ..Stream::default()
        }
    }

    /// The value the response's field `name` has so far, as the plugin set it before there is a
    /// response.
    fn response(&self, name: &str) -> Option<&str> {
        let mut v = None;
        for op in &self.ops {
            match op {
                Op::Set(n, val) if n.eq_ignore_ascii_case(name) => v = Some(val.as_str()),
                Op::Add(n, val) if n.eq_ignore_ascii_case(name) => v = v.or(Some(val.as_str())),
                Op::Del(n) if n.eq_ignore_ascii_case(name) => v = None,
                _ => {}
            }
        }
        v
    }

    /// Records a change to the response's fields, before there is a response: past [`MAX_OPS`],
    /// the call fails, as fit's.
    fn push_op(&mut self, op: Op) -> Result<(), Trap> {
        if self.ops.len() >= MAX_OPS {
            return Err(abi("more than 256 changes to the response's fields"));
        }
        self.ops.push(op);
        Ok(())
    }

    /// Counts a change made in place to a header map.
    fn count(&mut self) -> Result<(), Trap> {
        if self.changes >= MAX_CHANGES {
            return Err(abi(
                "more than 1024 changes to a header map in one callback",
            ));
        }
        self.changes = self.changes.saturating_add(1);
        Ok(())
    }

    /// Replaces `size` bytes of the body from `start` with `data`, as `proxy_set_buffer_bytes`
    /// does: `start` at the end appends, and `size` 0 inserts. `BadArgument` past `max`.
    pub(crate) fn set_body(&mut self, start: u32, size: u32, data: &[u8], max: u64) -> u32 {
        let n = self.body.len();
        let start = usize::try_from(start).unwrap_or(usize::MAX).min(n);
        let end = start
            .saturating_add(usize::try_from(size).unwrap_or(usize::MAX))
            .min(n);
        let len = n
            .saturating_sub(end.saturating_sub(start))
            .saturating_add(data.len());
        if u64::try_from(len).unwrap_or(u64::MAX) > max {
            return BAD_ARGUMENT;
        }
        self.body.splice(start..end, data.iter().copied());
        OK
    }

    /// The request's changes, as the request's fields and target: `None` for those that did not
    /// change. Its fields come with `Host` again.
    pub(crate) fn request_changes(&self) -> (Option<Headers>, Option<String>, Option<String>) {
        let headers = (self.fields_changed || self.pseudo_changed).then(|| {
            let mut h = self.req.headers.clone();
            h.set("Host", &self.req.host);
            h
        });
        let (method, target) = if self.pseudo_changed {
            (Some(self.req.method.clone()), Some(self.req.target.clone()))
        } else {
            (None, None)
        };
        (headers, method, target)
    }
}

/// `h`'s fields as Proxy-Wasm has them, in lower case, after `pairs`.
fn field_pairs(pairs: &mut Vec<(String, String)>, h: &Headers) {
    for (n, v) in h.iter() {
        pairs.push((n.to_ascii_lowercase(), v.to_owned()));
    }
}

/// Encodes `pairs` as Proxy-Wasm does: their count, each one's key and value lengths, then each
/// key and value with a NUL after it, the numbers little-endian.
pub(crate) fn serialize_pairs(pairs: &[(String, String)]) -> Vec<u8> {
    let size = pairs.iter().fold(4usize, |n, (k, v)| {
        n.saturating_add(10)
            .saturating_add(k.len())
            .saturating_add(v.len())
    });
    let mut b = Vec::with_capacity(size);
    b.extend_from_slice(&len32(pairs.len()).to_le_bytes());
    for (k, v) in pairs {
        b.extend_from_slice(&len32(k.len()).to_le_bytes());
        b.extend_from_slice(&len32(v.len()).to_le_bytes());
    }
    for (k, v) in pairs {
        b.extend_from_slice(k.as_bytes());
        b.push(0);
        b.extend_from_slice(v.as_bytes());
        b.push(0);
    }
    b
}

fn len32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Decodes what [`serialize_pairs`] encodes, copying it: `None` for what is not that, holds more
/// than [`MAX_OPS`] pairs, or text that is not UTF-8.
pub(crate) fn deserialize_pairs(b: &[u8]) -> Option<Vec<(String, String)>> {
    if b.is_empty() {
        return Some(Vec::new());
    }
    let u32_at = |at: usize| -> Option<usize> {
        let bytes: [u8; 4] = b.get(at..at.checked_add(4)?)?.try_into().ok()?;
        usize::try_from(u32::from_le_bytes(bytes)).ok()
    };
    let n = u32_at(0)?;
    if n > MAX_OPS {
        return None;
    }
    let mut data = n.checked_mul(8)?.checked_add(4)?;
    if data > b.len() {
        return None;
    }
    let mut pairs = Vec::with_capacity(n);
    for i in 0..n {
        let at = i.checked_mul(8)?.checked_add(4)?;
        let kl = u32_at(at)?;
        let vl = u32_at(at.checked_add(4)?)?;
        let k = b.get(data..data.checked_add(kl)?)?;
        data = data.checked_add(kl)?.checked_add(1)?;
        let v = b.get(data..data.checked_add(vl)?)?;
        data = data.checked_add(vl)?.checked_add(1)?;
        if data > b.len() {
            return None;
        }
        pairs.push((
            String::from_utf8(k.to_vec()).ok()?,
            String::from_utf8(v.to_vec()).ok()?,
        ));
    }
    Some(pairs)
}

/// Finds export `name` of the instance, checked to take `params` i32s and return as many i32s
/// as one of `results`.
fn export(
    p: &Pooled<Host>,
    name: &str,
    params: usize,
    results: &[usize],
) -> Result<Option<Export>, Trap> {
    let Some(f) = p.export_func(name) else {
        return Ok(None);
    };
    let t = p.store().func_type(f);
    let all_i32 = |v: &[ValType]| v.iter().all(|t| *t == ValType::I32);
    if t.params().len() != params
        || !results.contains(&t.results().len())
        || !all_i32(t.params())
        || !all_i32(t.results())
    {
        return Err(abi(&format!(
            "the Proxy-Wasm plugin's {name} is not of the type Proxy-Wasm calls it with"
        )));
    }
    Ok(Some(Export {
        f,
        results: t.results().len(),
    }))
}

/// Calls `e` with `args`: its result, or 1 when it returns none.
fn call(p: &mut Pooled<Host>, e: Export, args: &[u32]) -> Result<i32, Trap> {
    let args: Vec<Value> = args.iter().map(|&a| Value::I32(a as i32)).collect();
    let mut out = [Value::I32(1)];
    let results: &mut [Value] = if e.results == 0 { &mut [] } else { &mut out };
    p.call(e.f, &args, results)?;
    Ok(match out {
        [Value::I32(v)] => v,
        _ => 1,
    })
}

/// Starts an instance of a Proxy-Wasm plugin, once it is made: finds its functions and starts
/// its root context with the `vm_config` and the `config`.
pub(crate) fn start(p: &mut Pooled<Host>) -> Result<(), Trap> {
    let required = |p: &Pooled<Host>, name: &str, params, results| {
        export(p, name, params, results)?
            .ok_or_else(|| abi(&format!("the Proxy-Wasm plugin does not export {name}")))
    };
    let x = Exports {
        context_create: required(p, ON_CONTEXT_CREATE, 2, &[0, 1])?,
        request_headers: required(p, ON_REQUEST_HEADERS, 3, &[1])?,
        done: required(p, ON_DONE, 1, &[0, 1])?,
        log: required(p, ON_LOG, 1, &[0, 1])?,
        delete: required(p, ON_DELETE, 1, &[0, 1])?,
        allocate: required(p, ALLOCATE, 1, &[1])?,
        response_headers: export(p, ON_RESPONSE_HEADERS, 3, &[1])?,
        response_body: export(p, ON_RESPONSE_BODY, 3, &[1])?,
        response_trailers: export(p, ON_RESPONSE_TRAILERS, 2, &[1])?,
    };
    let vm_start = export(p, ON_VM_START, 2, &[0, 1])?;
    let configure = export(p, ON_CONFIGURE, 2, &[0, 1])?;
    p.state_mut().pw = Some(Box::new(PwInstance::new(x)));
    call(p, x.context_create, &[ROOT_CONTEXT, 0])?;
    let shared = Arc::clone(&p.state().shared);
    if let Some(f) = vm_start {
        if call(p, f, &[ROOT_CONTEXT, len32(shared.pw.vm_config.len())])? == 0 {
            return Err(abi("the plugin refused its vm_config"));
        }
    }
    if let Some(f) = configure {
        if call(p, f, &[ROOT_CONTEXT, len32(shared.config.len())])? == 0 {
            return Err(abi("the plugin refused its config"));
        }
    }
    Ok(())
}

/// The instance's Proxy-Wasm state, in a job.
pub(crate) fn instance(p: &mut Pooled<Host>) -> Option<&mut PwInstance> {
    p.state_mut().pw.as_deref_mut()
}

/// Runs the plugin's request callbacks for `req`, in a context of its own, on the pool's
/// instance `p`. A request the plugin follows keeps its context open for its response (unless
/// the plugin answered it), as many as `max_streams` at once; past them the request is answered
/// 503.
pub(crate) fn run_request(p: &mut Pooled<Host>, req: PwRequest, gone: &AtomicBool) -> Outcome {
    if gone.load(Ordering::Acquire) {
        return Outcome::Gone;
    }
    let max_streams = p.state().shared.pw.max_streams;
    let instance_id = p.id();
    let Some(pw) = instance(p) else {
        return Outcome::Failed;
    };
    if req.follows && pw.streams.len() >= max_streams {
        // As many responses as an instance follows are under way.
        return Outcome::Busy;
    }
    let x = pw.x;
    let id = pw.next_id();
    let n_headers = len32(req.headers.len().saturating_add(4));
    let end = u32::from(!req.has_body);
    pw.streams.insert(id, Stream::new(req));
    pw.current = Some(id);
    let r = call(p, x.context_create, &[id, ROOT_CONTEXT])
        .and_then(|_| call(p, x.request_headers, &[id, n_headers, end]));
    let Some(pw) = instance(p) else {
        return Outcome::Failed;
    };
    pw.current = None;
    let Ok(action) = r else {
        pw.streams.remove(&id);
        return Outcome::Failed;
    };
    let paused = action == ACTION_PAUSE;
    let Some(s) = pw.streams.get_mut(&id) else {
        return Outcome::Failed;
    };
    let (headers, method, target) = s.request_changes();
    let follow = !paused && s.answer.is_none() && s.req.follows;
    let ops = std::mem::take(&mut s.ops);
    let answer = s.answer.take();
    if follow {
        s.changes = 0;
        return Outcome::Ran(Ran {
            headers,
            method,
            target,
            ops,
            answer: None,
            follow: Some((instance_id, id)),
        });
    }
    // The stream is over, as far as the plugin is to know.
    if close_context(p, id).is_err() {
        return Outcome::Failed;
    }
    if paused && answer.is_none() {
        // A plugin that pauses a request without answering it waits for a resume akit does not
        // offer, as fit.
        return Outcome::Failed;
    }
    Outcome::Ran(Ran {
        headers,
        method,
        target,
        ops,
        answer,
        follow: None,
    })
}

/// Closes context `id`, once: done, logged and deleted, the plugin's calls able to read the
/// request meanwhile; the first that fails ends it.
pub(crate) fn close_context(p: &mut Pooled<Host>, id: u32) -> Result<(), Trap> {
    let Some(pw) = instance(p) else {
        return Ok(());
    };
    if !pw.streams.contains_key(&id) {
        return Ok(());
    }
    let x = pw.x;
    pw.current = Some(id);
    let r = call(p, x.done, &[id])
        .and_then(|_| call(p, x.log, &[id]))
        .and_then(|_| call(p, x.delete, &[id]));
    if let Some(pw) = instance(p) {
        pw.current = None;
        pw.streams.remove(&id);
    }
    r.map(drop)
}

/// Calls `e` for context `id`, the host functions acting on it meanwhile.
pub(crate) fn invoke(p: &mut Pooled<Host>, e: Export, id: u32, args: &[u32]) -> Result<i32, Trap> {
    if let Some(pw) = instance(p) {
        pw.current = Some(id);
        if let Some(s) = pw.streams.get_mut(&id) {
            s.changes = 0;
        }
    }
    let mut all = Vec::with_capacity(args.len().saturating_add(1));
    all.push(id);
    all.extend_from_slice(args);
    let r = call(p, e, &all);
    if let Some(pw) = instance(p) {
        pw.current = None;
    }
    r
}

type C<'a, 'b> = &'a mut Caller<'b, Host>;

/// The Proxy-Wasm state of the instance running a host function.
fn pw<'a>(c: &'a mut Caller<'_, Host>) -> Result<&'a mut PwInstance, Trap> {
    c.state_mut()
        .pw
        .as_deref_mut()
        .ok_or_else(|| abi("a host function called outside an instance"))
}

/// The context the call under way serves.
fn current<'a>(c: &'a mut Caller<'_, Host>) -> Result<&'a mut Stream, Trap> {
    let pw = pw(c)?;
    pw.current
        .and_then(|id| pw.streams.get_mut(&id))
        .ok_or_else(|| abi("a host function called outside a request"))
}

/// `n` bytes of the plugin's memory at `ptr`, copied: past [`MAX_BODY`], or outside its memory,
/// the call fails.
fn read(c: &Caller<'_, Host>, ptr: u32, n: u32) -> Result<Vec<u8>, Trap> {
    abi::bytes(c, ptr, n, MAX_BODY, "a string longer than allowed")
}

/// A field's name or value, or a log line, copied: past [`MAX_FIELD`] the call fails.
fn field(c: &Caller<'_, Host>, ptr: u32, n: u32) -> Result<Vec<u8>, Trap> {
    abi::bytes(c, ptr, n, MAX_FIELD, "a field longer than 64 KiB")
}

/// A field's name, lower case, if it is a token or a pseudo-header's (`:` and a token).
fn name_of(b: Vec<u8>) -> Option<String> {
    let s = String::from_utf8(b).ok()?;
    let token = s.strip_prefix(':').unwrap_or(&s);
    (!token.is_empty() && token.bytes().all(is_tchar)).then(|| s.to_ascii_lowercase())
}

/// A field's value, if it is UTF-8 without a line break or NUL.
fn value_of(b: Vec<u8>) -> Option<String> {
    let s = String::from_utf8(b).ok()?;
    (!s.bytes().any(|b| matches!(b, b'\r' | b'\n' | 0))).then_some(s)
}

/// Hands `data` to the plugin: memory it allocates (`proxy_on_memory_allocate`), whose address
/// and length go to the pointers at `ptr_ptr` and `size_ptr`.
fn give(c: C<'_, '_>, data: &[u8], ptr_ptr: u32, size_ptr: u32) -> Result<u32, Trap> {
    let len = u32::try_from(data.len()).map_err(|_| abi("a value longer than 4 GiB"))?;
    let mut p = 0;
    if len > 0 {
        let alloc = pw(c)?.x.allocate;
        let mut out = [Value::I32(0)];
        c.call(alloc.f, &[Value::I32(len as i32)], &mut out)?;
        p = match out {
            [Value::I32(v)] => v as u32,
            _ => return Ok(BAD_ARGUMENT),
        };
        if c.write(p, data).is_err() {
            return Ok(BAD_ARGUMENT);
        }
    }
    if c.write_u32(ptr_ptr, p).is_err() || c.write_u32(size_ptr, len).is_err() {
        return Ok(BAD_ARGUMENT);
    }
    Ok(OK)
}

/// A host function that answers `status` and does nothing else, of `params` i32 params.
fn status_fn(
    l: &mut Linker<Host>,
    name: &str,
    params: usize,
    status: u32,
) -> Result<(), LinkError> {
    let ty = FuncType::new(&vec![ValType::I32; params], &[ValType::I32]);
    l.func_new(ENV, name, ty, move |_, _, results| {
        if let Some(r) = results.first_mut() {
            *r = Value::I32(status as i32);
        }
        Ok(())
    })?;
    Ok(())
}

/// Adds the module `env`, Proxy-Wasm's host functions, to `l`.
pub(crate) fn link(l: &mut Linker<Host>) -> Result<(), LinkError> {
    l.func_wrap(
        ENV,
        "proxy_log",
        |c: C<'_, '_>, (level, mp, ml): (u32, u32, u32)| {
            pw(c)?;
            // Proxy-Wasm's trace and debug are debug, which akit drops (it has no plugin logger of
            // its own); info is info, warn warn, error and critical error.
            let level = match level {
                0 | 1 => return Ok(OK),
                2 => 0,
                3 => 4,
                _ => 8,
            };
            let msg = field(c, mp, ml)?;
            let shared = Arc::clone(&c.state().shared);
            if shared.may_log() {
                abi::write_log(&shared.path, level, &msg);
            }
            Ok(OK)
        },
    )?;
    l.func_wrap(
        ENV,
        "proxy_send_local_response",
        |c: C<'_, '_>,
         (status, _dp, _dl, bp, bl, pp, pl, grpc): (u32, u32, u32, u32, u32, u32, u32, i32)| {
            current(c)?;
            let Some(status) = u16::try_from(status)
                .ok()
                .filter(|s| (200..=599).contains(s))
            else {
                return Ok(BAD_ARGUMENT);
            };
            if bl > MAX_BODY {
                return Ok(BAD_ARGUMENT);
            }
            let body = read(c, bp, bl)?;
            let raw = read(c, pp, pl)?;
            let Some(pairs) = deserialize_pairs(&raw) else {
                return Ok(BAD_ARGUMENT);
            };
            let mut ops = Vec::with_capacity(pairs.len().saturating_add(1));
            for (k, v) in pairs {
                let (Some(k), Some(v)) = (name_of(k.into_bytes()), value_of(v.into_bytes())) else {
                    return Ok(BAD_ARGUMENT);
                };
                if k.starts_with(':') {
                    return Ok(BAD_ARGUMENT);
                }
                ops.push(Op::Add(canonical(&k), v));
            }
            if grpc >= 0 {
                ops.push(Op::Set("Grpc-Status".to_owned(), grpc.to_string()));
            }
            let s = current(c)?;
            for op in ops {
                s.push_op(op)?;
            }
            s.answer = Some((status, body));
            Ok(OK)
        },
    )?;
    l.func_wrap(
        ENV,
        "proxy_get_header_map_value",
        |c: C<'_, '_>, (map, np, nl, vp, vl): (u32, u32, u32, u32, u32)| {
            let raw = field(c, np, nl)?;
            let Some(name) = name_of(raw) else {
                return Ok(NOT_FOUND);
            };
            let s = current(c)?;
            let v = match map {
                REQUEST_HEADERS if name.starts_with(':') => s.req.pseudo(&name).map(str::to_owned),
                REQUEST_HEADERS if name == "host" => None,
                REQUEST_HEADERS => s.req.headers.get(&name).map(str::to_owned),
                RESPONSE_HEADERS if name == ":status" && s.status != 0 => {
                    Some(s.status.to_string())
                }
                RESPONSE_HEADERS => match &s.head {
                    Some(h) => h.get(&name).map(str::to_owned),
                    None if s.status == 0 => s.response(&name).map(str::to_owned),
                    None => None,
                },
                RESPONSE_TRAILERS => s
                    .trailers
                    .as_ref()
                    .and_then(|t| t.get(&name))
                    .map(str::to_owned),
                _ => None,
            };
            match v {
                Some(v) => give(c, v.as_bytes(), vp, vl),
                None => Ok(NOT_FOUND),
            }
        },
    )?;
    for (fname, set) in [
        ("proxy_replace_header_map_value", true),
        ("proxy_add_header_map_value", false),
    ] {
        l.func_wrap(
            ENV,
            fname,
            move |c: C<'_, '_>, (map, np, nl, vp, vl): (u32, u32, u32, u32, u32)| {
                let name = field(c, np, nl)?;
                let value = field(c, vp, vl)?;
                let (Some(name), Some(value)) = (name_of(name), value_of(value)) else {
                    return Ok(BAD_ARGUMENT);
                };
                change(current(c)?, map, Change::new(set, name, value))
            },
        )?;
    }
    l.func_wrap(
        ENV,
        "proxy_remove_header_map_value",
        |c: C<'_, '_>, (map, np, nl): (u32, u32, u32)| {
            let Some(name) = name_of(field(c, np, nl)?) else {
                return Ok(BAD_ARGUMENT);
            };
            change(current(c)?, map, Change::Del(name))
        },
    )?;
    l.func_wrap(
        ENV,
        "proxy_get_header_map_pairs",
        |c: C<'_, '_>, (map, vp, vl): (u32, u32, u32)| {
            let s = current(c)?;
            let pairs = match map {
                REQUEST_HEADERS => s.req.pairs(),
                RESPONSE_HEADERS => match &s.head {
                    Some(h) => {
                        let mut p = vec![(":status".to_owned(), s.status.to_string())];
                        field_pairs(&mut p, h);
                        p
                    }
                    None if s.status != 0 => vec![(":status".to_owned(), s.status.to_string())],
                    None => {
                        let mut h = Headers::new();
                        apply(&s.ops, &mut h);
                        let mut p = Vec::new();
                        field_pairs(&mut p, &h);
                        p
                    }
                },
                RESPONSE_TRAILERS => {
                    let mut p = Vec::new();
                    if let Some(t) = &s.trailers {
                        field_pairs(&mut p, t);
                    }
                    p
                }
                _ => return Ok(NOT_FOUND),
            };
            give(c, &serialize_pairs(&pairs), vp, vl)
        },
    )?;
    l.func_wrap(
        ENV,
        "proxy_set_header_map_pairs",
        |c: C<'_, '_>, (map, pp, pl): (u32, u32, u32)| {
            let raw = read(c, pp, pl)?;
            let s = current(c)?;
            let Some(pairs) = deserialize_pairs(&raw) else {
                return Ok(BAD_ARGUMENT);
            };
            match map {
                REQUEST_HEADERS => {
                    s.req.headers = Headers::new();
                    s.fields_changed = true;
                }
                RESPONSE_HEADERS => match &mut s.head {
                    Some(h) => *h = Headers::new(),
                    None if s.status != 0 => return Ok(BAD_ARGUMENT),
                    None => s.ops.clear(),
                },
                RESPONSE_TRAILERS => match &mut s.trailers {
                    Some(t) => *t = Headers::new(),
                    None => return Ok(BAD_ARGUMENT),
                },
                _ => return Ok(NOT_FOUND),
            }
            for (k, v) in pairs {
                let (Some(k), Some(v)) = (name_of(k.into_bytes()), value_of(v.into_bytes())) else {
                    return Ok(BAD_ARGUMENT);
                };
                let pseudo = k.starts_with(':');
                if pseudo && map != REQUEST_HEADERS {
                    // The status stays as it is.
                    continue;
                }
                let r = change(s, map, Change::new(pseudo, k, v))?;
                if r != OK {
                    return Ok(r);
                }
            }
            Ok(OK)
        },
    )?;
    l.func_wrap(
        ENV,
        "proxy_get_buffer_bytes",
        |c: C<'_, '_>, (buf, start, size, vp, vl): (u32, u32, u32, u32, u32)| {
            let shared = Arc::clone(&c.state().shared);
            let pw = pw(c)?;
            let whole: Vec<u8> = match buf {
                VM_CONFIGURATION => shared.pw.vm_config.as_bytes().to_vec(),
                PLUGIN_CONFIGURATION => shared.config.as_bytes().to_vec(),
                RESPONSE_BODY => match pw.current.and_then(|id| pw.streams.get(&id)) {
                    Some(s) if s.in_body => {
                        let Some(part) = window(&s.body, start, size) else {
                            return Ok(BAD_ARGUMENT);
                        };
                        let part = part.to_vec();
                        return give(c, &part, vp, vl);
                    }
                    _ => return Ok(NOT_FOUND),
                },
                _ => return Ok(NOT_FOUND),
            };
            match window(&whole, start, size) {
                Some(part) => give(c, part, vp, vl),
                None => Ok(BAD_ARGUMENT),
            }
        },
    )?;
    l.func_wrap(
        ENV,
        "proxy_set_buffer_bytes",
        |c: C<'_, '_>, (buf, start, size, dp, dl): (u32, u32, u32, u32, u32)| {
            let max = c.state().shared.pw.max_body_bytes;
            {
                let pw = pw(c)?;
                let live = pw
                    .current
                    .and_then(|id| pw.streams.get(&id))
                    .is_some_and(|s| s.in_body);
                if buf != RESPONSE_BODY || !live {
                    return Ok(NOT_FOUND);
                }
            }
            let data = read(c, dp, dl)?;
            Ok(current(c)?.set_body(start, size, &data, max))
        },
    )?;
    l.func_wrap(
        ENV,
        "proxy_get_property",
        |c: C<'_, '_>, (pp, pl, vp, vl): (u32, u32, u32, u32)| {
            let raw = read(c, pp, pl)?;
            let path = String::from_utf8_lossy(&raw);
            let path = path.strip_suffix('\0').unwrap_or(&path).to_owned();
            match property(c, &path)? {
                Some(v) => give(c, v.as_bytes(), vp, vl),
                None => Ok(NOT_FOUND),
            }
        },
    )?;
    status_fn(l, "proxy_set_effective_context", 1, OK)?;
    status_fn(l, "proxy_done", 0, OK)?;
    l.func_wrap(
        ENV,
        "proxy_define_metric",
        |c: C<'_, '_>, (_kind, np, nl, ip): (u32, u32, u32, u32)| {
            let name = String::from_utf8_lossy(&field(c, np, nl)?).into_owned();
            let pw = pw(c)?;
            let id = match pw.metric_names.get(&name) {
                Some(&id) => id,
                None => {
                    if pw.metrics.len() >= MAX_METRICS {
                        return Ok(BAD_ARGUMENT);
                    }
                    let id = len32(pw.metrics.len());
                    pw.metrics.push(0);
                    pw.metric_names.insert(name, id);
                    id
                }
            };
            Ok(if c.write_u32(ip, id).is_ok() {
                OK
            } else {
                BAD_ARGUMENT
            })
        },
    )?;
    for (fname, increment) in [
        ("proxy_increment_metric", true),
        ("proxy_record_metric", false),
    ] {
        l.func_wrap(ENV, fname, move |c: C<'_, '_>, (id, v): (u32, i64)| {
            let pw = pw(c)?;
            let Some(m) = usize::try_from(id).ok().and_then(|i| pw.metrics.get_mut(i)) else {
                return Ok(NOT_FOUND);
            };
            *m = if increment { m.wrapping_add(v) } else { v };
            Ok(OK)
        })?;
    }
    l.func_wrap(
        ENV,
        "proxy_get_metric",
        |c: C<'_, '_>, (id, vp): (u32, u32)| {
            let pw = pw(c)?;
            let Some(&m) = usize::try_from(id).ok().and_then(|i| pw.metrics.get(i)) else {
                return Ok(NOT_FOUND);
            };
            Ok(if c.write_u64(vp, m as u64).is_ok() {
                OK
            } else {
                BAD_ARGUMENT
            })
        },
    )?;
    for (name, params) in [
        ("proxy_set_property", 4),
        ("proxy_continue_stream", 1),
        ("proxy_close_stream", 1),
        ("proxy_http_call", 10),
        ("proxy_call_foreign_function", 6),
        ("proxy_set_tick_period_milliseconds", 1),
        ("proxy_get_shared_data", 5),
        ("proxy_set_shared_data", 5),
        ("proxy_register_shared_queue", 3),
        ("proxy_resolve_shared_queue", 5),
        ("proxy_dequeue_shared_queue", 3),
        ("proxy_enqueue_shared_queue", 3),
    ] {
        status_fn(l, name, params, UNIMPLEMENTED)?;
    }
    Ok(())
}

/// `data[start..start + size]`, cut at its end; `None` when `start` is past it.
fn window(data: &[u8], start: u32, size: u32) -> Option<&[u8]> {
    let start = usize::try_from(start).ok()?;
    let end = start
        .saturating_add(usize::try_from(size).unwrap_or(usize::MAX))
        .min(data.len());
    data.get(start..end)
}

/// A change to a header map.
#[derive(Debug)]
enum Change {
    Set(String, String),
    Add(String, String),
    Del(String),
}

impl Change {
    fn new(set: bool, name: String, value: String) -> Change {
        if set {
            Change::Set(name, value)
        } else {
            Change::Add(name, value)
        }
    }

    fn into_op(self) -> Op {
        match self {
            Change::Set(n, v) => Op::Set(canonical(&n), v),
            Change::Add(n, v) => Op::Add(canonical(&n), v),
            Change::Del(n) => Op::Del(canonical(&n)),
        }
    }

    fn name(&self) -> &str {
        match self {
            Change::Set(n, _) | Change::Add(n, _) | Change::Del(n) => n,
        }
    }
}

/// Makes `ch` to the map `map` of `s`: the request's header (its pseudo-headers too), the
/// response's while the plugin has it (or its fields to be, before there is one), or its trailer
/// at the end of its body.
fn change(s: &mut Stream, map: u32, ch: Change) -> Result<u32, Trap> {
    let pseudo = ch.name().starts_with(':');
    match map {
        REQUEST_HEADERS if pseudo || ch.name() == "host" => {
            // `Host` is the request's `:authority`, as Go's request keeps it.
            let (Change::Set(n, v) | Change::Add(n, v)) = ch else {
                return Ok(BAD_ARGUMENT);
            };
            if !set_pseudo(&mut s.req, &n, v) {
                return Ok(BAD_ARGUMENT);
            }
            s.count()?;
            s.pseudo_changed = true;
        }
        REQUEST_HEADERS => {
            s.count()?;
            apply(std::slice::from_ref(&ch.into_op()), &mut s.req.headers);
            s.fields_changed = true;
        }
        RESPONSE_HEADERS => {
            if s.head.is_some() {
                if pseudo {
                    // Neither the status nor anything else of the head can change.
                    return Ok(BAD_ARGUMENT);
                }
                s.count()?;
                let op = ch.into_op();
                if let Some(h) = &mut s.head {
                    apply(std::slice::from_ref(&op), h);
                }
            } else if s.status != 0 || pseudo {
                // The header has gone.
                return Ok(BAD_ARGUMENT);
            } else {
                s.push_op(ch.into_op())?;
            }
        }
        RESPONSE_TRAILERS => {
            if s.trailers.is_none() || pseudo {
                return Ok(BAD_ARGUMENT);
            }
            s.count()?;
            let op = ch.into_op();
            if let Some(t) = &mut s.trailers {
                apply(std::slice::from_ref(&op), t);
            }
        }
        _ => return Ok(NOT_FOUND),
    }
    Ok(OK)
}

/// Sets one of the request's pseudo-headers (`host` is `:authority`), and reports whether `name`
/// is one it can set to `value`.
fn set_pseudo(r: &mut PwRequest, name: &str, value: String) -> bool {
    let visible = |v: &str| !v.is_empty() && v.bytes().all(|b| b > b' ' && b != 0x7f);
    match name {
        ":method" if !value.is_empty() && value.bytes().all(is_tchar) => r.method = value,
        ":authority" | "host" if visible(&value) => r.host = value,
        ":path" if value.starts_with('/') && visible(&value) => r.target = value,
        _ => return false,
    }
    true
}

/// The value of the property at `path` (its segments joined by NUL): one the config sets, the
/// plugin's name, or one of the request's.
fn property(c: C<'_, '_>, path: &str) -> Result<Option<String>, Trap> {
    let shared = Arc::clone(&c.state().shared);
    if let Some((_, v)) = shared.pw.properties.iter().find(|(p, _)| p == path) {
        return Ok(Some(v.clone()));
    }
    if path == "plugin_name" {
        return Ok(Some(shared.path.clone()));
    }
    let pw = pw(c)?;
    let Some(s) = pw.current.and_then(|id| pw.streams.get(&id)) else {
        return Ok(None);
    };
    let r = &s.req;
    let v = match path {
        "request\0path" => r.target.clone(),
        "request\0url_path" => r.url_path(),
        "request\0query" => r.query().to_owned(),
        "request\0host" => r.host.clone(),
        "request\0method" => r.method.clone(),
        "request\0scheme" => r.scheme.to_owned(),
        "request\0protocol" => r.protocol.to_owned(),
        "source\0address" => r.remote.clone(),
        "response\0code" if s.status != 0 => s.status.to_string(),
        _ => return Ok(None),
    };
    Ok(Some(v))
}

/// The id of an instance and a context on it: where a followed response's calls go.
pub(crate) type Ticket = (InstanceId, u32);

/// fit's `TestHostProxyWasm`, `TestHostProxyWasmResponse`, `TestHostModules`, `TestPairs` and
/// `TestBodyBuffer`: the host functions, called through a guest of the tests' own that forwards
/// to each, on a request of the tests'; and the encodings.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation
)]
mod tests {
    use std::time::{Duration, Instant};

    use akio_wasm::{Budget, Extern, Instance, InstanceLimits, Module, ModuleLimits, Store};

    use super::*;
    use crate::abi::{MAX_LOGS, Shared};
    use crate::encode::{
        self, BLOCK_EMPTY, Func, Import, OP_CALL, OP_END, OP_I32_CONST, OP_I32_EQZ, OP_I32_LOAD,
        OP_I64_EXTEND_I32_S, OP_IF, OP_LOCAL_GET, OP_UNREACHABLE, TYPE_I32, TYPE_I64,
    };

    /// The host functions the guest forwards to: name, i32 params (`usize::MAX` for an i32 and
    /// an i64), results.
    const FUNCS: [(&str, usize); 18] = [
        ("proxy_log", 3),
        ("proxy_send_local_response", 8),
        ("proxy_get_header_map_value", 5),
        ("proxy_add_header_map_value", 5),
        ("proxy_replace_header_map_value", 5),
        ("proxy_remove_header_map_value", 3),
        ("proxy_get_header_map_pairs", 3),
        ("proxy_set_header_map_pairs", 3),
        ("proxy_get_buffer_bytes", 5),
        ("proxy_set_buffer_bytes", 5),
        ("proxy_get_property", 4),
        ("proxy_set_effective_context", 1),
        ("proxy_done", 0),
        ("proxy_define_metric", 4),
        ("proxy_increment_metric", usize::MAX),
        ("proxy_record_metric", usize::MAX),
        ("proxy_get_metric", 2),
        ("proxy_http_call", 10),
    ];

    /// Where the guest's `proxy_on_memory_allocate` reads the address it returns (0: it traps).
    const ALLOC_AT: u32 = 8;
    const ALLOC: u32 = 16384;
    const STRINGS: u32 = 16;
    const PTR: u32 = 12000;
    const SIZE: u32 = 12004;
    const BUF: u32 = 20000;
    const BAD: u32 = 0xffff_ff00;

    fn guest() -> Arc<Module> {
        let mut imports = Vec::new();
        let mut funcs = Vec::new();
        for (i, (name, params)) in FUNCS.iter().enumerate() {
            let (import, func) = if *params == usize::MAX {
                let mut im = Import::new(ENV, name, 2, 1);
                im.param_types = vec![TYPE_I32, TYPE_I64];
                let body = vec![
                    OP_LOCAL_GET,
                    0,
                    OP_LOCAL_GET,
                    1,
                    OP_I64_EXTEND_I32_S,
                    OP_CALL,
                    i as u8,
                ];
                (im, Func::export(name, 2, 1, body))
            } else {
                (
                    Import::new(ENV, name, *params, 1),
                    Func::export(name, *params, 1, encode::forward(i as u32, *params)),
                )
            };
            imports.push(import);
            funcs.push(func);
        }
        let load = [OP_I32_CONST, ALLOC_AT as u8, OP_I32_LOAD, 2, 0];
        let mut body = load.to_vec();
        body.extend([OP_I32_EQZ, OP_IF, BLOCK_EMPTY, OP_UNREACHABLE, OP_END]);
        body.extend(load);
        funcs.push(Func::export(ALLOCATE, 1, 1, body));
        let m = encode::Module {
            imports,
            funcs,
            pages: 1,
            data: Vec::new(),
        };
        Arc::new(Module::new(&m.encode(), &ModuleLimits::default()).unwrap())
    }

    /// A store with an instance of the guest, serving a request of the tests' as context 2.
    struct Env {
        store: Store<Host>,
        inst: Instance,
        next: u32,
    }

    fn request() -> PwRequest {
        let mut headers = Headers::new();
        headers.append("X-A", "1");
        PwRequest {
            method: "GET".into(),
            target: "/p/q?x=1".into(),
            host: "example.com".into(),
            scheme: "http",
            protocol: "HTTP/1.1",
            remote: "10.0.0.1:5".into(),
            headers,
            follows: false,
            has_body: false,
        }
    }

    impl Env {
        fn new() -> Env {
            let mut l = Linker::new();
            link(&mut l).unwrap();
            let mut shared = Shared::new("test.wasm", "the config");
            shared.pw = PwShared {
                vm_config: "the vm config".into(),
                properties: vec![("a\0b".into(), "ab".into())],
                max_body_bytes: 8,
                max_streams: 4,
            };
            let mut store = Store::new(Host::new(Arc::new(shared)), InstanceLimits::default());
            let inst = l.instantiate(&mut store, &guest(), &budget()).unwrap();
            let alloc = store.export_func(inst, ALLOCATE).unwrap();
            let e = Export {
                f: alloc,
                results: 1,
            };
            let mut pw = PwInstance::new(Exports {
                context_create: e,
                request_headers: e,
                done: e,
                log: e,
                delete: e,
                allocate: e,
                response_headers: None,
                response_body: None,
                response_trailers: None,
            });
            pw.streams.insert(2, Stream::new(request()));
            pw.current = Some(2);
            store.state_mut().pw = Some(Box::new(pw));
            let mut env = Env {
                store,
                inst,
                next: STRINGS,
            };
            env.set_alloc(ALLOC);
            env
        }

        fn memory(&self) -> akio_wasm::Memory {
            match self.store.export(self.inst, "memory") {
                Some(Extern::Memory(m)) => m,
                _ => panic!("no memory"),
            }
        }

        fn write(&mut self, at: u32, b: &[u8]) {
            let m = self.memory();
            self.store.memory_data_mut(m)[at as usize..at as usize + b.len()].copy_from_slice(b);
        }

        fn bytes(&self, at: u32, n: u32) -> Vec<u8> {
            let m = self.memory();
            self.store.memory_data(m)[at as usize..(at + n) as usize].to_vec()
        }

        fn u32_at(&self, at: u32) -> u32 {
            u32::from_le_bytes(self.bytes(at, 4).try_into().unwrap())
        }

        fn set_alloc(&mut self, v: u32) {
            self.write(ALLOC_AT, &v.to_le_bytes());
        }

        /// Writes `s` into the guest's memory: its address and length, as two args.
        fn put(&mut self, s: &str) -> [u32; 2] {
            let p = self.next;
            self.write(p, s.as_bytes());
            self.next += s.len() as u32;
            assert!(self.next < PTR, "out of room for strings");
            [p, s.len() as u32]
        }

        /// What `give` handed over to `PTR` and `SIZE`.
        fn given(&self) -> Vec<u8> {
            let (p, n) = (self.u32_at(PTR), self.u32_at(SIZE));
            if n == 0 {
                return Vec::new();
            }
            self.bytes(p, n)
        }

        fn given_str(&self) -> String {
            String::from_utf8(self.given()).unwrap()
        }

        fn clear_given(&mut self) {
            self.write(PTR, &[0; 8]);
        }

        /// Calls the host function `name` with `args`: its status, or its trap's message.
        fn call(&mut self, name: &str, args: &[u32]) -> Result<u32, String> {
            let f = self.store.export_func(self.inst, name).unwrap();
            let args: Vec<Value> = args.iter().map(|&a| Value::I32(a as i32)).collect();
            let mut out = [Value::I32(0)];
            self.store
                .call(f, &args, &mut out, &budget())
                .map_err(|t| t.message().to_owned())?;
            Ok(match out {
                [Value::I32(v)] => v as u32,
                _ => 0,
            })
        }

        fn ok(&mut self, name: &str, args: &[u32]) -> u32 {
            self.call(name, args)
                .unwrap_or_else(|e| panic!("{name}: trapped with {e}"))
        }

        fn stream(&mut self) -> &mut Stream {
            let pw = self.store.state_mut().pw.as_deref_mut().unwrap();
            pw.streams.get_mut(&2).unwrap()
        }

        fn pw(&mut self) -> &mut PwInstance {
            self.store.state_mut().pw.as_deref_mut().unwrap()
        }

        /// `proxy_get_header_map_value`: the value, or the status.
        fn get(&mut self, map: u32, name: &str) -> Result<String, u32> {
            let n = self.put(name);
            self.clear_given();
            match self.ok("proxy_get_header_map_value", &[map, n[0], n[1], PTR, SIZE]) {
                OK => Ok(self.given_str()),
                s => Err(s),
            }
        }

        fn change(&mut self, f: &str, map: u32, name: &str, value: &str) -> u32 {
            let n = self.put(name);
            let v = self.put(value);
            self.ok(f, &[map, n[0], n[1], v[0], v[1]])
        }

        fn pairs(&mut self, map: u32) -> Result<Vec<(String, String)>, u32> {
            self.clear_given();
            match self.ok("proxy_get_header_map_pairs", &[map, PTR, SIZE]) {
                OK => Ok(deserialize_pairs(&self.given()).unwrap()),
                s => Err(s),
            }
        }

        fn set_pairs(&mut self, map: u32, pairs: &[(&str, &str)]) -> u32 {
            let pairs: Vec<(String, String)> = pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect();
            let b = serialize_pairs(&pairs);
            self.write(BUF, &b);
            self.ok("proxy_set_header_map_pairs", &[map, BUF, b.len() as u32])
        }

        fn property(&mut self, path: &str) -> Result<String, u32> {
            let p = self.put(path);
            self.clear_given();
            match self.ok("proxy_get_property", &[p[0], p[1], PTR, SIZE]) {
                OK => Ok(self.given_str()),
                s => Err(s),
            }
        }
    }

    fn budget() -> Budget {
        Budget::deadline(Instant::now() + Duration::from_secs(10))
    }

    fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
        p.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn outside() {
        let mut e = Env::new();
        let line = e.put("a line");
        e.store.state_mut().pw = None;
        let r = e.call("proxy_log", &[2, line[0], line[1]]);
        assert!(
            matches!(&r, Err(m) if m.contains("outside an instance")),
            "{r:?}"
        );
        let mut e = Env::new();
        e.pw().current = None;
        let r = e.call("proxy_get_header_map_pairs", &[REQUEST_HEADERS, PTR, SIZE]);
        assert!(
            matches!(&r, Err(m) if m.contains("outside a request")),
            "{r:?}"
        );
        // The configuration, metrics and plugin's properties need no request (a trap ended the
        // store: another one).
        let mut e = Env::new();
        e.pw().current = None;
        e.clear_given();
        assert_eq!(
            e.ok(
                "proxy_get_buffer_bytes",
                &[PLUGIN_CONFIGURATION, 0, 100, PTR, SIZE]
            ),
            OK
        );
        assert_eq!(e.property("plugin_name"), Ok("test.wasm".into()));
        assert_eq!(e.property("request\0path"), Err(NOT_FOUND));
    }

    #[test]
    fn host_proxy_wasm() {
        let mut e = Env::new();
        let line = e.put("a line");
        for level in [1, 2, 3, 4] {
            assert_eq!(e.ok("proxy_log", &[level, line[0], line[1]]), OK);
        }
        // Debug lines are dropped before they are read.
        assert_eq!(e.ok("proxy_log", &[0, BAD, 4]), OK);
        let shared = Arc::clone(&e.store.state().shared);
        for _ in 0..MAX_LOGS * 2 {
            e.ok("proxy_log", &[2, line[0], line[1]]);
        }
        assert!(!shared.may_log(), "lines past 100 a second are dropped");

        // proxy_get_buffer_bytes, and give, which hands its data over.
        for (buf, start, size, status, want) in [
            (PLUGIN_CONFIGURATION, 0, 100, OK, "the config"),
            (PLUGIN_CONFIGURATION, 4, 3, OK, "con"),
            (PLUGIN_CONFIGURATION, 10, 5, OK, ""),
            (VM_CONFIGURATION, 0, 100, OK, "the vm config"),
            (PLUGIN_CONFIGURATION, 11, 1, BAD_ARGUMENT, ""),
            (9, 0, 1, NOT_FOUND, ""),
            // The request's body is not the plugin's.
            (0, 0, 1, NOT_FOUND, ""),
        ] {
            e.clear_given();
            let s = e.ok("proxy_get_buffer_bytes", &[buf, start, size, PTR, SIZE]);
            assert_eq!(s, status, "buffer {buf} {start}");
            if s == OK {
                assert_eq!(e.given_str(), want, "buffer {buf} {start}");
            }
        }
        let args = [PLUGIN_CONFIGURATION, 0, 100, BAD, SIZE];
        assert_eq!(e.ok("proxy_get_buffer_bytes", &args), BAD_ARGUMENT);
        e.set_alloc(0xffff_ff00);
        let args = [PLUGIN_CONFIGURATION, 0, 100, PTR, SIZE];
        assert_eq!(
            e.ok("proxy_get_buffer_bytes", &args),
            BAD_ARGUMENT,
            "memory allocated outside memory"
        );
        e.set_alloc(ALLOC);

        // proxy_send_local_response
        let body = e.put("no");
        let pw_pairs = serialize_pairs(&pairs(&[("x-l", "1")]));
        e.write(BUF, &pw_pairs);
        let p = [BUF, pw_pairs.len() as u32];
        let bad = e.put("\x01\0\0");
        for (what, args, status) in [
            (
                "a status out of range",
                [99, 0, 0, body[0], body[1], p[0], p[1], u32::MAX],
                BAD_ARGUMENT,
            ),
            (
                "a body too long",
                [200, 0, 0, 0, MAX_BODY + 1, p[0], p[1], u32::MAX],
                BAD_ARGUMENT,
            ),
            (
                "bad pairs",
                [200, 0, 0, body[0], body[1], bad[0], bad[1], u32::MAX],
                BAD_ARGUMENT,
            ),
            (
                "no gRPC status",
                [403, 0, 0, body[0], body[1], p[0], p[1], u32::MAX],
                OK,
            ),
        ] {
            e.stream().ops.clear();
            assert_eq!(e.ok("proxy_send_local_response", &args), status, "{what}");
        }
        assert_eq!(e.stream().answer, Some((403, b"no".to_vec())));
        assert_eq!(e.stream().response("grpc-status"), None);
        e.stream().ops.clear();
        let args = [200, 0, 0, body[0], body[1], p[0], p[1], 7];
        assert_eq!(e.ok("proxy_send_local_response", &args), OK);
        assert_eq!(e.stream().response("grpc-status"), Some("7"));
        assert_eq!(e.stream().response("x-l"), Some("1"));
        e.stream().answer = None;

        // proxy_get_header_map_value
        for (map, name, want) in [
            (REQUEST_HEADERS, ":path", Ok("/p/q?x=1")),
            (REQUEST_HEADERS, ":scheme", Ok("http")),
            (REQUEST_HEADERS, ":authority", Ok("example.com")),
            (REQUEST_HEADERS, ":nope", Err(NOT_FOUND)),
            (REQUEST_HEADERS, "x-a", Ok("1")),
            (REQUEST_HEADERS, "X-A", Ok("1")),
            (REQUEST_HEADERS, "x-none", Err(NOT_FOUND)),
            (RESPONSE_HEADERS, "x-l", Ok("1")),
            (5, "x-a", Err(NOT_FOUND)),
        ] {
            assert_eq!(e.get(map, name), want.map(str::to_owned), "{map} {name}");
        }
        e.stream().req.scheme = "https";
        assert_eq!(e.get(REQUEST_HEADERS, ":scheme"), Ok("https".into()));
        e.stream().req.scheme = "http";

        // proxy_replace_header_map_value, proxy_add_header_map_value and
        // proxy_remove_header_map_value
        let (set, add) = (
            "proxy_replace_header_map_value",
            "proxy_add_header_map_value",
        );
        for (f, map, name, value, status) in [
            (set, REQUEST_HEADERS, ":method", "POST", OK),
            (set, REQUEST_HEADERS, ":authority", "other.example", OK),
            (set, REQUEST_HEADERS, ":path", "/n?y=2", OK),
            (set, REQUEST_HEADERS, ":path", "not a path", BAD_ARGUMENT),
            (set, REQUEST_HEADERS, ":scheme", "https", BAD_ARGUMENT),
            (set, REQUEST_HEADERS, ":method", "NOT A TOKEN", BAD_ARGUMENT),
            (set, REQUEST_HEADERS, "x-b", "3", OK),
            (add, REQUEST_HEADERS, "x-b", "4", OK),
            (set, REQUEST_HEADERS, "x b", "4", BAD_ARGUMENT),
            (set, REQUEST_HEADERS, "x-c", "a\r\nX-Evil: 1", BAD_ARGUMENT),
            (set, RESPONSE_HEADERS, "x-c", "5", OK),
            (add, 5, "x-c", "5", NOT_FOUND),
        ] {
            assert_eq!(e.change(f, map, name, value), status, "{name} {value}");
        }
        let s = e.stream();
        assert_eq!(
            (&*s.req.method, &*s.req.host, &*s.req.target),
            ("POST", "other.example", "/n?y=2")
        );
        assert_eq!(s.req.headers.get_all("x-b").collect::<Vec<_>>(), ["3", "4"]);
        let (headers, method, target) = s.request_changes();
        assert_eq!(
            (method.as_deref(), target.as_deref()),
            (Some("POST"), Some("/n?y=2"))
        );
        assert_eq!(headers.unwrap().get("host"), Some("other.example"));
        let m = e.put(":method");
        assert_eq!(
            e.ok(
                "proxy_remove_header_map_value",
                &[REQUEST_HEADERS, m[0], m[1]]
            ),
            BAD_ARGUMENT
        );
        let b = e.put("x-b");
        assert_eq!(
            e.ok(
                "proxy_remove_header_map_value",
                &[REQUEST_HEADERS, b[0], b[1]]
            ),
            OK
        );
        assert_eq!(e.stream().req.headers.get("x-b"), None);

        // proxy_get_header_map_pairs
        let p = e.pairs(REQUEST_HEADERS).unwrap();
        assert!(p.contains(&(":method".into(), "POST".into())), "{p:?}");
        assert!(p.contains(&("x-a".into(), "1".into())), "{p:?}");
        let p = e.pairs(RESPONSE_HEADERS).unwrap();
        assert!(p.contains(&("x-c".into(), "5".into())), "{p:?}");
        assert_eq!(e.pairs(5), Err(NOT_FOUND));

        // proxy_set_header_map_pairs
        assert_eq!(
            e.ok(
                "proxy_set_header_map_pairs",
                &[REQUEST_HEADERS, bad[0], bad[1]]
            ),
            BAD_ARGUMENT
        );
        let r = e.call(
            "proxy_set_header_map_pairs",
            &[REQUEST_HEADERS, 0, MAX_BODY + 1],
        );
        assert!(
            matches!(&r, Err(m) if m.contains("longer than allowed")),
            "{r:?}"
        );
        let mut e = Env::new();
        assert_eq!(
            e.set_pairs(REQUEST_HEADERS, &[(":path", "/s"), ("x-d", "4")]),
            OK
        );
        let s = e.stream();
        assert_eq!(s.req.target, "/s");
        assert_eq!(s.req.headers.len(), 1);
        assert_eq!(s.req.headers.get("X-D"), Some("4"));
        assert_eq!(e.set_pairs(RESPONSE_HEADERS, &[("x-e", "5")]), OK);
        assert_eq!(e.stream().ops, [Op::Add("X-E".into(), "5".into())]);
        assert_eq!(e.set_pairs(5, &[("x-e", "5")]), NOT_FOUND);
        assert_eq!(
            e.set_pairs(REQUEST_HEADERS, &[(":scheme", "https")]),
            BAD_ARGUMENT
        );

        // proxy_get_property
        e.stream().req.target = "/s%20t?z=1".into();
        for (path, want) in [
            ("a\0b\0", Ok("ab")),
            ("plugin_name", Ok("test.wasm")),
            ("request\0path", Ok("/s%20t?z=1")),
            ("request\0url_path", Ok("/s t")),
            ("request\0query", Ok("z=1")),
            ("request\0host", Ok("example.com")),
            ("request\0method", Ok("GET")),
            ("request\0scheme", Ok("http")),
            ("request\0protocol", Ok("HTTP/1.1")),
            ("source\0address", Ok("10.0.0.1:5")),
            ("request\0nope", Err(NOT_FOUND)),
            ("request", Err(NOT_FOUND)),
            ("request\0path\0x", Err(NOT_FOUND)),
            ("response\0code", Err(NOT_FOUND)),
        ] {
            assert_eq!(e.property(path), want.map(str::to_owned), "{path:?}");
        }
        let r = Env::new().call("proxy_get_property", &[0, MAX_BODY + 1, PTR, SIZE]);
        assert!(
            matches!(&r, Err(m) if m.contains("longer than allowed")),
            "{r:?}"
        );

        // Metrics
        let define = |e: &mut Env, name: &str, at: u32| {
            let n = e.put(name);
            let s = e.ok("proxy_define_metric", &[0, n[0], n[1], at]);
            (e.u32_at(PTR), s)
        };
        assert_eq!(define(&mut e, "m1", PTR), (0, OK));
        assert_eq!(define(&mut e, "m2", PTR), (1, OK));
        assert_eq!(define(&mut e, "m1", PTR), (0, OK));
        assert_eq!(define(&mut e, "m3", BAD).1, BAD_ARGUMENT);
        for (f, v, want) in [
            ("proxy_increment_metric", 5, 5),
            ("proxy_increment_metric", 5, 10),
            ("proxy_record_metric", 3, 3),
            ("proxy_increment_metric", -4i32 as u32, -1),
        ] {
            assert_eq!(e.ok(f, &[0, v]), OK);
            assert_eq!(e.pw().metrics[0], want);
        }
        assert_eq!(e.ok("proxy_increment_metric", &[999, 1]), NOT_FOUND);
        assert_eq!(e.ok("proxy_get_metric", &[0, BUF]), OK);
        assert_eq!(e.bytes(BUF, 8), (-1i64).to_le_bytes());
        assert_eq!(e.ok("proxy_get_metric", &[999, BUF]), NOT_FOUND);
        assert_eq!(e.ok("proxy_get_metric", &[0, BAD]), BAD_ARGUMENT);
        while e.pw().metrics.len() < MAX_METRICS {
            e.pw().metrics.push(0);
        }
        e.next = STRINGS;
        assert_eq!(define(&mut e, "one too many", PTR).1, BAD_ARGUMENT);

        // fit's TestHostModules: proxy_done and what akit does not offer.
        assert_eq!(e.ok("proxy_done", &[]), OK);
        assert_eq!(e.ok("proxy_set_effective_context", &[2]), OK);
        assert_eq!(e.ok("proxy_http_call", &[0; 10]), UNIMPLEMENTED);
    }

    #[test]
    fn an_allocation_that_traps_fails_the_call() {
        let mut e = Env::new();
        e.set_alloc(0);
        let r = e.call(
            "proxy_get_buffer_bytes",
            &[PLUGIN_CONFIGURATION, 0, 100, PTR, SIZE],
        );
        assert_eq!(r, Err("unreachable".to_owned()));
    }

    #[test]
    fn changes_are_bounded() {
        let mut e = Env::new();
        for _ in 0..MAX_CHANGES {
            e.next = STRINGS;
            assert_eq!(
                e.change("proxy_add_header_map_value", REQUEST_HEADERS, "x-m", "v"),
                OK
            );
        }
        e.next = STRINGS;
        let n = e.put("x-m");
        let v = e.put("v");
        let r = e.call(
            "proxy_add_header_map_value",
            &[REQUEST_HEADERS, n[0], n[1], v[0], v[1]],
        );
        assert!(
            matches!(&r, Err(m) if m.contains("more than 1024")),
            "{r:?}"
        );
        let mut e = Env::new();
        for _ in 0..MAX_OPS {
            e.next = STRINGS;
            e.change("proxy_add_header_map_value", RESPONSE_HEADERS, "x-m", "v");
        }
        e.next = STRINGS;
        let n = e.put("x-m");
        let r = e.call(
            "proxy_add_header_map_value",
            &[RESPONSE_HEADERS, n[0], n[1], n[0], n[1]],
        );
        assert!(matches!(&r, Err(m) if m.contains("more than 256")), "{r:?}");
    }

    /// fit's `TestHostProxyWasmResponse`: the host functions as the response's callbacks call
    /// them: on the head while `proxy_on_response_headers` runs, on the status once it has gone,
    /// on the trailer and on the body.
    #[test]
    fn host_proxy_wasm_response() {
        let mut e = Env::new();
        // While proxy_on_response_headers runs, the head is the response's own.
        let mut h = Headers::new();
        h.append("X-R", "r");
        e.stream().status = 201;
        e.stream().head = Some(h);
        assert_eq!(e.get(RESPONSE_HEADERS, ":status"), Ok("201".into()));
        assert_eq!(e.get(RESPONSE_HEADERS, "x-r"), Ok("r".into()));
        assert_eq!(e.get(RESPONSE_HEADERS, "x-none"), Err(NOT_FOUND));
        let set = "proxy_replace_header_map_value";
        assert_eq!(
            e.change(set, RESPONSE_HEADERS, ":status", "500"),
            BAD_ARGUMENT
        );
        assert_eq!(e.change(set, RESPONSE_HEADERS, "x-s", "1"), OK);
        assert_eq!(e.stream().head.as_ref().unwrap().get("X-S"), Some("1"));
        assert_eq!(e.property("response\0code"), Ok("201".into()));
        let p = e.pairs(RESPONSE_HEADERS).unwrap();
        assert!(p.contains(&(":status".into(), "201".into())), "{p:?}");
        assert!(p.contains(&("x-s".into(), "1".into())), "{p:?}");
        assert_eq!(
            e.set_pairs(RESPONSE_HEADERS, &[(":status", "500"), ("x-t", "2")]),
            OK
        );
        let head = e.stream().head.clone().unwrap();
        assert_eq!((head.len(), head.get("X-T")), (1, Some("2")));
        assert_eq!(e.stream().status, 201);

        // Once the head has gone only its status is left, which cannot change.
        e.stream().head = None;
        assert_eq!(e.pairs(RESPONSE_HEADERS), Ok(pairs(&[(":status", "201")])));
        assert_eq!(
            e.change("proxy_add_header_map_value", RESPONSE_HEADERS, "x-u", "3"),
            BAD_ARGUMENT
        );
        assert_eq!(e.set_pairs(RESPONSE_HEADERS, &[("x-u", "3")]), BAD_ARGUMENT);

        // The trailer is there only at the end of the body.
        assert_eq!(e.change(set, RESPONSE_TRAILERS, "x-v", "4"), BAD_ARGUMENT);
        assert_eq!(
            e.set_pairs(RESPONSE_TRAILERS, &[("x-v", "4")]),
            BAD_ARGUMENT
        );
        let mut t = Headers::new();
        t.append("X-Old", "o");
        e.stream().trailers = Some(t);
        assert_eq!(e.change(set, RESPONSE_TRAILERS, ":x", "4"), BAD_ARGUMENT);
        let old = e.put("x-old");
        assert_eq!(
            e.ok(
                "proxy_remove_header_map_value",
                &[RESPONSE_TRAILERS, old[0], old[1]]
            ),
            OK
        );
        assert!(e.stream().trailers.as_ref().unwrap().is_empty());
        assert_eq!(e.set_pairs(RESPONSE_TRAILERS, &[("x-w", "5")]), OK);
        assert_eq!(e.get(RESPONSE_TRAILERS, "x-w"), Ok("5".into()));
        assert_eq!(e.pairs(RESPONSE_TRAILERS), Ok(pairs(&[("x-w", "5")])));

        // The body is the plugin's to read and change only in its callback.
        e.stream().body = b"body".to_vec();
        let one = e.put("<");
        for current in [None, Some(2)] {
            e.pw().current = current;
            let args = [RESPONSE_BODY, 0, 4, PTR, SIZE];
            assert_eq!(e.ok("proxy_get_buffer_bytes", &args), NOT_FOUND);
            let args = [RESPONSE_BODY, 0, 0, one[0], one[1]];
            assert_eq!(e.ok("proxy_set_buffer_bytes", &args), NOT_FOUND);
        }
        e.stream().in_body = true;
        let args = [RESPONSE_BODY, 5, 1, PTR, SIZE];
        assert_eq!(e.ok("proxy_get_buffer_bytes", &args), BAD_ARGUMENT);
        let args = [RESPONSE_BODY, 1, 100, PTR, SIZE];
        assert_eq!(e.ok("proxy_get_buffer_bytes", &args), OK);
        assert_eq!(e.given_str(), "ody");
        let args = [RESPONSE_BODY, 0, 0, one[0], one[1]];
        assert_eq!(e.ok("proxy_set_buffer_bytes", &args), OK);
        assert_eq!(e.stream().body, b"<body");
        let args = [PLUGIN_CONFIGURATION, 0, 0, one[0], one[1]];
        assert_eq!(e.ok("proxy_set_buffer_bytes", &args), NOT_FOUND);
        // Past max_body_bytes (8 here).
        let long = e.put("123456");
        let args = [RESPONSE_BODY, 0, 0, long[0], long[1]];
        assert_eq!(e.ok("proxy_set_buffer_bytes", &args), BAD_ARGUMENT);
        assert_eq!(e.stream().body, b"<body");
    }

    #[test]
    fn pairs_encoded() {
        let p = pairs(&[(":path", "/a"), ("x-a", "1"), ("x-b", "")]);
        assert_eq!(deserialize_pairs(&serialize_pairs(&p)), Some(p));
        for bad in [
            &[1u8, 0, 0][..],
            &[1, 0, 0, 0, 5, 0, 0, 0, 1, 0, 0, 0],
            &[0xff, 0xff, 0, 0],
            &[1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0xff, 0, 0xfe, 0],
        ] {
            assert_eq!(deserialize_pairs(bad), None, "{bad:?}");
        }
        assert_eq!(deserialize_pairs(&[]), Some(Vec::new()));
    }

    /// fit's `TestBodyBuffer`: `proxy_set_buffer_bytes`'s arithmetic, and its bound.
    #[test]
    fn body_buffer() {
        let mut s = Stream::default();
        for (start, size, data, want) in [
            (0, 0, "ab", "ab"),
            (1 << 31, 0, "cd", "abcd"),
            (0, 0, "<", "<abcd"),
            (1, 2, "x", "<xcd"),
            (0, 1 << 31, "new", "new"),
        ] {
            assert_eq!(s.set_body(start, size, data.as_bytes(), 8), OK);
            assert_eq!(s.body, want.as_bytes(), "{start} {size} {data}");
        }
        assert_eq!(s.set_body(3, 0, b"123456", 8), BAD_ARGUMENT);
        assert_eq!(s.body, b"new");
    }

    #[test]
    fn context_ids() {
        let e = Env::new();
        let x = e.store.state().pw.as_ref().unwrap().x;
        let mut pw = PwInstance::new(x);
        pw.next_id = u32::MAX;
        pw.streams.insert(2, Stream::default());
        // The id after the last wraps around, past the root's and those still open.
        assert_eq!(pw.next_id(), u32::MAX);
        assert_eq!(pw.next_id(), 3);
    }
}
