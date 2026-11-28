//! fit's `tunnel/proxy/webproxy_test.go`, ported (I103b).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use akio_http::ws::{MessageRef, WsHandler, WsSender};

use crate::proxy::ProxyApi;
use crate::testutil::{Env, HASH, PAGE_MARK, page_console, read_to_end, wait_for};
use crate::wscrypt::{Secret, Session};

/// Gets `/` through the web proxy on `port`, on a connection of its own: the status line's code,
/// or 0 when nothing came back.
fn proxy_get(port: u16) -> (u16, String) {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) else {
        return (0, String::new());
    };
    let _ = s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    let got = String::from_utf8_lossy(&read_to_end(&mut s)).into_owned();
    let code = got
        .strip_prefix("HTTP/1.1 ")
        .and_then(|r| r.get(..3))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    (code, got)
}

/// Dials `port` and expects the connection closed without a byte.
fn closed_soon(port: u16) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    match s.read(&mut [0u8; 1]) {
        Ok(0) => {}
        Err(e)
            if e.kind() != std::io::ErrorKind::WouldBlock
                && e.kind() != std::io::ErrorKind::TimedOut => {}
        r => panic!("{port}: {r:?}"),
    }
}

fn web_proxies(env: &Env) -> Vec<super::WebProxyView> {
    let api = env.roles.api("tunnel.proxy").unwrap();
    api.downcast_ref::<ProxyApi>().unwrap().web_proxies()
}

/// The port the first web proxy bound (its `listen` says port 0).
fn web_port(env: &Env) -> u16 {
    web_proxies(env)[0]
        .addr
        .parse::<std::net::SocketAddr>()
        .expect("the web proxy's address")
        .port()
}

/// Where the hub's console socket listens (its `address` says port 0).
fn socket_port(env: &Env) -> u16 {
    let api = env.roles.api("tunnel.hub").unwrap();
    api.downcast_ref::<crate::hub::HubApi>()
        .unwrap()
        .socket_addr()
        .expect("the console socket's address")
        .port()
}

/// The hub (its console on `console`, its socket on port 0) and the proxy with `proxies`.
fn doc(console: u16, proxies: &str) -> String {
    format!(
        r#"{{"tunnel": {{
          "hub": {{"name": "hub", "password": "pw", "console": {{"address": "127.0.0.1:{console}", "users": [{{"username": "admin", "password_hash": "{HASH}"}}]}},
                   "console_socket": {{"address": "127.0.0.1:0", "password": "sock"}}}},
          "proxy": {{"name": "me", "web_proxies": [{proxies}]}}}}}}"#
    )
}

/// fit's `TestWebProxy`: a web proxy serves the hub's pages, takes a new password and a new limit
/// for the connections it accepts from then on, and closes its connections when it is removed.
#[test]
fn web_proxy() {
    let con = page_console();
    // The hub first, its socket on port 0; then the web proxy, given the port the socket bound,
    // on port 0 too.
    let mut env = Env::start(&[crate::HUB, crate::PROXY], &doc(con, ""));
    let sock = socket_port(&env);
    let pm = |password: &str, conns: usize| {
        format!(
            r#"{{"name": "hub", "listen": "127.0.0.1:0", "url": "ws://127.0.0.1:{sock}/", "password": "{password}", "max_conns": {conns}}}"#
        )
    };
    env.apply(&doc(con, &pm("sock", 0)));
    let port = web_port(&env);
    let (code, body) = proxy_get(port);
    assert_eq!(code, 200, "{body}");
    assert!(body.contains(PAGE_MARK), "the page: {body}");
    let v = web_proxies(&env);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].name, "hub");
    assert_eq!(v[0].addr, format!("127.0.0.1:{port}"));
    assert_eq!(v[0].error, "");

    // Another password: the hub opens nothing the proxy sends.
    env.apply(&doc(con, &pm("wrong", 0)));
    assert_eq!(web_port(&env), port, "the listener kept");
    let (code, _) = proxy_get(port);
    assert_eq!(code, 0, "a wrong password got an answer");

    // One connection at most: a second is closed at once.
    env.apply(&doc(con, &pm("sock", 1)));
    assert_eq!(web_port(&env), port, "the listener kept");
    let mut first = TcpStream::connect(("127.0.0.1", port)).unwrap();
    wait_for("the first connection", || {
        let v = web_proxies(&env);
        v.len() == 1 && v[0].conns == 1
    });
    closed_soon(port);

    // Removed, it closes the connection it has.
    env.apply(&doc(con, ""));
    first
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    assert!(
        matches!(first.read(&mut [0u8; 1]), Ok(0) | Err(_)),
        "the connection of a removed proxy"
    );
    assert!(web_proxies(&env).is_empty());

    // A socket nothing listens on (port 1): each connection is closed.
    env.apply(&doc(
        con,
        r#"{"listen": "127.0.0.1:0", "url": "ws://127.0.0.1:1/", "password": "sock"}"#,
    ));
    closed_soon(web_port(&env));
}

/// fit's `TestWebProxyBadURL`: a web proxy whose URL cannot be dialed closes each connection.
#[test]
fn bad_url() {
    let env = Env::start(
        &[crate::PROXY],
        r#"{"tunnel": {"proxy": {"name": "me", "web_proxies": [{"listen": "127.0.0.1:0", "url": "ws://127.0.0.1:1/", "password": "p"}]}}}"#,
    );
    closed_soon(web_port(&env));
    drop(env);
}

/// What a WebSocket server of the test's own hears.
enum Event {
    Open(WsSender),
    Msg(Vec<u8>),
    Close,
}

struct Recorder(Sender<Event>);

impl WsHandler for Recorder {
    fn on_open(&mut self, ws: &WsSender) {
        let _ = self.0.send(Event::Open(ws.clone()));
    }
    fn on_message_ref(&mut self, _ws: &WsSender, msg: MessageRef<'_>) {
        let _ = self.0.send(Event::Msg(msg.as_bytes().to_vec()));
    }
    fn on_close(&mut self, _code: Option<u16>, _reason: &str) {
        let _ = self.0.send(Event::Close);
    }
}

/// A WebSocket server of the test's own on `engine`, in place of a hub's console socket: its
/// port, and what it hears.
fn ws_server(engine: &Arc<akio::Engine>) -> (u16, Receiver<Event>) {
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let svc = move |ctx: &mut akio_http::Context| {
        let h = Recorder(tx.lock().unwrap().clone());
        ctx.upgrade_websocket(h, &akio_http::WsOptions::default());
    };
    let id = engine
        .add_listener(
            akio::Listener::tcp("127.0.0.1:0".parse().unwrap())
                .threads(1)
                .serve(akio_http::serve_with(crate::ws::http_config(), svc)),
        )
        .unwrap();
    let port = engine
        .listeners()
        .iter()
        .find(|i| i.id == id)
        .unwrap()
        .local_addr
        .port();
    (port, rx)
}

fn next(rx: &Receiver<Event>) -> Event {
    rx.recv_timeout(Duration::from_secs(10))
        .expect("timed out waiting for the server")
}

fn open(rx: &Receiver<Event>) -> WsSender {
    match next(rx) {
        Event::Open(ws) => ws,
        _ => panic!("not an open"),
    }
}

fn msg(rx: &Receiver<Event>) -> Vec<u8> {
    match next(rx) {
        Event::Msg(m) => m,
        _ => panic!("not a message"),
    }
}

fn closed(rx: &Receiver<Event>) {
    loop {
        if let Event::Close = next(rx) {
            return;
        }
    }
}

/// A proxy whose web proxy (on port 0) dials the WebSocket server on `ws`.
fn proxy_to(ws: u16) -> String {
    format!(
        r#"{{"tunnel": {{"proxy": {{"name": "me", "web_proxies": [{{"listen": "127.0.0.1:0", "url": "ws://127.0.0.1:{ws}/", "password": "sock"}}]}}}}}}"#
    )
}

/// fit's `TestProxyConnSession`: what the local connection sends before the session is up waits
/// for it, and then goes sealed, in order, after the proof of the password; what the socket sends
/// is opened, and one message that does not open ends it, and the local connection with it.
#[test]
fn conn_session() {
    let engine = Arc::new(akio::Engine::builder().client_threads(2).build().unwrap());
    engine.start().unwrap();
    let (ws_port, rx) = ws_server(&engine);
    let env = Env::start(&[crate::PROXY], &proxy_to(ws_port));
    let port = web_port(&env);
    let secret = Secret::new("sock");

    let mut local = TcpStream::connect(("127.0.0.1", port)).unwrap();
    local
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    local.write_all(b"early").unwrap();
    let ws = open(&rx);
    let server_hello = crate::wscrypt::hello();
    ws.send_binary(&server_hello).unwrap();
    let client_hello = msg(&rx);
    let mut srv = Session::new(&secret, &server_hello, &client_hello, true).unwrap();
    // The proof of the password comes first: an empty message.
    assert_eq!(srv.open(&msg(&rx)).unwrap(), b"");
    assert_eq!(srv.open(&msg(&rx)).unwrap(), b"early");
    local.write_all(b"later").unwrap();
    assert_eq!(srv.open(&msg(&rx)).unwrap(), b"later");

    ws.send_binary(&srv.seal(b"reply")).unwrap();
    let mut got = [0u8; 5];
    local.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"reply");
    ws.send_binary(b"not sealed").unwrap();
    closed(&rx);
    assert_eq!(read_to_end(&mut local), b"");
    drop(env);
    engine.shutdown();
    engine.wait();
}

/// fit's `TestProxyConnClosed`: a local connection that closes before its session is up closes the
/// WebSocket (once its reads, held until then, see its end); a hello that is not one ends the
/// socket, and the local connection.
#[test]
fn conn_closed() {
    let engine = Arc::new(akio::Engine::builder().client_threads(2).build().unwrap());
    engine.start().unwrap();
    let (ws_port, rx) = ws_server(&engine);
    let env = Env::start(&[crate::PROXY], &proxy_to(ws_port));
    let port = web_port(&env);

    let local = TcpStream::connect(("127.0.0.1", port)).unwrap();
    drop(local);
    // Its reads held, the local connection's end is read once the session is up.
    let ws = open(&rx);
    ws.send_binary(&crate::wscrypt::hello()).unwrap();
    closed(&rx);
    wait_for("the connection to go", || web_proxies(&env)[0].conns == 0);
    let mut local = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let ws = open(&rx);
    ws.send_binary(b"short").unwrap();
    closed(&rx);
    assert_eq!(read_to_end(&mut local), b"");
    drop(env);
    engine.shutdown();
    engine.wait();
}

/// A sealed message the WebSocket refuses (here its queue bounded below one message, so the send
/// fails with `Overloaded`, as akio refuses one past its bound, A305) ends the WebSocket and the
/// local connection: its number is not skipped, so the socket never gets a message that does not
/// open, nor waits on a WebSocket that sends nothing more.
#[test]
fn a_message_not_sent_ends_the_websocket() {
    let engine = Arc::new(akio::Engine::builder().client_threads(2).build().unwrap());
    engine.start().unwrap();
    let (ws_port, rx) = ws_server(&engine);
    let env = Env::start(&[crate::PROXY], &proxy_to(ws_port));
    let port = web_port(&env);

    let mut local = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let ws = open(&rx);
    let server_hello = crate::wscrypt::hello();
    ws.send_binary(&server_hello).unwrap();
    let client_hello = msg(&rx);
    let mut srv = Session::new(&Secret::new("sock"), &server_hello, &client_hello, true).unwrap();
    assert_eq!(srv.open(&msg(&rx)).unwrap(), b"");
    // The proxy's end of the WebSocket, its queue bounded below a message of 4 KiB.
    let api = env.roles.api("tunnel.proxy").unwrap();
    let core = &api.downcast_ref::<ProxyApi>().unwrap().0;
    let proxy = Arc::clone(&super::lock(&core.webs.list)[0]);
    let conn = super::lock(&proxy.inner)
        .conns
        .values()
        .next()
        .cloned()
        .unwrap();
    let sender = super::lock(&conn.st).ws.clone().unwrap();
    sender.set_max_queued_bytes(Some(64)).unwrap();
    // Refused; then one small enough to go, were the first one's number skipped.
    local.write_all(&[7u8; 4 << 10]).unwrap();
    let _ = local.write_all(b"x");
    loop {
        match next(&rx) {
            Event::Msg(m) => assert!(srv.open(&m).is_ok(), "a message that does not open"),
            Event::Close => break,
            Event::Open(_) => panic!("another WebSocket"),
        }
    }
    assert_eq!(read_to_end(&mut local), b"");
    drop(env);
    engine.shutdown();
    engine.wait();
}
