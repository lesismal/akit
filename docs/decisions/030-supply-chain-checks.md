# 030: Supply-chain checks: cargo-deny, cargo-audit daily, and the release binary's graph

[English](030-supply-chain-checks.md) | [简体中文](030-supply-chain-checks.zh-CN.md)

Status: accepted in I80 (2026-11-26).

## Context

I80 asks for `cargo audit` and `cargo deny` clean before the external security review. akit ran
cargo-deny in CI since I01 (`deny.toml`: advisories, bans, licenses, sources), on pushes and pull
requests only, so an advisory published against a locked crate went unseen until the next change.
cargo-audit did not run at all.

Both tools read RustSec, which lists crates from crates.io. akit's release binary links none:
`cargo tree -e normal,build -p akit` reaches the akio crates alone, from akio's repository at the
pinned commit ([decision 001](001-dependency-policy.md), rule 22). The registry crates in
`Cargo.lock` (30 at I80) are development dependencies (`stats_alloc`) and akio's loom models
(`loom` and what it brings, behind akio's `cfg(loom)`, which only its loom tests set). So a clean
audit says nothing about the code the binary runs: that is akio's, and akio runs cargo-deny,
cargo-audit, its unsafe inventory, Miri, loom, sanitizers and fuzzing in its own CI
([akio's security policy](https://github.com/lesismal/akio/blob/main/SECURITY.md)). What akit's
checks can hold is that the binary stays that way.

fit's counterpart is `govulncheck` over its module graph; fit has no separate check that its
binary's dependencies stay few, since Go's are reviewed with its `go.mod`.

## Decision

- **A workflow of its own, `.github/workflows/security.yml`**, on every push to `main`, every pull
  request, daily at 04:17 UTC and by hand: `cargo deny check` (all four checks, as CI's `deny` job
  runs them), `cargo audit -D warnings` (yanked, unmaintained and unsound crates fail too), and
  `tools/release-deps.py`. The daily run is what catches a new advisory without a change to akit.
- **No advisory is ignored.** An exception, when one is needed, is written into both `deny.toml`'s
  `[advisories] ignore` and `.cargo/audit.toml` (which does not exist until then), each with its
  reason and the crate's path to akit, as akio does.
- **`tools/release-deps.py` holds decision 001 for the binary**: it walks `cargo metadata`'s
  resolved graph from `akit` along normal and build edges, every target's `cfg` included (a
  dependency only another OS links still counts) but test-only cfgs (`cfg(loom)`), and fails on a
  registry crate its `ALLOWED` list does not name. A crate allowed there names the decision note
  that accepted it. At I80 the list is empty: 40 packages, all akit's or akio's.
- **`deny.toml`'s license list keeps Apache-2.0** although no crate is counted under it today
  (every dual-licensed crate is counted as MIT), with `unused-allowed-license = "allow"`, so the
  run is clean and a future Apache-2.0-only crate needs no change to the list.
- **akio's advisories reach akit through the pin**: akio publishes its own (GitHub Security
  Advisories and RustSec), and akit moves its `rev` to the fixed commit (rule 22), saying so in its
  own advisory ([SECURITY.md](../../SECURITY.md)).

## Consequences

- An advisory against a development dependency fails the daily run although the binary is not
  affected; it is fixed by updating the lock file, or ignored with its reason as above. That is
  the cost of a clean, unconditional gate.
- A pull request that adds a registry crate to the binary fails until the crate has a decision
  note and an `ALLOWED` entry: the exception decision 001 asks for is now checked, not only
  reviewed.
- Neither tool sees a vulnerability in akio's code; the external review's scope
  ([review scope](../security/review-scope.md)) says what of akio it covers.
