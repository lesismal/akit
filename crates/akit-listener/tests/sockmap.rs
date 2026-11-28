//! The eBPF sockmap relay (I51b): a `tcp` listener with `sockmap` has its services' connections
//! spliced in the kernel where the relay loads (Linux 6.15+, `CAP_BPF` and `CAP_NET_ADMIN`), and
//! relayed in userspace as I51's otherwise; the data and its order are the same either way, the
//! closes forward everything, and the idle timeout follows the sockets' counters.
//!
//! Where the relay loads, every test checks its connections were spliced; elsewhere that none
//! was. `AKIO_TEST_EXPECT_EBPF=1` makes a relay that does not load a failure (the root run on
//! Linux), `AKIT_TEST_SOCKMAP=userspace` one that does (the run without capabilities, under
//! `setpriv`). On the backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation
)]

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use akio::tls::stream::Stream;
use akio::tls::{Certificate, ClientConfig, PrivateKey, RootCerts, ServerConfig};
use akio::{Backend, UringConfig};
use akit_listener::Listeners;
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, Graph, GraphCell, HandlerParams, Registry, compile};

fn options() -> akit_listener::Options {
    let mut o = akit_listener::Options::default();
    o.backend = Some(match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    });
    o.shutdown_timeout = Duration::from_secs(5);
    o
}

fn graph(json: &str, slot: &EngineSlot) -> Graph {
    let config = akit_config::from_json(json, None).unwrap();
    let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
    compile(&config, &registry, &Env::new(slot.clone())).unwrap()
}

/// akit serving a config, as `akit run` does.
struct Akit {
    cell: GraphCell,
    engine: Arc<akio::Engine>,
    listeners: Option<Listeners>,
    addr: SocketAddr,
}

impl Akit {
    fn start(json: &str) -> Akit {
        let slot = EngineSlot::new();
        let cell = GraphCell::new(graph(json, &slot));
        let opts = options();
        let engine = Arc::new(akit_listener::engine(&cell, &opts).unwrap());
        engine.start().unwrap();
        assert!(slot.set(engine.clone()));
        let listeners = Listeners::new(Arc::clone(&engine), cell.clone(), &opts);
        let addr = akit_listener::addrs(&cell.with(|g| g.listeners().to_vec()), &engine)[0]
            .addr()
            .unwrap();
        Akit {
            cell,
            engine,
            listeners: Some(listeners),
            addr,
        }
    }

    fn dial(&self) -> TcpStream {
        let c = TcpStream::connect_timeout(&self.addr, Duration::from_secs(5)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        c.set_write_timeout(Some(Duration::from_secs(30))).unwrap();
        c
    }

    /// The service `name`'s connections spliced so far.
    fn spliced(&self, name: &str) -> u64 {
        self.cell.with(|g| {
            g.services()
                .iter()
                .find(|s| s.name() == name)
                .unwrap()
                .spliced()
        })
    }

    /// How many connections the service `name` proxies now.
    fn conns(&self, name: &str) -> usize {
        self.cell.with(|g| {
            g.services()
                .iter()
                .find(|s| s.name() == name)
                .unwrap()
                .conns()
        })
    }
}

impl Drop for Akit {
    fn drop(&mut self) {
        drop(self.listeners.take());
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
    }
}

/// Whether the kernel relay loaded, as the run expects (see the module's docs); call it once
/// an `Akit` with `sockmap` has started, which loads it.
fn kernel() -> bool {
    let loaded = akit_l4::sockmap::available();
    if std::env::var("AKIO_TEST_EXPECT_EBPF").as_deref() == Ok("1") {
        assert!(loaded, "the sockmap relay did not load");
    }
    if std::env::var("AKIT_TEST_SOCKMAP").as_deref() == Ok("userspace") {
        assert!(!loaded, "the sockmap relay loaded");
    }
    loaded
}

/// `n` if the relay loaded, else 0: the connections a test expects spliced.
fn if_kernel(n: u64) -> u64 {
    if kernel() { n } else { 0 }
}

/// A TCP server serving each connection with `serve` on a thread of its own.
fn backend(serve: impl Fn(TcpStream) + Send + Sync + 'static) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let serve = Arc::new(serve);
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            let serve = Arc::clone(&serve);
            thread::spawn(move || {
                s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
                serve(s);
            });
        }
    });
    addr
}

fn echo(mut s: TcpStream) {
    let mut r = s.try_clone().unwrap();
    let _ = std::io::copy(&mut r, &mut s);
}

/// A config whose `tcp` listener (with `listener` in its object) serves the service `svc` (with
/// `service` in its object), which proxies to `upstream`.
fn config(listener: &str, service: &str, upstream: &str) -> String {
    format!(
        r#"{{
  "listeners": [{{"name": "l4", "address": "127.0.0.1:0", "protocols": ["tcp"]{listener}}}],
  "upstreams": [{{"name": "app", "addresses": [{upstream:?}]}}],
  "services": [{{"name": "svc", "listeners": ["l4"], "upstream": "app"{service}}}]
}}"#
    )
}

const SOCKMAP: &str = r#", "sockmap": true"#;

/// `n` bytes that are not all alike, seeded by `seed`.
fn pattern(n: usize, seed: u8) -> Vec<u8> {
    (0..n).map(|i| (i.wrapping_mul(31) as u8) ^ seed).collect()
}

fn round_trip(c: &mut TcpStream, data: &[u8]) -> Vec<u8> {
    c.write_all(data).unwrap();
    let mut got = vec![0u8; data.len()];
    c.read_exact(&mut got).unwrap();
    got
}

/// Checks that `c` is closed within `d`: its read ends without data.
fn expect_closed(c: &mut impl Read, d: Duration) {
    let deadline = Instant::now() + d;
    loop {
        match c.read(&mut [0u8; 4096]) {
            Ok(0) => return,
            Ok(_) => {}
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                panic!("still open after {d:?}")
            }
            Err(_) => return,
        }
        assert!(Instant::now() < deadline, "still sending after {d:?}");
    }
}

/// Connections are spliced and echo, one with 4 MB each way at once, 20 at a time; a listener
/// without the field relays in userspace.
#[test]
fn splices_and_echoes() {
    let up = backend(echo);
    let a = Akit::start(&config(SOCKMAP, "", &up));
    let mut c = a.dial();
    assert_eq!(round_trip(&mut c, b"hello"), b"hello");
    assert_eq!(a.spliced("svc"), if_kernel(1));

    const SIZE: usize = 4 << 20;
    let want = pattern(SIZE, 3);
    let mut w = c.try_clone().unwrap();
    let sent = want.clone();
    let writer = thread::spawn(move || w.write_all(&sent).unwrap());
    let mut got = vec![0u8; SIZE];
    c.read_exact(&mut got).unwrap();
    writer.join().unwrap();
    assert!(got == want, "the {SIZE} bytes sent came back otherwise");

    let threads: Vec<_> = (0..20u8)
        .map(|i| {
            let mut c = a.dial();
            thread::spawn(move || {
                for round in 0..5u8 {
                    let want = pattern(10_000 + usize::from(i) * 100, i ^ round);
                    assert_eq!(round_trip(&mut c, &want), want, "connection {i}");
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(a.spliced("svc"), if_kernel(21));

    let b = Akit::start(&config("", "", &up));
    let mut c = b.dial();
    assert_eq!(round_trip(&mut c, b"plain"), b"plain");
    assert_eq!(b.spliced("svc"), 0);
}

/// The upstream sends 32 MB to a client that reads nothing for a while: the kernel's relay
/// waits on the client's full buffers, and the client then gets every byte in order (the
/// kernels before 6.15 lost or reordered bytes here, which akio's relay refuses to run on).
#[test]
fn slow_client_gets_everything() {
    const SIZE: usize = 32 << 20;
    let up = backend(|mut s| {
        let mut b = [0u8; 1];
        if s.read_exact(&mut b).is_ok() {
            let _ = s.write_all(&pattern(SIZE, 9));
        }
    });
    let a = Akit::start(&config(SOCKMAP, "", &up));
    let mut c = a.dial();
    c.write_all(b"g").unwrap();
    thread::sleep(Duration::from_millis(500));
    let mut got = vec![0u8; SIZE];
    c.read_exact(&mut got).unwrap();
    assert!(got == pattern(SIZE, 9), "the {SIZE} bytes came otherwise");
    assert_eq!(a.spliced("svc"), if_kernel(1));
}

/// A client that sends 8 MB and ends its side: the upstream gets every byte before its
/// connection ends, also what the kernel's relay still held when the client's end came.
#[test]
fn client_end_forwards_everything() {
    const SIZE: usize = 8 << 20;
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let up = backend(move |mut s| {
        let mut all = Vec::new();
        let r = s.read_to_end(&mut all);
        let _ = tx.lock().unwrap().send((r.is_ok(), all));
    });
    let a = Akit::start(&config(SOCKMAP, "", &up));
    for seed in 0..3u8 {
        let c = a.dial();
        (&c).write_all(&pattern(SIZE, seed)).unwrap();
        c.shutdown(Shutdown::Write).unwrap();
        let (ended, got) = rx.recv_timeout(Duration::from_secs(30)).unwrap();
        assert!(ended, "the upstream's read failed");
        assert!(
            got == pattern(SIZE, seed),
            "the upstream got {} bytes",
            got.len()
        );
    }
    assert_eq!(a.spliced("svc"), if_kernel(3));
}

/// An upstream that sends 8 MB and closes: the client gets every byte, then its end.
#[test]
fn upstream_end_forwards_everything() {
    const SIZE: usize = 8 << 20;
    let up = backend(|mut s| {
        let mut b = [0u8; 1];
        if s.read_exact(&mut b).is_ok() {
            let _ = s.write_all(&pattern(SIZE, b[0]));
        }
    });
    let a = Akit::start(&config(SOCKMAP, "", &up));
    for seed in 0..3u8 {
        let mut c = a.dial();
        c.write_all(&[seed]).unwrap();
        let mut got = Vec::new();
        c.read_to_end(&mut got).unwrap();
        assert!(
            got == pattern(SIZE, seed),
            "the client got {} bytes",
            got.len()
        );
    }
    assert_eq!(a.spliced("svc"), if_kernel(3));
    let deadline = Instant::now() + Duration::from_secs(5);
    while a.conns("svc") != 0 {
        assert!(
            Instant::now() < deadline,
            "{} still proxied",
            a.conns("svc")
        );
        thread::sleep(Duration::from_millis(20));
    }
}

/// A spliced connection whose bytes go one way or the other within the idle timeout stays open,
/// though they never reach akit's handlers; one that carries nothing for it is closed.
#[test]
fn idle_timeout_from_counters() {
    // The upstream echoes, and on "t" sends a byte every 100 ms until the client goes.
    let up = backend(|mut s| {
        let mut b = [0u8; 64];
        loop {
            let Ok(n) = s.read(&mut b) else { return };
            if n == 0 {
                return;
            }
            if &b[..n] == b"t" {
                while s.write_all(b".").is_ok() {
                    thread::sleep(Duration::from_millis(100));
                }
                return;
            }
            if s.write_all(&b[..n]).is_err() {
                return;
            }
        }
    });
    let a = Akit::start(&config(SOCKMAP, r#", "idle_timeout": "500ms""#, &up));

    // From the upstream alone, every 100 ms for 1.5 s.
    let mut d = a.dial();
    d.write_all(b"t").unwrap();
    let mut b = [0u8; 1];
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(1500) {
        d.read_exact(&mut b).unwrap();
        assert_eq!(&b, b".");
    }
    drop(d);
    // Both ways, every 150 ms for 1.5 s.
    let mut c = a.dial();
    let mut quiet = Instant::now();
    for i in 0..10u8 {
        thread::sleep(Duration::from_millis(150));
        assert_eq!(round_trip(&mut c, &[b'a' + i]), [b'a' + i]);
        quiet = Instant::now();
    }
    assert_eq!(a.spliced("svc"), if_kernel(2));

    // Quiet from the last byte: closed after the timeout, within a sweep or two.
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    expect_closed(&mut c, Duration::from_secs(5));
    let took = quiet.elapsed();
    assert!(
        took >= Duration::from_millis(450) && took < Duration::from_secs(3),
        "closed {took:?} after the last byte"
    );
}

/// The service's PROXY header of each version goes to the upstream first, written by akit
/// before the kernel relays the client's bytes.
#[test]
fn proxy_header_goes_first() {
    let (up, got) = header_backend();
    for (i, v) in ["v1", "v2"].into_iter().enumerate() {
        let a = Akit::start(&config(
            SOCKMAP,
            &format!(r#", "proxy_protocol": "{v}""#),
            &up,
        ));
        let mut c = a.dial();
        assert_eq!(round_trip(&mut c, b"hi"), b"hi");
        let (client, akit) = (c.local_addr().unwrap().port(), a.addr.port());
        let want = match v {
            "v1" => format!("PROXY TCP4 127.0.0.1 127.0.0.1 {client} {akit}\r\n"),
            _ => format!("v2 2111000c7f0000017f000001{client:04x}{akit:04x}"),
        };
        assert_eq!(got.recv_timeout(Duration::from_secs(5)).unwrap(), want);
        assert_eq!(a.spliced("svc"), if_kernel(1), "{i}");
    }
}

/// A connection that began with a PROXY header the listener read is spliced too: the tunnel's
/// base counters count the header userspace read (akio A170), and the kernel relays only what
/// follows it.
#[test]
fn client_proxy_header_spliced() {
    let up = backend(echo);
    let a = Akit::start(&config(
        r#", "sockmap": true, "proxy_protocol": {"trusted": ["127.0.0.1"]}"#,
        "",
        &up,
    ));
    const HEADER: &[u8] = b"PROXY TCP4 203.0.113.9 198.51.100.1 5555 443\r\n";
    let mut c = a.dial();
    c.write_all(HEADER).unwrap();
    assert_eq!(round_trip(&mut c, b"via a proxy"), b"via a proxy");
    let more = pattern(256 << 10, 7);
    assert!(
        round_trip(&mut c, &more) == more,
        "the echo came back otherwise"
    );
    assert_eq!(a.spliced("svc"), if_kernel(1));

    // The header and the first bytes in one write: the read that brings the header brings
    // them too, after the listener took the connection, and they go to the upstream ahead of
    // what the kernel relays.
    let mut c = a.dial();
    c.write_all(&[HEADER, b"in one write"].concat()).unwrap();
    let mut got = [0u8; 12];
    c.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"in one write");
    assert!(
        round_trip(&mut c, &more) == more,
        "the echo came back otherwise"
    );
    assert_eq!(a.spliced("svc"), if_kernel(2));
}

/// A TLS connection routed by the server name of its ClientHello is spliced once its upstream
/// opens: the hello, read by akit to pick the service, goes first, and the kernel relays the
/// rest of the handshake and the data.
#[test]
fn sni_passthrough_spliced() {
    let doc = format!(
        r#"{{
  "listeners": [{{"name": "tcp", "address": "127.0.0.1:0", "protocols": ["tcp"], "sockmap": true}}],
  "upstreams": [{{"name": "a", "addresses": [{:?}]}}, {{"name": "f", "addresses": [{:?}]}}],
  "services": [
    {{"name": "a", "listeners": ["tcp"], "upstream": "a", "server_names": ["a.example.com"]}},
    {{"name": "f", "listeners": ["tcp"], "upstream": "f"}}
  ]
}}"#,
        tls_backend("a"),
        backend(echo)
    );
    let a = Akit::start(&doc);
    for _ in 0..3 {
        let mut s = tls_client("a.example.com", a.dial()).unwrap();
        let mut hi = vec![0u8; 15];
        s.read_exact(&mut hi).unwrap();
        assert_eq!(hi, b"a:a.example.com");
        let want = pattern(256 << 10, 5);
        s.write_all(&want).unwrap();
        s.flush().unwrap();
        let mut got = vec![0u8; want.len()];
        s.read_exact(&mut got).unwrap();
        assert!(got == want, "the echo came back otherwise");
    }
    // Not TLS: the fallback's, spliced too.
    let mut c = a.dial();
    assert_eq!(round_trip(&mut c, b"clear"), b"clear");
    assert_eq!(a.spliced("a"), if_kernel(3));
    assert_eq!(a.spliced("f"), if_kernel(1));
}

/// A backend that reports the PROXY header each connection begins with (a v1 line, or a v2
/// header in hex from its version byte), then echoes.
fn header_backend() -> (String, Receiver<String>) {
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let addr = backend(move |mut s| {
        let mut head = Vec::new();
        let mut b = [0u8; 1];
        let report = loop {
            if s.read(&mut b).unwrap_or(0) == 0 {
                return;
            }
            head.push(b[0]);
            if head.starts_with(b"\r\n\r\n\0\r\nQUIT\n") {
                if head.len() >= 16 {
                    let len = 16 + usize::from(u16::from_be_bytes([head[14], head[15]]));
                    if head.len() == len {
                        let hex: String = head[12..].iter().map(|b| format!("{b:02x}")).collect();
                        break format!("v2 {hex}");
                    }
                }
            } else if head.ends_with(b"\n") && !b"\r\n\r\n\0\r\nQUIT\n".starts_with(&head) {
                break String::from_utf8_lossy(&head).into_owned();
            }
        };
        let _ = tx.lock().unwrap().send(report);
        echo(s);
    });
    (addr, rx)
}

/// A certificate for a.example.com, its key, and roots that trust it.
struct Pki {
    chain: Vec<Certificate>,
    key: PrivateKey,
    roots: RootCerts,
}

fn pki() -> &'static Pki {
    static PKI: std::sync::OnceLock<Pki> = std::sync::OnceLock::new();
    PKI.get_or_init(|| {
        let (cert, key) = akio_x509::generate_simple_self_signed(["a.example.com"]).unwrap();
        let leaf = Certificate::from_der(cert.as_der().to_vec());
        let mut roots = RootCerts::empty();
        roots.add(&leaf).unwrap();
        Pki {
            chain: vec![leaf],
            key: PrivateKey::from_pkcs8_der(key.to_pkcs8_der()),
            roots,
        }
    })
}

/// A socket whose reads give `pre` first: what a backend read to see the ClientHello.
struct Replay {
    pre: Vec<u8>,
    at: usize,
    s: TcpStream,
}

impl Read for Replay {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.at < self.pre.len() {
            let n = buf.len().min(self.pre.len() - self.at);
            buf[..n].copy_from_slice(&self.pre[self.at..self.at + n]);
            self.at += n;
            return Ok(n);
        }
        self.s.read(buf)
    }
}

impl Write for Replay {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.s.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.s.flush()
    }
}

/// A TLS server that reads the ClientHello as it came (akit passes it on untouched), completes
/// the handshake, tells the client its `name` and the hello's server name, and echoes.
fn tls_backend(name: &'static str) -> String {
    let cfg = ServerConfig::builder(pki().chain.clone(), pki().key.clone())
        .build()
        .unwrap();
    backend(move |mut s| {
        let mut pre = Vec::new();
        let mut buf = [0u8; 4096];
        let sni = loop {
            let Ok(n) = s.read(&mut buf) else { return };
            if n == 0 {
                return;
            }
            pre.extend_from_slice(&buf[..n]);
            match akio_mux::tls::parse_client_hello(&pre) {
                Ok(h) => break h.server_name().unwrap_or("").to_owned(),
                Err(akio_mux::tls::HelloError::Incomplete) => {}
                Err(_) => return,
            }
        };
        let mut tls = Stream::server(&cfg, Replay { pre, at: 0, s });
        if tls.handshake().is_err() {
            return;
        }
        let _ = tls.write_all(format!("{name}:{sni}").as_bytes());
        let _ = tls.flush();
        let mut b = [0u8; 16 << 10];
        while let Ok(n) = tls.read(&mut b) {
            if n == 0 || tls.write_all(&b[..n]).is_err() || tls.flush().is_err() {
                return;
            }
        }
    })
}

fn tls_client(name: &str, sock: TcpStream) -> std::io::Result<Stream<TcpStream>> {
    let cfg = ClientConfig::builder()
        .roots(pki().roots.clone())
        .build()
        .unwrap();
    let mut s = Stream::client(&cfg, name, sock)?;
    s.handshake()?;
    Ok(s)
}
