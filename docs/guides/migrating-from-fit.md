# Migrating from fit

[English](migrating-from-fit.md) | [简体中文](migrating-from-fit.zh-CN.md)

akit is fit rebuilt in Rust on [akio](https://github.com/lesismal/akio): the same config, in the same
three formats, the same Admin API, the same commands and flags and access log fields
([decision 002](../decisions/002-config-compatibility.md)), with akit's own names wherever fit's
say "fit" ([decision 037](../decisions/037-akit-names.md)). Moving a config from fit to akit is
meant to be a binary swap and the [names below](#akits-names). What else stands in the way is what
akit does not support yet, which it says by name rather than ignore; this guide lists it, and the
[compatibility page](../compatibility.md) lists every field of fit's with whether akit reads it.

## Check the config first

```bash
akit validate -c /etc/fit
```

reads the config as `akit run` would, opening the files it names, and prints `valid`, or each
problem with its file, line and column. A field akit does not support yet is one of them:

```text
fit.conf:12:2: access: not supported yet
fit.conf:40:4: routes[3].handler.module: not supported yet: the "hls" handler
```

Remove or replace each, and validate again. Nothing akit refuses is read and ignored, so a config
that validates runs as fit runs it, within what akit supports.

## What akit does not run yet

- **Whole parts**: `pprof`, and the roles
  beside the gateway: `llm`, `tunnel`, `webrtc`, `media_auth`, `streaming`, `media_library`, `sync`
  and `bittorrent`. The [roadmap](../roadmap.md) says when each comes.
- **Handlers** for media, BitTorrent and sync: `hls`, `httpflv`, `vod`, `whip`, `whep`,
  `stream_whip`, `stream_whep`, `webrtc_proxy`, `websockify`, `media_pages`, `media_tokens`,
  `bt_publish`, `bt_stream`, `bt_tracker` and `sync_ingest`. Every middleware of fit's is supported.
- **Fields of parts akit reads**: `engine`'s `loop_wait`, `poll_batch` and `kernel_tls_offload` (the rest of
  `engine` is read onto akio's engine, as the [reference](../reference/config.md#engine) says);
  every `mptcp`; `tls.kernel_tls_offload`; `upstreams[].deny_internal`;
  the proxy's `splice_uploads`; the Kubernetes provider's `allow_external_name_services`; and in
  `admin`, everything but `address`, `token_file`, `history`, `max_config_bytes`, `max_conns`,
  `disable_dashboard`, `access`, `cert_file`, `key_file`, `client_ca_file` and `console`: no store,
  assistant or cluster.
- **Values**: `static`'s `disk_io auto` (akit looks at the disk on its file pool only).
- **In `.conf` files**: `import` and `include`; name every file with `-c`, or a directory.
- **Commands and flags**: akit has `run`, `validate`, `adapt`, `fmt`, `build`, `hash-password` and
  `version`; `run`'s `-admin` and `-timeout` are refused. fit's `check`, `token`, `store`, `sync`,
  `bt` and `media-token` are not there yet.

## akit's names

Where fit's names say "fit", akit's say "akit", and fit's are not accepted as aliases
([decision 037](../decisions/037-akit-names.md)). A fit user changes these:

| What | fit | akit | To change |
| --- | --- | --- | --- |
| Docker labels (`providers[].docker.label_prefix`) | `fit.enable`, `fit.routes.<r>.host`, ... | `akit.enable`, `akit.routes.<r>.host`, ... | the labels, or `label_prefix fit` |
| Consul tags (`providers[].consul.tag_prefix`) | `fit.enable=true`, ... | `akit.enable=true`, ... | the tags, or `tag_prefix fit` |
| Consul KV and etcd keys (`prefix`) | `fit/` | `akit/` | the keys, or `prefix fit/` |
| Shared certificates (`acme.shared_storage.prefix`) | `fit/` | `akit/` | `prefix fit/`, to share fit's |
| Kubernetes controller (`controller_name`) | `lesismal.github.io/fit` | `lesismal.github.io/akit` | the GatewayClasses and IngressClasses, or `controller_name` |
| Kubernetes Ingress annotations | `fit.lesismal.github.io/*` | `akit.lesismal.github.io/*` | the annotations |
| The sticky cookie (`sticky_cookie.name`) | `fit_sticky` | `akit_sticky` | `name fit_sticky`, to keep clients' sessions |
| `basic_auth`'s `realm`; `jwt`'s challenge | `fit` | `akit` | `realm fit` (`jwt`'s is fixed) |
| `telemetry.service_name`; the OTLP scope; span attributes | `fit`; `fit`; `fit.listener`, `fit.route`, `fit.upstream.address` | `akit`; `akit`; `akit.listener`, `akit.route`, `akit.upstream.address` | `service_name fit`; queries and dashboards |
| Prometheus metrics | `fit_*` | `akit_*` | dashboards, alerts and recording rules |
| `Cache-Status`'s cache name | `fit; hit` | `akit; hit` | what reads it |
| The trailer of a body cut mid-way | `Fit-Error` | `Akit-Error` | what reads it |
| Admin API header fields | `Fit-Config-Version`, `Fit-Dry-Run` | `Akit-Config-Version`, `Akit-Dry-Run` | scripts and clients |
| The console's cookie and CSRF header | `fit_console_<port>`, `X-Fit-Csrf` | `akit_console_<port>`, `X-Akit-Csrf` | scripts; logins start again |
| The config export's file name | `fit-config-v<n>.json` | `akit-config-v<n>.json` | what picks it up |
| WebAssembly plugins | export `fit_on_request`, imports from `fit` | export `akit_on_request`, imports from `akit` | rebuild with the names renamed ([WebAssembly](wasm.md#akits-abi)) |
| The default config file | `fit.json` | `akit.json` | the file, or `-c fit.json` |
| `adapt`'s `.conf` format | `-from fit`, `-to fit` | `-from akit`, `-to akit` | scripts |

The config's fields keep fit's names, the `.conf`, YAML and JSON formats are fit's, and Proxy-Wasm
filters run unchanged. akit and fit side by side no longer take each other's labels, keys,
GatewayClasses or cookies.

## What else to look at

- **ACME's storage** defaults to `~/.local/share/akit`, not fit's directory. To keep fit's accounts
  and certificates, point `acme.storage` at fit's directory, or `shared_storage` at fit's store.
- **Platforms**: Linux, where akio uses io_uring (5.19 and later) or epoll, and macOS for
  development. Windows is not supported.
- **WebAssembly plugins** of fit's ABI are built again for akit's names (above); Proxy-Wasm filters
  run unchanged. They run on akio-wasm's interpreter rather than wazero's compiler: a plugin's call
  costs more ([WebAssembly](wasm.md)).

## akit's own fields

`listeners[].sockmap` splices a `tcp` listener's proxied connections in the kernel, with akio's eBPF
sockmap relay ([decision 010](../decisions/010-sockmap-relay.md)). fit refuses it as an unknown
field: a config that sets it is akit's alone.

```fit
listener db :5432 tcp { sockmap true }

upstream postgres 10.0.0.5:5432

service postgres { listeners db; upstream postgres }
```

## Converting the format

`akit adapt` and `akit fmt` write configs as `fit adapt` and `fit fmt` write them, so a config can be
moved between `.conf`, YAML and JSON with either:

```bash
akit adapt -c fit.conf -to yaml -o fit.yaml
akit adapt -c fit.yaml -to akit -o akit.conf
akit fmt -w -c /etc/fit
```
