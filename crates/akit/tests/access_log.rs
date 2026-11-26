//! End to end: `akit run` with `access_log` (I21) writes a line for each request, with fit's
//! fields: answered by a handler, proxied (with the upstream's address and time), not routed
//! (404), refused (400), `HEAD`; every line is written by the time akit has stopped, even with a
//! flush interval that never came; a reload changes the level in place. I81: a file rotated by
//! size and compressed keeps every line, whole, in exactly one file, and a reload changes its
//! retention in place.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use akio_sys::process::{SIGHUP, SIGTERM, kill};

/// A directory of the test's own, removed when it ends.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let d = std::env::temp_dir().join(format!("akit-{name}-{}", std::process::id()));
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

/// The running binary, killed if the test fails before it exits.
struct Akit {
    child: Child,
    out: mpsc::Receiver<String>,
}

impl Drop for Akit {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Akit {
    fn start(config: &Path) -> (Akit, SocketAddr) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
            .args(["run", "-shutdown-timeout", "5s", "-c"])
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
        let a = Akit { child, out };
        let line = a.until("akit: listener ");
        // `akit: listener "web" on ADDR (...)`.
        let addr = line.split(' ').nth(4).unwrap().parse().unwrap();
        (a, addr)
    }

    /// stdout's first line from now that starts with `prefix`.
    fn until(&self, prefix: &str) -> String {
        loop {
            let line = self
                .out
                .recv_timeout(Duration::from_secs(10))
                .unwrap_or_else(|e| panic!("waiting for {prefix:?}: {e}"));
            if line.starts_with(prefix) {
                return line;
            }
        }
    }

    /// SIGTERM, and the exit.
    fn stop(mut self) {
        kill(self.child.id(), SIGTERM).unwrap();
        self.until("akit: stopped");
        assert!(self.child.wait().unwrap().success());
    }
}

/// An upstream answering every request `200 up`, on a thread of its own.
fn upstream() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            std::thread::spawn(move || {
                let mut r = BufReader::new(s);
                loop {
                    let mut len = 0;
                    loop {
                        let mut h = String::new();
                        if r.read_line(&mut h).unwrap_or(0) == 0 {
                            return;
                        }
                        let h = h.trim_end().to_ascii_lowercase();
                        if h.is_empty() {
                            break;
                        }
                        if let Some(v) = h.strip_prefix("content-length:") {
                            len = v.trim().parse().unwrap_or(0);
                        }
                    }
                    let mut body = vec![0; len];
                    if r.read_exact(&mut body).is_err() {
                        return;
                    }
                    let _ = r
                        .get_mut()
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nup");
                }
            });
        }
    });
    addr
}

/// Sends `req` on a connection of its own and returns the status line.
fn send(addr: SocketAddr, req: &str) -> String {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(req.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    String::from_utf8_lossy(&out)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn config(dir: &Path, log: &Path, up: SocketAddr, access: &str) -> PathBuf {
    let path = dir.join("akit.json");
    std::fs::write(
        &path,
        format!(
            r#"{{
  "listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
  "routes": [
    {{"name": "hi", "match": {{"path": ["/hello"]}},
      "handler": {{"module": "respond", "params": {{"body": "hello"}}}}}},
    {{"name": "api", "match": {{"path": ["/api/*"]}},
      "handler": {{"module": "proxy", "params": {{"upstream": "app"}}}}}},
    {{"name": "old", "match": {{"path": ["/old/*"]}},
      "middleware": [{{"module": "rewrite", "params": {{"strip_prefix": ["/old"], "add_prefix": "/api"}}}}],
      "handler": {{"module": "proxy", "params": {{"upstream": "app"}}}}}}
  ],
  "upstreams": [{{"name": "app", "addresses": ["{up}"]}}],
  "access_log": {{"output": "{}", {access}}}
}}"#,
            log.display()
        ),
    )
    .unwrap();
    path
}

/// The raw value of `"key":` in a JSON line, up to the next field.
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line.split(&format!("\"{key}\":")).nth(1)?;
    let end = rest.find(",\"").or_else(|| rest.find('}'))?;
    Some(&rest[..end])
}

#[test]
fn every_request_has_its_line() {
    let dir = Dir::new("access-log");
    let log = dir.0.join("access.log");
    let up = upstream();
    // A flush interval that never comes: the lines are written at shutdown.
    let cfg = config(
        &dir.0,
        &log,
        up,
        r#""flush_interval": "1h", "time_zone": "utc""#,
    );
    let (akit, addr) = Akit::start(&cfg);
    let reqs = [
        "GET /hello?x=1 HTTP/1.1\r\nHost: example.com\r\nUser-Agent: t/1\r\nReferer: https://r/\r\nConnection: close\r\n\r\n",
        "POST /api/x HTTP/1.1\r\nHost: example.com\r\nContent-Length: 4\r\nConnection: close\r\n\r\nabcd",
        "GET /nope HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n",
        "GET /a/../../x HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n",
        "HEAD /hello HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n",
        "GET /hello HTTP/1.0\r\n\r\n",
        "GET /old/y?z=1 HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n",
        "GET /x/./../hello HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n",
        "GET http://u:p@abs.example/hello?y=2 HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n",
    ];
    let statuses: Vec<String> = reqs.iter().map(|r| send(addr, r)).collect();
    assert_eq!(
        statuses,
        [
            "HTTP/1.1 200 OK",
            "HTTP/1.1 200 OK",
            "HTTP/1.1 404 Not Found",
            "HTTP/1.1 400 Bad Request",
            "HTTP/1.1 200 OK",
            "HTTP/1.1 200 OK",
            "HTTP/1.1 200 OK",
            "HTTP/1.1 200 OK",
            "HTTP/1.1 200 OK"
        ]
    );
    akit.stop();
    let text = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), reqs.len(), "{text}");
    let by = |method: &str, uri: &str, proto: &str| {
        *lines
            .iter()
            .find(|l| {
                field(l, "method") == Some(&format!("\"{method}\""))
                    && field(l, "uri") == Some(&format!("\"{uri}\""))
                    && field(l, "proto") == Some(&format!("\"{proto}\""))
            })
            .unwrap_or_else(|| panic!("no line for {method} {uri}: {text}"))
    };
    for l in &lines {
        let time = field(l, "time").unwrap();
        assert!(time.len() == 26 && time.ends_with("Z\""), "{l}");
        assert_eq!(field(l, "listener"), Some("\"web\""), "{l}");
        assert_eq!(field(l, "client"), Some("\"127.0.0.1\""), "{l}");
        assert!(field(l, "duration").unwrap().parse::<f64>().is_ok(), "{l}");
        assert!(l.ends_with('}'), "{l}");
    }
    let hello = by("GET", "/hello?x=1", "HTTP/1.1");
    assert!(
        hello.contains(concat!(
            r#""route":"hi","client":"127.0.0.1","proto":"HTTP/1.1","method":"GET","#,
            r#""host":"example.com","uri":"/hello?x=1","status":200,"bytes_in":0,"bytes_out":5,"#,
        )),
        "{hello}"
    );
    assert!(
        hello.ends_with(r#""user_agent":"t/1","referer":"https://r/"}"#),
        "{hello}"
    );
    assert_eq!(field(hello, "upstream"), None, "{hello}");
    let api = by("POST", "/api/x", "HTTP/1.1");
    assert_eq!(field(api, "route"), Some("\"api\""), "{api}");
    assert_eq!(field(api, "status"), Some("200"), "{api}");
    assert_eq!(field(api, "bytes_in"), Some("4"), "{api}");
    assert_eq!(field(api, "bytes_out"), Some("2"), "{api}");
    assert_eq!(
        field(api, "upstream"),
        Some(format!("\"{up}\"").as_str()),
        "{api}"
    );
    let took: f64 = field(api, "upstream_duration").unwrap().parse().unwrap();
    let total: f64 = field(api, "duration").unwrap().parse().unwrap();
    assert!(took <= total + 1e-6, "{api}");
    let nope = by("GET", "/nope", "HTTP/1.1");
    assert_eq!(field(nope, "route"), None, "{nope}");
    assert_eq!(
        (field(nope, "status"), field(nope, "bytes_out")),
        (Some("404"), Some("19")),
        "{nope}"
    );
    let bad = by("GET", "/a/../../x", "HTTP/1.1");
    assert_eq!(field(bad, "status"), Some("400"), "{bad}");
    let head = by("HEAD", "/hello", "HTTP/1.1");
    assert_eq!(
        (field(head, "status"), field(head, "bytes_out")),
        (Some("200"), Some("0")),
        "{head}"
    );
    let old = by("GET", "/hello", "HTTP/1.0");
    assert_eq!(field(old, "host"), Some("\"\""), "{old}");
    // The target fit's `RequestURI` keeps: a rewritten path is not written (the upstream got
    // `/api/y?z=1`), a canonical one is (dot segments removed by the listener).
    let rewritten = by("GET", "/old/y?z=1", "HTTP/1.1");
    assert_eq!(field(rewritten, "route"), Some("\"old\""), "{rewritten}");
    let canonical = by("GET", "/hello", "HTTP/1.1");
    assert_eq!(field(canonical, "status"), Some("200"), "{canonical}");
    // A target in absolute form is routed by its path and host and written as sent, as fit's
    // `RequestURI` (akio's A200).
    let absolute = by("GET", "http://u:p@abs.example/hello?y=2", "HTTP/1.1");
    assert_eq!(field(absolute, "route"), Some("\"hi\""), "{absolute}");
    assert_eq!(field(absolute, "host"), Some("\"abs.example\""), "{absolute}");
}

#[test]
fn reload_changes_the_level() {
    let dir = Dir::new("access-log-reload");
    let log = dir.0.join("access.log");
    let up = upstream();
    let cfg = config(
        &dir.0,
        &log,
        up,
        r#""flush_interval": "10ms", "format": "text""#,
    );
    let (akit, addr) = Akit::start(&cfg);
    assert_eq!(
        send(
            addr,
            "GET /nope HTTP/1.1\r\nHost: h\r\nConnection: close\r\n\r\n"
        ),
        "HTTP/1.1 404 Not Found"
    );
    // Only 5xx from now on.
    config(
        &dir.0,
        &log,
        up,
        r#""flush_interval": "10ms", "format": "text", "level": "error""#,
    );
    kill(akit.child.id(), SIGHUP).unwrap();
    akit.until("akit: reloaded");
    for uri in ["/nope", "/hello"] {
        send(
            addr,
            &format!("GET {uri} HTTP/1.1\r\nHost: h\r\nConnection: close\r\n\r\n"),
        );
    }
    akit.stop();
    let text = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "{text}");
    assert!(lines[0].starts_with("time="), "{text}");
    assert!(
        lines[0].contains(" level=WARN msg=access listener=web client=127.0.0.1 proto=HTTP/1.1 method=GET host=h uri=/nope status=404 "),
        "{text}"
    );
}

/// The lines of the log `base` and its rotated files in `dir`, gzipped or not; each file ends
/// on a whole line.
fn lines_of(dir: &Path, base: &str) -> (Vec<String>, Vec<String>) {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n == base || n.starts_with(&format!("{base}.")))
        .collect();
    names.sort();
    let mut lines = Vec::new();
    for n in &names {
        assert!(!n.ends_with(".tmp"), "{names:?}");
        let data = std::fs::read(dir.join(n)).unwrap();
        let data = if n.ends_with(".gz") {
            akio_deflate::decompress(akio_deflate::Format::Gzip, &data, 1 << 26).unwrap()
        } else {
            data
        };
        let text = String::from_utf8(data).unwrap();
        assert!(text.is_empty() || text.ends_with('\n'), "{n} ends mid-line");
        lines.extend(text.lines().map(str::to_owned));
    }
    (names, lines)
}

#[test]
fn a_rotated_file_keeps_every_line() {
    let dir = Dir::new("access-log-rotate");
    let log = dir.0.join("access.log");
    let up = upstream();
    let rotate = r#""flush_interval": "10ms", "rotate": {"max_bytes": 1024, "max_files": 1000}"#;
    let cfg = config(&dir.0, &log, up, rotate);
    let (akit, addr) = Akit::start(&cfg);
    for i in 0..60 {
        send(
            addr,
            &format!("GET /hello?n={i} HTTP/1.1\r\nHost: h\r\nConnection: close\r\n\r\n"),
        );
        if i % 5 == 0 {
            // Some flushes between, so batches are small.
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    akit.stop();
    let (names, lines) = lines_of(&dir.0, "access.log");
    assert!(names.len() >= 5, "{names:?}");
    assert!(names.iter().any(|n| n.ends_with(".gz")), "{names:?}");
    for i in 0..60 {
        let uri = format!("\"uri\":\"/hello?n={i}\"");
        assert_eq!(
            lines.iter().filter(|l| l.contains(&uri)).count(),
            1,
            "{uri}: {lines:?}"
        );
    }
}

#[test]
fn reload_changes_the_retention() {
    let dir = Dir::new("access-log-retention");
    let log = dir.0.join("access.log");
    let up = upstream();
    let keep = |n: u32| {
        format!(
            r#""flush_interval": "10ms", "rotate": {{"max_bytes": 512, "max_files": {n}, "compress": false}}"#
        )
    };
    let cfg = config(&dir.0, &log, up, &keep(100));
    let (akit, addr) = Akit::start(&cfg);
    let burst = |from: u32| {
        for i in from..from + 30 {
            send(
                addr,
                &format!("GET /hello?n={i} HTTP/1.1\r\nHost: h\r\nConnection: close\r\n\r\n"),
            );
            std::thread::sleep(Duration::from_millis(15));
        }
    };
    burst(0);
    // Each line is about 230 bytes: many files by now, none removed.
    std::thread::sleep(Duration::from_millis(100));
    let (before, _) = lines_of(&dir.0, "access.log");
    assert!(before.len() > 4, "{before:?}");
    config(&dir.0, &log, up, &keep(2));
    kill(akit.child.id(), SIGHUP).unwrap();
    akit.until("akit: reloaded");
    burst(100);
    akit.stop();
    // The open file and the two newest rotated ones.
    let (after, lines) = lines_of(&dir.0, "access.log");
    assert_eq!(after.len(), 3, "{after:?}");
    assert!(
        lines.iter().any(|l| l.contains("/hello?n=129")),
        "{lines:?}"
    );
}
