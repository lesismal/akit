# akit 开发计划

[English](dev-plan.md) | [简体中文](dev-plan.zh-CN.md)

本计划把 [路线图](roadmap.zh-CN.md) 拆成一系列小迭代。每个迭代是一份能在一次工作会话、一个 commit 或 PR
内完成的工作：小到它涉及的代码、文档和测试能放进一次上下文。

- [如何进行一次迭代](#如何进行一次迭代)
- [工作区结构](#工作区结构)
- [索引](#索引)
- [基准与优化状态](#基准与优化状态)
- [M0 基础](#m0-基础)
- [M1 代理核心](#m1-代理核心)
- [M2 动态配置与 TLS](#m2-动态配置与-tls)
- [M3 网关能力](#m3-网关能力)
- [M4 平台集成](#m4-平台集成)
- [M5 缓存与扩展](#m5-缓存与扩展)
- [1.0](#10)
- [1.0 之后](#10-之后)

## 如何进行一次迭代

**选一个。** [开发进度](progress.zh-CN.md) 说明了当前进展和下一个迭代。否则在 [索引](#索引) 中取第一个状态为
`todo`、且依赖全部为 `done` 的迭代。彼此没有依赖的迭代可以在不同分支上并行进行。

**只读需要的内容。** 一次会话从以下内容开始：

1. [开发进度](progress.zh-CN.md)、本文件的索引和该迭代自己的小节；
2. [设计原则](roadmap.zh-CN.md#设计原则) 和 [开发规则](rules.zh-CN.md)；
3. **涉及** 中列出的 crate，**参考** 中列出的 akio crate 和文档，以及 **fit** 中列出的 fit 包：fit 的代码、
   测试和文档就是要实现的内容的规格。

**移植，不重新发明。** 迭代会遇到的每个问题，fit 都已经遇到过。它的决定说明（`fit/docs/decisions`）、测试和
配置参考说明了 akit 应该做 *什么*；akit 改变的是 *怎么做*（akio 的回调、Rust 的所有权），保留 *做什么*。akit
必须与 fit 不同时，由该迭代在决定说明中记录原因。

**保持小。** 目标是改动不超过约 1500 行（含测试）。如果迭代比预想的大，就停在一个可用、有测试的位置，把剩余部分
在两个计划文件中拆成新的迭代（`I17b` 等），然后完成当前迭代。

**收尾到位。** 迭代完成的条件是：

- **完成标准** 中的各项都满足；
- `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings` 以及它涉及的 crate 的测试通过，在 macOS
  上以及 Linux 的 epoll 和 io_uring 上都通过；
- 请求路径上没有任何操作阻塞 I/O 线程，并且 [开发规则](rules.zh-CN.md) 的每一条都满足；
- 在同一个 commit 中把索引里的状态改为 `done`，两个计划文件都要改；
- 两个 progress 文件都记录做了什么、如何验证、与计划的差异，以及下一个迭代；
- 迭代中做出的决定写成 `docs/decisions/NNN-title.md` 及其中文版。

**会话提示词。** 可以用下面的话开始一次会话：

> 实现 docs/dev-plan.md 中的迭代 IXX。先读 docs/progress.md、索引和该迭代的小节、docs/rules.md，以及它指向的
> fit 代码和测试。遵守开发规则；完成后更新两个计划文件和两个 progress 文件。

**akio 不足时**，迭代先做不依赖该缺口的部分，把缺口写进 [akio API 需求](akio-api-requests.zh-CN.md)，临时措施
记录在 progress 文件中。

## 工作区结构

I01 建立、后续迭代逐步填充的结构。名字可以调整，但每个迭代应该尽量少地涉及这些 crate。表中同时给出 fit 中承担
相同职责的包，供参考。

| Crate | 职责 | fit |
| --- | --- | --- |
| `akit` | 可执行文件：`run`、`validate`、`adapt`、`fmt`、`store`、`build` | `cmd/fit`、`cli` |
| `akit-config` | 配置模型，JSON、YAML 和 .conf 的严格读取器，校验 | `config`、`schema` |
| `akit-runtime` | 模块注册表、编译、运行时图及其替换 | `runtime`、`module` |
| `akit-listener` | 监听器，TLS 和 QUIC 入口，mux | `listener` |
| `akit-router` | 匹配器、优先级、表达式语言 | `router` |
| `akit-middleware` | 内置中间件 | `middleware` |
| `akit-handler` | `respond`、`static`、`redirect`、FastCGI 等 handler | `handler` |
| `akit-proxy` | HTTP、WebSocket 和 gRPC 反向代理 | `handler/proxy` |
| `akit-upstream` | 上游连接池、负载均衡、健康检查 | `upstream` |
| `akit-l4` | TCP 和 UDP 代理、SNI 分流、PROXY protocol | `l4` |
| `akit-tls` | 证书：文件、ACME、存储、OCSP、按需签发 | `tlsmgr` |
| `akit-admin` | Admin API、文件 provider | `admin` |
| `akit-provider` | Docker、Kubernetes、Consul、etcd | `provider` |
| `akit-observe` | 日志、指标、追踪 | `observe`、`logs` |
| `akit-cache` | 响应缓存 | `handler/cache` |
| `akit-plugin` | WASM 插件 | `plugin` |
| `tests/e2e` | 可执行文件的端到端测试 | `test/e2e` |
| `tests/compat` | fit 配置兼容性门禁：固定夹具、字段清单 | `templates`、fit-examples |
| `bench` | 基准框架、场景和报告 | `bench` |

## 索引

状态：`todo`、`doing` 或 `done`。下一个可用的迭代编号是 I89（I84 属于日志路线，未使用；I85–I88 属于访问控制列表路线）。

| ID | 迭代 | 依赖 | 状态 |
| --- | --- | --- | --- |
| **M0** | | | |
| I01 | 工作区骨架与 CI | — | done |
| I02 | 配置模型 v0 与严格的 JSON 读取器 | I01 | done |
| I03 | 核心 trait 与模块注册表 | I01 | done |
| I04 | 运行时图及其替换 | I02, I03 | done |
| I05 | `akit run`：监听器与 `respond` | I04 | done |
| I06 | CLI、信号与优雅关闭 | I05 | done |
| I07a | 兼容性门禁 I：固定夹具、字段清单、测试框架 | I01 | done |
| I07b | 兼容性门禁接入 validate | I02, I07a | done |
| I08 | 基准框架与基线 | I05 | done |
| **M1** | | | |
| I09 | 从文件加载证书的 TLS 入口 | I05 | done |
| I10 | HTTP/3 入口 | I09 | done |
| I11 | 路由 v1：host 与 path | I04 | done |
| I12 | 静态文件 I | I11 | done |
| I13 | 静态文件 II | I12 | done |
| I14 | 基于 HTTP/1 客户端的上游连接池 | I04 | done |
| I15 | 反向代理 I：请求与头处理 | I11, I14 | done |
| I16 | 反向代理 II：流式 body 与联动背压 | I15 | done |
| I17 | 反向代理 III：超时、取消、trailer | I16 | done |
| I17b | 反向代理 III b：响应头与空闲超时、响应头之前的取消、写停滞 | I17、akio A36–A38 | done |
| I18 | HTTP/2 与 h2c 上游 | I17 | done |
| I18b | HTTP/2 上游 II：由 akio 重置、计时和控制节奏的流 | I18、akio A51–A53、A66 | done |
| I19 | 负载均衡 v1 | I14 | done |
| I20 | 被动健康检查 | I19 | done |
| I21 | 访问日志 | I15 | done |
| I22 | SIGHUP 重载 | I06 | done |
| I23 | .conf 读取器 | I02 | done |
| I24 | YAML 读取器 | I02 | done |
| I25 | M1 一致性测试与报告 | I09–I24 | done |
| **M2** | | | |
| I26 | Admin API I：加载、读取、版本 | I22 | done |
| I27 | Admin API II：局部修改、回滚、认证 | I26 | done |
| I28 | 运行时的监听差量 | I26, A1 | done |
| I29 | 文件 provider | I26 | done |
| I30 | 证书管理与存储 | I09 | done |
| I31 | ACME I：账户、订单、HTTP-01 | I30 | done |
| I32 | ACME II：TLS-ALPN-01、续期、OCSP | I31 | done |
| I33 | 默认 HTTPS | I32 | done |
| I34 | 按需 TLS | I33, A2 | done |
| I35 | 主动健康检查 | I20 | done |
| I36 | 负载均衡 v2 | I19 | done |
| I37 | Prometheus 指标 | I15 | done |
| I38 | M2 退出：频繁替换测试与 staging 证书 | I26–I37 | done |
| **M3** | | | |
| I39 | 匹配器 v2 与表达式 | I11 | done |
| I40 | 重写、跳转与头处理 | I39 | done |
| I41 | 限流、并发与大小限制 | I39 | done |
| I42 | Basic 认证与 mTLS | I39 | done |
| I43 | ForwardAuth | I42 | done |
| I44 | JWT 与 JWKS | I42 | done |
| I45 | 重试、熔断、按路由超时 | I17 | done |
| I46 | 加权服务、镜像、故障转移 | I45 | done |
| I47 | 压缩 | I16 | done |
| I48 | fit 的其余中间件 | I40 | done |
| I49 | WebSocket 代理 | I17 | done |
| I50 | gRPC 代理 | I18 | done |
| I51 | TCP 与 UDP 代理 | I05 | done |
| I51b | eBPF sockmap 中转 | I51 | done |
| I52 | SNI 分流与 PROXY protocol | I51, A4, A5 | done |
| I53 | 一个端口多种协议 | I52 | done |
| I54 | 上游 TLS | I18 | done |
| I55 | SOCKS 与 CONNECT | I53 | done |
| I56 | M3 退出 | I39–I55 | done |
| **M4** | | | |
| I57 | Provider 框架与生效配置 | I29 | done |
| I58 | Docker provider | I57 | done |
| I59 | Kubernetes Ingress | I57 | done |
| I60 | Kubernetes Gateway API 与一致性测试 | I59 | done |
| I61 | Consul 与 etcd | I57 | done |
| I62 | 共享证书存储 | I30, I61 | done |
| I63 | 仪表盘 | I37 | done |
| I64 | OpenTelemetry | I37 | done |
| I65 | 打包 | I06 | done |
| I66 | M4 退出 | I57–I65 | done |
| **M5** | | | |
| I67 | 缓存 I：内存层与 RFC 9111 | I17 | done |
| I68 | 缓存 II：磁盘层、缓存锁、stale | I67 | done |
| I69 | 缓存 III：清除与标签 | I68, I27 | done |
| I70 | `akit build` | I03 | done |
| I71 | WASM I：运行时与 ABI | I03 | done |
| I72 | WASM II：Proxy-Wasm 子集 | I71 | done |
| I73 | `akit adapt` 与 `akit fmt` | I23, I24 | done |
| I74 | nginx 与 Caddyfile 转换器 | I73 | done |
| I75 | FastCGI | I16 | done |
| I76 | M5 退出 | I67–I75 | done |
| **1.0** | | | |
| I77 | Fuzz | I56 | done |
| I78 | 配置参考与指南 | I76 | done |
| I79 | v1 冻结与兼容性 | I78 | done |
| I79b | 门禁仍拒绝的网关示例：`limits.max_pending_bytes`、只有 h2c、按接受的连接计数的 `max_conns` | I79、akio A214、A168、A215 | done |
| I80 | 为安全审查做好准备 | I77, I79 | done |
| I80b | 安全加固：威胁模型的局限与审查的发现 | I80、审查 | todo |
| **1.0 之后** | | | |
| I81 | 日志文件：按大小和时间切割、保留、压缩；`access_log.rotate` | I21 | done |
| I82 | akit 自身的日志：fit 的 `log`，其日志器、格式、文件与时区 | I81 | done |
| I83 | 重载时的日志文件，以及 Admin API 的 `/logs` | I82、I27 | done |

## 基准与优化状态

[规则 24](rules.zh-CN.md) 要求做基准的每项功能（热路径：同轮 A/B；热路径之外的数据面：crate 的微基准）在这里都有一行；
控制面（配置读取、CLI、Admin API、证书、provider）没有。**已测**指 akit 在这项上的数据已测出并入库；**已优化**指做过
针对其开销的工作并有测量，所以只出现在基线里算“已测”，不算“已优化”。两列不都是“是”的行，都是留给压测任务的待办项
（[规则 30](rules.zh-CN.md)）。功能任务把它所在迭代的行加为“否”，并写明应当测什么；压测任务用日期、commit 和结果
更新这一行。

| 功能 | 迭代 | 已测 | 已优化 | 依据或剩余工作 |
| --- | --- | --- | --- | --- |
| 每个请求读取运行时图 | I04 | 是（2026-11-24，`c9d490a`，`crates/akit-runtime/benches/graph_read.rs`，数据见 [进度](progress.zh-CN.md)） | 部分 | 每线程缓存的读取方式是与 `Arc` 克隆和互斥锁对比测量后选定的（1 线程 4.06 ns，16 线程 8.78 ns）；此后没有剖析或调优。 |
| 监听器与 `respond`，HTTP/1.1 和 h2c；空闲连接 | I05 | 是（2026-11-25，`9d9321e`，`bench/results/m1-2026-11-25-akio-fixes`；首次 2026-11-24，`bench/results/baseline-2026-11-24`） | 否 | M1 报告（I25）：fixed 在 HTTP/1.1、HTTPS/1.1、h2c 和 h2 上都排第一；空闲 HTTP/1.1 在 1 万时 1,144 字节、10 万时 1,132（fit 1,497 和 1,441，nginx 556 和 530）。剩余：两者的剖析；h2c 上的 fixed 比 M0 基线低 7–8%（`7c835ad` 对 `9d9321e` 的同轮 A/B）；空闲 HTTP/1.1 对照 nginx；h2c 空闲连接。 |
| TLS 入口（TLS 上的 HTTP/1.1 和 h2） | I09 | 是（2026-11-25，`9d9321e` 带 akio A172，`bench/results/m1-2026-11-25-akio-fixes`） | 否 | M1 报告：fixed 和 static 在 HTTPS/1.1 和 h2 上排第一；空闲 TLS 在 1 万时 12,942 字节、10 万时 12,872（fit 9,879 和 8,593，nginx 29,924 和 29,583）。按锁定的 akio（`dad77a7a`），harness 的 wrk 每次握手都失败（A172）。剩余：每秒握手数对比 fit 和 nginx；空闲 TLS 内存对照 fit。 |
| HTTP/3 入口 | I10 | 否 | 否（尚未实现） | h3 上的 fixed，对比 fit 和 Caddy。 |
| 路由 v1：按主机和路径查找 | I11 | 否 | 否（已实现，未测） | 查找的微基准（不分配内存），以及路由较多时 fixed 对比 I05 线性扫描的同轮 A/B。 |
| 静态文件 | I12、I13 | 是（2026-11-25，`9d9321e`，`bench/results/m1-2026-11-25-akio-fixes`） | 否 | M1 报告：在 HTTP/1.1、HTTPS/1.1 和 h2 上排第一（nginx 的 1.2 到 3.8 倍），h2c 上为 fit 的 98%。剩余：预压缩文件和大文件的 `sendfile`；akio ADR-124 之后的缓存命中（双环缓存、文件很多时 `max_cache_bytes` 的淘汰）；带 `compress` 的大文件（在 akio 文件池上读取并编码）与 fit 对比。 |
| 上游连接池 | I14 | 是（2026-11-25，通过 proxy 场景，`bench/results/m1-2026-11-25-akio-fixes`） | 否 | M1 报告：连接被复用，HTTP/1.1 上 256 个客户端约用 220 个后端连接。剩余：连接数受限时等待连接。 |
| 反向代理：HTTP/1.1、流式请求体、h2 和 h2c 上游 | I15–I18 | 是（2026-11-25，`9d9321e` 带 akio A171，`bench/results/m1-2026-11-25-akio-fixes`） | 否 | M1 报告：排在 fit 和 nginx 之后第三，HTTP/1.1 上每秒 24.5 万请求、HTTPS/1.1 23 万、h2c 23.8 万、h2 22.1 万（最佳的 86–93%），每请求 CPU 27.5 µs，nginx 18.2，HTTP/1.1 上 p99 3.16 ms，fit 2.20；按锁定的 akio，其内存按路由超时窗口内的请求每个增长约 240 字节（A171）。剩余：剖析每请求开销；双向大请求体，及每个进行中请求占用的内存；带 trailer 的请求（I17 每个请求解析 `Trailer`）；h2c 上游（I18）：与 HTTP/1.1 上游对比每秒请求数和每请求 CPU，许多客户端复用少数连接，以及租用流时的按地址锁（`akit_upstream::h2`）；I17b 每个被代理请求的 `CancelToken` 和 `on_gone`；经 h2c 时（I18b）akio 的每流状态及其响应头与空闲定时器（每个 DATA 帧读一次时钟，每个请求一个定时任务），以及被暂停的下载与有节奏的上传，与 HTTP/1.1 对比。 |
| 负载均衡 v1 和 v2 | I19、I36 | 否 | 否（已实现，未测量） | 每种策略选择的微基准（fit 的 `BenchmarkPick` 和 `BenchmarkPickNew`：九种策略、哈希一个键、查找会话保持 cookie），1 线程和 16 线程；对比单个服务器的 proxy A/B；proxy 场景下 `round_robin` 对比 `p2c`、`header_hash` 和 `sticky_cookie` 的同轮 A/B（每个请求的键哈希，以及首个请求复制的 `Set-Cookie`）。 |
| 被动健康检查 | I20 | 否 | 否（已实现，未测） | 请求路径上记录结果的微基准。 |
| 主动健康检查 | I35 | 否 | 否（已实现，未测） | 不在热路径上（选择地址仍只读每个地址一个原子量）：繁忙引擎上探测的开销，每秒探测数千个地址（http 和 tcp），连同探测定时器的线程（akio A45 之前的临时方案）；proxy 场景下开关健康检查的同轮 A/B。 |
| 访问日志 | I21 | 否 | 否（已实现，未测） | fixed 和 proxy 开关日志的同轮 A/B（json，以及 text 或 line），写文件和写 `/dev/null`；默认 4 MiB 缓冲、磁盘慢时满负载下丢弃的行数；每个请求的开销（装箱的响应过滤器、分片锁、格式化）。 |
| 日志文件：写入端、切割、保留、压缩 | I81 | 否 | 否（已实现，未测） | fixed 和 proxy 场景下，访问日志写到带 `rotate`（`max_bytes` 1 MiB，使其频繁切割；`compress` 开和关）的文件，与写不带 `rotate` 的同一文件、与 I81 之前的 main 做同轮 A/B：访问日志的写线程把每次刷出的缓冲原样交给写入端的队列（不复制），由写入端的线程写盘；磁盘慢时满负载下丢弃的行（日志 4 MiB 缓冲之后是写入端 1 MiB 的队列）；负载下一次切割的开销（一次改名、一次打开、为保留策略列一次目录）以及压缩线程上 gzip 的开销（每 MiB 的 CPU，大量切割时 16 个的队列）。 |
| 每次握手选择证书（读取运行时图的 resolver） | I30 | 否 | 否（已实现，未测） | 一个和十个证书时每秒完整 TLS 握手数，对比 I09 的 `sni_cert` 和 fit。 |
| 明文监听器上的 HTTP-01 检查 | I31 | 否 | 否（已实现，未测） | HTTP/1.1 和 h2c 上的 fixed，与 I31 之前的 main 做同轮 A/B（`akit run` 下每个请求一次前缀比较）。 |
| 带 TLS-ALPN-01 和 OCSP 装订的证书选择 | I32 | 否 | 否（已实现，未测） | 与 I31 对比每秒完整 TLS 握手数，有和没有装订的响应（读取 ALPN 列表，带响应者的文件证书还要读第二个 `Published` 映射）；带响应时握手的大小。 |
| Prometheus 指标 | I37 | 否 | 否（已实现，未测量） | fixed 和 proxy 场景下开关 `metrics` 的同轮 A/B（inline 路由的代价是一次线程局部读和三次原子加；代理的路由在 akio A29 之前还多一个装箱的应答过滤器），以及同时开启访问日志（两者共用一个钩子）；大量路由、64 个线程时的一次抓取（按线程的分片）。 |
| OpenTelemetry | I64 | 否 | 否（已实现，未测量） | fixed 和 proxy 场景下 `telemetry.traces` 关闭、开启且 `sample_ratio` 为 0（每个请求一次字段查找）、开启且为 1（每个请求：生成 id、设置 `traceparent` local、把 span 编码进线程自己的缓冲区、一次分片锁）的同轮 A/B；默认 4 MiB 队列、collector 很慢时满负载下丢弃的 span；导出线程的开销；`Tracer::sample` 和 `Tracer::end` 的微基准（fit 的 `BenchmarkSample`、`BenchmarkEnd`）。 |
| 静态 musl 发布二进制 | I65 | 否 | 否（已实现，未测量） | 用发布的静态 musl 二进制（`scripts/dist.sh`，musl 的分配器）与同一提交的 glibc 构建做 fixed 和 proxy 场景的同轮 A/B、轮次交替：多线程下 musl 的分配器是否损失吞吐量或内存，从而决定全局分配器是否值得引入一个依赖（[决策 009](decisions/009-packaging.zh-CN.md)）。 |
| M4 退出：里程碑结束时的完整测试框架；空闲的 provider | I66 | 否 | 否 | M4 没有改动请求路径。留给基准测试任务（规则 24 的里程碑结束）：用 M4 的构建与 fit、nginx、Caddy 和 Traefik 跑完整测试框架，按 [M4 报告](../bench/results/m4-report-2026-11-25.zh-CN.md) 的方式排名；对正在服务、带一个监视中但空闲的 Kubernetes 或 Docker provider 的 akit 与不带 provider 的同一配置做同轮 A/B、轮次交替，以及 provider 每秒应用一个新版本时请求的 p99。 |
| engine 预算与只统计客户端的连接上限 | I79b | 部分（2026-11-26，akio `76a010c1`：akio 的同轮 A/B，见 akio 的 docs/BENCHMARKS“引擎排队字节预算与已接受连接上限”：HTTP/1.1、pipelined、h2c 与 TCP echo 在 epoll 和 io_uring 上、两项设置关闭与开启时都与 akio 的 main 相差 1% 以内） | 否（已实现，akit 中未测） | akio 一侧已测。akit 中留待测量：fixed 与 proxy 场景在默认 `max_pending_bytes`（内存的一半：监视线程，每次读一次 relaxed load）与去掉预算的同一构建（akio 的 `Live::set_max_pending_bytes(None)`）之间对比，HTTP/1.1 和 h2c；大量慢客户端（10,000 个不读 1 MiB 响应的客户端）下与 fit 对比：akit 的 RSS 是否受预算约束、预算生效期间其他客户端的延迟，以及恢复的速度。 |
| 匹配器 v2 与表达式 | I39 | 否 | 否（已实现，未测） | 带匹配器、表达式和路径正则的 `find_request` 微基准（不分配内存，`tests/no_alloc.rs`）；fixed 场景下对比 I39 基线的同轮 A/B（一条路由、没有匹配器：预期在噪声以内）。 |
| 重写、跳转与头处理 | I40 | 部分（2026-11-25，`2a1422d`，[M3 报告](../bench/results/m3-report-2026-11-25.zh-CN.md)：`rewrite` 每请求 +0.4、`headers` +0.5 µs（epoll；fit 为 +0.3、+1.0）；`redirect` 每秒 815k（fit 765k）） | 否（已实现，未测） | 在 proxy 场景上做同轮 A/B：带 `headers`（请求和响应修改）与 `rewrite` 的路由对比不带它们的同一路由；每个请求上响应过滤器（一个装箱闭包和一次 `Arc` 克隆）的开销，以及把请求字段放在上下文 local 中的开销（在 akio API 需求 A33 之前的临时方案）。 |
| 限流、并发与大小限制 | I41 | 部分（2026-11-25，`2a1422d`，[M3 报告](../bench/results/m3-report-2026-11-25.zh-CN.md)：`rate_limit` +0.2、`concurrency_limit` +0.4、`body_limit` -0.1 µs（epoll；fit 为 +0.4、+0.4、+0.3），单个键，256 个连接） | 否（已实现，未测） | 在 proxy 场景上做同轮 A/B：每种限制设得足够高、从不拒绝的路由对比不带它的同一路由：按 ip 和按头的 `rate_limit`（一把分片锁、一次 SipHash、一次 map 查找），1、1000 和 1000000 个客户端键、64 个线程（分片的争用），与 fit 的 `BenchmarkRate` 一样；`concurrency_limit`（同一共享缓存行上的两次原子操作，以及在 akio A48 之前的一个装箱响应过滤器和一次 `Arc` 克隆）；`body_limit`（每个请求只查一次字段）。 |
| Basic 认证与客户端证书 | I42 | 部分（2026-11-25，`2a1422d`，[M3 报告](../bench/results/m3-report-2026-11-25.zh-CN.md)：记住密码后的 `basic_auth` +0.7 µs、无证书的 `client_cert` +0.0（epoll；fit 为 +0.6、+0.4），HTTP/1.1） | 否（已实现，未测） | 在 proxy 场景上做同轮 A/B：带 `basic_auth` 的路由（已记住的密码：一次 base64 解码、一次 HMAC-SHA-256，以及在 akio A33 之前把请求字段改写进上下文本地值）和带 `client_cert` 与 `subject_header` 的路由（主题按线程缓存），对比不带它们的同一路由，覆盖 HTTP/1.1、h2 和 h3；密码的首次校验（一次挂起，到 akio 线程池再回来）在每秒几百个新用户时的开销，以及大量错误密码对已记住用户延迟的影响。 |
| 转发认证 | I43 | 部分（2026-11-25，`2a1422d`，[M3 报告](../bench/results/m3-report-2026-11-25.zh-CN.md)：epoll 上每请求 +20.4 µs，fit 为 +17.0，每秒请求数为 fit 的 97%，鉴权服务很快（nginx）） | 否（已实现，未测） | 在 proxy 场景上做同轮 A/B：带 `forward_auth`、认证服务在本机且很快的路由，对比不带它的同一路由，覆盖 HTTP/1.1 和 h2：检查的开销（构造认证请求的字段、一次挂起、认证 upstream 的连接池上的一次交换、在应答所在的 I/O 线程上恢复）；成千上万个请求等待慢认证服务时的延迟和内存，以及此时快路由的延迟。 |
| JWT 与 JWKS | I44 | 部分（2026-11-25，`2a1422d`，[M3 报告](../bench/results/m3-report-2026-11-25.zh-CN.md)：HS256、令牌已验证并缓存：epoll 上每请求 +1.9 µs，io_uring +2.4，fit 为 +1.1） | 否（已实现，未测） | proxy 场景上带 `jwt` 的路由与不带它的路由的同轮 A/B，经 HTTP/1.1 和 h2，按算法（HS256、EdDSA、ES256、RS256），令牌已在缓存中（一次 SHA-256 和一次每线程 map 查找，claims 字段在 akio A33 之前改写进上下文局部变量）和重新校验（大量不同令牌：在 I/O 线程上校验签名），同 fit 的 `BenchmarkJWT`；等待 JWK 集获取的请求的延迟，以及期间其他路由的延迟。 |
| 重试、熔断、按路由超时 | I45 | 部分（2026-11-25，`2a1422d`，[M3 报告](../bench/results/m3-report-2026-11-25.zh-CN.md)：`circuit_breaker`、上游健康：epoll 上每请求 +0.7 µs，fit 为 +1.1） | 否（已实现，未测量） | 在 proxy 场景上对带 `retry` 和带 `circuit_breaker` 的路由与不带它们的同一路由做同轮 A/B，上游健康：每个请求读取预算的开销（向满的预算存入不写内存），为重放保留的模板（复制出站字段和最多 64 KiB 的 body，一个装箱的 `Again`），熔断器装箱的响应过滤器及其在共享步计数上的两次原子加，64 个线程（计数的争用）；以及上游一半请求失败时：相对预算的每秒重试数，和熔断器的打开路径。 |
| 加权上游、镜像、故障转移 | I46 | 否 | 否（已实现，未测量） | 在 proxy 场景上对带 `upstreams`（两个，按权重）和带 `failover` 的路由与带 `upstream` 的路由做同轮 A/B：每个请求按权重抽取和读取 `available` 的开销；`mirror` 为 100% 时，对快的镜像（复制字段和消息体，每个请求多一次交换）和对不回应的镜像（保持 256 的上限：请求的延迟和 akit 的内存），覆盖 HTTP/1.1 和 h2。 |
| 中间件（其余） | I48 | 是（2026-11-25，`2a1422d`，[M3 报告](../bench/results/m3-report-2026-11-25.zh-CN.md)，`bench/modules.sh`） | 否 | 每个中间件在 HTTP/1.1 的 proxy 场景上与 fit 对比，akit 分别在 epoll 和 io_uring 上，二者运行同一份配置：每个中间件增加的开销与 fit 相差不到 1 µs，只有 gzip（+6.7 µs，fit +3.9）、`wasm`（+13.6，fit +6.4）和 `forward_auth`（+20.4，fit +17.0）例外。剩下：h2 和 h3 上的同样测量，以及对落后的两项做剖析。 |
| 压缩 | I47 | 部分（2026-11-25，`8413090`，[M3 报告](../bench/results/m3-report-2026-11-25.zh-CN.md)：以 fit 的默认级别 gzip 1 KiB JSON：每请求 +6.7 µs，fit 为 +3.9；不压缩的响应 +0.8，fit 为 +1.2；仅 HTTP/1.1） | 否（已实现，未测量） | 按 I47 的要求，每种编码对比 fit：代理场景前面加 `compress`，完整的 JSON 消息体和服务端推送事件流，走 HTTP/1.1 和 h2，每种编码用 fit 的默认级别（每秒响应数、服务端 CPU、线上字节数）；过滤器对它不压缩的响应（图片）的开销，与不加中间件对比；同时保持的压缩流（1000 个）的内存；静态处理器的副本与 fit 的对比（内存中文件的 `compress`）。 |
| 重写与重定向的正则 | I48 | 否 | 否（已实现，未测量） | 对比 fit：代理场景前面加带 `regexp` 与 `replacement` 的 `rewrite`（匹配与不匹配）和 `to` 中带分组的 `redirect`，每秒请求数和每请求分配次数，与不加中间件对比；akio-regex 的 `captures` 与 `replace_all` 在路径长度文本上对比 Go 的 `regexp`。 |
| WebSocket 与 gRPC 代理 | I49、I50 | 部分（2026-11-25，1 万个空闲 WebSocket，`bench/results/m1-2026-11-25-akio-fixes`：每个 4,452 字节，fit 5,916，nginx 22,275） | 否（I49 与 I50 已实现） | 对比 fit：经 akit 的 WebSocket 每秒消息数（小消息回显）与每秒字节数（大消息），来自 HTTP/1.1、h2 和 h3 客户端；每个打开的 WebSocket 的内存（空闲时，以及每个方向各有 64 KiB 待发时）；每秒握手数（生成密钥、上游连接离开连接池）；I50：对比 fit，每秒 gRPC 调用数（一元调用，h2c 与 h2 上游）与一个流上每个方向的每秒消息数。 |
| TCP 和 UDP 代理、SNI 路由、单端口多协议 | I51–I53 | 否 | 否（I51–I53 已实现，未测） | 按 I51 的要求对比 fit 和 nginx 的 `stream`：TCP 吞吐（fit 的 `BenchmarkTCPProxy`，每次回显 256 KB，直连与经 akit 对比）、每秒连接数、每个空闲连接的内存；UDP 每秒数据报数与同时的会话数；10,000 个连接时每个服务在 akit 定时线程上的空闲扫描（akio A78 之前的权宜之计）；共享端口上协议嗅探的开销（I53）。I52：按 SNI 分流的监听器（拨号前读取并解析 ClientHello）与不分流的监听器每秒连接数的对比，以及接受时读 PROXY 头、向上游发送 PROXY 头时的情形；带与不带 `proxy_protocol` 的监听器上 HTTP/1.1 与 h2c 的请求速率，以及不带时 akio ADR-118 的开销（每个请求一次 `OnceLock::get`）。 |
| eBPF sockmap 中转 | I51b | 否 | 否（I51b 已实现，未测） | 在基准主机上以 root（Linux 6.15+）运行，对比不带 `sockmap` 的同一监听器以及 fit：TCP 吞吐（fit 的 `BenchmarkTCPProxy`，每次回显 256 KB）和每 GB 的 CPU，每秒连接数（每个连接拼接的开销：两次 `TCP_INFO` 读取、映射表的两次插入和闸门），以及小回显的延迟；10,000 个已拼接连接时 akit 定时线程上的空闲扫描（每次扫描每个套接字三次系统调用），以及大量连接同时关闭时排空的轮询。 |
| 正向代理：SOCKS 与 HTTP CONNECT | I55 | 否 | 否（已实现，未测） | 对比 fit（原始吞吐另对比 nginx 的 `stream`）：经 SOCKS5 隧道和 HTTP/1.1 CONNECT 隧道的每秒字节数（每次回显 256 KB，同 fit 的 `BenchmarkTCPProxy`），每秒建立的隧道数（握手、规则检查、在线程池上解析名字以及 `connected` 检查），每个空闲隧道的内存；共享端口上 SOCKS 的开销（akio-mux 的问候规则与 HTTP、TLS 并列）对比只有 HTTP；10,000 条隧道时 akio-socks 看门狗线程的开销（ADR-119 在其上增加了连接超时）。 |
| 响应缓存 | I67、I68、I69 | 部分（2026-11-25，I76，`346ad8b`：基准框架的 `cache-hot` 和 `cache-zipf` 对照 nginx 和 fit，以及 1,000 个客户端的击穿，见 [M5 报告](../bench/results/m5-report-2026-11-25.zh-CN.md)） | 否 | I76 已测：经基准框架的内存命中（4 KiB；akit 第一，HTTP/1.1 上是 nginx 的 1.5 倍、h2c 上 6.2 倍，100,000 个路径时 1.75 倍，领先 fit 12% 到 23%），以及 1,000 个客户端请求一个冷键时只取一次源站。剩下的，对比 fit，按 I67 的实现：内存命中（4 KiB 与 1 MiB 消息体，HTTP/1.1 与 h2），经代理保存的未命中（旁路复制的流式消息体）对比仅代理，缓存直接放行的请求（POST、`no-store` 响应）对比无中间件的开销，大量键与单个热点键下分片锁的表现。I68：从磁盘读回的命中（64 KiB 与 1 MiB，大量并发下磁盘线程池的线程与队列），未命中率高到填满 `MAX_PENDING_WRITES` 时的磁盘写入，大量客户端对同一个键的惊群，开启锁与 `disable_lock` 两种情况（等待者的挂起与恢复、每次获取一个定时器），以及 stale-while-revalidate 的后台获取。akio ADR-116 的旁路：有旁路的流对比无旁路的流。I69：带标签响应（0 与 32 个标签）对比无标签响应的保存开销，以及服务期间对大 zone 按前缀和按标签清除（分片锁与磁盘索引锁的持有时间）。 |
| FastCGI | I75 | 否 | 否（已实现，未测量） | 对比 fit 和 nginx 的 `fastcgi_pass`，经 TCP 和 Unix 域套接字连接 PHP-FPM（或立即应答的 FastCGI 应用）：小页面的每秒请求数与延迟，HTTP/1.1 与 h2；1 MiB 上传与 16 MiB 下载（每个进行中请求的内存，双向 64 KiB 水位线）；连接复用对比每个请求一个连接；客户端多于 `max_conns` 时连接池的队列；`try_files` 的开销（每个候选在 akio 逻辑线程池上一次 `stat`）对比不带它的路由；每秒大量请求时，handler 每 100 ms 在 akit 定时器线程（A45 的临时方案）上对其交换的巡检。 |
| WASM 插件 | I71、I72 | 部分（2026-11-25，I76，`346ad8b`：fit 的 `headers` 插件在 akit 和 fit 中，HTTP/1.1 和 h2c，见 [M5 报告](../bench/results/m5-report-2026-11-25.zh-CN.md)） | 否 | I76 已测（`bench/run.sh -servers akit-wasm,fit-wasm -scenarios fixed,wasm`）：fit 的 `headers` 插件在 HTTP/1.1 上让 akit 每个请求多花 14.7 µs CPU，fit 6.8 µs（akio-wasm 解释执行，wazero 编译执行：akio API 请求 A180，akio 的 W5）；h2c 上 2,048 个同时进行的请求超出 1,024 的队列，两者都立即以 503 回答多出的部分。剩下的：对照 fit（wazero 的编译器）和无插件：请求经过 fit 的 `headers` 插件（Go guest）和一个空插件（`fit_on_request` 返回 0），HTTP/1.1 和 h2，每秒请求数和延迟，对比不带该中间件的路由（交给池再交回、复制请求、调用本身）；实例上单独的 guest 调用（akio 的 W5：akio-wasm 的解释器对比 wazero 的解释器和编译器）；客户端多于 `instances` 时的池（队列，超出后 503）；trap 之后重建实例（Go guest 的 `_initialize`）。Proxy-Wasm（I72）：请求经过 Proxy-Wasm Go SDK 的 `http_routing`（配置 `1`，请求不变），对照 fit 和无插件，如 fit 的 `BenchmarkProxyWasm`；响应经过 fit 的 `response` 过滤器（`upper`）和 SDK 的 `http_headers`，头部在池上挂起、响应体在池上过滤，一个完整的 4 KiB 响应体和一个流式的 1 MiB 响应体，HTTP/1.1 和 h2，对照 `response: none` 和无插件。 |
| 有流量时频繁替换配置 | I38 | 否 | 否（已测试，未测量） | fixed 和 proxy 场景，HTTP/1.1 和 h2c，期间每 10 ms 经 SIGHUP 和 Admin API 替换一次配置，对照同一轮中不替换：替换期间的每秒请求数、p99 延迟和错误，以及替换 10 万次后 akit 的 RSS 和描述符数；完整测试套件的 M2 报告（规则 24，里程碑结束）。 |
| 监听器拆开 absolute-form 目标 | I77 | 否 | 否（已实现，未测量） | 路由前每个请求多一次比较（目标是否以 `/` 开头）：在同一轮中对 I77 之前的 main 做 HTTP/1.1 上 fixed 场景的 A/B；预计没有开销。akio 实现 A200 后去掉。 |
| akio 请求替换掉的权宜实现（A12、A79、A45、A117、A30、A54、A29、A75、A78、A22、A170、A24、A49、A35、A148） | —（akio 请求） | 否 | 否（已实现，未测量） | 对照这些提交之前的 main，同一轮 A/B 测 fixed 和 proxy 场景，HTTP/1.1、h2c 和 h3：访问日志和指标走监听器的响应观察者（`set_observed`，不分配）对比装箱的过滤器，以及每个请求都调用 `limit_body`；代理在 HTTP/1.1 上按长度成帧的下载对比 chunked；第 4 层转发由 akio 的每连接空闲超时关闭对比 akit 的扫描（大量空闲连接）；sockmap 转发经 `akio::ebpf::Tunnel`（以 root 测 TCP 吞吐，同 I51b 一行）；健康检查探测负载走 `akio::timer::after_send` 对比 akit 原来的定时线程。 |

## M0 基础

### I01 工作区骨架与 CI

- **做什么**：Cargo 工作区，按 [工作区结构](#工作区结构) 建立各个空库 crate 和 `akit` 可执行文件；edition 和 MSRV
  与 akio 一致；akio 按 `rev` 固定；每个 crate 都有 `#![forbid(unsafe_code)]`；`clippy.toml`、`deny.toml`、
  `rustfmt.toml`；带 `check`、`test`、`test-changed`、`lint` 的 `Makefile`（或 `xtask`）；CI 运行 fmt、clippy，
  在 macOS 和 Linux 上跑测试（epoll，以及 Docker 中的 io_uring），运行 cargo-deny，并检查每个 `docs/*.md` 都有
  对应的 `.zh-CN.md`。
- **完成标准**：`make check` 在本机和 CI 中通过；空的 `akit --version` 能运行。
- **涉及**：全部为新建。
- **参考**：akio 的 `Cargo.toml`、`deny.toml`、`clippy.toml`、`docs/rules.md`。
- **fit**：`Makefile`、`.github/workflows/ci.yml`。

### I02 配置模型 v0 与严格的 JSON 读取器

- **做什么**：fit 顶层（`listeners`、`routes`、`upstreams`、`admin`、`log`）和 `respond` handler 的 Rust 类型；
  akit 自己的 JSON 读取器（或按 [决定 001](decisions/001-dependency-policy.zh-CN.md) 选定的读取器），拒绝未知字段、
  重复键和错误类型，并给出出错路径；像 fit 一样允许 `//` 与 `/* */` 注释和尾逗号。
- **完成标准**：fit 的 `hello` 示例的 JSON 版能加载；表驱动测试覆盖每种错误及其路径；CI 中对读取器 fuzz 一分钟。
- **涉及**：`akit-config`。
- **fit**：`config`，决定 001（严格解码）。

### I03 核心 trait 与模块注册表

- **做什么**：监听器、匹配器、路由器、中间件、handler、service 和上游的 trait；把配置名（`respond`、`proxy`
  等）映射到模块构造函数的注册表，初始化可以失败，释放时执行清理。模块在编译期注册（没有全局可变状态）。
- **完成标准**：一个测试模块能注册、从配置初始化并被清理；未知模块名报错并写明该名字。
- **涉及**：`akit-runtime`。
- **参考**：akio 的 `Handler`、`Acceptor`，`akio-http` 的 `Router` 和 `Middleware`。
- **fit**：`module`、`runtime`。

### I04 运行时图及其替换

- **做什么**：`compile(config) -> Graph`，图发布前所有模块都已初始化；当前图放在原子指针后面；每个 I/O 线程缓存
  一份，通过 epoch 检查刷新，请求不增减任何共享引用计数；持有某个图的最后一个请求和连接结束时，释放该图（清理其
  模块）。
- **完成标准**：测试在多个 I/O 线程处理请求时从另一线程替换图，每个旧图恰好释放一次；微基准表明每个请求的读取
  不在共享内存上做原子 RMW。
- **涉及**：`akit-runtime`。
- **fit**：`runtime/graph*.go`，决定 004。

### I05 `akit run`：监听器与 `respond`

- **做什么**：在 akio 的 `Engine` 上按配置建立监听器（HTTP/1.1 和 h2c）；`respond` handler（状态码、头、body）；
  没有匹配的路由时返回 404。
- **完成标准**：e2e 测试中，fit 的 `hello` 示例经 HTTP/1.1 和 h2c 应答 `/hello`。
- **涉及**：`akit-listener`、`akit-handler`、`akit`。
- **参考**：akio 的 `Engine::builder`，`akio-http` 服务端。

### I06 CLI、信号与优雅关闭

- **做什么**：`akit run`、`akit validate`、`akit version`，带 fit 的参数（`-c` 可重复，可以是文件或目录）；SIGINT
  和 SIGTERM 触发优雅关闭，带排空超时；退出码与 fit 一致。
- **完成标准**：e2e 测试覆盖 validate 的错误、多个 `-c`，以及关闭时让进行中的请求完成。
- **涉及**：`akit`。
- **fit**：`cmd/fit`、`cli`。

### I07a 兼容性门禁 I：固定夹具、字段清单、测试框架

从 I07 拆分出来，使不依赖配置模型的那一半可以与 I02 并行。

- **做什么**：把 fit 的 `templates/`（三种格式，单文件和 `conf.d` 目录两种形式）和 fit-examples 的每份配置
  按固定提交复制到 `tests/compat/fixtures/`，附上许可证、`SOURCE.md` 和移动固定提交的脚本；fit 接受的每个
  字段路径的清单（`tests/compat/fit-fields.txt`），由开发工具从 fit 的 JSON Schema 生成；枚举夹具并判定每个
  结果的测试框架（有效，或只因 "not supported yet" 且指向清单中的字段而被拒绝）；`docs/compatibility.md`
  附上生成的完整字段清单，每个字段都是"尚不支持"。
- **完成标准**：测试框架用替身校验函数测试；`make check` 检查清单及其表格与固定提交一致。
- **涉及**：`tests/compat`、`tools`、`docs/compatibility.md`。
- **fit**：`templates/`、`schema/fit.schema.json`、`docs/reference/config.md`；fit-examples 的 `examples_test.go`。

### I07b 兼容性门禁接入 validate

- **做什么**：打开 `akit-compat` 的 `compat-gate` feature：其测试对每个夹具调用 `akit_config::from_json_file`（`gate.rs` 中的 `validate_with_akit_config`，随 I23、I24 扩展到 `conf.d` 目录、.conf 和 YAML）
  （文件或 `conf.d` 目录，与 `akit validate -c` 的参数相同）；akit-config 的错误给出路径，由测试框架与字段清单
  匹配；每个夹具要么通过，要么只因 "not supported yet: <field>" 失败；确定校验会打开哪些文件（示例引用的
  文件和 WebAssembly 插件，相对于 fit-examples 根目录），以及 fit 只做 adapt 的 `https` 示例如何处理。读取后
  又被忽略的字段不能通过门禁。
- **完成标准**：门禁在 CI 中运行（`make compat-gate`），其报告已提交；I02 读取的字段已写入
  `tests/compat/supported.txt`。
- **涉及**：`tests/compat`、`akit-config`（如需调整错误路径）、CI。
- **fit**：`cli`（`validate`）、fit-examples 的 `examples_test.go`。
- **实际做法**：不用 feature：门禁是普通测试，`make check` 和 CI 的 `cargo test --workspace` 都会运行。
  akit 还没有读取器的格式的夹具（.conf 和 `conf.d` 直到 I23，YAML 直到 I24）结果为 "format not supported
  yet"；每个结果都是入库的 `tests/compat/gate-report.txt` 中的一行，每次运行都必须与之一致（用
  `make compat-bless` 重写）。校验会为示例打开哪些文件，移到第一个读取示例的迭代 I23 决定。见
  [进展](progress.zh-CN.md)。

### I08 基准框架与基线

- **做什么**：akit 复用 fit 的基准框架（`bench/`）：相同的场景、配置和压测工具，akit 和 fit 都作为被测服务器，与
  nginx、Caddy 和 Traefik 并列；为压测机交叉编译并在其上运行的脚本；用于热路径改动的同轮 A/B 脚本。
- **完成标准**：压测机上五个服务器的基线已提交，并记录机器和提交。
- **涉及**：`bench`。
- **fit**：`bench/README.md`、`bench/ab.sh`。**akio**：`tools/bench`、`quiet.sh`。

## M1 代理核心

### I09 从文件加载证书的 TLS 入口

- **做什么**：基于 `akio-tls` 的 TLS 监听器，证书和私钥来自文件；ALPN 选择 h2 或 http/1.1；按 SNI 在多张证书中
  选择；fit 的 TLS 字段（版本、密码套件、客户端认证）。
- **完成标准**：e2e 测试经 HTTPS 以 HTTP/1.1 和 HTTP/2 访问到路由，并能按 SNI 访问。
- **涉及**：`akit-listener`、`akit-tls`。
- **参考**：`akio-tls` 的 `ServerConfigBuilder::sni_cert`。

### I10 HTTP/3 入口

- **做什么**：基于 `akio-h3` 的 HTTP/3 监听器，共用证书来源；同一 host 的 TCP 监听器上通告 `Alt-Svc`。
- **完成标准**：e2e 测试经 h3 访问到同一路由，并在 h2 上看到 `Alt-Svc`。
- **涉及**：`akit-listener`。
- **实际完成**：还涉及 `akit-config`（`h3` 协议）、`akit-tls`（文件读一次，生成两份配置）、`akit-runtime`
  （`Graph::h3_tls`）和 `akit`（打印的地址）。与 fit 一样，HTTP/3 监听在该监听器 TCP 一侧所用的端口上；见
  [进度](progress.zh-CN.md)。

### I11 路由 v1：host 与 path

- **做什么**：带 host（精确、通配）和 path（精确、前缀）匹配器的路由；先看显式优先级，再按最具体优先，与 fit 的
  排序一致；没有匹配时返回 404。I05 的路由器 v0 已按 fit 的规则逐条扫描路由：把扫描换成按 host 和 path 查找。
  先规范化路径，与 fit 的监听器一致（去掉点段和空段；越过根目录、或在转义里藏了分隔符的路径返回 400）。
- **完成标准**：移植过来的 fit 路由器表驱动测试通过；查找不分配内存；移植过来的 fit 路径规范化测试通过。
- **涉及**：`akit-router`。
- **fit**：`router`。

### I12 静态文件 I

- **做什么**：基于 `akio-http` 的 `Files` 的 `static` handler：根目录、索引文件、ETag、`Last-Modified`、条件请求
  和范围请求、MIME 类型；拒绝路径穿越；缓存未命中时的文件打开不在 I/O 线程上进行。
- **完成标准**：移植过来的 fit 静态文件测试通过，包括穿越尝试。
- **涉及**：`akit-handler`。
- **参考**：`akio-http` 的 `files.rs`。

### I13 静态文件 II

- **做什么**：按 `Accept-Encoding` 选择预压缩副本（`.br`、`.zst`、`.gz`）；`try_files` 回退；大文件用 `sendfile`；
  目录列表默认关闭。
- **完成标准**：测试覆盖协商和回退；一个基准场景用到它。
- **涉及**：`akit-handler`、`bench`。

### I14 基于 HTTP/1 客户端的上游连接池

- **做什么**：带各自服务器的命名上游（`upstreams`）；每个上游一个连接池，基于 akio 的 HTTP/1 客户端，在 akio 允许
  时与监听器共用同一个 engine；限制连接数和等待连接的请求数；图被释放时，连接池先排空再关闭。
- **完成标准**：测试覆盖各项上限、负载下排空连接池，以及拒绝连接的上游。
- **涉及**：`akit-upstream`。
- **参考**：`akio-http` 的 `client.rs`。
- **fit**：`upstream`，决定 004。

### I15 反向代理 I：请求与头处理

- **做什么**：`proxy` handler：请求转发到连接池，响应写回；删除逐跳头；设置 `X-Forwarded-For/-Proto/-Host` 和
  `Forwarded`；可信代理；按 fit 的字段透传或改写 `Host`。
- **完成标准**：移植过来的 fit 头处理测试通过。
- **涉及**：`akit-proxy`。
- **fit**：`handler/proxy`。

### I16 反向代理 II：流式 body 与联动背压

- **做什么**：请求体边到边流式发往上游，响应体流式写回；一侧超过水位时对另一侧 `hold_reads`，在 `on_drain` 时
  恢复；两个方向都支持 chunked 和固定长度；body 上限。
- **完成标准**：在 epoll 和 io_uring 上，用慢客户端和慢上游测试，双向大 body 时内存有上限。
- **涉及**：`akit-proxy`。

### I17 反向代理 III：超时、取消、trailer

- **做什么**：连接、响应头、空闲和总时长超时，按 fit 的方式返回 502 和 504；客户端离开时取消上游请求；透传
  trailer。
- **完成标准**：移植过来的 fit HTTP/1.1 代理一致性测试套件通过。
- **涉及**：`akit-proxy`、`akit-upstream`。
- **拆分**：akio 还做不到的部分移到 I17b。

### I17b 反向代理 III b：响应头与空闲超时、响应头之前的取消、写停滞

- **做什么**：上游的 `response_header_timeout` 和 `response_idle_timeout`（akio API 请求 A36）；客户端在响应头之前
  离开、或经 HTTP/1 在上传中途离开时取消上游请求（A37、A38）；`limits.write_stall_timeout`，重载时也生效（A38）。
- **完成标准**：移植过来的 fit `TestResponseHeaderTimeout`、`TestResponseIdleTimeout` 和 `TestClientAbort` 的其余
  部分通过。
- **涉及**：`akit-proxy`、`akit-upstream`、`akit-listener`、`akit-config`。
- **拆分**：akio 的 h2 客户端尚未遵守的部分（h2 上的超时和取消，akio API 请求 A66）在 I18b 之前由
  `akit_upstream::h2` 的定时检查完成，I18b 起交给 akio。

### I18 HTTP/2 与 h2c 上游

- **做什么**：基于 akio 的 `H2Client`，经 h2 和 h2c 访问上游，带逐流背压和每连接流数上限。
- **完成标准**：一致性测试套件对 h2 和 h2c 上游都通过。
- **涉及**：`akit-upstream`、`akit-proxy`。
- **参考**：`akio-http` 的 `h2client.rs`。
- **拆分**：akio 的 h2 客户端还做不到的部分移到 I18b。

### I18b HTTP/2 上游 II：由 akio 重置、计时和控制节奏的流

- **做什么**：用 akio 的 h2 客户端替换 I18 在 `akit_upstream::h2` 中的权宜之计：放弃的流被重置，`response_timeout`
  由 akio 启用，请求 trailer 能发出（akio API 请求 A51）；h2 下载按流暂停、上传按流控制节奏（A52）；每个连接按服务器的
  `SETTINGS_MAX_CONCURRENT_STREAMS` 确定流数，拨号受上游 `connect_timeout` 限制，ALPN 未选择 h2 时 `h2` 回退到
  HTTP/1.1（A53）；响应头超时、空闲超时和取消令牌由 akio 遵守（A66），定时检查中对它们的检查去掉。
- **完成标准**：h2 代理测试中的 `request_trailers` 通过；在 epoll 和 io_uring 上，慢客户端和慢上游经 h2 时内存有界，
  与 I16 的测试在 HTTP/1.1 上所示一致。
- **涉及**：`akit-upstream`、`akit-proxy`。

### I19 负载均衡 v1

- **做什么**：轮询、加权轮询和最少连接；在 fit 的结果允许时状态按 I/O 线程保存，必须共享时才共享。
- **完成标准**：分布测试在容差内符合 fit 的预期。
- **涉及**：`akit-upstream`。

### I20 被动健康检查

- **做什么**：服务器在一个时间窗内失败 N 次后标记为下线，超时后恢复，与 nginx 的 `max_fails` / `fail_timeout`
  一致。
- **完成标准**：测试覆盖标记下线、恢复，以及所有服务器都下线。
- **涉及**：`akit-upstream`。

### I21 访问日志

- **做什么**：采用 fit 格式和字段的结构化访问日志，按 I/O 线程缓冲，由一个有界的写线程写出；缓冲满时丢弃日志行并
  计数，而不是阻塞。
- **完成标准**：测试覆盖各字段，以及写入比流量慢的情况。
- **涉及**：`akit-observe`。
- **fit**：`observe`、`logs`。

### I22 SIGHUP 重载

- **做什么**：SIGHUP 构建新图并替换；配置有误时保留正在运行的图；地址没变的监听器保留。新增和移除的监听器使用
  A1（已在 akio `a9a620ab` 上完成；之前用的 `adopt_tcp` 临时措施已去掉）。
- **完成标准**：e2e 测试在有流量时重载，没有失败的请求。
- **涉及**：`akit`、`akit-runtime`、`akit-listener`。

### I23 .conf 读取器

- **做什么**：把 fit 的 .conf 格式读入同一模型，错误带行号和列号；按 fit 的合并方式处理拆分的文件和目录；兼容性
  门禁的钩子用它读取 .conf 文件和 `conf.d` 目录，并确定校验会为示例打开哪些文件（示例引用的文件和 WebAssembly
  插件，相对于 fit-examples 根目录，以及替身密钥），这一项从 I07b 移到这里。
- **完成标准**：fit 模板和 fit-examples 中的每个 .conf 读入的模型与对应的 JSON 版相同。
- **涉及**：`akit-config`、`tests/compat`。
- **fit**：`docs/reference/fit.md`、`config`（.conf 解析器），决定 021。

### I24 YAML 读取器

- **做什么**：把 YAML 读入同一模型，范围是 fit 接受的子集（用决定说明确定：akit 自己的读取器或某个 crate，依据
  决定 001）。
- **完成标准**：fit 的 YAML 模板读入的模型与对应的 JSON 版相同。
- **涉及**：`akit-config`。
- **fit**：决定 019。

### I25 M1 一致性测试与报告

- **做什么**：在 HTTP/2 入口上运行 h2spec；在 HTTP/1.1 和 HTTP/2 上运行移植过来的一致性测试套件；静态文件和代理
  的基准；10k 和 100k 连接时每个空闲连接的内存。
- **完成标准**：带 *akit 的现状* 的 M1 报告已提交，路线图中 M1 的退出标准满足。
- **涉及**：`bench`、`tests`。
- **已完成**（[M1 报告](../bench/results/m1-report-2026-11-25.zh-CN.md)）：h2spec 145/145；一致性测试套件
  `crates/akit/tests/conformance.rs` 通过（依赖 akit-proxy 对 A169 的临时措施）；基准与 1 万、10 万空闲连接的内存已发布，
  HTTPS/1.1 和代理内存需要 akio 的 A172 和 A171（在 akio 分支上，尚未锁定）。性能目标中静态文件达到，代理吞吐量、代理 p99
  以及空闲 HTTP/1.1 和 TLS 内存尚未达到。

## M2 动态配置与 TLS

### I26 Admin API I：加载、读取、版本

- **做什么**：监听 localhost 或 Unix 套接字的 Admin API：加载整份配置、读取配置、版本和 ETag，路径和请求体与 fit
  一致。
- **完成标准**：移植过来的 fit 对这些路径的 Admin API 测试通过。
- **涉及**：`akit-admin`。
- **fit**：`admin`，决定 005。
- **已完成**（决定 006）：运行在自己的 akio 引擎上，加载由控制线程经 I22 的重载应用；Unix 套接字自 akio A23 起支持
  （2026-11-25），拒绝请求体后关闭连接要等 A24。

### I27 Admin API II：局部修改、回滚、认证

- **做什么**：按路径局部修改，回滚到更早的版本，可选认证，通过 ETag 拒绝并发写入。
- **完成标准**：测试覆盖冲突、回滚和认证。
- **涉及**：`akit-admin`。
- **fit**：`admin`，决定 005 的 I25 更新。
- **已完成**（决定 006 的 I27 更新）：`PUT`、`PATCH` 和 `DELETE /config[/<pointer>]`，`POST /rollback/{n}`、
  `POST /reload` 和 `?dry_run=1`，每个修改都在为应用排序的锁下从正在运行的配置做出；`admin.token_file`。
  API 的 TLS（以及随之而来的非回环地址）没有做；401 之后关闭连接要等 A24。

### I28 运行时的监听差量

- **做什么**：替换时绑定新地址，移除的地址带超时排空，不变的保留，与 I22 在 SIGHUP 时的做法相同（基于 A1，`adopt_tcp` 临时措施已去掉）。依赖 akio A1。
- **完成标准**：测试在有流量时增删监听器，没有失败的请求。
- **涉及**：`akit-listener`、`akit-runtime`。
- **fit**：`listener.Server.Reload`、`Plan` 和 `binding.same`；`listener/reload_test.go`。
- **已完成**：在 I22 的运行时监听器之上，按 fit 的 `binding.same` 决定是否保留监听器：证书文件和 `tls.acme` 名字可以变
  （由它的管理器提供新的证书），客户端 CA 按内容比较。服务 h3 的启动监听器仍等待 A22；排空时在空闲 HTTP/1 连接上发出的
  请求会遇到关闭（A54）。

### I29 文件 provider

- **做什么**：监视配置文件（按 fit 的决定采用轮询），并经与 Admin API 相同的路径应用改动。
- **完成标准**：测试覆盖修改、错误的修改和被删除的文件。
- **涉及**：`akit-admin`。
- **fit**：决定 006。

### I30 证书管理与存储

- **做什么**：放在 trait 后面的证书管理器，带文件系统存储和锁；按 SNI 从中取证书提供服务；重新加载时不丢握手。
- **完成标准**：测试覆盖存储、加锁，以及有流量时替换证书。
- **涉及**：`akit-tls`。
- **fit**：`tlsmgr`。

### I31 ACME I：账户、订单、HTTP-01

- **做什么**：akit 自己的 ACME 客户端，基于 `akio-crypto`（JWS、ECDSA）和 `akio-x509`（CSR），不在 I/O 线程上
  运行；账户、订单，HTTP-01 由监听器应答。
- **完成标准**：针对 Pebble 的测试能签发证书。
- **涉及**：`akit-tls`。
- **fit**：决定 007。

### I32 ACME II：TLS-ALPN-01、续期、OCSP

- **做什么**：TLS-ALPN-01；在到期前续期，带抖动和退避；OCSP stapling。
- **完成标准**：针对 Pebble 的测试覆盖 TLS-ALPN-01 和续期；stapling 有检查。
- **涉及**：`akit-tls`。
- **已完成**：TLS-ALPN-01，按有效期和按 ARI（RFC 9773）续期，为 ACME 的证书以及受信任根证书下的文件证书装订 OCSP。
  用系统根证书为文件证书装订等待 akio A57；握手后关闭 `acme-tls/1` 连接等待 A58。

### I33 默认 HTTPS

- **做什么**：除非另有配置，配置中的每个主机名都获得证书；HTTP 跳转到 HTTPS；fit 的默认值和关闭选项。
- **完成标准**：只写了主机名的配置能针对 Pebble 提供 HTTPS。
- **涉及**：`akit-tls`、`akit-runtime`。
- **已完成**：fit 的 `auto_https` 在 `akit-config` 中实现（读配置时生成或补全监听器），重定向在 `akit-listener` 的分发器中；
  只写一个主机名的配置通过生成的 `http` 监听器上的 HTTP-01 从 Pebble 获取证书，并在 `https` 上服务。

### I34 按需 TLS

- **做什么**：在握手时为 "ask" 端点允许的名字签发证书，带限流。依赖 akio A2（由稍后完成的回调选择证书）。
- **完成标准**：测试覆盖允许、拒绝和被限流的名字，签发期间 I/O 线程从不阻塞。
- **涉及**：`akit-tls`。
- **已完成**：fit 的 `acme.on_demand` 和 `tls.on_demand`；询问（akio-http 的异步客户端）和 ACME 工作线程接收名字时，
  握手的 `CertDone` 等待；fit 对询问、速率、名字数和闲置名字的上限；针对 Pebble 的真实握手测试。

### I35 主动健康检查

- **做什么**：HTTP 和 TCP 探测，带间隔、阈值和抖动，在 akio 的客户端上运行。
- **完成标准**：停止应答的上游在其时间窗内退出轮转，之后能恢复。
- **涉及**：`akit-upstream`。

### I36 负载均衡 v2

- **做什么**：随机、P2C，按 IP、头、cookie 或 URI 的一致性哈希，基于 cookie 的会话保持。
- **完成标准**：分布和会话保持测试通过。
- **涉及**：`akit-upstream`。

### I37 Prometheus 指标

- **做什么**：按路由器、service 和上游的指标；连接和连接池的 gauge；重载和证书到期；akio 的 engine 指标；采用 fit
  的指标名。
- **完成标准**：抓取测试对照 fit 检查名字和标签。
- **涉及**：`akit-observe`。

### I38 M2 退出：频繁替换测试与 staging 证书

- **做什么**：有流量时进行数千次替换；一张 Let's Encrypt staging 证书；健康检查时间窗测试。
- **完成标准**：路线图中 M2 的退出标准满足，结果记录在 progress 中。
- **涉及**：`tests`、`bench`。
- **已完成**：频繁替换测试（`crates/akit/tests/churn.rs`：在 `akit run` 下经 SIGHUP 和 Admin API 替换 2000
  次，以及在进程内经 `Listeners::reload` 在 epoll 和 io_uring 上替换）和健康检查时间窗测试（`health_window.rs`）
  通过；只写了主机名的配置经 `akit run` 从 Pebble（在基准主机的 Docker 中）取得证书并提供 HTTPS（`acme_m2.rs`）。
  频繁替换测试发现每次重载都重建所有上游的连接池（逐个重新拨号，健康状态重置）：现在未变的上游像 fit 一样保留连接池。
  同一测试于 2026-11-26 在一台公网主机上为一个公网域名对 Let's Encrypt 的 staging CA（`AKIT_ACME_STAGING_NAME`）运行
  通过，证书由 `(STAGING) Baloney Bulgur YE2` 签发（见 [progress](progress.zh-CN.md)）。

## M3 网关能力

### I39 匹配器 v2 与表达式

- **做什么**：method、头、query、远端 IP 和 CIDR、正则路径（`akio-regex`），以及组合它们的 fit 表达式语言；在编译
  配置时编译。
- **完成标准**：移植过来的 fit 匹配器测试通过；查找仍不分配内存。
- **涉及**：`akit-router`。

### I40 重写、跳转与头处理

- **做什么**：路径重写、去前缀、跳转、请求头和响应头修改，值在编译时构建。
- **完成标准**：移植过来的 fit 相关测试通过。
- **涉及**：`akit-middleware`。

### I41 限流、并发与大小限制

- **做什么**：按键限流（令牌桶）、并发限制、请求大小限制；在精度允许时状态按 I/O 线程分片。
- **完成标准**：测试覆盖每种限制在低于、等于和超过上限时的表现。
- **涉及**：`akit-middleware`。

### I42 Basic 认证与 mTLS

- **做什么**：Basic 认证，密码哈希以常数时间比较；按路由要求客户端证书。
- **完成标准**：测试覆盖两者；哈希计算较慢时（bcrypt）不在 I/O 线程上校验。
- **涉及**：`akit-middleware`、`akit-tls`。

### I43 ForwardAuth

- **做什么**：认证请求经 akio 的客户端发出；请求等待时不占用线程；按 fit 的字段复制头。
- **完成标准**：测试覆盖允许、拒绝、慢的和宕掉的认证服务器。
- **涉及**：`akit-middleware`。

### I44 JWT 与 JWKS

- **做什么**：每个请求校验 JWT；JWKS 由控制面拉取并缓存。
- **完成标准**：测试覆盖各算法、过期、密钥轮换和 JWKS 不可用。
- **涉及**：`akit-middleware`。

### I45 重试、熔断、按路由超时

- **做什么**：带预算和幂等规则的重试；熔断；按路由超时。
- **完成标准**：测试覆盖每一项，包括无法重放的 body。
- **涉及**：`akit-proxy`、`akit-upstream`。

### I46 加权服务、镜像、故障转移

- **做什么**：按权重拆分的 service，镜像到第二个 service 且不等待它，以及故障转移。
- **完成标准**：测试覆盖分布、慢的镜像和故障转移。
- **涉及**：`akit-upstream`、`akit-proxy`、`akit-config`。

### I47 压缩

- **做什么**：按请求协商 gzip、zstd 和 brotli，流式压缩，跳过已压缩的类型。
- **完成标准**：测试覆盖协商和流式压缩；基准与 fit 对比。
- **涉及**：`akit-middleware`。
- **参考**：`akio-http` 的 `compress`、`akio-deflate`、`akio-zstd`、`akio-brotli`。

### I48 fit 的其余中间件

- **做什么**：fit 配置参考中列出、akit 尚未实现的每个中间件（CORS、请求 ID、IP 头等），各自带移植过来的测试。
- **完成标准**：兼容性门禁不再把任何中间件列为不支持。
- **涉及**：`akit-middleware`。
- **基于**：本迭代加进 akio 的 akio API 需求 A32（ADR-115）。除 `cache`（I67）和 `wasm`（I71）外，fit 有而 akit
  缺的只剩 `rewrite` 的 `regexp` 与 `replacement`，以及 `redirect` 的 `to` 中正则的分组。

### I49 WebSocket 代理

- **做什么**：双向背压的 WebSocket 代理，从 HTTP/1.1、HTTP/2 和 HTTP/3 入口到 HTTP/1.1 上游。
- **完成标准**：每个入口上带慢对端的端到端测试通过。
- **涉及**：`akit-proxy`。
- **参考**：`akio-ws`。
- **基于**：本迭代加进 akio 的 akio API 需求 A93 和 A94（ADR-113）。

### I50 gRPC 代理

- **做什么**：经 h2c 和 TLS 上游的 gRPC，传递 trailer 和 deadline，支持一元调用和流式调用。
- **完成标准**：针对 grpc-go 的端到端测试通过。
- **涉及**：`akit-proxy`。
- **拆分**：端到端测试是 fit 的测试，移植到 akio-grpc 的客户端和服务器上（fit 在 fib 的 gRPC 上
  运行它们）；针对 grpc-go 的运行需要 akit 的 CI 没有的 Go 工具链，留给一个互操作任务。

### I51 TCP 与 UDP 代理

- **做什么**：TCP 和 UDP 的四层路由，带联动背压和超时；监听器要求时使用 eBPF sockmap 中转。
- **完成标准**：带慢对端的测试通过；基准与 fit 和 nginx 的 `stream` 对比。
- **涉及**：`akit-l4`。
- **拆分**：eBPF sockmap 中转移到 I51b；基准由维护者的任务完成（规则 30）。

### I51b eBPF sockmap 中转

- **做什么**：在监听器要求、且内核与权限允许时，`tcp` 服务的连接在内核中拼接（akio 的 `ebpf::Relay`），否则
  退回 I51 的中转；空闲超时依据套接字的字节计数。
- **完成标准**：有一个字段开启它（fit 没有：作为 akit 的扩展，或先加进 fit）；在 Linux 6.15+ 上以 root 运行的
  测试覆盖拼接、回退和空闲超时。
- **涉及**：`akit-l4`、`akit-config`。
- **状态**：已完成（2026-11-25）：`listeners[].sockmap`，akit 的扩展字段
  （[决策 010](decisions/010-sockmap-relay.zh-CN.md)）；使用已锁定版本中 akio 的转发，在 akio API 需求 A170 之前通过文档中
  隐藏的 `Conn::inner` 访问；见进度日志。

### I52 SNI 分流与 PROXY protocol

- **做什么**：按 SNI 分流 TLS 而不终止；接收和发送 PROXY protocol v1/v2。依赖 akio A4 和 A5。
- **完成标准**：测试覆盖两者，包括被截断或伪造的 PROXY 头。
- **涉及**：`akit-l4`、`akit-listener`。
- **基于**：akio API 需求 A4 和 A5（ADR-095），以及本迭代加进 akio 的 A105（ADR-118：请求的对端取自 PROXY 头）。

### I53 一个端口多种协议

- **做什么**：由 akio-mux 把一个监听器拆分为 HTTP、TLS（终止或按 SNI 透传）、gRPC、SOCKS 和自定义协议，带嗅探
  上限和超时。
- **完成标准**：一个端口同时提供 HTTP/1.1、HTTP/2、按 SNI 的 TLS 透传和 SOCKS。
- **涉及**：`akit-listener`。
- **拆分**：共享端口上的 SOCKS 归入 I55，由它实现 fit 的正向代理（`socks` 与 `listeners[].socks`）并加上
  对应的 akio-mux 路由；I53 在一个端口上提供 HTTP/1.1、HTTP/2（h2c 和 h2）、HTTPS、按 SNI 的 TLS 透传和原始
  TCP，同 fit 不带 `socks` 时的 `sniff`。gRPC 属于 HTTP/2；自定义协议即没有 `server_names` 的服务所接管的连接。

### I54 上游 TLS

- **做什么**：到上游的 TLS，支持自定义 CA、客户端证书和覆盖 SNI。
- **完成标准**：测试覆盖每一项。
- **涉及**：`akit-upstream`。
- **基于**：本迭代加进 akio 的 akio API 需求 A108 和 A109（ADR-117）。

### I55 SOCKS 与 CONNECT

- **做什么**：基于 `akio-socks` 和 HTTP CONNECT 的正向代理，带认证和上限。
- **完成标准**：测试用慢对端覆盖两者；一个端口在 HTTP、TLS 和 `tcp` 旁同时提供 SOCKS（从 I53 转来：在
  akit-listener 的 `sniff` 中加一条 `Rule::Socks5`/`Rule::Socks4` 路由）。
- **涉及**：`akit-l4`、`akit-listener`、`akit-handler`（`forward_proxy`）、`akit-config`。
- **基于**：akio API 需求 A132 和 A133（ADR-119：隧道的上游连接建立后，按所请求的名字和两端地址检查；独立的
  连接超时；SOCKS 客户端取受信任 PROXY 头中的客户端），由本迭代加入 akio。
- **拆分**：“认证”按 fit 的做法：fit 的正向代理不认证任何人，而是按 `clients` 和目的地规则判定，akit 也一样。
  经 HTTP/2 和 HTTP/3 的 HTTP CONNECT（fit 的 `forward_proxy` 也支持隧道）随 akio API 需求 A134 实现
  （2026-11-25）；此前这样的 CONNECT 应答 501。

### I56 M3 退出

- **做什么**：每个中间件的一致性测试和基准；M3 报告。
- **完成标准**：路线图中 M3 的退出标准满足。
- **涉及**：`tests`、`bench`。
- **状态**：已完成（2026-11-25）：每条标准都满足；[M3 报告](../bench/results/m3-report-2026-11-25.zh-CN.md)
  包含各中间件与 fit 的基准对比（`bench/modules.sh`）和完整的基准套件运行。`crates/akit/tests/workers.rs`
  （fit 的 `test/workers`）检查没有模块阻塞 I/O 线程；新增的慢对端测试是 WebSocket 的慢上游和 UDP 的。没有
  akio 需求，没有决策记录。

## M4 平台集成

### I57 Provider 框架与生效配置

- **做什么**：provider 走同一条编译路径；fit 的合并规则；一个 Admin API 端点，展示生效配置及其各部分的来源。
- **完成标准**：测试覆盖合并和该端点。
- **涉及**：`akit-provider`、`akit-admin`。
- **fit**：决策 009。

### I58 Docker provider

- **做什么**：Docker label 作为配置，经 Unix 套接字上 akio 的 HTTP 客户端跟踪事件流。
- **完成标准**：Docker Compose 示例在服务启动和停止时随之路由。
- **涉及**：`akit-provider`。
- **fit**：决定 010。
- **状态**：已完成（2026-11-24），附带 akio API 需求 A135（ADR-120）；见进度日志。

### I59 Kubernetes Ingress

- **做什么**：akit 自己的 watch 客户端（经 HTTP/2 的 list 和 watch），把 Ingress 转为路由。
- **完成标准**：针对 kind 集群的测试按 Ingress 路由。
- **涉及**：`akit-provider`。
- **fit**：决策 011。
- **状态**：已完成（2026-11-24），没有 akio API 需求；测试与 fit 的一样针对一个假 API 服务器，经 HTTP/2
  和 HTTP/1.1，而非 kind 集群；见进度日志。

### I60 Kubernetes Gateway API 与一致性测试

- **做什么**：HTTPRoute、GRPCRoute、TLSRoute、TCPRoute、UDPRoute；一致性测试套件。
- **完成标准**：akit 通过 `GATEWAY-HTTP` profile。
- **涉及**：`akit-provider`。
- **fit**：决定 012。
- **状态**：已完成（2026-11-25），没有 akio 需求；移植了 fit 的转换、状态回写及其测试，与 fit 一样针对
  假 API 服务器；针对 kind 集群运行一致性测试套件的方法写在
  [tests/conformance](../tests/conformance/README.zh-CN.md) 中，留给维护者运行；见进度日志。

### I61 Consul 与 etcd

- **做什么**：来自 Consul（HTTP）和 etcd（与 fit 一样经 etcd 在客户端端口上提供的 JSON 网关访问其
  v3 API；最初计划基于 `akio-grpc`）的服务目录和 KV 配置。
- **完成标准**：针对两者的测试能跟踪变化。
- **涉及**：`akit-provider`。
- **fit**：决策 013。
- **状态**：done（2026-11-24），没有 akio 请求：两者都基于 akio 的 HTTP 客户端，Consul 用阻塞
  查询，etcd 用流式 watch；见进度日志。

### I62 共享证书存储

- **做什么**：证书存储在 Redis、etcd 或 Consul 中并带锁，让多个实例共享签发。
- **完成标准**：两个实例共享一张证书，只签发一次。
- **涉及**：`akit-tls`。
- **fit**：决策 014。
- **状态**：已完成（2026-11-25），没有 akio 请求：经 akio 的连接以 RESP2 访问 Redis，经 akio 的 HTTP
  客户端访问 etcd 和 Consul；挑战在 fit 的“挑战进行中”标记之后共享；见进度日志。

### I63 仪表盘

- **做什么**：由 akit 提供 fit 的只读仪表盘页面。
- **完成标准**：页面展示路由器、service、上游健康状态、证书和版本。
- **涉及**：`akit-admin`。
- **状态**：已完成（2026-11-24），无需 akio API 需求；见进度日志。

### I64 OpenTelemetry

- **做什么**：追踪，上下文传递到上游；OTLP 导出追踪和指标，批量处理且不在 I/O 线程上进行。
- **完成标准**：测试中追踪数据到达 collector。
- **涉及**：`akit-observe`。
- **状态**：done（2026-11-24），没有 akio 请求：fit 的 tracer 跑在 akio 的 HTTP 客户端上
  （[决策 007](decisions/007-opentelemetry.zh-CN.md)）；见进度日志。

### I65 打包

- **做什么**：Linux（amd64、arm64）的静态 musl 二进制和 macOS 构建；容器镜像；与 fit 相同的 Helm chart 和
  systemd unit。
- **完成标准**：发布流程产出它们并附校验和。
- **涉及**：`deploy`、CI。
- **状态**：done（2026-11-25），没有 akio 请求：用 `rust-lld` 链接的静态 musl 二进制
  （[决策 009](decisions/009-packaging.zh-CN.md)）；见进度日志。

### I66 M4 退出

- **做什么**：CI 中运行一致性测试和 Compose 示例；M4 报告。
- **完成标准**：路线图中 M4 的退出标准满足。
- **状态**：已完成（2026-11-25），没有改动 akit 代码，也没有 akio 需求：Gateway API 的
  `GATEWAY-HTTP` core 在 v1.3.0 上通过（通过 33，失败 0，跳过 0，
  [报告](../tests/conformance/reports/gateway-http-v1.3.0-2026-11-25.yaml)），Docker Compose 示例
  （[deploy/compose](../deploy/compose/README.zh-CN.md)）在其容器扩缩、停止和启动时按标签路由；两者都
  在 CI 中运行；[M4 报告](../bench/results/m4-report-2026-11-25.zh-CN.md)；见进度日志。

## M5 缓存与扩展

### I67 缓存 I：内存层与 RFC 9111

- **做什么**：`Cache-Control`、`Vary`、校验器、`Age`；有上限的内存层。
- **完成标准**：RFC 9111 测试通过。
- **涉及**：`akit-cache`。
- **状态**：已完成（2026-11-24），基于 akio API 需求 A111–A113（ADR-116）；见进度日志。

### I68 缓存 II：磁盘层、缓存锁、stale

- **做什么**：基于逻辑池的磁盘层；缓存锁，使一次未命中只回源一次；stale-while-revalidate 和 stale-if-error。
- **完成标准**：缓存击穿测试只产生一次上游请求；磁盘 I/O 从不在 I/O 线程上运行。
- **涉及**：`akit-cache`。
- **状态**：已完成（2026-11-24）；磁盘线程池直接基于 `akio-pool`，是 akio API 需求 A117 的权宜之计；见进度日志。

### I69 缓存 III：清除与标签

- **做什么**：经 Admin API 按键和标签清除。
- **完成标准**：测试覆盖两者。
- **涉及**：`akit-cache`、`akit-admin`。

### I70 `akit build`

- **做什么**：把用户的 crate 作为模块构建进 akit（生成一个注册它们的工作区），就像 `fit build` 对 Go 模块所做的；
  模块开发指南。
- **完成标准**：一个示例模块能构建进去并提供服务。
- **涉及**：`akit`、`docs`。

### I71 WASM I：运行时与 ABI

- **做什么**：选定运行时（决定说明：wasmi、wasmtime 或其他，对照 fit 的 wazero 测量）；fit 的 ABI；插件运行在自己
  的池上，带时间和内存上限。
- **完成标准**：fit 的改头插件无需改动即可运行，开销经过测量且有上限。
- **涉及**：`akit-plugin`。
- **fit**：决定 017、018。
- **状态**：已完成（2026-11-25），基于 akio 的 `akio-wasm`（ADR-125，W1–W4；akio API 需求 A165）和
  `Context::req_mut`（A33），见[决定 008](decisions/008-wasm-runtime.zh-CN.md)；开销留给基准测试任务（规则 30，见上表）；
  见进度日志。

### I72 WASM II：Proxy-Wasm 子集

- **做什么**：fit 支持的 Proxy-Wasm 子集。
- **完成标准**：移植过来的 fit Proxy-Wasm 测试通过。
- **涉及**：`akit-plugin`。
- **fit**：决定 018、023。
- **状态**：已完成（2026-11-25），基于 akio 的挂起响应头和响应体过滤器（ADR-128，akio API 需求 A166 和 A167），
  见[决定 011](decisions/011-proxy-wasm.zh-CN.md)；开销留给基准测试任务（规则 30，见上表）；见进度日志。

### I73 `akit adapt` 与 `akit fmt`

- **做什么**：.conf、YAML 和 JSON 之间的转换；与 fit 一致的格式化。
- **完成标准**：在 fit 的模板上往返转换无损。
- **涉及**：`akit-config`、`akit`。

### I74 nginx 与 Caddyfile 转换器

- **做什么**：针对 fit 所转换子集的转换器，列出跳过的内容。
- **完成标准**：fit 的转换器样例得到相同输出。
- **涉及**：`akit-config`。

### I75 FastCGI

- **做什么**：FastCGI handler，带流式 body、连接池和超时。
- **完成标准**：针对 php-fpm 的测试通过。
- **涉及**：`akit-handler`。
- **状态**：done（2026-11-24）；它的 PHP-FPM 测试（fit 的 `TestPHPFPM`）在 I76 中于基准测试主机上运行，对照
  `php:8.4-fpm` 镜像里 PHP 8.4.28 的 FPM，在 epoll 和 io_uring 上都通过。

### I76 M5 退出

- **做什么**：缓存与 nginx 的 `proxy_cache` 对比；WASM 开销；转换器样例；M5 报告。
- **完成标准**：路线图中 M5 的退出标准满足。
- **状态**：done（2026-11-25）。[M5 报告](../bench/results/m5-report-2026-11-25.zh-CN.md)，含 *akit 的位置*：每条标准都成立。
  缓存保存了 nginx 的 `proxy_cache` 保存的内容，速率是它的 1.5 到 6.2 倍，1,000 个客户端的击穿只取一次
  （`bench/stampede.sh`）；fit 的 `headers` 插件，两边同一个 `.wasm`，让 akit 每个请求多花 14.7 µs，有界，是 fit 的两倍
  （akio API 请求 A180）；fit 的 nginx 样例中的每条指令都被转换或列出（探测测试）。见进度日志。

## 1.0

### I77 Fuzz

- **做什么**：配置读取器、路由器、代理的头处理、PROXY protocol 和缓存的 fuzz 目标；CI 中跑固定时长，定时任务中
  跑得更久。
- **完成标准**：每个解析器都有目标，语料已提交。
- **涉及**：`fuzz/`（自成一个 workspace 的 cargo-fuzz 项目，每个解析器一个目标）、被 fuzz 的每个 crate 中的 `fuzzing`
  feature 和模块、`.github/workflows/ci.yml` 和 `fuzz.yml`。PROXY protocol 和 ClientHello 的解析器属于 akio-mux，在
  akio 中 fuzz；akit fuzz 它用这些解析器读到的内容。

### I78 配置参考与指南

- **做什么**：从代码生成的配置参考；入门；从 fit、nginx、Caddy 和 Traefik 迁移；全部中英双语。
- **完成标准**：模型中的每个字段都出现在参考中。
- **状态**：已完成（2026-11-26）。[配置参考](reference/config.zh-CN.md)的表格由 `tools/config-reference.py` 根据 fit 的 schema 和
  akit-config 读取的字段生成，并由 `reference_matches_akit_config` 与 `model.rs` 对应；[指南](guides/README.zh-CN.md)，其中包括从 fit、
  nginx、Caddy 和 Traefik 迁移；文档中的每个配置示例都由 `crates/akit/tests/docs_examples.rs` 交给 `akit validate` 检查。见开发进度的日志。

### I79 v1 冻结与兼容性

- **做什么**：配置 schema 和 Admin API 冻结为 v1，与 fit 的 v1 相同；一份说明哪些内容保持稳定的兼容性文档。
- **完成标准**：兼容性门禁在每个 fit 模板上通过，没有不支持的字段。
- **状态**：已完成（2026-11-26）。[政策](compatibility.zh-CN.md#v1哪些保持稳定)即 akit 所提供部分的 fit v1；v1 的范围冻结在
  `tests/compat/v1` 中，由 `tools/v1-freeze.py`（配置）和 `v1_surface_is_frozen`（Admin API）把关，任何未标记为兼容增加的改动都会使其
  失败（[决定 029](decisions/029-v1-freeze.zh-CN.md)）；fit 改名字段的旧名按 fit 的警告读取；开始读取 `limits.max_header_bytes`、
  `max_conns`、`max_write_queue_bytes` 和 `engine`（[决定 028](decisions/028-engine-on-akio.zh-CN.md)）。每个 fit 模板都通过且没有被拒绝的
  字段（6 个中 6 个，与之前相同）；fit-examples 中仍有字段被拒绝的网关示例等待 akio（I79b）；其余属于 1.0 之后的路线。见开发进度的日志。

### I79b 门禁仍拒绝的网关示例

- **做什么**：基于 akio 的跨连接预算（A214）支持 `limits.max_pending_bytes`，使 fit-examples 的 `limits` 和 `proxy` 有效；只有 `h2c`
  的监听器（A168），使 `grpc` 有效；`limits.max_conns` 只计入被接受的连接（A215）。
- **完成标准**：1.0 之后路线以外的 fit-examples 配置都不被拒绝（门禁报告只列出 `bittorrent`、`console`、`llm`、`logging`、媒体、
  `streaming`、`sync`、`tunnel` 和 `webrtc` 的示例），每个字段都用 `make v1-bless` 标记。

### I80 为安全审查做好准备

- **做什么**：威胁模型；`cargo audit` 和 `cargo deny` 无问题；外部审查的范围（TLS 处理、Admin API、插件沙箱）。
- **完成标准**：审查可以开始。
- **状态**：已完成（2026-11-26）。[威胁模型](security/threat-model.zh-CN.md)（资产、信任边界、每种攻击及应对它的代码和测试、15 项已知局限
  及其计划），[审查范围](security/review-scope.zh-CN.md)（TLS 处理、Admin API 与仪表盘、插件沙箱、带 I77 模糊测试目标的请求解析与头处理、
  sockmap 路径；akio 的内部和 akit 尚未实现的部分不在范围之内），以及 [SECURITY.zh-CN.md](../SECURITY.zh-CN.md)（报告方式、支持的版本）。
  `cargo deny check` 和 `cargo audit -D warnings` 无问题，由 `.github/workflows/security.yml` 在每次改动和每天运行，同时运行
  `tools/release-deps.py`，它让发布二进制只依赖 akio（[决定 030](decisions/030-supply-chain-checks.zh-CN.md)）。修复了按需 TLS 的
  `https://` ask URL：其客户端没有任何 CA。见开发进度的日志。

### I80b 安全加固

- **做什么**：[威胁模型的已知局限](security/threat-model.zh-CN.md#已知的局限)中属于 akit 的各项，以及外部审查陆续给出的发现：
  - L2：`GET /config`、`/versions/{n}` 和 `/config/effective` 对内联的密钥脱敏，像 fit 的控制台显示 `[redacted]` 那样。
  - L3：对组或其他用户可读的密钥和私钥文件发出警告；存储目录收紧到 0700，并在重命名后同步。
  - L4：向回环之外的地址明文发送令牌或密码时发出警告（ACME、Consul、etcd、Redis、经 `tcp://` 的 Docker、kubeconfig 的 `http://` 服务器
    或 `insecure-skip-tls-verify`）。
  - L6：正向代理的指南写明，除非 `deny` 列出，否则私有地址段可达，并且没有认证。
  - L7：参照 fit 的行为决定指向 `static` 根目录之外的符号链接。
  - L8：基于 akio-tls 的选项检查客户端证书的吊销（CRL）。
  - L9：WASM 模块的摘要，先向 fit 提议（[决定 002](decisions/002-config-compatibility.zh-CN.md)）。
  - L12：如果 fit 有相应字段，从中读取 akio 的头和消息体超时。
  - L13：在配置参考的压缩部分加入 BREACH 说明。
  - L14：测试不带令牌和带 `Origin` 时的 `POST /cache/purge`。
  - L15：请 akio 安排对 akio-crypto、akio-tls、akio-http 和 akio-wasm 的外部审查。
  其余各项在别处：L1 和 L5 在控制台路线，L6 的访问列表在访问列表路线，L10 随 akio A24，L11 随 I79b。
- **完成标准**：审查的每项发现都已修复，或写明理由记为接受；上面属于 akit 的每项局限都已解决，或移到将解决它的计划中。

## 1.0 之后

每条路线在开始时拆成迭代，依据 fit 中同一路线的迭代（编号见 fit 的
[开发计划](https://github.com/lesismal/fit/blob/main/docs/dev-plan.zh-CN.md)）。下表是计划的顺序；彼此没有依赖的
路线可以并行进行。

| 顺序 | 路线 | fit 的迭代 | akit 中的第一步 |
| --- | --- | --- | --- |
| 1 | 日志：文件、切割与保留 | I155–I157 | 已完成：[I81–I83](#日志文件切割与保留)，带时区的日志文件，按大小和时间切割，保留策略 |
| 2 | 访问控制列表 | I195 | akio 中的 accept 时过滤（A3）；全局和按作用域的列表 |
| 3 | 控制台与加密的 store | I104、I157 的页面 | 加密的 store、控制台页面（包括基于 I83 的 `/logs` 的“日志”页面）、登录 |
| 4 | 隧道与 LLM | I105–I107, I159–I162 | 基于 `akio-ws` 和 `akio-quic` 的 hub、node 和 proxy 角色；链路 token |
| 5 | WebRTC | I87, I108–I111 | 基于 `akio-turn` 的 TURN；基于 `akio-webrtc` 的 WHIP 和 WHEP |
| 6 | 流媒体与媒体认证 | I115–I117 | 基于 `akio-media` 的流媒体角色；`media_auth` |
| 7 | 媒体库与频道 | I131–I139 | MP4 读取器（akio 需求）；点播；频道；签名 URL |
| 8 | 集群管理 | I140–I150, I153 | 成员、概览、带健康门槛的发布 |
| 9 | AI 助手 | I124–I130, I152–I154 | 对话面板、工具、MCP 服务器 |
| 10 | BitTorrent | I196–I210 | 基于 `akio-bittorrent` 的客户端和 tracker 角色 |
| 11 | 数据同步 | I163–I194, I211–I222 | 先做流水线核心和检查点，再按 fit 的顺序做连接器 |
| 12 | Prefork 与 NUMA | fit 中已规划 | 取消（2026-11-25）：akit 不使用 prefork，akio 已删除 `akio-prefork`。不再计划。 |
| 13 | P2P 与 VPN | I84–I103 | 待 fit 的这条路线完成后 |

### 日志：文件、切割与保留

对应 fit 的 I155–I157（[fit 的决策 034](https://github.com/lesismal/fit/blob/main/docs/decisions/034-log-files.zh-CN.md)），
分三次迭代（[决策 033](decisions/033-log-files.zh-CN.md)）。fit I157 的工作台“日志”页面等控制台路线；它的 Admin API 在这里做。

### I81 日志文件：按大小和时间切割、保留、压缩

- **做什么**：决策 033。akit-observe 的文件写入端，对应 fit 的 `logs.Files` 和 `logs.File`：每个路径一个写入端，写该路径的
  所有日志共用；1 MiB 的队列由它自己的线程写盘，超出时拒绝并计数，从不等待。在一次写入会使文件超过 `max_bytes` 之前切割（大于
  它的缓冲独占一个文件），也在 `interval` 的下一个周期里的第一次写入时切割，周期从日志时区的午夜算起，以先到者为准；切割出的文件
  名为 `<path>.<yyyy-mm-ddThh-mm-ss.mmm>`（UTC），名字被占用时加 `-N`，压缩后加 `.gz`（akio-deflate，在文件集自己的线程上，
  队列 16 个；失败时保留未压缩的文件）。打开时、每次切割后和每十分钟执行保留策略：早于 `max_age` 的、超过 `max_files` 的、再从
  最旧的删到总量不超过 `max_total_bytes`；只删除切割生成的文件名，绝不删除正在写的文件。不经由文件名处的符号链接或第二个硬链接
  写、读或删除任何文件。按 fit 的 `config.Rotate` 读取 `rotate`（默认 1 GiB、720h、1 GiB，`compress` 开启；0 关闭该项限制；
  小于 1s 的 interval，以及超过 24h 又不是整天数的，拒绝）；`access_log.rotate`：访问日志的批次原样交给写入端；重载时就地改变
  访问日志文件的切割设置和时区。
- **完成标准**：文件在 `max_bytes` 处、在某时区的每个周期边界处切割（用假时钟），每一行完整地只出现在一个文件里；按时间、数量和
  总量从最旧的开始删除，不删正在写的文件和非写入端生成的文件名；gzip 解压后正是那些行；拒绝链接和硬链接；队列满时拒绝并计数；
  `akit run` 切割访问日志，重载改变它的保留策略；配置检查；参考文档、兼容性门禁和 v1 冻结（`access_log.rotate` 标为新增）。
- **涉及**：`akit-observe`（`files`）、`akit-config`（`log`）、`akit`（`run`）、`tests/compat`、`docs/reference`。
- **fit**：`logs/file.go`、`config/rotate.go`、`observe/accesslog.go`；I155、I156。
- **状态**：done（2026-11-26）。见进度日志。

### I82 akit 自身的日志：fit 的 `log`

- **做什么**：fit 的 `log`（`level`、`levels`、`output`、`format`、`time_zone`、`rotate`、`loggers` 及其自己的 `level`、
  `output`、`format` 和 `rotate`），对应 fit 的 `logs.Root`：fit 的日志器（`server`、`admin`、`tls`、`provider`、`telemetry`、
  `access_log`、`plugin`、`fastcgi`、`engine`，fit 的其他名字也接受），低于日志器级别的消息不做格式化，按日志时区以 fit 的
  `line`、`text` 或 `json` 写出；未设置输出时 debug 和 info 写标准输出，其余写标准错误；文件经由 I81 的写入端。akit 各 crate
  关于自身的消息（证书、provider、遥测、FastCGI 的 stderr、写入端的失败）经由各自的日志器；akio 的记录（akio-log）经由
  `engine`。与访问日志共用的文件是同一个写入端；fit 的检查：未知日志器、级别设置两次、同一文件两个 `rotate` 或两个时区、未知
  时区。`akit run` 对没有 `rotate` 的文件输出给出提示。
- **完成标准**：每个日志器的每种格式都写所选时区；级别在格式化之前过滤；日志器自己的文件会切割；访问日志和某个日志器写同一路径时
  共用一个写入端且每行完整；fit 的 `logging` 示例通过门禁；配置检查；参考文档。
- **涉及**：`akit-observe`（`logger`）、`akit-config`（`log`）、`akit`、原先写标准错误的各 crate、`tests/compat`、
  `docs/reference`。
- **fit**：`logs/logs.go`、`config/log.go`；I155、I156。
- **状态**：done（2026-11-26）。见进度日志。

### I83 重载时的日志文件，以及 Admin API 的 `/logs`

- **做什么**：重载就地改变 `log` 的级别、`time_zone` 以及每个文件的 `rotate`，对应 fit 的 `Root.SetConfig`；路径或格式的
  改变提示需要重启。Admin API 的 `GET /logs`（`log` 和访问日志的每个文件输出：路径、日志器、格式、时区、填入默认值的
  `rotate`、当前文件和切割出的文件及其大小和时间（有上限）、丢弃的行数和最后的错误）、`POST /logs/rotate` 和
  `POST /logs/cleanup`（`dry_run`）；不是运行中配置的输出路径一律拒绝；在写入端的线程上执行，从不在 I/O 线程上；标为 v1 新增。
- **完成标准**：重载无需重启即改变级别、时区和保留策略；路径或格式的改变给出提示；三个端点及其上限和拒绝；参考文档。
- **涉及**：`akit-observe`、`akit-admin`、`akit`、`tests/compat/v1`、`docs/reference`。
- **fit**：`admin/logs.go`、`logs.Root.SetConfig`；I155–I157。
- **状态**：done（2026-11-26）。见进度日志。本路线完成；fit 工作台的“日志”页面等控制台路线。
