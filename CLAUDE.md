# akit

The development rules below are loaded into every session and bind every change here; the summary
after them does not replace them.

@docs/rules.md

In short: akit is fit rebuilt in Rust on akio. fit (the sibling repository `../fit`) is the
specification: its code, tests, config reference and decision notes say *what* to build; akit
decides *how* on akio's model. Nothing blocks an akio I/O thread: blocking work goes to the logical
pool with `spawn_blocking`; bodies are streamed; every buffer, queue and table is bounded;
backpressure is linked with `hold_reads` / `on_drain`. akit has no `unsafe` and no async runtime.
What akio cannot do yet is written up in [docs/akio-api-requests.md](docs/akio-api-requests.md) and
added to akio (the sibling repository `../akio`), pushed to akio's `main`, and akit moves to that
commit by its hash (rule 22); it is never worked around in akit in breach of the rules.

**Load tests and benchmarks never run on this machine** (rules 23-24): only on the Linux benchmark
host fit and akio use, one session at a time. Locally run only ordinary tests, fmt and clippy.

**Every doc is bilingual** (rule 26): `X.md` and `X.zh-CN.md`, changed together.

For iteration work, read [docs/progress.md](docs/progress.md) and the iteration's section in
[docs/dev-plan.md](docs/dev-plan.md); when an iteration is done, update both dev-plan files and
both progress files.
