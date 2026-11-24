# 006: The Admin API on an akio engine of its own, applied by the control thread

[English](006-admin-api.md) | [简体中文](006-admin-api.zh-CN.md)

Status: accepted in I26 (2026-11-24).

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
  and the `Host` must name a loopback host. Without a token (I27) the address must be a loopback
  one; a Unix socket waits on akio (A23).
- **fit's paths and payloads**, the `Fit-Config-Version` header included, so fit's clients work
  against akit; edits, rollback, `POST /reload` and dry runs answer 501 until I27.

## Consequences

- akit runs one more akio engine, of one thread, when the config has `admin`.
- A load waits behind a SIGHUP reload and the other way round: there is one control thread.
- akio-http cannot yet close a connection after a response that refuses its body (A24): the rest
  of a body past `max_config_bytes` is read and dropped, bounded by the body timeout, where fit
  closes the connection.
- Changes to `admin` take effect on a restart, and a load that changes it says so, as in fit.
