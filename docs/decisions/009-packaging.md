# 009: Packaging: static musl binaries linked by rust-lld, fit's image, chart and units

[English](009-packaging.md) | [简体中文](009-packaging.zh-CN.md)

Status: accepted in I65 (2026-11-25).

## Context

I65 packages akit as fit is packaged ([fit's deploy directory](https://github.com/lesismal/fit/blob/main/deploy/README.md),
[fit's decision 016](https://github.com/lesismal/fit/blob/main/docs/decisions/016-container-image.md)):
release archives with checksums, a container image on distroless's static image, a Helm chart and
systemd units. fit gets static binaries for free from Go with cgo off. A Rust binary links the C
library of its target: glibc's on `*-linux-gnu`, which is dynamic, or musl's on `*-linux-musl`,
which Rust links in statically. akit and akio are Rust alone (no `cc`, no C dependency on any
target akit builds for), so the musl targets need no C toolchain for akit's code; only the final
link needs a linker that knows the target.

## Decision

- **Linux binaries are static, from Rust's musl targets** (`x86_64-unknown-linux-musl`,
  `aarch64-unknown-linux-musl`), as the dev plan's I65 asks. They run on any distribution, and on
  distroless's static image, which has no C library.
- **They are linked by `rust-lld`**, the linker rustup ships with every toolchain, with the C
  runtime Rust ships for the musl targets. Any host (Linux or macOS, x86-64 or arm64) builds both
  Linux targets with no cross toolchain, `cross`, zig or emulation, and so does the image's builder
  stage, run on the builder's own platform.
- **macOS binaries** (`x86_64-apple-darwin`, `aarch64-apple-darwin`) are built on macOS with Xcode's
  linker, as every macOS program is, and link its system libraries. Windows and FreeBSD, which fit
  ships, wait until akio runs there.
- **The release profile is unchanged**: `lto = "thin"`, one codegen unit, symbols kept, so a panic's
  backtrace names its functions and the benchmark host's profiles read the same binary a release
  ships. The archives are a little larger for it (about 5 MB each).
- **The version** a release prints comes from `AKIT_VERSION` at build time (`option_env!`), as fit's
  `-X main.version`; a build without it prints the crate's version.
- **The image, chart and units are fit's**, with akit's names and paths (`/etc/akit/akit.json`,
  `/var/lib/akit`, the user `akit`), and the chart's controller `lesismal.github.io/akit` rather than
  fit's, so the two may serve their own classes in one cluster.

## Consequences

- musl's memory allocator is slower than glibc's under many threads. akit sets no global allocator
  (one in C would be a new dependency, refused by [decision 001](001-dependency-policy.md) unless a
  measurement makes the case), so the static binaries use musl's. Whether that costs akit
  throughput is for a benchmark task: the fixed and proxy scenarios with the musl binary against a
  glibc build of the same commit (the dev plan's benchmark table has the row).
- Nothing is signed: the image has no cosign signature or provenance, the archives only
  `checksums.txt`, and the macOS binaries are not notarized. Each needs a key or an identity of the
  maintainer's, written up in [deploy/README.md](../../deploy/README.md).
