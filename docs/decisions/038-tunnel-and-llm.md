# 038: The tunnel and the LLM feature: fit's roles, akit's names, fit's wire protocols

[English](038-tunnel-and-llm.md) | [简体中文](038-tunnel-and-llm.zh-CN.md)

Status: accepted in I98 (2026-11-27), waiting for review ([decisions waiting for
review](../decisions-pending.md), row 17); extended by the iterations of the Tunnel and LLM track
(I98–I109) as they land.

## Context

fit's I105–I107 and I159–I162 added six roles to fit's config: the tunnel's `tunnel.hub`,
`tunnel.node` and `tunnel.proxy`, which reach TCP services behind NAT through a hub, and the LLM
feature's `llm.hub`, `llm.node` and `llm.proxy`, which share AI models among machines
([fit's decision 028](https://github.com/lesismal/fit/blob/main/docs/decisions/028-llm-and-tunnel.md)).
Their links authenticate with tokens a listener issues, `fitt1.<id>.<secret>`
([fit's decision 035](https://github.com/lesismal/fit/blob/main/docs/decisions/035-link-tokens.md)),
and run fit's own protocols: the tunnel's frames and registration over TCP, TLS, WebSocket and
QUIC (ALPN `fit-tunnel`), and the LLM mesh's frames, sessions and streams over WebSocket. Those
protocols carry fit's name in places no user sees: the HKDF and HMAC labels of the token proofs
and the sealed bodies (`fit link token server key`, `fit-tunnel-link v4 token dialer to
listener`, `fit tunnel register v1`, `fit llm mesh v1`, `fit llm wire listener`), and the ALPN.

[Decision 037](037-akit-names.md) makes every external name akit shows say "akit", with no alias,
and keeps what is fit's format rather than a name (the config's fields, `fitstore`, Proxy-Wasm's
ABI). Whether an akit node can link to a fit hub depends on which side of that line the wire
protocols fall.

## Decision

1. **The roles are read as fit's** (I98): the six members with fit's fields, defaults, bounds and
   checks, in `akit-config`'s `roles` module, the gate's six examples valid. `akit run` refuses a
   config that sets a role until the iteration that runs it (I100–I109), so that a config never
   says more than akit does.
2. **The names akit shows are akit's** (decision 037): link tokens are `akitt1.<id>.<secret>`
   (`fitt1.` is not read); the proxy's response header is `X-Akit-Node`; the files akit writes are
   `akit.tunnel.hub.pem` and `akit.llm.<role>.pem` beside `akit.json`; the self-signed
   certificates' subjects say "akit"; the LLM pages' cookie is `akit_llm_<role>`; the GPU scripts'
   variable is `AKIT_LLM_GPU_DATA_DIR`. The loggers are fit's `tunnel` and `llm`. The LLM proxy's
   API keys keep their `ah-` prefix, which names no gateway.
3. **The wire protocols are fit's, byte for byte**: frames, field orders and sizes, method names
   (`/mesh/hello`, `/hub/proxy`, ...), WebSocket paths (`/tunnel`, `/ws`), close codes, the ALPN
   `fit-tunnel`, and every HKDF and HMAC label. They are protocol identifiers, never shown, like
   `fitstore` and Proxy-Wasm's ABI, so they stay as decision 037's point 2 keeps formats. **An
   akit node or proxy links to a fit hub, and a fit node or proxy to an akit hub**; the LLM mesh's
   roles mix the same way, each side checking the other's certificate and token as fit does.
4. **A token's secret, not its prefix, is the credential.** A listener keeps the SHA-256 of a key
   derived from the secret alone, and a dialer sends the token's ID and proofs, never the token.
   So a token fit issued links to akit's listener once the listener holds its entry (`id`, `hash`,
   the rest as fit wrote them), and an akit node given a token fit issued links to the fit hub
   once the token is written `akitt1.<id>.<secret>`, the ID and secret unchanged. That rewrite of
   the prefix is the whole cost of akit's name on the wire's edge; nothing else differs.
5. **The version a role says hello with is akit's** (`akit <version>`), as fit's says fit's; what
   a peer does with it is written here by the iteration that ports it (I101, I104).

## Alternatives

- **akit's names on the wire too** (`akit-tunnel`, `akit link token ...`): no akit role would
  link with a fit role, for labels nobody sees, and akit's tests could not check its proofs
  against fit's vectors. Refused.
- **Read `fitt1.` as an alias of `akitt1.`**: decision 037 refuses aliases; the prefix is a
  config value a user rewrites once.

## Consequences

- Mixed deployments work: a fit hub with akit nodes, an akit hub with fit proxies, during a move
  from one to the other or for good. The interoperability is tested against fit's own vectors (the
  token proofs and keys, I99) and, where a frame's bytes are fixed, against fit's encodings.
- A change fit makes to its protocols is a change akit follows, as a change to the config's
  format is.
- `akitt1.` tokens given to a fit node must be written `fitt1.` there: fit's checks refuse any
  other prefix.
