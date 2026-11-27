//! End to end: the Admin API's access lists (I87; fit's `admin/access_test.go`): `admin.access`,
//! then the global access, refuse a client at accept; they follow every config applied (the
//! Admin API's loads, SIGHUP, the list files changing), though the rest of `admin` takes a
//! restart; an Admin API on a Unix socket is never checked, and `admin.access` with one is an
//! error.
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
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use akio_sys::process::{SIGHUP, kill};

mod support;

use support::run::{ANY, listener, printed};

/// A directory of the test's own, removed when it ends.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        Dir::under(&PathBuf::from(env!("CARGO_TARGET_TMPDIR")), name)
    }

    /// Under the system's temporary directory, for a Unix socket's path, which must be short.
    fn short(name: &str) -> Dir {
        Dir::under(
            &std::env::temp_dir(),
            &format!("{name}-{}", std::process::id()),
        )
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
    /// Where the Admin API listens, as `akit run` printed it; none on a Unix socket.
    api: Option<SocketAddr>,
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
            api: None,
        };
        let line = a.until("akit: admin API on");
        if !line.starts_with("akit: admin API on unix/") {
            a.api = Some(printed(&line, "akit: admin API on "));
        }
        a
    }

    /// Waits for a line that starts with `prefix`: the line.
    fn until(&self, prefix: &str) -> String {
        let mut seen = Vec::new();
        loop {
            let line = self
                .out
                .recv_timeout(Duration::from_secs(10))
                .unwrap_or_else(|e| panic!("waiting for {prefix:?} after {seen:?}: {e}"));
            if line.starts_with(prefix) {
                return line;
            }
            seen.push(line);
        }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }
}

/// Whether an HTTP GET of `path` on a connection of its own is answered (fit's `reaches`): one
/// closed as it is accepted is not.
fn reaches(addr: SocketAddr, path: &str) -> bool {
    let Ok(mut s) = TcpStream::connect(addr) else {
        return false;
    };
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    let _ = s.write_all(req.as_bytes());
    let mut resp = Vec::new();
    let _ = s.read_to_end(&mut resp);
    resp.starts_with(b"HTTP/1.1 200")
}

/// A load of `body` through the API: its status.
fn load(api: SocketAddr, body: &str) -> u16 {
    let mut s = TcpStream::connect(api).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "POST /load HTTP/1.1\r\nHost: {api}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    raw.split(' ').nth(1).unwrap().parse().unwrap()
}

/// fit's `withAccess`: a config with the Admin API at `api` (or `address`, as written), a
/// listener and a route, and `members` at its top level.
fn with_access(api: &str, web: &str, members: &str, admin: &str) -> String {
    format!(
        r#"{{"admin": {{"address": "{api}"{admin}}},
  "listeners": [{{"name": "web", "address": "{web}"}}],
  "routes": [{{"handler": {{"module": "respond", "params": {{"body": "hi"}}}}}}]{members}}}"#
    )
}

/// Waits until `f` holds, for at most 10 seconds.
fn eventually(what: &str, f: impl Fn() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < Duration::from_secs(10), "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn admin_access_follows_every_config() {
    let d = Dir::new("e2e-admin-access");
    // The API and the listener on port 0, and every config after keeps them there.
    let (a, w) = (ANY, ANY);
    let path = d.0.join("akit.json");
    std::fs::write(&path, with_access(a, w, "", "")).unwrap();
    let akit = Akit::run(&path, &[]);
    let api = akit.api.unwrap();
    let web = listener(api, "web", None);
    // No access anywhere: served, as before.
    assert!(reaches(api, "/versions"), "not reached without access");

    // admin.access denies the client: the load is answered, its next connections closed, and
    // the API keeps accepting.
    let deny = with_access(a, w, "", r#", "access": {"deny": ["127.0.0.1"]}"#);
    assert_eq!(load(api, &deny), 200);
    for _ in 0..3 {
        assert!(
            !reaches(api, "/versions"),
            "a denied client reached the API"
        );
    }
    // The listeners' requests are not the API's: served.
    assert!(reaches(web, "/"));

    // SIGHUP applies the file: the global access applies to the API, and admin.access's allow
    // decides first, keeping local access whatever the global access says.
    std::fs::write(
        &path,
        with_access(
            a,
            w,
            r#", "access": {"allow": ["10.0.0.0/8"]}"#,
            r#", "access": {"allow": ["127.0.0.1", "::1"]}"#,
        ),
    )
    .unwrap();
    kill(akit.pid(), SIGHUP).unwrap();
    akit.until("akit: reloaded");
    assert!(
        reaches(api, "/versions"),
        "admin.access's allow did not keep local access"
    );
    assert!(
        !reaches(web, "/"),
        "the global access did not refuse the listener's client"
    );

    // The global access alone refuses the API's clients too.
    let global = with_access(a, w, r#", "access": {"deny": ["127.0.0.0/8"]}"#, "");
    assert_eq!(load(api, &global), 200);
    assert!(
        !reaches(api, "/versions"),
        "a client the global access denies reached the API"
    );
    drop(akit);
}

#[test]
fn admin_access_files_are_watched() {
    let d = Dir::new("e2e-admin-access-file");
    let list = d.0.join("deny.txt");
    std::fs::write(&list, "# none of ours\n192.0.2.1\n").unwrap();
    let path = d.0.join("akit.json");
    let admin = format!(
        r#", "access": {{"deny_files": [{:?}]}}"#,
        list.display().to_string()
    );
    std::fs::write(&path, with_access(ANY, ANY, "", &admin)).unwrap();
    let akit = Akit::run(&path, &["-watch-interval", "50ms"]);
    let api = akit.api.unwrap();
    assert!(
        reaches(api, "/versions"),
        "a client the deny file does not hold was refused"
    );
    // The file is watched, as certificate files are: a change applies the config again.
    std::fs::write(&list, "127.0.0.1\n").unwrap();
    akit.until("akit: reloaded certificate files");
    eventually(
        "a client the changed deny file holds reached the API",
        || !reaches(api, "/versions"),
    );
}

#[test]
fn admin_access_on_a_unix_socket() {
    let d = Dir::short("akit-aa");
    let sock = d.0.join("a.sock");
    let web = ANY;
    let address = format!("unix/{}", sock.display());
    // admin.access with a Unix socket is an error.
    let bad = d.0.join("bad.json");
    std::fs::write(
        &bad,
        with_access(&address, web, "", r#", "access": {"allow": ["127.0.0.1"]}"#),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["validate", "-c"])
        .arg(&bad)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("Unix socket"), "{err}");

    // The global access does not shut out a Unix socket's peers, which have no address.
    let path = d.0.join("akit.json");
    std::fs::write(
        &path,
        with_access(
            &address,
            web,
            r#", "access": {"allow": ["10.0.0.0/8"]}"#,
            "",
        ),
    )
    .unwrap();
    let akit = Akit::run(&path, &[]);
    let mut s = std::os::unix::net::UnixStream::connect(&sock).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(b"GET /versions HTTP/1.1\r\nHost: akit\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    drop(akit);
}
