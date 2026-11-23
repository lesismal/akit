# 005: YAML is read by a reader of akit's own

[English](005-yaml-reader.md) | [简体中文](005-yaml-reader.zh-CN.md)

Status: accepted in I24 (2026-11-24).

## Context

fit reads a config written in YAML as it reads JSON ([fit's decision 019](https://github.com/lesismal/fit/blob/main/docs/decisions/019-yaml-config.md),
`adapter/yaml`): the YAML says what the JSON would, key for key. A mapping is an object, a
sequence an array, a scalar the JSON value its YAML 1.2 tag says; anchors, aliases and merge keys
(`<<`) are resolved, with bounds on what aliases expand to (2^20 nodes, 64 MiB, 256 levels); a
key given twice, a key that is not a scalar, a tag fit's config does not know (`!custom`), a
number JSON cannot hold (`.inf`) and a second document are refused; an empty file is an empty
config; errors point at a line and column of the YAML. fit parses with go.yaml.in/yaml/v3, a port
of libyaml, and walks its node tree.

akit must read the same files into the same model ([decision 002](002-config-compatibility.md)),
and [decision 001](001-dependency-policy.md) left the choice to I24: a reader of akit's own for
the subset fit accepts, or a crate, weighed by size, `unsafe`, strictness and dependency count.

The crates, at the time of writing:

- **serde_yaml** is no longer maintained; it and its forks (serde_yml) parse with unsafe-libyaml,
  libyaml translated mechanically to Rust, which is `unsafe` throughout, and bring serde,
  indexmap, itoa and ryu. They decode into values that keep no positions.
- **yaml-rust2** (the maintained fork of yaml-rust) and its successor **saphyr** are pure Rust,
  written from the YAML 1.2 specification, with an event API that has positions. They bring a few
  crates of their own (ring buffers, hash maps, encodings, some with `unsafe`); their loaders keep
  one of two duplicate keys, so duplicates would be checked over the events.

None of them is fit's library: each reads YAML's corners (tabs, `:` and `?` in flow collections,
anchors' names, tags' characters, which scalars are numbers) its own way, and matching fit means
writing the resolution of scalars, the merge keys, the bounds and the errors over the crate
anyway, then checking it against fit.

## Decision

- **akit reads YAML with a reader of its own**, `akit-config`'s `yaml` module: no dependency, no
  `unsafe` (the crate is `#![forbid(unsafe_code)]`), no panic on any input (fuzzed, target
  `config_yaml`), linear in the input's length. It produces the same tree with byte offsets as
  the JSON reader, so the model reads both alike and every problem has a line and column in the
  YAML: a config's error names the line and column of the value it is about, as fit's do.
- **It reads what fit's library reads**, not only the subset the templates use: block and flow
  collections, plain, quoted, literal and folded scalars with YAML's folding and chomping,
  explicit keys (`? k`), anchors, aliases and merge keys, the core schema's tags and verbatim
  `!<tag:yaml.org,2002:…>` tags, `%YAML` directives, `---` and `...`. Scalars are resolved as fit's
  library resolves them (`0x1F`, `0o17`, `017`, `1_000`, `.5`, timestamps as strings, `yes`/`on`
  as strings), and numbers are written as fit's adapter writes them for its JSON decoder, so the
  model's checks see what fit's see.
- **What fit does, akit does**: the same refusals (above), the same bounds on what aliases and
  merge keys expand to, an empty file as an empty config.
- **Checked against fit, not only against the specification.** `tools/yaml-diff/run.sh` builds
  fit's adapter as a command (Go, offline, fit beside akit), has it and akit judge a corpus
  (hand-written cases, every YAML file and YAML block of fit's repository and docs, the templates
  with their commented-out fields written out, and random edits of them) and lists every case
  read differently. At I24, runs of some 470,000 cases in all were brought down to 2 cases read
  differently in the last run of 150,000, both libyaml oddities (a tab between `-` and a comment,
  an empty explicit key in a flow sequence) which akit refuses and fit accepts. A sample of the corpus with fit's verdicts is checked in
  (`crates/akit-config/testdata/yaml-fit.txt`) and tested on every run.
- **What akit refuses that fit reads**, each with an error, never read otherwise: `%TAG`
  directives and named tag handles (`!e!x`), "not supported yet"; a lone carriage return as a
  line break; collections nested more than 128 deep (fit allows 256), the JSON reader's bound,
  which keeps the parser's recursion small; the 2 corners above; an explicit empty document (`---` alone) is an empty config to
  both, read by akit as `{}` where fit's adapter writes `null`.

## Consequences

The reader is about 1,700 lines of code, which akit maintains and fuzzes, against no
dependency to vet, follow or keep off the data plane. It is read on the control plane only, when
a config is loaded. A difference from fit is found by running the differential check, which needs
Go and a fit checkout; the checked-in sample keeps the cases it holds from regressing without
them. `akit adapt` (I73) can write YAML back with the same rules.
