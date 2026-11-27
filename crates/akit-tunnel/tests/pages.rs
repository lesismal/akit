//! The hub's pages (I109): its `console`, served by akit-admin's role consoles with the hub's
//! module, its edits written back into its member and applied as `akit run` applies them; fit's
//! `roleconsole_test.go` and `tunnel/hub/api_test.go` (`TestPages`, `TestPagesPeers`), ported.
//! And, when `AKIT_FIT_BIN` names fit's binary, akit's hub's pages through its console socket and
//! fit's web proxy.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use akit_admin::console::RoleConsoles;
use akit_config::value::{self, Value};
use akit_roles::host::{Edit, Editor, Manager};

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn hash() -> String {
    akio_crypto::bcrypt::hash(b"pw", akio_crypto::bcrypt::MIN_COST).unwrap()
}

/// A console member for `address`, the user `admin` with the password `pw`.
fn console_json(address: &str) -> String {
    format!(
        r#"{{"address": "{address}", "users": [{{"username": "admin", "password_hash": "{}"}}]}}"#,
        hash()
    )
}

/// An echo server.
fn echo() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            std::thread::spawn(move || {
                let mut buf = [0u8; 16 << 10];
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

/// The roles of a config, with a control thread that applies their edits as `akit run`'s does,
/// and their consoles; the hub's on `console`.
struct Env {
    engine: Arc<akio::Engine>,
    roles: Arc<Mutex<Manager>>,
    /// The running config, as the control thread has it.
    value: Arc<Mutex<Value>>,
    console: SocketAddr,
}

impl Drop for Env {
    fn drop(&mut self) {
        self.roles.lock().unwrap().stop(Duration::ZERO);
        self.engine.shutdown();
        self.engine.wait();
    }
}

fn start(doc: &str, console: u16) -> Env {
    let engine = akio::Engine::builder().client_threads(2).build().unwrap();
    engine.start().unwrap();
    let engine = Arc::new(engine);
    let (tx, rx) = std::sync::mpsc::channel::<Edit>();
    let tx = Mutex::new(tx);
    let editor: Editor = Arc::new(move |edit| {
        let _ = tx.lock().unwrap().send(edit);
        Ok(())
    });
    let mut roles = Manager::new(
        &[akit_tunnel::HUB, akit_tunnel::NODE, akit_tunnel::PROXY],
        Arc::clone(&engine),
        Some(editor),
    );
    roles.set_consoles(Box::new(RoleConsoles::new(&akit_admin::Options::default())));
    let config = akit_config::from_json(doc, None).unwrap_or_else(|e| panic!("{doc}\n{e}"));
    let errs = roles.apply(&config);
    assert!(errs.is_empty(), "{errs:?}");
    let roles = Arc::new(Mutex::new(roles));
    let value = Arc::new(Mutex::new(value::parse(doc).unwrap()));
    let (control, v) = (Arc::clone(&roles), Arc::clone(&value));
    std::thread::spawn(move || {
        while let Ok(edit) = rx.recv() {
            let mut next = v.lock().unwrap().clone();
            match edit.apply_to(&mut next) {
                Ok(done) => match akit_config::from_json(&next.to_json(), None) {
                    Ok(config) => {
                        let errs = control.lock().unwrap().apply(&config);
                        *v.lock().unwrap() = next;
                        done(Ok(errs));
                    }
                    Err(e) => done(Err(e.to_string())),
                },
                Err((done, err)) => done(Err(err)),
            }
        }
    });
    assert!(
        roles
            .lock()
            .unwrap()
            .modules()
            .ids()
            .contains(&"tunnel.hub")
    );
    let console = SocketAddr::from(([127, 0, 0, 1], console));
    Env {
        engine,
        roles,
        value,
        console,
    }
}

impl Env {
    /// The hub's member, as JSON.
    fn current(&self) -> String {
        let v = self.value.lock().unwrap();
        v.get("tunnel")
            .and_then(|t| t.get("hub"))
            .map_or_else(String::new, Value::to_json)
    }

    /// Sets the hub's member in the running config without applying it, as fit's `setMember`.
    fn set_member(&self, json: &str) {
        let m = value::parse(json).unwrap();
        let mut v = self.value.lock().unwrap();
        let Value::Object(top) = &mut *v else {
            panic!()
        };
        let (_, Value::Object(t)) = top.iter_mut().find(|(k, _)| k == "tunnel").unwrap() else {
            panic!()
        };
        let (_, h) = t.iter_mut().find(|(k, _)| k == "hub").unwrap();
        *h = m;
    }

    fn hub(&self) -> akit_tunnel::hub::HubApi {
        let g = self.roles.lock().unwrap();
        let api = g.api("tunnel.hub").unwrap();
        api.downcast_ref::<akit_tunnel::hub::HubApi>()
            .unwrap()
            .clone()
    }
}

#[derive(Debug)]
struct Resp {
    status: u16,
    head: String,
    body: String,
}

/// One exchange with the console at `addr`, on a connection of its own.
fn exchange(addr: SocketAddr, method: &str, path: &str, headers: &str, body: &str) -> Resp {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n{headers}Content-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let raw = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    Resp {
        status: head.split(' ').nth(1).unwrap_or("0").parse().unwrap_or(0),
        head: head.to_owned(),
        body: body.to_owned(),
    }
}

/// A logged-in browser of a console (fit's `login` of its tests).
struct Page {
    addr: SocketAddr,
    auth: String,
    cookie_name: String,
}

fn login(addr: SocketAddr) -> Page {
    let r = exchange(
        addr,
        "POST",
        "/api/login",
        "Content-Type: application/json\r\n",
        r#"{"username": "admin", "password": "pw"}"#,
    );
    assert_eq!(r.status, 200, "{r:?}");
    let set = r
        .head
        .lines()
        .find_map(|l| l.strip_prefix("Set-Cookie: "))
        .unwrap();
    let session = set.split(';').next().unwrap().to_owned();
    let cookie_name = session.split('=').next().unwrap().to_owned();
    let csrf = r
        .body
        .split("\"csrf\":\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    Page {
        addr,
        auth: format!("Cookie: {session}\r\nX-Akit-Csrf: {csrf}\r\n"),
        cookie_name,
    }
}

impl Page {
    fn get(&self, path: &str) -> Resp {
        exchange(self.addr, "GET", path, &self.auth, "")
    }

    fn post(&self, path: &str, body: &str) -> Resp {
        exchange(
            self.addr,
            "POST",
            path,
            &format!("{}Content-Type: application/json\r\n", self.auth),
            body,
        )
    }

    /// Posts `body` to `path` and expects `status`: the body answered.
    fn expect(&self, status: u16, path: &str, body: &str) -> String {
        let r = self.post(path, body);
        assert_eq!(r.status, status, "{path} {body}: {r:?}");
        r.body
    }
}

fn wait_for(what: &str, cond: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !cond() {
        assert!(Instant::now() < deadline, "waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn hub_doc(console: u16, extra: &str) -> String {
    format!(
        r#"{{"tunnel": {{"hub": {{"name": "hub", "password": "pw", "listeners": [{{"protocol": "tcp", "address": "127.0.0.1:0"}}], "console": {}{extra}}}}}}}"#,
        console_json(&format!("127.0.0.1:{console}"))
    )
}

fn hub_env(extra_doc: impl FnOnce(u16) -> String) -> Env {
    let port = free_port();
    start(&extra_doc(port), port)
}

/// fit's `TestModule` (roleconsole): the hub's console serves its module, under its own cookie,
/// at `/api/` and at `/api/m/tunnel.hub/`, and no other.
#[test]
fn the_console_serves_the_hubs_module() {
    let env = hub_env(|p| hub_doc(p, ""));
    let r = exchange(env.console, "GET", "/api/status", "", "");
    assert_eq!(r.status, 401, "{r:?}");
    let pg = login(env.console);
    assert_eq!(
        pg.cookie_name,
        format!("akit_tunnel_hub_{}", env.console.port())
    );
    let wb = pg.get("/api/workbench");
    assert_eq!(wb.status, 200);
    assert!(
        wb.body.contains(r#""modules":["tunnel.hub"]"#),
        "{}",
        wb.body
    );
    for path in ["/api/status", "/api/m/tunnel.hub/status"] {
        let st = pg.get(path);
        assert_eq!(st.status, 200, "{path}: {st:?}");
        assert!(st.body.contains(r#""name":"hub""#), "{}", st.body);
        assert!(st.body.contains(r#""fingerprint""#), "{}", st.body);
    }
    assert_eq!(pg.get("/api/m/admin/users").status, 404);
    assert_eq!(pg.get("/api/m/llm.hub/status").status, 404);
    assert_eq!(pg.get("/api/nope").status, 404);
    // The workbench's files.
    let index = exchange(env.console, "GET", "/", "", "");
    assert_eq!(index.status, 200);
    assert!(index.body.contains("core/workbench.js"), "{}", index.body);
    // The token routes: none, one issued (its member written back), its expiry, revoked.
    let t = pg.get("/api/tokens");
    assert_eq!(t.status, 200);
    assert_eq!(t.body.trim(), r#"{"tokens":[]}"#);
    let issued = pg.expect(200, "/api/tokens", r#"{"name": "lab", "ttl": "1h"}"#);
    assert!(issued.contains(r#""token":"akitt1."#), "{issued}");
    let id = issued
        .split("\"id\":\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    assert!(env.current().contains(&id), "{}", env.current());
    pg.expect(
        200,
        "/api/tokens/expiry",
        &format!(r#"{{"id": "{id}", "expires_at": 0}}"#),
    );
    pg.expect(404, "/api/tokens/revoke", r#"{"id": "0123456789abcdef"}"#);
    pg.expect(400, "/api/tokens", r#"{"name": ""}"#);
    pg.expect(200, "/api/tokens/revoke", &format!(r#"{{"id": "{id}"}}"#));
    assert!(!env.current().contains(&id), "{}", env.current());
    // Moved with the member: another port, the old one closed.
    let next = free_port();
    let cur = env.current();
    let moved = cur.replace(
        &format!("127.0.0.1:{}", env.console.port()),
        &format!("127.0.0.1:{next}"),
    );
    let doc = format!(r#"{{"tunnel": {{"hub": {moved}}}}}"#);
    let errs = env
        .roles
        .lock()
        .unwrap()
        .apply(&akit_config::from_json(&doc, None).unwrap());
    assert!(errs.is_empty(), "{errs:?}");
    let pg2 = login(SocketAddr::from(([127, 0, 0, 1], next)));
    assert_eq!(pg2.get("/api/status").status, 200);
    assert!(TcpStream::connect(env.console).is_err());
}

/// fit's `TestPages`: what the hub's API refuses, and what it changes.
#[test]
fn pages_change_the_member() {
    let env = hub_env(|p| hub_doc(p, ""));
    let pg = login(env.console);

    // Bodies that do not decode.
    for path in [
        "/api/listeners",
        "/api/listeners/update",
        "/api/listeners/delete",
        "/api/settings",
        "/api/peers/refresh",
        "/api/peers/services",
        "/api/peers/kick",
        "/api/tunnels/close",
    ] {
        pg.expect(400, path, "not json");
    }
    // Not sent as JSON.
    let r = exchange(env.console, "POST", "/api/settings", &pg.auth, "{}");
    assert_eq!(r.status, 415, "{r:?}");

    // Listeners: a ws path without its slash gets one.
    let ws = r#"{"protocol": "ws", "address": "127.0.0.1:0", "path": "x"}"#;
    pg.expect(200, "/api/listeners", ws);
    assert!(env.current().contains(r#""/x""#), "{}", env.current());
    pg.expect(400, "/api/listeners", ws);
    let tcp = r#"{"protocol": "tcp", "address": "127.0.0.1:0"}"#;
    let none = r#"{"protocol": "tcp", "address": "127.0.0.1:1"}"#;
    pg.expect(
        400,
        "/api/listeners/update",
        &format!(r#"{{"old": {none}, "new": {tcp}}}"#),
    );
    pg.expect(
        400,
        "/api/listeners/update",
        &format!(r#"{{"old": {ws}, "new": {tcp}}}"#),
    );
    pg.expect(
        200,
        "/api/listeners/update",
        &format!(
            r#"{{"old": {ws}, "new": {{"protocol": "ws", "address": "127.0.0.1:0", "path": "/y"}}}}"#
        ),
    );
    let cur = env.current();
    assert!(cur.contains(r#""/y""#) && !cur.contains(r#""/x""#), "{cur}");
    let st = pg.get("/api/status").body;
    assert!(st.contains("/y"), "{st}");
    pg.expect(400, "/api/listeners/delete", none);
    pg.expect(
        200,
        "/api/listeners/delete",
        r#"{"protocol": "ws", "address": "127.0.0.1:0", "path": "/y"}"#,
    );
    assert!(!env.current().contains(r#""/y""#), "{}", env.current());
    // A member whose listeners do not decode is not changed.
    let cur = env.current();
    env.set_member(r#"{"listeners": 5}"#);
    pg.expect(400, "/api/listeners", none);
    env.set_member(&cur);

    // Settings: passwords set, then cleared; the console socket moved.
    let sock = "127.0.0.1:0";
    pg.expect(
        200,
        "/api/settings",
        &format!(
            r#"{{"password": "pw2", "body_password": "b", "socket_address": "{sock}", "socket_password": "sp"}}"#
        ),
    );
    let m = value::parse(&env.current()).unwrap();
    let text = |v: Option<&Value>| match v {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    assert_eq!(text(m.get("password")), "pw2");
    assert_eq!(text(m.get("body_password")), "b");
    let cs = m.get("console_socket").unwrap();
    assert_eq!(text(cs.get("address")), sock);
    assert_eq!(text(cs.get("password")), "sp");
    assert!(env.hub().socket_addr().is_some());
    pg.expect(
        200,
        "/api/settings",
        &format!(
            r#"{{"clear_body_password": true, "socket_address": "{sock}", "clear_socket_password": true}}"#
        ),
    );
    let m = value::parse(&env.current()).unwrap();
    assert!(m.get("body_password").is_none());
    let cs = m.get("console_socket").unwrap();
    assert_eq!(text(cs.get("address")), sock);
    assert!(cs.get("password").is_none());
    let st = pg.get("/api/status").body;
    let st = value::parse(&st).unwrap();
    let set = st.get("settings").unwrap();
    assert_eq!(set.get("has_password"), Some(&Value::Bool(true)));
    assert_eq!(set.get("has_body_password"), Some(&Value::Bool(false)));
    assert_eq!(set.get("has_socket_password"), Some(&Value::Bool(false)));
    assert_eq!(
        set.get("socket_address"),
        Some(&Value::String(sock.to_owned()))
    );
    let cur = env.current();
    env.set_member(r#"{"console_socket": 5}"#);
    pg.expect(
        400,
        "/api/settings",
        &format!(r#"{{"socket_address": "{sock}"}}"#),
    );
    env.set_member(&cur);
    pg.expect(200, "/api/settings", r#"{"socket_address": ""}"#);
    assert!(!env.current().contains("console_socket"));
    assert!(env.hub().socket_addr().is_none());
    // A change that does not load is refused, the member as it was.
    let cur = env.current();
    pg.expect(
        400,
        "/api/listeners",
        r#"{"protocol": "tcp", "address": "nope"}"#,
    );
    assert_eq!(env.current(), cur);
}

/// fit's `TestPagesPeers`: the hub's API on peers and tunnels, with akit's own node and proxy.
#[test]
fn pages_on_peers_and_tunnels() {
    let (link, svc, fwd) = (free_port(), echo(), free_port());
    let up = format!(
        r#"{{"name": "hub", "endpoints": [{{"address": "127.0.0.1:{link}"}}], "password": "pw", "pool_size": 1, "retry_interval": "100ms"}}"#
    );
    let env = hub_env(|p| {
        format!(
            r#"{{"tunnel": {{
          "hub": {{"name": "hub", "password": "pw", "listeners": [{{"protocol": "tcp", "address": "127.0.0.1:{link}"}}], "console": {}}},
          "node": {{"name": "lab", "allow_hub_services": true, "hubs": [{up}], "services": [{{"name": "echo", "address": "127.0.0.1:{svc}"}}]}},
          "proxy": {{"name": "me", "hubs": [{up}], "forwards": [{{"listen": "127.0.0.1:{fwd}", "node": "lab", "service": "b"}}]}}}}}}"#,
            console_json(&format!("127.0.0.1:{p}"))
        )
    });
    let pg = login(env.console);
    let api = env.hub();
    wait_for("the node and the proxy to register", || {
        api.peer_id("node", "lab").is_some() && api.peer_id("proxy", "me").is_some()
    });
    let node = api.peer_id("node", "lab").unwrap();
    let proxy = api.peer_id("proxy", "me").unwrap();

    pg.expect(404, "/api/peers/refresh", r#"{"id": 999}"#);
    pg.expect(404, "/api/peers/refresh", &format!(r#"{{"id": {proxy}}}"#));
    let s = pg.expect(200, "/api/peers/refresh", &format!(r#"{{"id": {node}}}"#));
    assert!(s.contains("echo"), "refresh: {s}");
    pg.expect(
        404,
        "/api/peers/services",
        &format!(r#"{{"id": {proxy}, "services": []}}"#),
    );
    let s = pg.expect(
        200,
        "/api/peers/services",
        &format!(
            r#"{{"id": {node}, "services": [{{"name": " b ", "address": "127.0.0.1:{svc}"}}]}}"#
        ),
    );
    assert!(s.contains(r#""name":"b""#), "services: {s}");

    // A tunnel through the forward, to the service set; closed by its ID.
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut t = loop {
        if let Ok(mut s) = TcpStream::connect(("127.0.0.1", fwd)) {
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let _ = s.write_all(b"ping");
            let mut b = [0u8; 4];
            if s.read_exact(&mut b).is_ok() && &b == b"ping" {
                break s;
            }
        }
        assert!(Instant::now() < deadline, "no tunnel opened");
        std::thread::sleep(Duration::from_millis(100));
    };
    let st = value::parse(&pg.get("/api/status").body).unwrap();
    let Some(Value::Array(tunnels)) = st.get("tunnels") else {
        panic!("{}", st.to_json())
    };
    let Some(Value::String(id)) = tunnels[0].get("id") else {
        panic!("{}", st.to_json())
    };
    pg.expect(400, "/api/tunnels/close", r#"{"id": "x"}"#);
    pg.expect(404, "/api/tunnels/close", r#"{"id": "1"}"#);
    pg.expect(200, "/api/tunnels/close", &format!(r#"{{"id": "{id}"}}"#));
    t.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut b = [0u8; 1];
    assert!(
        matches!(t.read(&mut b), Ok(0) | Err(_)),
        "the tunnel stayed"
    );

    // A peer kicked hears why, and goes (the node links again after its retry).
    pg.expect(404, "/api/peers/kick", r#"{"id": 999}"#);
    pg.expect(200, "/api/peers/kick", &format!(r#"{{"id": {node}}}"#));
    wait_for("the kicked node to go", || {
        api.peer_id("node", "lab") != Some(node)
    });
}

/// fit's `TestPagesPeers`' failing node: one that does not take the hub's services answers with
/// an error, 502.
#[test]
fn a_node_that_refuses_is_a_bad_gateway() {
    let link = free_port();
    let up = format!(
        r#"{{"name": "hub", "endpoints": [{{"address": "127.0.0.1:{link}"}}], "password": "pw", "pool_size": 1, "retry_interval": "100ms"}}"#
    );
    let env = hub_env(|p| {
        format!(
            r#"{{"tunnel": {{
          "hub": {{"name": "hub", "password": "pw", "listeners": [{{"protocol": "tcp", "address": "127.0.0.1:{link}"}}], "console": {}}},
          "node": {{"name": "lab", "hubs": [{up}], "services": [{{"name": "echo", "address": "127.0.0.1:1"}}]}}}}}}"#,
            console_json(&format!("127.0.0.1:{p}"))
        )
    });
    let pg = login(env.console);
    let api = env.hub();
    wait_for("the node to register", || {
        api.peer_id("node", "lab").is_some()
    });
    let node = api.peer_id("node", "lab").unwrap();
    pg.expect(
        502,
        "/api/peers/services",
        &format!(r#"{{"id": {node}, "services": []}}"#),
    );
}

/// Against fit's binary (`AKIT_FIT_BIN`; skipped without it): akit's hub's pages, served by its
/// console, through its console socket and fit's web proxy: the workbench, a login and the
/// hub's status, as a browser of fit's proxy gets them.
#[test]
fn akits_hub_pages_through_fits_web_proxy() {
    let Ok(bin) = std::env::var("AKIT_FIT_BIN") else {
        eprintln!("AKIT_FIT_BIN not set: skipped");
        return;
    };
    let sock = free_port();
    let env = hub_env(|p| {
        hub_doc(
            p,
            &format!(
                r#", "console_socket": {{"address": "127.0.0.1:{sock}", "password": "sock"}}"#
            ),
        )
    });
    let web = free_port();
    let dir = std::env::temp_dir().join(format!("akit-i109-proxy-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fit.json");
    std::fs::write(
        &path,
        format!(
            r#"{{"tunnel": {{"proxy": {{"name": "fit", "web_proxies": [{{"name": "hub", "listen": "127.0.0.1:{web}", "url": "ws://127.0.0.1:{sock}/", "password": "sock"}}]}}}}}}"#
        ),
    )
    .unwrap();
    let log = std::fs::File::create(dir.join("fit.log")).unwrap();
    let mut child = std::process::Command::new(bin)
        .args(["run", "-c"])
        .arg(&path)
        .current_dir(&dir)
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let web = SocketAddr::from(([127, 0, 0, 1], web));
    let deadline = Instant::now() + Duration::from_secs(15);
    let index = loop {
        if TcpStream::connect(web).is_ok() {
            let r = exchange(web, "GET", "/", "", "");
            if r.status == 200 {
                break r;
            }
        }
        assert!(Instant::now() < deadline, "no page through fit's web proxy");
        std::thread::sleep(Duration::from_millis(200));
    };
    assert!(index.body.contains("core/workbench.js"), "{}", index.body);
    // The Host a browser of the proxy sends is the proxy's: the console checks the Origin only
    // when there is one.
    let pg = login(web);
    assert_eq!(
        pg.cookie_name,
        format!("akit_tunnel_hub_{}", env.console.port())
    );
    let st = pg.get("/api/status");
    assert_eq!(st.status, 200, "{st:?}");
    assert!(st.body.contains(r#""name":"hub""#), "{}", st.body);
    assert!(
        st.body.contains(&format!(r#""socket":"127.0.0.1:{sock}""#)),
        "{}",
        st.body
    );
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
