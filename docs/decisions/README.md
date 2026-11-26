# Decisions

[English](README.md) | [简体中文](README.zh-CN.md)

Short notes on decisions that shape akit: a dependency, a format, a design choice that later work
must follow. One file per decision, `NNN-title.md` with its Chinese twin `NNN-title.zh-CN.md`,
numbered in order and never renumbered. A decision that is replaced stays, with its status saying
which one replaces it.

Each note has a **Status** (proposed, accepted in an iteration, replaced by NNN), a **Context**, the
**Decision** and its **Consequences**. Where fit made the same decision, the note links fit's and
says whether akit follows it.

| No. | Decision | Status |
| --- | --- | --- |
| [001](001-dependency-policy.md) | Dependencies: akio, the standard library, and few others | accepted |
| [002](002-config-compatibility.md) | akit reads fit's config | accepted |
| [003](003-module-registry-and-traits.md) | The module registry and the data-plane traits | accepted |
| [004](004-graph-publication-and-retirement.md) | Publishing graphs, reading them per request, retiring them from the control plane | accepted |
| [005](005-yaml-reader.md) | YAML is read by a reader of akit's own | accepted |
| [006](006-admin-api.md) | The Admin API on an akio engine of its own, applied by the control thread | accepted |
| [007](007-opentelemetry.md) | OpenTelemetry: fit's tracer, exported through akio's HTTP client | accepted |
| [008](008-wasm-runtime.md) | WASM plugins on akio-wasm, with fit's ABI | accepted |
| [009](009-packaging.md) | Packaging: static musl binaries linked by rust-lld, fit's image, chart and units | accepted |
| [010](010-sockmap-relay.md) | The eBPF sockmap relay, asked for by a listener's `sockmap`, an akit extension | accepted |
| [011](011-proxy-wasm.md) | Proxy-Wasm's subset, as fit's, on akio's held heads and body filters | accepted |
| [028](028-engine-on-akio.md) | fit's `engine` read onto akio's engine | accepted |
| [029](029-v1-freeze.md) | The v1 freeze: baselines, marked changes, and two checks | accepted |
| [030](030-supply-chain-checks.md) | Supply-chain checks: cargo-deny, cargo-audit daily, and the release binary's graph | accepted |
| [032](032-limits-on-akio-budget.md) | The limits taken at start on akio's engine budget, and h2c alone | accepted |
| [033](033-log-files.md) | Log files, rotation and retention, as fit's | accepted |
| [034](034-access-lists.md) | Access lists on akio's accept filter | accepted |
| [035](035-admin-api-redacts-secrets.md) | The Admin API redacts secrets, akit warns of exposed ones, and the rest of I80b's limitations | waiting for review |
| [036](036-console.md) | The console on an engine of its own, fit's pages served as fit ships them, the store waiting for akio | waiting for review; names amended by 037 |
| [037](037-akit-names.md) | akit's names: every external name that says "fit" says "akit", with no aliases | accepted |
| [038](038-tunnel-and-llm.md) | The tunnel and the LLM feature: fit's roles, akit's names, fit's wire protocols | waiting for review |
