//! Listeners from a config on akio: HTTP/1.1 and h2c, routing per listener, and 404. On the
//! backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use akio::{Backend, Engine, UringConfig};
use akio_http::client::ClientRequest;
use akio_http::h2client::{H2Client, H2Options};
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};

fn backend() -> Backend {
    match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    }
}

/// Serves `json`; returns the engine and where each listener listens.
fn serve(json: &str) -> (Engine, Vec<SocketAddr>) {
    let config = akit_config::from_json(json, None).unwrap();
    let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
    let cell = GraphCell::new(compile(&config, &registry, &Env::default()).unwrap());
    let e = akit_listener::engine(&cell, &backend()).unwrap();
    e.start().unwrap();
    let addrs = e.local_addrs();
    (e, addrs)
}

/// An HTTP/1.1 GET; the status line and the body.
fn get(addr: SocketAddr, host: &str, path: &str) -> (String, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    let (head, body) = resp.split_once("\r\n\r\n").unwrap();
    (head.lines().next().unwrap().to_owned(), body.to_owned())
}

/// An HTTP/2 GET with prior knowledge, in the clear; the status and the body.
fn get_h2c(addr: SocketAddr, path: &str) -> Result<(u16, String), String> {
    let client = Engine::builder().client_threads(1).build().unwrap();
    client.start().unwrap();
    let c = H2Client::dial_blocking(&client, addr, None, H2Options::default())
        .map_err(|e| e.to_string())?;
    let r = c
        .send(ClientRequest::get(&format!("http://{addr}{path}")).timeout(Duration::from_secs(5)))
        .wait()
        .map_err(|e| e.to_string())?;
    Ok((r.status, String::from_utf8(r.body).unwrap()))
}

const HELLO: &str = r#"{
  "listeners": [{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}],
  "routes": [
    {"name": "hello", "match": {"path": ["/hello"]},
     "handler": {"module": "respond", "params": {"body": "hello from fit\n"}}},
    {"name": "teapot", "match": {"host": ["tea.example"], "path": ["/hello"]},
     "handler": {"module": "respond", "params": {"status": 418, "headers": {"X-Tea": "1"}}}}
  ]
}"#;

#[test]
fn http1_and_h2c_with_404() {
    let (e, addrs) = serve(HELLO);
    let a = addrs[0];
    assert_eq!(
        get(a, "x", "/hello"),
        ("HTTP/1.1 200 OK".into(), "hello from fit\n".into())
    );
    // fit's answer when no route matches.
    assert_eq!(
        get(a, "x", "/nope"),
        (
            "HTTP/1.1 404 Not Found".into(),
            "404 page not found\n".into()
        )
    );
    // The more specific host wins.
    let (status, body) = get(a, "TEA.example:80", "/hello");
    assert!(status.starts_with("HTTP/1.1 418"), "{status}");
    assert_eq!(body, "");
    assert_eq!(
        get_h2c(a, "/hello").unwrap(),
        (200, "hello from fit\n".into())
    );
    assert_eq!(
        get_h2c(a, "/nope").unwrap(),
        (404, "404 page not found\n".into())
    );
    e.shutdown();
    e.wait();
}

#[test]
fn h2c_upgrade() {
    let (e, addrs) = serve(HELLO);
    let mut s = TcpStream::connect(addrs[0]).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(
        b"GET /hello HTTP/1.1\r\nHost: x\r\nConnection: Upgrade, HTTP2-Settings\r\n\
          Upgrade: h2c\r\nHTTP2-Settings: AAMAAABkAAQCAAAAAAIAAAAA\r\n\r\n",
    )
    .unwrap();
    let mut buf = [0u8; 64];
    let n = s.read(&mut buf).unwrap();
    let head = String::from_utf8_lossy(&buf[..n]);
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    e.shutdown();
    e.wait();
}

/// Without h2c (fit's default protocols), HTTP/2 is not served.
#[test]
fn http1_only_by_default() {
    let (e, addrs) = serve(
        r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}],
            "routes": [{"handler": {"module": "respond", "params": {"body": "one"}}}]}"#,
    );
    assert_eq!(get(addrs[0], "x", "/").1, "one");
    assert!(get_h2c(addrs[0], "/").is_err());
    e.shutdown();
    e.wait();
}

/// A route serves on the listeners it names, or on every one.
#[test]
fn routes_per_listener() {
    let (e, addrs) = serve(
        r#"{"listeners": [{"name": "a", "address": "127.0.0.1:0"},
                          {"name": "b", "address": "127.0.0.1:0"}],
            "routes": [
              {"listeners": ["a"], "match": {"path": ["/x"]},
               "handler": {"module": "respond", "params": {"body": "a"}}},
              {"handler": {"module": "respond", "params": {"body": "all"}}}]}"#,
    );
    assert_eq!(get(addrs[0], "h", "/x").1, "a");
    assert_eq!(get(addrs[1], "h", "/x").1, "all");
    assert_eq!(get(addrs[0], "h", "/y").1, "all");
    e.shutdown();
    e.wait();
}

#[test]
fn h2c_without_http1_is_not_supported_yet() {
    let config = akit_config::from_json(
        r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0", "protocols": ["h2c"]}]}"#,
        None,
    )
    .unwrap();
    let registry: Registry<HandlerParams> = Registry::new(&[akit_handler::MODULES]).unwrap();
    let cell = GraphCell::new(compile(&config, &registry, &Env::default()).unwrap());
    let err = akit_listener::engine(&cell, &backend())
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        "listener web: not supported yet: protocols h2c without http/1.1"
    );
}
