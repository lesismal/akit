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
| [`fitbench/akit.go`](fitbench/akit.go) | akit as a server of fitbench's: copied into fit's `bench/cmd/fitbench`, it adds akit to the list of servers, run from the image `akit-bench:<tag>` |
| [`configs/akit/fit.json`](configs/akit/fit.json) | akit's config for the harness, copied into fit's `bench/configs/akit`: the routes fit's `bench/configs/fit/fit.conf` serves, as far as akit serves them |
| [`lib.sh`](lib.sh) | staging, shared by the scripts below |
| [`run.sh`](run.sh) | a run of the harness |
| [`ab.sh`](ab.sh), [`abreport.py`](abreport.py) | a same-run comparison of two akit builds |
| [`results`](results) | the runs checked in, each with its results, report, parameters and the akit commit; a run is fetched into `results/run-<time>` (not checked in) and renamed to keep it, as `baseline-2026-11-24` |

fit's own files are not changed. The scripts upload fit's tree at `FIT_REV` (from the fit
repository beside akit's, or `FIT_REPO`) to the host once, add akit's two files to it each time,
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
(`fixed,static,proxy`), `-protocols` (`http/1.1,h2c,https/1.1,h2`), `-idle` (`http1,tls`), and `-args` for any
other fitbench flag, as `-args '-duration 15s -warmup 5s'`.

The defaults are what akit serves so far: the fixed scenario over HTTP/1.1, h2c, HTTPS/1.1 and h2
(TLS since I09), the proxy scenario (I15) over the same, and idle HTTP/1.1 and TLS connections.
The other servers run every scenario they serve; akit's other scenarios are recorded as skipped
("akit cannot serve it") until the iterations that build them: static files in I12, HTTP/3 in
I10, the cache in I69. Each adds what it
serves to `fitbench/akit.go` and `configs/akit/fit.json`, and the protocols to the defaults.

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
