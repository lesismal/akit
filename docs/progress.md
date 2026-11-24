# akit development progress

[English](progress.md) | [简体中文](progress.zh-CN.md)

What has actually been built, iteration by iteration, and where the next session picks up. The
[development plan](dev-plan.md) says what each iteration is meant to do; this file says what was
done and where the work differs from the plan.

## Where things stand

| | |
| --- | --- |
| Last updated | 2026-11-24 |
| Milestone | M0, foundations, done (I01–I08); M1, the proxy core: I09–I24, I17b and I18b done; M2's I26–I33 and I35–I37 and M3's I39–I43, I45, I47, I49 and I51 done early. |
| akio | `c5cf4353` on akio's `main` (`1.0.0-rc.1`, with A1–A8, A11, A13, A16–A21, A36–A38, A51–A53, A63–A66, A90, A91, A93 and A94), pinned by `rev` in `Cargo.toml` |
| fit | the specification; read at `7daee573` for this plan |
| What works | `akit run` and `akit validate` with fit's flags and exit codes, configs in JSON, `.conf` and YAML merged from files and directories; SIGHUP reloads the config (a broken one keeps the running one; listeners kept (as fit's `binding.same`: certificate files and ACME names may change), added, changed and drained on akio's A1, those it started with too, h3 ones added by a reload; a startup h3 listener stays until a restart); a graceful shutdown on SIGINT and SIGTERM; HTTP/1.1, h2c, TLS (SNI, ALPN, client authentication; certificates chosen per handshake as fit's, replaced by a reload or by `-watch-certs` without closing the listener) and HTTP/3 listeners, `Alt-Svc` on the TCP side; router v1 on canonical paths, with fit's matchers (method, header, query, remote IP, path regexps) and expressions; route middleware: `rewrite` (prefixes), `redirect`, `headers`, `rate_limit`, `concurrency_limit`, `body_limit`, `basic_auth` (bcrypt, checked on akio's pool), `client_cert` (mTLS per route), `forward_auth` (an auth service asked on akio's client, the request suspended meanwhile) and `circuit_breaker`the request suspended meanwhile) and `compress` (zstd, brotli and gzip by q value, streamed bodies encoded write by write); the `respond`, `static` (conditional and range requests, files opened on akio's pool, precompressed twins by q value, compressed copies of files kept in memory, `try_files`, listings, `strip_prefix`) and `proxy` handlers (fit's forwarding fields, trusted proxies, the client's `Host`, bodies streamed both ways with linked backpressure, trailers both ways, `timeout` and the upstream's `response_timeout`, `response_header_timeout` and `response_idle_timeout`, 504 for timeouts, the upstream request given up whenever the client goes, `retry` with its budget, the `timeout` bounding the attempts together; upstreams over HTTP/1.1, h2c and h2); `limits.max_body_bytes` on every listener (413) and in the proxy, and `limits.write_stall_timeout` (changed by a reload too); access logs in fit's json, text and line formats; upstream pools with fit's nine load balancing policies (round robin, weighted round robin, least connections, random, p2c, and consistent hashing of the client's IP, the path, a header or a cookie) and sticky cookies, passing over addresses taken out by passive health checks (`max_fails`, `fail_timeout`) and by active ones (`health_check`: http and tcp probes); the Admin API on loopback (`GET /config`, by JSON pointer too, `POST /load`, edits by pointer with `PUT`, `PATCH` and `DELETE /config`, `POST /rollback/{n}`, `POST /reload`, each with `If-Match` and all but a rollback with `?dry_run=1`, `GET /versions`; with `token_file`, a bearer token on every request), changes applied as SIGHUP's reloads, one history for both; `run -watch` applying the config's files each time they change (fit's file provider, polled); Prometheus metrics at the Admin API's `GET /metrics` (requests by listener and route with `metrics`, upstreams and backends, certificates, connections, reloads, akio's engine); certificates obtained with ACME through HTTP-01 or TLS-ALPN-01 for the names listeners list in `tls.acme`, kept in a storage and renewed by their lifetime or the CA's renewal information (ARI), their OCSP responses stapled; HTTPS by default (`auto_https`): a config that writes no listeners served on HTTPS with its routes' host names' certificates and on HTTP redirected to it (308), an empty `tls` given its routes' names; layer 4 `services` (I51): `tcp` and `udp` listeners proxied to an upstream's addresses, backpressure linked both ways, idle timeouts, `max_conns`, drained after a reload; WebSockets proxied (I49) from HTTP/1.1 upgrades and HTTP/2 and HTTP/3 extended CONNECTs to HTTP/1.1 upstreams, backpressure linked both ways, `tunnel_idle_timeout`; the compatibility gate: 80 fixtures, 24 valid, 55 refused only as not supported yet, 1 skipped, none failed |
| Next | the rest of M1's and M2's iterations; I50 (gRPC) and I54 (upstream TLS) on I18b |
| akio requests | A1–A8, A11, A13, A16–A21, A36–A38, A51–A53, A63–A66, A90, A91, A93 and A94 on akio's main and pinned; A9, A10, A12, A14, A15, A22–A24, A29, A30, A32, A33, A35, A45, A48, A49, A54, A57, A58, A75, A78, A79 and A92 requested ([akio API requests](akio-api-requests.md)) |

## Log

### 2026-11-24: I49, WebSocket proxying

- Taken early from M3, on akio API requests **A93** and **A94**, which this iteration added to akio
  (ADR-113, akio branch `claude/a93-a94-tunnels`, commit `30cdf93a`, on akio main `f6750e93`):
  akio-http's client hands a connection over on `101` (`ClientRequest::upgrade`,
  `ResponseHandler::on_upgrade`), and a streamed `2xx` to an HTTP/2 extended CONNECT is a tunnel
  whose halves close separately (HTTP/3's already were). akit pins that commit; the coordinating
  session merges it onto akio's main with A66 and moves the pin to the merged commit.
- `akit-proxy`'s `websocket` module, after fit's `handler/proxy/upgrade.go`: an HTTP/1.1 request
  that asks to switch (`Connection: upgrade` and an `Upgrade` other than `h2c`) goes to the
  upstream as the upgrade it is; an HTTP/2 or HTTP/3 extended CONNECT for `websocket` as the
  HTTP/1.1 handshake it stands for (`GET`, a `Sec-WebSocket-Key` of akit's own); one for another
  protocol is answered 501, and an h2 or h2c upstream, which cannot switch, 502. The fields go up
  as any proxied request's (forwarding fields, trusted proxies, `Host`), with the address the
  upstream's balancing and sticky cookie pick.
- The switch: an HTTP/1.1 client's connection is taken over at the handshake
  (`Context::upgrade_raw`) and gets the upstream's `101` with its fields once the upstream
  switched; an HTTP/2 or HTTP/3 client gets a streamed `200` with them, without
  `Sec-WebSocket-Accept`. The upstream's connection leaves the pool (`Exchange::on_upgrade` in
  `akit-upstream`): its slot is given back at once, so open WebSockets do not fill `max_conns`
  or `max_pending`, and it counts among the address's connections (`least_conn`) until it closes.
- The tunnel: bytes go on from the callback that read them; past 64 KiB waiting to go to one side
  (fit's `tunnelWatermark`) the other side's reads are held (an HTTP/1.1 connection's, an HTTP/2
  or HTTP/3 stream's `BodyReads`, the upstream connection's) until that side drained. Bytes a
  client sends before the switch wait, its reads held, up to 64 KiB. A side that ends ends the
  other after what it sent; a client that goes closes the upstream's connection, or gives the
  handshake up (its `CancelToken`). `tunnel_idle_timeout` (fit's 10 minutes when left out or 0)
  closes a tunnel that carried nothing either way, checked on akit's timer thread.
- Answers that do not switch are the client's: over HTTP/2 and HTTP/3 through the proxy's
  ordinary relay (streamed past 64 KiB with backpressure), but a `2xx`, which a client would take
  for an open tunnel, is a 502 and the upstream's connection closed; over HTTP/1.1 written on the
  connection akit already holds, framed by its length (or by the connection's close), whole up
  to 64 KiB (a cut one then a 502), streamed past it with backpressure (a cut one cut short in
  turn). A handshake that gets no answer is fit's 502, 503 or 504.
- Config: the proxy's `tunnel_idle_timeout` (`akit_config::DEFAULT_TUNNEL_IDLE_TIMEOUT`).
- Tests (fit's `websocket_test.go`, `websocket_ext_test.go`, `websocket_unix_test.go` and
  `upgrade_test.go`, ported in `crates/akit-proxy/tests/websocket.rs`): echo with subprotocol and
  16 MiB both ways at once, refusals and no upstream, the idle timeout, WebSockets leaving the
  pool, a 64 MiB flood to a client that reads nothing over HTTP/1.1, h2c (its stream window) and
  HTTP/3 (its stream's reads held), large refusals by length, chunked and close-delimited with
  the backend kept in rotation, a slow client's small refusal, refusals cut short, a client gone
  mid-refusal, WebSockets over h2c, h2 and h3 to an akio WebSocket server, and an upstream
  answering an extended CONNECT without switching.
- Not done: an HTTP/1.1 WebSocket's access log line (its `101` is written by akit on the
  connection it took over, not sent through akio's response path, which writes the line); fit's
  drain of open tunnels on a reload (they run on until they end or idle out; the engine's
  shutdown closes them).
- No benchmark (rule 30): what should be measured is in the dev plan's benchmark table.

### 2026-11-24: I47, compression

- `akit-config` reads fit's `compress` middleware (`encodings`, `level`, `gzip_level`,
  `brotli_level`, `zstd_level`, `min_bytes` and its old name `min_length`, `max_in_flight`) into
  `Handler::Compress`, its levels worked out as fit's `Provision` does (fastest 1/1/1, default
  5/4/3, better 7/6/6 for gzip/brotli/zstd, a level by number in its place), with fit's checks: an
  encoding of `br`, `zstd` and `gzip`, none listed twice, levels in range. `compress` leaves
  `FIT_MIDDLEWARE`. The `static` handler's `compress` leaves `STATIC_FIT_ONLY`.
- `akit-middleware` `compress`, after fit's `middleware/compress`: a response filter picks the
  encoding the client prefers most by q value among those offered (`x-gzip` is gzip, `*` stands
  for the rest, among equals the first offered), for a response whose status carries a body and
  is not 206, with no `Content-Encoding` (but `identity`) or `Content-Range`, no `no-transform`,
  a type fib's list calls compressible, and a declared length (the field's, or a whole body's) of
  at least `min_bytes`. It adds `Vary: Accept-Encoding` (HEAD gets that alone), drops
  `Content-Length` and makes a strong `ETag` weak. A whole body is compressed whole in the filter;
  a streamed one (the proxy's, I16) is encoded by akio write by write and flushed, so each piece
  the upstream sends reaches the client decodable at once, under the relay's linked backpressure.
  Streams use fit's windows (2^18). At most `max_in_flight` responses are compressed at once: a
  streamed one keeps its place until its `BodyStream` ends or is dropped (a client that goes),
  through akio's `Response::keep_until_sent`.
- The `static` handler's `compress`: akio's `Files::compress` keeps a file held in memory
  compressed too, one copy per encoding, made once on akio's disk pool at gzip 9, brotli 9 and
  zstd 19 the first time a client prefers it; a copy has the file's tag with `-encoding` added and
  no `Accept-Ranges`; ranges, small files, types that do not compress and copies that save less
  than a twentieth get the file; twins on disk go first.
- akio: requested and built as **A90** (`akio_http::Coding`, `Response::set_stream_coding`,
  `is_streaming` and `keep_until_sent`: a filter of akit's own sets the stream's coding, which
  akio's own `compress` middleware alone could) and **A91** (`Files::compress`), on akio's branch
  `claude/a90-a91-compress` (`b5cd3f25`, ADR-112, off akio main `f6750e93`), not merged. akit's
  `Cargo.toml` still pins `4f7169c4`: this commit builds once the pin moves to the merged akio
  commit; it was built and tested through a local, uncommitted `[patch]`. Requested only: **A92**,
  a body sent from a file on disk encoded too.
- Tests: unit tests of the negotiation (fit's `TestNegotiate`), eligibility and `Vary` (fit's
  `TestEligibleAndVary`), compressible types and whole bodies decoded.
  `tests/compress.rs` through akit's listener, over HTTP/1.1 and h2c: fit's `TestCompress` (the
  negotiation table, the responses left alone, HEAD, weak tags), `TestCompressStream` (the first
  event decoded before the second is written, each encoding), `TestCompressBounded` (past
  `max_in_flight` uncompressed; a stream whose client goes gives its place back),
  `TestCompressReleased`, `TestCompressLevels` and `TestCompressParams`; `min_bytes` under and at;
  through the proxy, events from a pausing upstream arriving decoded before its pause ends, a body
  by its length compressed, a client that reads nothing for half a second getting a 4 MiB body
  whole, and one that goes midway giving its place back; the middleware in front of the `static`
  handler; fit's `TestCompressCopies` through the `static` handler. In akio: a filter of its own
  setting the coding and its guard kept until the stream ends, and the copies.
- Compatibility gate: 11 fields more in `tests/compat/supported.txt`; `make compat-bless`:
  `examples/compress` is valid (80 fixtures: 23 valid, 56 refused only as not supported yet, 1
  skipped, none failed).
- Differs from fit: a body akio sends from disk (a static file above `max_file_bytes`) goes
  uncompressed, where fit's middleware compresses it as it is read (A92); every write of a
  streamed body is flushed, where fit flushes when the handler does, so a stream of many small
  writes compresses less well; the static copies are not counted in `max_cache_bytes`, which akit
  does not have yet (A15); fit's `TestCacheOutside` waits for the `cache` middleware (I67).
- To measure, in the maintainer's benchmark tasks (rule 30): see the dev plan's benchmark row for
  I47.

### 2026-11-24: I45, retries, circuit breakers, route timeouts

- `akit-config` reads the proxy's `retry` (`attempts`, 1 to 10, 2 by default; `statuses`, each
  5xx; `budget_ratio`, 0 to 1, 0.2; `budget_min_per_second`, at least 0, 10) into
  `Proxy::retry` (`Retry`), and the `circuit_breaker` middleware (`window`, `min_requests`,
  `error_ratio`, `statuses`, `slow_threshold`, `slow_ratio`, `open_duration`,
  `half_open_requests`, with fit's defaults) into `Handler::CircuitBreaker` (`CircuitBreaker`),
  with fit's checks and messages ("404 is not a 5xx status", "600 is not a status", the ranges of
  its tags).
- Retries (`akit-proxy`'s `retry.rs`), as fit's `retry.go`: a request whose connection could not
  be made (refused, unreachable, its connect timed out) or whose address had `max_pending`
  requests waiting is sent again whatever its method; one of an idempotent method (GET, HEAD,
  OPTIONS, TRACE, PUT, DELETE) also when it failed before its response began (not when the pool
  is closed) or was answered with one of `statuses`. Only a request whose body went whole is
  replayed (the template kept beside it is a copy of a body of at most 64 KiB); one whose body
  streams (past 64 KiB, of no declared length, or carrying trailers) is sent once. Neither one
  whose client went, nor one past the route's `timeout`. The next attempt is made from the pool's
  callback for the one before, before anything reaches the client: a new `Relay` takes the client
  over, the address the upstream picks next and its sticky cookie recomputed, as fit's retry
  does. fit's turn is the upstream's, akit's each I/O thread's, so an address tried already is
  passed over while one is left that was not. A retried status's small body is read and dropped,
  which keeps the connection; a larger one is cancelled. The access log names the address that
  answered.
- The budget, as fit's (Finagle's and Linkerd's): each request adds `budget_ratio` of a retry, it
  gains `budget_min_per_second` a second, at most ten seconds' worth (at least 10), and a retry
  takes one. fit's mutex is an atomic `f64` here: a deposit into a full budget (a healthy
  upstream's) only reads it; a withdrawal, which only a failed attempt makes, takes a small lock
  for the time earned.
- Route timeouts: the proxy's `timeout` (I17) now counts from when the proxy takes the request,
  and bounds the attempts together: each attempt gets what is left of it, and none is made past
  it (504).
- The circuit breaker (`akit-middleware`'s `breaker.rs`), as fit's `middleware/breaker`: closed,
  open (503 with `Retry-After`), half-open with `half_open_requests` probes, the window in ten
  steps. How a request fared is read by a response filter, which sees the status of a whole,
  proxied or streamed response when its head goes; a probe whose client goes gives its place
  back. fit's mutex taken twice a request is, for a closed breaker, an atomic state and atomic
  step counters tagged with their step; the lock is taken only to change state and by half-open
  admissions.
- Tests: `crates/akit-proxy/tests/retry.rs` ports fit's `TestRetryConnectFailure`,
  `TestRetryNotResentOnceSent` (the PUT reaches the upstream exactly four times: akio's client
  does not resend on its own as fib's does), `TestRetryStatuses` and `TestRetryBudget`, and adds a
  streamed body that is not replayed, the route's timeout spanning the attempts, the sticky
  cookie naming the address that answered, and no retry without `retry`; `retry.rs`'s unit tests
  the budget, the methods and the errors. fit's `TestBreakerTransitions`, `HalfOpenLimit`,
  `Window`, `SlowAndStatuses` and `LateOutcome` run on a test clock in `breaker.rs`;
  `crates/akit-middleware/tests/breaker.rs` opens it on the `respond` handler's failures and a
  proxied upstream's (whose requests then stop), closes it with probes, and counts slow proxied
  responses. `crates/akit-config/tests/proxy.rs` and `tests/breaker.rs` port `TestRetryParams`
  and `TestBreakerParams`. fit's `TestRouteTimeout` was ported with I17 (`tests/failure.rs`).
- Compatibility gate: 13 fields more; `make compat-bless`: `examples/circuit-breaker` is valid,
  and `examples/load-balancing` no longer refused for `retry` (80 fixtures: 23 valid, 56 refused
  only as not supported yet, 1 skipped, none failed). No akio requests (A87–A89 unused).
- Checked: `make check` on macOS; on gpu51 under the bench lock, with epoll and with
  `AKIO_TEST_BACKEND=uring`, the tests of akit-config, akit-proxy, akit-middleware, akit-runtime,
  akit-upstream and akit-compat: all passing.
- To measure, in the maintainer's benchmark tasks (rule 30): see the dev plan's benchmark row for
  I45.

### 2026-11-24: I18b, HTTP/2 upstreams II: streams reset, timed and paced by akio

- akio moves to `d27e6a20` (rule 22): A51–A53 (ADR-110, `f6750e93`) and A66, which this
  iteration's session added to akio (ADR-111: `H2Client::request` and `open` heed
  `response_header_timeout`, `response_idle_timeout` and a `CancelToken`, each resetting the
  stream alone).
- `akit_upstream::h2` drops I18's and I17b's stopgaps. A stream given up (the client gone after
  the head, its body dropped unfinished, its `CancelToken`) is reset by akio and fails
  `Cancelled`, which is not the address's failure; its connection goes on carrying the others
  (no more retiring). `response_timeout`, `response_header_timeout` and `response_idle_timeout`
  are akio's (`ClientRequest::timeout` and the two fields), so the per-address tick only closes
  idle connections, and runs only while an address has one. A held download keeps its stream's
  window closed (`H2Stream::hold_reads`; `HELD_LIMIT` is gone); an upload is paced by
  `H2Upload::queued_bytes` and `on_drain`, exactly as over HTTP/1.1 (`Upload::queued_bytes`,
  `on_drain` over h2); request trailers end the body (`finish_with_trailers`).
- Connections are sized by the server's `SETTINGS_MAX_CONCURRENT_STREAMS`
  (`H2Client::max_concurrent_streams`; `STREAMS_PER_CONN`, 100, until it says or when it sets no
  limit), a new one dialed while the address has fewer than `max_conns` open; past that the
  stream waits in akio on the least busy connection. A connection that is no longer open (after a
  `GOAWAY`: `is_open`) takes no more streams and is closed once its last ends. The pool's slots
  still bound an address at `max_conns` × 100 streams.
- The dial is bounded by the upstream's `connect_timeout` (`H2Options::happy_eyeballs`), the
  probes' by the check's timeout, per address raced when a name resolves to several (a lone
  address has the engine's, as over HTTP/1.1: A12). `h2` offers `h2` and `http/1.1` in ALPN, as fit's does; a server
  that chooses `http/1.1` fails akio's dial with `NotH2`, and the address is spoken HTTP/1.1 over
  TLS by the pool's HTTP/1 client (given the same TLS configuration) from then on, the requests
  that waited for the dial included; its probes follow.
- Tests: `akit-upstream` `tests/h2.rs` (a connection's streams up to the server's limit, here 10;
  the exchange's timeout, the header timeout and a cancel token each resetting their stream with
  the connection kept; a cancelled token's request never sent and no failure); `akit-proxy`
  `tests/h2.rs` (`request_trailers` no longer ignored; a slow client holding an h2 download and
  then getting all of it, in place of the held-limit stopgap's test; an `h2` upstream whose server
  chooses HTTP/1.1); `akit-proxy` `tests/stream.rs`, fit's stream tests against an h2c upstream of
  the test's own (std sockets, honouring flow control): 64 MiB down to a slow client and up to a
  slow upstream, by length and chunked, from HTTP/1.1 and h2c clients, the resident memory's rise
  bounded as over HTTP/1.1 (checked on Linux).
- Not measured (rule 30): the proxy to an h2c upstream (dev plan's benchmark table, its row
  updated: akio's per-stream timers, a held download and a paced upload against HTTP/1.1's).

### 2026-11-24: I51, TCP and UDP proxying

- Taken early from M3. `akit-config` reads fit's `services` (`name`, `listeners`, `upstream`,
  `idle_timeout`, `max_conns`) into `Config::services`, and `listeners[].protocols` takes `tcp`
  (alone: beside other protocols is I53's, refused as not supported yet) and `udp` (alone, as
  fit requires). fit's checks and messages: names unique and valid, at least one listener, each a
  `tcp` or `udp` one and one service's, the upstream there, every `tcp` and `udp` listener taken
  by a service, no route on one, no `tls` on a listener that serves no HTTP. `server_names` and
  `proxy_protocol` (I52) and `access` are refused as not supported yet; services with
  `server_names` may share a `tcp` listener, as in fit, so that the gate reports only those.
- `akit-l4`, after fit's `l4` package: a `Service` per config service, with its upstream's pool, in
  the graph (`Graph::services`, `Graph::service(listener)`); `ClientAcceptor` serves a `tcp`
  listener (an akio TCP listener) or a `udp` one (`Listener::udp`, each client address a peer
  connection), each connection going to the service the current graph has for the listener when
  it opens. The client's reads are held while the upstream is dialed on akio's engine
  (`Engine::dial`, `dial_udp`; a host name resolved by `akio::dns` and, over TCP, its addresses
  raced by `HappyEyeballs` within the upstream's `connect_timeout`). Data goes on from the
  callback that received it; past 64 KiB unsent (`WATERMARK`) the sending side's reads are held
  until the other's `on_drain`; both connections are `set_relayed`. A side that ends closes the
  other after what it sent (`close_after_send`). UDP: each datagram one datagram, dropped when it
  cannot be sent, up to 64 KiB of them kept until the upstream socket opens; an ICMP refusal ends
  the session and counts against the address.
- The address: `Pool::stream_pick` (the upstream's `load_balancing`, `ip_hash` by the client's
  address, down addresses passed over) counts a connection on the address, which `least_conn`
  reads, until `Pool::stream_done`, which records its outcome as a request's (`max_fails`); a
  connect that fails is tried on the next address picked, up to three.
- Limits: `max_conns` per service (10,000), one past it closed on accept (a UDP datagram dropped);
  idle timeouts (10 minutes for TCP, 1 for UDP, or `idle_timeout`) swept per service on akit's
  timer thread (`akit_upstream::timer`, now public) every quarter of the shorter one, between
  10 ms and 1 s; akio's UDP peer timeout raised to the longest service's, at start and on each
  reload. Both are stopgaps for akio API requests A45 and **A78** (an idle timeout per
  connection). The engine's connect timeout bounds a dial of an IP literal, as for HTTP (A12;
  **A79** asks for `Engine::dial` with a timeout of its own).
- Listeners: built and reloaded beside HTTP's (`akit_listener::engine`, `Listeners::reload`); a
  kept `tcp` or `udp` listener hands new connections to the new graph's service, and those the
  old graph's services still proxy are closed after the drain timeout (`-shutdown-timeout`), as
  fit's reload drains them. A `udp` listener akit started with cannot be removed or changed by a
  reload (akio A22), as for h3. The engine's shutdown closes relays at once (`on_shutdown`).
- Not done: the eBPF sockmap relay the plan names went to **I51b**: fit has no field for a
  listener to ask for it, so it needs a field of akit's own (or fit's first), and its idle
  timeout must come from the sockets' byte counts.
- Tests: `crates/akit-listener/tests/l4.rs` ports fit's `TestTCPEcho`, `TestTCPLarge` (32 MB each
  way), `TestTCPSlowClient` and `TestTCPSlowUpstream` (64 MB held back), `TestTCPHangUp`,
  `TestTCPConnectFailure`, `TestTCPEngineStopped`, `TestTCPIdleTimeout`, `TestTCPMaxConns`,
  `TestTCPReload`, `TestUDPEcho` (200 sessions), `TestUDPSessionsExpire`, `TestUDPMaxConns`,
  `TestUDPRefused` and `TestUDPReload`, with a host-name upstream and the counts on the
  address; `crates/akit-config/tests/services.rs` the fields, defaults and checks. fit's
  `TestTCPDenyInternal` waits for `deny_internal`, and the SNI, PROXY and sniffing tests for
  I52 and I53.
- Compatibility gate: 6 fields more; `make compat-bless`: `examples/l4-udp` is valid and
  `examples/l4-tcp` refused only for `services[].server_names` and `proxy_protocol` (80 fixtures:
  19 valid, 60 refused only as not supported yet, 1 skipped, none failed).
- Checked: `make check` on macOS (fmt, clippy, the workspace's tests, `cargo deny`, the docs and
  the compatibility inventory); on gpu51 under the bench lock, with epoll and with
  `AKIO_TEST_BACKEND=uring`, the tests of akit-config, akit-upstream, akit-l4, akit-runtime,
  akit-listener, akit-compat and akit: all passing. The two tests that open hundreds of sockets
  run alone (gpu51's many cores give each engine many threads, and a soft limit of 1,024 files
  was passed with every test's engine open at once).
- To measure, in the maintainer's benchmark tasks (rule 30): see the dev plan's benchmark row for
  I51–I53.

### 2026-11-24: I17b, reverse proxy III b: header and idle timeouts, cancellation at any point, write stalls

- On akio `4f7169c4` (A36–A38, ADR-108), already pinned. Config: the upstream's
  `response_header_timeout` and `response_idle_timeout` (`Upstream::response_header_timeout`,
  `response_idle_timeout`; fit's 60s when left out or 0, `DEFAULT_RESPONSE_HEADER_TIMEOUT`,
  `DEFAULT_RESPONSE_IDLE_TIMEOUT`) and `limits.write_stall_timeout` (`Limits::write_stall_timeout`,
  fit's 30s, `DEFAULT_WRITE_STALL_TIMEOUT`) are read; none of the three is refused any more.
- `akit-upstream`: a pool's client gets both timeouts as its defaults (`ClientOptions`), so each
  HTTP/1.1 request fails with `ClientError::HeaderTimeout` or `IdleTimeout` past them, counted
  against the address (`Pool::response_header_timeout`, `response_idle_timeout`). A request may
  carry a `CancelToken` (re-exported); cancelled, it fails with `ClientError::Cancelled`, which
  `Error::is_upstream_failure` and the pool do not count against the address. Over h2 akio's
  client heeds none of the three (akio API request A66), so the stopgap tick of
  `akit_upstream::h2` checks each stream every 100ms for its head (from the body's end), its
  silence mid-body (stopped while the download is held) and its token, besides its deadline.
- `akit-proxy`: every proxied request carries a `CancelToken`, cancelled from `Context::on_gone`
  (registered before `detach`, carried to the streamed response): the client gone before the
  head, mid-upload over HTTP/1.1 too, or mid-download gives the upstream request up wherever it
  is, a wait for a connection included, and the exchange says it gave up, so the address is not
  counted as failing. `status_of` answers 504 for any timeout (`ClientError::is_timeout`); an idle
  timeout after the head cuts the response short as I17's broken bodies are.
- `akit-listener`: the engine's `write_timeout` is the config's `limits.write_stall_timeout`, and
  `Listeners::reload` sets the new one on akio's live settings (`Live::set_write_timeout`), the
  open connections included. `akit-runtime`: `Graph::write_stall_timeout`.
- Tests: `akit-proxy/tests/failure.rs` ports `TestResponseHeaderTimeout` (504 after it, over
  HTTP/1.1 and h2c, the upstream's request given up; a long body is not bound by it),
  `TestResponseIdleTimeout` (bodies cut short, of a length and of none, HTTP/1.1 and h2c, h2c's
  `Fit-Error`), the rest of `TestClientAbort` (before the head over HTTP/1.1 and h2c, mid-upload
  over HTTP/1.1, no failure counted), an idle timeout that spares a client that reads nothing
  for longer, and a write stall timeout that closes a client that reads nothing; `tests/h2.rs`
  the same three against an h2c upstream; `akit-upstream/tests/pool.rs` the header timeout and
  cancelled requests (sent and waiting); `akit-listener/tests/reload.rs` the write timeout set at
  start and changed by reloads; the config fields and their errors in `akit-config`. The compat
  gate's samples cover the three fields: `examples/grpc` is valid now (19 valid, 60 refused,
  58 fields refused).
- Differences from fit:
  - `write_stall_timeout` is akio's engine write timeout: a connection is closed between one and
    two timeouts after its last progress, an HTTP/2 connection whole rather than one stream reset,
    and HTTP/3 is not bounded by it (akio's is TCP's).
  - Over an h2 upstream the header and idle timeouts and the cancel are noticed within the tick
    (100ms), and a stream given up for them is left running with its connection retired (A51,
    A66).
  - A cancelled request waiting for a connection keeps its place in the address's queue until a
    connection is free, and then fails at once without being sent.
- An unrelated test fix: `akit-upstream`'s `an_upstream_that_refuses_connections` no longer
  assumes the test thread's round-robin turn starts at the first address.
- For a benchmark task: each proxied request now makes a `CancelToken` (one allocation) and
  registers an `on_gone` callback; over h2c, each DATA frame re-arms the idle timeout (a clock
  read and a store). The proxy scenario's row in the
  [benchmark table](dev-plan.md#benchmark-and-optimization-status) names them.

### 2026-11-24: I33, automatic HTTPS defaults

- fit's `auto_https` (`config/autohttps.go`), in a new `akit-config` module `auto_https`: the
  field (`disable`, `disable_redirects`, `http_port`, `https_port`; on, with redirects, on 80
  and 443 when left out, 0 for the default) and fit's `Effective`. A config that writes no
  listeners gets an `https` listener on `:https_port` (HTTP/1.1 and h2) whose `tls.acme` is every
  route's host name, and, unless redirects are off, an `http` listener on `:http_port`; routes
  may name both. A listener written with an empty `tls` (no certificates, no names, `acme: []`
  included) gets the host names of the routes it serves. The names are fit's `AutoHTTPSName`:
  two labels or more, lower case, each once, no wildcard, IP address or private name
  (`localhost`, `local`, `internal`, `home.arpa`, `invalid` and their subdomains).
- Worked out once when the config is read: `Config::listeners` holds the listeners served,
  written or made (fit keeps the written ones and calls `Effective` where it needs the others),
  so the runtime binds, diffs on reload and serves them as any other; `Config::auto_https` keeps
  the field. A made `http` listener carries `Listener::redirect_https`, the port it redirects
  to; the graph gives it no routes.
- The redirect (fit's `redirectHTTPS`): on such a listener, a request no route takes (all but
  HTTP-01's challenges, answered first as on every listener in the clear) is answered 308 to
  `https://` the request's host without its port (an IPv6 address in brackets; the target's
  authority when there is no `Host`), `:port` unless 443, and the same target (the path and
  query of one in absolute form); 400 with fit's body when there is no host. The listener's port
  is read only for a request no route takes, so routed requests do no more work than before.
- fit's checks and messages: no listeners and no host names (`at least one listener is needed,
  or a route that matches a host name for HTTPS by default`), no listeners with it off, the HTTP
  port the HTTPS one (at `auto_https.http_port`), and an empty `tls` that gets no names (at the
  `tls`). The "not supported yet" refusals of a config without listeners and of an empty `tls`
  are gone. **Differs from fit**: a config with no listeners and no routes is still accepted
  (akit started with only the Admin API), where fit refuses it unless it has a console or roles.
- Tests: `akit-config` unit test of the names (fit's `TestAutoHTTPSName`); `tests/auto_https.rs`
  ports fit's `TestEffective` and the checks of `config_test.go` and `more_test.go`, with
  positions; `akit-listener`'s unit test of redirect locations (fit's `TestRedirectEdges`) and
  `tests/auto_https.rs`: fit's `TestAutoHTTPSRedirect` (the listeners and ports made, 308 for
  GET and POST, hosts with ports and IPv6, the challenge path of an unknown token, absolute
  targets), no redirects, written listeners with an empty `tls`, and fit's
  `TestAutoHTTPSFromPebble`: a config naming only a host gets its certificate from Pebble
  through HTTP-01 on the made `http` listener and serves it on `https`, and `http` redirects the
  rest.
- Compatibility gate: `auto_https` and its four fields in `tests/compat/supported.txt`;
  `make compat-bless`: `examples/auto-https` is valid (80 fixtures: 19 valid, 60 refused only as
  not supported yet, 1 skipped, none failed).
- Checked: `make check` on macOS (with Pebble); on gpu51 under the bench lock, with epoll and
  with `AKIO_TEST_BACKEND=uring`, the tests of akit-config, akit-compat, akit-tls, akit-runtime,
  akit-listener and akit: all passing. No akio request.
- To measure: nothing; routed requests take the same path as before (rule 24: control plane,
  and a redirect off the hot path).

### 2026-11-24: I43, ForwardAuth

- `akit-config` reads fit's `forward_auth` (`upstream`, `uri`, `request_headers`, `copy_headers`,
  `timeout`) into `Handler::ForwardAuth`, with the checks of fit's `Provision` and its messages:
  `uri` a path with a query or not (starting with `/`, no control character, valid `%` escapes in
  the path), field names HTTP tokens, `timeout` 10s when left out or 0. It leaves
  `FIT_MIDDLEWARE`. That `upstream` names one of the config's is checked when the middleware is
  provisioned, as the proxy's.
- `akit-middleware` `forward`: the auth service is sent a `GET` of `uri` (its path escaped as Go
  writes it) on the upstream's pool, by its load balancing, with the request's fields (those
  `request_headers` names, or all but the hop-by-hop ones, those `Connection` names and
  `Content-Length`, `Content-Type` and `Expect`), its `Host`, and `X-Forwarded-Method`, `-Proto`,
  `-Host`, `-Uri` and `-For`. The request is suspended (`Context::suspend`, akio's A65) and resumed
  from the pool's callbacks, so it holds no I/O thread while it waits. A 2xx answer lets it
  through with the fields `copy_headers` names, after every spelling of them the client sent is
  removed (as `basic_auth`'s `user_header`); its body is read and dropped when small, else given
  up. Any other answer is the client's, its status, fields but the hop-by-hop ones, and its body
  when within 64 KiB (a larger one is given up and the answer goes without it, as fit's). Past
  `timeout` (the request's own timeout on the pool), 504; an auth service that cannot be reached
  or fails, 502; a pool closed meanwhile, 503. A client that goes while its request waits cancels
  the auth request (`Context::on_gone` and akio's `CancelToken`, A37/A38), which is not counted
  against the auth service's health.
- A request whose body would stream to its handler cannot be suspended (ADR-109): as
  `basic_auth`, `forward_auth`'s `body_mode` asks for it whole, within the listener's
  `max_body_bytes`. Requested of akio as **A75** (a request whose body streams, suspended with its
  reads held).
- Tests: unit tests of field names, the fields sent by default, origin forms and the failures'
  statuses. `tests/forward_auth.rs` through akit's listener, with an auth service on plain
  threads: fit's `TestForwardAuth` (allowed with the auth service's fields and what it saw, forged
  fields in every spelling, a redirect passed on with its fields and body, a 401 whose 200 KiB body
  is dropped, 504 past the timeout), `TestForwardAuthUnreachable` (502),
  `TestForwardAuthClientGone` (the auth service sees its request given up),
  `TestForwardAuthSlowService` (128 requests wait half a second at once while another route
  answers within 250 ms) and `TestForwardAuthParams`; `request_headers` limiting what is sent, a
  chunked body read whole before the check, and fit's `examples/forward-auth` shape: `forward_auth`
  in front of the `proxy` handler, the app getting the copied field and a 200 KiB body. fit's `TestForwardAuthAuthenticates` has no akit
  counterpart yet (no request is marked authenticated, as for I42).
- Compatibility gate: 7 fields more in `tests/compat/supported.txt`; `make compat-bless`:
  `examples/forward-auth` is valid (80 fixtures: 19 valid, 60 refused only as not supported yet, 1
  skipped, none failed).
- Differs from fit: a request body that would stream is read whole before the check (A75); fit's
  drain of waiting checks when a config is replaced is the pool's drain here (the check finishes,
  then the old graph's handler answers); the request is not marked authenticated for the access
  log or a cache (as I42).
- To measure, in the maintainer's benchmark tasks (rule 30): see the dev plan's benchmark row for
  I43.

### 2026-11-24: I29, the file provider

- `akit run -watch` (fit's file provider, `provider/file`, behind fit's `run -watch`): the config's
  files are looked at every `-watch-interval` (1s) and, once a change has settled for 500 ms and
  the files hold something other than what was applied last (SHA-256 of their names and
  contents), they are applied as SIGHUP applies them, a new version with source `file` through
  `Configs::update`, the same path as SIGHUP and the Admin API (`akit: FILES changed;
  reloading`, then fit's reload lines). As fit decided (fit's decision 006) it polls sizes and
  modification times rather than asking for events: no dependency, the same on every system, and
  a file renamed into place is seen as well as one written in place. The files `-c` stands for
  are found afresh at each look (`akit_config::config_files`), so a file added to a directory, or
  taken out, is a change too. A broken edit is reported once (fit's lines and errors) and the
  running config stays until the next edit; a file that cannot be read (removed, being replaced)
  is reported once (`akit: watching FILES: ...`) and applied when it comes back; a touch, or a
  change changed back, applies nothing. SIGHUP tells the watcher what it read and applied, so the
  watcher does not apply it again. The read is bounded at 64 MiB for all the files
  (`akit_config::MAX_CONFIG_BYTES`), as fit's.
- `akit_admin::Watcher` (`crates/akit-admin/src/watch.rs`), fit's `file.Watcher`, with
  `WatchOptions` (debounce, the most read, how paths expand), `tick(now, apply)` returning a
  `WatchReport` (`Unreadable(WatchError)` or `Failed`), and `applied(files)`. It starts as if the
  files had just changed, as fit's, so an edit made between the config's read and the watcher's
  start is applied.
- `akit-config`: `ConfigFile` (a file's path and bytes), `read_config_files` (bounded, as
  before) and `load_config_files` (the config and its JSON document from files already read);
  `load_files` and `from_files` are built on them. `akit run` reads the files once at start and
  on SIGHUP, and loads what it read, so that the watcher compares with exactly what was applied.
- Tests: `akit-admin/tests/watch.rs` (12, fit's `provider/file` tests: an edit, a rename into
  place, a touch, an edit before watching, writes in a row applied once, a broken edit reported
  once and the next applied, a file removed and back, a file too large, contents applied another
  way, a directory's files added and taken out, akit's own directory reading, defaults, files
  that cannot be found, a directory read as a file); `akit/tests/watch.rs` (2, end to end with
  `akit run -watch`, fit's `TestServeWatch`, `TestWatch` and `TestConfigDirectory`: each edit
  served, SIGHUP's edit not applied again, a broken edit reported once while the running config
  serves, a removed file reported and applied when back, a touch ignored; a directory of `.conf`
  and YAML files with files added, taken out and broken, reported by name). `cli.rs`: `-watch` is
  no longer a usage error.
- Checked: `make check` on macOS; the tests of akit-config, akit-admin and akit on Linux (the
  benchmark host, gpu51), epoll and io_uring.
- Differences from the plan and from fit:
  - The watcher is ticked on the control thread's loop, beside `-watch-certs`' (they share
    `-watch-interval`), not on a goroutine of its own: the reloads it starts, SIGHUP's and the
    Admin API's are one after the other there, and none blocks an I/O thread (rule 1).
  - fit refuses `-watch` with a config kept in its store; akit has no store yet.
  - No config field: the file provider is a flag, as in fit, so `tests/compat/supported.txt` is
    unchanged. A Unix control socket (A23) is not part of it.
- No benchmark: the control plane (rule 24).

### 2026-11-24: I42, basic auth and mTLS

- Taken early from M3, after I41. `akit-config` reads fit's `basic_auth` (`realm`, `users` with
  `username` and `password_hash`, `user_header`, `forward_authorization`) and `client_cert`
  (`optional`, `subject_header`) into `Handler::BasicAuth` and `ClientCert`, with the checks of
  fit's `Provision`s and their messages (the realm with no quote, backslash or control
  character; at least one user; names not empty and without a colon or control character, none
  twice; hashes that `akio_crypto::bcrypt` parses, with its reason; field names HTTP tokens). They
  leave `FIT_MIDDLEWARE`; akit-config depends on akio-crypto for the hashes.
- akio (rule 21): what I42 needs went into akio first, on branch `claude/a63-a65-auth`, commit
  `5e0d433c` (ADR-109), off akio's main `afbbb521`: **A63** bcrypt in akio-crypto; **A64**
  `Request::peer_certificates`, the client's verified chain on every HTTP/1.1, HTTP/2 and HTTP/3
  request (`TlsInfo::peer_certificates`, `akio_tls::peer_chain`,
  `akio_quic::Connection::peer_certificates`); **A65** `Context::suspend`, `Suspended::resume`
  and `Context::suspend_to_pool`. With their tests, on macOS and on gpu51 (epoll and io_uring).
  **Not on akio's main and not pushed**: akit's `Cargo.toml` still pins `afbbb521`, and this
  commit builds once the pin moves to the merged akio commit (akit was built and tested here
  against `5e0d433c` through a local `[patch]`, not committed).
- `akit-middleware` `auth`:
  - `basic_auth`: HTTP Basic credentials read as Go's `Request.BasicAuth`; 401 with
    `WWW-Authenticate: Basic realm="…", charset="UTF-8"` and fit's body. A password not yet
    remembered is checked on akio's logical pool: the request is suspended
    (`suspend_to_pool`) and resumed into the next middleware and the handler, or answered 401.
    An unknown user's password is checked against the first user's hash, so a refusal takes as
    long. A password that passed is remembered per user as an HMAC-SHA-256 under a random key,
    read without a lock (a seqlock of four words) and compared in constant time, so the user's
    next requests are let in inline. As fit's pool: at most 1024 checks waiting or running, one
    that waited 5 s answered 503 with `Retry-After: 1`, and 503 from a full pool. Admitted, the
    request loses `Authorization` unless `forward_authorization`, and `user_header` is set to the
    user after every spelling of it a client sent is removed (`X_User`, `x_user`, …, as fit's
    `fieldSet`: names compared ignoring case, `_` read as `-`).
  - A request whose body streams to its handler cannot be suspended (ADR-109): `basic_auth`'s
    `body_mode` asks for the body whole when the request's password is not remembered (within
    the listener's `max_body_bytes`), the handler's mode otherwise. A password forgotten between
    `body_mode` and `call` (another one stored for the user) is answered 503.
  - `client_cert`: no verified certificate (none sent, or in the clear) is 403 `Forbidden`
    unless `optional`; `subject_header` is set to the leaf's subject as Go's
    `pkix.Name.String` writes it (RDNs most specific first, multi-valued with `+`, Go's
    escaping, other types by OID), control characters replaced as fit's `headerSafe`, after any
    spelling of it the client sent is removed, with or without a certificate. The subject is
    parsed once per run of a connection's requests (a per-thread cache by the shared chain).
- Tests: unit tests of credentials, field spellings, the remembered HMAC, the checks' bound,
  `headerSafe` (fit's `TestHeaderSafe`), names (checked against Go's output) and OIDs.
  `tests/auth.rs` through akit's listener: fit's `TestBasic` (refusals, remembered passwords,
  per-user, forged fields in every spelling), `TestBasicForwardAuthorization`,
  `TestBasicParams`, `TestClientCert` and `TestAuthenticated`'s cases (a TLS listener with
  `verify_if_given`: verified, none, optional, in the clear, with `basic_auth` before it), the
  certificate over h2 and h3; a slow check on one HTTP/2 connection while other streams are
  answered (the check is off the I/O thread); bodies sent with a first-time password through the
  proxy. fit's `TestBasicBusy` is covered by the unit test of the bound and akio's own 503 tests.
- Compatibility gate: 12 fields more in `tests/compat/supported.txt`; `make compat-bless`:
  `examples/basic-auth`, `examples/client-cert` and `examples/mtls` are valid (80 fixtures: 18
  valid, 61 refused only as not supported yet, 1 skipped, none failed).
- Checked, against akio `5e0d433c`: `make check` on macOS (fmt, clippy, the workspace's tests,
  `cargo deny` with the lock pinned at `afbbb521`, the docs and the compatibility inventory); on
  gpu51 under the bench lock, with epoll and with `AKIO_TEST_BACKEND=uring`, the tests of
  akio-crypto, akio-tls, akio-quic, akio-http and akio-h3 and of akit-config, akit-middleware,
  akit-compat, akit-runtime, akit-listener and akit-proxy: all passing.
- Differs from fit: who the request was authenticated as is not in the access log line (`user`)
  yet: akit's line cannot carry what a request set until akio A29; nor is a request marked
  authenticated for a cache (I67). A password forgotten between `body_mode` and `call` is 503
  rather than checked.
- To measure, in the maintainer's benchmark tasks (rule 30): see the dev plan's benchmark row for
  I42.

### 2026-11-24: I32, ACME II: TLS-ALPN-01, renewal, OCSP

- **TLS-ALPN-01** (RFC 8737), fit's: `acme.challenges` takes `tls-alpn-01`, and both, HTTP-01
  first, are the default, as fit's. A challenge's certificate is self-signed on a P-256 key of its
  own, for the name alone, with the critical acmeIdentifier extension holding the key
  authorization's SHA-256 (`akio_x509::CertificateParams` with a custom extension); it is kept in
  memory while the challenge is under way (`Acme::alpn_challenge`, a `Published` map, read without
  a lock). A TLS listener that lists names offers `acme-tls/1` last in ALPN, so that only a client
  offering it alone negotiates it; such a handshake gets the challenge's certificate of a name the
  listener lists, or none (`ListenerTls::get`). The challenge tried first moves on with each
  failure in a row (fit's `attempt`), so a failed TLS-ALPN-01 is followed by HTTP-01.
- **Renewal**: by lifetime with jitter and backoff as I31 has it, and now by **ARI** (RFC 9773,
  fit's `ari.go`, `acme/ari.rs`): after each run that leaves a name with a certificate, once it is
  served, the CA's `renewalInfo` is asked about it (its ID from the authority key identifier and
  the serial number), again when `Retry-After` says (1 min to 24 h, 6 h when it says nothing;
  `Acme::set_ari_bounds` for tests); the certificate is due at a time picked at random in the
  suggested window, at once once the window has begun; a stored certificate ARI says is due is not
  taken from the storage. `Acme::renew_at` and `Acme::renewal_info` tell when.
- **OCSP stapling** (`akit_tls::Ocsp`, fit's `tlsmgr.OCSP`, `ocsp.rs`): each certificate naming a
  responder and with its issuer in its chain is kept by its chain's SHA-256, shared by the graphs
  a reload makes, dropped with its last `Staple`. Two worker threads of the control plane fetch
  responses (akio-http's client on an engine of its own, 10 s, 64 KiB, no redirect), checked with
  akio-x509's `OcspResponse::verify` before they are stapled (`CertifiedKey::with_ocsp_response`);
  fetched again halfway through their validity (an hour without a next update), failures backed
  off as ACME's, never stapled past their next update; revoked or unknown: none stapled. A
  handshake reads the stapled certificate from a `Published` map. A certificate ACME obtained is
  stapled with it, so neither is served without the other; one its responder says is revoked is
  obtained again at once, the stored one not taken. The manager owns the stapler
  (`Acme::ocsp`), so `ListenerTls::load_with` staples the listeners' certificates from files
  through it too and neither `Env` nor `akit run` changed.
- **Certificates from files**: as fit's, their responders are asked only when their chain
  verifies against trusted roots, at public addresses only (unless the roots allow private ones),
  over `http`, dialled at the address checked with the name in `Host`. fit's default roots are
  the system's, which akio-tls does not expose outside a handshake: **stopgap**, until akio A57,
  a certificate from files is stapled only under roots given (`Ocsp::trust_roots`), which `akit
  run` gives none, so it staples ACME's certificates and none from files (it says once per chain
  why).
- akio requests: **A57** (the system's roots usable outside a handshake) and **A58** (an HTTP
  service sees the negotiated ALPN, or akio closes a connection that negotiated `acme-tls/1`).
  Until A58 a client that negotiates `acme-tls/1` and then sends an HTTP/1 request is served it
  (fit closes it); the handshake has given it only the challenge's self-signed certificate.
- Config: `acme.challenges` takes `tls-alpn-01`; the default is both. The compatibility gate's
  `acme.challenges` case now reads both, in the order given; no field added, none newly refused.
- Tests: unit tests of the challenge certificate (SAN, critical extension, digest), fit's
  `TestCertsALPN` (`ListenerTls` with acme names: `acme-tls/1` offered last, a challenge served
  only to a handshake offering it alone, for a listed name), the revoked path (a stored
  certificate not loaded, obtained again, the one in memory still served), ARI's cert ID
  (RFC 9773's example), `Retry-After`, RFC 3339 and HTTP dates, windows, public addresses and
  pinned responder URLs. `akit-tls/tests/ocsp.rs` (9), fit's `ocsp_test.go` against a responder of
  the test's own: stapled, refreshed halfway, kept while the responder is down and dropped once
  expired, revoked reported, chains shared, at most two fetches at once and `close` prompt, files
  stapled under trusted roots and given back, not asked when untrusted (said once), not asked at a
  loopback address unless allowed; and stapled in a handshake to a client that asks
  (`ocsp_stapling`), not to a certificate without one. `akit-tls/tests/ari.rs` (2), fit's
  `TestARIWindowBegun` and `TestARIWindowLater` against a CA in the clear. `akit-listener/tests/acme.rs`
  (4 more, Pebble, epoll and io_uring): fit's `TestPebbleTLSALPN` (a TLS listener on Pebble's
  TLS-ALPN-01 port, no listener in the clear; the challenge's certificate gone afterwards; a
  client offering `acme-tls/1` alone then refused), `TestChallengeFallback`, a 30-second
  certificate renewed, served to a new handshake and stored, and `TestPebbleARI` (the window
  within the lifetime; moved into the past, the certificate renewed). Pebble's harness starts it
  again on other ports when one it picked was taken by a test running alongside (a flake I31's
  harness had).
- Checked: `make check` on macOS (the Pebble tests run); the tests of akit-config, akit-tls,
  akit-runtime, akit-listener, akit and akit-compat on gpu51, epoll and io_uring, with
  `$AKIT_PEBBLE_REQUIRED` set, all passing.
- Differences from the plan and from fit:
  - Files' certificates are not stapled under the system's roots until akio A57 (above).
  - `acme-tls/1` connections are not closed after their handshake until akio A58 (above).
  - fit's OCSP fetches go through the environment's proxy; akit's do not (`HTTP_PROXY` and the
    like are not read), and a responder of a certificate from files must be `http`.
  - Pebble suggests (ARI) a window covering the whole life of a short certificate, so the
    renewal test's certificate is renewed by ARI at once; the renewal by lifetime alone is
    `renew_at`'s unit test, as in I31.
  - ARI is asked once the certificate is served (fit asks before), so a CA out of reach delays no
    certificate from the storage.
  - TLS 1.2 clients are not tested for stapling (akio's client has no way to cap at 1.2; akio
    tests its 1.2 stapling itself).
- What a benchmark task should measure: full TLS handshakes a second against I31, with and
  without a stapled response (the certificate choice now reads the ALPN list and, for files with
  responders, a second `Published` map), and the size of the handshake with a response.

### 2026-11-24: I18, HTTP/2 and h2c upstreams

- `upstreams[].protocol` takes `h2c` and `h2` (`akit_config::UpstreamProtocol`); `h2` uses the
  system's roots and offers ALPN `h2` (its `tls_*` fields are I54's; `Pool::with_tls` gives a pool
  another TLS configuration, as the tests do).
- `akit_upstream::h2`: each address's h2 connections, on akio's `H2Client`. A request takes a
  stream on the first connection with one free, and a connection is dialed when none has one; the
  requests waiting for a connection being dialed get it once it is open, or its error. At most
  `STREAMS_PER_CONN` (100) streams to a connection, and `max_conns` bounds the connections through
  the streams under way to an address (`max_conns` × 100); `max_pending`, the balancers (least
  connections counts streams), passive and active health, `response_timeout`, the metrics and the
  pool's drain on close work as over HTTP/1.1. A connection that carried its last stream is kept
  for the next request, at most `max_idle_conns` of them, each for `idle_timeout`.
- Active health checks of an h2 or h2c upstream probe over HTTP/2, as fit's probe over the
  upstream's protocol: a connection of their own, closed once the head has arrived.
- `Exchange`'s bodies are akit_upstream's own now, the same for both protocols: `Upload` (HTTP/1's
  `RequestBody` or an h2 stream's body) and `Head`; `Download` holds either response. The proxy
  (`relay.rs`) is unchanged but for the types: requests to an h2c upstream go with their `Host` as
  HTTP/2's `:authority`, their fields, body and trailers as over HTTP/1.1.
- Stopgaps for what akio's h2 client lacks, each bounded and listed in the module's docs, until
  akio API requests A51–A53 (I18b): a stream given up (the client gone, a timeout, a client too
  slow) is left running with its events dropped, its connection taking no more streams and closed
  once its other streams are over (A51: no reset); `response_timeout` over h2 is checked every
  100 ms by a job of akit's timer thread per address with connections, which also closes idle
  connections past `idle_timeout` (A51: `open` has no timeout); a held h2 download goes on
  arriving and is cut off past `HELD_LIMIT` (4 MiB) more (A52: the stream window is given back at
  once); an upload over h2 is not paced, what waits in akio's queue bounded by the body limit
  (A52); request trailers to an h2 upstream are dropped (A51); 100 streams a connection, since the
  server's `SETTINGS_MAX_CONCURRENT_STREAMS` is not reported (A53); the dial is bounded by the
  engine's connect timeout, and `h2` does not fall back to HTTP/1.1 (A53). A connection that fails
  a stream as closed (after a `GOAWAY`) takes no more.
- Tests: `akit-upstream` `tests/h2.rs` (requests over h2c, streams sharing a connection and a
  second one past its 100, `max_conns` and `max_pending`, an address that refuses connections,
  idle connections closed by `idle_timeout` and `max_idle_conns: 0`, a streamed exchange, its
  timeout giving the stream up and retiring its connection, a whole request's timeout, a pool
  drained, probes over h2c taking an address out and back); `akit-proxy` `tests/h2.rs`, fit's `eachProto` tests ported for h2c upstreams from
  HTTP/1.1 and h2c clients (`TestForwarding`, `TestHopByHopRequest`, `TestForwardedUntrusted` and
  `Trusted`, `TestHostHeader`, `TestManyRequests`, `TestHead`, `TestPassiveHealth`,
  `TestStreamDownload`, `TestStreamUpload`, `TestTrailers` the response's way,
  `TestUpstreamBreaksMidBody`, `TestClientAbort`), the proxy's `timeout`, the held-download
  stopgap, and an exchange over h2 with TLS; `request_trailers` is ignored, naming A51.
  `TestResponseHeaderTimeout` and `TestResponseIdleTimeout` stay I17b's (A36).
- Compatibility gate: `examples/grpc` no longer refuses `upstreams[].protocol` (fields refused 66
  to 65; it still refuses `response_idle_timeout`).
- Not measured (rule 30): the proxy to an h2c upstream against an HTTP/1.1 one, and the
  per-address lock that leases a stream (dev plan's benchmark table).

### 2026-11-24: I36, load balancing v2

- Config: `upstreams[].load_balancing` takes fit's other six policies (`random`, `p2c`, `ip_hash`,
  `uri_hash`, `header_hash`, `cookie_hash`; `LoadBalancing::{Random, P2c, IpHash, UriHash,
  HeaderHash, CookieHash}`, `is_hash`), with `hash_key` (`Upstream::hash_key`) and
  `sticky_cookie` (`akit_config::StickyCookie`: `name`, default `fit_sticky`
  (`DEFAULT_STICKY_COOKIE_NAME`), `path`, default `/`, `max_age`, `secure`), with fit's checks and
  messages: `header_hash` and `cookie_hash` need a `hash_key` that is a token, no other policy
  takes one, and the cookie's name is a token and its path starts with `/` and holds no `;`.
- `akit_upstream::balance`, fit's `upstream/hash.go`: **random** draws by weight among the
  addresses up (all when none is); **p2c** draws two distinct addresses and takes the one with
  fewer requests under way (busy plus waiting, as `least_conn` reads them) for its weight, drawing
  again from those up for a choice that is down. Each thread draws from a wyrand generator of its
  own (seeded from std's random hash keys), so a draw writes nothing shared (rule 13).
- **Consistent hashing**: a ring of 160 points per unit of weight for each address, at fit's
  hashes (FNV-1a with MurmurHash3's finish) of `address#i`, sorted by hash then address, built when
  the config is compiled; a key goes to the first point at or after its hash, or past it to the
  first whose address is up (a ring with every address down is not walked). The keys:
  `ip_hash` the client's IP address (an IPv4-mapped IPv6 address as IPv4), hashed as text without
  formatting it into a buffer; `uri_hash` the path the route matched and the query; `header_hash`
  the first value of the header; `cookie_hash` the cookie, as fit's `cookieValue` reads it. A
  request without its key goes in turn. `Balancer::keyed`, `Balancer::key` and
  `Balancer::pick_keyed` take the addresses, hash a request's `Keys` and pick by the hash.
- **Sticky cookie** (`akit_upstream::sticky`): a request whose cookie names an address that is up
  goes there; the response to any other carries `Set-Cookie` naming the address it went to (after
  the upstream's own), its value fit's `StickyID` (the first 64 bits of the address's SHA-256 in
  hex, on akio-crypto), so akit and fit with the same addresses agree. The ids, sorted for a binary
  search, and each address's whole `Set-Cookie` value are made at compile time (rule 12).
- The pool: `Pool::pick_for(&Keys)` (the sticky cookie, then the balancer), `Pool::set_cookie`,
  `Pool::is_sticky`; `Pool::pick` keeps its meaning (no request: hashes go in turn). The proxy
  picks with the request's peer, canonical path, query and fields (as the middleware left them) and
  hands the relay the `Set-Cookie` to add, copied only when one is to be set.
- Tests: `akit-upstream/tests/hash.rs` (13), fit's `TestRandom`, `TestP2C`, `TestP2COneUp`,
  `TestPickSkipsDownNewPolicies`, `TestWeightedRandomAllDown`, `TestHashDistribution`,
  `TestHashStable`, `TestHashKeys`, `TestHashRingEdges`, `TestSticky` (on a pool with an address
  taken down by a failure) and `TestStickyNilRequest`, ported; the ring bounded; and the picks of
  fit's own `upstream` package for the same keys (ip, uri and header hashes over four weighted
  addresses, and `StickyID`), which akit's match exactly. `akit-upstream/tests/no_alloc.rs`, fit's
  `TestNewPoliciesAllocs`: no pick of any of the nine policies allocates, with a key and an address
  down, nor does a pool's sticky pick and its `set_cookie` (counted by `stats_alloc`, a
  dev-dependency, as akit-router's). `akit-proxy/tests/balance.rs` (3), fit's `TestStickyCookie`
  (responses sent whole and streamed) and `TestHeaderHash`, and uri and ip hashes end to end.
  `akit-config/tests/upstreams.rs`: the policies, fit's `TestHashedPolicyConfig`, the cookie's
  defaults and values. The gate's samples for the six new paths.
- Checked: `make check` on macOS; the tests of akit-config, akit-upstream, akit-proxy and
  akit-compat on Linux (the benchmark host), epoll and io_uring. The gate (`make compat-bless`):
  `examples/l4-udp` no longer refuses `ip_hash` and `examples/load-balancing` no longer refuses
  `sticky_cookie` (fields refused 66 to 64); each still refuses other fields. No benchmark was run
  (rule 30); what to measure is in the dev plan's status table.
- Differences from fit:
  - A ring holds at most `MAX_RING_POINTS` (2^20) points: past weights summing to 6,553, each unit
    of weight gets fewer than 160, which spreads keys a little less evenly; fit's ring grows
    without bound. Below that, keys land exactly where fit's do.
  - `uri_hash` hashes the canonical path the route matched (as sent, percent-encoded), where fit
    hashes Go's decoded `URL.Path`; keys whose paths have escapes may land elsewhere than in fit.
  - Random draws come from a per-thread wyrand generator rather than Go's `math/rand/v2`; the
    shares are the same.

### 2026-11-24: I28, listeners diffed at run time

- I22 already moved listeners on akio's runtime listeners (A1): new and changed ones added before
  the swap (an address in use fails the reload with nothing changed), removed and changed ones
  drained for `-shutdown-timeout` after it, a replacement at the same address added beside the old
  one on Linux, and the Admin API's changes (I26, I27) and `-watch-certs` going through the same
  `Listeners::reload`. I28 makes "unchanged" fit's: `crates/akit-listener/src/reload.rs` `same`
  is fit's `binding.same`, comparing the name, address, protocols, `limits.max_body_bytes`, TLS
  or not, `tls.max_version` and `client_auth.mode`, and `ListenerTls::same_config` the ALPN, h3 and
  the client authorities' contents. A TLS listener whose certificate files or `tls.acme` names
  change is kept, with its sockets and connections, and its next handshakes get the new
  certificates from its manager (I30, I31), where before it was opened anew; authorities moved to
  another file with the same contents keep it too. A kept listener now goes by the new config, which
  the next reload compares with.
- Tests, from fit's `listener/reload_test.go`: in `akit-listener/tests/reload.rs`, a keep-alive
  connection to a kept listener, open across the reloads, answered from each new graph (fit's
  `TestReload`); `listeners_added_and_removed_under_traffic_fail_no_request`, I28's test: four
  clients on a kept listener, over kept and new connections, while ten reloads add a listener and
  remove it again with the routes changed each time, with no failed request and the versions they
  see only going up. In `tests/certs.rs`, `other_certificate_files_keep_the_listener`: other
  certificate files and the same authorities from another file keep the listener (the next
  handshake gets the new certificate, a connection made before still serves), another
  `client_auth.mode` or no TLS opens it anew. fit's `TestReloadFails`, `TestReloadSameAddress` and
  `TestPlan` were ported with I22 and I27.
- akio: A22 (a UDP listener removed on a running engine) is still not in akio's main, so a
  reload that removes or changes a listener serving h3 that akit started with is still refused.
  Requested A54: akio-http closes an idle HTTP/1 connection at once when it is drained, so a
  request the client sent on it at that moment meets the close; a grace on a drain would answer it
  with `Connection: close`. Until then the e2e test's clients send such a request again, as before.
- Checked: `make check` on macOS; `akit-listener`, `akit-runtime` and `akit` tests on Linux (the
  benchmark host), epoll and io_uring. No benchmark: the hot path is unchanged (rule 30).
- Left: a startup h3 listener (A22); the keep-alive close race of a drained listener (A54).

### 2026-11-24: I31, ACME I: account, orders, HTTP-01

- `akit_tls::Acme`, fit's `tlsmgr.ACME` (fit's decision 007) with an ACME client of akit's own
  (RFC 8555): JWS on akio-crypto (`acme/jws.rs`: a P-256 account key, its JWK and RFC 7638
  thumbprint, ES256 in the flattened serialization), the CSR on akio-x509 (`CsrParams`, akio A6),
  and the CA's protocol (`acme/protocol.rs`: the directory, a bounded pool of nonces, `badNonce`
  retried with a fresh one, problem documents, `newAccount` agreeing to the terms, `newOrder`,
  authorizations, `Retry-After`, finalize, the certificate as PEM).
- Requests go through akio-http's asynchronous client on an engine of the manager's own (one
  client thread, made the first time the CA is needed): its callback only hands the response to a
  channel, and the worker waits for it in 50 ms slices, so `close` ends work under way at once.
  Responses are capped at 1 MiB, each request at `acme.request_timeout` (30 s) and each order at
  `acme.timeout` (10 min); waits between looks at an order or authorization follow `Retry-After`
  within 1 s to 30 s.
- The manager: `manage(names)` (names in lower case, without a trailing dot, at most
  `MAX_NAMES`), `acme.max_concurrent` worker threads (2) started when the first name is managed,
  each taking the name due soonest. A name is obtained once, kept in the storage
  (`certificates/<ca>/<name>.pem`, chain and key in one PEM; the account key in
  `acme/<ca>/account.pem`; `<ca>` is fit's `caKey` of the directory URL) and used from there until
  it is due for renewal: a third of its lifetime before it expires, up to a fiftieth earlier by
  its serial number (fit's `defaultRenewAt`). Instances sharing a storage lock
  `issue/<ca>/<name>` and `account/<ca>` and look again once they hold the lock. A failure is
  tried again after a minute, doubling up to an hour, less up to a tenth at random
  (`set_backoff` changes the bounds) and reported (`last_error`, and the log). Names dropped by a
  reload are no longer served; their certificates stay in the storage.
- What the I/O threads read, they read without a lock (rules 1 and 13): the certificates and the
  HTTP-01 answers are each a `published::Published` map, replaced whole by the workers and cached
  per thread by epoch as the graph cell is.
- `tls.acme`: `ListenerTls::load_with(tls, protocols, acme)` and `ListenerTls::get`, fit's
  `tlsmgr.Certs`: a handshake for a listed name gets its certificate once there is one (a client
  naming none, on a listener with no files, the first name's); otherwise the files'. A `tls` may
  list names and no certificates. `ListenerCerts` (I30) now reads the listener's `ListenerTls`
  from the current graph.
- HTTP-01 is answered by every listener in the clear: the dispatcher answers a request for
  `/.well-known/acme-challenge/<token>` (canonical path) whose challenge is under way with its key
  authorization, `200 text/plain`, before routing; any other token goes to the routes, as fit's.
- `akit run` makes one manager from the `acme` it starts with (`Env::with_acme`); every config
  applied (start, SIGHUP, the Admin API, `-watch-certs`) has it manage the names its listeners list
  (`Graph::acme_names`) once the graph is current; a changed `acme` is warned of as taking a
  restart; the manager is closed at shutdown. `akit validate` obtains nothing.
- Config: `acme` (`directory`, `email`, `ca_file`, `storage`, `max_concurrent`, `timeout`,
  `request_timeout`, `challenges`) and `listeners[].tls.acme`, with fit's checks (the directory an
  http or https URL; challenges known and once; names valid host names, no wildcard, no IP
  address, none twice; a listener requiring client certificates needs HTTP-01). `challenges`
  takes `http-01`; `tls-alpn-01` (I32), `shared_storage` and `on_demand` (I34) are refused as not
  supported yet. Ten fields more in the compatibility gate: 12 valid fixtures (`http3` now), 67
  refused only as not supported yet.
- akio: A6's CSR (branch `claude/x509-csr`) merged onto akio's main (`60547435`) as `a49aaae3` on
  akio branch `claude/i31-acme-csr`, its ADR renumbered 107 (093 is taken on main); akit is
  pinned at it. **Not pushed**: to build, fetch akio from a checkout that has the commit (or push
  it to akio's main first).
- Tests: unit tests of the JWS (signed, then verified with the public key; RFC 7638's
  thumbprint), problem documents, `caKey`, the renewal time, the backoff and `Published` (reads
  across threads, nested reads). `akit-config/tests/acme.rs` (6): defaults and values, what is
  refused and what is not supported yet, one file only, `tls.acme` with and without certificates,
  fit's checks of the names. `akit-listener/tests/acme.rs` (2, on epoll and io_uring), against
  Pebble v2.10.1 as fit's `internal/pebbletest` runs it (`support/pebble.rs`; skipped without its
  binaries, which `$AKIT_PEBBLE_BIN` points to): a listener in the clear on Pebble's HTTP-01 port
  and a TLS listener with `tls.acme` and no files; no certificate before, then one obtained and
  served to a handshake verified against Pebble's root (in any case of the name); the challenge
  path back to the routes afterwards; a new manager on the same storage, with Pebble stopped,
  serving the same certificate; a name dropped no longer served. And a CA out of reach: the
  failure reported, tried again, `close` prompt.
- Checked: `make check` on macOS (the Pebble tests run, with `$AKIT_PEBBLE_BIN` set); the tests of
  akit-config, akit-tls, akit-runtime, akit-listener, akit and akit-compat on gpu51, epoll and
  io_uring, with `$AKIT_PEBBLE_REQUIRED` set, all passing.
- Differences from the plan and from fit:
  - TLS-ALPN-01, OCSP stapling and ARI are I32; on-demand TLS I34; shared storages I62. fit's
    default `challenges` (HTTP-01 then TLS-ALPN-01) is HTTP-01 alone until I32.
  - The default storage is `$XDG_DATA_HOME/akit` or `~/.local/share/akit` (fit's: `.../fit`).
  - The manager's threads are akit's own (the control plane), not akio's logical pool: they wait
    on the CA for seconds, and on the storage's locks.
  - ACME's certificates are not yet in the metrics' certificate list (I37 lists the files').
- What a benchmark task should measure: the dispatcher's HTTP-01 check on listeners in the clear
  (a prefix comparison per request when `akit run` has a manager): fixed over HTTP/1.1 and h2c,
  same-run A/B against main before I31.

### 2026-11-24: I27, Admin API II: patch, rollback, auth

- Config: `admin.token_file` read (`Admin::token_file`; empty is none). An address off loopback is
  still refused: fit asks for TLS there too (`cert_file`, `key_file`), which akit does not serve
  the API over yet; the message says so.
- `akit-admin`: `PUT`, `PATCH` (`application/merge-patch+json` or `application/json`) and `DELETE
  /config[/<pointer>]`, `POST /rollback/{n}` and `POST /reload`, as fit's: a value set, added or
  appended (`-`), merged as RFC 7396, removed; the config itself cannot be deleted; a whole
  config put is kept as written, an edited one written out again as fit writes it
  (`Value::to_json`). Bodies streamed in as a load's are, under `max_config_bytes` (413, 415);
  the pointer and `?dry_run` checked on the I/O thread, the change handed to the control thread as
  a `Job` (now an `Op`: load, put, patch, delete, rollback, reload). `Configs::change` makes the
  change from the running config under the lock that orders applies, after `If-Match` (412 with
  nothing called), so concurrent writers lose nothing; `Configs::update` is it with fixed data.
  `Refused::Request` carries a pointer's 400/404 and "version n is not kept"; `SOURCE_ROLLBACK`
  (`rollback`) for a rollback, `admin` for a reload, as fit's. `pointer.rs` gains `put`,
  `remove`, `merge_patch` and `lookup_mut`, with fit's messages.
- Dry runs (`?dry_run=1` or `true` on `POST /load`, `PUT`, `PATCH`, `DELETE` and `POST /reload`;
  anything else but `0`, `false` or none is 400): `Mode::DryRun` takes the apply path without its
  effects; the answer has `dry_run`, the running version and ETag, `new_etag`, `unchanged`, the
  listeners an apply would add and remove and its warnings, and `Fit-Dry-Run: 1`
  (`DRY_RUN_HEADER`); a refused one answers word for word as the apply would.
  `akit_listener::Listeners::plan` (new) says what `reload` would open and close without changing
  anything, resolving a new listener's address and binding it for a moment unless a running
  listener holds it; `akit run`'s `Runner::check` compiles with an empty engine slot and a
  registry of its own and calls it, and counts no reload.
- The token: read when the API starts (trimmed; empty, or unreadable, fails the start); every
  request then carries `Authorization: Bearer <token>`, compared in constant time, or gets 401
  with `WWW-Authenticate`; the loopback `Host` check is dropped with a token, `Origin` still
  refused, as fit's guard.
- Tests: `akit-admin/tests/admin.rs` (21, 11 new), fit's `edit_test.go` and `dryrun_test.go` and
  `TestToken` ported: edits by pointer with fit's expected config, edits refused (an invalid
  address with fit's line and column, a module that does not exist, unknown field, not JSON, 404
  pointers, 415, 405, the root not deletable), `If-Match` stale for every kind of change, 16
  concurrent writers (8 with `If-Match` retrying on 412, 8 appending without) each applied once,
  rollback (as a new version, `rollback` source, a version gone, `x`), reload, dry runs (answers,
  headers, the same refusals as an apply, a taken address, bad `dry_run`, then applied with the ETag
  the dry run gave), a dry run that removes a listener and frees nothing, the token, an edit's body
  under, at and over the limit and cut off midway; unit tests of the edit tree (fit's
  `TestEditTree` and `TestPointer`), `Configs::change` and the token comparison.
  `akit-listener/tests/reload.rs`: `Listeners::plan`. `akit-config/tests/admin.rs`: `token_file`.
  `akit/tests/admin.rs`: `akit run` with a token, an edit, a dry run and a rollback end to end.
  The gate's sample for `admin.token_file`.
- Checked: `make check` on macOS; the tests of akit-admin, akit-config, akit-listener, akit and
  akit-compat on Linux (the benchmark host), epoll and io_uring, all passing (an earlier epoll run hit the known flake in `akit-listener`'s `unchanged_listeners_are_kept_and_new_ones_opened_and_closed`).
- Differences from fit:
  - No TLS for the API (`cert_file`, `key_file`, `client_ca_file`), so no address off loopback,
    token or not; no Unix socket yet (A23).
  - A 401 does not close the connection as fit's does (akio API request A24, as the 413).
  - fit's connection limiter (a quarter of `max_conns` kept for clients that showed the token, an
    unauthenticated connection closed after 5 s, a share per peer) is not ported: the engine's
    `max_connections` stays the limit.
  - A body decoded for an edit takes akit's JSON reader, which allows comments and trailing
    commas where fit's `encoding/json` refuses them.
  - akit-config takes a config without listeners, which fit refuses: `TestDryRun`'s first refused
    case is left out.
  - fit's `/config/text`, `/node`, the store and the console are not in akit.
- Benchmark: none (rule 24: the control plane).

### 2026-11-24: I17, reverse proxy III: timeouts, cancellation, trailers

- Config: the upstream's `response_timeout` (`Upstream::response_timeout`, fit's 1h when left out
  or 0, `akit_config::DEFAULT_RESPONSE_TIMEOUT`) and the proxy's `timeout`
  (`Proxy::timeout`, none when left out or 0) are read; `response_header_timeout` and
  `response_idle_timeout` stay refused as not supported yet (A36).
- `akit-upstream`: a pool bounds each exchange by its upstream's `response_timeout`
  (`Pool::response_timeout`); a request's own timeout bounds it too, the shorter holding (it used
  to replace the pool's). `Exchange::gave_up`: an exchange that gave its request up before the
  response (its client gone) is not counted against the address.
- `akit-proxy`, as fit's `exchange`:
  - Timeouts: a connect that times out (`ClientError::Io` of kind `TimedOut`, as akio reports
    `connect_timeout`) is answered 504, as the request timeouts already were. The proxy's
    `timeout` goes on each request, and the pool takes the shorter of it and `response_timeout`:
    past it before the head, 504; after, the response is cut short.
  - Cut short (by the timeout, or an upstream that broke off): an HTTP/1 client's connection is
    closed, an HTTP/2 or HTTP/3 stream of a declared length reset, and one of no declared length
    ended with fit's `Fit-Error: upstream failed mid-body` trailer (`relay::BROKEN_TRAILER`).
  - Cancellation: a client gone mid-download was already noticed (the next write fails; a held
    download hears it at once, as akio calls `on_drain` on a close). A client gone mid-upload
    over HTTP/2 or HTTP/3 drops its body's sink (`Sink`'s `Drop`, before `End`), which drops the
    upstream request's unfinished body (closing its connection), and the exchange says it gave
    up, so the address is not counted as failing.
  - Trailers, as fit's: the names a request announces (`headers::trailer_names`, fit's
    `trailerNames`) go up in its `Trailer` and only those trailers follow
    (`headers::announced`); a request that announces trailers goes up chunked, one whose body
    came whole too (written up as a stream from `Client::preload`), and one that announces none
    sends none. A streamed response keeps the upstream's `Trailer` announcement; a whole one
    (within 64 KiB) that ends with trailers is streamed, announced, so that HTTP/1.1 carries them
    (akio sends a whole response's trailers on HTTP/2 alone).
- Tests: `akit-proxy/tests/failure.rs`, fit's `failure_test.go` ported: `connect_timeout` (a
  listener whose backlog is full; skipped where a full backlog refuses, as on macOS),
  `timeout_before_head` (`response_timeout`, `timeout`, and the shorter of both: 504 after it, the
  upstream's connection closed), `cut_short_mid_body` (`TestResponseIdleTimeout` with
  `response_timeout`, and `TestUpstreamBreaksMidBody` over h2c: HTTP/1.1 bodies end short, h2c
  ones of a length fail and those of none end with `Fit-Error`), `client_abort` (mid-download
  over HTTP/1.1 and h2c, mid-upload over h2c: the upstream sees its connection closed, and its
  failure count stays 0), `trailers` (`TestTrailers`: announced request trailers up, unannounced
  ones dropped, response trailers back over HTTP/1.1 and h2c, small and streamed). Unit tests of
  `trailer_names` and `announced`, `status_of` for I/O timeouts, the config fields and their
  errors, the pool's `response_timeout` against a request's own, and `forwarding` checks that a
  GET without a body goes up without framing. The compat gate's samples cover the two fields.
- Checked: `make check` on macOS; `akit-proxy`, `akit-upstream` and `akit-config` tests on Linux
  (gpu51) with epoll and with `AKIO_TEST_BACKEND=uring`.
- Differences from the plan and from fit (I17b, both plan files):
  - fit's `response_header_timeout` and `response_idle_timeout` need per-request header and idle
    timeouts in akio's client (akio API request A36): `TestResponseHeaderTimeout` is not ported,
    and `TestResponseIdleTimeout` is ported with `response_timeout` in its place.
  - A client that goes before the response's head (with its body all sent), and an HTTP/1 client
    that goes mid-upload, go unnoticed until the upstream answers or `response_timeout` passes:
    akio's client has no handle to cancel a request before its head (A37), a detached server
    request hears nothing of its client going, and akio's HTTP/1 session keeps a streamed
    request's body sink while the proxy's pending answer holds the connection (A38, found by
    `client_abort`). `TestClientAbort`'s "before the header" case and its HTTP/1 upload are not
    ported.
  - fit's `limits.write_stall_timeout` stays refused: akio's engine `write_timeout` is the same
    bound but cannot change on reload (A38); akio's default (60s) bounds a client that stops
    reading meanwhile.
  - A request's trailers are tested over HTTP/1.1 alone: akio's h2 client sends none.
- For a benchmark task: nothing on the hot path changed but one `Trailer` lookup per proxied
  request and one more field in the shared state; the proxy scenario's row in the
  [benchmark table](dev-plan.md#benchmark-and-optimization-status) names requests with trailers.

### 2026-11-24: I41, rate, concurrency and size limits

- Taken early from M3, after I40. `akit-config` reads fit's `rate_limit`, `concurrency_limit` and
  `body_limit` middleware (`middleware/limit`) into `Handler::RateLimit`, `ConcurrencyLimit` and
  `BodyLimit`, with the checks of fit's `Provision`s and their messages and fit's defaults (burst:
  the rate rounded up; key `ip`; `ipv6_prefix` 64; `max_keys` 100000; status 429); a decoder of
  JSON numbers for `rate`. They leave `FIT_MIDDLEWARE`.
- `akit-middleware` `limit`:
  - `rate_limit`: a token bucket a key, refilled at `rate` a second up to `burst`, in nanotokens
    as fit's; a request with no token is answered `status` with `Retry-After` (whole seconds,
    rounded up). Keys: the client's address (an IPv6 one cut to `ipv6_prefix` bits; behind
    `trusted_proxies`, the last `X-Forwarded-For` address that is not one, walked as fit's
    `clientIP` without allocating), a header's first value by its keyed hash, or the route. The
    buckets are fit's: 64 shards by the key's hash, each a map under a lock of its own, shared by
    the I/O threads (sharding per thread would let a client on many connections take the rate as
    many times), at most `max_keys` keys, a new key past it evicting one whose bucket is full
    among 8 it looks at, else the first of them. A lookup allocates nothing: the key is the
    address or a 64-bit SipHash with a random key (fit keeps values of up to 128 bytes as they
    are and hashes longer ones), and the shard's map takes that hash as it is.
  - `concurrency_limit`: one atomic count of the route's requests under way; one past `max` is
    answered `status` at once. A request's place is a guard in a response filter: given back
    when the response is sent, or when the request ends without one (the filter dropped with the
    request or its `Detached`).
  - `body_limit`: a request that declares a body past `max_bytes` is answered 413 (fit's
    `Request Entity Too Large`) before its handler runs, its body taken as a stream so that none
    of it is buffered; a buffered body past it likewise once it has arrived; one that streams to
    its handler with no declared length has the listener's `MAX_BODY_BYTES` local lowered to the
    limit, which the proxy holds it to (413 when it grows past).
- Stopgaps, until akio can (rule 21; in [akio API requests](akio-api-requests.md)):
  - A48: akio tells a service neither that a response was sent in full nor that a client went
    while its request waits. So a streamed response (the proxy's past 64 KiB) gives its
    concurrency place back when its head is sent, and a request whose client went holds it until
    its handler lets the request go (the proxy's upstream answers or fails). The response filter
    is a boxed closure with an `Arc` clone, per request.
  - A49: a middleware cannot bound a streamed body it does not read. A handler that streams
    bodies holds them to `MAX_BODY_BYTES` (the proxy does; it is the only one); a 413 does not
    close the HTTP/1 connection, whose rest of the body akio reads and drops (A24).
- Tests: `akit-middleware` unit tests on a clock the test moves: fit's `TestRateBucket`,
  `TestRateSlow`, `TestRateKeys`, `TestClientIP`, `TestRateMemoryBounded`,
  `TestEvictPrefersFull`, `TestRateIPv6Prefix` and `TestPeerAndName`, ported, and the extremes of
  rate, burst and clock (rule 15). `tests/limits.rs` through akit's listener: a burst, then 429
  with `Retry-After: 100`; keys by header with status 503; fit's `TestConcurrency` (inline
  answers give their place back; at the limit one more is refused; one answered later, or let go
  unanswered, makes room); fit's `TestConcurrencyClientGone` as akit has it; fit's `TestBody` and
  `TestBodyOverNetwork` (under, at and over, by length and chunked, to a handler that takes the
  body whole and to the proxy, which streams it); fit's `TestParamErrors`.
- Compatibility gate: the 16 fields in `tests/compat/supported.txt`; `make compat-bless`:
  `examples/rate-limit` and `examples/body-limit` are valid (80 fixtures: 13 valid, 66 refused
  only as not supported yet, 1 skipped, none failed).
- Checked: `make check` on macOS; on the benchmark host, the touched crates' tests (`cargo test
  -p akit-middleware -p akit-config -p akit-compat`) with epoll and with
  `AKIO_TEST_BACKEND=uring`.
- Differs from fit: a header value used as a key is kept by its 64-bit keyed hash whatever its
  length; a refused body of a declared length past the listener's `max_body_bytes` too is
  answered by the listener (akio's `Payload Too Large`) before the middleware sees it.
- To measure, in the maintainer's benchmark tasks (rule 30): see the dev plan's benchmark row
  for I41.

### 2026-11-24: I35, active health checks

- Config: `upstreams[].health_check` and its ten fields (`type`, `path`, `host`, `port`,
  `expect_status`, `interval`, `timeout`, `jitter`, `fails`, `passes`) are read into
  `akit_config::HealthCheck` (`Upstream::health_check`, `HealthCheckType`,
  `DEFAULT_HEALTH_INTERVAL` 5s, `DEFAULT_HEALTH_TIMEOUT` 2s, `DEFAULT_HEALTH_THRESHOLD` 2; the
  jitter a tenth of the interval when left out; 0 means the default, as fit's `orDefault`), with
  fit's checks and messages: the http fields refused on a tcp check, a path starting with `/`, a
  host name or IP address (no wildcard), a timeout no longer than the interval, a jitter less than
  it (reported at the check's object when the field is left out, as fit does).
  `akit_config::duration::format` prints durations as Go does, for those messages.
- `akit_upstream::probe`, fit's prober on akio's clients: each address is probed every `interval`,
  plus a random jitter, from the start of one probe to the start of the next; one address's probes
  follow one another. **http**: `GET path` with `User-Agent: akit-health-check` and the check's
  `Host`, through an akio-http `Client` of the prober's own that keeps no idle connection (a probe
  sees whether the address takes new connections and never waits behind the pool's requests); the
  status is read from the head and the body is given up (`ResponseBody::cancel`); 200 to 399 pass,
  or what `expect_status` lists; the request's timeout (armed by akio from the start, connecting
  included) is the check's. **tcp**: passes once a connection opens (`HappyEyeballs::connect`,
  which resolves names on akio's resolver), closed at once; the timeout is kept by the prober's
  timer, since akio dials a lone address with its engine's connect timeout (A12). `port` probes
  another port of each address's host.
- State: fit's `record`: `fails` failures in a row take an address out
  (`Health::set_probed_down`), `passes` passes in a row put it back, a result agreeing with the
  state ends a run against it. `Health` keeps the passive and the active state apart, as fit's
  `storeDown`: `down_until` is `u64::MAX` while probes keep the address out, else the passive
  window's end; a response clears the failures, not the probes. Picks read the same one atomic as
  before, so the balancers, `Pool::is_down`, `available` and the metrics' `backend up` gauge see
  both. The pool adds `health_check()` and `is_probed_down(i)` (fit's `ProbedDown`).
- Lifecycle: the probes start when the pool's engine is there: `EngineSlot::set` runs what pools
  made while it was empty registered (`akit run` makes its pools before its engine), and a pool
  made with a filled slot starts at once. The prober holds its pool weakly: once the pool is closed
  (its graph retired) or dropped, no probe starts and the result of one under way is dropped. A
  reload's new pool starts with every address in rotation, as I20's passive state does (fit
  carries both over a reload; akit's pools do not share state across graphs yet).
- Threads (rule 1): no thread waits for a probe. Each is started by a job of
  `akit_upstream::timer`, a stopgap for akio API request **A45** (a public timer; akio's are
  internal at the pinned commit): one thread of akit's own, a heap of pending jobs bounded at
  65,536 (each probed address has at most two: its next probe and its probe's timeout), whose jobs
  only start a probe or fail one. Results arrive on akio's callbacks (an I/O thread, the resolver's
  or a timer's), which record them and schedule the next probe.
- Tests: `akit-upstream/tests/probe.rs` (6), fit's `record`, `TestProberEdges` and e2e
  `TestHealthCheckRotation` ported onto pools on real sockets: an address whose probe path hangs
  leaves rotation within `fails × interval + timeout + interval` and is sent nothing while out,
  and comes back within `passes × interval + interval`; http probes' `Host`, `User-Agent`, default
  and listed statuses, and a huge body not read (its connection closed); a tcp probe of the
  check's `port`, out once it refuses and back once it listens, the address's own port still
  serving; a refused tcp probe; a closed pool's probes stop; probes wait for the engine slot; a
  pool without a check never probes; probes and failures kept apart in `Health`.
  `akit-config/tests/health_check.rs` (4): defaults, every field, fit's "health checks" case with
  its positions, and bad values. The gate's samples for the eleven paths.
- Checked: `make check` on macOS; the tests of akit-config, akit-upstream, akit-compat and akit-runtime on Linux
  (the benchmark host), epoll and io_uring. The gate (`make compat-bless`): `examples/grpc`,
  `examples/l4-tcp`, `examples/load-balancing`, `examples/proxy` and `examples/webrtc-proxy` no
  longer refuse `health_check` (fields refused 72 to 71); each still refuses other fields.
  No benchmark was run (rule 30); what to measure is in the dev plan's status table.
- Differences from fit:
  - Probes are HTTP/1.1 over plain TCP, as the pools are: fit's probes follow the upstream's
    protocol and TLS, which come with I17 and I18; their probes must then follow too.
  - The probes' timer is akit's own thread until akio A45; a probe's timeout job stays pending
    when the probe ends first (it then does nothing), since the stopgap cannot cancel.
  - Probed state does not survive a reload (nor does the passive state, I20).

### 2026-11-24: I37, Prometheus metrics

- `akit_observe::metrics` (std only), fit's `observe` metrics: `Histogram` (fit's
  `DurationBuckets`, sharded by thread), `TextWriter` (Prometheus's text format 0.0.4, fit's
  escaping and number format, `CONTENT_TYPE`), `Metrics` (the registry), `Lease` (what one graph
  takes from it), `UpstreamStats` and `BackendCounters`, `ConfigStats` (reloads, version, time
  applied).
- Requests by listener and route, as fit counts them: each `(listener, route)` a graph serves
  takes a slot when the graph is compiled (`Lease::route`; the route's name, or `#` and its index,
  or empty for the requests no route takes, fit's `metricsName`) and gives it back when the graph
  is retired (a module cleanup, `akit_runtime::METRICS`), so a route a reload keeps keeps its
  counts and one it removes is no longer scraped; the counts of a slot given back stay in
  `Metrics::totals`. The counters are kept per thread (`Metrics::shard`, as many shards as the
  host's threads up to 64, slots allocated 64 at a time on a thread's first use): no lock and no
  shared reference count on the request path (rule 13). At most 65,536 pairs are counted at once
  (rule 11).
- The listener (`Dispatcher`) counts a request answered before its handler returns (a handler
  whose `Handler::inline` says so, fit's `module.Inline`: `respond`; the 404 and the 400) once the
  call returns, from `Context::status`, with no hook and no allocation; any other request by a
  response filter on the thread that sends its response, the access log's own when there is one
  (one boxed closure for both: akio API request A29 removes it).
- Upstreams, as fit's: a pool counts its response times (`Pool::response_times`) and each
  backend's exchanges over and failed (`backend_requests`, the same outcomes that count towards
  passive health), and gives each backend's requests not over (`backend_active`). Through
  `Lease::upstream` the counters are shared by upstream name and address with every graph that
  has the upstream, so they carry over reloads (akit makes a graph's pools anew; fit shares the
  pools).
- Certificates: `akit_tls::CertificateInfo::of` gives a certificate's name (its first DNS name,
  else its common name), serial (as Go's `Text(16)`) and expiry; the scrape reads them from the
  running graph's certificates (`Graph::certificates`, I30's `Certs`), so a renewal the
  certificate watcher applies shows at once.
- Config: `metrics` (`{}`, no settings) is read into `Config::metrics`; without `admin` it fails
  with fit's message, "the Admin API serves metrics: set admin too".
- `akit_listener::metrics::Scrape` writes the scrape, fit's families in fit's order with fit's
  help: `fit_http_requests_total`, `fit_http_request_duration_seconds` (with `metrics`),
  `fit_upstream_requests_in_flight`, `fit_upstream_response_duration_seconds`,
  `fit_upstream_backend_requests_total`, `fit_upstream_backend_requests_in_flight`,
  `fit_upstream_backend_up`, `fit_tls_certificate_not_after_seconds`, `fit_connections_open`,
  `fit_connections_refused_total`, `fit_pending_bytes`, `fit_reads_paused_total`,
  `fit_config_reloads_total`, `fit_config_version`, `fit_config_last_applied_timestamp_seconds`,
  `fit_access_log_dropped_total`; then akio's engine metrics (`akio_*`, per I/O thread) where fit
  has the Go runtime's (`go_*`). `akit run` makes one registry for the process and keeps it,
  with the count of configs applied (`ConfigStats`), in I26's `Runner`: every config goes through
  `Runner::apply` (or fails before it, in `reload` or `apply_text`), so SIGHUP, Admin API loads and
  I30's certificate reloads are all counted. The Admin API serves the scrape at `GET /metrics`
  (`akit_admin::Options::metrics`).
- Tests: `akit/tests/metrics.rs`, fit's `TestMetrics` and `TestMetricsOff` ported (the scrape
  parsed as fit's test parses it, every sample fit checks with its labels and value: requests by
  route and class through a reload that keeps the routes, the unmatched 404, the durations, the
  upstream's response times, each backend's successes and failures, in flight and up, the TLS
  listener's certificate, reloads and version, the reads paused by budget, the gauges and akio's
  families present, a renamed route dropped once the old graph is retired, the totals kept), and
  the access log and the metrics sharing one hook; `akit/tests/admin.rs`: `akit run` with `admin`
  and `metrics` serving `GET /metrics` (content type, requests, a SIGHUP, an Admin API load and a
  refused one counted). Unit tests of buckets, classes, number format,
  escaping, slots carried over, given back and reused, shared upstream counters, config stats
  (`akit-observe`), serials (`akit-tls`); `akit-config/tests/metrics.rs` (needs admin, no
  settings, one file only); the gate's sample.
- Merged with I26, I30 and I40 (akit main `4afa34a` with I30, `131908f`).
- Checked: `make check` on macOS; the workspace tests on Linux (the benchmark host), epoll and
  io_uring, all passing but two flakes that are not I37's: on epoll `akit-proxy`'s `bad_gateway`
  and `hop_by_hop_request` failed together once (the port `bad_gateway` freed was taken by the
  other test's upstream; 14 of 14 when rerun), and on io_uring I22's
  `reloads_under_traffic_with_no_failed_request` (`akit/tests/reload.rs`) failed 1 run in 5, as
  it does on the merged base `131908f` without I37 (1 in 4 there). The gate (`make compat-bless`): `metrics` supported; `examples/metrics` is valid (80
  fixtures: 11 valid, 68 refused only as not supported yet, 1 skipped, none failed).
- Differences from the plan and from fit:
  - Round robin keeps its turn per I/O thread in akit, so the ported test sends proxied requests
    until both backends were tried, and checks the counts it saw, where fit's sends exactly one to
    each.
  - `fit_connections_open` counts the upstream connections akit dials on the listeners' engine,
    and UDP peers, as akio's `open_connections` does (akio API request A30). `reason="budget"` of
    `fit_reads_paused_total` is 0: akit has no `limits.max_pending_bytes` yet.
  - An upstream's response time is to the whole response, read by akio's client, until it streams
    (A19–A21); fit times the head.
  - A request in flight across two reloads, the first removing its route and the second giving
    its slot to another pair, is counted under that pair (fit's garbage-collected counters have no
    such window).
  - No trace spans (`fit_trace_spans_total`: I64), no apps', vod's or websockify's metrics, which
    akit does not have yet.
- akio API requests: A29 (a response hook without an allocation) and A30 (the open connections a
  listener accepted).
- What a benchmark task should measure: same-run A/B of the fixed and proxy scenarios with
  `metrics` on and off, and with the access log on as well; a scrape's time with many routes on 64
  threads.
### 2026-11-24: I30, certificate manager and storage

- The manager is akio-tls's `CertResolver` (akio A2, ADR-098), what fit's `tlsmgr.Manager` is to
  Go's `GetCertificate`: asked for each full handshake's certificate, it answers in the call from
  memory (or later from another thread, which on-demand TLS, I34, will use). Every TLS listener's
  configuration is now built around one (`ServerConfig::builder_with_resolver`) instead of
  I09's `sni_cert` table.
- `akit_tls::Certs`, fit's `tlsmgr.Static`: a listener's certificates (`CertifiedKey`s) indexed by
  the names in their leaves (DNS names in lower case, IP addresses) and by the parent of each
  `*.` name. A handshake gets the first certificate for its name, exactly or by a wildcard one
  label deep, that the client can take (its signature schemes and, in TLS 1.2, an ECDHE suite of
  the key's kind, as Go's `SupportsCertificate`), else the first for the name, else the first of
  all; one certificate serves every name. So a wildcard in any certificate now works (I09 refused
  one past the first), and of an RSA and an ECDSA certificate for a name the client gets the one
  it supports, as in fit (I09 served the first).
- `akit_tls::ListenerTls`: a listener's `tls` loaded once (certificates, client authorities,
  protocols) with the SHA-256 of every file read; `server_configs(manager)` builds its TCP and h3
  configurations around a manager; `same_config` compares all but the certificates.
  `server_config` and `server_configs(tls, protocols)` are kept, serving the loaded certificates.
- Reloads without dropping handshakes: `akit_runtime::ListenerCerts`, the manager of each TLS
  listener, serves the certificates the cell's current graph loaded for the listener (by name),
  reading the graph through the I/O thread's cache as a request does, with no lock. A reload whose
  listener has the same config and the same client authorities (their files' contents included)
  keeps the listener, its sockets and connections, and the next handshake gets the new
  certificate; a handshake under way keeps the one it was given. A listener opened by a reload
  serves its own graph's certificates until the swap; one being drained serves its own when the
  current graph no longer has it. New client authorities reopen the listener (beside the old one,
  which drains), since a configuration holds them. This fixes I22's kept TLS listener keeping its
  old certificates.
- `run -watch-certs` (on by default, as fit's) and `-watch-interval` (1s): `akit_tls::FileWatcher`,
  fit's `tlsmgr.FileWatcher`, looks at the size and modification time of every certificate, key
  and authority file the running config loaded (`Graph::cert_files`) on the main thread's loop,
  and once a change has settled (500 ms) and the contents differ, applies the running config again
  as a new version with source `certificates` (`akit_admin::SOURCE_CERTIFICATES`), through
  `Configs::update` as SIGHUP and the Admin API do, as fit's `Configs.Reapply`
  (`akit: certificate files changed; reloading`, then `akit: reloaded certificate files (config
  version N)`). `Runner::apply` keeps the files the running graph loaded, whichever way it came. A pair that fails (a key not written yet)
  is reported once and not tried again until the files change; an unreadable file is reported
  once. `-watch` (the config file, I29) is still refused.
- Storage: `akit_tls::Storage`, fit's `tlsmgr.Storage` (load, store, delete, list, lock, unlock),
  with a deadline in place of a context; its methods block and are called off the I/O threads.
  `FileStorage`, fit's `FileStorage`: a file per key under a root (keys checked: no empty, `.` or
  `..` part, no backslash or NUL), written to a temporary file, synced and renamed; values up to
  4 MiB; directories 0700 and files 0600; `list` sorted, skipping locks and temporary files.
  Locks are files under `locks/` made with `create_new`, touched every 5 s by a thread per lock
  while held, removed when untouched for 30 s (a holder that stopped), polled every second by a
  waiter until its deadline; at most 256 held at once (rule 11). ACME (I31) is its first user;
  `acme.storage` and `shared_storage` stay refused until then.
- Tests: `akit-tls` unit tests (the choice of a certificate as fit's `TestStatic`: exact,
  wildcard, one label only, order, the client's schemes; Go's support rules); `tests/storage.rs`
  (4, fit's storage tests: keys, values, sizes, permissions, list, deadlines, a lock taken in turn
  by two storages on one directory, a fresh lock waited for, a stale one taken over);
  `tests/watch.rs` (2, fit's watcher tests: a renewal applied once, touches and changes back
  ignored, a failing pair reported once, an unreadable file, a config applied another way);
  `tests/files.rs` (a wildcard in any certificate, file sums and `same_config`).
  `akit-listener/tests/certs.rs` (3, on epoll and io_uring): wildcards in any certificate by SNI;
  a certificate replaced under traffic (four clients making full handshakes all along, a reload
  that keeps the listener, the new certificate served after it, no failed request, a connection
  made before still serving); new client authorities reopening the listener.
  `akit/tests/watch_certs.rs` (2, end to end with `akit run`): renewed files served without a
  restart and with the listener kept, a key not yet written failing once and then applied; and
  `-watch-certs=false`. `cli.rs`: bad `-watch-interval` values exit 2.
- Checked: `make check` on macOS; the tests of akit-tls, akit-runtime, akit-listener and akit on
  Linux (the benchmark host, gpu51), epoll all passing, io_uring all passing but
  `akit-listener/tests/reload.rs` (plain HTTP, a read timing out in its `get`), which fails as
  often on main `1a5fccf` (2 of 5 runs each) on the host at a load average of 200 to 480: a flake
  under load that predates I30. After merging main (I26, I40): `make check` on macOS, and the
  tests of akit and akit-listener on gpu51 (load average about 10), epoll and io_uring, all
  passing, `reload.rs` included.
- Differences from the plan and from fit:
  - fit's `Certs` (ACME and files together), OCSP stapling (`Staple`) and the shared storages
    (Redis, etcd, Consul) are I31, I32 and I62; the trait is ready for them.
  - The watcher runs on the control thread's loop (`recv_timeout` on I26's `Control` channel),
    not a goroutine of its own; the reloads it starts, SIGHUP's and the Admin API's are one after
    the other there.
  - A session resumed by a client sends no certificate, so it keeps the certificate of its first
    handshake until its ticket expires, as with Go.
- What a benchmark task should measure: full TLS handshakes a second on a listener with one and
  with ten certificates, against I09's `sni_cert` (main before I30) and fit; the cost of the
  choice per handshake (a graph read and an `Arc` clone).

### 2026-11-24: I40, rewrites, redirects, headers

- Taken early from M3, after I39. A route's `middleware` is read: `akit-config` decodes each
  module reference (`module`, `params`) into `Route::middleware`, the middleware variants of
  `akit_config::Handler` (`Rewrite`, `Redirect`, `Headers`: one params type for handlers and
  middleware, as akit-runtime's registry provisions every module from one), with the checks of
  fit's `Provision`s and their messages; fit's other middleware modules are refused as not
  supported yet, naming the module (`FIT_MIDDLEWARE`). `akit-runtime`'s `compile` loads each
  route's middleware from the registry, errors at `routes[i].middleware[j].params.<field>`, and
  chains them around the handler once (rule 12), the first outermost. The binary's registry has
  `akit_middleware::MODULES`.
- `akit-middleware` has fit's `rewrite`, `redirect` and `headers` (`middleware/rewrite`,
  `middleware/headers`):
  - `rewrite`: `strip_prefix` (the first prefix the path starts with at a segment boundary) and
    `add_prefix`, on the decoded path, the new path escaped afresh as Go writes it
    (`/api/a%2Fb` stripped of `/api` is `/a/b`; `!` is `%21`). The new path is set in
    `CANONICAL_PATH`, which the `proxy` handler sends upstream (with the query as it came) and
    the `static` handler looks up, and which a middleware after it goes by.
  - `redirect`: `to` with `{scheme}`, `{host}`, `{path}`, `{query}` and `{uri}`, split into its
    parts when the config is compiled; `status`; `regexp` (`akio-regex`, compiled as the router
    compiles path regexps) matched against the escaped path as Go's `URL.EscapedPath` gives it
    (a raw `\` is `%5C`), the other requests going on to the handler; fit's open-redirect check
    (`otherHost`, `hasScheme`), answered `400 bad redirect target`.
  - `headers`: `request` and `response`, each `delete` (names, or prefixes ending in `*`), `set`
    and `add`, in that order, names made canonical as Go's `CanonicalHeaderKey`. The response's
    changes are an akio response filter, so they reach every response of the route: answered at
    once, later from a completion callback (a proxied one) or streamed.
- Stopgaps, until akio can (rule 21; both in [akio API requests](akio-api-requests.md)):
  - A32: `akio-regex` answers whether a pattern matches, not where, and captures nothing. So the
    rewrite's `regexp` and `replacement`, and groups of the redirect's regexp in `to` (`$1`,
    `${name}`), are refused as not supported yet when the config is read; `$$` is `$`.
  - A33: akio's `Context` gives the request by shared reference only. The `headers` middleware's
    request changes are kept in a context local (`akit_runtime::REQUEST_FIELDS`, written by
    `set_request_headers`), which `akit_runtime::request_headers` reads and the `proxy` handler
    sends upstream; what akio itself reads of the request (the `static` handler's conditional and
    range fields) is as the client sent it. The path rewrite goes through `CANONICAL_PATH`, as
    I11's canonical path does.
- Tests: `akit-middleware` `tests/middleware.rs` through akit's listener: fit's `TestRewrite`,
  `TestRedirect`, `TestRedirectHeaderIsItsOwn`, `TestHeaders` (twice each, values not shared),
  `TestResponseOfAnyKind` (answered at once and proxied) and `TestParamErrors` of both packages,
  ported; the cases that need A32 checked to be refused as not supported yet; the rewritten path
  and the changed request fields as the upstream sees them, with a 1 MiB body; middleware run in
  order (a rewrite then a redirect); clients that leave midway; unit tests of the path escaping,
  `otherHost`, `hasScheme`, the placeholders and the request-fields local.
- Compatibility gate: the 22 fields in `tests/compat/supported.txt`; the gate names a refused
  middleware module as `routes[].middleware[]<module>` and the rewrite's refused fields with
  their module; `make compat-bless`: `examples/headers` is valid, and the other examples are
  refused only for the middleware modules they use (80 fixtures: 10 valid, 69 refused only as
  not supported yet, 1 skipped, none failed).
- Checked: `make check` on macOS; on the benchmark host, the touched crates' tests (`cargo test
  -p akit-middleware -p akit-config -p akit-runtime -p akit-router -p akit-proxy -p akit-compat`)
  with epoll and with `AKIO_TEST_BACKEND=uring`.
- Differs from fit: through akit's listener the path is canonical before any middleware sees it,
  as fit's listener makes it, so fit's `path makes //` case is answered `302 /evil.com/x` end to
  end (and a `\` next to a `/` is refused 400 by the listener). Errors of a header name are
  reported at the `request` or `response` object, as fit's.
- To measure, in the maintainer's benchmark tasks (rule 30): see the dev plan's benchmark row
  for I40.

### 2026-11-24: I26, Admin API I: load, read, versions

- Config: `admin` read into `akit_config::Admin` (`address`, `history`, `max_config_bytes`,
  `max_conns`; `DEFAULT_ADMIN_*`, fit's defaults: 127.0.0.1:2019, 10, 4 MiB, 16; 0 is the
  default). fit's check of the address: a host that is not `localhost` or a loopback address is
  refused (off loopback fit asks for `token_file`, `cert_file` and `key_file`, which come with
  I27); `unix/<path>` is refused as not supported yet (akio API request A23); `admin` is given by
  one file only. The other admin fields are refused as not supported yet.
- `akit_config::value` (new): `Value`, a JSON value with members in order and numbers as written,
  `parse`, `to_json` (fit's Admin API writer: two spaces to a level), `standardize` (comments and
  trailing commas blanked to spaces, every other byte in place, as fit's `config.Standardize`),
  and `json_of`, a config's files as one JSON document (a lone JSON file standardized; several,
  or YAML or `.conf`, merged as fit's adapter merges them and written out). `load_files` reads a
  config and that document together.
- `akit-admin` (decision 006): `Configs`, the history (numbered from 1, the last `history`
  kept, ETags of the first 96 bits of SHA-256 via akio-crypto, `If-Match` and `If-None-Match` as
  fit compares them) and the one path every config takes, `Configs::update`, one at a time;
  `Server`, akio-http on an akio engine of its own (one I/O thread, HTTP/1.1, `max_conns` as the
  engine's `max_connections`, fit's timeouts): `GET /config` (ETag, `Fit-Config-Version`, 304 on
  `If-None-Match`), `GET /config/<pointer>` (RFC 6901, fit's messages), `POST /load`
  (`application/json`, streamed in under `max_config_bytes`, 413/415, `If-Match` 412),
  `GET /versions`, `GET /versions/{n}`; fit's guard (`Origin` 403 but for same-origin reads,
  `Host` must be loopback). A load is detached and handed to the control thread as a `Job`
  (bounded queue, 503 when full), which applies it and answers it. Answers are fit's JSON:
  `{"version","etag","listeners_added","listeners_removed","warnings"}`, and config errors as
  `{"error","errors":[{"path","line","column","message"}]}`.
- `akit run`: with `admin`, the API is started after the listeners (`akit: admin API on ADDR`);
  signals and loads share one bounded control channel, so the main loop applies both, one after
  another: a load through `Runner::apply_text` (parse, compile, `Listeners::reload`, the access
  log's level and zone), SIGHUP through the same `Runner::apply`, each as a new version
  (`akit: reloaded FILE (config version N)`, `akit: loaded a config from the admin API (config
  version N)`). A load that changes `admin` warns that it takes a restart; so do access log
  changes, as before. On shutdown the API stops first and loads still queued get 503.
- Tests: `akit-admin/tests/admin.rs` (10), fit's `admin_test.go` ported: load and read back
  (whole, by pointer, by version, 304, HEAD, 404s, 405), comments kept as standard JSON, loads
  refused (unknown field with fit's exact answer, a module that does not exist, an address in
  use, wrong type, too large by length and chunked, unended JSON; 501 for I27's endpoints and dry
  runs), `If-Match`, browsers refused, history, 8 concurrent loads with distinct versions, slow
  and cut clients, the connection limit, listeners added and removed and the admin warning; unit
  tests of the history, ETags, pointers and loopback hosts. `akit-config/tests/admin.rs` (4): the
  fields, fit's address check, one file only, `load_files`; `value`'s unit tests.
  `akit/tests/admin.rs`: `akit run` end to end, a load and a SIGHUP numbered in one history. The
  gate's samples for the five fields.
- Fuzzing: a new target, `config_value` (in CI beside the others): standardizing keeps the length
  and the value, and a value written out parses back to itself; 3 million runs in 45 s, no crash.
- Checked: `make check` on macOS; the tests of akit-config, akit-admin, akit, akit-compat,
  akit-runtime and akit-listener on Linux (the benchmark host): io_uring all passing; epoll all
  passing but I22's same-address replacement tests (`akit-listener` and `akit` `tests/reload.rs`),
  which fail now and then on main `1a5fccf` too (1 run in 6 there, 1 in 3 here): a request reaches
  the replaced listener in the moment before it stops accepting, and gets its graph's 404. The gate (`make compat-bless`): `admin` and four of its
  fields supported; `examples/admin` now refused only for its Unix socket, `examples/console` for
  `admin.console` and `admin.store`, and admin no longer refused in `bittorrent`, `metrics`,
  `proxy` and the sync examples (fields refused 58 to 60, as `admin` splits into its subfields).
- Differences from the plan and from fit:
  - No Unix socket yet: akio's listeners take a `SocketAddr` only (akio API request A23).
  - A load past `max_config_bytes` gets 413, but akio-http keeps reading the rest of the body
    (dropped) and keeps the connection, where fit closes it (akio API request A24); the body
    timeout bounds a slow client.
  - fit's token and Unix socket tests are not ported (I27, A23); `TestWarnings`' access log and
    limits warnings are not either: akit's warnings come from `akit run`'s own apply step.
  - `PUT`/`PATCH`/`DELETE /config`, `POST /rollback/{n}`, `POST /reload` and `?dry_run=1` answer
    501 until I27; `/config/effective` (providers) is I57 and reads as a pointer for now.
  - The version history's `applied` is RFC 3339 in UTC with milliseconds; fit's is Go's
    `time.Time` (nanoseconds, the local offset).
- For I37 (metrics): `akit_admin::Options::metrics`, a writer `GET /metrics` answers with
  (`METRICS_CONTENT_TYPE`; 404 when unset). At the merge, `akit run` passes I37's `Scrape::write`
  there and calls `ConfigStats::reloaded` after each Admin API load (`Job::run` returns the
  outcome, `None` when nothing was applied), as its SIGHUP reload does.
- Benchmark: none (rule 24: the control plane).

### 2026-11-24: I20, passive health checks

- Config: `upstreams[].max_fails` (from 0; 1 when left out, 0 keeps every address in rotation)
  and `upstreams[].fail_timeout` (10s when left out or 0) are read into `akit_config::Upstream`
  (`max_fails`, `fail_timeout`; `DEFAULT_MAX_FAILS`, `DEFAULT_FAIL_TIMEOUT`), as fit's.
- `akit_upstream::health::Health`, one per address of a pool, as fit's `Backend`: `max_fails`
  failures within `fail_timeout` (the window starting at the first) take the address out of
  rotation for `fail_timeout` from the last; one response from it clears the failures and puts it
  back at once; failures in different windows do not add up. A failure is what
  `Error::is_upstream_failure` says: the client's errors (refused or timed-out connect, a broken
  connection, a malformed or too large response, the exchange's timeout) and a timeout waiting
  for a connection; `Busy`, `Closed`, `NoEngine` and a bad target are not. Any response counts
  as a success, whatever its status, as in fit. The pool records the outcome before it calls the
  request's callback, so the callback sees the health the exchange left.
- The balancers pass over addresses that are down, as fit's do: `Balancer::pick_up(load, up)`
  (`pick(load)` is unchanged and takes every address as up). Round robin and least connections
  move the slot's turn past the addresses passed over; weighted round robin draws more entries of
  its round, so a down address's share goes to the others by weight, and the locked form (rounds
  too long to work out ahead) runs nginx's algorithm over the addresses up. Every address down:
  health is ignored and the pick is the one without health. `Pool::pick` (signature unchanged)
  passes each address's health; the pool adds `is_down(i)`, `available()` (fit's `Available`),
  `max_fails()` and `fail_timeout()`.
- State (rule 13): health is the address's, shared by every I/O thread, as fit's, so that a
  failure seen on one thread takes the address out for all. A pick reads one atomic per address
  it looks at, and the clock (once a pick) only when an address has been out; a success reads two
  atomics and writes nothing while the address has no failure counted. Only a failure, and the
  first success after failures, take the address's own mutex: no global lock, nothing on the path
  of requests that succeed. Times are nanoseconds since the pool was made (`Instant`).
- Tests: `akit-upstream/tests/health.rs` (9), fit's `health_test.go` and
  `TestWeightedRoundRobinDown` ported: `max_fails` (counting, the window, out for `fail_timeout`
  and back, `max_fails` 0 and 1) on a clock the test moves; a success clearing the count; every
  policy passing over a down address (exact shares), all down (health ignored), back after the
  window; least connections passing over an idle address that is down; weighted round robin down
  (shares by weight; all down gives the weights exactly) with and without a schedule; through a
  pool on real sockets: fit's `TestLeaveAndRejoin` (an address that starts refusing is taken out
  by its first failure, nothing reaches it while out, and it is back after `fail_timeout`), all
  addresses down still picked, and `max_fails` 3 counted through the pool.
  `akit-config/tests/upstreams.rs`: the two fields, defaults and bounds. The gate's samples for
  both. `akit-upstream/tests/pool.rs`'s refusing-address test sets `max_fails` 0 so its
  alternation does not depend on timing.
- Rebased onto akit main `69e255e` (akio `a9a620ab`): conflicts only in the config exports, the
  status rows and the generated compatibility files, regenerated.
- Checked: `make check` on macOS; the tests of akit-config and akit-upstream on Linux (the
  benchmark host), epoll and io_uring, all passing. The gate (`make compat-bless`): `upstreams[]`
  12 of its 37 fields supported; `examples/load-balancing` and `examples/proxy` no longer refuse
  `max_fails` and `fail_timeout` (fields refused 60 to 58).
- Differences from the plan and from fit:
  - fit's allocation check of a pick with an address down (`TestPickSkipsDown`'s
    `AllocsPerRun`) is not ported (a counting allocator needs `unsafe`); picking allocates
    nothing by construction.
  - fit's active health checks (`health_check`, `probedDown`) are I35; `Health` has no probed
    state yet, and `health_check` stays refused.
  - A response's body is buffered by akio's client, so the outcome is known when the callback
    runs; once I16 streams bodies, a body that breaks off should count as a failure as fit's
    `Done(b, true)` does (I16 then needs a way to record it: a `Pool` method that does not count
    the exchange twice).
- What a benchmark task should measure: `Pool::pick` per policy with no address down, one down
  and all down, 1 and all cores (the clock read and the extra draws); recording a success on the
  request path (the two atomic loads); a proxy run with one address refusing against fit, checking
  the errors stay at one per `fail_timeout`.


### 2026-11-24: I22 on akio's A1: every listener reloadable, h3 added by a reload

- `Listeners::reload` (`crates/akit-listener/src/reload.rs`) now moves listeners with akio's
  runtime listeners (A1, ADR-099) instead of the `adopt_tcp` stopgap, whose accept threads are
  gone. Before the swap, a new or changed listener's TCP side is added with
  `Engine::add_listener` (the backend of `Options`, its own I/O threads, as the startup ones), so
  an address in use still fails the reload with nothing changed (those added for it are removed
  again). After the swap, a removed or changed one is drained with
  `Engine::shutdown_listener` for `-shutdown-timeout` (fit drains its retired bindings): each of
  its connections is asked to close (`Connection: close` after the response in flight, `GOAWAY`
  on HTTP/2) and those still open at the deadline are closed. `Listeners::new` takes the
  `Options` and reads the listeners' ids from `Engine::listeners`.
- The listeners akit started with are reloaded like the rest: changed (protocols, `tls`,
  limits) or removed, no longer refused. A listener replacing one at the same address is added
  beside it: on Linux akio's listeners share the port (`SO_REUSEPORT`) while the old one drains,
  so no connection is refused. On macOS akio's listeners do not share it: the old one stops
  listening first (retried for up to a second while its I/O thread closes the socket), as fit's
  reload does on such a system, and it cannot be put back if the reload then fails.
- HTTP/3 on reload: a listener serving `h3` added by a reload gets its TCP side from
  `add_listener` and its HTTP/3 from `Engine::bind_udp` on the port TCP got (probed free for both
  at port 0, as at startup), on the engine's UDP threads: those of the last startup listener
  serving h3, or one UDP thread `engine` now adds (`udp_client_threads(1)`) when none does. One
  removed by a later reload is closed (`UdpBinding::close`, its connections closed with it).
  akio adds and removes TCP listeners only: a startup listener serving h3, whose UDP listener the
  engine was built with, cannot be removed or changed, and such a reload is refused
  (`ReloadError::Udp`, "… serves h3 and is removed or changed, which needs a restart …"), the
  running config kept. Requested from akio as A22.
- `lib.rs`: `build` split into `tcp_listener` and `h3_acceptor`, which `reload` uses too.
- Tests: `akit-listener/tests/reload.rs` (3): listeners kept, added, changed at the same
  address and removed, an idle connection of a removed listener drained, the address free again;
  the listener akit started with changed at its address and removed; refusals with nothing
  changed (an address in use after another new listener was added, h2c alone).
  `akit-listener/tests/h3.rs` `reload_knows_h3_listeners`: an h3 listener added by a reload
  answers over HTTP/3 and over HTTP/1.1 with its `Alt-Svc`, removing the startup h3 listener is
  refused with the epoch unchanged, the added one removed again. `akit/tests/reload.rs`, the e2e
  test under traffic: the clients now honour `Connection: close` (a drained listener's answer),
  `web` listens at a port fixed in every config, and on Linux it is replaced at that address
  (its protocols changed, then back) under traffic, with no failed request; the step needing a
  startup listener refused is gone. A drain closes an idle HTTP/1 keep-alive connection at once
  (akio-http's `on_shutdown`), so a request the client had already sent on it meets the close
  unanswered; the test's clients send it again on a new connection, as HTTP clients do with
  idempotent requests (RFC 9112 section 9.3.1), and count it (at most one per client per
  replacement); every other error still fails the test.
- Checked: `make check` on macOS; the tests of akit and akit-listener on Linux (the benchmark
  host), epoll and io_uring, the e2e test five more times.
- Left: a startup h3 listener (A22); a removed listener's open requests that arrive after the swap
  still find no listener of their name and get 404 (their connections are told to close); a kept
  TLS listener keeps its startup certificates (A2's resolver, I30); a client that does not retry
  sees the keep-alive close race above when a listener it uses is removed or replaced.

### 2026-11-24: I13, static files II

- On akio `a9a620ab` (A13 and A16–A18, akio ADR-100), the `static` handler sets akio-http's
  `Files` up from all of fit's I13 params:
  - `precompressed` (akit-config `Static::precompressed`, `Encoding`; `"deflate" is not one of "br",
    "zstd", "gzip"` at `precompressed[i]`) gives `Files::twins`: the twin (`name.br`, `name.zst`,
    `name.gz`) of the coding the client prefers by its q values (RFC 9110: `q=0` refuses a coding,
    `*` stands for those not named, names in any case), among those listed and in their order among
    equals; none, and no `Vary`, when left out. From memory, or from disk past `max_file_bytes`
    with the twin's length; `Content-Encoding`, `Vary: Accept-Encoding`, the original's
    `Content-Type`, an `ETag` of its own that conditions apply to, ranges of the twin; a twin that
    changes or goes is followed as files are.
  - `try_files` gives `Files::try_paths`: `{path}` stands for the path asked for, a candidate ending
    in `/` names a directory, served by its index (or listing) and not redirected; the first that
    exists is served, else 404. What the candidates resolved a path to is kept (up to 4096 paths)
    and trusted for `check_interval`, so a path resolved lately is answered on the I/O thread; the
    rest are resolved on akio's file pool. Checked as fit checks it: each starts with `/` or
    `{path}`, and with `dotfiles` `deny` names no dotfile but under `/.well-known/`. An empty list,
    as fit's, finds nothing (every request 404).
  - `browse` and `browse_max_entries` (10000 when left out or 0) give `Files::list_directories`:
    fit's page for a directory without its index file, read on the pool for every request.
  - `dotfiles` `deny` also gives `Files::hide`, so a `try_files` candidate a dotfile segment names
    is passed over, as fit's handler passes it over.
  - The path served is the canonical one the route matched (akit-listener's `CANONICAL_PATH`, A13's
    `Files::serve`): a `..` that stays inside the root (`/x/../ok.txt`) is removed and the file
    served, as fit's listener and handler together serve it, where I12 answered 400.
  - `strip_prefix`: cut from that path as written, a path without it answered 404, the rest
    checked as before and served; `"assets" does not start with /` as fit's. Only a handler with a
    prefix, or a request whose path was not canonical, copies the path; otherwise `Files` reads it
    from the request.
- `sendfile` for large files: there since I12. `Files` sends a file (or twin) larger than
  `max_file_bytes` from a kept handle as a `FileBody`, which akio sends with `sendfile` on plain
  HTTP/1; over TLS and h2 akio reads it in pieces.
- The compatibility gate: `strip_prefix` and `try_files` supported too (72 fields in all); fit's
  `examples/static` is refused only for `max_cache_bytes` and `middleware`, `examples/bittorrent` no
  longer for `strip_prefix` (`make compat-bless`). `compress` stays refused (I47's compression),
  `max_cache_bytes` and `default_type` wait on A15 and A14.
- No benchmark scenario added: fit's harness has none for twins, `try_files` or listings (its
  configs serve nothing precompressed, bench README), so akit's has none either (rule 30: no
  benchmark run for a feature task).
- Tests (`akit-handler/tests/static_files.rs`, 16): `precompressed`, fit's TestPrecompressed ported
  with fit's answers for all 11 `Accept-Encoding` fields, ETags and `If-None-Match` per
  representation, a range of a twin, `HEAD`, twins from disk, a list of one encoding, no list (no
  twin, no `Vary`); `precompressed_follows_the_disk`; `large_from_disk` with fit's large twin;
  `try_files` (fit's TestTryFiles and TestTryFilesUnsafe, each path twice to go through what
  `Files` keeps, an empty list, a dotfile candidate passed over); `browse` (fit's TestBrowse,
  TestBrowseMaxEntries, TestBrowseOrder); `strip_prefix` (fit's TestStripPrefix); `config_errors`
  with fit's TestFeatureErrors, TestDotfilesConfig and `strip_prefix`'s check; `traversal` now
  with `/x/../ok.txt` served. A unit test of the dotfile check on decoded paths (`Files::hide`'s).
- Checked: `make check` on macOS; on Linux, the tests of akit-handler, akit-config and akit-compat with epoll and of akit-handler with io_uring, passing.
- Differences from the plan and from fit:
  - The plan's "a benchmark scenario uses it": none, as above; the benchmarks table (dev-plan) says
    static files are not benchmarked yet.
  - A14 (I12's): `Files` serves a directory asked for without its slash (fit redirects, 301), here
    also below `strip_prefix`; its 404 for a path no candidate finds has its own text; a listing
    below `strip_prefix` is titled by the path below the prefix (`Index of /`, no `../` link) where
    fit's is titled by the whole path (`Index of /assets/`). Added to A14.
  - `strip_prefix` is cut from the path as sent (percent-encoded), fit's from the decoded path: a
    prefix spelt with escapes in the request (`/%61ssets/`) is 404 in akit.
  - `/../secret` with `try_files`: 400 from akio's parser, with its own text, as I12's traversal
    tests already record.
- For a benchmark task: none until fit's harness has a scenario; then a twin from memory and a large
  twin from disk against the original, `try_files` resolved from what `Files` keeps against the
  pool, and the cost of `Accept-Encoding` ranking per request.

### 2026-11-24: I21, access logs

- `akit-config` reads fit's `access_log`: `output` (`stdout`, the default, `stderr` or a file's
  path), `flush_interval` (1s), `buffer_bytes` (4 MiB; 0 stands for the defaults, as fit's),
  `level` (`info`, `warn`, `error`: the least status written, 0, 400 or 500), `format` (`json`,
  `text`, `line`) and `time_zone` (as written). `rotate` stays refused as not supported yet (fit's
  `logs` package's rotation). As fit's adapter merges files, `access_log` given by a second file
  is refused: `also given in a.json: only the lists at the top of a config (listeners, routes,
  upstreams, services, providers) may be split across files`. JSON, YAML and `.conf` alike.
- `akit-observe` (std alone):
  - `AccessLog`: lines formatted on the thread that finishes a response, into a buffer of that
    thread's (a shard: one per thread that logs, up to 64, a power of two; threads past that
    share), each with its own mutex, which only that thread and the writer (once a flush) take.
    A shard hands its buffer to the writer thread over a bounded channel once it holds 64 KiB
    (fit's `flushBytes`), and the writer takes every shard's on the flush interval, under the
    shard's lock through the same channel, so one thread's lines keep their order. What waits for
    the writer plus what a shard holds is bounded by `buffer_bytes`, as fit's; a line past it is
    dropped and counted, never waited for. The writer reports drops once a flush
    (`akit: access log: N lines dropped, the output was too slow` on stderr, fit's message); an
    output that fails loses the buffer's lines, counted as dropped. `close` writes out what is
    left and joins the writer; lines after it are dropped. `set_min_status` and `set_zone` change
    the level and zone in place (a reload).
  - `Format`, `Record` and `append_line`: fit's three formats with its fields in its order
    (`time`, `listener`, `route` when one took it, `client`, `user` when set, `proto`, `method`,
    `host`, `uri`, `status`, `bytes_in`, `bytes_out`, `duration`, `upstream` and
    `upstream_duration` when proxied, `user_agent`, `referer`); seconds with six decimals rounded
    to the microsecond; JSON strings as fit's `appendString` (bytes that are not UTF-8 as U+FFFD);
    logfmt values bare or quoted as Go's `strconv.Quote`; `text`'s and `line`'s levels by status
    (INFO, WARN for 4xx, ERROR for 5xx), `line`'s padded to five. Times are RFC 3339 with
    milliseconds in the zone (`Z` for UTC, else `+08:00`), `line`'s `2006-01-02 15:04:05.000`;
    the zone's offset is looked up once a second per shard.
  - `Zone`: `local` (or empty: `TZ`, else `/etc/localtime`, else UTC, as Go's `time.Local`),
    `utc`, or an IANA name read from the system's zoneinfo directories (Go's list); a TZif reader
    (versions 1 to 4) with the POSIX `TZ` rule of the footer for times past the last transition
    (`Mm.w.d`, `Jn`, `n`, times up to 167h, the US rules for a rule without dates). A name is
    checked to stay inside the directory. `akit run` and `akit validate` load it, so a bad one is
    `access_log.time_zone: "Nowhere/City" is not a time zone: local, utc or an IANA name such as
    Asia/Shanghai`, fit's message.
  - `with_upstream`, `upstream`, `dispatch_logged`, `logged`: thread-local hand-offs for what a
    handler knows; `Name`, a short name kept inline (a listener's or route's, carried to the line
    without an allocation of its own); `open_output`.
- `akit-runtime`: `Env::with_access_log` gives the process's log to the graph of a config with
  `access_log` (`Graph::access_log`); a config without one logs nothing.
- `akit-listener`: when the graph has a log, the `Dispatcher` registers a response filter
  (`Context::add_response_filter`) for each request, the 400 for a bad path and the 404 included,
  holding this thread's shard, the start, the listener's and route's names. akio runs it on the
  thread that sends the response (the I/O thread, or the thread a detached handler answers from),
  and it builds the line there from the request and the response (`access_record`): the client
  (an IPv4-mapped address as IPv4), `HTTP/1.0` to `HTTP/3.0`, the `Host` (or an absolute
  target's authority), the target as sent, `bytes_in` (the body read, else `Content-Length`),
  `bytes_out` (0 for `HEAD`, 1xx, 204 and 304; a file's length; the body's; for a streamed body,
  whose bytes are not known when the head goes out, its `Content-Length` or 0). A response never
  sent (its connection gone) has no line, as fit's. The handler runs inside `dispatch_logged`.
- `akit-proxy`: for a logged request it picks the address itself (`Pool::pick`, then `send`) and
  answers inside `with_upstream(address, time)`, so the line has `upstream` and
  `upstream_duration`; an unlogged request goes as before, nothing copied.
- `akit run`: opens the log before compiling the graph (`akit run: access log: <error>` and exit
  1 when the output cannot be opened), warns, as fit does, that a file output without `rotate`
  grows without bound, and closes it after the engine has stopped (also when the shutdown ran out
  of time), so every response finished has its line. A reload sets the new level and zone; a
  changed `output`, `format`, `flush_interval` or `buffer_bytes` takes a restart, and says so, as
  does an `access_log` added by a reload.
- Compatibility: `supported.txt` marks `access_log` and its six fields; the gate maps
  `ACCESS_LOG_FIELDS` and `ACCESS_LOG_FIT_ONLY` and samples each field. `make compat-bless`:
  `examples/logging` no longer refuses `access_log` (still refused for `log`); 65 fields refused,
  from 66.
- Tests: `akit-observe` unit tests (dates, times, durations, JSON and logfmt escaping, POSIX rules
  of both hemispheres, a TZif file with a footer and truncated ones, the system's
  `Asia/Shanghai` and `America/New_York` where present, names that would leave the zoneinfo
  directory, `Name`, the thread-local hand-offs) and `tests/access_log.rs` (9, from fit's
  `accesslog_test.go`): every field in each format, exactly; the level and zone changed in place;
  the flush interval; a full buffer written without waiting for it; **a writer slower than the
  traffic**: the output blocked in its first write while 4 threads log 10,000 lines, which return
  at once (under 2 s), lines past the buffer dropped, written plus dropped equal to logged, every
  drop reported, no line cut; a failing output; 8 threads keeping their order; a file appended
  to. `akit-config/tests/access_log.rs` (5): defaults, every field, every error kind with its
  path, YAML and `.conf`, one file only. `akit/tests/access_log.rs` (2, end to end with
  `akit run`): a respond route, a proxied `POST` (`upstream`, `upstream_duration`, `bytes_in`), a
  404, a 400, a `HEAD`, an HTTP/1.0 request, each line's fields checked, all written at
  shutdown though the flush interval is an hour; a reload to `level: error` stopping 4xx lines.
  The slow-writer test found a bug in the first version: a close that arrived while the writer
  was flushing on its interval was taken for a buffer and lost, and the close waited forever.
- Where it differs from the plan and from fit: it touches `akit-config`, `akit-runtime`,
  `akit-listener`, `akit-proxy` and `akit` besides `akit-observe`. A line goes to the shard of the
  thread that dispatched the request, not of its client's address (fit's sharding by address
  keeps a connection's lines in order; one I/O thread serves a connection here, which does the
  same with no hashing). A proxied request's line is built on the thread its upstream's response
  comes in on (the pools' threads), and locks the dispatching thread's shard there: the shard
  lock is then contended across two threads. fit's free list of written buffers is left out (a
  new buffer per 64 KiB of lines). `user` is never set until authentication exists (I42).
  `bytes_out` of a streamed response is its `Content-Length`, not the bytes written: akio-http
  shows response filters the head only. The line is built before filters registered after it
  run, so once compression (I47) registers one, `bytes_out` would be the body before
  compression, where fit's counts what was written. Go's `unicode.IsPrint` is approximated for
  logfmt quoting (controls, separators and common format characters escaped). No fit `log`
  section (fit's own log, and its shared file sink with the access log).
- Checked: `make check` on macOS; the touched crates (akit-observe, akit-config, akit-runtime,
  akit-listener, akit-proxy, akit, akit-compat) on Linux (gpu51), epoll and io_uring.
- For a benchmark task (none was run, rule 30): a same-run A/B of the fixed `respond` route and
  of the proxy with the log off, on to `/dev/null` and on to a file, in `json` and `line`; the
  per-request cost of the response filter (one boxed closure each, akio-http's API: if it shows,
  an akio API request for an allocation-free completion hook), the shard lock (contended for
  proxied requests) and formatting; lines dropped at full load with the default 4 MiB buffer and
  with a slow disk.

### 2026-11-24: I16, reverse proxy II: streamed bodies, linked backpressure

- Done on akio `60547435` (ADR-101), where A19–A21 landed (named A16–A18 when first written up
  here, renumbered at the merge). The pin moved there (`Cargo.toml`, `akit_runtime::AKIO_REV`,
  `akit version`'s line). The body limit came first, while they waited (below).
- `akit-upstream`: `Pool::stream(backend, req, impl Exchange)`, an exchange whose request body is
  written as it is produced (`Exchange::on_request_body`, akio's `RequestBody`) and whose response
  arrives as it is read (`on_head` with a `Download` that holds and resumes the upstream's reads
  or cancels, `on_data`, `on_end`, `on_error` with the pool's `Error`), on akio's
  `Client::request_with`. It is bounded by `max_conns` and `max_pending`, balanced and timed as
  `send` is, and holds its address until the body is complete, so `max_conns` and `least_conn`
  count a long download as busy, as fit's do. A body that breaks off after the head counts as the
  address's failure, as fit's `Done(b, true)`, unless the proxy cancelled it (its client went):
  the open point I20 left.
- `akit-listener`: the `Dispatcher` routes `body_mode` (the route found on the current graph, as
  `call` finds it; a reload between the two can give a streamed body to a route that buffers,
  which then sees it empty). A route whose handler streams gets its body so, unless the request
  declares a length past `max_body_bytes`, which is then read as before and answered 413 by akio.
  A request whose body streams carries the limit in `akit_runtime::MAX_BODY_BYTES` (a context
  local, as `TLS`), since akio holds a streamed body to none.
- `akit-proxy` (`relay.rs`), as fit's `exchange`:
  - A request body past 64 KiB or of no declared length (chunked, or HTTP/2 and HTTP/3 without
    `Content-Length`) streams (fit's `StreamThreshold`); a smaller one comes whole and goes up
    whole. A streamed one goes up framed as it came: `Content-Length`, or chunked (an HTTP/2 body
    without a length goes chunked, as fit's test expects), with its trailers when chunked.
  - What arrives before the upstream connection is ready waits in the relay, the client's reads
    held. Then each piece is written to the `RequestBody`; past 64 KiB queued (fit's
    `uploadPending`) the client's reads are held (`BodyReads`) until the queue drains
    (`RequestBody::on_drain`). A body growing past `max_body_bytes` is answered 413 and its
    upstream request dropped (its connection closed). An upstream that answers before the body is
    in ends the upload; what still arrives is dropped, unheld.
  - A response of a length within 64 KiB (fit's upstream `StreamThreshold`), and one to `HEAD` or
    of a status without a body, is collected and sent whole; any other is sent as it arrives
    (`stream_response`). Past 64 KiB waiting for the client the upstream's reads are held
    (`Download::hold_reads`) until the client has taken it (`BodyStream::on_drain`). A client
    gone cancels the upstream response. A response that breaks off mid-body closes the client's
    connection. The upstream's trailers go on after a chunked body.
  - One lock per request between the two sides' threads, never held while calling into akio
    (whose calls may call back); the request body's handle goes to the side that ends it alone.
- Tests: `akit-proxy/tests/stream.rs`, fit's `TestStreamDownload` and `TestStreamUpload` ported:
  64 MiB of a prime-length pattern each way, by length and chunked, over HTTP/1.1 and h2c (a
  minimal h2c client of the test's own, which reads as slowly as it is told and gives its window
  back only as it does); the slow side pauses 20 ms after every 4 MiB; every byte checked; the
  process's resident memory (`/proc/self/statm`, sampled every 5 ms; a counting allocator would
  need `unsafe`, rule 14) may rise less than 16 MiB, checked on Linux. `broken_download`, fit's
  `TestUpstreamBreaksMidBody` over HTTP/1.1: the client's connection closes short of the body's
  end. `proxy.rs`'s upstream reads chunked bodies too, as `body_limit`'s chunked uploads now go up
  chunked.
- Checked: `make check` on macOS; `cargo test --workspace` on Linux (gpu51) with epoll and with
  `AKIO_TEST_BACKEND=uring`, `--no-fail-fast`: everything passes but
  `akit-listener`'s `reload.rs` under io_uring, which fails one of its tests in about half its runs
  (a GET on a listener just reloaded is cut off, `reload.rs:72`); akit's `main` (`3631362`, akio
  `a9a620ab`) fails it as often there, so it is not I16's. The stream tests' resident memory rose
  at most 4 MiB moving 64 MiB, either way, on both backends. h2c downloads to the test's slow
  client took 1 to 20 s where HTTP/1.1 took under 1 s (the test's client gives its window back a
  frame at a time); not measured further (rule 30).
- Differences from the plan and from fit:
  - An HTTP/1.1 response that streams goes chunked, its `Content-Length` dropped: akio cannot
    frame a streamed HTTP/1.1 response by its length yet (akio API request A35). HTTP/2 and
    HTTP/3 keep it. A whole response (within 64 KiB) keeps it everywhere.
  - fit's spliced uploads (`spliceMinBody`, splice(2) from the client's socket to the
    upstream's) are not ported: akio has no splice between a server and a client connection. A
    benchmark should say whether it is worth an akio request.
  - A client that stops reading holds its upstream until the exchange's timeout (1h): fit's
    `write_stall_timeout` and `response_idle_timeout` are I17, as are cancelling the upstream
    request when the client goes before the response, and announced trailers.
  - An HTTP/2 response that breaks off mid-body without a length ends with its stream dropped
    (akio closes the connection), not fit's `Fit-Error` trailer (I17, with the trailers).
  - The h2 upstreams (`H2Client` has no streaming yet) are I18.
- For a benchmark task: memory per proxied request in flight with large bodies both ways and a
  slow peer, and throughput of large uploads and downloads, against fit; the 64 KiB watermarks
  (fit's) against larger ones; the cost of `body_mode`'s second route lookup for requests with a
  body.

Before A19–A21 landed (when this entry said `doing`):

- fit's body limit. `akit_config::Limits` reads `limits`:
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
- HTTP/3's limit is set but tested only through the shared code (the h3 test client sends GETs
  alone). No `limits` field is printed at startup (fit's `-v` line of limits comes with the rest
  of them).

### 2026-11-24: I39, matchers v2 and expressions

- Taken early from M3: its one dependency, I11, is done. `akit-router` has fit's matchers
  (`router/matchers.go`) as `akit_router::Matcher`: `methods` (compared as written), `headers`
  (names ignoring case, a field's values compared as they are; any value when none are listed),
  `query` (keys and values compared once unescaped, `+` as a space, without allocating; an
  invalid escape matches nothing), `remote_ip` (addresses and CIDR prefixes, `parse_prefix` as
  Go's `netip` with fit's rules: an IPv4 address mapped into IPv6 is IPv4, an IPv4 prefix written
  in IPv6 refused, a prefix masked; the peer's address unmapped), `path_regexp` (on `akio-regex`,
  decision 001: linear time, compiled with Go's ASCII `\d \w \s \b`; a path that is not UTF-8
  reads as one U+FFFD per invalid byte, as Go's regexp reads it, through `is_match_fmt`, without
  allocating) and `expr`, fit's expression language (`router/expr.go`, `akit_router::Expr`):
  `Host`, `Path`, `PathPrefix`, `PathRegexp`, `Method`, `Header`, `Query` and `RemoteIP` combined
  with `&&`, `||`, `!` and parentheses, strings quoted as in Go (`strconv.Unquote`, `\x` and
  octal escapes included, so arguments are bytes), at most 64 KiB and nested at most 100 deep,
  with fit's error messages (`at byte N: ...`) and its canonical printing. Chains of `&&` and `||`
  are kept flat, so evaluating, printing and dropping an expression recurse only as deep as its
  parentheses and `!`s (a chain of thousands of matchers is not deep).
- The router (`Router::find_request`, generic over `akit_router::Request`: method, host, path,
  query, peer address and header fields) runs a route's matchers as fit's does: under each host
  and path pattern the routes are kept best first (priority, host and path specificity, then more
  matchers, then the route written first) and cut after the first with no matchers; a route's
  matchers run only once its host and path match and only while it would beat the best route
  found, so a lower-priority route's matchers never run once a higher one matched.
  `Router::find(host, path)` stays, as a GET with nothing else (`Parts::new`). Every matcher is
  built when the config is compiled (rule 12), cheapest first as fit's graph orders them
  (method, remote IP, header, query, path regexp, expression; an expression counts as one).
- `akit-config` reads `match.path_regexp`, `method`, `header`, `query`, `remote_ip` and `expr`
  with fit's checks (`config/validate.go`: a method and a header name must be tokens, a query
  key not empty, the regexps, prefixes and expression must build, with the router's own
  builders: akit-config now depends on akit-router). `akit-runtime` builds them into
  `CompiledRoute::matchers` and has `Graph::route_request`; `akit-listener`'s `Dispatcher`
  routes with it, reading the method, `Host`, query, peer address and header fields from akio's
  context.
- Tests: fit's `TestMatchers`, `TestMatcherErrors`, `TestMatcherOrder` (its `panicMatcher` cases
  with a request that counts the matchers that read it), `TestParseExpr`, `TestParseExprErrors`,
  `TestExprMatch`, `TestQueryHexEscapes`, `TestSameBucket`, `TestPathTwice` with a matcher, and
  `FuzzParseExpr`'s seeds and more as a round-trip test (`akit-router` `src/tests_matchers.rs`),
  plus details of query pairs, prefixes, ASCII classes and non-UTF-8 paths; `tests/no_alloc.rs`
  extended with fit's `TestMatchersAllocs` and `TestExprAllocs` (and a path regexp): 0
  allocations in 100 lookups each; fit's config test "route matchers" (`akit-config`
  `tests/json_config.rs`); `akit-listener` `tests/listeners.rs` `route_matchers`: every kind of
  matcher on requests over HTTP/1.1, the query and method over h2c.
- Compatibility gate: the six fields in `tests/compat/supported.txt`; `make compat-bless`:
  `examples/routing` is now valid, and `match.method` / `match.remote_ip` no longer refuse
  body-limit, cache, forward-proxy, sync-wasm-sink and video-site (80 fixtures: 9 valid, 70
  refused only as not supported yet, 1 skipped, none failed).
- Checked: `make check` on macOS; on the benchmark host, `cargo test -p akit-router -p
  akit-config -p akit-runtime -p akit-listener` (epoll) and `AKIO_TEST_BACKEND=uring cargo test
  -p akit-listener`.
- Differs from fit: path regexps are `akio-regex`'s syntax, a subset of Go's: Unicode classes
  (`\pL`), POSIX classes (`[[:alpha:]]`) and `\Q...\E` are refused when the config is read, and
  the end of a regexp's error message is akio-regex's (`unclosed group at byte 1`), not Go's.
  Go's `%q` quoting is approximated: Rust has no Unicode categories, so "printable" is control
  characters, spaces other than U+0020 and common format characters left out (the printed form
  always parses back to the same). A map key in an error's path is written `header["Bad Name"]`,
  akit's form, where fit writes `header.Bad Name`. As in fit, `Header("Host")` never matches
  (Go's server takes `Host` out of the header fields).
- To measure, in the maintainer's benchmark tasks (rule 30): `find_request` with matchers,
  expressions and path regexps against `find` on the same routes (a micro-benchmark in
  `benches/lookup.rs`), and the fixed-scenario A/B of I39 against its base.

### 2026-11-24: I22, reload on SIGHUP

- Since moved onto akio's A1 (the entry above): the `adopt_tcp` stopgap and the refusal of
  startup listeners described here are gone.
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

### 2026-11-24: akio pinned at `a9a620ab`; I15 done

- The akio pin moved from `50a82315` to `a9a620ab` on akio's `main`: A1–A5, A7 and A8 (merged
  2026-11-24), A11 (I15's `Host`), A13 and A16–A18 (I13's `Files`). Every akio crate moved with
  it, and `akit_runtime::AKIO_REV`.
- I15 is `done`: its three A11 tests (`forwarding`, `request_fields_in_order`, `host_header`) are
  no longer ignored and pass.
- Checked: `make check` on macOS; on the benchmark host, the workspace's tests on epoll and
  io_uring.

### 2026-11-24: I15, reverse proxy I: requests and headers (was doing: waited for akio A11)

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
