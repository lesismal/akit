//! I55: a listener's `socks`, fit's SOCKS5 and SOCKS4 forward proxy, through akit's listener on
//! akio (fit's `l4/forward.go` rules, `config` checks and `test/e2e/multiproto_test.go` SOCKS
//! parts): CONNECT by address and by name to what the rules allow, the rest refused (clients,
//! ports, `deny`, internal addresses by address and by the address a name resolves to), BIND and
//! UDP ASSOCIATE refused; large transfers both ways with slow peers on either side, connections
//! cut midway, the idle and connect timeouts, a PROXY header's client, and a reload that changes
//! the rules. SOCKS beside HTTP and `tcp` on one port is in `one_port.rs`. On the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, UringConfig};
use akit_listener::Listeners;
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, Graph, GraphCell, HandlerParams, Registry, compile};

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
    let config = akit_config::from_json(json, None).unwrap_or_else(|e| panic!("{json}: {e}"));
    let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
    compile(&config, &registry, &Env::new(slot.clone())).unwrap()
}

/// akit serving a config, as `akit run` does.
struct Akit {
    engine: Arc<Engine>,
    listeners: Option<Listeners>,
    cell: GraphCell,
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
            engine,
            listeners: Some(listeners),
            cell,
            slot,
            addr,
        }
    }

    fn dial(&self) -> TcpStream {
        let c = TcpStream::connect_timeout(&self.addr, Duration::from_secs(5)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
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

/// A SOCKS listener with `socks` as its rules.
fn socks_config(socks: &str) -> String {
    format!(
        r#"{{"listeners": [{{"name": "s", "address": "127.0.0.1:0", "protocols": ["socks"], "socks": {socks}}}]}}"#
    )
}

/// A TCP server serving each connection with `serve` on a thread of its own.
fn backend(serve: impl Fn(TcpStream) + Send + Sync + 'static) -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
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

/// SOCKS5's `ATYP ADDR PORT` of an IPv4 address.
fn v4(a: SocketAddr) -> Vec<u8> {
    let SocketAddr::V4(a) = a else { unreachable!() };
    let mut t = vec![1];
    t.extend_from_slice(&a.ip().octets());
    t.extend_from_slice(&a.port().to_be_bytes());
    t
}

/// SOCKS5's `ATYP ADDR PORT` of a host name.
fn name(host: &str, port: u16) -> Vec<u8> {
    let mut t = vec![3, u8::try_from(host.len()).unwrap()];
    t.extend_from_slice(host.as_bytes());
    t.extend_from_slice(&port.to_be_bytes());
    t
}

/// A SOCKS5 request of command `cmd` (1 CONNECT, 2 BIND, 3 UDP ASSOCIATE) for `dest` over `c`,
/// without authentication; the reply's code.
fn socks5(c: &mut TcpStream, cmd: u8, dest: &[u8]) -> u8 {
    c.write_all(&[5, 1, 0]).unwrap();
    let mut m = [0u8; 2];
    c.read_exact(&mut m).unwrap();
    assert_eq!(m, [5, 0], "method");
    let mut req = vec![5, cmd, 0];
    req.extend_from_slice(dest);
    c.write_all(&req).unwrap();
    let mut head = [0u8; 4];
    c.read_exact(&mut head).unwrap();
    let rest = match head[3] {
        1 => 6,
        4 => 18,
        3 => {
            let mut l = [0u8; 1];
            c.read_exact(&mut l).unwrap();
            usize::from(l[0]) + 2
        }
        _ => 0,
    };
    c.read_exact(&mut vec![0u8; rest]).unwrap();
    head[1]
}

/// A SOCKS4 CONNECT to `dest`, or SOCKS4a to `host` at `dest`'s port; the reply's code.
fn socks4(c: &mut TcpStream, dest: SocketAddr, host: Option<&str>) -> u8 {
    let SocketAddr::V4(a) = dest else {
        unreachable!()
    };
    let mut req = vec![4, 1];
    req.extend_from_slice(&a.port().to_be_bytes());
    match host {
        None => req.extend_from_slice(&a.ip().octets()),
        Some(_) => req.extend_from_slice(&[0, 0, 0, 1]),
    }
    req.push(0);
    if let Some(h) = host {
        req.extend_from_slice(h.as_bytes());
        req.push(0);
    }
    c.write_all(&req).unwrap();
    let mut r = [0u8; 8];
    c.read_exact(&mut r).unwrap();
    r[1]
}

fn expect_echo(c: &mut TcpStream, msg: &[u8]) {
    c.write_all(msg).unwrap();
    let mut got = vec![0u8; msg.len()];
    c.read_exact(&mut got).unwrap();
    assert_eq!(got, msg);
}

/// Reads until the peer closes, within `d`; what came before.
fn expect_closed(c: &mut TcpStream, d: Duration) -> Vec<u8> {
    c.set_read_timeout(Some(d)).unwrap();
    let start = Instant::now();
    let mut all = Vec::new();
    let mut b = [0u8; 4096];
    loop {
        match c.read(&mut b) {
            Ok(0) => break,
            Ok(n) => all.extend_from_slice(&b[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                panic!("still open after {:?}", start.elapsed())
            }
            Err(_) => break,
        }
    }
    all
}

fn pattern(i: usize) -> u8 {
    u8::try_from((i * 7 + i / 251) % 256).unwrap()
}

/// CONNECT by address, SOCKS5 and SOCKS4, and by name, SOCKS5 and SOCKS4a, to an echo server the
/// rules allow; BIND and UDP ASSOCIATE refused as commands fit does not serve.
#[test]
fn connect_by_address_and_name() {
    let echo = backend(echo);
    let a = Akit::start(&socks_config(
        r#"{"clients": ["127.0.0.1"], "allow": ["127.0.0.0/8", "localhost"]}"#,
    ));
    let mut c = a.dial();
    assert_eq!(socks5(&mut c, 1, &v4(echo)), 0);
    expect_echo(&mut c, b"by address");
    let mut c = a.dial();
    assert_eq!(socks5(&mut c, 1, &name("localhost", echo.port())), 0);
    expect_echo(&mut c, b"by name");
    let mut c = a.dial();
    assert_eq!(socks4(&mut c, echo, None), 0x5a);
    expect_echo(&mut c, b"socks4");
    let mut c = a.dial();
    assert_eq!(socks4(&mut c, echo, Some("localhost")), 0x5a);
    expect_echo(&mut c, b"socks4a");
    // fit serves CONNECT alone.
    for cmd in [2, 3] {
        let mut c = a.dial();
        assert_eq!(socks5(&mut c, cmd, &v4(echo)), 7, "command {cmd}");
        expect_closed(&mut c, Duration::from_secs(5));
    }
}

/// What the rules refuse gets reply 2 (SOCKS4 0x5b) and is closed, and nothing reaches the
/// destination: a client not in `clients`, a port not in `ports`, an address or name `deny`
/// holds, a name `allow` does not, and, with neither, loopback and link-local, by address and by
/// a name that resolves there, which is decided once connected.
#[test]
fn refusals() {
    // How much each connection it accepts is sent: a name refused once connected opens one,
    // and closes it before sending anything.
    let (tx, reached) = mpsc::channel::<usize>();
    let tx = std::sync::Mutex::new(tx);
    let target = backend(move |mut s| {
        let mut n = 0;
        let mut b = [0u8; 1024];
        while let Ok(k) = s.read(&mut b) {
            if k == 0 {
                break;
            }
            n += k;
        }
        let _ = tx.lock().unwrap().send(n);
    });
    let port = target.port();
    let refused = |rules: &str, dest: &[u8]| {
        let a = Akit::start(&socks_config(rules));
        let mut c = a.dial();
        assert_eq!(socks5(&mut c, 1, dest), 2, "{rules} {dest:?}");
        expect_closed(&mut c, Duration::from_secs(5));
    };
    refused(
        r#"{"clients": ["10.0.0.0/8"], "allow": ["127.0.0.0/8"]}"#,
        &v4(target),
    );
    refused(
        r#"{"clients": ["127.0.0.1"], "allow": ["127.0.0.0/8"], "ports": [1]}"#,
        &v4(target),
    );
    refused(
        r#"{"clients": ["127.0.0.1"], "allow": ["127.0.0.0/8"], "deny": ["127.0.0.1"]}"#,
        &v4(target),
    );
    refused(
        r#"{"clients": ["127.0.0.1"], "allow": ["127.0.0.0/8", "localhost"], "deny": ["*.localhost", "localhost"]}"#,
        &name("localhost", port),
    );
    refused(
        r#"{"clients": ["127.0.0.1"], "allow": ["example.com"]}"#,
        &name("localhost", port),
    );
    // The defaults: loopback by address, and by a name resolving there once connected.
    refused(r#"{"clients": ["127.0.0.1"]}"#, &v4(target));
    refused(
        r#"{"clients": ["127.0.0.1"]}"#,
        &[1, 169, 254, 169, 254, 0, 80],
    );
    refused(r#"{"clients": ["127.0.0.1"]}"#, &name("localhost", port));
    // A deny range holds the address a name allow holds resolves to.
    refused(
        r#"{"clients": ["127.0.0.1"], "allow": ["localhost"], "deny": ["127.0.0.0/8", "::1"]}"#,
        &name("localhost", port),
    );
    // A wider allow range does not lift the refusal of loopback.
    refused(
        r#"{"clients": ["127.0.0.1"], "allow": ["0.0.0.0/0"]}"#,
        &v4(target),
    );
    // SOCKS4 refused.
    let a = Akit::start(&socks_config(r#"{"clients": ["127.0.0.1"]}"#));
    let mut c = a.dial();
    assert_eq!(socks4(&mut c, target, None), 0x5b);
    drop(c);
    while let Ok(n) = reached.recv_timeout(Duration::from_millis(300)) {
        assert_eq!(n, 0, "a refused destination was sent something");
    }
}

/// Large transfers both ways through the tunnel while each side reads slowly in turn: the relay
/// holds the fast side's reads instead of buffering, and every byte arrives in order.
#[test]
fn slow_peers_both_ways() {
    const N: usize = 8 << 20;
    // A slow upstream: reads in small pieces with pauses, then answers with as much.
    let slow_reader = backend(|mut s| {
        let mut got = 0;
        let mut b = [0u8; 16 << 10];
        while got < N {
            let n = s.read(&mut b).unwrap();
            assert!(n > 0, "cut at {got}");
            for (i, x) in b[..n].iter().enumerate() {
                assert_eq!(*x, pattern(got + i), "byte {}", got + i);
            }
            got += n;
            if got % (1 << 20) < n {
                thread::sleep(Duration::from_millis(50));
            }
        }
        let out: Vec<u8> = (0..N).map(pattern).collect();
        s.write_all(&out).unwrap();
    });
    let a = Akit::start(&socks_config(
        r#"{"clients": ["127.0.0.1"], "allow": ["127.0.0.0/8"]}"#,
    ));
    let mut c = a.dial();
    assert_eq!(socks5(&mut c, 1, &v4(slow_reader)), 0);
    let data: Vec<u8> = (0..N).map(pattern).collect();
    c.write_all(&data).unwrap();
    // A slow client: reads the answer in pieces with pauses.
    let mut got = 0;
    let mut b = vec![0u8; 32 << 10];
    while got < N {
        let n = c.read(&mut b).unwrap();
        assert!(n > 0, "cut at {got}");
        assert!(
            b[..n]
                .iter()
                .enumerate()
                .all(|(i, x)| *x == pattern(got + i)),
            "bytes at {got} differ"
        );
        got += n;
        if got % (1 << 20) < n {
            thread::sleep(Duration::from_millis(50));
        }
    }
}

/// A side that leaves midway ends the other: the client's close reaches the destination, and
/// the destination's close reaches the client.
#[test]
fn cut_midway() {
    let (tx, closed) = mpsc::channel::<usize>();
    let tx = std::sync::Mutex::new(tx);
    let counter = backend(move |mut s| {
        let mut n = 0;
        let mut b = [0u8; 4096];
        while let Ok(k) = s.read(&mut b) {
            if k == 0 {
                break;
            }
            n += k;
        }
        let _ = tx.lock().unwrap().send(n);
    });
    let a = Akit::start(&socks_config(
        r#"{"clients": ["127.0.0.1"], "allow": ["127.0.0.0/8"]}"#,
    ));
    let mut c = a.dial();
    assert_eq!(socks5(&mut c, 1, &v4(counter)), 0);
    c.write_all(&[1u8; 100_000]).unwrap();
    drop(c);
    let n = closed.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(n <= 100_000, "{n}");

    let closer = backend(|mut s| {
        s.write_all(b"bye").unwrap();
        let _ = s.shutdown(Shutdown::Both);
    });
    let mut c = a.dial();
    assert_eq!(socks5(&mut c, 1, &v4(closer)), 0);
    assert_eq!(expect_closed(&mut c, Duration::from_secs(5)), b"bye");
}

/// A tunnel that carries nothing for `idle_timeout` is closed, one kept busy past it is not; a
/// destination that does not answer within `connect_timeout` gets reply 4 (or 5 where the
/// network refuses it at once).
#[test]
fn idle_and_connect_timeouts() {
    let echo = backend(echo);
    let a = Akit::start(&socks_config(
        r#"{"clients": ["127.0.0.1"], "allow": ["127.0.0.0/8", "10.0.0.0/8"], "idle_timeout": "300ms", "connect_timeout": "300ms"}"#,
    ));
    let mut c = a.dial();
    assert_eq!(socks5(&mut c, 1, &v4(echo)), 0);
    for _ in 0..4 {
        thread::sleep(Duration::from_millis(100));
        expect_echo(&mut c, b"x");
    }
    let start = Instant::now();
    expect_closed(&mut c, Duration::from_secs(3));
    assert!(
        start.elapsed() >= Duration::from_millis(250),
        "{:?}",
        start.elapsed()
    );

    // 10.255.255.1 is unroutable on most hosts.
    let mut c = a.dial();
    let start = Instant::now();
    let code = socks5(&mut c, 1, &[1, 10, 255, 255, 1, 0, 9]);
    assert!(code == 4 || code == 5, "{code}");
    if code == 4 {
        assert!(
            start.elapsed() >= Duration::from_millis(250),
            "{:?}",
            start.elapsed()
        );
    }
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
    expect_closed(&mut c, Duration::from_secs(5));
}

/// Behind a trusted proxy's PROXY header, the client checked is the one the header names.
#[test]
fn proxy_header_names_the_client() {
    let echo = backend(echo);
    let json = r#"{"listeners": [{"name": "s", "address": "127.0.0.1:0", "protocols": ["socks"],
        "proxy_protocol": {"trusted": ["127.0.0.0/8"]},
        "socks": {"clients": ["203.0.113.0/24"], "allow": ["127.0.0.0/8"]}}]}"#;
    let a = Akit::start(json);
    let mut c = a.dial();
    c.write_all(b"PROXY TCP4 203.0.113.9 127.0.0.1 5555 1080\r\n")
        .unwrap();
    assert_eq!(socks5(&mut c, 1, &v4(echo)), 0);
    expect_echo(&mut c, b"proxied");
    let mut c = a.dial();
    c.write_all(b"PROXY TCP4 198.51.100.9 127.0.0.1 5555 1080\r\n")
        .unwrap();
    assert_eq!(socks5(&mut c, 1, &v4(echo)), 2);
}

/// A reload that changes the rules binds the listener anew with them; one that keeps them keeps
/// the listener and its open tunnels.
#[test]
fn reload_changes_the_rules() {
    let echo = backend(echo);
    let open = socks_config(r#"{"clients": ["127.0.0.1"], "allow": ["127.0.0.0/8"]}"#);
    let mut a = Akit::start(&open);
    let mut kept = a.dial();
    assert_eq!(socks5(&mut kept, 1, &v4(echo)), 0);
    a.reload(&open);
    expect_echo(&mut kept, b"kept");
    a.reload(&socks_config(
        r#"{"clients": ["127.0.0.1"], "allow": ["127.0.0.0/8"], "ports": [1]}"#,
    ));
    let mut c = a.dial();
    assert_eq!(socks5(&mut c, 1, &v4(echo)), 2);
    a.reload(&open);
    let mut c = a.dial();
    assert_eq!(socks5(&mut c, 1, &v4(echo)), 0);
    expect_echo(&mut c, b"again");
}
