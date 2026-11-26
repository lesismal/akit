# 036: The console, fit's pages served as fit ships them, and the store waiting for akio

[English](036-console.md) | [简体中文](036-console.zh-CN.md)

Status: accepted in I90 (2026-11-26), waiting for review ([decisions waiting for
review](../decisions-pending.md), row 16); extended by I91 (the pages) and I92 (the Admin API over
TLS).

## Context

fit's I104 ([fit's decision 027](https://github.com/lesismal/fit/blob/main/docs/decisions/027-console-and-store.md))
gave fit two things beside its Admin API: `admin.console`, web pages behind logins from which the
whole config is read and changed, and `admin.store`, the config kept sealed in a file of fit's own
format, `fitstore`. fit's console has grown since into the workbench (`app/workbench`): plain HTML,
CSS and JavaScript, 2.5 MB of them, embedded in fit's binary, that call the console's API
(`/api/login`, `/api/session`, `/api/workbench`, `/api/m/<module>/...`), the config's module being
the Admin API's endpoints called in process. akit's Admin API is fit's (decision 006), on an akio
engine of its own. akit and fit are both under the Apache License 2.0.

## Decision

1. **The console on an engine of its own**, one I/O thread, HTTP/1.1, as the Admin API's (decision
   006), not on the listeners' engine as fit's has been since 2026-11-02: for the same reason fit
   gives (a config whose listeners fail leaves it reachable), and so that the console's few
   connections and its pool are bounded apart from the listeners'. It binds before the Admin API,
   so that an address it cannot have stops akit at start.
2. **The Admin API called in process, on the same request.** A request to `/api/m/admin/<path>` has
   its target rewritten to `/<path>` (akio-http's `Context::req_mut`) and goes to the Admin API's
   dispatch, past its token and `Host` checks, which the console's session, CSRF token and origin
   check replace; changes go to the control thread as the API's own, applied as source `console`.
   No copy of the request or the answer is made, unlike fit's recorder; the console's headers
   and its log of the change (a response filter) go with the answer whoever writes it.
3. **fit's names, so that fit's pages and scripts work unchanged**: the cookie
   `fit_console_<port>`, the header `X-Fit-Csrf`, the endpoints and their JSON, the messages.
4. **The logins are fit's** (package `login`, ported with its tests): lockouts by address and by
   user, sessions by their SHA-256, CSRF tokens. A password is checked on a pool of the console's
   own (`akio::Pool`, one thread and 16 waiting, then 503), never on the I/O thread.
5. **Connections bounded as the Admin API's until akio's A232.** fit closes a connection without a
   login 5 s after it opens, and past `max_conns` closes the oldest such connection to make room,
   keeping a quarter of the slots for logged-in ones; akio-http gives a handler no handle on its
   connection. Meanwhile: `max_conns` at accept, akio's head (10 s), body (30 s) and idle (2 min)
   timeouts, and an answer to a request without a valid session closes its connection.
6. **Passwords change only with a store**, as in fit: without `admin.store` the account endpoints
   answer 409. The store (I93) waits for akio-crypto's Argon2id (A230) and XChaCha20-Poly1305
   (A231): fit's format cannot be read without them, and a format of akit's own would be a store
   fit cannot open. The threat model's L5 (keys stored unencrypted) stays accepted: fit's store
   holds the config and the console's users, not the certificate storage, which akit shares with
   fit (I62) and which an encrypted store would make unreadable to it.
7. **`akit hash-password` shows the password typed at a terminal** until akio can turn the echo off
   (A233); the prompt says so and how to hide it.

## Consequences

- A config changed in the console runs and is in `/versions` as the console's; a restart starts
  from the config files again, as fit's without a store.
- fit's console users and their hashes work in akit as they are.
- Until A232, a client without a login can hold up to `max_conns` connections for akio's head
  timeout each.
