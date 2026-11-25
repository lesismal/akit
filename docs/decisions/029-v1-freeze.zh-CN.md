# 029：v1 冻结：基线、标记的改动和两项检查

[English](029-v1-freeze.md) | [简体中文](029-v1-freeze.zh-CN.md)

状态：在 I79 中采纳（2026-11-26）。

## 背景

fit 在其 I77 中把配置 schema 和 Admin API 冻结为 v1（[fit 的政策](https://github.com/lesismal/fit/blob/main/docs/compatibility.zh-CN.md)）：
一份成文的政策、`schema_version`，以及读取改名字段的旧名并给出警告。它的 schema 由 Go 类型生成，因此配置的改动会在评审时体现在
schema 的 diff 中。akit 的配置就是 fit 的配置，由手写的解码器读取，Admin API 的应答也是手写的：如果没有检查，破坏 1.x 承诺的改动
不会显现出来。

## 决定

- **v1 的范围在冻结时一次写定**，放在 [tests/compat/v1](../../tests/compat/v1)：配置的范围（fit 的 schema 版本 1 的每个字段及其
  类型、必填性和默认值、枚举的取值、弃用的旧名、akit 读取的字段、akit 的扩展字段）由 `tools/v1-freeze.py --write-baseline` 生成；
  Admin API 的范围（端点、头字段、查询参数、变更、错误和 `GET /versions` 应答的成员）手工写成。`tools/v1-freeze.py` 保存两个文件的
  SHA-256：修改基线意味着新版本。
- **此后的每项改动都要标记**在 `additions.txt` 中：兼容的增加标为 `added`，政策允许 1.x 版本改变的项（为安全收紧的默认值）标为
  `retired` 并写明原因。`make v1-bless` 标记配置的增加项，从不标记破坏性的改动。
- **两项检查**：CI 的 lint 任务中的 `tools/v1-freeze.py` 从 fit 的固定 schema 和 `supported.txt` 构建配置的范围，遇到任何未标记的差异
  即失败；akit-admin 的 `v1_surface_is_frozen` 在每个测试任务中运行 Admin API，并按其范围逐一探测。

## 影响

- 让 akit 开始读取某个字段的迭代，或带来 fit 新字段的固定提交移动，要在同一个提交中运行 `make v1-bless`；评审时能在
  `additions.txt` 中看到这项增加。
- Admin API 的检查只能在分发器以字符串字面量写出新路径时发现它（它读取 `server.rs` 的 `call`），并只检查三种应答的成员，不检查
  `/status`、`/config/effective` 或 `/cache` 的；这些仍由它们自己的测试和评审把关。
- 并行增加支持字段的分支各自标记自己的增加项；合并时把这些行拼接起来即可。
