//! The node's and the proxy's pages (I109b): their `console`, served by akit-admin's role
//! consoles with their modules, their edits written back into their members and applied as
//! `akit run` applies them; fit's `tunnel/node/node_test.go` (`TestPages`) and
//! `tunnel/proxy/proxy_test.go` (`TestProxyAPI`, `TestPick`), ported. And, when `AKIT_FIT_BIN`
//! names fit's binary, akit's node and proxy linked to fit's hub, their pages showing what fit's
//! hub tells them.
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

/// A port free now, for what must name its port before it binds: a forward added through the
/// pages, which name a forward by its address as fit's do. Another process may take it
/// meanwhile. akit's other listeners bind port 0 and are asked where they listen, and fit's too
/// ([`fit::Fit::port`]).
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A console member for `address`, the user `admin` with the password `pw`.
fn console_json(address: &str) -> String {
    let hash = akio_crypto::bcrypt::hash(b"pw", akio_crypto::bcrypt::MIN_COST).unwrap();
    format!(
        r#"{{"address": "{address}", "users": [{{"username": "admin", "password_hash": "{hash}"}}]}}"#
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

/// The roles of a config, with a control thread that applies their edits as `akit run`'s does
/// (a role's failure a warning), and their consoles.
struct Env {
    engine: Arc<akio::Engine>,
    roles: Arc<Mutex<Manager>>,
    value: Arc<Mutex<Value>>,
}

impl Drop for Env {
    fn drop(&mut self) {
        self.roles.lock().unwrap().stop(Duration::ZERO);
        self.engine.shutdown();
        self.engine.wait();
    }
}

/// Applies `doc` on `roles`, which must take it.
fn apply(roles: &mut Manager, doc: &str) {
    let config = akit_config::from_json(doc, None).unwrap_or_else(|e| panic!("{doc}\n{e}"));
    let errs = roles.apply(&config);
    assert!(errs.is_empty(), "{errs:?}");
}

/// The port of `addr`, a `host:port`.
fn port_of(addr: &str) -> u16 {
    addr.parse::<SocketAddr>()
        .unwrap_or_else(|e| panic!("{addr:?}: {e}"))
        .port()
}

/// The port the hub's first listener bound.
fn hub_port(roles: &Manager) -> u16 {
    let api = roles.api("tunnel.hub").unwrap();
    let st = api
        .downcast_ref::<akit_tunnel::hub::HubApi>()
        .unwrap()
        .status();
    let st = value::parse(&st).unwrap();
    match arr(&st, "listen").first() {
        Some(Value::String(a)) => port_of(a),
        other => panic!("the hub's listeners: {other:?}"),
    }
}

/// The hub's console socket's address.
fn socket_addr(roles: &Manager) -> SocketAddr {
    let api = roles.api("tunnel.hub").unwrap();
    api.downcast_ref::<akit_tunnel::hub::HubApi>()
        .unwrap()
        .socket_addr()
        .expect("the console socket's address")
}

/// The port the proxy's first forward bound.
fn forward_port(roles: &Manager) -> u16 {
    let api = roles.api("tunnel.proxy").unwrap();
    let st = api
        .downcast_ref::<akit_tunnel::proxy::ProxyApi>()
        .unwrap()
        .status();
    let st = value::parse(&st).unwrap();
    port_of(&text(&arr(&st, "forwards")[0], "addr"))
}

/// The port role `id`'s console bound.
fn console_port(roles: &Manager, id: &str) -> u16 {
    roles
        .console_addr(id)
        .expect("the console's address")
        .port()
}

/// The roles of `first` (members of `tunnel`, their listeners on port 0), then of `first` and the
/// members `more` writes once they run, given the roles to ask where they listen. Edits are
/// applied to the doc of them all.
fn start(first: &str, more: impl FnOnce(&Manager) -> String) -> Env {
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
    let mut doc = format!(r#"{{"tunnel": {{{first}}}}}"#);
    apply(&mut roles, &doc);
    let m = more(&roles);
    if !m.is_empty() {
        doc = format!(r#"{{"tunnel": {{{first}, {m}}}}}"#);
        apply(&mut roles, &doc);
    }
    let roles = Arc::new(Mutex::new(roles));
    let value = Arc::new(Mutex::new(value::parse(&doc).unwrap()));
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
    Env {
        engine,
        roles,
        value,
    }
}

impl Env {
    /// Member `tunnel.<role>`, as JSON.
    fn member(&self, role: &str) -> String {
        let v = self.value.lock().unwrap();
        v.get("tunnel")
            .and_then(|t| t.get(role))
            .map_or_else(String::new, Value::to_json)
    }

    fn node(&self) -> akit_tunnel::node::NodeApi {
        let g = self.roles.lock().unwrap();
        let api = g.api("tunnel.node").unwrap();
        api.downcast_ref::<akit_tunnel::node::NodeApi>()
            .unwrap()
            .clone()
    }

    /// What `f` makes of the roles: where they listen.
    fn ask<T>(&self, f: impl FnOnce(&Manager) -> T) -> T {
        f(&self.roles.lock().unwrap())
    }

    fn proxy(&self) -> akit_tunnel::proxy::ProxyApi {
        let g = self.roles.lock().unwrap();
        let api = g.api("tunnel.proxy").unwrap();
        api.downcast_ref::<akit_tunnel::proxy::ProxyApi>()
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

/// A logged-in browser of a console.
struct Page {
    addr: SocketAddr,
    auth: String,
}

fn login(port: u16) -> Page {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    wait_for("the console", || TcpStream::connect(addr).is_ok());
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
    }
}

impl Page {
    fn get(&self, path: &str) -> Resp {
        exchange(self.addr, "GET", path, &self.auth, "")
    }

    fn status(&self) -> Value {
        let r = self.get("/api/status");
        assert_eq!(r.status, 200, "{r:?}");
        value::parse(&r.body).unwrap()
    }

    /// Posts `body` to `path` and expects `status`, and `want` in the answer: the body answered.
    fn expect(&self, status: u16, path: &str, body: &str, want: &str) -> String {
        let r = exchange(
            self.addr,
            "POST",
            path,
            &format!("{}Content-Type: application/json\r\n", self.auth),
            body,
        );
        assert_eq!(r.status, status, "{path} {body}: {r:?}");
        assert!(r.body.contains(want), "{path} {body}: {r:?}");
        r.body
    }

    /// Adds a forward on a free address (fit's `forward`): the address.
    fn forward(&self, node: &str, service: &str, hub: &str) -> u16 {
        let port = free_port();
        self.expect(
            200,
            "/api/forwards",
            &format!(
                r#"{{"listen": "127.0.0.1:{port}", "node": "{node}", "service": "{service}", "hub": "{hub}"}}"#
            ),
            "ok",
        );
        port
    }
}

fn wait_for(what: &str, cond: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !cond() {
        assert!(Instant::now() < deadline, "waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A connection through the forward on `port` that echoed `what`, kept open (fit's `echoOnce`).
fn echo_once(port: u16, what: &str) -> TcpStream {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(what.as_bytes()).unwrap();
    let mut b = vec![0u8; what.len()];
    s.read_exact(&mut b).unwrap();
    assert_eq!(b, what.as_bytes());
    s
}

/// A connection through the forward on `port` is closed without a byte (fit's `refused`).
fn refused(port: u16) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let _ = s.write_all(b"x");
    let mut b = [0u8; 1];
    assert!(
        matches!(s.read(&mut b), Ok(0) | Err(_)),
        "a connection to {port} was carried"
    );
}

fn arr(v: &Value, k: &str) -> Vec<Value> {
    match v.get(k) {
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    }
}

fn text(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_json().trim().to_owned(),
        None => String::new(),
    }
}

fn uplink(name: &str, port: u16) -> String {
    format!(
        r#"{{"name": "{name}", "endpoints": [{{"address": "127.0.0.1:{port}"}}], "password": "pw", "pool_size": 1, "retry_interval": "100ms", "ping_interval": "200ms"}}"#
    )
}

/// fit's node `TestPages`: what the node's pages refuse and change, its status (its hubs'
/// links, its services' connects, its tunnels), a tunnel closed.
#[test]
#[allow(clippy::too_many_lines)]
fn the_nodes_pages() {
    let svc = echo();
    let env = start(
        r#""hub": {"name": "hub", "password": "pw", "listeners": [{"protocol": "tcp", "address": "127.0.0.1:0"}]}"#,
        |roles| {
            let link = hub_port(roles);
            format!(
                r#""node": {{"name": "lab", "hubs": [{}], "services": [{{"name": "echo", "address": "127.0.0.1:{svc}"}}], "console": {}}},
          "proxy": {{"name": "me", "hubs": [{}], "forwards": [{{"listen": "127.0.0.1:0", "node": "lab", "service": "echo"}}]}}"#,
                uplink("hub", link),
                console_json("127.0.0.1:0"),
                uplink("hub", link)
            )
        },
    );
    let (fwd, con) = env.ask(|r| (forward_port(r), console_port(r, "tunnel.node")));
    let pg = login(con);
    let wb = pg.get("/api/workbench");
    assert!(wb.body.contains(r#""modules":["tunnel.node"]"#), "{wb:?}");
    for path in [
        "/api/hubs",
        "/api/hubs/update",
        "/api/hubs/delete",
        "/api/services",
        "/api/tunnels/close",
    ] {
        pg.expect(400, path, "not json", "error");
    }
    assert_eq!(pg.get("/api/nope").status, 404);

    // A tunnel through the proxy, as the node's pages show it.
    let p = env.proxy();
    wait_for("the proxy to know the node", || {
        p.has_node("hub", "lab", "echo")
    });
    let t = echo_once(fwd, "one");
    let st = pg.status();
    assert_eq!(text(&st, "name"), "lab");
    assert_eq!(text(&st, "open"), "1");
    let hubs = arr(&st, "hubs");
    assert_eq!(hubs.len(), 1, "{}", st.to_json());
    assert_eq!(text(&hubs[0], "state"), "registered");
    assert_eq!(text(&hubs[0], "tunnels"), "1");
    assert_eq!(text(&hubs[0], "pool"), "1");
    assert_eq!(text(&hubs[0], "has_password"), "true");
    assert_eq!(arr(&hubs[0], "links").len(), 1);
    assert_eq!(text(&arr(&hubs[0], "links")[0], "proto"), "tcp");
    let tunnels = arr(&st, "tunnels");
    assert_eq!(tunnels.len(), 1, "{}", st.to_json());
    assert_eq!(text(&tunnels[0], "hub"), "hub");
    assert_eq!(text(&tunnels[0], "service"), "echo");
    assert_eq!(text(&tunnels[0], "peer"), "me");
    assert_eq!(text(&tunnels[0], "addr"), format!("127.0.0.1:{svc}"));
    assert!(tunnels[0].get("connect_rtt").is_some(), "{}", st.to_json());
    let rtt = st.get("service_rtt").and_then(|s| s.get("echo")).unwrap();
    assert_eq!(text(rtt, "samples"), "1");
    // The connects' round trips reach the proxy through the hub's pings.
    wait_for("the service's round trip at the proxy", || {
        let ps = value::parse(&p.status()).unwrap();
        let hubs = arr(&ps, "hubs");
        hubs.first()
            .map(|h| arr(h, "nodes"))
            .and_then(|n| n.first().cloned())
            .is_some_and(|n| n.get("service_rtt").is_some())
    });

    // A tunnel closed by its hub and ID.
    let id = text(&tunnels[0], "id");
    pg.expect(
        400,
        "/api/tunnels/close",
        r#"{"hub": "hub", "id": "x"}"#,
        "not a tunnel's",
    );
    pg.expect(
        404,
        "/api/tunnels/close",
        &format!(r#"{{"hub": "nope", "id": "{id}"}}"#),
        "no such hub",
    );
    pg.expect(
        404,
        "/api/tunnels/close",
        r#"{"hub": "hub", "id": "8"}"#,
        "no such tunnel",
    );
    pg.expect(
        200,
        "/api/tunnels/close",
        &format!(r#"{{"hub": "hub", "id": "{id}"}}"#),
        "ok",
    );
    let n = env.node();
    wait_for("the tunnel to close", || n.tunnels() == 0);
    drop(t);

    // The services set; a hub added, changed and removed.
    pg.expect(
        200,
        "/api/services",
        &format!(
            r#"[{{"name": " echo ", "address": "127.0.0.1:{svc}"}}, {{"name": "two", "address": "127.0.0.1:{svc}", "description": "d"}}]"#
        ),
        "ok",
    );
    let m = value::parse(&env.member("node")).unwrap();
    let services = arr(&m, "services");
    assert_eq!(services.len(), 2, "{}", m.to_json());
    assert_eq!(text(&services[0], "name"), "echo");
    assert_eq!(text(&services[1], "description"), "d");
    let far = 1;
    let far_req = format!(
        r#"{{"name": "far", "endpoints": [{{"protocol": "tcp", "address": "127.0.0.1:{far}", "enabled": true}}], "password": "pw"}}"#
    );
    pg.expect(200, "/api/hubs", &far_req, "ok");
    pg.expect(400, "/api/hubs", &far_req, "a hub named far exists");
    assert_eq!(n.hubs(), ["hub", "far"]);
    pg.expect(
        200,
        "/api/hubs/update",
        &format!(
            r#"{{"name": "far", "hub": {{"name": "far2", "endpoints": [{{"protocol": "ws", "address": "127.0.0.1:{far}", "enabled": true}}], "pool_size": 2}}}}"#
        ),
        "ok",
    );
    let m = value::parse(&env.member("node")).unwrap();
    let far2 = arr(&m, "hubs")
        .into_iter()
        .find(|h| text(h, "name") == "far2")
        .unwrap();
    assert_eq!(text(&far2, "pool_size"), "2", "{}", m.to_json());
    assert_eq!(
        text(&arr(&far2, "endpoints")[0], "address"),
        format!("ws://127.0.0.1:{far}/tunnel")
    );
    assert_eq!(text(&far2, "password"), "pw");
    let st = pg.status();
    let far2 = arr(&st, "hubs")
        .into_iter()
        .find(|h| text(h, "name") == "far2")
        .unwrap();
    assert_eq!(text(&far2, "pool_size"), "2");
    assert_ne!(text(&far2, "state"), "registered");
    assert_eq!(
        text(&arr(&far2, "endpoints")[0], "protocol"),
        "ws",
        "{}",
        far2.to_json()
    );
    pg.expect(
        400,
        "/api/hubs/update",
        r#"{"name": "nope", "hub": {"endpoints": [{"address": "h:1"}]}}"#,
        "no hub named nope",
    );
    pg.expect(
        400,
        "/api/hubs",
        r#"{"endpoints": [{"protocol": "tls", "address": "hub:7000#x", "enabled": true}], "password": "pw"}"#,
        "pin or ca_file",
    );
    pg.expect(400, "/api/hubs/delete", r#"{"name": "nope"}"#, "no hub");
    pg.expect(200, "/api/hubs/delete", r#"{"name": "far2"}"#, "ok");
    pg.expect(200, "/api/hubs/delete", r#"{"name": "hub"}"#, "ok");
    assert!(n.hubs().is_empty());
    assert!(
        !env.member("node").contains("hubs"),
        "{}",
        env.member("node")
    );
}

struct ProxyEnv {
    env: Env,
    fwd: u16,
    con: u16,
    svc: u16,
}

/// fit's `startHub`: a hub with a console socket, a node reaching echo, and a proxy with a
/// console, a forward to echo and a web proxy to the socket, `extra` added to the proxy's member.
fn start_hub(extra: &str) -> ProxyEnv {
    let svc = echo();
    let env = start(
        &format!(
            r#""hub": {{"name": "hub", "password": "pw", "allow_proxy_services": true, "listeners": [{{"protocol": "tcp", "address": "127.0.0.1:0"}}], "console_socket": {{"address": "127.0.0.1:0", "password": "sock"}}, "console": {}}}"#,
            console_json("127.0.0.1:0")
        ),
        |roles| {
            let (link, sock) = (hub_port(roles), socket_addr(roles));
            format!(
                r#""node": {{"name": "lab", "allow_hub_services": true, "hubs": [{}], "services": [{{"name": "echo", "address": "127.0.0.1:{svc}"}}]}},
          "proxy": {{"name": "me", "hubs": [{}], "forwards": [{{"listen": "127.0.0.1:0", "node": "lab", "service": "echo"}}],
            "web_proxies": [{{"name": "hub", "listen": "127.0.0.1:0", "url": "ws://{sock}/", "password": "sock"}}], "console": {}{extra}}}"#,
                uplink("hub", link),
                uplink("hub", link),
                console_json("127.0.0.1:0")
            )
        },
    );
    let (fwd, con) = env.ask(|r| (forward_port(r), console_port(r, "tunnel.proxy")));
    let p = env.proxy();
    wait_for("the proxy to know the node", || {
        p.has_node("hub", "lab", "echo")
    });
    ProxyEnv { env, fwd, con, svc }
}

/// fit's `TestProxyAPI`: the pages' API, and the proxy's state as the changes it makes apply.
#[test]
#[allow(clippy::too_many_lines)]
fn the_proxys_pages() {
    let ProxyEnv { env, fwd, con, svc } = start_hub("");
    let p = env.proxy();
    let pg = login(con);
    let wb = pg.get("/api/workbench");
    assert!(wb.body.contains(r#""modules":["tunnel.proxy"]"#), "{wb:?}");

    // Two tunnels, listed oldest first, one closed from the pages. The first is open, and has
    // echoed, before the second is: told apart by the bytes each carried.
    let one = echo_once(fwd, "one");
    let two = echo_once(fwd, "second");
    let st = pg.status();
    let tunnels = arr(&st, "tunnels");
    assert_eq!(tunnels.len(), 2, "{}", st.to_json());
    assert_eq!(
        (text(&tunnels[0], "out"), text(&tunnels[1], "out")),
        ("3".to_owned(), "6".to_owned()),
        "{}",
        st.to_json()
    );
    assert_eq!(text(&tunnels[0], "hub"), "hub");
    // The forward's `listen`, as its member says (port 0: the one it bound is `fwd`).
    assert_eq!(text(&tunnels[0], "peer"), "127.0.0.1:0");
    assert_eq!(text(&tunnels[0], "addr"), format!("127.0.0.1:{svc}"));
    let id = text(&tunnels[0], "id");
    pg.expect(
        400,
        "/api/tunnels/close",
        r#"{"hub": "hub", "id": "x"}"#,
        "not a tunnel's",
    );
    pg.expect(
        404,
        "/api/tunnels/close",
        &format!(r#"{{"hub": "nope", "id": "{id}"}}"#),
        "no such hub",
    );
    pg.expect(
        404,
        "/api/tunnels/close",
        r#"{"hub": "hub", "id": "999999999"}"#,
        "no such tunnel",
    );
    pg.expect(
        200,
        "/api/tunnels/close",
        &format!(r#"{{"hub": "hub", "id": "{id}"}}"#),
        "ok",
    );
    wait_for("the tunnel to close", || p.tunnels() == 1);
    drop((one, two));

    // The nodes, asked for again.
    pg.expect(
        404,
        "/api/nodes/refresh",
        r#"{"hub": "nope"}"#,
        "no such hub",
    );
    pg.expect(200, "/api/nodes/refresh", r#"{"hub": "hub"}"#, r#""lab""#);

    // A node's services, set through the hub.
    let extra = echo();
    pg.expect(
        404,
        "/api/nodes/services",
        r#"{"hub": "nope", "node": "lab"}"#,
        "no such hub",
    );
    pg.expect(
        502,
        "/api/nodes/services",
        r#"{"hub": "hub", "node": "ghost", "services": []}"#,
        "ghost",
    );
    pg.expect(
        200,
        "/api/nodes/services",
        &format!(
            r#"{{"hub": "hub", "node": "lab", "services": [{{"name": "echo", "address": "127.0.0.1:{svc}"}}, {{"name": " extra ", "address": " 127.0.0.1:{extra} ", "description": "more"}}]}}"#
        ),
        "ok",
    );
    wait_for("the node's new service", || {
        p.has_node("hub", "lab", "extra")
    });

    // The meta, told to the hubs the proxy keeps.
    pg.expect(200, "/api/meta", r#"{"site": "home"}"#, "ok");
    assert_eq!(
        text(pg.status().get("meta").unwrap(), "site"),
        "home",
        "{}",
        env.member("proxy")
    );
    pg.expect(400, "/api/meta", r#"{"site": 1}"#, "error");

    // Forwards: one listened on, one refused, one taken.
    pg.expect(
        400,
        "/api/forwards",
        r#"{"listen": "127.0.0.1:0", "node": "lab", "service": "echo"}"#,
        "forwarded already",
    );
    let by_hub = pg.forward("lab", "extra", " hub ");
    let to_missing = pg.forward("ghost", "echo", "");
    let bad_service = pg.forward("lab", "nope", "hub");
    drop(echo_once(by_hub, "by name"));
    refused(to_missing);
    refused(bad_service);
    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let taken = held.local_addr().unwrap().to_string();
    pg.expect(
        400,
        "/api/forwards",
        &format!(r#"{{"listen": "{taken}", "node": "lab", "service": "echo"}}"#),
        &format!("forward {taken}"),
    );
    let fv = arr(&pg.status(), "forwards")
        .into_iter()
        .find(|f| text(f, "listen") == taken)
        .unwrap();
    assert!(!text(&fv, "error").is_empty(), "{}", fv.to_json());
    drop(held);
    pg.expect(
        400,
        "/api/forwards/delete",
        r#"{"listen": "127.0.0.1:1"}"#,
        "not forwarded",
    );
    for a in [
        taken,
        format!("127.0.0.1:{to_missing}"),
        format!("127.0.0.1:{bad_service}"),
    ] {
        pg.expect(
            200,
            "/api/forwards/delete",
            &format!(r#"{{"listen": "{a}"}}"#),
            "ok",
        );
    }

    // A hub that does not answer, a forward through it, and its changes.
    let far = 1;
    let ep = format!(r#"[{{"protocol": "tcp", "address": "127.0.0.1:{far}", "enabled": true}}]"#);
    let far_req = format!(r#"{{"name": "far", "endpoints": {ep}, "password": "pw"}}"#);
    pg.expect(200, "/api/hubs", &far_req, "ok");
    pg.expect(400, "/api/hubs", &far_req, "exists");
    assert_eq!(p.hubs(), ["hub", "far"]);
    pg.expect(502, "/api/nodes/refresh", r#"{"hub": "far"}"#, "error");
    let via_far = pg.forward("lab", "echo", "far");
    refused(via_far);
    // A forward to any hub skips the one not registered.
    drop(echo_once(fwd, "still"));
    pg.expect(
        200,
        "/api/hubs/update",
        &format!(
            r#"{{"name": "far", "hub": {{"name": "far", "endpoints": {ep}, "pool_size": 1}}}}"#
        ),
        "ok",
    );
    pg.expect(
        400,
        "/api/hubs/update",
        &format!(r#"{{"name": "nope", "hub": {{"endpoints": {ep}}}}}"#),
        "no hub",
    );
    pg.expect(
        200,
        "/api/forwards/delete",
        &format!(r#"{{"listen": "127.0.0.1:{via_far}"}}"#),
        "ok",
    );
    pg.expect(200, "/api/hubs/delete", r#"{"name": "far"}"#, "ok");
    pg.expect(400, "/api/hubs/delete", r#"{"name": "far"}"#, "no hub");
    assert_eq!(p.hubs(), ["hub"]);

    // Bodies the pages send that do not decode.
    for path in [
        "/api/hubs",
        "/api/hubs/update",
        "/api/hubs/delete",
        "/api/nodes/refresh",
        "/api/nodes/services",
        "/api/meta",
        "/api/forwards",
        "/api/forwards/delete",
        "/api/tunnels/close",
    ] {
        pg.expect(400, path, r#""not an object""#, "error");
    }

    // The status, with the nodes' shares.
    let st = pg.status();
    assert_eq!(text(&st, "name"), "me");
    assert_eq!(arr(&st, "web_proxies").len(), 1, "{}", st.to_json());
    let hubs = arr(&st, "hubs");
    assert_eq!(hubs.len(), 1, "{}", st.to_json());
    let nodes = arr(&hubs[0], "nodes");
    assert_eq!(nodes.len(), 1, "{}", st.to_json());
    assert_eq!(text(&nodes[0], "share"), "1", "{}", st.to_json());
    assert_eq!(text(&nodes[0], "name"), "lab");
    assert_eq!(text(&hubs[0], "hub"), "hub");
}

/// fit's `TestPick`: a forward past `max_tunnels` refuses a connection; what no hub can carry is
/// refused.
#[test]
fn the_proxy_picks_a_hub() {
    let ProxyEnv { env, fwd, .. } = start_hub(r#", "max_tunnels": 1"#);
    let p = env.proxy();
    let held = echo_once(fwd, "held");
    refused(fwd);
    drop(held);
    let f = |node: &str, service: &str, hub: &str| akit_config::roles::TunnelForward {
        listen: String::new(),
        node: node.to_owned(),
        service: service.to_owned(),
        hub: hub.to_owned(),
    };
    assert!(p.pick(&f("lab", "echo", "nope")).is_err());
    assert!(p.pick(&f("lab", "nope", "")).is_err());
    assert_eq!(p.pick(&f("lab", "echo", "")).unwrap(), "hub");
}

/// The node's and the proxy's consoles under `admin.console`'s modules too, and moved with their
/// members.
#[test]
fn the_modules_are_listed() {
    let ProxyEnv { env, .. } = start_hub("");
    let ids = env.roles.lock().unwrap().modules().ids();
    for id in ["tunnel.hub", "tunnel.node", "tunnel.proxy"] {
        assert!(ids.contains(&id), "{ids:?}");
    }
}

#[path = "support/fit.rs"]
mod fit;

/// Against fit's binary (`AKIT_FIT_BIN`; skipped without it): akit's node and proxy linked to
/// fit's hub; the proxy's pages show fit's hub's nodes with their round trips, the node's
/// services' connects that fit's hub carries to it, and set the node's services through fit's
/// hub; the node's pages close a tunnel fit's hub routes.
#[test]
fn with_fits_hub() {
    let Ok(bin) = std::env::var("AKIT_FIT_BIN") else {
        eprintln!("AKIT_FIT_BIN not set: skipped");
        return;
    };
    let svc = echo();
    // fit's hub's link listener on port 0, where it listens read from its log.
    let fit_hub = fit::start(
        &bin,
        std::env::temp_dir().join(format!("akit-i109b-fit-{}", std::process::id())),
        r#"{"tunnel": {"hub": {"name": "fithub", "password": "pw", "allow_proxy_services": true, "listeners": [{"protocol": "tcp", "address": "127.0.0.1:0"}]}}}"#,
        |_| {},
    );
    let link = fit_hub.port("nodes and proxies link on ");
    let env = start(
        &format!(
            r#""node": {{"name": "lab", "allow_hub_services": true, "hubs": [{}], "services": [{{"name": "echo", "address": "127.0.0.1:{svc}"}}], "console": {}}},
          "proxy": {{"name": "me", "hubs": [{}], "forwards": [{{"listen": "127.0.0.1:0", "node": "lab", "service": "echo"}}], "console": {}}}"#,
            uplink("fithub", link),
            console_json("127.0.0.1:0"),
            uplink("fithub", link),
            console_json("127.0.0.1:0")
        ),
        |_| String::new(),
    );
    let (fwd, ncon, pcon) = env.ask(|r| {
        (
            forward_port(r),
            console_port(r, "tunnel.node"),
            console_port(r, "tunnel.proxy"),
        )
    });
    let p = env.proxy();
    wait_for("the proxy to know the node through fit's hub", || {
        p.has_node("fithub", "lab", "echo")
    });
    let t = echo_once(fwd, "through fit");
    let pp = login(pcon);
    // The node's services' connects, through fit's hub's pings, and its round trip to fit's hub.
    wait_for("the node's round trips at the proxy", || {
        let st = pp.status();
        arr(&st, "hubs")
            .first()
            .map(|h| arr(h, "nodes"))
            .and_then(|n| n.first().cloned())
            .is_some_and(|n| {
                n.get("service_rtt").is_some()
                    && n.get("rtt").is_some_and(|r| text(r, "samples") != "0")
            })
    });
    let st = pp.status();
    let hub = &arr(&st, "hubs")[0];
    assert_eq!(text(hub, "hub"), "fithub", "{}", st.to_json());
    assert_eq!(text(&arr(hub, "nodes")[0], "share"), "1");
    pp.expect(
        200,
        "/api/nodes/refresh",
        r#"{"hub": "fithub"}"#,
        r#""lab""#,
    );
    let extra = echo();
    pp.expect(
        200,
        "/api/nodes/services",
        &format!(
            r#"{{"hub": "fithub", "node": "lab", "services": [{{"name": "echo", "address": "127.0.0.1:{svc}"}}, {{"name": "extra", "address": "127.0.0.1:{extra}"}}]}}"#
        ),
        "ok",
    );
    wait_for("the node's new service through fit's hub", || {
        p.has_node("fithub", "lab", "extra")
    });
    assert!(
        env.member("node").contains("extra"),
        "{}",
        env.member("node")
    );
    // The node's pages close the tunnel fit's hub routes.
    let np = login(ncon);
    let st = np.status();
    let tunnels = arr(&st, "tunnels");
    assert_eq!(tunnels.len(), 1, "{}", st.to_json());
    assert_eq!(text(&tunnels[0], "peer"), "me");
    np.expect(
        200,
        "/api/tunnels/close",
        &format!(
            r#"{{"hub": "fithub", "id": "{}"}}"#,
            text(&tunnels[0], "id")
        ),
        "ok",
    );
    wait_for("the proxy's end to close", || p.tunnels() == 0);
    drop(t);
    eprintln!("akit's node's and proxy's pages over fit's hub: as fit's");
}
