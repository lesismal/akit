# 007: OpenTelemetry: fit's tracer, exported through akio's HTTP client

[English](007-opentelemetry.md) | [简体中文](007-opentelemetry.zh-CN.md)

Status: accepted in I64 (2026-11-24).

## Context

I64 adds fit's `telemetry`: a server span for each request it samples, by W3C `traceparent` and
`sample_ratio`, its `traceparent` sent on to the upstream, and the spans and the metrics `/metrics`
serves exported over OTLP/HTTP as JSON. fit builds it without the OpenTelemetry SDK
([fit's decision 015](https://github.com/lesismal/fit/blob/main/docs/decisions/015-opentelemetry-without-the-sdk.md)):
a request it does not sample costs a field lookup; a sampled one's span is written as OTLP JSON
into a bounded, sharded buffer that a goroutine exports.

akio already has a tracing stack: akio-log's spans, akio-http's `http.server.request` span per
request (parent taken from `traceparent`), and akio-export, which installs a process-wide tracer
and pushes spans, logs and metrics over OTLP (protobuf or JSON, HTTP or gRPC) from a thread of its
own. Rule 21 asks that what akio cannot do be added to akio rather than worked around, so the
question was whether akit should build on that stack.

## Decision

- **akit ports fit's tracer, in `akit-observe` (`trace`), and does not turn on akio's request
  spans or akio-export's tracer.** akio's stack does what it is for, but not what fit's
  `telemetry` is:
  - akio-http's request span is created for every request while spans are on, sampled or not (a
    box and its ids), where fit's unsampled request costs a field lookup and nothing else (rule
    13);
  - it ends when the response's head is sent, without the body's size, the route's name as akit
    names it, the listener or the upstream's address, which fit's span carries
    (`fit.listener`, `fit.route`, `fit.upstream.address`, `http.response.body.size`) and which
    only akit knows;
  - akio-export's queue is bounded in spans and batches, retried with backoff; fit's is bounded
    in bytes (`max_queue_bytes`), exported every `interval` or at 256 KiB, a failed export counted
    and dropped, and its counts are fit's `fit_trace_spans_total`;
  - akio-export's tracer is process-wide and installed once, where akit's is a value that graphs
    share and tests make as many of as they like.

  None of this is a gap in akio's engine: it is the gateway's own protocol logic, like the access
  log's formats (I21), so nothing is requested of akio.
- **The export goes through akio's HTTP client** (`akio_http::client::Client`), on an akio engine
  of its own with one client thread (`akit_observe::Otlp`), as the ACME client's does (I31):
  `headers_file`, `ca_file`, `cert_file`, `key_file` and `timeout` as fit's. A tracer's and a
  metrics exporter's thread each wait there for the answer, never an I/O thread (rule 1).
- **On the request path**: the listener asks the graph's tracer per request (one field lookup;
  a request with two `traceparent` fields is taken as having none, as fit's); a sampled request
  gets its `traceparent` as a context local the proxy sends in place of the client's, and its
  span is ended by the response hook the access log and the metrics already share, encoded into
  a buffer of its thread's own and copied into a shard under a lock held for the copy.
- **Metrics** go out as fit's: the scrape's families (`Scrape::write_metrics`) through a
  `MetricWriter`, written as Prometheus text (`TextWriter`) or OTLP JSON (`OtlpWriter`): counters
  as cumulative monotonic sums, gauges, histograms with explicit bounds. akio's own engine
  metrics stay in the Prometheus scrape only, as fit's Go runtime metrics are.

## Consequences

- akit's spans and metrics are fit's, by name and attribute: dashboards and collectors set up for
  fit read akit's the same (the scope is `fit`, `service.name` defaults to `fit`).
- akio's request spans stay available to anyone who turns them on with akio-log; akit does not,
  so a request is not traced twice.
- The export's protocol is JSON over HTTP only, as fit's; gRPC and protobuf, which akio-export
  has, are not offered.
- A sampled request allocates its `traceparent` and its share of the response hook; an unsampled
  one nothing more than before (the hook is still there for the access log or metrics, if any).
