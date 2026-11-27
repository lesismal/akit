//! SNI routing and the PROXY protocol (I52; fit's `l4/sni_test.go` and `l4/edges_test.go`):
//! services sharing a `tcp` listener by the server names of their TLS ClientHellos, passed
//! through without terminating their TLS; PROXY headers taken on `tcp` listeners from trusted
//! proxies, truncated, forged and missing ones refused; PROXY headers sent to upstreams. HTTP
//! listeners' PROXY headers are akit-proxy's `tests/proxy_protocol.rs`.
//! On the backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
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
    o.shutdown_timeout = Duration::from_secs(2);
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
    slot: EngineSlot,
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
            slot,
            addr,
        }
    }

    fn reload(&mut self, json: &str) {
        let g = graph(json, &self.slot);
        self.listeners.as_mut().unwrap().reload(g).unwrap();
        self.cell.collect();
    }

    /// How many connections the first listener's services proxy now (fit's `conns`).
    fn conns(&self) -> usize {
        self.cell
            .with(|g| g.entry(0).map_or(0, akit_l4::Entry::conns))
    }

    fn dial(&self) -> TcpStream {
        let c = TcpStream::connect_timeout(&self.addr, Duration::from_secs(5)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c
    }

    /// Shuts akit down, as its process does on a signal.
    fn shutdown(&mut self) {
        drop(self.listeners.take());
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
    }
}

impl Drop for Akit {
    fn drop(&mut self) {
        if self.listeners.is_some() {
            self.shutdown();
        }
    }
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
            thread::spawn(move || serve(s));
        }
    });
    addr
}

fn echo(mut s: TcpStream) {
    let mut r = s.try_clone().unwrap();
    let _ = std::io::copy(&mut r, &mut s);
}

/// Checks that akit closes `c` within `d`: its read ends without data.
fn expect_closed(c: &mut impl Read, d: Duration) {
    let start = Instant::now();
    match c.read(&mut [0u8; 1]) {
        Ok(0) => {}
        Err(e) if !matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
        r => panic!("read {r:?}; want the connection closed"),
    }
    assert!(
        start.elapsed() < d,
        "closed only after {:?}",
        start.elapsed()
    );
}

fn is_timeout(e: &std::io::Error) -> bool {
    matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

/// A certificate for the names the tests ask for, its key, and roots that trust it.
struct Pki {
    chain: Vec<Certificate>,
    key: PrivateKey,
    roots: RootCerts,
}

fn pki() -> &'static Pki {
    static PKI: std::sync::OnceLock<Pki> = std::sync::OnceLock::new();
    PKI.get_or_init(|| {
        let (cert, key) = akio_x509::generate_simple_self_signed([
            "a.example.com",
            "*.w.example.com",
            "b.example.com",
        ])
        .unwrap();
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

/// A TLS server (fit's `tlsBackend`) that reads the ClientHello as it came, which akit must
/// pass on untouched, completes the handshake with its own certificate, tells the client its
/// own `name` and the server name the hello asked for, and then echoes.
fn tls_backend(name: &'static str) -> String {
    let cfg = ServerConfig::builder(pki().chain.clone(), pki().key.clone())
        .build()
        .unwrap();
    backend(move |mut s| {
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
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
        let mut b = [0u8; 1024];
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

/// One tcp listener shared by three services: a.example.com's, *.w.example.com's and, with
/// `fallback`, one without server_names whose upstream echoes in the clear (fit's `sniConfig`).
/// `listener` is put in the listener's object.
fn sni_config(fallback: bool, listener: &str) -> String {
    let mut services = String::from(
        r#"{"name": "a", "listeners": ["tcp"], "upstream": "a", "server_names": ["a.example.com"]},
           {"name": "w", "listeners": ["tcp"], "upstream": "w", "server_names": ["*.w.example.com"]}"#,
    );
    if fallback {
        services.push_str(r#", {"name": "f", "listeners": ["tcp"], "upstream": "f"}"#);
    }
    format!(
        r#"{{
  "listeners": [{{"name": "tcp", "address": "127.0.0.1:0", "protocols": ["tcp"]{listener}}}],
  "upstreams": [{{"name": "a", "addresses": [{:?}]}}, {{"name": "w", "addresses": [{:?}]}}, {{"name": "f", "addresses": [{:?}]}}],
  "services": [{services}]
}}"#,
        tls_backend("a"),
        tls_backend("w"),
        backend(echo)
    )
}

/// Reads exactly `n` bytes as text.
fn read_text(r: &mut impl Read, n: usize) -> String {
    let mut got = vec![0u8; n];
    r.read_exact(&mut got).unwrap();
    String::from_utf8(got).unwrap()
}

/// TLS connections go to the service their ClientHello's name picks, without being
/// terminated: the backend's own certificate completes the handshake, and the backend sees
/// the name. A connection that is not TLS goes to the fallback, or is closed when there is
/// none; so is a name no service has without a fallback (fit's `TestSNIRouting`).
#[test]
fn sni_routing() {
    let a = Akit::start(&sni_config(true, ""));
    for (name, want) in [
        ("a.example.com", "a:a.example.com"),
        ("x.w.example.com", "w:x.w.example.com"),
        // Without case (the client sends it in lower case).
        ("A.Example.COM", "a:a.example.com"),
    ] {
        let mut c = tls_client(name, a.dial()).unwrap();
        assert_eq!(read_text(&mut c, want.len()), want, "{name}");
        c.write_all(b"ping").unwrap();
        c.flush().unwrap();
        assert_eq!(read_text(&mut c, 4), "ping", "{name}");
    }
    // A name one label too deep for the wildcard, and one no service has: the fallback's
    // echo, which is no TLS server, so the handshake fails.
    for name in ["y.x.w.example.com", "b.example.com"] {
        assert!(tls_client(name, a.dial()).is_err(), "{name}");
    }
    // Not TLS: the fallback's echo, the first bytes included.
    let mut c = a.dial();
    c.write_all(b"hello").unwrap();
    assert_eq!(read_text(&mut c, 5), "hello");
    drop(a);

    // No fallback: a name no service takes, and a client in the clear, are closed.
    let a = Akit::start(&sni_config(false, ""));
    match tls_client("b.example.com", a.dial()) {
        Ok(_) => panic!("a name no service takes was served"),
        Err(e) => assert!(!is_timeout(&e), "{e}"),
    }
    let mut c = a.dial();
    c.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
    expect_closed(&mut c, Duration::from_secs(5));
    assert_eq!(a.conns(), 0);
}

/// A client that has sent half a ClientHello is held: no service has taken it yet (fit's
/// `TestSNIHalfHello`); and one that sends the rest, a byte at a time, is served.
#[test]
fn sni_half_hello() {
    let a = Akit::start(&sni_config(true, ""));
    let mut c = a.dial();
    c.write_all(&[0x16, 3, 1, 0x02, 0x00, 1, 0, 1]).unwrap();
    c.set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    match c.read(&mut [0u8; 1]) {
        Err(e) if is_timeout(&e) => {}
        r => panic!("half a hello was answered: {r:?}"),
    }
    assert_eq!(a.conns(), 0, "a service took half a hello");

    // A hello trickled in: a TLS client over a socket that writes a byte at a time.
    struct Trickle(TcpStream);
    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(buf)
        }
    }
    impl Write for Trickle {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            for b in buf {
                self.0.write_all(std::slice::from_ref(b))?;
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.0.flush()
        }
    }
    let cfg = ClientConfig::builder()
        .roots(pki().roots.clone())
        .build()
        .unwrap();
    let sock = a.dial();
    sock.set_nodelay(true).unwrap();
    let mut s = Stream::client(&cfg, "a.example.com", Trickle(sock)).unwrap();
    s.handshake().unwrap();
    assert_eq!(read_text(&mut s, 15), "a:a.example.com");
}

/// A reload that gives a kept listener's services other names routes by the new ones, the
/// listener left bound.
#[test]
fn sni_reload() {
    let mut a = Akit::start(&sni_config(true, ""));
    let mut c = tls_client("a.example.com", a.dial()).unwrap();
    assert_eq!(read_text(&mut c, 15), "a:a.example.com");
    let doc = sni_config(true, "").replace(r#"["a.example.com"]"#, r#"["b.example.com"]"#);
    a.reload(&doc);
    let mut c = tls_client("b.example.com", a.dial()).unwrap();
    assert_eq!(read_text(&mut c, 15), "a:b.example.com");
    assert!(tls_client("a.example.com", a.dial()).is_err());
}

/// A backend that reports the first line, or v2 header, each connection sends it, then echoes
/// (fit's `headerBackend`).
fn header_backend() -> (String, Receiver<String>) {
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let addr = backend(move |mut s| {
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut head = Vec::new();
        let mut b = [0u8; 1];
        // Byte by byte, so that what follows the header is echoed.
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

fn tcp_config(service: &str, listener: &str, upstream: &str) -> String {
    format!(
        r#"{{
  "listeners": [{{"name": "l4", "address": "127.0.0.1:0", "protocols": ["tcp"]{listener}}}],
  "upstreams": [{{"name": "app", "addresses": [{upstream:?}]}}],
  "services": [{{"name": "svc", "listeners": ["l4"], "upstream": "app"{service}}}]
}}"#
    )
}

fn recv(rx: &Receiver<String>) -> String {
    rx.recv_timeout(Duration::from_secs(5)).unwrap()
}

/// The upstream is sent a PROXY header of each version, naming the client and the address it
/// connected to, before what the client sends (fit's `TestProxyProtocolToUpstream`); on each
/// connection of a retried dial.
#[test]
fn proxy_protocol_to_upstream() {
    let (up, got) = header_backend();
    for v in ["v1", "v2"] {
        let a = Akit::start(&tcp_config(
            &format!(r#", "proxy_protocol": "{v}""#),
            "",
            &up,
        ));
        let mut c = a.dial();
        c.write_all(b"hi").unwrap();
        assert_eq!(read_text(&mut c, 2), "hi");
        let (client, akit) = (c.local_addr().unwrap().port(), a.addr.port());
        let want = match v {
            "v1" => format!("PROXY TCP4 127.0.0.1 127.0.0.1 {client} {akit}\r\n"),
            _ => format!("v2 2111000c7f0000017f000001{client:04x}{akit:04x}"),
        };
        assert_eq!(recv(&got), want, "{v}");
    }
}

/// A tcp connection's PROXY header is taken from a trusted address and its client passed on,
/// in one write with the data after it or split; a trusted connection without one is closed,
/// and one from elsewhere is served as it is (fit's `TestProxyProtocolFromClient`).
#[test]
fn proxy_protocol_from_client() {
    let (up, got) = header_backend();
    let doc = |trusted: &str| {
        tcp_config(
            r#", "proxy_protocol": "v1""#,
            &format!(r#", "proxy_protocol": {{"trusted": ["{trusted}"]}}"#),
            &up,
        )
    };
    let a = Akit::start(&doc("127.0.0.0/8"));
    for header in [
        &b"PROXY TCP4 203.0.113.9 198.51.100.1 5555 443\r\n"[..],
        &b"\r\n\r\n\0\r\nQUIT\n\x21\x11\x00\x0c\xcb\x00\x71\x09\xc6\x33\x64\x01\x15\xb3\x01\xbb"[..],
    ] {
        for split in [false, true] {
            let mut c = a.dial();
            if split {
                c.write_all(&header[..7]).unwrap();
                thread::sleep(Duration::from_millis(20));
                c.write_all(&[&header[7..], b"hi"].concat()).unwrap();
            } else {
                c.write_all(&[header, b"hi"].concat()).unwrap();
            }
            assert_eq!(read_text(&mut c, 2), "hi");
            assert_eq!(
                recv(&got),
                "PROXY TCP4 203.0.113.9 198.51.100.1 5555 443\r\n"
            );
        }
    }
    let mut c = a.dial();
    c.write_all(b"hi there, no header\r\n").unwrap();
    expect_closed(&mut c, Duration::from_secs(5));
    drop(a);

    let a = Akit::start(&doc("10.0.0.0/8"));
    let mut c = a.dial();
    c.write_all(b"hi").unwrap();
    assert_eq!(read_text(&mut c, 2), "hi");
    let h = recv(&got);
    assert!(
        h.starts_with("PROXY TCP4 127.0.0.1 127.0.0.1 "),
        "an untrusted client's own address: {h:?}"
    );
}

/// Truncated and forged PROXY headers from a trusted address are refused: the connection is
/// closed before any service takes it, and no upstream connection is made.
#[test]
fn proxy_protocol_bad_headers() {
    let dials = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let d = Arc::clone(&dials);
    let up = backend(move |s| {
        d.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        echo(s);
    });
    let a = Akit::start(&tcp_config(
        "",
        r#", "proxy_protocol": {"trusted": ["127.0.0.1"]}"#,
        &up,
    ));
    let sig = b"\r\n\r\n\0\r\nQUIT\n";
    let forged: Vec<Vec<u8>> = vec![
        // v1: an unknown family, addresses not of the family, a port past 65535, no CRLF
        // within 107 bytes, a misspelt signature.
        b"PROXY BAD HEADER\r\n".to_vec(),
        b"PROXY TCP4 ::1 ::2 1 2\r\n".to_vec(),
        b"PROXY TCP4 1.2.3.4 1.2.3.5 70000 2\r\n".to_vec(),
        [&b"PROXY TCP4 "[..], &[b'1'; 120]].concat(),
        b"PROXI TCP4 1.2.3.4 1.2.3.5 1 2\r\n".to_vec(),
        // v2: version 3, a length past the 4096 bytes read, too short for IPv4's addresses,
        // a bad command.
        [&sig[..], b"\x31\x11\x00\x0c", &[0; 12]].concat(),
        [&sig[..], b"\x21\x11\xff\xff"].concat(),
        [&sig[..], b"\x21\x11\x00\x04abcd"].concat(),
        [&sig[..], b"\x2f\x11\x00\x0c", &[0; 12]].concat(),
    ];
    for h in &forged {
        let mut c = a.dial();
        c.write_all(h).unwrap();
        expect_closed(&mut c, Duration::from_secs(5));
    }
    // Truncated: the client ends before the header does.
    for h in [&b"PROXY TCP4 203.0.113.9 "[..], &sig[..8], &sig[..]] {
        let mut c = a.dial();
        c.write_all(h).unwrap();
        c.shutdown(Shutdown::Write).unwrap();
        expect_closed(&mut c, Duration::from_secs(5));
    }
    assert_eq!(dials.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(a.conns(), 0);
}

/// A connection whose header has not come, and an idle one served after its header, are both
/// closed when akit shuts down (fit's `TestProxyProtocolHTTPEdges`, on a tcp listener).
#[test]
fn proxy_protocol_shutdown() {
    let mut a = Akit::start(&tcp_config(
        "",
        r#", "proxy_protocol": {"trusted": ["127.0.0.1"]}"#,
        &backend(echo),
    ));
    let mut idle = a.dial();
    idle.write_all(b"PROXY TCP4 203.0.113.9 198.51.100.1 5555 80\r\nhi")
        .unwrap();
    assert_eq!(read_text(&mut idle, 2), "hi");
    let mut waiting = a.dial();
    waiting.write_all(b"PROXY ").unwrap();
    thread::sleep(Duration::from_millis(50));
    a.shutdown();
    expect_closed(&mut waiting, Duration::from_secs(5));
    expect_closed(&mut idle, Duration::from_secs(5));
}

/// A listener that routes by SNI takes a PROXY header, and the ClientHello after it in a write
/// of its own (fit's `TestProxyHeaderThenHello`); the service's own header names the client the
/// first one named.
#[test]
fn proxy_header_then_hello() {
    let a = Akit::start(&sni_config(
        true,
        r#", "proxy_protocol": {"trusted": ["127.0.0.0/8"]}"#,
    ));
    let c = a.dial();
    (&c).write_all(b"PROXY TCP4 203.0.113.9 198.51.100.1 5555 443\r\n")
        .unwrap();
    thread::sleep(Duration::from_millis(20));
    let mut s = tls_client("a.example.com", c).unwrap();
    assert_eq!(read_text(&mut s, 15), "a:a.example.com");

    // The header and a hello in one write, to a service sending PROXY v1 upstream.
    let (up, got) = header_backend();
    let doc = format!(
        r#"{{
  "listeners": [{{"name": "tcp", "address": "127.0.0.1:0", "protocols": ["tcp"], "proxy_protocol": {{"trusted": ["127.0.0.1"]}}}}],
  "upstreams": [{{"name": "a", "addresses": [{up:?}]}}],
  "services": [{{"name": "a", "listeners": ["tcp"], "upstream": "a", "server_names": ["a.example.com"], "proxy_protocol": "v1"}}]
}}"#
    );
    let b = Akit::start(&doc);
    let c = b.dial();
    let hello = {
        // A ClientHello as a TLS client writes it: the first flight, read off a socket pair.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        thread::spawn(move || {
            let _ = tls_client("a.example.com", TcpStream::connect(addr).unwrap());
        });
        let (mut s, _) = l.accept().unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut pre = Vec::new();
        let mut buf = [0u8; 4096];
        while akio_mux::tls::parse_client_hello(&pre).is_err() {
            let n = s.read(&mut buf).unwrap();
            assert!(n > 0);
            pre.extend_from_slice(&buf[..n]);
        }
        pre
    };
    let mut wire = b"PROXY TCP6 2001:db8::9 2001:db8::1 5555 443\r\n".to_vec();
    wire.extend_from_slice(&hello);
    (&c).write_all(&wire).unwrap();
    assert_eq!(
        recv(&got),
        "PROXY TCP6 2001:db8::9 2001:db8::1 5555 443\r\n"
    );
    // And the hello after it, untouched: the backend echoes it.
    let mut back = vec![0u8; hello.len()];
    (&c).read_exact(&mut back).unwrap();
    assert_eq!(back, hello);
}
