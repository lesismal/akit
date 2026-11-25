# Caching

[English](caching.md) | [简体中文](caching.zh-CN.md)

The `cache` middleware keeps a route's responses and answers from them, as RFC 9111 has a shared
cache work: what it keeps and for how long is what the responses' `Cache-Control`, `Expires`,
`Vary` and validators say. It keeps them in memory, in a zone the routes that name it share, and
with `disk` on disk too. A response is kept as it goes to the client, never held back to be stored,
streamed bodies included. The [reference](../reference/config.md#cache) has every field.

## A cache in front of an app

```fit
listener web :8080

upstream app 127.0.0.1:9000

route {
	middleware cache {
		zone app
		max_bytes 512MiB
		max_entry_bytes 8MiB
		default_ttl 5m
	}
	handler proxy { upstream app }
}
```

It keeps the answers to `GET` requests that a shared cache may keep: not `no-store`, `private` or
`no-cache`, setting no cookie, no larger than `max_entry_bytes`, fresh for `s-maxage`, `max-age` or
`Expires`, or for `default_ttl` when the response says nothing and its status allows it. A response to
a request with `Authorization` is kept only when it says `public`, `s-maxage` or `must-revalidate`. A
`POST`, `PUT` or `DELETE` that succeeds drops what was kept for its URI. Every answer says what the
cache did in `Cache-Status` (RFC 9211): a hit, a miss, stale, or stored.

The zone is bounded by `max_bytes`; past it the least recently used responses go. Each encoding a
`compress` middleware after the cache makes is a variant of its own.

## Many clients at once, and a failing upstream

When many requests miss on the same response at once, one fetches it and the rest wait for it (the
cache lock), so that a burst of clients makes one request to the upstream; `lock_timeout` bounds the
wait, and `disable_lock` turns it off. `stale_while_revalidate` answers at once with a stale response
while it is fetched again in the background, and `stale_if_error` answers with it when fetching fails,
for responses that do not say so themselves (RFC 5861).

```fit
listener web :8080

upstream app 127.0.0.1:9000

route {
	match { path /api/catalog/* }
	middleware cache {
		zone catalog
		max_bytes 256MiB
		lock_timeout 5s
		stale_while_revalidate 30s
		stale_if_error 10m
	}
	handler proxy { upstream app }
}
```

## On disk

`disk` adds a disk tier to the zone: every response kept in memory is written to disk too, on a pool
of threads of the cache's own, and one memory no longer holds is read back from disk while its
request waits, never holding one of the threads that serve connections. `max_bytes` and
`max_entries` bound it.

```fit
listener web :8080

upstream assets 127.0.0.1:9001

route {
	match { path /assets/* }
	middleware cache {
		zone assets
		max_bytes 256MiB
		default_ttl 1h
		disk { path cache/assets; max_bytes 10GiB; max_entries 1000000 }
	}
	handler proxy { upstream assets }
}
```

## Purging

The Admin API removes what a zone keeps: by URL, by URL prefix, by tag (the `Cache-Tag` field of the
responses), or all of it; `GET /cache` says what each zone keeps.

```bash
curl localhost:2019/cache
curl -X POST -H 'Content-Type: application/json' localhost:2019/cache/purge \
  -d '{"zone": "assets", "prefixes": ["https://example.com/assets/v1/"]}'
curl -X POST -H 'Content-Type: application/json' localhost:2019/cache/purge \
  -d '{"zone": "catalog", "tags": ["product-42"]}'
```

The answer, `{"purged": n}`, says how many responses went.
