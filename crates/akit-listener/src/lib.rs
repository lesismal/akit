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
//! I10: a TLS listener serving `h3` serves HTTP/3 on akio-h3, over QUIC on its port in UDP, with
//! the certificates its TCP connections serve; its HTTP/1.1 and HTTP/2 responses announce it with
//! `Alt-Svc`, as fit's do.
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations that
//! fill this crate in.

use std::borrow::Cow;
use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv6Addr, SocketAddr, TcpListener, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use akio::tls::Tls;
use akio::{Backend, Config as EngineConfig, Engine, Listener as EngineListener};
use akio_http::{Context, HttpConfig};
use akit_config::{Listener, Protocol};
use akit_runtime::GraphCell;

/// fit's answer when no route matches.
const NOT_FOUND: &str = "404 page not found\n";

/// fit's answer to a path that has no canonical form ([`akit_router::canonicalize`]).
const BAD_REQUEST_PATH: &str = "400 bad request path\n";

pub use akit_runtime::{CANONICAL_PATH, TLS};

/// The akio service of one listener: each request goes to the route the current graph picks for
/// it on this listener, or is answered 404. Runs on the I/O thread, and only reads the graph.
pub struct Dispatcher {
    cell: GraphCell,
    listener: String,
    tls: bool,
    /// The `Alt-Svc` value every response gets, for the TCP side of a listener that also serves
    /// HTTP/3; made once, when the listener is set up.
    alt_svc: Option<String>,
}

impl Dispatcher {
    /// The dispatcher of the listener named `listener`.
    #[must_use]
    pub fn new(cell: GraphCell, listener: impl Into<String>) -> Dispatcher {
        Dispatcher {
            cell,
            listener: listener.into(),
            tls: false,
            alt_svc: None,
        }
    }

    /// The same, for a listener that serves TLS: its requests carry [`TLS`].
    #[must_use]
    pub fn tls(mut self, tls: bool) -> Dispatcher {
        self.tls = tls;
        self
    }

    /// Adds `Alt-Svc: <value>` to every response, as fit's dispatcher does on a listener that also
    /// serves HTTP/3 ([`alt_svc`]).
    #[must_use]
    pub fn with_alt_svc(mut self, value: impl Into<String>) -> Dispatcher {
        self.alt_svc = Some(value.into());
        self
    }
}

/// The `Alt-Svc` value announcing HTTP/3 on `port` of the same host, as fib's `http3.AltSvc`
/// writes it: `h3=":443"; ma=86400`.
#[must_use]
pub fn alt_svc(port: u16) -> String {
    format!("h3=\":{port}\"; ma=86400")
}

impl fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dispatcher")
            .field("listener", &self.listener)
            .field("alt_svc", &self.alt_svc)
            .finish_non_exhaustive()
    }
}

impl akio_http::Service for Dispatcher {
    fn call(&self, ctx: &mut Context) {
        // On every response, the 400 and 404 below included, as fit adds it before routing.
        if let Some(v) = &self.alt_svc {
            ctx.add_response_header("alt-svc", v);
        }
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
            if self.tls {
                ctx.set_local(TLS, "");
            }
            match route {
                Some(r) => r.handler.call(ctx),
                None => ctx.text(404, NOT_FOUND),
            }
        });
    }

    // Request bodies are buffered (akio's default) until a handler takes them as a stream:
    // the proxy's, I16, routes in `body_mode` too.
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

/// Whether a listener serves HTTP over TCP: HTTP/1.1, h2c or h2.
fn serves_tcp(l: &Listener) -> bool {
    l.protocols
        .iter()
        .any(|p| matches!(p, Protocol::Http11 | Protocol::H2c | Protocol::H2))
}

/// Whether a listener serves HTTP/3, over QUIC in UDP.
fn serves_h3(l: &Listener) -> bool {
    l.protocols.contains(&Protocol::H3)
}

/// How many ports [`engine`] tries for listeners at port 0 that serve HTTP/3 beside TCP, whose
/// UDP port is the one TCP gets and may be taken (fit's `maxPortZeroBinds`).
const MAX_PORT_ZERO_BINDS: usize = 8;

/// Where one listener listens, as [`addrs`] gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Addrs {
    /// Its TCP address, for a listener serving HTTP/1.1, h2c or h2.
    pub tcp: Option<SocketAddr>,
    /// Its UDP address, where HTTP/3 listens, for a listener serving h3: the same port as TCP's.
    pub udp: Option<SocketAddr>,
}

impl Addrs {
    /// The address to report: TCP's, or UDP's for a listener serving HTTP/3 alone (fit's
    /// `Addr.Addr`).
    #[must_use]
    pub fn addr(&self) -> Option<SocketAddr> {
        self.tcp.or(self.udp)
    }
}

/// Where each of `listeners` (the graph's [`engine`] was built from) listens on `engine`, in
/// their order.
#[must_use]
pub fn addrs(listeners: &[Listener], engine: &Engine) -> Vec<Addrs> {
    let mut bound = engine.local_addrs().into_iter();
    listeners
        .iter()
        .map(|l| Addrs {
            tcp: if serves_tcp(l) { bound.next() } else { None },
            udp: if serves_h3(l) { bound.next() } else { None },
        })
        .collect()
}

/// An akio engine serving every listener of the cell's current graph, each with its own
/// [`Dispatcher`], and hosting the connections of its upstream pools (on the last listener's
/// threads); built, not started. A listener serving `h3` serves HTTP/3 over UDP on the port its
/// TCP side listens on, which the `Alt-Svc` of its TCP responses names. [`addrs`] gives where
/// each listens, in the graph's order ([`Engine::local_addrs`] lists each listener's TCP address,
/// then its UDP one).
///
/// # Errors
///
/// When an address does not resolve, a listener asks for what akit does not support yet, or akio
/// cannot build the engine (an address in use).
pub fn engine(cell: &GraphCell, opts: &Options) -> Result<Engine, Error> {
    let mut last = None;
    for _ in 0..MAX_PORT_ZERO_BINDS {
        match build(cell, opts) {
            // A listener at port 0 serving h3 beside TCP: the port probed free for both was
            // taken before akio bound it. Another one, as fit's bind tries.
            Err((Error::Engine(e), true)) if e.kind() == io::ErrorKind::AddrInUse => {
                last = Some(e);
            }
            Ok(e) => return Ok(e),
            Err((e, _)) => return Err(e),
        }
    }
    Err(Error::Engine(
        last.unwrap_or_else(|| io::ErrorKind::AddrInUse.into()),
    ))
}

/// [`engine`]'s one try; the error says too whether a port was probed for a listener at port 0.
fn build(cell: &GraphCell, opts: &Options) -> Result<Engine, (Error, bool)> {
    type Loaded = (
        Listener,
        Option<akio::tls::ServerConfig>,
        Option<akio::tls::ServerConfig>,
    );
    let listeners: Vec<Loaded> = cell.with(|g| {
        g.listeners()
            .iter()
            .enumerate()
            .map(|(i, l)| (l.clone(), g.tls(i).cloned(), g.h3_tls(i).cloned()))
            .collect()
    });
    let mut probed = false;
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
    for (l, tls, h3_tls) in &listeners {
        let fail = |e: Error| (e, false);
        let mut addr = resolve(&l.address).map_err(|error| {
            fail(Error::Address {
                listener: l.name.clone(),
                address: l.address.clone(),
                error,
            })
        })?;
        let (tcp, h3) = (serves_tcp(l), serves_h3(l));
        if tcp && h3 && addr.port() == 0 {
            // HTTP/3 goes on the port TCP gets, the one Alt-Svc names: a port free for both.
            probed = true;
            let port = free_port(addr.ip()).map_err(|e| (Error::Engine(e), true))?;
            addr.set_port(port);
        }
        if tcp {
            let mut d = Dispatcher::new(cell.clone(), l.name.clone()).tls(l.tls.is_some());
            if h3 {
                d = d.with_alt_svc(alt_svc(addr.port()));
            }
            let http = akio_http::serve_with(http_config(l).map_err(fail)?, d);
            let listener = EngineListener::tcp(addr).backend(opts.backend.clone());
            b = b.listen(match (&l.tls, tls) {
                (None, _) => listener.serve(http),
                (Some(_), Some(cfg)) => listener.serve(Tls::server(cfg.clone(), http)),
                // The graph loads every listener's tls, or is not compiled.
                (Some(_), None) => return Err(fail(not_loaded(l))),
            });
        }
        if h3 {
            // akit-config refuses h3 without tls; the graph loads its h3 TLS with the rest.
            let Some(tls) = h3_tls else {
                return Err(fail(not_loaded(l)));
            };
            let quic = akio_quic::server_config_from(tls)
                .map_err(|e| fail(Error::Engine(io::Error::other(e.to_string()))))?;
            // Per-peer UDP listeners run on akio's epoll (kqueue) UDP loop whatever the TCP
            // backend: the backend is left to akio.
            b = b.listen(EngineListener::udp(addr).serve(akio_h3::serve_with(
                quic,
                akio_h3::H3Config::default(),
                // HTTP/3 is always over TLS.
                Dispatcher::new(cell.clone(), l.name.clone()).tls(true),
            )));
        }
    }
    b.build().map_err(|e| (Error::Engine(e), probed))
}

fn not_loaded(l: &Listener) -> Error {
    Error::Engine(io::Error::other(format!(
        "listener {}: its tls was not loaded",
        l.name
    )))
}

/// A port on `ip` free for TCP and for UDP at once, for a listener at port 0 serving HTTP/3 on
/// the port TCP gets: TCP's free port, unless UDP's is taken, as fit's bind tries them. Binds
/// and closes sockets: control plane only. The port may still be taken before akio binds it;
/// [`engine`] then tries again.
fn free_port(ip: IpAddr) -> io::Result<u16> {
    let mut last = io::Error::from(io::ErrorKind::AddrInUse);
    for _ in 0..MAX_PORT_ZERO_BINDS {
        let tcp = TcpListener::bind(SocketAddr::new(ip, 0))?;
        let port = tcp.local_addr()?.port();
        match UdpSocket::bind(SocketAddr::new(ip, port)) {
            Ok(_) => return Ok(port),
            Err(e) => last = e,
        }
    }
    Err(last)
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
