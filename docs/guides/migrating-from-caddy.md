# Migrating from Caddy

[English](migrating-from-caddy.md) | [简体中文](migrating-from-caddy.zh-CN.md)

akit shares Caddy's ideas: HTTPS by default, a strict structured config, and an Admin API that swaps
configs atomically. It differs in what is built in (caching, rate limiting, layer 4 proxying, which
Caddy leaves to plugins) and in costing less per connection. Its `.conf` format, fit's, is written in
blocks of directives as a Caddyfile is, with a `site` for each set of host names, but its directives
are the config's own fields. `akit adapt` converts a Caddyfile's common subset to it, as `fit adapt`
does; there is no converter from Caddy's JSON. The [config reference](../reference/config.md) has
every field.

## A site

```caddyfile
example.com {
	encode zstd gzip
	handle_path /api/* {
		reverse_proxy 127.0.0.1:9000 127.0.0.1:9001 {
			lb_policy least_conn
			health_uri /healthz
		}
	}
	root * public
	file_server
}
```

is converted by `akit adapt -from caddyfile` to this config (Caddy's Admin API, on by default,
becomes akit's `admin {}`):

```fit
route example.com-api {
	match { host example.com; path /api/* }
	middleware compress { encodings zstd gzip }
	middleware rewrite { strip_prefix /api }
	handler proxy { upstream proxy_127_0_0_1_9000 }
}

route example.com {
	match { host example.com }
	middleware compress { encodings zstd gzip }
	handler static { root public }
}

upstream proxy_127_0_0_1_9000 127.0.0.1:9000 127.0.0.1:9001 {
	load_balancing least_conn
	health_check { path /healthz }
}

admin {}
```

and may be written by hand, more briefly, with a site:

```fit
upstream app {
	addresses 127.0.0.1:9000 127.0.0.1:9001
	load_balancing least_conn
	health_check {
		path /healthz
	}
}

site example.com {
	middleware compress
	route /api/* {
		middleware rewrite { strip_prefix /api }
		handler proxy { upstream app }
	}
	route /* {
		handler static { root public }
	}
}
```

As with Caddy, a config that writes no listeners is served over HTTPS on 443, with certificates from
Let's Encrypt for the routes' host names, and port 80 redirects to it.

## Converting a Caddyfile

```bash
akit adapt -from caddyfile -c Caddyfile -to akit -o akit.conf
akit validate -c akit.conf
```

writes a `.conf` file and lists on standard error, with their lines, the directives it left out or
converted with doubts. It converts global options (`email`, `acme_ca`, `acme_ca_root`,
`storage file_system`, `admin`, `auto_https`, `http_port`, `https_port`, `on_demand_tls`,
`servers { protocols }`, `log`, `debug`), snippets and their `import`, environment placeholders,
sites by their addresses, matchers (paths, `path_regexp`, `host`, `method`, `header`, `query`,
`remote_ip`, `client_ip`, `not`), and the directives `handle`, `handle_path`, `route`,
`reverse_proxy`, `file_server`, `root`, `try_files`, `respond`, `redir`, `rewrite`, `uri`, `header`,
`request_header`, `encode`, `basic_auth`, `forward_auth`, `request_body`, `php_fastcgi`, `tls` and
`log`.

A global `log` or `debug` converts to fit's `log`, akit's own log, which akit reads as fit does
([the reference](../reference/config.md#log)). A site's `log` is the access log.

## How the parts map

| Caddy | akit |
| --- | --- |
| a site address, `example.com` | `site example.com { ... }`, whose routes match the host; listeners made by HTTPS by default, or written |
| `:8080` | a listener: `listener web { address :8080; protocols http/1.1 h2c }` |
| matchers: `path`, `host`, `method`, `header`, `query`, `remote_ip`, `path_regexp`, `expression` | a route's `match { ... }`, with fields of the same names, and `expr` |
| `handle`, `route` and their order | routes, picked by `priority`, then the most specific host and path |
| `handle_path /api/*` | `route /api/* { middleware rewrite { strip_prefix /api } ... }` |
| `reverse_proxy` | `handler proxy { upstream app }` and an `upstream app { ... }` |
| `lb_policy` | the upstream's `load_balancing` |
| `health_uri`, `health_interval`, `fail_duration`, `max_fails` | `health_check`, `fail_timeout`, `max_fails` |
| `transport http { versions h2c }` | the upstream's `protocol`: `h2c`, or `h2` over TLS |
| `file_server`, `root`, `browse`, `try_files`, `precompressed` | the `static` handler's `root`, `browse`, `try_files`, `precompressed` |
| `encode` | the `compress` middleware |
| `header`, `request_header` | the `headers` middleware |
| `redir` | the `redirect` middleware |
| `rewrite`, `uri strip_prefix` | the `rewrite` middleware |
| `respond` | the `respond` handler |
| `basicauth` | the `basic_auth` middleware, with bcrypt hashes as Caddy's are |
| `forward_auth` | the `forward_auth` middleware |
| `php_fastcgi` | the `fastcgi` handler, with `split_path`, `index` and `try_files` |
| `tls cert.pem key.pem` | a listener's `tls.certificates` |
| `tls { on_demand }` and `on_demand_tls { ask ... }` | a listener's `tls.on_demand` and `acme.on_demand` |
| `email`, `acme_ca`, `storage` (Redis, Consul) | `acme`'s `email`, `directory`, `storage`, and `shared_storage` for Redis, etcd or Consul |
| `log` in a site | `access_log`, with its `output`, `level` and `format` |
| `metrics` | `metrics`, served by the Admin API at `/metrics` |
| `tracing` | `telemetry`, over OTLP |
| plugins: rate limiting, caching, layer 4 | built in: `rate_limit`, `cache`, `tcp` and `udp` listeners with services |
| `xcaddy build --with ...` | `akit build` ([modules](../modules.md)) |
| `caddy run --watch`, `caddy reload` | `akit run -watch`, SIGHUP or the Admin API's `POST /load` |
| `caddy validate`, `caddy hash-password` | `akit validate`; `htpasswd -B` for the hashes |

## The Admin API

akit's Admin API is Caddy's in shape: `GET`, `PUT`, `PATCH` and `DELETE` on `/config/` and a path
into it, `POST /load`, and ETags with `If-Match` against lost updates. A path is a JSON pointer's
tokens (`/config/routes/0/handler`), with no `@id` shortcuts; `PATCH` is a JSON merge patch; akit
keeps the configs it applied (`GET /versions`), and `POST /rollback/{n}` applies one again. It listens
on 127.0.0.1:2019, or a Unix socket, and takes a bearer token (`admin.token_file`); it is not served
over TLS yet ([the Admin API](admin-api.md)).

## What works differently

- **The config's shape.** akit's config has listeners, routes, upstreams and services at its top,
  rather than apps and servers; a route is a flat list of middleware and one handler, not nested
  handlers.
- **Rewrites do not re-route.** `rewrite` and `uri` rewrite the path after the route is picked.
- **Dotfiles are not served.** `static` and `fastcgi` answer 404 for a path with a segment starting
  with a dot, but for `/.well-known/`, where Caddy serves them; `dotfiles allow` serves them.
- **Not there**: Caddy's internal CA (`tls internal`), DNS challenges (akit answers HTTP-01 and
  TLS-ALPN-01), `templates`, `map`, `handle_errors` and Caddy's events. A module built in with
  `akit build`, or a WebAssembly plugin, takes the place of a plugin.
