# Migrating from nginx

[English](migrating-from-nginx.md) | [简体中文](migrating-from-nginx.zh-CN.md)

akit keeps what nginx is good at, event loops and a small footprint per connection, and changes how
it is configured: one structured config, checked before it runs, and changed in place through the
Admin API without a reload. Its `.conf` format, fit's, is written in blocks of directives as nginx's
is, but its directives are the config's own fields. This guide maps nginx's directives to akit's
config; the [config reference](../reference/config.md) has every field.

## Convert first

`akit adapt` converts a common subset of nginx configs, as `fit adapt` does, and lists, with their
lines, the directives it left out:

```bash
akit adapt -from nginx -c nginx.conf -to fit -o akit.conf
akit validate -c akit.conf
```

It converts the `http` block's servers (`listen`, `server_name`, `ssl_certificate`), their locations
(exact, prefix, `^~` and regular expressions, nested), and in them `proxy_pass` and `upstream`
blocks, `root`, `alias`, `index`, `try_files`, `return`, `rewrite`, `limit_req` with
`limit_req_zone`, `client_max_body_size`, `add_header`, `proxy_set_header` with literal values, and
`gzip`, and `error_log` and `access_log`. Read what it reports, and finish the config by hand with
the table below. What `auth_basic` guards answers 401 until a `basic_auth` middleware with its users
takes its place, so that nothing a password kept is served to anyone.

`error_log` converts to fit's `log`, which akit reads as fit does ([the reference](../reference/config.md#log)):
its file and level are akit's own log's.

For example, this:

```nginx
events {}
http {
    upstream app {
        least_conn;
        server 127.0.0.1:9000;
        server 127.0.0.1:9001;
    }
    server {
        listen 8080;
        server_name example.com;
        gzip on;
        client_max_body_size 10m;
        location /api/ {
            proxy_pass http://app;
        }
        location / {
            root public;
            try_files $uri $uri/ /index.html;
        }
    }
}
```

converts to this config, with a warning for `events`, which has no counterpart:

```fit
listener http_8080 :8080 http/1.1

upstream app 127.0.0.1:9000 127.0.0.1:9001 { load_balancing least_conn }

route example.com-api {
	listeners http_8080
	match { host example.com; path /api/* }
	middleware body_limit { max_bytes 10MiB }
	middleware compress
	handler proxy { upstream app }
}

route example.com {
	listeners http_8080
	match { host example.com; path /* }
	middleware body_limit { max_bytes 10MiB }
	middleware compress
	handler static { root public; try_files {path} {path}/ /index.html }
}
```

A `site` writes the same routes more briefly, when you tidy the result by hand.

## How the parts map

| nginx | akit |
| --- | --- |
| `server { listen 8080; }` | a listener: `listener web { address :8080; protocols http/1.1 h2c }` |
| `listen 443 ssl http2;` with `ssl_certificate` and `ssl_certificate_key` | a listener with `protocols http/1.1 h2` and `tls { certificates { cert_file ...; key_file ... } }`; add `h3` for HTTP/3 |
| `server_name a.example *.example;` | `site a.example *.example { ... }`, or a route's `match.host`; `*.` stands for one label, `**.` for one or more |
| `location = /x` | `match.path: ["/x"]` |
| `location /x/` | `match.path: ["/x/*"]` |
| `location ~ \.php$` | `match.path_regexp: ["(?i)\\.php$"]` |
| `proxy_pass http://app;` and `upstream app { server ...; }` | `handler proxy { upstream app }`, and `upstream app { addresses ... }` |
| `least_conn;`, `ip_hash;`, `hash $cookie_x;` | the upstream's `load_balancing`: `least_conn`, `ip_hash`, `cookie_hash` with `hash_key` |
| `server ... weight=3 max_fails=2 fail_timeout=10s;` | `weights`, `max_fails`, `fail_timeout` |
| `server ... backup;` | the proxy's `failover`: another upstream, used while none of the first is up |
| `proxy_connect_timeout`, `proxy_read_timeout` | `connect_timeout`, `response_header_timeout` and `response_idle_timeout` |
| `keepalive 32;` | `max_idle_conns` (32 by default) |
| `proxy_set_header`, `add_header` | the `headers` middleware's `request` and `response` |
| `root`, `alias`, `index`, `try_files`, `autoindex on;` | the `static` handler's `root`, `strip_prefix`, `index`, `try_files`, `browse` |
| `return 301 https://$host$request_uri;` | the `redirect` middleware: `to https://{host}{uri}; status 301` |
| `rewrite ^/old/(.*)$ /new/$1;` | the `rewrite` middleware's `regexp` and `replacement` |
| `return 200 "ok";` | the `respond` handler |
| `gzip on;`, `gzip_comp_level`, `gzip_min_length` | the `compress` middleware, with zstd and brotli too |
| `limit_req_zone` and `limit_req` | the `rate_limit` middleware: `rate`, `burst`, `key` |
| `limit_conn` | the `concurrency_limit` middleware |
| `client_max_body_size 10m;` | the `body_limit` middleware, or `limits.max_body_bytes` for every listener |
| `auth_basic` and `auth_basic_user_file` | the `basic_auth` middleware, with bcrypt hashes (`htpasswd -B`) |
| `auth_request` | the `forward_auth` middleware |
| `proxy_cache_path` and `proxy_cache` | the `cache` middleware: `zone`, `max_bytes`, and `disk` for a disk tier ([caching](caching.md)) |
| `fastcgi_pass` with `fastcgi_param` | the `fastcgi` handler, with PHP's parameters set for you |
| `stream { server { listen 5432; proxy_pass db; } }` | a listener of `tcp` (or `udp`) and a service naming it and an upstream |
| `ssl_preread` and `map $ssl_preread_server_name` | a service's `server_names`, which passes TLS through by SNI |
| `access_log /var/log/access.log buffer=32k flush=5s;` | `access_log`'s `output`, `buffer_bytes` and `flush_interval`; lines are JSON, text or line, with fields of their own |
| `nginx -s reload` | SIGHUP, `akit run -watch`, or the Admin API's `POST /load` |

## What works differently

- **Which route matches.** nginx tries exact locations, then the longest prefix, then regular
  expressions in order. akit picks by `priority`, then the most specific host, then path, then the
  route with the most other matchers; regular expressions are matchers like any other. Give a route
  a `priority` to put it first.
- **No variables.** akit has no `$variables` or `if`. Matching uses `match` (host, path, method,
  header, query, remote address, or an `expr` combining them); redirects and rewrites use their own
  placeholders (`{host}`, `{uri}`, `$1`).
- **HTTPS by default.** A config with no listeners and routes with host names is served over HTTPS
  on 443 with certificates from Let's Encrypt, and HTTP on 80 redirects to it. Write listeners, or
  set `auto_https.disable`, to keep nginx's behavior.
- **Health checks and upstreams that change.** Active health checks are built in (`health_check`),
  and upstreams may come from Docker, Consul, etcd or Kubernetes (`providers`).
- **Limits are on by default.** Upstream connections, queues and timeouts, request bodies and
  responses left unread all have safe defaults.
- **Dotfiles are not served.** `static` and `fastcgi` answer 404 for a path with a segment starting
  with a dot, as `location ~ /\. { deny all; }` would, but for `/.well-known/`.
- **Not converted**: Lua, `njs`, `map`, `geo`, `split_clients`, `mirror` (the proxy has a `mirror`
  of its own), `sub_filter`, mail proxying, and nginx's modules. WebAssembly plugins and modules
  built in with `akit build` take the place of the extensions.
