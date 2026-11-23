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
| 可用功能 | `akit run` 和 `akit validate`，带 fit 的参数和退出码（`-c` 可重复，文件和目录合并），SIGINT 和 SIGTERM 时优雅关闭；`akit run` 按 fit 的 host 与 path 规则（在规范路径上查找、不分配内存；fit 拒绝的路径回答 400）提供 HTTP/1.1 和 h2c 监听器上的 `respond` 路由，没有匹配时返回 404；`akit version` 和 `akit help`；工作区能构建并通过 `make check`；`akit_config::from_json` 能读取 fit 的监听器、路由和 `respond` handler，fit 配置的其余部分按名字报为暂不支持；模块注册表和数据面 trait（`akit-runtime`）；`compile` 把配置编译成 `Graph`，`GraphCell` 负责发布、读取和退役图；兼容性门禁在 `make check` 和 CI 中把 fit 的每个夹具交给 akit-config |
| 下一步 | I08 的基线（来自基准测试主机），M0 的最后一项；然后是 I09（TLS）。 |
| akio 需求 | A1–A8 现在全部加进 akio，各在 akio 的独立分支上进行（[akio API 需求](akio-api-requests.zh-CN.md)） |

## 日志

### 2026-11-24：I11，路由 v1：host 与 path

- `akit_router::Router` 改为查找路由而不是逐条扫描，做法同 fit 的 `router` 包（`router/router.go`，不含
  matcher，matcher 属于 I39）：按 host 模式建表（精确、`*.` 按从点开始的后缀、`**.` 同样、以及没有 host 的
  路由），每个 host 模式下再按精确路径和前缀建表，前缀长度从长到短排好。一次查找依次试请求的精确 host、
  它的 `*.` 后缀和最多四个 `**.` 后缀（从长到短），最后是没有 host 的路由；在每个 host 模式下先试精确路径，
  再按前缀长度逐个试，一旦剩下的前缀都不可能胜出就停止。顺序同 fit：优先级，然后 host 精确度，然后 path
  精确度，最后是先写的路由。查找不分配内存：含大写字母的 host 在栈上的缓冲区里转成小写（超过 255 字节的
  host 只能匹配没有 host 的路由，同 fit）。
- fit 没有的部分：这些表用 akit 自己的 FxHash 式哈希（表由配置建立、之后只读，请求的 host 无法让探测超过
  配置放进去的条目）；不超过 8 个键的表直接扫描而不哈希；所有路由都没有 host 时不看 host。由于还没有
  matcher，每个 host 与 path 模式只保留最优的路由。
- `akit_router::canonicalize` 把请求路径规范成 fit 的形式（`listener/path.go`）：按 RFC 3986 去掉点段，
  `%2E` 视为 `.`，去掉空段；`..` 越过根目录的路径，以及解码后出现由编码形式（`%2F`、`%5C`、`\`）构成的空段
  或点段的路径，一律拒绝。没有需要解码或删除内容的路径只扫描一遍。`akit_listener::Dispatcher` 在路由前
  规范每个请求的路径，被拒绝的路径回答 `400 bad request path\n`，同 fit；路由匹配百分号解码后的路径，
  就像 fit 匹配 Go 的 `URL.Path`，因此 `Router::find` 和 `Graph::route` 以字节接收路径
  （`impl AsRef<[u8]>`：解码后的路径不一定是 UTF-8）。
- 测试：移植 fit 的路由表测试（`akit-router` 的 `src/tests.rs`：`TestHostOnly`、`TestHost`、
  `TestDeepWildcard`、`TestPath`、`TestNoRoutes`、九张 `TestOrder` 表、`TestPathTwice`、
  `TestHostOnlyUnclosed`），保留 v0 的测试，并新增超长 host、多种前缀长度、非 UTF-8 路径和 `**.` 后缀上限的
  测试；移植 fit 的 `TestCanonicalize` 表，外加错误的转义和非 UTF-8 转义；`tests/no_alloc.rs` 对应 fit 的
  `TestMatchAllocs` 和 `TestCanonicalizeAllocs`：独立的二进制、不用测试框架，以 `stats_alloc` 为全局分配器
  计数（dev-dependency：akit 没有 `unsafe` 来自己写一个，规则 14），检查七种查找（命中与未命中、大写、端口、
  `**.`、IPv6）和三个已规范路径的规范化各调用 100 次都没有分配，并确认计数有效（带点段的路径确实分配）；
  `akit-listener` 的 `tests/listeners.rs`：用 `respond` 路由移植 fit 的 `TestPathConfusionRoutes`，覆盖 HTTP/1.1
  和 h2c（为绕开受保护路由而拼写的路径仍然命中它，或得到 400），以及 handler 看到的规范路径。
- 微基准 `crates/akit-router/benches/lookup.rs`（`harness = false`）：v0（I05 的扫描，保留在
  `benches/v0.rs`）对比 v1，单位是每次查找的纳秒数，数据为 fit 的 `BenchmarkMatch` 表（每条路由一个自己的
  host，带一个前缀和一个精确路径，外加一个 `*.` 兜底路由）以及基准场景使用的单路由配置。在基准测试主机上、
  `quiet.sh` 之下：

  | 每次查找 ns | v0 | v1 | v0/v1 |
  | --- | ---: | ---: | ---: |
  | 1 条路由， `/` | 12.5 | 7.7 | 1.6x |
  | 10 条路由 + 兜底，中间的路由 | 69.0 | 36.7 | 1.9x |
  | 10 条路由 + 兜底，兜底路由 | 48.6 | 24.2 | 2.0x |
  | 10 条路由 + 兜底，无匹配 | 33.2 | 16.3 | 2.0x |
  | 100 条路由 + 兜底，中间的路由 | 383.3 | 37.2 | 10.3x |
  | 100 条路由 + 兜底，兜底路由 | 315.4 | 24.1 | 13.1x |
  | 100 条路由 + 兜底，无匹配 | 227.7 | 16.1 | 14.1x |
  | 1000 条路由 + 兜底，中间的路由 | 3523.8 | 30.9 | 114x |
  | 1000 条路由 + 兜底，兜底路由 | 2292.4 | 23.9 | 96x |
  | 1000 条路由 + 兜底，无匹配 | 2207.1 | 15.8 | 140x |

  v1 在 10 条路由和 1000 条路由时开销差不多；v0 随路由数增长。在小表改为扫描而不是哈希之前，v1 在单路由配置上
  比 v0 慢（15.7 对 12.6 ns）；改为扫描后是 7.7。
- A/B：`bench/ab.sh main work` 已以 `ab-20261124-010304` 启动，但失败了：每一轮构建 fitbench 镜像时
  `apt-get` 都失败，因为基准测试主机的磁盘已满（`/` 100%，剩余 0 字节，主要是其他会话的构建目录）。主机有
  空间后需要重跑；上面的微基准已经跑完。
- 已检查：macOS 上的 `make check`；基准测试主机上 Linux（epoll）的
  `cargo test -p akit-router -p akit-runtime -p akit-listener`，以及 `AKIO_TEST_BACKEND=uring cargo test -p akit-listener`。
- 与计划和 fit 的不同：没有 matcher（I39）。错误的转义（`%zz`）回答 `400 bad request path`；fit 的 Go 服务器
  在 fit 看到之前就用它自己的 400 拒绝。akio 的请求目标无法改写，所以规范路径通过上下文局部值
  `akit_listener::CANONICAL_PATH` 交给 handler，只在它与发来的路径不同时设置；I12 和 I15 读取它（或者 akio
  增加改写目标的方法）。字面的 `\` 在规范路径中保持原样，而 Go 会写成 `%5C`。绝对形式的请求
  （`GET http://host/path`）仍按 akio 的 `path()`（整个 URI）路由，与 I11 之前相同。

### 2026-11-24：I08，基准测试框架（基线待完成）

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
- 尚未完成：基线。第一次五个服务器的运行（akit 在 `main`，`1e45124`）已部署到基准测试主机，正在等待被其他会话的
  基准测试占用的锁；在结果入库到 `bench/results/` 之前，I08 保持 `doing`。
- 已验证：macOS 上的 `make check`（两种语言的 bench README）；脚本能上传 fit 的源码树并在主机上构建 fit 的框架。两个
  脚本都还没有完成过一次运行。

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
- 已验证：macOS 上的 `make check`。尚未验证：Linux（epoll 和 io_uring）上的测试，与 I04、I05 的一起在基准测试主机上排队；
  结果由后续提交记录。
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
- 已验证：macOS 上的 `make check`。尚未验证：Linux（epoll 和 io_uring）上的测试，正在基准测试主机上运行；结果由后续
  提交记录。
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
