# 037: akit's names: every external name that says "fit" says "akit", with no aliases

[English](037-akit-names.md) | [简体中文](037-akit-names.zh-CN.md)

Status: accepted (2026-11-26), decided by the maintainer; I97. It reverses what decisions 002, 006,
007, 008 and 036 kept of fit's names, and amends them.

## Context

akit ports fit (decision 002), and until now kept fit's names wherever akit shows a name to the
outside, so that fit's tools worked with akit unchanged: the Admin API's `Fit-Config-Version` and
`Fit-Dry-Run`, the console's cookie `fit_console_<port>` and header `X-Fit-Csrf` (decision 036,
3), the `Fit-Error` trailer, the metrics `fit_*`, the WebAssembly plugin ABI (`fit_on_request`
and the host module `fit`, decision 008), the defaults that name the gateway (Docker labels under
`fit.`, KV keys under `fit/`, the sticky cookie `fit_sticky`, the realm `fit`, the telemetry
service `fit`, the Kubernetes controller `lesismal.github.io/fit`), the default config file
`fit.json` and the console's pages, fit's workbench saying "fit". A user of akit then sees fit's
name on akit's own surfaces, and akit and fit side by side take each other's labels, classes and
keys. akit has had no release, so its names can still change without breaking anyone's akit.

## Decision

1. **Every external name that says "fit" says "akit"**: header fields, cookies, metrics, trace
   attributes, the plugin ABI, defaults that name the gateway, the CLI's format name, the default
   config file, file names akit writes, and the console's pages. **No old name is kept as an
   alias**: fit's names are not accepted, read or answered by akit.
2. **What does not change**: the config's field names (none says "fit"; akit reads fit's config
   format, decision 002), fit's config format itself (`.conf`, YAML, JSON) and the `fitstore` store
   format I93 is to read and write as fit's (decision 036, 6); Proxy-Wasm's ABI (`proxy_*`, a
   standard of its own, decision 011); and "fit" where it names fit, the specification, in docs,
   comments and messages ("as fit does", "fit's `X`"). `akit adapt`'s converter warnings and
   errors (103 of them) name the gateway akit, as a fit user reads them from akit; the golden test
   reads akit's name in them as fit's before comparing with fit's golden files byte for byte
   (fit's corpus never holds "akit").
3. **What was renamed**:

   | Where | fit's | akit's |
   | --- | --- | --- |
   | Admin API header fields | `Fit-Config-Version`, `Fit-Dry-Run` | `Akit-Config-Version`, `Akit-Dry-Run` |
   | The trailer of a body cut mid-way (proxy, FastCGI) | `Fit-Error` | `Akit-Error` |
   | The console's cookie and CSRF header | `fit_console_<port>`, `X-Fit-Csrf` | `akit_console_<port>`, `X-Akit-Csrf` |
   | The export's download name | `fit-<members>-v<n>.<format>` | `akit-<members>-v<n>.<format>` |
   | Prometheus metrics (17 families, `/metrics` and OTLP) | `fit_*` | `akit_*` |
   | OTLP: the instrumentation scope, span attributes | `fit`, `fit.listener`, `fit.route`, `fit.upstream.address` | `akit`, `akit.listener`, `akit.route`, `akit.upstream.address` |
   | `Cache-Status`'s cache name | `fit; hit`, `fit; fwd=...` | `akit; hit`, `akit; fwd=...` |
   | The WASM plugin ABI | export `fit_on_request`, module `fit` | export `akit_on_request`, module `akit` (and `fit_sync*` as `akit_sync*` for fit's guest package) |
   | Defaults (9 fields) | `label_prefix` and `tag_prefix` `fit`; `prefix` of Consul KV, etcd and `acme.shared_storage` `fit/`; `controller_name` `lesismal.github.io/fit`; `realm` and `telemetry.service_name` `fit`; `sticky_cookie.name` `fit_sticky` | `akit`, `akit/`, `lesismal.github.io/akit`, `akit`, `akit_sticky` |
   | Kubernetes Ingress annotations | `fit.lesismal.github.io/*` | `akit.lesismal.github.io/*` |
   | `jwt`'s challenge | `Bearer realm="fit"` | `Bearer realm="akit"` |
   | The default config file of `run`, `validate`, `adapt`, `fmt` | `fit.json` | `akit.json` |
   | `adapt`'s name of the `.conf` format (`-from`, `-to`) | `fit` | `akit` |
   | The console's pages | fit's workbench, "fit" | the same files renamed (below) |

   akit's own log, logger names, `User-Agent` (`akit-health-check`), ACME storage
   (`~/.local/share/akit`), environment variables and Docker image were akit's already.
4. **The console's pages are fit's workbench renamed by a deterministic transform.**
   `tools/console-pages.py` copies fit's `app/workbench/web` at the gate's pin and applies its
   `RENAMES` in order to every text file but `vendor/`'s: `FIT_*` to `AKIT_*`, `Fit` before a
   capital or `-` (header fields, the pages' globals) to `Akit`, the token prefixes `fitt1.` and
   `fite1.`, "a fit" to "an akit", the plural "fits" of the gateway to "akits", and every other
   `fit` not inside a longer word to `akit`; `KEEP` leaves `auto-fit` (CSS), "as you see fit" and
   `fitstore`. The same transform writes fit's config schema as `console/schema.json`, which the
   console serves, so that the Chinese descriptions, keyed by the English ones, still match. Each
   file it changed starts with a notice, as Apache-2.0's section 4(b) asks, and
   `console/CHANGES` lists the renames; fit's `LICENSE` stays as it is. `--check` (in `make
   compat-check`) checks that every file is a fixed point of the renames, that `schema.json` is
   fit's schema renamed, and, given a clone of fit, that every file is fit's renamed byte for byte;
   a test checks that the pages send and read the server's names.
5. **The WebAssembly test plugin is rebuilt for akit's ABI.** fit's guests no longer load in akit
   (a test says so). `tools/plugin-guest.sh` builds `crates/akit-plugin/tests/guests/headers-akit.wasm.gz`
   from fit's `plugin/wasm/guest` and `plugin/wasm/testdata/headers` at the gate's pin, with the
   `//go:wasmimport` and `//go:wasmexport` names renamed (`fit` to `akit`, `fit_*` to `akit_*`) and
   nothing else changed, with Go 1.27.3 (`GOOS=wasip1 GOARCH=wasm -buildmode=c-shared -trimpath`,
   `gzip -9 -n`); `--check` builds it again and compares. fit's own build stays as
   `headers-fit.wasm.gz`, for the test that refuses it and for the benchmark's fit server.
6. **The v1 baselines are written again, once, before the first release.** The Admin API's
   (`tests/compat/v1/admin-api.txt`) has its two header fields renamed by hand, with a comment
   saying so; the config's (`config.txt`) by `python3 tools/v1-freeze.py --write-baseline`, which
   now writes the surface less the additions marked since and takes akit's defaults from
   `tools/config-reference-notes.json`'s `defaults` (9 items changed); the two sums in
   `tools/v1-freeze.py` are the new files'. `make v1-bless` still refuses a breaking change: this
   one is akit's last before 1.0.

## Consequences

- **fit's tools do not work with akit unchanged.** fit's Grafana dashboards and alert rules read
  `fit_*` metrics; fit's scripts send `Fit-Config-Version`, `Fit-Dry-Run` and `X-Fit-Csrf`; fit's
  WebAssembly plugins export `fit_on_request` and import from `fit`; fit's containers carry `fit.`
  labels, its Consul services `fit` tags, its KV and etcd keys and shared certificates sit under
  `fit/`, its Ingresses carry `fit.lesismal.github.io/` annotations, and its GatewayClasses name
  `lesismal.github.io/fit`. Each has to change, or the config has to say fit's name: `label_prefix
  fit`, `tag_prefix fit`, `prefix fit/`, `controller_name lesismal.github.io/fit`, `realm fit`,
  `service_name fit`, `sticky_cookie { name fit_sticky }`. The [migration
  guide](../guides/migrating-from-fit.md) lists what to change.
- akit and fit side by side no longer take each other's labels, keys, classes or cookies by
  default; sharing certificates through a store takes the same `shared_storage.prefix` on both.
- Clients' sticky sessions and logins do not survive a move from fit to akit: the cookies' names
  differ.
- The gate still reads fit's config files unchanged; what it checks of the defaults is akit's.
- A fit commit the gate pins moves the pages with it: `tools/console-pages.py` and
  `tools/plugin-guest.sh` are run again, and the renames reviewed for names fit added.
