# 008: WASM plugins on akio-wasm, with fit's ABI

[English](008-wasm-runtime.md) | [简体中文](008-wasm-runtime.zh-CN.md)

Status: accepted in I71 (2026-11-25).

## Context

I71 runs fit's WebAssembly plugins: a route's `wasm` middleware loads a module when the config is
compiled and calls it for each request, sandboxed and bounded in time and memory, never on an I/O
thread. fit runs them on wazero
([fit's decision 017](https://github.com/lesismal/fit/blob/main/docs/decisions/017-wasm-runtime.md)),
a runtime in pure Go that compiles to machine code. [Decision 001](001-dependency-policy.md) named
the WASM runtime as the expected exception to akit's dependencies (akio and `std`), behind a
trait of akit's own, with a note giving the crate, its `unsafe` and its dependency count.

The Rust runtimes were weighed for akio's
[ADR-125](https://github.com/lesismal/akio/blob/main/docs/adr/125-wasm-runtime-for-plugins.md):
wasmtime (Cranelift) compiles to machine code, which needs `unsafe` and executable memory, and
brings over a hundred crates; wasmi is a mature interpreter but uses `unsafe` in its executor and
is a third-party dependency. Neither runs without `unsafe` (rule 14), and each is the kind of
dependency rule 21 sends to akio instead.

## Decision

- **The runtime is akio's own `akio-wasm`** (akio ADR-125, akit API request A165): an interpreter
  of WebAssembly 2.0's core without SIMD, `#![forbid(unsafe_code)]`, with no dependency but
  `akio-crypto` (WASI's `random_get`). It decodes and validates a module once (`Module`), makes
  instances with a memory bound and a call depth (`InstanceLimits`), runs each call under a budget
  (a deadline checked every slice of instructions, an interrupt), implements WASI preview 1 for
  reactors, and runs plugins on `InstancePool`: threads that own instances, a bounded queue that
  refuses a job rather than wait, a deadline per job, an instance made again after a trap. Being
  akio's, it needs no exception to decision 001 and no trait of akit's in front of it.
- **The ABI is fit's** (`fit_on_request` and the host module `fit`), implemented in `akit-plugin`
  on `akio-wasm`'s `Linker`, so that fit's plugins run unchanged; fit's Go test plugin is run by
  akit's tests. Proxy-Wasm, fit's decision 018, came in I72 ([decision 011](011-proxy-wasm.md)).
- **Requests wait suspended** (`Context::suspend`, ADR-109) while their call waits for and runs on
  an instance, and go on from the pool's completion; their fields are changed through
  `Context::req_mut` (A33).

## Consequences

- An interpreter is slower than wazero's compiler: ADR-125's spike measured wazero's interpreter
  at about eight times its compiler's cost per guest call (29.6 us against 3.6 us on fit's
  benchmark). akio-wasm's cost, and a request's through a no-op plugin against none, are measured
  by the benchmark tasks (akio's W5; rule 30), not by this note.
- No machine code is generated, so no `unsafe`, no W^X memory and no compiler in the binary; a
  plugin cannot reach more than its memory and the host functions it is given.
- fit's Go plugins carry Go's runtime: each instance's `_initialize` runs millions of instructions,
  paid when the instance is made (as the config is compiled, or after a trap), not per request.
