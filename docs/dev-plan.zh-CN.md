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
| `akit-admin` | Admin API、控制台及其登录（I89–I91）、文件 provider | `admin`、`app/console`、`login`、`app/workbench` |
| `akit-provider` | Docker、Kubernetes、Consul、etcd | `provider` |
| `akit-observe` | 日志、指标、追踪 | `observe`、`logs` |
| `akit-cache` | 响应缓存 | `handler/cache` |
| `akit-plugin` | WASM 插件 | `plugin` |
| `akit-access` | 访问控制列表：编译成区间的允许和拒绝列表、策略（I85） | `access` |
| `akit-roles` | 网关之外的角色：链路 token（I99）、运行它们的宿主（I100） | `app`、`internal/linktoken` |
| `akit-webrtc` | WebRTC 角色：TURN 服务器、WHIP 和 WHEP、媒体代理（I110–I114） | `webrtc` |
| `akit-cluster` | 集群管理：管理其他 akit 的管理端、其成员及其状态（I132） | `cluster` |
| `tests/e2e` | 可执行文件的端到端测试 | `test/e2e` |
| `tests/compat` | fit 配置兼容性门禁：固定夹具、字段清单 | `templates`、fit-examples |
| `bench` | 基准框架、场景和报告 | `bench` |

## 索引

状态：`todo`、`doing` 或 `done`。下一个可用的迭代编号是 I120（为日志路线和访问控制列表路线预留的 I84 和 I88 未使用；I95 和 I96 为控制台路线预留；I97 改名了 fit 的对外名字；I98–I109、I102b、I102c、I102d、I103b、I109b、I109c 和 I109d 是隧道与 LLM 路线；I110–I114 是 WebRTC 路线，I115–I119 为它预留）。

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
| I80b | 安全加固：威胁模型的局限与审查的发现 | I80、审查 | doing |
| **1.0 之后** | | | |
| I81 | 日志文件：按大小和时间切割、保留、压缩；`access_log.rotate` | I21 | done |
| I82 | akit 自身的日志：fit 的 `log`，其日志器、格式、文件与时区 | I81 | done |
| I83 | 重载时的日志文件，以及 Admin API 的 `/logs` | I82、I27 | done |
| **1.0 之后：访问控制列表** | | | |
| I85 | 访问控制列表 I：列表、监听器和路由 | I79、akio A3 | done |
| I86 | 访问控制列表 II：服务与正向代理 | I85 | done |
| I87 | 访问控制列表 III：Admin API 与配置参考 | I85 | done |
| **1.0 之后：控制台与加密的 store** | | | |
| I89 | 控制台 I：`admin.console` 及其登录：用户、锁定、会话；`akit hash-password` | I87 | done |
| I90 | 控制台 II：控制台的服务器：TLS、访问控制、CSRF 与来源、进程内的 Admin API | I89, I27 | done |
| I91 | 控制台 III：页面：fit 的工作台、schema、原文编辑器和导入导出、“日志”页面 | I90, I83 | done |
| I92 | 非回环地址上带 TLS 的 Admin API（L1） | I27 | done |
| I93 | 加密的 store I：`fitstore` 格式、`admin.store`、`akit store export` 和 `import` | I90, akio A230, A231 | done |
| I93b | 加密的 store II：`akit run` 把配置保存在其中；控制台的密码修改；`/config/export` 和 `/config/import` 的 `store` 格式 | I93 | done |
| I93c | 加密的 store III：`akit run -admin`、配置文件旁边的 store | I93b | done |
| I94 | 文件变化时重新读取 Admin API 和控制台的证书 | I92 | done |
| **1.0 之后：akit 的名字** | | | |
| I97 | akit 的名字：对外凡写 "fit" 的名字都改为 "akit"（决定 037） | I91, I92 | done |
| **1.0 之后：隧道与 LLM** | | | |
| I98 | 隧道与 LLM I：配置中的 `tunnel` 和 `llm` | I79 | done |
| I99 | 链路 token：`akitt1.` token、证明、登记表和管理 | I98 | done |
| I100 | `akit run` 中的角色：在监听器的引擎上启动、迁移和停止；一个地址，一个服务器；Admin API 的 `/tokens` 和 `akit token` | I99, I27 | done |
| I101 | 隧道 I：TCP 上的链路、注册和隧道 | I99, I100 | done |
| I102 | 隧道 II：TLS 链路；pin；保存 hub 的证书 | I101 | done |
| I102b | 隧道 II b：QUIC 链路 | I102 | done |
| I102c | 隧道 II c：WebSocket 链路 | I102b | done |
| I102d | 隧道 II d：hub 的 QUIC 握手限制；监听器的证书文件变化时重新读取 | I102c | done |
| I103 | 隧道 III：hub 的调用和状态、token 过期 | I102c | done |
| I103b | 隧道 III b：控制台套接字和 web 代理 | I103 | done |
| I104 | LLM I：mesh：帧、会话、带信用的流、握手、hello、连接池 | I99, I100 | done |
| I105 | LLM II：node：模型、检查、双向流式转发到上游的调用 | I104 | done |
| I106 | LLM III：hub：node 表、模型、选择和转发 | I105 | done |
| I106b | LLM III b：hub 供页面使用的调用（`/hub/nodes`、`/hub/node/call`、`/hub/node/stream`）、node 列表的字节上限、路由计数（`/mesh/status`） | I106 | done |
| I107 | LLM IV：proxy：agent 的 API、API key、路由、错误 | I106 | done |
| I108 | LLM V：node 的管理：硬件、GPU、固定动作和作业 | I105 | done |
| I109 | 角色的页面：其控制台和方法，写回的编辑；hub 的页面（拆出 I109b、I109c） | I103, I107, I91 | done |
| I109b | 隧道 node 和 proxy 的页面：其状态、hub、服务、转发和 meta | I109 | done |
| I109c | LLM 角色的页面：`/api/me`、其方法（`/api/rpc`）与流（`/api/stream`，Server-Sent Events） | I109, I106b, I108 | done |
| I109d | LLM 角色的页面调用而角色尚缺的方法：`/mesh/*`（dial、undial、kick、listen、limits、tokens）、proxy 的方法（`/proxy/*`：其 API、key、模型、node、profile、统计）；身份的 `http_url` | I109c | done |
| **1.0 之后：WebRTC** | | | |
| I110 | WebRTC I：配置中的 `webrtc`：`webrtc.turn`、`webrtc.media`、`webrtc.proxy`；处理器 `whip`、`whep` 和 `webrtc_proxy` | I79, I100 | done |
| I111 | WebRTC II：基于 akio-turn 的 TURN 服务器，经 UDP、TCP 和 TLS | I110 | done |
| I112 | WebRTC III：基于 akio 钩子的 TURN 服务器：端口范围、带宽、事件、就地修改的限制、排空 | I111, akio A256–A258 | done |
| I113 | WebRTC IV：WHIP 和 WHEP，基于 akio-webrtc `RtcServer` 的 SFU | I110 | done |
| I109d | LLM 角色的页面调用而角色尚缺的方法：`/mesh/*`（dial、undial、kick、listen、limits、tokens）、proxy 的方法（`/proxy/*`：其 API、key、模型、node、profile、统计）；身份的 `http_url` | I109c | todo |
| **1.0 之后：流媒体与媒体认证** | | | |
| I120 | 流媒体与媒体认证 I：配置中的 `media_auth` 和 `streaming` | I79 | done |
| I121 | 媒体认证 II：策略、HTTP 请求的检查、`akit media-token` | I120 | done |
| I122 | 流媒体 I：角色的媒体中心及其 RTMP、RTSP 和 SRT 监听器 | I121, I100 | todo |
| I123 | 流媒体 II：`httpflv` 和 `hls` | I122 | todo |
| I124 | 流媒体 III：`streaming.webrtc`、`stream_whip` 和 `stream_whep` | I123 | todo |
| I125 | 媒体页面：`media_pages`、其登录和房间 API | I121、WebRTC 路线的 `whip` 和 `whep` | todo |
| I126 | `whip`、`whep` 和 `webrtc_proxy` 上的媒体认证 | I121、WebRTC 路线的处理器 | todo |
| I114 | WebRTC V：代理的 WHIP 和 WHEP：`webrtc_proxy` 和 `webrtc.proxy` | I113, I14 | done |
| I150 | BitTorrent I：配置中的 `bittorrent.client` 和 `bittorrent.tracker`，fit 的字段、检查和冲突；后续迭代才运行的内容以"尚不支持"拒绝 | I98 | todo |
| I150 | BitTorrent I：配置中的 `bittorrent.client` 和 `bittorrent.tracker`，fit 的字段、检查和冲突；后续迭代才运行的内容以"尚不支持"拒绝 | I98 | done |
| I151 | BitTorrent II：基于 akio-bittorrent 的 `TrackerServer` 的 tracker 角色和 `bt_tracker`：HTTP 和 UDP、允许列表、访问控制、上限、统计、其控制台 | I150, I100 | done |
| I152 | BitTorrent III：基于 akio-bittorrent 的 `Client` 的客户端角色：`data_dir` 中的状态、不经校验恢复、原地或以新客户端重载、其 API 和控制台 | I150, I151 | done |
| I153 | BitTorrent IV：通过 URL 和 `watch_dir` 添加、分类、队列、磁盘检查 | I152 | todo |
| I154 | BitTorrent V：`alt_speed` 时间表、单个种子的限制、客户端的 `access` 和连接限制（akio A284） | I152 | todo |
| I155 | BitTorrent VI：`bt_stream`，边下载边播放文件 | I152 | todo |
| I156 | BitTorrent VII：把目录发布为种子，`bt_publish` | I152 | todo |
| I157 | BitTorrent VIII：Admin API 的 `/bittorrent/...`、事件、`akit bt`、指标 | I153 | todo |
| I158 | BitTorrent IX：客户端和 tracker 的页面 | I157, I109 | todo |
| I159 | BitTorrent X：RSS 自动下载、实例共享的 tracker 存储、私有 tracker 的 passkey | I153, I151 | todo |
| **1.0 之后：集群管理** | | | |
| I130 | 集群 I：配置中的 `admin.node_id`、`admin.cluster` 和 `admin.cluster_member` | I93c | done |
| I131 | 集群 II：成员需要什么：`GET /node`、保存节点 ID、`GET /schema` | I130 | done |
| I132 | 集群 III：管理端（`akit-cluster`）：存储中的成员和凭据，fit 的文档；轮询器；`/api/m/cluster/nodes`、`/test`、`/state` | I131 | done |
| I133 | 集群 IV：集群页面：概览、节点和节点页面；`docs/guides/cluster.md` | I132 | done |
| I134 | 集群 V：从管理端打开成员自己的页面（`/api/n/<member>/...`） | I133 | done |
| I135 | 集群 VI：分组和对多个成员的操作 | I133 | done |
| I136 | 集群 VII：模板；`/node/managed` | I135 | done |
| I137 | 集群 VIII：带健康门槛的发布 | I136 | done |
| I138 | 集群 IX：漂移和告警；`/config/check` | I137 | done |
| I139 | 集群 X：用户、角色和审计日志 | I137 | done |
| I139b | 集群 XI：NAT 之后主动外连的成员 | I132 | todo |
| I139c | 集群 XII：跨成员的指标、日志和证书；`akit cluster` | I138 | todo |

状态：`todo`、`doing` 或 `done`。下一个可用的迭代编号是 I127（为日志路线和访问控制列表路线预留的 I84 和 I88 未使用；I95 和 I96 为控制台路线预留；I97 改名了 fit 的对外名字；I98–I109、I102b、I102c、I102d、I103b、I109b、I109c 和 I109d 是隧道与 LLM 路线；I110–I114 是 WebRTC 路线，I115–I119 为它预留；I120–I126 是流媒体与媒体认证路线）。
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
| LLM 角色：hub、node、proxy | I105–I107 | 否 | 否（已实现，未测） | 对比 fit 的角色（fit 对 fit、akit 对 akit、以及混合），以 OpenAI 兼容的桩作上游：经 proxy → hub → node → 上游每秒的调用数，小正文和 3 MiB 正文（超过 1 MiB 时流式传输）；流式回答的首字节与各事件的延迟；256 个调用同时进行；hub 每转发一个调用的 CPU；每个角色上每个进行中调用的内存；一个慢 agent 只压住自己的调用。 |
| LLM mesh | I104 | 否 | 否（已实现，未测） | 对比 fit 的 mesh（fit 对 fit、akit 对 akit、以及混合）：一条链路上和一组链路上每秒的调用数；一条链路上 256 KiB 窗口下单个流每秒的字节数，以及 64 个流同时；一个慢读者只压住自己的流而链路上其他流照常进行（窗口，以及链路的 64 KiB 高水位及其暂停的读取）；1 KiB 以上帧的 gzip 每帧 CPU；每条空闲链路和每个流的内存。 |
| 隧道：hub、node、proxy | I101–I103（I102b、I102c、I103b） | 否 | 否（已实现，未测） | 对比 fit 的隧道（fit 的 hub 配 fit 的 node 和 proxy、akit 的三者、以及混合）：一条隧道、以及一条链路上 64 条隧道同时的每秒字节数，明文、`body_password` 密封以及经 TLS（I102）、QUIC（I102b：流的发送缓冲和 wire 的溢出队列，合起来是 `LINK_QUEUE`）和 WebSocket（I102c）；每秒打开的隧道数（proxy 的 accept、经 hub 的 `tunnel/open`、node 的拨号）；hub 转发每 GB 的 CPU；每条空闲隧道和每条链路的内存；一个慢服务压住一条隧道而链路上其他隧道照常进行（只由窗口调节链路，其队列上限为 fit 的 `LinkQueue`，A234）；I103b：经 web 代理和 hub 控制台套接字的每秒字节数（双向密封，每条消息 64 KiB），对比 fit，以及 hub 每 GB 的 CPU。 |
| WebRTC：TURN、WHIP 和 WHEP、代理 | I111–I114 | 否 | 否（尚未实现） | 与 fit 的角色对比（fit 的 TURN 服务器和媒体服务器对比 akit 的，使用相同的客户端）：经一个 allocation 和同时 1,000 个 allocation 中继的 TURN 数据报数和每秒字节数，经 channel 和经 indication，基于 UDP、TCP 和 TLS，并列 coturn；每秒建立的 allocation（校验凭据、绑定中继）；每 GB 中继的 CPU 和每个空闲 allocation 的内存。WHIP 和 WHEP：一个发布者 4 Mb/s 的视频转发给 1、100 和 256 个观看者（每个观看者的 CPU、转发延迟、丢弃的包），每秒应答的 PeerConnection，每个会话的内存；一个慢观看者不拖住其他人。代理：经其端口每秒中继的数据报，每秒应答的会话，每个中继的内存。 |
| 流媒体角色与媒体认证 | I121–I124 | 否 | 否（已实现，未测） | 对比 fit 的角色以及 SRS 或 MediaMTX：一路通过 RTMP 推送的流（ffmpeg `-re`，H.264 和 AAC，4 Mbit/s）由 1、100 和 1,000 个播放器通过 RTMP、HTTP-FLV、HLS（fMP4 和 MPEG-TS）、RTSP 和 SRT 播放：CPU、每个播放器的内存、出口吞吐、为慢播放器丢弃的包、首帧时间；每秒推流和播放数（握手和认证检查）；每次推流和每个 HLS 分片的一次 `media_auth` 检查，分别用静态 token、签名 token 和已记住的 bcrypt 用户。 |
| 正向代理：SOCKS 与 HTTP CONNECT | I55 | 否 | 否（已实现，未测） | 对比 fit（原始吞吐另对比 nginx 的 `stream`）：经 SOCKS5 隧道和 HTTP/1.1 CONNECT 隧道的每秒字节数（每次回显 256 KB，同 fit 的 `BenchmarkTCPProxy`），每秒建立的隧道数（握手、规则检查、在线程池上解析名字以及 `connected` 检查），每个空闲隧道的内存；共享端口上 SOCKS 的开销（akio-mux 的问候规则与 HTTP、TLS 并列）对比只有 HTTP；10,000 条隧道时 akio-socks 看门狗线程的开销（ADR-119 在其上增加了连接超时）。 |
| 响应缓存 | I67、I68、I69 | 部分（2026-11-25，I76，`346ad8b`：基准框架的 `cache-hot` 和 `cache-zipf` 对照 nginx 和 fit，以及 1,000 个客户端的击穿，见 [M5 报告](../bench/results/m5-report-2026-11-25.zh-CN.md)） | 否 | I76 已测：经基准框架的内存命中（4 KiB；akit 第一，HTTP/1.1 上是 nginx 的 1.5 倍、h2c 上 6.2 倍，100,000 个路径时 1.75 倍，领先 fit 12% 到 23%），以及 1,000 个客户端请求一个冷键时只取一次源站。剩下的，对比 fit，按 I67 的实现：内存命中（4 KiB 与 1 MiB 消息体，HTTP/1.1 与 h2），经代理保存的未命中（旁路复制的流式消息体）对比仅代理，缓存直接放行的请求（POST、`no-store` 响应）对比无中间件的开销，大量键与单个热点键下分片锁的表现。I68：从磁盘读回的命中（64 KiB 与 1 MiB，大量并发下磁盘线程池的线程与队列），未命中率高到填满 `MAX_PENDING_WRITES` 时的磁盘写入，大量客户端对同一个键的惊群，开启锁与 `disable_lock` 两种情况（等待者的挂起与恢复、每次获取一个定时器），以及 stale-while-revalidate 的后台获取。akio ADR-116 的旁路：有旁路的流对比无旁路的流。I69：带标签响应（0 与 32 个标签）对比无标签响应的保存开销，以及服务期间对大 zone 按前缀和按标签清除（分片锁与磁盘索引锁的持有时间）。 |
| FastCGI | I75 | 否 | 否（已实现，未测量） | 对比 fit 和 nginx 的 `fastcgi_pass`，经 TCP 和 Unix 域套接字连接 PHP-FPM（或立即应答的 FastCGI 应用）：小页面的每秒请求数与延迟，HTTP/1.1 与 h2；1 MiB 上传与 16 MiB 下载（每个进行中请求的内存，双向 64 KiB 水位线）；连接复用对比每个请求一个连接；客户端多于 `max_conns` 时连接池的队列；`try_files` 的开销（每个候选在 akio 逻辑线程池上一次 `stat`）对比不带它的路由；每秒大量请求时，handler 每 100 ms 在 akit 定时器线程（A45 的临时方案）上对其交换的巡检。 |
| WASM 插件 | I71、I72 | 部分（2026-11-25，I76，`346ad8b`：fit 的 `headers` 插件在 akit 和 fit 中，HTTP/1.1 和 h2c，见 [M5 报告](../bench/results/m5-report-2026-11-25.zh-CN.md)） | 否 | I76 已测（`bench/run.sh -servers akit-wasm,fit-wasm -scenarios fixed,wasm`）：fit 的 `headers` 插件在 HTTP/1.1 上让 akit 每个请求多花 14.7 µs CPU，fit 6.8 µs（akio-wasm 解释执行，wazero 编译执行：akio API 请求 A180，akio 的 W5）；h2c 上 2,048 个同时进行的请求超出 1,024 的队列，两者都立即以 503 回答多出的部分。剩下的：对照 fit（wazero 的编译器）和无插件：请求经过 fit 的 `headers` 插件（Go guest）和一个空插件（`fit_on_request` 返回 0），HTTP/1.1 和 h2，每秒请求数和延迟，对比不带该中间件的路由（交给池再交回、复制请求、调用本身）；实例上单独的 guest 调用（akio 的 W5：akio-wasm 的解释器对比 wazero 的解释器和编译器）；客户端多于 `instances` 时的池（队列，超出后 503）；trap 之后重建实例（Go guest 的 `_initialize`）。Proxy-Wasm（I72）：请求经过 Proxy-Wasm Go SDK 的 `http_routing`（配置 `1`，请求不变），对照 fit 和无插件，如 fit 的 `BenchmarkProxyWasm`；响应经过 fit 的 `response` 过滤器（`upper`）和 SDK 的 `http_headers`，头部在池上挂起、响应体在池上过滤，一个完整的 4 KiB 响应体和一个流式的 1 MiB 响应体，HTTP/1.1 和 h2，对照 `response: none` 和无插件。 |
| 有流量时频繁替换配置 | I38 | 否 | 否（已测试，未测量） | fixed 和 proxy 场景，HTTP/1.1 和 h2c，期间每 10 ms 经 SIGHUP 和 Admin API 替换一次配置，对照同一轮中不替换：替换期间的每秒请求数、p99 延迟和错误，以及替换 10 万次后 akit 的 RSS 和描述符数；完整测试套件的 M2 报告（规则 24，里程碑结束）。 |
| 访问控制列表 | I85–I87 | 否 | 否（已实现，未测量） | 同 fit 的 `BenchmarkAllows`：一个策略的查找（一个含 10、10,000 和 1,000,000 个前缀、从文件读入的列表，以及串联的三个列表），IPv4 和 IPv6 地址，1 和 16 个线程，不分配内存。与不带 `access` 的同一配置做同一轮 A/B、交替轮次：带大型拒绝列表时，监听器在 accept 时拒绝或接受的每秒连接数（akio 的过滤器，A3：每次 accept 一次 `Arc` 读取加一次查找）；路由带自己的列表（逐请求检查：一次图读取加一次查找）与不带时，fixed 和 proxy 场景在 HTTP/1.1、h2c 和 h3 上的表现；由 akit-l4 的入口决定的 `tcp` 服务每秒连接数；被拒绝的 `udp` 数据报洪泛（在 akio 的过滤器处丢弃，A220）对正常会话延迟的影响。 |
| 控制台与加密的 store | I89–I93c | 否 | 否（控制面） | 控制台属于控制面（规则 24：自身无需基准），但与数据面共用主机。同一轮 A/B、交替轮次：fixed 和 proxy 场景在控制台遭受大量失败登录（成本 10 的 bcrypt 在其单线程登录池上，16 个等待，其余 503）时和没有时，并有一个已登录客户端每秒轮询 `GET /api/m/admin/status`：数据面的每秒请求数和 p99 不应变化。控制台自身的数字仅作记录：成本 10 和 12 时每秒登录数；工作台首次加载（其 2.5 MB 文件，从内存提供）在 HTTP/1.1 和 TLS 上的耗时。I93：启动时 store 的密钥派生（fit 的 Argon2id 参数 3、64 MiB、4 线程：耗时和峰值内存），以及 4 MiB 配置一次写入（密封、同步、重命名）的延迟。 |
| 集群管理：管理端的轮询器和对成员的调用 | I132–I139c | 否 | 否（控制面） | 控制面（规则 24），但与数据面共用主机。同轮 A/B、交替进行：在每秒轮询 256 个替身成员（`max_concurrency` 8，各读 `/node` 和 `/status`）的管理端上跑 fixed 和 proxy 场景，与不轮询相比；在每秒被轮询的成员上跑，与未被轮询相比：每秒请求数和 p99 不应变化。记录：256 个成员一轮轮询的耗时；50 个成员、每批 10 个、门槛 5 s 的滚动发布从头到尾的耗时；有 256 个成员及其状态时管理端的内存。 |
| akio 请求替换掉的权宜实现（A12、A79、A45、A117、A30、A54、A29、A75、A78、A22、A170、A24、A49、A35、A148） | —（akio 请求） | 否 | 否（已实现，未测量） | 对照这些提交之前的 main，同一轮 A/B 测 fixed 和 proxy 场景，HTTP/1.1、h2c 和 h3：访问日志和指标走监听器的响应观察者（`set_observed`，不分配）对比装箱的过滤器，以及每个请求都调用 `limit_body`；代理在 HTTP/1.1 上按长度成帧的下载对比 chunked；第 4 层转发由 akio 的每连接空闲超时关闭对比 akit 的扫描（大量空闲连接）；sockmap 转发经 `akio::ebpf::Tunnel`（以 root 测 TCP 吞吐，同 I51b 一行）；健康检查探测负载走 `akio::timer::after_send` 对比 akit 原来的定时线程。 |
| BitTorrent：客户端和 tracker | I151, I152 | no | no（尚未构建） | 对照 fit 的角色（fit 对 fit、akit 对 akit 以及混合），在回环地址上和两台主机之间：从一个做种者和从多个做种者下载一个种子的 MiB/s（TCP 和 uTP，加密和明文）、慢磁盘下磁盘池的队列、有和没有 resume 数据时重启到做种的时间、每个种子和每个 peer 的内存；tracker 在 100,000 个种子和 500,000 个 peer 下 HTTP 和 UDP 每秒的 announce 数及其内存；如 fit 的 I205 那样以 fit 的 `bench/bt.sh` 对照 anacrolix 和 libtorrent。 |

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
- **状态**：doing（2026-11-26）：各项局限已完成，审查的发现等待审查（[决定 035](decisions/035-admin-api-redacts-secrets.zh-CN.md)）。
  - 已关闭：**L2**（Admin API 按 fit 的 `secretName` 规则把内联的秘密答为 `[redacted]`，修改送回的 `[redacted]` 保留秘密）；
    **L3**（对他人开放的秘密文件发出警告；证书存储的根目录及其写入的目录收紧为 0700，重命名后同步目录）；**L4**（不在环回上明文
    发送的凭据和密钥发出警告，Kubernetes provider 的写入其日志）；**L6**（参考文档的 `forward_proxy`；没有正向代理的指南）；
    **L7**（与 fit 相同跟随链接，写明并有测试）；**L12**（fit 没有这样的字段：按现状关闭）；**L13**（`compress` 处的说明）；
    **L14**（`cache_purge_guarded`）。
  - 已移出：**L8** 和 **L9** 交给 fit（决定 002）：在决定 035 中提议 `tls.client_auth.crl_files` 和 `wasm` 中间件的
    `module_sha256`；akio-tls 已支持 CRL（`client_crls`），两者都不需要 akio。**L15** 已作为
    [A225](akio-api-requests.zh-CN.md#a225-对-akio-的密码学tlshttp-解析器和-webassembly-的外部安全审查) 向 akio 提出。
  - 剩下：外部审查的发现（尚无）。它们修复或接受后即完成。

## 1.0 之后

每条路线在开始时拆成迭代，依据 fit 中同一路线的迭代（编号见 fit 的
[开发计划](https://github.com/lesismal/fit/blob/main/docs/dev-plan.zh-CN.md)）。下表是计划的顺序；彼此没有依赖的
路线可以并行进行。

| 顺序 | 路线 | fit 的迭代 | akit 中的第一步 |
| --- | --- | --- | --- |
| 1 | 日志：文件、切割与保留 | I155–I157 | 已完成：[I81–I83](#日志文件切割与保留)，带时区的日志文件，按大小和时间切割，保留策略 |
| 2 | 访问控制列表 | I195 | [I85–I87](#访问控制列表)：列表、监听器和路由；服务与 SOCKS；Admin API |
| 3 | 控制台与加密的 store | I104、I157 的页面 | [I89–I93c](#控制台与加密的-store)：控制台的登录、服务器和页面（包括基于 I83 的 `/logs` 的“日志”页面）、带 TLS 的 Admin API、加密的 store |
| 4 | 隧道与 LLM | I105–I107, I159–I162 | [I98–I109](#隧道与-llm)：配置中的角色、链路 token、`akit run` 中的角色；基于 TCP、TLS、WebSocket 和 QUIC 的隧道链路；基于 `akio-ws` 的 LLM mesh 及其 node、hub 和 proxy；角色的页面 |
| 5 | WebRTC | I87, I108–I111 | [I110–I114](#webrtc)：配置中的角色；基于 `akio-turn` 的 TURN；基于 `akio-webrtc` 的 WHIP 和 WHEP；代理的 WHIP 和 WHEP |
| 6 | 流媒体与媒体认证 | I115–I117 | [I120–I126](#流媒体与媒体认证)：配置中的 `media_auth` 和 `streaming`、策略与 `akit media-token`、基于 `akio-media` 的流媒体角色（其媒体中心、RTMP、RTSP 和 SRT 监听器、`httpflv`、`hls`、`stream_whip` 和 `stream_whep`）、媒体页面 |
| 6 | 流媒体与媒体认证 | I115–I117 | 基于 `akio-media` 的流媒体角色；`media_auth` |
| 7 | 媒体库与频道 | I131–I139 | MP4 读取器（akio 需求）；点播；频道；签名 URL |
| 8 | 集群管理 | I140–I150, I153 | [I130–I139c](#集群管理)：配置、成员需要什么、管理端及其成员、页面、分组、模板、带健康门槛的发布、漂移和告警、角色、NAT 之后的成员、指标和 CLI；I153 随 AI 助手路线 |
| 9 | AI 助手 | I124–I130, I152–I154 | 对话面板、工具、MCP 服务器 |
| 10 | BitTorrent | I196–I210 | [I150–I159](#bittorrent)：配置中的角色、基于 `akio-bittorrent` 的 tracker 和客户端、添加种子、时间表和访问控制、`bt_stream`、发布、Admin API 和事件、页面、RSS |
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

### 访问控制列表

fit 的迭代 I195（见它的[开发计划](https://github.com/lesismal/fit/blob/main/docs/dev-plan.zh-CN.md)、
`access` 包和[配置参考](https://github.com/lesismal/fit/blob/main/docs/reference/config.md)）：客户端地址的
允许列表（白名单）和拒绝列表（黑名单），一个全局的 `access`，以及每个监听器、路由、服务和 Admin API 上各自的一个。
在适用的列表中，最具体且有发言权的那个说了算：路由的，然后是其监听器的，最后是全局的；服务的，然后是其监听器的，
最后是全局的；Admin API 的，然后是全局的。在接受连接时就能确定策略的，由 akio 的 accept 过滤器（A3）当场拒绝；
只有之后才能确定的（路由或服务带有自己的列表、由 PROXY 头指明客户端），则请求回答 403，或在知道服务后关闭连接。
fit 的各个角色（`streaming`、`webrtc`、`tunnel`、`llm`、`bittorrent`）和控制台的 `access` 随各自的路线实现。

### I85 访问控制列表 I：列表、监听器和路由

- **做什么**：新 crate `akit-access`，对应 fit 的 `access` 包：把内联条目和列表文件（地址和前缀、区间、eMule
  `ipfilter.dat` 行和 PeerGuardian `.p2p` 行，每个文件最多 64 MiB）编译成按地址族分开、排序并合并的区间，用二分查找；
  由适用的列表组成的策略，最具体的在前。在顶层、监听器和路由上读取 `access`，带 fit 的检查。图按 fit 的
  `compileAccess` 编译每个监听器的策略：监听器自己的列表和全局列表就是全部时，用 akio 的 accept 过滤器（A3），
  每次重载在保留的监听器上重新设置，不重新绑定；监听器的某个路由有自己的列表、或监听器接受 PROXY 头时，逐个检查 HTTP 请求，
  被拒绝的回答 `403 forbidden`。akio 的 HTTP/3 没有 accept 过滤器（A220）：它的请求逐个检查。列表文件在编译配置时读取，
  读不了的文件使配置失败，并与证书文件一起被监视（`-watch-certs`）。
- **完成标准**：移植后的 fit `access` 测试（`TestPolicyLayers`、`TestChainEmpty`、`TestDenyBeatsAllowInOneList`、
  `TestEdges`、`TestSetAgainstLinear`、`TestFiles`、`TestCompileErrors`、`TestRangeOf`、`TestParseLine`、
  `TestP2PAndDATFiles`、`TestRangesAgainstLinear`）和 `listener/access_test.go` 中 HTTP 部分（`TestAccessRefusedAtAccept`、
  `TestAccessRoute`、`TestAccessFollowsReload`、`TestAccessProxyProtocol`）在 epoll 和 io_uring 上通过，HTTP/3 逐请求拒绝；
  这些字段改为读取，用 `make v1-bless` 标记。
- **涉及**：`akit-access`（新）、`akit-config`、`akit-runtime`、`akit-listener`、`tests/compat`。
- **阅读**：akio 的 `Listener::filter`、`Engine::set_listener_filter`（ADR-099）。
- **fit**：`access`、`config`（`Access` 及其 `check`）、`runtime/graph.go`（`compileAccess`）、`listener`（I195）。
- **状态**：已完成（2026-11-26）。akit-access（[决定 034](decisions/034-access-lists.zh-CN.md)）；在顶层、监听器和路由上读取 `access`；
  由 akio 的过滤器在 accept 时决定、每次重载替换，或逐请求决定（403）；在 akio 的 A220 之前 HTTP/3 逐请求检查，此后（2026-11-27）由 akio 在其 UDP 套接字上的过滤器决定；列表文件与证书一起被监视。
  `udp` 监听器和接受 PROXY 头的 `tcp` 监听器留待 I86。见进度日志。

### I86 访问控制列表 II：服务与正向代理

- **做什么**：`services[].access`。只有一个不带 `server_names` 的服务、独占端口且不接受 PROXY 头的 `tcp` 监听器，
  在 accept 时按该服务的策略决定；服务按服务器名挑选、与 HTTP 或 SOCKS 共用端口、或接受 PROXY 头的监听器，由
  akit-l4 的入口在知道服务后决定，被拒绝的连接被关闭；`udp` 会话在打开时被拒绝（akio 还不过滤数据报，A220：
  被拒绝客户端的数据报会打开一个会话并立即关闭）；监听器之后才决定时，`socks` 正向代理在客户端发出请求时检查它，
  同 fit 的 `SOCKSAccess`。
- **完成标准**：移植后的 fit `TestAccessService` 通过（一个 `tcp` 监听器，以及一个与 HTTP 共用的，服务的列表双向覆盖全局列表），
  另有 `udp` 服务、按服务器名挑选的服务和共用端口上的 SOCKS；在 epoll 和 io_uring 上。
- **涉及**：`akit-config`、`akit-runtime`、`akit-l4`、`akit-listener`、`tests/compat`。
- **fit**：`l4/entry.go`（`SetAccess`、`CheckAccess`）、`l4/forward.go`（`NewSOCKS`）、I195。
- **状态**：已完成（2026-11-26）。`services[].access`；只有一个服务的 `tcp` 监听器在 accept 时决定，其他由 akit-l4 的入口在知道服务后决定，
  包括接受 PROXY 头的监听器；在 akio 的 A220 之前，`udp` 会话在打开时关闭，此后（2026-11-27）它们的数据报在 akio 的过滤器处丢弃；SOCKS 客户端在发出请求时检查。见进度日志。

### I87 访问控制列表 III：Admin API 与配置参考

- **做什么**：`admin.access`，在全局列表之前决定，全局列表也适用于 Admin API（及其提供的仪表盘）；在 accept 时决定，
  每次应用配置都会重新设置，尽管 `admin` 的其余部分需要重启；Admin API 在 Unix 套接字上时拒绝它，因为其对端没有地址、
  从不检查。配置参考中关于访问控制列表的一章，取自 fit 的 `schema/doc/access.md`：列表如何组合、各自在哪里决定、
  列表文件的格式、使用 ACME 的 HTTP-01 和 TLS-ALPN-01 时允许列表必须放行 CA 的验证服务器。决定 034 记录 akit 与 fit
  决定位置不同的地方。
- **完成标准**：移植后的 fit `admin/access_test.go`（`TestAdminAccess`、`TestAdminAccessFile`、`TestAdminAccessUnixSocket`）
  通过；配置参考中的示例通过 `akit validate`。
- **涉及**：`akit-config`、`akit-admin`、`akit`、`docs/reference`、`docs/decisions`。
- **fit**：`admin/server.go`（`accessListener`）、`admin/configs.go`（`setAccess`）、`config/validate.go`、
  `schema/doc/access.md`。
- **状态**：已完成（2026-11-26）。`admin.access` 在 accept 时决定，每次应用配置都重新设置；在 Unix 套接字上被拒绝；配置参考的这一章、
  Admin API 指南和决定 034 涵盖整条路线。各角色和控制台的 `access` 随各自的路线实现。见进度日志。

### 控制台与加密的 store

fit 的 I104（[fit 的决定 027](https://github.com/lesismal/fit/blob/main/docs/decisions/027-console-and-store.zh-CN.md)、
其[控制台指南](https://github.com/lesismal/fit/blob/main/docs/console.zh-CN.md)）以及其 I157 中工作台的“日志”页面（基于
I83 的 `/logs`），分六次迭代（[决定 036](decisions/036-console.zh-CN.md)），I94 由 I92 拆出，I93b 和 I93c 由 I93 拆出。`admin.console` 是一个独立的服务器，与
Admin API 并列，在自己的登录之后于进程内调用 Admin API 的端点；它按 fit 发布时的原样提供 fit 的工作台页面。`admin.store`
以 fit 的 `fitstore` 格式密封保存配置。威胁模型的 L1（Admin API 没有 TLS、不能远程访问）由带 TLS 的控制台（I90）和带 TLS
的 Admin API（I92）关闭；L5（密钥未加密存放）在这里重新审视，仍为接受：fit 的 store 保存配置和控制台用户，不保存证书存储，
而 akit 与 fit 共用证书存储（I62）。控制台不需要 store：没有 store 时，它像 Admin API 一样修改运行中的配置，修改密码被拒绝。

### I89 控制台 I：`admin.console` 及其登录

- **做什么**：按 fit 的 `config.Console` 读取 `admin.console`：`address`（127.0.0.1:2020）、`cert_file` 和 `key_file`
  （成对出现；非回环地址两者都需要）、`users`（至少一个，每个有 `username` 和 bcrypt 的 `password_hash`，名字不重复）、
  `session_timeout`（30m）、`session_lifetime`（12h）、`max_sessions`（64）、`max_conns`（32）、`access`（I85 的列表）；
  `mptcp` 以“尚不支持”拒绝。只有 `admin`、没有监听器和路由的配置是有效的，同 fit 只有控制台的配置。登录同 fit 的 `login`
  包：不是用户的名字用一个假哈希检查，最多同时检查两个；一个地址（IPv6 按 /64）15 分钟内失败 5 次即锁定 15 分钟，之后每次
  再被锁定时长加倍，最长一小时，一个地址同一时间只检查一个，最多记录 4,096 个地址；一个用户连续失败 10 次后，其检查从
  2 秒起被延缓，加倍，最长 5 分钟；会话为 32 个随机字节，按其 SHA-256 保存，空闲 `session_timeout`、总计 `session_lifetime`、
  登出时，或超过 `max_sessions` 时最旧的一个被结束；每个会话一个 CSRF 令牌，以常数时间比较。`akit hash-password`
  （`-cost`，10）：标准输入第一行的 bcrypt 哈希，从终端读取时询问两次。
- **完成标准**：移植后的 fit `login` 测试通过，使用假时钟（`TestKey`、`TestCheck`、`TestSessions`、`TestLockout`、
  `TestAdmitOneAtATime`、`TestAdmitAttack`、`TestAdmitFullTable`、`TestUserBackoff`、`TestUserBackoffQuiet`、
  `TestAddressBackoff`、`TestAdmitForgetsQuiet`、`TestEndUnknown`、`TestCookieName`、`TestUsersAndPasswords`、
  `TestEndUserSessions`）；配置检查同 fit 的 `checkConsole`；`hash-password` 产生可验证的哈希；字段用 `make v1-bless` 标记。
- **涉及**：`akit-config`、`akit-admin`（`console/login`）、`akit`（`hash-password`）、`tests/compat`、`docs/reference`。
- **fit**：`config/config.go`（`Console`）、`config/validate.go`（`checkConsole`）、`login`、`cli`（`hash-password`）；I104。
- **状态**：已完成（2026-11-26）。在 akio 的 A233 之前终端回显保持开启；此后（2026-11-27）与 fit 相同，使用原始模式并为每个字符显示一个 `*`。见进度日志。

### I90 控制台 II：控制台的服务器

- **做什么**：控制台运行在自己的 akio 引擎上，一个 I/O 线程，同 Admin API（决定 006），HTTP/1.1，用 `cert_file` 和
  `key_file` 提供 TLS（TLS 1.2 及以上）；`max_conns` 在 accept 时限制；`admin.console.access`、再是全局 `access`，在 accept
  时决定，每次应用配置都重新设置，尽管 `admin.console` 的其余部分需要重启。fit 的端点：`POST /api/login`（JSON，最多
  4 KiB；在自己的有界池上检查，1 个线程、16 个等待，超出 503；锁定时 429，带 `Retry-After` 和 `retry_after`）、
  `POST /api/logout`、`GET /api/session`、`GET /api/workbench`（用户和模块 `admin`）、`/api/m/admin/<path>`：Admin API
  的端点，在进程内调用，用会话代替令牌，所应用的配置来源为 `console`；`GET /api/m/admin/users`、
  `POST /api/m/admin/account/password` 和 `POST /api/m/admin/users/password`（没有 store 时 409：I93）。会话的 cookie
  （`fit_console_<端口>`，`HttpOnly`、`SameSite=Strict`，TLS 上 `Secure`；自 I97 起为 `akit_console_<端口>`），除读取外每个
  请求带 `X-Fit-Csrf`（自 I97 起为 `X-Akit-Csrf`），`Origin`
  不是控制台自己的则拒绝，fit 的响应头（Content-Security-Policy、`X-Frame-Options`、`Referrer-Policy`、`nosniff`、TLS 上的
  HSTS、API 上的 `Cache-Control: no-store`）。`admin` 日志记录器：谁从哪里登录、每次失败、每次登出、每个用户的每次修改及其
  应用的版本。`akit run` 与 Admin API 一起启动它。
- **完成标准**：移植后的 fit `admin/console_test.go`（`TestConsoleLoginAndEdit`、`TestConsoleLoginRefused`、
  `TestConsoleSessions`、`TestConsoleFailureTableBounded`）和 `app/console` 测试中既不需要页面也不需要 store 的用例
  （`TestConsoleTLS`、`TestConsoleLoginQueue`、CSRF、其他来源、响应头）通过；accept 时及重载后的访问控制；通过控制台修改的
  配置运行并被记录；在 epoll 和 io_uring 上。
- **涉及**：`akit-admin`（`console`）、`akit`（`run`）、`docs/guides`。
- **阅读**：akio 的 `Pool`（A117）、`Context::req_mut`、`Detached`。
- **fit**：`app/console`、`admin/console.go`、`login`；I104。
- **状态**：已完成（2026-11-26）。在 akio 的 A232 之前连接像 Admin API 的那样受限，此后（2026-11-27）按 fit 的规则。见进度日志。

### I91 控制台 III：页面

- **做什么**：fit 的工作台（`app/workbench/web`，Apache-2.0，纯 HTML、CSS 和 JavaScript，没有构建步骤）在兼容性门禁固定的
  fit 提交上由脚本原样复制，连同其许可证，编进 akit，像 fit 嵌入它那样从内存提供（[决定 036](decisions/036-console.zh-CN.md)）；
  `GET /api/m/admin/schema` 为门禁固定的 fit `fit.schema.json`。原文编辑器和导入导出页面，作为控制台调用的 Admin API 端点：
  `POST /config/render` 和 `POST /config/parse`（`.conf`、`.yaml`、`.json`）、`PUT /config/text`（`?members=`）、
  `POST /config/export` 和 `POST /config/import` 的文本格式（`store` 格式等 I93）；标记为 v1 新增。fit I157 的“日志”页面
  通过 I83 的 `/logs`、`/logs/rotate` 和 `/logs/cleanup` 工作，每个操作记入 `admin` 日志记录器。
- **完成标准**：页面在浏览器中加载、登录、读取并保存配置（一次表单编辑、JSON 页面、一个 `.conf` 标签页）、回滚一个版本，
  并列出、切割和清理日志文件；移植后的 fit `admin/text_test.go`（`TestTextRenderParse`、`TestTextParseErrors`、
  `TestTextPartSave`、`TestTextWholeConfig`）和 `transfer_test.go` 中文本格式的用例通过；提供的文件与 fit 的逐字节相同。
- **涉及**：`akit-admin`（`console`、`text`）、`tools`、`tests/compat/v1`、`docs/guides`。
- **fit**：`app/workbench`、`admin/check.go`（`renderText`、`parseText`、`putText`）、`admin/transfer.go`；I104、I157。
- **状态**：已完成（2026-11-26）。页面即 fit 的，带 akit 尚不运行的功能的菜单（决定 036）；在 I97 之前原样提供，I97 把其中
  fit 的名字改了名（决定 037）。见进度日志。

### I92 非回环地址上带 TLS 的 Admin API

- **做什么**：`admin.cert_file`、`key_file` 和 `client_ca_file`，同 fit 的 `checkAdminAddress`：API 通过 TLS（1.2 及以上）
  提供，要求客户端证书并用 `client_ca_file` 验证；非回环地址在有令牌和 TLS 时允许，没有时拒绝；Unix 套接字上三者都不允许。
  关闭威胁模型的 L1。
- **完成标准**：移植后的 fit `admin/tls_test.go` 通过（`TestTLS`、`TestTLSClientCA`、`TestTLSStartErrors`；`TestTLSReload`
  移到 I94），以及非回环地址有无令牌两种情况；字段用 `make v1-bless` 标记。
- **涉及**：`akit-config`、`akit-admin`、`docs/reference`、`docs/security`。
- **fit**：`admin/server.go`、`admin/tls.go`、`config/validate.go`（`checkAdminAddress`）。
- **状态**：已完成（2026-11-26）。证书文件在 akit 启动时读取；文件变化时重新读取是 I94。见进度日志。

### I93 加密的 store I：格式、`admin.store` 和 `akit store`

- **做什么**：逐字节移植 fit 的 `admin/store`，即 `akit_admin::store`：`fitstore` 格式（59 字节的头、Argon2id 的参数和盐、以
  XChaCha20-Poly1305 密封的内容，头为其附加数据；读取版本 1 到 3，fit 存放在其中的 sections 原样保留；限制文件及导入的文件可要求的
  Argon2id 代价），每次写入用新的 nonce 密封，0600、同步、重命名，导出用自己的密码密封；按 fit 的方式读取并检查 `admin.store`
  （`path`，`password`、`password_file`、`password_env` 恰好一个）；`akit store export` 和 `akit store import`，同 fit 的 `fit store`。
- **完成标准**：移植后的 fit `admin/store` 测试通过；fit 写的 store（各个版本、fit 自己的 Argon2id 参数、导出文件）能在 akit 中打开，
  akit 写的能在 fit 中打开，两者都用 fit 自己的代码检查；`store export` 和 `import` 往返一致。
- **涉及**：`akit-admin`（`store`）、`akit-config`、`akit`（`store`）、`tests/compat`、`docs`。
- **阅读**：akio-crypto 的 `kdf::argon2id`（[A230](akio-api-requests.zh-CN.md#a230-akio-crypto-中的-argon2id)）和
  `chacha::XChaCha20Poly1305`（[A231](akio-api-requests.zh-CN.md#a231-akio-crypto-中的-xchacha20-poly1305)）。
- **fit**：`admin/store`、`cli/store.go`、`config`（`Store`、`checkStore`）；I104。
- **状态**：已完成（2026-11-27），拆分出 I93b 和 I93c：在 I93b 把配置保存在 store 中之前，`akit run` 拒绝 `admin.store`。见进度日志。

### I93b 加密的 store II：`akit run` 把配置保存在其中

- **做什么**：同 fit 的 `cli/run.go`、`admin/bootstrap.go` 和 `admin/configs.go`：store 作为 akit 运行所依据的基础配置，每次启动时合并
  到配置文件之上（其值优先）并写回，不存在时由配置文件播种；`admin` 取自引导文件，从不保存、显示，有 store 时也不从发来的配置中
  取；每个应用的配置（Admin API、控制台、回滚、SIGHUP、provider 的基础配置）写入其中，写不进去的仍然应用并说明未保存；SIGHUP
  重新读取它；有 store 时拒绝 `-watch`；控制台的密码修改（`POST /api/m/admin/account/password`、`/users/password`，没有 store 时回答
  409）保存在其中，其用户覆盖引导文件中的；Admin API 的 `/config/export` 和 `/config/import` 支持 `store` 格式（自己的密码或正在运行的
  store 的密钥，导入按 fit 的 `Unseal` 限制）。密钥在启动时于控制线程上派生一次；写入在控制线程上。
- **完成标准**：移植后的 fit `cli` store 测试和 `admin` 的 store、transfer 测试通过；在控制台中修改的配置运行、存入 store 并在重启后
  仍在；错误的密码或改动一个字节使 akit 无法启动。
- **涉及**：`akit-admin`（`configs`、`console`、`server`）、`akit`（`run`）、`docs`。
- **fit**：`cli/run.go`、`cli/store.go`、`admin/bootstrap.go`、`admin/configs.go`、`admin/transfer.go`、`admin/console.go`；I104。
- **状态**：已完成（2026-11-27），与 I93c 一起。Admin API 的 `store` 导出和导入的密钥派生在其线程池上进行，一次一个，带自己密码的导出
  也一样（fit 只限制导入）；密码修改在控制台的登录池上做哈希并保存。fit 的 `TestStoreCommand` 中 `{"listeners": []}` 的用例未移植：
  akit 接受空的监听器列表。除了由 fit 的代码写入和读取 store（`tests/fitstore`）之外，`AKIT_FIT_BIN` 指定 fit 的二进制时，fit 和
  akit 轮流从同一个 store 运行。见进度日志。

### I93c 加密的 store III：`akit run -admin` 和配置旁边的 store

- **做什么**：同 fit 对其决定 027 的 2026-11-04 修改：`-c` 指定一个 `.store` 文件时用它作为 store；没有 `admin.store` 时，配置文件旁边的
  store（`site.yaml` 对应 `site.store`）存在后即使用，其密码来自环境变量或旁边的密钥文件（`<store>.key`，两者都不存在时生成，0600 且
  随机）；`akit run -admin`：配置省略时补上 `admin` 和 `admin.console` 及其默认值，配置保存在 store 中，没有控制台用户时添加用户
  `admin`，临时密码为 `admin`，须在 `-timeout` 内修改，使用它的会话只能修改密码。
- **完成标准**：移植后的 fit `run -admin` 测试通过。
- **涉及**：`akit`（`run`、`store`）、`akit-admin`（`console`）、`docs`。
- **fit**：`cli/run.go`、`cli/store.go`、`admin/bootstrap.go`（`WithDefaultConsole`）；I104。
- **状态**：已完成（2026-11-27），与 I93b 一起。fit 的名字按决定 037 改名：`AKIT_STORE_PASSWORD`，没有配置文件时为 `akit.store`；
  密钥文件 `<store>.key` 同 fit，使 fit 和 akit 共用一个 store 及其密钥。见进度日志。

### I94 文件变化时重新读取 Admin API 和控制台的证书

- **做什么**：同 fit 的 `admin/tls.go`：`admin.cert_file`、`key_file`、`client_ca_file` 和 `admin.console.cert_file`、`key_file`
  的文件变化时重新读取（`-watch-certs`，akit 的证书监视器），新证书无需重启就提供给新连接，加载失败的文件记入日志并保留旧证书。
- **完成标准**：移植后的 fit `TestTLSReload` 对 API 和控制台都通过。
- **涉及**：`akit-admin`、`akit`（`run`）。
- **fit**：`admin/tls.go`。
- **状态**：已完成（2026-11-27），先于 I93 完成，I94 不需要它。文件在控制线程上查看
  （`-watch-certs`，每个 `-watch-interval`，以及 SIGHUP），而不是像 fit 那样在握手时查看（规则 1）；控制台的证书也会重新
  读取，fit 的不会。见进度日志。

### akit 的名字

不是 fit 的路线：维护者决定 akit 对外的名字是 akit 自己的（[决定 037](decisions/037-akit-names.zh-CN.md)），在 akit 首次发布之前。

### I97 akit 的名字

- **做什么**：对外凡写 "fit" 的名字都改为 "akit"，不保留别名：Admin API 的 `Akit-Config-Version` 和 `Akit-Dry-Run`、
  `Akit-Error` trailer、控制台的 `akit_console_<端口>` 和 `X-Akit-Csrf`、导出的 `akit-*` 文件名、指标 `akit_*`、OTLP 的
  scope 和 span 属性、`Cache-Status` 的 `akit`、WebAssembly ABI（`akit_on_request`、模块 `akit`）、以网关命名的九个默认值、
  Ingress 注解 `akit.lesismal.github.io/*`、`jwt` 的 realm、默认配置文件 `akit.json`、`adapt` 的格式名 `akit`。控制台的页面由
  `tools/console-pages.py` 的确定性改名列表改名，由 `make compat-check` 检查；WebAssembly 测试插件由 fit 的插件把 ABI 的名字改名
  后重新构建（`tools/plugin-guest.sh`）；重写 v1 基线；更新所有文档。
- **完成标准**：没有对外名字写着 "fit"（决定 037 的报告中的 grep）；fit 的名字被拒绝（fit 构建的插件、旧的请求头和 cookie
  不被读取）；页面在浏览器中对 akit 的服务器正常工作；门禁、v1 检查和整个工作区在 macOS、epoll 和 io_uring 上通过。
- **涉及**：每个带名字的 crate、`tools`、`tests/compat/v1`、`bench`、`deploy`、`docs`。
- **状态**：已完成（2026-11-26）。见进度日志。

### 隧道与 LLM

fit 的 I105–I107 和 I159–I162（其[隧道](https://github.com/lesismal/fit/blob/main/docs/tunnel.zh-CN.md)和
[LLM](https://github.com/lesismal/fit/blob/main/docs/llm.zh-CN.md)指南、其决定 028 和 035，以及其包 `tunnel`、`llm`、
`internal/linktoken`、`config/tunnel.go` 和 `config/llm.go`），分十二次迭代（[决定 038](decisions/038-tunnel-and-llm.zh-CN.md)）。
配置中的六个角色：`tunnel.hub`、`tunnel.node` 和 `tunnel.proxy` 经由 hub 访问 NAT 之后的 TCP 服务；`llm.hub`、`llm.node`
和 `llm.proxy` 在多台机器之间共享 AI 模型，并向 agent 提供它们的 API。它们运行在监听器的 akio 引擎上，与网关的路由并列，
或者没有路由，基于 akio 的 TCP、TLS、WebSocket（`akio-ws`）、QUIC（`akio-quic`）和 HTTP 客户端。akit 对外显示的名字都是
akit 的（决定 037：token `akitt1.`、`X-Akit-Node`、akit 写的文件）；线上协议的常量（ALPN、HKDF 和 HMAC 标签、方法名）沿用
fit 的，使 akit 的角色能与 fit 的角色互连（决定 038）。

### I98 隧道与 LLM I：配置中的 `tunnel` 和 `llm`

- **做什么**：六个成员与其余配置一样严格读取，字段、默认值、范围和检查与 fit 的相同（`config/tunnel.go`、`config/llm.go`）：
  hub 的 `listeners`、`quic`、`console_socket` 和各项上限；node 和 proxy 的 `hubs` 及其端点（`tls://`、`wss://`、`quic://`、
  `tcp://`、`ws://`；拒绝查询、片段或用户信息）、`pin`（旧名 `quic_pin` 读取时给出警告）、`ca_file` 和 `quic_insecure`；
  `services`、`forwards` 和 `web_proxies`；LLM 角色的 `id`、`name`、`listen`、`peers`、`limits`、`tokens`，node 的
  `models`、`settings` 和 `proxy`，proxy 的 `api` 和 `api_keys`；每个角色的 `access` 和 `console`。按 fit 的 I160 在非回环地址上
  要求 TLS：拒绝非回环地址上没有 `body_password` 的 `tcp` 或 `ws` 监听器或端点、非回环地址上没有它的 `quic_insecure`、
  非回环地址上没有 TLS 的 LLM 监听器或对端。在角色由其迭代运行之前，`akit run` 拒绝带该角色的配置。
- **完成标准**：移植后的 fit `config/tunnel_test.go`、`llm_test.go` 和 `roles_test.go` 用例通过；fit-examples 的 `tunnel-*` 和
  `llm-*` 通过门禁；配置参考列出每个字段；v1 冻结把它们标为新增。
- **涉及**：`akit-config`（`tunnel`、`llm`）、`akit`、`tests/compat`、`docs/reference`。
- **fit**：`config/tunnel.go`、`config/llm.go`、`config/linktoken.go`；I105、I159、I160。
- **状态**：已完成（2026-11-27）。见进度日志。

### I99 链路 token

- **做什么**：fit 的决定 035，同 `internal/linktoken`，放在新 crate `akit-roles` 中：token `akitt1.<id>.<secret>`（16 位
  十六进制数字，256 个随机位），其 client、stored、server 和 body 密钥，拨号方和监听方基于包含双方 nonce 和方向的记录的
  证明；监听方的 token 登记表（成员绑定到其首次注册所用的 token、过期、最后使用、使用每个 token 的链路，最多 1,024 个
  token、每个 token 256 个成员）；页面、Admin API 和 CLI 共用的管理（签发、修改过期时间、撤销、绑定），作为对成员 token
  列表的编辑。Admin API 的 `/tokens` 和 `akit token` 随运行角色的宿主一起实现（I100）。
- **完成标准**：移植后的 fit `linktoken_test.go` 通过，fit 自己的测试向量逐字节一致（fit token 的 secret 得出相同的哈希、
  body 密钥和证明）。
- **涉及**：`akit-roles`（新，`linktoken`）。
- **fit**：`internal/linktoken`；I159。
- **状态**：已完成（2026-11-27）。见进度日志。

### I100 `akit run` 中的角色

- **做什么**：同 fit 的 `app`（I105）：一个宿主，在每次应用配置时于监听器之后启动、迁移和停止每个角色，角色失败只是警告；
  角色的绑定加在监听器的 akio 引擎上（角色不另建引擎），每次应用都重新设置其 `access`；一个地址，一个服务器（fit 的 I162）：
  配置监听的每个地址（包括各角色的）规范化后统一检查一次，两者中后出现的被拒绝，并指出先出现的那个。Admin API 的
  `GET /tokens`、`GET` 和 `POST /tokens/{成员}`、`PATCH` 和 `DELETE /tokens/{成员}/{id}`（fit 的 `admin/tokens.go`），每个修改
  都是对成员的一次编辑，像任何配置一样应用并记录到 `admin` 日志器；以及调用它们的 `akit token create|list|expiry|revoke`
  （fit 的 `cli/token.go`）。
- **完成标准**：配置能启动、迁移和停止角色（测试中用一个占位角色）；角色失败是警告，网关照常运行；fit 的 `bindings_test.go`
  中的冲突被拒绝；通过 Admin API 和 CLI 签发、列出、修改和撤销 token，token 只显示一次，只保存其哈希；v1 冻结把这些端点
  标为新增。
- **涉及**：`akit-roles`、`akit-config`、`akit-admin`、`akit`、`tests/compat/v1`。
- **fit**：`app/app.go`、`config/bindings.go`、`internal/bindcheck`、`admin/tokens.go`、`cli/token.go`；I105、I159、I162。
- **状态**：已完成（2026-11-27）。见进度日志。

### I101 隧道 I：TCP 上的链路、注册和隧道

- **做什么**：fit 的链路协议（`tunnel/internal/proto`、`link`、`bodycrypt`），逐字节一致：帧、版本 3 和 4 的 hello、用 token
  和已弃用的共享密码注册（双向证明）、`body_password` 密封的帧；hub（对端、链路、把隧道路由到它指定的 node 并转发其帧）、
  node（其 `services`，它只拨这些地址）和 proxy（`forwards`，每个一个本地监听器，隧道分布在其 hub 上）；每条隧道 4 MiB 的窗口，
  两端 TCP 上用 `hold_reads` 和 `on_drain`，由 hub 检查；各角色的上限和超时。基于 `tcp://`。
- **完成标准**：移植后的 fit `tunnel/e2e_test.go` 中基于 TCP 的用例通过（大量传输、慢服务和慢 proxy、中途断开、密封和明文、
  每种角色的 token、成员 ID 绑定到其 token、伪造或重放的证明被拒绝）。
- **涉及**：`akit-tunnel`（新）、`akit`。
- **fit**：`tunnel/hub`、`tunnel/node`、`tunnel/proxy`、`tunnel/internal/{proto,link,bodycrypt,tunnel,uplink,common}`；I106、I159。
- **状态**：已完成（2026-11-27）。见进度日志。

### I102 隧道 II：TLS 链路和 hub 的证书

- **做什么**：`tls://` 链路（akio-tls，TLS 1.3，ALPN `fit-tunnel`，不恢复会话）；hub 每个监听器的证书（`cert_file` 和 `key_file`，
  在应用成员时读取），或它自己的自签名证书，只生成一次并保存在配置旁（`akit.tunnel.hub.pem`，0600），或在 `quic.cert` 和
  `quic.key` 中给出，其指纹写入日志；node 或 proxy 的 `pin`、`ca_file` 或系统根证书，交给拨号，从不从地址中读取。pin 在握手完成
  之后、链路发送任何字节之前检查（akio API 请求 A235 要求 akio-tls 在握手中检查固定的证书）。
- **完成标准**：基于 TLS 的端到端测试在 pin 和 `ca_file` 下通过；错误的 pin 打开不了任何东西；重新启动的 hub 保持其指纹；
  fit 的 hub 与 akit 的 node 和 proxy、以及反过来，用 pin 经 TLS 互连。
- **涉及**：`akit-tunnel`（`tls`、`hub`、`uplink`、`link`）。
- **fit**：`tunnel/internal/transport/tls.go`、`internal/certpin`；I106、I160。
- **状态**：已完成（2026-11-27）。见进度日志。

### I102b 隧道 II b：QUIC 链路

- **做什么**：`quic://` 链路（运行时绑定在监听器引擎上的 UDP 套接字上的 akio-quic，ALPN `fit-tunnel`，每个连接一条双向流，
  以 fit 的版本字节开头，`idle_timeout` 和 `keep_alive`，hub 的链路数和握手数限制其连接数）；流的发送缓冲即链路的高水位，
  超出的部分由链路的 wire 排队至多 24 MiB，在 akio-quic 的 `on_stream_writable` 时发出；`quic_insecure`；pin 的检查与
  TLS 链路相同（A235）。
- **完成标准**：基于 QUIC 的端到端测试通过（pin、`quic_insecure`、错误的 pin 打不开任何隧道、超过流缓冲的传输、同时多条隧道）；
  fit 的 hub 与 akit 的 node 和 proxy、以及反过来，用 pin 通过 QUIC 互连。
- **涉及**：`akit-tunnel`（`quic`、`hub`、`uplink`）。
- **fit**：`tunnel/internal/transport/quic`；I106、I160。
- **状态**：已完成（2026-11-27）。见进度日志。

### I102c 隧道 II c：WebSocket 链路

- **做什么**：`wss://` 和 `ws://` 链路（akio-ws：hub 默认路径 `/tunnel`，其他路径回 404，握手限 10 秒，链路的字节放在每条至多
  1 MiB 的二进制消息中）。
- **等待**：akio API 请求 A236（压住 WebSocket 的读取、得知其发送队列排空）：没有它们，WebSocket 链路既压不住对端，也无法让
  喂给它的一方恢复（规则 8 和 9）。已完成（akio `6151f5c5`）。
- **完成标准**：基于 WebSocket 和 TLS 上的 WebSocket 的端到端测试通过，并能与 fit 的 hub、node 和 proxy 互连。
- **涉及**：`akit-tunnel`。
- **fit**：`tunnel/internal/transport/ws`；I106、I160。
- **状态**：done（2026-11-27）。见进度日志。hub 的 QUIC 握手限制和监听器证书的重新读取移到 I102d。

### I102d 隧道 II d：hub 的 QUIC 握手限制；证书重新读取

- **做什么**：hub 的 QUIC 握手限制（`handshake_timeout`、`max_handshakes`，超过一半时用 Retry，同 fit），akio-quic 的服务端尚不
  支持（需写一个请求：它的 Retry 由自己在负载下决定，握手由空闲超时约束）；监听器的证书（以及 hub 自己的）在文件变化时重新读取。
- **完成标准**：停滞的 QUIC 握手在 `handshake_timeout` 时关闭；超过 `max_handshakes` 一半时要求新对端 Retry；监听器续期的证书
  无需重启即可生效。
- **涉及**：`akit-tunnel`；akio-quic。
- **fit**：`tunnel/internal/transport/quic`、`tunnel/hub`；I160。
- **状态**：done（2026-11-27）。见进度日志。hub 的 QUIC 监听器基于 akio 的 A245（`handshake_timeout`、`max_handshakes`）；隧道
  hub 监听器的证书重新读取（`tls`、`wss`、`quic`，以及 hub 自己保存的文件）。LLM mesh 监听器证书的重新读取留给 LLM 路线，因为另一
  改动正在编辑其角色（待审决定第 44 行）。

### I103 隧道 III：hub 的调用和状态、token 过期

- **做什么**：hub 的调用（查询 node、在 `allow_hub_services` 允许时设置其服务、踢掉对端、关闭隧道）及其状态，按 fit 的页面读取的
  样子，通过角色交给其页面的句柄（`Role::api`、`akit_tunnel::hub::HubApi`）；token 过期时在过期时刻关闭其链路，吊销时立即关闭；
  `password_peers`。
- **完成标准**：移植的 fit hub 和 token 测试通过（过期、列出使用共享密码的对端）；这些调用对连到 hub 的 node 和 proxy 生效。
- **涉及**：`akit-tunnel`、`akit-roles`。
- **fit**：`tunnel/hub/api.go`、`hub.go` 中的调用。
- **状态**：done（2026-11-27）。见进度日志。控制台套接字和 web 代理移到 I103b。

### I103b 隧道 III b：控制台套接字和 web 代理

- **做什么**：hub 的 `console_socket`（其加密的 WebSocket，fit 的 `wscrypt`：PBKDF2、HKDF、每个方向的 AES-GCM；`max_conns`，来自
  同一地址的未证明连接至多四分之一，10 秒内证明；每个 WebSocket 对应一条到 hub 控制台的 TCP 连接，双向调节）和 proxy 的
  `web_proxies`（一个本地监听器，每条连接对应一个到 hub 套接字的 WebSocket）。
- **完成标准**：移植的 fit `socket_test.go` 和 `webproxy_test.go` 通过；web 代理经 hub 的套接字提供 hub 的控制台，双向对照 fit 的
  hub 和 proxy。
- **涉及**：`akit-tunnel`。
- **fit**：`tunnel/hub/socket.go`、`tunnel/proxy/webproxy.go`、`tunnel/internal/wscrypt`。套接字承载的控制台是 hub 的页面（I109）。
- **状态**：done（2026-11-27）。见进度日志。在 I109 提供套接字承载的 hub 页面之前，`akit run` 拒绝各角色的 `console`，因而也
  拒绝 hub 的 `console_socket`。

### I104 LLM I：网状链路（mesh）

- **做什么**：fit 的 `llm/internal/wire` 和 `mesh`：帧（类型、标志、1 KiB 起 gzip、最大 16 MiB）、会话（奇偶流 ID）、请求、
  按序的推送（最多 1,024 个排队）、每个方向 256 KiB 窗口的流；用 token 或角色密码的握手，双方都给出证明；hello 和角色规则；
  按对端 ID 的连接池、拨号退避、心跳、慢超时和排空；基于 akio-ws，回环上用 `ws://`，带 `pin` 或 `ca_file` 时用 `wss://`；
  最多 256 条未认证链路，单个地址 16 条。
- **等待**：akio API 请求 A236，与 I102c 相同：网格的链路是 WebSocket（自 `6151f5c5` 起已在 akio 的 main 上并被锁定）。
- **完成标准**：移植后的 fit `wire` 和 `mesh` 测试通过（信用、慢读者只阻塞自己的流、取消、空闲、ping 和慢超时、排空、hello
  拒绝、token 和密码）。
- **涉及**：`akit-llm`（新）。
- **fit**：`llm/internal/wire`、`llm/internal/mesh`、`llm/internal/role`；I107、I159、I160。
- **状态**：done（2026-11-27）。见进度日志。留给各角色的迭代：证书文件变化时重新读取监听器的证书（随 I108 完成，决定 44），以及
  `self_signed`（由角色生成，I105–I107）；供页面使用的 `/mesh/*` 方法（I109）。

### I105 LLM II：node

- **做什么**：`models` 及其状态（`disabled`、`error`、`ready`），每 60 秒检查一次（`error` 时 15 秒），基于 akio 异步 HTTP 客户端
  的 `/node/proxy`：每种 API 的 URL、密钥头、发往上游的头和查询参数、在 JSON 正文或表单中设置 `upstream`、在流的信用下双向
  流式传输正文、`connect_timeout`、`first_byte_timeout`、`idle_timeout` 和 `timeout`；推送 node 的信息（间隔 200 毫秒，带序号）。
- **完成标准**：移植后的 fit `llm/node` 代理和模型测试针对每种 API 的模拟上游通过（流式事件、慢的和中途断开的上游、大正文、
  模型改名）。
- **涉及**：`akit-llm`。
- **fit**：`llm/node/{node,models,proxy,push}.go`、`llm/internal/model`。
- **状态**：done（2026-11-27）。见进度日志。node 的管理方法（由其页面或 hub 保存、删除和检查模型，其资料、设置和上游上限）属于
  I108 和 I109；其信息中的硬件属于 I108。

### I106 LLM III：hub

- **做什么**：node 表（`max_nodes`，遗忘离线最久的）、其在线 node 的模型推送给 proxy、`/hub/proxy` 选择负载最低的 node 并在
  每个流的信用下转发帧（响应头之前最多 3 个 node）、带上限的 node 列表；供页面使用的 `/hub/node/call` 和 `/hub/node/stream`。
- **完成标准**：移植后的 fit `llm/hub` 测试通过；一次端到端调用 agent → proxy → hub → node → 模拟上游双向流式传输。
- **涉及**：`akit-llm`。
- **fit**：`llm/hub`。
- **状态**：done（2026-11-27），`/hub/node/call`、`/hub/node/stream` 和 node 列表的 4 MiB 上限除外，拆分为 I106b。见进度日志。

### I106b LLM III b：hub 供页面使用的调用

- **做什么**：`/hub/nodes`、`/hub/node/call` 和 `/hub/node/stream`（在 node 允许 hub 管理时转发其管理方法）、hub 的设置方法；
  node 列表以 4 MiB 为上限，先略去离线的 node；路由计数（`mesh.Routes`）和 `/mesh/status`。
- **完成标准**：移植后的 fit `llm/hub` 中 node 调用和列表上限的测试通过。
- **涉及**：`akit-llm`。
- **fit**：`llm/hub/{manage,nodelist}.go`、`llm/internal/mesh/routes.go`。
- **状态**：done（2026-11-27）。见进度日志。角色的其他 `/mesh/*` 方法（peers、dial、listen、limits）属于 I109。

### I107 LLM IV：proxy

- **做什么**：`api.address` 上供 agent 使用的 API（独立的绑定，非回环地址上需要 TLS）：OpenAI、Anthropic、Gemini、Ollama 和
  Cohere 的路径、模型列表、API key（`ah-`，只保存 SHA-256，常数时间比较）、先直连 node 再经 hub 的路由（响应头之前最多 4 条）、
  `X-Akit-Node`、每种 API 的错误格式和状态码、`max_conns`、`max_inflight`、`max_body_bytes` 和各项超时。
- **完成标准**：移植后的 fit `llm/proxy` 和 `llm/e2e` 测试通过。
- **涉及**：`akit-llm`、`akit`。
- **fit**：`llm/proxy`、`llm/e2e`。
- **状态**：done（2026-11-27）。见进度日志。proxy 页面的方法（创建和删除 key、其 API 设置、其统计）属于 I109。

### I108 LLM V：node 的管理

- **做什么**：每隔 `refresh_interval` 读取硬件，在小线程池（2 个运行、16 个等待）上读取 GPU 状态；固定的动作和 GPU 设置作为
  作业运行（8 个运行、保留 200 个、每个保留 1 MiB 输出，超过 `job_timeout` 杀掉进程组），从不运行调用方的命令，从不经过 shell；
  页面以及在 `allow_hub_manage` 下 hub 可以调用的方法。
- **完成标准**：移植后的 fit `llm/node` 动作、作业和 GPU 测试在 Linux 和 macOS 上通过。
- **涉及**：`akit-llm`。
- **fit**：`llm/node/{actions,jobs,exec,gpu,hwinfo_*}.go`、`llm/internal/gpu`。
- **状态**：done（2026-11-27）。见进度日志。akio 的 A247（有效用户）、A248（向进程组发信号）和 A249（文件系统的空间）已替换其临时做法（akio `a8171fda`）。

### I109 角色的页面

- **做什么**：每个角色的 `console`，与 `admin.console` 一样在登录之后，以及 `admin.console` 上的角色模块：改名后的 fit 工作台
  页面（决定 037）、供页面使用的隧道 API 和 LLM 角色的 `/mesh/*`、`/node/*`、`/hub/*`、`/proxy/*` 方法，每个修改都作为一次
  配置编辑写回角色的成员。
- **完成标准**：移植后的 fit 角色页面测试通过；页面的修改被应用、进入历史，不能加载时被拒绝。
- **涉及**：`akit-admin`、`akit-tunnel`、`akit-llm`、`tools/console-pages.py`。
- **fit**：`app/roleconsole`、`app/workbench`、`tunnel/*/api.go`、`llm/internal/webui`。
- **状态**：角色的控制台与 hub 的页面 done（2026-11-27）；隧道 node 和 proxy 的页面拆为 I109b，LLM 角色的页面拆为 I109c
  （决定第 46 行）。见进度日志。

### I109b 隧道 node 和 proxy 的页面

- **做什么**：node 的模块（fit 的 `tunnel/node/api.go`：其状态，含与各 hub 的链路、各服务的往返时间和隧道；添加、修改、删除 hub；
  设置服务；关闭隧道）和 proxy 的模块（fit 的 `tunnel/proxy/api.go`：其状态，含每个 hub 的 node 及其按往返时间的份额（fit 的
  `Weights`）、转发和 web 代理；其 hub；向某 hub 的 node 询问信息或设置服务；其 meta；添加和删除转发；关闭隧道），在 I109
  的控制台上提供；`akit run` 不再拒绝它们的 `console`。
- **完成标准**：移植后的 fit node 和 proxy 页面测试通过；页面上的修改被应用、进入历史，不能加载时被拒绝。
- **涉及**：`akit-tunnel`。
- **fit**：`tunnel/node/api.go`、`tunnel/proxy/api.go`、`tunnel/internal/common/hubs.go`。
- **状态**：done（2026-11-27）。见进度日志。同时，角色控制台的证书在其文件变化时重新读取（决定第 45 和 61 行）。

### I109c LLM 角色的页面

- **做什么**：在 I109 的控制台上为 LLM 的 hub、node 和 proxy 提供 fit 的 `llm/internal/webui`：`GET /api/me`（用户、角色的身份、
  proxy 的 API URL），对角色路由器的每个方法提供 `POST /api/rpc<method>`，经 `akit_llm::local::Local`（I106b）以角色自己的
  页面身份在角色的池上调用，对其流方法提供 `POST /api/stream<method>`，以 Server-Sent Events 返回，同时至多 16 个，每帧只在
  浏览器取走后才还给流，每次等待至多 30 秒；编辑由方法自己的 `role::Base::edit` 写回；`akit run` 提供它们的 `console`。
- **完成标准**：移植后的 fit `webui` 测试（`api_test.go`）在 akit 的 hub、node 和 proxy 上通过。
- **涉及**：`akit-llm`。
- **fit**：`llm/internal/webui`、`llm/internal/role`（`applyConsole`）。
- **状态**：done（2026-11-27）。见进度日志。页面调用而角色尚缺的方法属于 I109d（决定第 47 行）。

### I109d LLM 角色供页面使用的方法

- **做什么**：fit 的 LLM 页面调用而 akit 的角色尚缺的方法：`/mesh/dial`、`/mesh/undial`、`/mesh/kick`、`/mesh/listen`、
  `/mesh/limits/get` 和 `/set`、`/mesh/tokens/list`、`/create`、`/expiry` 和 `/revoke`（fit 的 `llm/internal/role`），以及
  proxy 的 `/proxy/api/get` 和 `/set`、`/proxy/keys/list`、`/create` 和 `/delete`、`/proxy/models`、`/proxy/node/list`、
  `/proxy/profile/set` 和 `/proxy/stats`（fit 的 `llm/proxy`），每个修改都用 `role::Base::edit` 写回；身份的 `http_url`，
  即控制台的 URL。
- **完成标准**：移植后的 fit 角色和 proxy 方法测试通过；页面的调用与 fit 的应答一致。
- **涉及**：`akit-llm`。
- **fit**：`llm/internal/role/{role,settings,tokens}.go`、`llm/proxy/{keys,nodes,stats,role}.go`。
- **状态**：done（2026-11-27）。见进度日志。控制台的 URL 是其控制台服务器告诉角色的那个
  （`akit_roles::pages::Module::console_url`，决定第 64 行），控制台启动时才选定的端口也一样。

### WebRTC

fit 的 I87 和 I108–I111（其 [WebRTC 指南](https://github.com/lesismal/fit/blob/main/docs/webrtc.zh-CN.md)、其决定 029，
以及其包 `webrtc/turn`、`webrtc/media` 和 `config/webrtc.go`），分五次迭代（[决定 039](decisions/039-webrtc.zh-CN.md)）。
配置中的三个角色，由 I100 的宿主在监听器的 akio 引擎上运行：`webrtc.turn`，基于 `akio-turn` 的 TURN 服务器（RFC 8656）；
`webrtc.media`，`whip` 和 `whep` 处理器的 PeerConnection 共享的 UDP 端口，基于 `akio-webrtc` ICE lite `RtcServer` 的 SFU；
`webrtc.proxy`，`webrtc_proxy` 处理器后面的媒体服务器的 peer 经由其中继的 UDP 端口，每个新 peer 按其首个 STUN 请求的
ICE ufrag 路由。它们对外显示的是 akit 的名字（决定 037：`SOFTWARE` 为 `akit`，指标 `akit_turn_*` 和 `akit_webrtc_*`）；
线上协议是 RFC 的，因此 fit 和 akio 的客户端、浏览器和 pion 都能同样访问它们。处理器的 `auth` 引用一个 `media_auth`
策略，属于流媒体与媒体鉴权路线：在该路线读取 `media_auth` 之前，它作为尚不支持被拒绝。

### I110 WebRTC I：配置中的 `webrtc`

- **做什么**：`webrtc.turn`、`webrtc.media` 和 `webrtc.proxy` 与其余配置一样严格读取，字段、默认值、范围和检查与 fit 的
  相同（`config/webrtc.go`）：TURN 服务器的 `listen`、`listen_tcp`、带 `tls.certificates` 的 `listen_tls`、`realm`、`users`
  （密码或由其派生的密钥）、`secret`、同一地址族的 `relay_ip` 和 `public_ip`、端口范围、配额、上限、生存期、
  `allowed_peers` 和 `denied_peers`（前缀、地址，或地址加端口）；媒体服务器的 `listen`、`public_ips`、`ice_servers`
  （`stun:`、`stuns:`、`turn:`、`turns:`，可安全放入 `Link` 头）、`turn_urls`（需要 `webrtc.turn` 的 `secret`）、`codecs`
  和各项限制；代理的 `listen`、`public_ips`、各项限制、`relay_ip` 和端口范围；各自的 `access`。它们的地址加入"一个地址
  一个服务器"的检查（fit 的 `config/bindings.go`）。处理器 `whip`、`whep`（`stream`，由 1 到 128 个 `[A-Za-z0-9._~-]`
  组成的名字）和 `webrtc_proxy`（`upstream`、`media_addrs`、`timeout`、`max_sdp_bytes`、`action`），其 `auth` 作为尚不
  支持被拒绝。在角色和处理器由其迭代运行之前，`akit run` 拒绝它们。
- **完成标准**：移植后的 fit `config/webrtc_test.go` 以及 `bindings_test.go` 和 `more_test.go` 的 WebRTC 用例通过；
  fit-examples 的 `webrtc-turn`、`whip-whep` 和 `webrtc-proxy` 通过门禁；配置参考列出每个字段；v1 冻结把它们标为新增。
- **涉及**：`akit-config`（`webrtc`）、`akit`、`tests/compat`、`docs/reference`。
- **fit**：`config/webrtc.go`、`config/bindings.go`、`webrtc/media/handler.go`、`webrtc/media/proxyhttp.go`；I87、I109、I110。
- **状态**：已完成（2026-11-27）。见进度日志。处理器在其角色运行之前回答 503（同 fit），`akit run` 拒绝它们。

### I111 WebRTC II：TURN 服务器

- **做什么**：基于 `akio-turn` 的 `TurnServer` 的 `webrtc.turn`，由 `akit run` 运行：`listen` 上的 UDP（每个地址一个绑定，
  `access` 过滤新客户端的数据报）、`listen_tcp` 上的 TCP 和 `listen_tls` 上的 TLS（`TurnServer::stream`，akio-tls 使用来自
  文件的证书）；用户的密钥和 coturn 风格的 `secret`（限时凭据）、realm、`relay_ip` 和 `public_ip`；`max_allocations`（508）、
  `max_allocations_per_user`（486）、`max_permissions`、`max_channels` 和各生存期；在 akio-turn 对内部地址的拒绝之上的
  peer 规则，感知端口，akit 自己的地址只能在 `allowed_peers` 指明的端口上访问；重新加载时就地修改用户、secret、peer 规则
  和 access，其余变化启动新服务器，旧服务器在 `-shutdown-timeout` 内排空；`webrtc` 日志器；服务器的计数器进入 `/metrics`。
  akio-turn 缺少 fit 的钩子之处（中继端口范围、用户名的带宽、事件、就地修改的限制、排空、nonce 密钥、流的上限），提交
  akio 请求并记录临时方案。
- **完成标准**：在 akio-turn 能支持之处，移植后的 fit `webrtc/turn` 测试通过，使用 `akio-turn` 的客户端和 fib 的客户端
  （从 fit 的模块树构建的 Go 程序）：allocation 经 indication 和 channel 与 peer 双向中继，基于 UDP、TCP 和 TLS；错误的
  密码和过期的限时凭据被拒绝；配额以 486 和 508 拒绝；私有 peer 除非被允许否则被拒绝；重新加载用户保留 allocation，
  重新加载地址则迁移服务器。
- **涉及**：`akit-webrtc`（新，`turn`）、`akit`、`akit-roles`。
- **fit**：`webrtc/turn`；I87、I111。
- **状态**：已完成（2026-11-27）。见进度日志。在 akio 的 A256-A258 之前的临时方案（决定 039）：运行时拒绝端口范围；带宽、
  nonce secret 和流的上限不生效。

### I112 WebRTC III：基于 akio 钩子的 TURN 服务器

- **做什么**：akio 满足其请求后替换 I111 的临时方案：从 `port_min` 到 `port_max` 的中继、`user_bandwidth`（每个用户名
  和方向一个令牌桶，超出的数据报被丢弃并计数）、计数并记录的事件（分配、刷新、按原因释放、按错误码拒绝、凭据失败）、
  就地修改的配额、上限和生存期、拒绝新 allocation 而让已有 allocation 自然结束的排空、`nonce_secret`、
  `max_stream_message` 和 `stream_idle_timeout`、服务器的客户端可访问服务器自己的中继地址。
- **等待**：akio API 请求 A256、A257 和 A258。
- **完成标准**：移植后的 fit I111 的 `webrtc/turn` 测试通过（`turn_test.go` 的带宽、事件、上限和排空用例，`apply_test.go`）。
- **涉及**：`akit-webrtc`。
- **fit**：`webrtc/turn`、`webrtc/turn/limiter.go`；I111。
- **状态**：已完成（2026-11-27）。见进度日志。

### I113 WebRTC IV：WHIP 和 WHEP

- **做什么**：基于 `akio-webrtc` `RtcServer` 的 `webrtc.media`（ICE lite，所有 PeerConnection 共用一个 UDP 端口，`access`
  过滤新地址的数据报），以及处理器 `whip` 和 `whep`：用发布者各类媒体中第一个列于 `codecs` 的编解码器应答其 offer，
  其媒体转发给其流的每个观看者，观看者的关键帧请求最多每 `key_frame_interval` 转给它一次；`max_bitrate` 以 REMB 告知，
  并以两秒的突发量强制执行；按 RFC 9725 和 WHEP 草案处理 DELETE 和 OPTIONS（PATCH 回答 405，同 fit）；`Link` 头中的
  ICE 服务器和带有由 `webrtc.turn` 的 secret 生成凭据的 `turn_urls`；各项限制（PeerConnection、流、每个流的观看者、
  offer 的大小）、`connect_timeout` 和 `ice_timeout`；保持 `listen` 和 `public_ips` 不变的重新加载保留会话，否则结束会话；
  `/metrics`。
- **等待**：没有阻塞它的：akio 请求 A259（每个 peer 自己的配置）、A260（RTP 转发）和 A261（发送 REMB、ICE 超时）都有
  计划的临时方案，其中包括以转发帧代替转发包。
- **完成标准**：移植后的 fit `webrtc/media` 媒体服务器测试通过，以 akio-webrtc 的 `Endpoint` 作为发布者和观看者，fib 的
  PeerConnection（Go 程序）经 akit 发布和播放：发布者离开时其观看者结束；慢的和断开的观看者不拖住发布者；限制生效。
- **涉及**：`akit-webrtc`（`media`）、`akit-handler` 或 `akit`、`akit-roles`。
- **fit**：`webrtc/media/{media,session,handler}.go`；I109。
- **状态**：已完成（2026-11-27）。见进度日志。

### I114 WebRTC V：代理的 WHIP 和 WHEP

- **做什么**：处理器 `webrtc_proxy` 和角色 `webrtc.proxy`：offer 发给路由 upstream 中的一个媒体服务器（其选择、健康检查和
  超时），应答中的 candidate 换成代理的端口，其 ufrag 与媒体服务器的地址对应记下（`media_addrs`，或应答中的第一个 host
  或 server-reflexive candidate），会话的 URL 改写为 akit 的；UDP 端口按新 peer 地址首个 STUN Binding 请求的 USERNAME
  路由，此后经其自己的一个朝向媒体服务器的套接字（`relay_ip`、端口范围）中继其五元组，带 `max_sessions`、`max_relays`
  （每个会话 8 个）、`session_timeout` 和 `idle_timeout`。
- **完成标准**：移植后的 fit `proxy_test.go`、`proxy_edges_test.go` 和 `proxy_ports_test.go` 通过：一个 akit 端口后的两个
  媒体服务器各自得到自己的 peer；peer 在其 Binding 请求之前的数据报被丢弃；离开的媒体服务器释放其路由。
- **涉及**：`akit-webrtc`（`proxy`）、`akit-upstream`、`akit`。
- **fit**：`webrtc/media/{proxy,proxyhttp,relay}.go`；I110、I111。
- **状态**：已完成（2026-11-27）。见进度日志。

### 流媒体与媒体认证

fit 的 I115–I117（其[流媒体指南](https://github.com/lesismal/fit/blob/main/docs/streaming.zh-CN.md)、其决定 030，以及其包
`mediaauth`、`streaming` 和 `handler/mediapages`、`config/mediaauth.go` 和 `config/streaming.go`），分七次迭代
（[决定 039](decisions/039-streaming-and-media-auth.zh-CN.md)）。`media_auth` 命名规定谁可以推送和播放哪些流的策略；`streaming`
在监听器的引擎上、与网关并列运行一个基于 akio-media 的媒体中心，带有自己的 RTMP、RTSP 和 SRT 监听器，以及通过 HTTP-FLV、HLS
和 WHEP 播放其流的处理器；fit 的媒体页面是基于 WHIP 和 WHEP 的房间通话以及推流和播放页面。akit 对外显示的名字都是 akit 的
（决定 037：cookie `akit_media_token`、`akit media-token`、指标 `akit_streaming_*`）；akit 签发的 token 采用 fit 的格式，使双方
互认对方的 token（决定 039）。媒体协议都是标准协议，因此 akit 的角色与 fit 的角色可通过 ffmpeg 和标准客户端互通。fit 之后在
这些成员上新增的内容（I119 的密钥环和 `media_tokens`、媒体库的频道、推送目标和签名 URL）属于“媒体库与频道”路线，在此之前作为
“尚不支持”拒绝。

### I120 流媒体与媒体认证 I：配置中的 `media_auth` 和 `streaming`

- **做什么**：两个成员与其余配置一样严格读取，字段、默认值、范围和检查与 fit 的相同（`config/mediaauth.go`、
  `config/streaming.go`）：策略的名字、token（或其 SHA-256）、用户（密码或 bcrypt 哈希）、`secret` 或 `secret_file`、`public`、
  `token_ttl` 和流模式；角色的上限、其 `rtmp`、`rtsp`、`srt` 和 `webrtc` 监听器及其超时和上限，以及其 `access`；监听器跨文件
  引用的策略；其地址参与“一个地址一个服务器”的检查。作为“尚不支持”拒绝：策略的 `keys` 和 `url_signing`（fit 的 I119、媒体库）、
  角色的 `channels` 和 `push`（媒体库）、`mptcp`、RTMP 的 `max_message_bytes`（akio A263）、RTSP 的 `auth_methods`、`realm` 和
  `idle_timeout`（akio A264）。在 I121 和 I122 运行它们之前，`akit run` 拒绝带有这两个成员之一的配置。
- **完成标准**：移植后的 fit `config/mediaauth_test.go` 和 `streaming_test.go` 用例通过（akit 作为“尚不支持”拒绝的用例按拒绝
  处理）；fit-examples 的 `streaming` 除其处理器外通过门禁；配置参考列出每个字段；v1 冻结把它们标为新增。
- **涉及**：`akit-config`（`media`、`binds`）、`akit-roles`（`members`）、`akit`、`tests/compat`、`docs/reference`。
- **fit**：`config/mediaauth.go`、`config/streaming.go`、`config/bindings.go`；I115、I116。
- **状态**：已完成（2026-11-27）。见进度日志。

### I121 媒体认证 II：策略、检查、`akit media-token`

- **做什么**：把 fit 的 `mediaauth` 包做成新 crate `akit-media`：静态 token 和签名 token（fit 的格式）、带明文密码（常数时间比较）
  或 bcrypt 哈希的用户（在 crate 自己的有界池上检查，与 fit 相同：CPU 数的四分之一、最多 256 个等待、5 秒；验证正确的密码以策略
  随机密钥下的 HMAC 记住）、公开的动作、流模式；HTTP 请求的凭据（`Bearer`、`Basic`、`?token=`、cookie `akit_media_token`）和
  URL 查询中的凭据；HTTP 请求的守卫（401 带 Bearer 质询、403、503 带 `Retry-After`；bcrypt 检查期间挂起请求）；查询参数中的
  token 在访问日志中写为 `REDACTED`；按引擎和名字查找的策略，由 `media_auth` 角色在每次应用配置时就地修改；`secret_file` 在每次
  应用时读取并由 `-watch-certs` 监视；`akit media-token`。
- **完成标准**：移植后的 fit `mediaauth_test.go` 用例（匹配、允许、公开、bcrypt、签发、注册表、密钥文件）和 `TestGuard` 通过；
  接受 fit 的 `media-token` 签发的 token，且对相同声明 akit 签发的 token 与 fit 的逐字节相同；`akit media-token` 签发策略接受的
  token，并像 fit 一样拒绝错误用法。
- **涉及**：`akit-media`（新增：`auth`、`http`、`role`）、`akit`（`media-token`、`ROLES`）。
- **fit**：`mediaauth/mediaauth.go`、`http.go`、`registry.go`、`pool.go`、`cli/mediatoken.go`；I115。
- **状态**：已完成（2026-11-27）。见进度日志。

### I122 流媒体 I：角色的媒体中心及其 RTMP、RTSP 和 SRT 监听器

- **做什么**：`akit run` 基于 akio-media 的 hub 运行 `streaming`，其监听器加到监听器的引擎上：RTMP、RTSP（交织 RTP；请求 UDP 的
  SETUP 答复 461）和 SRT，各自带 `auth`（RTMP 的查询参数、SRT 的 stream ID、在 akio A264 之前 RTSP 的 URL 查询参数）、
  `max_conns`、超时和 `play_wait`，以及基于 akio 接入过滤器的角色 `access` 和全局 `access`；重载时就地修改 `auth` 和
  `access`，其他字段变化的监听器重启，上限变化时重启 hub；bcrypt 用户在池检查密码期间以各协议的重试方式拒绝一次；指标
  `akit_streaming_*`。
- **完成标准**：移植后的 fit `streaming_test.go`、`listeners_test.go`、`limits_test.go` 和 `access_test.go` 用例通过，按 hub 的
  状态而非时间同步；ffmpeg 通过 RTMP、RTSP 和 SRT（若其构建带 libsrt）用有效和无效凭据推流并分别回放，同样的命令对 fit 的二进制
  也通过；上限和重载都成立。
- **涉及**：`akit-media`（`streaming`）、`akit`（`ROLES`）、`docs/reference`。
- **fit**：`streaming/streaming.go`、`listeners.go`、`rtspauth.go`；I116。
- **状态**：todo。

### I123 流媒体 II：`httpflv` 和 `hls`

- **做什么**：基于 akio-media 的 `HttpFlv` 和 `Hls` 的处理器，带 `prefix`、`auth`（I121 的守卫）、`wait` 和 `play_wait`、`cors`；
  HLS 的 `format`（另一种格式用 `?format=`）、`target_duration`、`part_duration`（LL-HLS）、`playlist_size`、`max_bytes` 和
  `idle_timeout`，参数不变的重载保留其分片，带 `?token=` 获取的播放列表把 token 交给其中的每个 URI；角色未运行时答复 503。
- **完成标准**：移植后的 fit 处理器测试通过；HTTP-FLV、WebSocket-FLV 和 HLS（fMP4 和 MPEG-TS）带凭据可以播放、不带则被拒绝；
  门禁除 `stream_whip` 和 `stream_whep` 外接受 fit-examples 的 `streaming`。
- **涉及**：`akit-media`（`handlers`）、`akit-config`、`akit`、`tests/compat`、`docs/reference`。
- **fit**：`streaming/handlers.go`；I116。
- **状态**：todo。

### I124 流媒体 III：`streaming.webrtc`、`stream_whip` 和 `stream_whep`

- **做什么**：基于 akio-media 的 `webrtc` 特性（akio-webrtc，单个 UDP 端口上的 ICE lite 代理）的角色 WebRTC 端口，以及向 hub
  推送和从 hub 播放的 `stream_whip` 和 `stream_whep`：POST 上的 `auth`、会话的 PATCH 和 DELETE 不需要凭据、`max_sessions`、
  `connect_timeout`、`play_wait`、`max_offer_bytes`、`public_ips`，以及放在 `Link` 头字段中的 `ice_servers`。
- **完成标准**：移植后的 fit WHIP 和 WHEP 测试通过；通过 WHIP 推送的流可通过 RTMP 和 HLS 播放，通过 RTMP 推送的流可通过 WHEP
  播放；fit-examples 的 `streaming` 有效。
- **涉及**：`akit-media`、`akit-config`、`tests/compat`、`docs/reference`。
- **fit**：`streaming/handlers.go`、`listeners.go`（`applyWebRTC`）；I116。
- **状态**：todo。

### I125 媒体页面

- **做什么**：fit 的 I117：处理器 `media_pages`，其资源为改名后的 fit 资源（决定 037），基于策略用户的登录，以 HttpOnly cookie
  `akit_media_token` 签发签名 token，以及有界的房间 API；移植 I118 时一并加入其聊天。
- **完成标准**：移植后的 fit `handler/mediapages` 测试通过；fit-examples 的 `media-pages` 有效。
- **涉及**：`akit-media`（`pages`）、`akit-config`、`tests/compat`、`docs/reference`。
- **fit**：`handler/mediapages`；I117、I118。
- **状态**：todo；需要 WebRTC 路线的 `whip` 和 `whep`。

### I126 WebRTC 路线的处理器上的媒体认证

- **做什么**：按 fit 的 I115，在 `whip`、`whep` 和 `webrtc_proxy`（带 `action`）上用 I121 的守卫实现 `auth`，会话的 DELETE 不需要
  凭据。
- **完成标准**：移植后的 fit 中这些处理器上 `auth` 的测试通过。
- **涉及**：WebRTC 路线的处理器、`akit-config`。
- **fit**：`webrtc/media`；I115。
- **状态**：todo；需要 WebRTC 路线的处理器。
### BitTorrent

fit 的 I196–I210（其 [BitTorrent 指南](https://github.com/lesismal/fit/blob/main/docs/bittorrent.md)、
其决定 037、其包 `bittorrent`、`bittorrent/tracker`、`handler/btstream`、`handler/btpublish`、
`config/bittorrent*.go`），分为十个迭代。配置中的两个角色：`bittorrent.client` 下载并做种，种子保存在
其 `data_dir` 中跨重启保留；`bittorrent.tracker` 应答 announce 和 scrape，HTTP 经由路由的
`bt_tracker` 处理器，UDP 在其自己的 `listen` 上。它们运行在 akio-bittorrent 的 `Client` 和
`TrackerServer` 上，与网关并列，由 `akit run`（I100 的宿主）运行。fit 为它们向 fib 提出的需求
（fib 的 A75–A79、A85–A88），akio-bittorrent 缺少的部分 akit 向 akio 提出（A283–A288）：客户端运行在
监听器的引擎上、按地址拒绝 peer 并在运行时修改限制、tracker 存储的上限原地修改。角色的文件与 fit
相同（`torrents.json`、`torrents/<id>.torrent`），使数据目录可在 fit 与 akit 之间迁移；resume 数据
是 akio 自己的格式，fit 的会被拒绝并重新校验数据（决定第 75 行）。没有任何东西阻塞 I/O 线程：存储
和哈希在 akio-bittorrent 的磁盘池上运行，状态文件在角色自己的线程上写，tracker 的 announce 只是一次
查找加一个分片锁。fit 的 I205（互通、故障测试）放在每个角色自己的迭代中：akit 的 tracker 对 fit 的
客户端和 aria2c，akit 的客户端对 fit 的 tracker 和做种者，只在回环地址上。fit 的 I206（集群查看
成员的种子）等待集群管理路线。

### I150 BitTorrent I：配置中的 `bittorrent`

- **做什么**：`bittorrent.client` 和 `bittorrent.tracker` 与其余部分一样严格读取，字段、默认值、
  上限和检查与 fit 相同（`config/bittorrent.go`、`config/bittorrent_tracker.go`）：客户端的
  `listen` 和 `data_dir`（必填）、`download_dir`、`encryption`、`dht`、`pex`、`utp`、`lsd`、
  `disk_workers`、`disk_queue_bytes`、`save_interval`、`limits` 和 `console`；tracker 的 `listen`、
  `interval`、`min_interval`、`peer_ttl`、其存储的上限、`max_numwant`、`max_scrape`、`allow`、
  `allow_files`、`access` 和 `console`。客户端的端口以 TCP 和 UDP 进入"一个地址一个服务器"的检查，
  tracker 的以 UDP 进入（fit 的 `bindings.go`）；`bittorrent` 日志器。后续迭代才运行的内容以"尚不
  支持"拒绝并指明字段：`categories`、`watch_dir` 及其上限、`url_timeout`、`alt_speed`、客户端的
  `access`、akio 尚不能设置的限制（`max_conns`、`max_half_open`、`max_active_*`、`min_free_bytes`）、
  `publish`、`rss`、tracker 的 `auth`（media_auth）和 `store`。在角色的迭代运行它之前，`akit run`
  拒绝含有该角色的配置。
- **完成标准**：移植后的 fit `config/bittorrent_test.go` 和 `bittorrent_tracker_test.go` 用例通过；
  fit-examples 的 `bittorrent-*` 通过兼容性闸门，或只以"尚不支持"失败；参考手册列出每个字段；v1
  冻结把它们标为新增。
- **涉及**：`akit-config`（`bittorrent`）、`akit`、`tests/compat`、`docs/reference`。
- **fit**：`config/bittorrent.go`、`config/bittorrent_tracker.go`、`config/bindings.go`；I197、I201。
- **状态**：done（2026-11-27）。见进度日志。

### I151 BitTorrent II：tracker 角色

- **做什么**：基于 akio-bittorrent 的 `TrackerServer` 的 `bittorrent.tracker`，在新 crate
  `akit-bittorrent` 中：HTTP 的 announce 和 scrape 经由 `bt_tracker` 处理器（fit 的
  `bittorrent/tracker/handler.go`：按路径最后一段区分、拒绝以 failure reason 应答、角色未运行时
  503），UDP 在 `listen` 上加入监听器的引擎，角色的访问控制在 akio 的过滤器处执行，BEP 15 的连接 ID
  以跨重载保留的密钥生成；fit 的应答（`unregistered torrent`、`min interval`、默认 50 个 peer、至多
  `max_numwant`、UDP 应答不超过 1,400 字节、`max_scrape`）；允许列表可内联也可在文件中，文件变化时
  重新读取；HTTP 的访问控制在处理器中检查；统计（announce、scrape、按原因计的拒绝）；控制台的只读
  路由（`/api/stats`、`/api/torrents`、`/api/torrents/<hash>`）。只有允许列表、访问控制或应答变化的
  重载保留 swarm；存储上限的变化在 akio 的 A285 之前会建立新存储。
- **完成标准**：移植后的 fit tracker 测试通过（HTTP 和 UDP 的 announce 与 scrape、允许列表及其文件、
  访问控制、上限、重载、逐字节发送的客户端、垃圾数据报）；fit 的客户端和 aria2c（若已安装）向
  akit 的 tracker announce，两个 fit 客户端经由它互相找到。
- **涉及**：`akit-bittorrent`（新）、`akit-config`、`akit`。
- **fit**：`bittorrent/tracker`；I201、I209。
- **状态**：done（2026-11-27）。见进度日志。在 akio 的 A285 之前的临时做法：决定第 77–79 行。

### I152 BitTorrent III：客户端角色

- **做什么**：基于 akio-bittorrent 的 `Client` 的 `bittorrent.client`（在 akio 的 A283 之前使用
  它自己的引擎）：状态以 fit 的格式保存在 `data_dir` 中（`torrents.json`、`torrents/<id>.torrent`，
  resume 数据在 `resume/` 中），先写临时文件、同步再改名，由角色自己的线程至多每 `save_interval`
  写一次，种子列表变化时立即写，停止时也写；启动时恢复而不重新校验；重载原地修改速率和做种限制，
  当 `listen`、`encryption`、`dht`、`pex`、`utp`、`lsd`、`disk_workers` 或 `disk_queue_bytes`
  变化时建立新客户端（种子转移过去，不经校验继续），新的 `data_dir` 从该目录的状态开始；端口被占用时
  恢复旧客户端。角色的 API（以磁力链接或 .torrent 文件添加、删除时保留或删除数据、暂停、继续、列出、
  状态）和控制台的只读路由（`/api/torrents`、`/api/torrents/<id>`）。
- **完成标准**：移植后的 fit 角色测试通过（两个角色在回环地址上下载、重启后不读数据即继续、磁力链接的
  .torrent 被保存并从中恢复、重载限制时保留 peer、`listen` 变化时保留种子、损坏和过大的状态文件、
  损坏的种子仍被列出、API 的拒绝）；akit 的客户端从经由 fit 的 tracker 找到的 fit 做种客户端下载，
  fit 的客户端也从 akit 的下载。
- **涉及**：`akit-bittorrent`、`akit`。
- **fit**：`bittorrent/{role,state,api}.go`；I197。
- **状态**：done（2026-11-27）。见进度日志。在 akio 的 A283、A284 和 A286 之前的临时做法：决定第 75 行。

### I153 BitTorrent IV：添加种子、分类、队列

- **做什么**：同 fit 的 I198：基于 akio HTTP 客户端的 .torrent URL（32 MiB、5 次重定向、`url_timeout`、
  同时 4 个）、`watch_dir`（.torrent 和 .magnet 文件，`.done` 和 `.failed`）、分类（目录、限制、在该
  路线落地后对 `media_library` 重新扫描）、队列（`max_active_downloads`、`max_active_seeds`）、磁盘
  检查（`min_free_bytes`，akio 的 A249）。种子自己的下载目录需要 akio-bittorrent 为磁力链接提供的存储
  （A286）。
- **涉及**：`akit-bittorrent`、`akit-config`。
- **fit**：`bittorrent/{manage,fetch,watch}.go`；I198。

### I154 BitTorrent V：时间表、单个种子的限制、客户端的访问控制

- **做什么**：同 fit 的 I199：带每周时间表和时区的 `alt_speed`、分类和单个种子的限制、客户端的
  `access`（akio 的 A284：按地址拒绝 peer，重载拒绝它们时关闭连接），以及 akio 在运行时设置的连接
  限制（`max_conns`、`max_half_open`、`upload_slots`，A284）。
- **涉及**：`akit-bittorrent`、`akit-config`。
- **fit**：`bittorrent/manage.go`；I199。

### I155 BitTorrent VI：`bt_stream`

- **做什么**：同 fit 的 I200：种子下载期间按范围提供其文件，被请求的范围先读（akio-bittorrent 的一个
  等待分片的读取器，A287）、`max_streams`、超时、从磁力链接添加的临时种子（`auth` 在 media_auth
  路线之后）。
- **涉及**：`akit-bittorrent`。
- **fit**：`handler/btstream`；I200。

### I156 BitTorrent VII：发布

- **做什么**：同 fit 的 I202 和 I209：`bittorrent.client.publish` 和 `bt_publish` 处理器，目录在有界
  池上构建为混合种子，原地做种，web seed 指向 akit 的静态文件，跟随磁盘变化。
- **涉及**：`akit-bittorrent`、`akit-config`。
- **fit**：`bittorrent/{publish,publish_walk}.go`、`handler/btpublish`；I202、I209。

### I157 BitTorrent VIII：Admin API、事件、`akit bt`、指标

- **做什么**：同 fit 的 I203 和 I210：Admin API 上的 `/bittorrent/...`、事件流、`akit bt`、按分类和
  状态的指标；每个 tracker 的 announce 和 web seed 的状态（A288）、原地重新校验。
- **涉及**：`akit-bittorrent`、`akit-admin`、`akit`、`akit-observe`。
- **fit**：`admin/bittorrent.go`、`admin/btevents.go`、`cli/bt.go`、`bittorrent/{events,inspect,
  metrics}.go`；I203、I210。

### I158 BitTorrent IX：页面

- **做什么**：同 fit 的 I204：客户端和 tracker 的页面，基于控制台机制（`akit_roles::pages`）和 I157
  的 API。
- **涉及**：`akit-bittorrent`、`akit-admin`。
- **fit**：workbench 的 BitTorrent 页面；I204。

### I159 BitTorrent X：RSS 与实例共享的 tracker 存储

- **做什么**：同 fit 的 I207 和 I208：RSS 自动下载（`rss`），以及 tracker 在 Redis 中的 `store`；
  在读取 `media_auth` 之后（流媒体与媒体鉴权路线）支持私有 tracker 的 passkey。
- **涉及**：`akit-bittorrent`、`akit-config`。
- **fit**：`bittorrent/rss*.go`、`bittorrent/tracker/sharedstore`；I207、I208。

### 集群管理

fit 的 I140–I150（其[集群指南](https://github.com/lesismal/fit/blob/main/docs/cluster.zh-CN.md)、
其[决定 033](https://github.com/lesismal/fit/blob/main/docs/decisions/033-cluster-management.zh-CN.md)、
其包 `cluster`、`admin/node.go` 和 `config/cluster.go`），分十二次迭代（[决定 041](decisions/041-cluster-management.zh-CN.md)）；
fit 的 I153（跨集群的助手）随 AI 助手路线进行，因为它需要该路线。一个设置了 `admin.cluster` 的 akit 管理其他 akit，即它的
成员：它把成员的 URL 和凭据保存在自己的存储中，轮询每个成员的状态，并在控制台的集群页面中展示它们，这些页面就是 I91 复制的
fit 工作台页面。成员尽量少改动：管理端使用成员已有的 API，即它的控制台或 Admin API。线上协议沿用 fit 的（决定 041）：成员的
`GET /node`、`/status` 和控制台登录的回答与 fit 的相同，管理端存储中的 `cluster` 分区就是 fit 的文档，因此 akit 和 fit
的成员可以共用任一种管理端，akit 与 fit 的管理端也能打开彼此的存储。默认关闭：没有 `admin.cluster` 就没有集群菜单，也不会
向外发起调用。

### I130 集群 I：配置中的 `admin.node_id`、`admin.cluster` 和 `admin.cluster_member`

- **做什么**：三者按 fit 的方式读取（`config/cluster.go`、`checkCluster`、`checkClusterMember`、节点 ID 的模式）：
  `admin.node_id`（1 到 64 个 `[A-Za-z0-9._-]` 字符，以字母或数字开头）；`admin.cluster`（`poll_interval` 10s、`timeout`
  10s，至少 1s；`max_nodes` 256，最多 10,000；`max_concurrency` 8，最多 256；`max_response_bytes` 4 MiB，16 KiB 到 64 MiB；
  `link_address`、`link_url`（wss，需与 `link_address` 同设）；`admins`，每个控制台用户只列一次，至多 64 个），需要
  `admin.console`；`admin.cluster_member`（`manager` 为 wss URL，`fingerprint` 为 64 个十六进制数字，`token`（`akite1.`，即
  fit 的 `fite1.` 改名，决定 037）或 `token_file`）。在运行它们的迭代（I131、I132、I139b）之前，`akit run` 拒绝每一项并
  指名。
- **完成标准**：fit 对这三者的 `config` 测试移植后通过；门禁的示例有效；字段用 `make v1-bless` 标记；参考文档重新生成。
- **涉及**：`akit-config`、`akit`、`tests/compat`、`docs/reference`。
- **fit**：`config/cluster.go`、`config/validate.go`；I140、I141、I149。
- **状态**：done（2026-11-27）。`akit_config::cluster`；`akit run` 拒绝 `admin.cluster` 和 `admin.cluster_member` 并指名其迭代
  （决定第 74 行）。见进度日志。

### I131 集群 II：成员需要什么

- **做什么**：在每个 akit 上实现 fit 的 I140：`GET /node`（以及控制台的 `/api/node`，I90 已转发）：节点 ID 只生成一次（随机
  UUIDv4），保存在存储的 `node` 分区（fit 的），或配置旁的文件（`akit.json` 旁的 `akit.node-id`），或由 `admin.node_id`
  给出；主机名、akit 与 akio 的版本、启动时间、运行中配置的版本、ETag、时间和来源、运行中的角色、schema 的版本和摘要，以及
  以 fit 的能力名列出 akit API 提供的功能；`GET /schema`。控制台登录已经回答 CSRF 令牌，I89 已按 fit 的方式放慢失败的登录。
- **完成标准**：fit 的 `admin/node_test.go` 移植后通过：有无存储时重启后 ID 不变，文件中的 ID 被并入存储，`admin.node_id`
  优先；能力只列出实际提供的；账户页面显示节点。
- **涉及**：`akit-admin`、`akit`（`run`）、`docs`。
- **fit**：`admin/node.go`；I140。
- **状态**：done（2026-11-27）。`akit_admin::node`。fit 的 `go_version` 回答为空（决定第 71 行）。见进度日志。

### I132 集群 III：管理端、它的成员及其状态

- **做什么**：以新 crate `akit-cluster` 实现 fit 的 I141：存储的 `cluster` 分区，即 fit 的文档（读取版本 1 到 3；akit 尚未
  用到的部分——分组、模板、发布、链接、角色——原样保留），更新版本的文档只读；成员有名字、标签、URL 和访问方式（`console`，
  遇 401 重新登录；`admin_api`，bearer token），凭据从不出现在回答或日志中，也只发给其成员；TLS 由系统根证书、CA PEM 或
  固定的 SHA-256（akio-tls 的 `pin_sha256`）校验；`http` 只允许回环地址。轮询器是一个独立线程，在 akio 的异步 HTTP 客户端
  上发起调用（规则 1），同时最多 `max_concurrency` 个成员，每个成员每 `poll_interval` 一次，读取 `/node`、`/status`，以及
  成员提供时的 `/config/check`，每次调用受 `timeout` 和 `max_response_bytes` 约束；登录退避和限速按 fit 的方式。控制台模块
  `cluster`：`GET/POST/PUT/DELETE /api/m/cluster/nodes[/<name>]`、`POST .../nodes/<name>/test` 和 `POST /api/test`（回答
  证书指纹，供人确认）、`GET /api/m/cluster/state`、`/audit` 和 `/me`；添加、修改和删除的审计日志。
- **完成标准**：fit 关于成员、轮询器和 API 的 `cluster` 测试针对替身成员移植后通过；一个管理端同时读取一个 akit 成员和一个
  fit 成员（fit 从源码构建），分别用密码和 token；错误的密码、不受信任的证书、不匹配的固定指纹和下线的成员能被区分；任何回答
  或日志中都不出现凭据；fit 管理端写的存储能在 akit 中打开，反之亦然。
- **涉及**：`akit-cluster`（新）、`akit-admin`（`console`）、`akit`（`run`）、`docs`。
- **fit**：`cluster/{cluster,members,poll,transport,probe,read,call,audit}.go`；I141。
- **状态**：done（2026-11-27）。在 akio 的 A277 之前，成员的证书由一次独立的探测读取；设置了用户角色的文档在 I139 之前
  只读。见进度日志。

### I133 集群 IV：集群页面、概览和节点页面

- **做什么**：fit 的 I142：控制台列出模块 `cluster`，工作台因而画出 I91 复制的集群菜单、概览和节点页面；成员页面经管理端
  读取成员（`GET /api/m/cluster/nodes/<name>/read/{node,status,versions,check}`，同时最多 8 个）；双语的
  `docs/guides/cluster.md`。
- **完成标准**：在浏览器中添加、测试、信任、编辑和删除一个成员；概览显示 I132 的每种故障并能恢复；两种语言。
- **涉及**：`akit-cluster`、`akit-admin`、`docs/guides`。
- **fit**：`cluster/read.go`、`app/workbench/web/cluster`；I142。
- **状态**：done（2026-11-27），随 I132 完成：模块的提供、读取、指南；已在浏览器中检查（概览、节点、成员页面）。菜单中
  后面的项在各自迭代之前回答 "no such endpoint"。见进度日志。

### I134 集群 V：从管理端打开成员自己的页面

- **做什么**：fit 的 I143：`/api/n/<member>/...` 带着保存的会话转发给成员，成员的 schema 从成员读取，写操作记入审计，schema
  不同的成员只读。
- **完成标准**：fit 的 `passthrough` 测试移植后通过；每个配置页面都能读取并保存 akit 和 fit 成员的配置。
- **涉及**：`akit-cluster`、`akit-admin`（`console`）。
- **fit**：`cluster/passthrough.go`；I143。
- **状态**：done（2026-11-27）。fit 成员的 `Fit-Config-Version` 和 `Fit-Dry-Run` 以 akit 的名字返回；它们只读打开
  （schema 不同）。见进度日志。

### I135 集群 VI：分组和对多个成员的操作

- **做什么**：fit 的 I144：按标签选择器或名字分组，带变量；对一个分组或一组选中成员的操作（重载、回滚、检查、按键、前缀或标签
  清除缓存、测试），有并发上限，按成员给出结果；`/api/m/cluster/groups`、`/ops`。
- **完成标准**：fit 的 `groups` 和 `ops` 测试移植后通过。
- **涉及**：`akit-cluster`、`akit-admin`（`cache`：按前缀清除，fit 的 `cache.prefix`）。
- **fit**：`cluster/{groups,ops}.go`；I144。
- **状态**：done（2026-11-27）。akit 的 `POST /cache/purge` 本已接受 `prefixes`；在 I138 提供 `/config/check` 之前，`check` 对
  akit 成员回答 `unsupported`。见进度日志。

### I136 集群 VII：模板

- **做什么**：fit 的 I145：模板拥有顶层成员和具名元素，有变量和顺序，每个成员有同步状态，可从成员导入，可比较两个成员；成员上的
  `PUT /node/managed`。
- **完成标准**：fit 的 `templates`、`render` 和 `sync` 测试移植后通过。
- **涉及**：`akit-cluster`、`akit-admin`（`/node/managed`）。
- **fit**：`cluster/{templates,render,sync,text,jsonv}.go`、`admin/managed.go`；I145。
- **状态**：done（2026-11-27）。akit 成员的密钥按其 API 回答的样子（已脱敏）比较（决定 041 第 8 点）；修改之后立即告知成员，
  一次一个刷新，没有 fit 的延迟。见进度日志。

### I137 集群 VIII：带健康门槛的发布

- **做什么**：fit 的 I146：发布计划（每个成员的差异和试运行、其 ETag），一次全部、滚动或金丝雀；每批之后有健康门槛（可达、
  重载成功、没有新的漂移、在一个时间窗内由 `/status` 的 `requests` 计算的 5xx 比例低于阈值）；门槛失败则回滚该批
  （带 `If-Match` 的 `/rollback/{n}`）；暂停、继续、中止；计划之后被改动的成员跳过；保存在存储中，重启后仍在。
- **完成标准**：fit 的 `rollouts` 测试移植后通过，成员有 akit 也有 fit。
- **涉及**：`akit-cluster`、`akit-admin`（`/status` 的 `requests`）。
- **fit**：`cluster/{rollouts,rollrun,rollsteer,rolldraft,rollapi}.go`；I146。
- **状态**：done（2026-11-27）。漂移门槛只用于列出 `config.check` 的成员（fit 成员；akit 成员从 I138 起）；从 akit 成员复制时
  每个目标保留自己的密钥；计划和执行各在自己的线程上（决定 041 第 9 点）。见进度日志。

### I138 集群 IX：漂移和告警

- **做什么**：fit 的 I147：相对模板和成员一致性检查的漂移（fit 的 `GET /config/check`，akit 在这里首次提供）；告警规则、
  webhook、静默。
- **完成标准**：fit 的 `alerts`、`alerteval`、`webhooks` 和 `drift` 测试移植后通过。
- **涉及**：`akit-cluster`、`akit-admin`（`/config/check`）。
- **fit**：`cluster/{alerts,alerteval,alertapi,webhooks,drift}.go`、`admin/drift.go`；I147。
- **状态**：done（2026-11-27）。akit 的检查从 `/status` 和 `/node` 的写出函数读取运行中的状态；规则按事件和自身的时间评估，而不是
  fit 的定时；webhook 签名在 `X-Akit-Signature` 中（决定 041 第 10 点）。见进度日志。

### I139 集群 X：用户、角色和审计日志

- **做什么**：fit 的 I148：按分组的角色（viewer、operator、admin）、`admin.cluster.admins`，每个端点都检查；每个操作的审计
  日志，可过滤，可导出为 JSON lines。
- **完成标准**：fit 的 `roles` 和 `audit` 测试移植后通过。
- **涉及**：`akit-cluster`。
- **fit**：`cluster/{roles,audit}.go`；I148。
- **状态**：done（2026-11-27）。管理端自身的控制台经集群模块把关；审计日志不带逐行差异（决定 041 第 11 点）。见进度日志。

### I139b 集群 XI：NAT 之后主动外连的成员

- **做什么**：fit 的 I149：`admin.cluster_member`；管理端的链接监听器（`link_address`）、注册 token（`akite1.`、`akitc1.`；
  线上使用 fit 的标签，按决定 038 的规则）、经链接承载的成员 Admin API。
- **完成标准**：fit 的 `link` 测试移植后通过；akit 成员能链接到 fit 管理端，反之亦然。
- **涉及**：`akit-cluster`、`akit-roles`（`linktoken`）、`akit`。
- **fit**：`cluster/{linkserver,linkmember,linkproto,enroll}.go`、`admin/clusterlink.go`；I149。

### I139c 集群 XII：跨成员的指标、日志和证书；`akit cluster`

- **做什么**：fit 的 I150：每次轮询得到的时间序列、成员日志的跟踪（成员上的 `GET /logs/recent`）、证书清单；CLI 中经管理端
  Admin API 使用的 `akit cluster`。
- **完成标准**：fit 的 `insight`、`logtail` 和 `adminapi` 测试移植后通过；CLI 能从头到尾驱动一次发布。
- **涉及**：`akit-cluster`、`akit-admin`（`/logs/recent`）、`akit`（`cluster`）。
- **fit**：`cluster/{insight,logtail,adminapi}.go`、`cli/cluster.go`、`admin/logsrecent.go`；I150。
