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
| A3 | A filter on accepted connections before any handler runs | in akio's main (`a9a620ab`), pinned, used by I85 | 11 | I85–I87 |
| A4 | PROXY protocol v1/v2: parse on accept, write on dial | in akio's main (`a9a620ab`), pinned, used by I52 | 12 | I52 |
| A5 | An akio-mux rule that routes TLS by SNI without terminating it | in akio's main (`a9a620ab`), pinned, its ClientHello parser used by I52 | 12 | I52, I53 |
| A6 | CSR generation in `akio-x509` (and JWS helpers in `akio-crypto`) | in akio's main (`afbbb521`), pinned | 15 | I31 |
| A7 | An MP4 (ISO BMFF) reader for segmenting files without transcoding | in akio's main (`a9a620ab`), pinned | — | media library |
| A8 | TUN devices | in akio's main (`a9a620ab`), pinned | — | VPN |
| A9 | A TLS server capped at TLS 1.2 (public `ServerConfigBuilder` API) | in akio's main (`4989cd8b`), pinned | 17 | I09 |
| A10 | Closing an HTTP client's idle connections now | on akio's main and pinned (since `0fc8f3a7`) | 10, 11 | I14, I22, I38 |
| A11 | An HTTP/1 client request that keeps its own `Host` field (and sends no `User-Agent` when given an empty one) | in akio's main (`a9a620ab`), pinned | — | I15 |
| A12 | The HTTP client's connect timeout for a lone address too | on akio's main and pinned (since `6bcd8ad6`, akio's ADR-154) | 10 | I14, I35 |
| A13 | `Files` serving a path the caller names | in akio's main (`a9a620ab`), pinned | 19 | I12, I13 |
| A14 | `Files` answering as a gateway's static handler does | in akio's main (`4989cd8b`), pinned | 19 | I12, I13 |
| A15 | `Files` bounded in memory, its disk pool with a timeout | in akio's main (`4989cd8b`), pinned | 4, 11 | I12 |
| A16 | `Files` choosing a compressed twin by the client's q values, among the encodings configured | in akio's main (`a9a620ab`), pinned | 19 | I13 |
| A17 | `Files` trying candidate paths in turn (`try_files`) | in akio's main (`a9a620ab`), pinned | 19 | I13 |
| A18 | `Files` listing a directory that has no index file | in akio's main (`a9a620ab`), pinned | 4, 11, 19 | I13 |
| A19 | An HTTP client request whose body is sent as it is produced, with backpressure | in akio's main (`60547435`), pinned, used by I16 | 5, 8 | I16, I17, I18, I49, I75 |
| A20 | An HTTP client response delivered as it arrives, its reads held and resumed | in akio's main (`60547435`), pinned, used by I16 | 5, 8 | I16, I17, I18, I67 |
| A21 | A server request that holds its client's reads and hears when its client drained | in akio's main (`60547435`), pinned, used by I16 | 5, 8 | I16, I47, I75 |
| A22 | UDP listeners removed and drained on a running engine | on akio's main and pinned (since `6bcd8ad6`, akio's ADR-155) | 10, 11 | I22, I28 |
| A23 | Unix-domain stream listeners (with a socket file mode) | in akio's main (`4989cd8b`), pinned | 11 | I26, I29 |
| A24 | An HTTP/1 response that closes its connection, and stops reading a body it refused | on akio's main and pinned (since `6bcd8ad6`, akio's ADR-157) | 7, 10, 11 | I26, I27, I41 |
| A29 | A response hook without an allocation: a listener's observer, called with a few words the request set | on akio's main and pinned (since `6bcd8ad6`, akio's ADR-158) | 13 | I21, I37, I64 |
| A30 | The open connections a listener accepted, apart from dialed ones and UDP peers | on akio's main and pinned (since `6bcd8ad6`) | — | I37 |
| A32 | Where an `akio-regex` pattern matches, its groups, and replacing them | in akio's main (`e9891b52`), pinned | 19 | I40, I48 |
| A33 | A request's fields and target changed by a middleware (`Context::req_mut`) | in akio's main (`dad77a7a`), pinned | 12, 19 | I40, I43, I44, I71 |
| A35 | A streamed HTTP/1.1 response framed by its length | on akio's main and pinned (since `6bcd8ad6`, akio's ADR-157) | 5 | I16, I67 |
| A36 | An HTTP client request's response-header and idle timeouts | in akio's main (`4f7169c4`), pinned, used by I17b | 10 | I17b, I18 |
| A37 | An HTTP client request cancelled at any point | in akio's main (`4f7169c4`), pinned, used by I17b | 3 | I17b, I45 |
| A38 | A detached server request that hears its client went; a write stall timeout changed at run time | in akio's main (`4f7169c4`), pinned, used by I17b | 3, 10 | I17b, I49 |
| A45 | A public timer: a callback run once a delay has passed, from any thread | on akio's main and pinned (since `6bcd8ad6`, akio's ADR-154) | 1, 11 | I35, I31, I52, I54 |
| A48 | A request's end heard: its response sent in full, or its client gone while it waits | in akio's main (`dad77a7a`), pinned | 3, 11, 13 | I41, I17, I21, I37 |
| A49 | A request body limited by a middleware, whoever reads the body | on akio's main and pinned (since `6bcd8ad6`, akio's ADR-157) | 7, 11 | I41 |
| A51 | `H2Client`'s streamed requests: reset, timeout, request trailers, and failed rather than forgotten on a `GOAWAY` | in akio's main (`f6750e93`, ADR-110), pinned, used by I18b 3, 10 | I18b, I50 |
| A52 | `H2Client` backpressure per stream: a streamed response's reads held, an upload's queue reported | in akio's main (`f6750e93`, ADR-110), pinned, used by I18b 8 | I18b, I50 |
| A53 | `H2Client`'s connection: the server's stream limit heeded and reported, the connect timeout, ALPN's HTTP/1.1 fallback | in akio's main (`f6750e93`, ADR-110), pinned, used by I18b 10, 11 | I18b, I54 |
| A54 | An idle HTTP/1 connection drained after a grace, not at once | on akio's main and pinned (since `6bcd8ad6`) | 10 | I28 |
| A57 | Root certificates usable outside a handshake: the system's, to verify a chain against | in akio's main (`4989cd8b`), pinned | 1, 19 | I32 |
| A58 | The ALPN protocol a TLS connection negotiated, seen by an HTTP service (or such a connection closed) | in akio's main (`4989cd8b`), pinned | 19 | I32, I34 |
| A63 | bcrypt password hashes in `akio-crypto` | in akio's main (`4f7169c4`), pinned | 1, 4 | I42 |
| A64 | The client's verified certificate on each server request | in akio's main (`4f7169c4`), pinned | — | I42 |
| A65 | A request suspended by a middleware and resumed with its context | in akio's main (`4f7169c4`), pinned | 1, 3, 4 | I42, I43 |
| A66 | `H2Client`'s requests heeding `response_header_timeout`, `response_idle_timeout` and a `CancelToken` | in akio's main (`d27e6a20`, ADR-111), pinned, used by I18b | 3, 10 | I17b, I18b |
| A75 | A request whose body streams suspended, its body's reads held until it is resumed | on akio's main and pinned (since `6bcd8ad6`, akio's ADR-159) | 5, 7, 8 | I42, I43, I44 |
| A78 | An idle timeout per connection, TCP or a UDP peer, set by its handler | on akio's main and pinned (since `6bcd8ad6`, akio's ADR-155) | 10, 11 | I51, I55 |
| A79 | `Engine::dial` and `dial_udp` with a connect timeout of their own | on akio's main and pinned (since `6bcd8ad6`, akio's ADR-154) | 10 | I51, I54, I55 |
| A90 | A streamed response's coding set by a response filter of the caller's own, and a value kept until the response is sent | in akio's main (`c5cf4353`), pinned | 8, 11 | I47 |
| A91 | `Files` keeping compressed copies of the files it holds in memory | in akio's main (`c5cf4353`), pinned | 4, 12 | I47 |
| A92 | A body sent from a file on disk encoded by a stream coding | in akio's main (`4989cd8b`), pinned | 1, 5 | I47 |
| A93 | The HTTP/1 client hands a connection over on `101 Switching Protocols` | in akio's main (`c5cf4353`), pinned | 8, 9 | I49, I55 |
| A94 | A streamed `2xx` to an HTTP/2 extended CONNECT is a tunnel whose halves close separately | in akio's main (`c5cf4353`), pinned | 9 | I49 |
| A105 | A request's peer is the client a trusted PROXY header names (akio-http) | in akio's main (`e9891b52`), pinned | 19 | I52 |
| A108 | The HTTP/1 client's TLS server name, in place of the URL's host | in akio's main (`e9891b52`), pinned | 19 | I54 |
| A109 | A client certificate's key checked against its leaf when the TLS configuration is built | in akio's main (`e9891b52`), pinned | 19 | I54 |
| A111 | A response filter sees a streamed body as it is sent, and how it ended (a body tap) | in akio's main (`e9891b52`), pinned | 5, 6 | I67 |
| A112 | An HTTP-date parsed in all three formats RFC 9110 has recipients accept | in akio's main (`e9891b52`, ADR-116), pinned | 15 | I67 |
| A113 | A response filter that runs after the inner ones (an outer filter) | in akio's main (`e9891b52`, ADR-116), pinned | 5 | I67 |
| A117 | `akio::Pool` and `PoolConfig` together: a bounded pool of an application's own for its blocking work | on akio's main and pinned (since `6bcd8ad6`) | 4 | I68 |
| A132 | A proxy tunnel's upstream checked once connected, by the name asked for and both addresses; the SOCKS client a trusted PROXY header names | in akio's main (`8453b60c`), pinned | 1, 19 | I55 |
| A133 | A proxy's connect timeout of its own, answered (SOCKS5 4, HTTP 504) | in akio's main (`8453b60c`), pinned | 10 | I55 |
| A134 | CONNECT tunnels over HTTP/2 and HTTP/3 (RFC 9113 §8.5, RFC 9114 §4.4) | in akio's main (`4989cd8b`), pinned | 9, 19 | I55 |
| A135 | Dialing a Unix-domain stream socket, and the HTTP client over one (akio, akio-http, akio-sys) | in akio's main (`0f2c419b`), pinned | 1, 2 | I58 |
| A148 | The address a request came to (the listener's local address), on HTTP/1.1, h2 and h3 | on akio's main and pinned (since `6bcd8ad6`, akio's ADR-157) | — | I75 |
| A165 | A WebAssembly runtime for plugins: a sandboxed interpreter, its host API, WASI preview 1, a pool of instances; the response hooks Proxy-Wasm needs | in akio's main (`dad77a7a`), pinned | 1, 4, 11 | I71, I72 |
| A166 | A response's head held while a call made elsewhere decides on it (fit's A41 from fib) | in akio's main (`dad77a7a`), pinned | 1, 4 | I72 |
| A167 | A response body filtered by calls made elsewhere, one at a time, its trailers in the last (fit's A46 from fib) | in akio's main (`dad77a7a`), pinned | 1, 4, 5, 8 | I72 |
| A168 | An HTTP server that serves HTTP/2 with prior knowledge alone (a listener with `h2c` and no `http/1.1`) | on akio's main and pinned (since `6bcd8ad6`, akio's ADR-174) | 19 | I05, I25, I79b |
| A169 | Requests RFC 9112, RFC 9113 and RFC 9114 call malformed refused by akio's servers: two `Host` fields, bad bytes in HTTP/2 and HTTP/3 field names and values, a streamed body's bad framing answered 400 | on akio's main and pinned (since `0fc8f3a7`) | 15, 19 | I25 |
| A171 | An HTTP client request's timer jobs cancelled when it ends, not held until its timeout would pass | on akio's main and pinned (since `0fc8f3a7`) | 11, 13 | I15, I18, I25 |
| A172 | A TLS server that takes a ClientHello whose server name is an IP address (as no name) rather than failing the handshake | on akio's main and pinned (since `0fc8f3a7`) | 19 | I09, I25 |
| A170 | A connection's TCP counters (and unread bytes) without reaching into `Conn::inner`, and a splice that writes first through the engine's own API | on akio's main and pinned (since `6bcd8ad6`, akio's ADR-156) | 1, 14 | I51b |
| A180 | akio-wasm's call made cheaper, measured against wazero (akio's W5) | on akio's main and pinned (since `077a6710`) | 13 | I71, I72, I76 |
| A200 | A request target in absolute form taken as RFC 9112 asks: its path as the request's path, its authority as its host | on akio's main and pinned (since `077a6710`, akio's ADR-177) | 15, 19 | I77 |
| A214 | A budget of the bytes queued for every connection together, which pauses every connection's reads past it (fit's `limits.max_pending_bytes`) | on akio's main and pinned (since `6bcd8ad6`, akio's ADR-172) | 8, 11 | I79, I79b |
| A215 | A connection limit that counts the connections accepted, not those the engine dials (fit's `limits.max_conns`) | on akio's main and pinned (since `6bcd8ad6`, akio's ADR-173) | 11 | I79, I79b |
| A220 | An accept filter on UDP and datagram listeners: a refused peer's datagrams dropped before a connection (a `udp` session, a QUIC connection) is made for it | on akio's main and pinned (since `9e21ada7`, akio's ADR-178) | 1, 11 | I85, I86 |
| A225 | An external security review of akio-crypto, akio-tls (with akio-x509), akio-http's HTTP/1 and HTTP/2 parsers and akio-wasm (no API) | requested (I80b): the threat model's L15 | 15 | I80, I80b |
| A230 | Argon2id (RFC 9106), with BLAKE2b, in akio-crypto: the key derivation of fit's `fitstore` format | on akio's main and pinned (since `9e21ada7`, akio's ADR-179), used by I93 | 15, 20 | I93 |
| A231 | XChaCha20-Poly1305 (HChaCha20 and a 24-byte nonce) in akio-crypto: the AEAD of fit's `fitstore` format | on akio's main and pinned (since `9e21ada7`, akio's ADR-179), used by I93 | 15, 20 | I93 |
| A232 | An HTTP connection's idle timeout and close from its handler, and the connections accepted told to the application: fit's bounds on connections without a login | on akio's main and pinned (since `9e21ada7`, akio's ADR-180) | 10, 11 | I90 |
| A233 | A terminal's echo turned off (raw mode), to read a password | on akio's main and pinned (since `9e21ada7`) | 14 | I89 |
| A234 | A connection's own bound of the bytes queued for it, past the engine's `max_queued_bytes` or below it | on akio's main and pinned (since `6151f5c5`, akio's ADR-181), used by the tunnel's links | 8, 11 | I101, I104 |
| A235 | A client TLS configuration that checks the server's certificate by its SHA-256 (a pin) in the handshake, in place of the chain and name | on akio's main and pinned (since `6151f5c5`, akio's ADR-182), used by the tunnel's TLS and QUIC links | 1, 15 | I102, I104 |
| A236 | A WebSocket's reads held and resumed, its drain heard, and its peer's address, on `WsSender` (both akio-ws's client and akio-http's upgrade) | on akio's main and pinned (since `6151f5c5`, akio's ADR-183), used by I102c | 8, 9 | I102c, I104 |
| A237 | `Engine::wait` (and `Pool::is_current`) safe in a thread-local destructor | on akio's main and pinned (since `6151f5c5`) | 1, 15 | — |
| A238 | A WebSocket ended at once, what is queued for it dropped, without the close handshake (`WsSender::abort`) | on akio's main and pinned (since `8a9601b9`, akio's ADR-184) | 8, 10 | I102c, I103b, I104 |
| A245 | The QUIC server's handshake bounds: a handshake timeout, and the handshakes under way bounded with a Retry past half (`ServerConfig::handshake_timeout`, `max_handshakes`) | on akio's main and pinned (since `8a9601b9`, akio's ADR-185) | 10, 11 | I102d |
| A247 | The process's effective user (`akio_sys::process::euid`) | on akio's main and pinned (since `a8171fda`), used by the LLM node | 14 | I108 |
| A248 | A signal to a process group (`akio_sys::process::kill_group`) | on akio's main and pinned (since `a8171fda`), used by the LLM node's jobs | 1, 14 | I108 |
| A249 | A file system's size and space free (`akio_sys::fs::space`) | on akio's main and pinned (since `a8171fda`), used by the LLM node's disks | 1, 14 | I108 |
| A256 | akio-turn's static settings fit's TURN role sets: the relayed sockets bound in a port range, the nonces' key from a secret servers share, and a stream client's largest message | on akio's main and pinned (since `d8d51f61`, akio's ADR-186): `TurnConfig::relay_ports`, `nonce_secret`, `max_stream_message` | 4, 11 | I111, I112 |
| A257 | akio-turn's hooks for an embedding server: each relayed datagram's username, direction and size, with a say in whether it goes (a username's bandwidth); the server's events (allocated, refreshed, freed, refused, credentials failed); whether an address is one of its relayed ones; its permissions, channels and pending allocations counted | on akio's main and pinned (since `d8d51f61`, akio's ADR-186 update 1): `TurnConfig::on_relay`, `on_event`, `TurnServer::holds_relay`, `TurnStats`' new counters | 1, 11 | I111, I112 |
| A258 | akio-turn's settings changed on a running server (credentials, quotas, caps, lifetimes) and a drain: no new allocation while those held run out | on akio's main and pinned (since `d8d51f61`, akio's ADR-186 update 2): `TurnServer::set_limits`, `set_auth`, `drain` | 10, 11 | I111, I112 |
| A259 | A peer's own configuration on an `RtcServer` (`accept_offer_with`): the codecs of one session, as an SFU answers a publisher with the codec it picked of each kind and its viewers with the same | on akio's main and pinned (since `d8d51f61`, akio's ADR-187): `RtcServer::accept_offer_with` | 12 | I113 |
| A260 | RTP forwarding in akio-webrtc for an SFU: a received packet written on another peer's sending line, its SSRC, payload type, sequence and timestamp rewritten, its header extensions mapped | on akio's main and pinned (since `d8d51f61`, akio's ADR-187): `Peer::write_rtp` | 13 | I113 |
| A261 | A receiver's bitrate cap told to the remote sender as REMB, and the ICE consent timeout set per peer | on akio's main and pinned (since `d8d51f61`, akio's ADR-187): `Peer::set_receive_bitrate_cap`, `RtcConfig::set_ice_timeouts` | 10, 11 | I113 |
| A263 | akio-media's RTMP server: a message bound set per server, a refusal told as fit's (`NetStream.Publish.Unauthorized`, `NetStream.Play.Failed`, a retry), the play's statuses after its subscription, a deadline from accept to publish or play | requested by I120 | 4, 11, 19 | I122 |
| A264 | akio-media's RTSP server: a hook on each request (its method, URL, `Authorization` and the connection's session) that may answer it; DESCRIBE asked of the hub's play hook; a URL's query kept for the tracks (`Content-Base`) and taken before a track's control (`.../cam?token=T/streamid=0`); an idle deadline for a connection without a session | requested by I120 | 4, 19 | I122 |
| A265 | akio-media's SRT server: the query of an access control stream ID's resource (`#!::r=live/cam?token=T,m=publish`) split from the stream's path, as of a plain one | requested by I122 | 19 | I122 |
| A266 | akio-media's `HttpFlv` and `Hls` under a path prefix, the request's path left as it came; one `Hls` serving both segment formats (`?format=`) | requested by I123 | 13, 19 | I123 |
| A267 | akio-media's WHEP: an offer for a stream with no publisher held until one comes, within a wait (and a bound on those held), and a viewer whose publisher leaves kept for the next one within a wait | requested by I124 | 10, 11, 19 | I124 |
| A268 | An `RtcServer` on a UDP listener bound to port 0: its host candidates taking the port the listener bound | requested by I124 | 19 | I124 |
| A264 | akio-media's RTSP server: a hook on each request (its method, URL, `Authorization` and the connection's session) that may answer it; DESCRIBE asked of the hub's play hook; an idle deadline for a connection without a session | requested by I120 | 4, 19 | I122 |
| A283 | The BitTorrent client on the caller's engine (fit's fib A79) | requested; stopgap: the client on an engine of its own | 16 | I152 |
| A284 | The BitTorrent client's peers refused by address, its connections and dials bounded, its limits changed at run time (fib's A77, A78) | requested; until then the client's `access`, `max_conns`, `max_half_open` and `disk_queue_bytes` are refused as not supported yet | 11 | I152, I154 |
| A285 | The tracker server's store: a bound of all its peers, bounds and TTL changed in place, its torrents listed and counted (fib's A85) | requested; until then `max_peers` is refused, and a change of the bounds makes a new store | 11 | I151 |
| A286 | A torrent's own storage directory for a magnet link, once its info arrives (fib's `StorageOpener`) | requested | — | I153 |
| A287 | A reader of a torrent's bytes that waits for their pieces and asks for them first, and file priorities | requested | 1 | I155 |
| A288 | A torrent's trackers with their announces, its peer sources and web seeds' state, and a recheck in place (fib's A87, A88) | requested | — | I157 |
| A277 | A client response's peer certificate chain, and a client TLS verifier that keeps a certificate it refused for the caller to see | requested by I132; a recorded stopgap (a probe of the member's certificate) | 1, 15 | I132–I139c |
| A301 | `H2Client` ends its connection at the engine's graceful shutdown (`GOAWAY`; closed now with no stream open, else after its last), as the HTTP/1 client closes its idle ones | requested; stopgap: `EngineSlot::shutdown`, which `akit run` calls before the engine's shutdown, closes the pools' idle h2 connections | 4, 10 | I18, I18b |

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
in any certificate (I09's `sni_cert` stopgap refused one past the first). I34 answers `CertDone`
later for a name taken on demand: from the ask client's thread when the name is refused, from an
ACME worker once its certificate is obtained.

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

**Built** (akio ADR-121): `ServerConfigBuilder::tls12_only(bool)` and
`ClientConfigBuilder::tls12_only(bool)`. akit-tls sets it on a listener's TCP configuration when
`max_version` is `1.2`; HTTP/3 stays TLS 1.3.

## A10 Closing an HTTP client's idle connections

**Status.** On akio's main (`0fc8f3a7`, `Client::close_idle`) and pinned.

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

**Seen again (I38).** The churn test: every reload that changes an upstream replaces its pool, and
the replaced pool's idle connections stay open up to `idle_timeout`; thousands of reloads a minute
leave thousands of upstream connections open (I38 shares an unchanged upstream's pool with the next
graph, as fit does, which leaves only the changed ones).

**Built (I38).** On akio branch `claude/a10-close-idle`, commit `af959290` (off `e79cc740`), not
pushed or merged: `Client::close_idle` closes the idle connections now and sets the client's
`shutting_down` flag (the engine's graceful shutdown's), so that a connection whose response is in
closes rather than going back idle; no change on the request path. Tested by `client`'s
`close_idle_closes_idle_connections_now` (macOS, and gpu51 epoll and io_uring). akit calls it from
`Pool::close` once akio's main has it and akit's `rev` moves there.

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

**Status.** Built on akio branch `claude/akit-open-requests` (`22e038cc`, akio's ADR-154); used by akit, the pin not moved yet. `akio::dns::HappyEyeballs` dials every attempt, a lone address too, with `Engine::dial_with` and its `connect_timeout`, so akio-http's `Client` and `H2Client` honour their connect timeout for any number of addresses. akit no longer sets the engine's `connect_timeout` to the shortest of the upstreams' (akit-listener's `build`); `connect_timeout` in akit-listener's `upstreams` tests checks a lone address held to its own timeout, shorter and longer than another upstream's.

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

**Done** (akio ADR-124, on akio's branch `claude/a57-a58-a14-a15-a92` at `7d1b1670`, not merged
yet, pinned): `Files::index_files`, `redirect_directories`, `mime_types` (`MimeTypes::system`, Go's
table with the system's `mime.types`), `default_type`, `sniff` (Go's `DetectContentType`),
`error_response` and `serve_prefixed`; 403 for a file that may not be read, 404 for what is not a
regular file (never opened) and for a file asked for with a trailing slash, `:` allowed. akit's
`static` handler takes fit's `index` lists and `default_type`, redirects as fit's, answers fit's
short bodies, and titles listings below `strip_prefix` by the whole path.

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

**Done** (akio ADR-124, branch `claude/a57-a58-a14-a15-a92` at `7d1b1670`, pinned):
`Files::max_cache_bytes` (64 MiB by default, twins and compressed copies counted, CLOCK eviction)
and `Files::disk_timeout` (503 with `Retry-After` past it). akit's `static` handler takes fit's
`max_cache_bytes` and waits on the file pool for fit's 10 s.

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

**Status.** Built on akio branch `claude/akit-open-requests` (`22bc4402`, akio's ADR-155); used by akit, the pin not moved yet. `Engine::remove_listener` and `Engine::shutdown_listener` take per-peer UDP listeners (new peers dropped, the others drained, closed at the deadline), `UdpBinding::shutdown` drains a socket bound at run time, and akio-h3 sends `GOAWAY` to a draining listener's connections. akit-listener's reload removes and changes listeners serving `h3` or `udp` as it does TCP ones: the refusal (`ReloadError::Udp`) is gone; the engine's own UDP listener is drained with `shutdown_listener`, one a reload bound with `UdpBinding::shutdown`. A UDP listener replaced on its own address drains for at most `UDP_REPLACED_DRAIN` (2 s), since the new peers the kernel hashes to its sockets go unanswered meanwhile; one removed with nothing in its place drains for the whole drain timeout. Tests: `reload_knows_h3_listeners` and `reload_drains_h3` in akit-listener's `h3` (a startup h3 listener removed, and replaced on its address; a request in flight answered), `udp_listener_removed` in its `l4`.

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

**Built** (akio ADR-121): `Listener::unix(path)`, `Listener::unix_from_std(UnixListener)` and
`Listener::unix_mode(mode)` (set before `listen`), on epoll, kqueue and io_uring; a stale socket
file replaced, one in use or a file that is not a socket `AddrInUse`; addresses `UNIX_ADDR`; the
file left for the caller. akit's Admin API listens there (0600) and removes the file on shutdown.

## A24 An HTTP/1 response that closes its connection

**Status.** Built on akio branch `claude/akit-open-requests` (`22bc4402`, akio's ADR-157); used by akit, the pin not moved yet. An HTTP/1 response that says `Connection: close` closes its connection once sent, from any path, holds its reads (the body it refused is not read) and serves no request pipelined behind it. The Admin API's 401 (`guard`) and `read_body`'s 413s say `Connection: close`, as fit's do; so does `body_limit`'s 413 for a body that streams. Tests: akit-admin's `token` (a request pipelined behind a 401 not served) and `loads_refused` (the connection closed after a 413, the rest of the body not read), akit-middleware's `body_limit_closes_http1`.

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

**Status.** Built on akio branch `claude/akit-open-requests` (`8caab91c`, akio's ADR-158); used by akit, the pin not moved yet. `akio_http::ResponseObserver`, `HttpAcceptor::observer`, `Context::set_observed` and `akio_h3::serve_with_observer`. akit-listener's `Dispatcher::track` has the listener's observer write the access log line and count the request from two words (the graph's ID; the metrics slot, the route's index and whether it is logged), with no allocation. A traced request (its span's IDs do not fit in two words) and a logged one whose route may change its target (`rewrite`, `wasm`: `Middleware::changes_target`; the line writes the target the listener dispatched) keep a response filter. The authenticated `user` is still not logged: two words carry no name.

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

**Status.** Built on akio branch `claude/akit-open-requests` (`57a942af`); used by akit, the pin not moved yet. `Telemetry::open_accepted` (and `Metrics::open_accepted`): akit-listener's scrape writes `akit_connections_open` (fit's `fit_connections_open`) from it, the accepted connections alone.

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

**How akit uses it.** `akit_listener::metrics::Scrape` writes it as `akit_connections_open` (fit's `fit_connections_open`), in
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

**Done** (akio ADR-126, branch `claude/a33-a48` at `2afb267b`, pinned): `Context::req_mut() ->
Option<&mut Request>`, `None` once the request is shared (`detach`, an upgrade); route parameters
keep reading the path they were matched in; a suspended request is resumed with its request whole,
to change. akit's `headers`, `basic_auth`, client-certificate, ForwardAuth and JWT middlewares and
the cache change the request's fields in place (`akit_runtime::set_request_headers` replaces them);
`rewrite` changes its target (`set_request_path`), and akit-listener makes the target's path
canonical, so `REQUEST_FIELDS`, `request_headers` and `CANONICAL_PATH` are gone: the handlers read
`ctx.req()` and `ctx.path()`. The access log writes the target the listener left (fit's
`RequestURI`, which its `rewrite` leaves alone), copied for a logged request.
I71's `wasm` middleware sets the request's fields as its plugin changed them through it, on the
request resumed from the plugin's pool.

## A35 A streamed HTTP/1.1 response framed by its length

**Status.** Built on akio branch `claude/akit-open-requests` (`22bc4402`, akio's ADR-157); used by akit, the pin not moved yet. `stream_response` frames an HTTP/1.1 body by the `Content-Length` its head gives: a body short of it closes the connection, a write past it is refused, and a stream coding or a body filter drops the length and goes chunked. The proxy keeps an upstream's length on every version (but on HTTP/1.1 a response with trailers goes chunked, which alone carries them), and the FastCGI handler keeps the application's (one that is not a number is dropped). Tests: `stream_download` and `broken_download` in akit-proxy's `stream` (framed by its length; cut short of it, the connection closed).

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

**Status.** Built on akio branch `claude/akit-open-requests` (`22e038cc`, akio's ADR-154); used by akit, the pin not moved yet. `akio::timer::after_send` and `SendTimer::cancel`, one bounded timer thread of akio's. akit's own timer thread (`akit_upstream::timer`) is gone: the probes, the h2 pools' idle sweeps, the cache's lock and disk timeouts, the JWK sets' fetches, the FastCGI sweeps, the WebSocket tunnels' idle timeouts, the layer 4 services' opening timeouts, sweeps and drains and the sockmap drain use akio's, and cancel a timer once what it waits for is moot (a probe settled, a connection taken, a fetch ended, a disk read done).

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

**Done** (akio ADR-127, branch `claude/a33-a48` at `2afb267b`, pinned): `Context::on_end(f) ->
bool`, `Detached::on_end(f)` and `End { Sent, Gone }`, each hook run exactly once (`Sent` once the
response was handed to the connection in full, a streamed one's at its finish; `Gone` when the
client went first, heard while the request waits or streams, or the response was given up);
`on_gone` lives beside them. `concurrency_limit` gives its place back from `on_end`. The proxy
already cancels its upstream from `on_gone` (A38). Still open: the access log, metrics and spans
time a streamed response to its head, and write its `Content-Length` as its size; fit writes them at
`OnFinish`, with the bytes written, which `End` does not carry (a body tap could count them, at a
cost per streamed response): a follow-up.

## A49 A request body limited by a middleware

**Status.** Built on akio branch `claude/akit-open-requests` (`22bc4402`, akio's ADR-157); used by akit, the pin not moved yet. `Context::limit_body` bounds a request's body for whoever reads it. Past it, a streamed body's sink hears the new `BodyEvent::TooLarge` and is dropped, and the request is answered 413 unless its handler took the response. `BodyEvent` gaining a variant is a breaking change of akio's (the enum is exhaustive): every sink of akit's matches it (the proxy's, the WebSocket tunnel's, the FastCGI handler's, the Admin API's, the tests'). akit-listener bounds every streamed body with `limit_body(max_body_bytes)`, but a CONNECT's (a tunnel's or an extended CONNECT's stream, which is no body); `body_limit` calls `limit_body(max_bytes)` for every request, after answering itself, in fit's words, a body declared past it or buffered past it (akio's own 413 says "Payload Too Large"); the proxy and the FastCGI handler hear `TooLarge` and answer 413, or cut the response short if it began, giving the upstream request up. akit-runtime's `MAX_BODY_BYTES` local, which the handlers held a streamed body to, is gone. Tests: akit-middleware's `body_limit`, `body_limit_streamed`, `body_limit_h2` (h2c: under, at and over, declared and growing, to a buffering handler and to the proxy) and `body_limit_closes_http1`; akit-handler's `streamed_body_limit` (the FastCGI handler over HTTP/1.1 and h2c).

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
answered 504, an idle one cuts the response short as a broken one is (I17's `Akit-Error` trailer, fit's `Fit-Error`, on
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

**Status.** Built on akio branch `claude/akit-open-requests` (`8caab91c`); used by akit, the pin not moved yet. `HttpConfig::idle_drain_grace` (250 ms by default; akit keeps the default, fit having no such knob). akit's reload test (`crates/akit/tests/reload.rs`) no longer counts a request sent on a draining listener's idle connection as a retry: every client error fails it.

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

**Done** (akio ADR-123, branch `claude/a57-a58-a14-a15-a92` at `7d1b1670`, pinned):
`RootCerts::store()` and `From<RootCerts> for RootStore`. akit's `Ocsp` verifies a certificate from
files against the system's roots unless roots are given (`trust_roots`), as fit's does.

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

**Done** (akio ADR-123, branch `claude/a57-a58-a14-a15-a92` at `7d1b1670`, pinned), as the
TLS setting `ServerConfigBuilder::handshake_only_alpn`: a connection negotiating `acme-tls/1` is
closed once its handshake is done and nothing it sends is served. akit's TLS listeners set it.

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

**Status.** Built on akio branch `claude/akit-open-requests` (`a88c5d7b`, akio's ADR-159); used by akit, the pin not moved yet. `Context::suspend` holds a streamed body's reads until the resumed handler takes it. `forward_auth`, `basic_auth`, `jwt` and `wasm` no longer ask for a body whole to suspend its request; `forward_auth_streams_the_body` in akit-middleware's tests checks a 1 MiB upload streamed to the proxy's upstream, whole and in order, the upstream having the request before the client sent the second half.

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

**Status.** Built on akio branch `claude/akit-open-requests` (`22bc4402`, akio's ADR-155); used by akit, the pin not moved yet. `Conn::set_idle_timeout` and `Conn::set_idle_counts_writes`. akit-l4 gives each relay's client and upstream connections, and each UDP session's two sides, the service's idle timeout with the data they send counted, and akio closes them on their I/O threads. The service's sweep of every connection is gone; it stays for spliced connections alone, whose bytes akio never sees (their akio idle timeout turned off; A170's `Tunnel::moved`). akit-listener no longer raises the engine's UDP idle timeout to the longest of the services' (`udp_idle`, at start and on reload): it is HTTP/3's peers' alone. Tests: `tcp_one_way_is_not_idle` (a download and an upload outlive the idle timeout, and close once quiet) besides `tcp_idle_timeout` and `udp_sessions_expire` in akit-listener's `l4`.

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

**Status.** Built on akio branch `claude/akit-open-requests` (`22e038cc`, akio's ADR-154); used by akit, the pin not moved yet. `akio::DialOptions` and `Engine::dial_with`: akit-l4 dials a TCP upstream's IP literal with the upstream's `connect_timeout` (`dial_udp` needs none: UDP has no handshake).

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

**Done** (akio ADR-124, branch `claude/a57-a58-a14-a15-a92` at `7d1b1670`, pinned): a file
body a response filter gives a stream coding is read, encoded and written in 64 KiB pieces on the
disk pool, under the connection's backpressure. akit's `compress` middleware compresses bodies
from disk.

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

**Status.** Built on akio branch `claude/akit-open-requests` (`57a942af`); used by akit, the pin not moved yet. `akio::{Pool, PoolConfig, Rejected, Job}` from the facade: akit-cache's disk pool uses `akio::Pool`, and the `akio-pool` dependency is gone from akit.

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

## A135 Dialing a Unix-domain stream socket, and the HTTP client over one

**Problem.** The Docker provider (I58) lists containers and follows the daemon's events through
the Engine API, which the daemon serves on a Unix-domain socket, `/var/run/docker.sock`, by
default. Rule 2 has akit's requests go through akio's asynchronous clients; akio's engine dials
TCP only and its HTTP client resolves the URL's host, so akit could reach the socket only with a
blocking client of its own.

**Evidence.** At `e9891b52`: `Engine::dial` and `dial_from` take a `SocketAddr`;
`akio_sys::connect_nonblocking` makes `AF_INET`/`AF_INET6` sockets only; epoll's and io_uring's
`finish_connect` read the peer and local addresses as IP ones; `Client::dial` resolves
`key.host` and dials the addresses.

**Suggested API.** `Engine::dial_unix(path, acceptor, on_error)`, a Unix-domain connection served
as a dialed TCP one (its `peer_addr` and `local_addr` a fixed `UNIX_ADDR`); and
`Client::with_unix_socket(Option<PathBuf>)`, every request of the client sent to that socket, the
URL's host its `Host` field and the key of its idle connections.

**How akit uses it.** `akit_provider::docker` builds its client with `with_unix_socket` for a
`unix://` endpoint and runs `GET /events` and `GET /containers/json` on it, their callbacks on
akio's I/O threads. Done in akio by I58 (ADR-120, branch `claude/a135-unix-dial`, `0f2c419b`),
to be merged onto akio's main.

## A148 The address a request came to

**Status.** Built on akio branch `claude/akit-open-requests` (`22bc4402`, akio's ADR-157); used by akit, the pin not moved yet. `Request::local` and `Context::local_addr`. The FastCGI handler's `SERVER_ADDR` and `SERVER_PORT` are the address the request came to, as fit's are from its TCP connection; with none (port 0: a Unix-domain socket), as before, empty and the `Host` field's port. Over HTTP/3 akio gives the UDP socket's address, which akit uses where fit (whose address comes from a TCP connection only) leaves `SERVER_ADDR` empty. Tests: `server_address` in the handler's `params` and `params` in akit-handler's `fastcgi`.

**Problem.** A FastCGI application reads the address and port a request came to as CGI's
`SERVER_ADDR` and `SERVER_PORT` (PHP's `$_SERVER['SERVER_ADDR']`), which fit fills from its
connection's local address. A handler on akio's `Context` sees the request's peer
(`Request::peer`) but not the local address of the connection it came on, so akit's `fastcgi`
handler (I75) sends `SERVER_ADDR` empty and `SERVER_PORT` from the `Host` field (or 80 and 443),
as fit does only when its connection is not TCP. A listener bound to several addresses (`0.0.0.0`,
`[::]`) cannot tell either; neither can the access log's `server_addr` or a matcher on it.

**Evidence.** At `e9891b52`: `crates/akio-http/src/message.rs` has `Request::peer` and no local
address; `Context` has `peer()` and nothing for the other end; `akio_core::Conn::local_addr`
exists but a request handler never sees the connection.

**Suggested API.** `Request::local: SocketAddr` (or `Context::local_addr()`), filled as `peer` is:
the connection's local address on HTTP/1.1 and h2, the UDP socket's on h3 (an unspecified address
where there is none, as for a virtual connection).

**How akit uses it.** The `fastcgi` handler's `SERVER_ADDR` and `SERVER_PORT`, as fit's; later,
any field or matcher that names the address a request came to.

## A132 A proxy tunnel's upstream checked once connected

**Problem.** fit's forward proxy (a listener's `socks`, the `forward_proxy` handler) decides a
destination host name by the address it resolved to, differently by the name: one its `allow`
names is refused only by a `deny` network or by the refusal of internal addresses (loopback,
link-local, multicast, unspecified, the host's own), another needs an `allow` network to hold its
address. The host's own address shows only on the open connection, as one coming from the
address it goes to. fit decides once connected, before anything is sent, so that DNS cannot
answer the check one way and the dial another. akio-socks has `ProxyConfig::destinations`, which
sees each resolved address without the name or the local address, and `Socks::allow`, which sees
the name unresolved. Behind a trusted PROXY header, fit also checks the SOCKS client the header
names; akio-socks's access hook saw the socket's peer.

**Evidence.** At `e9891b52`: `crates/akio-socks/src/lib.rs`, `Destinations::allows(SocketAddr)`
filters resolved addresses before the dial; `Request::client` is `c.peer_addr()`.

**Suggested API.** `ProxyConfig::connected: Option<Connected>`, `Connected = Arc<dyn Fn(&Dest,
SocketAddr, SocketAddr) -> bool>`: called on the upstream's I/O thread once it opens, with the
destination as asked and the remote and local addresses; `false` closes it and fails the request
as not allowed (SOCKS5 2, SOCKS4 `0x5b`, HTTP 502), no other address tried. `Request::client` is
`akio_mux::proxy::client_addr(conn)`.

**How akit uses it.** `akit_l4::forward::Rules::proxy_config` sets `destinations` to `Any`, decides
the request in the access hook (and, for HTTP, in the handler) by what was asked, and the address
in `connected` (fit's `Rules.resolved` and `netguard.Refused`). Done in akio by I55 (ADR-119,
branch `claude/a132-a133-socks-rules`, `9b096d0d`), to be merged onto akio's main.

## A133 A proxy's connect timeout of its own

**Problem.** fit's proxies have `connect_timeout` (10s): resolving and connecting may take that
long, answered `504` over HTTP and "host unreachable" over SOCKS5, apart from the SOCKS
handshake's timeout. akio-socks's `handshake_timeout` covers the greeting through the connect
and closes the client without a reply.

**Evidence.** At `e9891b52`: `crates/akio-socks/src/guard.rs` has the handshake and idle
timeouts only; a fired timeout calls `client.close()`.

**Suggested API.** `ProxyConfig::connect_timeout: Option<Duration>`, from the `CONNECT` request to
its upstream connected; past it SOCKS5 reply 4, SOCKS4 `0x5b`, HTTP `504`, then the close.

**How akit uses it.** Each forward proxy's `connect_timeout`, with a handshake timeout of 10s
(fit's `OpeningTimeout`) plus it. Done in akio by I55 with A132 (ADR-119).

## A134 CONNECT tunnels over HTTP/2 and HTTP/3

**Problem.** fit's `forward_proxy` tunnels a CONNECT on an HTTP/1.1 connection, an h2 or h2c
stream (RFC 9113 §8.5) and an h3 stream (RFC 9114 §4.4). akio-http dispatches an HTTP/2 stream
as a tunnel only for an extended CONNECT (`:protocol`, A94), and akio-socks's `connect_proxy`
tunnels with `Context::upgrade_raw`, HTTP/1 only: on an HTTP/2 stream it returns `false` and the
request is never answered.

**Evidence.** At `e9891b52`: `crates/akio-http/src/h2.rs` sets `st.connect`/`streaming` only when
`req.protocol.is_some()`; `Context::upgrade_raw` documents "HTTP/1 only; `false` otherwise";
`connect_proxy_with` ignores its result.

**Suggested API.** A plain CONNECT on an HTTP/2 or HTTP/3 stream dispatched at once with its DATA
streamed as the request body, and a streamed `2xx` to it opening a tunnel whose halves close
separately (as A94 does for extended CONNECT); `connect_proxy` relaying such a stream to its
upstream with the same backpressure, timeouts and checks as its HTTP/1.1 tunnels.

**How akit uses it.** The `forward_proxy` handler hands every CONNECT to `connect_proxy`; until
then it answers one on an HTTP/2 or HTTP/3 stream 501 (a stopgap, in the progress log).

**Built** (akio ADR-122): akio-http dispatches an HTTP/2 CONNECT (`:method` and `:authority`
alone, the target the authority) at once and akio-h3 one at its head; a streamed `2xx` is a
tunnel. `connect_proxy` relays it on its stream with the HTTP/1.1 tunnel's checks, timeouts and
statuses and backpressure both ways; the client's END_STREAM shuts the upstream's send direction
down (the new `Conn::shutdown_write_after_send`). `H2Client` sends such a CONNECT. The 501 is gone.

## A165 A WebAssembly runtime for plugins

**Problem.** fit runs WASM plugins on each request of a route (decision 017: wazero, fit's own
ABI; decisions 018 and 023: the Proxy-Wasm subset, its request and response callbacks), on a pool
of its own with time and memory limits. akit has no `unsafe` and no async runtime (rules 1 and
21), so it cannot carry a WebAssembly runtime itself, and akio has none.

**Evidence.** At `8453b60c`: no crate of akio decodes or runs WebAssembly; akio-http has
synchronous response filters (`add_response_filter`) and body taps (ADR-116), but nothing that
holds a response's head, or passes its body through a call made elsewhere, until that call
answers.

**Suggested API.** Designed in akio's
[ADR-125](https://github.com/lesismal/akio/blob/main/docs/adr/125-wasm-runtime-for-plugins.md)
(proposed; on akio branch `claude/wasm-design-95c5ec` until merged): a new crate `akio-wasm`,
`#![forbid(unsafe_code)]` and without dependencies, an interpreter of WebAssembly 2.0's core
without SIMD (what Go, TinyGo and Rust guests emit); `Module`, `Linker`, `Instance` with memory
and stack limits and a per-call budget (deadline, fuel, interrupt) checked per basic block; a
WASI preview 1 subset; `InstancePool`, threads that own instances, with bounded shared and
per-instance queues. For I72's response phase, two akio-http hooks (H1: a response's head held
until a call elsewhere hands it back; H2: a streamed body filtered by calls made elsewhere, one at
a time), fit's A41 and A46 from fib, to be requested on their own when I72 starts. Milestones
W1-W6 in the ADR; I71 needs W1-W4 and A33.

**How akit uses it.** `akit-plugin` implements fit's ABI under akit's names (module `akit`, decision 037; with `akit_sync`'s names
trapping if called) and Proxy-Wasm's host functions on `akio-wasm`'s safe API, suspends each
request (`Context::suspend`, ADR-109) and resumes it from the pool when the call returns, as fit's
`wasm` middleware does with fib.

**Built: W1-W4** (akio ADR-125, accepted; branch `claude/akio-wasm-w4-445f36`, `a2373416`, merged
with A33 on `claude/i71-akio-base`, `1ae0a05f`; akit pins `dad77a7a`, W6 on top of it, in akio's
`main`): the
decoder and validator, the interpreter, the host API (`Linker`, `Caller`, typed functions) and WASI
preview 1, and `InstancePool` (deadline per job, a full queue refused with the job handed back,
instances made again after a trap). I71 runs fit's ABI on it (`akit-plugin`); W5 (its cost
measured) and the response hooks (H1, H2), are for I72 and the benchmark tasks.

## A166 A response's head held while a call made elsewhere decides on it

**Status.** Built on akio branch `claude/akio-wasm-w6-afc4f2` (`dad77a7a`), as akio's
[ADR-128](https://github.com/lesismal/akio/blob/main/docs/adr/128-held-response-heads-and-body-filters.md),
milestone W6 of ADR-125; in akio's main (`dad77a7a`), pinned.

**Problem.** fit calls a Proxy-Wasm plugin's `proxy_on_response_headers` on the plugin's pool
(decision 023, with fib's A41 `OnHeaderAsync`). akio-http's response hooks, response filters
(`add_response_filter`, `add_outer_response_filter`), run where the head is settled, often an I/O
thread, and decide there: akit cannot run the plugin there (rule 4), nor hold the head until the
pool answers.

**Evidence.** At akio `1ae0a05f`: `Finish::apply` runs the filters synchronously from `Out::send`
and `start_stream`, and the head goes as soon as they return; nothing holds it.

**As built.** `Context::hold_response_head(timeout, f: impl FnOnce(HeldHead, &Request) + Send)`.
`f` runs where the head would go, after the ordinary response filters and the extra fields, before
the outer filters; it hands the `HeldHead` on and returns. `HeldHead` has `status`, `headers`,
`headers_mut`, `body` (a whole body), `is_live`, and, from any thread, `resume()`,
`respond(Response)` (a local response in its place: unlike fib, akio can, as nothing has gone)
and `fail()`; dropped, it fails. A failure or the deadline cuts the response (HTTP/1 connection
closed, h2 and h3 stream reset, `End::Gone`). Meanwhile a whole response waits whole, and a
streamed body written meanwhile is held up to 4 MiB (in `BodyStream::queued_bytes`, so a paced
producer waits as for a slow client). Responses to `CONNECT` and upgrades pass over it, as in fib.
HTTP/1.1, h2 and h3 alike.

**How akit uses it.** `akit-plugin`, for a Proxy-Wasm plugin that exports
`proxy_on_response_headers`, registers a hold on each request it lets go on, and from the hook
queues the call for the instance that has the request's context (`InstancePool::submit_to`),
with the head's fields; the completion applies the plugin's changes and resumes, or answers its
local response with `respond`, or fails the response as `on_error` says. The changes the request
phase recorded for the response are applied before (a response filter, as now).

**In use since I72** (`crates/akit-plugin/src/response.rs`): as above, the hold registered only
for a filter that exports `proxy_on_response_headers`, for at most `wait_timeout` + `timeout` +
1 s; a local response from it is sent with `HeldHead::respond`
([decision 011](decisions/011-proxy-wasm.md)).

## A167 A response body filtered by calls made elsewhere

**Status.** Built on akio branch `claude/akio-wasm-w6-afc4f2` (`dad77a7a`), as akio's ADR-128,
milestone W6 of ADR-125; in akio's main (`dad77a7a`), pinned.

**Problem.** fit calls `proxy_on_response_body` and `proxy_on_response_trailers` on the plugin's
pool as the body is written (decision 023, with fib's A46 `OnBodyFilterAsync`). akio-http's body
hooks, a stream coding (ADR-112) and a body tap (ADR-116), run on the thread writing the body,
and a tap only sees it: nothing lets a call made elsewhere replace the body, hold it to its end, or
change its trailers.

**Evidence.** At akio `1ae0a05f`: `BodyStream::write` encodes and writes at once; `BodyTap::data`
gets a copy and returns nothing.

**As built.** `Context::add_body_filter(impl BodyFilter)`:
`BodyFilter::filter(&mut self, BodyChunk { data, end, trailers }, done: FilterDone)` and
`BodyFilter::abort(&mut self)`; `FilterDone::done(data, trailers)` or `fail()`, from any thread.
One call at a time with everything written since the last; what is written meanwhile is held, up
to 4 MiB, and goes to the next call. The last call (`end`) carries the trailers (empty when none),
which it may change or add. The output goes on to the next body filter, then through the stream
coding (so `compress` codes the plugin's output, and the plugin sees the identity body) and any
tap; it is paced by the client (the next call waits while the connection holds more than
256 KiB). A whole response goes as a streamed one, without `Content-Length`, in one last call; a
file body is read on the disk pool into the filter. A failed call, a dropped `FilterDone` or the
bound cuts the response; `abort` hears a response that ended before its last call. Not called for
`HEAD`, `1xx`, `204`, `304`, `CONNECT` or upgrades. `on_end` (A48) runs after the last call's
output went, which is when the done, log and delete callbacks go. HTTP/1.1, h2 and h3 alike.

**How akit uses it.** For a Proxy-Wasm plugin that exports `proxy_on_response_body` or
`proxy_on_response_trailers` (`response: all`), `akit-plugin` adds a body filter whose calls are
queued for the instance with the request's context: `proxy_on_response_body` with what the plugin
holds (it answers with nothing while it pauses, and with all it held at the end, up to the
plugin's `max_body_bytes`), the last call `proxy_on_response_trailers` with the trailers. `on_end`
queues `proxy_on_done`, `proxy_on_log` and `proxy_on_delete`. `response: headers` registers no
body filter, so file bodies keep `sendfile`.

## A168 HTTP/2 with prior knowledge alone

**Status.** Built (I79b, 2026-11-26) on akio branch `claude/i79b-akio`, not in akio's main yet: `HttpConfig::http1`
(default `true`; with `false` a connection that does not begin with the preface gets a `GOAWAY`
`PROTOCOL_ERROR` and is closed, no `Upgrade: h2c`), akio's ADR-174. akit's I79b branch sets it for
a listener of `h2c` alone and drops its "not supported yet"; akit's main moves to it with the pin.

**Problem.** fit's listener with `protocols: [h2c]` serves HTTP/2 with prior knowledge and
nothing else, as fit's benchmark config's port 8081 does. akio-http's server always takes
HTTP/1.1 on a connection that does not start with HTTP/2's preface, so akit refuses such a
listener when it starts (`not supported yet: protocols h2c without http/1.1`) and its benchmark
config lists `http/1.1` beside `h2c`.

**Evidence.** At akio `dad77a7a`: `HttpConfig` has `h2: Option<H2Config>` to turn HTTP/2 off, and
no setting to turn HTTP/1 off; `Session` reads any connection without the preface as HTTP/1.

**Suggested API.** `HttpConfig::http1: bool` (default `true`): when `false`, a connection that
does not begin with the HTTP/2 preface is closed (or answered with a GOAWAY after the preface it
lacks, as Go's h2c-only server does), and an `Upgrade: h2c` request is not taken either.

**How akit would use it.** `akit-listener`'s `http_config` sets `http1` from whether the listener
lists `http/1.1`, and drops its "not supported yet". `akit validate` reports the same until then
(I25 found that it did not).

## A169 Malformed requests refused by akio's servers

**Status.** The first three parts on akio's main (`0fc8f3a7`) and pinned; the fourth is not built. The same-run A/B of the HTTP/2 request path that was to come before akio's main was not run before the merge: it is owed as a follow-up ([decisions waiting for review](decisions-pending.md)).

**Problem.** I25's conformance suite (`crates/akit/tests/conformance.rs`) found requests akio's
servers hand to the application although the RFCs call them malformed, so that akit's proxy sent
them on to an HTTP/1.1 upstream:

1. HTTP/1: a request with two `Host` fields was served (RFC 9112 section 3.2 asks for 400). An
   upstream may route by the other one.
2. HTTP/2: a field value with a NUL, CR or LF, and a field name with a byte outside a token
   (a space, a non-ASCII byte) were accepted (RFC 9113 section 8.2.1 makes the request malformed).
   akio's HTTP/1 client dropped the CR and LF when it wrote them upstream, and wrote the NUL.
3. HTTP/3: the same (RFC 9114 section 4.2).
4. HTTP/1, a streamed body: a bad chunk size after the request was dispatched closes the
   connection without an answer; fit answers 400 (RFC 9112: "SHOULD respond with a 400").

**Evidence.** At akio `dad77a7a`: `Session::on_head` keeps `host` as a bool; `h2.rs`'s
`build_request` and trailer decoding check names for uppercase only and values not at all;
akio-h3's `header_list` likewise; `Session`'s `State::Stream` closes on a `ChunkDecoder` error.

**As built (parts 1–3).** `Session::on_head` counts `Host` fields and answers 400 to more than
one in any version. HTTP/2's request and trailer fields: a regular name must hold only bytes in
0x21–0x40 and 0x5B–0x7E but `:`, and every value (pseudo-header fields' too) no NUL, CR or LF,
else the stream is reset with `PROTOCOL_ERROR`. akio-h3's request fields the same.

**Suggested (part 4).** When a streamed request body's framing fails before any of the response
was written, answer 400 and close, and tell the handler its request is gone (A38's `on_gone`), so
it gives up its upstream exchange.

**How akit uses it.** Once pinned, akit-proxy's stopgap (`Refused::BadFieldValue` and
`Refused::TwoHosts` in `headers::outgoing`, which answer 400 in the loop that copies the fields)
can stay as defence in depth, as fit's check of field names does, or go; with part 4 the
conformance suite's bad chunk size case expects 400 as fit's.

## A171 Client requests' timer jobs cancelled when they end

**Status.** On akio's main (`0fc8f3a7`) and pinned, merged with main's panic-safe timer thread. The same-run A/B that was to come before akio's main was not run before the merge: it is owed as a follow-up ([decisions waiting for review](decisions-pending.md)).

**Problem.** I25's first run measured akit's proxy at 1.8 GiB peak memory (fit 28–73 MiB, nginx
about 200 MiB) at 230k requests a second. Memory grew by about 240 bytes a request, whatever the
pool's bounds, and stayed after the load: `akio_http::timer::after` queues a boxed job on one
timer thread for every client request with a `timeout` (`Client::request`) and for every h2
stream's timeout and header and idle checks (`H2Client`), and the job stays until its delay
passes, holding the request's `Exchange` allocation through its `Weak` (or, for an h2 request,
its callback and stream control). With fit's 60s route timeout that is a minute of requests,
about 14 million at that rate; with akit's default bound of an exchange, 1h, an hour of them.
The queue is unbounded too (rule 11).

**Evidence.** At akio `dad77a7a`: `timer.rs` has `after` and no way to cancel; `client.rs`'s
exchange schedules `give_up(GaveUp::Timeout)` per request; `h2client.rs` schedules per request
and per stream phase. On the benchmark host, akit at `9d9321e` proxying to a local backend with
wrk at 256 connections for 10s: pinned, its RSS rose 104, 198, 306, 437, 518 MiB at 2–10s and
stayed at 525 MiB after; with A171, 17–18 MiB throughout, and 246k requests a second against
238k.

**As built.** `timer::after` returns a key and `timer::cancel(key)` drops the job at once; an
`Exchange` cancels its timeout's job when dropped, an h2 stream's control its pending check and
streamed request's timeout, and a whole h2 request its timeout when its callback runs. Cancelled
jobs' heap entries are dropped once they outnumber the live jobs (past 4096).

**How akit uses it.** Nothing to change; akit's proxy, forward auth, mirrors and health probes
all send through akio's clients with timeouts and stop holding memory per request once it is
pinned. Until then akit's proxy grows by the requests of its route's timeout window under load.

## A172 A ClientHello whose server name is an IP address

**Status.** On akio's main (`0fc8f3a7`) and pinned.

**Problem.** fit's benchmark harness sends its HTTPS/1.1 load with wrk to
`https://127.0.0.1:8443/`. wrk (OpenSSL) puts the URL's host in the ClientHello's `server_name`
even when it is an IP address, which RFC 6066 does not allow. akio-tls's server answered such a
ClientHello with a `decode_error` alert, so every request failed: I25's first run measured
akit at 0 requests a second over HTTPS/1.1 in every scenario (2 million connect errors each),
while Go's, OpenSSL's and nginx's servers, and so fit, nginx and Caddy, take such a name as none.
A host name with a trailing dot failed the same way.

**Evidence.** At akio `dad77a7a`: `proto/msgs.rs`'s `parse_host_name` refuses a name that parses
as an `IpAddr` or ends with `.`, and its unit test pins it. `openssl s_client -servername
127.0.0.1` gets alert 50; with `-servername localhost`, and wrk to `https://localhost:...`, akit
answers.

**As built.** An IP literal as the server name is taken as no name (the handshake goes on, the
default certificate is chosen, `server_name()` is `None`); a trailing dot is dropped; an empty or
non-ASCII name still fails the handshake.

**How akit uses it.** Nothing to change: akit's certificate choice already falls back to the
default certificate without SNI. Once pinned, the harness's HTTPS/1.1 scenarios measure akit.

**In use since I72** (`crates/akit-plugin/src/response.rs`): as above, except that the context is
closed (done, logged, deleted) in the last call, or by a job queued when the response's hooks are
dropped, rather than from `on_end`.

## A170 A connection's TCP counters, and a splice with a first write

**Status.** Built on akio branch `claude/akit-open-requests` (`22bc4402`, akio's ADR-156); used by akit, the pin not moved yet. `akio::ebpf::{Relay::tunnel, Tunnel, TunnelSide, conn_byte_counts, ByteCounts::unread, ByteCounts::consumed, SpliceError}` and akio-mux's `ProxyHeader::wire_len`. akit-l4 splices with `Relay::tunnel` (what of `first` the splice did not write goes through the engine, by `SpliceError::written`), and follows a spliced connection with `Tunnel::moved` (its idle timeout, by the service's sweep), `Tunnel::close_after_forward` (a side's end, on akio-ebpf's drain thread) and `Tunnel::late_payload`; `Conn::inner`, akit's own counters and its drain on akio's timer are gone. A connection that began with a PROXY header the listener read is spliced too: the tunnel's base counts what userspace read. [Decision 010](decisions/010-sockmap-relay.md) says so. Tests: akit-listener's `sockmap`, run as root on Linux (`client_proxy_header_spliced` now expects a splice).

**Problem.** A `tcp` service splices each connection with `akio::ebpf::Relay` once its upstream
opens. A spliced connection's bytes never reach its handlers, so akit reads the sockets' counters
(`akio::ebpf::byte_counts`) for the idle timeout and to close a side only once the kernel has
forwarded what the other received. `byte_counts` takes a `&std::net::TcpStream`, and a splice that
must write first (the service's PROXY header, the ClientHello akit read to pick the service) needs
`Relay::splice_inner_with`, which takes `&ConnInner`; the only way from a `Conn<H>` to either is
`Conn::inner`, hidden from akio's docs as being for akio's protocol crates. akio-socks, which does
the same for `CONNECT` tunnels, keeps its splice control (`SpliceCtl`, its drain thread) private.
And to know how many bytes the kernel relays for a side, akit counts what userspace read from the
client; a PROXY header read by akio-mux's `ProxyProtocol` is consumed without saying how long it
was, so akit relays such connections in userspace.

**Evidence.** At `dad77a7a`: `crates/akio-ebpf/src/relay.rs` has `byte_counts(&TcpStream)` and
`Relay::splice_inner_with(&ConnInner, &ConnInner, ..)`; `crates/akio-core/src/conn.rs` has
`#[doc(hidden)] pub fn inner(&self) -> &Arc<ConnInner>` on `Conn`; `akio` does not re-export
`ConnInner`; `crates/akio-mux/src/proxy.rs` hands off after `skip` bytes without recording `skip`.

**Suggested API.** `Conn::byte_counts() -> io::Result<ByteCounts>` (or
`ebpf::conn_byte_counts(&Conn<H>)`), with `ByteCounts` gaining `unread` (`SIOCINQ`), so that a
proxy can take a splice's base as `received - unread` without counting what userspace read;
`Relay::splice_conns_with(&Conn<A>, &Conn<B>, first: &[u8])`, writing `first` to `b`'s socket
ahead of the relayed bytes and returning the base counters of both; and the PROXY header's length
beside the header akio-mux attaches (`ProxyHeader::wire_len`). Better still, akio-socks's spliced
tunnel (the counters, the idle probe and the drain on close) as a public type in akio-ebpf.

**How akit would use it.** akit-l4's `sockmap` module would call these instead of `Conn::inner`,
and splice the connections of a listener with `proxy_protocol` too.

## A180 akio-wasm's call made cheaper, measured against wazero (akio's W5)

**Done.** On akio's main (`077a6710`) and pinned: a call of fit's Go `headers` guest went from 9,089 ns to 5,068 ns on gpu51 (wazero's compiler 3,520 ns), in akio's docs/BENCHMARKS. akit's `wasm` scenario is left for a benchmark task to measure again.

**Status.** Requested by I76 (2026-11-25), with the first numbers for akio's W5 (ADR-125 section
8, "a performance pass against fit (wazero)", still open in akio's development plan). Nothing in
akit waits on it: the cost is bounded and measured, and M5's criterion holds.

**Problem.** A request through the `wasm` middleware costs akit about twice what it costs fit with
the same `.wasm`. fit runs the guest on wazero's compiler; akit on akio-wasm's interpreter (no
`unsafe`, so no compiler), and the plugin's share of a request is most of what akit spends on it.

**Evidence.** The [M5 report](../bench/results/m5-report-2026-11-25.md#a-wasm-plugin-that-edits-headers):
fit's own header-editing test plugin (`plugin/wasm/testdata/headers`, a Go `wasip1` guest of
1.2 MB gzipped), the fixed response behind the `wasm` middleware with its defaults (7 instances, a
queue of 1,024), fitbench on the benchmark host, 256 HTTP/1.1 connections. akit: 312,679 requests
a second at 22.25 µs of CPU each, against 831,888 at 7.52 µs without the plugin in the same
container, so 14.7 µs a request for the plugin. fit: 452,353 at 14.92 µs against 785,657 at 8.15
µs, 6.8 µs. akit's peak memory with the plugin, 81 MiB, is a third of fit's 235 MiB. What akit's
14.7 µs is made of (the hand-off to the pool and back, the request copied in, the guest's own
instructions, the host calls) is not measured apart yet.

**Suggested API.** None new: W5's pass over akio-wasm's interpreter and `InstancePool`, measured
first apart (a guest call alone on an instance, against wazero's interpreter and compiler, as fit's
`BenchmarkGuestCall`; the pool's hand-off and wake-up), then what the profile names: the dispatch
loop, the per-block budget check, the host calls' argument passing, the memory bounds checks; and
whatever makes entering a Go `wasip1` guest's export cheap to start.

**How akit would use it.** As it is: `akit-plugin` runs every guest on `akio_wasm::InstancePool`;
a cheaper call needs no change in akit, which measures it again with
`bench/run.sh -servers akit-wasm,fit-wasm -scenarios fixed,wasm`.

## A200 A request target in absolute form, taken as RFC 9112 asks

**Done.** On akio's main (`077a6710`, akio's ADR-177) and pinned: akio gives a target in absolute form in origin form, the target's host in place of `Host`, and `Request::raw_target()` as sent. akit-listener's rewrite is gone and the access log writes `raw_target()`. One difference from fit: an http URI with no host (`http:///x`) is answered 400 by akio (RFC 9110 section 4.2.1), where fit routes it by its `Host` field ([decisions waiting for review](decisions-pending.md), row 15).

**Status.** Requested by I77 (2026-11-26). akit-listener rewrites such a request meanwhile, a
stopgap recorded in [progress](progress.md).

**Problem.** An HTTP/1.1 server must accept a request target in absolute form
(`GET http://a.example/admin/x HTTP/1.1`) and, as an origin server, take the target's authority
as the request's host, ignoring `Host` (RFC 9112 section 3.2.2); Go's server does (`r.URL.Path`
is `/admin/x`, `r.Host` is `a.example`), and so fit routes such a request by its path. akio's
HTTP/1 server accepts the target and keeps it whole: `Request::path()` is
`http://a.example/admin/x`, and `Host` stays what the field said. A router that goes by
`path()`, akit's or akio-http's own, takes the request for no path it knows, while a proxy that
sends its upstream the origin form (akit's does, as it must) asks for `/admin/x`: fuzzing found
that a request spelled so missed a route protecting `/admin/*` and reached the upstream through a
catch-all route.

**Evidence.** At `dad77a7a` (and `111004f8`): `crates/akio-http/src/message.rs`,
`Request::path()` is the target up to its `?`; `crates/akio-http/src/parse.rs` copies the
request line's target as it is (`new_target`). akit's test `path_confusion`
(`crates/akit-listener/tests/listeners.rs`) answered `GET http://a/admin/secret` from the
catch-all (200) before the stopgap.

**Suggested API.** The HTTP/1 server parses an absolute-form target into its parts: the
`Request`'s `target` in origin form (`/admin/x?q`) and its host the target's authority (without
user information), set as `Host` or given by a `Request::authority()` that `Host` defers to; the
absolute form kept for logs if wanted (`Request::raw_target()`). A target in authority form stays
for `CONNECT` alone.

**How akit would use it.** akit-listener's `absolute_form` rewrite (in `Dispatcher::call` and
`body_mode`) goes; routing, the proxy, the cache's keys and the access log read the request as
akio gives it.

## A214 A budget of the bytes queued for every connection together

**Status.** Built (I79b, 2026-11-26) on akio branch `claude/i79b-akio`, not in akio's main yet:
`Config::max_pending_bytes` (akio's ADR-172; a watcher thread sums the I/O threads' queue gauges
every 1–10 ms, connections pause through `ConnInner::read_held` and are woken through their loops;
`LoopCounters::budget_pauses`, `Telemetry::pending_bytes`), with a same-run A/B in akio's
docs/BENCHMARKS. akio has no helper for a share of memory: akit reads it (decision 032). akit's
I79b branch reads `limits.max_pending_bytes` onto it; until the pin moves, akit's main refuses it.

**Problem.** fit's `limits.max_pending_bytes` bounds the bytes of responses waiting, across every
connection, for their clients to take them: past it, every connection stops reading until some go
("50%" of the memory fit may use by default, a number of bytes or a share; fib's
`Config.MaxPendingBytes`). It is what keeps many slow clients together from holding the memory
that each one's watermark allows: 100,000 connections at a 256 KiB watermark may queue 25 GiB.

**Evidence.** At akio `dad77a7a` (and `main` at `111004f8`), `akio_core::Config` has
`write_high_watermark` (reads paused per connection) and `max_queued_bytes` (a send refused per
connection), and the telemetry sums the bytes queued (`queued_bytes`), but nothing pauses reads on
that sum. akio's roadmap lists an "engine-wide `max_pending_bytes`" under backpressure, and its dev
plan checks it, but no setting of that name or meaning is in the code. akit cannot build it on top:
it would have to hold every connection's reads from outside the I/O threads that own them, which is
akio's job (rule 21).

**Suggested API.** `Config::max_pending_bytes: Option<usize>` (default `None`, live-reloadable):
while the bytes queued across the engine's connections are at or past it, every connection's reads
are paused after its current read, as the watermark pauses one connection's; they resume once the
sum is back under, say, three quarters of it. A counter of the pauses it caused beside the
watermark's (`backpressure`), so akit's `akit_reads_paused_total{reason="budget"}` reports it, and a
helper that turns a share of the memory into bytes (the machine's, or the cgroup's limit where
lower), or akit computes it.

**How akit would use it.** `akit-listener`'s `engine_settings` sets it from `limits.max_pending_bytes`
(bytes, or the share of memory, "50%" by default, 256 MiB where the memory cannot be read), at
start as fit's; `supported.txt` marks the field, `make v1-bless` marks the addition, and the gate's
`limits` and `proxy` examples are valid.

## A215 A connection limit that counts only the connections accepted

**Status.** Built (I79b, 2026-11-26) on akio branch `claude/i79b-akio`, not in akio's main yet:
`Config::max_accepted_connections` (akio's ADR-173), on the counters of A30's
`Telemetry::open_accepted`. akit's I79b branch sets it from `limits.max_conns` in place of
`max_connections`, and the reference's note is gone; akit's main keeps `max_connections` until the
pin moves.

**Problem.** fit's `limits.max_conns` bounds "the client connections the listeners keep open at
once, across all of them". akit's upstream pools dial on the engine that serves the listeners, so
with akio's limit a busy proxy's upstream connections take room from its clients: at the default
100,000 and 30,000 upstream connections open, the 70,001st client is refused.

**Evidence.** At akio `dad77a7a`, `Config::max_connections` counts "engine-wide, dialed
connections and UDP peers included", though only an accepted connection is closed past it.

**Suggested API.** `Config::max_accepted_connections: usize` (0, no limit, by default;
live-reloadable as `max_connections` is): accepted TCP connections past it, across the engine's
listeners, are closed right after `accept` and counted as `rejected`, whatever the engine dials.

**How akit would use it.** `engine_settings` sets it from `limits.max_conns` in place of
`max_connections`, and the reference's note on `limits.max_conns` goes.

## A220 An accept filter on UDP and datagram listeners

**Done.** On akio's main (`9e21ada7`, akio's ADR-178) and pinned: `Listener::filter` on a UDP listener, `Engine::bind_udp_filtered`, `UdpBinding::set_filter`, and akio-quic's server asking it again before an Initial packet makes a connection. akit-listener gives a listener's UDP socket (HTTP/3's, or a `udp` service's) the policy its peers go by, set again by every reload on the socket it keeps; the HTTP/3 dispatcher's check (`Dispatcher::unfiltered`) and the `udp` sessions' check as they open are gone, and the tunnel hub's QUIC listeners refuse at the filter too.

**Status.** Requested by I85 (2026-11-26). akit refuses meanwhile where it can without it, a
stopgap recorded in [progress](progress.md): an HTTP/3 request is answered 403 by the listener's
dispatcher (I85), and a `udp` session is closed as it opens (I86).

**Problem.** fit's access lists drop the datagrams of a refused UDP peer before a session or a
QUIC connection is made for it (fib's `Binding.Filter`, fit's A73), as they close a refused TCP
connection at accept. akio's accept filter (A3, ADR-099) is for TCP listeners only: on a UDP or
datagram listener `Listener::filter` fails the build with `InvalidInput`, and
`Engine::set_listener_filter` answers `Unsupported`. So a refused client's QUIC handshake is done
in full (TLS work for every Initial packet it sends) before its request is answered 403, and each
datagram of a refused `udp` client opens a connection that akit closes at once.

**Evidence.** At akio `6bcd8ad6`, `Engine::swap_filter` refuses any `Kind` but `Tcp`
(`crates/akio/src/lib.rs`), and `Listener::filter`'s documentation says "TCP listeners on epoll,
kqueue and io_uring".

**Suggested API.** The same `Fn(&SocketAddr) -> bool`, asked for each datagram from a peer that
has no connection yet: `Listener::filter` on `Listener::udp` (and on the listeners `Engine::bind_udp`
binds, through a `filter` on that call or on `UdpBinding`), swapped by `Engine::set_listener_filter`
and `UdpBinding::set_filter`; a refused datagram dropped and counted in `LoopCounters::filtered`,
nothing remembered of the peer (so that a policy change applies at once and nothing grows with the
peers refused, as fit's decision 7 of I195 has it). akio-quic's server asks it on a connection's
first Initial packet.

**How akit would use it.** akit-listener installs the access policy the listener's TCP side has on
its UDP side too, the HTTP/3 dispatcher stops checking requests (`Dispatcher::unfiltered`), and
akit-l4's `udp` sessions are no longer checked as they open.

## A225 An external security review of akio's cryptography, TLS, HTTP parsers and WebAssembly

**Status.** Requested by I80b (2026-11-26) for the threat model's L15. Not an API: a review akio
commissions or takes part in. Nothing in akit waits on it; akit's own external review
([review scope](security/review-scope.md)) leaves akio's internals out.

**Problem.** Every connection akit serves goes through akio's own TLS 1.2/1.3 (akio-tls), X.509
verification (akio-x509), cryptography (akio-crypto), QUIC, and HTTP/1 and HTTP/2 parsers
(akio-http), and every `wasm` plugin through akio-wasm's interpreter. None is a widely reviewed
library: each was written in akio and has had akio's internal review only.

**Evidence.** akio's 1.0 checks: fuzzing of its protocol parsers, cargo-audit and cargo-deny, the
unsafe review, Miri, loom, sanitizers, and constant-time checks of its cryptography
(`docs/unsafe-review.md`, `docs/constant-time.md` in akio). The [threat model](security/threat-model.md#known-limitations)
lists the lack of an external review as L15; the review scope's [akio](security/review-scope.md#akio)
section recommends one before akit's 1.0 relies on them.

**Suggested API.** None. An external review, in this order: akio-crypto and akio-tls with
akio-x509 (the handshake, the record layer, certificate and CRL verification, the constant-time
paths), then akio-http's HTTP/1 and HTTP/2 parsers (request smuggling, HPACK, flow control), then
akio-wasm (validation, the sandbox's bounds). Its findings fixed in akio, as akio's security policy
says, and the report linked from akio's docs.

**How akit would use it.** It pins the akio commit with the fixes (rule 22), and the threat model's
L15 is closed, with the report linked from it and from the review scope.

## A230 Argon2id in akio-crypto

**Done.** On akio's main (`9e21ada7`, akio's ADR-179) and pinned: `kdf::argon2id` (RFC 9106, as Go's `argon2.IDKey`) and `hash::Blake2b`. akit-admin's `store` derives a `fitstore` file's key with it (fit's time 3, 64 MiB and 4 lanes for a new store; the header's own, within fit's bounds, for one read), on the control plane; stores fit wrote open in akit and akit's in fit (I93).

**Status.** Requested by I93 (2026-11-26). I93, the encrypted store, waits for it: fit's store
format cannot be read without it.

**Problem.** fit keeps its config sealed in a file of its own format, `fitstore` (fit's decision
027, `admin/store`), and akit is to read and write the same files, so that a store moves between
fit and akit (`admin.store`, `akit store export` and `import`). The key is derived from the store's
password with Argon2id (RFC 9106), with the time, memory and threads the file's header names (fit
writes time 3, 64 MiB and 4 threads, RFC 9106's second recommended setting, and reads up to time
16 and 1 GiB). akio-crypto has HKDF and PBKDF2 (`kdf`) and bcrypt, but no
Argon2 and no BLAKE2b, which Argon2 is built on.

**Evidence.** At akio `077a6710`: `crates/akio-crypto/src/kdf.rs` has `hkdf`, `hkdf_extract`,
`hkdf_expand` and `pbkdf2_hmac`; nothing in akio's tree names Argon2 or BLAKE2. fit's
`admin/store/store.go` calls `argon2.IDKey(password, salt, time, memoryKiB, threads, 32)` from
`golang.org/x/crypto/argon2`.

**Suggested API.** In `akio_crypto::kdf`: `argon2id(password: &[u8], salt: &[u8], time: u32,
memory_kib: u32, lanes: u8, out: &mut [u8]) -> Result<(), Error>` (Argon2 version 0x13, RFC 9106,
no secret or associated data, as Go's `IDKey`), checking RFC 9106's bounds (a salt of 8 bytes or
more, `memory_kib` at least 8 a lane, `time` at least 1) and erroring rather than panicking; its
memory allocated once, fallibly, so that a huge `memory_kib` is an error, not an abort; the lanes
computed one after another or on threads, the result the same. BLAKE2b (RFC 7693) with it, as
`hash::Blake2b` (variable output, as Argon2's H' needs). Tested against RFC 9106's test vector and
Go's `argon2.IDKey` outputs for fit's parameters, and constant-time where the password is read.

**How akit would use it.** akit-admin's `store` derives the store's key once at start, on the
control thread (a fair share of a second and 64 MiB, never on an I/O thread), after checking the
header's settings against fit's bounds; an import sent to the Admin API is bounded to twice fit's
cost before anything is derived, as fit's `Unseal`.

## A231 XChaCha20-Poly1305 in akio-crypto

**Done.** On akio's main (`9e21ada7`, akio's ADR-179) and pinned: `chacha::XChaCha20Poly1305` and `chacha::hchacha20`, as Go's `chacha20poly1305.NewX`. akit-admin's `store` seals each write with a new random nonce, the 59-byte header as additional data (I93).

**Status.** Requested by I93 (2026-11-26), with A230. I93 waits for it.

**Problem.** `fitstore` seals its payload with XChaCha20-Poly1305 (a 24-byte nonce, random for
each write, the 59-byte header as additional data). akio-crypto has ChaCha20-Poly1305 with a
12-byte nonce (RFC 8439, `chacha::ChaCha20Poly1305`) but not the extended-nonce construction
(HChaCha20 then ChaCha20-Poly1305), so a fit store can neither be opened nor written.

**Evidence.** At akio `077a6710`: `crates/akio-crypto/src/chacha/mod.rs` has `NONCE_LEN = 12`,
`ChaCha20`, `Poly1305` and `ChaCha20Poly1305`, and no HChaCha20 or XChaCha20. fit's store calls
`chacha20poly1305.NewX(key)` (`golang.org/x/crypto/chacha20poly1305`).

**Suggested API.** `chacha::hchacha20(key: &[u8; 32], nonce: &[u8; 16]) -> [u8; 32]` and
`chacha::XChaCha20Poly1305` with `new(&[u8; 32])`, `XNONCE_LEN = 24` and the same `encrypt`,
`decrypt` and in-place methods as `ChaCha20Poly1305` (the subkey from HChaCha20 over the nonce's
first 16 bytes, the inner nonce four zero bytes and its last 8), as draft-irtf-cfrg-xchacha and
Go's `NewX`. Tested against the draft's vectors, Wycheproof's `xchacha20_poly1305_test.json` and
Go's output.

**How akit would use it.** akit-admin's `store` opens a store with the key A230 derives, the header
as additional data, and seals each write with a new random nonce (`akio_crypto::rand`), as fit's
`Store.seal`.

## A232 An HTTP connection's idle timeout and close from its handler, and a table of the connections accepted

**Done.** On akio's main (`9e21ada7`, akio's ADR-180) and pinned: `ConnHandle` (`close`, `close_at`, `close_after`, `cancel_close`, `set_idle_timeout`), `Listener::on_accept` and `on_close`, and akio-http's `Context::connection()`. The console keeps fit's rules on them: a connection without a valid session closed 5 s after accept (a request served then answered with `Connection: close` within as long again), one with a session kept 2 minutes between requests, and past `max_conns` the oldest unauthenticated connection of the client holding the most closed for a new one (`console::slots`, fit's `login.Slots`). The stopgap, an answer without a valid session closing its connection, is gone. The Admin API keeps `max_conns` at accept: fit's Admin API rules for connections without a token are not ported yet.

**Status.** Requested by I90 (2026-11-26). The console works without it; meanwhile its connections
are bounded as the Admin API's are (a stopgap recorded in [progress](progress.md)).

**Problem.** fit's console (and, since fit's I161, its Admin API) bounds the connections of clients
that have not logged in: one is closed 5 s after it was accepted, whatever it sends, until a
request with a valid session comes; from then on it is kept for 2 minutes between requests, and
put back on the 5 s rule when its session ends. When `max_conns` are open, a new connection closes
the oldest unauthenticated one of the client holding the most (loopback counting as one client),
keeping a quarter of the slots for connections with a login, rather than being refused. So clients
without a login cannot hold the console's few connections, and an attacker reconnecting cannot
keep an operator out. akio-http's server has the timeouts of `HttpConfig` (head, body, keep-alive)
for every connection alike, and a handler has no handle on its connection: it can neither change
its idle timeout nor close it later, and nothing tells an application of each connection accepted
so that it can choose one to close.

**Evidence.** At akio `077a6710`: akio-core's `Conn` has `set_idle_timeout` and `close`, but
akio-http's `Context` exposes neither (no `conn()` accessor); `Listener::filter` (A3) sees the peer
address only, and answers yes or no.

**Suggested API.** `Context::connection(&self) -> ConnHandle`, a cheap clonable handle with
`set_idle_timeout(Option<Duration>)`, `close()` and `id()`, usable from any thread; and a
`Listener::on_accept(Fn(ConnHandle, SocketAddr))` hook (or the filter given the handle), so that an
application keeps its own table of the connections open and closes the one it chooses. A deadline
set at accept (`ConnHandle::close_at(Instant)`), cancelled by the handler, would cover the 5 s rule
without a timer of the application's own.

**How akit would use it.** The console (and the Admin API, as fit's I161 has them) keeps a table of
its connections: each closed 5 s after accept unless a request with a valid session cancels it; a
valid session sets 2 minutes idle; past `max_conns`, the oldest unauthenticated connection of the
client holding the most is closed to make room.

## A233 A terminal's echo turned off, to read a password

**Done.** On akio's main (`9e21ada7`) and pinned: akio-sys's `terminal::no_echo`, `terminal::raw` and `echo_enabled`, with a `TerminalGuard` that puts the modes back. `akit hash-password` reads a password typed at a terminal in raw mode, a `*` shown for each character, as fit's `readMasked` (Backspace, Ctrl-U, escape sequences dropped, Ctrl-C and Ctrl-D interrupting); the note about the shown password is gone. From a pipe it still reads the first line.

**Status.** Requested by I89 (2026-11-26). `akit hash-password` works without it: from a terminal,
the password is seen as it is typed, and the prompt says so and how to hide it (a stopgap recorded
in [progress](progress.md)).

**Problem.** `fit hash-password` reads a password from a terminal in raw mode, without echo,
showing a `*` for each character (golang.org/x/term's `MakeRaw`). That takes `tcgetattr` and
`tcsetattr`, which need `unsafe` (libc) that akit forbids; akio-sys has no terminal calls.

**Evidence.** At akio `077a6710`, `crates/akio-sys/src/process.rs` has `allowed_cpus`,
`pin_current_thread`, `signal_pipe` and `kill`; nothing in akio-sys names `termios`.

**Suggested API.** In akio-sys: `terminal::raw(fd: BorrowedFd<'_>) -> io::Result<RawGuard>` (or
`no_echo`), which puts the terminal in raw mode (or turns `ECHO` and `ICANON` off) and restores it
when the guard is dropped, an error for a descriptor that is not a terminal.

**How akit would use it.** `akit hash-password` reads the password from a terminal with echo off,
as fit does, a `*` written for each character, and the note about the shown password goes.

## A234 A connection's own bound of the bytes queued for it

**Done.** On akio's main (`6151f5c5`, akio's ADR-181) and pinned: `Conn::set_max_queued_bytes` (and `ConnInner`'s, and akio-ws's `WsSender::set_max_queued_bytes`). Every tunnel link's wire is bounded at fit's `LinkQueue` (`akit_tunnel::msg::LINK_QUEUE`, 64 windows of 4 MiB and 4 MiB for control messages) as the link is made (`Wire::set_max_queued`); the 8 MiB high water (`akit_tunnel::link::HIGH_WATER`, `Link::congested`, `Link::when_drained`) and the holds it put on a tunnel's end and on the hub's other link are gone, so tunnels are paced by their windows alone, as fit's. A QUIC link's stream buffer and its wire's overflow queue hold the same `LinkQueue` together.

**Status.** Requested by I101 (2026-11-27). Nothing waits for it: the tunnel's links pace what
feeds them by their own high water meanwhile (a stopgap recorded in [progress](progress.md)).

**Problem.** fit raises the send-queue bound of each tunnel link to what its tunnels may have in
flight: 64 tunnels' 4 MiB windows and room for control messages (`proto.LinkQueue`, about
260 MiB, fib's `SetQueueLimit`), so that a link never fails a send its tunnels' windows allow.
akio has one bound for every connection of an engine, `Config::max_queued_bytes`, which akit sets
from `limits.max_write_queue_bytes` (32 MiB by default): the gateway's clients and the tunnel's
links share it, and a link past it fails its send and closes. The LLM mesh's links (I104) carry
streams with 256 KiB windows each, up to `limits.max_inflight` of them, and meet the same bound.

**Evidence.** At akio `077a6710`, `akio-core/src/config.rs` has `max_queued_bytes` (64 MiB by
default) and `ConnInner::send` fails with `Overloaded` past it; neither `Conn` nor `ConnInner` has
a setter of a connection's own.

**Suggested API.** `Conn::set_max_queued_bytes(&self, n: Option<usize>)` (and on `ConnInner`):
this connection's bound in place of the engine's, `None` back to the engine's; effective for the
next send.

**How akit would use it.** A tunnel link's connection gets fit's `LinkQueue` and drops its 8 MiB
high water (`akit_tunnel::link::HIGH_WATER`), so that its tunnels are paced by their windows alone,
as fit's are; an LLM link gets its streams' windows times `max_inflight`.

## A235 A pinned server certificate in akio-tls's client

**Done.** On akio's main (`6151f5c5`, akio's ADR-182) and pinned: `ClientConfigBuilder::pin_sha256`, which akio-quic's clients built from the configuration check too. The tunnel's TLS and QUIC links give their pin to the client configuration and start at once: a certificate that does not match fails the handshake, and nothing the link sent meanwhile leaves. `crate::tls::when_handshaken` and its poll on akio's timer thread, `Link::deferred` and `Link::start`, and the QUIC dialer's own check of the leaf are gone.

**Status.** Requested by I102 (2026-11-27). Nothing waits for it: the tunnel's TLS links check the
pin once the handshake is done and before they send a byte (a stopgap recorded in
[progress](progress.md)).

**Problem.** fit's tunnel and LLM mesh check a hub's (or a listener's) self-signed certificate by
its SHA-256, in place of the chain and the name (`certpin.Pinned`: Go's `VerifyPeerCertificate`
with `InsecureSkipVerify`), in the handshake: a certificate that does not match fails it. akio-tls's
`ClientConfigBuilder` verifies the chain and the name against roots, or nothing
(`insecure_skip_verify`, which still checks the server's CertificateVerify with the leaf's key);
it has no hook for a certificate of the caller's choosing, and no callback when the handshake is
done. akit therefore polls `tls_info` on akio's timer thread until the handshake is done, compares
the leaf with the pin, and only then starts the link: sound (the signature proves the key), but a
poll, and the handshake completes where it should have failed.

**Evidence.** At akio `077a6710`, `akio-tls/src/config.rs`'s `ClientConfigBuilder` has `roots`,
`client_cert`, `insecure_skip_verify` and the rest, and `proto/client.rs`'s `peer_certificate`
either `verify_chain`s or takes `leaf_key` under `insecure`.

**Suggested API.** `ClientConfigBuilder::pin_sha256(pins: Vec<[u8; 32]>)`: the server's leaf must
hash to one of them (compared in constant time); chain, name and dates are not checked; the
handshake fails with `bad_certificate` otherwise. The same for akio-quic's client, whose TLS is
akio-tls's.

**How akit would use it.** The tunnel's TLS links (and QUIC's, I102b) and the LLM mesh's `wss://`
links (I104) give their pin to the client configuration and start the link at once, as with
`ca_file`; `crate::tls::when_handshaken` goes.

## A236 A WebSocket's reads held, its drain heard, and its peer's address

**Done.** On akio's main (`6151f5c5`, akio's ADR-183) and pinned: `WsSender::hold_reads`, `on_drain`, `peer_addr`, `set_relayed` and `set_max_queued_bytes`, on akio-ws's client and akio-http's upgrade alike. The tunnel's `ws://` and `wss://` links (I102c) run on them: `akit_tunnel::ws::WsWire` maps a link's `hold_reads`, `on_drain`, `queued`, `peer` and `set_max_queued` onto them, as `ConnWire` does onto a TCP connection's, and marks the WebSocket's connection relayed, as fit's `prepareLink` does.

**Status.** Requested by I102c (2026-11-27). I102c (the tunnel's `ws://` and `wss://` links) and
I104 (the LLM mesh, whose links are WebSockets) wait for it.

**Problem.** A tunnel link, and an LLM mesh link, carries many streams whose bytes come from
other connections: when the link's queue passes its high water, what feeds it is held, and it
resumes once the queue drains; when what the link delivers cannot be written on (a slow service,
a congested link on the hub's other side), the link's own reads are held, so that the peer's
window stops it (rules 8 and 9; fit's `transport.Conn` has `SetReadPaused` and an `OnDrain` on its
WebSocket links as on its TCP ones). akio-ws's `WsSender` has `queued_bytes` and nothing else of
the kind: no way to stop reading a WebSocket, no callback when its queue drains, no peer address
(which the hub's bounds per address and its access policy need). A link on it could only drop or
poll.

**Evidence.** At akio `077a6710` (and akio's main at `88618b83`), `akio-ws/src/session.rs`'s
`WsSender` has `send_*`, `ping`, `pong`, `close`, `subprotocol`, `is_open` and `queued_bytes`;
`WsHandler` has `on_open`, `on_message`, `on_message_ref`, `on_pong` and `on_close`; nothing in
akio-ws or in akio-http's upgrade glue (`Context::upgrade_websocket`) calls `hold_reads` or
`on_drain` on the connection underneath.

**Suggested API.** On `WsSender`: `hold_reads(&self, hold: bool)` (the connection's, for a
WebSocket over TCP or TLS; the stream's for one over HTTP/2 or HTTP/3), `on_drain(&self, f: impl
FnOnce() + Send + 'static)` (once its queue is empty, as `Conn::on_drain`), `peer_addr(&self) ->
Option<SocketAddr>`, and `set_relayed(&self, bool)` (what a relay that writes as it reads needs,
as `Conn::set_relayed`). Or `WsHandler::on_drain(&mut self, ws: &WsSender)`, called when a queue
that passed a watermark drains.

**How akit would use it.** A WebSocket link's `Wire` (`akit_tunnel::link::Wire`) maps `hold_reads`,
`on_drain`, `queued` and `peer` onto them, as `ConnWire` does onto a TCP connection's; the LLM
mesh's sessions hold a link's reads while a stream's reader is slow past its window.

## A237 `Engine::wait` safe in a thread-local destructor

**Done.** On akio's main (`6151f5c5`) and pinned: akio-pool's thread-locals read with `try_with`, a destroyed one taken as "not a pool thread". `akit_observe::stop_client_engine` shuts the engine down and waits for it on the calling thread again (not on an I/O thread); the thread it spawned for the wait is gone. akit-listener's `acme_shared` passes on macOS.

**Status.** Requested on 2026-11-27, when akit moved to akio `9e21ada7`. Meanwhile akit's client
engines (OTLP's, the providers', the shared ACME storage's) are waited for on a thread spawned for
it (`akit_observe::stop_client_engine`), a stopgap recorded in [progress](progress.md).

**Problem.** Since `9e21ada7`, `Engine::wait` first asks `Pool::is_current` whether it runs on one
of the engine's pool threads, which reads akio-pool's thread-local `CURRENT` with `with`. A thread's
thread-locals are destroyed in an unspecified order when it exits; an engine dropped by another
thread-local's destructor (akit's: a thread's cached graph, whose last reference drops a client
engine's owner) finds `CURRENT` destroyed already, `with` panics, and a panic in a thread-local
destructor aborts the process. akit's `acme_shared` test aborted so on macOS
(`a_challenge_nobody_has_is_404`); the same can happen to an I/O thread that exits holding the
last reference to a graph.

**Evidence.** At akio `9e21ada7`: `crates/akio/src/lib.rs`, `Engine::wait` calls
`self.pool.is_current()`; `crates/akio-pool/src/lib.rs`, `Pool::is_current` is
`CURRENT.with(...)`, `CURRENT` a `RefCell<Option<Weak<Pool>>>` (it has a destructor, so it is
destroyed with the thread). `timer::on_loop` reads a `Cell<u64>`, which has none, and is safe.

**Suggested API.** No new API: `Pool::is_current` (and `on_pool_thread`, and any thread-local
read on the way to waiting or joining) with `try_with`, a destroyed thread-local taken as "not a
pool thread".

**How akit would use it.** `stop_client_engine` goes back to `shutdown` and `wait` on the calling
thread.

## A238 A WebSocket ended at once, what is queued dropped

**Done.** On akio's main (`8a9601b9`, akio's ADR-184) and pinned: `WsSender::abort(&self)` ends the WebSocket at once, without a close frame: what is queued is dropped, later sends fail `Closed`, no message reaches the handler after it, and both ends' `on_close` get `Some(1006)` once; over TCP or TLS the connection closes (no `close_notify`), over HTTP/2 the stream is reset with `CANCEL`, over HTTP/3 with `RESET_STREAM` and `STOP_SENDING` (`H3_REQUEST_CANCELLED`); callable from any thread and from the handler's callbacks, after `close` too. akit's stopgaps went: `akit_tunnel::ws::WsWire::close` (a failed link, a kick, a revoked token, a role stopped) and a refused link's WebSocket abort, as fit's `Abort`; the hub's console socket and the proxy's web proxies abort the WebSockets they refuse or that fail; the LLM mesh dialer's force close (a dead or too slow peer) aborts, and its 1 s close timeout is gone.

**Status.** Requested by I102c (2026-11-27). Nothing waits for it: a WebSocket link closes with
`WsSender::close` meanwhile (a stopgap recorded in [progress](progress.md)).

**Problem.** fit's links close at once, dropping what is queued, when the link fails (a frame that
does not open, a breach of the protocol), when a hub kicks a peer or its token is revoked, and when
a role stops: fit's WebSocket transport calls `Abort`. akio-ws's `WsSender` has `close(code,
reason)` alone, which queues a close frame behind what is queued (a tunnel link may have fit's
`LinkQueue`, about 260 MiB) and closes once it is written (a server) or once the peer answers it or
`close_timeout` passes (a client). A kicked or failed WebSocket link so goes on sending what it
had queued, and a client's waits for its peer, where a TCP, TLS or QUIC link closes at once
(`Conn::close`, `Connection::close`).

**Evidence.** At akio `6151f5c5`, `akio-ws/src/session.rs`'s `WsSender` has `send_*`, `ping`,
`pong`, `close`, `subprotocol`, `is_open`, `queued_bytes`, `hold_reads`, `on_drain`, `peer_addr`,
`set_relayed` and `set_max_queued_bytes`; `close_with` sends a close frame and closes after it.

**Suggested API.** `WsSender::abort(&self)`: closes the transport now (the TCP or TLS connection's
`close`; over HTTP/2 or HTTP/3, the stream reset), what is queued dropped, no close frame;
`WsHandler::on_close` is told with 1006.

**How akit would use it.** `akit_tunnel::ws::WsWire::close` (a failed link, a kick, a revoked token,
a role stopped) aborts, as `ConnWire::close` closes a TCP link; the LLM mesh's links (I104) the
same.

## A247 The process's effective user

**Done.** On akio's main (`a8171fda`) and pinned: `akio_sys::process::euid() -> u32`
(`geteuid(2)`). `akit_llm::exec::is_root` calls it whenever it is asked; `init_euid` and its
reads of `/proc/self` or `id -u` are gone.

**Status.** Requested by I108 (2026-11-27). Nothing waits for it: an LLM node reads it once when
it starts, on the control thread, from `/proc/self`'s owner on Linux and `id -u` elsewhere (a
stopgap recorded in [progress](progress.md)).

**Problem.** fit's node prefixes its root commands and GPU settings with `sudo -n` only when it
does not run as root (`os.Geteuid() != 0`), and its GPU status says whether it is root. Rust's
standard library has no `geteuid`, and akit has no `unsafe`.

**Evidence.** At akio `6151f5c5`, `akio-sys`'s `process` module has `allowed_cpus`,
`pin_current_thread`, `signal_pipe` and `kill`; nothing gives the process's user.

**Suggested API.** `akio_sys::process::euid() -> u32` (`geteuid(2)`, which cannot fail).

**How akit would use it.** `akit_llm::exec::is_root` calls it, and `init_euid` goes.

## A248 A signal to a process group

**Done.** On akio's main (`a8171fda`) and pinned: `akio_sys::process::kill_group(pgid, sig)`
(`killpg(2)`; 0, 1 and pgids past `i32::MAX` refused with `EINVAL`, `ESRCH` for an empty group)
and `process::SIGKILL`. `akit_llm::exec::Killer::kill` sends the group SIGKILL at once, from any
thread, under the lock that keeps the leader unreaped; the `kill(1)` run on a pool is gone, and
`Killer` and `Jobs` take no pool. The commands `exec::output` runs alone are started in a group of
their own too, so a timeout kills their children.

**Status.** Requested by I108 (2026-11-27). Nothing waits for it: a job's leader gets SIGKILL
from `akio_sys::process::kill`, the rest of its group from `kill -KILL -- -<pgid>` run on the
node's pool (a stopgap recorded in [progress](progress.md)).

**Problem.** fit's node runs each job in a process group of its own and kills the whole group when
the job is canceled or runs past its timeout (`syscall.Kill(-pid, SIGKILL)`), so that a script's
children go too. `akio_sys::process::kill` refuses a pid that is not positive, by design.

**Evidence.** At akio `6151f5c5`, `akio-sys/src/process.rs`'s `kill(pid, sig)` documents that
"process groups and every process are refused".

**Suggested API.** `akio_sys::process::kill_group(pgid: u32, sig: i32) -> io::Result<()>`
(`killpg(2)`), refusing 0 and 1.

**How akit would use it.** `akit_llm::exec::Killer::kill` signals the group at once, from any
thread, instead of running `kill(1)` on a pool.

## A249 A file system's size and space free

**Done.** On akio's main (`a8171fda`) and pinned: `akio_sys::fs::space(path) -> io::Result<Space>`
(`total`, `free`, `available`, bytes). `akit_llm::hw`'s disks on Linux read each file system of
`/proc/mounts` of fit's kinds with it, on the node's pool (it may block on a network mount), as
fit's `syscall.Statfs`: the space available to a user that is not root; `df` is no longer run
there. macOS keeps `df -k -P -l`, as fit's.

**Status.** Requested by I108 (2026-11-27). Nothing waits for it: an LLM node reads its disks from
`df -k -P` on its pool (a stopgap recorded in [progress](progress.md)).

**Problem.** fit's node reports each mounted file system's size and space free, read with
`syscall.Statfs` on Linux. Rust's standard library has no `statvfs`.

**Evidence.** At akio `6151f5c5`, `akio-sys` binds no `statfs` or `statvfs`.

**Suggested API.** `akio_sys::fs::space(path: &Path) -> io::Result<Space>` with `total`,
`free` and `available` in bytes (`statvfs(3)`).

**How akit would use it.** `akit_llm::hw`'s disks on Linux: `/proc/mounts`' file systems, each
read with it, as fit reads them, without running `df`.

## A245 The QUIC server's handshake bounds

**Done.** On akio's main (`8a9601b9`, akio's ADR-185) and pinned: `ServerConfig::handshake_timeout(Option<Duration>)` (default 10 s; a server connection whose handshake is not done in time is dropped without a word, its handler's `on_close` told `ConnectionError::TimedOut`) and `ServerConfig::max_handshakes(usize)` (default 8192; from half of them in progress, a client without a valid Retry or NEW_TOKEN token is answered with a Retry; at the limit, a new client's Initial is dropped unanswered, counted in `QuicServer::refused_connections`). fit lets an invalid Retry token through to an `INVALID_TOKEN` answer; akio drops it. The hub's `quic` listeners (I102d) take `tunnel.hub.quic`'s `handshake_timeout` and `max_handshakes` (fit's 10 s and 64 when left out).

**Status.** Requested by I102d (2026-11-27).

**Problem.** fit's QUIC listener bounds what handshakes may hold: `handshake_timeout` (10 s) drops a
connection whose handshake is not done in time; `max_handshakes` (64) bounds those under way: past
half, a client whose Initial has no valid token gets a Retry, so that Initials from forged addresses
hold at most half; past all, a new client's Initial is dropped (it sends it again and gets in once a
handshake is over). akio-quic's server had `max_connections` and a Retry from a fixed 4096
handshakes in progress, and nothing bounding a handshake's time but the idle timeout, which every
client packet renews.

**Evidence.** At akio `6151f5c5`: `akio-quic/src/types.rs`'s `ServerConfig` had no handshake
bound; `proto/endpoint.rs` answered with a Retry from `RETRY_THRESHOLD` (4096). fit:
`tunnel/internal/transport/quic/listen.go`'s `admit` (dropped at `max`, a Retry from `max/2`) and
`conn.go`'s `fq.Config.HandshakeTimeout`.

**Suggested API.** On `akio_quic::ServerConfig`: `handshake_timeout(Option<Duration>)` and
`max_handshakes(usize)`.

**How akit would use it.** `akit_tunnel::quic`'s hub listener builds its `ServerConfig` with
`.handshake_timeout(Some(TunnelHubQuic::handshake(q)))` and
`.max_handshakes(TunnelHubQuic::handshakes(q))` (fit's 10 s and 64 when left out). The time a
connection may go without its link's stream after the handshake stays the hub's own timer, as fit's
`streamWait`.

## A256 akio-turn's static settings: a relay port range, a shared nonce key, a stream's largest message

**Done.** On akio's main and pinned (since `d8d51f61`, akio's ADR-186): `TurnConfig::relay_ports`, `nonce_secret`, `max_stream_message`.

**Status.** Requested by I111 (2026-11-27). The TURN role refuses `port_min` and `port_max` at run
time (`akit validate` takes them), and says when its server starts that `nonce_secret` and
`max_stream_message` are not applied (decision 039).

**Problem.** fit's `webrtc.turn` binds its relayed sockets in `port_min`..`port_max`, the range a
firewall in front of the server opens (fit-examples' `webrtc-turn` sets 49152-49999); makes its
nonces with `nonce_secret`, so that servers behind one load balancer take each other's; and bounds
what a TCP or TLS client's connection buffers of a message not yet whole with
`max_stream_message` (8 KiB by default, at least 2048). akio-turn's `TurnServer` binds each relay
on `relay_ip` port 0, keys its nonces with random bytes of its own, and frames stream messages up
to 64 KiB.

**Evidence.** At akio `8a9601b9` (and `a8171fda`): `akio-turn/src/core.rs`'s `TurnConfig` has no
port range, `Shared::new` fills `nonce_key` from `fill_random`, and `framing.rs`'s `MAX_FRAME` is
`HEADER_LEN + 0xfffc`; the environment's `open_relay` is asked for `relay_ip:0`. fit:
`webrtc/turn/turn.go` (`turnserver.Config.PortMin`, `PortMax`, `NonceSecret`,
`MaxStreamMessage`), fib's A55 and A57.

**Suggested API.** On `TurnConfig`: `relay_ports(min: u16, max: u16)` (a port in use skipped, an
exhausted range answered 508), `nonce_secret(&[u8])` (the nonce key derived from it),
`max_stream_message(usize)` (a stream whose next message claims more is closed).

**How akit would use it.** `akit_webrtc::turn` sets the three from the member, and drops its
refusal and warnings (I112).

## A257 akio-turn's hooks for an embedding server: relayed bytes by username, events, its relayed addresses

**Done.** On akio's main and pinned (since `d8d51f61`, akio's ADR-186 update 1): `TurnConfig::on_relay`, `on_event`, `TurnServer::holds_relay`, `TurnStats`' new counters.

**Status.** Requested by I111 (2026-11-27). `user_bandwidth` is not enforced (a warning when the
server starts); the server's own addresses (`relay_ip`, `public_ip`) are reached on the ports
`allowed_peers` names with them alone, so two clients of one server do not relay to each other's
relayed addresses; `/metrics` counts datagrams, not bytes, and no events (decision 039).

**Problem.** fit's TURN role limits each username's relayed bytes a second each way
(`user_bandwidth`, a token bucket per username and direction, the datagrams past it dropped) through
fib's `OnRelay(user, toPeer, n) bool`; counts and logs fib's `OnEvent` (allocated, refreshed, freed
with a reason, refused with a code, credentials failed); and lets peers on its own addresses be
reached on its relayed ports alone through fib's `HoldsRelay`, so that two clients relay to each
other but nobody reaches fit's other listeners through the relay. Its gauges count permissions,
channels and allocations pending. akio-turn's `PeerFilter` sees the peer alone; its `TurnStats`
counts allocations and datagrams.

**Evidence.** At akio `8a9601b9`: `akio-turn/src/core.rs`'s `TurnConfig::peer_allowed` is
`Fn(SocketAddr) -> bool`; `TurnStats` holds `allocations`, `to_peer`, `to_client`,
`auth_failures`, `tcp_connections`, `icmp`, `ttl_expired`; no callback runs per relayed datagram or
per event. fit: `webrtc/turn/turn.go`'s `onRelay`, `onEvent`, `peerAllowed`, `WriteMetrics`;
`limiter.go`.

**Suggested API.** On `TurnConfig`: `on_relay(Arc<dyn Fn(&str, bool, usize) -> bool + Send +
Sync>)` (username, toward the peer, bytes; false drops it), `on_event(Arc<dyn Fn(&TurnEvent) + Send
+ Sync>)` (`Allocated`, `Refreshed`, `Freed { reason }`, `Refused { method, code }`,
`AuthFailed`, with the username, client and relayed address), called after the answer is sent and
never under a lock the I/O path needs; on `TurnServer`: `holds_relay(SocketAddr) -> bool`;
`TurnStats` with `permissions`, `channels`, `pending` and `dropped`.

**How akit would use it.** `akit_webrtc::turn` keeps fit's limiter (a bucket per username and
direction, 65,536 at most), counts the events into `akit_turn_*` and the `webrtc` logger at debug,
and reaches its own addresses on its relayed ports (I112).

## A258 akio-turn's settings changed on a running server, and a drain

**Done.** On akio's main and pinned (since `d8d51f61`, akio's ADR-186 update 2): `TurnServer::set_limits`, `set_auth`, `drain`.

**Status.** Requested by I111 (2026-11-27). The users, secret and peer rules change in place
already (akit's `AuthFn` and peer filter read them); a change of the quotas, caps or lifetimes
starts a new server, ending the old one's allocations when the two share an address; the server a
reload replaced keeps serving, new clients included, for `-shutdown-timeout`, then closes
(decision 039).

**Problem.** fit's reload changes a running server's quotas (`max_allocations`,
`max_allocations_per_user`), caps (`max_permissions`, `max_channels`) and lifetimes in place
(fib's `SetLimits`, `SetAuth`), the allocations kept; and when its addresses change it starts a new
server and drains the old one (fib's `Shutdown(ctx)`: new allocations refused with 508, those held
refreshed until they end or the drain's time runs out). akio-turn's `TurnConfig` is fixed when the
server is made, and `close_all` ends every allocation at once.

**Evidence.** At akio `8a9601b9`: `TurnServer::new(cfg)` keeps `cfg` in `Shared`, read without a
lock; nothing sets it again; `close_all` is the only way down. fit: `webrtc/turn/turn.go`'s `Apply`
(`SetLimits`, `SetAuth`, `drainRunning`), fib's A58.

**Suggested API.** On `TurnServer`: `set_limits(TurnLimits)` (the quotas, caps and lifetimes,
applied to the next request), `set_auth(AuthFn)`, and `drain()` (new allocations refused with 508,
refreshes, permissions and channels of those held served) with `allocations()` to watch them run
out.

**How akit would use it.** `akit_webrtc::turn` takes the quotas, caps and lifetimes out of its
static key and sets them in place; a server a reload replaced drains, refusing new allocations
(I112).

## A259 A peer's own configuration on an `RtcServer`

**Done.** On akio's main and pinned (since `d8d51f61`, akio's ADR-187): `RtcServer::accept_offer_with`.

**Status.** Requested by I111 (2026-11-27), for I113. Nothing runs on it yet; I113's planned
stopgap: the server's configuration closure reads the codecs of the session being answered, set on
the calling thread beside `accept_offer` (which builds the peer's configuration synchronously).

**Problem.** fit's media server (`webrtc.media`) answers a publisher with one codec of each kind,
the first of its offer's codecs that `codecs` lists, and answers that stream's viewers with the same
codecs, so that what the publisher sends is what each viewer decodes. Every PeerConnection shares
one UDP port (fib's `webrtc.Mux`), each with its own `Configuration`. akio-webrtc's `RtcServer`
builds every peer's `RtcConfig` with one closure set before the server is cloned, called with no
argument.

**Evidence.** At akio `8a9601b9`: `akio-webrtc/src/lib.rs`'s `RtcServer::config(self, f: impl Fn()
-> RtcConfig)` and `Hub::accept_offer(&self, offer, handler)`, which calls `new_rtc()`. fit:
`webrtc/media/session.go`'s `pickCodecs` and `peerConfig`.

**Suggested API.** `RtcServer::accept_offer_with<H: PeerHandler>(&self, config: RtcConfig, offer:
&str, handler: H) -> Result<(Peer, String), Error>`, the crypto provider and ICE mode set over
`config` as for the closure's.

**How akit would use it.** `akit_webrtc::media` answers a publisher with a configuration of its
picked codecs, and its viewers with the same, without the closure's thread-local.

## A260 RTP forwarding in akio-webrtc for an SFU

**Done.** On akio's main and pinned (since `d8d51f61`, akio's ADR-187): `Peer::write_rtp`.

**Status.** Requested by I111 (2026-11-27), for I113. I113's planned stopgap: each publisher's
frames (`on_media_data`) written to each viewer with `write_media`, depacketized once and packetized
again per viewer.

**Problem.** fit's media server forwards a publisher's RTP packets to each viewer as they arrive
(fib's `TrackLocal.WriteRTP`: the SSRC, payload type and sequence numbers of the viewer's sending
track, the header extensions kept), on the I/O thread that received them, so that a viewer gets
each packet one hop later, without waiting for a whole frame, and the server's work per viewer is a
header rewrite and an SRTP seal. akio-webrtc delivers received RTP (`set_rtp_mode`, `on_rtp`) but
sends media only as samples (`write_media`, `write_simulcast`), packetized by the peer.

**Evidence.** At akio `8a9601b9`: `akio-webrtc/src/lib.rs`'s `Peer` has `write_media`,
`write_simulcast`, `request_keyframe`, `payload_type`; no method takes an RTP packet. fit:
`webrtc/media/session.go`'s `forward` and `onTrack`.

**Suggested API.** `Peer::write_rtp(&self, mid: Mid, packet: &RtpPacket) -> Result<(), Error>`:
the packet sent on `mid` with the peer's SSRC and the payload type it negotiated for the packet's
codec, sequence numbers and timestamps continuous across the sources written to it (a publisher
replaced), header extensions mapped by URI; NACK and RTX answered from what it sent.

**How akit would use it.** `akit_webrtc::media` runs publishers in RTP mode and writes each packet
to the viewers of its stream, in place of the frames.

## A261 A receiver's bitrate cap told as REMB, and the ICE consent timeout per peer

**Done.** On akio's main and pinned (since `d8d51f61`, akio's ADR-187): `Peer::set_receive_bitrate_cap`, `RtcConfig::set_ice_timeouts`.

**Status.** Requested by I111 (2026-11-27), for I113. I113's planned stopgap: `max_bitrate` enforced
by dropping a publisher's media past two seconds' burst, without telling the publisher; peers left
on akio's consent timeout whatever `ice_timeout` says.

**Problem.** fit tells each publisher `max_bitrate` once a second as REMB
(draft-alvestrand-rmcat-remb), which browsers keep under, and drops what passes it with two seconds
of burst; and ends a session whose peer is silent for `ice_timeout` (30 s by default), through fib's
`ICEDisconnectedTimeout` and `ICEFailedTimeout`. akio-webrtc reads REMB from remote receivers but
sends none, and its consent freshness timeout is fixed.

**Evidence.** At akio `8a9601b9`: `akio-webrtc/src/rtc.rs` parses `Rtcp::Remb` (line 3667) and
advertises `goog-remb`; `Peer::request_max_bitrate` sends TMMBR, which browsers do not honour; no
setting of `RtcConfig` names the consent or disconnected timeout. fit: `webrtc/media/session.go`'s
`sendREMB` and `peerConfig`.

**Suggested API.** `Peer::set_receive_bitrate_cap(&self, bps: Option<u64>)` (REMB with the cap, or
the estimate when lower, once a second for every remote SSRC), and
`RtcConfig::set_ice_timeouts(disconnected: Duration, failed: Duration)`.

**How akit would use it.** `akit_webrtc::media` caps each publisher at `max_bitrate` and sets
`ice_timeout` on each session's configuration (with A259).

## A263 akio-media's RTMP server: a message bound, fit's refusals, a deadline to publish or play

**Status.** Requested by I120 (2026-11-27). Nothing waits for it: `streaming.rtmp.max_message_bytes`
is refused as not supported yet, and RTMP's refusals are akio-media's (a stopgap recorded in
[progress](progress.md)).

**Problem.** fit's RTMP listener bounds the largest message a client may send
(`max_message_bytes`, 8 MiB), answers a publish its policy refuses with
`NetStream.Publish.Unauthorized` and a play with `NetStream.Play.Failed`, saying so when the
refusal is a password still being checked (the client's next attempt passes), and gives a client
`handshake_timeout` from connecting to publishing or playing. akio-media's server reads messages
up to a fixed 16 MiB, answers a publish its hub's hook refuses with `NetStream.Publish.Denied`,
sends a play's `NetStream.Play.Reset` and `NetStream.Play.Start` before it subscribes and
`NetStream.Play.StreamNotFound` when the hub refuses, and its `handshake_timeout` ends with the
handshake (C2), after which only `idle_timeout` bounds a client that never publishes or plays.

**Evidence.** At akio `8a9601b9`, `akio-media/src/rtmp/mod.rs`: `RtmpConfig` has no message bound;
`RtmpServer::on_open` makes its `ChunkReader::new()` (16 MiB messages, 32 MiB partial); `publish`
maps every `HubError` but `StreamBusy` to `NetStream.Publish.Denied`; `on_play` sends `Play.Reset`
and `Play.Start` before `hub.subscribe`, and `Play.StreamNotFound` on its error;
`crate::watch::Deadlines`' `handshake` is cleared by `Watch::ready` after C2.

**Suggested API.** `RtmpConfig::max_message_bytes: usize` (the chunk reader's message bound, its
partial bound twice it); a refusal of the hub's hooks answered `NetStream.Publish.Unauthorized` or
`NetStream.Play.Failed` with the hook's reason as the description (or a status the hook names in
`HubError::Denied`); a play's statuses sent once its subscription is made; and a deadline from
accept to the first publish or play (`RtmpConfig::start_timeout`, or `handshake_timeout` so
extended).

**How akit would use it.** `akit_media::streaming`'s RTMP listener sets `max_message_bytes` and its
deadline from `streaming.rtmp`, which reads `max_message_bytes` then; its hook's refusals reach the
client as fit's.

## A264 akio-media's RTSP server: a hook on each request, DESCRIBE asked, a sessionless idle bound

**Status.** Requested by I120 (2026-11-27). Nothing waits for it: RTSP's `auth` takes credentials in
the URL's query, checked in the hub's hooks, and `auth_methods`, `realm` and `idle_timeout` are
refused as not supported yet (a stopgap recorded in [progress](progress.md)).

**Problem.** fit checks every RTSP request that publishes or plays against the listener's policy
(fib's `OnRequest`): DESCRIBE and a SETUP play on the stream their URL names, ANNOUNCE publish,
PLAY and RECORD play and publish on the connection's session's stream, with credentials from the
`Authorization` field (Digest, with its own stateless nonces and realm, or Basic when offered) or
the URL's query; a refusal is 401 with the challenges, 403, or 503 with `Retry-After` for a password
being checked. akio-media's server has `RtspConfig::credentials`, which makes every request
authenticate and does not tell the hooks who the user is, and the hub's hooks, asked on ANNOUNCE
and on SETUP and PLAY's subscription with the URL's query only; DESCRIBE answers a stream's SDP
without asking them, and its `Content-Base` leaves the URL's query out, so the SETUP and PLAY a
client sends after it carry no credentials. A recording's SETUP whose URL has the query before the
track's control, as ffmpeg writes it after an ANNOUNCE with a query
(`rtsp://h/live/cam?token=T/streamid=0`), is answered 404: the control is looked for in the path.
A connection without a session is held to `session_timeout`, where fit has `idle_timeout` for it.

**Evidence.** At akio `8a9601b9`, `akio-media/src/rtsp/mod.rs`: `RtspSession::handle` checks
`cfg.credentials` for every request but OPTIONS and answers 401 with both challenges;
`describe` reads `hub.stream_info` directly and writes `Content-Base` without the query; `setup`
matches a recording's media by its control against the URL up to the `?`; `PeerInfo` carries the
protocol, the peer and the query; `on_open`'s deadlines are `request_timeout` and
`session_timeout`. akit's `crates/akit/tests/media.rs` shows ffmpeg's RTSP publish with
`?token=` answered 404 by akit and taken by fit.

**Suggested API.** `RtspConfig::on_request: Option<Arc<dyn Fn(&RequestInfo<'_>) -> Option<Response>
+ Send + Sync>>`, called before a request is handled, on its connection's thread, with the method,
the URL, the header fields, the connection's session path (if any) and whether it records; a
`Response` returned answers the request instead. DESCRIBE asked of the hub's `on_subscribe`, its
`Content-Base` keeping the query; a track's control taken after the query too (fit's `rtspPath`).
And `RtspConfig::idle_timeout` for a connection without a session.

**How akit would use it.** fit's `streaming/rtspauth.go` ported onto the hook (Digest with fit's
nonces, Basic, the query; 401, 403, 503); `streaming.rtsp`'s `auth_methods`, `realm` (akit's
default `akit`, decision 037) and `idle_timeout` read.

## A265 akio-media's SRT server: a resource's query in the access control syntax

**Status.** Requested by I122 (2026-11-27). Nothing waits for it: akit refuses such a stream ID
with 1400 (a bad request) rather than publish or play it under a path holding the query (a stopgap
recorded in [progress](progress.md)); a token goes as `s=`, or in a plain stream ID's query.

**Problem.** fit takes credentials in the query of the access control syntax's resource,
`#!::r=live/cam?token=T,m=publish`, as of a plain stream ID, `live/cam?token=T` (fit's
`parseStreamID`). akio-media's `StreamId::parse` splits the query of a plain stream ID only: the
access control syntax's `r=` is the resource whole, so the hub path would be `live/cam?token=T`.

**Evidence.** At akio `8a9601b9`, `akio-media/src/srt/listener.rs`: `StreamId::parse` sets
`query` for a stream ID without `#!::` and takes `r=` as it is otherwise; the server publishes and
plays `id.resource`.

**Suggested API.** `StreamId::parse` splitting `r=`'s query into `query` (an `r=` that is only a
query refused), as fit's; or a parse hook in `SrtConfig`.

**How akit would use it.** `akit_media::streaming`'s SRT connect hook reads the query's token, or
user and pass, as it does a plain stream ID's, and stops refusing such stream IDs.

## A266 akio-media's HTTP-FLV and HLS under a path prefix, and HLS of both formats

**Status.** Requested by I123 (2026-11-27). Nothing waits for it: akit's `httpflv` and `hls` with a
`prefix` other than `/` set the request's path without it before calling akio-media, and `hls`
keeps one `Hls` a format (a stopgap recorded in [progress](progress.md)).

**Problem.** fit's handlers serve their streams under a route's prefix (`/hls/live/cam/index.m3u8`
plays `live/cam`) and play either segment format from one handler, the other with `?format=`.
akio-media's `HttpFlv` and `Hls` take the stream from the whole path, and an `Hls` has one format.
Setting the path to fit them changes what the access log writes, and two `Hls` keep two sets of
segments under two `max_bytes`.

**Evidence.** At akio `8a9601b9`, `akio-media/src/httpflv.rs`: `path_of(ctx.path(), ".flv")`;
`hls.rs`: `HlsConfig::format`, one `SegmentFormat`, and the playlist's path parsed from
`ctx.path()`.

**Suggested API.** `HttpFlvConfig::prefix` and `HlsConfig::prefix` (the part of the path before the
stream's), and `HlsConfig::formats` (more formats served by the same `Hls`, chosen by `?format=`
for a playlist and by the extension for a segment), as fib's `HandlerConfig.Prefix` and
`Formats`.

**How akit would use it.** `akit_media::handlers` hands the prefix and both formats to one service
each and stops rewriting the request's path.

## A267 akio-media's WHEP: waiting for a publisher

**Status.** Requested by I124 (2026-11-27). Nothing waits for it: akit's `stream_whep` answers an
offer for a stream with no publisher 404 at once, whatever `streaming.webrtc.play_wait` says
(`akit run` logs a warning when it is set), and a viewer whose publisher leaves is ended at once (a
stopgap recorded in [progress](progress.md)).

**Problem.** fit's `streaming.webrtc.play_wait` lets a WHEP offer wait for a stream that has no
publisher yet, its request held, answered once one arrives or 404 when none does in time, at most
1,024 offers waiting; and fit's viewer whose publisher leaves waits 10 seconds for the next one (a
republish, an encoder's reconnect). akio-media's `WhipServer` answers such an offer 404 at once, and
`RtcMedia`'s viewer is closed when its stream ends. akit cannot hold the request itself without a
subscription of its own whose sink, run under the stream's mutex, would have to hand the request
back to the server on another thread.

**Evidence.** At akio `d8d51f61`, `akio-media/src/whip.rs`: `offer` answers
`HubError::NotFound` when `stream_info(&path)` is not publishing; `webrtc.rs`: `Player` subscribes
with `SubscribeOptions` without `wait` or `start_wait`. fib's `media/rtc` `Config.PlayWait`,
`MaxPlayWaiters` (1,024) and `RepublishWait` (10 s).

**Suggested API.** `WhipConfig::play_wait` and `max_play_waiters` (the offer held, its answer sent
once the stream has tracks, 404 past the wait, 503 past the bound), and
`RtcMediaConfig::republish_wait` (the viewer's subscription waits for the next publisher).

**How akit would use it.** `streaming.webrtc.play_wait` set on the role's `WhipServer`, the
warning dropped, and a republish wait of 10 seconds, as fit's.

## A268 An `RtcServer` on a listener bound to port 0

**Status.** Requested by I124 (2026-11-27). Nothing waits for it: akit finds a free port with a
socket of its own, closes it and binds the role's WebRTC listener to that port, as akio-media's
own tests do; another process may take the port between the two (a stopgap recorded in
[progress](progress.md)).

**Problem.** An answer's host candidates name the port peers reach the server at, so
`RtcServer::new` takes them before the listener it serves is bound; a listener on port 0, as fit's
tests and a config of `"listen": "127.0.0.1:0"` have it, binds a port only `Engine::add_listener`
tells. fib's `Mux` reads the binding's address once bound.

**Evidence.** At akio `d8d51f61`, `akio-webrtc/src/lib.rs`: `RtcServer::new(addrs)`, the
candidates fixed at construction (`host_candidates(&addrs)`); `akio-media/tests/webrtc_media.rs`:
`setup` binds a probe socket to find the port.

**Suggested API.** `RtcServer::set_host_candidates` (or candidates given with port 0 taking the
port of the listener the server is served on once it is bound), or `Listener::udp_from_std` for a
socket bound by the caller.

**How akit would use it.** The role's WebRTC listener bound as configured and the candidates taken
from the port it got, without the probe; `webrtc.media`'s (I113), which finds its port the same way,
too.
## A283 The BitTorrent client on the caller's engine

**Status.** Requested by I150 (2026-11-27) for I152. Stopgap: the client role runs
akio-bittorrent's `Client` on the engine `Client::new` makes for it (its I/O threads, one port for
TCP and UDP), beside the listeners' engine, recorded in [progress](progress.md).

**Problem.** fit's client role runs fib's client on the listeners' engine (fib's A79,
`Config.SharedEngine`): one engine, one set of I/O threads and limits for the gateway and its
roles, as akit's other roles (`akit_roles::host::Host::engine`). akio-bittorrent's `Client::new`
always builds and starts an `Engine` of its own (`ClientConfig::threads` I/O threads per
listener, client threads for trackers and web seeds).

**Evidence.** At akio `a8171fda`, `crates/akio-bittorrent/src/client/mod.rs`: `Client::new` calls
`Engine::builder().listen(Listener::tcp(listen)...).listen(Listener::udp(listen)...).build()`
and `engine.start()`; `ClientConfig` has no engine.

**Suggested API.** `ClientConfig::engine: Option<Arc<Engine>>`: when set, the client adds its TCP
listener (`Engine::add_listener`) and UDP binding (`bind_udp`) to that running engine, dials and
runs its tracker and HTTP clients on it, and `Client::close` removes them, leaving the engine and
another client on it running.

**How akit would use it.** The client role passes `Host::engine`, as the tunnel's and LLM roles
do, and its port joins the listeners' engine.

## A284 The BitTorrent client's peers refused by address, its connections bounded, its limits changed at run time

**Status.** Requested by I150 (2026-11-27) for I152 and I154. Until then akit refuses the client's
`access`, `limits.max_conns`, `limits.max_half_open` and `disk_queue_bytes` as not supported yet,
and a change of `max_conns_per_torrent` or `upload_slots` makes a new client.

**Problem.** fit's client (fib's A77 and A78) refuses peers by address (the role's `access`: a
refused peer is closed as soon as it connects over TCP, its uTP connection ignored, never dialed,
and as a DHT node gets no answers), bounds the client's connections in all (`max_conns`, 200) and
its dials at once (`max_half_open`, 32), bounds the received blocks waiting for the disk
(`disk_queue_bytes`, 64 MiB, holding the peers' reads past it), and changes its limits in place
(`Client.SetLimits`, `Torrent.SetLimits`), keeping the peers.

**Evidence.** At akio `a8171fda`, `ClientConfig` has `max_peers_per_torrent` and `upload_slots`,
read once by `Client::new`, and no filter, no client-wide connection or dial bound and no bound of
the pieces waiting for the pool; `Client::set_rate_limits` and `Torrent::set_rate_limits` change
the rates only.

**Suggested API.** `ClientConfig::filter: Option<Arc<dyn Fn(&SocketAddr) -> bool + Send + Sync>>`
and `Client::refilter()` (closing the connected peers it now refuses); `ClientConfig::max_conns`,
`max_half_open`, `disk_queue_bytes`; `Client::set_limits(Limits)` and `Torrent::set_limits` for
the connection counts, upload slots and rates.

**How akit would use it.** The role sets the filter from its `access` and the global one at every
apply and refilters, and moves its limits in place.

## A285 The tracker server's store: a bound of all its peers, bounds changed in place, its torrents listed

**Status.** Requested by I150 (2026-11-27) for I151. Until then akit refuses the tracker's
`max_peers`, and a change of `max_torrents`, `max_peers_per_torrent` or `peer_ttl` makes a new
`TrackerServer`, its swarms starting empty (fit's I201 did the same before fib's A85).

**Problem.** fit's tracker store (fib's `MemoryStore`, A85) bounds every torrent's peers together
(`max_peers`, 500,000 by default, about 80 MiB), changes its bounds and TTL in place
(`SetConfig`), lists its torrents (`Range`, for a public tracker's torrents on its pages) and counts
its peers (`Peers`, for its statistics).

**Evidence.** At akio `a8171fda`, `TrackerConfig` has `max_swarms` and `max_peers_per_swarm` only,
read by `TrackerServer::new`; `TrackerServer` has `swarms()` and `stats(&InfoHash)`, no peer count,
no way to change its config, and no way to walk its swarms.

**Suggested API.** `TrackerConfig::max_peers` (all swarms together; a peer past it still gets
peers, as one past `max_peers_per_swarm`); `TrackerServer::set_config(TrackerConfig)` keeping the
swarms; `TrackerServer::peers() -> usize`; `TrackerServer::range(|InfoHash, ScrapeStats| -> bool)`.

**How akit would use it.** The role reads `max_peers`, sets its bounds in place on a reload, and
lists a public tracker's torrents and its peers on its console.

Also, so that the role can use `TrackerServer::http()` and `udp()` as they are (I151 answers both
with its own code on `TrackerServer::announce` and `stats`, a stopgap recorded in
[progress](progress.md)): the answers as fib's (a request's `compact` on unless `0`, a missing
`left` a leecher, 50 peers when `numwant` is missing or -1, `min interval`, `external ip`,
`peers6`, `tracker is full`, a UDP answer within 1,400 bytes, a UDP request with a connection ID
that is not the client's dropped rather than answered, fib's failure reasons for a bad query), and
hooks for an info hash allowed and a request authorized (`TrackerConfig::allow_fn`,
`authorize`), whose refusals are the failure reasons.

## A286 A torrent's own storage directory for a magnet link

**Status.** Requested by I150 (2026-11-27) for I153. Stopgap (I153, recorded in
[progress](progress.md)): a magnet link added in a category downloads where the client was made
to until its metadata comes; the client's event wakes the role's manager, which removes the
torrent from the client, removes what it wrote there, and adds it again from its .torrent into
its category's directory, where it dials its peers again.

**Problem.** fit downloads a torrent into its category's directory, a magnet link's too
(fib's `StorageOpener`, called when the info arrives). akio-bittorrent's `AddOptions::storage`
takes a storage made from the torrent's `Info` (`FileStorage::new(dir, &info)`), which a magnet
link does not have yet; without it the files go below `ClientConfig::data_dir`.

**Evidence.** At akio `a8171fda`, `AddOptions::storage: Option<Arc<dyn Storage>>`;
`FileStorage::new(impl AsRef<Path>, &Info)`.

**Suggested API.** `AddOptions::dir: Option<PathBuf>` (the directory a torrent's `FileStorage`
is made below, when its info is known), or a storage opener called with the `Info`.

**How akit would use it.** Categories and `download_dir` changes for torrents added by magnet
link.

## A287 A reader of a torrent's bytes that waits for their pieces, and file priorities

**Status.** Requested by I150 (2026-11-27) for I155.

**Problem.** fit's `bt_stream` plays a torrent's file while it downloads: a reader (fib's
`Reader`) whose reads ask for their pieces first and wait for them, read ahead within a bound,
and a temporary torrent downloads only the files streamed (file priorities).

**Evidence.** At akio `a8171fda`, `Torrent` has no read of its bytes and no file or piece
priorities.

**Suggested API.** `Torrent::reader(file) -> Reader` with `read_at(offset, len, done)` completing
on the pool once the pieces are verified, a readahead bound, and `Torrent::set_file_priorities`.

**How akit would use it.** `bt_stream` answers ranges from it, never waiting on an I/O thread.

## A288 A torrent's trackers and web seeds' state, and a recheck in place

**Status.** Requested by I150 (2026-11-27) for I157.

**Problem.** fit's Admin API and pages show each tracker's announces (last, next, errors, peers
got), the peer sources and the web seeds' state (fib's A87), and recheck a torrent's data in place
(fib's A88).

**Evidence.** At akio `a8171fda`, `Torrent` has `peers()` and `stats()`; trackers' failures only
as `TorrentEvent::TrackerError`; no recheck.

**Suggested API.** `Torrent::trackers() -> Vec<TrackerState>`, `Torrent::web_seeds()`,
`Torrent::recheck()`.

**How akit would use it.** `/bittorrent/torrents/<id>/trackers` and the recheck action.

## A277 A client response's peer certificate chain, and a refused certificate kept for the caller

**Status.** Requested by I132 (2026-11-27). Nothing waits for it: the cluster manager probes a
member's certificate on a connection of its own (a stopgap recorded in [progress](progress.md)).

**Problem.** fit's cluster manager (`cluster/transport.go`) checks a member's certificate itself in
`VerifyConnection`, which every handshake runs, against a pin, the member's CAs or the system's,
keeping the certificate whatever comes of it: a certificate refused is shown to a person with its
fingerprint, so that it can be pinned, and the overview shows each member's certificate. akio's
HTTP client gives a response without the chain its connection's handshake saw
(`ClientResponse` has no peer certificates), and akio-tls's client fails the handshake on a
certificate it refuses without letting the caller see it: the request fails with `Io` or `Closed`,
and nothing tells a certificate refused from a connection refused.

**Evidence.** At akio `a8171fda`, `akio-http/src/client.rs`'s `ClientResponse` has `status`,
`headers`, `body`, `trailers`, `pushed`, `url`; `akio-tls/src/proto/client.rs`'s
`peer_certificate` returns a `Fatal` from `verify_chain` or `pinned_leaf_key` and the chain is
dropped; `tls_info` gives the chain only for a connection the caller holds.

**Suggested API.** `ClientResponse::peer_certificates: Option<Arc<[Vec<u8>]>>` (the chain of the
connection that answered, leaf first, as `akio_tls::peer_chain` gives a server); and
`ClientError::Tls { alert, chain: Vec<Vec<u8>> }` for a handshake that failed verifying the
server's certificate, with the chain it refused (or a `ClientConfigBuilder::on_refused(Fn(&[Vec<u8>]))`
callback).

**How akit would use it.** `akit_cluster::transport` reads the certificate of every call's
connection for the state's `tls`, and a refused one from the error for `tls_untrusted` and
`tls_pin_mismatch`; the probe (`Transport::probe`, a TLS connection that verifies nothing, sends a
request without credentials, reads `tls_info`'s chain and checks it with akio-x509's verifier or
the pin) goes.

## A301 `H2Client` ended by the engine's graceful shutdown

**Status.** Requested (2026-11-28). Stopgap in akit: `akit_upstream::EngineSlot::shutdown`, which
`akit run` calls before it shuts its engine down, closes the pools' idle h2 connections (recorded
in [progress](progress.md)).

**Problem.** `akit run` proxying to an h2c or h2 upstream waited the whole `-shutdown-timeout` on
every stop and exited 1, with nothing in flight: the pool's idle `H2Client`, dialed on the
listeners' engine, stays open through `Engine::shutdown`, and `Engine::wait` waits for it until
`Config::shutdown_timeout`, then forces it closed. akio's HTTP/1 client does not do this: its
`DialAcceptor::on_shutdown` closes an idle keep-alive connection at once and a busy one once its
response is in. Any user of `H2Client` that keeps a connection for later requests, as akit's
pools do, pays the whole timeout at the engine's shutdown.

**Evidence.** At akio `d8d51f61` (and `5297cdd0`), `akio-http/src/h2client.rs`: `impl Acceptor for
H2Acceptor` has only `on_open`; `client.rs`'s `DialAcceptor::on_shutdown` (the comment there
describes the same symptom, fixed for HTTP/1). Reproduced by akit's
`akit-upstream/tests/h2.rs` `an_idle_connection_does_not_hold_the_engine_shutdown` with the
stopgap call left out: the shutdown takes the whole 20 s timeout.

**Suggested API.** No new API: `H2Acceptor::on_shutdown` sets the connection's `goaway` flag (so
`H2Client::is_open` is false and no new stream starts), sends `GOAWAY(NO_ERROR)`, and closes the
connection at once when no stream is open, else after its last stream is over (requests waiting
for a stream fail with `ClientError::Closed`, as after the server's `GOAWAY`). The same for
`H2Client::connect`'s TLS dial.

**How akit would use it.** `EngineSlot::shutdown`, the list of h2 pools it walks and its
call in `akit run` go; the pools already stop using a connection that is no longer
open (`H2Client::is_open`) and close it once its last stream is over.
