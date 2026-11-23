# akit development progress

[English](progress.md) | [简体中文](progress.zh-CN.md)

What has actually been built, iteration by iteration, and where the next session picks up. The
[development plan](dev-plan.md) says what each iteration is meant to do; this file says what was
done and where the work differs from the plan.

## Where things stand

| | |
| --- | --- |
| Last updated | 2026-11-24 |
| Milestone | M0, foundations, done (I01–I08); M1, the proxy core: I09, I11, I14, I23 and I24 done. |
| akio | `50a82315` on akio's `main` (`1.0.0-rc.1`), pinned by `rev` in `Cargo.toml` |
| fit | the specification; read at `7daee573` for this plan |
| What works | `akit run` and `akit validate` with fit's flags and exit codes, `-c` repeated, files and directories merged, in JSON, fit's `.conf` and YAML; a graceful shutdown on SIGINT and SIGTERM; HTTP/1.1, h2c and TLS listeners (certificates from files chosen by SNI, ALPN choosing h2 or HTTP/1.1, client authentication); `respond` routes chosen by router v1 (fit's host and path rules by lookup, on canonical paths), 404 when nothing matches; `upstreams` compiled into a pool each on akio's HTTP/1 client, drained when their graph is retired (the proxy that sends through them is I15); the compatibility gate: 80 fixtures, 5 valid, 74 refused only as not supported yet, 1 skipped, none failed; the I08 baseline: akit first in the fixed scenario over HTTP/1.1 and h2c |
| Next | I15 (reverse proxy I, on I11 and I14), I10 (HTTP/3), I12 (static files), I19 (load balancing), I22 (reload) |
| akio requests | A1–A8 being added to akio, each on its own akio branch, not merged; A9 (a TLS server capped at TLS 1.2) requested by I09; A10–A12 (HTTP client: closing idle connections, a request's own `Host`, the connect timeout for a lone address) by I14 ([akio API requests](akio-api-requests.md)) |

## Log

### 2026-11-24: I14, upstream pools on the HTTP/1 client

- `upstreams` are read (`akit_config::Upstream`), joined across files as fit joins them: `name`,
  `addresses`, `protocol` (`http/1.1`; `h2c` and `h2` refused as not supported yet),
  `max_conns`, `max_pending`, `max_idle_conns`, `idle_timeout` and `connect_timeout`, with fit's
  defaults (10000, 50000, 32, 60s, 10s; 0 is the default for all but `max_idle_conns`, where it
  keeps none) and fit's checks and messages (a valid name unique among the upstreams, at least
  one address, each a host and a port from 1 to 65535 and not a URL, none listed twice). fit's
  durations are Go's (`akit_config::duration`), negative ones refused. fit's 15 other upstream
  fields are refused as not supported yet by name.
- `akit-upstream`: a `Pool` per upstream on akio-http's `Client`. Per address, at most
  `max_conns` requests under way, which bounds the connections (HTTP/1.1 carries one request at a
  time, and akio's client reuses an idle connection before it dials and puts one back idle before
  it calls back); the next ones wait in order, at most `max_pending`, and one more fails at once
  with `Error::Busy`. Taking a slot is a compare-and-swap while nobody waits; the address's queue
  lock is taken only at `max_conns`. A queued request is started from the completion that frees
  its slot, never recursively (a thread-local trampoline). `max_idle_conns` and `idle_timeout`
  are akio's per-host idle pool; `connect_timeout` is akio's Happy Eyeballs per-attempt timeout;
  each exchange, its wait for a connection included, is bounded by fit's `response_timeout`
  default (1h, the field itself is I17's). `pick` gives the addresses in turn (fit's
  `round_robin`, its default; I19 the rest). `Error::is_upstream_failure` says which failures
  count against an address (I20).
- Drain: `Pool::close` refuses new requests with `Error::Closed` and lets those sent before it
  finish, the queued ones included; the pool is closed (`is_closed`, `on_closed`) once the last
  is done. `compile` makes the pools before the modules, gives them to the modules through
  `Env::upstream` (what I15's proxy takes at provisioning), keeps them in the graph
  (`Graph::upstream`), and records their cleanup first so it runs last: a retired graph, or a
  config that fails to compile, closes its pools (decision 004).
- The engine: pools are made with the graph, before the engine (built from the graph's
  listeners), so they reach it through an `EngineSlot` that `akit run` fills once the engine is
  built; connections are dialed on the listeners' threads, on the same engine. `akit validate`
  leaves the slot empty: its pools fail every request with `Error::NoEngine` and never dial.
- The compatibility gate: 9 upstream fields supported (29 in all); `templates/json/fit.json` is
  now valid, `templates/json/conf.d` refused only for the `proxy` handler (`make compat-bless`).
- Tests: `akit-config/tests/upstreams.rs` (5: defaults, limits, fit's checks and messages, what
  is not supported yet, joining across files) and Go durations; `akit-upstream/tests/pool.rs`
  (8: `max_conns` and `max_pending` at and over the limit, `max_pending` 0 as fit's default, idle
  connections kept, none kept and closed after `idle_timeout`, a pool drained under load with 4
  under way and 16 queued, an idle pool closed at once, an upstream that refuses connections
  alone and beside one that answers, no engine and the slot filled later, bad targets and
  timeouts while queued); `akit-runtime/tests/upstreams.rs` (3: pools compiled and given to
  modules, a failed compile closes its pools, a retired graph drains its pools under load);
  `akit-listener/tests/upstreams.rs` (2: pools dial on the listeners' engine, `connect_timeout`
  against an address that never answers); the gate's field samples.
- Checked: `make check` on macOS; the tests of akit-config, akit-upstream, akit-runtime,
  akit-listener, akit and akit-compat on Linux (the benchmark host), epoll and io_uring, all
  passing. The connect timeout test needs an address that leaves connects unanswered (a full
  accept queue on Linux) and skips on macOS.
- Differences from the plan:
  - `max_conns` bounds requests under way per address rather than counting sockets: the same
    bound over HTTP/1.1 (the tests check the upstream never sees more connections), and akio's
    client keeps its connections to itself.
  - A request waiting for a connection has no timer of its own: its wait counts toward the
    exchange's timeout, checked when it gets a connection (fib's client bounds the wait the same
    way); a full queue is refused at once.
  - Stopgaps for what akio lacks: a closed pool's idle connections stay open until their
    `idle_timeout` or the engine's shutdown (A10, `Client::close_idle`); akio applies the client's
    connect timeout only to a name with several addresses, so `akit_listener::engine` sets the
    engine's `connect_timeout` to the shortest of the upstreams' (A12). I15 needs A11 (the
    request's own `Host`). Hand-over notes for I15 are in the session notes folder.
  - Not done here: a pool kept across graphs on reload (fit's `Acquire`), I22's.

| Last updated | 2026-11-23 |
| Milestone | M0, foundations, done (I01–I08); M1, the proxy core, next. |
| akio | `50a82315` on akio's `main` (`1.0.0-rc.1`), pinned by `rev` in `Cargo.toml` |
| fit | the specification; read at `7daee573` for this plan |
| What works | `akit run` and `akit validate` with fit's flags and exit codes (`-c` repeated, files and directories, merged), and a graceful shutdown on SIGINT and SIGTERM; `akit run` serves HTTP/1.1 and h2c listeners with `respond` routes, by fit's host and path rules, 404 when nothing matches; `akit version` and `akit help`; the workspace builds and passes `make check`; `akit_config::from_json` reads fit's listeners, routes and the `respond` handler, and refuses the rest of fit's config by name as not supported yet; the module registry and the data-plane traits (`akit-runtime`); `compile` turns a config into a `Graph` and `GraphCell` publishes, reads and retires graphs; the compatibility gate runs every fit fixture through akit-config in `make check` and CI |
| Next | M1: I09 (TLS), I11 (router v1), I14 (upstream pools), I23 (.conf) and I24 (YAML) are on branches of their own, to merge; then I10, I12, I15 onwards. |
| akio requests | A1–A8 all being added to akio now, each on its own akio branch ([akio API requests](akio-api-requests.md)) |

## Log

### 2026-11-24: I11, router v1: host and path

- `akit_router::Router` looks routes up instead of scanning them, as fit's `router` package does
  (`router/router.go`, without its matchers, which are I39): maps of host patterns (exact, `*.` by
  the suffix from its dot, `**.` likewise, and the routes with no hosts), and under each host
  pattern maps of exact paths and of prefixes, with the prefixes' lengths longest first. A lookup
  tries the request's exact host, its `*.` suffix and up to four `**.` suffixes, the longest first,
  then the routes with no hosts; under each, its exact path and then each prefix length, stopping
  once no prefix left can win. The order is fit's: priority, then host, then path specificity,
  then the route written first. A lookup does not allocate: a host with capitals is lower-cased in
  a stack buffer (one longer than 255 bytes matches only routes with no hosts, as in fit).
- Not in fit: the maps hash with an FxHash-style hasher of akit's own (they are built from the
  config and only read, so a request's host cannot lengthen a probe beyond what the config put
  there); a map of up to 8 keys is scanned rather than hashed; a router whose routes have no hosts
  does not look at the host. Since there are no matchers yet, each host and path pattern keeps only
  its best route.
- `akit_router::canonicalize` puts a request's path in fit's canonical form (`listener/path.go`):
  dot-segments removed as RFC 3986 says, `%2E` counting as `.`, empty segments removed; a path
  whose `..` climbs above the root, or that, decoded, has an empty or dot-segment that came
  encoded (`%2F`, `%5C`, `\`), is refused. A path with nothing to decode or remove is only
  scanned. `akit_listener::Dispatcher` canonicalizes every request before routing it and answers
  a refused path `400 bad request path\n`, as fit does; routes match the percent-decoded path, as
  fit's match Go's `URL.Path`, so `Router::find` and `Graph::route` take the path as bytes
  (`impl AsRef<[u8]>`: a decoded path need not be UTF-8).
- Tests: fit's router table tests, ported (`akit-router` `src/tests.rs`: `TestHostOnly`,
  `TestHost`, `TestDeepWildcard`, `TestPath`, `TestNoRoutes`, the nine `TestOrder` tables,
  `TestPathTwice`, `TestHostOnlyUnclosed`), v0's tests kept, and new ones for long hosts, many
  prefix lengths, a non-UTF-8 path and the bound on `**.` suffixes; fit's `TestCanonicalize`
  table, ported, plus malformed escapes and non-UTF-8 escapes; `tests/no_alloc.rs`, fit's
  `TestMatchAllocs` and `TestCanonicalizeAllocs`: a binary of its own, without the test harness,
  counting with `stats_alloc` as its global allocator (a dev-dependency: akit has no `unsafe` to
  write one, rule 14), checks that seven lookups, hits and misses, upper case, ports, `**.` and
  IPv6, and canonicalizing three canonical paths make no allocation in 100 calls each, and that
  the count is real (a path with dots does allocate); `akit-listener` `tests/listeners.rs`: fit's
  `TestPathConfusionRoutes` with `respond` routes, over HTTP/1.1 and h2c (paths spelled to miss
  a protected route get it, or 400), and the canonical path a handler sees.
- Micro-benchmark `crates/akit-router/benches/lookup.rs` (`harness = false`): v0 (I05's scan,
  kept in `benches/v0.rs`) against v1, ns per lookup, on fit's `BenchmarkMatch` table (each route
  a host of its own with a prefix and an exact path, plus a `*.` catch-all) and on the one-route
  config the benchmark scenarios serve. On the benchmark host, under `quiet.sh`:

  | ns per lookup | v0 | v1 | v0/v1 |
  | --- | ---: | ---: | ---: |
  | 1 route, `/` | 12.5 | 7.7 | 1.6x |
  | 10 routes + catch-all, middle route | 69.0 | 36.7 | 1.9x |
  | 10 routes + catch-all, catch-all | 48.6 | 24.2 | 2.0x |
  | 10 routes + catch-all, no route | 33.2 | 16.3 | 2.0x |
  | 100 routes + catch-all, middle route | 383.3 | 37.2 | 10.3x |
  | 100 routes + catch-all, catch-all | 315.4 | 24.1 | 13.1x |
  | 100 routes + catch-all, no route | 227.7 | 16.1 | 14.1x |
  | 1000 routes + catch-all, middle route | 3523.8 | 30.9 | 114x |
  | 1000 routes + catch-all, catch-all | 2292.4 | 23.9 | 96x |
  | 1000 routes + catch-all, no route | 2207.1 | 15.8 | 140x |

  v1 costs about the same at 10 routes as at 1000; v0 grows with the routes. Before small maps
  were scanned rather than hashed, v1 was slower than v0 on the one-route config (15.7 against
  12.6 ns); the scan made it 7.7.
- A/B, same run (`bench/ab.sh 8a7a8b8 ebcfe02`, `ab-20261124-015157`: I11's base against I11, the
  fixed scenario, 3 rounds of 10 s): HTTP/1.1 825k against 824k requests a second (-0.1%), h2c
  5979k against 5927k (-0.9%), CPU per request within 3%, p99 the same: within noise, as expected for one
  route with no host, which is a few nanoseconds of a request's microseconds. The first attempt
  (`ab-20261124-010304`) failed because the benchmark host's disk was full; a second found that
  `bench/ab.sh` could build B with A's crates (git archive's commit-time mtimes and a shared
  target directory), fixed in `bench/lib.sh` by extracting with the current time.
- To measure, in the maintainer's benchmark tasks (rule 30: feature tasks run no benchmarks): the
  router lookup, v0 against v1 (`cargo bench -p akit-router --bench lookup`), and the fixed-scenario
  A/B of I11 against its base. The numbers above were measured before rule 30; treat them as
  provisional until those tasks repeat them.
- Checked: `make check` on macOS; `cargo test -p akit-router -p akit-runtime -p akit-listener` on
  Linux (epoll) and `AKIO_TEST_BACKEND=uring cargo test -p akit-listener`, on the benchmark host.
- Differs from the plan and from fit: no matchers (I39). A malformed escape (`%zz`) is answered
  `400 bad request path`; fit's Go server refuses it, before fit sees it, with its own 400. akio's
  request target cannot be rewritten, so the canonical path reaches handlers as the context local
  `akit_listener::CANONICAL_PATH`, set only when it differs from the path sent; I12 and I15 read it
  (or akio gains a way to rewrite the target). A literal `\` stays as sent in the canonical path,
  where Go writes `%5C`. A request in absolute form (`GET http://host/path`) is still routed by
  akio's `path()`, the whole URI, as before I11.

### 2026-11-24: I09, TLS entry points from files

- `akit-config` reads a listener's `tls` and the `h2` protocol, with fit's fields, defaults and
  errors: `tls.certificates[]` (`cert_file`, `key_file`, both required), `tls.client_auth`
  (`ca_files`, required, at least one; `mode`, `require` by default or `verify_if_given`) and
  `tls.max_version` (`"1.3"`, quoted or bare). A listener with `tls` and no `protocols` serves
  fit's `DefaultTLSProtocols`, `http/1.1` and `h2`. fit's checks: `h2` needs `tls`, `h2c` is
  refused on a listener with `tls`, `mode` is one of its two values. Refused as not supported yet:
  `tls.acme`, `tls.on_demand`, `tls.kernel_tls_offload`, `max_version` `1.2`, and a `tls` without
  certificates (fit obtains them with ACME for the routes' host names).
- `akit-tls` (`server_config`) loads a listener's `tls` into an akio-tls `ServerConfig`, as fit's
  `tlsmgr.LoadStatic` and `LoadClientAuth` do: PEM files of at most 1 MiB, each key checked
  against its certificate, each problem at `listeners[i].tls.certificates[j]` or
  `.client_auth.ca_files[j]` with the file's path. Certificates are picked by SNI by fit's rule:
  the first certificate whose subjectAltName names (DNS, IP) hold the name, exactly or by its
  first certificate's `*.` wildcard, else the first. ALPN from the protocols as fit's
  `tlsmgr.ServerConfig`: `h2, http/1.1`, `h2` or `http/1.1`. Client authentication with
  `client_auth_required` or `client_auth_optional`. Session tickets on, as Go's servers have them.
- Versions and cipher suites need no setting: akio-tls serves TLS 1.2 and 1.3, and for 1.2 only
  the six ECDHE AEAD suites fit allows (AES-GCM and ChaCha20-Poly1305).
- The graph (`akit_runtime::compile`) loads every TLS listener's files, as fit's graph does, so
  `akit validate` checks them too; `Graph::tls(i)` gives the configuration. `akit_listener::engine`
  wraps a TLS listener's HTTP acceptor in `akio::tls::Tls::server`; akio-http's server takes HTTP/2
  when ALPN chose h2. An `h2`-only listener refuses a client offering only `http/1.1` in the
  handshake and serves HTTP/1.1 to one offering no ALPN, as Go's server does.
- Bench: `bench/configs/akit/fit.json` serves HTTPS/1.1 on 8443 and h2 on 8445 with fitbench's
  certificate at `/certs`, as fit's; `bench/fitbench/akit.go` mounts it and holds idle TLS
  connections; `run.sh` and `ab.sh` run `https/1.1` and `h2` by default. Not benchmarked.
- akio: `akio-x509` (the names in a certificate) is a new workspace dependency at the pinned
  commit, `akio-crypto` a test one; akio-tls is used through `akio::tls`. New request A9: a public
  way to cap a TLS server at 1.2 ([akio API requests](akio-api-requests.md)); A2 notes that
  wildcard certificates past the first wait for its resolver.
- The compatibility gate marks the eight `listeners[].tls` fields akit reads (`supported.txt`, 28
  fields); its report is unchanged (the fixtures that use `tls` are `.conf` and YAML files).
- Tests: `akit-config` (3: reading, fit's problems with their positions, the fields not supported
  yet); `akit-tls` (3 unit: SNI picks, ALPN, IP names; 3 on files: loading, each problem's path, a
  wildcard past the first certificate); `akit-listener/tests/tls.rs` (5: HTTPS/1.1 and h2 by ALPN
  and with no ALPN, ALPN following the protocols, SNI picking the certificate, client
  authentication required and if given, a client cut mid-handshake and a client speaking plain
  HTTP not disturbing others); `akit/tests/https.rs` (2, end to end with `akit run`: a route over
  HTTPS with HTTP/1.1, HTTP/2 and by SNI; `validate` and `run` refusing a missing certificate
  file with its path). Certificates are made by the tests with `akio-x509`.
- Where it differs from the plan: `max_version` `1.2` waits for akio (A9); a wildcard name in a
  certificate other than the first is refused (akio-tls looks names up exactly, A2); when two
  certificates hold the same name akit serves the first, where fit serves the first the client
  supports (RSA or ECDSA); the handshake timeout is akio's 10 s default, not tested at its limit
  here (akio tests it).
- Checked: `make check` on macOS; the touched crates (akit-config, akit-tls, akit-runtime, akit-listener, akit, akit-compat) on Linux (gpu51), epoll and io_uring, all passing.

### 2026-11-24: I24, the YAML reader

- [Decision 005](decisions/005-yaml-reader.md): YAML is read by a reader of akit's own, not a
  crate. serde_yaml and its forks parse with unsafe-libyaml (`unsafe` throughout) and keep no
  positions; yaml-rust2 and saphyr are pure Rust but bring crates of their own and read YAML's
  corners differently from fit's libyaml-based library, so matching fit meant writing most of the
  work over them anyway.
- `akit-config`'s `yaml` module (`crates/akit-config/src/yaml.rs`, about 1,700 lines of code, no
  dependency, no `unsafe`) parses a YAML stream into a tree of YAML nodes, anchors shared, then
  turns it into the JSON reader's `Node` tree with byte offsets, as fit's `adapter/yaml` turns it
  into JSON: block and flow collections, plain, quoted, literal and folded scalars, explicit keys,
  anchors, aliases and merge keys (`<<`), the core schema's tags, `%YAML`, `---` and `...`;
  scalars resolved as fit's library resolves them and numbers written as fit's adapter writes
  them; a duplicate key, a non-scalar key, an unknown tag, `.inf`/`.nan`, a second document and
  more than 2^20 nodes or 64 MiB through aliases refused, as fit refuses them; an empty file an
  empty config. The model's errors point at the YAML's line and column.
- `akit_config::Format` (`Json`, `Yaml`) on `Source`, `Format::of(path)`, `from_yaml`;
  `from_files` reads `.yaml` and `.yml` files with it, so YAML and JSON files merge in one config.
  `akit run -c` and `akit validate -c` take YAML; the usage text says so.
- The compatibility gate's hook reads YAML fixtures through `from_files`. `make compat-bless`:
  80 fixtures, 0 valid, 4 refused only as not supported yet (fields refused: 2), 75 in a format
  not supported yet (.conf, I23), 1 skipped, 0 failed. The two YAML templates went from "format
  not supported yet" to exactly their JSON twins' outcomes (`upstreams`, and
  `routes[].handler<proxy>` for `conf.d`).
- Tests: `tests/yaml_config.rs` (the templates, file and `conf.d`, read into the same model with
  the same problems as their JSON twins; one config in both formats; positions; YAML and JSON
  files merged), and the module's own: fit's `adapter/yaml` test cases ported, scalars, folding,
  structure, positions, Go's float format, linear time on long lines, every prefix and one-byte
  edit of a document, every template's tree against its JSON twin's, with the commented-out
  fields written out too (6 of the 10 templates write out to a valid config in both formats), and
  fit's verdicts on 493 documents (`crates/akit-config/testdata/yaml-fit.txt`).
- `tools/yaml-diff/run.sh`: the differential check against fit (fit's adapter built with Go from
  a fit checkout, a corpus of hand-written cases, fit's YAML files and docs, the written-out
  templates and random edits). It found about 25 kinds of difference, fixed one by one; the last
  run, 150,000 cases, reads 2 differently (libyaml oddities, refused by akit, in decision 005).
- Fuzz target `config_yaml` (`fuzz/`), seeded with two small configs; a 10-minute run found
  nothing.
- Differences from the plan: the reader covers everything fit's library reads rather than a
  subset, refusing only `%TAG` and named tag handles (as not supported yet), nesting deeper than
  128 (fit: 256; the JSON reader's bound) and the 2 corners above; an explicit empty document (`---`) is `{}` to akit, `null` in fit's JSON, an empty config
  to both. The plan's "Touches" also gained `tests/compat` (the hook) and `tools/yaml-diff`.
- Checked: `make check` on macOS.

### 2026-11-24: I23, the .conf reader

- fit's own format, `.conf`, read by akit-config (`crates/akit-config/src/conf`):
  `akit_config::from_conf`, and `from_files` reads a file ending in `.conf`, given or found in a
  directory, merged with JSON files as fit merges them. The lexer and parser are fit's
  (`adapter/fitconf`): comments, double quotes with their escapes, backquotes, `;`, `{}`,
  `{$NAME}` and `{$NAME:default}`, bounded at 2^20 words and 64 levels of blocks. Then `def` and
  `use`, `site` (and `site *`) expanded into routes, the top-level lists in the singular with the
  name as the argument, the shorthands (a listener's address and protocols, an upstream's
  addresses, `tls <cert> <key>`, numbers of bytes in units), modules as their name and a block of
  params, and every mistake fit reports while converting, with its message, at its line and
  column.
- How: the reader converts a `.conf` file into the same tree the JSON reader parses, as fit
  converts it into JSON, each node keeping its offset in the `.conf` text, and the one decoder
  reads both. A reader of its own feeding the decoder directly was the other choice; converting
  keeps one decoder, so a field akit supports in a later iteration (I09, I14) is read from `.conf`
  with no change here, and the decoder's problems point into the `.conf` file. Like fit's, the
  conversion is typed by the config's fields: their types come from fit's field inventory,
  `tests/compat/fit-fields.txt` (fit's JSON Schema at the pinned commit), built into akit-config,
  so every field fit has converts as fit converts it, whether akit reads it yet or not, and is
  then read or refused by name. A directive no field takes is converted as written and refused
  as an unknown field, as in fit.
- Not supported yet, and refused as such at their place: `include` and `import`, which read
  other files from a `.conf` file (no fixture uses them; files are named with `-c` instead).
  fit's old, renamed field names are not read, in `.conf` as in JSON.
- The compatibility gate reads `.conf` files and `conf.d` directories through `from_files`. What
  validation opens for the examples is settled: nothing but the config's own files, since every
  field naming a file or a secret is refused as not supported yet ([compatibility](compatibility.md#what-validation-opens)).
  A value refused of a field akit reads is now reported with the value
  (`listeners[].protocols: the "h3" protocol`, `listeners: a config with no listeners, ...
  (auto_https)`) and is not counted as the field refused: the gate's check that no supported
  field is refused had flagged `listeners[].protocols` and `listeners`, which akit reads.
- The gate's counts, at fit `7daee573` and fit-examples `9659d7e4`: 80 fixtures; 2 valid
  (`hello`, `virtual-hosts`); 75 refused only as not supported yet (52 fields or values refused);
  2 in a format not supported yet (the YAML template and its `conf.d`, I24); 1 skipped (`https`);
  0 failed. Before: 0 valid, 2 refused, 77 not read. `templates/conf/*` refuse exactly what their
  JSON twins refuse.
- Tests: `conf/tests.rs` (fit's conversion, shorthand and error tables, ported: 21 configs and 60
  errors with their line and column; the bounds; positions of the decoder's problems in the
  `.conf` file; a `.conf` config and its JSON read into the same `Config`; every `.conf` template
  converts into the same tree as its JSON twin; every example converts), `conf/convert.rs`
  (numbers, bytes, the inventory's types), `files.rs` (`.conf` and JSON files merged from a
  directory, problems at their place), `akit/tests/cli.rs` (`akit validate` of a `.conf` file), the gate (value refusals). Fuzz target `config_conf` with
  a corpus of examples, fuzzed for a minute in CI beside `config_json`.
- Checked: `make check` on macOS; `config_conf` fuzzed for 60 s locally.

### 2026-11-24: I08, benchmark harness and baselines

- akit is measured with fit's harness, not a copy of it (`bench/`, [bench/README.md](../bench/README.md)):
  `bench/fitbench/akit.go`, copied into fit's `bench/cmd/fitbench`, adds akit to fitbench's
  servers (fit's server list is a slice, so no file of fit's changes); `bench/configs/akit/fit.json`
  serves fit's bench routes as far as akit serves them (the fixed response on 8080, HTTP/1.1, and
  8081, h2c). `bench/FIT_REV` pins fit's harness at the commit the compatibility fixtures use.
- `bench/run.sh [REF]` stages fit's tree on the benchmark host once, adds akit's two files, builds
  fitbench, benchreport, idleconn and fit there with Go, builds akit at REF there with cargo into
  an image of its own (`akit-bench:<id>`, on `ubuntu:24.04`), and starts fitbench with nginx,
  Caddy, Traefik, fit and akit under the host's `quiet.sh` (the lock alone); `-report ID` fetches
  the results and fit's benchreport tables into `bench/results/ID`. `bench/ab.sh A B` compares two
  akit builds in the same run, rounds alternating, with fit's `abreport.py`.
- akit's runs are the fixed scenario over HTTP/1.1 and h2c, with idle HTTP/1.1 connections; its
  other scenarios are recorded as skipped until the iterations that build them, each of which adds
  them to the overlay and the config. Where akit's config differs from fit's (port 8081 also
  takes HTTP/1.1, no backlog or connection limit settings yet, akio's I/O threads per listener)
  is in the bench README.
- The baseline, `bench/results/baseline-2026-11-24` (results, fit's benchreport tables, params and
  the akit commit): akit at `7c835ad`, fit `7daee573`, nginx 1.31.4, Caddy 2.10.2 and Traefik
  3.5.0 on the benchmark host (AMD Ryzen 7 7800X3D, 8 cores, 16 threads, Linux 7.0), fitbench's
  defaults (30s runs after 5s, 256 wrk connections, h2load 64 × 32), the fixed, static and proxy
  scenarios over HTTP/1.1 and h2c, and idle HTTP/1.1 connections; no errors.
- Where akit stands (rule 28), in what it serves so far: first in fixed over HTTP/1.1, 825,563
  requests a second at p99 0.32 ms and 2 MiB, nginx 99% of it, fit 92%, Caddy 41%; first in
  fixed over h2c, 6,029,715 a second at 3 MiB, fit 74%, nginx 10%, Caddy 3%; second in memory per
  idle HTTP/1.1 connection, 1,068 bytes, after nginx's 548 (fit 1,522). Static and proxy are not
  run for akit (I12, I15); Traefik serves only the proxy.
- Checked: `make check` on macOS; `bench/run.sh` end to end on the benchmark host (the first try
  failed when the host's disk was full, before measuring). `bench/ab.sh` has not finished a
  comparison: under rule 30 (added meanwhile), feature tasks run none.

### 2026-11-23: I06, CLI, signals and graceful shutdown

- `akit run`, `akit validate`, `akit version` and `akit help` with fit's flags, read as Go's
  `flag` package reads them (`-c x`, `-c=x`, `--c`, bool flags, `--`), and fit's exit codes: 0, 1
  when it did not work (config errors, a shutdown out of time), 2 when used wrongly (a bad or
  unknown flag, an argument, no command). `validate` prints `<files>: valid`; `version` prints
  akit's version, akio's commit and the platform, a line each, as fit's.
- `-c` repeated, files and directories: `akit_config::from_files` finds the files as fit does
  (a directory's `.json`, `.conf`, `.yaml` and `.yml` files, in subdirectories too, in lexical
  order, dotfiles left out, a file named twice read once, at most 10,000 files and 64 MiB) and
  merges them as fit does: `listeners` and `routes` joined in the files' order, `schema_version`
  given by each if they agree, the checks across elements on the whole. Each problem names its
  file, line and column, with its path in that file. A `.conf` or YAML file is refused as not
  supported yet (I23, I24); any other is JSON, as in fit. `fit.json` when no `-c` is given.
- Signals through akio-sys's `signal_pipe` (its `unsafe` stays in akio): SIGINT and SIGTERM print
  `akit: shutting down`, stop the listeners, close idle connections and wait up to
  `-shutdown-timeout` (30s by default, a Go duration) for requests in flight, then print
  `akit: stopped` and exit 0, or say the shutdown took longer and exit 1. A second signal exits at
  once. SIGHUP says reloading is not supported yet (I22) instead of killing the process.
  `akit_listener::shutdown` does the drain; `akit_listener::Options` carries the backend and the
  timeout.
- fit's other `run` flags (`-watch`, `-watch-interval`, `-watch-certs`, `-admin`, `-timeout`) are
  refused as not supported yet, exit 2.
- The compatibility gate now reads a directory of JSON files through the merge:
  `templates/json/conf.d` is refused only as not supported yet (`upstreams`, the `proxy`
  handler), no longer a format akit cannot read; 2 fixtures refused only as not supported yet, 77
  in a format not supported yet.
- Tests: `akit-config/tests/files.rs` (6: joining, problems in their file, schema versions,
  directories, formats, missing and empty); flags and Go durations; `akit-listener` (2 more: a
  request in flight finishes during the shutdown while an idle connection is closed at once; one
  that outlasts the timeout is cut and reported); `akit/tests/cli.rs` (4: validate across files
  and a directory, usage errors exit 2, `run` from two files stopped by SIGTERM with an idle
  keep-alive connection, `run` from a directory stopped by SIGINT).
- Checked: `make check` on macOS; the workspace's tests on Linux, epoll and io_uring, on the
  benchmark host (2026-11-24, with I04's and I05's): all pass.
- Differs from the plan: the request in flight is tested with the library's server and a handler
  that answers later, since `respond`, the only handler, answers at once; the binary's tests cover
  the signals, the output and the exit codes. Whether the shutdown ran out of time is told by how
  long akio's drain took (akio reports no count of the connections it closed). The second signal
  exits with 1, where Go's default handler would kill the process by the signal.

### 2026-11-23: I05, `akit run`: listeners and `respond`

- `akit run -c <file>` reads the config (JSON; `fit.json` when no `-c` is given, as fit's),
  compiles it, serves it, and prints each listener's address as fit does
  (`akit: listener "web" on 127.0.0.1:8080 (http/1.1, h2c)`). A config error prints fit's
  `file:line:col: path: message` lines and exits 1; a bad flag exits 2.
- `akit-handler`: the `respond` module (`http.handlers.respond`), its response built once at
  provisioning (status, headers, a `text/plain; charset=utf-8` content type when there is a body and
  none is set); akit-config already checks its params as fit's `Provision` does.
- `akit-listener`: `engine(cell, backend)` builds one akio TCP listener per config listener, each
  serving a `Dispatcher` that routes the request through the cell's current graph and calls the
  route's handler, or answers fit's `404 page not found`. `":8080"` listens on `[::]` dual-stack,
  as Go's `net.Listen`. `http/1.1` alone turns akio's HTTP/2 off; with `h2c`, prior knowledge and
  the `Upgrade: h2c` request are both served.
- `akit-router`: router v0, fit's host (exact, `*.`, `**.`, ignoring case, port and trailing dot)
  and path (exact, `*` prefix) rules and its order (priority, host, path, then the order written),
  by a scan of the routes; the graph builds one per listener at compile time, a route with no
  `listeners` serving on all.
- Fixed: akit-config's default listener protocols, found by I07b, are now fit's, `http/1.1`
  alone without TLS (`DEFAULT_PROTOCOLS`), with a test.
- Tests: router (6), respond (3), listener addresses; `akit-listener/tests/listeners.rs` (5):
  HTTP/1.1, h2c prior knowledge and upgrade, 404, host specificity, routes per listener, HTTP/2
  refused by default, h2c alone refused; `akit/tests/hello.rs` (3): fit's `hello` example, as
  JSON, answers `/hello` and its `not-found` route over HTTP/1.1 and h2c through the binary; a
  bad config exits 1 with its errors, bad flags exit 2.
- Checked: `make check` on macOS; the workspace's tests on Linux, epoll and io_uring, on the
  benchmark host (2026-11-24, in I06's run): all pass.
- Differs from the plan: the router v0 was not in I05's plan; without it the example's
  `not-found` route (priority -1, no match) would shadow `/hello`. I11 keeps its rules and makes
  lookups allocation-free, and now also canonicalizes paths as fit's listener does (not planned
  anywhere before). The example is written as JSON in the test: `.conf` is I23. A listener with
  `h2c` and no `http/1.1` is refused as not supported yet: fit serves HTTP/2 alone then, and akio's
  server always takes HTTP/1. Request bodies are buffered (akio's default, 1 MiB) until a handler
  streams them (I14).

### 2026-11-23: I04, the runtime graph and its swap

- `akit_runtime::compile(config, registry, env) -> Graph`: every route's handler loaded from the
  registry and provisioned before the graph exists. A config that fails leaves nothing behind: the
  modules provisioned before the failure are cleaned up, and errors carry the path of what failed
  (`routes[1].handler.params.status` for a provision error naming a field).
- `GraphCell` ([decision 004](decisions/004-graph-publication-and-retirement.md)): `swap` publishes
  a graph under a lock and bumps an epoch; `with` reads it through a per-thread cached `Arc`,
  refreshed once per thread per swap, so a request bumps no shared reference count; `hold` keeps a
  graph for a request or connection that outlives its handler's call; `collect` retires replaced
  graphs, cleaning their modules up once, when every thread is outside them or past them and no
  `Hold` keeps them.
- Tests (`tests/graph.rs`, 8): compile and its failures, swap and collect, holds, a request still
  inside the old graph, an idle thread that does not pin it, 8 threads swapping and reading 300
  times each with every graph retired exactly once, and swaps while 4 akio I/O threads serve.
- Benchmark `benches/graph_read.rs`: a read through `GraphCell::with` against an `Arc` clone and a
  mutex-and-clone, 20 million reads a thread on the benchmark host (16 cores): `GraphCell::with`
  4.06 ns a read on 1 thread and 8.78 ns on 16; an `Arc` clone and drop 2.77 ns and 324.51 ns; a
  mutex and a clone 6.57 ns and 1,167.62 ns. The shared reference count costs 37 times the cell's
  read once 16 threads contend for it.
- Checked: `make check` on macOS; the workspace's tests on Linux, epoll and io_uring, on the
  benchmark host (2026-11-24, in I06's run): all pass; the read benchmark there, under
  `quiet.sh` (above).
- Differs from the plan: the "atomic pointer" is a mutex taken once per thread per swap (no unsafe,
  rule 14; the epoch is the atomic). A graph is not released by its last request: akio has no hook
  on an idle I/O thread, whose cache would keep the old graph, so the control plane retires it
  with `collect` (decision 004). Handler params are `akit_config::Handler`, decoded and checked by
  akit-config. Routes stay in the config's order until I11.

### 2026-11-23: I07b, the compatibility gate wired to akit-config

- The gate is always on: the `compat-gate` feature is gone, akit-config is a dev-dependency of
  `akit-compat`, and `gate_through_akit_config` is an ordinary test that `make check` and CI's
  `cargo test --workspace` (Linux epoll and io_uring, macOS) run. One configuration, so the gate
  cannot be left out by a missing flag. `make compat-gate` runs it alone and prints the report.
- Formats akit has no reader for are an outcome of their own, "format not supported yet": the
  .conf files and `conf.d` directories until I23, the YAML template until I24. Every outcome is a
  line of the checked-in `tests/compat/gate-report.txt` (one line a field for a fixture refused
  as not supported yet), and the gate fails when a run differs from it either way;
  `make compat-bless` rewrites it from a passing run. JSON fixtures must read with only
  "not supported yet"; anything else, or "format not supported yet" beside other findings, fails.
- Outcome now, at fit `7daee573` and fit-examples `9659d7e4`: 80 fixtures; 0 valid; 1 refused
  only as not supported yet (`templates/json/fit.json`: `upstreams`; the rest of the template is
  commented out); 78 in a format not supported yet (74 examples and `templates/conf/fit.conf`,
  .conf; `templates/yaml/fit.yaml`, YAML; three `conf.d` directories); 1 skipped
  (`examples/https`); 0 failed.
- The hook names akit-config's problems by their path, written as the inventory writes it: a
  handler module akit lacks, refused at `routes[0].handler.module`, is `routes[].handler<proxy>`;
  a refused value of an array of values (`listeners[0].protocols[0]`, `h3`) is the array.
  `akit_compat::inventory_path` does the matching.
- `tests/compat/supported.txt` marks the 20 fields I02 reads: `schema_version`; `listeners` and
  their `name`, `address`, `protocols` (`http/1.1` and `h2c`; fit's other protocols are refused);
  `routes` and their `name`, `listeners`, `priority`, `match`, `match.host`, `match.path`,
  `handler`, `handler.module`, `handler.params`; `handler<respond>`, its `params` and their
  `status`, `headers`, `body`. `docs/compatibility.md` regenerated: 20 of 3,821 supported.
- New tests hold the marks to the code: `supported_fields_are_read` reads a config with each
  marked field and checks the value; `akit_config_lists_match_the_inventory` reads akit-config's
  lists in `model.rs` from the source and checks every entry is an inventory field, that they
  cover each object's fields in fit, that the refused handlers and protocols are fit's, and that
  what akit-config reads is exactly `supported.txt`. All of them pass: no typo in I02's lists.
- No change to akit-config: its lists are read from the source text, so no accessor was needed.
- Differs from the plan: what validation opens for the examples (files and WebAssembly plugins
  they name, the stand-in secrets) moves to I23, the first iteration that reads them; the gate
  does not need the secrets until akit expands variables.
- Found, not fixed (akit-config is I02's and I04's): akit-config's default listener protocols are
  `http/1.1` and `h2c` (`DEFAULT_PROTOCOLS`), while fit's are `http/1.1` alone without TLS
  (fit `config/config.go`, the `protocols` field's doc). Neither the gate nor the inventory can see
  a default; it needs a fix and a test in akit-config.

### 2026-11-23: I07a, compatibility gate I: fixtures, inventory, harness

- I07 is split: I07a, done here, needs no config model; I07b wires the gate to
  `akit_config::from_json_file` once I02 is merged ([dev plan](dev-plan.md#i07b-compatibility-gate-wired-to-validate)).
- Pinned fixtures in `tests/compat/fixtures/`: fit `7daee573` (`templates/`, 30 files: each of
  the three formats as one file and as a `conf.d` directory of 9 files; `schema/fit.schema.json`)
  and fit-examples `9659d7e4` (74 `*/fit.conf`), with both Apache-2.0 licences,
  `tests/compat/SOURCE.md` and `tests/compat/update-fixtures.sh <fit> <fit-examples>`, which
  moves the pin and regenerates everything below.
- The field inventory, `tests/compat/fit-fields.txt`: 3,821 field paths (path, section, type,
  flags), generated by `tools/compat-inventory.py` (Python 3, standard library, a dev tool) from
  fit's JSON Schema, which fit generates from its config types and modules as it does its config
  reference. Module params are listed per module (`routes[].handler<respond>.params.status`),
  121 sections in all.
- The harness, the test crate `akit-compat` (`tests/compat`, a workspace member, no
  dependency): enumerates the 80 fixtures (6 templates, 74 examples), runs a validator on each
  and judges it as decision 002 says; `examples/https` is skipped as fit-examples' own test skips
  it. Tested with stand-in validators. The gate itself (`gate_through_akit_config`) is behind the
  `compat-gate` feature, which adds the akit-config dependency, and is `#[ignore]`d until I07b;
  `make compat-gate` runs it with fit-examples' stand-in secrets.
- `docs/compatibility.md`: what the gate checks, how to read and refresh it, and the generated
  table of every field, all "not supported yet". `tests/compat/supported.txt` (empty) is where
  later iterations mark fields supported. `make check` and CI's lint job run
  `make compat-check`, which fails when the inventory or the tables are out of date.
- Checked: `make check` on macOS. No engine code, so nothing to run on Linux backends.
- Differs from the plan: I07 split into I07a and I07b; the inventory comes from fit's JSON Schema
  rather than from parsing `docs/reference/config.md` (same source in fit, machine-readable).
  The examples' non-config files (certificates, WebAssembly plugin sources) are not copied;
  I07b decides what validation opens.

### 2026-11-23: I03, core traits and the module registry

- `akit_runtime::traits`, on akio's callback model: `Handler` (an akio-http `Service`: inline on the
  I/O thread, responds or detaches), `HandlerService` (a handler as akio's `Service`), `Middleware`
  (`wrap(next)` once at compile time, as fit's) with `chain` and `AkioMiddleware` (akio's
  per-request middleware as akit's), `Matcher`, `Router`, `Service` (fit's: picks an `Upstream`),
  `Upstream` and `Listener` (a name and an akio `AnyAcceptor`); the last three provisional, as in fit.
- `akit_runtime::module`: `Info` (ID, doc, typed `Constructor`), `Registry::new` from crates'
  `static` lists (no global state; malformed, duplicate or misplaced IDs are a `RegistryError`),
  `lookup`, `namespace`, `handler` and `middleware` loading by config name with `Provision` (params
  of a generic type `P`, `Env`); `Provisioned::on_cleanup`; `Modules` running cleanups last
  provisioned first, once, on `release` or drop. Errors keep fit's wording: `there is no module
  "nope" in http.handlers (it has "echo", "respond")`; `ProvisionError` carries a message and maybe
  a field for I04 to place at the params' path.
- [Decision 003](decisions/003-module-registry-and-traits.md) records the design later iterations
  follow.
- Checked: `make check` on macOS (fmt, clippy `-D warnings`, tests, cargo-deny, docs);
  `RUSTDOCFLAGS=-D warnings cargo doc -p akit-runtime`. Tests (`crates/akit-runtime/tests/modules.rs`):
  a test module registers, provisions from params and is cleaned up on release and on drop; unknown
  names are errors naming them; a failure midway (a constructor error or an unknown name) cleans up
  what was provisioned, in reverse; registry mistakes; and a provisioned middleware + handler chain
  answers a real request on akio (epoll/kqueue; `AKIO_TEST_BACKEND=uring` on Linux). Not run on
  Linux in this session. No benchmark (control plane).
- Differs from the plan: akit-runtime now depends on `akio-http` (same akio `rev`) for `Service`,
  `Context` and `Middleware`. The params type is generic, not akit-config's, since I02 was being
  written in parallel: I04 sets it. `Env` is empty until I04. fit's optional interfaces (`Drainer`,
  `Inline`, `Unwrapped`, `NeedsAuthentication`, `Holder`) are left to the iterations that need them.

### 2026-11-23: I02, config model v0 and the strict JSON reader

- `akit-config`: a JSON reader of akit's own (decision 001) that keeps each value's position:
  RFC 8259 plus `//` and `/* */` comments and trailing commas after a value, as fit reads JSON;
  a key given twice, a control character, a lone surrogate, a leading zero, nesting past
  `MAX_DEPTH` (128) and documents past `MAX_CONFIG_BYTES` (64 MiB) are refused.
- The model v0, with fit's names, defaults and checks: `schema_version` (1); `listeners` (`name`,
  `address` as Go's `SplitHostPort` takes it, `protocols` `http/1.1` and `h2c`, fit's default
  both); `routes` (`name`, `listeners` that must exist, `priority`, `match.host` and `match.path`
  with fit's checks, `handler`); the `respond` handler (`status` 100–599, informational refused,
  `headers` as tokens without line breaks, no body on 204 and 304). fit's names: letters, digits,
  `_ . -`, unique per kind.
- Problems are collected, not stopped at, each as fit writes it: `file:line:col: path: message`,
  with fit's messages (`unknown field (did you mean …?)`, `missing required field`, `expected an
  array, got an object`, `another listener is already named …`).
- Decision 002 in code: every field and value fit reads that akit does not yet (the other top-level
  sections, `tls`, `middleware`, the other matchers, protocols and handler modules) is
  `not supported yet` by name; anything else is an unknown field or module.
  `Errors::only_not_supported_yet` tells a valid fit config akit cannot run yet from a wrong one.
- Checked: unit tests for the reader (values, comments, every refusal, depth, positions, every
  prefix and single-byte change of a config), the model's checks, and integration tests with
  fit-examples' `hello`, defaults, every problem in one document, file errors; fit's full
  1 MB JSON template (`templates/json/fit.json`, read from `../fit` when present) reads with
  nothing but "not supported yet". `make check` passes. Fuzzed with cargo-fuzz (`fuzz/`,
  target `config_json`): 3.4 million runs in 60 s, no crash; CI runs it for 60 s on every push.
- Differs from the plan: `upstreams`, `admin` and `log` are refused as "not supported yet" rather
  than modelled, since nothing would act on them until I14, I26 and I21 and decision 002 forbids
  reading a field and ignoring it; they come with those iterations.

### 2026-11-23: I01, workspace skeleton and CI

- The Cargo workspace: the `akit` binary and the fifteen library crates of the
  [layout](dev-plan.md#workspace-layout), empty but documented; edition 2024 and MSRV 1.85 as
  akio's; Apache-2.0. akio is pinned by `rev` (`50a82315`, akio's `main`) in
  `[workspace.dependencies]`, used by `akit-runtime`, whose `AKIO_REV` the binary prints.
- No `unsafe`: `unsafe_code = "forbid"` in the workspace lints and `#![forbid(unsafe_code)]` in every
  crate. Clippy lints for rule 15 (`unwrap_used` per library, `expect_used`, `panic`,
  `indexing_slicing`, `arithmetic_side_effects`), with tests allowed to unwrap (`clippy.toml`).
- `deny.toml`: Apache-2.0 and MIT only for now, unknown registries and git sources refused except
  akio's, and tokio, async-std and smol banned (rule 16).
- `Makefile`: `check` (fmt, clippy, tests, cargo-deny, docs), `test-changed`
  (`scripts/test-changed.sh`: changed crates and their dependents), `build`, `lint`.
  `scripts/check-docs.sh` checks that every document has its Chinese twin and that both link each
  other under the title.
- CI (`.github/workflows/ci.yml`) on pushes to `main` and pull requests: fmt, clippy and the docs
  check; tests on Linux (epoll, and `AKIO_TEST_BACKEND=uring`) and macOS; `cargo +1.85 check`;
  cargo-deny.
- The binary: `akit version` / `--version` prints `akit 0.0.1 (akio 50a823155a06)`; `help`; an
  unknown command exits 2. `run` and `validate` come with I05 and I06.
- Checked: `make check` on macOS; in Docker on Linux arm64, `cargo test --workspace` (plain and with
  `AKIO_TEST_BACKEND=uring`) and `cargo check` with Rust 1.85. The CI workflow itself first runs when
  this branch is pushed. The engine is not exercised yet, so the backends add nothing to test today.
- Differs from the plan: no `xtask` (the `Makefile` and two scripts are enough); `tests/e2e` and
  `bench` are created by the iterations that fill them (I05, I08).

### 2026-11-23: decisions

- The user accepted [decision 001](decisions/001-dependency-policy.md) (the data plane depends on
  akio and `std` only, no tokio) and [decision 002](decisions/002-config-compatibility.md) (akit
  reads fit's config); akit is Apache-2.0, as fit; A1–A8 are added to akio now.

### 2026-11-23: planning

- Wrote the roadmap, development plan, rules, architecture, akio API requests and decisions 001
  and 002, in English and Chinese.
- Not checked: the akio API requests were found by reading akio's public API, not by building
  against it; each is to be confirmed before akio is changed.
