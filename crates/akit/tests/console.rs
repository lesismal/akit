//! End to end: the console (I90) under `akit run`: started beside the Admin API where
//! `admin.console` says, a login and a config changed through it, each in the `admin` logger;
//! `admin.console.access`, then the global access, at accept, following every config applied.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

mod support;

use support::run::{ANY, after, listener, printed};

/// A directory of the test's own, removed when it ends.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        Dir::under(&PathBuf::from(env!("CARGO_TARGET_TMPDIR")), name)
    }

    fn under(base: &Path, name: &str) -> Dir {
        let d = base.join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Akit {
    child: Child,
    out: mpsc::Receiver<String>,
    /// Where the Admin API listens, as `akit run` printed it.
    api: SocketAddr,
    /// Where `admin.console` listens, as `akit run` printed it.
    console: SocketAddr,
    /// The lines read and not yet waited for: what a role printed as it started may come before
    /// the line a test waits for first.
    seen: std::cell::RefCell<Vec<String>>,
}

impl Drop for Akit {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Akit {
    fn run(config: &Path, flags: &[&str]) -> Akit {
        let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
            .arg("run")
            .args(flags)
            .args(["-shutdown-timeout", "5s", "-c"])
            .arg(config)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, out) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut a = Akit {
            child,
            out,
            api: ([127, 0, 0, 1], 0).into(),
            console: ([127, 0, 0, 1], 0).into(),
            seen: std::cell::RefCell::default(),
        };
        a.api = printed(&a.until("akit: admin API on"), "akit: admin API on ");
        a.console = printed(&a.until("akit: console on"), "akit: console on ");
        a
    }

    /// The first line that holds `prefix` (the log's lines start with their time), waited for
    /// unless one was read already.
    fn until(&self, prefix: &str) -> String {
        let mut seen = self.seen.borrow_mut();
        if let Some(i) = seen.iter().position(|l| l.contains(prefix)) {
            return seen.remove(i);
        }
        loop {
            let line = self
                .out
                .recv_timeout(Duration::from_secs(10))
                .unwrap_or_else(|e| panic!("waiting for {prefix:?} after {seen:?}: {e}"));
            if line.contains(prefix) {
                return line;
            }
            seen.push(line);
        }
    }
}

/// Waits until `f` holds, for at most 10 seconds.
fn eventually(what: &str, f: impl Fn() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < Duration::from_secs(10), "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One request to the console on a connection of its own: the status, the `Set-Cookie` field and
/// the body.
fn call(
    console: SocketAddr,
    method: &str,
    path: &str,
    headers: &str,
    body: &str,
) -> (u16, String, String) {
    let Ok(mut s) = TcpStream::connect(console) else {
        return (0, String::new(), String::new());
    };
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    // A connection the console closes as it is accepted may refuse the write.
    let _ = write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: {console}\r\nConnection: close\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut raw = String::new();
    let _ = s.read_to_string(&mut raw);
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let status = head.split(' ').nth(1).unwrap_or("0").parse().unwrap_or(0);
    let cookie = head
        .lines()
        .find_map(|l| l.strip_prefix("Set-Cookie: "))
        .unwrap_or("")
        .to_owned();
    (status, cookie, body.to_owned())
}

/// The Admin API, `admin.console` and listener `web`, each on port 0, with `members`.
fn config(hash: &str, members: &str, console_extra: &str) -> String {
    format!(
        r#"{{"admin": {{"address": "{ANY}", "console": {{"address": "{ANY}",
      "users": [{{"username": "ada", "password_hash": "{hash}"}}]{console_extra}}}}},
  "listeners": [{{"name": "web", "address": "{ANY}"}}],
  "routes": [{{"handler": {{"module": "respond", "params": {{"body": "hi"}}}}}}]{members}}}"#
    )
}

#[test]
fn console_under_akit_run() {
    let d = Dir::new("e2e-console");
    let hash = akio_crypto::bcrypt::hash(b"pw", 4).unwrap();
    let path = d.0.join("akit.json");
    std::fs::write(&path, config(&hash, "", "")).unwrap();
    let akit = Akit::run(&path, &[]);
    let (api, console) = (akit.api, akit.console);
    let web = listener(api, "web", None);

    let (status, cookie, body) = call(
        console,
        "POST",
        "/api/login",
        "",
        r#"{"username": "ada", "password": "pw"}"#,
    );
    assert_eq!(status, 200, "{body}");
    akit.until("console: \"ada\" logged in from 127.0.0.1");
    let session = cookie.split(';').next().unwrap().to_owned();
    let csrf = body
        .split("\"csrf\":\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    let auth = format!("Cookie: {session}\r\nX-Akit-Csrf: {csrf}\r\n");

    // A config changed through the console runs, and says who changed it.
    let next = config(&hash, "", "").replace("\"hi\"", "\"changed\"");
    let (status, _, body) = call(console, "POST", "/api/m/admin/load", &auth, &next);
    assert_eq!(status, 200, "{body}");
    akit.until("console: \"ada\": POST /load applied config version 2");
    eventually("the change served", || {
        let mut s = TcpStream::connect(web).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let _ = s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        let mut r = String::new();
        let _ = s.read_to_string(&mut r);
        r.ends_with("changed")
    });

    // The global access applies to the console, which then closes a refused client at accept;
    // admin.console.access's allow, deciding first, keeps local access.
    let deny = config(&hash, r#", "access": {"deny": ["127.0.0.0/8"]}"#, "").replacen(
        &format!("\"admin\": {{\"address\": \"{ANY}\""),
        &format!("\"admin\": {{\"address\": \"{ANY}\", \"access\": {{\"allow\": [\"127.0.0.1\"]}}"),
        1,
    );
    let (status, _, body) = call(console, "POST", "/api/m/admin/load", &auth, &deny);
    assert_eq!(status, 200, "{body}");
    eventually("the console refuses a denied client", || {
        call(console, "GET", "/api/session", "", "").0 == 0
    });
    let keep = deny.replace(
        r#""users""#,
        r#""access": {"allow": ["127.0.0.1"]}, "users""#,
    );
    // Through the Admin API, which admin.access keeps reachable.
    let mut s = TcpStream::connect(api).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "POST /load HTTP/1.1\r\nHost: {api}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{keep}",
        keep.len()
    )
    .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    assert!(raw.starts_with("HTTP/1.1 200"), "{raw}");
    eventually("admin.console.access keeps local access", || {
        call(console, "GET", "/api/session", &auth, "").0 == 200
    });
    drop(akit);
}

/// Where the tunnel hub's listener `i` listens, as its status on `console` says.
fn hub_listen(console: SocketAddr, admin: &str, i: usize) -> SocketAddr {
    let (status, _, body) = call(console, "GET", "/api/m/tunnel.hub/status", admin, "");
    assert_eq!(status, 200, "{body}");
    let st = akit_config::value::parse(&body).unwrap();
    let Some(akit_config::value::Value::Array(listen)) = st.get("listen") else {
        panic!("{body}")
    };
    let Some(akit_config::value::Value::String(a)) = listen.get(i) else {
        panic!("{body}")
    };
    a.parse().unwrap()
}

/// A logged-in session's headers for `console`: its cookie and CSRF token.
fn session(console: SocketAddr, user: &str) -> String {
    let (status, cookie, body) = call(
        console,
        "POST",
        "/api/login",
        "",
        &format!(r#"{{"username": "{user}", "password": "pw"}}"#),
    );
    assert_eq!(status, 200, "{body}");
    let session = cookie.split(';').next().unwrap().to_owned();
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

/// End to end (I109): `akit run` serves the hub's own console with its module, and
/// `admin.console` shows the hub's module beside the config's; a change made on the hub's page is
/// written back into the config, a version the Admin API lists.
#[test]
fn role_pages_under_akit_run() {
    let d = Dir::new("e2e-role-pages");
    let hash = akio_crypto::bcrypt::hash(b"pw", 4).unwrap();
    // Every listener on port 0; the one added on the hub's page on localhost's, to tell it apart.
    let extra = "localhost:0";
    let hub = format!(
        r#", "tunnel": {{"hub": {{"name": "hub", "password": "pw",
      "listeners": [{{"protocol": "tcp", "address": "{ANY}"}}],
      "console": {{"address": "{ANY}", "users": [{{"username": "hal", "password_hash": "{hash}"}}]}},
      "console_socket": {{"address": "{ANY}", "password": "sock"}}}}}}"#
    );
    let path = d.0.join("akit.json");
    std::fs::write(&path, config(&hash, &hub, "")).unwrap();
    let akit = Akit::run(&path, &[]);
    let console = akit.console;
    let sock = after(
        &akit.until("tunnel.hub: the console over an encrypted WebSocket on"),
        "WebSocket on ws://",
    );
    let hub_console = after(&akit.until("tunnel.hub: console on"), "console on ");

    // admin.console: the hub's module beside the config's.
    let admin = session(console, "ada");
    let (status, _, body) = call(console, "GET", "/api/workbench", &admin, "");
    assert_eq!(status, 200, "{body}");
    assert!(
        body.contains(r#""modules":["admin","tunnel.hub"]"#),
        "{body}"
    );
    let (status, _, body) = call(console, "GET", "/api/m/tunnel.hub/status", &admin, "");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(&format!(r#""socket":"{sock}""#)), "{body}");

    // The hub's console: its own users, its module; a listener added there is written back.
    let (status, _, _) = call(hub_console, "GET", "/api/status", &admin, "");
    assert_eq!(status, 401);
    let hal = session(hub_console, "hal");
    let (status, _, body) = call(hub_console, "GET", "/api/workbench", &hal, "");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""modules":["tunnel.hub"]"#), "{body}");
    let (status, _, body) = call(
        hub_console,
        "POST",
        "/api/listeners",
        &hal,
        &format!(r#"{{"protocol": "tcp", "address": "{extra}"}}"#),
    );
    assert_eq!(status, 200, "{body}");
    // The hub's status lists it where it bound it, second.
    let added = hub_listen(console, &admin, 1);
    assert!(TcpStream::connect(added).is_ok(), "{added} not listening");
    let (status, _, body) = call(console, "GET", "/api/m/admin/config", &admin, "");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(extra), "{body}");
    let (status, _, body) = call(console, "GET", "/api/m/admin/versions", &admin, "");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""current":2"#), "{body}");

    drop(akit);
}

/// End to end (I109b): `akit run` serves the tunnel node's and proxy's own consoles, and
/// `admin.console` shows their modules; a change on the proxy's page is written back into the
/// config, and one its role cannot apply (a forward on an address in use) is answered 400 with
/// why, as fit's.
#[test]
fn node_and_proxy_pages_under_akit_run() {
    let d = Dir::new("e2e-peer-pages");
    let hash = akio_crypto::bcrypt::hash(b"pw", 4).unwrap();
    // The hub first, on port 0; then the node and the proxy, loaded through the Admin API,
    // linking to the port it bound, their consoles on port 0 too.
    let hub = format!(
        r#""hub": {{"name": "hub", "password": "pw", "listeners": [{{"protocol": "tcp", "address": "{ANY}"}}]}}"#
    );
    let path = d.0.join("akit.json");
    std::fs::write(
        &path,
        config(&hash, &format!(r#", "tunnel": {{{hub}}}"#), ""),
    )
    .unwrap();
    let akit = Akit::run(&path, &[]);
    let (api, console) = (akit.api, akit.console);
    let admin = session(console, "ada");
    let link = hub_listen(console, &admin, 0);
    let up = format!(
        r#"{{"name": "hub", "endpoints": [{{"address": "{link}"}}], "password": "pw", "retry_interval": "100ms"}}"#
    );
    let members = format!(
        r#", "tunnel": {{{hub},
      "node": {{"name": "lab", "hubs": [{up}], "console": {{"address": "{ANY}", "users": [{{"username": "nel", "password_hash": "{hash}"}}]}}}},
      "proxy": {{"name": "me", "hubs": [{up}], "console": {{"address": "{ANY}", "users": [{{"username": "pat", "password_hash": "{hash}"}}]}}}}}}"#
    );
    let full = config(&hash, &members, "");
    let mut s = TcpStream::connect(api).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "POST /load HTTP/1.1\r\nHost: {api}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{full}",
        full.len()
    )
    .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    assert!(raw.starts_with("HTTP/1.1 200"), "{raw}");
    let node_console = after(&akit.until("tunnel.node: console on"), "console on ");
    let proxy_console = after(&akit.until("tunnel.proxy: console on"), "console on ");

    let (status, _, body) = call(console, "GET", "/api/workbench", &admin, "");
    assert_eq!(status, 200, "{body}");
    assert!(
        body.contains(r#""modules":["admin","tunnel.hub","tunnel.node","tunnel.proxy"]"#),
        "{body}"
    );
    eventually("the node to link to the hub", || {
        let (_, _, body) = call(console, "GET", "/api/m/tunnel.node/status", &admin, "");
        body.contains(r#""state":"registered""#)
    });

    let nel = session(node_console, "nel");
    let (status, _, body) = call(node_console, "GET", "/api/status", &nel, "");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""name":"lab""#), "{body}");

    let pat = session(proxy_console, "pat");
    let (status, _, body) = call(
        proxy_console,
        "POST",
        "/api/meta",
        &pat,
        r#"{"site": "home"}"#,
    );
    assert_eq!(status, 200, "{body}");
    let (status, _, body) = call(console, "GET", "/api/m/admin/config", &admin, "");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("home"), "{body}");
    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let taken = held.local_addr().unwrap();
    let (status, _, body) = call(
        proxy_console,
        "POST",
        "/api/forwards",
        &pat,
        &format!(r#"{{"listen": "{taken}", "node": "lab", "service": "echo"}}"#),
    );
    assert_eq!(status, 400, "{body}");
    assert!(body.contains(&format!("forward {taken}")), "{body}");
    drop(held);
    drop(akit);
}
