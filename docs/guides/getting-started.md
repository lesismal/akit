# Getting started

[English](getting-started.md) | [简体中文](getting-started.zh-CN.md)

This guide takes akit from nothing to a site served over HTTPS, with files, a proxied app and a
config changed while it runs. Each step adds to a config file; the
[config reference](../reference/config.md) has every field. akit reads fit's config, so a config
written for [fit](https://github.com/lesismal/fit) runs on akit unchanged, but for the parts akit
does not support yet, which it refuses by name.

## Install

Build it with Rust 1.85 or later:

```bash
git clone https://github.com/lesismal/akit && cd akit
make build
```

which puts it in `target/release/akit`. A tagged release also has static Linux binaries and macOS
ones, and the repository's Dockerfile builds a container image; [deploy](../../deploy/README.md) has
them, the Helm chart and the systemd units. `akit build` makes an akit with modules of your own
([modules](../modules.md)).

```bash
akit version
```

## A first config

A config may be a `.conf` file, YAML or JSON; this guide uses the `.conf` format, fit's own,
written in blocks of directives. Save this as `akit.conf`:

```fit
listener web {
	address :8080
	protocols http/1.1 h2c
}

route hello {
	match {
		path /hello
	}
	handler respond {
		body "hello from akit\n"
	}
}
```

A listener says where akit listens, and what it speaks there. A route says which requests it takes
(`match`), what they go through (`middleware`) and what answers them (`handler`), each a module with
its params in a block. Check the config, then run it:

```bash
akit validate -c akit.conf
akit run -c akit.conf
```

```bash
curl localhost:8080/hello
```

akit checks a config strictly: an unknown field, a value of the wrong type or a missing one is an
error, with its line and column, and akit does not start. So is a field fit reads that akit does
not yet: `not supported yet: <path>`, never read and ignored. A request no route matches gets 404.

## Serve files

Add a route that serves a directory, after the first:

```fit
listener web {
	address :8080
	protocols http/1.1 h2c
}

route hello {
	match {
		path /hello
	}
	handler respond {
		body "hello from akit\n"
	}
}

route files {
	match {
		path /*
	}
	handler static {
		root ./public
	}
}
```

A path ending in `*` matches a prefix. When several routes match, the most specific wins: `/hello`
before `/*`, whatever their order. `static` serves `index.html` for a directory; `browse` lists one
that has none, and `try_files` serves a single-page app. Files are read on akio's file pool, never on
the threads that serve connections. The [static files guide](static-files.md) has more.

## Proxy to an app

An upstream is a set of backends, by name; the `proxy` handler sends requests to one:

```fit
listener web {
	address :8080
	protocols http/1.1 h2c
}

upstream app {
	addresses 127.0.0.1:9000 127.0.0.1:9001
	health_check {
		path /healthz
		interval 5s
	}
}

route api {
	match {
		path /api/*
	}
	handler proxy {
		upstream app
	}
}

route files {
	match {
		path /*
	}
	handler static {
		root ./public
	}
}
```

Requests to `/api/` go to the two backends in turn, and a backend that fails its health check is
left out until it passes again. WebSockets and gRPC go through the same way. Every connection, queue
and timeout has a safe default, which the upstream's fields change. The
[reverse proxy guide](reverse-proxy.md) has more.

## HTTPS

Write the routes in a site named by its host names, and leave the listeners out:

```fit
upstream app {
	addresses 127.0.0.1:9000 127.0.0.1:9001
}

site example.com {
	route /api/* {
		handler proxy { upstream app }
	}
	route /* {
		handler static { root ./public }
	}
}
```

A site's routes match its host names, and each its own paths. akit then listens on 443 for HTTPS,
with a certificate for `example.com` from Let's Encrypt, which it renews, and on 80 only to redirect
to HTTPS. The name's DNS must point at the host, and ports 80 and 443 reach it. `acme` sets the CA
and where certificates are kept, `auto_https` the ports, or turns it off. To use certificates of
your own, write the listener, and name it in the site:

```fit
listener web {
	address :443
	protocols http/1.1 h2 h3
	tls {
		certificates {
			cert_file certs/example.com.pem
			key_file certs/example.com.key
		}
	}
}

site example.com {
	listeners web
	route /* {
		handler static { root ./public }
	}
}
```

akit loads the files again when they change, without closing the listener. The
[HTTPS guide](https.md) has more.

## Middleware

Middleware runs in order before the handler. In a site, middleware runs for each of its routes;
this compresses responses, and adds a field to each:

```fit
site example.com {
	middleware compress
	middleware headers {
		response {
			set {
				Strict-Transport-Security "max-age=31536000"
			}
		}
	}
	route /* {
		handler static { root ./public }
	}
}
```

The [reference](../reference/config.md#middleware) lists them all: rewrites and redirects, rate and
concurrency limits, Basic auth, JWTs, forward auth, client certificates, a circuit breaker, a cache
([caching](caching.md)), WebAssembly plugins ([WebAssembly](wasm.md)).

## Several files

As a config grows, split it: `-c` takes a directory, and every `.conf`, `.yaml`, `.yml` and `.json`
file in it, in its subdirectories too, is read, the routes and upstreams of each joined:

```bash
akit run -c /etc/akit
```

with, say, `/etc/akit/base.conf` holding the listeners and `acme`, and a file in `/etc/akit/sites/`
for each site. A field other than the lists, as `admin` or `acme`, is given in one file only. `-c`
may also be given more than once.

## Change it while it runs

`akit run` applies the config again on SIGHUP, or each time one of its files changes with `-watch`,
a file added to its directory or taken out included. A config that does not load leaves the running
one in place, and says why. Listeners that did not change keep their connections, and requests under
way finish on the config they started with.

With `admin {}` in the config, the Admin API, on 127.0.0.1:2019 by default, reads and changes the
running config, in fit's native JSON: here, the whole of it, then the first route's params, by their
JSON pointer, then a whole new config, converted from the `.conf` file by `akit adapt`, and back to
the first:

```bash
curl localhost:2019/config
curl -X PATCH -H 'Content-Type: application/merge-patch+json' \
  -d '{"body": "hello again\n"}' localhost:2019/config/routes/0/handler/params
akit adapt -c akit.conf -o akit.json
curl -X POST -H 'Content-Type: application/json' -d @akit.json localhost:2019/load
curl -X POST localhost:2019/rollback/1
```

It keeps the configs it applied, to read again and roll back to, and serves metrics at `/metrics`
and a dashboard at `/dashboard/`. The [Admin API guide](admin-api.md) has more.

## Next

- The [config reference](../reference/config.md): every field, its type and default, and where
  akit differs from fit.
- The [guides](README.md): the reverse proxy, static files, HTTPS, the Admin API, caching and
  WebAssembly plugins.
- Coming from another server: [fit](migrating-from-fit.md), [nginx](migrating-from-nginx.md),
  [Caddy](migrating-from-caddy.md), [Traefik](migrating-from-traefik.md).
- [Examples](https://github.com/lesismal/fit-examples): a config for each of fit's features; akit
  runs those it supports.
- [Deploy](../../deploy/README.md): the release archives, the container image, Kubernetes and
  systemd.
