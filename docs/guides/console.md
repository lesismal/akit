# The console

[English](console.md) | [简体中文](console.zh-CN.md)

The console is a set of web pages behind a user name and password, from which the running config is
read and changed: fit's console, which akit serves with fit's own pages under akit's names. It is `admin.console`,
beside the [Admin API](admin-api.md), whose endpoints it calls in process behind its own logins, so
that a change made in the console is applied by the one path every config takes, with the same
checks, and is in `/versions` as the console's. The [reference](../reference/config.md#the-console)
has every field; [decision 036](../decisions/036-console.md) says where akit differs from fit.

## Setting it up

A user is a name and a bcrypt hash; a password is never written in clear text. `akit
hash-password` prints a password's hash, read from standard input (`-cost` sets bcrypt's cost, 10 by
default; a password is at most 72 bytes). It never takes the password as an argument, which other
users of the machine could see. From a terminal it asks twice, but cannot hide what is typed yet
(akio's A233); to hide it:

```bash
read -rs p; printf '%s\n' "$p" | akit hash-password
```

fit's hashes work as they are. Then:

```fit
admin {
	address 127.0.0.1:2019
	console {
		address 127.0.0.1:2020
		users { username admin; password_hash "$2b$10$r4CnBqUvPHMEJ2c8gc9SsOZeUyDeOB3PHsKffYxFv2ckCM4qrof.C" }
	}
}

listener web :8080 http/1.1 h2c
route { handler respond { body "hello\n" } }
```

and open <http://127.0.0.1:2020/>. A config of `admin` alone, with no listener, is valid: the rest
can be added from the console.

**Off loopback, the console needs TLS**: an address that is not a loopback one must have
`cert_file` and `key_file`, so that passwords never cross a network in the clear; `akit validate`
refuses one without. On a loopback address TLS is optional, as for a console reached through an SSH
tunnel. `access` says which client addresses it serves ([access lists](../reference/config.md#access-lists)),
deciding before the global `access`, which applies to the console too: `allow 127.0.0.1 ::1` keeps
local access whatever the global access says. Unlike the rest of `admin.console`, read when akit
starts, `access` follows every config applied.

## Logins and sessions

- **Checking a password.** A name that is not a user is checked against a dummy hash, so the time
  taken does not tell names apart; at most two checks run at once, on a pool of their own (one
  running, 16 waiting, then `503`), never on the console's I/O thread.
- **Lockouts.** A client address that fails 5 times in 15 minutes is refused for 15 minutes, with
  `429` and `Retry-After`; each further lockout before it has been quiet for 15 minutes lasts twice
  as long, up to an hour. An IPv6 address counts by its /64. One check from an address runs at a
  time; 4,096 addresses are kept.
- **Slowed users.** A user whose password fails 10 times in a row, from any addresses, has its checks
  refused for 2 seconds, then twice as long after each further failure, up to 5 minutes.
- **Sessions.** A login gives a session: 32 random bytes in the cookie `akit_console_<port>`,
  `HttpOnly`, `SameSite=Strict`, and `Secure` over TLS. The console keeps only its SHA-256, and ends
  it after `session_timeout` (30m) without a request, after `session_lifetime` (12h) in all, at
  logout, or as the oldest when `max_sessions` (64) are kept. Sessions are in memory: a restart logs
  everyone out.
- **Other web pages.** Every request but a read carries the session's CSRF token in `X-Akit-Csrf`,
  or gets `403`; a request whose `Origin` is not the console's own gets `403`. Answers carry a
  Content-Security-Policy that allows the pages' own files only and `frame-ancestors 'none'`,
  `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, and over TLS `Strict-Transport-Security`;
  the API's answers `Cache-Control: no-store`.

The `admin` logger records who logged in and from where, each failed login, each logout, and each
change a user made with the config version it applied.

**Passwords are changed with a store.** fit keeps a changed password in `admin.store`, which akit
does not have yet ([I93](../dev-plan.md#i93-the-encrypted-store)): until then the endpoints answer
`409` and the users are the config's.

## The pages

After logging in, the workbench: fit's own pages (fit's `app/workbench/web` at the fit commit the
compatibility gate pins), with fit's names renamed as akit's
([decision 037](../decisions/037-akit-names.md)): they say "akit", send `X-Akit-Csrf`, read
`Akit-Config-Version`. `tools/console-pages.py` copies them and applies a fixed list of renames,
which `crates/akit-admin/console/CHANGES` lists; nothing else of fit's pages changes. Its menu is
one flat list: the config's global pages first (overview, listeners, the engine's ports, the global
settings, the whole config as JSON, the versions with rollback, the consistency check, the logs and
the account pages), then the protocol and feature pages. Each config page is a form made from fit's
config schema, with each field's doc, and `.conf`, `.yaml` and `.json` tabs that edit its part of
the config as text; **Save and apply** sends the draft (or the tab's text) with `If-Match`, and
**Check** sends it with `?dry_run=1`. A config with errors is refused and its errors are shown at
their paths (and, from a text tab, at their lines).

The **Logs** page (fit's I157) lists each file output of `log` and the access log: the loggers
writing there, the format and zone, the rotation and retention, the files on disk with their sizes
and ages; **Rotate now** and **Clean up now** (with a preview), each in the `admin` logger; and the
forms of `log` and `access_log`, whose zone, rotation and retention apply without a restart.

What akit does not run yet shows as fit has it, and akit answers it as it answers the config:

- The pages of fit's features that akit refuses in a config (streaming, media auth, WebRTC, the
  tunnel and LLM roles, the media library, BitTorrent, sync) are in the menu; their forms edit the
  draft, and a save that sets one is refused as "not supported yet".
- Endpoints akit does not have answer 404, and the page that calls them says so: the consistency
  check (`/config/check`), **This node** (`/node`), the cluster manager's marks
  (`/node/managed`), and the media, BitTorrent and sync pages' own.
- The `store` format of the transfer page, and changing a password, need `admin.store` (I93).

## For scripts

The console's own endpoints, as fit's:

| Endpoint | |
| --- | --- |
| `POST /api/login` | `{"username", "password"}`, answered with `{"user", "csrf"}` and the session's cookie |
| `POST /api/logout` | ends the session (`204`) |
| `GET /api/session` | `{"user", "csrf"}` |
| `GET /api/workbench` | the user and the modules served: `admin` |
| `GET /api/m/admin/users` | the signed-in user, the users, and whether a store keeps them |
| `GET /api/m/admin/schema` | fit's config schema, renamed as the pages are, which their forms are made from |
| `/api/m/admin/<path>` | the Admin API's `/<path>`, behind the session instead of the token, as `GET /api/m/admin/config` |

A program keeps the cookie and sends `csrf` in `X-Akit-Csrf` on every request but a GET or a HEAD
(fit's scripts, which send `X-Fit-Csrf`, take akit's name):

```bash
curl -c jar -H 'Content-Type: application/json' \
  -d '{"username": "admin", "password": "change me"}' http://127.0.0.1:2020/api/login
curl -b jar http://127.0.0.1:2020/api/m/admin/config
```

## Limits

| | |
| --- | --- |
| Connections | `max_conns` (32); past it a new connection is closed as it is accepted |
| A request's header | read within 10s |
| A request's body | `admin.max_config_bytes` and 64 KiB, read within 30s; a login's 4 KiB |
| A connection | kept 2 minutes between requests; an answer to a request without a valid session closes it |
| Password checks at once | 2, on a pool of one thread with 16 waiting |
| Failed logins | 5 an address (an IPv6 /64) in 15 minutes, then refused for 15 minutes, one check at a time; 4,096 addresses kept |

fit also closes a connection without a login 5 seconds after it opens, and when `max_conns` are
open closes the oldest one without a login to make room, keeping a quarter for those with one; akit
waits for akio to let it (A232).
