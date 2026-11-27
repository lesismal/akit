# Compatibility

[English](compatibility.md) | [简体中文](compatibility.zh-CN.md)

From 1.0, akit keeps what it promises here: a config that loads keeps loading, and a client of the
Admin API keeps working, across every 1.x release. akit's v1 is fit's v1: the same config schema,
version 1, and the same Admin API, v1, for what akit serves of them
([fit's policy](https://github.com/lesismal/fit/blob/main/docs/compatibility.md)), with akit's
names where fit's say "fit" ([decision 037](decisions/037-akit-names.md)). What changes in a way
that could break them waits for a new version of the config schema or of the Admin API, and comes
with a way over.

akit reads fit's config ([decision 002](decisions/002-config-compatibility.md)): the same fields,
defaults and meanings, in the same three formats; nine defaults that name the gateway are akit's
(`akit` for fit's `fit`, [decision 037](decisions/037-akit-names.md)), and the
[migration guide](guides/migrating-from-fit.md#akits-names) lists them. A field akit does not support yet is refused by
name, "not supported yet: `<path>`", never read and ignored. The second half of this page says how
that is checked against fit's templates and examples, and lists every field of fit's config with
whether akit supports it. The list shrinks milestone by milestone.

- [v1: what stays stable](#v1-what-stays-stable)
  - [What is covered](#what-is-covered)
  - [What a 1.x release may do](#what-a-1x-release-may-do)
  - [The schema's version](#the-schemas-version)
  - [Renamed fields and deprecations](#renamed-fields-and-deprecations)
  - [The Admin API, v1](#the-admin-api-v1)
  - [A new version](#a-new-version)
  - [How the freeze is checked](#how-the-freeze-is-checked)
- [The gate](#the-gate)
- [Reading a result](#reading-a-result)
- [Refreshing the fixtures](#refreshing-the-fixtures)
- [Supporting a field](#supporting-a-field)
- [akit's extensions](#akits-extensions)
- [The fields](#the-fields)

## v1: what stays stable

### What is covered

| Surface | Version | What it is |
| --- | --- | --- |
| The config | schema version 1 | Every field of fit's schema version 1 (the pinned [`fit.schema.json`](../tests/compat/fixtures/fit/schema/fit.schema.json)), the handlers' and middleware's params included: its name, its type, what it means, and its default; and akit's [extensions](#akits-extensions). A field akit reads stays read; one it refuses as not supported yet may become read. `.conf`, YAML and JSON are the same config written three ways. |
| The Admin API | v1 | Its paths and methods, the bodies they take and give, their status codes, the header fields `ETag`, `If-Match`, `If-None-Match`, `Akit-Config-Version` and `Akit-Dry-Run` (fit's `Fit-Config-Version` and `Fit-Dry-Run`, renamed by decision 037), the bearer token, and `?dry_run` ([below](#the-admin-api-v1)). |
| The command line | 1 | The commands `run`, `validate`, `adapt`, `fmt`, `build` and `version`, their flags, and their exit codes, as fit's. |
| Metrics | 1 | The names, types and labels of the metrics `/metrics` serves and OTLP exports: fit's, each renamed from `fit_` to `akit_` (decision 037). |
| The access log | 1 | The fields of its JSON lines, fit's. |

Not covered:

- What akit writes for people rather than programs: messages, warnings, the dashboard,
  `akit adapt`'s output beyond its being a valid config, and the text of errors (their JSON shape
  is covered).
- The host functions of WASM plugins (the `akit` import module, fit's `fit` renamed by decision 037) and fit's subset of Proxy-Wasm,
  which are experimental in fit's 1.0 too; the `wasm` middleware's config is covered.
- The nginx and Caddyfile converters' coverage, which only grows.
- The Rust API of akit's crates, which modules built with `akit build` use: in 1.0 it may change in
  any release (fit covers its Go `module` packages; akit does not yet).
- Performance.

### What a 1.x release may do

- Add config fields, modules, providers and enum values; a field added is optional, and leaving it
  out keeps the behavior of the release before. Read a field it refused as not supported yet.
- Add Admin API paths, optional request fields, response fields and response fields' values. A
  client should ignore fields it does not know.
- Add metrics, and labels to metrics that had none of that name.
- Add commands and flags.
- Tighten a default limit, or change a default, only to close a security hole or a resource that
  was unbounded; the release notes say so.
- Rename a field ([below](#renamed-fields-and-deprecations)).
- Fix behavior that contradicts the docs, fit's behavior, or an RFC akit claims to follow.

A config that a 1.x release refuses, because it was wrong before and went unnoticed (two fields
that cannot go together, a value out of range), is a bug fix; the release notes list each.

### The schema's version

A config may say which version of the schema it is written for:

```json
{"schema_version": 1, "listeners": [{"name": "web", "address": ":8080"}], "routes": [{"handler": {"module": "respond"}}]}
```

It is optional, and a config that leaves it out is read as version 1. akit reads version 1 and
refuses a config that says another (`schema_version: 2 is greater than the maximum 1`) rather than
read it wrongly, as fit does.

### Renamed fields and deprecations

A field renamed within a version keeps its old name until the next one. A config that uses the old
name loads as before, and akit says where, in fit's words:

```text
fit.json:12:40: routes[0].middleware[0].params.max_size: deprecated: renamed "max_bytes", which takes the same value; the old name goes in config version 2
```

`akit validate` and `akit run` print it, at the old name's line and column in the file (`.conf`
and YAML too), both at start and on each reload, and the Admin API returns it in the `warnings` of a
change's answer. fit's schema keeps the old name, marked `deprecated`. A config may not give both
names (`"max_size" is the old name of "max_bytes"; give only "max_bytes"`).

These were renamed for fit's 1.0, so that every size in bytes ends in `_bytes` and every period in
`_interval`, and akit reads both names of each:

| Module | Old name | Name |
| --- | --- | --- |
| `cache` | `max_size` | `max_bytes` |
| `cache` | `max_entry_size` | `max_entry_bytes` |
| `cache` | `disk.max_size` | `disk.max_bytes` |
| `compress` | `min_length` | `min_bytes` |
| `jwt` | `jwks_refresh` | `jwks_refresh_interval` |
| `wasm` | `max_memory` | `max_memory_bytes` |

fit's seventh, the tunnel's `quic_pin` (now `pin`), belongs to a part akit does not run yet.

A deprecation is how anything leaves v1: the field, value or endpoint stays and works as before,
warned of where a warning can be given, for the rest of the major version; it goes only in the next
version of the schema or the API, which says so in its list of changes.

### The Admin API, v1

| Method and path | What it does |
| --- | --- |
| `GET`, `HEAD` `/config`, `/config/{path...}` | The running config as it was written, or the part at a path, its inline secrets answered as `[redacted]` |
| `PUT`, `PATCH`, `DELETE` on `/config` and `/config/{path...}` | Replace, merge patch (RFC 7396) or delete the config or the part at a path, and apply the result |
| `POST /load` | Apply a whole config |
| `GET`, `HEAD` `/config/effective` | The config merged with what providers provide, and where each part came from |
| `GET`, `HEAD` `/versions`, `/versions/{n}` | The configs applied and kept, and one of them |
| `POST /rollback/{n}` | Apply a kept config again, as a new version |
| `POST /reload` | Apply the running config again, as a new version |
| `GET`, `HEAD` `/status` | The listeners, connections, upstreams and their health, and certificates |
| `GET`, `HEAD` `/metrics` | Metrics in Prometheus's text format |
| `GET`, `HEAD` `/cache`, `POST /cache/purge` | The cache's zones, and purges by URL, URL prefix, tag or zone |
| `GET`, `HEAD` `/logs`, `POST /logs/rotate`, `POST /logs/cleanup` | The log files, one rotated now, and its retention applied or listed (added after the freeze, I83) |
| `POST /config/render`, `POST /config/parse`, `PUT /config/text`, `POST /config/export`, `POST /config/import` | The config, or some of its top-level members (`?members=`), as `.conf`, YAML or JSON text (`?format=`), and a text or file applied, as fit's console's raw editors and transfer page call them (added after the freeze, I91; fit's `store` format, a `fitstore` file sealed with a password of its own or the running store's key, since I93b) |
| `GET`, `HEAD` `/tokens` and `/tokens/{member}`, `POST /tokens/{member}`, `PATCH` and `DELETE /tokens/{member}/{id}` | The link tokens of the tunnel's hub and the LLM roles, issued, listed, their expiry changed and revoked, as fit's `admin/tokens.go`; akit's work on the member the running config sets, whether or not its role runs yet (added after the freeze, I100) |
| `GET /dashboard/` | The dashboard, for people (not covered) |

A path in the config is a JSON pointer's tokens. A change answers with the version applied, in the
body (`version`) and in `Akit-Config-Version`, and with the config's `ETag`; a change sent with
`If-Match` is applied only to the config that ETag names, and is answered 412 otherwise; one sent
with `?dry_run=1` is checked, not applied, and answered with `Akit-Dry-Run: 1`. A config that does
not load is answered 400 with `{"error": ..., "errors": [{"path", "line", "column", "message"}]}`.
These are fit's, with fit's header fields `Fit-Config-Version` and `Fit-Dry-Run` renamed
([decision 037](decisions/037-akit-names.md)): a client of fit's that reads or sends those takes
akit's names; fit's are not accepted.

Since I80b, a secret written inline in the config (under a member named as fit's console names
secrets: `password_hash`, `secret`, `token`, an `Authorization` header) is answered as
`[redacted]` by `/config`, `/versions/{n}` and `/config/effective`, as fit's console shows it, and a
change that sends `[redacted]` back keeps the secret in its place
([decision 035](decisions/035-admin-api-redacts-secrets.md)). fit's own Admin API answers the
config as written; the change closes the threat model's L2, as a 1.x release may to close a
security hole.

fit's v1 has more endpoints, for the parts akit does not run yet: the link tokens of the tunnel's
and the LLM roles (`/tokens`), the media libraries and the workbench's media pages (`/media/...`)
and the streaming role (`/streaming/...`). akit answers them 404 until it runs those parts, and adds
them then, as fit has them, a compatible addition.

Since I90, the console (`admin.console`) serves the same endpoints under `/api/m/admin/`, behind
its logins instead of the token, with fit's own console endpoints (`/api/login`, `/api/logout`,
`/api/session`, `/api/workbench`), and akit's names for its cookie and CSRF header,
`akit_console_<port>` and `X-Akit-Csrf` (fit's `fit_console_<port>` and `X-Fit-Csrf`, decision
037; [the console guide](guides/console.md)).

A v2 of the Admin API, if one is needed, would be served under `/v2/` beside v1's paths, which would
stay for the rest of akit's major version.

### A new version

A change that cannot keep old configs or clients working waits for a new schema or Admin API
version, fit's first, which comes with:

- a list of every change, and why;
- for the config, `akit adapt`, which rewrites a config of the version before into the new one,
  and the new akit reading the version before, with warnings, for at least one release;
- for the Admin API, the version before served beside the new one for the rest of the major
  version.

### How the freeze is checked

[tests/compat/v1](../tests/compat/v1) holds the v1 surface as it was frozen (I79), and what changed
since:

- `config.txt`, the config's: every field of fit's schema version 1 with its type, whether it is
  required and its default, the values of its enums, its deprecated names, the fields akit reads,
  and akit's extensions, one item a line;
- `admin-api.txt`, the Admin API's: its endpoints, header fields, query parameter, and the members
  of the answers to a change, to an error and to `GET /versions`;
- `additions.txt`: each change since, marked `added` (a compatible addition) or `retired` (an item
  a 1.x release changed under this policy, with why).

Two checks fail on any change not marked, in CI and in `make check`:

- `tools/v1-freeze.py` (CI's lint job, `make compat-check`) builds the config's surface from fit's
  pinned schema, [`supported.txt`](../tests/compat/supported.txt) and the extensions, and compares
  it with the baseline and the additions. A field read for the first time, or a field fit added when
  the pin moves, is an addition: `make v1-bless` marks it, and refuses to mark anything while a
  change breaks v1 (a field gone, its type, default or requiredness changed, a required field added
  to an object v1 has, a field akit read refused again).
- `v1_surface_is_frozen` (akit-admin's tests, CI's test jobs) runs the Admin API and probes it:
  every endpoint frozen is served, no other method of a frozen path is, the paths the dispatcher
  names are frozen, and the header fields and the answers' members are those frozen. A new one is
  marked in `additions.txt` by hand, as the failing test prints it.

The baselines themselves are never edited within v1: `tools/v1-freeze.py` holds their SHA-256
sums, and a change to either is a new version. They were written again once before akit's first
release, by [decision 037](decisions/037-akit-names.md): the config's by
`python3 tools/v1-freeze.py --write-baseline` (the surface less the additions marked, with akit's
nine defaults that rename fit's), the Admin API's by hand (its two header fields renamed).

## The gate

The compatibility gate is the `akit-compat` test crate in [tests/compat](../tests/compat). It
takes, pinned at a fit commit and a fit-examples commit ([where they come from](../tests/compat/SOURCE.md)):

- every template in fit's `templates/`: `.conf`, YAML and JSON, each as one file and as a
  `conf.d` directory of files by kind (6 configs);
- every config of fit-examples, each example's `fit.conf` (74 configs at the current pin).

and runs each through akit's validation, as `akit validate -c <file or directory>` does. Each
must:

- be **valid**, or
- be refused **only** with "not supported yet: `<path>`", every path naming a field fit accepts
  (a field of the [inventory](#the-fields)).

Anything else fails the gate: a parse error, a type error, a field fit accepts refused as unknown
or under a wrong path, or a refusal that names nothing. A field read and then ignored also fails
it; the inventory is what makes that visible, since a field that is not marked supported must be
refused.

The parts, and where they stand:

| Part | Where | State |
| --- | --- | --- |
| Pinned fixtures and their licences | `tests/compat/fixtures/`, `tests/compat/SOURCE.md` | done (I07a) |
| The field inventory | `tests/compat/fit-fields.txt`, generated by `tools/compat-inventory.py` | done (I07a) |
| The harness: enumerate, judge, report | `tests/compat/src/lib.rs`, `tests/compat/tests/gate.rs` | done (I07a) |
| The gate through akit-config, its report checked in | `gate_through_akit_config` in `tests/compat/tests/gate.rs`, `tests/compat/gate-report.txt` | done (I07b) |
| The supported fields read, akit-config's lists checked against the inventory | `supported_fields_are_read`, `akit_config_lists_match_the_inventory` in the same file | done (I07b) |
| The .conf fixtures and `conf.d` directories read | akit-config's .conf reader (`crates/akit-config/src/conf`) and merge | done (I23) |
| The YAML fixtures read | akit-config's YAML reader ([decision 005](decisions/005-yaml-reader.md)) | done (I24) |

The gate is an ordinary test of the workspace, with no feature and nothing ignored: `make test`,
and so `make check`, runs it, and so does CI's `cargo test --workspace` on Linux (epoll,
io_uring) and macOS. One build of one configuration means the gate cannot be left off by
accident; the feature that held it back until I02 landed is gone. `make compat-check` also fails
when `fit-fields.txt` or the tables below are out of date.

### Formats akit cannot read yet

akit-config reads JSON, fit's own `.conf` format (I23) and YAML (I24,
[decision 005](decisions/005-yaml-reader.md)), and merges a directory's files, as fit does: every
example, every template and every `conf.d` directory goes through it, and the YAML templates read
into the same model as their JSON twins, with the same outcomes. A fixture in a format akit had no
reader for would be neither passed nor failed silently: its outcome would be "format not
supported yet", written in the gate's report like every other outcome. A fixture akit reads must
read with only "not supported yet" findings; any other error fails the gate, and so does a
"format not supported yet" beside other findings.

The `.conf` reader types each directive by its field in fit's own types, which it takes from the
inventory (`fit-fields.txt`, built into akit-config): a `.conf` file converts into the tree its
JSON twin parses to, member for member, and the one decoder reads both. A test
(`templates_match_their_json_twins` in `crates/akit-config/src/conf/tests.rs`) holds every `.conf`
template of the pin to its JSON twin. Refreshing the fixtures refreshes the reader's types with
the inventory.

### What validation opens

Validation reads the config's own files, and nothing they name: the files a config names (TLS
certificates and keys, the examples' WebAssembly plugins, static roots, `password_file`) and the
secrets it names by environment variable (`password_env`) belong to fields akit-config refuses
as not supported yet, so the examples are validated where they are pinned and need neither
fit-examples' root as the working directory nor real secrets. `make compat-gate` still sets the
stand-in secrets fit-examples' own test sets, for when such a field is supported; whether
validation then opens what it names, relative to the directory fit-examples runs from, is that
field's iteration's decision. `examples/https` stays skipped: its certificate files are the
user's own.

### The checked-in report

[tests/compat/gate-report.txt](../tests/compat/gate-report.txt) holds every outcome, one a line,
and the gate fails when a run's outcomes differ from it in either direction: a fixture that
fails, a field newly refused, but also a fixture that starts being read, or a field that stops
being refused. The commit that changes an outcome rewrites the report with

```bash
make compat-bless
```

which refuses to write while any fixture fails; review its diff as part of the change. The gate
also fails when a field it sees refused as not supported yet is one `supported.txt` marks.

## Reading a result

`make compat-gate` runs the gate alone and prints its report, the lines of `gate-report.txt`:
one a fixture, or one a field for a fixture refused as not supported yet.

```text
examples/hello: valid
examples/http3: not supported yet: listeners[].tls.acme
examples/l4-udp: not supported yet: listeners[].protocols: the "udp" protocol
examples/https: skipped: its certificate and key files are the user's own
templates/conf/fit.conf: not supported yet: upstreams
templates/yaml/conf.d: not supported yet: upstreams
```

A fixture that fails is written `<fixture>: FAILED: <why>`, a line for each finding that is not an
allowed "not supported yet", and a difference from the checked-in report as `- <line>` (only
in the checked-in report) and `+ <line>` (only in this run). Paths akit names are written as the
inventory writes them: array indices (`routes[0]`) and map keys (`log.loggers.engine`) stand
for `[]` and `*`; a handler module akit does not have, refused at `routes[0].handler.module`, is
`routes[].handler<proxy>`. A value akit does not support yet, of a field it reads, is written
with the value after the field: `listeners[0].protocols[0]`, the `udp` protocol, is
`listeners[].protocols: the "udp" protocol`, and a config with no listeners, which fit serves
over HTTPS by default, is `listeners: a config with no listeners, ... (auto_https)`. Such a line
is not the field refused: the field stays supported, and the gate's check that no supported
field is refused looks at fields refused whole.

## Refreshing the fixtures

```bash
tests/compat/update-fixtures.sh <fit-commit> <fit-examples-commit>
```

replaces the fixtures, rewrites the pins, and regenerates the inventory and the tables below.
Moving the pin is a reviewed change, committed on its own; [SOURCE.md](../tests/compat/SOURCE.md)
says how. When only the inventory's form changed (the generator, `supported.txt`), regenerate it
alone:

```bash
python3 tools/compat-inventory.py
```

## Supporting a field

The iteration that makes akit read a field, and act on it, adds the field's path to
[tests/compat/supported.txt](../tests/compat/supported.txt) and reruns the generator in the same
commit. The generator refuses a path fit does not accept, and marks the field supported in both
languages of this page. Three tests then hold the mark to the code:

- `supported_fields_are_read` reads a small JSON config using each marked field through
  akit-config and checks the value read: a field marked supported that akit refuses, or reads and
  drops, fails it; a sample is added for each new field.
- `akit_config_lists_match_the_inventory` reads the lists in
  `crates/akit-config/src/model.rs` of what each object reads and what it refuses as not
  supported yet (and the handler modules and protocols it refuses): each entry must be a field
  of the inventory (a typo fails), together they must cover each such object's fields in the
  inventory (no field fit accepts refused as unknown), and what akit-config reads must be what
  `supported.txt` marks. A new list in `model.rs` is mapped to its inventory path in this test.
- the gate, whose report changes as the field stops being refused (`make compat-bless`).

The same commit regenerates the [config reference](reference/config.md)'s tables with
`python3 tools/config-reference.py` (`make reference`), which moves the field from not supported yet
to read; a note on how akit reads it differently from fit goes in `tools/config-reference-notes.json`.
`reference_matches_akit_config` (in the same file) fails while the reference's tables and
akit-config's lists disagree, and CI's `config-reference.py --check` while the tables are not what
the generator writes. A new extension is listed there too, with its decision. The same commit marks
the field read as an addition since the v1 freeze with `make v1-bless`
([how the freeze is checked](#how-the-freeze-is-checked)).

## akit's extensions

A few fields are akit's own, for what akio can do and fib cannot; each has a decision note, which
says why, and is to be proposed to fit ([decision 002](decisions/002-config-compatibility.md)).
akit-config lists them apart from fit's fields (`*_AKIT_ONLY` in `model.rs`), the `.conf` reader
and writer take their types from `EXTENSIONS` in `crates/akit-config/src/conf/convert.rs`, and
`akit_config_lists_match_the_inventory` fails for one that fit's inventory has, which then moves to
fit's list. A config that sets one is akit's alone until fit takes it: fit refuses it as an unknown
field.

| Field | Type | Since | Decision |
| --- | --- | --- | --- |
| `listeners[].sockmap` | `boolean` | I51b | [010](decisions/010-sockmap-relay.md): a `tcp` listener's connections spliced in the kernel by akio's eBPF sockmap relay where it loads |

## The fields

Every field fit accepts, generated from fit's JSON Schema at the pinned commit
(`fixtures/fit/schema/fit.schema.json`, which fit generates from its config types and modules, as
it does its [config reference](https://github.com/lesismal/fit/blob/main/docs/reference/config.md)).
The same list, with flags, is [tests/compat/fit-fields.txt](../tests/compat/fit-fields.txt), one
tab-separated line a field: path, section, type, flags.

Notation: `a.b` is a field of an object, `a[]` the items of an array, `a.*` the values of a map,
and `h<m>` the object at `h` whose `module` is `m`: fit's handlers, middleware and sync sources,
sinks, transforms and codecs, whose `params` depend on the module. So
`routes[].handler<respond>.params.status` is the `status` param of a route's handler when the
handler is `respond`. A module's params are in a section of their own, `<namespace>/<module>`; a
module used in several places (a sync codec under each source and sink) is listed at each.
Types: `duration` is a string such as `"1.5s"`; `bytes|percent` is a number of bytes or a share
of memory such as `"50%"`; `enum(…)` lists the choices; `module` is the module itself.

<!-- BEGIN GENERATED: tools/compat-inventory.py; do not edit -->

3821 fields in 121 sections, at fit `7daee573`; 1044 supported.

| Section | Fields | Supported | Not supported yet |
| --- | --- | --- | --- |
| [`access`](#fields-access) | 5 | 5 | 0 |
| [`access_log`](#fields-access-log) | 14 | 14 | 0 |
| [`acme`](#fields-acme) | 43 | 43 | 0 |
| [`admin`](#fields-admin) | 105 | 36 | 69 |
| [`auto_https`](#fields-auto-https) | 5 | 5 | 0 |
| [`bittorrent`](#fields-bittorrent) | 160 | 68 | 92 |
| [`engine`](#fields-engine) | 11 | 8 | 3 |
| [`http.handlers/bt_publish`](#fields-http-handlers-bt-publish) | 3 | 0 | 3 |
| [`http.handlers/bt_stream`](#fields-http-handlers-bt-stream) | 14 | 0 | 14 |
| [`http.handlers/bt_tracker`](#fields-http-handlers-bt-tracker) | 2 | 2 | 0 |
| [`http.handlers/fastcgi`](#fields-http-handlers-fastcgi) | 18 | 18 | 0 |
| [`http.handlers/forward_proxy`](#fields-http-handlers-forward-proxy) | 8 | 8 | 0 |
| [`http.handlers/hls`](#fields-http-handlers-hls) | 12 | 0 | 12 |
| [`http.handlers/httpflv`](#fields-http-handlers-httpflv) | 6 | 0 | 6 |
| [`http.handlers/media_pages`](#fields-http-handlers-media-pages) | 29 | 0 | 29 |
| [`http.handlers/media_tokens`](#fields-http-handlers-media-tokens) | 11 | 0 | 11 |
| [`http.handlers/proxy`](#fields-http-handlers-proxy) | 20 | 19 | 1 |
| [`http.handlers/respond`](#fields-http-handlers-respond) | 5 | 5 | 0 |
| [`http.handlers/static`](#fields-http-handlers-static) | 16 | 16 | 0 |
| [`http.handlers/stream_whep`](#fields-http-handlers-stream-whep) | 5 | 0 | 5 |
| [`http.handlers/stream_whip`](#fields-http-handlers-stream-whip) | 5 | 0 | 5 |
| [`http.handlers/sync_ingest`](#fields-http-handlers-sync-ingest) | 6 | 0 | 6 |
| [`http.handlers/vod`](#fields-http-handlers-vod) | 13 | 0 | 13 |
| [`http.handlers/webrtc_proxy`](#fields-http-handlers-webrtc-proxy) | 8 | 7 | 1 |
| [`http.handlers/websockify`](#fields-http-handlers-websockify) | 12 | 0 | 12 |
| [`http.handlers/whep`](#fields-http-handlers-whep) | 4 | 3 | 1 |
| [`http.handlers/whip`](#fields-http-handlers-whip) | 4 | 3 | 1 |
| [`http.middleware/basic_auth`](#fields-http-middleware-basic-auth) | 8 | 8 | 0 |
| [`http.middleware/body_limit`](#fields-http-middleware-body-limit) | 3 | 3 | 0 |
| [`http.middleware/cache`](#fields-http-middleware-cache) | 17 | 17 | 0 |
| [`http.middleware/circuit_breaker`](#fields-http-middleware-circuit-breaker) | 10 | 10 | 0 |
| [`http.middleware/client_cert`](#fields-http-middleware-client-cert) | 4 | 4 | 0 |
| [`http.middleware/compress`](#fields-http-middleware-compress) | 10 | 10 | 0 |
| [`http.middleware/concurrency_limit`](#fields-http-middleware-concurrency-limit) | 4 | 4 | 0 |
| [`http.middleware/forward_auth`](#fields-http-middleware-forward-auth) | 7 | 7 | 0 |
| [`http.middleware/headers`](#fields-http-middleware-headers) | 10 | 10 | 0 |
| [`http.middleware/jwt`](#fields-http-middleware-jwt) | 13 | 13 | 0 |
| [`http.middleware/rate_limit`](#fields-http-middleware-rate-limit) | 9 | 9 | 0 |
| [`http.middleware/redirect`](#fields-http-middleware-redirect) | 5 | 5 | 0 |
| [`http.middleware/rewrite`](#fields-http-middleware-rewrite) | 6 | 6 | 0 |
| [`http.middleware/wasm`](#fields-http-middleware-wasm) | 18 | 18 | 0 |
| [`limits`](#fields-limits) | 7 | 7 | 0 |
| [`listeners`](#fields-listeners) | 30 | 28 | 2 |
| [`llm`](#fields-llm) | 218 | 215 | 3 |
| [`log`](#fields-log) | 24 | 24 | 0 |
| [`media_auth`](#fields-media-auth) | 34 | 0 | 34 |
| [`media_library`](#fields-media-library) | 24 | 0 | 24 |
| [`metrics`](#fields-metrics) | 1 | 1 | 0 |
| [`pprof`](#fields-pprof) | 4 | 0 | 4 |
| [`providers`](#fields-providers) | 60 | 59 | 1 |
| [`routes`](#fields-routes) | 24 | 24 | 0 |
| [`schema_version`](#fields-schema-version) | 1 | 1 | 0 |
| [`services`](#fields-services) | 13 | 13 | 0 |
| [`streaming`](#fields-streaming) | 76 | 0 | 76 |
| [`sync`](#fields-sync) | 42 | 0 | 42 |
| [`sync.codecs/canal_json`](#fields-sync-codecs-canal-json) | 87 | 0 | 87 |
| [`sync.codecs/csv`](#fields-sync-codecs-csv) | 116 | 0 | 116 |
| [`sync.codecs/debezium`](#fields-sync-codecs-debezium) | 58 | 0 | 58 |
| [`sync.codecs/json`](#fields-sync-codecs-json) | 87 | 0 | 87 |
| [`sync.codecs/line`](#fields-sync-codecs-line) | 87 | 0 | 87 |
| [`sync.codecs/raw`](#fields-sync-codecs-raw) | 87 | 0 | 87 |
| [`sync.codecs/ticdc_simple`](#fields-sync-codecs-ticdc-simple) | 87 | 0 | 87 |
| [`sync.codecs/wasm`](#fields-sync-codecs-wasm) | 203 | 0 | 203 |
| [`sync.sinks/clickhouse`](#fields-sync-sinks-clickhouse) | 38 | 0 | 38 |
| [`sync.sinks/dead_letters`](#fields-sync-sinks-dead-letters) | 12 | 0 | 12 |
| [`sync.sinks/doris`](#fields-sync-sinks-doris) | 40 | 0 | 40 |
| [`sync.sinks/elasticsearch`](#fields-sync-sinks-elasticsearch) | 34 | 0 | 34 |
| [`sync.sinks/file`](#fields-sync-sinks-file) | 30 | 0 | 30 |
| [`sync.sinks/http`](#fields-sync-sinks-http) | 42 | 0 | 42 |
| [`sync.sinks/influxdb`](#fields-sync-sinks-influxdb) | 48 | 0 | 48 |
| [`sync.sinks/kafka`](#fields-sync-sinks-kafka) | 52 | 0 | 52 |
| [`sync.sinks/loki`](#fields-sync-sinks-loki) | 42 | 0 | 42 |
| [`sync.sinks/memory`](#fields-sync-sinks-memory) | 8 | 0 | 8 |
| [`sync.sinks/milvus`](#fields-sync-sinks-milvus) | 32 | 0 | 32 |
| [`sync.sinks/mongo`](#fields-sync-sinks-mongo) | 44 | 0 | 44 |
| [`sync.sinks/mqtt`](#fields-sync-sinks-mqtt) | 70 | 0 | 70 |
| [`sync.sinks/mssql`](#fields-sync-sinks-mssql) | 66 | 0 | 66 |
| [`sync.sinks/mysql`](#fields-sync-sinks-mysql) | 40 | 0 | 40 |
| [`sync.sinks/nats`](#fields-sync-sinks-nats) | 42 | 0 | 42 |
| [`sync.sinks/otlp_logs`](#fields-sync-sinks-otlp-logs) | 52 | 0 | 52 |
| [`sync.sinks/postgres`](#fields-sync-sinks-postgres) | 42 | 0 | 42 |
| [`sync.sinks/pulsar`](#fields-sync-sinks-pulsar) | 38 | 0 | 38 |
| [`sync.sinks/qdrant`](#fields-sync-sinks-qdrant) | 28 | 0 | 28 |
| [`sync.sinks/rabbitmq`](#fields-sync-sinks-rabbitmq) | 48 | 0 | 48 |
| [`sync.sinks/redis`](#fields-sync-sinks-redis) | 46 | 0 | 46 |
| [`sync.sinks/s3`](#fields-sync-sinks-s3) | 36 | 0 | 36 |
| [`sync.sinks/stdout`](#fields-sync-sinks-stdout) | 4 | 0 | 4 |
| [`sync.sinks/tdengine`](#fields-sync-sinks-tdengine) | 42 | 0 | 42 |
| [`sync.sinks/wasm`](#fields-sync-sinks-wasm) | 16 | 0 | 16 |
| [`sync.sources/clickhouse`](#fields-sync-sources-clickhouse) | 22 | 0 | 22 |
| [`sync.sources/doris`](#fields-sync-sources-doris) | 21 | 0 | 21 |
| [`sync.sources/elasticsearch`](#fields-sync-sources-elasticsearch) | 21 | 0 | 21 |
| [`sync.sources/file`](#fields-sync-sources-file) | 16 | 0 | 16 |
| [`sync.sources/generate`](#fields-sync-sources-generate) | 7 | 0 | 7 |
| [`sync.sources/http`](#fields-sync-sources-http) | 10 | 0 | 10 |
| [`sync.sources/kafka`](#fields-sync-sources-kafka) | 24 | 0 | 24 |
| [`sync.sources/mongo`](#fields-sync-sources-mongo) | 24 | 0 | 24 |
| [`sync.sources/mqtt`](#fields-sync-sources-mqtt) | 34 | 0 | 34 |
| [`sync.sources/mssql`](#fields-sync-sources-mssql) | 34 | 0 | 34 |
| [`sync.sources/mysql`](#fields-sync-sources-mysql) | 21 | 0 | 21 |
| [`sync.sources/nats`](#fields-sync-sources-nats) | 32 | 0 | 32 |
| [`sync.sources/postgres`](#fields-sync-sources-postgres) | 24 | 0 | 24 |
| [`sync.sources/pulsar`](#fields-sync-sources-pulsar) | 21 | 0 | 21 |
| [`sync.sources/rabbitmq`](#fields-sync-sources-rabbitmq) | 26 | 0 | 26 |
| [`sync.sources/redis`](#fields-sync-sources-redis) | 19 | 0 | 19 |
| [`sync.sources/s3`](#fields-sync-sources-s3) | 22 | 0 | 22 |
| [`sync.sources/tidb`](#fields-sync-sources-tidb) | 39 | 0 | 39 |
| [`sync.sources/wasm`](#fields-sync-sources-wasm) | 10 | 0 | 10 |
| [`sync.transforms/cast`](#fields-sync-transforms-cast) | 3 | 0 | 3 |
| [`sync.transforms/embed`](#fields-sync-transforms-embed) | 17 | 0 | 17 |
| [`sync.transforms/envelope`](#fields-sync-transforms-envelope) | 5 | 0 | 5 |
| [`sync.transforms/fields`](#fields-sync-transforms-fields) | 10 | 0 | 10 |
| [`sync.transforms/filter`](#fields-sync-transforms-filter) | 11 | 0 | 11 |
| [`sync.transforms/key`](#fields-sync-transforms-key) | 5 | 0 | 5 |
| [`sync.transforms/mask`](#fields-sync-transforms-mask) | 5 | 0 | 5 |
| [`sync.transforms/route`](#fields-sync-transforms-route) | 7 | 0 | 7 |
| [`sync.transforms/wasm`](#fields-sync-transforms-wasm) | 9 | 0 | 9 |
| [`telemetry`](#fields-telemetry) | 14 | 14 | 0 |
| [`tunnel`](#fields-tunnel) | 164 | 161 | 3 |
| [`upstreams`](#fields-upstreams) | 38 | 37 | 1 |
| [`webrtc`](#fields-webrtc) | 76 | 76 | 0 |
| **Total** | 3821 | 1044 | 2777 |

<a id="fields-access"></a>

#### `access`

| Field | Type | Status |
| --- | --- | --- |
| `access` | `object` | supported |
| `access.allow` | `array<string>` | supported |
| `access.allow_files` | `array<string>` | supported |
| `access.deny` | `array<string>` | supported |
| `access.deny_files` | `array<string>` | supported |

<a id="fields-access-log"></a>

#### `access_log`

| Field | Type | Status |
| --- | --- | --- |
| `access_log` | `object` | supported |
| `access_log.buffer_bytes` | `integer` | supported |
| `access_log.flush_interval` | `duration` | supported |
| `access_log.format` | `enum(json\|text\|line)` | supported |
| `access_log.level` | `enum(info\|warn\|error)` | supported |
| `access_log.output` | `string` | supported |
| `access_log.rotate` | `object` | supported |
| `access_log.rotate.compress` | `boolean` | supported |
| `access_log.rotate.interval` | `duration` | supported |
| `access_log.rotate.max_age` | `duration` | supported |
| `access_log.rotate.max_bytes` | `integer` | supported |
| `access_log.rotate.max_files` | `integer` | supported |
| `access_log.rotate.max_total_bytes` | `integer` | supported |
| `access_log.time_zone` | `string` | supported |

<a id="fields-acme"></a>

#### `acme`

| Field | Type | Status |
| --- | --- | --- |
| `acme` | `object` | supported |
| `acme.ca_file` | `string` | supported |
| `acme.challenges` | `array<enum(http-01\|tls-alpn-01)>` | supported |
| `acme.directory` | `string` | supported |
| `acme.email` | `string` | supported |
| `acme.max_concurrent` | `integer` | supported |
| `acme.on_demand` | `object` | supported |
| `acme.on_demand.ask` | `string, required` | supported |
| `acme.on_demand.idle_timeout` | `duration` | supported |
| `acme.on_demand.interval` | `duration` | supported |
| `acme.on_demand.max_names` | `integer` | supported |
| `acme.on_demand.rate` | `integer` | supported |
| `acme.on_demand.timeout` | `duration` | supported |
| `acme.request_timeout` | `duration` | supported |
| `acme.shared_storage` | `object` | supported |
| `acme.shared_storage.consul` | `object` | supported |
| `acme.shared_storage.consul.ca_file` | `string` | supported |
| `acme.shared_storage.consul.cert_file` | `string` | supported |
| `acme.shared_storage.consul.datacenter` | `string` | supported |
| `acme.shared_storage.consul.endpoint` | `string` | supported |
| `acme.shared_storage.consul.key_file` | `string` | supported |
| `acme.shared_storage.consul.token_file` | `string` | supported |
| `acme.shared_storage.etcd` | `object` | supported |
| `acme.shared_storage.etcd.ca_file` | `string` | supported |
| `acme.shared_storage.etcd.cert_file` | `string` | supported |
| `acme.shared_storage.etcd.endpoints` | `array<string>` | supported |
| `acme.shared_storage.etcd.key_file` | `string` | supported |
| `acme.shared_storage.etcd.password_file` | `string` | supported |
| `acme.shared_storage.etcd.username` | `string` | supported |
| `acme.shared_storage.lock_ttl` | `duration` | supported |
| `acme.shared_storage.prefix` | `string` | supported |
| `acme.shared_storage.redis` | `object` | supported |
| `acme.shared_storage.redis.address` | `string` | supported |
| `acme.shared_storage.redis.ca_file` | `string` | supported |
| `acme.shared_storage.redis.cert_file` | `string` | supported |
| `acme.shared_storage.redis.db` | `integer` | supported |
| `acme.shared_storage.redis.key_file` | `string` | supported |
| `acme.shared_storage.redis.password_file` | `string` | supported |
| `acme.shared_storage.redis.tls` | `boolean` | supported |
| `acme.shared_storage.redis.username` | `string` | supported |
| `acme.shared_storage.request_timeout` | `duration` | supported |
| `acme.storage` | `string` | supported |
| `acme.timeout` | `duration` | supported |

<a id="fields-admin"></a>

#### `admin`

| Field | Type | Status |
| --- | --- | --- |
| `admin` | `object` | supported |
| `admin.access` | `object` | supported |
| `admin.access.allow` | `array<string>` | supported |
| `admin.access.allow_files` | `array<string>` | supported |
| `admin.access.deny` | `array<string>` | supported |
| `admin.access.deny_files` | `array<string>` | supported |
| `admin.address` | `string` | supported |
| `admin.assistant` | `object` | not supported yet |
| `admin.assistant.data` | `object` | not supported yet |
| `admin.assistant.data.redact` | `array<string>` | not supported yet |
| `admin.assistant.data.send_access_log` | `boolean` | not supported yet |
| `admin.assistant.data.send_logs` | `boolean` | not supported yet |
| `admin.assistant.data.send_records` | `boolean` | not supported yet |
| `admin.assistant.data.send_subjects` | `boolean` | not supported yet |
| `admin.assistant.default_level` | `enum(off\|read\|edit\|propose\|apply)` | not supported yet |
| `admin.assistant.default_model` | `object` | not supported yet |
| `admin.assistant.default_model.model` | `string, required` | not supported yet |
| `admin.assistant.default_model.provider` | `string, required` | not supported yet |
| `admin.assistant.limits` | `object` | not supported yet |
| `admin.assistant.limits.max_audit_entries` | `integer` | not supported yet |
| `admin.assistant.limits.max_concurrent_chats` | `integer` | not supported yet |
| `admin.assistant.limits.max_conversations` | `integer` | not supported yet |
| `admin.assistant.limits.max_stream_bytes` | `integer` | not supported yet |
| `admin.assistant.limits.max_tokens_per_day` | `integer` | not supported yet |
| `admin.assistant.limits.max_tool_calls` | `integer` | not supported yet |
| `admin.assistant.limits.max_tool_result_bytes` | `integer` | not supported yet |
| `admin.assistant.limits.max_turns` | `integer` | not supported yet |
| `admin.assistant.mcp` | `object` | not supported yet |
| `admin.assistant.mcp.allow` | `array<string>` | not supported yet |
| `admin.assistant.mcp.max_calls_per_minute` | `integer` | not supported yet |
| `admin.assistant.mcp.max_concurrent_calls` | `integer` | not supported yet |
| `admin.assistant.mcp.max_request_bytes` | `integer` | not supported yet |
| `admin.assistant.mcp.max_sessions` | `integer` | not supported yet |
| `admin.assistant.mcp.max_token_days` | `integer` | not supported yet |
| `admin.assistant.mcp.max_tokens_per_user` | `integer` | not supported yet |
| `admin.assistant.mcp.session_idle_timeout` | `duration` | not supported yet |
| `admin.assistant.params` | `object` | not supported yet |
| `admin.assistant.params.max_output_tokens` | `integer` | not supported yet |
| `admin.assistant.params.temperature` | `number` | not supported yet |
| `admin.assistant.params.thinking` | `enum(off\|low\|medium\|high)` | not supported yet |
| `admin.assistant.providers` | `array<object>` | not supported yet |
| `admin.assistant.providers[].api_key_env` | `string` | not supported yet |
| `admin.assistant.providers[].api_version` | `string` | not supported yet |
| `admin.assistant.providers[].base_url` | `string` | not supported yet |
| `admin.assistant.providers[].connect_timeout` | `duration` | not supported yet |
| `admin.assistant.providers[].first_byte_timeout` | `duration` | not supported yet |
| `admin.assistant.providers[].headers` | `map<string>` | not supported yet |
| `admin.assistant.providers[].idle_timeout` | `duration` | not supported yet |
| `admin.assistant.providers[].kind` | `enum(anthropic\|openai\|openai_responses\|gemini\|azure_openai\|ollama\|openai_compatible\|llm_proxy), required` | not supported yet |
| `admin.assistant.providers[].models` | `array<object>` | not supported yet |
| `admin.assistant.providers[].models[].context` | `integer` | not supported yet |
| `admin.assistant.providers[].models[].name` | `string, required` | not supported yet |
| `admin.assistant.providers[].models[].tools` | `boolean` | not supported yet |
| `admin.assistant.providers[].name` | `string, required` | not supported yet |
| `admin.assistant.providers[].retries` | `integer` | not supported yet |
| `admin.assistant.providers[].timeout` | `duration` | not supported yet |
| `admin.assistant.users` | `array<object>` | not supported yet |
| `admin.assistant.users[].level` | `enum(off\|read\|edit\|propose\|apply), required` | not supported yet |
| `admin.assistant.users[].username` | `string, required` | not supported yet |
| `admin.cert_file` | `string` | supported |
| `admin.client_ca_file` | `string` | supported |
| `admin.cluster` | `object` | not supported yet |
| `admin.cluster.admins` | `array<string>` | not supported yet |
| `admin.cluster.link_address` | `string` | not supported yet |
| `admin.cluster.link_url` | `string` | not supported yet |
| `admin.cluster.max_concurrency` | `integer` | not supported yet |
| `admin.cluster.max_nodes` | `integer` | not supported yet |
| `admin.cluster.max_response_bytes` | `integer` | not supported yet |
| `admin.cluster.poll_interval` | `duration` | not supported yet |
| `admin.cluster.timeout` | `duration` | not supported yet |
| `admin.cluster_member` | `object` | not supported yet |
| `admin.cluster_member.fingerprint` | `string, required` | not supported yet |
| `admin.cluster_member.manager` | `string, required` | not supported yet |
| `admin.cluster_member.token` | `string` | not supported yet |
| `admin.cluster_member.token_file` | `string` | not supported yet |
| `admin.console` | `object` | supported |
| `admin.console.access` | `object` | supported |
| `admin.console.access.allow` | `array<string>` | supported |
| `admin.console.access.allow_files` | `array<string>` | supported |
| `admin.console.access.deny` | `array<string>` | supported |
| `admin.console.access.deny_files` | `array<string>` | supported |
| `admin.console.address` | `string` | supported |
| `admin.console.cert_file` | `string` | supported |
| `admin.console.key_file` | `string` | supported |
| `admin.console.max_conns` | `integer` | supported |
| `admin.console.max_sessions` | `integer` | supported |
| `admin.console.mptcp` | `boolean` | not supported yet |
| `admin.console.session_lifetime` | `duration` | supported |
| `admin.console.session_timeout` | `duration` | supported |
| `admin.console.users` | `array<object>, required` | supported |
| `admin.console.users[].password_hash` | `string, required` | supported |
| `admin.console.users[].username` | `string, required` | supported |
| `admin.disable_dashboard` | `boolean` | supported |
| `admin.history` | `integer` | supported |
| `admin.key_file` | `string` | supported |
| `admin.max_config_bytes` | `integer` | supported |
| `admin.max_conns` | `integer` | supported |
| `admin.mptcp` | `boolean` | not supported yet |
| `admin.node_id` | `string` | not supported yet |
| `admin.store` | `object` | supported |
| `admin.store.password` | `string` | supported |
| `admin.store.password_env` | `string` | supported |
| `admin.store.password_file` | `string` | supported |
| `admin.store.path` | `string, required` | supported |
| `admin.token_file` | `string` | supported |

<a id="fields-auto-https"></a>

#### `auto_https`

| Field | Type | Status |
| --- | --- | --- |
| `auto_https` | `object` | supported |
| `auto_https.disable` | `boolean` | supported |
| `auto_https.disable_redirects` | `boolean` | supported |
| `auto_https.http_port` | `integer` | supported |
| `auto_https.https_port` | `integer` | supported |

<a id="fields-bittorrent"></a>

#### `bittorrent`

| Field | Type | Status |
| --- | --- | --- |
| `bittorrent` | `object` | supported |
| `bittorrent.client` | `object` | supported |
| `bittorrent.client.access` | `object` | not supported yet |
| `bittorrent.client.access.allow` | `array<string>` | not supported yet |
| `bittorrent.client.access.allow_files` | `array<string>` | not supported yet |
| `bittorrent.client.access.deny` | `array<string>` | not supported yet |
| `bittorrent.client.access.deny_files` | `array<string>` | not supported yet |
| `bittorrent.client.alt_speed` | `object` | not supported yet |
| `bittorrent.client.alt_speed.download_rate` | `integer` | not supported yet |
| `bittorrent.client.alt_speed.schedules` | `array<object>` | not supported yet |
| `bittorrent.client.alt_speed.schedules[].days` | `array<string>` | not supported yet |
| `bittorrent.client.alt_speed.schedules[].from` | `string, required` | not supported yet |
| `bittorrent.client.alt_speed.schedules[].to` | `string, required` | not supported yet |
| `bittorrent.client.alt_speed.time_zone` | `string` | not supported yet |
| `bittorrent.client.alt_speed.upload_rate` | `integer` | not supported yet |
| `bittorrent.client.categories` | `array<object>` | not supported yet |
| `bittorrent.client.categories[].dir` | `string` | not supported yet |
| `bittorrent.client.categories[].limits` | `object` | not supported yet |
| `bittorrent.client.categories[].limits.download_rate` | `integer` | not supported yet |
| `bittorrent.client.categories[].limits.max_conns` | `integer` | not supported yet |
| `bittorrent.client.categories[].limits.seed_ratio` | `number` | not supported yet |
| `bittorrent.client.categories[].limits.seed_time` | `duration` | not supported yet |
| `bittorrent.client.categories[].limits.upload_rate` | `integer` | not supported yet |
| `bittorrent.client.categories[].limits.upload_slots` | `integer` | not supported yet |
| `bittorrent.client.categories[].media_library` | `string` | not supported yet |
| `bittorrent.client.categories[].name` | `string, required` | not supported yet |
| `bittorrent.client.console` | `object` | supported |
| `bittorrent.client.console.access` | `object` | supported |
| `bittorrent.client.console.access.allow` | `array<string>` | supported |
| `bittorrent.client.console.access.allow_files` | `array<string>` | supported |
| `bittorrent.client.console.access.deny` | `array<string>` | supported |
| `bittorrent.client.console.access.deny_files` | `array<string>` | supported |
| `bittorrent.client.console.address` | `string, required` | supported |
| `bittorrent.client.console.cert_file` | `string` | supported |
| `bittorrent.client.console.key_file` | `string` | supported |
| `bittorrent.client.console.max_conns` | `integer` | supported |
| `bittorrent.client.console.max_sessions` | `integer` | supported |
| `bittorrent.client.console.mptcp` | `boolean` | not supported yet |
| `bittorrent.client.console.session_lifetime` | `duration` | supported |
| `bittorrent.client.console.session_timeout` | `duration` | supported |
| `bittorrent.client.console.users` | `array<object>, required` | supported |
| `bittorrent.client.console.users[].password_hash` | `string, required` | supported |
| `bittorrent.client.console.users[].username` | `string, required` | supported |
| `bittorrent.client.data_dir` | `string, required` | supported |
| `bittorrent.client.dht` | `boolean` | supported |
| `bittorrent.client.disk_queue_bytes` | `integer` | not supported yet |
| `bittorrent.client.disk_workers` | `integer` | supported |
| `bittorrent.client.download_dir` | `string` | supported |
| `bittorrent.client.encryption` | `enum(allow\|prefer\|require)` | supported |
| `bittorrent.client.limits` | `object` | supported |
| `bittorrent.client.limits.download_rate` | `integer` | supported |
| `bittorrent.client.limits.max_active_downloads` | `integer` | not supported yet |
| `bittorrent.client.limits.max_active_seeds` | `integer` | not supported yet |
| `bittorrent.client.limits.max_conns` | `integer` | not supported yet |
| `bittorrent.client.limits.max_conns_per_torrent` | `integer` | supported |
| `bittorrent.client.limits.max_half_open` | `integer` | not supported yet |
| `bittorrent.client.limits.max_torrents` | `integer` | supported |
| `bittorrent.client.limits.min_free_bytes` | `integer` | not supported yet |
| `bittorrent.client.limits.seed_ratio` | `number` | supported |
| `bittorrent.client.limits.seed_time` | `duration` | supported |
| `bittorrent.client.limits.upload_rate` | `integer` | supported |
| `bittorrent.client.limits.upload_slots` | `integer` | supported |
| `bittorrent.client.listen` | `string, required` | supported |
| `bittorrent.client.lsd` | `boolean` | supported |
| `bittorrent.client.pex` | `boolean` | supported |
| `bittorrent.client.publish` | `array<object>` | not supported yet |
| `bittorrent.client.publish[].announce` | `array<string>` | not supported yet |
| `bittorrent.client.publish[].check_interval` | `duration` | not supported yet |
| `bittorrent.client.publish[].dir` | `string, required` | not supported yet |
| `bittorrent.client.publish[].exclude` | `array<string>` | not supported yet |
| `bittorrent.client.publish[].files` | `array<string>` | not supported yet |
| `bittorrent.client.publish[].max_files` | `integer` | not supported yet |
| `bittorrent.client.publish[].max_hashing` | `integer` | not supported yet |
| `bittorrent.client.publish[].max_total_bytes` | `integer` | not supported yet |
| `bittorrent.client.publish[].mode` | `enum(per_file\|whole_dir)` | not supported yet |
| `bittorrent.client.publish[].name` | `string, required` | not supported yet |
| `bittorrent.client.publish[].piece_length` | `integer` | not supported yet |
| `bittorrent.client.publish[].private` | `boolean` | not supported yet |
| `bittorrent.client.publish[].web_seed` | `string` | not supported yet |
| `bittorrent.client.rss` | `object` | not supported yet |
| `bittorrent.client.rss.feeds` | `array<object>` | not supported yet |
| `bittorrent.client.rss.feeds[].enabled` | `boolean` | not supported yet |
| `bittorrent.client.rss.feeds[].headers` | `map<string>` | not supported yet |
| `bittorrent.client.rss.feeds[].headers_env` | `map<string>` | not supported yet |
| `bittorrent.client.rss.feeds[].interval` | `duration` | not supported yet |
| `bittorrent.client.rss.feeds[].max_items` | `integer` | not supported yet |
| `bittorrent.client.rss.feeds[].name` | `string, required` | not supported yet |
| `bittorrent.client.rss.feeds[].url` | `string, required` | not supported yet |
| `bittorrent.client.rss.history_size` | `integer` | not supported yet |
| `bittorrent.client.rss.max_feed_bytes` | `integer` | not supported yet |
| `bittorrent.client.rss.rules` | `array<object>` | not supported yet |
| `bittorrent.client.rss.rules[].category` | `string` | not supported yet |
| `bittorrent.client.rss.rules[].enabled` | `boolean` | not supported yet |
| `bittorrent.client.rss.rules[].episodes` | `boolean` | not supported yet |
| `bittorrent.client.rss.rules[].exclude` | `array<string>` | not supported yet |
| `bittorrent.client.rss.rules[].feeds` | `array<string>` | not supported yet |
| `bittorrent.client.rss.rules[].max_per_poll` | `integer` | not supported yet |
| `bittorrent.client.rss.rules[].max_size` | `integer` | not supported yet |
| `bittorrent.client.rss.rules[].min_size` | `integer` | not supported yet |
| `bittorrent.client.rss.rules[].name` | `string, required` | not supported yet |
| `bittorrent.client.rss.rules[].patterns` | `array<string>` | not supported yet |
| `bittorrent.client.rss.rules[].paused` | `boolean` | not supported yet |
| `bittorrent.client.rss.timeout` | `duration` | not supported yet |
| `bittorrent.client.save_interval` | `duration` | supported |
| `bittorrent.client.url_timeout` | `duration` | not supported yet |
| `bittorrent.client.utp` | `boolean` | supported |
| `bittorrent.client.watch_dir` | `string` | not supported yet |
| `bittorrent.client.watch_interval` | `duration` | not supported yet |
| `bittorrent.client.watch_max_files` | `integer` | not supported yet |
| `bittorrent.tracker` | `object` | supported |
| `bittorrent.tracker.access` | `object` | supported |
| `bittorrent.tracker.access.allow` | `array<string>` | supported |
| `bittorrent.tracker.access.allow_files` | `array<string>` | supported |
| `bittorrent.tracker.access.deny` | `array<string>` | supported |
| `bittorrent.tracker.access.deny_files` | `array<string>` | supported |
| `bittorrent.tracker.allow` | `array<string>` | supported |
| `bittorrent.tracker.allow_files` | `array<string>` | supported |
| `bittorrent.tracker.auth` | `string` | not supported yet |
| `bittorrent.tracker.console` | `object` | supported |
| `bittorrent.tracker.console.access` | `object` | supported |
| `bittorrent.tracker.console.access.allow` | `array<string>` | supported |
| `bittorrent.tracker.console.access.allow_files` | `array<string>` | supported |
| `bittorrent.tracker.console.access.deny` | `array<string>` | supported |
| `bittorrent.tracker.console.access.deny_files` | `array<string>` | supported |
| `bittorrent.tracker.console.address` | `string, required` | supported |
| `bittorrent.tracker.console.cert_file` | `string` | supported |
| `bittorrent.tracker.console.key_file` | `string` | supported |
| `bittorrent.tracker.console.max_conns` | `integer` | supported |
| `bittorrent.tracker.console.max_sessions` | `integer` | supported |
| `bittorrent.tracker.console.mptcp` | `boolean` | not supported yet |
| `bittorrent.tracker.console.session_lifetime` | `duration` | supported |
| `bittorrent.tracker.console.session_timeout` | `duration` | supported |
| `bittorrent.tracker.console.users` | `array<object>, required` | supported |
| `bittorrent.tracker.console.users[].password_hash` | `string, required` | supported |
| `bittorrent.tracker.console.users[].username` | `string, required` | supported |
| `bittorrent.tracker.interval` | `duration` | supported |
| `bittorrent.tracker.listen` | `string` | supported |
| `bittorrent.tracker.max_numwant` | `integer` | supported |
| `bittorrent.tracker.max_peers` | `integer` | not supported yet |
| `bittorrent.tracker.max_peers_per_torrent` | `integer` | supported |
| `bittorrent.tracker.max_scrape` | `integer` | supported |
| `bittorrent.tracker.max_torrents` | `integer` | supported |
| `bittorrent.tracker.min_interval` | `duration` | supported |
| `bittorrent.tracker.peer_ttl` | `duration` | supported |
| `bittorrent.tracker.store` | `object` | not supported yet |
| `bittorrent.tracker.store.cache_peers` | `integer` | not supported yet |
| `bittorrent.tracker.store.cache_torrents` | `integer` | not supported yet |
| `bittorrent.tracker.store.prefix` | `string` | not supported yet |
| `bittorrent.tracker.store.queue` | `integer` | not supported yet |
| `bittorrent.tracker.store.redis` | `object` | not supported yet |
| `bittorrent.tracker.store.redis.address` | `string` | not supported yet |
| `bittorrent.tracker.store.redis.ca_file` | `string` | not supported yet |
| `bittorrent.tracker.store.redis.cert_file` | `string` | not supported yet |
| `bittorrent.tracker.store.redis.db` | `integer` | not supported yet |
| `bittorrent.tracker.store.redis.key_file` | `string` | not supported yet |
| `bittorrent.tracker.store.redis.password_file` | `string` | not supported yet |
| `bittorrent.tracker.store.redis.tls` | `boolean` | not supported yet |
| `bittorrent.tracker.store.redis.username` | `string` | not supported yet |
| `bittorrent.tracker.store.refresh` | `duration` | not supported yet |
| `bittorrent.tracker.store.request_timeout` | `duration` | not supported yet |

<a id="fields-engine"></a>

#### `engine`

| Field | Type | Status |
| --- | --- | --- |
| `engine` | `object` | supported |
| `engine.backlog` | `integer` | supported |
| `engine.event_loops` | `integer` | supported |
| `engine.kernel_tls_offload` | `boolean` | not supported yet |
| `engine.loop_wait` | `enum(park\|block)` | not supported yet |
| `engine.min_workers` | `integer` | supported |
| `engine.poll_batch` | `integer` | not supported yet |
| `engine.read_buffer_size` | `integer` | supported |
| `engine.task_queue` | `integer` | supported |
| `engine.workers` | `integer` | supported |
| `engine.write_watermark` | `integer` | supported |

<a id="fields-http-handlers-bt-publish"></a>

#### `http.handlers/bt_publish`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].handler<bt_publish>` | `module` | not supported yet |
| `routes[].handler<bt_publish>.params` | `object` | not supported yet |
| `routes[].handler<bt_publish>.params.publish` | `array<string>` | not supported yet |

<a id="fields-http-handlers-bt-stream"></a>

#### `http.handlers/bt_stream`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].handler<bt_stream>` | `module` | not supported yet |
| `routes[].handler<bt_stream>.params` | `object` | not supported yet |
| `routes[].handler<bt_stream>.params.auth` | `string` | not supported yet |
| `routes[].handler<bt_stream>.params.cache_control` | `string` | not supported yet |
| `routes[].handler<bt_stream>.params.cors` | `string` | not supported yet |
| `routes[].handler<bt_stream>.params.info_timeout` | `duration` | not supported yet |
| `routes[].handler<bt_stream>.params.magnet` | `boolean` | not supported yet |
| `routes[].handler<bt_stream>.params.max_streams` | `integer` | not supported yet |
| `routes[].handler<bt_stream>.params.max_temporary` | `integer` | not supported yet |
| `routes[].handler<bt_stream>.params.max_temporary_bytes` | `integer` | not supported yet |
| `routes[].handler<bt_stream>.params.prefix` | `string` | not supported yet |
| `routes[].handler<bt_stream>.params.read_timeout` | `duration` | not supported yet |
| `routes[].handler<bt_stream>.params.readahead_bytes` | `integer` | not supported yet |
| `routes[].handler<bt_stream>.params.temporary_ttl` | `duration` | not supported yet |

<a id="fields-http-handlers-bt-tracker"></a>

#### `http.handlers/bt_tracker`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].handler<bt_tracker>` | `module` | supported |
| `routes[].handler<bt_tracker>.params` | `object` | supported |

<a id="fields-http-handlers-fastcgi"></a>

#### `http.handlers/fastcgi`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].handler<fastcgi>` | `module` | supported |
| `routes[].handler<fastcgi>.params` | `object` | supported |
| `routes[].handler<fastcgi>.params.address` | `string, required` | supported |
| `routes[].handler<fastcgi>.params.connect_timeout` | `duration` | supported |
| `routes[].handler<fastcgi>.params.dotfiles` | `enum(deny\|allow)` | supported |
| `routes[].handler<fastcgi>.params.env` | `map<string>` | supported |
| `routes[].handler<fastcgi>.params.idle_timeout` | `duration` | supported |
| `routes[].handler<fastcgi>.params.index` | `string` | supported |
| `routes[].handler<fastcgi>.params.max_conns` | `integer` | supported |
| `routes[].handler<fastcgi>.params.max_header_bytes` | `integer` | supported |
| `routes[].handler<fastcgi>.params.max_idle_conns` | `integer` | supported |
| `routes[].handler<fastcgi>.params.max_pending` | `integer` | supported |
| `routes[].handler<fastcgi>.params.response_header_timeout` | `duration` | supported |
| `routes[].handler<fastcgi>.params.response_idle_timeout` | `duration` | supported |
| `routes[].handler<fastcgi>.params.root` | `string, required` | supported |
| `routes[].handler<fastcgi>.params.split_path` | `array<string>` | supported |
| `routes[].handler<fastcgi>.params.timeout` | `duration` | supported |
| `routes[].handler<fastcgi>.params.try_files` | `array<string>` | supported |

<a id="fields-http-handlers-forward-proxy"></a>

#### `http.handlers/forward_proxy`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].handler<forward_proxy>` | `module` | supported |
| `routes[].handler<forward_proxy>.params` | `object` | supported |
| `routes[].handler<forward_proxy>.params.allow` | `array<string>` | supported |
| `routes[].handler<forward_proxy>.params.clients` | `array<string>, required` | supported |
| `routes[].handler<forward_proxy>.params.connect_timeout` | `duration` | supported |
| `routes[].handler<forward_proxy>.params.deny` | `array<string>` | supported |
| `routes[].handler<forward_proxy>.params.idle_timeout` | `duration` | supported |
| `routes[].handler<forward_proxy>.params.ports` | `array<integer>` | supported |

<a id="fields-http-handlers-hls"></a>

#### `http.handlers/hls`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].handler<hls>` | `module` | not supported yet |
| `routes[].handler<hls>.params` | `object` | not supported yet |
| `routes[].handler<hls>.params.auth` | `string` | not supported yet |
| `routes[].handler<hls>.params.cors` | `string` | not supported yet |
| `routes[].handler<hls>.params.format` | `enum(fmp4\|ts)` | not supported yet |
| `routes[].handler<hls>.params.idle_timeout` | `duration` | not supported yet |
| `routes[].handler<hls>.params.max_bytes` | `integer` | not supported yet |
| `routes[].handler<hls>.params.part_duration` | `duration` | not supported yet |
| `routes[].handler<hls>.params.play_wait` | `duration` | not supported yet |
| `routes[].handler<hls>.params.playlist_size` | `integer` | not supported yet |
| `routes[].handler<hls>.params.prefix` | `string` | not supported yet |
| `routes[].handler<hls>.params.target_duration` | `duration` | not supported yet |

<a id="fields-http-handlers-httpflv"></a>

#### `http.handlers/httpflv`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].handler<httpflv>` | `module` | not supported yet |
| `routes[].handler<httpflv>.params` | `object` | not supported yet |
| `routes[].handler<httpflv>.params.auth` | `string` | not supported yet |
| `routes[].handler<httpflv>.params.cors` | `string` | not supported yet |
| `routes[].handler<httpflv>.params.prefix` | `string` | not supported yet |
| `routes[].handler<httpflv>.params.wait` | `duration` | not supported yet |

<a id="fields-http-handlers-media-pages"></a>

#### `http.handlers/media_pages`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].handler<media_pages>` | `module` | not supported yet |
| `routes[].handler<media_pages>.params` | `object` | not supported yet |
| `routes[].handler<media_pages>.params.auth` | `string` | not supported yet |
| `routes[].handler<media_pages>.params.cam_default` | `boolean` | not supported yet |
| `routes[].handler<media_pages>.params.channels` | `boolean` | not supported yet |
| `routes[].handler<media_pages>.params.chat` | `boolean` | not supported yet |
| `routes[].handler<media_pages>.params.hls` | `string` | not supported yet |
| `routes[].handler<media_pages>.params.ice_servers` | `array<object>` | not supported yet |
| `routes[].handler<media_pages>.params.ice_servers[].credential` | `string` | not supported yet |
| `routes[].handler<media_pages>.params.ice_servers[].urls` | `array<string>, required` | not supported yet |
| `routes[].handler<media_pages>.params.ice_servers[].username` | `string` | not supported yet |
| `routes[].handler<media_pages>.params.libraries` | `array<string>` | not supported yet |
| `routes[].handler<media_pages>.params.links` | `array<object>` | not supported yet |
| `routes[].handler<media_pages>.params.links[].name` | `string, required` | not supported yet |
| `routes[].handler<media_pages>.params.links[].url` | `string, required` | not supported yet |
| `routes[].handler<media_pages>.params.login` | `boolean` | not supported yet |
| `routes[].handler<media_pages>.params.max_file_bytes` | `integer` | not supported yet |
| `routes[].handler<media_pages>.params.max_members` | `integer` | not supported yet |
| `routes[].handler<media_pages>.params.max_message_bytes` | `integer` | not supported yet |
| `routes[].handler<media_pages>.params.max_rooms` | `integer` | not supported yet |
| `routes[].handler<media_pages>.params.member_ttl` | `duration` | not supported yet |
| `routes[].handler<media_pages>.params.mic_default` | `boolean` | not supported yet |
| `routes[].handler<media_pages>.params.pages` | `boolean` | not supported yet |
| `routes[].handler<media_pages>.params.prefix` | `string` | not supported yet |
| `routes[].handler<media_pages>.params.stall_timeout` | `duration` | not supported yet |
| `routes[].handler<media_pages>.params.title` | `string` | not supported yet |
| `routes[].handler<media_pages>.params.vod` | `string` | not supported yet |
| `routes[].handler<media_pages>.params.whep` | `string` | not supported yet |
| `routes[].handler<media_pages>.params.whip` | `string` | not supported yet |

<a id="fields-http-handlers-media-tokens"></a>

#### `http.handlers/media_tokens`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].handler<media_tokens>` | `module` | not supported yet |
| `routes[].handler<media_tokens>.params` | `object` | not supported yet |
| `routes[].handler<media_tokens>.params.auth` | `string` | not supported yet |
| `routes[].handler<media_tokens>.params.max_body_bytes` | `integer` | not supported yet |
| `routes[].handler<media_tokens>.params.max_tokens` | `integer` | not supported yet |
| `routes[].handler<media_tokens>.params.max_ttl` | `duration` | not supported yet |
| `routes[].handler<media_tokens>.params.prefix` | `string` | not supported yet |
| `routes[].handler<media_tokens>.params.prefixes` | `array<string>` | not supported yet |
| `routes[].handler<media_tokens>.params.revoke_timeout` | `duration` | not supported yet |
| `routes[].handler<media_tokens>.params.url_base` | `string` | not supported yet |
| `routes[].handler<media_tokens>.params.url_prefixes` | `array<string>` | not supported yet |

<a id="fields-http-handlers-proxy"></a>

#### `http.handlers/proxy`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].handler<proxy>` | `module` | supported |
| `routes[].handler<proxy>.params` | `object` | supported |
| `routes[].handler<proxy>.params.failover` | `string` | supported |
| `routes[].handler<proxy>.params.host_header` | `string` | supported |
| `routes[].handler<proxy>.params.mirror` | `object` | supported |
| `routes[].handler<proxy>.params.mirror.percent` | `number` | supported |
| `routes[].handler<proxy>.params.mirror.upstream` | `string, required` | supported |
| `routes[].handler<proxy>.params.retry` | `object` | supported |
| `routes[].handler<proxy>.params.retry.attempts` | `integer` | supported |
| `routes[].handler<proxy>.params.retry.budget_min_per_second` | `number` | supported |
| `routes[].handler<proxy>.params.retry.budget_ratio` | `number` | supported |
| `routes[].handler<proxy>.params.retry.statuses` | `array<integer>` | supported |
| `routes[].handler<proxy>.params.splice_uploads` | `boolean` | not supported yet |
| `routes[].handler<proxy>.params.timeout` | `duration` | supported |
| `routes[].handler<proxy>.params.trusted_proxies` | `array<string>` | supported |
| `routes[].handler<proxy>.params.tunnel_idle_timeout` | `duration` | supported |
| `routes[].handler<proxy>.params.upstream` | `string` | supported |
| `routes[].handler<proxy>.params.upstreams` | `array<object>` | supported |
| `routes[].handler<proxy>.params.upstreams[].upstream` | `string, required` | supported |
| `routes[].handler<proxy>.params.upstreams[].weight` | `integer, required` | supported |

<a id="fields-http-handlers-respond"></a>

#### `http.handlers/respond`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].handler<respond>` | `module` | supported |
| `routes[].handler<respond>.params` | `object` | supported |
| `routes[].handler<respond>.params.body` | `string` | supported |
| `routes[].handler<respond>.params.headers` | `map<string>` | supported |
| `routes[].handler<respond>.params.status` | `integer` | supported |

<a id="fields-http-handlers-static"></a>

#### `http.handlers/static`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].handler<static>` | `module` | supported |
| `routes[].handler<static>.params` | `object` | supported |
| `routes[].handler<static>.params.browse` | `boolean` | supported |
| `routes[].handler<static>.params.browse_max_entries` | `integer` | supported |
| `routes[].handler<static>.params.check_interval` | `duration` | supported |
| `routes[].handler<static>.params.compress` | `array<enum(zstd\|br\|gzip)>` | supported |
| `routes[].handler<static>.params.default_type` | `string` | supported |
| `routes[].handler<static>.params.disk_io` | `enum(pool\|auto)` | supported |
| `routes[].handler<static>.params.dotfiles` | `enum(deny\|allow)` | supported |
| `routes[].handler<static>.params.index` | `array<string>` | supported |
| `routes[].handler<static>.params.max_cache_bytes` | `integer` | supported |
| `routes[].handler<static>.params.max_file_bytes` | `integer` | supported |
| `routes[].handler<static>.params.precompressed` | `array<enum(br\|zstd\|gzip)>` | supported |
| `routes[].handler<static>.params.root` | `string, required` | supported |
| `routes[].handler<static>.params.strip_prefix` | `string` | supported |
| `routes[].handler<static>.params.try_files` | `array<string>` | supported |

<a id="fields-http-handlers-stream-whep"></a>

#### `http.handlers/stream_whep`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].handler<stream_whep>` | `module` | not supported yet |
| `routes[].handler<stream_whep>.params` | `object` | not supported yet |
| `routes[].handler<stream_whep>.params.auth` | `string` | not supported yet |
| `routes[].handler<stream_whep>.params.prefix` | `string` | not supported yet |
| `routes[].handler<stream_whep>.params.stream` | `string` | not supported yet |

<a id="fields-http-handlers-stream-whip"></a>

#### `http.handlers/stream_whip`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].handler<stream_whip>` | `module` | not supported yet |
| `routes[].handler<stream_whip>.params` | `object` | not supported yet |
| `routes[].handler<stream_whip>.params.auth` | `string` | not supported yet |
| `routes[].handler<stream_whip>.params.prefix` | `string` | not supported yet |
| `routes[].handler<stream_whip>.params.stream` | `string` | not supported yet |

<a id="fields-http-handlers-sync-ingest"></a>

#### `http.handlers/sync_ingest`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].handler<sync_ingest>` | `module` | not supported yet |
| `routes[].handler<sync_ingest>.params` | `object` | not supported yet |
| `routes[].handler<sync_ingest>.params.max_body_bytes` | `integer` | not supported yet |
| `routes[].handler<sync_ingest>.params.pipeline` | `string, required` | not supported yet |
| `routes[].handler<sync_ingest>.params.retry_after` | `duration` | not supported yet |
| `routes[].handler<sync_ingest>.params.timeout` | `duration` | not supported yet |

<a id="fields-http-handlers-vod"></a>

#### `http.handlers/vod`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].handler<vod>` | `module` | not supported yet |
| `routes[].handler<vod>.params` | `object` | not supported yet |
| `routes[].handler<vod>.params.auth` | `string` | not supported yet |
| `routes[].handler<vod>.params.cache_control` | `string` | not supported yet |
| `routes[].handler<vod>.params.container` | `enum(fmp4\|ts)` | not supported yet |
| `routes[].handler<vod>.params.cors` | `string` | not supported yet |
| `routes[].handler<vod>.params.encrypt` | `boolean` | not supported yet |
| `routes[].handler<vod>.params.library` | `string, required` | not supported yet |
| `routes[].handler<vod>.params.max_cache_bytes` | `integer` | not supported yet |
| `routes[].handler<vod>.params.max_segment_bytes` | `integer` | not supported yet |
| `routes[].handler<vod>.params.prefix` | `string` | not supported yet |
| `routes[].handler<vod>.params.segment_duration` | `duration` | not supported yet |
| `routes[].handler<vod>.params.timeout` | `duration` | not supported yet |

<a id="fields-http-handlers-webrtc-proxy"></a>

#### `http.handlers/webrtc_proxy`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].handler<webrtc_proxy>` | `module` | supported |
| `routes[].handler<webrtc_proxy>.params` | `object` | supported |
| `routes[].handler<webrtc_proxy>.params.action` | `enum(publish\|play)` | supported |
| `routes[].handler<webrtc_proxy>.params.auth` | `string` | not supported yet |
| `routes[].handler<webrtc_proxy>.params.max_sdp_bytes` | `integer` | supported |
| `routes[].handler<webrtc_proxy>.params.media_addrs` | `map<string>` | supported |
| `routes[].handler<webrtc_proxy>.params.timeout` | `duration` | supported |
| `routes[].handler<webrtc_proxy>.params.upstream` | `string, required` | supported |

<a id="fields-http-handlers-websockify"></a>

#### `http.handlers/websockify`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].handler<websockify>` | `module` | not supported yet |
| `routes[].handler<websockify>.params` | `object` | not supported yet |
| `routes[].handler<websockify>.params.address` | `string` | not supported yet |
| `routes[].handler<websockify>.params.connect_timeout` | `duration` | not supported yet |
| `routes[].handler<websockify>.params.idle_timeout` | `duration` | not supported yet |
| `routes[].handler<websockify>.params.max_conns` | `integer` | not supported yet |
| `routes[].handler<websockify>.params.max_frame_bytes` | `integer` | not supported yet |
| `routes[].handler<websockify>.params.name` | `string` | not supported yet |
| `routes[].handler<websockify>.params.origins` | `array<string>` | not supported yet |
| `routes[].handler<websockify>.params.token_param` | `string` | not supported yet |
| `routes[].handler<websockify>.params.tokens` | `map<string>` | not supported yet |
| `routes[].handler<websockify>.params.upstream` | `string` | not supported yet |

<a id="fields-http-handlers-whep"></a>

#### `http.handlers/whep`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].handler<whep>` | `module` | supported |
| `routes[].handler<whep>.params` | `object` | supported |
| `routes[].handler<whep>.params.auth` | `string` | not supported yet |
| `routes[].handler<whep>.params.stream` | `string` | supported |

<a id="fields-http-handlers-whip"></a>

#### `http.handlers/whip`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].handler<whip>` | `module` | supported |
| `routes[].handler<whip>.params` | `object` | supported |
| `routes[].handler<whip>.params.auth` | `string` | not supported yet |
| `routes[].handler<whip>.params.stream` | `string` | supported |

<a id="fields-http-middleware-basic-auth"></a>

#### `http.middleware/basic_auth`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].middleware[]<basic_auth>` | `module` | supported |
| `routes[].middleware[]<basic_auth>.params` | `object` | supported |
| `routes[].middleware[]<basic_auth>.params.forward_authorization` | `boolean` | supported |
| `routes[].middleware[]<basic_auth>.params.realm` | `string` | supported |
| `routes[].middleware[]<basic_auth>.params.user_header` | `string` | supported |
| `routes[].middleware[]<basic_auth>.params.users` | `array<object>, required` | supported |
| `routes[].middleware[]<basic_auth>.params.users[].password_hash` | `string, required` | supported |
| `routes[].middleware[]<basic_auth>.params.users[].username` | `string, required` | supported |

<a id="fields-http-middleware-body-limit"></a>

#### `http.middleware/body_limit`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].middleware[]<body_limit>` | `module` | supported |
| `routes[].middleware[]<body_limit>.params` | `object` | supported |
| `routes[].middleware[]<body_limit>.params.max_bytes` | `integer, required` | supported |

<a id="fields-http-middleware-cache"></a>

#### `http.middleware/cache`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].middleware[]<cache>` | `module` | supported |
| `routes[].middleware[]<cache>.params` | `object` | supported |
| `routes[].middleware[]<cache>.params.default_ttl` | `duration` | supported |
| `routes[].middleware[]<cache>.params.disable_lock` | `boolean` | supported |
| `routes[].middleware[]<cache>.params.disk` | `object` | supported |
| `routes[].middleware[]<cache>.params.disk.max_bytes` | `integer` | supported |
| `routes[].middleware[]<cache>.params.disk.max_entries` | `integer` | supported |
| `routes[].middleware[]<cache>.params.disk.max_size` | `integer, deprecated` | supported |
| `routes[].middleware[]<cache>.params.disk.path` | `string, required` | supported |
| `routes[].middleware[]<cache>.params.lock_timeout` | `duration` | supported |
| `routes[].middleware[]<cache>.params.max_bytes` | `integer` | supported |
| `routes[].middleware[]<cache>.params.max_entry_bytes` | `integer` | supported |
| `routes[].middleware[]<cache>.params.max_entry_size` | `integer, deprecated` | supported |
| `routes[].middleware[]<cache>.params.max_size` | `integer, deprecated` | supported |
| `routes[].middleware[]<cache>.params.stale_if_error` | `duration` | supported |
| `routes[].middleware[]<cache>.params.stale_while_revalidate` | `duration` | supported |
| `routes[].middleware[]<cache>.params.zone` | `string` | supported |

<a id="fields-http-middleware-circuit-breaker"></a>

#### `http.middleware/circuit_breaker`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].middleware[]<circuit_breaker>` | `module` | supported |
| `routes[].middleware[]<circuit_breaker>.params` | `object` | supported |
| `routes[].middleware[]<circuit_breaker>.params.error_ratio` | `number` | supported |
| `routes[].middleware[]<circuit_breaker>.params.half_open_requests` | `integer` | supported |
| `routes[].middleware[]<circuit_breaker>.params.min_requests` | `integer` | supported |
| `routes[].middleware[]<circuit_breaker>.params.open_duration` | `duration` | supported |
| `routes[].middleware[]<circuit_breaker>.params.slow_ratio` | `number` | supported |
| `routes[].middleware[]<circuit_breaker>.params.slow_threshold` | `duration` | supported |
| `routes[].middleware[]<circuit_breaker>.params.statuses` | `array<integer>` | supported |
| `routes[].middleware[]<circuit_breaker>.params.window` | `duration` | supported |

<a id="fields-http-middleware-client-cert"></a>

#### `http.middleware/client_cert`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].middleware[]<client_cert>` | `module` | supported |
| `routes[].middleware[]<client_cert>.params` | `object` | supported |
| `routes[].middleware[]<client_cert>.params.optional` | `boolean` | supported |
| `routes[].middleware[]<client_cert>.params.subject_header` | `string` | supported |

<a id="fields-http-middleware-compress"></a>

#### `http.middleware/compress`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].middleware[]<compress>` | `module` | supported |
| `routes[].middleware[]<compress>.params` | `object` | supported |
| `routes[].middleware[]<compress>.params.brotli_level` | `integer` | supported |
| `routes[].middleware[]<compress>.params.encodings` | `array<enum(zstd\|br\|gzip)>` | supported |
| `routes[].middleware[]<compress>.params.gzip_level` | `integer` | supported |
| `routes[].middleware[]<compress>.params.level` | `enum(fastest\|default\|better)` | supported |
| `routes[].middleware[]<compress>.params.max_in_flight` | `integer` | supported |
| `routes[].middleware[]<compress>.params.min_bytes` | `integer` | supported |
| `routes[].middleware[]<compress>.params.min_length` | `integer, deprecated` | supported |
| `routes[].middleware[]<compress>.params.zstd_level` | `integer` | supported |

<a id="fields-http-middleware-concurrency-limit"></a>

#### `http.middleware/concurrency_limit`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].middleware[]<concurrency_limit>` | `module` | supported |
| `routes[].middleware[]<concurrency_limit>.params` | `object` | supported |
| `routes[].middleware[]<concurrency_limit>.params.max` | `integer, required` | supported |
| `routes[].middleware[]<concurrency_limit>.params.status` | `integer` | supported |

<a id="fields-http-middleware-forward-auth"></a>

#### `http.middleware/forward_auth`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].middleware[]<forward_auth>` | `module` | supported |
| `routes[].middleware[]<forward_auth>.params` | `object` | supported |
| `routes[].middleware[]<forward_auth>.params.copy_headers` | `array<string>` | supported |
| `routes[].middleware[]<forward_auth>.params.request_headers` | `array<string>` | supported |
| `routes[].middleware[]<forward_auth>.params.timeout` | `duration` | supported |
| `routes[].middleware[]<forward_auth>.params.upstream` | `string, required` | supported |
| `routes[].middleware[]<forward_auth>.params.uri` | `string` | supported |

<a id="fields-http-middleware-headers"></a>

#### `http.middleware/headers`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].middleware[]<headers>` | `module` | supported |
| `routes[].middleware[]<headers>.params` | `object` | supported |
| `routes[].middleware[]<headers>.params.request` | `object` | supported |
| `routes[].middleware[]<headers>.params.request.add` | `map<array<string>>` | supported |
| `routes[].middleware[]<headers>.params.request.delete` | `array<string>` | supported |
| `routes[].middleware[]<headers>.params.request.set` | `map<string>` | supported |
| `routes[].middleware[]<headers>.params.response` | `object` | supported |
| `routes[].middleware[]<headers>.params.response.add` | `map<array<string>>` | supported |
| `routes[].middleware[]<headers>.params.response.delete` | `array<string>` | supported |
| `routes[].middleware[]<headers>.params.response.set` | `map<string>` | supported |

<a id="fields-http-middleware-jwt"></a>

#### `http.middleware/jwt`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].middleware[]<jwt>` | `module` | supported |
| `routes[].middleware[]<jwt>.params` | `object` | supported |
| `routes[].middleware[]<jwt>.params.algorithms` | `array<enum(RS256\|RS384\|RS512\|PS256\|PS384\|PS512\|ES256\|ES384\|ES512\|EdDSA\|HS256\|HS384\|HS512)>` | supported |
| `routes[].middleware[]<jwt>.params.audience` | `array<string>` | supported |
| `routes[].middleware[]<jwt>.params.claims_headers` | `map<string>` | supported |
| `routes[].middleware[]<jwt>.params.hmac_secret_file` | `string` | supported |
| `routes[].middleware[]<jwt>.params.issuer` | `string` | supported |
| `routes[].middleware[]<jwt>.params.jwks_refresh` | `duration, deprecated` | supported |
| `routes[].middleware[]<jwt>.params.jwks_refresh_interval` | `duration` | supported |
| `routes[].middleware[]<jwt>.params.jwks_url` | `string` | supported |
| `routes[].middleware[]<jwt>.params.leeway` | `duration` | supported |
| `routes[].middleware[]<jwt>.params.public_key_files` | `array<string>` | supported |
| `routes[].middleware[]<jwt>.params.token_from` | `string` | supported |

<a id="fields-http-middleware-rate-limit"></a>

#### `http.middleware/rate_limit`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].middleware[]<rate_limit>` | `module` | supported |
| `routes[].middleware[]<rate_limit>.params` | `object` | supported |
| `routes[].middleware[]<rate_limit>.params.burst` | `integer` | supported |
| `routes[].middleware[]<rate_limit>.params.ipv6_prefix` | `integer` | supported |
| `routes[].middleware[]<rate_limit>.params.key` | `string` | supported |
| `routes[].middleware[]<rate_limit>.params.max_keys` | `integer` | supported |
| `routes[].middleware[]<rate_limit>.params.rate` | `number, required` | supported |
| `routes[].middleware[]<rate_limit>.params.status` | `integer` | supported |
| `routes[].middleware[]<rate_limit>.params.trusted_proxies` | `array<string>` | supported |

<a id="fields-http-middleware-redirect"></a>

#### `http.middleware/redirect`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].middleware[]<redirect>` | `module` | supported |
| `routes[].middleware[]<redirect>.params` | `object` | supported |
| `routes[].middleware[]<redirect>.params.regexp` | `string` | supported |
| `routes[].middleware[]<redirect>.params.status` | `integer` | supported |
| `routes[].middleware[]<redirect>.params.to` | `string, required` | supported |

<a id="fields-http-middleware-rewrite"></a>

#### `http.middleware/rewrite`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].middleware[]<rewrite>` | `module` | supported |
| `routes[].middleware[]<rewrite>.params` | `object` | supported |
| `routes[].middleware[]<rewrite>.params.add_prefix` | `string` | supported |
| `routes[].middleware[]<rewrite>.params.regexp` | `string` | supported |
| `routes[].middleware[]<rewrite>.params.replacement` | `string` | supported |
| `routes[].middleware[]<rewrite>.params.strip_prefix` | `array<string>` | supported |

<a id="fields-http-middleware-wasm"></a>

#### `http.middleware/wasm`

| Field | Type | Status |
| --- | --- | --- |
| `routes[].middleware[]<wasm>` | `module` | supported |
| `routes[].middleware[]<wasm>.params` | `object` | supported |
| `routes[].middleware[]<wasm>.params.config` | `string` | supported |
| `routes[].middleware[]<wasm>.params.instances` | `integer` | supported |
| `routes[].middleware[]<wasm>.params.max_body_bytes` | `integer` | supported |
| `routes[].middleware[]<wasm>.params.max_memory` | `integer, deprecated` | supported |
| `routes[].middleware[]<wasm>.params.max_memory_bytes` | `integer` | supported |
| `routes[].middleware[]<wasm>.params.max_streams` | `integer` | supported |
| `routes[].middleware[]<wasm>.params.on_error` | `enum(fail\|skip)` | supported |
| `routes[].middleware[]<wasm>.params.path` | `string, required` | supported |
| `routes[].middleware[]<wasm>.params.properties` | `array<object>` | supported |
| `routes[].middleware[]<wasm>.params.properties[].path` | `array<string>, required` | supported |
| `routes[].middleware[]<wasm>.params.properties[].value` | `string` | supported |
| `routes[].middleware[]<wasm>.params.queue` | `integer` | supported |
| `routes[].middleware[]<wasm>.params.response` | `enum(all\|headers\|none)` | supported |
| `routes[].middleware[]<wasm>.params.timeout` | `duration` | supported |
| `routes[].middleware[]<wasm>.params.vm_config` | `string` | supported |
| `routes[].middleware[]<wasm>.params.wait_timeout` | `duration` | supported |

<a id="fields-limits"></a>

#### `limits`

| Field | Type | Status |
| --- | --- | --- |
| `limits` | `object` | supported |
| `limits.max_body_bytes` | `integer` | supported |
| `limits.max_conns` | `integer` | supported |
| `limits.max_header_bytes` | `integer` | supported |
| `limits.max_pending_bytes` | `bytes\|percent` | supported |
| `limits.max_write_queue_bytes` | `integer` | supported |
| `limits.write_stall_timeout` | `duration` | supported |

<a id="fields-listeners"></a>

#### `listeners`

| Field | Type | Status |
| --- | --- | --- |
| `listeners` | `array<object>` | supported |
| `listeners[].access` | `object` | supported |
| `listeners[].access.allow` | `array<string>` | supported |
| `listeners[].access.allow_files` | `array<string>` | supported |
| `listeners[].access.deny` | `array<string>` | supported |
| `listeners[].access.deny_files` | `array<string>` | supported |
| `listeners[].address` | `string, required` | supported |
| `listeners[].mptcp` | `boolean` | not supported yet |
| `listeners[].name` | `string, required` | supported |
| `listeners[].protocols` | `array<enum(http/1.1\|h2c\|h2\|h3\|tcp\|udp\|socks)>` | supported |
| `listeners[].proxy_protocol` | `object` | supported |
| `listeners[].proxy_protocol.trusted` | `array<string>, required` | supported |
| `listeners[].socks` | `object` | supported |
| `listeners[].socks.allow` | `array<string>` | supported |
| `listeners[].socks.clients` | `array<string>, required` | supported |
| `listeners[].socks.connect_timeout` | `duration` | supported |
| `listeners[].socks.deny` | `array<string>` | supported |
| `listeners[].socks.idle_timeout` | `duration` | supported |
| `listeners[].socks.ports` | `array<integer>` | supported |
| `listeners[].tls` | `object` | supported |
| `listeners[].tls.acme` | `array<string>` | supported |
| `listeners[].tls.certificates` | `array<object>` | supported |
| `listeners[].tls.certificates[].cert_file` | `string, required` | supported |
| `listeners[].tls.certificates[].key_file` | `string, required` | supported |
| `listeners[].tls.client_auth` | `object` | supported |
| `listeners[].tls.client_auth.ca_files` | `array<string>, required` | supported |
| `listeners[].tls.client_auth.mode` | `enum(require\|verify_if_given)` | supported |
| `listeners[].tls.kernel_tls_offload` | `boolean` | not supported yet |
| `listeners[].tls.max_version` | `enum(1.2\|1.3)` | supported |
| `listeners[].tls.on_demand` | `boolean` | supported |

<a id="fields-llm"></a>

#### `llm`

| Field | Type | Status |
| --- | --- | --- |
| `llm` | `object` | supported |
| `llm.hub` | `object` | supported |
| `llm.hub.access` | `object` | supported |
| `llm.hub.access.allow` | `array<string>` | supported |
| `llm.hub.access.allow_files` | `array<string>` | supported |
| `llm.hub.access.deny` | `array<string>` | supported |
| `llm.hub.access.deny_files` | `array<string>` | supported |
| `llm.hub.console` | `object` | supported |
| `llm.hub.console.access` | `object` | supported |
| `llm.hub.console.access.allow` | `array<string>` | supported |
| `llm.hub.console.access.allow_files` | `array<string>` | supported |
| `llm.hub.console.access.deny` | `array<string>` | supported |
| `llm.hub.console.access.deny_files` | `array<string>` | supported |
| `llm.hub.console.address` | `string, required` | supported |
| `llm.hub.console.cert_file` | `string` | supported |
| `llm.hub.console.key_file` | `string` | supported |
| `llm.hub.console.max_conns` | `integer` | supported |
| `llm.hub.console.max_sessions` | `integer` | supported |
| `llm.hub.console.mptcp` | `boolean` | not supported yet |
| `llm.hub.console.session_lifetime` | `duration` | supported |
| `llm.hub.console.session_timeout` | `duration` | supported |
| `llm.hub.console.users` | `array<object>, required` | supported |
| `llm.hub.console.users[].password_hash` | `string, required` | supported |
| `llm.hub.console.users[].username` | `string, required` | supported |
| `llm.hub.id` | `string` | supported |
| `llm.hub.limits` | `object` | supported |
| `llm.hub.limits.max_conns` | `integer` | supported |
| `llm.hub.limits.max_inflight` | `integer` | supported |
| `llm.hub.limits.ping_interval` | `duration` | supported |
| `llm.hub.limits.retry_interval` | `duration` | supported |
| `llm.hub.limits.slow_timeout` | `duration` | supported |
| `llm.hub.limits.stream_idle` | `duration` | supported |
| `llm.hub.listen` | `object` | supported |
| `llm.hub.listen.address` | `string, required` | supported |
| `llm.hub.listen.cert` | `string` | supported |
| `llm.hub.listen.cert_file` | `string` | supported |
| `llm.hub.listen.hub_password` | `string` | supported |
| `llm.hub.listen.key` | `string` | supported |
| `llm.hub.listen.key_file` | `string` | supported |
| `llm.hub.listen.node_password` | `string` | supported |
| `llm.hub.listen.password` | `string` | supported |
| `llm.hub.listen.proxy_password` | `string` | supported |
| `llm.hub.listen.self_signed` | `boolean` | supported |
| `llm.hub.max_nodes` | `integer` | supported |
| `llm.hub.name` | `string` | supported |
| `llm.hub.peers` | `array<object>` | supported |
| `llm.hub.peers[].address` | `string, required` | supported |
| `llm.hub.peers[].ca_file` | `string` | supported |
| `llm.hub.peers[].password` | `string` | supported |
| `llm.hub.peers[].pin` | `string` | supported |
| `llm.hub.peers[].role` | `enum(hub\|node\|proxy), required` | supported |
| `llm.hub.peers[].server_name` | `string` | supported |
| `llm.hub.peers[].token` | `string` | supported |
| `llm.hub.tokens` | `array<object>` | supported |
| `llm.hub.tokens[].created_at` | `integer` | supported |
| `llm.hub.tokens[].expires_at` | `integer` | supported |
| `llm.hub.tokens[].hash` | `string, required` | supported |
| `llm.hub.tokens[].id` | `string, required` | supported |
| `llm.hub.tokens[].members` | `array<string>` | supported |
| `llm.hub.tokens[].name` | `string, required` | supported |
| `llm.hub.tokens[].note` | `string` | supported |
| `llm.node` | `object` | supported |
| `llm.node.access` | `object` | supported |
| `llm.node.access.allow` | `array<string>` | supported |
| `llm.node.access.allow_files` | `array<string>` | supported |
| `llm.node.access.deny` | `array<string>` | supported |
| `llm.node.access.deny_files` | `array<string>` | supported |
| `llm.node.console` | `object` | supported |
| `llm.node.console.access` | `object` | supported |
| `llm.node.console.access.allow` | `array<string>` | supported |
| `llm.node.console.access.allow_files` | `array<string>` | supported |
| `llm.node.console.access.deny` | `array<string>` | supported |
| `llm.node.console.access.deny_files` | `array<string>` | supported |
| `llm.node.console.address` | `string, required` | supported |
| `llm.node.console.cert_file` | `string` | supported |
| `llm.node.console.key_file` | `string` | supported |
| `llm.node.console.max_conns` | `integer` | supported |
| `llm.node.console.max_sessions` | `integer` | supported |
| `llm.node.console.mptcp` | `boolean` | not supported yet |
| `llm.node.console.session_lifetime` | `duration` | supported |
| `llm.node.console.session_timeout` | `duration` | supported |
| `llm.node.console.users` | `array<object>, required` | supported |
| `llm.node.console.users[].password_hash` | `string, required` | supported |
| `llm.node.console.users[].username` | `string, required` | supported |
| `llm.node.description` | `string` | supported |
| `llm.node.id` | `string` | supported |
| `llm.node.limits` | `object` | supported |
| `llm.node.limits.max_conns` | `integer` | supported |
| `llm.node.limits.max_inflight` | `integer` | supported |
| `llm.node.limits.ping_interval` | `duration` | supported |
| `llm.node.limits.retry_interval` | `duration` | supported |
| `llm.node.limits.slow_timeout` | `duration` | supported |
| `llm.node.limits.stream_idle` | `duration` | supported |
| `llm.node.listen` | `object` | supported |
| `llm.node.listen.address` | `string, required` | supported |
| `llm.node.listen.cert` | `string` | supported |
| `llm.node.listen.cert_file` | `string` | supported |
| `llm.node.listen.hub_password` | `string` | supported |
| `llm.node.listen.key` | `string` | supported |
| `llm.node.listen.key_file` | `string` | supported |
| `llm.node.listen.node_password` | `string` | supported |
| `llm.node.listen.password` | `string` | supported |
| `llm.node.listen.proxy_password` | `string` | supported |
| `llm.node.listen.self_signed` | `boolean` | supported |
| `llm.node.models` | `array<object>` | supported |
| `llm.node.models[].api` | `enum(openai\|anthropic\|gemini\|azure_openai\|ollama\|cohere), required` | supported |
| `llm.node.models[].api_key` | `string` | supported |
| `llm.node.models[].api_version` | `string` | supported |
| `llm.node.models[].base_url` | `string, required` | supported |
| `llm.node.models[].description` | `string` | supported |
| `llm.node.models[].disabled` | `boolean` | supported |
| `llm.node.models[].max_inflight` | `integer` | supported |
| `llm.node.models[].name` | `string, required` | supported |
| `llm.node.models[].upstream` | `string` | supported |
| `llm.node.name` | `string` | supported |
| `llm.node.peers` | `array<object>` | supported |
| `llm.node.peers[].address` | `string, required` | supported |
| `llm.node.peers[].ca_file` | `string` | supported |
| `llm.node.peers[].password` | `string` | supported |
| `llm.node.peers[].pin` | `string` | supported |
| `llm.node.peers[].role` | `enum(hub\|node\|proxy), required` | supported |
| `llm.node.peers[].server_name` | `string` | supported |
| `llm.node.peers[].token` | `string` | supported |
| `llm.node.proxy` | `object` | supported |
| `llm.node.proxy.connect_timeout` | `duration` | supported |
| `llm.node.proxy.first_byte_timeout` | `duration` | supported |
| `llm.node.proxy.idle_timeout` | `duration` | supported |
| `llm.node.proxy.max_body_bytes` | `integer` | supported |
| `llm.node.proxy.max_inflight` | `integer` | supported |
| `llm.node.proxy.timeout` | `duration` | supported |
| `llm.node.settings` | `object` | supported |
| `llm.node.settings.allow_hub_manage` | `boolean` | supported |
| `llm.node.settings.gpu_data_dir` | `string` | supported |
| `llm.node.settings.gpu_sudo` | `boolean` | supported |
| `llm.node.settings.job_timeout` | `duration` | supported |
| `llm.node.settings.refresh_interval` | `duration` | supported |
| `llm.node.tokens` | `array<object>` | supported |
| `llm.node.tokens[].created_at` | `integer` | supported |
| `llm.node.tokens[].expires_at` | `integer` | supported |
| `llm.node.tokens[].hash` | `string, required` | supported |
| `llm.node.tokens[].id` | `string, required` | supported |
| `llm.node.tokens[].members` | `array<string>` | supported |
| `llm.node.tokens[].name` | `string, required` | supported |
| `llm.node.tokens[].note` | `string` | supported |
| `llm.proxy` | `object` | supported |
| `llm.proxy.access` | `object` | supported |
| `llm.proxy.access.allow` | `array<string>` | supported |
| `llm.proxy.access.allow_files` | `array<string>` | supported |
| `llm.proxy.access.deny` | `array<string>` | supported |
| `llm.proxy.access.deny_files` | `array<string>` | supported |
| `llm.proxy.api` | `object` | supported |
| `llm.proxy.api.address` | `string, required` | supported |
| `llm.proxy.api.cert_file` | `string` | supported |
| `llm.proxy.api.first_byte_timeout` | `duration` | supported |
| `llm.proxy.api.key_file` | `string` | supported |
| `llm.proxy.api.max_body_bytes` | `integer` | supported |
| `llm.proxy.api.max_conns` | `integer` | supported |
| `llm.proxy.api.max_inflight` | `integer` | supported |
| `llm.proxy.api.timeout` | `duration` | supported |
| `llm.proxy.api_keys` | `array<object>` | supported |
| `llm.proxy.api_keys[].created_at` | `integer` | supported |
| `llm.proxy.api_keys[].hash` | `string, required` | supported |
| `llm.proxy.api_keys[].id` | `string, required` | supported |
| `llm.proxy.api_keys[].name` | `string, required` | supported |
| `llm.proxy.api_keys[].prefix` | `string` | supported |
| `llm.proxy.console` | `object` | supported |
| `llm.proxy.console.access` | `object` | supported |
| `llm.proxy.console.access.allow` | `array<string>` | supported |
| `llm.proxy.console.access.allow_files` | `array<string>` | supported |
| `llm.proxy.console.access.deny` | `array<string>` | supported |
| `llm.proxy.console.access.deny_files` | `array<string>` | supported |
| `llm.proxy.console.address` | `string, required` | supported |
| `llm.proxy.console.cert_file` | `string` | supported |
| `llm.proxy.console.key_file` | `string` | supported |
| `llm.proxy.console.max_conns` | `integer` | supported |
| `llm.proxy.console.max_sessions` | `integer` | supported |
| `llm.proxy.console.mptcp` | `boolean` | not supported yet |
| `llm.proxy.console.session_lifetime` | `duration` | supported |
| `llm.proxy.console.session_timeout` | `duration` | supported |
| `llm.proxy.console.users` | `array<object>, required` | supported |
| `llm.proxy.console.users[].password_hash` | `string, required` | supported |
| `llm.proxy.console.users[].username` | `string, required` | supported |
| `llm.proxy.id` | `string` | supported |
| `llm.proxy.limits` | `object` | supported |
| `llm.proxy.limits.max_conns` | `integer` | supported |
| `llm.proxy.limits.max_inflight` | `integer` | supported |
| `llm.proxy.limits.ping_interval` | `duration` | supported |
| `llm.proxy.limits.retry_interval` | `duration` | supported |
| `llm.proxy.limits.slow_timeout` | `duration` | supported |
| `llm.proxy.limits.stream_idle` | `duration` | supported |
| `llm.proxy.listen` | `object` | supported |
| `llm.proxy.listen.address` | `string, required` | supported |
| `llm.proxy.listen.cert` | `string` | supported |
| `llm.proxy.listen.cert_file` | `string` | supported |
| `llm.proxy.listen.hub_password` | `string` | supported |
| `llm.proxy.listen.key` | `string` | supported |
| `llm.proxy.listen.key_file` | `string` | supported |
| `llm.proxy.listen.node_password` | `string` | supported |
| `llm.proxy.listen.password` | `string` | supported |
| `llm.proxy.listen.proxy_password` | `string` | supported |
| `llm.proxy.listen.self_signed` | `boolean` | supported |
| `llm.proxy.name` | `string` | supported |
| `llm.proxy.peers` | `array<object>` | supported |
| `llm.proxy.peers[].address` | `string, required` | supported |
| `llm.proxy.peers[].ca_file` | `string` | supported |
| `llm.proxy.peers[].password` | `string` | supported |
| `llm.proxy.peers[].pin` | `string` | supported |
| `llm.proxy.peers[].role` | `enum(hub\|node\|proxy), required` | supported |
| `llm.proxy.peers[].server_name` | `string` | supported |
| `llm.proxy.peers[].token` | `string` | supported |
| `llm.proxy.tokens` | `array<object>` | supported |
| `llm.proxy.tokens[].created_at` | `integer` | supported |
| `llm.proxy.tokens[].expires_at` | `integer` | supported |
| `llm.proxy.tokens[].hash` | `string, required` | supported |
| `llm.proxy.tokens[].id` | `string, required` | supported |
| `llm.proxy.tokens[].members` | `array<string>` | supported |
| `llm.proxy.tokens[].name` | `string, required` | supported |
| `llm.proxy.tokens[].note` | `string` | supported |

<a id="fields-log"></a>

#### `log`

| Field | Type | Status |
| --- | --- | --- |
| `log` | `object` | supported |
| `log.format` | `enum(line\|text\|json)` | supported |
| `log.level` | `enum(debug\|info\|warn\|error)` | supported |
| `log.levels` | `map<string>` | supported |
| `log.loggers` | `map<object>` | supported |
| `log.loggers.*.format` | `enum(line\|text\|json)` | supported |
| `log.loggers.*.level` | `enum(debug\|info\|warn\|error)` | supported |
| `log.loggers.*.output` | `string` | supported |
| `log.loggers.*.rotate` | `object` | supported |
| `log.loggers.*.rotate.compress` | `boolean` | supported |
| `log.loggers.*.rotate.interval` | `duration` | supported |
| `log.loggers.*.rotate.max_age` | `duration` | supported |
| `log.loggers.*.rotate.max_bytes` | `integer` | supported |
| `log.loggers.*.rotate.max_files` | `integer` | supported |
| `log.loggers.*.rotate.max_total_bytes` | `integer` | supported |
| `log.output` | `string` | supported |
| `log.rotate` | `object` | supported |
| `log.rotate.compress` | `boolean` | supported |
| `log.rotate.interval` | `duration` | supported |
| `log.rotate.max_age` | `duration` | supported |
| `log.rotate.max_bytes` | `integer` | supported |
| `log.rotate.max_files` | `integer` | supported |
| `log.rotate.max_total_bytes` | `integer` | supported |
| `log.time_zone` | `string` | supported |

<a id="fields-media-auth"></a>

#### `media_auth`

| Field | Type | Status |
| --- | --- | --- |
| `media_auth` | `array<object>` | not supported yet |
| `media_auth[].keys` | `array<object>` | not supported yet |
| `media_auth[].keys[].key` | `string, required` | not supported yet |
| `media_auth[].keys[].secret` | `string` | not supported yet |
| `media_auth[].keys[].secret_file` | `string` | not supported yet |
| `media_auth[].name` | `string, required` | not supported yet |
| `media_auth[].public` | `array<enum(publish\|play)>` | not supported yet |
| `media_auth[].secret` | `string` | not supported yet |
| `media_auth[].secret_file` | `string` | not supported yet |
| `media_auth[].token_ttl` | `duration` | not supported yet |
| `media_auth[].tokens` | `array<object>` | not supported yet |
| `media_auth[].tokens[].name` | `string` | not supported yet |
| `media_auth[].tokens[].play` | `array<string>` | not supported yet |
| `media_auth[].tokens[].publish` | `array<string>` | not supported yet |
| `media_auth[].tokens[].token` | `string` | not supported yet |
| `media_auth[].tokens[].token_sha256` | `string` | not supported yet |
| `media_auth[].url_signing` | `object` | not supported yet |
| `media_auth[].url_signing.bind_address` | `enum(ip\|subnet)` | not supported yet |
| `media_auth[].url_signing.cookie` | `boolean` | not supported yet |
| `media_auth[].url_signing.max_sessions` | `integer` | not supported yet |
| `media_auth[].url_signing.max_tracked_sessions` | `integer` | not supported yet |
| `media_auth[].url_signing.on_limit` | `enum(refuse\|end_oldest)` | not supported yet |
| `media_auth[].url_signing.origins` | `array<string>` | not supported yet |
| `media_auth[].url_signing.play_grace` | `duration` | not supported yet |
| `media_auth[].url_signing.referers` | `array<string>` | not supported yet |
| `media_auth[].url_signing.session_idle` | `duration` | not supported yet |
| `media_auth[].url_signing.trusted_proxies` | `array<string>` | not supported yet |
| `media_auth[].url_signing.ttl` | `duration` | not supported yet |
| `media_auth[].users` | `array<object>` | not supported yet |
| `media_auth[].users[].name` | `string, required` | not supported yet |
| `media_auth[].users[].password` | `string` | not supported yet |
| `media_auth[].users[].password_hash` | `string` | not supported yet |
| `media_auth[].users[].play` | `array<string>` | not supported yet |
| `media_auth[].users[].publish` | `array<string>` | not supported yet |

<a id="fields-media-library"></a>

#### `media_library`

| Field | Type | Status |
| --- | --- | --- |
| `media_library` | `array<object>` | not supported yet |
| `media_library[].exclude` | `array<string>` | not supported yet |
| `media_library[].file_queue` | `integer` | not supported yet |
| `media_library[].file_timeout` | `duration` | not supported yet |
| `media_library[].file_workers` | `integer` | not supported yet |
| `media_library[].files` | `array<string>` | not supported yet |
| `media_library[].follow_symlinks` | `boolean` | not supported yet |
| `media_library[].max_catalog_bytes` | `integer` | not supported yet |
| `media_library[].max_files` | `integer` | not supported yet |
| `media_library[].max_items` | `integer` | not supported yet |
| `media_library[].max_probes` | `integer` | not supported yet |
| `media_library[].max_sidecar_bytes` | `integer` | not supported yet |
| `media_library[].max_subtitle_bytes` | `integer` | not supported yet |
| `media_library[].max_table_bytes` | `integer` | not supported yet |
| `media_library[].max_upload_bytes` | `integer` | not supported yet |
| `media_library[].max_uploads` | `integer` | not supported yet |
| `media_library[].min_free_bytes` | `integer` | not supported yet |
| `media_library[].name` | `string, required` | not supported yet |
| `media_library[].roots` | `array<string>, required` | not supported yet |
| `media_library[].scan_interval` | `duration` | not supported yet |
| `media_library[].secret` | `string` | not supported yet |
| `media_library[].secret_file` | `string` | not supported yet |
| `media_library[].upload_dir` | `string` | not supported yet |
| `media_library[].upload_expiry` | `duration` | not supported yet |

<a id="fields-metrics"></a>

#### `metrics`

| Field | Type | Status |
| --- | --- | --- |
| `metrics` | `object` | supported |

<a id="fields-pprof"></a>

#### `pprof`

| Field | Type | Status |
| --- | --- | --- |
| `pprof` | `object` | not supported yet |
| `pprof.block_profile_rate` | `integer` | not supported yet |
| `pprof.max_duration` | `duration` | not supported yet |
| `pprof.mutex_profile_fraction` | `integer` | not supported yet |

<a id="fields-providers"></a>

#### `providers`

| Field | Type | Status |
| --- | --- | --- |
| `providers` | `array<object>` | supported |
| `providers[].consul` | `object` | supported |
| `providers[].consul.ca_file` | `string` | supported |
| `providers[].consul.cert_file` | `string` | supported |
| `providers[].consul.datacenter` | `string` | supported |
| `providers[].consul.endpoint` | `string` | supported |
| `providers[].consul.expose_all` | `boolean` | supported |
| `providers[].consul.key_file` | `string` | supported |
| `providers[].consul.max_services` | `integer` | supported |
| `providers[].consul.refresh_interval` | `duration` | supported |
| `providers[].consul.request_timeout` | `duration` | supported |
| `providers[].consul.tag_prefix` | `string` | supported |
| `providers[].consul.token_file` | `string` | supported |
| `providers[].consul_kv` | `object` | supported |
| `providers[].consul_kv.ca_file` | `string` | supported |
| `providers[].consul_kv.cert_file` | `string` | supported |
| `providers[].consul_kv.datacenter` | `string` | supported |
| `providers[].consul_kv.endpoint` | `string` | supported |
| `providers[].consul_kv.key_file` | `string` | supported |
| `providers[].consul_kv.max_keys` | `integer` | supported |
| `providers[].consul_kv.prefix` | `string` | supported |
| `providers[].consul_kv.refresh_interval` | `duration` | supported |
| `providers[].consul_kv.request_timeout` | `duration` | supported |
| `providers[].consul_kv.token_file` | `string` | supported |
| `providers[].docker` | `object` | supported |
| `providers[].docker.endpoint` | `string` | supported |
| `providers[].docker.expose_all` | `boolean` | supported |
| `providers[].docker.label_prefix` | `string` | supported |
| `providers[].docker.max_containers` | `integer` | supported |
| `providers[].docker.network` | `string` | supported |
| `providers[].docker.refresh_interval` | `duration` | supported |
| `providers[].docker.request_timeout` | `duration` | supported |
| `providers[].docker.use_published_ports` | `boolean` | supported |
| `providers[].etcd` | `object` | supported |
| `providers[].etcd.ca_file` | `string` | supported |
| `providers[].etcd.cert_file` | `string` | supported |
| `providers[].etcd.endpoints` | `array<string>` | supported |
| `providers[].etcd.key_file` | `string` | supported |
| `providers[].etcd.max_keys` | `integer` | supported |
| `providers[].etcd.password_file` | `string` | supported |
| `providers[].etcd.prefix` | `string` | supported |
| `providers[].etcd.refresh_interval` | `duration` | supported |
| `providers[].etcd.request_timeout` | `duration` | supported |
| `providers[].etcd.username` | `string` | supported |
| `providers[].kubernetes` | `object` | supported |
| `providers[].kubernetes.addresses` | `array<string>` | supported |
| `providers[].kubernetes.allow_external_name_services` | `boolean` | not supported yet |
| `providers[].kubernetes.backends` | `enum(endpoints\|cluster_ip)` | supported |
| `providers[].kubernetes.certificates_dir` | `string` | supported |
| `providers[].kubernetes.context` | `string` | supported |
| `providers[].kubernetes.controller_name` | `string` | supported |
| `providers[].kubernetes.ingress` | `object` | supported |
| `providers[].kubernetes.ingress.http_port` | `integer` | supported |
| `providers[].kubernetes.ingress.https_port` | `integer` | supported |
| `providers[].kubernetes.kubeconfig` | `string` | supported |
| `providers[].kubernetes.listen_host` | `string` | supported |
| `providers[].kubernetes.max_objects` | `integer` | supported |
| `providers[].kubernetes.namespaces` | `array<string>` | supported |
| `providers[].kubernetes.request_timeout` | `duration` | supported |
| `providers[].name` | `string` | supported |

<a id="fields-routes"></a>

#### `routes`

| Field | Type | Status |
| --- | --- | --- |
| `routes` | `array<object>` | supported |
| `routes[].access` | `object` | supported |
| `routes[].access.allow` | `array<string>` | supported |
| `routes[].access.allow_files` | `array<string>` | supported |
| `routes[].access.deny` | `array<string>` | supported |
| `routes[].access.deny_files` | `array<string>` | supported |
| `routes[].handler` | `object, required` | supported |
| `routes[].handler.module` | `enum(bt_publish\|bt_stream\|bt_tracker\|fastcgi\|forward_proxy\|hls\|httpflv\|media_pages\|media_tokens\|proxy\|respond\|static\|stream_whep\|stream_whip\|sync_ingest\|vod\|webrtc_proxy\|websockify\|whep\|whip), required` | supported |
| `routes[].handler.params` | `object` | supported |
| `routes[].listeners` | `array<string>` | supported |
| `routes[].match` | `object` | supported |
| `routes[].match.expr` | `string` | supported |
| `routes[].match.header` | `map<array<string>>` | supported |
| `routes[].match.host` | `array<string>` | supported |
| `routes[].match.method` | `array<string>` | supported |
| `routes[].match.path` | `array<string>` | supported |
| `routes[].match.path_regexp` | `array<string>` | supported |
| `routes[].match.query` | `map<array<string>>` | supported |
| `routes[].match.remote_ip` | `array<string>` | supported |
| `routes[].middleware` | `array<object>` | supported |
| `routes[].middleware[].module` | `enum(basic_auth\|body_limit\|cache\|circuit_breaker\|client_cert\|compress\|concurrency_limit\|forward_auth\|headers\|jwt\|rate_limit\|redirect\|rewrite\|wasm), required` | supported |
| `routes[].middleware[].params` | `object` | supported |
| `routes[].name` | `string` | supported |
| `routes[].priority` | `integer` | supported |

<a id="fields-schema-version"></a>

#### `schema_version`

| Field | Type | Status |
| --- | --- | --- |
| `schema_version` | `integer` | supported |

<a id="fields-services"></a>

#### `services`

| Field | Type | Status |
| --- | --- | --- |
| `services` | `array<object>` | supported |
| `services[].access` | `object` | supported |
| `services[].access.allow` | `array<string>` | supported |
| `services[].access.allow_files` | `array<string>` | supported |
| `services[].access.deny` | `array<string>` | supported |
| `services[].access.deny_files` | `array<string>` | supported |
| `services[].idle_timeout` | `duration` | supported |
| `services[].listeners` | `array<string>, required` | supported |
| `services[].max_conns` | `integer` | supported |
| `services[].name` | `string, required` | supported |
| `services[].proxy_protocol` | `enum(v1\|v2)` | supported |
| `services[].server_names` | `array<string>` | supported |
| `services[].upstream` | `string, required` | supported |

<a id="fields-streaming"></a>

#### `streaming`

| Field | Type | Status |
| --- | --- | --- |
| `streaming` | `object` | not supported yet |
| `streaming.access` | `object` | not supported yet |
| `streaming.access.allow` | `array<string>` | not supported yet |
| `streaming.access.allow_files` | `array<string>` | not supported yet |
| `streaming.access.deny` | `array<string>` | not supported yet |
| `streaming.access.deny_files` | `array<string>` | not supported yet |
| `streaming.channels` | `array<object>` | not supported yet |
| `streaming.channels[].encrypt` | `boolean` | not supported yet |
| `streaming.channels[].items` | `array<string>` | not supported yet |
| `streaming.channels[].key_rotation` | `integer` | not supported yet |
| `streaming.channels[].library` | `string, required` | not supported yet |
| `streaming.channels[].loop` | `boolean` | not supported yet |
| `streaming.channels[].max_buffer_bytes` | `integer` | not supported yet |
| `streaming.channels[].name` | `string, required` | not supported yet |
| `streaming.channels[].order` | `enum(sequence\|shuffle)` | not supported yet |
| `streaming.channels[].push` | `array<object>` | not supported yet |
| `streaming.channels[].push[].connect_timeout` | `duration` | not supported yet |
| `streaming.channels[].push[].name` | `string` | not supported yet |
| `streaming.channels[].push[].retry_max` | `duration` | not supported yet |
| `streaming.channels[].push[].retry_min` | `duration` | not supported yet |
| `streaming.channels[].push[].stream` | `string` | not supported yet |
| `streaming.channels[].push[].url` | `string, required` | not supported yet |
| `streaming.channels[].read_ahead` | `duration` | not supported yet |
| `streaming.channels[].rendition` | `string` | not supported yet |
| `streaming.channels[].schedule` | `array<object>` | not supported yet |
| `streaming.channels[].schedule[].at` | `string, required` | not supported yet |
| `streaming.channels[].schedule[].item` | `string, required` | not supported yet |
| `streaming.channels[].time_zone` | `string` | not supported yet |
| `streaming.gop_cache_bytes` | `integer` | not supported yet |
| `streaming.max_players` | `integer` | not supported yet |
| `streaming.max_streams` | `integer` | not supported yet |
| `streaming.push` | `array<object>` | not supported yet |
| `streaming.push[].connect_timeout` | `duration` | not supported yet |
| `streaming.push[].name` | `string` | not supported yet |
| `streaming.push[].retry_max` | `duration` | not supported yet |
| `streaming.push[].retry_min` | `duration` | not supported yet |
| `streaming.push[].stream` | `string` | not supported yet |
| `streaming.push[].url` | `string, required` | not supported yet |
| `streaming.rtmp` | `object` | not supported yet |
| `streaming.rtmp.auth` | `string` | not supported yet |
| `streaming.rtmp.handshake_timeout` | `duration` | not supported yet |
| `streaming.rtmp.idle_timeout` | `duration` | not supported yet |
| `streaming.rtmp.listen` | `string, required` | not supported yet |
| `streaming.rtmp.max_conns` | `integer` | not supported yet |
| `streaming.rtmp.max_message_bytes` | `integer` | not supported yet |
| `streaming.rtmp.mptcp` | `boolean` | not supported yet |
| `streaming.rtmp.play_wait` | `duration` | not supported yet |
| `streaming.rtsp` | `object` | not supported yet |
| `streaming.rtsp.auth` | `string` | not supported yet |
| `streaming.rtsp.auth_methods` | `array<enum(digest\|basic)>` | not supported yet |
| `streaming.rtsp.idle_timeout` | `duration` | not supported yet |
| `streaming.rtsp.listen` | `string, required` | not supported yet |
| `streaming.rtsp.max_conns` | `integer` | not supported yet |
| `streaming.rtsp.mptcp` | `boolean` | not supported yet |
| `streaming.rtsp.play_wait` | `duration` | not supported yet |
| `streaming.rtsp.realm` | `string` | not supported yet |
| `streaming.rtsp.session_timeout` | `duration` | not supported yet |
| `streaming.srt` | `object` | not supported yet |
| `streaming.srt.auth` | `string` | not supported yet |
| `streaming.srt.latency` | `duration` | not supported yet |
| `streaming.srt.listen` | `string, required` | not supported yet |
| `streaming.srt.max_conns` | `integer` | not supported yet |
| `streaming.srt.passphrase` | `string` | not supported yet |
| `streaming.srt.peer_idle_timeout` | `duration` | not supported yet |
| `streaming.srt.play_wait` | `duration` | not supported yet |
| `streaming.webrtc` | `object` | not supported yet |
| `streaming.webrtc.connect_timeout` | `duration` | not supported yet |
| `streaming.webrtc.ice_servers` | `array<object>` | not supported yet |
| `streaming.webrtc.ice_servers[].credential` | `string` | not supported yet |
| `streaming.webrtc.ice_servers[].urls` | `array<string>, required` | not supported yet |
| `streaming.webrtc.ice_servers[].username` | `string` | not supported yet |
| `streaming.webrtc.listen` | `string, required` | not supported yet |
| `streaming.webrtc.max_offer_bytes` | `integer` | not supported yet |
| `streaming.webrtc.max_sessions` | `integer` | not supported yet |
| `streaming.webrtc.play_wait` | `duration` | not supported yet |
| `streaming.webrtc.public_ips` | `array<string>` | not supported yet |

<a id="fields-sync"></a>

#### `sync`

| Field | Type | Status |
| --- | --- | --- |
| `sync` | `object` | not supported yet |
| `sync.pipelines` | `array<object>` | not supported yet |
| `sync.pipelines[].checkpoint_interval` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.batch` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.batch.max_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.batch.max_records` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.batch.max_wait` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.name` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink` | `object, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink.module` | `enum(clickhouse\|dead_letters\|doris\|elasticsearch\|file\|http\|influxdb\|kafka\|loki\|memory\|milvus\|mongo\|mqtt\|mssql\|mysql\|nats\|otlp_logs\|postgres\|pulsar\|qdrant\|rabbitmq\|redis\|s3\|stdout\|tdengine\|wasm), required` | not supported yet |
| `sync.pipelines[].dead_letter.sink.params` | `object` | not supported yet |
| `sync.pipelines[].job` | `object` | not supported yet |
| `sync.pipelines[].job.max_parallel` | `integer` | not supported yet |
| `sync.pipelines[].job.schedule` | `string` | not supported yet |
| `sync.pipelines[].job.time_zone` | `string` | not supported yet |
| `sync.pipelines[].mode` | `enum(stream\|once)` | not supported yet |
| `sync.pipelines[].name` | `string, required` | not supported yet |
| `sync.pipelines[].on_error` | `enum(stop\|skip\|dead_letter)` | not supported yet |
| `sync.pipelines[].queue` | `object` | not supported yet |
| `sync.pipelines[].queue.max_bytes` | `integer` | not supported yet |
| `sync.pipelines[].queue.max_records` | `integer` | not supported yet |
| `sync.pipelines[].retry` | `object` | not supported yet |
| `sync.pipelines[].retry.backoff` | `duration` | not supported yet |
| `sync.pipelines[].retry.max_attempts` | `integer` | not supported yet |
| `sync.pipelines[].retry.max_backoff` | `duration` | not supported yet |
| `sync.pipelines[].sinks` | `array<object>, required` | not supported yet |
| `sync.pipelines[].sinks[].batch` | `object` | not supported yet |
| `sync.pipelines[].sinks[].batch.max_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].batch.max_records` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].batch.max_wait` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].name` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink` | `object, required` | not supported yet |
| `sync.pipelines[].sinks[].sink.module` | `enum(clickhouse\|dead_letters\|doris\|elasticsearch\|file\|http\|influxdb\|kafka\|loki\|memory\|milvus\|mongo\|mqtt\|mssql\|mysql\|nats\|otlp_logs\|postgres\|pulsar\|qdrant\|rabbitmq\|redis\|s3\|stdout\|tdengine\|wasm), required` | not supported yet |
| `sync.pipelines[].sinks[].sink.params` | `object` | not supported yet |
| `sync.pipelines[].source` | `object, required` | not supported yet |
| `sync.pipelines[].source.module` | `enum(clickhouse\|doris\|elasticsearch\|file\|generate\|http\|kafka\|mongo\|mqtt\|mssql\|mysql\|nats\|postgres\|pulsar\|rabbitmq\|redis\|s3\|tidb\|wasm), required` | not supported yet |
| `sync.pipelines[].source.params` | `object` | not supported yet |
| `sync.pipelines[].transforms` | `array<object>` | not supported yet |
| `sync.pipelines[].transforms[].module` | `enum(cast\|embed\|envelope\|fields\|filter\|key\|mask\|route\|wasm), required` | not supported yet |
| `sync.pipelines[].transforms[].params` | `object` | not supported yet |
| `sync.state_dir` | `string, required` | not supported yet |

<a id="fields-sync-codecs-canal-json"></a>

#### `sync.codecs/canal_json`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<canal_json>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<canal_json>` | `module` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<canal_json>.params` | `object` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<canal_json>.params.time_zone` | `string` | not supported yet |

<a id="fields-sync-codecs-csv"></a>

#### `sync.codecs/csv`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<csv>.params.comma` | `string` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<csv>` | `module` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<csv>.params` | `object` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<csv>.params.columns` | `array<string>` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<csv>.params.comma` | `string` | not supported yet |

<a id="fields-sync-codecs-debezium"></a>

#### `sync.codecs/debezium`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<debezium>.params` | `object` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<debezium>` | `module` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<debezium>.params` | `object` | not supported yet |

<a id="fields-sync-codecs-json"></a>

#### `sync.codecs/json`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<json>` | `module` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<json>.params` | `object` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<json>.params.form` | `enum(plain\|record)` | not supported yet |

<a id="fields-sync-codecs-line"></a>

#### `sync.codecs/line`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<line>.params.field` | `string` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<line>` | `module` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<line>.params` | `object` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<line>.params.field` | `string` | not supported yet |

<a id="fields-sync-codecs-raw"></a>

#### `sync.codecs/raw`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<raw>.params.field` | `string` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<raw>` | `module` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<raw>.params` | `object` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<raw>.params.field` | `string` | not supported yet |

<a id="fields-sync-codecs-ticdc-simple"></a>

#### `sync.codecs/ticdc_simple`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<ticdc_simple>` | `module` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<ticdc_simple>.params` | `object` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<ticdc_simple>.params.time_zone` | `string` | not supported yet |

<a id="fields-sync-codecs-wasm"></a>

#### `sync.codecs/wasm`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].source<file>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].source<http>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<wasm>` | `module` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec<wasm>.params.timeout` | `duration` | not supported yet |

<a id="fields-sync-sinks-clickhouse"></a>

#### `sync.sinks/clickhouse`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<clickhouse>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.database` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.deleted_column` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.deletes` | `enum(flag\|delete\|ignore)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.max_request_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.plain` | `boolean` | not supported yet |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.settings` | `map<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.table` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.urls` | `array<string>, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.username` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.version_column` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.version_field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<clickhouse>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<clickhouse>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.database` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.deleted_column` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.deletes` | `enum(flag\|delete\|ignore)` | not supported yet |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.max_request_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.plain` | `boolean` | not supported yet |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.settings` | `map<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.table` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.urls` | `array<string>, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.username` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.version_column` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.version_field` | `string` | not supported yet |

<a id="fields-sync-sinks-dead-letters"></a>

#### `sync.sinks/dead_letters`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<dead_letters>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<dead_letters>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<dead_letters>.params.max_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<dead_letters>.params.max_records` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<dead_letters>.params.path` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<dead_letters>.params.when_full` | `enum(refuse\|drop_oldest)` | not supported yet |
| `sync.pipelines[].sinks[].sink<dead_letters>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<dead_letters>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<dead_letters>.params.max_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<dead_letters>.params.max_records` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<dead_letters>.params.path` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<dead_letters>.params.when_full` | `enum(refuse\|drop_oldest)` | not supported yet |

<a id="fields-sync-sinks-doris"></a>

#### `sync.sinks/doris`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<doris>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<doris>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<doris>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<doris>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<doris>.params.database` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<doris>.params.deletes` | `enum(sign\|ignore)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<doris>.params.flavor` | `enum(doris\|starrocks)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<doris>.params.format` | `enum(json\|csv)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<doris>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<doris>.params.label_prefix` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<doris>.params.max_request_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<doris>.params.partial_columns` | `boolean` | not supported yet |
| `sync.pipelines[].dead_letter.sink<doris>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<doris>.params.properties` | `map<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<doris>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<doris>.params.table` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<doris>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<doris>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<doris>.params.urls` | `array<string>, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<doris>.params.username` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<doris>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<doris>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<doris>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<doris>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<doris>.params.database` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<doris>.params.deletes` | `enum(sign\|ignore)` | not supported yet |
| `sync.pipelines[].sinks[].sink<doris>.params.flavor` | `enum(doris\|starrocks)` | not supported yet |
| `sync.pipelines[].sinks[].sink<doris>.params.format` | `enum(json\|csv)` | not supported yet |
| `sync.pipelines[].sinks[].sink<doris>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<doris>.params.label_prefix` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<doris>.params.max_request_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<doris>.params.partial_columns` | `boolean` | not supported yet |
| `sync.pipelines[].sinks[].sink<doris>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<doris>.params.properties` | `map<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<doris>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<doris>.params.table` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<doris>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<doris>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<doris>.params.urls` | `array<string>, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<doris>.params.username` | `string, required` | not supported yet |

<a id="fields-sync-sinks-elasticsearch"></a>

#### `sync.sinks/elasticsearch`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<elasticsearch>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.action` | `enum(index\|create)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.api_key_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.index` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.max_request_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.pipeline` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.routing` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.urls` | `array<string>, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.username` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<elasticsearch>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.action` | `enum(index\|create)` | not supported yet |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.api_key_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.index` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.max_request_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.pipeline` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.routing` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.urls` | `array<string>, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.username` | `string` | not supported yet |

<a id="fields-sync-sinks-file"></a>

#### `sync.sinks/file`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<file>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.rotate` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.rotate.compress` | `boolean` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.rotate.interval` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.rotate.max_age` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.rotate.max_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.rotate.max_files` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.rotate.max_total_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.template` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<file>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.rotate` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.rotate.compress` | `boolean` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.rotate.interval` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.rotate.max_age` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.rotate.max_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.rotate.max_files` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.rotate.max_total_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.template` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<file>.params.time_zone` | `string` | not supported yet |

<a id="fields-sync-sinks-http"></a>

#### `sync.sinks/http`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<http>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.bearer_token_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.content_type` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.format` | `enum(ndjson\|json)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.headers` | `map<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.idempotency_header` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.max_request_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.method` | `enum(POST\|PUT)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.url` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<http>.params.username` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.bearer_token_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.content_type` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.format` | `enum(ndjson\|json)` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.headers` | `map<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.idempotency_header` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.max_request_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.method` | `enum(POST\|PUT)` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.url` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<http>.params.username` | `string` | not supported yet |

<a id="fields-sync-sinks-influxdb"></a>

#### `sync.sinks/influxdb`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<influxdb>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.api` | `enum(v2\|v3\|v1)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.bucket` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.database` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.deletes` | `enum(skip\|fail)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.fields` | `array<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.gzip` | `boolean` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.max_request_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.measurement` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.org` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.precision` | `enum(ns\|us\|ms\|s)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.retention_policy` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.tags` | `array<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.time_field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.token_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.url` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.username` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params.api` | `enum(v2\|v3\|v1)` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params.bucket` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params.database` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params.deletes` | `enum(skip\|fail)` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params.fields` | `array<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params.gzip` | `boolean` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params.max_request_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params.measurement` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params.org` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params.precision` | `enum(ns\|us\|ms\|s)` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params.retention_policy` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params.tags` | `array<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params.time_field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params.token_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params.url` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<influxdb>.params.username` | `string` | not supported yet |

<a id="fields-sync-sinks-kafka"></a>

#### `sync.sinks/kafka`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<kafka>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.acks` | `enum(all\|leader\|none)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.brokers` | `array<string>, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.client_id` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.compression` | `enum(none\|gzip\|snappy\|lz4\|zstd)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.compression_level` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.create_topics` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.create_topics.configs` | `map<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.create_topics.partitions` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.create_topics.replication_factor` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.deletes` | `enum(tombstone\|encode)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.max_batch_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.sasl` | `enum(PLAIN\|SCRAM-SHA-256\|SCRAM-SHA-512)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.topic` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<kafka>.params.username` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.acks` | `enum(all\|leader\|none)` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.brokers` | `array<string>, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.client_id` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.compression` | `enum(none\|gzip\|snappy\|lz4\|zstd)` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.compression_level` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.create_topics` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.create_topics.configs` | `map<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.create_topics.partitions` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.create_topics.replication_factor` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.deletes` | `enum(tombstone\|encode)` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.max_batch_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.sasl` | `enum(PLAIN\|SCRAM-SHA-256\|SCRAM-SHA-512)` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.topic` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<kafka>.params.username` | `string` | not supported yet |

<a id="fields-sync-sinks-loki"></a>

#### `sync.sinks/loki`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<loki>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.bearer_token_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.encoding` | `enum(protobuf\|json)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.labels` | `map<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.max_request_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.structured_metadata` | `map<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.tenant` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.tenant_field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.timestamp_field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.urls` | `array<string>, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<loki>.params.username` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.bearer_token_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.encoding` | `enum(protobuf\|json)` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.labels` | `map<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.max_request_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.structured_metadata` | `map<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.tenant` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.tenant_field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.timestamp_field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.urls` | `array<string>, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<loki>.params.username` | `string` | not supported yet |

<a id="fields-sync-sinks-memory"></a>

#### `sync.sinks/memory`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<memory>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<memory>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<memory>.params.max_records` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<memory>.params.store` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<memory>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<memory>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<memory>.params.max_records` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<memory>.params.store` | `string` | not supported yet |

<a id="fields-sync-sinks-milvus"></a>

#### `sync.sinks/milvus`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<milvus>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<milvus>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<milvus>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<milvus>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<milvus>.params.collection` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<milvus>.params.database` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<milvus>.params.fields` | `array<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<milvus>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<milvus>.params.max_request_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<milvus>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<milvus>.params.primary_key` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<milvus>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<milvus>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<milvus>.params.token_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<milvus>.params.url` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<milvus>.params.username` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<milvus>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<milvus>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<milvus>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<milvus>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<milvus>.params.collection` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<milvus>.params.database` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<milvus>.params.fields` | `array<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<milvus>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<milvus>.params.max_request_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<milvus>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<milvus>.params.primary_key` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<milvus>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<milvus>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<milvus>.params.token_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<milvus>.params.url` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<milvus>.params.username` | `string` | not supported yet |

<a id="fields-sync-sinks-mongo"></a>

#### `sync.sinks/mongo`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<mongo>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mongo>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mongo>.params.auth_source` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mongo>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mongo>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mongo>.params.collection` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mongo>.params.connect_timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mongo>.params.database` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mongo>.params.hosts` | `array<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mongo>.params.journal` | `boolean` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mongo>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mongo>.params.max_conns` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mongo>.params.max_request_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mongo>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mongo>.params.replica_set` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mongo>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mongo>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mongo>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mongo>.params.uri` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mongo>.params.username` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mongo>.params.write_concern` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mongo>.params.write_timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<mongo>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<mongo>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<mongo>.params.auth_source` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mongo>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mongo>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mongo>.params.collection` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mongo>.params.connect_timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<mongo>.params.database` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mongo>.params.hosts` | `array<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<mongo>.params.journal` | `boolean` | not supported yet |
| `sync.pipelines[].sinks[].sink<mongo>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mongo>.params.max_conns` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<mongo>.params.max_request_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<mongo>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mongo>.params.replica_set` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mongo>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mongo>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<mongo>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].sinks[].sink<mongo>.params.uri` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mongo>.params.username` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mongo>.params.write_concern` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mongo>.params.write_timeout` | `duration` | not supported yet |

<a id="fields-sync-sinks-mqtt"></a>

#### `sync.sinks/mqtt`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<mqtt>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.auth_method` | `enum(SCRAM-SHA-1\|SCRAM-SHA-256\|SCRAM-SHA-512)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.client_id` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.content_type` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.keep_alive` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.max_inflight` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.max_packet_size` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.message_expiry` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.qos` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.retain` | `boolean` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.servers` | `array<string>, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.topic` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.user_properties` | `map<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.username` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.version` | `enum(3.1.1\|5)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.will` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.will.content_type` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.will.delay` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.will.message_expiry` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.will.payload` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.will.qos` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.will.retain` | `boolean` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.will.topic` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.will.user_properties` | `map<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.auth_method` | `enum(SCRAM-SHA-1\|SCRAM-SHA-256\|SCRAM-SHA-512)` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.client_id` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.content_type` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.keep_alive` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.max_inflight` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.max_packet_size` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.message_expiry` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.qos` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.retain` | `boolean` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.servers` | `array<string>, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.topic` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.user_properties` | `map<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.username` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.version` | `enum(3.1.1\|5)` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.will` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.will.content_type` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.will.delay` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.will.message_expiry` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.will.payload` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.will.qos` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.will.retain` | `boolean` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.will.topic` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<mqtt>.params.will.user_properties` | `map<string>` | not supported yet |

<a id="fields-sync-sinks-mssql"></a>

#### `sync.sinks/mssql`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<mssql>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.address` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.authority` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.client_certificate_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.client_id` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.client_secret_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.kdc` | `array<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.keytab_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.krb5_conf` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.managed_identity` | `boolean` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.method` | `enum(entra\|kerberos\|ntlm), required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.realm` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.spn` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.tenant_id` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.token_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.token_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.bulk_rows` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.checkpoint` | `enum(file\|sink)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.checkpoint_table` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.database` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.ddl` | `enum(report\|stop)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.encrypt` | `enum(on\|strict\|off)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.identity_insert` | `boolean` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.table` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.transactions` | `enum(source\|batch)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.upsert` | `enum(merge\|update_insert)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mssql>.params.username` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.address` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.authority` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.client_certificate_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.client_id` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.client_secret_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.kdc` | `array<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.keytab_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.krb5_conf` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.managed_identity` | `boolean` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.method` | `enum(entra\|kerberos\|ntlm), required` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.realm` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.spn` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.tenant_id` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.token_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.token_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.bulk_rows` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.checkpoint` | `enum(file\|sink)` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.checkpoint_table` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.database` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.ddl` | `enum(report\|stop)` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.encrypt` | `enum(on\|strict\|off)` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.identity_insert` | `boolean` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.table` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.transactions` | `enum(source\|batch)` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.upsert` | `enum(merge\|update_insert)` | not supported yet |
| `sync.pipelines[].sinks[].sink<mssql>.params.username` | `string` | not supported yet |

<a id="fields-sync-sinks-mysql"></a>

#### `sync.sinks/mysql`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<mysql>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mysql>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mysql>.params.address` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mysql>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mysql>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mysql>.params.checkpoint` | `enum(file\|sink)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mysql>.params.checkpoint_table` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mysql>.params.database` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mysql>.params.ddl` | `enum(report\|stop)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mysql>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mysql>.params.max_transaction_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mysql>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mysql>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mysql>.params.server_public_key_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mysql>.params.table` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mysql>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mysql>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mysql>.params.tls_rsa` | `boolean` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mysql>.params.transactions` | `enum(source\|batch)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<mysql>.params.username` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<mysql>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<mysql>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<mysql>.params.address` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mysql>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mysql>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mysql>.params.checkpoint` | `enum(file\|sink)` | not supported yet |
| `sync.pipelines[].sinks[].sink<mysql>.params.checkpoint_table` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mysql>.params.database` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mysql>.params.ddl` | `enum(report\|stop)` | not supported yet |
| `sync.pipelines[].sinks[].sink<mysql>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mysql>.params.max_transaction_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<mysql>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mysql>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mysql>.params.server_public_key_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mysql>.params.table` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<mysql>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<mysql>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].sinks[].sink<mysql>.params.tls_rsa` | `boolean` | not supported yet |
| `sync.pipelines[].sinks[].sink<mysql>.params.transactions` | `enum(source\|batch)` | not supported yet |
| `sync.pipelines[].sinks[].sink<mysql>.params.username` | `string, required` | not supported yet |

<a id="fields-sync-sinks-nats"></a>

#### `sync.sinks/nats`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<nats>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.creds_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.max_in_flight` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.mode` | `enum(jetstream\|core)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.nkey_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.servers` | `array<string>, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.subject` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.token_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<nats>.params.username` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.creds_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.max_in_flight` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.mode` | `enum(jetstream\|core)` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.nkey_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.servers` | `array<string>, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.subject` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.token_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<nats>.params.username` | `string` | not supported yet |

<a id="fields-sync-sinks-otlp-logs"></a>

#### `sync.sinks/otlp_logs`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<otlp_logs>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.attributes` | `map<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.bearer_token_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.body` | `enum(line\|row)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.compression` | `enum(gzip\|none)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.encoding` | `enum(protobuf\|json)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.headers_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.max_request_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.resource` | `map<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.service_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.severity_field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.span_id_field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.timestamp_field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.trace_id_field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.urls` | `array<string>, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.username` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.attributes` | `map<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.bearer_token_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.body` | `enum(line\|row)` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.compression` | `enum(gzip\|none)` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.encoding` | `enum(protobuf\|json)` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.headers_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.max_request_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.resource` | `map<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.service_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.severity_field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.span_id_field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.timestamp_field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.trace_id_field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.urls` | `array<string>, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.username` | `string` | not supported yet |

<a id="fields-sync-sinks-postgres"></a>

#### `sync.sinks/postgres`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<postgres>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<postgres>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<postgres>.params.address` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<postgres>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<postgres>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<postgres>.params.channel_binding` | `enum(prefer\|require\|disable)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<postgres>.params.checkpoint` | `enum(file\|sink)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<postgres>.params.checkpoint_table` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<postgres>.params.copy` | `enum(auto\|always\|never)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<postgres>.params.copy_min_rows` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<postgres>.params.database` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<postgres>.params.key_columns` | `array<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<postgres>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<postgres>.params.max_statement_rows` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<postgres>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<postgres>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<postgres>.params.sslmode` | `enum(disable\|require\|verify-ca\|verify-full)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<postgres>.params.table` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<postgres>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<postgres>.params.transactions` | `enum(batch\|source)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<postgres>.params.user` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<postgres>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<postgres>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<postgres>.params.address` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<postgres>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<postgres>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<postgres>.params.channel_binding` | `enum(prefer\|require\|disable)` | not supported yet |
| `sync.pipelines[].sinks[].sink<postgres>.params.checkpoint` | `enum(file\|sink)` | not supported yet |
| `sync.pipelines[].sinks[].sink<postgres>.params.checkpoint_table` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<postgres>.params.copy` | `enum(auto\|always\|never)` | not supported yet |
| `sync.pipelines[].sinks[].sink<postgres>.params.copy_min_rows` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<postgres>.params.database` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<postgres>.params.key_columns` | `array<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<postgres>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<postgres>.params.max_statement_rows` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<postgres>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<postgres>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<postgres>.params.sslmode` | `enum(disable\|require\|verify-ca\|verify-full)` | not supported yet |
| `sync.pipelines[].sinks[].sink<postgres>.params.table` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<postgres>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<postgres>.params.transactions` | `enum(batch\|source)` | not supported yet |
| `sync.pipelines[].sinks[].sink<postgres>.params.user` | `string, required` | not supported yet |

<a id="fields-sync-sinks-pulsar"></a>

#### `sync.sinks/pulsar`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<pulsar>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.compression` | `enum(none\|lz4\|zlib\|zstd\|snappy)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.dedup` | `enum(read_back\|off)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.deletes` | `enum(tombstone\|encode)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.max_batch_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.max_batch_messages` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.producer_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.token_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.topic` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.url` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.compression` | `enum(none\|lz4\|zlib\|zstd\|snappy)` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.dedup` | `enum(read_back\|off)` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.deletes` | `enum(tombstone\|encode)` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.max_batch_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.max_batch_messages` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.producer_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.token_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.topic` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<pulsar>.params.url` | `string, required` | not supported yet |

<a id="fields-sync-sinks-qdrant"></a>

#### `sync.sinks/qdrant`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<qdrant>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<qdrant>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.api_key_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.collection` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.max_request_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.payload` | `array<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.url` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.vector_field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.vector_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<qdrant>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<qdrant>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<qdrant>.params.api_key_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<qdrant>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<qdrant>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<qdrant>.params.collection` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<qdrant>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<qdrant>.params.max_request_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<qdrant>.params.payload` | `array<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<qdrant>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<qdrant>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<qdrant>.params.url` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<qdrant>.params.vector_field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<qdrant>.params.vector_name` | `string` | not supported yet |

<a id="fields-sync-sinks-rabbitmq"></a>

#### `sync.sinks/rabbitmq`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<rabbitmq>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.addresses` | `array<string>, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.auth` | `enum(PLAIN\|EXTERNAL)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.content_type` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.exchange` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.frame_max` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.heartbeat` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.max_message_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.routing_key` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.transient` | `boolean` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.unroutable` | `enum(error\|drop)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.username` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.vhost` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.addresses` | `array<string>, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.auth` | `enum(PLAIN\|EXTERNAL)` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.content_type` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.exchange` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.frame_max` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.heartbeat` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.max_message_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.routing_key` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.transient` | `boolean` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.unroutable` | `enum(error\|drop)` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.username` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.vhost` | `string` | not supported yet |

<a id="fields-sync-sinks-redis"></a>

#### `sync.sinks/redis`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<redis>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<redis>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<redis>.params.address` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<redis>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<redis>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<redis>.params.db` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<redis>.params.entry_id` | `enum(auto\|key)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<redis>.params.form` | `enum(fields\|record)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<redis>.params.key` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<redis>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<redis>.params.max_len` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<redis>.params.max_pipeline` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<redis>.params.mode` | `enum(stream\|keys)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<redis>.params.op_field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<redis>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<redis>.params.protocol` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<redis>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<redis>.params.stream` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<redis>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<redis>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].dead_letter.sink<redis>.params.ttl` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<redis>.params.username` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<redis>.params.value` | `enum(json\|record\|hash)` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>.params.address` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>.params.db` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>.params.entry_id` | `enum(auto\|key)` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>.params.form` | `enum(fields\|record)` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>.params.key` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>.params.max_len` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>.params.max_pipeline` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>.params.mode` | `enum(stream\|keys)` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>.params.op_field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>.params.protocol` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>.params.stream` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>.params.ttl` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>.params.username` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<redis>.params.value` | `enum(json\|record\|hash)` | not supported yet |

<a id="fields-sync-sinks-s3"></a>

#### `sync.sinks/s3`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<s3>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.access_key_id` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.addressing` | `enum(path\|virtual)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.bucket` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.endpoint` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.gzip` | `boolean` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.key` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.max_object_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.region` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.secret_access_key_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.session_token_env` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<s3>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.access_key_id` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.addressing` | `enum(path\|virtual)` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.bucket` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.endpoint` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.gzip` | `boolean` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.key` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.max_object_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.region` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.secret_access_key_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.session_token_env` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<s3>.params.timeout` | `duration` | not supported yet |

<a id="fields-sync-sinks-stdout"></a>

#### `sync.sinks/stdout`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<stdout>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<stdout>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<stdout>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<stdout>.params` | `object` | not supported yet |

<a id="fields-sync-sinks-tdengine"></a>

#### `sync.sinks/tdengine`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<tdengine>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<tdengine>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.database` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.deletes` | `enum(skip\|fail)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.fields` | `array<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.max_request_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.measurement` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.mode` | `enum(schemaless\|sql)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.password_env` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.precision` | `enum(ns\|us\|ms)` | not supported yet |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.subtable` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.tags` | `array<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.time_column` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.time_field` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.url` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.username` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<tdengine>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<tdengine>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<tdengine>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<tdengine>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<tdengine>.params.database` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<tdengine>.params.deletes` | `enum(skip\|fail)` | not supported yet |
| `sync.pipelines[].sinks[].sink<tdengine>.params.fields` | `array<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<tdengine>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<tdengine>.params.max_request_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<tdengine>.params.measurement` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<tdengine>.params.mode` | `enum(schemaless\|sql)` | not supported yet |
| `sync.pipelines[].sinks[].sink<tdengine>.params.password_env` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<tdengine>.params.precision` | `enum(ns\|us\|ms)` | not supported yet |
| `sync.pipelines[].sinks[].sink<tdengine>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<tdengine>.params.subtable` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<tdengine>.params.tags` | `array<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<tdengine>.params.time_column` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<tdengine>.params.time_field` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<tdengine>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<tdengine>.params.url` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<tdengine>.params.username` | `string, required` | not supported yet |

<a id="fields-sync-sinks-wasm"></a>

#### `sync.sinks/wasm`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<wasm>` | `module` | not supported yet |
| `sync.pipelines[].dead_letter.sink<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].dead_letter.sink<wasm>.params.allow_hosts` | `array<string>` | not supported yet |
| `sync.pipelines[].dead_letter.sink<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].dead_letter.sink<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<wasm>.params.max_response_bytes` | `integer` | not supported yet |
| `sync.pipelines[].dead_letter.sink<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].dead_letter.sink<wasm>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].sinks[].sink<wasm>` | `module` | not supported yet |
| `sync.pipelines[].sinks[].sink<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].sinks[].sink<wasm>.params.allow_hosts` | `array<string>` | not supported yet |
| `sync.pipelines[].sinks[].sink<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].sinks[].sink<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<wasm>.params.max_response_bytes` | `integer` | not supported yet |
| `sync.pipelines[].sinks[].sink<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].sinks[].sink<wasm>.params.timeout` | `duration` | not supported yet |

<a id="fields-sync-sources-clickhouse"></a>

#### `sync.sources/clickhouse`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].source<clickhouse>` | `module` | not supported yet |
| `sync.pipelines[].source<clickhouse>.params` | `object` | not supported yet |
| `sync.pipelines[].source<clickhouse>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].source<clickhouse>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].source<clickhouse>.params.chunk_rows` | `integer` | not supported yet |
| `sync.pipelines[].source<clickhouse>.params.cursor` | `string` | not supported yet |
| `sync.pipelines[].source<clickhouse>.params.database` | `string` | not supported yet |
| `sync.pipelines[].source<clickhouse>.params.deleted_column` | `string` | not supported yet |
| `sync.pipelines[].source<clickhouse>.params.key` | `array<string>, required` | not supported yet |
| `sync.pipelines[].source<clickhouse>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].source<clickhouse>.params.namespace` | `string` | not supported yet |
| `sync.pipelines[].source<clickhouse>.params.page_rows` | `integer` | not supported yet |
| `sync.pipelines[].source<clickhouse>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].source<clickhouse>.params.poll_interval` | `duration` | not supported yet |
| `sync.pipelines[].source<clickhouse>.params.query` | `string` | not supported yet |
| `sync.pipelines[].source<clickhouse>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].source<clickhouse>.params.settings` | `map<string>` | not supported yet |
| `sync.pipelines[].source<clickhouse>.params.snapshot` | `enum(initial\|never)` | not supported yet |
| `sync.pipelines[].source<clickhouse>.params.table` | `string` | not supported yet |
| `sync.pipelines[].source<clickhouse>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<clickhouse>.params.urls` | `array<string>, required` | not supported yet |
| `sync.pipelines[].source<clickhouse>.params.username` | `string` | not supported yet |

<a id="fields-sync-sources-doris"></a>

#### `sync.sources/doris`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].source<doris>` | `module` | not supported yet |
| `sync.pipelines[].source<doris>.params` | `object` | not supported yet |
| `sync.pipelines[].source<doris>.params.address` | `string` | not supported yet |
| `sync.pipelines[].source<doris>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].source<doris>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].source<doris>.params.cursor` | `string` | not supported yet |
| `sync.pipelines[].source<doris>.params.database` | `string` | not supported yet |
| `sync.pipelines[].source<doris>.params.deleted_column` | `string` | not supported yet |
| `sync.pipelines[].source<doris>.params.key` | `array<string>, required` | not supported yet |
| `sync.pipelines[].source<doris>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].source<doris>.params.namespace` | `string` | not supported yet |
| `sync.pipelines[].source<doris>.params.page_rows` | `integer` | not supported yet |
| `sync.pipelines[].source<doris>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].source<doris>.params.poll_interval` | `duration` | not supported yet |
| `sync.pipelines[].source<doris>.params.query` | `string` | not supported yet |
| `sync.pipelines[].source<doris>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].source<doris>.params.snapshot` | `enum(initial\|never)` | not supported yet |
| `sync.pipelines[].source<doris>.params.table` | `string` | not supported yet |
| `sync.pipelines[].source<doris>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<doris>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].source<doris>.params.username` | `string, required` | not supported yet |

<a id="fields-sync-sources-elasticsearch"></a>

#### `sync.sources/elasticsearch`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].source<elasticsearch>` | `module` | not supported yet |
| `sync.pipelines[].source<elasticsearch>.params` | `object` | not supported yet |
| `sync.pipelines[].source<elasticsearch>.params.api_key_env` | `string` | not supported yet |
| `sync.pipelines[].source<elasticsearch>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].source<elasticsearch>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].source<elasticsearch>.params.chunk_documents` | `integer` | not supported yet |
| `sync.pipelines[].source<elasticsearch>.params.cursor` | `string` | not supported yet |
| `sync.pipelines[].source<elasticsearch>.params.delay` | `duration` | not supported yet |
| `sync.pipelines[].source<elasticsearch>.params.deleted_field` | `string` | not supported yet |
| `sync.pipelines[].source<elasticsearch>.params.index` | `string, required` | not supported yet |
| `sync.pipelines[].source<elasticsearch>.params.keep_alive` | `duration` | not supported yet |
| `sync.pipelines[].source<elasticsearch>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].source<elasticsearch>.params.max_page_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<elasticsearch>.params.page_size` | `integer` | not supported yet |
| `sync.pipelines[].source<elasticsearch>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].source<elasticsearch>.params.poll_interval` | `duration` | not supported yet |
| `sync.pipelines[].source<elasticsearch>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].source<elasticsearch>.params.snapshot` | `enum(pit\|cursor\|none)` | not supported yet |
| `sync.pipelines[].source<elasticsearch>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<elasticsearch>.params.urls` | `array<string>, required` | not supported yet |
| `sync.pipelines[].source<elasticsearch>.params.username` | `string` | not supported yet |

<a id="fields-sync-sources-file"></a>

#### `sync.sources/file`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].source<file>` | `module` | not supported yet |
| `sync.pipelines[].source<file>.params` | `object` | not supported yet |
| `sync.pipelines[].source<file>.params.close_after` | `duration` | not supported yet |
| `sync.pipelines[].source<file>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].source<file>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].source<file>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].source<file>.params.fingerprint_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<file>.params.max_files` | `integer` | not supported yet |
| `sync.pipelines[].source<file>.params.max_line_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<file>.params.max_open_files` | `integer` | not supported yet |
| `sync.pipelines[].source<file>.params.multiline` | `object` | not supported yet |
| `sync.pipelines[].source<file>.params.multiline.max_lines` | `integer` | not supported yet |
| `sync.pipelines[].source<file>.params.multiline.start` | `string, required` | not supported yet |
| `sync.pipelines[].source<file>.params.paths` | `array<string>, required` | not supported yet |
| `sync.pipelines[].source<file>.params.poll_interval` | `duration` | not supported yet |
| `sync.pipelines[].source<file>.params.scan_interval` | `duration` | not supported yet |

<a id="fields-sync-sources-generate"></a>

#### `sync.sources/generate`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].source<generate>` | `module` | not supported yet |
| `sync.pipelines[].source<generate>.params` | `object` | not supported yet |
| `sync.pipelines[].source<generate>.params.chunk_records` | `integer` | not supported yet |
| `sync.pipelines[].source<generate>.params.interval` | `duration` | not supported yet |
| `sync.pipelines[].source<generate>.params.namespace` | `string` | not supported yet |
| `sync.pipelines[].source<generate>.params.records` | `integer` | not supported yet |
| `sync.pipelines[].source<generate>.params.tx_size` | `integer` | not supported yet |

<a id="fields-sync-sources-http"></a>

#### `sync.sources/http`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].source<http>` | `module` | not supported yet |
| `sync.pipelines[].source<http>.params` | `object` | not supported yet |
| `sync.pipelines[].source<http>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].source<http>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].source<http>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].source<http>.params.form` | `enum(plain\|record)` | not supported yet |
| `sync.pipelines[].source<http>.params.framing` | `enum(lines\|body)` | not supported yet |
| `sync.pipelines[].source<http>.params.max_pending_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<http>.params.max_pending_requests` | `integer` | not supported yet |
| `sync.pipelines[].source<http>.params.namespace` | `string` | not supported yet |

<a id="fields-sync-sources-kafka"></a>

#### `sync.sources/kafka`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].source<kafka>` | `module` | not supported yet |
| `sync.pipelines[].source<kafka>.params` | `object` | not supported yet |
| `sync.pipelines[].source<kafka>.params.brokers` | `array<string>, required` | not supported yet |
| `sync.pipelines[].source<kafka>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].source<kafka>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].source<kafka>.params.client_id` | `string` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].source<kafka>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].source<kafka>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].source<kafka>.params.max_wait` | `duration` | not supported yet |
| `sync.pipelines[].source<kafka>.params.namespace` | `string` | not supported yet |
| `sync.pipelines[].source<kafka>.params.offset_reset` | `enum(error\|earliest\|latest)` | not supported yet |
| `sync.pipelines[].source<kafka>.params.partition_max_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<kafka>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].source<kafka>.params.read_uncommitted` | `boolean` | not supported yet |
| `sync.pipelines[].source<kafka>.params.sasl` | `enum(PLAIN\|SCRAM-SHA-256\|SCRAM-SHA-512)` | not supported yet |
| `sync.pipelines[].source<kafka>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].source<kafka>.params.start` | `enum(earliest\|latest)` | not supported yet |
| `sync.pipelines[].source<kafka>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<kafka>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].source<kafka>.params.tombstones` | `enum(delete\|skip)` | not supported yet |
| `sync.pipelines[].source<kafka>.params.topics` | `array<string>, required` | not supported yet |
| `sync.pipelines[].source<kafka>.params.username` | `string` | not supported yet |

<a id="fields-sync-sources-mongo"></a>

#### `sync.sources/mongo`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].source<mongo>` | `module` | not supported yet |
| `sync.pipelines[].source<mongo>.params` | `object` | not supported yet |
| `sync.pipelines[].source<mongo>.params.auth_source` | `string` | not supported yet |
| `sync.pipelines[].source<mongo>.params.batch_size` | `integer` | not supported yet |
| `sync.pipelines[].source<mongo>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].source<mongo>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].source<mongo>.params.chunk_documents` | `integer` | not supported yet |
| `sync.pipelines[].source<mongo>.params.collections` | `array<string>` | not supported yet |
| `sync.pipelines[].source<mongo>.params.connect_timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<mongo>.params.database` | `string` | not supported yet |
| `sync.pipelines[].source<mongo>.params.full_document` | `enum(updateLookup\|whenAvailable\|required)` | not supported yet |
| `sync.pipelines[].source<mongo>.params.full_document_before_change` | `enum(off\|whenAvailable\|required)` | not supported yet |
| `sync.pipelines[].source<mongo>.params.hosts` | `array<string>` | not supported yet |
| `sync.pipelines[].source<mongo>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].source<mongo>.params.max_await` | `duration` | not supported yet |
| `sync.pipelines[].source<mongo>.params.max_conns` | `integer` | not supported yet |
| `sync.pipelines[].source<mongo>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].source<mongo>.params.replica_set` | `string` | not supported yet |
| `sync.pipelines[].source<mongo>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].source<mongo>.params.snapshot` | `enum(initial\|never)` | not supported yet |
| `sync.pipelines[].source<mongo>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<mongo>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].source<mongo>.params.uri` | `string` | not supported yet |
| `sync.pipelines[].source<mongo>.params.username` | `string` | not supported yet |

<a id="fields-sync-sources-mqtt"></a>

#### `sync.sources/mqtt`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].source<mqtt>` | `module` | not supported yet |
| `sync.pipelines[].source<mqtt>.params` | `object` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.auth_method` | `enum(SCRAM-SHA-1\|SCRAM-SHA-256\|SCRAM-SHA-512)` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.client_id` | `string, required` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.idle` | `duration` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.keep_alive` | `duration` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.max_packet_size` | `integer` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.namespace` | `string` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.qos` | `integer` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.receive_maximum` | `integer` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.servers` | `array<string>, required` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.session_expiry` | `duration` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.topics` | `array<string>, required` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.username` | `string` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.version` | `enum(3.1.1\|5)` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.will` | `object` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.will.content_type` | `string` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.will.delay` | `duration` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.will.message_expiry` | `duration` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.will.payload` | `string` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.will.qos` | `integer` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.will.retain` | `boolean` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.will.topic` | `string, required` | not supported yet |
| `sync.pipelines[].source<mqtt>.params.will.user_properties` | `map<string>` | not supported yet |

<a id="fields-sync-sources-mssql"></a>

#### `sync.sources/mssql`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].source<mssql>` | `module` | not supported yet |
| `sync.pipelines[].source<mssql>.params` | `object` | not supported yet |
| `sync.pipelines[].source<mssql>.params.address` | `string` | not supported yet |
| `sync.pipelines[].source<mssql>.params.auth` | `object` | not supported yet |
| `sync.pipelines[].source<mssql>.params.auth.authority` | `string` | not supported yet |
| `sync.pipelines[].source<mssql>.params.auth.client_certificate_file` | `string` | not supported yet |
| `sync.pipelines[].source<mssql>.params.auth.client_id` | `string` | not supported yet |
| `sync.pipelines[].source<mssql>.params.auth.client_secret_env` | `string` | not supported yet |
| `sync.pipelines[].source<mssql>.params.auth.kdc` | `array<string>` | not supported yet |
| `sync.pipelines[].source<mssql>.params.auth.keytab_file` | `string` | not supported yet |
| `sync.pipelines[].source<mssql>.params.auth.krb5_conf` | `string` | not supported yet |
| `sync.pipelines[].source<mssql>.params.auth.managed_identity` | `boolean` | not supported yet |
| `sync.pipelines[].source<mssql>.params.auth.method` | `enum(entra\|kerberos\|ntlm), required` | not supported yet |
| `sync.pipelines[].source<mssql>.params.auth.realm` | `string` | not supported yet |
| `sync.pipelines[].source<mssql>.params.auth.spn` | `string` | not supported yet |
| `sync.pipelines[].source<mssql>.params.auth.tenant_id` | `string` | not supported yet |
| `sync.pipelines[].source<mssql>.params.auth.token_env` | `string` | not supported yet |
| `sync.pipelines[].source<mssql>.params.auth.token_file` | `string` | not supported yet |
| `sync.pipelines[].source<mssql>.params.batch_size` | `integer` | not supported yet |
| `sync.pipelines[].source<mssql>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].source<mssql>.params.database` | `string, required` | not supported yet |
| `sync.pipelines[].source<mssql>.params.encrypt` | `enum(on\|strict\|off)` | not supported yet |
| `sync.pipelines[].source<mssql>.params.exclude` | `array<string>` | not supported yet |
| `sync.pipelines[].source<mssql>.params.keys` | `map<array<string>>` | not supported yet |
| `sync.pipelines[].source<mssql>.params.mode` | `enum(cdc\|change_tracking)` | not supported yet |
| `sync.pipelines[].source<mssql>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].source<mssql>.params.poll_interval` | `duration` | not supported yet |
| `sync.pipelines[].source<mssql>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].source<mssql>.params.snapshot` | `enum(initial\|always\|never)` | not supported yet |
| `sync.pipelines[].source<mssql>.params.tables` | `array<string>` | not supported yet |
| `sync.pipelines[].source<mssql>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<mssql>.params.username` | `string` | not supported yet |
| `sync.pipelines[].source<mssql>.params.window_rows` | `integer` | not supported yet |
| `sync.pipelines[].source<mssql>.params.window_transactions` | `integer` | not supported yet |

<a id="fields-sync-sources-mysql"></a>

#### `sync.sources/mysql`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].source<mysql>` | `module` | not supported yet |
| `sync.pipelines[].source<mysql>.params` | `object` | not supported yet |
| `sync.pipelines[].source<mysql>.params.address` | `string` | not supported yet |
| `sync.pipelines[].source<mysql>.params.batch_size` | `integer` | not supported yet |
| `sync.pipelines[].source<mysql>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].source<mysql>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].source<mysql>.params.chunk_rows` | `integer` | not supported yet |
| `sync.pipelines[].source<mysql>.params.exclude` | `array<string>` | not supported yet |
| `sync.pipelines[].source<mysql>.params.heartbeat` | `duration` | not supported yet |
| `sync.pipelines[].source<mysql>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].source<mysql>.params.keys` | `map<array<string>>` | not supported yet |
| `sync.pipelines[].source<mysql>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].source<mysql>.params.server_id` | `integer, required` | not supported yet |
| `sync.pipelines[].source<mysql>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].source<mysql>.params.server_public_key_file` | `string` | not supported yet |
| `sync.pipelines[].source<mysql>.params.snapshot` | `enum(initial\|always\|never)` | not supported yet |
| `sync.pipelines[].source<mysql>.params.tables` | `array<string>` | not supported yet |
| `sync.pipelines[].source<mysql>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<mysql>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].source<mysql>.params.tls_rsa` | `boolean` | not supported yet |
| `sync.pipelines[].source<mysql>.params.username` | `string, required` | not supported yet |

<a id="fields-sync-sources-nats"></a>

#### `sync.sources/nats`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].source<nats>` | `module` | not supported yet |
| `sync.pipelines[].source<nats>.params` | `object` | not supported yet |
| `sync.pipelines[].source<nats>.params.ack_wait` | `duration` | not supported yet |
| `sync.pipelines[].source<nats>.params.batch` | `integer` | not supported yet |
| `sync.pipelines[].source<nats>.params.buffer` | `integer` | not supported yet |
| `sync.pipelines[].source<nats>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].source<nats>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].source<nats>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].source<nats>.params.consumer` | `string` | not supported yet |
| `sync.pipelines[].source<nats>.params.create_consumer` | `boolean` | not supported yet |
| `sync.pipelines[].source<nats>.params.creds_file` | `string` | not supported yet |
| `sync.pipelines[].source<nats>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].source<nats>.params.max_ack_pending` | `integer` | not supported yet |
| `sync.pipelines[].source<nats>.params.max_wait` | `duration` | not supported yet |
| `sync.pipelines[].source<nats>.params.metadata_field` | `string` | not supported yet |
| `sync.pipelines[].source<nats>.params.mode` | `enum(jetstream\|core)` | not supported yet |
| `sync.pipelines[].source<nats>.params.name` | `string` | not supported yet |
| `sync.pipelines[].source<nats>.params.namespace` | `string` | not supported yet |
| `sync.pipelines[].source<nats>.params.nkey_file` | `string` | not supported yet |
| `sync.pipelines[].source<nats>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].source<nats>.params.queue` | `string` | not supported yet |
| `sync.pipelines[].source<nats>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].source<nats>.params.servers` | `array<string>, required` | not supported yet |
| `sync.pipelines[].source<nats>.params.start` | `enum(all\|new)` | not supported yet |
| `sync.pipelines[].source<nats>.params.stream` | `string` | not supported yet |
| `sync.pipelines[].source<nats>.params.subjects` | `array<string>` | not supported yet |
| `sync.pipelines[].source<nats>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<nats>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].source<nats>.params.token_env` | `string` | not supported yet |
| `sync.pipelines[].source<nats>.params.username` | `string` | not supported yet |

<a id="fields-sync-sources-postgres"></a>

#### `sync.sources/postgres`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].source<postgres>` | `module` | not supported yet |
| `sync.pipelines[].source<postgres>.params` | `object` | not supported yet |
| `sync.pipelines[].source<postgres>.params.address` | `string` | not supported yet |
| `sync.pipelines[].source<postgres>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].source<postgres>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].source<postgres>.params.channel_binding` | `enum(prefer\|require\|disable)` | not supported yet |
| `sync.pipelines[].source<postgres>.params.chunk_rows` | `integer` | not supported yet |
| `sync.pipelines[].source<postgres>.params.connect_timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<postgres>.params.create` | `boolean` | not supported yet |
| `sync.pipelines[].source<postgres>.params.database` | `string, required` | not supported yet |
| `sync.pipelines[].source<postgres>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].source<postgres>.params.lag_warning` | `integer` | not supported yet |
| `sync.pipelines[].source<postgres>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].source<postgres>.params.publication` | `string, required` | not supported yet |
| `sync.pipelines[].source<postgres>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].source<postgres>.params.slot` | `string, required` | not supported yet |
| `sync.pipelines[].source<postgres>.params.snapshot` | `enum(initial\|never)` | not supported yet |
| `sync.pipelines[].source<postgres>.params.snapshot_parallel` | `integer` | not supported yet |
| `sync.pipelines[].source<postgres>.params.spool_dir` | `string` | not supported yet |
| `sync.pipelines[].source<postgres>.params.sslmode` | `enum(disable\|require\|verify-ca\|verify-full)` | not supported yet |
| `sync.pipelines[].source<postgres>.params.status_interval` | `duration` | not supported yet |
| `sync.pipelines[].source<postgres>.params.streaming` | `enum(auto\|off)` | not supported yet |
| `sync.pipelines[].source<postgres>.params.tables` | `array<string>` | not supported yet |
| `sync.pipelines[].source<postgres>.params.user` | `string, required` | not supported yet |

<a id="fields-sync-sources-pulsar"></a>

#### `sync.sources/pulsar`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].source<pulsar>` | `module` | not supported yet |
| `sync.pipelines[].source<pulsar>.params` | `object` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.metadata_field` | `string` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.mode` | `enum(reader\|subscription)` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.namespace` | `string` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.receiver_queue` | `integer` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.start` | `enum(earliest\|latest)` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.subscription` | `string` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.subscription_type` | `enum(exclusive\|failover)` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.token_env` | `string` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.tombstones` | `enum(delete\|skip)` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.topics` | `array<string>, required` | not supported yet |
| `sync.pipelines[].source<pulsar>.params.url` | `string, required` | not supported yet |

<a id="fields-sync-sources-rabbitmq"></a>

#### `sync.sources/rabbitmq`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].source<rabbitmq>` | `module` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params` | `object` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.addresses` | `array<string>, required` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.auth` | `enum(PLAIN\|EXTERNAL)` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.frame_max` | `integer` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.heartbeat` | `duration` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.idle_end` | `duration` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.max_message_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.metadata_field` | `string` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.namespace` | `string` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.prefetch` | `integer` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.queues` | `array<string>` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.stream_start` | `enum(first\|last\|next)` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.streams` | `array<string>` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.username` | `string` | not supported yet |
| `sync.pipelines[].source<rabbitmq>.params.vhost` | `string` | not supported yet |

<a id="fields-sync-sources-redis"></a>

#### `sync.sources/redis`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].source<redis>` | `module` | not supported yet |
| `sync.pipelines[].source<redis>.params` | `object` | not supported yet |
| `sync.pipelines[].source<redis>.params.address` | `string` | not supported yet |
| `sync.pipelines[].source<redis>.params.block` | `duration` | not supported yet |
| `sync.pipelines[].source<redis>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].source<redis>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].source<redis>.params.count` | `integer` | not supported yet |
| `sync.pipelines[].source<redis>.params.db` | `integer` | not supported yet |
| `sync.pipelines[].source<redis>.params.form` | `enum(fields\|record)` | not supported yet |
| `sync.pipelines[].source<redis>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].source<redis>.params.namespace` | `string` | not supported yet |
| `sync.pipelines[].source<redis>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].source<redis>.params.protocol` | `integer` | not supported yet |
| `sync.pipelines[].source<redis>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].source<redis>.params.snapshot` | `enum(initial\|never)` | not supported yet |
| `sync.pipelines[].source<redis>.params.streams` | `array<string>, required` | not supported yet |
| `sync.pipelines[].source<redis>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<redis>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].source<redis>.params.username` | `string` | not supported yet |

<a id="fields-sync-sources-s3"></a>

#### `sync.sources/s3`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].source<s3>` | `module` | not supported yet |
| `sync.pipelines[].source<s3>.params` | `object` | not supported yet |
| `sync.pipelines[].source<s3>.params.access_key_id` | `string` | not supported yet |
| `sync.pipelines[].source<s3>.params.addressing` | `enum(path\|virtual)` | not supported yet |
| `sync.pipelines[].source<s3>.params.bucket` | `string, required` | not supported yet |
| `sync.pipelines[].source<s3>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec` | `object` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].source<s3>.params.codec.params` | `object` | not supported yet |
| `sync.pipelines[].source<s3>.params.decompress` | `enum(auto\|gzip\|none)` | not supported yet |
| `sync.pipelines[].source<s3>.params.endpoint` | `string` | not supported yet |
| `sync.pipelines[].source<s3>.params.max_line_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<s3>.params.namespace` | `string` | not supported yet |
| `sync.pipelines[].source<s3>.params.poll_interval` | `duration` | not supported yet |
| `sync.pipelines[].source<s3>.params.prefix` | `string` | not supported yet |
| `sync.pipelines[].source<s3>.params.range_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<s3>.params.region` | `string` | not supported yet |
| `sync.pipelines[].source<s3>.params.secret_access_key_env` | `string` | not supported yet |
| `sync.pipelines[].source<s3>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].source<s3>.params.session_token_env` | `string` | not supported yet |
| `sync.pipelines[].source<s3>.params.start_after` | `string` | not supported yet |
| `sync.pipelines[].source<s3>.params.timeout` | `duration` | not supported yet |

<a id="fields-sync-sources-tidb"></a>

#### `sync.sources/tidb`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].source<tidb>` | `module` | not supported yet |
| `sync.pipelines[].source<tidb>.params` | `object` | not supported yet |
| `sync.pipelines[].source<tidb>.params.address` | `string` | not supported yet |
| `sync.pipelines[].source<tidb>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].source<tidb>.params.cert_file` | `string` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka` | `object, required` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.brokers` | `array<string>, required` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.ca_file` | `string` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.cert_file` | `string` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.client_id` | `string` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec` | `object` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.codec.params` | `object` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.key_file` | `string` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.max_wait` | `duration` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.namespace` | `string` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.offset_reset` | `enum(error\|earliest\|latest)` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.partition_max_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.password_env` | `string` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.read_uncommitted` | `boolean` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.sasl` | `enum(PLAIN\|SCRAM-SHA-256\|SCRAM-SHA-512)` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.server_name` | `string` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.start` | `enum(earliest\|latest)` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.tls` | `boolean` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.tombstones` | `enum(delete\|skip)` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.topics` | `array<string>, required` | not supported yet |
| `sync.pipelines[].source<tidb>.params.kafka.username` | `string` | not supported yet |
| `sync.pipelines[].source<tidb>.params.key_file` | `string` | not supported yet |
| `sync.pipelines[].source<tidb>.params.page_rows` | `integer` | not supported yet |
| `sync.pipelines[].source<tidb>.params.password_env` | `string` | not supported yet |
| `sync.pipelines[].source<tidb>.params.protocol` | `enum(canal-json\|simple)` | not supported yet |
| `sync.pipelines[].source<tidb>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].source<tidb>.params.snapshot` | `enum(initial\|never)` | not supported yet |
| `sync.pipelines[].source<tidb>.params.tables` | `array<string>` | not supported yet |
| `sync.pipelines[].source<tidb>.params.time_zone` | `string` | not supported yet |
| `sync.pipelines[].source<tidb>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].source<tidb>.params.tls` | `boolean` | not supported yet |
| `sync.pipelines[].source<tidb>.params.username` | `string` | not supported yet |

<a id="fields-sync-sources-wasm"></a>

#### `sync.sources/wasm`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].source<wasm>` | `module` | not supported yet |
| `sync.pipelines[].source<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].source<wasm>.params.allow_hosts` | `array<string>` | not supported yet |
| `sync.pipelines[].source<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].source<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<wasm>.params.max_response_bytes` | `integer` | not supported yet |
| `sync.pipelines[].source<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].source<wasm>.params.poll_interval` | `duration` | not supported yet |
| `sync.pipelines[].source<wasm>.params.timeout` | `duration` | not supported yet |

<a id="fields-sync-transforms-cast"></a>

#### `sync.transforms/cast`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].transforms[]<cast>` | `module` | not supported yet |
| `sync.pipelines[].transforms[]<cast>.params` | `object` | not supported yet |
| `sync.pipelines[].transforms[]<cast>.params.fields` | `map<string>, required` | not supported yet |

<a id="fields-sync-transforms-embed"></a>

#### `sync.transforms/embed`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].transforms[]<embed>` | `module` | not supported yet |
| `sync.pipelines[].transforms[]<embed>.params` | `object` | not supported yet |
| `sync.pipelines[].transforms[]<embed>.params.api_key_env` | `string` | not supported yet |
| `sync.pipelines[].transforms[]<embed>.params.attempts` | `integer` | not supported yet |
| `sync.pipelines[].transforms[]<embed>.params.batch_records` | `integer` | not supported yet |
| `sync.pipelines[].transforms[]<embed>.params.ca_file` | `string` | not supported yet |
| `sync.pipelines[].transforms[]<embed>.params.cache_bytes` | `integer` | not supported yet |
| `sync.pipelines[].transforms[]<embed>.params.dimensions` | `integer` | not supported yet |
| `sync.pipelines[].transforms[]<embed>.params.fields` | `array<string>, required` | not supported yet |
| `sync.pipelines[].transforms[]<embed>.params.into` | `string` | not supported yet |
| `sync.pipelines[].transforms[]<embed>.params.max_parallel` | `integer` | not supported yet |
| `sync.pipelines[].transforms[]<embed>.params.max_text_bytes` | `integer` | not supported yet |
| `sync.pipelines[].transforms[]<embed>.params.model` | `string, required` | not supported yet |
| `sync.pipelines[].transforms[]<embed>.params.separator` | `string` | not supported yet |
| `sync.pipelines[].transforms[]<embed>.params.server_name` | `string` | not supported yet |
| `sync.pipelines[].transforms[]<embed>.params.timeout` | `duration` | not supported yet |
| `sync.pipelines[].transforms[]<embed>.params.url` | `string` | not supported yet |

<a id="fields-sync-transforms-envelope"></a>

#### `sync.transforms/envelope`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].transforms[]<envelope>` | `module` | not supported yet |
| `sync.pipelines[].transforms[]<envelope>.params` | `object` | not supported yet |
| `sync.pipelines[].transforms[]<envelope>.params.deletes` | `enum(keep\|drop\|rewrite)` | not supported yet |
| `sync.pipelines[].transforms[]<envelope>.params.meta` | `map<string>` | not supported yet |
| `sync.pipelines[].transforms[]<envelope>.params.shape` | `enum(flat\|pair\|debezium)` | not supported yet |

<a id="fields-sync-transforms-fields"></a>

#### `sync.transforms/fields`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].transforms[]<fields>` | `module` | not supported yet |
| `sync.pipelines[].transforms[]<fields>.params` | `object` | not supported yet |
| `sync.pipelines[].transforms[]<fields>.params.drop` | `array<string>` | not supported yet |
| `sync.pipelines[].transforms[]<fields>.params.keep` | `array<string>` | not supported yet |
| `sync.pipelines[].transforms[]<fields>.params.nest` | `array<object>` | not supported yet |
| `sync.pipelines[].transforms[]<fields>.params.nest[].fields` | `array<string>, required` | not supported yet |
| `sync.pipelines[].transforms[]<fields>.params.nest[].into` | `string, required` | not supported yet |
| `sync.pipelines[].transforms[]<fields>.params.rename` | `map<string>` | not supported yet |
| `sync.pipelines[].transforms[]<fields>.params.set` | `map<any>` | not supported yet |
| `sync.pipelines[].transforms[]<fields>.params.unnest` | `array<string>` | not supported yet |

<a id="fields-sync-transforms-filter"></a>

#### `sync.transforms/filter`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].transforms[]<filter>` | `module` | not supported yet |
| `sync.pipelines[].transforms[]<filter>.params` | `object` | not supported yet |
| `sync.pipelines[].transforms[]<filter>.params.action` | `enum(keep\|drop)` | not supported yet |
| `sync.pipelines[].transforms[]<filter>.params.exclude_namespaces` | `array<string>` | not supported yet |
| `sync.pipelines[].transforms[]<filter>.params.namespaces` | `array<string>` | not supported yet |
| `sync.pipelines[].transforms[]<filter>.params.ops` | `array<enum(insert\|update\|delete\|ddl\|message)>` | not supported yet |
| `sync.pipelines[].transforms[]<filter>.params.where` | `array<object>` | not supported yet |
| `sync.pipelines[].transforms[]<filter>.params.where[].equals` | `string` | not supported yet |
| `sync.pipelines[].transforms[]<filter>.params.where[].field` | `string, required` | not supported yet |
| `sync.pipelines[].transforms[]<filter>.params.where[].in` | `array<string>` | not supported yet |
| `sync.pipelines[].transforms[]<filter>.params.where[].present` | `boolean` | not supported yet |

<a id="fields-sync-transforms-key"></a>

#### `sync.transforms/key`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].transforms[]<key>` | `module` | not supported yet |
| `sync.pipelines[].transforms[]<key>.params` | `object` | not supported yet |
| `sync.pipelines[].transforms[]<key>.params.fields` | `array<string>, required` | not supported yet |
| `sync.pipelines[].transforms[]<key>.params.format` | `enum(map\|value\|join)` | not supported yet |
| `sync.pipelines[].transforms[]<key>.params.separator` | `string` | not supported yet |

<a id="fields-sync-transforms-mask"></a>

#### `sync.transforms/mask`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].transforms[]<mask>` | `module` | not supported yet |
| `sync.pipelines[].transforms[]<mask>.params` | `object` | not supported yet |
| `sync.pipelines[].transforms[]<mask>.params.fields` | `array<string>, required` | not supported yet |
| `sync.pipelines[].transforms[]<mask>.params.key_env` | `string` | not supported yet |
| `sync.pipelines[].transforms[]<mask>.params.with` | `enum(hmac_sha256\|sha256\|null)` | not supported yet |

<a id="fields-sync-transforms-route"></a>

#### `sync.transforms/route`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].transforms[]<route>` | `module` | not supported yet |
| `sync.pipelines[].transforms[]<route>.params` | `object` | not supported yet |
| `sync.pipelines[].transforms[]<route>.params.map` | `map<string>` | not supported yet |
| `sync.pipelines[].transforms[]<route>.params.rules` | `array<object>` | not supported yet |
| `sync.pipelines[].transforms[]<route>.params.rules[].match` | `string, required` | not supported yet |
| `sync.pipelines[].transforms[]<route>.params.rules[].to` | `string, required` | not supported yet |
| `sync.pipelines[].transforms[]<route>.params.unmatched` | `enum(keep\|drop)` | not supported yet |

<a id="fields-sync-transforms-wasm"></a>

#### `sync.transforms/wasm`

| Field | Type | Status |
| --- | --- | --- |
| `sync.pipelines[].transforms[]<wasm>` | `module` | not supported yet |
| `sync.pipelines[].transforms[]<wasm>.params` | `object` | not supported yet |
| `sync.pipelines[].transforms[]<wasm>.params.batch_records` | `integer` | not supported yet |
| `sync.pipelines[].transforms[]<wasm>.params.config` | `string` | not supported yet |
| `sync.pipelines[].transforms[]<wasm>.params.instances` | `integer` | not supported yet |
| `sync.pipelines[].transforms[]<wasm>.params.max_memory_bytes` | `integer` | not supported yet |
| `sync.pipelines[].transforms[]<wasm>.params.max_output_bytes` | `integer` | not supported yet |
| `sync.pipelines[].transforms[]<wasm>.params.path` | `string, required` | not supported yet |
| `sync.pipelines[].transforms[]<wasm>.params.timeout` | `duration` | not supported yet |

<a id="fields-telemetry"></a>

#### `telemetry`

| Field | Type | Status |
| --- | --- | --- |
| `telemetry` | `object` | supported |
| `telemetry.ca_file` | `string` | supported |
| `telemetry.cert_file` | `string` | supported |
| `telemetry.endpoint` | `string` | supported |
| `telemetry.headers_file` | `string` | supported |
| `telemetry.key_file` | `string` | supported |
| `telemetry.metrics` | `object` | supported |
| `telemetry.metrics.interval` | `duration` | supported |
| `telemetry.service_name` | `string` | supported |
| `telemetry.timeout` | `duration` | supported |
| `telemetry.traces` | `object` | supported |
| `telemetry.traces.interval` | `duration` | supported |
| `telemetry.traces.max_queue_bytes` | `integer` | supported |
| `telemetry.traces.sample_ratio` | `number` | supported |

<a id="fields-tunnel"></a>

#### `tunnel`

| Field | Type | Status |
| --- | --- | --- |
| `tunnel` | `object` | supported |
| `tunnel.hub` | `object` | supported |
| `tunnel.hub.access` | `object` | supported |
| `tunnel.hub.access.allow` | `array<string>` | supported |
| `tunnel.hub.access.allow_files` | `array<string>` | supported |
| `tunnel.hub.access.deny` | `array<string>` | supported |
| `tunnel.hub.access.deny_files` | `array<string>` | supported |
| `tunnel.hub.allow_proxy_services` | `boolean` | supported |
| `tunnel.hub.body_password` | `string` | supported |
| `tunnel.hub.call_timeout` | `duration` | supported |
| `tunnel.hub.console` | `object` | supported |
| `tunnel.hub.console.access` | `object` | supported |
| `tunnel.hub.console.access.allow` | `array<string>` | supported |
| `tunnel.hub.console.access.allow_files` | `array<string>` | supported |
| `tunnel.hub.console.access.deny` | `array<string>` | supported |
| `tunnel.hub.console.access.deny_files` | `array<string>` | supported |
| `tunnel.hub.console.address` | `string, required` | supported |
| `tunnel.hub.console.cert_file` | `string` | supported |
| `tunnel.hub.console.key_file` | `string` | supported |
| `tunnel.hub.console.max_conns` | `integer` | supported |
| `tunnel.hub.console.max_sessions` | `integer` | supported |
| `tunnel.hub.console.mptcp` | `boolean` | not supported yet |
| `tunnel.hub.console.session_lifetime` | `duration` | supported |
| `tunnel.hub.console.session_timeout` | `duration` | supported |
| `tunnel.hub.console.users` | `array<object>, required` | supported |
| `tunnel.hub.console.users[].password_hash` | `string, required` | supported |
| `tunnel.hub.console.users[].username` | `string, required` | supported |
| `tunnel.hub.console_socket` | `object` | supported |
| `tunnel.hub.console_socket.address` | `string, required` | supported |
| `tunnel.hub.console_socket.max_conns` | `integer` | supported |
| `tunnel.hub.console_socket.password` | `string` | supported |
| `tunnel.hub.listeners` | `array<object>` | supported |
| `tunnel.hub.listeners[].address` | `string, required` | supported |
| `tunnel.hub.listeners[].cert_file` | `string` | supported |
| `tunnel.hub.listeners[].key_file` | `string` | supported |
| `tunnel.hub.listeners[].path` | `string` | supported |
| `tunnel.hub.listeners[].protocol` | `enum(tcp\|tls\|ws\|wss\|quic), required` | supported |
| `tunnel.hub.max_links` | `integer` | supported |
| `tunnel.hub.max_links_per_peer` | `integer` | supported |
| `tunnel.hub.max_peers` | `integer` | supported |
| `tunnel.hub.max_tunnels` | `integer` | supported |
| `tunnel.hub.max_unregistered` | `integer` | supported |
| `tunnel.hub.max_unregistered_per_ip` | `integer` | supported |
| `tunnel.hub.name` | `string` | supported |
| `tunnel.hub.password` | `string` | supported |
| `tunnel.hub.quic` | `object` | supported |
| `tunnel.hub.quic.cert` | `string` | supported |
| `tunnel.hub.quic.handshake_timeout` | `duration` | supported |
| `tunnel.hub.quic.idle_timeout` | `duration` | supported |
| `tunnel.hub.quic.keep_alive` | `duration` | supported |
| `tunnel.hub.quic.key` | `string` | supported |
| `tunnel.hub.quic.max_handshakes` | `integer` | supported |
| `tunnel.hub.register_timeout` | `duration` | supported |
| `tunnel.hub.tokens` | `array<object>` | supported |
| `tunnel.hub.tokens[].created_at` | `integer` | supported |
| `tunnel.hub.tokens[].expires_at` | `integer` | supported |
| `tunnel.hub.tokens[].hash` | `string, required` | supported |
| `tunnel.hub.tokens[].id` | `string, required` | supported |
| `tunnel.hub.tokens[].members` | `array<string>` | supported |
| `tunnel.hub.tokens[].name` | `string, required` | supported |
| `tunnel.hub.tokens[].note` | `string` | supported |
| `tunnel.node` | `object` | supported |
| `tunnel.node.allow_hub_services` | `boolean` | supported |
| `tunnel.node.console` | `object` | supported |
| `tunnel.node.console.access` | `object` | supported |
| `tunnel.node.console.access.allow` | `array<string>` | supported |
| `tunnel.node.console.access.allow_files` | `array<string>` | supported |
| `tunnel.node.console.access.deny` | `array<string>` | supported |
| `tunnel.node.console.access.deny_files` | `array<string>` | supported |
| `tunnel.node.console.address` | `string, required` | supported |
| `tunnel.node.console.cert_file` | `string` | supported |
| `tunnel.node.console.key_file` | `string` | supported |
| `tunnel.node.console.max_conns` | `integer` | supported |
| `tunnel.node.console.max_sessions` | `integer` | supported |
| `tunnel.node.console.mptcp` | `boolean` | not supported yet |
| `tunnel.node.console.session_lifetime` | `duration` | supported |
| `tunnel.node.console.session_timeout` | `duration` | supported |
| `tunnel.node.console.users` | `array<object>, required` | supported |
| `tunnel.node.console.users[].password_hash` | `string, required` | supported |
| `tunnel.node.console.users[].username` | `string, required` | supported |
| `tunnel.node.dial_timeout` | `duration` | supported |
| `tunnel.node.hubs` | `array<object>` | supported |
| `tunnel.node.hubs[].body_password` | `string` | supported |
| `tunnel.node.hubs[].ca_file` | `string` | supported |
| `tunnel.node.hubs[].call_timeout` | `duration` | supported |
| `tunnel.node.hubs[].dial_timeout` | `duration` | supported |
| `tunnel.node.hubs[].endpoints` | `array<object>, required` | supported |
| `tunnel.node.hubs[].endpoints[].address` | `string, required` | supported |
| `tunnel.node.hubs[].endpoints[].disabled` | `boolean` | supported |
| `tunnel.node.hubs[].name` | `string` | supported |
| `tunnel.node.hubs[].password` | `string` | supported |
| `tunnel.node.hubs[].pin` | `string` | supported |
| `tunnel.node.hubs[].ping_interval` | `duration` | supported |
| `tunnel.node.hubs[].ping_timeout` | `duration` | supported |
| `tunnel.node.hubs[].pool_size` | `integer` | supported |
| `tunnel.node.hubs[].quic_insecure` | `boolean` | supported |
| `tunnel.node.hubs[].quic_pin` | `string, deprecated` | supported |
| `tunnel.node.hubs[].retry_interval` | `duration` | supported |
| `tunnel.node.hubs[].token` | `string` | supported |
| `tunnel.node.idle_timeout` | `duration` | supported |
| `tunnel.node.max_tunnels` | `integer` | supported |
| `tunnel.node.meta` | `map<string>` | supported |
| `tunnel.node.name` | `string` | supported |
| `tunnel.node.services` | `array<object>` | supported |
| `tunnel.node.services[].address` | `string, required` | supported |
| `tunnel.node.services[].description` | `string` | supported |
| `tunnel.node.services[].name` | `string, required` | supported |
| `tunnel.proxy` | `object` | supported |
| `tunnel.proxy.access` | `object` | supported |
| `tunnel.proxy.access.allow` | `array<string>` | supported |
| `tunnel.proxy.access.allow_files` | `array<string>` | supported |
| `tunnel.proxy.access.deny` | `array<string>` | supported |
| `tunnel.proxy.access.deny_files` | `array<string>` | supported |
| `tunnel.proxy.console` | `object` | supported |
| `tunnel.proxy.console.access` | `object` | supported |
| `tunnel.proxy.console.access.allow` | `array<string>` | supported |
| `tunnel.proxy.console.access.allow_files` | `array<string>` | supported |
| `tunnel.proxy.console.access.deny` | `array<string>` | supported |
| `tunnel.proxy.console.access.deny_files` | `array<string>` | supported |
| `tunnel.proxy.console.address` | `string, required` | supported |
| `tunnel.proxy.console.cert_file` | `string` | supported |
| `tunnel.proxy.console.key_file` | `string` | supported |
| `tunnel.proxy.console.max_conns` | `integer` | supported |
| `tunnel.proxy.console.max_sessions` | `integer` | supported |
| `tunnel.proxy.console.mptcp` | `boolean` | not supported yet |
| `tunnel.proxy.console.session_lifetime` | `duration` | supported |
| `tunnel.proxy.console.session_timeout` | `duration` | supported |
| `tunnel.proxy.console.users` | `array<object>, required` | supported |
| `tunnel.proxy.console.users[].password_hash` | `string, required` | supported |
| `tunnel.proxy.console.users[].username` | `string, required` | supported |
| `tunnel.proxy.forwards` | `array<object>` | supported |
| `tunnel.proxy.forwards[].hub` | `string` | supported |
| `tunnel.proxy.forwards[].listen` | `string, required` | supported |
| `tunnel.proxy.forwards[].node` | `string, required` | supported |
| `tunnel.proxy.forwards[].service` | `string, required` | supported |
| `tunnel.proxy.hubs` | `array<object>` | supported |
| `tunnel.proxy.hubs[].body_password` | `string` | supported |
| `tunnel.proxy.hubs[].ca_file` | `string` | supported |
| `tunnel.proxy.hubs[].call_timeout` | `duration` | supported |
| `tunnel.proxy.hubs[].dial_timeout` | `duration` | supported |
| `tunnel.proxy.hubs[].endpoints` | `array<object>, required` | supported |
| `tunnel.proxy.hubs[].endpoints[].address` | `string, required` | supported |
| `tunnel.proxy.hubs[].endpoints[].disabled` | `boolean` | supported |
| `tunnel.proxy.hubs[].name` | `string` | supported |
| `tunnel.proxy.hubs[].password` | `string` | supported |
| `tunnel.proxy.hubs[].pin` | `string` | supported |
| `tunnel.proxy.hubs[].ping_interval` | `duration` | supported |
| `tunnel.proxy.hubs[].ping_timeout` | `duration` | supported |
| `tunnel.proxy.hubs[].pool_size` | `integer` | supported |
| `tunnel.proxy.hubs[].quic_insecure` | `boolean` | supported |
| `tunnel.proxy.hubs[].quic_pin` | `string, deprecated` | supported |
| `tunnel.proxy.hubs[].retry_interval` | `duration` | supported |
| `tunnel.proxy.hubs[].token` | `string` | supported |
| `tunnel.proxy.idle_timeout` | `duration` | supported |
| `tunnel.proxy.max_tunnels` | `integer` | supported |
| `tunnel.proxy.meta` | `map<string>` | supported |
| `tunnel.proxy.name` | `string` | supported |
| `tunnel.proxy.open_timeout` | `duration` | supported |
| `tunnel.proxy.web_proxies` | `array<object>` | supported |
| `tunnel.proxy.web_proxies[].listen` | `string, required` | supported |
| `tunnel.proxy.web_proxies[].max_conns` | `integer` | supported |
| `tunnel.proxy.web_proxies[].name` | `string` | supported |
| `tunnel.proxy.web_proxies[].password` | `string, required` | supported |
| `tunnel.proxy.web_proxies[].url` | `string, required` | supported |

<a id="fields-upstreams"></a>

#### `upstreams`

| Field | Type | Status |
| --- | --- | --- |
| `upstreams` | `array<object>` | supported |
| `upstreams[].addresses` | `array<string>, required` | supported |
| `upstreams[].connect_timeout` | `duration` | supported |
| `upstreams[].deny_internal` | `boolean` | not supported yet |
| `upstreams[].fail_timeout` | `duration` | supported |
| `upstreams[].hash_key` | `string` | supported |
| `upstreams[].health_check` | `object` | supported |
| `upstreams[].health_check.expect_status` | `array<integer>` | supported |
| `upstreams[].health_check.fails` | `integer` | supported |
| `upstreams[].health_check.host` | `string` | supported |
| `upstreams[].health_check.interval` | `duration` | supported |
| `upstreams[].health_check.jitter` | `duration` | supported |
| `upstreams[].health_check.passes` | `integer` | supported |
| `upstreams[].health_check.path` | `string` | supported |
| `upstreams[].health_check.port` | `integer` | supported |
| `upstreams[].health_check.timeout` | `duration` | supported |
| `upstreams[].health_check.type` | `enum(http\|tcp)` | supported |
| `upstreams[].idle_timeout` | `duration` | supported |
| `upstreams[].load_balancing` | `enum(round_robin\|weighted_round_robin\|least_conn\|random\|p2c\|ip_hash\|uri_hash\|header_hash\|cookie_hash)` | supported |
| `upstreams[].max_conns` | `integer` | supported |
| `upstreams[].max_fails` | `integer` | supported |
| `upstreams[].max_idle_conns` | `integer` | supported |
| `upstreams[].max_pending` | `integer` | supported |
| `upstreams[].name` | `string, required` | supported |
| `upstreams[].protocol` | `enum(http/1.1\|h2c\|h2)` | supported |
| `upstreams[].response_header_timeout` | `duration` | supported |
| `upstreams[].response_idle_timeout` | `duration` | supported |
| `upstreams[].response_timeout` | `duration` | supported |
| `upstreams[].sticky_cookie` | `object` | supported |
| `upstreams[].sticky_cookie.max_age` | `duration` | supported |
| `upstreams[].sticky_cookie.name` | `string` | supported |
| `upstreams[].sticky_cookie.path` | `string` | supported |
| `upstreams[].sticky_cookie.secure` | `boolean` | supported |
| `upstreams[].tls_ca_file` | `string` | supported |
| `upstreams[].tls_cert_file` | `string` | supported |
| `upstreams[].tls_key_file` | `string` | supported |
| `upstreams[].tls_server_name` | `string` | supported |
| `upstreams[].weights` | `array<integer>` | supported |

<a id="fields-webrtc"></a>

#### `webrtc`

| Field | Type | Status |
| --- | --- | --- |
| `webrtc` | `object` | supported |
| `webrtc.media` | `object` | supported |
| `webrtc.media.access` | `object` | supported |
| `webrtc.media.access.allow` | `array<string>` | supported |
| `webrtc.media.access.allow_files` | `array<string>` | supported |
| `webrtc.media.access.deny` | `array<string>` | supported |
| `webrtc.media.access.deny_files` | `array<string>` | supported |
| `webrtc.media.codecs` | `array<string>` | supported |
| `webrtc.media.connect_timeout` | `duration` | supported |
| `webrtc.media.ice_servers` | `array<object>` | supported |
| `webrtc.media.ice_servers[].credential` | `string` | supported |
| `webrtc.media.ice_servers[].urls` | `array<string>, required` | supported |
| `webrtc.media.ice_servers[].username` | `string` | supported |
| `webrtc.media.ice_timeout` | `duration` | supported |
| `webrtc.media.key_frame_interval` | `duration` | supported |
| `webrtc.media.listen` | `string, required` | supported |
| `webrtc.media.max_bitrate` | `integer` | supported |
| `webrtc.media.max_peer_connections` | `integer` | supported |
| `webrtc.media.max_sdp_bytes` | `integer` | supported |
| `webrtc.media.max_streams` | `integer` | supported |
| `webrtc.media.max_viewers` | `integer` | supported |
| `webrtc.media.public_ips` | `array<string>` | supported |
| `webrtc.media.turn_credential_ttl` | `duration` | supported |
| `webrtc.media.turn_urls` | `array<string>` | supported |
| `webrtc.proxy` | `object` | supported |
| `webrtc.proxy.access` | `object` | supported |
| `webrtc.proxy.access.allow` | `array<string>` | supported |
| `webrtc.proxy.access.allow_files` | `array<string>` | supported |
| `webrtc.proxy.access.deny` | `array<string>` | supported |
| `webrtc.proxy.access.deny_files` | `array<string>` | supported |
| `webrtc.proxy.idle_timeout` | `duration` | supported |
| `webrtc.proxy.listen` | `string, required` | supported |
| `webrtc.proxy.max_relays` | `integer` | supported |
| `webrtc.proxy.max_sessions` | `integer` | supported |
| `webrtc.proxy.port_max` | `integer` | supported |
| `webrtc.proxy.port_min` | `integer` | supported |
| `webrtc.proxy.public_ips` | `array<string>` | supported |
| `webrtc.proxy.relay_ip` | `string` | supported |
| `webrtc.proxy.session_timeout` | `duration` | supported |
| `webrtc.turn` | `object` | supported |
| `webrtc.turn.access` | `object` | supported |
| `webrtc.turn.access.allow` | `array<string>` | supported |
| `webrtc.turn.access.allow_files` | `array<string>` | supported |
| `webrtc.turn.access.deny` | `array<string>` | supported |
| `webrtc.turn.access.deny_files` | `array<string>` | supported |
| `webrtc.turn.allowed_peers` | `array<string>` | supported |
| `webrtc.turn.default_lifetime` | `duration` | supported |
| `webrtc.turn.denied_peers` | `array<string>` | supported |
| `webrtc.turn.listen` | `array<string>, required` | supported |
| `webrtc.turn.listen_tcp` | `array<string>` | supported |
| `webrtc.turn.listen_tls` | `array<string>` | supported |
| `webrtc.turn.max_allocations` | `integer` | supported |
| `webrtc.turn.max_allocations_per_user` | `integer` | supported |
| `webrtc.turn.max_channels` | `integer` | supported |
| `webrtc.turn.max_lifetime` | `duration` | supported |
| `webrtc.turn.max_permissions` | `integer` | supported |
| `webrtc.turn.max_stream_message` | `integer` | supported |
| `webrtc.turn.mptcp` | `boolean` | supported |
| `webrtc.turn.nonce_lifetime` | `duration` | supported |
| `webrtc.turn.nonce_secret` | `string` | supported |
| `webrtc.turn.port_max` | `integer` | supported |
| `webrtc.turn.port_min` | `integer` | supported |
| `webrtc.turn.public_ip` | `string` | supported |
| `webrtc.turn.realm` | `string, required` | supported |
| `webrtc.turn.relay_ip` | `string, required` | supported |
| `webrtc.turn.secret` | `string` | supported |
| `webrtc.turn.stream_idle_timeout` | `duration` | supported |
| `webrtc.turn.tls` | `object` | supported |
| `webrtc.turn.tls.certificates` | `array<object>, required` | supported |
| `webrtc.turn.tls.certificates[].cert_file` | `string, required` | supported |
| `webrtc.turn.tls.certificates[].key_file` | `string, required` | supported |
| `webrtc.turn.user_bandwidth` | `integer` | supported |
| `webrtc.turn.users` | `array<object>` | supported |
| `webrtc.turn.users[].key` | `string` | supported |
| `webrtc.turn.users[].name` | `string, required` | supported |
| `webrtc.turn.users[].password` | `string` | supported |

<!-- END GENERATED -->
