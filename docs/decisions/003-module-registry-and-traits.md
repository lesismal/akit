# 003: The module registry and the data-plane traits

[English](003-module-registry-and-traits.md) | [简体中文](003-module-registry-and-traits.zh-CN.md)

Status: accepted in I03 (2026-11-23).

## Context

fit's `module` package registers modules from Go `init` functions into a global map, builds a
module by reflection (a zero struct whose exported fields are decoded from the params), and finds
out what a module can do by type assertions (`Handler`, `Provisioner`, `Cleaner`, `Drainer`,
`Inline`, …). Its handler is fib's handler; its middleware wraps a handler once, when a route is
compiled. Rust has no `init`, no reflection and, at akit's MSRV (1.85), no upcasting from one trait
object to another; akit's rules ask for no global mutable state, no panics on config input, and a
data plane that runs inline on akio's I/O threads.

## Decision

- **The data-plane traits are akio's** (`akit_runtime::traits`). `Handler` is an akio-http
  `Service` (`call`, `body_mode`, `streams_bodies`), as fit's handler is fib's; it runs on the I/O
  thread, responds or detaches, and never blocks. `HandlerService` hands one to akio. `Middleware`
  is fit's: `wrap(next) -> handler`, called once at compile time (rule 12); `chain` builds a route's
  chain, first middleware outermost, and `AkioMiddleware` adapts akio's per-request middleware.
  `Matcher`, `Router`, `Service` (fit's: picks an upstream, not akio's `Service`), `Upstream` and
  `Listener` read a request's akio `Context` or give an akio `AnyAcceptor`; `Service`, `Upstream`
  and `Listener` are provisional, as fit's are, until I05, I14 and I19.
- **Registration is a `static` list per crate.** A crate ships `static MODULES: &[Info<P>]`, each an
  ID (`http.handlers.respond`), a doc line and a typed constructor; the binary builds one
  `Registry` from the lists it links and passes it to compile. No global, no `inventory`/`linkme`.
  A malformed ID, an ID given twice, or an ID outside its kind's namespace is a `RegistryError`
  (fit panics), which a test of the binary's registry catches.
- **The namespace is the kind.** `http.handlers` holds handlers and `http.middleware` middleware;
  the constructor's type says which (`Constructor::Handler`/`Middleware`), so fit's
  "is not a handler" error cannot happen. New kinds (L4 handlers, …) add a `Kind` and a namespace.
- **The constructor decodes and provisions in one step**, `fn(&Provision<P>) ->
  Result<Provisioned<T>, ProvisionError>`. `P` is the params type: akit-runtime does not know the
  config model. I04 sets `P` to akit-config's params value and turns a `ProvisionError` (a message
  and maybe a field) into a config error at the params' path, as fit's `Load` does.
- **Cleanup is a closure the constructor attaches** (`Provisioned::on_cleanup`), recorded in the
  config's `Modules`, which runs the cleanups last provisioned first, each once, when released or
  dropped. A compile that fails drops its `Modules`, cleaning up what it had provisioned. Cleanup
  may run on an I/O thread (the last holder of a graph releases it, I04), so it never blocks.
- **Errors keep fit's wording**: `there is no module "nope" in http.handlers (it has "proxy",
  "respond")`, and a provisioning error's own message; the path and position are the caller's.

## Consequences

- Every module crate exports its `static` list, and `akit build` (I70) composes lists instead of
  imports. Since I70 the `akit` crate is a library whose `akit::main(version, lists)` the
  binary, and every binary a build generates, calls with its lists ([modules](../modules.md)); the
  config model keeps a module it does not know as `Handler::Module`, its params as written, so the
  registry reports an unknown name at compile time with fit's message, as fit's `module.Load`
  does, and a module built in decodes its own params.
- fit's optional interfaces become provided methods on the traits (`Handler` can grow `inline`,
  `unwrapped`, …) or fields on `Provisioned` (a drain hook, I17), not type assertions.
- `Env` is empty; I04 adds what fit's `module.Env` holds (upstream pools, the engine, the graph
  hold) as the iterations that need them come.
- akit-runtime depends on `akio-http` (same akio commit as `akio`), for `Service`, `Context` and
  `Middleware`.
