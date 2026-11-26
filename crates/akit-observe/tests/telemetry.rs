//! OpenTelemetry (I64), as fit's `observe/trace_test.go` and `edges_test.go` test it: reading
//! `traceparent`, sampling, spans exported to a receiver with the export's headers and resource,
//! the queue's bound (spans dropped and counted, never waited for), failures logged once and
//! their end logged, exports on the interval and when enough waits, and metrics as OTLP's.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use akit_config::value::{Value, parse};
use akit_config::{MetricsExport, Telemetry, TracesExport};
use akit_observe::metrics::{DURATION_BUCKETS, Histogram};
use akit_observe::otlp::{Log, MetricWriter, MetricsExporter, Otlp};
use akit_observe::trace::{SPAN_FLUSH_BYTES, SpanEnd, append_span};
use akit_observe::{Trace, Tracer};

const PARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

/// An OTLP/HTTP receiver that keeps what it is sent: HTTP/1.1 with keep-alive, a thread per
/// connection.
#[derive(Clone)]
struct Receiver {
    url: String,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
}

#[derive(Default)]
struct State {
    /// By path.
    bodies: HashMap<String, Vec<Vec<u8>>>,
    /// Of the last request, names in lower case.
    headers: Vec<(String, String)>,
    /// What it answers; 200 when 0.
    status: u16,
}

impl Drop for Receiver {
    fn drop(&mut self) {
        if Arc::strong_count(&self.stop) == 1 {
            self.stop.store(true, Ordering::Release);
        }
    }
}

impl Receiver {
    fn start() -> Receiver {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let r = Receiver {
            url: format!("http://{}", l.local_addr().unwrap()),
            state: Arc::default(),
            stop: Arc::default(),
        };
        l.set_nonblocking(true).unwrap();
        let (state, stop) = (r.state.clone(), r.stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                match l.accept() {
                    Ok((c, _)) => {
                        let state = state.clone();
                        std::thread::spawn(move || serve(c, &state));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        r
    }

    fn got(&self, path: &str) -> Vec<Vec<u8>> {
        self.state
            .lock()
            .unwrap()
            .bodies
            .get(path)
            .cloned()
            .unwrap_or_default()
    }

    fn header(&self, name: &str) -> Option<String> {
        let s = self.state.lock().unwrap();
        s.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    }

    fn set_status(&self, status: u16) {
        self.state.lock().unwrap().status = status;
    }
}

fn serve(c: TcpStream, state: &Mutex<State>) {
    c.set_nonblocking(false).unwrap();
    let mut r = BufReader::new(c.try_clone().unwrap());
    let mut w = c;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let path = line.split(' ').nth(1).unwrap_or("").to_owned();
        let mut headers = Vec::new();
        let mut length = 0;
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
                length = v.parse().unwrap();
            }
            headers.push((k, v));
        }
        let mut body = vec![0; length];
        r.read_exact(&mut body).unwrap();
        let json = headers
            .iter()
            .any(|(k, v)| k == "content-type" && v == "application/json");
        let status = {
            let mut s = state.lock().unwrap();
            s.bodies.entry(path).or_default().push(body);
            s.headers = headers;
            s.status
        };
        let (status, text) = match status {
            _ if !json => (415, "not JSON"),
            0 => (200, "{}"),
            s => (s, "refused"),
        };
        let resp = format!(
            "HTTP/1.1 {status} X\r\ncontent-length: {}\r\n\r\n{text}",
            text.len()
        );
        if w.write_all(resp.as_bytes()).is_err() {
            return;
        }
    }
}

fn telemetry(endpoint: &str) -> Telemetry {
    Telemetry {
        endpoint: endpoint.to_owned(),
        headers_file: String::new(),
        ca_file: String::new(),
        cert_file: String::new(),
        key_file: String::new(),
        service_name: "akit".to_owned(),
        timeout: Duration::from_secs(10),
        traces: None,
        metrics: None,
    }
}

fn traces(ratio: f64, max_bytes: usize, interval: Duration) -> TracesExport {
    TracesExport {
        sample_ratio: ratio,
        max_queue_bytes: if max_bytes == 0 { 4 << 20 } else { max_bytes },
        interval,
    }
}

/// A log that keeps its lines.
fn lines() -> (Log, Arc<Mutex<Vec<String>>>) {
    let lines = Arc::new(Mutex::new(Vec::new()));
    let l = lines.clone();
    (
        Arc::new(move |m: &str| l.lock().unwrap().push(m.to_owned())),
        lines,
    )
}

fn quiet() -> Log {
    Arc::new(|m: &str| eprintln!("{m}"))
}

fn tracer(endpoint: &str, ratio: f64, max_bytes: usize) -> Tracer {
    let otlp = Otlp::new(&telemetry(endpoint), "dev", quiet()).unwrap();
    Tracer::new(otlp, &traces(ratio, max_bytes, Duration::from_secs(3600))).unwrap()
}

/// A request's span, as fit's tests' `request()`: `GET /api/x` from 192.0.2.1 to example.com.
fn span<'a>(start: SystemTime) -> SpanEnd<'a> {
    SpanEnd {
        method: "GET",
        route: "",
        path: "/api/x",
        tls: false,
        host: "example.com",
        client: IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
        version: "1.1",
        status: 200,
        size: 1,
        listener: "web",
        upstream: "",
        start,
        end: SystemTime::now(),
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|c| format!("{c:02x}")).collect()
}

fn wait_for(ok: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("akit-otel-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
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

/// An attribute's value, of whatever type.
fn attr<'a>(span: &'a Value, key: &str) -> Option<&'a str> {
    arr(span.get("attributes"))
        .iter()
        .find(|a| a.get("key").map(s) == Some(key))
        .and_then(|a| match a.get("value") {
            Some(Value::Object(ms)) => ms.first().map(|(_, v)| s(v)),
            _ => None,
        })
}

/// The resource and spans of export requests.
fn decode_spans(bodies: &[Vec<u8>]) -> (Value, Vec<Value>) {
    let mut resource = Value::Null;
    let mut spans = Vec::new();
    for body in bodies {
        let text = std::str::from_utf8(body).unwrap();
        let v = parse(text).unwrap_or_else(|e| panic!("not OTLP JSON: {e:?}\n{text}"));
        for rs in arr(v.get("resourceSpans")) {
            resource = rs.get("resource").unwrap().clone();
            for ss in arr(rs.get("scopeSpans")) {
                assert_eq!(
                    ss.get("scope").and_then(|s| s.get("name")).map(s),
                    Some("akit")
                );
                spans.extend(arr(ss.get("spans")).iter().cloned());
            }
        }
    }
    (resource, spans)
}

/// fit's `TestSample`.
#[test]
fn samples() {
    let none = tracer("http://127.0.0.1:1", 0.0, 0);
    assert!(
        none.sample(None).is_none(),
        "ratio 0 sampled a request without a caller"
    );
    let t = none.sample(Some(PARENT)).unwrap();
    assert_eq!(hex(&t.trace_id), "4bf92f3577b34da6a3ce929d0e0e4736");
    assert_eq!(hex(&t.parent), "00f067aa0ba902b7");
    assert_ne!(t.span_id, [0; 8]);
    let unsampled = format!("{}0", PARENT.strip_suffix('1').unwrap());
    assert!(
        none.sample(Some(&unsampled)).is_none(),
        "a caller that did not sample was not followed"
    );

    let all = tracer("http://127.0.0.1:1", 1.0, 0);
    assert!(
        all.sample(Some(&unsampled)).is_none(),
        "ratio 1 sampled a request its caller did not"
    );
    let a = all.sample(None).unwrap();
    let b = all.sample(Some("garbage")).unwrap();
    assert_ne!(a.trace_id, b.trace_id);
    assert_ne!(a.trace_id, [0; 16]);
    assert_eq!(a.parent, [0; 8]);

    let half = tracer("http://127.0.0.1:1", 0.5, 0);
    let n = (0..20000).filter(|_| half.sample(None).is_some()).count();
    assert!(
        (9000..=11000).contains(&n),
        "ratio 0.5 sampled {n} of 20000"
    );
    for t in [none, all, half] {
        t.close();
    }
}

/// fit's `TestTracerExport`: the spans reach the receiver, with the export's headers, the
/// resource, and each span's names, IDs, times, status and attributes.
#[test]
fn exports_spans() {
    let rcv = Receiver::start();
    let dir = temp_dir("export");
    let headers = dir.join("headers");
    std::fs::write(&headers, "# a key\nX-Api-Key: s3cret\n\nX-Tenant:  akit \n").unwrap();
    let mut c = telemetry(&rcv.url);
    c.headers_file = headers.display().to_string();
    c.service_name = "edge".to_owned();
    let tr = Tracer::new(
        Otlp::new(&c, "dev", quiet()).unwrap(),
        &traces(1.0, 0, Duration::from_secs(3600)),
    )
    .unwrap();
    let t = tr.sample(Some(PARENT)).unwrap();
    let start = SystemTime::now() - Duration::from_millis(1);
    tr.end(
        &t,
        &SpanEnd {
            route: r#"api "v2""#,
            upstream: "10.0.0.1:80",
            status: 502,
            size: 12,
            ..span(start)
        },
    );
    let t2 = tr.sample(None).unwrap();
    tr.end(
        &t2,
        &SpanEnd {
            method: "POST",
            version: "2",
            path: "/%ff%2Fx",
            status: 200,
            size: 0,
            ..span(SystemTime::now())
        },
    );
    tr.close();
    assert_eq!(
        (tr.exported(), tr.failed(), tr.dropped()),
        (2, 0, 0),
        "exported, failed, dropped"
    );
    let bodies = rcv.got("/v1/traces");
    assert_eq!(bodies.len(), 1);
    assert_eq!(rcv.header("x-api-key").as_deref(), Some("s3cret"));
    assert_eq!(rcv.header("x-tenant").as_deref(), Some("akit"));
    let (resource, mut spans) = decode_spans(&bodies);
    let r = resource.to_json();
    assert!(
        r.contains("service.name") && r.contains("edge") && r.contains("service.version"),
        "{r}"
    );
    assert_eq!(spans.len(), 2);
    if spans[0].get("name").map(s) == Some("POST") {
        spans.swap(0, 1);
    }
    let sp = &spans[0];
    assert_eq!(sp.get("name").map(s), Some(r#"GET api "v2""#));
    assert_eq!(sp.get("kind").map(s), Some("2"));
    assert_eq!(
        sp.get("traceId").map(s),
        Some("4bf92f3577b34da6a3ce929d0e0e4736")
    );
    assert_eq!(sp.get("parentSpanId").map(s), Some("00f067aa0ba902b7"));
    assert_eq!(sp.get("spanId").map(s).unwrap(), hex(&t.span_id));
    let (a, b) = (
        sp.get("startTimeUnixNano")
            .map(s)
            .unwrap()
            .parse::<u128>()
            .unwrap(),
        sp.get("endTimeUnixNano")
            .map(s)
            .unwrap()
            .parse::<u128>()
            .unwrap(),
    );
    assert!(a < b, "{a} {b}");
    assert_eq!(
        sp.get("status").and_then(|s| s.get("code")).map(s),
        Some("2")
    );
    for (k, v) in [
        ("http.request.method", "GET"),
        ("url.path", "/api/x"),
        ("url.scheme", "http"),
        ("server.address", "example.com"),
        ("client.address", "192.0.2.1"),
        ("network.protocol.version", "1.1"),
        ("http.response.status_code", "502"),
        ("http.response.body.size", "12"),
        ("akit.listener", "web"),
        ("akit.route", r#"api "v2""#),
        ("akit.upstream.address", "10.0.0.1:80"),
    ] {
        assert_eq!(attr(sp, k), Some(v), "{k}");
    }
    let sp = &spans[1];
    assert_eq!(sp.get("name").map(s), Some("POST"));
    assert!(sp.get("parentSpanId").is_none());
    assert!(sp.get("status").is_none());
    assert_eq!(attr(sp, "network.protocol.version"), Some("2"));
    assert_eq!(attr(sp, "url.path"), Some("/\u{fffd}/x"));
    assert_eq!(attr(sp, "akit.route"), None);
    let _ = std::fs::remove_dir_all(dir);
}

/// fit's `TestTracerBounds`: spans past `max_queue_bytes` are dropped and counted; an export the
/// receiver refuses counts its spans failed, and is logged once while it lasts; room comes back
/// once exported, and exports that work again are logged.
#[test]
fn bounds_and_failures() {
    let rcv = Receiver::start();
    rcv.set_status(503);
    let (log, lines) = lines();
    let tr = Tracer::new(
        Otlp::new(&telemetry(&rcv.url), "dev", log).unwrap(),
        &traces(1.0, 2000, Duration::from_secs(3600)),
    )
    .unwrap();
    let one = |tr: &Tracer| {
        let t = tr.sample(None).unwrap();
        tr.end(&t, &span(SystemTime::now()));
    };
    let mut len = Vec::new();
    append_span(&mut len, &Trace::default(), &span(SystemTime::now()));
    assert!(
        (450..800).contains(&len.len()),
        "a span of {} bytes",
        len.len()
    );
    // About 700 bytes a span: two or three fit, the rest are dropped.
    for _ in 0..10 {
        one(&tr);
    }
    let d = tr.dropped();
    assert!((5..=8).contains(&d), "{d} dropped");
    tr.flush();
    tr.flush(); // nothing waits: no request
    assert_eq!(rcv.got("/v1/traces").len(), 1);
    assert_eq!((tr.failed(), tr.exported()), (10 - d, 0));
    // Room again once exported, and a failure that lasts is logged once.
    for _ in 0..2 {
        one(&tr);
        tr.flush();
    }
    rcv.set_status(0);
    one(&tr);
    tr.close();
    assert_eq!(tr.exported(), 1);
    let lines = lines.lock().unwrap();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].contains("503") && lines[0].contains("traces"),
        "{lines:?}"
    );
    assert!(lines[1].contains("works again"), "{lines:?}");
}

/// fit's `TestTracerTicks`: spans are exported every interval.
#[test]
fn exports_every_interval() {
    let rcv = Receiver::start();
    let tr = Tracer::new(
        Otlp::new(&telemetry(&rcv.url), "dev", quiet()).unwrap(),
        &traces(1.0, 0, Duration::from_millis(10)),
    )
    .unwrap();
    let t = tr.sample(None).unwrap();
    tr.end(&t, &span(SystemTime::now()));
    wait_for(|| tr.exported() == 1);
    tr.close();
}

/// fit's `TestTracerFlushesWhenFull`: spans are exported before the interval once enough wait.
#[test]
fn exports_when_enough_wait() {
    let rcv = Receiver::start();
    let tr = tracer(&rcv.url, 1.0, 0);
    let mut n = 0;
    while tr.queued() < SPAN_FLUSH_BYTES && n < 10000 {
        let t = tr.sample(None).unwrap();
        tr.end(&t, &span(SystemTime::now()));
        n += 1;
    }
    wait_for(|| !rcv.got("/v1/traces").is_empty());
    tr.close();
}

/// Spans ended on many threads at once all reach the receiver, or are counted dropped.
#[test]
fn many_threads() {
    let rcv = Receiver::start();
    let tr = tracer(&rcv.url, 1.0, 0);
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let tr = tr.clone();
            std::thread::spawn(move || {
                for _ in 0..500 {
                    let t = tr.sample(None).unwrap();
                    tr.end(&t, &span(SystemTime::now()));
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    tr.close();
    assert_eq!(tr.exported() + tr.dropped(), 4000);
    assert_eq!(tr.failed(), 0);
    let (_, spans) = decode_spans(&rcv.got("/v1/traces"));
    assert_eq!(spans.len() as u64, tr.exported());
}

/// fit's `TestSpanProtocols`: the scheme and protocol version of HTTPS, HTTP/3 and HTTP/1.0
/// requests, and the client's address of an IPv4 address mapped into IPv6.
#[test]
fn span_protocols() {
    for (version, tls, want) in [
        (
            "1.0",
            false,
            [
                r#""url.scheme","value":{"stringValue":"http"}"#,
                r#""network.protocol.version","value":{"stringValue":"1.0"}"#,
            ],
        ),
        (
            "3",
            true,
            [
                r#""url.scheme","value":{"stringValue":"https"}"#,
                r#""network.protocol.version","value":{"stringValue":"3"}"#,
            ],
        ),
    ] {
        let mut b = Vec::new();
        let sp = SpanEnd {
            version,
            tls,
            client: "::ffff:192.0.2.7".parse().unwrap(),
            ..span(SystemTime::now())
        };
        append_span(&mut b, &Trace::default(), &sp);
        let got = String::from_utf8(b).unwrap();
        for w in want {
            assert!(got.contains(w), "HTTP/{version}: {got} lacks {w}");
        }
        assert!(
            got.contains(r#""client.address","value":{"stringValue":"192.0.2.7"}"#),
            "{got}"
        );
        assert!(got.ends_with("},"), "{got}");
    }
}

/// fit's `TestExporterErrors`: a headers file that is missing or holds no headers, and a CA file
/// that is missing or holds no certificate, refuse the exporter, naming the file.
#[test]
fn exporter_errors() {
    let dir = temp_dir("errors");
    let bad = dir.join("bad");
    std::fs::write(&bad, "not a header\n").unwrap();
    let missing = dir.join("missing").display().to_string();
    let bad = bad.display().to_string();
    for (field, path) in [
        ("headers_file", &missing),
        ("headers_file", &bad),
        ("ca_file", &missing),
        ("ca_file", &bad),
    ] {
        let mut c = telemetry("http://127.0.0.1:1");
        match field {
            "headers_file" => c.headers_file = path.clone(),
            _ => c.ca_file = path.clone(),
        }
        let err = Otlp::new(&c, "dev", quiet()).unwrap_err();
        assert!(err.contains(path.as_str()), "{field} {path}: {err}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// fit's `TestExportFailures`: exports to an endpoint nothing listens on fail, and are counted.
#[test]
fn export_failures() {
    let mut c = telemetry("http://127.0.0.1:1");
    c.timeout = Duration::from_secs(2);
    let (log, lines) = lines();
    let m = MetricsExporter::new(
        Otlp::new(&c, "dev", log).unwrap(),
        Duration::from_secs(3600),
        Box::new(|w: &mut dyn MetricWriter| {
            w.family("akit_up", "gauge", "Up.");
            w.sample("akit_up", 1.0, &[]);
        }),
    )
    .unwrap();
    m.export();
    m.close();
    assert_eq!((m.failed(), m.exported()), (2, 0));
    let lines = lines.lock().unwrap();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("exporting metrics"), "{lines:?}");
}

/// fit's `TestMetricsExporter`: a counter as a cumulative, monotonic sum, a gauge as a gauge, a
/// histogram as an explicit-bucket one, each bucket counting its own.
#[test]
fn exports_metrics() {
    let rcv = Receiver::start();
    let h = Histogram::new();
    h.observe(Duration::from_millis(3));
    h.observe(Duration::from_millis(30));
    h.observe(Duration::from_secs(20));
    let snap = h.snapshot();
    let mut c = telemetry(&format!("{}/", rcv.url));
    c.endpoint = c.endpoint.trim_end_matches('/').to_owned();
    c.metrics = Some(MetricsExport {
        interval: Duration::from_secs(3600),
    });
    let m = MetricsExporter::new(
        Otlp::new(&c, "dev", quiet()).unwrap(),
        Duration::from_secs(3600),
        Box::new(move |w: &mut dyn MetricWriter| {
            w.family("akit_http_requests_total", "counter", "Requests.");
            w.sample(
                "akit_http_requests_total",
                7.0,
                &[("route", "api"), ("code", "2xx")],
            );
            w.family("akit_connections_open", "gauge", "Connections.");
            w.sample_u64("akit_connections_open", 3, &[]);
            w.family(
                "akit_http_request_duration_seconds",
                "histogram",
                "Durations.",
            );
            w.histogram(
                "akit_http_request_duration_seconds",
                &snap,
                &[("route", "api")],
            );
        }),
    )
    .unwrap();
    m.close();
    let bodies = rcv.got("/v1/metrics");
    assert_eq!((bodies.len(), m.exported()), (1, 1));
    let text = std::str::from_utf8(&bodies[0]).unwrap();
    let v = parse(text).unwrap_or_else(|e| panic!("{e:?}\n{text}"));
    let rm = &arr(v.get("resourceMetrics"))[0];
    assert!(
        rm.get("resource")
            .unwrap()
            .to_json()
            .contains("service.name")
    );
    let sm = &arr(rm.get("scopeMetrics"))[0];
    let ms = arr(sm.get("metrics"));
    assert_eq!(ms.len(), 3, "{text}");
    let sum = ms[0].get("sum").unwrap();
    let point = &arr(sum.get("dataPoints"))[0];
    assert_eq!(ms[0].get("name").map(s), Some("akit_http_requests_total"));
    assert_eq!(sum.get("isMonotonic"), Some(&Value::Bool(true)));
    assert_eq!(sum.get("aggregationTemporality").map(s), Some("2"));
    assert_eq!(point.get("asDouble").map(s), Some("7"));
    assert!(point.get("startTimeUnixNano").is_some());
    assert_eq!(arr(point.get("attributes")).len(), 2);
    let g = &arr(ms[1].get("gauge").unwrap().get("dataPoints"))[0];
    assert_eq!(g.get("asDouble").map(s), Some("3"));
    assert!(g.get("startTimeUnixNano").is_none());
    let hp = &arr(ms[2].get("histogram").unwrap().get("dataPoints"))[0];
    let buckets: Vec<&str> = arr(hp.get("bucketCounts")).iter().map(s).collect();
    assert_eq!(ms[2].get("unit").map(s), Some("s"));
    assert_eq!(hp.get("count").map(s), Some("3"));
    assert_eq!(
        buckets,
        ["1", "0", "0", "1", "0", "0", "0", "0", "0", "0", "0", "1"]
    );
    assert_eq!(arr(hp.get("explicitBounds")).len(), DURATION_BUCKETS.len());
}

/// fit's `TestMetricsExporterTicks`: metrics are exported every interval.
#[test]
fn exports_metrics_every_interval() {
    let rcv = Receiver::start();
    let m = MetricsExporter::new(
        Otlp::new(&telemetry(&rcv.url), "dev", quiet()).unwrap(),
        Duration::from_millis(10),
        Box::new(|w: &mut dyn MetricWriter| {
            w.family("akit_up", "gauge", "Up.");
            w.sample("akit_up", 1.0, &[]);
        }),
    )
    .unwrap();
    wait_for(|| rcv.got("/v1/metrics").len() >= 2);
    m.close();
}
