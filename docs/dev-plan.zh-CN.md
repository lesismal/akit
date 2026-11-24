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

状态：`todo`、`doing` 或 `done`。下一个可用的迭代编号是 I81。

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
| I25 | M1 一致性测试与报告 | I09–I24 | todo |
| **M2** | | | |
| I26 | Admin API I：加载、读取、版本 | I22 | done |
| I27 | Admin API II：局部修改、回滚、认证 | I26 | done |
| I28 | 运行时的监听差量 | I26, A1 | done |
| I29 | 文件 provider | I26 | done |
| I30 | 证书管理与存储 | I09 | done |
| I31 | ACME I：账户、订单、HTTP-01 | I30 | done |
| I32 | ACME II：TLS-ALPN-01、续期、OCSP | I31 | done |
| I33 | 默认 HTTPS | I32 | done |
| I34 | 按需 TLS | I33, A2 | todo |
| I35 | 主动健康检查 | I20 | done |
| I36 | 负载均衡 v2 | I19 | done |
| I37 | Prometheus 指标 | I15 | done |
| I38 | M2 退出：频繁替换测试与 staging 证书 | I26–I37 | todo |
| **M3** | | | |
| I39 | 匹配器 v2 与表达式 | I11 | done |
| I40 | 重写、跳转与头处理 | I39 | done |
| I41 | 限流、并发与大小限制 | I39 | done |
| I42 | Basic 认证与 mTLS | I39 | done |
| I43 | ForwardAuth | I42 | done |
| I44 | JWT 与 JWKS | I42 | done |
| I45 | 重试、熔断、按路由超时 | I17 | done |
| I46 | 加权服务、镜像、故障转移 | I45 | todo |
| I47 | 压缩 | I16 | done |
| I48 | fit 的其余中间件 | I40 | todo |
| I49 | WebSocket 代理 | I17 | done |
| I50 | gRPC 代理 | I18 | todo |
| I51 | TCP 与 UDP 代理 | I05 | done |
| I51b | eBPF sockmap 中转 | I51 | todo |
| I52 | SNI 分流与 PROXY protocol | I51, A4, A5 | todo |
| I53 | 一个端口多种协议 | I52 | todo |
| I54 | 上游 TLS | I18 | todo |
| I55 | SOCKS 与 CONNECT | I53 | todo |
| I56 | M3 退出 | I39–I55 | todo |
| **M4** | | | |
| I57 | Provider 框架与生效配置 | I29 | todo |
| I58 | Docker provider | I57 | todo |
| I59 | Kubernetes Ingress | I57 | todo |
| I60 | Kubernetes Gateway API 与一致性测试 | I59 | todo |
| I61 | Consul 与 etcd | I57 | todo |
| I62 | 共享证书存储 | I30, I61 | todo |
| I63 | 仪表盘 | I37 | todo |
| I64 | OpenTelemetry | I37 | todo |
| I65 | 打包 | I06 | todo |
| I66 | M4 退出 | I57–I65 | todo |
| **M5** | | | |
| I67 | 缓存 I：内存层与 RFC 9111 | I17 | todo |
| I68 | 缓存 II：磁盘层、缓存锁、stale | I67 | todo |
| I69 | 缓存 III：清除与标签 | I68, I27 | todo |
| I70 | `akit build` | I03 | todo |
| I71 | WASM I：运行时与 ABI | I03 | todo |
| I72 | WASM II：Proxy-Wasm 子集 | I71 | todo |
| I73 | `akit adapt` 与 `akit fmt` | I23, I24 | todo |
| I74 | nginx 与 Caddyfile 转换器 | I73 | todo |
| I75 | FastCGI | I16 | todo |
| I76 | M5 退出 | I67–I75 | todo |
| **1.0** | | | |
| I77 | Fuzz | I56 | todo |
| I78 | 配置参考与指南 | I76 | todo |
| I79 | v1 冻结与兼容性 | I78 | todo |
| I80 | 为安全审查做好准备 | I77, I79 | todo |

## 基准与优化状态

[规则 24](rules.zh-CN.md) 要求做基准的每项功能（热路径：同轮 A/B；热路径之外的数据面：crate 的微基准）在这里都有一行；
控制面（配置读取、CLI、Admin API、证书、provider）没有。**已测**指 akit 在这项上的数据已测出并入库；**已优化**指做过
针对其开销的工作并有测量，所以只出现在基线里算“已测”，不算“已优化”。两列不都是“是”的行，都是留给压测任务的待办项
（[规则 30](rules.zh-CN.md)）。功能任务把它所在迭代的行加为“否”，并写明应当测什么；压测任务用日期、commit 和结果
更新这一行。

| 功能 | 迭代 | 已测 | 已优化 | 依据或剩余工作 |
| --- | --- | --- | --- | --- |
| 每个请求读取运行时图 | I04 | 是（2026-11-24，`c9d490a`，`crates/akit-runtime/benches/graph_read.rs`，数据见 [进度](progress.zh-CN.md)） | 部分 | 每线程缓存的读取方式是与 `Arc` 克隆和互斥锁对比测量后选定的（1 线程 4.06 ns，16 线程 8.78 ns）；此后没有剖析或调优。 |
| 监听器与 `respond`，HTTP/1.1 和 h2c；空闲连接 | I05 | 是（2026-11-24，`7c835ad`，`bench/results/baseline-2026-11-24`） | 否 | 只在第一份基线中：fixed 在 HTTP/1.1 和 h2c 上都排第一。剩余：两者的剖析；每个空闲 HTTP/1.1 连接的内存（1,068 字节，nginx 548）；h2c 空闲连接，以及 10 万连接（I25）。 |
| TLS 入口（TLS 上的 HTTP/1.1 和 h2） | I09 | 否 | 否（已实现，未测） | 每秒握手数，以及 TLS 上的 fixed，对比 fit 和 nginx。 |
| HTTP/3 入口 | I10 | 否 | 否（尚未实现） | h3 上的 fixed，对比 fit 和 Caddy。 |
| 路由 v1：按主机和路径查找 | I11 | 否 | 否（已实现，未测） | 查找的微基准（不分配内存），以及路由较多时 fixed 对比 I05 线性扫描的同轮 A/B。 |
| 静态文件 | I12、I13 | 否 | 否（已实现，未测） | static 场景，HTTP/1.1 和 h2c（基线中 akit 跳过）；预压缩文件和大文件的 `sendfile`。 |
| 上游连接池 | I14 | 否 | 否（已实现，未测） | 通过 proxy 场景测：连接复用，连接数受限时等待连接。 |
| 反向代理：HTTP/1.1、流式请求体、h2 和 h2c 上游 | I15–I18 | 否 | 否（已实现，未测量） | proxy 场景，HTTP/1.1 和 h2c（基线中 akit 跳过）；双向大请求体，及每个进行中请求占用的内存；带 trailer 的请求（I17 每个请求解析 `Trailer`）；h2c 上游（I18）：与 HTTP/1.1 上游对比每秒请求数和每请求 CPU，许多客户端复用少数连接，以及租用流时的按地址锁（`akit_upstream::h2`）；I17b 每个被代理请求的 `CancelToken` 和 `on_gone`；经 h2c 时（I18b）akio 的每流状态及其响应头与空闲定时器（每个 DATA 帧读一次时钟，每个请求一个定时任务），以及被暂停的下载与有节奏的上传，与 HTTP/1.1 对比。 |
| 负载均衡 v1 和 v2 | I19、I36 | 否 | 否（已实现，未测量） | 每种策略选择的微基准（fit 的 `BenchmarkPick` 和 `BenchmarkPickNew`：九种策略、哈希一个键、查找会话保持 cookie），1 线程和 16 线程；对比单个服务器的 proxy A/B；proxy 场景下 `round_robin` 对比 `p2c`、`header_hash` 和 `sticky_cookie` 的同轮 A/B（每个请求的键哈希，以及首个请求复制的 `Set-Cookie`）。 |
| 被动健康检查 | I20 | 否 | 否（已实现，未测） | 请求路径上记录结果的微基准。 |
| 主动健康检查 | I35 | 否 | 否（已实现，未测） | 不在热路径上（选择地址仍只读每个地址一个原子量）：繁忙引擎上探测的开销，每秒探测数千个地址（http 和 tcp），连同探测定时器的线程（akio A45 之前的临时方案）；proxy 场景下开关健康检查的同轮 A/B。 |
| 访问日志 | I21 | 否 | 否（已实现，未测） | fixed 和 proxy 开关日志的同轮 A/B（json，以及 text 或 line），写文件和写 `/dev/null`；默认 4 MiB 缓冲、磁盘慢时满负载下丢弃的行数；每个请求的开销（装箱的响应过滤器、分片锁、格式化）。 |
| 每次握手选择证书（读取运行时图的 resolver） | I30 | 否 | 否（已实现，未测） | 一个和十个证书时每秒完整 TLS 握手数，对比 I09 的 `sni_cert` 和 fit。 |
| 明文监听器上的 HTTP-01 检查 | I31 | 否 | 否（已实现，未测） | HTTP/1.1 和 h2c 上的 fixed，与 I31 之前的 main 做同轮 A/B（`akit run` 下每个请求一次前缀比较）。 |
| 带 TLS-ALPN-01 和 OCSP 装订的证书选择 | I32 | 否 | 否（已实现，未测） | 与 I31 对比每秒完整 TLS 握手数，有和没有装订的响应（读取 ALPN 列表，带响应者的文件证书还要读第二个 `Published` 映射）；带响应时握手的大小。 |
| Prometheus 指标 | I37 | 否 | 否（已实现，未测量） | fixed 和 proxy 场景下开关 `metrics` 的同轮 A/B（inline 路由的代价是一次线程局部读和三次原子加；代理的路由在 akio A29 之前还多一个装箱的应答过滤器），以及同时开启访问日志（两者共用一个钩子）；大量路由、64 个线程时的一次抓取（按线程的分片）。 |
| 匹配器 v2 与表达式 | I39 | 否 | 否（已实现，未测） | 带匹配器、表达式和路径正则的 `find_request` 微基准（不分配内存，`tests/no_alloc.rs`）；fixed 场景下对比 I39 基线的同轮 A/B（一条路由、没有匹配器：预期在噪声以内）。 |
| 重写、跳转与头处理 | I40 | 否 | 否（已实现，未测） | 在 proxy 场景上做同轮 A/B：带 `headers`（请求和响应修改）与 `rewrite` 的路由对比不带它们的同一路由；每个请求上响应过滤器（一个装箱闭包和一次 `Arc` 克隆）的开销，以及把请求字段放在上下文 local 中的开销（在 akio API 需求 A33 之前的临时方案）。 |
| 限流、并发与大小限制 | I41 | 否 | 否（已实现，未测） | 在 proxy 场景上做同轮 A/B：每种限制设得足够高、从不拒绝的路由对比不带它的同一路由：按 ip 和按头的 `rate_limit`（一把分片锁、一次 SipHash、一次 map 查找），1、1000 和 1000000 个客户端键、64 个线程（分片的争用），与 fit 的 `BenchmarkRate` 一样；`concurrency_limit`（同一共享缓存行上的两次原子操作，以及在 akio A48 之前的一个装箱响应过滤器和一次 `Arc` 克隆）；`body_limit`（每个请求只查一次字段）。 |
| Basic 认证与客户端证书 | I42 | 否 | 否（已实现，未测） | 在 proxy 场景上做同轮 A/B：带 `basic_auth` 的路由（已记住的密码：一次 base64 解码、一次 HMAC-SHA-256，以及在 akio A33 之前把请求字段改写进上下文本地值）和带 `client_cert` 与 `subject_header` 的路由（主题按线程缓存），对比不带它们的同一路由，覆盖 HTTP/1.1、h2 和 h3；密码的首次校验（一次挂起，到 akio 线程池再回来）在每秒几百个新用户时的开销，以及大量错误密码对已记住用户延迟的影响。 |
| 转发认证 | I43 | 否 | 否（已实现，未测） | 在 proxy 场景上做同轮 A/B：带 `forward_auth`、认证服务在本机且很快的路由，对比不带它的同一路由，覆盖 HTTP/1.1 和 h2：检查的开销（构造认证请求的字段、一次挂起、认证 upstream 的连接池上的一次交换、在应答所在的 I/O 线程上恢复）；成千上万个请求等待慢认证服务时的延迟和内存，以及此时快路由的延迟。 |
| JWT 与 JWKS | I44 | 否 | 否（已实现，未测） | proxy 场景上带 `jwt` 的路由与不带它的路由的同轮 A/B，经 HTTP/1.1 和 h2，按算法（HS256、EdDSA、ES256、RS256），令牌已在缓存中（一次 SHA-256 和一次每线程 map 查找，claims 字段在 akio A33 之前改写进上下文局部变量）和重新校验（大量不同令牌：在 I/O 线程上校验签名），同 fit 的 `BenchmarkJWT`；等待 JWK 集获取的请求的延迟，以及期间其他路由的延迟。 |
| 重试、熔断、按路由超时 | I45 | 否 | 否（已实现，未测量） | 在 proxy 场景上对带 `retry` 和带 `circuit_breaker` 的路由与不带它们的同一路由做同轮 A/B，上游健康：每个请求读取预算的开销（向满的预算存入不写内存），为重放保留的模板（复制出站字段和最多 64 KiB 的 body，一个装箱的 `Again`），熔断器装箱的响应过滤器及其在共享步计数上的两次原子加，64 个线程（计数的争用）；以及上游一半请求失败时：相对预算的每秒重试数，和熔断器的打开路径。 |
| 中间件（重试等） | I46–I48 | 否 | 否（尚未实现） | I56 对每个中间件做基准；每个中间件在 proxy 场景上的同轮 A/B。 |
| 压缩 | I47 | 否 | 否（已实现，未测量） | 按 I47 的要求，每种编码对比 fit：代理场景前面加 `compress`，完整的 JSON 消息体和服务端推送事件流，走 HTTP/1.1 和 h2，每种编码用 fit 的默认级别（每秒响应数、服务端 CPU、线上字节数）；过滤器对它不压缩的响应（图片）的开销，与不加中间件对比；同时保持的压缩流（1000 个）的内存；静态处理器的副本与 fit 的对比（内存中文件的 `compress`）。 |
| WebSocket 与 gRPC 代理 | I49、I50 | 否 | 否（I49 已实现，未测量；I50 尚未实现） | 对比 fit：经 akit 的 WebSocket 每秒消息数（小消息回显）与每秒字节数（大消息），来自 HTTP/1.1、h2 和 h3 客户端；每个打开的 WebSocket 的内存（空闲时，以及每个方向各有 64 KiB 待发时）；每秒握手数（生成密钥、上游连接离开连接池）；I50：每秒 gRPC 调用数。 |
| TCP 和 UDP 代理、SNI 路由、单端口多协议 | I51–I53 | 否 | 否（I51 已实现，未测；I52、I53 尚未实现） | 按 I51 的要求对比 fit 和 nginx 的 `stream`：TCP 吞吐（fit 的 `BenchmarkTCPProxy`，每次回显 256 KB，直连与经 akit 对比）、每秒连接数、每个空闲连接的内存；UDP 每秒数据报数与同时的会话数；10,000 个连接时每个服务在 akit 定时线程上的空闲扫描（akio A78 之前的权宜之计）；共享端口上协议嗅探的开销（I53）。 |
| 响应缓存 | I67、I68 | 否 | 否（尚未实现） | 内存和磁盘命中，对比 fit。 |
| WASM 插件 | I71、I72 | 否 | 否（尚未实现） | 空插件对比无插件的 A/B。 |

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

## M2 动态配置与 TLS

### I26 Admin API I：加载、读取、版本

- **做什么**：监听 localhost 或 Unix 套接字的 Admin API：加载整份配置、读取配置、版本和 ETag，路径和请求体与 fit
  一致。
- **完成标准**：移植过来的 fit 对这些路径的 Admin API 测试通过。
- **涉及**：`akit-admin`。
- **fit**：`admin`，决定 005。
- **已完成**（决定 006）：运行在自己的 akio 引擎上，加载由控制线程经 I22 的重载应用；Unix 套接字要等 akio A23，
  拒绝请求体后关闭连接要等 A24。

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
- **涉及**：`akit-upstream`。

### I47 压缩

- **做什么**：按请求协商 gzip、zstd 和 brotli，流式压缩，跳过已压缩的类型。
- **完成标准**：测试覆盖协商和流式压缩；基准与 fit 对比。
- **涉及**：`akit-middleware`。
- **参考**：`akio-http` 的 `compress`、`akio-deflate`、`akio-zstd`、`akio-brotli`。

### I48 fit 的其余中间件

- **做什么**：fit 配置参考中列出、akit 尚未实现的每个中间件（CORS、请求 ID、IP 头等），各自带移植过来的测试。
- **完成标准**：兼容性门禁不再把任何中间件列为不支持。
- **涉及**：`akit-middleware`。

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

### I52 SNI 分流与 PROXY protocol

- **做什么**：按 SNI 分流 TLS 而不终止；接收和发送 PROXY protocol v1/v2。依赖 akio A4 和 A5。
- **完成标准**：测试覆盖两者，包括被截断或伪造的 PROXY 头。
- **涉及**：`akit-l4`、`akit-listener`。

### I53 一个端口多种协议

- **做什么**：由 akio-mux 把一个监听器拆分为 HTTP、TLS（终止或按 SNI 透传）、gRPC、SOCKS 和自定义协议，带嗅探
  上限和超时。
- **完成标准**：一个端口同时提供 HTTP/1.1、HTTP/2、按 SNI 的 TLS 透传和 SOCKS。
- **涉及**：`akit-listener`。

### I54 上游 TLS

- **做什么**：到上游的 TLS，支持自定义 CA、客户端证书和覆盖 SNI。
- **完成标准**：测试覆盖每一项。
- **涉及**：`akit-upstream`。

### I55 SOCKS 与 CONNECT

- **做什么**：基于 `akio-socks` 和 HTTP CONNECT 的正向代理，带认证和上限。
- **完成标准**：测试用慢对端覆盖两者。
- **涉及**：`akit-l4`。

### I56 M3 退出

- **做什么**：每个中间件的一致性测试和基准；M3 报告。
- **完成标准**：路线图中 M3 的退出标准满足。
- **涉及**：`tests`、`bench`。

## M4 平台集成

### I57 Provider 框架与生效配置

- **做什么**：provider 走同一条编译路径；fit 的合并规则；一个 Admin API 端点，展示生效配置及其各部分的来源。
- **完成标准**：测试覆盖合并和该端点。
- **涉及**：`akit-provider`、`akit-admin`。

### I58 Docker provider

- **做什么**：Docker label 作为配置，经 Unix 套接字上 akio 的 HTTP 客户端跟踪事件流。
- **完成标准**：Docker Compose 示例在服务启动和停止时随之路由。
- **涉及**：`akit-provider`。

### I59 Kubernetes Ingress

- **做什么**：akit 自己的 watch 客户端（经 HTTP/2 的 list 和 watch），把 Ingress 转为路由。
- **完成标准**：针对 kind 集群的测试按 Ingress 路由。
- **涉及**：`akit-provider`。

### I60 Kubernetes Gateway API 与一致性测试

- **做什么**：HTTPRoute、GRPCRoute、TLSRoute、TCPRoute、UDPRoute；一致性测试套件。
- **完成标准**：akit 通过 `GATEWAY-HTTP` profile。
- **涉及**：`akit-provider`。
- **fit**：决定 012。

### I61 Consul 与 etcd

- **做什么**：来自 Consul（HTTP）和 etcd（其 gRPC API，基于 `akio-grpc`）的服务目录和 KV 配置。
- **完成标准**：针对两者的测试能跟踪变化。
- **涉及**：`akit-provider`。

### I62 共享证书存储

- **做什么**：证书存储在 Redis、etcd 或 Consul 中并带锁，让多个实例共享签发。
- **完成标准**：两个实例共享一张证书，只签发一次。
- **涉及**：`akit-tls`。

### I63 仪表盘

- **做什么**：由 akit 提供 fit 的只读仪表盘页面。
- **完成标准**：页面展示路由器、service、上游健康状态、证书和版本。
- **涉及**：`akit-admin`。

### I64 OpenTelemetry

- **做什么**：追踪，上下文传递到上游；OTLP 导出追踪和指标，批量处理且不在 I/O 线程上进行。
- **完成标准**：测试中追踪数据到达 collector。
- **涉及**：`akit-observe`。

### I65 打包

- **做什么**：Linux（amd64、arm64）的静态 musl 二进制和 macOS 构建；容器镜像；与 fit 相同的 Helm chart 和
  systemd unit。
- **完成标准**：发布流程产出它们并附校验和。
- **涉及**：`deploy`、CI。

### I66 M4 退出

- **做什么**：CI 中运行一致性测试和 Compose 示例；M4 报告。
- **完成标准**：路线图中 M4 的退出标准满足。

## M5 缓存与扩展

### I67 缓存 I：内存层与 RFC 9111

- **做什么**：`Cache-Control`、`Vary`、校验器、`Age`；有上限的内存层。
- **完成标准**：RFC 9111 测试通过。
- **涉及**：`akit-cache`。

### I68 缓存 II：磁盘层、缓存锁、stale

- **做什么**：基于逻辑池的磁盘层；缓存锁，使一次未命中只回源一次；stale-while-revalidate 和 stale-if-error。
- **完成标准**：缓存击穿测试只产生一次上游请求；磁盘 I/O 从不在 I/O 线程上运行。
- **涉及**：`akit-cache`。

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

### I72 WASM II：Proxy-Wasm 子集

- **做什么**：fit 支持的 Proxy-Wasm 子集。
- **完成标准**：移植过来的 fit Proxy-Wasm 测试通过。
- **涉及**：`akit-plugin`。

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

### I76 M5 退出

- **做什么**：缓存与 nginx 的 `proxy_cache` 对比；WASM 开销；转换器样例；M5 报告。
- **完成标准**：路线图中 M5 的退出标准满足。

## 1.0

### I77 Fuzz

- **做什么**：配置读取器、路由器、代理的头处理、PROXY protocol 和缓存的 fuzz 目标；CI 中跑固定时长，定时任务中
  跑得更久。
- **完成标准**：每个解析器都有目标，语料已提交。

### I78 配置参考与指南

- **做什么**：从代码生成的配置参考；入门；从 fit、nginx、Caddy 和 Traefik 迁移；全部中英双语。
- **完成标准**：模型中的每个字段都出现在参考中。

### I79 v1 冻结与兼容性

- **做什么**：配置 schema 和 Admin API 冻结为 v1，与 fit 的 v1 相同；一份说明哪些内容保持稳定的兼容性文档。
- **完成标准**：兼容性门禁在每个 fit 模板上通过，没有不支持的字段。

### I80 为安全审查做好准备

- **做什么**：威胁模型；`cargo audit` 和 `cargo deny` 无问题；外部审查的范围（TLS 处理、Admin API、插件沙箱）。
- **完成标准**：审查可以开始。

## 1.0 之后

每条路线在开始时拆成迭代，依据 fit 中同一路线的迭代（编号见 fit 的
[开发计划](https://github.com/lesismal/fit/blob/main/docs/dev-plan.zh-CN.md)）。下表是计划的顺序；彼此没有依赖的
路线可以并行进行。

| 顺序 | 路线 | fit 的迭代 | akit 中的第一步 |
| --- | --- | --- | --- |
| 1 | 日志：文件、切割与保留 | I155–I157 | 带时区的日志文件，按大小和时间切割，保留策略 |
| 2 | 访问控制列表 | I195 | akio 中的 accept 时过滤（A3）；全局和按作用域的列表 |
| 3 | 控制台与加密的 store | I104 | 加密的 store、控制台页面、登录 |
| 4 | 隧道与 LLM | I105–I107, I159–I162 | 基于 `akio-ws` 和 `akio-quic` 的 hub、node 和 proxy 角色；链路 token |
| 5 | WebRTC | I87, I108–I111 | 基于 `akio-turn` 的 TURN；基于 `akio-webrtc` 的 WHIP 和 WHEP |
| 6 | 流媒体与媒体认证 | I115–I117 | 基于 `akio-media` 的流媒体角色；`media_auth` |
| 7 | 媒体库与频道 | I131–I139 | MP4 读取器（akio 需求）；点播；频道；签名 URL |
| 8 | 集群管理 | I140–I150, I153 | 成员、概览、带健康门槛的发布 |
| 9 | AI 助手 | I124–I130, I152–I154 | 对话面板、工具、MCP 服务器 |
| 10 | BitTorrent | I196–I210 | 基于 `akio-bittorrent` 的客户端和 tracker 角色 |
| 11 | 数据同步 | I163–I194, I211–I222 | 先做流水线核心和检查点，再按 fit 的顺序做连接器 |
| 12 | Prefork 与 NUMA | fit 中已规划 | 带 CPU 放置的 `akio-prefork`；默认开启前先测量 |
| 13 | P2P 与 VPN | I84–I103 | 待 fit 的这条路线完成后 |
