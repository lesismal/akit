# Scheduling A/B, 2026-11-28

akit at e3473df3 (the A303 stopgap removed; built from the working tree that became that commit), one binary in two images: A with `AKIO_DEFAULT_SCHEDULING=event-loops`, B with akio's default (work stealing on epoll). `bench/ab.sh -scenarios static,proxy -protocols http/1.1 -rounds 5 -env-a AKIO_DEFAULT_SCHEDULING=event-loops work work` on gpu51, the measured rounds under `/root/bench.lock` alone; the host's services were left running (no `quiet.sh`), so only the same-run ratios count.

```
A: work (uncommitted changes of 939784ac) with AKIO_DEFAULT_SCHEDULING=event-loops
B: work (uncommitted changes of 939784ac), the same binary
scenarios=static,proxy protocols=http/1.1 rounds=5 duration=10s warmup=2s args= env_a=AKIO_DEFAULT_SCHEDULING=event-loops env_b= fit=7daee573b102d87837cc10f033e4cc5894769210

scenario protocol     A req/s    B req/s   change  A µs/req B µs/req    A p99   B p99  rounds  verdict
proxy    http/1.1        256k       260k    +1.5%     21.35    25.16     3.83    1.09  5/5     within noise
static   http/1.1        776k       730k    -6.0%      8.41     8.56     1.25    0.58  5/5     SLOWER
```
