//! The scrape (I37): every metric akit has, in Prometheus's text format, with fit's names,
//! labels and help (fit's `admin.Configs.WriteMetrics`), then akio's engine metrics.
//!
//! fit serves it at the Admin API's `GET /metrics`, with [`CONTENT_TYPE`]; akit's Admin API
//! (I26) serves [`Scrape::write`] there. What it writes, in fit's order:
//!
//! - with `metrics` in the running config: `fit_http_requests_total` and
//!   `fit_http_request_duration_seconds`, by listener and route, of the routes the graphs in use
//!   have;
//! - the running graph's upstreams: `fit_upstream_requests_in_flight`,
//!   `fit_upstream_response_duration_seconds`, and by backend
//!   `fit_upstream_backend_requests_total`, `fit_upstream_backend_requests_in_flight` and
//!   `fit_upstream_backend_up`;
//! - `fit_tls_certificate_not_after_seconds`, by listener, name and serial;
//! - the listeners' connections: `fit_connections_open`, `fit_connections_refused_total`,
//!   `fit_pending_bytes`, `fit_reads_paused_total`, from akio's engine telemetry
//!   (`fit_connections_open` counts the upstream connections dialed on the engine too, until
//!   akio can tell them apart: akio API request A30; `reason="budget"` is 0, akit having no
//!   `limits.max_pending_bytes` yet);
//! - `fit_config_reloads_total`, `fit_config_version` and
//!   `fit_config_last_applied_timestamp_seconds`;
//! - `fit_trace_spans_total`, by what became of the spans, with `telemetry.traces` (I64);
//! - `fit_access_log_dropped_total`, with an access log;
//! - akio's engine metrics (`akio_*`), where fit has a few of the Go runtime's (`go_*`).
//!
//! Scraping sums the counters and takes the registry's lock for a moment: control plane work, a
//! few microseconds a route, never on the request path.

use std::fmt;
use std::sync::Arc;

use akio::Engine;
use akit_observe::metrics::{ConfigStats, STATUS_CLASSES, TextWriter};
use akit_observe::{AccessLog, MetricWriter, Tracer};
use akit_runtime::GraphCell;

pub use akit_observe::metrics::CONTENT_TYPE;

/// What a scrape reads: the graphs, the engine, the configs applied and the access log. Cheap to
/// clone; safe to use from any thread.
#[derive(Clone)]
pub struct Scrape {
    cell: GraphCell,
    engine: Arc<Engine>,
    config: ConfigStats,
    access_log: Option<AccessLog>,
    tracer: Option<Tracer>,
}

impl fmt::Debug for Scrape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scrape").finish_non_exhaustive()
    }
}

impl Scrape {
    /// The scrape of the graphs `cell` publishes, served by `engine`, whose configs `config`
    /// counts.
    #[must_use]
    pub fn new(cell: GraphCell, engine: Arc<Engine>, config: ConfigStats) -> Scrape {
        Scrape {
            cell,
            engine,
            config,
            access_log: None,
            tracer: None,
        }
    }

    /// The same, with the process's tracer (I64), whose spans it counts.
    #[must_use]
    pub fn with_tracer(mut self, tracer: Option<Tracer>) -> Scrape {
        self.tracer = tracer;
        self
    }

    /// The same, with the process's access log, whose dropped lines it counts.
    #[must_use]
    pub fn with_access_log(mut self, log: Option<AccessLog>) -> Scrape {
        self.access_log = log;
        self
    }

    /// Every metric, in Prometheus's text format ([`CONTENT_TYPE`]).
    #[must_use]
    pub fn text(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }

    /// Appends every metric to `out`, in Prometheus's text format ([`CONTENT_TYPE`]).
    pub fn write(&self, out: &mut Vec<u8>) {
        let mut w = TextWriter::new(std::mem::take(out));
        self.write_metrics(&mut w);
        *out = w.out;
        // akio's engine: its I/O threads' counters, connections and logical pool.
        out.extend_from_slice(self.engine.telemetry().render_prometheus().as_bytes());
    }

    /// Writes akit's metrics (all but akio's engine's, which come in Prometheus's text alone) to
    /// `w`: the scrape's, and OpenTelemetry's export of them (I64).
    #[allow(clippy::cast_precision_loss, clippy::too_many_lines)]
    pub fn write_metrics(&self, w: &mut dyn MetricWriter) {
        self.cell.with(|g| {
            if let Some(m) = g.metrics() {
                let all = m.routes();
                w.family(
                    "fit_http_requests_total",
                    "counter",
                    "Requests answered, by listener, route and the class of their status.",
                );
                for r in &all {
                    for (class, n) in STATUS_CLASSES.iter().zip(r.codes) {
                        if n > 0 {
                            w.sample_u64(
                                "fit_http_requests_total",
                                n,
                                &[("listener", &r.listener), ("route", &r.route), ("code", class)],
                            );
                        }
                    }
                }
                w.family(
                    "fit_http_request_duration_seconds",
                    "histogram",
                    "How long requests took, from when the handler was called to when the response was handed to the connection.",
                );
                for r in &all {
                    w.histogram(
                        "fit_http_request_duration_seconds",
                        &r.duration,
                        &[("listener", &r.listener), ("route", &r.route)],
                    );
                }
            }

            let pools = g.upstreams();
            w.family(
                "fit_upstream_requests_in_flight",
                "gauge",
                "Requests to an upstream that are not over, body included.",
            );
            for p in pools {
                w.sample_u64(
                    "fit_upstream_requests_in_flight",
                    p.in_flight() as u64,
                    &[("upstream", p.name())],
                );
            }
            w.family(
                "fit_upstream_response_duration_seconds",
                "histogram",
                "How long an upstream took to send a response's header, or to fail without one.",
            );
            for p in pools {
                w.histogram(
                    "fit_upstream_response_duration_seconds",
                    &p.response_times(),
                    &[("upstream", p.name())],
                );
            }
            w.family(
                "fit_upstream_backend_requests_total",
                "counter",
                "Requests to a backend that are over, by whether the backend failed them.",
            );
            for p in pools {
                for (i, addr) in p.addresses().enumerate() {
                    let (done, failed) = p.backend_requests(i);
                    let labels = |result| [("upstream", p.name()), ("backend", addr), ("result", result)];
                    w.sample_u64(
                        "fit_upstream_backend_requests_total",
                        done.saturating_sub(failed),
                        &labels("success"),
                    );
                    w.sample_u64(
                        "fit_upstream_backend_requests_total",
                        failed,
                        &labels("failure"),
                    );
                }
            }
            w.family(
                "fit_upstream_backend_requests_in_flight",
                "gauge",
                "Requests to a backend that are not over, body included.",
            );
            for p in pools {
                for (i, addr) in p.addresses().enumerate() {
                    w.sample_u64(
                        "fit_upstream_backend_requests_in_flight",
                        p.backend_active(i) as u64,
                        &[("upstream", p.name()), ("backend", addr)],
                    );
                }
            }
            w.family(
                "fit_upstream_backend_up",
                "gauge",
                "Whether a backend is in rotation (1) or out of it, for failing requests or its health check (0).",
            );
            for p in pools {
                for (i, addr) in p.addresses().enumerate() {
                    w.sample(
                        "fit_upstream_backend_up",
                        if p.is_down(i) { 0.0 } else { 1.0 },
                        &[("upstream", p.name()), ("backend", addr)],
                    );
                }
            }

            w.family(
                "fit_tls_certificate_not_after_seconds",
                "gauge",
                "When a certificate a listener serves expires, in seconds since the Unix epoch.",
            );
            for (i, l) in g.listeners().iter().enumerate() {
                for c in g.certificates(i) {
                    w.sample(
                        "fit_tls_certificate_not_after_seconds",
                        c.not_after as f64,
                        &[("listener", &l.name), ("name", &c.name), ("serial", &c.serial)],
                    );
                }
            }
        });

        let tel = self.engine.telemetry().metrics();
        w.family(
            "fit_connections_open",
            "gauge",
            "Client connections open on the listeners.",
        );
        w.sample_u64("fit_connections_open", tel.open_connections as u64, &[]);
        w.family(
            "fit_connections_refused_total",
            "counter",
            "Client connections closed as they were accepted, for limits.max_conns.",
        );
        w.sample_u64("fit_connections_refused_total", tel.total.rejected, &[]);
        w.family(
            "fit_pending_bytes",
            "gauge",
            "Bytes queued for clients that their connections have not taken, which limits.max_pending_bytes bounds.",
        );
        w.sample_u64("fit_pending_bytes", tel.total.queued_bytes, &[]);
        w.family(
            "fit_reads_paused_total",
            "counter",
            "Times a connection's reads were paused: for its own queued output (watermark), or for limits.max_pending_bytes (budget).",
        );
        w.sample_u64(
            "fit_reads_paused_total",
            tel.total.backpressure,
            &[("reason", "watermark")],
        );
        // akit has no budget of pending bytes across connections yet (limits.max_pending_bytes).
        w.sample_u64("fit_reads_paused_total", 0, &[("reason", "budget")]);

        let (tried, failed) = self.config.reloads();
        w.family(
            "fit_config_reloads_total",
            "counter",
            "Configs applied after the first, by SIGHUP, the file watcher, the Admin API or a change of certificate files, by result.",
        );
        w.sample_u64(
            "fit_config_reloads_total",
            tried.saturating_sub(failed),
            &[("result", "success")],
        );
        w.sample_u64("fit_config_reloads_total", failed, &[("result", "failure")]);
        w.family(
            "fit_config_version",
            "gauge",
            "The version of the running config.",
        );
        w.sample_u64("fit_config_version", self.config.version(), &[]);
        w.family(
            "fit_config_last_applied_timestamp_seconds",
            "gauge",
            "When the running config was applied, in seconds since the Unix epoch.",
        );
        w.sample(
            "fit_config_last_applied_timestamp_seconds",
            self.config.applied_ms() as f64 / 1e3,
            &[],
        );

        if let Some(t) = &self.tracer {
            w.family(
                "fit_trace_spans_total",
                "counter",
                "Spans of traced requests, by what became of them: exported, failed (in an export the receiver did not take) or dropped (for want of room while they waited).",
            );
            w.sample_u64(
                "fit_trace_spans_total",
                t.exported(),
                &[("result", "exported")],
            );
            w.sample_u64("fit_trace_spans_total", t.failed(), &[("result", "failed")]);
            w.sample_u64(
                "fit_trace_spans_total",
                t.dropped(),
                &[("result", "dropped")],
            );
        }
        if let Some(log) = &self.access_log {
            w.family(
                "fit_access_log_dropped_total",
                "counter",
                "Access log lines dropped because the log's buffer was full.",
            );
            w.sample_u64("fit_access_log_dropped_total", log.dropped(), &[]);
        }
    }
}
