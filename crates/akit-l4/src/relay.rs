//! One client's connection, or UDP session, and the upstream's it is joined to.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use akio::{Acceptor, Conn, Handler};
use akio_mux::proxy::{self, ProxyHeader};
use akio_mux::tls::{HelloError, parse_client_hello};

use crate::sockmap::{self, Spliced};
use crate::{Entry, Inner, MAX_CLIENT_HELLO, OPENING_TIMEOUT, Service, WATERMARK};

/// What a `tcp` or `udp` listener serves its connections with: each goes to a service of the
/// [`Entry`] `find` gives when it opens, the one the current graph has for the listener, or is
/// closed when there is none. When the entry picks its services by server name, a TCP
/// connection's ClientHello is read first, at most [`MAX_CLIENT_HELLO`] bytes within
/// [`OPENING_TIMEOUT`]. `find` runs on the I/O thread, once per connection, and must not block.
pub struct ClientAcceptor<F> {
    find: F,
    udp: bool,
}

impl<F> ClientAcceptor<F>
where
    F: Fn() -> Option<Entry> + Send + Sync + 'static,
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
    F: Fn() -> Option<Entry> + Send + Sync + 'static,
{
    type Handler = Client;

    fn on_open(&self, c: &Conn<Client>) -> Client {
        let Some(entry) = (self.find)() else {
            c.close();
            return Client::default();
        };
        if self.udp || !entry.routes() {
            let Some(s) = entry.fallback() else {
                c.close();
                return Client::default();
            };
            return Client {
                relay: start(s, c, self.udp, Vec::new(), entry.sockmap()),
                opening: None,
            };
        }
        // The ClientHello picks the service: read before one takes the connection, within the
        // opening timeout, which closes it unless a service took it first.
        let waiting = Arc::new(Mutex::new(Some(c.clone())));
        let w = Arc::clone(&waiting);
        let armed = akit_upstream::timer::after(OPENING_TIMEOUT, move || {
            if let Some(c) = w.lock().unwrap_or_else(PoisonError::into_inner).take() {
                c.close();
            }
        });
        if !armed {
            // The timers are full: a connection that could wait forever is not taken.
            c.close();
            return Client::default();
        }
        Client {
            relay: None,
            opening: Some(Box::new(Opening {
                entry,
                buf: Vec::new(),
                waiting,
            })),
        }
    }

    fn on_shutdown(&self, c: &Conn<Client>) {
        c.close();
    }
}

/// Has `s` proxy `c`, a connection one of its listeners accepted or a UDP listener's new
/// session, `initial` what the client sent before, spliced in the kernel when `sockmap` (its
/// listener's) and the relay allow; `None` when `s` takes no more (`max_conns`, or drained), and
/// `c` is closed.
fn start(
    s: &Service,
    c: &Conn<Client>,
    udp: bool,
    initial: Vec<u8>,
    sockmap: bool,
) -> Option<Arc<Relay>> {
    let s = Arc::clone(&s.0);
    if !s.admit() {
        // Past max_conns: closed at once, a UDP session's datagram dropped.
        c.close();
        return None;
    }
    // The client a trusted PROXY header named, and where it connected to; else the socket's.
    let client_addr = proxy::client_addr(c);
    let dst = proxy::header(c)
        .and_then(ProxyHeader::destination)
        .unwrap_or_else(|| c.local_addr());
    let pending = initial.len();
    let datagrams = if initial.is_empty() {
        Vec::new()
    } else {
        vec![initial]
    };
    let r = Arc::new(Relay {
        id: c.id(),
        udp,
        peer: client_addr.ip(),
        client_addr,
        dst,
        client: c.clone(),
        up: OnceLock::new(),
        state: Mutex::new(State {
            datagrams,
            pending,
            ..State::default()
        }),
        to_up_held: AtomicBool::new(false),
        to_client_held: AtomicBool::new(false),
        last: AtomicU64::new(s.now()),
        ended: AtomicU8::new(0),
        failed: AtomicBool::new(false),
        backend: AtomicUsize::new(usize::MAX),
        kernel: if udp || !sockmap {
            None
        } else {
            sockmap::relay()
        },
        spliced: OnceLock::new(),
        s,
    });
    if !udp {
        // Nothing is read from the client until there is somewhere to send it.
        c.hold_reads(true);
        c.set_relayed(true);
    }
    r.s.add(&r);
    Relay::dial(&r);
    Some(r)
}

/// A TCP connection whose ClientHello is still being read, before a service takes it.
struct Opening {
    entry: Entry,
    /// What the client sent so far, sent on to the upstream once a service takes it.
    buf: Vec<u8>,
    /// The connection, until a service takes it or the opening timeout closes it.
    waiting: Arc<Mutex<Option<Conn<Client>>>>,
}

impl Opening {
    /// Takes what the client sent; the service its ClientHello picks once it is whole, or not a
    /// hello, or past [`MAX_CLIENT_HELLO`]. `None` while more is needed.
    fn read(&mut self, data: &[u8]) -> Option<Option<Service>> {
        self.buf.extend_from_slice(data);
        let name = match parse_client_hello(&self.buf) {
            Ok(h) => h.server_name().map(str::to_owned),
            Err(HelloError::Incomplete) if self.buf.len() < MAX_CLIENT_HELLO => return None,
            // Not TLS, a hello akio-mux cannot read, or too long: the fallback's.
            Err(_) => None,
        };
        Some(self.entry.pick(name.as_deref()).cloned())
    }

    /// Disarms the opening timeout: false when it closed the connection already.
    fn settle(&self) -> bool {
        self.waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .is_some()
    }
}

/// The handler of a client's connection or UDP session.
#[derive(Default)]
pub struct Client {
    relay: Option<Arc<Relay>>,
    /// Its ClientHello is being read.
    opening: Option<Box<Opening>>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("relay", &self.relay)
            .field("opening", &self.opening.is_some())
            .finish()
    }
}

impl Handler for Client {
    fn on_data(&mut self, c: &Conn<Client>, data: &[u8]) {
        if let Some(o) = &mut self.opening {
            let Some(picked) = o.read(data) else {
                return;
            };
            let Some(o) = self.opening.take() else {
                return;
            };
            if !o.settle() {
                return;
            }
            match picked {
                Some(s) => self.relay = start(&s, c, false, o.buf, o.entry.sockmap()),
                None => c.close(),
            }
            return;
        }
        if let Some(r) = &self.relay {
            r.from_client(data);
        }
    }

    fn on_close(&mut self, _c: &Conn<Client>, _err: Option<akio::Error>) {
        if let Some(o) = self.opening.take() {
            o.settle();
        }
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
    /// The client's address, which the upstream's address is picked by (`ip_hash`).
    peer: IpAddr,
    /// The client's address and port, and where it connected to: a trusted PROXY header's when
    /// it sent one, else the socket's. The PROXY header sent upstream names them.
    client_addr: SocketAddr,
    dst: SocketAddr,
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
    /// The kernel relay to splice the connection with once the upstream's opens: its
    /// listener's `sockmap`, when the relay is loaded (I51b).
    kernel: Option<&'static akio::ebpf::Relay>,
    /// Set once the connection is spliced: what its counters are read against.
    spliced: OnceLock<Spliced>,
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
        // What goes to the upstream first: the service's PROXY header, then what the client sent
        // meanwhile.
        let mut first = Vec::new();
        if !self.udp {
            up.set_relayed(true);
            if let Some(v) = self.s.proxy {
                let h = ProxyHeader::proxy(unmap(self.client_addr), unmap(self.dst));
                if let Some(b) = h.encode(v) {
                    first.push(b);
                }
            }
        }
        let client_read = st.pending;
        first.append(&mut st.datagrams);
        st.pending = 0;
        if let Some(k) = self.kernel {
            // The client's reads are held, and the upstream's have not begun: nothing is in
            // flight in userspace but `first`, which the splice writes ahead of the kernel.
            first = self.splice(k, up, first, client_read);
        }
        for d in first {
            let _ = up.send_owned(d);
        }
        let _ = self.up.set(up.clone());
        drop(st);
        if !self.udp {
            self.client.hold_reads(false);
        }
    }

    /// Splices the connection to `up` with the kernel relay `k`, `first` written to the
    /// upstream ahead of the relayed bytes, `client_read` bytes of it the client's (I51b). What
    /// is left for userspace to send is returned: nothing once spliced, and all of what was
    /// not written when the connection cannot be spliced.
    fn splice(
        &self,
        k: &akio::ebpf::Relay,
        up: &Conn<Upstream>,
        first: Vec<Vec<u8>>,
        client_read: usize,
    ) -> Vec<Vec<u8>> {
        // A PROXY header the listener read: how long it was is not known, which the counters
        // need (akio API request A170).
        if proxy::header(&self.client).is_some() {
            return first;
        }
        let client_read = u64::try_from(client_read).unwrap_or(u64::MAX);
        match sockmap::splice(k, &self.client, up, first.concat(), client_read) {
            Ok(sp) => {
                let _ = self.spliced.set(sp);
                self.s.spliced.fetch_add(1, Ordering::Relaxed);
                Vec::new()
            }
            Err(rest) if rest.is_empty() => Vec::new(),
            Err(rest) => vec![rest],
        }
    }

    /// Whether the connection is spliced in the kernel.
    pub(crate) fn is_spliced(&self) -> bool {
        self.spliced.get().is_some()
    }

    /// A spliced connection's counters read for its idle timeout: it counts as active when
    /// either socket received anything since the last read. Off the I/O threads.
    pub(crate) fn probe(&self) {
        if let (Some(sp), Some(up)) = (self.spliced.get(), self.up.get()) {
            if sp.moved(&self.client, up) {
                self.touch();
            }
        }
    }

    /// Payload read in userspace from a spliced connection, `to` being the other side: while
    /// it is open, a read that completed before the splice took over, which the kernel's newer
    /// bytes may have overtaken: both sides are closed rather than the stream reordered. Once
    /// `to` closed, the kernel hands userspace what has nowhere to go, which is dropped.
    fn spliced_payload(&self, to_open: bool) -> bool {
        if !self.is_spliced() {
            return false;
        }
        if to_open {
            self.abandon();
        }
        true
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
        if self.spliced_payload(up.is_open()) {
            return;
        }
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
        if self.spliced_payload(self.client.is_open()) {
            return;
        }
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

    /// The client closed: the upstream's side ends once what the client sent has gone (from
    /// the kernel's relay too, when spliced).
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
            } else if let Some(sp) = self.spliced.get() {
                sp.close_after_forward(&self.client, true, up, self.s.idle);
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
        } else if let (Some(sp), Some(up)) = (self.spliced.get(), self.up.get()) {
            sp.close_after_forward(up, false, self.client.clone(), self.s.idle);
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

/// `a` with an IPv4-mapped IPv6 address as the IPv4 address it maps, as fit writes PROXY
/// headers.
fn unmap(a: SocketAddr) -> SocketAddr {
    match a.ip() {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map_or(a, |v4| SocketAddr::new(IpAddr::V4(v4), a.port())),
        IpAddr::V4(_) => a,
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
