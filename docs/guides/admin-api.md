# The Admin API and reloads

[English](admin-api.md) | [简体中文](admin-api.zh-CN.md)

A running akit takes a new config in three ways, all applied the same way: SIGHUP reads its files
again, `akit run -watch` reads them each time one changes, and the Admin API takes a config, or a
change to one, over HTTP. A config that does not load leaves the running one in place, and says why.
Listeners that did not change keep their connections; one that changed is bound anew and the old one
drained; requests under way finish on the config they started with. The
[reference](../reference/config.md#admin-api) has every field.

## Reloading

```bash
akit run -c /etc/akit
kill -HUP "$(pidof akit)"
akit run -watch -c /etc/akit
```

With `-watch`, a file of the config changed, added to its directory or taken out applies the config
again, looked at every `-watch-interval` (1s). Certificate files are watched on their own
(`-watch-certs`, on by default). SIGINT and SIGTERM stop akit gracefully, letting requests under way
finish for up to `-shutdown-timeout` (30s).

## The Admin API

```fit
admin {
	address 127.0.0.1:2019
	token_file admin.token
}

metrics {}

listener web :8080

route hello {
	match { path /hello }
	handler respond { body "hello\n" }
}
```

The API listens on 127.0.0.1:2019 by default, or on a Unix socket (`unix//run/akit/admin.sock`,
made 0600). akit does not serve it over TLS yet, so it takes no other address: reach it from the host,
through an SSH tunnel, or with `docker exec`. With `token_file`, every request carries the token as
`Authorization: Bearer <token>`. Its settings are read when akit starts.

What it serves:

| Request | What it does |
| --- | --- |
| `GET /config`, `GET /config/<pointer>` | the running config in fit's native JSON, or the value at a JSON pointer, with its `ETag` |
| `POST /load` | a whole config, in fit's native JSON (`akit adapt` converts the other formats) |
| `PUT`, `PATCH`, `DELETE /config/<pointer>` | a change to the value at a pointer; `PATCH` takes a JSON merge patch |
| `POST /reload` | the running config applied again, as a new version: certificate files and the like read again |
| `GET /versions`, `GET /versions/{n}`, `POST /rollback/{n}` | the configs applied, one of them, and a rollback to it, applied as a new version |
| `GET /config/effective` | the config with the providers' parts merged, and where each came from |
| `GET /status` | listeners, upstreams and their addresses' health, certificates, reloads |
| `GET /metrics` | metrics in Prometheus's text format |
| `GET /cache`, `POST /cache/purge` | what each cache zone keeps, and purges ([caching](caching.md)) |
| `GET /logs`, `POST /logs/rotate`, `POST /logs/cleanup` | the log files of `log` and `access_log` with their rotation and files on disk, one rotated now, and what its retention removes, removed or (`dry_run`) listed |
| `GET /dashboard/` | a read-only dashboard of the status and the config |

A change sent with `If-Match: <etag>` is refused with 412 if the config changed since it was read,
so that two clients cannot overwrite each other. With `?dry_run=1` it is checked through the whole
path an apply takes and answered as an apply would be, with nothing applied.

```bash
TOKEN=$(cat admin.token)
curl -H "Authorization: Bearer $TOKEN" localhost:2019/config/routes/0
curl -H "Authorization: Bearer $TOKEN" -X PATCH \
  -H 'Content-Type: application/merge-patch+json' \
  -d '{"body": "hello again\n"}' localhost:2019/config/routes/0/handler/params
curl -H "Authorization: Bearer $TOKEN" -X POST 'localhost:2019/load?dry_run=1' \
  -H 'Content-Type: application/json' -d @akit.json
curl -H "Authorization: Bearer $TOKEN" localhost:2019/versions
curl -H "Authorization: Bearer $TOKEN" -X POST localhost:2019/rollback/1
```

A config loaded through the API is not written back to the files: a SIGHUP afterwards applies the
files again. Keep the files the source of truth, or load every change
through the API.

## Metrics and tracing

`GET /metrics` serves akit's metrics: upstreams and their addresses, certificates, connections,
reloads, akio's engine, and, with `metrics {}`, requests by listener and route. `telemetry` exports
traces of requests and the same metrics over OTLP/HTTP to a collector
([telemetry](../reference/config.md#telemetry)).

## Not supported yet

fit's console, its encrypted store, the assistant, cluster management, TLS and access lists for the
Admin API, and `fit run -admin` are not supported yet: akit refuses them by name.
