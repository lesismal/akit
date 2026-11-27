//! fit's `tunnel/hub/socket_test.go`, ported (I103b).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::sync::Arc;
use std::time::Duration;

use super::super::{HubApi, HubCore};
use super::*;
use crate::testutil::{Env, HASH, PAGE_MARK, WsClient, free_port, page_console, wait_for};

fn console(port: u16) -> String {
    format!(
        r#"{{"address": "127.0.0.1:{port}", "users": [{{"username": "admin", "password_hash": "{HASH}"}}]}}"#
    )
}

fn hub_doc(console_port: u16, socket: &str) -> String {
    format!(
        r#"{{"tunnel": {{"hub": {{"password": "pw", "console": {}{socket}}}}}}}"#,
        console(console_port)
    )
}

fn core(env: &Env) -> Arc<HubCore> {
    let api = env.roles.api("tunnel.hub").unwrap();
    Arc::clone(&api.downcast_ref::<HubApi>().unwrap().0)
}

fn state(c: &HubCore) -> Arc<SocketState> {
    Arc::clone(&lock(&c.socket).as_ref().unwrap().state)
}

fn url(c: &HubCore) -> String {
    format!("ws://{}/", c.socket_addr().unwrap())
}

/// fit's `TestConsoleSocket`: the socket carries the console to a WebSocket, the request sent in
/// more than one message before the console is connected; it refuses WebSockets past
/// `max_conns`, a bad hello, and messages once the console is gone.
#[test]
fn console_socket() {
    let con = page_console();
    let socket =
        r#", "console_socket": {"address": "127.0.0.1:0", "password": "s", "max_conns": 1}"#;
    let mut env = Env::start(&[crate::HUB], &hub_doc(con, socket));
    let c = core(&env);
    let w = WsClient::dial(&env.engine, &url(&c));
    let mut sess = w.session("s");
    // The request goes in more than one message, the second while the console is being dialed.
    // Its end waits for the WebSocket past max_conns below.
    w.send(&sess.seal(b"GET / HTTP/1.1\r\n"));
    w.send(&sess.seal(b"Host: x\r\nConnection: close\r\n"));

    // Past max_conns, a WebSocket closes at once.
    let refused = WsClient::dial(&env.engine, &url(&c));
    refused.wait_closed();
    w.send(&sess.seal(b"\r\n"));

    let mut got = Vec::new();
    while !String::from_utf8_lossy(&got).contains(PAGE_MARK) {
        got.extend(sess.open(&w.next()).unwrap());
    }
    assert!(got.starts_with(b"HTTP/1.1 200"), "the page: {got:?}");
    w.wait_closed();
    let st = state(&c);
    wait_for("the WebSocket to go", || st.conns() == 0);

    // A hello of the wrong length.
    let w = WsClient::dial(&env.engine, &url(&c));
    w.next();
    w.send(b"short");
    w.wait_closed();
    wait_for("the WebSocket to go", || st.conns() == 0);

    // Without the console, there is nothing to carry: the console's address now has nothing
    // listening; the socket, on the same address, stays.
    env.apply(&hub_doc(free_port(), socket));
    let w = WsClient::dial(&env.engine, &url(&c));
    let mut sess = w.session("s");
    w.send(&sess.seal(b"GET / HTTP/1.1\r\n\r\n"));
    w.wait_closed();
}

/// fit's `TestConsoleAddr`.
#[test]
fn console_addrs() {
    let s = |h: &str, p: u16| Some((h.to_owned(), p));
    for (addr, want) in [
        ("", None),
        ("no port", None),
        (":80", s("127.0.0.1", 80)),
        ("0.0.0.0:80", s("127.0.0.1", 80)),
        ("[::]:80", s("127.0.0.1", 80)),
        ("10.1.2.3:80", s("10.1.2.3", 80)),
        ("[::1]:80", s("::1", 80)),
        ("example:8080", s("example", 8080)),
    ] {
        assert_eq!(console_addr(addr), want, "{addr:?}");
    }
    assert_eq!(unauth_per_ip(1), 1);
    assert_eq!(unauth_per_ip(8), 2);
    assert_eq!(unauth_per_ip(256), 64);
}

/// fit's `TestSocketUnauthenticated`: WebSockets that do not prove the password cannot hold the
/// socket's places: each is closed past the auth timeout, and one address has at most a quarter
/// of `max_conns` of them open. One that proves it, with an empty message as a web proxy sends,
/// stays, and frees its address's place.
#[test]
fn unauthenticated() {
    let con = page_console();
    let env = Env::start(
        &[crate::HUB],
        &hub_doc(
            con,
            r#", "console_socket": {"address": "127.0.0.1:0", "password": "s", "max_conns": 8}"#,
        ),
    );
    let c = core(&env);
    let st = state(&c);
    let timeout = Duration::from_millis(300);
    *lock(&st.auth_timeout) = timeout;
    // Two of eight from one address; the third closes at once.
    let silent = WsClient::dial(&env.engine, &url(&c));
    let hello = WsClient::dial(&env.engine, &url(&c));
    silent.next();
    hello.next();
    hello.send(&crate::wscrypt::hello());
    wait_for("two WebSockets", || st.conns() == 2);
    let third = WsClient::dial(&env.engine, &url(&c));
    third.wait_closed();
    // Neither proves the password: both close.
    silent.wait_closed();
    hello.wait_closed();

    // A proof keeps a WebSocket past the timeout, and frees its place.
    let w = WsClient::dial(&env.engine, &url(&c));
    let mut sess = w.session("s");
    w.send(&sess.seal(b""));
    wait_for("the proof to count", || {
        lock(&st.inner).unauth.is_empty() && st.conns() == 1
    });
    let (a, b) = (
        WsClient::dial(&env.engine, &url(&c)),
        WsClient::dial(&env.engine, &url(&c)),
    );
    a.next();
    b.next();
    std::thread::sleep(timeout * 2);
    assert!(
        !w.is_closed(),
        "a WebSocket that proved the password closed"
    );
    a.wait_closed();
    b.wait_closed();
    // The proof was not passed to the console: it carries a request still.
    w.send(&sess.seal(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"));
    let plain = sess.open(&w.next()).unwrap();
    assert!(plain.starts_with(b"HTTP/1.1 200"), "{plain:?}");
}

/// fit's `TestSocketAborts`: a message that does not open aborts its WebSocket, and so does the
/// socket's removal every WebSocket it has: ended without a close frame (1006, akio's A238).
#[test]
fn aborts() {
    let con = page_console();
    let socket = r#", "console_socket": {"address": "127.0.0.1:0", "password": "s"}"#;
    let mut env = Env::start(&[crate::HUB], &hub_doc(con, socket));
    let c = core(&env);
    let w = WsClient::dial(&env.engine, &url(&c));
    w.session("s");
    w.send(&[1u8; 32]);
    assert_eq!(w.wait_closed(), Some(1006));

    let w = WsClient::dial(&env.engine, &url(&c));
    w.session("s");
    let st = state(&c);
    wait_for("the WebSocket", || st.conns() == 1);
    env.apply(&hub_doc(con, ""));
    assert_eq!(w.wait_closed(), Some(1006));
    assert!(c.socket_addr().is_none(), "the socket still listens");
    // Its address shows in the status while it listens, not after.
    assert!(!HubApi(Arc::clone(&c)).status().contains("\"socket\":"));
}

/// The socket's password is the hub's when it has none of its own, and a new one applies to the
/// WebSockets that open from then on, the socket listening on.
#[test]
fn the_hubs_password_and_a_new_one() {
    let con = page_console();
    let mut env = Env::start(
        &[crate::HUB],
        &hub_doc(con, r#", "console_socket": {"address": "127.0.0.1:0"}"#),
    );
    let c = core(&env);
    let at = c.socket_addr().unwrap();
    assert!(
        HubApi(Arc::clone(&c))
            .status()
            .contains(&format!("\"socket\":\"{at}\""))
    );
    let w = WsClient::dial(&env.engine, &url(&c));
    let mut sess = w.session("pw");
    w.send(&sess.seal(b"GET / HTTP/1.1\r\n\r\n"));
    assert!(sess.open(&w.next()).unwrap().starts_with(b"HTTP/1.1 200"));

    let addr = format!("127.0.0.1:{}", at.port());
    env.apply(&hub_doc(
        con,
        &format!(r#", "console_socket": {{"address": "{addr}", "password": "new"}}"#),
    ));
    // Another address: listened on anew.
    let at = c.socket_addr().unwrap();
    env.apply(&hub_doc(
        con,
        &format!(r#", "console_socket": {{"address": "{at}", "password": "new2"}}"#),
    ));
    assert_eq!(
        c.socket_addr(),
        Some(at),
        "the same address listened on anew"
    );
    // A new WebSocket takes the new password alone.
    let w = WsClient::dial(&env.engine, &url(&c));
    let mut sess = w.session("new");
    w.send(&sess.seal(b"GET / HTTP/1.1\r\n\r\n"));
    w.wait_closed();
    let w = WsClient::dial(&env.engine, &url(&c));
    let mut sess = w.session("new2");
    w.send(&sess.seal(b"GET / HTTP/1.1\r\n\r\n"));
    assert!(sess.open(&w.next()).unwrap().starts_with(b"HTTP/1.1 200"));
}
