//! End to end: `akit run` as a cluster manager (I132, fit's `cluster` tests of members, the
//! poller and the API): akit members reached by their console and by their Admin API's token,
//! kept in the manager's store across its restart; a wrong password, a member down and a draft
//! tested told apart; no credential in an answer or the log. With `AKIT_FIT_BIN` naming fit's
//! binary, mixed clusters (decision 041): fit members under akit's manager, by console and by
//! token, an akit member under fit's manager, by token, and the store's members read by both.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use akio_sys::process::{SIGTERM, kill};

#[path = "support/certs.rs"]
mod certs;

/// A directory of the test's own, removed when it ends.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("cluster")
            .join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }

    fn file(&self, name: &str, text: &str) -> PathBuf {
        let p = self.0.join(name);
        std::fs::write(&p, text).unwrap();
        p
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// `p` as a JSON string.
fn js(p: &Path) -> String {
    format!("{:?}", s(p))
}

struct Run {
    child: Option<Child>,
    out: Arc<Mutex<String>>,
    err: Arc<Mutex<String>>,
    readers: Vec<std::thread::JoinHandle<()>>,
}

fn keep(r: impl Read + Send + 'static, into: Arc<Mutex<String>>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for line in BufReader::new(r).lines() {
            let Ok(line) = line else { break };
            let mut s = into.lock().unwrap();
            s.push_str(&line);
            s.push('\n');
        }
    })
}

impl Run {
    fn start(args: &[&str]) -> Run {
        Run::spawn(Path::new(env!("CARGO_BIN_EXE_akit")), args)
    }

    /// `<bin> run <args>`: akit, or fit's binary.
    fn spawn(bin: &Path, args: &[&str]) -> Run {
        let mut child = Command::new(bin)
            .arg("run")
            .args(args)
            .args(["-shutdown-timeout", "5s"])
            .env_remove("AKIT_STORE_PASSWORD")
            .env_remove("FIT_STORE_PASSWORD")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (out, err) = (Arc::default(), Arc::default());
        let readers = vec![
            keep(child.stdout.take().unwrap(), Arc::clone(&out)),
            keep(child.stderr.take().unwrap(), Arc::clone(&err)),
        ];
        Run {
            child: Some(child),
            out,
            err,
            readers,
        }
    }

    fn out(&self) -> String {
        self.out.lock().unwrap().clone()
    }

    fn err(&self) -> String {
        self.err.lock().unwrap().clone()
    }

    /// Waits for a line of standard output holding `want`; the rest of that line after it.
    fn wait_for(&self, want: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(line) = self.out().lines().find(|l| l.contains(want)) {
                let at = line.find(want).unwrap() + want.len();
                return line[at..].to_owned();
            }
            assert!(
                Instant::now() < deadline,
                "waiting for {want:?}: {} {}",
                self.out(),
                self.err()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The address after `want` in standard output, up to a space or a slash.
    fn addr(&self, want: &str) -> SocketAddr {
        let rest = self.wait_for(want);
        let end = rest.find([' ', '/']).unwrap_or(rest.len());
        rest[..end].parse().unwrap()
    }

    fn signal(&self, sig: i32) {
        kill(self.child.as_ref().unwrap().id(), sig).unwrap();
    }

    /// Stops it with SIGTERM, unless it has stopped already: its exit code.
    fn stop(mut self) -> (Option<i32>, String, String) {
        let mut child = self.child.take().unwrap();
        if child.try_wait().unwrap().is_none() {
            let _ = kill(child.id(), SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(st) = child.try_wait().unwrap() {
                break st;
            }
            assert!(Instant::now() < deadline, "akit did not stop");
            std::thread::sleep(Duration::from_millis(20));
        };
        for r in self.readers.drain(..) {
            let _ = r.join();
        }
        (status.code(), self.out(), self.err())
    }

    /// Waits for it to exit by itself: its exit code, output and errors.
    fn exit(mut self) -> (Option<i32>, String, String) {
        let mut child = self.child.take().unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(st) = child.try_wait().unwrap() {
                break st;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                panic!("akit did not exit: {} {}", self.out(), self.err());
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        for r in self.readers.drain(..) {
            let _ = r.join();
        }
        (status.code(), self.out(), self.err())
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        if let Some(c) = &mut self.child {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// One request on a connection of its own: the status, the headers and the body.
fn http(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (n, v) in headers {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).unwrap();
    let mut raw = String::new();
    let _ = s.read_to_string(&mut raw);
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    (status, head.to_owned(), body.to_owned())
}

fn get(addr: SocketAddr, path: &str) -> String {
    http(addr, "GET", path, &[], "").2
}

struct Browser {
    addr: SocketAddr,
    name: &'static str,
    cookie: String,
    csrf: String,
}

impl Browser {
    fn new(addr: SocketAddr) -> Browser {
        Browser::of("akit", addr)
    }

    fn of(name: &'static str, addr: SocketAddr) -> Browser {
        Browser {
            addr,
            name,
            cookie: String::new(),
            csrf: String::new(),
        }
    }

    fn call(&self, method: &str, path: &str, body: &str) -> (u16, String, String) {
        self.call_with(method, path, &[], body)
    }

    fn call_with(
        &self,
        method: &str,
        path: &str,
        extra: &[(&str, &str)],
        body: &str,
    ) -> (u16, String, String) {
        let cookie = format!("{}_console_{}={}", self.name, self.addr.port(), self.cookie);
        let csrf = if self.name == "fit" {
            "X-Fit-Csrf"
        } else {
            "X-Akit-Csrf"
        };
        let mut h = vec![("Content-Type", "application/json")];
        if !self.cookie.is_empty() {
            h.push(("Cookie", &cookie));
        }
        if !self.csrf.is_empty() {
            h.push((csrf, &self.csrf));
        }
        h.extend_from_slice(extra);
        http(self.addr, method, path, &h, body)
    }

    fn login(&mut self, user: &str, pw: &str) -> (u16, String) {
        let (st, head, body) = self.call(
            "POST",
            "/api/login",
            &format!(r#"{{"username": "{user}", "password": "{pw}"}}"#),
        );
        if st == 200 {
            let set = head
                .lines()
                .find_map(|l| l.strip_prefix("Set-Cookie: "))
                .unwrap();
            let (_, v) = set.split(';').next().unwrap().split_once('=').unwrap();
            self.cookie = v.to_owned();
            let csrf = body.split("\"csrf\":\"").nth(1).unwrap();
            self.csrf = csrf[..csrf.find('"').unwrap()].to_owned();
        }
        (st, body)
    }
}

fn hash(pw: &str) -> String {
    akio_crypto::bcrypt::hash(pw.as_bytes(), akio_crypto::bcrypt::MIN_COST).unwrap()
}

const WEB: &str = r#""listeners": [{"name": "web", "address": "127.0.0.1:0"}],
"routes": [{"handler": {"module": "respond", "params": {"body": "one"}}}]"#;

/// A member's console users and the config of a member reached by its console.
fn console_member(d: &Dir, name: &str, pw_hash: &str) -> PathBuf {
    d.file(
        &format!("{name}.json"),
        &format!(
            r#"{{"admin": {{"address": "127.0.0.1:0", "console": {{"address": "127.0.0.1:0",
                "users": [{{"username": "ada", "password_hash": "{pw_hash}"}}]}}}}, {WEB}}}"#
        ),
    )
}

/// A member reached by its Admin API's token.
fn token_member(d: &Dir, name: &str, token: &str) -> PathBuf {
    let tf = d.file(&format!("{name}.token"), token);
    d.file(
        &format!("{name}.json"),
        &format!(
            r#"{{"admin": {{"address": "127.0.0.1:0", "token_file": {}}}, {WEB}}}"#,
            js(&tf)
        ),
    )
}

/// The value of member `key` in `v`, a JSON object, as JSON text.
fn member(v: &akit_config::value::Value, key: &str) -> String {
    v.get(key).map(|x| format!("{x:?}")).unwrap_or_default()
}

fn parse(body: &str) -> akit_config::value::Value {
    akit_config::value::parse(body).unwrap_or_else(|e| panic!("{e}: {body}"))
}

/// The state of member `name` in a manager's `GET /api/m/cluster/state`.
fn state_of(b: &Browser, name: &str) -> akit_config::value::Value {
    let (st, _, body) = b.call("GET", "/api/m/cluster/state", "");
    assert_eq!(st, 200, "{body}");
    let v = parse(&body);
    let Some(akit_config::value::Value::Array(nodes)) = v.get("nodes") else {
        panic!("{body}")
    };
    nodes
        .iter()
        .find(|n| matches!(n.get("name"), Some(akit_config::value::Value::String(s)) if s == name))
        .cloned()
        .unwrap_or_else(|| panic!("no {name} in {body}"))
}

fn str_member<'v>(v: &'v akit_config::value::Value, key: &str) -> &'v str {
    match v.get(key) {
        Some(akit_config::value::Value::String(s)) => s,
        _ => "",
    }
}

/// Waits for member `name`'s state to be `status` (and its problem `kind`, when given).
fn wait_state(b: &Browser, name: &str, status: &str, kind: &str) -> akit_config::value::Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let st = state_of(b, name);
        let k = st
            .get("error")
            .map(|e| str_member(e, "kind").to_owned())
            .unwrap_or_default();
        if str_member(&st, "status") == status && (kind.is_empty() || k == kind) {
            return st;
        }
        assert!(
            Instant::now() < deadline,
            "{name}: waiting for {status} {kind}: {}",
            member(&st, "status")
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A manager: `akit run -admin` with `admin.cluster` polling every second, its console user ada.
fn manager(d: &Dir, pw_hash: &str) -> PathBuf {
    d.file(
        "manager.json",
        &format!(
            r#"{{"admin": {{"address": "127.0.0.1:0", "console": {{"address": "127.0.0.1:0",
                "users": [{{"username": "ada", "password_hash": "{pw_hash}"}}]}},
                "cluster": {{"poll_interval": "1s", "timeout": "3s"}}}}, {WEB}}}"#
        ),
    )
}

fn add(b: &Browser, body: &str) {
    let (st, _, resp) = b.call("POST", "/api/m/cluster/nodes", body);
    assert_eq!(st, 201, "{body}: {resp}");
}

#[test]
fn manager_reads_akit_members() {
    let d = Dir::new("akit-members");
    let h = hash("pw");
    let a = Run::start(&["-c", s(&console_member(&d, "a", &h)), "-watch-certs=false"]);
    let a_console = a.addr("console on http://");
    let token = "a-token-of-member-b-0123456789";
    let bm = Run::start(&["-c", s(&token_member(&d, "b", token)), "-watch-certs=false"]);
    let b_api = bm.addr("admin API on ");
    // A port nothing listens on: a member down.
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let down_addr = free.local_addr().unwrap();
    drop(free);

    let mcfg = manager(&d, &h);
    let m = Run::start(&["-admin", "-c", s(&mcfg), "-watch-certs=false"]);
    let console = m.addr("console on http://");
    let mut b = Browser::new(console);
    assert_eq!(b.login("ada", "pw").0, 200);
    let (st, _, wb) = b.call("GET", "/api/workbench", "");
    assert_eq!(st, 200);
    assert!(wb.contains("\"cluster\""), "{wb}");
    let (st, _, me) = b.call("GET", "/api/m/cluster/me", "");
    assert_eq!(st, 200);
    assert!(me.contains("\"open\":true"), "{me}");

    add(
        &b,
        &format!(
            r#"{{"name": "a", "url": "http://{a_console}", "username": "ada", "password": "pw", "labels": {{"region": "eu"}}}}"#
        ),
    );
    add(
        &b,
        &format!(
            r#"{{"name": "b", "url": "http://{b_api}/", "access": "admin_api", "token": "{token}"}}"#
        ),
    );
    add(
        &b,
        &format!(
            r#"{{"name": "c", "url": "http://{a_console}", "username": "ada", "password": "wrong-pw"}}"#
        ),
    );
    add(
        &b,
        &format!(
            r#"{{"name": "d", "url": "http://{down_addr}", "access": "admin_api", "token": "t0ken"}}"#
        ),
    );
    let (st, _, resp) = b.call(
        "POST",
        "/api/m/cluster/nodes",
        r#"{"name": "a", "url": "http://127.0.0.1:1", "access": "admin_api", "token": "t"}"#,
    );
    assert_eq!(st, 409, "{resp}");
    let (st, _, resp) = b.call(
        "POST",
        "/api/m/cluster/nodes",
        r#"{"name": "e", "url": "http://example.com", "access": "admin_api", "token": "t"}"#,
    );
    assert_eq!(st, 400, "{resp}");
    assert!(resp.contains(r#""field":"url""#), "{resp}");

    let sa = wait_state(&b, "a", "up", "");
    let node = sa.get("node").unwrap();
    assert!(!str_member(node, "akit_version").is_empty(), "{sa:?}");
    assert!(
        str_member(node, "fib_version").starts_with("akio "),
        "{sa:?}"
    );
    assert!(
        sa.get("reloads").is_some() && sa.get("config").is_some(),
        "{sa:?}"
    );
    wait_state(&b, "b", "up", "");
    wait_state(&b, "c", "error", "auth_failed");
    wait_state(&b, "d", "down", "unreachable");

    // A member's reads, through the manager.
    let (st, _, body) = b.call("GET", "/api/m/cluster/nodes/a/read/status", "");
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("\"reloads\""), "{body}");
    let (st, _, body) = b.call("GET", "/api/m/cluster/nodes/b/read/versions", "");
    assert_eq!(st, 200, "{body}");
    let (st, _, body) = b.call("GET", "/api/m/cluster/nodes/b/read/check", "");
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("\"summary\""), "{body}");

    // Tests: a draft, and the member as it would be changed.
    let (st, _, body) = b.call(
        "POST",
        "/api/m/cluster/test",
        &format!(
            r#"{{"name": "x", "url": "http://{b_api}", "access": "admin_api", "token": "{token}"}}"#
        ),
    );
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("\"ok\":true"), "{body}");
    let (st, _, body) = b.call(
        "POST",
        "/api/m/cluster/nodes/c/test",
        &format!(r#"{{"url": "http://{a_console}", "username": "ada", "password": "pw"}}"#),
    );
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("\"ok\":true"), "{body}");
    let (st, _, body) = b.call("POST", "/api/m/cluster/nodes/c/test", "");
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("auth_failed"), "{body}");

    // c fixed, d removed.
    let (st, _, body) = b.call(
        "PUT",
        "/api/m/cluster/nodes/c",
        &format!(r#"{{"url": "http://{a_console}", "username": "ada", "password": "pw"}}"#),
    );
    assert_eq!(st, 200, "{body}");
    wait_state(&b, "c", "up", "");
    let (st, _, body) = b.call("DELETE", "/api/m/cluster/nodes/d", "");
    assert_eq!(st, 200, "{body}");

    // No credential in any answer.
    for path in [
        "/api/m/cluster/nodes",
        "/api/m/cluster/state",
        "/api/m/cluster/audit",
        "/api/m/cluster/nodes/a",
    ] {
        let (st, _, body) = b.call("GET", path, "");
        assert_eq!(st, 200, "{path}: {body}");
        assert!(
            !body.contains(token) && !body.contains("\"pw\"") && !body.contains("wrong-pw"),
            "{path}: {body}"
        );
    }
    let (_, _, audit) = b.call("GET", "/api/m/cluster/audit?action=node.", "");
    for a in ["node.add", "node.update", "node.remove", "node.test"] {
        assert!(audit.contains(a), "{a}: {audit}");
    }

    // The manager restarted keeps its members, and reads them again.
    let (code, out, err) = m.stop();
    assert_eq!(code, Some(0), "{out} {err}");
    assert!(!out.contains(token) && !err.contains(token), "{out} {err}");
    assert!(
        err.contains("member \"c\" is error") || out.contains("member \"c\" is error"),
        "{out} {err}"
    );
    let m = Run::start(&["-admin", "-c", s(&mcfg), "-watch-certs=false"]);
    let mut b = Browser::new(m.addr("console on http://"));
    assert_eq!(b.login("ada", "pw").0, 200);
    for n in ["a", "b", "c"] {
        wait_state(&b, n, "up", "");
    }
    let (_, _, list) = b.call("GET", "/api/m/cluster/nodes", "");
    assert!(!list.contains("\"name\":\"d\""), "{list}");

    // A member restarted is read again.
    drop(a);
    wait_state(&b, "a", "down", "");
    let _ = bm.stop();
    let _ = m.stop();
}

/// Mixed clusters (decision 041), with fit's binary: akit's manager reads fit members by console
/// and by token; fit's manager reads an akit member by token; a store an akit manager wrote lists
/// its members on fit's manager.
#[test]
fn mixed_clusters_with_fit() {
    let Some(fit) = std::env::var_os("AKIT_FIT_BIN").map(PathBuf::from) else {
        eprintln!("skipped: AKIT_FIT_BIN is not set");
        return;
    };
    let d = Dir::new("mixed");
    let h = hash("pw");
    let fa = Run::spawn(
        &fit,
        &["-c", s(&console_member(&d, "fa", &h)), "-watch-certs=false"],
    );
    let fa_console = fa.addr("console on http://");
    let token = "a-token-of-fit-member-0123456789";
    let fb = Run::spawn(
        &fit,
        &[
            "-c",
            s(&token_member(&d, "fb", token)),
            "-watch-certs=false",
        ],
    );
    let fb_api = fb.addr("admin API on ");
    let ak_token = "a-token-of-akit-member-0123456789";
    let ak = Run::start(&[
        "-c",
        s(&token_member(&d, "ak", ak_token)),
        "-watch-certs=false",
    ]);
    let ak_api = ak.addr("admin API on ");
    let ak_web = ak.addr("listener \"web\" on ");

    let mcfg = manager(&d, &h);
    let m = Run::start(&["-admin", "-c", s(&mcfg), "-watch-certs=false"]);
    let mut b = Browser::new(m.addr("console on http://"));
    assert_eq!(b.login("ada", "pw").0, 200);
    add(
        &b,
        &format!(
            r#"{{"name": "fa", "url": "http://{fa_console}", "username": "ada", "password": "pw"}}"#
        ),
    );
    add(
        &b,
        &format!(
            r#"{{"name": "fb", "url": "http://{fb_api}", "access": "admin_api", "token": "{token}"}}"#
        ),
    );
    add(
        &b,
        &format!(
            r#"{{"name": "ak", "url": "http://{ak_api}", "access": "admin_api", "token": "{ak_token}"}}"#
        ),
    );
    for n in ["fa", "fb", "ak"] {
        let st = wait_state(&b, n, "up", "");
        let v = str_member(st.get("node").unwrap(), "akit_version").to_owned();
        assert_eq!(v.starts_with("fit "), n != "ak", "{n}: {v}");
    }
    let (st, _, body) = b.call("GET", "/api/m/cluster/nodes/fa/read/check", "");
    assert_eq!(st, 200, "{body}");
    // fit members through the node switcher: read-only (another schema), fit's header fields
    // answered under akit's names, a dry run passed on, a write refused.
    let (st, _, wb) = b.call("GET", "/api/n/fb/workbench", "");
    assert_eq!(st, 200, "{wb}");
    assert!(
        wb.contains(r#""read_only":true"#) && wb.contains(r#""reason_kind":"version""#),
        "{wb}"
    );
    let (st, _, wb) = b.call("GET", "/api/n/fa/workbench", "");
    assert_eq!(st, 200, "{wb}");
    assert!(wb.contains(r#""read_only":true"#), "{wb}");
    let (st, head, cfg) = b.call("GET", "/api/n/fb/m/admin/config", "");
    assert_eq!(st, 200, "{cfg}");
    assert_eq!(header_of(&head, "Akit-Config-Version"), "1", "{head}");
    let (st, head, body) = b.call("PUT", "/api/n/fa/m/admin/config?dry_run=1", &cfg);
    assert_eq!(st, 200, "{body}");
    assert_eq!(header_of(&head, "Akit-Dry-Run"), "1", "{head}");
    let (st, _, body) = b.call("PUT", "/api/n/fb/m/admin/config", &cfg);
    assert_eq!(st, 409, "{body}");
    // A template on fit members: compared, and fit told what akit's manager owns there.
    let (st, _, g) = b.call(
        "POST",
        "/api/m/cluster/groups",
        r#"{"name": "fits", "members": ["fa", "fb"]}"#,
    );
    assert_eq!(st, 201, "{g}");
    let (st, _, v) = b.call(
        "POST",
        "/api/m/cluster/templates",
        r#"{"name": "t", "groups": ["fits"], "config": {"routes": [{"name": "x", "match": {"path": ["/x"]}, "handler": {"module": "respond", "params": {"body": "${node.name}"}}}]}}"#,
    );
    assert_eq!(st, 201, "{v}");
    let (st, _, sync) = b.call("POST", "/api/m/cluster/sync", r#"{"nodes": ["fa", "fb"]}"#);
    assert_eq!(st, 200, "{sync}");
    assert!(
        sync.contains(r#""reason":"missing""#) && !sync.contains("\"managed\":\"failed"),
        "{sync}"
    );
    wait_admin(fb_api, token, "/node/managed", |b| b.contains("routes[x]"));
    // Rollouts on fit members (I137): the template, a batch at a time, gated by fit's GET /node;
    // then its route copied from fit's fb to akit's ak.
    let (tid, r) = plan(&b, r#"{"kind": "template", "template": "t"}"#);
    assert_eq!(rstatus(&r, "fa"), "ready", "{r:?}");
    assert_eq!(rstatus(&r, "fb"), "ready", "{r:?}");
    let (st, v) = steer(
        &b,
        &tid,
        "start",
        r#"{"strategy": {"mode": "rolling", "batch_size": 1, "gate": {"wait_ms": 100}}}"#,
    );
    assert_eq!(st, 200, "{v}");
    let r = wait_rollout(&b, &tid, "done", state_is("done"));
    assert_eq!(rstatus(&r, "fb"), "done", "{r:?}");
    let (cid, r) = plan(
        &b,
        r#"{"kind": "copy", "from": "fb", "paths": ["routes[x]"], "nodes": ["ak"]}"#,
    );
    assert_eq!(rstatus(&r, "ak"), "ready", "{r:?}");
    let (st, v) = steer(&b, &cid, "start", r#"{"strategy": {"mode": "all"}}"#);
    assert_eq!(st, 200, "{v}");
    wait_rollout(&b, &cid, "done", state_is("done"));
    assert_eq!(get(ak_web, "/x"), "fb");
    let (code, _, err) = m.stop();
    assert_eq!(code, Some(0), "{err}");

    // fit's manager, on the store akit's manager wrote: it lists the members, and reads them.
    let fm = Run::spawn(&fit, &["-admin", "-c", s(&mcfg), "-watch-certs=false"]);
    let mut fb_browser = Browser::of("fit", fm.addr("console on http://"));
    assert_eq!(fb_browser.login("ada", "pw").0, 200);
    let (st, _, list) = fb_browser.call("GET", "/api/m/cluster/nodes", "");
    assert_eq!(st, 200, "{list}");
    for n in ["\"fa\"", "\"fb\"", "\"ak\""] {
        assert!(list.contains(n), "{n}: {list}");
    }
    for n in ["fa", "fb", "ak"] {
        wait_state(&fb_browser, n, "up", "");
    }
    // And the rollouts akit's manager kept, ended.
    let (st, _, list) = fb_browser.call("GET", "/api/m/cluster/rollouts", "");
    assert_eq!(st, 200, "{list}");
    for id in [&tid, &cid] {
        assert!(list.contains(&format!(r#""id":"{id}""#)), "{id}: {list}");
    }
    assert!(!list.contains(r#""state":"running""#), "{list}");
    let _ = fm.stop();
    let _ = (fa.stop(), fb.stop(), ak.stop());
}

/// A member over TLS (fit's `TestTLS` cases of I141): its certificate not trusted by the
/// system's CAs is reported with its fingerprint, a test shows it, its CA or its pin trusts it,
/// and a pin that does not match is told apart.
#[test]
fn members_over_tls() {
    let d = Dir::new("tls");
    let h = hash("pw");
    let ca = certs::Ca::new("akit cluster test CA");
    let (cert, key) = ca.leaf(&d.0, "member", &["127.0.0.1", "localhost"]);
    let cfg = d.file(
        "member.json",
        &format!(
            r#"{{"admin": {{"address": "127.0.0.1:0", "console": {{"address": "127.0.0.1:0",
                "cert_file": {}, "key_file": {},
                "users": [{{"username": "ada", "password_hash": "{h}"}}]}}}}, {WEB}}}"#,
            js(&cert),
            js(&key)
        ),
    );
    let mem = Run::start(&["-c", s(&cfg), "-watch-certs=false"]);
    let console = mem.addr("console on https://");
    let m = Run::start(&["-admin", "-c", s(&manager(&d, &h)), "-watch-certs=false"]);
    let mut b = Browser::new(m.addr("console on http://"));
    assert_eq!(b.login("ada", "pw").0, 200);
    let url = format!("https://{console}");
    // Not trusted by the system's CAs: the test shows the certificate for a person to confirm.
    let (st, _, body) = b.call(
        "POST",
        "/api/m/cluster/test",
        &format!(r#"{{"name": "t", "url": "{url}", "username": "ada", "password": "pw"}}"#),
    );
    assert_eq!(st, 200, "{body}");
    let v = parse(&body);
    assert_eq!(
        str_member(v.get("error").unwrap(), "kind"),
        "tls_untrusted",
        "{body}"
    );
    let fp = str_member(v.get("certificate").unwrap(), "fingerprint").to_owned();
    assert_eq!(fp.len(), 95, "{body}");
    add(
        &b,
        &format!(r#"{{"name": "t", "url": "{url}", "username": "ada", "password": "pw"}}"#),
    );
    let st = wait_state(&b, "t", "error", "tls_untrusted");
    assert_eq!(
        str_member(
            st.get("error").unwrap().get("certificate").unwrap(),
            "fingerprint"
        ),
        fp
    );
    // Its CA trusts it.
    let pem = ca.cert.to_pem().replace('\n', "\\n");
    let (code, _, body) = b.call(
        "PUT",
        "/api/m/cluster/nodes/t",
        &format!(r#"{{"url": "{url}", "username": "ada", "tls": {{"ca": "{pem}"}}}}"#),
    );
    assert_eq!(code, 200, "{body}");
    let st = wait_state(&b, "t", "up", "");
    assert_eq!(
        str_member(st.get("tls").unwrap(), "verified"),
        "ca",
        "{st:?}"
    );
    // Its pin trusts it; another pin does not.
    let (code, _, body) = b.call(
        "PUT",
        "/api/m/cluster/nodes/t",
        &format!(r#"{{"url": "{url}", "username": "ada", "tls": {{"fingerprint": "{fp}"}}}}"#),
    );
    assert_eq!(code, 200, "{body}");
    let st = wait_state(&b, "t", "up", "");
    assert_eq!(
        str_member(st.get("tls").unwrap(), "verified"),
        "pin",
        "{st:?}"
    );
    let wrong = "ab".repeat(32);
    let (code, _, body) = b.call(
        "PUT",
        "/api/m/cluster/nodes/t",
        &format!(r#"{{"url": "{url}", "username": "ada", "tls": {{"fingerprint": "{wrong}"}}}}"#),
    );
    assert_eq!(code, 200, "{body}");
    wait_state(&b, "t", "error", "tls_pin_mismatch");
    let _ = (m.stop(), mem.stop());
}

/// The value of header `name` in `head`, a response's head.
fn header_of(head: &str, name: &str) -> String {
    head.lines()
        .find_map(|l| {
            let (n, v) = l.split_once(':')?;
            n.trim()
                .eq_ignore_ascii_case(name)
                .then(|| v.trim().to_owned())
        })
        .unwrap_or_default()
}

/// The node switcher (I134, fit's `passthrough` tests): a member's own pages read and written
/// through the manager, by console and by token; writes audited with the body's digest, a stale
/// ETag refused as the member refuses it, a dry run passed on, what is not passed on refused.
#[test]
fn node_switcher() {
    let d = Dir::new("switcher");
    let h = hash("pw");
    let a = Run::start(&["-c", s(&console_member(&d, "a", &h)), "-watch-certs=false"]);
    let a_console = a.addr("console on http://");
    let a_web = a.addr("listener \"web\" on ");
    let token = "a-token-of-member-b-0123456789";
    let bm = Run::start(&["-c", s(&token_member(&d, "b", token)), "-watch-certs=false"]);
    let b_api = bm.addr("admin API on ");
    let m = Run::start(&["-admin", "-c", s(&manager(&d, &h)), "-watch-certs=false"]);
    let mut b = Browser::new(m.addr("console on http://"));
    assert_eq!(b.login("ada", "pw").0, 200);
    add(
        &b,
        &format!(
            r#"{{"name": "a", "url": "http://{a_console}", "username": "ada", "password": "pw"}}"#
        ),
    );
    add(
        &b,
        &format!(
            r#"{{"name": "b", "url": "http://{b_api}", "access": "admin_api", "token": "{token}"}}"#
        ),
    );
    wait_state(&b, "a", "up", "");
    wait_state(&b, "b", "up", "");

    // The member's workbench: writable, the same schema as the manager's.
    let (st, _, wb) = b.call("GET", "/api/n/a/workbench", "");
    assert_eq!(st, 200, "{wb}");
    assert!(
        wb.contains(r#""read_only":false"#) && wb.contains(r#""modules":["admin"]"#),
        "{wb}"
    );
    let (st, _, wb) = b.call("GET", "/api/n/b/workbench", "");
    assert_eq!(st, 200, "{wb}");
    assert!(wb.contains(r#""read_only":false"#), "{wb}");

    // A member's config read and changed through the manager, its ETag kept.
    let (st, head, cfg) = b.call("GET", "/api/n/a/m/admin/config", "");
    assert_eq!(st, 200, "{cfg}");
    let etag = header_of(&head, "ETag");
    assert!(
        !etag.is_empty() && header_of(&head, "Akit-Config-Version") == "1",
        "{head}"
    );
    assert!(cfg.contains("\"one\""), "{cfg}");
    let two = r#"{"handler": {"module": "respond", "params": {"body": "two"}}}"#;
    let (st, head, body) = b.call_with(
        "PUT",
        "/api/n/a/m/admin/config/routes/0",
        &[("If-Match", &etag)],
        two,
    );
    assert_eq!(st, 200, "{body}");
    assert_eq!(header_of(&head, "Akit-Config-Version"), "2", "{head}");
    assert_eq!(get(a_web, "/"), "two");
    // The old ETag is refused, as the member refuses it.
    let (st, _, body) = b.call_with(
        "PUT",
        "/api/n/a/m/admin/config/routes/0",
        &[("If-Match", &etag)],
        two,
    );
    assert_eq!(st, 412, "{body}");
    // A dry run changes nothing and is not audited.
    let (st, head, body) = b.call("PUT", "/api/n/b/m/admin/config/routes/0?dry_run=1", two);
    assert_eq!(st, 200, "{body}");
    assert_eq!(header_of(&head, "Akit-Dry-Run"), "1", "{head}");
    let (st, _, body) = b.call("GET", "/api/n/b/node", "");
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("\"capabilities\""), "{body}");

    // What is not passed on.
    for (path, want) in [
        ("/api/n/a/m/admin/users", 403),
        ("/api/n/a/m/cluster/api/nodes", 403),
        ("/api/n/a/m/admin/../x", 404),
        ("/api/n/nobody/m/admin/config", 404),
        ("/api/n/b/m/tunnel.hub/api/status", 404),
        ("/api/m/cluster/n/a/node", 404),
    ] {
        let (st, _, body) = b.call("GET", path, "");
        assert_eq!(st, want, "{path}: {body}");
    }
    let (st, _, body) = b.call_with(
        "GET",
        "/api/n/a/m/admin/config",
        &[("Accept", "text/event-stream")],
        "",
    );
    assert_eq!(st, 501, "{body}");

    // The writes are audited, with the digest of what was sent, never the body.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (_, _, audit) = b.call("GET", "/api/m/cluster/audit?action=node.api", "");
        if audit.matches("node.api").count() >= 3 {
            assert!(
                audit.contains("sha256:") && !audit.contains("respond"),
                "{audit}"
            );
            assert!(audit.contains("config version 2"), "{audit}");
            assert!(audit.contains("412"), "{audit}");
            break;
        }
        assert!(Instant::now() < deadline, "{audit}");
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = (m.stop(), a.stop(), bm.stop());
}

/// Waits for operation `id` to finish: its answer.
fn wait_op(b: &Browser, id: &str) -> akit_config::value::Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (st, _, body) = b.call("GET", &format!("/api/m/cluster/ops/{id}"), "");
        assert_eq!(st, 200, "{body}");
        let v = parse(&body);
        if matches!(
            v.get("finished"),
            Some(akit_config::value::Value::Bool(true))
        ) {
            return v;
        }
        assert!(Instant::now() < deadline, "{body}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Starts an operation: its ID.
fn start_op(b: &Browser, body: &str) -> String {
    let (st, _, resp) = b.call("POST", "/api/m/cluster/ops", body);
    assert_eq!(st, 202, "{body}: {resp}");
    str_member(&parse(&resp), "id").to_owned()
}

/// Each member's status and problem kind in an operation's answer.
fn outcomes(op: &akit_config::value::Value) -> Vec<(String, String, String)> {
    let Some(akit_config::value::Value::Array(os)) = op.get("outcomes") else {
        panic!("{op:?}")
    };
    os.iter()
        .map(|o| {
            (
                str_member(o, "node").to_owned(),
                str_member(o, "status").to_owned(),
                o.get("error")
                    .map(|e| str_member(e, "kind").to_owned())
                    .unwrap_or_default(),
            )
        })
        .collect()
}

/// Groups and operations on many (I135, fit's `groups` and `ops` tests): a group by selector and
/// by name follows the labels as they are; an operation on a group where one member is down
/// reports the others done and that one failed; rollback, purge, check and test on akit members;
/// the operations in the audit log; a member removed leaves the groups naming it.
#[test]
fn groups_and_operations() {
    let d = Dir::new("groups");
    let h = hash("pw");
    let a = Run::start(&["-c", s(&console_member(&d, "a", &h)), "-watch-certs=false"]);
    let a_console = a.addr("console on http://");
    let token = "a-token-of-members-b-and-c-012345";
    let bm = Run::start(&["-c", s(&token_member(&d, "b", token)), "-watch-certs=false"]);
    let b_api = bm.addr("admin API on ");
    let cm = Run::start(&["-c", s(&token_member(&d, "c", token)), "-watch-certs=false"]);
    let c_api = cm.addr("admin API on ");
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let down = free.local_addr().unwrap();
    drop(free);
    let m = Run::start(&["-admin", "-c", s(&manager(&d, &h)), "-watch-certs=false"]);
    let mut b = Browser::new(m.addr("console on http://"));
    assert_eq!(b.login("ada", "pw").0, 200);
    add(
        &b,
        &format!(
            r#"{{"name": "a", "url": "http://{a_console}", "username": "ada", "password": "pw", "labels": {{"region": "eu"}}}}"#
        ),
    );
    add(
        &b,
        &format!(
            r#"{{"name": "b", "url": "http://{b_api}", "access": "admin_api", "token": "{token}", "labels": {{"region": "eu"}}}}"#
        ),
    );
    add(
        &b,
        &format!(
            r#"{{"name": "c", "url": "http://{c_api}", "access": "admin_api", "token": "{token}", "labels": {{"region": "us"}}}}"#
        ),
    );
    add(
        &b,
        &format!(
            r#"{{"name": "d", "url": "http://{down}", "access": "admin_api", "token": "{token}", "labels": {{"region": "eu"}}}}"#
        ),
    );
    for n in ["a", "b", "c"] {
        wait_state(&b, n, "up", "");
    }
    wait_state(&b, "d", "down", "");

    // A group by selector and by name; its preview.
    let (st, _, g) = b.call(
        "POST",
        "/api/m/cluster/groups",
        r#"{"name": "eu", "selector": "region = eu", "members": ["c", "ghost"], "variables": {"domain": "eu.example"}}"#,
    );
    assert_eq!(st, 201, "{g}");
    assert!(g.contains(r#""selector":"region=eu""#), "{g}");
    assert!(
        g.contains(r#""nodes":["a","b","c","d"]"#) && g.contains(r#""missing":["ghost"]"#),
        "{g}"
    );
    let (st, _, sel) = b.call(
        "POST",
        "/api/m/cluster/select",
        r#"{"selector": "region!=eu"}"#,
    );
    assert_eq!(st, 200, "{sel}");
    assert!(sel.contains(r#""nodes":["c"]"#), "{sel}");
    let (st, _, body) = b.call(
        "POST",
        "/api/m/cluster/groups",
        r#"{"name": "x", "selector": "a in (b)"}"#,
    );
    assert_eq!(st, 400, "{body}");
    assert!(body.contains(r#""field":"selector""#), "{body}");
    assert!(member(&state_of(&b, "a"), "groups").contains("eu"));
    // A label changed moves a member out of the group its selector picked it by.
    let (st, _, body) = b.call(
        "PUT",
        "/api/m/cluster/nodes/b",
        &format!(
            r#"{{"url": "http://{b_api}", "access": "admin_api", "labels": {{"region": "us"}}}}"#
        ),
    );
    assert_eq!(st, 200, "{body}");
    let (st, _, g) = b.call("GET", "/api/m/cluster/groups/eu", "");
    assert_eq!(st, 200, "{g}");
    assert!(
        g.contains(r#""nodes":["a","c","d"]"#) && g.contains(r#""states":["#),
        "{g}"
    );

    // A reload on the group: three done, the member down failed.
    let id = start_op(&b, r#"{"op": "reload", "group": "eu"}"#);
    let op = wait_op(&b, &id);
    assert_eq!(
        outcomes(&op),
        [
            ("a".to_owned(), "done".to_owned(), String::new()),
            ("c".to_owned(), "done".to_owned(), String::new()),
            (
                "d".to_owned(),
                "failed".to_owned(),
                "unreachable".to_owned()
            ),
        ]
    );
    // No earlier config differs from the running one: nothing to roll back to.
    let id = start_op(&b, r#"{"op": "rollback", "members": ["a"]}"#);
    assert_eq!(outcomes(&wait_op(&b, &id))[0].2, "no_earlier_config");
    // A config changed on b, then rolled back to the one before.
    let (st, head, _) = b.call("GET", "/api/n/b/m/admin/config", "");
    assert_eq!(st, 200);
    let etag = header_of(&head, "ETag");
    let two = r#"{"handler": {"module": "respond", "params": {"body": "two"}}}"#;
    let (st, _, body) = b.call_with(
        "PUT",
        "/api/n/b/m/admin/config/routes/0",
        &[("If-Match", &etag)],
        two,
    );
    assert_eq!(st, 200, "{body}");
    let id = start_op(&b, r#"{"op": "rollback", "members": ["b"]}"#);
    let op = wait_op(&b, &id);
    assert_eq!(outcomes(&op)[0].1, "done", "{op:?}");
    // Purge, check and test.
    let id = start_op(
        &b,
        r#"{"op": "purge", "members": ["a", "b"], "purge": {"prefixes": ["https://a.example/x/"]}}"#,
    );
    let op = wait_op(&b, &id);
    assert!(
        outcomes(&op)
            .iter()
            .all(|o| o.1 == "done" || o.2 == "refused"),
        "{op:?}"
    );
    let id = start_op(&b, r#"{"op": "check", "members": ["a"]}"#);
    let op = wait_op(&b, &id);
    assert_eq!(outcomes(&op)[0].1, "done", "{op:?}");
    let id = start_op(
        &b,
        r#"{"op": "test", "group": "eu", "members": ["a", "c"]}"#,
    );
    let op = wait_op(&b, &id);
    assert!(outcomes(&op).iter().all(|o| o.1 == "done"), "{op:?}");
    for (body, want) in [
        (r#"{"op": "reboot", "members": ["a"]}"#, 400),
        (r#"{"op": "reload"}"#, 400),
        (r#"{"op": "reload", "members": ["nobody"]}"#, 404),
        (r#"{"op": "reload", "group": "eu", "members": ["b"]}"#, 400),
        (r#"{"op": "purge", "members": ["a"]}"#, 400),
        (
            r#"{"op": "reload", "members": ["a"], "purge": {"all": true}}"#,
            400,
        ),
    ] {
        let (st, _, resp) = b.call("POST", "/api/m/cluster/ops", body);
        assert_eq!(st, want, "{body}: {resp}");
    }
    let (st, _, list) = b.call("GET", "/api/m/cluster/ops", "");
    assert_eq!(st, 200);
    assert!(list.matches("\"id\"").count() >= 6, "{list}");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (_, _, audit) = b.call("GET", "/api/m/cluster/audit?action=op.", "");
        if audit.contains("op.reload") && audit.contains("op.test") {
            assert!(
                audit.contains(r#""outcome":"partial""#) && audit.contains("\"outcomes\""),
                "{audit}"
            );
            break;
        }
        assert!(Instant::now() < deadline, "{audit}");
        std::thread::sleep(Duration::from_millis(100));
    }

    // A member removed leaves the group that named it; the group removed.
    let (st, _, body) = b.call("DELETE", "/api/m/cluster/nodes/c", "");
    assert_eq!(st, 200, "{body}");
    let (_, _, g) = b.call("GET", "/api/m/cluster/groups/eu", "");
    assert!(g.contains(r#""members":["ghost"]"#), "{g}");
    let (st, _, body) = b.call("DELETE", "/api/m/cluster/groups/eu", "");
    assert_eq!(st, 200, "{body}");
    let (_, _, gs) = b.call("GET", "/api/m/cluster/groups", "");
    assert!(gs.contains(r#""groups":[]"#), "{gs}");
    let _ = (m.stop(), a.stop(), bm.stop(), cm.stop());
}

/// `GET path` on an Admin API with its bearer token: the status and body.
fn admin_get(addr: SocketAddr, token: &str, path: &str) -> (u16, String) {
    let auth = format!("Bearer {token}");
    let (st, _, body) = http(addr, "GET", path, &[("Authorization", &auth)], "");
    (st, body)
}

/// Waits for `cond` on `GET path` of an Admin API: the body that met it.
fn wait_admin(addr: SocketAddr, token: &str, path: &str, cond: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let (st, body) = admin_get(addr, token, path);
        if st == 200 && cond(&body) {
            return body;
        }
        assert!(Instant::now() < deadline, "{path}: {st} {body}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Templates and each member's sync state (I136, fit's `templates`, `render` and `sync` tests):
/// a template of a route attached to a group renders per member with its variables; members told
/// what the manager owns (`/node/managed`) and their pages marking it; a member brought in sync, a
/// part changed on it a conflict; a template made from a member's config; import, compare and the
/// text tabs; secrets masked and kept; a group renamed followed; a template removed, the members
/// told they own nothing.
#[test]
fn templates_and_sync() {
    let d = Dir::new("templates");
    let h = hash("pw");
    let a = Run::start(&["-c", s(&console_member(&d, "a", &h)), "-watch-certs=false"]);
    let a_console = a.addr("console on http://");
    let token = "a-token-of-member-b-0123456789";
    let bm = Run::start(&["-c", s(&token_member(&d, "b", token)), "-watch-certs=false"]);
    let b_api = bm.addr("admin API on ");
    let b_web = bm.addr("listener \"web\" on ");
    let m = Run::start(&["-admin", "-c", s(&manager(&d, &h)), "-watch-certs=false"]);
    let mut b = Browser::new(m.addr("console on http://"));
    assert_eq!(b.login("ada", "pw").0, 200);
    add(
        &b,
        &format!(
            r#"{{"name": "a", "url": "http://{a_console}", "username": "ada", "password": "pw", "labels": {{"region": "eu"}}}}"#
        ),
    );
    add(
        &b,
        &format!(
            r#"{{"name": "b", "url": "http://{b_api}", "access": "admin_api", "token": "{token}", "labels": {{"region": "eu"}}}}"#
        ),
    );
    wait_state(&b, "a", "up", "");
    wait_state(&b, "b", "up", "");
    let (st, _, g) = b.call(
        "POST",
        "/api/m/cluster/groups",
        r#"{"name": "eu", "selector": "region=eu", "variables": {"domain": "eu.example"}}"#,
    );
    assert_eq!(st, 201, "{g}");

    // A template of a route, its variables and members.
    let tpl = r#"{"name": "base", "groups": ["eu"], "config": {"routes": [{"name": "tpl", "match": {"path": ["/tpl"]},
        "handler": {"module": "respond", "params": {"body": "${node.name} in ${domain}"}}}]}}"#;
    let (st, _, v) = b.call("POST", "/api/m/cluster/templates", tpl);
    assert_eq!(st, 201, "{v}");
    for want in [
        r#""parts":[{"path":"routes[tpl]","member":"routes","name":"tpl"}]"#,
        r#""variables":["domain","node.name"]"#,
        r#""nodes":["a","b"]"#,
    ] {
        assert!(v.contains(want), "{want}: {v}");
    }
    let (st, _, bad) = b.call(
        "POST",
        "/api/m/cluster/templates",
        r#"{"name": "x", "config": {"routes": [{"match": {}}]}}"#,
    );
    assert_eq!(st, 400, "{bad}");
    assert!(bad.contains("has no name"), "{bad}");

    // Read now: missing on both, which are told what the manager owns.
    let (st, _, sync) = b.call("POST", "/api/m/cluster/sync", "{}");
    assert_eq!(st, 200, "{sync}");
    assert!(
        sync.contains(r#""state":"out_of_sync""#) && sync.contains(r#""reason":"missing""#),
        "{sync}"
    );
    // Told when the template was added, or now.
    assert!(
        sync.contains(r#""managed":"told""#) || sync.contains(r#""managed":"kept""#),
        "{sync}"
    );
    let rec = wait_admin(b_api, token, "/node/managed", |b| b.contains("routes[tpl]"));
    assert!(
        rec.contains(r#""template":"base""#) && rec.contains(r#""present":false"#),
        "{rec}"
    );
    let (st, _, desired) = b.call("GET", "/api/m/cluster/desired/a", "");
    assert_eq!(st, 200, "{desired}");
    assert!(desired.contains("a in eu.example"), "{desired}");

    // Brought in sync on b (as a rollout will): in sync, its hash kept, not changed.
    let (st, _, desired) = b.call("GET", "/api/m/cluster/desired/b", "");
    assert_eq!(st, 200);
    let dv = parse(&desired);
    let routes = dv.get("desired").and_then(|d| d.get("routes")).unwrap();
    let routes_json = format!("{routes:?}");
    assert!(routes_json.contains("b in eu.example"), "{routes_json}");
    let (st, head, _) = b.call("GET", "/api/n/b/m/admin/config", "");
    assert_eq!(st, 200);
    let etag = header_of(&head, "ETag");
    let body = akit_config::value::Value::to_json(routes);
    let (st, _, resp) = b.call_with(
        "PUT",
        "/api/n/b/m/admin/config/routes",
        &[("If-Match", &etag)],
        &body,
    );
    assert_eq!(st, 200, "{resp}");
    assert_eq!(get(b_web, "/tpl"), "b in eu.example");
    let (_, _, sync) = b.call("POST", "/api/m/cluster/sync", r#"{"nodes": ["b"]}"#);
    assert!(sync.contains(r#""state":"in_sync""#), "{sync}");
    let rec = wait_admin(b_api, token, "/node/managed", |b| b.contains(r#""hash":""#));
    assert!(
        rec.contains(r#""changed":false"#) && rec.contains(r#""present":true"#),
        "{rec}"
    );

    // Changed on b: a conflict, and b's record marks it changed.
    let (st, head, _) = b.call("GET", "/api/n/b/m/admin/config", "");
    assert_eq!(st, 200);
    let etag = header_of(&head, "ETag");
    let (st, _, resp) = b.call_with(
        "PUT",
        "/api/n/b/m/admin/config/routes/1/handler/params/body",
        &[("If-Match", &etag)],
        r#""edited""#,
    );
    assert_eq!(st, 200, "{resp}");
    let (_, _, sync) = b.call("POST", "/api/m/cluster/sync", r#"{"nodes": ["b"]}"#);
    assert!(
        sync.contains(r#""reason":"changed_here""#) && sync.contains(r#""state":"conflict""#),
        "{sync}"
    );
    wait_admin(b_api, token, "/node/managed", |b| {
        b.contains(r#""changed":true"#)
    });
    let (_, _, states) = b.call("GET", "/api/m/cluster/sync", "");
    assert!(
        states.contains(r#""node":"a""#) && states.contains(r#""node":"b""#),
        "{states}"
    );
    assert!(!states.contains("\"have\""), "{states}");

    // A template made from a member's config; import, compare and the text tabs.
    let (st, _, v) = b.call(
        "POST",
        "/api/m/cluster/templates",
        r#"{"name": "copy", "from": {"node": "b", "paths": ["routes[tpl]"]}}"#,
    );
    assert_eq!(st, 201, "{v}");
    assert!(v.contains("edited") && v.contains(r#""nodes":[]"#), "{v}");
    let (st, _, imp) = b.call("POST", "/api/m/cluster/import", r#"{"node": "a"}"#);
    assert_eq!(st, 200, "{imp}");
    assert!(
        imp.contains(r#""path":"routes[#0]""#) && imp.contains(r#""ownable":false"#),
        "{imp}"
    );
    let (st, _, cmp) = b.call("POST", "/api/m/cluster/compare", r#"{"a": "a", "b": "b"}"#);
    assert_eq!(st, 200, "{cmp}");
    assert!(
        cmp.contains(r#""path":"routes[tpl]","state":"only_b""#),
        "{cmp}"
    );
    let (st, _, conf) = b.call(
        "POST",
        "/api/m/cluster/text/render?format=conf",
        r#"{"log": {"level": "debug"}}"#,
    );
    assert_eq!(st, 200, "{conf}");
    let (st, _, back) = b.call_with(
        "POST",
        "/api/m/cluster/text/parse?format=conf",
        &[("Content-Type", "text/plain")],
        &conf,
    );
    assert_eq!(st, 200, "{back}");
    assert!(back.contains("debug"), "{back}");

    // Secrets masked in what the API answers, kept when sent back masked.
    let secret_tpl = r#"{"name": "auth", "config": {"telemetry": {"endpoint": "http://127.0.0.1:4318", "headers": {"api_key": "k3y"}}}}"#;
    let (st, _, v) = b.call("POST", "/api/m/cluster/templates", secret_tpl);
    assert_eq!(st, 201, "{v}");
    assert!(!v.contains("k3y") && v.contains("[secret]"), "{v}");
    let (st, _, v) = b.call("PUT", "/api/m/cluster/templates/auth", r#"{"description": "kept", "config": {"telemetry": {"endpoint": "http://127.0.0.1:4318", "headers": {"api_key": "[secret]"}}}}"#);
    assert_eq!(st, 200, "{v}");
    let (st, _, v) = b.call(
        "PUT",
        "/api/m/cluster/templates/auth",
        r#"{"config": {"log": {"x": "[secret]"}}}"#,
    );
    assert_eq!(st, 400, "{v}");

    // A group renamed is followed by its templates; a template removed leaves its members owning
    // nothing.
    let (st, _, g) = b.call(
        "PUT",
        "/api/m/cluster/groups/eu",
        r#"{"name": "europe", "selector": "region=eu", "variables": {"domain": "eu.example"}}"#,
    );
    assert_eq!(st, 200, "{g}");
    let (_, _, v) = b.call("GET", "/api/m/cluster/templates/base", "");
    assert!(v.contains(r#""groups":["europe"]"#), "{v}");
    let (st, _, v) = b.call("DELETE", "/api/m/cluster/templates/base", "");
    assert_eq!(st, 200, "{v}");
    wait_admin(b_api, token, "/node/managed", |b| {
        b.contains(r#""paths":[]"#)
    });
    let (_, _, audit) = b.call("GET", "/api/m/cluster/audit?action=template.", "");
    assert!(
        audit.contains("template.add") && audit.contains("template.remove"),
        "{audit}"
    );
    let _ = (m.stop(), a.stop(), bm.stop());
}

/// Waits for rollout `id` to meet `cond`: the rollout as `GET /api/m/cluster/rollouts/<id>`
/// answers it.
fn wait_rollout(
    b: &Browser,
    id: &str,
    what: &str,
    cond: impl Fn(&akit_config::value::Value) -> bool,
) -> akit_config::value::Value {
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        let (st, _, body) = b.call("GET", &format!("/api/m/cluster/rollouts/{id}"), "");
        assert_eq!(st, 200, "{body}");
        let v = parse(&body);
        if cond(&v) {
            return v;
        }
        assert!(
            Instant::now() < deadline,
            "rollout {id}: waiting for {what}: {body}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn state_is(state: &'static str) -> impl Fn(&akit_config::value::Value) -> bool {
    move |r| str_member(r, "state") == state
}

/// The status of member `node` in rollout `r`.
fn rstatus(r: &akit_config::value::Value, node: &str) -> String {
    let Some(akit_config::value::Value::Array(ms)) = r.get("members") else {
        return String::new();
    };
    ms.iter()
        .find(|m| str_member(m, "node") == node)
        .map(|m| str_member(m, "status").to_owned())
        .unwrap_or_default()
}

/// Plans a rollout: its ID, once planned.
fn plan(b: &Browser, body: &str) -> (String, akit_config::value::Value) {
    let (st, _, v) = b.call("POST", "/api/m/cluster/rollouts", body);
    assert_eq!(st, 202, "{body}: {v}");
    assert!(v.contains(r#""state":"planning""#), "{v}");
    let id = str_member(&parse(&v), "id").to_owned();
    let r = wait_rollout(b, &id, "its plan", state_is("planned"));
    (id, r)
}

fn steer(b: &Browser, id: &str, action: &str, body: &str) -> (u16, String) {
    let (st, _, v) = b.call(
        "POST",
        &format!("/api/m/cluster/rollouts/{id}/{action}"),
        body,
    );
    (st, v)
}

/// Rollouts with health gates (I137, fit's `rollouts` tests): a template rolled out in rolling
/// batches; a plan in sync unchanged, and discarded; a canary that waits, a person's go, pause and
/// resume; a member changed since the plan skipped, not overwritten; a person's rollback; a gate
/// on the share of 5xx that fails and rolls its batch back, and one that cannot be judged where
/// requests are not counted; a manager restarted with a rollout under way finds it paused, its
/// members held, and an abort ends it.
#[test]
fn rollouts_with_gates() {
    let d = Dir::new("rollouts");
    let h = hash("pw");
    let a = Run::start(&["-c", s(&console_member(&d, "a", &h)), "-watch-certs=false"]);
    let a_console = a.addr("console on http://");
    let a_web = a.addr("listener \"web\" on ");
    let tb = "a-token-of-member-b-0123456789";
    let bm = Run::start(&["-c", s(&token_member(&d, "b", tb)), "-watch-certs=false"]);
    let b_api = bm.addr("admin API on ");
    let b_web = bm.addr("listener \"web\" on ");
    // c counts its requests, and answers /boom with a 500.
    let tc = "a-token-of-member-c-0123456789";
    let tcf = d.file("c.token", tc);
    let ccfg = d.file(
        "c.json",
        &format!(
            r#"{{"admin": {{"address": "127.0.0.1:0", "token_file": {}}}, "metrics": {{}},
                "listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
                "routes": [{{"name": "boom", "match": {{"path": ["/boom"]}}, "handler": {{"module": "respond", "params": {{"status": 500, "body": "boom"}}}}}},
                           {{"handler": {{"module": "respond", "params": {{"body": "one"}}}}}}]}}"#,
            js(&tcf)
        ),
    );
    let cm = Run::start(&["-c", s(&ccfg), "-watch-certs=false"]);
    let c_api = cm.addr("admin API on ");
    let c_web = cm.addr("listener \"web\" on ");
    let mcfg = manager(&d, &h);
    let m = Run::start(&["-admin", "-c", s(&mcfg), "-watch-certs=false"]);
    let mut b = Browser::new(m.addr("console on http://"));
    assert_eq!(b.login("ada", "pw").0, 200);
    add(
        &b,
        &format!(
            r#"{{"name": "a", "url": "http://{a_console}", "username": "ada", "password": "pw", "labels": {{"region": "eu"}}}}"#
        ),
    );
    for (n, api, t) in [("b", b_api, tb), ("c", c_api, tc)] {
        add(
            &b,
            &format!(
                r#"{{"name": "{n}", "url": "http://{api}", "access": "admin_api", "token": "{t}", "labels": {{"region": "eu"}}}}"#
            ),
        );
    }
    for n in ["a", "b", "c"] {
        wait_state(&b, n, "up", "");
    }
    let (st, _, g) = b.call(
        "POST",
        "/api/m/cluster/groups",
        r#"{"name": "eu", "selector": "region=eu"}"#,
    );
    assert_eq!(st, 201, "{g}");
    let tpl = |v: &str| {
        format!(
            r#"{{"name": "base", "groups": ["eu"], "config": {{"routes": [{{"name": "tpl", "match": {{"path": ["/tpl"]}},
            "handler": {{"module": "respond", "params": {{"body": "${{node.name}} {v}"}}}}}}]}}}}"#
        )
    };
    let (st, _, v) = b.call("POST", "/api/m/cluster/templates", &tpl("v1"));
    assert_eq!(st, 201, "{v}");

    // The template, planned: each member's change and dry run; then rolled out two at a time.
    let (st, _, bad) = b.call("POST", "/api/m/cluster/rollouts", r#"{"kind": "nope"}"#);
    assert_eq!(st, 400, "{bad}");
    let (id, r) = plan(
        &b,
        r#"{"kind": "template", "template": "base", "description": "v1"}"#,
    );
    for n in ["a", "b", "c"] {
        assert_eq!(rstatus(&r, n), "ready", "{r:?}");
    }
    let (_, _, one) = b.call("GET", &format!("/api/m/cluster/rollouts/{id}"), "");
    assert!(
        one.contains(r#""path":"routes[tpl]","state":"added""#),
        "{one}"
    );
    let (_, _, list) = b.call("GET", "/api/m/cluster/rollouts", "");
    assert!(
        list.contains(r#""max_active":4"#) && !list.contains("\"changes\""),
        "{list}"
    );
    let (st, v) = steer(
        &b,
        &id,
        "start",
        r#"{"strategy": {"mode": "rolling", "batch_size": 4}}"#,
    );
    assert_eq!(st, 400, "{v}");
    let (st, v) = steer(
        &b,
        &id,
        "start",
        r#"{"strategy": {"mode": "rolling", "batch_size": 2, "gate": {"wait_ms": 100}}}"#,
    );
    assert_eq!(st, 200, "{v}");
    let r = wait_rollout(&b, &id, "done", state_is("done"));
    assert!(
        member(&r, "counts").contains("done") && rstatus(&r, "c") == "done",
        "{r:?}"
    );
    assert_eq!(get(a_web, "/tpl"), "a v1");
    assert_eq!(get(b_web, "/tpl"), "b v1");
    assert_eq!(get(c_web, "/tpl"), "c v1");

    // Planned again: in sync, nothing to start; the plan discarded.
    let (id, r) = plan(&b, r#"{"kind": "template", "template": "base"}"#);
    assert_eq!(rstatus(&r, "a"), "unchanged", "{r:?}");
    let (st, v) = steer(&b, &id, "start", r#"{"strategy": {"mode": "all"}}"#);
    assert_eq!(st, 409, "{v}");
    let (st, _, v) = b.call("DELETE", &format!("/api/m/cluster/rollouts/{id}"), "");
    assert_eq!(st, 200, "{v}");
    assert!(v.contains(&format!(r#""discarded":"{id}""#)), "{v}");
    let (st, _, _) = b.call("GET", &format!("/api/m/cluster/rollouts/{id}"), "");
    assert_eq!(st, 404);

    // v2 by a sync, a canary first; c changed since the plan is skipped.
    let (st, _, v) = b.call("PUT", "/api/m/cluster/templates/base", &tpl("v2"));
    assert_eq!(st, 200, "{v}");
    let (id, r) = plan(&b, r#"{"kind": "sync", "group": "eu"}"#);
    assert_eq!(rstatus(&r, "c"), "ready", "{r:?}");
    let (st, head, _) = b.call("GET", "/api/n/c/m/admin/config", "");
    assert_eq!(st, 200);
    let etag = header_of(&head, "ETag");
    let (st, _, resp) = b.call_with(
        "PUT",
        "/api/n/c/m/admin/config/routes/0/handler/params/body",
        &[("If-Match", &etag)],
        r#""boom!""#,
    );
    assert_eq!(st, 200, "{resp}");
    let (st, v) = steer(
        &b,
        &id,
        "start",
        r#"{"strategy": {"mode": "canary", "canary": 1, "batch_size": 1, "pause_ms": 60000, "gate": {"wait_ms": 100}}}"#,
    );
    assert_eq!(st, 200, "{v}");
    let r = wait_rollout(&b, &id, "its canary", state_is("waiting"));
    assert_eq!(rstatus(&r, "a"), "done", "{r:?}");
    assert_eq!(get(a_web, "/tpl"), "a v2");
    assert_eq!(get(b_web, "/tpl"), "b v1");
    let (st, v) = steer(&b, &id, "pause", "");
    assert_eq!(st, 409, "{v}");
    let (st, v) = steer(&b, &id, "go", "");
    assert_eq!(st, 200, "{v}");
    // b's batch passes; the pause before c's is cut short by a person's pause.
    wait_rollout(&b, &id, "the pause after b", |r| {
        rstatus(r, "b") == "done" && r.get("next_at").is_some()
    });
    let (st, v) = steer(&b, &id, "pause", "");
    assert_eq!(st, 200, "{v}");
    let r = wait_rollout(&b, &id, "paused", state_is("paused"));
    assert!(
        str_member(&r, "reason").contains("paused by a person"),
        "{r:?}"
    );
    assert_eq!(rstatus(&r, "c"), "pending", "{r:?}");
    let (st, v) = steer(&b, &id, "resume", "");
    assert_eq!(st, 200, "{v}");
    let r = wait_rollout(&b, &id, "done", state_is("done"));
    assert_eq!(rstatus(&r, "c"), "skipped", "{r:?}");
    let (_, _, one) = b.call("GET", &format!("/api/m/cluster/rollouts/{id}"), "");
    assert!(one.contains("it was not overwritten"), "{one}");
    assert_eq!(get(c_web, "/tpl"), "c v1");
    assert_eq!(get(c_web, "/boom"), "boom!");
    // A person's rollback: a and b run v1 again.
    let (st, v) = steer(&b, &id, "rollback", "");
    assert_eq!(st, 200, "{v}");
    let r = wait_rollout(&b, &id, "rolled back", state_is("rolled_back"));
    assert_eq!(rstatus(&r, "a"), "rolled_back", "{r:?}");
    assert_eq!(get(a_web, "/tpl"), "a v1");
    assert_eq!(get(b_web, "/tpl"), "b v1");

    // v3: a gate on the share of 5xx where a counts no requests cannot be judged; on c, whose
    // /boom answers 500s in the window, it fails, and c is rolled back.
    let (st, _, v) = b.call("PUT", "/api/m/cluster/templates/base", &tpl("v3"));
    assert_eq!(st, 200, "{v}");
    let gate = r#"{"strategy": {"mode": "all", "gate": {"wait_ms": 100, "max_5xx_ratio": 0.5, "window_ms": 5000, "min_requests": 5}}}"#;
    let (id, _) = plan(
        &b,
        r#"{"kind": "template", "template": "base", "nodes": ["a"]}"#,
    );
    let (st, v) = steer(&b, &id, "start", gate);
    assert_eq!(st, 200, "{v}");
    let r = wait_rollout(&b, &id, "failed", state_is("failed"));
    assert!(
        str_member(&r, "reason").contains("cannot be gated on a"),
        "{r:?}"
    );
    assert_eq!(get(a_web, "/tpl"), "a v1");
    let (id, _) = plan(
        &b,
        r#"{"kind": "template", "template": "base", "nodes": ["c"]}"#,
    );
    let (st, v) = steer(&b, &id, "start", gate);
    assert_eq!(st, 200, "{v}");
    // Gating once c's requests counted so far were read: the 500s from now on are in its window.
    wait_rollout(&b, &id, "c's gate", |r| {
        let Some(akit_config::value::Value::Array(bs)) = r.get("batches") else {
            return false;
        };
        rstatus(r, "c") == "applied" && bs.iter().any(|x| str_member(x, "state") == "gating")
    });
    for _ in 0..20 {
        let (st, _, _) = http(c_web, "GET", "/boom", &[], "");
        assert_eq!(st, 500);
    }
    let r = wait_rollout(&b, &id, "failed", state_is("failed"));
    let reason = str_member(&r, "reason").to_owned();
    assert!(
        reason.contains("the gate of batch 1 failed") && reason.contains("its batch rolled back"),
        "{r:?}"
    );
    assert_eq!(rstatus(&r, "c"), "rolled_back", "{r:?}");
    assert_eq!(get(c_web, "/tpl"), "c v1");

    // A rollout under way when the manager stops is paused where it stopped, and still holds its
    // members; aborted, it lets them go.
    let (id, _) = plan(&b, r#"{"kind": "sync", "nodes": ["a", "b"]}"#);
    let (st, v) = steer(
        &b,
        &id,
        "start",
        r#"{"strategy": {"mode": "rolling", "batch_size": 1, "pause_ms": 600000, "gate": {"wait_ms": 0}}}"#,
    );
    assert_eq!(st, 200, "{v}");
    wait_rollout(&b, &id, "the pause after a", |r| {
        rstatus(r, "a") == "done" && r.get("next_at").is_some()
    });
    let (code, _, err) = m.stop();
    assert_eq!(code, Some(0), "{err}");
    let m = Run::start(&["-admin", "-c", s(&mcfg), "-watch-certs=false"]);
    let mut b = Browser::new(m.addr("console on http://"));
    assert_eq!(b.login("ada", "pw").0, 200);
    let r = wait_rollout(&b, &id, "paused", state_is("paused"));
    assert!(
        str_member(&r, "reason").contains("paused where it stopped"),
        "{r:?}"
    );
    assert_eq!(get(a_web, "/tpl"), "a v3");
    assert_eq!(get(b_web, "/tpl"), "b v1");
    wait_state(&b, "b", "up", "");
    let (st, _, v) = b.call(
        "POST",
        "/api/m/cluster/ops",
        r#"{"op": "test", "members": ["b"]}"#,
    );
    assert_eq!(st, 409, "{v}");
    assert!(v.contains(&format!("rollout {id}")), "{v}");
    let (st, v) = steer(&b, &id, "abort", "");
    assert_eq!(st, 200, "{v}");
    let r = wait_rollout(&b, &id, "aborted", state_is("aborted"));
    assert_eq!(rstatus(&r, "b"), "pending", "{r:?}");
    let op = start_op(&b, r#"{"op": "test", "members": ["b"]}"#);
    wait_op(&b, &op);

    // A draft of the template, saved as the template when it starts; a patch, which takes no
    // variables.
    let draft = r#"{"kind": "template", "template": "base", "nodes": ["b"], "config": {"routes": [{"name": "tpl", "match": {"path": ["/tpl"]},
        "handler": {"module": "respond", "params": {"body": "${node.name} v4"}}}]}}"#;
    let (id, _) = plan(&b, draft);
    let (_, _, t) = b.call("GET", "/api/m/cluster/templates/base", "");
    assert!(t.contains("v3") && !t.contains("v4"), "{t}");
    let (st, v) = steer(&b, &id, "start", r#"{"strategy": {"mode": "all"}}"#);
    assert_eq!(st, 200, "{v}");
    wait_rollout(&b, &id, "done", state_is("done"));
    assert_eq!(get(b_web, "/tpl"), "b v4");
    let (_, _, t) = b.call("GET", "/api/m/cluster/templates/base", "");
    assert!(t.contains("v4"), "{t}");
    let patch = |body: &str| {
        format!(
            r#"{{"kind": "patch", "nodes": ["a"], "config": {{"routes": [{{"name": "p", "match": {{"path": ["/p"]}},
            "handler": {{"module": "respond", "params": {{"body": "{body}"}}}}}}]}}}}"#
        )
    };
    let (st, _, v) = b.call("POST", "/api/m/cluster/rollouts", &patch("${node.name}"));
    assert_eq!(st, 400, "{v}");
    assert!(v.contains("a patch takes no variables"), "{v}");
    let (id, _) = plan(&b, &patch("patched"));
    let (st, v) = steer(&b, &id, "start", r#"{"strategy": {"mode": "all"}}"#);
    assert_eq!(st, 200, "{v}");
    wait_rollout(&b, &id, "done", state_is("done"));
    assert_eq!(get(a_web, "/p"), "patched");
    // The audit log, once the store has been written.
    let want = [
        "rollout.plan",
        "rollout.start",
        "rollout.go",
        "rollout.pause",
        "rollout.rollback",
        "rollout.abort",
        "rollout.end",
    ];
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let (_, _, audit) = b.call("GET", "/api/m/cluster/audit?action=rollout.", "");
        if want.iter().all(|a| audit.contains(a)) {
            break;
        }
        assert!(Instant::now() < deadline, "{audit}");
        std::thread::sleep(Duration::from_millis(100));
    }
    let (code, _, err) = m.stop();
    assert_eq!(code, Some(0), "{err}");
    let _ = (a.stop(), bm.stop(), cm.stop());
}

/// The consistency check on an akit (I138, fit's `admin/drift.go` tests): a running akit in step
/// with its config has only infos (a dynamic port); a setting that takes a restart, changed,
/// is a drift, its fix applied by `POST /config/reconcile` with the check's ETag (428 without,
/// 400 for a fix not offered); `config.check` listed.
#[test]
fn consistency_check() {
    let d = Dir::new("check");
    let token = "a-token-of-the-checked-akit-0123";
    let tf = d.file("check.token", token);
    let cfg = d.file(
        "check.json",
        &format!(
            r#"{{"admin": {{"address": "127.0.0.1:0", "token_file": {}}}, "limits": {{"max_conns": 100}},
                "listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
                "upstreams": [{{"name": "u", "addresses": ["127.0.0.1:1"]}}],
                "routes": [{{"handler": {{"module": "proxy", "params": {{"upstream": "u"}}}}}}]}}"#,
            js(&tf)
        ),
    );
    let r = Run::start(&["-c", s(&cfg), "-watch-certs=false"]);
    let api = r.addr("admin API on ");
    let auth = format!("Bearer {token}");
    let call = |method: &str, path: &str, extra: &[(&str, &str)], body: &str| {
        let mut h = vec![
            ("Authorization", auth.as_str()),
            ("Content-Type", "application/json"),
        ];
        h.extend_from_slice(extra);
        http(api, method, path, &h, body)
    };
    let (st, body) = admin_get(api, token, "/node");
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("\"config.check\""), "{body}");
    let (st, head, body) = call("GET", "/config/check", &[], "");
    assert_eq!(st, 200, "{body}");
    assert!(
        body.contains(r#""id":"listener:web:port""#) && body.contains(r#""summary":{"drift":0,"#),
        "{body}"
    );
    let etag = header_of(&head, "ETag");
    let (st, _, resp) = call(
        "PUT",
        "/config/limits/max_conns",
        &[("If-Match", &etag)],
        "200",
    );
    assert_eq!(st, 200, "{resp}");
    let (st, head, body) = call("GET", "/config/check", &[], "");
    assert_eq!(st, 200, "{body}");
    assert!(
        body.contains(r#""id":"setting:limits.max_conns""#)
            && body.contains(r#""patch":{"op":"replace","path":"/limits/max_conns","value":100}"#),
        "{body}"
    );
    let etag = header_of(&head, "ETag");
    let fix = r#"{"fixes": [{"id": "setting:limits.max_conns", "action": "replace"}]}"#;
    let (st, _, resp) = call("POST", "/config/reconcile", &[], fix);
    assert_eq!(st, 428, "{resp}");
    let (st, _, resp) = call(
        "POST",
        "/config/reconcile",
        &[("If-Match", &etag)],
        r#"{"fixes": [{"id": "setting:limits.max_conns", "action": "delete"}]}"#,
    );
    assert_eq!(st, 400, "{resp}");
    let (st, _, resp) = call("POST", "/config/reconcile", &[("If-Match", &etag)], fix);
    assert_eq!(st, 200, "{resp}");
    let (_, _, body) = call("GET", "/config/check", &[], "");
    assert!(
        !body.contains("setting:limits.max_conns") && body.contains(r#""summary":{"drift":0,"#),
        "{body}"
    );
    assert_eq!(
        admin_get(api, token, "/config/limits/max_conns").1.trim(),
        "100"
    );
    let _ = r.stop();
}

/// A webhook's receiver on loopback: each request's head and body, answered 200.
struct Receiver {
    addr: SocketAddr,
    got: Arc<Mutex<Vec<(String, String)>>>,
}

impl Receiver {
    fn start() -> Receiver {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let got: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
        let into = Arc::clone(&got);
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { return };
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    head.push_str(&line);
                }
                let len = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
                let mut body = vec![0; len];
                let _ = r.read_exact(&mut body);
                into.lock()
                    .unwrap()
                    .push((head, String::from_utf8_lossy(&body).into_owned()));
                let _ = s.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                );
            }
        });
        Receiver { addr, got }
    }

    /// Waits for a request whose body holds `want`: its head and body.
    fn wait(&self, want: &str) -> (String, String) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(x) = self
                .got
                .lock()
                .unwrap()
                .iter()
                .find(|(_, b)| b.contains(want))
            {
                return x.clone();
            }
            assert!(
                Instant::now() < deadline,
                "no {want:?} in {:?}",
                self.got.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Waits for `GET path` on the manager to meet `cond`: its body.
fn wait_get(b: &Browser, path: &str, what: &str, cond: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        let (st, _, body) = b.call("GET", path, "");
        if st == 200 && cond(&body) {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "{path}: waiting for {what}: {st} {body}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Alerts, webhooks and drift (I138, fit's `alerts`, `alerteval`, `webhooks` and `drift`
/// tests): the default rules; a webhook tested, signed; a member down firing a rule, sent to the
/// webhook, silenced and the silence ended; the drift against the templates fixed by a sync
/// rollout; a member's consistency check finding a drift; the alerts kept across the manager's
/// restart.
#[test]
fn alerts_and_drift() {
    let d = Dir::new("alerts");
    let h = hash("pw");
    let ta = "a-token-of-member-a-0123456789";
    let am = Run::start(&["-c", s(&token_member(&d, "a", ta)), "-watch-certs=false"]);
    let a_api = am.addr("admin API on ");
    let a_web = am.addr("listener \"web\" on ");
    let tb = "a-token-of-member-b-0123456789";
    let bm = Run::start(&["-c", s(&token_member(&d, "b", tb)), "-watch-certs=false"]);
    let b_api = bm.addr("admin API on ");
    let rx = Receiver::start();
    let mcfg = manager(&d, &h);
    let m = Run::start(&["-admin", "-c", s(&mcfg), "-watch-certs=false"]);
    let mut b = Browser::new(m.addr("console on http://"));
    assert_eq!(b.login("ada", "pw").0, 200);
    for (n, api, t) in [("a", a_api, ta), ("b", b_api, tb)] {
        add(
            &b,
            &format!(
                r#"{{"name": "{n}", "url": "http://{api}", "access": "admin_api", "token": "{t}", "labels": {{"region": "eu"}}}}"#
            ),
        );
        wait_state(&b, n, "up", "");
    }
    let (st, head, cfg) = b.call("GET", "/api/m/cluster/alerts/config", "");
    assert_eq!(st, 200, "{cfg}");
    for r in [
        "member-down",
        "drift",
        "rollout-failed",
        "certificate-expiry",
        "backends-down",
        "reloads-failing",
    ] {
        assert!(cfg.contains(&format!(r#""name":"{r}""#)), "{r}: {cfg}");
    }
    let etag = header_of(&head, "ETag");

    // A webhook, tested: signed with its secret, its URL's path never answered.
    let hook = format!(
        r#"{{"name": "hook", "url": "http://{}/h/T0K3N", "secret": "s3cret", "template": "x"}}"#,
        rx.addr
    );
    let (st, _, v) = b.call_with(
        "POST",
        "/api/m/cluster/alerts/webhooks",
        &[("If-Match", "\"a999\"")],
        &hook,
    );
    assert_eq!(st, 412, "{v}");
    let (st, _, v) = b.call_with(
        "POST",
        "/api/m/cluster/alerts/webhooks",
        &[("If-Match", &etag)],
        &hook,
    );
    assert_eq!(st, 201, "{v}");
    assert!(
        !v.contains("T0K3N") && !v.contains("s3cret") && v.contains(r#""secret_set":true"#),
        "{v}"
    );
    let (st, _, v) = b.call("POST", "/api/m/cluster/alerts/webhooks/hook/test", "");
    assert_eq!(st, 200, "{v}");
    assert!(v.contains(r#""ok":true"#), "{v}");
    let (head, body) = rx.wait("[TEST] akit cluster");
    assert!(head.starts_with("POST /h/T0K3N "), "{head}");
    let ts = header_of(&head, "X-Akit-Timestamp");
    let mut mac = akio_crypto::mac::Hmac::<akio_crypto::hash::Sha256>::new(b"s3cret");
    mac.update(format!("{ts}.").as_bytes());
    mac.update(body.as_bytes());
    let sig = format!(
        "sha256={}",
        akit_roles::linktoken::hex(mac.finalize().as_ref())
    );
    assert_eq!(header_of(&head, "X-Akit-Signature"), sig);

    // A rule of no time on a member down: firing, sent, silenced, the silence ended.
    let (st, _, v) = b.call(
        "POST",
        "/api/m/cluster/alerts/rules",
        r#"{"name": "down-now", "kind": "member_down", "webhooks": ["hook"], "repeat_ms": 0}"#,
    );
    assert_eq!(st, 201, "{v}");
    let (st, _, v) = b.call(
        "POST",
        "/api/m/cluster/alerts/rules",
        r#"{"name": "x", "kind": "nope"}"#,
    );
    assert_eq!(st, 400, "{v}");
    let _ = bm.stop();
    let alerts = wait_get(&b, "/api/m/cluster/alerts", "b down firing", |v| {
        v.contains(r#""rule":"down-now""#) && v.contains(r#""state":"firing""#)
    });
    assert!(alerts.contains(r#""node":"b""#), "{alerts}");
    rx.wait("down-now · b");
    let (st, _, v) = b.call(
        "POST",
        "/api/m/cluster/alerts/silences",
        r#"{"rule": "down-now", "node": "b", "duration_ms": 600000, "comment": "maintenance"}"#,
    );
    assert_eq!(st, 201, "{v}");
    let sid = str_member(&parse(&v), "id").to_owned();
    let (_, _, v) = b.call("GET", "/api/m/cluster/alerts", "");
    assert!(
        v.contains(&format!(r#""silenced_by":"{sid}""#)) && v.contains(r#""silenced":1"#),
        "{v}"
    );
    let (st, _, v) = b.call(
        "DELETE",
        &format!("/api/m/cluster/alerts/silences/{sid}"),
        "",
    );
    assert_eq!(st, 200, "{v}");
    let (st, _, v) = b.call("DELETE", "/api/m/cluster/alerts/webhooks/hook", "");
    assert_eq!(st, 409, "{v}");

    // Drift against a template, fixed by a sync rollout.
    let (st, _, g) = b.call(
        "POST",
        "/api/m/cluster/groups",
        r#"{"name": "eu", "members": ["a"]}"#,
    );
    assert_eq!(st, 201, "{g}");
    let tpl = r#"{"name": "base", "groups": ["eu"], "config": {"routes": [{"name": "tpl", "match": {"path": ["/tpl"]},
        "handler": {"module": "respond", "params": {"body": "templated"}}}]}}"#;
    let (st, _, v) = b.call("POST", "/api/m/cluster/templates", tpl);
    assert_eq!(st, 201, "{v}");
    let drift = wait_get(&b, "/api/m/cluster/drift", "a out of sync", |v| {
        v.contains(r#""node":"a""#) && v.contains(r#""fixable":true"#)
    });
    assert!(drift.contains(r#""managed":true"#), "{drift}");
    let (st, _, v) = b.call("POST", "/api/m/cluster/drift/fix", r#"{"nodes": ["a"]}"#);
    assert_eq!(st, 202, "{v}");
    let id = str_member(&parse(&v), "id").to_owned();
    wait_rollout(&b, &id, "its plan", state_is("planned"));
    let (st, v) = steer(&b, &id, "start", r#"{"strategy": {"mode": "all"}}"#);
    assert_eq!(st, 200, "{v}");
    wait_rollout(&b, &id, "done", state_is("done"));
    assert_eq!(get(a_web, "/tpl"), "templated");
    wait_get(&b, "/api/m/cluster/drift", "a in sync", |v| {
        v.contains(r#""fixable":false"#) && !v.contains(r#""drifted":true"#)
    });

    // A member's consistency check finding a drift fires a drift rule.
    let (st, _, v) = b.call(
        "POST",
        "/api/m/cluster/alerts/rules",
        r#"{"name": "drift-now", "kind": "drift", "source": "consistency"}"#,
    );
    assert_eq!(st, 201, "{v}");
    let (st, head, _) = b.call("GET", "/api/n/a/m/admin/config", "");
    assert_eq!(st, 200);
    let etag = header_of(&head, "ETag");
    let (st, _, v) = b.call_with(
        "PUT",
        "/api/n/a/m/admin/config/limits",
        &[("If-Match", &etag)],
        r#"{"max_conns": 77}"#,
    );
    assert_eq!(st, 200, "{v}");
    wait_get(&b, "/api/m/cluster/alerts", "a's drift firing", |v| {
        v.contains(r#""rule":"drift-now""#)
            && v.contains("its consistency check finds 1 differences")
    });

    // Kept across a restart.
    let (code, _, err) = m.stop();
    assert_eq!(code, Some(0), "{err}");
    let m = Run::start(&["-admin", "-c", s(&mcfg), "-watch-certs=false"]);
    let mut b = Browser::new(m.addr("console on http://"));
    assert_eq!(b.login("ada", "pw").0, 200);
    let (_, _, cfg) = b.call("GET", "/api/m/cluster/alerts/config", "");
    assert!(
        cfg.contains(r#""name":"down-now""#) && cfg.contains(r#""name":"hook""#),
        "{cfg}"
    );
    let (_, _, v) = b.call("GET", "/api/m/cluster/alerts", "");
    assert!(v.contains(r#""rule":"down-now""#), "{v}");
    let (_, _, audit) = b.call("GET", "/api/m/cluster/audit?action=alert.", "");
    for a in [
        "alert.webhook.add",
        "alert.webhook.test",
        "alert.rule.add",
        "alert.silence.add",
        "alert.silence.expire",
    ] {
        assert!(audit.contains(a), "{a}: {audit}");
    }
    let _ = (m.stop(), am.stop());
}
