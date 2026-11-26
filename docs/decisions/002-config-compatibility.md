# 002: akit reads fit's config

[English](002-config-compatibility.md) | [简体中文](002-config-compatibility.zh-CN.md)

Status: accepted (2026-11-23), by the user before I02; amended by [decision 037](037-akit-names.md):
the names akit shows to the outside (metrics, header fields, cookies, the plugin ABI, defaults that
name the gateway) are akit's, not fit's.

## Context

akit and fit do the same job on two engines. Their users, examples, templates, Admin API clients and
console pages could be shared, or could fork into two dialects that drift.

## Decision

- **fit's config schema is akit's.** Field names, defaults, meanings and the three formats (.conf,
  YAML, JSON with comments and trailing commas) are the same, but for the defaults that name the
  gateway (decision 037). So are the Admin API's paths and payloads, the CLI's commands and flags,
  and access log fields; metric names and header fields are fit's with `fit` renamed `akit`.
- **Not yet supported means refused.** A field akit does not implement yet is an error naming it
  ("not supported yet: `<path>`"), never read and ignored. The list of such fields is generated
  into `docs/compatibility.md` by the compatibility gate (I07a, I07b) and shrinks milestone by milestone.
- **fit leads the schema.** A new field is designed in fit first, or in both at once; akit does not
  add a field fit would not accept. Where akio allows something fib does not (an io_uring backend
  per listener, eBPF steering), the field is proposed to fit too, which may accept it and ignore it
  on fib with a documented note, or refuse it.
- **The gate is a test.** CI runs fit's templates and fit-examples, at a pinned fit commit, through
  `akit validate`. Moving the pin is a reviewed change.

## Consequences

Moving a config between fit and akit is a binary swap, given the names decision 037 renamed;
docs and examples serve both, and fit's tools take akit's names. akit cannot
improve the config on its own, and must follow fit's changes, which the gate makes visible.
