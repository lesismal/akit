# Guides

[English](README.md) | [简体中文](README.zh-CN.md)

How to do the main things with akit, each guide with configs to start from. The
[config reference](../reference/config.md) has every field; every config in these guides is checked
by a test that runs `akit validate` on it.

- [Getting started](getting-started.md): from nothing to a site served over HTTPS, with files, a
  proxied app and a config changed while it runs.
- [Reverse proxy and load balancing](reverse-proxy.md): upstreams, load balancing, health checks,
  timeouts and retries, canary releases, WebSockets and gRPC, layer 4.
- [Static files and PHP](static-files.md): sites, single-page apps, compression, PHP-FPM.
- [HTTPS and certificates](https.md): HTTPS by default, ACME, certificates of your own and on
  demand, several instances, client certificates.
- [The Admin API and reloads](admin-api.md): SIGHUP, `-watch`, reading and changing the running
  config, rollbacks, metrics.
- [Caching](caching.md): the `cache` middleware, its disk tier and purges.
- [WebAssembly plugins](wasm.md): fit's and Proxy-Wasm plugins, their bounds and cost.

Coming from another server:

- [fit](migrating-from-fit.md): what akit does not run yet, and what to look at.
- [nginx](migrating-from-nginx.md), with `akit adapt -from nginx`.
- [Caddy](migrating-from-caddy.md), with `akit adapt -from caddyfile`.
- [Traefik](migrating-from-traefik.md).
