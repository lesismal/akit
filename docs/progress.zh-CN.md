# akit 开发进度

[English](progress.md) | [简体中文](progress.zh-CN.md)

本文件按迭代记录实际完成的内容，以及下一次会话从哪里继续。[开发计划](dev-plan.zh-CN.md) 说明每个迭代应该做
什么；本文件说明实际做了什么，以及与计划不同的地方。

## 当前状态

| | |
| --- | --- |
| 最后更新 | 2026-11-24 |
| 里程碑 | M0 基础：I01–I06、I07a、I07b 已完成。 |
| akio | akio `main` 上的 `50a82315`（`1.0.0-rc.1`），在 `Cargo.toml` 中按 `rev` 锁定 |
| fit | 作为规格；本计划基于 `7daee573` 阅读 |
| 可用功能 | `akit run` 和 `akit validate`，带 fit 的参数和退出码（`-c` 可重复，文件和目录合并），SIGINT 和 SIGTERM 时优雅关闭；`akit run` 按 fit 的 host 与 path 规则提供 HTTP/1.1 和 h2c 监听器上的 `respond` 路由，没有匹配时返回 404；`akit version` 和 `akit help`；工作区能构建并通过 `make check`；`akit_config::from_json` 能读取 fit 的监听器、路由和 `respond` handler，fit 配置的其余部分按名字报为暂不支持；模块注册表和数据面 trait（`akit-runtime`）；`compile` 把配置编译成 `Graph`，`GraphCell` 负责发布、读取和退役图；兼容性门禁在 `make check` 和 CI 中把 fit 的每个夹具交给 akit-config；带连接限制的 `upstreams`，图中每个上游一个基于 akio HTTP/1 客户端的连接池，图退役时排空（I14；通过它转发的代理是 I15） |
| 下一步 | I08 的基线（来自基准测试主机），M0 的最后一项；然后是 I09（TLS）。 |
| akio 需求 | A1–A8 现在全部加进 akio，各在 akio 的独立分支上进行；A9–A11（HTTP 客户端：关闭空闲连接、请求自己的 `Host`、单个地址的连接超时）由 I14 提出（[akio API 需求](akio-api-requests.zh-CN.md)） |

## 日志

### 2026-11-24：I14，基于 HTTP/1 客户端的上游连接池

- 读取 `upstreams`（`akit_config::Upstream`），多个文件按 fit 的方式合并：`name`、`addresses`、`protocol`
  （`http/1.1`；`h2c` 和 `h2` 报为暂不支持）、`max_conns`、`max_pending`、`max_idle_conns`、`idle_timeout` 和
  `connect_timeout`，默认值与 fit 相同（10000、50000、32、60s、10s；除 `max_idle_conns` 外 0 都表示默认值，
  `max_idle_conns` 为 0 表示不保留空闲连接），检查和消息也与 fit 相同（名字合法且在上游中唯一、至少一个地址、每个
  地址是主机加 1 到 65535 的端口且不是 URL、不重复）。fit 的时长是 Go 的格式（`akit_config::duration`），负值被拒绝。
  fit 上游的其余 15 个字段按名字报为暂不支持。
- `akit-upstream`：每个上游一个基于 akio-http `Client` 的 `Pool`。每个地址最多 `max_conns` 个进行中的请求，
  这也限制了连接数（HTTP/1.1 每个连接同时只承载一个请求，akio 的客户端先复用空闲连接再拨号，并在回调之前把连接放回
  空闲池）；之后的请求按顺序等待，最多 `max_pending` 个，再多一个立即以 `Error::Busy` 失败。没有人等待时，取槽位只是
  一次比较并交换；只有到达 `max_conns` 时才会取该地址队列的锁。排队的请求由释放其槽位的完成回调启动，从不递归
  （线程局部的蹦床）。`max_idle_conns` 和 `idle_timeout` 对应 akio 每个主机的空闲池；`connect_timeout` 对应 akio
  Happy Eyeballs 每次尝试的超时；每次交换（包括等待连接的时间）受 fit 的 `response_timeout` 默认值限制（1h，该字段
  本身属于 I17）。`pick` 依次给出各地址（fit 的 `round_robin`，即其默认值；其余策略属于 I19）。
  `Error::is_upstream_failure` 说明哪些失败计入地址（I20）。
- 排空：`Pool::close` 以 `Error::Closed` 拒绝新请求，让之前发出的请求（包括排队的）完成；最后一个完成后连接池即
  关闭（`is_closed`、`on_closed`）。`compile` 先于模块创建连接池，通过 `Env::upstream` 交给模块（I15 的代理在
  provision 时取用），保存在图中（`Graph::upstream`），并最先登记它们的清理，使其最后执行：退役的图，或编译失败的
  配置，都会关闭其连接池（决策 004）。
- 引擎：连接池随图创建，早于引擎（引擎由图的监听器构建），所以它们通过 `EngineSlot` 取得引擎，`akit run` 在构建
  引擎后填入；连接在监听器的线程上、在同一个引擎上拨号。`akit validate` 不填这个槽：它的连接池对每个请求都以
  `Error::NoEngine` 失败，从不拨号。
- 兼容性门禁：支持 9 个上游字段（共 29 个）；`templates/json/fit.json` 现在有效，`templates/json/conf.d` 只因
  `proxy` handler 报为暂不支持（`make compat-bless`）。
- 测试：`akit-config/tests/upstreams.rs`（5 个：默认值、限制、fit 的检查与消息、暂不支持的字段、跨文件合并）和
  Go 时长；`akit-upstream/tests/pool.rs`（8 个：`max_conns` 和 `max_pending` 在上限和超过上限时、`max_pending` 为 0
  即 fit 的默认值、保留空闲连接、不保留以及 `idle_timeout` 后关闭、4 个进行中 16 个排队时负载下排空、空闲的连接池
  立即关闭、拒绝连接的上游（单独以及与正常上游并存）、没有引擎以及稍后填入槽、错误的目标和排队时超时）；
  `akit-runtime/tests/upstreams.rs`（3 个：编译出连接池并交给模块、编译失败时关闭连接池、退役的图在负载下排空其
  连接池）；`akit-listener/tests/upstreams.rs`（2 个：连接池在监听器的引擎上拨号、对从不应答的地址的
  `connect_timeout`）；门禁的字段样例。
- 已检查：macOS 上 `make check`；在 Linux（基准测试主机）上以 epoll 和 io_uring 运行 akit-config、akit-upstream、
  akit-runtime、akit-listener、akit 和 akit-compat 的测试，全部通过。连接超时测试需要一个让连接无人应答的地址
  （Linux 上占满的 accept 队列），在 macOS 上跳过。
- 与计划的不同：
  - `max_conns` 限制每个地址进行中的请求，而不是统计套接字：在 HTTP/1.1 上是同一个上限（测试检查上游看到的连接
    从不超过它），而 akio 的客户端不对外暴露其连接。
  - 等待连接的请求没有自己的定时器：等待时间计入交换的超时，在取得连接时检查（fib 的客户端也这样限制等待）；
    队列满时立即拒绝。
  - akio 欠缺之处的临时办法：已关闭连接池的空闲连接保持到其 `idle_timeout` 或引擎关闭（A10，`Client::close_idle`）；
    akio 只对解析出多个地址的名字使用客户端的连接超时，所以 `akit_listener::engine` 把引擎的 `connect_timeout`
    设为各上游中最短的那个（A12）。I15 需要 A11（请求自己的 `Host`）。给 I15 的交接说明在会话笔记目录中。
  - 这里没做的：重载时在图之间保留连接池（fit 的 `Acquire`），属于 I22。

| 最后更新 | 2026-11-23 |
| 里程碑 | M0 基础已完成（I01–I08）；接下来是 M1，代理核心。 |
| akio | akio `main` 上的 `50a82315`（`1.0.0-rc.1`），在 `Cargo.toml` 中按 `rev` 锁定 |
| fit | 作为规格；本计划基于 `7daee573` 阅读 |
| 可用功能 | `akit run` 和 `akit validate`，带 fit 的参数和退出码（`-c` 可重复，文件和目录合并），SIGINT 和 SIGTERM 时优雅关闭；`akit run` 按 fit 的 host 与 path 规则提供 HTTP/1.1 和 h2c 监听器上的 `respond` 路由，没有匹配时返回 404；`akit version` 和 `akit help`；工作区能构建并通过 `make check`；`akit_config::from_json` 能读取 fit 的监听器、路由和 `respond` handler，fit 配置的其余部分按名字报为暂不支持；模块注册表和数据面 trait（`akit-runtime`）；`compile` 把配置编译成 `Graph`，`GraphCell` 负责发布、读取和退役图；兼容性门禁在 `make check` 和 CI 中把 fit 的每个夹具交给 akit-config |
| 下一步 | M1：I09（TLS）、I11（路由器 v1）、I14（上游连接池）、I23（.conf）和 I24（YAML）已在各自的分支上，待合并；然后是 I10、I12、I15 及之后。 |
| akio 需求 | A1–A8 现在全部加进 akio，各在 akio 的独立分支上进行（[akio API 需求](akio-api-requests.zh-CN.md)） |

## 日志

### 2026-11-24：I09，从文件加载证书的 TLS 入口

- `akit-config` 读取监听器的 `tls` 和 `h2` 协议，字段、默认值和错误与 fit 一致：`tls.certificates[]`
  （`cert_file`、`key_file`，都必填）、`tls.client_auth`（`ca_files`，必填，至少一个；`mode`，默认 `require`，或
  `verify_if_given`）以及 `tls.max_version`（`"1.3"`，带引号或不带）。有 `tls` 而没写 `protocols` 的监听器使用 fit
  的 `DefaultTLSProtocols`：`http/1.1` 和 `h2`。fit 的检查：`h2` 需要 `tls`，有 `tls` 的监听器拒绝 `h2c`，`mode`
  只能取两个值之一。报为暂不支持的：`tls.acme`、`tls.on_demand`、`tls.kernel_tls_offload`、`max_version` 为 `1.2`，
  以及没有证书的 `tls`（fit 会用 ACME 为路由的主机名获取证书）。
- `akit-tls`（`server_config`）像 fit 的 `tlsmgr.LoadStatic` 和 `LoadClientAuth` 那样，把监听器的 `tls` 加载成
  akio-tls 的 `ServerConfig`：PEM 文件最大 1 MiB，每个私钥都与其证书核对，每个问题报在
  `listeners[i].tls.certificates[j]` 或 `.client_auth.ca_files[j]`，并带上文件路径。按 fit 的规则以 SNI 选证书：
  subjectAltName 中的名字（DNS、IP）精确包含该名字、或被第一个证书的 `*.` 通配符包含的第一个证书，否则用第一个。
  ALPN 按协议生成，与 fit 的 `tlsmgr.ServerConfig` 一致：`h2, http/1.1`、`h2` 或 `http/1.1`。客户端认证用
  `client_auth_required` 或 `client_auth_optional`。会话票据打开，与 Go 的服务端一致。
- 版本和密码套件无需设置：akio-tls 提供 TLS 1.2 和 1.3，1.2 只有 fit 允许的六个 ECDHE AEAD 套件（AES-GCM 和
  ChaCha20-Poly1305）。
- 运行时图（`akit_runtime::compile`）像 fit 的图一样加载每个 TLS 监听器的文件，所以 `akit validate` 也会检查它们；
  `Graph::tls(i)` 给出配置。`akit_listener::engine` 用 `akio::tls::Tls::server` 包装 TLS 监听器的 HTTP acceptor；
  ALPN 选中 h2 时 akio-http 的服务端走 HTTP/2。只有 `h2` 的监听器在握手中拒绝只提供 `http/1.1` 的客户端，对不带 ALPN
  的客户端提供 HTTP/1.1，与 Go 的服务端一致。
- 基准：`bench/configs/akit/fit.json` 像 fit 一样在 8443 提供 HTTPS/1.1、在 8445 提供 h2，证书是 fitbench 挂在
  `/certs` 的那份；`bench/fitbench/akit.go` 挂载它并保持空闲 TLS 连接；`run.sh` 和 `ab.sh` 默认运行 `https/1.1` 和
  `h2`。没有运行基准。
- akio：`akio-x509`（证书中的名字）成为锁定提交上的新工作区依赖，`akio-crypto` 是测试依赖；akio-tls 通过
  `akio::tls` 使用。新需求 A9：把 TLS 服务端限制在 1.2 的公开方式（[akio API 需求](akio-api-requests.zh-CN.md)）；
  A2 记下第一个证书以外的通配符证书要等它的解析器。
- 兼容性门禁标记了 akit 读取的八个 `listeners[].tls` 字段（`supported.txt`，共 28 个字段）；报告没有变化（用到
  `tls` 的夹具都是 `.conf` 和 YAML 文件）。
- 测试：`akit-config`（3 个：读取、fit 的问题及其位置、暂不支持的字段）；`akit-tls`（3 个单元测试：SNI 选择、
  ALPN、IP 名字；3 个文件测试：加载、每个问题的路径、第一个证书以外的通配符）；`akit-listener/tests/tls.rs`（5 个：
  按 ALPN 和不带 ALPN 的 HTTPS/1.1 与 h2、ALPN 跟随协议、SNI 选证书、要求和可选的客户端认证、握手中途断开的客户端和
  使用明文 HTTP 的客户端不影响其他连接）；`akit/tests/https.rs`（2 个，用 `akit run` 端到端：经 HTTPS 以 HTTP/1.1、
  HTTP/2 和按 SNI 访问路由；`validate` 和 `run` 拒绝缺失的证书文件并给出路径）。证书由测试用 `akio-x509` 生成。
- 与计划的不同：`max_version` 为 `1.2` 要等 akio（A9）；第一个证书以外的证书中的通配符名字被拒绝（akio-tls 按名字
  精确查找，A2）；两个证书包含同一名字时 akit 提供第一个，而 fit 提供客户端支持（RSA 或 ECDSA）的第一个；握手超时用
  akio 默认的 10 秒，这里没有测到上限（akio 测了）。
- 检查：macOS 上的 `make check`；改动的 crate（akit-config、akit-tls、akit-runtime、akit-listener、akit、akit-compat）在 Linux（gpu51）上的 epoll 和 io_uring 测试，全部通过。

### 2026-11-24：I24，YAML 读取器

- [决定 005](decisions/005-yaml-reader.zh-CN.md)：YAML 由 akit 自己的读取器读取，不用 crate。serde_yaml 及其分支
  用 unsafe-libyaml 解析（通篇 `unsafe`），且不保留位置；yaml-rust2 和 saphyr 是纯 Rust，但自带 crate，并且对
  YAML 边角的读法与 fit 基于 libyaml 的库不同，要与 fit 一致，大部分工作仍得在它们之上自己写。
- `akit-config` 的 `yaml` 模块（`crates/akit-config/src/yaml.rs`，约 1,700 行代码，无依赖，无 `unsafe`）把 YAML
  流解析成 YAML 节点树（锚点共享），再像 fit 的 `adapter/yaml` 把它转成 JSON 那样，转成 JSON 读取器的带字节偏移的
  `Node` 树：块集合与流集合，普通、带引号、字面和折叠标量，显式键，锚点、别名和合并键（`<<`），核心 schema 的标签，
  `%YAML`、`---` 和 `...`；标量按 fit 的库解析，数字按 fit 的适配器的方式写出；重复键、非标量键、未知标签、
  `.inf`/`.nan`、第二个文档以及经别名展开超过 2^20 个节点或 64 MiB 都会被拒绝，与 fit 相同；空文件即空配置。模型的
  错误指向 YAML 中的行和列。
- `Source` 上的 `akit_config::Format`（`Json`、`Yaml`）、`Format::of(path)`、`from_yaml`；`from_files` 用它读取
  `.yaml` 和 `.yml` 文件，因此 YAML 和 JSON 文件可以合并成一份配置。`akit run -c` 和 `akit validate -c` 接受 YAML，
  用法说明也已更新。
- 兼容性门禁的钩子通过 `from_files` 读取 YAML 夹具。`make compat-bless`：80 个夹具，0 个有效，4 个仅因暂不支持而被
  拒绝（被拒字段：2），75 个格式暂不支持（.conf，I23），1 个跳过，0 个失败。两个 YAML 模板从 "format not supported
  yet" 变为与对应 JSON 版完全相同的结果（`upstreams`，`conf.d` 还有 `routes[].handler<proxy>`）。
- 测试：`tests/yaml_config.rs`（模板的单文件和 `conf.d` 读入的模型和问题与 JSON 版相同；同一份配置的两种格式；
  位置；YAML 与 JSON 文件合并），以及模块自己的测试：移植的 fit `adapter/yaml` 测试用例、标量、折行、结构、位置、Go
  的浮点格式、长行的线性时间、文档的每个前缀和每个单字节修改、每个模板的树与 JSON 版的树对照（也包括把注释掉的字段
  写出后的版本；10 个模板中有 6 个写出后两种格式都是有效配置），以及 fit 对 493 个文档的判定
  （`crates/akit-config/testdata/yaml-fit.txt`）。
- `tools/yaml-diff/run.sh`：与 fit 的差分检查（用 Go 从 fit 检出构建 fit 的适配器；语料包括手写用例、fit 的 YAML 文件
  和文档、写出后的模板以及随机编辑）。它发现了约 25 类差异，逐一修复；最后一轮 150,000 个用例中有 2 个读法不同
  （libyaml 的怪异之处，akit 拒绝，见决定 005）。
- 模糊测试目标 `config_yaml`（`fuzz/`），以两份小配置为种子；运行 10 分钟没有发现问题。
- 与计划的不同：读取器覆盖 fit 的库能读的全部内容，而不只是一个子集，只拒绝 `%TAG` 和具名标签句柄（报暂不支持）、
  超过 128 层的嵌套（fit 为 256；JSON 读取器的上限）以及上述 2 个边角；显式空文档（`---`）对 akit 是 `{}`，在 fit 的 JSON 中是 `null`，对两者都是空配置。计划中的
  "涉及"还多了 `tests/compat`（钩子）和 `tools/yaml-diff`。
- 已检查：macOS 上的 `make check`。

### 2026-11-24：I23，.conf 读取器

- akit-config 读取 fit 自己的格式 `.conf`（`crates/akit-config/src/conf`）：`akit_config::from_conf`；
  `from_files` 读取以 `.conf` 结尾的文件（直接给出的或在目录中找到的），并像 fit 一样与 JSON 文件合并。
  词法和语法分析与 fit 的（`adapter/fitconf`）相同：注释、带转义的双引号、反引号、`;`、`{}`、`{$NAME}`
  和 `{$NAME:default}`，上限为 2^20 个词、64 层块。然后是 `def` 和 `use`、展开为路由的 `site`（及
  `site *`）、顶层列表的单数写法（名字作参数）、简写（监听器的地址和协议、上游的地址、
  `tls <cert> <key>`、带单位的字节数）、模块写作名字加参数块，以及 fit 在转换时报告的每个错误，
  消息相同，位于相同的行和列。
- 做法：读取器像 fit 把 `.conf` 转成 JSON 那样，把 `.conf` 文件转成与 JSON 读取器解析结果相同的树，
  每个节点保留它在 `.conf` 文本中的偏移，由同一个解码器读取两者。另一种选择是自己的读取器直接驱动
  解码器；转换的做法只保留一个解码器，于是后续迭代（I09、I14）支持的字段无需改动这里就能从 `.conf`
  读取，解码器报告的问题也指向 `.conf` 文件。与 fit 一样，转换按配置的字段定型：字段类型取自 fit 的
  字段清单 `tests/compat/fit-fields.txt`（固定提交下 fit 的 JSON Schema），编译进 akit-config，因此 fit
  的每个字段——无论 akit 是否已读取——都按 fit 的方式转换，再被读取或按名字拒绝。没有字段接受的指令
  按原样转换，并像 fit 一样作为未知字段拒绝。
- 暂不支持，并在其位置上以暂不支持拒绝：从 `.conf` 文件读取其他文件的 `include` 和 `import`（没有
  夹具用到；改用 `-c` 给出文件）。fit 已改名字段的旧名字不读取，`.conf` 与 JSON 相同。
- 兼容性门禁经由 `from_files` 读取 `.conf` 文件和 `conf.d` 目录。校验为示例打开什么已经确定：只打开
  配置自身的文件，因为每个提到文件或密钥的字段都以暂不支持拒绝（[兼容性](compatibility.zh-CN.md#校验会打开什么)）。
  akit 读取的字段中被拒绝的值，现在连同该值一起报告（`listeners[].protocols: the "h3" protocol`、
  `listeners: a config with no listeners, ... (auto_https)`），不算作整个字段被拒绝：门禁"已支持字段不得
  被拒绝"的检查此前会把 akit 读取的 `listeners[].protocols` 和 `listeners` 标出来。
- 门禁计数（fit `7daee573`、fit-examples `9659d7e4`）：80 个夹具；2 个有效（`hello`、`virtual-hosts`）；
  75 个只以暂不支持拒绝（拒绝的字段或值共 52 个）；2 个格式尚不支持（YAML 模板及其 `conf.d`，I24）；
  1 个跳过（`https`）；0 个失败。此前：0 个有效、2 个拒绝、77 个无法读取。`templates/conf/*` 拒绝的内容
  与其 JSON 孪生文件完全相同。
- 测试：`conf/tests.rs`（移植 fit 的转换、简写和错误表：21 个配置、60 个错误及其行列；上限；解码器问题在
  `.conf` 文件中的位置；同一配置的 `.conf` 与 JSON 读成相同的 `Config`；每个 `.conf` 模板转换出与其 JSON
  孪生文件相同的树；每个示例都能转换），`conf/convert.rs`（数字、字节数、清单中的类型），`files.rs`（从
  目录合并 `.conf` 与 JSON 文件，问题报告在其位置），`akit/tests/cli.rs`（对 `.conf` 文件执行 `akit validate`），门禁（值的拒绝）。模糊测试目标 `config_conf`，以示例
  为语料，在 CI 中与 `config_json` 并列各跑一分钟。
- 已检查：macOS 上的 `make check`；本地对 `config_conf` 模糊测试 60 秒。

### 2026-11-24：I08，基准测试框架与基线

- akit 用 fit 的框架测量，而不是它的副本（`bench/`，[bench/README.zh-CN.md](../bench/README.zh-CN.md)）：
  `bench/fitbench/akit.go` 复制到 fit 的 `bench/cmd/fitbench` 后把 akit 加入 fitbench 的服务器列表（fit 的服务器
  列表是切片，所以不改 fit 的任何文件）；`bench/configs/akit/fit.json` 提供 fit 基准路由中 akit 已能提供的部分
  （8080 上 HTTP/1.1、8081 上 h2c 的固定响应）。`bench/FIT_REV` 把 fit 的框架锁定在兼容性夹具所用的提交。
- `bench/run.sh [REF]` 在基准测试主机上部署一次 fit 的源码树，加入 akit 的两个文件，在主机上用 Go 构建 fitbench、
  benchreport、idleconn 和 fit，用 cargo 构建 REF 处的 akit 并放进独立镜像（`akit-bench:<id>`，基于
  `ubuntu:24.04`），然后在主机的 `quiet.sh`（独占锁）下以 nginx、Caddy、Traefik、fit 和 akit 启动 fitbench；
  `-report ID` 把结果和 fit 的 benchreport 表格取回到 `bench/results/ID`。`bench/ab.sh A B` 在同一次运行中交替各轮
  对比两个 akit 构建，用 fit 的 `abreport.py` 汇总。
- akit 的运行是 HTTP/1.1 和 h2c 上的 fixed 场景，以及空闲 HTTP/1.1 连接；它的其他场景在实现它们的迭代之前记为跳过，
  每个迭代把它们加入 overlay 和配置。akit 的配置与 fit 的不同之处（8081 也接受 HTTP/1.1、还没有 backlog 和连接数
  限制设置、akio 按监听器的 I/O 线程）见 bench README。
- 基线 `bench/results/baseline-2026-11-24`（结果、fit 的 benchreport 表格、参数和 akit 提交）：akit `7c835ad`、fit
  `7daee573`、nginx 1.31.4、Caddy 2.10.2 和 Traefik 3.5.0，在基准测试主机上（AMD Ryzen 7 7800X3D，8 核 16 线程，
  Linux 7.0），fitbench 的默认设置（预热 5s 后测 30s、wrk 256 个连接、h2load 64 × 32），HTTP/1.1 和 h2c 上的 fixed、
  static 和 proxy 场景，以及空闲 HTTP/1.1 连接；没有错误。
- akit 所处的位置（规则 28），就它目前能提供的而言：HTTP/1.1 上的 fixed 第一，每秒 825,563 个请求，p99 0.32 ms，
  2 MiB，nginx 是它的 99%，fit 92%，Caddy 41%；h2c 上的 fixed 第一，每秒 6,029,715 个，3 MiB，fit 74%，nginx 10%，
  Caddy 3%；每个空闲 HTTP/1.1 连接的内存第二，1,068 字节，次于 nginx 的 548（fit 1,522）。akit 没有运行 static 和
  proxy（I12、I15）；Traefik 只提供代理。
- 已验证：macOS 上的 `make check`；`bench/run.sh` 在基准测试主机上端到端运行（第一次尝试因主机磁盘写满在测量前失败）。
  `bench/ab.sh` 还没有完成过一次对比：按其间新增的规则 30，功能任务不运行它。

### 2026-11-23：I06，命令行、信号与优雅关闭

- `akit run`、`akit validate`、`akit version` 和 `akit help`，带 fit 的参数，按 Go 的 `flag` 包的方式读取
  （`-c x`、`-c=x`、`--c`、布尔参数、`--`），退出码与 fit 一致：成功为 0；没成功为 1（配置错误、关闭超时）；用法
  错误为 2（参数错误或未定义、多余的参数、没有命令）。`validate` 打印 `<files>: valid`；`version` 按 fit 的格式
  逐行打印 akit 的版本、akio 的提交和平台。
- `-c` 可重复，可以是文件或目录：`akit_config::from_files` 按 fit 的方式查找文件（目录中的 `.json`、`.conf`、
  `.yaml` 和 `.yml` 文件，包括子目录，按字典序，跳过点开头的文件，同一文件只读一次，最多 10,000 个文件、64 MiB），
  并按 fit 的方式合并：`listeners` 和 `routes` 按文件顺序拼接，`schema_version` 各文件可分别给出但必须一致，跨元素
  的检查在合并后进行。每个问题都指明所在文件、行和列，以及它在该文件中的路径。`.conf` 和 YAML 文件按暂不支持拒绝
  （I23、I24）；其他文件与 fit 一样按 JSON 读取。没有 `-c` 时为 `fit.json`。
- 信号经 akio-sys 的 `signal_pipe` 接收（其 `unsafe` 留在 akio 中）：SIGINT 和 SIGTERM 时打印
  `akit: shutting down`，停止监听，关闭空闲连接，最多等待 `-shutdown-timeout`（默认 30s，Go 的时长格式）让进行中
  的请求完成，然后打印 `akit: stopped` 并以 0 退出，或说明关闭超时并以 1 退出。第二个信号立即退出。SIGHUP 时说明
  暂不支持重载（I22），而不是让进程退出。排空由 `akit_listener::shutdown` 完成；`akit_listener::Options` 带后端
  和超时。
- fit 的其他 `run` 参数（`-watch`、`-watch-interval`、`-watch-certs`、`-admin`、`-timeout`）按暂不支持拒绝，
  以 2 退出。
- 兼容性门禁现在经合并读取由 JSON 文件组成的目录：`templates/json/conf.d` 只因暂不支持而被拒绝（`upstreams`、
  `proxy` handler），不再是 akit 读不了的格式；2 个夹具只因暂不支持被拒绝，77 个格式暂不支持。
- 测试：`akit-config/tests/files.rs`（6 个：拼接、问题所在文件、schema 版本、目录、格式、缺失与空目录）；参数与
  Go 时长；`akit-listener`（新增 2 个：关闭期间进行中的请求能完成，空闲连接立即关闭；超过超时的请求被切断并报告）；
  `akit/tests/cli.rs`（4 个：跨文件和目录的 validate、用法错误以 2 退出、从两个文件运行并由 SIGTERM 停止且有空闲
  keep-alive 连接、从目录运行并由 SIGINT 停止）。
- 已验证：macOS 上的 `make check`；基准测试主机上 Linux epoll 和 io_uring 的整个工作区测试（2026-11-24，与 I04、I05
  的一起）：全部通过。
- 与计划的不同：进行中的请求用库层的服务器和一个稍后应答的 handler 测试，因为唯一的 handler `respond` 立即应答；
  二进制的测试覆盖信号、输出和退出码。关闭是否超时由 akio 排空所用的时间判断（akio 不报告它关闭了多少连接）。第二
  个信号以 1 退出，而 Go 的默认处理会让进程因该信号终止。

### 2026-11-23：I05，`akit run`：监听器与 `respond`

- `akit run -c <file>` 读取配置（JSON；没有 `-c` 时为 `fit.json`，与 fit 一致），编译并提供服务，按 fit 的格式
  打印每个监听器的地址（`akit: listener "web" on 127.0.0.1:8080 (http/1.1, h2c)`）。配置错误按 fit 的
  `file:line:col: path: message` 逐行打印并以 1 退出；参数错误以 2 退出。
- `akit-handler`：`respond` 模块（`http.handlers.respond`），响应在初始化时构建一次（状态码、头；有 body 且没有
  设置时 content type 为 `text/plain; charset=utf-8`）；akit-config 已像 fit 的 `Provision` 一样检查其参数。
- `akit-listener`：`engine(cell, backend)` 为每个配置监听器建一个 akio TCP 监听器，各自服务一个 `Dispatcher`：
  它经 cell 的当前图为请求选路由并调用路由的 handler，否则返回 fit 的 `404 page not found`。`":8080"` 监听
  `[::]` 双栈，与 Go 的 `net.Listen` 一致。只有 `http/1.1` 时关闭 akio 的 HTTP/2；有 `h2c` 时，先验知识和
  `Upgrade: h2c` 请求都能处理。
- `akit-router`：路由器 v0，fit 的 host 规则（精确、`*.`、`**.`，忽略大小写、端口和末尾的点）和 path 规则（精确、
  `*` 前缀）及其排序（优先级、host、path、书写顺序），逐条扫描路由；图在编译时为每个监听器建一个，没有
  `listeners` 的路由在所有监听器上服务。
- 修复：I07b 发现的 akit-config 监听器默认协议现在与 fit 一致，没有 TLS 时只有 `http/1.1`（`DEFAULT_PROTOCOLS`），
  并补了测试。
- 测试：路由器（6 个）、respond（3 个）、监听地址；`akit-listener/tests/listeners.rs`（5 个）：HTTP/1.1、h2c 先验
  知识与升级、404、host 具体程度、按监听器的路由、默认拒绝 HTTP/2、拒绝只有 h2c；`akit/tests/hello.rs`（3 个）：
  经二进制，fit 的 `hello` 示例（写成 JSON）通过 HTTP/1.1 和 h2c 应答 `/hello` 及其 `not-found` 路由；错误配置
  打印错误并以 1 退出，错误参数以 2 退出。
- 已验证：macOS 上的 `make check`；基准测试主机上 Linux epoll 和 io_uring 的整个工作区测试（2026-11-24，在 I06 的
  运行中）：全部通过。
- 与计划的不同：路由器 v0 不在 I05 的计划中；没有它，示例的 `not-found` 路由（优先级 -1、无匹配条件）会遮住
  `/hello`。I11 保留其规则并让查找不分配内存，现在还要像 fit 的监听器那样规范化路径（此前没有计划）。示例在测试中
  写成 JSON：`.conf` 属于 I23。有 `h2c` 而没有 `http/1.1` 的监听器按暂不支持拒绝：fit 此时只提供 HTTP/2，而 akio
  的服务端总是也接受 HTTP/1。请求 body 先缓冲（akio 的默认值，1 MiB），直到有 handler 以流的方式读取（I14）。

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
- 基准 `benches/graph_read.rs`：经 `GraphCell::with` 的读取，对比 `Arc` 克隆和加锁再克隆，在基准测试主机（16 核）上每线程
  读 2000 万次：`GraphCell::with` 1 线程每次 4.06 ns、16 线程 8.78 ns；`Arc` 克隆再释放 2.77 ns 和 324.51 ns；加锁再
  克隆 6.57 ns 和 1,167.62 ns。16 个线程争用时，共享引用计数的开销是 cell 读取的 37 倍。
- 已验证：macOS 上的 `make check`；基准测试主机上 Linux epoll 和 io_uring 的整个工作区测试（2026-11-24，在 I06 的
  运行中）：全部通过；读取基准也在那里于 `quiet.sh` 下运行（见上）。
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
