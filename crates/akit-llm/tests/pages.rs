//! The LLM roles' pages (I109c) on akit's own roles: a node's and a hub's `console`, served as
//! `akit run` serves them, their methods called by a browser (`/api/rpc`, `/api/stream`), a
//! change written back into the member; the hub's pages reaching its node (`/hub/node/call`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod support;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use support::{Roles, free_port, until};

fn console(port: u16) -> String {
    let hash = akio_crypto::bcrypt::hash(b"pw", akio_crypto::bcrypt::MIN_COST).unwrap();
    format!(
        r#"{{"address": "127.0.0.1:{port}", "users": [{{"username": "admin", "password_hash": "{hash}"}}]}}"#
    )
}

/// One exchange: the status, the head and the body, a chunked one decoded.
fn exchange(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &str,
    body: &str,
) -> (u16, String, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let raw = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let status = head.split(' ').nth(1).unwrap_or("0").parse().unwrap_or(0);
    let body = if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        let mut out = String::new();
        let mut b = body;
        while let Some((size, rest)) = b.split_once("\r\n") {
            let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
            if n == 0 || rest.len() < n {
                break;
            }
            out.push_str(&rest[..n]);
            b = rest[n..].strip_prefix("\r\n").unwrap_or(&rest[n..]);
        }
        out
    } else {
        body.to_owned()
    };
    (status, head.to_owned(), body)
}

/// Logs in: the session's headers.
fn login(addr: SocketAddr) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(addr).is_err() {
        assert!(std::time::Instant::now() < deadline, "no console at {addr}");
        std::thread::sleep(Duration::from_millis(20));
    }
    let (status, head, body) = exchange(
        addr,
        "POST",
        "/api/login",
        "",
        r#"{"username": "admin", "password": "pw"}"#,
    );
    assert_eq!(status, 200, "{body}");
    let session = head
        .lines()
        .find_map(|l| l.strip_prefix("Set-Cookie: "))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let csrf = body
        .split("\"csrf\":\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    format!("Cookie: {session}\r\nX-Akit-Csrf: {csrf}\r\n")
}

/// A node's pages: who it is, its methods, a model saved into its member, a job followed as
/// Server-Sent Events.
#[test]
fn a_nodes_pages() {
    // The console's port is named before it binds: the identity's `http_url` is the console's
    // address as its member writes it (fit's), none for port 0.
    let port = free_port();
    let roles = Roles::start(&format!(
        r#"{{"llm": {{"node": {{"id": "node-0000000000000021", "name": "box", "console": {}}}}}}}"#,
        console(port)
    ));
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let auth = login(addr);
    let (status, _, body) = exchange(addr, "GET", "/api/workbench", &auth, "");
    assert_eq!(status, 200);
    assert!(body.contains(r#""modules":["llm.node"]"#), "{body}");
    let (status, _, body) = exchange(addr, "GET", "/api/me", &auth, "");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""user":"admin""#), "{body}");
    assert!(
        body.contains(r#""role":"node""#) && body.contains("node-0000000000000021"),
        "{body}"
    );
    let (status, _, body) = exchange(addr, "POST", "/api/rpc/mesh/status", &auth, "");
    assert_eq!(status, 200, "{body}");
    // The identity says where the console is (I109d), and the mesh's methods answer.
    assert!(
        body.contains(&format!(r#""http_url":"http://127.0.0.1:{port}/""#)),
        "{body}"
    );
    for m in ["/mesh/peers", "/mesh/limits/get", "/mesh/tokens/list"] {
        let (status, _, body) = exchange(addr, "POST", &format!("/api/rpc{m}"), &auth, "");
        assert_eq!(status, 200, "{m}: {body}");
    }
    let (status, _, body) = exchange(
        addr,
        "POST",
        "/api/rpc/mesh/undial",
        &auth,
        r#"{"addr": "127.0.0.1:9"}"#,
    );
    assert!(
        status == 404 && body.contains(r#""code":3000"#),
        "{status} {body}"
    );
    let (status, _, body) = exchange(
        addr,
        "POST",
        "/api/m/llm.node/rpc/node/model/list",
        &auth,
        "",
    );
    assert_eq!((status, body.trim()), (200, "[]"));
    // A model saved on the page is written back into the member.
    let (status, _, body) = exchange(
        addr,
        "POST",
        "/api/rpc/node/model/save",
        &auth,
        r#"{"name": "m1", "api": "openai", "base_url": "http://127.0.0.1:1/v1"}"#,
    );
    assert_eq!(status, 200, "{body}");
    until("the model in the member", || {
        roles.member("llm.node").contains("\"m1\"")
    });
    let (status, _, body) = exchange(
        addr,
        "POST",
        "/api/rpc/node/model/save",
        &auth,
        r#"{"name": "m2", "api": "nope"}"#,
    );
    assert!(
        status == 400 && body.contains(r#""code":1000"#),
        "{status} {body}"
    );
    let (status, _, body) = exchange(addr, "POST", "/api/rpc/node/nope", &auth, "");
    assert_eq!(status, 404, "{body}");
    // A stream method: a job that does not exist ends before any frame.
    let (status, _, body) = exchange(
        addr,
        "POST",
        "/api/stream/node/job/follow",
        &auth,
        r#"{"id": 999}"#,
    );
    assert!(
        status == 404 && body.contains(r#""code":3000"#),
        "{status} {body}"
    );
    roles.roles.lock().unwrap().stop(Duration::ZERO);
    assert!(
        TcpStream::connect(addr).is_err(),
        "the console outlived its role"
    );
}

/// A hub's pages reach its node: `/hub/nodes` lists it, `/hub/node/call` calls its method.
#[test]
fn a_hubs_pages_reach_its_node() {
    let (roles, _) = Roles::start_with(|p| {
        format!(
            r#"{{"llm": {{
          "hub": {{"id": "hub-0000000000000031", "name": "hub", "console": {},
                   "listen": {{"address": "127.0.0.1:0", "node_password": "pw"}}}},
          "node": {{"id": "node-0000000000000032", "name": "box",
                    "settings": {{"allow_hub_manage": true, "gpu_sudo": false}},
                    "peers": [{{"address": "127.0.0.1:{}", "password": "pw", "role": "hub"}}]}}}}}}"#,
            console(0),
            p.hub
        )
    });
    let addr = SocketAddr::from(([127, 0, 0, 1], roles.console_port("llm.hub")));
    let auth = login(addr);
    until("the node in the hub's list", || {
        let (_, _, body) = exchange(addr, "POST", "/api/rpc/hub/nodes", &auth, "");
        body.contains(r#""online":true"#)
    });
    let (status, _, body) = exchange(
        addr,
        "POST",
        "/api/rpc/hub/node/call",
        &auth,
        r#"{"id": "node-0000000000000032", "method": "/node/model/list", "req": {}}"#,
    );
    assert_eq!(status, 200, "{body}");
    drop(roles);
}
