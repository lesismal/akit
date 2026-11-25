//! The `wasm` middleware's params (I71), as fit's `plugin/wasm` `WASM`, with its defaults and
//! checks. fit's own plugin ABI is read; the params only a Proxy-Wasm plugin uses (I72) are
//! refused as not supported yet.

use std::time::Duration;

use crate::decode::Decoder;
use crate::json::Node;
use crate::model::{WASM_FIELDS, WASM_FIT_ONLY, limit_params, size_field_at};

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

/// What a call that fails, traps or runs past its timeout leads to: fit's `on_error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WasmOnError {
    /// `fail` (the default): the request is answered 500.
    #[default]
    Fail,
    /// `skip`: the request goes on as if the plugin were not there.
    Skip,
}

/// The `wasm` middleware's params, as fit's `plugin/wasm` `WASM` (fit's ABI; Proxy-Wasm is I72).
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
    o.finish(d, WASM_FIELDS, WASM_FIT_ONLY);
    ok.then_some(w)
}
