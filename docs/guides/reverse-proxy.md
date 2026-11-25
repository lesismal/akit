# Reverse proxy and load balancing

[English](reverse-proxy.md) | [简体中文](reverse-proxy.zh-CN.md)

akit's `proxy` handler sends a route's requests to an upstream, a named set of backends, with load
balancing, health checks, timeouts and retries. Requests go out on akio's asynchronous HTTP clients
and bodies are streamed both ways, with each side's backpressure holding the other: a slow client
or a slow backend makes akit hold a bounded number of bytes for it, never a thread. The
[reference](../reference/config.md#upstreams) has every field.

## An upstream and a route

```fit
listener web :8080 http/1.1 h2c

upstream app {
	addresses 10.0.0.1:8080 10.0.0.2:8080 10.0.0.3:8080
	load_balancing least_conn
}

route api {
	match { path /api/* }
	handler proxy { upstream app }
}
```

The proxy sends what fit sends: `X-Forwarded-For`, `X-Forwarded-Proto` and `X-Forwarded-Host`
(from the client, or kept from a proxy named in `trusted_proxies`), and the client's `Host` unless
`host_header` names another. When the client goes away, the request to the backend is given up.

## Load balancing

`load_balancing` picks an address for each request: `round_robin` (the default), and
`weighted_round_robin` with `weights`; `least_conn`; `random`; `p2c`, the less busy of two picked at
random; and the consistent hashes `ip_hash`, `uri_hash`, `header_hash` and `cookie_hash`, which keep a
client, path, header or cookie on one address while it is up. `sticky_cookie` keeps each client on
the address it first went to with a cookie.

```fit
listener web :8080

upstream app {
	addresses 10.0.0.1:8080 10.0.0.2:8080
	weights 3 1
	load_balancing weighted_round_robin
	sticky_cookie { name app_backend; max_age 1h }
}

route { handler proxy { upstream app } }
```

## Health checks

Passively, an address whose requests fail `max_fails` times within `fail_timeout` is left out for
`fail_timeout`. Actively, `health_check` probes each address on its own schedule, over HTTP or TCP,
and an address is out after `fails` failed probes and back after `passes` good ones. An upstream
with no address in rotation answers 502; a proxy's `failover` names an upstream that takes over then.

```fit
listener web :8080

upstream app {
	addresses 10.0.0.1:8080 10.0.0.2:8080
	max_fails 3
	fail_timeout 30s
	health_check {
		path /healthz
		interval 5s
		timeout 2s
		fails 2
		passes 2
	}
}

upstream maintenance 10.0.0.9:8080

route {
	handler proxy {
		upstream app
		failover maintenance
	}
}
```

## Timeouts, limits and retries

Every exchange with a backend is bounded: `connect_timeout`, `response_header_timeout` (until the
response begins), `response_idle_timeout` (silence within a body) and `response_timeout` (the whole
response), and the route's `timeout` around all the attempts together. A request that times out is
answered 504. Each address takes at most `max_conns` connections and `max_pending` requests waiting
for one; past them a request is answered 503 at once, rather than queued without bound.

`retry` sends a failed request again, to another address: those that could not connect, and with
`statuses` those answered with them. Only a request whose body came whole is retried, and the
retries are kept within a budget (`budget_ratio`, `budget_min_per_second`), so that retries cannot
multiply a backend's load when it struggles.

```fit
listener web :8080

upstream app {
	addresses 10.0.0.1:8080 10.0.0.2:8080
	connect_timeout 3s
	response_header_timeout 10s
	response_idle_timeout 30s
	max_conns 1000
	max_pending 5000
}

route {
	handler proxy {
		upstream app
		timeout 30s
		retry { attempts 2; statuses 502 503 }
	}
}
```

## Canary releases and mirroring

`upstreams` splits a route's requests over several upstreams by weight, and `mirror` sends a copy of
a share of them elsewhere, its answers thrown away, never waited for.

```fit
listener web :8080

upstream app 10.0.0.1:8080
upstream app-next 10.0.0.2:8080
upstream shadow 10.0.0.3:8080

route {
	handler proxy {
		upstreams { upstream app; weight 95 }
		upstreams { upstream app-next; weight 5 }
		mirror { upstream shadow; percent 10 }
	}
}
```

## HTTP/2, TLS, WebSockets and gRPC

An upstream speaks `http/1.1` by default, `h2c` (HTTP/2 in the clear), or `h2` over TLS, checked
against the system's roots or `tls_ca_file`, with a client certificate if it asks for one. WebSocket
upgrades, and HTTP/2's and HTTP/3's extended CONNECTs, go through the same route to an HTTP/1.1
backend, idle for at most `tunnel_idle_timeout`; gRPC calls go to an `h2` or `h2c` backend, streamed
both ways with their trailers, bounded by their `grpc-timeout`.

```fit
listener web :8443 http/1.1 h2 {
	tls certs/example.com.pem certs/example.com.key
}

upstream grpc {
	addresses 10.0.0.5:50051
	protocol h2c
}

upstream secure {
	addresses 10.0.0.6:443
	protocol h2
	tls_server_name api.internal
	tls_ca_file certs/ca.pem
}

route {
	match { header { Content-Type application/grpc* } }
	handler proxy { upstream grpc }
}

route { handler proxy { upstream secure } }
```

## Layer 4

A `tcp` or `udp` listener and a service proxy connections and datagrams to an upstream byte for byte,
with the same load balancing and health checks; a `tcp` listener's services may share it by the
server names of TLS ClientHellos, passed through without being terminated. On Linux, a `tcp`
listener's `sockmap` (akit's own) splices them in the kernel where akio's eBPF relay loads.

```fit
listener db :5432 tcp { sockmap true }
listener tls :443 tcp

upstream postgres 10.0.0.5:5432
upstream mail 10.0.0.25:443
upstream web 10.0.0.26:443

service postgres { listeners db; upstream postgres; idle_timeout 30m }
service mail { listeners tls; upstream mail; server_names mail.example.com }
service web { listeners tls; upstream web; server_names www.example.com example.com }
```

## Upstreams from elsewhere

Upstreams, and the routes to them, may come from Docker containers' labels, a Consul catalog,
Consul's KV store, etcd, or a Kubernetes cluster's Gateway API resources and Ingresses, followed as
they change ([providers](../reference/config.md#providers)).
