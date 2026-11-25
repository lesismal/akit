# akit benchmark harness

[English](README.md) | [简体中文](README.zh-CN.md)

akit is measured with fit's harness, not a copy of it: the same scenarios, configs, load
generators and reports, with akit added as a server under test beside nginx, Caddy, Traefik and
fit ([fit's bench/README.md](https://github.com/lesismal/fit/blob/main/bench/README.md) says what
the harness does). The results come only from the benchmark host
([rule 23](../docs/rules.md#10-load-testing)).

## How akit joins fit's harness

| File | What it is |
| --- | --- |
| [`FIT_REV`](FIT_REV) | the fit commit whose harness is used: the one the compatibility fixtures are pinned at |
| [`fitbench/akit.go`](fitbench/akit.go) | akit as a server of fitbench's: copied into fit's `bench/cmd/fitbench`, it adds akit to the list of servers, run from the image `akit-bench:<tag>`; and the `wasm` scenario, with the servers `akit-wasm` and `fit-wasm` that serve it |
| [`configs/akit/fit.json`](configs/akit/fit.json) | akit's config for the harness, copied into fit's `bench/configs/akit`: the routes fit's `bench/configs/fit/fit.conf` serves, as far as akit serves them |
| [`configs/akit/wasm.json`](configs/akit/wasm.json), [`configs/akit/fit-wasm.conf`](configs/akit/fit-wasm.conf) | the `wasm` scenario's route, for akit and for fit, each a second config file beside the server's own |
| [`stampede.sh`](stampede.sh), [`stampede`](stampede) | the stampede check: many clients, one cold key, a slow origin counting its fetches, for nginx, fit and akit |
| [`lib.sh`](lib.sh) | staging, shared by the scripts |
| [`run.sh`](run.sh) | a run of the harness |
| [`ab.sh`](ab.sh), [`abreport.py`](abreport.py) | a same-run comparison of two akit builds |
| [`results`](results) | the runs checked in, each with its results, report, parameters and the akit commit; a run is fetched into `results/run-<time>` (not checked in) and renamed to keep it, as `baseline-2026-11-24` |

fit's own files are not changed. The scripts upload fit's tree at `FIT_REV` (from the fit
repository beside akit's, or `FIT_REPO`; a git worktree of akit sets it) to the host once, add
akit's files to it each time (with the `wasm` scenario's plugin, unpacked from
`crates/akit-plugin/tests/guests/headers-go.wasm.gz`),
and build fitbench, benchreport, idleconn and fit there with Go; they upload akit at the ref asked
for and build it there with cargo, into an image of its own on `ubuntu:24.04`, the base image fit
runs on. Builds take the host's `/root/bench.lock` shared; measured runs take it alone, through
the host's `quiet.sh`, which also quiets the host's own services for the run.

## Running it

```bash
bench/run.sh main
```

starts a run of akit at `main` (a git ref, or `work` for the working tree), with nginx, Caddy,
Traefik and fit, and returns its id. `bench/run.sh -report ID` fetches it into
`bench/results/ID` and prints fit's benchreport tables; `-wait ID` waits for it first. Options,
passed to fitbench: `-servers` (`nginx,caddy,traefik,fit,akit`), `-scenarios`
(`fixed,static,proxy`, and `cache-hot`, `cache-zipf`, `wasm`), `-protocols`
(`http/1.1,h2c,https/1.1,h2`), `-idle` (`http1,tls`), and `-args` for any other fitbench flag, as
`-args '-duration 15s -warmup 5s'`. `BENCH_DIR` (default `/root/akit-bench`) is where on the host
the scripts stage and run; a session that changes the harness while another runs it sets one of
its own.

The defaults are what akit serves so far: the fixed, static and proxy scenarios (static since I12,
from fitbench's files mounted at `/srv/www` as for fit; the proxy since I15, to fitbench's
backend) over HTTP/1.1, h2c, HTTPS/1.1 and h2 (TLS since I09), and idle HTTP/1.1 and TLS
connections. akit also serves the cache scenarios (since I76), `cache-hot` and `cache-zipf`, as
fit and nginx do: the route `cache`, a 1 GiB zone with the cache lock on, in front of fitbench's
cache origin, whose fetches give each run's hit ratio; like fit's harness, a run has them only when
`-scenarios` names them. The other servers run every scenario they serve; akit's other scenarios
(HTTP/3, the website workloads) are recorded as skipped ("akit cannot serve it"). Each iteration
adds what it serves to `fitbench/akit.go` and `configs/akit/fit.json`, and the protocols to the
defaults.

## The WASM plugin's cost

```bash
bench/run.sh -servers akit-wasm,fit-wasm -scenarios fixed,wasm -protocols http/1.1,h2c -idle "" work
```

measures what a header-editing WebAssembly plugin costs a request, in akit and in fit, with the
same `.wasm`: fit's own test plugin (`plugin/wasm/testdata/headers`, built by Go for
`wasip1`), which sets and removes request fields and adds three response fields. The `wasm`
scenario is the fixed response behind the `wasm` middleware, with its defaults (an instance a CPU,
a queue of 1024, 100 ms a call); `fixed` measured in the same container is the baseline. The
servers `akit-wasm` and `fit-wasm` are akit and fit with their harness configs and a second config
file each that adds the route `/wasm` ([`configs/akit/wasm.json`](configs/akit/wasm.json),
[`configs/akit/fit-wasm.conf`](configs/akit/fit-wasm.conf)), so that fit's own config stays as it
is and a run that does not name them loads no plugin.

## The stampede check

```bash
bench/stampede.sh work
```

asks a cache for one cold key with many clients at once (1,000 by default: over HTTP/1.1 a
connection each, over h2c 100 streams on each of 10 connections), while the origin
([`stampede/origin.py`](stampede/origin.py)) takes a second to answer; the origin counts the
fetches that reach it. It runs nginx with `proxy_cache_lock on`, fit, akit, and akit with
`disable_lock` (the control, whose every client reaches the origin), each alone on the server's
CPUs, three cold keys a server and protocol, and prints a line a key: the server, the protocol, the
requests answered 200 and the origin's count. It waits for its end (about two minutes, under the
host's lock alone) and keeps the lines in `results/stampede-<time>`. Options: `-clients`, `-keys`,
`-delay`.

## Comparing two builds

```bash
bench/ab.sh main work
```

compares two builds of akit, in the same run: akit alone, the fixed scenario over HTTP/1.1 and
h2c, 3 rounds alternating A and B, 10s runs after 2s of warm-up, every round under the lock. It
returns an id; `bench/ab.sh -report ID` summarizes what has finished, requests a second, CPU per
request and p99, marking a change only past 1% (`AB_NOISE`) and the spread of the rounds, with
fit's abreport; `-wait ID` waits for the end. Options: `-scenarios`, `-protocols`, `-rounds`,
`-duration`, `-warmup`, `-args`.

## Where akit's config differs from fit's

| Setting | fit | akit |
| --- | --- | --- |
| Port 8081 | h2c alone | HTTP/1.1 and h2c: akio's server always takes HTTP/1.1 as well; no run sends it there |
| Listen backlog | `engine.backlog`, the host's `somaxconn` | akio's default; akit has no `engine` settings yet |
| Connections | `limits.max_conns 1000000` | no limit of akit's own yet |
| Workers | fib's, one per CPU, shared by the listeners | akio's I/O threads, one per CPU the container is pinned to, for each listener; a run loads one listener, whose threads do the work |
| Admin API | at 127.0.0.1:9005, for counting goroutines | none: akit has no goroutines, and fitbench counts none for it |
