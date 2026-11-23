//! End to end: `akit run` with fit's `hello` example answers `/hello` over HTTP/1.1 and h2c
//! (I05). The example is fit-examples' `hello/fit.conf` (tests/compat/fixtures) written as
//! JSON, the one format akit reads until I23; its listener's address is 127.0.0.1:0 rather than
//! :8080, and akit prints the port it got.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use akio::Engine;
use akio_http::client::ClientRequest;
use akio_http::h2client::{H2Client, H2Options};

const HELLO: &str = r#"{
  // fit-examples hello/fit.conf
  "listeners": [{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}],
  "routes": [
    {"name": "hello", "match": {"path": ["/hello"]},
     "handler": {"module": "respond", "params": {"body": "hello from fit\n"}}},
    {"name": "not-found", "priority": -1,
     "handler": {"module": "respond", "params": {"status": 404, "body": "not found\n"}}}
  ]
}"#;

/// The running binary, killed when dropped.
struct Akit(Child);

impl Drop for Akit {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn config(name: &str, json: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&path, json).unwrap();
    path
}

/// Starts `akit run -c <config>` and reads where its listener listens.
fn start(config: &PathBuf) -> (Akit, SocketAddr) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
        .arg("run")
        .arg("-c")
        .arg(config)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let akit = Akit(child);
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line).unwrap();
    // akit: listener "web" on 127.0.0.1:PORT (http/1.1, h2c)
    assert!(line.starts_with("akit: listener \"web\" on "), "{line:?}");
    assert!(line.trim_end().ends_with("(http/1.1, h2c)"), "{line:?}");
    let addr = line.split(' ').nth(4).unwrap().parse().unwrap();
    (akit, addr)
}

fn get(addr: SocketAddr, path: &str) -> (String, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    let (head, body) = resp.split_once("\r\n\r\n").unwrap();
    (head.lines().next().unwrap().to_owned(), body.to_owned())
}

#[test]
fn hello_over_http1_and_h2c() {
    let (_akit, addr) = start(&config("hello.json", HELLO));
    assert_eq!(
        get(addr, "/hello"),
        ("HTTP/1.1 200 OK".into(), "hello from fit\n".into())
    );
    assert_eq!(
        get(addr, "/other"),
        ("HTTP/1.1 404 Not Found".into(), "not found\n".into())
    );

    let client = Engine::builder()
        .client_threads(1)
        .config({
            let mut v = akio::Config::default();
            v.shutdown_timeout = Duration::from_millis(200);
            v
        })
        .build()
        .unwrap();
    client.start().unwrap();
    let c = H2Client::dial_blocking(&client, addr, None, H2Options::default()).unwrap();
    for (path, status, body) in [
        ("/hello", 200, "hello from fit\n"),
        ("/other", 404, "not found\n"),
    ] {
        let r = c
            .send(
                ClientRequest::get(&format!("http://{addr}{path}")).timeout(Duration::from_secs(5)),
            )
            .wait()
            .unwrap();
        assert_eq!(
            (r.status, String::from_utf8(r.body).unwrap().as_str()),
            (status, body)
        );
    }
    drop(c);
    client.shutdown();
    client.wait();
}

#[test]
fn a_bad_config_exits_1_with_its_errors() {
    let path = config(
        "bad.json",
        r#"{"listeners": [{"name": "web", "address": "nope"}]}"#,
    );
    let out = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["run", "-c"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(err.starts_with(&format!("{}:1:", path.display())), "{err}");
    assert!(err.contains("listeners[0].address"), "{err}");
}

#[test]
fn bad_flags_exit_2() {
    for args in [&["run", "-x"][..], &["run", "-c"], &["run", "extra"]] {
        let out = Command::new(env!("CARGO_BIN_EXE_akit"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
    }
}
