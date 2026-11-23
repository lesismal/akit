# akit 开发进度

[English](progress.md) | [简体中文](progress.zh-CN.md)

本文件按迭代记录实际完成的内容，以及下一次会话从哪里继续。[开发计划](dev-plan.zh-CN.md) 说明每个迭代应该做
什么；本文件说明实际做了什么，以及与计划不同的地方。

## 当前状态

| | |
| --- | --- |
| 最后更新 | 2026-11-23 |
| 里程碑 | M0 基础：I01、I02、I03、I04、I07a、I07b 已完成。 |
| akio | akio `main` 上的 `50a82315`（`1.0.0-rc.1`），在 `Cargo.toml` 中按 `rev` 锁定 |
| fit | 作为规格；本计划基于 `7daee573` 阅读 |
| 可用功能 | 工作区能构建并通过 `make check`；`akit version` 和 `akit help`；`akit_config::from_json` 能读取 fit 的监听器、路由和 `respond` handler，fit 配置的其余部分按名字报为暂不支持；模块注册表和数据面 trait（`akit-runtime`）；`compile` 把配置编译成 `Graph`，`GraphCell` 负责发布、读取和退役图；兼容性门禁在 `make check` 和 CI 中把 fit 的每个夹具交给 akit-config |
| 下一步 | I05，`akit run`：监听器与 `respond`。 |
| akio 需求 | A1–A8 现在全部加进 akio，各在 akio 的独立分支上进行（[akio API 需求](akio-api-requests.zh-CN.md)） |

## 日志

### 2026-11-23：I04，运行时图及其替换

- `akit_runtime::compile(config, registry, env) -> Graph`：图存在之前，每条路由的 handler 都已从注册表加载并
  初始化。失败的配置不留下任何东西：失败前已初始化的模块会被清理，错误带有出错位置的路径（初始化错误指明字段时为
  `routes[1].handler.params.status`）。
- `GraphCell`（[决定 004](decisions/004-graph-publication-and-retirement.zh-CN.md)）：`swap` 在锁内发布图并递增
  epoch；`with` 通过每线程缓存的 `Arc` 读取，每次替换每个线程只刷新一次，因此请求不增减任何共享引用计数；`hold`
  为处理函数返回后仍在进行的请求或连接保留图；`collect` 在每个线程都已离开或越过旧图、且没有 `Hold` 保留它时退役
  被替换的图，只清理一次其模块。
- 测试（`tests/graph.rs`，8 个）：编译及其失败、替换与回收、hold、仍在旧图内的请求、空闲线程不会钉住旧图、8 个线程
  各替换并读取 300 次且每个图恰好退役一次，以及 4 个 akio I/O 线程服务期间的替换。
- 基准 `benches/graph_read.rs`：经 `GraphCell::with` 的读取，对比 `Arc` 克隆和加锁再克隆，1 线程和 16 线程；数据来自基准测试主机。
- 已验证：macOS 上的 `make check`。尚未验证：Linux（epoll 和 io_uring）上的测试和基准，已在基准测试主机上排队；
  结果由后续提交记录。
- 与计划的不同：“原子指针”是每次替换每个线程取一次的互斥锁（不用 unsafe，规则 14；epoch 才是原子量）。图不是由最后
  一个请求释放的：akio 没有在空闲 I/O 线程上运行代码的钩子，空闲线程的缓存会保留旧图，所以由控制面用 `collect` 退役
  （决定 004）。handler 参数是 `akit_config::Handler`，由 akit-config 解码和检查。在 I11 之前路由保持配置中的顺序。

### 2026-11-23：I07b，兼容性门禁接入 akit-config

- 门禁始终开启：删除了 `compat-gate` feature，akit-config 成为 `akit-compat` 的 dev-dependency，
  `gate_through_akit_config` 是普通测试，`make check` 和 CI 的 `cargo test --workspace`（Linux 的 epoll 和
  io_uring、macOS）都会运行。只有一种配置，门禁不会因为漏了某个参数而被跳过。`make compat-gate` 单独运行
  它并输出报告。
- akit 还没有读取器的格式自成一种结果 "format not supported yet"：.conf 文件和 `conf.d` 目录直到 I23，
  YAML 模板直到 I24。每个结果都是入库的 `tests/compat/gate-report.txt` 中的一行（因 "not supported yet"
  被拒绝的夹具每个字段一行），一次运行与之有任何方向的不同，门禁都会失败；`make compat-bless` 从一次通过的
  运行重写它。JSON 夹具只能出现 "not supported yet"；其他任何错误，或 "format not supported yet" 与其他发现
  同时出现，都不能通过。
- 当前结果（fit `7daee573`、fit-examples `9659d7e4`）：80 个夹具；0 个有效；1 个只因 "not supported yet"
  被拒绝（`templates/json/fit.json`：`upstreams`，模板其余部分都被注释掉了）；78 个格式尚不支持（74 个示例和
  `templates/conf/fit.conf` 为 .conf，`templates/yaml/fit.yaml` 为 YAML，另有三个 `conf.d` 目录）；1 个跳过
  （`examples/https`）；0 个失败。
- 钩子用 akit-config 问题的路径命名字段，并按字段清单的写法记录：akit 还没有的 handler 模块在
  `routes[0].handler.module` 处被拒绝，记为 `routes[].handler<proxy>`；值数组中被拒绝的值
  （`listeners[0].protocols[0]`，`h3`）记为该数组。匹配由 `akit_compat::inventory_path` 完成。
- `tests/compat/supported.txt` 标记了 I02 读取的 20 个字段：`schema_version`；`listeners` 及其 `name`、
  `address`、`protocols`（`http/1.1` 和 `h2c`；fit 的其他协议被拒绝）；`routes` 及其 `name`、`listeners`、
  `priority`、`match`、`match.host`、`match.path`、`handler`、`handler.module`、`handler.params`；
  `handler<respond>`、它的 `params` 以及其中的 `status`、`headers`、`body`。已重新生成
  `docs/compatibility.md`：3,821 个字段中已支持 20 个。
- 新测试把标记与代码对应起来：`supported_fields_are_read` 读取使用每个已标记字段的配置并检查读到的值；
  `akit_config_lists_match_the_inventory` 从源码读取 akit-config 在 `model.rs` 中的列表，检查每一项都是
  字段清单中的字段、它们覆盖 fit 中每个对象的字段、被拒绝的 handler 和协议都是 fit 的，以及 akit-config
  读取的字段正是 `supported.txt` 中的字段。全部通过：I02 的列表中没有拼写错误。
- akit-config 没有任何改动：它的列表从源码文本读取，因此不需要访问函数。
- 与计划不同之处：校验会为示例打开哪些文件（示例引用的文件和 WebAssembly 插件、替身密钥）移到第一个读取
  示例的迭代 I23；在 akit 展开变量之前，门禁不需要这些密钥。
- 发现但未修复（akit-config 归 I02 和 I04）：akit-config 的监听器默认协议是 `http/1.1` 和 `h2c`
  （`DEFAULT_PROTOCOLS`），而 fit 在没有 TLS 时只有 `http/1.1`（fit `config/config.go` 中 `protocols`
  字段的说明）。门禁和字段清单都看不到默认值；需要在 akit-config 中修复并补测试。

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
