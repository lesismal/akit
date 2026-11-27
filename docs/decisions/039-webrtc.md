# 039: WebRTC: fit's three roles on akio-turn and akio-webrtc, akit's names, the RFCs' wire

[English](039-webrtc.md) | [简体中文](039-webrtc.zh-CN.md)

Status: accepted in I110 (2026-11-27), waiting for review ([decisions waiting for
review](../decisions-pending.md), rows 50–54); extended by the iterations of the WebRTC track
(I110–I114) as they land.

## Context

fit's I87 and I108–I111 added WebRTC to fit's config
([fit's decision 029](https://github.com/lesismal/fit/blob/main/docs/decisions/029-webrtc.md)):
`webrtc.turn`, a TURN server on fib's `webrtc/turn/turnserver`; `webrtc.media`, an SFU whose
PeerConnections share one UDP port (fib's `webrtc.Mux`), which the handlers `whip` and `whep`
publish to and play from; `webrtc.proxy`, the UDP port of the handler `webrtc_proxy`, which puts
other media servers behind fit and relays each peer to the one that answered it, found by the ICE
ufrag of the peer's first STUN request. akio has the same layers: `akio-turn`'s `TurnServer` (UDP,
TCP, TLS, DTLS) and `akio-webrtc`'s ICE lite `RtcServer` with every peer on one port.

The handlers' `auth` names a policy of `media_auth`, which the Streaming and media auth track
reads. fit's TURN role leans on hooks fit asked of fib (its A55–A59): a relay port range, a
username's bandwidth counted per datagram, the server's events, limits changed in place, a drain,
a nonce secret shared among servers, stream bounds, and a port-aware check of the server's own
addresses. akio-turn has none of them at `8a9601b9` (nor at `a8171fda`).

## Decision

1. **The roles are read as fit's** (I110): the three members with fit's fields, defaults, bounds
   and checks, in `akit-config`'s `webrtc` module, the handlers' params with them, the gate's three
   examples valid. `akit run` refuses a member or a handler until the iteration that runs it
   (I111–I114), so that a config never says more than akit does.
2. **The handlers' `auth` is refused as not supported yet** until `media_auth` is read (the
   Streaming and media auth track): a policy akit cannot find would otherwise let everyone in.
3. **The roles run on akio's servers** (`akio-turn`, `akio-webrtc`), in a crate of their own,
   `akit-webrtc`, as `akit-tunnel` runs the tunnel's: akio crates, which decision 001 allows.
4. **What they show outside is akit's** (decision 037): the TURN server's `SOFTWARE` is `akit`; the
   metrics are `akit_turn_*`, `akit_webrtc_media_*` and `akit_webrtc_proxy_*`; the logger is fit's
   `webrtc`. The wire is the RFCs' (STUN, TURN, ICE, DTLS-SRTP, WHIP, WHEP), which carries no
   gateway's name, so fit's clients and servers, browsers and pion reach akit's alike.
5. **Where akio-turn lacks fit's hooks, akio is asked** (A256–A258) and akit runs a recorded
   stopgap that follows the rules, or refuses the setting at run time:
   - `port_min` and `port_max` (A256): the role refuses to start with a range, logged; a relay
     bound outside the range a firewall opens would hand clients addresses nobody can reach.
   - `user_bandwidth` (A257): not enforced; a warning when the server starts. The allocations, their
     permissions and channels stay bounded.
   - The server's own addresses (`relay_ip`, `public_ip`) as peers (A257): reached only on the ports
     `allowed_peers` names with them, never on the server's relayed ports, so akit's other
     listeners stay out of reach; two clients of one akit server relay to each other through one
     relayed address and the other's server-reflexive one, not through both relayed ones.
   - The quotas, caps and lifetimes (A258) start a new server when they change, as the addresses
     do; the users, secret, peer rules and access change in place.
   - A drain (A258): the server a reload replaced keeps serving what it holds, and anyone who
     reaches its addresses, for `-shutdown-timeout`, then closes its allocations.
   - `nonce_secret`, `max_stream_message` (A256): not applied, a warning; akio-turn's nonces use a
     random key of each server, and its framing takes messages up to 64 KiB.
   - `mptcp`: plain TCP, as fit on a kernel without Multipath TCP; a warning.

## Consequences

- A config of fit's with a TURN server runs on akit unless it sets a port range, with the
  differences above logged; I112 removes them as akio serves A256–A258.
- `whip`, `whep` and `webrtc_proxy` with `auth` wait for the Streaming and media auth track.
