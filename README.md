# akit

[English](README.md) | [简体中文](README.zh-CN.md)

akit is a reverse proxy, load balancer, TLS terminator, static file server and API gateway written in
Rust on [akio](https://github.com/lesismal/akio), in the same space as nginx, Caddy, Traefik and
[fit](https://github.com/lesismal/fit). It is fit rebuilt on akio, the Rust counterpart of fit's
engine [fib](https://github.com/lesismal/fib): the same features, the same config files in the same
three formats, the same Admin API, with no garbage collector, callbacks inline on thread-per-core
I/O threads, io_uring or epoll chosen per listener, and no `unsafe` of its own.

**Status: before 1.0.** Milestones M0 to M5 are done: the proxy core, dynamic config and TLS,
the gateway's features, providers, the cache and WebAssembly plugins ([progress](docs/progress.md)).
1.0 freezes fit's config and Admin API as v1; fit's tracks beyond the edge proxy come after it.

## Docs

- [Getting started](docs/guides/getting-started.md), and the [guides](docs/guides/README.md): the
  reverse proxy, static files, HTTPS, the Admin API, caching, WebAssembly plugins, and moving from
  fit, nginx, Caddy and Traefik.
- [Config reference](docs/reference/config.md): every field, its type and default, and where akit
  differs from fit.
- [Roadmap](docs/roadmap.md): goals, design principles, milestones, the feature map against fit.
- [Development plan](docs/dev-plan.md): the milestones broken into iterations, one session each.
- [Progress](docs/progress.md): what is done, and where to pick up.
- [Architecture](docs/architecture.md): crates, threads, the runtime graph, a request's path.
- [Development rules](docs/rules.md): what every change must follow.
- [akio API requests](docs/akio-api-requests.md): what akit needs from akio that it lacks.
- [Decisions](docs/decisions/README.md): dependency policy, config compatibility with fit, and the
  rest as they are made.

## What it looks like

A config fit runs, such as fit's [hello](https://github.com/lesismal/fit-examples/blob/main/hello/fit.conf),
runs unchanged:

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

```bash
akit validate -c hello.conf
akit run -c hello.conf
```

## Building

Rust 1.85 or later (akio's MSRV). `make build` builds `target/release/akit`; `make check` runs fmt,
clippy, the tests, cargo-deny and the bilingual docs check, and `make test-changed` tests only the
crates a change reaches. akio is fetched from GitHub at the commit `Cargo.toml` pins. Linux is the first-class platform; macOS is for development; Windows is
not supported.

## License

Apache License 2.0, see [LICENSE](LICENSE).
