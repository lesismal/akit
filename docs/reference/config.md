# Config reference

[English](config.md) | [简体中文](config.zh-CN.md)

<!-- The tables between BEGIN GENERATED and END GENERATED are written by
tools/config-reference.py from fit's pinned schema, tests/compat/supported.txt (held equal to
akit-config's lists of fields) and tools/config-reference-notes.json; do not edit them. Regenerate
with: python3 tools/config-reference.py. The prose and the examples around them are written by hand;
every example is checked by crates/akit/tests/docs_examples.rs, which runs akit validate on it. -->

Every field of akit's config: what each part of a config is for, an example of it, and a table of
its fields, with each field's type, default and description. akit reads fit's config
([decision 002](../decisions/002-config-compatibility.md)): the same fields, defaults and meanings,
in the same three formats, a [.conf file](https://github.com/lesismal/fit/blob/main/docs/reference/fit.md),
YAML or JSON, any of them split across several files. Where akit differs from fit, the table says
so in a note marked **akit:**; nine defaults that name the gateway are akit's, `akit` where fit's
say `fit` ([decision 037](../decisions/037-akit-names.md)).

<!-- BEGIN GENERATED: summary -->
akit reads **504** of the 3821 fields of fit's config (fit `7daee573`, counted as [the compatibility page](../compatibility.md) counts them), refuses the rest by name as not supported yet, and adds **1** of its own. Of the 76 objects akit reads, each has its table below.
<!-- END GENERATED -->

How to read the tables:

- A field left out takes its default, when it has one; a field marked *required* must be written.
- A duration is a string such as `"1.5s"` or `"2m"`; bytes or percent is an integer of bytes or a
  share of memory such as `"50%"`; in a `.conf` file a size may be written `64KiB` or `1GiB`.
- `object` links to the object's own table.
- **Not supported yet**: a field fit reads and akit does not yet. akit refuses it by name, with
  `not supported yet: <path>`, and does not start; it never reads a field and ignores it. The
  [compatibility page](../compatibility.md) lists every field of fit's with whether akit reads it,
  the fields of the parts akit refuses whole among them.
- **akit's extension**: a field fit does not have ([below](#akits-extensions)).

Every example on this page is a whole config, checked by a test that runs `akit validate` on it.
Files they name are relative to the directory akit runs in: `public/` (a directory to serve),
`certs/example.com.pem` and `certs/example.com.key` (a certificate and its key), `certs/ca.pem` (a
CA), `admin.token` (a token), `lists/deny.txt` (a list of addresses), and `plugins/headers.wasm` and `plugins/http_headers.wasm` (WebAssembly plugins
of fit's ABI and of Proxy-Wasm's). `akit validate`
opens what a config names, as `akit run` does: a certificate that does not load, a static root that
is not there, a plugin that does not compile are errors.

## Contents

- [The config](#the-config)
- [Listeners](#listeners): [`listeners[]`](#cfg-listeners), [`tls`](#cfg-listeners-tls), [`tls.certificates[]`](#cfg-listeners-tls-certificates), [`tls.client_auth`](#cfg-listeners-tls-client-auth), [`socks`](#cfg-listeners-socks), [`proxy_protocol`](#cfg-listeners-proxy-protocol)
- [Routes](#routes): [`routes[]`](#cfg-routes), [`match`](#cfg-routes-match), [`handler`](#cfg-routes-handler), [`middleware[]`](#cfg-routes-middleware)
- [Upstreams](#upstreams): [`upstreams[]`](#cfg-upstreams), [`sticky_cookie`](#cfg-upstreams-sticky-cookie), [`health_check`](#cfg-upstreams-health-check)
- [Services](#services)
- [Access lists](#access-lists): [`access`](#cfg-access), [`listeners[].access`](#cfg-listeners-access), [`routes[].access`](#cfg-routes-access), [`services[].access`](#cfg-services-access), [`admin.access`](#cfg-admin-access)
- [Access log](#access-log)
- [Log](#log): [`log`](#cfg-log), [`rotate`](#cfg-log-rotate), [`loggers`](#cfg-log-loggers), [`loggers.*.rotate`](#cfg-log-loggers-rotate)
- [Telemetry](#telemetry)
- [Limits](#limits)
- [Engine](#engine)
- [Admin API](#admin-api): [`admin`](#cfg-admin), [`console`](#cfg-admin-console), [`console.users[]`](#cfg-admin-console-users), [`console.access`](#cfg-admin-console-access)
- [HTTPS by default](#https-by-default)
- [Metrics](#metrics)
- [ACME](#acme): [`acme`](#cfg-acme), [`on_demand`](#cfg-acme-on-demand), [`shared_storage`](#cfg-acme-shared-storage)
- [Providers](#providers): [`docker`](#cfg-providers-docker), [`consul`](#cfg-providers-consul), [`consul_kv`](#cfg-providers-consul-kv), [`etcd`](#cfg-providers-etcd), [`kubernetes`](#cfg-providers-kubernetes)
- [Handlers](#handlers): [`respond`](#respond), [`static`](#static), [`proxy`](#proxy), [`fastcgi`](#fastcgi), [`forward_proxy`](#forward_proxy)
- [Middleware](#middleware): [`basic_auth`](#basic_auth), [`body_limit`](#body_limit), [`cache`](#cache), [`circuit_breaker`](#circuit_breaker), [`client_cert`](#client_cert), [`compress`](#compress), [`concurrency_limit`](#concurrency_limit), [`forward_auth`](#forward_auth), [`headers`](#headers), [`jwt`](#jwt), [`rate_limit`](#rate_limit), [`redirect`](#redirect), [`rewrite`](#rewrite), [`wasm`](#wasm)
- [akit's extensions](#akits-extensions)
- [What akit does not read yet](#what-akit-does-not-read-yet)

## The config

A config is one object. Nothing in it is required: a config of a route alone, whose match names a
host, is served over HTTPS with a certificate obtained for that host ([HTTPS by
default](#https-by-default)). Names are matched exactly; an unknown field, a value of the wrong type
and a key written twice are errors, each reported with its file, line and column, and akit does not
start (or, on a reload, keeps the config it runs).

```fit
listener web :8080 http/1.1 h2c

route hello {
	match { path /hello }
	handler respond { body "hello from akit\n" }
}
```

The same config in YAML:

```yaml
listeners:
  - name: web
    address: :8080
    protocols: [http/1.1, h2c]
routes:
  - name: hello
    match:
      path: [/hello]
    handler:
      module: respond
      params:
        body: "hello from akit\n"
```

and in JSON, the form the Admin API reads and writes:

```json
{
  "listeners": [{"name": "web", "address": ":8080", "protocols": ["http/1.1", "h2c"]}],
  "routes": [
    {"name": "hello", "match": {"path": ["/hello"]},
     "handler": {"module": "respond", "params": {"body": "hello from akit\n"}}}
  ]
}
```

`akit adapt` converts between the three, as `fit adapt` does, and `akit fmt` writes a config's
files again in their own formats. A `.conf` file's `import` and `include` are not supported yet:
name every file on the command line (`-c` may be given again, or name a directory).

### `config`

<!-- BEGIN GENERATED: config -->
<a id="cfg-config"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `access` | [object](#cfg-access) |  | The client addresses akit serves, by allow (whitelist) and deny (blacklist) lists, for every listener, route, service and role, the Admin API and the consoles, that sets no access of its own that decides: a more specific one, set on a listener, route, service, role, admin or a console, decides first, and this one only for an address that one has no say on. Every address is served when left out. |
| `access_log` | [object](#cfg-access-log) |  | Where a line is written for each request, as JSON or, as its format says, text or line; no access log when left out. |
| `acme` | [object](#cfg-acme) |  | How certificates for the names listeners list in tls.acme are obtained and renewed; the defaults when left out. Its settings are read when akit starts: a change to them takes effect when akit restarts. The names follow every reload. |
| `admin` | [object](#cfg-admin) |  | The Admin API, through which the running config is read and replaced, and which serves metrics in Prometheus's text format at /metrics; off when left out. Its settings are read when akit starts: a change to them takes effect when akit restarts. |
| `auto_https` | [object](#cfg-auto-https) |  | HTTPS by default: a config that writes no listeners is served over HTTPS on port 443, with certificates obtained with acme for the host names its routes match, and over HTTP on port 80 only to be redirected; a listener whose tls is empty gets certificates for the host names of the routes it serves. On when left out. |
| `bittorrent` | object |  | **Not supported yet**: refused by name. fit's BitTorrent roles, beside the gateway on its engine: a client that downloads and seeds torrents on one port for TCP and UDP, and a tracker. None when left out: no BitTorrent port and no state on disk. |
| `engine` | [object](#cfg-engine) |  | How akio, the network engine akit serves and dials on, is set up: its event loops, workers and buffers. akio's own defaults for what is left out, and for all of it when engine is. Read when akit starts: a change takes effect when akit restarts. |
| `limits` | [object](#cfg-limits) |  | Limits that apply to every listener. |
| `listeners` | list of [object](#cfg-listeners) |  | The addresses akit listens on, and the protocols each one serves. When left out, HTTPS by default makes them from the routes' host names; see auto_https. |
| `llm` | object |  | **Not supported yet**: refused by name. The LLM feature's roles to run beside the gateway, on its engine: a hub, a node and a proxy, which share AI models among machines and serve them to agents. None when left out. What their web pages change is written back here. |
| `log` | [object](#cfg-log) |  | akit's own log: what it says about starting, reloading, certificates, providers and the engine, by logger, each with the least level written, and the format and output it writes in, as akio writes its own. Messages at info and debug to stdout and the rest to stderr, at info and above, as lines, when left out. |
| `media_auth` | list of object |  | **Not supported yet**: refused by name. Named policies of who may publish and play which streams, with tokens, users and keys for tokens fit signs: the handlers and roles that take media (whip, whep, webrtc_proxy) refer to one with their auth param. A handler without auth lets everyone in, as the route's middleware allow. |
| `media_library` | list of object |  | **Not supported yet**: refused by name. Named libraries of media files: directories scanned into a catalog of items, each a title with its renditions, audio, subtitles and poster, probed off fib's workers on a bounded file pool of the library's own, and read by the Admin API's /media/library. None when left out: no files are scanned or served. |
| `metrics` | object |  | Count and time requests by listener and route, for the Admin API's /metrics, which serves the rest of akit's metrics without it; off when left out, since it costs every request a little. Needs admin. |
| `pprof` | object |  | **Not supported yet**: refused by name. Serve Go's runtime profiles, as net/http/pprof does, at the Admin API's /debug/pprof/: CPU, heap, allocations, goroutines, blocking, mutexes and execution traces, for go tool pprof and go tool trace. Off when left out: the endpoints answer 404, and blocking and mutexes are not sampled. Follows every config applied, with no restart. Needs admin; served on the Admin API's own address, behind its token, and not through the console. |
| `providers` | list of [object](#cfg-providers) |  | Sources of routes, upstreams, services and listeners other than this file and the Admin API, merged into the config: what this config writes wins over every provider, and a provider over those listed after it; a provider's element named as one before it is left out. The Admin API's /config/effective shows the result and where each part came from. Read when akit starts: a change takes effect when akit restarts. |
| `routes` | list of [object](#cfg-routes) |  | How requests are matched, and what handles them. |
| `schema_version` | integer, 1 to 1 | `1` | The version of akit's config this one is written for, 1; akit refuses a config written for a version it does not read, rather than read it wrongly. Optional: a config that leaves it out is read as version 1. |
| `services` | list of [object](#cfg-services) |  | Layer 4 services: the upstreams the connections of tcp listeners are proxied to, byte for byte, and the datagrams of udp listeners, one for one. |
| `streaming` | object |  | **Not supported yet**: refused by name. fit's streaming role, beside the gateway on its engine: one media hub where a stream is published once, over RTMP, RTSP, SRT or WHIP, and played over any of them, HTTP-FLV and HLS too, each listener and handler with its media_auth policy. None when left out. |
| `sync` | object |  | **Not supported yet**: refused by name. fit's sync role: pipelines that read records from a source (a database's changes, files, a queue) and write them to sinks, each with a checkpoint saved only once every sink has acknowledged the records it covers, kept in state_dir. None when left out: no connections out and no state on disk. |
| `telemetry` | [object](#cfg-telemetry) |  | OpenTelemetry: traces of requests, with W3C trace context sent on to upstreams, and metrics, exported over OTLP/HTTP to a collector. Neither when left out. Read when akit starts: a change takes effect when akit restarts. |
| `tunnel` | object |  | **Not supported yet**: refused by name. The tunnel's roles to run beside the gateway, on its engine: a hub, a node and a proxy, which reach services behind NAT through the hub. None when left out. What their web pages change is written back here. |
| `upstreams` | list of [object](#cfg-upstreams) |  | The servers requests can be proxied to, by name. |
| `webrtc` | object |  | **Not supported yet**: refused by name. What fit runs for WebRTC beside the gateway, on its engine: a TURN server, the media port of the whip and whep handlers, and the relay of the webrtc_proxy handler. None when left out. |
<!-- END GENERATED -->

## Listeners

A listener is an address and the protocols served there. Routes and services refer to listeners by
`name`; a route that names none serves every listener.

- **Protocols.** `http/1.1` and `h2c` (HTTP/2 in the clear) without TLS; with `tls`, `h2` (HTTP/2,
  chosen through ALPN) and `h3` (HTTP/3 over QUIC, on the same port in UDP, announced with
  `Alt-Svc`). `tcp` proxies raw connections to a [service](#services), `udp` datagrams, and
  `socks` is a SOCKS5 and SOCKS4 forward proxy. A listener that names none serves `http/1.1`, and
  `http/1.1` and `h2` with TLS. Several may share a port: akit tells them apart by what a client
  sends first.
- **TLS.** `tls` makes the listener's HTTP HTTPS, with certificates from files, names to obtain
  certificates for with [ACME](#acme), or certificates obtained during the handshake
  (`on_demand`). An empty `tls`, `{}`, gets certificates for the host names of the routes the
  listener serves. `client_auth` asks clients for certificates (mutual TLS). Certificate files are
  loaded again when they change (`akit run -watch-certs`, on by default), without closing the
  listener.
- **Behind a proxy.** `proxy_protocol` takes the PROXY protocol header from the proxies it trusts,
  and serves each connection as from the client the header names.
- **In the kernel.** `sockmap`, akit's own, splices a `tcp` listener's proxied connections in the
  kernel where akio's eBPF relay loads.

A reload keeps a listener whose address, protocols and TLS settings are unchanged, with its
connections; one that changes is bound anew and the old one drained.

```fit
listener web :8080 http/1.1 h2c

listener secure :8443 http/1.1 h2 h3 {
	tls certs/example.com.pem certs/example.com.key
}

listener internal 127.0.0.1:8081 { proxy_protocol { trusted 10.0.0.0/8 } }

route { listeners secure; handler respond { body secure } }
route { listeners web internal; handler respond { body plain } }
```

### `listeners[]`

<!-- BEGIN GENERATED: listeners[] -->
<a id="cfg-listeners"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `address` | string | required | The host and port to listen on, as in ":8080" or "127.0.0.1:8080". |
| `name` | string | required | A name unique among the listeners, by which routes refer to this one. |
| `access` | [object](#cfg-listeners-access) |  | The client addresses the listener serves, deciding before the global access: a connection it refuses is closed as soon as it is accepted, or, for HTTP/3 and udp, its datagrams dropped, unless a route or service of the listener sets an access of its own, which then decides first, for its requests or connections. With proxy_protocol the address is the one the header names. **akit:** HTTP/3's requests are checked one by one, a refused one answered `403`, where fit drops a refused peer's datagrams before a session is made: akio's UDP listeners take no accept filter yet (akit's request A220). |
| `mptcp` | boolean |  | **Not supported yet**: refused by name. Listen the listener's TCP connections with Multipath TCP where the kernel offers it (Linux 5.6+, net.mptcp.enabled), falling back to plain TCP where it does not; a client without Multipath TCP falls back too. Off by default: a listener is plain TCP. udp listeners and HTTP/3 take none. |
| `protocols` | list of string: `http/1.1`, `h2c`, `h2`, `h3`, `tcp`, `udp`, `socks` |  | The protocols served: http/1.1, h2c (HTTP/2 in the clear) and, with tls, h2 (HTTP/2 over TLS, chosen through ALPN) and h3 (HTTP/3 over QUIC, on the same port in UDP, announced to HTTP/1 and HTTP/2 clients with Alt-Svc); tcp, raw TCP, whose connections the services that list the listener proxy to their upstreams; socks, a SOCKS5 and SOCKS4 forward proxy, as socks says; or udp alone, whose datagrams a service proxies. A listener that serves more than one of HTTP, tcp and socks tells them apart on its port by what a client sends first: a TLS ClientHello goes to the service whose server_names hold its name, or else, with tls, is served HTTPS, or else goes to the service without server_names; a SOCKS greeting to socks; an HTTP request, in the clear, to HTTP; and whatever else to the service without server_names, or it is closed. A client that waits for the server to speak first is closed after 10s: such a protocol needs a listener of its own. http/1.1 alone when left out, or http/1.1 and h2 with tls. **akit:** With `h2c` and no `http/1.1` the listener serves HTTP/2 with prior knowledge alone, as fit's (I79b): a client that sends anything else gets akio's `GOAWAY` (`PROTOCOL_ERROR`) before the connection closes, where fit closes it with nothing (akio's ADR-174). |
| `proxy_protocol` | [object](#cfg-listeners-proxy-protocol) |  | Take a PROXY protocol header, of version 1 or 2, at the start of each connection from a trusted address, and serve the connection as from the client it names. For listeners of tcp and of HTTP over TCP; with tls the header comes before TLS. HTTP/3 takes none. |
| `sockmap` | boolean | `false` | **akit's extension** ([decision 010](../decisions/010-sockmap-relay.md)). The connections of the `tcp` services this listener accepts are spliced in the kernel by akio's eBPF sockmap relay where it loads (Linux 6.15 or later, with `CAP_BPF` and `CAP_NET_ADMIN`), and relayed in userspace where it does not. Only for a listener whose protocols include `tcp`. A reload that changes it applies to the connections accepted after it. fit refuses it as an unknown field. |
| `socks` | [object](#cfg-listeners-socks) |  | Who may use the listener's socks, and where it connects them: needed with socks. It serves CONNECT, without authentication, and refuses BIND and UDP ASSOCIATE. |
| `tls` | [object](#cfg-listeners-tls) |  | Serve TLS, with these certificates: the listener's HTTP is HTTPS. |
<!-- END GENERATED -->

### `listeners[].tls`

<!-- BEGIN GENERATED: listeners[].tls -->
<a id="cfg-listeners-tls"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `acme` | list of string |  | Host names to obtain certificates for with ACME, as the top-level acme says, through its challenges: the CA must reach akit at each name on port 80, where any listener of akit's in the clear answers HTTP-01, or on port 443, where this listener answers TLS-ALPN-01. A client asking for one of these names is given its certificate once there is one; until then, and for other names, the certificates from files. |
| `certificates` | list of [object](#cfg-listeners-tls-certificates) |  | Certificates from files. A client is given the first whose names match the one it asks for, or the first of all when none does or it asks for none. |
| `client_auth` | [object](#cfg-listeners-tls-client-auth) |  | Ask clients for certificates and verify them (mutual TLS). |
| `kernel_tls_offload` | boolean |  | **Not supported yet**: refused by name. Hand the encryption of what this listener's connections send to the kernel's TLS right after their handshakes (true) or only when one sends a large file (false), whatever engine.kernel_tls_offload says; as engine.kernel_tls_offload when left out. For a machine whose network card encrypts TLS, where a listener of large files gains from it and one of small API responses may not. The card takes TLS 1.2 connections only (see max_version); the kernel encrypts TLS 1.3 ones itself. A reload that changes it binds the listener anew. **akit:** It comes with the `engine` settings. |
| `max_version` | string: `1.2`, `1.3` |  | The newest TLS version this listener's connections may use: 1.2 or 1.3; 1.3 when left out. Network cards that encrypt TLS (kernel_tls_offload) take TLS 1.2 connections only, Linux's offload having none for 1.3, so a listener meant for such a card sets 1.2; under 1.3 the kernel encrypts instead, at the CPU's cost. TLS 1.2 takes a round trip more to connect than 1.3. HTTP/3, which needs TLS 1.3, keeps it. A reload that changes it binds the listener anew. |
| `on_demand` | boolean |  | Obtain a certificate during the handshake for a name a client asks for that neither the files nor acme have, as the top-level acme.on_demand allows; needs it. |
<!-- END GENERATED -->

### `listeners[].tls.certificates[]`

<!-- BEGIN GENERATED: listeners[].tls.certificates[] -->
<a id="cfg-listeners-tls-certificates"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `cert_file` | string | required | A PEM file with the certificate, followed by any intermediates. |
| `key_file` | string | required | A PEM file with the certificate's private key. |
<!-- END GENERATED -->

### `listeners[].tls.client_auth`

<!-- BEGIN GENERATED: listeners[].tls.client_auth -->
<a id="cfg-listeners-tls-client-auth"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `ca_files` | list of string | required | PEM files of the certificate authorities a client's certificate must chain to. They are watched as certificate files are: a change loads the config again. |
| `mode` | string: `require`, `verify_if_given` | `"require"` | require: a client without a valid certificate is refused in the handshake; verify_if_given: a client may send none, and a route that needs one says so with the client_cert middleware, but one it sends must be valid. require when left out. A listener that requires certificates cannot answer ACME's TLS-ALPN-01 challenge, whose CA sends none. |
<!-- END GENERATED -->

### `listeners[].socks`

The same object as the [`forward_proxy`](#forward_proxy) handler's params.

```fit
listener egress 127.0.0.1:1080 socks {
	socks { clients 127.0.0.1/32; allow *.example.com; ports 443 }
}
```

<!-- BEGIN GENERATED: listeners[].socks -->
<a id="cfg-listeners-socks"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `clients` | list of string | required | The addresses and CIDR ranges of the clients that may use the proxy, as "10.0.0.0/8"; others are refused. |
| `allow` | list of string |  | The destinations the proxy connects to: host names, "*.example.com" for every name below example.com, IP addresses and CIDR ranges, which also hold the address a host name resolves to. Every destination deny does not hold when left out, but for loopback, link-local (169.254.169.254, the metadata service of most clouds, among them), unspecified and multicast addresses and the host's own, which are refused unless a range here lies inside theirs, as "127.0.0.0/8" does and "0.0.0.0/0" does not, or, for the host's own, names that address alone. |
| `connect_timeout` | duration | `"10s"` | How long resolving and connecting to a destination may take. 10s when left out. |
| `deny` | list of string |  | Destinations refused, written as in allow, whatever allow holds: a host name that resolves to an address deny holds is refused once it is connected, before anything is sent. |
| `idle_timeout` | duration | `"5m"` | How long a proxied connection may carry nothing either way before both sides are closed. 5m when left out. |
| `ports` | list of integer |  | The destination ports allowed; any when left out. |
<!-- END GENERATED -->

### `listeners[].proxy_protocol`

<!-- BEGIN GENERATED: listeners[].proxy_protocol -->
<a id="cfg-listeners-proxy-protocol"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `trusted` | list of string | required | The addresses and CIDR ranges of the proxies that send the header, as "10.0.0.0/8": a connection from one of them must begin with a header, within 10s, or it is closed, and one from elsewhere is served as it is, with no header. |
<!-- END GENERATED -->

## Routes

A route says which requests it takes (`match`), the [middleware](#middleware) they pass through,
in order, and the [handler](#handlers) that answers. Each request goes to one route:

1. the routes of the listener it came on whose `match` it matches: every field of `match` must
   hold, and within a field, one of the values listed;
2. of those, the one with the highest `priority`;
3. among equal priorities, the most specific host (exact, then `*.`, then `**.`, then none), then
   the most specific path (exact, then the longest prefix), then the one with the most other
   matchers set, then the one written first.

A request no route takes is answered 404. A `match` field left out matches everything; `path`
takes exact paths and prefixes ending in `*`; `path_regexp` takes regular expressions in Go's
syntax, as fit's, which akit converts; `expr` combines matchers with `&&`, `||`, `!` and
parentheses, as in `Host("a.example") && (PathPrefix("/api/") || Method("POST"))`. The routes are
compiled into a matcher when the config is loaded, not looked through at each request.

The path is made canonical before any route sees it, and the route's middleware and handler get the
same one: its dot-segments are removed (`/a/./b/../c` is `/a/c`), repeated slashes are one, and a
path whose `..` climbs above `/`, or in which a decoded `%2F` or `%5C` makes an empty or
dot-segment, is answered 400.

In a `.conf` file, a `site` writes the routes of its host names together: each `route` in it takes
the site's hosts and its own paths, and the site's middleware before its own.

```fit
upstream api {
	addresses 127.0.0.1:9000
}

site example.com {
	route /api/* {
		name api-writes
		match { method POST PUT DELETE }
		middleware basic_auth {
			users {
				username admin
				password_hash $2a$14$Zkx19XLiW6VYouLHR5NmfOFU0z2GTNmpkT/5qqR7hx4IjWJPDhjvG
			}
		}
		handler proxy { upstream api }
	}
	route /api/* {
		name api
		handler proxy { upstream api }
	}
	route {
		name home
		handler respond { body home }
	}
}
```

### `routes[]`

<!-- BEGIN GENERATED: routes[] -->
<a id="cfg-routes"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `handler` | [object](#cfg-routes-handler) | required | What answers the request. |
| `access` | [object](#cfg-routes-access) |  | The client addresses the route serves, deciding before its listener's access and the global one: a request it refuses is answered 403. The address is the connection's, or the one its PROXY header names, not X-Forwarded-For's. |
| `listeners` | list of string |  | The names of the listeners the route serves; every listener when left out. |
| `match` | [object](#cfg-routes-match) |  | What a request must match; every request when left out. |
| `middleware` | list of [object](#cfg-routes-middleware) |  | Middleware a request passes through, in order, before the handler. |
| `name` | string |  | A name unique among the routes, for logs and metrics. |
| `priority` | integer |  | Routes with a higher priority are picked first; among equal ones, the most specific host, then path, then the one with the most other matchers set (method, header, query, remote_ip, path_regexp, expr), then the one written first. |
<!-- END GENERATED -->

### `routes[].match`

<!-- BEGIN GENERATED: routes[].match -->
<a id="cfg-routes-match"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `expr` | string |  | An expression the request must match, combining matchers with &&, \|\|, ! and parentheses, as in Host("a.example") && (PathPrefix("/api/") \|\| Method("POST")). Its matchers are Host, Path, PathPrefix, PathRegexp, Method, Header(name, value...), Query(key, value...) and RemoteIP, whose arguments are strings quoted as in Go. It counts as one matcher in the order of routes, and its hosts and paths do not make a route more specific: give it a priority. |
| `header` | map of list of string |  | Headers the request must have, each with one of the values listed as the value of one of its fields, or with any value when none are listed. Names ignore case; values do not. |
| `host` | list of string |  | Host names, exact, or with a leading "*." wildcard, which stands for one label, or a leading "**." wildcard, which stands for one label or more, as the Gateway API's wildcards do. |
| `method` | list of string |  | Methods, as in "GET"; compared as written, since methods are case-sensitive. |
| `path` | list of string |  | Paths, exact or ending in "*" to match a prefix. |
| `path_regexp` | list of string |  | Regular expressions in Go's syntax, matched against the path anywhere in it unless anchored with ^ and $. |
| `query` | map of list of string |  | Query parameters the request must have, each with one of the values listed, or with any value when none are listed; compared once unescaped. |
| `remote_ip` | list of string |  | IP addresses and CIDR prefixes, as in "10.0.0.0/8", the client's address must be in: the address of the connection, not of X-Forwarded-For. |
<!-- END GENERATED -->

### `routes[].handler`

A module: `module` names it and `params` are its own, in the [Handlers](#handlers) chapter.

<!-- BEGIN GENERATED: routes[].handler -->
<a id="cfg-routes-handler"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `module` | string: `bt_publish`, `bt_stream`, `bt_tracker`, `fastcgi`, `forward_proxy`, `hls`, `httpflv`, `media_pages`, `media_tokens`, `proxy`, `respond`, `static`, `stream_whep`, `stream_whip`, `sync_ingest`, `vod`, `webrtc_proxy`, `websockify`, `whep`, `whip` | required | The module's name within its namespace. |
| `params` | object |  |  |
<!-- END GENERATED -->

### `routes[].middleware[]`

A module: `module` names it and `params` are its own, in the [Middleware](#middleware) chapter.

<!-- BEGIN GENERATED: routes[].middleware[] -->
<a id="cfg-routes-middleware"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `module` | string: `basic_auth`, `body_limit`, `cache`, `circuit_breaker`, `client_cert`, `compress`, `concurrency_limit`, `forward_auth`, `headers`, `jwt`, `rate_limit`, `redirect`, `rewrite`, `wasm` | required | The module's name within its namespace. |
| `params` | object |  |  |
<!-- END GENERATED -->

## Upstreams

An upstream is a named set of servers, which the [proxy](#proxy) handler,
[`forward_auth`](#forward_auth) and [services](#services) send to.

- **Load balancing** (`load_balancing`): `round_robin` in turn; `weighted_round_robin` in turn by
  `weights`; `least_conn` to the address with the fewest requests under way; `random`; `p2c`, the
  better of two picked at random; `ip_hash`, `uri_hash`, `header_hash` and `cookie_hash`, the same
  address for the same client address, URI, header or cookie (`hash_key` names the last two).
  `sticky_cookie` keeps each client on one address with a cookie of akit's.
- **Connections**: at most `max_conns` to each address, `max_pending` requests waiting past them,
  `max_idle_conns` kept idle for `idle_timeout`.
- **Timeouts**: `connect_timeout`, `response_header_timeout` (until the response begins),
  `response_idle_timeout` (silence within a body) and `response_timeout` (the whole response).
- **Health**: passively, an address that fails `max_fails` requests within `fail_timeout` is out
  for `fail_timeout`; actively, `health_check` probes each address over HTTP or TCP.
- **Protocol**: `http/1.1`, `h2c`, or `h2` over TLS, with `tls_server_name`, `tls_ca_file` and a
  client certificate (`tls_cert_file`, `tls_key_file`).

Requests reach an upstream on akio's asynchronous HTTP clients: no I/O thread waits for an upstream.

```fit
listener web :8080

upstream app 10.0.0.1:8080 10.0.0.2:8080 {
	load_balancing least_conn
	connect_timeout 3s
	max_fails 3
	fail_timeout 30s
	health_check { path /healthz; interval 5s }
}

route { handler proxy { upstream app } }
```

### `upstreams[]`

<!-- BEGIN GENERATED: upstreams[] -->
<a id="cfg-upstreams"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `addresses` | list of string | required | The addresses of the upstream's servers, each a host and a port, as in "10.0.0.1:8080" or "backend:80". |
| `name` | string | required | A name unique among the upstreams, by which handlers refer to this one. |
| `connect_timeout` | duration | `"10s"` | How long opening a connection may take; 10s when left out. A request whose connect times out gets 504. |
| `deny_internal` | boolean |  | **Not supported yet**: refused by name. Refuse to connect where an address, or the address its host name resolves to, is loopback, link-local (169.254.169.254, the metadata service of most clouds, among them), unspecified, multicast or the host's own, decided once each connection is open and before anything is sent on it; a request refused so gets 502, as one whose connect fails. For addresses others choose: the Kubernetes provider sets it on the upstreams of ExternalName Services. Off when left out. **akit:** It comes with the Kubernetes provider's ExternalName Services (`allow_external_name_services`). |
| `fail_timeout` | duration | `"10s"` | The window max_fails are counted in, and how long an address that reached them stays out; 10s when left out. |
| `hash_key` | string |  | With header_hash, the header, and with cookie_hash, the cookie, whose value requests are hashed by. |
| `health_check` | [object](#cfg-upstreams-health-check) |  | Probe each address on a schedule, and take one that fails its probes out of rotation until it passes them again; no probes when left out. Probes and max_fails take an address out each on their own account. |
| `idle_timeout` | duration | `"60s"` | How long an idle connection is kept before it is closed; 60s when left out. |
| `load_balancing` | string: `round_robin`, `weighted_round_robin`, `least_conn`, `random`, `p2c`, `ip_hash`, `uri_hash`, `header_hash`, `cookie_hash` |  | How requests are spread over the addresses: round_robin, in turn; weighted_round_robin, in turn by weight (smoothly, as nginx does); least_conn, to the address with the fewest requests under way for its weight; random, at random by weight; p2c, to the less busy for its weight of two addresses chosen at random; ip_hash, uri_hash, header_hash and cookie_hash, by a consistent hash of the client's IP address, the request's path and query, the header hash_key names or the cookie it names, so that the same key goes to the same address while it is up, and adding or removing an address moves only the keys that went to it or will. A request without the header or cookie goes in turn. round_robin when left out, or weighted_round_robin when weights are given. |
| `max_conns` | integer, 0 or more | `10000` | The most connections open, or being opened, to one address; a request past it waits for one to be free. 10000 when left out or 0: large enough that high concurrency against a slow upstream does not reach it, and still a bound. |
| `max_fails` | integer, 0 or more | `1` | How many failed requests to an address within fail_timeout take it out of rotation for fail_timeout, as nginx's max_fails does: a connect that failed, an upstream that broke off or timed out. 1 when left out; 0 keeps every address in rotation. |
| `max_idle_conns` | integer, 0 or more | `32` | How many idle connections to one address are kept for later requests; 32 when left out, and 0 keeps none. |
| `max_pending` | integer, 0 or more | `50000` | How many requests may wait for a connection to one address once its max_conns are busy; one past it is answered 503 at once. 50000 when left out or 0. |
| `protocol` | string: `http/1.1`, `h2c`, `h2` | `"http/1.1"` | How requests reach the upstream: http/1.1 in the clear, h2c (HTTP/2 in the clear, which the servers must expect), or h2 (TLS, with HTTP/2 when the server chooses it through ALPN and HTTP/1.1 otherwise). http/1.1 when left out. |
| `response_header_timeout` | duration | `"60s"` | How long the upstream may take, once the request is sent, to begin its response; 60s when left out. A request past it gets 504. |
| `response_idle_timeout` | duration | `"60s"` | How long the upstream may go silent in the middle of a response body; 60s when left out. |
| `response_timeout` | duration | `"1h"` | How long a whole response may take, from sending the request to the end of its body; 1h when left out. A request past it before its response began gets 504, and one past it later is cut short. |
| `sticky_cookie` | [object](#cfg-upstreams-sticky-cookie) |  | Keep each client on one address with a cookie: a request whose cookie names an address that is up goes there, and any other gets the cookie for the address it went to. No cookie when left out. |
| `tls_ca_file` | string |  | With h2: a PEM file of the certificates the servers' are checked against, in place of the system's roots. It is watched, as a listener's certificate files are: a change loads the config again. |
| `tls_cert_file` | string |  | With h2: a PEM file of the client certificate, and the chain after it, that akit presents to servers that ask for one (mutual TLS); needs tls_key_file. It is watched, as tls_ca_file is: a change loads the config again. |
| `tls_key_file` | string |  | With h2: the PEM file of tls_cert_file's private key. |
| `tls_server_name` | string |  | With h2: the name the servers' certificates are checked against, and sent in SNI; each address's host when left out. |
| `weights` | list of integer, 1 to 1000 |  | Each address's weight, in the order of addresses: its share of requests under weighted_round_robin, and how many requests it takes to be as busy as an address of weight 1 under least_conn. 1 each when left out. |
<!-- END GENERATED -->

### `upstreams[].sticky_cookie`

<!-- BEGIN GENERATED: upstreams[].sticky_cookie -->
<a id="cfg-upstreams-sticky-cookie"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `max_age` | duration |  | How long the client keeps the cookie, as its Max-Age; until the browser closes when left out. |
| `name` | string | `"akit_sticky"` | The cookie's name; akit_sticky when left out. **akit:** fit's default is `fit_sticky`, renamed (decision 037). |
| `path` | string | `"/"` | The cookie's Path; / when left out. |
| `secure` | boolean |  | Mark the cookie Secure, so that it is sent only over HTTPS. It is always HttpOnly and SameSite=Lax. |
<!-- END GENERATED -->

### `upstreams[].health_check`

<!-- BEGIN GENERATED: upstreams[].health_check -->
<a id="cfg-upstreams-health-check"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `expect_status` | list of integer, 100 to 599 |  | With http: the statuses that pass; any from 200 to 399 when left out. |
| `fails` | integer, 0 or more | `2` | How many probes in a row must fail to take an address out of rotation; 2 when left out or 0. |
| `host` | string |  | With http: the Host field the probe sends; the address's host when left out. |
| `interval` | duration | `"5s"` | How long from the start of one probe of an address to the start of the next; 5s when left out. |
| `jitter` | duration |  | Up to how much longer than interval each wait between probes may be, chosen at random each time, so that probes from many upstreams spread out; a tenth of interval when left out. Less than interval. |
| `passes` | integer, 0 or more | `2` | How many probes in a row must pass to put an address that was out back in; 2 when left out or 0. |
| `path` | string | `"/"` | With http: the path, and query, the probe asks for; / when left out. |
| `port` | integer, 1 to 65535 |  | The port probes go to, in place of each address's own. |
| `timeout` | duration | `"2s"` | How long a probe may take, connecting included, before it fails; 2s when left out. No longer than interval. |
| `type` | string: `http`, `tcp` | `"http"` | http sends a GET request, over the upstream's protocol, and passes on an expected status; tcp passes once a connection opens. http when left out. |
<!-- END GENERATED -->

## Services

A service proxies at layer 4: the connections of `tcp` listeners, byte for byte, and the datagrams
of `udp` listeners, a session per client, to an upstream's addresses, picked by its load balancing
and health. TLS connections can be told apart by the name they ask for (`server_names`) and passed
through without being terminated; `proxy_protocol` tells the upstream who the client was.
Backpressure is linked both ways: a side that does not read holds the other's reads.

```fit
listener postgres :5432 tcp
listener dns :53 udp
listener tls :8443 tcp

upstream db 10.0.0.5:5432
upstream resolvers 10.0.0.53:53 10.0.0.54:53
upstream mail 10.0.0.25:8443

service postgres { listeners postgres; upstream db; idle_timeout 30m }
service dns { listeners dns; upstream resolvers }
service mail { listeners tls; upstream mail; server_names mail.example.com }
```

### `services[]`

<!-- BEGIN GENERATED: services[] -->
<a id="cfg-services"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `listeners` | list of string | required | The names of the listeners whose connections the service takes: those that serve tcp, alone or beside other protocols, and udp listeners, each one service's. A udp listener's session is a client's address: the datagrams it sends go to one upstream address, from a socket of the session's own, and what comes back to that socket goes to the client. |
| `name` | string | required | A name unique among the services. |
| `upstream` | string | required | The name of the upstream connections go to: one of its addresses, picked by its load_balancing (ip_hash by the client's address; header, cookie and URI hashes, which a connection has none of, in turn), out of rotation for its max_fails and health checks as for HTTP, dialed within its connect_timeout. A connect that fails is tried on the next address picked, up to three addresses. Its protocol and HTTP settings are not used. |
| `access` | [object](#cfg-services-access) |  | The client addresses the service takes, deciding before its listener's access and the global one: a connection it refuses is closed, once its PROXY header and ClientHello are read when the listener needs them, and the datagrams of a udp session it refuses are dropped. **akit:** A `udp` session from a client it refuses is closed as it opens, its first datagram dropped and each later one opening another, where fit drops the datagrams before a session is made: akio's UDP listeners take no accept filter yet (akit's request A220). |
| `idle_timeout` | duration |  | How long a connection or a udp session may carry nothing either way before both sides are closed; 10m for a connection and 1m for a udp session when left out. |
| `max_conns` | integer, 0 or more | `10000` | The most connections and udp sessions the service proxies at once; a connection past it is closed as soon as it is accepted, and the datagrams of a session past it are dropped. 10000 when left out or 0. |
| `proxy_protocol` | string: `v1`, `v2` |  | Send each upstream connection a PROXY protocol header of this version (v1, text, or v2, binary) first, naming the client's address and the one it connected to. For tcp listeners. |
| `server_names` | list of string |  | The server names (SNI) of the TLS connections the service takes from its tcp listeners, which it proxies as they are, without terminating their TLS; "*.example.com" takes the names one label below example.com. A tcp listener may then be listed by several services: a connection goes to the one whose names hold the name its ClientHello asks for, or else to the listener's service without server_names, if it has one, as one that is not TLS or asks for no name does; otherwise it is closed. The ClientHello must come within 10s. |
<!-- END GENERATED -->

## Access lists

Which client addresses akit serves, by allow lists (whitelists) and deny lists (blacklists) of IP
addresses and CIDR prefixes, written inline or in files, as the lists published for firewalls are.
The top-level `access` is global: every listener, route and service, and the Admin API, goes by
it. Each of them may set an `access` of its own, which decides first, so that the more specific
setting wins:

- a list's `deny` refuses an address, whatever its `allow` holds;
- else its `allow` serves an address it holds;
- else, when its `allow` has entries, it refuses the address, since an allow list serves only what
  it holds;
- else it has no say, and the next one out decides: a route's, then its listener's, then the global
  one; a service's, then its listener's, then the global one; the Admin API's, then the global one.
  An address none has a say on is served.

A route's `allow` therefore serves an address the global `deny` refuses, and a route's `deny`
refuses one the global `allow` serves; `allow 0.0.0.0/0 ::/0` opens a route or service to every
address whatever the global access says.

Where it can, akit decides when a connection is accepted: a refused TCP connection is closed before
anything is read from it, by akio's accept filter. So does a `tcp` listener of one service without
`server_names`, on a port of its own, by that service's access. A listener whose routes or services
set an access of their own, or that takes PROXY headers, decides later instead: a refused HTTP
request is answered `403`, a refused `tcp` connection closed once its service is known (after its
PROXY header and its ClientHello, as the listener needs them), and a client of the listener's
`socks` refused at its request. The address is the connection's, or the one its PROXY header names,
never `X-Forwarded-For`'s. A change applies to the connections that start after it: a reload changes
the lists without binding a listener again, and the connections already open, and the later requests
of an HTTP/2 connection a listener decided at accept, are not checked again.

The Admin API (`admin.access`, which serves the dashboard too) goes by the global access as well,
after its own, and decides at accept. Unlike the rest of `admin`, it follows every config applied,
without a restart. A global `allow` that leaves out `127.0.0.1` therefore shuts local clients out
of the Admin API too; to keep local access whatever the global access says, set the API's own
`allow 127.0.0.1 ::1`, which decides first. An Admin API on a Unix socket is never checked: its peers
have no address, and setting `admin.access` with one is an error. A config applied with an allow
list that leaves out the address the change came from does not close that connection, but the next
one is refused.

A list file has one entry on each line, each line told apart by its content, so that one file may
mix them: an address or prefix, with what follows `#` or `;` left out; a range, `first-last`; an
eMule `ipfilter.dat` line, `first - last , level , name`, where a line of a level above 127 is not
listed (qBittorrent's reading, and eMule's at its default filter level); or a PeerGuardian `.p2p`
line, `name:first-last`. IPv4 addresses in ranges may have leading zeros, as `ipfilter.dat` writes
them. Blank lines and lines that begin with `#`, `;` or `//` are left out; any other line is an
error naming the file and the line, and the config is not applied. A file is read when the config
is applied, and the config applied again when it changes, as certificate files are (`akit run
-watch-certs`); each file is at most 64 MiB.

An allow list on a listener that obtains certificates with ACME's HTTP-01 or TLS-ALPN-01 challenges
must serve the CA's validation servers, or the certificates cannot be obtained.

```fit
access { deny 203.0.113.0/24; deny_files lists/deny.txt }

listener web :8080
listener internal :8081 { access { allow 10.0.0.0/8 192.168.0.0/16 } }

route partners {
	listeners web
	match { path /partner/* }
	access { allow 203.0.113.7 198.51.100.0/24 }
	handler respond { body partners }
}

route { handler respond { body hello } }

listener postgres :5432 tcp
upstream db 10.0.0.5:5432
service postgres { listeners postgres; upstream db; access { allow 10.0.0.0/8 } }

admin { address 127.0.0.1:2019; access { allow 127.0.0.1 ::1 } }
```

**akit:** HTTP/3 requests are checked one by one, a refused one answered `403`, and a `udp`
session from a refused client is closed as it opens, where fit drops a refused peer's datagrams
before a session is made: akio's UDP listeners take no accept filter yet (akit's request A220).

### `access`

<!-- BEGIN GENERATED: access -->
<a id="cfg-access"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `allow` | list of string |  | IP addresses and CIDR prefixes served, as "10.0.0.0/8" or "2001:db8::/32": with any entry here or in allow_files, every address neither holds is refused, unless deny refuses it first. |
| `allow_files` | list of string |  | Files of more allow entries, one on each line, each line told apart by its content: an address or prefix, with what follows # or ; left out, as the lists published for firewalls are written; a range, first-last; an eMule ipfilter.dat line, first - last , level , name, where a level above 127 is not listed; or a PeerGuardian .p2p line, name:first-last. IPv4 addresses of ranges may have leading zeros. Blank lines and lines that begin with #, ; or // are left out; any other line is an error. Read when the config is applied, and applied again when one changes; each at most 64 MiB. |
| `deny` | list of string |  | IP addresses and CIDR prefixes refused, whatever allow holds. |
| `deny_files` | list of string |  | Files of more deny entries, written as allow_files. |
<!-- END GENERATED -->

### `listeners[].access`

<!-- BEGIN GENERATED: listeners[].access -->
<a id="cfg-listeners-access"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `allow` | list of string |  | IP addresses and CIDR prefixes served, as "10.0.0.0/8" or "2001:db8::/32": with any entry here or in allow_files, every address neither holds is refused, unless deny refuses it first. |
| `allow_files` | list of string |  | Files of more allow entries, one on each line, each line told apart by its content: an address or prefix, with what follows # or ; left out, as the lists published for firewalls are written; a range, first-last; an eMule ipfilter.dat line, first - last , level , name, where a level above 127 is not listed; or a PeerGuardian .p2p line, name:first-last. IPv4 addresses of ranges may have leading zeros. Blank lines and lines that begin with #, ; or // are left out; any other line is an error. Read when the config is applied, and applied again when one changes; each at most 64 MiB. |
| `deny` | list of string |  | IP addresses and CIDR prefixes refused, whatever allow holds. |
| `deny_files` | list of string |  | Files of more deny entries, written as allow_files. |
<!-- END GENERATED -->

### `admin.access`

<!-- BEGIN GENERATED: admin.access -->
<a id="cfg-admin-access"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `allow` | list of string |  | IP addresses and CIDR prefixes served, as "10.0.0.0/8" or "2001:db8::/32": with any entry here or in allow_files, every address neither holds is refused, unless deny refuses it first. |
| `allow_files` | list of string |  | Files of more allow entries, one on each line, each line told apart by its content: an address or prefix, with what follows # or ; left out, as the lists published for firewalls are written; a range, first-last; an eMule ipfilter.dat line, first - last , level , name, where a level above 127 is not listed; or a PeerGuardian .p2p line, name:first-last. IPv4 addresses of ranges may have leading zeros. Blank lines and lines that begin with #, ; or // are left out; any other line is an error. Read when the config is applied, and applied again when one changes; each at most 64 MiB. |
| `deny` | list of string |  | IP addresses and CIDR prefixes refused, whatever allow holds. |
| `deny_files` | list of string |  | Files of more deny entries, written as allow_files. |
<!-- END GENERATED -->

### `services[].access`

<!-- BEGIN GENERATED: services[].access -->
<a id="cfg-services-access"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `allow` | list of string |  | IP addresses and CIDR prefixes served, as "10.0.0.0/8" or "2001:db8::/32": with any entry here or in allow_files, every address neither holds is refused, unless deny refuses it first. |
| `allow_files` | list of string |  | Files of more allow entries, one on each line, each line told apart by its content: an address or prefix, with what follows # or ; left out, as the lists published for firewalls are written; a range, first-last; an eMule ipfilter.dat line, first - last , level , name, where a level above 127 is not listed; or a PeerGuardian .p2p line, name:first-last. IPv4 addresses of ranges may have leading zeros. Blank lines and lines that begin with #, ; or // are left out; any other line is an error. Read when the config is applied, and applied again when one changes; each at most 64 MiB. |
| `deny` | list of string |  | IP addresses and CIDR prefixes refused, whatever allow holds. |
| `deny_files` | list of string |  | Files of more deny entries, written as allow_files. |
<!-- END GENERATED -->

### `routes[].access`

<!-- BEGIN GENERATED: routes[].access -->
<a id="cfg-routes-access"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `allow` | list of string |  | IP addresses and CIDR prefixes served, as "10.0.0.0/8" or "2001:db8::/32": with any entry here or in allow_files, every address neither holds is refused, unless deny refuses it first. |
| `allow_files` | list of string |  | Files of more allow entries, one on each line, each line told apart by its content: an address or prefix, with what follows # or ; left out, as the lists published for firewalls are written; a range, first-last; an eMule ipfilter.dat line, first - last , level , name, where a level above 127 is not listed; or a PeerGuardian .p2p line, name:first-last. IPv4 addresses of ranges may have leading zeros. Blank lines and lines that begin with #, ; or // are left out; any other line is an error. Read when the config is applied, and applied again when one changes; each at most 64 MiB. |
| `deny` | list of string |  | IP addresses and CIDR prefixes refused, whatever allow holds. |
| `deny_files` | list of string |  | Files of more deny entries, written as allow_files. |
<!-- END GENERATED -->

## Access log

A line for each request, written to standard output, standard error or a file, through a bounded
buffer: when the output cannot keep up, lines past the buffer are dropped and counted rather than
holding requests up. A file is written by a thread of akit's own, never by an I/O thread. `format`
is how a line is written: `json`, the default, an object a line; `text`, every part as `key=value`
(logfmt); or `line`, the time and level, then the fields as `key=value`. A request's level is its
response's status: info below 400, warn for 4xx and error for 5xx; `level` writes the requests at
that level and above.

A file output is written through a sink of its own (decision 033), shared with akit's own log when
both name the same path: a queue of 1 MiB and a thread that writes, so a slow disk drops lines and
counts them rather than holding anything up. `rotate` rotates the file before a write that would
take it past `max_bytes`, and at the first write in a later period of `interval`, counted from
midnight in `time_zone` (so `24h` turns at local midnight by default, and `1h` on the hour),
whichever comes first. A rotated file is named `<output>.<yyyy-mm-ddThh-mm-ss.mmm>`, the rotation's
time in UTC, with `-1`, `-2` when the name is taken and `.gz` once compressed. Rotated files older
than `max_age`, then past `max_files`, then the oldest until they total at most `max_total_bytes`
are removed when the file opens, after each rotation and every ten minutes; only names a rotation
made from that path are ever removed, never the open file. `"rotate": {}` is a bounded policy of
fit's defaults; without `rotate` the file grows without bound, and `akit run` says so when it
starts. A reload changes `time_zone` and every `rotate` field in place; `output` and `format` take
a restart.

```fit
listener web :8080

access_log {
	output stdout
	flush_interval 1s
	level warn
	format line
	time_zone UTC
}

route { handler respond { body hello } }
```

A file rotated every day at local midnight or at 100 MiB, kept for two weeks and at most 2 GiB:

```fit
listener web :8080

access_log {
	output /var/log/akit/access.log
	rotate {
		interval 24h
		max_bytes 100MiB
		max_age 336h
		max_total_bytes 2GiB
	}
}

route { handler respond { body hello } }
```

### `access_log`

<!-- BEGIN GENERATED: access_log -->
<a id="cfg-access-log"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `buffer_bytes` | integer, 0 or more | `4194304` | How many bytes of lines may wait to be written; lines past it are dropped. 4 MiB when left out. |
| `flush_interval` | duration | `"1s"` | How long a line may wait in the buffer before it is written; 1s when left out. |
| `format` | string: `json`, `text`, `line` | `"json"` | How a request's line is written, with the same fields whichever: json, a JSON object; text, time, level, msg and the fields as key=value (logfmt); or line, the time and level, then the fields as key=value, as akit's own log writes lines. A request's level is its status's, as level says. json when left out. A change takes a restart. |
| `level` | string: `info`, `warn`, `error` | `"info"` | The least level of the requests written: a response with a status below 400 is info, a 4xx warn and a 5xx error, so warn writes the requests answered with 4xx and 5xx, and error those answered with 5xx. Every request when left out. A change takes effect on reload. |
| `output` | string | `"stdout"` | "stdout", "stderr", or the path of a file lines are appended to; "stdout" when left out. |
| `rotate` | [object](#cfg-access-log-rotate) |  | Rotate the file output names, and remove old rotated files: the size and interval a file is rotated at, and the age, count and total size past which rotated files are removed. None when left out: the file grows without bound, as before. |
| `time_zone` | string |  | The zone a line's time is written in, and the zone a file's interval is rotated at: local (the default), utc, or an IANA zone name such as Asia/Shanghai. json and text wrote UTC and line local before this setting; now every format writes this zone, local when left out, and 24h turns at local midnight. A change takes effect on reload. |
<!-- END GENERATED -->

### `access_log.rotate`

<!-- BEGIN GENERATED: access_log.rotate -->
<a id="cfg-access-log-rotate"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `compress` | boolean | `true` | Gzip a file when it is rotated. On when left out. |
| `interval` | duration |  | How long a file is written before it is rotated, at the next period boundary of the log's time_zone: 24h turns at midnight, 1h on the hour; a fraction of a second, or a period over 24h that is not a whole number of days, is refused. Off when left out. |
| `max_age` | duration | `"720h"` | How long a rotated file is kept after it was last written: 720h (30 days) when left out; 0 keeps it until a count or size limit removes it. |
| `max_bytes` | integer, 0 or more | `1073741824` | How large a file may grow before it is rotated: a write that would take it past this starts a new file. 1 GiB when left out; 0 does not rotate by size. |
| `max_files` | integer, 0 or more |  | The most rotated files kept, newest first; 0 or left out has no count limit. |
| `max_total_bytes` | integer, 0 or more | `1073741824` | The most the rotated files may total, the oldest removed until they fit. 1 GiB when left out; 0 does not limit the total. |
<!-- END GENERATED -->

## Log

akit's own log: what akit says about itself (certificates, providers, telemetry, FastCGI
applications' standard error, its files, akio's engine), as opposed to the access log's line for
each request. Each message comes from one of fit's loggers: `server`, `admin`, `tls`, `provider`,
`telemetry`, `access_log`, `plugin`, `fastcgi` and `engine` (akio's warnings and errors; its info
and debug records are the engine's debug, as fib's engine lines are off unless asked); fit's
`llm`, `tunnel`, `webrtc`, `sync` and `bittorrent` are taken too, for the tracks that bring them. A
message below its logger's level is not formatted. Lines are written as fit's (fib's): `line`, the
time, level and message, then `logger=` and any attributes; `text` (logfmt) and `json`, as Go's
log/slog writes them; every format in `time_zone`, local by default. With no `output`, debug and
info go to standard output and warn and error to standard error; a file is written through the same
sink as the access log's (decision 033), shared when both name one path, which then needs one
`rotate` and one `time_zone`. Until `akit run` has read the config, and in `akit validate`, a
message is written to standard error as `akit: <message>`. akit's command lines (`akit: listener …`,
`akit: reloaded …`) are printed as they are, not through the log.

```fit
listener web :8080

log {
	level info
	levels { tls debug }
	format json
	time_zone utc
	loggers {
		engine { output /var/log/akit/engine.log; format line; rotate { interval 24h } }
	}
}

route { handler respond { body hello } }
```

### `log`

<!-- BEGIN GENERATED: log -->
<a id="cfg-log"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `format` | string: `line`, `text`, `json` | `"line"` | How messages are written, those of every logger loggers does not set one for, as fib writes its own, which akit writes as fit does: line, the time, level and message, then the attributes as key=value; text, every part as key=value (logfmt); or json, a JSON object a line. Each carries the logger's name as logger. line when left out. A change takes a restart. **akit:** The formats are fit's, as fib writes them (akio's own log formats are not used): `line` as fib's, `text` and `json` as Go's log/slog. |
| `level` | string: `debug`, `info`, `warn`, `error` | `"info"` | The least level written, of every logger levels and loggers do not set one for: debug, info, warn or error. info when left out. A change takes effect on reload. |
| `levels` | map of string |  | The least level of some loggers, by name, over level: server (listeners, loading and reloading configs, shutting down), admin (the Admin API), tls (certificates: ACME, OCSP, certificate files), provider (providers), telemetry (exporting over OTLP), access_log (the access log's dropped lines), plugin (wasm plugins), fastcgi (what FastCGI applications write to their stderr), llm and tunnel (their roles), webrtc (the TURN server), sync (its pipelines), bittorrent (its roles) and engine (akio, the network engine, and whatever else writes to Go's log and log/slog). The same as loggers' level, which a logger may not have both of. A change takes effect on reload. **akit:** `engine` takes akio's records: its warnings and errors at their levels, its info and debug records at debug (fib's engine lines are off unless asked). akit's crates whose messages carry no level (ACME, OCSP, providers, telemetry, FastCGI's stderr) write them at warn. `plugin` has no messages yet. |
| `loggers` | map of [object](#cfg-log-loggers) |  | Settings of some loggers of their own, by name (those levels lists), over level, output, format and rotate: engine's to a file of its own, say, or access_log's as json. |
| `output` | string |  | "stdout", "stderr", or the path of a file messages are appended to, of every logger loggers does not set one for. When left out, debug and info messages go to stdout and warn and error ones to stderr. A change takes a restart. |
| `rotate` | [object](#cfg-log-rotate) |  | Rotate the file log.output names, and remove old rotated files: the size and interval a file is rotated at, and the age, count and total size past which rotated files are removed. None when left out: the file grows without bound, as before. |
| `time_zone` | string |  | The zone the time of every line is written in, and the zone a file's interval is rotated at: local (the default), utc, or an IANA zone name such as Asia/Shanghai. So 24h turns at local midnight by default, and at UTC midnight with utc. The zone database is embedded, so an IANA name works in a container without zoneinfo. A change takes effect on reload. **akit:** An IANA name is read from the system's zoneinfo files, as `access_log.time_zone`'s; fit embeds Go's zone database. |
<!-- END GENERATED -->

### `log.rotate`

<!-- BEGIN GENERATED: log.rotate -->
<a id="cfg-log-rotate"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `compress` | boolean | `true` | Gzip a file when it is rotated. On when left out. |
| `interval` | duration |  | How long a file is written before it is rotated, at the next period boundary of the log's time_zone: 24h turns at midnight, 1h on the hour; a fraction of a second, or a period over 24h that is not a whole number of days, is refused. Off when left out. |
| `max_age` | duration | `"720h"` | How long a rotated file is kept after it was last written: 720h (30 days) when left out; 0 keeps it until a count or size limit removes it. |
| `max_bytes` | integer, 0 or more | `1073741824` | How large a file may grow before it is rotated: a write that would take it past this starts a new file. 1 GiB when left out; 0 does not rotate by size. |
| `max_files` | integer, 0 or more |  | The most rotated files kept, newest first; 0 or left out has no count limit. |
| `max_total_bytes` | integer, 0 or more | `1073741824` | The most the rotated files may total, the oldest removed until they fit. 1 GiB when left out; 0 does not limit the total. |
<!-- END GENERATED -->

### `log.loggers`

A logger's own settings, by its name; what it leaves out is the log's.

<!-- BEGIN GENERATED: log.loggers.* -->
<a id="cfg-log-loggers"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `format` | string: `line`, `text`, `json` |  | How the logger's messages are written: line, text or json, as the log's format. A change takes a restart. |
| `level` | string: `debug`, `info`, `warn`, `error` |  | The least level the logger writes. A change takes effect on reload. |
| `output` | string |  | "stdout", "stderr", or the path of a file the logger's messages are appended to. A change takes a restart. |
| `rotate` | [object](#cfg-log-loggers-rotate) |  | Rotate this logger's file, when output names one of its own; the log's rotate is used when the logger writes to the log's output. The settings are the log's rotate's. |
<!-- END GENERATED -->

### `log.loggers.*.rotate`

<!-- BEGIN GENERATED: log.loggers.*.rotate -->
<a id="cfg-log-loggers-rotate"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `compress` | boolean | `true` | Gzip a file when it is rotated. On when left out. |
| `interval` | duration |  | How long a file is written before it is rotated, at the next period boundary of the log's time_zone: 24h turns at midnight, 1h on the hour; a fraction of a second, or a period over 24h that is not a whole number of days, is refused. Off when left out. |
| `max_age` | duration | `"720h"` | How long a rotated file is kept after it was last written: 720h (30 days) when left out; 0 keeps it until a count or size limit removes it. |
| `max_bytes` | integer, 0 or more | `1073741824` | How large a file may grow before it is rotated: a write that would take it past this starts a new file. 1 GiB when left out; 0 does not rotate by size. |
| `max_files` | integer, 0 or more |  | The most rotated files kept, newest first; 0 or left out has no count limit. |
| `max_total_bytes` | integer, 0 or more | `1073741824` | The most the rotated files may total, the oldest removed until they fit. 1 GiB when left out; 0 does not limit the total. |
<!-- END GENERATED -->

## Telemetry

OpenTelemetry over OTLP/HTTP: a span for each request sampled, with W3C trace context sent on to
upstreams, and akit's metrics exported on a schedule, to a collector. Read when akit starts.

```fit
listener web :8080
route { handler respond }

telemetry {
	endpoint http://otel-collector:4318
	service_name edge
	traces { sample_ratio 0.1 }
	metrics { interval 30s }
}
```

### `telemetry`

<!-- BEGIN GENERATED: telemetry -->
<a id="cfg-telemetry"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `ca_file` | string |  | A PEM file with the CAs the receiver's certificate is verified with, over https; the system's when left out. |
| `cert_file` | string |  | A PEM file with the client certificate sent over https; with key_file. |
| `endpoint` | string | `"http://127.0.0.1:4318"` | The OTLP/HTTP receiver: http:// or https:// and a host and port, as of an OpenTelemetry Collector; traces go to its /v1/traces and metrics to /v1/metrics, as JSON. http://127.0.0.1:4318 when left out. |
| `headers_file` | string |  | A file of header fields sent with each export, one "Name: value" a line, as an API key a receiver asks for; read when akit starts. None when left out. |
| `key_file` | string |  | A PEM file with cert_file's private key. |
| `metrics` | [object](#cfg-telemetry-metrics) |  | Export the metrics the Admin API's /metrics serves, requests counted included, every interval. No metrics over OTLP when left out. |
| `service_name` | string | `"akit"` | The service.name of the resource traces and metrics are about. akit when left out. **akit:** fit's default is `fit`, renamed (decision 037). |
| `timeout` | duration | `"10s"` | How long an export may take, connecting included; a batch not sent by then is dropped and counted. 10s when left out. |
| `traces` | [object](#cfg-telemetry-traces) |  | Trace requests: a server span for each one sampled, and its context sent on to upstreams in traceparent. No traces when left out. |
<!-- END GENERATED -->

### `telemetry.traces`

<!-- BEGIN GENERATED: telemetry.traces -->
<a id="cfg-telemetry-traces"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `interval` | duration | `"5s"` | How often spans are exported, and sooner when 256 KiB of them wait. 5s when left out. |
| `max_queue_bytes` | integer, 0 or more | `4194304` | The most bytes of spans waiting to be exported, besides the batch being sent; spans past it are dropped and counted. 4 MiB when left out or 0. |
| `sample_ratio` | number, 0 to 1 |  | The share of requests traced among those whose caller did not decide: from 0 to 1. A request whose traceparent says its caller sampled it is always traced, and one that says it did not is never. 0, only those a caller sampled, when left out. |
<!-- END GENERATED -->

### `telemetry.metrics`

<!-- BEGIN GENERATED: telemetry.metrics -->
<a id="cfg-telemetry-metrics"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `interval` | duration | `"30s"` | How often the metrics are exported. 30s when left out. |
<!-- END GENERATED -->

## Limits

Limits on what clients can make akit hold, on every listener: the largest request head
(`max_header_bytes`, answered 431) and body (`max_body_bytes`, answered 413), the client
connections open at once (`max_conns`; the connections akit dials to upstreams do not count), the
bytes one connection may have queued for its client (`max_write_queue_bytes`), the bytes waiting
for every client together (`max_pending_bytes`, half the memory by default: past it every
connection stops reading until some go) and how long a client may leave a response unread
(`write_stall_timeout`). `max_conns`, `max_pending_bytes` and `max_write_queue_bytes` take effect
at start; a reload that changes them is warned of. The [`body_limit`](#body_limit) middleware
limits a route's bodies further.

```fit
listener web :8080
route { handler respond }

limits {
	max_header_bytes 64KiB
	max_body_bytes 100MiB
	max_conns 50000
	write_stall_timeout 1m
}
```

### `limits`

<!-- BEGIN GENERATED: limits -->
<a id="cfg-limits"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `max_body_bytes` | integer, 0 or more | `16777216` | The largest request body a listener takes, whether it arrives whole or streams through; a larger one gets 413, or, once it has begun to stream, its connection or stream is ended. 16 MiB when left out or 0. |
| `max_conns` | integer, 0 or more | `100000` | The most client connections the listeners keep open at once, across all of them; one past it is closed as soon as it is accepted. Takes effect at start, not on a reload. 100000 when left out or 0. |
| `max_header_bytes` | integer, 0 or more | `1048576` | The largest request header a listener takes; a larger one gets 431. 1 MiB when left out or 0. |
| `max_pending_bytes` | bytes or percent | `"50%"` | How many bytes of responses may wait, across every connection, for clients to take them, the backpressure on them all; past it, every connection stops reading until some go. A number of bytes, or a share of the memory akit may use, such as "25%": the machine's, or the container's memory limit where that is lower. Takes effect at start, not on a reload. "50%" when left out or 0; 256 MiB where the memory cannot be read. **akit:** akio's budget of the bytes queued across the listeners' engine (akio's ADR-172): past it every connection, upstream ones too, stops reading after its current read, and all resume once the bytes queued are back under three quarters of it. akio sums the I/O threads' queues every millisecond or so, so what is queued meanwhile can pass the budget by a little. A share of memory is read from `/proc/meminfo` and the cgroup's limit on Linux; elsewhere (macOS) akit reads no memory and a share is 256 MiB. |
| `max_write_queue_bytes` | integer, 0 or more | `33554432` | The bytes one connection may have queued for its client before akit closes it, the bound on a peer that stops reading. 32 MiB when left out or 0. Takes effect at start, not on a reload. **akit:** akio's `max_queued_bytes`: past it akio refuses the write (`Overloaded`) rather than queue it, where fit closes the connection. |
| `write_stall_timeout` | duration | `"30s"` | How long a response may wait for its client to take what it was sent, each time it waits, before the client loses it: its HTTP/1 connection closed, its HTTP/2 or HTTP/3 stream reset. A client that keeps reading, however slowly, is not cut off, and nor is a response that has nothing to send. 30s when left out. |
<!-- END GENERATED -->

## Engine

How the engine that serves the listeners is set up, read onto akio's
([decision 028](../decisions/028-engine-on-akio.md)): `event_loops` is the number of I/O threads
each listener's connections are served on (one a core when left out), `workers`, `min_workers` and
`task_queue` size akio's logical pool, which runs the work that may block (files, plugins, the disk
cache), and `read_buffer_size`, `write_watermark` and `backlog` are akio's own settings of the same
meaning. What is left out, or 0, keeps akio's default. fib's `loop_wait` and `poll_batch`, which
have no akio setting, and `kernel_tls_offload` are not supported yet. The engine is set up when
akit starts: a reload that changes it is warned of, and the running one stays.

```fit
listener web :8080
route { handler respond }

engine {
	event_loops 4
	workers 64
	read_buffer_size 64KiB
}
```

### `engine`

<!-- BEGIN GENERATED: engine -->
<a id="cfg-engine"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `backlog` | integer, 0 or more |  | The length of each listening socket's queue of connections not yet accepted. The system's when left out or 0. **akit:** akio's 1024 when left out or 0, not the system's. |
| `event_loops` | integer, 0 to 1024 |  | How many event loops each engine waits for its connections' events on, which spread the waits, accepts and wake-ups over cores; the connections' work runs on the workers either way. akio's default when left out or 0: 1 on up to 4 CPUs, 2 on up to 16, and the CPUs divided by 8 on more. Windows has one loop whatever this says. **akit:** Each listener's akio I/O threads, which run its connections' callbacks too (fib's loops only wait); one a core when left out or 0, where fib has 1 on up to 4 CPUs. |
| `kernel_tls_offload` | boolean |  | **Not supported yet**: refused by name. Hand the encryption of what each TLS listener's connection sends to the kernel's TLS (kTLS) right after its handshake, for a network card that encrypts TLS records itself, such as NVIDIA (Mellanox) ConnectX-6 Dx and later with tls-hw-tx-offload on (ethtool -K <nic> tls-hw-tx-offload on; TlsTxDevice in /proc/net/tls_stat counts what the card took). Without such a card the kernel encrypts small responses at more CPU than fit does, so it is off by default, and a connection then hands its encryption to the kernel only when it sends a large file, which goes by sendfile. Linux, AES-GCM suites of TLS 1.2 and 1.3, though the card takes TLS 1.2 connections only, Linux's offload having none for 1.3: the kernel encrypts TLS 1.3 ones itself (TlsTxSw), so a listener meant for the card sets tls.max_version 1.2. ChaCha20-Poly1305 and other systems keep the encryption in fit. Receiving stays in fit either way. A listener's tls.kernel_tls_offload, when set, decides for that listener instead. Listeners only: the console, the tunnel and the other TLS servers fit runs keep the default. |
| `loop_wait` | string: `park`, `block` | `"park"` | **Not supported yet**: refused by name. How each event loop waits while none of its connections has an event ready: park, in Go's network poller, which gives the loop's CPU to other work at once, but on a busy server looks for the loop's events only when a CPU runs out of work or every 10ms; or block, in epoll_wait, which wakes the loop as soon as one arrives. Choose block for a shorter latency tail near saturation over HTTP/1.1, at the cost of the median and a little h2 throughput: proxying an API at 200,000 requests a second on 6 CPUs, block took the HTTP/1.1 p99 from 4.3 to 2.9-3.9ms and the median from 0.95 to 1.57ms, left h2 and the maximum as they were, and served h2 0.9% less at saturation. Linux only. park when left out. |
| `min_workers` | integer, 0 or more |  | How many workers an idle pool keeps, rather than retiring every one and starting them again when work arrives. None when left out or 0. **akit:** The logical pool's threads kept when idle (akio's `min_threads`). |
| `poll_batch` | integer, 0 or more | `1024` | **Not supported yet**: refused by name. The most events one wait of an event loop collects; the rest wait for the next. 1024 when left out or 0. |
| `read_buffer_size` | integer, 0 to 67108864 | `65536` | The bytes a connection reads its socket into at a time, which is also the most a proxied body is passed on in one write. 64 KiB when left out or 0. |
| `task_queue` | integer, 0 or more |  | How many tasks each engine's pool holds waiting for a worker. akio's default, which follows its workers, when left out or 0. **akit:** The jobs akio's logical pool holds waiting for a thread; akio's 65,536 when left out or 0. Past it, new work is refused rather than queued. |
| `workers` | integer, 0 or more |  | The most workers each engine's pool runs connections' reads and callbacks on at once. 100 for each CPU akit may use, and at least min_workers, when left out or 0. **akit:** The most threads of akio's logical pool, which runs only the work that may block (files, plugins, the disk cache), not connections' callbacks; akio's default (8 a core, at least 8) when left out or 0. |
| `write_watermark` | integer, 0 or more |  | The bytes a connection may have waiting to be written before it stops reading until they drain, the backpressure on one connection; limits.max_pending_bytes bounds them all together. When left out or 0, akio sizes it from read_buffer_size and the machine: four read buffers, at least 64 KiB and at most 256 KiB, so 256 KiB at the default 64 KiB read buffer on a machine with enough memory. **akit:** akio's `write_high_watermark`; akio's 256 KiB when left out or 0. Reads resume at a quarter of it. |
<!-- END GENERATED -->

## Admin API

The Admin API reads and replaces the running config: `GET`, `PUT`, `PATCH` and `DELETE` on
`/config/` and a JSON pointer into it, `POST /load` for a whole config, `POST /reload` to apply the
running config again, ETags against lost updates (`If-Match`), the configs applied (`/versions`)
and `POST /rollback/{n}` to one of them. A change sent with `?dry_run=1` is checked through the
whole path an apply takes, and answered as an apply would answer it, with nothing applied. It also
serves metrics at `/metrics`, the effective config with providers merged at `/config/effective`, the
status at `/status`, cache purges at `POST /cache/purge`, and a read-only dashboard at
`/dashboard/`. It is off when `admin` is left out, and listens on `127.0.0.1:2019` when its address
is. A change through the API is applied as a SIGHUP's reload is, and shares its history.

akit serves the API on a loopback address or a Unix socket, or, with TLS (`cert_file`,
`key_file`, and `client_ca_file` for client certificates) and a token, off loopback (I92);
`token_file` asks every request for a bearer token; `access` says which client addresses it
serves ([access lists](#access-lists)), and follows every config applied. The
[console](#the-console) is supported (I89); the store, the assistant and cluster management are not
supported yet. The [Admin API guide](../guides/admin-api.md) walks
through it.

```fit
admin {
	address 127.0.0.1:2019
	token_file admin.token
	history 20
}

route { match { host example.com }; handler respond { body hello } }
```

### `admin`

<!-- BEGIN GENERATED: admin -->
<a id="cfg-admin"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `access` | [object](#cfg-admin-access) |  | The client addresses the Admin API serves, deciding before the global access, which applies to the API too: a connection either refuses is closed as soon as it is accepted. Unlike the other admin settings it follows every config applied, without a restart. Set allow to "127.0.0.1" (and "::1") to keep local access whatever the global access says. A Unix socket's peers have no address and are never checked, and access is an error with one. Every address is served, unless the global access says otherwise, when left out. |
| `address` | string | `"127.0.0.1:2019"` | Where the Admin API listens: a host and port, or "unix/" and the path of a Unix socket, as in "unix//run/akit/admin.sock". A host that is not a loopback address or localhost needs token_file, cert_file and key_file, so that only holders of the token reach the API and the token never crosses a network in the clear. 127.0.0.1:2019 when left out. **akit:** A Unix socket is made 0600 (`unix/<path>`). |
| `assistant` | object |  | **Not supported yet**: refused by name. The workbench's assistant, a chat beside the console's pages that reads what fit does and drafts changes for a person to submit, calling the model APIs set here; off when left out, and then nothing is sent anywhere. Needs console. With a store, these settings are the first ones only: once the Settings page saves, the store's are used, keys included. |
| `cert_file` | string |  | A PEM file with the Admin API's certificate, followed by any intermediates; with key_file, the API is served over TLS, 1.2 or later. The files are read again when they change, so that a renewed certificate is served without a restart. Not for a Unix socket. **akit:** The files are looked at on the control thread every `-watch-interval` with `-watch-certs` (on by default), not at a handshake as fit's, and loaded once they have stayed as they are for 500ms; SIGHUP reads them again too (I94). New connections get the new certificate; files that do not load are logged in the `admin` logger and the running certificate stays. |
| `client_ca_file` | string |  | A PEM file with the CAs that clients' certificates must chain to: with it, every client must present a valid certificate, besides the token, before it is heard. Needs cert_file and key_file; read again when it changes, as they are. **akit:** Read again with `cert_file` (I94): new connections are checked against the new CAs. |
| `cluster` | object |  | **Not supported yet**: refused by name. Manage other fits from this one, the manager: their addresses and credentials, added on the workbench's Cluster pages, are kept in the store, and each one's state is read every poll_interval. Off when left out, and then nothing is called. Needs store and console. See decision 033. |
| `cluster_member` | object |  | **Not supported yet**: refused by name. Link this fit out to a manager of many fits, which then manages it through the link as it manages others over HTTP: for a fit behind NAT, or with no port open to the manager. The manager's Add by enrollment gives the line to paste here. Off when left out, and then nothing is dialed. Read when fit starts. See decision 033. |
| `console` | [object](#cfg-admin-console) |  | Serve the console, web pages behind a user name and password from which the whole config, admin aside, is read and changed; off when left out. |
| `disable_dashboard` | boolean |  | Do not serve the dashboard, the read-only web pages at /dashboard/ that show the routes, services, upstreams and their health, certificates and config versions from the Admin API's GET endpoints; served when left out or false. |
| `history` | integer, 0 or more | `10` | How many of the configs applied, the running one included, are kept to be read back. 10 when left out or 0. |
| `key_file` | string |  | A PEM file with the certificate's private key. |
| `max_config_bytes` | integer, 0 or more | `4194304` | The largest config the Admin API takes; a larger one gets 413. 4 MiB when left out or 0. |
| `max_conns` | integer, 0 or more | `16` | The most connections the Admin API keeps open at once, a quarter of them kept for those that have made a request with the token (any request, without token_file). When they are taken, a new connection closes the oldest of those that have not, of the client holding the most, and is closed as it is accepted only when every one has; one past a client's share, a quarter of them, is closed too, but for loopback clients. A connection that has not is closed 5s after it is accepted, whatever it is doing. 16 when left out or 0. |
| `mptcp` | boolean |  | **Not supported yet**: refused by name. Listen the Admin API with Multipath TCP where the kernel offers it (Linux 5.6+, net.mptcp.enabled), falling back to plain TCP where it does not. Off by default; a Unix socket has no paths. |
| `node_id` | string |  | **Not supported yet**: refused by name. This fit's node ID, which the Admin API's GET /node answers and a manager of many fits knows it by: 1 to 64 letters, digits, '.', '_' and '-', starting with a letter or digit. Left out, fit makes one, a random UUID, once, and keeps it in the store, or without one in a file beside the config, as fit.node-id beside fit.json; set it where neither lasts, as in a container whose config is read-only. |
| `store` | object |  | **Not supported yet**: refused by name. Keep the config encrypted in a file of its own, the store, which is the base config fit runs from then on: every config applied is written to it, and what this file sets beside admin is only the store's first content, used when the store does not exist yet. Left out, the config lives in this file. |
| `token_file` | string |  | A file holding a token that every request must carry, as "Authorization: Bearer <token>"; no token is asked for when left out. **akit:** Read when akit starts. |
<!-- END GENERATED -->

### The console

`admin.console` serves the console: web pages behind a user name and password from which the
running config is read and changed, on an address of its own (`127.0.0.1:2020` by default), with
TLS from `cert_file` and `key_file`, which an address off loopback must have. A user is a name and
a bcrypt hash, as `akit hash-password` prints it; a password is never written in clear text. A
client address that fails 5 logins in 15 minutes is refused for 15 minutes; sessions end after
`session_timeout` without a request or `session_lifetime` in all. The console calls the Admin API in
process, so `admin` must be there; like the rest of `admin`, its settings are read when akit starts,
but for `access`, which follows every config applied. The [console guide](../guides/console.md)
walks through it.

```fit
admin {
	console {
		address 127.0.0.1:2020
		users { username admin; password_hash "$2b$10$BnL9RRJoyXb.4XE2fYkhbuZmsS45tlwTM/ip3LmJmAW7ybYRggFd." }
		access { allow 127.0.0.1 ::1 }
	}
}
```

### `admin.console`

<!-- BEGIN GENERATED: admin.console -->
<a id="cfg-admin-console"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `users` | list of [object](#cfg-admin-console-users) | required | Who may log in: at least one user. |
| `access` | [object](#cfg-admin-console-access) |  | The client addresses the console serves, deciding before the global access, which applies to the console too: a connection either refuses is closed as soon as it is accepted. It follows every config applied, without a restart. Set allow to "127.0.0.1" (and "::1") to keep local access whatever the global access says. Every address is served, unless the global access says otherwise, when left out. |
| `address` | string | `"127.0.0.1:2020"` | The host and port the console listens on. A host that is not a loopback address needs cert_file and key_file, so that passwords never cross a network in the clear. 127.0.0.1:2020 when left out. |
| `cert_file` | string |  | A PEM file with the console's certificate, followed by any intermediates; with key_file, the console is served over TLS. **akit:** Read again when the files change, as `admin.cert_file` (I94): a renewed certificate is served to new connections without a restart; fit reads it when it starts. |
| `key_file` | string |  | A PEM file with the certificate's private key. |
| `max_conns` | integer, 0 or more | `32` | The most connections the console keeps open at once, a quarter of them kept for those with a login. When they are taken, a new connection closes the oldest of those without one, of the client holding the most, and is closed as it is accepted only when every one has a login; one past a client's share, a quarter of them, is closed too, but for loopback clients. 32 when left out or 0. **akit:** A connection past it is closed as it is accepted: none is kept for those with a login, and none without one is closed to make room, until akio's A232. |
| `max_sessions` | integer, 0 or more | `64` | The most logins kept at once; one more ends the oldest. 64 when left out or 0. |
| `mptcp` | boolean |  | **Not supported yet**: refused by name. Listen the console with Multipath TCP where the kernel offers it (Linux 5.6+, net.mptcp.enabled), falling back to plain TCP where it does not. Off by default. |
| `session_lifetime` | duration | `"12h"` | How long a login lasts at most, however busy. 12h when left out. |
| `session_timeout` | duration | `"30m"` | How long a login lasts without a request. 30m when left out. |
<!-- END GENERATED -->

### `admin.console.users[]`

<!-- BEGIN GENERATED: admin.console.users[] -->
<a id="cfg-admin-console-users"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `password_hash` | string | required | The bcrypt hash of the user's password, as akit hash-password makes it. **akit:** `akit hash-password` makes one; fit's hashes are taken as they are. |
| `username` | string | required | The user's name. |
<!-- END GENERATED -->

### `admin.console.access`

<!-- BEGIN GENERATED: admin.console.access -->
<a id="cfg-admin-console-access"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `allow` | list of string |  | IP addresses and CIDR prefixes served, as "10.0.0.0/8" or "2001:db8::/32": with any entry here or in allow_files, every address neither holds is refused, unless deny refuses it first. |
| `allow_files` | list of string |  | Files of more allow entries, one on each line, each line told apart by its content: an address or prefix, with what follows # or ; left out, as the lists published for firewalls are written; a range, first-last; an eMule ipfilter.dat line, first - last , level , name, where a level above 127 is not listed; or a PeerGuardian .p2p line, name:first-last. IPv4 addresses of ranges may have leading zeros. Blank lines and lines that begin with #, ; or // are left out; any other line is an error. Read when the config is applied, and applied again when one changes; each at most 64 MiB. |
| `deny` | list of string |  | IP addresses and CIDR prefixes refused, whatever allow holds. |
| `deny_files` | list of string |  | Files of more deny entries, written as allow_files. |
<!-- END GENERATED -->

## HTTPS by default

A config that writes no listeners is served over HTTPS on 443, with certificates obtained with
[ACME](#acme) for the host names its routes match, and over HTTP on 80, which redirects to HTTPS
(308) and answers ACME's HTTP-01 challenge. A listener written with an empty `tls` gets the
certificates of the routes it serves. `auto_https` turns it off, turns the redirects off, or moves
the ports. Names that no public CA certifies (`localhost`, IP addresses, `*.internal`, wildcards)
get none. The [HTTPS guide](../guides/https.md) has more.

```fit
auto_https { disable_redirects true; https_port 8443 }
route { match { host example.com }; handler respond { body hello } }
```

### `auto_https`

<!-- BEGIN GENERATED: auto_https -->
<a id="cfg-auto-https"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `disable` | boolean |  | Turn HTTPS by default off: no listeners are made for a config that writes none, and an empty tls gets no certificates. |
| `disable_redirects` | boolean |  | Make no listener in the clear to redirect to HTTPS. ACME's HTTP-01 challenge is then answered by none, and certificates come through TLS-ALPN-01. |
| `http_port` | integer, 0 to 65535 | `80` | The port of the listener in the clear that redirects to HTTPS. 80 when left out or 0. |
| `https_port` | integer, 0 to 65535 | `443` | The port of the HTTPS listener, and the one redirects name when it is not 443. 443 when left out or 0. |
<!-- END GENERATED -->

## Metrics

Counts and times requests by listener and route, for the Admin API's `/metrics` in Prometheus's
text format, which serves the rest of akit's metrics without it (upstreams, certificates,
connections, reloads, akio's engine). It costs every request a little, so it is off unless written,
and needs `admin`. It has no fields: `{}` turns it on.

```fit
admin {}
metrics {}
listener web :8080
route { handler respond }
```

## ACME

How certificates are obtained and renewed with ACME, for HTTPS by default and for the names a
listener's `tls.acme` lists: the CA's directory (Let's Encrypt's by default), the account's email,
where accounts and certificates are kept (a directory, or Redis, etcd or Consul for instances on
several hosts, `shared_storage`), the challenges answered (HTTP-01 and TLS-ALPN-01), and
`on_demand`, which says whom to ask before a certificate is obtained during a handshake. A
certificate is renewed by its lifetime or by the CA's renewal information (ARI), and its OCSP
response stapled. Read when akit starts; obtaining, renewing and the handshakes that wait for a
certificate never hold an I/O thread.

```fit
acme {
	email ops@example.com
	directory https://acme-staging-v02.api.letsencrypt.org/directory
	storage acme
	on_demand { ask http://localhost:5555/allowed }
}

listener https :443 { tls { on_demand true } }

route { handler respond { body hello } }
```

### `acme`

<!-- BEGIN GENERATED: acme -->
<a id="cfg-acme"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `ca_file` | string |  | A PEM file of the certificates the directory's is checked against, in place of the system's roots, for a CA of one's own. |
| `challenges` | list of string: `http-01`, `tls-alpn-01` | `["http-01", "tls-alpn-01"]` | The challenges answered, in the order tried: http-01, which the CA checks on port 80 at the name, answered by any listener of akit's in the clear, and tls-alpn-01, which it checks on port 443, answered by the TLS listener that lists the name. A failed attempt is tried again with the next. Both, http-01 first, when left out. |
| `directory` | string | `"https://acme-v02.api.letsencrypt.org/directory"` | The URL of the CA's ACME directory. Let's Encrypt's when left out; https://acme-staging-v02.api.letsencrypt.org/directory is its staging CA, for tests. |
| `email` | string |  | The address the CA may write to about the account's certificates; none when left out. |
| `max_concurrent` | integer, 0 or more | `2` | How many certificates are obtained at once; the rest wait their turn. 2 when left out or 0. |
| `on_demand` | [object](#cfg-acme-on-demand) |  | How certificates are obtained during handshakes, for listeners whose tls sets on_demand: whom to ask whether a name may have one, and how fast and how many. |
| `request_timeout` | duration | `"30s"` | How long each request to the CA may take. 30s when left out. |
| `shared_storage` | [object](#cfg-acme-shared-storage) |  | Keep accounts, keys and certificates in Redis, etcd or Consul instead of a directory, for instances of akit on several hosts: one instance obtains a certificate, holding a lock there, and the others take it from there; each challenge's answer is kept there too, so that whichever instance the CA reaches answers it. Not with storage. |
| `storage` | string |  | The directory accounts, keys and certificates are kept in, which instances of akit may share. $XDG_DATA_HOME/akit, or ~/.local/share/akit, when left out. **akit:** akit's own directory, not fit's: the two do not share accounts or certificates unless `storage` names one. |
| `timeout` | duration | `"10m"` | How long obtaining one certificate may take, challenges included, before it is given up and tried again later. 10m when left out. |
<!-- END GENERATED -->

### `acme.on_demand`

<!-- BEGIN GENERATED: acme.on_demand -->
<a id="cfg-acme-on-demand"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `ask` | string | required | A URL akit asks, with the name in the query as domain=<name>, whether a name may have a certificate: an answer of 2xx allows it, and any other refuses it. Refusals are remembered for a minute. At most 8 asks are under way at once, and 5 a second on average, 20 at once, however they are answered; a handshake past them fails without asking. |
| `idle_timeout` | duration | `"168h"` | How long a name taken on demand is kept, and renewed, with no handshake asking for it; then it is dropped and no longer counts against max_names, and its certificate stays in the storage for the next handshake that asks for it, which the ask URL must allow again. 168h, a week, when left out. |
| `interval` | duration | `"1m"` | The interval rate counts over. 1m when left out. |
| `max_names` | integer, 0 or more | `1000` | How many names may have certificates obtained on demand, in all; past it, a handshake for another fails. 1000 when left out or 0. |
| `rate` | integer, 0 or more | `10` | How many certificates may be obtained every interval; a handshake past it fails. 10 when left out or 0. |
| `timeout` | duration | `"10s"` | How long a handshake waits for its name's certificate, as long as akio's handshake timeout by default. 10s when left out. |
<!-- END GENERATED -->

### `acme.shared_storage`

One of `redis`, `etcd` and `consul`. One instance obtains a certificate, holding a leased lock in
the store, and the others take it from there; each challenge's answer is kept there too, so that
whichever instance the CA reaches answers it.

```fit
acme {
	email ops@example.com
	shared_storage {
		redis { address redis:6379 }
	}
}

route { match { host example.com }; handler respond { body hello } }
```

<!-- BEGIN GENERATED: acme.shared_storage -->
<a id="cfg-acme-shared-storage"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `consul` | [object](#cfg-acme-shared-storage-consul) |  | Keep them in Consul's KV store, with locks held by sessions. |
| `etcd` | [object](#cfg-acme-shared-storage-etcd) |  | Keep them in etcd, through its v3 JSON gateway, with locks held by leases. |
| `lock_ttl` | duration | `"30s"` | How long a lock outlives an instance that stopped holding it without giving it back; its holder renews it every third of that. At least 10s; 30s when left out. |
| `prefix` | string | `"akit/"` | What the keys kept start with, as akit/certificates/<CA>/<name>.pem; instances that share certificates share it. akit/ when left out. **akit:** fit's default is `fit/`, renamed (decision 037). |
| `redis` | [object](#cfg-acme-shared-storage-redis) |  | Keep them in Redis, a standalone server or a primary (not Redis Cluster), with locks of SET NX and an expiry. |
| `request_timeout` | duration | `"10s"` | How long each request to the store may take, connecting included. 10s when left out. |
<!-- END GENERATED -->

### `acme.shared_storage.redis`

<!-- BEGIN GENERATED: acme.shared_storage.redis -->
<a id="cfg-acme-shared-storage-redis"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `address` | string | `"127.0.0.1:6379"` | The server's host and port. 127.0.0.1:6379 when left out. |
| `ca_file` | string |  | A PEM file with the CAs the server's certificate is verified with, over TLS; the system's when left out. |
| `cert_file` | string |  | A PEM file with the client certificate sent over TLS; with key_file. |
| `db` | integer, 0 or more |  | The database selected. 0 when left out. |
| `key_file` | string |  | A PEM file with cert_file's private key. |
| `password_file` | string |  | A file holding the password to authenticate with, read at each connection; none when left out. |
| `tls` | boolean |  | Connect over TLS. |
| `username` | string |  | The ACL user to authenticate as, with password_file; the default user when left out. |
<!-- END GENERATED -->

### `acme.shared_storage.etcd`

<!-- BEGIN GENERATED: acme.shared_storage.etcd -->
<a id="cfg-acme-shared-storage-etcd"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `ca_file` | string |  | A PEM file with the CAs etcd's certificate is verified with, over https; the system's when left out. |
| `cert_file` | string |  | A PEM file with the client certificate sent over https; with key_file. |
| `endpoints` | list of string | `["http://127.0.0.1:2379"]` | Where etcd's client API is: http:// or https:// and a host and port, for each member to try, in turn when one fails. http://127.0.0.1:2379 when left out. |
| `key_file` | string |  | A PEM file with cert_file's private key. |
| `password_file` | string |  | A file holding username's password, read each time akit authenticates. |
| `username` | string |  | The user to authenticate as, with password_file, when etcd's authentication is on; none when left out. |
<!-- END GENERATED -->

### `acme.shared_storage.consul`

<!-- BEGIN GENERATED: acme.shared_storage.consul -->
<a id="cfg-acme-shared-storage-consul"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `ca_file` | string |  | A PEM file with the CAs Consul's certificate is verified with, over https; the system's when left out. |
| `cert_file` | string |  | A PEM file with the client certificate sent over https; with key_file. |
| `datacenter` | string |  | The datacenter whose KV store is used; the agent's when left out. |
| `endpoint` | string | `"http://127.0.0.1:8500"` | Where Consul's HTTP API is, as of the local agent: http:// or https:// and a host and port. http://127.0.0.1:8500 when left out. |
| `key_file` | string |  | A PEM file with cert_file's private key. |
| `token_file` | string |  | A file holding the ACL token to send, read when akit starts; none when left out. The token needs to read and write the keys under prefix and to make sessions. |
<!-- END GENERATED -->

## Providers

Providers add listeners, routes, upstreams and services from outside the config, following them as
they change: Docker containers' labels, a Consul catalog's tags, keys in Consul's KV store or etcd,
and a Kubernetes cluster's Gateway API resources and Ingresses. What the config writes wins over
every provider, and a provider over those after it; the Admin API's `/config/effective` shows the
result and where each part came from. Each provider's part is applied as a version of its own. Read
when akit starts.

```fit
listener web :80

provider { docker { endpoint unix:///var/run/docker.sock } }
provider kv { etcd { endpoints http://etcd:2379; prefix akit/ } }
```

### `providers[]`

<!-- BEGIN GENERATED: providers[] -->
<a id="cfg-providers"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `consul` | [object](#cfg-providers-consul) |  | Routes, upstreams and services from the tags of the services in a Consul catalog, sending to their instances whose health checks pass. |
| `consul_kv` | [object](#cfg-providers-consul-kv) |  | Listeners, routes, upstreams and services from the keys under a prefix in Consul's KV store, each holding a JSON object of them. |
| `docker` | [object](#cfg-providers-docker) |  | Routes, upstreams and services from the labels of running Docker containers. |
| `etcd` | [object](#cfg-providers-etcd) |  | Listeners, routes, upstreams and services from the keys under a prefix in etcd, each holding a JSON object of them. |
| `kubernetes` | [object](#cfg-providers-kubernetes) |  | Listeners, routes, services and upstreams from a Kubernetes cluster's Gateway API resources: the Gateways of the GatewayClasses whose controllerName is akit's, and their HTTPRoutes, GRPCRoutes, TLSRoutes, TCPRoutes and UDPRoutes, whose status akit writes back. |
| `name` | string |  | The name the provider goes by in the Admin API's /config/effective and in logs: lower-case letters, digits, '_', '.' and '-', unique among the providers. Its kind, as docker, when left out. |
<!-- END GENERATED -->

### `providers[].docker`

<!-- BEGIN GENERATED: providers[].docker -->
<a id="cfg-providers-docker"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `endpoint` | string | `"unix:///var/run/docker.sock"` | Where the Docker daemon's API is: unix:// and a socket's path, or tcp:// and a host and port, as of a socket proxy, in plain HTTP. unix:///var/run/docker.sock when left out. |
| `expose_all` | boolean |  | Take every running container with routes or services in its labels; only those labelled <label_prefix>.enable=true when left out or false. A container labelled <label_prefix>.enable=false is never taken. |
| `label_prefix` | string | `"akit"` | What the labels read start with, as in akit.routes.web.host. akit when left out. **akit:** fit's default is `fit`, renamed (decision 037). |
| `max_containers` | integer, 0 or more | `1000` | The most containers taken; past it, the rest, by name, are left out and reported. 1000 when left out or 0. |
| `network` | string |  | The Docker network whose address of a container's is used, when the container is on several and has no <label_prefix>.network label; the first by name when left out. |
| `refresh_interval` | duration | `"30s"` | How often the containers are listed again whether or not an event came, in case one was missed. 30s when left out. |
| `request_timeout` | duration | `"10s"` | How long listing the containers may take, connecting included. 10s when left out. |
| `use_published_ports` | boolean |  | Send to the port a container's port is published on, on the host, rather than to the container's own address: for a akit outside Docker that cannot reach containers' addresses, as on Docker Desktop. |
<!-- END GENERATED -->

### `providers[].consul`

<!-- BEGIN GENERATED: providers[].consul -->
<a id="cfg-providers-consul"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `ca_file` | string |  | A PEM file with the CAs Consul's certificate is verified with, over https; the system's when left out. |
| `cert_file` | string |  | A PEM file with the client certificate sent over https, as Consul's verify_incoming asks for; with key_file. |
| `datacenter` | string |  | The datacenter whose catalog is read; the agent's when left out. |
| `endpoint` | string | `"http://127.0.0.1:8500"` | Where Consul's HTTP API is, as of the local agent: http:// or https:// and a host and port. http://127.0.0.1:8500 when left out. |
| `expose_all` | boolean |  | Take every service in the catalog; only those tagged <tag_prefix>.enable=true when left out or false. An instance tagged <tag_prefix>.enable=false is never taken. |
| `key_file` | string |  | A PEM file with cert_file's private key. |
| `max_services` | integer, 0 or more | `1000` | The most services taken; past it, the rest, by name, are left out and reported. 1000 when left out or 0. |
| `refresh_interval` | duration | `"30s"` | How often the catalog is read again whether or not it changed. 30s when left out. |
| `request_timeout` | duration | `"10s"` | How long each request to Consul may take, connecting included, beyond the five minutes a blocking query waits for a change. 10s when left out. |
| `tag_prefix` | string | `"akit"` | What the tags read start with, as in akit.routes.web.host=example.com; tags without '=' are passed over. Besides the routes, services and upstream settings a Docker container's labels may set, <tag_prefix>.name names the upstream, the service's name when left out. akit when left out. **akit:** fit's default is `fit`, renamed (decision 037). |
| `token_file` | string |  | A file holding the ACL token to send, read each time a watch starts; none when left out. The token needs to read the services and nodes served. |
<!-- END GENERATED -->

### `providers[].consul_kv`

<!-- BEGIN GENERATED: providers[].consul_kv -->
<a id="cfg-providers-consul-kv"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `ca_file` | string |  | A PEM file with the CAs Consul's certificate is verified with, over https; the system's when left out. |
| `cert_file` | string |  | A PEM file with the client certificate sent over https, as Consul's verify_incoming asks for; with key_file. |
| `datacenter` | string |  | The datacenter whose KV store is read; the agent's when left out. |
| `endpoint` | string | `"http://127.0.0.1:8500"` | Where Consul's HTTP API is, as of the local agent: http:// or https:// and a host and port. http://127.0.0.1:8500 when left out. |
| `key_file` | string |  | A PEM file with cert_file's private key. |
| `max_keys` | integer, 0 or more | `1000` | The most keys taken; past it, the rest, by key, are left out and reported. 1000 when left out or 0. |
| `prefix` | string | `"akit/"` | The keys read are those starting with it. Each holds a JSON object of listeners, routes, upstreams and services, as a config holds them; keys ending in '/' and empty ones are passed over, and a key whose value is not such an object is left out and reported. akit/ when left out. **akit:** fit's default is `fit/`, renamed (decision 037). |
| `refresh_interval` | duration | `"30s"` | How often what the keys provide is sent again whether or not it changed, so that a part refused because of the rest of the config is tried again. 30s when left out. |
| `request_timeout` | duration | `"10s"` | How long each request to Consul may take, connecting included, beyond the five minutes a blocking query waits for a change. 10s when left out. |
| `token_file` | string |  | A file holding the ACL token to send, read each time a watch starts; none when left out. The token needs to read the keys under prefix. |
<!-- END GENERATED -->

### `providers[].etcd`

<!-- BEGIN GENERATED: providers[].etcd -->
<a id="cfg-providers-etcd"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `ca_file` | string |  | A PEM file with the CAs etcd's certificate is verified with, over https; the system's when left out. |
| `cert_file` | string |  | A PEM file with the client certificate sent over https, as etcd's client-cert-auth asks for; with key_file. |
| `endpoints` | list of string | `["http://127.0.0.1:2379"]` | Where etcd's client API is: http:// or https:// and a host and port, for each member to try, in turn when one fails. http://127.0.0.1:2379 when left out. |
| `key_file` | string |  | A PEM file with cert_file's private key. |
| `max_keys` | integer, 0 or more | `1000` | The most keys taken; past it, the rest, by key, are left out and reported. 1000 when left out or 0. |
| `password_file` | string |  | A file holding username's password, read each time a watch starts. |
| `prefix` | string | `"akit/"` | The keys read are those starting with it. Each holds a JSON object of listeners, routes, upstreams and services, as a config holds them; keys ending in '/' and empty ones are passed over, and a key whose value is not such an object is left out and reported. akit/ when left out. **akit:** fit's default is `fit/`, renamed (decision 037). |
| `refresh_interval` | duration | `"30s"` | How often the keys are read again whether or not a watch event came. 30s when left out. |
| `request_timeout` | duration | `"10s"` | How long each request to etcd may take, connecting included, and a watch to start; a watch then lasts until it fails. 10s when left out. |
| `username` | string |  | The user to authenticate as, with password_file, when etcd's authentication is on; none when left out. |
<!-- END GENERATED -->

### `providers[].kubernetes`

The Gateway API's Gateways of akit's GatewayClasses, with their HTTPRoutes, GRPCRoutes, TLSRoutes,
TCPRoutes and UDPRoutes, and the Ingresses of akit's IngressClasses, listed and watched, and served
to their Services' ready endpoints; the Gateway API's statuses are written back.

```fit
provider k8s {
	kubernetes {
		namespaces default
		ingress { http_port 8080; https_port 8443 }
	}
}
```

<!-- BEGIN GENERATED: providers[].kubernetes -->
<a id="cfg-providers-kubernetes"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `addresses` | list of string |  | The addresses where clients reach akit's Gateways, IP addresses or host names, written into each one's status.addresses, as clients of the Gateway API and its conformance tests read them. None are written when left out, and those there are kept. |
| `allow_external_name_services` | boolean |  | **Not supported yet**: refused by name. Send requests to the backends of ExternalName Services, to the host name each names, refusing one that is loopback, link-local (169.254.169.254, the metadata service of most clouds, among them), unspecified or multicast, by address or by the address the name resolves to, or the host's own: whoever may create a Service could otherwise point a route at fit's Admin API or the cloud's metadata service. Off when left out: a backend that is an ExternalName Service is not resolved, and its route says so in its status. |
| `backends` | string: `endpoints`, `cluster_ip` | `"endpoints"` | Where requests to a Service go: endpoints, to its ready endpoints from its EndpointSlices, balanced by akit; or cluster_ip, to its cluster IP, balanced by the cluster. endpoints when left out. |
| `certificates_dir` | string |  | The directory the certificates of HTTPS listeners are written to, from the Secrets they name, for akit's TLS to load: each Secret's chain and key in one file only akit's user may read, named for the Secret and a hash of its contents. Files of that form that are no longer named are removed. A directory of akit's own in the system's temporary directory, removed when akit stops, when left out. |
| `context` | string |  | The kubeconfig's context to use; its current-context when left out. |
| `controller_name` | string | `"lesismal.github.io/akit"` | The controllerName of the GatewayClasses whose Gateways akit serves. lesismal.github.io/akit when left out. **akit:** fit's default is `lesismal.github.io/fit`, renamed (decision 037). |
| `ingress` | [object](#cfg-providers-kubernetes-ingress) |  | Serve Ingresses too: those whose ingressClassName, or kubernetes.io/ingress.class annotation, names an IngressClass whose controller is controller_name, and those naming none when such a class is the default. Their rules are served on http_port, and on https_port for the hosts of their tls, with the certificates of its Secrets. akit's account then needs to list and watch Ingresses and IngressClasses. No Ingresses when left out. |
| `kubeconfig` | string |  | A kubeconfig file to reach the cluster with: its server, CA, client certificate, token or token file. When left out, the pod's service account, inside the cluster, or else $KUBECONFIG or ~/.kube/config. Exec and auth-provider plugins are not supported. |
| `listen_host` | string |  | The IP address Gateway listeners are bound on, each on its port; every address when left out. |
| `max_objects` | integer, 0 or more | `10000` | The most objects of each kind kept: past it, the rest are left out and reported. 10000 when left out or 0. |
| `namespaces` | list of string |  | The namespaces whose Gateways and routes are served; every namespace when left out. Resources are watched across the cluster either way, so akit's account needs to list and watch them cluster-wide, Secrets of type kubernetes.io/tls among them, and to patch the status of GatewayClasses, Gateways and routes. |
| `request_timeout` | duration | `"10s"` | How long each request to the API server may take, but watches, which last five minutes each. 10s when left out. |
<!-- END GENERATED -->

### `providers[].kubernetes.ingress`

<!-- BEGIN GENERATED: providers[].kubernetes.ingress -->
<a id="cfg-providers-kubernetes-ingress"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `http_port` | integer | `80` | The port Ingresses' rules are served on over HTTP. 80 when left out or 0. |
| `https_port` | integer | `443` | The port the hosts of Ingresses' tls are served on over HTTPS. 443 when left out or 0. |
<!-- END GENERATED -->

## Handlers

A route's `handler` answers its requests: one of these, named by `module`, with its `params`. A
module of another crate, built into akit with `akit build` ([modules](../modules.md)), is named
the same way, its params checked by the module.

<!-- BEGIN GENERATED: modules:routes[].handler -->
| Module | Status |
| --- | --- |
| `bt_publish` | **not supported yet**: refused by name |
| `bt_stream` | **not supported yet**: refused by name |
| `bt_tracker` | **not supported yet**: refused by name |
| `fastcgi` | supported: [params](#cfg-routes-handler-fastcgi-params) |
| `forward_proxy` | supported: [params](#cfg-routes-handler-forward-proxy-params) |
| `hls` | **not supported yet**: refused by name |
| `httpflv` | **not supported yet**: refused by name |
| `media_pages` | **not supported yet**: refused by name |
| `media_tokens` | **not supported yet**: refused by name |
| `proxy` | supported: [params](#cfg-routes-handler-proxy-params) |
| `respond` | supported: [params](#cfg-routes-handler-respond-params) |
| `static` | supported: [params](#cfg-routes-handler-static-params) |
| `stream_whep` | **not supported yet**: refused by name |
| `stream_whip` | **not supported yet**: refused by name |
| `sync_ingest` | **not supported yet**: refused by name |
| `vod` | **not supported yet**: refused by name |
| `webrtc_proxy` | **not supported yet**: refused by name |
| `websockify` | **not supported yet**: refused by name |
| `whep` | **not supported yet**: refused by name |
| `whip` | **not supported yet**: refused by name |
<!-- END GENERATED -->

### `respond`

A fixed answer: a health check, a maintenance page, a 404 for what no other route takes.

```fit
listener web :8080

route {
	match { path /health }
	handler respond { status 200; body ok; headers { Cache-Control no-store } }
}
```

<!-- BEGIN GENERATED: routes[].handler<respond>.params -->
<a id="cfg-routes-handler-respond-params"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `body` | string |  | The response body. |
| `headers` | map of string |  | Response headers. Content-Type defaults to text/plain; charset=utf-8 when there is a body. |
| `status` | integer, 100 to 599 | `200` | The status code; 200 when left out. |
<!-- END GENERATED -->

### `static`

Serves files from `root`, small ones from memory, with conditional and range requests, listings
(`browse`), precompressed twins, compressed copies of files kept in memory, and `try_files` for
single-page apps. Files are opened and read on akio's file pool, never on an I/O thread. A path with
a segment that starts with a dot (`/.git/config`, `/.env`) is answered 404, but `/.well-known/`;
`dotfiles: allow` serves them. The [static files guide](../guides/static-files.md) has more.

A request cannot name a file outside `root` (a `..` segment, a NUL or a backslash is refused with
400), but symbolic links under `root` are followed wherever they lead, as fit follows them: whoever
can create a link under the root decides what it serves. Keep the root writable by its publisher
alone, and leave links out of it unless they are meant
([threat model, L7](../security/threat-model.md#known-limitations)).

```fit
route {
	match { host example.com }
	handler static {
		root public
		try_files {path} {path}/ /index.html
		precompressed br gzip
		compress zstd br gzip
	}
}
```

<!-- BEGIN GENERATED: routes[].handler<static>.params -->
<a id="cfg-routes-handler-static-params"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `root` | string | required | The directory files are served from. On a file system that folds case, as Windows' and macOS' do by default, a request whose path is not written as the names on disk are, or for a file whose extension a fastcgi handler's split_path names, in any case, is answered 404, so that a route matching names, as \.php$, is not passed by. |
| `browse` | boolean |  | List a directory that has no index file, rather than answering 404. Files whose names start with a dot are left out. |
| `browse_max_entries` | integer, 0 or more | `10000` | The most entries a listing reads and shows; a directory with more is listed in part, and says so. 10000 when left out or 0. |
| `check_interval` | duration | `"5s"` | How long what the handler learned of a request path, the file it names and its validators, is trusted before the disk is looked at again, where no watch reports its changes. On Linux a watch of the directories under the root reports a change within moments, a file written, replaced or removed, a directory renamed, or a symbolic link on the way to the root pointed elsewhere, and what it watches is trusted until it does; this interval is for other systems, and for a file that is a symbolic link, whose target's changes the watch does not see. A file is told by its size, modification and status change times and inode, so one replaced by another of the same size and modification time is seen too. 0 looks at the disk for every request, with no watch, so that a response always carries the file as it is on disk then, also on a network file system, whose changes from other machines no watch sees. 5s when left out. |
| `compress` | list of string: `zstd`, `br`, `gzip` |  | Encodings a file kept in memory is also kept compressed with, for a client that accepts one: the one it prefers by Accept-Encoding, and among equals the first listed. Each copy is made once, on the file pool, at a level that makes it small (gzip 9, brotli 9, zstd's best), the first time a client asks for it, and kept until the file changes; until then the file goes as it is, for a compress middleware to compress. A file smaller than 1400 bytes, of a type that does not compress, as images and archives, or that compresses by less than a twentieth is not; nor is a ranged request answered from a copy. The copies count in max_cache_bytes. A precompressed twin on disk goes ahead of a copy. None when left out. |
| `default_type` | string |  | The Content-Type of a file whose extension gives none, as nginx's default_type, such as application/octet-stream; when left out, such a file's type is sniffed from its first 512 bytes, as net/http's is, which costs a read of a file not in memory. |
| `disk_io` | string: `pool`, `auto` | `"pool"` | Where a request that needs the disk looks at it, for a file the cache does not hold: pool on the file pool, which keeps a disk that hangs off akio's I/O threads altogether, a request waiting on it past the pool's timeout answered 503; auto on akio's worker that serves it, as nginx does in its event loop, while no more requests than a quarter of the process's CPUs do so at once, and on the file pool past that. A request on akio's worker waits for the disk however long it takes: with auto a disk that hangs holds no more of akio's I/O threads than that quarter, and the requests past them get the pool's 503. On a disk that answers from memory, as tmpfs or a page cache that holds the files does, auto serves a few percent more than pool (2.6% at 20 KiB files), with the 99th percentile at saturation 5-12% longer. pool when left out. **akit:** Only `pool`: akit looks at the disk on akio's file pool, never on an I/O thread, and refuses `auto` as not supported yet. |
| `dotfiles` | string: `deny`, `allow` | `"deny"` | Whether files and directories whose names start with a dot, as .git, .env and .htpasswd do, are served: deny answers a request for one, or for anything under a dot directory, 404, as for a file not there, but /.well-known/ under the root, for ACME's HTTP-01 challenge and security.txt; allow serves them as other files. Listings leave them out either way. On Windows deny also hides a path with a segment shaped as an 8.3 short name, as GIT~1 for .git, which NTFS may give a dotfile. deny when left out. |
| `index` | list of string | `["index.html"]` | The files that stand for a directory, tried in order; ["index.html"] when left out, and none for []. |
| `max_cache_bytes` | integer, 0 or more | `67108864` | How many bytes of files are kept in memory at most; 64 MiB when left out. |
| `max_file_bytes` | integer, 0 or more | `1048576` | The largest file kept in memory, in bytes; 1 MiB when left out. |
| `precompressed` | list of string: `br`, `zstd`, `gzip` |  | Encodings whose compressed twins, name.br, name.zst and name.gz, are served in place of a file to a client that accepts them: the one it prefers by Accept-Encoding, and among equals the first listed. None when left out. |
| `strip_prefix` | string |  | A prefix removed from the request path before it is looked up under the root; a request whose path lacks it gets 404. |
| `try_files` | list of string |  | Paths tried in turn for a request, as nginx's try_files: {path} stands for the request's path, and one ending in / for a directory, served by its index file. The first that exists is served, and when none does the answer is 404; ["{path}", "{path}/", "/index.html"] serves a single-page app. A directory asked for without its slash is not redirected then. |
<!-- END GENERATED -->

### `proxy`

Proxies to an [upstream](#upstreams) by name; or, with `upstreams`, splits requests over several by
weight, as a canary release does. `failover` names an upstream that takes over when none of the
first's addresses is in rotation, `mirror` copies a share of the requests elsewhere (requests whose
bodies came whole, at most 256 under way, never waited for), and `retry` sends a failed request
again, within a budget. Bodies are streamed both ways with linked backpressure; WebSockets (HTTP/1.1
upgrades and HTTP/2 and HTTP/3 extended CONNECTs) and gRPC calls are proxied too. The upstream
request is given up whenever the client goes. The [reverse proxy guide](../guides/reverse-proxy.md)
has more.

```fit
upstream app 10.0.0.1:8080
upstream app-canary 10.0.0.2:8080
upstream app-backup 10.0.0.3:8080

route {
	match { host example.com; path /api/* }
	handler proxy {
		upstreams { upstream app; weight 95 }
		upstreams { upstream app-canary; weight 5 }
		failover app-backup
		host_header api.internal
		retry { attempts 2; statuses 502 503 }
		timeout 30s
	}
}
```

<!-- BEGIN GENERATED: routes[].handler<proxy>.params -->
<a id="cfg-routes-handler-proxy-params"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `failover` | string |  | An upstream that takes the requests when none of the chosen upstream's backends is in rotation, for failing or for its health checks, and one of its own is. |
| `host_header` | string |  | The Host header sent upstream; the client's when left out. |
| `mirror` | [object](#cfg-routes-handler-proxy-params-mirror) |  | Send a copy of a share of the requests whose bodies arrive whole (64KB at most) to another upstream, and drop its answers. At most 256 copies are under way at once; past that, requests are not copied. The requests themselves do not wait for their copies. |
| `retry` | [object](#cfg-routes-handler-proxy-params-retry) |  | Send a request that failed again, to the backend the upstream picks next: one whose connection could not be made, whatever its method, and, for GET, HEAD, OPTIONS, TRACE, PUT and DELETE, one that failed before its response began or was answered with a status of statuses. A request whose body streams (past 64KB, or still arriving) is never sent again. No retries when left out. |
| `splice_uploads` | boolean | `true` | **Not supported yet**: refused by name. On Linux, move a large upload (128KB or more, framed by its Content-Length) from a client's HTTP/1.1 connection without TLS to an HTTP/1.1 upstream without TLS with splice(2), the kernel passing its pages from one socket to the other, rather than copying it through the proxy. An upload that cannot go so, to an h2, h2c or TLS upstream, or chunked, goes the ordinary way. It costs the proxy about 40% less CPU on such uploads and the upstream about 5% more, which reads the body cold rather than as the proxy just wrote it, and more once the upstream itself is the bottleneck: false suits an upstream whose CPU is scarcer than the proxy's. False copies every upload. True when left out. **akit:** Not planned yet: `splice(2)` needs `unsafe` or an akio API. |
| `timeout` | duration |  | The most a request may take, from when the proxy takes it to the end of its response, retries included: one past it before its response began is answered 504, and one whose response is under way is cut short. None when left out. |
| `trusted_proxies` | list of string |  | The addresses ("10.0.0.1") and networks ("10.0.0.0/8") of proxies in front of this one, whose X-Forwarded-* and Forwarded fields are believed. None when left out. |
| `tunnel_idle_timeout` | duration | `"10m"` | How long a tunnel, as a WebSocket the proxy has joined to its upstream is, may carry nothing either way before it is closed; 10m when left out. |
| `upstream` | string |  | The name of the upstream requests go to; or upstreams. |
| `upstreams` | list of [object](#cfg-routes-handler-proxy-params-upstreams) |  | Upstreams requests are spread over at random by weight, as a canary release does, in place of upstream. |
<!-- END GENERATED -->

#### `upstreams[]` in `proxy`

<!-- BEGIN GENERATED: routes[].handler<proxy>.params.upstreams[] -->
<a id="cfg-routes-handler-proxy-params-upstreams"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `upstream` | string | required | The name of the upstream. |
| `weight` | integer, 1 to 1000 | required | Its share of the requests, against the others' weights. |
<!-- END GENERATED -->

#### `mirror` in `proxy`

<!-- BEGIN GENERATED: routes[].handler<proxy>.params.mirror -->
<a id="cfg-routes-handler-proxy-params-mirror"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `upstream` | string | required | The name of the upstream the copies go to. |
| `percent` | number, 0 to 100 | `100` | The share of requests copied, chosen at random; all of them when left out. |
<!-- END GENERATED -->

#### `retry` in `proxy`

<!-- BEGIN GENERATED: routes[].handler<proxy>.params.retry -->
<a id="cfg-routes-handler-proxy-params-retry"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `attempts` | integer, 1 to 10 | `2` | How many times a request may be sent again after the first; 2 when left out. |
| `budget_min_per_second` | number, 0 or more | `10` | Retries allowed each second beyond budget_ratio, so that a proxy taking few requests may still retry; 10 when left out. |
| `budget_ratio` | number, 0 to 1 | `0.2` | Retries may add at most this share of the requests the proxy takes, so that retries cannot multiply the load on an upstream already failing; 0.2 when left out. |
| `statuses` | list of integer |  | Statuses of the upstream's, 5xx ones, that have a request of an idempotent method sent again, as 502, 503 and 504; none when left out, so that only requests that got no response are. |
<!-- END GENERATED -->

### `fastcgi`

Answers requests from a FastCGI application, as PHP-FPM is, over TCP or a Unix socket, through a
bounded pool of connections, bodies streamed both ways. `try_files` sends paths that are no script
of their own to a front controller, as most PHP frameworks want. A script whose path has a segment
that starts with a dot is not run but answered 404; `dotfiles: allow` runs it.

```fit
route {
	match { host example.com }
	handler fastcgi {
		address unix//run/php/php-fpm.sock
		root /var/www/app/public
		try_files {path} {path}/index.php /index.php
	}
}
```

<!-- BEGIN GENERATED: routes[].handler<fastcgi>.params -->
<a id="cfg-routes-handler-fastcgi-params"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `address` | string | required | Where the application listens: a host and port, as "127.0.0.1:9000", or "unix/" and a socket's path, as "unix//run/php/php-fpm.sock". |
| `root` | string | required | The document root, which SCRIPT_FILENAME, DOCUMENT_ROOT and PATH_TRANSLATED are made from, as the application sees it. |
| `connect_timeout` | duration | `"5s"` | How long connecting may take; 5s when left out. A request that times out connecting is answered 504. |
| `dotfiles` | string: `deny`, `allow` | `"deny"` | Whether scripts whose paths have a segment starting with a dot, as /.git/x.php or /.env, run: deny answers a request for one 404, and try_files passes over it, but /.well-known/ under the root; allow runs them as other scripts. On Windows deny also refuses a script with a segment shaped as an 8.3 short name, as GIT~1 for .git, which NTFS may give a dotfile. The path info after a script is not looked at. deny when left out. |
| `env` | map of string |  | More parameters for the application, set after akit's own. |
| `idle_timeout` | duration | `"60s"` | How long an idle connection is kept; 60s when left out. |
| `index` | string | `"index.php"` | The script a path ending in / runs; index.php when left out. |
| `max_conns` | integer, 0 or more | `64` | The most connections to the application, each carrying one request at a time. 64 when left out or 0. |
| `max_header_bytes` | integer, 0 or more | `65536` | The most bytes the response's CGI header may take; past it the request is answered 502. 64 KiB when left out or 0. |
| `max_idle_conns` | integer, 0 or more | `16` | The most idle connections kept for later requests. 16 when left out or 0. |
| `max_pending` | integer, 0 or more | `1024` | How many requests may wait for a connection when max_conns are busy; past it a request is answered 503. 1024 when left out or 0. |
| `response_header_timeout` | duration | `"60s"` | How long the application may take to begin its response; 60s when left out. |
| `response_idle_timeout` | duration | `"60s"` | How long the application may go silent in the middle of a response; 60s when left out. |
| `split_path` | list of string | `[".php"]` | What ends a script's name in a path, the rest being PATH_INFO: "/index.php/a/b" runs /index.php with PATH_INFO /a/b. A static handler on a file system that folds case serves no file with one of these extensions, in any case. [".php"] when left out. |
| `timeout` | duration | `"10m"` | The most a whole exchange may take; 10m when left out. |
| `try_files` | list of string |  | Paths tried in turn, as nginx's try_files, for the script to run: {path} stands for the request's path, and the first whose script exists under root runs; ["{path}", "{path}/index.php", "/index.php"] sends what is no script of its own to the front controller, as most PHP applications want. Looked up on a pool of threads of its own. Left out, the path runs as it is. |
<!-- END GENERATED -->

### `forward_proxy`

An HTTP CONNECT proxy for the clients `clients` lists, to the destinations `allow` and `deny`
permit; a listener's [`socks`](#cfg-listeners-socks) does the same for SOCKS. Loopback, link-local
(`169.254.169.254` among them), unspecified and multicast addresses and the host's own are refused
unless `allow` names them; a host name is decided by the address it was connected to, before
anything is sent.

The proxy has no authentication of its own (no `Proxy-Authorization`, SOCKS5 without methods):
`clients` alone says who may use it, so keep it to the hosts meant to. Private ranges (`10.0.0.0/8`,
`172.16.0.0/12`, `192.168.0.0/16`, `100.64.0.0/10`, `fc00::/7`) are reachable unless `deny` lists
them, as in fit: a proxy whose clients come from outside should deny the internal network, as
`deny 10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 100.64.0.0/10 fc00::/7`
([threat model, L6](../security/threat-model.md#known-limitations)).

```fit
listener web :8080

route {
	match { method CONNECT }
	handler forward_proxy { clients 10.0.0.0/8; allow *.example.com; ports 443 }
}
```

<!-- BEGIN GENERATED: routes[].handler<forward_proxy>.params -->
<a id="cfg-routes-handler-forward-proxy-params"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `clients` | list of string | required | The addresses and CIDR ranges of the clients that may use the proxy, as "10.0.0.0/8"; others are answered 403. |
| `allow` | list of string |  | The destinations the proxy connects to: host names, "*.example.com" for every name below example.com, IP addresses and CIDR ranges, which also hold the address a host name resolves to. Every destination deny does not hold when left out, but for loopback, link-local (169.254.169.254, the metadata service of most clouds, among them), unspecified and multicast addresses and the host's own, which are refused unless a range here lies inside theirs, as "127.0.0.0/8" does and "0.0.0.0/0" does not, or, for the host's own, names that address alone; others are answered 403, or 502 for a host name whose address decides. |
| `connect_timeout` | duration | `"10s"` | How long resolving and connecting to a destination may take; past it the request is answered 504. 10s when left out. |
| `deny` | list of string |  | Destinations refused, written as in allow, whatever allow holds. |
| `idle_timeout` | duration | `"5m"` | How long a tunnel may carry nothing either way before it and its connection to the destination are closed. 5m when left out. |
| `ports` | list of integer |  | The destination ports allowed; any when left out. |
<!-- END GENERATED -->

## Middleware

A route's `middleware` is a list of these, each named by `module`, with its `params`, which a
request passes through in order before the handler; a middleware may answer itself, as
authentication does when it refuses. Each route's chain is built when the config is loaded.

<!-- BEGIN GENERATED: modules:routes[].middleware[] -->
| Module | Status |
| --- | --- |
| `basic_auth` | supported: [params](#cfg-routes-middleware-basic-auth-params) |
| `body_limit` | supported: [params](#cfg-routes-middleware-body-limit-params) |
| `cache` | supported: [params](#cfg-routes-middleware-cache-params) |
| `circuit_breaker` | supported: [params](#cfg-routes-middleware-circuit-breaker-params) |
| `client_cert` | supported: [params](#cfg-routes-middleware-client-cert-params) |
| `compress` | supported: [params](#cfg-routes-middleware-compress-params) |
| `concurrency_limit` | supported: [params](#cfg-routes-middleware-concurrency-limit-params) |
| `forward_auth` | supported: [params](#cfg-routes-middleware-forward-auth-params) |
| `headers` | supported: [params](#cfg-routes-middleware-headers-params) |
| `jwt` | supported: [params](#cfg-routes-middleware-jwt-params) |
| `rate_limit` | supported: [params](#cfg-routes-middleware-rate-limit-params) |
| `redirect` | supported: [params](#cfg-routes-middleware-redirect-params) |
| `rewrite` | supported: [params](#cfg-routes-middleware-rewrite-params) |
| `wasm` | supported: [params](#cfg-routes-middleware-wasm-params) |
<!-- END GENERATED -->

### `basic_auth`

Asks for a user name and password, checked against bcrypt hashes (`htpasswd -B` makes them) on
akio's pool, never on an I/O thread.

```fit
route {
	match { host example.com; path /admin/* }
	middleware basic_auth {
		realm admin
		users {
			username alice
			password_hash $2a$14$Zkx19XLiW6VYouLHR5NmfOFU0z2GTNmpkT/5qqR7hx4IjWJPDhjvG
		}
		user_header X-User
	}
	handler respond { body hello }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<basic_auth>.params -->
<a id="cfg-routes-middleware-basic-auth-params"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `users` | list of [object](#cfg-routes-middleware-basic-auth-params-users) | required | The users allowed in, with their passwords' bcrypt hashes. |
| `forward_authorization` | boolean |  | Leave the Authorization field, with the password, on the request the handler sees; it is removed when left out. |
| `realm` | string | `"akit"` | The realm the browser shows when it asks for a user and password; akit when left out. **akit:** fit's default is `fit`, renamed (decision 037). |
| `user_header` | string |  | A request field to set to the user's name for the handler, and the upstream a proxy sends the request to; any the client sent, also spelled with _ for -, is replaced. |
<!-- END GENERATED -->

#### `users[]` in `basic_auth`

<!-- BEGIN GENERATED: routes[].middleware[]<basic_auth>.params.users[] -->
<a id="cfg-routes-middleware-basic-auth-params-users"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `password_hash` | string | required | The bcrypt hash of the user's password, as htpasswd -B makes it. |
| `username` | string | required | The user's name; it may not contain a colon. |
<!-- END GENERATED -->

### `body_limit`

A route's own limit on request bodies, below the listeners' `limits.max_body_bytes`.

```fit
listener web :8080

route {
	match { path /upload }
	middleware body_limit { max_bytes 100000000 }
	handler respond { body ok }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<body_limit>.params -->
<a id="cfg-routes-middleware-body-limit-params"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `max_bytes` | integer, 0 or more | required | The most bytes a request's body may have. |
<!-- END GENERATED -->

### `cache`

Keeps responses by RFC 9111's rules, in a bounded memory tier shared by the routes of a zone and,
with `disk`, on disk, on a pool of its own; one request fetches what many ask for at once (the cache
lock), and the rest wait for it; answers stale while refreshing, or when the upstream fails, if told
to. Purged through the Admin API's `POST /cache/purge`, by URL, URL prefix, tag or zone. The
[caching guide](../guides/caching.md) has more.

```fit
listener web :8080

upstream app 127.0.0.1:9000

route {
	match { path /assets/* }
	middleware cache {
		zone assets
		max_bytes 256MiB
		default_ttl 10m
		stale_while_revalidate 1m
		disk { path cache/assets; max_bytes 10GiB }
	}
	handler proxy { upstream app }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<cache>.params -->
<a id="cfg-routes-middleware-cache-params"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `default_ttl` | duration |  | How long a response is fresh for when it gives no lifetime of its own (no s-maxage, max-age or Expires) and its status allows one to be assumed (200, 203, 300, 301, 308, 404, 405, 410, 414, 501): in place of a tenth of the time since its Last-Modified, at most a day, which is used when this is left out. |
| `disable_lock` | boolean |  | Lets every request that misses fetch for itself. When left out, a request that misses while another fetches the same response waits for that fetch, up to lock_timeout, and is answered from what it keeps (a cache lock), so that many requests for a response not kept make one fetch. |
| `disk` | [object](#cfg-routes-middleware-cache-params-disk) |  | A disk tier for the zone: every response kept in memory is written to disk too, on a pool of threads of its own, and one the memory no longer holds is read back from disk, on that pool, and kept in memory again. None when left out. Caches of one zone take the disk of the last one loaded. |
| `lock_timeout` | duration | `"5s"` | How long a request waits for another's fetch of its response before it goes on to fetch for itself. 5s when left out. |
| `max_bytes` | integer, 0 or more | `134217728` | The most bytes the zone keeps in memory, bodies, fields and keys counted; past it the least recently used responses go. 128 MiB when left out or 0. Caches of one zone take the size of the last one loaded. |
| `max_entry_bytes` | integer, 0 or more | `1048576` | The largest body kept; a larger response goes to its client without being kept. At most a sixteenth of max_bytes. 1 MiB when left out or 0. |
| `max_entry_size` | integer, 0 or more | `1048576` | Deprecated. Renamed max_entry_bytes; the old name goes in config version 2. |
| `max_size` | integer, 0 or more | `134217728` | Deprecated. Renamed max_bytes; the old name goes in config version 2. |
| `stale_if_error` | duration |  | How long past its lifetime a response may still be answered when fetching it again fails, with no response or a 500, 502, 503 or 504, for a response that does not say how long itself (Cache-Control stale-if-error, RFC 5861). None when left out. |
| `stale_while_revalidate` | duration |  | How long past its lifetime a response may still be answered while the cache fetches it again in the background, for a response that does not say how long itself (Cache-Control stale-while-revalidate, RFC 5861). None when left out. A response that says must-revalidate, proxy-revalidate or s-maxage is never answered stale. |
| `zone` | string | `"default"` | The store responses are kept in. Caches of the same zone, in one route or several, share it, its size and its keys; it is kept across reloads while a cache uses it. default when left out. |
<!-- END GENERATED -->

#### `disk` in `cache`

<!-- BEGIN GENERATED: routes[].middleware[]<cache>.params.disk -->
<a id="cfg-routes-middleware-cache-params-disk"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `path` | string | required | The directory the zone's responses are kept in, made if it is missing. It is the zone's alone: no other zone may name it. What it holds is found again when akit starts. |
| `max_bytes` | integer, 0 or more | `1073741824` | The most bytes the zone keeps on disk; past it the least recently used responses go. 1 GiB when left out or 0. |
| `max_entries` | integer, 0 or more | `100000` | The most responses the zone keeps on disk, which bounds the memory their index takes, a few hundred bytes each. 100000 when left out or 0. |
| `max_size` | integer, 0 or more | `1073741824` | Deprecated. Renamed max_bytes; the old name goes in config version 2. |
<!-- END GENERATED -->

### `circuit_breaker`

Answers 503 at once while too many of a route's requests fail or are slow, then lets a few
through to try again.

```fit
listener web :8080

upstream app 127.0.0.1:9000

route {
	middleware circuit_breaker {
		window 10s
		error_ratio 0.5
		slow_threshold 2s
		open_duration 30s
	}
	handler proxy { upstream app }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<circuit_breaker>.params -->
<a id="cfg-routes-middleware-circuit-breaker-params"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `error_ratio` | number, 0 to 1 | `0.5` | The share of failures in the window that opens it; 0.5 when left out. |
| `half_open_requests` | integer, 1 or more | `5` | How many requests it lets through, half-open, that must all fare well for it to close; 5 when left out. Others are answered 503 meanwhile. |
| `min_requests` | integer, 1 or more | `20` | How many requests the window must have counted before it can open; 20 when left out. |
| `open_duration` | duration | `"30s"` | How long it stays open before letting requests through to try; 30s when left out. |
| `slow_ratio` | number, 0 to 1 | `0.5` | With slow_threshold, the share of slow responses in the window that opens it; 0.5 when left out. |
| `slow_threshold` | duration |  | A response that takes longer than this to begin counts as slow; none is when left out. |
| `statuses` | list of integer |  | The statuses that count as failures; 500 to 599 when left out. |
| `window` | duration | `"10s"` | How far back requests are counted, in ten steps; 10s when left out. |
<!-- END GENERATED -->

### `client_cert`

Requires a client certificate on a route of a listener whose `tls.client_auth` mode is
`verify_if_given`, and passes its subject on.

```fit
listener mtls :8443 http/1.1 h2 {
	tls {
		certificates { cert_file certs/example.com.pem; key_file certs/example.com.key }
		client_auth { ca_files certs/ca.pem; mode verify_if_given }
	}
}

upstream app 127.0.0.1:9000

route {
	match { path /internal/* }
	middleware client_cert { subject_header X-Client-Subject }
	handler proxy { upstream app }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<client_cert>.params -->
<a id="cfg-routes-middleware-client-cert-params"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `optional` | boolean |  | Let a request without a certificate through as well, unauthenticated; refused with 403 Forbidden when left out. |
| `subject_header` | string |  | A request field to set to the certificate's subject, as in "CN=alice,O=Example", for the handler and the upstream a proxy sends the request to; any the client sent, also spelled with _ for -, is removed, with or without a certificate. |
<!-- END GENERATED -->

### `compress`

Compresses responses with zstd, brotli or gzip, by the client's `Accept-Encoding` and its q values,
streamed bodies encoded write by write.

Compression over TLS can give away a secret (BREACH): when a response carries a secret (a CSRF token,
a session identifier) and also reflects what the request sent (a query parameter, a form field),
an attacker who can make the victim's browser send many requests learns the secret, byte by byte,
from the compressed lengths. Leave such routes out of `compress`, or have the application mask the
secret anew for each response, as nginx's and Caddy's docs advise
([threat model, L13](../security/threat-model.md#known-limitations)).

```fit
listener web :8080

route {
	middleware compress { encodings zstd br gzip; min_bytes 1024 }
	handler static { root public }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<compress>.params -->
<a id="cfg-routes-middleware-compress-params"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `brotli_level` | integer, 1 to 11 |  | brotli's quality, 1 to 11, in place of the one level gives it: 1 is fastest, 4 default, 6 better and 11 the smallest, many times slower. |
| `encodings` | list of string: `zstd`, `br`, `gzip` | `["zstd", "gzip", "br"]` | The encodings offered, the one the client prefers most chosen, and among those it prefers as much, the first here; zstd, gzip and br when left out, gzip ahead of br since at the levels a response is compressed at as it goes, brotli is slower than gzip for about the same size. |
| `gzip_level` | integer, 1 to 9 |  | gzip's level, 1 to 9, in place of the one level gives it: 1 is fastest, 5 default, 7 better and 9 the smallest. |
| `level` | string: `fastest`, `default`, `better` | `"fastest"` | How hard the compressors work: fastest (gzip 1, brotli 1, zstd 1), default (gzip 5, brotli 4, zstd 3) or better (gzip 7, brotli 6, zstd 6); fastest when left out. gzip_level, brotli_level and zstd_level set one encoding's level by number instead. |
| `max_in_flight` | integer, 1 or more | `1024` | How many responses are compressed at once; past it, responses go uncompressed, so that the compressors' memory, a few hundred kilobytes each, is bounded. 1024 when left out. |
| `min_bytes` | integer, 0 or more | `1400` | The shortest declared body compressed; 1400 when left out, below which a response goes in one packet either way. A body that declares no length, as one that streams, is compressed. |
| `min_length` | integer, 0 or more | `1400` | Deprecated. Renamed min_bytes; the old name goes in config version 2. |
| `zstd_level` | integer, 1 to 22 |  | zstd's level, 1 to 22, in place of the one level gives it: 1 is fastest, 3 default, 6 better and 10 the smallest. The encoder has four speeds, which the levels fall into: 1-2, 3-5, 6-9 and 10-22. |
<!-- END GENERATED -->

### `concurrency_limit`

At most `max` of a route's requests under way at once; one more is answered at once with `status`.

```fit
listener web :8080

upstream app 127.0.0.1:9000

route {
	middleware concurrency_limit { max 100 }
	handler proxy { upstream app }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<concurrency_limit>.params -->
<a id="cfg-routes-middleware-concurrency-limit-params"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `max` | integer, 1 or more | required | The most requests under way at once. |
| `status` | integer | `429` | The status of a request refused; 429 when left out, or 503 as nginx's limit_conn answers. |
<!-- END GENERATED -->

### `forward_auth`

Asks an auth service whether a request may pass, on akio's client, the request suspended
meanwhile: a 2xx lets it through, with the fields `copy_headers` names copied onto it, and any other
answer goes back to the client.

```fit
listener web :8080

upstream auth 127.0.0.1:9091
upstream app 127.0.0.1:9000

route {
	middleware forward_auth {
		upstream auth
		uri /verify
		copy_headers Remote-User Remote-Email
	}
	handler proxy { upstream app }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<forward_auth>.params -->
<a id="cfg-routes-middleware-forward-auth-params"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `upstream` | string | required | The name of the upstream the auth service is. |
| `copy_headers` | list of string |  | Fields copied from a 2xx answer onto the request, for the handler and the upstream a proxy sends it to. Any of these the client sent, also spelled with _ for -, are removed, so that it cannot pass them off as the auth service's. |
| `request_headers` | list of string |  | The request's fields sent to the auth service; all but the hop-by-hop ones and those of the body when left out. |
| `timeout` | duration | `"10s"` | How long to wait for the auth service's answer, after which the request is answered 504 Gateway Timeout; 10s when left out. A service that cannot be reached is answered 502 Bad Gateway. |
| `uri` | string | `"/"` | The path, and query, of the request sent to the auth service; / when left out. |
<!-- END GENERATED -->

### `headers`

Sets, adds and deletes header fields of the request on its way in and of the response on its way
out.

```fit
listener web :8080

route {
	middleware headers {
		request { set { X-Forwarded-Proto https } }
		response {
			set { Strict-Transport-Security "max-age=31536000" }
			delete Server
		}
	}
	handler respond { body hello }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<headers>.params -->
<a id="cfg-routes-middleware-headers-params"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `request` | [object](#cfg-routes-middleware-headers-params-request) |  | Changes to the request's fields, made before the handler, and the upstream a proxy sends the request to, sees them. |
| `response` | [object](#cfg-routes-middleware-headers-params-response) |  | Changes to the response's fields, made just before it is sent, whichever handler wrote it. |
<!-- END GENERATED -->

#### `request` and `response` in `headers`

<!-- BEGIN GENERATED: routes[].middleware[]<headers>.params.request -->
<a id="cfg-routes-middleware-headers-params-request"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `add` | map of list of string |  | Values to add to fields, after any already there. |
| `delete` | list of string |  | Fields to delete: names, or a prefix ending in "*", as in "X-Debug-*". |
| `set` | map of string |  | Fields to set, each replacing any of the same name. |
<!-- END GENERATED -->

`response` has the same fields:

<!-- BEGIN GENERATED: routes[].middleware[]<headers>.params.response -->
<a id="cfg-routes-middleware-headers-params-response"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `add` | map of list of string |  | Values to add to fields, after any already there. |
| `delete` | list of string |  | Fields to delete: names, or a prefix ending in "*", as in "X-Debug-*". |
| `set` | map of string |  | Fields to set, each replacing any of the same name. |
<!-- END GENERATED -->

### `jwt`

Verifies JSON Web Tokens with akio-crypto, keys from PEM files, a secret or a JWK set fetched by
akio's client and refreshed, and passes claims on as header fields.

```fit
listener web :8080

route {
	middleware jwt {
		jwks_url https://auth.example.com/.well-known/jwks.json
		issuer https://auth.example.com/
		audience api
		claims_headers { sub X-User }
	}
	handler respond { body hello }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<jwt>.params -->
<a id="cfg-routes-middleware-jwt-params"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `algorithms` | list of string: `RS256`, `RS384`, `RS512`, `PS256`, `PS384`, `PS512`, `ES256`, `ES384`, `ES512`, `EdDSA`, `HS256`, `HS384`, `HS512` |  | The algorithms tokens may be signed with; any that fits the keys when left out. none never is. |
| `audience` | list of string |  | Audiences of which a token's aud must name one; any when left out. |
| `claims_headers` | map of string |  | Request fields to set to claims, as {"X-User": "sub"}: a string as it is, anything else as JSON. Any of these fields the client sent, also spelled with _ for -, are removed, with or without the claim. |
| `hmac_secret_file` | string |  | A file whose contents are the secret of tokens signed with HS256, HS384 or HS512; at least 32 bytes. |
| `issuer` | string |  | The iss a token must have; any when left out. |
| `jwks_refresh` | duration | `"1h"` | Deprecated. Renamed jwks_refresh_interval; the old name goes in config version 2. |
| `jwks_refresh_interval` | duration | `"1h"` | How often the JWK set is fetched again; 1h when left out. A token naming a kid the set does not have has it fetched sooner, at most every 10s. |
| `jwks_url` | string |  | The URL of a JWK set (RFC 7517) whose signing keys tokens are verified with. |
| `leeway` | duration |  | How far exp and nbf may be off, for clocks that differ; none when left out. |
| `public_key_files` | list of string |  | PEM files of public keys or certificates (RSA, ECDSA or Ed25519) tokens are verified with. |
| `token_from` | string | `"header"` | Where the token is: header, after Bearer in the Authorization field; header:Name, the whole of that field; cookie:name; or query:name. header when left out. |
<!-- END GENERATED -->

### `rate_limit`

Limits each client, or each value of a key, to `rate` requests a second with bursts of `burst`; the
keys kept are bounded by `max_keys`.

```fit
listener web :8080

route {
	middleware rate_limit { rate 10; burst 20 }
	handler respond { body hello }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<rate_limit>.params -->
<a id="cfg-routes-middleware-rate-limit-params"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `rate` | number | required | How many requests a second each key may make, on average; at least 0.001. |
| `burst` | integer, 0 or more |  | How many requests a key may make at once, after it has been idle: the bucket's size. The rate, rounded up, when left out. |
| `ipv6_prefix` | integer, 0 to 128 | `64` | How many leading bits of an IPv6 client address name the client with key ip: a host is given a /64 to pick its addresses from, so counting each address apart would let it make a fresh key with each request. 64 when left out or 0; 128 counts each address apart. |
| `key` | string | `"ip"` | What requests are counted by: ip, the client's address; route, every request of the route together; or header:Name, a header's value, with a request without it counted by ip. ip when left out. |
| `max_keys` | integer, 0 or more | `100000` | The most keys kept at once, which bounds the memory the buckets take: under 100 bytes each, and up to 128 more for a header; 100000 when left out or 0. |
| `status` | integer | `429` | The status of a request refused; 429 when left out. |
| `trusted_proxies` | list of string |  | The addresses ("10.0.0.1") and networks ("10.0.0.0/8") of proxies in front of akit: for a request that comes from one, the client's address is the last in X-Forwarded-For that is not one of them. None when left out, and the client is the connection's peer. |
<!-- END GENERATED -->

### `redirect`

Answers with a redirect, to a fixed URL or to one made from a regular expression's groups.

```fit
route {
	match { host www.example.com }
	middleware redirect { to https://example.com{uri}; status 301 }
	handler respond
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<redirect>.params -->
<a id="cfg-routes-middleware-redirect-params"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `to` | string | required | Where to: a URL or a path, in which {scheme}, {host} (with its port, as the client sent it), {path}, {query} (without the '?') and {uri} (the path and query) stand for the request's, and, with regexp, $1 or ${name} for its groups, as in "https://{host}{uri}" or "/new/$1". |
| `regexp` | string |  | A regular expression in Go's syntax that the path, as escaped ("/a%20b", "/a%2Fb"), must match for the request to be redirected; the rest go on to the handler. Every request is redirected when left out. A URL made from the request that would leave the site, which to does not, is answered 400: one that starts with "//" or "/\", and, when to starts with a group ($1), one with a scheme. |
| `status` | integer | `302` | The status: 301 and 308 are permanent, 302, 303 and 307 temporary; 307 and 308 keep the method and body. 302 when left out. |
<!-- END GENERATED -->

### `rewrite`

Rewrites the path before the handler sees it: prefixes stripped or added, or a regular expression
replaced.

```fit
listener web :8080

upstream app 127.0.0.1:9000

route {
	match { path /api/* }
	middleware rewrite { strip_prefix /api }
	handler proxy { upstream app }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<rewrite>.params -->
<a id="cfg-routes-middleware-rewrite-params"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `add_prefix` | string |  | A prefix put before the path, as in "/v2". |
| `regexp` | string |  | A regular expression in Go's syntax; each match in the path is replaced with replacement. |
| `replacement` | string |  | What replaces each match of regexp, in which $1 or ${name} stand for the regexp's groups. |
| `strip_prefix` | list of string |  | Prefixes to take off the path: the first that the path starts with, at a segment boundary, is removed, and what is left starts with '/'. "/api" makes "/api/users" "/users" and "/api" "/", and leaves "/apix" alone. |
<!-- END GENERATED -->

### `wasm`

Runs a WebAssembly plugin of fit's ABI, or of proxy-wasm's, unchanged, on akio-wasm's pool of
instances, bounded in time and memory, never on an I/O thread. The [WebAssembly
guide](../guides/wasm.md) has more.

```fit
listener web :8080

route {
	middleware wasm {
		path plugins/headers.wasm
		instances 4
		timeout 50ms
	}
	handler respond { body hello }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<wasm>.params -->
<a id="cfg-routes-middleware-wasm-params"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `path` | string | required | The plugin: a WebAssembly module that exports akit_on_request, as a WASI reactor; fit's package plugin/wasm/guest, its names renamed, builds one from Go with GOOS=wasip1 GOARCH=wasm go build -buildmode=c-shared. **akit:** The plugin runs on akio-wasm's instance pool, never on an I/O thread. akit's ABI is fit's with its names renamed (decision 037): a plugin of fit's is built again with fit's guest package renamed (as tools/plugin-guest.sh does); Proxy-Wasm filters run unchanged. |
| `config` | string |  | A string the plugin reads, for settings of its own. |
| `instances` | integer, 0 or more |  | How many instances of the plugin run calls at once, each with its own memory: the CPUs akit has, at most 16, when left out or 0. |
| `max_body_bytes` | integer, 0 or more | `1048576` | For a Proxy-Wasm plugin given the response's body, how much of it the plugin may hold, pausing it to see more, or make it; past it the response fails, or goes on without the plugin when on_error is skip. 1 MiB when left out or 0. |
| `max_memory` | integer, 0 or more | `134217728` | Deprecated. Renamed max_memory_bytes; the old name goes in config version 2. |
| `max_memory_bytes` | integer, 0 or more | `134217728` | The most memory an instance may have; one that asks for more fails its call. 128 MiB when left out or 0. |
| `max_streams` | integer, 0 or more | `1024` | For a Proxy-Wasm plugin that follows responses, how many an instance follows at once, each keeping its context; past it a request is answered 503. 1024 when left out or 0. |
| `on_error` | string: `fail`, `skip` | `"fail"` | What a call that fails, traps or runs past its timeout leads to: fail answers 500, or fails a response under way, closing its HTTP/1 connection or resetting its stream; skip lets the request or its response go on as if the plugin were not there. fail when left out. |
| `properties` | list of [object](#cfg-routes-middleware-wasm-params-properties) |  | For a Proxy-Wasm plugin, properties it reads by their path (proxy_get_property), beside the request's own: request.path, request.url_path, request.query, request.host, request.method, request.scheme, request.protocol, source.address and plugin_name. |
| `queue` | integer, 0 or more | `1024` | How many requests may wait for an instance; past it a request is answered 503. 1024 when left out or 0. |
| `response` | string: `all`, `headers`, `none` | `"all"` | For a Proxy-Wasm plugin that exports response callbacks, which it is given: all, the header, the body and the trailer; headers, the header alone, the body going by untouched; none, none of them, the plugin forgetting each request once it has gone on. all when left out. |
| `timeout` | duration | `"100ms"` | How long a call may run; past it the call is stopped and its instance replaced, and the request goes as a failure does (on_error). 100ms when left out. |
| `vm_config` | string |  | For a Proxy-Wasm plugin, its VM configuration, which it reads as it starts; config is its plugin configuration. |
| `wait_timeout` | duration | `"1s"` | How long a request may wait for an instance; past it it is answered 503. 1s when left out. |
<!-- END GENERATED -->

#### `properties[]` in `wasm`

<!-- BEGIN GENERATED: routes[].middleware[]<wasm>.params.properties[] -->
<a id="cfg-routes-middleware-wasm-params-properties"></a>

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `path` | list of string | required | The property's path, by segment, as ["route_metadata", "filter_metadata", "envoy.filters.http.wasm", "auth"]. |
| `value` | string |  | Its value. |
<!-- END GENERATED -->

## akit's extensions

A few fields are akit's own, for what akio can do and fit's engine cannot. Each has a decision note,
which says why, and is to be proposed to fit; a config that sets one is akit's alone until fit takes
it, since fit refuses it as an unknown field.

<!-- BEGIN GENERATED: extensions -->
| Field | Type | Since | Decision |
| --- | --- | --- | --- |
| `listeners[].sockmap` | boolean | I51b | [010](../decisions/010-sockmap-relay.md): a `tcp` listener's connections spliced in the kernel by akio's eBPF sockmap relay where it loads |
<!-- END GENERATED -->

## What akit does not read yet

These parts of fit's config are refused whole, by name, as not supported yet: `log`, `pprof`,
`llm`, `tunnel`, `webrtc`, `media_auth`, `streaming`, `media_library`, `sync` and `bittorrent`, with fit's handlers for media, BitTorrent and sync. Within the parts akit reads,
the fields marked **Not supported yet** in the tables above are refused the same way. The
[roadmap](../roadmap.md) says when each comes, and the [compatibility page](../compatibility.md)
lists every field.
