# 034: Access lists on akio's accept filter

[English](034-access-lists.md) | [简体中文](034-access-lists.zh-CN.md)

Status: accepted in I85 (2026-11-26); extended by I86 (layer 4 and SOCKS) and I87 (the Admin
API); points 6 and 9 replaced on 2026-11-27, once akio's UDP listeners took the filter (A220,
akio's ADR-178).

## Context

fit's iteration I195 gave fit allow lists (whitelists) and deny lists (blacklists) of client
addresses: a global `access`, and one on each listener, route, service, role, the Admin API and the
consoles, the most specific that has a say deciding. fit refuses a client at accept through fib's
`Binding.Filter` where the policy is known then, and later (a request answered 403, a connection
closed once its service is known) where it is not. fit's own decisions on I195 (in its development
plan, "decisions pending review") say how the lists combine, which address counts, when a change
applies, how list files are read, and where each module decides. akio has the accept filter fit's
`Binding.Filter` is (akit's request A3, akio's ADR-099), for TCP listeners only.

## Decision

1. **fit's semantics, unchanged.** Each list in turn may decide: its `deny` refuses, else its
   `allow` serves, else an `allow` with entries refuses; otherwise the next one out decides (route,
   then listener, then global). An address no list has a say on is served. The address is the
   connection's, or the one a trusted PROXY header names, never `X-Forwarded-For`'s. A change
   applies to what starts after it. List files are fit's: addresses and prefixes, ranges,
   `ipfilter.dat` and `.p2p` lines told apart line by line, at most 64 MiB a file and 16 KiB a
   line, a file that cannot be read failing the config.
2. **A crate of its own, `akit-access`**, fit's package `access`: a list is compiled into sorted,
   merged ranges of each family (`u32` and `u128`), looked up by binary search, with no allocation
   or lock on the request path; a policy is the lists that apply, most specific first. It depends on
   akit-config, akit-router (fit's `ParsePrefix`) and akio-crypto (the files' SHA-256): no new
   dependency.
3. **Where each listener decides is compiled into the graph** (`akit_runtime::access`, fit's
   `compileAccess`): at accept when the listener's own list and the global one are all there is and
   no PROXY header names the client; per request otherwise.
4. **At accept, akio's filter.** A listener's TCP side is built with `Listener::filter` holding its
   policy (an `Arc`, read with no lock); a reload that keeps the listener swaps it with
   `Engine::set_listener_filter` (or clears it) once the new graph is current, without binding
   anew, so a listener's identity (`same`) does not include `access`. Connections accepted before
   the swap are not checked again, nor the later requests of an HTTP/2 connection, as fit's.
5. **Per request, the dispatcher.** A refused request is answered fit's `403 forbidden` (plain
   text), logged and counted with its route, before its handler runs; its body is not buffered
   (its body mode is `Stream`, nothing reads it). A request whose path has no canonical form is
   refused before it is answered 400, as fit's.
6. **HTTP/3 at akio's filter too.** A listener's UDP socket for HTTP/3 takes the policy its TCP
   side refuses by at accept (`Listener::filter` on a UDP listener, `Engine::bind_udp_filtered`
   for one a reload binds, swapped by a reload that keeps it): a refused peer's datagrams are
   dropped before a QUIC connection, or any TLS work, is made, as fit drops them. Where a route has
   a list of its own, the peer is served and its requests are decided one by one (403). Until
   akio's A220 (2026-11-27), HTTP/3 was checked per request.
7. **List files are watched with the certificate files.** Their SHA-256 sums join the graph's
   `cert_files`, which `akit run -watch-certs` compares the files with: a change applies the config
   again, as fit's `watchFiles`.

8. **Layer 4, once the service is known (I86).** A `tcp` listener of one service without
   `server_names`, on a port of its own and without PROXY headers, refuses at accept by that
   service's policy (its own list, the listener's, the global one), as fit's. Any other listener
   with a service's list of its own, or with PROXY headers, has akit-l4's entry check each
   connection once it knows the service (after the PROXY header and the ClientHello, as the
   listener needs them), with the policies compiled into the entry with the graph: a refused one
   is closed before its upstream is dialed.
9. **`udp` listeners at akio's filter.** A `udp` listener's socket takes the policy of the
   service its sessions go to (the one without server names: its own list, the listener's, the
   global one): a refused client's datagrams are dropped before a session is made, and nothing is
   remembered of the peer, so nothing grows with the clients refused (fit's decision 7 on I195).
   Until akio's A220 (2026-11-27), each datagram of a refused client opened and closed a session.
10. **SOCKS at the client's request.** A listener's `socks` rules are fixed for as long as it is
    bound; its access is not: the server asks the graph current at each request for the policy
    the listener's SOCKS clients go by when the listener decides later, fit's `SOCKSAccess`.
11. **The Admin API at accept, following every config applied (I87).** Its policy,
    `admin.access` then the global access, is compiled with each graph (its files watched with the
    rest), set on the API's listener when akit starts and again, with
    `Engine::set_listener_filter`, after each config is applied, though the rest of `admin` takes a
    restart (and a change to `access` alone is not warned of as one). The dashboard is the API's
    and goes by it. An API on a Unix socket is never checked, and `admin.access` with one is an
    error, as fit's. The roles' consoles (`admin.console.access`, the roles' `console.access`) come
    with the Console track.

## Consequences

- A refused TCP client costs an `accept` and a close, with no TLS, no read and no request.
- A refused HTTP/3 client costs a lookup per datagram, no QUIC handshake (one per request where a
  route has its own list).
- A listener that decides per request costs every request a graph read and a lookup, as fit's.
- A refused `udp` client costs a lookup per datagram.
