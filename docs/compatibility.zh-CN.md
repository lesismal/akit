# 兼容性

[English](compatibility.md) | [简体中文](compatibility.zh-CN.md)

从 1.0 起，akit 遵守这里的承诺：能加载的配置继续能加载，Admin API 的客户端继续能工作，在每个 1.x 版本中都是如此。akit 的 v1
就是 fit 的 v1：同样的配置 schema 版本 1、同样的 Admin API v1，限于 akit 提供的部分（[fit 的政策](https://github.com/lesismal/fit/blob/main/docs/compatibility.zh-CN.md)），
fit 写着 "fit" 的名字换成 akit 的（[决定 037](decisions/037-akit-names.zh-CN.md)）。
可能破坏它们的改动要等到配置 schema 或 Admin API 的新版本，并附带迁移的办法。

akit 读取 fit 的配置（[决策 002](decisions/002-config-compatibility.zh-CN.md)）：同样的字段、默认值和
含义，同样的三种格式；其中以网关命名的九个默认值是 akit 的（fit 的 `fit` 改为 `akit`，[决定 037](decisions/037-akit-names.zh-CN.md)），
[迁移指南](guides/migrating-from-fit.zh-CN.md#akit-的名字)列出了它们。akit 尚不支持的字段会按名称拒绝——"not supported yet: `<path>`"——绝不读取后
忽略。本页后半部分说明这一点如何对照 fit 的模板和示例检查，并列出 fit 配置的每个字段及 akit 是否支持。这份清单会随里程碑逐步缩短。

- [v1：哪些保持稳定](#v1哪些保持稳定)
  - [覆盖的范围](#覆盖的范围)
  - [1.x 版本可以做什么](#1x-版本可以做什么)
  - [schema 的版本](#schema-的版本)
  - [改名的字段与弃用](#改名的字段与弃用)
  - [Admin API v1](#admin-api-v1)
  - [新版本](#新版本)
  - [冻结如何检查](#冻结如何检查)
- [门禁](#门禁)
- [解读结果](#解读结果)
- [刷新夹具](#刷新夹具)
- [支持一个字段](#支持一个字段)
- [akit 的扩展字段](#akit-的扩展字段)
- [字段清单](#字段清单)

## v1：哪些保持稳定

### 覆盖的范围

| 范围 | 版本 | 内容 |
| --- | --- | --- |
| 配置 | schema 版本 1 | fit 的 schema 版本 1（固定的 [`fit.schema.json`](../tests/compat/fixtures/fit/schema/fit.schema.json)）中的每个字段，包括处理器和中间件的参数：名称、类型、含义和默认值；以及 akit 的[扩展字段](#akit-的扩展字段)。akit 读取的字段会一直被读取；按“尚不支持”拒绝的字段以后可能变为读取。`.conf`、YAML 和 JSON 是同一份配置的三种写法。 |
| Admin API | v1 | 路径和方法、它们接受和返回的内容、状态码、头字段 `ETag`、`If-Match`、`If-None-Match`、`Akit-Config-Version` 和 `Akit-Dry-Run`（fit 的 `Fit-Config-Version` 和 `Fit-Dry-Run`，由决定 037 改名）、bearer token，以及 `?dry_run`（[见下文](#admin-api-v1)）。 |
| 命令行 | 1 | 命令 `run`、`validate`、`adapt`、`fmt`、`build` 和 `version`，它们的参数和退出码，与 fit 相同。 |
| 指标 | 1 | `/metrics` 提供和 OTLP 导出的指标的名称、类型和标签：fit 的指标，每个都从 `fit_` 改名为 `akit_`（决定 037）。 |
| 访问日志 | 1 | 其 JSON 行的字段，与 fit 相同。 |

不覆盖：

- akit 写给人看而不是给程序读的东西：消息、警告、仪表盘、`akit adapt` 的输出（除了它是有效配置这一点），以及错误的文字
  （错误的 JSON 结构是覆盖的）。
- WASM 插件的宿主函数（`akit` 导入模块，即由决定 037 改名的 fit 的 `fit`）和 fit 的 Proxy-Wasm 子集，它们在 fit 的 1.0 中也是实验性的；`wasm` 中间件的配置是覆盖的。
- nginx 和 Caddyfile 转换器的覆盖范围，它只会增长。
- akit 各 crate 的 Rust API（`akit build` 构建的模块使用它）：在 1.0 中它可能在任何版本中改变（fit 覆盖其 Go 的 `module`
  包；akit 暂不覆盖）。
- 性能。

### 1.x 版本可以做什么

- 增加配置字段、模块、provider 和枚举值；增加的字段是可选的，省略它时保持前一版本的行为。开始读取原先按“尚不支持”拒绝的字段。
- 增加 Admin API 的路径、可选的请求字段、响应字段和响应字段的取值。客户端应忽略它不认识的字段。
- 增加指标，以及给原先没有该名称标签的指标增加标签。
- 增加命令和参数。
- 只为堵住安全漏洞或约束原本无界的资源，收紧默认上限或改变默认值；发布说明会写明。
- 给字段改名（[见下文](#改名的字段与弃用)）。
- 修正与文档、fit 的行为或 akit 声明遵循的 RFC 相矛盾的行为。

1.x 版本拒绝的配置，如果是因为它原本就有错而未被发现（两个不能同时出现的字段、超出范围的值），属于缺陷修复；发布说明会逐条列出。

### schema 的版本

配置可以说明它按哪个 schema 版本编写：

```json
{"schema_version": 1, "listeners": [{"name": "web", "address": ":8080"}], "routes": [{"handler": {"module": "respond"}}]}
```

它是可选的，省略时按版本 1 读取。akit 读取版本 1，对于写着其他版本的配置（`schema_version: 2 is greater than the maximum 1`），
akit 会拒绝而不是错误地读取，与 fit 相同。

### 改名的字段与弃用

在一个版本内改名的字段，在下一个版本之前保留旧名。使用旧名的配置照常加载，akit 会用 fit 的原话指出位置：

```text
fit.json:12:40: routes[0].middleware[0].params.max_size: deprecated: renamed "max_bytes", which takes the same value; the old name goes in config version 2
```

`akit validate` 和 `akit run` 在启动和每次重载时打印它，位置是旧名在文件中的行和列（`.conf` 和 YAML 同样如此），Admin API 在变更
应答的 `warnings` 中返回它。fit 的 schema 保留旧名并标记为 `deprecated`。配置不能同时给出两个名字
（`"max_size" is the old name of "max_bytes"; give only "max_bytes"`）。

以下字段为 fit 的 1.0 改名，使表示字节数的字段都以 `_bytes` 结尾、表示周期的都以 `_interval` 结尾，akit 两个名字都读取：

| 模块 | 旧名 | 名称 |
| --- | --- | --- |
| `cache` | `max_size` | `max_bytes` |
| `cache` | `max_entry_size` | `max_entry_bytes` |
| `cache` | `disk.max_size` | `disk.max_bytes` |
| `compress` | `min_length` | `min_bytes` |
| `jwt` | `jwks_refresh` | `jwks_refresh_interval` |
| `wasm` | `max_memory` | `max_memory_bytes` |

fit 的第七个，tunnel 的 `quic_pin`（现为 `pin`），属于 akit 尚未运行的部分。

任何东西离开 v1 都要经过弃用：字段、取值或端点保留下来并照常工作，在能给出警告的地方给出警告，直到主版本结束；只有在 schema 或
API 的下一个版本中才会去掉，并在其改动清单中写明。

### Admin API v1

| 方法和路径 | 作用 |
| --- | --- |
| `GET`、`HEAD` `/config`、`/config/{path...}` | 正在运行的配置（按写入时的样子），或某个路径处的部分，其中内联的秘密以 `[redacted]` 给出 |
| 对 `/config` 和 `/config/{path...}` 的 `PUT`、`PATCH`、`DELETE` | 替换、合并补丁（RFC 7396）或删除整个配置或某个路径处的部分，并应用结果 |
| `POST /load` | 应用一份完整的配置 |
| `GET`、`HEAD` `/config/effective` | 合并了 provider 所提供内容的配置，以及每部分的来源 |
| `GET`、`HEAD` `/versions`、`/versions/{n}` | 已应用并保留的配置，以及其中之一 |
| `POST /rollback/{n}` | 把保留的某个配置作为新版本再次应用 |
| `POST /reload` | 把正在运行的配置作为新版本再次应用 |
| `GET`、`HEAD` `/status` | 监听器、连接、上游及其健康状态，以及证书 |
| `GET`、`HEAD` `/metrics` | Prometheus 文本格式的指标 |
| `GET`、`HEAD` `/cache`、`POST /cache/purge` | 缓存的区域，以及按 URL、URL 前缀、标签或区域清除 |
| `GET`、`HEAD` `/logs`、`POST /logs/rotate`、`POST /logs/cleanup` | 日志文件，立即切割其中一个，以及执行或列出其保留策略（冻结之后新增，I83） |
| `POST /config/render`、`POST /config/parse`、`PUT /config/text`、`POST /config/export`、`POST /config/import` | 配置或其部分顶层成员（`?members=`）的 `.conf`、YAML 或 JSON 文本（`?format=`），以及应用一段文本或一个文件，即 fit 控制台的原文编辑器和导入导出页面所调用的（冻结之后新增，I91；fit 的 `store` 格式，即用自己的密码或正在运行的 store 的密钥密封的 `fitstore` 文件，自 I93b 起） |
| `GET`、`HEAD` `/tokens` 和 `/tokens/{成员}`、`POST /tokens/{成员}`、`PATCH` 和 `DELETE /tokens/{成员}/{id}` | 隧道 hub 和 LLM 角色的链路 token：签发、列出、修改过期时间和撤销，同 fit 的 `admin/tokens.go`；akit 的作用于运行中配置所设置的成员，无论其角色是否已在运行（冻结之后新增，I100） |
| `GET /dashboard/` | 仪表盘，给人看（不覆盖） |

配置中的路径是 JSON pointer 的各个 token。变更的应答给出所应用的版本，在内容中（`version`）和 `Akit-Config-Version` 中，并给出配置的
`ETag`；带 `If-Match` 的变更只应用于该 ETag 所指的配置，否则答 412；带 `?dry_run=1` 的变更只检查不应用，应答带
`Akit-Dry-Run: 1`。无法加载的配置答 400，内容为 `{"error": ..., "errors": [{"path", "line", "column", "message"}]}`。
这些都是 fit 的，只是 fit 的请求头字段 `Fit-Config-Version` 和 `Fit-Dry-Run` 改了名（[决定 037](decisions/037-akit-names.zh-CN.md)）：
读取或发送它们的 fit 客户端要改用 akit 的名字；fit 的名字不被接受。

自 I80b 起，配置中内联写出的秘密（成员名与 fit 控制台认定秘密的规则相同：`password_hash`、`secret`、`token`、`Authorization`
头）在 `/config`、`/versions/{n}` 和 `/config/effective` 中以 `[redacted]` 给出，与 fit 控制台的显示相同；修改中送回的
`[redacted]` 保留原位置的秘密（[决定 035](decisions/035-admin-api-redacts-secrets.zh-CN.md)）。fit 自己的 Admin API
按写入时的样子返回配置；这一改动关闭威胁模型的 L2，属于 1.x 版本为修补安全漏洞可以做的改动。

fit 的 v1 还有更多端点，用于 akit 尚未运行的部分：tunnel 和 LLM 角色的链接令牌（`/tokens`）、媒体库和工作台的媒体页面
（`/media/...`），以及流媒体角色（`/streaming/...`）。akit 在运行这些部分之前对它们答 404，届时按 fit 的样子加入，属于兼容的增加。

自 I90 起，控制台（`admin.console`）在 `/api/m/admin/` 下提供同样的端点，用它的登录代替令牌，另有 fit 控制台自己的端点（`/api/login`、`/api/logout`、`/api/session`、`/api/workbench`），cookie 和 CSRF 请求头用 akit 的名字 `akit_console_<端口>` 和 `X-Akit-Csrf`（fit 的 `fit_console_<端口>` 和 `X-Fit-Csrf`，决定 037；[控制台指南](guides/console.zh-CN.md)）。

如果需要 Admin API 的 v2，它会在 `/v2/` 下与 v1 的路径并存，v1 的路径在 akit 的整个主版本内保留。

### 新版本

无法保持旧配置或旧客户端继续工作的改动，要等待新的 schema 或 Admin API 版本（先由 fit 推出），它附带：

- 每项改动及其原因的清单；
- 对配置：`akit adapt`，把前一版本的配置改写为新版本；新的 akit 至少在一个发布中仍读取前一版本，并给出警告；
- 对 Admin API：前一版本与新版本并存，直到主版本结束。

### 冻结如何检查

[tests/compat/v1](../tests/compat/v1) 保存冻结时（I79）的 v1 范围，以及此后的改动：

- `config.txt`，配置的范围：fit 的 schema 版本 1 的每个字段及其类型、是否必填和默认值，枚举的取值，弃用的旧名，akit 读取的字段，
  以及 akit 的扩展字段，每行一项；
- `admin-api.txt`，Admin API 的范围：端点、头字段、查询参数，以及变更、错误和 `GET /versions` 的应答成员；
- `additions.txt`：此后的每项改动，标为 `added`（兼容的增加）或 `retired`（1.x 版本按本政策改变的项，附原因）。

两项检查在任何未标记的改动上失败，在 CI 和 `make check` 中运行：

- `tools/v1-freeze.py`（CI 的 lint 任务，`make compat-check`）从 fit 的固定 schema、[`supported.txt`](../tests/compat/supported.txt)
  和扩展字段构建配置的范围，与基线和增加项比较。首次读取的字段，或固定提交移动时 fit 新增的字段，属于增加：`make v1-bless` 会标记它；
  只要有破坏 v1 的改动（字段消失，类型、默认值或必填性改变，给 v1 已有的对象增加必填字段，akit 原本读取的字段又被拒绝），它就拒绝标记任何东西。
- `v1_surface_is_frozen`（akit-admin 的测试，CI 的测试任务）运行 Admin API 并逐一探测：冻结的每个端点都提供服务，冻结路径的其他方法都不提供，
  分发器中出现的路径都在冻结范围内，头字段和应答成员与冻结的一致。新增的项按失败的测试打印的内容，手工标记在 `additions.txt` 中。

基线文件本身在 v1 内从不修改：`tools/v1-freeze.py` 保存它们的 SHA-256，改动任何一个都意味着新版本。它们在 akit 首次发布之前
按[决定 037](decisions/037-akit-names.zh-CN.md)重写过一次：配置的基线由 `python3 tools/v1-freeze.py --write-baseline` 重写
（当前表面减去已标记的新增项，并带上 akit 改名 fit 默认值的九项），Admin API 的基线手工修改（其两个请求头字段改名）。

## 门禁

兼容性门禁是 [tests/compat](../tests/compat) 中的测试 crate `akit-compat`。它取用固定在某个 fit 提交
和某个 fit-examples 提交上的配置（[来源说明](../tests/compat/SOURCE.zh-CN.md)）：

- fit `templates/` 中的每个模板：`.conf`、YAML 和 JSON，各有单文件和按类别拆分的 `conf.d` 目录两种
  形式（共 6 份配置）；
- fit-examples 的每份配置，即每个示例的 `fit.conf`（当前固定提交下共 74 份）。

并像 `akit validate -c <文件或目录>` 那样，把每一份交给 akit 的校验。每一份必须：

- **有效**，或者
- **只**因 "not supported yet: `<path>`" 被拒绝，且每个路径都是 fit 接受的字段（即
  [字段清单](#字段清单) 中的字段）。

其他任何情况都不能通过门禁：解析错误、类型错误、fit 接受的字段被当作未知字段或以错误路径拒绝、
拒绝时没有指出任何字段。读取后被忽略的字段同样不能通过；字段清单让这一点可见，因为没有标记为
已支持的字段必须被拒绝。

各部分及其状态：

| 部分 | 位置 | 状态 |
| --- | --- | --- |
| 固定的夹具及其许可证 | `tests/compat/fixtures/`、`tests/compat/SOURCE.md` | 完成（I07a） |
| 字段清单 | `tests/compat/fit-fields.txt`，由 `tools/compat-inventory.py` 生成 | 完成（I07a） |
| 测试框架：枚举、判定、报告 | `tests/compat/src/lib.rs`、`tests/compat/tests/gate.rs` | 完成（I07a） |
| 经由 akit-config 的门禁，报告入库 | `tests/compat/tests/gate.rs` 中的 `gate_through_akit_config`，`tests/compat/gate-report.txt` | 完成（I07b） |
| 已支持字段确实被读取，akit-config 的列表对照字段清单检查 | 同一文件中的 `supported_fields_are_read`、`akit_config_lists_match_the_inventory` | 完成（I07b） |
| 读取 .conf 夹具和 `conf.d` 目录 | akit-config 的 .conf 读取器（`crates/akit-config/src/conf`）与合并 | 已完成（I23） |
| 读取 YAML 夹具 | akit-config 的 YAML 读取器（[决定 005](decisions/005-yaml-reader.zh-CN.md)） | 已完成（I24） |

门禁是工作区里的普通测试，没有 feature，也没有 ignore：`make test`（因而 `make check`）会运行它，
CI 在 Linux（epoll、io_uring）和 macOS 上的 `cargo test --workspace` 也会运行它。只有一种构建配置，
门禁就不会被意外漏掉；在 I02 合并之前用来挡住它的 feature 已经删除。此外，当 `fit-fields.txt` 或下方
表格过期时，`make compat-check` 失败。

### akit 还不能读取的格式

akit-config 读取 JSON、fit 自己的 `.conf` 格式（I23）和 YAML（I24，[决定 005](decisions/005-yaml-reader.zh-CN.md)），
并像 fit 那样合并目录中的文件：所有示例、所有模板和所有 `conf.d` 目录都经由它读取，YAML 模板读入的模型与对应的
JSON 版相同，结果也相同。若有 akit 没有读取器的格式的夹具，它既不会被悄悄判为通过，也不会被悄悄判为失败：它的
结果是 "format not supported yet"（格式尚不支持），和其他结果一样写进门禁的报告。akit 能读取的夹具读取时只能出现
"not supported yet" 的发现；其他任何错误都不能通过门禁，"format not supported yet" 与其他发现同时出现也不能通过。

`.conf` 读取器按 fit 自己的类型给每条指令定型，类型取自字段清单（`fit-fields.txt`，编译进
akit-config）：`.conf` 文件转换成的树与它的 JSON 孪生文件解析出的树逐个成员相同，由同一个解码器读取。
一个测试（`crates/akit-config/src/conf/tests.rs` 中的 `templates_match_their_json_twins`）要求固定
提交下的每个 `.conf` 模板与其 JSON 孪生文件一致。刷新夹具时，读取器的类型随字段清单一起刷新。

### 校验会打开什么

校验只读取配置自身的文件，不打开配置中提到的任何东西：配置提到的文件（TLS 证书和私钥、示例的
WebAssembly 插件、静态文件根目录、`password_file`）以及按环境变量名提到的密钥（`password_env`）都属于
akit-config 以 "not supported yet" 拒绝的字段，所以示例就在固定的位置上校验，既不需要以 fit-examples
的根目录为工作目录，也不需要真实的密钥。`make compat-gate` 仍会设置 fit-examples 自己的测试所设置的
替身密钥，以备这类字段得到支持；届时校验是否打开它们（相对于 fit-examples 运行时所在的目录），由支持
该字段的迭代决定。`examples/https` 仍然跳过：它的证书文件属于用户自己。

### 入库的报告

[tests/compat/gate-report.txt](../tests/compat/gate-report.txt) 记录每一个结果，每行一条。一次运行的
结果与它有任何方向的不同，门禁都会失败：某个夹具失败、某个字段新被拒绝，以及某个夹具开始能被读取、
某个字段不再被拒绝。改变结果的提交用下面的命令重写报告：

```bash
make compat-bless
```

只要有夹具失败，它就拒绝写入；报告的 diff 随变更一起评审。若门禁看到某个被当作 "not supported yet"
拒绝的字段在 `supported.txt` 中标记为已支持，也会失败。

## 解读结果

`make compat-gate` 单独运行门禁并输出报告，即 `gate-report.txt` 的各行：每个夹具一行；因
"not supported yet" 被拒绝的夹具则每个字段一行。

```text
examples/hello: valid
examples/http3: not supported yet: listeners[].tls.acme
examples/l4-udp: not supported yet: listeners[].protocols: the "udp" protocol
examples/https: skipped: its certificate and key files are the user's own
templates/conf/fit.conf: not supported yet: upstreams
templates/yaml/conf.d: not supported yet: upstreams
```

失败的夹具写作 `<夹具>: FAILED: <原因>`，每条不属于允许的 "not supported yet" 的发现一行；与入库报告的
差异写作 `- <行>`（只在入库报告中）和 `+ <行>`（只在本次运行中）。akit 给出的路径按字段清单的写法记录：
数组下标（`routes[0]`）和映射键（`log.loggers.engine`）分别对应 `[]` 和 `*`；akit 还没有的 handler
模块在 `routes[0].handler.module` 处被拒绝，记为 `routes[].handler<proxy>`。akit 读取的字段中它尚不
支持的某个值，写在字段之后：`listeners[0].protocols[0]`（即 `udp` 协议）记为
`listeners[].protocols: the "udp" protocol`；没有监听器的配置（fit 默认以 HTTPS 提供服务）记为
`listeners: a config with no listeners, ... (auto_https)`。这样的行不是整个字段被拒绝：该字段仍是
已支持的，门禁"已支持字段不得被拒绝"的检查只看整体被拒绝的字段。

## 刷新夹具

```bash
tests/compat/update-fixtures.sh <fit-commit> <fit-examples-commit>
```

替换夹具、改写固定提交，并重新生成字段清单和下方表格。移动固定提交是需要评审的变更，需单独提交；
做法见 [SOURCE.zh-CN.md](../tests/compat/SOURCE.zh-CN.md)。如果只是清单的形式变了（生成器或
`supported.txt`），单独重新生成即可：

```bash
python3 tools/compat-inventory.py
```

## 支持一个字段

让 akit 读取某个字段并真正起作用的迭代，要在同一个提交中把该字段的路径加入
[tests/compat/supported.txt](../tests/compat/supported.txt)，并重新运行生成器。生成器会拒绝 fit
不接受的路径，并在本页的两种语言中把该字段标记为已支持。随后由三个测试把标记与代码对应起来：

- `supported_fields_are_read` 用每个已标记字段构造一份小的 JSON 配置，交给 akit-config 读取并检查读到
  的值：标记为已支持、但 akit 拒绝或读取后丢弃的字段会让它失败；每个新字段都要补一个样例。
- `akit_config_lists_match_the_inventory` 读取 `crates/akit-config/src/model.rs` 中的列表——每个对象
  读取哪些字段、把哪些字段作为 "not supported yet" 拒绝（以及拒绝的 handler 模块和协议）：每一项都必须
  是字段清单中的字段（拼写错误会失败），合起来必须覆盖字段清单中该对象的全部字段（fit 接受的字段不会被
  当作未知字段拒绝），并且 akit-config 读取的字段必须正是 `supported.txt` 标记的字段。`model.rs` 新增的
  列表要在这个测试中映射到它在字段清单中的路径。
- 门禁本身：字段不再被拒绝时，报告随之变化（`make compat-bless`）。

同一个提交还要以 `python3 tools/config-reference.py`（`make reference`）重新生成[配置参考](reference/config.zh-CN.md)的表格，
把该字段从“尚不支持”改为已读取；akit 读取它的方式与 fit 不同时，在 `tools/config-reference-notes.json` 中写一条说明。参考的表格与
akit-config 的列表不一致时，`reference_matches_akit_config`（同一文件）会失败；表格不是生成器写出的内容时，CI 的
`config-reference.py --check` 会失败。新的扩展字段也要连同其决定列在那里。同一个提交还要用 `make v1-bless` 把新读取的字段标记为 v1
冻结之后的增加（[冻结如何检查](#冻结如何检查)）。

## akit 的扩展字段

少数字段属于 akit 自己，用于 akio 能做而 fib 不能做的事；每个都有一份决策记录说明原因，并且要向 fit 提出
（[决策 002](decisions/002-config-compatibility.zh-CN.md)）。akit-config 把它们与 fit 的字段分开列出（`model.rs` 中的
`*_AKIT_ONLY`），`.conf` 的读取器和写出器从 `crates/akit-config/src/conf/convert.rs` 的 `EXTENSIONS` 取得它们的类型，
如果 fit 的字段清单中有了其中某个字段，`akit_config_lists_match_the_inventory` 会失败，这时把它移到 fit 字段的列表。
设置了扩展字段的配置在 fit 采纳之前只属于 akit：fit 会把它当作未知字段拒绝。

| 字段 | 类型 | 引入 | 决策 |
| --- | --- | --- | --- |
| `listeners[].sockmap` | `boolean` | I51b | [010](decisions/010-sockmap-relay.zh-CN.md)：`tcp` 监听器的连接在 akio 的 eBPF sockmap 转发能加载的地方于内核中拼接 |

## 字段清单

fit 接受的每个字段，由固定提交下 fit 的 JSON Schema 生成（`fixtures/fit/schema/fit.schema.json`，
fit 由其配置类型和模块生成，与其
[配置参考](https://github.com/lesismal/fit/blob/main/docs/reference/config.zh-CN.md) 同源）。同一份
清单（含标志）见 [tests/compat/fit-fields.txt](../tests/compat/fit-fields.txt)，每个字段一行，以制表符
分隔：路径、部分、类型、标志。

记法：`a.b` 是对象的字段，`a[]` 是数组的元素，`a.*` 是映射的值，`h<m>` 是位于 `h`、`module` 为 `m`
的对象：fit 的 handler、middleware 以及 sync 的 source、sink、transform 和 codec，它们的 `params` 取决于
模块。因此 `routes[].handler<respond>.params.status` 是路由的 handler 为 `respond` 时的 `status` 参数。
模块的参数单独成为一个部分 `<namespace>/<module>`；在多处使用的模块（例如每个 source 和 sink 下的
sync codec）在每一处都会列出。类型：`duration` 是形如 `"1.5s"` 的字符串；`bytes|percent` 是字节数或
内存占比（如 `"50%"`）；`enum(…)` 列出可选值；`module` 指模块本身。

<!-- BEGIN GENERATED: tools/compat-inventory.py; do not edit -->

共 3821 个字段，分属 121 个部分，对应 fit `7daee573`；已支持 1198 个。

| 部分 | 字段 | 已支持 | 尚不支持 |
| --- | --- | --- | --- |
| [`access`](#fields-access) | 5 | 5 | 0 |
| [`access_log`](#fields-access-log) | 14 | 14 | 0 |
| [`acme`](#fields-acme) | 43 | 43 | 0 |
| [`admin`](#fields-admin) | 105 | 51 | 54 |
| [`auto_https`](#fields-auto-https) | 5 | 5 | 0 |
| [`bittorrent`](#fields-bittorrent) | 160 | 91 | 69 |
| [`engine`](#fields-engine) | 11 | 8 | 3 |
| [`http.handlers/bt_publish`](#fields-http-handlers-bt-publish) | 3 | 0 | 3 |
| [`http.handlers/bt_stream`](#fields-http-handlers-bt-stream) | 14 | 0 | 14 |
| [`http.handlers/bt_tracker`](#fields-http-handlers-bt-tracker) | 2 | 2 | 0 |
| [`http.handlers/fastcgi`](#fields-http-handlers-fastcgi) | 18 | 18 | 0 |
| [`http.handlers/forward_proxy`](#fields-http-handlers-forward-proxy) | 8 | 8 | 0 |
| [`http.handlers/hls`](#fields-http-handlers-hls) | 12 | 12 | 0 |
| [`http.handlers/httpflv`](#fields-http-handlers-httpflv) | 6 | 6 | 0 |
| [`http.handlers/media_pages`](#fields-http-handlers-media-pages) | 29 | 26 | 3 |
| [`http.handlers/media_tokens`](#fields-http-handlers-media-tokens) | 11 | 0 | 11 |
| [`http.handlers/proxy`](#fields-http-handlers-proxy) | 20 | 19 | 1 |
| [`http.handlers/respond`](#fields-http-handlers-respond) | 5 | 5 | 0 |
| [`http.handlers/static`](#fields-http-handlers-static) | 16 | 16 | 0 |
| [`http.handlers/stream_whep`](#fields-http-handlers-stream-whep) | 5 | 5 | 0 |
| [`http.handlers/stream_whip`](#fields-http-handlers-stream-whip) | 5 | 5 | 0 |
| [`http.handlers/sync_ingest`](#fields-http-handlers-sync-ingest) | 6 | 0 | 6 |
| [`http.handlers/vod`](#fields-http-handlers-vod) | 13 | 0 | 13 |
| [`http.handlers/webrtc_proxy`](#fields-http-handlers-webrtc-proxy) | 8 | 8 | 0 |
| [`http.handlers/websockify`](#fields-http-handlers-websockify) | 12 | 0 | 12 |
| [`http.handlers/whep`](#fields-http-handlers-whep) | 4 | 4 | 0 |
| [`http.handlers/whip`](#fields-http-handlers-whip) | 4 | 4 | 0 |
| [`http.middleware/basic_auth`](#fields-http-middleware-basic-auth) | 8 | 8 | 0 |
| [`http.middleware/body_limit`](#fields-http-middleware-body-limit) | 3 | 3 | 0 |
| [`http.middleware/cache`](#fields-http-middleware-cache) | 17 | 17 | 0 |
| [`http.middleware/circuit_breaker`](#fields-http-middleware-circuit-breaker) | 10 | 10 | 0 |
| [`http.middleware/client_cert`](#fields-http-middleware-client-cert) | 4 | 4 | 0 |
| [`http.middleware/compress`](#fields-http-middleware-compress) | 10 | 10 | 0 |
| [`http.middleware/concurrency_limit`](#fields-http-middleware-concurrency-limit) | 4 | 4 | 0 |
| [`http.middleware/forward_auth`](#fields-http-middleware-forward-auth) | 7 | 7 | 0 |
| [`http.middleware/headers`](#fields-http-middleware-headers) | 10 | 10 | 0 |
| [`http.middleware/jwt`](#fields-http-middleware-jwt) | 13 | 13 | 0 |
| [`http.middleware/rate_limit`](#fields-http-middleware-rate-limit) | 9 | 9 | 0 |
| [`http.middleware/redirect`](#fields-http-middleware-redirect) | 5 | 5 | 0 |
| [`http.middleware/rewrite`](#fields-http-middleware-rewrite) | 6 | 6 | 0 |
| [`http.middleware/wasm`](#fields-http-middleware-wasm) | 18 | 18 | 0 |
| [`limits`](#fields-limits) | 7 | 7 | 0 |
| [`listeners`](#fields-listeners) | 30 | 28 | 2 |
| [`llm`](#fields-llm) | 218 | 215 | 3 |
| [`log`](#fields-log) | 24 | 24 | 0 |
| [`media_auth`](#fields-media-auth) | 34 | 18 | 16 |
| [`media_library`](#fields-media-library) | 24 | 0 | 24 |
| [`metrics`](#fields-metrics) | 1 | 1 | 0 |
| [`pprof`](#fields-pprof) | 4 | 0 | 4 |
| [`providers`](#fields-providers) | 60 | 59 | 1 |
| [`routes`](#fields-routes) | 24 | 24 | 0 |
| [`schema_version`](#fields-schema-version) | 1 | 1 | 0 |
| [`services`](#fields-services) | 13 | 13 | 0 |
| [`streaming`](#fields-streaming) | 76 | 41 | 35 |
| [`sync`](#fields-sync) | 42 | 0 | 42 |
| [`sync.codecs/canal_json`](#fields-sync-codecs-canal-json) | 87 | 0 | 87 |
| [`sync.codecs/csv`](#fields-sync-codecs-csv) | 116 | 0 | 116 |
| [`sync.codecs/debezium`](#fields-sync-codecs-debezium) | 58 | 0 | 58 |
| [`sync.codecs/json`](#fields-sync-codecs-json) | 87 | 0 | 87 |
| [`sync.codecs/line`](#fields-sync-codecs-line) | 87 | 0 | 87 |
| [`sync.codecs/raw`](#fields-sync-codecs-raw) | 87 | 0 | 87 |
| [`sync.codecs/ticdc_simple`](#fields-sync-codecs-ticdc-simple) | 87 | 0 | 87 |
| [`sync.codecs/wasm`](#fields-sync-codecs-wasm) | 203 | 0 | 203 |
| [`sync.sinks/clickhouse`](#fields-sync-sinks-clickhouse) | 38 | 0 | 38 |
| [`sync.sinks/dead_letters`](#fields-sync-sinks-dead-letters) | 12 | 0 | 12 |
| [`sync.sinks/doris`](#fields-sync-sinks-doris) | 40 | 0 | 40 |
| [`sync.sinks/elasticsearch`](#fields-sync-sinks-elasticsearch) | 34 | 0 | 34 |
| [`sync.sinks/file`](#fields-sync-sinks-file) | 30 | 0 | 30 |
| [`sync.sinks/http`](#fields-sync-sinks-http) | 42 | 0 | 42 |
| [`sync.sinks/influxdb`](#fields-sync-sinks-influxdb) | 48 | 0 | 48 |
| [`sync.sinks/kafka`](#fields-sync-sinks-kafka) | 52 | 0 | 52 |
| [`sync.sinks/loki`](#fields-sync-sinks-loki) | 42 | 0 | 42 |
| [`sync.sinks/memory`](#fields-sync-sinks-memory) | 8 | 0 | 8 |
| [`sync.sinks/milvus`](#fields-sync-sinks-milvus) | 32 | 0 | 32 |
| [`sync.sinks/mongo`](#fields-sync-sinks-mongo) | 44 | 0 | 44 |
| [`sync.sinks/mqtt`](#fields-sync-sinks-mqtt) | 70 | 0 | 70 |
| [`sync.sinks/mssql`](#fields-sync-sinks-mssql) | 66 | 0 | 66 |
| [`sync.sinks/mysql`](#fields-sync-sinks-mysql) | 40 | 0 | 40 |
| [`sync.sinks/nats`](#fields-sync-sinks-nats) | 42 | 0 | 42 |
| [`sync.sinks/otlp_logs`](#fields-sync-sinks-otlp-logs) | 52 | 0 | 52 |
| [`sync.sinks/postgres`](#fields-sync-sinks-postgres) | 42 | 0 | 42 |
| [`sync.sinks/pulsar`](#fields-sync-sinks-pulsar) | 38 | 0 | 38 |
| [`sync.sinks/qdrant`](#fields-sync-sinks-qdrant) | 28 | 0 | 28 |
| [`sync.sinks/rabbitmq`](#fields-sync-sinks-rabbitmq) | 48 | 0 | 48 |
| [`sync.sinks/redis`](#fields-sync-sinks-redis) | 46 | 0 | 46 |
| [`sync.sinks/s3`](#fields-sync-sinks-s3) | 36 | 0 | 36 |
| [`sync.sinks/stdout`](#fields-sync-sinks-stdout) | 4 | 0 | 4 |
| [`sync.sinks/tdengine`](#fields-sync-sinks-tdengine) | 42 | 0 | 42 |
| [`sync.sinks/wasm`](#fields-sync-sinks-wasm) | 16 | 0 | 16 |
| [`sync.sources/clickhouse`](#fields-sync-sources-clickhouse) | 22 | 0 | 22 |
| [`sync.sources/doris`](#fields-sync-sources-doris) | 21 | 0 | 21 |
| [`sync.sources/elasticsearch`](#fields-sync-sources-elasticsearch) | 21 | 0 | 21 |
| [`sync.sources/file`](#fields-sync-sources-file) | 16 | 0 | 16 |
| [`sync.sources/generate`](#fields-sync-sources-generate) | 7 | 0 | 7 |
| [`sync.sources/http`](#fields-sync-sources-http) | 10 | 0 | 10 |
| [`sync.sources/kafka`](#fields-sync-sources-kafka) | 24 | 0 | 24 |
| [`sync.sources/mongo`](#fields-sync-sources-mongo) | 24 | 0 | 24 |
| [`sync.sources/mqtt`](#fields-sync-sources-mqtt) | 34 | 0 | 34 |
| [`sync.sources/mssql`](#fields-sync-sources-mssql) | 34 | 0 | 34 |
| [`sync.sources/mysql`](#fields-sync-sources-mysql) | 21 | 0 | 21 |
| [`sync.sources/nats`](#fields-sync-sources-nats) | 32 | 0 | 32 |
| [`sync.sources/postgres`](#fields-sync-sources-postgres) | 24 | 0 | 24 |
| [`sync.sources/pulsar`](#fields-sync-sources-pulsar) | 21 | 0 | 21 |
| [`sync.sources/rabbitmq`](#fields-sync-sources-rabbitmq) | 26 | 0 | 26 |
| [`sync.sources/redis`](#fields-sync-sources-redis) | 19 | 0 | 19 |
| [`sync.sources/s3`](#fields-sync-sources-s3) | 22 | 0 | 22 |
| [`sync.sources/tidb`](#fields-sync-sources-tidb) | 39 | 0 | 39 |
| [`sync.sources/wasm`](#fields-sync-sources-wasm) | 10 | 0 | 10 |
| [`sync.transforms/cast`](#fields-sync-transforms-cast) | 3 | 0 | 3 |
| [`sync.transforms/embed`](#fields-sync-transforms-embed) | 17 | 0 | 17 |
| [`sync.transforms/envelope`](#fields-sync-transforms-envelope) | 5 | 0 | 5 |
| [`sync.transforms/fields`](#fields-sync-transforms-fields) | 10 | 0 | 10 |
| [`sync.transforms/filter`](#fields-sync-transforms-filter) | 11 | 0 | 11 |
| [`sync.transforms/key`](#fields-sync-transforms-key) | 5 | 0 | 5 |
| [`sync.transforms/mask`](#fields-sync-transforms-mask) | 5 | 0 | 5 |
| [`sync.transforms/route`](#fields-sync-transforms-route) | 7 | 0 | 7 |
| [`sync.transforms/wasm`](#fields-sync-transforms-wasm) | 9 | 0 | 9 |
| [`telemetry`](#fields-telemetry) | 14 | 14 | 0 |
| [`tunnel`](#fields-tunnel) | 164 | 161 | 3 |
| [`upstreams`](#fields-upstreams) | 38 | 37 | 1 |
| [`webrtc`](#fields-webrtc) | 76 | 76 | 0 |
| **合计** | 3821 | 1198 | 2623 |

<a id="fields-access"></a>

#### `access`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `access` | `object` | 已支持 |
| `access.allow` | `array<string>` | 已支持 |
| `access.allow_files` | `array<string>` | 已支持 |
| `access.deny` | `array<string>` | 已支持 |
| `access.deny_files` | `array<string>` | 已支持 |

<a id="fields-access-log"></a>

#### `access_log`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `access_log` | `object` | 已支持 |
| `access_log.buffer_bytes` | `integer` | 已支持 |
| `access_log.flush_interval` | `duration` | 已支持 |
| `access_log.format` | `enum(json\|text\|line)` | 已支持 |
| `access_log.level` | `enum(info\|warn\|error)` | 已支持 |
| `access_log.output` | `string` | 已支持 |
| `access_log.rotate` | `object` | 已支持 |
| `access_log.rotate.compress` | `boolean` | 已支持 |
| `access_log.rotate.interval` | `duration` | 已支持 |
| `access_log.rotate.max_age` | `duration` | 已支持 |
| `access_log.rotate.max_bytes` | `integer` | 已支持 |
| `access_log.rotate.max_files` | `integer` | 已支持 |
| `access_log.rotate.max_total_bytes` | `integer` | 已支持 |
| `access_log.time_zone` | `string` | 已支持 |

<a id="fields-acme"></a>

#### `acme`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `acme` | `object` | 已支持 |
| `acme.ca_file` | `string` | 已支持 |
| `acme.challenges` | `array<enum(http-01\|tls-alpn-01)>` | 已支持 |
| `acme.directory` | `string` | 已支持 |
| `acme.email` | `string` | 已支持 |
| `acme.max_concurrent` | `integer` | 已支持 |
| `acme.on_demand` | `object` | 已支持 |
| `acme.on_demand.ask` | `string, required` | 已支持 |
| `acme.on_demand.idle_timeout` | `duration` | 已支持 |
| `acme.on_demand.interval` | `duration` | 已支持 |
| `acme.on_demand.max_names` | `integer` | 已支持 |
| `acme.on_demand.rate` | `integer` | 已支持 |
| `acme.on_demand.timeout` | `duration` | 已支持 |
| `acme.request_timeout` | `duration` | 已支持 |
| `acme.shared_storage` | `object` | 已支持 |
| `acme.shared_storage.consul` | `object` | 已支持 |
| `acme.shared_storage.consul.ca_file` | `string` | 已支持 |
| `acme.shared_storage.consul.cert_file` | `string` | 已支持 |
| `acme.shared_storage.consul.datacenter` | `string` | 已支持 |
| `acme.shared_storage.consul.endpoint` | `string` | 已支持 |
| `acme.shared_storage.consul.key_file` | `string` | 已支持 |
| `acme.shared_storage.consul.token_file` | `string` | 已支持 |
| `acme.shared_storage.etcd` | `object` | 已支持 |
| `acme.shared_storage.etcd.ca_file` | `string` | 已支持 |
| `acme.shared_storage.etcd.cert_file` | `string` | 已支持 |
| `acme.shared_storage.etcd.endpoints` | `array<string>` | 已支持 |
| `acme.shared_storage.etcd.key_file` | `string` | 已支持 |
| `acme.shared_storage.etcd.password_file` | `string` | 已支持 |
| `acme.shared_storage.etcd.username` | `string` | 已支持 |
| `acme.shared_storage.lock_ttl` | `duration` | 已支持 |
| `acme.shared_storage.prefix` | `string` | 已支持 |
| `acme.shared_storage.redis` | `object` | 已支持 |
| `acme.shared_storage.redis.address` | `string` | 已支持 |
| `acme.shared_storage.redis.ca_file` | `string` | 已支持 |
| `acme.shared_storage.redis.cert_file` | `string` | 已支持 |
| `acme.shared_storage.redis.db` | `integer` | 已支持 |
| `acme.shared_storage.redis.key_file` | `string` | 已支持 |
| `acme.shared_storage.redis.password_file` | `string` | 已支持 |
| `acme.shared_storage.redis.tls` | `boolean` | 已支持 |
| `acme.shared_storage.redis.username` | `string` | 已支持 |
| `acme.shared_storage.request_timeout` | `duration` | 已支持 |
| `acme.storage` | `string` | 已支持 |
| `acme.timeout` | `duration` | 已支持 |

<a id="fields-admin"></a>

#### `admin`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `admin` | `object` | 已支持 |
| `admin.access` | `object` | 已支持 |
| `admin.access.allow` | `array<string>` | 已支持 |
| `admin.access.allow_files` | `array<string>` | 已支持 |
| `admin.access.deny` | `array<string>` | 已支持 |
| `admin.access.deny_files` | `array<string>` | 已支持 |
| `admin.address` | `string` | 已支持 |
| `admin.assistant` | `object` | 尚不支持（not supported yet） |
| `admin.assistant.data` | `object` | 尚不支持（not supported yet） |
| `admin.assistant.data.redact` | `array<string>` | 尚不支持（not supported yet） |
| `admin.assistant.data.send_access_log` | `boolean` | 尚不支持（not supported yet） |
| `admin.assistant.data.send_logs` | `boolean` | 尚不支持（not supported yet） |
| `admin.assistant.data.send_records` | `boolean` | 尚不支持（not supported yet） |
| `admin.assistant.data.send_subjects` | `boolean` | 尚不支持（not supported yet） |
| `admin.assistant.default_level` | `enum(off\|read\|edit\|propose\|apply)` | 尚不支持（not supported yet） |
| `admin.assistant.default_model` | `object` | 尚不支持（not supported yet） |
| `admin.assistant.default_model.model` | `string, required` | 尚不支持（not supported yet） |
| `admin.assistant.default_model.provider` | `string, required` | 尚不支持（not supported yet） |
| `admin.assistant.limits` | `object` | 尚不支持（not supported yet） |
| `admin.assistant.limits.max_audit_entries` | `integer` | 尚不支持（not supported yet） |
| `admin.assistant.limits.max_concurrent_chats` | `integer` | 尚不支持（not supported yet） |
| `admin.assistant.limits.max_conversations` | `integer` | 尚不支持（not supported yet） |
| `admin.assistant.limits.max_stream_bytes` | `integer` | 尚不支持（not supported yet） |
| `admin.assistant.limits.max_tokens_per_day` | `integer` | 尚不支持（not supported yet） |
| `admin.assistant.limits.max_tool_calls` | `integer` | 尚不支持（not supported yet） |
| `admin.assistant.limits.max_tool_result_bytes` | `integer` | 尚不支持（not supported yet） |
| `admin.assistant.limits.max_turns` | `integer` | 尚不支持（not supported yet） |
| `admin.assistant.mcp` | `object` | 尚不支持（not supported yet） |
| `admin.assistant.mcp.allow` | `array<string>` | 尚不支持（not supported yet） |
| `admin.assistant.mcp.max_calls_per_minute` | `integer` | 尚不支持（not supported yet） |
| `admin.assistant.mcp.max_concurrent_calls` | `integer` | 尚不支持（not supported yet） |
| `admin.assistant.mcp.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `admin.assistant.mcp.max_sessions` | `integer` | 尚不支持（not supported yet） |
| `admin.assistant.mcp.max_token_days` | `integer` | 尚不支持（not supported yet） |
| `admin.assistant.mcp.max_tokens_per_user` | `integer` | 尚不支持（not supported yet） |
| `admin.assistant.mcp.session_idle_timeout` | `duration` | 尚不支持（not supported yet） |
| `admin.assistant.params` | `object` | 尚不支持（not supported yet） |
| `admin.assistant.params.max_output_tokens` | `integer` | 尚不支持（not supported yet） |
| `admin.assistant.params.temperature` | `number` | 尚不支持（not supported yet） |
| `admin.assistant.params.thinking` | `enum(off\|low\|medium\|high)` | 尚不支持（not supported yet） |
| `admin.assistant.providers` | `array<object>` | 尚不支持（not supported yet） |
| `admin.assistant.providers[].api_key_env` | `string` | 尚不支持（not supported yet） |
| `admin.assistant.providers[].api_version` | `string` | 尚不支持（not supported yet） |
| `admin.assistant.providers[].base_url` | `string` | 尚不支持（not supported yet） |
| `admin.assistant.providers[].connect_timeout` | `duration` | 尚不支持（not supported yet） |
| `admin.assistant.providers[].first_byte_timeout` | `duration` | 尚不支持（not supported yet） |
| `admin.assistant.providers[].headers` | `map<string>` | 尚不支持（not supported yet） |
| `admin.assistant.providers[].idle_timeout` | `duration` | 尚不支持（not supported yet） |
| `admin.assistant.providers[].kind` | `enum(anthropic\|openai\|openai_responses\|gemini\|azure_openai\|ollama\|openai_compatible\|llm_proxy), required` | 尚不支持（not supported yet） |
| `admin.assistant.providers[].models` | `array<object>` | 尚不支持（not supported yet） |
| `admin.assistant.providers[].models[].context` | `integer` | 尚不支持（not supported yet） |
| `admin.assistant.providers[].models[].name` | `string, required` | 尚不支持（not supported yet） |
| `admin.assistant.providers[].models[].tools` | `boolean` | 尚不支持（not supported yet） |
| `admin.assistant.providers[].name` | `string, required` | 尚不支持（not supported yet） |
| `admin.assistant.providers[].retries` | `integer` | 尚不支持（not supported yet） |
| `admin.assistant.providers[].timeout` | `duration` | 尚不支持（not supported yet） |
| `admin.assistant.users` | `array<object>` | 尚不支持（not supported yet） |
| `admin.assistant.users[].level` | `enum(off\|read\|edit\|propose\|apply), required` | 尚不支持（not supported yet） |
| `admin.assistant.users[].username` | `string, required` | 尚不支持（not supported yet） |
| `admin.cert_file` | `string` | 已支持 |
| `admin.client_ca_file` | `string` | 已支持 |
| `admin.cluster` | `object` | 已支持 |
| `admin.cluster.admins` | `array<string>` | 已支持 |
| `admin.cluster.link_address` | `string` | 已支持 |
| `admin.cluster.link_url` | `string` | 已支持 |
| `admin.cluster.max_concurrency` | `integer` | 已支持 |
| `admin.cluster.max_nodes` | `integer` | 已支持 |
| `admin.cluster.max_response_bytes` | `integer` | 已支持 |
| `admin.cluster.poll_interval` | `duration` | 已支持 |
| `admin.cluster.timeout` | `duration` | 已支持 |
| `admin.cluster_member` | `object` | 已支持 |
| `admin.cluster_member.fingerprint` | `string, required` | 已支持 |
| `admin.cluster_member.manager` | `string, required` | 已支持 |
| `admin.cluster_member.token` | `string` | 已支持 |
| `admin.cluster_member.token_file` | `string` | 已支持 |
| `admin.console` | `object` | 已支持 |
| `admin.console.access` | `object` | 已支持 |
| `admin.console.access.allow` | `array<string>` | 已支持 |
| `admin.console.access.allow_files` | `array<string>` | 已支持 |
| `admin.console.access.deny` | `array<string>` | 已支持 |
| `admin.console.access.deny_files` | `array<string>` | 已支持 |
| `admin.console.address` | `string` | 已支持 |
| `admin.console.cert_file` | `string` | 已支持 |
| `admin.console.key_file` | `string` | 已支持 |
| `admin.console.max_conns` | `integer` | 已支持 |
| `admin.console.max_sessions` | `integer` | 已支持 |
| `admin.console.mptcp` | `boolean` | 尚不支持（not supported yet） |
| `admin.console.session_lifetime` | `duration` | 已支持 |
| `admin.console.session_timeout` | `duration` | 已支持 |
| `admin.console.users` | `array<object>, required` | 已支持 |
| `admin.console.users[].password_hash` | `string, required` | 已支持 |
| `admin.console.users[].username` | `string, required` | 已支持 |
| `admin.disable_dashboard` | `boolean` | 已支持 |
| `admin.history` | `integer` | 已支持 |
| `admin.key_file` | `string` | 已支持 |
| `admin.max_config_bytes` | `integer` | 已支持 |
| `admin.max_conns` | `integer` | 已支持 |
| `admin.mptcp` | `boolean` | 尚不支持（not supported yet） |
| `admin.node_id` | `string` | 已支持 |
| `admin.store` | `object` | 已支持 |
| `admin.store.password` | `string` | 已支持 |
| `admin.store.password_env` | `string` | 已支持 |
| `admin.store.password_file` | `string` | 已支持 |
| `admin.store.path` | `string, required` | 已支持 |
| `admin.token_file` | `string` | 已支持 |

<a id="fields-auto-https"></a>

#### `auto_https`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `auto_https` | `object` | 已支持 |
| `auto_https.disable` | `boolean` | 已支持 |
| `auto_https.disable_redirects` | `boolean` | 已支持 |
| `auto_https.http_port` | `integer` | 已支持 |
| `auto_https.https_port` | `integer` | 已支持 |

<a id="fields-bittorrent"></a>

#### `bittorrent`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `bittorrent` | `object` | 已支持 |
| `bittorrent.client` | `object` | 已支持 |
| `bittorrent.client.access` | `object` | 尚不支持（not supported yet） |
| `bittorrent.client.access.allow` | `array<string>` | 尚不支持（not supported yet） |
| `bittorrent.client.access.allow_files` | `array<string>` | 尚不支持（not supported yet） |
| `bittorrent.client.access.deny` | `array<string>` | 尚不支持（not supported yet） |
| `bittorrent.client.access.deny_files` | `array<string>` | 尚不支持（not supported yet） |
| `bittorrent.client.alt_speed` | `object` | 已支持 |
| `bittorrent.client.alt_speed.download_rate` | `integer` | 已支持 |
| `bittorrent.client.alt_speed.schedules` | `array<object>` | 已支持 |
| `bittorrent.client.alt_speed.schedules[].days` | `array<string>` | 已支持 |
| `bittorrent.client.alt_speed.schedules[].from` | `string, required` | 已支持 |
| `bittorrent.client.alt_speed.schedules[].to` | `string, required` | 已支持 |
| `bittorrent.client.alt_speed.time_zone` | `string` | 已支持 |
| `bittorrent.client.alt_speed.upload_rate` | `integer` | 已支持 |
| `bittorrent.client.categories` | `array<object>` | 已支持 |
| `bittorrent.client.categories[].dir` | `string` | 已支持 |
| `bittorrent.client.categories[].limits` | `object` | 已支持 |
| `bittorrent.client.categories[].limits.download_rate` | `integer` | 已支持 |
| `bittorrent.client.categories[].limits.max_conns` | `integer` | 尚不支持（not supported yet） |
| `bittorrent.client.categories[].limits.seed_ratio` | `number` | 已支持 |
| `bittorrent.client.categories[].limits.seed_time` | `duration` | 已支持 |
| `bittorrent.client.categories[].limits.upload_rate` | `integer` | 已支持 |
| `bittorrent.client.categories[].limits.upload_slots` | `integer` | 尚不支持（not supported yet） |
| `bittorrent.client.categories[].media_library` | `string` | 尚不支持（not supported yet） |
| `bittorrent.client.categories[].name` | `string, required` | 已支持 |
| `bittorrent.client.console` | `object` | 已支持 |
| `bittorrent.client.console.access` | `object` | 已支持 |
| `bittorrent.client.console.access.allow` | `array<string>` | 已支持 |
| `bittorrent.client.console.access.allow_files` | `array<string>` | 已支持 |
| `bittorrent.client.console.access.deny` | `array<string>` | 已支持 |
| `bittorrent.client.console.access.deny_files` | `array<string>` | 已支持 |
| `bittorrent.client.console.address` | `string, required` | 已支持 |
| `bittorrent.client.console.cert_file` | `string` | 已支持 |
| `bittorrent.client.console.key_file` | `string` | 已支持 |
| `bittorrent.client.console.max_conns` | `integer` | 已支持 |
| `bittorrent.client.console.max_sessions` | `integer` | 已支持 |
| `bittorrent.client.console.mptcp` | `boolean` | 尚不支持（not supported yet） |
| `bittorrent.client.console.session_lifetime` | `duration` | 已支持 |
| `bittorrent.client.console.session_timeout` | `duration` | 已支持 |
| `bittorrent.client.console.users` | `array<object>, required` | 已支持 |
| `bittorrent.client.console.users[].password_hash` | `string, required` | 已支持 |
| `bittorrent.client.console.users[].username` | `string, required` | 已支持 |
| `bittorrent.client.data_dir` | `string, required` | 已支持 |
| `bittorrent.client.dht` | `boolean` | 已支持 |
| `bittorrent.client.disk_queue_bytes` | `integer` | 尚不支持（not supported yet） |
| `bittorrent.client.disk_workers` | `integer` | 已支持 |
| `bittorrent.client.download_dir` | `string` | 已支持 |
| `bittorrent.client.encryption` | `enum(allow\|prefer\|require)` | 已支持 |
| `bittorrent.client.limits` | `object` | 已支持 |
| `bittorrent.client.limits.download_rate` | `integer` | 已支持 |
| `bittorrent.client.limits.max_active_downloads` | `integer` | 已支持 |
| `bittorrent.client.limits.max_active_seeds` | `integer` | 已支持 |
| `bittorrent.client.limits.max_conns` | `integer` | 尚不支持（not supported yet） |
| `bittorrent.client.limits.max_conns_per_torrent` | `integer` | 已支持 |
| `bittorrent.client.limits.max_half_open` | `integer` | 尚不支持（not supported yet） |
| `bittorrent.client.limits.max_torrents` | `integer` | 已支持 |
| `bittorrent.client.limits.min_free_bytes` | `integer` | 已支持 |
| `bittorrent.client.limits.seed_ratio` | `number` | 已支持 |
| `bittorrent.client.limits.seed_time` | `duration` | 已支持 |
| `bittorrent.client.limits.upload_rate` | `integer` | 已支持 |
| `bittorrent.client.limits.upload_slots` | `integer` | 已支持 |
| `bittorrent.client.listen` | `string, required` | 已支持 |
| `bittorrent.client.lsd` | `boolean` | 已支持 |
| `bittorrent.client.pex` | `boolean` | 已支持 |
| `bittorrent.client.publish` | `array<object>` | 尚不支持（not supported yet） |
| `bittorrent.client.publish[].announce` | `array<string>` | 尚不支持（not supported yet） |
| `bittorrent.client.publish[].check_interval` | `duration` | 尚不支持（not supported yet） |
| `bittorrent.client.publish[].dir` | `string, required` | 尚不支持（not supported yet） |
| `bittorrent.client.publish[].exclude` | `array<string>` | 尚不支持（not supported yet） |
| `bittorrent.client.publish[].files` | `array<string>` | 尚不支持（not supported yet） |
| `bittorrent.client.publish[].max_files` | `integer` | 尚不支持（not supported yet） |
| `bittorrent.client.publish[].max_hashing` | `integer` | 尚不支持（not supported yet） |
| `bittorrent.client.publish[].max_total_bytes` | `integer` | 尚不支持（not supported yet） |
| `bittorrent.client.publish[].mode` | `enum(per_file\|whole_dir)` | 尚不支持（not supported yet） |
| `bittorrent.client.publish[].name` | `string, required` | 尚不支持（not supported yet） |
| `bittorrent.client.publish[].piece_length` | `integer` | 尚不支持（not supported yet） |
| `bittorrent.client.publish[].private` | `boolean` | 尚不支持（not supported yet） |
| `bittorrent.client.publish[].web_seed` | `string` | 尚不支持（not supported yet） |
| `bittorrent.client.rss` | `object` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.feeds` | `array<object>` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.feeds[].enabled` | `boolean` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.feeds[].headers` | `map<string>` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.feeds[].headers_env` | `map<string>` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.feeds[].interval` | `duration` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.feeds[].max_items` | `integer` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.feeds[].name` | `string, required` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.feeds[].url` | `string, required` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.history_size` | `integer` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.max_feed_bytes` | `integer` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.rules` | `array<object>` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.rules[].category` | `string` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.rules[].enabled` | `boolean` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.rules[].episodes` | `boolean` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.rules[].exclude` | `array<string>` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.rules[].feeds` | `array<string>` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.rules[].max_per_poll` | `integer` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.rules[].max_size` | `integer` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.rules[].min_size` | `integer` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.rules[].name` | `string, required` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.rules[].patterns` | `array<string>` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.rules[].paused` | `boolean` | 尚不支持（not supported yet） |
| `bittorrent.client.rss.timeout` | `duration` | 尚不支持（not supported yet） |
| `bittorrent.client.save_interval` | `duration` | 已支持 |
| `bittorrent.client.url_timeout` | `duration` | 已支持 |
| `bittorrent.client.utp` | `boolean` | 已支持 |
| `bittorrent.client.watch_dir` | `string` | 已支持 |
| `bittorrent.client.watch_interval` | `duration` | 已支持 |
| `bittorrent.client.watch_max_files` | `integer` | 已支持 |
| `bittorrent.tracker` | `object` | 已支持 |
| `bittorrent.tracker.access` | `object` | 已支持 |
| `bittorrent.tracker.access.allow` | `array<string>` | 已支持 |
| `bittorrent.tracker.access.allow_files` | `array<string>` | 已支持 |
| `bittorrent.tracker.access.deny` | `array<string>` | 已支持 |
| `bittorrent.tracker.access.deny_files` | `array<string>` | 已支持 |
| `bittorrent.tracker.allow` | `array<string>` | 已支持 |
| `bittorrent.tracker.allow_files` | `array<string>` | 已支持 |
| `bittorrent.tracker.auth` | `string` | 尚不支持（not supported yet） |
| `bittorrent.tracker.console` | `object` | 已支持 |
| `bittorrent.tracker.console.access` | `object` | 已支持 |
| `bittorrent.tracker.console.access.allow` | `array<string>` | 已支持 |
| `bittorrent.tracker.console.access.allow_files` | `array<string>` | 已支持 |
| `bittorrent.tracker.console.access.deny` | `array<string>` | 已支持 |
| `bittorrent.tracker.console.access.deny_files` | `array<string>` | 已支持 |
| `bittorrent.tracker.console.address` | `string, required` | 已支持 |
| `bittorrent.tracker.console.cert_file` | `string` | 已支持 |
| `bittorrent.tracker.console.key_file` | `string` | 已支持 |
| `bittorrent.tracker.console.max_conns` | `integer` | 已支持 |
| `bittorrent.tracker.console.max_sessions` | `integer` | 已支持 |
| `bittorrent.tracker.console.mptcp` | `boolean` | 尚不支持（not supported yet） |
| `bittorrent.tracker.console.session_lifetime` | `duration` | 已支持 |
| `bittorrent.tracker.console.session_timeout` | `duration` | 已支持 |
| `bittorrent.tracker.console.users` | `array<object>, required` | 已支持 |
| `bittorrent.tracker.console.users[].password_hash` | `string, required` | 已支持 |
| `bittorrent.tracker.console.users[].username` | `string, required` | 已支持 |
| `bittorrent.tracker.interval` | `duration` | 已支持 |
| `bittorrent.tracker.listen` | `string` | 已支持 |
| `bittorrent.tracker.max_numwant` | `integer` | 已支持 |
| `bittorrent.tracker.max_peers` | `integer` | 尚不支持（not supported yet） |
| `bittorrent.tracker.max_peers_per_torrent` | `integer` | 已支持 |
| `bittorrent.tracker.max_scrape` | `integer` | 已支持 |
| `bittorrent.tracker.max_torrents` | `integer` | 已支持 |
| `bittorrent.tracker.min_interval` | `duration` | 已支持 |
| `bittorrent.tracker.peer_ttl` | `duration` | 已支持 |
| `bittorrent.tracker.store` | `object` | 尚不支持（not supported yet） |
| `bittorrent.tracker.store.cache_peers` | `integer` | 尚不支持（not supported yet） |
| `bittorrent.tracker.store.cache_torrents` | `integer` | 尚不支持（not supported yet） |
| `bittorrent.tracker.store.prefix` | `string` | 尚不支持（not supported yet） |
| `bittorrent.tracker.store.queue` | `integer` | 尚不支持（not supported yet） |
| `bittorrent.tracker.store.redis` | `object` | 尚不支持（not supported yet） |
| `bittorrent.tracker.store.redis.address` | `string` | 尚不支持（not supported yet） |
| `bittorrent.tracker.store.redis.ca_file` | `string` | 尚不支持（not supported yet） |
| `bittorrent.tracker.store.redis.cert_file` | `string` | 尚不支持（not supported yet） |
| `bittorrent.tracker.store.redis.db` | `integer` | 尚不支持（not supported yet） |
| `bittorrent.tracker.store.redis.key_file` | `string` | 尚不支持（not supported yet） |
| `bittorrent.tracker.store.redis.password_file` | `string` | 尚不支持（not supported yet） |
| `bittorrent.tracker.store.redis.tls` | `boolean` | 尚不支持（not supported yet） |
| `bittorrent.tracker.store.redis.username` | `string` | 尚不支持（not supported yet） |
| `bittorrent.tracker.store.refresh` | `duration` | 尚不支持（not supported yet） |
| `bittorrent.tracker.store.request_timeout` | `duration` | 尚不支持（not supported yet） |

<a id="fields-engine"></a>

#### `engine`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `engine` | `object` | 已支持 |
| `engine.backlog` | `integer` | 已支持 |
| `engine.event_loops` | `integer` | 已支持 |
| `engine.kernel_tls_offload` | `boolean` | 尚不支持（not supported yet） |
| `engine.loop_wait` | `enum(park\|block)` | 尚不支持（not supported yet） |
| `engine.min_workers` | `integer` | 已支持 |
| `engine.poll_batch` | `integer` | 尚不支持（not supported yet） |
| `engine.read_buffer_size` | `integer` | 已支持 |
| `engine.task_queue` | `integer` | 已支持 |
| `engine.workers` | `integer` | 已支持 |
| `engine.write_watermark` | `integer` | 已支持 |

<a id="fields-http-handlers-bt-publish"></a>

#### `http.handlers/bt_publish`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].handler<bt_publish>` | `module` | 尚不支持（not supported yet） |
| `routes[].handler<bt_publish>.params` | `object` | 尚不支持（not supported yet） |
| `routes[].handler<bt_publish>.params.publish` | `array<string>` | 尚不支持（not supported yet） |

<a id="fields-http-handlers-bt-stream"></a>

#### `http.handlers/bt_stream`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].handler<bt_stream>` | `module` | 尚不支持（not supported yet） |
| `routes[].handler<bt_stream>.params` | `object` | 尚不支持（not supported yet） |
| `routes[].handler<bt_stream>.params.auth` | `string` | 尚不支持（not supported yet） |
| `routes[].handler<bt_stream>.params.cache_control` | `string` | 尚不支持（not supported yet） |
| `routes[].handler<bt_stream>.params.cors` | `string` | 尚不支持（not supported yet） |
| `routes[].handler<bt_stream>.params.info_timeout` | `duration` | 尚不支持（not supported yet） |
| `routes[].handler<bt_stream>.params.magnet` | `boolean` | 尚不支持（not supported yet） |
| `routes[].handler<bt_stream>.params.max_streams` | `integer` | 尚不支持（not supported yet） |
| `routes[].handler<bt_stream>.params.max_temporary` | `integer` | 尚不支持（not supported yet） |
| `routes[].handler<bt_stream>.params.max_temporary_bytes` | `integer` | 尚不支持（not supported yet） |
| `routes[].handler<bt_stream>.params.prefix` | `string` | 尚不支持（not supported yet） |
| `routes[].handler<bt_stream>.params.read_timeout` | `duration` | 尚不支持（not supported yet） |
| `routes[].handler<bt_stream>.params.readahead_bytes` | `integer` | 尚不支持（not supported yet） |
| `routes[].handler<bt_stream>.params.temporary_ttl` | `duration` | 尚不支持（not supported yet） |

<a id="fields-http-handlers-bt-tracker"></a>

#### `http.handlers/bt_tracker`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].handler<bt_tracker>` | `module` | 已支持 |
| `routes[].handler<bt_tracker>.params` | `object` | 已支持 |

<a id="fields-http-handlers-fastcgi"></a>

#### `http.handlers/fastcgi`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].handler<fastcgi>` | `module` | 已支持 |
| `routes[].handler<fastcgi>.params` | `object` | 已支持 |
| `routes[].handler<fastcgi>.params.address` | `string, required` | 已支持 |
| `routes[].handler<fastcgi>.params.connect_timeout` | `duration` | 已支持 |
| `routes[].handler<fastcgi>.params.dotfiles` | `enum(deny\|allow)` | 已支持 |
| `routes[].handler<fastcgi>.params.env` | `map<string>` | 已支持 |
| `routes[].handler<fastcgi>.params.idle_timeout` | `duration` | 已支持 |
| `routes[].handler<fastcgi>.params.index` | `string` | 已支持 |
| `routes[].handler<fastcgi>.params.max_conns` | `integer` | 已支持 |
| `routes[].handler<fastcgi>.params.max_header_bytes` | `integer` | 已支持 |
| `routes[].handler<fastcgi>.params.max_idle_conns` | `integer` | 已支持 |
| `routes[].handler<fastcgi>.params.max_pending` | `integer` | 已支持 |
| `routes[].handler<fastcgi>.params.response_header_timeout` | `duration` | 已支持 |
| `routes[].handler<fastcgi>.params.response_idle_timeout` | `duration` | 已支持 |
| `routes[].handler<fastcgi>.params.root` | `string, required` | 已支持 |
| `routes[].handler<fastcgi>.params.split_path` | `array<string>` | 已支持 |
| `routes[].handler<fastcgi>.params.timeout` | `duration` | 已支持 |
| `routes[].handler<fastcgi>.params.try_files` | `array<string>` | 已支持 |

<a id="fields-http-handlers-forward-proxy"></a>

#### `http.handlers/forward_proxy`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].handler<forward_proxy>` | `module` | 已支持 |
| `routes[].handler<forward_proxy>.params` | `object` | 已支持 |
| `routes[].handler<forward_proxy>.params.allow` | `array<string>` | 已支持 |
| `routes[].handler<forward_proxy>.params.clients` | `array<string>, required` | 已支持 |
| `routes[].handler<forward_proxy>.params.connect_timeout` | `duration` | 已支持 |
| `routes[].handler<forward_proxy>.params.deny` | `array<string>` | 已支持 |
| `routes[].handler<forward_proxy>.params.idle_timeout` | `duration` | 已支持 |
| `routes[].handler<forward_proxy>.params.ports` | `array<integer>` | 已支持 |

<a id="fields-http-handlers-hls"></a>

#### `http.handlers/hls`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].handler<hls>` | `module` | 已支持 |
| `routes[].handler<hls>.params` | `object` | 已支持 |
| `routes[].handler<hls>.params.auth` | `string` | 已支持 |
| `routes[].handler<hls>.params.cors` | `string` | 已支持 |
| `routes[].handler<hls>.params.format` | `enum(fmp4\|ts)` | 已支持 |
| `routes[].handler<hls>.params.idle_timeout` | `duration` | 已支持 |
| `routes[].handler<hls>.params.max_bytes` | `integer` | 已支持 |
| `routes[].handler<hls>.params.part_duration` | `duration` | 已支持 |
| `routes[].handler<hls>.params.play_wait` | `duration` | 已支持 |
| `routes[].handler<hls>.params.playlist_size` | `integer` | 已支持 |
| `routes[].handler<hls>.params.prefix` | `string` | 已支持 |
| `routes[].handler<hls>.params.target_duration` | `duration` | 已支持 |

<a id="fields-http-handlers-httpflv"></a>

#### `http.handlers/httpflv`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].handler<httpflv>` | `module` | 已支持 |
| `routes[].handler<httpflv>.params` | `object` | 已支持 |
| `routes[].handler<httpflv>.params.auth` | `string` | 已支持 |
| `routes[].handler<httpflv>.params.cors` | `string` | 已支持 |
| `routes[].handler<httpflv>.params.prefix` | `string` | 已支持 |
| `routes[].handler<httpflv>.params.wait` | `duration` | 已支持 |

<a id="fields-http-handlers-media-pages"></a>

#### `http.handlers/media_pages`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].handler<media_pages>` | `module` | 已支持 |
| `routes[].handler<media_pages>.params` | `object` | 已支持 |
| `routes[].handler<media_pages>.params.auth` | `string` | 已支持 |
| `routes[].handler<media_pages>.params.cam_default` | `boolean` | 已支持 |
| `routes[].handler<media_pages>.params.channels` | `boolean` | 尚不支持（not supported yet） |
| `routes[].handler<media_pages>.params.chat` | `boolean` | 已支持 |
| `routes[].handler<media_pages>.params.hls` | `string` | 已支持 |
| `routes[].handler<media_pages>.params.ice_servers` | `array<object>` | 已支持 |
| `routes[].handler<media_pages>.params.ice_servers[].credential` | `string` | 已支持 |
| `routes[].handler<media_pages>.params.ice_servers[].urls` | `array<string>, required` | 已支持 |
| `routes[].handler<media_pages>.params.ice_servers[].username` | `string` | 已支持 |
| `routes[].handler<media_pages>.params.libraries` | `array<string>` | 尚不支持（not supported yet） |
| `routes[].handler<media_pages>.params.links` | `array<object>` | 已支持 |
| `routes[].handler<media_pages>.params.links[].name` | `string, required` | 已支持 |
| `routes[].handler<media_pages>.params.links[].url` | `string, required` | 已支持 |
| `routes[].handler<media_pages>.params.login` | `boolean` | 已支持 |
| `routes[].handler<media_pages>.params.max_file_bytes` | `integer` | 已支持 |
| `routes[].handler<media_pages>.params.max_members` | `integer` | 已支持 |
| `routes[].handler<media_pages>.params.max_message_bytes` | `integer` | 已支持 |
| `routes[].handler<media_pages>.params.max_rooms` | `integer` | 已支持 |
| `routes[].handler<media_pages>.params.member_ttl` | `duration` | 已支持 |
| `routes[].handler<media_pages>.params.mic_default` | `boolean` | 已支持 |
| `routes[].handler<media_pages>.params.pages` | `boolean` | 已支持 |
| `routes[].handler<media_pages>.params.prefix` | `string` | 已支持 |
| `routes[].handler<media_pages>.params.stall_timeout` | `duration` | 已支持 |
| `routes[].handler<media_pages>.params.title` | `string` | 已支持 |
| `routes[].handler<media_pages>.params.vod` | `string` | 尚不支持（not supported yet） |
| `routes[].handler<media_pages>.params.whep` | `string` | 已支持 |
| `routes[].handler<media_pages>.params.whip` | `string` | 已支持 |

<a id="fields-http-handlers-media-tokens"></a>

#### `http.handlers/media_tokens`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].handler<media_tokens>` | `module` | 尚不支持（not supported yet） |
| `routes[].handler<media_tokens>.params` | `object` | 尚不支持（not supported yet） |
| `routes[].handler<media_tokens>.params.auth` | `string` | 尚不支持（not supported yet） |
| `routes[].handler<media_tokens>.params.max_body_bytes` | `integer` | 尚不支持（not supported yet） |
| `routes[].handler<media_tokens>.params.max_tokens` | `integer` | 尚不支持（not supported yet） |
| `routes[].handler<media_tokens>.params.max_ttl` | `duration` | 尚不支持（not supported yet） |
| `routes[].handler<media_tokens>.params.prefix` | `string` | 尚不支持（not supported yet） |
| `routes[].handler<media_tokens>.params.prefixes` | `array<string>` | 尚不支持（not supported yet） |
| `routes[].handler<media_tokens>.params.revoke_timeout` | `duration` | 尚不支持（not supported yet） |
| `routes[].handler<media_tokens>.params.url_base` | `string` | 尚不支持（not supported yet） |
| `routes[].handler<media_tokens>.params.url_prefixes` | `array<string>` | 尚不支持（not supported yet） |

<a id="fields-http-handlers-proxy"></a>

#### `http.handlers/proxy`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].handler<proxy>` | `module` | 已支持 |
| `routes[].handler<proxy>.params` | `object` | 已支持 |
| `routes[].handler<proxy>.params.failover` | `string` | 已支持 |
| `routes[].handler<proxy>.params.host_header` | `string` | 已支持 |
| `routes[].handler<proxy>.params.mirror` | `object` | 已支持 |
| `routes[].handler<proxy>.params.mirror.percent` | `number` | 已支持 |
| `routes[].handler<proxy>.params.mirror.upstream` | `string, required` | 已支持 |
| `routes[].handler<proxy>.params.retry` | `object` | 已支持 |
| `routes[].handler<proxy>.params.retry.attempts` | `integer` | 已支持 |
| `routes[].handler<proxy>.params.retry.budget_min_per_second` | `number` | 已支持 |
| `routes[].handler<proxy>.params.retry.budget_ratio` | `number` | 已支持 |
| `routes[].handler<proxy>.params.retry.statuses` | `array<integer>` | 已支持 |
| `routes[].handler<proxy>.params.splice_uploads` | `boolean` | 尚不支持（not supported yet） |
| `routes[].handler<proxy>.params.timeout` | `duration` | 已支持 |
| `routes[].handler<proxy>.params.trusted_proxies` | `array<string>` | 已支持 |
| `routes[].handler<proxy>.params.tunnel_idle_timeout` | `duration` | 已支持 |
| `routes[].handler<proxy>.params.upstream` | `string` | 已支持 |
| `routes[].handler<proxy>.params.upstreams` | `array<object>` | 已支持 |
| `routes[].handler<proxy>.params.upstreams[].upstream` | `string, required` | 已支持 |
| `routes[].handler<proxy>.params.upstreams[].weight` | `integer, required` | 已支持 |

<a id="fields-http-handlers-respond"></a>

#### `http.handlers/respond`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].handler<respond>` | `module` | 已支持 |
| `routes[].handler<respond>.params` | `object` | 已支持 |
| `routes[].handler<respond>.params.body` | `string` | 已支持 |
| `routes[].handler<respond>.params.headers` | `map<string>` | 已支持 |
| `routes[].handler<respond>.params.status` | `integer` | 已支持 |

<a id="fields-http-handlers-static"></a>

#### `http.handlers/static`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].handler<static>` | `module` | 已支持 |
| `routes[].handler<static>.params` | `object` | 已支持 |
| `routes[].handler<static>.params.browse` | `boolean` | 已支持 |
| `routes[].handler<static>.params.browse_max_entries` | `integer` | 已支持 |
| `routes[].handler<static>.params.check_interval` | `duration` | 已支持 |
| `routes[].handler<static>.params.compress` | `array<enum(zstd\|br\|gzip)>` | 已支持 |
| `routes[].handler<static>.params.default_type` | `string` | 已支持 |
| `routes[].handler<static>.params.disk_io` | `enum(pool\|auto)` | 已支持 |
| `routes[].handler<static>.params.dotfiles` | `enum(deny\|allow)` | 已支持 |
| `routes[].handler<static>.params.index` | `array<string>` | 已支持 |
| `routes[].handler<static>.params.max_cache_bytes` | `integer` | 已支持 |
| `routes[].handler<static>.params.max_file_bytes` | `integer` | 已支持 |
| `routes[].handler<static>.params.precompressed` | `array<enum(br\|zstd\|gzip)>` | 已支持 |
| `routes[].handler<static>.params.root` | `string, required` | 已支持 |
| `routes[].handler<static>.params.strip_prefix` | `string` | 已支持 |
| `routes[].handler<static>.params.try_files` | `array<string>` | 已支持 |

<a id="fields-http-handlers-stream-whep"></a>

#### `http.handlers/stream_whep`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].handler<stream_whep>` | `module` | 已支持 |
| `routes[].handler<stream_whep>.params` | `object` | 已支持 |
| `routes[].handler<stream_whep>.params.auth` | `string` | 已支持 |
| `routes[].handler<stream_whep>.params.prefix` | `string` | 已支持 |
| `routes[].handler<stream_whep>.params.stream` | `string` | 已支持 |

<a id="fields-http-handlers-stream-whip"></a>

#### `http.handlers/stream_whip`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].handler<stream_whip>` | `module` | 已支持 |
| `routes[].handler<stream_whip>.params` | `object` | 已支持 |
| `routes[].handler<stream_whip>.params.auth` | `string` | 已支持 |
| `routes[].handler<stream_whip>.params.prefix` | `string` | 已支持 |
| `routes[].handler<stream_whip>.params.stream` | `string` | 已支持 |

<a id="fields-http-handlers-sync-ingest"></a>

#### `http.handlers/sync_ingest`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].handler<sync_ingest>` | `module` | 尚不支持（not supported yet） |
| `routes[].handler<sync_ingest>.params` | `object` | 尚不支持（not supported yet） |
| `routes[].handler<sync_ingest>.params.max_body_bytes` | `integer` | 尚不支持（not supported yet） |
| `routes[].handler<sync_ingest>.params.pipeline` | `string, required` | 尚不支持（not supported yet） |
| `routes[].handler<sync_ingest>.params.retry_after` | `duration` | 尚不支持（not supported yet） |
| `routes[].handler<sync_ingest>.params.timeout` | `duration` | 尚不支持（not supported yet） |

<a id="fields-http-handlers-vod"></a>

#### `http.handlers/vod`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].handler<vod>` | `module` | 尚不支持（not supported yet） |
| `routes[].handler<vod>.params` | `object` | 尚不支持（not supported yet） |
| `routes[].handler<vod>.params.auth` | `string` | 尚不支持（not supported yet） |
| `routes[].handler<vod>.params.cache_control` | `string` | 尚不支持（not supported yet） |
| `routes[].handler<vod>.params.container` | `enum(fmp4\|ts)` | 尚不支持（not supported yet） |
| `routes[].handler<vod>.params.cors` | `string` | 尚不支持（not supported yet） |
| `routes[].handler<vod>.params.encrypt` | `boolean` | 尚不支持（not supported yet） |
| `routes[].handler<vod>.params.library` | `string, required` | 尚不支持（not supported yet） |
| `routes[].handler<vod>.params.max_cache_bytes` | `integer` | 尚不支持（not supported yet） |
| `routes[].handler<vod>.params.max_segment_bytes` | `integer` | 尚不支持（not supported yet） |
| `routes[].handler<vod>.params.prefix` | `string` | 尚不支持（not supported yet） |
| `routes[].handler<vod>.params.segment_duration` | `duration` | 尚不支持（not supported yet） |
| `routes[].handler<vod>.params.timeout` | `duration` | 尚不支持（not supported yet） |

<a id="fields-http-handlers-webrtc-proxy"></a>

#### `http.handlers/webrtc_proxy`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].handler<webrtc_proxy>` | `module` | 已支持 |
| `routes[].handler<webrtc_proxy>.params` | `object` | 已支持 |
| `routes[].handler<webrtc_proxy>.params.action` | `enum(publish\|play)` | 已支持 |
| `routes[].handler<webrtc_proxy>.params.auth` | `string` | 已支持 |
| `routes[].handler<webrtc_proxy>.params.max_sdp_bytes` | `integer` | 已支持 |
| `routes[].handler<webrtc_proxy>.params.media_addrs` | `map<string>` | 已支持 |
| `routes[].handler<webrtc_proxy>.params.timeout` | `duration` | 已支持 |
| `routes[].handler<webrtc_proxy>.params.upstream` | `string, required` | 已支持 |

<a id="fields-http-handlers-websockify"></a>

#### `http.handlers/websockify`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].handler<websockify>` | `module` | 尚不支持（not supported yet） |
| `routes[].handler<websockify>.params` | `object` | 尚不支持（not supported yet） |
| `routes[].handler<websockify>.params.address` | `string` | 尚不支持（not supported yet） |
| `routes[].handler<websockify>.params.connect_timeout` | `duration` | 尚不支持（not supported yet） |
| `routes[].handler<websockify>.params.idle_timeout` | `duration` | 尚不支持（not supported yet） |
| `routes[].handler<websockify>.params.max_conns` | `integer` | 尚不支持（not supported yet） |
| `routes[].handler<websockify>.params.max_frame_bytes` | `integer` | 尚不支持（not supported yet） |
| `routes[].handler<websockify>.params.name` | `string` | 尚不支持（not supported yet） |
| `routes[].handler<websockify>.params.origins` | `array<string>` | 尚不支持（not supported yet） |
| `routes[].handler<websockify>.params.token_param` | `string` | 尚不支持（not supported yet） |
| `routes[].handler<websockify>.params.tokens` | `map<string>` | 尚不支持（not supported yet） |
| `routes[].handler<websockify>.params.upstream` | `string` | 尚不支持（not supported yet） |

<a id="fields-http-handlers-whep"></a>

#### `http.handlers/whep`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].handler<whep>` | `module` | 已支持 |
| `routes[].handler<whep>.params` | `object` | 已支持 |
| `routes[].handler<whep>.params.auth` | `string` | 已支持 |
| `routes[].handler<whep>.params.stream` | `string` | 已支持 |

<a id="fields-http-handlers-whip"></a>

#### `http.handlers/whip`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].handler<whip>` | `module` | 已支持 |
| `routes[].handler<whip>.params` | `object` | 已支持 |
| `routes[].handler<whip>.params.auth` | `string` | 已支持 |
| `routes[].handler<whip>.params.stream` | `string` | 已支持 |

<a id="fields-http-middleware-basic-auth"></a>

#### `http.middleware/basic_auth`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].middleware[]<basic_auth>` | `module` | 已支持 |
| `routes[].middleware[]<basic_auth>.params` | `object` | 已支持 |
| `routes[].middleware[]<basic_auth>.params.forward_authorization` | `boolean` | 已支持 |
| `routes[].middleware[]<basic_auth>.params.realm` | `string` | 已支持 |
| `routes[].middleware[]<basic_auth>.params.user_header` | `string` | 已支持 |
| `routes[].middleware[]<basic_auth>.params.users` | `array<object>, required` | 已支持 |
| `routes[].middleware[]<basic_auth>.params.users[].password_hash` | `string, required` | 已支持 |
| `routes[].middleware[]<basic_auth>.params.users[].username` | `string, required` | 已支持 |

<a id="fields-http-middleware-body-limit"></a>

#### `http.middleware/body_limit`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].middleware[]<body_limit>` | `module` | 已支持 |
| `routes[].middleware[]<body_limit>.params` | `object` | 已支持 |
| `routes[].middleware[]<body_limit>.params.max_bytes` | `integer, required` | 已支持 |

<a id="fields-http-middleware-cache"></a>

#### `http.middleware/cache`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].middleware[]<cache>` | `module` | 已支持 |
| `routes[].middleware[]<cache>.params` | `object` | 已支持 |
| `routes[].middleware[]<cache>.params.default_ttl` | `duration` | 已支持 |
| `routes[].middleware[]<cache>.params.disable_lock` | `boolean` | 已支持 |
| `routes[].middleware[]<cache>.params.disk` | `object` | 已支持 |
| `routes[].middleware[]<cache>.params.disk.max_bytes` | `integer` | 已支持 |
| `routes[].middleware[]<cache>.params.disk.max_entries` | `integer` | 已支持 |
| `routes[].middleware[]<cache>.params.disk.max_size` | `integer, deprecated` | 已支持 |
| `routes[].middleware[]<cache>.params.disk.path` | `string, required` | 已支持 |
| `routes[].middleware[]<cache>.params.lock_timeout` | `duration` | 已支持 |
| `routes[].middleware[]<cache>.params.max_bytes` | `integer` | 已支持 |
| `routes[].middleware[]<cache>.params.max_entry_bytes` | `integer` | 已支持 |
| `routes[].middleware[]<cache>.params.max_entry_size` | `integer, deprecated` | 已支持 |
| `routes[].middleware[]<cache>.params.max_size` | `integer, deprecated` | 已支持 |
| `routes[].middleware[]<cache>.params.stale_if_error` | `duration` | 已支持 |
| `routes[].middleware[]<cache>.params.stale_while_revalidate` | `duration` | 已支持 |
| `routes[].middleware[]<cache>.params.zone` | `string` | 已支持 |

<a id="fields-http-middleware-circuit-breaker"></a>

#### `http.middleware/circuit_breaker`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].middleware[]<circuit_breaker>` | `module` | 已支持 |
| `routes[].middleware[]<circuit_breaker>.params` | `object` | 已支持 |
| `routes[].middleware[]<circuit_breaker>.params.error_ratio` | `number` | 已支持 |
| `routes[].middleware[]<circuit_breaker>.params.half_open_requests` | `integer` | 已支持 |
| `routes[].middleware[]<circuit_breaker>.params.min_requests` | `integer` | 已支持 |
| `routes[].middleware[]<circuit_breaker>.params.open_duration` | `duration` | 已支持 |
| `routes[].middleware[]<circuit_breaker>.params.slow_ratio` | `number` | 已支持 |
| `routes[].middleware[]<circuit_breaker>.params.slow_threshold` | `duration` | 已支持 |
| `routes[].middleware[]<circuit_breaker>.params.statuses` | `array<integer>` | 已支持 |
| `routes[].middleware[]<circuit_breaker>.params.window` | `duration` | 已支持 |

<a id="fields-http-middleware-client-cert"></a>

#### `http.middleware/client_cert`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].middleware[]<client_cert>` | `module` | 已支持 |
| `routes[].middleware[]<client_cert>.params` | `object` | 已支持 |
| `routes[].middleware[]<client_cert>.params.optional` | `boolean` | 已支持 |
| `routes[].middleware[]<client_cert>.params.subject_header` | `string` | 已支持 |

<a id="fields-http-middleware-compress"></a>

#### `http.middleware/compress`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].middleware[]<compress>` | `module` | 已支持 |
| `routes[].middleware[]<compress>.params` | `object` | 已支持 |
| `routes[].middleware[]<compress>.params.brotli_level` | `integer` | 已支持 |
| `routes[].middleware[]<compress>.params.encodings` | `array<enum(zstd\|br\|gzip)>` | 已支持 |
| `routes[].middleware[]<compress>.params.gzip_level` | `integer` | 已支持 |
| `routes[].middleware[]<compress>.params.level` | `enum(fastest\|default\|better)` | 已支持 |
| `routes[].middleware[]<compress>.params.max_in_flight` | `integer` | 已支持 |
| `routes[].middleware[]<compress>.params.min_bytes` | `integer` | 已支持 |
| `routes[].middleware[]<compress>.params.min_length` | `integer, deprecated` | 已支持 |
| `routes[].middleware[]<compress>.params.zstd_level` | `integer` | 已支持 |

<a id="fields-http-middleware-concurrency-limit"></a>

#### `http.middleware/concurrency_limit`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].middleware[]<concurrency_limit>` | `module` | 已支持 |
| `routes[].middleware[]<concurrency_limit>.params` | `object` | 已支持 |
| `routes[].middleware[]<concurrency_limit>.params.max` | `integer, required` | 已支持 |
| `routes[].middleware[]<concurrency_limit>.params.status` | `integer` | 已支持 |

<a id="fields-http-middleware-forward-auth"></a>

#### `http.middleware/forward_auth`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].middleware[]<forward_auth>` | `module` | 已支持 |
| `routes[].middleware[]<forward_auth>.params` | `object` | 已支持 |
| `routes[].middleware[]<forward_auth>.params.copy_headers` | `array<string>` | 已支持 |
| `routes[].middleware[]<forward_auth>.params.request_headers` | `array<string>` | 已支持 |
| `routes[].middleware[]<forward_auth>.params.timeout` | `duration` | 已支持 |
| `routes[].middleware[]<forward_auth>.params.upstream` | `string, required` | 已支持 |
| `routes[].middleware[]<forward_auth>.params.uri` | `string` | 已支持 |

<a id="fields-http-middleware-headers"></a>

#### `http.middleware/headers`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].middleware[]<headers>` | `module` | 已支持 |
| `routes[].middleware[]<headers>.params` | `object` | 已支持 |
| `routes[].middleware[]<headers>.params.request` | `object` | 已支持 |
| `routes[].middleware[]<headers>.params.request.add` | `map<array<string>>` | 已支持 |
| `routes[].middleware[]<headers>.params.request.delete` | `array<string>` | 已支持 |
| `routes[].middleware[]<headers>.params.request.set` | `map<string>` | 已支持 |
| `routes[].middleware[]<headers>.params.response` | `object` | 已支持 |
| `routes[].middleware[]<headers>.params.response.add` | `map<array<string>>` | 已支持 |
| `routes[].middleware[]<headers>.params.response.delete` | `array<string>` | 已支持 |
| `routes[].middleware[]<headers>.params.response.set` | `map<string>` | 已支持 |

<a id="fields-http-middleware-jwt"></a>

#### `http.middleware/jwt`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].middleware[]<jwt>` | `module` | 已支持 |
| `routes[].middleware[]<jwt>.params` | `object` | 已支持 |
| `routes[].middleware[]<jwt>.params.algorithms` | `array<enum(RS256\|RS384\|RS512\|PS256\|PS384\|PS512\|ES256\|ES384\|ES512\|EdDSA\|HS256\|HS384\|HS512)>` | 已支持 |
| `routes[].middleware[]<jwt>.params.audience` | `array<string>` | 已支持 |
| `routes[].middleware[]<jwt>.params.claims_headers` | `map<string>` | 已支持 |
| `routes[].middleware[]<jwt>.params.hmac_secret_file` | `string` | 已支持 |
| `routes[].middleware[]<jwt>.params.issuer` | `string` | 已支持 |
| `routes[].middleware[]<jwt>.params.jwks_refresh` | `duration, deprecated` | 已支持 |
| `routes[].middleware[]<jwt>.params.jwks_refresh_interval` | `duration` | 已支持 |
| `routes[].middleware[]<jwt>.params.jwks_url` | `string` | 已支持 |
| `routes[].middleware[]<jwt>.params.leeway` | `duration` | 已支持 |
| `routes[].middleware[]<jwt>.params.public_key_files` | `array<string>` | 已支持 |
| `routes[].middleware[]<jwt>.params.token_from` | `string` | 已支持 |

<a id="fields-http-middleware-rate-limit"></a>

#### `http.middleware/rate_limit`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].middleware[]<rate_limit>` | `module` | 已支持 |
| `routes[].middleware[]<rate_limit>.params` | `object` | 已支持 |
| `routes[].middleware[]<rate_limit>.params.burst` | `integer` | 已支持 |
| `routes[].middleware[]<rate_limit>.params.ipv6_prefix` | `integer` | 已支持 |
| `routes[].middleware[]<rate_limit>.params.key` | `string` | 已支持 |
| `routes[].middleware[]<rate_limit>.params.max_keys` | `integer` | 已支持 |
| `routes[].middleware[]<rate_limit>.params.rate` | `number, required` | 已支持 |
| `routes[].middleware[]<rate_limit>.params.status` | `integer` | 已支持 |
| `routes[].middleware[]<rate_limit>.params.trusted_proxies` | `array<string>` | 已支持 |

<a id="fields-http-middleware-redirect"></a>

#### `http.middleware/redirect`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].middleware[]<redirect>` | `module` | 已支持 |
| `routes[].middleware[]<redirect>.params` | `object` | 已支持 |
| `routes[].middleware[]<redirect>.params.regexp` | `string` | 已支持 |
| `routes[].middleware[]<redirect>.params.status` | `integer` | 已支持 |
| `routes[].middleware[]<redirect>.params.to` | `string, required` | 已支持 |

<a id="fields-http-middleware-rewrite"></a>

#### `http.middleware/rewrite`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].middleware[]<rewrite>` | `module` | 已支持 |
| `routes[].middleware[]<rewrite>.params` | `object` | 已支持 |
| `routes[].middleware[]<rewrite>.params.add_prefix` | `string` | 已支持 |
| `routes[].middleware[]<rewrite>.params.regexp` | `string` | 已支持 |
| `routes[].middleware[]<rewrite>.params.replacement` | `string` | 已支持 |
| `routes[].middleware[]<rewrite>.params.strip_prefix` | `array<string>` | 已支持 |

<a id="fields-http-middleware-wasm"></a>

#### `http.middleware/wasm`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes[].middleware[]<wasm>` | `module` | 已支持 |
| `routes[].middleware[]<wasm>.params` | `object` | 已支持 |
| `routes[].middleware[]<wasm>.params.config` | `string` | 已支持 |
| `routes[].middleware[]<wasm>.params.instances` | `integer` | 已支持 |
| `routes[].middleware[]<wasm>.params.max_body_bytes` | `integer` | 已支持 |
| `routes[].middleware[]<wasm>.params.max_memory` | `integer, deprecated` | 已支持 |
| `routes[].middleware[]<wasm>.params.max_memory_bytes` | `integer` | 已支持 |
| `routes[].middleware[]<wasm>.params.max_streams` | `integer` | 已支持 |
| `routes[].middleware[]<wasm>.params.on_error` | `enum(fail\|skip)` | 已支持 |
| `routes[].middleware[]<wasm>.params.path` | `string, required` | 已支持 |
| `routes[].middleware[]<wasm>.params.properties` | `array<object>` | 已支持 |
| `routes[].middleware[]<wasm>.params.properties[].path` | `array<string>, required` | 已支持 |
| `routes[].middleware[]<wasm>.params.properties[].value` | `string` | 已支持 |
| `routes[].middleware[]<wasm>.params.queue` | `integer` | 已支持 |
| `routes[].middleware[]<wasm>.params.response` | `enum(all\|headers\|none)` | 已支持 |
| `routes[].middleware[]<wasm>.params.timeout` | `duration` | 已支持 |
| `routes[].middleware[]<wasm>.params.vm_config` | `string` | 已支持 |
| `routes[].middleware[]<wasm>.params.wait_timeout` | `duration` | 已支持 |

<a id="fields-limits"></a>

#### `limits`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `limits` | `object` | 已支持 |
| `limits.max_body_bytes` | `integer` | 已支持 |
| `limits.max_conns` | `integer` | 已支持 |
| `limits.max_header_bytes` | `integer` | 已支持 |
| `limits.max_pending_bytes` | `bytes\|percent` | 已支持 |
| `limits.max_write_queue_bytes` | `integer` | 已支持 |
| `limits.write_stall_timeout` | `duration` | 已支持 |

<a id="fields-listeners"></a>

#### `listeners`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `listeners` | `array<object>` | 已支持 |
| `listeners[].access` | `object` | 已支持 |
| `listeners[].access.allow` | `array<string>` | 已支持 |
| `listeners[].access.allow_files` | `array<string>` | 已支持 |
| `listeners[].access.deny` | `array<string>` | 已支持 |
| `listeners[].access.deny_files` | `array<string>` | 已支持 |
| `listeners[].address` | `string, required` | 已支持 |
| `listeners[].mptcp` | `boolean` | 尚不支持（not supported yet） |
| `listeners[].name` | `string, required` | 已支持 |
| `listeners[].protocols` | `array<enum(http/1.1\|h2c\|h2\|h3\|tcp\|udp\|socks)>` | 已支持 |
| `listeners[].proxy_protocol` | `object` | 已支持 |
| `listeners[].proxy_protocol.trusted` | `array<string>, required` | 已支持 |
| `listeners[].socks` | `object` | 已支持 |
| `listeners[].socks.allow` | `array<string>` | 已支持 |
| `listeners[].socks.clients` | `array<string>, required` | 已支持 |
| `listeners[].socks.connect_timeout` | `duration` | 已支持 |
| `listeners[].socks.deny` | `array<string>` | 已支持 |
| `listeners[].socks.idle_timeout` | `duration` | 已支持 |
| `listeners[].socks.ports` | `array<integer>` | 已支持 |
| `listeners[].tls` | `object` | 已支持 |
| `listeners[].tls.acme` | `array<string>` | 已支持 |
| `listeners[].tls.certificates` | `array<object>` | 已支持 |
| `listeners[].tls.certificates[].cert_file` | `string, required` | 已支持 |
| `listeners[].tls.certificates[].key_file` | `string, required` | 已支持 |
| `listeners[].tls.client_auth` | `object` | 已支持 |
| `listeners[].tls.client_auth.ca_files` | `array<string>, required` | 已支持 |
| `listeners[].tls.client_auth.mode` | `enum(require\|verify_if_given)` | 已支持 |
| `listeners[].tls.kernel_tls_offload` | `boolean` | 尚不支持（not supported yet） |
| `listeners[].tls.max_version` | `enum(1.2\|1.3)` | 已支持 |
| `listeners[].tls.on_demand` | `boolean` | 已支持 |

<a id="fields-llm"></a>

#### `llm`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `llm` | `object` | 已支持 |
| `llm.hub` | `object` | 已支持 |
| `llm.hub.access` | `object` | 已支持 |
| `llm.hub.access.allow` | `array<string>` | 已支持 |
| `llm.hub.access.allow_files` | `array<string>` | 已支持 |
| `llm.hub.access.deny` | `array<string>` | 已支持 |
| `llm.hub.access.deny_files` | `array<string>` | 已支持 |
| `llm.hub.console` | `object` | 已支持 |
| `llm.hub.console.access` | `object` | 已支持 |
| `llm.hub.console.access.allow` | `array<string>` | 已支持 |
| `llm.hub.console.access.allow_files` | `array<string>` | 已支持 |
| `llm.hub.console.access.deny` | `array<string>` | 已支持 |
| `llm.hub.console.access.deny_files` | `array<string>` | 已支持 |
| `llm.hub.console.address` | `string, required` | 已支持 |
| `llm.hub.console.cert_file` | `string` | 已支持 |
| `llm.hub.console.key_file` | `string` | 已支持 |
| `llm.hub.console.max_conns` | `integer` | 已支持 |
| `llm.hub.console.max_sessions` | `integer` | 已支持 |
| `llm.hub.console.mptcp` | `boolean` | 尚不支持（not supported yet） |
| `llm.hub.console.session_lifetime` | `duration` | 已支持 |
| `llm.hub.console.session_timeout` | `duration` | 已支持 |
| `llm.hub.console.users` | `array<object>, required` | 已支持 |
| `llm.hub.console.users[].password_hash` | `string, required` | 已支持 |
| `llm.hub.console.users[].username` | `string, required` | 已支持 |
| `llm.hub.id` | `string` | 已支持 |
| `llm.hub.limits` | `object` | 已支持 |
| `llm.hub.limits.max_conns` | `integer` | 已支持 |
| `llm.hub.limits.max_inflight` | `integer` | 已支持 |
| `llm.hub.limits.ping_interval` | `duration` | 已支持 |
| `llm.hub.limits.retry_interval` | `duration` | 已支持 |
| `llm.hub.limits.slow_timeout` | `duration` | 已支持 |
| `llm.hub.limits.stream_idle` | `duration` | 已支持 |
| `llm.hub.listen` | `object` | 已支持 |
| `llm.hub.listen.address` | `string, required` | 已支持 |
| `llm.hub.listen.cert` | `string` | 已支持 |
| `llm.hub.listen.cert_file` | `string` | 已支持 |
| `llm.hub.listen.hub_password` | `string` | 已支持 |
| `llm.hub.listen.key` | `string` | 已支持 |
| `llm.hub.listen.key_file` | `string` | 已支持 |
| `llm.hub.listen.node_password` | `string` | 已支持 |
| `llm.hub.listen.password` | `string` | 已支持 |
| `llm.hub.listen.proxy_password` | `string` | 已支持 |
| `llm.hub.listen.self_signed` | `boolean` | 已支持 |
| `llm.hub.max_nodes` | `integer` | 已支持 |
| `llm.hub.name` | `string` | 已支持 |
| `llm.hub.peers` | `array<object>` | 已支持 |
| `llm.hub.peers[].address` | `string, required` | 已支持 |
| `llm.hub.peers[].ca_file` | `string` | 已支持 |
| `llm.hub.peers[].password` | `string` | 已支持 |
| `llm.hub.peers[].pin` | `string` | 已支持 |
| `llm.hub.peers[].role` | `enum(hub\|node\|proxy), required` | 已支持 |
| `llm.hub.peers[].server_name` | `string` | 已支持 |
| `llm.hub.peers[].token` | `string` | 已支持 |
| `llm.hub.tokens` | `array<object>` | 已支持 |
| `llm.hub.tokens[].created_at` | `integer` | 已支持 |
| `llm.hub.tokens[].expires_at` | `integer` | 已支持 |
| `llm.hub.tokens[].hash` | `string, required` | 已支持 |
| `llm.hub.tokens[].id` | `string, required` | 已支持 |
| `llm.hub.tokens[].members` | `array<string>` | 已支持 |
| `llm.hub.tokens[].name` | `string, required` | 已支持 |
| `llm.hub.tokens[].note` | `string` | 已支持 |
| `llm.node` | `object` | 已支持 |
| `llm.node.access` | `object` | 已支持 |
| `llm.node.access.allow` | `array<string>` | 已支持 |
| `llm.node.access.allow_files` | `array<string>` | 已支持 |
| `llm.node.access.deny` | `array<string>` | 已支持 |
| `llm.node.access.deny_files` | `array<string>` | 已支持 |
| `llm.node.console` | `object` | 已支持 |
| `llm.node.console.access` | `object` | 已支持 |
| `llm.node.console.access.allow` | `array<string>` | 已支持 |
| `llm.node.console.access.allow_files` | `array<string>` | 已支持 |
| `llm.node.console.access.deny` | `array<string>` | 已支持 |
| `llm.node.console.access.deny_files` | `array<string>` | 已支持 |
| `llm.node.console.address` | `string, required` | 已支持 |
| `llm.node.console.cert_file` | `string` | 已支持 |
| `llm.node.console.key_file` | `string` | 已支持 |
| `llm.node.console.max_conns` | `integer` | 已支持 |
| `llm.node.console.max_sessions` | `integer` | 已支持 |
| `llm.node.console.mptcp` | `boolean` | 尚不支持（not supported yet） |
| `llm.node.console.session_lifetime` | `duration` | 已支持 |
| `llm.node.console.session_timeout` | `duration` | 已支持 |
| `llm.node.console.users` | `array<object>, required` | 已支持 |
| `llm.node.console.users[].password_hash` | `string, required` | 已支持 |
| `llm.node.console.users[].username` | `string, required` | 已支持 |
| `llm.node.description` | `string` | 已支持 |
| `llm.node.id` | `string` | 已支持 |
| `llm.node.limits` | `object` | 已支持 |
| `llm.node.limits.max_conns` | `integer` | 已支持 |
| `llm.node.limits.max_inflight` | `integer` | 已支持 |
| `llm.node.limits.ping_interval` | `duration` | 已支持 |
| `llm.node.limits.retry_interval` | `duration` | 已支持 |
| `llm.node.limits.slow_timeout` | `duration` | 已支持 |
| `llm.node.limits.stream_idle` | `duration` | 已支持 |
| `llm.node.listen` | `object` | 已支持 |
| `llm.node.listen.address` | `string, required` | 已支持 |
| `llm.node.listen.cert` | `string` | 已支持 |
| `llm.node.listen.cert_file` | `string` | 已支持 |
| `llm.node.listen.hub_password` | `string` | 已支持 |
| `llm.node.listen.key` | `string` | 已支持 |
| `llm.node.listen.key_file` | `string` | 已支持 |
| `llm.node.listen.node_password` | `string` | 已支持 |
| `llm.node.listen.password` | `string` | 已支持 |
| `llm.node.listen.proxy_password` | `string` | 已支持 |
| `llm.node.listen.self_signed` | `boolean` | 已支持 |
| `llm.node.models` | `array<object>` | 已支持 |
| `llm.node.models[].api` | `enum(openai\|anthropic\|gemini\|azure_openai\|ollama\|cohere), required` | 已支持 |
| `llm.node.models[].api_key` | `string` | 已支持 |
| `llm.node.models[].api_version` | `string` | 已支持 |
| `llm.node.models[].base_url` | `string, required` | 已支持 |
| `llm.node.models[].description` | `string` | 已支持 |
| `llm.node.models[].disabled` | `boolean` | 已支持 |
| `llm.node.models[].max_inflight` | `integer` | 已支持 |
| `llm.node.models[].name` | `string, required` | 已支持 |
| `llm.node.models[].upstream` | `string` | 已支持 |
| `llm.node.name` | `string` | 已支持 |
| `llm.node.peers` | `array<object>` | 已支持 |
| `llm.node.peers[].address` | `string, required` | 已支持 |
| `llm.node.peers[].ca_file` | `string` | 已支持 |
| `llm.node.peers[].password` | `string` | 已支持 |
| `llm.node.peers[].pin` | `string` | 已支持 |
| `llm.node.peers[].role` | `enum(hub\|node\|proxy), required` | 已支持 |
| `llm.node.peers[].server_name` | `string` | 已支持 |
| `llm.node.peers[].token` | `string` | 已支持 |
| `llm.node.proxy` | `object` | 已支持 |
| `llm.node.proxy.connect_timeout` | `duration` | 已支持 |
| `llm.node.proxy.first_byte_timeout` | `duration` | 已支持 |
| `llm.node.proxy.idle_timeout` | `duration` | 已支持 |
| `llm.node.proxy.max_body_bytes` | `integer` | 已支持 |
| `llm.node.proxy.max_inflight` | `integer` | 已支持 |
| `llm.node.proxy.timeout` | `duration` | 已支持 |
| `llm.node.settings` | `object` | 已支持 |
| `llm.node.settings.allow_hub_manage` | `boolean` | 已支持 |
| `llm.node.settings.gpu_data_dir` | `string` | 已支持 |
| `llm.node.settings.gpu_sudo` | `boolean` | 已支持 |
| `llm.node.settings.job_timeout` | `duration` | 已支持 |
| `llm.node.settings.refresh_interval` | `duration` | 已支持 |
| `llm.node.tokens` | `array<object>` | 已支持 |
| `llm.node.tokens[].created_at` | `integer` | 已支持 |
| `llm.node.tokens[].expires_at` | `integer` | 已支持 |
| `llm.node.tokens[].hash` | `string, required` | 已支持 |
| `llm.node.tokens[].id` | `string, required` | 已支持 |
| `llm.node.tokens[].members` | `array<string>` | 已支持 |
| `llm.node.tokens[].name` | `string, required` | 已支持 |
| `llm.node.tokens[].note` | `string` | 已支持 |
| `llm.proxy` | `object` | 已支持 |
| `llm.proxy.access` | `object` | 已支持 |
| `llm.proxy.access.allow` | `array<string>` | 已支持 |
| `llm.proxy.access.allow_files` | `array<string>` | 已支持 |
| `llm.proxy.access.deny` | `array<string>` | 已支持 |
| `llm.proxy.access.deny_files` | `array<string>` | 已支持 |
| `llm.proxy.api` | `object` | 已支持 |
| `llm.proxy.api.address` | `string, required` | 已支持 |
| `llm.proxy.api.cert_file` | `string` | 已支持 |
| `llm.proxy.api.first_byte_timeout` | `duration` | 已支持 |
| `llm.proxy.api.key_file` | `string` | 已支持 |
| `llm.proxy.api.max_body_bytes` | `integer` | 已支持 |
| `llm.proxy.api.max_conns` | `integer` | 已支持 |
| `llm.proxy.api.max_inflight` | `integer` | 已支持 |
| `llm.proxy.api.timeout` | `duration` | 已支持 |
| `llm.proxy.api_keys` | `array<object>` | 已支持 |
| `llm.proxy.api_keys[].created_at` | `integer` | 已支持 |
| `llm.proxy.api_keys[].hash` | `string, required` | 已支持 |
| `llm.proxy.api_keys[].id` | `string, required` | 已支持 |
| `llm.proxy.api_keys[].name` | `string, required` | 已支持 |
| `llm.proxy.api_keys[].prefix` | `string` | 已支持 |
| `llm.proxy.console` | `object` | 已支持 |
| `llm.proxy.console.access` | `object` | 已支持 |
| `llm.proxy.console.access.allow` | `array<string>` | 已支持 |
| `llm.proxy.console.access.allow_files` | `array<string>` | 已支持 |
| `llm.proxy.console.access.deny` | `array<string>` | 已支持 |
| `llm.proxy.console.access.deny_files` | `array<string>` | 已支持 |
| `llm.proxy.console.address` | `string, required` | 已支持 |
| `llm.proxy.console.cert_file` | `string` | 已支持 |
| `llm.proxy.console.key_file` | `string` | 已支持 |
| `llm.proxy.console.max_conns` | `integer` | 已支持 |
| `llm.proxy.console.max_sessions` | `integer` | 已支持 |
| `llm.proxy.console.mptcp` | `boolean` | 尚不支持（not supported yet） |
| `llm.proxy.console.session_lifetime` | `duration` | 已支持 |
| `llm.proxy.console.session_timeout` | `duration` | 已支持 |
| `llm.proxy.console.users` | `array<object>, required` | 已支持 |
| `llm.proxy.console.users[].password_hash` | `string, required` | 已支持 |
| `llm.proxy.console.users[].username` | `string, required` | 已支持 |
| `llm.proxy.id` | `string` | 已支持 |
| `llm.proxy.limits` | `object` | 已支持 |
| `llm.proxy.limits.max_conns` | `integer` | 已支持 |
| `llm.proxy.limits.max_inflight` | `integer` | 已支持 |
| `llm.proxy.limits.ping_interval` | `duration` | 已支持 |
| `llm.proxy.limits.retry_interval` | `duration` | 已支持 |
| `llm.proxy.limits.slow_timeout` | `duration` | 已支持 |
| `llm.proxy.limits.stream_idle` | `duration` | 已支持 |
| `llm.proxy.listen` | `object` | 已支持 |
| `llm.proxy.listen.address` | `string, required` | 已支持 |
| `llm.proxy.listen.cert` | `string` | 已支持 |
| `llm.proxy.listen.cert_file` | `string` | 已支持 |
| `llm.proxy.listen.hub_password` | `string` | 已支持 |
| `llm.proxy.listen.key` | `string` | 已支持 |
| `llm.proxy.listen.key_file` | `string` | 已支持 |
| `llm.proxy.listen.node_password` | `string` | 已支持 |
| `llm.proxy.listen.password` | `string` | 已支持 |
| `llm.proxy.listen.proxy_password` | `string` | 已支持 |
| `llm.proxy.listen.self_signed` | `boolean` | 已支持 |
| `llm.proxy.name` | `string` | 已支持 |
| `llm.proxy.peers` | `array<object>` | 已支持 |
| `llm.proxy.peers[].address` | `string, required` | 已支持 |
| `llm.proxy.peers[].ca_file` | `string` | 已支持 |
| `llm.proxy.peers[].password` | `string` | 已支持 |
| `llm.proxy.peers[].pin` | `string` | 已支持 |
| `llm.proxy.peers[].role` | `enum(hub\|node\|proxy), required` | 已支持 |
| `llm.proxy.peers[].server_name` | `string` | 已支持 |
| `llm.proxy.peers[].token` | `string` | 已支持 |
| `llm.proxy.tokens` | `array<object>` | 已支持 |
| `llm.proxy.tokens[].created_at` | `integer` | 已支持 |
| `llm.proxy.tokens[].expires_at` | `integer` | 已支持 |
| `llm.proxy.tokens[].hash` | `string, required` | 已支持 |
| `llm.proxy.tokens[].id` | `string, required` | 已支持 |
| `llm.proxy.tokens[].members` | `array<string>` | 已支持 |
| `llm.proxy.tokens[].name` | `string, required` | 已支持 |
| `llm.proxy.tokens[].note` | `string` | 已支持 |

<a id="fields-log"></a>

#### `log`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `log` | `object` | 已支持 |
| `log.format` | `enum(line\|text\|json)` | 已支持 |
| `log.level` | `enum(debug\|info\|warn\|error)` | 已支持 |
| `log.levels` | `map<string>` | 已支持 |
| `log.loggers` | `map<object>` | 已支持 |
| `log.loggers.*.format` | `enum(line\|text\|json)` | 已支持 |
| `log.loggers.*.level` | `enum(debug\|info\|warn\|error)` | 已支持 |
| `log.loggers.*.output` | `string` | 已支持 |
| `log.loggers.*.rotate` | `object` | 已支持 |
| `log.loggers.*.rotate.compress` | `boolean` | 已支持 |
| `log.loggers.*.rotate.interval` | `duration` | 已支持 |
| `log.loggers.*.rotate.max_age` | `duration` | 已支持 |
| `log.loggers.*.rotate.max_bytes` | `integer` | 已支持 |
| `log.loggers.*.rotate.max_files` | `integer` | 已支持 |
| `log.loggers.*.rotate.max_total_bytes` | `integer` | 已支持 |
| `log.output` | `string` | 已支持 |
| `log.rotate` | `object` | 已支持 |
| `log.rotate.compress` | `boolean` | 已支持 |
| `log.rotate.interval` | `duration` | 已支持 |
| `log.rotate.max_age` | `duration` | 已支持 |
| `log.rotate.max_bytes` | `integer` | 已支持 |
| `log.rotate.max_files` | `integer` | 已支持 |
| `log.rotate.max_total_bytes` | `integer` | 已支持 |
| `log.time_zone` | `string` | 已支持 |

<a id="fields-media-auth"></a>

#### `media_auth`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `media_auth` | `array<object>` | 已支持 |
| `media_auth[].keys` | `array<object>` | 尚不支持（not supported yet） |
| `media_auth[].keys[].key` | `string, required` | 尚不支持（not supported yet） |
| `media_auth[].keys[].secret` | `string` | 尚不支持（not supported yet） |
| `media_auth[].keys[].secret_file` | `string` | 尚不支持（not supported yet） |
| `media_auth[].name` | `string, required` | 已支持 |
| `media_auth[].public` | `array<enum(publish\|play)>` | 已支持 |
| `media_auth[].secret` | `string` | 已支持 |
| `media_auth[].secret_file` | `string` | 已支持 |
| `media_auth[].token_ttl` | `duration` | 已支持 |
| `media_auth[].tokens` | `array<object>` | 已支持 |
| `media_auth[].tokens[].name` | `string` | 已支持 |
| `media_auth[].tokens[].play` | `array<string>` | 已支持 |
| `media_auth[].tokens[].publish` | `array<string>` | 已支持 |
| `media_auth[].tokens[].token` | `string` | 已支持 |
| `media_auth[].tokens[].token_sha256` | `string` | 已支持 |
| `media_auth[].url_signing` | `object` | 尚不支持（not supported yet） |
| `media_auth[].url_signing.bind_address` | `enum(ip\|subnet)` | 尚不支持（not supported yet） |
| `media_auth[].url_signing.cookie` | `boolean` | 尚不支持（not supported yet） |
| `media_auth[].url_signing.max_sessions` | `integer` | 尚不支持（not supported yet） |
| `media_auth[].url_signing.max_tracked_sessions` | `integer` | 尚不支持（not supported yet） |
| `media_auth[].url_signing.on_limit` | `enum(refuse\|end_oldest)` | 尚不支持（not supported yet） |
| `media_auth[].url_signing.origins` | `array<string>` | 尚不支持（not supported yet） |
| `media_auth[].url_signing.play_grace` | `duration` | 尚不支持（not supported yet） |
| `media_auth[].url_signing.referers` | `array<string>` | 尚不支持（not supported yet） |
| `media_auth[].url_signing.session_idle` | `duration` | 尚不支持（not supported yet） |
| `media_auth[].url_signing.trusted_proxies` | `array<string>` | 尚不支持（not supported yet） |
| `media_auth[].url_signing.ttl` | `duration` | 尚不支持（not supported yet） |
| `media_auth[].users` | `array<object>` | 已支持 |
| `media_auth[].users[].name` | `string, required` | 已支持 |
| `media_auth[].users[].password` | `string` | 已支持 |
| `media_auth[].users[].password_hash` | `string` | 已支持 |
| `media_auth[].users[].play` | `array<string>` | 已支持 |
| `media_auth[].users[].publish` | `array<string>` | 已支持 |

<a id="fields-media-library"></a>

#### `media_library`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `media_library` | `array<object>` | 尚不支持（not supported yet） |
| `media_library[].exclude` | `array<string>` | 尚不支持（not supported yet） |
| `media_library[].file_queue` | `integer` | 尚不支持（not supported yet） |
| `media_library[].file_timeout` | `duration` | 尚不支持（not supported yet） |
| `media_library[].file_workers` | `integer` | 尚不支持（not supported yet） |
| `media_library[].files` | `array<string>` | 尚不支持（not supported yet） |
| `media_library[].follow_symlinks` | `boolean` | 尚不支持（not supported yet） |
| `media_library[].max_catalog_bytes` | `integer` | 尚不支持（not supported yet） |
| `media_library[].max_files` | `integer` | 尚不支持（not supported yet） |
| `media_library[].max_items` | `integer` | 尚不支持（not supported yet） |
| `media_library[].max_probes` | `integer` | 尚不支持（not supported yet） |
| `media_library[].max_sidecar_bytes` | `integer` | 尚不支持（not supported yet） |
| `media_library[].max_subtitle_bytes` | `integer` | 尚不支持（not supported yet） |
| `media_library[].max_table_bytes` | `integer` | 尚不支持（not supported yet） |
| `media_library[].max_upload_bytes` | `integer` | 尚不支持（not supported yet） |
| `media_library[].max_uploads` | `integer` | 尚不支持（not supported yet） |
| `media_library[].min_free_bytes` | `integer` | 尚不支持（not supported yet） |
| `media_library[].name` | `string, required` | 尚不支持（not supported yet） |
| `media_library[].roots` | `array<string>, required` | 尚不支持（not supported yet） |
| `media_library[].scan_interval` | `duration` | 尚不支持（not supported yet） |
| `media_library[].secret` | `string` | 尚不支持（not supported yet） |
| `media_library[].secret_file` | `string` | 尚不支持（not supported yet） |
| `media_library[].upload_dir` | `string` | 尚不支持（not supported yet） |
| `media_library[].upload_expiry` | `duration` | 尚不支持（not supported yet） |

<a id="fields-metrics"></a>

#### `metrics`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `metrics` | `object` | 已支持 |

<a id="fields-pprof"></a>

#### `pprof`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `pprof` | `object` | 尚不支持（not supported yet） |
| `pprof.block_profile_rate` | `integer` | 尚不支持（not supported yet） |
| `pprof.max_duration` | `duration` | 尚不支持（not supported yet） |
| `pprof.mutex_profile_fraction` | `integer` | 尚不支持（not supported yet） |

<a id="fields-providers"></a>

#### `providers`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `providers` | `array<object>` | 已支持 |
| `providers[].consul` | `object` | 已支持 |
| `providers[].consul.ca_file` | `string` | 已支持 |
| `providers[].consul.cert_file` | `string` | 已支持 |
| `providers[].consul.datacenter` | `string` | 已支持 |
| `providers[].consul.endpoint` | `string` | 已支持 |
| `providers[].consul.expose_all` | `boolean` | 已支持 |
| `providers[].consul.key_file` | `string` | 已支持 |
| `providers[].consul.max_services` | `integer` | 已支持 |
| `providers[].consul.refresh_interval` | `duration` | 已支持 |
| `providers[].consul.request_timeout` | `duration` | 已支持 |
| `providers[].consul.tag_prefix` | `string` | 已支持 |
| `providers[].consul.token_file` | `string` | 已支持 |
| `providers[].consul_kv` | `object` | 已支持 |
| `providers[].consul_kv.ca_file` | `string` | 已支持 |
| `providers[].consul_kv.cert_file` | `string` | 已支持 |
| `providers[].consul_kv.datacenter` | `string` | 已支持 |
| `providers[].consul_kv.endpoint` | `string` | 已支持 |
| `providers[].consul_kv.key_file` | `string` | 已支持 |
| `providers[].consul_kv.max_keys` | `integer` | 已支持 |
| `providers[].consul_kv.prefix` | `string` | 已支持 |
| `providers[].consul_kv.refresh_interval` | `duration` | 已支持 |
| `providers[].consul_kv.request_timeout` | `duration` | 已支持 |
| `providers[].consul_kv.token_file` | `string` | 已支持 |
| `providers[].docker` | `object` | 已支持 |
| `providers[].docker.endpoint` | `string` | 已支持 |
| `providers[].docker.expose_all` | `boolean` | 已支持 |
| `providers[].docker.label_prefix` | `string` | 已支持 |
| `providers[].docker.max_containers` | `integer` | 已支持 |
| `providers[].docker.network` | `string` | 已支持 |
| `providers[].docker.refresh_interval` | `duration` | 已支持 |
| `providers[].docker.request_timeout` | `duration` | 已支持 |
| `providers[].docker.use_published_ports` | `boolean` | 已支持 |
| `providers[].etcd` | `object` | 已支持 |
| `providers[].etcd.ca_file` | `string` | 已支持 |
| `providers[].etcd.cert_file` | `string` | 已支持 |
| `providers[].etcd.endpoints` | `array<string>` | 已支持 |
| `providers[].etcd.key_file` | `string` | 已支持 |
| `providers[].etcd.max_keys` | `integer` | 已支持 |
| `providers[].etcd.password_file` | `string` | 已支持 |
| `providers[].etcd.prefix` | `string` | 已支持 |
| `providers[].etcd.refresh_interval` | `duration` | 已支持 |
| `providers[].etcd.request_timeout` | `duration` | 已支持 |
| `providers[].etcd.username` | `string` | 已支持 |
| `providers[].kubernetes` | `object` | 已支持 |
| `providers[].kubernetes.addresses` | `array<string>` | 已支持 |
| `providers[].kubernetes.allow_external_name_services` | `boolean` | 尚不支持（not supported yet） |
| `providers[].kubernetes.backends` | `enum(endpoints\|cluster_ip)` | 已支持 |
| `providers[].kubernetes.certificates_dir` | `string` | 已支持 |
| `providers[].kubernetes.context` | `string` | 已支持 |
| `providers[].kubernetes.controller_name` | `string` | 已支持 |
| `providers[].kubernetes.ingress` | `object` | 已支持 |
| `providers[].kubernetes.ingress.http_port` | `integer` | 已支持 |
| `providers[].kubernetes.ingress.https_port` | `integer` | 已支持 |
| `providers[].kubernetes.kubeconfig` | `string` | 已支持 |
| `providers[].kubernetes.listen_host` | `string` | 已支持 |
| `providers[].kubernetes.max_objects` | `integer` | 已支持 |
| `providers[].kubernetes.namespaces` | `array<string>` | 已支持 |
| `providers[].kubernetes.request_timeout` | `duration` | 已支持 |
| `providers[].name` | `string` | 已支持 |

<a id="fields-routes"></a>

#### `routes`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `routes` | `array<object>` | 已支持 |
| `routes[].access` | `object` | 已支持 |
| `routes[].access.allow` | `array<string>` | 已支持 |
| `routes[].access.allow_files` | `array<string>` | 已支持 |
| `routes[].access.deny` | `array<string>` | 已支持 |
| `routes[].access.deny_files` | `array<string>` | 已支持 |
| `routes[].handler` | `object, required` | 已支持 |
| `routes[].handler.module` | `enum(bt_publish\|bt_stream\|bt_tracker\|fastcgi\|forward_proxy\|hls\|httpflv\|media_pages\|media_tokens\|proxy\|respond\|static\|stream_whep\|stream_whip\|sync_ingest\|vod\|webrtc_proxy\|websockify\|whep\|whip), required` | 已支持 |
| `routes[].handler.params` | `object` | 已支持 |
| `routes[].listeners` | `array<string>` | 已支持 |
| `routes[].match` | `object` | 已支持 |
| `routes[].match.expr` | `string` | 已支持 |
| `routes[].match.header` | `map<array<string>>` | 已支持 |
| `routes[].match.host` | `array<string>` | 已支持 |
| `routes[].match.method` | `array<string>` | 已支持 |
| `routes[].match.path` | `array<string>` | 已支持 |
| `routes[].match.path_regexp` | `array<string>` | 已支持 |
| `routes[].match.query` | `map<array<string>>` | 已支持 |
| `routes[].match.remote_ip` | `array<string>` | 已支持 |
| `routes[].middleware` | `array<object>` | 已支持 |
| `routes[].middleware[].module` | `enum(basic_auth\|body_limit\|cache\|circuit_breaker\|client_cert\|compress\|concurrency_limit\|forward_auth\|headers\|jwt\|rate_limit\|redirect\|rewrite\|wasm), required` | 已支持 |
| `routes[].middleware[].params` | `object` | 已支持 |
| `routes[].name` | `string` | 已支持 |
| `routes[].priority` | `integer` | 已支持 |

<a id="fields-schema-version"></a>

#### `schema_version`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `schema_version` | `integer` | 已支持 |

<a id="fields-services"></a>

#### `services`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `services` | `array<object>` | 已支持 |
| `services[].access` | `object` | 已支持 |
| `services[].access.allow` | `array<string>` | 已支持 |
| `services[].access.allow_files` | `array<string>` | 已支持 |
| `services[].access.deny` | `array<string>` | 已支持 |
| `services[].access.deny_files` | `array<string>` | 已支持 |
| `services[].idle_timeout` | `duration` | 已支持 |
| `services[].listeners` | `array<string>, required` | 已支持 |
| `services[].max_conns` | `integer` | 已支持 |
| `services[].name` | `string, required` | 已支持 |
| `services[].proxy_protocol` | `enum(v1\|v2)` | 已支持 |
| `services[].server_names` | `array<string>` | 已支持 |
| `services[].upstream` | `string, required` | 已支持 |

<a id="fields-streaming"></a>

#### `streaming`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `streaming` | `object` | 已支持 |
| `streaming.access` | `object` | 已支持 |
| `streaming.access.allow` | `array<string>` | 已支持 |
| `streaming.access.allow_files` | `array<string>` | 已支持 |
| `streaming.access.deny` | `array<string>` | 已支持 |
| `streaming.access.deny_files` | `array<string>` | 已支持 |
| `streaming.channels` | `array<object>` | 尚不支持（not supported yet） |
| `streaming.channels[].encrypt` | `boolean` | 尚不支持（not supported yet） |
| `streaming.channels[].items` | `array<string>` | 尚不支持（not supported yet） |
| `streaming.channels[].key_rotation` | `integer` | 尚不支持（not supported yet） |
| `streaming.channels[].library` | `string, required` | 尚不支持（not supported yet） |
| `streaming.channels[].loop` | `boolean` | 尚不支持（not supported yet） |
| `streaming.channels[].max_buffer_bytes` | `integer` | 尚不支持（not supported yet） |
| `streaming.channels[].name` | `string, required` | 尚不支持（not supported yet） |
| `streaming.channels[].order` | `enum(sequence\|shuffle)` | 尚不支持（not supported yet） |
| `streaming.channels[].push` | `array<object>` | 尚不支持（not supported yet） |
| `streaming.channels[].push[].connect_timeout` | `duration` | 尚不支持（not supported yet） |
| `streaming.channels[].push[].name` | `string` | 尚不支持（not supported yet） |
| `streaming.channels[].push[].retry_max` | `duration` | 尚不支持（not supported yet） |
| `streaming.channels[].push[].retry_min` | `duration` | 尚不支持（not supported yet） |
| `streaming.channels[].push[].stream` | `string` | 尚不支持（not supported yet） |
| `streaming.channels[].push[].url` | `string, required` | 尚不支持（not supported yet） |
| `streaming.channels[].read_ahead` | `duration` | 尚不支持（not supported yet） |
| `streaming.channels[].rendition` | `string` | 尚不支持（not supported yet） |
| `streaming.channels[].schedule` | `array<object>` | 尚不支持（not supported yet） |
| `streaming.channels[].schedule[].at` | `string, required` | 尚不支持（not supported yet） |
| `streaming.channels[].schedule[].item` | `string, required` | 尚不支持（not supported yet） |
| `streaming.channels[].time_zone` | `string` | 尚不支持（not supported yet） |
| `streaming.gop_cache_bytes` | `integer` | 已支持 |
| `streaming.max_players` | `integer` | 已支持 |
| `streaming.max_streams` | `integer` | 已支持 |
| `streaming.push` | `array<object>` | 尚不支持（not supported yet） |
| `streaming.push[].connect_timeout` | `duration` | 尚不支持（not supported yet） |
| `streaming.push[].name` | `string` | 尚不支持（not supported yet） |
| `streaming.push[].retry_max` | `duration` | 尚不支持（not supported yet） |
| `streaming.push[].retry_min` | `duration` | 尚不支持（not supported yet） |
| `streaming.push[].stream` | `string` | 尚不支持（not supported yet） |
| `streaming.push[].url` | `string, required` | 尚不支持（not supported yet） |
| `streaming.rtmp` | `object` | 已支持 |
| `streaming.rtmp.auth` | `string` | 已支持 |
| `streaming.rtmp.handshake_timeout` | `duration` | 已支持 |
| `streaming.rtmp.idle_timeout` | `duration` | 已支持 |
| `streaming.rtmp.listen` | `string, required` | 已支持 |
| `streaming.rtmp.max_conns` | `integer` | 已支持 |
| `streaming.rtmp.max_message_bytes` | `integer` | 尚不支持（not supported yet） |
| `streaming.rtmp.mptcp` | `boolean` | 尚不支持（not supported yet） |
| `streaming.rtmp.play_wait` | `duration` | 已支持 |
| `streaming.rtsp` | `object` | 已支持 |
| `streaming.rtsp.auth` | `string` | 已支持 |
| `streaming.rtsp.auth_methods` | `array<enum(digest\|basic)>` | 尚不支持（not supported yet） |
| `streaming.rtsp.idle_timeout` | `duration` | 尚不支持（not supported yet） |
| `streaming.rtsp.listen` | `string, required` | 已支持 |
| `streaming.rtsp.max_conns` | `integer` | 已支持 |
| `streaming.rtsp.mptcp` | `boolean` | 尚不支持（not supported yet） |
| `streaming.rtsp.play_wait` | `duration` | 已支持 |
| `streaming.rtsp.realm` | `string` | 尚不支持（not supported yet） |
| `streaming.rtsp.session_timeout` | `duration` | 已支持 |
| `streaming.srt` | `object` | 已支持 |
| `streaming.srt.auth` | `string` | 已支持 |
| `streaming.srt.latency` | `duration` | 已支持 |
| `streaming.srt.listen` | `string, required` | 已支持 |
| `streaming.srt.max_conns` | `integer` | 已支持 |
| `streaming.srt.passphrase` | `string` | 已支持 |
| `streaming.srt.peer_idle_timeout` | `duration` | 已支持 |
| `streaming.srt.play_wait` | `duration` | 已支持 |
| `streaming.webrtc` | `object` | 已支持 |
| `streaming.webrtc.connect_timeout` | `duration` | 已支持 |
| `streaming.webrtc.ice_servers` | `array<object>` | 已支持 |
| `streaming.webrtc.ice_servers[].credential` | `string` | 已支持 |
| `streaming.webrtc.ice_servers[].urls` | `array<string>, required` | 已支持 |
| `streaming.webrtc.ice_servers[].username` | `string` | 已支持 |
| `streaming.webrtc.listen` | `string, required` | 已支持 |
| `streaming.webrtc.max_offer_bytes` | `integer` | 已支持 |
| `streaming.webrtc.max_sessions` | `integer` | 已支持 |
| `streaming.webrtc.play_wait` | `duration` | 已支持 |
| `streaming.webrtc.public_ips` | `array<string>` | 已支持 |

<a id="fields-sync"></a>

#### `sync`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines` | `array<object>` | 尚不支持（not supported yet） |
| `sync.pipelines[].checkpoint_interval` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.batch` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.batch.max_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.batch.max_records` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.batch.max_wait` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.name` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink` | `object, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink.module` | `enum(clickhouse\|dead_letters\|doris\|elasticsearch\|file\|http\|influxdb\|kafka\|loki\|memory\|milvus\|mongo\|mqtt\|mssql\|mysql\|nats\|otlp_logs\|postgres\|pulsar\|qdrant\|rabbitmq\|redis\|s3\|stdout\|tdengine\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].job` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].job.max_parallel` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].job.schedule` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].job.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].mode` | `enum(stream\|once)` | 尚不支持（not supported yet） |
| `sync.pipelines[].name` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].on_error` | `enum(stop\|skip\|dead_letter)` | 尚不支持（not supported yet） |
| `sync.pipelines[].queue` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].queue.max_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].queue.max_records` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].retry` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].retry.backoff` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].retry.max_attempts` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].retry.max_backoff` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks` | `array<object>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].batch` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].batch.max_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].batch.max_records` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].batch.max_wait` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].name` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink` | `object, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink.module` | `enum(clickhouse\|dead_letters\|doris\|elasticsearch\|file\|http\|influxdb\|kafka\|loki\|memory\|milvus\|mongo\|mqtt\|mssql\|mysql\|nats\|otlp_logs\|postgres\|pulsar\|qdrant\|rabbitmq\|redis\|s3\|stdout\|tdengine\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source` | `object, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source.module` | `enum(clickhouse\|doris\|elasticsearch\|file\|generate\|http\|kafka\|mongo\|mqtt\|mssql\|mysql\|nats\|postgres\|pulsar\|rabbitmq\|redis\|s3\|tidb\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms` | `array<object>` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[].module` | `enum(cast\|embed\|envelope\|fields\|filter\|key\|mask\|route\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[].params` | `object` | 尚不支持（not supported yet） |
| `sync.state_dir` | `string, required` | 尚不支持（not supported yet） |

<a id="fields-sync-codecs-canal-json"></a>

#### `sync.codecs/canal_json`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<canal_json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<canal_json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<canal_json>.params.time_zone` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-codecs-csv"></a>

#### `sync.codecs/csv`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<csv>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<csv>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<csv>.params.columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<csv>.params.comma` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-codecs-debezium"></a>

#### `sync.codecs/debezium`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<debezium>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<debezium>.params` | `object` | 尚不支持（not supported yet） |

<a id="fields-sync-codecs-json"></a>

#### `sync.codecs/json`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<json>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<json>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<json>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |

<a id="fields-sync-codecs-line"></a>

#### `sync.codecs/line`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<line>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<line>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<line>.params.field` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-codecs-raw"></a>

#### `sync.codecs/raw`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<raw>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<raw>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<raw>.params.field` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-codecs-ticdc-simple"></a>

#### `sync.codecs/ticdc_simple`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<ticdc_simple>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<ticdc_simple>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<ticdc_simple>.params.time_zone` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-codecs-wasm"></a>

#### `sync.codecs/wasm`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-clickhouse"></a>

#### `sync.sinks/clickhouse`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<clickhouse>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.database` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.deleted_column` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.deletes` | `enum(flag\|delete\|ignore)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.plain` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.settings` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.table` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.urls` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.version_column` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<clickhouse>.params.version_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<clickhouse>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<clickhouse>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.database` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.deleted_column` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.deletes` | `enum(flag\|delete\|ignore)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.plain` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.settings` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.table` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.urls` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.version_column` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<clickhouse>.params.version_field` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-dead-letters"></a>

#### `sync.sinks/dead_letters`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<dead_letters>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<dead_letters>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<dead_letters>.params.max_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<dead_letters>.params.max_records` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<dead_letters>.params.path` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<dead_letters>.params.when_full` | `enum(refuse\|drop_oldest)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<dead_letters>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<dead_letters>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<dead_letters>.params.max_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<dead_letters>.params.max_records` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<dead_letters>.params.path` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<dead_letters>.params.when_full` | `enum(refuse\|drop_oldest)` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-doris"></a>

#### `sync.sinks/doris`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<doris>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<doris>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<doris>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<doris>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<doris>.params.database` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<doris>.params.deletes` | `enum(sign\|ignore)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<doris>.params.flavor` | `enum(doris\|starrocks)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<doris>.params.format` | `enum(json\|csv)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<doris>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<doris>.params.label_prefix` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<doris>.params.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<doris>.params.partial_columns` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<doris>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<doris>.params.properties` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<doris>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<doris>.params.table` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<doris>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<doris>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<doris>.params.urls` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<doris>.params.username` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<doris>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<doris>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<doris>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<doris>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<doris>.params.database` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<doris>.params.deletes` | `enum(sign\|ignore)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<doris>.params.flavor` | `enum(doris\|starrocks)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<doris>.params.format` | `enum(json\|csv)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<doris>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<doris>.params.label_prefix` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<doris>.params.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<doris>.params.partial_columns` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<doris>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<doris>.params.properties` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<doris>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<doris>.params.table` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<doris>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<doris>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<doris>.params.urls` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<doris>.params.username` | `string, required` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-elasticsearch"></a>

#### `sync.sinks/elasticsearch`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<elasticsearch>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.action` | `enum(index\|create)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.api_key_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.index` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.pipeline` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.routing` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.urls` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<elasticsearch>.params.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<elasticsearch>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.action` | `enum(index\|create)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.api_key_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.index` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.pipeline` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.routing` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.urls` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<elasticsearch>.params.username` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-file"></a>

#### `sync.sinks/file`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<file>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.rotate` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.rotate.compress` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.rotate.interval` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.rotate.max_age` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.rotate.max_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.rotate.max_files` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.rotate.max_total_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.template` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<file>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.rotate` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.rotate.compress` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.rotate.interval` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.rotate.max_age` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.rotate.max_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.rotate.max_files` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.rotate.max_total_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.template` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<file>.params.time_zone` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-http"></a>

#### `sync.sinks/http`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<http>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.bearer_token_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.content_type` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.format` | `enum(ndjson\|json)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.headers` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.idempotency_header` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.method` | `enum(POST\|PUT)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.url` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<http>.params.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.bearer_token_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.content_type` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.format` | `enum(ndjson\|json)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.headers` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.idempotency_header` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.method` | `enum(POST\|PUT)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.url` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<http>.params.username` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-influxdb"></a>

#### `sync.sinks/influxdb`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<influxdb>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.api` | `enum(v2\|v3\|v1)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.bucket` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.database` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.deletes` | `enum(skip\|fail)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.fields` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.gzip` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.measurement` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.org` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.precision` | `enum(ns\|us\|ms\|s)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.retention_policy` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.tags` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.time_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.token_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.url` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<influxdb>.params.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params.api` | `enum(v2\|v3\|v1)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params.bucket` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params.database` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params.deletes` | `enum(skip\|fail)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params.fields` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params.gzip` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params.measurement` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params.org` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params.precision` | `enum(ns\|us\|ms\|s)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params.retention_policy` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params.tags` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params.time_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params.token_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params.url` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<influxdb>.params.username` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-kafka"></a>

#### `sync.sinks/kafka`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<kafka>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.acks` | `enum(all\|leader\|none)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.brokers` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.client_id` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.compression` | `enum(none\|gzip\|snappy\|lz4\|zstd)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.compression_level` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.create_topics` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.create_topics.configs` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.create_topics.partitions` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.create_topics.replication_factor` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.deletes` | `enum(tombstone\|encode)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.max_batch_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.sasl` | `enum(PLAIN\|SCRAM-SHA-256\|SCRAM-SHA-512)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.topic` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<kafka>.params.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.acks` | `enum(all\|leader\|none)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.brokers` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.client_id` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.compression` | `enum(none\|gzip\|snappy\|lz4\|zstd)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.compression_level` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.create_topics` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.create_topics.configs` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.create_topics.partitions` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.create_topics.replication_factor` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.deletes` | `enum(tombstone\|encode)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.max_batch_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.sasl` | `enum(PLAIN\|SCRAM-SHA-256\|SCRAM-SHA-512)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.topic` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<kafka>.params.username` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-loki"></a>

#### `sync.sinks/loki`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<loki>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.bearer_token_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.encoding` | `enum(protobuf\|json)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.labels` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.structured_metadata` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.tenant` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.tenant_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.timestamp_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.urls` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<loki>.params.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.bearer_token_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.encoding` | `enum(protobuf\|json)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.labels` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.structured_metadata` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.tenant` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.tenant_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.timestamp_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.urls` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<loki>.params.username` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-memory"></a>

#### `sync.sinks/memory`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<memory>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<memory>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<memory>.params.max_records` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<memory>.params.store` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<memory>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<memory>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<memory>.params.max_records` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<memory>.params.store` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-milvus"></a>

#### `sync.sinks/milvus`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<milvus>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<milvus>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<milvus>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<milvus>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<milvus>.params.collection` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<milvus>.params.database` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<milvus>.params.fields` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<milvus>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<milvus>.params.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<milvus>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<milvus>.params.primary_key` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<milvus>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<milvus>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<milvus>.params.token_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<milvus>.params.url` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<milvus>.params.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<milvus>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<milvus>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<milvus>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<milvus>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<milvus>.params.collection` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<milvus>.params.database` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<milvus>.params.fields` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<milvus>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<milvus>.params.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<milvus>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<milvus>.params.primary_key` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<milvus>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<milvus>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<milvus>.params.token_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<milvus>.params.url` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<milvus>.params.username` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-mongo"></a>

#### `sync.sinks/mongo`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<mongo>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mongo>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mongo>.params.auth_source` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mongo>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mongo>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mongo>.params.collection` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mongo>.params.connect_timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mongo>.params.database` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mongo>.params.hosts` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mongo>.params.journal` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mongo>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mongo>.params.max_conns` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mongo>.params.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mongo>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mongo>.params.replica_set` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mongo>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mongo>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mongo>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mongo>.params.uri` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mongo>.params.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mongo>.params.write_concern` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mongo>.params.write_timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mongo>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mongo>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mongo>.params.auth_source` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mongo>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mongo>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mongo>.params.collection` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mongo>.params.connect_timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mongo>.params.database` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mongo>.params.hosts` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mongo>.params.journal` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mongo>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mongo>.params.max_conns` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mongo>.params.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mongo>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mongo>.params.replica_set` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mongo>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mongo>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mongo>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mongo>.params.uri` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mongo>.params.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mongo>.params.write_concern` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mongo>.params.write_timeout` | `duration` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-mqtt"></a>

#### `sync.sinks/mqtt`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<mqtt>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.auth_method` | `enum(SCRAM-SHA-1\|SCRAM-SHA-256\|SCRAM-SHA-512)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.client_id` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.content_type` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.keep_alive` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.max_inflight` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.max_packet_size` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.message_expiry` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.qos` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.retain` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.servers` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.topic` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.user_properties` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.version` | `enum(3.1.1\|5)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.will` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.will.content_type` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.will.delay` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.will.message_expiry` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.will.payload` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.will.qos` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.will.retain` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.will.topic` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mqtt>.params.will.user_properties` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.auth_method` | `enum(SCRAM-SHA-1\|SCRAM-SHA-256\|SCRAM-SHA-512)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.client_id` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.content_type` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.keep_alive` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.max_inflight` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.max_packet_size` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.message_expiry` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.qos` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.retain` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.servers` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.topic` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.user_properties` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.version` | `enum(3.1.1\|5)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.will` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.will.content_type` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.will.delay` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.will.message_expiry` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.will.payload` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.will.qos` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.will.retain` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.will.topic` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mqtt>.params.will.user_properties` | `map<string>` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-mssql"></a>

#### `sync.sinks/mssql`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<mssql>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.address` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.authority` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.client_certificate_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.client_id` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.client_secret_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.kdc` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.keytab_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.krb5_conf` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.managed_identity` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.method` | `enum(entra\|kerberos\|ntlm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.realm` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.spn` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.tenant_id` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.token_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.auth.token_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.bulk_rows` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.checkpoint` | `enum(file\|sink)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.checkpoint_table` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.database` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.ddl` | `enum(report\|stop)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.encrypt` | `enum(on\|strict\|off)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.identity_insert` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.table` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.transactions` | `enum(source\|batch)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.upsert` | `enum(merge\|update_insert)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mssql>.params.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.address` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.authority` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.client_certificate_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.client_id` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.client_secret_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.kdc` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.keytab_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.krb5_conf` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.managed_identity` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.method` | `enum(entra\|kerberos\|ntlm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.realm` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.spn` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.tenant_id` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.token_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.auth.token_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.bulk_rows` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.checkpoint` | `enum(file\|sink)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.checkpoint_table` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.database` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.ddl` | `enum(report\|stop)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.encrypt` | `enum(on\|strict\|off)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.identity_insert` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.table` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.transactions` | `enum(source\|batch)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.upsert` | `enum(merge\|update_insert)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mssql>.params.username` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-mysql"></a>

#### `sync.sinks/mysql`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<mysql>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mysql>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mysql>.params.address` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mysql>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mysql>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mysql>.params.checkpoint` | `enum(file\|sink)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mysql>.params.checkpoint_table` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mysql>.params.database` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mysql>.params.ddl` | `enum(report\|stop)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mysql>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mysql>.params.max_transaction_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mysql>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mysql>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mysql>.params.server_public_key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mysql>.params.table` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mysql>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mysql>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mysql>.params.tls_rsa` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mysql>.params.transactions` | `enum(source\|batch)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<mysql>.params.username` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mysql>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mysql>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mysql>.params.address` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mysql>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mysql>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mysql>.params.checkpoint` | `enum(file\|sink)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mysql>.params.checkpoint_table` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mysql>.params.database` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mysql>.params.ddl` | `enum(report\|stop)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mysql>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mysql>.params.max_transaction_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mysql>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mysql>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mysql>.params.server_public_key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mysql>.params.table` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mysql>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mysql>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mysql>.params.tls_rsa` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mysql>.params.transactions` | `enum(source\|batch)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<mysql>.params.username` | `string, required` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-nats"></a>

#### `sync.sinks/nats`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<nats>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.creds_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.max_in_flight` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.mode` | `enum(jetstream\|core)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.nkey_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.servers` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.subject` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.token_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<nats>.params.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.creds_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.max_in_flight` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.mode` | `enum(jetstream\|core)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.nkey_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.servers` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.subject` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.token_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<nats>.params.username` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-otlp-logs"></a>

#### `sync.sinks/otlp_logs`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<otlp_logs>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.attributes` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.bearer_token_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.body` | `enum(line\|row)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.compression` | `enum(gzip\|none)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.encoding` | `enum(protobuf\|json)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.headers_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.resource` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.service_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.severity_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.span_id_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.timestamp_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.trace_id_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.urls` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<otlp_logs>.params.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.attributes` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.bearer_token_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.body` | `enum(line\|row)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.compression` | `enum(gzip\|none)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.encoding` | `enum(protobuf\|json)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.headers_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.resource` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.service_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.severity_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.span_id_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.timestamp_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.trace_id_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.urls` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<otlp_logs>.params.username` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-postgres"></a>

#### `sync.sinks/postgres`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<postgres>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<postgres>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<postgres>.params.address` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<postgres>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<postgres>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<postgres>.params.channel_binding` | `enum(prefer\|require\|disable)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<postgres>.params.checkpoint` | `enum(file\|sink)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<postgres>.params.checkpoint_table` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<postgres>.params.copy` | `enum(auto\|always\|never)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<postgres>.params.copy_min_rows` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<postgres>.params.database` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<postgres>.params.key_columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<postgres>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<postgres>.params.max_statement_rows` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<postgres>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<postgres>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<postgres>.params.sslmode` | `enum(disable\|require\|verify-ca\|verify-full)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<postgres>.params.table` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<postgres>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<postgres>.params.transactions` | `enum(batch\|source)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<postgres>.params.user` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<postgres>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<postgres>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<postgres>.params.address` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<postgres>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<postgres>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<postgres>.params.channel_binding` | `enum(prefer\|require\|disable)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<postgres>.params.checkpoint` | `enum(file\|sink)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<postgres>.params.checkpoint_table` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<postgres>.params.copy` | `enum(auto\|always\|never)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<postgres>.params.copy_min_rows` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<postgres>.params.database` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<postgres>.params.key_columns` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<postgres>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<postgres>.params.max_statement_rows` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<postgres>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<postgres>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<postgres>.params.sslmode` | `enum(disable\|require\|verify-ca\|verify-full)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<postgres>.params.table` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<postgres>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<postgres>.params.transactions` | `enum(batch\|source)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<postgres>.params.user` | `string, required` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-pulsar"></a>

#### `sync.sinks/pulsar`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<pulsar>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.compression` | `enum(none\|lz4\|zlib\|zstd\|snappy)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.dedup` | `enum(read_back\|off)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.deletes` | `enum(tombstone\|encode)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.max_batch_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.max_batch_messages` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.producer_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.token_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.topic` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<pulsar>.params.url` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.compression` | `enum(none\|lz4\|zlib\|zstd\|snappy)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.dedup` | `enum(read_back\|off)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.deletes` | `enum(tombstone\|encode)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.max_batch_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.max_batch_messages` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.producer_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.token_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.topic` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<pulsar>.params.url` | `string, required` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-qdrant"></a>

#### `sync.sinks/qdrant`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<qdrant>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<qdrant>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.api_key_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.collection` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.payload` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.url` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.vector_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<qdrant>.params.vector_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<qdrant>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<qdrant>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<qdrant>.params.api_key_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<qdrant>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<qdrant>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<qdrant>.params.collection` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<qdrant>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<qdrant>.params.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<qdrant>.params.payload` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<qdrant>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<qdrant>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<qdrant>.params.url` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<qdrant>.params.vector_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<qdrant>.params.vector_name` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-rabbitmq"></a>

#### `sync.sinks/rabbitmq`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<rabbitmq>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.addresses` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.auth` | `enum(PLAIN\|EXTERNAL)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.content_type` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.exchange` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.frame_max` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.heartbeat` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.max_message_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.routing_key` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.transient` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.unroutable` | `enum(error\|drop)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<rabbitmq>.params.vhost` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.addresses` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.auth` | `enum(PLAIN\|EXTERNAL)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.content_type` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.exchange` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.frame_max` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.heartbeat` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.max_message_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.routing_key` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.transient` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.unroutable` | `enum(error\|drop)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<rabbitmq>.params.vhost` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-redis"></a>

#### `sync.sinks/redis`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<redis>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<redis>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<redis>.params.address` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<redis>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<redis>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<redis>.params.db` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<redis>.params.entry_id` | `enum(auto\|key)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<redis>.params.form` | `enum(fields\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<redis>.params.key` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<redis>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<redis>.params.max_len` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<redis>.params.max_pipeline` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<redis>.params.mode` | `enum(stream\|keys)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<redis>.params.op_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<redis>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<redis>.params.protocol` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<redis>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<redis>.params.stream` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<redis>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<redis>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<redis>.params.ttl` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<redis>.params.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<redis>.params.value` | `enum(json\|record\|hash)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>.params.address` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>.params.db` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>.params.entry_id` | `enum(auto\|key)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>.params.form` | `enum(fields\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>.params.key` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>.params.max_len` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>.params.max_pipeline` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>.params.mode` | `enum(stream\|keys)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>.params.op_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>.params.protocol` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>.params.stream` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>.params.ttl` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>.params.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<redis>.params.value` | `enum(json\|record\|hash)` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-s3"></a>

#### `sync.sinks/s3`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<s3>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.access_key_id` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.addressing` | `enum(path\|virtual)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.bucket` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.endpoint` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.gzip` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.key` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.max_object_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.region` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.secret_access_key_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.session_token_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<s3>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.access_key_id` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.addressing` | `enum(path\|virtual)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.bucket` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.endpoint` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.gzip` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.key` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.max_object_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.region` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.secret_access_key_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.session_token_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<s3>.params.timeout` | `duration` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-stdout"></a>

#### `sync.sinks/stdout`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<stdout>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<stdout>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<stdout>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<stdout>.params` | `object` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-tdengine"></a>

#### `sync.sinks/tdengine`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<tdengine>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<tdengine>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.database` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.deletes` | `enum(skip\|fail)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.fields` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.measurement` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.mode` | `enum(schemaless\|sql)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.password_env` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.precision` | `enum(ns\|us\|ms)` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.subtable` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.tags` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.time_column` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.time_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.url` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<tdengine>.params.username` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<tdengine>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<tdengine>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<tdengine>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<tdengine>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<tdengine>.params.database` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<tdengine>.params.deletes` | `enum(skip\|fail)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<tdengine>.params.fields` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<tdengine>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<tdengine>.params.max_request_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<tdengine>.params.measurement` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<tdengine>.params.mode` | `enum(schemaless\|sql)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<tdengine>.params.password_env` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<tdengine>.params.precision` | `enum(ns\|us\|ms)` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<tdengine>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<tdengine>.params.subtable` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<tdengine>.params.tags` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<tdengine>.params.time_column` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<tdengine>.params.time_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<tdengine>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<tdengine>.params.url` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<tdengine>.params.username` | `string, required` | 尚不支持（not supported yet） |

<a id="fields-sync-sinks-wasm"></a>

#### `sync.sinks/wasm`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].dead_letter.sink<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<wasm>.params.allow_hosts` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<wasm>.params.max_response_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].dead_letter.sink<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<wasm>.params.allow_hosts` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<wasm>.params.max_response_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].sinks[].sink<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |

<a id="fields-sync-sources-clickhouse"></a>

#### `sync.sources/clickhouse`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].source<clickhouse>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<clickhouse>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<clickhouse>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<clickhouse>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<clickhouse>.params.chunk_rows` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<clickhouse>.params.cursor` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<clickhouse>.params.database` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<clickhouse>.params.deleted_column` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<clickhouse>.params.key` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<clickhouse>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<clickhouse>.params.namespace` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<clickhouse>.params.page_rows` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<clickhouse>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<clickhouse>.params.poll_interval` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<clickhouse>.params.query` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<clickhouse>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<clickhouse>.params.settings` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<clickhouse>.params.snapshot` | `enum(initial\|never)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<clickhouse>.params.table` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<clickhouse>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<clickhouse>.params.urls` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<clickhouse>.params.username` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sources-doris"></a>

#### `sync.sources/doris`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].source<doris>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<doris>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<doris>.params.address` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<doris>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<doris>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<doris>.params.cursor` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<doris>.params.database` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<doris>.params.deleted_column` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<doris>.params.key` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<doris>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<doris>.params.namespace` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<doris>.params.page_rows` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<doris>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<doris>.params.poll_interval` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<doris>.params.query` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<doris>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<doris>.params.snapshot` | `enum(initial\|never)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<doris>.params.table` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<doris>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<doris>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<doris>.params.username` | `string, required` | 尚不支持（not supported yet） |

<a id="fields-sync-sources-elasticsearch"></a>

#### `sync.sources/elasticsearch`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].source<elasticsearch>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<elasticsearch>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<elasticsearch>.params.api_key_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<elasticsearch>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<elasticsearch>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<elasticsearch>.params.chunk_documents` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<elasticsearch>.params.cursor` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<elasticsearch>.params.delay` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<elasticsearch>.params.deleted_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<elasticsearch>.params.index` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<elasticsearch>.params.keep_alive` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<elasticsearch>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<elasticsearch>.params.max_page_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<elasticsearch>.params.page_size` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<elasticsearch>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<elasticsearch>.params.poll_interval` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<elasticsearch>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<elasticsearch>.params.snapshot` | `enum(pit\|cursor\|none)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<elasticsearch>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<elasticsearch>.params.urls` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<elasticsearch>.params.username` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sources-file"></a>

#### `sync.sources/file`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].source<file>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.close_after` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.fingerprint_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.max_files` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.max_line_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.max_open_files` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.multiline` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.multiline.max_lines` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.multiline.start` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.paths` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.poll_interval` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<file>.params.scan_interval` | `duration` | 尚不支持（not supported yet） |

<a id="fields-sync-sources-generate"></a>

#### `sync.sources/generate`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].source<generate>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<generate>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<generate>.params.chunk_records` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<generate>.params.interval` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<generate>.params.namespace` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<generate>.params.records` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<generate>.params.tx_size` | `integer` | 尚不支持（not supported yet） |

<a id="fields-sync-sources-http"></a>

#### `sync.sources/http`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].source<http>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.form` | `enum(plain\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.framing` | `enum(lines\|body)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.max_pending_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.max_pending_requests` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<http>.params.namespace` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sources-kafka"></a>

#### `sync.sources/kafka`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].source<kafka>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.brokers` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.client_id` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.max_wait` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.namespace` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.offset_reset` | `enum(error\|earliest\|latest)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.partition_max_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.read_uncommitted` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.sasl` | `enum(PLAIN\|SCRAM-SHA-256\|SCRAM-SHA-512)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.start` | `enum(earliest\|latest)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.tombstones` | `enum(delete\|skip)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.topics` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<kafka>.params.username` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sources-mongo"></a>

#### `sync.sources/mongo`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].source<mongo>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params.auth_source` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params.batch_size` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params.chunk_documents` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params.collections` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params.connect_timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params.database` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params.full_document` | `enum(updateLookup\|whenAvailable\|required)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params.full_document_before_change` | `enum(off\|whenAvailable\|required)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params.hosts` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params.max_await` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params.max_conns` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params.replica_set` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params.snapshot` | `enum(initial\|never)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params.uri` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mongo>.params.username` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sources-mqtt"></a>

#### `sync.sources/mqtt`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].source<mqtt>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.auth_method` | `enum(SCRAM-SHA-1\|SCRAM-SHA-256\|SCRAM-SHA-512)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.client_id` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.idle` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.keep_alive` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.max_packet_size` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.namespace` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.qos` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.receive_maximum` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.servers` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.session_expiry` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.topics` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.version` | `enum(3.1.1\|5)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.will` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.will.content_type` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.will.delay` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.will.message_expiry` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.will.payload` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.will.qos` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.will.retain` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.will.topic` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mqtt>.params.will.user_properties` | `map<string>` | 尚不支持（not supported yet） |

<a id="fields-sync-sources-mssql"></a>

#### `sync.sources/mssql`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].source<mssql>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.address` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.auth` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.auth.authority` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.auth.client_certificate_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.auth.client_id` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.auth.client_secret_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.auth.kdc` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.auth.keytab_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.auth.krb5_conf` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.auth.managed_identity` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.auth.method` | `enum(entra\|kerberos\|ntlm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.auth.realm` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.auth.spn` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.auth.tenant_id` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.auth.token_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.auth.token_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.batch_size` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.database` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.encrypt` | `enum(on\|strict\|off)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.exclude` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.keys` | `map<array<string>>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.mode` | `enum(cdc\|change_tracking)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.poll_interval` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.snapshot` | `enum(initial\|always\|never)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.tables` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.window_rows` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mssql>.params.window_transactions` | `integer` | 尚不支持（not supported yet） |

<a id="fields-sync-sources-mysql"></a>

#### `sync.sources/mysql`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].source<mysql>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mysql>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mysql>.params.address` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mysql>.params.batch_size` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mysql>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mysql>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mysql>.params.chunk_rows` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mysql>.params.exclude` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mysql>.params.heartbeat` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mysql>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mysql>.params.keys` | `map<array<string>>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mysql>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mysql>.params.server_id` | `integer, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mysql>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mysql>.params.server_public_key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mysql>.params.snapshot` | `enum(initial\|always\|never)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mysql>.params.tables` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mysql>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mysql>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mysql>.params.tls_rsa` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<mysql>.params.username` | `string, required` | 尚不支持（not supported yet） |

<a id="fields-sync-sources-nats"></a>

#### `sync.sources/nats`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].source<nats>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.ack_wait` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.batch` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.buffer` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.consumer` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.create_consumer` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.creds_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.max_ack_pending` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.max_wait` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.metadata_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.mode` | `enum(jetstream\|core)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.namespace` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.nkey_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.queue` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.servers` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.start` | `enum(all\|new)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.stream` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.subjects` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.token_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<nats>.params.username` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sources-postgres"></a>

#### `sync.sources/postgres`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].source<postgres>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params.address` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params.channel_binding` | `enum(prefer\|require\|disable)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params.chunk_rows` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params.connect_timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params.create` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params.database` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params.lag_warning` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params.publication` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params.slot` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params.snapshot` | `enum(initial\|never)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params.snapshot_parallel` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params.spool_dir` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params.sslmode` | `enum(disable\|require\|verify-ca\|verify-full)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params.status_interval` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params.streaming` | `enum(auto\|off)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params.tables` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<postgres>.params.user` | `string, required` | 尚不支持（not supported yet） |

<a id="fields-sync-sources-pulsar"></a>

#### `sync.sources/pulsar`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].source<pulsar>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.metadata_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.mode` | `enum(reader\|subscription)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.namespace` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.receiver_queue` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.start` | `enum(earliest\|latest)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.subscription` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.subscription_type` | `enum(exclusive\|failover)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.token_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.tombstones` | `enum(delete\|skip)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.topics` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<pulsar>.params.url` | `string, required` | 尚不支持（not supported yet） |

<a id="fields-sync-sources-rabbitmq"></a>

#### `sync.sources/rabbitmq`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].source<rabbitmq>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.addresses` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.auth` | `enum(PLAIN\|EXTERNAL)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.frame_max` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.heartbeat` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.idle_end` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.max_message_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.metadata_field` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.namespace` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.prefetch` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.queues` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.stream_start` | `enum(first\|last\|next)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.streams` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<rabbitmq>.params.vhost` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sources-redis"></a>

#### `sync.sources/redis`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].source<redis>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<redis>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<redis>.params.address` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<redis>.params.block` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<redis>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<redis>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<redis>.params.count` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<redis>.params.db` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<redis>.params.form` | `enum(fields\|record)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<redis>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<redis>.params.namespace` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<redis>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<redis>.params.protocol` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<redis>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<redis>.params.snapshot` | `enum(initial\|never)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<redis>.params.streams` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<redis>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<redis>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<redis>.params.username` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sources-s3"></a>

#### `sync.sources/s3`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].source<s3>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.access_key_id` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.addressing` | `enum(path\|virtual)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.bucket` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.decompress` | `enum(auto\|gzip\|none)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.endpoint` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.max_line_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.namespace` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.poll_interval` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.prefix` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.range_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.region` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.secret_access_key_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.session_token_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.start_after` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<s3>.params.timeout` | `duration` | 尚不支持（not supported yet） |

<a id="fields-sync-sources-tidb"></a>

#### `sync.sources/tidb`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].source<tidb>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.address` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka` | `object, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.brokers` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.cert_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.client_id` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec.module` | `enum(canal_json\|csv\|debezium\|json\|line\|raw\|ticdc_simple\|wasm), required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.codec.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.max_wait` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.namespace` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.offset_reset` | `enum(error\|earliest\|latest)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.partition_max_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.read_uncommitted` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.sasl` | `enum(PLAIN\|SCRAM-SHA-256\|SCRAM-SHA-512)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.start` | `enum(earliest\|latest)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.tombstones` | `enum(delete\|skip)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.topics` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.kafka.username` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.key_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.page_rows` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.password_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.protocol` | `enum(canal-json\|simple)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.snapshot` | `enum(initial\|never)` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.tables` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.time_zone` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.tls` | `boolean` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<tidb>.params.username` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-sources-wasm"></a>

#### `sync.sources/wasm`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].source<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<wasm>.params.allow_hosts` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<wasm>.params.max_response_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<wasm>.params.poll_interval` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].source<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |

<a id="fields-sync-transforms-cast"></a>

#### `sync.transforms/cast`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].transforms[]<cast>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<cast>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<cast>.params.fields` | `map<string>, required` | 尚不支持（not supported yet） |

<a id="fields-sync-transforms-embed"></a>

#### `sync.transforms/embed`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].transforms[]<embed>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<embed>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<embed>.params.api_key_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<embed>.params.attempts` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<embed>.params.batch_records` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<embed>.params.ca_file` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<embed>.params.cache_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<embed>.params.dimensions` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<embed>.params.fields` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<embed>.params.into` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<embed>.params.max_parallel` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<embed>.params.max_text_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<embed>.params.model` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<embed>.params.separator` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<embed>.params.server_name` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<embed>.params.timeout` | `duration` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<embed>.params.url` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-transforms-envelope"></a>

#### `sync.transforms/envelope`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].transforms[]<envelope>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<envelope>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<envelope>.params.deletes` | `enum(keep\|drop\|rewrite)` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<envelope>.params.meta` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<envelope>.params.shape` | `enum(flat\|pair\|debezium)` | 尚不支持（not supported yet） |

<a id="fields-sync-transforms-fields"></a>

#### `sync.transforms/fields`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].transforms[]<fields>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<fields>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<fields>.params.drop` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<fields>.params.keep` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<fields>.params.nest` | `array<object>` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<fields>.params.nest[].fields` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<fields>.params.nest[].into` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<fields>.params.rename` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<fields>.params.set` | `map<any>` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<fields>.params.unnest` | `array<string>` | 尚不支持（not supported yet） |

<a id="fields-sync-transforms-filter"></a>

#### `sync.transforms/filter`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].transforms[]<filter>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<filter>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<filter>.params.action` | `enum(keep\|drop)` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<filter>.params.exclude_namespaces` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<filter>.params.namespaces` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<filter>.params.ops` | `array<enum(insert\|update\|delete\|ddl\|message)>` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<filter>.params.where` | `array<object>` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<filter>.params.where[].equals` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<filter>.params.where[].field` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<filter>.params.where[].in` | `array<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<filter>.params.where[].present` | `boolean` | 尚不支持（not supported yet） |

<a id="fields-sync-transforms-key"></a>

#### `sync.transforms/key`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].transforms[]<key>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<key>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<key>.params.fields` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<key>.params.format` | `enum(map\|value\|join)` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<key>.params.separator` | `string` | 尚不支持（not supported yet） |

<a id="fields-sync-transforms-mask"></a>

#### `sync.transforms/mask`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].transforms[]<mask>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<mask>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<mask>.params.fields` | `array<string>, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<mask>.params.key_env` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<mask>.params.with` | `enum(hmac_sha256\|sha256\|null)` | 尚不支持（not supported yet） |

<a id="fields-sync-transforms-route"></a>

#### `sync.transforms/route`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].transforms[]<route>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<route>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<route>.params.map` | `map<string>` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<route>.params.rules` | `array<object>` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<route>.params.rules[].match` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<route>.params.rules[].to` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<route>.params.unmatched` | `enum(keep\|drop)` | 尚不支持（not supported yet） |

<a id="fields-sync-transforms-wasm"></a>

#### `sync.transforms/wasm`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `sync.pipelines[].transforms[]<wasm>` | `module` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<wasm>.params` | `object` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<wasm>.params.batch_records` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<wasm>.params.config` | `string` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<wasm>.params.instances` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<wasm>.params.max_memory_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<wasm>.params.max_output_bytes` | `integer` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<wasm>.params.path` | `string, required` | 尚不支持（not supported yet） |
| `sync.pipelines[].transforms[]<wasm>.params.timeout` | `duration` | 尚不支持（not supported yet） |

<a id="fields-telemetry"></a>

#### `telemetry`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `telemetry` | `object` | 已支持 |
| `telemetry.ca_file` | `string` | 已支持 |
| `telemetry.cert_file` | `string` | 已支持 |
| `telemetry.endpoint` | `string` | 已支持 |
| `telemetry.headers_file` | `string` | 已支持 |
| `telemetry.key_file` | `string` | 已支持 |
| `telemetry.metrics` | `object` | 已支持 |
| `telemetry.metrics.interval` | `duration` | 已支持 |
| `telemetry.service_name` | `string` | 已支持 |
| `telemetry.timeout` | `duration` | 已支持 |
| `telemetry.traces` | `object` | 已支持 |
| `telemetry.traces.interval` | `duration` | 已支持 |
| `telemetry.traces.max_queue_bytes` | `integer` | 已支持 |
| `telemetry.traces.sample_ratio` | `number` | 已支持 |

<a id="fields-tunnel"></a>

#### `tunnel`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `tunnel` | `object` | 已支持 |
| `tunnel.hub` | `object` | 已支持 |
| `tunnel.hub.access` | `object` | 已支持 |
| `tunnel.hub.access.allow` | `array<string>` | 已支持 |
| `tunnel.hub.access.allow_files` | `array<string>` | 已支持 |
| `tunnel.hub.access.deny` | `array<string>` | 已支持 |
| `tunnel.hub.access.deny_files` | `array<string>` | 已支持 |
| `tunnel.hub.allow_proxy_services` | `boolean` | 已支持 |
| `tunnel.hub.body_password` | `string` | 已支持 |
| `tunnel.hub.call_timeout` | `duration` | 已支持 |
| `tunnel.hub.console` | `object` | 已支持 |
| `tunnel.hub.console.access` | `object` | 已支持 |
| `tunnel.hub.console.access.allow` | `array<string>` | 已支持 |
| `tunnel.hub.console.access.allow_files` | `array<string>` | 已支持 |
| `tunnel.hub.console.access.deny` | `array<string>` | 已支持 |
| `tunnel.hub.console.access.deny_files` | `array<string>` | 已支持 |
| `tunnel.hub.console.address` | `string, required` | 已支持 |
| `tunnel.hub.console.cert_file` | `string` | 已支持 |
| `tunnel.hub.console.key_file` | `string` | 已支持 |
| `tunnel.hub.console.max_conns` | `integer` | 已支持 |
| `tunnel.hub.console.max_sessions` | `integer` | 已支持 |
| `tunnel.hub.console.mptcp` | `boolean` | 尚不支持（not supported yet） |
| `tunnel.hub.console.session_lifetime` | `duration` | 已支持 |
| `tunnel.hub.console.session_timeout` | `duration` | 已支持 |
| `tunnel.hub.console.users` | `array<object>, required` | 已支持 |
| `tunnel.hub.console.users[].password_hash` | `string, required` | 已支持 |
| `tunnel.hub.console.users[].username` | `string, required` | 已支持 |
| `tunnel.hub.console_socket` | `object` | 已支持 |
| `tunnel.hub.console_socket.address` | `string, required` | 已支持 |
| `tunnel.hub.console_socket.max_conns` | `integer` | 已支持 |
| `tunnel.hub.console_socket.password` | `string` | 已支持 |
| `tunnel.hub.listeners` | `array<object>` | 已支持 |
| `tunnel.hub.listeners[].address` | `string, required` | 已支持 |
| `tunnel.hub.listeners[].cert_file` | `string` | 已支持 |
| `tunnel.hub.listeners[].key_file` | `string` | 已支持 |
| `tunnel.hub.listeners[].path` | `string` | 已支持 |
| `tunnel.hub.listeners[].protocol` | `enum(tcp\|tls\|ws\|wss\|quic), required` | 已支持 |
| `tunnel.hub.max_links` | `integer` | 已支持 |
| `tunnel.hub.max_links_per_peer` | `integer` | 已支持 |
| `tunnel.hub.max_peers` | `integer` | 已支持 |
| `tunnel.hub.max_tunnels` | `integer` | 已支持 |
| `tunnel.hub.max_unregistered` | `integer` | 已支持 |
| `tunnel.hub.max_unregistered_per_ip` | `integer` | 已支持 |
| `tunnel.hub.name` | `string` | 已支持 |
| `tunnel.hub.password` | `string` | 已支持 |
| `tunnel.hub.quic` | `object` | 已支持 |
| `tunnel.hub.quic.cert` | `string` | 已支持 |
| `tunnel.hub.quic.handshake_timeout` | `duration` | 已支持 |
| `tunnel.hub.quic.idle_timeout` | `duration` | 已支持 |
| `tunnel.hub.quic.keep_alive` | `duration` | 已支持 |
| `tunnel.hub.quic.key` | `string` | 已支持 |
| `tunnel.hub.quic.max_handshakes` | `integer` | 已支持 |
| `tunnel.hub.register_timeout` | `duration` | 已支持 |
| `tunnel.hub.tokens` | `array<object>` | 已支持 |
| `tunnel.hub.tokens[].created_at` | `integer` | 已支持 |
| `tunnel.hub.tokens[].expires_at` | `integer` | 已支持 |
| `tunnel.hub.tokens[].hash` | `string, required` | 已支持 |
| `tunnel.hub.tokens[].id` | `string, required` | 已支持 |
| `tunnel.hub.tokens[].members` | `array<string>` | 已支持 |
| `tunnel.hub.tokens[].name` | `string, required` | 已支持 |
| `tunnel.hub.tokens[].note` | `string` | 已支持 |
| `tunnel.node` | `object` | 已支持 |
| `tunnel.node.allow_hub_services` | `boolean` | 已支持 |
| `tunnel.node.console` | `object` | 已支持 |
| `tunnel.node.console.access` | `object` | 已支持 |
| `tunnel.node.console.access.allow` | `array<string>` | 已支持 |
| `tunnel.node.console.access.allow_files` | `array<string>` | 已支持 |
| `tunnel.node.console.access.deny` | `array<string>` | 已支持 |
| `tunnel.node.console.access.deny_files` | `array<string>` | 已支持 |
| `tunnel.node.console.address` | `string, required` | 已支持 |
| `tunnel.node.console.cert_file` | `string` | 已支持 |
| `tunnel.node.console.key_file` | `string` | 已支持 |
| `tunnel.node.console.max_conns` | `integer` | 已支持 |
| `tunnel.node.console.max_sessions` | `integer` | 已支持 |
| `tunnel.node.console.mptcp` | `boolean` | 尚不支持（not supported yet） |
| `tunnel.node.console.session_lifetime` | `duration` | 已支持 |
| `tunnel.node.console.session_timeout` | `duration` | 已支持 |
| `tunnel.node.console.users` | `array<object>, required` | 已支持 |
| `tunnel.node.console.users[].password_hash` | `string, required` | 已支持 |
| `tunnel.node.console.users[].username` | `string, required` | 已支持 |
| `tunnel.node.dial_timeout` | `duration` | 已支持 |
| `tunnel.node.hubs` | `array<object>` | 已支持 |
| `tunnel.node.hubs[].body_password` | `string` | 已支持 |
| `tunnel.node.hubs[].ca_file` | `string` | 已支持 |
| `tunnel.node.hubs[].call_timeout` | `duration` | 已支持 |
| `tunnel.node.hubs[].dial_timeout` | `duration` | 已支持 |
| `tunnel.node.hubs[].endpoints` | `array<object>, required` | 已支持 |
| `tunnel.node.hubs[].endpoints[].address` | `string, required` | 已支持 |
| `tunnel.node.hubs[].endpoints[].disabled` | `boolean` | 已支持 |
| `tunnel.node.hubs[].name` | `string` | 已支持 |
| `tunnel.node.hubs[].password` | `string` | 已支持 |
| `tunnel.node.hubs[].pin` | `string` | 已支持 |
| `tunnel.node.hubs[].ping_interval` | `duration` | 已支持 |
| `tunnel.node.hubs[].ping_timeout` | `duration` | 已支持 |
| `tunnel.node.hubs[].pool_size` | `integer` | 已支持 |
| `tunnel.node.hubs[].quic_insecure` | `boolean` | 已支持 |
| `tunnel.node.hubs[].quic_pin` | `string, deprecated` | 已支持 |
| `tunnel.node.hubs[].retry_interval` | `duration` | 已支持 |
| `tunnel.node.hubs[].token` | `string` | 已支持 |
| `tunnel.node.idle_timeout` | `duration` | 已支持 |
| `tunnel.node.max_tunnels` | `integer` | 已支持 |
| `tunnel.node.meta` | `map<string>` | 已支持 |
| `tunnel.node.name` | `string` | 已支持 |
| `tunnel.node.services` | `array<object>` | 已支持 |
| `tunnel.node.services[].address` | `string, required` | 已支持 |
| `tunnel.node.services[].description` | `string` | 已支持 |
| `tunnel.node.services[].name` | `string, required` | 已支持 |
| `tunnel.proxy` | `object` | 已支持 |
| `tunnel.proxy.access` | `object` | 已支持 |
| `tunnel.proxy.access.allow` | `array<string>` | 已支持 |
| `tunnel.proxy.access.allow_files` | `array<string>` | 已支持 |
| `tunnel.proxy.access.deny` | `array<string>` | 已支持 |
| `tunnel.proxy.access.deny_files` | `array<string>` | 已支持 |
| `tunnel.proxy.console` | `object` | 已支持 |
| `tunnel.proxy.console.access` | `object` | 已支持 |
| `tunnel.proxy.console.access.allow` | `array<string>` | 已支持 |
| `tunnel.proxy.console.access.allow_files` | `array<string>` | 已支持 |
| `tunnel.proxy.console.access.deny` | `array<string>` | 已支持 |
| `tunnel.proxy.console.access.deny_files` | `array<string>` | 已支持 |
| `tunnel.proxy.console.address` | `string, required` | 已支持 |
| `tunnel.proxy.console.cert_file` | `string` | 已支持 |
| `tunnel.proxy.console.key_file` | `string` | 已支持 |
| `tunnel.proxy.console.max_conns` | `integer` | 已支持 |
| `tunnel.proxy.console.max_sessions` | `integer` | 已支持 |
| `tunnel.proxy.console.mptcp` | `boolean` | 尚不支持（not supported yet） |
| `tunnel.proxy.console.session_lifetime` | `duration` | 已支持 |
| `tunnel.proxy.console.session_timeout` | `duration` | 已支持 |
| `tunnel.proxy.console.users` | `array<object>, required` | 已支持 |
| `tunnel.proxy.console.users[].password_hash` | `string, required` | 已支持 |
| `tunnel.proxy.console.users[].username` | `string, required` | 已支持 |
| `tunnel.proxy.forwards` | `array<object>` | 已支持 |
| `tunnel.proxy.forwards[].hub` | `string` | 已支持 |
| `tunnel.proxy.forwards[].listen` | `string, required` | 已支持 |
| `tunnel.proxy.forwards[].node` | `string, required` | 已支持 |
| `tunnel.proxy.forwards[].service` | `string, required` | 已支持 |
| `tunnel.proxy.hubs` | `array<object>` | 已支持 |
| `tunnel.proxy.hubs[].body_password` | `string` | 已支持 |
| `tunnel.proxy.hubs[].ca_file` | `string` | 已支持 |
| `tunnel.proxy.hubs[].call_timeout` | `duration` | 已支持 |
| `tunnel.proxy.hubs[].dial_timeout` | `duration` | 已支持 |
| `tunnel.proxy.hubs[].endpoints` | `array<object>, required` | 已支持 |
| `tunnel.proxy.hubs[].endpoints[].address` | `string, required` | 已支持 |
| `tunnel.proxy.hubs[].endpoints[].disabled` | `boolean` | 已支持 |
| `tunnel.proxy.hubs[].name` | `string` | 已支持 |
| `tunnel.proxy.hubs[].password` | `string` | 已支持 |
| `tunnel.proxy.hubs[].pin` | `string` | 已支持 |
| `tunnel.proxy.hubs[].ping_interval` | `duration` | 已支持 |
| `tunnel.proxy.hubs[].ping_timeout` | `duration` | 已支持 |
| `tunnel.proxy.hubs[].pool_size` | `integer` | 已支持 |
| `tunnel.proxy.hubs[].quic_insecure` | `boolean` | 已支持 |
| `tunnel.proxy.hubs[].quic_pin` | `string, deprecated` | 已支持 |
| `tunnel.proxy.hubs[].retry_interval` | `duration` | 已支持 |
| `tunnel.proxy.hubs[].token` | `string` | 已支持 |
| `tunnel.proxy.idle_timeout` | `duration` | 已支持 |
| `tunnel.proxy.max_tunnels` | `integer` | 已支持 |
| `tunnel.proxy.meta` | `map<string>` | 已支持 |
| `tunnel.proxy.name` | `string` | 已支持 |
| `tunnel.proxy.open_timeout` | `duration` | 已支持 |
| `tunnel.proxy.web_proxies` | `array<object>` | 已支持 |
| `tunnel.proxy.web_proxies[].listen` | `string, required` | 已支持 |
| `tunnel.proxy.web_proxies[].max_conns` | `integer` | 已支持 |
| `tunnel.proxy.web_proxies[].name` | `string` | 已支持 |
| `tunnel.proxy.web_proxies[].password` | `string, required` | 已支持 |
| `tunnel.proxy.web_proxies[].url` | `string, required` | 已支持 |

<a id="fields-upstreams"></a>

#### `upstreams`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `upstreams` | `array<object>` | 已支持 |
| `upstreams[].addresses` | `array<string>, required` | 已支持 |
| `upstreams[].connect_timeout` | `duration` | 已支持 |
| `upstreams[].deny_internal` | `boolean` | 尚不支持（not supported yet） |
| `upstreams[].fail_timeout` | `duration` | 已支持 |
| `upstreams[].hash_key` | `string` | 已支持 |
| `upstreams[].health_check` | `object` | 已支持 |
| `upstreams[].health_check.expect_status` | `array<integer>` | 已支持 |
| `upstreams[].health_check.fails` | `integer` | 已支持 |
| `upstreams[].health_check.host` | `string` | 已支持 |
| `upstreams[].health_check.interval` | `duration` | 已支持 |
| `upstreams[].health_check.jitter` | `duration` | 已支持 |
| `upstreams[].health_check.passes` | `integer` | 已支持 |
| `upstreams[].health_check.path` | `string` | 已支持 |
| `upstreams[].health_check.port` | `integer` | 已支持 |
| `upstreams[].health_check.timeout` | `duration` | 已支持 |
| `upstreams[].health_check.type` | `enum(http\|tcp)` | 已支持 |
| `upstreams[].idle_timeout` | `duration` | 已支持 |
| `upstreams[].load_balancing` | `enum(round_robin\|weighted_round_robin\|least_conn\|random\|p2c\|ip_hash\|uri_hash\|header_hash\|cookie_hash)` | 已支持 |
| `upstreams[].max_conns` | `integer` | 已支持 |
| `upstreams[].max_fails` | `integer` | 已支持 |
| `upstreams[].max_idle_conns` | `integer` | 已支持 |
| `upstreams[].max_pending` | `integer` | 已支持 |
| `upstreams[].name` | `string, required` | 已支持 |
| `upstreams[].protocol` | `enum(http/1.1\|h2c\|h2)` | 已支持 |
| `upstreams[].response_header_timeout` | `duration` | 已支持 |
| `upstreams[].response_idle_timeout` | `duration` | 已支持 |
| `upstreams[].response_timeout` | `duration` | 已支持 |
| `upstreams[].sticky_cookie` | `object` | 已支持 |
| `upstreams[].sticky_cookie.max_age` | `duration` | 已支持 |
| `upstreams[].sticky_cookie.name` | `string` | 已支持 |
| `upstreams[].sticky_cookie.path` | `string` | 已支持 |
| `upstreams[].sticky_cookie.secure` | `boolean` | 已支持 |
| `upstreams[].tls_ca_file` | `string` | 已支持 |
| `upstreams[].tls_cert_file` | `string` | 已支持 |
| `upstreams[].tls_key_file` | `string` | 已支持 |
| `upstreams[].tls_server_name` | `string` | 已支持 |
| `upstreams[].weights` | `array<integer>` | 已支持 |

<a id="fields-webrtc"></a>

#### `webrtc`

| 字段 | 类型 | 状态 |
| --- | --- | --- |
| `webrtc` | `object` | 已支持 |
| `webrtc.media` | `object` | 已支持 |
| `webrtc.media.access` | `object` | 已支持 |
| `webrtc.media.access.allow` | `array<string>` | 已支持 |
| `webrtc.media.access.allow_files` | `array<string>` | 已支持 |
| `webrtc.media.access.deny` | `array<string>` | 已支持 |
| `webrtc.media.access.deny_files` | `array<string>` | 已支持 |
| `webrtc.media.codecs` | `array<string>` | 已支持 |
| `webrtc.media.connect_timeout` | `duration` | 已支持 |
| `webrtc.media.ice_servers` | `array<object>` | 已支持 |
| `webrtc.media.ice_servers[].credential` | `string` | 已支持 |
| `webrtc.media.ice_servers[].urls` | `array<string>, required` | 已支持 |
| `webrtc.media.ice_servers[].username` | `string` | 已支持 |
| `webrtc.media.ice_timeout` | `duration` | 已支持 |
| `webrtc.media.key_frame_interval` | `duration` | 已支持 |
| `webrtc.media.listen` | `string, required` | 已支持 |
| `webrtc.media.max_bitrate` | `integer` | 已支持 |
| `webrtc.media.max_peer_connections` | `integer` | 已支持 |
| `webrtc.media.max_sdp_bytes` | `integer` | 已支持 |
| `webrtc.media.max_streams` | `integer` | 已支持 |
| `webrtc.media.max_viewers` | `integer` | 已支持 |
| `webrtc.media.public_ips` | `array<string>` | 已支持 |
| `webrtc.media.turn_credential_ttl` | `duration` | 已支持 |
| `webrtc.media.turn_urls` | `array<string>` | 已支持 |
| `webrtc.proxy` | `object` | 已支持 |
| `webrtc.proxy.access` | `object` | 已支持 |
| `webrtc.proxy.access.allow` | `array<string>` | 已支持 |
| `webrtc.proxy.access.allow_files` | `array<string>` | 已支持 |
| `webrtc.proxy.access.deny` | `array<string>` | 已支持 |
| `webrtc.proxy.access.deny_files` | `array<string>` | 已支持 |
| `webrtc.proxy.idle_timeout` | `duration` | 已支持 |
| `webrtc.proxy.listen` | `string, required` | 已支持 |
| `webrtc.proxy.max_relays` | `integer` | 已支持 |
| `webrtc.proxy.max_sessions` | `integer` | 已支持 |
| `webrtc.proxy.port_max` | `integer` | 已支持 |
| `webrtc.proxy.port_min` | `integer` | 已支持 |
| `webrtc.proxy.public_ips` | `array<string>` | 已支持 |
| `webrtc.proxy.relay_ip` | `string` | 已支持 |
| `webrtc.proxy.session_timeout` | `duration` | 已支持 |
| `webrtc.turn` | `object` | 已支持 |
| `webrtc.turn.access` | `object` | 已支持 |
| `webrtc.turn.access.allow` | `array<string>` | 已支持 |
| `webrtc.turn.access.allow_files` | `array<string>` | 已支持 |
| `webrtc.turn.access.deny` | `array<string>` | 已支持 |
| `webrtc.turn.access.deny_files` | `array<string>` | 已支持 |
| `webrtc.turn.allowed_peers` | `array<string>` | 已支持 |
| `webrtc.turn.default_lifetime` | `duration` | 已支持 |
| `webrtc.turn.denied_peers` | `array<string>` | 已支持 |
| `webrtc.turn.listen` | `array<string>, required` | 已支持 |
| `webrtc.turn.listen_tcp` | `array<string>` | 已支持 |
| `webrtc.turn.listen_tls` | `array<string>` | 已支持 |
| `webrtc.turn.max_allocations` | `integer` | 已支持 |
| `webrtc.turn.max_allocations_per_user` | `integer` | 已支持 |
| `webrtc.turn.max_channels` | `integer` | 已支持 |
| `webrtc.turn.max_lifetime` | `duration` | 已支持 |
| `webrtc.turn.max_permissions` | `integer` | 已支持 |
| `webrtc.turn.max_stream_message` | `integer` | 已支持 |
| `webrtc.turn.mptcp` | `boolean` | 已支持 |
| `webrtc.turn.nonce_lifetime` | `duration` | 已支持 |
| `webrtc.turn.nonce_secret` | `string` | 已支持 |
| `webrtc.turn.port_max` | `integer` | 已支持 |
| `webrtc.turn.port_min` | `integer` | 已支持 |
| `webrtc.turn.public_ip` | `string` | 已支持 |
| `webrtc.turn.realm` | `string, required` | 已支持 |
| `webrtc.turn.relay_ip` | `string, required` | 已支持 |
| `webrtc.turn.secret` | `string` | 已支持 |
| `webrtc.turn.stream_idle_timeout` | `duration` | 已支持 |
| `webrtc.turn.tls` | `object` | 已支持 |
| `webrtc.turn.tls.certificates` | `array<object>, required` | 已支持 |
| `webrtc.turn.tls.certificates[].cert_file` | `string, required` | 已支持 |
| `webrtc.turn.tls.certificates[].key_file` | `string, required` | 已支持 |
| `webrtc.turn.user_bandwidth` | `integer` | 已支持 |
| `webrtc.turn.users` | `array<object>` | 已支持 |
| `webrtc.turn.users[].key` | `string` | 已支持 |
| `webrtc.turn.users[].name` | `string, required` | 已支持 |
| `webrtc.turn.users[].password` | `string` | 已支持 |

<!-- END GENERATED -->
