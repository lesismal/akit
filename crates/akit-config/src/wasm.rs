//! The `wasm` middleware's params (I71, I72), as fit's `plugin/wasm` `WASM`, with its defaults
//! and checks: those of fit's own plugin ABI, and those only a Proxy-Wasm plugin uses
//! (`vm_config`, `properties`, `response`, `max_streams`, `max_body_bytes`).

use std::time::Duration;

use crate::decode::Decoder;
use crate::json::Node;
use crate::model::{WASM_FIELDS, WASM_PROPERTY_FIELDS, limit_params, size_field_at};

/// fit's default `queue` of a `wasm` middleware (`DefaultQueue`).
pub const DEFAULT_WASM_QUEUE: usize = 1024;
/// fit's default `timeout` of a `wasm` middleware: how long a call may run.
pub const DEFAULT_WASM_TIMEOUT: Duration = Duration::from_millis(100);
/// fit's default `wait_timeout` of a `wasm` middleware: how long a request may wait for an
/// instance.
pub const DEFAULT_WASM_WAIT_TIMEOUT: Duration = Duration::from_secs(1);
/// fit's default `max_memory_bytes` of a `wasm` middleware: 128 MiB.
pub const DEFAULT_WASM_MAX_MEMORY_BYTES: u64 = 128 << 20;
/// The most memory an instance may be given: WebAssembly's 32-bit memories hold 4 GiB, which fit
/// caps the pages at (65536).
pub const MAX_WASM_MEMORY_BYTES: u64 = 4 << 30;
/// The most `instances` a `wasm` middleware may run: each is a thread with its own memory. fit
/// sets no bound; this keeps a config's threads and memory bounded (rule 11).
pub const MAX_WASM_INSTANCES: usize = 1024;
/// The most `queue` a `wasm` middleware may have (rule 11): fit sets no bound.
pub const MAX_WASM_QUEUE: usize = 1 << 20;
/// fit's default `max_streams` of a `wasm` middleware (`DefaultMaxStreams`): the responses an
/// instance of a Proxy-Wasm plugin follows at once.
pub const DEFAULT_WASM_MAX_STREAMS: usize = 1024;
/// The most `max_streams` a `wasm` middleware may have (rule 11): fit sets no bound. Each stream
/// keeps a context on its instance, and its calls a place in the instance's queue.
pub const MAX_WASM_MAX_STREAMS: usize = 1 << 16;
/// fit's default `max_body_bytes` of a `wasm` middleware (`DefaultMaxBodyBytes`): 1 MiB of a
/// response's body a Proxy-Wasm plugin may hold.
pub const DEFAULT_WASM_MAX_BODY_BYTES: u64 = 1 << 20;
/// The most `max_body_bytes` a `wasm` middleware may have: the body is handed into the plugin's
/// memory, which holds at most 4 GiB (rule 11; fit sets no bound).
pub const MAX_WASM_MAX_BODY_BYTES: u64 = MAX_WASM_MEMORY_BYTES;

/// What a call that fails, traps or runs past its timeout leads to: fit's `on_error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WasmOnError {
    /// `fail` (the default): the request is answered 500.
    #[default]
    Fail,
    /// `skip`: the request goes on as if the plugin were not there.
    Skip,
}

/// Which of a Proxy-Wasm plugin's response callbacks it is given: fit's `response`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WasmResponse {
    /// `all` (the default): the header, the body and the trailer.
    #[default]
    All,
    /// `headers`: the header alone, the body going by untouched.
    Headers,
    /// `none`: none of them, the plugin forgetting each request once it has gone on.
    None,
}

/// A property a Proxy-Wasm plugin reads (`proxy_get_property`): fit's `Property`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmProperty {
    /// `path` (required): the property's path, by segment.
    pub path: Vec<String>,
    /// `value`: its value.
    pub value: String,
}

/// The `wasm` middleware's params, as fit's `plugin/wasm` `WASM`: fit's ABI and Proxy-Wasm's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wasm {
    /// `path` (required): the plugin, a WebAssembly module that exports `fit_on_request`, as a
    /// WASI reactor; read when the config is compiled.
    pub path: String,
    /// `config`: a string the plugin reads, for settings of its own.
    pub config: String,
    /// `instances`: how many instances of the plugin run calls at once, each with its own thread
    /// and memory; 0 (left out) for the CPUs akit has, at most 16. At most
    /// [`MAX_WASM_INSTANCES`].
    pub instances: usize,
    /// `queue`: how many requests may wait for an instance; past it a request is answered 503.
    /// [`DEFAULT_WASM_QUEUE`] when left out or 0.
    pub queue: usize,
    /// `timeout`: how long a call may run; past it the call is stopped and its instance made
    /// again, and the request goes as a failure does (`on_error`). [`DEFAULT_WASM_TIMEOUT`] when
    /// left out or 0.
    pub timeout: Duration,
    /// `wait_timeout`: how long a request may wait for an instance; past it it is answered 503.
    /// [`DEFAULT_WASM_WAIT_TIMEOUT`] when left out or 0.
    pub wait_timeout: Duration,
    /// `max_memory_bytes` (once `max_memory`): the most memory an instance may have; one that
    /// asks for more fails its call. [`DEFAULT_WASM_MAX_MEMORY_BYTES`] when left out or 0, at
    /// most [`MAX_WASM_MEMORY_BYTES`].
    pub max_memory_bytes: u64,
    /// `on_error`: what a failed call leads to.
    pub on_error: WasmOnError,
    /// `vm_config`: for a Proxy-Wasm plugin, its VM configuration, which it reads as it starts;
    /// `config` is its plugin configuration.
    pub vm_config: String,
    /// `properties`: for a Proxy-Wasm plugin, properties it reads by their path, beside the
    /// request's own.
    pub properties: Vec<WasmProperty>,
    /// `response`: for a Proxy-Wasm plugin that exports response callbacks, which it is given.
    pub response: WasmResponse,
    /// `max_streams`: for a Proxy-Wasm plugin that follows responses, how many an instance
    /// follows at once; past it a request is answered 503. [`DEFAULT_WASM_MAX_STREAMS`] when left
    /// out or 0, at most [`MAX_WASM_MAX_STREAMS`].
    pub max_streams: usize,
    /// `max_body_bytes`: for a Proxy-Wasm plugin given the response's body, how much of it the
    /// plugin may hold or make; past it the response fails, or goes on without the plugin
    /// (`on_error: skip`). [`DEFAULT_WASM_MAX_BODY_BYTES`] when left out or 0, at most
    /// [`MAX_WASM_MAX_BODY_BYTES`].
    pub max_body_bytes: u64,
}

/// The `wasm` middleware's params, checked as fit's `WASM.Provision` and its schema check them.
pub(crate) fn wasm(
    d: &mut Decoder<'_>,
    params: Option<&Node>,
    off: usize,
    path: &str,
) -> Option<Wasm> {
    let mut o = limit_params(d, params, off, path, "path")?;
    let mut ok = true;
    let mut w = Wasm {
        path: String::new(),
        config: String::new(),
        instances: 0,
        queue: DEFAULT_WASM_QUEUE,
        timeout: DEFAULT_WASM_TIMEOUT,
        wait_timeout: DEFAULT_WASM_WAIT_TIMEOUT,
        max_memory_bytes: DEFAULT_WASM_MAX_MEMORY_BYTES,
        on_error: WasmOnError::Fail,
        vm_config: String::new(),
        properties: Vec::new(),
        response: WasmResponse::All,
        max_streams: DEFAULT_WASM_MAX_STREAMS,
        max_body_bytes: DEFAULT_WASM_MAX_BODY_BYTES,
    };
    match o.require(d, "path") {
        Some(v) => {
            let at = o.at("path");
            match d.string(v, &at) {
                Some(p) if p.is_empty() => {
                    d.error(v.off, &at, "the plugin's path is empty");
                    ok = false;
                }
                Some(p) => w.path = p,
                None => ok = false,
            }
        }
        None => ok = false,
    }
    if let Some(v) = o.take("config") {
        match d.string(v, &o.at("config")) {
            Some(c) => w.config = c,
            None => ok = false,
        }
    }
    let max_instances = i64::try_from(MAX_WASM_INSTANCES).unwrap_or(i64::MAX);
    if let Some(v) = o.take("instances") {
        match d
            .int(v, &o.at("instances"), 0, max_instances)
            .and_then(|n| usize::try_from(n).ok())
        {
            Some(n) => w.instances = n,
            None => ok = false,
        }
    }
    let max_queue = i64::try_from(MAX_WASM_QUEUE).unwrap_or(i64::MAX);
    if let Some(v) = o.take("queue") {
        match d
            .int(v, &o.at("queue"), 0, max_queue)
            .and_then(|n| usize::try_from(n).ok())
        {
            Some(0) => {}
            Some(n) => w.queue = n,
            None => ok = false,
        }
    }
    for (key, slot) in [
        ("timeout", &mut w.timeout),
        ("wait_timeout", &mut w.wait_timeout),
    ] {
        if let Some(v) = o.take(key) {
            match d.duration(v, &o.at(key)) {
                Some(t) if !t.is_zero() => *slot = t,
                Some(_) => {}
                None => ok = false,
            }
        }
    }
    if let Some((m, at_off, at)) =
        size_field_at(d, &mut o, "max_memory_bytes", "max_memory", &mut ok)
    {
        if m > MAX_WASM_MEMORY_BYTES {
            d.error(
                at_off,
                &at,
                format!("is more than 4 GiB ({MAX_WASM_MEMORY_BYTES})"),
            );
            ok = false;
        } else {
            w.max_memory_bytes = m;
        }
    }
    if let Some(v) = o.take("on_error") {
        let at = o.at("on_error");
        match d.string(v, &at).as_deref() {
            Some("fail" | "") => w.on_error = WasmOnError::Fail,
            Some("skip") => w.on_error = WasmOnError::Skip,
            Some(other) => {
                d.error(
                    v.off,
                    &at,
                    format!("{other:?} is not one of \"fail\", \"skip\""),
                );
                ok = false;
            }
            None => ok = false,
        }
    }
    if let Some(v) = o.take("vm_config") {
        match d.string(v, &o.at("vm_config")) {
            Some(c) => w.vm_config = c,
            None => ok = false,
        }
    }
    if let Some(v) = o.take("properties") {
        let at = o.at("properties");
        let before = d.errors.len();
        w.properties = d.array(v, &at, property);
        ok &= d.errors.len() == before;
    }
    if let Some(v) = o.take("response") {
        let at = o.at("response");
        match d.string(v, &at).as_deref() {
            Some("all" | "") => w.response = WasmResponse::All,
            Some("headers") => w.response = WasmResponse::Headers,
            Some("none") => w.response = WasmResponse::None,
            Some(other) => {
                d.error(
                    v.off,
                    &at,
                    format!("{other:?} is not one of \"all\", \"headers\", \"none\""),
                );
                ok = false;
            }
            None => ok = false,
        }
    }
    let max_streams = i64::try_from(MAX_WASM_MAX_STREAMS).unwrap_or(i64::MAX);
    if let Some(v) = o.take("max_streams") {
        match d
            .int(v, &o.at("max_streams"), 0, max_streams)
            .and_then(|n| usize::try_from(n).ok())
        {
            Some(0) => {}
            Some(n) => w.max_streams = n,
            None => ok = false,
        }
    }
    let max_body = i64::try_from(MAX_WASM_MAX_BODY_BYTES).unwrap_or(i64::MAX);
    if let Some(v) = o.take("max_body_bytes") {
        match d
            .int(v, &o.at("max_body_bytes"), 0, max_body)
            .and_then(|n| u64::try_from(n).ok())
        {
            Some(0) => {}
            Some(n) => w.max_body_bytes = n,
            None => ok = false,
        }
    }
    o.finish(d, WASM_FIELDS, &[]);
    ok.then_some(w)
}

/// One of `properties`: its `path` (required) and `value`.
fn property(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<WasmProperty> {
    let mut o = d.object(n, path)?;
    let mut ok = true;
    let mut p = WasmProperty {
        path: Vec::new(),
        value: String::new(),
    };
    match o.require(d, "path") {
        Some(v) => {
            let before = d.errors.len();
            p.path = d
                .strings(v, &o.at("path"))
                .into_iter()
                .map(|(_, s)| s)
                .collect();
            ok &= d.errors.len() == before;
        }
        None => ok = false,
    }
    if let Some(v) = o.take("value") {
        match d.string(v, &o.at("value")) {
            Some(s) => p.value = s,
            None => ok = false,
        }
    }
    o.finish(d, WASM_PROPERTY_FIELDS, &[]);
    ok.then_some(p)
}
