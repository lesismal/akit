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
| A1 | TCP listeners added, removed and drained on a running engine | in akio's main (`a9a620ab`), pinned, used by I22 | 10, 11 | I22, I28 |
| A2 | A server certificate chosen per handshake by a callback that may complete later | in akio's main (`a9a620ab`), pinned | 1, 4 | I30, I34 |
| A3 | A filter on accepted connections before any handler runs | in akio's main (`a9a620ab`), pinned | 11 | access lists |
| A4 | PROXY protocol v1/v2: parse on accept, write on dial | in akio's main (`a9a620ab`), pinned, used by I52 | 12 | I52 |
| A5 | An akio-mux rule that routes TLS by SNI without terminating it | in akio's main (`a9a620ab`), pinned, its ClientHello parser used by I52 | 12 | I52, I53 |
| A6 | CSR generation in `akio-x509` (and JWS helpers in `akio-crypto`) | in akio's main (`afbbb521`), pinned | 15 | I31 |
| A7 | An MP4 (ISO BMFF) reader for segmenting files without transcoding | in akio's main (`a9a620ab`), pinned | — | media library |
| A8 | TUN devices | in akio's main (`a9a620ab`), pinned | — | VPN |
| A9 | A TLS server capped at TLS 1.2 (public `ServerConfigBuilder` API) | requested (I09) | 17 | I09 |
| A10 | Closing an HTTP client's idle connections now | requested (I14) | 10, 11 | I14, I22 |
| A11 | An HTTP/1 client request that keeps its own `Host` field (and sends no `User-Agent` when given an empty one) | in akio's main (`a9a620ab`), pinned | — | I15 |
| A12 | The HTTP client's connect timeout for a lone address too | requested (I14) | 10 | I14, I35 |
| A13 | `Files` serving a path the caller names | in akio's main (`a9a620ab`), pinned | 19 | I12, I13 |
| A14 | `Files` answering as a gateway's static handler does | requested (I12) | 19 | I12, I13 |
| A15 | `Files` bounded in memory, its disk pool with a timeout | requested (I12) | 4, 11 | I12 |
| A16 | `Files` choosing a compressed twin by the client's q values, among the encodings configured | in akio's main (`a9a620ab`), pinned | 19 | I13 |
| A17 | `Files` trying candidate paths in turn (`try_files`) | in akio's main (`a9a620ab`), pinned | 19 | I13 |
| A18 | `Files` listing a directory that has no index file | in akio's main (`a9a620ab`), pinned | 4, 11, 19 | I13 |
| A19 | An HTTP client request whose body is sent as it is produced, with backpressure | in akio's main (`60547435`), pinned, used by I16 | 5, 8 | I16, I17, I18, I49, I75 |
| A20 | An HTTP client response delivered as it arrives, its reads held and resumed | in akio's main (`60547435`), pinned, used by I16 | 5, 8 | I16, I17, I18, I67 |
| A21 | A server request that holds its client's reads and hears when its client drained | in akio's main (`60547435`), pinned, used by I16 | 5, 8 | I16, I47, I75 |
| A22 | UDP listeners removed and drained on a running engine | requested (I22) | 10, 11 | I22, I28 |
| A23 | Unix-domain stream listeners (with a socket file mode) | requested (I26) | 11 | I26, I29 |
| A24 | An HTTP/1 response that closes its connection, and stops reading a body it refused | requested (I26) | 7, 10, 11 | I26, I27, I41 |
| A29 | A response hook without an allocation: a listener's observer, called with a few words the request set | requested (I37) | 13 | I21, I37, I64 |
| A30 | The open connections a listener accepted, apart from dialed ones and UDP peers | requested (I37) | — | I37 |
| A32 | Where an `akio-regex` pattern matches, its groups, and replacing them | in akio's main (`e9891b52`), pinned | 19 | I40, I48 |
| A33 | A request's fields and target changed by a middleware (`Context::req_mut`) | requested (I40) | 12, 19 | I40, I43, I44 |
| A35 | A streamed HTTP/1.1 response framed by its length | requested (I16) | 5 | I16, I67 |
| A36 | An HTTP client request's response-header and idle timeouts | in akio's main (`4f7169c4`), pinned, used by I17b | 10 | I17b, I18 |
| A37 | An HTTP client request cancelled at any point | in akio's main (`4f7169c4`), pinned, used by I17b | 3 | I17b, I45 |
| A38 | A detached server request that hears its client went; a write stall timeout changed at run time | in akio's main (`4f7169c4`), pinned, used by I17b | 3, 10 | I17b, I49 |
| A45 | A public timer: a callback run once a delay has passed, from any thread | requested (I35) | 1, 11 | I35, I31, I52, I54 |
| A48 | A request's end heard: its response sent in full, or its client gone while it waits | requested (I41) | 3, 11, 13 | I41, I17, I21, I37 |
| A49 | A request body limited by a middleware, whoever reads the body | requested (I41) | 7, 11 | I41 |
| A51 | `H2Client`'s streamed requests: reset, timeout, request trailers, and failed rather than forgotten on a `GOAWAY` | in akio's main (`f6750e93`, ADR-110), pinned, used by I18b 3, 10 | I18b, I50 |
| A52 | `H2Client` backpressure per stream: a streamed response's reads held, an upload's queue reported | in akio's main (`f6750e93`, ADR-110), pinned, used by I18b 8 | I18b, I50 |
| A53 | `H2Client`'s connection: the server's stream limit heeded and reported, the connect timeout, ALPN's HTTP/1.1 fallback | in akio's main (`f6750e93`, ADR-110), pinned, used by I18b 10, 11 | I18b, I54 |
| A54 | An idle HTTP/1 connection drained after a grace, not at once | requested (I28) | 10 | I28 |
| A57 | Root certificates usable outside a handshake: the system's, to verify a chain against | requested (I32) | 1, 19 | I32 |
| A58 | The ALPN protocol a TLS connection negotiated, seen by an HTTP service (or such a connection closed) | requested (I32) | 19 | I32, I34 |
| A63 | bcrypt password hashes in `akio-crypto` | in akio's main (`4f7169c4`), pinned | 1, 4 | I42 |
| A64 | The client's verified certificate on each server request | in akio's main (`4f7169c4`), pinned | — | I42 |
| A65 | A request suspended by a middleware and resumed with its context | in akio's main (`4f7169c4`), pinned | 1, 3, 4 | I42, I43 |
| A66 | `H2Client`'s requests heeding `response_header_timeout`, `response_idle_timeout` and a `CancelToken` | in akio's main (`d27e6a20`, ADR-111), pinned, used by I18b | 3, 10 | I17b, I18b |
| A75 | A request whose body streams suspended, its body's reads held until it is resumed | requested (I43) | 5, 7, 8 | I42, I43, I44 |
| A78 | An idle timeout per connection, TCP or a UDP peer, set by its handler | requested (I51) | 10, 11 | I51, I55 |
| A79 | `Engine::dial` and `dial_udp` with a connect timeout of their own | requested (I51) | 10 | I51, I54, I55 |
| A90 | A streamed response's coding set by a response filter of the caller's own, and a value kept until the response is sent | in akio's main (`c5cf4353`), pinned | 8, 11 | I47 |
| A91 | `Files` keeping compressed copies of the files it holds in memory | in akio's main (`c5cf4353`), pinned | 4, 12 | I47 |
| A92 | A body sent from a file on disk encoded by a stream coding | requested (I47) | 1, 5 | I47 |
| A93 | The HTTP/1 client hands a connection over on `101 Switching Protocols` | in akio's main (`c5cf4353`), pinned | 8, 9 | I49, I55 |
| A94 | A streamed `2xx` to an HTTP/2 extended CONNECT is a tunnel whose halves close separately | in akio's main (`c5cf4353`), pinned | 9 | I49 |
| A105 | A request's peer is the client a trusted PROXY header names (akio-http) | in akio's main (`e9891b52`), pinned | 19 | I52 |
| A108 | The HTTP/1 client's TLS server name, in place of the URL's host | in akio's main (`e9891b52`), pinned | 19 | I54 |
| A109 | A client certificate's key checked against its leaf when the TLS configuration is built | in akio's main (`e9891b52`), pinned | 19 | I54 |
| A111 | A response filter sees a streamed body as it is sent, and how it ended (a body tap) | in akio's main (`e9891b52`), pinned | 5, 6 | I67 |
| A112 | An HTTP-date parsed in all three formats RFC 9110 has recipients accept | built on akio's branch `claude/a111-body-tap` (`0e36f8d6`), pinned | 15 | I67 |
| A113 | A response filter that runs after the inner ones (an outer filter) | built on akio's branch `claude/a111-body-tap` (`0e36f8d6`), pinned | 5 | I67 |
| A117 | `akio::Pool` and `PoolConfig` together: a bounded pool of an application's own for its blocking work | requested (I68); akit-cache depends on `akio-pool` meanwhile | 4 | I68 |

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

**How akit uses it.** I22 and I28 diff listeners on each swap. I22 does on `a9a620ab`
(`akit_listener::Listeners::reload`): `add_listener` before the swap, `shutdown_listener` with
the shutdown timeout after it; the `adopt_tcp` stopgap it used before is gone. A UDP listener
cannot be removed yet (A22).

## A2 Certificates chosen per handshake, asynchronously

**Problem.** `ServerConfigBuilder::sni_cert` fixes the certificates when the config is built.
Automatic HTTPS replaces them as they renew, and on-demand TLS must issue one during the handshake
without blocking the I/O thread.

**Suggested API.** A resolver on the server config, `fn resolve(&self, hello: &ClientHello, done:
CertDone)`, where `done` may be completed later from another thread with a certificate or an alert;
the handshake waits without holding the thread. A synchronous fast path for a cached certificate.

**How akit uses it.** On `a9a620ab` (ADR-098), I30: every TLS listener's configuration is built
with `ServerConfig::builder_with_resolver`, and its resolver, `akit_runtime::ListenerCerts`,
answers inline from the current graph's `akit_tls::Certs` (fit's choice: exact or wildcard name,
then what the client supports, from `ClientHelloInfo`'s signature schemes and suites), so a
reload or `-watch-certs` replaces certificates without closing the listener, and a wildcard works
in any certificate (I09's `sni_cert` stopgap refused one past the first). I34 will answer
`CertDone` later, from the issuing thread.

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

**Done** (akio ADR-107, numbered 093 on its branch): `akio_x509::CsrParams::sign` makes the request and
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

**Done** (akio ADR-100, in akio's main at `a9a620ab`, pinned): A13 as `Files::serve(ctx, path)`;
A16 as `Files::twins(&[&str])` and a twin picked by q value; A17 as `Files::try_paths(&[&str])`
with `Files::hide(f)` for fit's dotfiles; A18 as `Files::list_directories(max_entries)`, fit's page
byte for byte. akit's `static` handler uses them since I13: it serves the canonical path (so
`/x/../ok.txt` is served, as fit's stack serves it), and `strip_prefix`, `precompressed`,
`try_files`, `browse` and `browse_max_entries` are supported.

## A14 `Files` answering as a gateway's static handler does

**Problem.** fit's `static` handler (whose tests akit ports, rule 19) answers some requests
otherwise than `Files`, which has no setting for these:

- a list of index files tried in order, or none (`index: []`); `Files` takes one name;
- a directory asked for without its trailing slash is redirected (301, `Location` with the slash,
  the query kept), so that relative links resolve; `Files` serves the index file;
- a file asked for with a trailing slash (`/a.txt/`) is 404; `Files` drops empty segments and
  serves it;
- (done with A16: `Files::twins`, which akit sets from `precompressed`) compressed twins only for
  the encodings the config lists, with `Vary: Accept-Encoding` only then;
- the content type from a full table (Go's `mime.TypeByExtension`: `.xml` is
  `text/xml; charset=utf-8`, `.avif` is known, and on Linux `/etc/mime.types` adds more), a configured `default_type`, else sniffed from
  the first 512 bytes; `Files` has a fixed table of 22 extensions and `application/octet-stream`
  for the rest;
- 403 for a file that may not be read; `Files` answers 404 for any `stat` error;
- only regular files: `Files` opens anything that is not a directory, so a FIFO under the root
  holds a thread of its disk pool until a writer comes (akit's test
  `a_hung_disk_holds_no_io_thread` relies on it);
- a name with `:` is a name like others; `Files` answers 404 for it;
- error bodies as `"404 Not Found\n"`; `Files` sends `"Not Found"` (also for a path no
  `try_files` candidate finds);
- a listing below `strip_prefix` is titled by the request's whole path (`Index of /assets/`, with a
  `../` link); `Files::serve` titles it by the path it is given (`Index of /`, no `../`): a title
  (or prefix) given alongside the path would do (I13).

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

## A16 A compressed twin chosen by q values, among the encodings configured

**Problem.** fit's `static` handler serves a file's `.br`, `.zst` or `.gz` twin only for the
encodings `precompressed` lists, and picks the one the client prefers by the q values of its
`Accept-Encoding` (RFC 9110 section 12.5.3), among equals the first listed; a coding with `q=0` is
refused, `*` stands for the codings the field does not name, and codings are named in any case.
`Files` takes the first of `br`, `zstd`, `gzip` (its fixed order) whose token the field holds,
whatever its q value: `br;q=0` gets the `br` twin, which the client refused; `br;q=0.5, gzip` gets
`br`; `*` gets none; `GZIP` and `gzip ;q=1` (a blank before the parameters) get none.

**Evidence.** `crates/akio-http/src/files.rs` at `50a82315`: `pick` splits the field on `,`, then
compares `tok.trim().split(';').next()` with `TWINS`' names, case-sensitively and without trimming
the name or reading `q`; `TWINS` is a fixed `[("br", "br"), ("zstd", "zst"), ("gzip", "gz")]`.
akit's test `precompressed` (`crates/akit-handler/tests/static_files.rs`) checks these answers.

**Suggested API.** With A14's `twins(&[&str])` (the encodings, in order of preference among
equals; none turns the lookups and `Vary` off), `pick` ranks the twins on disk by their q value
(the coding's own, else `*`'s, 0 refusing it), case-insensitively, the configured order breaking
ties, and serves the original when none has a q above 0.

**How akit uses it.** The `static` handler passes `precompressed`'s list. Until then akit takes
`precompressed` and `Files` serves the twins it finds as above; the differences are in the I13
progress entry, and the test names this request for each.

## A17 Candidate paths tried in turn (`try_files`)

**Problem.** fit's `try_files` serves the first of a list of paths that exists, `{path}` standing
for the request's path and a path ending in `/` for a directory served by its index file (or its
listing, A18); none existing is 404, and a directory without its slash is not redirected then.
`["{path}", "{path}/", "/index.html"]` serves a single-page application. A candidate with a `..`
segment, or a dotfile `dotfiles` `deny` hides, is passed over. `Files` serves only the path the
request names, and its 404 is final: akit cannot ask it whether a path exists, nor make it serve
another path (A13), without doing on the I/O thread the disk work `Files` does on its pool.

**Evidence.** `crates/akio-http/src/files.rs` at `50a82315`: `Service::call` takes the path from
`Files::relative` (the route parameter `path` or the request target) and answers 404 itself when
`Inner::serve_disk` finds nothing; no method takes a path or reports a miss to the caller.

**Suggested API.** `Files::try_paths(&[&str])`: candidate templates with a `{path}` placeholder,
each resolved on the disk pool in turn (and cached, as a path is, with what it resolved to), the
first that is a file, or a directory with an index file, served; a candidate that is unsafe
(`..`, NUL, `\`), or that a hook refuses (`hide(fn(&str) -> bool)`, for fit's dotfiles), passed
over; 404 when none is. Or, more general: `Files::serve_first(ctx, &[PathBuf])` with A13's
`serve`.

**How akit uses it.** The `static` handler passes `try_files` and its dotfile check. Until then
akit checks `try_files` as fit does (each starts with `/` or `{path}`; with `dotfiles` `deny`, none
names a dotfile) and refuses it as not supported yet, naming A13 and this request.

## A18 A directory listing for a directory without an index file

**Problem.** fit's `browse` lists a directory asked for with its trailing slash that has no index
file: an HTML page (`Index of /files/`, a `../` link but at the root, directories first, then by
name, each name HTML-escaped and its `href` a relative, percent-escaped segment, size and
modification time in UTC), at most `browse_max_entries` entries read (10000 by default; more read
says so in a last paragraph), names starting with a dot left out whatever `dotfiles` says,
`Content-Type: text/html; charset=utf-8` and `Cache-Control: no-cache`, read on the file pool every
time. `Files` answers 404 for a directory without its index file, from its disk pool, and akit
cannot tell that 404 from others nor list the directory there.

**Evidence.** `crates/akio-http/src/files.rs` at `50a82315`: `Inner::serve_disk` returns
`Response::text(404, "Not Found")` when `fs::metadata(dir/index)` fails or is not a file.

**Suggested API.** `Files::list_directories(max_entries)` (off by default), answering as above on
the disk pool, the entries bounded before they are sorted (rules 4 and 11); or a hook
`on_directory(fn(&Path, &Request) -> Response)` run on the disk pool for a directory without an
index file, for the caller to build the page.

**How akit uses it.** The `static` handler turns it on for `browse` with `browse_max_entries`.
Until then `browse` `true` is refused as not supported yet naming this request; `browse` `false`,
fit's default, and `browse_max_entries` are taken.

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
I18's h2 upstreams (the same on `H2Client`), I49 (WebSocket) and I75 (FastCGI). akio built it in
`60547435` (ADR-101: `ClientRequest::streamed`, `RequestBody`, its `on_drain` on the connection's
empty queue); I16 does as above through `akit_upstream::Pool::stream`.

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
cache (I67), which stores a response as it passes. akio built it in `60547435` (ADR-101:
`Client::request_with`, `ResponseHandler`, `ResponseBody`; not yet on `H2Client`, nor the idle
timeout between reads, which come with I18 and I17); I16 does as above.

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
resumes them from `BodyStream::on_drain`. Also compression (I47) and FastCGI (I75). akio built it in
`60547435` (ADR-101: `Context::body_reads`, `BodyReads::hold`, `BodyStream::on_drain`, on
HTTP/1.1, HTTP/2 and HTTP/3); I16 does as above.

## A22 Removing a UDP listener of a running engine

**Problem.** A1 adds, removes and drains TCP listeners at run time; a UDP listener given to the
builder cannot be removed (`Engine::remove_listener` answers `Unsupported`). akit serves HTTP/3
on such a listener, so a reload (or an Admin API change) that removes or changes a listener
serving `h3` that akit started with is refused until a restart. HTTP/3 added at run time goes on
`Engine::bind_udp`, whose `UdpBinding::close` closes its connections at once, without a drain.

**Evidence.** `crates/akio/src/lib.rs` at `a9a620ab`: `Engine::unlisten` refuses every kind but
`Kind::Tcp`; `Engine::add_listener` refuses UDP (`bind_udp` binds UDP at run time);
`crates/akio-core/src/group.rs` `UdpBinding::close`.

**Suggested API.** `Engine::remove_listener` and `Engine::shutdown_listener(id, drain)` for UDP
listeners too (per-peer UDP: stop receiving new peers, call `on_shutdown` on the existing ones,
close those left at the deadline), and `Engine::add_listener` for a UDP listener; or
`UdpBinding::shutdown(drain)` with the same meaning.

**How akit uses it.** `akit_listener::Listeners::reload` (I22, I28) removes and drains a
listener's HTTP/3 as it does its TCP side, and the `ReloadError::Udp` refusal goes.

## A23 Unix-domain stream listeners

**Problem.** fit's Admin API listens on a Unix socket when `admin.address` is `unix/<path>`
(made 0600, a stale socket file replaced, one in use refused), so that only the owner's processes
reach it, with no port and no token. akio's listeners take a `SocketAddr` only: TCP or UDP. akit
refuses `unix/` addresses as not supported yet until akio serves them.

**Evidence.** `crates/akio/src/lib.rs` at `a9a620ab`: `Listener::tcp(SocketAddr)`,
`Listener::tcp_from_std(std::net::TcpListener)`, `Listener::udp(SocketAddr)`; no Unix-domain
listener, and `Conn::peer_addr` is a `SocketAddr`.

**Suggested API.** `Listener::unix(path)` (and `Listener::unix_from_std(UnixListener)`), serving
the same acceptors (akio-http's `serve`), its connections' peer address a placeholder or an enum;
the socket file's mode set before it is listened on (`Listener::mode(0o600)`), and a stale file
removed by the caller or by an option. On epoll, kqueue and io_uring.

**How akit uses it.** `akit_admin::Server::start` listens on the path when `admin.address` is
`unix/<path>`, as fit's `admin.Start`; the Host check is skipped there, as fit skips it, and the
file provider's control socket (I29) may use it too.

## A24 An HTTP/1 response that closes its connection

**Problem.** A service that refuses a request before reading its body (413 for a body past a
limit it found mid-stream, or one whose `Content-Length` is already too large on a streamed
route) cannot close the connection after its answer: akio-http decides keep-alive from the
request alone, keeps reading the rest of the body (delivering it to a sink no one reads, or
discarding it) and keeps the connection for the next request. A `Connection: close` header the
service sets is sent but not honoured. fit's Admin API closes such a connection (Go's
`MaxBytesReader`), so a client cannot make the server read an unbounded body it already refused;
under akio-http the only bound is the body timeout, which a fast client never trips.

**Evidence.** `crates/akio-http/src/session.rs` at `a9a620ab`: `keep_alive` comes from the
request's `Connection` and version; in `State::Stream` the decoder keeps delivering chunks after
the response; `crates/akio-http/src/message.rs` `Response::encode` writes a service's
`Connection` field as it is and `Responder::send` closes only when `!keep_alive`.

**Suggested API.** Honour `Connection: close` on a response (close after it is sent, and stop
reading the request body), or `Context::close_after_response()` / `Detached::close_after(..)`;
for a streamed body, a way for the sink to stop the delivery (`BodyStream`-side
`ctx.stop_body()`), after which the connection closes once the response is out.

**How akit uses it.** The Admin API's 413 for a load past `max_config_bytes` (I26) closes the
connection, as fit's, and so does its 401 for a request without the token (I27), so that a client
cannot keep trying tokens on one connection; I41's size limits on the data plane do the same.

## A29 A response hook without an allocation

**Problem.** The access log (I21) and the metrics (I37) learn of each response on the thread that
sends it, whichever thread that is (a proxied request's is answered later, from the upstream's
completion). The only way is `Context::add_response_filter`, which boxes its closure: a closure
that carries anything (the start time, the metrics slot, the log's shard) costs an allocation per
request, which rule 13 asks the hot path to avoid. fit's dispatcher counts with a hook too, but
Go's closure costs no more than the `OnFinish` it is given. akit already counts a request
answered before its handler returns (fit's inline handlers, `respond`, the 404) without a hook,
from `Context::status` after the call; a detached response (the proxy, static files read on the
pool) has no other way.

**Evidence.** `crates/akio-http/src/context.rs` at `a9a620ab`: `add_response_filter` does
`self.fin.filters.push(Box::new(f))`; `Finish::filters` keeps two inline, but each is a
`Box<dyn FnOnce>`. akio-http's own `HttpMetrics::middleware` (`observe.rs`) allocates the same way,
and clones an `Arc` per request.

**Suggested API.** A per-service observer, set once when the listener is built, and a few words a
request carries inline in its `Finish`:

```rust
pub trait ResponseObserver: Send + Sync + 'static {
    /// The response is being sent (on the thread that sends it), with the words the request set.
    fn on_response(&self, resp: &Response, req: &Request, words: [u64; 2], started: Instant);
}
akio_http::serve_with(cfg, svc).observer(Arc<dyn ResponseObserver>);
ctx.set_observed(words: [u64; 2]); // no allocation; started is taken then
```

A detached context keeps the words and calls the same observer when its response is sent.

**How akit uses it.** `akit_listener::Dispatcher` sets the route's metrics slot and the log's
shard index as the words, and its observer writes the access log line and counts the request:
no allocation and no reference count per request. The boxed filter (`Dispatcher::track`) goes;
I64's traces take the same hook.

## A30 The open connections a listener accepted

**Problem.** fit's `fit_connections_open` counts the client connections open on the listeners.
akio's `Telemetry::open_connections` counts every connection open, the ones the engine dialed (the
upstream pools' connections, which akit dials on the listeners' engine) and UDP peers included,
so akit's gauge is too high by its upstream connections; and the per-loop counters
(`accepted`, `dialed`, `closed`) do not say which kind a closed connection was.

**Evidence.** `crates/akio-core/src/telemetry.rs` at `a9a620ab`: `conn_opened(limited)` adds to one
`open` counter for every kind (`akio-epoll/src/reactor.rs` and `akio-uring/src/reactor.rs` call it
for dialed connections with `limited` false); `conn_closed` takes from it whatever the kind.

**Suggested API.** `Telemetry::open_accepted()` (or an `accepted_open` gauge per loop in
`LoopCounters`): accepted TCP connections open now, apart from dialed ones and UDP peers.

**How akit uses it.** `akit_listener::metrics::Scrape` writes it as `fit_connections_open`, in
place of `open_connections`.

## A32 Where a regular expression matches, its groups, and replacing them

**Problem.** fit's `rewrite` middleware replaces each match of a regular expression in the path
(`regexp` and `replacement`, with `$1` and `${name}` for its groups), and its `redirect` middleware
builds the URL from the groups of the one match (`"to": "/new/$1"`). akio-regex answers only
whether a pattern occurs in a text: it neither finds where nor captures a group. akit refuses those
fields as not supported yet (I40).

**Evidence.** `crates/akio-regex/src/lib.rs` at `a9a620ab`: `Regex` has `is_match`,
`is_match_fmt` and `nfa_is_match`; the crate's documentation says groups are parsed but "nothing
is captured". The Pike VM (`crates/akio-regex/src/exec.rs`) keeps no positions per thread.

**Suggested API.** On `Regex`, with the leftmost-first semantics of Go's `regexp` and the `regex`
crate, in linear time as now (a Pike VM with capture slots, or the DFA to find the match's end and
start, then the slots only for the match):

- `find_at(&self, text: &str, start: usize) -> Option<Range<usize>>` and `find_iter`;
- `captures(&self, text: &str) -> Option<Captures>` (each group's range, by index and by name);
  `group_names`;
- `expand(&self, caps: &Captures, template: &str, text: &str, out: &mut String)`, with Go's
  `Regexp.Expand` template rules (`$1`, `${1}`, `$name`, `${name}`, `$$`; an unknown or unmatched
  group is empty; a malformed `$` is itself), and `replace_all(&self, text, template) -> Cow<str>`
  as Go's `ReplaceAllString`, empty matches included.

**How akit uses it.** `rewrite`'s `regexp` and `replacement` (`replace_all` on the decoded path)
and `redirect`'s groups in `to` (`captures` on the escaped path, then `expand`), compiled when the
config is; the not-supported-yet refusals in akit-config go.

**Status.** Built by I48 in akio (ADR-115, `26c500b5`): `find`, `find_at`, `find_iter`, `captures`,
`expand` and `replace_all` as suggested, with Go's leftmost-first matches and groups (repetitions
compiled in Go's shapes), checked against Go's `regexp` on 132 000 random cases.

## A33 A request changed by a middleware: its fields and its target

**Problem.** A middleware changes the request before the handler sees it: fit's `headers`
middleware sets, adds and deletes request fields, its `rewrite` middleware changes the path, and
later ones (ForwardAuth's copied fields, JWT's claims as fields) do too. akio-http's `Context`
gives its request by shared reference only (`req()`), so neither its fields nor its target can be
changed.

**Evidence.** `crates/akio-http/src/context.rs` at `a9a620ab`: `Context` holds the request in a
private `ReqCell` (owned until a `Detached` or an upgrade shares it) and offers `req(&self) ->
&Request`, `path`, `header` and `local`; there is no `&mut` access. `Request`'s `headers` and
`target` are public fields.

**Suggested API.** `Context::req_mut(&mut self) -> Option<&mut Request>`, `None` once the request
is shared (after `detach` or an upgrade), or narrower: `Context::request_headers_mut` and
`Context::set_target(&mut self, target: &str)`. What akio reads of the request afterwards (`Files`'s
conditional and range fields, `path()`, the router's params) then sees the change.

**How akit uses it.** The `headers` middleware changes the request's fields in place, and the
`rewrite` middleware its target, before calling the handler; the stopgaps go: the request fields
kept in a context local (`akit_runtime::REQUEST_FIELDS`, read by `request_headers`) and the
rewritten path kept in `CANONICAL_PATH` (which stays for I11's canonical path only, or goes too).
Until then the changed fields reach the `proxy` handler's upstream only (recorded in the I40
progress entry).

## A35 A streamed HTTP/1.1 response framed by its length

**Problem.** A proxy that streams an upstream's response of a known length should send it with
that length, as fit does: the client then knows how much is coming (a download's progress, a
range's end) and can tell a body cut short from a whole one by its framing. akio-http's server
streams every HTTP/1.1 response (`stream_response`) chunked on a keep-alive connection; a
`Content-Length` field given with it would be sent beside `Transfer-Encoding: chunked`, which is
not valid, so akit drops it. HTTP/2 and HTTP/3 keep the field, their DATA frames being framing
enough.

**Evidence.** `crates/akio-http/src/context.rs` at `60547435`: `start_stream` sets
`Transfer-Encoding: chunked` whenever the HTTP/1 response is keep-alive 1.1, whatever its
headers; `BodyStream::write` chunk-frames each piece. `message.rs` `Response::encode` adds no
`Content-Length` to a streamed response but keeps one it is given.

**Suggested API.** `stream_response` honours a `Content-Length` in the headers it is given on
HTTP/1: no chunked framing, the pieces written as they are, `finish` checking that the length was
written in full (closing the connection when it was not, as a body cut short must), a write past
the length refused. Or `Context::stream_response_sized(status, headers, len: u64)` with the same
meaning, if a field read from the headers is too implicit.

**How akit uses it.** I16's proxy streams a response of a known length past 64 KiB to an HTTP/1.1
client with its `Content-Length` (it now goes chunked, the field dropped: recorded in the I16
progress entry). Also the cache (I67), which serves a stored response as it reads it.

## A45 A public timer

**Problem.** Active health checks (I35) probe each address of an upstream every interval, and a
TCP probe must fail once its timeout passes: something has to run a callback after a delay. akio
has timers, but none is public at the pinned commit: the I/O loops' wheels (`akio-timer`) are
internal, and akio-http's timer thread (`crate::timer::after`, which arms its clients' request
timeouts) is `pub(crate)`. A gateway has more uses: ACME renewals and retries (I31), retries with
backoff (I54), any periodic job on the control plane, each of which would otherwise start a
thread of its own.

**Evidence.** At `60547435`: `crates/akio-http/src/timer.rs` (`pub(crate) fn after`);
`crates/akio-timer` ("akio internals ... (use `akio`)"), re-exported nowhere. akio's
`claude/akio-all` branch has an `akio_core::timer::after(delay, f)` on the calling I/O thread's
wheel, not on main.

**Suggested API.** `akio::timer::after(delay, f: impl FnOnce() + Send + 'static) -> TimerHandle`,
`f` run on a timer thread (or an engine's I/O thread) once `delay` has passed, never blocking it,
and `TimerHandle::cancel`. An engine-bound form, `Engine::after`, would run `f` on one of its I/O
threads and drop it when the engine stops. Bounded, as everything in akio is: a cap on pending
timers with an error past it.

**How akit uses it.** `akit_upstream::timer` is the stopgap: one thread of akit's own with a heap
of pending jobs, bounded at 65,536 (each probed address has at most two pending: its next probe
and its probe's timeout). It is replaced by this API, and the probes' timeouts cancelled rather
than left to run.

## A48 A request's end heard: its response sent in full, or its client gone

**Problem.** fit's `concurrency_limit` counts a request as under way until its response is sent
or its client has gone, with fib's `OnFinish` and `OnCancel`. akio-http tells a service neither:
the one hook on a request is a response filter (`Context::add_response_filter`), which runs when
the response's head is sent. A streamed response (`stream_response`, which akit's proxy uses for
every body past 64 KiB) runs it before its body, so the request leaves the limit while its body
still goes out; and a request whose client goes while it waits detached (a slow upstream) hears
nothing until its handler lets the `Detached` go, so its place is held until the upstream answers
or times out. The filter's closure is also boxed, an allocation a request (as A29 records).

**Evidence.** `crates/akio-http/src/context.rs` at `60547435`: `Finish::apply` runs the filters
from `Out::send` and `start_stream`, at the head; `BodyStream::finish` and its drop run nothing of
the request's; `Detached` and `BodyStream` give no way to learn that the connection or stream
closed before they are used (`BodyStream::write` returns an error only when written to).

**Suggested API.** `Context::on_end(f: impl FnOnce(End) + Send)`, run once when the request is
done with: `End::Sent` once the response is written in full (a streamed one's last byte handed to
the connection), `End::Gone` when the connection or stream closes first, whether the request was
answering, detached or not. A form that takes a few words rather than a boxed closure (as A29's
observer) keeps it off the allocator. `Detached::on_gone(f)` (or a pollable `is_gone()`) lets a
handler that waits give up its upstream work when the client goes.

**How akit uses it.** `concurrency_limit` gives a request's place back on `on_end`, as fit's
`OnFinish`/`OnCancel`; today it gives it back when the response's head is sent, or when the
request is dropped unanswered (the stopgap recorded in the I41 progress entry). The proxy (I17)
cancels its upstream request on `Gone`; the access log (I21) and metrics (I37) can count a
streamed response's duration to its end.

## A49 A request body limited by a middleware

**Problem.** fit's `body_limit` (fib's `Context.LimitBody`) bounds a request's body for whatever
handler reads it: a streamed body that grows past the limit fails in the handler's `OnBody` with
`ErrBodyTooLarge` and is answered 413, and an HTTP/1 connection closes rather than read the rest.
In akio-http only the listener's `HttpConfig::max_body_bytes` bounds a buffered body; a streamed
one (`BodyMode::Stream`) is delivered to the handler's sink without a bound, and a middleware
cannot see or wrap that sink (`Context::on_body` replaces it; the sink is private).

**Evidence.** `crates/akio-http/src/context.rs` at `60547435`: `on_body` stores one sink,
`deliver` is crate-private; `crates/akio-http/src/session.rs`: the `max_body_bytes` check applies
to buffered bodies and to declared lengths of buffered ones only.

**Suggested API.** `Context::limit_body(&mut self, max: u64)`, callable before or after the
handler sets its sink, lowering the bound for this request: past it akio-http ends the delivery
with an error event to the sink (`BodyEvent::TooLarge`, or `Err` beside `Chunk` and `End`),
answers 413 unless a response has begun, and on HTTP/1 closes the connection once the answer is
out rather than read on (with A24); HTTP/2 and HTTP/3 reset the stream.

**How akit uses it.** `body_limit` calls it for every request and drops its stopgap: a streamed
body of no declared length is held to the limit only by a handler that reads akit-runtime's
`MAX_BODY_BYTES` local (the proxy), which the middleware lowers; a buffered one is checked once
it has arrived (the stopgap recorded in the I41 progress entry). The listener's own
`max_body_bytes` for streamed bodies moves to it too.

## A36 An HTTP client request's response-header and idle timeouts

**Problem.** fit bounds an upstream exchange three ways (its `response_header_timeout`,
`response_idle_timeout` and `response_timeout`): how long the upstream may take to begin its
response (504 past it), how long it may go silent in the middle of its body (the response cut short
past it), and the exchange in all. akio's HTTP/1 client has the last alone
(`ClientRequest::timeout`, which runs until `on_end` for `request_with`): a proxy cannot answer
504 for an upstream that is slow to begin while letting a long download run, nor cut off an
upstream that stalls mid-body before the whole exchange's hour is up. akit has no timer of its own
on the I/O threads to do it with (akio's timers are internal), and a timer thread of akit's would
need to cancel a request before its head, which akio cannot do either (A37).

**Evidence.** `crates/akio-http/src/client.rs` at `60547435`: `ClientRequest` has `timeout`
alone; `ClientError::Timeout` is the one timeout error. ADR-101 notes that the streamed response
has "no idle-gap timeout".

**Suggested API.** `ClientRequest::header_timeout(d)`: from the request's head written (its body,
when streamed, may still be going) to the response's head; past it the request fails with
`ClientError::Timeout` (or a `HeaderTimeout` kind) and its connection is closed.
`ClientRequest::idle_timeout(d)`: the longest gap between reads of the response's body; past it
`on_error` with the same error, the connection closed. Both for `request`, `request_with` and
`request_streamed`, and `H2Client`'s alike (I18). The time a held body (`ResponseBody::hold_reads`)
spends held is not idle: the client chose to stop reading.

**How akit uses it.** I17b: the upstream's `response_header_timeout` and
`response_idle_timeout` (fit's 60s defaults) are set on each proxied request; a header timeout is
answered 504, an idle one cuts the response short as a broken one is (I17's `Fit-Error` trailer on
HTTP/2 and HTTP/3 for a body of no declared length).

## A37 An HTTP client request cancelled at any point

**Problem.** A proxy gives up an upstream request when its client goes (fit's `OnCancel`). akio's
client can give up a response once its head came (`ResponseBody::cancel`), and a streamed request
by dropping its unfinished `RequestBody`; but a request whose body went whole (most requests) has
no handle before its head: `Client::request` and `request_with` return nothing. A client that
goes while the upstream thinks leaves the upstream working, and an akit connection and `max_conns`
slot taken, until the upstream answers or `response_timeout` passes (1h by default).

**Evidence.** `crates/akio-http/src/client.rs` at `60547435`: `request`, `request_with` and
`request_streamed` return `()`; `ResponseBody::cancel` comes with `on_head`.

**Suggested API.** `request_with` (and `request`) return a `RequestHandle` (`Clone + Send`) with
`cancel()`: before the request is sent it is dropped; once sent, its connection is closed (on
HTTP/2, its stream reset); the handler gets `on_error(ClientError::Cancelled)` once, unless it had
`on_end` or `on_error` already. A cancel from another thread is posted to the connection's I/O
thread, as `ResponseBody::cancel` is.

**How akit uses it.** I17b: the proxy keeps the handle beside its answer and cancels on the client
going (A38); I45's retries cancel an attempt they give up on.

## A38 A detached server request that hears its client went; a write stall timeout changed at run time

**Problem.** Two server-side gaps, both about clients that leave or stop:

1. A detached request (`Context::detach`) hears nothing of its client going until it writes: a
   proxy waiting for its upstream's head cannot cancel the upstream request (A37) for a client
   that left. Worse, on HTTP/1 a request whose body streams (`BodyMode::Stream`) keeps its body
   sink while the connection closes: the HTTP/1 session holds the request's context (and so the
   sink) in its state, and the session lives on while the detached answer holds the connection,
   so the sink is neither told nor dropped. HTTP/2 drops a closed connection's streams, and with
   them their sinks, which is how akit's proxy (I17) cancels an h2 or h3 client's upload; an
   HTTP/1 client that goes mid-upload goes unnoticed until the exchange ends.
2. fit's `write_stall_timeout` (30s): how long a response may wait for its client to take what
   was sent. akio's `Config::write_timeout` is that bound (output queued, none written), but per
   engine, fixed when it is built (`Live` can change `idle_timeout` and others, not it): akit
   cannot apply the config's value on reload.

**Evidence.** `crates/akio-http/src/session.rs` at `60547435`: `Session::on_close` leaves
`State::Stream { ctx, .. }` in place; `h2.rs` `on_close` drains the streams. `context.rs`:
`Detached` has `respond` and `stream_response` alone. `akio-core` `telemetry::Live` has no
`set_write_timeout`. Found with akit's `failure.rs` test of a client that goes mid-upload (I17).

**Suggested API.** (1) `Detached::on_gone(f)` (and `Context::on_gone` before detaching): `f`
runs once, on the connection's I/O thread, when the request's connection closes or its stream is
reset before its response is finished; at once if it already happened. The HTTP/1 session drops
(or ends with a `BodyEvent::Aborted`) a streamed request's sink when its connection closes, as
HTTP/2 does. (2) `Live::set_write_timeout(Option<Duration>)`, as for `idle_timeout`.

**How akit uses it.** I17b: the proxy registers `on_gone` to cancel its upstream request (A37) and
not count it against the upstream's health, closing fit's `TestClientAbort` "before the header"
and HTTP/1 "mid-upload" cases; `limits.write_stall_timeout` sets the engine's write timeout at
start and on reload. I49's WebSocket and tunnel relays hear a client go the same way.

## A54 An idle HTTP/1 connection drained after a grace, not at once

**Problem.** When a listener is drained (`Engine::shutdown_listener`, `Engine::shutdown`), akio-http
closes an idle HTTP/1 keep-alive connection at once. A client may have sent its next request on
that connection already, or be sending it: the request meets the close unanswered, and the client
sees a reset or an end of file. HTTP/1 has no way to tell an idle client that its connection is
going (HTTP/2 has `GOAWAY`), so the close itself races every request in flight on the wire. akit
drains a listener a reload removes or changes (I22, I28), so a client using a removed or replaced
listener at that moment sees a failed request unless it sends the request again, as HTTP clients
may with idempotent requests (RFC 9112 section 9.3.1).

**Evidence.** `crates/akio-http/src/session.rs` at `afbbb521`: `on_shutdown` runs
`Session::shutdown`, whose HTTP/1 path is `h1_shutdown`; with no response in the order,
`reserve_response` hands a slot at once and `fill_close` closes the connection. akit's
`crates/akit/tests/reload.rs` (I22) counts the requests its clients send again after such a close
(at most one per client per replacement), since without the retry they fail.

**Suggested API.** A grace for idle HTTP/1 connections on a drain: an idle connection stays open
for up to the grace, a request that arrives in it is answered with `Connection: close` and the
connection closed after the response, and the connection is closed at the end of the grace if none
does. One that receives part of a request in the grace is treated as busy. The grace is capped by
the drain's deadline: `ServerConfig::idle_drain_grace(Duration)` (a default of a few hundred
milliseconds, 0 for today's behaviour), or a parameter of `shutdown_listener`. A request already
in the socket's receive buffer when the drain starts is read and answered either way.

**How akit uses it.** `akit_listener::Listeners::reload` drains removed and changed listeners
with it, from `-shutdown-timeout`; the retries in `akit/tests/reload.rs` go, and its clients count
every error as a failure. It narrows the race to a client that sends its next request only after
the grace, which only a client idle for that long does.

## A51 `H2Client`'s streamed requests: reset, timeout, trailers, `GOAWAY`

**Problem.** A proxy to an h2 upstream streams each exchange (`H2Client::open`), as it streams
an HTTP/1 one (`Client::request_with`, A19, A20). Four things the HTTP/1 path has, the h2 one
lacks:

1. **No reset.** `open` returns an `H2Upload` with `write` and `finish` alone: a proxy whose client
   went cannot reset the upstream's stream (`RST_STREAM CANCEL`). `H2Conn::cancel` exists but only
   a whole request's timeout reaches it.
2. **No timeout.** `ClientRequest::timeout` is armed by `request` alone; `open` ignores it.
3. **No request trailers.** `H2Upload` cannot end the body with a trailing HEADERS frame, so a
   request's trailers (gRPC's among them, I50) cannot reach an h2 upstream.
4. **Forgotten on `GOAWAY`.** A `GOAWAY`'s doomed streams (above its last stream id) are removed
   with their callback failed, but a streamed request's `on_event` is not told: it waits forever.
   And after a `GOAWAY` the client still says `is_open`, while every new stream fails `Closed`.

**Evidence.** `crates/akio-http/src/h2client.rs` at `afbbb521`: `open` returns `H2Upload { write,
finish }`; `request` alone arms `crate::timer::after` for its timeout; `H2Conn::cancel` is private;
the `GOAWAY` arm of `handle` calls `s.cb.take()` and never `s.events`; `is_open` is
`conn.is_open()`. Found by I18's proxy over h2c (`crates/akit-upstream/src/h2.rs`).

**Suggested API.** `H2Upload::cancel()` (also on a handle `open` returns beside it): the stream is
reset `CANCEL` and `on_event` gets `H2Event::Error(ClientError::Cancelled)` once, unless it ended.
`open` arms `req.timeout` as `request` does: past it, reset and `Error(Timeout)`.
`H2Upload::finish_with_trailers(Headers)`. On `GOAWAY`, doomed streamed requests get
`Error(Closed)` (a `Refused` kind would tell a caller it may retry them elsewhere), and
`H2Client::is_open` (or a new `accepts_streams`) is false once a `GOAWAY` came.

**How akit uses it.** I18b: `akit_upstream::h2` drops its stopgaps: a stream given up (client gone,
timeout, a slow client) is reset rather than left running with its connection retired and closed
once its other streams are over; the per-address timer tick that enforces `response_timeout` goes;
request trailers go up; a `GOAWAY` fails its doomed streams (502, or a retry, I45) and retires the
connection. I50's gRPC proxying needs all four.

**Done** (akio ADR-110, in akio's main at `f6750e93`, pinned by I18b): `H2Upload::stream()`
gives an `H2Stream` whose `cancel` resets the stream (`H2Upload::cancel` too); `open` arms
`ClientRequest::timeout`; `H2Upload::finish_with_trailers`; a `GOAWAY`'s doomed streams, and
requests after it, fail with the new `ClientError::Refused`, and `is_open` is false once one came.
I18b uses all four.

## A52 `H2Client` backpressure per stream

**Problem.** Rule 8 links backpressure: a slow client holds the upstream's reads, and a slow
upstream holds the client's. Over HTTP/1, akio's `ResponseBody::hold_reads` and
`RequestBody::queued_bytes` / `on_drain` do it (A19, A20). akio's h2 client has neither:

1. It gives each stream's receive window back as soon as DATA arrives (`window_update` on every
   frame), so the server sends a streamed response as fast as the connection allows whatever the
   consumer does. A proxy with a slow client can only buffer it or give the stream up.
2. `H2Upload::write` queues without bound and reports nothing: the bytes wait in the stream's
   `upload` queue until the window opens, and the producer cannot tell how many.

**Evidence.** `crates/akio-http/src/h2client.rs` at `afbbb521`: the `DATA` arm of `handle` calls
`window_update(&mut out, sid, total)` for every frame not ending the stream; `H2Upload::write`
pushes onto `UpShared::q` and `pump` moves it into `CStream::upload` (a `VecDeque<u8>`) with no
count kept outside.

**Suggested API.** On the stream `open` makes: `hold_reads(bool)`: while held, the stream's window
is not given back (DATA already in flight still arrives, at most one window); released, what was
consumed is given back at once. Optionally `consume(n)` for a caller that gives the window back as
it hands bytes on. On `H2Upload`: `queued_bytes()` (accepted, not yet in a DATA frame) and
`on_drain(f)` (once it falls to 0, or at once), with `write` failing past a `max_queued_bytes`, as
`RequestBody`'s do.

**How akit uses it.** I18b: the proxy's relay holds an h2 download while its client is behind and
paces an upload by its queue, exactly as over HTTP/1; akit's stopgap, which cuts a held download
off past `HELD_LIMIT` (4 MiB) and lets an upload's queue grow up to the body limit, goes.

**Done** (akio ADR-110, `f6750e93`): `H2Stream::hold_reads` keeps the stream's window closed
while held; `H2Upload::try_write`, `queued_bytes` and `on_drain`, bounded by
`H2Options::max_queued_bytes`. Not done: `consume(n)`. I18b holds downloads and paces uploads with
them, as over HTTP/1.1.

## A53 `H2Client`'s connection: the server's stream limit, the connect timeout, ALPN's fallback

**Problem.** A pool of h2 connections opens a stream on a connection with one free and dials
another when none has: it must know how many the server allows. And it bounds the connect as
the upstream's `connect_timeout` says, and, for fit's `h2`, falls back to HTTP/1.1 when the server
does not choose h2 in ALPN.

1. akio's client ignores `SETTINGS_MAX_CONCURRENT_STREAMS` (setting 3) and does not report it: a
   request past it is refused by the server (`REFUSED_STREAM`). akit assumes 100, the least RFC 9113
   lets a server advertise.
2. `H2Client::connect` races addresses with `HappyEyeballs::default()`, whose `connect_timeout` the
   caller cannot set; `dial` uses the engine's (as A12 for the HTTP/1 client).
3. Over TLS, `dial` speaks h2 whatever ALPN chose; a server that picks `http/1.1` fails.

**Evidence.** `crates/akio-http/src/h2client.rs` at `afbbb521`: the `SETTINGS` arm of `handle`
matches 1, 4, 5 and 8, not 3; `connect` builds `akio::dns::HappyEyeballs::default()`; `dial` passes
the TLS acceptor without looking at the negotiated protocol.

**Suggested API.** `H2Client::max_concurrent_streams()` (the server's latest, `u32::MAX` until its
SETTINGS) and `open_streams()`; a request past the limit waits in the client (bounded) or fails
`Busy` at once, as the caller chooses. `H2Client::connect_with(engine, host, port, tls, opts,
HappyEyeballs, ready)`. A dial over TLS that reports the protocol ALPN chose (`ready` given an
`enum { H2(H2Client), Http1(Conn) }`), or fails with a clear error when it is not `h2`.

**How akit uses it.** I18b: `akit_upstream::h2` sizes each connection by the server's limit, dials
within the upstream's `connect_timeout`, and with `h2` falls back to akio's HTTP/1 client for a
server that does not speak h2, as fit's `h2` does (I54 adds the `tls_*` fields).

**Done** (akio ADR-110, `f6750e93`): `H2Client::max_concurrent_streams`, `open_streams` and
`waiting_streams`, requests past the limit waiting in the client (`H2Options::max_pending_streams`,
else `ClientError::Busy`); `H2Options::happy_eyeballs` for `connect`; over TLS, a dial that
fails with `NotH2 { alpn }` when ALPN did not choose `h2`. Not done: `connect_with` and the
`enum { H2, Http1 }` (rejected in ADR-110: the race belongs in `H2Options`, and a fallback dials
akio's HTTP/1 client). I18b sizes connections by the server's limit, races addresses within
`connect_timeout` (a lone address keeps the engine's, A12), and speaks HTTP/1.1 over TLS to an `h2` address on `NotH2`.

## A57 Root certificates usable outside a handshake

**Problem.** fit staples the OCSP response of a certificate from files only when its chain
verifies against the system's roots (`x509.SystemCertPool`): a certificate written from what
someone else gave (a tenant's Kubernetes Secret) must not have the gateway ask a responder it
names, unless a public CA issued it. akit-tls verifies the chain with akio-x509's `Verifier`, which
takes an `akio_x509::RootStore`; akio-tls loads the system's roots (`RootCerts::system()`, ADR-010
and ADR-059: the keychains on macOS, the distribution's bundle elsewhere) but keeps the store
private to `RootCerts`, so they can only be used by a handshake.

**Evidence.** `crates/akio-tls/src/config.rs` at `afbbb521`: `pub struct RootCerts(akio_x509::RootStore)`
with no accessor; `RootCerts::system()` builds the store from `crate::system::load()`, which is
private. Found by I32 (`akit-tls/src/ocsp.rs`).

**Suggested API.** `RootCerts::store(&self) -> &akio_x509::RootStore` (or `impl
AsRef<RootStore>`, or `From<RootCerts> for RootStore`); or `akio_x509::RootStore::system()`
sharing akio-tls's loader and cache.

**How akit uses it.** `Ocsp` verifies a certificate from files against `RootCerts::system()`'s
store, on its own worker thread, as fit's `OCSP` does by default; the roots an operator gives
(`Ocsp::trust_roots`) stay a replacement. Until then a certificate from files is stapled only
under roots given, so `akit run` staples ACME's certificates and none from files (the stopgap
recorded in the I32 progress entry).

## A58 The ALPN protocol a TLS connection negotiated, seen by an HTTP service

**Problem.** A listener that obtains certificates through TLS-ALPN-01 offers `acme-tls/1` last
in ALPN (RFC 8737), so that only a CA checking a challenge, which offers it alone, negotiates it.
Such a connection is for the handshake only: fit closes it once the handshake is done (fib's
server sees `NegotiatedProtocol` and serves no HTTP on it). akio-http's server tells HTTP/2 by its
preface, not by ALPN, and gives a service nothing of the TLS connection, so a client that
negotiates `acme-tls/1` and then sends an HTTP/1 request is served it. Nothing is exposed (the
handshake already gave it the challenge's self-signed certificate), but it is not what fit does,
and on-demand TLS (I34) has the same case.

**Evidence.** `crates/akio-http/src/session.rs` and `context.rs` at `afbbb521`: no access to the
connection's `TlsInfo` (`alpn`) from a `Service`; `akio-tls`'s `Connection::alpn()` is there.

**Suggested API.** Either `Context::tls()` (the negotiated ALPN protocol and SNI name of the
request's connection, `None` in the clear), or an `HttpConfig` setting that closes a TLS
connection whose negotiated protocol is not one the server speaks (`http/1.1`, `h2`, none) once
its handshake is done.

**How akit uses it.** The dispatcher closes a connection whose ALPN is `acme-tls/1` without
answering (or akio closes it), as fit's listener does; today it is served as HTTP/1.1.

## A63 bcrypt password hashes

**Problem.** fit's `basic_auth` checks passwords against bcrypt hashes (`$2a$`, `$2b$`, `$2y$`,
as `htpasswd -B` and its own `hash-password` make them). `akio-crypto` has PBKDF2 and HKDF, not
bcrypt, and a gateway should not carry Blowfish of its own.

**Evidence.** `crates/akio-crypto/src/lib.rs` and `kdf.rs` at `afbbb521`: no bcrypt or Blowfish.

**Suggested API.** `akio_crypto::bcrypt`: `Hash::parse(&str) -> Result<Hash, ParseError>` (the
reason in the error), `Hash::verify(&self, password) -> bool` compared in constant time, `cost`,
`Display`; `hash(password, cost)` and `hash_with_salt`. As Go: a password read to its first 72
bytes, the minor versions alike.

**How akit uses it.** akit-config checks `password_hash` with `Hash::parse` and says why one is
not a hash; `basic_auth` checks passwords with `verify` on akio's logical pool (A65).

**Done.** As suggested, in `5e0d433c` (ADR-109): vectors from Go, `htpasswd -B` and OpenBSD;
Blowfish's tables generated from pi's digits and checked against Go's.

## A64 The client's verified certificate on each server request

**Problem.** fit's `client_cert` lets a request through only when its client sent a certificate
that the listener verified (`tls.client_auth`), and passes its subject on. An akio-http handler
sees a `Context`, not the connection; `akio_tls::TlsInfo` did not carry the peer's chain, and
`akio_quic::Connection` did not give it either.

**Evidence.** `crates/akio-tls/src/lib.rs` at `afbbb521`: `TlsInfo` has ALPN, SNI, version, early
data and OCSP, not the chain, though the protocol core keeps it (`peer_certificates`, restored on
resumption); `crates/akio-http/src/message.rs`: `Request` has `peer` only.

**Suggested API.** `Request::peer_certificates: Option<Arc<[Vec<u8>]>>`, the chain the client sent
and the handshake verified, set on HTTP/1.1, HTTP/2 and HTTP/3 requests, shared by a connection's
requests; `TlsInfo::peer_certificates` and `akio_quic::Connection::peer_certificates` beside it.

**How akit uses it.** `client_cert` refuses a request without one (403) unless `optional`, and
writes the leaf's subject, as Go's `pkix.Name.String` writes it, into `subject_header`.

**Done.** As suggested, in `5e0d433c` (ADR-109), with `akio_tls::peer_chain` (`PeerChain`), which
the servers ask once a connection's handshake is done; a request that came as TLS 1.3 early data
has none.

## A65 A request suspended by a middleware and resumed with its context

**Problem.** A bcrypt check takes milliseconds to tens of milliseconds of CPU, so `basic_auth`
cannot check a password on the I/O thread (rule 1). fit runs the check on a pool while the
request waits, then goes on to the next middleware and the handler. In akio a middleware can
detach the response and answer from the pool (`Context::detach_to_pool`), but not go on with the
chain: the next handler takes a `Context`, and a detached request has none. `run_on_pool` moves
every request of a listener to the pool. ForwardAuth (I43) has the same need: the request waits
for an authorization server, then goes on.

**Evidence.** `crates/akio-http/src/context.rs` and `pool.rs` at `afbbb521`: `detach` gives a
`Detached` (respond or stream a response); `call_on_pool` moves a whole context to the pool, but is
crate-private and taken for every request of a listener with `run_on_pool`.

**Suggested API.** `Context::suspend(&mut self) -> Option<Suspended>` and
`Suspended::resume(self, impl FnOnce(&mut Context))`, from any thread, finishing the request as a
server finishes one after `call`; `Context::suspend_to_pool(work)`, with `503` when the pool is
full.

**How akit uses it.** `basic_auth` suspends a request whose password is not remembered, checks it
on the pool, and resumes it into the next middleware and the handler, or answers 401. I43's
ForwardAuth resumes the request from the auth request's completion.

**Done.** As suggested, in `5e0d433c` (ADR-109). A request whose body streams to its handler
cannot be suspended (akio feeds the body to the context after `call`, as `run_on_pool` draws the
line): `basic_auth` asks for the body whole (within the listener's `max_body_bytes`) only for a
request with a body whose password it has not checked yet. A later version may let a suspended
request hold its body's reads instead.

## A75 A request whose body streams, suspended

**Problem.** `forward_auth` (I43) and `basic_auth` (I42) suspend a request while they wait
(`Context::suspend`, A65), then resume it into the next middleware and the handler. akio cannot
suspend a request whose body streams to its handler (`BodyMode::Stream`): it feeds the body to
the context after `call`, so the context cannot move. Both middlewares therefore ask for such a
body whole, within the listener's `max_body_bytes` (16 MiB by default, or what a config sets):
behind them, the proxy's uploads lose their streaming and the memory they take is that of the
whole body, not one read's (rules 5 and 8). fit's `forward_auth` keeps the body streaming: the
request waits retained, its body unread.

**Evidence.** `crates/akio-http/src/context.rs` at `4f7169c4`: `Context::suspend` returns `None`
when `self.streamed`; `Service::body_mode` is asked before `call`, so a middleware that cannot know
yet whether it will suspend must ask for every body whole.

**Suggested API.** `Context::suspend` on a request whose body streams: the body's reads are held
(as `BodyReads::hold(true)`) from the suspension until the resumed context's handler registers
its sink (`on_body`) and lets them go, and what arrived before stays buffered (one read's worth)
for it. A client that goes meanwhile runs the context's `on_gone` as for any suspended request.

**How akit would use it.** `forward_auth`, and `basic_auth` for a password not remembered yet,
stop asking for bodies whole: a request whose body streams is suspended with its reads held, and
the proxy behind them streams the upload once the check let it through.

## A66 `H2Client`'s requests heeding the header and idle timeouts and a `CancelToken`

**Problem.** ADR-108 (A36, A37) gave akio's HTTP/1 client a request's `response_header_timeout`,
`response_idle_timeout` and `cancel` (`CancelToken`), and left `H2Client` out: it ignores the
three fields, as it ignores `streamed`. A proxy to an h2 upstream bounds its exchanges by fit's
same timeouts and gives a request up when its client goes, wherever the request is; over h2 it
has nothing to hand them to. A51 asks for a streamed request's reset and its whole timeout; these
are the rest of what the HTTP/1 client now does.

**Evidence.** akio `4f7169c4`: `ClientRequest::response_header_timeout`, `response_idle_timeout`
and `cancel` are documented "HTTP/1 client only"; `crates/akio-http/src/h2client.rs` reads none of
them (ADR-108's "Rejected: `H2Client` in this change"). Found by I17b's proxy over h2c
(`crates/akit-upstream/src/h2.rs`).

**Suggested API.** No new API: `H2Client::open` and `request` heed the request's three fields as
`Client` does. The header timeout runs from the request's END_STREAM (its body ended) to the
final response's HEADERS; the idle timeout is the longest gap between DATA frames, stopped while
the stream's reads are held (A52); a cancelled token resets the stream (`CANCEL`). Each fails
the stream with `ClientError::HeaderTimeout`, `IdleTimeout` or `Cancelled` (`H2Event::Error`),
once, and leaves the connection to its other streams.

**How akit uses it.** I18b: `akit_upstream::h2`'s per-address tick stops checking each stream for
its head, its silence and its token (I17b's stopgap, within a `TICK` of 100ms), and a stream given
up for one of them is reset rather than left running with its connection retired.

**Done** (akio ADR-111, `d27e6a20`, made by I18b's session): as suggested, no new API.
`H2Client::request` and `open` heed the three fields: the header timeout from the request's
END_STREAM to the final HEADERS, the idle timeout between DATA frames (held reads not counted),
and a `CancelToken` that now covers h2 streams too; each resets the stream alone and fails it
once with `HeaderTimeout`, `IdleTimeout` or `Cancelled`. I18b drops the per-address tick's checks.

## A78 An idle timeout per connection

**Problem.** fit's layer 4 services (I51) close a connection, or a UDP session, that carries
nothing either way for the service's `idle_timeout`: 10 minutes for TCP and 1 for UDP by default,
each service its own. akio has idle timeouts for the whole engine only: `Config::idle_timeout`
(TCP, counting inbound data alone) and `Config::udp_idle_timeout` (UDP peers, listeners' and
`dial_udp`'s alike), the latter also bounding the HTTP/3 listeners' peers. fib, under fit, sets
one per connection (`SetUDPIdleTimeout(-1)` turns the engine's off for a session the service times
itself).

**Evidence.** At `4f7169c4`: `crates/akio-core/src/config.rs` (`idle_timeout`,
`udp_idle_timeout`, applied to every connection of the engine); `Live::set_udp_idle_timeout`
changes the engine's value, not a connection's. `Conn` has no setter.

**Suggested API.** `Conn::set_idle_timeout(Option<Duration>)`, for TCP connections and UDP peers
(accepted or dialed), from any thread, overriding the engine's for that connection; `None` turns
it off for it. An option to count outbound data as activity too
(`Conn::set_idle_counts_writes(bool)`), since a relay is idle only when neither side sends: or
akit touches both connections from the relay, which the per-connection timer then reads.

**How akit uses it.** `akit_l4` sets each connection's and session's idle timeout from its
service's and lets akio close it on its own I/O thread. Until then, the stopgap: each service
sweeps its open connections on akit's timer thread (`akit_upstream::timer`, A45's stopgap), every
quarter of its shorter idle timeout (between 10 ms and 1 s), and akit raises the engine's
`udp_idle_timeout` to the longest of the services' (`Live::set_udp_idle_timeout`, on each reload)
so that akio does not close a session first.

## A79 `Engine::dial` with a connect timeout of its own

**Problem.** A layer 4 service (I51) dials its upstream's addresses within the upstream's
`connect_timeout`, as fit's do. `Engine::dial` and `Engine::dial_udp` take the engine's
`Config::connect_timeout`, one for every dial; `akio::dns::HappyEyeballs` has a timeout per
attempt but dials a lone address with the engine's too. It is A12's gap for dials that are not
HTTP's.

**Evidence.** At `4f7169c4`: `crates/akio/src/lib.rs` (`Engine::dial`, `dial_udp`, `dial_from`
take no timeout); `crates/akio/src/dns.rs` (`HappyEyeballs::dial` calls `engine.dial` for a single
address).

**Suggested API.** `Engine::dial_with(addr, DialOptions { connect_timeout, .. }, acceptor,
on_error)`, and `HappyEyeballs::dial` passing its `connect_timeout` to a lone address too.

**How akit uses it.** `akit_l4` dials each address within its upstream's `connect_timeout`. Until
then the engine's connect timeout is the shortest of the upstreams' (`akit_listener::engine`, as
for A12), and a host name's addresses are raced with `HappyEyeballs` at the upstream's timeout.

## A90 A streamed response's coding set by a filter of the caller's own

**Problem.** fit's `compress` middleware (I47) compresses a streamed response (a proxied one) as it
is written, with its own negotiation (q values among the encodings it offers, in their order),
its own eligibility (every status with a body but 206, `no-transform`, `Content-Range`, a
declared length below `min_bytes`), weak tags, and a bound on the responses compressed at once
(`max_in_flight`), each given back when its response ends. akio's `compress` middleware has its
own choices, which differ, and only it can give a streamed body a coding.

**Evidence.** At `4f7169c4`: `crates/akio-http/src/message.rs` (`Response::stream_coding` and
`streaming` are `pub(crate)`), `crates/akio-http/src/coding.rs` (`StreamCoding` is
`pub(crate)`), `crates/akio-http/src/middleware.rs` (`compress`'s filter sets them). Nothing tells
a filter when a streamed response's body ends.

**Suggested API.** A public `Coding` (gzip, deflate, br, zstd with levels and windows),
`Response::is_streaming`, `Response::set_stream_coding(Option<Coding>)`, and
`Response::keep_until_sent(value)`: a value kept with the response until it is sent (for a
streamed one, until its `BodyStream` is finished or dropped), whose drop gives a place back.

**How akit uses it.** `akit-middleware`'s `compress` filter picks the encoding as fit's does, sets
the coding on a streamed head and keeps its place's guard there; a whole body it compresses itself
with akio's compressors. **Done** on akio's branch `claude/a90-a91-compress` (`b5cd3f25`,
ADR-112), not merged yet.

## A91 `Files` keeping compressed copies of files held in memory

**Problem.** fit's static handler's `compress` keeps a file it holds in memory compressed too, with
each encoding a client asks for, made once on its file pool at a level that makes it small and
served from then on, so that no request compresses it again.

**Evidence.** At `4f7169c4`: `crates/akio-http/src/files.rs` serves compressed twins from disk
(`twins`, ADR-100) and keeps small files in memory (`max_cached_bytes`), but makes no copies.

**Suggested API.** `Files::compress(&["br", "zstd", "gzip"])`: a copy per coding of a file held in
memory, large enough and of a compressible type, made on the disk pool the first time a client
prefers that coding, kept with the file's entry; its own tag, no ranges; twins first.

**How akit uses it.** The `static` handler passes its `compress` to it. **Done** on akio's branch
`claude/a90-a91-compress` (`b5cd3f25`, ADR-112), not merged yet. Not done: counting the copies in a
bound on the cache's memory (A15).

## A92 A body sent from a file on disk encoded by a stream coding

**Problem.** fit's `compress` middleware compresses whatever the handler writes, a large static
file read from disk included. akio sends a file body (`Response::file`, for files above
`max_cached_bytes`) with `sendfile(2)` or by reading it on the transport's side; a filter cannot
give it a coding, and the file cannot be read on the I/O thread to encode it (rule 1).

**Evidence.** At `4f7169c4`: `crates/akio-http/src/message.rs` (`FileBody`), `context.rs` and
`h2.rs` (file bodies sent as they are); `Response::set_stream_coding` (A90) applies to streamed
bodies only.

**Suggested API.** A file body with a stream coding set is read in pieces on the disk pool and
encoded there, its pieces written as a streamed body's, under the connection's backpressure (the
next piece read once the queue drains), its `Content-Length` dropped.

**How akit uses it.** The `compress` middleware sets the coding on a file body as on a streamed one.
Until then a body from disk goes uncompressed, which the progress files list as a difference from
fit.

## A93 The HTTP/1 client hands a connection over on `101`

**Problem.** A WebSocket proxy (I49) sends the client's handshake to the upstream as an HTTP/1.1
upgrade and, once the upstream answers `101 Switching Protocols`, joins its connection to the
client's, as fit's `pool.SendUpgrade` and `resp.Tunnel()` do. akio-http's `Client` could not hand
a connection over: it read a `101` as a response without a body, ended the exchange and put the
connection back in its keep-alive pool, where the bytes that followed were taken for garbage and
closed it.

**Evidence.** At `f6750e93`: `crates/akio-http/src/client.rs`, `ClientConn::advance` (`no_body`
for `101`, `complete` returning the connection to the pool; `RState::Idle` closing on bytes nobody
asked for). Nothing in `ResponseHandler` or `ResponseBody` gives the connection out.

**Suggested API.** `ClientRequest::upgrade(protocol)` and `ResponseHandler::on_upgrade(&ResponseHead,
&RawConn) -> Option<Box<dyn RawHandler>>`: on a `101` to a request that asked to upgrade, the
request is over and the connection, with what the server sent after the head, goes to the returned
handler as a byte stream (`RawConn`: `send`, `hold_reads`, `on_drain`, `queued_bytes`).

**How akit uses it.** `akit_upstream::Exchange::on_upgrade` passes it on, gives the address's slot
back and counts the connection among the address's until it closes; `akit_proxy`'s `websocket`
joins it to the client's side with backpressure both ways. Done in akio by this iteration
(ADR-113, branch `claude/a93-a94-tunnels`, `30cdf93a`), to be merged onto akio's main.

## A94 A streamed `2xx` to an HTTP/2 extended CONNECT is a tunnel

**Problem.** An HTTP/2 or HTTP/3 client opens a WebSocket with an extended CONNECT (RFC 8441, RFC
9220); the proxy answers it only once the upstream switched, with the upstream's fields, so it
cannot use `Context::upgrade_websocket`, which answers at once with its own choices and ends the
WebSocket in the server. akio's HTTP/2 server already dispatches an extended CONNECT as a streamed
request (its DATA to `on_body`, held with `BodyReads`) that a streamed response answers, but only
`upgrade_websocket` marked the stream a tunnel: a streamed `200` finished while the client's half
was open was answered `RST_STREAM NO_ERROR`, cutting off the client's last bytes (its close frame's
echo).

**Evidence.** At `f6750e93`: `crates/akio-http/src/h2.rs`, `Core::finish_if_done` (only
`st.tunnel` keeps the peer's half open; `start_tunnel`, used by `upgrade_websocket`, alone sets
it). HTTP/3 (`akio-h3`) already keeps a stream's directions apart.

**Suggested API.** No new API: a `2xx` streamed (`stream_response`) to an extended CONNECT makes
the stream a tunnel, as `upgrade_websocket`'s `200` does, with the same linger before a peer that
never ends its half is reset.

**How akit uses it.** An HTTP/2 or HTTP/3 WebSocket's client side is its streamed request and a
streamed `200`. Done in akio by this iteration (ADR-113, with A93).

## A108 The HTTP/1 client's TLS server name

**Problem.** fit's `tls_server_name` (I54) is the name an `h2` upstream's certificates are checked
against, and sent in SNI, in place of each address's host: an upstream reached at `10.0.0.5:443`
whose certificate names `app.internal`. `H2Client::connect` takes the TLS name apart from the host,
but an address whose server chooses HTTP/1.1 in ALPN, and its probes, go through akio-http's
`Client`, which always used the URL's host; the URL cannot carry the name instead, since the
connection goes to its host.

**Evidence.** At `c5cf4353`: `crates/akio-http/src/client.rs`, `Client::dial` (`Tls::client(cfg,
&key.host, acceptor)`); nothing in `Client`, `ClientOptions` or `ClientRequest` sets the name.

**Suggested API.** `Client::with_tls_server_name(name)`: every `https://` connection sends `name` in
SNI and checks the certificate against it; connections still pooled by host and port; an empty
name restores the URL's host.

**How akit uses it.** An `h2` pool's HTTP/1 client (the fallback) and its probes' client are given
the upstream's `tls_server_name`. Done in akio by this iteration (ADR-117, branch
`claude/a108-a109-upstream-tls`, `252fba42`), to be merged onto akio's main.

## A109 A client certificate's key checked when the configuration is built

**Problem.** fit loads `tls_cert_file` and `tls_key_file` with Go's `tls.LoadX509KeyPair`, which
refuses a key that is not the certificate's, and reports it at the field when the config is
compiled. akio-tls's `ClientConfigBuilder::build` parsed the key without checking it against the
leaf, as `ServerConfigBuilder::build` does, so a mismatched pair built and then failed every
handshake that asked for the certificate.

**Evidence.** At `c5cf4353`: `crates/akio-tls/src/config.rs`, `ClientConfigBuilder::build`
(`CertKey::new(chain, key.parse())`; the server side calls `certified_key`, which compares the
public keys).

**Suggested API.** No new API: `ClientConfigBuilder::build` checks the client chain as the server
side does, and fails with `private key does not match the certificate`.

**How akit uses it.** `akit_tls::UpstreamTls::load` reports the mismatch at
`upstreams[i].tls_cert_file` when the config is compiled, as fit does. Done in akio by this
iteration (ADR-117, with A108).

## A111 A streamed body seen by a response filter as it is sent

**Problem.** The response cache (I67), as fit's, keeps a response as it goes to the client and
stores it once all of it went, never holding it back. A response filter sees the head of a
streamed response (`Context::stream_response`, `Detached::stream_response`), the proxy's for a
body past 64 KiB, but not its body: the writes go from `BodyStream::write` to the transport, and
nothing learns whether the body ended whole, cut short or with trailers.

**Evidence.** At `c5cf4353`: `crates/akio-http/src/context.rs`, `start_stream` and
`BodyStream::write` / `finish` / `finish_with_trailers`.

**Suggested API.** `Response::tap_body(impl BodyTap)`, set by a filter on a streamed head;
`BodyTap::data(&[u8])` for each write after the stream coding, `BodyTap::end(TapEnd)` once, last
(`Finished`, `Trailers`, `Cut`).

**How akit uses it.** The cache's filter taps a storable streamed response, copies what passes
(bounded per response and per zone) and stores it on `Finished`. Done in akio by this iteration
(ADR-116, with A112 and A113).

## A112 HTTP-dates in all three formats

**Problem.** A cache reads `Date`, `Expires`, `Last-Modified` and `If-Modified-Since`, which RFC
9110 section 5.6.7 has recipients accept in three formats, as Go's `http.ParseTime` does for fit.
akio parsed only the IMF-fixdate, privately in `files.rs`.

**Suggested API.** `akio_http::parse_http_date(&str) -> Option<u64>`.

**How akit uses it.** For every date the cache reads. Done in akio by this iteration (ADR-116).

## A113 A response filter that runs after the inner ones

**Problem.** Filters run in registration order, so a middleware's filter, registered before it
calls what it wraps, runs before those of the middlewares inside it: a cache in front of
`compress` would store a response before it is compressed. fit runs its header hooks innermost
first; akit's middlewares rely on akio's order, which cannot change for everyone.

**Suggested API.** `Context::add_outer_response_filter(f)`: outer filters run after every
ordinary one, the last added first.

**How akit uses it.** The cache's filter is an outer one, so each encoding `compress` picks is a
variant of its own. Done in akio by this iteration (ADR-116).

## A105 A request's peer is the client a trusted PROXY header names

**Problem.** An HTTP listener behind a load balancer takes the client's address from a PROXY
header, as fit's `proxy_protocol` does: the client it names is the request's, in
`X-Forwarded-For`, the access log, `remote_ip` matchers, rate limits and `ip_hash`, all of which
akit reads from `Request::peer`. akio-mux's `ProxyProtocol` layer (A4) reads the header and
attaches it to the connection, but akio-http fills `Request::peer` with the socket's peer, and a
request handler never sees the connection, so akit cannot reach the header.

**Evidence.** At `c5cf4353`: `crates/akio-http/src/session.rs` builds `Request { peer:
c.peer_addr(), .. }` and gives HTTP/2 sessions `c.peer_addr()`; ADR-095 leaves "using
`proxy::client_addr` there" to a later change.

**Suggested API.** No new API: `Request::peer` (and `Context::peer`) is `akio_mux::proxy::
client_addr(conn)`, the header's source when the layer attached one naming an address, else the
socket's peer; `Conn::peer_addr` unchanged.

**How akit uses it.** `akit-listener` puts `ProxyProtocol` in front of a listener with
`proxy_protocol` (HTTP over TCP, before TLS), trusting its `trusted` addresses; every feature that
reads the client's address then reads the header's. Done in akio by I52 (ADR-118, branch
`claude/a105-proxy-peer`, `e3f03a32`), to be merged onto akio's main.

## A117 A bounded pool of an application's own, from the `akio` facade

**Problem.** Rule 4 has blocking work run on akio's logical pool or on a bounded pool of akit's
own. The cache's disk tier (I68) wants a pool of its own, apart from the logical pool, so that a
disk that hangs holds up neither the handlers on the logical pool nor anything else, as fit's
cache has its own file pool and akio's `Files` has its own `disk_pool`. akio has the pool
(`akio_pool::Pool`, behind `Conn::spawn_blocking`), and the `akio` facade re-exports
`PoolConfig` and `PoolStats` but not `Pool`, so an application builds a `PoolConfig` it cannot
use.

**Evidence.** At `e9891b52`: `crates/akio/src/lib.rs` has `use akio_core::{.., Pool, ..}`
(private) beside `pub use akio_core::{.., PoolConfig, PoolStats, ..}`; `akio-pool`'s manifest
calls it "akio internals ... (use `akio`)".

**Suggested API.** `pub use akio_core::Pool` from `akio` (with `Rejected` and `Job`, which
`Pool::try_execute` takes and gives back), documented as the pool for an application's own
blocking work.

**How akit uses it.** `akit-cache`'s disk pool (`disk.rs`): `Pool::new` with at least 16 threads
and 4 a CPU and a queue of 4,096, every file operation submitted with `try_execute`. Meanwhile
akit-cache depends on `akio-pool` directly, at the same commit as every akio crate (a stopgap, in
the progress log); it moves to `akio::Pool` once this is on akio's main.
