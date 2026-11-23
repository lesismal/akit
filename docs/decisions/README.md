# Decisions

[English](README.md) | [简体中文](README.zh-CN.md)

Short notes on decisions that shape akit: a dependency, a format, a design choice that later work
must follow. One file per decision, `NNN-title.md` with its Chinese twin `NNN-title.zh-CN.md`,
numbered in order and never renumbered. A decision that is replaced stays, with its status saying
which one replaces it.

Each note has a **Status** (proposed, accepted in an iteration, replaced by NNN), a **Context**, the
**Decision** and its **Consequences**. Where fit made the same decision, the note links fit's and
says whether akit follows it.

| No. | Decision | Status |
| --- | --- | --- |
| [001](001-dependency-policy.md) | Dependencies: akio, the standard library, and few others | accepted |
| [002](002-config-compatibility.md) | akit reads fit's config | accepted |
| [003](003-module-registry-and-traits.md) | The module registry and the data-plane traits | accepted |
| [004](004-graph-publication-and-retirement.md) | Publishing graphs, reading them per request, retiring them from the control plane | accepted |
