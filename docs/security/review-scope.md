# External security review: scope

[English](review-scope.md) | [简体中文](review-scope.zh-CN.md)

The scope of 1.0's external security review, as the [roadmap](../roadmap.md) asks for one of TLS
handling, the Admin API and the plugin sandbox: what the reviewers look at, where it is, what to
ask of it, what they can leave out, and what is already known. The
[threat model](threat-model.md) is the companion: its assets, boundaries and controls are what
this review tests; its [known limitations](threat-model.md#known-limitations) are known, not
findings.

- [The target](#the-target)
- [What we want to learn](#what-we-want-to-learn)
- [In scope](#in-scope)
  - [1. TLS handling](#1-tls-handling)
  - [2. The Admin API and the dashboard](#2-the-admin-api-and-the-dashboard)
  - [3. The plugin sandbox](#3-the-plugin-sandbox)
  - [4. Request parsing and the proxy's header handling](#4-request-parsing-and-the-proxys-header-handling)
  - [5. The eBPF sockmap path](#5-the-ebpf-sockmap-path)
  - [6. If time allows: providers and shared stores](#6-if-time-allows-providers-and-shared-stores)
- [Fuzz targets](#fuzz-targets)
- [Out of scope](#out-of-scope)
- [akio](#akio)
- [Working on akit](#working-on-akit)
- [Findings](#findings)

## The target

- **akit**: the commit the maintainer names when the review starts. At I80 that is `main` with I77
  (fuzzing) and I80 (this page) merged, on top of `953cb57`.
- **akio**: the commit akit pins (`rev` in `Cargo.toml`; `0fc8f3a7`, `1.0.0-rc.1`, at I80), the
  release binary's only dependency.
- **Platforms**: Linux (x86_64 and aarch64; epoll and io_uring; kernel 6.15 or later for the
  sockmap relay), macOS (kqueue) for development. The release binary is the static musl build of
  `scripts/dist.sh`, and the image is the `Dockerfile`'s.
- **Configs**: fit's config ([decision 002](../decisions/002-config-compatibility.md)), as the
  [config reference](../reference/config.md) documents it; the starting points are the image's
  `deploy/docker/akit.json` and the [guides](../guides/README.md).

## What we want to learn

1. Can a client, an upstream, a web page or a local user without the token make akit do what its
   config does not allow: read or change the config, reach a route past its middleware, read a
   file outside a root, reach an address the forward proxy refuses, obtain a certificate for a
   name the ask URL refused, or get a key?
2. Can any of them crash akit, hang or block an I/O thread, or exhaust memory, connections or a
   pool within the limits akit sets by default?
3. Does akit use akio's TLS, HTTP, WebAssembly and eBPF parts as they are meant to be used: the
   options it sets, the ones it leaves at their defaults, the checks it relies on akio for?
4. Are the controls the [threat model](threat-model.md) claims there, and do its tests test them?

## In scope

Each area lists its code, the threat model's section, and the questions we would ask first. Paths
are relative to the repository's root.

### 1. TLS handling

Code: `crates/akit-tls` (`lib.rs`: `ListenerTls`, `server_configs`, client authentication;
`certs.rs`: the certificate chosen per handshake; `watch.rs`; `storage.rs`; `ocsp.rs`;
`published.rs`; `upstream.rs`; `acme/`: `mod.rs`, `protocol.rs`, `jws.rs`, `ari.rs`,
`on_demand.rs`, `shared.rs`; `shared/`: Redis, etcd, Consul), `crates/akit-runtime/src/tls.rs`
(the resolver reading the graph), `crates/akit-listener/src/lib.rs` (HTTP-01 and TLS-ALPN-01
answered, listeners bound with their TLS), `crates/akit-upstream/src/h2.rs` (h2 upstreams over TLS).
Threat model: [TLS from clients](threat-model.md#tls-from-clients),
[certificates](threat-model.md#certificates-files-acme-on-demand-tls-ocsp),
[upstreams](threat-model.md#upstreams).

- **akio-tls as akit uses it**: the versions, suites, ALPN, session tickets, client authentication
  modes and the per-handshake resolver akit configures; that early data stays off; that
  TLS-ALPN-01's certificate is never served to an ordinary client. akio-tls's own handshake,
  record layer and X.509 verification are akio's ([below](#akio)).
- **Certificates**: loading and matching pairs, the choice by SNI (the first certificate for an
  unknown name, as fit), replacement under traffic (`-watch-certs`), client CA changes.
- **ACME**: the account key and its storage, JWS signing, orders and challenges, ARI, renewal and
  backoff; the HTTP-01 answer path on listeners in the clear; the storage's modes and atomic writes;
  shared storage's locks and what an instance trusts from the store.
- **On-demand TLS**: whether a client can make akit ask about, or obtain, names beyond the ask URL's
  answer and the rate limits; the bounds on waiting handshakes; the ask client's TLS (fixed in I80:
  it had no CAs, so every `https://` ask failed).
- **OCSP**: responses verified and matched, responders of certificates from files asked only at
  public addresses, nothing stapled past `nextUpdate`.
- **Upstream TLS**: verification always on, `tls_server_name`, client certificates.

### 2. The Admin API and the dashboard

Code: `crates/akit-admin` (`server.rs`: the dispatcher, `guard`, bodies, jobs; `configs.rs`: the
history; `pointer.rs`: JSON pointers and merge patches; `cache.rs`: purges; `status.rs`;
`dashboard.rs` and the dashboard's files; `watch.rs`), `crates/akit-config/src/model.rs` (`admin`),
`crates/akit/src/main.rs` (the control thread that applies jobs), `crates/akit-provider/src/merge.rs`
(what a provider's part may hold). Threat model:
[the Admin API and the dashboard](threat-model.md#the-admin-api-and-the-dashboard). The endpoints are
listed in the [compatibility page](../compatibility.md#the-admin-api-v1) and frozen by
`tests/compat/v1/admin-api.txt`.

- **Authentication**: the address checks (loopback, the Unix socket's mode), the token's reading
  and comparison, what is reachable without it (the dashboard's static files).
- **Browsers**: the `Origin` and `Host` rules against CSRF and DNS rebinding, with and without a
  token; the dashboard's headers and how it keeps the token.
- **Changes**: bodies and their limits, JSON pointers, merge patches, `If-Match`, `?dry_run`,
  rollbacks and reloads; whether a change can be applied without being compiled, or a dry run
  change anything (listeners bound, files written, certificates ordered).
- **Reads**: what `GET /config`, `/config/effective`, `/versions`, `/status`, `/metrics` and `/cache`
  disclose (known: inline values are not redacted, L2).
- fit's console (logins, sessions, the encrypted store) is not built and is out of scope.

### 3. The plugin sandbox

Code: `crates/akit-plugin` (`plugin.rs`: loading and the pool's configuration; `abi.rs`: fit's ABI;
`proxywasm.rs`: the Proxy-Wasm host functions and contexts; `wasm.rs`: the middleware, suspension
and response hooks; `encode.rs`, `response.rs`), `crates/akit-config/src/wasm.rs` (limits and
defaults). Threat model: [plugins](threat-model.md#plugins). Decisions
[008](../decisions/008-wasm-runtime.md) and [011](../decisions/011-proxy-wasm.md).

- **Host functions**: every pointer and length a guest passes is checked against its memory; what
  a guest can read (request and response fields, properties) and change (fields, pseudo-headers,
  local responses, bodies), and that nothing it writes reaches the wire unchecked.
- **Bounds**: time (a deadline a call cannot outlive), memory, instances, the queue, streams,
  bodies, metrics; an instance remade after a trap carries nothing over.
- **Contexts**: a request's context closed in every ending (done, client gone, response cut,
  instance remade); whether one request's data can reach another's context.
- **akio-wasm as akit uses it**: the instance limits and WASI configuration akit sets. The
  interpreter's own validation and execution are akio's ([below](#akio)), and a natural extension
  if time allows.

### 4. Request parsing and the proxy's header handling

Code: `crates/akit-listener/src/lib.rs` (the dispatcher: canonical paths, absolute-form targets,
limits, PROXY protocol), `crates/akit-router` (`path.rs`, `matcher.rs`, `expr.rs`),
`crates/akit-proxy` (`headers.rs`: hop-by-hop fields, forwarding fields and trust; `lib.rs`;
`websocket.rs`, `grpc.rs`, `relay.rs`, `retry.rs`, `mirror.rs`), `crates/akit-middleware` (`auth.rs`,
`jwt.rs`, `jws.rs`, `jwks.rs`, `forward.rs`, `limit.rs`, `rewrite.rs`, `headers.rs`, `redirect.rs`),
`crates/akit-handler` (`static_files.rs`, `fastcgi/`, `forward_proxy.rs`), `crates/akit-l4`
(`forward.rs`, `entry.rs`), `crates/akit-cache` (`cache.rs`, `semantics.rs`). Threat model:
[clients](threat-model.md#clients), [upstreams](threat-model.md#upstreams).

- **Smuggling and desync**: whether akit and an upstream can read one request's bounds or headers
  differently (HTTP/1.1 to HTTP/1.1, h2 or h3 to HTTP/1.1, WebSocket and CONNECT upgrades, gRPC);
  HTTP/1 parsing itself is akio's.
- **Routing**: whether two requests that akit routes differently can mean the same resource to the
  upstream, or the other way round (canonical paths, `%2F`, `strip_prefix`, rewrites, absolute
  forms, `Host` against `:authority`).
- **Trust of forwarding fields**: `X-Forwarded-*`, `Forwarded`, PROXY headers, from trusted and
  untrusted peers; the fields auth middleware sets for upstreams.
- **Authentication middleware**: basic auth, JWT (algorithms, keys, claims), JWKS fetching, forward
  auth, client certificates per route.
- **Files**: the static handler's and FastCGI's paths, dotfiles, `try_files` (symbolic links: L7).
- **The forward proxy**: destination rules, DNS rebinding, the host's own addresses (no
  authentication and private ranges reachable: L6).
- **The cache**: keys, `Vary`, what is shared between users.

### 5. The eBPF sockmap path

Code: `crates/akit-l4/src/sockmap.rs` and `relay.rs`, their use in `crates/akit-listener/src/lib.rs`;
akio's `akio-ebpf` relay (`akio::ebpf::Relay`) as akit loads and feeds it.
[Decision 010](../decisions/010-sockmap-relay.md); threat model:
[layer 4 and the sockmap relay](threat-model.md#layer-4-and-the-sockmap-relay).

- Which connections are spliced, and whether a check that runs in userspace (a PROXY header, TLS,
  `max_conns`, idle timeouts, a reload's drain) can be skipped by one that is.
- The privileges loading takes, what happens without them, and whether the loaded program and maps
  can be reached by another process.
- The relay's bounds (pairs, the sockets' buffers) and the fallback when they are reached.
- akit reads a connection's TCP counters through akio's doc-hidden `Conn::inner` until akio API
  request A170 is pinned.

### 6. If time allows: providers and shared stores

`crates/akit-provider` (Docker, Kubernetes, Consul, etcd: clients, parsers, the merge) and
`crates/akit-tls/src/shared` (Redis, etcd, Consul as certificate stores). Their transports'
defaults are a known limitation (L4); their parsers are fuzzed. Threat model:
[providers and shared stores](threat-model.md#providers-and-shared-stores).

## Fuzz targets

I77 added a target for every parser akit owns (`fuzz/`, cargo-fuzz, a workspace of its own); each
calls its crate's `fuzzing` module, and `cargo test` replays the checked-in corpus
(`fuzz/corpus/<target>`). CI runs them for a fixed time on each change and longer daily
(`.github/workflows/fuzz.yml`). The PROXY protocol and ClientHello parsers are akio-mux's, fuzzed in
akio.

| Area | Targets |
| --- | --- |
| Config readers and converters (operator input) | `config_json`, `config_conf`, `config_yaml`, `config_value`, `config_adapt`, `convert_nginx`, `convert_caddyfile` |
| Routing | `router_expr`, `router_path`, `router_find` |
| The proxy's headers | `proxy_headers`, `proxy_grpc_timeout` |
| Middleware | `jwt_token`, `jwks_keys`, `middleware_fields` |
| Files and FastCGI | `static_path`, `fastcgi_paths`, `fastcgi_response` |
| Layer 4 and the forward proxy | `l4_hello`, `l4_forward` |
| Cache | `cache_semantics`, `cache_disk` |
| TLS and stores | `tls_acme`, `tls_redis_reply` |
| Admin API | `admin_pointer` |
| Providers | `provider_docker`, `provider_kubernetes`, `provider_consul`, `provider_kv`, `provider_merge` |
| Plugins | `plugin_pairs` |
| Logs and upstreams | `observe_access_log`, `observe_escapes`, `observe_tzif`, `upstream_keys` |

```bash
cargo +nightly fuzz run proxy_headers -- -max_total_time=600
```

## Out of scope

- **akio's internals** ([below](#akio)): its parsers, TLS, X.509, cryptography, QUIC, the
  WebAssembly interpreter, the I/O backends and its `unsafe`. akit's use of them is in scope.
- **What akit does not build yet**: fit's console, tunnel, LLM mesh, WebRTC, streaming, media,
  cluster, AI assistant, BitTorrent, sync, P2P and VPN (the [After 1.0 tracks](../dev-plan.md#after-10));
  their fields are refused as not supported.
- **An operator's own input**: what a config, its files, the environment or the Admin API with its
  token can do is the operator's power (the [threat model's assumptions](threat-model.md#assumptions)).
  The config readers and converters must not crash on it, and are fuzzed, but are not a boundary.
- **Denial of service beyond the configured limits**, or with a limit turned off.
- **Everything not shipped**: `bench/`, `tools/`, `fuzz/`, `tests/`, the conformance harness, the
  deployment examples beyond what `deploy/` installs by default; development dependencies.
- **The known limitations** of the [threat model](threat-model.md#known-limitations): a finding
  that one of them is worse than described is in scope.

## akio

akio is akit's engine and its only dependency, from the same maintainer, with its own
[security policy](https://github.com/lesismal/akio/blob/main/SECURITY.md). Its 1.0 review was its
own: fuzzing of its protocol parsers, cargo-audit and cargo-deny, the
[unsafe review](https://github.com/lesismal/akio/blob/main/docs/unsafe-review.md), Miri, loom,
sanitizers, and [constant-time checks](https://github.com/lesismal/akio/blob/main/docs/constant-time.md)
of its cryptography. Its TLS, X.509, cryptography and QUIC are written in akio, not taken from a
widely reviewed library, and have had no external review (threat model L15). That review belongs
with akio and is recommended before akit's 1.0 relies on it; if the engagement can extend, the
order we would suggest is akio-crypto and akio-tls (handshake, record layer, certificate
verification), then akio-http's HTTP/1 and HTTP/2 parsers, then akio-wasm. akit asked akio for it
as [A225](../akio-api-requests.md#a225-an-external-security-review-of-akios-cryptography-tls-http-parsers-and-webassembly)
(I80b).

## Working on akit

- Build and test: `cargo build --release -p akit`; `cargo test --workspace` (on Linux, set
  `AKIO_TEST_BACKEND=uring` and `AKIO_DEFAULT_BACKEND=uring` for io_uring);
  `cargo clippy --workspace --all-targets -- -D warnings`; `cargo deny check`;
  `cargo audit -D warnings`; `python3 -I tools/release-deps.py`.
- Run: `akit run -c <config>`; `akit validate -c <config>`; the Admin API on `127.0.0.1:2019` when
  `admin` is set ([the Admin API guide](../guides/admin-api.md)).
- Architecture: [architecture](../architecture.md), [modules](../modules.md), the
  [development rules](../rules.md) every change follows, and the [decisions](../decisions/README.md).
- Load tests and benchmarks run only on the maintainer's benchmark host ([rule 23](../rules.md));
  a reviewer's own load testing is welcome on the reviewer's machines.

## Findings

Report findings privately as [SECURITY.md](../../SECURITY.md) says, each with its severity, the
affected code, a reproduction (a config and a request, a test, or a fuzz input) and a suggested
fix. Fixes and the threat model's limitations are tracked together in the
[development plan's I80b](../dev-plan.md#i80b-security-hardening); a finding in akio goes to akio and
reaches akit when akit moves its pin.
