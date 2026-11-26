//! Reloading (I22, I28): a new graph published and the listeners moved to its, as fit's
//! `listener.Server.Reload` moves them, with akio's listeners added and removed at run time
//! (akio API request A1, ADR-099).
//!
//! A listener the new graph has unchanged (the same name, address, protocols and limits, TLS or
//! not, the same TLS version and client authorities, as fit's `binding.same` compares them; its
//! certificates may differ: I28) is kept: its sockets stay open, and its requests go to the new
//! graph's routes from the swap on, since a [`Dispatcher`] reads the current graph per request; a
//! TLS one's handshakes get the new graph's certificates from then on (I30), since its
//! certificate manager reads the current graph too. The others:
//!
//! - **Added**, the changed ones too, before the swap: the TCP side with
//!   [`Engine::add_listener`], HTTP/3 with [`Engine::bind_udp`] on the engine's UDP thread, so an
//!   address in use fails the reload with nothing changed. A listener replacing one at the same
//!   address is added beside it (Linux shares the port, `SO_REUSEPORT`); where the system refuses,
//!   the old one stops listening first, as fit's reload does, and cannot be put back if the reload
//!   then fails.
//! - **Removed**, the changed ones too, after the swap: [`Engine::shutdown_listener`] stops it
//!   accepting and drains its connections for [`Options::shutdown_timeout`] (each is asked to
//!   close, an HTTP/1 one after its response, an HTTP/2 one with `GOAWAY`). Its UDP side (HTTP/3,
//!   or a `udp` service's) is drained alike (akio's A22, ADR-155): the engine's own UDP listener
//!   with [`Engine::shutdown_listener`], a socket a reload bound with [`UdpBinding::shutdown`];
//!   new peers are dropped, HTTP/3 connections get `GOAWAY` and finish their requests. While a
//!   UDP listener drains beside its replacement on the same address, the kernel still hands part
//!   of the new peers to the old sockets, which drop them: such a drain lasts at most
//!   [`UDP_REPLACED_DRAIN`], a removed one without a replacement the whole drain timeout.
//!
//! A kept `tcp` or `udp` listener (I51) hands its new connections to the new graph's service from
//! the swap on; those the old graph's services still proxy are closed once
//! [`Options::shutdown_timeout`], the drain timeout, has passed, as fit's reload drains them.

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use akio::{Acceptor, Engine, ListenerId, UdpBinding};
use akit_config::{Listener, Protocol};
use akit_runtime::{Graph, GraphCell};

use crate::{
    Error, MAX_PORT_ZERO_BINDS, Options, free_port, h3_acceptor, l4_acceptor, resolve, serves_h3,
    tcp_listener, tcp_side, udp_side,
};

/// A listener as it is served now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Served {
    /// Its name.
    pub name: String,
    /// Where it listens: its TCP address, or its UDP one when it serves HTTP/3 alone.
    pub address: SocketAddr,
    /// Its protocols.
    pub protocols: Vec<Protocol>,
}

/// The listeners served now, as [`Listeners`] last moved them, for another thread to read: the
/// Admin API's `GET /status` (I63). Cheap to clone; reading it takes a lock only to clone an
/// `Arc`, which the control thread takes only to replace it after a reload.
#[derive(Clone, Default)]
pub struct ServedView(std::sync::Arc<std::sync::Mutex<std::sync::Arc<[Served]>>>);

impl fmt::Debug for ServedView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ServedView").field(&self.get()).finish()
    }
}

impl ServedView {
    /// The listeners served now, in the order of the current graph's.
    #[must_use]
    pub fn get(&self) -> std::sync::Arc<[Served]> {
        std::sync::Arc::clone(
            &self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    fn set(&self, served: Vec<Served>) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = served.into();
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

/// The longest a removed UDP listener (HTTP/3, or a `udp` service's) drains while another
/// listens on its address: the new peers the kernel hashes to its sockets go unanswered
/// meanwhile (akio's ADR-155), so its HTTP/3 requests in flight get this long to finish, not
/// the whole drain timeout. A QUIC client whose first packets were dropped sends them again.
pub const UDP_REPLACED_DRAIN: Duration = Duration::from_secs(2);

/// Why a reload was refused; the running graph and listeners are as they were (but see
/// [`Listeners::reload`] on a system without `SO_REUSEPORT`).
#[derive(Debug)]
#[non_exhaustive]
pub enum ReloadError {
    /// A new listener cannot be set up (its address does not resolve or is in use, or it asks for
    /// what akit does not support yet).
    Listener(Error),
}

impl fmt::Display for ReloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReloadError::Listener(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ReloadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ReloadError::Listener(e) => Some(e),
        }
    }
}

/// A listener's UDP socket: its HTTP/3, or its `udp` service's.
#[derive(Debug)]
enum Udp {
    /// A UDP listener the engine was built with, and where it listens.
    Builder(ListenerId, SocketAddr),
    /// Bound by a reload.
    Bound(UdpBinding),
}

impl Udp {
    fn address(&self) -> SocketAddr {
        match self {
            Udp::Builder(_, a) => *a,
            Udp::Bound(b) => b.local_addr(),
        }
    }
}

/// One listener served.
#[derive(Debug)]
struct Entry {
    config: Listener,
    /// Its TLS as loaded, for a reload to tell whether its configuration changed besides its
    /// certificates (which its manager serves from the current graph: I30).
    tls: Option<akit_tls::ListenerTls>,
    /// Its TCP listener in the engine, and where it listens.
    tcp: Option<(ListenerId, SocketAddr)>,
    /// Its HTTP/3, or its `udp` service's socket.
    udp: Option<Udp>,
}

impl Entry {
    fn address(&self) -> Option<SocketAddr> {
        self.tcp
            .map(|(_, a)| a)
            .or_else(|| self.udp.as_ref().map(Udp::address))
    }

    fn served(&self) -> Option<Served> {
        Some(Served {
            name: self.config.name.clone(),
            address: self.address()?,
            protocols: self.config.protocols.clone(),
        })
    }

    /// Stops it listening; `drain`: its TCP connections are drained for that long, and its UDP
    /// side's peers (HTTP/3 connections, `udp` sessions) for `udp_drain` (none: the TCP ones are
    /// left to finish, the UDP ones closed at once, which frees the address: a UDP listener's
    /// sockets stay open while it has peers).
    fn remove(&self, engine: &Engine, drain: Option<Duration>, udp_drain: Option<Duration>) {
        if let Some((id, _)) = self.tcp {
            // Only `NotFound` (removed already) can come back.
            let _ = match drain {
                Some(d) => engine.shutdown_listener(id, d),
                None => engine.remove_listener(id),
            };
        }
        match (&self.udp, udp_drain) {
            (Some(Udp::Builder(id, _)), Some(d)) => {
                let _ = engine.shutdown_listener(*id, d);
            }
            (Some(Udp::Builder(id, _)), None) => {
                let _ = engine.shutdown_listener(*id, Duration::ZERO);
            }
            (Some(Udp::Bound(b)), Some(d)) => b.shutdown(d),
            (Some(Udp::Bound(b)), None) => b.close(),
            (None, _) => {}
        }
    }

    /// [`remove`](Self::remove) at once, for a listener that never served (opened for a reload
    /// that failed) or that makes room for its replacement.
    fn stop(&self, engine: &Engine) {
        self.remove(engine, None, None);
    }
}

/// Whether a running listener configured as `running` can serve `next` as it is, as fit's
/// `binding.same` decides (I28): the same name, address, protocols, body and header limits,
/// `proxy_protocol` (I52) and `socks` rules (I55), TLS or not as `next` has it, and the same newest TLS version and client
/// authentication mode, each fixed in its engine listener. Its certificates may differ, their files and its ACME names: a kept
/// listener's manager serves `next`'s per handshake (I30). Its client authorities are compared
/// by their contents, by [`akit_tls::ListenerTls::same_config`], not by their paths.
fn same(running: &Listener, next: &Listener) -> bool {
    running.name == next.name
        && running.address == next.address
        && running.protocols == next.protocols
        && running.max_body_bytes == next.max_body_bytes
        && running.max_header_bytes == next.max_header_bytes
        && running.proxy_protocol == next.proxy_protocol
        && running.socks == next.socks
        && match (&running.tls, &next.tls) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.max_version == b.max_version
                    && a.client_auth.as_ref().map(|c| c.mode)
                        == b.client_auth.as_ref().map(|c| c.mode)
            }
            _ => false,
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

fn in_use(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::AddrInUse
}

/// `f` until its address is free: a listener that stopped to make room closes its socket at its
/// I/O thread's next loop iteration, not at once. On the control plane, which may wait.
fn retry<T>(f: impl Fn() -> Result<T, Error>) -> Result<T, Error> {
    let mut tries = 0;
    loop {
        match f() {
            Err(Error::Address { error, .. }) if in_use(&error) && tries < ROOM_TRIES => {
                tries = tries.saturating_add(1);
                std::thread::sleep(ROOM_WAIT);
            }
            r => return r,
        }
    }
}

/// A UDP socket bound on `engine` at `addr` serving `acceptor`, its new peers refused by `policy`
/// at akio's filter from the first datagram (ADR-178, akit's A220).
fn bind_udp<A: Acceptor>(
    engine: &Engine,
    addr: SocketAddr,
    acceptor: A,
    policy: Option<&std::sync::Arc<akit_access::Policy>>,
) -> io::Result<UdpBinding> {
    match policy {
        Some(p) => engine.bind_udp_filtered(addr, acceptor, crate::accept_filter(p)),
        None => engine.bind_udp(addr, acceptor),
    }
}

/// How long [`retry`] waits for an address: 100 tries, 10 ms apart.
const ROOM_TRIES: u32 = 100;
const ROOM_WAIT: Duration = Duration::from_millis(10);

/// The listeners of a running engine, moved to each graph [`reload`](Listeners::reload)
/// publishes.
pub struct Listeners {
    engine: std::sync::Arc<Engine>,
    cell: GraphCell,
    opts: Options,
    entries: Vec<Entry>,
    view: ServedView,
    /// The `engine.event_loops` akit started with (I79): the I/O threads of a listener a reload
    /// adds, as of those it started with.
    event_loops: usize,
}

impl fmt::Debug for Listeners {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Listeners")
            .field("served", &self.served())
            .finish_non_exhaustive()
    }
}

impl Listeners {
    /// The listeners of `engine`, built by [`engine`](crate::engine) from `cell`'s current graph
    /// with `opts`, whose backend the listeners a reload adds take, and whose shutdown timeout is
    /// how long a removed listener's connections are drained.
    #[must_use]
    pub fn new(engine: std::sync::Arc<Engine>, cell: GraphCell, opts: &Options) -> Listeners {
        let configs: Vec<(Listener, Option<akit_tls::ListenerTls>)> = cell.with(|g| {
            g.listeners()
                .iter()
                .enumerate()
                .map(|(i, l)| (l.clone(), g.tls(i).cloned()))
                .collect()
        });
        // The builder's listeners first, in order: each listener's TCP side, then its UDP one
        // when it serves h3 (I10), as `addrs` reads them.
        let mut bound = engine.listeners().into_iter();
        let entries = configs
            .into_iter()
            .map(|(config, tls)| Entry {
                tls,
                tcp: if tcp_side(&config) {
                    bound.next().map(|i| (i.id, i.local_addr))
                } else {
                    None
                },
                udp: if udp_side(&config) {
                    bound.next().map(|i| Udp::Builder(i.id, i.local_addr))
                } else {
                    None
                },
                config,
            })
            .collect();
        let event_loops = cell.with(|g| g.engine_config().event_loops);
        let l = Listeners {
            engine,
            cell,
            opts: opts.clone(),
            entries,
            view: ServedView::default(),
            event_loops,
        };
        l.view.set(l.served());
        l
    }

    /// A view of the listeners served now, which each reload updates.
    #[must_use]
    pub fn view(&self) -> ServedView {
        self.view.clone()
    }

    /// The listeners served now, in the order of the current graph's.
    #[must_use]
    pub fn served(&self) -> Vec<Served> {
        self.entries.iter().filter_map(Entry::served).collect()
    }

    /// Opens listener `l` (the new graph's `index`th), beside those running; a running listener
    /// of `room` (its index, and whether it stopped already) at an address the system will not
    /// share stops listening first.
    fn open(
        &self,
        graph: &Graph,
        index: usize,
        l: &Listener,
        room: &mut [(usize, bool)],
    ) -> Result<Entry, Error> {
        let address = |error| Error::Address {
            listener: l.name.clone(),
            address: l.address.clone(),
            error,
        };
        let resolved = resolve(&l.address).map_err(address)?;
        let (tcp, h3) = (tcp_side(l), udp_side(l));
        // Its certificates served per handshake from the cell's graph once `graph` is published
        // in it, and from `graph` until then (I30).
        let tls = akit_runtime::tls::server_configs(&self.cell, graph, index, true)
            .transpose()
            .map_err(|e| crate::tls_error(&e))?;
        let mut last = None;
        for _ in 0..MAX_PORT_ZERO_BINDS {
            let mut addr = resolved;
            let probed = tcp && h3 && addr.port() == 0;
            if probed {
                // HTTP/3 goes on the port TCP gets, the one Alt-Svc names: a port free for both.
                addr.set_port(free_port(addr.ip()).map_err(address)?);
            }
            let mut entry = Entry {
                config: l.clone(),
                tls: graph.tls(index).cloned(),
                tcp: None,
                udp: None,
            };
            if tcp {
                let accept = graph.access(index).and_then(|a| a.accept());
                let id = self.add_tcp(tls.as_ref().map(|c| &c.tcp), l, addr, room, accept)?;
                let local = self
                    .engine
                    .listeners()
                    .into_iter()
                    .find(|i| i.id == id)
                    .map_or(addr, |i| i.local_addr);
                entry.tcp = Some((id, local));
                addr.set_port(local.port());
            }
            if h3 {
                let datagrams = graph.access(index).and_then(|a| a.datagrams());
                let h3_tls = tls.as_ref().and_then(|c| c.h3.as_ref());
                match self.bind_h3(h3_tls, l, addr, room, datagrams) {
                    Ok(b) => entry.udp = Some(Udp::Bound(b)),
                    Err(e) => {
                        entry.stop(&self.engine);
                        match e {
                            Error::Address { error, .. } if probed && in_use(&error) => {
                                last = Some(error);
                                continue;
                            }
                            e => return Err(e),
                        }
                    }
                }
            }
            return Ok(entry);
        }
        Err(address(
            last.unwrap_or_else(|| io::ErrorKind::AddrInUse.into()),
        ))
    }

    /// Stops listening at once the first running listener of `room` not stopped yet that `at`
    /// picks; whether there was one, or one stopped already (its TCP side making room before
    /// its UDP side: a UDP listener's sockets close at its threads' next look, not at once).
    fn make_room(&self, room: &mut [(usize, bool)], at: impl Fn(&Entry) -> bool) -> bool {
        for (j, stopped) in room.iter_mut() {
            if let Some(e) = self.entries.get(*j).filter(|e| at(e)) {
                if !*stopped {
                    e.stop(&self.engine);
                    *stopped = true;
                }
                return true;
            }
        }
        false
    }

    fn add_tcp(
        &self,
        tls: Option<&akio::tls::ServerConfig>,
        l: &Listener,
        addr: SocketAddr,
        room: &mut [(usize, bool)],
        accept: Option<&std::sync::Arc<akit_access::Policy>>,
    ) -> Result<ListenerId, Error> {
        let h3_port = serves_h3(l).then_some(addr.port());
        let make = || {
            tcp_listener(
                &self.cell,
                l,
                tls,
                crate::tcp_engine_listener(addr, &self.opts.backend, self.event_loops),
                h3_port,
                accept,
            )
        };
        let failed = |error| Error::Address {
            listener: l.name.clone(),
            address: l.address.clone(),
            error,
        };
        match self.engine.add_listener(make()?) {
            Err(e)
                if in_use(&e)
                    && self.make_room(room, |g| g.tcp.is_some_and(|(_, a)| a == addr)) =>
            {
                retry(|| self.engine.add_listener(make()?).map_err(failed))
            }
            r => r.map_err(failed),
        }
    }

    fn bind_h3(
        &self,
        h3_tls: Option<&akio::tls::ServerConfig>,
        l: &Listener,
        addr: SocketAddr,
        room: &mut [(usize, bool)],
        datagrams: Option<&std::sync::Arc<akit_access::Policy>>,
    ) -> Result<UdpBinding, Error> {
        let bind = || {
            let failed = |error| Error::Address {
                listener: l.name.clone(),
                address: l.address.clone(),
                error,
            };
            if !serves_h3(l) {
                // A `udp` listener's (I51).
                let a = l4_acceptor(&self.cell, l);
                return bind_udp(&self.engine, addr, a, datagrams).map_err(failed);
            }
            let a = h3_acceptor(&self.cell, l, h3_tls)?;
            bind_udp(&self.engine, addr, a, datagrams).map_err(failed)
        };
        match bind() {
            Err(Error::Address { error: e, .. })
                if in_use(&e)
                    && self.make_room(room, |g| {
                        g.udp.as_ref().is_some_and(|u| u.address() == addr)
                    }) =>
            {
                retry(bind)
            }
            r => r,
        }
    }

    /// Sets each listener's accept filters to the current graph's access lists (I85, akio's A3;
    /// its UDP side's, ADR-178, A220): a listener kept by a reload is not bound anew, its filters
    /// swapped; connections (and UDP peers) it accepted before are not checked again. Every
    /// `accept`, and every datagram of a new peer, after this goes by the new ones.
    fn gate(&self) {
        self.cell.with(|g| {
            for e in &self.entries {
                let access = g.listener_index(&e.config.name).and_then(|i| g.access(i));
                if let Some((id, _)) = e.tcp {
                    // Only `NotFound` (removed meanwhile) can come back.
                    let _ = match access.and_then(|a| a.accept()) {
                        Some(p) => self.engine.set_listener_filter(id, crate::accept_filter(p)),
                        None => self.engine.clear_listener_filter(id),
                    };
                }
                let datagrams = access.and_then(|a| a.datagrams());
                // As above; a bound socket's filter always swaps.
                let _ = match (&e.udp, datagrams) {
                    (Some(Udp::Builder(id, _)), Some(p)) => self
                        .engine
                        .set_listener_filter(*id, crate::accept_filter(p)),
                    (Some(Udp::Builder(id, _)), None) => self.engine.clear_listener_filter(*id),
                    (Some(Udp::Bound(b)), Some(p)) => b.set_filter(crate::accept_filter(p)),
                    (Some(Udp::Bound(b)), None) => b.clear_filter(),
                    (None, _) => Ok(()),
                };
            }
        });
    }

    /// Per listener of `graph`, the running one it keeps; and per running listener, whether it
    /// is kept: the first not kept yet that is [`same`] and whose loaded TLS is the same but for
    /// its certificates. A TLS listener whose certificates changed (their files, their contents,
    /// its ACME names) is kept: its manager serves the new ones from the swap on.
    fn keeps(&self, graph: &Graph) -> (Vec<Option<usize>>, Vec<bool>) {
        let mut kept = vec![false; self.entries.len()];
        let keeps: Vec<Option<usize>> = graph
            .listeners()
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let tls = graph.tls(i);
                claim(&self.entries, &mut kept, |e| {
                    same(&e.config, l)
                        && match (&e.tls, tls) {
                            (Some(a), Some(b)) => a.same_config(b),
                            (a, b) => a.is_none() && b.is_none(),
                        }
                })
            })
            .collect();
        (keeps, kept)
    }

    /// What [`reload`](Listeners::reload) would do with `graph`, done without changing anything,
    /// for a dry run (I27, as fit's): the listeners it would open, at their addresses as
    /// configured (a port 0 stays 0), and those it would close. A new listener's address is
    /// resolved and, unless a running listener holds it (which a reload makes room on), tried by
    /// binding it and letting it go at once. Runs on the control plane (it resolves names and
    /// binds sockets).
    ///
    /// # Errors
    ///
    /// As [`reload`](Listeners::reload)'s: a new listener's address that does not resolve or is
    /// taken.
    pub fn plan(&self, graph: &Graph) -> Result<Reloaded, ReloadError> {
        let (keeps, kept) = self.keeps(graph);
        let mut report = Reloaded {
            kept: keeps.iter().filter(|k| k.is_some()).count(),
            ..Reloaded::default()
        };
        for (l, keep) in graph.listeners().iter().zip(&keeps) {
            if keep.is_some() {
                continue;
            }
            let address = |error| {
                ReloadError::Listener(Error::Address {
                    listener: l.name.clone(),
                    address: l.address.clone(),
                    error,
                })
            };
            let addr = resolve(&l.address).map_err(address)?;
            let held = self.entries.iter().any(|e| e.address() == Some(addr));
            if addr.port() != 0 && !held {
                if tcp_side(l) {
                    std::net::TcpListener::bind(addr).map_err(address)?;
                }
                if udp_side(l) {
                    std::net::UdpSocket::bind(addr).map_err(address)?;
                }
            }
            report.added.push(Served {
                name: l.name.clone(),
                address: addr,
                protocols: l.protocols.clone(),
            });
        }
        report.removed = self
            .entries
            .iter()
            .zip(&kept)
            .filter(|(_, k)| !**k)
            .filter_map(|(e, _)| e.served())
            .collect();
        Ok(report)
    }

    /// Publishes `graph` in the cell and moves the listeners to its: the unchanged ones kept,
    /// the new and changed ones opened before the swap, the removed and changed ones drained
    /// after it. Runs on the control plane (it resolves names and binds sockets). The cell's old
    /// graph is left retiring: the caller [`collect`](GraphCell::collect)s it.
    ///
    /// # Errors
    ///
    /// When a new listener cannot be set up; nothing changes then, and `graph` is dropped (its
    /// modules cleaned up). Except: a listener that had to stop listening to make room for its
    /// replacement on a system that does not share ports stays stopped, as in fit.
    pub fn reload(&mut self, graph: Graph) -> Result<Reloaded, ReloadError> {
        let configs = graph.listeners().to_vec();
        let (keeps, kept) = self.keeps(&graph);
        crate::load_sockmap(configs.iter());
        // The running listeners not kept, by index, and whether they stopped already.
        let mut room: Vec<(usize, bool)> = kept
            .iter()
            .enumerate()
            .filter(|(_, k)| !**k)
            .map(|(j, _)| (j, false))
            .collect();

        // Before the swap: the new and changed listeners opened.
        let mut opened: Vec<Option<Entry>> = Vec::with_capacity(configs.len());
        for (i, (l, keep)) in configs.iter().zip(&keeps).enumerate() {
            if keep.is_some() {
                opened.push(None);
                continue;
            }
            match self.open(&graph, i, l, &mut room) {
                Ok(e) => opened.push(Some(e)),
                Err(err) => {
                    // Nothing changes: the listeners opened for it are closed again; those that
                    // stopped to make room stay stopped.
                    for e in opened.iter().flatten() {
                        e.stop(&self.engine);
                    }
                    let stopped: Vec<usize> =
                        room.iter().filter(|(_, s)| *s).map(|(j, _)| *j).collect();
                    let mut j = 0usize;
                    self.entries.retain(|_| {
                        let keep = !stopped.contains(&j);
                        j = j.saturating_add(1);
                        keep
                    });
                    return Err(ReloadError::Listener(err));
                }
            }
        }

        // The old graph's layer 4 services, whose connections are closed after the drain
        // timeout (I51).
        let services: Vec<akit_l4::Service> = self.cell.with(|g| g.services().to_vec());
        let live = self.engine.telemetry().live();

        // The swap: requests go to the new graph from here on, and its write stall timeout to
        // every connection, those open included (akio's live write timeout, ADR-108).
        let stall = graph.write_stall_timeout();
        self.cell.swap(graph);
        live.set_write_timeout(Some(stall));
        if !services.is_empty() {
            let drain = self.opts.shutdown_timeout;
            let left = services.clone();
            // The timers full: drained at once rather than never.
            let timer = akio::timer::after_send(drain, move || {
                left.iter().for_each(akit_l4::Service::drain);
            });
            if timer.is_err() {
                services.iter().for_each(akit_l4::Service::drain);
            }
        }

        let mut old: Vec<Option<Entry>> = self.entries.drain(..).map(Some).collect();
        let mut report = Reloaded {
            kept: keeps.iter().filter(|k| k.is_some()).count(),
            ..Reloaded::default()
        };
        let drain = self.opts.shutdown_timeout;
        // The UDP addresses the new listeners took: an old UDP listener there drains briefly.
        let taken: Vec<SocketAddr> = opened
            .iter()
            .flatten()
            .filter_map(|e| e.udp.as_ref().map(Udp::address))
            .collect();
        for (j, stopped) in room {
            if let Some(e) = old.get_mut(j).and_then(Option::take) {
                if !stopped {
                    let replaced = e.udp.as_ref().is_some_and(|u| taken.contains(&u.address()));
                    let udp_drain = if replaced {
                        drain.min(UDP_REPLACED_DRAIN)
                    } else {
                        drain
                    };
                    e.remove(&self.engine, Some(drain), Some(udp_drain));
                }
                report.removed.extend(e.served());
            }
        }
        let mut next = Vec::with_capacity(configs.len());
        for (i, ((keep, new), config)) in keeps.iter().zip(opened).zip(configs).enumerate() {
            let e = match (keep, new) {
                (_, Some(e)) => {
                    report.added.extend(e.served());
                    Some(e)
                }
                // Kept: it goes by the new config from here on (its certificates may differ),
                // which the next reload compares with.
                (Some(j), None) => old.get_mut(*j).and_then(Option::take).map(|mut e| {
                    e.tls = self.cell.with(|g| g.tls(i).cloned());
                    e.config = config;
                    e
                }),
                (None, None) => None,
            };
            next.extend(e);
        }
        self.entries = next;
        self.gate();
        self.view.set(self.served());
        Ok(report)
    }
}
