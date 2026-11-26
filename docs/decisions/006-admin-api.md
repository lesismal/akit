# 006: The Admin API on an akio engine of its own, applied by the control thread

[English](006-admin-api.md) | [简体中文](006-admin-api.zh-CN.md)

Status: accepted in I26 (2026-11-24); its header fields renamed by [decision 037](037-akit-names.md)
(`Akit-Config-Version`, `Akit-Dry-Run`: fit's clients that send fit's names do not work against akit).

## Context

I26 adds fit's Admin API: `GET /config` (whole, or a value by JSON pointer), `POST /load`,
`GET /versions` and `GET /versions/{n}`, with ETags, `If-Match` and `If-None-Match`
([fit's decision 005](https://github.com/lesismal/fit/blob/main/docs/decisions/005-admin-api.md)).
fit serves it with net/http on goroutines of its own, off fib's workers, because applying a
config reads files and binds listeners, which block. akit has no goroutines and no async runtime
(rules 1 and 16): it has to decide what serves the API, where a config is applied, and how the
two meet without a blocking wait on an I/O thread.

## Decision

- **akio-http serves the API, on an akio engine of its own with one I/O thread**, built and
  started by `akit_admin::Server::start` beside the listeners' engine: a busy data plane does not
  hold the API up, and the API's limits are its engine's. HTTP/1.1 only (one request at a time on
  each connection), `max_conns` as the engine's `max_connections` (one past it is closed as it is
  accepted), 64 KiB of header, timeouts of 10 s for a head, 60 s without a body byte and 60 s
  idle, as fit's.
- **Reads are answered on the I/O thread** from `akit_admin::Configs`, the history, which the
  I/O thread holds only to clone an `Arc` of a version (rule 1: no lock held long).
- **A load is streamed in under `max_config_bytes`** (413 past it, from `Content-Length` or as it
  arrives), then detached (`Context::detach`) and handed to the control thread as a `Job` through
  a bounded queue (`try_send`; full: 503). The control thread, `akit run`'s main loop that already
  serves signals, applies it with the same code as SIGHUP: parse, compile, then
  `akit_listener::Listeners::reload` (I22), and answers from there (`Detached::respond` is safe
  from any thread). Configs are applied one at a time, `If-Match` compared with the config each
  one replaces, so concurrent writers lose nothing.
- **Every config takes `Configs::update`**, from the files (SIGHUP, source `file`) or the API
  (source `admin`): one numbering, from 1 at start, one more per config applied; the last
  `admin.history` (10) are kept.
- **Configs are kept as written**: a load's body and a lone JSON file are kept as standard JSON
  (comments and trailing commas blanked to spaces, every other byte in place, as fit's
  `config.Standardize`); several files, or a YAML or `.conf` one, as one JSON document written as
  fit writes one (`akit_config::value::json_of`). The ETag is the first 96 bits of the SHA-256 of
  that text (akio-crypto), as fit's.
- **Web pages are refused as in fit**: `Origin` gets 403 but on a read from the API's own origin,
  and the `Host` must name a loopback host. The address must be a loopback one (off loopback fit
  asks for a token and TLS, below); a Unix socket waits on akio (A23).
- **fit's paths and payloads**, the version header included (fit's `Fit-Config-Version`, akit's
  `Akit-Config-Version` since decision 037); edits, rollback, `POST /reload` and dry runs answered 501 until I27.

Update, I27 (2026-11-24):

- **Every change is a `Job` the control thread runs**: `POST /load`, `PUT`, `PATCH` and
  `DELETE /config[/<pointer>]`, `POST /rollback/{n}` and `POST /reload`. Bodies are streamed in as
  a load's are, the pointer and `?dry_run` checked on the I/O thread; the control thread makes the
  change from the running config inside `Configs::change`, under the lock that orders applies,
  after `If-Match` is compared, so that a writer's edit starts from the config before it and a
  stale one gets 412 with nothing called. An edited config is written out again as fit writes it
  (`Value::to_json`); a whole config (`POST /load`, `PUT /config`) is kept as written. A rollback
  applies version `n`'s config as a new version, source `rollback`; a reload applies the running
  one again, source `admin`, as fit's.
- **A dry run takes the apply path without its effects**: the config is parsed and compiled with
  an empty engine slot (its pools dial and probe nothing) and a registry of its own (its counters
  reach no scrape), and `Listeners::plan` lists what `Listeners::reload` would open and close,
  resolving each new listener's address and binding it for a moment unless a running listener
  holds it. Nothing is kept or counted; the answer carries the running version and ETag, the ETag
  the config would have, and `Akit-Dry-Run: 1` (fit's `Fit-Dry-Run`, renamed by decision 037).
- **The token** (`admin.token_file`) is read when the API starts, trimmed, and refused when
  empty; every request then carries `Authorization: Bearer <token>`, compared in constant time, or
  gets 401 with `WWW-Authenticate`; the `Host` check is dropped then, as in fit, and `Origin` is
  still refused. An address off loopback stays refused: fit asks for TLS there too (`cert_file`,
  `key_file`), and akit does not serve the API over TLS yet.
- Not taken from fit: its connection limiter (a quarter of `max_conns` kept for clients that
  showed the token, unauthenticated connections closed after 5 s; akit keeps the engine's
  `max_connections`), and the connection closed after a 401, which waits on A24 as the 413's does.

## Consequences

- akit runs one more akio engine, of one thread, when the config has `admin`.
- A load waits behind a SIGHUP reload and the other way round: there is one control thread.
- akio-http cannot yet close a connection after a response that refuses its body (A24): the rest
  of a body past `max_config_bytes` is read and dropped, bounded by the body timeout, where fit
  closes the connection.
- Changes to `admin` take effect on a restart, and a load that changes it says so, as in fit.
