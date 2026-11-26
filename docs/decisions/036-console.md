# 036: The console, fit's pages served as fit ships them, and the store waiting for akio

[English](036-console.md) | [简体中文](036-console.zh-CN.md)

Status: accepted in I90 (2026-11-26), waiting for review ([decisions waiting for
review](../decisions-pending.md), row 16); extended by I91 (the pages) and I92 (the Admin API over
TLS); its 3 and 8 amended by [decision 037](037-akit-names.md) (akit's names); its 5 replaced on
2026-11-27, once akio had A232, its 7 once akio had A233, and its 6 brought up to date by I93.

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
3. **fit's endpoints, their JSON and messages; akit's names** (amended by decision 037, which
   renamed what this kept of fit's so that fit's pages and scripts worked unchanged): the cookie
   `akit_console_<port>` (fit's `fit_console_<port>`), the header `X-Akit-Csrf` (fit's
   `X-Fit-Csrf`).
4. **The logins are fit's** (package `login`, ported with its tests): lockouts by address and by
   user, sessions by their SHA-256, CSRF tokens. A password is checked on a pool of the console's
   own (`akio::Pool`, one thread and 16 waiting, then 503), never on the I/O thread.
5. **fit's connection rules, on akio's connection handles** (A232, akio's ADR-180): a connection
   without a valid session is closed 5 s after it was accepted, a request served then answered
   with `Connection: close` within 5 s more; one with a session is kept 2 minutes between
   requests; past `max_conns`, or past the three quarters of it unauthenticated connections may
   hold, a new connection closes the oldest unauthenticated one of the client holding the most,
   each client other than the host's own held to a quarter (fit's `login.Slots` and `Peers`,
   ported with their tests as `console::slots`). The listener's `on_accept` and `on_close` keep
   the table, `ConnHandle::close_at` and `cancel_close` the deadlines; the engine's own bound is
   twice `max_conns`, for the connections being closed. Until A232 (2026-11-27) the connections
   were bounded as the Admin API's: `max_conns` at accept, and an answer to a request without a
   valid session closed its connection.
6. **Passwords change only with a store**, as in fit: without `admin.store` the account endpoints
   answer 409. The store is fit's format, byte for byte, on akio-crypto's Argon2id (A230) and
   XChaCha20-Poly1305 (A231), which akio has since 2026-11-27: a format of akit's own would be a
   store fit cannot open. I93 brings the format and `akit store`; I93b keeps the running config
   in it, and the account endpoints work with it then. The threat model's L5 (keys stored unencrypted) stays accepted: fit's store
   holds the config and the console's users, not the certificate storage, which akit shares with
   fit (I62) and which an encrypted store would make unreadable to it.
7. **`akit hash-password` reads a password typed at a terminal as fit's**: in raw mode (akio-sys's
   `terminal::raw`, A233), a `*` shown for each character, Backspace, Ctrl-U and escape sequences
   handled as fit's `readMasked`. Until A233 (2026-11-27) the password was shown as typed, the
   prompt saying so.
8. **The pages are fit's workbench, served with akit's names** (I91; renamed since decision 037:
   `tools/console-pages.py` applies a deterministic list of renames, and `console/CHANGES` lists
   them). fit's licence (Apache-2.0, akit's too) lets akit serve them, and they need no build: they are plain files that fit embeds and
   serves from memory. `tools/console-pages.py` copies `app/workbench/web` and fit's licence at the
   fit commit the compatibility gate pins (`tests/compat/fixtures/PINS`) into
   `crates/akit-admin/console`, and writes the list `include_bytes!` builds into akit; `make
   compat-check` and a test check that the list is the directory's and the commit the pin. The
   forms are made from fit's schema at the same pin (`GET /api/m/admin/schema`). akit adds what the
   config pages call that it did not have, as Admin API endpoints, fit's: `/config/render`,
   `/config/parse`, `/config/text`, `/config/export` and `/config/import` (their `store` format
   waits for I93); a text's errors are placed at their lines in it (`akit_config::position_of`),
   as fit's adapter places them.
9. **The Admin API over TLS** (I92), as fit's `checkAdminAddress`: `cert_file` and `key_file`
   (TLS 1.2 and 1.3), `client_ca_file` asking every client for a certificate it verifies; off
   loopback only with a token and TLS. With the console over TLS, this closes the threat model's
   L1. The API's and the console's certificate files are read when akit starts; fit reads them
   again when they change, which I94 brings.

## Consequences

- A config changed in the console runs and is in `/versions` as the console's; a restart starts
  from the config files again, as fit's without a store.
- fit's console users and their hashes work in akit as they are.
- A client without a login holds a connection 5 s at most, and cannot keep a logged-in operator
  out by reconnecting.
- The pages said "fit" until decision 037; their menu holds fit's features that akit does not run yet: their forms
  edit the draft, a save that sets one is refused as not supported yet, and the endpoints akit does
  not have (`/node`, `/config/check`, the media, BitTorrent and sync pages') answer 404. An
  `index.html` of akit's own loading only the modules akit runs would change one file of fit's;
  it is the other choice in [row 16](../decisions-pending.md).
- Moving the gate's pin moves the pages with it: `tools/console-pages.py` is run in the same commit.
