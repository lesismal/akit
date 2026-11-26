//! fit's `conn_test.go`, `conn_edge_test.go`, `limits_test.go` and `token_test.go`, ported: links
//! over akio-ws on one engine.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use akit_config::roles::LinkToken;
use akit_roles::linktoken::{Registry, Secret};

use super::super::session::tests::{H, read_all, streamer};
use super::*;

const WAIT: Duration = Duration::from_secs(10);

pub(crate) fn engine() -> Arc<Engine> {
    let e = Engine::builder().client_threads(2).build().unwrap();
    e.start().unwrap();
    Arc::new(e)
}

fn listener_cfg(creds: Vec<Credential>, tokens: Option<Arc<dyn Tokens>>) -> ListenerConfig {
    ListenerConfig {
        session: Options::default(),
        timing: Timing::default(),
        credentials: creds,
        tokens,
        token_roles: vec!["node".to_owned()],
        max_conns: 0,
        max_pending: 0,
        max_pending_per_ip: 0,
        accept: None,
        handlers: Arc::new(|_| Arc::new(streamer())),
        on_auth_failed: None,
        on_open: None,
        on_close: None,
    }
}

fn listen(engine: &Arc<Engine>, cfg: ListenerConfig) -> (Listener, u16) {
    let l = Listener::new(cfg);
    let id = engine
        .add_listener(l.serve("127.0.0.1:0".parse().unwrap(), None, None))
        .unwrap();
    let port = engine
        .listeners()
        .into_iter()
        .find(|i| i.id == id)
        .unwrap()
        .local_addr
        .port();
    (l, port)
}

fn dial_cfg(password: &str, token: Option<Secret>) -> DialConfig {
    DialConfig {
        session: Options::default(),
        timing: Timing::default(),
        token,
        password: password.to_owned(),
        tls: None,
        server_name: None,
        timeout: Duration::from_secs(5),
        handlers: Arc::new(|_| Arc::new(H::default())),
        on_close: None,
    }
}

fn dial_sync(engine: &Arc<Engine>, port: u16, cfg: DialConfig) -> Result<Arc<Conn>, Error> {
    let (tx, rx) = mpsc::channel();
    let _ = dial(
        engine,
        &format!("ws://127.0.0.1:{port}{PATH}"),
        cfg,
        move |r| {
            let _ = tx.send(r);
        },
    );
    rx.recv_timeout(WAIT).expect("the dial settles")
}

fn creds(pw: &str) -> Vec<Credential> {
    vec![Credential {
        password: pw.to_owned(),
        roles: vec!["node".to_owned()],
    }]
}

#[test]
fn passwords_both_ways_and_streams() {
    let e = engine();
    type Opened = Vec<(Vec<String>, String)>;
    let opened: Arc<Mutex<Opened>> = Arc::default();
    let o = Arc::clone(&opened);
    let mut cfg = listener_cfg(creds("secret"), None);
    cfg.on_open = Some(Arc::new(move |c: &Arc<Conn>| {
        o.lock()
            .unwrap()
            .push((c.roles().to_vec(), c.token().to_owned()));
    }));
    let (_l, port) = listen(&e, cfg);
    let c = dial_sync(&e, port, dial_cfg("secret", None)).unwrap();
    assert_eq!(
        opened.lock().unwrap()[0],
        (vec!["node".to_owned()], String::new())
    );
    // 8 MiB down a stream: the link's pacing and the window both turn.
    let st = c.session().open_stream("/down", b"512").unwrap();
    let (data, r) = read_all(&st).recv_timeout(WAIT).unwrap();
    assert_eq!(r, Ok(()));
    assert_eq!(data.len(), 512 * (16 << 10));
    // A wrong password: refused with 4001.
    assert!(matches!(
        dial_sync(&e, port, dial_cfg("wrong", None)),
        Err(Error::Auth(_))
    ));
    c.close();
    e.shutdown();
}

#[test]
fn tokens_both_ways() {
    let e = engine();
    let sec = Secret::new().unwrap();
    let reg = Registry::default();
    reg.set(&[LinkToken {
        id: sec.id().to_owned(),
        name: "n".to_owned(),
        hash: sec.hash(),
        ..LinkToken::default()
    }]);
    let tok: Arc<Mutex<String>> = Arc::default();
    let t2 = Arc::clone(&tok);
    let mut cfg = listener_cfg(Vec::new(), Some(Arc::new(reg)));
    cfg.on_open = Some(Arc::new(move |c: &Arc<Conn>| {
        *t2.lock().unwrap() = c.token().to_owned();
    }));
    let (_l, port) = listen(&e, cfg);
    let c = dial_sync(&e, port, dial_cfg("", Some(sec.clone()))).unwrap();
    assert_eq!(*tok.lock().unwrap(), sec.id());
    c.close();
    // A token the listener did not issue.
    let other = Secret::new().unwrap();
    match dial_sync(&e, port, dial_cfg("", Some(other))) {
        Err(Error::Auth(m)) => assert!(m.contains("no such token"), "{m}"),
        r => panic!("{r:?}"),
    }
    // A password where only tokens are taken.
    assert!(matches!(
        dial_sync(&e, port, dial_cfg("pw", None)),
        Err(Error::Auth(_))
    ));
    e.shutdown();
}

#[test]
fn refusals_and_bounds() {
    let e = engine();
    let mut cfg = listener_cfg(creds("pw"), None);
    cfg.max_conns = 1;
    let (_l, port) = listen(&e, cfg);
    let c = dial_sync(&e, port, dial_cfg("pw", None)).unwrap();
    assert!(matches!(
        dial_sync(&e, port, dial_cfg("pw", None)),
        Err(Error::Refused(m)) if m.contains("too many")
    ));
    c.close();
    // Another path is not the mesh's.
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(WAIT)).unwrap();
    s.write_all(b"GET /other HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut b = [0u8; 12];
    s.read_exact(&mut b).unwrap();
    assert_eq!(&b, b"HTTP/1.1 404");
    // An accept hook refuses.
    let mut cfg = listener_cfg(creds("pw"), None);
    cfg.accept = Some(Arc::new(|| Err("shutting down".to_owned())));
    let (_l2, port2) = listen(&e, cfg);
    assert!(matches!(
        dial_sync(&e, port2, dial_cfg("pw", None)),
        Err(Error::Refused(m)) if m == "shutting down"
    ));
    e.shutdown();
}

#[test]
fn pending_links_per_address_are_bounded() {
    let e = engine();
    let mut cfg = listener_cfg(creds("pw"), None);
    cfg.max_pending_per_ip = 2;
    let (l, port) = listen(&e, cfg);
    // Two silent connections hold the address's places; a third is closed as it opens.
    let a = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let b = TcpStream::connect(("127.0.0.1", port)).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
    c.set_read_timeout(Some(WAIT)).unwrap();
    let mut buf = [0u8; 1];
    assert!(matches!(c.read(&mut buf), Ok(0) | Err(_)), "closed");
    drop((a, b));
    std::thread::sleep(Duration::from_millis(200));
    // Their places free, a link authenticates.
    let k = dial_sync(&e, port, dial_cfg("pw", None)).unwrap();
    // Closing the listener's links ends it.
    let (tx, rx) = mpsc::channel();
    k.session().on_close(move |_| {
        let _ = tx.send(());
    });
    l.close_all();
    rx.recv_timeout(WAIT).unwrap();
    e.shutdown();
}

#[test]
fn a_silent_peer_is_dropped() {
    let e = engine();
    let mut cfg = listener_cfg(creds("pw"), None);
    cfg.timing = Timing {
        ping: Duration::from_millis(100),
        slow: Duration::from_millis(300),
    };
    let closed: Arc<Mutex<Option<Error>>> = Arc::default();
    let c2 = Arc::clone(&closed);
    cfg.on_close = Some(Arc::new(move |_, err: &Error| {
        *c2.lock().unwrap() = Some(err.clone());
    }));
    let (_l, port) = listen(&e, cfg);
    // A dialer without a heartbeat of its own whose reads are held: the listener's pings go
    // unanswered.
    let mut d = dial_cfg("pw", None);
    d.timing = Timing {
        ping: Duration::ZERO,
        slow: Duration::ZERO,
    };
    let c = dial_sync(&e, port, d).unwrap();
    c.ws.hold_reads(true);
    for _ in 0..100 {
        if closed.lock().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let err = closed.lock().unwrap().clone();
    assert!(
        matches!(err, Some(Error::Slow | Error::Heartbeat)),
        "{err:?}"
    );
    e.shutdown();
}

#[test]
fn a_handshake_that_does_not_finish_is_closed() {
    let e = engine();
    let (_l, port) = listen(&e, listener_cfg(creds("pw"), None));
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
    s.write_all(
        b"GET /ws HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
    )
    .unwrap();
    // The upgrade, the challenge, and then, without a proof, the close (4001) within 5 s.
    let mut all = Vec::new();
    let mut buf = [0u8; 4096];
    let start = Instant::now();
    loop {
        match s.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => all.extend_from_slice(&buf[..n]),
        }
    }
    assert!(start.elapsed() < Duration::from_secs(12));
    let text = String::from_utf8_lossy(&all);
    assert!(text.starts_with("HTTP/1.1 101"), "{text}");
    assert!(all.windows(2).any(|w| w == [0x0f, 0xa1]), "a close of 4001");
    e.shutdown();
}
