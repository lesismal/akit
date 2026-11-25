# 011: Proxy-Wasm's subset, as fit's, on akio's held heads and body filters

[English](011-proxy-wasm.md) | [简体中文](011-proxy-wasm.zh-CN.md)

Status: accepted in I72 (2026-11-25).

## Context

I72 runs the Proxy-Wasm filters fit runs: fit's
[decision 018](https://github.com/lesismal/fit/blob/main/docs/decisions/018-proxy-wasm-subset.md)
chose Proxy-Wasm v0.2.x's request phase, and
[decision 023](https://github.com/lesismal/fit/blob/main/docs/decisions/023-proxy-wasm-response.md)
added the response's callbacks once fib could hold a response's header (A41) and pass its body
through calls made elsewhere (A46). akit runs plugins on akio-wasm's `InstancePool`
([decision 008](008-wasm-runtime.md)), never on an I/O thread (rule 1), so its response callbacks
need the same two things of akio: akio API requests A166 and A167, built as akio's ADR-128
(`Context::hold_response_head`, `Context::add_body_filter`) and pinned since `dad77a7a`.

## Decision

- **The subset is fit's.** A module that exports `proxy_abi_version_0_2_0` or `_0_2_1` is a
  Proxy-Wasm filter and must export `proxy_on_context_create`, `proxy_on_request_headers`,
  `proxy_on_done`, `proxy_on_log`, `proxy_on_delete` and `proxy_on_memory_allocate`. Each
  instance starts its root context with `vm_config` and `config`, and fails its load if the
  plugin refuses either. Each request is a context of its own; the host module `env` has fit's
  functions with fit's statuses: logging, local responses (with gRPC status), the header maps
  (request with its pseudo-headers, response, trailers) by value and by pairs, the VM and plugin
  configuration and the response body as buffers, properties (the config's, `plugin_name`, the
  request's and `response.code`), and per-instance metrics (at most 256). Calls out, shared data
  and queues, ticks, foreign functions, the request's body and trailer, and setting properties
  answer `Unimplemented` or `NotFound`. A filter that pauses a request without answering it fails
  as `on_error` says. `response`, `max_streams` and `max_body_bytes` mean what they mean in fit.
- **Contexts live on their instance.** A context is kept in its instance's store state, so an
  instance made again after a trap starts with none, as fit's worker does. A followed response's
  callbacks are jobs queued for the instance that has its context (`InstancePool::submit_to`), in
  that instance's queue, sized four calls for each of `max_streams` (fit's); they run before new
  requests. A job that cannot be queued (its instance made again, the pool closed) is a failed
  callback: the response fails, or goes on without the plugin with `on_error: skip`. The request's
  context is closed (done, logged, deleted) at the end of its last callback, or by a job queued
  when the last of its response's hooks is dropped, so that a response cut, a client gone or an
  instance remade never leaves a context open.
- **The response phase runs on akio's hooks.** A filter that exports `proxy_on_response_headers`
  (and `response` is not `none`) has each followed response's head held
  (`hold_response_head`, for at most `wait_timeout` + `timeout` + 1 s, the pool answering before);
  one that exports `proxy_on_response_body` or `proxy_on_response_trailers` with `response: all`
  has its body filtered (`add_body_filter`), so with `response: headers` file bodies keep
  `sendfile`. The changes the request's callbacks made to the response's fields are made by a
  response filter, before the head is held.

## Where akit differs from fit

- **A local response from `proxy_on_response_headers` is sent** in the response's place
  (`HeldHead::respond`), as Envoy sends it: nothing of the response has gone. fib could not
  replace a held header, so fit fails such a response; one sent from the body's or trailer's
  callbacks fails in both.
- **What is written while a body callback runs is held by akio up to 4 MiB** (fit's fib: 64 KB),
  counted in the stream's queued bytes, so a producer that paces itself on them (a proxy's
  upstream) waits as for a slow client; `max_body_bytes` still bounds what the plugin is handed at
  once.
- **A released config's plugin keeps running for the responses it follows**, as fit holds the
  config until they end, but at most 10 minutes (rule 11); its pool closes then.
- **Bounds and checks of akit's own** (rule 11): a field name that is not a token, a value with a
  line break or NUL, text that is not UTF-8, a `:path` that is not an origin-form path and a
  `:method` that is not a token are refused (`BadArgument`) where fit stores them; more than 1024
  changes to a header map in one callback fail the call; `max_streams` is at most 65536 and
  `max_body_bytes` at most 4 GiB. The request's `host` field is its `:authority`, as Go's request
  keeps it. The types of the exports Proxy-Wasm calls are checked as an instance starts: a filter
  whose `proxy_on_request_headers` takes two params fails its load, where wazero would call it.
- Plugin log lines below info are dropped, as for fit's ABI (I71): akit has no plugin logger.

## Consequences

- Filters written for Envoy with the Proxy-Wasm Go SDK run unchanged: akit's tests run the SDK's
  `http_routing`, `properties`, `http_headers` and `http_body` examples and fit's own `response`
  filter, built by Go and committed gzipped (`crates/akit-plugin/tests/guests`, 4.3 MB), so that
  the tests need no Go toolchain.
- No akio change was needed beyond A165-A167.
- A response through a filter that follows it costs, besides the request's call, a held head and a
  call per body part on the pool; how much, against fit's wazero and against no plugin, is for the
  benchmark tasks (rule 30; the dev plan's WASM row).
