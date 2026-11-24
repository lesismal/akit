# akit development plan

[English](dev-plan.md) | [简体中文](dev-plan.zh-CN.md)

This plan breaks the [roadmap](roadmap.md) into small iterations. Each iteration is one piece of
work that fits in one working session and one commit or pull request: small enough that the code it
touches, its docs and its tests fit in one context.

- [How to work an iteration](#how-to-work-an-iteration)
- [Workspace layout](#workspace-layout)
- [Index](#index)
- [Benchmark and optimization status](#benchmark-and-optimization-status)
- [M0, foundations](#m0-foundations)
- [M1, the proxy core](#m1-the-proxy-core)
- [M2, dynamic config and TLS](#m2-dynamic-config-and-tls)
- [M3, gateway features](#m3-gateway-features)
- [M4, platform integration](#m4-platform-integration)
- [M5, caching and extensions](#m5-caching-and-extensions)
- [1.0](#10)
- [After 1.0](#after-10)

## How to work an iteration

**Pick one.** [Progress](progress.md) says where the work stands and which iteration is next.
Otherwise take the first iteration in the [index](#index) whose status is `todo` and whose
dependencies are all `done`. Iterations with no dependency between them may run in parallel, in
separate branches.

**Read only what it needs.** A session starts from:

1. [progress](progress.md), this file's index and the iteration's own section;
2. the [design principles](roadmap.md#design-principles) and [the rules](rules.md);
3. the crates listed under **Touches**, the akio crates and docs under **Read**, and the fit
   packages under **fit**: fit's code, tests and docs are the specification of what to build.

**Port, do not reinvent.** fit has already met each question an iteration raises. Its decision
notes (`fit/docs/decisions`), its tests and its config reference say what akit should do; akit
changes the *how* (akio's callbacks, Rust's ownership) and keeps the *what*. Where akit must differ,
the iteration records why in a decision note.

**Stay small.** Aim for at most about 1,500 changed lines, tests included. If an iteration turns
out bigger, stop at a working, tested point, split the rest into a new iteration (`I17b` and so
on) in both plan files, and finish the current one.

**Finish properly.** An iteration is done when:

- its **Done when** items hold;
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and the tests of the crates it
  touches pass, on macOS and on Linux with epoll and io_uring;
- nothing on the request path blocks an I/O thread, and every rule in [the rules](rules.md) holds;
- its status in the index is `done`, in both plan files, in the same commit;
- both progress files record what was built, how it was checked, where it differs from the plan,
  and which iteration is next;
- any decision it made is a note in `docs/decisions/NNN-title.md` and its Chinese twin.

**Session prompt.** A session can be started with:

> Implement iteration IXX from docs/dev-plan.md. Read docs/progress.md, the index and that
> iteration's section, docs/rules.md, and the fit code and tests it points to. Follow the rules;
> update both plan files and both progress files when done.

**When akio falls short**, the iteration does what does not depend on the gap, writes the gap in
[akio API requests](akio-api-requests.md), and records any stopgap in the progress files.

## Workspace layout

Set up in I01 and filled in by later iterations. Names may change; each iteration should touch as
few crates as it can. fit's package of the same role is given for reference.

| Crate | Role | fit |
| --- | --- | --- |
| `akit` | the binary: `run`, `validate`, `adapt`, `fmt`, `store`, `build` | `cmd/fit`, `cli` |
| `akit-config` | the config model, strict readers for JSON, YAML and .conf, validation | `config`, `schema` |
| `akit-runtime` | module registry, compile, the graph and its swap | `runtime`, `module` |
| `akit-listener` | listeners, TLS and QUIC entry points, mux | `listener` |
| `akit-router` | matchers, priorities, the expression language | `router` |
| `akit-middleware` | built-in middlewares | `middleware` |
| `akit-handler` | `respond`, `static`, `redirect`, FastCGI and other handlers | `handler` |
| `akit-proxy` | the HTTP, WebSocket and gRPC reverse proxy | `handler/proxy` |
| `akit-upstream` | upstream pools, balancing, health checks | `upstream` |
| `akit-l4` | TCP and UDP proxying, SNI routing, PROXY protocol | `l4` |
| `akit-tls` | certificates: files, ACME, storage, OCSP, on-demand | `tlsmgr` |
| `akit-admin` | the Admin API, the file provider | `admin` |
| `akit-provider` | Docker, Kubernetes, Consul, etcd | `provider` |
| `akit-observe` | logs, metrics, tracing | `observe`, `logs` |
| `akit-cache` | the response cache | `handler/cache` |
| `akit-plugin` | WASM plugins | `plugin` |
| `tests/e2e` | end-to-end tests of the binary | `test/e2e` |
| `tests/compat` | the fit config compatibility gate: pinned fixtures, field inventory | `templates`, fit-examples |
| `bench` | the harness, scenarios and reports | `bench` |

## Index

Status: `todo`, `doing` or `done`. The next free iteration number is I81.

| ID | Iteration | Depends on | Status |
| --- | --- | --- | --- |
| **M0** | | | |
| I01 | Workspace skeleton and CI | — | done |
| I02 | Config model v0 and the strict JSON reader | I01 | done |
| I03 | Core traits and the module registry | I01 | done |
| I04 | The runtime graph and its swap | I02, I03 | done |
| I05 | `akit run`: listeners and `respond` | I04 | done |
| I06 | CLI, signals and graceful shutdown | I05 | done |
| I07a | Compatibility gate I: pinned fixtures, field inventory, harness | I01 | done |
| I07b | Compatibility gate wired to validate | I02, I07a | done |
| I08 | Benchmark harness and baselines | I05 | done |
| **M1** | | | |
| I09 | TLS entry points from files | I05 | done |
| I10 | HTTP/3 entry point | I09 | done |
| I11 | Router v1: host and path | I04 | done |
| I12 | Static files I | I11 | done |
| I13 | Static files II | I12 | done |
| I14 | Upstream pools on the HTTP/1 client | I04 | done |
| I15 | Reverse proxy I: requests and headers | I11, I14 | done |
| I16 | Reverse proxy II: streamed bodies, linked backpressure | I15 | done |
| I17 | Reverse proxy III: timeouts, cancellation, trailers | I16 | done |
| I17b | Reverse proxy III b: header and idle timeouts, cancellation before the head, write stalls | I17, akio A36–A38 | done |
| I18 | HTTP/2 and h2c upstreams | I17 | done |
| I18b | HTTP/2 upstreams II: streams reset, timed and paced by akio | I18, akio A51–A53, A66 | todo |
| I19 | Load balancing v1 | I14 | done |
| I20 | Passive health checks | I19 | done |
| I21 | Access logs | I15 | done |
| I22 | Reload on SIGHUP | I06 | done |
| I23 | The .conf reader | I02 | done |
| I24 | The YAML reader | I02 | done |
| I25 | M1 conformance and report | I09–I24 | todo |
| **M2** | | | |
| I26 | Admin API I: load, read, versions | I22 | done |
| I27 | Admin API II: patch, rollback, auth | I26 | done |
| I28 | Listeners diffed at run time | I26, A1 | done |
| I29 | File provider | I26 | done |
| I30 | Certificate manager and storage | I09 | done |
| I31 | ACME I: account, orders, HTTP-01 | I30 | done |
| I32 | ACME II: TLS-ALPN-01, renewal, OCSP | I31 | done |
| I33 | Automatic HTTPS defaults | I32 | done |
| I34 | On-demand TLS | I33, A2 | todo |
| I35 | Active health checks | I20 | done |
| I36 | Load balancing v2 | I19 | done |
| I37 | Prometheus metrics | I15 | done |
| I38 | M2 exit: churn test and staging certificate | I26–I37 | todo |
| **M3** | | | |
| I39 | Matchers v2 and expressions | I11 | done |
| I40 | Rewrites, redirects, headers | I39 | done |
| I41 | Rate, concurrency and size limits | I39 | done |
| I42 | Basic auth and mTLS | I39 | done |
| I43 | ForwardAuth | I42 | done |
| I44 | JWT and JWKS | I42 | todo |
| I45 | Retries, circuit breakers, route timeouts | I17 | todo |
| I46 | Weighted services, mirroring, failover | I45 | todo |
| I47 | Compression | I16 | done |
| I48 | The rest of fit's middlewares | I40 | todo |
| I49 | WebSocket proxying | I17 | todo |
| I50 | gRPC proxying | I18 | todo |
| I51 | TCP and UDP proxying | I05 | done |
| I51b | The eBPF sockmap relay | I51 | todo |
| I52 | SNI routing and PROXY protocol | I51, A4, A5 | todo |
| I53 | Many protocols on one port | I52 | todo |
| I54 | Upstream TLS | I18 | todo |
| I55 | SOCKS and CONNECT | I53 | todo |
| I56 | M3 exit | I39–I55 | todo |
| **M4** | | | |
| I57 | Provider framework and the effective config | I29 | todo |
| I58 | Docker provider | I57 | todo |
| I59 | Kubernetes Ingress | I57 | todo |
| I60 | Kubernetes Gateway API and conformance | I59 | todo |
| I61 | Consul and etcd | I57 | todo |
| I62 | Shared certificate storage | I30, I61 | todo |
| I63 | Dashboard | I37 | todo |
| I64 | OpenTelemetry | I37 | todo |
| I65 | Packaging | I06 | todo |
| I66 | M4 exit | I57–I65 | todo |
| **M5** | | | |
| I67 | Cache I: memory tier and RFC 9111 | I17 | todo |
| I68 | Cache II: disk tier, locks, stale | I67 | todo |
| I69 | Cache III: purge and tags | I68, I27 | todo |
| I70 | `akit build` | I03 | todo |
| I71 | WASM I: runtime and ABI | I03 | todo |
| I72 | WASM II: the Proxy-Wasm subset | I71 | todo |
| I73 | `akit adapt` and `akit fmt` | I23, I24 | todo |
| I74 | nginx and Caddyfile converters | I73 | todo |
| I75 | FastCGI | I16 | todo |
| I76 | M5 exit | I67–I75 | todo |
| **1.0** | | | |
| I77 | Fuzzing | I56 | todo |
| I78 | Config reference and guides | I76 | todo |
| I79 | v1 freeze and compatibility | I78 | todo |
| I80 | Security review readiness | I77, I79 | todo |

## Benchmark and optimization status

Every feature that [rule 24](rules.md) gives a benchmark (the hot path: same-run A/B; the data
plane off the hot path: micro-benchmarks of the crate) has a row here; the control plane (config
readers, CLI, Admin API, certificates, providers) has none. **Benchmarked** means akit's numbers
for it are measured and checked in; **Optimized** means work aimed at its cost was done and
measured, so appearing in a baseline alone is "benchmarked", not "optimized". A row not "yes" in
both columns is an open item for the benchmark tasks ([rule 30](rules.md)). A feature task adds
its iteration's row as "no" and names what to measure; a benchmark task updates it with the date,
commit and result.

| Feature | Iteration | Benchmarked | Optimized | Evidence or what is left |
| --- | --- | --- | --- | --- |
| Graph read per request | I04 | yes (2026-11-24, `c9d490a`, `crates/akit-runtime/benches/graph_read.rs`, numbers in [progress](progress.md)) | partial | The per-thread cached read was chosen by measuring it against an `Arc` clone and a mutex (4.06 ns on 1 thread, 8.78 ns on 16); no profile or tuning pass since. |
| Listeners and `respond`, HTTP/1.1 and h2c; idle connections | I05 | yes (2026-11-24, `7c835ad`, `bench/results/baseline-2026-11-24`) | no | In the first baseline only: first in fixed over HTTP/1.1 and h2c. Left: a profile of both; memory per idle HTTP/1.1 connection (1,068 bytes, nginx 548); idle connections over h2c, and at 100k (I25). |
| TLS entry points (HTTP/1.1 and h2 over TLS) | I09 | no | no (built, not measured) | Handshakes a second and fixed over TLS against fit and nginx. |
| HTTP/3 entry point | I10 | no | no (not built yet) | Fixed over h3 against fit and Caddy. |
| Router v1: host and path lookup | I11 | no | no (built, not measured) | A micro-benchmark of the lookup (no allocation) and same-run A/B of fixed against I05's scan, with many routes. |
| Static files | I12, I13 | no | no (built, not measured) | The static scenario, HTTP/1.1 and h2c (skipped for akit in the baseline); precompressed twins and `sendfile` for large files. |
| Upstream pools | I14 | no | no (built, not measured) | Through the proxy scenario: connection reuse, waiting for a connection under a limit. |
| Reverse proxy: HTTP/1.1, streamed bodies, h2 and h2c upstreams | I15–I18 | no | no (built, not measured) | The proxy scenario, HTTP/1.1 and h2c (skipped for akit in the baseline); large bodies both ways, with memory per request in flight; requests with trailers (I17's `Trailer` parsing per request); h2c upstreams (I18): requests a second and CPU a request against an HTTP/1.1 upstream, with many clients multiplexed on few connections, and the per-address lock that leases a stream (`akit_upstream::h2`); I17b's `CancelToken` and `on_gone` per proxied request, and over h2c the idle timeout re-armed per DATA frame. |
| Load balancing v1 and v2 | I19, I36 | no | no (built, not measured) | A micro-benchmark of each policy's pick (fit's `BenchmarkPick` and `BenchmarkPickNew`: the nine policies, a key hashed, a sticky cookie looked up), 1 and 16 threads; proxy A/B against a single server; same-run A/B of the proxy scenario with `round_robin` against `p2c`, `header_hash` and a `sticky_cookie` (the per-request key hashing and, for a first request, the `Set-Cookie` copied). |
| Passive health checks | I20 | no | no (built, not measured) | A micro-benchmark of recording a result on the request path. |
| Active health checks | I35 | no | no (built, not measured) | Off the hot path (a pick still reads one atomic per address): the probes' cost on a busy engine, thousands of addresses probed every second (http and tcp), with the probe timer's thread (the stopgap for akio A45); same-run A/B of the proxy scenario with a health check on and off. |
| Access logs | I21 | no | no (built, not measured) | Same-run A/B of fixed and proxy with logs on and off (json, and text or line), to a file and to `/dev/null`; lines dropped at full load with the default 4 MiB buffer and a slow disk; the per-request cost (the boxed response filter, the shard lock, formatting). |
| Certificates chosen per handshake (the resolver reading the graph) | I30 | no | no (built, not measured) | Full TLS handshakes a second with one and ten certificates, against I09's `sni_cert` and fit. |
| The HTTP-01 check on listeners in the clear | I31 | no | no (built, not measured) | Fixed over HTTP/1.1 and h2c, same-run A/B against main before I31 (a prefix comparison per request under `akit run`). |
| The certificate choice with TLS-ALPN-01 and OCSP staples | I32 | no | no (built, not measured) | Full TLS handshakes a second against I31, with and without a stapled response (the ALPN list read, and a second `Published` map for files with responders); the handshake's size with a response. |
| Prometheus metrics | I37 | no | no (built, not measured) | Same-run A/B of the fixed and proxy scenarios with `metrics` on and off (an inline route costs a thread-local read and three atomic adds; a proxied one also a boxed response filter until akio A29), and with the access log on too (one hook for both); a scrape with many routes and 64 threads (the per-thread shards). |
| Matchers v2 and expressions | I39 | no | no (built, not measured) | A micro-benchmark of `find_request` with matchers, expressions and path regexps (no allocation, `tests/no_alloc.rs`); same-run A/B against I39's base on the fixed scenario (one route, no matchers: within noise expected). |
| Rewrites, redirects, headers | I40 | no | no (built, not measured) | Same-run A/B on the proxy scenario of a route with `headers` (request and response changes) and `rewrite` against the same route without them; the cost per request of the response filter (a boxed closure and an `Arc` clone) and of the request fields kept in a context local (the stopgap until akio API request A33). |
| Rate, concurrency and size limits | I41 | no | no (built, not measured) | Same-run A/B on the proxy scenario of a route with each limit set high enough never to refuse, against the route without it: `rate_limit` by ip and by header (a shard lock, a SipHash, a map lookup), with 1, 1000 and 1000000 clients' keys and 64 threads (the shards' contention), as fit's `BenchmarkRate`; `concurrency_limit` (two atomic operations on one shared line, and a boxed response filter with an `Arc` clone until akio A48); `body_limit` (nothing per request but a field lookup). |
| Basic auth and client certificates | I42 | no | no (built, not measured) | Same-run A/B on the proxy scenario of a route with `basic_auth` (a remembered password: a base64 decode, an HMAC-SHA-256 and the request's fields rewritten into a context local until akio A33) and with `client_cert` and `subject_header` (the subject cached per thread), against the route without them, over HTTP/1.1, h2 and h3; the first check of a password (a suspension and a hop to akio's pool and back) at a few hundred new users a second, and a flood of wrong passwords against remembered users' latency. |
| Forward auth | I43 | no | no (built, not measured) | Same-run A/B on the proxy scenario of a route with `forward_auth` against a fast local auth service, against the route without it, over HTTP/1.1 and h2: the cost of the check (the auth request's fields built, a suspension, one exchange on the auth upstream's pool, the resume on the answer's I/O thread); latency and memory with thousands of requests waiting on a slow auth service, and the fast route's latency meanwhile. |
| Middlewares (retries and the rest) | I44–I48 | no | no (not built yet) | I56 benchmarks every middleware; same-run A/B of each on the proxy scenario. |
| Compression | I47 | no | no (built, not measured) | Against fit, each encoding, as I47 asks: the proxy scenario with `compress` in front, a whole JSON body and a stream of server-sent events, over HTTP/1.1 and h2, per encoding at fit's default levels (responses a second, server CPU, bytes on the wire); the cost of the filter on responses it leaves alone (an image) against no middleware; the memory of held compressed streams (1,000 at once); the static handler's copies against fit's (`compress` on a file kept in memory). |
| WebSocket and gRPC proxying | I49, I50 | no | no (not built yet) | Messages a second and memory per connection against fit. |
| TCP and UDP proxying, SNI routing, many protocols on one port | I51–I53 | no | no (I51 built, not measured; I52, I53 not built yet) | Against fit and nginx's `stream`, as I51 asks: TCP throughput (fit's `BenchmarkTCPProxy`, 256 KB echoed at a time, direct and through akit), connections a second, memory per idle connection; UDP datagrams a second and sessions at once; the per-service idle sweep on akit's timer thread with 10,000 connections (the stopgap for akio A78); the cost of sniffing on a shared port (I53). |
| Response cache | I67, I68 | no | no (not built yet) | Hits from memory and disk against fit. |
| WASM plugins | I71, I72 | no | no (not built yet) | A/B of a no-op plugin against none. |

## M0, foundations

### I01 Workspace skeleton and CI

- **Do**: the Cargo workspace with the crates of the [layout](#workspace-layout) as empty libraries
  and the `akit` binary; edition and MSRV as akio's; akio pinned by `rev`; `#![forbid(unsafe_code)]`
  in every crate; `clippy.toml`, `deny.toml`, `rustfmt.toml`; a `Makefile` (or `xtask`) with
  `check`, `test`, `test-changed`, `lint`; CI with fmt, clippy, tests on macOS and Linux (epoll and
  io_uring in Docker), cargo-deny, and a check that every `docs/*.md` has its `.zh-CN.md` twin.
- **Done when**: `make check` passes locally and in CI; an empty `akit --version` runs.
- **Touches**: everything new.
- **Read**: akio `Cargo.toml`, `deny.toml`, `clippy.toml`, `docs/rules.md`.
- **fit**: `Makefile`, `.github/workflows/ci.yml`.

### I02 Config model v0 and the strict JSON reader

- **Do**: Rust types for fit's top level (`listeners`, `routes`, `upstreams`, `admin`, `log`) and the
  `respond` handler; a JSON reader of akit's own (or one chosen by [decision 001](decisions/001-dependency-policy.md))
  that refuses unknown fields, duplicate keys and wrong types with the path of the error; `//` and
  `/* */` comments and trailing commas, as fit allows.
- **Done when**: fit's `hello` example in JSON loads; table tests cover every error kind with its
  path; the reader is fuzzed for a minute in CI.
- **Touches**: `akit-config`.
- **fit**: `config`, decision 001 (strict decoding).

### I03 Core traits and the module registry

- **Do**: the traits for listener, matcher, router, middleware, handler, service and upstream; the
  registry that maps a config name (`respond`, `proxy`, …) to a module's constructor, with
  provisioning that may fail and cleanup that runs on release. Modules are registered at compile
  time (no global mutable state).
- **Done when**: a test module registers, provisions from config and is cleaned up; an unknown
  module name is an error naming it.
- **Touches**: `akit-runtime`.
- **Read**: akio `Handler`, `Acceptor`, `akio-http` `Router` and `Middleware`.
- **fit**: `module`, `runtime`.

### I04 The runtime graph and its swap

- **Do**: `compile(config) -> Graph`, every module provisioned before the graph is published; the
  current graph behind an atomic pointer; a per-I/O-thread cache of it, refreshed by an epoch check,
  so requests bump no shared reference count; a graph released (its modules cleaned up) when the
  last request and connection holding it end.
- **Done when**: a test swaps graphs from another thread while requests run on several I/O threads,
  and every old graph is released exactly once; a micro-benchmark shows the per-request read costs
  no atomic RMW on shared memory.
- **Touches**: `akit-runtime`.
- **fit**: `runtime/graph*.go`, decision 004.

### I05 `akit run`: listeners and `respond`

- **Do**: listeners from the config on akio's `Engine` (HTTP/1.1 and h2c); the `respond` handler
  (status, headers, body); a route with no match answers 404.
- **Done when**: fit's `hello` example answers `/hello` over HTTP/1.1 and h2c in an e2e test.
- **Touches**: `akit-listener`, `akit-handler`, `akit`.
- **Read**: akio `Engine::builder`, `akio-http` server.

### I06 CLI, signals and graceful shutdown

- **Do**: `akit run`, `akit validate`, `akit version` with fit's flags (`-c` repeated, files and
  directories); SIGINT and SIGTERM shut down gracefully with a drain timeout; exit codes as fit's.
- **Done when**: e2e tests cover validate errors, multiple `-c`, and a shutdown that lets an
  in-flight request finish.
- **Touches**: `akit`.
- **fit**: `cmd/fit`, `cli`.

### I07a Compatibility gate I: pinned fixtures, field inventory, harness

Split from I07 so that the half that needs no config model runs beside I02.

- **Do**: fit's `templates/` (all three formats, as files and as `conf.d` directories) and every
  config of fit-examples copied into `tests/compat/fixtures/` at pinned commits, with their
  licences, `SOURCE.md` and a script that moves the pin; the inventory of every field path fit
  accepts (`tests/compat/fit-fields.txt`), generated from fit's JSON Schema by a dev tool; the
  harness that enumerates the fixtures and judges each outcome (valid, or refused only with
  "not supported yet" naming a field of the inventory); `docs/compatibility.md` with the full
  inventory, generated, every field "not supported yet".
- **Done when**: the harness is tested with stand-in validators; `make check` checks that the
  inventory and its tables match the pin.
- **Touches**: `tests/compat`, `tools`, `docs/compatibility.md`.
- **fit**: `templates/`, `schema/fit.schema.json`, `docs/reference/config.md`; fit-examples'
  `examples_test.go`.

### I07b Compatibility gate wired to validate

- **Do**: turn on the `compat-gate` feature of `akit-compat`: its test calls
  `akit_config::from_json_file` on
  every fixture (`validate_with_akit_config` in `gate.rs`, extended to `conf.d` directories, and to
  .conf and YAML as I23 and I24 land); akit-config's errors name paths the harness matches against the inventory; each
  fixture either passes or fails only with "not supported yet: <field>"; settle what validation
  opens (the files and WebAssembly plugins examples name, relative to fit-examples' root) and the
  `https` example fit only adapts. A field read and then ignored fails the gate.
- **Done when**: the gate runs in CI (`make compat-gate`) and its report is checked in; the fields
  I02 reads are in `tests/compat/supported.txt`.
- **Touches**: `tests/compat`, `akit-config` (the error paths, if needed), CI.
- **fit**: `cli` (`validate`), fit-examples' `examples_test.go`.
- **As built**: no feature: the gate is an ordinary test that `make check` and CI's
  `cargo test --workspace` run. A fixture in a format akit has no reader for (.conf and
  `conf.d` until I23, YAML until I24) is a "format not supported yet" outcome; every outcome is a
  line of the checked-in `tests/compat/gate-report.txt`, which a run must reproduce
  (`make compat-bless` rewrites it). What validation opens for the examples moves to I23, the
  first iteration that reads them. See [progress](progress.md).

### I08 Benchmark harness and baselines

- **Do**: fit's harness (`bench/`) reused for akit: the same scenarios, configs and load generators,
  with akit and fit both servers under test beside nginx, Caddy and Traefik; scripts to cross-build
  for the host and run there; a same-run A/B script for hot-path changes.
- **Done when**: baselines for all five servers from the benchmark host are checked in with the host
  and commit recorded.
- **Touches**: `bench`.
- **fit**: `bench/README.md`, `bench/ab.sh`. **akio**: `tools/bench`, `quiet.sh`.

## M1, the proxy core

### I09 TLS entry points from files

- **Do**: TLS listeners with certificates and keys from files on `akio-tls`; ALPN choosing h2 or
  http/1.1; SNI choosing among several certificates; fit's TLS fields (versions, ciphers, client
  auth).
- **Done when**: e2e tests reach a route over HTTPS with HTTP/1.1 and HTTP/2, and by SNI.
- **Touches**: `akit-listener`, `akit-tls`.
- **Read**: `akio-tls` `ServerConfigBuilder::sni_cert`.

### I10 HTTP/3 entry point

- **Do**: HTTP/3 listeners on `akio-h3`, sharing the certificate source; `Alt-Svc` advertised on the
  TCP listeners for the same host.
- **Done when**: an e2e test reaches the same route over h3 and sees `Alt-Svc` on h2.
- **Touches**: `akit-listener`.
- **As built**: also touches `akit-config` (the `h3` protocol), `akit-tls` (one read of the
  files, two configurations), `akit-runtime` (`Graph::h3_tls`) and `akit` (the address it
  prints). HTTP/3 listens on the port the listener's TCP side has, as fit's; see
  [progress](progress.md).

### I11 Router v1: host and path

- **Do**: routes with host (exact, wildcard) and path (exact, prefix) matchers; explicit priority,
  then most specific first, as fit orders them; 404 when nothing matches. I05's router v0 has
  fit's rules by a scan of the routes: replace the scan with lookups by host and path. Paths are
  canonicalized first, as fit's listener does (dot segments and empty segments removed, a path
  that climbs above the root or hides a separator in an escape answered 400).
- **Done when**: fit's router table tests, ported, pass; a lookup does not allocate; fit's path
  canonicalization tests, ported, pass.
- **Touches**: `akit-router`.
- **fit**: `router`.

### I12 Static files I

- **Do**: the `static` handler on `akio-http`'s `Files`: root, index files, ETag, `Last-Modified`,
  conditional and range requests, MIME types; path traversal refused; file opens on a cache miss
  kept off the I/O thread.
- **Done when**: fit's static tests, ported, pass, including traversal attempts.
- **Touches**: `akit-handler`.
- **Read**: `akio-http` `files.rs`.

### I13 Static files II

- **Do**: precompressed twins (`.br`, `.zst`, `.gz`) by `Accept-Encoding`; `try_files` fallbacks;
  `sendfile` for large files; directory listing off by default.
- **Done when**: tests cover negotiation and fallbacks; a benchmark scenario uses it.
- **Touches**: `akit-handler`, `bench`.

### I14 Upstream pools on the HTTP/1 client

- **Do**: named upstreams (`upstreams`) with their servers; a pool per upstream on akio's HTTP/1
  client, on the same engine as the listeners where akio allows; limits on connections and on
  requests waiting for one; a pool that drains before it closes when its graph is released.
- **Done when**: tests cover the limits, a pool drained under load, and an upstream that refuses
  connections.
- **Touches**: `akit-upstream`.
- **Read**: `akio-http` `client.rs`.
- **fit**: `upstream`, decision 004.

### I15 Reverse proxy I: requests and headers

- **Do**: the `proxy` handler: the request forwarded to the pool, the response sent back; hop-by-hop
  headers removed; `X-Forwarded-For/-Proto/-Host` and `Forwarded` set; trusted proxies; `Host`
  passed or rewritten as fit's fields say.
- **Done when**: fit's header tests, ported, pass.
- **Touches**: `akit-proxy`.
- **fit**: `handler/proxy`.

### I16 Reverse proxy II: streamed bodies, linked backpressure

- **Do**: request bodies streamed to the upstream as they arrive and response bodies streamed back;
  `hold_reads` on one side while the other is above its watermark, resumed on `on_drain`; chunked
  and fixed lengths both ways; body limits.
- **Done when**: tests with a slow client and a slow upstream show bounded memory with large bodies
  both ways, on epoll and io_uring.
- **Touches**: `akit-proxy`.

### I17 Reverse proxy III: timeouts, cancellation, trailers

- **Do**: connect, header, idle and total timeouts with 502 and 504 as fit answers them; the upstream
  request cancelled when the client leaves; trailers passed through.
- **Done when**: fit's proxy conformance suite for HTTP/1.1, ported, passes.
- **Touches**: `akit-proxy`, `akit-upstream`.
- **Split**: what akio cannot do yet went to I17b.

### I17b Reverse proxy III b: header and idle timeouts, cancellation before the head, write stalls

- **Do**: the upstream's `response_header_timeout` and `response_idle_timeout` (akio API request
  A36); the upstream request cancelled when the client goes before the response's head, or, over
  HTTP/1, mid-upload (A37, A38); `limits.write_stall_timeout`, applied on reload too (A38).
- **Done when**: fit's `TestResponseHeaderTimeout`, `TestResponseIdleTimeout` and the rest of
  `TestClientAbort`, ported, pass.
- **Touches**: `akit-proxy`, `akit-upstream`, `akit-listener`, `akit-config`.
- **Split**: what akio's h2 client does not heed yet (the timeouts and the cancel over h2, akio API
  request A66) is checked by `akit_upstream::h2`'s tick until I18b.

### I18 HTTP/2 and h2c upstreams

- **Do**: upstreams over h2 and h2c on akio's `H2Client`, with per-stream backpressure and limits on
  streams per connection.
- **Done when**: the conformance suite passes against h2 and h2c upstreams.
- **Touches**: `akit-upstream`, `akit-proxy`.
- **Read**: `akio-http` `h2client.rs`.
- **Split**: what akio's h2 client cannot do yet went to I18b.

### I18b HTTP/2 upstreams II: streams reset, timed and paced by akio

- **Do**: replace I18's stopgaps in `akit_upstream::h2` with akio's h2 client: streams reset when
  given up, `response_timeout` armed by akio and request trailers sent (akio API request A51);
  h2 downloads held and uploads paced per stream (A52); each connection sized by the server's
  `SETTINGS_MAX_CONCURRENT_STREAMS`, the dial bounded by the upstream's `connect_timeout`, and
  `h2`'s fallback to HTTP/1.1 when ALPN does not choose h2 (A53); the response header and idle
  timeouts and the cancel token heeded by akio (A66), the tick's checks of them gone.
- **Done when**: the h2 proxy tests' `request_trailers` passes, and a slow client and a slow
  upstream show bounded memory over h2 as I16's tests show it over HTTP/1.1, on epoll and
  io_uring.
- **Touches**: `akit-upstream`, `akit-proxy`.

### I19 Load balancing v1

- **Do**: round robin, weighted round robin and least connections, with state per I/O thread where
  fit's results allow and shared where it must be.
- **Done when**: distribution tests match fit's expectations within their tolerance.
- **Touches**: `akit-upstream`.

### I20 Passive health checks

- **Do**: a server marked down after N failures within a window and back after its timeout, as
  nginx's `max_fails` / `fail_timeout`.
- **Done when**: tests cover marking down, recovery and all servers down.
- **Touches**: `akit-upstream`.

### I21 Access logs

- **Do**: structured access logs in fit's formats and fields, buffered per I/O thread and written by
  a bounded writer thread; a full buffer drops lines and counts them rather than block.
- **Done when**: tests cover the fields and a writer slower than the traffic.
- **Touches**: `akit-observe`.
- **fit**: `observe`, `logs`.

### I22 Reload on SIGHUP

- **Do**: SIGHUP builds a new graph and swaps it; a broken config leaves the running one; listeners
  whose address did not change are kept. New and removed listeners use A1 (done on akio `a9a620ab`;
  the `adopt_tcp` stopgap it used before is gone).
- **Done when**: an e2e test reloads under traffic with no failed request.
- **Touches**: `akit`, `akit-runtime`, `akit-listener`.

### I23 The .conf reader

- **Do**: fit's .conf format read into the same model, with errors at line and column; split files
  and directories as fit merges them; the compatibility gate's hook reads .conf files and
  `conf.d` directories with it, settling what validation opens for the examples (the files and
  WebAssembly plugins they name, relative to fit-examples' root, and the stand-in secrets), moved
  here from I07b.
- **Done when**: every .conf in fit's templates and fit-examples reads into the same model as its
  JSON twin.
- **Touches**: `akit-config`, `tests/compat`.
- **fit**: `docs/reference/fit.md`, `config` (the .conf parser), decision 021.

### I24 The YAML reader

- **Do**: YAML read into the same model, the subset fit accepts (decided with a note: a reader of
  akit's own or a crate, per decision 001).
- **Done when**: fit's YAML templates read into the same model as their JSON twins.
- **Touches**: `akit-config`.
- **fit**: decision 019.

### I25 M1 conformance and report

- **Do**: h2spec on the HTTP/2 entry point; the ported conformance suite on HTTP/1.1 and HTTP/2;
  benchmarks for static files and proxying; memory per idle connection at 10k and 100k connections.
- **Done when**: the M1 report, with *Where akit stands*, is checked in and the roadmap's M1 exit
  criteria hold.
- **Touches**: `bench`, `tests`.

## M2, dynamic config and TLS

### I26 Admin API I: load, read, versions

- **Do**: the Admin API on localhost or a Unix socket: load a whole config, read it, versions and
  ETags, with fit's paths and payloads.
- **Done when**: fit's Admin API tests for these paths, ported, pass.
- **Touches**: `akit-admin`.
- **fit**: `admin`, decision 005.
- **Done** (decision 006): on an akio engine of its own, loads applied by the control thread
  through I22's reload; a Unix socket waits on akio A23, closing a connection after a refused body
  on A24.

### I27 Admin API II: patch, rollback, auth

- **Do**: patch by path, roll back to an earlier version, optional auth, concurrent writers refused
  by ETag.
- **Done when**: tests cover conflicts, rollback and auth.
- **Touches**: `akit-admin`.
- **fit**: `admin`, decision 005's I25 update.
- **Done** (decision 006's I27 update): `PUT`, `PATCH` and `DELETE /config[/<pointer>]`,
  `POST /rollback/{n}`, `POST /reload` and `?dry_run=1`, each change made from the running config
  under the lock that orders applies; `admin.token_file`. TLS for the API, and with it an address
  off loopback, is not done; closing a connection after a 401 waits on A24.

### I28 Listeners diffed at run time

- **Do**: on a swap, new addresses bound, removed ones drained with a timeout, unchanged ones kept;
  as I22 does on SIGHUP (on A1, which removed the `adopt_tcp` stopgap). Depends on akio A1.
- **Done when**: a test adds and removes listeners under traffic with no failed request.
- **Touches**: `akit-listener`, `akit-runtime`.
- **fit**: `listener.Server.Reload`, `Plan` and `binding.same`; `listener/reload_test.go`.
- **Done**: on I22's runtime listeners, a listener is kept as fit's `binding.same` decides: its
  certificate files and `tls.acme` names may change (its manager serves the new ones), its client
  authorities are compared by content. A startup h3 listener still waits on A22; a request sent on
  an idle HTTP/1 connection as it is drained meets the close (A54).

### I29 File provider

- **Do**: watch the config files (by polling, as fit decided) and apply changes through the same
  path as the Admin API.
- **Done when**: tests cover edits, broken edits and removed files.
- **Touches**: `akit-admin`.
- **fit**: decision 006.

### I30 Certificate manager and storage

- **Do**: a certificate manager behind a trait, with a file system store and locks; certificates
  served per SNI from it; reloads without dropping handshakes.
- **Done when**: tests cover storage, locking and a certificate replaced under traffic.
- **Touches**: `akit-tls`.
- **fit**: `tlsmgr`.

### I31 ACME I: account, orders, HTTP-01

- **Do**: an ACME client of akit's own on `akio-crypto` (JWS, ECDSA) and `akio-x509` (CSR), run off
  the I/O threads; accounts, orders, HTTP-01 answered by the listeners.
- **Done when**: tests against Pebble issue a certificate.
- **Touches**: `akit-tls`.
- **fit**: decision 007.

### I32 ACME II: TLS-ALPN-01, renewal, OCSP

- **Do**: TLS-ALPN-01; renewal ahead of expiry with jitter and backoff; OCSP stapling.
- **Done when**: tests against Pebble cover TLS-ALPN-01 and renewal; stapling is checked.
- **Touches**: `akit-tls`.
- **Done**: TLS-ALPN-01, renewal by lifetime and by ARI (RFC 9773), OCSP stapling of ACME's
  certificates and of files' under trusted roots. Files' under the system's roots wait on akio
  A57; closing `acme-tls/1` connections after their handshake waits on A58.

### I33 Automatic HTTPS defaults

- **Do**: every host name in the config gets a certificate unless told otherwise; HTTP redirected to
  HTTPS; fit's defaults and opt-outs.
- **Done when**: a config with only a host name serves HTTPS against Pebble.
- **Touches**: `akit-tls`, `akit-runtime`.
- **Done**: fit's `auto_https` in `akit-config` (the listeners made or filled in when the config is
  read), the redirect in `akit-listener`'s dispatcher; a config naming only a host gets its
  certificate from Pebble through HTTP-01 on the `http` listener made and serves it on `https`.

### I34 On-demand TLS

- **Do**: certificates issued at handshake time for names an "ask" endpoint allows, rate limited.
  Depends on akio A2 (a certificate chosen by a callback that completes later).
- **Done when**: tests cover allowed, refused and rate-limited names, with the I/O thread never
  blocked during issuance.
- **Touches**: `akit-tls`.

### I35 Active health checks

- **Do**: HTTP and TCP probes with intervals, thresholds and jitter, run on akio's clients.
- **Done when**: an upstream that stops answering leaves rotation within its window and comes back.
- **Touches**: `akit-upstream`.

### I36 Load balancing v2

- **Do**: random, P2C, consistent hashing on IP, header, cookie or URI, sticky sessions by cookie.
- **Done when**: distribution and stickiness tests pass.
- **Touches**: `akit-upstream`.

### I37 Prometheus metrics

- **Do**: per router, service and upstream; connection and pool gauges; reloads and certificate
  expiry; akio's engine metrics; fit's metric names.
- **Done when**: a scrape test checks the names and labels against fit's.
- **Touches**: `akit-observe`.

### I38 M2 exit: churn test and staging certificate

- **Do**: thousands of swaps under traffic; a Let's Encrypt staging certificate; the health window
  test.
- **Done when**: the roadmap's M2 exit criteria hold and the results are in progress.
- **Touches**: `tests`, `bench`.

## M3, gateway features

### I39 Matchers v2 and expressions

- **Do**: method, header, query, remote IP and CIDR, regexp paths (`akio-regex`), and fit's
  expression language combining them; compiled at config time.
- **Done when**: fit's matcher tests, ported, pass; lookups stay allocation-free.
- **Touches**: `akit-router`.

### I40 Rewrites, redirects, headers

- **Do**: path rewrites, prefix stripping, redirects, request and response header changes, with
  values built at compile time.
- **Done when**: fit's tests for these, ported, pass.
- **Touches**: `akit-middleware`.

### I41 Rate, concurrency and size limits

- **Do**: rate limits by key (token bucket), concurrency limits, request size limits, with state
  sharded per I/O thread where exactness allows.
- **Done when**: tests cover each limit under, at and over.
- **Touches**: `akit-middleware`.

### I42 Basic auth and mTLS

- **Do**: Basic auth with hashed passwords checked in constant time; client certificates required
  per route.
- **Done when**: tests cover both, and hashes are checked off the I/O thread when they are slow
  (bcrypt).
- **Touches**: `akit-middleware`, `akit-tls`.

### I43 ForwardAuth

- **Do**: the auth request sent on akio's client; the request waits without holding the thread;
  headers copied as fit's fields say.
- **Done when**: tests cover allow, deny, a slow and a dead auth server.
- **Touches**: `akit-middleware`.

### I44 JWT and JWKS

- **Do**: JWT checked per request; JWKS fetched and cached by the control plane.
- **Done when**: tests cover algorithms, expiry, key rotation and a JWKS outage.
- **Touches**: `akit-middleware`.

### I45 Retries, circuit breakers, route timeouts

- **Do**: retries with budgets and idempotency rules; circuit breakers; per-route timeouts.
- **Done when**: tests cover each, including a body that cannot be replayed.
- **Touches**: `akit-proxy`, `akit-upstream`.

### I46 Weighted services, mirroring, failover

- **Do**: services that split by weight, mirror to a second service without waiting for it, and
  fail over.
- **Done when**: tests cover distribution, a slow mirror and failover.
- **Touches**: `akit-upstream`.

### I47 Compression

- **Do**: gzip, zstd and brotli negotiated per request, streamed, skipping compressed types.
- **Done when**: tests cover negotiation and streaming; a benchmark compares with fit.
- **Touches**: `akit-middleware`.
- **Read**: `akio-http` `compress`, `akio-deflate`, `akio-zstd`, `akio-brotli`.

### I48 The rest of fit's middlewares

- **Do**: every middleware fit's config reference lists that is not yet in akit (CORS, request IDs,
  IP headers and the like), each with its ported tests.
- **Done when**: the compatibility gate lists no middleware as unsupported.
- **Touches**: `akit-middleware`.

### I49 WebSocket proxying

- **Do**: WebSocket proxied with backpressure both ways, from HTTP/1.1, HTTP/2 and HTTP/3 entry
  points to HTTP/1.1 upstreams.
- **Done when**: end-to-end tests with slow peers pass on each entry point.
- **Touches**: `akit-proxy`.
- **Read**: `akio-ws`.

### I50 gRPC proxying

- **Do**: gRPC over h2c and TLS upstreams, trailers and deadlines carried, unary and streaming.
- **Done when**: end-to-end tests against grpc-go pass.
- **Touches**: `akit-proxy`.

### I51 TCP and UDP proxying

- **Do**: layer 4 routes for TCP and UDP with linked backpressure and timeouts; the eBPF sockmap relay
  where the listener asks for it.
- **Done when**: tests with slow peers pass; a benchmark compares with fit and nginx's `stream`.
- **Touches**: `akit-l4`.
- **Split**: the eBPF sockmap relay went to I51b; the benchmark is the maintainer's (rule 30).

### I51b The eBPF sockmap relay

- **Do**: a `tcp` service's connections spliced in the kernel (akio's `ebpf::Relay`) where a
  listener asks for it and the kernel and privileges allow, falling back to I51's relay; idle
  timeouts kept from the sockets' byte counts.
- **Done when**: a field (fit has none: an akit extension, or one added to fit first) turns it on;
  tests on Linux 6.15+ as root cover splicing, the fallback and the idle timeout.
- **Touches**: `akit-l4`, `akit-config`.

### I52 SNI routing and PROXY protocol

- **Do**: TLS routed by SNI without termination; PROXY protocol v1/v2 accepted and sent. Depends on
  akio A4 and A5.
- **Done when**: tests cover both, including a truncated or forged PROXY header.
- **Touches**: `akit-l4`, `akit-listener`.

### I53 Many protocols on one port

- **Do**: a listener split by akio-mux into HTTP, TLS (terminated or passed by SNI), gRPC, SOCKS and
  custom protocols, with sniff limits and timeouts.
- **Done when**: one port serves HTTP/1.1, HTTP/2, TLS passthrough by SNI and SOCKS at once.
- **Touches**: `akit-listener`.

### I54 Upstream TLS

- **Do**: TLS to upstreams with custom CAs, client certificates and SNI overrides.
- **Done when**: tests cover each.
- **Touches**: `akit-upstream`.

### I55 SOCKS and CONNECT

- **Do**: forward proxying with `akio-socks` and HTTP CONNECT, with auth and limits.
- **Done when**: tests cover both with slow peers.
- **Touches**: `akit-l4`.

### I56 M3 exit

- **Do**: conformance tests and benchmarks for every middleware; the M3 report.
- **Done when**: the roadmap's M3 exit criteria hold.
- **Touches**: `tests`, `bench`.

## M4, platform integration

### I57 Provider framework and the effective config

- **Do**: providers feeding the same compile path; fit's merge rules; an Admin API endpoint showing
  the effective config and where each part came from.
- **Done when**: tests cover merging and the endpoint.
- **Touches**: `akit-provider`, `akit-admin`.

### I58 Docker provider

- **Do**: Docker labels as config, followed through the events stream on akio's HTTP client over the
  Unix socket.
- **Done when**: the Docker Compose example routes services as they start and stop.
- **Touches**: `akit-provider`.

### I59 Kubernetes Ingress

- **Do**: a watch client of akit's own (list and watch over HTTP/2), Ingress to routes.
- **Done when**: tests against a kind cluster route by Ingress.
- **Touches**: `akit-provider`.

### I60 Kubernetes Gateway API and conformance

- **Do**: HTTPRoute, GRPCRoute, TLSRoute, TCPRoute, UDPRoute; the conformance suite.
- **Done when**: akit passes the `GATEWAY-HTTP` profile.
- **Touches**: `akit-provider`.
- **fit**: decision 012.

### I61 Consul and etcd

- **Do**: service catalogues and KV config from Consul (HTTP) and etcd (its gRPC API on `akio-grpc`).
- **Done when**: tests against both follow changes.
- **Touches**: `akit-provider`.

### I62 Shared certificate storage

- **Do**: certificate storage in Redis, etcd or Consul with locks, so instances share issuance.
- **Done when**: two instances share one certificate with one issuance.
- **Touches**: `akit-tls`.

### I63 Dashboard

- **Do**: fit's read-only dashboard pages served by akit.
- **Done when**: the pages show routers, services, upstream health, certificates and versions.
- **Touches**: `akit-admin`.

### I64 OpenTelemetry

- **Do**: tracing with context propagated to upstreams; OTLP export of traces and metrics, batched
  off the I/O threads.
- **Done when**: traces reach a collector in a test.
- **Touches**: `akit-observe`.

### I65 Packaging

- **Do**: static musl binaries for Linux (amd64, arm64) and macOS builds; a container image; a Helm
  chart and systemd units as fit's.
- **Done when**: the release workflow produces them with checksums.
- **Touches**: `deploy`, CI.

### I66 M4 exit

- **Do**: conformance and the Compose example in CI; the M4 report.
- **Done when**: the roadmap's M4 exit criteria hold.

## M5, caching and extensions

### I67 Cache I: memory tier and RFC 9111

- **Do**: `Cache-Control`, `Vary`, validators, `Age`; a bounded memory tier.
- **Done when**: RFC 9111 tests pass.
- **Touches**: `akit-cache`.

### I68 Cache II: disk tier, locks, stale

- **Do**: a disk tier on the logical pool; cache locks so one miss fetches; stale-while-revalidate and
  stale-if-error.
- **Done when**: a stampede test makes one upstream fetch; disk I/O never runs on an I/O thread.
- **Touches**: `akit-cache`.

### I69 Cache III: purge and tags

- **Do**: purge by key and tag through the Admin API.
- **Done when**: tests cover both.
- **Touches**: `akit-cache`, `akit-admin`.

### I70 `akit build`

- **Do**: build akit with the user's crates as modules (a generated workspace that registers them),
  as `fit build` does with Go modules; a module guide.
- **Done when**: an example module builds in and serves.
- **Touches**: `akit`, `docs`.

### I71 WASM I: runtime and ABI

- **Do**: choose the runtime (decision note: wasmi, wasmtime or other, measured against fit's
  wazero); fit's ABI; plugins on a pool of their own with time and memory limits.
- **Done when**: fit's header-editing plugin runs unchanged with a measured, bounded cost.
- **Touches**: `akit-plugin`.
- **fit**: decisions 017, 018.

### I72 WASM II: the Proxy-Wasm subset

- **Do**: the Proxy-Wasm subset fit supports.
- **Done when**: fit's Proxy-Wasm tests, ported, pass.
- **Touches**: `akit-plugin`.

### I73 `akit adapt` and `akit fmt`

- **Do**: conversion between .conf, YAML and JSON; formatting as fit's.
- **Done when**: round trips over fit's templates are lossless.
- **Touches**: `akit-config`, `akit`.

### I74 nginx and Caddyfile converters

- **Do**: converters for the subset fit converts, listing what they skipped.
- **Done when**: fit's converter samples give the same output.
- **Touches**: `akit-config`.

### I75 FastCGI

- **Do**: a FastCGI handler with streamed bodies, pools and timeouts.
- **Done when**: tests against php-fpm pass.
- **Touches**: `akit-handler`.

### I76 M5 exit

- **Do**: the cache against nginx's `proxy_cache`; the WASM cost; converter samples; the M5 report.
- **Done when**: the roadmap's M5 exit criteria hold.

## 1.0

### I77 Fuzzing

- **Do**: fuzz targets for the config readers, the router, the proxy's header handling, the PROXY
  protocol and the cache; run in CI for a fixed time and longer on a schedule.
- **Done when**: every parser has a target and the corpus is checked in.

### I78 Config reference and guides

- **Do**: the config reference generated from the code; getting started; migration from fit, nginx,
  Caddy and Traefik; all bilingual.
- **Done when**: every field in the model appears in the reference.

### I79 v1 freeze and compatibility

- **Do**: the config schema and the Admin API frozen as v1, the same as fit's v1; a compatibility
  document saying what stays stable.
- **Done when**: the compatibility gate passes on every fit template with no unsupported field.

### I80 Security review readiness

- **Do**: a threat model; `cargo audit` and `cargo deny` clean; the scope of the external review (TLS
  handling, the Admin API, the plugin sandbox).
- **Done when**: the review can start.

## After 1.0

Each track is broken into iterations when it starts, from fit's iterations of the same track
(their numbers in fit's [development plan](https://github.com/lesismal/fit/blob/main/docs/dev-plan.md)). The order
below is the planned order; tracks with no dependency between them may run in parallel.

| Order | Track | fit's iterations | First steps in akit |
| --- | --- | --- | --- |
| 1 | Logging: files, rotation and retention | I155–I157 | log files with time zones, rotation by size and time, retention |
| 2 | Access lists | I195 | accept-time filter in akio (A3); global and per-scope lists |
| 3 | Console and encrypted store | I104 | the encrypted store, the console's pages, logins |
| 4 | Tunnel and LLM | I105–I107, I159–I162 | hub, node and proxy roles on `akio-ws` and `akio-quic`; link tokens |
| 5 | WebRTC | I87, I108–I111 | TURN on `akio-turn`; WHIP and WHEP on `akio-webrtc` |
| 6 | Streaming and media auth | I115–I117 | the streaming role on `akio-media`; `media_auth` |
| 7 | Media library and channels | I131–I139 | an MP4 reader (akio request); VOD; channels; signed URLs |
| 8 | Cluster management | I140–I150, I153 | members, overview, rollouts with health gates |
| 9 | AI assistant | I124–I130, I152–I154 | the chat panel, tools, MCP server |
| 10 | BitTorrent | I196–I210 | the client and tracker roles on `akio-bittorrent` |
| 11 | Sync | I163–I194, I211–I222 | the pipeline core and checkpoints, then connectors by fit's order |
| 12 | Prefork and NUMA | planned in fit | `akio-prefork` with placement; measured before it is on by default |
| 13 | P2P and VPN | I84–I103 | after fit's track is done |
