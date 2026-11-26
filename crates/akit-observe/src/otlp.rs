//! OpenTelemetry's export (I64), as fit's `observe/otlp.go`: OTLP/HTTP requests of JSON to a
//! receiver (an OpenTelemetry Collector), the metrics [`MetricWriter`]s write as OTLP's, and
//! [`MetricsExporter`], which exports them every interval.
//!
//! [`Otlp`] posts to the receiver from the thread that calls it, a [`Tracer`](crate::Tracer)'s or
//! a [`MetricsExporter`]'s own, never an I/O thread's: through akio's HTTP client, on an engine of
//! its own with one client thread, and waits for the answer, at most the export timeout. A
//! failure is logged once while it lasts, and that exports work again once they do, as fit's.

use std::fmt;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use akio::Engine;
use akio::tls::{Certificate, ClientConfig, PrivateKey, RootCerts};
use akio_http::client::{Client, ClientOptions, ClientRequest, RedirectPolicy};

use crate::json_string;
use crate::metrics::{DURATION_BUCKETS, HistogramSnapshot, TextWriter};

/// The most bytes of a receiver's answer read.
const MAX_ANSWER_BYTES: usize = 64 << 10;

/// The instrumentation scope of what akit exports: fit's `fit`, renamed as its metrics and
/// attributes are (decision 037).
pub const SCOPE: &str = "akit";

/// Where a telemetry exporter's messages go: an export that failed, and exports that work again.
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// The [`Log`] that writes to the `telemetry` logger (I82), at warn: standard error until akit's
/// log is installed.
#[must_use]
pub fn stderr_log() -> Log {
    crate::logger::lines(
        crate::logger::Logger::Telemetry,
        crate::logger::Level::Warn,
        "",
    )
}

/// Posts OTLP/HTTP JSON requests to a receiver: `telemetry`'s endpoint, headers, TLS and timeout,
/// and the resource the data is about. Shared by the tracer and the metrics exporter.
pub struct Otlp {
    endpoint: String,
    headers: Vec<(String, String)>,
    timeout: Duration,
    resource: Vec<u8>,
    engine: Arc<Engine>,
    client: Client,
    log: Log,
}

impl fmt::Debug for Otlp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Otlp")
            .field("endpoint", &self.endpoint)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl Otlp {
    /// The receiver `c` names, the data about the service `c.service_name` at `version`; `log`
    /// hears exports that fail. Reads `headers_file`, `ca_file`, `cert_file` and `key_file`, and
    /// starts the client's engine: control plane only.
    ///
    /// # Errors
    ///
    /// When a file cannot be read or holds what it should not, or the engine cannot start; the
    /// message names the file.
    pub fn new(c: &akit_config::Telemetry, version: &str, log: Log) -> Result<Arc<Otlp>, String> {
        let headers = if c.headers_file.is_empty() {
            Vec::new()
        } else {
            let path = &c.headers_file;
            let data = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            akit_config::parse_headers(&data).map_err(|line| {
                format!("{path}: line {line}: not a header field: \"Name: value\"")
            })?
        };
        let tls = tls(c)?;
        let mut cfg = akio::Config::default();
        cfg.shutdown_timeout = Duration::from_secs(1);
        let engine = Engine::builder()
            .client_threads(1)
            .config(cfg)
            .build()
            .map_err(|e| format!("telemetry's client: {e}"))?;
        engine
            .start()
            .map_err(|e| format!("telemetry's client: {e}"))?;
        let engine = Arc::new(engine);
        let mut opts = ClientOptions::default();
        opts.max_body_bytes = MAX_ANSWER_BYTES;
        opts.max_idle_per_host = 2;
        opts.max_idle = 2;
        let mut client = Client::new(Arc::clone(&engine))
            .with_options(opts)
            .with_redirects(RedirectPolicy::none())
            .with_timeout(c.timeout);
        if let Some(tls) = tls {
            client = client.with_tls(tls);
        }
        let mut resource = Vec::with_capacity(128);
        resource.extend_from_slice(b"{\"attributes\":[");
        append_attr(&mut resource, "service.name", &c.service_name, true);
        append_attr(&mut resource, "service.version", version, false);
        resource.extend_from_slice(b"]}");
        Ok(Arc::new(Otlp {
            endpoint: c.endpoint.clone(),
            headers,
            timeout: c.timeout,
            resource,
            engine,
            client,
            log,
        }))
    }

    /// The resource the data is about, as OTLP JSON.
    #[must_use]
    pub fn resource(&self) -> &[u8] {
        &self.resource
    }

    /// The receiver's URL of `path` (`/v1/traces`, `/v1/metrics`).
    #[must_use]
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.endpoint)
    }

    /// Posts `body` to the receiver's `path` and waits for its answer, at most the export
    /// timeout: an answer other than 2xx is an error. Blocks: never call it on an I/O thread.
    ///
    /// # Errors
    ///
    /// Why the export failed.
    pub fn post(&self, path: &str, body: Vec<u8>) -> Result<(), String> {
        let url = self.url(path);
        let mut req = ClientRequest::post(&url, body)
            .header("content-type", "application/json")
            .timeout(self.timeout);
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }
        let (tx, rx) = mpsc::sync_channel(1);
        // On the client's I/O thread: hands the outcome over, never blocks (room for one).
        self.client.request(req, move |r| {
            let _ = tx.try_send(r);
        });
        // The client's own timeout ends the request; this one bounds the wait should it not.
        let wait = self.timeout.saturating_add(Duration::from_secs(1));
        match rx.recv_timeout(wait) {
            Ok(Ok(r)) if (200..300).contains(&r.status) => Ok(()),
            Ok(Ok(r)) => {
                let text = String::from_utf8_lossy(&r.body);
                let text = text.trim();
                let mut msg = format!("POST {url}: {}", r.status);
                if !text.is_empty() {
                    let cut = text
                        .char_indices()
                        .nth(200)
                        .map_or(text, |(i, _)| text.get(..i).unwrap_or(text));
                    let _ = write!(msg, ": {cut}");
                }
                Err(msg)
            }
            Ok(Err(e)) => Err(format!("POST {url}: {e}")),
            Err(RecvTimeoutError::Timeout) => Err(format!("POST {url}: timed out")),
            Err(RecvTimeoutError::Disconnected) => {
                Err(format!("POST {url}: the request was dropped"))
            }
        }
    }
}

impl Drop for Otlp {
    fn drop(&mut self) {
        crate::stop_client_engine(&self.engine);
    }
}

/// The client's TLS: `ca_file`'s CAs in place of the system's, `cert_file` and `key_file` sent
/// when the receiver asks for a certificate; `None` when `c` names no file.
fn tls(c: &akit_config::Telemetry) -> Result<Option<ClientConfig>, String> {
    if c.ca_file.is_empty() && c.cert_file.is_empty() {
        return Ok(None);
    }
    let read = |path: &str| std::fs::read(path).map_err(|e| format!("{path}: {e}"));
    let roots = if c.ca_file.is_empty() {
        RootCerts::system().map_err(|e| format!("the system's CAs: {e}"))?
    } else {
        let pem = read(&c.ca_file)?;
        let mut r = RootCerts::empty();
        if !matches!(r.add_pem(&pem), Ok(n) if n > 0) {
            return Err(format!("{} holds no PEM certificate", c.ca_file));
        }
        r
    };
    let mut b = ClientConfig::builder().roots(roots);
    if !c.cert_file.is_empty() {
        let (cert, key) = (&c.cert_file, &c.key_file);
        let chain = Certificate::from_pem(&read(cert)?).map_err(|e| format!("{cert}: {e}"))?;
        if chain.is_empty() {
            return Err(format!("{cert}: no PEM certificate found"));
        }
        let k = PrivateKey::from_pem(&read(key)?).map_err(|e| format!("{key}: {e}"))?;
        b = b.client_cert(chain, k);
    }
    b.build().map(Some).map_err(|e| e.to_string())
}

/// Logs an export's error when it is not the one logged last, and that exports work again after
/// one failed: fit's `exporter.report`. One for each signal.
#[derive(Debug, Default)]
pub(crate) struct Reporter {
    last: Mutex<String>,
}

impl Reporter {
    pub(crate) fn report(&self, log: &Log, what: &str, r: &Result<(), String>) {
        let msg = match r {
            Ok(()) => "",
            Err(e) => e.as_str(),
        };
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        if *last == msg {
            return;
        }
        match r {
            Err(e) => log(&format!("telemetry: exporting {what}: {e}")),
            Ok(()) => log(&format!("telemetry: exporting {what} works again")),
        }
        msg.clone_into(&mut last);
    }
}

impl Otlp {
    pub(crate) fn log(&self) -> &Log {
        &self.log
    }
}

/// Appends an attribute of a string value, after a comma unless it is the `first`.
pub(crate) fn append_attr(b: &mut Vec<u8>, key: &str, value: &str, first: bool) {
    append_attr_bytes(b, key, value.as_bytes(), first);
}

/// [`append_attr`] of a value that may not be UTF-8: each byte that is not is written U+FFFD.
pub(crate) fn append_attr_bytes(b: &mut Vec<u8>, key: &str, value: &[u8], first: bool) {
    if !first {
        b.push(b',');
    }
    b.extend_from_slice(b"{\"key\":");
    json_string(b, key.as_bytes());
    b.extend_from_slice(b",\"value\":{\"stringValue\":");
    json_string(b, value);
    b.extend_from_slice(b"}}");
}

/// Appends an attribute of an integer value, after a comma.
pub(crate) fn append_int_attr(b: &mut Vec<u8>, key: &str, v: i64) {
    b.extend_from_slice(b",{\"key\":");
    json_string(b, key.as_bytes());
    let _ = write!(Text(b), ",\"value\":{{\"intValue\":\"{v}\"}}}}");
}

/// A `Vec<u8>` written to with `write!`.
pub(crate) struct Text<'a>(pub(crate) &'a mut Vec<u8>);

impl fmt::Write for Text<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

/// Nanoseconds since the Unix epoch of `t`; 0 before it.
pub(crate) fn unix_nanos(t: SystemTime) -> u128 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos())
}

/// What takes metrics: a family begun with [`family`](Self::family), then its samples.
/// [`TextWriter`] writes them in Prometheus's text format, and [`OtlpWriter`] as an OTLP export
/// request: fit's `observe.MetricWriter`.
pub trait MetricWriter {
    /// Begins a family: `typ` is `counter`, `gauge` or `histogram`.
    fn family(&mut self, name: &str, typ: &str, help: &str);
    /// A sample of the family begun last, with `labels`, names and values.
    fn sample(&mut self, name: &str, value: f64, labels: &[(&str, &str)]);
    /// A histogram of durations of the family begun last.
    fn histogram(&mut self, name: &str, h: &HistogramSnapshot, labels: &[(&str, &str)]);
    /// A counter's or gauge's sample of an integer.
    #[allow(clippy::cast_precision_loss)]
    fn sample_u64(&mut self, name: &str, value: u64, labels: &[(&str, &str)]) {
        self.sample(name, value as f64, labels);
    }
}

impl MetricWriter for TextWriter {
    fn family(&mut self, name: &str, typ: &str, help: &str) {
        TextWriter::family(self, name, typ, help);
    }

    fn sample(&mut self, name: &str, value: f64, labels: &[(&str, &str)]) {
        TextWriter::sample(self, name, value, labels);
    }

    fn histogram(&mut self, name: &str, h: &HistogramSnapshot, labels: &[(&str, &str)]) {
        TextWriter::histogram(self, name, h, labels);
    }
}

/// What the family begun last is, in OTLP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// A counter: a cumulative, monotonic sum.
    Sum,
    Gauge,
    /// A histogram of seconds: an explicit-bucket histogram.
    Histogram,
}

/// Gathers metrics as OTLP's, as fit's `OTLPWriter`: a counter as a cumulative, monotonic sum, a
/// gauge as a gauge, and a histogram of seconds as an explicit-bucket histogram. A sample with
/// no family before it, or whose value JSON cannot carry (NaN, an infinity), is passed over, as
/// is a histogram of a family that is not one.
#[derive(Debug)]
pub struct OtlpWriter {
    now: String,
    start: String,
    /// The metrics, each a JSON object, comma separated; the last one still open.
    out: Vec<u8>,
    cur: Option<Kind>,
    points: usize,
    metrics: usize,
}

impl OtlpWriter {
    /// A writer of metrics read at `now`, the cumulative ones counted since `start`.
    #[must_use]
    pub fn new(now: SystemTime, start: SystemTime) -> OtlpWriter {
        OtlpWriter {
            now: unix_nanos(now).to_string(),
            start: unix_nanos(start).to_string(),
            out: Vec::new(),
            cur: None,
            points: 0,
            metrics: 0,
        }
    }

    /// How many metrics were begun.
    #[must_use]
    pub fn metrics(&self) -> usize {
        self.metrics
    }

    fn close(&mut self) {
        match self.cur.take() {
            Some(Kind::Sum) => self
                .out
                .extend_from_slice(b"],\"aggregationTemporality\":2,\"isMonotonic\":true}}"),
            Some(Kind::Gauge) => self.out.extend_from_slice(b"]}}"),
            Some(Kind::Histogram) => self
                .out
                .extend_from_slice(b"],\"aggregationTemporality\":2}}"),
            None => {}
        }
    }

    fn point_start(&mut self, labels: &[(&str, &str)]) {
        if self.points > 0 {
            self.out.push(b',');
        }
        self.points = self.points.saturating_add(1);
        self.out.push(b'{');
        if !labels.is_empty() {
            self.out.extend_from_slice(b"\"attributes\":[");
            for (i, (k, v)) in labels.iter().enumerate() {
                append_attr(&mut self.out, k, v, i == 0);
            }
            self.out.extend_from_slice(b"],");
        }
    }

    /// The export request of the metrics written, about `resource` (OTLP JSON).
    #[must_use]
    pub fn finish(mut self, resource: &[u8]) -> Vec<u8> {
        self.close();
        let mut b = Vec::with_capacity(
            self.out
                .len()
                .saturating_add(resource.len())
                .saturating_add(96),
        );
        b.extend_from_slice(b"{\"resourceMetrics\":[{\"resource\":");
        b.extend_from_slice(resource);
        b.extend_from_slice(b",\"scopeMetrics\":[{\"scope\":{\"name\":");
        json_string(&mut b, SCOPE.as_bytes());
        b.extend_from_slice(b"},\"metrics\":[");
        b.extend_from_slice(&self.out);
        b.extend_from_slice(b"]}]}]}");
        b
    }
}

impl MetricWriter for OtlpWriter {
    fn family(&mut self, name: &str, typ: &str, help: &str) {
        self.close();
        if self.metrics > 0 {
            self.out.push(b',');
        }
        self.metrics = self.metrics.saturating_add(1);
        self.points = 0;
        self.out.extend_from_slice(b"{\"name\":");
        json_string(&mut self.out, name.as_bytes());
        if !help.is_empty() {
            self.out.extend_from_slice(b",\"description\":");
            json_string(&mut self.out, help.as_bytes());
        }
        let kind = match typ {
            "counter" => Kind::Sum,
            "histogram" => Kind::Histogram,
            _ => Kind::Gauge,
        };
        self.out.extend_from_slice(match kind {
            Kind::Sum => b",\"sum\":{\"dataPoints\":[".as_slice(),
            Kind::Gauge => b",\"gauge\":{\"dataPoints\":[".as_slice(),
            Kind::Histogram => b",\"unit\":\"s\",\"histogram\":{\"dataPoints\":[".as_slice(),
        });
        self.cur = Some(kind);
    }

    fn sample(&mut self, _: &str, value: f64, labels: &[(&str, &str)]) {
        let Some(kind @ (Kind::Sum | Kind::Gauge)) = self.cur else {
            return;
        };
        if !value.is_finite() {
            return;
        }
        self.point_start(labels);
        if kind == Kind::Sum {
            let _ = write!(
                Text(&mut self.out),
                "\"startTimeUnixNano\":\"{}\",",
                self.start
            );
        }
        let _ = write!(
            Text(&mut self.out),
            "\"timeUnixNano\":\"{}\",\"asDouble\":{value}}}",
            self.now
        );
    }

    fn histogram(&mut self, _: &str, h: &HistogramSnapshot, labels: &[(&str, &str)]) {
        if self.cur != Some(Kind::Histogram) {
            return;
        }
        self.point_start(labels);
        let sum = h.sum_seconds();
        let _ = write!(
            Text(&mut self.out),
            "\"startTimeUnixNano\":\"{}\",\"timeUnixNano\":\"{}\",\"count\":\"{}\",\"sum\":{},\"bucketCounts\":[",
            self.start,
            self.now,
            h.count,
            if sum.is_finite() { sum } else { 0.0 },
        );
        // The snapshot's buckets are cumulative; OTLP's each count their own.
        let mut prev = 0u64;
        for c in h.buckets {
            let _ = write!(Text(&mut self.out), "\"{}\",", c.saturating_sub(prev));
            prev = c;
        }
        let _ = write!(
            Text(&mut self.out),
            "\"{}\"],\"explicitBounds\":[",
            h.count.saturating_sub(prev)
        );
        for (i, b) in DURATION_BUCKETS.iter().enumerate() {
            if i > 0 {
                self.out.push(b',');
            }
            let _ = write!(Text(&mut self.out), "{b}");
        }
        self.out.extend_from_slice(b"]}");
    }
}

/// What a [`MetricsExporter`] calls for the metrics, every interval: the scrape's.
pub type WriteMetrics = Box<dyn FnMut(&mut dyn MetricWriter) + Send>;

enum Msg {
    /// Export now, and say when done.
    Export(SyncSender<()>),
    /// Export once more, then stop.
    Stop,
}

/// Exports metrics over OTLP/HTTP every interval, from a thread of its own, as fit's
/// `MetricsExporter`. [`close`](Self::close) exports once more and stops it.
pub struct MetricsExporter {
    tx: SyncSender<Msg>,
    stats: Arc<ExportStats>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl fmt::Debug for MetricsExporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MetricsExporter")
            .field("exported", &self.exported())
            .field("failed", &self.failed())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
struct ExportStats {
    exported: AtomicU64,
    failed: AtomicU64,
}

impl MetricsExporter {
    /// Starts exporting what `write` writes, through `otlp`, every `interval` from now.
    ///
    /// # Errors
    ///
    /// When the thread cannot be started.
    pub fn new(
        otlp: Arc<Otlp>,
        interval: Duration,
        mut write: WriteMetrics,
    ) -> std::io::Result<MetricsExporter> {
        let (tx, rx) = mpsc::sync_channel::<Msg>(1);
        let stats = Arc::new(ExportStats::default());
        let s = Arc::clone(&stats);
        let start = SystemTime::now();
        let thread = std::thread::Builder::new()
            .name("akit-otlp-metrics".into())
            .spawn(move || {
                let reporter = Reporter::default();
                let mut export = || {
                    let mut w = OtlpWriter::new(SystemTime::now(), start);
                    write(&mut w);
                    let r = otlp.post("/v1/metrics", w.finish(otlp.resource()));
                    reporter.report(otlp.log(), "metrics", &r);
                    let n = if r.is_ok() { &s.exported } else { &s.failed };
                    n.fetch_add(1, Ordering::Relaxed);
                };
                let mut next = Instant::now().checked_add(interval);
                loop {
                    let wait = next.map_or(Duration::MAX, |n| {
                        n.saturating_duration_since(Instant::now())
                    });
                    match rx.recv_timeout(wait) {
                        Ok(Msg::Export(done)) => {
                            export();
                            let _ = done.try_send(());
                        }
                        Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => {
                            export();
                            return;
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            export();
                            next = Instant::now().checked_add(interval);
                        }
                    }
                }
            })?;
        Ok(MetricsExporter {
            tx,
            stats,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Reads the metrics and exports them now, and returns once that is done. Blocks.
    pub fn export(&self) {
        let (tx, rx) = mpsc::sync_channel(1);
        if self.tx.send(Msg::Export(tx)).is_ok() {
            let _ = rx.recv();
        }
    }

    /// The exports that succeeded.
    #[must_use]
    pub fn exported(&self) -> u64 {
        self.stats.exported.load(Ordering::Relaxed)
    }

    /// The exports that failed.
    #[must_use]
    pub fn failed(&self) -> u64 {
        self.stats.failed.load(Ordering::Relaxed)
    }

    /// Exports once more and stops. Blocks until then; later calls do nothing.
    pub fn close(&self) {
        let thread = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(t) = thread {
            let _ = self.tx.send(Msg::Stop);
            let _ = t.join();
        }
    }
}

impl Drop for MetricsExporter {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::time::SystemTime;

    use super::{MetricWriter, OtlpWriter};
    use crate::metrics::HistogramSnapshot;

    /// fit's `TestOTLPWriterIgnores`: samples with no family before them, or whose value OTLP
    /// JSON cannot carry, and histograms of a family that is not one.
    #[test]
    fn otlp_writer_ignores() {
        let mut w = OtlpWriter::new(SystemTime::now(), SystemTime::now());
        w.sample("orphan", 1.0, &[]);
        w.histogram("orphan", &HistogramSnapshot::default(), &[]);
        w.family("akit_g", "gauge", "G.");
        w.sample("akit_g", f64::NAN, &[]);
        w.sample("akit_g", f64::INFINITY, &[]);
        w.histogram("akit_g", &HistogramSnapshot::default(), &[]);
        assert_eq!(w.metrics(), 1);
        let body = String::from_utf8(w.finish(b"{}")).unwrap();
        assert!(body.contains(r#""gauge":{"dataPoints":[]}"#), "{body}");
        assert!(!body.contains("orphan"), "{body}");
    }
}
