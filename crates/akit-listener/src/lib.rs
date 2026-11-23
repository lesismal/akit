#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's listeners on akio: TCP, UDP, TLS and QUIC entry points, and protocol splitting with
//! akio-mux.
//!
//! So far (I05): HTTP/1.1 and h2c listeners from the graph's config, each an akio TCP listener
//! serving a [`Dispatcher`], which routes every request through the current graph of a
//! [`GraphCell`] and answers 404 when no route matches. I09: a listener with `tls` serves HTTPS,
//! its HTTP acceptor wrapped in akio-tls with the configuration the graph loaded (certificates
//! by SNI, ALPN choosing h2 or http/1.1, client authentication). Since I11 the routes go by the
//! request's canonical path, as fit's listener does: dot and empty segments removed, a path
//! climbing above the root or hiding a separator in an escape answered `400 bad request path`.
//!
//! I22: [`Listeners::reload`] publishes a new graph and moves the listeners to its (see
//! [`reload`]).
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

use std::borrow::Cow;
use std::fmt;
use std::io;
use std::net::{Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

use akio::tls::Tls;
use akio::{Backend, Config as EngineConfig, Engine, Listener as EngineListener};
use akio_http::{Context, HttpConfig};
use akit_config::{Listener, Protocol};
use akit_runtime::GraphCell;

pub mod reload;
pub use reload::{Listeners, ReloadError, Reloaded, Served};

/// fit's answer when no route matches.
const NOT_FOUND: &str = "404 page not found\n";

/// fit's answer to a path that has no canonical form ([`akit_router::canonicalize`]).
const BAD_REQUEST_PATH: &str = "400 bad request path\n";

/// The [`Context::local`] that holds a request's canonical path, escaped, when it differs from
/// the path sent (dot or empty segments were removed): what the routes matched, and what handlers
/// go by (akio's request target cannot be rewritten). Unset for nearly every request, whose path
/// is canonical already.
pub const CANONICAL_PATH: &str = "akit.canonical_path";

/// The akio service of one listener: each request goes to the route the current graph picks for
/// it on this listener, or is answered 404. Runs on the I/O thread, and only reads the graph.
pub struct Dispatcher {
    cell: GraphCell,
    listener: String,
}

impl Dispatcher {
    /// The dispatcher of the listener named `listener`.
    #[must_use]
    pub fn new(cell: GraphCell, listener: impl Into<String>) -> Dispatcher {
        Dispatcher {
            cell,
            listener: listener.into(),
        }
    }
}

impl fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dispatcher")
            .field("listener", &self.listener)
            .finish_non_exhaustive()
    }
}

impl akio_http::Service for Dispatcher {
    fn call(&self, ctx: &mut Context) {
        self.cell.with(|g| {
            // The routes go by the canonical path, as fit's do; a path that has none is answered
            // 400.
            let (route, changed) = {
                let Some(path) = akit_router::canonicalize(ctx.path()) else {
                    ctx.text(400, BAD_REQUEST_PATH);
                    return;
                };
                let route = g
                    .listener_index(&self.listener)
                    .and_then(|i| g.route(i, ctx.header("host").unwrap_or(""), &path.decoded));
                let changed = match path.escaped {
                    Cow::Owned(p) => Some(p),
                    Cow::Borrowed(_) => None,
                };
                (route, changed)
            };
            if let Some(p) = changed {
                ctx.set_local(CANONICAL_PATH, p);
            }
            match route {
                Some(r) => r.handler.call(ctx),
                None => ctx.text(404, NOT_FOUND),
            }
        });
    }

    // Request bodies are buffered (akio's default) until a handler takes them as a stream:
    // the proxy, I14, routes in `body_mode` too.
}

/// Why the listeners could not be set up.
#[derive(Debug)]
pub enum Error {
    /// A listener's address does not resolve.
    Address {
        /// The listener's name.
        listener: String,
        /// Its address.
        address: String,
        /// Why.
        error: io::Error,
    },
    /// A listener asks for what akit does not support yet.
    NotSupportedYet {
        /// The listener's name.
        listener: String,
        /// What.
        what: String,
    },
    /// akio could not build or start the engine (an address in use, say).
    Engine(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Address {
                listener,
                address,
                error,
            } => write!(f, "listener {listener}: address {address:?}: {error}"),
            Error::NotSupportedYet { listener, what } => {
                write!(f, "listener {listener}: not supported yet: {what}")
            }
            Error::Engine(e) => write!(f, "listening: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Address { error, .. } | Error::Engine(error) => Some(error),
            Error::NotSupportedYet { .. } => None,
        }
    }
}

/// Where a listener's `address` listens: `":8080"` on every address, IPv6 and IPv4 (akio binds
/// `[::]` dual-stack), as Go's `net.Listen` does; a host name resolved, its first address taken.
/// Resolving may block: call it on the control plane.
///
/// # Errors
///
/// When the address does not resolve.
pub fn resolve(address: &str) -> io::Result<SocketAddr> {
    if let Some(port) = address.strip_prefix(':') {
        let port = port
            .parse::<u16>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "not a port"))?;
        return Ok(SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), port));
    }
    // "[::1]:80", "127.0.0.1:80" and "localhost:80" as std takes them.
    address
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "resolves to no address"))
}

/// The akio HTTP settings of a listener serving `protocols`.
fn http_config(l: &Listener) -> Result<HttpConfig, Error> {
    let h1 = l.protocols.contains(&Protocol::Http11);
    let h2c = l.protocols.contains(&Protocol::H2c);
    // Over TLS, ALPN picks h2 (akit_tls::alpn): a client that negotiates it sends HTTP/2's
    // preface. One that offers no ALPN is served HTTP/1.1 even by an h2-only listener, as Go's
    // server serves it.
    let h2 = l.protocols.contains(&Protocol::H2);
    if h2c && !h1 {
        // fit serves HTTP/2 alone then; akio's server always takes HTTP/1 as well.
        return Err(Error::NotSupportedYet {
            listener: l.name.clone(),
            what: "protocols h2c without http/1.1".into(),
        });
    }
    let mut cfg = HttpConfig::default();
    if !h2c && !h2 {
        cfg.h2 = None;
    }
    Ok(cfg)
}

/// How the listeners are served.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Options {
    /// The akio backend of every listener (epoll, kqueue on macOS, by default).
    pub backend: Backend,
    /// How long [`shutdown`] waits for requests in flight before closing their connections:
    /// fit's `-shutdown-timeout`, 30 s by default.
    pub shutdown_timeout: Duration,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            backend: Backend::default(),
            shutdown_timeout: Duration::from_secs(30),
        }
    }
}

/// An akio engine serving every listener of the cell's current graph, each with its own
/// [`Dispatcher`], and hosting the connections of its upstream pools (on the last listener's
/// threads); built, not started. [`Engine::local_addrs`] gives where each listens, in the
/// graph's order.
///
/// # Errors
///
/// When an address does not resolve, a listener asks for what akit does not support yet, or akio
/// cannot build the engine (an address in use).
pub fn engine(cell: &GraphCell, opts: &Options) -> Result<Engine, Error> {
    let (listeners, tls): (Vec<Listener>, Vec<_>) = cell.with(|g| {
        let ls = g.listeners().to_vec();
        let tls = (0..ls.len()).map(|i| g.tls(i).cloned()).collect();
        (ls, tls)
    });
    let mut cfg = EngineConfig::default();
    cfg.shutdown_timeout = opts.shutdown_timeout;
    // The upstream pools dial on this engine. akio's client honours an upstream's
    // connect_timeout only for a name that resolves to several addresses, and dials a lone one
    // with the engine's (akio API request A12): until then the engine's is the shortest of the
    // upstreams', which is each one's own when they agree (fit's default, 10s, when none sets it).
    if let Some(t) = cell.with(|g| g.upstreams().iter().map(|p| p.connect_timeout()).min()) {
        cfg.connect_timeout = Some(t);
    }
    let mut b = Engine::builder().config(cfg);
    for (l, tls) in listeners.iter().zip(tls) {
        let addr = resolve(&l.address).map_err(|error| Error::Address {
            listener: l.name.clone(),
            address: l.address.clone(),
            error,
        })?;
        let http = akio_http::serve_with(
            http_config(l)?,
            Dispatcher::new(cell.clone(), l.name.clone()),
        );
        let listener = EngineListener::tcp(addr).backend(opts.backend.clone());
        b = b.listen(match (&l.tls, tls) {
            (None, _) => listener.serve(http),
            (Some(_), Some(cfg)) => listener.serve(Tls::server(cfg, http)),
            // The graph loads every listener's tls, or is not compiled.
            (Some(_), None) => {
                return Err(Error::Engine(io::Error::other(format!(
                    "listener {}: its tls was not loaded",
                    l.name
                ))));
            }
        });
    }
    b.build().map_err(Error::Engine)
}

/// Shuts `engine` down gracefully and waits until it has: the listeners stop accepting, idle
/// connections are closed, and a connection with requests in flight is closed once their
/// responses are sent (an HTTP/2 one gets `GOAWAY`), or when the engine's shutdown timeout
/// ([`Options::shutdown_timeout`], `timeout` here) passes. Returns false when it passed with
/// connections still open, which akio then closed.
pub fn shutdown(engine: &Engine, timeout: Duration) -> bool {
    let open = engine.telemetry().open_connections();
    let start = Instant::now();
    engine.shutdown();
    engine.wait();
    // akio waits as long as the timeout only for connections it then closes.
    open == 0 || start.elapsed() < timeout
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::net::SocketAddr;

    #[test]
    fn addresses() {
        assert_eq!(
            super::resolve(":8080").unwrap(),
            "[::]:8080".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            super::resolve("127.0.0.1:0").unwrap(),
            "127.0.0.1:0".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            super::resolve("[::1]:80").unwrap(),
            "[::1]:80".parse::<SocketAddr>().unwrap()
        );
        assert!(super::resolve("localhost:80").unwrap().ip().is_loopback());
        assert!(super::resolve(":x").is_err());
    }
}
