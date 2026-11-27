# Modules and `akit build`

[English](modules.md) | [简体中文](modules.zh-CN.md)

akit's handlers and middleware are modules: each has an ID in its kind's namespace
(`http.handlers.respond`, `http.middleware.headers`) and a constructor the registry calls when a
config is compiled ([decision 003](decisions/003-module-registry-and-traits.md)). `akit build`
builds an akit binary with the modules of other crates besides its own, or with fewer of its own,
as fit's `fit build` does with Go modules and xcaddy does for Caddy.

## Writing a module

A module crate is a Rust library that depends on `akit-runtime` and exports, at its root, the list
of its modules:

```rust
pub static MODULES: &[Info<HandlerParams>] = &[Info::middleware(
    "http.middleware.hello",
    "Adds X-Hello to every response of its routes.",
    new,
)];
```

Rust has no `init`: nothing registers itself. The `main` that `akit build` generates passes each
crate's `MODULES` to akit's registry, which refuses an ID given twice or outside its kind's
namespace when the binary starts.

The example in
[`crates/akit/tests/testdata/hello-module`](../crates/akit/tests/testdata/hello-module) is a
complete module, the one `akit build`'s test builds in. Its parts:

- **`Cargo.toml`** depends on `akit-runtime` alone. `akit-runtime` re-exports what a module needs
  from the rest: `akit_runtime::akio_http` (a handler is an akio-http `Service`) and
  `akit_runtime::config` (the params). A crate of your own depends on akit's git repository at the
  revision you build for:

  ```toml
  [dependencies]
  akit-runtime = { git = "https://github.com/lesismal/akit", rev = "<commit>" }
  ```

  A build from a directory of akit replaces that repository with the directory (Cargo's
  `[patch]`), so a module and the akit it is built into always share one `akit-runtime`. A build
  from a revision of akit uses that revision: give your crate the same one.
- **The constructor**, `fn(&Provision<HandlerParams>) -> Result<Provisioned<dyn Middleware>,
  ProvisionError>` (or `dyn Handler` for a handler), runs once, when the config is compiled. It
  checks the params and builds everything serving needs; requests only read what it built
  (rules 12 and 13).
- **The params.** akit's config model knows akit's own modules; for any other name it keeps the
  reference as `config::Handler::Module(ModuleRef { name, middleware, params })`, the params as
  written (`config::value::Value`: objects keep their members' order, numbers are kept as written).
  The constructor decodes them, as strictly as akit decodes its own: a field it does not know is an
  error. `ProvisionError::field("greeting", "…")` reports one at the field's path, as akit reports
  its own (`routes[0].middleware[0].params.greeting: …`).
- **The handler** runs on an akio I/O thread for every request, so it never blocks (rule 1): no
  disk or network I/O, no waiting, no lock held for long. Work that can only block goes to akio's
  logical pool with `spawn_blocking`. A middleware's `wrap` is called once per route, when the
  route is compiled, and returns the handler that runs in front of the next.
- **No panics** on what a client or a config sends (rule 15): an error is a value, as in akit.
  akit forbids `unsafe` in its own crates; a module should too.

A module may not take a name fit has and akit does not support yet (`hls`, `wasm` and the rest):
akit's config model refuses those as "not supported yet". A name akit's binary has no module for
is reported when the config is compiled, as fit reports it: `routes[0].handler.module: there is no
module "nope" in http.handlers (it has "fastcgi", …)`.

## Building

```bash
akit build -with akit-hello=crates/akit/tests/testdata/hello-module -o akit-hello
```

This generates a Cargo package (a `main.rs` and a `Cargo.toml`), builds it in release mode with
the `cargo` found on `PATH` (or named by `CARGO`), and writes the binary to `-o`. The binary is akit,
with every command, and its registry has the modules of the crates named with `-with` besides
akit's own; `akit version` says the version it was built as.

| Flag | |
| --- | --- |
| `-o file` | the binary to write; `akit` when left out |
| `-with crate` | a crate to build in, repeated for each: `name`, `name@version` (crates.io), `name=dir` for one whose source is in `dir`, or `name=git+url#reference` for one in a git repository (a commit, a tag `v…` or a branch; its default branch without `#`) |
| `-without crate` | leave out one of akit's own module crates, repeated for each: `akit-handler` (`respond`, `static`, `fastcgi`, `forward_proxy`), `akit-middleware` (every middleware but `cache`), `akit-cache` (`cache`), `akit-proxy` (`proxy`), `akit-webrtc` (`whip`, `whep`, `webrtc_proxy`) or `akit-bittorrent` (`bt_tracker`) |
| `-akit v` | the akit to build on: a revision of `github.com/lesismal/akit` (a commit, or a tag `v…`), `git+url#reference` for another repository of it, or a directory holding its source (a checkout's root or its `crates/akit`); the source this akit was built from when left out |
| `-keep dir` | generate the package in `dir` and keep it there; otherwise a temporary directory is used and removed |

A build from a directory of akit uses that checkout's `Cargo.lock`, so it builds with the
versions akit is tested with. Cargo's own settings apply: `CARGO_TARGET_DIR` keeps the compiled
crates between builds (a first build compiles all of akit and akio; a later one with the same
directory, little), and `CARGO_BUILD_TARGET` builds for another system (the binary is then in
that target's directory, and `-o` gets it).

The exit codes are akit's: 0 when the binary was written, 1 when the build failed (cargo's messages
say why), 2 when the flags were wrong.

## What is not here yet

- Modules of other kinds: providers, layer 4 handlers, matchers. akit's providers are built in,
  and `-without` leaves out module crates only (fit's `-without` takes its provider and app
  packages too).
- WebAssembly plugins (I71, I72), which load modules without a build.
