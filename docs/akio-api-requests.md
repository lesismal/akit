# APIs akit needs from akio

[English](akio-api-requests.md) | [简体中文](akio-api-requests.zh-CN.md)

What akit asks of [akio](https://github.com/lesismal/akio), why, and how akit would use it
([rule 21](rules.md#8-when-akio-falls-short)). Names and signatures are suggestions; akio decides
the final shape. akit's sessions may make the change in akio themselves, by akio's rules.

This first list comes from reading akio `1.0.0-rc.1` (commit `342e14d3`) against what fit asks of
fib ([fib API requests](https://github.com/lesismal/fit/blob/main/docs/fib-api-requests.md)). Each
entry is to be confirmed against akio's code by the iteration that first needs it, before akio is
changed: an API may exist under another name.

On 2026-11-23 the user decided that all of A1–A8 are added to akio now, rather than when akit's
iterations reach them. Each is built on an akio branch of its own, confirmed against akio's code
first; its status here records the branch, then the akio commit akit pins.

## Summary

| ID | API | Status | Rules | akit iterations |
| --- | --- | --- | --- | --- |
| A1 | TCP listeners added, removed and drained on a running engine | in akio's main (`a9a620ab`), pinned | 10, 11 | I22, I28 |
| A2 | A server certificate chosen per handshake by a callback that may complete later | in akio's main (`a9a620ab`), pinned | 1, 4 | I30, I34 |
| A3 | A filter on accepted connections before any handler runs | in akio's main (`a9a620ab`), pinned | 11 | access lists |
| A4 | PROXY protocol v1/v2: parse on accept, write on dial | in akio's main (`a9a620ab`), pinned | 12 | I52 |
| A5 | An akio-mux rule that routes TLS by SNI without terminating it | in akio's main (`a9a620ab`), pinned | 12 | I52, I53 |
| A6 | CSR generation in `akio-x509` (and JWS helpers in `akio-crypto`) | done on akio branch `claude/x509-csr` (`7ea64069`), not merged | 15 | I31 |
| A7 | An MP4 (ISO BMFF) reader for segmenting files without transcoding | in akio's main (`a9a620ab`), pinned | — | media library |
| A8 | TUN devices | in akio's main (`a9a620ab`), pinned | — | VPN |
| A9 | A TLS server capped at TLS 1.2 (public `ServerConfigBuilder` API) | requested (I09) | 17 | I09 |
| A10 | Closing an HTTP client's idle connections now | requested (I14) | 10, 11 | I14, I22 |
| A11 | An HTTP/1 client request that keeps its own `Host` field (and sends no `User-Agent` when given an empty one) | in akio's main (`a9a620ab`), pinned | — | I15 |
| A12 | The HTTP client's connect timeout for a lone address too | requested (I14) | 10 | I14 |
| A13 | `Files` serving a path the caller names | in akio's main (`a9a620ab`), pinned | 19 | I12 |
| A14 | `Files` answering as a gateway's static handler does | requested (I12) | 19 | I12, I13 |
| A15 | `Files` bounded in memory, its disk pool with a timeout | requested (I12) | 4, 11 | I12 |
| A19 | An HTTP client request whose body is sent as it is produced, with backpressure | requested (I16) | 5, 8 | I16, I17, I18, I49, I75 |
| A20 | An HTTP client response delivered as it arrives, its reads held and resumed | requested (I16) | 5, 8 | I16, I17, I18, I67 |
| A21 | A server request that holds its client's reads and hears when its client drained | requested (I16) | 5, 8 | I16, I47, I75 |

## A1 TCP listeners on a running engine

**Problem.** `EngineBuilder::listen` adds TCP listeners before `build`; `Engine` has `bind_udp` at
run time but nothing for TCP, nor a way to close one listener and drain its connections. fit swaps
listeners on every reload and Admin API change (fib's `AddBinding`, `RemoveBinding`,
`ShutdownBinding`).

**Evidence.** `crates/akio/src/lib.rs`: `EngineBuilder::listen`, `Engine::bind_udp`,
`Engine::adopt_tcp` (serves a stream accepted elsewhere).

**Suggested API.** `Engine::add_listener(Listener) -> io::Result<ListenerId>`,
`Engine::remove_listener(id)` (stop accepting), `Engine::shutdown_listener(id, drain: Duration)`
(stop accepting, close its connections after the drain), with the listener's backend and accel
choices as at build time.

**How akit uses it.** I22 and I28 diff listeners on each swap. Until then, a stopgap: a listening
socket of akit's own with one accept thread handing streams to `adopt_tcp`, recorded in progress.

## A2 Certificates chosen per handshake, asynchronously

**Problem.** `ServerConfigBuilder::sni_cert` fixes the certificates when the config is built.
Automatic HTTPS replaces them as they renew, and on-demand TLS must issue one during the handshake
without blocking the I/O thread.

**Suggested API.** A resolver on the server config, `fn resolve(&self, hello: &ClientHello, done:
CertDone)`, where `done` may be completed later from another thread with a certificate or an alert;
the handshake waits without holding the thread. A synchronous fast path for a cached certificate.

**How akit uses it.** I30 serves certificates from its manager; I34 issues on demand. I09 picks
certificates from files with `sni_cert`, which looks a name up exactly: a wildcard name
(`*.example.com`) in a certificate other than the first, the default, is refused as not supported
yet until the resolver (whose `CertStore` takes `*.` names) is on akio's `main`.

## A3 A filter on accepted connections

**Problem.** fit's allow and deny lists refuse a client at accept, before any handler or TLS work
(fib `Binding.Filter`, A73 and A74 in fit's list).

**Suggested API.** `Listener::filter(Fn(&SocketAddr) -> bool)`, called on the I/O thread before the
acceptor, with a counter of refused connections; swappable at run time.

## A4 PROXY protocol

**Problem.** Layer 4 and HTTP listeners behind a load balancer take the client address from a PROXY
header; upstream connections may send one.

**Suggested API.** A listener option that reads and checks a v1 or v2 header (with a timeout and a
size limit) before the acceptor runs and exposes the addresses on `Conn`; a dial option that writes
one.

## A5 SNI routing in akio-mux

**Problem.** `akio-mux` has `Rule::Tls`, which matches any ClientHello. TLS passthrough needs the
server name, read from a ClientHello that may span several reads, without terminating TLS.

**Suggested API.** `Rule::TlsSni(matcher)` or a decision callback given the parsed SNI and ALPN,
with the sniff limits `Mux` already has.

## A6 CSR and JWS

**Problem.** akit's ACME client needs a PKCS#10 CSR for its keys and JWS signatures (ES256, RS256).

**Done** (akio ADR-093): `akio_x509::CsrParams::sign` makes the request and
`CertificateRequest` parses and checks one. JWS needed nothing new: `akio_crypto::pkey::PrivateKey::sign`
already gives ES256/384/512, EdDSA and RS256 in the raw forms JWS uses.

## A7 An MP4 reader

**Problem.** fit's media library cuts HLS and fMP4 segments from MP4 files without transcoding (fib
A64). Needed when the media library track starts.

## A8 TUN devices

**Problem.** fit's VPN track runs TUN interfaces on the engine. Needed when that track starts in
akit, after fit's.

## A9 A TLS server capped at TLS 1.2

**Problem.** fit's `listeners[].tls.max_version` takes `1.2`, for network cards that offload TLS
1.2 alone (Go's `tls.Config.MaxVersion`). akio-tls's `ServerConfigBuilder` has `tls13_only`, but
caps a server at TLS 1.2 only with `__tls12_only`, hidden and marked for tests and interop tools.

**Evidence.** `crates/akio-tls/src/config.rs` at `50a82315`: `ServerConfigBuilder::tls13_only`,
`ServerConfigBuilder::__tls12_only` (`#[doc(hidden)]`).

**Suggested API.** `ServerConfigBuilder::max_version(TlsVersion)` (and `min_version`), or a public
`tls12_only(bool)`; the same on `ClientConfigBuilder`.

**How akit uses it.** I09 reads `max_version` and refuses `1.2` as not supported yet; with the API,
akit-tls sets it from the listener's `tls`.

## A10 Closing an HTTP client's idle connections

**Problem.** When a graph is retired, its upstream pools drain and close (I14, decision 004). Once
the last request is done, the connections akio-http's `Client` keeps idle should close too, as
fit's pool closes fib's client. akio's client has no way to close them: they close only after
`ClientOptions::idle_timeout` (fit's default 60s), when the server closes them, or when the engine
shuts down; dropping the `Client` does not close them, since each idle connection's handler holds
a clone of it.

**Evidence.** `crates/akio-http/src/client.rs` at `50a82315`: `Pool` (the idle connections) is
private to `Inner`; `ClientConn` keeps `client: Option<Client>`; only `DialAcceptor::on_shutdown`
and the idle sweep close idle connections.

**Suggested API.** `Client::close_idle(&self)`: closes every idle connection now and keeps
connections that are in use from going back idle (each closes once its response is in), as Go's
`Transport.CloseIdleConnections` and the engine's shutdown do. Never blocks.

**How akit uses it.** `Pool::close` calls it once the pool has drained. Until then, a closed pool's
idle connections stay open up to its `idle_timeout` (recorded in the I14 progress entry).

## A11 A request's own `Host` field

**Problem.** A reverse proxy passes the client's `Host` on to the upstream unless told to rewrite
it (fit's proxy, I15). akio-http's HTTP/1 client always writes `Host` from the request URL and
drops a `Host` field the request carries.

**Evidence.** `crates/akio-http/src/client.rs` at `50a82315`, `Client::attempt`: the head is
written with `Host: {host_header}` from `parse_url`, and request fields named `host` are skipped.

**Suggested API.** A `Host` field set on `ClientRequest::headers` replaces the one taken from the
URL (the URL still says where to connect), or a `ClientRequest::host(name)` setter.

**How akit uses it.** I15's proxy sends the client's `Host`, or the one its params set, through
the upstream pool.

**Done (I15).** On akio branch `claude/i15-proxy-699f18`, commit `552fe283`, not merged: a `Host`
field in `ClientRequest::headers` is sent in place of the URL's (the URL still says where to
connect and which name TLS asks for), kept on a redirect to the same origin and dropped on one
to another. With it, for fit's proxy, which sends no `User-Agent` its client did not send: an
empty `User-Agent` field sends none, and akio's default (`akio-http`) is not added, as Go's
client takes it. akio-http's tests pass on macOS and on Linux (epoll and io_uring). akit's I15
uses both; its tests of them are ignored until akit's pin moves to an akio commit with A11.

## A12 The HTTP client's connect timeout for every connect

**Problem.** fit bounds opening a connection to an upstream by its `connect_timeout`, per
upstream. akio-http's client takes one through `ClientOptions::happy_eyeballs.connect_timeout`, but
the Happy Eyeballs racer applies it only when a name resolves to two addresses or more: a lone
address (an IP literal, a name with one record), or a race the racer is too busy to take, is dialed
with `Engine::dial`, bounded by the engine's `Config::connect_timeout` instead.

**Evidence.** `crates/akio/src/dns.rs` at `50a82315`: `HappyEyeballs::dial` returns
`engine.dial(addrs[0], ...)` for one address, and `in_turn` dials with `engine.dial` for
`Outcome::Busy`.

**Suggested API.** `Engine::dial_timeout(addr, timeout, acceptor, on_error)` (or a timeout on
`dial`), used by `HappyEyeballs::dial` for every attempt, so that its `connect_timeout` holds for
any number of addresses.

**How akit uses it.** Each upstream's pool sets its own. Until then, `akit_listener::engine` sets
the engine's `connect_timeout` to the shortest of the upstreams', which is each one's when they
agree (recorded in the I14 progress entry).

## A13 `Files` serving a path the caller names

**Problem.** fit's `static` handler looks up the request path minus its `strip_prefix` under the
root, and akit's listener routes by the request's canonical path (dot segments removed, I11),
which it keeps in a context local, since akio's request target cannot be rewritten. akio-http's
`Files` reads the path from the route parameter `path` (set by akio's own `Router` only) or from
the request target as sent, so akit can give it neither.

**Evidence.** `crates/akio-http/src/files.rs` at `50a82315`, `Files::relative`:
`ctx.param_raw("path")`, else `ctx.path()`; `Context::set_params` is `pub(crate)`.

**Suggested API.** `Files::serve(&self, ctx: &mut Context, path: &str)`, the `Service::call` body
with the path given (still percent-decoded and checked by `Files`), or a public way to set a
request's route parameters.

**How akit uses it.** The `static` handler passes the canonical path minus `strip_prefix`; until
then `strip_prefix` is refused as not supported yet, and a path with a `..` segment that stays
inside the root (`/x/../ok.txt`), which the listener routes as `/ok.txt` and fit's stack serves, is
answered 400 by the handler, as fit's handler alone answers it.

## A14 `Files` answering as a gateway's static handler does

**Problem.** fit's `static` handler (whose tests akit ports, rule 19) answers some requests
otherwise than `Files`, which has no setting for these:

- a list of index files tried in order, or none (`index: []`); `Files` takes one name;
- a directory asked for without its trailing slash is redirected (301, `Location` with the slash,
  the query kept), so that relative links resolve; `Files` serves the index file;
- a file asked for with a trailing slash (`/a.txt/`) is 404; `Files` drops empty segments and
  serves it;
- compressed twins (`.br`, `.zst`, `.gz`) are served only for the encodings the config lists
  (`precompressed`, none by default), with `Vary: Accept-Encoding` only then; `Files` always
  serves a twin it finds and always adds `Vary`;
- the content type from a full table (Go's `mime.TypeByExtension`: `.xml` is
  `text/xml; charset=utf-8`, `.avif` is known, and on Linux `/etc/mime.types` adds more), a configured `default_type`, else sniffed from
  the first 512 bytes; `Files` has a fixed table of 22 extensions and `application/octet-stream`
  for the rest;
- 403 for a file that may not be read; `Files` answers 404 for any `stat` error;
- only regular files: `Files` opens anything that is not a directory, so a FIFO under the root
  holds a thread of its disk pool until a writer comes (akit's test
  `a_hung_disk_holds_no_io_thread` relies on it);
- a name with `:` is a name like others; `Files` answers 404 for it;
- error bodies as `"404 Not Found\n"`; `Files` sends `"Not Found"`.

**Evidence.** `crates/akio-http/src/files.rs` at `50a82315`: `Inner::index: String`,
`Files::relative` (empty segments filtered, `:` refused), `Inner::serve_disk` (index served for a
directory; `fs::metadata` errors answered 404), `Inner::load_tracked` (`TWINS` always looked up),
`Cached::new` (`Vary` always), `Files::mime`, `Inner::read` (`fs::read` on any non-directory).

**Suggested API.** Builder settings on `Files`: `index_files(&[&str])`,
`redirect_directories(bool)`, `twins(&[&str])` (the encodings, in order; none turns the lookups
off), `mime(fn(&Path) -> Option<&'static str>)` or a table, `default_type(&str)` with
`sniff(bool)`; and fixed: 403 on `PermissionDenied`, 404 for what is not a regular file or a
directory, a file with a trailing slash 404, `:` allowed, and `error_body(fn(u16) -> Response)`
for the short answers.

**How akit uses it.** The `static` handler sets them from fit's params (`index`, `precompressed`,
`default_type`) and fit's fixed behaviour; until then the differences are recorded in the I12
progress entry, its tests check akit's present answers naming this request, and `index` lists of
other than one name and `default_type` are refused as not supported yet.

## A15 `Files` bounded in memory, its disk pool with a timeout

**Problem.** fit keeps at most `max_cache_bytes` (64 MiB by default) of files in memory, and a
request waiting on its file pool past the pool's timeout is answered 503, so a hung disk is not
waited on forever (rule 4). `Files` bounds the size of each file it keeps (`max_cached_bytes`) and
the open handles of larger ones (`max_open_files`), but not the total kept: every file under the
root up to that size can end up in memory; and a request queued on its disk pool (16 threads, 4096
queued) waits for as long as the disk takes.

**Evidence.** `crates/akio-http/src/files.rs` at `50a82315`: the `shards` maps grow with each file
read (`Inner::load_tracked`); `disk_pool()` is a `Pool` with `max_threads` 16 and
`queue_capacity` 4096, and `try_execute` refuses only a full queue.

**Suggested API.** `Files::max_cache_bytes(n)` (the in-memory entries, evicted least recently used
first, as the open handles are), and `Files::disk_timeout(d)`: a request not answered from the disk
pool within `d` is answered 503 with `Retry-After`, and its job, when it runs later, answers
nothing.

**How akit uses it.** The `static` handler sets fit's `max_cache_bytes` (and default) and the pool
timeout; until then `max_cache_bytes` is refused as not supported yet, and the memory `Files` keeps
is bounded only by the files under the root of at most `max_file_bytes` (recorded in the I12
progress entry).

## A19 An HTTP client request whose body is sent as it is produced

**Problem.** A reverse proxy sends the client's request body to the upstream as it arrives (rule
5), never whole, and stops taking more from the client while the upstream is behind (rule 8).
akio-http's HTTP/1 client takes a request body only whole: the proxy would have to hold every
upload in memory, up to `limits.max_body_bytes` (16 MiB by default) per request, before the
upstream sees a byte.

**Evidence.** `crates/akio-http/src/client.rs` at `50a82315`: `ClientRequest::body` is a `Vec<u8>`;
`Client::request` encodes the head and the whole body into one buffer (`Pending::bytes`) and
`ClientConn::begin` sends it with one `send_owned`. There is no chunked request body and no way to
learn that the connection's write queue drained. (akio-core has what is needed below the client:
`Conn::on_drain` and the write high watermark, `conn.rs`.)

**Suggested API.** A body that is a stream rather than bytes:

- `ClientRequest::streamed(len: Option<u64>)` (or `ClientBody::Stream { len }`): the body is
  written later, framed by `Content-Length` when `len` is known and chunked otherwise.
- `Client::request_streamed(req, on_ready, done)`: `on_ready(RequestBody)` is called on the
  connection's I/O thread once the head is written (after a connect, a queue for a pooled
  connection, a reused connection found dead and retried; a streamed request is never retried
  once its body has begun, as Go's client does not retry one). `RequestBody` is `Send` and has
  `write(&[u8]) -> Result<(), Closed>` (copies into the connection's queue, chunk-framed when
  chunked), `queued_bytes()`, `on_drain(f)` (once the queue falls below the low watermark, `f`
  runs on the I/O thread), `finish()` (the last chunk, or a check that `len` bytes were written)
  and `finish_with_trailers(Headers)`; dropping it unfinished closes the connection and fails the
  request with `ClientError::Closed`. `done` gets the response as today, or with A20 its head.
- A response may begin, or come whole, before the body is finished (an upstream answering 413
  early): `done` then runs and later writes fail.

**How akit uses it.** I16's proxy takes the client's body as a stream (`BodyMode::Stream`,
`on_body`), writes each chunk to the upstream's `RequestBody`, and holds the client's reads (A21)
while `queued_bytes` is above the watermark, resuming them from `on_drain`. Also the upstream
pools (`akit-upstream`, which wraps `Client::request`), I17's cancellation (dropping the body),
I18's h2 upstreams (the same on `H2Client`), I49 (WebSocket) and I75 (FastCGI). Until then akit's
proxy forwards bodies whole, within `limits.max_body_bytes` (recorded in the I16 progress entry).

## A20 An HTTP client response delivered as it arrives

**Problem.** A reverse proxy sends the upstream's response back as it arrives and stops reading
the upstream while the client is behind (rules 5 and 8). akio-http's client buffers each response
body whole (up to `ClientOptions::max_body_bytes`, 64 MiB by default) and calls back once: a
download through the proxy is held in memory whole, a response past 64 MiB fails, and a slow
client cannot slow the upstream down.

**Evidence.** `crates/akio-http/src/client.rs` at `50a82315`: `ClientConn` appends every body byte
to `ClientResponse::body` (`RState::Body`, `RState::Chunked`, `RState::UntilClose`) and calls the
callback from `complete`; reads on the connection are never held.

**Suggested API.** `Client::request_with(req, handler: impl ResponseHandler)` (and on `H2Client`):

- `on_head(&mut self, head: ResponseHead, body: ResponseBody)`: the status, reason and fields once
  the head is in (interim 1xx responses before it, or skipped as today); `ResponseBody` is `Send`
  and has `hold_reads(bool)` (stops and resumes reading the connection, or for h2 the stream's
  flow-control window) and `cancel()` (closes the connection, or resets the stream).
- `on_data(&mut self, chunk: &[u8])`: each piece of the body as it is read, dechunked, valid for
  the call only (rule 6).
- `on_end(&mut self, trailers: &Headers)` and `on_error(&mut self, e: ClientError)`; with a
  timeout of its own for the gap between reads (fit's `response_idle_timeout`, I17).
- `ClientOptions::max_body_bytes` does not apply to such a response; the handler decides.
- The keep-alive pool takes the connection back after `on_end`, as today after the callback.

**How akit uses it.** I16's proxy starts the client's response from `on_head`
(`Detached::stream_response`), writes each chunk to its `BodyStream`, and calls
`hold_reads(true)` while the client's `queued_bytes` is above the watermark, `false` from the
client's drain (A21). Also I17 (trailers, idle timeouts, cancellation), I18 (h2 upstreams) and the
cache (I67), which stores a response as it passes. Until then akit's proxy sends responses back
whole, each at most akio's 64 MiB (recorded in the I16 progress entry).

## A21 A server request that holds its client's reads and hears when its client drained

**Problem.** The other half of linked backpressure (rule 8) is the client's side of a proxied
request: while the upstream is behind, stop reading the client's body; while the client is behind
on the response, hear when it caught up, to resume the upstream. akio-http's server gives a
streamed request body to `on_body` as fast as the client sends it, and a streamed response
(`BodyStream`) can only be polled (`queued_bytes`); neither the context nor the stream reaches
the connection's `hold_reads` or `on_drain`.

**Evidence.** `crates/akio-http/src/context.rs` at `50a82315`: `Context` and `Detached` have no
access to the connection; `BodyStream` has `write`, `queued_bytes` and `finish`.
`crates/akio-http/src/session.rs`, `State::Stream`: each read is delivered to the sink at once.
`crates/akio-http/src/h2.rs`, `on_data`: the stream's receive window is replenished as DATA arrives
("bodies are consumed immediately"), whether the sink keeps up or not. akio-core has both halves
per connection (`ConnInner::hold_reads`, `ConnInner::on_drain`, `crates/akio-core/src/conn.rs`), as
does `akio_http::raw` for upgraded connections.

**Suggested API.**

- `Context::body_reads() -> BodyReads` (for `BodyMode::Stream`; `Send`, kept by the handler after
  `detach`): `hold(bool)`. On HTTP/1 it holds the connection's reads (and so the pipelined requests
  behind it, as it must); on h2 and h3 it stops replenishing the stream's flow-control window, so
  that the other streams go on.
- `BodyStream::on_drain(f)`: `f` runs on the connection's I/O thread once the response's queued
  bytes fall below the low watermark (at once if they are already), or when the connection
  closes; on h2 and h3, when the stream's send window and the connection's queue allow more.
- A body past a limit while streaming: akio already lets the handler answer early or drop the
  `Detached` (closing the connection); no more is needed.

**How akit uses it.** I16's proxy holds the client's body reads while the upstream's
`RequestBody::queued_bytes` (A19) is above the watermark, and resumes them from its `on_drain`;
it holds the upstream's reads (A20) while the client's `BodyStream::queued_bytes` is above it, and
resumes them from `BodyStream::on_drain`. Also compression (I47) and FastCGI (I75). Until then akit
buffers proxied bodies whole, within the limits (recorded in the I16 progress entry).
