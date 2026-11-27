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
| `akit-admin` | the Admin API, the console and its logins (I89–I91), the file provider | `admin`, `app/console`, `login`, `app/workbench` |
| `akit-provider` | Docker, Kubernetes, Consul, etcd | `provider` |
| `akit-observe` | logs, metrics, tracing | `observe`, `logs` |
| `akit-cache` | the response cache | `handler/cache` |
| `akit-plugin` | WASM plugins | `plugin` |
| `akit-access` | access lists: allow and deny lists compiled into ranges, policies (I85) | `access` |
| `akit-roles` | the roles beside the gateway: link tokens (I99), the host that runs them (I100) | `app`, `internal/linktoken` |
| `akit-webrtc` | the WebRTC roles: the TURN server, WHIP and WHEP, the media proxy (I110–I114) | `webrtc` |
| `tests/e2e` | end-to-end tests of the binary | `test/e2e` |
| `tests/compat` | the fit config compatibility gate: pinned fixtures, field inventory | `templates`, fit-examples |
| `bench` | the harness, scenarios and reports | `bench` |

## Index

Status: `todo`, `doing` or `done`. The next free iteration number is I120 (I84 and I88, reserved for the
Logging and Access lists tracks, are unused; I95 and I96 are reserved for the Console track; I97
renamed fit's external names; I98–I109, I102b, I102c, I102d, I103b, I109b, I109c and I109d are the
Tunnel and LLM track; I110–I114 are the WebRTC track, and I115–I119 are reserved for it).

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
| I18b | HTTP/2 upstreams II: streams reset, timed and paced by akio | I18, akio A51–A53, A66 | done |
| I19 | Load balancing v1 | I14 | done |
| I20 | Passive health checks | I19 | done |
| I21 | Access logs | I15 | done |
| I22 | Reload on SIGHUP | I06 | done |
| I23 | The .conf reader | I02 | done |
| I24 | The YAML reader | I02 | done |
| I25 | M1 conformance and report | I09–I24 | done |
| **M2** | | | |
| I26 | Admin API I: load, read, versions | I22 | done |
| I27 | Admin API II: patch, rollback, auth | I26 | done |
| I28 | Listeners diffed at run time | I26, A1 | done |
| I29 | File provider | I26 | done |
| I30 | Certificate manager and storage | I09 | done |
| I31 | ACME I: account, orders, HTTP-01 | I30 | done |
| I32 | ACME II: TLS-ALPN-01, renewal, OCSP | I31 | done |
| I33 | Automatic HTTPS defaults | I32 | done |
| I34 | On-demand TLS | I33, A2 | done |
| I35 | Active health checks | I20 | done |
| I36 | Load balancing v2 | I19 | done |
| I37 | Prometheus metrics | I15 | done |
| I38 | M2 exit: churn test and staging certificate | I26–I37 | done |
| **M3** | | | |
| I39 | Matchers v2 and expressions | I11 | done |
| I40 | Rewrites, redirects, headers | I39 | done |
| I41 | Rate, concurrency and size limits | I39 | done |
| I42 | Basic auth and mTLS | I39 | done |
| I43 | ForwardAuth | I42 | done |
| I44 | JWT and JWKS | I42 | done |
| I45 | Retries, circuit breakers, route timeouts | I17 | done |
| I46 | Weighted services, mirroring, failover | I45 | done |
| I47 | Compression | I16 | done |
| I48 | The rest of fit's middlewares | I40 | done |
| I49 | WebSocket proxying | I17 | done |
| I50 | gRPC proxying | I18 | done |
| I51 | TCP and UDP proxying | I05 | done |
| I51b | The eBPF sockmap relay | I51 | done |
| I52 | SNI routing and PROXY protocol | I51, A4, A5 | done |
| I53 | Many protocols on one port | I52 | done |
| I54 | Upstream TLS | I18 | done |
| I55 | SOCKS and CONNECT | I53 | done |
| I56 | M3 exit | I39–I55 | done |
| **M4** | | | |
| I57 | Provider framework and the effective config | I29 | done |
| I58 | Docker provider | I57 | done |
| I59 | Kubernetes Ingress | I57 | done |
| I60 | Kubernetes Gateway API and conformance | I59 | done |
| I61 | Consul and etcd | I57 | done |
| I62 | Shared certificate storage | I30, I61 | done |
| I63 | Dashboard | I37 | done |
| I64 | OpenTelemetry | I37 | done |
| I65 | Packaging | I06 | done |
| I66 | M4 exit | I57–I65 | done |
| **M5** | | | |
| I67 | Cache I: memory tier and RFC 9111 | I17 | done |
| I68 | Cache II: disk tier, locks, stale | I67 | done |
| I69 | Cache III: purge and tags | I68, I27 | done |
| I70 | `akit build` | I03 | done |
| I71 | WASM I: runtime and ABI | I03 | done |
| I72 | WASM II: the Proxy-Wasm subset | I71 | done |
| I73 | `akit adapt` and `akit fmt` | I23, I24 | done |
| I74 | nginx and Caddyfile converters | I73 | done |
| I75 | FastCGI | I16 | done |
| I76 | M5 exit | I67–I75 | done |
| **1.0** | | | |
| I77 | Fuzzing | I56 | done |
| I78 | Config reference and guides | I76 | done |
| I79 | v1 freeze and compatibility | I78 | done |
| I79b | The gateway examples the gate still refuses: `limits.max_pending_bytes`, h2c alone, `max_conns` by accepted connections | I79, akio A214, A168, A215 | done |
| I80 | Security review readiness | I77, I79 | done |
| I80b | Security hardening: the threat model's limitations and the review's findings | I80, the review | doing |
| **After 1.0** | | | |
| I81 | Log files: rotation by size and time, retention, compression; `access_log.rotate` | I21 | done |
| I82 | akit's own log: fit's `log`, its loggers, formats, files and zone | I81 | done |
| I83 | Log files on reload, and the Admin API's `/logs` | I82, I27 | done |
| **After 1.0: access lists** | | | |
| I85 | Access lists I: the lists, listeners and routes | I79, akio A3 | done |
| I86 | Access lists II: services and the forward proxy | I85 | done |
| I87 | Access lists III: the Admin API and the reference | I85 | done |
| **After 1.0: console and encrypted store** | | | |
| I89 | Console I: `admin.console` and its logins: users, lockouts, sessions; `akit hash-password` | I87 | done |
| I90 | Console II: the console's server: TLS, access, CSRF and origins, the Admin API in process | I89, I27 | done |
| I91 | Console III: the pages: fit's workbench, the schema, the raw editors and transfer, the Logs page | I90, I83 | done |
| I92 | The Admin API over TLS, off loopback (L1) | I27 | done |
| I93 | The encrypted store I: the `fitstore` format, `admin.store`, `akit store export` and `import` | I90, akio A230, A231 | done |
| I93b | The encrypted store II: `akit run` keeps the config in it; the console's password changes; the `store` format of `/config/export` and `/config/import` | I93 | done |
| I93c | The encrypted store III: `akit run -admin`, the store beside the config files | I93b | done |
| I94 | The Admin API's and the console's certificates read again when they change | I92 | done |
| **After 1.0: akit's names** | | | |
| I97 | akit's names: every external name that says "fit" says "akit" (decision 037) | I91, I92 | done |
| **After 1.0: tunnel and LLM** | | | |
| I98 | Tunnel and LLM I: `tunnel` and `llm` in the config | I79 | done |
| I99 | Link tokens: `akitt1.` tokens, proofs, registry and management | I98 | done |
| I100 | The roles in `akit run`: started, moved and stopped on the listeners' engine; one address, one server; the Admin API's `/tokens` and `akit token` | I99, I27 | done |
| I101 | Tunnel I: links, registration and tunnels over TCP | I99, I100 | done |
| I102 | Tunnel II: TLS links; pins; the hub's certificate kept | I101 | done |
| I102b | Tunnel II b: QUIC links | I102 | done |
| I102c | Tunnel II c: WebSocket links | I102b | done |
| I102d | Tunnel II d: the hub's QUIC handshake bounds; a listener's certificate read again when its files change | I102c | done |
| I103 | Tunnel III: the hub's calls and status, token expiry | I102c | done |
| I103b | Tunnel III b: the console socket and web proxies | I103 | done |
| I104 | LLM I: the mesh: frames, sessions, streams with credit, the handshake, hello, pools | I99, I100 | done |
| I105 | LLM II: the node: models, checks, calls forwarded upstream streamed both ways | I104 | done |
| I106 | LLM III: the hub: node table, models, picking and relaying | I105 | done |
| I106b | LLM III b: the hub's calls for the pages (`/hub/nodes`, `/hub/node/call`, `/hub/node/stream`), the node list's byte bound, the routes' counts (`/mesh/status`) | I106 | done |
| I107 | LLM IV: the proxy: the agents' API, API keys, routing, errors | I106 | done |
| I108 | LLM V: the node's management: hardware, GPUs, fixed actions and jobs | I105 | done |
| I109 | The roles' pages: their consoles and methods, edits written back; the hub's pages (I109b, I109c split) | I103, I107, I91 | done |
| I109b | The tunnel's node's and proxy's pages: their status, hubs, services, forwards and meta | I109 | done |
| I109c | The LLM roles' pages: `/api/me`, their methods (`/api/rpc`) and streams (`/api/stream`, Server-Sent Events) | I109, I106b, I108 | done |
| I109d | The LLM roles' methods their pages call that they lack: `/mesh/*` (dial, undial, kick, listen, limits, tokens), the proxy's (`/proxy/*`: its API, keys, models, nodes, profile, stats); the identity's `http_url` | I109c | done |
| **After 1.0: WebRTC** | | | |
| I110 | WebRTC I: `webrtc` in the config: `webrtc.turn`, `webrtc.media`, `webrtc.proxy`; the handlers `whip`, `whep` and `webrtc_proxy` | I79, I100 | done |
| I111 | WebRTC II: the TURN server on akio-turn, over UDP, TCP and TLS | I110 | done |
| I112 | WebRTC III: the TURN server on akio's hooks: port range, bandwidth, events, limits in place, drain | I111, akio A256–A258 | done |
| I113 | WebRTC IV: WHIP and WHEP, an SFU on akio-webrtc's `RtcServer` | I110 | done |
| I114 | WebRTC V: WHIP and WHEP proxied: `webrtc_proxy` and `webrtc.proxy` | I113, I14 | todo |
| I150 | BitTorrent I: `bittorrent.client` and `bittorrent.tracker` in the config, fit's fields, checks and clashes; what later iterations run refused as not supported yet | I98 | todo |
| I150 | BitTorrent I: `bittorrent.client` and `bittorrent.tracker` in the config, fit's fields, checks and clashes; what later iterations run refused as not supported yet | I98 | done |
| I151 | BitTorrent II: the tracker role and `bt_tracker` on akio-bittorrent's `TrackerServer`: HTTP and UDP, the allow-list, access, bounds, statistics, its console | I150, I100 | done |
| I152 | BitTorrent III: the client role on akio-bittorrent's `Client`: the state in `data_dir`, restore without a check, reloads in place or by a new client, its API and console | I150, I151 | done |
| I153 | BitTorrent IV: adding by URL and `watch_dir`, categories, the queue, disk checks | I152 | todo |
| I154 | BitTorrent V: `alt_speed` schedules, per-torrent limits, the client's `access` and connection limits (akio A284) | I152 | todo |
| I155 | BitTorrent VI: `bt_stream`, a file played while it downloads | I152 | todo |
| I156 | BitTorrent VII: publishing a directory as torrents, `bt_publish` | I152 | todo |
| I157 | BitTorrent VIII: the Admin API's `/bittorrent/...`, events, `akit bt`, metrics | I153 | todo |
| I158 | BitTorrent IX: the client's and the tracker's pages | I157, I109 | todo |
| I159 | BitTorrent X: RSS auto-download, the tracker store instances share, private trackers' passkeys | I153, I151 | todo |
| **After 1.0: cluster management** | | | |
| I130 | Cluster I: `admin.node_id`, `admin.cluster` and `admin.cluster_member` in the config | I93c | done |
| I131 | Cluster II: what a member needs: `GET /node`, the node ID kept, `GET /schema` | I130 | todo |
| I132 | Cluster III: the manager (`akit-cluster`): members and credentials in the store, fit's document; the poller; `/api/m/cluster/nodes`, `/test`, `/state` | I131 | todo |
| I133 | Cluster IV: the Cluster pages: Overview, Nodes and a node's page; `docs/guides/cluster.md` | I132 | todo |
| I134 | Cluster V: a member's own pages opened from the manager (`/api/n/<member>/...`) | I133 | todo |
| I135 | Cluster VI: groups and operations on many | I133 | todo |
| I136 | Cluster VII: templates; `/node/managed` | I135 | todo |
| I137 | Cluster VIII: rollouts with health gates | I136 | todo |
| I138 | Cluster IX: drift and alerts; `/config/check` | I137 | todo |
| I139 | Cluster X: users, roles and the audit log | I137 | todo |
| I139b | Cluster XI: members behind NAT, linking out | I132 | todo |
| I139c | Cluster XII: metrics, logs and certificates across members; `akit cluster` | I138 | todo |

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
| Listeners and `respond`, HTTP/1.1 and h2c; idle connections | I05 | yes (2026-11-25, `9d9321e`, `bench/results/m1-2026-11-25-akio-fixes`; first 2026-11-24, `bench/results/baseline-2026-11-24`) | no | The M1 report (I25): fixed first over HTTP/1.1, HTTPS/1.1, h2c and h2; idle HTTP/1.1 1,144 bytes at 10k and 1,132 at 100k (fit 1,497 and 1,441, nginx 556 and 530). Left: a profile of both; fixed over h2c 7–8% below M0's baseline (a same-run A/B of `7c835ad` against `9d9321e`); idle HTTP/1.1 against nginx's; idle connections over h2c. |
| TLS entry points (HTTP/1.1 and h2 over TLS) | I09 | yes (2026-11-25, `9d9321e` with akio A172, `bench/results/m1-2026-11-25-akio-fixes`) | no | The M1 report: fixed and static first over HTTPS/1.1 and h2; idle TLS 12,942 bytes at 10k and 12,872 at 100k (fit 9,879 and 8,593, nginx 29,924 and 29,583). With akio as pinned (`dad77a7a`) the harness's wrk fails every handshake (A172). Left: handshakes a second against fit and nginx; idle TLS memory against fit's. |
| HTTP/3 entry point | I10 | no | no (not built yet) | Fixed over h3 against fit and Caddy. |
| Router v1: host and path lookup | I11 | no | no (built, not measured) | A micro-benchmark of the lookup (no allocation) and same-run A/B of fixed against I05's scan, with many routes. |
| Static files | I12, I13 | yes (2026-11-25, `9d9321e`, `bench/results/m1-2026-11-25-akio-fixes`) | no | The M1 report: first over HTTP/1.1, HTTPS/1.1 and h2 (1.2 to 3.8 times nginx's), 98% of fit over h2c. Left: precompressed twins and `sendfile` for large files; cache hits after akio's ADR-124 (the two-ring cache, `max_cache_bytes` evictions with many files); large files with `compress`, read and encoded on akio's file pool, against fit. |
| Upstream pools | I14 | yes (2026-11-25, through the proxy scenario, `bench/results/m1-2026-11-25-akio-fixes`) | no | The M1 report: connections reused, about 220 to the backend for 256 clients over HTTP/1.1. Left: waiting for a connection under a limit. |
| Reverse proxy: HTTP/1.1, streamed bodies, h2 and h2c upstreams | I15–I18 | yes (2026-11-25, `9d9321e` with akio A171, `bench/results/m1-2026-11-25-akio-fixes`) | no | The M1 report: third behind fit and nginx, 245k requests a second over HTTP/1.1, 230k over HTTPS/1.1, 238k over h2c, 221k over h2 (86–93% of the best), 27.5 µs of CPU a request against nginx's 18.2, p99 3.16 ms over HTTP/1.1 against fit's 2.20; with akio as pinned its memory grows by about 240 bytes a request of the route's timeout window (A171). Left: a profile of the per-request cost; large bodies both ways, with memory per request in flight; requests with trailers (I17's `Trailer` parsing per request); h2c upstreams (I18): requests a second and CPU a request against an HTTP/1.1 upstream, with many clients multiplexed on few connections, and the per-address lock that leases a stream (`akit_upstream::h2`); I17b's `CancelToken` and `on_gone` per proxied request; over h2c (I18b) akio's per-stream state and its header and idle timers (a clock read per DATA frame, a timer job per request), a held download and a paced upload against HTTP/1.1's. |
| Load balancing v1 and v2 | I19, I36 | no | no (built, not measured) | A micro-benchmark of each policy's pick (fit's `BenchmarkPick` and `BenchmarkPickNew`: the nine policies, a key hashed, a sticky cookie looked up), 1 and 16 threads; proxy A/B against a single server; same-run A/B of the proxy scenario with `round_robin` against `p2c`, `header_hash` and a `sticky_cookie` (the per-request key hashing and, for a first request, the `Set-Cookie` copied). |
| Passive health checks | I20 | no | no (built, not measured) | A micro-benchmark of recording a result on the request path. |
| Active health checks | I35 | no | no (built, not measured) | Off the hot path (a pick still reads one atomic per address): the probes' cost on a busy engine, thousands of addresses probed every second (http and tcp), with the probe timer's thread (the stopgap for akio A45); same-run A/B of the proxy scenario with a health check on and off. |
| Access logs | I21 | no | no (built, not measured) | Same-run A/B of fixed and proxy with logs on and off (json, and text or line), to a file and to `/dev/null`; lines dropped at full load with the default 4 MiB buffer and a slow disk; the per-request cost (the boxed response filter, the shard lock, formatting). |
| Log files: the sink, rotation, retention, compression | I81 | no | no (built, not measured) | Same-run A/B of fixed and proxy with the access log to a file with `rotate` (`max_bytes` 1 MiB, so it turns often, `compress` on and off) against the same file without it, against main before I81: the access log's writer hands each flush to the sink's queue (no copy) and the sink's thread writes it; lines dropped at full load with a slow disk (the sink's 1 MiB queue behind the log's 4 MiB buffer); the cost of a rotation under load (a rename, an open, a directory listed for retention) and of gzip on the compression thread (CPU per MiB, the queue of 16 under many rotations). |
| Certificates chosen per handshake (the resolver reading the graph) | I30 | no | no (built, not measured) | Full TLS handshakes a second with one and ten certificates, against I09's `sni_cert` and fit. |
| The HTTP-01 check on listeners in the clear | I31 | no | no (built, not measured) | Fixed over HTTP/1.1 and h2c, same-run A/B against main before I31 (a prefix comparison per request under `akit run`). |
| The certificate choice with TLS-ALPN-01 and OCSP staples | I32 | no | no (built, not measured) | Full TLS handshakes a second against I31, with and without a stapled response (the ALPN list read, and a second `Published` map for files with responders); the handshake's size with a response. |
| Prometheus metrics | I37 | no | no (built, not measured) | Same-run A/B of the fixed and proxy scenarios with `metrics` on and off (an inline route costs a thread-local read and three atomic adds; a proxied one also a boxed response filter until akio A29), and with the access log on too (one hook for both); a scrape with many routes and 64 threads (the per-thread shards). |
| OpenTelemetry | I64 | no | no (built, not measured) | Same-run A/B of the fixed and proxy scenarios with `telemetry.traces` off, on with `sample_ratio` 0 (a field lookup a request), and on with 1 (every request: its ids, a `traceparent` local, the span encoded into a thread's buffer and a shard lock); spans dropped at full load with the default 4 MiB queue and a slow collector; the export thread's cost; a micro-benchmark of `Tracer::sample` and `Tracer::end` (fit's `BenchmarkSample`, `BenchmarkEnd`). |
| Static musl release binaries | I65 | no | no (built, not measured) | The fixed and proxy scenarios with the release's static musl binary (`scripts/dist.sh`, musl's allocator) against a glibc build of the same commit, same-run A/B, alternating rounds: whether musl's allocator costs throughput or memory under many threads, and so whether a global allocator is worth a dependency ([decision 009](decisions/009-packaging.md)). |
| M4 exit: the full harness at the milestone's end; providers at rest | I66 | no | no | M4 changed no request path. Left for the benchmark task (rule 24's milestone end): the full harness with M4's build against fit, nginx, Caddy and Traefik, ranked as the [M4 report](../bench/results/m4-report-2026-11-25.md) ranks; same-run A/B, alternating rounds, of a serving akit with a Kubernetes or Docker provider watching but idle against the same config without one, and requests' p99 while a provider applies a new version every second. |
| Matchers v2 and expressions | I39 | no | no (built, not measured) | A micro-benchmark of `find_request` with matchers, expressions and path regexps (no allocation, `tests/no_alloc.rs`); same-run A/B against I39's base on the fixed scenario (one route, no matchers: within noise expected). |
| Rewrites, redirects, headers | I40 | partial (2026-11-25, `2a1422d`, [M3 report](../bench/results/m3-report-2026-11-25.md): `rewrite` +0.4, `headers` +0.5 µs a request on epoll (fit +0.3, +1.0); `redirect` 815k a second (fit 765k)) | no (built, not measured) | Same-run A/B on the proxy scenario of a route with `headers` (request and response changes) and `rewrite` against the same route without them; the cost per request of the response filter (a boxed closure and an `Arc` clone) and of the request fields kept in a context local (the stopgap until akio API request A33). |
| Rate, concurrency and size limits | I41 | partial (2026-11-25, `2a1422d`, [M3 report](../bench/results/m3-report-2026-11-25.md): `rate_limit` +0.2, `concurrency_limit` +0.4, `body_limit` -0.1 µs a request on epoll (fit +0.4, +0.4, +0.3), one key, 256 connections) | no (built, not measured) | Same-run A/B on the proxy scenario of a route with each limit set high enough never to refuse, against the route without it: `rate_limit` by ip and by header (a shard lock, a SipHash, a map lookup), with 1, 1000 and 1000000 clients' keys and 64 threads (the shards' contention), as fit's `BenchmarkRate`; `concurrency_limit` (two atomic operations on one shared line, and a boxed response filter with an `Arc` clone until akio A48); `body_limit` (nothing per request but a field lookup). |
| Basic auth and client certificates | I42 | partial (2026-11-25, `2a1422d`, [M3 report](../bench/results/m3-report-2026-11-25.md): `basic_auth` with a remembered password +0.7 µs, `client_cert` without a certificate +0.0 on epoll (fit +0.6, +0.4), HTTP/1.1) | no (built, not measured) | Same-run A/B on the proxy scenario of a route with `basic_auth` (a remembered password: a base64 decode, an HMAC-SHA-256 and the request's fields rewritten into a context local until akio A33) and with `client_cert` and `subject_header` (the subject cached per thread), against the route without them, over HTTP/1.1, h2 and h3; the first check of a password (a suspension and a hop to akio's pool and back) at a few hundred new users a second, and a flood of wrong passwords against remembered users' latency. |
| Forward auth | I43 | partial (2026-11-25, `2a1422d`, [M3 report](../bench/results/m3-report-2026-11-25.md): +20.4 µs a request on epoll against fit's +17.0, 97% of fit's requests a second, a fast auth service (nginx)) | no (built, not measured) | Same-run A/B on the proxy scenario of a route with `forward_auth` against a fast local auth service, against the route without it, over HTTP/1.1 and h2: the cost of the check (the auth request's fields built, a suspension, one exchange on the auth upstream's pool, the resume on the answer's I/O thread); latency and memory with thousands of requests waiting on a slow auth service, and the fast route's latency meanwhile. |
| JWT and JWKS | I44 | partial (2026-11-25, `2a1422d`, [M3 report](../bench/results/m3-report-2026-11-25.md): HS256, a token kept verified: +1.9 µs a request on epoll, +2.4 on io_uring, against fit's +1.1) | no (built, not measured) | Same-run A/B on the proxy scenario of a route with `jwt` against the route without it, over HTTP/1.1 and h2, by algorithm (HS256, EdDSA, ES256, RS256), with the token kept verified (a SHA-256 and a per-thread map lookup, the claims fields rewritten into a context local until akio A33) and verified anew (many distinct tokens: the signature check on the I/O thread), as fit's `BenchmarkJWT`; the latency of requests waiting for a JWK set fetch, and of other routes meanwhile. |
| Retries, circuit breakers, route timeouts | I45 | partial (2026-11-25, `2a1422d`, [M3 report](../bench/results/m3-report-2026-11-25.md): `circuit_breaker`, upstream healthy: +0.7 µs a request on epoll against fit's +1.1) | no (built, not measured) | Same-run A/B on the proxy scenario of a route with `retry` and with `circuit_breaker` against the route without them, upstreams healthy: per request, the budget's read (a deposit into a full budget writes nothing), the template kept for a replay (a clone of the outgoing fields and of a body up to 64 KiB, a boxed `Again`), the breaker's boxed response filter and its two atomic adds on shared step counters, with 64 threads (the counters' contention); and with an upstream failing half its requests: retries a second against the budget, and the breaker's open path. |
| Weighted upstreams, mirroring, failover | I46 | no | no (built, not measured) | Same-run A/B on the proxy scenario of a route with `upstreams` (two, by weight) and with `failover` against the route with `upstream`: the draw by weight and `available` read per request; with `mirror` at 100% against a fast mirror (a copy of the fields and body, a second exchange per request) and against a mirror that answers nothing (the bound of 256 held: the requests' latency and akit's memory), over HTTP/1.1 and h2. |
| Middlewares (the rest) | I48 | yes (2026-11-25, `2a1422d`, [M3 report](../bench/results/m3-report-2026-11-25.md), `bench/modules.sh`) | no | Every middleware against fit's on the proxy scenario over HTTP/1.1, akit on epoll and io_uring, one config both serve: each adds what it adds to fit within a microsecond, but gzip (+6.7 µs against +3.9), `wasm` (+13.6 against +6.4) and `forward_auth` (+20.4 against +17.0). Left: the same over h2 and h3, and a profile of the two that lag. |
| Compression | I47 | partial (2026-11-25, `8413090`, [M3 report](../bench/results/m3-report-2026-11-25.md): gzip of 1 KiB of JSON at fit's default level: +6.7 µs a request against fit's +3.9; a response left alone +0.8 against +1.2; HTTP/1.1 only) | no (built, not measured) | Against fit, each encoding, as I47 asks: the proxy scenario with `compress` in front, a whole JSON body and a stream of server-sent events, over HTTP/1.1 and h2, per encoding at fit's default levels (responses a second, server CPU, bytes on the wire); the cost of the filter on responses it leaves alone (an image) against no middleware; the memory of held compressed streams (1,000 at once); the static handler's copies against fit's (`compress` on a file kept in memory). |
| Rewrite and redirect regexps | I48 | no | no (built, not measured) | Against fit: the proxy scenario behind a `rewrite` with a `regexp` and `replacement` (matched and not matched) and a `redirect` with groups in `to`, requests a second and allocations per request, against no middleware; akio-regex's `captures` and `replace_all` on path-sized texts against Go's `regexp`. |
| WebSocket and gRPC proxying | I49, I50 | partly (2026-11-25, idle WebSockets at 10k, `bench/results/m1-2026-11-25-akio-fixes`: 4,452 bytes each, fit 5,916, nginx 22,275) | no (I49 and I50 built) | Against fit: WebSocket messages a second (small, echoed) and bytes a second (large) through akit from HTTP/1.1, h2 and h3 clients, memory per open WebSocket (idle, and with 64 KiB pending each way), and handshakes a second (a key generated, the upstream connection leaving the pool); I50: gRPC calls a second (unary, h2c and h2 upstreams) and messages a second on a stream each way, against fit. |
| TCP and UDP proxying, SNI routing, many protocols on one port | I51–I53 | no | no (I51–I53 built, not measured) | Against fit and nginx's `stream`, as I51 asks: TCP throughput (fit's `BenchmarkTCPProxy`, 256 KB echoed at a time, direct and through akit), connections a second, memory per idle connection; UDP datagrams a second and sessions at once; the per-service idle sweep on akit's timer thread with 10,000 connections (the stopgap for akio A78); the cost of sniffing on a shared port (I53). I52: connections a second through a listener that routes by SNI (the ClientHello read and parsed before the dial) against one that does not, and with a PROXY header read on accept and one sent upstream; HTTP/1.1 and h2c request rates on a listener with `proxy_protocol` against one without, and akio's ADR-118 cost (a `OnceLock::get` per request) on a listener without. |
| The eBPF sockmap relay | I51b | no | no (I51b built, not measured) | On the benchmark host as root (Linux 6.15+), against the same listener without `sockmap` and against fit: TCP throughput (fit's `BenchmarkTCPProxy`, 256 KB echoed at a time) and CPU per gigabyte, connections a second (the splice's cost on each: two `TCP_INFO` reads, the map's two inserts and the gate), and the latency of a small echo; the idle sweep on akit's timer thread with 10,000 spliced connections (three syscalls a socket a sweep) and the drain's polling as many close at once. |
| LLM roles: hub, node, proxy | I105–I107 | no | no (built, not measured) | Against fit's roles (fit with fit, akit with akit, and mixed), an OpenAI-compatible stub upstream: calls a second through proxy → hub → node → upstream, small and with 3 MiB bodies (streamed past 1 MiB); a streamed answer's first byte and its events' latency; 256 calls at once; the hub's CPU per call relayed; memory per call in flight on each role; a slow agent holding its own call only. |
| LLM mesh | I104 | no | no (built, not measured) | Against fit's mesh (fit with fit, akit with akit, and mixed): calls a second over one link and over a pool of links; a stream's bytes a second through one link at 256 KiB windows, and 64 streams at once; a slow reader holding its own stream only while the link's others go on (the window and the link's 64 KiB high water with its held reads); gzip's CPU per frame of 1 KiB and more; memory per idle link and per stream. |
| Tunnel: hub, node, proxy | I101–I103 (I102b, I102c, I103b) | no | no (built, not measured) | Against fit's tunnel (fit's hub with fit's node and proxy, the same three in akit, and mixed): bytes a second through one tunnel and through 64 at once over one link, plain, sealed with `body_password` and over TLS (I102), QUIC (I102b: the stream's send buffer and the wire's overflow queue, `LINK_QUEUE` together) and WebSocket (I102c); tunnels opened a second (a proxy's accept, `tunnel/open` through the hub, the node's dial); the hub's CPU per gigabyte passed; memory per idle tunnel and per link; a slow service holding one tunnel while the link's others go on (the windows alone pace a link, its queue bounded at fit's `LinkQueue`, A234); I103b: bytes a second through a web proxy and the hub's console socket (sealed both ways, 64 KiB a message), against fit's, and the hub's CPU per gigabyte of it. |
| WebRTC: TURN, WHIP and WHEP, the proxy | I111–I114 | no | no (not built yet) | Against fit's roles (fit's TURN server and media server against akit's, the same clients): the TURN relay's datagrams and bytes a second through one allocation and 1,000 at once, by channel and by indication, over UDP, TCP and TLS, coturn alongside; allocations made a second (the credentials checked, a relay bound); the CPU per gigabyte relayed and memory per idle allocation. WHIP and WHEP: one publisher's 4 Mb/s video forwarded to 1, 100 and 256 viewers (the CPU per viewer, the forwarding's latency, packets dropped), PeerConnections answered a second, memory per session; a slow viewer holding none of the others. The proxy: datagrams a second relayed through its port, sessions answered a second, memory per relay. |
| Forward proxy: SOCKS and HTTP CONNECT | I55 | no | no (built, not measured) | Against fit (and nginx's `stream` for raw throughput): bytes a second through a SOCKS5 tunnel and an HTTP/1.1 CONNECT tunnel (256 KB echoed at a time, as fit's `BenchmarkTCPProxy`), tunnels opened a second (the handshake, the rules' checks, a name resolved on the pool and the `connected` check), memory per idle tunnel; the cost of SOCKS on a shared port (akio-mux's greeting rule beside HTTP and TLS) against HTTP alone; akio-socks's watchdog thread (the connect timeout added to it, ADR-119) with 10,000 tunnels. |
| Response cache | I67, I68, I69 | partial (2026-11-25, I76, `346ad8b`: the harness's `cache-hot` and `cache-zipf` against nginx and fit, and a stampede of 1,000 clients, in the [M5 report](../bench/results/m5-report-2026-11-25.md)) | no | Measured in I76: hits from memory through the harness (4 KiB; akit first, 1.5 times nginx over HTTP/1.1 and 6.2 times over h2c, 1.75 times for 100,000 paths, 12% to 23% ahead of fit) and one fetch for 1,000 clients on a cold key. Left, against fit, as I67 built it: hits from memory (a 4 KiB and a 1 MiB body, HTTP/1.1 and h2), misses kept through the proxy (a streamed body copied by the tap) against the proxy alone, the cost on requests the cache passes through (POST, `no-store` responses) against no middleware, the shards' locks under many keys and one hot key. I68: hits read back from disk (64 KiB and 1 MiB, the disk pool's threads and queue under many at once), writes to disk under a miss rate that fills `MAX_PENDING_WRITES`, a stampede of many clients on one key with the lock and with `disable_lock` (the waiters' suspend and resume, the timer per fetch), and stale-while-revalidate's background fetches. akio's ADR-116 tap: a tapped stream against an untapped one. I69: storing tagged responses (none and 32 tags) against untagged ones, and purges of a large zone by prefix and by tag while it serves (the shards' and the disk index's lock hold times). |
| FastCGI | I75 | no | no (built, not measured) | Against fit and nginx's `fastcgi_pass`, PHP-FPM (or a FastCGI application that answers at once) over TCP and a Unix-domain socket: requests a second and latency for a small page, HTTP/1.1 and h2; a 1 MiB upload and a 16 MiB download (memory per request in flight, the 64 KiB watermarks both ways); keep-alive reuse against a connection a request; the pool's queue under more clients than `max_conns`; the cost of `try_files` (a `stat` on akio's logical pool a candidate) against a route without it; the handler's sweep every 100 ms on akit's timer thread (A45's stopgap) over its exchanges at many requests a second. |
| WASM plugins | I71, I72 | partial (2026-11-25, I76, `346ad8b`: fit's `headers` plugin in akit and fit, HTTP/1.1 and h2c, in the [M5 report](../bench/results/m5-report-2026-11-25.md)) | no | Measured in I76 (`bench/run.sh -servers akit-wasm,fit-wasm -scenarios fixed,wasm`): fit's `headers` plugin costs akit 14.7 µs of CPU a request over HTTP/1.1, fit 6.8 µs (akio-wasm interprets, wazero compiles: akio API request A180, akio's W5); over h2c 2,048 requests in flight overrun the queue of 1,024 and both answer the rest 503 at once. Left: against fit (wazero's compiler) and none: a request through fit's `headers` plugin (Go guest) and through a no-op one (`fit_on_request` returning 0), HTTP/1.1 and h2, requests a second and latency, against the route without the middleware (the hand-off to the pool and back, the request copied, the call); the guest call alone on an instance (akio's W5: akio-wasm's interpreter against wazero's interpreter and compiler); the pool under more clients than `instances` (the queue, 503 past it); an instance remade after a trap (a Go guest's `_initialize`). Proxy-Wasm (I72): a request through the Proxy-Wasm Go SDK's `http_routing` (config `1`, the request unchanged) against fit and none, as fit's `BenchmarkProxyWasm`; a response through fit's `response` filter (`upper`) and the SDK's `http_headers`, the head held and the body filtered on the pool, a whole 4 KiB body and a streamed 1 MiB one, HTTP/1.1 and h2, against `response: none` and none. |
| Engine budget and client-only connection limit | I79b | partial (2026-11-26, akio `76a010c1`: akio's same-run A/B, in akio's docs/BENCHMARKS, "Engine budget and accepted-connection limit": HTTP/1.1, pipelined, h2c and TCP echo within 1% of akio's main on epoll and io_uring, with both settings off and on) | no (built, not measured in akit) | akio's side is measured. Left in akit: the fixed and proxy scenarios with the default `max_pending_bytes` (half the memory: the watcher thread, a relaxed load per read) against the same build with the budget removed (akio's `Live::set_max_pending_bytes(None)`), HTTP/1.1 and h2c; many slow clients (10,000 not reading a 1 MiB response) against fit: akit's RSS bounded by the budget, other clients' latency while it holds, and how fast they resume. |
| Config churn under traffic | I38 | no | no (tested, not measured) | The fixed and proxy scenarios, HTTP/1.1 and h2c, while the config is swapped every 10 ms through SIGHUP and the Admin API, against the same run with no swaps: requests a second, p99 latency and errors during the churn, and akit's RSS and descriptors after 100k swaps; the M2 report of the full harness (rule 24, a milestone's end). |
| Access lists | I85–I87 | no | no (built, not measured) | As fit's `BenchmarkAllows`: a policy's lookup (one list of 10, 10,000 and 1,000,000 prefixes from a file, and three lists chained) for IPv4 and IPv6 addresses, 1 and 16 threads, no allocation. Same-run A/B, alternating rounds, against the same config without `access`: connections a second through a listener refused or served at accept (akio's filter, A3: an `Arc` read and the lookup per accept) with a large deny list; the fixed and proxy scenarios over HTTP/1.1, h2c and h3 with a route's own access (checked per request: a graph read and the lookup) and without; a `tcp` service's connections a second decided by akit-l4's entry; a flood of refused `udp` datagrams (dropped at akio's filter, A220) against the served sessions' latency. |
| Console and encrypted store | I89–I93c | no | no (control plane) | The console is control plane (rule 24: no benchmark of its own), but it shares the host with the data plane. Same-run A/B, alternating rounds, of the fixed and proxy scenarios with and without a console being flooded with failed logins (bcrypt at cost 10 on its one-thread login pool, 16 waiting, the rest 503) and with a logged-in client polling `GET /api/m/admin/status` every second: the data plane's requests a second and p99 should not move. The console's own numbers, for the record: logins a second at costs 10 and 12; the workbench's first load (its 2.5 MB of files, from memory) over HTTP/1.1 and TLS. For I93: the store's key derivation at start (Argon2id at fit's 3, 64 MiB, 4 threads: time and peak memory), and a write's latency with a 4 MiB config (sealed, synced, renamed). |
| Cluster management: the manager's poller and calls to members | I132–I139c | no | no (control plane) | Control plane (rule 24), sharing the host with the data plane. Same-run A/B, alternating rounds, of the fixed and proxy scenarios on a manager polling 256 stand-in members every second (`max_concurrency` 8, `/node` and `/status` each) against none, and on a member polled every second against one not polled: requests a second and p99 should not move. For the record: a poll round's time for 256 members; a rolling rollout over 50 members in batches of 10 with 5 s gates, end to end; the manager's memory with 256 members and their states. |
| akio requests moved off their stopgaps (A12, A79, A45, A117, A30, A54, A29, A75, A78, A22, A170, A24, A49, A35, A148) | — (akio requests) | no | no (built, not measured) | Same-run A/B of the fixed and proxy scenarios, HTTP/1.1, h2c and h3, against main before these commits: the access log and metrics through the listener's response observer (`set_observed`, no allocation) against the boxed filter, and `limit_body` called on every request; the proxy's downloads framed by their length on HTTP/1.1 against chunked; layer 4 relays timed out by akio's per-connection idle timeout against akit's sweep (many idle connections); the sockmap relay through `akio::ebpf::Tunnel` (TCP throughput as root, as I51b's row); a health-check probe load on `akio::timer::after_send` against akit's old timer thread. |
| BitTorrent: client and tracker | I151, I152 | no | no (not built yet) | Against fit's roles (fit with fit, akit with akit, and mixed), on loopback and between two hosts: one torrent's download in MiB a second from one seeder and from several (TCP and uTP, encrypted and plain), the disk pool's queue under a slow disk, a restart's time to seeding with resume data and without, memory per torrent and per peer; the tracker's announces a second over HTTP and UDP with 100,000 torrents and 500,000 peers, and its memory; fit's `bench/bt.sh` against anacrolix and libtorrent as fit's I205 ran it. |

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
  request A66) was checked by `akit_upstream::h2`'s tick until I18b, which hands them to akio.

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
- **Done** ([the M1 report](../bench/results/m1-report-2026-11-25.md)): h2spec 145/145; the
  conformance suite, `crates/akit/tests/conformance.rs`, passes (with akit-proxy's stopgap for
  A169); the benchmarks and idle memory at 10k and 100k are published, HTTPS/1.1 and the proxy's
  memory with akio's A172 and A171 (on akio branches, not pinned yet). Of the performance targets,
  static files meet theirs; the proxy's throughput and p99 and idle HTTP/1.1 and TLS memory do not
  yet.

## M2, dynamic config and TLS

### I26 Admin API I: load, read, versions

- **Do**: the Admin API on localhost or a Unix socket: load a whole config, read it, versions and
  ETags, with fit's paths and payloads.
- **Done when**: fit's Admin API tests for these paths, ported, pass.
- **Touches**: `akit-admin`.
- **fit**: `admin`, decision 005.
- **Done** (decision 006): on an akio engine of its own, loads applied by the control thread
  through I22's reload; a Unix socket since akio A23 (2026-11-25), closing a connection after a
  refused body waits on A24.

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
- **Done**: fit's `acme.on_demand` and `tls.on_demand`; the handshake's `CertDone` waits while the
  ask (akio-http's asynchronous client) and the ACME workers take the name on; fit's bounds on
  asks, rate, names and idle names; tests against Pebble through real handshakes.

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
- **Done**: the churn test (`crates/akit/tests/churn.rs`: 2000 swaps through SIGHUP and the
  Admin API under `akit run`, and through `Listeners::reload` in process on epoll and io_uring)
  and the health window test (`health_window.rs`) pass; a config with only a host name gets its
  certificate from Pebble (in Docker on the benchmark host) and serves HTTPS through `akit run`
  (`acme_m2.rs`). The churn found that every reload made every upstream's pool again (dialing
  each anew, its health reset): an unchanged upstream now keeps its pool, as fit's does. The same
  test against Let's Encrypt's staging CA (`AKIT_ACME_STAGING_NAME`) passed on a public host for
  a public name on 2026-11-26, the certificate issued by `(STAGING) Baloney Bulgur YE2`
  ([progress](progress.md)).

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
- **Touches**: `akit-upstream`, `akit-proxy`, `akit-config`.

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
- **Built on**: akio API request A32 (ADR-115), added to akio by this iteration. What fit had that
  akit lacked, besides `cache` (I67) and `wasm` (I71), was `rewrite`'s `regexp` and `replacement`
  and the groups of `redirect`'s regexp in `to`.

### I49 WebSocket proxying

- **Do**: WebSocket proxied with backpressure both ways, from HTTP/1.1, HTTP/2 and HTTP/3 entry
  points to HTTP/1.1 upstreams.
- **Done when**: end-to-end tests with slow peers pass on each entry point.
- **Touches**: `akit-proxy`.
- **Read**: `akio-ws`.
- **Built on**: akio API requests A93 and A94 (ADR-113), added to akio by this iteration.

### I50 gRPC proxying

- **Do**: gRPC over h2c and TLS upstreams, trailers and deadlines carried, unary and streaming.
- **Done when**: end-to-end tests against grpc-go pass.
- **Touches**: `akit-proxy`.
- **Split**: the end-to-end tests are fit's, ported onto akio-grpc's client and server (fit runs
  them on fib's gRPC); a run against grpc-go, which needs a Go toolchain akit's CI does not
  have, is left to an interoperability task.

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
- **Status**: done (2026-11-25): `listeners[].sockmap`, an akit extension
  ([decision 010](decisions/010-sockmap-relay.md)); akio's relay as pinned, reached through the
  doc-hidden `Conn::inner` until akio API request A170; see the progress log.

### I52 SNI routing and PROXY protocol

- **Do**: TLS routed by SNI without termination; PROXY protocol v1/v2 accepted and sent. Depends on
  akio A4 and A5.
- **Done when**: tests cover both, including a truncated or forged PROXY header.
- **Touches**: `akit-l4`, `akit-listener`.
- **Built on**: akio API requests A4 and A5 (ADR-095), and A105 (ADR-118: a request's peer from
  the PROXY header), added to akio by this iteration.

### I53 Many protocols on one port

- **Do**: a listener split by akio-mux into HTTP, TLS (terminated or passed by SNI), gRPC, SOCKS and
  custom protocols, with sniff limits and timeouts.
- **Done when**: one port serves HTTP/1.1, HTTP/2, TLS passthrough by SNI and SOCKS at once.
- **Touches**: `akit-listener`.
- **Split**: SOCKS on the shared port goes to I55, which builds fit's forward proxy (`socks` and
  `listeners[].socks`) and adds its akio-mux route; I53 serves HTTP/1.1, HTTP/2 (h2c and h2),
  HTTPS, TLS passthrough by SNI and raw TCP on one port, as fit's `sniff` does without `socks`.
  gRPC is HTTP/2's; the custom protocols are what the service without `server_names` takes.

### I54 Upstream TLS

- **Do**: TLS to upstreams with custom CAs, client certificates and SNI overrides.
- **Done when**: tests cover each.
- **Touches**: `akit-upstream`.
- **Built on**: akio API requests A108 and A109 (ADR-117), added to akio by this iteration.

### I55 SOCKS and CONNECT

- **Do**: forward proxying with `akio-socks` and HTTP CONNECT, with auth and limits.
- **Done when**: tests cover both with slow peers; one port serves SOCKS beside HTTP, TLS and
  `tcp` (carried over from I53: a `Rule::Socks5`/`Rule::Socks4` route in akit-listener's `sniff`).
- **Touches**: `akit-l4`, `akit-listener`, `akit-handler` (`forward_proxy`), `akit-config`.
- **Built on**: akio API requests A132 and A133 (ADR-119: a tunnel's upstream checked once
  connected, by the name asked for and both addresses; a connect timeout of its own; the SOCKS
  client from a trusted PROXY header), added to akio by this iteration.
- **Split**: "auth" is fit's: its forward proxy authenticates no one, and decides by `clients`
  and the destination rules, as akit does. HTTP CONNECT over HTTP/2 and HTTP/3, which fit's
  `forward_proxy` tunnels too, came with akio API request A134 (2026-11-25); until then such a
  CONNECT was answered 501.

### I56 M3 exit

- **Do**: conformance tests and benchmarks for every middleware; the M3 report.
- **Done when**: the roadmap's M3 exit criteria hold.
- **Touches**: `tests`, `bench`.
- **Status**: done (2026-11-25): every criterion holds; the
  [M3 report](../bench/results/m3-report-2026-11-25.md) has the module benchmarks against fit's
  (`bench/modules.sh`) and the full harness. `crates/akit/tests/workers.rs` (fit's `test/workers`)
  checks that no module blocks an I/O thread; the slow-peer tests added are a WebSocket's slow
  upstream and UDP's. No akio request, no decision note.

## M4, platform integration

### I57 Provider framework and the effective config

- **Do**: providers feeding the same compile path; fit's merge rules; an Admin API endpoint showing
  the effective config and where each part came from.
- **Done when**: tests cover merging and the endpoint.
- **Touches**: `akit-provider`, `akit-admin`.
- **fit**: decision 009.

### I58 Docker provider

- **Do**: Docker labels as config, followed through the events stream on akio's HTTP client over the
  Unix socket.
- **Done when**: the Docker Compose example routes services as they start and stop.
- **Touches**: `akit-provider`.
- **fit**: decision 010.
- **Status**: done (2026-11-24), with akio API request A135 (ADR-120); see the progress log.

### I59 Kubernetes Ingress

- **Do**: a watch client of akit's own (list and watch over HTTP/2), Ingress to routes.
- **Done when**: tests against a kind cluster route by Ingress.
- **Touches**: `akit-provider`.
- **fit**: decision 011.
- **Status**: done (2026-11-24), with no akio API request; its tests run against a fake API server
  over HTTP/2 and HTTP/1.1, as fit's do, not a kind cluster; see the progress log.

### I60 Kubernetes Gateway API and conformance

- **Do**: HTTPRoute, GRPCRoute, TLSRoute, TCPRoute, UDPRoute; the conformance suite.
- **Done when**: akit passes the `GATEWAY-HTTP` profile.
- **Touches**: `akit-provider`.
- **fit**: decision 012.
- **Status**: done (2026-11-25), with no akio request; fit's translation, statuses and their tests
  ported, against the fake API server as fit's; the conformance suite against a kind cluster is
  written up in [tests/conformance](../tests/conformance/README.md) and left to the maintainer;
  see the progress log.

### I61 Consul and etcd

- **Do**: service catalogues and KV config from Consul (HTTP) and etcd (its v3 API through the
  JSON gateway etcd serves on its client port, as fit's; first planned on `akio-grpc`).
- **Done when**: tests against both follow changes.
- **Touches**: `akit-provider`.
- **fit**: decision 013.
- **Status**: done (2026-11-24), with no akio request: both over akio's HTTP client, Consul with
  blocking queries, etcd with a streamed watch; see the progress log.

### I62 Shared certificate storage

- **Do**: certificate storage in Redis, etcd or Consul with locks, so instances share issuance.
- **Done when**: two instances share one certificate with one issuance.
- **Touches**: `akit-tls`.
- **fit**: decision 014.
- **Status**: done (2026-11-25), with no akio request: Redis spoken in RESP2 over akio's
  connections, etcd and Consul over akio's HTTP client; challenges shared behind fit's marker of
  challenges in progress; see the progress log.

### I63 Dashboard

- **Do**: fit's read-only dashboard pages served by akit.
- **Done when**: the pages show routers, services, upstream health, certificates and versions.
- **Touches**: `akit-admin`.
- **Status**: done (2026-11-24), with no akio API request; see the progress log.

### I64 OpenTelemetry

- **Do**: tracing with context propagated to upstreams; OTLP export of traces and metrics, batched
  off the I/O threads.
- **Done when**: traces reach a collector in a test.
- **Touches**: `akit-observe`.
- **Status**: done (2026-11-24), with no akio request: fit's tracer on akio's HTTP client
  ([decision 007](decisions/007-opentelemetry.md)); see the progress log.

### I65 Packaging

- **Do**: static musl binaries for Linux (amd64, arm64) and macOS builds; a container image; a Helm
  chart and systemd units as fit's.
- **Done when**: the release workflow produces them with checksums.
- **Touches**: `deploy`, CI.
- **Status**: done (2026-11-25), with no akio request: static musl binaries linked by `rust-lld`
  ([decision 009](decisions/009-packaging.md)); see the progress log.

### I66 M4 exit

- **Do**: conformance and the Compose example in CI; the M4 report.
- **Done when**: the roadmap's M4 exit criteria hold.
- **Status**: done (2026-11-25), with no akit code change and no akio request: the Gateway API's
  `GATEWAY-HTTP` core passes at v1.3.0 (33 passed, 0 failed, 0 skipped,
  [the report](../tests/conformance/reports/gateway-http-v1.3.0-2026-11-25.yaml)), and the Docker
  Compose example ([deploy/compose](../deploy/compose/README.md)) routes by labels as its
  containers are scaled, stopped and started; both in CI; the
  [M4 report](../bench/results/m4-report-2026-11-25.md); see the progress log.

## M5, caching and extensions

### I67 Cache I: memory tier and RFC 9111

- **Do**: `Cache-Control`, `Vary`, validators, `Age`; a bounded memory tier.
- **Done when**: RFC 9111 tests pass.
- **Touches**: `akit-cache`.
- **Status**: done (2026-11-24), on akio API requests A111–A113 (ADR-116); see the progress log.

### I68 Cache II: disk tier, locks, stale

- **Do**: a disk tier on the logical pool; cache locks so one miss fetches; stale-while-revalidate and
  stale-if-error.
- **Done when**: a stampede test makes one upstream fetch; disk I/O never runs on an I/O thread.
- **Touches**: `akit-cache`.
- **Status**: done (2026-11-24); the disk pool on `akio-pool` directly, a stopgap for akio API
  request A117; see the progress log.

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
- **Status**: done (2026-11-25), on akio's `akio-wasm` (ADR-125, W1-W4; akio API request A165) and
  `Context::req_mut` (A33), [decision 008](decisions/008-wasm-runtime.md); the cost is left to the
  benchmark tasks (rule 30, the row above); see the progress log.

### I72 WASM II: the Proxy-Wasm subset

- **Do**: the Proxy-Wasm subset fit supports.
- **Done when**: fit's Proxy-Wasm tests, ported, pass.
- **Touches**: `akit-plugin`.
- **fit**: decisions 018, 023.
- **Status**: done (2026-11-25), on akio's held heads and body filters (ADR-128, akio API requests
  A166 and A167), [decision 011](decisions/011-proxy-wasm.md); the cost is left to the benchmark
  tasks (rule 30, the row above); see the progress log.

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
- **Status**: done (2026-11-24); its PHP-FPM test (fit's `TestPHPFPM`) ran in I76 on the benchmark
  host, against PHP 8.4.28's FPM in the `php:8.4-fpm` image, and passed on epoll and io_uring.

### I76 M5 exit

- **Do**: the cache against nginx's `proxy_cache`; the WASM cost; converter samples; the M5 report.
- **Done when**: the roadmap's M5 exit criteria hold.
- **Status**: done (2026-11-25). The [M5 report](../bench/results/m5-report-2026-11-25.md), with
  *Where akit stands*: every criterion holds. The cache keeps what nginx's `proxy_cache` keeps at
  1.5 to 6.2 times its rate and makes one fetch for a stampede of 1,000 (`bench/stampede.sh`); fit's
  `headers` plugin, the same `.wasm` in both, costs akit 14.7 µs a request, bounded, twice fit's
  (akio API request A180); every directive fit's nginx samples hold is converted or listed (a probe
  test). See the progress log.

## 1.0

### I77 Fuzzing

- **Do**: fuzz targets for the config readers, the router, the proxy's header handling, the PROXY
  protocol and the cache; run in CI for a fixed time and longer on a schedule.
- **Done when**: every parser has a target and the corpus is checked in.
- **Touches**: `fuzz/` (a cargo-fuzz workspace of its own, a target for each parser), a `fuzzing`
  feature and module in each crate it fuzzes, `.github/workflows/ci.yml` and `fuzz.yml`. The PROXY
  protocol and ClientHello parsers are akio-mux's, fuzzed in akio; akit fuzzes what it reads with
  them.

### I78 Config reference and guides

- **Do**: the config reference generated from the code; getting started; migration from fit, nginx,
  Caddy and Traefik; all bilingual.
- **Done when**: every field in the model appears in the reference.
- **Status**: done (2026-11-26). [The config reference](reference/config.md), its tables generated
  by `tools/config-reference.py` from fit's schema and the fields akit-config reads, held to
  `model.rs` by `reference_matches_akit_config`; [the guides](guides/README.md), migration from fit,
  nginx, Caddy and Traefik among them; every config example in the docs run through `akit validate`
  by `crates/akit/tests/docs_examples.rs`. See the progress log.

### I79 v1 freeze and compatibility

- **Do**: the config schema and the Admin API frozen as v1, the same as fit's v1; a compatibility
  document saying what stays stable.
- **Done when**: the compatibility gate passes on every fit template with no unsupported field.
- **Status**: done (2026-11-26). [The policy](compatibility.md#v1-what-stays-stable), fit's v1 for
  what akit serves; the v1 surface frozen in `tests/compat/v1` and held by `tools/v1-freeze.py` (the
  config's) and `v1_surface_is_frozen` (the Admin API's), each failing on a change not marked as a
  compatible addition ([decision 029](decisions/029-v1-freeze.md)); the old names of fit's renamed
  fields read with fit's warning; `limits.max_header_bytes`, `max_conns`, `max_write_queue_bytes`
  and `engine` read ([decision 028](decisions/028-engine-on-akio.md)). Every fit template passes
  with no field refused (6 of 6, as before); fit-examples' gateway examples that still refuse a
  field wait for akio (I79b); the rest are the After-1.0 tracks'. See the progress log.

### I79b The gateway examples the gate still refuses

- **Do**: `limits.max_pending_bytes` on akio's budget across connections (A214), so fit-examples'
  `limits` and `proxy` are valid; a listener of `h2c` alone (A168), so `grpc` is; `limits.max_conns`
  counting accepted connections only (A215).
- **Done when**: no fit-examples config outside the After-1.0 tracks is refused (the gate's report
  lists only `bittorrent`, `console`, `llm`, `logging`, media, `streaming`, `sync`, `tunnel` and
  `webrtc` examples), each field marked with `make v1-bless`.

### I80 Security review readiness

- **Do**: a threat model; `cargo audit` and `cargo deny` clean; the scope of the external review (TLS
  handling, the Admin API, the plugin sandbox).
- **Done when**: the review can start.
- **Status**: done (2026-11-26). [The threat model](security/threat-model.md) (assets, trust
  boundaries, each attack with the code and tests that answer it, 15 known limitations with their
  plans), [the review's scope](security/review-scope.md) (TLS handling, the Admin API and dashboard,
  the plugin sandbox, request parsing and header handling with I77's fuzz targets, the sockmap path;
  akio's internals and what akit does not build out of scope), and [SECURITY.md](../SECURITY.md)
  (reporting, supported versions). `cargo deny check` and `cargo audit -D warnings` clean, run on
  every change and daily by `.github/workflows/security.yml` with `tools/release-deps.py`, which
  holds the release binary to akio alone ([decision 030](decisions/030-supply-chain-checks.md)).
  On-demand TLS's `https://` ask URL fixed: its client had no CAs. See the progress log.

### I80b Security hardening

- **Do**: the [threat model's known limitations](security/threat-model.md#known-limitations) that are
  akit's, with the external review's findings as they come:
  - L2: `GET /config`, `/versions/{n}` and `/config/effective` redact inline secrets, as fit's
    console shows `[redacted]`.
  - L3: a warning for secret and key files readable by group or others; the storage directory
    tightened to 0700 and synced after a rename.
  - L4: a warning when a token or password is sent in the clear to an address off loopback (ACME,
    Consul, etcd, Redis, Docker over `tcp://`, a kubeconfig's `http://` server or
    `insecure-skip-tls-verify`).
  - L6: the forward proxy's guide says that private ranges are reachable unless `deny` lists them,
    and that it has no authentication.
  - L7: symbolic links out of a `static` root decided against fit's behaviour.
  - L8: revocation of client certificates (CRLs) on akio-tls's options.
  - L9: a digest for WASM modules, proposed to fit first ([decision 002](decisions/002-config-compatibility.md)).
  - L12: akio's head and body timeouts read from fit's fields, if fit has them.
  - L13: a BREACH note in the compression part of the reference.
  - L14: `POST /cache/purge` tested without the token and with an `Origin`.
  - L15: an external review of akio-crypto, akio-tls, akio-http and akio-wasm asked of akio.
  The rest are elsewhere: L1 and L5 in the Console track, L6's access lists in the Access lists
  track, L10 with akio A24, L11 with I79b.
- **Done when**: every finding of the review is fixed or recorded as accepted with its reason, and
  each akit limitation above is closed or moved to the plan that will close it.
- **Status**: doing (2026-11-26): the limitations are done, the review's findings wait for the
  review ([decision 035](decisions/035-admin-api-redacts-secrets.md)).
  - Closed: **L2** (the Admin API answers inline secrets as `[redacted]` by fit's `secretName`
    rule, and keeps a secret a change sends back as `[redacted]`); **L3** (secret files open to
    others warned of; the certificate storage's root and the directories it writes into tightened
    to 0700, the directory synced after a rename); **L4** (credentials and keys sent in the clear off
    loopback warned of, the Kubernetes provider's in its log); **L6** (the reference's
    `forward_proxy`; there is no forward proxy guide); **L7** (links followed as fit's, documented
    and tested); **L12** (fit has no such field: closed as it is); **L13** (the note at
    `compress`); **L14** (`cache_purge_guarded`).
  - Moved: **L8** and **L9** to fit (decision 002): `tls.client_auth.crl_files` and the `wasm`
    middleware's `module_sha256` proposed in decision 035; akio-tls already has CRLs
    (`client_crls`), so neither needs akio. **L15** asked of akio as
    [A225](akio-api-requests.md#a225-an-external-security-review-of-akios-cryptography-tls-http-parsers-and-webassembly).
  - Left: the external review's findings (none yet). Done when they are fixed or accepted.

## After 1.0

Each track is broken into iterations when it starts, from fit's iterations of the same track
(their numbers in fit's [development plan](https://github.com/lesismal/fit/blob/main/docs/dev-plan.md)). The order
below is the planned order; tracks with no dependency between them may run in parallel.

| Order | Track | fit's iterations | First steps in akit |
| --- | --- | --- | --- |
| 1 | Logging: files, rotation and retention | I155–I157 | done: [I81–I83](#logging-files-rotation-and-retention), log files with time zones, rotation by size and time, retention |
| 2 | Access lists | I195 | [I85–I87](#access-lists): the lists, listeners and routes; services and SOCKS; the Admin API |
| 3 | Console and encrypted store | I104, I157's page | [I89–I93c](#console-and-encrypted-store): the console's logins, server and pages (the Logs page on I83's `/logs` among them), the Admin API over TLS, the encrypted store |
| 4 | Tunnel and LLM | I105–I107, I159–I162 | [I98–I109](#tunnel-and-llm): the roles in the config, link tokens, the roles in `akit run`; the tunnel's links over TCP, TLS, WebSocket and QUIC; the LLM mesh on `akio-ws`, its node, hub and proxy; the roles' pages |
| 5 | WebRTC | I87, I108–I111 | [I110–I114](#webrtc): the roles in the config; TURN on `akio-turn`; WHIP and WHEP on `akio-webrtc`; WHIP and WHEP proxied |
| 6 | Streaming and media auth | I115–I117 | the streaming role on `akio-media`; `media_auth` |
| 7 | Media library and channels | I131–I139 | an MP4 reader (akio request); VOD; channels; signed URLs |
| 8 | Cluster management | I140–I150, I153 | [I130–I139c](#cluster-management): the config, what a member needs, the manager and its members, the pages, groups, templates, rollouts with health gates, drift and alerts, roles, members behind NAT, metrics and the CLI; I153 with the AI assistant track |
| 9 | AI assistant | I124–I130, I152–I154 | the chat panel, tools, MCP server |
| 10 | BitTorrent | I196–I210 | [I150–I159](#bittorrent): the roles in the config, the tracker and the client on `akio-bittorrent`, adding torrents, schedules and access, `bt_stream`, publishing, the Admin API and events, the pages, RSS |
| 11 | Sync | I163–I194, I211–I222 | the pipeline core and checkpoints, then connectors by fit's order |
| 12 | Prefork and NUMA | planned in fit | dropped (2026-11-25): akit does not use prefork, and akio removed `akio-prefork`. Not planned. |
| 13 | P2P and VPN | I84–I103 | after fit's track is done |

### Logging: files, rotation and retention

fit's I155–I157 ([fit's decision 034](https://github.com/lesismal/fit/blob/main/docs/decisions/034-log-files.md)),
in three iterations ([decision 033](decisions/033-log-files.md)). The workbench's Logs page of fit's
I157 waits for the Console track; its Admin API comes here.

### I81 Log files: rotation by size and time, retention, compression

- **Do**: decision 033. akit-observe's file sink, as fit's `logs.Files` and `logs.File`: one sink a
  path, shared by every log writing there, a queue of 1 MiB written by a thread of its own, a write
  refused and counted past it, never waited for. Rotation before a write that would take the file
  past `max_bytes` (a buffer larger than that gets a file of its own) and at the first write in a
  later period of `interval`, counted from midnight in the log's zone, whichever comes first; the
  rotated file named `<path>.<yyyy-mm-ddThh-mm-ss.mmm>` in UTC, `-N` when taken, `.gz` once gzipped
  (akio-deflate, on a thread of the set's own, a queue of 16; a failure keeps the plain file).
  Retention at open, after each rotation and every ten minutes: older than `max_age`, past
  `max_files`, then the oldest past `max_total_bytes`; only names a rotation made, never the open
  file. No link or second hard link at a name is written, read or removed through. `rotate` read
  as fit's `config.Rotate` (defaults 1 GiB, 720h, 1 GiB, `compress` on; 0 a limit off; an interval
  under 1s, or over 24h and not whole days, refused); `access_log.rotate`, the access log's batches
  handed to the sink as they are; a reload changes the access log's file's rotation and zone in
  place.
- **Done when**: a file rotates at `max_bytes` and at each period boundary in a zone (a fake clock),
  every line whole in exactly one file; retention by age, number and total size, oldest first,
  sparing the open file and names the sink did not make; gzip whose text is the lines; links and
  hard links refused; a full queue refuses and counts; `akit run` rotates an access log and a
  reload changes its retention; the config's checks; the reference, the gate and the v1 freeze
  (`access_log.rotate` marked as additions).
- **Touches**: `akit-observe` (`files`), `akit-config` (`log`), `akit` (`run`), `tests/compat`,
  `docs/reference`.
- **fit**: `logs/file.go`, `config/rotate.go`, `observe/accesslog.go`; I155, I156.
- **Status**: done (2026-11-26). See the progress log.

### I82 akit's own log: fit's `log`

- **Do**: fit's `log` (`level`, `levels`, `output`, `format`, `time_zone`, `rotate`, `loggers`
  with their own `level`, `output`, `format` and `rotate`), as fit's `logs.Root`: fit's loggers
  (`server`, `admin`, `tls`, `provider`, `telemetry`, `access_log`, `plugin`, `fastcgi`, `engine`,
  and fit's other names, accepted), each message below its logger's level not formatted, written in
  fit's `line`, `text` or `json` in the log's zone, debug and info to standard output and the rest
  to standard error when no output is set, a file through I81's sink. What akit's crates say about
  themselves (certificates, providers, telemetry, FastCGI's stderr, the sink's failures) goes
  through their loggers; akio's records (akio-log) through `engine`. A file shared with the access
  log is one sink; fit's checks: an unknown logger, a level set twice, two `rotate`s or two zones
  for one file, an unknown zone. `akit run` warns of a file output with no `rotate`.
- **Done when**: every format of every logger in the chosen zone; levels filter before formatting;
  a logger's file of its own rotates; the access log and a logger on one path share a sink and each
  line is whole; fit's `logging` example passes the gate; the config's checks; the reference.
- **Touches**: `akit-observe` (`logger`), `akit-config` (`log`), `akit`, the crates that wrote to
  standard error, `tests/compat`, `docs/reference`.
- **fit**: `logs/logs.go`, `config/log.go`; I155, I156.
- **Status**: done (2026-11-26). See the progress log.

### I83 Log files on reload, and the Admin API's `/logs`

- **Do**: a reload changes `log`'s levels, `time_zone` and every file's `rotate` in place, as fit's
  `Root.SetConfig`; a changed path or format is warned of as taking a restart. The Admin API's
  `GET /logs` (each file output of `log` and the access log: path, loggers, format, zone, `rotate`
  with its defaults, the current and rotated files with sizes and times, bounded, the lines dropped
  and the last error), `POST /logs/rotate` and `POST /logs/cleanup` (`dry_run`), a path not one of
  the running config's outputs refused, run on the sink's thread, never on an I/O thread; marked as
  v1 additions.
- **Done when**: a reload changes levels, zone and retention without a restart; a path or format
  change is warned of; the three endpoints with their bounds and refusals; the reference.
- **Touches**: `akit-observe`, `akit-admin`, `akit`, `tests/compat/v1`, `docs/reference`.
- **fit**: `admin/logs.go`, `logs.Root.SetConfig`; I155–I157.
- **Status**: done (2026-11-26). See the progress log. The track is done; fit's workbench Logs
  page waits for the Console track.

### Access lists

fit's iteration I195 (its [development plan](https://github.com/lesismal/fit/blob/main/docs/dev-plan.md),
its package `access` and [reference](https://github.com/lesismal/fit/blob/main/docs/reference/config.md)):
allow lists (whitelists) and deny lists (blacklists) of client addresses, a global `access` and one
on each listener, route and service and on the Admin API. Of the lists that apply, the most specific
that has a say decides: a route's, then its listener's, then the global one; a service's, then its
listener's, then the global one; the Admin API's, then the global one. Where the policy is known
when a connection is accepted, akio's accept filter (A3) refuses it then; where it is known only
later (a route or service with a list of its own, a PROXY header naming the client), the request is
answered 403 or the connection closed once its service is known. fit's roles (`streaming`,
`webrtc`, `tunnel`, `llm`, `bittorrent`) and its console take their `access` with their tracks.

### I85 Access lists I: the lists, listeners and routes

- **Do**: a crate `akit-access`, fit's package `access`: inline entries and list files (addresses
  and prefixes, ranges, eMule `ipfilter.dat` and PeerGuardian `.p2p` lines, at most 64 MiB a file)
  compiled into sorted, merged ranges of each family, looked up by binary search; a policy of the
  lists that apply, the most specific first. `access` read at the top level, on listeners and on
  routes, with fit's checks. The graph compiles each listener's policies as fit's `compileAccess`
  does: akio's accept filter (A3) when the listener's own list and the global one are all there is,
  set again by every reload on the listener it keeps, without binding anew; each HTTP request
  checked, and answered `403 forbidden` when refused, when a route of the listener has a list of its
  own or the listener takes PROXY headers. HTTP/3 has no accept filter in akio (A220): its requests
  are checked one by one. List files are read when the config is compiled, a file that cannot be
  read failing it, and watched with the certificate files (`-watch-certs`).
- **Done when**: fit's `access` tests (`TestPolicyLayers`, `TestChainEmpty`,
  `TestDenyBeatsAllowInOneList`, `TestEdges`, `TestSetAgainstLinear`, `TestFiles`,
  `TestCompileErrors`, `TestRangeOf`, `TestParseLine`, `TestP2PAndDATFiles`,
  `TestRangesAgainstLinear`) and `listener/access_test.go`'s HTTP ones (`TestAccessRefusedAtAccept`,
  `TestAccessRoute`, `TestAccessFollowsReload`, `TestAccessProxyProtocol`) pass ported, on epoll
  and io_uring, with HTTP/3 refused per request; the fields read, marked with `make v1-bless`.
- **Touches**: `akit-access` (new), `akit-config`, `akit-runtime`, `akit-listener`, `tests/compat`.
- **Read**: akio's `Listener::filter`, `Engine::set_listener_filter` (ADR-099).
- **fit**: `access`, `config` (`Access`, its `check`), `runtime/graph.go` (`compileAccess`),
  `listener` (I195).
- **Status**: done (2026-11-26). akit-access ([decision 034](decisions/034-access-lists.md)); `access`
  read at the top level, on listeners and on routes; at accept by akio's filter, swapped by each
  reload, or per request (403); HTTP/3 per request until akio's A220, at akio's filter on its UDP
  socket since (2026-11-27); list files watched with the certificates. A `udp` listener and a `tcp`
  one taking PROXY headers wait for I86. See the progress log.

### I86 Access lists II: services and the forward proxy

- **Do**: `services[].access`. A `tcp` listener of one service without `server_names`, on a port
  of its own and without PROXY headers, decided at accept by that service's policy; one whose
  services pick by server name, that shares its port with HTTP or SOCKS, or that takes PROXY headers
  decided by akit-l4's entry once it knows the service, the connection closed when refused; `udp`
  sessions refused as they open (akio filters no datagrams yet, A220: a refused client's datagram
  opens a session that is closed at once); the `socks` forward proxy checking its clients at their
  request where the listener decides later, as fit's `SOCKSAccess`.
- **Done when**: fit's `TestAccessService` passes ported (a `tcp` listener, and one shared with
  HTTP, the service overriding the global list both ways), with a `udp` service, a service picked
  by server name and SOCKS on a shared port; on epoll and io_uring.
- **Touches**: `akit-config`, `akit-runtime`, `akit-l4`, `akit-listener`, `tests/compat`.
- **fit**: `l4/entry.go` (`SetAccess`, `CheckAccess`), `l4/forward.go` (`NewSOCKS`), I195.
- **Status**: done (2026-11-26). `services[].access`; a lone service's `tcp` listener at accept,
  others by akit-l4's entry once it knows the service, PROXY-header listeners included; `udp`
  sessions closed as they open until akio's A220, their datagrams dropped at akio's filter since
  (2026-11-27); SOCKS clients at their request. See the progress log.

### I87 Access lists III: the Admin API and the reference

- **Do**: `admin.access`, decided before the global list, which applies to the Admin API (and the
  dashboard it serves) too; at accept, set again by every config applied though the rest of `admin`
  takes a restart; refused with an Admin API on a Unix socket, whose peers have no address and are
  never checked. The config reference's chapter on access lists, from fit's `schema/doc/access.md`:
  how the lists combine, where each is decided, the list files' formats, the CA's validation
  servers an allow list must serve for ACME's HTTP-01 and TLS-ALPN-01. Decision 034 for where akit
  decides differently from fit.
- **Done when**: fit's `admin/access_test.go` (`TestAdminAccess`, `TestAdminAccessFile`,
  `TestAdminAccessUnixSocket`) passes ported; the reference's examples pass `akit validate`.
- **Touches**: `akit-config`, `akit-admin`, `akit`, `docs/reference`, `docs/decisions`.
- **fit**: `admin/server.go` (`accessListener`), `admin/configs.go` (`setAccess`),
  `config/validate.go`, `schema/doc/access.md`.
- **Status**: done (2026-11-26). `admin.access` at accept, set again by every config applied;
  refused on a Unix socket; the reference's chapter, the Admin API's guide and decision 034 cover
  the whole track. The roles' and consoles' `access` come with their tracks. See the progress log.

### Console and encrypted store

fit's I104 ([fit's decision 027](https://github.com/lesismal/fit/blob/main/docs/decisions/027-console-and-store.md),
its [console guide](https://github.com/lesismal/fit/blob/main/docs/console.md)) and the workbench's
Logs page of its I157, on I83's `/logs`, in six iterations ([decision 036](decisions/036-console.md)), I94 split from I92,
I93b and I93c from I93.
`admin.console` is a server of its own, beside the Admin API, whose endpoints it calls in process
behind logins of its own; it serves fit's workbench pages as fit ships them. `admin.store` keeps the
config sealed in fit's `fitstore` format. The threat model's L1 (no Admin API over TLS, no remote
access) is closed by the console over TLS (I90) and the Admin API over TLS (I92); L5 (keys stored
unencrypted) is revisited here and stays accepted: fit's store holds the config and the console's
users, not the certificate storage, which akit shares with fit (I62). The console needs no store:
without one it changes the running config as the Admin API does, and password changes are refused.

### I89 Console I: `admin.console` and its logins

- **Do**: `admin.console` read as fit's `config.Console`: `address` (127.0.0.1:2020), `cert_file`
  and `key_file` (together; an address off loopback needs both), `users` (at least one, each a
  `username` and a bcrypt `password_hash`, no name twice), `session_timeout` (30m),
  `session_lifetime` (12h), `max_sessions` (64), `max_conns` (32), `access` (I85's lists);
  `mptcp` refused as not supported yet. A config with only `admin` and no listener or route is
  valid, as fit's with only its console. The logins, as fit's package `login`: a name that is no
  user's checked against a dummy hash, at most two checks at once; 5 failures from an address (an
  IPv6 /64) in 15 minutes lock it out for 15 minutes, doubled for each further lockout up to an
  hour, one check from an address at a time, 4,096 addresses kept; 10 failures in a row as a user
  slow its checks from 2 seconds, doubled, up to 5 minutes; sessions of 32 random bytes kept by
  their SHA-256, ended after `session_timeout` idle, `session_lifetime` in all, at logout or as
  the oldest past `max_sessions`; a CSRF token a session, compared in constant time. `akit
  hash-password` (`-cost`, 10): a bcrypt hash of the first line of standard input, asked twice from
  a terminal.
- **Done when**: fit's `login` tests pass ported, with a fake clock (`TestKey`, `TestCheck`,
  `TestSessions`, `TestLockout`, `TestAdmitOneAtATime`, `TestAdmitAttack`, `TestAdmitFullTable`,
  `TestUserBackoff`, `TestUserBackoffQuiet`, `TestAddressBackoff`, `TestAdmitForgetsQuiet`,
  `TestEndUnknown`, `TestCookieName`, `TestUsersAndPasswords`, `TestEndUserSessions`); the config's checks as fit's `checkConsole`;
  `hash-password` hashes that verify; the fields marked with `make v1-bless`.
- **Touches**: `akit-config`, `akit-admin` (`console/login`), `akit` (`hash-password`),
  `tests/compat`, `docs/reference`.
- **fit**: `config/config.go` (`Console`), `config/validate.go` (`checkConsole`), `login`,
  `cli` (`hash-password`); I104.
- **Status**: done (2026-11-26). The terminal's echo stayed on until akio's A233; raw mode and a
  `*` a character since (2026-11-27), as fit's. See the progress log.

### I90 Console II: the console's server

- **Do**: the console on an akio engine of its own, one I/O thread, as the Admin API (decision
  006), HTTP/1.1, over TLS from `cert_file` and `key_file` (TLS 1.2 or later); `max_conns` at
  accept; `admin.console.access`, then the global `access`, at accept, set again by every config
  applied, though the rest of `admin.console` takes a restart. fit's endpoints: `POST /api/login`
  (JSON, 4 KiB at most; checked on a bounded pool of its own, 1 thread and 16 waiting, then 503;
  429 with `Retry-After` and `retry_after` for a lockout), `POST /api/logout`, `GET /api/session`,
  `GET /api/workbench` (the user and the module `admin`), `/api/m/admin/<path>`: the Admin API's
  endpoints, called in process behind the session instead of the token, its configs applied as
  source `console`; `GET /api/m/admin/users`, `POST /api/m/admin/account/password` and
  `POST /api/m/admin/users/password` (409 without a store: I93). The session's cookie
  (`fit_console_<port>`, `HttpOnly`, `SameSite=Strict`, `Secure` over TLS; `akit_console_<port>`
  since I97), `X-Fit-Csrf` (`X-Akit-Csrf` since I97) on every
  request but a read, an `Origin` that is not the console's refused, fit's headers
  (Content-Security-Policy, `X-Frame-Options`, `Referrer-Policy`, `nosniff`, HSTS over TLS,
  `Cache-Control: no-store` on the API). The `admin` logger: who logged in and from where, each
  failure, each logout, each change a user made with the version it applied. `akit run` starts it
  with the Admin API.
- **Done when**: fit's `admin/console_test.go` (`TestConsoleLoginAndEdit`, `TestConsoleLoginRefused`,
  `TestConsoleSessions`, `TestConsoleFailureTableBounded`) and the cases of `app/console`'s tests
  that need neither the pages nor the store (`TestConsoleTLS`, `TestConsoleLoginQueue`, CSRF, other
  origins, the headers) pass ported; access at accept and after a reload; a config changed through
  the console runs and is logged; on epoll and io_uring.
- **Touches**: `akit-admin` (`console`), `akit` (`run`), `docs/guides`.
- **Read**: akio's `Pool` (A117), `Context::req_mut`, `Detached`.
- **fit**: `app/console`, `admin/console.go`, `login`; I104.
- **Status**: done (2026-11-26). Connections bounded as the Admin API's until akio's A232, by fit's
  rules since (2026-11-27). See
  the progress log.

### I91 Console III: the pages

- **Do**: fit's workbench (`app/workbench/web`, Apache-2.0, plain HTML, CSS and JavaScript with no
  build step) copied unchanged at the fit commit the gate pins, by a script, with its licence, and
  built into akit and served from memory as fit embeds it ([decision 036](decisions/036-console.md));
  `GET /api/m/admin/schema`, fit's `fit.schema.json` the gate pins. The raw editors and the transfer
  page, as Admin API endpoints the console calls: `POST /config/render` and `POST /config/parse`
  (`.conf`, `.yaml`, `.json`), `PUT /config/text` (`?members=`), `POST /config/export` and
  `POST /config/import` in the text formats (the `store` format waits for I93); marked as v1
  additions. The Logs page of fit's I157 through I83's `/logs`, `/logs/rotate` and `/logs/cleanup`,
  each action in the `admin` logger.
- **Done when**: the pages load in a browser, log in, read and save the config (a form edit, the
  JSON page, a `.conf` tab), roll back a version, and list, rotate and clean up the log files;
  fit's `admin/text_test.go` (`TestTextRenderParse`, `TestTextParseErrors`, `TestTextPartSave`,
  `TestTextWholeConfig`) and `transfer_test.go`'s cases for the text formats pass ported; the
  files served are fit's byte for byte.
- **Touches**: `akit-admin` (`console`, `text`), `tools`, `tests/compat/v1`, `docs/guides`.
- **fit**: `app/workbench`, `admin/check.go` (`renderText`, `parseText`, `putText`),
  `admin/transfer.go`; I104, I157.
- **Status**: done (2026-11-26). The pages are fit's, with its menus of features akit does not run
  yet (decision 036); unchanged until I97, which renamed fit's names in them (decision 037). See the
  progress log.

### I92 The Admin API over TLS, off loopback

- **Do**: `admin.cert_file`, `key_file` and `client_ca_file`, as fit's `checkAdminAddress`: the API
  served over TLS (1.2 or later), client certificates asked for and verified against
  `client_ca_file`; an address off loopback allowed with a token and TLS, refused without them;
  none of them with a Unix socket. The threat model's L1 closed.
- **Done when**: fit's `admin/tls_test.go` passes ported (`TestTLS`, `TestTLSClientCA`,
  `TestTLSStartErrors`; `TestTLSReload` moved to I94), and off loopback with and without a
  token; the fields marked with `make v1-bless`.
- **Touches**: `akit-config`, `akit-admin`, `docs/reference`, `docs/security`.
- **fit**: `admin/server.go`, `admin/tls.go`, `config/validate.go` (`checkAdminAddress`).
- **Status**: done (2026-11-26). The certificate files are read when akit starts; reading them
  again when they change is I94. See the progress log.

### I93 The encrypted store I: the format, `admin.store` and `akit store`

- **Do**: fit's `admin/store` byte for byte, as `akit_admin::store`: the `fitstore` format (a
  59-byte header, Argon2id's settings and salt, the payload sealed with XChaCha20-Poly1305, the
  header its additional data; versions 1 to 3 read, the sections fit keeps there kept as they
  are; the bounds on Argon2id's cost a file, and an imported file, may ask for), writes sealed
  with a new nonce, 0600, synced and renamed, exports sealed with a password of their own;
  `admin.store` (`path`, exactly one of `password`, `password_file`, `password_env`) read and
  checked as fit's; `akit store export` and `akit store import`, as fit's `fit store`.
- **Done when**: fit's `admin/store` tests pass ported; a store fit wrote (every version, fit's
  own Argon2id settings, an export) opens in akit, and one akit wrote opens in fit, both checked
  with fit's own code; `store export` and `import` round-trip.
- **Touches**: `akit-admin` (`store`), `akit-config`, `akit` (`store`), `tests/compat`, `docs`.
- **Read**: akio-crypto's `kdf::argon2id` ([A230](akio-api-requests.md#a230-argon2id-in-akio-crypto))
  and `chacha::XChaCha20Poly1305` ([A231](akio-api-requests.md#a231-xchacha20-poly1305-in-akio-crypto)).
- **fit**: `admin/store`, `cli/store.go`, `config` (`Store`, `checkStore`); I104.
- **Status**: done (2026-11-27), split from I93b and I93c: `akit run` refuses `admin.store` until
  I93b keeps the config in it. See the progress log.

### I93b The encrypted store II: `akit run` keeps the config in it

- **Do**: as fit's `cli/run.go`, `admin/bootstrap.go` and `admin/configs.go`: the store as the base
  config akit runs from, merged onto the config files at each start (its values winning) and
  written back, seeded from them when it does not exist; `admin` put in from the bootstrap file,
  never kept, shown or taken from a config sent while there is a store; every config applied
  (Admin API, console, rollback, SIGHUP, providers' base) written to it, one that cannot be
  written applied and said unsaved; SIGHUP reading it again; `-watch` refused with a store; the
  console's password changes (`POST /api/m/admin/account/password`, `/users/password`, which
  answer 409 without a store) kept in it, its users overriding the bootstrap file's; the Admin
  API's `/config/export` and `/config/import` with the `store` format (a password of their own or
  the running store's key, an import bounded as fit's `Unseal`). The key derived once, on the
  control thread at start; writes on the control thread.
- **Done when**: fit's `cli` store tests and `admin`'s store and transfer tests pass ported; a
  config changed in the console runs, is in the store and survives a restart; a wrong password or
  a changed byte stops akit from starting.
- **Touches**: `akit-admin` (`configs`, `console`, `server`), `akit` (`run`), `docs`.
- **fit**: `cli/run.go`, `cli/store.go`, `admin/bootstrap.go`, `admin/configs.go`,
  `admin/transfer.go`, `admin/console.go`; I104.
- **Status**: done (2026-11-27), with I93c. Key derivations for the Admin API's `store` exports and
  imports run on its pool, one at a time, an export with a password of its own too (fit bounds
  imports only); a password change is hashed and kept on the console's login pool. fit's
  `TestStoreCommand` case of `{"listeners": []}` is left out: akit takes an empty list of
  listeners. Besides fit's code writing and reading the stores (`tests/fitstore`), fit's binary and
  akit's run in turn from one store when `AKIT_FIT_BIN` names fit's. See the progress log.

### I93c The encrypted store III: `akit run -admin` and the store beside the config

- **Do**: as fit's 2026-11-04 changes to its decision 027: `-c` naming one `.store` file, used as
  the store; without `admin.store`, a store beside the config files (`site.yaml` gives
  `site.store`) used once it exists, its password an environment variable or a key file beside it
  (`<store>.key`, made 0600 and random when neither exists); `akit run -admin`: `admin` and
  `admin.console` with their defaults where the config leaves them out, the config kept in a
  store, and with no console user the user `admin` with the temporary password `admin`, to be
  changed within `-timeout`, a session with it limited to changing it.
- **Done when**: fit's `run -admin` tests pass ported.
- **Touches**: `akit` (`run`, `store`), `akit-admin` (`console`), `docs`.
- **fit**: `cli/run.go`, `cli/store.go`, `admin/bootstrap.go` (`WithDefaultConsole`); I104.
- **Status**: done (2026-11-27), with I93b. fit's names renamed as decision 037 renames them:
  `AKIT_STORE_PASSWORD`, `akit.store` with no config file; the key file `<store>.key` as fit's, so
  that fit and akit share a store and its key. See the progress log.

### I94 The Admin API's and the console's certificates read again when they change

- **Do**: as fit's `admin/tls.go`: the files of `admin.cert_file`, `key_file`, `client_ca_file` and
  `admin.console.cert_file`, `key_file` read again when they change (`-watch-certs`, akit's
  certificate watcher), the new certificate served to new connections without a restart, a file
  that fails to load logged and the old one kept.
- **Done when**: fit's `TestTLSReload` passes ported, for the API and the console.
- **Touches**: `akit-admin`, `akit` (`run`).
- **fit**: `admin/tls.go`.
- **Status**: done (2026-11-27), before I93, which I94 does not need. The files are looked at on the control thread (`-watch-certs`, every
  `-watch-interval`, and SIGHUP), not at the handshake as fit's (rule 1); the console's
  certificate is read again too, which fit's is not. See the progress log.

### akit's names

Not a track of fit's: the maintainer's decision that akit's external names are akit's
([decision 037](decisions/037-akit-names.md)), before akit's first release.

### I97 akit's names

- **Do**: every external name that says "fit" says "akit", with no alias: the Admin API's
  `Akit-Config-Version` and `Akit-Dry-Run`, the `Akit-Error` trailer, the console's
  `akit_console_<port>` and `X-Akit-Csrf`, the export's `akit-*` file names, the metrics `akit_*`,
  the OTLP scope and span attributes, `Cache-Status`'s `akit`, the WebAssembly ABI
  (`akit_on_request`, module `akit`), the nine defaults that name the gateway, the Ingress
  annotations `akit.lesismal.github.io/*`, `jwt`'s realm, the default config file `akit.json`,
  `adapt`'s format `akit`. The console's pages renamed by `tools/console-pages.py`'s deterministic
  list, checked by `make compat-check`; the WebAssembly test plugin rebuilt from fit's with the
  ABI's names renamed (`tools/plugin-guest.sh`); the v1 baselines written again; every doc.
- **Done when**: no external name says "fit" (the grep of decision 037's report); fit's names are
  refused (a fit-built plugin, the old header and cookie unread); the pages work in a browser
  against akit's server; the gate, the v1 check and the workspace pass on macOS, epoll and
  io_uring.
- **Touches**: every crate with a name, `tools`, `tests/compat/v1`, `bench`, `deploy`, `docs`.
- **Status**: done (2026-11-26). See the progress log.

### Tunnel and LLM

fit's I105–I107 and I159–I162 (its [tunnel](https://github.com/lesismal/fit/blob/main/docs/tunnel.md)
and [LLM](https://github.com/lesismal/fit/blob/main/docs/llm.md) guides, its decisions 028 and 035,
and its packages `tunnel`, `llm`, `internal/linktoken`, `config/tunnel.go` and `config/llm.go`), in
twelve iterations ([decision 038](decisions/038-tunnel-and-llm.md)). Six roles of the config:
`tunnel.hub`, `tunnel.node` and `tunnel.proxy` reach TCP services behind NAT through a hub;
`llm.hub`, `llm.node` and `llm.proxy` share AI models among machines and serve their APIs to agents.
They run on the listeners' akio engine beside the gateway's routes, or without any, on akio's TCP,
TLS, WebSocket (`akio-ws`), QUIC (`akio-quic`) and HTTP clients. Every external name akit shows is
akit's (decision 037: the tokens `akitt1.`, `X-Akit-Node`, the files akit writes); the wire
protocols' constants (ALPN, HKDF and HMAC labels, method names) are fit's, so that akit's roles
link with fit's (decision 038).

### I98 Tunnel and LLM I: `tunnel` and `llm` in the config

- **Do**: the six members read as strictly as the rest, with fit's fields, defaults, bounds and
  checks (`config/tunnel.go`, `config/llm.go`): the hub's `listeners`, `quic`, `console_socket`
  and bounds; nodes' and proxies' `hubs` with their endpoints (`tls://`, `wss://`, `quic://`,
  `tcp://`, `ws://`; a query, fragment or user refused), `pin` (its old name `quic_pin` read with a
  warning), `ca_file` and `quic_insecure`; `services`, `forwards` and `web_proxies`; the LLM roles'
  `id`, `name`, `listen`, `peers`, `limits`, `tokens`, a node's `models`, `settings` and `proxy`,
  a proxy's `api` and `api_keys`; each role's `access` and `console`. TLS off loopback, as fit's
  I160 refuses: a `tcp` or `ws` listener or endpoint off loopback without `body_password`,
  `quic_insecure` off loopback without it, an LLM listener or peer off loopback without TLS.
  `akit run` refuses a config with a role until the role's iteration runs it.
- **Done when**: fit's `config/tunnel_test.go`, `llm_test.go` and `roles_test.go` cases pass ported;
  fit-examples' `tunnel-*` and `llm-*` pass the gate; the reference lists every field; the v1
  freeze marks them as additions.
- **Touches**: `akit-config` (`tunnel`, `llm`), `akit`, `tests/compat`, `docs/reference`.
- **fit**: `config/tunnel.go`, `config/llm.go`, `config/linktoken.go`; I105, I159, I160.
- **Status**: done (2026-11-27). See the progress log.

### I99 Link tokens

- **Do**: decision 035 of fit's, as `internal/linktoken`, in a new crate `akit-roles`: a token
  `akitt1.<id>.<secret>` (16 hex digits, 256 random bits), its client, stored, server and body
  keys, the dialer's and the listener's proofs over a transcript of both nonces and the direction;
  the registry of a listener's tokens (members bound to the token they first registered with,
  expiry, last use, the links using each, at most 1,024 tokens and 256 members a token); the
  management the pages, the Admin API and the CLI share (issue, change the expiry, revoke, bind),
  as edits of a member's token list. The Admin API's `/tokens` and `akit token` come with the host
  that runs the roles (I100).
- **Done when**: fit's `linktoken_test.go` passes ported, and fit's own vectors byte for byte (a
  fit token's secret makes the same hash, body key and proofs).
- **Touches**: `akit-roles` (new, `linktoken`).
- **fit**: `internal/linktoken`; I159.
- **Status**: done (2026-11-27). See the progress log.

### I100 The roles in `akit run`

- **Do**: as fit's `app` (I105): a host that starts, moves and stops each role on every config
  applied after the listeners, a role's failure a warning; the roles' bindings added to the
  listeners' akio engine (a role's own engine nowhere), with its `access` set again on every
  apply; one address, one server (fit's I162): every address the config listens on, the roles'
  among them, normalized and checked once, the later of two refused with the earlier named. The
  Admin API's `GET /tokens`, `GET` and `POST /tokens/{member}`, `PATCH` and `DELETE
  /tokens/{member}/{id}` (fit's `admin/tokens.go`), each change an edit of the member applied as
  any config and logged to the `admin` logger, and `akit token create|list|expiry|revoke`, which
  calls them (fit's `cli/token.go`).
- **Done when**: a config starts, moves and stops a role (a stub role in the tests); a role's
  failure is a warning and the gateway runs; fit's `bindings_test.go` clashes are refused; a token
  is issued, listed, changed and revoked through the Admin API and the CLI, shown once and kept as
  a hash; the v1 freeze marks the endpoints as additions.
- **Touches**: `akit-roles`, `akit-config`, `akit-admin`, `akit`, `tests/compat/v1`.
- **fit**: `app/app.go`, `config/bindings.go`, `internal/bindcheck`, `admin/tokens.go`,
  `cli/token.go`; I105, I159, I162.
- **Status**: done (2026-11-27). See the progress log.

### I101 Tunnel I: links, registration and tunnels over TCP

- **Do**: fit's link protocol (`tunnel/internal/proto`, `link`, `bodycrypt`), byte for byte: the
  frames, the hello of versions 3 and 4, registration with a token and with the deprecated shared
  password, both proven both ways, `body_password`'s sealed frames; the hub (peers, links, routing a
  tunnel to the node it names and passing its frames), the node (its `services`, the only addresses
  it dials) and the proxy (`forwards`, a local listener each, tunnels spread over its hubs); each
  tunnel's 4 MiB window with `hold_reads` and `on_drain` on both TCP ends, checked by the hub; the
  roles' bounds and timeouts. Over `tcp://`.
- **Done when**: fit's `tunnel/e2e_test.go` cases over TCP pass ported (large transfers, slow
  services and proxies, cut connections, sealed and plain, a token of each role, a member's ID
  bound to its token, a forged or replayed proof refused).
- **Touches**: `akit-tunnel` (new), `akit`.
- **fit**: `tunnel/hub`, `tunnel/node`, `tunnel/proxy`, `tunnel/internal/{proto,link,bodycrypt,
  tunnel,uplink,common}`; I106, I159.
- **Status**: done (2026-11-27). See the progress log.

### I102 Tunnel II: TLS links and the hub's certificate

- **Do**: `tls://` links (akio-tls, TLS 1.3, ALPN `fit-tunnel`, no session resumed); a hub's
  certificate per listener (`cert_file` and `key_file`, read when the member is applied) or its
  own self-signed one, made once and kept beside the config (`akit.tunnel.hub.pem`, 0600) or
  given in `quic.cert` and `quic.key`, its fingerprint logged; a node's or proxy's `pin`,
  `ca_file` or the system's roots, given to the dial, never read from the address. The pin is
  checked once the handshake is done and before the link sends a byte (akio API request A235
  asks akio-tls to check a pinned certificate in the handshake).
- **Done when**: the e2e tests over TLS pass with a pin and with `ca_file`; a wrong pin opens
  nothing; a hub started again keeps its fingerprint; a fit hub and akit's node and proxy, and the
  other way round, link over TLS with a pin.
- **Touches**: `akit-tunnel` (`tls`, `hub`, `uplink`, `link`).
- **fit**: `tunnel/internal/transport/tls.go`, `internal/certpin`; I106, I160.
- **Status**: done (2026-11-27). See the progress log.

### I102b Tunnel II b: QUIC links

- **Do**: `quic://` links (akio-quic on a UDP socket bound on the listeners' engine at run time,
  ALPN `fit-tunnel`, one bidirectional stream per connection opened with fit's version byte,
  `idle_timeout` and `keep_alive`, the hub's links and handshakes bounding its connections); the
  stream's send buffer the link's high water, what passes it queued by the link's wire up to
  24 MiB and sent on akio-quic's `on_stream_writable`; `quic_insecure`; the pin checked as the
  TLS links' (A235).
- **Done when**: the e2e tests over QUIC pass (a pin, `quic_insecure`, a wrong pin opening
  nothing, transfers past the stream's buffer, several tunnels at once); a fit hub and akit's node
  and proxy, and the other way round, link over QUIC with a pin.
- **Touches**: `akit-tunnel` (`quic`, `hub`, `uplink`).
- **fit**: `tunnel/internal/transport/quic`; I106, I160.
- **Status**: done (2026-11-27). See the progress log.

### I102c Tunnel II c: WebSocket links

- **Do**: `wss://` and `ws://` links (akio-ws: a hub's `/tunnel` by default, 404 for another
  path, a handshake bounded by 10 s, the link's bytes in binary messages of at most 1 MiB).
- **Waits on**: akio API request A236 (a WebSocket's reads held and its drain heard): without
  them a WebSocket link can neither hold the peer nor resume its feeders (rules 8 and 9). Done
  (akio `6151f5c5`).
- **Done when**: the e2e tests over WebSocket and WebSocket over TLS pass, and against fit's hub,
  node and proxy.
- **Touches**: `akit-tunnel`.
- **fit**: `tunnel/internal/transport/ws`; I106, I160.
- **Status**: done (2026-11-27). See the progress log. The hub's QUIC handshake bounds and a
  listener's certificate read again moved to I102d.

### I102d Tunnel II d: the hub's QUIC handshake bounds; certificates read again

- **Do**: the hub's QUIC handshake bounds (`handshake_timeout`, `max_handshakes` with a Retry past
  half, as fit's), which akio-quic's server does not take yet (a request to write: its Retry is
  its own, under load, and a handshake is bounded by the idle timeout); a listener's certificate
  (and the hub's own) read again when its files change.
- **Done when**: a QUIC handshake that stalls is closed at `handshake_timeout`; past half of
  `max_handshakes` new peers are asked to Retry; a listener's renewed certificate is served
  without a restart.
- **Touches**: `akit-tunnel`; akio-quic.
- **fit**: `tunnel/internal/transport/quic`, `tunnel/hub`; I160.
- **Status**: done (2026-11-27). See the progress log. The hub's QUIC listeners on akio's A245
  (`handshake_timeout`, `max_handshakes`); the tunnel hub's listeners' certificates read again
  (`tls`, `wss`, `quic`, and the hub's own kept file). The LLM mesh listener's certificate read
  again is left to the LLM track, whose roles another change was editing (decision row 44).

### I103 Tunnel III: the hub's calls and status, token expiry

- **Do**: the hub's calls (query a node, set its services where `allow_hub_services` lets it, kick
  a peer, close a tunnel) and its status, as fit's pages read them, through a handle the role
  gives its pages (`Role::api`, `akit_tunnel::hub::HubApi`); tokens' expiry closing their links at
  their expiry, and revocation at once; `password_peers`.
- **Done when**: fit's hub and token tests pass ported (expiry, the shared password listed); the
  calls work against a node and a proxy linked to the hub.
- **Touches**: `akit-tunnel`, `akit-roles`.
- **fit**: `tunnel/hub/api.go`, `hub.go`'s calls.
- **Status**: done (2026-11-27). See the progress log. The console socket and web proxies moved to
  I103b.

### I103b Tunnel III b: the console socket and web proxies

- **Do**: the hub's `console_socket` (its sealed WebSocket, fit's `wscrypt`: PBKDF2, HKDF, AES-GCM
  each way; `max_conns`, a quarter of them at most unproven from one address, the 10 s proof; each
  WebSocket a TCP connection to the hub's console, both ways paced) and a proxy's `web_proxies`
  (a local listener, each connection a WebSocket of its own to a hub's socket).
- **Done when**: fit's `socket_test.go` and `webproxy_test.go` pass ported; a web proxy serves a
  hub's console through its socket, against fit's hub and proxy both ways.
- **Touches**: `akit-tunnel`.
- **fit**: `tunnel/hub/socket.go`, `tunnel/proxy/webproxy.go`, `tunnel/internal/wscrypt`. The
  console the socket carries is the hub's pages (I109).
- **Status**: done (2026-11-27). See the progress log. `akit run` refused the roles' `console`, and
  so the hub's `console_socket`, until I109 served the hub's pages the socket carries.

### I104 LLM I: the mesh

- **Do**: fit's `llm/internal/wire` and `mesh`: frames (types, flags, gzip at 1 KiB, 16 MiB at
  most), sessions with odd and even stream IDs, requests, pushes in order (1,024 waiting),
  streams with a 256 KiB window each way; the handshake with a token or a role's password, both
  sides proven; hello and the roles' rules; pools by peer ID, dial backoff, the heartbeat, the slow
  timeout and drain; over akio-ws, `ws://` on loopback and `wss://` with `pin` or `ca_file`; at
  most 256 links not authenticated, 16 from one address.
- **Waits on**: akio API request A236, as I102c: the mesh's links are WebSockets (on akio's main and
  pinned since `6151f5c5`).
- **Done when**: fit's `wire` and `mesh` tests pass ported (credit, a slow reader holding its own
  stream only, cancel, idle, ping and slow timeout, drain, hello refusals, tokens and passwords).
- **Touches**: `akit-llm` (new).
- **fit**: `llm/internal/wire`, `llm/internal/mesh`, `llm/internal/role`; I107, I159, I160.
- **Status**: done (2026-11-27). See the progress log. Left to the roles' iterations: a listener's
  certificate read again when its files change (done with I108, decision 44), and `self_signed`
  (the role makes it, I105–I107); `/mesh/*` methods for the pages (I109).

### I105 LLM II: the node

- **Do**: `models` with their state (`disabled`, `error`, `ready`), checks every 60 s (15 s in
  `error`), `/node/proxy` on akio's asynchronous HTTP client: each API's URL, key header, the
  headers and query that go upstream, `upstream` set in a JSON body or a form, bodies streamed
  both ways under the stream's credit, `connect_timeout`, `first_byte_timeout`, `idle_timeout`
  and `timeout`; the node's info pushed (200 ms apart, with its sequence).
- **Done when**: fit's `llm/node` proxy and model tests pass ported against a mock upstream of each
  API (streamed events, slow and cut upstreams, a large body, the model renamed).
- **Touches**: `akit-llm`.
- **fit**: `llm/node/{node,models,proxy,push}.go`, `llm/internal/model`.
- **Status**: done (2026-11-27). See the progress log. The node's management methods (models
  saved, deleted and checked by its pages or a hub, its profile, settings and upstream bounds) are
  I108's and I109's; its info's hardware is I108's.

### I106 LLM III: the hub

- **Do**: the node table (`max_nodes`, the offline seen longest ago forgotten), the models of its
  online nodes pushed to proxies, `/hub/proxy` picking the least loaded node and relaying the
  frames under each stream's credit (up to 3 nodes before the head), the node list with its
  bounds; `/hub/node/call` and `/hub/node/stream` for the pages.
- **Done when**: fit's `llm/hub` tests pass ported; an end-to-end call agent → proxy → hub → node
  → mock upstream streams both ways.
- **Touches**: `akit-llm`.
- **fit**: `llm/hub`.
- **Status**: done (2026-11-27), but for `/hub/node/call`, `/hub/node/stream` and the node list's
  4 MiB bound, split into I106b. See the progress log.

### I106b LLM III b: the hub's calls for the pages

- **Do**: `/hub/nodes`, `/hub/node/call` and `/hub/node/stream` (a node's management methods
  relayed, where the node allows a hub to manage it), the hub's settings methods; the node list
  bounded at 4 MiB, the offline nodes left out first; the routes' counts (`mesh.Routes`) and
  `/mesh/status`.
- **Done when**: fit's `llm/hub` tests of the node calls and the list's bounds pass ported.
- **Touches**: `akit-llm`.
- **fit**: `llm/hub/{manage,nodelist}.go`, `llm/internal/mesh/routes.go`.
- **Status**: done (2026-11-27). See the progress log. The roles' other `/mesh/*` methods (peers,
  dial, listen, limits) are I109's.

### I107 LLM IV: the proxy

- **Do**: the agents' API on `api.address` (a binding of its own, TLS off loopback): OpenAI's,
  Anthropic's, Gemini's, Ollama's and Cohere's paths, the model lists, API keys (`ah-`, SHA-256
  kept, compared in constant time), routes to direct nodes then hubs (4 at most before the head),
  `X-Akit-Node`, each API's error shape and status, `max_conns`, `max_inflight`,
  `max_body_bytes` and the timeouts.
- **Done when**: fit's `llm/proxy` and `llm/e2e` tests pass ported.
- **Touches**: `akit-llm`, `akit`.
- **fit**: `llm/proxy`, `llm/e2e`.
- **Status**: done (2026-11-27). See the progress log. The proxy's pages' methods (keys made and
  deleted, its API settings, its stats) are I109's.

### I108 LLM V: the node's management

- **Do**: the hardware read every `refresh_interval` and the GPUs' status on a small pool (2
  running, 16 waiting), the fixed actions and GPU settings as jobs (8 running, 200 kept, 1 MiB of
  output each, `job_timeout` killing the process group), never a command of the caller's, never
  through a shell; the methods the pages and, with `allow_hub_manage`, a hub call.
- **Done when**: fit's `llm/node` action, job and GPU tests pass ported on Linux and macOS.
- **Touches**: `akit-llm`.
- **fit**: `llm/node/{actions,jobs,exec,gpu,hwinfo_*}.go`, `llm/internal/gpu`.
- **Status**: done (2026-11-27). See the progress log. akio's A247 (the effective user), A248 (a
  process group's signal) and A249 (a file system's space) replaced its stopgaps (akio
  `a8171fda`).

### I109 The roles' pages

- **Do**: each role's `console` behind logins as `admin.console`'s, and the roles' modules on
  `admin.console`: fit's workbench pages renamed (decision 037), the tunnel's API and the LLM
  roles' `/mesh/*`, `/node/*`, `/hub/*` and `/proxy/*` methods for the pages, every change written
  back into the role's member as a config edit.
- **Done when**: fit's role page tests pass ported; a page's change is applied, in the history, and
  refused when it does not load.
- **Touches**: `akit-admin`, `akit-tunnel`, `akit-llm`, `tools/console-pages.py`.
- **fit**: `app/roleconsole`, `app/workbench`, `tunnel/*/api.go`, `llm/internal/webui`.
- **Status**: done (2026-11-27) for the roles' consoles and the hub's pages; the tunnel's node's
  and proxy's pages split into I109b, the LLM roles' into I109c (decision row 46). See the
  progress log.

### I109b The tunnel's node's and proxy's pages

- **Do**: the node's module (fit's `tunnel/node/api.go`: its status, with its hubs' links, its
  services' round trips and its tunnels; its hubs added, changed and removed; its services set;
  a tunnel closed) and the proxy's (fit's `tunnel/proxy/api.go`: its status, with each hub's nodes
  and their shares by round trip (fit's `Weights`), its forwards and web proxies; its hubs; a
  hub's node asked for its info or given services; its meta; its forwards added and removed; a
  tunnel closed), on I109's consoles; `akit run` stops refusing their `console`.
- **Done when**: fit's node and proxy page tests pass ported; a change on their pages is applied,
  in the history, and refused when it does not load.
- **Touches**: `akit-tunnel`.
- **fit**: `tunnel/node/api.go`, `tunnel/proxy/api.go`, `tunnel/internal/common/hubs.go`.
- **Status**: done (2026-11-27). See the progress log. With it, the roles' consoles' certificates
  read again when their files change (decision rows 45 and 61).

### I109c The LLM roles' pages

- **Do**: fit's `llm/internal/webui` on I109's consoles, for the LLM hub, node and proxy:
  `GET /api/me` (the user, the role's identity, a proxy's API URL), `POST /api/rpc<method>` for
  every method of the role's router, called as its own pages through `akit_llm::local::Local`
  (I106b) on the role's pool, and `POST /api/stream<method>` for its stream methods as
  Server-Sent Events, at most 16 at once, each frame released to the stream only as the browser
  takes it, 30 s at most per wait; edits written back by the methods' own `role::Base::edit`;
  `akit run` serves their `console`.
- **Done when**: fit's `webui` tests (`api_test.go`) pass ported, against akit's hub, node and
  proxy.
- **Touches**: `akit-llm`.
- **fit**: `llm/internal/webui`, `llm/internal/role` (`applyConsole`).
- **Status**: done (2026-11-27). See the progress log. The methods the pages call that the roles
  lack are I109d's (decision row 47).

### I109d The LLM roles' methods for their pages

- **Do**: the methods fit's LLM pages call that akit's roles lack: `/mesh/dial`, `/mesh/undial`,
  `/mesh/kick`, `/mesh/listen`, `/mesh/limits/get` and `/set`, `/mesh/tokens/list`, `/create`,
  `/expiry` and `/revoke` (fit's `llm/internal/role`), and the proxy's `/proxy/api/get` and `/set`,
  `/proxy/keys/list`, `/create` and `/delete`, `/proxy/models`, `/proxy/node/list`,
  `/proxy/profile/set` and `/proxy/stats` (fit's `llm/proxy`), each change written back with
  `role::Base::edit`; the identity's `http_url`, the console's URL.
- **Done when**: fit's role and proxy method tests pass ported; the pages' calls answer as fit's.
- **Touches**: `akit-llm`.
- **fit**: `llm/internal/role/{role,settings,tokens}.go`, `llm/proxy/{keys,nodes,stats,role}.go`.
- **Status**: done (2026-11-27). See the progress log. The console's URL is the one its console
  server tells the role (`akit_roles::pages::Module::console_url`, decision row 64), a port chosen
  when the console starts too.

### WebRTC

fit's I87 and I108–I111 (its [WebRTC guide](https://github.com/lesismal/fit/blob/main/docs/webrtc.md),
its decision 029, and its packages `webrtc/turn`, `webrtc/media` and `config/webrtc.go`), in five
iterations ([decision 039](decisions/039-webrtc.md)). Three roles of the config, run by I100's host
on the listeners' akio engine: `webrtc.turn`, a TURN server (RFC 8656) on `akio-turn`;
`webrtc.media`, the UDP port the `whip` and `whep` handlers' PeerConnections share, an SFU on
`akio-webrtc`'s ICE lite `RtcServer`; `webrtc.proxy`, the UDP port through which the peers of the
media servers behind the `webrtc_proxy` handler are relayed, each new peer routed by the ICE ufrag
of its first STUN request. What they show outside is akit's (decision 037: `SOFTWARE` `akit`, the
metrics `akit_turn_*` and `akit_webrtc_*`); the wire is the RFCs', so that fit's and akio's
clients, browsers and pion reach them alike. The handlers' `auth` names a `media_auth` policy, the
Streaming and media auth track's: it is refused as not supported yet until that track reads
`media_auth`.

### I110 WebRTC I: `webrtc` in the config

- **Do**: `webrtc.turn`, `webrtc.media` and `webrtc.proxy` read as strictly as the rest, with
  fit's fields, defaults, bounds and checks (`config/webrtc.go`): the TURN server's `listen`,
  `listen_tcp`, `listen_tls` with `tls.certificates`, `realm`, `users` (a password or the key it
  derives), `secret`, `relay_ip` and `public_ip` of one family, the port range, quotas, caps,
  lifetimes, `allowed_peers` and `denied_peers` (a prefix, an address, or an address and a port);
  the media server's `listen`, `public_ips`, `ice_servers` (`stun:`, `stuns:`, `turn:`, `turns:`,
  safe in a `Link` header), `turn_urls` (needing `webrtc.turn`'s `secret`), `codecs` and limits;
  the proxy's `listen`, `public_ips`, limits, `relay_ip` and port range; each one's `access`. Their
  addresses join the one-address-one-server check (fit's `config/bindings.go`). The handlers
  `whip`, `whep` (`stream`, a name of 1 to 128 of `[A-Za-z0-9._~-]`) and `webrtc_proxy`
  (`upstream`, `media_addrs`, `timeout`, `max_sdp_bytes`, `action`), their `auth` refused as not
  supported yet. `akit run` refuses the roles and handlers until their iterations run them.
- **Done when**: fit's `config/webrtc_test.go` and the WebRTC cases of `bindings_test.go` and
  `more_test.go` pass ported; fit-examples' `webrtc-turn`, `whip-whep` and `webrtc-proxy` pass the
  gate; the reference lists every field; the v1 freeze marks them as additions.
- **Touches**: `akit-config` (`webrtc`), `akit`, `tests/compat`, `docs/reference`.
- **fit**: `config/webrtc.go`, `config/bindings.go`, `webrtc/media/handler.go`,
  `webrtc/media/proxyhttp.go`; I87, I109, I110.
- **Status**: done (2026-11-27). See the progress log. The handlers answer 503 until their roles run,
  as fit's do, and `akit run` refuses them.

### I111 WebRTC II: the TURN server

- **Do**: `webrtc.turn` on `akio-turn`'s `TurnServer`, run by `akit run`: UDP on `listen` (one
  binding per address, `access` filtering a new client's datagrams), TCP on `listen_tcp` and TLS on
  `listen_tls` (`TurnServer::stream`, akio-tls with the certificates from files); the users' keys
  and the coturn-style `secret` (time-limited credentials), the realm, `relay_ip` and `public_ip`;
  `max_allocations` (508), `max_allocations_per_user` (486), `max_permissions`, `max_channels`
  and the lifetimes; the peer rules over akio-turn's denial of internal addresses, port-aware,
  with akit's own addresses reached on no port that `allowed_peers` does not name; a reload that
  changes the users, secret, peer rules and access in place and starts a new server for the rest,
  draining the old one for `-shutdown-timeout`; the `webrtc` logger; the server's counters in
  `/metrics`. Where akio-turn lacks fit's hooks (a relay port range, a username's bandwidth, the
  events, limits changed in place, a drain, the nonce secret, the stream bounds), an akio request
  and a recorded stopgap.
- **Done when**: fit's `webrtc/turn` tests pass ported where akio-turn serves them, with
  `akio-turn`'s client and fib's (a Go program built from fit's module tree): an allocation relays
  both ways to a peer by indication and channel, over UDP, TCP and TLS; a wrong password and an
  expired time-limited credential are refused; the quotas refuse with 486 and 508; a private peer
  is refused unless allowed; a reload of the users keeps the allocation, one of the address moves
  the server.
- **Touches**: `akit-webrtc` (new, `turn`), `akit`, `akit-roles`.
- **fit**: `webrtc/turn`; I87, I111.
- **Status**: done (2026-11-27). See the progress log. Stopgaps until akio's A256-A258 (decision 039):
  a port range refused at run time; bandwidth, nonce secret and stream bound not applied.

### I112 WebRTC III: the TURN server on akio's hooks

- **Do**: the stopgaps of I111 replaced once akio serves its requests: relays from `port_min` to
  `port_max`, `user_bandwidth` (a token bucket a username and direction, the datagrams past it
  dropped and counted), the events counted and logged (allocated, refreshed, freed by reason,
  refused by code, credentials failed), the quotas, caps and lifetimes changed in place, a drain
  that refuses new allocations while the held ones run out, `nonce_secret`, `max_stream_message`
  and `stream_idle_timeout`, the server's own relayed addresses reached by its clients.
- **Waits on**: akio API requests A256, A257 and A258.
- **Done when**: fit's `webrtc/turn` tests of I111 pass ported (`turn_test.go`'s bandwidth,
  events, caps and drain cases, `apply_test.go`).
- **Touches**: `akit-webrtc`.
- **fit**: `webrtc/turn`, `webrtc/turn/limiter.go`; I111.
- **Status**: done (2026-11-27). See the progress log.

### I113 WebRTC IV: WHIP and WHEP

- **Do**: `webrtc.media` on `akio-webrtc`'s `RtcServer` (ICE lite, every PeerConnection on one UDP
  port, `access` filtering a new address's datagrams), and the handlers `whip` and `whep`: a
  publisher's offer answered with the first of its codecs of each kind that `codecs` lists, its
  media forwarded to each viewer of its stream, the viewers' key frame requests passed to it at
  most once a `key_frame_interval`; `max_bitrate` told as REMB and enforced with two seconds of
  burst; DELETE and OPTIONS per RFC 9725 and the WHEP draft (PATCH answered 405, as fit); ICE
  servers and `turn_urls` with credentials made from `webrtc.turn`'s secret in `Link` headers;
  the limits (PeerConnections, streams, viewers a stream, the offer's size), `connect_timeout`
  and `ice_timeout`; sessions kept by a reload that keeps `listen` and `public_ips`, ended by one
  that does not; `/metrics`.
- **Waits on**: nothing that stops it: akio requests A259 (a peer's own configuration), A260 (RTP
  forwarding) and A261 (REMB sent, ICE timeouts) have planned stopgaps, frames forwarded in place of
  packets among them.
- **Done when**: fit's `webrtc/media` tests of the media server pass ported with akio-webrtc's
  `Endpoint` as publisher and viewers, and fib's PeerConnection (a Go program) publishes and plays
  through akit: a publisher gone ends its viewers; slow and cut viewers do not hold the publisher;
  the limits refuse.
- **Touches**: `akit-webrtc` (`media`), `akit-handler` or `akit`, `akit-roles`.
- **fit**: `webrtc/media/{media,session,handler}.go`; I109.
- **Status**: done (2026-11-27). See the progress log.

### I114 WebRTC V: WHIP and WHEP proxied

- **Do**: the handler `webrtc_proxy` and the role `webrtc.proxy`: an offer sent to a media server
  of the route's upstream (its pick, health and timeouts), the answer's candidates replaced by the
  proxy's port and its ufrag remembered against the media server's address (`media_addrs`, or the
  answer's first host or server-reflexive candidate), the session's URL rewritten to akit's; the
  UDP port routing a new peer address by the USERNAME of its first STUN Binding request, then
  relaying its 5-tuple through a socket of its own toward the media server (`relay_ip`, the port
  range), with `max_sessions`, `max_relays` (8 a session), `session_timeout` and `idle_timeout`.
- **Done when**: fit's `proxy_test.go`, `proxy_edges_test.go` and `proxy_ports_test.go` pass
  ported: two media servers behind one akit port each get their own peers; a peer's datagrams
  before its Binding request are dropped; a media server gone frees its routes.
- **Touches**: `akit-webrtc` (`proxy`), `akit-upstream`, `akit`.
- **fit**: `webrtc/media/{proxy,proxyhttp,relay}.go`; I110, I111.
- **Status**: todo.

### BitTorrent

fit's I196–I210 (its [BitTorrent guide](https://github.com/lesismal/fit/blob/main/docs/bittorrent.md),
its decision 037, its packages `bittorrent`, `bittorrent/tracker`, `handler/btstream`,
`handler/btpublish`, `config/bittorrent*.go`), in ten iterations. Two roles of the config:
`bittorrent.client` downloads and seeds torrents kept in its `data_dir` across restarts, and
`bittorrent.tracker` answers announces and scrapes, over HTTP through the routes' `bt_tracker`
handler and over UDP on its own `listen`. They run on akio-bittorrent's `Client` and
`TrackerServer`, beside the gateway, by `akit run` (I100's host). What fit asked of fib for them
(fib's A75–A79, A85–A88) akit asks of akio where akio-bittorrent lacks it (A283–A288): the client
on the listeners' engine, peers refused by address and limits changed at run time, the tracker
store's bounds in place. The roles' files are fit's (`torrents.json`, `torrents/<id>.torrent`) so
that a data directory moves between fit and akit; resume data is akio's own format, and fit's is
refused and the data checked again (decision row 75). Nothing blocks an I/O thread: storage and
hashing run on akio-bittorrent's disk pool, the state files on the role's own thread, and the
tracker's announces are a lookup and a shard lock. fit's I205 (interop, failure tests) is in
each role's iteration: akit's tracker with fit's client and aria2c, akit's client with fit's
tracker and seeder, on loopback only. fit's I206 (the cluster's view of its members' torrents)
waits for the Cluster management track.

### I150 BitTorrent I: `bittorrent` in the config

- **Do**: `bittorrent.client` and `bittorrent.tracker` read as strictly as the rest, with fit's
  fields, defaults, bounds and checks (`config/bittorrent.go`, `config/bittorrent_tracker.go`):
  the client's `listen` and `data_dir` (required), `download_dir`, `encryption`, `dht`, `pex`,
  `utp`, `lsd`, `disk_workers`, `disk_queue_bytes`, `save_interval`, `limits` and `console`; the
  tracker's `listen`, `interval`, `min_interval`, `peer_ttl`, its store's bounds, `max_numwant`,
  `max_scrape`, `allow`, `allow_files`, `access` and `console`. The client's port in the one
  address check for TCP and UDP, the tracker's for UDP (fit's `bindings.go`); the `bittorrent`
  logger. What a later iteration runs is refused as not supported yet, naming its field:
  `categories`, `watch_dir` and its bounds, `url_timeout`, `alt_speed`, the client's `access`, the
  limits akio cannot set yet (`max_conns`, `max_half_open`, `max_active_*`, `min_free_bytes`),
  `publish`, `rss`, the tracker's `auth` (media_auth) and `store`. `akit run` refuses a config
  with a role until the role's iteration runs it.
- **Done when**: fit's `config/bittorrent_test.go` and `bittorrent_tracker_test.go` cases pass
  ported; fit-examples' `bittorrent-*` pass the gate or fail only as not supported yet; the
  reference lists every field; the v1 freeze marks them as additions.
- **Touches**: `akit-config` (`bittorrent`), `akit`, `tests/compat`, `docs/reference`.
- **fit**: `config/bittorrent.go`, `config/bittorrent_tracker.go`, `config/bindings.go`; I197,
  I201.
- **Status**: done (2026-11-27). See the progress log.

### I151 BitTorrent II: the tracker role

- **Do**: `bittorrent.tracker` on akio-bittorrent's `TrackerServer`, in a new crate
  `akit-bittorrent`: HTTP announces and scrapes through the `bt_tracker` handler (fit's
  `bittorrent/tracker/handler.go`: the path's last segment, refusals as failure reasons, 503
  while the role does not run), UDP on `listen` added to the listeners' engine with the role's
  access at akio's filter, BEP 15 connection IDs keyed by a secret kept across reloads; fit's
  answers (`unregistered torrent`, `min interval`, 50 peers by default and `max_numwant` at most,
  a UDP answer within 1,400 bytes, `max_scrape`); the allow-list inline and in files, read again
  when one changes; access for HTTP in the handler; statistics (announces, scrapes, refusals by
  reason); the console's read-only routes (`/api/stats`, `/api/torrents`, `/api/torrents/<hash>`).
  A reload keeps the swarms when only the allow-list, access or answers change; a change of the
  store's bounds makes a new store until akio's A285.
- **Done when**: fit's tracker tests pass ported (announces and scrapes over HTTP and UDP, the
  allow-list and its files, access, bounds, reloads, a byte-at-a-time client, garbage datagrams);
  fit's client and aria2c (where installed) announce to akit's tracker and two fit clients find
  each other through it.
- **Touches**: `akit-bittorrent` (new), `akit-config`, `akit`.
- **fit**: `bittorrent/tracker`; I201, I209.
- **Status**: done (2026-11-27). See the progress log. Stopgaps until akio's A285: decision rows
  77–79.

### I152 BitTorrent III: the client role

- **Do**: `bittorrent.client` on akio-bittorrent's `Client` (its own engine until akio's A283):
  the state in `data_dir` as fit's (`torrents.json`, `torrents/<id>.torrent`, resume data in
  `resume/`), written to a temporary file, synced and renamed, by the role's own thread at most
  every `save_interval`, at once for a change of the list, and at stop; restored at start without
  a check; a reload changes the rates and seed limits in place and makes a new client (the
  torrents carried over, resuming without a check) when `listen`, `encryption`, `dht`, `pex`,
  `utp`, `lsd`, `disk_workers` or `disk_queue_bytes` change, a new `data_dir` starting from that
  directory's state; a port taken brings the old client back. The role's API (add by magnet link
  or .torrent file, remove with or without the data, pause, resume, list, status) and the
  console's read-only routes (`/api/torrents`, `/api/torrents/<id>`).
- **Done when**: fit's role tests pass ported (a download between two roles on loopback, a restart
  resuming without reading the data, a magnet link's .torrent saved and restored from, a reload of
  limits keeping the peer, a `listen` change keeping the torrents, bad and oversized state files,
  broken torrents kept listed, the API's refusals); akit's client downloads from fit's seeding
  client found through fit's tracker, and fit's client from akit's.
- **Touches**: `akit-bittorrent`, `akit`.
- **fit**: `bittorrent/{role,state,api}.go`; I197.
- **Status**: done (2026-11-27). See the progress log. Stopgaps until akio's A283, A284 and
  A286: decision row 75.

### I153 BitTorrent IV: adding torrents, categories, the queue

- **Do**: as fit's I198: .torrent URLs on akio's HTTP client (32 MiB, 5 redirects,
  `url_timeout`, 4 at once), `watch_dir` (.torrent and .magnet files, `.done` and `.failed`),
  categories (directories, limits, a `media_library` rescan once that track lands), the queue
  (`max_active_downloads`, `max_active_seeds`), the disk checked (`min_free_bytes`, akio's A249).
  A torrent's own download directory needs akio-bittorrent's storage for a magnet link (A286).
- **Touches**: `akit-bittorrent`, `akit-config`.
- **fit**: `bittorrent/{manage,fetch,watch}.go`; I198.

### I154 BitTorrent V: schedules, per-torrent limits, the client's access

- **Do**: as fit's I199: `alt_speed` with weekly schedules and a time zone, per-category and
  per-torrent limits, the client's `access` (akio's A284: peers refused by address, closed when a
  reload refuses them), and the connection limits akio sets at run time (`max_conns`,
  `max_half_open`, `upload_slots`, A284).
- **Touches**: `akit-bittorrent`, `akit-config`.
- **fit**: `bittorrent/manage.go`; I199.

### I155 BitTorrent VI: `bt_stream`

- **Do**: as fit's I200: a torrent's file served by ranges while it downloads, the ranges asked
  read first (a reader of akio-bittorrent's that waits for pieces, A287), `max_streams`, timeouts,
  temporary torrents added from a magnet link (after media_auth's track for `auth`).
- **Touches**: `akit-bittorrent`.
- **fit**: `handler/btstream`; I200.

### I156 BitTorrent VII: publishing

- **Do**: as fit's I202 and I209: `bittorrent.client.publish` and the `bt_publish` handler, a
  directory built into hybrid torrents on a bounded pool, seeded in place, web seeds at akit's
  static files, following the disk.
- **Touches**: `akit-bittorrent`, `akit-config`.
- **fit**: `bittorrent/{publish,publish_walk}.go`, `handler/btpublish`; I202, I209.

### I157 BitTorrent VIII: the Admin API, events, `akit bt`, metrics

- **Do**: as fit's I203 and I210: `/bittorrent/...` on the Admin API, the event stream, `akit bt`,
  metrics by category and state; each tracker's announces and the web seeds' state (A288), a
  recheck in place.
- **Touches**: `akit-bittorrent`, `akit-admin`, `akit`, `akit-observe`.
- **fit**: `admin/bittorrent.go`, `admin/btevents.go`, `cli/bt.go`, `bittorrent/{events,inspect,
  metrics}.go`; I203, I210.

### I158 BitTorrent IX: the pages

- **Do**: as fit's I204: the client's and the tracker's pages on the console machinery
  (`akit_roles::pages`), on I157's API.
- **Touches**: `akit-bittorrent`, `akit-admin`.
- **fit**: the workbench's BitTorrent pages; I204.

### I159 BitTorrent X: RSS and the tracker store instances share

- **Do**: as fit's I207 and I208: RSS auto-download (`rss`), and the tracker's `store` in Redis;
  a private tracker's passkeys once `media_auth` is read (the Streaming and media auth track).
- **Touches**: `akit-bittorrent`, `akit-config`.
- **fit**: `bittorrent/rss*.go`, `bittorrent/tracker/sharedstore`; I207, I208.

### Cluster management

fit's I140–I150 (its [cluster guide](https://github.com/lesismal/fit/blob/main/docs/cluster.md),
its [decision 033](https://github.com/lesismal/fit/blob/main/docs/decisions/033-cluster-management.md),
its package `cluster`, `admin/node.go` and `config/cluster.go`), in twelve iterations
([decision 041](decisions/041-cluster-management.md)); fit's I153, the assistant across a cluster,
goes with the AI assistant track, which it needs. One akit, with `admin.cluster`, manages others,
its members: it keeps their URLs and credentials in its store, polls each one's state, and serves
them in the console's Cluster pages, fit's workbench pages as I91 copied them. A member changes as
little as possible: the manager speaks the APIs it already has, its console or its Admin API.
The wire is fit's (decision 041): a member's `GET /node`, `/status` and the console's login answer
as fit's, and the manager's store section `cluster` is fit's document, so that akit and fit members
share a manager of either kind, and an akit and a fit manager open each other's stores. Off by
default: no `admin.cluster`, no Cluster menu, no outgoing calls.

### I130 Cluster I: `admin.node_id`, `admin.cluster` and `admin.cluster_member` in the config

- **Do**: the three read as fit's (`config/cluster.go`, `checkCluster`, `checkClusterMember`, the
  node ID's pattern): `admin.node_id` (1 to 64 of `[A-Za-z0-9._-]`, a letter or digit first);
  `admin.cluster` (`poll_interval` 10s and `timeout` 10s, 1s at least; `max_nodes` 256 up to
  10,000; `max_concurrency` 8 up to 256; `max_response_bytes` 4 MiB, 16 KiB to 64 MiB;
  `link_address`, `link_url` (wss, with `link_address`); `admins`, console users named once, 64
  at most), which needs `admin.console`; `admin.cluster_member` (`manager` a wss URL,
  `fingerprint` 64 hex digits, `token` (`akite1.`, fit's `fite1.` renamed, decision 037) or
  `token_file`). `akit run` refuses each until the iteration that runs it (I131, I132, I139b),
  naming it.
- **Done when**: fit's `config` tests of the three pass ported; the gate's examples are valid;
  the fields marked with `make v1-bless`; the reference regenerated.
- **Touches**: `akit-config`, `akit`, `tests/compat`, `docs/reference`.
- **fit**: `config/cluster.go`, `config/validate.go`; I140, I141, I149.
- **Status**: done (2026-11-27). `akit_config::cluster`; `akit run` refuses `admin.cluster` and
  `admin.cluster_member` naming the iteration (decision row 74). See the progress log.

### I131 Cluster II: what a member needs

- **Do**: fit's I140 on every akit: `GET /node` (and the console's `/api/node`, as I90 routes it):
  a node ID made once (a random UUIDv4) and kept in the store's section `node` (fit's), or in a
  file beside the config (`akit.node-id` beside `akit.json`), or `admin.node_id`; the host name,
  akit's and akio's versions, the start, the running config's version, ETag, time and source, the
  roles running, the schema's version and digest, and fit's capability names for what akit's API
  serves; `GET /schema`. The console's login already answers the CSRF token, and I89 slows failed
  logins as fit's.
- **Done when**: fit's `admin/node_test.go` passes ported: the same ID across restarts with and
  without a store, a file's ID taken into a store, `admin.node_id` winning; the capabilities name
  only what is served; the Account page shows the node.
- **Touches**: `akit-admin`, `akit` (`run`), `docs`.
- **fit**: `admin/node.go`; I140.

### I132 Cluster III: the manager, its members and their state

- **Do**: fit's I141 as a new crate, `akit-cluster`: the store's section `cluster`, fit's document
  (versions 1 to 3 read; what akit does not use yet, groups, templates, rollouts, links, roles,
  kept as it is), a newer one read-only; members with name, labels, URL and access (`console`,
  logged in again on 401; `admin_api`, a bearer token), credentials never answered, logged or
  sent but to their member; TLS by the system's roots, a CA PEM or a pinned SHA-256 (akio-tls's
  `pin_sha256`); `http` only to loopback. A poller, a thread of its own starting calls on akio's asynchronous HTTP client (rule 1), at most
  `max_concurrency` members at once, each every `poll_interval`, reading `/node`, `/status` and,
  where served, `/config/check`, each call within
  `timeout` and `max_response_bytes`; the login backoff and the rate limits as fit's. The console
  module `cluster`: `GET/POST/PUT/DELETE /api/m/cluster/nodes[/<name>]`, `POST
  .../nodes/<name>/test` and `POST /api/test` (the certificate's fingerprint answered for a
  person to confirm), `GET /api/m/cluster/state`, `/audit` and `/me`; the audit log of adds,
  changes and removals.
- **Done when**: fit's `cluster` tests of members, the poller and the API pass ported against
  stand-in members; a manager with an akit member and a fit member (fit built from source) reads
  both, by password and by token; a wrong password, a certificate not trusted, a pin that does not
  match and a member down are told apart; no credential appears in any answer or log; a store
  written by fit's manager opens in akit's and the reverse.
- **Touches**: `akit-cluster` (new), `akit-admin` (`console`), `akit` (`run`), `docs`.
- **fit**: `cluster/{cluster,members,poll,transport,probe,read,call,audit}.go`; I141.

### I133 Cluster IV: the Cluster pages, the overview and a node's page

- **Do**: fit's I142: the console lists the module `cluster`, so the workbench draws its Cluster
  menu, Overview and Nodes, as I91 copied them; a member's page reads the member through the
  manager (`GET /api/m/cluster/nodes/<name>/read/{node,status,versions,check}`, 8 at once);
  `docs/guides/cluster.md` in both languages.
- **Done when**: in a browser, a member is added, tested, trusted, edited and removed; the
  overview shows each failure mode of I132 and recovers; both languages.
- **Touches**: `akit-cluster`, `akit-admin`, `docs/guides`.
- **fit**: `cluster/read.go`, `app/workbench/web/cluster`; I142.

### I134 Cluster V: a member's own pages, opened from the manager

- **Do**: fit's I143: `/api/n/<member>/...` passed on to the member with the stored session, the
  member's schema read from it, writes audited, a member of another schema read-only.
- **Done when**: fit's `passthrough` tests pass ported; every config page reads and saves an akit
  and a fit member.
- **Touches**: `akit-cluster`, `akit-admin` (`console`).
- **fit**: `cluster/passthrough.go`; I143.

### I135 Cluster VI: groups and operations on many

- **Do**: fit's I144: groups by label selector or by name, with variables; operations (reload,
  rollback, check, purge by key, prefix or tag, test) on a group or a selection, bounded,
  answered per member; `/api/m/cluster/groups`, `/ops`.
- **Done when**: fit's `groups` and `ops` tests pass ported.
- **Touches**: `akit-cluster`, `akit-admin` (`cache`: purge by prefix, fit's `cache.prefix`).
- **fit**: `cluster/{groups,ops}.go`; I144.

### I136 Cluster VII: templates

- **Do**: fit's I145: templates owning top-level members and named elements, variables, order,
  each member's sync state, import from a member, compare two; `PUT /node/managed` on members.
- **Done when**: fit's `templates`, `render` and `sync` tests pass ported.
- **Touches**: `akit-cluster`, `akit-admin` (`/node/managed`).
- **fit**: `cluster/{templates,render,sync,text,jsonv}.go`, `admin/managed.go`; I145.

### I137 Cluster VIII: rollouts with health gates

- **Do**: fit's I146: a rollout's plan (each member's diff and dry run, its ETag), all at once,
  rolling or canary; after each batch a health gate (reachable, reload succeeded, no new drift, a
  5xx share under a threshold over a window from `/status`'s `requests`); a failed gate rolls the
  batch back (`/rollback/{n}` with `If-Match`); pause, resume, abort; a member changed since the
  plan skipped; kept in the store across a restart.
- **Done when**: fit's `rollouts` tests pass ported, with akit and fit members.
- **Touches**: `akit-cluster`, `akit-admin` (`/status`'s `requests`).
- **fit**: `cluster/{rollouts,rollrun,rollsteer,rolldraft,rollapi}.go`; I146.

### I138 Cluster IX: drift and alerts

- **Do**: fit's I147: drift against the templates and the member's consistency check (fit's
  `GET /config/check`, which akit serves first here); alert rules, webhooks, silences.
- **Done when**: fit's `alerts`, `alerteval`, `webhooks` and `drift` tests pass ported.
- **Touches**: `akit-cluster`, `akit-admin` (`/config/check`).
- **fit**: `cluster/{alerts,alerteval,alertapi,webhooks,drift}.go`, `admin/drift.go`; I147.

### I139 Cluster X: users, roles and the audit log

- **Do**: fit's I148: roles per group (viewer, operator, admin), `admin.cluster.admins`, every
  endpoint checked; the audit log of every action, filtered, as JSON lines.
- **Done when**: fit's `roles` and `audit` tests pass ported.
- **Touches**: `akit-cluster`.
- **fit**: `cluster/{roles,audit}.go`; I148.

### I139b Cluster XI: members behind NAT, linking out

- **Do**: fit's I149: `admin.cluster_member`; the manager's link listener (`link_address`), the
  enrollment tokens (`akite1.`, `akitc1.`; fit's labels on the wire, decision 038's rule), the
  member's Admin API carried over the link.
- **Done when**: fit's `link` tests pass ported; an akit member links to a fit manager and the
  reverse.
- **Touches**: `akit-cluster`, `akit-roles` (`linktoken`), `akit`.
- **fit**: `cluster/{linkserver,linkmember,linkproto,enroll}.go`, `admin/clusterlink.go`; I149.

### I139c Cluster XII: metrics, logs and certificates across members; `akit cluster`

- **Do**: fit's I150: series from each poll, the members' logs tailed (`GET /logs/recent` on
  members), the certificate inventory; `akit cluster` in the CLI through the manager's Admin API.
- **Done when**: fit's `insight`, `logtail` and `adminapi` tests pass ported; the CLI drives a
  rollout end to end.
- **Touches**: `akit-cluster`, `akit-admin` (`/logs/recent`), `akit` (`cluster`).
- **fit**: `cluster/{insight,logtail,adminapi}.go`, `cli/cluster.go`, `admin/logsrecent.go`; I150.
