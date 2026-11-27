# 039: Streaming and media auth: fit's members, akio-media's hub, fit's tokens

[English](039-streaming-and-media-auth.md) | [简体中文](039-streaming-and-media-auth.zh-CN.md)

Status: accepted in I120 (2026-11-27), waiting for review ([decisions waiting for
review](../decisions-pending.md), rows 55–59); extended by the iterations of the Streaming and media
auth track (I120–I126) as they land.

## Context

fit's I115–I117 added two members to its config
([fit's decision 030](https://github.com/lesismal/fit/blob/main/docs/decisions/030-media-auth-and-pages.md)):
`media_auth`, named policies of who may publish and play which streams (static tokens, users, a
secret that signs tokens, public actions), and `streaming`, a media hub on fib's `media/hub` with
RTMP, RTSP and SRT listeners and handlers that play it over HTTP-FLV, HLS and WHEP; and the media
pages. fit later added to the same members what belongs to other tracks: a keyring and the handler
`media_tokens` (fit's I119), signed playback URLs, channels and push targets (the media library).
akio has the media stack fib has: akio-media's hub, RTMP, RTSP and SRT servers, HTTP-FLV and HLS,
WHIP and WHEP. Some of what fit's role asks of fib is not in akio-media's API yet.

## Decision

1. **The members are read as fit's** (I120): `media_auth` and `streaming` with fit's fields,
   defaults, bounds and checks, in `akit-config`'s `media` module. What belongs to later tracks
   (`keys`, `url_signing`, `channels`, `push`) and what akio-media cannot do yet (below) is refused
   as not supported yet, by name, so that a config never says more than akit does. `akit run`
   refuses a member until the iteration that runs it.
2. **One crate, `akit-media`**, holds the policies, the role and the handlers, on akio-media (an
   akio crate, at the same commit as the rest: no dependency of a new kind).
3. **The names akit shows are akit's** (decision 037): the cookie a token may come in is
   `akit_media_token`, the command `akit media-token`, a 401's challenge `Bearer realm="akit"`, the
   metrics `akit_streaming_*`; RTSP's `realm`, when akit reads it, defaults to `akit`.
4. **The tokens akit signs are fit's, byte for byte**: `fm1.<claims>.<mac>`, the claims fit's JSON
   (`sub`, `exp`, `pub`, `play`) in unpadded base64url, the MAC an HMAC-SHA256 of `fm1.` and the
   claims under a key derived from the policy's secret and name with fit's label (`fit media
   token`). Like the tunnel's wire constants (decision 038), the prefix and the label are a format,
   never shown, so they stay fit's: a token fit's `media-token` signed is taken by akit given the
   same secret and policy name, and one akit signs by fit.
5. **bcrypt off the I/O threads, as fit's**: a `password_hash` user's password is checked on a
   bounded pool of `akit-media`'s own (a quarter of the CPUs, 256 waiting, 5 s, fit's numbers),
   and remembered once right as an HMAC under the policy's random key. An HTTP request waits for
   the check, suspended (`Context::suspend`); a listener's hook, which must answer at once,
   refuses with the protocol's retry and starts the check (I122).
6. **Policies by engine and name**: a policy is one object across reloads, changed in place by the
   role `media_auth`, which `akit run` applies before the roles and handlers that name it; a
   handler provisioned before the role applies a new policy compiles it from the config being
   compiled, as fit's `Provision`. The state is read through a per-thread cache of what the control
   thread swapped, so a check takes no lock and touches no shared count.
7. **Stopgaps where akio-media falls short**, each with an akio request (rule 21), each field it
   would need refused until then: RTMP's `max_message_bytes` (A263), RTSP's Digest and Basic,
   `auth_methods`, `realm` and the sessionless `idle_timeout` (A264). Until A264, RTSP takes
   credentials in the URL's query (a token, or a user and password), checked in the hub's hooks.
8. **The role runs on the listeners' engine as the tunnel's roles do** (I122): its RTMP and RTSP
   listeners are akio-media's servers behind an acceptor of the role's own that bounds
   `max_conns` (a connection past it closed as it opens, counted down when its peer goes), its SRT
   listener a UDP listener; the role's `access`, then the global one, on akio's accept filter, set
   again by every apply. Its metrics join `/metrics` through `Role::metrics`, with akit's
   names. More stopgaps showed under ffmpeg (A264 extended, A265): RTSP's play and ffmpeg's
   publish cannot carry a query token through akio-media's DESCRIBE and SETUP, and SRT's
   `r=...?token=` is refused with 1400. Each fails closed: a client fit would let in may be
   refused, none fit would refuse is let in.

## Alternatives

- **akit's names on the tokens** (`am1.`, an akit label): fit's and akit's tokens would not be
  taken by each other, for names nobody sees. Refused, as for the tunnel.
- **Digest and Basic on akio's `RtspConfig::credentials`**: it asks every request to authenticate
  and does not tell the hooks who the user is, so a policy's public play, its tokens and its
  per-user grants could not be honoured. Refused until akio's request hook (A264).

## Consequences

- A token from fit's `media-token` works on akit, and one from `akit media-token` on fit; the
  cookie does not move between them (decision 037).
- RTSP clients that only speak Digest or Basic cannot publish or play a stream behind `auth` until
  A264; ffmpeg and most players take the URL's `?token=` or `?user=&pass=`.
