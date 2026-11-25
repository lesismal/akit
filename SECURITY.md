# Security policy

[English](SECURITY.md) | [简体中文](SECURITY.zh-CN.md)

akit is a reverse proxy, TLS terminator and API gateway: it faces the internet, holds private keys
and forwards credentials. This page says how to report a security problem, which versions get
fixes, and what akit does to keep its own code and its dependencies in check. What akit defends
against, and how, is in the [threat model](docs/security/threat-model.md); what the external
review of 1.0 covers is in the [review scope](docs/security/review-scope.md).

## Supported versions

akit is before 1.0 (`0.0.x`). Until 1.0.0 is released, only the latest commit on `main` receives
fixes; there are no releases to back-port to.

From 1.0.0 on:

| Version | Fixes |
| --- | --- |
| The latest minor release of the current major version | every security fix, as a patch release |
| The minor release before it | security fixes of high or critical severity, for 3 months after the newer minor release |
| Anything older | none: upgrade (a 1.x upgrade keeps configs and Admin API clients working, [compatibility](docs/compatibility.md#v1-what-stays-stable)) |

A fix that has to tighten a default limit or change a default to close a hole may do so in a
1.x release; the release notes say so ([what a 1.x release may do](docs/compatibility.md#what-a-1x-release-may-do)).

## Reporting a vulnerability

**Please do not open a public issue, pull request or discussion for a security problem.**

Report it privately through GitHub's
[private vulnerability reporting](https://github.com/lesismal/akit/security/advisories/new)
("Security" tab → "Report a vulnerability"). Include:

- the akit version or commit, the OS and kernel, and the I/O backend (epoll, io_uring, kqueue);
- the smallest config that shows it (with secrets replaced), and the request, connection or file
  that triggers it: a `curl` line, a script, a packet capture or a test case;
- what an attacker gains: memory or data disclosure, a request smuggled or routed past a check,
  authentication bypassed, a limit bypassed (denial of service within the configured limits), a
  panic or a hang of an I/O thread, code run in or out of the plugin sandbox;
- whether you want to be credited, and under which name.

A problem in akio itself (HTTP/1, HTTP/2, HTTP/3 or QUIC parsing, TLS, X.509, the crypto, the
WebAssembly interpreter, the I/O backends) is akio's: report it through
[akio's policy](https://github.com/lesismal/akio/blob/main/SECURITY.md). If you are not sure which
project it belongs to, report it to akit and we will move it.

## What to expect

| Step | Target |
| --- | --- |
| Acknowledgement of the report | within 3 working days |
| First assessment (accepted, needs more information, not a vulnerability) | within 7 days |
| Fix or mitigation of an accepted report | within 30 days for high and critical severity, best effort otherwise |
| Public advisory | when the fix is released, coordinated with the reporter |

Accepted issues are published as GitHub Security Advisories. akit follows coordinated disclosure:
please give us a reasonable time to ship a fix before publishing details (90 days by default,
shorter if the issue is already being exploited). A fix in akio reaches akit when akit moves its
pinned akio commit ([rule 22](docs/rules.md)); the advisory names both.

## Scope

In scope: akit's code (`crates/*`), the release binaries and the container image it builds
(`scripts/dist.sh`, `Dockerfile`), and the default config of each, as they behave on a supported
platform. In particular:

- a request, connection, datagram or TLS handshake from a client that crashes akit, hangs or
  blocks an I/O thread, or gets past a limit the config sets or a default sets;
- a request routed, forwarded or answered in a way that bypasses a route's middleware
  (authentication, access checks, limits), or that the upstream reads differently from akit
  (request smuggling, header injection);
- the Admin API or dashboard reachable, or changing the config, without what its config asks for
  (loopback, a token);
- private keys, ACME account keys, tokens or passwords disclosed: in answers, logs, metrics,
  `GET /config`, error messages or files written with loose permissions;
- a WebAssembly plugin reaching beyond its memory, its time budget or the host functions it is
  given;
- a provider (Docker, Kubernetes, Consul, etcd) or a shared certificate store whose answers make
  akit crash or exceed its bounds.

Out of scope:

- denial of service that needs more resources than the configured limits allow, or a config that
  opts out of a limit;
- an operator who can already write akit's config, its files or its environment (the config is
  trusted: it can proxy anywhere, read files the process can read, and load plugins);
- the benchmark harness and tools under `bench/` and `tools/`, the fuzz targets under `fuzz/`, the
  test servers of `tests/`, and the deployment examples under `deploy/` beyond what they install
  by default;
- vulnerabilities in akio (report them to akio, above) or in a development dependency that never
  reaches the release binary;
- missing hardening that the [threat model](docs/security/threat-model.md#known-limitations)
  already lists as a known limitation with its plan.

## Hardening already in place

- **No `unsafe` in akit**: every crate is `#![forbid(unsafe_code)]` ([rule 14](docs/rules.md)).
  `unsafe` lives in akio only, behind its
  [unsafe review](https://github.com/lesismal/akio/blob/main/docs/unsafe-review.md).
- **One dependency in the release binary**: akio, pinned by commit hash
  ([rule 22](docs/rules.md)); no crate from crates.io is linked into it
  ([decision 001](docs/decisions/001-dependency-policy.md),
  [decision 030](docs/decisions/030-supply-chain-checks.md)). akio's TLS, X.509 and crypto are its
  own, with their constant-time checks
  ([akio's constant-time page](https://github.com/lesismal/akio/blob/main/docs/constant-time.md)).
- **`cargo deny check` and `cargo audit -D warnings`** in CI on every push and pull request, and
  daily ([`.github/workflows/security.yml`](.github/workflows/security.yml)): RustSec advisories,
  yanked and unmaintained crates, licenses, sources (crates.io and akio's repository only) and
  banned crates (no async runtime).
- **No panics on peer or config input** ([rule 15](docs/rules.md)), hunted by fuzzing: a target
  for every parser akit owns, with its corpus replayed by `cargo test`.
- **Every resource bounded with a safe default** ([rule 11](docs/rules.md)), every exchange with the
  outside timed ([rule 10](docs/rules.md)), and nothing blocking an I/O thread
  ([rule 1](docs/rules.md)), with akio's slow-callback detector on in every test run.
