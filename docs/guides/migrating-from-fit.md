# Migrating from fit

[English](migrating-from-fit.md) | [简体中文](migrating-from-fit.zh-CN.md)

akit is fit rebuilt in Rust on [akio](https://github.com/lesismal/akio): the same config, in the same
three formats, the same Admin API, the same commands and flags, metric names and access log fields
([decision 002](../decisions/002-config-compatibility.md)). Moving a config from fit to akit is meant
to be a binary swap. What stands in the way is what akit does not support yet, which it says by name
rather than ignore; this guide lists it, and the [compatibility page](../compatibility.md) lists every
field of fit's with whether akit reads it.

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
  `disable_dashboard` and `access`: no TLS, so no address off loopback, and no console, store, assistant or
  cluster.
- **Values**: `static`'s `disk_io auto` (akit looks at the disk on its file pool only).
- **In `.conf` files**: `import` and `include`; name every file with `-c`, or a directory.
- **Commands and flags**: akit has `run`, `validate`, `adapt`, `fmt`, `build` and `version`; `run`'s
  `-admin` and `-timeout` are refused. fit's `check`, `hash-password`, `token`, `store`, `sync`, `bt`
  and `media-token` are not there yet; `htpasswd -B` makes bcrypt hashes for `basic_auth`.

## What stays the same, and what to look at

- **The defaults' names are fit's**: Docker labels under `fit.`, Consul tags under `fit`, KV keys
  under `fit/`, the sticky cookie `fit_sticky`, the telemetry service `fit`, the Kubernetes
  controller `lesismal.github.io/fit`. akit and fit side by side in one cluster or on one Docker
  host would both take the same labels and GatewayClasses: give one of them its own prefix or
  `controller_name`.
- **ACME's storage** defaults to `~/.local/share/akit`, not fit's directory. To keep fit's accounts
  and certificates, point `acme.storage` at fit's directory, or `shared_storage` at fit's store.
- **Platforms**: Linux, where akio uses io_uring (5.19 and later) or epoll, and macOS for
  development. Windows is not supported.
- **WebAssembly plugins** built for fit run unchanged, on akio-wasm's interpreter rather than
  wazero's compiler: a plugin's call costs more ([WebAssembly](wasm.md)).

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
akit fmt -w -c /etc/fit
```
