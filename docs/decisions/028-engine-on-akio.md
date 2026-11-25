# 028: fit's `engine` read onto akio's engine

[English](028-engine-on-akio.md) | [简体中文](028-engine-on-akio.zh-CN.md)

Status: accepted in I79 (2026-11-26).

## Context

fit's `engine` sets up fib's engines: `event_loops` that wait for the connections' events,
`workers` (and `min_workers`, `task_queue`) that run the connections' reads and callbacks,
`read_buffer_size`, `write_watermark`, `backlog`, how a loop waits (`loop_wait`), how many events
a wait collects (`poll_batch`) and `kernel_tls_offload`. fit-examples' `limits` example sets most of
them, and akit refused the whole part as not supported yet, so the example could not run on akit
([the gate](../compatibility.md#the-gate)).

akio's engine is built differently ([akio's architecture](https://github.com/lesismal/akio/blob/main/docs/architecture.md)):
each listener has I/O threads that wait for its connections' events *and* run their callbacks, and
a logical pool runs only what may block (`spawn_blocking`: file opens, plugins, the disk cache,
bcrypt). It has a read buffer, a write watermark and a backlog of the same meaning as fib's, and no
setting for how a wait parks or how many events it takes.

## Decision

akit reads `engine` and sets akio's engine that serves the listeners from it, at start:

| fit | akio | Left out or 0 |
| --- | --- | --- |
| `event_loops` | each TCP listener's I/O threads (`Listener::threads`) | akio's: one a core |
| `workers` | the logical pool's `max_threads` | akio's: 8 a core, at least 8 |
| `min_workers` | the pool's `min_threads` (and `max_threads` at least as many) | none |
| `task_queue` | the pool's `queue_capacity` | akio's: 65,536 |
| `read_buffer_size` | `Config::read_buffer` | 64 KiB, fit's and akio's |
| `write_watermark` | `Config::write_high_watermark` | akio's: 256 KiB |
| `backlog` | `Config::backlog` | akio's: 1024 |

`loop_wait` and `poll_batch` describe fib's event loops, which akio does not have; `kernel_tls_offload`
waits for akio's kernel TLS. The three stay refused as not supported yet, by name.

`event_loops` is the setting whose meaning moves most: fib's loops only wait and hand work to the
workers, so fit's default is small (one loop on up to 4 CPUs); akio's I/O threads do the work, so
akit keeps akio's default, one a core, when the config leaves it out, and a config that sets it gets
that many threads on each listener. A config written for fit's small number of loops gets fewer
threads than akio would choose; the reference says so (`engine.event_loops`'s note).

The engine is set up once, when akit starts: a reload that changes `engine` is applied otherwise and
warned of ("engine takes a restart; the running one stays"), as fit's engine is built once.
Listeners a reload adds get the `event_loops` akit started with.

`limits.max_conns` and `limits.max_write_queue_bytes` go to the same engine, at start, as fit's
do: `Config::max_connections` and `Config::max_queued_bytes`. akio counts the connections akit
dials on that engine (to upstreams) in `max_connections` too, though it closes only accepted ones
past it (akio API request A215).

## Consequences

- fit-examples' `limits` example is refused only for `limits.max_pending_bytes`, a budget of bytes
  across every connection that akio's engine does not keep (akio API request A214).
- The other engines akit runs (the Admin API's, the providers', ACME's, OTLP's) keep their own small
  settings, as fit's console and tunnel keep theirs.
- No benchmark: the defaults are unchanged when `engine` is left out.
