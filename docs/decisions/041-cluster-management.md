# 041: Cluster management: fit's manager, fit's wire to members, akit's names

[English](041-cluster-management.md) | [简体中文](041-cluster-management.zh-CN.md)

Status: accepted in the plan of the track (2026-11-27), waiting for review ([decisions waiting for
review](../decisions-pending.md), rows 70–74); extended by the track's iterations (I130–I139c) as
they land.

## Context

fit's I140–I150 make one fit, with `admin.cluster`, the manager of others, its members
([fit's decision 033](https://github.com/lesismal/fit/blob/main/docs/decisions/033-cluster-management.md)).
The manager keeps its members, their credentials, groups, templates, rollouts and audit log in its
store's section `cluster`, polls each member's `GET /node`, `/status` and `/config/check`, and
changes members through their own APIs: the console, logged in with a user name and a password, or
the Admin API with its token. Members that cannot be reached link out to the manager (I149). The
workbench's Cluster pages, which I91 copied into akit with the rest of fit's pages (renamed by
[decision 037](037-akit-names.md)), draw all of it.

Two things decide how akit follows: what of fit's protocol between a manager and its members akit
keeps, and which names on it are akit's.

## Decision

1. **The track is fit's, in twelve iterations** (I130–I139c, the dev plan's "Cluster management"),
   in fit's order; fit's I153, the assistant across a cluster, goes with the AI assistant track,
   which it needs. The manager is a crate of its own, `akit-cluster`, served by `admin.console`
   as the console module `cluster`.
2. **What a member answers is fit's protocol**: the paths (`/node`, `/status`, `/config/check`,
   `/schema`, `/api/login`, `/api/logout`, `/api/node`, `/api/m/admin/...`), the JSON members,
   the capability names (`config.dry_run`, `status.requests`, ...), the problem kinds, the store's
   sections `node` and `cluster` and the cluster document's shape. They are protocol identifiers,
   as decision 038's point 3 keeps the tunnel's. The members fit names after itself are renamed
   as decision 037 renames them everywhere, since the pages read them so: `GET /node` answers
   `akit_version`, not `fit_version`. Its `fib_version` and `go_version`, fit's engine and
   toolchain, which the pages show beside it, carry akit's: `akio <commit>` and the empty string
   (akit's build records no compiler version).
3. **A manager speaks to fit and akit members alike.** akit's manager sends the session's CSRF
   token in both `X-Akit-Csrf` and fit's `X-Fit-Csrf`, each side reading its own, and reads a fit
   member's `fit_version` as the version `fit <version>`, so that the pages show which a member
   is. A fit manager reaches an akit member through its Admin API (`admin_api`, a token): akit's
   console reads only `X-Akit-Csrf` (decision 037: no alias), so a fit manager logged in to an
   akit console reads it and cannot write.
4. **The store is shared**: the `cluster` document is fit's, versions 1 to 3 read, version 2
   written without roles and 3 with them, as fit's; what an akit manager does not yet use (groups,
   templates, rollouts, links, roles, before their iterations) is kept as it was read and written
   back unchanged, so that a store moves between an akit and a fit manager without losing them. A
   newer document is shown read-only, as fit's.
5. **The names akit shows are akit's** (decision 037): the node ID's file beside the config is
   `akit.node-id`; enrollment tokens and credentials are `akite1.` and `akitc1.` (fit's `fite1.`
   and `fitc1.`); the manager's `User-Agent` is `akit-cluster-manager`. A token's prefix is
   rewritten when it moves between akit and fit, as decision 038's point 4 says of link tokens.

6. **The manager as I132 builds it**: `akit run` starts it on the listeners' engine and refuses
   `admin.cluster` without a store; a poller thread of its own waits out the interval and starts
   asynchronous calls; the store is written on a pool of the manager's own, one write at a time.
   A member's certificate is read by a probe that verifies nothing and sends no credential, until
   akio's client gives a response's chain and a refused certificate (A277). Tests are audited, as
   fit's I148 decided. The console's bodies other than a config grow to 128 KiB, fit's bound of the
   module.

7. **The node switcher (I134)** passes fit's header fields back under akit's names
   (`Fit-Config-Version` as `Akit-Config-Version`, `Fit-Dry-Run` as `Akit-Dry-Run`) and sends a store
   password in both `Akit-Store-Password` and `Fit-Store-Password`, so that akit's pages read a fit
   member; a fit member's schema is fit's, so its pages open read-only, as fit's I143 decides for any
   schema the manager does not know.

8. **Templates on akit members (I136)**: an akit member answers its config with its secrets
   redacted (decision 035), so the manager compares a part, and keeps its hash, with the secrets
   redacted on both sides for an akit member (one whose `GET /node` answers `akit_version`), and
   whole for a fit member; an akit member's `GET /node/managed` hashes its config as its API answers
   it, so the two agree. A secret changed in a template alone does not show an akit member out of
   sync.

9. **Rollouts (I137)** are fit's: the plan, the strategies, the gates, the steering, and the
   document's `rollouts`, which either manager reads. Three things are akit's. A rollout's plan and
   its run each take a thread of their own and wait for the members' answers there, never on an
   I/O thread (rule 1). A copy from an akit member, whose config answers its secrets as
   `[redacted]`, masks them as a template's `[secret]`, so that each member it is copied to keeps
   its own and a fit member is never written the word `[redacted]`. And the drift gate applies
   where a member lists `config.check`, as fit's does: fit members, and akit members from I138.

10. **Drift and alerts (I138)** are fit's: the consistency check's differences and fixes (`GET
    /config/check`, `POST /config/reconcile`), the rules, silences and webhooks, and the store's
    section `cluster.alerts`, which either manager reads. akit's check reads what runs from `GET
    /status`'s and `GET /node`'s writers, and the settings that take a restart are those a config's
    warnings name; its values are answered with their secrets redacted (decision 035). A webhook's
    notification says `akit-cluster` and "akit cluster", signed in `X-Akit-Signature` and
    `X-Akit-Timestamp` (decision 037: an external name). The rules are evaluated when a poll ends,
    something changes, or the earliest of their times comes, not on fit's one-second tick (`tick_ms`
    answers 0); a managed member is read against its templates when its running config changed,
    not again every ten minutes as fit's.

## Alternatives

- **akit's names on the wire too** (`akit_version` only, `X-Akit-Csrf` only): a manager of one
  kind could not manage members of the other. Refused for the manager's side; kept for the
  member's console, whose CSRF header decision 037 renamed.
- **An akit member answering fit's `fit_version` too**: an alias, which decision 037 refuses; a
  fit manager shows no version for an akit member.

## Consequences

- akit and fit members share a manager of either kind: an akit manager with console or token
  access to both; a fit manager with token access to akit members.
- A fit manager's switcher treats an akit member as another schema (its digest is of akit's
  renamed schema): read-only there, as fit's I143 decides for any schema it does not know.
