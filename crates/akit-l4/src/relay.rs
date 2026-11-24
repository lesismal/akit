//! One client's connection, or UDP session, and the upstream's it is joined to.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use akio::{Acceptor, Conn, Handler};

use crate::{Inner, Service, WATERMARK};

/// What a `tcp` or `udp` listener serves its connections with: each goes to the service `find`
/// gives when it opens, the one the current graph has for the listener, or is closed when there
/// is none. `find` runs on the I/O thread, once per connection, and must not block.
pub struct ClientAcceptor<F> {
    find: F,
    udp: bool,
}

impl<F> ClientAcceptor<F>
where
    F: Fn() -> Option<Service> + Send + Sync + 'static,
{
    /// The acceptor of a `tcp` listener whose service `find` gives.
    #[must_use]
    pub fn tcp(find: F) -> ClientAcceptor<F> {
        ClientAcceptor { find, udp: false }
    }

    /// The acceptor of a `udp` listener ([`akio::Listener::udp`]) whose service `find` gives:
    /// each client address a session.
    #[must_use]
    pub fn udp(find: F) -> ClientAcceptor<F> {
        ClientAcceptor { find, udp: true }
    }
}

impl<F> std::fmt::Debug for ClientAcceptor<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientAcceptor").finish_non_exhaustive()
    }
}

impl<F> Acceptor for ClientAcceptor<F>
where
    F: Fn() -> Option<Service> + Send + Sync + 'static,
{
    type Handler = Client;

    fn on_open(&self, c: &Conn<Client>) -> Client {
        let Some(service) = (self.find)() else {
            c.close();
            return Client { relay: None };
        };
        let s = service.0;
        if !s.admit() {
            // Past max_conns: closed at once, a UDP session's datagram dropped.
            c.close();
            return Client { relay: None };
        }
        let udp = self.udp;
        let r = Arc::new(Relay {
            id: c.id(),
            udp,
            peer: c.peer_addr().ip(),
            client: c.clone(),
            up: OnceLock::new(),
            state: Mutex::new(State::default()),
            to_up_held: AtomicBool::new(false),
            to_client_held: AtomicBool::new(false),
            last: AtomicU64::new(s.now()),
            ended: AtomicU8::new(0),
            failed: AtomicBool::new(false),
            backend: AtomicUsize::new(usize::MAX),
            s,
        });
        if !udp {
            // Nothing is read from the client until there is somewhere to send it.
            c.hold_reads(true);
            c.set_relayed(true);
        }
        r.s.add(&r);
        Relay::dial(&r);
        Client { relay: Some(r) }
    }

    fn on_shutdown(&self, c: &Conn<Client>) {
        c.close();
    }
}

/// The handler of a client's connection or UDP session.
#[derive(Debug)]
pub struct Client {
    relay: Option<Arc<Relay>>,
}

impl Handler for Client {
    fn on_data(&mut self, _c: &Conn<Client>, data: &[u8]) {
        if let Some(r) = &self.relay {
            r.from_client(data);
        }
    }

    fn on_close(&mut self, _c: &Conn<Client>, _err: Option<akio::Error>) {
        if let Some(r) = self.relay.take() {
            r.client_closed();
        }
    }
}

/// What the dial's outcome is ordered with: what the client sent before the upstream opened (a
/// UDP session's datagrams; a TCP client's reads are held, but what a read brought before the
/// hold took effect is kept too), and whether the client went first.
#[derive(Default)]
struct State {
    /// What came before the upstream's connection opened: datagrams, at most [`WATERMARK`] bytes
    /// of them, or a TCP client's bytes.
    datagrams: Vec<Vec<u8>>,
    pending: usize,
    client_gone: bool,
    /// Addresses tried so far.
    attempt: usize,
}

/// A client's connection, or UDP session, and the upstream's it is joined to.
pub(crate) struct Relay {
    pub(crate) s: Arc<Inner>,
    /// The client connection's ID, its key among the service's open connections.
    pub(crate) id: u64,
    pub(crate) udp: bool,
    peer: IpAddr,
    client: Conn<Client>,
    /// The upstream's connection, once it is open.
    up: OnceLock<Conn<Upstream>>,
    state: Mutex<State>,
    to_up_held: AtomicBool,
    to_client_held: AtomicBool,
    /// When data last went either way, in the service's milliseconds.
    last: AtomicU64,
    /// Sides that are over.
    ended: AtomicU8,
    /// The last dial failed, or the upstream refused the session: counted against the address.
    failed: AtomicBool,
    /// The address dialed last, or `usize::MAX` before the first.
    backend: AtomicUsize,
}

impl std::fmt::Debug for Relay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Relay")
            .field("id", &self.id)
            .field("udp", &self.udp)
            .finish_non_exhaustive()
    }
}

impl Relay {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn last(&self) -> u64 {
        self.last.load(Ordering::Relaxed)
    }

    fn touch(&self) {
        self.last.store(self.s.now(), Ordering::Relaxed);
    }

    /// Picks an address and connects to it, as the service's next attempt.
    fn dial(me: &Arc<Relay>) {
        let pool = &me.s.pool;
        let backend = pool.stream_pick(me.peer);
        me.backend.store(backend, Ordering::SeqCst);
        {
            let mut st = me.state();
            st.attempt = st.attempt.saturating_add(1);
        }
        if let Some(addr) = pool.socket_addr(backend) {
            return Relay::connect(me, addr);
        }
        // A host name, resolved off the I/O thread. Over TCP its addresses are raced (akio's
        // Happy Eyeballs, each attempt within the upstream's connect_timeout); a UDP session
        // takes the first.
        let Some((host, port)) = pool.address(backend).and_then(split_host_port) else {
            return Relay::dial_failed(me);
        };
        let r = Arc::clone(me);
        akio::dns::resolve(host, port, move |res| match res {
            Ok(addrs) if !r.udp => Relay::race(&r, addrs),
            Ok(addrs) => match addrs.first() {
                Some(a) => Relay::connect(&r, *a),
                None => Relay::dial_failed(&r),
            },
            Err(_) => Relay::dial_failed(&r),
        });
    }

    /// Dials `addr`: an IP literal of the upstream's, dialed within the engine's connect
    /// timeout (the shortest of the upstreams', as for HTTP: akio API request A12).
    fn connect(me: &Arc<Relay>, addr: SocketAddr) {
        let Some(engine) = me.s.pool.engine() else {
            return Relay::dial_failed(me);
        };
        let up = UpstreamAcceptor {
            relay: Arc::clone(me),
        };
        let r = Arc::clone(me);
        let failed = move |_: io::Error| Relay::dial_failed(&r);
        if me.udp {
            engine.dial_udp(addr, up, failed);
        } else {
            engine.dial(addr, up, failed);
        }
    }

    /// Dials the first of the addresses a host name resolved to that answers.
    fn race(me: &Arc<Relay>, addrs: Vec<SocketAddr>) {
        let Some(engine) = me.s.pool.engine() else {
            return Relay::dial_failed(me);
        };
        let mut he = akio::dns::HappyEyeballs::default();
        he.connect_timeout = me.s.pool.connect_timeout();
        let up = UpstreamAcceptor {
            relay: Arc::clone(me),
        };
        let r = Arc::clone(me);
        he.dial(engine, addrs, up, move |_| Relay::dial_failed(&r));
    }

    /// The dial failed: it is tried on the next address, up to the service's attempts, while
    /// the client is still there.
    fn dial_failed(me: &Arc<Relay>) {
        let (gone, attempt) = {
            let st = me.state();
            (st.client_gone, st.attempt)
        };
        if !gone && attempt < me.s.attempts {
            me.s.pool
                .stream_done(me.backend.load(Ordering::SeqCst), true);
            return Relay::dial(me);
        }
        // There is no upstream side to wait for.
        me.failed.store(true, Ordering::SeqCst);
        me.side_ended();
        me.client.close();
    }

    /// The upstream's connection opened: joined to the client's, and sent what the client sent
    /// meanwhile; closed at once when the client went first.
    fn opened(&self, up: &Conn<Upstream>) {
        let mut st = self.state();
        if st.client_gone {
            drop(st);
            up.close();
            return;
        }
        if !self.udp {
            up.set_relayed(true);
        }
        for d in std::mem::take(&mut st.datagrams) {
            let _ = up.send_owned(d);
        }
        st.pending = 0;
        let _ = self.up.set(up.clone());
        drop(st);
        if !self.udp {
            self.client.hold_reads(false);
        }
    }

    /// Passes what the client sent to the upstream, or keeps a datagram until the upstream's
    /// socket is open.
    fn from_client(self: &Arc<Self>, data: &[u8]) {
        let up = match self.up.get() {
            Some(up) => up,
            None => {
                let mut st = self.state();
                match self.up.get() {
                    Some(up) => {
                        drop(st);
                        up
                    }
                    None => {
                        // A UDP session's datagrams, up to the watermark, the rest dropped. A
                        // TCP client's reads are held until then, so at most what one read
                        // brought comes here; it is kept, in order.
                        let total = st.pending.saturating_add(data.len());
                        if !self.udp || total <= WATERMARK {
                            st.pending = total;
                            st.datagrams.push(data.to_vec());
                        }
                        return;
                    }
                }
            }
        };
        self.touch();
        // A datagram that cannot be sent is dropped.
        if up.send(data).is_err() || self.udp {
            return;
        }
        if up.queued_bytes() > WATERMARK && !self.to_up_held.swap(true, Ordering::SeqCst) {
            // Held before the drain is heard: a drain that comes at once lifts it.
            self.client.hold_reads(true);
            let me = Arc::clone(self);
            up.on_drain(move || {
                me.to_up_held.store(false, Ordering::SeqCst);
                me.client.hold_reads(false);
            });
        }
    }

    /// Passes what the upstream sent to the client.
    fn from_upstream(self: &Arc<Self>, up: &Conn<Upstream>, data: &[u8]) {
        self.touch();
        if self.client.send(data).is_err() || self.udp {
            return;
        }
        if self.client.queued_bytes() > WATERMARK
            && !self.to_client_held.swap(true, Ordering::SeqCst)
        {
            let from = up.clone();
            from.hold_reads(true);
            let me = Arc::clone(self);
            self.client.on_drain(move || {
                me.to_client_held.store(false, Ordering::SeqCst);
                from.hold_reads(false);
            });
        }
    }

    /// The client closed: the upstream's side ends once what the client sent has gone.
    fn client_closed(&self) {
        let up = {
            let mut st = self.state();
            st.client_gone = true;
            st.datagrams.clear();
            self.up.get().cloned()
        };
        if let Some(up) = up {
            if self.udp {
                up.close();
            } else {
                up.close_after_send();
            }
        }
        self.side_ended();
    }

    /// The upstream closed: the client's side ends once what the upstream sent has gone. A UDP
    /// socket closes when the upstream refuses its datagrams, which counts against the address,
    /// and ends the session: the client's next datagram starts another.
    fn upstream_closed(&self, err: Option<&akio::Error>) {
        if self.udp {
            if matches!(err, Some(akio::Error::Io(e)) if e.kind() == io::ErrorKind::ConnectionRefused)
            {
                self.failed.store(true, Ordering::SeqCst);
            }
            self.client.close();
        } else {
            self.client.close_after_send();
        }
        self.side_ended();
    }

    /// Closes both sides at once, for the idle timeout or a drain.
    pub(crate) fn abandon(&self) {
        self.client.close();
        if let Some(up) = self.up.get() {
            up.close();
        }
    }

    /// Counts a side's end; once both are over, the relay lets go of its place, and the address
    /// hears how the connection went.
    fn side_ended(&self) {
        if self.ended.fetch_add(1, Ordering::SeqCst) != 1 {
            return;
        }
        self.s.remove(self.id);
        let backend = self.backend.load(Ordering::SeqCst);
        if backend != usize::MAX {
            self.s
                .pool
                .stream_done(backend, self.failed.load(Ordering::SeqCst));
        }
        self.s.leave();
    }
}

/// `host:port`, the port a number; the host unbracketed.
fn split_host_port(a: &str) -> Option<(&str, u16)> {
    let (h, p) = a.rsplit_once(':')?;
    let h = h
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(h);
    Some((h, p.trim_start_matches('+').parse().ok()?))
}

/// Opens the upstream's side of a relay.
struct UpstreamAcceptor {
    relay: Arc<Relay>,
}

impl Acceptor for UpstreamAcceptor {
    type Handler = Upstream;

    fn on_open(&self, c: &Conn<Upstream>) -> Upstream {
        self.relay.opened(c);
        Upstream {
            relay: Arc::clone(&self.relay),
        }
    }

    fn on_shutdown(&self, c: &Conn<Upstream>) {
        c.close();
    }
}

/// The handler of the upstream's connection.
pub(crate) struct Upstream {
    relay: Arc<Relay>,
}

impl Handler for Upstream {
    fn on_data(&mut self, c: &Conn<Upstream>, data: &[u8]) {
        self.relay.from_upstream(c, data);
    }

    fn on_close(&mut self, _c: &Conn<Upstream>, err: Option<akio::Error>) {
        self.relay.upstream_closed(err.as_ref());
    }
}
