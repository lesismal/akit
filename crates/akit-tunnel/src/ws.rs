//! WebSocket links (I102c, fit's `tunnel/internal/transport/ws`): `ws://host:port/path`, and
//! `wss://` the same over TLS, for networks that let only HTTP through. Plain RFC 6455 on akio-ws:
//! a link's sends go out as binary messages of at most [`MAX_FRAME`] bytes, and the messages that
//! arrive go to the link as if read off a TCP connection. A link is a byte stream, so where its
//! frames fall in messages does not matter, and fit's side and akit's link with each other.
//!
//! A hub's listener serves its path alone (`/tunnel` by default; another path is answered 404),
//! takes any `Origin`, bounds the opening handshake's head at [`MAX_HANDSHAKE`] and its time at
//! [`HANDSHAKE_TIMEOUT`]. A `wss` listener presents the hub's certificate, a dial checks it as a
//! `tls://` dial does (a pin, `ca_file` or the system's roots), never by the address; the TLS's
//! ALPN is `http/1.1`, which carries the handshake, so that a link may pass a TLS-terminating
//! HTTP proxy.
//!
//! Backpressure as on a TCP link (akio's A236): the WebSocket's connection is relayed (its reads
//! do not pause for its own output, fit's `SetWriteBackpressure(false)`), its queue bounded at
//! fit's `LinkQueue` (`set_max_queued_bytes`), and the link's [`Wire`] maps reads held, the queue
//! and its drain onto the WebSocket's own.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use akio_http::ws::{MessageRef, WsConfig, WsHandler, WsSender};

use crate::link::{Link, Wire};

/// The TLS application protocol of `wss`: HTTP/1.1, which carries the opening handshake.
pub const ALPN: &str = "http/1.1";
/// The most of a send one message carries; a longer send (a link's frame of up to 4 MiB) goes in
/// several.
pub const MAX_FRAME: usize = 1 << 20;
/// The longest message a link takes: twice [`MAX_FRAME`], as fit's.
pub const MAX_MESSAGE: usize = 2 * MAX_FRAME;
/// The most bytes of the opening handshake's head.
pub const MAX_HANDSHAKE: usize = 16 << 10;
/// The longest the opening handshake may take.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// The WebSocket settings of a link: [`MAX_MESSAGE`], no compression (a sealed link's bodies do
/// not compress, and fit's offer none).
#[must_use]
pub fn config() -> WsConfig {
    let mut c = WsConfig::default();
    c.max_message_bytes = MAX_MESSAGE;
    c.permessage_deflate = false;
    c
}

/// A link's wire on a WebSocket.
pub struct WsWire {
    ws: WsSender,
    /// Keeps the messages of one send together.
    order: Mutex<()>,
}

impl std::fmt::Debug for WsWire {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WsWire")
            .field("peer", &self.ws.peer_addr())
            .finish_non_exhaustive()
    }
}

impl WsWire {
    /// The wire of `ws`, whose connection no longer stops reading while its own output is queued
    /// (fit's `prepareLink`): its senders pace themselves, each tunnel by its window.
    #[must_use]
    pub fn new(ws: &WsSender) -> WsWire {
        ws.set_relayed(true);
        WsWire {
            ws: ws.clone(),
            order: Mutex::new(()),
        }
    }
}

impl Wire for WsWire {
    fn send(&self, b: &[u8]) -> bool {
        let _g = self.order.lock().unwrap_or_else(PoisonError::into_inner);
        b.chunks(MAX_FRAME).all(|m| self.ws.send_binary(m).is_ok())
    }
    fn close(&self) {
        // At once, what is queued dropped, no close frame (fit's `Abort`; akio's A238).
        self.ws.abort();
    }
    fn close_after_send(&self) {
        self.ws.close(1000, "");
    }
    fn peer(&self) -> String {
        self.ws
            .peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_default()
    }
    fn hold_reads(&self, hold: bool) {
        self.ws.hold_reads(hold);
    }
    fn queued(&self) -> usize {
        self.ws.queued_bytes()
    }
    fn on_drain(&self, f: Box<dyn FnOnce() + Send>) {
        self.ws.on_drain(f);
    }
    fn set_max_queued(&self, n: usize) {
        // Over TCP or TLS, as a link's WebSocket always is.
        let _ = self.ws.set_max_queued_bytes(Some(n));
    }
}

/// Makes the link of a WebSocket that opened, `None` to close it.
pub type Open = Box<dyn FnOnce(&WsSender) -> Option<Arc<Link>> + Send>;

/// akio-ws's handler of a link's WebSocket: the link is made as it opens, by `open`.
pub struct WsLink {
    open: Option<Open>,
    link: Option<Arc<Link>>,
}

impl std::fmt::Debug for WsLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WsLink").finish_non_exhaustive()
    }
}

impl WsLink {
    /// A WebSocket whose link `open` makes.
    #[must_use]
    pub fn new(open: Open) -> WsLink {
        WsLink {
            open: Some(open),
            link: None,
        }
    }
}

/// Why a link's WebSocket closed: a close code that is not a plain end, as fit's `closeErr`.
fn close_err(code: Option<u16>, reason: &str) -> Option<String> {
    match code {
        None | Some(1000 | 1001 | 1005 | 1006) => None,
        Some(c) => Some(format!("ws: closed with {c} {reason}")),
    }
}

impl WsHandler for WsLink {
    fn on_open(&mut self, ws: &WsSender) {
        if let Some(open) = self.open.take() {
            match open(ws) {
                Some(l) => self.link = Some(l),
                // Refused: ended at once, as fit's `opened`.
                None => ws.abort(),
            }
        }
    }

    fn on_message_ref(&mut self, _ws: &WsSender, msg: MessageRef<'_>) {
        if let Some(l) = &self.link {
            l.on_data(msg.as_bytes());
        }
    }

    fn on_close(&mut self, code: Option<u16>, reason: &str) {
        if let Some(l) = self.link.take() {
            l.on_closed(close_err(code, reason));
        }
    }
}

/// The HTTP settings of a hub's WebSocket listener: HTTP/1.1 alone, the handshake's head bounded
/// by [`MAX_HANDSHAKE`] and [`HANDSHAKE_TIMEOUT`].
#[must_use]
pub fn http_config() -> akio_http::HttpConfig {
    let mut c = akio_http::HttpConfig::default();
    c.max_header_bytes = MAX_HANDSHAKE;
    c.max_body_bytes = 0;
    c.h2 = None;
    c.head_timeout = Some(HANDSHAKE_TIMEOUT);
    c.keep_alive_timeout = Some(HANDSHAKE_TIMEOUT);
    c
}

/// How a hub takes the link of a WebSocket that opened from a peer: the link, or `None` to refuse
/// it.
pub type Accept = Arc<dyn Fn(SocketAddr, Arc<dyn Wire>) -> Option<Arc<Link>> + Send + Sync>;

/// A hub's WebSocket service: the handshakes for `path` upgraded, their links taken by `accept`;
/// any other path answered 404.
pub fn service(path: String, accept: Accept) -> impl akio_http::Service {
    move |ctx: &mut akio_http::Context| {
        if ctx.path() != path {
            ctx.text(404, "404 page not found\n");
            return;
        }
        let accept = Arc::clone(&accept);
        let h = WsLink::new(Box::new(move |ws: &WsSender| {
            let peer = ws.peer_addr()?;
            accept(peer, Arc::new(WsWire::new(ws)))
        }));
        let mut o = akio_http::WsOptions::default();
        o.config = config();
        ctx.upgrade_websocket(h, &o);
    }
}

/// The dial options of a link to `url`'s hub: `tls` for `wss://`, the handshake within `within`.
#[must_use]
pub fn dial_options(
    tls: Option<akio::tls::ClientConfig>,
    within: Duration,
) -> akio_http::ws::client::DialOptions {
    let mut o = akio_http::ws::client::DialOptions::default();
    o.config = config();
    o.tls = tls;
    o.handshake_timeout = within;
    o.happy_eyeballs.connect_timeout = within;
    o.headers = vec![("User-Agent".to_owned(), "akit".to_owned())];
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_codes_as_fits() {
        assert_eq!(close_err(None, ""), None);
        assert_eq!(close_err(Some(1000), ""), None);
        assert_eq!(close_err(Some(1006), ""), None);
        assert_eq!(
            close_err(Some(1011), "boom"),
            Some("ws: closed with 1011 boom".to_owned())
        );
    }
}
