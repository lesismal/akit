//! The console socket and the web proxies end to end (I103b): a hub's console carried through its
//! console socket to a proxy's web proxy, both on one akio engine as `akit run` runs them; large
//! transfers both ways, a slow reader holding the console back, connections cut midway. And,
//! when `AKIT_FIT_BIN` names fit's binary, the same against fit's hub and fit's proxy.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use akit_roles::host::Manager;

const HASH: &str = "$2a$10$N9qo8uLOickgx2ZMRZoMyeIjZAgcfl7p92ldGxad68LJZdL17lhWy";

/// A console that sends back what it gets.
fn echo_console() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            std::thread::spawn(move || {
                let mut buf = vec![0u8; 64 << 10];
                loop {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if s.write_all(&buf[..n]).is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }
    });
    port
}

/// What a source console wrote, and whether its connection ended.
#[derive(Default)]
struct Source {
    written: AtomicUsize,
    ended: AtomicBool,
}

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| u8::try_from(i % 251).unwrap()).collect()
}

/// A console that, once its one connection sends a byte (the socket connects to the console on the
/// first bytes a browser sends), writes it `n` bytes of [`pattern`] as fast as they are taken.
fn source_console(n: usize) -> (u16, Arc<Source>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let src = Arc::new(Source::default());
    let s2 = Arc::clone(&src);
    std::thread::spawn(move || {
        let Ok((mut s, _)) = l.accept() else { return };
        let _ = s.read(&mut [0u8; 1]);
        let data = pattern(n);
        for chunk in data.chunks(16 << 10) {
            if s.write_all(chunk).is_err() {
                break;
            }
            s2.written.fetch_add(chunk.len(), Ordering::SeqCst);
        }
        // Its end is when the peer is gone.
        let _ = s.read(&mut [0u8; 1]);
        s2.ended.store(true, Ordering::SeqCst);
    });
    (port, src)
}

struct Env {
    engine: Arc<akio::Engine>,
    roles: Manager,
    web: u16,
}

impl Drop for Env {
    fn drop(&mut self) {
        self.roles.stop(Duration::ZERO);
        self.engine.shutdown();
        self.engine.wait();
    }
}

/// Applies `doc` on `roles`, which must take it.
fn apply(roles: &mut Manager, doc: &str) {
    let config = akit_config::from_json(doc, None).unwrap_or_else(|e| panic!("{doc}\n{e}"));
    let errs = roles.apply(&config);
    assert!(errs.is_empty(), "{errs:?}");
}

/// The port the first web proxy of the proxy of `roles` bound (port 0 in its config).
fn web_port(roles: &Manager) -> u16 {
    roles
        .api("tunnel.proxy")
        .unwrap()
        .downcast_ref::<akit_tunnel::proxy::ProxyApi>()
        .unwrap()
        .web_proxies()
        .first()
        .and_then(|w| w.addr.parse::<std::net::SocketAddr>().ok())
        .expect("the web proxy's address")
        .port()
}

/// A hub whose console is on `console`, its socket's, and a proxy's web proxy to it: the hub
/// first, its socket on port 0, then the proxy given the address the socket bound, its web proxy
/// on port 0 too.
fn start(console: u16) -> Env {
    let hub = format!(
        r#""hub": {{"name": "hub", "password": "pw", "console": {{"address": "127.0.0.1:{console}", "users": [{{"username": "admin", "password_hash": "{HASH}"}}]}},
                   "console_socket": {{"address": "127.0.0.1:0", "password": "sock"}}}}"#
    );
    let engine = akio::Engine::builder().client_threads(2).build().unwrap();
    engine.start().unwrap();
    let engine = Arc::new(engine);
    let mut roles = Manager::new(
        &[akit_tunnel::HUB, akit_tunnel::PROXY],
        Arc::clone(&engine),
        None,
    );
    apply(&mut roles, &format!(r#"{{"tunnel": {{{hub}}}}}"#));
    let sock = roles
        .api("tunnel.hub")
        .unwrap()
        .downcast_ref::<akit_tunnel::hub::HubApi>()
        .unwrap()
        .socket_addr()
        .expect("the console socket's address");
    apply(
        &mut roles,
        &format!(
            r#"{{"tunnel": {{{hub},
              "proxy": {{"name": "me", "web_proxies": [{{"name": "hub", "listen": "127.0.0.1:0", "url": "ws://{sock}/", "password": "sock"}}]}}}}}}"#
        ),
    );
    let web = web_port(&roles);
    Env { engine, roles, web }
}

fn wait_for(what: &str, cond: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn web_conns(env: &Env) -> usize {
    let api = env.roles.api("tunnel.proxy").unwrap();
    api.downcast_ref::<akit_tunnel::proxy::ProxyApi>()
        .unwrap()
        .web_proxies()
        .iter()
        .map(|v| v.conns)
        .sum()
}

/// `n` bytes through the web proxy to an echo console and back, written while they are read, so
/// that both sides' pacing turns many times over.
#[test]
fn large_transfers_both_ways() {
    let env = start(echo_console());
    let mut s = TcpStream::connect(("127.0.0.1", env.web)).unwrap();
    let n = 24 << 20;
    let data = pattern(n);
    let mut w = s.try_clone().unwrap();
    let send = data.clone();
    let writer = std::thread::spawn(move || w.write_all(&send).unwrap());
    let mut got = vec![0u8; n];
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s.read_exact(&mut got).unwrap();
    writer.join().unwrap();
    assert!(got == data, "the bytes came back changed");
    // Several at once.
    let mut more: Vec<TcpStream> = (0..8)
        .map(|_| TcpStream::connect(("127.0.0.1", env.web)).unwrap())
        .collect();
    for s in &mut more {
        s.write_all(&pattern(1 << 20)).unwrap();
    }
    for s in &mut more {
        let mut got = vec![0u8; 1 << 20];
        s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        s.read_exact(&mut got).unwrap();
        assert!(got == pattern(1 << 20));
    }
}

/// A browser that stops reading holds the console back: what the console could write is bounded
/// by the sockets' buffers and the relays' watermarks, not by what it has to send; all of it
/// arrives once the browser reads again.
#[test]
fn a_slow_reader_holds_the_console() {
    let n = 96 << 20;
    let (con, src) = source_console(n);
    let env = start(con);
    let mut s = TcpStream::connect(("127.0.0.1", env.web)).unwrap();
    s.write_all(b"g").unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut first = [0u8; 1];
    s.read_exact(&mut first).unwrap();
    // Read nothing more for a while: the console stops.
    std::thread::sleep(Duration::from_millis(1500));
    let held = src.written.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(500));
    let still = src.written.load(Ordering::SeqCst);
    assert!(
        held < n / 2,
        "the console wrote {held} of {n} to a reader that stopped"
    );
    assert!(
        still <= held.saturating_add(1 << 20),
        "the console went on: {held}, then {still}"
    );
    let mut got = vec![0u8; n - 1];
    s.read_exact(&mut got).unwrap();
    let want = pattern(n);
    assert!(got[..] == want[1..], "the bytes came changed");
}

/// A browser's connection cut midway ends the console's connection, and the WebSocket between;
/// the console's end, cut, ends the browser's.
#[test]
fn cut_connections() {
    let n = 64 << 20;
    let (con, src) = source_console(n);
    let env = start(con);
    let mut s = TcpStream::connect(("127.0.0.1", env.web)).unwrap();
    s.write_all(b"g").unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut part = vec![0u8; 1 << 20];
    s.read_exact(&mut part).unwrap();
    drop(s);
    wait_for("the console's connection to end", || {
        src.ended.load(Ordering::SeqCst)
    });
    wait_for("the web proxy's connection to go", || web_conns(&env) == 0);
    let hub = env.roles.api("tunnel.hub").unwrap();
    let hub = hub.downcast_ref::<akit_tunnel::hub::HubApi>().unwrap();
    assert!(hub.socket_addr().is_some());

    // The console's side cut: the browser's connection ends once it has what was sent.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let env = start(l.local_addr().unwrap().port());
    let mut s = TcpStream::connect(("127.0.0.1", env.web)).unwrap();
    s.write_all(b"hello").unwrap();
    let (mut c, _) = l.accept().unwrap();
    let mut b = [0u8; 5];
    c.read_exact(&mut b).unwrap();
    assert_eq!(&b, b"hello");
    c.write_all(b"bye").unwrap();
    drop(c);
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut got = Vec::new();
    s.read_to_end(&mut got).unwrap();
    assert_eq!(got, b"bye");
}

/// fit's binary running `doc` in a directory of its own, stopped when dropped; the lines it
/// logs, as they come.
struct Fit {
    child: std::process::Child,
    dir: std::path::PathBuf,
    lines: std::sync::mpsc::Receiver<String>,
}

impl Drop for Fit {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Fit {
    /// The address fit logs after `before` (as `ws://127.0.0.1:1234/` or `http://127.0.0.1:1234`)
    /// once it listens there, within 15 s: its listeners bind port 0, as akit's do, so that no
    /// other process takes a port between the test choosing it and fit binding it.
    fn addr(&self, before: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = self
                .lines
                .recv_timeout(left)
                .unwrap_or_else(|e| panic!("fit logged no {before:?}: {e}"));
            if let Some((_, rest)) = line.split_once(before) {
                return rest
                    .split(|c: char| c == '/' || c.is_whitespace())
                    .next()
                    .unwrap()
                    .to_owned();
            }
        }
    }
}

fn fit(bin: &str, name: &str, doc: &str) -> Fit {
    let dir = std::env::temp_dir().join(format!("akit-i103b-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fit.json");
    std::fs::write(&path, doc).unwrap();
    let mut child = std::process::Command::new(bin)
        .args(["run", "-c"])
        .arg(&path)
        .current_dir(&dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::fs::File::create(dir.join("fit.err")).unwrap())
        .spawn()
        .unwrap();
    let out = child.stdout.take().unwrap();
    let (tx, lines) = std::sync::mpsc::channel();
    let name = name.to_owned();
    std::thread::spawn(move || {
        use std::io::BufRead as _;
        for line in std::io::BufReader::new(out).lines() {
            let Ok(line) = line else { break };
            eprintln!("fit {name}: {line}");
            let _ = tx.send(line);
        }
    });
    Fit { child, dir, lines }
}

/// A connection to `port` whose reads time out after `limit`, or `None` for a try that got
/// nothing: refused, or closed already. Until fit's side is up, a web proxy closes what it
/// accepts as its WebSocket's dial is refused, and on macOS a socket's options are refused
/// (`EINVAL`) once its peer has closed it and reset what was sent after.
fn connect_timed(port: u16, limit: Duration) -> Option<TcpStream> {
    let s = TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(limit)).ok()?;
    Some(s)
}

/// Gets `path` through the web proxy on `port` until it answers, within 15 s.
fn get_page(port: u16, path: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(mut s) = connect_timed(port, Duration::from_secs(10)) {
            let _ = write!(
                s,
                "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
            );
            let mut got = Vec::new();
            let _ = s.read_to_end(&mut got);
            let got = String::from_utf8_lossy(&got).into_owned();
            if got.starts_with("HTTP/1.1 ") {
                return got;
            }
        }
        assert!(Instant::now() < deadline, "no page through the web proxy");
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Against fit's binary (`AKIT_FIT_BIN`; skipped without it, as in CI): fit's hub's console
/// through akit's web proxy, and akit's hub's console socket through fit's web proxy, a page and
/// 8 MiB echoed both ways.
#[test]
fn with_fits_hub_and_proxy() {
    let Ok(bin) = std::env::var("AKIT_FIT_BIN") else {
        eprintln!("AKIT_FIT_BIN not set: skipped");
        return;
    };
    // fit's hub, akit's proxy.
    let hub = fit(
        &bin,
        "hub",
        &format!(
            r#"{{"tunnel": {{"hub": {{"name": "hub", "password": "pw", "console": {{"address": "127.0.0.1:0", "users": [{{"username": "admin", "password_hash": "{HASH}"}}]}},
                 "console_socket": {{"address": "127.0.0.1:0", "password": "sock"}}}}}}}}"#
        ),
    );
    let sock = hub.addr("console over an encrypted WebSocket on ws://");
    let engine = akio::Engine::builder().client_threads(2).build().unwrap();
    engine.start().unwrap();
    let engine = Arc::new(engine);
    let mut roles = Manager::new(&[akit_tunnel::PROXY], Arc::clone(&engine), None);
    let doc = format!(
        r#"{{"tunnel": {{"proxy": {{"name": "me", "web_proxies": [{{"name": "hub", "listen": "127.0.0.1:0", "url": "ws://{sock}/", "password": "sock"}}]}}}}}}"#
    );
    apply(&mut roles, &doc);
    let web = web_port(&roles);
    let page = get_page(web, "/");
    assert!(
        page.starts_with("HTTP/1.1 200") && page.contains("core/workbench.js"),
        "fit's console through akit's web proxy: {page:.300}"
    );
    // A larger part of the pages, whole.
    let js = get_page(web, "/core/workbench.js");
    let (head, body) = js.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let len: Option<usize> = head
        .lines()
        .find_map(|l| l.strip_prefix("Content-Length: "))
        .and_then(|v| v.trim().parse().ok());
    if let Some(len) = len {
        assert_eq!(body.len(), len);
    }
    eprintln!(
        "fit's hub, akit's web proxy: the page and workbench.js ({} bytes)",
        body.len()
    );
    roles.stop(Duration::ZERO);
    engine.shutdown();
    engine.wait();

    // akit's hub, fit's proxy: a console of the test's own, an echo.
    let env = start(echo_console());
    let hub = env.roles.api("tunnel.hub").unwrap();
    let sock = hub
        .downcast_ref::<akit_tunnel::hub::HubApi>()
        .unwrap()
        .socket_addr()
        .unwrap();
    let proxy = fit(
        &bin,
        "proxy",
        &format!(
            r#"{{"tunnel": {{"proxy": {{"name": "fit", "web_proxies": [{{"name": "hub", "listen": "127.0.0.1:0", "url": "ws://{sock}/", "password": "sock"}}]}}}}}}"#
        ),
    );
    let web: u16 = proxy
        .addr(" on http://")
        .rsplit_once(':')
        .and_then(|(_, p)| p.parse().ok())
        .expect("the port of fit's web proxy");
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut s = loop {
        if let Some(mut s) = connect_timed(web, Duration::from_secs(5)) {
            let _ = s.write_all(b"ping");
            let mut b = [0u8; 4];
            if s.read_exact(&mut b).is_ok() && &b == b"ping" {
                break s;
            }
        }
        assert!(Instant::now() < deadline, "nothing through fit's web proxy");
        std::thread::sleep(Duration::from_millis(200));
    };
    let n = 8 << 20;
    let data = pattern(n);
    let mut w = s.try_clone().unwrap();
    let send = data.clone();
    let writer = std::thread::spawn(move || w.write_all(&send).unwrap());
    let mut got = vec![0u8; n];
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s.read_exact(&mut got).unwrap();
    writer.join().unwrap();
    assert!(got == data, "the bytes came back changed");
    eprintln!("akit's hub, fit's web proxy: {n} bytes echoed");
}
