# 001: Dependencies: akio, the standard library, and few others

[English](001-dependency-policy.md) | [简体中文](001-dependency-policy.zh-CN.md)

Status: accepted (2026-11-23), by the user before I01.

## Context

fit depends on fib and Go's standard library, with a handful of exceptions each recorded in a
decision (ACME's protocol package, wazero). akio's libraries have no required third-party
dependency ([akio ADR-005](https://github.com/lesismal/akio/blob/main/docs/adr/005-dependency-policy.md)):
TLS, QUIC, compression, regexes and crypto are its own. Rust's standard library is smaller than
Go's (no JSON, no HTTP, no crypto), so the pull towards crates such as serde, tokio-based clients or
ring is stronger.

## Decision

- **Data plane crates depend on akio and `std` only.** What akio has is used (`akio-crypto`,
  `akio-x509`, `akio-regex`, the compressors, the HTTP clients); what it lacks and belongs in a
  network library is requested from akio ([akio API requests](../akio-api-requests.md)).
- **No async runtime** in any crate the binary links on the data plane path (tokio, async-std,
  smol). Clients for Kubernetes, etcd, Docker and the like are written on akio's clients, for the
  part of their protocols akit uses.
- **Config readers are akit's own for JSON and .conf**, strict by construction and fuzzed. YAML is
  decided in I24, between a reader of akit's own for the subset fit accepts and a crate, by size,
  `unsafe` and strictness.
- **Exceptions** are allowed where writing our own would be much worse (the WASM runtime is the
  expected one), behind a trait of akit's own, each with a decision note giving the crate, its
  `unsafe` and dependency count, and how it is kept off the I/O threads if it blocks.
- **Dev-dependencies** (test servers, property testing, benchmarking) are free, but stay out of the
  release binary; cargo-deny enforces licenses and duplicates.

## Consequences

akit is a small, auditable binary with one engine underneath, and akio grows where akit needs it.
The cost is more code of akit's own for formats and control-plane clients, and slower early
iterations.
