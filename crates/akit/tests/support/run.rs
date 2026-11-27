//! Where `akit run` listens when its config says port 0: what it prints, and what its Admin API's
//! `GET /status` says. A test that picked a free port and handed it to `akit run` raced every other
//! process for it; one that asks where akit listens does not.
#![allow(
    dead_code,
    unreachable_pub,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use akit_config::value::{self, Value};

/// Loopback, port 0: where a config's listener binds a port of its own.
pub const ANY: &str = "127.0.0.1:0";

/// The address in `line`, after `prefix` (`akit: admin API on 127.0.0.1:1`), up to a space or
/// a slash: what `akit run` printed it listens on.
pub fn printed(line: &str, prefix: &str) -> SocketAddr {
    let rest = line
        .strip_prefix(prefix)
        .unwrap_or_else(|| panic!("{line:?} does not start with {prefix:?}"));
    let rest = rest.strip_prefix("http://").unwrap_or(rest);
    let rest = rest.strip_prefix("https://").unwrap_or(rest);
    let end = rest.find([' ', '/']).unwrap_or(rest.len());
    rest[..end]
        .parse()
        .unwrap_or_else(|e| panic!("{line:?}: {e}"))
}

/// The address in `line` after `marker`, wherever it is in the line
/// (`akit: tunnel.hub: console on http://127.0.0.1:1/`).
pub fn after(line: &str, marker: &str) -> SocketAddr {
    let at = line
        .find(marker)
        .unwrap_or_else(|| panic!("no {marker:?} in {line:?}"));
    printed(&line[at..], marker)
}

/// The Admin API's `GET /status`, with `auth` (an `Authorization` value) when given.
pub fn status(api: SocketAddr, auth: Option<&str>) -> Value {
    let mut s = TcpStream::connect(api).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let auth = auth.map_or_else(String::new, |a| format!("Authorization: {a}\r\n"));
    write!(
        s,
        "GET /status HTTP/1.1\r\nHost: {api}\r\n{auth}Connection: close\r\n\r\n"
    )
    .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("HTTP/1.1 200"), "GET /status: {raw}");
    value::parse(body).unwrap_or_else(|e| panic!("GET /status: {e}: {body}"))
}

/// Where listener `name` listens, as the Admin API on `api` says.
pub fn listener(api: SocketAddr, name: &str, auth: Option<&str>) -> SocketAddr {
    let st = status(api, auth);
    let Some(Value::Array(ls)) = st.get("listeners") else {
        panic!("no listeners: {}", st.to_json())
    };
    for l in ls {
        if matches!(l.get("name"), Some(Value::String(n)) if n == name) {
            if let Some(Value::String(a)) = l.get("address") {
                return a.parse().unwrap();
            }
        }
    }
    panic!("no listener {name:?}: {}", st.to_json())
}
