//! End to end: `akit run` with `telemetry` (I64) traces the requests whose caller sampled them
//! (and those the sample ratio picks), sends the upstream a `traceparent` naming its own span,
//! passes on the `traceparent` of a request it does not trace as it came, and exports the spans
//! and the metrics to an OTLP/HTTP collector, the last of them when it stops.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use akio_sys::process::{SIGTERM, kill};
use akit_config::value::{Value, parse};

const SAMPLED: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
const UNSAMPLED: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-00";

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
            .stderr(Stdio::inherit())
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
        let addr = line.split(' ').nth(4).unwrap().parse().unwrap();
        a.until("akit: exporting telemetry to http://127.0.0.1:");
        (a, addr)
    }

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

    fn stop(mut self) {
        kill(self.child.id(), SIGTERM).unwrap();
        self.until("akit: stopped");
        assert!(self.child.wait().unwrap().success());
    }
}

/// The `traceparent` fields of each request the upstream had, by path.
type Seen = Vec<(String, Vec<String>)>;

/// What a test server answers a request with: its path, fields (names in lower case) and body.
type Answer = dyn Fn(&str, &[(String, String)], Vec<u8>) -> String + Send + Sync;

/// Reads HTTP/1.1 requests from `s`, calling `f` with each one and writing what it returns, until
/// the connection ends.
fn serve(s: TcpStream, f: &Answer) {
    let mut r = BufReader::new(s);
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let path = line.split(' ').nth(1).unwrap_or("").to_owned();
        let mut fields = Vec::new();
        let mut len = 0;
        loop {
            let mut h = String::new();
            if r.read_line(&mut h).unwrap_or(0) == 0 {
                return;
            }
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            let (k, v) = h.split_once(':').unwrap();
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_owned());
            if k == "content-length" {
                len = v.parse().unwrap_or(0);
            }
            fields.push((k, v));
        }
        let mut body = vec![0; len];
        if r.read_exact(&mut body).is_err() {
            return;
        }
        let resp = f(&path, &fields, body);
        if r.get_mut().write_all(resp.as_bytes()).is_err() {
            return;
        }
    }
}

fn listen(
    f: impl Fn(&str, &[(String, String)], Vec<u8>) -> String + Send + Sync + 'static,
) -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let f: Arc<Answer> = Arc::new(f);
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            let f = f.clone();
            std::thread::spawn(move || serve(s, &*f));
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

fn s(v: &Value) -> &str {
    match v {
        Value::String(s) | Value::Number(s) => s,
        _ => "",
    }
}

fn arr(v: Option<&Value>) -> &[Value] {
    match v {
        Some(Value::Array(a)) => a,
        _ => &[],
    }
}

fn attr<'a>(span: &'a Value, key: &str) -> Option<&'a str> {
    arr(span.get("attributes"))
        .iter()
        .find(|a| a.get("key").map(s) == Some(key))
        .and_then(|a| match a.get("value") {
            Some(Value::Object(ms)) => ms.first().map(|(_, v)| s(v)),
            _ => None,
        })
}

#[test]
fn traces_reach_the_collector() {
    let dir = Dir::new("telemetry");
    // The collector: what it was sent, by path.
    let got: Arc<Mutex<HashMap<String, Vec<String>>>> = Arc::default();
    let g = got.clone();
    let collector = listen(move |path, fields, body| {
        assert!(
            fields
                .iter()
                .any(|(k, v)| k == "content-type" && v == "application/json")
        );
        let body = String::from_utf8(body).unwrap();
        g.lock()
            .unwrap()
            .entry(path.to_owned())
            .or_default()
            .push(body);
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}".to_owned()
    });
    // The upstream: the traceparent fields of each request, by path.
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let sn = seen.clone();
    let up = listen(move |path, fields, _| {
        let tp = fields
            .iter()
            .filter(|(k, _)| k == "traceparent")
            .map(|(_, v)| v.clone())
            .collect();
        sn.lock().unwrap().push((path.to_owned(), tp));
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nup".to_owned()
    });
    let cfg = dir.0.join("akit.json");
    std::fs::write(
        &cfg,
        format!(
            r#"{{
  "listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
  "routes": [
    {{"name": "hi", "match": {{"path": ["/hello"]}},
      "handler": {{"module": "respond", "params": {{"body": "hello"}}}}}},
    {{"name": "api", "match": {{"path": ["/api/*"]}},
      "handler": {{"module": "proxy", "params": {{"upstream": "app"}}}}}}
  ],
  "upstreams": [{{"name": "app", "addresses": ["{up}"]}}],
  "telemetry": {{
    "endpoint": "http://{collector}",
    "service_name": "edge-gateway",
    "traces": {{"sample_ratio": 0, "interval": "1h"}},
    "metrics": {{"interval": "1h"}}
  }}
}}"#
        ),
    )
    .unwrap();
    let (akit, addr) = Akit::start(&cfg);
    let req = |path: &str, tp: Option<&str>| {
        let tp = tp
            .map(|t| format!("traceparent: {t}\r\n"))
            .unwrap_or_default();
        send(
            addr,
            &format!(
                "GET {path} HTTP/1.1\r\nHost: example.com\r\n{tp}tracestate: k=v\r\nConnection: close\r\n\r\n"
            ),
        )
    };
    assert_eq!(req("/api/traced", Some(SAMPLED)), "HTTP/1.1 200 OK");
    assert_eq!(req("/api/declined", Some(UNSAMPLED)), "HTTP/1.1 200 OK");
    assert_eq!(req("/api/none", None), "HTTP/1.1 200 OK");
    assert_eq!(req("/hello", Some(SAMPLED)), "HTTP/1.1 200 OK");
    assert_eq!(req("/nope", Some(SAMPLED)), "HTTP/1.1 404 Not Found");
    // The spans wait for an interval that never comes: they are exported as akit stops.
    akit.stop();

    let seen = seen.lock().unwrap().clone();
    let tp = |path: &str| {
        seen.iter()
            .find(|(p, _)| p == path)
            .map(|(_, t)| t.clone())
            .unwrap()
    };
    // A request its caller sampled: the upstream gets akit's span, in the caller's trace.
    let traced = tp("/api/traced");
    assert_eq!(traced.len(), 1, "{traced:?}");
    let t = &traced[0];
    assert!(
        t.starts_with("00-4bf92f3577b34da6a3ce929d0e0e4736-") && t.ends_with("-01"),
        "{t}"
    );
    let span_id = &t[36..52];
    assert_ne!(span_id, "00f067aa0ba902b7");
    // A request it did not: its field goes on as it came; one without has none.
    assert_eq!(tp("/api/declined"), [UNSAMPLED]);
    assert!(tp("/api/none").is_empty());

    let got = got.lock().unwrap().clone();
    let traces = got.get("/v1/traces").expect("no traces exported");
    let mut spans = Vec::new();
    for body in traces {
        let v = parse(body).unwrap();
        for rs in arr(v.get("resourceSpans")) {
            let r = rs.get("resource").unwrap().to_json();
            assert!(r.contains("edge-gateway"), "{r}");
            for ss in arr(rs.get("scopeSpans")) {
                spans.extend(arr(ss.get("spans")).iter().cloned());
            }
        }
    }
    assert_eq!(spans.len(), 3, "{traces:?}");
    let by = |path: &str| {
        spans
            .iter()
            .find(|sp| attr(sp, "url.path") == Some(path))
            .unwrap_or_else(|| panic!("no span of {path}"))
    };
    let sp = by("/api/traced");
    assert_eq!(sp.get("name").map(s), Some("GET api"));
    assert_eq!(sp.get("spanId").map(s), Some(span_id));
    assert_eq!(sp.get("parentSpanId").map(s), Some("00f067aa0ba902b7"));
    assert_eq!(attr(sp, "akit.route"), Some("api"));
    assert_eq!(attr(sp, "akit.listener"), Some("web"));
    assert_eq!(
        attr(sp, "akit.upstream.address"),
        Some(up.to_string().as_str())
    );
    assert_eq!(attr(sp, "http.response.status_code"), Some("200"));
    assert_eq!(attr(sp, "http.response.body.size"), Some("2"));
    assert_eq!(attr(sp, "server.address"), Some("example.com"));
    assert_eq!(attr(sp, "client.address"), Some("127.0.0.1"));
    let sp = by("/hello");
    assert_eq!(sp.get("name").map(s), Some("GET hi"));
    assert_eq!(attr(sp, "akit.upstream.address"), None);
    let sp = by("/nope");
    assert_eq!(sp.get("name").map(s), Some("GET"));
    assert_eq!(attr(sp, "http.response.status_code"), Some("404"));

    // The metrics, requests counted (telemetry.metrics counts them, as metrics does), and the
    // spans exported by the time of the last export.
    let metrics = got.get("/v1/metrics").expect("no metrics exported");
    let last = metrics.last().unwrap();
    for name in [
        "akit_http_requests_total",
        "akit_http_request_duration_seconds",
        "akit_upstream_backend_requests_total",
        "akit_trace_spans_total",
        "akit_config_version",
    ] {
        assert!(
            last.contains(&format!("\"name\":\"{name}\"")),
            "{name}: {last}"
        );
    }
    parse(last).unwrap();
}

/// A telemetry setting akit cannot use stops `akit run` before it serves, naming it.
#[test]
fn a_bad_headers_file_stops_the_run() {
    let dir = Dir::new("telemetry-bad");
    let cfg = dir.0.join("akit.json");
    let headers = dir.0.join("missing-headers");
    std::fs::write(
        &cfg,
        format!(
            r#"{{
  "listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
  "routes": [{{"handler": {{"module": "respond", "params": {{"body": "hi"}}}}}}],
  "telemetry": {{"headers_file": "{}", "traces": {{}}}}
}}"#,
            headers.display()
        ),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["run", "-c"])
        .arg(&cfg)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("telemetry") && err.contains("missing-headers"),
        "{err}"
    );
}
