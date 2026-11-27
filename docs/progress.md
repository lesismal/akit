# akit development progress

[English](progress.md) | [简体中文](progress.zh-CN.md)

What has actually been built, iteration by iteration, and where the next session picks up. The
[development plan](dev-plan.md) says what each iteration is meant to do; this file says what was
done and where the work differs from the plan.

## Where things stand

| | |
| --- | --- |
| Last updated | 2026-11-27 |
| Milestone | M0, foundations, done (I01–I08); M1, the proxy core, done (I09–I25, I17b, I18b; [the M1 report](../bench/results/m1-report-2026-11-25.md)); M2, dynamic config and TLS, done (I26–I38); M3, gateway features, done (I39–I56, I51b; [the M3 report](../bench/results/m3-report-2026-11-25.md)); M4, platform integration, done (I57–I66; [the M4 report](../bench/results/m4-report-2026-11-25.md)); M5, caching and extensions, done (I67–I76; [the M5 report](../bench/results/m5-report-2026-11-25.md)); 1.0, done: I77, I78, I79, I79b and I80; I80b doing (the threat model's akit limitations done, the review's findings to come); after 1.0, Logging done (I81–I83) and Access lists done (I85–I87); akit's names done (I97, decision 037: every external name that said "fit" says "akit"); Tunnel and LLM (I98–I109, decision 038) done to I103b and I102d (I102c's WebSocket links on akio's A236). |
| akio | `a8171fda` on akio's `main` (`1.0.0-rc.1`, with every request akit pins in [akio API requests](akio-api-requests.md), akio-wasm, ADR-125, and the shared I/O groups of ADR-130), pinned by `rev` in `Cargo.toml` |
| fit | the specification; read at `7daee573` for this plan |
| What works | the console (`admin.console`, I89–I91: fit's workbench pages, with akit's names (I97), behind bcrypt logins, over TLS, its access lists, its connections bounded by fit's rules on akio's A232) and the Admin API over TLS off loopback (I92), their certificates read again when the files change (I94); the encrypted store (I93–I93c): fit's `fitstore` byte for byte, `admin.store`, `akit store export` and `import`, `akit run` keeping the config in it (merged onto the config files at a start, written back on every change, read again on SIGHUP, `admin` kept in the bootstrap file), the console's password changes kept in it, the Admin API's `store` exports and imports, and `akit run -admin` (the console with a temporary password, the store beside the config with its key file, a `.store` given with `-c`), fit and akit running in turn from one store; `akit hash-password`; `akit run` and `akit validate` with fit's flags and exit codes, `akit adapt` and `akit fmt` writing configs as fit writes them (I73), `akit build` building akit with the modules of other crates, or fewer of its own (I70), nginx configs and Caddyfiles converted by `akit adapt -from` as fit converts them (I74), configs in JSON, `.conf` and YAML merged from files and directories; SIGHUP reloads the config (a broken one keeps the running one; listeners kept (as fit's `binding.same`: certificate files and ACME names may change), added, changed and drained on akio's A1, those it started with too, h3 and `udp` ones too, their UDP side drained (akio's A22)); a graceful shutdown on SIGINT and SIGTERM; HTTP/1.1, h2c, TLS (SNI, ALPN, client authentication; certificates chosen per handshake as fit's, replaced by a reload or by `-watch-certs` without closing the listener) and HTTP/3 listeners, `Alt-Svc` on the TCP side; router v1 on canonical paths, with fit's matchers (method, header, query, remote IP, path regexps) and expressions; route middleware: `rewrite` (prefixes and regexps), `redirect` (with the regexp's groups), `headers`, `rate_limit`, `concurrency_limit`, `body_limit`, `basic_auth` (bcrypt, checked on akio's pool), `client_cert` (mTLS per route), `forward_auth` (an auth service asked on akio's client, the request suspended meanwhile), `circuit_breaker`, `compress` (zstd, brotli and gzip by q value, streamed bodies encoded write by write) and `jwt` (JSON Web Tokens verified with akio-crypto, keys from PEM files, a secret or a JWK set fetched by akio's client and rotated); the `respond`, `static` (conditional and range requests, files opened on akio's pool, precompressed twins by q value, compressed copies of files kept in memory, `try_files`, listings, `strip_prefix`) and `proxy` handlers (fit's forwarding fields, trusted proxies, the client's `Host`, bodies streamed both ways with linked backpressure, trailers both ways, `timeout` and the upstream's `response_timeout`, `response_header_timeout` and `response_idle_timeout`, 504 for timeouts, the upstream request given up whenever the client goes, `retry` with its budget, the `timeout` bounding the attempts together; `upstreams` chosen at random by weight, a `failover` taking the requests while the chosen upstream has no address in rotation, a `mirror` getting copies of requests whose bodies came whole, at most 256 under way, never waited for; upstreams over HTTP/1.1, h2c and h2, h2 with fit's `tls_server_name`, `tls_ca_file` and client certificate); `limits.max_body_bytes` on every listener (413) and in the proxy, and `limits.write_stall_timeout` (changed by a reload too); access logs in fit's json, text and line formats, and akit's own log (fit's `log`, its loggers, akio's records as `engine`), to files rotated by size and time, kept and gzipped, changed in place by a reload, listed, rotated and cleaned up at the Admin API's `/logs` (I81–I83); upstream pools with fit's nine load balancing policies (round robin, weighted round robin, least connections, random, p2c, and consistent hashing of the client's IP, the path, a header or a cookie) and sticky cookies, passing over addresses taken out by passive health checks (`max_fails`, `fail_timeout`) and by active ones (`health_check`: http and tcp probes), an unchanged upstream's pool, connections and health kept across reloads (I38); the Admin API on loopback (`GET /config`, by JSON pointer too, `POST /load`, edits by pointer with `PUT`, `PATCH` and `DELETE /config`, `POST /rollback/{n}`, `POST /reload`, each with `If-Match` and all but a rollback with `?dry_run=1`, `GET /versions`; with `token_file`, a bearer token on every request), changes applied as SIGHUP's reloads, one history for both; `run -watch` applying the config's files each time they change (fit's file provider, polled); Prometheus metrics at the Admin API's `GET /metrics` (requests by listener and route with `metrics`, upstreams and backends, certificates, connections, reloads, akio's engine); fit's read-only dashboard at the Admin API's `/dashboard/`, on `GET /status` and `GET /config/effective` (I63); certificates obtained with ACME through HTTP-01 or TLS-ALPN-01 for the names listeners list in `tls.acme`, kept in a storage and renewed by their lifetime or the CA's renewal information (ARI), their OCSP responses stapled, and obtained during handshakes for the names an ask URL allows (`tls.on_demand`, rate limited, the handshakes waiting without holding an I/O thread); HTTPS by default (`auto_https`): a config that writes no listeners served on HTTPS with its routes' host names' certificates and on HTTP redirected to it (308), an empty `tls` given its routes' names; layer 4 `services` (I51): `tcp` and `udp` listeners proxied to an upstream's addresses, backpressure linked both ways, idle timeouts, `max_conns`, drained after a reload; services sharing a `tcp` listener by the server names of their TLS ClientHellos, passed through without terminating them (I52), and PROXY protocol headers (v1, v2) taken from a listener's trusted proxies, on `tcp` and HTTP listeners, and sent to a service's upstream; `tcp` beside HTTP on one port (I53), told apart by what a client sends first; the forward proxy (I55): SOCKS5, SOCKS4 and 4a CONNECT on a listener's `socks`, alone or beside HTTP and `tcp` on one port, and HTTP/1.1 CONNECT tunnels by the `forward_proxy` handler, with fit's rules for clients and destinations (an address decided once connected), connect and idle timeouts; WebSockets proxied (I49) from HTTP/1.1 upgrades and HTTP/2 and HTTP/3 extended CONNECTs to HTTP/1.1 upstreams, backpressure linked both ways, `tunnel_idle_timeout`; gRPC calls proxied (I50) to h2 and h2c upstreams, streamed both ways with their trailers, bounded by `grpc-timeout`, answered with gRPC statuses when they fail; the response cache (I67, I68): RFC 9111 in a bounded memory tier shared by a zone, streamed bodies kept as they go, the cache lock, stale-while-revalidate and stale-if-error, a disk tier on a pool of its own, and purges by URL, URL prefix, tag or zone at the Admin API's `POST /cache/purge` (I69); the provider framework (I57): providers' parts merged into the base config by fit's rules, applied as versions of their own, `GET /config/effective` with where each part came from; the Docker provider (I58): routes, upstreams and services from the labels of running containers, followed through the daemon's events on akio's HTTP client over its Unix socket (or TCP); the Kubernetes provider (I59, I60): the Gateway API's Gateways of akit's GatewayClasses and their HTTPRoutes, GRPCRoutes, TLSRoutes, TCPRoutes and UDPRoutes, and the Ingresses of akit's IngressClasses, listed and watched over HTTP/2 or HTTP/1.1, served as listeners, routes and services to their Services' ready endpoints, their TLS Secrets written as certificate files, the Gateway API's statuses written back; the Consul catalog, Consul KV and etcd providers (I61), followed with blocking queries and watches; certificates shared by instances on several hosts through Redis, etcd or Consul (`acme.shared_storage`, I62), with leased locks and challenges answered by whichever instance the CA reaches; OpenTelemetry (I64): requests traced by their `traceparent` and `telemetry.traces.sample_ratio`, the upstream sent akit's span, spans and metrics exported over OTLP/HTTP; the `fastcgi` handler (I75): PHP-FPM and other FastCGI applications over TCP or a Unix-domain socket, a bounded pool of connections, bodies streamed both ways with linked backpressure, `try_files`, dotfiles and fit's timeouts; the `wasm` middleware (I71): WebAssembly plugins of akit's ABI (fit's, its names renamed: `akit_on_request`, module `akit`, I97) on akio-wasm's instance pool, bounded in time and memory, never on an I/O thread; access lists (I85–I87): allow and deny lists of client addresses, global, per listener, route and service and on the Admin API, inline or in list files (addresses, prefixes, ranges, `ipfilter.dat` and `.p2p` lines, watched with the certificates), refused at accept by akio's filter, answered 403 per request, or closed once a layer 4 connection's service is known; SOCKS clients checked at their request; the compatibility gate: 80 fixtures, 56 valid, 23 refused only as not supported yet (19 fields), 1 skipped, none failed, every fit template valid; the config schema and the Admin API frozen as v1 (I79, [the policy](compatibility.md#v1-what-stays-stable)), checked by `tools/v1-freeze.py` and `v1_surface_is_frozen`; the old names of fit's renamed fields read with fit's warning; `limits.max_header_bytes`, `max_conns`, `max_write_queue_bytes` and `engine` (decision 028); the proxy conformance suite and h2spec (I25); fit's `tunnel` and `llm` roles read and checked (I98), one address one server across the gateway and the roles (I98); link tokens (`akitt1.`, fit's proofs, I99) issued, listed, expired and revoked at the Admin API's `/tokens` and by `akit token` (I100); the roles run beside the gateway by `akit run` on its engine (I100); the tunnel's hub, node and proxy (I101) over TCP, TLS (I102, pinned or by CAs, the hub's certificate kept), QUIC (I102b) and WebSocket (I102c, `ws` and `wss`), registered with tokens or the shared password, sealed by `body_password`, linking with fit's own hub, node and proxy (decision 038); the hub's calls and status for its pages, tokens closing their links at their expiry (I103); a hub's console socket and a proxy's web proxies, sealed as fit's (I103b); the roles' consoles (their certificates read again when their files change), the hub's pages (I109), the tunnel's node's and proxy's (I109b) and the LLM roles' (I109c, with their methods, I109d), shown on `admin.console` too; the hub's QUIC handshakes bounded as fit's (`handshake_timeout`, `max_handshakes` with a Retry past half, akio's A245) and its listeners' certificates read again when their files change (I102d) |
| Next | the external security review ([scope](security/review-scope.md)), whose findings finish I80b; fit's answer to the fields decision 035 proposes (CRL files, a WASM digest); the tracks after 1.0 ([the plan](dev-plan.md#after-10)): Console and encrypted store done (I89–I94, I93b, I93c); fit's Admin API rules for connections without the token, on the console's slots (akio's A232) (Logging done: I81–I83; Access lists done: I85–I87); the Gateway API's conformance run (tests/conformance, the maintainer's) |
| akio requests | A1–A24, A29, A30, A32, A33, A35–A38, A45, A48, A49, A51–A54, A57, A58, A63–A66, A75, A78, A79, A90–A94, A105, A108, A109, A111–A113, A117, A132–A135, A148, A165–A172, A180, A200, A214, A215, A220, A230–A238, A245 and A247–A249 on akio's main and pinned; A225 (an external review, L15) requested ([akio API requests](akio-api-requests.md)) |
| Streaming and media auth | [I120–I126](dev-plan.md#streaming-and-media-auth), fit's I115–I117 (decision 040): I120 (`media_auth` and `streaming` in the config), I121 (the policies, the checks, `akit media-token`), I122 (the role's hub, RTMP, RTSP and SRT), I123 (`httpflv`, `hls`), I124 (the WebRTC port, `stream_whip`, `stream_whep`), I125 (`media_pages`) and I126 (`auth` on `whip`, `whep`; `webrtc_proxy`'s with I114) done; the media library's `libraries`, `vod` and `channels` wait for its track; akio requests A263–A268 |

## Log

### 2026-11-27: I114, WHIP and WHEP proxied

- `akit_webrtc::proxy`, the role `webrtc.proxy` (fit's `proxy.go`, `relay.go`): a UDP listener of
  the engine (the member's access its filter); sessions with their ufrags (at most 4 a session)
  and the address their peers are relayed to; a new peer address's first datagram must be a STUN
  Binding request whose USERNAME begins with a session's ufrag and whose MESSAGE-INTEGRITY its pwd
  signs (dropped and counted otherwise: not STUN, unknown ufrag, integrity); its relay a UDP socket
  of its own on the engine (`Engine::bind_udp` from `relay_ip`, a port of `port_min`..`port_max`
  tried from a random one, one in use skipped, a full range dropped as `no_port`), relaying the
  5-tuple both ways, at most `max_relays` and 8 a session; a peer silent for `idle_timeout` closed
  by akio's idle timeout of its connection; a session nothing is relayed for forgotten after
  `session_timeout` (a timer re-armed when its last relay ends); a DELETE forgetting it, its relays
  closed a second later. `/metrics`: fit's proxy families, `akit_webrtc_proxy_*`.
- The handler `webrtc_proxy` (fit's `proxyhttp.go`): an offer, its candidates taken out, sent to a
  media server the upstream picks (akit-upstream's pool, akio's client, `timeout`), within
  `max_sessions` (503, `Retry-After: 1`) and `max_sdp_bytes` (413; 502 for an answer past it); a
  201 answered with the answer's candidates replaced by the proxy port's (and its `c=` lines),
  its ufrags and pwds recorded against `media_addrs` or its best host or server-reflexive
  candidate (IPv4 first), `Location` the endpoint's path and the session's ID, the media server's
  `Link` headers below its session's URL moved below akit's; a session's URL and the paths below
  it passed to the same media server (DELETE forgetting the session; PATCH with an ICE restart's
  offer recording its credentials, a trickled candidate answered 204); an answer akit cannot
  take answered 502 and the session hung up at the media server. `akit run` runs the role and
  serves the handler: nothing of `webrtc` is refused any more.
- The UDP ports of `webrtc.proxy`, `webrtc.media` and `webrtc.turn` on the config's
  `engine.event_loops` I/O threads, as the HTTP listeners (one a core when 0): on a 16-core host
  three akits in one test process ran out of descriptors under the default.
- Tests (`tests/proxy.rs`, three akits on loopback): two akit media servers behind one akit
  proxy port, a publisher and a viewer on each, media flowing through the relays, a DELETE
  through akit; the answers naming the proxy's port alone; datagrams that are not a Binding
  request, and a Binding request of an unknown ufrag, dropped; `max_sessions`; a media server's
  404 passed on; a one-port range relaying one address and dropping the next; `session_timeout`;
  the proxy not running. Unit tests of the SDP rewrite (fit's `TestRewriteSDP`), the candidates
  stripped, the paths and `Link` headers, and the request's ufrag. The media tests' helpers moved
  to `tests/support`.

### 2026-11-27: a proxied body framed by its length, read whole by a client that went, is finished

- `proxy`: an HTTP/1.1 client that has read every byte a streamed response's `Content-Length`
  declares may close before the upstream's end reaches akit, and its going gave the upstream
  request up first: the body was dropped unfinished, so a body tap heard it cut, and the `cache`
  did not keep a response its client got whole. Now a stream framed by its length whose every
  byte was written is finished when the exchange ends early (the client gone, or any other
  error); one short of its length is still cut. This was `akit-cache`'s
  `bodies_through_the_proxy` failing on io_uring (10 of 30 runs alone on gpu51, the `/large`
  over HTTP/1.1 kept as cut); `a_framed_body_read_whole_before_its_end_is_kept` makes the order
  certain with an h2c upstream that ends the body only once let go.

### 2026-11-27: a node's and a proxy's tunnels listed in the order they opened

- `the_proxys_pages` failed now and then under load: it compared two tunnels' `created` as
  strings after opening them 5 ms apart. `created` is RFC 3339 with the millisecond's trailing
  zeros dropped, as fit's JSON writes it (`…:05.1Z` for 100 ms), which does not sort as text
  (`…:05.1Z` after `…:05.105Z`, `…:05Z` after `…:05.005Z`). The list itself was sorted by the wall
  clock's milliseconds, then by tunnel ID, which ties within a millisecond (and the ID is no age
  across links) and goes wrong when the clock steps back. A tunnel table now numbers its tunnels
  as it takes them and lists them in that order, as fit sorts by its monotonic clock; `created`
  stays what the pages show. The test opens the second tunnel once the first has echoed, no
  sleep, and tells them apart by the bytes each carried.

### 2026-11-27: the tunnel's tests bind port 0

- `tunnels_sealed_by_the_body_password` failed once in a full run on gpu51 (epoll): "forward
  127.0.0.1:3921: Address already in use". The tunnel's tests picked their ports by binding port 0
  and letting it go, then had the roles bind them; another process could take one in between.
  akit's own listeners in them now bind port 0, and the tests ask where they listen: the hub's
  listeners from its status, the forwards from the proxy's, the console socket from the hub,
  the web proxies from the proxy, and the roles' consoles from `Manager::console_addr` (new:
  `Consoles::addr`, which `RoleConsoles` answers). A node or proxy that links to a hub is applied
  once the hub has bound its port. A hub nothing answers on is port 1. What must still name its
  port before it binds keeps a picked one: fit's binary's listeners, a forward added through the
  pages (named by its address, as fit's), and a console its hub's socket dials (at its member's
  address, as fit's). The web proxy tests' reads take a connection the peer already reset as
  empty (macOS refuses its timeout then).

### 2026-11-27: the tunnel proxy's nodes no longer lost to its own apply

- `tunnels_with_tokens_plain_and_sealed` failed now and then on gpu51 (5 in 60 runs alone with
  `AKIO_TEST_BACKEND=uring`, always in its second, sealed round; the tunnel's tests do not read
  that variable, so their engines ran on epoll): "no tunnel opened", the proxy refusing every
  connection with no hub having node `lab`, though both had registered. The proxy's apply started
  a new hub's uplink, let go of the uplinks' lock, and only then forgot the nodes of the hubs
  stopped or linked anew. A fast hub let the new uplink register, and the hub's node list arrive,
  in between: the list just taken was forgotten, and the hub sends no other while its nodes stay
  as they are. The nodes are now forgotten before the lock is let go, and `set_nodes` holds it
  until it has taken a list (as fit's `setNodes` holds its own), so a list taken for a new uplink
  always comes after; a lost link's hub forgets its nodes under the same lock, only while its
  uplink is still the proxy's. After the fix: 250 of 250 runs alone on epoll, 50 of 50 on io_uring
  (`AKIO_DEFAULT_BACKEND=uring`).

### 2026-11-27: I152, the BitTorrent client

- `bittorrent.client` (`akit_bittorrent::client`) run by `akit run` on akio-bittorrent's
  `Client`, on an engine of its own until akio's A283: its torrents kept in `data_dir` as fit keeps
  them (`torrents.json` and `torrents/<id>.torrent`, each written to a temporary file, synced and
  renamed, members akit does not read written back as read) with akio's resume data in `resume/`;
  restored at start resuming without a check; a state file that does not read (not JSON, a later
  version, more than 100,000 torrents, an ID twice or malformed, a relative directory, past 64 MiB)
  keeps the role from starting rather than be overwritten; a torrent whose own files do not read
  stays listed with its error. A reload changes the rates and seed limits in place; a change of
  `listen`, `encryption`, `dht`, `pex`, `utp`, `lsd`, `disk_workers`, `data_dir`,
  `max_conns_per_torrent` or `upload_slots` makes a new client from the saved state, and one that
  cannot be made (its port taken) brings the old one back. The DHT bootstraps from fit's public
  routers, resolved on a thread of its own, only when `dht` is on.
- Its API (`ClientApi`, for I157's Admin API): add by .torrent file or magnet link (128 MiB and
  64 KiB at most, `max_torrents`), remove with or without the data, pause, resume, list, get; each
  change of the list saved at once. The client's events (metadata, completion, a seed limit) only
  note what happened; a saver thread writes it every `save_interval` (a magnet link's .torrent,
  `completed`, a torrent its seed limit paused saved paused). The console's `/api/torrents` and
  `/api/torrents/<id>`, answered on akio's pool.
- Peer ID `-AK0001-` (decision row 75).
- Tests: `client::tests` (fit's role tests: a download between two roles, a restart with no piece
  read again, a magnet link's .torrent saved and restored, reloads in place and by a new client, a
  port taken, bad state files, fit's state file read and written back, a broken torrent kept
  listed, the API's refusals, pause, resume, removal with the data, the console's routes);
  `tests/fit_interop.rs` with fit's binary: akit's client downloads from fit's through fit's
  tracker and fit's from akit's; `tests/aria2c_interop.rs` (`AKIT_ARIA2C`): aria2c 1.37 downloads from
  akit's client through akit's tracker over HTTP (aria2c speaks UDP to trackers only with its DHT
  on), on epoll and io_uring on the benchmark host. `akit_bittorrent::tracker::service` serves
  `bt_tracker` on a listener outside a config's routes.
- Decisions waiting for review: row 75.

### 2026-11-27: I113, WHIP and WHEP: the media server

- `akit_webrtc::media`, the role `webrtc.media` that `akit run` runs: akio-webrtc's ICE lite
  `RtcServer` on a UDP listener of the engine (its candidates `public_ips`, the listen address, or
  for a wildcard every interface's address that is up; a port 0 resolved to a free one first),
  the member's access its accept filter. Sessions as fit's: a publisher's offer answered with one
  codec of each kind, the first of its sending sections' that `codecs` lists (fit's `pickCodecs`,
  akio's `accept_offer_with`, A259), its viewers with the same; the publisher in RTP mode, each
  packet forwarded to every viewer's line of its kind as it arrives (`Peer::write_rtp`, A260),
  within `max_bitrate` with two seconds' burst (dropped and counted past it) and told as REMB
  (`set_receive_bitrate_cap`, A261); viewers' key frame requests, and each new connection, passed
  to the publisher as a PLI at most once a `key_frame_interval`; `ice_timeout` as the peers'
  consent timeouts (`set_ice_timeouts(min(8 s, ice_timeout), ice_timeout)`, A261);
  `connect_timeout` a timer of the session's own. The limits (PeerConnections, streams, viewers a
  stream, the offer's size) refuse as fit's (503 with `Retry-After: 5`, 409, 404, 413, 415, 422).
  A publisher gone ends its viewers. A reload that keeps `listen` and `public_ips` keeps the
  sessions; one that changes them ends them. `/metrics`: fit's media families, `akit_webrtc_media_*`.
- The handlers `whip` and `whep` (`akit_webrtc::handler`, fit's `handler.go`): the endpoint's
  POST and OPTIONS, the session's URL's DELETE and OPTIONS (PATCH 405, as fit's), `Location` the
  POST's path and the session's ID, `Link` headers with `ice_servers` and `turn_urls` with
  time-limited credentials of `webrtc.turn`'s secret (username `<expiry>:akit-<id>`). The offer is
  the route's buffered body, checked against `max_sdp_bytes`; the handler answers inline. They find
  the media server by the engine they serve on. `akit run` serves them (`webrtc_proxy` waits for
  I114).
- Tests (`tests/media.rs`, akio-webrtc `Endpoint`s publishing and playing through an akit
  listener): audio and video from a publisher reach two viewers; DELETE of a viewer, then of the
  publisher, ending its viewers; the limits; the HTTP edges; a reload kept and a restart; a viewer
  cut midway holding no one; `connect_timeout`; `max_bitrate`. Unit tests of the SDP read, the
  codecs picked, the `Link` headers and the bitrate bucket. A real WHIP/WHEP client of fit's: fit's
  media tests use fib's PeerConnection in Go, not run here (decision row 54).

### 2026-11-27: I112, the TURN server on akio's hooks

- On akio's A256-A258 (`d8d51f61`): relays bound in `port_min`..`port_max` (a range used up is
  answered 508), nonces made with `nonce_secret`, a TCP or TLS client's messages bounded by
  `max_stream_message`; `user_bandwidth` is fit's limiter (`akit_webrtc::limiter`: a token bucket
  a username and direction holding a second's worth, 65,536 at most, refilled ones swept every
  10 s) on akio-turn's `on_relay`; the events (`on_event`) counted and logged at debug; a reload's
  quotas, caps and lifetimes set on the running server (`set_limits`), so they no longer start a
  new one; a server a reload replaced drains (`drain`: new allocations refused with 508) until
  `-shutdown-timeout`, then closes (it does not end early when emptied, as fit's does); the
  servers' own addresses reached on their relayed ports (`holds_relay`) and on ports
  `allowed_peers` lists. The warnings of I111's stopgaps go, `mptcp`'s stays.
- `/metrics`: fit's TURN families, named `akit_turn_*`: allocations, pending allocations,
  permissions, channels, draining servers, relayed bytes by direction, dropped (bandwidth),
  unrelayed, allocated, refreshed, freed by reason, refused by code, auth failures, peers refused,
  restarts (`akit_turn_relayed_datagrams_total` goes, as fit has bytes).
- Tests: fit's `TestCaps` (508s counted, a cap raised in place), `TestBandwidth`, `TestOwnAddress`
  (two clients relay to each other's relayed addresses), `TestReload` (quotas in place, 508 from
  a draining server), a port range (its port taken, 508 when used up, free again after a delete),
  `TestLimiter`; the fib interop (`tests/fib_turn.rs`) against akit and fit alike, on macOS.

### 2026-11-27: I125, the media pages

- `media_pages` (`akit_media::pages`): fit's pages, `handler/mediapages/web` at the gate's pin,
  copied with the console's renames (decision 037) by `tools/media-pages.py`, which
  `make compat-check` checks byte for byte against fit's renamed; hls.js is the console's copy.
  The index with the prefix and title in it, ETags and 304s, a Content Security Policy naming the
  endpoints' other origins; `api/config` as fit's (its keys in order); the login against the
  policy's users (`Policy::login`, a bcrypt user checked on the pool while the request waits),
  answered with a token the policy signs and the cookie `akit_media_token` (Secure over TLS);
  `api/logout`.
- The room API (`pages::rooms`): join, members (the heartbeat), leave and the event stream, each
  room's members dropped after `member_ttl` without a heartbeat, a room forgotten once empty, by a
  timer of the room's every third of `member_ttl`; the rooms of a prefix on an engine shared by the
  configs that serve it, kept across a reload. A watcher's stream is written without waiting:
  past 64 KiB queued it waits for a drain (akio's `BodyStream::on_drain`, which may run at once
  while the waiter is registered, handled under the watcher's lock), holding only the latest room
  state and a bounded queue of chat events (cut past 256 events or 1 MiB), and is cut after
  `stall_timeout` without one.
- The chat (`pages::chat`, fit's I118): text messages and files, a room's 64 latest kept, the
  hub's file bytes bounded (64 MiB, the oldest files dropped), downloads with a
  `Content-Disposition` as Go's `mime.FormatMediaType` writes it.
- Differences recorded in the reference: a file's body is read whole by the listener (within its
  `max_body_bytes`) before the member is checked, where fit reserves the bytes before reading;
  a reload does not drain the event streams, which go on under the new config. `libraries`,
  `vod` and `channels` are refused as not supported yet; `api/library` and `api/play` answer 404.
- Tested: `crates/akit-media/tests/pages.rs`, fit's `TestPages`, `TestLogin`, `TestParams`,
  `TestLoginWithoutUsers`, `TestRoom`, `TestRoomTTLAndLimits`, `TestWatcherCutAndSlow` (a watcher
  that reads nothing cut while another sees every member's last state), `TestReload`,
  `TestPagesConfigDefaults`, `TestPagesOff`, `TestChatText`, `TestChatBounds`, `TestChatOff`,
  `TestChatFile`, `TestChatFileTooLarge`, `TestChatHistoryCap` and `TestChatFileEviction` ported;
  the workers check's case. The gate takes fit-examples' `media-pages`; `video-site` is refused
  only for `channels` and `libraries`.

### 2026-11-27: I126, media auth on `whip` and `whep`

- akit-config reads `whip`'s and `whep`'s `auth` (a policy of the config, checked across files and
  handed to the handler as `httpflv`'s); `webrtc_proxy`'s `auth` and `action` stay refused as not
  supported yet until the role `webrtc.proxy` runs (I114).
- `akit_media::http::HandlerAuth`: a handler's policy, found once the engine is there and read per
  request without a lock, shared by `httpflv`, `hls`, `stream_whip`, `stream_whep`, `whip` and
  `whep`. A handler whose named policy is not there (its config not compiling) now refuses every
  request as unavailable (503) rather than letting it on unchecked, as the I123 handlers did.
- akit-webrtc's `whip` and `whep` (akit-webrtc now depends on akit-media): a POST, once its stream
  name is checked, goes through the guard (publish or play on the name) before the offer is read,
  as fit's; OPTIONS and a session's DELETE need no credentials.
- Tested: `crates/akit-webrtc/tests/auth.rs`, fit's `TestHandlerAuth` ported (with akio-webrtc
  `Endpoint`s, the roles `media_auth` and `webrtc.media` on one engine): 401 with the challenge
  and with `invalid_token`, 403, 201 by token, Basic, a bcrypt password (through the pool), a token
  akit signed in the query; a reload changes the policy in place and `public` opens play; a reload
  without `auth` opens the routes. akit-config's `handlers` (an `auth` naming no policy); the gate
  takes fit-examples' `media-pages` but for `media_pages` itself.

### 2026-11-27: I124, Streaming III: `streaming.webrtc`, `stream_whip` and `stream_whep`

- The role's WebRTC port: akio-webrtc's ICE-lite `RtcServer` on a UDP listener of the listeners'
  engine (under the role's `access`, set again by a reload), carrying akio-media's WHIP and WHEP
  sessions (`RtcMedia`, `WhipServer` at fit's `/whip/`, `/whep/` and `/rtc/session/`), with
  `max_sessions` (the server's sessions and peers), `connect_timeout`, `ice_servers` as `Link`
  header fields (fib's `linkHeaders`) and the host candidates of fib's `hostAddrs`: `public_ips`
  at the port, or, for a wildcard address, every interface's address that is up and is not
  loopback, link-local or multicast. A change of any of its fields starts it again, as fit's
  `rtcKey`; the role's `Current` hands it to the handlers. `akit run` no longer refuses
  `streaming.webrtc`.
- `stream_whip` and `stream_whep` (akit-config's `StreamRtc`: `stream`, `prefix` with `/whip/` and
  `/whep/` its defaults, `auth` checked against the policies): fit's `rtcHandler`, a POST through
  I121's guard (publish or play) then to the server under its own path, its answer's `Location`
  written back under the POST's path (a response filter); a session's PATCH, DELETE and OPTIONS
  without credentials (32 hex digits); an empty or oversized offer, or PATCH body, 400 by
  `max_offer_bytes`; 404 outside the prefix or without a stream; 503 without the role or without
  `streaming.webrtc`. The CORS headers of akio's server are off, as fit's.
- Stopgaps: `play_wait` is not acted on (a WHEP offer for a stream with no publisher is 404 at
  once, a warning logged when it is set) and a viewer whose publisher leaves is ended at once
  (akio A267); a port of 0 is found with a probe socket before the listener binds it (akio A268).
- Tested: `crates/akit-media/tests/rtc.rs`, akio-webrtc `Endpoint`s over loopback through akit's
  listener with the role on the same engine (fit's `TestStreamWHIPWHEP`, `TestHandlerPaths` and
  `TestNotRunning`): 401 with the challenge, 403 for a viewing token, 400 for an offer with no
  media, an empty one or one past the bound, 404 for a DELETE of no session; a WHEP viewer of an
  RTMP stream gets its media, its session under `/whep/live/cam/<id>` with its ETag, and a DELETE
  ends it; a WHIP publisher's H.264 plays over RTMP (pulled into a hub of the test's) and HLS, and
  a DELETE of its session ends the stream; an OPTIONS of the endpoint lists POST; 503 without
  `streaming.webrtc`. `crates/akit/tests/media.rs`: ffmpeg's WHIP muxer publishes to `akit run`
  and to fit's binary alike, refused without the token, played back by ffprobe over RTMP and HLS.
  akit-config's `rtc_handlers`, the workers check's two cases, and fit-examples' `streaming` valid
  in the gate.

### 2026-11-27: I123, Streaming II: `httpflv` and `hls`

- akit-config reads the two handlers' params as fit's (`prefix`, made to end with `/`, `auth`,
  `wait`, `cors`; HLS's `format`, `target_duration`, `part_duration` within 100ms to 5s,
  `playlist_size` at most 100, `max_bytes`, `play_wait`, `idle_timeout`), each `auth` checked
  against the config's policies across files and the policy's config handed to the handler.
- `akit_media::handlers` (in `STANDARD`, and `akit build`'s `akit-media`): a handler finds the
  role's slot and its policy once the engine is there (`EngineSlot::when_set`), compiling the
  policy from the config when the role `media_auth` has not applied it yet, and reads both per
  request without a lock. 404 for a path that names no stream (fit's `validStream`), 503 while
  the role does not run, every request through I121's guard; then akio-media's `HttpFlv` (HTTP-FLV
  and WebSocket-FLV) or `Hls`, made again when the role's hub is another (each hub numbered).
  `hls` keeps an fMP4 and an MPEG-TS `Hls`, the playlist's format by `?format=` and a segment's by
  its extension, shared by the handlers of the same params on one engine so that a reload keeps
  the segments; a playlist fetched with `?token=` has `token=` added to every URI it names (a
  response filter, fit's `withQuery`). With a prefix, the path is set without it before akio-media
  is called, which the access log then writes (akio A266, as one `Hls` for both formats).
- Tested: `crates/akit-media/tests/handlers.rs` through akit's listener with the role on the same
  engine: a stream published over RTMP plays over HTTP-FLV with a bearer token and `?token=`, 401
  with the challenge and the handler's CORS without, 404 for no stream; HLS fMP4 and MPEG-TS
  playlists hand the token on, their segments play with it and are refused without; 503 without
  the role. `crates/akit/tests/media.rs`: ffprobe plays HTTP-FLV and HLS (fMP4, MPEG-TS) of
  ffmpeg's RTMP stream through `akit run` and through fit's binary alike. fit-examples' `streaming`
  is refused only for `stream_whip` and `stream_whep` now.

### 2026-11-27: I122, Streaming I: the role's hub and its RTMP, RTSP and SRT listeners

- `akit_media::streaming`, the role `streaming` in `akit run` (after `media_auth` in `ROLES`):
  akio-media's hub with fit's limits (`max_streams`, `max_players`, `gop_cache_bytes`), and its
  listeners added to the listeners' engine (`Engine::add_listener`): RTMP (`RtmpServer`:
  `handshake_timeout`, `idle_timeout`, `play_wait`), RTSP (`RtspServer`, interleaved RTP, a SETUP
  for UDP answered 461: `session_timeout`, `play_wait`) and SRT (`SrtServer` on a UDP listener:
  `latency`, `passphrase`, `max_conns`, `peer_idle_timeout`, `play_wait`). `max_conns` of RTMP and
  RTSP by an acceptor of the role's own around akio-media's, which closes a connection past it as
  it opens and counts one down when its peer goes (`Conn::on_peer_gone`), as fit's `connLimit`.
- `auth` in the hub's hooks, on the I/O thread, the listener's policy read through the per-thread
  cache: RTMP's and RTSP's with the query's `token`, or `user` and `pass`; SRT's in the server's
  connect hook with `s=` (a token) or `u=` and `s=` (a user's password), or a plain stream ID's
  query, refused with 1401, 1403 or 1402 (fit's codes). A bcrypt user not remembered yet is
  refused once as a try again while the pool checks the password. The handlers' protocols pass
  the hook (they check before), and every publish let through is counted.
- Reloads as fit's: `auth` and `access` changed in place (`Engine::set_listener_filter`), a
  listener whose other fields changed started again (the old one first when the two may share an
  address, the old one started again when the new one fails), the hub and every listener started
  again when the hub's limits changed (`akit_streaming_restarts_total`). The role's metrics are on
  `/metrics` (`akit_streaming_streams{state}`, `_players`, `_published_total`,
  `_auth_refused_total{protocol,reason}`, `_restarts_total`), through `Role::metrics` (the roles' hook the WebRTC track added).
- Stopgaps, each with an akio request: RTMP's refusals are akio-media's statuses
  (`NetStream.Publish.Denied`, `NetStream.Play.StreamNotFound`) and its handshake deadline ends at
  C2 (A263). RTSP takes credentials in the URL's query alone, a refusal 403 without challenges,
  and two more gaps showed under ffmpeg (A264, extended): akio-media's `Content-Base` leaves the
  query out, so a play's SETUP and PLAY carry no credentials (a play needs the policy's public
  play), and ffmpeg's SETUP after an ANNOUNCE with a query names the track after it
  (`.../cam?token=T/streamid=0`), answered 404, so ffmpeg cannot publish with a query token (fit
  takes both; akio's own RTSP client publishes). SRT's `r=live/cam?token=T` is refused with 1400
  rather than published under a path holding the query (A265). `streaming.webrtc` is refused by
  `akit run` until I124.
- Tested (`crates/akit-media/tests/streaming.rs`, on akio-media's own clients, every wait on the
  hub's or a client's state with a deadline, ten runs in a row clean): RTMP publish refused without
  credentials and for a stream the token may not publish, let in with a token and with a user's
  query password, played back; RTSP publish with a query token and refused without, play refused
  without public play and let in with it; SRT's 1401, 1403, 1400 and 1404, a publish with a token
  and with a user, a plain stream ID's play; `max_conns`; the access refusing and a reload allowing
  the next connection; the hub's limits changed ending the stream; `auth` changed in place keeping
  the publisher connected. `crates/akit/tests/media.rs`: ffmpeg publishes a test pattern over RTMP
  (refused without the token) and over RTSP and SRT (where its build has libsrt), ffprobe plays it
  over RTMP, RTSP and SRT; the same commands against fit's binary (`AKIT_FIT_BIN`, built from fit's
  source at `b6c5cebf`) give the same answers but for ffmpeg's RTSP publish with a query token
  (above); `/metrics` lists the role's metrics.

### 2026-11-27: I121, media auth II: the policies, the checks, `akit media-token`

- fit's `mediaauth` in a new crate, `akit-media` (decision 040): static tokens looked up by their
  SHA-256, tokens akit signs in fit's format (`fm1.`, the claims in base64url, an HMAC-SHA256 under
  a key derived from the secret and the policy's name with fit's label), users with a plaintext
  password compared in constant time, or a bcrypt hash checked on a bounded pool of the crate's own
  (a quarter of the CPUs, 256 waiting, 5 s, as fit's; a password found right remembered as an HMAC
  under the policy's random key; an unknown user checked against a decoy hash, so that a refusal
  takes as long), public actions, `*` patterns matched as fit's.
- The policies by engine and name (`auth::registry`), one object across reloads: the role
  `media_auth` (`akit_media::role::MEDIA_AUTH`, first in `ROLES`) applies every config, compiling
  all its policies before it changes any, and marks those left out removed (503 for whoever holds
  one); a module provisioned first compiles its policy from the config being compiled, as fit's
  `Provision`. `secret_file` is read at each apply and reported by `Role::files`, so `-watch-certs`
  applies the config again when it changes. The state is read through a per-thread cache of what
  the control thread swapped (`swap::Swap`), so a check takes no lock and no shared count.
- HTTP (`akit_media::http`): the credentials of a request (`Bearer`, `Basic`, `?token=`, the cookie
  `akit_media_token`), every `token` and `sig` of the query written `REDACTED` in the target for the
  access log, the guard (401 with `Bearer realm="akit"`, `error="invalid_token"` for bad
  credentials, 403, 503 with `Retry-After`; a bcrypt check with the request suspended and resumed
  on the pool), the access a playlist hands on.
- `akit media-token` (`-c`, `-policy`, `-publish`, `-play`, `-ttl`, `-subject`), as fit's.
- Tested: fit's `TestMatch`, `TestAllow`, `TestPublic`, `TestBcrypt`, `TestIssue`, `TestRegistry`,
  the secret file and `TestGuard` ported (`crates/akit-media/src/auth/tests.rs`, `tests/guard.rs`,
  over akio HTTP); a token fit's binary signed (`fit media-token`, fit built from source at
  `b6c5cebf`) is taken, and akit signs the same claims to the same bytes; `akit media-token`'s
  token is taken and its usage errors are fit's (`crates/akit/tests/media.rs`).
- Not done here: fit's `Login` for the media pages (I125), the keyring and rotation (fit's I119,
  the media library's track), signed playback URLs.

### 2026-11-27: I120, Streaming and media auth I: `media_auth` and `streaming` in the config

- The track starts: fit's I115–I117 broken into I120–I126 in the plan (decision 040), from
  `config/mediaauth.go`, `config/streaming.go`, `mediaauth`, `streaming` and `handler/mediapages`.
- `akit-config`'s `media` module reads both members as fit's, with fit's checks: names, tokens and
  their SHA-256, users and bcrypt hashes, secrets of at least 16 bytes, stream patterns, the
  listeners' addresses (in the one address, one server check, `streaming.rtmp listens on TCP :1935
  already`), the SRT passphrase's length, `public_ips`, `ice_servers` and their `Link` safety,
  `max_offer_bytes` at most 64 KiB, and the policies the listeners name, checked once every file is
  read. RTSP's `realm` default is akit's when it comes (decision 037).
- Refused as not supported yet: `keys`, `url_signing`, `channels`, `push` (later tracks), `mptcp`,
  RTMP's `max_message_bytes` (akio A263: akio-media's RTMP server has a fixed 16 MiB bound) and
  RTSP's `auth_methods`, `realm` and `idle_timeout` (akio A264: no request hook for Digest and
  Basic, no sessionless idle bound). `akit run` refuses `media_auth` until I121 and `streaming`
  until I122 runs it; `akit validate` checks both.
- The gate: fit-examples' `streaming` is refused for its handlers only; `bittorrent`, `media-pages`
  and `video-site` no longer for `media_auth`. The reference has the two members' tables, the v1
  freeze marks 59 fields as additions.
- Tested: fit's `TestMediaAuth` and `TestStreaming` ported (`crates/akit-config/src/media/tests.rs`),
  the clashes of the role's addresses, the samples of every field read (`tests/compat`).

### 2026-11-27: I139, users, roles and the audit log

- Roles (`akit_cluster::roles`, fit's `cluster/roles.go`): viewer, operator and admin,
  cluster-wide or on groups (64 a user, 256 users), a member's role the highest of the cluster's
  and its groups', a default role for console users not named, `admin.cluster.admins`
  cluster-wide admins whatever they say; open (every user an admin) until roles are set. Kept as
  the document's `access`, fit's, which no longer leaves the document read-only (the document is
  written as version 3); a group renamed or removed followed.
- Every endpoint checked as fit's: a read needs a role somewhere, a change a cluster-wide admin,
  but for those that check what they touch: a member added, changed, removed or tested as a draft
  (an admin of it), tested as kept (an operator), read (a viewer); the switcher (a viewer, an
  admin to change); operations (an operator of each member); syncs, desired configs, imports and
  comparisons (a viewer); rollouts (an operator of a template's or a sync's members, an admin of a
  copy's and a patch's, a viewer of what a copy copies; a template's draft a cluster-wide admin's);
  silences (an operator of their member or their rule's group). A refusal is 403 with the role it
  needs and where (fit's `ErrForbidden`). The name `Admin API` is refused as a console user.
- The manager's own console, on a manager with roles: reads, the cluster module's endpoints, the
  switcher, password changes, the text tools and dry runs for anyone; the rest a cluster-wide
  admin's, through `Module::console_gate`, which `akit-admin`'s console asks of the cluster module
  (fit's `ConsoleGate`).
- `GET /api/me` (the user's roles on the cluster, each group and each member), `GET`/`PUT
  /api/access` (every console user, the admins, the groups; `If-Match`; at least one cluster-wide
  admin left), in the audit log as `access.update`. The audit log's filters and JSON lines were
  there since I132.
- Tests: `roles::tests` (roles resolved, checked, kept, renamed, refusals described); akit's
  `roles_and_audit` (open, then roles set, refused without an admin; a viewer and a group's
  operator: groups, operations, reads, the switcher, members, silences and the manager's own
  console allowed or refused; a group renamed; kept across a restart; the audit log as JSON lines).

### 2026-11-27: I138, drift and alerts; the consistency check on akit

- The consistency check on every akit (`akit_admin::check`, fit's `admin/drift.go`): `GET
  /config/check` answers the differences between what runs and the config it runs from, each a
  drift with its fixes or an info: the settings that take a restart (those a config's warnings
  name, against version 1, kept beside the history), listeners bound or not and dynamic ports,
  the certificates asked for and served, upstreams running or not and with every backend down,
  roles set and running, cache zones used and kept, what providers provided and left out. What
  runs is read from `GET /status`'s and `GET /node`'s writers; values are answered redacted.
  `POST /config/reconcile` takes `If-Match` the check's ETag (428 without), checks again and
  applies the fixes named (the deletes last, from the end of their arrays) as a new version. The
  capability `config.check`, so a manager's polls read the drift and a rollout's drift gate
  applies to akit members; the console's Consistency page works. v1 additions.
- Alerts (`akit_cluster::alerts`, `alerteval`, fit's `cluster/alerts.go`, `alerteval.go` and
  `alertapi.go`): fit's six default rules (a member down, drift by templates or by the check, a
  rollout failed, a certificate near its expiry, backends down, reloads failing), a group's or
  every member's, each pending for its time, then firing, sent again after its repeat, resolved
  (and sent so, to webhooks that ask) once its condition is gone; silences by rule, member or
  both; 64 rules, 16 webhooks, 256 silences, 2,048 alerts followed and 500 resolved kept. Kept in
  the store's section `cluster.alerts`, fit's document. The rules are evaluated on a thread of
  the manager's when a poll ends, a rule, webhook or silence changes, a member's templates were
  read or a rollout ended, and at the earliest of their own times; members a template manages are
  read against their templates when their running config changed, once a poll.
- Webhooks (`akit_cluster::webhooks`, fit's `webhooks.go`): JSON, Slack, Discord, Feishu,
  DingTalk, WeCom and custom templates; https, or http to loopback; signed with the secret
  (`X-Akit-Signature`, `X-Akit-Timestamp`; Feishu's and DingTalk's own); one sender each on akio's
  client, 64 notifications waiting, each within its timeout; the URL's path and query and the
  secret never answered or logged; a test answered with how it went.
- Drift (`GET /api/drift`, `POST /api/drift/fix`, fit's `drift.go`): each member's templates'
  and check's drift; a fix plans a sync rollout of the members named.
- Endpoints: `/api/alerts`, `/summary`, `/config` (with its ETag; `If-Match` on the changes),
  `/rules`, `/webhooks` (and `/<name>/test`), `/silences`; the changes on the manager's pool, in
  the audit log (`alert.*`).
- Tests: `check::tests`, `alerts::tests`, `alerteval::tests` (the life of an alert, its time,
  each kind's finding), `webhooks::tests` (checks, rendering, signatures, chat services' refusals);
  akit's `consistency_check` (a drift, its fix reconciled, 428, a fix not offered) and
  `alerts_and_drift` (the default rules, a webhook tested and its signature checked, a member down
  firing and sent, silenced and the silence ended, a template's drift fixed by a rollout, a check's
  drift firing a rule, the alerts kept across a restart).

### 2026-11-27: I137, rollouts with health gates

- Rollouts (`akit_cluster::rollouts` and `rollrun`, fit's `cluster/rollouts.go`, `rollrun.go`,
  `rollsteer.go`, `rolldraft.go` and `rollapi.go`): of a template, or of a draft of it (saved as
  the template when the rollout starts, refused if the template changed since the plan); a sync
  with the templates; a copy of parts of one member's config to others; a patch of parts (no
  variables). The plan reads each member: held ones, and those that do not list
  `config.if_match`, `config.dry_run` and `config.versions`, are not ready; its desired config, the
  parts that change with their sync states, and its dry run with `If-Match` the ETag read make it
  ready, unchanged or an error. 8 MiB of configs and diffs a rollout; a plan expires after an hour.
- Strategies: all at once, rolling (by size or percentage), or a canary that then waits for a
  person's go; a pause between batches (24 h at most); a failed gate rolls back its batch, or every
  member applied. Each member is applied with `If-Match` the ETag planned; a 412 or a lost answer is
  settled by its `GET /node`'s ETag (applied, skipped as changed since the plan, or unsure). The
  gate, after `wait_ms` or the 5xx window: the member reachable and running the new ETag; no more
  drift than before, where it lists `config.check`; its share of 5xx over the window from
  `/status`'s `requests`, judged from `min_requests`, the window starting once the requests
  counted so far are read, when the batch says `gating` (members that count no requests fail the
  batch before anything is applied). A rollback is `POST /rollback/{n}` with `If-Match` the new
  ETag; the members of a batch that passed are told what the manager owns.
- Steering: pause (after the batch under way), resume, go, abort, rollback (once paused, waiting
  or ended) and discard (a plan); each in the audit log (`rollout.plan`, `rollout.start`,
  `rollout.<action>`, `rollout.end` with every member's outcome). A rollout holds its members
  against operations and other rollouts.
- Kept typed in the store's document, `rollouts`, as fit writes them (a member's config in base64,
  as Go writes `[]byte`), with every write of the document, one write queued at a time. At a start,
  a rollout that ran is paused where it stopped, one being planned expired, one rolling back
  failed. Bounds: 4 planned or under way, 50 kept, 20 errors a member.
- Threads: a plan and a run each take a thread of their own, which calls members on akio's client
  and waits for their answers, `max_concurrency` at once on scoped threads; the pause between
  batches and a gate's window are waited for on a condition variable that a pause, an abort and
  the manager's close wake (the close waits 2 s for them). Endpoints: `GET`/`POST /api/rollouts`,
  `GET`/`DELETE /api/rollouts/<id>`, `POST /api/rollouts/<id>/{start,pause,resume,go,abort,
  rollback}`; those that write, on the manager's pool.
- Mixed clusters: a copy from an akit member masks its `[redacted]` secrets so that each target
  keeps its own (decision 041, 9); fit's manager reads the rollouts akit's wrote.
- Tests: `rollouts::tests` (strategies, fit's `TestStrategies`; times and numbers as Go writes
  them; the stored form read back; a restart's transitions; expiry and trimming; parts and
  secrets) and `rollrun::tests` (changes, bodies, requests, secrets); akit's `rollouts_with_gates`
  (rolling batches; a plan unchanged, then discarded; a canary, go, pause and resume; a member
  changed since the plan skipped; a rollback; a 5xx gate that cannot be judged and one that fails
  and rolls back; a rollout under way at a restart found paused and holding its members, then
  aborted; a draft saved as the template; a patch) and `mixed_clusters_with_fit` (a template rolled
  out on fit members, a copy from fit to akit, the rollouts read by fit's manager).

### 2026-11-27: I136, templates and what a member knows the manager owns

- Templates (`akit_cluster::templates`, fit's `cluster/templates.go`): a fragment of the config,
  its top-level members owned whole (but `admin` and `schema_version`) and the elements of
  `listeners`, `routes`, `services` and `upstreams` owned by name (named, each once, names without
  variables); variables `${node.name}`, `${node.id}`, `${node.hostname}`, `${node.labels.<key>}`
  and a group's, `$${` a `${` as written; attached to groups (32), an order (−1,000 to 1,000), 64
  KiB and 256 parts at most, 128 templates. Secrets (fit's rule) answered as `[secret]` and kept
  when sent back so; paths to files warned of. Typed in the store's document, as fit writes them;
  a group renamed or removed is followed. Endpoints: `GET`/`POST /api/templates` (with `from`, made
  from parts of a member's config read now), `GET`/`PUT`/`DELETE /api/templates/<name>`.
- Each member against its templates (`akit_cluster::sync`, fit's `render.go` and `sync.go`): the
  parts its templates render, a later template winning a part (`overrides`); the desired config,
  its own with those parts in their places, a new element at the end; each part in sync, out of
  sync (`missing`, `template_changed`) or a conflict (`own_value`, `changed_here`,
  `removed_here`), against the hash the member keeps of the part as last seen in sync; parts no
  template owns now released. `POST /api/sync` (`nodes`, `template`, or every member; 4 at once;
  those not read in 30 s pending), `GET /api/sync` (summaries, no values), `GET
  /api/desired/<name>`, `POST /api/import` (a member's parts, for a template), `POST
  /api/compare` (two members part by part), `POST /api/text/render` and `/parse` (a template's
  config as `.conf`, YAML or JSON, through `akit_config::adapt`). Members are read
  `max_concurrency` at once, each answer starting the next read.
- Members told what the manager owns there (`PUT /node/managed`, `If-Match`, once more after a
  412), after every change of members, groups or templates, one refresh at a time, a change
  meanwhile running another after it (fit's delay of half a second is not taken: no timer that is
  not about time); with a sync from the pages too.
- On every akit (`akit_admin::managed`, fit's `admin/managed.go`): `GET`, `PUT` and `DELETE
  /node/managed`, the capability `node.managed`; the record (the manager's node ID and host, each
  part with its template and hash) checked as fit's, kept in the store's section `managed` (fit's)
  or in memory without a store; each part said present and changed or not against the config as the
  API answers it. Marked as v1 additions.
- Mixed clusters: hashes are fit's (`managed.Hash`: canonical JSON, keys sorted, numbers as
  written, Go's escaping), checked against Go's own output; an akit member answers its secrets
  redacted (decision 035), so a part is compared and its hash kept redacted on both sides for an
  akit member (decision 041, 8), whole for a fit member.
- Tests: `templates::tests` (variables, configs, secrets, checks), `sync::tests` (rendering by order
  with variables, every sync state, the record, the desired config, redacted secrets, fragments),
  `jv::tests` (Go's hashes), `managed::tests`; akit's `templates_and_sync` (a route by template on
  two members, told, brought in sync, changed into a conflict and marked changed on the member,
  `from`, import, compare, the text tabs, secrets, a group renamed, a template removed and the
  members owning nothing) and `mixed_clusters_with_fit` (fit members compared and told).

### 2026-11-27: I135, groups and operations on many

- Groups (`akit_cluster::groups`, fit's `cluster/groups.go`): a label selector (Kubernetes'
  equality terms, `key=value`, `key==value`, `key!=value`, `key`, `!key`; written back as fit
  writes it; 1 KiB and 32 terms at most; the empty selector selects no one), members by name, or
  both; a description, and variables for I136's templates (64, names of letters, digits and `_`,
  values of 4 KiB). Membership is computed from the labels as they are, never stored: a label
  changed moves a member in or out; a name with no member is kept and shown missing; a member
  renamed or removed is renamed in, or taken out of, the groups naming it; a group renamed or
  removed is renamed in, or taken out of, the templates fit kept. Groups are typed in the store's
  document now, what akit does not read of one kept. Endpoints: `GET`/`POST /api/groups`,
  `GET`/`PUT`/`DELETE /api/groups/<name>` (a group with its members' states), `POST /api/select`
  (a form's preview); 256 groups. Each member's state lists its groups, and `/api/me` the groups.
- Operations (`akit_cluster::ops`, fit's `cluster/ops.go`): `POST /api/ops` (202) on a group, a
  selection, or members of a group: `reload` (`POST /reload`), `rollback` (to the latest version
  kept whose ETag differs from the running one's, with `If-Match`; `no_earlier_config` without
  one), `check` (`GET /config/check`), `purge` (by key, prefix, tag or all, checked as fit's: 64
  absolute URLs or tags a list, 2 KiB an item) and `test` (`GET /node`); each member's capabilities
  checked first (`unsupported`), its refusal read as fit's `replyProblem` (`refused`, the first of a
  config's errors); `GET /api/ops` and `/api/ops/<id>` follow them. At most `max_concurrency`
  members at once, each answer starting the next member's call (no thread of its own), 4
  operations at once (429), one on a member (409), 32 kept. An operation ended is in the audit log
  (`op.<name>`, each member's outcome, its config versions before and after, `ok`, `partial` or
  `failed`) without changing the members' ETag; its members are polled again at once.
- Tests: `groups::tests` (fit's selectors and checks), `ops::tests` (fit's purge checks,
  refusals); akit's `groups_and_operations` (a group by selector and by name with a name missing,
  the preview, a label changed, a reload on a group with a member down, a rollback with none to go
  to and one done after a change through the switcher, purge, check, test, the bad requests, the
  audit, a member and a group removed). Checked in a browser: the Groups page, and a member's
  Overview through the node switcher.

### 2026-11-27: I134, a member's own pages opened from the manager

- The node switcher's pass-through (`akit_cluster::pass`, fit's `cluster/passthrough.go`): the
  console serves `/api/n/<member>/workbench`, `/node` and `/m/<module>/<path>` through the module
  `cluster` (the module's own `/api/m/cluster/n/...` answers 404), passed on to the member through
  its console with the manager's session or its Admin API with its token. Not passed on: the
  member's users and account (403), its debug and dashboard (404), the modules `cluster`, `ai` and
  `llm.*` (403), a role's module of a member reached by token (404), streams (501), path segments
  `.`, `..`, empty or with `\` or NUL (404). Bounds as fit's: 3 calls a member and 16 waiting (503
  past the call timeout), 64 in all (429), a body within `max_response_bytes` (the console takes
  it: `Options::cluster_max_body`).
- Request headers passed on: `Content-Type`, `If-Match`, `If-None-Match`, `Upload-Offset`,
  `Upload-Length`, `Akit-Store-Password` (also as fit's `Fit-Store-Password`); answer headers
  passed back: `ETag`, `Akit-Config-Version` and `Akit-Dry-Run` (a fit member's `Fit-Config-Version`
  and `Fit-Dry-Run` under akit's names, decision 041), `Retry-After`, `Allow`, `Upload-Offset`,
  `Upload-Length`, an attachment's `Content-Disposition`; a content type a browser would render
  becomes `application/octet-stream`. A member's 401 and its 403 asking for a new password become
  502 with the problem.
- Writes (all but GET, HEAD, the text conversions and dry runs) are refused while the member's
  schema digest, as last polled, is not the manager's (`Options::schema`, the console's) or the
  manager's members are read-only (409); otherwise audited (`node.api`, the outcome, the config
  versions before and after, the SHA-256 of the body, never the body), 64 waiting at most (429).
  `/workbench` reads the member's `/node` now, polls it again when its schema changed, and answers
  the member with `read_only` and why (`not_read`, `version`, `manager`), and, for a console member,
  its modules but those not passed on. A member with no `GET /schema` is drawn with the manager's
  (`Akit-Schema-From: manager`). fit members open read-only: their schema is fit's.
- Tests: `pass::tests` (fit's routes and writes, fit's headers under akit's names); akit's
  `node_switcher` (by console and by token: a route changed with `If-Match`, the stale ETag 412, a
  dry run, what is not passed on, the audit), and `mixed_clusters_with_fit` (fit members read-only,
  `Fit-Config-Version` and `Fit-Dry-Run` translated, a write refused).

### 2026-11-27: akio pinned at `d8d51f61`: A256-A261

- akio: `d8d51f61` on akio's main, with A256-A258 in akio-turn (ADR-186: a relay port range, the
  nonce secret, the stream message bound; `on_relay`, `on_event`, `holds_relay` and more counters;
  limits and credentials set on a running server, a drain) and A259-A261 in akio-webrtc and
  akio-rtp (ADR-187: `accept_offer_with`, `Peer::write_rtp`, `set_receive_bitrate_cap`,
  `set_ice_timeouts`). akit's `rev` and `akit_runtime::AKIO_REV` move with it; akit builds unchanged.
  I112 and I113 take them up.

### 2026-11-27: the WebRTC track stops after I111; I113's akio requests

- The track stops after I111 in this run: I112 waits on akio's A256-A258; I113 (WHIP and WHEP, the
  SFU on akio-webrtc's `RtcServer`) and I114 (the proxy) are not started. Reading akio-webrtc at
  `8a9601b9` for I113 found three gaps, written as akio requests with planned stopgaps, so that
  I113 is not blocked: A259, a peer's own configuration on an `RtcServer` (the codecs a session is
  answered with; stopgap: the configuration closure reading the session's codecs set on the
  calling thread); A260, RTP forwarding for an SFU (stopgap: frames forwarded, `on_media_data` to
  `write_media`); A261, REMB sent to a publisher and the ICE consent timeout per peer (stopgap:
  `max_bitrate` enforced by dropping, not told; akio's consent timeout). Decision row 53.

### 2026-11-27: I111, the TURN server

- `akit_webrtc::turn`, the role `webrtc.turn` that `akit run` runs (`akit::ROLES`): akio-turn's
  `TurnServer` with `SOFTWARE` `akit`, a UDP listener of the engine for each `listen` address, a
  TCP one for each of `listen_tcp` and a TLS one for each of `listen_tls` (akio-turn's `stream()`
  under akio-tls; the certificates loaded by akit-tls and chosen per handshake from the last
  apply's, reported through `Role::files` for `-watch-certs`); TCP allocations refused with 442, as
  fit's. The users' keys (a password, or a key in hex) and the secret's time-limited credentials
  (akio-turn's `ephemeral_auth`) are read through akit's `AuthFn` in the realm; the peer rules
  through its peer filter, port-aware as fit's `peerAllowed`: `allowed_peers` first, then
  `denied_peers` and akio-turn's denial of internal addresses. Every listener's accept filter is
  the member's access, then the global one, set again by every apply.
- A reload changes the users, secret, peer rules, access and certificates in place; any other
  change (addresses, realm, relay addresses, quotas, caps, lifetimes, nonce secret, stream bounds,
  Multipath TCP) starts a new server: on the same address after closing the old one (started
  again when the new one cannot start), elsewhere beside the old one, which keeps serving for
  `-shutdown-timeout` and is then closed by a timer.
- Stopgaps until akio's A256-A258 (decision 039, row 51): `port_min` and `port_max` refused at run
  time; `user_bandwidth`, `nonce_secret`, `max_stream_message` and `mptcp` said when the server
  starts and not applied; the server's own addresses reached on the ports `allowed_peers` names
  alone; the quotas, caps and lifetimes start a new server; the replaced server takes new clients
  while it drains. akio-turn's relayed sockets are bound on the engine's UDP client threads: one,
  when the config has no UDP listener (a benchmark task should measure it, the benchmark table's
  row).
- `/metrics` gains the running roles' metrics (`Role::metrics`, the host's `Meters`, written after
  the rest): `akit_turn_allocations`, `akit_turn_draining_servers`,
  `akit_turn_relayed_datagrams_total{direction}`, `akit_turn_auth_failures_total` and
  `akit_turn_restarts_total`. The OTLP export does not carry the roles' metrics yet.
- Tests (`crates/akit-webrtc/tests/turn.rs`, fit's `turn_test.go` ported, a client on
  akio-turn's codec over UDP, TCP and TLS): relay by indication and channel with a password, a
  key and the secret's credentials; refused credentials (wrong password, unknown user, expired,
  another secret); quotas 486 and 508; caps 508; private peers and the rules changed in place;
  the own address; reload in place, restart on the same address, drain on another; a refused
  reload keeping the server (a port range, an address in use); TCP and TLS; access; akio-turn's
  own client (`client::Client`) by indication and by channel. `tests/fib_turn.rs` (ignored without
  `AKIT_FIB_TURN`): fib's TURN client, fit's, from `tests/interop/fib-turn` (a Go program on fib's
  `webrtc/turn`), over UDP, TCP and TLS with a password, a key and the secret, by indication and
  channel, and refused credentials; with `AKIT_FIT_BIN`, fit's own server answers the same
  scenarios alike (run on macOS against fit built from `b6c5cebf`).
- A restart on the same address may lose a datagram sent while akio closes the old UDP listener's
  sockets (on its threads' next turn; seen once on io_uring, gpu51): clients retransmit (RFC 8489),
  and the tests' client does too.

### 2026-11-27: I110, `webrtc` in the config

- `akit-config`'s `webrtc` module: `webrtc.turn`, `webrtc.media` and `webrtc.proxy` with fit's
  fields, defaults, bounds and checks (`config/webrtc.go`): listen lists checked and joined to the
  one-address-one-server check (UDP for `listen`, `webrtc.media` and `webrtc.proxy`, TCP for
  `listen_tcp` and `listen_tls`), `tls` only with `listen_tls`, users with a password or a key of
  32 hex digits, `relay_ip` and `public_ip` (no wildcard, one family), port ranges, quotas and
  lifetimes (`max_lifetime` not below `default_lifetime`), `max_stream_message` at least 2048,
  peer rules as fit's `ParsePeerRule` (a prefix, an address, an address and a port), ICE servers
  and `turn_urls` safe in a `Link` header (`turn_urls` needing `webrtc.turn`'s secret), `codecs`
  among fit's five. The handlers `whip`, `whep` (`stream`, 1 to 128 of `[A-Za-z0-9._~-]`) and
  `webrtc_proxy` (`upstream`, `media_addrs` as an address or an IP, `timeout`, `max_sdp_bytes`,
  `action`) are `Handler` variants; their `auth`, a `media_auth` policy, is refused as not
  supported yet, naming the module (decision 039, row 50).
- A new crate, `akit-webrtc`, one of the standard module crates (`akit build -without` takes it):
  the three handlers, provisioned as fit's (`webrtc_proxy`'s upstream looked up), answering 503 as
  fit's do while their role does not run. `akit run` refuses the three members and the routes with
  those handlers, naming the iteration that runs each; `akit validate` checks them.
- The gate: `webrtc-turn`, `whip-whep` and `webrtc-proxy` valid; `media-pages` refused only for its
  `auth` and `media_auth` (59 valid of 80); 89 fields marked supported and as v1 additions; the
  reference's WebRTC chapter and the three handlers' tables. Tests: fit's `config/webrtc_test.go`
  (members, media, proxy), the WebRTC cases of `bindings_test.go` and `more_test.go`, and the
  handlers' params, in `webrtc/tests.rs`; `binds.rs`'s every-kind clash test with the WebRTC kinds;
  `akit run`'s refusals.

### 2026-11-27: the WebRTC track planned as I110–I114

- The WebRTC track (after 1.0, order 5) broken into five iterations from fit's I87 and I108–I111
  ([the plan](dev-plan.md#webrtc), [decision 039](decisions/039-webrtc.md)): I110, `webrtc` in the
  config (`webrtc.turn`, `webrtc.media`, `webrtc.proxy`, the handlers `whip`, `whep` and
  `webrtc_proxy`); I111, the TURN server on akio-turn over UDP, TCP and TLS; I112, the TURN
  server on the hooks akio-turn lacks (A256–A258); I113, WHIP and WHEP, an SFU on akio-webrtc's
  `RtcServer`; I114, WHIP and WHEP proxied. I115–I119 are reserved for the track. Its benchmark
  row names what to measure.

### 2026-11-27: I151, the BitTorrent tracker

- `bittorrent.tracker` (`akit_bittorrent::tracker`, a new crate, [decision 042](decisions/042-bittorrent.md))
  run by `akit run` on akio-bittorrent's `TrackerServer` as its store: HTTP announces and scrapes
  through the routes' `bt_tracker` handler (now read by akit-config and found in the registry;
  503 while the role does not run), UDP (BEP 15) on its `listen`, a binding on the listeners'
  engine with the role's access at akio's filter, moved by a reload (a port taken leaves the one
  before). The answers are fib's (decision row 78): the query read as fib reads it with its
  failure reasons, `unregistered torrent`, `tracker is full`, `min interval`, `external ip`,
  `peers6` and the compact or dictionary peer lists, 50 peers by default and `max_numwant` at
  most, a UDP answer within 1,400 bytes, `max_scrape` and 74 for UDP, connection IDs an HMAC of
  the minute and the address under a secret kept across reloads, a forged ID dropped. The
  allow-list inline and in files (each read on the control thread, watched as `-watch-certs`
  watches certificates, a bad line refusing the reload); HTTP refused by the role's access with
  the failure `refused`, counted; statistics (announces, scrapes, refusals by reason); the
  console's `/api/stats`, `/api/torrents` (the allow-list's), `/api/torrents/<hash>`.
- A request reads the role's snapshot from a board keyed by the engine, through a cache of each
  thread's refreshed when an apply changes it: no lock or shared reference count per request.
- Stopgaps until akio's A285: akit's own HTTP and UDP sides (row 78); new bounds make a new store
  (row 77); no peer count, and a public tracker's torrents not listed (row 79).
- `akit-bittorrent` joins `STANDARD` (`akit build -without akit-bittorrent` leaves it out).
- Tests: `tracker::tests` (fit's tracker tests: announces and scrapes over HTTP and UDP with
  akio-bittorrent's tracker client, bad queries and bounds, the store's bounds, the allow-list
  and its files, access, UDP written by hand (junk, forged IDs, the 1,400-byte answer, 74
  hashes), reloads, a byte-at-a-time client, cut clients, the role stopped, the console's routes,
  two akio-bittorrent clients finding each other through it); akit's `tests/bittorrent.rs`: the
  tracker under `akit run`, and with fit's binary (`AKIT_FIT_BIN`, built from fit's source) two of
  fit's clients finding each other only through akit's tracker, one over HTTP and one over UDP,
  the file downloaded whole.
- Decisions waiting for review: rows 77–79.

### 2026-11-27: I150, `bittorrent` in the config

- `bittorrent.client` and `bittorrent.tracker` read as strictly as the rest
  (`akit_config::bittorrent`), with fit's fields, defaults, bounds and checks
  (`config/bittorrent.go`, `config/bittorrent_tracker.go`): the client's `listen` and `data_dir`
  (required, `data_dir` not blank), `download_dir` (`<data_dir>/downloads` by default),
  `encryption` (`prefer`), `dht`, `pex`, `utp` (on), `lsd` (off), `disk_workers` (at most 64),
  `save_interval` (5 s), `limits` (rates, `max_conns_per_torrent` 50, `upload_slots` 4,
  `seed_ratio`, `seed_time`, `max_torrents` 1,000, at most 100,000) and `console`; the tracker's
  `listen`, `interval` (30 m), `min_interval` (not past the interval), `peer_ttl` (1 h, longer than
  the interval), `max_torrents`, `max_peers_per_torrent`, `max_numwant`, `max_scrape` (fit's
  defaults and maxima), `allow` (info hashes of 40 or 64 hex digits, at most 1,048,576),
  `allow_files`, `access` and `console`. The client's port in the one address check for TCP and
  UDP, the tracker's for UDP, their consoles for TCP.
- Refused as not supported yet, each by its iteration or akio request: the client's `categories`,
  `watch_dir`, `watch_interval`, `watch_max_files`, `url_timeout` (I153), `alt_speed` (I154),
  `access`, `disk_queue_bytes` and the limits `max_conns`, `max_half_open` (akio's A284),
  `max_active_downloads`, `max_active_seeds`, `min_free_bytes` (I153), `publish` (I156), `rss`
  (I159); the tracker's `max_peers` (A285), `auth` (a `media_auth` policy) and `store` (I159).
  `akit run` refuses a config with either role until I151 and I152 run them.
- akio requests A283–A288 filed: the client on the caller's engine, peers refused by address and
  limits changed at run time, the tracker store's bounds in place, a magnet link's storage
  directory, a reader that waits for pieces, trackers' state and a recheck in place.
- Tests: `bittorrent::tests` (fit's `TestConfigChecks`, `TestBitTorrentTrackerChecks` and
  `TestBitTorrentTrackerBinds`, the fields refused as not supported yet); the gate samples every
  field read; akit's `roles_are_refused_until_they_run`. The gate's example `bittorrent` is still
  refused (its `media_auth`, `media_library`, the `bt_*` handlers and the fields above).
- Decisions waiting for review: row 76.

### 2026-11-27: the BitTorrent track planned (I150–I159)

- The tenth track after 1.0 broken into iterations from fit's I196–I210
  ([the plan](dev-plan.md#bittorrent)): the roles in the config (I150), the tracker (I151) and the
  client (I152) on akio-bittorrent, adding torrents (I153), schedules and access (I154),
  `bt_stream` (I155), publishing (I156), the Admin API and events (I157), the pages (I158), RSS
  and the shared tracker store (I159). fit's I205 (interop and failure tests) goes into each
  role's iteration; fit's I206 (the cluster's view) waits for the Cluster management track. A row
  in the benchmark table names what to measure.

### 2026-11-27: I132 and I133, the cluster manager, its members and the Cluster pages

- `akit-cluster` (new, fit's package `cluster`, its I141 and I142): the manager `admin.cluster`
  turns on, which `akit run` starts on the listeners' engine with its members in the store's
  section `cluster` (`admin.store` or the store `-admin` keeps; none refuses to start) and closes
  before the engine, logging the members' consoles out (2 s at most). The store's document is
  fit's (`member::Document`): versions 1 to 3 read, version 2 written, every list present, the
  parts akit does not use yet (groups, templates, rollouts, `link_cert`, a member's `link`) kept as
  they were read; a newer version, or one that sets users' roles, is read-only (decision row 73),
  and one that is not the manager's document is left alone. Members checked as fit's
  `Member.check` (names, labels as Kubernetes', `https` or loopback `http`, console or Admin API
  access and their credentials, `ca` or `fingerprint`), credentials never answered or logged.
- Each member's transport (`transport`): akio's asynchronous HTTP client, `timeout` for the call,
  the answer's head and its connect (10 s at most), `max_response_bytes` for the body, one idle
  connection, no session resumption; TLS by a pin (akio-tls's `pin_sha256`), the member's CAs or
  the system's (read once, at start). The poller (`node`, a thread of its own, at most
  `max_concurrency` polls at once, waiting on the interval as fit's): `GET /node`, then `/status`
  and `/config/check` when the member lists them, each call started from the answer before; a
  console logged in and again on 401, logins shared by the calls waiting on them, a refused
  password waited out (a minute doubling to 15), a member's 429 by its `Retry-After`, a temporary
  password reported and its session logged out; fit's problem kinds and messages, the member's
  credentials taken out of them; the state (up, error, down, unknown) with fit's members. A fit
  member's version shows as `fit <version>`; the CSRF token goes in both `X-Akit-Csrf` and
  `X-Fit-Csrf` (decision row 72).
- **Stopgap (akio request A277)**: akio's client gives no response's peer chain and drops a
  certificate it refuses, so a member's certificate is read by a probe of its own: a TLS
  connection that verifies nothing, sends a request without credentials, reads `tls_info`'s chain
  and checks it with the pin or akio-x509's verifier against the CAs and the URL's host. It runs at
  a member's first poll, for each test, and after a failed call over TLS, which it tells apart as
  `tls_untrusted` or `tls_pin_mismatch` with the certificate.
- The console module `cluster` (`api`): `GET`/`POST /api/nodes`, `GET`/`PUT`/`DELETE
  /api/nodes/<name>` (If-Match the members' ETag, writes on the manager's pool, one at a time, 16
  waiting), `POST /api/nodes/<name>/test` and `/api/test` (4 at once), `GET
  /api/nodes/<name>/read/{node,status,versions,check}` (8 at once, a capability the member does not
  list answered 502), `/api/state`, `/api/audit` (user, node, action or its prefix; JSON lines),
  `/api/me` (every console user a cluster-wide admin until I139). The audit log records adds,
  changes (the fields' names), removals and tests, 1,000 entries and about 2 MiB, with the
  document's revisions. `admin.console` serves the module (`Options::cluster`), so the workbench
  draws its Cluster menu; the console's bodies other than a config are 128 KiB at most (fit's
  cluster bound), up from 64 KiB.
- I133: Overview, Nodes and a member's page checked in a browser against a manager with an akit
  member up and one down; `docs/guides/cluster.md`. The menu's later entries (Groups, Templates,
  Rollouts, Drift and alerts, Users and audit, Metrics) answer "no such endpoint" until I134–I139c.
- Tests: `akit-cluster`'s unit tests (members checked as fit's, the document kept and bounded,
  cookies, `fill`, fit's versions); akit's `tests/cluster.rs`: `manager_reads_akit_members` (by
  console and by token, a wrong password, a member down, reads, tests of a draft and a change,
  edit and removal, no credential in any answer or the log, the members kept across the manager's
  restart, a member stopped seen down), `members_over_tls` (untrusted with its fingerprint, trusted
  by its CA, by its pin, a pin that does not match), and, with `AKIT_FIT_BIN`,
  `mixed_clusters_with_fit` (fit members under akit's manager by console and by token, an akit
  member under fit's manager by token, the store akit's manager wrote read by fit's).

### 2026-11-27: I131, what a member needs

- `GET /node` on the Admin API, and so the console's `/api/node` and `/api/m/admin/node`
  (`akit_admin::node`, fit's `admin/node.go`): the node ID and where it is kept, the host name,
  `akit_version`, fit's `fib_version` as `akio <commit>` and `go_version` empty (decision row 71),
  the OS and architecture, the start and uptime, the running config's version, ETag, time and
  source, the roles running, the schema's version and digest (`sha256:` of the console's schema),
  and fit's capability names for what akit serves (`node`, `config.if_match`, `config.dry_run`,
  `config.text`, `config.effective`, `config.versions`, `config.transfer`, `config.reload`,
  `status`, `cache`, `cache.prefix`, `logs`, `tokens`, `schema`; `metrics`, `store`, `console` and
  `status.requests` when they are); the ETag and `Akit-Config-Version` in its headers. Not listed
  until akit serves them: `config.check`, `logs.recent`, `node.managed`, media, sync and
  BitTorrent. `GET /schema`, the console's schema.
- The node ID (fit's `NodeID`): `admin.node_id`, or a random UUIDv4 made once and kept in the
  store's section `node` (fit's, so that fit and akit sharing a store share it), or without a store
  in `akit.node-id` beside the config (written through a file renamed over it); a file's ID taken
  into a store that has none; kept nowhere, with a message, when neither can be written.
  `Configs::use_node`, `node`; `akit run` sets it before serving, with akit's version and akio's
  commit; `Options::node` reads the roles running and whether the running config counts requests.
- `GET /node` and `/schema` marked as v1 additions of the Admin API.
- Tests: `node::tests` (fit's `TestNodeIDKept`, `TestNewNodeID`, `TestUseNodeKeeps`); the console's
  `node_and_schema` (fit's `TestNode`, through the API and the console); akit's
  `node_id_kept_across_restarts` (without a store, into a store, `admin.node_id`).

### 2026-11-27: I130, cluster management in the config

- `admin.node_id`, `admin.cluster` and `admin.cluster_member` read and checked as fit's
  `config/cluster.go` (`akit_config::cluster`): the node ID's pattern; the manager's
  `poll_interval` and `timeout` (10s, 1s at least), `max_nodes` (256, at most 10,000),
  `max_concurrency` (8, at most 256), `max_response_bytes` (4 MiB, 16 KiB to 64 MiB),
  `link_address` (a host and a port), `link_url` (wss, with `link_address`) and `admins` (console
  users, each once, 64 at most), which needs `admin.console`; the member's `manager`,
  `fingerprint`, and `token` (`akite1.`, fit's `fite1.` renamed) or `token_file`, not both; fit's
  messages. `akit run` refuses `admin.cluster` until I132 and `admin.cluster_member` until I139b,
  naming the iteration (decision row 74).
- The 15 fields marked as v1 additions; the compatibility inventory and the config reference
  regenerated, with `admin.cluster` and `admin.cluster_member` sections in both languages.
- Tests: `akit-config`'s `tests/cluster.rs` (fit's `TestClusterErrors`,
  `TestClusterLinkSettings`) and `cluster::tests` (`TestClusterDefaults`, the helpers); akit's
  `cluster_is_refused_until_it_runs`.

### 2026-11-27: the Cluster management track planned (I130–I139c)

- fit's I140–I150 in twelve iterations ([the plan](dev-plan.md#cluster-management),
  [decision 041](decisions/041-cluster-management.md)): the config (I130), what a member needs
  (I131), the manager and its members (I132), the pages (I133), the node switcher (I134), groups and
  operations (I135), templates (I136), rollouts with health gates (I137), drift and alerts (I138),
  roles and the audit log (I139), members behind NAT (I139b), metrics, logs and the CLI (I139c).
  fit's I153 goes with the AI assistant track. A member's API and the store's `cluster` document
  are fit's, so that akit and fit members share a manager of either kind. Decisions waiting for
  review: rows 70–74.

### 2026-11-27: I109b, the tunnel's node's and proxy's pages; the roles' consoles' certificates read again

- The node's module (`akit_tunnel::node`'s pages, fit's `tunnel/node/api.go`) and the proxy's
  (`akit_tunnel::proxy`'s, fit's `tunnel/proxy/api.go`), served by I109's consoles at `/api/` and
  `/api/m/tunnel.node/` or `/api/m/tunnel.proxy/`: their status; hubs added, changed (the
  passwords and what the pages do not show kept, a token replacing the password and the password a
  token, `quic_pin` giving way to `pin`) and removed, an endpoint the config would refuse refused
  (fit's `EditHub`, `akit_tunnel::pages`); the node's services set; the proxy's nodes asked for
  again through a hub, a node's services set through it, its meta, its forwards added and
  removed; a tunnel closed by its hub and ID. An edit is answered 400 when the role's own apply
  fails, as fit's (decision row 60).
- What their status shows and the roles did not keep: each uplink's state as fit's
  `uplink.Status` (its links by their places with the protocol and address each dialed, `since`,
  the pool, the endpoints, the passwords as whether they are set, the token's ID); each tunnel's
  view (its hub, the node's address, the local and remote addresses, when it was opened, the
  bytes each way, a node's connect time); the node's connects to its services measured
  (`service_rtt`, kept by service, forgotten when a service goes or moves) and sent to its hubs
  with its pings, which hand them to proxies; the proxy's nodes of each hub as fit's `NodeList`
  (their address, round trip and services' connects, kept from the hub's lists, newest by epoch
  and version, and its pings' answers) with their shares of the tunnels to them (fit's `Weights`).
  The proxy now picks the hub a tunnel goes through as fit's `pick` (fit's `Cost`, `Weights` and
  `Choose`, `akit_tunnel::proxy::pick`), and a forward keeps its listener when its target changes,
  one that could not listen listed with why.
- `akit run` serves `tunnel.node.console` and `tunnel.proxy.console`: every role's pages are
  served.
- The roles' consoles' certificates are read again when their files change (decision rows 45 and
  61): `-watch-certs` looks at them beside `admin.console`'s, and SIGHUP reads them, each console
  serving the new one to its new connections, kept with its sessions
  (`akit_roles::pages::Consoles::watch_certs`, `RoleConsoles::watch_certs`).
- Tests: `pages::tests` (fit's `TestEndpointAddress`, `TestEditHubRefusesFragment`,
  `TestEditHub`, `TestDeleteHub`), `proxy::pick::tests` (`TestChoose`, `TestWeights`);
  `tests/peer_pages.rs`: fit's node `TestPages` and proxy `TestProxyAPI` and `TestPick` on akit's
  hub, node and proxy, and, with `AKIT_FIT_BIN`, akit's node and proxy linked to fit's hub (the
  proxy's pages showing fit's hub's nodes with their round trips and the node's services'
  connects, a node's services set through fit's hub, a tunnel it routes closed from the node's
  pages); akit-admin's `a_role_consoles_tls_reload`; akit's `node_and_proxy_pages_under_akit_run`
  and `a_role_consoles_certificate_renewed_without_a_restart`.
- Decisions waiting for review: rows 60 and 61.

### 2026-11-27: I109d, the LLM roles' methods for their pages

- Every LLM role's own pages' mesh methods (`akit_llm::role::admin`, fit's `mesh.RegisterAdmin`,
  `role/settings.go`, `role/tokens.go`), the caller `local` alone: `/mesh/peers`, `/mesh/dial`
  (a token replaces the password and the password a token, neither keeps both, 64 targets at
  most), `/mesh/undial`, `/mesh/kick`, `/mesh/listen` (an empty password keeps the current one,
  each role's apart, unless `clear_passwords`; a self-signed certificate kept; off loopback only
  over TLS; one that does not bind answered 5000 with why), `/mesh/limits/get` and `/set` (written
  without the defaults), `/mesh/tokens/list`, `/create`, `/expiry` and `/revoke`; each change an
  edit of the member (`role::edit_member`), answered once applied. A CA file is read on a pool of
  the role's (fit's: 16 running, 256 waiting, 503 when full).
- The proxy's (`akit_llm::proxy::methods`, fit's `llm/proxy`): `/proxy/models` (each model with
  its routes), `/proxy/stats` (the calls, errors, response bytes and calls under way per model since
  the start, 1,024 names at most, the others as `other`), `/proxy/keys/list`, `/create` (`ah-` and
  32 random bytes, shown once) and `/delete` (256 keys at most), `/proxy/profile/set`,
  `/proxy/node/list` (each hub's node list, kept from its pushes and fetched when it links, and the
  nodes linked directly with their links and traffic), `/proxy/api/get` and `/set` (its
  certificate read on the pool). The keys' last use and calls are counted in memory.
- The identity's `http_url`: the role's console's URL, as the console server tells the role where
  it listens (`akit_roles::pages::Module::console_url`, which `RoleConsoles` calls when a console
  starts and stops), and until then from its member's `console`.
- Tests: `role::admin::tests` and `proxy::methods::tests` (fit's `TestEditMesh`, `TestAdminTokens`,
  `TestLimitsSettings`, `TestStatsOverflow`, `TestCatalogOrder`, `newKey`, `setAPI`);
  `tests/page_methods.rs` (fit's `TestRegisterAdmin`, `TestAdminEdges`, `TestKeysMethods`,
  `TestProxyRoleEdges` on akit's roles, edits written by a control thread as `akit run`'s; a
  console on a port chosen when it starts, its URL in `http_url`); `tests/pages.rs` (the methods
  through a node's console); `tests/fit_pages.rs` (with fit's binary: a token issued from akit's
  hub's pages takes fit's node and its revocation closes the link; akit's proxy dials fit's hub
  from its pages and lists its nodes, models and a call).
- Decisions waiting for review: rows 63 and 64.
### 2026-11-27: I109c, the LLM roles' pages; I109d split

- The LLM roles' module (`akit_llm::pages`, fit's `llm/internal/webui`), served by I109's
  consoles at `/api/` and `/api/m/llm.<role>/`: `GET /api/me` (the identity, the user, a proxy's
  `api_url`); `POST /api/rpc<method>` for every method of the role's router, called as the role's
  own pages through `Local` (I106b), answered with its JSON or `{"error", "code"}` with fit's
  status of the code (`status_of`, 504 for a timeout), a body that is not JSON 400; `POST
  /api/stream<method>` for its stream methods as Server-Sent Events (a data event per frame,
  `null` for an empty one, `end` with the result, `error` with `{code, message}`, a refusal before
  any frame as JSON), at most 16 at once (429), each frame given back to the stream only once the
  browser took it (past 64 KiB queued, until the queue drains), a wait bounded by 30 s, the stream
  canceled when the browser goes. Changes are written back by the methods themselves
  (`role::Base::edit`). Each role makes its module once (`Base::pages`, `Role::module`).
- `akit run` serves the LLM roles' `console`; the tunnel's node's and proxy's stay refused until
  I109b. The methods the LLM pages call that akit's roles lack (`/mesh/dial`, `listen`, `limits`,
  `tokens`..., the proxy's `/proxy/*`) answer 404 until I109d (decision row 47).
- Tests: `pages::tests` (akit-llm): fit's `TestRPC`, `TestStatusOf`, `TestStream`,
  `TestStreamLimit`, `TestStreamSlowBrowser` and `TestSSEEnds`' empty result on a router of the
  tests' own, a browser that stalls past the timeout; `tests/pages.rs`: a node's pages (its
  identity, a model saved into its member, a refusal, a stream refused before any frame, its
  console stopped with it) and a hub's reaching its node (`/hub/node/call`). fit's `TestRPCBusy`
  has no counterpart: akit's methods do not wait for a pool of the pages'.
- Decisions waiting for review: row 47.

### 2026-11-27: I109, the roles' consoles and the hub's pages; I109b and I109c split

- The roles' consoles (`akit_admin::console::RoleConsoles`, fit's `app/roleconsole`): a role's
  `console` is served by the console's server (I90) with the role's module in place of the Admin
  API: fit's workbench, behind the console's logins, CSRF token, connection rules and access, the
  module at `/api/` and `/api/m/<role>/`, the cookie `akit_<role>_<port>`, bodies of 1 MiB. Each
  moves with its role's member (the roles' manager calls it after the role's apply, through
  `akit_roles::pages::Consoles`): started anew when its settings change, its access (the
  console's, then the global one) set by every config applied; one that does not start leaves the
  one before running, and is the role's warning. `admin.console` lists the running roles'
  modules beside its own (`/api/workbench`) and serves them at `/api/m/<role>/`.
- The roles' modules (`akit_roles::pages`, fit's `console.Module` and `tunnel/internal/common`):
  `Role::module`, the request's JSON read as fit's handlers decode it, `{"error"}` answers, the
  edits of a role's member (`pages::edit`, fit's `common.Edit`: answered once applied,
  `{"ok", "warnings"}`, 400 when the change or the config does not load, 404 for what is not
  there, 503 when the control thread is busy), and the link tokens' endpoints (`pages::tokens`,
  fit's `TokenRoutes`), their value helpers moved from akit-admin to `akit_roles::linktoken`.
- The hub's pages (`akit_tunnel::hub::pages`, fit's `tunnel/hub/api.go`): its status, tokens,
  listeners added, changed (their certificate files kept) and removed, settings (passwords set or
  cleared, the console socket moved or removed), a node refreshed or given services (502 when it
  refuses), a peer kicked, a tunnel closed; each change logged with its user.
- `akit run` serves `tunnel.hub.console`, and so its `console_socket`, which carries the hub's
  pages (decision row 35's outcome). It refuses `tunnel.node.console` and `tunnel.proxy.console`
  until I109b, and the LLM roles' `console`, which it had read and ignored, until I109c (decision
  row 46).
- Tests: `tests/pages.rs` (akit-tunnel): fit's `TestModule` (roleconsole), `TestPages` and
  `TestPagesPeers` against akit's own node and proxy, the token endpoints, a console moved with its
  member; against fit's binary, akit's hub's pages through its console socket and fit's web proxy
  (the workbench, a login, the status). `role_pages_under_akit_run` (akit): the hub's console and
  `admin.console`'s module under `akit run`, a listener added on the hub's page in the config's
  history. `consoles_move_with_their_roles` (akit-roles).
- Decisions waiting for review: rows 45 and 46.

### 2026-11-27: the stopgaps of A247–A249 go

- akio's A247–A249 in the LLM node (decision row 40's outcome): `akit_llm::exec::is_root` asks
  akio-sys's `process::euid` (`init_euid`, which read `/proc/self` or ran `id -u` at the node's
  start, is gone); `Killer::kill` sends a job's process group SIGKILL with
  `process::kill_group`, at once, from any thread, under the lock that keeps the leader unreaped
  (the `kill -KILL -- -<pgid>` run on the pool is gone, and `Killer` and `Jobs` take no pool); the
  commands `exec::output` runs alone (the hardware's and the GPUs' queries) are started in a group
  of their own too; Linux's disks are read with `fs::space` on the node's pool, as fit's
  `Statfs` (`df` is no longer run there; macOS keeps `df -k -P -l`, as fit's). `hw::collect`,
  `gpu::collect` and `exec::output` take no pool: they block, and run on the node's.
- Tests: `akit-llm`'s, unchanged, on macOS.

### 2026-11-27: akio pinned at `a8171fda`: A247–A249

- akio: `a8171fda` on akio's main, with A247 (`akio_sys::process::euid`), A248
  (`process::kill_group`, killpg, and `process::SIGKILL`) and A249 (`fs::space`, a file system's
  size and space free). akit's `rev` and `akit_runtime::AKIO_REV` move to it; nothing in akit had
  to change to build on it.

### 2026-11-27: the LLM mesh listener's certificate read again; a session test's race

- Decision 44's part left to the LLM track: a mesh listener's `cert_file` and `key_file` are read
  again at each apply (as `akit run -watch-certs` applies the config once a role's files change),
  and a certificate that changed is served from then on through akio-tls's `CertStore`, in place:
  the listener and its links stay, new links see the new fingerprint. Files that do not load
  (a certificate written, its key not yet) leave the running certificate, logged. The hub, node
  and proxy report the files through `Role::files` (I102d's), so `-watch-certs` watches them.
  Tested: `mesh::tests::cert_files_read_again`, `tests/hub_manage.rs`'s `listener_files_reported`.
- `wire::session::tests::cancel_idle_and_close` failed on gpu51 (10 of 13 runs on io_uring, 1 of
  3 on epoll, on akio `6151f5c5` as on `8a9601b9`): both ends of a stream time it out after the
  same `stream_idle`, and the opener's timeout cancels the stream (a stream-cancel frame), so the
  responder hears `Canceled` when the opener's timer runs first, as in fit, whose
  `TestStreamIdleTimeout` takes either. The session is right; the test expected the responder's
  own timeout only. It now takes either end's, as fit's tests do, and checks the opener's side
  too (its own timeout, or the responder's carried by its error end).

### 2026-11-27: I108, the LLM node's management: hardware, GPUs, fixed actions and jobs

- The hardware (`akit_llm::hw`, fit's `hwinfo_*.go`): read at the node's start and every
  `refresh_interval` on the node's pool (2 running, 16 waiting), without root and on macOS and
  Linux alike: procfs on Linux (kernel, distribution, CPU, load, uptime, memory), `sysctl`,
  `sw_vers`, `vm_stat` and `system_profiler` (once) on macOS, `nvidia-smi`'s GPUs, the disks
  (from `df -k -P`, a stopgap until akio's A249; on Linux those of `/proc/mounts` of fit's kinds),
  the addresses from akio-sys's interfaces; what cannot be read is left out. The node's info
  carries it and is pushed when it changed, the hardware included, as fit's.
- The GPUs (`akit_llm::gpu`, fit's `llm/internal/gpu`): the catalog of seven vendors (their
  tools, install scripts as constants, settings as argv templates; the scripts' variable is
  `AKIT_LLM_GPU_DATA_DIR`, decision 038), the PCI bus read for vendors on Linux, each installed
  tool queried and parsed (nvidia-smi's CSV, rocm-smi's JSON, xpu-smi, system_profiler) or kept
  raw, a status cached 5 s and read one at a time on the pool; settings' values checked
  (integers in bounds, options) inside one argument, `sudo -n` when set and not root.
- Commands and jobs (`akit_llm::exec`, `akit_llm::jobs`, `akit_llm::actions`): fit's eleven fixed
  actions, their parameters checked against anchored patterns (akio-regex), never a shell but a
  constant script; a command in a process group of its own, its output (both streams, in order,
  through a socket pair) in a buffer of the last 1 MiB with absolute offsets; jobs on a pool of
  their own, 8 running (one more refused, rate limited), 200 kept, `job_timeout` and cancel
  killing the group (the leader at once, the group by `kill(1)` on the pool, a stopgap until
  A248; root known from A247's stopgap); `/node/job/follow` sending the new output as it is
  written, 100 ms apart at least, the state every 30 s without any, under the stream's credit.
- The methods (`node::manage`): the model list (keys masked), save (an empty or masked key keeps
  the stored one; 256 at most), delete (ambiguous without its API) and check, the profile, the
  GPUs' status, catalog, install and settings (every GPU of a vendor with index -1), the actions
  and jobs, for the node's pages and a hub where `allow_hub_manage` is set (checked at each
  call); the settings and the upstream bounds for its pages alone, written into the member with
  the defaults left out.
- Tests: fit's action, command, output buffer, job, GPU and catalog tests (`node_test.go`,
  `gpu_test.go`, `fake_test.go`) on stand-in tools, never a real one (`exec::tools_in`, as fit's
  tests set `PATH`); `tests/node_manage.rs`: fit's `manage_test.go` and the node's GPU methods
  (access, jobs through the methods, profile, settings, upstream bounds, model edits and bound,
  checks, GPU status, set and install, hardware read again); akit's hub managing akit's node.
  Against fit's binary (`tests/fit_manage.rs`): fit's hub manages akit's node through its console
  (`/api/rpc/hub/node/call`: the models, a job, the catalog, a model saved into akit's member).

### 2026-11-27: I106b, the hub's calls for its pages, the node list's bounds, the routes' counts

- Every role's methods reach its own pages through `akit_llm::local::Local` (`Role::api`, fit's
  `Router.Invoke` and `OpenLocal` as the caller `local`): an in-memory session (`wire::pipe`), so a
  page's call or stream behaves as one over the mesh, through the same methods. A method that
  changes the config does it with `Base::edit` (fit's `Base.Edit`): the member changed by the
  control thread as a config from the role's pages, answered once applied; the change's own error
  keeps its code, a config refused is a bad request (decision 38).
- The routes' counts (`mesh::routes`, fit's `mesh.Routes`): the calls a role relays per route
  (from, to, model; 1,024 routes apart, the rest as `other`), their bytes read from the stream as
  they cross; counted by the node (caller to upstream, an upstream's error status a failure), the
  hub (proxy to node) and the proxy (its agents to a hub or a node, each attempt). `/mesh/status`
  on every role (its own pages): its identity, links and routes (fit's `MeshStatus`).
- The hub: `/hub/nodes` (fit's `HubNode`s, online first), `/hub/node/call` (a node management
  method, not a stream one, relayed within 90 s; offline, forbidden and other errors as fit's
  `manageErr` words them) and `/hub/node/stream` (`/node/job/follow` relayed frame by frame under
  both streams' credit, the pages' end canceling the node's stream), `/hub/settings/get` and
  `set` (`max_nodes`, the default left out of the member), `/hub/profile/set`. The node list as
  fit's: each node's hardware without disks and addresses, 16 GPUs, 64 models, its links and its
  traffic from the routes; encoded within 4 MiB (the nodes at its end left out); its sequence
  grows only when it changed; pushed 200 ms after a change and a second after the last at the
  soonest, every 5 s while proxies are linked.
- `akit_llm::hw::HwInfo`: fit's `HWInfo`, read from and written as a node's info carries it.
- Tests: the routes (fit's `routes_test.go`), the node list's summary and bound
  (`nodelist_test.go`); `tests/hub_manage.rs`: a node standing in for one, its calls and streams
  relayed, refused, offline, a peer refused the pages' methods; the list's sequence; settings and
  name written back, a failed edit answered (fit's `TestRole`); `tests/e2e.rs`: each role's
  routes after the end-to-end calls. Against fit's binary (`tests/fit_manage.rs`): akit's hub
  manages fit's node (its model list, a job run and followed, a model saved).

### 2026-11-27: I102d, the hub's QUIC handshake bounds and its listeners' certificates read again

- The hub's `quic` listeners on akio's A245 (ADR-185), as fit's: each handshake bounded by
  `tunnel.hub.quic.handshake_timeout` (10 s when left out), a connection whose handshake is not
  done by then dropped; at most `max_handshakes` under way (64 when left out), past half of them a
  new client asked to prove its address with a Retry, past all its Initials dropped until a
  handshake is over (counted in akio's `refused_connections`). A connection whose handshake is over
  and that has not opened its link's stream within `handshake_timeout` is closed (fit's
  `streamWait`): akio's timer, the handler's look through akio-quic's `Connection::notify`.
- A `tls`, `wss` or `quic` listener's certificate is read each time the hub's member is applied:
  its `cert_file` and `key_file`, or the hub's own certificate (`quic.cert`, or the file kept beside
  the config). A listener kept whose certificate changed serves the new one to the handshakes from
  then on, in place, through akio-tls's `CertStore` (TCP and QUIC alike): the listener is not
  reopened, and its links go on; files that do not load leave the running certificate, and the
  role's warning says why. The files read, with their SHA-256 (zeros for one that could not be
  read), are reported through `akit_roles::host::Role::files`, a new method (none by default), and
  `akit run` watches them with the listeners' own (`-watch-certs`): once they change and settle,
  the config is applied again, as for a listener's. The hub's status shows each listener's own
  fingerprint (fit's `ListenerView`). A hub with nowhere to keep its certificate makes it once a run.
- Not built here: the LLM mesh listener's certificate read again (decision row 44); another change
  was editing akit-llm's roles meanwhile. `Role::files` is what it would report through.
- Tests: fit's `TestServerIdleConn` and `TestListenerRetry`, and akio's relay tests, ported on the
  hub's server (`tests/quic_bounds.rs`: a stalled handshake dropped at a 300 ms timeout, a
  connection without its stream closed after it, `max_handshakes` 2: a Retry at half, the limit
  waited out, the defaults fit's); fit's `TestListenerCerts` (`a_listeners_certificate_is_read_again`:
  `tls`, `quic` and `wss` listeners' fingerprints, the files noted, a renewal served to new links
  over TLS and QUIC with the old link going on and the listeners' addresses kept, a key gone
  leaving the running certificate); end to end, `akit run` serving a hub listener's renewed
  certificate without a restart (`a_hub_listeners_certificate_renewed_without_a_restart`,
  `crates/akit/tests/watch_certs.rs`). Against fit's binary (`AKIT_FIT_BIN`,
  `with_fits_node_and_proxy_over_quic`): fit's node and proxy link to akit's hub over QUIC with the
  bounds set, 8 MiB echoed, and pin a renewed certificate.
- Decisions waiting for review: rows 42 to 44.
- Nothing to measure: the handshakes' bounds and the certificates are off the hot path (rule 24).

### 2026-11-27: akio's A238 in akit: WebSockets ended at once

- On akio's `WsSender::abort` (A238, ADR-184), as fit's `Abort`: `akit_tunnel::ws::WsWire::close`
  (a failed link, a kick, a revoked token, a role stopped) and a link's WebSocket the hub refuses
  end at once, what is queued dropped, without a close frame; `close_after_send` keeps the close
  frame behind what is queued. The hub's console socket aborts a WebSocket it refuses (past
  `max_conns` or a quarter from one address), one that does not prove the password in time, one
  whose message does not open, and every one when the socket stops; a proxy's web proxy aborts its
  WebSocket when the hub's hello or a message does not open, or the local connection is gone. A
  WebSocket whose TCP side closed still gets a close frame (1000), as fit's. The LLM mesh dialer's
  force close (a dead or too slow peer) aborts too, as fit's `forceClose`, and the 1 s close
  timeout it needed is gone.
- Tests: `ws_links_that_fail_end_at_once` (a link frame shorter than a header: the hub's
  WebSocket ends without a close frame, at once; it fails with the stopgap's close frame); the
  console socket's `aborts` (fit's `TestSocketAborts`) checks that both WebSockets end with 1006.
- Decision rows 20, 26 and 36 have their outcomes.

### 2026-11-27: akio pinned at `8a9601b9`: A238 and A245

- akio: `8a9601b9` on akio's main, with A238 (`WsSender::abort`: a WebSocket ended at once, no
  close frame, what is queued dropped, ADR-184) and A245 (akio-quic's server's handshake bounds,
  `ServerConfig::handshake_timeout` and `max_handshakes`, a Retry past half, ADR-185). akit's `rev`
  and `akit_runtime::AKIO_REV` move to it; nothing in akit had to change to build on it. The
  stopgaps for A238 stay until the change that replaces them, and I102d takes A245.

### 2026-11-27: I93b and I93c, `akit run` with the encrypted store, and `akit run -admin`

- **The config kept in the store** (I93b), as fit's `cli/run.go`, `cli/store.go` and
  `admin/bootstrap.go` (`akit_admin::bootstrap`, `crates/akit/src/boot.rs`): the store `admin.store`
  names is the base config akit runs from, what it holds merged onto the config files at each start
  (objects member by member, other values and lists whole, the store's winning) and written back
  when that changes it, seeded from the files when it does not exist; the bootstrap file's `admin`
  split off, never kept, shown or taken from a config sent (one that sets it is refused, 400), and
  put back as the last member of every config as it loads, so that every error stays at its place.
  `Configs::use_store`: every base config applied (the Admin API, the console, a rollback, SIGHUP, a
  role's own edit) written to the store, sealed, synced and renamed, on the control thread, unless
  it holds that config already; one that cannot be written runs and is said unsaved. SIGHUP reads
  the store again (a new key derived when `akit store import` gave it a new salt); `-watch` is
  refused with a store. A store written from the files at a start is put back as it was (or
  removed) when the start fails after it, so that a broken first start leaves no store whose values
  would win at the next; a store that does not open (a wrong password, a changed byte) stops akit.
  Without a store, roles keep what they make beside the config; with one, in the config.
- **The console's passwords** (I93b), as fit's `admin/console.go`: `POST
  /api/m/admin/account/password` (the current password checked as a login's, one from an address at
  a time, a failure counted toward the lockout) and `/users/password` (another user's), the new
  bcrypt hash made and written beside the config in the store on the console's login pool, the user's
  other sessions ended, logged in the `admin` logger; 409 without a store, 500 when the write fails
  (nothing changes); at a restart the store's hashes replace the bootstrap file's. `GET
  /api/m/admin/users` says `"store": true`.
- **The `store` format** of `POST /config/export` and `/config/import` (I93b), as fit's
  `admin/transfer.go`: a `fitstore` file sealed with the request's password or the running store's
  key (a whole config's with the console users whose passwords changed), and one opened with
  `Akit-Store-Password` or the store's password, applied as the config or `?members=` of it, the
  running `admin` kept. The keys are derived on the API's pool, one at a time in the process (the
  others 503 with `Retry-After`), a file asking for more than twice what akit writes refused before
  (`store::unseal`); the store's key and password reach the pool as a `Sealer`, apart from the store.
- **`akit run -admin`** (I93c), as fit's: `admin` and `admin.console` with their defaults where the
  config leaves them out, the config kept in a store: the one `admin.store` names, a `.store` file
  given with `-c` (two are refused), or the one beside the config files (`site.json` gives
  `site.store`, a directory `<dir>.store`, no file `akit.store`), used once it exists or `-admin`
  makes it, its password `$AKIT_STORE_PASSWORD` or the key file beside it (`site.store.key`, 52 base32
  characters, made 0600 with `O_EXCL` when neither it nor the store exists, never printed). With no
  console user, the user `admin` with the temporary password `admin`, to be changed within
  `-timeout` (30m; `-timeout` needs `-admin` and more than 0): a session with it gets `must_change_password`
  and `password_expires`, may read who it is and change the password and nothing else (403), and
  the Admin API refuses every request meanwhile; past its time the password logs in no more (401,
  `password_expired`). `akit store export` and `import` find the store as `run` does.
- **fit's tests ported**: `cli/store_test.go` (`TestServeStore`, `TestStoreCommand`,
  `TestServeStoreMergeUnwritable`, `TestServeStoreFailedFirstStart`) and `cli/adminflag_test.go`
  (`TestServeAdminFlag`) in `crates/akit/tests/store.rs`; `admin/accounts_test.go`,
  `accounts_edge_test.go`, `transfer_test.go` and `transfer_edge_test.go`'s store cases
  (`TestConsolePasswordChange`, `Reset`, `NoStore`, `StoreFails`, `Lockout`, `ResetLockedOut`,
  `Bodies`, `ResetStoreFails`, `TestTemporaryPassword`, `TestConsoleStartsTemporary`,
  `TestExportImport`, `TestImportStoreBounded`, `TestSetConsoleUsersWithoutStore`) in
  akit-admin's `tests/console.rs`; `TestMergeConfig`, `TestWithDefaultConsole` and
  `TestWithConsoleUsers` as `bootstrap`'s unit tests. What differs is decision 33.
- **With fit**: `tests/fitstore` gains a store fit's own code opened again, wrote a config back to
  and changed a password in (`writeback.store`, `akit_vectors_test.go`), which akit opens; akit's
  stores written so, and fit's written back by akit, open in fit (`TestAkitWrote`). With
  `AKIT_FIT_BIN` naming fit's binary (`go build ./cmd/fit` at the gate's pin), fit and akit run in
  turn from one store and its key file with `run -admin`, each serving the config the other changed
  in its console and logging in with the password the other changed (`fit_and_akit_share_a_store`),
  and each exports the other's store.
- **Decisions** 30 to 33 ([pending](decisions-pending.md)): fit's names renamed (`AKIT_STORE_PASSWORD`,
  `Akit-Store-Password`, `akit.store`) with the key file as fit's; export derivations bounded with
  imports'; the users' and the config's writes under two locks; the tests' differences.
- Tests: `make check` on macOS (kqueue) passes but for its doc tests, which other worktrees'
  builds of the shared target directory broke (E0460/E0463), passing alone in a target directory of
  their own; on gpu51 clippy `-D warnings` and the whole workspace in release, on epoll and with
  `AKIO_TEST_BACKEND=uring`, pass (epoll's run predates two akit-admin tests updated for the `store`
  format's messages, which the io_uring run includes). No benchmark (control plane, rule 30); the console's row of the benchmark
  table stands.

### 2026-11-27: I103b, the console socket and web proxies

- `akit_tunnel::wscrypt`: fit's `tunnel/internal/wscrypt` byte for byte: the password's PBKDF2
  (`fit-tunnel-ws`, 10,000 rounds, SHA-256), 16 random bytes each way, a key per direction by
  HKDF-SHA256 under both, AES-256-GCM with the message's number as its nonce; a message that does
  not open as the next one ends the session. A vector sealed by fit's own `wscrypt.go` is checked.
- The hub's `console_socket` (`akit_tunnel::hub::socket`, fit's `tunnel/hub/socket.go`): a listener
  of akio-http's upgrade, any path, the handshake bounded as a `ws` link's, the hub's access policy
  at akio's filter. Each WebSocket gets the hub's hello, then the proxy's; its first message that
  opens proves the password; `max_conns` at once, a quarter of them at most from one address unproven,
  each closed at 10 s unproven. Its first bytes dial the hub's `console.address` (an unspecified host
  the loopback), within 5 s; what arrives meanwhile waits (1 MiB at most), the WebSocket's reads held.
  The console's bytes go sealed, 64 KiB a message. The socket's password is its own or the hub's;
  a new address listens anew and closes the WebSockets, a new password or bound applies to those that
  open from then on; the hub's status gains fit's `socket`.
- A proxy's `web_proxies` (`akit_tunnel::proxy::web`, fit's `tunnel/proxy/webproxy.go`): a local
  listener each, filtered by the proxy's access policy; each connection, its reads held, gets a
  WebSocket of its own to the hub's socket (akio-ws's client, `User-Agent: akit`, the dial and
  handshake within 10 s); once the hub's hello comes, the proxy's hello and an empty sealed message
  (the proof) go, then what the browser sent. `max_conns` at once; a web proxy kept by its `listen`
  across applies takes a new password, URL and bound for the connections it accepts from then on; one
  removed closes its connections. `ProxyApi::web_proxies` (the proxy's `Role::api`, fit's
  `WebProxies`).
- Backpressure (rules 8 and 9): both relays pace each side by the other's queue
  (`akit_tunnel::pace::Gate`, fit's `common.Gate` at its 256 KiB `Watermark`), on akio's
  `hold_reads` and `on_drain` and akio-ws's A236 (`hold_reads`, `on_drain`, `set_relayed`); each
  side relayed. **Stopgap (akio request A238)**: a WebSocket refused or failed is closed with a close
  frame behind what is queued, where fit aborts (decision row 20). Gone since akio `8a9601b9`:
  they abort (`WsSender::abort`), as fit's.
- `akit run` refuses the tunnel roles' `console` until I109 serves the roles' pages, where it read
  and ignored it; the hub's `console_socket`, which needs one, is refused with it. A proxy's
  `web_proxies` run. Decisions waiting for review: rows 35–37.
- Tests: fit's `socket_test.go` ported (`hub::socket::tests`: a request in several messages before
  the console is connected, a WebSocket past `max_conns`, a bad hello, no console; unproven
  WebSockets closed at the timeout and bounded per address, a proof kept and not passed on; a message
  that does not open, the socket removed; the hub's password, a new one, a new address) and
  `webproxy_test.go` (`proxy::web::tests`: the page through the proxy, a wrong password, `max_conns`,
  a proxy removed, a socket nowhere, a bad URL; the session step by step against a WebSocket server
  of the test's own: the proof first, early bytes in order, a message that does not open, a local
  connection gone before its session, a bad hello). End to end (`tests/console.rs`): 24 MiB echoed
  both ways and eight at once, a reader that stops holding the console (about 3 MiB of 96 written
  on macOS), connections cut on either side. Against fit's binary (`b6c5cebf`, `AKIT_FIT_BIN`,
  skipped in CI): fit's hub's console (its page and `workbench.js`) through akit's web proxy, and
  8 MiB echoed through fit's web proxy and akit's hub's socket.
- Next: I102d; I109 serves the pages the socket carries.

### 2026-11-27: I105–I107, the LLM node, hub and proxy; I106b split

- The roles (`akit_llm::{node,hub,proxy}`), registered in `akit run` (`akit::ROLES`): what they
  share is `akit_llm::role::Base`: the member's `id` (made at the first start, `<role>-<16 hex>`,
  written back into the member) and `name`; the access policy (the member's, then the global
  one) at the listener's filter; the `limits`; the `listen`, its `self_signed` certificate made
  once and kept beside the config (`akit.llm.<role>.pem`, 0600; written into the member when
  nothing is kept beside the config); the `peers`; the `tokens`, a token's first member written
  back into its record, the links of expired tokens closed by a sweep armed again on every apply.
  The version a role says hello with is `akit <version>`.
- The node (I105): its models' state (`ready`, `disabled`, `error`), checked every 60 s (15 s in
  error) by a probe of the upstream's model list on akio's client (401, 403 and 5xx fail it, a
  stale probe dropped); its info (`/node/info`) and its pushes to its hubs and proxies
  (`/node/update`, 200 ms apart, only when it changed, with a growing sequence; at once to a peer
  that links); `/node/proxy`: the model taken within its `max_inflight` and the node's, the
  upstream URL and auth of each API (OpenAI's, Anthropic's with its version, Gemini's with `alt`,
  Azure OpenAI's deployments, Ollama's, Cohere's), the model renamed upstream in a JSON body, a
  form's field or a Gemini batch, the request body inline or streamed from the stream's frames
  (held against the window past 256 KiB waiting upstream, on the request body's `on_drain`), the
  response's head and body sent as they come under the stream's credit (the upstream's reads
  held otherwise), `first_byte_timeout` (which also bounds the connect: akio-http's client has no
  connect timeout of its own), `idle_timeout`, `timeout`, a keep-alive until the first byte, the
  model put in error when its upstream cannot be reached. The hardware in its info is a minimum
  (host, system, CPUs) until I108.
- The hub (I106): the node table (`max_nodes`, the offline node seen longest ago forgotten; a
  node's info of an older sequence in the same run dropped), the models of its online nodes
  pushed to its proxies when they change (200 ms apart), `/hub/models`, a node list
  (`/hub/node/list` and its pushes, 64 models a node), and `/hub/proxy`: the least loaded node
  serving the model ready, ties round robin, the call relayed by two pumps (the proxy's streamed
  body to the node, the node's frames to the proxy) under each stream's credit, another node
  (3 at most) while nothing reached the proxy and none of its body was taken.
- The proxy (I107): the agents' API on `api.address`, an akio-http listener of its own on the
  listeners' engine (TLS with `cert_file`, `max_conns` closed past at accept, the role's access
  policy, `max_body_bytes`); the model lists of OpenAI and Anthropic (`/v1/models`), Gemini
  (`/v1beta/models`) and Ollama (`/api/tags`); the calls of the five APIs, the key from
  `Authorization: Bearer`, `x-api-key`, `x-goog-api-key` or Gemini's `key`, its SHA-256 compared
  with every key's in constant time; the catalog of the direct nodes' and the hubs' models
  (pushed, and read when a peer links); routes: the ready direct nodes, least loaded first, then
  the hubs, 4 at most before the head; bodies past 1 MiB streamed in 64 KiB frames; the response
  streamed to the agent with `X-Akit-Node`, the stream's frames held while the agent is behind;
  `max_inflight` (429), `first_byte_timeout` (503) and `timeout` (504); each API's error shape
  and status.
- Tests: units of the form and JSON rewrites, upstream URLs, auth headers, endpoints and error
  shapes; end to end (`tests/e2e.rs`): an agent through akit's proxy, hub and node to a fake
  OpenAI-compatible upstream (the model renamed, the upstream's key, streamed events, a 3 MiB
  body streamed through the hub, a wrong key, an unknown model, a model of another API, bad JSON,
  an unknown path), a direct node first, a dead upstream answered 503, an expired token's link
  closed. Against fit's binary (`tests/fit_roles.rs`, `AKIT_FIT_BIN`): a fit hub between akit's
  node and proxy, akit's hub between fit's node and proxy, and akit's proxy to fit's node
  directly, each with the same calls.
- The mesh: a peer-up callback runs once the hello's answer has gone, so that what it sends (a
  hub's `/node/info`, a proxy's models) reaches a peer that has registered the link.
- Left: I106b (the hub's calls for the pages, the node list's byte bound, the routes' counts),
  I108 (the node's management, its hardware), I109 (the roles' pages).

### 2026-11-27: I104, the LLM mesh: wire, links and mesh on akio-ws

- `akit-llm` (new): fit's `llm/internal/wire` and `mesh`, byte for byte on the wire. Frames
  (`wire::frame`): fit's header, the method on calls, stream opens and pushes, gzip from 1 KiB when
  smaller (akio-deflate), 16 MiB encoded or inflated. Sessions (`wire::session`): odd and even
  stream IDs, calls with a timeout that tells the peer to cancel, pushes, pings with the round
  trips' meter, drain, `max_inbound` (rate limited past it), streams with a 256 KiB window each way
  and credit given back at half of it, the idle timeout (a timer that sets itself again for what is
  left), keep-alives, `Pump`. fit's goroutines are callbacks here: a call's handler answers through
  a `Reply` whenever it is ready (dropped unanswered, an internal error); a stream's reader gets
  its frames at `on_data` and gives them back with `release`; a sender that finds no credit
  (`try_send`) is told when to try again (`on_credit`). Pushes are handled on the link's thread in
  the order they came, so none waits (fit's queue of 1,024 has nothing to hold), and an outbound
  link's hello answer registers the peer before the next frame is read (fit's two-second wait in
  `caller` has nothing to wait for).
- Links (`wire::conn`): akio-ws on the listeners' engine. A listener is an akio listener of its own
  serving akio-http's upgrade on `/ws` alone (404 for any other path), over TLS (TLS 1.3, no
  tickets) or plain on loopback; fit's handshake after the upgrade (the 32-byte challenge, a
  token's `FTK1` answer or a password's HMAC and nonce, the `0x01` acknowledgement with the
  listener's proof, 4001 and 4003 closes); at most 256 links not authenticated and 16 from one
  address, counted from the TCP accept (akio's `on_accept`/`on_close`) and closed 10 s after it
  unless they authenticate; `max_conns`. Each link is paced as fit's `Conn.send`: past 64 KiB queued
  its streams are held and its reads held for a quarter of the shorter of the ping interval and the
  slow timeout at most, until akio-ws's `on_drain` (A236); the links are relayed (`set_relayed`),
  so that the engine's own pause, which only sends on the link's loop check, does not hold a link
  fed from other links. The heartbeat: a ping after an idle interval, after frames sent or after a
  round trip two intervals old; three intervals silent or a ping waiting the slow timeout drops the
  link. A dialer's pinned `wss://` link uses akio-tls's `pin_sha256` (A235), so the pin is checked
  in the handshake.
- The mesh (`mesh`): fit's `Mesh` on timers and callbacks: a listener (`set_listen`, TLS off
  loopback), dial targets kept linked with full-jitter backoff (longer after a refused hello), the
  hello both ways with fit's checks (roles taken, not one's own, not oneself, an ID held under
  another role, the password's roles, a token binding the first ID that says hello with it, the
  role a target must have), pools of links by peer ID, round robin over those not draining,
  `call`, `open_stream`, `push`, `peers`, `targets`, `kick`, `configure` (what did not change, its
  links among it, left alone), `shutdown` waiting for the calls and streams under way (a condition
  the links' sessions signal, no polling). The `Router` with each method's roles and timeout
  (`Answer`), the pages' `local` role always taken. The version a role says hello with is
  `akit <version>`; peers only show it.
- Tests (36): fit's frame, session, flow, edge and async tests (credit, a slow reader holding its
  own stream only, cancel reaching the handler, idle, keep-alive, close, push order, pings,
  `max_inbound`, drain, held streams, a peer's protocol errors, a window overrun, IDs wrapping, the
  pump holding a writer to a slow reader); links (passwords both ways, tokens, a wrong password,
  `max_conns`, the accept hook, 404, pending links per address, a silent peer dropped by the slow
  timeout, a handshake that never finishes closed with 4001); the mesh (both directions on one
  link, roles, the method timeout, streams, pushes in order, a kicked link dialed again, refusals
  of roles, passwords and an ID claimed under another role, tokens binding their members and a
  revoked token closing its link, a pool of two links and a drain, a link without hello closed,
  TLS with a pin and a wrong one, `configure`). Against fit's binary (`tests/fit_mesh.rs`, run with
  `AKIT_FIT_BIN`): an akit node links to a fit hub with the shared password, with a token fit
  issued (written `akitt1.`) and over TLS pinned, a wrong pin and a wrong password linking nothing;
  a fit node links to an akit hub with a role's password, with a token akit issued (written
  `fitt1.`) and over TLS pinned, pushes its info, and answers `/node/info`.
- Stopgap: a dialer drops a dead or too slow link with a close frame, which queues behind what
  waits, and akio-ws's client closes the TCP connection after its close timeout, 1 s here; a
  listener aborts at once through the connection's handle. akio API request A238 asks for
  `WsSender::abort`. Gone since akio `8a9601b9`: the dialer aborts, its 1 s close timeout dropped.
- Next: the roles on it, I105 (node), I106 (hub), I107 (proxy).

### 2026-11-27: I103, the hub's calls and status, tokens' expiry; I103b split

- The hub's calls (fit's `tunnel/hub/api.go` and the calls of `hub.go` it makes), on
  `akit_tunnel::hub::HubApi`, the handle the hub gives its pages through `Role::api` (a new method
  of akit-roles' `Role` with a default of `None`, and `Manager::api`): `status`, the JSON fit's
  `GET /api/status` answers (name, version, the listeners with the URLs peers dial and the
  certificate's fingerprint, the settings, the peers with their links' places, tokens, round trips
  and tunnels, the tunnels with their service, the address the node reached, when they opened and
  the bytes passed each way, `password_peers`); `refresh_node` (`node/info` on the node's link in
  the lowest place, the answer taken and the proxies told), `set_services` (`node/setServices`,
  which the node takes only with `allow_hub_services` and writes back to its member),
  `kick` (fit's notify with the reason as a JSON string, then each link closed once it is sent,
  those still registering too), `close_tunnel` (both ends told). A node or proxy kicked says so in
  its uplink's state (`Uplink::kicked`), and links again after `retry_interval`, as fit's.
- Tokens' expiry: the hub's sweep of expired tokens is scheduled again each time the config's
  tokens change (a generation supersedes the sweep scheduled before), so that a token's links close
  at its expiry, as fit's registry's timer does; before, the first sweep came a minute after the
  hub started, whatever the tokens. A revoked token's links close at once (the registry's closers,
  I101).
- Split: the console socket and web proxies are I103b. The console the socket carries is the hub's
  pages, which I109 serves. Decisions waiting for review: row 21.
- Tests: `the_hubs_calls_and_status` (the status of a hub with a node, a proxy and a tunnel; a
  node's info asked for again, a proxy refused; services set through a control thread that
  applies the node's edit as `akit run`'s does; a tunnel closed from the hub ending the client's
  connection; a proxy kicked and linking again), `a_tokens_expiry_closes_its_links` (fit's
  `TestTokenExpires`: a token expiring two seconds after its node linked closes its links, and it
  does not link again; tokens listed on the peers, no password peer).
- Next: I103b, or I102d.

### 2026-11-27: I102c, WebSocket links

- `ws://` and `wss://` links (`akit_tunnel::ws`, fit's `tunnel/internal/transport/ws`) on akio's
  A236: a hub's `ws` or `wss` listener serves akio-http's upgrade on its path (`/tunnel` by
  default; another path is answered 404), HTTP/1.1 alone, the handshake's head bounded at 16 KiB
  and 10 s, any `Origin`; a `wss` listener presents its certificate or the hub's, ALPN `http/1.1`,
  TLS 1.3. A node or proxy dials with akio-ws's client (`User-Agent: akit`), the handshake within
  `dial_timeout`, a `wss` dial checking the hub's certificate as a `tls` one does (a pin through
  `pin_sha256`, `ca_file` or the system's roots). Messages of at most 2 MiB are taken; a send goes
  out as binary messages of at most 1 MiB, the messages of one send kept together; no compression.
  A close code other than 1000, 1001, 1005 or 1006 is the link's error, as fit's.
- `ws::WsWire` is the link's `Wire` on a `WsSender`, as `ConnWire` on a TCP connection: the
  WebSocket's connection relayed (`set_relayed`), its queue bounded at `LINK_QUEUE`
  (`set_max_queued_bytes`), and `hold_reads`, `queued`, `on_drain` and `peer` mapped onto the
  sender's. The hub takes a WebSocket's link by the same bounds as a TCP one's (`accept_wire`),
  its access policy at akio's filter on the TCP listener.
- **Stopgap (akio request A238)**: akio-ws has no close without the handshake, so `WsWire::close`
  (a failed link, a kick, a revoked token, a role stopped) sends a close frame behind what is
  queued, where fit aborts; A238 asks for `WsSender::abort`. Gone since akio `8a9601b9`:
  `WsWire::close` aborts, and so does a refused link's WebSocket.
- Split: the hub's QUIC handshake bounds (`handshake_timeout`, `max_handshakes`' Retry), which
  akio-quic's server does not take yet, and a listener's certificate read again when its files
  change moved to I102d. akio API request A236 is marked done. Decisions waiting for review: row
  20.
- Tests: over `ws://` (12 MiB through a tunnel, four tunnels at once, another path answered 404
  and linking nothing, sealed by `body_password` on the default path), over `wss://` with the
  hub's pin (8 MiB) and a wrong pin (nothing opens), and a reader that stops holding its own
  tunnel while another on the same link echoes (`ws_links_a_slow_reader_holds_its_tunnel_alone`).
  Against fit's binary (`b6c5cebf`, a script in the session's scratchpad, not in CI): a fit hub
  with akit's node and proxy, and an akit hub with fit's, over `ws` and `wss` (pinned), plain and
  sealed: 4 MiB echoed through each of the eight.
- Next: I103.

### 2026-11-27: the stopgaps of A234, A235 and A237 go

- A234: every tunnel link's wire is bounded at fit's `LinkQueue` (`msg::LINK_QUEUE`, 64 windows of
  4 MiB and 4 MiB for control messages, about 260 MiB) as the link is made, as fit's `link.New`
  does (`Wire::set_max_queued`, a method with a default that does nothing, onto
  `Conn::set_max_queued_bytes`). The 8 MiB high water goes (`link::HIGH_WATER`, `Link::congested`,
  `Link::when_drained`), and with it the holds it put on a tunnel's end and on the link the hub
  reads from: the tunnels' windows alone bound what waits on a link, as in fit. A QUIC link's
  stream buffer (8 MiB, `quic::STREAM_BUFFER`) and its wire's overflow queue hold `LINK_QUEUE`
  together. Links stay relayed (`set_relayed`): akio's pause at `write_high_watermark` would stall
  two links writing to each other.
- A235: the TLS and QUIC links give their pin to akio-tls (`pin_sha256`) and start at once: a
  certificate that does not match fails the handshake, and what the link queued meanwhile never
  leaves. `tls::when_handshaken` and its 5 ms poll on akio's timer thread go, with `Link::deferred`
  and `Link::start` and the QUIC dialer's check of the leaf in `on_connected`.
- A237: `akit_observe::stop_client_engine` shuts the engine down and waits on the calling thread
  again (not on an I/O thread); akit-listener's `acme_shared`, which aborted before akio's fix,
  passes on macOS. Decisions waiting for review: row 18's outcome.
- akio API requests A234, A235 and A237 are marked done.
- Tests: a link bounds its wire at `LINK_QUEUE`; the TLS and QUIC e2e tests (a pin, a wrong pin
  opening nothing, 12 MiB over QUIC past its stream's buffer) pass unchanged.

### 2026-11-27: I93, the encrypted store's format and `akit store`; I93b and I93c split

- `akit_admin::store`: fit's `admin/store` byte for byte. The `fitstore` file (the magic `FITSTORE`
  kept: the format is fit's), a 59-byte header (version, Argon2id's time, memory and lanes, the
  salt, the nonce), the payload sealed with akio-crypto's XChaCha20-Poly1305 (A231) under a key
  from akio-crypto's Argon2id (A230), the header the additional data. Versions 1 to 3 read; 2
  written, 3 with sections, which akit does not read but keeps byte for byte (fit's assistant and
  cluster keep theirs there). The config kept as it was written: the payload's members found by a
  strict JSON reader of the module's own (RFC 8259, as Go's `json.Valid`), a config that is not
  JSON refused before anything is written. The header's Argon2id bounds (time 16, 1 GiB) for a
  store's own file, twice fit's settings for a file to unseal (an import); 64 MiB at most. Writes:
  a new random nonce, a temporary file beside the store made 0600, written, synced, renamed over
  it, the directory synced; an error leaves the store as it was. Exports sealed with a password of
  their own (`seal`, `unseal`) or the store's key. The key is derived when a store is opened (64
  MiB, a fair share of a second), on the control plane only.
- `admin.store` read and checked as fit's (`path` required, exactly one of `password`,
  `password_file`, `password_env`; the password never shown); `akit validate` takes it, and the
  gate's `examples/console` is valid now. `akit run` refuses it until I93b keeps the config in it,
  as it refuses the roles it does not run yet.
- `akit store export -c <config>` prints the config the store holds; `akit store import -c
  <config> <files>` checks the config the files hold with the bootstrap file's `admin` (compiled,
  as `akit validate`), refuses `admin` in it, and seals it as the store with a new salt, keeping
  the console users the old store kept when it opens. fit's `fit store`.
- Interop: `crates/akit-admin/tests/fitstore` holds stores fit's own `admin/store` code wrote
  (versions 1, 2 and 3 with sections, one with fit's own Argon2id settings, an export) and
  `vectors.json`; `opens_what_fit_wrote` opens every one, `keeps_what_it_does_not_read` writes the
  version 3 one back and keeps its sections and users. The other way: `akit_writes_for_fit`, with
  `AKIT_FITSTORE_OUT`, writes akit's stores (cheap settings, fit's own, sections, an export) and a
  manifest, which `TestAkitWrote` in the same directory's `akit_vectors_test.go`, run inside fit's
  `admin/store` package, opened with fit's code here: all four opened. fit's store tests ported
  (`store::tests`, 18), and `store_export_and_import` in akit's CLI tests (a store fit wrote
  exported too).
- Split: I93b (`akit run` with the store: the base config merged and written back, every config
  applied written, SIGHUP, `admin` from the bootstrap file, the console's password changes, the
  Admin API's `store` format) and I93c (`run -admin`, a `.store` given with `-c`, the store beside
  the config files and its key file). Decisions waiting for review: row 19.
- Benchmark: none (rule 24: the control plane); the dev plan's row names what to measure (the key's
  derivation at start, a 4 MiB config's write).

### 2026-11-27: akio pinned at `6151f5c5`: A234–A237

- akio: `6151f5c5` on akio's main, with A234 (a connection's own bound of its send queue,
  ADR-181), A235 (a pinned server certificate in akio-tls's and akio-quic's clients, ADR-182),
  A236 (a WebSocket's reads held, its drain heard, its peer's address, ADR-183) and A237
  (`Engine::wait` safe in a thread-local's destructor). akit's `rev` and `akit_runtime::AKIO_REV`
  move to it; nothing in akit had to change to build on it. The stopgaps for A234, A235 and A237
  stay until the iterations that replace them.

### 2026-11-27: akio pinned at `9e21ada7`: A220, A230–A233

- akio: `9e21ada7` on akio's main, with A220 (an accept filter on UDP listeners, bound UDP sockets
  and QUIC servers, akio's ADR-178), A230 and A231 (akio-crypto's Argon2id, BLAKE2b and
  XChaCha20-Poly1305, ADR-179), A232 (connection handles, close deadlines, a listener's accept and
  close hooks, ADR-180) and A233 (a terminal's echo turned off), and the logical pool's changes
  (admitted work resumed through a full pool, a pool thread that waits stood in for, a pressure
  watcher). akit's `rev` and `akit_runtime::AKIO_REV` move to it; nothing in akit had to change to
  build on it.
- **Stopgap (akio request A237)**: akio's `Engine::wait` now reads a thread-local of akio-pool's,
  which panics (and so aborts) when the wait runs in another thread-local's destructor: a graph
  whose last reference is a thread's cached one drops its client engines there (`acme_shared`
  aborted so). The client engines of OTLP, the providers and the shared ACME storage are stopped
  by `akit_observe::stop_client_engine`, which waits on a thread spawned for it (and not at all on
  an I/O thread), until akio reads its thread-locals with `try_with`.
- akit-middleware's `basic_auth_checks_off_the_io_thread` failed every run in release builds on
  gpu51, both backends (10 of 10 alone): its cost-8 bcrypt check, about a second in a debug build,
  took less than the 20 ms after which the requests that need none are sent, so the check passed
  first. The test now takes the lowest cost from 8 whose hash takes 200 ms there; 10 of 10 alone on
  each backend since.
- A220: a listener's UDP socket takes its access policy at akio's filter
  (`ListenerAccess::datagrams`): HTTP/3's the listener's own list then the global one, unless a
  route has a list of its own (its requests decided one by one then); a `udp` listener's the policy
  of the service its sessions go to. A refused peer's datagrams are dropped before a QUIC
  connection or a session is made, as fit's. A reload swaps the filter on the socket it keeps
  (`Engine::set_listener_filter`, `UdpBinding::set_filter`) and binds a new one filtered from its
  first datagram (`Engine::bind_udp_filtered`). The stopgaps go: the HTTP/3 dispatcher's per-request
  check (`Dispatcher::unfiltered`) and the `udp` entry's check as a session opens; decision 034's
  points 6 and 9 are replaced, and the config reference loses its two notes. The tunnel hub's QUIC
  listeners refuse at the filter too (`bind_udp_filtered`, swapped by `apply`), and the hub's
  check when a link's stream opens goes. Tests: `h3_peers_are_filtered` (refused peers unanswered
  and counted as filtered, a route's own list deciding per request, a reload swapping and binding),
  `udp_sessions` (a service's own list both ways, nothing accepted for a refused client, a reload
  swapping), the tunnel's `quic_links_refused_at_the_filter`.
- A232: the console's connections are fit's (`limitHandler`), on akio's connection handles. The
  listener's `on_accept` counts each connection (`console::slots`, fit's `login.Slots` and `Peers`
  ported with their tests: a quarter of `max_conns` kept for connections with a login, a quarter
  the most one client other than the host's own holds, the oldest unauthenticated connection of
  the client holding the most closed for a new one) and sets its deadline, 5 s on
  (`ConnHandle::close_at`); `on_close` frees its slot. A request with a valid session, a login
  that succeeds among them, cancels the deadline (2 minutes between requests from then on,
  akio-http's keep-alive timeout); one without, on a connection a session had kept, sets it again.
  A request served when its connection's time runs out is answered within as long again, with
  `Connection: close`. The engine's bound is twice `max_conns`, for connections being closed. The
  stopgap goes: an answer without a valid session no longer closes its connection, nor a logout
  or a 429. Decision 036's point 5 is replaced, the reference's note on `admin.console.max_conns`
  goes. Tests: fit's `TestConsoleIdleTimeouts`, `TestConsoleUnauthBoundFromOpen`,
  `TestConsoleUnauthBoundServing` (its grace set apart with `Options::console_grace`, so that a
  loaded host's check time does not decide it) and `TestLimitHandlerEvicts`. Not ported: fit's
  Admin API rules for connections without the token (decision 006 lists them), which the same
  slots could carry now.
- A233: `akit hash-password` at a terminal puts it in raw mode (`akio_sys::terminal::raw`, its
  guard putting it back) and reads the password twice as fit's `readMasked`: a `*` a character,
  Backspace taking back a character (all its bytes), Ctrl-U all of them, escape sequences dropped,
  Ctrl-C, Ctrl-D or the end of input "interrupted", at most 1024 bytes kept. The note that the
  password is shown goes; a pipe is still read a line. fit's `TestHashPasswordTerminal` ported
  (`asks_twice_masked`).

### 2026-11-27: I102b, QUIC links; the track stops at I102c, waiting on akio's A236

- `quic://` links (`akit_tunnel::quic`): akio-quic, the TLS links' configuration (ALPN
  `fit-tunnel`, TLS 1.3, no resumption), one bidirectional stream per connection, which the dialer
  opens and starts with fit's version byte (1), the hub strips; nothing else added, so fit's and
  akit's sides link to each other. A hub's `quic` listener is a UDP socket bound at run time on the
  listeners' engine (`Engine::bind_udp`; akio adds TCP listeners at run time, not UDP ones), served
  by a `QuicServer` holding at most the hub's links plus its `max_handshakes`; its connections are
  checked against the access policy and the hub's bounds when their stream opens (akio's A220
  would refuse them before a handshake). `idle_timeout` and `keep_alive` are the QUIC transport's;
  a node's or proxy's `quic_insecure` takes any certificate, its `pin` is checked in
  `on_connected`, before the stream opens (A235's stopgap without its poll).
- Backpressure: the stream's send buffer is the link's high water (8 MiB); a send akio-quic
  refuses past it waits in the wire's own queue, at most 24 MiB more (what a TCP link's connection
  takes past the high water), and goes out on `on_stream_writable`, after which the feeders the
  congested link held resume; holding the link's reads holds the stream's, whose flow control
  window stops the peer. Every call from outside the connection's handler runs under
  `without_driving`, so that no handler runs under the link's or the wire's lock.
- WebSocket links are I102c, and wait on akio API request A236: akio-ws's `WsSender` has neither
  `hold_reads` nor a drain callback nor the peer's address, so a WebSocket link could hold neither
  its peer nor its feeders (rules 8 and 9). I104's mesh links are WebSockets and wait on it too.
  The track stops there: I102c, I103 (whose console socket is a WebSocket) and I104–I109 are
  `todo`. The hub's QUIC `handshake_timeout` and `max_handshakes`' Retry and a listener's
  certificate read again when its files change moved to I102c.
- Tests: over QUIC with the hub's pin (12 MiB through, past the stream's buffer, and four tunnels
  at once), `quic_insecure`, a wrong pin (nothing opens); by hand against fit's binary: a fit hub
  over QUIC with akit's node and proxy pinning it, and an akit hub with fit's, tokens each way.
- Next: akio's A236, then I102c; or I103's parts that need no WebSocket.

### 2026-11-27: I102, TLS links and the hub's certificate

- `tls://` links: akio-tls, TLS 1.3 alone, the ALPN `fit-tunnel`, no session resumed
  (`resumption(false)`, `session_tickets(false)`). A hub's TLS listener presents its `cert_file`
  and `key_file`, read when the member is applied, or the hub's own certificate: `quic.cert` and
  `quic.key`, or a self-signed one made once (P-256, ten years, for `localhost` and the hub's
  name) and kept beside the config, `akit.tunnel.hub.pem` beside `akit.json`, mode 0600 (`Host::
  kept_file`, I100's prefix), its fingerprint logged; without a config file to keep it beside, it
  is made at each start and the log says so. A node's or proxy's link checks the certificate by
  `pin`, by `ca_file` (read when the uplink is made, on the control thread; fit reads it at each
  dial) or by the system's roots, never by anything in the address.
- The pin, until akio's A235: akio-tls checks a chain or nothing, so a pinned link's handshake
  runs with `insecure_skip_verify` (which still checks the server's signature with the leaf's
  key), the link is made `deferred` (nothing sent, its hello included), akio's timer thread polls
  `tls_info` every 5 ms until the handshake is done, within `dial_timeout`, and compares the
  leaf's SHA-256 with the pin in constant time; only then does the link start; a mismatch closes
  the connection unsent, "the hub's certificate does not match the pin".
- WebSocket and QUIC links split into I102b: their listeners and endpoints are still a warning
  when the role applies. A listener's certificate files are read when the member is applied, not
  again when they change (I102b).
- Tests: over TLS with the hub's pin (4 MiB through), a wrong pin (nothing opens), a hub started
  again with the same fingerprint, a CA-signed listener certificate checked by `ca_file` (and no
  certificate kept then); by hand against fit's binary: a fit hub over TLS with akit's node and
  proxy pinning it, and an akit hub with fit's, tokens each way.
- Next: I102b (WebSocket and QUIC links), or I103.

### 2026-11-27: I101, the tunnel's links, registration and tunnels over TCP

- A new crate, `akit-tunnel` (fit's `tunnel`): `crypt` (fit's `bodycrypt`), `msg` (fit's
  `proto`: the methods, the messages as JSON with fit's names, the password's MAC and the hub's
  proof, the tokens' transcript), `link` (fit's `link`: frames, calls, replies, notifies, hellos of
  versions 3 and 4, sealed bodies), `end` (fit's `tunnel`: each tunnel's end, its 4 MiB window,
  acknowledgements every 256 KiB once the connection's queue is under 1 MiB), `uplink` (a node's
  or proxy's pool of links to a hub: dial in order, register by token or password, check the
  hub's proof before serving it, ping, redial after `retry_interval`), `hub`, `node` and `proxy`.
  `akit run` runs the three (`akit::ROLES`); a config of roles alone gets an I/O thread to dial on.
- On akio: everything is a callback. A link is read in its connection's `on_data`; a call that
  waits (a node's dial of a service, a hub's open through a node, a proxy's open through a hub) is
  answered from the callback that ends the wait, with a `Responder`; calls' timeouts, retries,
  pings, idle sweeps and the tokens' expiry run on akio's timer thread (`after_send`); dials go
  through akio's resolver and Happy Eyeballs. Links and tunnels' connections are akio's "relayed"
  connections (`set_relayed`, fit's `SetWriteBackpressure(false)`): without it, two ends of a link
  each writing to the other stopped reading at akio's write watermark and waited for each other
  (found by the sealed-token test, which stalled at 64 KiB). Pacing is the windows and the link's
  high water instead: a link with more than 8 MiB queued holds what feeds it (a tunnel's
  connection, or the link the hub reads from) until it drains (`on_drain`).
- Where akit differs: fit raises a link's send-queue bound to the windows of its 64 tunnels
  (260 MiB); akio has one bound for the engine (`limits.max_write_queue_bytes`, 32 MiB), so a link
  holds its feeders past 8 MiB instead (written up as akio API request A234, a connection's own
  bound; nothing waits on it). TLS, WebSocket and QUIC links are I102's: a listener or endpoint of
  those is a warning when the role applies and is skipped. A hub's calls on peers (query, set
  services, kick), token expiry's links closed at expiry and the console socket are I103's; a
  revoked token's links close at once here already (the registry's closers).
- Interoperability (decision 038), checked by hand against fit's own binary at `7daee573` (a
  script in the session's scratchpad, not in CI): a fit hub with an akit node and proxy, and an
  akit hub with a fit node and proxy, over TCP with tokens, plain and sealed by `body_password`:
  a megabyte echoed through each.
- Tests: fit's `bodycrypt` and `proto.Prove` vectors (a body sealed under a password and under a
  token, a hub's proof) matched byte for byte; links in memory (calls, notifies, plain and sealed,
  mismatched sides, frames past the limit); end to end on one akio engine
  (`tests/e2e.rs`): 16 MiB and 8 tunnels at once with the password, 8 MiB sealed, tokens plain
  and sealed with their members bound, a wrong password, a token shared by two members; through
  the binary (`crates/akit/tests/tunnel.rs`): `akit run` with the Admin API and a hub,
  `akit token create` for a node and a proxy run by a second `akit run`, a tunnel through them,
  `akit token list` with the members bound, `akit token revoke` closing the proxy's links.
- To measure (rule 30, the benchmark table's new row): throughput of one and of 64 tunnels,
  tunnels opened a second, the hub's CPU per gigabyte, memory per tunnel and link, against fit.
- Next: I102, TLS, WebSocket and QUIC links.

### 2026-11-27: I100, the roles in `akit run`

- `akit-roles`'s `host`, fit's `app`: a `Manager` that, on every config applied and after the
  listeners, starts each role the config sets, applies the config to each running one and stops
  each it leaves out; a role's failure is a warning in the reload's answer and the gateway runs.
  A role gets a `Host`: the listeners' akio engine (no engine of its own), the drain timeout
  (`-shutdown-timeout`), the prefix of the files it keeps beside the config (`akit.tunnel.hub.pem`
  beside `akit.json`, decision 038) and `edit`, which changes its own member: the edit goes to the
  control thread through its queue, is applied there as any config with the member's path as its
  source, and its warnings or refusal are told back. The roles akit runs are `akit::ROLES`; a
  config that sets another member is refused by `run`, its reloads and the Admin API, and not by
  `validate` (which compiled configs refused roles since I98: moved out of `compile_config`). The
  roles stop before the engine.
- One address, one server (fit's I162, `config/bindings.go`): every address the config listens on,
  the served listeners (those HTTPS by default makes too), `admin`, `admin.console` and the
  roles' (consoles, `listen`, `api`, the hub's listeners and console socket, the proxy's forwards
  and web proxies), normalized (every address, `localhost`, IPv4-mapped) and checked once in
  `akit-config`'s `binds`, a clash reported at the later of two with the earlier named. akit
  checked none before: a config with two listeners on one port, which fit refuses at the pin, is
  now refused too. The bind-time probe fit adds for another process is not ported: akio's
  listeners bind as they always have.
- The Admin API's `GET /tokens`, `GET` and `POST /tokens/{member}`, `PATCH` and `DELETE
  /tokens/{member}/{id}` (fit's `admin/tokens.go`): reads on the API's thread, from the running
  config's member and the running role's registry (last uses, links) when it runs; changes on the
  control thread as edits of the member's `tokens`, logged to the `admin` logger. Unlike fit's,
  they work on a member the running config sets whether or not its role runs (fit answers 404
  until the role runs). Marked as v1 additions. `akit token create|list|expiry|revoke` (fit's
  `cli/token.go`) calls them on akio's HTTP client, the API's address, token and TLS read from the
  config's `admin`.
- Tests: the host (a stub role started, moved, failing, stopped; members no role runs refused;
  edits), the binds (fit's `TestBindsEveryKindClashes`, `TestBindsNormalized` and the tunnel's
  clashes), the tokens' endpoints end to end against the Admin API (issued and shown once, kept as
  a hash, listed, expiry changed, refusals, revoked), the v1 surface with the new endpoints.
- Next: I101, the tunnel's links over TCP. The CLI's end-to-end test comes with it, once a hub
  runs.

### 2026-11-27: I99, link tokens

- A new crate, `akit-roles` (fit's `app`, I100 adds the host): `linktoken`, fit's
  `internal/linktoken` (fit's decision 035). A token is `akitt1.<id>.<secret>`; its client,
  stored, server and body keys and both sides' proofs are fit's byte for byte, on akio-crypto's
  HMAC-SHA256, HKDF and SHA-256 (decision 038). The registry of a listener's tokens binds a
  member to the token it first linked with, refuses an unknown, expired or full token and a member
  bound to another, closes the links of a token revoked, rehashed or expired (`set`), and of one
  that expires while they are open (`sweep`, which the role calls every `next_sweep`, at most a
  minute: fit's timer is the role's here); a link's `Use` forgets it when dropped. Issuing,
  changing the expiry (`ttl` or `expires_at`), revoking and binding are edits of a member's token
  list, which the caller applies as a config.
- Where the plan changed: the Admin API's `/tokens` and `akit token` move to I100, with the host
  whose roles' registries they read; fit's CLI calls the Admin API, as akit's will.
- Tests: fit's `TestSecretRoundTrip`, `TestHandshake`, `TestRegistry`, `TestRegistryListOrder`,
  `TestRegistryFull`, `TestParseTTL`, `TestManager` and `TestBind` ported; fit's own vectors (a
  hash, a body key, a dialer's and a listener's proof, printed by a Go program calling fit's
  package at `7daee573`) matched byte for byte (`fits_vectors`).
- Next: I100, the roles in `akit run`.

### 2026-11-27: I98, `tunnel` and `llm` in the config

- The Tunnel and LLM track planned as I98–I109 (dev plan); [decision 038](decisions/038-tunnel-and-llm.md):
  akit's names where akit shows one, fit's wire protocols byte for byte, so that akit's and fit's
  roles link with each other.
- `akit-config`'s `roles` module: the six members (`tunnel.hub`, `tunnel.node`, `tunnel.proxy`,
  `llm.hub`, `llm.node`, `llm.proxy`) with fit's fields, defaults (methods, as fit's), bounds and
  checks (`config/tunnel.go`, `config/llm.go`, `config/linktoken.go`): endpoints taken apart and a
  query, fragment or user refused (I160's attacks), tcp and ws off loopback without
  `body_password`, `quic_insecure` off loopback, LLM listeners and peers off loopback without TLS,
  `quic_pin` read with fit's warning, tokens with their members bound once, models of each API.
  Link tokens are `akitt1.<id>.<secret>` (decision 037); `fitt1.` is refused.
- `akit run` refuses a config that sets a role until the iteration that runs it
  (`roles_not_running`); `akit validate` takes the six fit-examples (the gate: 55 valid, 24
  refused only as not supported yet). The clashes of two addresses are I100's (fit's I162).
- Tests: fit's `TestLLMChecks`, `TestLLMMembers`, `TestLLMDefaults`, `TestTunnelChecks`,
  `TestTunnelNames`, `TestLinkTokenChecks` and `tunnel/config_test.go` ported
  (`roles/tests.rs`); the gate samples each of the 376 fields with two configs that differ only
  in it (`tests/compat/tests/support/roles.rs`); 376 fields marked as v1 additions; the reference's
  Tunnel and LLM chapters.
- Next: I99, link tokens.

### 2026-11-27: I94, the Admin API's and the console's certificates read again

- Built before I93, which waits for akio's A230 and A231 and which I94 does not need.
- As fit's `admin/tls.go`: `admin.cert_file`, `key_file` and `client_ca_file`, and
  `admin.console.cert_file` and `key_file`, are read again when they change, and new connections
  are served with what they load, without a restart and without closing a connection; files that
  do not load (a certificate written and its key not yet, a client CA file with no certificate)
  are logged in the `admin` logger, `<admin|console>: <why>; the running certificate stays`, and
  are not tried again until they change.
- How (`crates/akit-admin/src/certs.rs`): each server's acceptor is a `Swap`, which asks akio-tls's
  `Tls` made from the configuration last loaded for each connection's transport (akio's
  `Acceptor::transport`, called per connection); the control thread replaces it under a lock held
  only for the swap. A changed client CA file is served too, which a certificate resolver alone
  could not do. No akio change was needed.
- Where akit differs from fit: fit looks at the files' sizes and times from the handshake, once a
  second at most; a look at a file blocks, and akit's handshakes run on an I/O thread (rule 1), so
  the files are looked at on the control thread by the listeners' certificate watcher
  (`akit_tls::FileWatcher`: every `-watch-interval` with `-watch-certs`, on by default, the files
  settled for 500ms, their contents compared with what was loaded), and SIGHUP reads them again at
  once. fit reads the console's certificate only when it starts; akit reads it again as the API's.
  `akit_admin::Server::watch_certs` and `reload_certs`; `akit run` calls them.
- Tests: fit's `TestTLSReload` ported for the API (`tls_reload`: renewed and read on SIGHUP, a key
  that does not load kept out through the watcher and not tried again, the files mended and loaded
  once settled, never before) and the console (`tls_reload`, `tests/console.rs`: renewed by rename
  through the watcher, a certificate without its key kept out, with the `admin` logger's lines);
  `tls_reload_client_ca` (a client CA replaced: the other CA's clients heard and the old one's
  refused; one with no certificate keeps the running one); end to end, `akit run` serving a renewed
  Admin API certificate without a restart, a half-written renewal leaving the running one
  (`admin_certificate_renewed_without_a_restart`, `tests/watch_certs.rs`). The config reference's
  notes, the Admin API guide (which still said the API had no TLS) and the console guide.
- Checked: `make check` on macOS (fmt, clippy, the workspace: 1,353 tests passed, deny, docs,
  compat, packaging); on gpu51 clippy and the workspace on epoll and io_uring (`--test-threads=4`),
  1,353 tests passed on each, none failed or hung..
- Nothing to measure: control plane (rule 24); the lock a connection takes at accept is the
  API's and console's own, never a listener's.

### 2026-11-26: I97, akit's names

- [Decision 037](decisions/037-akit-names.md), the maintainer's: every external name that said
  "fit" says "akit", with no alias kept. Renamed: the Admin API's `Akit-Config-Version` and
  `Akit-Dry-Run`; the `Akit-Error` trailer (proxy, FastCGI); the console's `akit_console_<port>`
  and `X-Akit-Csrf`; the export's `akit-<members>-v<n>.<format>`; the 17 metric families `akit_*`;
  the OTLP scope `akit` and the span attributes `akit.listener`, `akit.route`,
  `akit.upstream.address`; `Cache-Status`'s `akit`; the WebAssembly ABI (`akit_on_request`, module
  `akit`); nine defaults (`label_prefix`, `tag_prefix` `akit`; the KV, etcd and shared storage
  prefixes `akit/`; `controller_name` `lesismal.github.io/akit`; `realm` and `service_name`
  `akit`; `akit_sticky`), the Ingress annotations `akit.lesismal.github.io/*`, `jwt`'s realm; the
  default config file `akit.json`; `adapt`'s format `akit` (`-from`, `-to`). Kept: the config's
  field names and formats, `fitstore` (I93), Proxy-Wasm's `proxy_*`, "fit" naming fit in docs and
  messages, and `akit adapt`'s converter warnings, fit's byte for byte.
- The console's pages: `tools/console-pages.py` copies fit's workbench and applies a deterministic,
  listed set of renames (`RENAMES`, `KEEP`; written to `console/CHANGES`), each changed file
  marked with a notice (Apache-2.0 4(b)), fit's `LICENSE` kept; fit's schema renamed alike as
  `console/schema.json`, which `GET /api/m/admin/schema` serves. `--check` checks every file is a
  fixed point of the renames and, with a clone of fit, byte for byte fit's renamed. 50 of the 69
  files changed. Checked in the Browser pane: `akit run` with no `-c` read `akit.json`; the login
  page and title say "akit"; a form edit of `limits` checked (`?dry_run=1`) and applied as version
  2; the page read `Akit-Config-Version`; with curl, the cookie is `akit_console_<port>`, a POST
  with `X-Fit-Csrf` is refused 403 and with `X-Akit-Csrf` answered, the export named
  `akit-config-v2.json`, a dry run answered `Akit-Dry-Run: 1`.
- The WebAssembly test plugin: `tools/plugin-guest.sh` builds `headers-akit.wasm.gz` from fit's
  guest package and `testdata/headers` at the pin with only the `//go:wasmimport` and
  `//go:wasmexport` names renamed (Go 1.27.3, `-trimpath`, reproducible; `--check`); fit's build
  stays as `headers-fit.wasm.gz`, which a test shows akit refuses and the benchmark's fit server
  runs (`bench/lib.sh`, `bench/modules.sh`, `bench/fitbench/akit.go`). The bench's akit config is
  `bench/configs/akit/akit.json`.
- The v1 baselines written again once: `tools/v1-freeze.py --write-baseline` now writes the surface
  less the additions marked and takes akit's defaults from `tools/config-reference-notes.json`'s
  new `defaults` (9 items of `config.txt` changed); `admin-api.txt`'s two header fields renamed by
  hand; the sums updated. The config reference shows akit's defaults with fit's in a note.
- Docs: decision 037, decisions 002, 006, 007, 008, 032 and 036 amended, the migration guide's
  table of what a fit user changes, the WebAssembly guide's ABI section, the console, Traefik,
  nginx and Caddy guides, the compatibility page, the reference, the roadmap, the bench README,
  the compose example, the dev plan (I97).
- Tests: `make check` on macOS (fmt, clippy, the workspace, deny, docs, compat with
  the pages compared with fit's byte for byte, packaging); on gpu51 clippy and the workspace on
  epoll and io_uring (`--test-threads=4`), 1,348 tests passed on each, none failed or hung.
  Nothing to measure: the names do not touch a hot path; the next `wasm` benchmark runs akit on
  `headers-akit` and fit on `headers-fit`, the same plugin.

### 2026-11-26: I92, the Admin API over TLS; the track stops at I93

- `admin.cert_file`, `key_file` and `client_ca_file` read, with fit's `checkAdminAddress`: off
  loopback only with a token and TLS (the error names what is missing), the two files together,
  `client_ca_file` only with them, none with a Unix socket; `mptcp` still refused. The API is
  served over TLS (`akit_tls::server_config`, TLS 1.2 and 1.3), every client's certificate
  verified against `client_ca_file` when it is set (akio-tls's client authentication, required);
  an address with an empty host listens on every address; the guard takes the API's own origin as
  `https://` over TLS. Marked with `make v1-bless` (3 additions). The threat model's L1 is closed
  (the console over TLS with logins, I90, and the API over TLS, I92), with a section on the
  console's controls; L5 stays accepted (decision 036, 6).
- Tests: fit's `TestTLS`, `TestTLSClientCA` (no certificate, another CA's, the right one) and
  `TestTLSStartErrors` ported (`crates/akit-admin/tests/admin.rs`); the config's checks.
  `TestTLSReload` waits for I94: the files are read when akit starts.
- The track stops at I93, the encrypted store: fit's `fitstore` format derives its key with
  Argon2id and seals with XChaCha20-Poly1305, and akio-crypto (`077a6710`) has neither: akio
  requests [A230](akio-api-requests.md#a230-argon2id-in-akio-crypto) and
  [A231](akio-api-requests.md#a231-xchacha20-poly1305-in-akio-crypto). Until then the console's
  password endpoints answer 409, and the transfer page's `store` format is refused. I94 (the
  certificates read again when they change) follows I93 in the plan, buildable without akio.

### 2026-11-26: I91, the console's pages

- fit's workbench served unchanged ([decision 036](decisions/036-console.md), 8):
  `tools/console-pages.py` copies fit's `app/workbench/web` (69 files, 2.5 MB) and its licence at
  the gate's pin (`7daee573`) into `crates/akit-admin/console`, and writes `console/files.rs`,
  whose `include_bytes!` build them into akit; served from memory with fit's content types and
  `Cache-Control: no-cache`, `index.html` for `/`. `make compat-check` runs its `--check`, and a
  test checks the built files against the directory; `tests/compat/update-fixtures.sh` runs it when
  the pin moves. `GET /api/m/admin/schema` answers the gate's `fit.schema.json`.
- The Admin API's text endpoints, fit's (`admin/check.go`, `admin/transfer.go`), in
  `akit_admin::text`: `POST /config/render` and `/config/parse` (`.conf`, YAML, JSON, through
  `akit_config::adapt::document`), `PUT /config/text` (`members=`, `dry_run`, `If-Match`; the
  members a text sets checked against the page's), `POST /config/export` (`admin` left out,
  secrets redacted as the reads redact them) and `/config/import` (`admin` kept). A text's
  `[redacted]` secrets are put back as `PUT /config` puts them back; its errors are placed at their
  lines in the text (`akit_config::position_of`, new), as fit's adapter places them. The `store`
  format is refused until I93. Marked as v1 additions. The freeze test now reads the dispatcher's
  paths from `Api::serve`, where I90 moved them. A change with no body is answered (it hung before).
- Checked in a browser (the Browser pane, `akit run` with a console): the login, the overview from
  `/status`, the Logs page (its files, **Rotate now**), a `.conf` tab (render and parse, a parse
  error at its line), a form edit of `limits`, the versions and a rollback; each change in the
  `admin` logger. The menus of fit's features akit refuses, and the pages whose endpoints akit
  lacks (`/node`, `/config/check`), show as decision 036 says.
- Tests: fit's `TestTextRenderParse`, `TestTextParseErrors`, `TestTextPartSave`,
  `TestTextWholeConfig` and the text formats of `transfer_test.go` (`crates/akit-admin/tests/admin.rs`),
  `pages_and_schema` (`tests/console.rs`). The guide's pages section. Next: I92.

### 2026-11-26: I90, the console's server

- `akit_admin::console::server` (fit's `app/console` and `admin/console.go`; [decision
  036](decisions/036-console.md)): akio-http on an engine of its own, one I/O thread, HTTP/1.1, over
  TLS from `cert_file` and `key_file` (`akit_tls::server_config`, TLS 1.2 and 1.3);
  `max_conns` at accept; `admin.console.access`, then the global access, at accept
  (`Graph::console_access`, set again by every config applied, `Server::set_console_access`;
  `admin_warning` leaves it out). Started by `Server::start` with the Admin API when
  `admin.console` is set, bound first; `akit run` prints `akit: console on <url>`.
- fit's endpoints: `POST /api/login` (JSON within 4 KiB; admitted by the logins, then checked on a
  pool of its own, `akio::Pool` with one thread and 16 waiting, 503 past it; 429 with `Retry-After`
  and `retry_after`), `POST /api/logout`, `GET /api/session`, `GET /api/workbench`
  (`{"user", "modules": ["admin"]}`), `GET /api/m/admin/users`, the password endpoints (409: no
  store), and `/api/m/admin/<path>`: the Admin API's dispatch on the same request, its target
  rewritten (`Context::req_mut`), past its token and `Host` checks, its configs applied as source
  `console` (`Shared` gained a source; `Submit` became an `Arc` inside, so both share it). fit's
  cookie (`fit_console_<port>`, `HttpOnly`, `SameSite=Strict`, `Secure` over TLS), `X-Fit-Csrf`,
  the origin check and fit's headers on every answer. The `admin` logger: logins and where from,
  failures, logouts, and each change with the version it applied or its status (a response
  filter, so whoever writes the answer logs it).
- Stopgap until akio's [A232](akio-api-requests.md#a232-an-http-connections-idle-timeout-and-close-from-its-handler-and-a-table-of-the-connections-accepted):
  no 5 s bound on a connection without a login and no eviction past `max_conns`; an answer to a
  request without a valid session closes its connection.
- Tests: `crates/akit-admin/tests/console.rs` (fit's `TestConsoleLoginAndEdit`,
  `TestConsoleLoginRefused`, `TestConsoleSessions`, `TestConsoleFailureTableBounded`, TLS,
  access at accept and swapped); `crates/akit/tests/console.rs` under `akit run` (a login and a load
  through the console in the log, the global access and `admin.console.access` following loads).
  The guide `docs/guides/console.md`; the compatibility page's Admin API section. Next: I91.

### 2026-11-26: I89, the console's config and logins

- `admin.console` read as fit's `config.Console` (`akit-config`): `address` (127.0.0.1:2020),
  `cert_file` and `key_file` (together; an address off loopback refused without them), `users`
  (at least one; a name, no name twice; a hash that starts as bcrypt's, `$2`), `session_timeout`
  (30m), `session_lifetime` (12h), `max_sessions` (64), `max_conns` (32), `access`; `mptcp`
  refused as not supported yet. A config of `admin` alone, with no listener or route, was already
  valid. A user's `Debug` leaves its hash out. Marked with `make v1-bless` (16 additions); the
  gate's `examples/console` is now refused only for `admin.store` (`make compat-bless`); the
  reference has a section on the console, its tables generated.
- `akit_admin::console::login`, fit's package `login`: bcrypt checks (`akio_crypto::bcrypt`), two
  at once, an unknown name checked against a dummy hash at the users' cost; lockouts by address (an
  IPv6 /64), doubling to an hour, one check from an address at a time, 4,096 addresses kept and a
  full table never ending a lockout early; a user's checks slowed after 10 failures in a row, names
  of no user counted together; sessions of 32 random bytes kept by their SHA-256, ended idle, past
  their lifetime, at logout or as the oldest; a CSRF token each, compared in constant time; the
  cookie's name with the port. Times are durations from an origin a year back, so that a time never
  set is long past, as Go's zero time. fit's `login` tests pass ported (15), with a fake clock.
- `akit hash-password` (`-cost`, 10): the first line of standard input; from a terminal, asked
  twice. fit turns the terminal's echo off, which takes `tcsetattr` (unsafe): akio request
  [A233](akio-api-requests.md#a233-a-terminals-echo-turned-off-to-read-a-password). Stopgap until
  then: from a terminal the password is seen as typed, and the prompt says so and how to hide it.
- akio requests written for the track: A230 (Argon2id) and A231 (XChaCha20-Poly1305), which I93
  waits for, A232 (the console's connection bounds, for I90) and A233.
- Tests: `cargo test` of akit-config, akit-admin's `console`, akit's `cli` and the gate; clippy and
  fmt. Next: I90.

### 2026-11-26: akio pinned at `077a6710`: A200 and A180

- akio: `077a6710` on akio's main, with A200 (absolute-form request targets, akio's ADR-177) and
  A180 (a WebAssembly guest call 44% cheaper: fit's Go `headers` guest 9,089 to 5,068 ns a call on
  gpu51, wazero's compiler 3,520 ns), and akio main's renames (`handlers_on_logical_pool`,
  work-stealing scheduling), which akit used under no old name. Every akit request but A220 and
  A225 is on akio's main and pinned.
- A200: akit-listener's `absolute_form` rewrite (in `Dispatcher::call` and `body_mode`) and the
  router request's host override are gone: akio gives the request in origin form with the target's
  host. The access log writes `Request::raw_target()`, the target as sent, as fit's `RequestURI`
  (`every_request_has_its_line` gains an absolute-form request). One difference from fit: an http
  URI with no host (`GET http:///x`) is answered 400 by akio (RFC 9110 section 4.2.1), where Go's
  server routes it by its `Host` field; `absolute_form_targets_name_their_host` says so
  ([decisions waiting for review](decisions-pending.md), row 15). The plan's benchmark row for the
  listener's check goes with it.
- A180: nothing to change in akit; the `wasm` scenario's numbers are left to a benchmark task.

### 2026-11-26: I80b, Logging (I81–I83) and Access lists (I85–I87) merged

- akio: unchanged, `6bcd8ad6`. The three branches merged together: the plan's index and track
  table, the status rows and the log keep all three; the gate's inventory, the config reference
  (485 fields of fit's read) and the v1 additions (57) regenerated from the merged sources; the
  threat model's L6 says access lists are built. The next free iteration number is I89 (I84 and
  I88, reserved for the two tracks, unused).
- Tests: `make check` on macOS; on gpu51 clippy and the workspace on epoll and io_uring. In the
  io_uring run akit-middleware's `basic_auth_checks_off_the_io_thread`, `breaker`'s
  `stops_and_resumes_a_proxied_upstream` and `compress_proxied` failed while another session's
  io_uring tests ran beside them; alone they passed 3 of 3.

### 2026-11-26: I80b, security hardening: the threat model's limitations (doing)

- **L2, inline secrets redacted** (`crates/akit-admin/src/redact.rs`): `GET /config` and its
  pointers, `/versions/{n}` and `/config/effective` (and so the dashboard) answer `[redacted]` for
  each string under a member fit's `secretName` names a secret's (fit's console, assistant and
  cluster rule: `password_hash`, `secret`, `token`, `Authorization`, `*_key`, ...; not `*_file`,
  `max_*`; `rate_limit`'s `key` selectors kept; a push target's `url` redacted). A config with no
  secret is answered byte for byte; one with some as fit writes an edited config; the `ETag` is the
  config's. `POST /load`, `PUT` and `PATCH` put back the secret the running config keeps where the
  body sends `[redacted]` (matched by member, `name`, or index, as fit's `restoreSecrets`), and
  refuse one with no secret there (400). fit's own Admin API does not redact: [decision
  035](decisions/035-admin-api-redacts-secrets.md), the compatibility page and the Admin API guide
  say so.
- **L3**: `akit_config::hardening::secret_files` warns of a file named by `key_file`,
  `tls_key_file`, `token_file`, `password_file`, `hmac_secret_file` or `kubeconfig` that its group
  or others have any permission on (once a file), at `akit run`'s start, in `akit validate` and in
  the `warnings` of every change applied or checked (the runner's `apply` and `check` take the
  config's JSON for it). `FileStorage` tightens its root and every directory under it that it writes
  into to 0700 when open to others (best effort) and syncs the directory after each rename.
- **L4**: `hardening::cleartext` warns of an `http://` ACME directory off loopback, a shared storage
  reached in the clear off loopback (Redis without `tls`, etcd or Consul over `http://`: its
  password or token and the private keys it stores), a Consul token or an etcd password sent over
  `http://`, Docker over `tcp://`; the Kubernetes provider logs a kubeconfig's `http://` server or
  `insecure-skip-tls-verify` off loopback when it finds the cluster (`RestConfig::exposed`).
  Warnings only: nothing fit runs is refused.
- **L6, L13**: notes in the config reference at `forward_proxy` (no authentication; private ranges
  reachable unless denied, with the `deny` line to write) and `compress` (BREACH). The plan named a
  forward proxy guide, which does not exist: the reference's section is where the proxy is
  documented.
- **L7**: fit's `static` follows links wherever they lead, on purpose, with no option; akit keeps
  it, documents it at `static` and tests it (`symlinks_out_of_the_root_followed`).
- **L12**: fit's config has no head or body timeout field; closed as it is.
- **L14**: `cache_purge_guarded`: without the token, with a wrong one, and with an `Origin` (even
  the API's own), `POST /cache/purge` is refused and purges nothing; with the token it purges.
- **Moved**: L8 (akio-tls already has `client_crls` and `client_ocsp_policy`; fit has no field) and
  L9 to fit, `tls.client_auth.crl_files` and `module_sha256` proposed in decision 035; L15 asked of
  akio as [A225](akio-api-requests.md#a225-an-external-security-review-of-akios-cryptography-tls-http-parsers-and-webassembly).
  The threat model's L10 and L11, closed by the akio pin and I79b, are marked so too.
- Docs: [the threat model](security/threat-model.md) (the controls and tests, the limitations'
  states), [the review scope](security/review-scope.md), decision 035, [decisions waiting for
  review](decisions-pending.md) row 14, the dev plan's I80b status.
- Tests: `make check` on macOS (fmt, clippy, the workspace, deny, docs, compat, packaging). The gpu51 runs (clippy, the workspace on epoll and io_uring) did not run: an akio benchmark held `/root/bench.lock` exclusively; they are owed before the merge.
- Next: the external review's findings, which finish I80b.

### 2026-11-26: I83, log files on reload, and the Admin API's `/logs`

- **Reload**: `Root::set_config`, fit's `SetConfig`: the levels (and akio's maximum level), the
  zone, and each of the log's files' rotation and retention, in place; a file no longer named
  keeps writing, unrotated. `akit run`'s reload calls it for SIGHUP, `-watch`, the certificate files
  and the Admin API alike, and warns once when a logger's output or format changed ("log's outputs
  and formats take a restart; the running ones stay"); the access log's file took its rotation and
  zone in place since I81.
- **Admin API**: `GET /logs` (`{"outputs": [...]}`: each file output of `log` and the access log,
  its loggers, format, zone, `rotate` with its defaults, the current and rotated files with sizes
  and times, the total, the writes dropped and the last error; at most 1,000 files an output,
  `"truncated"` past it), `POST /logs/rotate` (`{"path"}`) and `POST /logs/cleanup`
  (`{"path", "dry_run"}`, `{"removed": [...]}`), fit's names and bodies; a path that is not one
  of the running config's file outputs is refused 400, so nothing else is listed or removed. Each
  runs on the API engine's logical pool (akio-http's `detach_to_pool`), never on its I/O thread.
  `Options::logs` hands the server akit's `Files`. The four endpoints are marked as v1 additions in
  `tests/compat/v1/additions.txt`. `akit_observe::time::rfc3339_nano` for the times.
- **Tests**: akit-admin's `logs` unit tests (the bodies; a listing cut at 1,000 files and its
  total); `v1_surface_is_frozen` with the additions; akit's `log` e2e
  (`reload_changes_levels_zone_and_rotation_in_place`: from UTC to the local zone, eight hours
  east, debug for akio's lines and a file shared with the access log that turns at 300 bytes, then
  a format changed warned of; `the_admin_api_lists_rotates_and_cleans_up`: the listing, a rotation
  now, a dry run that removes nothing and a cleanup that removes what it listed, a path not the
  config's refused).
- **Test runs of the track**: `make check` on macOS (fmt, clippy, the workspace, deny, docs,
  the gate and the v1 freeze); in its first run akit-listener's `udp_sessions_expire` failed once
  (a session's idle timing under the machine's load, a test this track does not touch) and passed 5
  of 5 alone, and the workspace then passed with `--no-fail-fast`. On gpu51 clippy, and the
  workspace on epoll and on io_uring (`--test-threads=4`), every test binary under
  `tools/test-timeout.sh`: all passed, no hang.
- **Docs**: the Admin API guide and the compatibility page list `/logs`; the migration guides from
  fit, nginx and Caddy no longer say `log` and `access_log.rotate` are not supported.
- **Differences from the plan**: fit's workbench Logs page (I157) waits for the Console track,
  as planned; its endpoints are here. Times in the answers are UTC (fit's are its process's
  zone). The track is done in three iterations; I84 is unused.
- **Next**: the Access lists track, the security review, I80b.

### 2026-11-26: I82, akit's own log: fit's `log`

- **Config**: `log` read as fit's `config.Log` (`level`, `levels`, `output`, `format`, `time_zone`,
  `rotate`, `loggers` with their own `level`, `output`, `format`, `rotate`), with `level_of`,
  `output_of`, `format_of`, `rotate_of` and `file_outputs` as fit's; fit's checks with fit's
  messages: an unknown logger, a level that is none, a logger's level in `levels` too, `rotate`'s,
  loggers sharing a file with other rotation settings (fit's order and its quirk: a logger naming
  the log's file must repeat its `rotate`), and the access log sharing a file of the log's with
  another `rotate` or zone (`sharing_problems`); `log.time_zone` loaded as the access log's is.
  `LOGGERS`, fit's fourteen names.
- **akit-observe `logger`**: `Root`, a level, format and output for each logger, a file through
  I81's `Files` (one sink a path, shared with the access log); fib's `line` and log/slog's `text`
  and `json` (RFC 3339 with nanoseconds, numbers bare), the time in the zone; the split of debug
  and info to standard output and the rest to standard error by default; `install`, `log`,
  `enabled`, `lines` (a callback for the crates) and the `error!`, `warn!`, `info!`, `debug!`
  macros; before `install`, `akit: <message>` on standard error. `bridge_akio` installs akio's
  logger: akio's records are `engine`'s, warn and error as they are, info and debug at debug.
- **Messages**: ACME, OCSP and the shared storage (`tls`), Docker, Kubernetes, Consul and etcd
  (`provider`), OTLP exports (`telemetry`), FastCGI's stderr (`fastcgi`), the access log's drops
  (`access_log`), the file sink and the sockmap relay (`engine`), the metrics' overflow and a file
  output with no `rotate` (`server`); akit-tls, akit-provider, akit-handler and akit-l4 depend on
  akit-observe for it. `akit run` makes the log before the access log, which shares its files, and
  closes it last on every way out.
- **Tests**: akit-observe's `logger::tests` (the three formats in two zones, an escaped line,
  attributes; levels and a logger's own file; a file shared with the access log's sink); akit-config's
  `log` (defaults and what each logger takes, every check, the shared file, fit's `logging`
  example); akit's `log` e2e (a warning on standard error in `line` by default; the log and the
  access log in one file, JSON in UTC, every line whole; `akit validate` refusing an unknown zone).
- **Gate and docs**: the `log` fields in `supported.txt` and marked as v1 additions; fit-examples'
  `logging` valid (49 of 80 valid now); the gate's reference check counts a map's values
  (`log.loggers.*`) as an object; the config reference's Log chapter and tables in both languages,
  with notes on the formats, the levels' `engine` and crates' messages, and zones.
- **Differences from the plan**: akio's info records are the engine's debug, and the crates'
  callback lines carry no level and are written at warn (decision 033, points 8 and 9); akit's
  command lines stay plain, the few fit writes through loggers included (point 10). fit's logger
  `plugin` has no message yet.
- **Next**: I83, log files on reload and the Admin API's `/logs`.

### 2026-11-26: I81, log files: rotation by size and time, retention, compression

- **Plan**: the Logging track broken into I81–I83 in both plan files (fit's I155–I157; fit's
  workbench Logs page waits for the Console track, its Admin API is I83), with rows in the index
  and I81's in the benchmark table. [Decision 033](decisions/033-log-files.md).
- **akit-observe `files`**: `Files`, a sink a cleaned path with a count of the logs holding it
  (`LogFile`, released on drop); a queue of 1 MiB or 256 buffers, `write` and `write_owned`
  refusing (`Refused::Full`, counted) past it, a thread of the sink's own writing. Rotation by
  `max_bytes` and by `interval` periods from midnight in a zone (`period_start`, fit's
  `periodStart`, tested at UTC and fixed offsets, an hour, a quarter and two days); rotated names
  in UTC with `-N`; a failed rotation retried after a minute; retention at open, after rotations and
  every ten minutes, oldest first, by age, number and total size, only names the sink makes;
  gzip by akio-deflate on a compression thread started with the first one (queue 16, a minute's
  wait), a temporary file created exclusively; `rotate_now`, `cleanup_now(dry_run)`, `sync` and
  `info` for I83. Files are opened only as regular files with one link, the same file the name held
  (no `O_NOFOLLOW` in akio-sys: decision 033, point 4), `O_NONBLOCK` against a FIFO; new ones with
  `O_EXCL`; a directory others may write is warned of. `Zone::fixed` for fixed offsets.
- **Access log**: `AccessLog::to_file` writes each flushed buffer into the sink without a copy,
  a refused one's lines counted as dropped. `akit run` opens a file output through `Files`, warns
  only when it has no `rotate`, and a reload changes the file's rotation and zone in place
  (`output`, `format`, `flush_interval`, `buffer_bytes` still take a restart).
- **Config**: `Rotate` (fit's pointer fields as `Option`s) and `Rotation` (the defaults filled in,
  fit's `Rotate.Equal`), `access_log.rotate` read with fit's checks (a negative limit, an
  interval under 1s, or over 24h and not whole days); `is_file_output`, `output_path` (Go's
  `filepath.Clean`) and `zone_name` for I82's checks.
- **Tests**: akit-observe's `files::tests` (size, a line larger than `max_bytes`, a period turning
  at midnight in UTC-4 and again when the zone changes, `period_start`, retention by number, total
  and age with a dry run, gzip, one sink a path and other settings refused, a link and a hard link
  refused and a link at a rotated name skipped, a full queue counted, the names);
  akit-config's `access_log` (`rotate`'s defaults, fields, `.conf` sizes and errors); akit's
  `access_log` e2e (`a_rotated_file_keeps_every_line`: 60 requests in rotated, gzipped files, each
  line in exactly one; `reload_changes_the_retention`: `max_files` 100 to 2 on SIGHUP).
- **Gate and docs**: `access_log.rotate` and its fields in `supported.txt`, marked as v1 additions
  (`make v1-bless`); the config reference's Access log chapter (rotation, naming, retention, an
  example) and a table for `access_log.rotate`, in both languages; `tools/config-reference.py`
  keeps fit's description's verb ("until they fit").
- **Differences from the plan**: none. The sink waits, when it closes, up to 30 s for its files'
  compressions, so `akit run` stopping leaves no temporary file (fit's pool is not waited for).
- **Next**: I82, akit's own log.

### 2026-11-26: I87, access lists III: the Admin API and the reference

- **`admin.access`** read with fit's checks, and refused with an Admin API on a Unix socket in
  fit's words. The graph compiles the API's policy, `admin.access` then the global access
  (`Graph::admin_access`), its files watched with the rest; a config without `admin` still has its
  global access apply to the API akit started with, as fit's.
- **At accept, following every config applied**: `akit run` builds the API's listener with the
  policy (`Options::access`, akio's filter) and sets it again after each config is applied, by the
  Admin API, SIGHUP, `-watch`, the providers or the watched files (`Server::set_access`), though the
  rest of `admin` takes a restart; a change to `admin.access` alone is not warned of as one
  (`admin_warning`). A Unix socket's peers are never checked. Decision 034, point 11.
- **Docs**: the reference's chapter on access lists covers the Admin API (with fit's advice to
  allow `127.0.0.1` and `::1` in `admin.access`), the Admin API chapter and guide say so, with an
  example; the migration guide no longer lists `access` as unsupported; the roadmap's Access track
  is done. The threat model's L6 is I80b's to update.
- **The gate**: 5 fields read (`admin.access` and its four), marked with `make v1-bless`. Of fit's
  `access` fields, those of the roles and their consoles (`admin.console.access`, `llm`, `tunnel`,
  `webrtc`, `streaming`, `bittorrent`) wait for their tracks, refused whole with them.

Tests (ported from fit): akit `admin_access.rs`, end to end (`TestAdminAccess`: `admin.access`
denying, the global access applying, `admin.access`'s allow keeping local access, each through the
Admin API's loads and SIGHUP; `TestAdminAccessFile`: a deny file watched and applied when it
changes; `TestAdminAccessUnixSocket`: the error with a Unix socket, and its peers never refused);
akit-admin `admin_access_takes_no_restart`; the gate's samples; the examples of the reference and
the Admin API guide.

Checks for I85–I87: `make check` on macOS (the whole branch); on the benchmark host, clippy and the
workspace on epoll and on io_uring (`--test-threads=4`), every test binary through
`tools/test-timeout.sh`: 153 test binaries passed on each backend, none failed or hung.

### 2026-11-26: I86, access lists II: services and the forward proxy

- **`services[].access`** read with fit's checks; the graph compiles each listener's services'
  policies (the service's own list, the listener's, the global one) as fit's `compileAccess`
  ([decision 034](decisions/034-access-lists.md), points 8–10).
- **At accept**: a `tcp` listener of one service without `server_names`, on a port of its own and
  without PROXY headers, by that service's policy, through akio's filter as in I85.
- **Later, in akit-l4's entry** (`Entry::with_access`, `Entry::admits`): a listener whose
  services pick by server name, that shares its port with HTTP or SOCKS, or that takes PROXY
  headers, checks each connection once its service is known (after the PROXY header and the
  ClientHello), and closes a refused one before its upstream is dialed. This also closes I85's gap:
  a `tcp` listener with PROXY headers now goes by its access.
- **Stopgap (akio request A220)**: a `udp` session from a refused client is closed as it opens, its
  datagram dropped; each later datagram opens and closes another, nothing remembered of the peer.
- **SOCKS**: a listener's `socks` asks the current graph, at each client's request, for the policy
  it goes by when the listener decides later (`Rules::socks_gated`, fit's `SOCKSAccess`).
- **Docs**: the reference's chapter covers services and SOCKS, with a service in its example and a
  note on `services[].access`; decision 034 extended.
- **The gate**: 5 fields read (`services[].access` and its four), marked with `make v1-bless`.

Tests (ported from fit): akit-listener `access_l4.rs` (`TestAccessService`: a `tcp` listener and
one shared with HTTP, the service's list overriding the global one both ways, the HTTP side
answering 403 or 200; services picked by server name with lists of their own; a `tcp` listener
taking PROXY headers refusing the address its header names; a `udp` service refusing and serving;
SOCKS beside HTTP refused and served); akit-config `access.rs` and `services.rs`; the gate's
samples; the reference's example. `make check` on macOS; the Linux runs are in I87's entry.

### 2026-11-26: I85, access lists I: the lists, listeners and routes

- **akit-access**, a crate of its own ([decision 034](decisions/034-access-lists.md)), fit's package
  `access`: inline entries and list files (addresses and prefixes, ranges, eMule `ipfilter.dat`
  lines up to level 127, PeerGuardian `.p2p` lines, at most 64 MiB a file and 16 KiB a line, a bad
  line naming its file and line) compiled into sorted, merged ranges of each family looked up by
  binary search; `Policy::chain` of the lists that apply, the most specific first, each deciding or
  having no say as fit's.
- **The config**: `access` at the top level (one file's), on listeners and on routes
  (`akit_config::Access`), each entry checked as fit's router parses it, each file's path not empty;
  `.conf` and YAML read it as JSON does.
- **The graph** compiles each listener's access as fit's `compileAccess`
  (`akit_runtime::access::ListenerAccess`): at accept when the listener's own list and the global
  one are all there is and no PROXY header names the client, per request otherwise. The list files
  are read then (a file that cannot be read fails the config, `validate` included) and their sums
  join `cert_files`, so `run -watch-certs` applies the config again when one changes.
- **The listeners**: akio's accept filter (A3) is built into a listener's TCP side and swapped by
  every reload on the listeners it keeps (`Listeners::gate`), which are not bound anew. A request a
  route's own list, or a listener taking PROXY headers, refuses is answered fit's `403 forbidden`
  before its handler runs, logged and counted with its route, its body not buffered; a path with no
  canonical form is refused before its 400, as fit's.
- **Stopgap (akio request A220)**: akio's UDP listeners take no accept filter, so HTTP/3 requests
  are checked one by one against the policy the TCP side has at accept and answered 403, where fit
  drops a refused peer's datagrams (`Dispatcher::unfiltered`). Until I86, a `udp` listener and a
  `tcp` listener taking PROXY headers go by no access, and `services[].access` is refused as not
  supported yet.
- **Docs**: the reference's chapter on access lists (both languages) with an example `akit
  validate` checks (`lists/deny.txt` added to the examples' sandbox), a note on `listeners[].access`;
  A220; decision 034; akit-access in the workspace layout and the architecture.
- **The gate**: 15 fields read (`access`, `listeners[].access`, `routes[].access` and their four
  fields each), marked with `make v1-bless`; the gate's outcomes unchanged (no fixture sets
  `access`: fit's templates have it commented out).

Tests (ported from fit): akit-access's unit tests (`TestPolicyLayers`, `TestChainEmpty`,
`TestDenyBeatsAllowInOneList`, `TestEdges`, `TestSetAgainstLinear`, `TestFiles`,
`TestCompileErrors`, `TestRangeOf`, `TestParseLine`, `TestP2PAndDATFiles`, `TestRangesAgainstLinear`,
and a list of 100,000 prefixes); akit-config `access.rs`; akit-listener `access.rs`
(`TestAccessRefusedAtAccept`, `TestAccessRoute`, `TestAccessFollowsReload`,
`TestAccessProxyProtocol`, list files watched and failing, HTTP/3 refused per request); the gate's
samples; the reference's example. `make check` on macOS; the Linux runs (epoll, io_uring) are in
I87's entry, which ran the whole branch.

### 2026-11-26: akio pinned at `6bcd8ad6`: the open requests, I79b and the A169/A171 cost fixes

- akio: `6bcd8ad6` on akio's main, merging the akio branches of the open requests
  (`claude/akit-open-requests`: A12, A22, A24, A29, A30, A35, A45, A49, A54, A75, A78, A79, A117,
  A148, A170), of I79b (`claude/i79b-akio`: A168, A214, A215) and the removal of A169's and A171's
  costs (akio's ADR-176; akio's main had made the same HTTP/2 default its own the same day, with
  `run_on_pool` renamed `handlers_on_logical_pool`). akit's `rev` and `akit_runtime::AKIO_REV`
  move to it; every request of [akio API requests](akio-api-requests.md) but A180 and A200 is on
  akio's main and pinned. The branches that built only against an akio branch build against the pin.
- Decisions: [decisions waiting for review](decisions-pending.md) row 11 brought up to date (the A/B
  done and the costs removed), row 13 for this merge.
- Tests: `make check` on macOS; on gpu51 clippy and the workspace on epoll and io_uring. In the
  full runs `sni_passthrough_spliced` (`sockmap`, both backends) and io_uring's FastCGI
  `cut_connections` failed once each; alone they passed 5 of 5 on each backend and 3 of 3: timing
  under the shared host's load, not the merge.
- The dev plan's benchmark table lost stale copies of the cache, FastCGI and WASM rows an earlier
  merge had left; the zh status table's "What works" row lost a repeated span, and the English one
  a missing separator.

### 2026-11-26: I79b, the gateway examples' last fields

- **akio first** (akio branch `claude/i79b-akio`, cut from akio's main `1b85fd83`, not merged or
  pinned yet; akit's `Cargo.toml` still pins `0fc8f3a7`): A214 `Config::max_pending_bytes`
  (akio's ADR-172: a watcher thread sums the I/O threads' queued-bytes gauges every 1–10 ms while
  a budget is set; connections pause through `ConnInner::read_held`, which every backend asks, and
  are woken through their loops; `budget_pauses`, `pending_bytes`), A215
  `Config::max_accepted_connections` (ADR-173, on the counters of A30's `Telemetry::open_accepted`,
  whose hunks the branch takes as they are on `claude/akit-open-requests` so that the two merge),
  A168 `HttpConfig::http1` (ADR-174: `false` serves HTTP/2 with prior knowledge alone, anything
  else gets a `GOAWAY`). Tests on every backend; the accept and read paths measured by a same-run
  A/B in akio's docs/BENCHMARKS.
- **`limits.max_pending_bytes`** read as fit reads it (bytes, or a share of memory such as
  `"25%"`, half the memory by default; fit's messages), `akit_config::Memory`, onto akio's budget at
  start; a reload that changes it is warned of. The memory a share is of is read by akit as fit's
  `sysmem` (Linux `/proc/meminfo` and the cgroup's limit; elsewhere fit's 256 MiB fallback),
  [decision 032](decisions/032-limits-on-akio-budget.md). `/metrics`'
  `fit_reads_paused_total{reason="budget"}` is akio's count.
- **`limits.max_conns`** counts the client connections alone: akio's `max_accepted_connections`
  in place of `max_connections`, so the upstream connections akit dials take no room from clients.
- **A listener of `h2c` alone** (`protocols: [h2c]`, or `[h2c, tcp]`): HTTP/2 with prior knowledge
  and nothing else, as fit's; on a shared port its HTTP rule is the preface alone.
- **The gate**: before, 80 fixtures, 45 valid, 34 refused only as not supported yet (26 fields);
  after, 48 valid, 31 refused (24 fields): `grpc`, `limits` and `proxy` are valid, and only the
  After-1.0 tracks' examples are refused (`bittorrent`, `console`, `llm-*`, `logging`,
  `media-pages`, `streaming`, `sync-*`, `tunnel-*`, `video-site`, `webrtc-*`, `whip-whep`).
  `limits.max_pending_bytes` is marked in `tests/compat/v1/additions.txt` (`make v1-bless`); the
  config reference's note on `max_conns` is gone, `max_pending_bytes` and `protocols` have notes.

Tests (ported from fit): akit-config `limits.rs` (`TestMemory`, `TestLimits`), `json_config.rs`
(h2c alone, beside `tcp`), `v1.rs` (fit-examples' `limits` in `.conf`); akit-listener `limits.rs`
(`TestPendingBytesBudget`; the engine's settings), `listeners.rs` (`TestProtocols`), `reload.rs` (a reload adds a listener of h2c alone),
`one_port.rs` (h2c alone beside `tcp`), `sysmem` (unit); akit-proxy `proxy.rs` (`max_conns` with
the upstream connections open), `grpc.rs` (calls on a listener of h2c alone); akit-compat `gate.rs` (the field's sample); akit `admin.rs`.
Checked on macOS and on the benchmark host, akio's suites and akit's workspace on epoll and
io_uring. akio's same-run A/B: HTTP/1.1, pipelined, h2c and TCP echo within 1% of akio's main with
both settings off and on; its `connect` case is bound by the client's reuse of TIME_WAIT ports on
loopback, not by the accept path. `akit`'s `watch` test (`each_edit_is_applied`) waited for a
stdout line before a stderr one, which a loaded host can swap; it now takes them in either order.

Differences from the plan: none in scope. No benchmark in akit (rule 30): the
[benchmark table](dev-plan.md#benchmark-and-optimization-status) has I79b's row.

### 2026-11-26: I80, security review readiness

- akio: unchanged, `0fc8f3a7`. No akio API request, ADR or decision number taken but decision 030.
- **[The threat model](security/threat-model.md)** and its Chinese twin: what akit is for the model
  (one process, the data plane on akio's I/O threads, the control plane off them, akio the release
  binary's only dependency); the assets (keys, the ACME account key, the Admin API's token,
  credentials akit holds, the config, traffic, route decisions, availability, the host); the actors
  and trust boundaries (clients, upstreams, the operator, local processes, web pages, providers and
  stores, the services akit calls, plugins); the assumptions (the config and the host are trusted);
  then, boundary by boundary, each attack with the controls akit has against it, naming the code and
  the tests (every test named was checked to exist); and **15 known limitations** (L1–L15), each
  with its plan: I80b for akit's, the Console and Access lists tracks, akio A24 and A214, and akio's
  own external review (L15: akio's TLS, X.509, cryptography and QUIC have had akio's internal review,
  not an external one).
- **[The review's scope](security/review-scope.md)** and its Chinese twin: the target (akit's commit,
  akio's pin, platforms, configs), the questions, the areas in scope with their code and first
  questions (TLS handling with akio-tls as akit uses it, certificates, ACME, on-demand TLS and OCSP;
  the Admin API and dashboard; the plugin sandbox; request parsing and the proxy's header handling;
  the eBPF sockmap path; providers and shared stores if time allows), I77's 35 fuzz targets by area,
  what is out of scope (akio's internals, what akit does not build, the operator's own input,
  denial of service past the limits, what is not shipped), a section on akio and the order in which
  we would review it, how to build and run akit, and how to report findings.
- **[SECURITY.md](../SECURITY.md)** and its Chinese twin, after akio's: supported versions (the latest
  commit before 1.0; from 1.0 the latest minor release, and the one before it for high and critical
  fixes for 3 months), private reporting through GitHub, response targets, coordinated disclosure,
  scope, and the hardening in place. Both READMEs link it.
- **Supply chain** ([decision 030](decisions/030-supply-chain-checks.md)): `cargo deny check` clean
  (its one warning, Apache-2.0 allowed but matched by no crate, answered with
  `unused-allowed-license = "allow"` and its reason in `deny.toml`), `cargo audit -D warnings` clean
  (74 crates in the lock file, no advisory, none ignored). A new workflow,
  `.github/workflows/security.yml`, runs both on every push to `main`, every pull request and daily,
  with `tools/release-deps.py`: it walks the release binary's graph (normal and build edges, every
  target's `cfg` but `cfg(loom)`) and fails on a registry crate without a decision note. At I80 the
  binary's graph is 40 packages, akit's and akio's, none from crates.io; the lock file's 30 registry
  crates are development dependencies and akio's loom models.
- **A bug found and fixed**: on-demand TLS with an `https://` ask URL never worked. Its client was
  built with `ClientConfig::builder()`, which in akio-tls starts with no CAs, so every ask failed
  its handshake and every on-demand name was refused (it failed closed). `OnDemand::new` now gives
  it the system's CAs, read as the config is compiled (`RootCerts::system`), never on an I/O thread;
  `OnDemand::with_roots` takes them from the caller. The new test `https_ask` serves an ask URL over
  HTTPS with a test CA: answered with that CA, refused with none, and an `http://` ask URL reads no
  CAs; it fails without the fix.
- Found by the survey and recorded as limitations rather than changed in I80 (I80b): no redaction
  in the Admin API's config reads; secret files' permissions unchecked; plain-text control-plane
  transports allowed; the forward proxy's private ranges and lack of authentication (as fit);
  symbolic links out of a static root; no revocation check of client certificates; no WASM module
  digest; no test of `POST /cache/purge` behind the token.
- Benchmarks: none (docs, CI and a control-plane fix; rule 24's "none").
- Tests: `cargo test -p akit-tls` (the on-demand tests with `https_ask`); fmt, clippy, `cargo deny
  check`, `cargo audit -D warnings`, `tools/release-deps.py` and `scripts/check-docs.sh` on macOS.

### 2026-11-26: akio requests A78, A22, A170, A24, A49, A35, A148 built, their stopgaps dropped

- akio: built on akio branch `claude/akit-open-requests` (`22bc4402`; ADR-155, ADR-156,
  ADR-157), not on `main` yet. akit uses them; the `rev` in `Cargo.toml` stays at akio main's `0fc8f3a7` until
  that branch is on akio's main, so this commit builds only against the branch (the coordinator
  moves the pin). A breaking change of akio's: `akio_http::BodyEvent` gained `TooLarge` (the enum
  is exhaustive), which every body sink of akit's now matches (the proxy's, the WebSocket
  tunnel's, the FastCGI handler's, the Admin API's, the tests').
- A24: an HTTP/1 response that says `Connection: close` closes its connection once sent, its
  reads held and no request pipelined behind it served. The Admin API's 401 (`guard`) and
  `read_body`'s 413s say it, as fit's Admin API closes such connections; the A24 comments are gone.
- A49: `Context::limit_body` bounds a body for whoever reads it. akit-listener calls
  `limit_body(max_body_bytes)` for every streamed body but a CONNECT's (a tunnel's or an extended
  CONNECT's stream is no body), in place of the `MAX_BODY_BYTES` local, which is gone from
  akit-runtime with every reader of it (the proxy, the FastCGI handler, the cache's own requests).
  `body_limit` calls `limit_body(max_bytes)` for every request; a body declared or buffered past
  it is still answered by the middleware, in fit's words (akio's own 413 says "Payload Too
  Large"), with `Connection: close` when it streams, the rest of it not read. The proxy hears
  `TooLarge`: 413 when its response has not begun, the upstream request given up either way
  (cancelled, which cuts a response that began, and no failure of the address's); the FastCGI
  handler fails its exchange with 413 alike.
- A35: a streamed HTTP/1.1 response keeps its `Content-Length` and is framed by it: the proxy
  keeps an upstream's on every version (a response with trailers still goes chunked on HTTP/1.1,
  which alone carries them), the FastCGI handler the application's (one that is not a number is
  dropped). A body cut short closes the connection short of the length.
- A148: the FastCGI handler's `SERVER_ADDR` and `SERVER_PORT` are the address the request came to
  (`Request::local`), as fit's from its TCP connection; with none (a Unix-domain socket, port 0) as
  before. Over HTTP/3 it is the UDP socket's, where fit leaves `SERVER_ADDR` empty.
- A78: akit-l4 gives each relay's two connections and each UDP session's two sides the service's
  idle timeout, the data they send counted (`Conn::set_idle_timeout`,
  `Conn::set_idle_counts_writes`), and akio closes them on their I/O threads. The sweep of every
  connection on akio's timer is gone (`Relay::touch` on every chunk with it); it stays for spliced
  connections alone, whose bytes akio never sees (their akio idle timeout turned off). akit-listener
  no longer raises the engine's UDP idle timeout to the services' (`udp_idle`, at start and on
  reload): it is HTTP/3's peers' alone.
- A22: a reload removes and changes listeners serving h3 or `udp`, those akit started with too:
  `ReloadError::Udp` is gone. The engine's own UDP listener is drained with
  `Engine::shutdown_listener`, a socket a reload bound with `UdpBinding::shutdown` (no longer
  closed with its connections): new peers are dropped, HTTP/3 connections get `GOAWAY` and finish
  their requests. One replaced by a listener on its own address drains for at most
  `UDP_REPLACED_DRAIN` (2 s), as the new peers the kernel hashes to its sockets go unanswered
  meanwhile; one removed with nothing in its place drains for the whole drain timeout
  ([decisions waiting for review](decisions-pending.md), row 12).
- A170: akit-l4's sockmap splices with `Relay::tunnel` (what of the first write it did not write
  goes through the engine) and follows the splice with `Tunnel`: `moved` for the idle timeout,
  `close_after_forward` for a side's end (on akio-ebpf's drain thread), `late_payload`. akit's
  `Spliced`, its counters, its drain on akio's timer and `Conn::inner` are gone. A connection that
  began with a PROXY header the listener read is spliced too: the tunnel's base counts what
  userspace read. [Decision 010](decisions/010-sockmap-relay.md) updated.
- Tests: akit-admin's `token` (a request pipelined behind a 401 not served) and `loads_refused`
  (closed after a 413); akit-middleware's `body_limit_h2` (h2c: under, at and over the limit,
  declared and growing, to a buffering handler and to the proxy) and `body_limit_closes_http1`;
  akit-handler's `streamed_body_limit` and `server_address` (and `SERVER_ADDR`, `SERVER_PORT` in
  `params`); akit-proxy's `stream_download` and `broken_download` (framed by the length);
  akit-listener's `tcp_one_way_is_not_idle` and `udp_listener_removed` (`l4`), `reload_drains_h3`
  and `reload_knows_h3_listeners` (`h3`: a startup h3 listener removed, and replaced on its
  address; a request in flight answered); `client_proxy_header_spliced` (`sockmap`, which runs
  spliced only as root on Linux). The workspace on macOS against that akio branch (a cargo
  `[patch]` passed on the command line, not committed): 1138 passed, 1 ignored; clippy passes for
  macOS and for `x86_64-unknown-linux-gnu` (the Linux-only code checked), fmt passes.
  `build_with_a_module_and_serve` (`crates/akit/tests/build.rs`) fails, as it builds a package of
  its own against the pinned akio, until the pin moves.
- To measure (benchmark task, rule 30): the `tcp` and `udp` services' throughput and connection
  rate against `dad77a7a` (akio's idle timer per connection, writes sampled, in place of akit's
  sweep and a clock store per chunk); a proxied download of a known length over HTTP/1.1 (framed
  by its length, no chunk headers); proxied uploads with a `body_limit` (akio counting the bytes
  in place of the proxy); the sockmap scenarios (`Tunnel`'s drain thread polling at 1 ms in place
  of akit's backoff on akio's timer); a reload under HTTP/3 load (no request lost, the replaced
  listener's 2 s drain).

### 2026-11-26: akio requests A12, A79, A45, A117, A30, A54, A29, A75 built, their stopgaps dropped

- akio: built on akio branch `claude/akit-open-requests` (`57a942af`, `22e038cc`, `8caab91c`,
  `a88c5d7b`; ADR-154, ADR-158, ADR-159), not on `main` yet. akit uses them; the `rev` in
  `Cargo.toml` stays at akio main's `0fc8f3a7` until that branch is on akio's main, so this commit builds only
  against the branch (the coordinator moves the pin).
- A117: akit-cache's disk pool is `akio::Pool` (`akio::{Pool, PoolConfig}` from the facade); the
  `akio-pool` dependency is gone from the workspace and akit-cache.
- A30: `fit_connections_open` is akio's `open_accepted`: the accepted connections alone, no longer
  the upstream connections dialed on the engine too.
- A12, A79: every dial carries its own connect timeout (`Engine::dial_with`; `HappyEyeballs` dials
  a lone address with its `connect_timeout` too), so akit-listener no longer sets the engine's
  `connect_timeout` to the shortest of the upstreams'; an upstream with a longer one is no longer
  cut to it. akit-l4 dials a TCP upstream's IP literal with `dial_with` and the upstream's
  `connect_timeout`; the probes keep their timer for the check's timeout in all (resolution
  included).
- A45: `akit_upstream::timer`, akit's own timer thread, is deleted; its callers use
  `akio::timer::after_send`: the health probes, the h2 pools' idle sweeps, the cache's lock and
  disk-read timeouts, the JWK sets' refreshes and refetches, the FastCGI sweep, the WebSocket
  tunnels' idle timeouts, the layer 4 services' opening timeouts and idle sweeps, the drain of the
  services a reload removes, and the sockmap drain (which A170 may replace). A timer whose job is
  moot is cancelled (`SendTimer::cancel`): a probe's timeout once it settles, a layer 4 opening
  timeout once a service takes the connection or it closes, the cache lock's once its fetch ends,
  a disk read's once it answers. akit-middleware and akit-proxy depend on `akio` for it (it was a
  dev-dependency of theirs).
- A54: a drained listener's idle HTTP/1 connection answers a request within akio's
  `HttpConfig::idle_drain_grace` (250 ms, akio's default; fit has no knob for it) with
  `Connection: close`: `crates/akit/tests/reload.rs` counts every client error as a failure, the
  retry counting gone.
- A75: `forward_auth`, `basic_auth`, `jwt` and `wasm` no longer ask for a streamed body whole to
  suspend its request: the body's reads are held while it waits and it streams to the handler
  after (the `jwt` middleware's `may_wait` goes with it). A Proxy-Wasm filter's `end_of_stream` is
  told from the fields for a body not read yet, as fit tells it (`Content-Length` above 0 or a
  `Transfer-Encoding`).
- A29: a request's access log line and its count are written by the listener's response observer
  (`HttpAcceptor::observer`, `akio_h3::serve_with_observer`) from two words it carries
  (`Context::set_observed`: the graph's ID; its metrics slot, its route's index and whether it is
  logged), with no allocation; `GraphCell::with_id` finds the graph a request was dispatched on
  when a reload overtook it (one dropped since: the current graph's log and registry, no route
  name). Kept on a response filter: a traced request (I64: its span's IDs and the tracer do not fit
  in two words) and a logged request whose route may change its target (`rewrite`, `wasm`:
  `Middleware::changes_target`, `CompiledRoute::keeps_target`), whose line writes the target the
  listener dispatched (fit's `RequestURI`), copied as before. The observer sees the response after
  every filter, as it goes. The authenticated `user` is still not logged (two words carry no name).
- Tests: `connect_timeout` in akit-listener's `upstreams` (a lone address held to 300 ms with the
  engine's 10 s, and another upstream's 1.5 s not cut to it); `forward_auth_streams_the_body` in
  akit-middleware's `forward_auth` (1 MiB streamed through `forward_auth` to the proxy's upstream,
  whole and in order, the upstream having the request before the second half is sent; it fails
  with the old whole-body override); the reload test without retries; the access log, metrics and
  tracing tests on both paths. The workspace on macOS
  against that akio branch (a cargo `[patch]` passed on the command line, not committed): 1131
  passed, 1 ignored; `build_with_a_module_and_serve` (`crates/akit/tests/build.rs`) fails, as it
  builds a package of its own against the pinned akio, which lacks these APIs, until the pin moves.
  fmt and clippy pass.
- To measure (benchmark task, rule 30): the proxy and static scenarios with the access log on, and
  with `metrics` on, against `dad77a7a` (the observer in place of a boxed filter a request); a
  load spanning reloads (the observer's lookup of an overtaken graph takes the cell's locks).

### 2026-11-26: I77, fuzzing

- akio: unchanged, `dad77a7a`. One request, **A200** (an absolute-form request target taken as
  RFC 9112 asks), requested only; akit-listener's rewrite below is its stopgap.
- `fuzz/` (cargo-fuzz, a workspace of its own, nightly): **35 targets**, one or more for each
  parser akit owns, each calling its crate's `fuzzing` module (feature `fuzzing`, `#[doc(hidden)]`,
  never in a release build). As in akio, each crate's `fuzzing::tests` replays `fuzz/corpus/<target>`
  on stable, so `cargo test` checks every seed and every input that once failed, on every platform.
  - Config readers (fit's `FuzzLoad`, `FuzzAdapt`, `FuzzConvert`): `config_json`, `config_conf`,
    `config_yaml`, `config_value` (I02–I26's, moved onto the crate's module), `convert_nginx` and
    `convert_caddyfile` (JSON out, warnings with their line, read without a panic),
    `config_adapt` (`akit adapt`: a config written in every format reads as the same config).
  - Router (`FuzzParseExpr`): `router_expr` (an expression prints back to one that parses to the
    same and matches the same requests), `router_path` (a canonical path is a fixed point, with no
    dot or empty segment), `router_find` (the router against a reference that tries every route).
  - Proxy (`FuzzHopByHop`, `FuzzForwarded`, `FuzzGRPCTimeout`): `proxy_headers` (no hop-by-hop
    field or one `Connection` names either way, the forwarding fields the proxy's own, a
    `Forwarded` element without control characters, no line broken), `proxy_grpc_timeout`.
  - Cache (`FuzzDirectives`): `cache_semantics` (directives, `Vary`, tags, freshness, the
    preconditions, the locations invalidated, purge keys), `cache_disk` (the disk tier's files
    both ways).
  - L4 (`FuzzReadSNI`): `l4_hello` (the service a ClientHello picks, however it is split),
    `l4_forward` (the forward proxy's rules: an internal address or the host's own never let
    through unless `allow` holds it). The PROXY protocol and ClientHello parsers are akio-mux's,
    fuzzed in akio (`proxy_header`, `mux_tls_hello`).
  - Handlers (`FuzzReader`): `fastcgi_response` (records read whole or in pieces, the CGI header),
    `fastcgi_paths` (`SCRIPT_FILENAME` and `PATH_TRANSLATED` under the root), `static_path` (the
    static handler's scan of a raw path agrees with the decoded one; CONNECT targets).
  - Middleware: `jwt_token` (tokens from every place, and claims signed with the test secret so
    that they are checked), `jwks_keys` (JWK sets and PEM keys), `middleware_fields` (Basic
    credentials, the rate limiter's client against fit's walk back over `X-Forwarded-For`,
    `Accept-Encoding`, path escapes, a rewrite of any bytes).
  - And: `observe_access_log` (a log line is one line, its JSON an object), `observe_escapes`
    (JSON, Go and logfmt quoting, `traceparent`), `observe_tzif`; `tls_redis_reply`, `tls_acme`
    (stored certificates, ARI times); `provider_kubernetes`, `provider_docker`, `provider_consul`,
    `provider_kv`, `provider_merge` (whatever the API sends, a part the merge reads, merged onto a
    config akit reads); `admin_pointer` (JSON pointers, merge patches); `upstream_keys` (hash
    policies and sticky cookies pick an address that is up); `plugin_pairs` (Proxy-Wasm's
    serialized header maps).
- Seed corpora checked in (`fuzz/corpus`): hand-written seeds per target, fit's own fuzz corpora
  (`testdata/fuzz`, decoded), fit's converter test files and akit's converter tables, akio's
  `mux_tls_hello` hellos; then what the runs below found, minimized with libFuzzer's `-set_cover_merge=1`, the
  hand-written and regression seeds kept as they are: 22,682 files, 12 MB. `fuzz/run-all.sh [SECS] [PAR] [targets]`
  runs them (`merge` folds a run's finds back).
- CI: `ci.yml`'s fuzz job runs every target for 60 s, four at a time; the new `fuzz.yml` runs each
  for ten minutes every Sunday and on release tags, in four shards, carrying the grown corpus in
  the Actions cache, the failing inputs kept as artifacts (fit's `fuzz.yml`).
- Run: on gpu51 (`flock -s`), every target for 300 s, eight at a time (2026-11-26), from 66 k
  executions (`router_expr`, which compiles path regexps) to 86 M (`proxy_grpc_timeout`); no
  crash, leak or timeout in akit's code; after the oracle of `admin_pointer` was corrected, it
  and the new `plugin_pairs` ran 300 s more (9.4 M and 50.5 M executions). Locally on macOS,
  every target for 15 s before.
- Findings, each fixed with its regression test:
  - **A route-protection bypass**: a request whose target is in absolute form
    (`GET http://a/admin/x HTTP/1.1`) was routed by the whole target, which no path route matches,
    so a catch-all route took it, while the proxy asks its upstream for `/admin/x`. fit (Go's
    server) routes it by its path and takes its host (RFC 9112 section 3.2.2). The listener now
    rewrites such a request to origin form with the target's host before routing, in `call` and
    `body_mode` (`absolute_form`); `path_confusion` gains absolute-form targets,
    `absolute_form_targets_name_their_host` and the unit test `absolute_form_targets`. Found when
    `cache_semantics` was given a request path that was not one. A stopgap until akio does it
    (A200); a benchmark-status row names the same-run A/B left to the benchmark tasks.
  - **`rewrite` corrupted a path**: one with a byte that is not UTF-8 and a character of the
    block its stand-ins took (U+10FF80 to U+10FFFF) came out with the character turned into a byte.
    The stand-ins now take a block of private use planes 15 and 16 that neither the path nor the
    replacement uses (`stand_in_base`); a path holding every one is left as it is.
    `characters_of_the_stand_ins_block_are_kept`; found by `middleware_fields`.
  - Not a bug, recorded: when a host matches two `**.` patterns of the same priority, the router
    tries the longer suffix first and passes over a longer path prefix of the shorter one, though
    an exact path of the shorter still wins; fit does the same (`router.go`'s early `break`), so
    akit keeps it (rule 19) and `router_find` checks only that the route picked matches. Worth a
    look in fit.
  - The rest were the targets' own oracles, corrected: fit (as Go) writes one U+FFFD per invalid
    byte; a TZif offset past a day is allowed (RFC 8536 only asks it), the log's times saturate;
    `-` names a member of an object (RFC 6901).
- Made `pub(crate)` for the targets: the cache's `same_origin`, `url_key`, disk `encode` and
  `decode`; FastCGI's `header_end`, `parse_head`, `origin_form`, `unescape`, and `try_files`'s
  pure step `candidate`; the static handler's `scan`; the forward proxy's `target`; the
  middlewares' `credentials`, `client_ip`, `negotiate`, `Rewrite::path`; akit-l4's `Opening`;
  Consul's `Instance::from_json`, etcd's `watch_message`; the Admin API's pointer `string`;
  Proxy-Wasm's `window`. No new dependency (libfuzzer-sys stays in `fuzz/`).
- Checked: `cargo test` of the 13 crates touched (macOS), fmt, clippy with and without
  `fuzzing`, `cargo deny`, the docs check.
- What should be measured (rule 30): the listener's absolute-form check, a same-run A/B of the
  fixed scenario over HTTP/1.1 (one comparison a request; no cost expected).

### 2026-11-26: I79, v1 freeze and compatibility

- **The policy**, [docs/compatibility.md](compatibility.md#v1-what-stays-stable) and its Chinese
  twin, fit's v1 for what akit serves: what is covered (the config, schema version 1, with
  akit's extensions; the Admin API v1; the command line; metrics; the access log), what is not
  (messages, the dashboard, WASM's host functions and the Proxy-Wasm subset, the converters'
  coverage, akit's Rust crates' API), what a 1.x release may do, `schema_version`, renamed fields
  and how deprecations work, the Admin API v1's endpoints (fit's `/tokens`, `/media/...` and
  `/streaming/...` answered 404 until akit runs those parts, then added), and what a new version
  brings. The gate's page keeps its second half.
- **The freeze**, [tests/compat/v1](../tests/compat/v1) ([decision 029](decisions/029-v1-freeze.md)):
  `config.txt`, the config's surface (5,181 items: every field of fit's schema version 1 with
  type, requiredness and default, the enums' values, 8 deprecated names, the 428 fields akit reads,
  the `sockmap` extension), generated by `tools/v1-freeze.py --write-baseline`; `admin-api.txt`, the
  Admin API's (26 endpoints, 2 uncovered for the dashboard, 8 header fields, `dry_run`, and the 27
  members of a change's, an error's and `GET /versions`' answers); `additions.txt`, empty, for the
  changes marked since (`added`, `retired`). `tools/v1-freeze.py` (CI's lint job, `make
  compat-check`) holds both baselines' SHA-256 and fails on any difference of the config's surface
  not marked; `make v1-bless` marks compatible additions and refuses while anything breaks v1 (tried
  on a copy: a field no longer supported, a default changed, a required field added, a baseline
  edited, each failed; a field newly supported failed until blessed). akit-admin's
  `v1_surface_is_frozen` (CI's test jobs) probes every method on every frozen path, reads the paths
  the dispatcher names in `server.rs`, and checks header fields and answers' members (tried: an
  endpoint retired, one added and a member retired, each failed).
- **Renamed fields**: the old names fit's I77 renamed (`cache`'s `max_size`, `max_entry_size`,
  `disk.max_size`; `compress`'s `min_length`; `jwt`'s `jwks_refresh`; `wasm`'s `max_memory`), which
  akit already read, now come with fit's warning at the old name's key, file, line and column
  (`Config::warnings`, `akit_config::Warnings`, equal whatever they hold so that configs compare by
  meaning); both names given is fit's error. `akit validate` and `akit run` print them, at start and
  on each reload, and the Admin API returns them in a change's `warnings`.
- **The gate** (I79b for the rest): `limits.max_header_bytes` (akio's HTTP/1, HTTP/2 and HTTP/3
  limits, 431; fit's 1 MiB default where akio's is 64 KiB; fixed in each listener's binding, so a
  reload that changes it rebinds), `limits.max_conns` and `limits.max_write_queue_bytes` (akio's
  `max_connections` and `max_queued_bytes`, at start), and `engine` read onto akio's engine
  ([decision 028](decisions/028-engine-on-akio.md): `event_loops` as each listener's I/O threads, the
  pool from `workers`, `min_workers` and `task_queue`, `read_buffer_size`, `write_watermark`,
  `backlog`; `loop_wait`, `poll_batch` and `kernel_tls_offload` refused). A reload that changes what
  is taken at start says so in its warnings. The reference has an Engine chapter and notes on each
  difference.

The gate's numbers: before, 80 fixtures, 45 valid, 34 refused only as not supported yet with 30
fields refused, 1 skipped; after, the same fixtures valid and refused, 26 fields refused. Every fit
template (6) was valid before and is; the examples still refused are `limits` and `proxy` (only
`limits.max_pending_bytes`, akio API request A214), `grpc` (h2c alone, A168), and the examples of
the After-1.0 tracks (`bittorrent`, `console`, `llm-*`, `logging`, `media-pages`, `streaming`,
`sync-*` (16), `tunnel-*`, `video-site`, `webrtc-*`, `whip-whep`).

Checked: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, the tests of
akit-config (new: `tests/v1.rs`, `limits.rs`), akit-runtime, akit-listener (new: `tests/limits.rs`:
431 under and over the limit and at the default over HTTP/1.1 and h2c, the connection past
`max_conns` closed and room again once one goes, the engine's settings), akit-admin
(`v1_surface_is_frozen`), akit-compat and akit (`validate_warns_of_renamed_fields`,
`loads_return_warnings`, the docs' examples) on macOS, and on the benchmark host with epoll and
io_uring.

Differences from the plan:

- The Done-when held before I79 (every template was valid); I79 also took the gateway examples'
  fields that akio can serve, and split those that need akio into I79b (A214, A168, A215).
- akit's Rust crates' API is not covered in 1.0, where fit covers its Go `module` packages.
- The Admin API check holds the members of three answers, not those of `/status`,
  `/config/effective` and `/cache` (decision 029).
- No benchmark: the control plane (rule 24). What changes on the data plane is the defaults of the
  listeners' engine: `max_connections` 100,000 (was no limit), counting the upstream connections
  too (A215), `max_queued_bytes` 32 MiB (was 64), and request heads up to 1 MiB (was akio's 64 KiB).
  A benchmark task running 100,000 connections or more against akit should set `limits.max_conns`.

### 2026-11-26: I78, config reference and guides

- **The config reference**, [docs/reference/config.md](reference/config.md) and its Chinese twin:
  every object akit reads (62 tables), each field's type, default and description, the fields of
  those objects refused as not supported yet, akit's notes where it reads a field differently from
  fit, and its extension (`listeners[].sockmap`); a chapter for each part of the config, with an
  example of it. The tables are generated by `tools/config-reference.py` (`make reference`) from
  fit's pinned schema (types, defaults and fit's descriptions, said of akit; akit keeps fit's
  default names, `fit_sticky`, `fit/` and the like, which stay as they are), `tests/compat/supported.txt`
  and `tools/config-reference-notes.json` (the extension and the notes, in both languages); the
  prose and the examples around them are written by hand.
- **The checks**, all in CI: `config-reference.py --check` (the lint job, and `make docs-check`)
  fails when a table is not what the generator writes, or an object akit reads has no table in
  either language; `reference_matches_akit_config` in the compatibility gate holds the tables to
  akit-config's lists in `model.rs` (every field read shown read, every other field of an object
  read shown refused, every handler and middleware module as `model.rs` takes it, the extensions
  shown exactly the `*_AKIT_ONLY` lists); `crates/akit/tests/docs_examples.rs` runs every config
  example of every Markdown document (154 at this commit: `.conf`, YAML and JSON, and the migration
  guides' nginx configs and Caddyfiles through `akit adapt -from` first) through `akit validate`,
  in a directory holding the files they name (certificates from a test CA, `public/`, a plugin of
  fit's ABI and a Proxy-Wasm one), and checks that each document and its Chinese twin show the same
  examples.
- **The guides**, [docs/guides](guides/README.md), in both languages: getting started; the reverse
  proxy and load balancing; static files and PHP; HTTPS and certificates; the Admin API and reloads;
  caching; WebAssembly plugins (fit's `llm` noted as not supported yet); migrating from fit (what
  akit does not run yet, fit's default names akit keeps, ACME's own storage), from nginx and Caddy
  (`akit adapt -from`, with what it writes for the guides' examples) and from Traefik. The README
  links them and no longer says nothing is built; `akit run -h` no longer says `.conf` comes later.
- Found while writing them, documented rather than changed: `akit adapt -from nginx` converts
  `error_log` (and `-from caddyfile` a global `log`) to fit's `log`, which akit refuses as not
  supported yet; the Admin API's `POST /reload` applies the running config again, it does not read
  the files (SIGHUP does).

Differences from the plan:

- The Chinese reference has every table, with each field's type, default, status and akit's notes,
  but not fit's descriptions, which are English in fit's schema; it links each table to its English
  twin, as fit's own Chinese reference does.
- No benchmark: docs and tests (rule 24). No decision note, akio request or ADR was needed.

### 2026-11-25: I25, M1 conformance and report

The [M1 report](../bench/results/m1-report-2026-11-25.md) has the details and *Where akit stands*.

- h2spec 2.0.0, every section, on the development Mac: 145/145 on h2c and on h2 over TLS, with
  `respond` and through `proxy` to an HTTP/1.1 upstream.
- The proxy conformance suite, `crates/akit/tests/conformance.rs` (23 tests, through the binary):
  fit's `TestProxyConformance` ported case by case over HTTP/1.1, and an HTTP/2 half on h2c frames
  written by hand (malformed requests of RFC 9113 section 8 reset or 400 and never forwarded,
  cookie crumbs joined, `:authority` as `Host`, no connection-specific fields back, 1xx, 204, 304
  and 502, windows honoured). Passes on macOS and Linux; akit-proxy's tests on epoll and io_uring.
- Found and fixed in akit: a NUL, CR or LF in a field value (accepted by akio's HTTP/2 server) and
  two `Host` fields (accepted by its HTTP/1 parser) were forwarded; akit-proxy refuses both with
  400 in the loop that copies the fields (`Refused::BadFieldValue`, `Refused::TwoHosts`), a stopgap
  until akio API request A169 is pinned, as fit's check of field names. `akit validate` now refuses
  a listener with `h2c` alone as `akit run` always did (A168): the compatibility gate moves
  fit-examples' `grpc` to not supported yet (80 fixtures: 45 valid, 34 refused only as not
  supported yet, 1 skipped). A client's HTTP connection idles 120s before it is closed, fit's
  (akio's default was 75s: at 100k idle connections akit closed a third of them).
- Found in akio, each fixed on an akio branch of its own, not in akio's main and not pinned (rule
  22), each needing akio's same-run A/B: A169, malformed requests (`claude/a169-request-checks`,
  `813f53ea`; its fourth part, a 400 for a streamed body's bad chunk size, not built); A171, the
  HTTP clients' timer jobs held every request until its timeout passed, so akit's proxy grew by
  about 240 bytes a request, 1.8 GiB in a run (`claude/a171-client-timer-cancel`, `90adec65`);
  A172, akio-tls failed a ClientHello whose server name is an IP address, which wrk sends, so
  HTTPS/1.1 had no responses (`claude/a172-sni-ip-literal`, `d23c9368`). A168 (h2c alone) is
  requested.
- Benchmarks on the benchmark host, fit's harness at `7daee573`, three runs: akit `00d9f18` with
  all five servers (10k idle connections, and 100k), and `9d9321e` built with the three akio fixes
  beside fit and nginx (`bench/results/m1-2026-11-25`, `-idle100k`, `-akio-fixes`). akit is first
  in fixed and static over HTTP/1.1, HTTPS/1.1, h2c and h2 but static over h2c (98% of fit), third
  in the proxy (245k, 230k, 238k and 221k requests a second, 86–93% of the best, 27.5 µs of CPU a
  request against nginx's 18.2). Idle connections, bytes each at 10k and 100k: HTTP/1.1 1,144 and
  1,132 (fit 1,497 and 1,441, nginx 556 and 530); TLS 12,942 and 12,872 (fit 9,879 and 8,593,
  nginx 29,924 and 29,583); WebSocket at 10k 4,452 (fit 5,916, nginx 22,275).
- The bench config is aligned with fit's: the proxy route's 60s timeout and the pool's
  `max_fails 0`, and `/ws` to fitbench's WebSocket backend with the `ws` idle kind (I49).
- The M1 exit criteria hold. Of the roadmap's performance targets, static files meet theirs; the
  proxy's throughput (above fit's and nginx's) and p99 (below fit's), and idle HTTP/1.1 (at or
  below nginx's) and TLS (below fit's) memory do not yet: the benchmark tasks' rows in the dev
  plan say what to measure.

Differences from the plan:

- I25 ran benchmarks and memory measurements, as its iteration asks and the user allowed.
- The HTTPS/1.1 and proxy-memory numbers of akit as committed are those of akio as pinned (no
  HTTPS/1.1 responses, 1.8 GiB); the report gives akit's with the akio fixes beside them.
- fixed over h2c measured 7–8% below M0's baseline, which fit and nginx do not show; a same-run
  A/B of `7c835ad` against `9d9321e` is left to a benchmark task.
- Traefik did not start in the proxy scenario: another process held port 8444 on the host.
- `akit-proxy`'s `split::failover` failed once on io_uring (502 for the failover's first request)
  and passed ten times alone: a rare race in the passive health check, not I25's.
- Checked on the benchmark host, the touched crates' tests on epoll and io_uring through
  `tools/test-timeout.sh`: all passing but `akit`'s `reloads_under_traffic_with_no_failed_request`
  (I22's), which failed 3 runs in 6 on epoll while other sessions' benchmarks loaded the host, as
  the base `2a10480` did 1 in 8; alternated with the base on a quieter host, 10 of 10 each.

### 2026-11-25: I76, M5 exit

- **M5 is done**: the [M5 report](../bench/results/m5-report-2026-11-25.md) (and its
  [Chinese twin](../bench/results/m5-report-2026-11-25.zh-CN.md)), with *Where akit stands*;
  every exit criterion holds. Measured on the benchmark host, each run under the host's lock alone,
  akit the working tree of this commit on akio `dad77a7a`, fit `7daee573`.
- **The cache against nginx's `proxy_cache`**: akit joins the harness's `cache-hot` and
  `cache-zipf` (`bench/fitbench/akit.go`, `bench/configs/akit/fit.json`: the route `cache`, a
  1 GiB zone, as fit's). The same hits or more (`cache-zipf` 99.98%, nginx 99.86%, fit 99.97%),
  first of the three: 589,973 requests a second over HTTP/1.1 (nginx 398,691, fit 526,977),
  2,086,275 over h2c (nginx 337,106, fit 1,694,120), 564,220 for 100,000 paths (nginx 322,575,
  fit 505,880).
- **A stampede**: `bench/stampede.sh` (new; `bench/stampede/`) asks nginx (`proxy_cache_lock on`),
  fit and akit for one cold key with 1,000 clients at once, while a slow origin (one second)
  counts its fetches: one fetch each, over HTTP/1.1 and h2c, three keys each, every client
  answered 200; akit with `disable_lock`, the control, 1,000 and 829.
- **The WASM cost**: a `wasm` scenario and two servers, `akit-wasm` and `fit-wasm`, which run fit's
  own `headers` test plugin, the same `.wasm` (`bench/configs/akit/wasm.json`,
  `bench/configs/akit/fit-wasm.conf`, a second config file beside each server's own; `lib.sh`
  unpacks the guest; `run.sh` passes `-akit-wasm-image`). Over HTTP/1.1 the plugin costs akit
  14.7 µs of CPU a request (312,679 requests a second against 831,888 without it), fit 6.8 µs
  (452,353 against 785,657); over h2c 2,048 requests in flight overrun the queue of 1,024 and
  both answer the rest 503 at once, their memory flat. Twice fit's cost: akio-wasm interprets where
  wazero compiles, asked of akio as **A180** (akio's W5, with these numbers).
- **The nginx converter**: `convert::tests::nginx_samples_list_every_skipped_directive`
  (`akit-config`): fit's three sample configs and the three other nginx configs the CLI's tests
  convert all convert, every warning names its line, and a directive no converter knows, put first
  in each of their 56 blocks and at the top, is always listed, at its line or in a block skipped
  whole, the output otherwise unchanged; a converter that drops unknown directives fails it.
  `nginx::convert` is split so that `convert_parsed` takes the directives read.
- **FastCGI**: I75's PHP-FPM test (fit's `TestPHPFPM`, `php_fpm`) ran, in the `php:8.4-fpm` image
  (PHP 8.4.28) on the host, and passed on epoll and io_uring.
- **Where akit stands** (full harness, every server, HTTP/1.1 and h2c): first in fixed (822,825
  and 5,584,430), static over HTTP/1.1 (785,510) and the three cache runs; second in static over
  h2c (99% of fit); third in the proxy (88% and 93% of nginx), its peak memory 1.8 GiB from akio's
  HTTP/1 client timer, which I25 found (A171); idle connections second to nginx over HTTP/1.1
  (1,136 bytes) and to fit over TLS (12,954).
- Harness: `BENCH_DIR` lets a session stage on the host apart from others; a git worktree sets
  `FIT_REPO`. The dev plan's benchmark rows for the cache and for WASM are marked partly
  benchmarked, with what is left.
- Checked: `cargo fmt`, `cargo clippy --all-targets -D warnings` and `cargo deny check`, the tests
  of `akit-config` and `akit` on macOS; on the benchmark host, `akit-config` and `akit` on epoll and
  io_uring through `tools/test-timeout.sh`.

### 2026-11-25: I56, M3 exit

- The roadmap's M3 exit criteria hold; the [M3 report](../bench/results/m3-report-2026-11-25.md)
  has each criterion and what checks it, each middleware against fit's, the full harness with
  every server and *Where akit stands*. akio unchanged (`dad77a7a`); no akio API request, no
  decision note.
- No module blocks an I/O thread: `crates/akit/tests/workers.rs`, fit's `test/workers` ported.
  Every module `akit::STANDARD` registers (14 middlewares, 5 handlers) serves a request whose
  answer waits 600 ms on something slow (a `proxy` upstream behind every middleware,
  `forward_auth`'s auth service, a FastCGI application, `forward_proxy`'s tunnel end, bcrypt on
  akio's pool, fit's plugin on the `wasm` pool); akio's slow-callback detector, set to 200 ms at
  run time (`Telemetry::live`), must record nothing, and the answer comes after the slow part.
  `every_module_covered` fails for a module without a case; a middleware made to sleep 300 ms
  fails the test, naming it. fit leaves `forward_proxy` and `wasm` out; akit covers both.
- Slow peers: `websocket_slow_upstream` (a client's 64 MiB flood held back by an upstream that
  reads nothing, then delivered whole and in order) and `udp_slow_peers` (an upstream that answers
  one session late holds no other back; a client that reads nothing while flooded keeps its
  session), beside I49–I51's. WebSocket from h2c and h3, gRPC both ways and TCP both ways had
  them; one port (I53, I55's `sniff_clear` and `socks_beside_https`) did too.
- Benchmarks: `bench/modules.sh` (with `modulesreport.py` and `configs/modules`) serves one config,
  each middleware on a route of its own in front of the proxy to nginx, with fit and with akit on
  epoll and io_uring (`crates/akit/examples/bench_serve.rs`: `akit run`'s path with the backend
  chosen), wrk on fit's CPU split, 3 rounds. Each middleware costs akit what it costs fit within a
  microsecond a request, except gzip (+6.7 µs against +3.9), `wasm` (+13.6 against +6.4) and
  `forward_auth` (+20.4 against +17.0). The full harness (`m3-2026-11-25`): akit 1st in fixed
  over HTTP/1.1 (830k), h2c (5.57M) and h2 (5.13M) and in static over HTTP/1.1 (788k) and h2
  (2.70M), 2nd in static h2c (98% of fit) and in idle memory (1,141 and 12,959 bytes), 3rd in the
  proxy (88–95% of the leader, 27.8 µs of CPU a request against fit's 21.2).
- Found: HTTPS/1.1 answers nothing (wrk's IP-literal SNI refused by akio-tls) and the proxy's
  memory grows to 1.8 GiB a run (akio-http's client timer keeps every request's job): I25's akio
  API requests A172 and A171, fixed on akio branches, not on akio's main; not requested again.
- Different from the plan: the module benchmark goes over loopback through the listener (akio has
  no in-process request context, which fit's `BenchmarkModule` uses), so it reads each
  middleware's cost as the CPU it adds to the proxied route, against fit's the same way. Rows in
  the dev plan's benchmark table for I40–I45, I47, I48, I67 and I71 updated.
- Checked: `cargo fmt --check`, `cargo clippy --all-targets -D warnings` of `akit`, `akit-proxy`
  and `akit-listener`, their tests on macOS; on gpu51 the `workers`, `websocket` and `l4` test
  binaries on epoll and io_uring under `tools/test-timeout.sh`, all passed.
- Next: M2's I38; I77 (fuzzing), unblocked by I56.

### 2026-11-25: I66, M4 exit

- akio: unchanged, `dad77a7a`; no akio API request, no akio ADR, no decision note (A194–A199,
  ADRs 146–149 and decisions 020–021 unused). No akit code changed.
- **The Gateway API's conformance suite passes**: `GATEWAY-HTTP` core at Gateway API v1.3.0, the
  version fit pins, 33 passed, 0 failed, 0 skipped, on the first run
  ([the report](../tests/conformance/reports/gateway-http-v1.3.0-2026-11-25.yaml), the same 33
  tests fit's I61 passes). Run by `tests/conformance/run.sh` on the benchmark host, under the
  shared lock: fit's suite module at `7daee573` (`go test -c`), a disposable kind cluster
  (`akit-i66`, v0.30.0, Kubernetes v1.34.0, its own pod and service subnets beside the host's
  other kind cluster), the experimental CRDs, akit's release binary outside the cluster. The other
  38 tests (extended features, TLSRoute, UDPRoute, the mesh) are skipped as unclaimed, as for fit.
  A second, informational run with the extended HTTP features waited over an hour on the host's
  lock behind other sessions' benchmarks and was given up (the
  [M4 report](../bench/results/m4-report-2026-11-25.md)). On epoll only: `akit run` has no
  switch for io_uring (akio's `Backend` is set in code, which only the tests do).
- **The Docker Compose example**: `deploy/compose` (`compose.yaml`, `akit.conf`, `test.sh`, a
  README in both languages), fit's `fit-examples/docker-compose` for akit, labels under `akit.`.
  `test.sh` checks that both replicas answer, another host gets the config's 404, a third replica
  answers once scaled up, the app's host gets 404 once its containers stop and 200 once they start;
  `AKIT_IMAGE` runs an image built elsewhere, `AKIT_PORT` moves the port. It passed on the
  benchmark host (Docker Compose 2.40.3, the release binary on `ubuntu:24.04`, as the host has no
  buildx for the Dockerfile).
- **CI**: the job "Gateway API conformance (kind)", as fit's (fit's suite module checked out at
  `7daee573`, kind, the CRDs, akit with `CAP_NET_BIND_SERVICE`, `run.sh`, the report and akit's
  log kept as an artifact), and a "Docker Compose example" step in the `packaging` job on the image
  it builds. Neither has run on GitHub yet (nothing pushed).
- Docs: `tests/conformance/README*` (the result, the CI job, subnets beside another kind
  cluster), `deploy/README*` (the Compose example), the
  [M4 report](../bench/results/m4-report-2026-11-25.md) with *Where akit stands*; the plan's
  benchmark status gains the "M4 exit" row (the full harness and providers at rest, for the
  benchmark task; rule 30: no benchmark run here).
- Checked: the two runs above on the benchmark host; `scripts/check-docs.sh`. No Rust changed, so
  no cargo test run. Cleaned up (rule 29): the kind cluster, its route, the Compose project, the
  image and `/root/akit-i66` on the benchmark host.

### 2026-11-25: I38, M2 exit: churn, health window, a certificate for a host name

- akio: unchanged, `dad77a7a`. Akio API request **A10** (closing an HTTP client's idle
  connections now), requested by I14, built on akio branch `claude/a10-close-idle`
  (`af959290`, off akio's `e79cc740`; not pushed or merged): `Client::close_idle`, tested on
  macOS and gpu51 (epoll, io_uring). No new akio request, akio ADR or akit decision note (I38's
  block, A187–A193, ADR-142–145 and notes 018–019, is unused).
- The tests, in `crates/akit/tests`, their support in `tests/m2/` (upstreams and clients, akit
  as `akit run` or in process, Pebble):
  - **Churn** (`churn.rs`), the first criterion. The traffic: HTTP/1.1 on a connection a request
    (two clients, one every 10 ms each), on a keep-alive connection busy throughout (proxied and
    not) and on one idle half a second between requests; HTTP/2 with prior knowledge, four
    streams at a time on one connection for the whole run; slow streamed responses through the
    proxy (20 KiB over 1 s, each across hundreds of swaps); a WebSocket through the proxy and a
    connection through a `tcp` service, both open for the whole run and echoing every 20 ms.
    Every swap changes `/v`'s answer; every 20th the proxied upstream's addresses and load
    balancing policy; a listener is added at every 100th from the 10th, checked, and removed 50
    swaps later.
    `churn_through_sighup_and_the_admin_api`: `akit run`, 2000 swaps (`AKIT_CHURN_SWAPS`), one
    after another as each is applied, through SIGHUP, the Admin API's `POST /load` and its edits
    by pointer (`PATCH /config/routes/0/handler/params`); then
    `fit_config_reloads_total{result="success"}` counts them all and none failed, and the
    process's descriptors (from `/proc` on Linux, `lsof` elsewhere) are back within 16 of where
    they were before the traffic, counted after a listener was added and removed once (akio
    makes per-thread pipes for runtime listeners the first time, 28 on macOS, and keeps them; a
    listener added at run time holds a socket and a poller on each I/O thread while it lasts). `churn_in_process`: the
    same through `Listeners::reload`, on the backend `AKIO_TEST_BACKEND` names (the churn on
    io_uring), every swap keeping the listeners the traffic uses, every replaced graph retired
    afterwards (`GraphCell::retiring` 0). Both check that no request failed, no connection was
    closed under a client (the HTTP/2 connection still open at the end), every kind of traffic
    kept going, the last config's answer is served, and the two upstreams accepted at most one
    connection for every two swaps.
  - **Health window** (`health_window.rs`), the third: requests every 10 ms through the proxy to
    an upstream of two addresses with an http `health_check` (200 ms interval, 100 ms timeout,
    `fails` and `passes` 2); one address's probe path hangs while its other requests are served,
    as fit's e2e `TestHealthCheckRotation`. It must be sent nothing more within
    `fails × interval + timeout + interval` (700 ms), nor while it is out, a reload of the same
    config included, and be sent requests again within `passes × interval + interval` (600 ms)
    of answering, each with 2 s of slack for a loaded host, no request failing. Through
    `akit run`, with the address's `fit_upstream_backend_up` gauge going 1, 0, 1 at
    `GET /metrics`; and in process on `AKIO_TEST_BACKEND`'s backend (the probes on that engine).
  - **A certificate for a host name** (`acme_m2.rs`), the second: `akit run` on a config naming
    only a host name (with the CA's directory and the ports besides, since Pebble is not Let's
    Encrypt and the test does not own 80 and 443): its certificate obtained through HTTP-01 on
    the `http` listener HTTPS by default makes, found in the storage, served on `https` over
    HTTP/1.1 and h2 (the chain checked up to Pebble's root, and up to its issuing intermediate
    alone), and `http` redirecting to it. `m2/pebble.rs` runs Pebble from its binaries, or in
    Docker with `AKIT_PEBBLE_DOCKER=1` (Let's Encrypt's `ghcr.io/letsencrypt/pebble:2.10.1` and
    `pebble-challtestsrv`, on the host's network, removed when the test ends);
    `AKIT_PEBBLE_REQUIRED` fails the test without Pebble.
- **Fixed: unchanged upstreams keep their pools across a reload** (fit's `Env.Previous` and
  `upstreamKey`, `runtime/graph.go`). The churn found that every swap dialed every upstream anew:
  akit made all the pools again on each reload, and a replaced pool's idle connections stayed
  open until its `idle_timeout` (A10), so 2000 swaps opened about 7,000 upstream connections,
  which ran macOS out of ephemeral ports, and an upstream's probed and passive health started
  over at each reload (a probed-out address back in rotation until its probes failed again).
  Now `akit_runtime::Previous::of(graph)` and `Env::with_previous` hand the running graph's pools
  to `compile`, which gives the new graph the pool of each upstream whose config and TLS files'
  sums are the same (`Pool::share`, fit's `Acquire`); the graph's cleanup lets go of its pools
  (`Pool::release`) and the last graph holding a pool closes it. `akit run`'s swap (SIGHUP, the
  Admin API, `-watch`, providers) passes the running graph; a dry run and `akit validate` share
  nothing. Tests: fit's `TestSharedPools` (`akit-runtime/tests/upstream_tls.rs`): shared when
  unchanged, a CA file or client certificate changed makes a new pool, a pool closes with the
  last graph holding it; the churn's connection count (406 over 2000 swaps, about 7,000 without
  sharing); the health window's reload while the address is out. Changed upstreams' replaced
  pools still keep their idle connections up to `idle_timeout` until akit pins A10 and
  `Pool::close` calls `Client::close_idle`; the churn's `app` sets a 1 s `idle_timeout` meanwhile.
- **What else the churn found** (fit's contract, followed): a connection through a `tcp` service
  was closed exactly `-shutdown-timeout` (5 s) after the first swap, though its listener and
  service were unchanged. A reload drains what the replaced graph still serves for the drain
  timeout, and past it closes the connections its `tcp` services still relay (fit's
  `TestTCPReload`, I51); fit's proxy also cuts its exchanges and WebSocket relays then
  (`Proxy.Drain`), which akit does not yet (recorded with I49). A long-lived connection so lasts
  at least the drain timeout past the swap that replaced its graph: the churns run with a drain
  timeout longer than they take (300 s), under which every connection must last the whole run.
  And the test's own short connections, back to back, ran macOS out of ephemeral ports (TIME_WAIT);
  they are paced at one every 10 ms.
- Results (2000 swaps each; macOS, debug build): through `akit run`, 5.0 s, about 117,000 requests
  answered (817 short, 42,981 kept, 72,832 over h2c, 5 slow streams, 214 WebSocket and 216 `tcp`
  echoes), 406 upstream connections, none failed; in process, 4.8 s, about 123,000, 406, none
  failed. Health window: out of rotation 0.48–0.51 s after the probes stopped answering (window
  0.7 s), back 0.33–0.37 s after they answered again (window 0.6 s). On gpu51 (Linux, debug build, the host shared
  with other sessions' runs): epoll, through `akit run` 4.8 s and about 37,000 requests, in
  process 5.1 s and about 29,000, 395 upstream connections each; io_uring in process 5.8 s and
  about 24,600, 391 (`akit run` on epoll in the same pass, 4.8 s, about 28,000); none failed, no
  connection dropped, the descriptors back where they were. Health window on gpu51: out after
  0.45–0.49 s, back after 0.39–0.40 s, on epoll and io_uring. The certificate from Pebble in
  Docker: issued, served and redirected in 1.3–2.9 s a test, on both backends; no container left.
- **The Let's Encrypt staging certificate**, the criterion itself, passed on 2026-11-26, run by
  the maintainer's session (no session registered or pointed a name): on a public Debian 12 host
  (Linux 6.1, x86_64; the host fit's own staging run used), for the DuckDNS name
  `akit-test.duckdns.org`, ports 80 and 443 free. The `akit` and `acme_m2` test binaries were
  built from this branch (akio `dad77a7a`) in a `rust:1-bookworm` container on gpu51 (glibc
  2.36, as the host's) and copied to the host; as root:

  ```sh
  AKIT_ACME_STAGING_NAME=akit-test.duckdns.org acme_m2 only_a_host_name_from_lets_encrypt_staging --exact --nocapture
  ```

  The config is only the route's host name and the staging directory
  (`https://acme-staging-v02.api.letsencrypt.org/directory`), on the default ports. The
  certificate was issued by `C=US, O=Let's Encrypt, CN=(STAGING) Baloney Bulgur YE2`, `https`
  served it over HTTP/1.1 and h2 (the chain checked up to its intermediate) and `http` redirected
  with 308: 1 passed in 11.48 s. The host's files were removed afterwards. To run it again, as
  root on a host a public name points at and the Internet reaches on port 80:
  `AKIT_ACME_STAGING_NAME=NAME cargo test -p akit --test acme_m2 only_a_host_name_from_lets_encrypt_staging -- --nocapture`.
- Not covered, and why: `akit run` has no setting for its engine's backend (always epoll's,
  kqueue's on macOS), so the churn through SIGHUP and the Admin API runs on epoll alone and the
  io_uring churn runs in process; an io_uring field is fit's to add first (decision 002). A
  changed listener (rather than a kept one) is replaced and its connections drained; that path
  is I22's and I28's tests', not the churn's.
- To measure (benchmark task, rule 30): the dev plan's new row, config churn under traffic: the
  fixed and proxy scenarios while the config is swapped every 10 ms, against no swaps, and akit's
  RSS and descriptors after 100k swaps; and the M2 report of the full harness (rule 24).
- Checked: `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, the docs
  check and `make compat-check` on macOS, with the tests of akit-upstream, akit-runtime,
  akit-listener, akit-proxy, akit-l4 and akit (all passed); on gpu51 under the shared lock, fmt,
  workspace clippy and the workspace suite on epoll and on io_uring through
  `tools/test-timeout.sh` (`--test-threads=1`, no `HANG:`): every binary passed but the churn's
  descriptor check through `akit run`, which counted the `extra` listener the last swap added and
  akio's one-time pipes (the test fixed as above); then the M2 tests again on both backends, all
  passed. The akio branch: akio-http's `client` tests and clippy on macOS and on gpu51 (epoll,
  io_uring). No benchmark (rule 30).

### 2026-11-25: I72, WASM II: the Proxy-Wasm subset

- akio: unchanged, `dad77a7a` (akio-wasm, ADR-125, with ADR-128's held heads and body filters,
  akio API requests A165-A167); no akio API request, no akio branch.
- `akit-plugin`: Proxy-Wasm filters, the subset fit runs ([decision 011](decisions/011-proxy-wasm.md)),
  fit's `plugin/wasm/proxywasm.go` and `response.go` ported: a module that exports
  `proxy_abi_version_0_2_0` or `_0_2_1` (and `proxy_on_context_create`,
  `proxy_on_request_headers`, `proxy_on_done`, `proxy_on_log`, `proxy_on_delete`,
  `proxy_on_memory_allocate`, their types checked as an instance starts) runs on the same
  `InstancePool` as fit's ABI. Each instance starts its root context with `vm_config` and `config`
  (`proxy_on_vm_start`, `proxy_on_configure`; a refusal fails the load). Each request is a context
  of its own, made on the pool with `proxy_on_request_headers`: the plugin's changes to `:method`,
  `:path`, `:authority` (or `host`) and the request's fields become the request's through
  `Context::req_mut`; its local response is sent with its fields and gRPC status; a pause without
  one fails as `on_error` says. The host module `env` (`crates/akit-plugin/src/proxywasm.rs`):
  `proxy_log`, `proxy_send_local_response`, the header maps by value and by pairs (request,
  response, trailers), `proxy_get_buffer_bytes` and `proxy_set_buffer_bytes` (VM and plugin
  configuration, the response body), `proxy_get_property` (`properties`, `plugin_name`,
  `request.*`, `source.address`, `response.code`), metrics (256 an instance),
  `proxy_set_effective_context` and `proxy_done`; calls out, shared data and queues, ticks,
  foreign functions and setting properties answer `Unimplemented`.
- The response's callbacks (`crates/akit-plugin/src/response.rs`): a filter that follows
  responses keeps the request's context open on its instance, at most `max_streams` an instance
  (503 past it), and its callbacks are jobs queued for that instance (`submit_to`, an instance
  queue of four calls a stream). `proxy_on_response_headers` runs on a held head
  (`Context::hold_response_head`, A166), which goes on changed, is replaced by the plugin's local
  response, or fails; `proxy_on_response_body` and `proxy_on_response_trailers` run on a body filter
  (`Context::add_body_filter`, A167), a paused body held up to `max_body_bytes`, the trailer in the
  last call. `response: headers` registers no body filter, `none` follows nothing. A callback that
  fails, or cannot be queued (its instance remade, the pool closed), fails the response or lets it
  go on without the plugin (`on_error: skip`). Contexts close at the end of the last callback, or
  by a job queued when the last of the response's hooks is dropped (a response cut, a client
  gone); a released config's pool waits for the responses it follows, at most 10 minutes, before
  it closes.
- Config (`akit-config`): `vm_config`, `properties` (`path` required, `value`), `response`
  (`all`, `headers`, `none`), `max_streams` (1024 by default, at most 65536) and `max_body_bytes`
  (1 MiB by default, at most 4 GiB) are read; every param of fit's `wasm` is now.
- Tests: fit's `TestProxyWasmRouting`, `TestProxyWasmProperties`,
  `TestProxyWasmResponseHeaders`, `TestProxyWasmResponseBody`, `TestProxyWasmResponseTrailers`,
  `TestProxyWasmResponseFails`, `TestProxyWasmMaxStreams` and `TestProxyWasmResponseClientGone`
  through akit's listener on epoll and io_uring (`crates/akit-plugin/tests/proxywasm.rs`), with
  fit's test filters run unchanged: the Proxy-Wasm Go SDK's `http_routing`, `properties`,
  `http_headers` and `http_body` examples and fit's `response` filter, built with Go 1.27.3 and
  committed gzipped (`tests/guests/pw-*.wasm.gz`, 4.3 MB; how in `tests/guests/README.md`).
  `respcall_test.go`'s cases with filters the tests encode: the head's, body's and trailer's
  callbacks that trap or answer late, under `fail` and `skip`; a paused body; `proxy_on_done`
  trapping; the request phase's rewrites, local response, pause and trap; load errors. Every
  followed test checks, with one instance and `max_streams: 1`, that no context is left open
  (fit counts the config's holds). Rule 18's: a 4 MiB body written as a proxy paces it, through a
  filter that changes each part, to a client that reads slowly; `max_body_bytes` under, at and
  over; a client gone while its response is held; and the filters over h2c (`:authority`, a
  held head changed and replaced, a streamed body and its trailer, a failed response's stream
  reset). In the crate: `TestHostProxyWasm`,
  `TestHostProxyWasmResponse`, `TestHostModules`, `TestPairs`, `TestBodyBuffer`, and the bounds.
- Compatibility: the `wasm` middleware's 7 Proxy-Wasm fields supported (417 fields); the gate's
  outcomes do not change (no fixture uses them).
- Different from fit (decision 011): a local response from `proxy_on_response_headers` is sent in
  the response's place (fit fails the response: fib cannot replace a held header); akio holds up
  to 4 MiB written meanwhile (fib 64 KB); a released config waits at most 10 minutes for its
  followed responses; names, values, `:path` and `:method` are checked (`BadArgument`), and 1024
  changes to a header map in one callback fail the call; fit's `TestResponseBigBuffer` (a 64 KiB
  buffer kept between calls) has nothing to test: akit hands each part's buffer on.
- To measure (benchmark task, rule 30): the dev plan's WASM row: a request through the SDK's
  `http_routing` (fit's `BenchmarkProxyWasm`) and a response through fit's `response` filter and the
  SDK's `http_headers`, against fit and none. In a debug build the interpreter upper-cases a few
  hundred KiB of body in a Go guest in more than the default 100 ms `timeout`
  (`large_body_slow_client` sets 20 s): worth measuring in release.
- Checked: `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo deny
  check`, `make compat-check`, the docs check, and the tests of `akit-plugin`, `akit-config`,
  `akit-compat`, `akit-runtime`, `akit-middleware` and `akit` on macOS; on gpu51 (Rust 1.100),
  fmt, workspace clippy and the same crates on epoll and io_uring through
  `tools/test-timeout.sh` (63 test binaries each, all passed), then `akit-plugin` three rounds
  more on each backend with the h2 test added: one io_uring round failed once in `request_phase`
  at building the listener's engine (`akit_listener::engine(...).unwrap()`, before any plugin
  ran; gpu51 shared with other sessions' runs), and that binary then passed six runs of six on
  io_uring.

### 2026-11-25: I51b, the eBPF sockmap relay

- Config: `listeners[].sockmap`, a boolean, false when left out: an akit extension fit does not
  have ([decision 010](decisions/010-sockmap-relay.md)). Read in JSON, YAML and `.conf` (the
  `.conf` reader and writer take its type from a list of akit's extensions,
  `EXTENSIONS` in `conf/convert.rs`), refused on a listener whose protocols do not include `tcp`.
  akit-config lists it apart from fit's fields (`LISTENER_AKIT_ONLY`); the compatibility gate's
  `akit_config_lists_match_the_inventory` checks no such field is one fit has.
  `docs/compatibility.md` has a section for akit's extensions.
- `akit-l4`'s `sockmap` module: akio's `ebpf::Relay` loaded once per process
  (`akit_l4::sockmap::load`) where akit-listener binds or reloads listeners, if one has `sockmap`,
  on the control thread (the verifier runs there, never on an I/O thread); a relay that cannot load
  (no `CAP_BPF`/`CAP_NET_ADMIN`, a kernel before 6.15, not Linux) is said once on standard error,
  and those listeners relay in userspace. The listener's flag travels in its `Entry` in the graph,
  so a reload that changes it applies to new connections without binding the listener again.
- Splicing: as the upstream's connection opens (on its I/O thread, the client's reads still held),
  the service's PROXY header and what the client sent before (its ClientHello on a listener that
  routes by server name, a read before the hold) are written to the upstream's socket directly
  once both sockets are in the map, before the kernel relays a byte
  (`Relay::splice_inner_with`); then the client's reads resume and only ends and errors reach
  akit. Anything that keeps a connection from splicing (a transport layer, the map full, a write
  the send buffer cannot take whole) leaves it to I51's userspace relay, with nothing lost: what was
  not written goes through the engine as before. A payload a handler still reads from a spliced
  connection while both sides are open (a read that completed before the splice took over) closes
  both sides rather than risk the order. `Service::spliced()` counts spliced connections.
- Counters: a spliced connection's idle timeout is kept by its sockets' TCP counters
  (`akio::ebpf::byte_counts`: bytes received), read by the service's sweep off the I/O threads,
  outside its locks. A side that ends has the other closed once its `written` counter reaches
  what it had at the splice plus what the ended side received since, less what userspace had read
  of it (which akit counts), polled on akit's timer thread (1 ms, doubling to 100 ms), or once that
  stops moving for the service's idle timeout; closing earlier would drop what the kernel's relay
  still holds.
- Stopgap (rule 21): akio has no public way from a `Conn` to its socket's counters or to a splice
  that writes first; akit-l4 uses `Conn::inner` (public, hidden in akio's docs, as akio-socks uses
  it), recorded as **akio API request A170**, which also asks for the length of a PROXY header
  akio-mux reads: until then a connection whose PROXY header the listener read is relayed in
  userspace (the counters would not add up). No akio change, no new dependency; akio stays at
  `dad77a7a`.
- Differences from the plan: none in scope. The field is the listener's (as the plan says) rather
  than the service's; `udp` listeners never splice (the relay is TCP's). akio-socks's `CONNECT`
  tunnels on a listener's `socks` are not spliced: akio-socks has its own `sockmap` option, left
  for a later change.
- Tests: `crates/akit-listener/tests/sockmap.rs`: splicing and echo (one connection with 4 MB
  each way at once, 20 at a time), a client that reads nothing while 32 MB come (the kernel's full
  send buffer, the 6.15 fix's case), a client's end and an upstream's end after 8 MB with every
  byte forwarded first, the idle timeout kept by bytes going either way and closing a quiet one
  after the timeout, the service's PROXY header v1 and v2 going first, TLS passthrough by SNI
  spliced after the ClientHello (and the fallback service), a client's PROXY header keeping the
  connection in userspace. Each checks the connections were spliced where the relay loads and
  that none was where it does not: `AKIO_TEST_EXPECT_EBPF=1` fails a run whose relay does not
  load, `AKIT_TEST_SOCKMAP=userspace` one whose relay does. `crates/akit-config/tests/services.rs`:
  the field in the three formats, written back by `akit adapt`, its checks.
- Checked: on macOS, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and the tests
  of akit-l4, akit-config, akit-compat, akit-runtime and akit-listener (the relay does not load:
  every connection relayed in userspace), `cargo deny check`, the docs and compatibility checks. On
  gpu51 (Linux 7.0) under the bench lock, in a private network namespace (`unshare -n`, loopback
  only, no host interface): clippy, and the same crates' tests with epoll and with io_uring as root
  with `AKIO_TEST_EXPECT_EBPF=1` (every sockmap test's connections spliced), and the sockmap tests
  with every capability dropped (`setpriv --bounding-set -all --inh-caps -all`, `CapEff` 0) with
  `AKIT_TEST_SOCKMAP=userspace` (the fallback): all passing; the sockmap tests three times in each
  of the four modes.
- To measure (benchmark task, rule 30): the dev plan's row for the eBPF sockmap relay (throughput
  and CPU against the same listener without it and against fit, connections a second, the sweep
  and drain with 10,000 spliced connections).

### 2026-11-25: I71, WASM I: the runtime and fit's ABI

- akio: akit is pinned to `dad77a7a`, akio's `main`:
  akio-wasm W6 on top of `1ae0a05f` (`claude/i71-akio-base`), which merges `akio-wasm` W1–W4
  (`claude/akio-wasm-w4-445f36`, `a2373416`, ADR-125, A165) and A33/A48 (`claude/a33-a48`,
  `2afb267b`, ADR-126, ADR-127), both off `main`'s `4989cd8b`; the changelogs and the ADR index
  merged as unions. Moving to akio's `main` once they land there means changing the `rev` again
  (rule 22).
- The runtime is akio's own interpreter, `akio-wasm` ([decision 008](decisions/008-wasm-runtime.md)):
  no `unsafe`, no third-party crate, no exception to decision 001.
- `akit-plugin`: the `wasm` middleware (`http.middleware.wasm`), as fit's `plugin/wasm` with fit's
  own ABI: the plugin read (at most 64 MiB) and decoded when the config is compiled, a module
  shared by the configs that load the same code; `fit_on_request` and the host module `fit`
  (`header_get`, `header_set`, `header_add`, `header_del`, `request_get`, `respond`, `log`) on
  akio-wasm's `Linker`, with WASI preview 1 (output discarded) and imports nothing defines trapping
  only if called; the calls on `akio_wasm::InstancePool` (`instances`, `queue`, `timeout` as each
  job's deadline, `wait_timeout`, `max_memory_bytes` as each instance's memory bound), never on an
  I/O thread. A request is copied as the plugin sees it, suspended, and resumed from the pool's
  completion: the request's fields as the plugin changed them become the request's through
  `Context::req_mut` (`akit_runtime::set_request_headers`, as the other middlewares since A33), then the
  plugin's answer is sent with the response fields it set, or the request goes on, a response
  filter applying the plugin's response fields to whatever response is sent. A full queue, a wait
  past `wait_timeout` and a pool closing are answered 503; a trap, a panic, a deadline or the
  memory bound passed, an ABI broken, an instance that cannot be made again 500, or the request
  goes on without the plugin with `on_error: skip`; a client gone while it waited leaves its call
  unrun. The pool is closed on a thread of its own when the config is released (cleanups may run
  on an I/O thread). Plugin log lines at info and above go to standard error, 100 a second.
- Bounds of akit's own beyond fit's: a field name that is not a token, a value with CR, LF or NUL,
  a name or value that is not UTF-8, and more than 256 changes to the request's fields in a call
  fail the call; `instances` at most 1024, `queue` at most 1 Mi, `max_memory_bytes` at most 4 GiB,
  an instance's start (Go's `_initialize`) at most 10 s.
- Config: `path`, `config`, `instances`, `queue`, `timeout`, `wait_timeout`, `max_memory_bytes`
  (and `max_memory`), `on_error`; Proxy-Wasm's `vm_config`, `properties`, `response`,
  `max_streams` and `max_body_bytes` are refused as not supported yet, and a Proxy-Wasm plugin
  fails its load naming I72.
- Tests: fit's `TestHeaders`, `TestRespond`, `TestFailures`, `TestNeverBlocks`, `TestParams`,
  `TestLoadErrors` and `TestCalls` (fit's ABI) through akit's listener on epoll and io_uring
  (`crates/akit-plugin/tests/wasm.rs`), with fit's Go test plugin run unchanged (akio-wasm's
  committed guest, copied to `crates/akit-plugin/tests/guests`); with an earlier `headers`
  middleware, clients cut midway and slow, bodies under and over the listener's limit. In the
  crate: `TestHostABI` (every host function and its failures, through a guest of the tests' own),
  `TestShared`, `TestClientGone`, `TestInstanceLost` and `TestCloseQueued` (as the verdicts on the
  pool's results). Modules that misbehave are encoded by `akit_plugin::encode`, fit's
  `encode_test.go`. The TinyGo guest is not built (akio-wasm W3 did not commit it).
- Compatibility: the `wasm` fields supported (410 fields); `examples/wasm` is valid (80 fixtures:
  46 valid, 33 refused only as not supported yet, 1 skipped, none failed).
- Still different from fit: Proxy-Wasm (I72); plugin log lines have no logger of their own
  (`log.loggers` is not read yet), debug lines are dropped; the request's body still comes whole
  to the plugin's route when its handler would stream it (A75).
- To measure (benchmark task, rule 30; akio's W5): a request through fit's `headers` plugin and a
  no-op plugin against none and against fit, the guest call alone against wazero, the pool past
  its instances, an instance remade after a trap (the dev plan's benchmark row).

### 2026-11-25: I74, nginx and Caddyfile converters

- `akit adapt -from nginx` and `-from caddyfile` (`akit_config::convert`): fit's
  `adapter/nginx` and `adapter/caddyfile` ported, converting the subset fit converts. nginx: the
  http block's servers (listen, server_name, TLS certificates), locations of every kind, nested,
  proxy_pass and upstream blocks (balancing, backups, timeouts), root, alias, index, try_files,
  return, rewrite, limit_req with its zones, client_max_body_size, add_header, proxy_set_header,
  gzip, error_log and access_log. Caddyfile: the global options fit has a place for, snippets and
  imports, `{$NAME}` placeholders and heredocs, site addresses and HTTPS by default, named
  matchers intersected as Caddy's blocks narrow them, handle, handle_path and route,
  reverse_proxy (upstreams, health checks, transport), file_server, respond, php_fastcgi, root,
  try_files, redir, rewrite, uri, header, request_header, encode, basic_auth, forward_auth,
  request_body, tls and log. Each directive left out is a warning naming its line, as fit's. The
  JSON is Go's `json.Marshal` of the same values; regular expressions are checked as Go's
  `regexp/syntax` checks them, its errors quoted word for word in the warnings. The converted
  config is checked as `akit validate` reads it, its problems placed at the source's lines; the
  file is read as it is (an nginx `include` is not followed, as in fit), at most 64 MiB.
- Where it differs from fit: a config that is not UTF-8 is refused (fit converts its invalid
  bytes); an nginx `listen ... quic` on an address another server listens on in the clear, which
  makes fit panic, converts without the certificates; Go's bound on a regular expression's
  compiled size is not checked.
- The validator's host name messages quote the host as fit does (`"a..b" is not a valid host
  name`), no longer escaped.
- Tests: golden files fit itself wrote (`tools/adapt-golden`): each converter on its corpus
  (1290 nginx configs and 1197 Caddyfiles: fit's samples, the assistant's migration cases, the
  benchmark's configs, the docs' Caddyfiles, the fuzz seeds, every string in the converters'
  tests, and the samples and seeds cut short at eleven places), akit's JSON, warnings and errors
  fit's byte for byte; `akit adapt -from` on 10 samples to all three formats (30 cases), standard
  output and standard error byte for byte; every converted config loaded or refused by akit's
  checks as by fit's, but where akit's knowingly differ (listed in the test). fit's converter unit
  tests are ported (parsers, positions, matches, units, bounds), with fit's `TestServe` for both
  converters end to end (`akit run` on the converted config: a file, a response, a redirect, a
  proxy with its prefix replaced), and broken configs (each corpus input cut short and with each
  brace and quote taken out, over 10 000 inputs) never panic.
- Found, left for a follow-up: akit does not refuse an Admin API address a listener's port takes
  (fit's `listener "x" listens on TCP :2019 already ...`).
- No akio API request (A168–A170 unused); no new dependency. No benchmark: the control plane
  (rule 24).
- Checked: `make check` on macOS. On gpu51, the touched crates' tests (`akit-router`,
  `akit-config`, `akit`) with epoll and with `AKIO_TEST_BACKEND=uring`, through
  `tools/test-timeout.sh`, queued behind the benchmark's lock (`/root/akit-i74.log`).

### 2026-11-25: I70, `akit build`

- `akit build` (`crates/akit/src/build_cli.rs`), fit's `fit build` with its flags, messages and
  exit codes: `-o`, `-with` (repeated: `name`, `name@version` from crates.io, `name=dir`, and
  `name=git+url#reference`, which fit's Go modules have no need of), `-without` (akit's module
  crates: `akit-handler`, `akit-middleware`, `akit-cache`, `akit-proxy`), `-akit` (a commit or tag
  of akit's repository, another repository of it, or a directory; the source this akit was built
  from when left out, as fit builds on its own version) and `-keep`.
- `akit::build` (`crates/akit/src/build.rs`), fit's `plugin/build`: a Cargo package generated with
  a `main.rs` that calls `akit::main` with the crates' `MODULES` lists (decision 003: lists, not
  imports) and a `Cargo.toml` (akit's release profile; a workspace of its own); built with the
  `cargo` on `PATH` or named by `CARGO`, its binary found in cargo's JSON messages and copied to
  `-o`. A build from a directory of akit uses its `Cargo.lock` and, for a module that may depend on
  akit by git, replaces akit's repository with the directory (`[patch]`, fit's `go mod edit
  -replace`).
- The `akit` crate is a library and a binary: `akit::main(version, lists)` is the command line
  (`src/main.rs`, now the library's `cli` module, kept in place so that other branches' edits to
  it merge; it can be renamed to `cli.rs` once they have), `akit::STANDARD` akit's own lists,
  `src/bin/akit.rs` the binary. `akit-runtime` re-exports `akio_http` and `akit_config` (as
  `config`), so a module crate depends on it alone.
- `akit-config`: a module name the model does not know is kept as `Handler::Module` (`ModuleRef`:
  name, middleware or not, params as written) instead of being refused, and the registry reports
  it at compile time with fit's message at fit's path (`routes[0].handler.module: there is no
  module "nope" in http.handlers (it has …)`), where akit said "there is no handler module". fit's
  names akit does not support yet are still refused as such.
- The module guide, [modules](modules.md), and an example module,
  `crates/akit/tests/testdata/hello-module` (fit's `testdata/hello`: a `hello` middleware adding
  `X-Hello`, its params decoded strictly).
- Tests: fit's `TestParseModule`, `TestImports`, `TestBuildFlags`, `TestBuildErrors` (cargo
  offline for what cannot be fetched) and `TestBuildNoWorkingDirectory`, ported; and fit's
  `TestBuildWithModule`: akit built from this checkout with the example module and without
  `akit-cache` (a debug build, its target directory kept under `target/tmp`), whose binary says
  its version, checks the module's params, refuses `cache`, and serves a route through the module.
- Not done: modules of other kinds (providers, layer 4 handlers); `-without` for providers, which
  are built in; an interrupted build leaves its temporary package behind.
- No benchmark: the command line and the control plane (rule 24).
- Checked: `make check` on macOS. On gpu51, the touched crates (`akit-config`, `akit-runtime`,
  `akit-middleware`, `akit-admin`, `akit-compat`, `akit`) with epoll and `AKIO_TEST_BACKEND=uring`
  through `tools/test-timeout.sh`, queued behind the benchmark under the shared lock: log
  `/root/akit-i70.log`.

### 2026-11-25: I65, packaging

- Release archives (`scripts/dist.sh`, `make dist`): `akit_<version>_<os>_<arch>.tar.gz` with the
  LICENSE and READMEs, and `checksums.txt`, as fit's `dist.sh`. Linux amd64 and arm64 are static
  musl binaries linked by `rust-lld`, so any host builds them with no cross toolchain (this Mac
  built all four, the macOS ones with Xcode's linker); `AKIT_VERSION` at build time sets what
  `akit version` prints, as fit's `-X main.version` ([decision 009](decisions/009-packaging.md)).
  `scripts/check-dist.sh` (`make dist-check`) checks the sums, each archive's files, each
  binary's platform, that the Linux ones are static, and runs the host's.
- The container image (`Dockerfile`, `make image`): fit's, the musl binary cross-compiled on the
  builder's platform and linked by `rust-lld` (a multi-platform build needs no emulation), on
  distroless's static image as user 65532, with a placeholder config at `/etc/akit/akit.json`.
  Built and run here: 14.1 MB, `akit version` and the placeholder page served.
- fit's Helm chart (`deploy/helm/akit`) and systemd units (`deploy/systemd`: `akit.service`,
  `akit@.service`, sysusers, and fit's `test.sh`) with akit's names and paths; the chart's
  controller is `lesismal.github.io/akit`. `deploy/README.md` documents them, with what a release
  needs from the maintainer.
- `release.yml`: on a tag `v*`, the binaries built on Linux and macOS runners, checked, summed with
  the packed chart and attached to the GitHub Release; the image pushed to ghcr.io for linux/amd64
  and linux/arm64; a tag whose version is not the chart's `appVersion` refused. Run by hand, it
  only builds. CI's `packaging` job builds the static binary, runs the systemd units' test as
  root, builds and runs the image, and renders the chart three ways, validating its config with
  akit.
- Tests: `crates/akit/tests/packaging.rs` (the placeholder config valid and serving; the units,
  the Dockerfile and the chart naming akit's commands and the same config path; `akit version`);
  `make packaging-check` in `make check` (the scripts parse, shellcheck where installed).
- Left to the maintainer: publishing (the first tag; making the ghcr.io package public), signing
  (cosign or provenance for the image, signatures beyond checksums, notarizing the macOS
  binaries), a chart repository. Not run here: the release workflow, CI's packaging job,
  `helm lint`, `systemd-analyze` (no helm or systemd on this Mac).
- No akio API request; no new dependency. No benchmark (rule 30): the benchmark table's new row
  asks whether musl's allocator costs the static binary throughput against a glibc build.

### 2026-11-25: akio requests A33 and A48 built: a request changed in place, its end heard

- akio (branch `claude/a33-a48`, `2afb267b`, ADR-126 and ADR-127, not on `main` yet), pinned
  here: `Context::req_mut` (A33); `Context::on_end`, `Detached::on_end` and `End { Sent, Gone }`,
  each hook run exactly once, `on_gone` beside them (A48). Tested in akio over HTTP/1.1, HTTP/2 and
  HTTP/3, on epoll and io_uring.
- A33: the `headers` middleware (I40), `basic_auth` and the client-certificate check (I42),
  ForwardAuth (I43), JWT (I44) and the cache's revalidation (I67) change the request's fields in
  place; `rewrite` (I40) changes its target, its query kept; akit-listener makes the target's path
  canonical (I11), as fit's listener changes its request's path and `RequestURI`. The stopgaps go:
  `REQUEST_FIELDS` and `request_headers` (handlers read `ctx.req().headers`), `CANONICAL_PATH`
  (handlers read `ctx.path()`, the proxy sends the request's target). What akio reads of the
  request sees the changes too: the `static` handler's conditional and range fields, the response
  filters. The access log writes the target the listener left, copied for a logged request, since
  fit's `rewrite` leaves `RequestURI` alone.
- A48: `concurrency_limit` (I41) gives a request's place back from `on_end`: a streamed response's
  once its body ends, not at its head, and a waiting request's as soon as its client goes, not when
  its handler lets it go (fit's `TestConcurrencyClientGone` now as fit has it).
- Tests: `akit-middleware` `tests/limits.rs`: the place given back as the client goes, while the
  handler still keeps the request, and given back once only; a streamed response holding its place
  past its head until finished, and one cut short giving it back as its client goes. The existing
  tests of `headers`, `rewrite`, the auth middlewares, the cache, the proxy, the listener's
  canonical path, `static` and `fastcgi` pass with the request changed in place.
- Still different from fit: the access log, metrics and spans time a streamed response to its head
  and write its `Content-Length` as its size (fit: at `OnFinish`, the bytes written): `End` carries
  no byte count; a follow-up (recorded in A48).
- To measure (benchmark task, rule 30): the proxy scenario with `concurrency_limit` (the hooks: an
  `Arc`, two boxes and a peer-gone registration per request, where a boxed filter was), with
  `headers` (fields changed in place rather than encoded into a local and decoded by the proxy),
  and with the access log on (the target copied per logged request).

### 2026-11-25: akio requests A57, A58, A14, A15 and A92 built, their refusals lifted

- akio (branch `claude/a57-a58-a14-a15-a92`, `7d1b1670`, ADR-123 and ADR-124, not on `main`
  yet): `RootCerts::store()` (A57); `ServerConfigBuilder::handshake_only_alpn` (A58); `Files`'
  `index_files`, `redirect_directories`, `mime_types` (`MimeTypes::system`), `default_type`,
  `sniff`, `error_response`, `serve_prefixed`, with 403, regular files only, 404 for a file with
  a trailing slash and `:` allowed (A14); `max_cache_bytes` and `disk_timeout` (A15); a file body
  given a stream coding encoded on the disk pool (A92). akit is pinned to it.
- `static` (I12, I13): `index` takes fit's lists (none for `[]`), `max_cache_bytes` (64 MiB) and
  `default_type` (checked as Go's `mime.ParseMediaType`) are read; a directory without its slash
  is redirected (301, the query kept, not with `try_files`), a file with one is 404, a file that
  may not be read 403, a FIFO 404 at once (never opened), types are Go's with the system's
  `mime.types` and sniffed without `default_type`, the error bodies are fit's (`404 Not Found\n`),
  listings below `strip_prefix` are titled by the whole path, and a request the file pool has not
  answered within fit's 10 s is answered 503. Tests from fit's `TestServe`, `TestNoIndex`,
  `TestPermission`, `TestCacheLimits`, `TestDefaultType`, `TestStripPrefix` and `TestErrors`.
- I32: `Ocsp` verifies a certificate from files against the system's roots when no roots are
  given, as fit's `x509.SystemCertPool`; a TLS listener's `acme-tls/1` connections are closed
  once their handshake is done and serve nothing (I32, I34), tested on a real engine.
- I47: `compress` compresses a static file sent from disk too (all three encodings, over HTTP/1.1
  and HTTP/2), holding its place until the body is sent.
- Compatibility: `max_cache_bytes` and `default_type` supported (371 fields).
- Still different from fit: the 503 of the file pool's timeout is tested in akio (a disk that
  hangs), not through akit; the redirect's `Location` is the path as sent, not decoded.
- To measure (benchmark task, rule 30): static files from disk with `compress` (the pieces read
  and encoded on the file pool) against fit; the static handler's hot path after the `Files`
  changes (cache hits, the two-ring cache).

### 2026-11-25: akio requests A9, A23 and A134 built, their refusals lifted

- akio (branch `claude/a134-a23-a9`, `7521e6b6`, ADR-121 and ADR-122), pinned here:
  `Listener::unix` / `unix_from_std` / `unix_mode` (A23), `tls12_only` public on both TLS
  builders (A9), a CONNECT on an HTTP/2 or HTTP/3 stream dispatched at once and relayed by
  `connect_proxy` on its stream, `H2Client` sending one, and `Conn::shutdown_write_after_send`
  for the client's END_STREAM (A134).
- A9: `listeners[].tls.max_version: "1.2"` is taken; akit-tls builds the listener's TCP
  configuration with `tls12_only` (a reload compares it), HTTP/3 stays TLS 1.3.
- A23: `admin.address: "unix/<path>"` is taken (`"unix/"` alone refused, fit's message): the Admin
  API listens on a socket made 0600, a stale one replaced and one in use refused, no `Host` check
  there (fit's `loopback` is false), the file removed when the API stops; `akit run` prints
  `admin API on unix/<path>`.
- A134: `forward_proxy` no longer answers a CONNECT on an h2 or h3 stream 501 (the stopgap I55
  recorded): it tunnels it, as fit does.
- Tests: fit's `TestTLSMaxVersion` (TCP part: TLS 1.2 over HTTP/1.1 and h2, a TLS 1.3-only client
  refused; `crates/akit/tests/https.rs`), `TestUnixSocket` (`crates/akit-admin/tests/admin.rs`)
  and `TestConnectH2C` (`crates/akit-handler/tests/forward_proxy.rs`, with a refused destination
  over h2c), and the config cases.
- Compatibility gate: `examples/admin` is valid (80 fixtures: 42 valid, 37 refused only as not
  supported yet, 1 skipped, none failed).
- Not measured (rule 30): the CONNECT relay on h2 and h3 against HTTP/1.1's, for the benchmark
  tasks.

### 2026-11-25: I62, shared certificate storage

- `akit-config` reads fit's `acme.shared_storage` (fit's decision 014) with its `redis`, `etcd` and
  `consul` kinds, `prefix`, `lock_ttl` and `request_timeout`, every field, with fit's defaults,
  checks, messages and positions (`checkSharedStorage`, `checkRedisStorage`: exactly one kind,
  `lock_ttl` at least 10s, a Redis address that is a host and port, `username` with
  `password_file`, TLS files with `tls`, `storage` and `shared_storage` not both). Its model is in
  `shared_storage.rs`.
- `akit-tls`: `SharedStore`, a `Storage` that says it is `shared` (a new default method of the
  trait, with `close`), as fit's `sharedstore.Store`. Redis spoken in RESP2 over at most 4
  connections made through akio (TLS when `tls` is set, `AUTH` and `SELECT` when made), its I/O
  thread only appending to a bounded inbox; etcd through its v3 JSON gateway, one endpoint at a
  time, authenticating and again once when the token is refused; Consul's KV store with an ACL
  token and a datacenter. All on an akio engine of the store's own; the calls wait on the caller's
  thread (the control plane's), each within `request_timeout` and its deadline. Locks are leased:
  `SET NX PX` with a random token and token-checking scripts; an etcd lease and a
  create-revision transaction; a Consul session with `Behavior: delete`; renewed every third of
  `lock_ttl` by a thread of each lock's, given back on `unlock` and `close`. Values at most 4 MiB,
  listings at most 100,000 keys, 256 locks held.
- ACME with a shared storage keeps each challenge's answer in it while it is under way
  (`challenges/<CA>/http-01/<token>`, `challenges/<CA>/tls-alpn-01/<name>`), and a marker of
  challenges in progress (two minutes, rewritten every 30s); it waits 1.25s before the CA checks.
  Another instance looks a challenge up only while the marker is good (read at most once a
  second), on a thread of the lookup's own, at most 16 at once and 4 per client (an IPv4 address
  or an IPv6 /64), each within 5s; a name looked up in vain is not looked up again for a second.
  The listener in the clear detaches a request for a token it does not have and answers it from
  the lookup (404 when there is none, rather than routing it, as fit's); a handshake offering
  `acme-tls/1` alone waits for the TLS-ALPN-01 lookup through akio's `CertDone`, never holding its
  I/O thread.
- Tests: fit's `sharedstore` tests (RESP2 replies, Redis over TLS with a user and a database, its
  dial errors and replies, a busy pool, etcd's endpoints, answers and authentication, Consul's,
  the store's errors and locks) against fake servers; fit's `TestStores` (keys shared, locks taken
  in turn, renewed past their TTL, lost once left behind) against fakes with Redis', etcd's and
  Consul's semantics; fit's `shared_test.go` (lookups bounded in all and per client, the marker's
  gate, misses, a listener answering another's TLS-ALPN-01 challenge, what is in memory first);
  and, against Pebble, two managers on one Redis, the CA reaching only the second: one
  certificate, one issuance, through HTTP-01 and through TLS-ALPN-01.
- The compatibility gate: `acme.shared_storage` and its 26 fields supported; the
  `acme-shared-storage` example is valid (80 fixtures, 42 valid, 37 refused only as not supported
  yet, 1 skipped).
- No akio API request (A159–A161 unused); no new dependency. No benchmark: the control plane
  (rule 24).

### 2026-11-25: I73, `akit adapt` and `akit fmt`

- `akit adapt` (`crates/akit/src/adapt.rs`), fit's `fit adapt` with its flags (`-c`, repeated or
  `-` for standard input, `-from json|yaml|fit`, `-to json|yaml|fit`, `-o`), errors and exit
  codes: the config is read and checked as `akit validate` reads it, without compiling it, then
  written byte for byte as fit writes it. A field akit does not run yet is converted as written,
  with a warning, so every config fit reads converts. `-from nginx` and `-from caddyfile` are not
  supported yet (I74).
- `akit fmt`, which fit does not have: each file of a config written again in its own format, as
  `akit adapt` writes it, to standard output, in place (`-w`) or listed when it would change
  (`-l`); the files are checked together first. Comments are not kept, so `-w` leaves a file that
  holds any as it is and names it; a JSON file ends with one line break, so formatting is stable.
- `akit_config::adapt`: fit's adapter set (a lone JSON file's strings and numbers as written, a
  YAML or `.conf` file's as Go's `json.Marshal` writes them, several files merged by fit's rules
  with its conflict messages) and its three writers: Go's `json.Indent` (trailing space kept, as
  fit's); `adapter/yaml.FromJSON`, a port of the part of go-yaml v3.0.5's emitter and resolver it
  reaches (block styles, the four scalar styles and their analysis, a string that reads as a
  boolean, number or timestamp quoted, by Go's `strconv` and `time.Parse` rules); and
  `adapter/fitconf.FromJSON` with its compaction, typed by the field inventory the `.conf` reader
  uses (the shorthands, units for bytes, module params by module).
- The `.conf` reader reads a number a float64 cannot hold (`1e400`) as a string, as fit's
  `isNumber` does.
- Tests: golden files fit itself wrote (`tools/adapt-golden/run.sh`, against fit's
  `modules/standard`): `fit adapt` on akit's copies of fit's templates (file and `conf.d`, in all
  three formats), its 74 examples and its `cli/testdata` configs, each to all three formats (249
  cases), and fit's two writers on 3819 documents (the cases' outputs, fit's writer tests, and
  documents built from strings and numbers each writer treats specially); akit's output is the
  same byte for byte in every case. Every template and example round trips through YAML and
  `.conf` to the same config, and writing the output again gives the same text. fit's `adapt`
  CLI tests are ported, with tests of the merge, comments and `fmt`.
- No benchmark: control plane (rule 24).
- Checked: `make check` on macOS; on gpu51, the touched crates' tests (`akit-config`, `akit`)
  with epoll and with `AKIO_TEST_BACKEND=uring`, through `tools/test-timeout.sh`: all passed
  but `akit`'s `reloads_under_traffic_with_no_failed_request` once on io_uring, a known flake
  under load, which passed 3/3 run alone.

### 2026-11-25: I60, the Kubernetes Gateway API

- `akit-config`: a `kubernetes` provider no longer needs `ingress`: it serves the Gateway API, as
  fit's always does, and `ingress` adds Ingresses. `addresses` is read, with fit's check (an IP
  address or a host name, no wildcard). `allow_external_name_services` stays not supported yet
  (it needs the upstreams' `deny_internal`).
- `akit-provider::kubernetes`, fit's `gateway.go`, `grpc.go`, `l4.go` and `status.go` on I59's
  translator (new `gateway.rs`, `status.rs`): the Gateways of GatewayClasses of
  `controller_name`, oldest first, each listener a listener of akit's for its port and family
  (HTTP; HTTPS with its Secrets' certificates and TLS passed through, told apart by server name,
  on one port; TCP; UDP, `<name>-<port>-udp`), refused (unknown protocol, no port, TLS
  terminated on a TLS listener), in conflict (another family on the port) or with unresolved
  references (certificateRefs, route kinds) as fit says; HTTPRoutes (each match a route: Exact,
  PathPrefix and RegularExpression paths, exact headers and query parameters, methods, the API's
  precedence as the priority, host names intersected with the listeners', `*.` as `**.`;
  RequestHeaderModifier, ResponseHeaderModifier, URLRewrite, RequestRedirect, one
  RequestMirror; weighted backends; 500 and 503 as the API says) and GRPCRoutes (methods as
  paths, over h2c); TLSRoutes, TCPRoutes and UDPRoutes as services (a TLSRoute's host names its
  `server_names`, one route to a listener's names, several backends one weighted upstream);
  parentRefs by section and port, allowedRoutes' namespaces (Same, All, Selector on the
  Namespaces' labels) and kinds, ReferenceGrants for Services and Secrets in other namespaces.
  The status of akit's GatewayClasses, Gateways (`addresses` written in), listeners
  (supportedKinds, attachedRoutes, Accepted, Programmed, ResolvedRefs, Conflicted) and of each
  route on each of akit's Gateways (Accepted, ResolvedRefs, PartiallyInvalid; other controllers'
  entries kept; at most 32) is written back as JSON merge patches naming the resource version
  they were made from, only where it differs, a condition keeping its time while its status
  stays. The writes go one at a time on the provider's thread, the last set replacing what was
  left of the one before; a write refused (409) or of an object gone is left to the watch's next
  change, another failure reported once. The client gains `PATCH .../status`. The kinds watched
  are fit's, TLSRoutes at v1, v1alpha3 or v1alpha2 and ReferenceGrants at v1beta1 or v1, the
  first the server serves; Ingresses and IngressClasses only with `ingress`. A kind not served
  says "(is the Gateway API installed?)", as fit's.
- Tests: `crates/akit-provider/src/kubernetes/gateway_tests.rs` ports fit's
  `TestTranslateGateway`, `TestTranslateOptions`, `TestPrecedence`, `TestIntersect`,
  `TestSelects`, `TestFilters`, `TestMirror`, `TestHTTPS`, `TestGRPCRoute`, `TestL4Routes`,
  `TestTranslateEdges` (ExternalName Services refused, as `allow_external_name_services` is not
  supported yet), `TestSecretTooLarge`, `TestStatus`, `TestStatusEdges` and `TestDecodeKinds`
  for the new kinds, every part checked to be a config akit takes; `TestIngressPorts` and
  `TestIngressEdges` now with their Gateway holding a port; `status.rs` `TestObjectPath`.
  `crates/akit-provider/tests/kubernetes.rs`: the fake API server applies status merge patches
  (409 for another resource version), and runs fit's `TestProvider` for the Gateway API (a kind
  served in neither of its versions, pages, a route through its watch) and `TestProviderStatus`
  (statuses written once and then left, a refused write made again, a TLSRoute at v1alpha2, an
  HTTPS listener's certificate file written and removed with its Secret).
- The conformance suite: [tests/conformance](../tests/conformance/README.md) runs fit's suite
  module against `akit run` and a kind cluster, as fit's decision 012 does (`run.sh`, akit's
  GatewayClass `akit` of controller `lesismal.github.io/akit`). It needs a cluster, so it is the
  maintainer's to run; `GATEWAY-HTTP` has not been run against akit yet.
- Compatibility gate: `providers[].kubernetes.addresses` supported; `make compat-bless`:
  `examples/provider-kubernetes` is valid (80 fixtures: 42 valid, 37 refused only as not
  supported yet, 1 skipped, none failed). Inventory: 370 supported.
- No akio request (A156–A158 unused): akio's clients already send PATCH with a body over HTTP/2
  and HTTP/1.1. The control plane: no benchmark (rule 24).
- Checked: `make check` on macOS; on gpu51 under the bench lock, `akit-provider`, `akit-config`,
  `akit-compat` and `akit` on epoll and `AKIO_TEST_BACKEND=uring`, each test binary through
  `tools/test-timeout.sh` (rule 31).

### 2026-11-24: I61, the Consul and etcd providers

- `akit-config` reads fit's `providers` with the `consul`, `consul_kv` and `etcd` kinds, every
  field, with fit's defaults and checks, messages and positions (`checkProviders`: one kind each,
  the name's pattern, no base source's name, unique across the files; `checkEndpoint`,
  `checkTLSFiles`, the tag prefix, `username` with `password_file`); `docker` and `kubernetes`
  are read by I58 and I59. The three kinds join their shared model (`providers.rs`); their
  settings are in `providers_kv.rs`. A name's problem is reported at
  the provider's object, where fit points at the name.
- `akit-provider`: `watch`, fit's `provider/internal/watch` follower on a thread of its own: the
  source opened, its watch on a second thread, the source read once the watch is in place, after
  changes (debounced 200ms) and every `refresh_interval`, a part sent when it changed (or on the
  interval), each problem logged once while it lasts, failures retried 1s to 30s apart; a `Stop`
  cancels a round's sleeps and akio requests (their `CancelToken`) and the threads are joined
  before `stop` returns. `api`: each provider's akio engine (one client thread), its TLS from the
  files, requests made from the provider's threads and waited for, every one with a timeout,
  answers at most 32 MiB. `kvpart` (fit's): the keys' parts joined in key order, `max_keys`.
  `labels`: fit's `provider/labels` (the Docker provider's too, I58). `consul`: the catalog
  (`/v1/catalog/services` and `/v1/health/state/any` polled with blocking queries, the services
  enabled by tag read with `?passing`, tags as labels, `max_services`) and KV (`/v1/kv/<prefix>
  ?recurse`, base64 values) providers, the token read from `token_file` at each watch, `dc`;
  an index that goes back starts over, one below 1 is 1, queries a poll 1s apart. `etcd`: etcd's
  v3 JSON gateway as fit has it (fit's decision 013), not gRPC as the plan said: `/v3/kv/range`
  over the prefix's range with `limit`, `/v3/watch` streamed and parsed line by line on the
  client's I/O thread (a line at most 32 MiB), created within `request_timeout`, a canceled
  watch (as after a compaction) started over, the endpoints taken in turn, a token from
  `/v3/auth/authenticate` when `username` is set. Beyond fit, the watch asks for
  `progress_notify` and is given up after 15 minutes of silence (rule 10). `API_KINDS` makes the
  three kinds for `Kinds`.
- `akit run`: the providers the config lists are made by kind, added to `Configs` and started
  (`akit: provider <name> watching ...`, fit's words), their parts applied as I57 applies them,
  and stopped before akit exits. Read when akit starts, as fit's.
- Tests: `crates/akit-provider` ports fit's `TestFollower`, `TestFollowerInterval`,
  `TestFollowerWatchEnds`, `TestFollowerReadFailsAfterChange`, `TestPart`, `TestParse`,
  `TestParseRefused`, Consul's `TestTranslate`, `TestCatalog`, `TestCatalogMaxServices`, `TestKV`,
  `TestEscapePath`, `TestKinds`, `TestOpenFails`, `TestCatalogReadFails`, `TestKVReadFails`,
  `TestGetFails`, `TestPollIndexZero` and `TestTranslateNoAddress`, and etcd's `TestProvider`,
  `TestProviderMaxKeys`, `TestProviderWatchNotCreated`, `TestProviderBadPassword`,
  `TestRangeEnd`, `TestReadWatch`, `TestReadWatchNoResult`, `TestKind`, `TestOpenFails` and
  `TestCallFails`, against fakes of Consul (blocking queries) and etcd's gateway (streamed
  watches) on threads of their own; `crates/akit-config/tests/providers.rs` fit's provider cases;
  `crates/akit/tests/providers.rs`, end to end: `akit run` serves the routes a Consul key holds,
  and the new ones when it changes.
- Compatibility gate: 34 fields more; `make compat-bless`: `examples/provider-consul`,
  `provider-consul-kv` and `provider-etcd` are valid (80 fixtures: 41 valid, 38 refused only as
  not supported yet, 1 skipped, none failed, with I58 and I59 merged).
- No akio request (A141–A143 unused): akio's HTTP client already streams responses, cancels by
  token and times every phase. The control plane: no benchmark (rule 24).
- Checked, after merging main (I55, I58, I59, I75, rule 31): `make check` on macOS; on gpu51
  under the bench lock, `akit-provider`, `akit-config`, `akit` and `akit-compat` on epoll and
  `AKIO_TEST_BACKEND=uring`, each test binary through `tools/test-timeout.sh` (rule 31).

### 2026-11-24: I59, Kubernetes Ingress

- `akit-config` reads fit's `providers` (new file `providers.rs`: `Provider`, `ProviderKind`,
  fit's `checkProviders`: one kind each, names valid, unique and none of the base config's
  sources) with the `kubernetes` kind's `kubeconfig`, `context`, `controller_name`,
  `namespaces`, `listen_host`, `backends`, `max_objects`, `request_timeout`, `ingress`
  (`http_port`, `https_port`) and `certificates_dir`, fit's defaults and checks. Not supported
  yet: a kubernetes provider without `ingress` (the Gateway API, I60), `addresses` (the Gateways'
  status, I60) and `allow_external_name_services` (needs the upstreams' `deny_internal`); the
  consul, consul_kv and etcd kinds (I61). The kind is a `ProviderKind` variant beside I58's
  Docker, in the same `providers.rs`. The gate's `provider-kubernetes` example is
  now refused only for `addresses`.
- `akit-provider::kubernetes`, fit's `provider/kubernetes` for Ingresses: a client of akit's own
  on akio's (fit's decision 011) that lists each kind in pages of 500 (64 MiB at most a page, a
  continue token that expired starting the list over) and watches it from the list's resource
  version (bookmarks, five minutes a watch, given up after ten silent ones, a 410 listing again),
  over HTTP/2 on one connection when the server is `https` and chooses `h2`, over HTTP/1.1
  otherwise (in the clear, or when ALPN chooses it); the kubeconfig (its context, server, CA,
  `tls-server-name`, `insecure-skip-tls-verify`, client certificate, `token` or `tokenFile`, read
  again every minute; exec and auth-provider plugins refused), the pod's service account, or
  `$KUBECONFIG` and `~/.kube/config`. Watch events are decoded and applied to the kind's store on
  the provider engine's I/O thread as they come (a short lock; at most 16 MiB an event; past
  `max_objects` left out and reported); the provider's thread lists, watches again, backs off,
  waits `debounce` after a change, sends the part when it changed and every `resync`. The
  translator is fit's `ingress.go` with what it uses of `gateway.go`: Ingresses of an IngressClass
  of `controller_name` (or none, when such a class is the default), in the namespaces watched,
  each path a route on `<name>-<http_port>` and, for its `tls` hosts, `<name>-<https_port>` with
  its Secrets' chain and key written to files named by their hash (removed when no longer named;
  a directory of the provider's own, removed when it stops, without `certificates_dir`); fit's
  priorities, path types and annotations (`fit.lesismal.github.io/`: `ssl-redirect`,
  `rewrite-target`, `backend-protocol`, `limit-rps`, `proxy-body-size`, `allow-source-range`,
  `request-timeout`); Service ports to upstreams of their ready endpoints or cluster IP, h2c by
  annotation or `appProtocol`. What is left out is reported once while it lasts. Ingresses have no
  status of akit's, as fit's.
- `akit run` starts the providers the config it starts with lists (fit's `startProviders`:
  `akit: provider k8s watching Kubernetes for the Ingresses of ...`), stops them on shutdown; a
  reload that changes `providers` is warned of ("take a restart").
- Tests: `crates/akit-provider/src/kubernetes/tests.rs` ports fit's `TestIngress`,
  `TestIngressPorts` and `TestIngressEdges` (but for a Gateway holding a port, I60), each part
  checked to be a config akit takes, `TestParseSize`, `TestKubeconfig`, `TestLoadConfigEdges`,
  `TestTokenRotates`, `TestCertFiles`, `TestDecodeKinds` and `TestInformerWatch` (events split at
  every size); `crates/akit-provider/tests/kubernetes.rs` runs the provider against a fake API
  server on akio's HTTP server (fit's `apiServer`): `TestProvider` and `TestProviderIngress` over
  HTTP/2 (pages, a watch too old, a watch ended, the Secrets' field selector),
  over HTTP/1.1 in the clear and over TLS, a kind not served, `TestProviderUnauthorized`,
  `TestProviderSetupFails`, and an Ingress's TLS end to end (the file written, removed). No kind
  cluster: the fake server is fit's way, and needs no Docker.
- The control plane: no benchmark (rule 24).

### 2026-11-24: I55, SOCKS and CONNECT

- `akit-config` reads fit's forward proxy, `config.ForwardProxy`: a listener's `socks` (with the
  `socks` protocol, `Protocol::Socks`) and the `forward_proxy` handler's params (`clients`,
  `allow`, `deny`, `ports`, `connect_timeout`, `idle_timeout`; `ForwardProxy`, `Destination`),
  with fit's defaults (10s, 5m) and checks, messages and positions: "socks needs the listener's
  socks", "socks is for a listener whose protocols include socks", `checkForwardProxy`'s, and "a
  socks listener, not routes" (at the route, where fit points at the listener's name, as akit
  already does for tcp and udp). `"socks"` and `forward_proxy` are no longer "not supported yet".
- `akit-l4`: `forward`, fit's `l4.Rules` and `internal/netguard`: a destination decided by what
  was asked before the dial (`Rules::dest`, fit's verdicts) and by the address once connected
  (`Rules::resolved`: `deny`'s networks, `allow`'s for a name it does not hold, and the refusal of
  loopback, link-local, multicast, unspecified, the metadata addresses and the host's own unless
  an `allow` network inside names it); `Rules::socks` builds akio-socks's server (CONNECT without
  authentication; BIND and UDP ASSOCIATE refused with reply 7), `Rules::proxy_config` its tunnels'
  config (handshake within 10s plus `connect_timeout`, `connect_timeout`, `idle_timeout`).
- `akit-listener`: a listener serving `socks` alone serves the SOCKS server; one serving it beside
  HTTP or `tcp` has akio-mux route a SOCKS5 or SOCKS4 greeting to it (the route I53 left to I55),
  behind a PROXY header too. Its tunnels dial on the engine the graph's slot gets
  (`Graph::engine`, new). A reload that changes a listener's `socks` binds it anew, as fit's
  `binding.same` compares the rules.
- `akit-handler`: `forward_proxy`, fit's `handler/forward`: the client (`Context::peer`, the PROXY
  header's when there is one) and the destination as written checked first (403), then
  akio-socks's `connect_proxy_with` with the same rules (502 for a name refused once connected or
  a destination not reached, 504 past `connect_timeout`); 405 for other methods and extended
  CONNECT, 400 for a target not `host:port`. Over HTTP/1.1 only: a CONNECT on an h2 or h3 stream
  is answered 501, a stopgap until akio API request A134.
- akio: A132 and A133, built on akio's branch `claude/a132-a133-socks-rules` (`9b096d0d`, from
  `e9891b52`; ADR-119), pinned: `ProxyConfig::connected` (a tunnel's upstream decided once open,
  with the name asked for and both addresses), `ProxyConfig::connect_timeout` (answered SOCKS5 4,
  HTTP 504), and the SOCKS access hook's client from a trusted PROXY header. A134 (CONNECT over
  HTTP/2 and HTTP/3) requested.
- Tests: `crates/akit-config/tests/forward_proxy.rs` ports fit's "listeners of many protocols,
  and socks" case, with the fields' defaults and the handler's params;
  `crates/akit-l4/src/forward.rs` fit's `TestRulesDest`, `TestRulesInternal`, `TestInternal` and
  `TestRefused`; `crates/akit-listener/tests/socks.rs`: CONNECT by address and by name, SOCKS4
  and 4a, BIND and UDP ASSOCIATE refused, every refusal (nothing sent to the destination), 8 MiB
  each way with a slow upstream and a slow client, a side cut midway, the idle and connect
  timeouts, a PROXY header's client, a reload changing the rules; `one_port.rs`: SOCKS in fit's
  `TestSniffClear` and `TestOnePortManyProtocols` (beside HTTPS, h2, TLS passthrough and `tcp`;
  HTTPS and SOCKS alone); `crates/akit-handler/tests/forward_proxy.rs`: fit's `TestConnect` and
  `TestConnectIdleTimeout`, 405, the connect timeout, h2c answered 501, slow peers and a cut.
- Compatibility gate: 15 fields more; `make compat-bless`: `examples/socks` and
  `examples/forward-proxy` are valid (80 fixtures: 35 valid, 44 refused only as not supported
  yet, 1 skipped, none failed).
- Checked: `make check` on macOS; on gpu51 under the bench lock, the touched crates' tests
  (akit-config, akit-l4, akit-listener, akit-handler, akit-runtime, akit, akit-compat) and
  akio-socks's on epoll and `AKIO_TEST_BACKEND=uring`.
- To measure, in the maintainer's benchmark tasks (rule 30): see the dev plan's benchmark row for
  I55.

### 2026-11-24: I75, FastCGI

- The `fastcgi` handler (`crates/akit-handler/src/fastcgi/`), as fit's `handler/fastcgi`:
  requests answered by a FastCGI application, as PHP-FPM is. The request's CGI/1.1 parameters
  (fit's `params.go`: what the request is, `SCRIPT_NAME`, `SCRIPT_FILENAME`, `PATH_INFO`,
  `PATH_TRANSLATED` under `root`, `REMOTE_ADDR`, `HTTPS`, `REDIRECT_STATUS`, each field as
  `HTTP_*` but `Proxy` (httpoxy) and fields with `_`, the fields as a `headers` middleware left
  them, then `env`) and its body go to the application as records, and the response's CGI header
  gives the status (`Status`, 302 with a `Location`) and fields, hop-by-hop ones dropped. A path
  splits into a script and its path info by `split_path` (any case); a path ending in `/` runs
  `index`; `try_files` looks at the disk on akio's logical pool (`Context::executor`), never on an
  I/O thread. Dotfiles: a script with a segment starting with a dot is answered 404 (but
  `/.well-known/`) unless `dotfiles` is `allow`, and `try_files` passes over it; a `..` segment,
  a NUL or a backslash, escaped or not, 400.
- Streamed both ways with linked backpressure (`exchange.rs`), as the proxy's relay: a body over
  64 KiB, or of no declared length, goes up as `STDIN` records as it arrives, the client's reads
  held while more than 64 KiB waits in the application's connection and resumed on its drain
  (what arrives before the connection is there waits, the reads held); past the listener's
  `max_body_bytes`, 413. A response whose end comes in the same read as its header is sent whole,
  with its length; any other streams as it arrives, the application's reads held while more than
  64 KiB waits for the client and resumed on the client's drain. One that breaks off after it
  began closes an HTTP/1.1 client's connection, and on HTTP/2 and HTTP/3 resets the stream or,
  of no declared length, ends it with fit's `Fit-Error` trailer.
- The connections (`pool.rs`), fit's pool: dialed through akio's engine (a host name resolved off
  the I/O threads by akio's resolver, a Unix-domain socket with `unix/` and a path), one request
  at a time on each, at most `max_conns` (64), `max_idle_conns` (16) kept idle and closed after
  `idle_timeout`, `max_pending` (1,024) requests waiting for one, past it 503. A request with no
  body and a method safe to repeat that took an idle connection the application closed unanswered
  (PHP-FPM's `pm.max_requests`) is sent again, once, on a new connection. A record on an idle
  connection, a record after the end, or a request ended other than as FastCGI says closes the
  connection. Timeouts: `connect_timeout`, `response_header_timeout`, `response_idle_timeout`
  (not counting time the client holds the response back) and `timeout`, checked with the idle
  connections' and the dials' by one sweep a handler every 100 ms on akit's timer thread (A45's
  stopgap), not a job a request in the table every user of that thread shares; 504 for each. The client gone at any point ends the
  exchange and closes its connection. What the application writes to its standard error is
  logged, at most 100 lines a second, on akio's logical pool.
- akio: dialing a Unix-domain socket is akio API request **A135** (`Engine::dial_unix`, ADR-120),
  built by I58 on akio's branch `claude/a135-unix-dial` (`0f2c419b`, two commits on `e9891b52`); akit pins
  that commit (every akio crate, `AKIO_REV`, the version test). The address a request came to is
  not known to a handler, so `SERVER_ADDR` is empty and `SERVER_PORT` comes from `Host` (or 80,
  443), as fit's when its connection is not TCP: akio API request **A148**.
- Config: `address` (a host and port, or `unix/` and a path), `root` (absolute), `split_path`,
  `index`, `try_files`, `dotfiles`, `env`, `max_conns`, `max_idle_conns`, `max_pending`,
  `connect_timeout`, `response_header_timeout`, `response_idle_timeout`, `timeout`,
  `idle_timeout` and `max_header_bytes`, with fit's defaults, checks and messages. Compatibility
  gate: 18 fields more; `make compat-bless`: `examples/fastcgi` is valid (80 fixtures: 34 valid,
  45 refused only as not supported yet, 1 skipped, none failed).
- Tests: `crates/akit-handler/tests/fastcgi.rs` runs a scripted FastCGI application of its own,
  as fit's `fakeApp`, and ports `TestParams`, `TestKeepAlive`, `TestLargeBodies` (10.5 MiB up,
  chunked, 16 MiB down to a slow client), `TestTimeoutsAndLimits`, `TestCutConnections`,
  `TestTryFilesAndUnix` (a Unix-domain socket too), `TestDotfiles`, `TestResponses`,
  `TestIdleConnections`, `TestQueued`, `TestClientsGo`, `TestBodies`, `TestOverallTimeout`,
  `TestNoApplication`, `TestStaleIdle`, `TestProvisionErrors` and `TestDotfilesProvision`, over
  HTTP/1.1 and h2c; unit tests port `TestSplit`, `TestRecords` and `TestParamsTLS`. fit's
  `TestPHPFPM` (its e2e test, PHP-FPM serving scripts: parameters, forms, a 3 MB upload, status
  and cookies, redirects, 4 MiB down, a front controller) is `php_fpm`, run against the PHP-FPM
  binary `AKIT_PHP_FPM` names over a Unix-domain socket, and skipped without it.
- Not done: no PHP-FPM is installed on this Mac or on gpu51, so `php_fpm` has not run yet: the
  plan's "tests against php-fpm pass" waits on it. A static handler on a file system that folds
  case does not yet refuse the files `split_path` names (fit's `dotfile.AddScripts`); fit's
  Windows rules (8.3 short names, aliases) do not apply to akit. fit's `TestDrainAndCleanup` is
  not ported as such: the idle connections close when the config is released (`on_cleanup`),
  after its last request; there is no drain that cuts busy ones. fit's `cover_test.go` (its
  internals) is not ported.
- Checked: `make check` on macOS; on gpu51 under the bench lock, the tests of `akit-handler`,
  `akit-config`, `akit-compat` and `akit` on epoll and `AKIO_TEST_BACKEND=uring`.
- No benchmark (rule 30): what should be measured is in the dev plan's benchmark row for
  FastCGI.

### 2026-11-24: I58, the Docker provider

- `akit-config` reads fit's `providers` (`crates/akit-config/src/providers.rs`): `name` and
  `docker` (`endpoint`, `expose_all`, `network`, `use_published_ports`, `label_prefix`,
  `max_containers`, `refresh_interval`, `request_timeout`) with fit's defaults and
  `checkProviders`' messages (exactly one kind, the name's pattern, not a base source, unique
  across files, `unix://<path>` or `tcp://<host>:<port>`, the prefix's pattern). `consul`,
  `consul_kv`, `etcd` and `kubernetes` stay not supported yet (I59–I61: a `ProviderKind` variant
  each). Read when akit starts: a reload that changes them warns that they take a restart.
- `akit-provider`: `labels` is fit's `provider/labels` (routes, services and an upstream's
  settings from labels under a prefix, and the part they make, the same for the same labels),
  for Consul's tags too in I61. `docker` is fit's `provider/docker`: containers' labels turned
  into upstreams (replicas by Compose project and service, or `name`), routes and layer 4
  services, those left out reported once each; `GET /events` (container events, `exec_` ones
  passed over) and `GET /containers/json` on akio's HTTP client, over the Unix socket with A135
  or over TCP. The event stream is read and the listing collected by callbacks on akio's I/O
  threads, which parse what came (an event at most 1 MiB, a listing at most 16 MiB, an error's
  body at most 4 KiB) and tell the provider's thread; that thread keeps fit's timings (200 ms of
  debounce after events, a listing every `refresh_interval` whether or not one came, the backoff
  from 1 s to 30 s after the daemon fails, the part sent again after a reconnect) and sends the
  parts. `request_timeout` bounds a listing and the head of the event stream; the stream itself
  may stay quiet (the refresh notices a daemon gone). The provider runs on an engine of its own
  (one client thread), as the OTLP exporter does.
- `akit run` makes the providers the config lists, in their order, names them to `Configs` and
  starts them on I57's `Updates` (fit's `startProviders`, its "built without the ... provider"
  message), and stops them at shutdown.
- akio A135 (ADR-120), built on akio's branch `claude/a135-unix-dial` (`0f2c419b`, off
  `e9891b52`) and pinned: `Engine::dial_unix` (epoll and io_uring), `UNIX_ADDR`,
  `akio_sys::connect_unix_nonblocking`, `Client::with_unix_socket`.
- Tests: `crates/akit-provider` ports `TestTranslate`, `TestTranslateOptions`,
  `TestTranslateEdges`, `TestProvider`, `TestProviderRefresh`, `TestProviderNoDaemon`,
  `TestProviderListFails`, `TestClientFails` (over TCP), `TestReadEvents` and `TestKind`, and
  the labels' `TestParse` and `TestParseRefused`, against a fake daemon on a Unix socket, the
  client on the backend under test; an event past the limit and one not JSON end the stream.
  `crates/akit-config/tests/providers.rs` ports `config_test.go`'s provider cases.
  `crates/akit/tests/docker.rs` runs fit's Docker Compose example's config against a fake daemon:
  services routed as their containers start and stop, the fallback route kept.
- Compatibility gate: 11 fields; `examples/docker-compose` and `examples/provider-docker` valid
  (80 fixtures: 35 valid, 44 refused only as not supported yet, 1 skipped).
- No benchmark (rule 24: the control plane).

### 2026-11-24: I69, Cache III: purge and tags

- Tags, as fit's `tagsOf`: a response kept is tagged by the members of its `Cache-Tag` fields
  (comma-separated) and its `Surrogate-Key` fields (space-separated), each once, at most 32 of
  them, none longer than 128 bytes; they count against the zone's size. Each of the memory tier's
  16 shards keeps an index of its keys by tag, under the shard's lock, so that storing a tagged
  response takes no lock of the zone's (rules.md rule 13); the disk tier's index keeps its own,
  rebuilt from the files' fields when it is read back (the file format is unchanged).
- `akit_cache::purge(zone, &Selection)`, as fit's `PurgeBy`: what a zone keeps (every zone's for
  `""`), in memory and on disk, removed by URL (an absolute `http` or `https` URL, its host in any
  case, every variant), by URL prefix (`https://example.com/img/`; one with user information or a
  fragment refused), by tag, or all of it; it returns how many keys it removed, fit's count. A key
  is found under each shard's lock and the disk index's for as long as the lookup takes (a prefix
  goes through the keys); the files go on the disk pool, as an eviction's do. A purge drops the
  writes still queued for what it removes, as an invalidation does, and a clear drops every write
  queued before it and every file read back after it, so nothing purged comes back on disk.
  `akit_cache::zones()` is fit's `Zones`.
- Admin API: `GET /cache` (each zone's `zone`, `keys`, `bytes`, and `disk_entries`, `disk_bytes`,
  `disk_path` when it has a disk tier) and `POST /cache/purge` (`{"zone", "urls", "prefixes",
  "tags", "all"}` as JSON within `max_config_bytes`, streamed in as a config is; `{"purged": n}`;
  404 for a zone no cache uses, 400 for a body that names nothing or a URL that is not one, 415
  for a body not sent as JSON), as fit's `admin/cache.go`. Both run on the API's I/O thread;
  `akit-admin` depends on `akit-cache` for them, as fit's admin package imports its cache.
- Tests: `crates/akit-cache/tests/cache.rs` ports fit's `TestPurge` (URL, tags, prefixes, all, in
  memory and on disk, the refusals and `Zones`) and adds a key on disk alone purged and a purge
  dropping writes queued behind a held disk pool; unit tests port `TestTagsOf` and cover the tag
  index through replaced variants, evictions, removals and a clear, and the URL keys;
  `crates/akit-admin/tests/cache.rs` ports `TestCachePurge` against a served listener, with the
  415 and 405 answers. No config field: `supported.txt` has a comment for I69, the gate report and
  the inventory are unchanged.
- No akio request (A144–A146 unused). Checked: `make check` on macOS; on gpu51 under the bench
  lock, `akit-cache`, `akit-admin` and `akit` on epoll and `AKIO_TEST_BACKEND=uring`.
- No benchmark (rule 30). To measure: the tag index's cost on storing tagged responses (none and
  32 tags) against untagged ones, and purges by prefix and by tag of a large zone while it serves
  (how long the shards' and the disk index's locks are held); recorded in the dev plan's response
  cache row.

### 2026-11-24: I64, OpenTelemetry

- `akit-config` reads fit's `telemetry` (`endpoint`, `headers_file`, `ca_file`, `cert_file`,
  `key_file`, `service_name`, `timeout`, `traces.sample_ratio`, `traces.max_queue_bytes`,
  `traces.interval`, `metrics.interval`) with fit's defaults and checks, messages and positions
  (`checkEndpoint`, `checkTLSFiles`, "set traces, metrics or both", the ratio from 0 to 1), given
  by one file; `parse_headers` is fit's `ParseHeaders`.
- `akit-observe`: `trace`, fit's tracer: `traceparent` read as fit reads it (a later version as
  far as 00 goes; an invalid one not followed), a sampled caller followed, an unsampled one never
  traced, `sample_ratio` for the rest (a thread's own wyrand); a span encoded as fit's OTLP JSON
  (`GET route`, kind server, fit's attributes, status error for 5xx, `url.path` percent-decoded)
  into the thread's buffer and copied into one of 16 shards, `max_queue_bytes` bounding them all
  (past it dropped and counted), exported by a thread of its own every `interval` or once 256 KiB
  wait; `close` exports what waits. `otlp`: `Otlp`, the receiver (headers, TLS from the files,
  the resource with `service.name` and `service.version`), posting through akio's HTTP client on
  an engine of its own with one client thread; failures logged once while they last, and their
  end; `MetricWriter` (Prometheus text or `OtlpWriter`, fit's OTLP metrics) and
  `MetricsExporter`, every `metrics.interval`, once more on close. No akio request: akio-export
  and akio-http's request spans are not used ([decision 007](decisions/007-opentelemetry.md)).
- `akit-listener`: the dispatcher asks the graph's tracer per request (one `traceparent` field, or
  none); a traced request gets `akit.traceparent` (`akit_runtime::TRACEPARENT`) and its span is
  ended by the response hook the access log and the metrics share, with the proxy's upstream
  address. `Scrape::write_metrics` writes through a `MetricWriter`, with `fit_trace_spans_total`.
  `akit-proxy` sends the upstream akit's `traceparent` in place of the client's, last, as fit's;
  `tracestate`, and an untraced request's `traceparent`, go on as they came. `akit-runtime`:
  `Env::with_tracer`, `Graph::tracer`; `telemetry.metrics` counts requests as `metrics` does.
- `akit run`: the exporters start from the `telemetry` akit starts with (a file it cannot read
  stops it); a reload that changes `telemetry` is warned of ("takes a restart"); the metrics
  exporter and then the tracer are closed after the listeners, so every span finished is exported.
- Tests: `crates/akit-observe/tests/telemetry.rs` ports fit's `TestSample`, `TestTracerExport`,
  `TestTracerBounds`, `TestTracerTicks`, `TestTracerFlushesWhenFull`, `TestSpanProtocols`,
  `TestExporterErrors`, `TestExportFailures`, `TestMetricsExporter` and
  `TestMetricsExporterTicks` (a receiver of its own), with spans from eight threads at once; the
  unit tests `TestParseTraceparent`, `TestSampledFlagShort` and `TestOTLPWriterIgnores`;
  `crates/akit-config/tests/telemetry.rs` fit's "telemetry settings" cases and `TestParseHeaders`;
  `crates/akit/tests/telemetry.rs`, end to end: `akit run` traces the requests a caller sampled,
  the upstream gets akit's span in the caller's trace, an unsampled `traceparent` goes on as it
  came, and the spans (proxied, answered, 404) and metrics reach a collector as akit stops.
- Compatibility gate: 14 fields more; `make compat-bless`: `examples/telemetry` is valid (80
  fixtures: 31 valid, 48 refused only as not supported yet, 1 skipped, none failed).
- Checked: `make check` on macOS; on gpu51 under the bench lock, the workspace's tests on epoll
  and `AKIO_TEST_BACKEND=uring`, all passing (the compat gate's samples, added after that run
  began, passed in a second run on both).
- To measure, in the maintainer's benchmark tasks (rule 30): see the dev plan's benchmark row for
  I64.

### 2026-11-24: I63, the dashboard

- `akit-admin` serves fit's dashboard at `/dashboard/`: fit's three files (`index.html`,
  `app.js`, `app.css`, at fit `7daee573`, named akit), built into the binary, plain HTML, CSS
  and JavaScript that fetch nothing from elsewhere and send nothing but GETs. Tabs: overview
  (config version, reloads, connections, routes, backends up, certificates, listeners, sources),
  routes, layer 4 services, upstreams and their backends' health, certificates, versions (each
  one viewable). Served with fit's headers (`DASHBOARD_CSP`, `nosniff`, `X-Frame-Options: DENY`,
  `no-referrer`, `no-cache`) and content types; `/dashboard` and `/dashboard/index.html`
  redirected to `/dashboard/` (301), as Go's file server does; other methods 405.
- The guard, as fit's: the dashboard's files hold no data and are served without the token (the
  page asks for it, keeps it in the tab's `sessionStorage` and sends it); a request with `Origin`
  passes only as a GET or HEAD from the API's own origin, so the page reads and nothing writes.
  `admin.disable_dashboard` (supported now) leaves the pages out (404).
- `GET /status`, fit's payload: the running config's version, when it was applied and its
  source, from the history; then `akit_admin::Options::status` (`Live`), which `akit run` sets to
  akit-listener's `Scrape::write_status`: reloads (succeeded, failed), the listeners served
  (name, bound address, protocols, from `Listeners::view`, a `ServedView` each reload updates),
  connections (open, refused, pending bytes, from akio's telemetry), upstreams by name with
  their in-flight count and each backend's address, up, `probed_down`, in flight, succeeded and
  failed, the certificates by listener then expiry (names: DNS names, then IP addresses, else the
  common name; issuer, serial, `not_before`, `not_after`), and with `metrics` the requests
  answered since start by class (I37's `Metrics::totals`). `akit_tls::CertificateInfo` has the
  names, issuer and `not_before` now.
- `GET /config/effective`, fit's payload for a config without providers (I57 brings them): the
  version, ETag, `precedence` (the config's source alone), the running config, and a source for
  each top-level member and each element of `listeners`, `routes`, `upstreams` and `services`,
  as fit's merge lists a base config's. Its other methods still edit the member `effective`.
- Tests: `akit-admin/tests/dashboard.rs` ports fit's `TestDashboard`, `TestDashboardToken`,
  `TestDashboardDisabled` and `TestDashboardReadOnly` (the files' checks by hand, without a regex
  crate), with `/status` alone and with a writer, and `/config/effective`;
  `akit/tests/metrics.rs` ports `TestStatus` on the live server (two listeners, one with TLS; an
  upstream with a live and a dead backend, the dead one out for its failures; the certificate;
  the requests by class), and the listener view following a reload; `akit/tests/admin.rs` checks
  the pages and `/status` and `/config/effective` through `akit run`; `akit-config` reads
  `disable_dashboard`; `akit-tls` writes IP names as Go does. Checked in a browser against
  `akit run`: every tab shows its data, no console errors.
- Compatibility gate: `admin.disable_dashboard` supported (249 fields).
- Checked: `make check` on macOS; on gpu51 under the bench lock, the touched crates' tests
  (akit-tls, akit-config, akit-listener, akit-admin, akit) on epoll and io_uring, all passing
  but I22's `reloads_under_traffic_with_no_failed_request` (`akit/tests/reload.rs`), which failed
  once on io_uring, as it does on main, and passed five times out of five alone.
- Differences from fit: `/status` has no `node` capabilities: akit has no `GET /node` (fit's
  `TestStatus` checks `/node` lists `status.requests`). Times are written with milliseconds.
  `/config/effective` has one source until I57. The dashboard is the control plane: no benchmark.

### 2026-11-24: I57, the provider framework and the effective config

- `akit-provider` has fit's merging (`provider.Parse`, `Merge`, `Merged.Errors`; fit's decision
  009): a provider's part is a JSON object of `listeners`, `routes`, `upstreams` and `services`
  arrays of objects and nothing else, at most 16 MiB (`MAX_PART_BYTES`), empty for nothing;
  comments and trailing commas are blanked out as in a file. `merge` splices the parts' elements,
  as written, after the base config's in each array, provider by provider in their order of
  precedence; a named element whose name the base config or an earlier provider took is left out
  and listed as shadowed, with who took it. The base config's text, indexes and positions are
  kept, and shared when nothing is added. `Merged::errors` maps the merged config's errors back:
  in the base config to its line and column, in a provider's element to its path and position in
  that part, with `(provided by <name>)`.
- fit runs providers on goroutines and applies their parts on another; akit has neither: a
  provider (the `Provider` trait: `start`, `stop`) watches on akio's clients, its logical pool or a
  thread of its own and sends parts through an `Update`, which never blocks. `Updates` keeps at
  most one part per provider, the latest replacing one still waiting, and wakes the control
  thread once per run of parts. `Kinds` is fit's kind registry, built from each provider crate's
  list as akit's module registry is.
- `akit-admin`'s `Configs` merges the providers' parts into every config it applies, the API's,
  the files', a rollback's or a reload's, so that all take one path; `add_provider` (fit's name
  rules: not a base source, not twice) and `provide` (a part merged into the running base config,
  applied as a version whose source is the provider, the base config and its ETag kept; the same
  part again is not applied; a refused part keeps the provider's last; an empty one takes its
  elements away). A version keeps its base source (`file`, `admin`, or after a rollback the source
  of the version rolled back to; a reload and the certificate files' reload keep it, as fit's
  `Reload`), the effective config, its sources and what was shadowed. `GET /config`, its pointers,
  ETags and rollbacks stay about the base config. `Version::source` and `Outcome::source` are now
  `Arc<str>`, since a provider's name is a source; `Configs::reapply` and `Configs::rollback` take
  the reload and rollback paths.
- `GET /config/effective` answers fit's `Effective`: the version, the base config's ETag, the
  precedence (the base config's source first), the effective config (compact, as Go writes it),
  the source of each member and array element (a pointer in the merged config, its source, a
  pointer into the provider's part), and what was shadowed; on the API's I/O thread, from two
  short locks. Until now that path read as a pointer into the config.
- `akit run` takes providers' parts on the control thread (`Control::Provide`, at most one per
  provider each pass, then its other work), and drops those waiting when it stops. No provider
  runs yet: the `providers` field and its kinds come with I58–I61, and stay not supported yet
  (no field added to the gate; `make compat-bless` and the inventory unchanged).
- Tests: `crates/akit-provider` ports `TestMerge`, `TestMergeIntoEmpty`, `TestParseRefused`,
  `TestErrors` (fit's lines and columns), `TestPosition`, `TestParseRefusedMore`, `TestErrorsOne`,
  `TestOffsetPastEnd`, `TestRun`, `TestRunCanceledWhileApplying` and `TestKinds`, with a part at,
  under and over the limit and providers on threads of their own sending as fast as they can;
  `crates/akit-admin/tests/providers.rs` ports `TestProviders` (with a reload and a dry run too)
  and `TestFileAndProviders` against served listeners. JSON syntax errors read as akit-config's,
  not `encoding/json`'s. Not ported: fit's count of reloads (akit counts them in `akit run`).
- No akio request (A123–A125 unused). Checked: `make check` on macOS; on gpu51 under the bench
  lock, `akit-provider`, `akit-admin` and `akit` on epoll and `AKIO_TEST_BACKEND=uring`.
- No benchmark (rule 24: the control plane).

### 2026-11-24: I68, Cache II: the disk tier, the cache lock, stale responses

- The cache lock (`crates/akit-cache/src/lock.rs`), as fit's `lock.go`: a GET that misses while
  another fetches the same key waits for that fetch, up to `lock_timeout` (5 s), and is answered
  from what it keeps; `disable_lock` lets every miss fetch. A waiter is suspended (akio's
  `Context::suspend`, ADR-109) and holds no thread; the fetch's end resumes it on the thread that
  ends it, as soon as the response is kept or known not to be (its head settled, its body kept,
  given up or cut). The timeout is one timer a fetch on akit's timer thread (A45's stopgap), set
  again for the waiters left. Bounds: 4,096 waiters a fetch (past it a request fetches for
  itself), a fetch older than twice `lock_timeout` taken as abandoned and replaced. A GET or HEAD
  whose body would stream comes whole, so that it can wait (akio cannot suspend a request whose
  body streams, ADR-109), within the listener's `max_body_bytes`.
- Stale responses: one with validators, or one `stale-if-error` allows, is fetched again by the
  cache itself while its request waits with any others: a request of the cache's own, the
  client's as the cache saw it without its range and preconditions, run through the handlers
  behind the cache with akio's `transport::dispatch_conn` to a sink that sees the response go by
  (a streamed one through the cache's body tap) and drops it. Its 304 freshens the response; its
  failure (no response, one cut short, a 500, 502, 503 or 504, streamed or not) is answered with
  the stale response within its `stale-if-error` window (`detail=stale-if-error`). A response
  within its `stale-while-revalidate` window is answered at once (`detail=stale-while-revalidate`)
  and fetched again so in the background, one fetch a key and at most 256 a zone. RFC 5861's
  directives win over the `stale_while_revalidate` and `stale_if_error` params; `must-revalidate`,
  `proxy-revalidate` and `s-maxage` rule both out. The three I67 tests that expected
  `fwd=stale; stored` now expect fit's answers from the cache's fetch.
- The disk tier (`crates/akit-cache/src/disk.rs`), as fit's `disk.go`: every response kept in
  memory is written to disk too (one file a response, named by the SHA-256 of its key and
  variant, written to `tmp` and renamed into place), and one memory no longer holds is read back
  and kept in memory again (`detail=disk`), while its request waits suspended; a read that fails
  or takes longer than 5 s, as from a disk that hangs, leaves the request to go on as a miss. An
  index in memory says what is on disk without touching it; it is read back from the directory
  when a zone opens it (files that are not whole or not the cache's removed, `tmp` cleared),
  while requests go on finding less. Writes and removals are ordered, so a write queued before
  its key was removed (an invalidation) does not bring it back. Bounds: `max_bytes` (1 GiB) and
  `max_entries` (100,000) by least recent use, 64 MiB of responses waiting to be written, 1 MiB
  of metadata a file. The file format is akit's own (a head, the key, times, fields and the
  variant's request fields, then the body); fit's files are not read.
- Every file operation runs on the disk pool, a bounded pool of akit's own (`akio_pool::Pool`, at
  least 16 threads and 4 a CPU, 4,096 jobs queued, a job that waited past the timeout expired)
  apart from akio's logical pool: never on an I/O thread (rules.md rule 1). A test counter of file
  operations made off the pool is 0 after every disk test. The `akio` facade does not re-export
  `Pool` at the pinned commit, so akit-cache depends on `akio-pool` directly: a stopgap recorded
  as akio API request **A117**.
- Zones: a zone takes the disk of the last cache compiled for it; one whose disk moved takes the
  new directory, read back; one whose config drops its disk has none; a directory another zone's
  disk uses is refused (`disk: the directory … is zone "a"'s`). The directory is made when the
  config is compiled.
- Config: `stale_while_revalidate`, `stale_if_error` (a zero is none) and `disk` (`path`
  required, `max_bytes` or `max_size`, `max_entries`) are read; nothing of `cache` is refused any
  more. Compatibility gate: 7 fields more; `make compat-bless`: `examples/cache` is valid (80
  fixtures: 31 valid, 48 refused only as not supported yet, 1 skipped, none failed).
- Tests: `crates/akit-cache/tests/cache.rs` ports fit's `TestStampede` (100 clients, one fetch;
  and through the proxy, streamed), `TestLockNotKept`, `TestLockTimeout`, `TestDisableLock`,
  `TestStaleWhileRevalidate`, `TestStaleIfError` (and through the proxy, whose 500 streams),
  `TestDiskRestart`, `TestDiskPromotion`, `TestDiskNeverBlocks` (the pool held as a hung disk:
  the request answered as a miss after the timeout while the I/O threads answer others),
  `TestDiskInvalidate`, `TestDiskZones`, a file gone from under its entry, and the params; unit
  tests port `TestParseFile`, `TestDiskAdd`, `TestDiskBounds` and `TestDiskBrokenFiles`.
- Not done: purging by key, tag and prefix (I69, with fit's `Cache-Tag`); fit's
  `TestWaiterGone` and `TestDiskReadClientGone` as such (a waiter whose client went is resumed and
  answered into nothing; a disk read whose client went is skipped, untested); the cache's own
  request carries what akit's listener records (TLS, the canonical path, the body limit) but not
  route parameters or other middleware's locals.
- Checked: `make check` on macOS; on gpu51 under the bench lock, clippy and the tests of
  `akit-config`, `akit-cache`, `akit-compat` and `akit` on epoll and `AKIO_TEST_BACKEND=uring`, all
  passing but `akit --test reload` once on epoll (a reload under traffic, no cache in it, seen
  failing under load before I52), which then passed three times out of three on each backend.
- No benchmark (rule 30): what should be measured is in the dev plan's benchmark row for the
  response cache.

### 2026-11-24: I34, on-demand TLS

- `akit-config` reads fit's `acme.on_demand` (`ask`, required, an http or https URL; `rate` 10,
  `interval` 1m, `max_names` 1000, `timeout` 10s, `idle_timeout` 168h, 0 the default) and
  `tls.on_demand`, with fit's checks and messages: `on_demand` needs `acme.on_demand`, a listener
  requiring client certificates needs HTTP-01 answered, and a `tls` with only `on_demand` is not
  empty (no names given by HTTPS by default; `"on_demand": false` still is).
- `akit-tls`, fit's `tlsmgr/ondemand.go` in a new `acme::on_demand` module: a listener whose
  `tls.on_demand` is set, for a name a client asks for that neither its files nor its `tls.acme`
  have, calls `Acme::on_demand` from its certificate resolver (akio's A2, ADR-098). A name with a
  certificate is answered in the call; any other leaves the handshake's `CertDone` waiting with
  the name and returns, so the I/O thread is never held. The ask URL is asked (`domain=<name>`
  added to its query) through akio-http's asynchronous client, on an engine of its own; its
  callback takes the name on, and the ACME workers obtain the certificate as they do a listed
  name's, answering every handshake waiting for it when they settle it. Handshakes for one name
  wait for one certificate and one ask.
- fit's bounds, all checked in the handshake's call, a handshake past one failing at once:
  refusals remembered for a minute (10,000 at most), at most 8 asks under way and 5 a second, 20
  at once (`MAX_ASKS`, `ASK_EVERY`, `ASK_BURST`), `rate` per `interval` and `max_names`; at most
  1,024 handshakes waiting for one name (`MAX_WAITERS`, beside akio's
  `max_pending_cert_resolutions`). The listener's `cert_resolver_timeout` is
  `acme.on_demand.timeout`, and its handshake timeout that plus 10s.
- A name whose first certificate fails is dropped (its waiting handshakes hear why), and a name
  no handshake has asked for in `idle_timeout` is dropped by the workers (recorded at most once a
  second per name, without a lock), its certificate left in the storage for the next handshake
  that is allowed it. A name a reload lists becomes the config's; a reload listing nothing keeps
  the names taken on. An on-demand listener answers TLS-ALPN-01 for any name.
- Tests: `akit-tls`'s unit tests port fit's `TestOnDemandRefused`, `RateLimited`, `MaxNames`,
  `Dropped`, `Names`, `Idle`, `IdleTimer` and `AskBounds` (an ask URL and CAs that hang or cut
  connections, in a few lines of std; a clock the tests move); `akit-listener/tests/on_demand.rs`
  ports `TestOnDemandAllowed` through real handshakes against Pebble (four at once, one ask, one
  certificate; while the ask is held, twenty handshakes and requests for a name from files go
  through at once, and a refused name fails) and checks the timeout and the rate through
  handshakes; `akit-config`'s tests and the gate's samples cover the fields.
- Compatibility gate: `acme.on_demand`, its six fields and `listeners[].tls.on_demand` in
  `tests/compat/supported.txt`; `examples/acme-on-demand` is valid (80 fixtures: 31 valid, 48
  refused only as not supported yet, 1 skipped, none failed).
- Checked: `make check` on macOS (with Pebble); on gpu51 under the bench lock, with epoll and
  with `AKIO_TEST_BACKEND=uring`, clippy and the tests of akit-config, akit-tls, akit-runtime,
  akit-listener, akit-compat and akit (Pebble required): all passing.
- No akio request (A120–A122 unused).
- To measure: nothing; certificates are chosen as before for every listener without
  `tls.on_demand` (rule 24: control plane).

### 2026-11-24: I53, many protocols on one port

- `akit-config` takes `tcp` beside HTTP in a listener's `protocols` (fit's `config.Sniffs`), no
  longer refused as not supported yet: in the clear (`http/1.1`, `h2c`) and with `tls` (`h2`,
  and `h3` on the port in UDP), with `proxy_protocol` too; routes serve its HTTP and a service
  takes its other connections. `socks`, the third protocol fit tells apart on a port, stays not
  supported yet: it is I55's (the forward proxy, `listeners[].socks`), and with it this
  iteration's "and SOCKS" part of its done-when.
- `akit-listener` serves such a listener with one akio TCP listener whose acceptor is an
  akio-mux `Mux` (`sniff`, fit's `Server.sniff`), behind the PROXY protocol layer when the
  listener has `proxy_protocol`. With `tls`: a complete ClientHello whose server name a service
  of the listener takes, in the current graph (`Rule::TlsHello`), goes to akit-l4's acceptor,
  which passes it through to that service; any other TLS is served HTTPS (h2 or HTTP/1.1 by
  ALPN); up to 64 KiB (`MAX_CLIENT_HELLO`) are read for the hello. In the clear: TLS goes to
  akit-l4's acceptor (the service of its name, else the one without `server_names`), HTTP/1.1
  (and HTTP/2's preface with `h2c`, `Rule::Http`) is served. Whatever else goes to the service
  without `server_names`, or is closed without one. A client that has not sent enough to tell
  within 10 s (`OPENING_TIMEOUT`, akio-mux's own timer) is closed. A `tcp` listener alone and an
  HTTP one alone are served as before, with no sniffing; the three now share one
  `tcp_listener`, at engine build and on a reload. A listener of `tcp` and `h3` at port 0 takes
  one port free for both, as one of HTTP and `h3` does. No akio request: akio-mux had all of it.
- Tests: `crates/akit-listener/tests/one_port.rs` ports fit's `TestSniffClear` (HTTP/1.1, h2c,
  TLS passed through by name and raw TCP on one port; without its SOCKS) and
  `TestTLSTerminatedWithoutNames` (HTTPS by ALPN beside the service), and adds TLS passed through
  beside HTTPS with and without a fallback service, HTTP/2's preface going to the service without
  `h2c`, a client held while its first byte could still be several protocols, PROXY headers
  before the sniffed bytes, `tcp` with `h3` on one port, and a reload adding `tcp` to an HTTP
  listener. `crates/akit-config/tests/services.rs` checks the listeners fit's "mixed" one stands
  for. Not tested: a silent client closed after the 10 s sniff timeout (akio-mux's own tests
  cover the timeout; fit has none).
- Compatibility gate: no field more (`protocols` was supported); `make compat-bless` changes no
  fixture's outcome.
- Checked: `make check` on macOS; on gpu51 under the bench lock, `akit-config`, `akit-l4` and
  `akit-listener` (tests on epoll and `AKIO_TEST_BACKEND=uring`).
- To measure, in the maintainer's benchmark tasks (rule 30): the cost of sniffing on a shared
  port, as the dev plan's benchmark row for I51–I53 says: connections a second and HTTP/1.1 and
  h2c request rates on a listener of `http/1.1`, `h2c` and `tcp` against one of HTTP alone, and
  TLS passthrough by name on it against a `tcp` listener alone.

### 2026-11-24: I52, SNI routing and PROXY protocol

- `akit-config` reads fit's `services[].server_names`, `services[].proxy_protocol` (`v1`, `v2`)
  and `listeners[].proxy_protocol.trusted` (addresses and CIDR ranges), with fit's checks and
  messages: a server name a host name or one under `*.`, not an IP address, not listed twice, not
  another service's on the same listener; neither field on a service with a `udp` listener;
  `trusted` required and not empty; no `proxy_protocol` on a listener that takes UDP alone. A
  `**.` wildcard is refused (fit's would never match). `access` stays not supported yet.
- `akit-l4`: an `Entry` per `tcp` or `udp` listener in the graph (`Graph::entry`, replacing
  `Graph::service`): its services by exact name and by `*.` wildcard (one label), without case or
  a trailing dot, and the one without `server_names` as the fallback. When an entry routes by
  name, a TCP connection's ClientHello is read before a service takes it, with akio-mux's parser
  (A5's `tls::parse_client_hello`; a hello may span reads and records), at most 64 KiB
  (`MAX_CLIENT_HELLO`) within 10 s (`OPENING_TIMEOUT`, on akit's timer thread: a stopgap for
  A45); the bytes read go to the upstream first, untouched. A name no service takes, a client
  that is not TLS, a malformed or too long hello go to the fallback, or are closed without one.
- PROXY protocol, on akio-mux's `ProxyProtocol` (A4): `akit-listener` puts it in front of a `tcp`
  listener or one of HTTP over TCP (before TLS) with `proxy_protocol`, trusting its `trusted`
  addresses: a connection from one must begin with a v1 or v2 header within 10 s and 4,096 bytes
  (`MAX_PROXY_HEADER`), or is closed; one from elsewhere is served as it is. A relay's client is
  then the header's (`ip_hash` picks by it), and a service with `proxy_protocol` sends each
  upstream connection a header naming that client and the address it connected to (the header's
  destination, or the listener's), IPv4-mapped addresses unmapped, before anything else. A kept
  listener whose `proxy_protocol` changes is re-bound by a reload (`same`).
- HTTP listeners: the request's client is the header's (`X-Forwarded-For`, access logs,
  `remote_ip`, rate limits, `ip_hash`), which needed akio: **A105** (ADR-118), built on akio
  branch `claude/a105-proxy-peer` (`e3f03a32`, from akio main `c5cf4353`): akio-http's
  `Request::peer` is `akio_mux::proxy::client_addr`. akit pins that commit; it moves to akio's
  main once the branch is merged there.
- Tests: `crates/akit-listener/tests/sni_proxy.rs` ports fit's `TestSNIRouting` (a TLS backend
  that checks the ClientHello came untouched and completes the handshake with its own
  certificate), `TestSNIHalfHello` (and a hello trickled a byte at a time),
  `TestProxyProtocolToUpstream`, `TestProxyProtocolFromClient`, `TestProxyHeaderThenHello` and
  the shutdown half of `TestProxyProtocolHTTPEdges`, with a reload that renames a service, and
  truncated and forged headers (bad families, ports, lengths past the limit, v2 version 3, a bad
  command, a header cut short by the client) closed with no upstream dialed;
  `crates/akit-proxy/tests/proxy_protocol.rs` ports `TestProxyProtocolHTTP` and the rest of
  `TestProxyProtocolHTTPEdges` (header split across writes, `UNKNOWN`, a bad or missing header);
  `crates/akit-l4/tests/entry.rs` ports `TestEntryPick`; `crates/akit-config/tests/services.rs`
  the fields and checks. fit's parser tests (`TestReadSNI`, `TestParseProxy`,
  `TestProxyRoundTrip`) are akio-mux's own (ADR-095). `TestTLSTerminatedWithoutNames` is I53's.
  Not tested: a hello that never completes closed after the 10 s opening timeout (as in fit).
- Compatibility gate: 4 fields more; `make compat-bless`: `examples/l4-tcp` and
  `examples/proxy-protocol` are valid (80 fixtures: 27 valid, 52 refused only as not supported
  yet, 1 skipped, none failed).
- Checked: `make check` on macOS; on gpu51 under the bench lock, the akio branch's akio-http and
  akio-mux (clippy, tests on epoll and `AKIO_TEST_BACKEND=uring`), and akit's workspace (clippy,
  tests on both backends), `sni_proxy` five times on each: all passing but three tests that
  failed once under the whole suite's load and passed five times out of five alone
  (`akit --test reload` on epoll; `akit-admin --test admin` and `akit-proxy --test h2
  host_header` on io_uring, an engine that did not start, seen before I52). akio's public API
  check could not run: gpu51 lacks `cargo-public-api` (akio-http's public items are unchanged).
- To measure, in the maintainer's benchmark tasks (rule 30): see the dev plan's benchmark row for
  I51–I53.

### 2026-11-24: I67, Cache I: the memory tier and RFC 9111

- `akit-cache` has fit's `cache` middleware (`http.middleware.cache`, registered in `akit`): GET
  responses a shared cache may keep, by RFC 9111's rules as fit's `semantics.go` has them
  (`Cache-Control` of requests and responses, `Expires`, heuristic lifetimes from
  `Last-Modified` or `default_ttl`, the storable statuses, `Vary` with up to 8 variants a key,
  `Set-Cookie`, trailers and `Vary: *` not kept, `Authorization` kept only with `public`,
  `s-maxage` or `must-revalidate`), answered with `Age` and fit's `Cache-Status`; HEAD answered
  from them; `If-None-Match` and `If-Modified-Since` answered 304 from them; the request's
  `no-cache`, `max-age`, `min-fresh`, `max-stale`, `only-if-cached` (504) and `Pragma: no-cache`;
  a `Range` request bypasses; POST, PUT, DELETE and the like drop the URI and its same-host
  `Location` and `Content-Location` on success.
- A stale response with validators is sent on with them (through the `proxy`, which forwards the
  changed fields, I40's local); the upstream's 304 freshens it (fields merged as RFC 9111 section
  3.2 says) and the client is answered from it, as a 304 if its own validators match.
- The memory tier: a zone's store, 16 shards each with its lock, its share of `max_bytes` and its
  least recently used order (an index-linked list, no unsafe); bodies, fields and keys counted;
  zones shared by caches of the same name and kept across reloads (held weakly, so a zone no
  cache uses goes).
- A response is kept as it goes, never held back: a whole one from its body; a streamed one
  (proxied past 64 KiB, I16) copied write by write by a body tap and stored only when its stream
  finished whole (not cut, not ended with trailers, as long as it declared). The cache's response
  filter is an outer one, so in front of `compress` each encoding is a variant of its own. What
  captures hold at once is bounded by a quarter of `max_bytes`.
- A route with an auth middleware (`basic_auth`, `client_cert`, `forward_auth`, `jwt`) has its
  cache treat every request as carrying `Authorization`, worked out when the config is compiled:
  fit marks each request an auth middleware let in, which comes to the same for those
  middlewares at no cost per request.
- Config: `zone`, `max_bytes` (`max_size`), `max_entry_bytes` (`max_entry_size`, at most a
  sixteenth of `max_bytes`), `default_ttl`; `disable_lock` and `lock_timeout` read and kept for
  I68's lock (until then every miss fetches); `stale_while_revalidate`, `stale_if_error` and
  `disk` refused as not supported yet (I68). The compatibility gate's `examples/cache` is now
  refused only for the stale windows.
- akio: requested and built as **A111** (`Response::tap_body`, `BodyTap`, `TapEnd`), **A112**
  (`akio_http::parse_http_date`, RFC 9110's three formats) and **A113**
  (`Context::add_outer_response_filter`), ADR-116 on akio's branch `claude/a111-body-tap`, commit
  `0e36f8d6` (from akio main `c5cf4353`), not pushed. akit pins that commit; it builds once the
  commit is on akio's main (tested through a local, uncommitted `[patch]`).
- Tests: `crates/akit-cache/tests/cache.rs` ports fit's `cache_test.go` (hit, storable, freshness,
  default_ttl, vary, conditional, revalidate, request directives, invalidate, bodies, client gone,
  compressed, params, zones) and `auth_test.go`, over HTTP/1.1 and h2c, with the proxy's bodies
  (streamed and 1 MiB kept, bodies cut short not kept) and revalidation through the proxy; unit
  tests of the semantics, the store (eviction, variants, zones) and same-origin locations.
  Where fit's answer comes from its cache lock (a stale response refetched while the request
  waits), the tests expect I67's `fwd=stale; stored` and say so.
- Not done: the cache lock, stale-while-revalidate, stale-if-error and the disk tier (I68);
  purging (I69); a body akio sends from a file on disk is not kept; fit's mark for a handler that
  authenticates a request itself (no akit handler does).
- No benchmark (rule 30): what should be measured is in the dev plan's benchmark table.

### 2026-11-24: I48, the rest of fit's middlewares

- Besides `cache` (I67) and `wasm` (I71), what fit's middleware had that akit lacked was
  `rewrite`'s `regexp` and `replacement` and the groups of `redirect`'s regexp in `to`, waiting on
  akio API request A32. I48 built A32 in akio (ADR-115, branch `claude/a32-regex-captures`,
  `26c500b5`, on `c5cf4353`): akio-regex's `find`, `find_iter`, `captures`, `expand` and
  `replace_all`, with the leftmost-first matches and groups of Go's `regexp` (repetitions compiled
  in Go's shapes, so a group in a repetition that can match nothing holds what Go's does) and Go's
  template rules, on a capturing Pike VM in linear time; 132 000 random cases checked against Go's
  `regexp`. akit is pinned to that commit (`Cargo.toml`, `AKIO_REV`).
- `akit-config` reads `rewrite`'s `regexp` (compiled as the router's, Go's ASCII classes) and
  `replacement` ("a replacement needs a regexp" without one, as fit's `Provision`), and takes
  groups in `redirect`'s `to`; the not-supported-yet refusals and `REWRITE_FIT_ONLY` are gone.
- `akit-middleware` `rewrite`: strip, replace, add, in fit's order; each match in the decoded path
  replaced (Go's `ReplaceAllString`), a leading `/` put back, the path escaped afresh. A decoded
  path that is not UTF-8 keeps its bytes: each byte of an invalid sequence stands for a private-use
  character while the regexp runs, as Go reads it as one character. `redirect`: with a `$` in `to`,
  the groups of the match on the escaped path are put in first, then the placeholders, as fit's
  `Expand` then `expand`; open redirects made by a group are answered 400 as before.
- fit's `TestRewrite`, `TestRedirect` and `TestParamErrors` cases for regexps ported (two of
  `TestRedirect`'s paths are made canonical or refused by akit-listener before the middleware, as
  the existing cases note); unit tests for the path rewrite, empty matches and non-UTF-8 bytes.
- Compatibility gate: `rewrite.regexp` and `rewrite.replacement` supported; fit's `redirect` and
  `rewrite` examples are valid (27 valid, 52 refused only as not supported yet). No middleware but
  `cache` and `wasm` is refused.
- Not measured (rule 30): the benchmark row "Rewrite and redirect regexps" in the dev plan.

### 2026-11-24: I54, upstream TLS

- `akit-config` reads fit's `tls_server_name`, `tls_ca_file`, `tls_cert_file` and `tls_key_file`
  on upstreams (no longer refused as not supported yet), with fit's checks and positions: each but
  the key needs `"protocol": "h2"`, and a certificate needs its key and a key its certificate.
- `akit-tls` `UpstreamTls::load`: the CA file's certificates in place of the system's roots, the
  client certificate and its key (the system's roots kept when no CA file is given), ALPN `h2` and
  `http/1.1` as fit's, read when the config is compiled (files capped as a listener's are). A file
  that does not load fails the config at `upstreams[i].tls_ca_file` or, for anything about the
  certificate or its key, `upstreams[i].tls_cert_file`, as fit's `upstream.New` reports it; a key
  that is not the certificate's is caught when akio builds the configuration (A109).
- The graph (`akit-runtime`) makes an h2 pool with that configuration (`Pool::new_with_tls`, so
  that probes started with the pool use it too: `with_tls` cannot reach a pool already shared with
  its prober) and adds the files to the graph's `cert_files`: `run -watch-certs` watches them as a
  listener's, and a change applies the config again, whose new pool reads them again (fit's
  `upstreamKey`; akit's pools are not shared between graphs, so any change makes new ones).
- `akit-upstream`: `tls_server_name` is the name sent in SNI and checked against the certificates
  over HTTP/2 (`H2Client::connect`'s name) and over HTTP/1.1, for an address whose server chooses
  it in ALPN (akio's `Client::with_tls_server_name`, A108), the probes of both included.
- akio: A108 (`Client::with_tls_server_name`) and A109 (`ClientConfigBuilder::build` checks a
  client certificate's key against its leaf), ADR-117, on akio branch
  `claude/a108-a109-upstream-tls` (`252fba42`, from `c5cf4353`, not pushed); akit is pinned to it
  until it lands on akio's main.
- Tests: fit's `TestValidate` "upstream tls without h2" with its positions
  (`akit-config/tests/upstreams.rs`); fit's `TestTLSCAFileErrors` and `TestTLSCertFileErrors`
  (`akit-tls/tests/upstream.rs`); fit's `TestCompileFails` for `tls_ca_file` and the watched files
  of `TestSharedPools` (`akit-runtime/tests/upstream_tls.rs`); fit's `TestClientCertificate` and
  `TestProbeProtocols` over h2 with `tls_ca_file` and `tls_server_name`, a wrong or missing server
  name, and the HTTP/1.1 fallback keeping the name and the certificate, probes included
  (`akit-proxy/tests/upstream_tls.rs`).
- Compatibility: the four fields in `tests/compat/supported.txt` (225 of 3821 fields supported);
  no fixture changed.
- No benchmark (rule 30): TLS to upstreams is set up per connection, off the request path; nothing
  new to measure beyond the h2 upstream rows already in the dev plan.

### 2026-11-24: I50, gRPC proxying

- Taken early from M3, on I18 and I18b's HTTP/2 upstreams; no akio request was needed (A102–A104,
  reserved for it, stay unused).
- `akit-proxy`'s `grpc` module, after fit's `handler/proxy/grpc.go`: a gRPC call is an HTTP/2 or
  HTTP/3 `POST` with one `Content-Type`, `application/grpc` or a subtype of it (`+proto`, `;...`;
  not `application/grpc-web`), and goes through the proxy's ordinary relay: messages stream both
  ways as they come with linked backpressure, the client's reads held while 64 KiB waits for the
  upstream and the upstream's stream window closed while 64 KiB waits for the client, and the
  upstream's trailers follow its response. Its body streams to the proxy from its head, whatever
  length it declares (fit's `StreamRequestBodyAtHeader`), so a client that waits for the answer
  to its first message is answered. A call needs an `h2` or `h2c` upstream, as in fit.
- What the proxy adds: `TE: trailers` upstream; `grpc-timeout` (eight digits and a unit) bounds
  the call as the proxy's `timeout` does, the shorter of the two holding, and when the route's is
  the shorter the upstream is told it as the call's `grpc-timeout`, rounded up (fit's
  `callTimeout`, `encodeGRPCTimeout`). The response is never collected to be sent whole (fit's
  `blockResponse` off): a trailers-only response (`grpc-status` in its head) is passed on as one,
  a single HEADERS frame, and any other streams however short.
- Failures as gRPC answers them (fit's `grpcFail`, `grpcBroken`): a call that got no response is
  answered with a trailers-only `200` in its own content type, with `grpc-status` and
  `grpc-message` from the HTTP status the proxy would have sent: 504 and 408 are
  `DEADLINE_EXCEEDED`, 413 `RESOURCE_EXHAUSTED`, 400 `INTERNAL`, the rest (502, 503)
  `UNAVAILABLE`. A response cut short ends with `UNAVAILABLE` trailers, or `DEADLINE_EXCEEDED`
  when its time ran out, after the messages that came. The messages are fit's with `akit:` for
  `fit:`.
- A request cut short by the proxy's own deadline (its `timeout`, or a call's `grpc-timeout`),
  gRPC or not, no longer counts against its address's passive health (`Exchange::gave_up`), as
  fit's `timedOut` keeps it in rotation.
- `akit_runtime::Env::with_upstreams` is public, for a module provisioned on pools made by hand
  (the `h2` tests' pool given the test CA until I54's `tls_*` fields).
- Config: no new fields (`examples/grpc` was already valid); `supported.txt` notes I50.
- Tests (fit's `grpc_test.go`, ported in `crates/akit-proxy/tests/grpc.rs`, on akio-grpc's client
  and server where fit's run on fib's): unary calls (an echo, 200 KB, the upstream's status and
  trailers, the deadline carried upstream), 2000 messages down, 500 up, 50 answered one by one
  with 300 KB among them, 32 MB each way to a client and an upstream that read nothing, no
  upstream, an upstream gone midway, calls cut by `grpc-timeout` and the route's timeout with the
  upstream kept in rotation, each over h2c and h2 with TLS; and a deadline mid-response, the
  fields that go upstream, a trailers-only answer passed on, and the helpers (fit's
  `TestGRPCHelpers`).
- Not done: a run against grpc-go (the plan's "done when"): fit's tests run here on akio-grpc, and
  grpc-go needs a Go toolchain the CI does not have (dev plan, I50's Split line).
- No benchmark (rule 30): gRPC calls a second and messages a second on a stream, against fit, are
  in the dev plan's benchmark table.

### 2026-11-24: I46, weighted upstreams, mirroring, failover

- `akit-config` reads the `proxy` handler's `upstreams` (`Split`: `upstream`, `weight` from 1 to
  1000, both required), `failover` and `mirror` (`Mirror`: `upstream`, required, and `percent`
  from 0 to 100, 100 when left out), with fit's checks and messages: `upstream` and `upstreams`
  are each other's alternatives (set one), and one of them is needed. They leave
  `PROXY_FIT_ONLY`, which keeps `splice_uploads` alone. `MAX_MIRRORED` is fit's `maxMirrored`, 256.
- `akit-upstream` `Split` is fit's `pickPool`: one upstream, or several drawn at random by weight
  (the balancer's per-thread wyrand, Lemire's reduction), then the failover instead when none of
  the chosen upstream's addresses is in rotation (passive health or probes, `Pool::available`)
  and one of the failover's is. A pick neither allocates nor locks (`tests/no_alloc.rs`); a
  proxy with one upstream and no failover draws nothing. `split::chance` draws a mirror's
  percent.
- `akit-proxy`: the handler holds a `Split` in place of its pool, and each request picks its
  upstream first; the address, the sticky cookie, retries (I45, which stay within the chosen
  upstream), WebSockets and the access log's address all follow the upstream picked. Each upstream
  name is looked up when the handler is provisioned, the error at its field
  (`upstreams[1].upstream`, `failover`, `mirror.upstream`), as fit's `provisionSplit`.
- `mirror.rs`: a request whose body came whole (none that streams, which is read once), chosen by
  `percent`, gets a copy sent to the mirror before the request goes up, as fit's `copyTo`: its
  method, target and outgoing fields (its `Trailer` dropped: the copy goes by its length), a copy
  of its body (64 KiB at most), no cancel token of the client's, the mirror upstream's own
  timeouts. The copy's response is read and dropped by a pool exchange that holds a slot of an
  atomic count: past 256 copies under way, requests are not copied, so a slow mirror costs at most
  256 connections or waiting requests and 16 MiB of bodies, and never slows the requests.
- Tests: `akit-config` `tests/proxy.rs` `split` (fit's `TestSplitParams`, the decoder's part);
  `akit-upstream` `tests/split.rs` (the shares by weight, the failover, the failover after the
  weights, the percent); `akit-proxy`'s unit tests of the slots' bound and the copy, and
  `tests/split.rs` through akit's listener: fit's `TestSplitWeights`, `TestSplitFailover`,
  `TestMirror`, `TestMirrorDoesNotSlow` (a mirror that answers none of its copies: 400 requests
  answered all the same, the copies stopping at the bound), `TestMirrorPercent` and
  `TestSplitParams` (the names); a failover standing in for one upstream of a split, a streamed
  body not copied, and a mirror that cannot be reached.
- Compatibility gate: 7 fields more in `tests/compat/supported.txt` (and the gate's samples, its
  `SPLIT_FIELDS` and `MIRROR_FIELDS`); `make compat-bless`: `examples/load-balancing` is valid (80
  fixtures: 26 valid, 53 refused only as not supported yet, 1 skipped, none failed). The garbled
  middleware list and gate counts of the status table's "What works" fixed in passing.
- No akio API request: the pools' asynchronous exchanges, `available` and the balancer's
  generator were enough. Not benchmarked (rule 30); the dev plan's benchmark table has a row for
  it.

### 2026-11-24: I44, JWT and JWKS

- `akit-config` reads fit's `jwt` (`jwks_url`, `jwks_refresh_interval` and its old name
  `jwks_refresh`, `public_key_files`, `hmac_secret_file`, `algorithms`, `issuer`, `audience`,
  `leeway`, `token_from`, `claims_headers`) into `Handler::Jwt`, with the checks of fit's
  `Provision` that need no file and its messages: a key is needed, `jwks_url` an http or https URL
  with a host, `algorithms` one of fit's thirteen (`JWT_ALGORITHMS`), `token_from` `header`,
  `header:Name`, `cookie:name` or `query:name`, `claims_headers` fields HTTP tokens naming a claim;
  `jwks_refresh_interval` 1h when left out or 0. Both names of the refresh interval given is an
  error, as in fit (akit has no warnings, so the old name is taken silently). It leaves
  `FIT_MIDDLEWARE`.
- `akit-middleware` `jws`: tokens taken apart as fit's `parseToken` (16 KiB at most, three parts,
  base64url, a JSON header whose `alg` and `kid` are strings, no `crit`) and verified with
  akio-crypto: RS256/384/512 and PS256/384/512 (`rsa::PublicKey`, PSS salt the hash's length),
  ES256/384/512 (P-256, P-384, P-521, the fixed `r || s` form), EdDSA (Ed25519) and HS256/384/512
  (a whole tag compared in constant time). A key fits only its kind and size, a JWK with `alg` only
  that algorithm, an HMAC key only the config's secret; `none` never. Keys from PEM files (`PUBLIC
  KEY`, `RSA PUBLIC KEY`, `CERTIFICATE`; others skipped) and an HMAC secret file of at least 32
  bytes, read when the middleware is provisioned (1 MiB each at most); JWK sets parsed as fit's
  `parseJWKS` (keys for encryption, of other types, for HMAC or that do not parse left out; RSA
  below 2048 bits and EC points off their curve refused).
- `akit-middleware` `jwt`: the token from `Authorization: Bearer`, a field, a cookie or a query
  parameter, as fit finds it; `exp` required and `nbf` optional, within `leeway`, `iss` and `aud`
  checked as fit's `check`; 401 with `Bearer realm="fit"` (and `error="invalid_token"` when a token
  was sent). A verified token is kept in a cache of the I/O thread's own, by its SHA-256 (1024 per
  thread), valid while the keys keep their generation (one atomic load per request), its
  `exp`/`nbf` checked on every use, and the values of its `claims_headers` fields kept with it: a
  token used again costs a hash and a lookup. `claims_headers` sets fields to claims (a string as
  it is, anything else as Go's `json.Marshal` writes it, made safe for a field), after removing
  every spelling of them the client sent.
- `akit-middleware` `jwks`: the JWK set fetched with akio's client on akit's engine (the engine
  slot's `when_set`, now public in akit-upstream) when it is set, every
  `jwks_refresh_interval`, and when a token names a kid the set lacks, at most every 10s (fit's
  `minRefetch`); 1 MiB, a 200 only, 10s, ten redirects at most. Parsed in the client's callback;
  the refresh and delayed fetches are jobs on akit's timer thread. A failed fetch keeps the keys.
  A request whose token names an unknown kid, or any before the first fetch, waits for the fetch
  suspended (`Context::suspend`, A65), at most 1024 at once (past that, 401), and is verified again
  on the thread the fetch ends on; one checked against a set replaced since is verified at once.
  When the config is released, a fetch under way is cancelled and the requests waiting are
  verified with the keys there are.
- A request whose body would stream to its handler cannot be suspended (ADR-109): `jwt`'s
  `body_mode` asks for it whole only when the request may wait for the set (before the first fetch,
  or a kid the set lacks), within the listener's `max_body_bytes`; other requests stream. A75
  would let them stream too.
- Tests: unit tests of the key set's lookup, tokens taken apart and claims refused, algorithms and
  keys that fit them, Go's JSON writing, NumericDates, JWK keys and sets, the claims and the clock,
  where tokens are found, the cache (other keys, another middleware, the bound), the key files'
  errors, waiting against a stale set, the waiters' bound and a failed fetch keeping the keys.
  `tests/jwt.rs` through akit's listener: fit's `TestJWTAlgorithms`, `TestJWTClaims` (and a kept
  token expiring), `TestJWTTokenFrom`, `TestJWTClaimsHeaders`, `TestJWTParams` and `TestJWTMore`'s
  params, `TestJWKSRotation`, `TestJWKSRefresh` (with `TestJWKSGet`'s failures: bad JSON, 500, too
  large, cut), `TestJWTClientGoneWaiting`; a JWK set out of reach and back; bodies of 200 KiB
  through `jwt` to the `proxy` handler, one waiting for the set. fit's RSA keys are generated per
  run; akio-crypto makes none, so two 2048-bit test keys are in `tests/data`.
- Compatibility gate: 13 fields more in `tests/compat/supported.txt`; `make compat-bless`:
  `examples/jwt` is valid, `examples/whip-whep` no longer refused for `jwt` (80 fixtures: 23 valid,
  56 refused only as not supported yet, 1 skipped, none failed). The garbled gate counts of the
  status table's "What works" fixed in passing.
- Differs from fit: claims are not passed to later middleware nor the user to the access log
  (A29), as for I42; a token's JSON may hold what akit-config's reader allows (comments, trailing
  commas) and not a key twice; RSA keys below 2048 bits from PEM files are refused (akio-crypto's
  bound); the cache is per I/O thread rather than 16 shared shards; a request that may wait has its
  streamed body read whole first (A75).
- To measure, in the maintainer's benchmark tasks (rule 30): see the dev plan's benchmark row for
  I44.

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
