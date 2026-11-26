# 032: The limits taken at start on akio's engine budget, and h2c alone

[English](032-limits-on-akio-budget.md) | [简体中文](032-limits-on-akio-budget.zh-CN.md)

Status: accepted in I79b (2026-11-26).

## Context

[Decision 028](028-engine-on-akio.md) read `limits.max_conns` and `limits.max_write_queue_bytes`
onto akio's engine and left three things to akio: `limits.max_pending_bytes`, a budget of the
bytes waiting for every client together (akio API request A214); `limits.max_conns` counting the
client connections alone, where akio's `max_connections` counted the upstream connections akit
dials on the same engine too (A215); and a listener of `h2c` alone, which akio's HTTP server could
not serve without HTTP/1.1 (A168). fit-examples' `limits`, `proxy` and `grpc` examples were refused
for them. akio built all three (its ADR-172, ADR-173 and ADR-174).

## Decision

1. **`limits.max_pending_bytes`** is akio's `Config::max_pending_bytes`, set when the listeners'
   engine is built, as fit's fib takes it at start; a reload that changes it is warned of. akit
   reads it as fit does: bytes, or a share of the memory (`"25%"`, kept as hundredths of a percent,
   as fit rounds it), half the memory when left out or 0.
2. **The memory a share is of** is read by akit, as fit's `internal/sysmem`: on Linux,
   `/proc/meminfo`'s `MemTotal`, or the cgroup's `memory.max` (v2) or `memory.limit_in_bytes` (v1)
   where lower, read once when the engine is built, with `std::fs` (never on an I/O thread).
   Elsewhere akit reads none (macOS's `sysctl` needs `unsafe`, which akit has not and akio's
   request did not take on), and a share is fit's fallback, 256 MiB. No dependency is added.
3. **`limits.max_conns`** is akio's `Config::max_accepted_connections`; akio's `max_connections`,
   which counts every connection, is left off, as it was before I79.
4. **A listener of `h2c` alone** sets akio-http's `HttpConfig::http1` to `false`, and on a port
   shared with `tcp` or `socks` its HTTP is akio-mux's `Rule::Http2` (the preface alone), as fit's
   mux matches `HTTP2()`. A client that sends anything else gets akio's `GOAWAY` and is closed
   (fit closes it with nothing), or, beside `tcp`, goes to the listener's service, as in fit.
5. **Metrics.** `akit_reads_paused_total{reason="budget"}` is akio's `budget_pauses`.

## Consequences

- No fit-examples config outside the After-1.0 tracks is refused (the gate's report).
- The budget is soft by what akio queues between two of its sums, about a millisecond; each
  connection is still bounded by `max_write_queue_bytes`.
- A budget pauses the upstream connections' reads too, which is what holds a proxy's queues: fit's
  fib pauses every connection alike.
- On macOS the default budget is 256 MiB, not half the memory: macOS is a development platform,
  and a config that sets the field in bytes gets it.
- Measuring is the benchmark tasks' (rule 30): the proxy and fixed scenarios with the default
  budget against none, and many slow clients against fit.
