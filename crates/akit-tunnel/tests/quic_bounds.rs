//! The hub's QUIC listener's handshake bounds (I102d, akio's A245), as fit's
//! `tunnel/internal/transport/quic` tests them: a handshake that stalls is dropped at
//! `handshake_timeout`; past half of `max_handshakes` a new client is asked to Retry, past all of
//! them its Initials wait; a connection whose handshake is over and that opens no link's stream
//! is closed once `handshake_timeout` has passed again (fit's `streamWait`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use akio_quic::{Bytes, Connection, ConnectionError, QuicHandler, QuicServer, StreamId};
use akit_config::roles::TunnelHubQuic;

/// A hub's QUIC server with `quic`'s bounds, on an engine of its own: the server (to count its
/// connections), the engine and its address. Every link's stream is refused.
fn hub_server(quic: &TunnelHubQuic) -> (QuicServer, akio::Engine, SocketAddr) {
    let (cert, key) = akit_tunnel::tls::self_signed(&["localhost".to_owned()]).unwrap();
    let tls = akit_tunnel::tls::server_config(&cert, &key).unwrap();
    let accept: akit_tunnel::quic::Accept = Arc::new(|_, _| None);
    let server = akit_tunnel::quic::server(&tls, Some(quic), 64, accept).unwrap();
    let e = akio::Engine::builder()
        .listen(akio::Listener::udp("127.0.0.1:0".parse().unwrap()).serve(server.clone()))
        .build()
        .unwrap();
    e.start().unwrap();
    let addr = e.local_addrs()[0];
    (server, e, addr)
}

fn client_engine() -> akio::Engine {
    let e = akio::Engine::builder()
        .udp_client_threads(1)
        .build()
        .unwrap();
    e.start().unwrap();
    e
}

fn client_config() -> akio_quic::ClientConfig {
    akio_quic::client_config_insecure(&[akit_tunnel::tls::ALPN.as_bytes()]).unwrap()
}

/// Reports the handshake (`Ok`) or the close (`Err`); opens no stream.
struct Watch(mpsc::Sender<Result<(), ConnectionError>>);

impl QuicHandler for Watch {
    fn on_connected(&mut self, _c: &Connection) {
        let _ = self.0.send(Ok(()));
    }
    fn on_stream_data(&mut self, _c: &Connection, _id: StreamId, _d: Bytes, _fin: bool) {}
    fn on_close(&mut self, _c: &Connection, r: ConnectionError) {
        let _ = self.0.send(Err(r));
    }
}

fn wait_for(what: &str, f: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// What a [`Relay`] passes from the hub back to its client.
#[derive(Clone, Copy, PartialEq)]
enum Down {
    /// Retry packets alone: the client gets into a handshake that then stalls.
    RetryOnly,
    /// Nothing, and only the client's first datagram goes up: a handshake that stalls at once.
    Nothing,
}

/// A UDP relay between one client and the hub, counting the hub's Retry packets.
struct Relay {
    addr: SocketAddr,
    retries: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

fn relay(server: SocketAddr, down: Down) -> Relay {
    let front = UdpSocket::bind("127.0.0.1:0").unwrap();
    let back = UdpSocket::bind("127.0.0.1:0").unwrap();
    back.connect(server).unwrap();
    for s in [&front, &back] {
        // Only so that the threads see `stop`.
        s.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
    }
    let addr = front.local_addr().unwrap();
    let retries = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let client: Arc<Mutex<Option<SocketAddr>>> = Arc::default();
    let (f2, b2, c2, s2) = (
        front.try_clone().unwrap(),
        back.try_clone().unwrap(),
        Arc::clone(&client),
        Arc::clone(&stop),
    );
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 65536];
        let mut up = 0;
        while !s2.load(Ordering::Acquire) {
            if let Ok((n, from)) = f2.recv_from(&mut buf) {
                *c2.lock().unwrap() = Some(from);
                up += 1;
                if down != Down::Nothing || up == 1 {
                    let _ = b2.send(&buf[..n]);
                }
            }
        }
    });
    let (r2, s3) = (Arc::clone(&retries), Arc::clone(&stop));
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 65536];
        while !s3.load(Ordering::Acquire) {
            if let Ok(n) = back.recv(&mut buf) {
                // A version 1 long header of type Retry.
                let retry = n > 0 && buf[0] & 0x80 != 0 && (buf[0] >> 4) & 0x3 == 3;
                if retry {
                    r2.fetch_add(1, Ordering::AcqRel);
                }
                let pass = down == Down::RetryOnly && retry;
                let to = *client.lock().unwrap();
                if let (true, Some(to)) = (pass, to) {
                    let _ = front.send_to(&buf[..n], to);
                }
            }
        }
    });
    Relay {
        addr,
        retries,
        stop,
    }
}

fn bounds(handshake_ms: u64, max_handshakes: usize) -> TunnelHubQuic {
    TunnelHubQuic {
        handshake_timeout: Duration::from_millis(handshake_ms),
        max_handshakes,
        ..TunnelHubQuic::default()
    }
}

/// A handshake that stalls is dropped at the hub's `handshake_timeout`, not at the idle timeout
/// (fit's `fq.Config.HandshakeTimeout`).
#[test]
fn a_stalled_handshake_is_dropped_at_the_timeout() {
    let (server, _e, addr) = hub_server(&bounds(300, 0));
    let c = client_engine();
    let r = relay(addr, Down::Nothing);
    let (tx, _rx) = mpsc::channel();
    let _stuck = akio_quic::dial(&c, r.addr, "localhost", client_config(), Watch(tx)).unwrap();
    wait_for("the stalled handshake", || server.connections() == 1);
    let t = Instant::now();
    wait_for("the stalled handshake to be dropped", || {
        server.connections() == 0
    });
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
}

/// fit's `TestServerIdleConn`: a connection whose handshake is over and that opens no link's
/// stream is closed once `handshake_timeout` has passed again (fit's `streamWait`).
#[test]
fn a_connection_without_its_stream_is_closed() {
    let (server, _e, addr) = hub_server(&bounds(300, 0));
    let c = client_engine();
    let (tx, rx) = mpsc::channel();
    let _idle = akio_quic::dial(&c, addr, "localhost", client_config(), Watch(tx)).unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Ok(()));
    let t = Instant::now();
    let closed = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(closed.is_err(), "{closed:?}");
    assert!(
        t.elapsed() >= Duration::from_millis(200) && t.elapsed() < Duration::from_secs(3),
        "{:?}",
        t.elapsed()
    );
    wait_for("the hub to forget it", || server.connections() == 0);
}

/// fit's `TestListenerRetry`, as fit's `admit`: with `max_handshakes` 2, the first client is let
/// in at once, the second, at half, after a Retry; at the limit a new client's Initials are
/// dropped unanswered (counted as refused) until a handshake is over, and it gets in then.
#[test]
fn handshakes_past_half_retry_and_past_the_limit_wait() {
    let (server, _e, addr) = hub_server(&bounds(1000, 2));
    let c = client_engine();
    let stalled = || {
        let r = relay(addr, Down::RetryOnly);
        let (tx, _rx) = mpsc::channel();
        let conn = akio_quic::dial(&c, r.addr, "localhost", client_config(), Watch(tx)).unwrap();
        (r, conn)
    };
    let (ra, _a) = stalled();
    wait_for("the first handshake", || server.connections() == 1);
    let (rb, _b) = stalled();
    wait_for("the second handshake", || server.connections() == 2);
    assert_eq!(ra.retries.load(Ordering::Acquire), 0);
    assert!(rb.retries.load(Ordering::Acquire) >= 1);
    drop((ra, rb));
    let (tx, rx) = mpsc::channel();
    let t = Instant::now();
    let _c = akio_quic::dial(&c, addr, "localhost", client_config(), Watch(tx)).unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(15)).unwrap(), Ok(()));
    assert!(
        t.elapsed() >= Duration::from_millis(500),
        "{:?}",
        t.elapsed()
    );
    assert!(server.refused_connections() >= 1);
}

/// Left out, the bounds are fit's: a handshake has 10 s, and 64 may be under way (32 before a
/// Retry).
#[test]
fn the_defaults_are_fits() {
    assert_eq!(TunnelHubQuic::handshake(None), Duration::from_secs(10));
    assert_eq!(TunnelHubQuic::handshakes(None), 64);
    let q = TunnelHubQuic::default();
    assert_eq!(TunnelHubQuic::handshake(Some(&q)), Duration::from_secs(10));
    assert_eq!(TunnelHubQuic::handshakes(Some(&q)), 64);
}
