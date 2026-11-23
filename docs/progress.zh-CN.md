# akit 开发进度

[English](progress.md) | [简体中文](progress.zh-CN.md)

本文件按迭代记录实际完成的内容，以及下一次会话从哪里继续。[开发计划](dev-plan.zh-CN.md) 说明每个迭代应该做
什么；本文件说明实际做了什么，以及与计划不同的地方。

## 当前状态

| | |
| --- | --- |
| 最后更新 | 2026-11-23 |
| 里程碑 | M0 基础：I01、I02、I03 已完成。 |
| akio | akio `main` 上的 `50a82315`（`1.0.0-rc.1`），在 `Cargo.toml` 中按 `rev` 锁定 |
| fit | 作为规格；本计划基于 `7daee573` 阅读 |
| 可用功能 | 工作区能构建并通过 `make check`；`akit version` 和 `akit help`；`akit_config::from_json` 能读取 fit 的监听器、路由和 `respond` handler，fit 配置的其余部分按名字报为暂不支持；模块注册表和数据面 trait（`akit-runtime`） |
| 下一步 | I04，把 I02 的配置模型与 I03 的注册表接起来；I07b，把兼容性门禁接到 `validate`。 |
| akio 需求 | A1–A8 现在全部加进 akio，各在 akio 的独立分支上进行（[akio API 需求](akio-api-requests.zh-CN.md)） |

## 日志

### 2026-11-23：I03，核心 trait 与模块注册表

- `akit_runtime::traits`，建立在 akio 的回调模型上：`Handler`（akio-http 的 `Service`：在 I/O 线程上内联运行，应答或
  detach）、`HandlerService`（把 handler 作为 akio 的 `Service`）、`Middleware`（与 fit 一样，在编译时 `wrap(next)`
  一次）以及 `chain` 和 `AkioMiddleware`（把 akio 的逐请求 middleware 作为 akit 的）、`Matcher`、`Router`、`Service`
  （fit 的含义：挑选 `Upstream`）、`Upstream` 和 `Listener`（名字和 akio 的 `AnyAcceptor`）；后三者与 fit 一样是暂定的。
- `akit_runtime::module`：`Info`（ID、说明、带类型的 `Constructor`），`Registry::new` 由各 crate 的 `static` 列表构建
  （无全局状态；格式错误、重复或放错命名空间的 ID 是 `RegistryError`），`lookup`、`namespace`，以及按配置名加载的
  `handler` 和 `middleware`，带 `Provision`（参数为泛型 `P`、`Env`）；`Provisioned::on_cleanup`；`Modules` 在
  `release` 或丢弃时按与 provision 相反的顺序运行清理，每个一次。错误沿用 fit 的措辞：`there is no module "nope" in
  http.handlers (it has "echo", "respond")`；`ProvisionError` 带一条消息，可能还有一个字段，由 I04 放到参数的路径上。
- [决定 003](decisions/003-module-registry-and-traits.zh-CN.md) 记录了之后的迭代要遵循的设计。
- 验证：在 macOS 上 `make check`（fmt、clippy `-D warnings`、测试、cargo-deny、文档）；
  `RUSTDOCFLAGS=-D warnings cargo doc -p akit-runtime`。测试（`crates/akit-runtime/tests/modules.rs`）：测试模块注册、
  由参数 provision，并在 release 和丢弃时清理；未知名字的错误会指出该名字；中途失败（构造函数出错或名字未知）会按相反
  顺序清理已 provision 的模块；注册表错误；以及 provision 出的 middleware + handler 链在 akio 上应答真实请求
  （epoll/kqueue；Linux 上可用 `AKIO_TEST_BACKEND=uring`）。本次会话未在 Linux 上运行。无基准测试（控制面）。
- 与计划的不同：akit-runtime 现在依赖 `akio-http`（同一个 akio `rev`），以使用 `Service`、`Context` 和
  `Middleware`。参数类型是泛型而非 akit-config 的类型，因为 I02 在并行编写：由 I04 设定。`Env` 在 I04 之前为空。
  fit 的可选接口（`Drainer`、`Inline`、`Unwrapped`、`NeedsAuthentication`、`Holder`）留给需要它们的迭代。

### 2026-11-23：I02，配置模型 v0 与严格的 JSON 读取器

- `akit-config`：akit 自己的 JSON 读取器（决定 001），记录每个值的位置：RFC 8259，加上 fit 读取 JSON 时允许的 `//` 和
  `/* */` 注释以及值后面的尾逗号；拒绝重复的键、控制字符、孤立代理项、前导零、嵌套超过 `MAX_DEPTH`（128）以及超过
  `MAX_CONFIG_BYTES`（64 MiB）的文档。
- 模型 v0，使用 fit 的字段名、默认值和检查：`schema_version`（1）；`listeners`（`name`，按 Go 的 `SplitHostPort` 解析的
  `address`，`protocols` 支持 `http/1.1` 和 `h2c`，fit 的默认值是两者）；`routes`（`name`，必须存在的 `listeners`，
  `priority`，带 fit 检查的 `match.host` 和 `match.path`，`handler`）；`respond` handler（`status` 100–599，拒绝 1xx，
  `headers` 须为 token 且不含换行，204 和 304 不能有 body）。名字规则同 fit：字母、数字、`_ . -`，同类唯一。
- 问题会全部收集而不是遇到第一个就停，每条都按 fit 的格式写：`file:line:col: path: message`，消息也用 fit 的
  （`unknown field (did you mean …?)`、`missing required field`、`expected an array, got an object`、
  `another listener is already named …`）。
- 决定 002 落到代码里：fit 能读而 akit 还不支持的字段和值（其余顶层部分、`tls`、`middleware`、其他匹配器、协议和
  handler 模块）一律按名字报为 `not supported yet`；其他的报为未知字段或未知模块。`Errors::only_not_supported_yet`
  用来区分“akit 暂时跑不了的合法 fit 配置”和“写错的配置”。
- 验证：读取器的单元测试（各种值、注释、每种拒绝、深度、位置、一份配置的每个前缀和每个单字节改动）、模型检查的单元测试，
  以及集成测试：fit-examples 的 `hello`、默认值、一份文档里的全部问题、文件错误；fit 完整的 1 MB JSON 模板
  （`templates/json/fit.json`，`../fit` 存在时读取）读下来只有“暂不支持”。`make check` 通过。用 cargo-fuzz 做了模糊测试
  （`fuzz/`，目标 `config_json`）：60 秒 340 万次运行，无崩溃；CI 每次推送跑 60 秒。
- 与计划的不同：`upstreams`、`admin` 和 `log` 报为“暂不支持”而没有建模，因为要到 I14、I26、I21 才有东西用到它们，
  而决定 002 禁止读了字段又忽略；它们随这些迭代加入。

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
