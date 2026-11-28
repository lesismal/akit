//! Layer 4 services (I51; fit's `l4/l4_test.go`, `l4/udp_test.go` and `l4/cover_test.go`): `tcp`
//! listeners whose connections, and `udp` listeners whose sessions, akit-l4 proxies to an
//! upstream, with backpressure both ways, idle timeouts, `max_conns`, retries on the next
//! address, and the drain of a reload. On the backend `AKIO_TEST_BACKEND` names (epoll by
//! default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation
)]

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, UringConfig};
use akit_listener::Listeners;
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, Graph, GraphCell, HandlerParams, Registry, compile};

/// The tests that open hundreds of sockets (`tcp_echo`, `udp_echo_sessions`) run alone: with
/// every test's engine at once, they would pass a soft limit of 1,024 open files.
static FILES: RwLock<()> = RwLock::new(());

fn shared() -> RwLockReadGuard<'static, ()> {
    FILES.read().unwrap_or_else(PoisonError::into_inner)
}

fn alone() -> RwLockWriteGuard<'static, ()> {
    FILES.write().unwrap_or_else(PoisonError::into_inner)
}

fn options(drain: Duration) -> akit_listener::Options {
    let mut o = akit_listener::Options::default();
    o.backend = Some(match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    });
    o.shutdown_timeout = drain;
    o
}

/// A config with a `tcp` (or `udp`) listener whose service, with `params`, proxies to the
/// upstream at `addrs`; fit's `tcpConfig` and `udpConfig`.
fn config(proto: &str, params: &str, addrs: &[&str]) -> String {
    let quoted: Vec<String> = addrs.iter().map(|a| format!("{a:?}")).collect();
    format!(
        r#"{{
  "listeners": [{{"name": "l4", "address": "127.0.0.1:0", "protocols": ["{proto}"]}}],
  "upstreams": [{{"name": "app", "addresses": [{}]}}],
  "services": [{{"name": "svc", "listeners": ["l4"], "upstream": "app"{params}}}]
}}"#,
        quoted.join(", ")
    )
}

fn tcp_config(params: &str, addrs: &[&str]) -> String {
    config("tcp", params, addrs)
}

fn udp_config(params: &str, addrs: &[&str]) -> String {
    config("udp", params, addrs)
}

/// akit serving a config, as `akit run` does: the graph with its pools, then the engine, then
/// the slot the pools dial through filled.
struct Akit {
    cell: GraphCell,
    engine: Arc<akio::Engine>,
    listeners: Option<Listeners>,
    slot: EngineSlot,
    addr: SocketAddr,
}

/// The config's graph, its akit on two I/O threads, not one a core: the tests run at once, and
/// each thread holds an epoll or io_uring and an eventfd. A reload's graph says the same, though a
/// reload keeps the threads akit started with.
fn graph(json: &str, slot: &EngineSlot) -> Graph {
    let mut config = akit_config::from_json(json, None).unwrap();
    config.engine.event_loops = 2;
    let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
    compile(&config, &registry, &Env::new(slot.clone())).unwrap()
}

impl Akit {
    fn start(json: &str) -> Akit {
        Akit::start_with(json, Duration::from_secs(5), true)
    }

    /// `drain`: the drain timeout of a reload; `fill`: whether the pools get the engine.
    fn start_with(json: &str, drain: Duration, fill: bool) -> Akit {
        let slot = EngineSlot::new();
        let cell = GraphCell::new(graph(json, &slot));
        let opts = options(drain);
        let engine = Arc::new(akit_listener::engine(&cell, &opts).unwrap());
        engine.start().unwrap();
        if fill {
            assert!(slot.set(engine.clone()));
        }
        let listeners = Listeners::new(Arc::clone(&engine), cell.clone(), &opts);
        let addr = akit_listener::addrs(&cell.with(|g| g.listeners().to_vec()), &engine)[0]
            .addr()
            .unwrap();
        Akit {
            cell,
            engine,
            listeners: Some(listeners),
            slot,
            addr,
        }
    }

    /// Replaces the config with `json`, as a reload does.
    fn reload(&mut self, json: &str) {
        let g = graph(json, &self.slot);
        self.listeners.as_mut().unwrap().reload(g).unwrap();
        self.cell.collect();
    }

    /// How many connections and sessions the service proxies now (fit's `conns`).
    fn conns(&self) -> usize {
        self.cell.with(|g| g.services()[0].conns())
    }

    /// Waits for the service to proxy `want`.
    fn wait_conns(&self, want: usize, within: Duration) {
        let deadline = Instant::now() + within;
        while self.conns() != want {
            assert!(
                Instant::now() < deadline,
                "the service has {} connections, want {want}",
                self.conns()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// A client of the `tcp` listener.
    fn dial(&self) -> TcpStream {
        TcpStream::connect_timeout(&self.addr, Duration::from_secs(5)).unwrap()
    }

    /// A client of the `udp` listener.
    fn dial_udp(&self) -> UdpSocket {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        s.connect(self.addr).unwrap();
        s
    }
}

impl Drop for Akit {
    fn drop(&mut self) {
        drop(self.listeners.take());
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
    }
}

/// A TCP server serving each connection with `serve`, on a thread of its own, until the test
/// ends (its threads are left to end with their connections).
fn backend(serve: impl Fn(TcpStream) + Send + Sync + 'static) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let serve = Arc::new(serve);
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            let serve = Arc::clone(&serve);
            thread::spawn(move || serve(s));
        }
    });
    addr
}

fn echo(mut s: TcpStream) {
    let mut r = s.try_clone().unwrap();
    let _ = std::io::copy(&mut r, &mut s);
}

/// An address nothing listens on.
fn refused() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().to_string()
}

/// `n` bytes that are not all alike, seeded by `seed`.
fn pattern(n: usize, seed: u8) -> Vec<u8> {
    (0..n).map(|i| (i.wrapping_mul(31) as u8) ^ seed).collect()
}

/// Checks that akit closes `c` within `d`: its read ends without data.
fn expect_closed(c: &mut TcpStream, d: Duration) {
    c.set_read_timeout(Some(d)).unwrap();
    match c.read(&mut [0u8; 1]) {
        Ok(0) => {}
        Err(e) if !matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
        r => panic!("read {r:?}; want the connection closed"),
    }
}

fn round_trip(c: &mut TcpStream, data: &[u8]) -> std::io::Result<Vec<u8>> {
    c.write_all(data)?;
    c.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut got = vec![0u8; data.len()];
    c.read_exact(&mut got)?;
    Ok(got)
}

/// Proxies connections to an echo server: one that says hello, and 50 at once with a little
/// more each.
#[test]
fn tcp_echo() {
    let _files = alone();
    let a = Akit::start(&tcp_config("", &[&backend(echo)]));
    let mut c = a.dial();
    assert_eq!(round_trip(&mut c, b"hello").unwrap(), b"hello");
    let threads: Vec<_> = (0..50u8)
        .map(|i| {
            let mut c = a.dial();
            thread::spawn(move || {
                let want = pattern(10_000 + usize::from(i), i);
                assert_eq!(round_trip(&mut c, &want).unwrap(), want, "connection {i}");
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
}

/// Sends 32 MB each way at once.
#[test]
fn tcp_large() {
    let _files = shared();
    const SIZE: usize = 32 << 20;
    let a = Akit::start(&tcp_config("", &[&backend(echo)]));
    let mut c = a.dial();
    let want = pattern(SIZE, 7);
    let mut w = c.try_clone().unwrap();
    let sent = want.clone();
    thread::spawn(move || {
        let _ = w.write_all(&sent);
    });
    c.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut got = vec![0u8; SIZE];
    c.read_exact(&mut got).unwrap();
    assert!(got == want, "the {SIZE} bytes sent came back otherwise");
}

/// A client stops reading while its upstream sends: akit holds the upstream back rather than
/// taking what it sends, and the client then gets all of it.
#[test]
fn tcp_slow_client() {
    let _files = shared();
    const SIZE: usize = 64 << 20;
    let data = Arc::new(pattern(SIZE, 3));
    let sent = Arc::new(AtomicUsize::new(0));
    let (d, n) = (Arc::clone(&data), Arc::clone(&sent));
    let up = backend(move |mut s| {
        // The client's word read first: a socket closed with input unread resets.
        if s.read_exact(&mut [0u8; 2]).is_err() {
            return;
        }
        for chunk in d.chunks(64 << 10) {
            if s.write_all(chunk).is_err() {
                return;
            }
            n.fetch_add(chunk.len(), Ordering::SeqCst);
        }
    });
    let a = Akit::start(&tcp_config("", &[&up]));
    let mut c = a.dial();
    c.write_all(b"go").unwrap();
    thread::sleep(Duration::from_secs(1));
    let n = sent.load(Ordering::SeqCst);
    assert!(
        n < SIZE / 2,
        "the upstream sent {n} bytes to a client that read none; want them held back"
    );
    c.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut got = vec![0u8; SIZE];
    c.read_exact(&mut got).unwrap();
    assert!(got == *data);
}

/// An upstream stops reading while its client sends: akit holds the client back, and the
/// upstream then gets all of it.
#[test]
fn tcp_slow_upstream() {
    let _files = shared();
    const SIZE: usize = 64 << 20;
    let data = Arc::new(pattern(SIZE, 5));
    let (start_tx, start_rx) = channel::<()>();
    let start_rx = Mutex::new(Some(start_rx));
    let (read_tx, read_rx) = channel();
    let d = Arc::clone(&data);
    let up = backend(move |mut s| {
        let Some(rx) = start_rx.lock().unwrap().take() else {
            return;
        };
        let _ = rx.recv();
        let mut got = vec![0u8; SIZE];
        let r = s.read_exact(&mut got).map(|()| got == *d);
        let _ = read_tx.send(r);
    });
    let a = Akit::start(&tcp_config("", &[&up]));
    let c = a.dial();
    let sent = Arc::new(AtomicUsize::new(0));
    let (n, d) = (Arc::clone(&sent), Arc::clone(&data));
    let mut w = c.try_clone().unwrap();
    thread::spawn(move || {
        for chunk in d.chunks(64 << 10) {
            if w.write_all(chunk).is_err() {
                return;
            }
            n.fetch_add(chunk.len(), Ordering::SeqCst);
        }
    });
    thread::sleep(Duration::from_secs(1));
    let n = sent.load(Ordering::SeqCst);
    assert!(
        n < SIZE / 2,
        "the client sent {n} bytes to an upstream that read none; want them held back"
    );
    start_tx.send(()).unwrap();
    let r = read_rx.recv_timeout(Duration::from_secs(30)).unwrap();
    assert!(r.unwrap(), "the upstream got other bytes than those sent");
    drop(c);
}

/// One side ends when the other does: the client gets what the upstream sent before it closed,
/// and the upstream hears the client close, after what it sent.
#[test]
fn tcp_hang_up() {
    let _files = shared();
    // The upstream.
    let a = Akit::start(&tcp_config(
        "",
        &[&backend(|mut s| {
            let _ = s.write_all(&pattern(1 << 20, 9));
        })],
    ));
    let mut c = a.dial();
    c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut got = Vec::new();
    c.read_to_end(&mut got).unwrap();
    assert!(got == pattern(1 << 20, 9), "read {} bytes", got.len());
    drop(a);

    // The client, at once and a little later.
    for late in [false, true] {
        let (tx, rx) = channel();
        let tx = Mutex::new(tx);
        let a = Akit::start(&tcp_config(
            "",
            &[&backend(move |mut s| {
                s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                let mut got = Vec::new();
                let r = s.read_to_end(&mut got).map(|_| got);
                let _ = tx.lock().unwrap().send(r);
            })],
        ));
        let mut c = a.dial();
        c.write_all(b"last words").unwrap();
        if late {
            thread::sleep(Duration::from_millis(300));
        }
        drop(c);
        let got = rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
        assert_eq!(got, b"last words", "late: {late}");
    }
}

/// A client whose upstream refuses is closed, and the next address is tried when there is one.
#[test]
fn tcp_connect_failure() {
    let _files = shared();
    let a = Akit::start(&tcp_config("", &[&refused()]));
    expect_closed(&mut a.dial(), Duration::from_secs(5));
    drop(a);

    let a = Akit::start(&tcp_config("", &[&refused(), &backend(echo)]));
    for _ in 0..4 {
        let mut c = a.dial();
        assert_eq!(
            round_trip(&mut c, b"hi").unwrap(),
            b"hi",
            "want the live address's echo"
        );
    }
}

/// A client whose upstream cannot be dialed at all, the engine the pools dial through never
/// given to them, is closed (fit's `TestTCPEngineStopped`).
#[test]
fn tcp_no_engine() {
    let _files = shared();
    let a = Akit::start_with(
        &tcp_config("", &[&backend(echo)]),
        Duration::from_secs(5),
        false,
    );
    expect_closed(&mut a.dial(), Duration::from_secs(5));
    a.wait_conns(0, Duration::from_secs(5));
}

/// A connection that carries nothing for the service's `idle_timeout` is closed, and one that
/// keeps carrying something is not.
#[test]
fn tcp_idle_timeout() {
    let _files = shared();
    let a = Akit::start(&tcp_config(
        r#", "idle_timeout": "300ms""#,
        &[&backend(echo)],
    ));
    let (mut busy, mut idle) = (a.dial(), a.dial());
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(900) {
        round_trip(&mut busy, b"x")
            .unwrap_or_else(|e| panic!("a busy connection after {:?}: {e}", start.elapsed()));
        thread::sleep(Duration::from_millis(100));
    }
    expect_closed(&mut idle, Duration::from_secs(1));
    a.wait_conns(1, Duration::from_secs(1));
}

/// A relay that carries data one way only is not idle, whichever way it goes: a download (the
/// client never sends) and an upload (the client never receives) outlive the idle timeout
/// many times over, as each side's own data counts (akio's per-connection idle timeout with
/// writes counted, A78); both close once quiet.
#[test]
fn tcp_one_way_is_not_idle() {
    let _files = shared();
    // On "d", sends a byte every 50 ms until the client goes; anything else is read and dropped.
    let up = backend(|mut s| {
        let mut b = [0u8; 64];
        loop {
            match s.read(&mut b) {
                Ok(0) | Err(_) => return,
                Ok(n) if &b[..n] == b"d" => {
                    while s.write_all(b".").is_ok() {
                        thread::sleep(Duration::from_millis(50));
                    }
                    return;
                }
                Ok(_) => {}
            }
        }
    });
    let a = Akit::start(&tcp_config(r#", "idle_timeout": "300ms""#, &[&up]));
    let mut down = a.dial();
    down.write_all(b"d").unwrap();
    down.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    let mut up_only = a.dial();
    let start = Instant::now();
    let mut b = [0u8; 1];
    while start.elapsed() < Duration::from_millis(1200) {
        down.read_exact(&mut b)
            .unwrap_or_else(|e| panic!("the download after {:?}: {e}", start.elapsed()));
        up_only
            .write_all(b"u")
            .unwrap_or_else(|e| panic!("the upload after {:?}: {e}", start.elapsed()));
    }
    assert_eq!(a.conns(), 2);
    drop(down);
    // Quiet now: closed within the idle timeout and an eighth of it (writes are sampled).
    expect_closed(&mut up_only, Duration::from_secs(2));
    a.wait_conns(0, Duration::from_secs(2));
}

/// A connection past the service's `max_conns` is closed at once, and another taken once one
/// has ended.
#[test]
fn tcp_max_conns() {
    let _files = shared();
    let a = Akit::start(&tcp_config(r#", "max_conns": 2"#, &[&backend(echo)]));
    let (mut x, mut y) = (a.dial(), a.dial());
    round_trip(&mut x, b"ok").unwrap();
    round_trip(&mut y, b"ok").unwrap();
    expect_closed(&mut a.dial(), Duration::from_secs(5));
    drop(x);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if round_trip(&mut a.dial(), b"ok").is_ok() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no connection was taken once one had ended"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

/// A `tcp` listener kept across a reload: a connection made before goes on with the old service
/// until the drain timeout closes it, and one made after goes to the new service's upstream.
#[test]
fn tcp_reload() {
    let _files = shared();
    fn greet(word: &'static str) -> String {
        backend(move |mut s| {
            let _ = s.write_all(word.as_bytes());
            echo(s);
        })
    }
    let mut a = Akit::start_with(
        &tcp_config("", &[&greet("old")]),
        Duration::from_millis(500),
        true,
    );
    let mut old = a.dial();
    old.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut got = [0u8; 3];
    old.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"old");
    a.reload(&tcp_config("", &[&greet("new")]));
    let mut c = a.dial();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    c.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"new", "after the reload");
    assert_eq!(round_trip(&mut old, b"abc").unwrap(), b"abc", "the old one");
    expect_closed(&mut old, Duration::from_secs(5));
}

/// What `udp_config`'s upstream heard: how many datagrams from each address.
type Peers = Arc<Mutex<HashMap<SocketAddr, usize>>>;

/// A UDP server answering each datagram with what `answer` makes of it and of its source.
fn udp_backend(
    answer: impl Fn(SocketAddr, &[u8]) -> Option<Vec<u8>> + Send + 'static,
) -> (String, Peers) {
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = s.local_addr().unwrap().to_string();
    let peers: Peers = Arc::default();
    let p = Arc::clone(&peers);
    thread::spawn(move || {
        let mut buf = vec![0u8; 65536];
        loop {
            let Ok((n, from)) = s.recv_from(&mut buf) else {
                return;
            };
            *p.lock().unwrap().entry(from).or_default() += 1;
            if let Some(reply) = answer(from, &buf[..n]) {
                let _ = s.send_to(&reply, from);
            }
        }
    });
    (addr, peers)
}

fn udp_echo(_: SocketAddr, data: &[u8]) -> Option<Vec<u8>> {
    Some(data.to_vec())
}

/// Answers with the address the datagram came from: the session's own socket in akit.
fn udp_who_am_i(from: SocketAddr, _: &[u8]) -> Option<Vec<u8>> {
    Some(from.to_string().into_bytes())
}

/// A UDP address nothing listens on.
fn refused_udp() -> String {
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    s.local_addr().unwrap().to_string()
}

/// Sends `data` and returns the answer, sending again, up to five times, when none comes within
/// `wait`: UDP may drop a datagram.
fn exchange(c: &UdpSocket, data: &[u8], wait: Duration) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0u8; 65536];
    let mut last = None;
    for _ in 0..5 {
        c.send(data)?;
        c.set_read_timeout(Some(wait))?;
        match c.recv(&mut buf) {
            Ok(n) => return Ok(buf[..n].to_vec()),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                last = Some(e);
            }
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap())
}

/// The largest datagram sent: the BSDs, macOS among them, refuse to send one past
/// `net.inet.udp.maxdgram`, 9216 bytes by default.
const MAX_DATAGRAM: usize = if cfg!(target_os = "macos") {
    9000
} else {
    60_000
};

/// Datagrams proxied to an echo server: each answered as one datagram, whatever its size, and
/// 200 clients at once, each a session of its own, which the upstream sees as one address
/// however much it sends.
#[test]
fn udp_echo_sessions() {
    let _files = alone();
    let (up, peers) = udp_backend(udp_echo);
    let a = Akit::start(&udp_config("", &[&up]));
    let c = a.dial_udp();
    for size in [1, 1400, MAX_DATAGRAM] {
        let want = pattern(size, size as u8);
        let got = exchange(&c, &want, Duration::from_secs(1)).unwrap();
        assert!(
            got == want,
            "a datagram of {size} bytes came back as {}",
            got.len()
        );
    }
    const CLIENTS: usize = 200;
    let threads: Vec<_> = (0..CLIENTS)
        .map(|i| {
            let c = a.dial_udp();
            thread::spawn(move || {
                for r in 0..10 {
                    let want = format!("client {i} round {r}").into_bytes();
                    let got = exchange(&c, &want, Duration::from_secs(1));
                    assert_eq!(got.unwrap(), want, "client {i} round {r}");
                }
                c
            })
        })
        .collect();
    let clients: Vec<UdpSocket> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(
        peers.lock().unwrap().len(),
        CLIENTS + 1,
        "the upstream's peers"
    );
    assert_eq!(a.conns(), CLIENTS + 1, "the service's sessions");
    drop(clients);
}

/// Slow peers over UDP (I56, M3's exit): an upstream that answers one session late holds no
/// other session back, and a client that reads nothing while its upstream floods it costs akit
/// only the datagrams the kernel keeps for it: the session still carries the client's next
/// datagram and its answer.
#[test]
fn udp_slow_peers() {
    let _files = shared();
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    let up = s.local_addr().unwrap().to_string();
    thread::spawn(move || {
        let mut buf = vec![0u8; 65536];
        loop {
            let Ok((n, from)) = s.recv_from(&mut buf) else {
                return;
            };
            match &buf[..n] {
                b"slow" => {
                    let s = s.try_clone().unwrap();
                    thread::spawn(move || {
                        thread::sleep(Duration::from_millis(600));
                        let _ = s.send_to(b"late", from);
                    });
                }
                b"flood" => {
                    let chunk = vec![b'f'; 1400];
                    for _ in 0..20_000 {
                        let _ = s.send_to(&chunk, from);
                    }
                }
                d => {
                    let _ = s.send_to(d, from);
                }
            }
        }
    });
    let a = Akit::start(&udp_config("", &[&up]));

    // The slow session's answer comes late; another session's meanwhile at once.
    let slow = a.dial_udp();
    let fast = a.dial_udp();
    let start = Instant::now();
    slow.send(b"slow").unwrap();
    thread::sleep(Duration::from_millis(50));
    assert_eq!(
        exchange(&fast, b"fast", Duration::from_millis(200)).unwrap(),
        b"fast"
    );
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "another session waited {:?} on the slow one",
        start.elapsed()
    );
    let mut buf = vec![0u8; 65536];
    slow.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let n = slow.recv(&mut buf).unwrap();
    assert_eq!(&buf[..n], b"late");
    assert!(start.elapsed() >= Duration::from_millis(500));

    // A client that reads nothing while its upstream floods it.
    let c = a.dial_udp();
    c.send(b"flood").unwrap();
    thread::sleep(Duration::from_secs(1));
    c.set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let mut flooded = 0usize;
    while let Ok(n) = c.recv(&mut buf) {
        assert_eq!(n, 1400, "a datagram of the flood");
        flooded += 1;
    }
    assert!(flooded > 0, "none of the flood came");
    assert_eq!(
        exchange(&c, b"after", Duration::from_secs(1)).unwrap(),
        b"after"
    );
    assert_eq!(
        exchange(&fast, b"again", Duration::from_secs(1)).unwrap(),
        b"again"
    );
}

/// A session that carries nothing for the idle timeout ends, one that does is kept, and an
/// expired client that sends again gets a new session, from a new socket.
#[test]
fn udp_sessions_expire() {
    let _files = shared();
    let (up, _) = udp_backend(udp_who_am_i);
    let a = Akit::start(&udp_config(r#", "idle_timeout": "300ms""#, &[&up]));
    let (busy, idle) = (a.dial_udp(), a.dial_udp());
    let wait = Duration::from_secs(1);
    let first = exchange(&busy, b"x", wait).unwrap();
    let before = exchange(&idle, b"x", wait).unwrap();
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(900) {
        assert_eq!(
            exchange(&busy, b"x", wait).unwrap(),
            first,
            "the busy session"
        );
        thread::sleep(Duration::from_millis(100));
    }
    a.wait_conns(1, Duration::from_secs(1));
    a.wait_conns(0, Duration::from_secs(2));
    let after = exchange(&idle, b"x", wait).unwrap();
    assert_ne!(after, before, "an expired client's session went on");
}

/// The datagrams of a session past `max_conns` are dropped, and the session taken once another
/// has expired.
#[test]
fn udp_max_conns() {
    let _files = shared();
    let (up, _) = udp_backend(udp_echo);
    let a = Akit::start(&udp_config(
        r#", "max_conns": 2, "idle_timeout": "300ms""#,
        &[&up],
    ));
    let first: Vec<UdpSocket> = (0..2)
        .map(|_| {
            let c = a.dial_udp();
            exchange(&c, b"ok", Duration::from_secs(1)).unwrap();
            c
        })
        .collect();
    let third = a.dial_udp();
    third.send(b"ok").unwrap();
    third
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let r = third.recv(&mut [0u8; 2]);
    assert!(
        matches!(&r, Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)),
        "a third session got {r:?}"
    );
    a.wait_conns(0, Duration::from_secs(2));
    exchange(&third, b"ok", Duration::from_secs(1)).expect("the third, once the others expired");
    drop(first);
}

/// A session whose upstream refuses it ends, and the client's next datagram goes to another
/// address.
#[test]
fn udp_refused() {
    let _files = shared();
    let a = Akit::start(&udp_config("", &[&refused_udp()]));
    let c = a.dial_udp();
    // The refusal comes back to the session's socket as it sends again; it counts against the
    // address, which max_fails (1) takes out of rotation, and ends the session.
    let pool = a.cell.with(|g| g.upstream("app").unwrap().clone());
    let start = Instant::now();
    loop {
        let _ = c.send(b"hello");
        thread::sleep(Duration::from_millis(20));
        if pool.is_down(0) && a.conns() == 0 {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "a refused session was not ended"
        );
    }
    drop(a);

    let (live, _) = udp_backend(udp_echo);
    let a = Akit::start(&udp_config("", &[&refused_udp(), &live]));
    let c = a.dial_udp();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if exchange(&c, b"hello", Duration::from_millis(200)).is_ok_and(|g| g == b"hello") {
            break;
        }
        assert!(Instant::now() < deadline, "no answer from the live address");
    }
}

/// A `udp` listener kept across a reload: a session started before goes on with the old service
/// until the drain timeout, after which its client's next datagram starts a session with the new
/// one, as a new client's does at once.
#[test]
fn udp_reload() {
    let _files = shared();
    fn greet(word: &'static str) -> String {
        udp_backend(move |_, _| Some(word.as_bytes().to_vec())).0
    }
    let mut a = Akit::start_with(
        &udp_config("", &[&greet("old")]),
        Duration::from_millis(500),
        true,
    );
    let wait = Duration::from_secs(1);
    let old = a.dial_udp();
    assert_eq!(exchange(&old, b"x", wait).unwrap(), b"old");
    a.reload(&udp_config("", &[&greet("new")]));
    assert_eq!(
        exchange(&a.dial_udp(), b"x", wait).unwrap(),
        b"new",
        "a new client"
    );
    assert_eq!(
        exchange(&old, b"x", wait).unwrap(),
        b"old",
        "the old session"
    );
    thread::sleep(Duration::from_secs(1));
    assert_eq!(
        exchange(&old, b"x", wait).unwrap(),
        b"new",
        "the old client, once drained"
    );
}

/// A `udp` listener akit started with, removed by a reload (akio's A22): the reload succeeds,
/// the listener's sessions end, and its address takes no new client.
#[test]
fn udp_listener_removed() {
    let _files = shared();
    let (up, _) = udp_backend(|_, d| Some(d.to_vec()));
    let mut a = Akit::start_with(&udp_config("", &[&up]), Duration::from_millis(500), true);
    let wait = Duration::from_secs(1);
    let old = a.dial_udp();
    assert_eq!(exchange(&old, b"x", wait).unwrap(), b"x");
    a.reload(&tcp_config("", &[&backend(echo)]));
    // A session's peer gets on_shutdown, which closes it: the socket closes once none is left.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let c = a.dial_udp();
        if exchange(&c, b"y", Duration::from_millis(100)).is_err() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the removed udp listener still answers"
        );
    }
}

/// The connections a service proxies are counted until both their sides are over, and the
/// upstream's address hears of each (`least_conn` counts them).
#[test]
fn tcp_counted_until_both_sides_end() {
    let _files = shared();
    let (tx, rx) = channel::<TcpStream>();
    let tx = Mutex::new(tx);
    let up = backend(move |s| {
        let _ = tx.lock().unwrap().send(s);
    });
    let a = Akit::start(&tcp_config("", &[&up]));
    let c = a.dial();
    let held: Receiver<TcpStream> = rx;
    let s = held.recv_timeout(Duration::from_secs(5)).unwrap();
    a.wait_conns(1, Duration::from_secs(5));
    let pool = a.cell.with(|g| g.upstream("app").unwrap().clone());
    assert_eq!(pool.stream_count(0), 1);
    c.shutdown(Shutdown::Both).unwrap();
    drop(c);
    a.wait_conns(0, Duration::from_secs(5));
    assert_eq!(pool.stream_count(0), 0);
    drop(s);
}

/// An upstream address that is a host name is resolved off the I/O thread, and its addresses
/// raced: `localhost` may resolve to `::1` first, where nothing listens.
#[test]
fn tcp_host_name() {
    let _files = shared();
    let up = backend(echo);
    let port = up.rsplit_once(':').unwrap().1;
    let a = Akit::start(&tcp_config("", &[&format!("localhost:{port}")]));
    let mut c = a.dial();
    assert_eq!(round_trip(&mut c, b"named").unwrap(), b"named");
}
