# 002: akit reads fit's config

[English](002-config-compatibility.md) | [简体中文](002-config-compatibility.zh-CN.md)

Status: accepted (2026-11-23), by the user before I02.

## Context

akit and fit do the same job on two engines. Their users, examples, templates, Admin API clients and
console pages could be shared, or could fork into two dialects that drift.

## Decision

- **fit's config schema is akit's.** Field names, defaults, meanings and the three formats (.conf,
  YAML, JSON with comments and trailing commas) are the same. So are the Admin API's paths and
  payloads, the CLI's commands and flags, metric names and access log fields.
- **Not yet supported means refused.** A field akit does not implement yet is an error naming it
  ("not supported yet: `<path>`"), never read and ignored. The list of such fields is generated
  into `docs/compatibility.md` by the compatibility gate (I07) and shrinks milestone by milestone.
- **fit leads the schema.** A new field is designed in fit first, or in both at once; akit does not
  add a field fit would not accept. Where akio allows something fib does not (an io_uring backend
  per listener, eBPF steering), the field is proposed to fit too, which may accept it and ignore it
  on fib with a documented note, or refuse it.
- **The gate is a test.** CI runs fit's templates and fit-examples, at a pinned fit commit, through
  `akit validate`. Moving the pin is a reviewed change.

## Consequences

Moving between fit and akit is a binary swap; docs, examples and tools serve both. akit cannot
improve the config on its own, and must follow fit's changes, which the gate makes visible.
