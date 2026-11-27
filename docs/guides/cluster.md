# Managing many akits

[English](cluster.md) | [简体中文](cluster.zh-CN.md)

One akit, the **manager**, can manage others, its **members**: it keeps their URLs and an
administrator's credentials in its encrypted store, reads each one's state every `poll_interval`,
and shows them in the console's Cluster pages. A member changes nothing for it: the manager speaks
the APIs the member already has, its console (a user name and a password) or its Admin API (a
token). The members may be fits too: what they answer is fit's protocol
([decision 041](../decisions/041-cluster-management.md)). The manager is not in the data path: a
manager down costs management and nothing else.

This guide covers what akit runs now (I130–I139): members, their state, the Overview and Nodes
pages, a member's page, the node switcher, groups and operations on many, templates, rollouts
with health gates, drift and alerts, users' roles and the audit log. Members behind NAT and the
`akit cluster` command follow in I139b and I139c ([the plan](../dev-plan.md#cluster-management));
their menu entries are drawn already and answer "no such endpoint" until then.

## A manager

`admin.cluster` turns it on. The members are served in the console, so `admin.console` is
needed, and kept in the store, which `akit run` checks: `admin.store`, or the store `akit run
-admin` keeps beside the config.

```json
{
  "admin": {
    "console": {"users": [{"username": "ada", "password_hash": "$2b$10$..."}]},
    "store": {"path": "/var/lib/akit/akit.store", "password_env": "AKIT_STORE_PASSWORD"},
    "cluster": {"poll_interval": "10s", "timeout": "10s", "max_concurrency": 8}
  }
}
```

A manager alone, with no listener or route, is a valid config: `akit run -admin -c manager.json`.

## Members

Every akit answers what a manager reads (I131): `GET /node` (its node ID, versions, running config
and capabilities), `/status` and `/schema`. The node ID is made once and kept in the store, or in
`akit.node-id` beside the config; set `admin.node_id` where neither lasts, as in a container whose
config is read-only.

On the manager, **Cluster → Nodes → Add**:

- **By console**: the member's console URL and one of its users. The manager logs in, keeps the
  session, and logs in again when it ends. A refused password is not tried again for a minute,
  doubling up to 15, unless the member is edited or tested, so that the manager never locks the
  member's users out.
- **By Admin API**: the member's Admin API URL and its token (`admin.token_file`).

The URL is `https`, or `http` to a loopback address only, so that credentials never cross a
network in the clear. A certificate is trusted by the system's CAs, a CA of the member's own, or
its SHA-256 pinned: **Test** shows the certificate a member presents, and confirming its
fingerprint pins it. Passwords and tokens are written once, shown as set or not, and never
answered, logged or sent but to their member.

The Overview shows each member's state, colored: **up**, **error** (reached, but its certificate,
its credentials or its answers failed, each told apart), **down** (not reached in time) or
**unknown** (not read yet), with its version, config version, connections, upstream backends
down, nearest certificate expiry and latency. A member's page reads its node, status, versions and
consistency through the manager.

## A member's own pages

The node switcher in the header opens any member's own pages (config with its text tabs, versions,
Engine, the role pages) against that member: the manager passes each call on with the session or
token it keeps, and the browser never holds the member's credentials. A member whose config schema
is not the manager's (another akit version, or a fit) opens read-only; every write through the
switcher is in the audit log with its user, the config versions before and after, and the digest of
what was sent.

## Groups and operations

A group is a label selector (`region=eu, tier!=db`, Kubernetes' equality terms), members by name,
or both; its members follow the labels as they are. **Cluster → Groups** lists them, with each
group's members and their states, and runs an operation on a group or a selection: **reload**,
**rollback** (to the latest earlier config that differs), **check**, **purge** a cache (by key,
URL prefix, tag or all) and **test**, at most `max_concurrency` members at once, each answered
apart. A member down fails alone; every operation is in the audit log with each member's outcome.

## Templates

A template owns parts of its members' configs: top-level members whole (`log`, `tls`), and the
elements of `listeners`, `routes`, `services` and `upstreams` by name. Its strings may use
`${node.name}`, `${node.id}`, `${node.hostname}`, `${node.labels.<key>}` and its groups'
variables. **Cluster → Templates** shows each member's sync state (in sync, out of sync, or a
conflict where the member changed an owned part), its desired config, and makes a template from a
member's config. Each member is told what the manager owns there and marks those fields on its own
pages. A secret in a template is shown as `[secret]`; an akit member's secrets are compared as its
API answers them, redacted.

## Rollouts

**Cluster → Rollouts** changes many members' configs at once: a template (or a draft of it, saved
as the template when the rollout starts), a sync with the templates, parts of one member's config
copied to others, or a patch of parts. A rollout is planned first: each member's changes and a dry
run, with the ETag of the config read. Started, it applies all at once, in rolling batches, or a
canary first, which then waits for a person to go on; each member is applied with `If-Match` that
ETag, so a member changed since the plan is skipped, never overwritten. After each batch a gate
checks each member: reachable and running the config applied, no more drift than before (where the
member checks its config), and, when set, its share of 5xx over a window from its `GET /status`
(its config must count requests: `metrics`). A failed gate rolls back its batch, or every member
applied. A rollout is paused, resumed, aborted or rolled back by a person; it is kept in the
store, and one under way when the manager stops is found paused, to be resumed.

## Drift and alerts

Each akit checks itself: **Gateway → Consistency** (`GET /config/check`) shows what runs that
differs from its config, a setting that takes a restart, a listener not bound, a certificate not
served, and offers fixes, applied together. **Cluster → Drift** shows each member's drift against
its templates and its own check, and fixes a template's drift by a sync rollout. **Cluster →
Alerts** has rules (a member down, drift, a rollout failed, a certificate near its expiry,
backends down, reloads failing), each firing once its condition has held for its time, sent to
webhooks (JSON signed in `X-Akit-Signature`, Slack, Discord, Feishu, DingTalk, WeCom, or a
template of one's own) and silenced by rule or member for a while.

## Users, roles and the audit log

Until roles are set (and with no `admin.cluster.admins`), every console user is an admin of the
cluster. **Cluster → Users and audit** gives each console user a role, viewer, operator or admin,
cluster-wide or on groups; a member's role is the highest of the cluster's and its groups'; users
not named get the default role, and `admin.cluster.admins` are admins whatever the roles say. A
viewer reads, an operator runs operations, syncs, rollouts of templates and silences on its
members, an admin changes them; the manager's own config, users and services are changed by a
cluster-wide admin only. A refusal says the role it needs, and where. Every change is in the audit
log, filtered by user, member and action, and exported as JSON lines.

## Mixed clusters

- An akit manager manages akit and fit members alike, by console or by token; a fit member's
  version reads `fit <version>`.
- A fit manager manages an akit member by its Admin API's token. akit's console takes its CSRF
  token in `X-Akit-Csrf` only (decision 037), so a fit manager logged in to an akit console can
  read it but not write.
- akit's and fit's managers share a store: each reads the members, groups, templates, rollouts,
  roles and alerts the other wrote, and keeps what it does not use as it was.

## Limits

`max_nodes` members (256); `max_concurrency` members polled at once (8), each call within
`timeout` and its answer within `max_response_bytes` (4 MiB); 4 tests and 8 reads of members at
once; the audit log keeps the last 1,000 changes (about 2 MiB); 4 rollouts planned or under way
and 50 kept, 8 MiB of configs and diffs a rollout, a plan started within an hour.
