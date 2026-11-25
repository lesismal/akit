//! A plugin loaded: its module decoded and validated once, shared by the configs that load the
//! same code, and the pool of instances a `wasm` middleware runs it on.

use std::io::Read as _;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use akio_wasm::{InstancePool, Linker, Module, ModuleLimits, PoolConfig};
use akit_config::Wasm;

use crate::abi::{self, EXPORT_ON_REQUEST, Host, Shared};

/// The most bytes a plugin's file may have: akio-wasm's default module limit, 64 MiB.
pub(crate) fn max_module_bytes() -> usize {
    ModuleLimits::default().max_module_bytes
}

/// How long making an instance may run: instantiating it, and the WASI reactor's
/// `_initialize`, which runs Go's runtime start (millions of instructions). fit bounds it by
/// nothing; akit bounds it, generously, so that a plugin whose start never ends fails its load.
pub(crate) const INIT_TIMEOUT: Duration = Duration::from_secs(10);

/// The exports that make a module a Proxy-Wasm filter, which I72 runs.
const PROXY_WASM_VERSIONS: [&str; 2] = ["proxy_abi_version_0_2_0", "proxy_abi_version_0_2_1"];

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

/// Checks that `m` is a plugin of fit's ABI.
pub(crate) fn check_exports(m: &Module) -> Result<(), String> {
    if m.export(EXPORT_ON_REQUEST).is_some() {
        return Ok(());
    }
    if PROXY_WASM_VERSIONS.iter().any(|v| m.export(v).is_some()) {
        return Err(
            "the plugin is a Proxy-Wasm filter, which akit does not run yet (I72)".to_owned(),
        );
    }
    Err(format!(
        "the plugin exports neither {EXPORT_ON_REQUEST} nor Proxy-Wasm's ABI version"
    ))
}

/// The linker a plugin's instances are made with: WASI (output discarded, as fit's) and fit's
/// host functions. An import neither defines fails the plugin only if it calls it: fit's Go
/// guests import fit's other host modules (sync, Proxy-Wasm) they may never call.
pub(crate) fn linker() -> Result<Linker<Host>, String> {
    let mut l = Linker::new();
    l.wasi_preview1(akio_wasm::WasiConfig::new())
        .map_err(|e| e.to_string())?
        .trap_unresolved(true);
    abi::link(&mut l).map_err(|e| e.to_string())?;
    Ok(l)
}

/// The pool's sizes and timeouts, from the params.
pub(crate) fn pool_config(w: &Wasm) -> PoolConfig {
    let mut c = PoolConfig::default();
    if w.instances > 0 {
        c.instances = w.instances;
    }
    c.queue = w.queue.max(1);
    // Calls are not sent to an instance of their choosing: its own queue goes unused.
    c.instance_queue = 1;
    c.wait_timeout = w.wait_timeout;
    c.call_timeout = w.timeout;
    c.init_timeout = INIT_TIMEOUT.max(w.timeout);
    c.limits.max_memory = usize::try_from(w.max_memory_bytes).unwrap_or(usize::MAX);
    "akit-wasm-".clone_into(&mut c.thread_name);
    c
}

/// A plugin running: its instances' pool, and what they share.
pub(crate) struct Plugin {
    pub(crate) pool: InstancePool<Host>,
    pub(crate) shared: Arc<Shared>,
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
        check_exports(&module)?;
        let shared = Arc::new(Shared::new(&w.path, &w.config));
        let s = Arc::clone(&shared);
        let pool = InstancePool::builder(module, linker()?, move |_| Host::new(Arc::clone(&s)))
            .config(pool_config(w))
            .on_start(|p| {
                let f = p.typed_func::<(), i32>(EXPORT_ON_REQUEST)?;
                p.state_mut().on_request = Some(f);
                Ok(())
            })
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Plugin { pool, shared })
    }

    /// Closes the pool on a thread of its own: closing waits for the instances' threads, and a
    /// config's cleanup may run on an I/O thread (rule 1).
    pub(crate) fn close_later(self: Arc<Plugin>) {
        let spawned = std::thread::Builder::new()
            .name("akit-wasm-close".to_owned())
            .spawn(move || self.pool.close());
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
        assert!(e.contains("exports neither fit_on_request"), "{e}");
        let pw = Wasm {
            funcs: vec![Func::export("proxy_abi_version_0_2_1", 0, 0, Vec::new())],
            ..Wasm::default()
        }
        .encode();
        let e = check_exports(&compile(&pw).unwrap()).unwrap_err();
        assert!(e.contains("Proxy-Wasm filter"), "{e}");
    }

    #[test]
    fn reading_is_bounded() {
        let e = read("/nonexistent/plugin.wasm").unwrap_err();
        assert!(e.contains("open /nonexistent/plugin.wasm"), "{e}");
        let e = compile(b"not wasm").unwrap_err();
        assert!(e.starts_with("compiling the plugin"), "{e}");
    }
}
