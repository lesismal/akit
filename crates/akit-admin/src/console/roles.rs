//! The roles' own consoles (I109, fit's `app/roleconsole`): a role's `console` serves the
//! workbench's pages behind its logins, as `admin.console` does, with the role's module at
//! `/api/` and at `/api/m/<id>/`. Each is moved from config to config with its role's member:
//! started anew when its settings change, its access (the console's, then the global one) set by
//! every config applied, whether or not it starts anew; one that does not start leaves the one
//! before running.

use std::collections::BTreeMap;
use std::sync::Arc;

use akit_observe::logger::{Level, Logger};
use akit_roles::pages::{Consoles, ModuleRef, console_of};

use super::server::Console;
use crate::server::Options;

/// A role's console that runs: its settings but the access, its module and its access.
struct Running {
    cfg: akit_config::Console,
    module: ModuleRef,
    access: Option<Arc<akit_access::Policy>>,
    console: Console,
}

/// The roles' consoles, by role: what the roles' manager moves with the roles
/// ([`akit_roles::host::Manager::set_consoles`]).
pub struct RoleConsoles {
    opts: Options,
    running: BTreeMap<&'static str, Running>,
}

impl std::fmt::Debug for RoleConsoles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoleConsoles")
            .field("running", &self.running.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// The policy of a console's own `access`, then the config's global one.
fn policy(
    own: Option<&akit_config::Access>,
    global: Option<&akit_config::Access>,
) -> Result<Option<Arc<akit_access::Policy>>, String> {
    let err = |es: Vec<akit_access::Error>| {
        es.iter()
            .map(|e| format!("access.{}: {}", e.field, e.msg))
            .collect::<Vec<_>>()
            .join("; ")
    };
    let own = akit_access::compile(own, None).map_err(err)?;
    let global = akit_access::compile(global, None).map_err(err)?;
    Ok(akit_access::Policy::chain([own.as_ref(), global.as_ref()]))
}

fn log(level: Level, args: std::fmt::Arguments<'_>) {
    akit_observe::logger::log(Logger::Admin, level, args);
}

impl RoleConsoles {
    /// No console yet; each is served with `opts`' backend and timeouts.
    #[must_use]
    pub fn new(opts: &Options) -> RoleConsoles {
        RoleConsoles {
            opts: opts.clone(),
            running: BTreeMap::new(),
        }
    }

    /// Where role `id`'s console listens, as a URL, when it runs.
    #[must_use]
    pub fn url(&self, id: &str) -> Option<String> {
        self.running.get(id).map(|r| r.console.url())
    }

    /// Where role `id`'s console listens, when it runs.
    #[must_use]
    pub fn addr(&self, id: &str) -> Option<std::net::SocketAddr> {
        self.running.get(id).map(|r| r.console.addr())
    }

    fn start(
        &self,
        cfg: &akit_config::Console,
        module: &ModuleRef,
        access: Option<&Arc<akit_access::Policy>>,
    ) -> std::io::Result<Console> {
        let mut opts = self.opts.clone();
        opts.console_access = access.cloned();
        Console::start_role(cfg, Arc::clone(module), &opts)
    }

    /// Stops every console.
    pub fn stop(&mut self) {
        for (_, r) in std::mem::take(&mut self.running) {
            r.console.shutdown();
        }
    }
}

impl Drop for RoleConsoles {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Consoles for RoleConsoles {
    fn apply(
        &mut self,
        id: &'static str,
        config: Option<&akit_config::Config>,
        module: Option<ModuleRef>,
    ) -> Result<(), String> {
        let want = config.and_then(|c| console_of(c, id).map(|cc| (c, cc)));
        let (Some((config, cfg)), Some(module)) = (want, module) else {
            if let Some(r) = self.running.remove(id) {
                r.console.shutdown();
            }
            return Ok(());
        };
        let access = policy(cfg.access.as_ref(), config.access.as_ref())?;
        let key = akit_config::Console {
            access: None,
            ..cfg.clone()
        };
        if let Some(r) = self.running.get_mut(id) {
            if r.cfg == key && Arc::ptr_eq(&r.module, &module) {
                r.console.set_access(access.as_ref());
                r.access = access;
                return Ok(());
            }
        }
        // The new one may listen where the old one does.
        let old = self.running.remove(id);
        if let Some(o) = &old {
            o.console.shutdown();
        }
        match self.start(&key, &module, access.as_ref()) {
            Ok(console) => {
                log(
                    Level::Info,
                    format_args!("{id}: console on {}", console.url()),
                );
                self.running.insert(
                    id,
                    Running {
                        cfg: key,
                        module,
                        access,
                        console,
                    },
                );
                Ok(())
            }
            Err(e) => {
                if let Some(o) = old {
                    match self.start(&o.cfg, &o.module, o.access.as_ref()) {
                        Ok(console) => {
                            self.running.insert(id, Running { console, ..o });
                        }
                        Err(e2) => log(
                            Level::Warn,
                            format_args!("{id}: the console before could not start again: {e2}"),
                        ),
                    }
                }
                Err(e.to_string())
            }
        }
    }
}
