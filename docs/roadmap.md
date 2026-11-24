# akit roadmap

[English](roadmap.md) | [简体中文](roadmap.zh-CN.md)

akit is a reverse proxy, load balancer, TLS terminator, static file server and API gateway written
in Rust on [akio](https://github.com/lesismal/akio). It is to akio what
[fit](https://github.com/lesismal/fit) is to [fib](https://github.com/lesismal/fib): the same
product, the same config, the same behaviour, built on the Rust counterpart of fit's engine.

fit has already been through every milestone below, and its [roadmap](https://github.com/lesismal/fit/blob/main/docs/roadmap.md),
[comparison with nginx, Caddy and Traefik](https://github.com/lesismal/fit/blob/main/docs/comparison.md)
and [progress](https://github.com/lesismal/fit/blob/main/docs/progress.md) are akit's specification. This roadmap
does not argue again what fit settled; it says in which order akit catches up, how we know each
step is done, and which part of akio it stands on. The [development plan](dev-plan.md) breaks the
milestones into iterations sized for one working session. Dates are left out on purpose: a
milestone ends when its exit criteria hold.

- [Goals](#goals)
- [Non-goals](#non-goals)
- [Design principles](#design-principles)
- [Architecture](#architecture)
- [Milestones](#milestones)
- [After 1.0](#after-10)
- [Feature map](#feature-map)
- [Performance targets](#performance-targets)
- [Risks and open questions](#risks-and-open-questions)

## Goals

1. **Feature parity with fit.** Every feature fit's users rely on is in akit, in the order of the
   milestones below. A feature is at parity when fit's own tests and examples for it, ported, pass
   against akit.
2. **One config for both.** A config fit accepts, in .conf, YAML or JSON, is accepted by akit and
   means the same thing; a field akit does not support yet is refused by name, never ignored.
   Moving between fit and akit is a binary swap ([decision 002](decisions/002-config-compatibility.md)).
3. **Faster and leaner than fit.** No garbage collector, callbacks inline on the I/O thread,
   io_uring and eBPF steering per listener: akit should beat fit on throughput, p99 latency and
   memory per connection on the same host, and stay at or ahead of nginx.
4. **Memory safety with no `unsafe` of its own.** All `unsafe` stays in akio's audited crates; akit
   is `#![forbid(unsafe_code)]` throughout.

## Non-goals

- **Diverging from fit on purpose.** Where akit and fit could differ (a field name, a default, an
  Admin API path) they do not. An improvement worth making is made in both, or raised with fit
  first.
- **An async runtime.** akit runs on akio's callbacks and logical pool; no tokio, no `async fn` on
  the data plane. An outside library that needs an async runtime is kept off the data plane or not
  used.
- **Windows.** akio does not support it. Linux is first-class; macOS is for development.
- **What fit leaves out**: a mail proxy, API management, Lua/njs plugins, full nginx config
  compatibility. fit's [service mesh track](https://github.com/lesismal/fit/blob/main/docs/mesh-roadmap.md)
  is not part of this roadmap.

## Design principles

fit's principles, held the Rust way. Every milestone is reviewed against them, and
[the development rules](rules.md) turn them into checks.

1. **The data plane keeps akio's model.** Callbacks run inline on the connection's I/O thread and
   never block. Anything that must block (disk cache misses, DNS, ACME, external auth, WASM)
   goes to akio's logical pool with `spawn_blocking`, or to a bounded pool of akit's own, and
   comes back as a completion on the owning thread. Waits between pools go one way only.
2. **The control plane and the data plane are separate.** Config parsing, providers, certificate
   issuance and health probes run off the I/O threads and produce an immutable runtime graph. The
   data plane reads the current graph through one atomic pointer, cached per I/O thread so that no
   request bumps a reference count every thread shares.
3. **One concept model for every protocol**: listener → router → middleware → service → upstream,
   the same for HTTP, TCP and UDP, with akio-mux splitting a listener's traffic by protocol first.
4. **Safe defaults**: HTTPS on, modern TLS, HTTP redirected to HTTPS, limits on every size and
   count, the Admin API on localhost or a Unix socket.
5. **nginx's bar for speed, static files and caching, and fit's as a floor.** Each milestone
   publishes numbers against nginx, Caddy, Traefik and fit on the benchmark host; a regression is
   a bug.
6. **Lean dependencies.** akit depends on akio and the standard library. A third-party crate comes
   in only where writing our own would be much worse, behind a trait of akit's own, with a
   decision note ([decision 001](decisions/001-dependency-policy.md)).
7. **What akio lacks is added to akio.** A gap is written up in
   [akio API requests](akio-api-requests.md) and fixed in akio, not worked around in akit.

## Architecture

```mermaid
flowchart LR
    subgraph Control plane, off the I/O threads
        Src["config sources<br/>files · Admin API · providers"] --> Ad["readers<br/>.conf · YAML · JSON → one model"]
        Ad --> V["validate (strict, fit's schema)"]
        V --> Cp["compile: provision every module"]
        Cp --> G["immutable graph v(n+1)"]
        ACME["certificates<br/>files · ACME · OCSP"] --> G
        HC["health checks"] --> G
    end
    G -->|"atomic swap; listeners diffed"| L
    subgraph Data plane on akio I/O threads
        L["listeners<br/>TCP · UDP · TLS · QUIC"] --> Mx["akio-mux<br/>split by protocol"]
        Mx --> R["routers<br/>host · path · header · SNI"]
        R --> Mw["middlewares"]
        Mw --> Sv["services<br/>balancing · splitting · failover"]
        Sv --> U["upstream pools<br/>akio HTTP/1, HTTP/2, HTTP/3 clients"]
    end
```

A request is served entirely by the graph it started on; long-lived connections keep their graph
until they end or a drain timeout closes them. [Architecture](architecture.md) has the crates, the
threading and the request path in detail.

## Milestones

Each milestone lists its **scope**, the **exit criteria** that end it, and the **akio parts** it
leans on. Releases are numbered 0.x until 1.0; each milestone ends in a release. The scope of each
is fit's milestone of the same name; what is listed is what it takes in akit.

### M0, foundations

**Scope**

- The Cargo workspace (`crates/akit-*`, the `akit` binary), CI with fmt, clippy, tests on macOS and
  Linux (epoll and io_uring), cargo-deny, and the bilingual docs check.
- akio pinned by commit hash ([rule 19](rules.md#9-the-akio-version)).
- The core traits: listener, matcher, router, middleware, handler, service, upstream, and the
  module registry config names resolve to.
- The config model: fit's schema in Rust types, strict decoding (an unknown field is an error),
  JSON first; `akit validate` and `akit run`.
- The runtime graph: compiled from the config, swapped through an atomic pointer, released when its
  last connection ends.
- The benchmark harness: fit's scenarios and load generators, with fit added to nginx, Caddy and
  Traefik as a server under test.

**Exit criteria**

- `akit run -c fit.json` serves a fixed response over HTTP/1.1 and HTTP/2, from a config fit also
  accepts.
- Baselines for nginx, Caddy, Traefik and fit on the benchmark host are checked in.

**akio parts**: `Engine`, `akio-http` (HTTP/1.1, HTTP/2, Router), `akio-tls`.

### M1, the proxy core

**Scope**

- Entry points: HTTP/1.1, HTTP/2 (TLS and h2c) and HTTP/3; TLS from certificate files; graceful
  shutdown.
- Static files on `akio-http`'s `Files` with `sendfile`: precompressed twins, ranges, index files,
  `try_files` fallbacks, ETag and `Last-Modified`.
- The reverse proxy, written for akio: upstream pools on the HTTP/1 and HTTP/2 clients; bodies
  streamed both ways with linked backpressure (`hold_reads` / `on_drain`); connect, header, idle
  and total timeouts; cancellation when the client leaves; hop-by-hop headers removed,
  `X-Forwarded-*` and `Forwarded` set with a trusted-proxy list.
- Load balancing: round robin, weighted round robin, least connections; passive health checks.
- Access logs: structured, buffered, written off the I/O threads.
- Reload on SIGHUP, keeping the listeners whose address did not change.
- The .conf and YAML readers, so fit's examples run from M1 on.

**Exit criteria**

- h2spec passes on the HTTP/2 entry point; fit's proxy conformance suite (headers, bodies,
  trailers, timeouts, client aborts), ported, passes on HTTP/1.1 and HTTP/2.
- Static file and proxy benchmarks published against M0's baselines; memory per idle connection
  at 10k and 100k connections compared with fit and nginx.

**akio parts**: HTTP/1, HTTP/2 and HTTP/3 servers and clients, `Files`, `sendfile`, `hold_reads`,
`on_drain`, `spawn_blocking`. Needs from akio: runtime listeners (A1).

### M2, dynamic config and TLS

**Scope**

- The Admin API: load, read, patch by path, roll back; versions and ETags; on localhost or a Unix
  socket by default.
- Atomic swaps: a new config provisioned completely before the swap; listeners diffed, removed
  ones drained.
- The file provider.
- Automatic HTTPS: ACME (HTTP-01, TLS-ALPN-01), renewal, OCSP stapling, certificate storage behind a
  trait, on-demand TLS behind an "ask" endpoint and rate limits.
- Active health checks (HTTP and TCP); random, P2C, consistent hashing and sticky sessions.
- Prometheus metrics per router, service and upstream.

**Exit criteria**

- A churn test (thousands of swaps under traffic) drops no connection and fails no request.
- A config with only a host name gets a certificate from Let's Encrypt staging and serves HTTPS.
- An upstream that stops answering probes leaves rotation within the window and returns after.

**akio parts**: `akio-tls`, `akio-x509`, `akio-crypto`, the HTTP client. Needs from akio: listeners
added, removed and drained at run time (A1); certificates chosen per handshake by a callback that
may complete later (A2).

### M3, gateway features

**Scope**

- Matchers (host, path, method, header, query, remote IP and CIDR, expressions) with explicit
  priorities; rewrites, redirects, header changes.
- Rate, concurrency and size limits; Basic, ForwardAuth (asynchronous), JWT with JWKS, mTLS.
- Retries with budgets, circuit breakers, per-route timeouts; weighted services, mirroring,
  failover.
- Compression: gzip, zstd, brotli (`akio-deflate`, `akio-zstd`, `akio-brotli`).
- WebSocket proxying, including from HTTP/2 and HTTP/3 entry points; gRPC proxying.
- Layer 4: TCP and UDP proxying, TLS routed by SNI without termination, PROXY protocol v1/v2 in and
  out, several protocols on one port through akio-mux.
- Upstream TLS: mTLS, custom CAs, SNI overrides.

**Exit criteria**

- Each middleware has a conformance test and a benchmark; none blocks an I/O thread.
- WebSocket, gRPC (unary and streaming) and TCP/UDP proxying pass end-to-end tests with slow peers.
- One port serves HTTP/1.1, HTTP/2, TLS passthrough by SNI and SOCKS at once.

**akio parts**: `akio-mux`, `akio-ws`, `akio-grpc`, `akio-socks`, `akio-regex`, the compressors, the
relay helpers and the eBPF sockmap relay for layer 4. Needs from akio: an SNI rule in akio-mux (A5),
PROXY protocol (A4).

### M4, platform integration

**Scope**

- Providers: Docker labels, Kubernetes Gateway API and Ingress, Consul, etcd; written-down merge
  rules and an endpoint showing the effective config and where each part came from.
- Shared certificate storage with locks for several instances.
- A read-only dashboard; OpenTelemetry tracing and OTLP metrics.
- Packaging: static musl binaries, a container image, a Helm chart, systemd units.

**Exit criteria**

- akit passes the Gateway API conformance profile fit claims (`GATEWAY-HTTP`).
- The Docker Compose example routes services by their labels as they start and stop.

**akio parts**: the HTTP/1 and HTTP/2 clients (watches are long-lived streamed requests). Control
plane work otherwise.

### M5, caching and extensions

**Scope**

- The response cache (RFC 9111): memory and disk tiers, disk I/O off the I/O threads, cache locks,
  stale-while-revalidate and stale-if-error, purge by key and tag.
- Compile-time modules: `akit build` builds akit with crates of the user's own, as `fit build` does
  with Go modules.
- WASM plugins on a pool of their own, with fit's ABI and its Proxy-Wasm subset.
- `akit adapt`: conversions between the formats and from nginx configs and Caddyfiles.
- FastCGI.

**Exit criteria**

- The cache matches nginx's `proxy_cache` on the hit-ratio scenarios and survives a stampede with
  one upstream fetch.
- A header-editing WASM plugin adds a bounded, measured cost; the same `.wasm` runs in fit and akit.
- The nginx converter handles fit's sample configs and lists every directive it skipped.

**akio parts**: the logical pool for blocking work; `Files` as a model for the memory tier.

### 1.0

1.0 is a promise of stability, not a feature list.

- The config schema and the Admin API are v1, the same v1 as fit's.
- Fuzzing runs on the config readers, the router, the proxy and the cache.
- An external security review of TLS handling, the Admin API and the plugin sandbox.
- Docs: getting started, a config reference generated from the code, and migration guides from fit,
  nginx, Caddy and Traefik.

## After 1.0

fit's tracks beyond the edge proxy, in the order akit takes them. Each becomes a section of the
[development plan](dev-plan.md) when it starts. Most protocol work is already in akio; what is left
is akit's config, roles, Admin API and workbench pages, as in fit.

| Track | What it delivers (fit's docs) | akio parts | Status |
| --- | --- | --- | --- |
| Logging | log files rotated by size and time, retention, time zones ([fit](https://github.com/lesismal/fit/blob/main/docs/configuration.md)) | — | planned |
| Access | allow and deny lists, global and per listener, route, service and role | accept-time filter (A3) | planned |
| Console and encrypted store | `admin.store`, `admin.console`, logins, `akit store export/import` ([fit](https://github.com/lesismal/fit/blob/main/docs/console.md)) | `akio-crypto` | planned |
| Tunnel and LLM | services behind NAT through a hub; AI models shared among machines ([tunnel](https://github.com/lesismal/fit/blob/main/docs/tunnel.md), [llm](https://github.com/lesismal/fit/blob/main/docs/llm.md)) | `akio-ws`, `akio-quic`, the HTTP clients | planned |
| WebRTC | TURN server; WHIP and WHEP as SFU and as proxy ([fit](https://github.com/lesismal/fit/blob/main/docs/webrtc.md)) | `akio-turn`, `akio-webrtc`, `akio-ice` | planned |
| Streaming | RTMP, RTSP, SRT, HTTP-FLV, HLS, WHIP/WHEP behind `media_auth` ([fit](https://github.com/lesismal/fit/blob/main/docs/streaming.md)) | `akio-media` | planned |
| Media library and channels | VOD over HLS/fMP4 without transcoding, channels, signed playback URLs ([fit](https://github.com/lesismal/fit/blob/main/docs/media-library.md)) | `akio-media`; an MP4 reader (to request) | planned |
| Cluster management | one akit managing others from the workbench, rollouts with health gates ([fit](https://github.com/lesismal/fit/blob/main/docs/cluster.md)) | the HTTP client, `akio-ws` | planned |
| AI assistant | the workbench's chat panel and MCP server on the user's model APIs | the HTTP client | planned |
| BitTorrent | client, `bt_stream`, tracker, publishing ([fit](https://github.com/lesismal/fit/blob/main/docs/bittorrent.md)) | `akio-bittorrent` | planned |
| Sync | pipelines from sources to sinks with checkpoints, exactly once into transactional sinks ([fit](https://github.com/lesismal/fit/blob/main/docs/sync.md)) | the HTTP client; wire protocols written in akit | planned |
| Prefork and NUMA | data plane in several processes placed on NUMA nodes | `akio-prefork`, `Engine::numa_status` | on hold (2026-11-25): akit does not use prefork; akio has stopped testing `akio-prefork` and decides later whether to delete it |
| P2P and VPN | hosts behind NAT connected through ICE or TURN; an overlay network over TUN | `akio-ice`, `akio-turn`; TUN devices (to request) | follows fit: starts once fit's track is done |

## Feature map

Rows follow fit's [feature map](https://github.com/lesismal/fit/blob/main/docs/roadmap.md#feature-map). "akio" says
whether akio has what the feature needs today: ✓ ready, a request ID from
[akio API requests](akio-api-requests.md), or — when it is akit's own work.

| Feature | Milestone | akio parts | akio |
| --- | --- | --- | --- |
| HTTP/1.1, HTTP/2 | M0, M1 | `akio-http` | ✓ |
| HTTP/3 (QUIC) | M1 | `akio-h3`, `akio-quic` | ✓ |
| h2c upstreams, gRPC proxying | M1, M3 | HTTP/2 client, `akio-grpc` | ✓ |
| WebSocket proxying | M3 | `akio-ws`, upgrade | ✓ |
| TCP/UDP proxying | M3 | `Conn`, `hold_reads`, sockmap relay | ✓ |
| TLS routing by SNI | M3 | `akio-mux` | A5 |
| Several protocols on one port | M3 | `akio-mux` | ✓ |
| SOCKS and CONNECT | M3 | `akio-socks` | ✓ |
| PROXY protocol v1/v2 | M3 | — | A4 |
| Static files | M1 | `Files`, `sendfile` | ✓ |
| FastCGI | M5 | — | — |
| Load balancing, health checks | M1, M2 | HTTP clients | — |
| Config changes without interruption | M1, M2 | runtime listeners | A1 |
| Automatic HTTPS, on-demand TLS | M2 | `akio-tls`, `akio-x509` | A2 |
| Response caching | M5 | logical pool | — |
| Rate and concurrency limits | M3 | `limiter` middleware as a start | ✓ |
| Authentication: Basic, ForwardAuth, JWT, mTLS | M3 | `akio-tls`, `akio-crypto` | ✓ |
| Compression: gzip, zstd, brotli | M3 | `akio-deflate`, `akio-zstd`, `akio-brotli` | ✓ |
| Kubernetes, Docker, Consul, etcd | M4 | HTTP clients | — |
| Prometheus, OpenTelemetry, dashboard | M2, M4 | `Engine::metrics`, telemetry | ✓ |
| Go modules → crates (`akit build`), WASM | M5 | — | — |
| nginx and Caddyfile converters | M5 | — | — |
| Allow and deny lists | after 1.0 | accept filter | A3 |
| TURN, WHIP and WHEP | after 1.0 | `akio-turn`, `akio-webrtc` | ✓ |
| RTMP, RTSP, SRT, FLV, HLS | after 1.0 | `akio-media` | ✓ |
| VOD from media files | after 1.0 | an MP4 reader | to request |
| BitTorrent client and tracker | after 1.0 | `akio-bittorrent` | ✓ |
| Prefork and NUMA | after 1.0, on hold | `akio-prefork` (on hold, not tested) | — |

## Performance targets

Measured on the benchmark host with fit's harness and scenarios, against nginx, Caddy, Traefik and
fit at the same commit as their baselines.

| Metric | Target |
| --- | --- |
| Memory per idle connection (HTTP/1.1, TLS, WebSocket) | below fit's; at or below nginx's |
| Throughput, static files | at or above nginx's |
| Throughput, reverse proxy | above fit's and nginx's |
| p99 latency under load | below fit's; flat while config is swapped |
| Allocations per request | measured per release; an increase needs a reason |
| Cost of a config swap | independent of the number of open connections |

## Risks and open questions

- **Config compatibility drift.** fit keeps adding fields. akit reads fit's schema from fit's own
  sources where it can (the generated config reference and templates), and its CI runs fit's
  templates and examples through `akit validate`, so a field fit adds shows up as an explicit
  "not supported yet" instead of drifting silently.
- **Gaps in akio.** The proxy needs runtime listeners (A1) and asynchronous certificate selection
  (A2) by M2. They are raised in akio first; until they land, M1 may use `Engine::adopt_tcp` with an
  accept thread of akit's own as a recorded stopgap.
- **Parsing the config formats without serde.** The lean-dependency rule argues for akit's own JSON,
  YAML-subset and .conf readers; serde would be faster to write. Settled in
  [decision 001](decisions/001-dependency-policy.md): own readers for .conf and JSON; YAML too, as
  [decision 005](decisions/005-yaml-reader.md) decided in I24.
- **ACME.** Written in akit on `akio-crypto` and `akio-x509` (JWS, CSR), as fit does on
  `x/crypto/acme`; akio may need CSR generation (to confirm in M2).
- **WASM runtime.** wasmtime is fast but large and needs `unsafe` and a JIT; wasmi is a pure-Rust
  interpreter. Decided in M5 behind akit's own trait, measured against fit's wazero.
- **Async-only ecosystems.** Kubernetes and etcd clients in Rust are mostly tokio based; akit writes
  the small part of their protocols it needs on akio's clients rather than run a second runtime.
- **License.** Settled: Apache-2.0, as fit.
