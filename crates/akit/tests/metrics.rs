//! The scrape (I37), checked against fit's names, labels and values as fit's `TestMetrics` and
//! `TestMetricsOff` check them (`admin/metrics_test.go`): requests by listener and route, which a
//! reload that keeps a route keeps counting and one that renames it drops; upstreams and their
//! backends; certificates; config reloads; connections; akio's engine metrics. The scrape is
//! parsed as fit's test parses it: each family's `HELP` and `TYPE` once, before its samples, no
//! sample twice, values that parse. And what `GET /status` says of the same (I63), as fit's
//! `TestStatus` checks it. On the backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::arithmetic_side_effects
)]

mod support;

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use akio::{Backend, UringConfig};
use akit_listener::Listeners;
use akit_listener::metrics::{CONTENT_TYPE, Scrape};
use akit_observe::AccessLog;
use akit_observe::metrics::{ConfigStats, Metrics};
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, Graph, GraphCell, HandlerParams, Registry, compile};
use std::sync::Mutex;
use support::certs::{Ca, dir};
use support::https::leaf_der;

fn options() -> akit_listener::Options {
    let mut o = akit_listener::Options::default();
    o.backend = match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    };
    o.shutdown_timeout = Duration::from_secs(5);
    o
}

/// Parses `text` as Prometheus's text format, as far as fit writes it, and returns the samples
/// by name and labels, as written (fit's `parseMetrics`).
fn parse(text: &str) -> HashMap<String, f64> {
    let mut samples = HashMap::new();
    let mut typed: HashMap<String, String> = HashMap::new();
    let mut family = String::new();
    for (i, line) in text.trim_end_matches('\n').split('\n').enumerate() {
        let at = i + 1;
        if let Some(rest) = line.strip_prefix("# HELP ") {
            let name = rest.split(' ').next().unwrap().to_owned();
            assert!(!typed.contains_key(&name), "line {at}: family {name} again");
            typed.insert(name.clone(), String::new());
            family = name;
        } else if let Some(rest) = line.strip_prefix("# TYPE ") {
            let f: Vec<&str> = rest.split(' ').collect();
            assert!(
                f.len() == 2 && f[0] == family,
                "line {at}: TYPE without its HELP: {line:?}"
            );
            typed.insert(family.clone(), f[1].to_owned());
        } else {
            let (key, value) = line
                .rsplit_once(' ')
                .unwrap_or_else(|| panic!("line {at}: not a sample: {line:?}"));
            let name = key.split('{').next().unwrap();
            assert!(
                !name.is_empty()
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b':'),
                "line {at}: not a sample: {line:?}"
            );
            let mut base = name;
            if typed.get(&family).map(String::as_str) == Some("histogram") {
                for s in ["_bucket", "_sum", "_count"] {
                    base = base.strip_suffix(s).unwrap_or(base);
                }
            }
            assert_eq!(
                base, family,
                "line {at}: a sample of {name} within family {family}"
            );
            let v: f64 = match value {
                "+Inf" => f64::INFINITY,
                "-Inf" => f64::NEG_INFINITY,
                v => v
                    .parse()
                    .unwrap_or_else(|_| panic!("line {at}: value {v:?}")),
            };
            assert!(
                samples.insert(key.to_owned(), v).is_none(),
                "line {at}: {key} twice"
            );
        }
    }
    samples
}

/// A backend answering every request "from the backend".
fn backend() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { return };
            thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let mut head = Vec::new();
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = s.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 16\r\nConnection: close\r\n\r\nfrom the backend",
                );
            });
        }
    });
    addr
}

/// The status of `GET path` from `addr`, over HTTP/1.1.
fn get(addr: SocketAddr, path: &str) -> u16 {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    resp.get(9..12)
        .and_then(|c| c.parse().ok())
        .unwrap_or_else(|| panic!("GET {path}: {resp:?}"))
}

/// Go's `big.Int.Text(16)` of a positive serial.
fn serial_hex(b: &[u8]) -> String {
    let mut s = String::new();
    for x in b {
        let _ = write!(s, "{x:02x}");
    }
    let t = s.trim_start_matches('0');
    if t.is_empty() { "0".into() } else { t.into() }
}

struct Fixture {
    engine: Arc<akio::Engine>,
    cell: GraphCell,
    listeners: Listeners,
    slot: EngineSlot,
    metrics: Metrics,
    stats: ConfigStats,
    scrape: Scrape,
}

impl Fixture {
    fn compile(
        slot: &EngineSlot,
        metrics: &Metrics,
        log: Option<&AccessLog>,
        json: &str,
        count: bool,
    ) -> Graph {
        let mut config = akit_config::from_json(json, None).unwrap();
        // `metrics` needs `admin`, which akit-config refuses until akit has an Admin API (I26):
        // set as a config with both would have it.
        if count {
            config.metrics = Some(Default::default());
        }
        let registry: Registry<HandlerParams> =
            Registry::new(&[akit_handler::MODULES, akit_proxy::MODULES]).unwrap();
        let env = Env::new(slot.clone())
            .with_metrics(Some(metrics.clone()))
            .with_access_log(log.cloned());
        compile(&config, &registry, &env).unwrap()
    }

    fn start(json: &str, count: bool) -> Fixture {
        Fixture::start_logged(json, count, None)
    }

    fn start_logged(json: &str, count: bool, log: Option<&AccessLog>) -> Fixture {
        let slot = EngineSlot::new();
        let metrics = Metrics::new();
        let cell = GraphCell::new(Fixture::compile(&slot, &metrics, log, json, count));
        let engine = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
        engine.start().unwrap();
        assert!(slot.set(Arc::clone(&engine)));
        let listeners = Listeners::new(Arc::clone(&engine), cell.clone(), &options());
        let stats = ConfigStats::new();
        let scrape = Scrape::new(cell.clone(), Arc::clone(&engine), stats.clone());
        Fixture {
            engine,
            cell,
            listeners,
            slot,
            metrics,
            stats,
            scrape,
        }
    }

    /// Applies `json`, as a reload does, counting it.
    fn load(&mut self, json: &str) {
        let g = Fixture::compile(&self.slot, &self.metrics, None, json, true);
        let r = self.listeners.reload(g);
        self.stats.reloaded(r.is_ok());
        r.unwrap();
        self.cell.collect();
    }

    fn addr(&self, listener: &str) -> SocketAddr {
        self.listeners
            .served()
            .into_iter()
            .find(|s| s.name == listener)
            .unwrap()
            .address
    }

    fn scrape(&self) -> HashMap<String, f64> {
        let text = String::from_utf8(self.scrape.text()).unwrap();
        parse(&text)
    }

    /// Scrapes until every sample in `want` has its value: a request is counted once its
    /// response has gone, maybe after the client has it (fit's `waitMetrics`).
    fn wait(&self, want: &[(String, f64)]) -> HashMap<String, f64> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let got = self.scrape();
            let missing: Vec<String> = want
                .iter()
                .filter(|(k, v)| got.get(k) != Some(v))
                .map(|(k, v)| format!("{k} = {:?}, want {v}", got.get(k)))
                .collect();
            if missing.is_empty() {
                return got;
            }
            assert!(
                Instant::now() < deadline,
                "metrics:\n{}",
                missing.join("\n")
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn stop(self) {
        assert!(akit_listener::shutdown(
            &self.engine,
            Duration::from_secs(5)
        ));
    }
}

#[test]
fn scrape_as_fit() {
    assert_eq!(CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8");
    let live = backend();
    // An address nothing listens on.
    let dead = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let d = dir("metrics-scrape");
    let ca = Ca::new("metrics test CA");
    let (cert, key) = ca.leaf(&d, "a", &["a.example"]);
    let leaf = akio_x509::Certificate::from_der(leaf_der(&cert)).unwrap();
    let doc = |body: &str| {
        format!(
            r#"{{
  "listeners": [
    {{"name": "web", "address": "127.0.0.1:0"}},
    {{"name": "secure", "address": "127.0.0.1:0", "tls": {{"certificates": [{{"cert_file": {cert:?}, "key_file": {key:?}}}]}}}}
  ],
  "upstreams": [{{"name": "back", "addresses": ["{live}", "{dead}"], "max_fails": 100}}],
  "routes": [
    {{"name": "hello", "listeners": ["web"], "match": {{"path": ["/hello"]}}, "handler": {{"module": "respond", "params": {{"body": "{body}"}}}}}},
    {{"listeners": ["web"], "match": {{"path": ["/api/*"]}}, "handler": {{"module": "proxy", "params": {{"upstream": "back"}}}}}}
  ]
}}"#
        )
    };
    let mut f = Fixture::start(&doc("v1"), true);
    let web = f.addr("web");
    for _ in 0..3 {
        assert_eq!(get(web, "/hello"), 200);
    }
    assert_eq!(get(web, "/nowhere"), 404);
    // Round robin, between the backend and the address nothing listens on: each I/O thread
    // keeps its own turn in akit, so requests go until both have been tried.
    let (mut ok, mut failed) = (0u32, 0u32);
    while ok == 0 || failed == 0 {
        match get(web, "/api/a") {
            200 => ok += 1,
            502 => failed += 1,
            other => panic!("/api/a: {other}"),
        }
        assert!(ok + failed < 50, "{ok} answered, {failed} failed");
    }
    let (ok, failed) = (f64::from(ok), f64::from(failed));

    // A reload that keeps the routes keeps their counts; a broken one fails.
    f.load(&doc("v2"));
    assert!(akit_config::from_json(r#"{"listeners": [{"name": "web"}]}"#, None).is_err());
    f.stats.reloaded(false);
    assert_eq!(get(web, "/hello"), 200);

    let s = |k: &str, v: f64| (k.to_owned(), v);
    let got = f.wait(&[
        s(r#"fit_http_requests_total{listener="web",route="hello",code="2xx"}"#, 4.0),
        s(r#"fit_http_requests_total{listener="web",route="",code="4xx"}"#, 1.0),
        s(r##"fit_http_requests_total{listener="web",route="#1",code="2xx"}"##, ok),
        s(r##"fit_http_requests_total{listener="web",route="#1",code="5xx"}"##, failed),
        s(r#"fit_http_request_duration_seconds_count{listener="web",route="hello"}"#, 4.0),
        s(
            r#"fit_http_request_duration_seconds_bucket{listener="web",route="hello",le="+Inf"}"#,
            4.0,
        ),
        s(r#"fit_http_request_duration_seconds_count{listener="secure",route=""}"#, 0.0),
        s(
            r#"fit_upstream_response_duration_seconds_count{upstream="back"}"#,
            ok + failed,
        ),
        s(r#"fit_upstream_requests_in_flight{upstream="back"}"#, 0.0),
        (
            format!(r#"fit_upstream_backend_requests_total{{upstream="back",backend="{live}",result="success"}}"#),
            ok,
        ),
        (
            format!(r#"fit_upstream_backend_requests_total{{upstream="back",backend="{live}",result="failure"}}"#),
            0.0,
        ),
        (
            format!(r#"fit_upstream_backend_requests_total{{upstream="back",backend="{dead}",result="failure"}}"#),
            failed,
        ),
        (
            format!(r#"fit_upstream_backend_requests_in_flight{{upstream="back",backend="{live}"}}"#),
            0.0,
        ),
        (
            format!(r#"fit_upstream_backend_up{{upstream="back",backend="{live}"}}"#),
            1.0,
        ),
        (
            format!(
                r#"fit_tls_certificate_not_after_seconds{{listener="secure",name="a.example",serial="{}"}}"#,
                serial_hex(leaf.serial())
            ),
            leaf.not_after() as f64,
        ),
        s(r#"fit_config_reloads_total{result="success"}"#, 1.0),
        s(r#"fit_config_reloads_total{result="failure"}"#, 1.0),
        s("fit_config_version", 2.0),
        s(r#"fit_reads_paused_total{reason="budget"}"#, 0.0),
    ]);
    for name in [
        "fit_connections_open",
        "fit_connections_refused_total",
        "fit_pending_bytes",
        "fit_config_last_applied_timestamp_seconds",
    ] {
        assert!(got.contains_key(name), "no {name}");
    }
    // akio's engine metrics, where fit has the Go runtime's: per I/O thread.
    for name in ["akio_connections_open{", "akio_connections_accepted_total{"] {
        assert!(got.keys().any(|k| k.starts_with(name)), "no {name}");
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let applied = got["fit_config_last_applied_timestamp_seconds"];
    assert!((now - applied).abs() < 60.0, "last applied {applied}");

    // A route a reload removes is no longer scraped, once the graphs that had it are gone.
    f.load(&doc("v3").replacen(r#""name": "hello""#, r#""name": "greeting""#, 1));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        f.cell.collect();
        let got = f.scrape();
        let old = got.contains_key(
            r#"fit_http_request_duration_seconds_count{listener="web",route="hello"}"#,
        );
        let renamed = got.contains_key(
            r#"fit_http_request_duration_seconds_count{listener="web",route="greeting"}"#,
        );
        if !old && renamed {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "after renaming the route: hello there {old}, greeting there {renamed}"
        );
        thread::sleep(Duration::from_millis(20));
    }
    // The totals never go back: the renamed route's requests are still in them.
    let totals = f.metrics.totals();
    assert_eq!(
        f64::from(u32::try_from(totals[1]).unwrap()),
        4.0 + ok,
        "{totals:?}"
    );
    f.stop();
    let _ = std::fs::remove_dir_all(&d);
}

/// A config without `metrics` counts no requests; the rest is still scraped (fit's
/// `TestMetricsOff`).
#[test]
fn scrape_without_metrics() {
    let f = Fixture::start(
        r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}],
            "routes": [{"handler": {"module": "respond", "params": {"body": "x"}}}]}"#,
        false,
    );
    assert_eq!(get(f.addr("web"), "/"), 200);
    let got = f.scrape();
    for k in got.keys() {
        assert!(!k.starts_with("fit_http_"), "{k} scraped without metrics");
    }
    assert!(got.contains_key("fit_config_version"));
    assert!(f.metrics.routes().is_empty());
    f.stop();
}

/// A buffer the access log writes to.
#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl Write for Sink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// With an access log too, one hook on each response both writes its line and counts it.
#[test]
fn counted_with_the_access_log() {
    let sink = Sink::default();
    let mut opts = akit_observe::Options::new();
    opts.flush_interval = Duration::from_millis(10);
    let log = AccessLog::new(Box::new(sink.clone()), opts).unwrap();
    let json = r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}],
            "routes": [{"name": "hello", "match": {"path": ["/hello"]}, "handler": {"module": "respond"}}],
            "access_log": {}}"#;
    let f = Fixture::start_logged(json, true, Some(&log));
    let web = f.addr("web");
    assert_eq!(get(web, "/hello"), 200);
    assert_eq!(get(web, "/hello"), 200);
    assert_eq!(get(web, "/nowhere"), 404);
    f.wait(&[
        (
            r#"fit_http_requests_total{listener="web",route="hello",code="2xx"}"#.to_owned(),
            2.0,
        ),
        (
            r#"fit_http_requests_total{listener="web",route="",code="4xx"}"#.to_owned(),
            1.0,
        ),
    ]);
    f.stop();
    log.close();
    let lines = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
    assert_eq!(lines.lines().count(), 3, "{lines}");
}

/// What `GET /status` says akit is doing now (I63), as fit's `TestStatus` checks it
/// (`admin/dashboard_test.go`): the listeners served, the upstream's live backend up and its dead
/// one out of rotation for its failures, the certificate the secure listener serves, every
/// request counted, by class, and the reloads.
#[test]
fn status_as_fit() {
    use akit_config::value::{Value, parse};
    let live = backend();
    let dead = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let d = dir("status");
    let ca = Ca::new("status test CA");
    let (cert, key) = ca.leaf(&d, "a", &["a.example"]);
    let leaf = akio_x509::Certificate::from_der(leaf_der(&cert)).unwrap();
    let doc = format!(
        r#"{{
  "listeners": [
    {{"name": "web", "address": "127.0.0.1:0"}},
    {{"name": "secure", "address": "127.0.0.1:0", "tls": {{"certificates": [{{"cert_file": {cert:?}, "key_file": {key:?}}}]}}}}
  ],
  "upstreams": [{{"name": "back", "addresses": ["{live}", "{dead}"], "max_fails": 1, "fail_timeout": "1m"}}],
  "routes": [{{"listeners": ["web"], "handler": {{"module": "proxy", "params": {{"upstream": "back"}}}}}}]
}}"#
    );
    let f = Fixture::start(&doc, true);
    let status = f.scrape.clone().with_listeners(f.listeners.view());
    let read = || {
        let mut b = String::from("{");
        status.write_status(&mut b);
        b.push('}');
        parse(&b).unwrap_or_else(|e| panic!("{b}: {e:?}"))
    };
    let backends = |v: &Value| match v.get("upstreams") {
        Some(Value::Array(us)) => match us[0].get("backends") {
            Some(Value::Array(bs)) => bs.clone(),
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    };
    let num = |v: Option<&Value>| match v {
        Some(Value::Number(n)) => n.parse::<u64>().unwrap(),
        other => panic!("{other:?}"),
    };
    let web = f.addr("web");
    // Requests until the dead backend is tried, fails and is taken out (each I/O thread keeps
    // its own turn of round robin), and the live one answers.
    let mut sent = 0u64;
    loop {
        let code = get(web, "/");
        assert!(code == 200 || code == 502, "{code}");
        sent += 1;
        let bs = backends(&read());
        if bs[1].get("up") == Some(&Value::Bool(false)) && num(bs[0].get("succeeded")) > 0 {
            break;
        }
        assert!(sent < 50, "{bs:?}");
    }

    // A request is counted once its answer is over, which may be after the client has read
    // it: read the status until the last one is counted.
    let deadline = Instant::now() + Duration::from_secs(5);
    let st = loop {
        let st = read();
        let total = st.get("requests").and_then(|r| r.get("total"));
        if num(total) >= sent || Instant::now() > deadline {
            break st;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let keys: Vec<&str> = match &st {
        Value::Object(m) => m.iter().map(|(k, _)| k.as_str()).collect(),
        _ => unreachable!(),
    };
    assert_eq!(
        keys,
        [
            "reloads",
            "listeners",
            "connections",
            "upstreams",
            "certificates",
            "requests"
        ]
    );
    let Some(Value::Array(ls)) = st.get("listeners") else {
        panic!("{st:?}")
    };
    assert_eq!(ls.len(), 2, "{ls:?}");
    assert_eq!(ls[1].get("name"), Some(&Value::String("secure".into())));
    assert_eq!(ls[0].get("address"), Some(&Value::String(web.to_string())));
    assert_eq!(
        st.get("upstreams").and_then(|u| match u {
            Value::Array(us) => us[0].get("name"),
            _ => None,
        }),
        Some(&Value::String("back".into()))
    );
    let bs = backends(&st);
    let (up, gone) = (&bs[0], &bs[1]);
    assert_eq!(up.get("address"), Some(&Value::String(live.to_string())));
    assert_eq!(up.get("up"), Some(&Value::Bool(true)), "{up:?}");
    assert!(
        num(up.get("succeeded")) > 0 && num(up.get("failed")) == 0,
        "{up:?}"
    );
    assert_eq!(gone.get("address"), Some(&Value::String(dead.to_string())));
    assert_eq!(gone.get("up"), Some(&Value::Bool(false)), "{gone:?}");
    assert_eq!(gone.get("probed_down"), None, "{gone:?}");
    assert!(num(gone.get("failed")) > 0, "{gone:?}");

    let Some(Value::Array(certs)) = st.get("certificates") else {
        panic!("{st:?}")
    };
    assert_eq!(certs.len(), 1, "{certs:?}");
    let c = &certs[0];
    assert_eq!(c.get("listener"), Some(&Value::String("secure".into())));
    assert_eq!(
        c.get("names"),
        Some(&Value::Array(vec![Value::String("a.example".into())]))
    );
    assert_eq!(
        c.get("serial"),
        Some(&Value::String(serial_hex(leaf.serial())))
    );
    assert_eq!(
        c.get("issuer"),
        Some(&Value::String("status test CA".into()))
    );
    let time = |k: &str| match c.get(k) {
        Some(Value::String(t)) => t.clone(),
        other => panic!("{k}: {other:?}"),
    };
    // RFC 3339 times sort as they compare.
    assert!(time("not_after") > time("not_before"), "{c:?}");

    // The requests are counted, the 502s of the dead backend's tries as 5xx; the totals add up.
    let rq = st.get("requests").unwrap();
    let Some(Value::Object(classes)) = rq.get("classes") else {
        panic!("{rq:?}")
    };
    let names: Vec<&str> = classes.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(names, ["1xx", "2xx", "3xx", "4xx", "5xx"]);
    assert_eq!(num(rq.get("total")), sent, "{rq:?}");
    assert_eq!(
        num(rq.get("classes").and_then(|c| c.get("2xx")))
            + num(rq.get("classes").and_then(|c| c.get("5xx"))),
        sent,
        "{rq:?}"
    );
    assert_eq!(
        st.get("reloads"),
        parse(r#"{"succeeded":0,"failed":0}"#).ok().as_ref()
    );
    f.stop();
}

/// Without `metrics`, `GET /status` has no requests, as fit's when nothing counts them; without
/// a view of the listeners, none.
#[test]
fn status_without_metrics() {
    let f = Fixture::start(
        r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}],
  "routes": [{"handler": {"module": "respond", "params": {"body": "x"}}}]}"#,
        false,
    );
    let mut b = String::from("{");
    f.scrape.write_status(&mut b);
    b.push('}');
    let v = akit_config::value::parse(&b).unwrap();
    assert!(v.get("requests").is_none(), "{b}");
    assert_eq!(
        v.get("listeners"),
        Some(&akit_config::value::Value::Array(Vec::new()))
    );
    let view = f.listeners.view();
    let mut b = String::from("{");
    f.scrape
        .clone()
        .with_listeners(view.clone())
        .write_status(&mut b);
    assert!(b.contains("\"name\":\"web\""), "{b}");
    // The view follows the listeners a reload serves.
    let mut f = f;
    f.load(
        r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}, {"name": "more", "address": "127.0.0.1:0"}],
  "routes": [{"handler": {"module": "respond", "params": {"body": "x"}}}]}"#,
    );
    let names: Vec<String> = view.get().iter().map(|l| l.name.clone()).collect();
    assert_eq!(names, ["web", "more"]);
    assert_eq!(view.get()[1].address, f.addr("more"));
    f.stop();
}
