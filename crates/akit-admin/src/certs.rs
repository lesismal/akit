//! The Admin API's and the console's certificates read again when their files change (I94), as
//! fit's `admin/tls.go`: a certificate a tool renews outside akit is served to new connections
//! without a restart, and files that do not load, as when the certificate is written and its key
//! not yet, leave the running TLS as it is until they change again.
//!
//! fit looks at the files from the handshake, on the connection's goroutine. akit cannot: a look
//! at a file blocks, and a handshake runs on an akio I/O thread (rules.md rule 1). So the files
//! are looked at on the control thread, every `-watch-certs` interval, by the certificate watcher
//! the listeners' files have (akit-tls's [`FileWatcher`]: the files settled for its debounce time,
//! their contents compared with what was loaded, a broken set not tried again until it changes),
//! and read again at once on SIGHUP. What they load is built into a new akio-tls configuration,
//! which the server's acceptor ([`Swap`]) hands to every connection accepted from then on; a
//! connection already open, or its handshake under way, keeps the one it was given.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Instant;

use akio::tls::{ServerConfig, Tls};
use akio::{Acceptor, Conn, Transport};
use akit_observe::logger::{Level, Logger};
use akit_tls::{FileWatcher, ListenerTls, Report, Sum};

/// What reading a server's certificate files again did, as [`crate::Server::watch_certs`] and
/// [`crate::Server::reload_certs`] report it; each is in the `admin` logger too.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CertReload {
    /// The files changed and loaded: `admin` (the API) or `console` serves them to new
    /// connections from now on.
    Reloaded(&'static str),
    /// The files changed and do not load, or cannot be read: why, as logged; the running
    /// certificate stays.
    Failed(String),
}

/// The files of a server's TLS, as loaded, and what to give the configuration they load to.
pub(crate) struct Reloader {
    /// `admin` or `console`: what messages start with.
    name: &'static str,
    tls: akit_config::Tls,
    swap: Arc<dyn SetConfig>,
    state: Mutex<State>,
}

struct State {
    watcher: FileWatcher,
    /// Bumped by every load, so that the watcher compares the files with the new ones.
    version: u64,
    /// The SHA-256 of each file as last loaded: certificate, key and client CAs.
    files: BTreeMap<String, Sum>,
}

impl std::fmt::Debug for Reloader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reloader")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Loads `tls` for a server of HTTP/1.1 alone: its configuration and its files' sums. Reads
/// files: control plane only.
fn load(tls: &akit_config::Tls) -> Result<(ServerConfig, BTreeMap<String, Sum>), akit_tls::Error> {
    let t = ListenerTls::load(tls, &[akit_config::Protocol::Http11])?;
    let cfg = t.server_configs(Arc::new(t.clone()))?.tcp;
    Ok((cfg, t.files().clone()))
}

impl Reloader {
    /// Loads `tls`'s files for the server `name`, and the acceptor that serves them over
    /// `inner`, the files as they are when each connection is accepted.
    ///
    /// # Errors
    ///
    /// When the files do not load, as `name: <why>`.
    pub(crate) fn start<A: Acceptor>(
        name: &'static str,
        tls: akit_config::Tls,
        inner: A,
    ) -> Result<(Reloader, Swap<A>), String> {
        let (cfg, files) = load(&tls).map_err(|e| format!("{name}: {e}"))?;
        let swap = Swap::new(cfg, inner);
        let reloader = Reloader {
            name,
            tls,
            swap: Arc::new(swap.handle()),
            state: Mutex::new(State {
                watcher: FileWatcher::new(akit_tls::DEFAULT_WATCH_DEBOUNCE),
                version: 0,
                files,
            }),
        };
        Ok((reloader, swap))
    }

    /// Looks at the files once (`-watch-certs`, every interval, on the control thread): once
    /// they have changed and stayed as they are for the watcher's debounce time, loads them.
    pub(crate) fn tick(&self, now: Instant) -> Option<CertReload> {
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let State {
            watcher,
            version,
            files,
        } = &mut *st;
        let mut done = None;
        let report = watcher.tick(now, *version, files, || match self.load() {
            Ok(f) => {
                done = Some(f);
                Ok(())
            }
            Err(e) => Err(e),
        });
        if let Some(f) = done {
            *files = f;
            *version = version.wrapping_add(1);
            return Some(self.reloaded());
        }
        match report? {
            Report::Failed(e) | Report::Unreadable(e) => Some(self.failed(&e)),
        }
    }

    /// Reads the files now (SIGHUP) and, when they changed, has new connections served with
    /// them.
    pub(crate) fn reload(&self) -> Option<CertReload> {
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        match load(&self.tls) {
            Ok((_, files)) if files == st.files => None,
            Ok((cfg, files)) => {
                self.swap.set(cfg);
                st.files = files;
                st.version = st.version.wrapping_add(1);
                Some(self.reloaded())
            }
            Err(e) => Some(self.failed(&e.to_string())),
        }
    }

    /// Loads the files and has new connections served with them; the sums of what was loaded.
    fn load(&self) -> Result<BTreeMap<String, Sum>, String> {
        let (cfg, files) = load(&self.tls).map_err(|e| e.to_string())?;
        self.swap.set(cfg);
        Ok(files)
    }

    fn reloaded(&self) -> CertReload {
        akit_observe::logger::log(
            Logger::Admin,
            Level::Info,
            format_args!("{}: certificate files changed; reloaded", self.name),
        );
        CertReload::Reloaded(self.name)
    }

    fn failed(&self, e: &str) -> CertReload {
        let msg = format!("{}: {e}; the running certificate stays", self.name);
        akit_observe::logger::log(Logger::Admin, Level::Warn, format_args!("{msg}"));
        CertReload::Failed(msg)
    }
}

/// What the control thread gives a server's acceptor a new configuration through.
trait SetConfig: Send + Sync {
    fn set(&self, cfg: ServerConfig);
}

/// An acceptor serving TLS over `A` with the configuration last set: akio-tls's [`Tls`], made
/// again from each configuration loaded, asked for the transport of every connection accepted.
/// The lock is held on the I/O thread only to make a connection's TLS state, and on the control
/// thread only to replace the configuration.
pub(crate) struct Swap<A> {
    inner: Arc<A>,
    tls: Arc<RwLock<Tls<Shared<A>>>>,
}

impl<A: Acceptor> Swap<A> {
    fn new(cfg: ServerConfig, inner: A) -> Swap<A> {
        let inner = Arc::new(inner);
        let tls = Tls::server(cfg, Shared(Arc::clone(&inner)));
        Swap {
            inner,
            tls: Arc::new(RwLock::new(tls)),
        }
    }

    fn handle(&self) -> Handle<A> {
        Handle {
            inner: Arc::clone(&self.inner),
            tls: Arc::clone(&self.tls),
        }
    }
}

impl<A: Acceptor> Acceptor for Swap<A> {
    type Handler = A::Handler;

    fn on_open(&self, c: &Conn<A::Handler>) -> A::Handler {
        self.inner.on_open(c)
    }

    fn on_shutdown(&self, c: &Conn<A::Handler>) {
        self.inner.on_shutdown(c);
    }

    fn transport(&self) -> Option<Box<dyn Transport>> {
        self.tls
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .transport()
    }
}

/// The control thread's hold on a [`Swap`].
struct Handle<A> {
    inner: Arc<A>,
    tls: Arc<RwLock<Tls<Shared<A>>>>,
}

impl<A: Acceptor> SetConfig for Handle<A> {
    fn set(&self, cfg: ServerConfig) {
        // Made before the lock is taken, so that a connection waits only for the swap.
        let tls = Tls::server(cfg, Shared(Arc::clone(&self.inner)));
        *self.tls.write().unwrap_or_else(PoisonError::into_inner) = tls;
    }
}

/// The acceptor each [`Tls`] wraps: the server's one, shared by every configuration.
struct Shared<A>(Arc<A>);

impl<A: Acceptor> Acceptor for Shared<A> {
    type Handler = A::Handler;

    fn on_open(&self, c: &Conn<A::Handler>) -> A::Handler {
        self.0.on_open(c)
    }

    fn on_shutdown(&self, c: &Conn<A::Handler>) {
        self.0.on_shutdown(c);
    }
}
