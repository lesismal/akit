//! A plugin loaded: its module decoded and validated once, shared by the configs that load the
//! same code, and the pool of instances a `wasm` middleware runs it on.

use std::io::Read as _;
use std::sync::{Arc, Condvar, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use akio_wasm::{InstancePool, Linker, Module, ModuleLimits, PoolConfig};
use akit_config::{Wasm, WasmOnError, WasmResponse};

use crate::abi::{self, EXPORT_ON_REQUEST, Host, Shared};
use crate::proxywasm::{
    self, ON_RESPONSE_BODY, ON_RESPONSE_HEADERS, ON_RESPONSE_TRAILERS, PwShared, REQUIRED,
};

/// The most bytes a plugin's file may have: akio-wasm's default module limit, 64 MiB.
pub(crate) fn max_module_bytes() -> usize {
    ModuleLimits::default().max_module_bytes
}

/// How long making an instance may run: instantiating it, and the WASI reactor's
/// `_initialize`, which runs Go's runtime start (millions of instructions). fit bounds it by
/// nothing; akit bounds it, generously, so that a plugin whose start never ends fails its load.
pub(crate) const INIT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a config's plugin, released, keeps running for the responses it still follows
/// before its pool closes (fit holds the config until they end; akit bounds it, rule 11).
pub(crate) const FOLLOW_GRACE: Duration = Duration::from_secs(600);

/// The modules loaded and still in use, by their code, so that a config reloaded with the same
/// plugin does not decode and validate it again (fit's `plugins`). Touched only when a config is
/// compiled; an entry whose module is gone is dropped then.
static LOADED: Mutex<Vec<Loaded>> = Mutex::new(Vec::new());

/// A plugin's code, and its module while some config uses it.
type Loaded = (Arc<[u8]>, Weak<Module>);

/// Reads the plugin at `path`, at most [`max_module_bytes`]: off the I/O threads, as the config
/// is compiled.
pub(crate) fn read(path: &str) -> Result<Vec<u8>, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("open {path}: {e}"))?;
    let max = max_module_bytes();
    let mut code = Vec::new();
    f.take(u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut code)
        .map_err(|e| format!("read {path}: {e}"))?;
    if code.len() > max {
        return Err(format!("{path}: larger than {max} bytes"));
    }
    Ok(code)
}

/// `code` decoded and validated, or the module an earlier config loaded from the same code.
pub(crate) fn compile(code: &[u8]) -> Result<Arc<Module>, String> {
    let mut loaded = LOADED.lock().unwrap_or_else(PoisonError::into_inner);
    loaded.retain(|(_, m)| m.strong_count() > 0);
    if let Some(m) = loaded
        .iter()
        .find(|(c, _)| **c == *code)
        .and_then(|(_, m)| m.upgrade())
    {
        return Ok(m);
    }
    let m = Arc::new(
        Module::new(code, &ModuleLimits::default())
            .map_err(|e| format!("compiling the plugin: {e}"))?,
    );
    loaded.push((Arc::from(code), Arc::downgrade(&m)));
    Ok(m)
}

/// Which ABI a plugin speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Abi {
    /// akit's own, fit's renamed: `akit_on_request`.
    Akit,
    /// Proxy-Wasm's: it exports `proxy_abi_version_0_2_0` or `_0_2_1`, as fit tells them apart.
    ProxyWasm,
}

/// Which ABI `m` speaks: a Proxy-Wasm filter, checked to export the functions the host calls, or
/// a plugin of akit's ABI.
pub(crate) fn check_exports(m: &Module) -> Result<Abi, String> {
    if proxywasm::VERSIONS.iter().any(|v| m.export(v).is_some()) {
        if let Some(name) = REQUIRED.iter().find(|n| m.export(n).is_none()) {
            return Err(format!("the Proxy-Wasm plugin does not export {name}"));
        }
        return Ok(Abi::ProxyWasm);
    }
    if m.export(EXPORT_ON_REQUEST).is_some() {
        return Ok(Abi::Akit);
    }
    Err(format!(
        "the plugin exports neither {EXPORT_ON_REQUEST} nor Proxy-Wasm's ABI version"
    ))
}

/// The linker a plugin's instances are made with: WASI (output discarded, as fit's), fit's host
/// functions and Proxy-Wasm's. An import none defines fails the plugin only if it calls it:
/// fit's Go guests import fit's other host modules (sync) they may never call.
pub(crate) fn linker() -> Result<Linker<Host>, String> {
    let mut l = Linker::new();
    l.wasi_preview1(akio_wasm::WasiConfig::new())
        .map_err(|e| e.to_string())?
        .trap_unresolved(true);
    abi::link(&mut l).map_err(|e| e.to_string())?;
    proxywasm::link(&mut l).map_err(|e| e.to_string())?;
    Ok(l)
}

/// How a Proxy-Wasm plugin follows responses, from its exports and params.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PwPlugin {
    /// Its responses' heads are held for `proxy_on_response_headers`.
    pub(crate) head: bool,
    /// Its responses' bodies go through `proxy_on_response_body` and
    /// `proxy_on_response_trailers` (`response: all`).
    pub(crate) body: bool,
    /// How long a head may be held: the call's wait and its run, and a second more, past which
    /// akio cuts the response (the pool answers before).
    pub(crate) hold_timeout: Duration,
    /// `max_body_bytes`.
    pub(crate) max_body_bytes: u64,
}

impl PwPlugin {
    fn new(m: &Module, w: &Wasm) -> PwPlugin {
        let exports = |n: &str| m.export(n).is_some();
        PwPlugin {
            head: w.response != WasmResponse::None && exports(ON_RESPONSE_HEADERS),
            body: w.response == WasmResponse::All
                && (exports(ON_RESPONSE_BODY) || exports(ON_RESPONSE_TRAILERS)),
            hold_timeout: w
                .wait_timeout
                .saturating_add(w.timeout)
                .saturating_add(Duration::from_secs(1)),
            max_body_bytes: w.max_body_bytes,
        }
    }

    /// Whether the plugin follows the responses of the requests it lets go on.
    pub(crate) fn follows(&self) -> bool {
        self.head || self.body
    }
}

/// The pool's sizes and timeouts, from the params.
pub(crate) fn pool_config(w: &Wasm) -> PoolConfig {
    let mut c = PoolConfig::default();
    if w.instances > 0 {
        c.instances = w.instances;
    }
    c.queue = w.queue.max(1);
    // A request's calls go to any instance; a followed response's to the instance that has its
    // context, in that instance's queue: at most a call under way and its context's closing for
    // each of the `max_streams` it follows (fit's queue holds four for each).
    c.instance_queue = w.max_streams.max(1).saturating_mul(4);
    c.wait_timeout = w.wait_timeout;
    c.call_timeout = w.timeout;
    c.init_timeout = INIT_TIMEOUT.max(w.timeout);
    c.limits.max_memory = usize::try_from(w.max_memory_bytes).unwrap_or(usize::MAX);
    "akit-wasm-".clone_into(&mut c.thread_name);
    c
}

/// A plugin running: its instances' pool, what they share, and, for a Proxy-Wasm plugin, how it
/// follows responses and how many it follows now.
pub(crate) struct Plugin {
    pub(crate) pool: InstancePool<Host>,
    pub(crate) shared: Arc<Shared>,
    pub(crate) pw: Option<PwPlugin>,
    pub(crate) on_error: WasmOnError,
    follows: Mutex<usize>,
    unfollowed: Condvar,
}

impl std::fmt::Debug for Plugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plugin")
            .field("path", &self.shared.path)
            .field("pool", &self.pool)
            .finish()
    }
}

impl Plugin {
    /// Loads the plugin `w` names and makes its instances, each initialized: blocks for as long
    /// as that takes, off the I/O threads, as the config is compiled.
    pub(crate) fn start(w: &Wasm) -> Result<Plugin, String> {
        let code = read(&w.path)?;
        Plugin::start_code(w, &code)
    }

    /// [`start`](Self::start) with the plugin's code read already.
    pub(crate) fn start_code(w: &Wasm, code: &[u8]) -> Result<Plugin, String> {
        let module = compile(code)?;
        let kind = check_exports(&module)?;
        let pw = (kind == Abi::ProxyWasm).then(|| PwPlugin::new(&module, w));
        let mut shared = Shared::new(&w.path, &w.config);
        shared.pw = PwShared {
            vm_config: w.vm_config.clone(),
            properties: w
                .properties
                .iter()
                .map(|p| (p.path.join("\0"), p.value.clone()))
                .collect(),
            max_body_bytes: w.max_body_bytes,
            max_streams: w.max_streams,
        };
        let shared = Arc::new(shared);
        let s = Arc::clone(&shared);
        let pool = InstancePool::builder(module, linker()?, move |_| Host::new(Arc::clone(&s)))
            .config(pool_config(w))
            .on_start(move |p| match kind {
                Abi::Akit => {
                    let f = p.typed_func::<(), i32>(EXPORT_ON_REQUEST)?;
                    p.state_mut().on_request = Some(f);
                    Ok(())
                }
                Abi::ProxyWasm => proxywasm::start(p),
            })
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Plugin {
            pool,
            shared,
            pw,
            on_error: w.on_error,
            follows: Mutex::new(0),
            unfollowed: Condvar::new(),
        })
    }

    /// Counts a response the plugin follows (`true`), or one it no longer does.
    pub(crate) fn followed(&self, more: bool) {
        let mut n = self.follows.lock().unwrap_or_else(PoisonError::into_inner);
        *n = if more {
            n.saturating_add(1)
        } else {
            n.saturating_sub(1)
        };
        if *n == 0 {
            self.unfollowed.notify_all();
        }
    }

    /// Waits until the plugin follows no response, or `grace` passes.
    fn wait_unfollowed(&self, grace: Duration) {
        let deadline = Instant::now().checked_add(grace);
        let mut n = self.follows.lock().unwrap_or_else(PoisonError::into_inner);
        while *n > 0 {
            let left = deadline.map_or(grace, |d| d.saturating_duration_since(Instant::now()));
            if left.is_zero() {
                return;
            }
            n = self
                .unfollowed
                .wait_timeout(n, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Closes the pool on a thread of its own, once the responses the plugin follows have ended
    /// (at most [`FOLLOW_GRACE`] later): closing waits for the instances' threads, and a config's
    /// cleanup may run on an I/O thread (rule 1).
    pub(crate) fn close_later(self: Arc<Plugin>) {
        let spawned = std::thread::Builder::new()
            .name("akit-wasm-close".to_owned())
            .spawn(move || {
                self.wait_unfollowed(FOLLOW_GRACE);
                self.pool.close();
            });
        // Without a thread, the pool closes when its last holder drops it.
        drop(spawned);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::encode::{Func, Module as Wasm};

    #[test]
    fn the_same_code_is_compiled_once() {
        let code = Wasm {
            funcs: vec![Func::export(
                EXPORT_ON_REQUEST,
                0,
                1,
                crate::encode::i32_const(0),
            )],
            ..Wasm::default()
        }
        .encode();
        let a = compile(&code).unwrap();
        let b = compile(&code).unwrap();
        assert!(Arc::ptr_eq(&a, &b), "the plugin was compiled twice");
        drop((a, b));
        // Gone with its last holder: compiled afresh.
        let c = compile(&code).unwrap();
        assert_eq!(Arc::strong_count(&c), 1);
    }

    #[test]
    fn exports_are_checked() {
        let none = Wasm::default().encode();
        let e = check_exports(&compile(&none).unwrap()).unwrap_err();
        assert!(e.contains("exports neither akit_on_request"), "{e}");
        let pw = Wasm {
            funcs: vec![Func::export("proxy_abi_version_0_2_1", 0, 0, Vec::new())],
            ..Wasm::default()
        }
        .encode();
        let e = check_exports(&compile(&pw).unwrap()).unwrap_err();
        assert_eq!(
            e,
            "the Proxy-Wasm plugin does not export proxy_on_context_create"
        );
    }

    #[test]
    fn reading_is_bounded() {
        let e = read("/nonexistent/plugin.wasm").unwrap_err();
        assert!(e.contains("open /nonexistent/plugin.wasm"), "{e}");
        let e = compile(b"not wasm").unwrap_err();
        assert!(e.starts_with("compiling the plugin"), "{e}");
    }
}
