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
