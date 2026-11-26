# akit architecture

[English](architecture.md) | [简体中文](architecture.zh-CN.md)

How akit is put together, and how a request goes through it. It is the target the
[development plan](dev-plan.md) builds towards; where the code and this page disagree, one of them
is fixed in the same change.

## 1. Layers

```text
┌──────────────────────────────────────────────────────────────┐
│ akit binary: run · validate · adapt · fmt · store · build     │
├───────────────────────────── control plane ──────────────────┤
│ akit-config   readers (.conf, YAML, JSON) → model → validate  │
│ akit-admin    Admin API, file provider                        │
│ akit-provider merging; Docker, Kubernetes, Consul, etcd       │
│ akit-tls      certificate manager, ACME, OCSP                 │
│ akit-runtime  registry · compile · Graph · atomic swap        │
├───────────────────────────── data plane ─────────────────────┤
│ akit-listener → akit-router → akit-middleware → handlers      │
│   (akit-handler, akit-proxy, akit-cache, akit-plugin)         │
│ akit-upstream pools · balancing · health                      │
│ akit-l4       TCP/UDP proxy · SNI · PROXY protocol            │
│ akit-access   allow and deny lists, compiled to ranges        │
│ akit-observe  logs · metrics · tracing                        │
├──────────────────────────────────────────────────────────────┤
│ akio: Engine · Conn · logical pool · akio-http/h3/ws/grpc ·   │
│       akio-tls · akio-mux · akio-socks · compressors · …      │
└──────────────────────────────────────────────────────────────┘
```

The control plane never runs on an akio I/O thread; the data plane never parses config or waits on
the control plane.

## 2. Threads

| Thread | Runs | Never |
| --- | --- | --- |
| akio I/O threads (one per core by default, per listener group) | accept, TLS, HTTP parsing, routing, middleware, proxying, upstream callbacks | blocks, locks for long, reads a file on a cache miss |
| akio logical pool | `spawn_blocking` work: file opens, disk cache, password hashes, WASM calls | waits for an I/O thread |
| control thread | config reading, compile, swap, the Admin API's handling, provider watches' processing | serves traffic |
| bounded writers | access and error logs, OTLP export | grows a queue without a limit |

The Admin API itself is served by akio (on an engine of its own, one I/O thread:
[decision 006](decisions/006-admin-api.md)); the work a request causes, such as a compile, is
handed to the control thread and answered when it completes.

## 3. The graph

`compile(config)` provisions every module of a config into a `Graph`: listeners' settings, routes
with matchers and middleware chains built, services, upstream pools, certificates in use. A graph
is immutable once published.

- **Publish** (`GraphCell::swap`): the control thread replaces the current `Arc<Graph>` under a
  lock and bumps an epoch.
- **Read** (`GraphCell::with`): each I/O thread keeps its own `Arc<Graph>` and the epoch it saw. A
  request loads the epoch and refreshes its copy, under the lock, only when it changed: once per
  thread per swap. Requests borrow the thread's copy: no shared reference count is bumped per
  request ([rule 13](rules.md#5-the-hot-path)); the only per-request writes are to the thread's
  own slot, marking it inside the graph and out again.
- **Keep** (`GraphCell::hold`): a request or connection that outlives its handler's call (a
  proxied response in flight, a WebSocket, a TCP tunnel) takes a `Hold`, once when it starts.
- **Retire** (`GraphCell::collect`): akio cannot run work on an idle I/O thread, whose cache may
  keep an old graph's `Arc` for long. So the control plane retires a replaced graph, cleaning its
  modules up (pools drain, files close), as soon as every thread is outside it or past it and no
  `Hold` keeps it; the thread's mark and the epoch are a sequentially consistent store and load on
  each side, so a request never starts on a retired graph. Cleanup runs on the control thread, once
  ([decision 004](decisions/004-graph-publication-and-retirement.md)).

Listeners are compared between graphs by address and transport: unchanged ones keep their sockets,
new ones are bound, removed ones drained ([A1](akio-api-requests.md#a1-tcp-listeners-on-a-running-engine)).

## 4. A proxied HTTP request

1. akio accepts the connection on an I/O thread; akio-mux (if configured) sniffs the protocol;
   `akio-tls` terminates TLS; `akio-http` parses the head.
2. The router of the thread's graph picks a route: listener's routes, then host, path and the other
   matchers, by priority. No allocation.
3. The route's middleware chain, built at compile time, runs in order. A middleware that must wait
   (ForwardAuth, a WASM call) detaches the response and resumes it from its completion.
4. The `proxy` handler takes a server from the service's balancer, a connection from the upstream
   pool, and sends the head. Request body pieces from `on_body` are forwarded as they arrive.
5. When the upstream's send queue passes its watermark, the client connection's reads are held;
   `on_drain` resumes them. The same holds the other way for the response body.
6. Timeouts (connect, head, idle, total) fire on akio's timers on the same thread; a client that
   leaves cancels the upstream request.
7. The access log line is pushed into the thread's buffer and written by the log writer.

## 5. Layer 4

TCP routes relay bytes between two `Conn`s with linked backpressure; where the listener asks for it
and the kernel allows, the relay is handed to akio's eBPF sockmap and leaves user space. UDP routes
keep a bounded table of peer sessions with idle timeouts. TLS passthrough reads the ClientHello's
SNI without terminating ([A5](akio-api-requests.md#a5-sni-routing-in-akio-mux)).

## 6. Config

One model, three readers (.conf, YAML, JSON), all strict: an unknown field, a duplicate key or a
wrong type is an error with its path, line and column. The model is fit's
([decision 002](decisions/002-config-compatibility.md)); a field fit has and akit does not support
yet is refused as "not supported yet", never ignored. `akit validate` compiles a config with a null
engine, as `fit validate` does.

## 7. Errors and limits

Errors are values on every path; nothing a peer or a config controls can panic. Every table keyed by
peer input (UDP sessions, rate-limit keys, cache entries, sticky sessions) has a cap and an eviction
rule, and every limit has a default, listed in the config reference.

## 8. Testing

- Unit tests per crate; fit's table tests ported where the behaviour is shared.
- End-to-end tests start the binary against real upstreams, slow peers and cut connections, on epoll
  and io_uring.
- The fit compatibility gate runs fit's templates and examples through `akit validate`.
- Fuzz targets for every parser; the benchmark harness on the benchmark host.
