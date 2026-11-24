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

    In a feature task, this column is left to the separate benchmark tasks (rule 30).

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

## 13. Cleaning up

29. **Every development, test and benchmark run cleans up after itself and frees the space it
    took.** When the work is done (an iteration, a fix, a test or benchmark run, a reproduction),
    remove what it left behind, on this machine and on the benchmark host: build output made for it
    (the `target/` directories of copies and worktrees, test binaries), copies of the tree and git
    worktrees, containers, images and volumes it created, and logs and temporary files no longer
    needed. Caches kept on purpose (the bench harness's shared target directory, the pinned server
    images) stay, but are pruned when they grow. The benchmark host's disk is shared by every
    session: check its free space (`df -h /`) before a large build, and never leave it fuller than
    you found it; a full disk stops everyone's builds and runs.

## 14. Feature work and benchmarks

30. **A feature task runs the basic CI tests, not load tests or benchmarks.** A task that develops a
    feature or fixes a bug runs what CI runs: `cargo test` of the crates it touches (and, before a
    push, the workspace suite rule 25 describes), fmt, clippy and `cargo deny`. It runs no load
    tests, benchmarks, A/B comparisons, perf profiles, full benchmark harness runs, or stress and
    soak runs (high repeat counts, long runs under load), neither here nor on the benchmark host.
    The benchmark host's time belongs to the benchmark tasks the maintainer runs separately
    (HTTPArena and the like); a feature task that loads the host wastes it and skews or delays those
    runs. Where another rule asks for a measurement, the feature task leaves it to such a benchmark
    task and says in its report what should be measured. A feature task runs a benchmark only when
    the user asks for one in that task. A feature task whose iteration needs a benchmark (rule 24)
    adds or keeps its row in the dev plan's
    [benchmark and optimization status](dev-plan.md#benchmark-and-optimization-status) table,
    marked not benchmarked and not optimized, naming what should be measured; the benchmark task
    that measures or optimizes it updates the row with the date, commit and result.

## 15. No run hangs unnoticed

31. **A hung test costs minutes, not hours.** A bug that deadlocks a test must fail the run
    quickly and visibly, never leave it waiting:
    - **Each test binary has a time limit.** Whole-suite runs and other long test runs start every
      test binary through the cargo runner [`tools/test-timeout.sh`](../tools/test-timeout.sh)
      (`CARGO_TARGET_<TRIPLE>_RUNNER`, e.g.
      `CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER=$PWD/tools/test-timeout.sh`). A binary still
      running after `AKIT_TEST_BINARY_TIMEOUT` seconds (default 900) is reported with a `HANG:`
      line and its threads (the harness names a test's thread after the test, so the hung test
      shows), killed, and the run goes on with the next binary.
    - **A `HANG:` is a failure like any other**: it is reported, and the test is rerun alone a
      few times (with `timeout`) to tell a rare hang from a deadlock the change brought in.
    - **Whoever waits on a long run watches its progress, not only its end.** The watcher checks
      that the log keeps growing and speaks up when it has not grown for 15 minutes; then the
      running processes are looked at (`ps`, the test binary's threads in `/proc/<pid>/task/*/comm`
      and `wchan`). Waiting for the end marker alone is not enough.
    - **Tests have their own deadlines.** A test that waits on a socket, a channel or a condition
      sets a read/write timeout, `recv_timeout` or a bounded wait, so a bug makes it fail rather
      than hang.
