# akit development rules

[English](rules.md) | [简体中文](rules.zh-CN.md)

Every change to akit follows these rules. They are what the
[roadmap's design principles](roadmap.md#design-principles) require of the code, adapted from
[fit's rules](https://github.com/lesismal/fit/blob/main/docs/rules.md) and
[akio's rules](https://github.com/lesismal/akio/blob/main/docs/rules.md), which hold the same line
for the Go gateway and for the engine underneath akit.

## 1. Never block an I/O thread

1. **Nothing on an akio I/O thread blocks.** Handlers, middleware, upstream callbacks and
   completions do no synchronous disk I/O (`std::fs`, `File::open`, `metadata`), no DNS
   (`to_socket_addrs`), no `thread::sleep`, no wait on a channel, condvar, join handle or future,
   no blocking `connect`, and hold no lock another thread may hold for long. akio's slow-callback
   detector is on in every test run and reports what slips through.
2. **Upstream requests go through akio's asynchronous clients** (`Client::request`,
   `H2Client::request`, the H3 client), with a completion callback. No blocking HTTP client on the
   request path.
3. **A handler that answers later** keeps what it needs alive explicitly (a detached response, a
   `Conn` clone, an `Arc` to the graph) and cancels its upstream work when the client leaves.
4. **Work that can only block** (disk cache, DNS, ACME, WASM, file reads on a cache miss) runs on
   akio's logical pool with `spawn_blocking`, or on a bounded pool of akit's own, with a timeout.
   A full pool (`Overloaded`) is handled, not unwrapped. Waits between pools go one way only.

## 2. Bodies are streamed and bounded

5. **Request and response bodies are streamed** (`BodyMode::Stream`, `on_body`, the clients'
   streamed responses) and forwarded as they arrive.
6. **Data handed to a callback is valid for that call only.** Keep it past the call by copying it,
   preferably into an `akio-buf` pooled buffer.
7. **Only a feature that really needs the whole body buffers it, always under a limit**, and past
   the limit answers 413 or falls back to passing the body through.
8. **Backpressure is linked.** When one side's pending bytes pass the watermark, `hold_reads` the
   other side; resume on `on_drain`. No queue or buffer anywhere is unbounded.

## 3. Other protocols alike

9. **WebSocket, gRPC streams, CONNECT, SOCKS and TCP/UDP tunnels are driven by callbacks on both
   sides**, with backpressure both ways. They neither block nor buffer whole streams.

## 4. Timeouts and limits

10. **Every exchange with the outside has timeouts**: connect, first byte, idle and total.
11. **Every resource has a limit with a safe default**: connections, streams, sessions, header
    size, body size, concurrency, table sizes keyed by peer input. A config that sets none is still
    bounded; "no limit" is an explicit opt-out.

## 5. The hot path

12. **What can be done when the config is compiled is not left to request time.** Middleware
    chains, matchers, regexes and fixed headers are built at compile time. Requests read the
    immutable graph.
13. **Per request: no global locks, no shared reference counts, as few allocations as possible.**
    The graph pointer is cached per I/O thread; counters are per thread or sharded; nothing is
    formatted that could have been formatted at compile time.

## 6. Rust

14. **No `unsafe` in akit.** Every crate is `#![forbid(unsafe_code)]`. What needs `unsafe` belongs
    in akio, behind its unsafe review.
15. **No panics on peer or config input.** No `unwrap`, `expect`, indexing or arithmetic that can
    overflow on data a client, an upstream or a config file controls; errors are values. A panic in
    a callback takes down an I/O thread's connections, so it is always a bug, and fuzzing hunts
    for it.
16. **No async runtime on the data plane.** No tokio, async-std or `block_on` in an akit crate the
    data plane links; a dependency that needs one is refused or kept to a tool.
17. **`cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo deny check` pass.** The
    MSRV is akio's.

## 7. Verification

18. **Every data plane feature has tests** for slow clients, slow upstreams, large bodies and
    connections cut midway, and for the limit it introduces (under, at and over), on epoll and
    io_uring. New code comes with its tests in the same change.
19. **Behaviour shared with fit is tested against fit's tests.** A feature ported from fit ports its
    tests and examples; a config fit's templates hold must pass `akit validate`, or fail with
    "not supported yet" naming the field.
20. **A new dependency comes with a decision note** in `docs/decisions/`
    ([decision 001](decisions/001-dependency-policy.md)).

## 8. When akio falls short

21. **What akio cannot do yet is added to akio, not worked around in akit.** Write it in
    [akio API requests](akio-api-requests.md) and its Chinese twin: the problem, the evidence, the
    suggested API and how akit would use it. akit's sessions may make the change in akio
    themselves, by akio's own rules (its tests on every backend, its unsafe review, a same-run A/B
    for the hot path). Work that does not depend on it goes ahead; a stopgap follows every other
    rule and is recorded in the progress files with the request that will replace it.

## 9. The akio version

22. **akit depends on akio by commit hash**, `git = "https://github.com/lesismal/akio", rev = "<hash>"`,
    never by a path dependency or a `[patch]` committed in akit. Moving to an akio change means
    updating the `rev` once that commit is on akio's `main` and its checks pass. A tag is used when
    the wanted version has one.

## 10. Load testing

23. **Load tests and benchmarks run on Linux, on the benchmark host**, the same quiet host fit and
    akio use, and nowhere else. Numbers from macOS, Docker Desktop or a loaded host are not used;
    when the host is loaded, only same-run A/B ratios are reported. One session at a time uses the
    host.
24. **Verification is sized to the change.**

    | Change | Benchmark |
    | --- | --- |
    | Docs, tests, the control plane (config, Admin API, certificates, providers) | none |
    | The data plane off the hot path | none, or micro-benchmarks of the crate |
    | The hot path | same-run A/B on the host, alternating rounds, with the scenarios it touches |
    | A milestone's end | the full harness, every server (fit included), and a report |

25. **Tests by reach.** While working, test the crates a change touches and those that depend on
    them; the whole workspace (macOS, Linux epoll, Linux io_uring) runs once before a push. At most
    two sessions run the whole suite at once. Benchmarks run in the background and are read before
    the change they measure is pushed.

## 11. Docs

26. **Every document is bilingual**: `X.md` in English and `X.zh-CN.md` in Chinese, each linking
    the other under its title, changed together in the same commit. A section, table row or
    status added to one is added to the other.
27. **The plan stays true.** An iteration's status changes in both dev-plan files, and what it
    built goes into both progress files, in the commit that finishes it.

## 12. Benchmark reports

28. **Every full-harness report says where akit stands**: its place in each workload and protocol
    against nginx, Caddy, Traefik and fit, with the servers ranked by fit's benchreport scores so
    the two projects' reports read the same way.
