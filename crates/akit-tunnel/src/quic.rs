//! QUIC links (I102b, fit's `tunnel/internal/transport/quic`): a link is one bidirectional stream
//! of a QUIC connection of its own, `quic://host:port`. The dialer opens the stream and sends one
//! byte, [`STREAM_VERSION`], before its data, which makes the stream known to the hub before the
//! dialer has anything else to say; the hub strips it. Nothing else is added: what one side writes
//! is what the other reads. The TLS is the TLS links' (ALPN `fit-tunnel`, TLS 1.3), the hub's
//! certificate checked the same way, a pin once the handshake is done (akio API request A235).
//!
//! Everything runs on akio-quic's callbacks. A stream's send buffer is the link's high water
//! ([`HIGH_WATER`]); what the link sends past it waits in the wire's own queue, bounded by
//! [`OVERFLOW`], until akio-quic says the stream has room again (`on_stream_writable`), and the
//! feeders a congested link holds resume then. Holding a link's reads holds its stream's, whose
//! flow control window then stops the peer.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use akio_crypto::hash::Sha256;
use akio_quic::{Connection, Dir, Side, StreamId, TransportConfig};

use crate::crypt::Coder;
use crate::link::{HIGH_WATER, Link, Router, Wire};

/// The byte the dialer sends first on its stream (fit's `streamVersion`).
pub const STREAM_VERSION: u8 = 1;

/// What a link may queue past its stream's send buffer before a send fails: what the TCP links'
/// connections take past [`HIGH_WATER`] (akit's default `limits.max_write_queue_bytes`, 32 MiB).
pub const OVERFLOW: usize = 24 << 20;

/// fit's code for a refused stream.
const CODE_REFUSED: u32 = 1;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The transport of a link's QUIC connection: idle past `idle`, pinged every `ping`, the stream's
/// send buffer the link's high water.
#[must_use]
pub fn transport(idle: Duration, ping: Duration) -> TransportConfig {
    let mut t = TransportConfig::default();
    t.max_idle_timeout = Some(idle);
    t.keep_alive_interval = Some(ping);
    t.max_concurrent_bidi_streams = 1;
    t.max_concurrent_uni_streams = 0;
    t.stream_send_buffer = HIGH_WATER as u64;
    t
}

#[derive(Default)]
struct WireState {
    pending: VecDeque<Vec<u8>>,
    pending_bytes: usize,
    waiters: Vec<Box<dyn FnOnce() + Send>>,
    closed: bool,
}

/// A link's stream.
pub struct QuicWire {
    c: Connection,
    id: StreamId,
    st: Mutex<WireState>,
}

impl std::fmt::Debug for QuicWire {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuicWire")
            .field("peer", &self.c.remote_addr())
            .finish_non_exhaustive()
    }
}

impl QuicWire {
    fn new(c: &Connection, id: StreamId) -> QuicWire {
        QuicWire {
            c: c.clone(),
            id,
            st: Mutex::new(WireState::default()),
        }
    }

    /// The stream has room again: what waits goes out, and once nothing does, the feeders resume.
    fn writable(&self) {
        let waiters = {
            let mut st = lock(&self.st);
            while let Some(b) = st.pending.pop_front() {
                let n = b.len();
                match self.c.send(self.id, b.clone()) {
                    Ok(()) => st.pending_bytes = st.pending_bytes.saturating_sub(n),
                    Err(akio_quic::Error::Overloaded) => {
                        st.pending.push_front(b);
                        return;
                    }
                    Err(_) => {
                        st.pending.clear();
                        st.pending_bytes = 0;
                        st.closed = true;
                        break;
                    }
                }
            }
            std::mem::take(&mut st.waiters)
        };
        for w in waiters {
            w();
        }
    }
}

impl Wire for QuicWire {
    fn send(&self, b: &[u8]) -> bool {
        // Not driven here: a send from outside the connection's handler would otherwise run the
        // handler on this thread, under this lock or the link's.
        self.c.without_driving(|| {
            let mut st = lock(&self.st);
            if st.closed {
                return false;
            }
            if st.pending.is_empty() {
                match self.c.send(self.id, b.to_vec()) {
                    Ok(()) => return true,
                    Err(akio_quic::Error::Overloaded) => {}
                    Err(_) => {
                        st.closed = true;
                        return false;
                    }
                }
            }
            let n = st.pending_bytes.saturating_add(b.len());
            if n > OVERFLOW {
                return false;
            }
            st.pending_bytes = n;
            st.pending.push_back(b.to_vec());
            true
        })
    }
    fn close(&self) {
        lock(&self.st).closed = true;
        self.c.without_driving(|| self.c.close(0, b""));
    }
    fn close_after_send(&self) {
        // The stream ends once what it holds is out; the peer's link closes on its end and the
        // connection with it, or here, a little later.
        self.c.without_driving(|| {
            let _ = self.c.finish(self.id);
        });
        let c = self.c.clone();
        let _ = akio::timer::after_send(Duration::from_secs(2), move || c.close(0, b""));
    }
    fn peer(&self) -> String {
        self.c.remote_addr().to_string()
    }
    fn hold_reads(&self, hold: bool) {
        self.c.without_driving(|| {
            let _ = self.c.hold_reads(self.id, hold);
        });
    }
    fn queued(&self) -> usize {
        let pending = lock(&self.st).pending_bytes;
        self.c.queued_bytes(self.id).saturating_add(pending)
    }
    fn on_drain(&self, f: Box<dyn FnOnce() + Send>) {
        {
            let mut st = lock(&self.st);
            // Something waits for room only after a refusal, which akio-quic answers with
            // `on_stream_writable`; with nothing waiting the queue is the stream's alone, under
            // the high water.
            if !st.pending.is_empty() && !st.closed {
                st.waiters.push(f);
                return;
            }
        }
        f();
    }
}

/// How a hub takes a link a QUIC connection opened: its peer and wire, the link or `None` to
/// refuse it.
pub type Accept = Arc<dyn Fn(SocketAddr, Arc<dyn Wire>) -> Option<Arc<Link>> + Send + Sync>;

/// What a dial ends in: the link, started, or why not.
pub type Dialed = Box<dyn FnOnce(Result<Arc<Link>, String>) + Send>;

type Slot = Arc<Mutex<Option<Dialed>>>;

enum Role {
    Dialer {
        pin: Option<[u8; 32]>,
        router: Arc<dyn Router>,
        coder: Option<Coder>,
        done: Slot,
    },
    Hub {
        accept: Accept,
    },
}

/// A link's QUIC connection.
pub struct QuicLink {
    role: Role,
    stream: Option<StreamId>,
    version: bool,
    wire: Option<Arc<QuicWire>>,
    link: Option<Arc<Link>>,
}

impl std::fmt::Debug for QuicLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuicLink").finish_non_exhaustive()
    }
}

fn finish(slot: &Slot, r: Result<Arc<Link>, String>) -> bool {
    match lock(slot).take() {
        Some(done) => {
            done(r);
            true
        }
        None => false,
    }
}

impl QuicLink {
    /// A hub's side of a connection.
    #[must_use]
    pub fn hub(accept: Accept) -> QuicLink {
        QuicLink {
            role: Role::Hub { accept },
            stream: None,
            version: false,
            wire: None,
            link: None,
        }
    }

    fn refuse(c: &Connection, id: StreamId) {
        let _ = c.stop(id, CODE_REFUSED);
        let _ = c.reset(id, CODE_REFUSED);
    }

    fn dialer_connected(&mut self, c: &Connection) -> Result<Arc<Link>, String> {
        let Role::Dialer {
            pin, router, coder, ..
        } = &self.role
        else {
            return Err("tunnel: not a dialer".to_owned());
        };
        let certs = c.peer_certificates().unwrap_or_default();
        let Some(leaf) = certs.first() else {
            return Err("tunnel: the hub sent no certificate".to_owned());
        };
        if let Some(pin) = pin {
            if !akio_crypto::ct_eq(&Sha256::digest(leaf), pin) {
                return Err("tunnel: the hub's certificate does not match the pin".to_owned());
            }
        }
        let id = c
            .open_bi()
            .ok_or_else(|| "tunnel: the hub lets this side open no stream".to_owned())?;
        c.send(id, vec![STREAM_VERSION])
            .map_err(|e| format!("tunnel: {e}"))?;
        let wire = Arc::new(QuicWire::new(c, id));
        self.stream = Some(id);
        self.wire = Some(Arc::clone(&wire));
        let l = Link::new(wire, Arc::clone(router), coder.clone());
        self.link = Some(Arc::clone(&l));
        Ok(l)
    }
}

impl akio_quic::QuicHandler for QuicLink {
    fn on_connected(&mut self, c: &Connection) {
        let Role::Dialer { done, .. } = &self.role else {
            return;
        };
        let done = Arc::clone(done);
        match self.dialer_connected(c) {
            Ok(l) => {
                if !finish(&done, Ok(Arc::clone(&l))) {
                    l.close();
                }
            }
            Err(e) => {
                finish(&done, Err(e));
                c.close(CODE_REFUSED, b"");
            }
        }
    }

    fn on_stream_data(&mut self, c: &Connection, id: StreamId, data: akio_quic::Bytes, fin: bool) {
        let mut data: &[u8] = &data;
        if self.stream != Some(id) {
            let Role::Hub { accept } = &self.role else {
                return Self::refuse(c, id);
            };
            if self.stream.is_some() || id.dir() != Dir::Bi || id.initiator() != Side::Client {
                return Self::refuse(c, id);
            }
            self.stream = Some(id);
            self.version = true;
            let wire = Arc::new(QuicWire::new(c, id));
            self.wire = Some(Arc::clone(&wire));
            match accept(c.remote_addr(), wire) {
                Some(l) => self.link = Some(l),
                None => {
                    c.close(CODE_REFUSED, b"");
                    return;
                }
            }
        }
        if self.version {
            let Some((&v, rest)) = data.split_first() else {
                if fin {
                    c.close(0, b"");
                }
                return;
            };
            if v != STREAM_VERSION {
                if let Some(l) = &self.link {
                    l.on_closed(Some(format!(
                        "quic: stream version {v}, not {STREAM_VERSION}"
                    )));
                }
                c.close(CODE_REFUSED, b"");
                return;
            }
            self.version = false;
            data = rest;
        }
        if let Some(l) = &self.link {
            if !data.is_empty() {
                l.on_data(data);
            }
            if fin {
                l.on_closed(Some("the peer ended the link".to_owned()));
                c.close(0, b"");
            }
        }
    }

    fn on_stream_reset(&mut self, c: &Connection, id: StreamId, code: u64) {
        if self.stream == Some(id) {
            if let Some(l) = &self.link {
                l.on_closed(Some(format!("quic: stream reset by peer, code {code}")));
            }
            c.close(0, b"");
        }
    }

    fn on_stream_writable(&mut self, _c: &Connection, id: StreamId) {
        if self.stream == Some(id) {
            if let Some(w) = &self.wire {
                w.writable();
            }
        }
    }

    fn on_close(&mut self, _c: &Connection, reason: akio_quic::ConnectionError) {
        let why = format!("quic: {reason}");
        if let Some(w) = &self.wire {
            lock(&w.st).closed = true;
        }
        if let Role::Dialer { done, .. } = &self.role {
            finish(done, Err(why.clone()));
        }
        if let Some(l) = &self.link {
            l.on_closed(Some(why));
        }
    }
}

/// Dials a hub's `quic://` endpoint `host:port`, checking its certificate with `tls` and `pin`;
/// `done` hears the link, started, or why not, within `within`. Resolves a name on akio's
/// resolver thread.
#[allow(clippy::too_many_arguments)]
pub fn dial(
    engine: &Arc<akio::Engine>,
    host: &str,
    port: u16,
    tls: &akio::tls::ClientConfig,
    pin: Option<[u8; 32]>,
    within: Duration,
    router: Arc<dyn Router>,
    coder: Option<Coder>,
    done: Dialed,
) {
    let cfg = match akio_quic::client_config_from(tls) {
        Ok(c) => c.transport(&transport(
            akit_config::roles::TunnelHubQuic::idle(None),
            akit_config::roles::TunnelHubQuic::ping(None),
        )),
        Err(e) => return done(Err(format!("tunnel: {e}"))),
    };
    let slot: Slot = Arc::new(Mutex::new(Some(done)));
    let engine = Arc::clone(engine);
    let name = host.to_owned();
    let slot2 = Arc::clone(&slot);
    akio::dns::resolve(host, port, move |r| {
        let addr = match r {
            Ok(a) => match a.first() {
                Some(a) => *a,
                None => {
                    finish(&slot2, Err(format!("tunnel: {name}: no address")));
                    return;
                }
            },
            Err(e) => {
                finish(&slot2, Err(format!("tunnel: {name}: {e}")));
                return;
            }
        };
        let handler = QuicLink {
            role: Role::Dialer {
                pin,
                router,
                coder,
                done: Arc::clone(&slot2),
            },
            stream: None,
            version: false,
            wire: None,
            link: None,
        };
        match akio_quic::dial(&engine, addr, &name, cfg, handler) {
            Ok(c) => {
                let slot3 = Arc::clone(&slot2);
                let _ = akio::timer::after_send(within, move || {
                    if finish(
                        &slot3,
                        Err("tunnel: the QUIC handshake timed out".to_owned()),
                    ) {
                        c.close(0, b"");
                    }
                });
            }
            Err(e) => {
                finish(&slot2, Err(format!("tunnel: {addr}: {e}")));
            }
        }
    });
    drop(slot);
}

/// A hub's QUIC listener's server: `tls` with the transport of `quic`, at most `max` connections,
/// each link taken by `accept`.
///
/// # Errors
///
/// A TLS configuration akio-quic refuses.
pub fn server(
    tls: &akio::tls::ServerConfig,
    quic: Option<&akit_config::roles::TunnelHubQuic>,
    max: usize,
    accept: Accept,
) -> Result<akio_quic::QuicServer, String> {
    let cfg = akio_quic::server_config_from(tls)
        .map_err(|e| e.to_string())?
        .transport(&transport(
            akit_config::roles::TunnelHubQuic::idle(quic),
            akit_config::roles::TunnelHubQuic::ping(quic),
        ))
        .max_connections(max);
    Ok(akio_quic::QuicServer::new(cfg, move |_c: &Connection| {
        QuicLink::hub(Arc::clone(&accept))
    }))
}
