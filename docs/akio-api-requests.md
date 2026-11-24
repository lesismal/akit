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
| A4 | PROXY protocol v1/v2: parse on accept, write on dial | in akio's main (`a9a620ab`), pinned | 12 | I52 |
| A5 | An akio-mux rule that routes TLS by SNI without terminating it | in akio's main (`a9a620ab`), pinned | 12 | I52, I53 |
| A6 | CSR generation in `akio-x509` (and JWS helpers in `akio-crypto`) | done on akio branch `claude/x509-csr` (`7ea64069`), not merged | 15 | I31 |
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
| A24 | An HTTP/1 response that closes its connection, and stops reading a body it refused | requested (I26) | 7, 10, 11 | I26, I41 |
| A29 | A response hook without an allocation: a listener's observer, called with a few words the request set | requested (I37) | 13 | I21, I37, I64 |
| A30 | The open connections a listener accepted, apart from dialed ones and UDP peers | requested (I37) | — | I37 |
| A32 | Where an `akio-regex` pattern matches, its groups, and replacing them | requested (I40) | 19 | I40, I48 |
| A33 | A request's fields and target changed by a middleware (`Context::req_mut`) | requested (I40) | 12, 19 | I40, I43, I44 |
| A35 | A streamed HTTP/1.1 response framed by its length | requested (I16) | 5 | I16, I67 |
| A45 | A public timer: a callback run once a delay has passed, from any thread | requested (I35) | 1, 11 | I35, I31, I54 |
| A48 | A request's end heard: its response sent in full, or its client gone while it waits | requested (I41) | 3, 11, 13 | I41, I17, I21, I37 |
| A49 | A request body limited by a middleware, whoever reads the body | requested (I41) | 7, 11 | I41 |
| A36 | An HTTP client request's response-header and idle timeouts | requested (I17) | 10 | I17b, I18 |
| A37 | An HTTP client request cancelled at any point | requested (I17) | 3 | I17b, I45 |
| A38 | A detached server request that hears its client went; a write stall timeout changed at run time | requested (I17) | 3, 10 | I17b, I49 |

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
connection, as fit's; I41's size limits on the data plane do the same.

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
