# akit 开发进度

[English](progress.md) | [简体中文](progress.zh-CN.md)

本文件按迭代记录实际完成的内容，以及下一次会话从哪里继续。[开发计划](dev-plan.zh-CN.md) 说明每个迭代应该做
什么；本文件说明实际做了什么，以及与计划不同的地方。

## 当前状态

| | |
| --- | --- |
| 最后更新 | 2026-11-23 |
| 里程碑 | M0 基础：I01 和 I07a 已完成。 |
| akio | akio `main` 上的 `50a82315`（`1.0.0-rc.1`），在 `Cargo.toml` 中按 `rev` 锁定 |
| fit | 作为规格；本计划基于 `7daee573` 阅读 |
| 可用功能 | 工作区能构建并通过 `make check`；`akit version` 和 `akit help` |
| 下一步 | I02 和 I03 进行中；I02 之后是 I07b（门禁接入 validate）。 |
| akio 需求 | A1–A8 现在全部加进 akio，各在 akio 的独立分支上进行（[akio API 需求](akio-api-requests.zh-CN.md)） |

## 日志

### 2026-11-23：I07a，兼容性门禁 I：夹具、字段清单、测试框架

- I07 拆分：本次完成的 I07a 不需要配置模型；I07b 在 I02 合并后把门禁接入 `akit_config::from_json_file`
  （[开发计划](dev-plan.zh-CN.md#i07b-兼容性门禁接入-validate)）。
- `tests/compat/fixtures/` 中的固定夹具：fit `7daee573`（`templates/` 共 30 个文件：三种格式各有单文件和含 9 个
  文件的 `conf.d` 目录；`schema/fit.schema.json`）和 fit-examples `9659d7e4`（74 个 `*/fit.conf`），附两份
  Apache-2.0 许可证、`tests/compat/SOURCE.md`，以及移动固定提交并重新生成以下全部内容的
  `tests/compat/update-fixtures.sh <fit> <fit-examples>`。
- 字段清单 `tests/compat/fit-fields.txt`：3,821 个字段路径（路径、部分、类型、标志），由
  `tools/compat-inventory.py`（Python 3，仅标准库，开发工具）从 fit 的 JSON Schema 生成；fit 由其配置类型
  和模块生成该 Schema，与其配置参考同源。模块参数按模块列出（`routes[].handler<respond>.params.status`），
  共 121 个部分。
- 测试框架：测试 crate `akit-compat`（`tests/compat`，工作区成员，无依赖）：枚举 80 个夹具（6 个模板、74 个
  示例），对每个运行校验函数，并按决策 002 判定；`examples/https` 与 fit-examples 自己的测试一样跳过。
  用替身校验函数测试。门禁本身（`gate_through_akit_config`）在 `compat-gate` feature 之后（该 feature 引入
  akit-config 依赖），在 I07b 之前标记为 `#[ignore]`；`make compat-gate` 用 fit-examples 的替身密钥运行它。
- `docs/compatibility.md`：门禁检查什么、如何解读和刷新，以及生成的全部字段表，目前全部"尚不支持"。
  之后的迭代在 `tests/compat/supported.txt`（目前为空）中标记已支持的字段。`make check` 和 CI 的 lint
  任务运行 `make compat-check`，清单或表格过期时失败。
- 检查：macOS 上 `make check`。没有引擎代码，无需在 Linux 后端上运行。
- 与计划不同之处：I07 拆分为 I07a 和 I07b；字段清单来自 fit 的 JSON Schema，而不是解析
  `docs/reference/config.md`（在 fit 中同源，且可机读）。示例中的非配置文件（证书、WebAssembly 插件源码）
  没有复制；由 I07b 决定校验会打开哪些文件。

### 2026-11-23：I01，工作区骨架和 CI

- Cargo 工作区：`akit` 可执行文件和[工作区结构](dev-plan.zh-CN.md#工作区结构)中的十五个库 crate，内容为空但有文档；
  edition 2024 和 MSRV 1.85 与 akio 一致；Apache-2.0。akio 在 `[workspace.dependencies]` 中按 `rev` 锁定
  （`50a82315`，akio 的 `main`），由 `akit-runtime` 使用，可执行文件会打印其中的 `AKIO_REV`。
- 没有 `unsafe`：工作区 lint 设 `unsafe_code = "forbid"`，每个 crate 也写 `#![forbid(unsafe_code)]`。为规则 15 开启
  clippy lint（每个库的 `unwrap_used`，以及 `expect_used`、`panic`、`indexing_slicing`、`arithmetic_side_effects`），
  测试中允许 unwrap（`clippy.toml`）。
- `deny.toml`：目前只允许 Apache-2.0 和 MIT；拒绝未知的 registry 和 git 来源（akio 除外）；禁止 tokio、async-std
  和 smol（规则 16）。
- `Makefile`：`check`（fmt、clippy、测试、cargo-deny、文档检查）、`test-changed`（`scripts/test-changed.sh`：改动的
  crate 及依赖它们的 crate）、`build`、`lint`。`scripts/check-docs.sh` 检查每份文档都有中文版，且两者在标题下方互相链接。
- CI（`.github/workflows/ci.yml`）在推送到 `main` 和 pull request 时运行：fmt、clippy 和文档检查；Linux（epoll，以及
  `AKIO_TEST_BACKEND=uring`）和 macOS 上的测试；`cargo +1.85 check`；cargo-deny。
- 可执行文件：`akit version` / `--version` 打印 `akit 0.0.1 (akio 50a823155a06)`；`help`；未知命令以 2 退出。
  `run` 和 `validate` 随 I05、I06 到来。
- 验证：macOS 上 `make check`；Docker 里的 Linux arm64 上 `cargo test --workspace`（普通与 `AKIO_TEST_BACKEND=uring`）
  以及 Rust 1.85 的 `cargo check`。CI 工作流本身要等这个分支推送后才第一次运行。目前还没有用到引擎，所以各后端暂时没有
  可测的东西。
- 与计划的不同：没有 `xtask`（`Makefile` 和两个脚本已够用）；`tests/e2e` 和 `bench` 由填充它们的迭代（I05、I08）创建。

### 2026-11-23：决定

- 用户采纳了[决定 001](decisions/001-dependency-policy.zh-CN.md)（数据面只依赖 akio 和 `std`，不用 tokio）和
  [决定 002](decisions/002-config-compatibility.zh-CN.md)（akit 读取 fit 的配置）；akit 与 fit 一样采用 Apache-2.0；
  A1–A8 现在就加进 akio。

### 2026-11-23：规划

- 写好路线图、开发计划、规则、架构、akio API 需求以及决定 001 和 002，中英文各一份。
- 未检验：akio API 需求是通过阅读 akio 的公开 API 发现的，而不是通过基于它构建；每一项都要在修改 akio 之前确认。
