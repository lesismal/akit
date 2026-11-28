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
made 0600). With `token_file`, every request carries the token as `Authorization: Bearer <token>`.
With `cert_file` and `key_file` it is served over TLS, 1.2 or later, and with `client_ca_file` only
to clients whose certificate chains to one of its CAs. An address off loopback needs both a token and
TLS, so that the token never crosses a network in the clear; without them, reach the API from the
host, through an SSH tunnel, or with `docker exec`. Its settings are read when akit starts, but for
`access`, which follows every config applied, and the certificate files: with `-watch-certs` they are
looked at every `-watch-interval`, and SIGHUP reads them again, and a certificate renewed outside
akit is served to new connections without a restart. Files that do not load, as when a certificate
is written and its key not yet, are logged in the `admin` logger, and the running certificate stays
until they change again.

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
| `GET /logs/recent?level=&logger=&contains=&limit=&after=` | the latest messages of akit's own log, from the last 1,000 it keeps in memory, oldest first, those numbered after `after`; a cluster manager tails them across its members |
| `POST /config/render?format=`, `POST /config/parse?format=` | a config fragment, a JSON object, written as `.conf`, `yaml` or `json`; and the JSON of such a text |
| `PUT /config/text?format=&members=` | a text applied as the whole config, or as those top-level members only, the rest kept; `If-Match` and `dry_run` as `PUT /config`; its errors placed at their lines in the text |
| `POST /config/export`, `POST /config/import?format=&members=` | the config (`admin` left out, secrets redacted), or some members, as a file; and a file applied, `admin` kept |
| `GET /dashboard/` | a read-only dashboard of the status and the config |
| `GET /tokens`, `GET` and `POST /tokens/{member}`, `PATCH` and `DELETE /tokens/{member}/{id}` | the link tokens the tunnel's hub and the LLM roles' listeners issue (`tunnel.hub`, `llm.hub`, `llm.node`, `llm.proxy`): listed with their expiry, last use, members and links; issued (`{name, note, ttl \| expires_at}`, the token shown once, `akitt1.<id>.<secret>`); their expiry changed; revoked. Each change is an edit of the member, logged to the `admin` logger; `akit token` calls them |
| `/bittorrent/...` | the BitTorrent roles' torrents, events and statistics ([below](#bittorrent)); `akit bt` calls them |
| `/cluster/...` | on a cluster manager (`admin.cluster`), its members, states, groups, drift, diffs, series, certificates, logs and rollouts, acting as a cluster-wide admin named `Admin API` ([the cluster guide](cluster.md#akit-cluster)); `akit cluster` calls them; 404 on an akit that manages no members |

A change sent with `If-Match: <etag>` is refused with 412 if the config changed since it was read,
so that two clients cannot overwrite each other. With `?dry_run=1` it is checked through the whole
path an apply takes and answered as an apply would be, with nothing applied.

Secrets written inline in the config, under a member whose name says it holds one (a
`password_hash`, a `secret`, a `token`, an `Authorization` header a route sets), are answered as
`[redacted]` by every read: `GET /config`, its pointers, `GET /versions/{n}` and
`GET /config/effective`, as fit's console shows them. A change that sends `[redacted]` back keeps
the secret the running config holds at that place, so a config read, edited and loaded again loses
none; `[redacted]` where no secret is kept is refused with 400. To change a secret, send the new
value. Secrets kept in files (`token_file`, `password_file`, `key_file`) are never in the config;
akit warns at start, and on each change, of such a file its group or others may read
([decision 035](../decisions/035-admin-api-redacts-secrets.md)).

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

`admin.access` says which client addresses the API serves, before the global `access`, which
applies to the API too ([access lists](../reference/config.md#access-lists)); unlike the rest of
`admin` it follows every config applied. A global `allow` that leaves out `127.0.0.1` shuts local
clients out of the API as well, unless the API's own allows them:

```fit
access { allow 10.0.0.0/8 }

admin {
	address 127.0.0.1:2019
	access { allow 127.0.0.1 ::1 }
}

route { match { host example.com }; handler respond { body hello } }
```

## Link tokens

The tunnel's hub and the LLM roles' listeners take links from members that hold a token they
issued, in place of a password every member shares (fit's decision 035, [decision
038](../decisions/038-tunnel-and-llm.md)). A token is `akitt1.<id>.<secret>`: the config keeps its
ID, name, times, note, the members bound to it and a hash, never the token, which is shown once.
`akit token` manages them through the Admin API of a running akit, its address, token and TLS
read from the config's `admin`:

```sh
akit token create -c akit.json -role tunnel.hub -name lab -ttl 90d   # prints the token, once
akit token list   -c akit.json -role tunnel.hub                      # expiry, last use, members, links
akit token expiry -c akit.json -role tunnel.hub -id fdd44d906b8e6ce2 -never
akit token revoke -c akit.json -role tunnel.hub -id fdd44d906b8e6ce2
```

A token fit issued is the same token with akit's prefix: `fitt1.<id>.<secret>` is written
`akitt1.<id>.<secret>`. Unlike fit's, the endpoints work on the member the running config sets
whether or not its role runs yet; the running role adds each token's last use and links.

## BitTorrent

With `bittorrent.client` running, the API serves its torrents below `/bittorrent/` (fit's
`/bittorrent/...`, [decision 042](../decisions/042-bittorrent.md)); with `bittorrent.tracker`, the
tracker's statistics. An endpoint of a role that does not run answers 404 saying so.

| Endpoint | What it does |
| --- | --- |
| `GET /bittorrent/torrents` | the torrents in queue order, `{torrents, total, offset}`; `state` (states with commas), `category`, `q` (in the name), `offset`, `limit` (1,000 by default, 10,000 at most) |
| `POST /bittorrent/torrents` | add one: JSON `{magnet \| url \| torrent (base64), category, paused, peers, limits}`; a .torrent file as `application/x-bittorrent`, `category` and `paused` in the query; or a form, `multipart/form-data`, the file in the part `torrent`. 201 with `Location` |
| `GET`, `POST`, `PATCH`, `DELETE /bittorrent/torrents/{id}` | one with its files; `{"action": "pause" \| "resume" \| "recheck"}`; a change of `paused`, `category`, `limits` (`null` clears them), `queue_position`; removed, `?delete_data=1` with its files |
| `GET /bittorrent/torrents/{id}/files`, `/peers`, `/trackers` | its files, its peers, its trackers and web seeds |
| `GET`, `PUT /bittorrent/alt_speed` | the alternative rates; `{"mode": "auto" \| "on" \| "off"}` |
| `GET /bittorrent/publish`, `/publish/{name}`, `/publish/{name}/torrents/{id}` | the publish entries, one, and a .torrent file one publishes |
| `GET /bittorrent/events` | the client's events as server-sent events (`added`, `removed`, `state`, `completed`, `error`, `seed_limit`, `alt_speed`); `Last-Event-ID` (or `?after=`) replays those missed of the last 1,024, `reset` when some are gone; a comment every 15 s; 32 streams at most |
| `GET /bittorrent/tracker/stats`, `/tracker/torrents`, `/tracker/torrents/{hash}` | the tracker's totals, the allow-list's swarms, one swarm |

A torrent's `{id}` is its v1 info hash in hex, or its v2 hash (64 hex digits). `GET /metrics`
carries the client's `akit_bittorrent_*` families (bytes, rates, peers and torrents in each state
by category, the alternative rates, the event streams, the published torrents) and the tracker's
`akit_bittorrent_tracker_*`. `akit bt` calls these endpoints, as `akit token` does its own:

```sh
akit bt add -c akit.json -category tv ubuntu.torrent 'magnet:?xt=urn:btih:...'
akit bt ls -c akit.json -state downloading,seeding
akit bt info -c akit.json ubuntu        # by name, ID, or 4 hex digits or more of it
akit bt limit -c akit.json -up 512K -seed-time 12h ubuntu
akit bt alt -c akit.json on
akit bt rm -c akit.json -delete-data ubuntu
```

What akio cannot do yet is refused by name, 501: a sequential download and file priorities
(akio's A287), a torrent's `max_conns` and `upload_slots` (A284). Until akio's A288 a torrent's
trackers are listed with their state unknown, and a recheck adds the torrent again without its
resume data: it checks every piece, and dials its peers again. fit's `fit bt rss` comes with RSS.

## Metrics and tracing

`GET /metrics` serves akit's metrics: upstreams and their addresses, certificates, connections,
reloads, akio's engine, and, with `metrics {}`, requests by listener and route. `telemetry` exports
traces of requests and the same metrics over OTLP/HTTP to a collector
([telemetry](../reference/config.md#telemetry)).

## Not supported yet

fit's assistant is not supported yet: akit refuses it by name. The
[console](console.md), the encrypted store (`admin.store`, which `akit run` keeps the config in, and
the `store` format of `/config/export` and `/config/import`) and `akit run -admin` are supported.
