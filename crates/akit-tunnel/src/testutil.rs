//! What the crate's tests of the console socket and the web proxies share (I103b): an engine, the
//! roles on it, a console of the tests' own, and a WebSocket the tests drive.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use akio_http::ws::{MessageRef, WsHandler, WsSender};
use akit_roles::host::{Info, Manager};

use crate::wscrypt::{Secret, Session};

/// A bcrypt hash, for the consoles the configs name.
pub(crate) const HASH: &str = "$2a$10$N9qo8uLOickgx2ZMRZoMyeIjZAgcfl7p92ldGxad68LJZdL17lhWy";

/// What a test console's page holds, as fit's tests look for its workbench.
pub(crate) const PAGE_MARK: &str = "core/workbench.js";

/// A console of the test's own: each request (its head, up to a blank line) answered with a page
/// that names [`PAGE_MARK`], the connection closed after it.
pub(crate) fn page_console() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            std::thread::spawn(move || {
                let mut head = Vec::new();
                let mut b = [0u8; 4096];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut b) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&b[..n]),
                    }
                }
                let body = format!("<html><script src=\"/{PAGE_MARK}\"></script></html>");
                let _ = write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            });
        }
    });
    port
}

/// The roles `infos` on an engine of their own, moved to `doc`.
pub(crate) struct Env {
    pub(crate) engine: Arc<akio::Engine>,
    pub(crate) roles: Manager,
}

impl Drop for Env {
    fn drop(&mut self) {
        self.roles.stop(Duration::ZERO);
        self.engine.shutdown();
        self.engine.wait();
    }
}

impl Env {
    pub(crate) fn start(infos: &[Info], doc: &str) -> Env {
        let engine = akio::Engine::builder().client_threads(2).build().unwrap();
        engine.start().unwrap();
        let engine = Arc::new(engine);
        let mut env = Env {
            roles: Manager::new(infos, Arc::clone(&engine), None),
            engine,
        };
        env.apply(doc);
        env
    }

    pub(crate) fn apply(&mut self, doc: &str) {
        let config = akit_config::from_json(doc, None).unwrap_or_else(|e| panic!("{doc}\n{e}"));
        let errs = self.roles.apply(&config);
        assert!(errs.is_empty(), "{errs:?}");
    }
}

/// Waits up to 10 s for `cond`.
pub(crate) fn wait_for(what: &str, cond: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A WebSocket a test dials and drives: the messages it gets, as they arrive, and its close.
pub(crate) struct WsClient {
    sender: Arc<Mutex<Option<WsSender>>>,
    msgs: Receiver<Vec<u8>>,
    /// The close code its `on_close` got; `None` for a dial that failed too.
    closed: Receiver<Option<u16>>,
}

struct ClientHandler {
    sender: Arc<Mutex<Option<WsSender>>>,
    msgs: Sender<Vec<u8>>,
    closed: Sender<Option<u16>>,
}

impl WsHandler for ClientHandler {
    fn on_open(&mut self, ws: &WsSender) {
        *self.sender.lock().unwrap() = Some(ws.clone());
    }
    fn on_message_ref(&mut self, _ws: &WsSender, msg: MessageRef<'_>) {
        let _ = self.msgs.send(msg.as_bytes().to_vec());
    }
    fn on_close(&mut self, code: Option<u16>, _reason: &str) {
        let _ = self.closed.send(code);
    }
}

impl WsClient {
    /// Dials `url` on `engine`; a dial that fails counts as closed.
    pub(crate) fn dial(engine: &Arc<akio::Engine>, url: &str) -> WsClient {
        let sender = Arc::new(Mutex::new(None));
        let (mt, msgs) = channel();
        let (ct, closed) = channel();
        let ct2 = ct.clone();
        akio_http::ws::client::dial(
            engine,
            url,
            &akio_http::ws::client::DialOptions::default(),
            ClientHandler {
                sender: Arc::clone(&sender),
                msgs: mt,
                closed: ct,
            },
            move |_| {
                let _ = ct2.send(None);
            },
        );
        WsClient {
            sender,
            msgs,
            closed,
        }
    }

    pub(crate) fn next(&self) -> Vec<u8> {
        self.msgs
            .recv_timeout(Duration::from_secs(10))
            .expect("timed out waiting for a message")
    }

    pub(crate) fn send(&self, b: &[u8]) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(ws) = self.sender.lock().unwrap().as_ref() {
                let _ = ws.send_binary(b);
                return;
            }
            assert!(Instant::now() < deadline, "the WebSocket did not open");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Waits for the WebSocket to close: the close code its `on_close` got (1006 for one ended
    /// without a close frame).
    pub(crate) fn wait_closed(&self) -> Option<u16> {
        self.closed
            .recv_timeout(Duration::from_secs(10))
            .expect("the WebSocket did not close")
    }

    /// The messages that arrived and were not taken yet, in order.
    pub(crate) fn received(&self) -> Vec<Vec<u8>> {
        self.msgs.try_iter().collect()
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.try_recv().is_ok()
    }

    /// Exchanges hellos with a console socket of `password`.
    pub(crate) fn session(&self, password: &str) -> Session {
        let hub = self.next();
        let mine = crate::wscrypt::hello();
        self.send(&mine);
        Session::new(&Secret::new(password), &hub, &mine, false).unwrap()
    }
}

/// Reads what `s` sends until it closes, within 10 s. Nothing when the peer already reset it:
/// macOS then refuses the timeout (`EINVAL`).
pub(crate) fn read_to_end(s: &mut TcpStream) -> Vec<u8> {
    if s.set_read_timeout(Some(Duration::from_secs(10))).is_err() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    out
}
