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
| A1 | TCP listeners added, removed and drained on a running engine | in progress, akio branch | 10, 11 | I22, I28 |
| A2 | A server certificate chosen per handshake by a callback that may complete later | in progress, akio branch | 1, 4 | I30, I34 |
| A3 | A filter on accepted connections before any handler runs | in progress, akio branch | 11 | access lists |
| A4 | PROXY protocol v1/v2: parse on accept, write on dial | in progress, akio branch | 12 | I52 |
| A5 | An akio-mux rule that routes TLS by SNI without terminating it | in progress, akio branch | 12 | I52, I53 |
| A6 | CSR generation in `akio-x509` (and JWS helpers in `akio-crypto`) | done on akio branch `claude/x509-csr` (`7ea64069`), not merged | 15 | I31 |
| A7 | An MP4 (ISO BMFF) reader for segmenting files without transcoding | in progress, akio branch | — | media library |
| A8 | TUN devices | in progress, akio branch | — | VPN |
| A9 | A TLS server capped at TLS 1.2 (public `ServerConfigBuilder` API) | requested (I09) | 17 | I09 |
| A10 | Closing an HTTP client's idle connections now | requested (I14) | 10, 11 | I14, I22 |
| A11 | An HTTP/1 client request that keeps its own `Host` field (and sends no `User-Agent` when given an empty one) | done on akio branch `claude/i15-proxy-699f18` (`552fe283`), not merged | — | I15 |
| A12 | The HTTP client's connect timeout for a lone address too | requested (I14) | 10 | I14 |
| A13 | `Files` serving a path the caller names | done on akio branch `claude/i13-static2-b61107` (`3209255c`), not merged | 19 | I12, I13 |
| A14 | `Files` answering as a gateway's static handler does | requested (I12) | 19 | I12, I13 |
| A15 | `Files` bounded in memory, its disk pool with a timeout | requested (I12) | 4, 11 | I12 |
| A16 | `Files` choosing a compressed twin by the client's q values, among the encodings configured | done on akio branch `claude/i13-static2-b61107` (`3209255c`), not merged | 19 | I13 |
| A17 | `Files` trying candidate paths in turn (`try_files`) | done on akio branch `claude/i13-static2-b61107` (`3209255c`), not merged | 19 | I13 |
| A18 | `Files` listing a directory that has no index file | done on akio branch `claude/i13-static2-b61107` (`3209255c`), not merged | 4, 11, 19 | I13 |

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

**Done** on akio branch `claude/i13-static2-b61107` (`3209255c`, ADR-100, not merged): A13 as
`Files::serve(ctx, path)`; A16 as `Files::twins(&[&str])` and a twin picked by q value; A17 as
`Files::try_paths(&[&str])` with `Files::hide(f)` for fit's dotfiles; A18 as
`Files::list_directories(max_entries)`, fit's page byte for byte. akit's `static` handler uses
them once akit's akio pin moves to an akio main with that branch merged.

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

**Done** on akio branch `claude/i13-static2-b61107` (`3209255c`, ADR-100, not merged): A13 as
`Files::serve(ctx, path)`; A16 as `Files::twins(&[&str])` and a twin picked by q value; A17 as
`Files::try_paths(&[&str])` with `Files::hide(f)` for fit's dotfiles; A18 as
`Files::list_directories(max_entries)`, fit's page byte for byte. akit's `static` handler uses
them once akit's akio pin moves to an akio main with that branch merged.
