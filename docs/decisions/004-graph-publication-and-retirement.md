# 004: Publishing graphs, reading them per request, retiring them from the control plane

[English](004-graph-publication-and-retirement.md) | [简体中文](004-graph-publication-and-retirement.zh-CN.md)

Status: accepted in I04 (2026-11-23).

## Context

fit reference-counts its graphs and releases one when its last connection ends. In akit, rule 13
forbids bumping a reference count every thread shares on each request, and akio's handlers run
inline on the I/O threads with no way for the control plane to run work on an idle thread. A
per-thread cache of the current graph answers the first; but an idle thread's cache would then keep
an old graph, and the modules it holds (upstream pools, files, plugins), alive indefinitely if
releasing followed the last `Arc`.

## Decision

- **`GraphCell`** publishes graphs: `swap` replaces the current graph under a lock and bumps an
  epoch.
- **Reads go through a per-thread cache** (`with`): the request loads the epoch and refreshes its
  thread's `Arc` only when the epoch moved. Each thread has a slot of its own that it marks
  "inside" while a request runs on the graph and records the epoch it last saw; these are its only
  per-request writes, to a line no other thread writes.
- **Graphs are retired by the control plane** (`collect`), not by the last `Arc`: a replaced graph
  is retired, its modules cleaned up once, when every thread's slot is outside or past it and no
  `Hold` keeps it. The slot's mark and the epoch are sequentially consistent store-then-load on
  both sides, so a request never starts on a graph `collect` retired. The graph's memory goes when
  the last cache drops its `Arc`; its resources go when it is retired.
- **`Hold`** is what keeps a graph past a handler's call: anything that answers later (a detached
  response, a proxied body, a WebSocket, a tunnel) takes one, once, while inside `with`, and drops
  it when done. A graph whose cell is gone is retired by its last `Hold`.
- **Handler params are akit-config's typed values** (`HandlerParams = akit_config::Handler`):
  akit-config decodes and checks params with positions when the config is read; a module's
  constructor takes the typed value and reports only what provisioning finds
  (`ProvisionError::field`), placed at `routes[i].handler.params.<field>`.

## Consequences

- The control plane calls `collect` after each swap and on a timer until `retiring()` is 0: I22
  (SIGHUP) and I28 (Admin API swaps) own that timer.
- Every module whose work outlives a handler's call (I15–I17's proxy, I49's WebSocket, I51's
  tunnels) takes a `Hold`; one that does not risks running after its modules are cleaned up.
- Cleanup runs on the control thread (or with the last `Hold`, possibly on an I/O thread, as
  decision 003 allows), and never blocks.
- Per request: one load of the epoch, a few stores to the thread's own slot, no allocation, no
  lock, no shared read-modify-write; measured in `crates/akit-runtime/benches/graph_read.rs`.
