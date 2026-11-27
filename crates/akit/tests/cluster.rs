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
    assert_eq!(st, 502, "{body}");
    assert!(body.contains("\"unsupported\""), "{body}");

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
