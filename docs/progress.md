# akit development progress

[English](progress.md) | [简体中文](progress.zh-CN.md)

What has actually been built, iteration by iteration, and where the next session picks up. The
[development plan](dev-plan.md) says what each iteration is meant to do; this file says what was
done and where the work differs from the plan.

## Where things stand

| | |
| --- | --- |
| Last updated | 2026-11-24 |
| Milestone | M0, foundations, done (I01–I08); M1, the proxy core: I09–I12, I14, I19, I22, I23 and I24 done, I15 doing (waits for akio's A11), I16 doing (waits for akio's A16–A18). |
| akio | `50a82315` on akio's `main` (`1.0.0-rc.1`), pinned by `rev` in `Cargo.toml` |
| fit | the specification; read at `7daee573` for this plan |
| What works | `akit run` and `akit validate` with fit's flags and exit codes, configs in JSON, `.conf` and YAML merged from files and directories; SIGHUP reloads the config (a broken one keeps the running one; listeners kept, added and removed, those it started with only once akio's A1 is pinned); a graceful shutdown on SIGINT and SIGTERM; HTTP/1.1, h2c, TLS (SNI, ALPN, client authentication) and HTTP/3 listeners, `Alt-Svc` on the TCP side; router v1 on canonical paths; the `respond`, `static` (conditional and range requests, files opened on akio's pool) and `proxy` handlers (fit's forwarding fields, trusted proxies; the client's `Host` once A11 is pinned); upstream pools with round robin, weighted round robin and least connections; `limits.max_body_bytes` (413 past it); the compatibility gate: 80 fixtures, 8 valid, 71 refused only as not supported yet, 1 skipped, none failed |
| Next | Move the akio pin to akio's main with A1–A8 and A11, then I15 done (its `Host` tests) and I22 on A1; then I13, I17, I20, I21; I16's streaming once akio has A16–A18 |
| akio requests | A1–A8 merged into akio's main (2026-11-24); A11 on akio branch `claude/i15-proxy-699f18`; A9, A10, A12–A18 requested ([akio API requests](akio-api-requests.md)) |

## Log

### 2026-11-24: I16, reverse proxy II: streamed bodies (doing: waits for akio A16–A18)

- Status `doing`, not `done`: the streaming itself cannot be built on akio without working around
  it (rule 21). Read against akio `50a82315` (the pin) and akio's `main` (`13ed6327`), which agree
  here: akio-http's HTTP/1 client takes a request body only whole (`ClientRequest::body`, one
  `send_owned` of head and body) and buffers each response whole (up to
  `ClientOptions::max_body_bytes`, 64 MiB) before calling back once; and a server request taken
  as a stream (`BodyMode::Stream`, `on_body`) cannot hold its client's reads, nor can a streamed
  response (`BodyStream`) say when its client drained (only `queued_bytes`, to poll); h2 even
  replenishes a stream's window as DATA arrives. akio-core has `hold_reads` and `on_drain` per
  connection, but akio-http exposes them only on upgraded connections (`raw`). Written up as akio
  API requests A16 (a client request body sent as it is produced, with `queued_bytes` and
  `on_drain`), A17 (a client response delivered as it arrives, its reads held and resumed) and A18
  (a server request that holds its client's reads, and `BodyStream::on_drain`), with how the proxy
  links them ([akio API requests](akio-api-requests.md)). I16 is `done` once they are in akio, the
  pin moves, and the proxy streams with the tests the plan names (a slow client and a slow upstream,
  large bodies both ways, bounded memory, epoll and io_uring).
- Built now, what does not wait on them: fit's body limit. `akit_config::Limits` reads `limits`:
  `max_body_bytes` (fit's int64, at least 0; 16 MiB when left out or 0), and refuses its other
  fields (`max_pending_bytes`, `max_header_bytes`, `max_conns`, `max_write_queue_bytes`,
  `write_stall_timeout`) as not supported yet, one by one. `limits` may be given by one file of a
  config only, refused in the second with fit's message (`adapter.Set.conflict`: "also given in
  …"). Each `Listener` carries the value (`Listener::max_body_bytes`), as fit's
  `runtime.Listener` does, so a reload that changes it changes every listener, as fit's
  `binding.same` compares it (a listener akit started with is then refused until A1).
- `akit-listener` sets it as akio's limit on HTTP/1.1 (`HttpConfig::max_body_bytes`), HTTP/2
  (`H2Config::max_body_bytes`) and HTTP/3 (`H3Config::max_body_bytes`): a body that declares a
  length past it is answered 413 before it is read, a chunked one once it passes it (on HTTP/1.1
  akio closes the connection after the 413, so a client still sending may see it reset, as fit's
  test allows). Before, akit took akio's defaults: 1 MiB on HTTP/1.1 and HTTP/2, 8 MiB on HTTP/3,
  so bodies fit takes (up to 16 MiB) were refused.
- The proxy: request bodies go to the upstream whole, as I15 sent them, within that limit (a body
  past it never reaches the handler); responses come back whole, within akio's client limit
  (64 MiB; past it the request fails and the client gets 502, where fit streams any size). The
  proxy's docs say so. The listener still answers `BodyMode::Buffered` for every route; it routes
  `body_mode` once the proxy can stream (A16–A18).
- Tests: `akit-config/tests/limits.rs` (3): the default, a value carried by every listener, 0, a
  negative or a string refused as wrong, the other fields not supported yet, a field fit lacks
  wrong, one file only (and the value from that file applying to another file's listeners).
  `akit-listener/tests/listeners.rs`: `body_limit`, fit's `TestBodyLimit` ported (1000 and 90000
  bytes answered 200, 150000 answered 413 against a limit of 100000; framed by length and chunked
  over HTTP/1.1, and over h2c), and `body_limit_default` (2 MiB taken over HTTP/1.1 and h2c,
  16 MiB + 1 answered 413). `akit-proxy/tests/proxy.rs`: `body_limit`, fit's
  `TestStreamedBodyLimit` ported (150000 bytes reach the upstream whole, 300000 answered 413
  against a limit of 200000, by length and chunked over HTTP/1.1 and over h2c; none past the limit
  reaches the upstream). The compatibility gate's samples read `limits` and
  `limits.max_body_bytes`.
- Checked: `make check` on macOS; `cargo test -p akit-config -p akit-listener -p akit-proxy` on
  Linux (gpu51) with epoll and with `AKIO_TEST_BACKEND=uring`. `make compat-bless`: the report's
  `limits` lines became one per field refused (`examples/limits`, `examples/proxy`), and
  `examples/body-limit` no longer refuses `limits` (still `routes[].match.method` and
  `routes[].middleware`); 70 fields refused, was 66; outcomes otherwise unchanged.
- Differences from the plan: the streaming, the linked backpressure and the "Done when" tests wait
  for A16–A18 (I16 stays `doing`); body limits came first. HTTP/3's limit is set but tested only
  through the shared code (the h3 test client sends GETs alone). No `limits` field is printed at
  startup (fit's `-v` line of limits comes with the rest of them).
- For a benchmark task: nothing to measure yet beyond the proxy scenario (large bodies are
  buffered whole today); once A16–A18 land, memory per proxied request in flight with large bodies
  both ways and a slow peer, and throughput of large uploads and downloads against fit.

### 2026-11-24: I22, reload on SIGHUP

- `akit run` reloads on SIGHUP: it prints `akit: reloading <files>`, reads and compiles the same
  `-c` paths again, and, when they compile and their listeners can be served, swaps the new graph
  in (`GraphCell::swap`), prints the listeners it removed (`akit: listener "x" on ADDR removed`)
  and added (the startup line), then `akit: reloaded <files>`. A config that does not read or
  compile, or whose listeners cannot be served, leaves the running one: `akit: reload failed; the
  running config stays` on stderr, then the errors as fit prints them (as `akit run` prints them
  at startup). The new graph's upstream pools dial through the running engine.
- Retiring: the control thread calls `GraphCell::collect` after each swap and every 100 ms while
  `retiring()` is above 0, waiting on the signal channel with a timeout in between (decision 004);
  a replaced graph's modules (its upstream pools) are cleaned up there once no request uses it.
- Listeners (`akit_listener::Listeners`, `crates/akit-listener/src/reload.rs`), as fit's
  `listener.Server.Reload` moves them: a listener the new graph has with the same name, address,
  protocols and `tls` is kept, its socket open, its requests going to the new graph's routes from
  the swap on (a `Dispatcher` reads the current graph per request, by listener name). The rest:
  - Added: the stopgap the plan names for A1. akit binds a socket of its own (std's
    `TcpListener`) before the swap, so an address in use or a listener akit cannot serve fails the
    reload with nothing changed (the sockets bound for it closed again); one accept thread per
    listener (a control plane thread) waits for the swap, then hands each connection to the
    engine with `Engine::adopt_tcp`, with the same HTTP or TLS acceptor the engine's own
    listeners have. The connections run on the engine's dial threads (the last startup
    listener's).
  - Removed: such a socket is closed after the swap (its accept thread stopped, woken by a
    connection of its own); the connections it accepted stay open. A changed one at the same
    address keeps its socket and serves the new settings from the swap on.
  - A listener the engine was built with cannot be closed until A1 lands: a reload that removes
    or changes one is refused, `akit: reload failed; the running config stays: listener "web" on
    ADDR is removed or changed, which needs a restart: akit cannot close a listener it started
    with yet (akio API request A1)`. fit binds the new one first and drains the old.
- `-watch` and `-watch-interval` stay refused (I29's file provider); their usage now names only
  I29.
- Tests: `akit/tests/reload.rs`, the e2e test: `akit run` with four client threads sending
  keep-alive requests (20 per connection) to its listener throughout, SIGHUP changing the routes
  and adding a listener, a broken config, a config removing the startup listener (refused), the
  added listener removed, then 20 reloads in a row adding and removing it; the clients see each
  new body and no request fails, and SIGTERM still exits 0. `akit-listener/tests/reload.rs` (2):
  listeners kept, added, changed at the same address and removed, a connection of a removed
  listener left open and the address free again, the old graph retired by `collect`; refusals
  with nothing changed (the epoch, the listeners and the answers as before): a startup listener
  removed or changed, an address in use after another new listener was bound, h2c alone.
- Checked: clippy (`-D warnings`) and the tests of akit, akit-listener and akit-runtime on macOS;
  the same tests on Linux (the benchmark host), epoll and io_uring, all passing.
- Differences from the plan and from fit:
  - Startup listeners cannot be removed or changed by a reload (A1, above); only listeners added
    by reloads can.
  - A kept TLS listener keeps the certificates it was started with: its TLS configuration is fixed
    in the engine's acceptor (fit's kept listener takes the new graph's). Certificates that
    change without a config change are I30's (`-watch-certs`), and A2's resolver makes them
    swappable.
  - A removed listener's open connections are not drained on the old graph, as fit drains them:
    they stay open, and a request they send after the swap finds no listener of that name in the
    new graph and is answered 404.
  - The engine's connect timeout (the shortest upstream `connect_timeout`, the stopgap for A12)
    is fixed at startup: a reload that shortens it gets it only after a restart.
  - fit prints the config version after `reloaded`; akit has none until the Admin API (I26).
- Left for A1: once akit pins an akio with `Engine::add_listener`, `remove_listener` and
  `shutdown_listener`, `Listeners::reload` adds listeners to the engine instead of adopting their
  connections (their own threads and accel choices, as the startup ones), removes and drains the
  startup listeners as well (the refusal goes), and drains a removed listener's connections for
  the shutdown timeout as fit does; the accept threads go.

### 2026-11-24: I10, the HTTP/3 entry point

- `akit-config` reads fit's `h3` protocol (`Protocol::H3`) with fit's check, `h3 needs tls`; a
  listener may list it beside `http/1.1` and `h2` or alone. `tcp`, `udp` and `socks` are still
  refused as not supported yet.
- `akit-tls`: `server_configs` reads a listener's files once and builds two akio-tls
  configurations from them, `tcp` (as I09's `server_config`, which now returns it) and, for a
  listener serving `h3`, `h3`: the same certificates picked by SNI the same way, the same client
  authentication, with TLS 1.3 alone and ALPN `h3` alone (fib's `http3.ConfigureTLS` puts `h3`
  first and keeps TCP's protocols after it, which no HTTP/3 client asks for). The graph keeps both (`Graph::tls`, `Graph::h3_tls`), so `akit validate` checks them as
  before.
- `akit-listener`: a listener serving `h3` adds a UDP listener on akio-h3 (`akio_h3::serve_with`
  over `akio_quic::server_config_from` the graph's `h3` TLS) on the port its TCP side listens on,
  serving the same `Dispatcher` (routes by the `:authority` akio-h3 gives as `Host`, canonical
  paths, 400 and 404 as over TCP). Its TCP dispatcher adds `alt-svc: h3=":<port>"; ma=86400`,
  fib's `http3.AltSvc` value, to every response, 400 and 404 included
  (`Context::add_response_header`; the value is formatted once when the listener is set up). A
  listener at port 0 serving TCP and h3 first finds a port free for both (a TCP port whose UDP
  twin binds), as fit binds UDP on the port TCP got, and the engine is built again on another
  port, at most 8 tries (fit's `maxPortZeroBinds`), if one is taken in between.
  `akit_listener::addrs` says where each listener listens (`tcp`, `udp`); `akit run` prints
  TCP's address, or UDP's for an h3-only listener, as fit prints `Addr`.
- akio: `akio-h3` and `akio-quic` are new workspace dependencies at the pinned commit; nothing was
  missing, no new API request.
- The compatibility gate: `listeners[].protocols` takes `h3` (`supported.txt` notes it; no new
  field). `make compat-bless`: `examples/http3` no longer refuses the `h3` protocol (60 fields
  refused, from 61); it is still refused for `acme`, `tls.acme`, the proxy and middleware.
  `docs/compatibility.md`'s example lines use the `udp` protocol now.
- Tests: `akit-config` (h3 read beside TCP's protocols and alone, `h3 needs tls`, listed twice);
  `akit-tls` (the `h3` configuration's ALPN, none without h3, a problem's path the same);
  `akit-listener/tests/h3.rs` (6: the same route over h3 as over h2 and HTTP/1.1 with `Alt-Svc`
  naming the UDP port, which is TCP's, also on a 404, none on h3 itself, a route by host and the
  certificate by SNI over h3, canonical paths and 404 over h3, an unknown name refused; h3 alone
  listening on UDP alone; no `Alt-Svc` and no UDP without h3; a fixed port; client
  authentication required and if given over h3; datagrams that are not QUIC not disturbing
  others); `akit/tests/h3.rs` (end to end with `akit run`: the route over h3, `Alt-Svc` on h2 and
  HTTP/1.1). The I10 done-when is `the_same_route_over_h3_and_alt_svc_on_h2` and the `akit` test.
- Where it differs from the plan: it touches `akit-config`, `akit-tls`, `akit-runtime` and `akit`
  besides `akit-listener`. HTTP/3's limits are akio-h3's defaults (8 MiB bodies, 64 KiB field
  sections, QUIC's idle timeout and 10,000 connections), as the TCP listeners use akio-http's,
  until akit reads fit's listener limits (`max_header_bytes`, `max_body_bytes`; fit's HTTP/3
  takes the listener's, 1 MiB and 16 MiB by default). A response that sets its own `Alt-Svc` (a
  `respond` route's `headers`) keeps it, where fit's hook overwrites it: akio-http's
  `add_response_header` yields to the response's field, and a response filter would cost a boxed
  closure per request. The UDP listener runs on akio's per-peer UDP loop (epoll, kqueue on macOS)
  whatever the TCP backend. Port 0 is probed before akio binds rather than bound TCP first (akio
  binds every listener when the engine is built).
- Checked: `make check` on macOS; the touched crates (akit-config, akit-tls, akit-runtime,
  akit-listener, akit, akit-compat) on Linux (gpu51), epoll and io_uring, all passing.
- For a benchmark task: HTTP/3 against fit's (requests per second and latency over the fixed
  `respond` route, with h2load or fitbench's h3 client; the harness has no akit h3 scenario yet),
  new QUIC handshakes per second, and the cost of the `Alt-Svc` field on the HTTP/1.1 and h2 hot
  path (a same-run A/B of a listener with and without `h3`).

### 2026-11-24: I15, reverse proxy I: requests and headers (doing: waits for akio A11)

- Status `doing`, not `done`: the proxy is built and fit's header tests are ported, but three of
  them (the `Host` the upstream sees, and no `User-Agent` made up) pass only against akio's A11,
  which is on an akio branch of its own, not merged: akio `claude/i15-proxy-699f18`, commit
  `552fe283` (`ClientRequest::headers`' own `Host` sent in place of the URL's; an empty
  `User-Agent` sends none). On the pinned akio (`50a82315`) akio's client drops the request's
  `Host` (sending the upstream's address) and sends the empty `User-Agent` as it is; those three
  tests are `#[ignore]`d, naming A11. I15 is `done` once A11 is on akio's `main`, the pin moves to
  it, and the three are no longer ignored.
- `akit_config::Proxy`: the `proxy` handler's params, with fit's checks and messages:
  `upstream`; `trusted_proxies`, addresses and networks parsed as fit's `parsePrefix` does
  (`akit_config::Network`: an address alone a network of one, an IPv4-mapped one taken as IPv4,
  host bits cleared, Go's prefix-length syntax), `"nope" is not an IP address or a network, ...`
  at `trusted_proxies[i]`; `host_header`, `"a b" is not a host`; `upstream or upstreams is
  needed` when both are left out. That `upstream` names one of the config's upstreams is checked
  when the handler is provisioned (`there is no upstream named "nope"` at `params.upstream`),
  after every file's upstreams are joined, as fit checks it. `upstreams`, `failover`, `mirror`,
  `retry`, `timeout`, `tunnel_idle_timeout` and `splice_uploads` are refused as not supported yet.
- `akit-proxy` (`MODULES`, `http.handlers.proxy`, in `akit`'s registry): per request, on the I/O
  thread, the upstream's request is built (method, target, fields, body), the context detached,
  and the request handed to the upstream's pool (I14); the pool's completion callback answers
  through the detached context. Nothing blocks (rules 1–3); a retired graph's pool drains the
  requests under way, so no graph hold is needed. Failures as fit's `statusOf` answers them:
  `503 Service Unavailable` for a full or closed pool, `504 Gateway Timeout` for a timeout,
  `502 Bad Gateway` otherwise; a field name that is not a token, 400.
- Fields (`headers.rs`), as fit's `outgoingFramed` and `responseBlock`: the request's fields go on
  in the order and case they came, `Host` first (the client's, or `host_header`); dropped are the
  hop-by-hop fields (`Connection` and the fields it names, `Keep-Alive`, `Proxy-Connection`,
  `Proxy-Authenticate`, `Proxy-Authorization`, `TE`, `Trailer`, `Transfer-Encoding`, `Upgrade`),
  `Content-Length` and `Expect` (akio frames the body), pseudo-fields; `X-Forwarded-For`,
  `X-Forwarded-Proto`, `X-Forwarded-Host` and `Forwarded` (RFC 7239) are the proxy's own, after
  the client's fields, unless the client's address is in `trusted_proxies`: then it is added to
  the `X-Forwarded-For` and `Forwarded` it sent, and its `X-Forwarded-Proto` and `-Host` are kept.
  A `User-Agent` the client did not send is not added. The response's fields come back in the
  upstream's order and case without the hop-by-hop ones, those its `Connection` names and
  `Content-Length` (akio frames the body afresh), but for a response to HEAD, which keeps the
  upstream's `Content-Length`.
- The target sent is the canonical path the route matched (akit-listener's `CANONICAL_PATH`, now
  defined in akit-runtime) with the query as it came, so that a path spelled to dodge a route
  reaches the upstream as the route saw it; an absolute-form target is sent in origin form.
  `X-Forwarded-Proto` is `https` for a request on a TLS listener: akit-listener's dispatcher sets
  the context local `akit_runtime::TLS` on such requests (akio's context does not say whether a
  request came over TLS).
- Tests: fit's header tests, ported (`akit-proxy/tests/proxy.rs`, over HTTP/1.1 and h2c to an
  HTTP/1.1 upstream on std sockets that reports each head as it arrived): `TestForwarding`,
  `TestHopByHopRequest`, `TestHopByHopResponse`, `TestForwardedUntrusted`,
  `TestForwardedTrusted`, `TestHostHeader` with `TestFramedHostHeader`, `TestBadGateway`,
  `TestManyRequests` (64 at once), `TestHead`, `TestConfigErrors`, `TestFramedRequestHeader`,
  `TestFramedResponseHeader`, `TestFramedResponseConnectionFields`, and the canonical path sent;
  unit tests (`headers.rs`): `TestFramedFieldNames`, `TestForwardedElement`,
  `TestRefusedHeaderName`, and the fields of each kind of request; `tests/tls.rs`: `https` behind
  a TLS listener; `akit-config/tests/proxy.rs`: the params, fit's messages, what is not supported
  yet, networks. Not ported: `TestFramedRequestReadAgain` and `TestFramedRequestHeaderMiddleware`
  (fit's map-built requests and middleware, which akit does not have: I40), `TestForwardedNotIP`
  (akit's peers are always IP addresses; `forwarded_element` covers `for=unknown`).
- The compatibility gate: 5 proxy fields supported (42 in all); 8 fixtures valid (the three
  `conf.d` templates now), 71 refused only as not supported yet (`make compat-bless`, its diff
  reviewed). One line of the report reads `examples/load-balancing: ... handler<fastcgi>.params.timeout`
  for a proxy's `timeout`: akit-config's error paths, like fit's, do not name the module, and the
  gate maps a handler param both fastcgi and proxy have to the first; the gate is right that the
  fixture is refused only as not supported yet.
- The bench config (`bench/configs/akit/fit.json`, `bench/fitbench/akit.go`): fit's `proxy` route
  (`/proxy`, `/proxy/*`, `/api/*`) to fitbench's backend, with fit's pool settings, and the
  `proxy` scenario; fit's `timeout 60s` and `max_fails 0` are not settings akit has yet (I17,
  I20). No benchmark was run (rule 30).
- Checked: `make check` on macOS on the pinned akio (the three A11 tests ignored); akit-proxy's
  tests, the ignored ones included, against akio's A11 branch through a `[patch]` given on the
  command line (never committed); on Linux (the benchmark host), epoll and io_uring: akio-http's
  tests on the A11 branch, and the tests of akit-config, akit-runtime, akit-listener,
  akit-handler, akit-proxy, akit and akit-compat against it, the ignored ones included.
- Differences from the plan:
  - Bodies travel whole (akio's buffered bodies, copied into the upstream's request): streaming
    them is I16. fit's 64KB threshold, splice uploads and trailers are I16 and I17.
  - The `User-Agent` rule needed A11 as well (an empty `User-Agent` sends none): added to A11
    rather than as a request of its own.
  - Known flake, not I15's: `akit-upstream`'s `max_conns_and_max_pending` sometimes sees
    `in_flight() == 1` under a loaded `make check`, since the pool calls a request's callback
    before it counts it done; the test should wait for the count (or the pool count first).
- What a benchmark task should measure: the `proxy` scenario (fitbench `-scenarios proxy`) over
  HTTP/1.1, h2c, HTTPS/1.1 and h2 against fit, nginx, Caddy and Traefik, once A11 is in the pin;
  per request the proxy builds a new header list and copies the body and the response's fields,
  and a TLS request sets a context local (an allocation): profile those against fit's
  `outgoingFramed`, which writes into a pooled buffer.

### 2026-11-24: I19, load balancing v1

- Config: `upstreams[].weights` (each from 1 to 1000, one per address) and
  `upstreams[].load_balancing` (`round_robin`, `weighted_round_robin`, `least_conn`) are read into
  `akit_config::Upstream` (`weights`, `load_balancing`, `weight(i)`), with fit's default (round
  robin, or weighted round robin when an address weighs other than 1) and fit's checks and
  messages: the bounds of each weight, as many weights as addresses, no weights with
  `round_robin`, and the policy one of fit's nine. fit's other six policies (`random`, `p2c`,
  the hashes) and `hash_key` are refused as not supported yet (I36).
- `akit_upstream::balance::Balancer`, behind `Pool::pick` (whose signature is unchanged; the pool
  adds `load_balancing()` and `weight(i)`): round robin; nginx's smooth weighted round robin, its
  round worked out ahead (weights divided by their greatest common divisor, at most 65536
  entries, as fit's `maxSchedule`) and taken one entry a pick; least connections, the address with
  the fewest requests under way for its weight (busy and waiting, the counts the pool keeps per
  address for `max_conns`), compared without dividing, ties broken from a starting point that
  moves on with each pick.
- State per I/O thread (rule 13): fit keeps one turn per upstream for every goroutine; akit gives
  each balancer one cache-line slot per core (the parallelism rounded up to a power of two), and
  each thread takes its turns from the slot it was given on its first pick. Each thread sees fit's
  sequences exactly and the shares are exact over whole rounds whichever threads share a slot;
  slots start at different places in the round so that every thread's first requests do not all
  go to the first address. What must be shared is read, not written: least connections reads the
  per-address counts. Weighted round robin with a round too long to work out ahead runs nginx's
  algorithm on each slot's own running weights, under the slot's own lock (no global lock).
  Picking does not allocate.
- Tests: `akit-upstream/tests/balance.rs` (9), fit's `balance_test.go` ported: round robin and
  smooth weighted round robin sequences (as rotations of fit's, since each slot starts elsewhere)
  and exact shares, the schedule from weights with a common divisor, weights too many to schedule
  (exact by weight), 8 threads at once (exact shares for every policy), least connections idle,
  busy and weighted, one address; the pool following the config, and least connections reading
  the requests held by two real upstreams (6 and 3 for weights 2 and 1).
  `akit-config/tests/upstreams.rs`: `load_balancing` (defaults, the three policies, fit's checks
  and messages, the policies not supported yet). The gate's samples for both fields.
- Checked: `make check` on macOS; the tests of akit-config, akit-upstream (epoll and io_uring)
  and akit-compat on Linux (the benchmark host), all passing. fit's messages and their line and
  column checked against `fit validate` on the same files. The gate (`make compat-bless`):
  `upstreams` 11 of 38 fields supported (39 in all); `examples/load-balancing` no longer refuses
  `weights` and `load_balancing`, and `examples/l4-udp` names the `ip_hash` policy it lacks.
- Differences from the plan and from fit:
  - Turns are per thread, not per upstream: over all threads together, round robin is fair over
    whole rounds per thread rather than strictly in turn, so two requests on different threads
    may go to the same address at once. fit's single-goroutine sequences hold per thread.
  - fit's passive and active health (passing over addresses that are down, `TestWeightedRoundRobinDown`)
    is not ported: that is I20. fit's allocation test (`TestPickAllocs`) is not ported either (a
    counting allocator needs `unsafe`); picking allocates nothing by construction.
  - Least connections counts a request from when the pool takes it (`send`), not from fit's
    `Pick`: a caller that picks and sends at once, as `Pool::request` does, sees the same.
- What a benchmark task should measure: `Pool::pick` per policy at 1 and at all cores (the
  per-thread slots against one shared counter), and a proxy run over several upstream addresses
  per policy against fit, checking the shares under load.

### 2026-11-24: I12, static files I

- The `static` handler (`akit-handler`, `static_files.rs`) on akio-http's `Files`. `Files` serves:
  `ETag` and `Last-Modified`, `If-Match`, `If-None-Match`, `If-Modified-Since`,
  `If-Unmodified-Since`, ranges with `If-Range` (several as `multipart/byteranges`), `HEAD` with the
  length, the `Content-Type` by extension, a directory's index file, files up to
  `max_file_bytes` kept in memory and larger ones sent from disk (`sendfile` from a kept handle).
  Only a file it knows within its trust window is answered on the I/O thread; every `stat`,
  `open` and read runs on akio's file pool (16 threads, 4096 queued, past that 503 with
  `Retry-After`), answered from there through a detached context (rules 1 and 4). On Linux, with
  a `check_interval` other than 0, it follows changes through inotify, as fit does; elsewhere it
  trusts what it learned for `check_interval`; 0 looks at the disk for every request.
- What akit adds before `Files`, as fit's handler: only `GET` and `HEAD` (405 with
  `Allow: GET, HEAD`); a path with a `..` segment, a NUL or a backslash, escaped or not, is 400
  (fit's `dotfile.Scan`, decoding as it scans, without allocating); with `dotfiles` `deny`, the
  default, a segment starting with a dot is 404 but `/.well-known/` at the root. Its short answers
  are fit's (`"404 Not Found\n"` and so on), built once.
- Config: `root` (required; that it is a directory is checked at provisioning, made absolute),
  `index` (one name; fit's checks: a file name, no dot with `dotfiles` `deny`), `max_file_bytes`
  (1 MiB), `dotfiles`, `disk_io` (`pool`; `auto`, which looks at the disk on the worker, is refused
  as not supported yet), `check_interval` (5s). Refused as not supported yet: `strip_prefix` (A13),
  `max_cache_bytes` (A15), `default_type` and `index` lists of other than one name (A14);
  `precompressed`, `compress`, `try_files`, `browse` and `browse_max_entries` (I13). The
  compatibility gate: 8 static fields supported (45 in all); fit's examples that use `static`
  are now refused only for those fields (`make compat-bless`).
- The bench config serves fit's `static` and `site` routes from `/srv/www`, mounted as for fit,
  and `static` is in akit's fitbench scenarios (`bench/configs/akit/fit.json`,
  `bench/fitbench/akit.go`, bench README). No benchmark was run (rule 30).
- Tests: `akit-handler/tests/static_files.rs` (11), fit's `static_test.go` ported through akit's
  listener: serving (types, index, 404, 405, HEAD, twice for the cache), an index file, traversal
  (12 attempts, escaped and not, 400 from the listener or the handler, dotfiles and
  `.well-known`, `dotfiles` `allow`), conditional and range requests from memory and from disk,
  a large file from disk, a changed and removed file with `check_interval` 0, the config's errors
  and what is not supported yet, fit's bench routes on one root; and: a disk that hangs (a FIFO
  no one writes to) holds a file pool thread while 16 requests on other connections are answered
  from memory, a 4 MiB file to a slow reader and to clients that leave midway, pipelined requests
  answered in order. Unit tests of the path scan (traversal, dotfiles, plain paths).
- Checked: `make check` on macOS; the tests of akit-handler, akit-config, akit-listener,
  akit-runtime and akit-compat on Linux (the benchmark host), epoll and io_uring, all passing.
- Differences from the plan and from fit, until akio's `Files` gains what A13–A15 ask:
  - A13: the handler looks up the path as sent, not the listener's canonical one, so a `..` that
    stays inside the root (`/x/../ok.txt`) is 400 where fit's whole stack serves `/ok.txt`;
    `strip_prefix` is refused.
  - A14: a directory without its slash is served, not redirected (301); a file with a trailing
    slash is served, not 404; a `.br`, `.zst` or `.gz` twin is served to a client that accepts it
    although `precompressed` is not set, and `Vary: Accept-Encoding` is always sent; types come
    from `Files`' table of 22 extensions, `application/octet-stream` for the rest, without
    sniffing; a file that may not be read is 404, not 403; a FIFO or device under the root is
    opened; a name with `:` is 404; `Files`' own 404 says `Not Found`. The ported tests check
    akit's present answers and name the request.
  - A15: the memory `Files` keeps is bounded per file (`max_file_bytes`) but not in total, and a
    request waiting on its disk pool has no timeout (the pool's queue is bounded, 503 past it).
  - `Files` merges ranges less than 80 bytes apart and answers several ranges of a file sent from
    disk with a multipart body, where fit sends that file whole; its `ETag` is the size and the
    modification second, fit's the size and the nanosecond. On macOS fit answers a path whose case
    differs from the names on disk 404; `Files` serves it.
  - The file pool is `Files`' own akio pool, not the engine's logical pool: `Files` keeps its
    disk work there.
- For a benchmark task: the `static` scenario (fitbench's 1 KiB `small.html`, from memory) over
  HTTP/1.1, h2c, HTTPS/1.1 and h2 against fit, nginx and Caddy; `download` and `site` (files from
  disk, `sendfile`, the file pool) once their routes and protocols are added to akit's scenarios;
  per request, the cost of the path scan and of `Files`' shard read lock on a hit.

### 2026-11-24: I14, upstream pools on the HTTP/1 client

- `upstreams` are read (`akit_config::Upstream`), joined across files as fit joins them: `name`,
  `addresses`, `protocol` (`http/1.1`; `h2c` and `h2` refused as not supported yet),
  `max_conns`, `max_pending`, `max_idle_conns`, `idle_timeout` and `connect_timeout`, with fit's
  defaults (10000, 50000, 32, 60s, 10s; 0 is the default for all but `max_idle_conns`, where it
  keeps none) and fit's checks and messages (a valid name unique among the upstreams, at least
  one address, each a host and a port from 1 to 65535 and not a URL, none listed twice). fit's
  durations are Go's (`akit_config::duration`), negative ones refused. fit's 15 other upstream
  fields are refused as not supported yet by name.
- `akit-upstream`: a `Pool` per upstream on akio-http's `Client`. Per address, at most
  `max_conns` requests under way, which bounds the connections (HTTP/1.1 carries one request at a
  time, and akio's client reuses an idle connection before it dials and puts one back idle before
  it calls back); the next ones wait in order, at most `max_pending`, and one more fails at once
  with `Error::Busy`. Taking a slot is a compare-and-swap while nobody waits; the address's queue
  lock is taken only at `max_conns`. A queued request is started from the completion that frees
  its slot, never recursively (a thread-local trampoline). `max_idle_conns` and `idle_timeout`
  are akio's per-host idle pool; `connect_timeout` is akio's Happy Eyeballs per-attempt timeout;
  each exchange, its wait for a connection included, is bounded by fit's `response_timeout`
  default (1h, the field itself is I17's). `pick` gives the addresses in turn (fit's
  `round_robin`, its default; I19 the rest). `Error::is_upstream_failure` says which failures
  count against an address (I20).
- Drain: `Pool::close` refuses new requests with `Error::Closed` and lets those sent before it
  finish, the queued ones included; the pool is closed (`is_closed`, `on_closed`) once the last
  is done. `compile` makes the pools before the modules, gives them to the modules through
  `Env::upstream` (what I15's proxy takes at provisioning), keeps them in the graph
  (`Graph::upstream`), and records their cleanup first so it runs last: a retired graph, or a
  config that fails to compile, closes its pools (decision 004).
- The engine: pools are made with the graph, before the engine (built from the graph's
  listeners), so they reach it through an `EngineSlot` that `akit run` fills once the engine is
  built; connections are dialed on the listeners' threads, on the same engine. `akit validate`
  leaves the slot empty: its pools fail every request with `Error::NoEngine` and never dial.
- The compatibility gate: 9 upstream fields supported (29 in all); `templates/json/fit.json` is
  now valid, `templates/json/conf.d` refused only for the `proxy` handler (`make compat-bless`).
- Tests: `akit-config/tests/upstreams.rs` (5: defaults, limits, fit's checks and messages, what
  is not supported yet, joining across files) and Go durations; `akit-upstream/tests/pool.rs`
  (8: `max_conns` and `max_pending` at and over the limit, `max_pending` 0 as fit's default, idle
  connections kept, none kept and closed after `idle_timeout`, a pool drained under load with 4
  under way and 16 queued, an idle pool closed at once, an upstream that refuses connections
  alone and beside one that answers, no engine and the slot filled later, bad targets and
  timeouts while queued); `akit-runtime/tests/upstreams.rs` (3: pools compiled and given to
  modules, a failed compile closes its pools, a retired graph drains its pools under load);
  `akit-listener/tests/upstreams.rs` (2: pools dial on the listeners' engine, `connect_timeout`
  against an address that never answers); the gate's field samples.
- Checked: `make check` on macOS; the tests of akit-config, akit-upstream, akit-runtime,
  akit-listener, akit and akit-compat on Linux (the benchmark host), epoll and io_uring, all
  passing. The connect timeout test needs an address that leaves connects unanswered (a full
  accept queue on Linux) and skips on macOS.
- Differences from the plan:
  - `max_conns` bounds requests under way per address rather than counting sockets: the same
    bound over HTTP/1.1 (the tests check the upstream never sees more connections), and akio's
    client keeps its connections to itself.
  - A request waiting for a connection has no timer of its own: its wait counts toward the
    exchange's timeout, checked when it gets a connection (fib's client bounds the wait the same
    way); a full queue is refused at once.
  - Stopgaps for what akio lacks: a closed pool's idle connections stay open until their
    `idle_timeout` or the engine's shutdown (A10, `Client::close_idle`); akio applies the client's
    connect timeout only to a name with several addresses, so `akit_listener::engine` sets the
    engine's `connect_timeout` to the shortest of the upstreams' (A12). I15 needs A11 (the
    request's own `Host`). Hand-over notes for I15 are in the session notes folder.
  - Not done here: a pool kept across graphs on reload (fit's `Acquire`), I22's.

### 2026-11-24: I11, router v1: host and path

- `akit_router::Router` looks routes up instead of scanning them, as fit's `router` package does
  (`router/router.go`, without its matchers, which are I39): maps of host patterns (exact, `*.` by
  the suffix from its dot, `**.` likewise, and the routes with no hosts), and under each host
  pattern maps of exact paths and of prefixes, with the prefixes' lengths longest first. A lookup
  tries the request's exact host, its `*.` suffix and up to four `**.` suffixes, the longest first,
  then the routes with no hosts; under each, its exact path and then each prefix length, stopping
  once no prefix left can win. The order is fit's: priority, then host, then path specificity,
  then the route written first. A lookup does not allocate: a host with capitals is lower-cased in
  a stack buffer (one longer than 255 bytes matches only routes with no hosts, as in fit).
- Not in fit: the maps hash with an FxHash-style hasher of akit's own (they are built from the
  config and only read, so a request's host cannot lengthen a probe beyond what the config put
  there); a map of up to 8 keys is scanned rather than hashed; a router whose routes have no hosts
  does not look at the host. Since there are no matchers yet, each host and path pattern keeps only
  its best route.
- `akit_router::canonicalize` puts a request's path in fit's canonical form (`listener/path.go`):
  dot-segments removed as RFC 3986 says, `%2E` counting as `.`, empty segments removed; a path
  whose `..` climbs above the root, or that, decoded, has an empty or dot-segment that came
  encoded (`%2F`, `%5C`, `\`), is refused. A path with nothing to decode or remove is only
  scanned. `akit_listener::Dispatcher` canonicalizes every request before routing it and answers
  a refused path `400 bad request path\n`, as fit does; routes match the percent-decoded path, as
  fit's match Go's `URL.Path`, so `Router::find` and `Graph::route` take the path as bytes
  (`impl AsRef<[u8]>`: a decoded path need not be UTF-8).
- Tests: fit's router table tests, ported (`akit-router` `src/tests.rs`: `TestHostOnly`,
  `TestHost`, `TestDeepWildcard`, `TestPath`, `TestNoRoutes`, the nine `TestOrder` tables,
  `TestPathTwice`, `TestHostOnlyUnclosed`), v0's tests kept, and new ones for long hosts, many
  prefix lengths, a non-UTF-8 path and the bound on `**.` suffixes; fit's `TestCanonicalize`
  table, ported, plus malformed escapes and non-UTF-8 escapes; `tests/no_alloc.rs`, fit's
  `TestMatchAllocs` and `TestCanonicalizeAllocs`: a binary of its own, without the test harness,
  counting with `stats_alloc` as its global allocator (a dev-dependency: akit has no `unsafe` to
  write one, rule 14), checks that seven lookups, hits and misses, upper case, ports, `**.` and
  IPv6, and canonicalizing three canonical paths make no allocation in 100 calls each, and that
  the count is real (a path with dots does allocate); `akit-listener` `tests/listeners.rs`: fit's
  `TestPathConfusionRoutes` with `respond` routes, over HTTP/1.1 and h2c (paths spelled to miss
  a protected route get it, or 400), and the canonical path a handler sees.
- Micro-benchmark `crates/akit-router/benches/lookup.rs` (`harness = false`): v0 (I05's scan,
  kept in `benches/v0.rs`) against v1, ns per lookup, on fit's `BenchmarkMatch` table (each route
  a host of its own with a prefix and an exact path, plus a `*.` catch-all) and on the one-route
  config the benchmark scenarios serve. On the benchmark host, under `quiet.sh`:

  | ns per lookup | v0 | v1 | v0/v1 |
  | --- | ---: | ---: | ---: |
  | 1 route, `/` | 12.5 | 7.7 | 1.6x |
  | 10 routes + catch-all, middle route | 69.0 | 36.7 | 1.9x |
  | 10 routes + catch-all, catch-all | 48.6 | 24.2 | 2.0x |
  | 10 routes + catch-all, no route | 33.2 | 16.3 | 2.0x |
  | 100 routes + catch-all, middle route | 383.3 | 37.2 | 10.3x |
  | 100 routes + catch-all, catch-all | 315.4 | 24.1 | 13.1x |
  | 100 routes + catch-all, no route | 227.7 | 16.1 | 14.1x |
  | 1000 routes + catch-all, middle route | 3523.8 | 30.9 | 114x |
  | 1000 routes + catch-all, catch-all | 2292.4 | 23.9 | 96x |
  | 1000 routes + catch-all, no route | 2207.1 | 15.8 | 140x |

  v1 costs about the same at 10 routes as at 1000; v0 grows with the routes. Before small maps
  were scanned rather than hashed, v1 was slower than v0 on the one-route config (15.7 against
  12.6 ns); the scan made it 7.7.
- A/B, same run (`bench/ab.sh 8a7a8b8 ebcfe02`, `ab-20261124-015157`: I11's base against I11, the
  fixed scenario, 3 rounds of 10 s): HTTP/1.1 825k against 824k requests a second (-0.1%), h2c
  5979k against 5927k (-0.9%), CPU per request within 3%, p99 the same: within noise, as expected for one
  route with no host, which is a few nanoseconds of a request's microseconds. The first attempt
  (`ab-20261124-010304`) failed because the benchmark host's disk was full; a second found that
  `bench/ab.sh` could build B with A's crates (git archive's commit-time mtimes and a shared
  target directory), fixed in `bench/lib.sh` by extracting with the current time.
- To measure, in the maintainer's benchmark tasks (rule 30: feature tasks run no benchmarks): the
  router lookup, v0 against v1 (`cargo bench -p akit-router --bench lookup`), and the fixed-scenario
  A/B of I11 against its base. The numbers above were measured before rule 30; treat them as
  provisional until those tasks repeat them.
- Checked: `make check` on macOS; `cargo test -p akit-router -p akit-runtime -p akit-listener` on
  Linux (epoll) and `AKIO_TEST_BACKEND=uring cargo test -p akit-listener`, on the benchmark host.
- Differs from the plan and from fit: no matchers (I39). A malformed escape (`%zz`) is answered
  `400 bad request path`; fit's Go server refuses it, before fit sees it, with its own 400. akio's
  request target cannot be rewritten, so the canonical path reaches handlers as the context local
  `akit_listener::CANONICAL_PATH`, set only when it differs from the path sent; I12 and I15 read it
  (or akio gains a way to rewrite the target). A literal `\` stays as sent in the canonical path,
  where Go writes `%5C`. A request in absolute form (`GET http://host/path`) is still routed by
  akio's `path()`, the whole URI, as before I11.

### 2026-11-24: I09, TLS entry points from files

- `akit-config` reads a listener's `tls` and the `h2` protocol, with fit's fields, defaults and
  errors: `tls.certificates[]` (`cert_file`, `key_file`, both required), `tls.client_auth`
  (`ca_files`, required, at least one; `mode`, `require` by default or `verify_if_given`) and
  `tls.max_version` (`"1.3"`, quoted or bare). A listener with `tls` and no `protocols` serves
  fit's `DefaultTLSProtocols`, `http/1.1` and `h2`. fit's checks: `h2` needs `tls`, `h2c` is
  refused on a listener with `tls`, `mode` is one of its two values. Refused as not supported yet:
  `tls.acme`, `tls.on_demand`, `tls.kernel_tls_offload`, `max_version` `1.2`, and a `tls` without
  certificates (fit obtains them with ACME for the routes' host names).
- `akit-tls` (`server_config`) loads a listener's `tls` into an akio-tls `ServerConfig`, as fit's
  `tlsmgr.LoadStatic` and `LoadClientAuth` do: PEM files of at most 1 MiB, each key checked
  against its certificate, each problem at `listeners[i].tls.certificates[j]` or
  `.client_auth.ca_files[j]` with the file's path. Certificates are picked by SNI by fit's rule:
  the first certificate whose subjectAltName names (DNS, IP) hold the name, exactly or by its
  first certificate's `*.` wildcard, else the first. ALPN from the protocols as fit's
  `tlsmgr.ServerConfig`: `h2, http/1.1`, `h2` or `http/1.1`. Client authentication with
  `client_auth_required` or `client_auth_optional`. Session tickets on, as Go's servers have them.
- Versions and cipher suites need no setting: akio-tls serves TLS 1.2 and 1.3, and for 1.2 only
  the six ECDHE AEAD suites fit allows (AES-GCM and ChaCha20-Poly1305).
- The graph (`akit_runtime::compile`) loads every TLS listener's files, as fit's graph does, so
  `akit validate` checks them too; `Graph::tls(i)` gives the configuration. `akit_listener::engine`
  wraps a TLS listener's HTTP acceptor in `akio::tls::Tls::server`; akio-http's server takes HTTP/2
  when ALPN chose h2. An `h2`-only listener refuses a client offering only `http/1.1` in the
  handshake and serves HTTP/1.1 to one offering no ALPN, as Go's server does.
- Bench: `bench/configs/akit/fit.json` serves HTTPS/1.1 on 8443 and h2 on 8445 with fitbench's
  certificate at `/certs`, as fit's; `bench/fitbench/akit.go` mounts it and holds idle TLS
  connections; `run.sh` and `ab.sh` run `https/1.1` and `h2` by default. Not benchmarked.
- akio: `akio-x509` (the names in a certificate) is a new workspace dependency at the pinned
  commit, `akio-crypto` a test one; akio-tls is used through `akio::tls`. New request A9: a public
  way to cap a TLS server at 1.2 ([akio API requests](akio-api-requests.md)); A2 notes that
  wildcard certificates past the first wait for its resolver.
- The compatibility gate marks the eight `listeners[].tls` fields akit reads (`supported.txt`, 28
  fields); its report is unchanged (the fixtures that use `tls` are `.conf` and YAML files).
- Tests: `akit-config` (3: reading, fit's problems with their positions, the fields not supported
  yet); `akit-tls` (3 unit: SNI picks, ALPN, IP names; 3 on files: loading, each problem's path, a
  wildcard past the first certificate); `akit-listener/tests/tls.rs` (5: HTTPS/1.1 and h2 by ALPN
  and with no ALPN, ALPN following the protocols, SNI picking the certificate, client
  authentication required and if given, a client cut mid-handshake and a client speaking plain
  HTTP not disturbing others); `akit/tests/https.rs` (2, end to end with `akit run`: a route over
  HTTPS with HTTP/1.1, HTTP/2 and by SNI; `validate` and `run` refusing a missing certificate
  file with its path). Certificates are made by the tests with `akio-x509`.
- Where it differs from the plan: `max_version` `1.2` waits for akio (A9); a wildcard name in a
  certificate other than the first is refused (akio-tls looks names up exactly, A2); when two
  certificates hold the same name akit serves the first, where fit serves the first the client
  supports (RSA or ECDSA); the handshake timeout is akio's 10 s default, not tested at its limit
  here (akio tests it).
- Checked: `make check` on macOS; the touched crates (akit-config, akit-tls, akit-runtime, akit-listener, akit, akit-compat) on Linux (gpu51), epoll and io_uring, all passing.

### 2026-11-24: I24, the YAML reader

- [Decision 005](decisions/005-yaml-reader.md): YAML is read by a reader of akit's own, not a
  crate. serde_yaml and its forks parse with unsafe-libyaml (`unsafe` throughout) and keep no
  positions; yaml-rust2 and saphyr are pure Rust but bring crates of their own and read YAML's
  corners differently from fit's libyaml-based library, so matching fit meant writing most of the
  work over them anyway.
- `akit-config`'s `yaml` module (`crates/akit-config/src/yaml.rs`, about 1,700 lines of code, no
  dependency, no `unsafe`) parses a YAML stream into a tree of YAML nodes, anchors shared, then
  turns it into the JSON reader's `Node` tree with byte offsets, as fit's `adapter/yaml` turns it
  into JSON: block and flow collections, plain, quoted, literal and folded scalars, explicit keys,
  anchors, aliases and merge keys (`<<`), the core schema's tags, `%YAML`, `---` and `...`;
  scalars resolved as fit's library resolves them and numbers written as fit's adapter writes
  them; a duplicate key, a non-scalar key, an unknown tag, `.inf`/`.nan`, a second document and
  more than 2^20 nodes or 64 MiB through aliases refused, as fit refuses them; an empty file an
  empty config. The model's errors point at the YAML's line and column.
- `akit_config::Format` (`Json`, `Yaml`) on `Source`, `Format::of(path)`, `from_yaml`;
  `from_files` reads `.yaml` and `.yml` files with it, so YAML and JSON files merge in one config.
  `akit run -c` and `akit validate -c` take YAML; the usage text says so.
- The compatibility gate's hook reads YAML fixtures through `from_files`. `make compat-bless`:
  80 fixtures, 0 valid, 4 refused only as not supported yet (fields refused: 2), 75 in a format
  not supported yet (.conf, I23), 1 skipped, 0 failed. The two YAML templates went from "format
  not supported yet" to exactly their JSON twins' outcomes (`upstreams`, and
  `routes[].handler<proxy>` for `conf.d`).
- Tests: `tests/yaml_config.rs` (the templates, file and `conf.d`, read into the same model with
  the same problems as their JSON twins; one config in both formats; positions; YAML and JSON
  files merged), and the module's own: fit's `adapter/yaml` test cases ported, scalars, folding,
  structure, positions, Go's float format, linear time on long lines, every prefix and one-byte
  edit of a document, every template's tree against its JSON twin's, with the commented-out
  fields written out too (6 of the 10 templates write out to a valid config in both formats), and
  fit's verdicts on 493 documents (`crates/akit-config/testdata/yaml-fit.txt`).
- `tools/yaml-diff/run.sh`: the differential check against fit (fit's adapter built with Go from
  a fit checkout, a corpus of hand-written cases, fit's YAML files and docs, the written-out
  templates and random edits). It found about 25 kinds of difference, fixed one by one; the last
  run, 150,000 cases, reads 2 differently (libyaml oddities, refused by akit, in decision 005).
- Fuzz target `config_yaml` (`fuzz/`), seeded with two small configs; a 10-minute run found
  nothing.
- Differences from the plan: the reader covers everything fit's library reads rather than a
  subset, refusing only `%TAG` and named tag handles (as not supported yet), nesting deeper than
  128 (fit: 256; the JSON reader's bound) and the 2 corners above; an explicit empty document (`---`) is `{}` to akit, `null` in fit's JSON, an empty config
  to both. The plan's "Touches" also gained `tests/compat` (the hook) and `tools/yaml-diff`.
- Checked: `make check` on macOS.

### 2026-11-24: I23, the .conf reader

- fit's own format, `.conf`, read by akit-config (`crates/akit-config/src/conf`):
  `akit_config::from_conf`, and `from_files` reads a file ending in `.conf`, given or found in a
  directory, merged with JSON files as fit merges them. The lexer and parser are fit's
  (`adapter/fitconf`): comments, double quotes with their escapes, backquotes, `;`, `{}`,
  `{$NAME}` and `{$NAME:default}`, bounded at 2^20 words and 64 levels of blocks. Then `def` and
  `use`, `site` (and `site *`) expanded into routes, the top-level lists in the singular with the
  name as the argument, the shorthands (a listener's address and protocols, an upstream's
  addresses, `tls <cert> <key>`, numbers of bytes in units), modules as their name and a block of
  params, and every mistake fit reports while converting, with its message, at its line and
  column.
- How: the reader converts a `.conf` file into the same tree the JSON reader parses, as fit
  converts it into JSON, each node keeping its offset in the `.conf` text, and the one decoder
  reads both. A reader of its own feeding the decoder directly was the other choice; converting
  keeps one decoder, so a field akit supports in a later iteration (I09, I14) is read from `.conf`
  with no change here, and the decoder's problems point into the `.conf` file. Like fit's, the
  conversion is typed by the config's fields: their types come from fit's field inventory,
  `tests/compat/fit-fields.txt` (fit's JSON Schema at the pinned commit), built into akit-config,
  so every field fit has converts as fit converts it, whether akit reads it yet or not, and is
  then read or refused by name. A directive no field takes is converted as written and refused
  as an unknown field, as in fit.
- Not supported yet, and refused as such at their place: `include` and `import`, which read
  other files from a `.conf` file (no fixture uses them; files are named with `-c` instead).
  fit's old, renamed field names are not read, in `.conf` as in JSON.
- The compatibility gate reads `.conf` files and `conf.d` directories through `from_files`. What
  validation opens for the examples is settled: nothing but the config's own files, since every
  field naming a file or a secret is refused as not supported yet ([compatibility](compatibility.md#what-validation-opens)).
  A value refused of a field akit reads is now reported with the value
  (`listeners[].protocols: the "h3" protocol`, `listeners: a config with no listeners, ...
  (auto_https)`) and is not counted as the field refused: the gate's check that no supported
  field is refused had flagged `listeners[].protocols` and `listeners`, which akit reads.
- The gate's counts, at fit `7daee573` and fit-examples `9659d7e4`: 80 fixtures; 2 valid
  (`hello`, `virtual-hosts`); 75 refused only as not supported yet (52 fields or values refused);
  2 in a format not supported yet (the YAML template and its `conf.d`, I24); 1 skipped (`https`);
  0 failed. Before: 0 valid, 2 refused, 77 not read. `templates/conf/*` refuse exactly what their
  JSON twins refuse.
- Tests: `conf/tests.rs` (fit's conversion, shorthand and error tables, ported: 21 configs and 60
  errors with their line and column; the bounds; positions of the decoder's problems in the
  `.conf` file; a `.conf` config and its JSON read into the same `Config`; every `.conf` template
  converts into the same tree as its JSON twin; every example converts), `conf/convert.rs`
  (numbers, bytes, the inventory's types), `files.rs` (`.conf` and JSON files merged from a
  directory, problems at their place), `akit/tests/cli.rs` (`akit validate` of a `.conf` file), the gate (value refusals). Fuzz target `config_conf` with
  a corpus of examples, fuzzed for a minute in CI beside `config_json`.
- Checked: `make check` on macOS; `config_conf` fuzzed for 60 s locally.

### 2026-11-24: I08, benchmark harness and baselines

- akit is measured with fit's harness, not a copy of it (`bench/`, [bench/README.md](../bench/README.md)):
  `bench/fitbench/akit.go`, copied into fit's `bench/cmd/fitbench`, adds akit to fitbench's
  servers (fit's server list is a slice, so no file of fit's changes); `bench/configs/akit/fit.json`
  serves fit's bench routes as far as akit serves them (the fixed response on 8080, HTTP/1.1, and
  8081, h2c). `bench/FIT_REV` pins fit's harness at the commit the compatibility fixtures use.
- `bench/run.sh [REF]` stages fit's tree on the benchmark host once, adds akit's two files, builds
  fitbench, benchreport, idleconn and fit there with Go, builds akit at REF there with cargo into
  an image of its own (`akit-bench:<id>`, on `ubuntu:24.04`), and starts fitbench with nginx,
  Caddy, Traefik, fit and akit under the host's `quiet.sh` (the lock alone); `-report ID` fetches
  the results and fit's benchreport tables into `bench/results/ID`. `bench/ab.sh A B` compares two
  akit builds in the same run, rounds alternating, with fit's `abreport.py`.
- akit's runs are the fixed scenario over HTTP/1.1 and h2c, with idle HTTP/1.1 connections; its
  other scenarios are recorded as skipped until the iterations that build them, each of which adds
  them to the overlay and the config. Where akit's config differs from fit's (port 8081 also
  takes HTTP/1.1, no backlog or connection limit settings yet, akio's I/O threads per listener)
  is in the bench README.
- The baseline, `bench/results/baseline-2026-11-24` (results, fit's benchreport tables, params and
  the akit commit): akit at `7c835ad`, fit `7daee573`, nginx 1.31.4, Caddy 2.10.2 and Traefik
  3.5.0 on the benchmark host (AMD Ryzen 7 7800X3D, 8 cores, 16 threads, Linux 7.0), fitbench's
  defaults (30s runs after 5s, 256 wrk connections, h2load 64 × 32), the fixed, static and proxy
  scenarios over HTTP/1.1 and h2c, and idle HTTP/1.1 connections; no errors.
- Where akit stands (rule 28), in what it serves so far: first in fixed over HTTP/1.1, 825,563
  requests a second at p99 0.32 ms and 2 MiB, nginx 99% of it, fit 92%, Caddy 41%; first in
  fixed over h2c, 6,029,715 a second at 3 MiB, fit 74%, nginx 10%, Caddy 3%; second in memory per
  idle HTTP/1.1 connection, 1,068 bytes, after nginx's 548 (fit 1,522). Static and proxy are not
  run for akit (I12, I15); Traefik serves only the proxy.
- Checked: `make check` on macOS; `bench/run.sh` end to end on the benchmark host (the first try
  failed when the host's disk was full, before measuring). `bench/ab.sh` has not finished a
  comparison: under rule 30 (added meanwhile), feature tasks run none.

### 2026-11-23: I06, CLI, signals and graceful shutdown

- `akit run`, `akit validate`, `akit version` and `akit help` with fit's flags, read as Go's
  `flag` package reads them (`-c x`, `-c=x`, `--c`, bool flags, `--`), and fit's exit codes: 0, 1
  when it did not work (config errors, a shutdown out of time), 2 when used wrongly (a bad or
  unknown flag, an argument, no command). `validate` prints `<files>: valid`; `version` prints
  akit's version, akio's commit and the platform, a line each, as fit's.
- `-c` repeated, files and directories: `akit_config::from_files` finds the files as fit does
  (a directory's `.json`, `.conf`, `.yaml` and `.yml` files, in subdirectories too, in lexical
  order, dotfiles left out, a file named twice read once, at most 10,000 files and 64 MiB) and
  merges them as fit does: `listeners` and `routes` joined in the files' order, `schema_version`
  given by each if they agree, the checks across elements on the whole. Each problem names its
  file, line and column, with its path in that file. A `.conf` or YAML file is refused as not
  supported yet (I23, I24); any other is JSON, as in fit. `fit.json` when no `-c` is given.
- Signals through akio-sys's `signal_pipe` (its `unsafe` stays in akio): SIGINT and SIGTERM print
  `akit: shutting down`, stop the listeners, close idle connections and wait up to
  `-shutdown-timeout` (30s by default, a Go duration) for requests in flight, then print
  `akit: stopped` and exit 0, or say the shutdown took longer and exit 1. A second signal exits at
  once. SIGHUP says reloading is not supported yet (I22) instead of killing the process.
  `akit_listener::shutdown` does the drain; `akit_listener::Options` carries the backend and the
  timeout.
- fit's other `run` flags (`-watch`, `-watch-interval`, `-watch-certs`, `-admin`, `-timeout`) are
  refused as not supported yet, exit 2.
- The compatibility gate now reads a directory of JSON files through the merge:
  `templates/json/conf.d` is refused only as not supported yet (`upstreams`, the `proxy`
  handler), no longer a format akit cannot read; 2 fixtures refused only as not supported yet, 77
  in a format not supported yet.
- Tests: `akit-config/tests/files.rs` (6: joining, problems in their file, schema versions,
  directories, formats, missing and empty); flags and Go durations; `akit-listener` (2 more: a
  request in flight finishes during the shutdown while an idle connection is closed at once; one
  that outlasts the timeout is cut and reported); `akit/tests/cli.rs` (4: validate across files
  and a directory, usage errors exit 2, `run` from two files stopped by SIGTERM with an idle
  keep-alive connection, `run` from a directory stopped by SIGINT).
- Checked: `make check` on macOS; the workspace's tests on Linux, epoll and io_uring, on the
  benchmark host (2026-11-24, with I04's and I05's): all pass.
- Differs from the plan: the request in flight is tested with the library's server and a handler
  that answers later, since `respond`, the only handler, answers at once; the binary's tests cover
  the signals, the output and the exit codes. Whether the shutdown ran out of time is told by how
  long akio's drain took (akio reports no count of the connections it closed). The second signal
  exits with 1, where Go's default handler would kill the process by the signal.

### 2026-11-23: I05, `akit run`: listeners and `respond`

- `akit run -c <file>` reads the config (JSON; `fit.json` when no `-c` is given, as fit's),
  compiles it, serves it, and prints each listener's address as fit does
  (`akit: listener "web" on 127.0.0.1:8080 (http/1.1, h2c)`). A config error prints fit's
  `file:line:col: path: message` lines and exits 1; a bad flag exits 2.
- `akit-handler`: the `respond` module (`http.handlers.respond`), its response built once at
  provisioning (status, headers, a `text/plain; charset=utf-8` content type when there is a body and
  none is set); akit-config already checks its params as fit's `Provision` does.
- `akit-listener`: `engine(cell, backend)` builds one akio TCP listener per config listener, each
  serving a `Dispatcher` that routes the request through the cell's current graph and calls the
  route's handler, or answers fit's `404 page not found`. `":8080"` listens on `[::]` dual-stack,
  as Go's `net.Listen`. `http/1.1` alone turns akio's HTTP/2 off; with `h2c`, prior knowledge and
  the `Upgrade: h2c` request are both served.
- `akit-router`: router v0, fit's host (exact, `*.`, `**.`, ignoring case, port and trailing dot)
  and path (exact, `*` prefix) rules and its order (priority, host, path, then the order written),
  by a scan of the routes; the graph builds one per listener at compile time, a route with no
  `listeners` serving on all.
- Fixed: akit-config's default listener protocols, found by I07b, are now fit's, `http/1.1`
  alone without TLS (`DEFAULT_PROTOCOLS`), with a test.
- Tests: router (6), respond (3), listener addresses; `akit-listener/tests/listeners.rs` (5):
  HTTP/1.1, h2c prior knowledge and upgrade, 404, host specificity, routes per listener, HTTP/2
  refused by default, h2c alone refused; `akit/tests/hello.rs` (3): fit's `hello` example, as
  JSON, answers `/hello` and its `not-found` route over HTTP/1.1 and h2c through the binary; a
  bad config exits 1 with its errors, bad flags exit 2.
- Checked: `make check` on macOS; the workspace's tests on Linux, epoll and io_uring, on the
  benchmark host (2026-11-24, in I06's run): all pass.
- Differs from the plan: the router v0 was not in I05's plan; without it the example's
  `not-found` route (priority -1, no match) would shadow `/hello`. I11 keeps its rules and makes
  lookups allocation-free, and now also canonicalizes paths as fit's listener does (not planned
  anywhere before). The example is written as JSON in the test: `.conf` is I23. A listener with
  `h2c` and no `http/1.1` is refused as not supported yet: fit serves HTTP/2 alone then, and akio's
  server always takes HTTP/1. Request bodies are buffered (akio's default, 1 MiB) until a handler
  streams them (I14).

### 2026-11-23: I04, the runtime graph and its swap

- `akit_runtime::compile(config, registry, env) -> Graph`: every route's handler loaded from the
  registry and provisioned before the graph exists. A config that fails leaves nothing behind: the
  modules provisioned before the failure are cleaned up, and errors carry the path of what failed
  (`routes[1].handler.params.status` for a provision error naming a field).
- `GraphCell` ([decision 004](decisions/004-graph-publication-and-retirement.md)): `swap` publishes
  a graph under a lock and bumps an epoch; `with` reads it through a per-thread cached `Arc`,
  refreshed once per thread per swap, so a request bumps no shared reference count; `hold` keeps a
  graph for a request or connection that outlives its handler's call; `collect` retires replaced
  graphs, cleaning their modules up once, when every thread is outside them or past them and no
  `Hold` keeps them.
- Tests (`tests/graph.rs`, 8): compile and its failures, swap and collect, holds, a request still
  inside the old graph, an idle thread that does not pin it, 8 threads swapping and reading 300
  times each with every graph retired exactly once, and swaps while 4 akio I/O threads serve.
- Benchmark `benches/graph_read.rs`: a read through `GraphCell::with` against an `Arc` clone and a
  mutex-and-clone, 20 million reads a thread on the benchmark host (16 cores): `GraphCell::with`
  4.06 ns a read on 1 thread and 8.78 ns on 16; an `Arc` clone and drop 2.77 ns and 324.51 ns; a
  mutex and a clone 6.57 ns and 1,167.62 ns. The shared reference count costs 37 times the cell's
  read once 16 threads contend for it.
- Checked: `make check` on macOS; the workspace's tests on Linux, epoll and io_uring, on the
  benchmark host (2026-11-24, in I06's run): all pass; the read benchmark there, under
  `quiet.sh` (above).
- Differs from the plan: the "atomic pointer" is a mutex taken once per thread per swap (no unsafe,
  rule 14; the epoch is the atomic). A graph is not released by its last request: akio has no hook
  on an idle I/O thread, whose cache would keep the old graph, so the control plane retires it
  with `collect` (decision 004). Handler params are `akit_config::Handler`, decoded and checked by
  akit-config. Routes stay in the config's order until I11.

### 2026-11-23: I07b, the compatibility gate wired to akit-config

- The gate is always on: the `compat-gate` feature is gone, akit-config is a dev-dependency of
  `akit-compat`, and `gate_through_akit_config` is an ordinary test that `make check` and CI's
  `cargo test --workspace` (Linux epoll and io_uring, macOS) run. One configuration, so the gate
  cannot be left out by a missing flag. `make compat-gate` runs it alone and prints the report.
- Formats akit has no reader for are an outcome of their own, "format not supported yet": the
  .conf files and `conf.d` directories until I23, the YAML template until I24. Every outcome is a
  line of the checked-in `tests/compat/gate-report.txt` (one line a field for a fixture refused
  as not supported yet), and the gate fails when a run differs from it either way;
  `make compat-bless` rewrites it from a passing run. JSON fixtures must read with only
  "not supported yet"; anything else, or "format not supported yet" beside other findings, fails.
- Outcome now, at fit `7daee573` and fit-examples `9659d7e4`: 80 fixtures; 0 valid; 1 refused
  only as not supported yet (`templates/json/fit.json`: `upstreams`; the rest of the template is
  commented out); 78 in a format not supported yet (74 examples and `templates/conf/fit.conf`,
  .conf; `templates/yaml/fit.yaml`, YAML; three `conf.d` directories); 1 skipped
  (`examples/https`); 0 failed.
- The hook names akit-config's problems by their path, written as the inventory writes it: a
  handler module akit lacks, refused at `routes[0].handler.module`, is `routes[].handler<proxy>`;
  a refused value of an array of values (`listeners[0].protocols[0]`, `h3`) is the array.
  `akit_compat::inventory_path` does the matching.
- `tests/compat/supported.txt` marks the 20 fields I02 reads: `schema_version`; `listeners` and
  their `name`, `address`, `protocols` (`http/1.1` and `h2c`; fit's other protocols are refused);
  `routes` and their `name`, `listeners`, `priority`, `match`, `match.host`, `match.path`,
  `handler`, `handler.module`, `handler.params`; `handler<respond>`, its `params` and their
  `status`, `headers`, `body`. `docs/compatibility.md` regenerated: 20 of 3,821 supported.
- New tests hold the marks to the code: `supported_fields_are_read` reads a config with each
  marked field and checks the value; `akit_config_lists_match_the_inventory` reads akit-config's
  lists in `model.rs` from the source and checks every entry is an inventory field, that they
  cover each object's fields in fit, that the refused handlers and protocols are fit's, and that
  what akit-config reads is exactly `supported.txt`. All of them pass: no typo in I02's lists.
- No change to akit-config: its lists are read from the source text, so no accessor was needed.
- Differs from the plan: what validation opens for the examples (files and WebAssembly plugins
  they name, the stand-in secrets) moves to I23, the first iteration that reads them; the gate
  does not need the secrets until akit expands variables.
- Found, not fixed (akit-config is I02's and I04's): akit-config's default listener protocols are
  `http/1.1` and `h2c` (`DEFAULT_PROTOCOLS`), while fit's are `http/1.1` alone without TLS
  (fit `config/config.go`, the `protocols` field's doc). Neither the gate nor the inventory can see
  a default; it needs a fix and a test in akit-config.

### 2026-11-23: I07a, compatibility gate I: fixtures, inventory, harness

- I07 is split: I07a, done here, needs no config model; I07b wires the gate to
  `akit_config::from_json_file` once I02 is merged ([dev plan](dev-plan.md#i07b-compatibility-gate-wired-to-validate)).
- Pinned fixtures in `tests/compat/fixtures/`: fit `7daee573` (`templates/`, 30 files: each of
  the three formats as one file and as a `conf.d` directory of 9 files; `schema/fit.schema.json`)
  and fit-examples `9659d7e4` (74 `*/fit.conf`), with both Apache-2.0 licences,
  `tests/compat/SOURCE.md` and `tests/compat/update-fixtures.sh <fit> <fit-examples>`, which
  moves the pin and regenerates everything below.
- The field inventory, `tests/compat/fit-fields.txt`: 3,821 field paths (path, section, type,
  flags), generated by `tools/compat-inventory.py` (Python 3, standard library, a dev tool) from
  fit's JSON Schema, which fit generates from its config types and modules as it does its config
  reference. Module params are listed per module (`routes[].handler<respond>.params.status`),
  121 sections in all.
- The harness, the test crate `akit-compat` (`tests/compat`, a workspace member, no
  dependency): enumerates the 80 fixtures (6 templates, 74 examples), runs a validator on each
  and judges it as decision 002 says; `examples/https` is skipped as fit-examples' own test skips
  it. Tested with stand-in validators. The gate itself (`gate_through_akit_config`) is behind the
  `compat-gate` feature, which adds the akit-config dependency, and is `#[ignore]`d until I07b;
  `make compat-gate` runs it with fit-examples' stand-in secrets.
- `docs/compatibility.md`: what the gate checks, how to read and refresh it, and the generated
  table of every field, all "not supported yet". `tests/compat/supported.txt` (empty) is where
  later iterations mark fields supported. `make check` and CI's lint job run
  `make compat-check`, which fails when the inventory or the tables are out of date.
- Checked: `make check` on macOS. No engine code, so nothing to run on Linux backends.
- Differs from the plan: I07 split into I07a and I07b; the inventory comes from fit's JSON Schema
  rather than from parsing `docs/reference/config.md` (same source in fit, machine-readable).
  The examples' non-config files (certificates, WebAssembly plugin sources) are not copied;
  I07b decides what validation opens.

### 2026-11-23: I03, core traits and the module registry

- `akit_runtime::traits`, on akio's callback model: `Handler` (an akio-http `Service`: inline on the
  I/O thread, responds or detaches), `HandlerService` (a handler as akio's `Service`), `Middleware`
  (`wrap(next)` once at compile time, as fit's) with `chain` and `AkioMiddleware` (akio's
  per-request middleware as akit's), `Matcher`, `Router`, `Service` (fit's: picks an `Upstream`),
  `Upstream` and `Listener` (a name and an akio `AnyAcceptor`); the last three provisional, as in fit.
- `akit_runtime::module`: `Info` (ID, doc, typed `Constructor`), `Registry::new` from crates'
  `static` lists (no global state; malformed, duplicate or misplaced IDs are a `RegistryError`),
  `lookup`, `namespace`, `handler` and `middleware` loading by config name with `Provision` (params
  of a generic type `P`, `Env`); `Provisioned::on_cleanup`; `Modules` running cleanups last
  provisioned first, once, on `release` or drop. Errors keep fit's wording: `there is no module
  "nope" in http.handlers (it has "echo", "respond")`; `ProvisionError` carries a message and maybe
  a field for I04 to place at the params' path.
- [Decision 003](decisions/003-module-registry-and-traits.md) records the design later iterations
  follow.
- Checked: `make check` on macOS (fmt, clippy `-D warnings`, tests, cargo-deny, docs);
  `RUSTDOCFLAGS=-D warnings cargo doc -p akit-runtime`. Tests (`crates/akit-runtime/tests/modules.rs`):
  a test module registers, provisions from params and is cleaned up on release and on drop; unknown
  names are errors naming them; a failure midway (a constructor error or an unknown name) cleans up
  what was provisioned, in reverse; registry mistakes; and a provisioned middleware + handler chain
  answers a real request on akio (epoll/kqueue; `AKIO_TEST_BACKEND=uring` on Linux). Not run on
  Linux in this session. No benchmark (control plane).
- Differs from the plan: akit-runtime now depends on `akio-http` (same akio `rev`) for `Service`,
  `Context` and `Middleware`. The params type is generic, not akit-config's, since I02 was being
  written in parallel: I04 sets it. `Env` is empty until I04. fit's optional interfaces (`Drainer`,
  `Inline`, `Unwrapped`, `NeedsAuthentication`, `Holder`) are left to the iterations that need them.

### 2026-11-23: I02, config model v0 and the strict JSON reader

- `akit-config`: a JSON reader of akit's own (decision 001) that keeps each value's position:
  RFC 8259 plus `//` and `/* */` comments and trailing commas after a value, as fit reads JSON;
  a key given twice, a control character, a lone surrogate, a leading zero, nesting past
  `MAX_DEPTH` (128) and documents past `MAX_CONFIG_BYTES` (64 MiB) are refused.
- The model v0, with fit's names, defaults and checks: `schema_version` (1); `listeners` (`name`,
  `address` as Go's `SplitHostPort` takes it, `protocols` `http/1.1` and `h2c`, fit's default
  both); `routes` (`name`, `listeners` that must exist, `priority`, `match.host` and `match.path`
  with fit's checks, `handler`); the `respond` handler (`status` 100–599, informational refused,
  `headers` as tokens without line breaks, no body on 204 and 304). fit's names: letters, digits,
  `_ . -`, unique per kind.
- Problems are collected, not stopped at, each as fit writes it: `file:line:col: path: message`,
  with fit's messages (`unknown field (did you mean …?)`, `missing required field`, `expected an
  array, got an object`, `another listener is already named …`).
- Decision 002 in code: every field and value fit reads that akit does not yet (the other top-level
  sections, `tls`, `middleware`, the other matchers, protocols and handler modules) is
  `not supported yet` by name; anything else is an unknown field or module.
  `Errors::only_not_supported_yet` tells a valid fit config akit cannot run yet from a wrong one.
- Checked: unit tests for the reader (values, comments, every refusal, depth, positions, every
  prefix and single-byte change of a config), the model's checks, and integration tests with
  fit-examples' `hello`, defaults, every problem in one document, file errors; fit's full
  1 MB JSON template (`templates/json/fit.json`, read from `../fit` when present) reads with
  nothing but "not supported yet". `make check` passes. Fuzzed with cargo-fuzz (`fuzz/`,
  target `config_json`): 3.4 million runs in 60 s, no crash; CI runs it for 60 s on every push.
- Differs from the plan: `upstreams`, `admin` and `log` are refused as "not supported yet" rather
  than modelled, since nothing would act on them until I14, I26 and I21 and decision 002 forbids
  reading a field and ignoring it; they come with those iterations.

### 2026-11-23: I01, workspace skeleton and CI

- The Cargo workspace: the `akit` binary and the fifteen library crates of the
  [layout](dev-plan.md#workspace-layout), empty but documented; edition 2024 and MSRV 1.85 as
  akio's; Apache-2.0. akio is pinned by `rev` (`50a82315`, akio's `main`) in
  `[workspace.dependencies]`, used by `akit-runtime`, whose `AKIO_REV` the binary prints.
- No `unsafe`: `unsafe_code = "forbid"` in the workspace lints and `#![forbid(unsafe_code)]` in every
  crate. Clippy lints for rule 15 (`unwrap_used` per library, `expect_used`, `panic`,
  `indexing_slicing`, `arithmetic_side_effects`), with tests allowed to unwrap (`clippy.toml`).
- `deny.toml`: Apache-2.0 and MIT only for now, unknown registries and git sources refused except
  akio's, and tokio, async-std and smol banned (rule 16).
- `Makefile`: `check` (fmt, clippy, tests, cargo-deny, docs), `test-changed`
  (`scripts/test-changed.sh`: changed crates and their dependents), `build`, `lint`.
  `scripts/check-docs.sh` checks that every document has its Chinese twin and that both link each
  other under the title.
- CI (`.github/workflows/ci.yml`) on pushes to `main` and pull requests: fmt, clippy and the docs
  check; tests on Linux (epoll, and `AKIO_TEST_BACKEND=uring`) and macOS; `cargo +1.85 check`;
  cargo-deny.
- The binary: `akit version` / `--version` prints `akit 0.0.1 (akio 50a823155a06)`; `help`; an
  unknown command exits 2. `run` and `validate` come with I05 and I06.
- Checked: `make check` on macOS; in Docker on Linux arm64, `cargo test --workspace` (plain and with
  `AKIO_TEST_BACKEND=uring`) and `cargo check` with Rust 1.85. The CI workflow itself first runs when
  this branch is pushed. The engine is not exercised yet, so the backends add nothing to test today.
- Differs from the plan: no `xtask` (the `Makefile` and two scripts are enough); `tests/e2e` and
  `bench` are created by the iterations that fill them (I05, I08).

### 2026-11-23: decisions

- The user accepted [decision 001](decisions/001-dependency-policy.md) (the data plane depends on
  akio and `std` only, no tokio) and [decision 002](decisions/002-config-compatibility.md) (akit
  reads fit's config); akit is Apache-2.0, as fit; A1–A8 are added to akio now.

### 2026-11-23: planning

- Wrote the roadmap, development plan, rules, architecture, akio API requests and decisions 001
  and 002, in English and Chinese.
- Not checked: the akio API requests were found by reading akio's public API, not by building
  against it; each is to be confirmed before akio is changed.
