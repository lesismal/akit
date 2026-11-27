//! End to end: `akit run`'s graceful shutdown ends once its requests are done. The upstream
//! connections its pools keep idle do not hold it open until `-shutdown-timeout`: an h2c one is
//! closed (`GOAWAY`) when the shutdown begins, as akio's HTTP/1 client closes its own, and akit
//! exits at once with status 0, as fit does (its upstream connections never hold its listeners'
//! shutdown). The upstream is akio-http's server, on the backend `AKIO_TEST_BACKEND` names; akit
//! runs on the one `AKIO_DEFAULT_BACKEND` names.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, Listener, UringConfig};
use akio_http::{Context, Version};
use akio_sys::process::{SIGTERM, kill};

/// How long akit's shutdown may wait for requests in flight (`-shutdown-timeout`).
const GRACE: Duration = Duration::from_secs(20);

fn backend() -> Backend {
    match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    }
}

/// An upstream on akio-http's server (HTTP/1.1, and h2c with prior knowledge): it answers with
/// the request's version.
fn upstream() -> Engine {
    let e = Engine::builder()
        .listen(
            Listener::tcp("127.0.0.1:0".parse().unwrap())
                .backend(backend())
                .serve(akio_http::serve(|ctx: &mut Context| {
                    let v = if ctx.req().version == Version::Http2 {
                        "h2"
                    } else {
                        "h1"
                    };
                    ctx.text(200, v);
                })),
        )
        .build()
        .unwrap();
    e.start().unwrap();
    e
}

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn lines(r: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(r).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

fn until(rx: &mpsc::Receiver<String>, within: Duration, pred: impl Fn(&str) -> bool) -> String {
    let start = Instant::now();
    loop {
        let left = within.saturating_sub(start.elapsed());
        let line = rx
            .recv_timeout(left)
            .unwrap_or_else(|e| panic!("waiting: {e}"));
        if pred(&line) {
            return line;
        }
    }
}

fn get(addr: SocketAddr, path: &str) -> String {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: h\r\nConnection: close\r\n\r\n").as_bytes())
        .unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

/// akit, proxying to `protocol` upstream: one request through it, then SIGTERM; the process
/// exits, successfully, well before `-shutdown-timeout`.
fn shuts_down_promptly(protocol: &str) {
    let up = upstream();
    let dir =
        Dir(std::env::temp_dir().join(format!("akit-shutdown-{protocol}-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir.0);
    std::fs::create_dir_all(&dir.0).unwrap();
    let config = dir.0.join("akit.json");
    std::fs::write(
        &config,
        format!(
            r#"{{
  "listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
  "routes": [{{"handler": {{"module": "proxy", "params": {{"upstream": "app"}}}}}}],
  "upstreams": [{{"name": "app", "addresses": ["{}"], "protocol": "{protocol}"}}]
}}"#,
            up.local_addrs()[0]
        ),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["run", "-shutdown-timeout", "20s", "-c"])
        .arg(&config)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = lines(child.stdout.take().unwrap());
    let err = lines(child.stderr.take().unwrap());
    let line = until(&out, Duration::from_secs(10), |l| {
        l.starts_with("akit: listener ")
    });
    let addr: SocketAddr = line.split(' ').nth(4).unwrap().parse().unwrap();
    let want = if protocol == "h2c" { "h2" } else { "h1" };
    let resp = get(addr, "/");
    assert!(
        resp.starts_with("HTTP/1.1 200") && resp.ends_with(want),
        "{resp}"
    );
    // The upstream connection is kept idle for the next request; the shutdown closes it.
    let start = Instant::now();
    kill(child.id(), SIGTERM).unwrap();
    // akit's shutdown is bounded by -shutdown-timeout: the process exits by then in any case.
    let status = child.wait().unwrap();
    let took = start.elapsed();
    let errors: Vec<String> = err.iter().collect();
    assert!(status.success(), "{status}: {errors:?}");
    assert!(out.iter().any(|l| l == "akit: stopped"));
    assert!(
        took < GRACE,
        "the shutdown took {took:?}, as long as -shutdown-timeout: {errors:?}"
    );
    up.shutdown();
}

#[test]
fn an_idle_h2c_upstream_connection_does_not_hold_the_shutdown() {
    shuts_down_promptly("h2c");
}

#[test]
fn an_idle_http1_upstream_connection_does_not_hold_the_shutdown() {
    shuts_down_promptly("http/1.1");
}
