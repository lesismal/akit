//! End to end: the commands, flags and exit codes (I06): `validate`, several `-c` and
//! directories, and `run`'s graceful shutdown on SIGTERM and SIGINT.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use akio_sys::process::{SIGINT, SIGTERM, kill};

const LISTENER: &str = r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}]}"#;
const ROUTE: &str = r#"{
  "routes": [{"listeners": ["web"], "match": {"path": ["/hello"]},
              "handler": {"module": "respond", "params": {"body": "hello\n"}}}]
}"#;

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("cli")
        .join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn write(d: &Path, name: &str, text: &str) -> PathBuf {
    let p = d.join(name);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, text).unwrap();
    p
}

fn akit(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(args)
        .output()
        .unwrap()
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn validate_merges_files_and_reports_errors() {
    let d = dir("validate");
    let a = write(&d, "a.json", LISTENER);
    let b = write(&d, "b.json", ROUTE);
    let out = akit(&["validate", "-c", s(&a), "-c", s(&b)]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("{}, {}: valid\n", a.display(), b.display())
    );

    // The route alone: its listener is in no file.
    let out = akit(&["validate", "-c", s(&b)]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(
        err.contains(&format!(
            "{}:2:14: routes[0].listeners[0]: there is no listener named \"web\"",
            b.display()
        )),
        "{err}"
    );
    assert!(out.stdout.is_empty());

    // A directory is its files, merged.
    let out = akit(&["validate", &format!("-c={}", d.display())]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // akit.json by default, which is not here.
    let out = Command::new(env!("CARGO_BIN_EXE_akit"))
        .arg("validate")
        .current_dir(&d)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .starts_with("akit.json: ")
    );

    // fit's own .conf format (I23), its problems at their line and column.
    let conf = write(
        &d,
        "fit.conf",
        "listener web :8080\nroute { handler respond { status 204 } }\n",
    );
    let out = akit(&["validate", "-c", s(&conf)]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bad = write(&d, "bad.conf", "listener web {\n\taddress nowhere\n}\n");
    let out = akit(&["validate", "-c", s(&bad)]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .starts_with(&format!("{}:2:10: listeners[0].address: ", bad.display()))
    );
}

/// I79: an old name of a field renamed in schema version 1 is read, and `validate` says so at
/// its place in the file, as fit's does; the config is still valid.
#[test]
fn validate_warns_of_renamed_fields() {
    let d = dir("renamed");
    let c = write(
        &d,
        "fit.conf",
        "listener web 127.0.0.1:0\nroute {\n\tmiddleware compress { min_length 10 }\n\thandler respond\n}\n",
    );
    let out = akit(&["validate", "-c", s(&c)]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("{}: valid\n", c.display())
    );
    let err = String::from_utf8(out.stderr).unwrap();
    assert_eq!(
        err,
        format!(
            "{}:3:24: routes[0].middleware[0].params.min_length: deprecated: renamed \"min_bytes\", \
             which takes the same value; the old name goes in config version 2\n",
            c.display()
        )
    );
}

/// I80b (the threat model's L3, L4): a secret file open to others and a credential sent in the
/// clear off loopback are warned of; the config is still valid, as fit runs it.
#[cfg(unix)]
#[test]
fn validate_warns_of_exposed_secrets() {
    use std::os::unix::fs::PermissionsExt as _;
    let d = dir("exposed");
    let token = write(&d, "token", "s3cret\n");
    fs::set_permissions(&token, fs::Permissions::from_mode(0o644)).unwrap();
    let c = write(
        &d,
        "fit.json",
        &format!(
            r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
  "admin": {{"token_file": "{}"}},
  "acme": {{"directory": "http://ca.example.com/directory"}}}}"#,
            token.display()
        ),
    );
    let out = akit(&["validate", "-c", s(&c)]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let err = String::from_utf8(out.stderr).unwrap();
    let lines: Vec<&str> = err.lines().collect();
    assert_eq!(lines.len(), 2, "{err}");
    assert!(
        lines[0].starts_with(&format!(
            "{}: admin.token_file: {} holds a secret and is open to its group or others (mode 0644)",
            c.display(),
            token.display()
        )),
        "{err}"
    );
    assert!(
        lines[1]
            .contains("acme.directory http://ca.example.com/directory is plain HTTP off loopback"),
        "{err}"
    );
    // Readable by its owner alone, it is not.
    fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
    let out = akit(&["validate", "-c", s(&c)]);
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(!err.contains("token_file"), "{err}");
}

#[test]
fn usage_errors_exit_2() {
    for args in [
        &[][..],
        &["nope"],
        &["run", "-x"],
        &["run", "-c"],
        &["run", "extra"],
        &["run", "-shutdown-timeout", "soon"],
        &["run", "-watch-interval", "soon"],
        &["run", "-watch-interval", "0s"],
        &["run", "-timeout", "1m"],
        &["run", "-admin", "-timeout", "soon"],
        &["run", "-admin", "-timeout", "0s"],
        &["validate", "-shutdown-timeout=1s"],
        &["version", "-x"],
    ] {
        assert_eq!(akit(args).status.code(), Some(2), "{args:?}");
    }
    assert_eq!(akit(&["help"]).status.code(), Some(0));
    let v = akit(&["version"]);
    assert_eq!(v.status.code(), Some(0));
    assert!(String::from_utf8(v.stdout).unwrap().starts_with("akit "));
}

/// The running binary, killed if a test fails before it exits.
struct Running(Option<Child>, SocketAddr);

impl Running {
    fn start(args: &[&str]) -> Running {
        let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        assert!(line.starts_with("akit: listener \"web\" on "), "{line:?}");
        let addr = line.split(' ').nth(4).unwrap().parse().unwrap();
        // Put the rest of stdout back for `stop` to read.
        child.stdout = None;
        let rest = std::thread::spawn(move || {
            let mut s = String::new();
            out.read_to_string(&mut s).unwrap();
            s
        });
        STDOUT.with(|r| *r.borrow_mut() = Some(rest));
        Running(Some(child), addr)
    }

    /// Sends `sig` and returns the exit code, stdout after the listener line, and stderr.
    fn stop(mut self, sig: i32) -> (Option<i32>, String, String) {
        let mut child = self.0.take().unwrap();
        kill(child.id(), sig).unwrap();
        let status = child.wait().unwrap();
        let mut err = String::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut err)
            .unwrap();
        let out = STDOUT.with(|r| r.borrow_mut().take().unwrap().join().unwrap());
        (status.code(), out, err)
    }
}

thread_local! {
    static STDOUT: std::cell::RefCell<Option<std::thread::JoinHandle<String>>> =
        const { std::cell::RefCell::new(None) };
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(c) = &mut self.0 {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn get(addr: SocketAddr, path: &str) -> String {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    resp
}

#[test]
fn run_merges_files_and_stops_on_sigterm() {
    let d = dir("run");
    let a = write(&d, "a.json", LISTENER);
    let b = write(&d, "b.json", ROUTE);
    let r = Running::start(&["run", "-c", s(&a), "-c", s(&b), "-shutdown-timeout", "5s"]);
    assert!(get(r.1, "/hello").ends_with("\r\n\r\nhello\n"));
    // An idle keep-alive connection does not hold the shutdown up.
    let mut idle = TcpStream::connect(r.1).unwrap();
    idle.write_all(b"GET /hello HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    idle.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut buf = [0u8; 256];
    assert!(idle.read(&mut buf).unwrap() > 0);
    let start = std::time::Instant::now();
    let (code, out, err) = r.stop(SIGTERM);
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(out, "akit: shutting down\nakit: stopped\n");
    assert!(start.elapsed() < Duration::from_secs(4));
    // The idle connection was closed.
    assert_eq!(idle.read(&mut buf).unwrap(), 0);
}

#[test]
fn run_stops_on_sigint_from_a_directory() {
    let d = dir("run-dir");
    write(&d, "a.json", LISTENER);
    write(&d, "routes/b.json", ROUTE);
    let r = Running::start(&["run", "-c", s(&d)]);
    assert!(get(r.1, "/hello").ends_with("hello\n"));
    let (code, out, _) = r.stop(SIGINT);
    assert_eq!(code, Some(0));
    assert_eq!(out, "akit: shutting down\nakit: stopped\n");
}

/// `akit hash-password` (I89), as fit's: the first line of standard input hashed, at `-cost`; an
/// empty or too long password, or an argument, refused.
#[test]
fn hash_password_from_standard_input() {
    let run = |args: &[&str], input: &[u8]| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
            .arg("hash-password")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        child.wait_with_output().unwrap()
    };
    let out = run(&["-cost", "4"], b"correct horse\nnext line\n");
    assert!(out.status.success(), "{out:?}");
    let hash = String::from_utf8(out.stdout).unwrap();
    let hash = hash.trim_end();
    assert!(hash.starts_with("$2b$04$"), "{hash}");
    assert!(
        akio_crypto::bcrypt::Hash::parse(hash)
            .unwrap()
            .verify(b"correct horse")
    );
    // The default cost, 10.
    let out = run(&[], b"pw\n");
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .starts_with("$2b$10$")
    );
    for (args, input, code, want) in [
        (&[][..], &b"\n"[..], 1, "the password is empty"),
        (&[], &[b'x'; 80][..], 1, "at most 72 bytes"),
        (&["-cost", "x"], b"pw\n", 2, "invalid value"),
        (&["pw"], b"", 2, "unexpected argument"),
    ] {
        let out = run(args, input);
        assert_eq!(out.status.code(), Some(code), "{args:?}");
        let err = String::from_utf8(out.stderr).unwrap();
        assert!(err.contains(want), "{args:?}: {err}");
    }
}

/// `akit store import` and `export` (I93), as fit's `fit store`: a config sealed into the store
/// `admin.store` names, printed back as it was sealed; a store fit wrote is exported; `admin` in what is imported, a wrong password and no
/// store refused; `akit validate` checks `admin.store` (`akit run` with it: `tests/store.rs`).
#[test]
fn store_export_and_import() {
    let d = dir("store");
    let store = d.join("akit.store");
    let boot = |pw: &str| {
        format!(
            r#"{{"admin": {{"address": "127.0.0.1:0", "store": {{"path": {:?}, "password_file": {:?}}}}}}}"#,
            s(&store),
            s(&d.join(format!("{pw}.pw")))
        )
    };
    fs::write(d.join("pw.pw"), "  secret\n").unwrap();
    fs::write(d.join("fit-v2.pw"), "fit-v2").unwrap();
    let b = write(&d, "boot.json", &boot("pw"));
    let conf = write(&d, "site.json", LISTENER);
    let route = write(&d, "route.json", ROUTE);

    let out = akit(&["validate", "-c", s(&b)]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let out = akit(&["store", "export", "-c", s(&b)]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("there is no store at"),
        "{out:?}"
    );
    let out = akit(&["store", "import", "-c", s(&b), s(&conf), s(&route)]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("sealed into"));
    let out = akit(&["store", "export", "-c", s(&b)]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let exported = String::from_utf8(out.stdout).unwrap();
    assert!(
        exported.contains("\"listeners\"") && exported.contains("/hello"),
        "{exported}"
    );
    assert!(!exported.contains("admin"), "{exported}");
    // The export imports again.
    let again = write(&d, "exported.json", &exported);
    let out = akit(&["store", "import", "-c", s(&b), s(&again)]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(
        String::from_utf8(akit(&["store", "export", "-c", s(&b)]).stdout).unwrap(),
        exported
    );

    // Refused: admin imported, a config that does not check, a wrong password, no store.
    let with_admin = write(&d, "admin.json", r#"{"admin": {}}"#);
    let out = akit(&["store", "import", "-c", s(&b), s(&with_admin)]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("admin stays in the bootstrap file"));
    let bad = write(
        &d,
        "bad.json",
        r#"{"routes": [{"listeners": ["nowhere"]}]}"#,
    );
    let out = akit(&["store", "import", "-c", s(&b), s(&bad)]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    fs::write(d.join("pw.pw"), "wrong").unwrap();
    let out = akit(&["store", "export", "-c", s(&b)]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("does not open with the password"),
        "{out:?}"
    );
    let out = akit(&["store", "export", "-c", s(&conf)]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("names no store"));
    for args in [
        &["store"][..],
        &["store", "x"],
        &["store", "export", "-c", s(&b), "extra"],
    ] {
        assert_eq!(akit(args).status.code(), Some(2), "{args:?}");
    }
    assert_eq!(
        akit(&["store", "import", "-c", s(&b)]).status.code(),
        Some(2)
    );

    // A store fit wrote (fit's own code, akit-admin's fixtures): exported as fit sealed it.
    let fit = Path::new(env!("CARGO_MANIFEST_DIR")).join("../akit-admin/tests/fitstore/v2.store");
    fs::copy(&fit, &store).unwrap();
    let b2 = write(&d, "boot-fit.json", &boot("fit-v2"));
    let out = akit(&["store", "export", "-c", s(&b2)]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .starts_with(r#"{"listeners": [{"name": "web", "address": ":8080"}]"#)
    );

    // Two ways to the password.
    let two = write(
        &d,
        "two.json",
        r#"{"admin": {"store": {"path": "x.store", "password": "a", "password_env": "B"}}}"#,
    );
    let out = akit(&["validate", "-c", s(&two)]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("set exactly one of password"));
    let _ = fs::remove_dir_all(&d);
}
