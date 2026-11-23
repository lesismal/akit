//! Reloading (I22): a new graph published and the listeners moved to its, as fit's
//! `listener.Server.Reload` moves them.
//!
//! A listener the new graph has unchanged (the same name, address, protocols and `tls`) is kept:
//! its socket stays open, and its requests go to the new graph's routes from the swap on, since a
//! [`Dispatcher`] reads the current graph per request. The others:
//!
//! - **Added**: until akio can add a TCP listener to a running engine (akio API request A1), akit
//!   listens on a socket of its own, with one accept thread (a control plane thread, not an I/O
//!   thread) that hands each connection to the engine with [`Engine::adopt_tcp`]. The socket is
//!   bound before the swap, so an address in use fails the reload with nothing changed, and
//!   accepts from the swap on.
//! - **Removed or changed**: one of these sockets is closed after the swap (its connections stay
//!   open and finish what they have); a changed one at the same address keeps its socket and
//!   serves the new settings from the swap on. A listener the engine was built with cannot be
//!   closed until A1 lands: a reload that removes or changes one is refused, and the running
//!   config stays.

use std::fmt;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use akio::tls::Tls;
use akio::{Acceptor, Conn, Engine, Transport};
use akit_config::{Listener, Protocol};
use akit_runtime::{Graph, GraphCell};

use crate::{Dispatcher, Error, http_config, resolve};

/// What an accept thread does with a connection: hands it to the engine with the listener's
/// acceptor.
type Serve = Arc<dyn Fn(TcpStream) + Send + Sync>;

/// An acceptor shared by the connections of one listener: [`Engine::adopt_tcp`] takes one per
/// connection.
struct Shared<A>(Arc<A>);

impl<A: Acceptor> Acceptor for Shared<A> {
    type Handler = A::Handler;
    fn on_open(&self, c: &Conn<A::Handler>) -> A::Handler {
        self.0.on_open(c)
    }
    fn on_shutdown(&self, c: &Conn<A::Handler>) {
        self.0.on_shutdown(c);
    }
    fn transport(&self) -> Option<Box<dyn Transport>> {
        self.0.transport()
    }
}

/// A listener as it is served now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Served {
    /// Its name.
    pub name: String,
    /// Where it listens.
    pub address: SocketAddr,
    /// Its protocols.
    pub protocols: Vec<Protocol>,
}

impl Served {
    fn new(l: &Listener, address: SocketAddr) -> Served {
        Served {
            name: l.name.clone(),
            address,
            protocols: l.protocols.clone(),
        }
    }
}

/// What a reload changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Reloaded {
    /// The listeners it opened, the changed ones included, in the new graph's order.
    pub added: Vec<Served>,
    /// The listeners it closed, the changed ones included.
    pub removed: Vec<Served>,
    /// How many it kept as they were.
    pub kept: usize,
}

/// Why a reload was refused; the running graph and listeners are as they were.
#[derive(Debug)]
pub enum ReloadError {
    /// It removes or changes a listener the engine was built with, which cannot be closed until
    /// akio can close a listener of a running engine (akio API request A1).
    Fixed {
        /// The listener's name.
        listener: String,
        /// Where it listens.
        address: SocketAddr,
    },
    /// A new listener cannot be set up (its address does not resolve or is in use, or it asks for
    /// what akit does not support yet).
    Listener(Error),
}

impl fmt::Display for ReloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReloadError::Fixed { listener, address } => write!(
                f,
                "listener {listener:?} on {address} is removed or changed, which needs a restart: \
                 akit cannot close a listener it started with yet (akio API request A1)"
            ),
            ReloadError::Listener(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ReloadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ReloadError::Fixed { .. } => None,
            ReloadError::Listener(e) => Some(e),
        }
    }
}

/// A listener the engine was built with.
struct Fixed {
    config: Listener,
    address: SocketAddr,
}

/// A listener on a socket of akit's own (the stopgap for A1).
struct Adopted {
    config: Listener,
    address: SocketAddr,
    serve: Arc<Mutex<Serve>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Adopted {
    /// Stops accepting and closes the socket; the connections it accepted stay open.
    fn close(&mut self) {
        self.stop.store(true, SeqCst);
        let Some(thread) = self.thread.take() else {
            return;
        };
        // Wake the accept thread, blocked in accept(), with a connection of our own.
        let mut to = self.address;
        if to.ip().is_unspecified() {
            to.set_ip(match to {
                SocketAddr::V4(_) => Ipv4Addr::LOCALHOST.into(),
                SocketAddr::V6(_) => Ipv6Addr::LOCALHOST.into(),
            });
        }
        if TcpStream::connect_timeout(&to, Duration::from_secs(1)).is_ok() {
            let _ = thread.join();
        }
        // Otherwise the thread is left to see `stop` at its next connection.
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The accept loop of an adopted listener: waits for the go (sent once its graph is current; a
/// dropped sender means the reload failed), then hands each connection on until stopped.
fn accept_loop(
    socket: TcpListener,
    serve: Arc<Mutex<Serve>>,
    stop: Arc<AtomicBool>,
    go: mpsc::Receiver<()>,
) {
    if go.recv().is_err() {
        return;
    }
    loop {
        let accepted = socket.accept();
        if stop.load(SeqCst) {
            return;
        }
        match accepted {
            Ok((stream, _)) => {
                let _ = stream.set_nodelay(true);
                let serve = Arc::clone(&lock(&serve));
                serve(stream);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            // Out of descriptors, say: wait rather than spin.
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// The first of `items` not `taken` for which `same` holds, now taken.
fn claim<T>(items: &[T], taken: &mut [bool], same: impl Fn(&T) -> bool) -> Option<usize> {
    let j = items
        .iter()
        .zip(taken.iter())
        .position(|(t, k)| !*k && same(t))?;
    if let Some(k) = taken.get_mut(j) {
        *k = true;
    }
    Some(j)
}

/// How the connections of listener `l` (TLS loaded as `tls`) are served on `engine`.
fn serve_for(
    engine: &Arc<Engine>,
    cell: &GraphCell,
    l: &Listener,
    tls: Option<akio::tls::ServerConfig>,
) -> Result<Serve, Error> {
    let http = akio_http::serve_with(
        http_config(l)?,
        Dispatcher::new(cell.clone(), &l.name).tls(l.tls.is_some()),
    );
    let on_error = |_: io::Error| {};
    Ok(match (&l.tls, tls) {
        (None, _) => {
            let a = Arc::new(http);
            let e = Arc::clone(engine);
            Arc::new(move |s| e.adopt_tcp(s, Shared(Arc::clone(&a)), on_error))
        }
        (Some(_), Some(cfg)) => {
            let a = Arc::new(Tls::server(cfg, http));
            let e = Arc::clone(engine);
            Arc::new(move |s| e.adopt_tcp(s, Shared(Arc::clone(&a)), on_error))
        }
        (Some(_), None) => {
            return Err(Error::Engine(io::Error::other(format!(
                "listener {}: its tls was not loaded",
                l.name
            ))));
        }
    })
}

/// The listeners of a running engine, moved to each graph [`reload`](Listeners::reload)
/// publishes. Call [`close`](Listeners::close) before shutting the engine down.
pub struct Listeners {
    engine: Arc<Engine>,
    cell: GraphCell,
    fixed: Vec<Fixed>,
    adopted: Vec<Adopted>,
}

impl fmt::Debug for Listeners {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Listeners")
            .field("served", &self.served())
            .finish_non_exhaustive()
    }
}

impl Listeners {
    /// The listeners of `engine`, built by [`engine`](crate::engine) from `cell`'s current graph.
    #[must_use]
    pub fn new(engine: Arc<Engine>, cell: GraphCell) -> Listeners {
        let configs = cell.with(|g| g.listeners().to_vec());
        // Through `addrs`: a listener serving h3 has a UDP listener of its own in the engine (I10).
        let fixed = crate::addrs(&configs, &engine)
            .into_iter()
            .zip(configs)
            .filter_map(|(a, config)| a.addr().map(|address| Fixed { config, address }))
            .collect();
        Listeners {
            engine,
            cell,
            fixed,
            adopted: Vec::new(),
        }
    }

    /// The listeners served now: those the engine was built with, then those added since.
    #[must_use]
    pub fn served(&self) -> Vec<Served> {
        self.fixed
            .iter()
            .map(|f| Served::new(&f.config, f.address))
            .chain(
                self.adopted
                    .iter()
                    .map(|a| Served::new(&a.config, a.address)),
            )
            .collect()
    }

    /// Publishes `graph` in the cell and moves the listeners to its: the unchanged ones kept,
    /// the new ones opened before the swap and accepting from it on, the removed ones closed
    /// after it. Runs on the control plane (it may resolve names and bind sockets). The cell's
    /// old graph is left retiring: the caller [`collect`](GraphCell::collect)s it.
    ///
    /// # Errors
    ///
    /// When a new listener cannot be set up, or a listener the engine was built with is removed
    /// or changed; nothing changes then, and `graph` is dropped (its modules cleaned up).
    pub fn reload(&mut self, graph: Graph) -> Result<Reloaded, ReloadError> {
        let configs = graph.listeners().to_vec();
        let mut fixed_kept = vec![false; self.fixed.len()];
        let mut adopted_kept = vec![false; self.adopted.len()];
        // The new graph's listeners not kept, by index.
        let mut fresh = Vec::new();
        for (i, l) in configs.iter().enumerate() {
            if claim(&self.fixed, &mut fixed_kept, |f| f.config == *l).is_none()
                && claim(&self.adopted, &mut adopted_kept, |a| a.config == *l).is_none()
            {
                fresh.push((i, l));
            }
        }
        // An adopted listener is a TCP socket of akit's own: HTTP/3 needs a UDP listener in the
        // engine, which waits for akio API request A1 as removing one does.
        if let Some((_, l)) = fresh
            .iter()
            .find(|(_, l)| l.protocols.contains(&Protocol::H3))
        {
            return Err(ReloadError::Listener(Error::NotSupportedYet {
                listener: l.name.clone(),
                what: "a listener serving h3 added by a reload (akio API request A1)".into(),
            }));
        }
        if let Some(f) = self.fixed.iter().zip(&fixed_kept).find(|(_, k)| !**k) {
            return Err(ReloadError::Fixed {
                listener: f.0.config.name.clone(),
                address: f.0.address,
            });
        }

        // Before the swap: the new sockets bound, their accept threads waiting for the go; a
        // changed listener keeps the socket of a removed one at its address.
        struct Opened<'a> {
            config: &'a Listener,
            serve: Serve,
            /// The adopted listener whose socket it takes over, or the new socket's state.
            target: Target,
        }
        enum Target {
            Reuse(usize),
            New {
                adopted: Box<Adopted>,
                go: mpsc::Sender<()>,
            },
        }
        let mut opened: Vec<Opened<'_>> = Vec::new();
        let fail = |opened: Vec<Opened<'_>>, e: Error| {
            for o in opened {
                if let Target::New { adopted, go } = o.target {
                    // The accept thread sees the sender dropped and exits without accepting.
                    drop(go);
                    if let Some(t) = adopted.thread {
                        let _ = t.join();
                    }
                }
            }
            Err(ReloadError::Listener(e))
        };
        // Adopted listeners whose sockets are taken over: those kept, and the reused ones.
        let mut reused = adopted_kept.clone();
        for (i, l) in fresh {
            let serve = match serve_for(&self.engine, &self.cell, l, graph.tls(i).cloned()) {
                Ok(s) => s,
                Err(e) => return fail(opened, e),
            };
            let address = match resolve(&l.address) {
                Ok(a) => a,
                Err(error) => {
                    return fail(
                        opened,
                        Error::Address {
                            listener: l.name.clone(),
                            address: l.address.clone(),
                            error,
                        },
                    );
                }
            };
            if let Some(j) = claim(&self.adopted, &mut reused, |a| {
                a.config.address == l.address || a.address == address
            }) {
                opened.push(Opened {
                    config: l,
                    serve,
                    target: Target::Reuse(j),
                });
                continue;
            }
            let bound = TcpListener::bind(address).and_then(|s| s.local_addr().map(|a| (s, a)));
            let (socket, address) = match bound {
                Ok(b) => b,
                Err(error) => {
                    return fail(
                        opened,
                        Error::Address {
                            listener: l.name.clone(),
                            address: l.address.clone(),
                            error,
                        },
                    );
                }
            };
            let shared = Arc::new(Mutex::new(Arc::clone(&serve)));
            let stop = Arc::new(AtomicBool::new(false));
            let (go, wait) = mpsc::channel();
            let thread = {
                let (shared, stop) = (Arc::clone(&shared), Arc::clone(&stop));
                std::thread::Builder::new()
                    .name(format!("akit-accept-{}", l.name))
                    .spawn(move || accept_loop(socket, shared, stop, wait))
            };
            let thread = match thread {
                Ok(t) => t,
                Err(e) => return fail(opened, Error::Engine(e)),
            };
            opened.push(Opened {
                config: l,
                serve,
                target: Target::New {
                    adopted: Box::new(Adopted {
                        config: l.clone(),
                        address,
                        serve: shared,
                        stop,
                        thread: Some(thread),
                    }),
                    go,
                },
            });
        }

        // The swap: requests go to the new graph from here on.
        self.cell.swap(graph);

        let mut report = Reloaded {
            kept: fixed_kept
                .len()
                .saturating_add(adopted_kept.iter().filter(|k| **k).count()),
            ..Reloaded::default()
        };
        let mut old: Vec<Option<Adopted>> = self.adopted.drain(..).map(Some).collect();
        let mut next: Vec<Adopted> = Vec::new();
        for (slot, kept) in old.iter_mut().zip(&adopted_kept) {
            if *kept {
                next.extend(slot.take());
            }
        }
        for o in opened {
            let l = o.config;
            match o.target {
                Target::Reuse(j) => {
                    if let Some(mut a) = old.get_mut(j).and_then(Option::take) {
                        report.removed.push(Served::new(&a.config, a.address));
                        *lock(&a.serve) = o.serve;
                        a.config = l.clone();
                        report.added.push(Served::new(l, a.address));
                        next.push(a);
                    }
                }
                Target::New { adopted, go } => {
                    let _ = go.send(());
                    report.added.push(Served::new(l, adopted.address));
                    next.push(*adopted);
                }
            }
        }
        for mut a in old.into_iter().flatten() {
            a.close();
            report.removed.push(Served::new(&a.config, a.address));
        }
        self.adopted = next;
        Ok(report)
    }

    /// Closes the listeners added since the engine was built (the engine's own close with it).
    /// Their connections are the engine's, and close with it.
    pub fn close(&mut self) {
        for a in &mut self.adopted {
            a.close();
        }
        self.adopted.clear();
    }
}

impl Drop for Listeners {
    fn drop(&mut self) {
        self.close();
    }
}
