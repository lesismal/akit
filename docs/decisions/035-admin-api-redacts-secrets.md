# 035: The Admin API redacts secrets, akit warns of exposed ones, and the rest of I80b's limitations

[English](035-admin-api-redacts-secrets.md) | [简体中文](035-admin-api-redacts-secrets.zh-CN.md)

Status: taken in I80b (2026-11-26), waiting for the maintainer's review
([decisions waiting for review](../decisions-pending.md), row 14).

## Context

I80 wrote the [threat model](../security/threat-model.md) and its known limitations; I80b closes
those that are akit's. Several ask for a choice:

- **L2.** `GET /config`, `/versions/{n}` and `/config/effective` answered the config as written,
  inline secrets included (a `password_hash`, a route's `Authorization` header, a JWT `secret`).
  fit's own Admin API does too ([decision 002](002-config-compatibility.md) makes its payloads
  akit's), but fit's console, its assistant and its cluster manager never show one: they answer
  `[redacted]` (or `[secret]`) for every string under a member fit's `secretName` takes for a
  secret, and put the secret back when a change sends the mask (fit's `restoreSecrets`).
- **L3, L4.** fit reads secret files whatever their permissions, and talks to ACME, Consul, etcd,
  Redis, Docker and Kubernetes over whatever transport the config names. Refusing either would
  refuse configs fit runs.
- **L7.** akio's `Files` follows symbolic links under a root wherever they lead. So does fit's
  `static` handler, on purpose (its package comment says so); fit has no option against it.
- **L8.** akio-tls checks client certificates against CRLs (`ServerConfigBuilder::client_crls`)
  and stapled OCSP (`client_ocsp_policy`), but fit's config has no field naming CRLs.
- **L9.** A WASM module is read from its `path` at each load; fit has no field pinning its digest.
- **L12.** akio's head and body timeouts (10 s, 60 s) are not configurable; fit's config has no
  client-side head or body timeout either (its `limits` has none; its Admin API and console set
  10 s in code).

## Decision

1. **The Admin API redacts inline secrets.** Every read of the config (`GET /config` and its
   pointers, `GET /versions/{n}`, `GET /config/effective`, and the dashboard on them) answers
   `[redacted]` for each non-empty string under a member whose name says it holds a password, a
   key, a token or a hash, by fit's `secretName` rule exactly (not a `*_file`, `*_env`, `*_ttl`,
   `max_*` or `min_*`), but for `rate_limit`'s `key` selectors (`ip`, `route`, `header:Name`), and
   a push target's `url`. A config with none is answered byte for byte as written; one with some is
   written as fit writes an edited config. The `ETag` stays the running config's, so `If-Match`
   works on what was read.
2. **`[redacted]` sent back keeps the secret.** `POST /load`, `PUT` and `PATCH` put back, for each
   `[redacted]` in the body, the secret the running config holds at the same place: matched by
   member, by `name` in a list of named objects, or by index, as fit's `restoreSecrets`. A
   `[redacted]` with no secret at its place is refused with 400 ("... is [redacted], but no secret
   is kept there: write the secret itself"), and nothing changes. There is no switch to read
   secrets back: whoever needs them reads the config's files.
3. **Exposed secrets are warned of, not refused** (`akit_config::hardening`), at `akit run`'s start,
   by `akit validate`, and in the `warnings` of every change applied or checked:
   - a file named by `key_file`, `tls_key_file`, `token_file`, `password_file`,
     `hmac_secret_file` or a provider's `kubeconfig` that its group or others have any permission
     on (once a file), with the `chmod 600` to make;
   - an `http://` ACME directory off loopback; a shared storage (which holds the private keys)
     reached without TLS off loopback, Redis without `tls`, etcd or Consul over `http://`; a
     Consul token or an etcd password sent over `http://` off loopback; Docker's API over
     `tcp://` off loopback;
   - the Kubernetes provider's API server over `http://` or with `insecure-skip-tls-verify` off
     loopback, said in the provider's log when the cluster is found.
4. **The certificate storage tightens its directories.** fit's file storage makes what it creates
   readable by its owner alone; akit's also tightens the root, and each directory under it that it
   writes into, to 0700 when its group or others had any permission (best effort: one akit's user
   does not own stays), and syncs the directory after each rename.
5. **Symbolic links stay followed, as fit's** (L7). The reference's `static` says so and what it
   means. An option to refuse links leading out of a root would be fit's first (decision 002): fit's
   media library has `follow_symlinks`, which a `static` field of the same name could follow.
6. **Proposed to fit, not built in akit** (decision 002): for L8, `tls.client_auth.crl_files`
   (PEM or DER CRLs, watched as certificate files), which akit would read onto akio-tls's
   `client_crls` with no akio change; for L9, `module_sha256` on the `wasm` middleware's params,
   the module refused at load when its SHA-256 differs.
7. **L12 is closed as it is**: fit's config has no field to read, and akio's 10 s and 60 s are the
   values fit's own servers use.

## Consequences

An operator, a script or the dashboard reading the config no longer sees inline secrets, and a
config read, edited and sent back keeps them. A client that wrote the literal string `[redacted]`
as a secret, or relied on reading secrets back through the API, behaves differently from fit's
Admin API; the compatibility page says so. The warnings make noise for configs that keep their
secrets open, which is the point; nothing a config fit runs is refused. L8 and L9 wait for fit.
