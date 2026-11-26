//! End to end: `akit run` with the encrypted store (I93b, I93c), fit's `cli/store_test.go`
//! (`TestServeStore`, `TestStoreCommand`, `TestServeStoreMergeUnwritable`,
//! `TestServeStoreFailedFirstStart`) and `cli/adminflag_test.go` (`TestServeAdminFlag`): the store
//! seeded from the bootstrap file, written on each change, read again on SIGHUP and merged onto
//! the files at a restart, `admin` never in it; `akit store` with the store's password changed;
//! `run -admin` with its temporary password, the store beside the config and its key file, and
//! `-c` naming the `.store` file. With `AKIT_FIT_BIN` naming fit's binary, fit and akit take turns
//! running from the same store and its key file, each opening what the other kept: the config
//! changed in the console and the console's password changed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use akio_sys::process::{SIGHUP, SIGTERM, kill};

/// A directory of the test's own, removed when it ends.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("store")
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

fn akit(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_akit"));
    c.args(args).env_remove("AKIT_STORE_PASSWORD");
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// `akit store export -c <cfg>`'s output: the config the store holds.
fn export(cfg: &Path, env: &[(&str, &str)]) -> String {
    let o = akit(&["store", "export", "-c", s(cfg)], env);
    assert_eq!(o.status.code(), Some(0), "export: {}", stderr(&o));
    String::from_utf8(o.stdout).unwrap()
}

/// `akit run`, its standard output and error kept as they come.
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

fn post(addr: SocketAddr, path: &str, body: &str) -> (u16, String) {
    let (st, _, b) = http(
        addr,
        "POST",
        path,
        &[("Content-Type", "application/json")],
        body,
    );
    (st, b)
}

/// A browser on the console's pages: its session's cookie and CSRF token; `name` is `akit`, or
/// `fit` for fit's console (its cookie and CSRF header named after it).
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

const ROUTES_ONE: &str = r#""listeners": [{"name": "web", "address": "127.0.0.1:0"}],
"routes": [{"handler": {"module": "respond", "params": {"body": "one"}}}]"#;

/// fit's `TestServeStore`: akit runs from a store, seeded from the bootstrap file, written on each
/// change from the Admin API, read again on SIGHUP and after a restart, with admin never in it.
#[test]
fn serve_store() {
    let d = Dir::new("serve");
    let path = d.0.join("akit.store");
    let boot_admin = format!(
        r#""admin": {{"address": "127.0.0.1:0", "store": {{"path": {}, "password": "s3cret"}}}}"#,
        js(&path)
    );
    let cfg = d.file("akit.json", &format!("{{{boot_admin},\n{ROUTES_ONE}}}"));
    let r = Run::start(&["-c", s(&cfg), "-watch-certs=false"]);
    let admin = r.addr("akit: admin API on ");
    // The seed is said once the start has succeeded.
    r.wait_for(&format!("is kept in the store {}", s(&path)));
    let data = export(&cfg, &[]);
    assert!(
        data.contains("\"one\"") && !data.contains("admin") && !data.contains("s3cret"),
        "the seed: {data}"
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    // GET /config shows no admin; a config with admin is refused; one without is applied and
    // saved, as it was sent.
    assert!(!get(admin, "/config").contains("admin"));
    let two = r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}], "routes": [{"handler": {"module": "respond", "params": {"body": "two"}}}]}"#;
    let (st, body) = post(admin, "/load", &format!("{{{boot_admin},{}", &two[1..]));
    assert_eq!(st, 400, "{body}");
    assert!(
        body.contains("admin is set in the config file akit started with"),
        "{body}"
    );
    let (st, body) = post(admin, "/load", two);
    assert_eq!(st, 200, "{body}");
    assert_eq!(export(&cfg, &[]), two);

    // SIGHUP reads the store again, as akit store import wrote it.
    let three = d.file("three.json", &two.replace("two", "three"));
    let o = akit(&["store", "import", "-c", s(&cfg), s(&three)], &[]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    r.signal(SIGHUP);
    r.wait_for(&format!("akit: reloaded the store {}", s(&path)));
    assert!(get(admin, "/config").contains("\"three\""));
    let (code, _, err) = r.stop();
    assert_eq!(code, Some(0), "{err}");

    // After a restart, the store is merged onto the bootstrap file, and wins over its routes:
    // lists are replaced whole.
    let r = Run::start(&["-c", s(&cfg), "-watch-certs=false"]);
    let admin = r.addr("akit: admin API on ");
    let web = r.addr("akit: listener \"web\" on ");
    assert!(get(admin, "/config").contains("\"three\""));
    assert_eq!(get(web, "/"), "three");
    assert_eq!(r.stop().0, Some(0));

    // A wrong password, or a changed byte, stops akit from starting; -watch is refused.
    let wrong = d.file(
        "wrong.json",
        &format!(
            r#"{{"admin": {{"store": {{"path": {}, "password": "wrong"}}}}}}"#,
            js(&path)
        ),
    );
    let (code, _, err) = Run::start(&["-c", s(&wrong)]).exit();
    assert_eq!(code, Some(1));
    assert!(err.contains("does not open with the password"), "{err}");
    let mut bytes = std::fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    let changed = d.0.join("changed.store");
    std::fs::write(&changed, &bytes).unwrap();
    let cfg_changed = d.file(
        "changed.json",
        &format!(
            r#"{{"admin": {{"store": {{"path": {}, "password": "s3cret"}}}}}}"#,
            js(&changed)
        ),
    );
    let (code, _, err) = Run::start(&["-c", s(&cfg_changed)]).exit();
    assert_eq!(code, Some(1));
    assert!(err.contains("does not open with the password"), "{err}");
    let only_admin = d.file("only-admin.json", &format!("{{{boot_admin}}}"));
    let (code, _, err) = Run::start(&["-c", s(&only_admin), "-watch"]).exit();
    assert_eq!(code, Some(1));
    assert!(err.contains("-watch watches config files"), "{err}");
}

use std::os::unix::fs::PermissionsExt;

/// fit's `TestStoreCommand`: `akit store` with the password in an environment variable; refused
/// imports write nothing; the password changed by exporting, changing it and importing.
#[test]
fn store_command() {
    let d = Dir::new("command");
    let path = d.0.join("akit.store");
    let cfg = d.file(
        "akit.json",
        &format!(
            r#"{{"admin": {{"store": {{"path": {}, "password_env": "AKIT_TEST_STORE"}}}}}}"#,
            js(&path)
        ),
    );
    let env = [("AKIT_TEST_STORE", "pw")];
    let o = akit(&["store", "export", "-c", s(&cfg)], &env);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("there is no store"), "{}", stderr(&o));
    for (doc, want) in [
        (
            r#"{"admin": {}, "listeners": [{"name": "web", "address": ":0"}], "routes": [{"handler": {"module": "respond"}}]}"#,
            "admin stays in the bootstrap file",
        ),
        (
            r#"{"listeners": [{"name": "web", "address": ":0"}], "routes": [{"handler": {"module": "nope"}}]}"#,
            "nope",
        ),
    ] {
        let bad = d.file("bad.json", doc);
        let o = akit(&["store", "import", "-c", s(&cfg), s(&bad)], &env);
        assert_eq!(o.status.code(), Some(1), "{doc}");
        assert!(stderr(&o).contains(want), "{doc}: {}", stderr(&o));
    }
    assert!(!path.exists(), "a refused import wrote the store");

    // The password changes: export, change it, import.
    let good = d.file(
        "good.json",
        r#"{"listeners": [{"name": "web", "address": ":0"}], "routes": [{"handler": {"module": "respond"}}]}"#,
    );
    let o = akit(&["store", "import", "-c", s(&cfg), s(&good)], &env);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let before = export(&cfg, &env);
    let new = [("AKIT_TEST_STORE", "new")];
    let o = akit(&["store", "export", "-c", s(&cfg)], &new);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("does not open"), "{}", stderr(&o));
    std::fs::write(&good, &before).unwrap();
    let o = akit(&["store", "import", "-c", s(&cfg), s(&good)], &new);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(export(&cfg, &new), before);
    assert_eq!(akit(&["store"], &[]).status.code(), Some(2));
}

/// Whether this process may write where its owner may not: root may.
fn root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
}

/// fit's `TestServeStoreMergeUnwritable`: a store merged onto the bootstrap file at a restart is
/// written back before akit serves; a store whose directory cannot be written stops akit from
/// starting.
#[test]
fn serve_store_merge_unwritable() {
    if root() {
        eprintln!("skipped: root writes to any directory");
        return;
    }
    let d = Dir::new("unwritable");
    let st_dir = d.0.join("st");
    std::fs::create_dir(&st_dir).unwrap();
    let path = st_dir.join("akit.store");
    let mut st = akit_admin::store::Store::create(&path, "s3cret").unwrap();
    st.write(
        br#"{"routes": [{"handler": {"module": "respond", "params": {"body": "stored"}}}]}"#
            .to_vec(),
    )
    .unwrap();
    let cfg = d.file(
        "akit.json",
        &format!(
            r#"{{"admin": {{"address": "127.0.0.1:0", "store": {{"path": {}, "password": "s3cret"}}}},
"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
"routes": [{{"handler": {{"module": "respond", "params": {{"body": "file"}}}}}}]}}"#,
            js(&path)
        ),
    );
    std::fs::set_permissions(&st_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let (code, out, err) = Run::start(&["-c", s(&cfg), "-watch-certs=false"]).exit();
    std::fs::set_permissions(&st_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(code, Some(1), "{out} {err}");
    assert!(err.contains("store: "), "{err}");
    assert!(!out.contains("merged onto"), "{out}");
}

/// fit's `TestServeStoreFailedFirstStart`: a first start that fails after the store is seeded,
/// here on a port another process holds, leaves no store behind: the fixed files are what the
/// next start reads.
#[test]
fn serve_store_failed_first_start() {
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let d = Dir::new("failed-first");
    let path = d.0.join("akit.store");
    let cfg = d.file(
        "akit.json",
        &format!(
            r#"{{"admin": {{"address": "127.0.0.1:0", "store": {{"path": {}, "password": "s3cret"}}}},
"listeners": [{{"name": "web", "address": "{}"}}],
"routes": [{{"handler": {{"module": "respond", "params": {{"body": "one"}}}}}}]}}"#,
            js(&path),
            held.local_addr().unwrap()
        ),
    );
    let (code, out, err) = Run::start(&["-c", s(&cfg), "-watch-certs=false"]).exit();
    assert_eq!(code, Some(1), "{out} {err}");
    assert!(!out.contains("is kept in the store"), "{out}");
    assert!(!path.exists(), "the store is left behind");
    drop(held);
}

/// fit's `TestServeAdminFlag`: `akit run -admin`, the console with the temporary password admin,
/// the store beside the config with its key file, the store merged onto the config file at a
/// restart, and `-c` with the `.store` file.
#[test]
fn serve_admin_flag() {
    let d = Dir::new("admin-flag");
    let cfg = d.0.join("site.json");
    let path = d.0.join("site.store");
    let key_path = d.0.join("site.store.key");
    let addrs = r#""admin": {"address": "127.0.0.1:0", "console": {"address": "127.0.0.1:0"}}"#;
    std::fs::write(
        &cfg,
        format!(
            r#"{{{addrs},
"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
"routes": [{{"handler": {{"module": "respond", "params": {{"body": "file"}}}}}}]}}"#
        ),
    )
    .unwrap();

    let (code, _, err) = Run::start(&["-c", s(&cfg), "-timeout", "1m"]).exit();
    assert_eq!(code, Some(2));
    assert!(err.contains("needs -admin"), "{err}");
    let (code, _, err) = Run::start(&["-admin", "-c", s(&cfg), "-timeout", "0s"]).exit();
    assert_eq!(code, Some(2));
    assert_eq!(err, "akit run: -timeout must be more than 0\n");

    let r = Run::start(&["-admin", "-c", s(&cfg), "-watch-certs=false"]);
    let console = r.addr("akit: console on http://");
    let admin = r.addr("akit: admin API on ");
    assert!(
        r.err()
            .contains(r#"the console user "admin" has the password "admin""#),
        "{}",
        r.err()
    );
    let meta = std::fs::metadata(&key_path).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    let key = std::fs::read_to_string(&key_path).unwrap();
    assert_eq!(key.trim().len(), 52);
    assert!(!r.out().contains(key.trim()) && !r.err().contains(key.trim()));
    assert!(path.exists());
    let (st, body) = post(admin, "/load", "{}");
    assert_eq!(st, 403, "{body}");
    assert!(body.contains("temporary password"), "{body}");
    let mut b = Browser::new(console);
    let (st, body) = b.login("admin", "admin");
    assert_eq!(st, 200, "{body}");
    assert!(body.contains(r#""must_change_password":true"#), "{body}");
    assert_eq!(b.call("GET", "/api/m/admin/config", "").0, 403);
    let (st, _, body) = b.call(
        "POST",
        "/api/m/admin/account/password",
        r#"{"current_password": "admin", "password": "n3w"}"#,
    );
    assert_eq!(st, 200, "{body}");
    let (st, _, body) = b.call("GET", "/api/m/admin/config", "");
    assert!(st == 200 && body.contains("\"file\""), "{st} {body}");
    assert!(get(admin, "/config").contains("\"file\""));
    // A change from the console is kept in the store.
    let two = r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}], "routes": [{"handler": {"module": "respond", "params": {"body": "console"}}}]}"#;
    let (st, _, body) = b.call("PUT", "/api/m/admin/config", two);
    assert_eq!(st, 200, "{body}");
    assert!(export(&path, &[]).contains("\"console\""));
    let (code, _, err) = r.stop();
    assert_eq!(code, Some(0), "{err}");

    // The file gains a member the store lacks; at a restart the store is merged onto it: its
    // routes win, the file's limits stay. The changed password is kept, so none is temporary.
    std::fs::write(
        &cfg,
        format!(
            r#"{{{addrs},
"limits": {{"max_conns": 1234}},
"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
"routes": [{{"handler": {{"module": "respond", "params": {{"body": "file"}}}}}}]}}"#
        ),
    )
    .unwrap();
    let r = Run::start(&["-admin", "-c", s(&cfg), "-watch-certs=false"]);
    let console = r.addr("akit: console on http://");
    let web = r.addr("akit: listener \"web\" on ");
    // The merge is said once the start has succeeded, after the console's line.
    r.wait_for("merged onto");
    assert!(
        !r.err().contains("temporary") && !r.err().contains(r#"has the password "admin""#),
        "{}",
        r.err()
    );
    assert_eq!(get(web, "/"), "console");
    let held = export(&path, &[]);
    assert!(
        held.contains("1234") && held.contains("\"console\""),
        "{held}"
    );
    let mut b = Browser::new(console);
    assert_eq!(b.login("admin", "admin").0, 401);
    let (st, body) = b.login("admin", "n3w");
    assert!(st == 200 && !body.contains("must_change"), "{st} {body}");
    assert_eq!(r.stop().0, Some(0));

    // -c with the .store alone: the config is the store's, opened with the key file beside it.
    let r = Run::start(&["-c", s(&path), "-watch-certs=false"]);
    let web = r.addr("akit: listener \"web\" on ");
    assert_eq!(get(web, "/"), "console");
    assert_eq!(r.stop().0, Some(0));
    // Elsewhere, without the key file, $AKIT_STORE_PASSWORD opens it.
    let other = Dir::new("admin-flag-moved");
    let moved = other.0.join("moved.store");
    std::fs::copy(&path, &moved).unwrap();
    let o = akit(&["store", "export", "-c", s(&moved)], &[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("no key file"), "{}", stderr(&o));
    let env = [("AKIT_STORE_PASSWORD", key.trim())];
    assert!(export(&moved, &env).contains("\"console\""));
    let o = akit(&["run", "-c", s(&path), "-c", s(&moved)], &[]);
    assert!(matches!(o.status.code(), Some(1 | 2)), "{o:?}");
    assert!(stderr(&o).contains("-c names two stores"), "{}", stderr(&o));
}

/// fit and akit run in turn from one store beside the config, with its key file (`run -admin`):
/// what each kept, a config changed in the console and a password changed, the other runs and
/// logs in with. fit's binary is `AKIT_FIT_BIN` (`go build -o <bin> ./cmd/fit` in fit at the
/// compatibility gate's pin); without it, nothing is run.
#[test]
fn fit_and_akit_share_a_store() {
    let Some(fit) = std::env::var_os("AKIT_FIT_BIN").map(PathBuf::from) else {
        eprintln!("skipped: AKIT_FIT_BIN is not set");
        return;
    };
    let d = Dir::new("fit-and-akit");
    let cfg = d.file(
        "site.json",
        r#"{"admin": {"address": "127.0.0.1:0", "console": {"address": "127.0.0.1:0"}},
"listeners": [{"name": "web", "address": "127.0.0.1:0"}],
"routes": [{"handler": {"module": "respond", "params": {"body": "file"}}}]}"#,
    );
    let akit_bin = PathBuf::from(env!("CARGO_BIN_EXE_akit"));
    let body = |b: &str| {
        format!(
            r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}], "routes": [{{"handler": {{"module": "respond", "params": {{"body": "{b}"}}}}}}]}}"#
        )
    };
    // Each in turn: logs in with `old`, changes it to `new` and the config to serve `name`.
    let mut old = "admin".to_owned();
    for (bin, name, new, before) in [
        (&akit_bin, "akit", "akit-pw", "file"),
        (&fit, "fit", "fit-pw", "akit"),
        (&akit_bin, "akit", "akit-pw2", "fit"),
    ] {
        let r = Run::spawn(bin, &["-admin", "-c", s(&cfg), "-watch-certs=false"]);
        let console = r.addr("console on http://");
        let web = r.addr("listener \"web\" on ");
        assert_eq!(get(web, "/"), before, "{name} serves what the store keeps");
        let mut b = Browser::of(name, console);
        let (st, resp) = b.login("admin", &old);
        assert_eq!(st, 200, "{name} logs in with {old}: {resp}");
        let (st, _, resp) = b.call(
            "POST",
            "/api/m/admin/account/password",
            &format!(r#"{{"current_password": "{old}", "password": "{new}"}}"#),
        );
        assert_eq!(st, 200, "{name}: {resp}");
        let (st, _, resp) = b.call("PUT", "/api/m/admin/config", &body(name));
        assert_eq!(st, 200, "{name}: {resp}");
        assert_eq!(get(web, "/"), name);
        let (code, out, err) = r.stop();
        assert_eq!(code, Some(0), "{name}: {out} {err}");
        old = new.to_owned();
    }
    // Each exports what the other kept.
    let store = d.0.join("site.store");
    let o = Command::new(&fit)
        .args(["store", "export", "-c", s(&store)])
        .env_remove("FIT_STORE_PASSWORD")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(String::from_utf8_lossy(&o.stdout).contains("\"akit\""));
    assert!(export(&store, &[]).contains("\"akit\""));
}
