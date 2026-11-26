# Where the compatibility fixtures come from

[English](SOURCE.md) | [简体中文](SOURCE.zh-CN.md)

`fixtures/` holds copies of fit's config templates and of fit-examples' configs, pinned at the
commits below, for the [compatibility gate](../../docs/compatibility.md). They are copied, not
fetched when the tests run, so the gate runs offline and the same on every machine, and a change
in fit reaches akit only when the pin moves.

| Repository | Commit | What is copied |
| --- | --- | --- |
| fit | `7daee573b102d87837cc10f033e4cc5894769210` | `templates/` (`.conf`, YAML and JSON, each as one file and as a `conf.d` directory), `schema/fit.schema.json`, `LICENSE` |
| fit-examples | `9659d7e4b30a6f97d908536c85c2b2cf5b75fe8d` | every `*/fit.conf`, `LICENSE` |

`fixtures/PINS` holds the same two hashes for tools and tests. The console's pages, fit's
`app/workbench/web` at the same fit commit, are copied by `tools/console-pages.py` into
`crates/akit-admin/console` (I91), which the script runs too.

- fit: <https://github.com/lesismal/fit>
- fit-examples: <https://github.com/lesismal/fit-examples>

## Licence

Both repositories are licensed under the Apache License, Version 2.0, as akit is. Their licence
files are kept beside the copies: [fixtures/fit/LICENSE](fixtures/fit/LICENSE) and
[fixtures/fit-examples/LICENSE](fixtures/fit-examples/LICENSE). The files are copied unmodified;
neither repository has a NOTICE file.

## What is left out

From fit-examples, only the configs: an example's other files (Docker Compose files, Kubernetes
manifests, data, the Go sources of its WebAssembly plugins) are not fit configs. fit-examples' own
test also sets stand-in secrets in the environment, builds the WebAssembly plugins some examples
load, and only adapts the `https` example, whose certificate files are the user's; the gate's
equivalents are in `src/lib.rs` (`EXAMPLE_SECRETS`, `NO_VALIDATE`) and `make compat-gate`.

## Moving the pin

```bash
tests/compat/update-fixtures.sh <fit-commit> <fit-examples-commit>
```

The script clones both repositories at the given commits (`FIT_REPO` and `FIT_EXAMPLES_REPO` may
name local clones), replaces `fixtures/`, rewrites `fixtures/PINS` and the table above in both
languages, and regenerates the field inventory (`fit-fields.txt`) and the tables of
`docs/compatibility.md`. Moving the pin is a reviewed change (decision 002): commit it on its own,
and read the diff of `fit-fields.txt` in review, since every field fit added is a field akit now
refuses until it supports it.
