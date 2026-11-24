//! Many protocols on one port (I53; fit's `l4/sniff_test.go` `TestSniffClear` and
//! `l4/edges_test.go` `TestTLSTerminatedWithoutNames`): a listener serving `tcp` beside HTTP
//! tells them apart by what a client sends first. HTTP/1.1 and h2c in the clear, HTTPS (h2 and
//! HTTP/1.1) with tls, TLS passed through by its server name to the service that takes it, and
//! the rest to the service without `server_names`, on one port, behind a PROXY header too; and
//! SOCKS beside them (I55), as fit's `TestSniffClear` and `test/e2e/multiproto_test.go` serve it.
//! On the backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod support;

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use akio::tls::stream::Stream;
use akio::tls::{Certificate, ClientConfig, PrivateKey, ServerConfig};
use akio::{Backend, Engine, UringConfig};
use akio_http::client::ClientRequest;
use akio_http::h2client::{H2Client, H2Options};
use akit_listener::Listeners;
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, Graph, GraphCell, HandlerParams, Registry, compile};
use support::certs::{Ca, dir};
use support::https::{client, get_h1, get_h2};

fn options() -> akit_listener::Options {
    let mut o = akit_listener::Options::default();
    o.backend = match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    };
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
    engine: Arc<Engine>,
    listeners: Option<Listeners>,
    cell: GraphCell,
    slot: EngineSlot,
    addrs: akit_listener::Addrs,
    /// Where it listens now: its first listener's TCP address.
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
        let addrs = akit_listener::addrs(&cell.with(|g| g.listeners().to_vec()), &engine)[0];
        Akit {
            engine,
            listeners: Some(listeners),
            cell,
            slot,
            addrs,
            addr: addrs.addr().unwrap(),
        }
    }

    fn addr(&self) -> SocketAddr {
        self.addr
    }

    fn dial(&self) -> TcpStream {
        let c = TcpStream::connect_timeout(&self.addr(), Duration::from_secs(5)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c
    }

    fn reload(&mut self, json: &str) {
        let g = graph(json, &self.slot);
        self.listeners.as_mut().unwrap().reload(g).unwrap();
        self.cell.collect();
        self.addr = self.listeners.as_ref().unwrap().served()[0].address;
    }
}

impl Drop for Akit {
    fn drop(&mut self) {
        drop(self.listeners.take());
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
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

/// Certificates under one CA: the passthrough backend's, for a.example.com, and the listener's
/// own, for b.example.com and localhost.
struct Pki {
    ca: PathBuf,
    backend: (PathBuf, PathBuf),
    listener: (PathBuf, PathBuf),
}

fn pki(name: &str) -> Pki {
    let d = dir(name);
    let ca = Ca::new("akit test ca");
    Pki {
        ca: ca.write(&d, "ca.pem"),
        backend: ca.leaf(&d, "backend", &["a.example.com"]),
        listener: ca.leaf(&d, "listener", &["b.example.com", "localhost"]),
    }
}

fn s(p: &Path) -> String {
    p.display().to_string()
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

/// A TLS server (fit's `tlsBackend`) with the certificate at `cert` and `key`, which reads the
/// ClientHello as it came, completes the handshake, tells the client its `name` and the server
/// name the hello asked for, and then echoes.
fn tls_backend(name: &'static str, (cert, key): &(PathBuf, PathBuf)) -> String {
    let chain = Certificate::from_pem(&std::fs::read(cert).unwrap()).unwrap();
    let key = PrivateKey::from_pem(&std::fs::read(key).unwrap()).unwrap();
    let cfg = ServerConfig::builder(chain, key).build().unwrap();
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

/// A TLS connection to `name` over `sock`, its handshake done.
fn tls_client(cfg: &ClientConfig, name: &str, sock: TcpStream) -> Stream<TcpStream> {
    let mut s = Stream::client(cfg, name, sock).unwrap();
    s.handshake().unwrap();
    s
}

/// Reads exactly `n` bytes.
fn read_bytes(r: &mut impl Read, n: usize) -> Vec<u8> {
    let mut got = vec![0u8; n];
    r.read_exact(&mut got).unwrap();
    got
}

/// Reads exactly `n` bytes as text.
fn read_text(r: &mut impl Read, n: usize) -> String {
    let mut got = vec![0u8; n];
    r.read_exact(&mut got).unwrap();
    String::from_utf8(got).unwrap()
}

/// Checks that what `c` sends comes back (fit's `expectEcho`).
fn expect_echo(c: &mut (impl Read + Write), msg: &str) {
    c.write_all(msg.as_bytes()).unwrap();
    c.flush().unwrap();
    assert_eq!(read_text(c, msg.len()), msg);
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

/// An HTTP/1.1 GET in the clear over `c`; the status line and the body.
fn get_clear(c: &mut TcpStream) -> (String, String) {
    c.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut resp = String::new();
    c.read_to_string(&mut resp).unwrap();
    let (head, body) = resp.split_once("\r\n\r\n").unwrap();
    (head.lines().next().unwrap().to_owned(), body.to_owned())
}

/// An HTTP/2 GET with prior knowledge, in the clear; the status and the body.
fn get_h2c(addr: SocketAddr) -> Result<(u16, String), String> {
    let client = Engine::builder().client_threads(1).build().unwrap();
    client.start().unwrap();
    let c = H2Client::dial_blocking(&client, addr, None, H2Options::default())
        .map_err(|e| e.to_string())?;
    let r = c
        .send(ClientRequest::get(&format!("http://{addr}/")).timeout(Duration::from_secs(5)))
        .wait()
        .map_err(|e| e.to_string())?;
    Ok((r.status, String::from_utf8(r.body).unwrap()))
}

/// One listener, `protocols` and `extra` in its object, shared by a service `a` taking
/// a.example.com's TLS to a passthrough backend, by a service `f` without server_names whose
/// upstream echoes when `fallback`, and by a route answering "site".
fn config(p: &Pki, protocols: &str, extra: &str, fallback: bool) -> String {
    let f = if fallback {
        r#", {"name": "f", "listeners": ["one"], "upstream": "f"}"#
    } else {
        ""
    };
    format!(
        r#"{{
  "listeners": [{{"name": "one", "address": "127.0.0.1:0", "protocols": [{protocols}]{extra}}}],
  "upstreams": [{{"name": "a", "addresses": [{:?}]}}, {{"name": "f", "addresses": [{:?}]}}],
  "services": [{{"name": "a", "listeners": ["one"], "upstream": "a", "server_names": ["a.example.com"]}}{f}],
  "routes": [{{"listeners": ["one"], "handler": {{"module": "respond", "params": {{"body": "site"}}}}}}]
}}"#,
        tls_backend("a", &p.backend),
        backend(echo),
    )
}

fn tls_of(p: &Pki) -> String {
    format!(
        r#", "tls": {{"certificates": [{{"cert_file": {:?}, "key_file": {:?}}}]}}"#,
        s(&p.listener.0),
        s(&p.listener.1)
    )
}

/// The `socks` block of a listener serving SOCKS to the tests' loopback servers.
const SOCKS: &str = r#", "socks": {"clients": ["127.0.0.1"], "allow": ["127.0.0.1"]}"#;

/// A SOCKS5 CONNECT to `dest`, an IPv4 address, over `c`, without authentication; the reply's
/// code (fit's `socks5Connect`).
fn socks5_connect(c: &mut TcpStream, dest: &str) -> u8 {
    let SocketAddr::V4(a) = dest.parse::<SocketAddr>().unwrap() else {
        unreachable!()
    };
    let mut req = vec![5, 1, 0, 5, 1, 0, 1];
    req.extend_from_slice(&a.ip().octets());
    req.extend_from_slice(&a.port().to_be_bytes());
    c.write_all(&req).unwrap();
    let mut reply = [0u8; 12];
    c.read_exact(&mut reply).unwrap();
    reply[3]
}

/// HTTP/1.1, h2c, TLS passed through by its name, SOCKS and raw TCP on one port in the clear
/// (fit's `TestSniffClear`).
#[test]
fn sniff_clear() {
    let p = pki("one-port-clear");
    let a = Akit::start(&config(
        &p,
        r#""http/1.1", "h2c", "tcp", "socks""#,
        SOCKS,
        true,
    ));
    assert_eq!(
        get_clear(&mut a.dial()),
        ("HTTP/1.1 200 OK".to_owned(), "site".to_owned())
    );
    assert_eq!(get_h2c(a.addr()).unwrap(), (200, "site".to_owned()));

    let cfg = client(&p.ca, &[], None);
    let mut c = tls_client(&cfg, "a.example.com", a.dial());
    assert_eq!(read_text(&mut c, 15), "a:a.example.com");
    expect_echo(&mut c, "ping");

    // SOCKS5, and SOCKS4, to an echo server.
    let echo = backend(echo);
    let mut c = a.dial();
    assert_eq!(socks5_connect(&mut c, &echo), 0);
    expect_echo(&mut c, "socks");
    let mut c = a.dial();
    let SocketAddr::V4(e) = echo.parse::<SocketAddr>().unwrap() else {
        unreachable!()
    };
    let mut req = vec![4, 1];
    req.extend_from_slice(&e.port().to_be_bytes());
    req.extend_from_slice(&e.ip().octets());
    req.push(0);
    c.write_all(&req).unwrap();
    assert_eq!(read_bytes(&mut c, 8)[1], 0x5a);
    expect_echo(&mut c, "socks4");

    // What no other protocol takes, the service without server_names.
    expect_echo(&mut a.dial(), "\x00raw bytes");
    // TLS for a name no service takes, in the clear: the fallback's echo, no TLS server, so the
    // handshake fails.
    let mut s = Stream::client(&cfg, "b.example.com", a.dial()).unwrap();
    assert!(s.handshake().is_err());
}

/// Without h2c, HTTP/2's preface is no HTTP the listener serves: the service's.
#[test]
fn sniff_clear_without_h2c() {
    let p = pki("one-port-no-h2c");
    let a = Akit::start(&config(&p, r#""http/1.1", "tcp""#, "", true));
    assert_eq!(get_clear(&mut a.dial()).1, "site");
    let preface = "PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
    expect_echo(&mut a.dial(), preface);
}

/// A listener of HTTPS and tcp whose service has no server_names: TLS connections are served
/// HTTPS by the listener, h2 and HTTP/1.1 as ALPN picks, and the rest go to the service (fit's
/// `TestTLSTerminatedWithoutNames`).
#[test]
fn tls_terminated_without_names() {
    let p = pki("one-port-terminated");
    let json = format!(
        r#"{{
  "listeners": [{{"name": "one", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2", "tcp"]{}}}],
  "upstreams": [{{"name": "f", "addresses": [{:?}]}}],
  "services": [{{"name": "f", "listeners": ["one"], "upstream": "f"}}],
  "routes": [{{"listeners": ["one"], "handler": {{"module": "respond", "params": {{"body": "site"}}}}}}]
}}"#,
        tls_of(&p),
        backend(echo)
    );
    let a = Akit::start(&json);
    let seen = get_h1(
        a.addr(),
        &client(&p.ca, &["http/1.1"], None),
        "b.example.com",
        "b.example.com",
        "/",
    )
    .unwrap();
    assert_eq!(
        (seen.status.as_str(), seen.body.as_str()),
        ("HTTP/1.1 200 OK", "site")
    );
    let h2 = client(&p.ca, &["h2"], None);
    assert_eq!(
        get_h2(a.addr(), &h2, "b.example.com", "/").unwrap(),
        (200, "site".to_owned())
    );
    expect_echo(&mut a.dial(), "hello");
    // HTTP in the clear is no HTTP a listener with tls serves: the service's.
    expect_echo(&mut a.dial(), "GET / HTTP/1.1\r\n\r\n");
}

/// With tls and services by name: a.example.com's TLS passed through to its service, any other
/// name served HTTPS by the listener, and the rest to the service without server_names, or
/// closed without one.
#[test]
fn tls_passthrough_beside_https() {
    let p = pki("one-port-tls");
    let a = Akit::start(&config(&p, r#""http/1.1", "h2", "tcp""#, &tls_of(&p), true));
    let cfg = client(&p.ca, &[], None);
    let mut c = tls_client(&cfg, "a.example.com", a.dial());
    assert_eq!(read_text(&mut c, 15), "a:a.example.com");
    expect_echo(&mut c, "ping");
    for name in ["b.example.com", "localhost"] {
        let seen = get_h1(a.addr(), &cfg, name, name, "/").unwrap();
        assert_eq!(seen.body, "site", "{name}");
    }
    expect_echo(&mut a.dial(), "raw");
    drop(a);

    let a = Akit::start(&config(
        &p,
        r#""http/1.1", "h2", "tcp""#,
        &tls_of(&p),
        false,
    ));
    let mut c = tls_client(&cfg, "a.example.com", a.dial());
    assert_eq!(read_text(&mut c, 15), "a:a.example.com");
    let seen = get_h1(a.addr(), &cfg, "b.example.com", "b.example.com", "/").unwrap();
    assert_eq!(seen.body, "site");
    let mut c = a.dial();
    c.write_all(b"raw").unwrap();
    expect_closed(&mut c, Duration::from_secs(5));
}

/// HTTPS (HTTP/1.1 and h2), TLS passed through by its server name, SOCKS and raw TCP on one
/// port; and HTTPS and SOCKS alone on another, whose other bytes are closed (fit's
/// `TestOnePortManyProtocols`, listeners "one" and "three").
#[test]
fn socks_beside_https() {
    let p = pki("one-port-socks");
    let echo = backend(echo);
    let a = Akit::start(&config(
        &p,
        r#""http/1.1", "h2", "tcp", "socks""#,
        &format!("{}{SOCKS}", tls_of(&p)),
        true,
    ));
    let cfg = client(&p.ca, &[], None);
    let seen = get_h1(a.addr(), &cfg, "b.example.com", "b.example.com", "/").unwrap();
    assert_eq!(seen.body, "site");
    let h2 = client(&p.ca, &["h2"], None);
    assert_eq!(
        get_h2(a.addr(), &h2, "b.example.com", "/").unwrap(),
        (200, "site".to_owned())
    );
    let mut c = tls_client(&cfg, "a.example.com", a.dial());
    assert_eq!(read_text(&mut c, 15), "a:a.example.com");
    let mut c = a.dial();
    assert_eq!(socks5_connect(&mut c, &echo), 0);
    expect_echo(&mut c, "socks to one");
    expect_echo(&mut a.dial(), "\x00raw to one");
    drop(a);

    let json = format!(
        r#"{{"listeners": [{{"name": "three", "address": "127.0.0.1:0", "protocols": ["http/1.1", "socks"]{}{SOCKS}}}],
  "routes": [{{"handler": {{"module": "respond", "params": {{"body": "site"}}}}}}]}}"#,
        tls_of(&p)
    );
    let a = Akit::start(&json);
    let seen = get_h1(a.addr(), &cfg, "localhost", "localhost", "/").unwrap();
    assert_eq!(seen.body, "site");
    let mut c = a.dial();
    assert_eq!(socks5_connect(&mut c, &echo), 0);
    expect_echo(&mut c, "socks to three");
    let mut c = a.dial();
    c.write_all(b"\x00raw").unwrap();
    expect_closed(&mut c, Duration::from_secs(5));
}

/// A client that sends half of what would tell its protocol is held, and served once it sends
/// the rest; one whose bytes nothing takes, without a service to fall back to, is closed.
#[test]
fn sniff_waits_for_enough() {
    let p = pki("one-port-wait");
    let a = Akit::start(&config(&p, r#""http/1.1", "h2c", "tcp""#, "", false));
    let mut c = a.dial();
    // "P" could begin POST, PUT, PATCH or the HTTP/2 preface.
    c.write_all(b"P").unwrap();
    c.set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let e = c.read(&mut [0u8; 1]).unwrap_err();
    assert!(
        matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
        "{e}"
    );
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    c.write_all(b"OST / HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut resp = String::new();
    c.read_to_string(&mut resp).unwrap();
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    assert!(resp.ends_with("site"), "{resp}");

    let mut c = a.dial();
    c.write_all(b"\x00raw").unwrap();
    expect_closed(&mut c, Duration::from_secs(5));
}

/// A PROXY header before the bytes the listener sniffs, in a write of its own: HTTP and the
/// service's TCP both served behind it.
#[test]
fn proxy_header_then_sniff() {
    let p = pki("one-port-proxy");
    let a = Akit::start(&config(
        &p,
        r#""http/1.1", "tcp""#,
        r#", "proxy_protocol": {"trusted": ["127.0.0.0/8"]}"#,
        true,
    ));
    let mut c = a.dial();
    c.write_all(b"PROXY TCP4 203.0.113.9 198.51.100.1 5555 80\r\n")
        .unwrap();
    thread::sleep(Duration::from_millis(20));
    assert_eq!(get_clear(&mut c).1, "site");
    let mut c = a.dial();
    c.write_all(b"PROXY TCP4 203.0.113.9 198.51.100.1 5555 80\r\n")
        .unwrap();
    thread::sleep(Duration::from_millis(20));
    expect_echo(&mut c, "raw");
}

/// A listener of tcp, HTTPS and h3 at port 0 takes one port for TCP and UDP, which Alt-Svc
/// names; a reload that adds tcp to an HTTP listener serves both on its port.
#[test]
fn ports_and_reload() {
    let p = pki("one-port-h3");
    let a = Akit::start(&config(
        &p,
        r#""http/1.1", "h2", "h3", "tcp""#,
        &tls_of(&p),
        true,
    ));
    let (tcp, udp) = (a.addrs.tcp.unwrap(), a.addrs.udp.unwrap());
    assert_eq!(tcp.port(), udp.port());
    let seen = get_h1(
        tcp,
        &client(&p.ca, &[], None),
        "localhost",
        "localhost",
        "/",
    )
    .unwrap();
    assert_eq!(
        seen.alt_svc.as_deref(),
        Some(akit_listener::alt_svc(tcp.port()).as_str())
    );
    expect_echo(&mut a.dial(), "raw");
    drop(a);

    let web = r#"{"listeners": [{"name": "one", "address": "127.0.0.1:0"}],
  "routes": [{"listeners": ["one"], "handler": {"module": "respond", "params": {"body": "site"}}}]}"#;
    let mut a = Akit::start(web);
    assert_eq!(get_clear(&mut a.dial()).1, "site");
    a.reload(&config(&p, r#""http/1.1", "tcp""#, "", true));
    assert_eq!(get_clear(&mut a.dial()).1, "site");
    expect_echo(&mut a.dial(), "raw");
}
