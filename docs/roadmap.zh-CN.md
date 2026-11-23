# akit 路线图

[English](roadmap.md) | [简体中文](roadmap.zh-CN.md)

akit 是用 Rust 基于 [akio](https://github.com/lesismal/akio) 编写的反向代理、负载均衡、TLS 终结、静态文件服务和
API 网关。它之于 akio，正如 [fit](https://github.com/lesismal/fit) 之于 [fib](https://github.com/lesismal/fib)：
同样的产品、同样的配置、同样的行为，只是建立在 fit 引擎的 Rust 对应物之上。

fit 已经走完了下面的每个里程碑，它的 [路线图](https://github.com/lesismal/fit/blob/main/docs/roadmap.zh-CN.md)、
[与 nginx、Caddy、Traefik 的对比](https://github.com/lesismal/fit/blob/main/docs/comparison.zh-CN.md)
和 [开发进度](https://github.com/lesismal/fit/blob/main/docs/progress.zh-CN.md) 就是 akit 的规格。本路线图
不再重新论证 fit 已经定下的东西，只说明 akit 按什么顺序追上、怎样判断每一步已完成、各自建立在 akio 的哪部分
之上。[开发计划](dev-plan.zh-CN.md) 把里程碑拆成大小适合一次工作会话的迭代。文中刻意不写日期：
里程碑在满足退出标准时结束。

- [目标](#目标)
- [非目标](#非目标)
- [设计原则](#设计原则)
- [架构](#架构)
- [里程碑](#里程碑)
- [1.0 之后](#10-之后)
- [功能对照](#功能对照)
- [性能目标](#性能目标)
- [风险与待定问题](#风险与待定问题)

## 目标

1. **功能与 fit 对齐。** fit 用户依赖的每项功能 akit 都有，按下面里程碑的顺序实现。一项功能算对齐，是指
   fit 自己针对它的测试和示例移植过来后，在 akit 上通过。
2. **两者共用一份配置。** fit 接受的配置（.conf、YAML 或 JSON）akit 也接受，且含义相同；akit 暂不支持的字段
   会按名字拒绝，绝不忽略。在 fit 和 akit 之间切换只需替换二进制（[决定 002](decisions/002-config-compatibility.zh-CN.md)）。
3. **比 fit 更快、更省。** 没有垃圾回收，回调在 I/O 线程上内联执行，每个监听器可用 io_uring 和 eBPF 分流：
   在同一台机器上，akit 的吞吐、p99 延迟和每连接内存都应胜过 fit，并保持与 nginx 持平或领先。
4. **内存安全，自身不含 `unsafe`。** 所有 `unsafe` 都留在 akio 经过审计的 crate 中；akit 全部
   `#![forbid(unsafe_code)]`。

## 非目标

- **有意偏离 fit。** akit 与 fit 可能出现差异的地方（字段名、默认值、Admin API 路径）都保持一致。值得做的改进
  两边一起做，或者先向 fit 提出。
- **异步运行时。** akit 运行在 akio 的回调和逻辑池上；不用 tokio，数据面上没有 `async fn`。需要异步运行时的
  外部库要么不进数据面，要么不用。
- **Windows。** akio 不支持。Linux 是一等平台；macOS 用于开发。
- **fit 不做的东西**：邮件代理、API 管理、Lua/njs 插件、完整兼容 nginx 配置。fit 的
  [服务网格路线](https://github.com/lesismal/fit/blob/main/docs/mesh-roadmap.zh-CN.md)
  不在本路线图之内。

## 设计原则

fit 的原则，以 Rust 的方式坚持。每个里程碑都要对照检查，
[开发规则](rules.zh-CN.md) 把它们落成具体的检查项。

1. **数据面保持 akio 的模型。** 回调在连接所属的 I/O 线程上内联执行，从不阻塞。必须阻塞的操作（磁盘缓存
   未命中、DNS、ACME、外部认证、WASM）通过 `spawn_blocking` 交给 akio 的逻辑池，或交给 akit 自己的有界池，
   完成后以完成事件的形式回到所属线程。池之间的等待只能单向。
2. **控制面与数据面分离。** 配置解析、provider、证书签发和健康探测都在 I/O 线程之外运行，产出不可变的运行时
   图。数据面通过一个原子指针读取当前的图，并在每个 I/O 线程上缓存，因此没有请求会去递增所有线程共享的引用
   计数。
3. **所有协议共用一套概念模型**：listener → router → middleware → service → upstream，HTTP、TCP、UDP
   一致；akio-mux 让监听器的流量先按协议分流。
4. **默认安全**：默认开启 HTTPS、使用现代 TLS、HTTP 跳转 HTTPS、每种大小和数量都有上限、Admin API 只绑定
   localhost 或 Unix socket。
5. **速度、静态文件和缓存以 nginx 为标杆，fit 为底线。** 每个里程碑都在压测机上发布与 nginx、Caddy、Traefik
   和 fit 的对比数据；性能回退按 bug 处理。
6. **依赖精简。** akit 只依赖 akio 和标准库。只有自研明显更差的地方才引入第三方 crate，并藏在 akit 自己的
   trait 后面，附一份决定记录（[决定 001](decisions/001-dependency-policy.zh-CN.md)）。
7. **akio 缺的东西补进 akio。** 缺口写进 [akio API 需求](akio-api-requests.zh-CN.md)，在 akio 中修复，
   而不是在 akit 里绕过。

## 架构

```mermaid
flowchart LR
    subgraph 控制面，在 I/O 线程之外
        Src["配置来源<br/>文件 · Admin API · provider"] --> Ad["读取器<br/>.conf · YAML · JSON → 同一模型"]
        Ad --> V["校验（严格，按 fit 的 schema）"]
        V --> Cp["编译：provision 所有模块"]
        Cp --> G["不可变图 v(n+1)"]
        ACME["证书<br/>文件 · ACME · OCSP"] --> G
        HC["健康检查"] --> G
    end
    G -->|"原子替换；监听器做差量"| L
    subgraph 数据面，在 akio I/O 线程上
        L["监听器<br/>TCP · UDP · TLS · QUIC"] --> Mx["akio-mux<br/>按协议分流"]
        Mx --> R["路由器<br/>host · path · header · SNI"]
        R --> Mw["中间件"]
        Mw --> Sv["服务<br/>负载均衡 · 拆分 · 故障转移"]
        Sv --> U["上游连接池<br/>akio HTTP/1、HTTP/2、HTTP/3 客户端"]
    end
```

一个请求从头到尾都在它开始时所在的那张图上处理；长连接一直持有自己的图，直到结束或被排空超时关闭。
[架构](architecture.zh-CN.md) 详细介绍了 crate、线程模型和请求路径。

## 里程碑

每个里程碑列出 **范围**、结束它的 **退出标准**，以及依赖的 **akio 能力**。1.0 之前版本号为 0.x；每个里程碑
结束时发布一个版本。每个里程碑的范围就是 fit 中同名的里程碑；这里列出的是在 akit 中需要做的事。

### M0 基础

**范围**

- Cargo workspace（`crates/akit-*`、`akit` 二进制），CI 包含 fmt、clippy、在 macOS 和 Linux（epoll 与
  io_uring）上的测试、cargo-deny，以及双语文档检查。
- akio 按 commit hash 固定（[规则 19](rules.zh-CN.md#9-akio-版本)）。
- 核心 trait：listener、matcher、router、middleware、handler、service、upstream，以及把配置中的名字解析成
  模块的模块注册表。
- 配置模型：用 Rust 类型表达 fit 的 schema，严格解码（未知字段即报错），先做 JSON；提供 `akit validate`
  和 `akit run`。
- 运行时图：由配置编译，通过原子指针替换，在最后一个连接结束时释放。
- 基准测试框架：沿用 fit 的场景和压测工具，并在 nginx、Caddy、Traefik 之外把 fit 也加为被测服务器。

**退出标准**

- `akit run -c fit.json` 能用一份 fit 也接受的配置，在 HTTP/1.1 和 HTTP/2 上返回固定响应。
- nginx、Caddy、Traefik 和 fit 在压测机上的基线数据已提交到仓库。

**akio 能力**：`Engine`、`akio-http`（HTTP/1.1、HTTP/2、Router）、`akio-tls`。

### M1 反向代理核心

**范围**

- 入口：HTTP/1.1、HTTP/2（TLS 和 h2c）、HTTP/3；从证书文件加载的 TLS；优雅退出。
- 基于 `akio-http` 的 `Files` 和 `sendfile` 的静态文件：预压缩文件、Range、索引文件、`try_files` 回退、
  ETag 和 `Last-Modified`。
- 为 akio 编写的反向代理：基于 HTTP/1 和 HTTP/2 客户端的上游连接池；请求体和响应体双向流式转发，两侧背压联动
  （`hold_reads` / `on_drain`）；连接、响应头、空闲和总超时；客户端离开时取消；删除 hop-by-hop 头，设置
  `X-Forwarded-*` 和 `Forwarded`，支持可信代理列表。
- 负载均衡：轮询、加权轮询、最少连接；被动健康检查。
- 访问日志：结构化、带缓冲，在 I/O 线程之外写入。
- SIGHUP 重载，地址不变的监听器保持不动。
- .conf 和 YAML 读取器，让 fit 的示例从 M1 起就能运行。

**退出标准**

- HTTP/2 入口通过 h2spec；fit 的代理一致性测试（头、body、trailer、超时、客户端中途断开）移植后在 HTTP/1.1
  和 HTTP/2 上通过。
- 发布静态文件和代理的基准数据，与 M0 的基线对比；1 万和 10 万连接下每个空闲连接的内存与 fit 和 nginx 对比。

**akio 能力**：HTTP/1、HTTP/2 和 HTTP/3 服务端与客户端，`Files`、`sendfile`、`hold_reads`、
`on_drain`、`spawn_blocking`。需要 akio 提供：运行时监听器（A1）。

### M2 动态配置与 TLS

**范围**

- Admin API：加载、读取、按路径修改、回滚；版本号和 ETag；默认在 localhost 或 Unix socket 上。
- 原子替换：新配置完整 provision 之后才替换；监听器做差量，移除的排空。
- 文件 provider。
- 自动 HTTPS：ACME（HTTP-01、TLS-ALPN-01）、续期、OCSP stapling、证书存储藏在 trait 后面、按需 TLS 由
  "ask" 接口和速率限制把关。
- 主动健康检查（HTTP 和 TCP）；随机、P2C、一致性哈希和粘性会话。
- 按 router、service 和 upstream 统计的 Prometheus 指标。

**退出标准**

- 频繁替换测试（流量持续时进行数千次替换）不断开任何连接、不失败任何请求。
- 只写了一个域名的配置能从 Let's Encrypt 测试环境拿到证书并提供 HTTPS。
- 停止响应探测的上游在时间窗口内被摘除，恢复后重新加入。

**akio 能力**：`akio-tls`、`akio-x509`、`akio-crypto`、HTTP 客户端。需要 akio 提供：运行时添加、移除和
排空监听器（A1）；每次握手由回调选择证书，且回调可以稍后完成（A2）。

### M3 网关能力

**范围**

- 匹配器（host、path、method、header、query、来源 IP 和 CIDR、表达式），显式优先级；重写、跳转、头修改。
- 速率、并发和大小限制；Basic、ForwardAuth（异步）、带 JWKS 的 JWT、mTLS。
- 带预算的重试、熔断、按路由的超时；加权 service、镜像、故障转移。
- 压缩：gzip、zstd、brotli（`akio-deflate`、`akio-zstd`、`akio-brotli`）。
- WebSocket 代理，包括来自 HTTP/2 和 HTTP/3 入口的；gRPC 代理。
- 四层：TCP 和 UDP 代理、不终结 TLS 按 SNI 路由、PROXY protocol v1/v2 收发，通过 akio-mux 在一个端口上
  承载多种协议。
- 上游 TLS：mTLS、自定义 CA、SNI 覆盖。

**退出标准**

- 每个中间件都有一致性测试和基准；没有一个阻塞 I/O 线程。
- WebSocket、gRPC（一元和流式）和 TCP/UDP 代理在对端很慢时通过端到端测试。
- 一个端口同时服务 HTTP/1.1、HTTP/2、按 SNI 透传的 TLS 和 SOCKS。

**akio 能力**：`akio-mux`、`akio-ws`、`akio-grpc`、`akio-socks`、`akio-regex`、各压缩器、中继辅助函数，以及
用于四层的 eBPF sockmap 中继。需要 akio 提供：akio-mux 中的 SNI 规则（A5）、PROXY protocol（A4）。

### M4 平台集成

**范围**

- Provider：Docker 标签、Kubernetes Gateway API 和 Ingress、Consul、etcd；成文的合并规则，以及一个展示最终
  生效配置和每部分来源的接口。
- 多实例共享的证书存储，带锁。
- 只读仪表盘；OpenTelemetry 链路追踪和 OTLP 指标。
- 打包：静态 musl 二进制、容器镜像、Helm chart、systemd 单元。

**退出标准**

- akit 通过 fit 声明支持的 Gateway API 一致性 profile（`GATEWAY-HTTP`）。
- Docker Compose 示例能按标签路由服务，并随服务启停更新。

**akio 能力**：HTTP/1 和 HTTP/2 客户端（watch 是长时间的流式请求）。其余是控制面的工作。

### M5 缓存与扩展

**范围**

- 响应缓存（RFC 9111）：内存和磁盘两级，磁盘 I/O 在 I/O 线程之外，缓存锁，stale-while-revalidate 和
  stale-if-error，按 key 和 tag 清除。
- 编译期模块：`akit build` 把用户自己的 crate 编进 akit，如同 `fit build` 对 Go 模块所做的。
- WASM 插件，在独立的池中运行，使用 fit 的 ABI 及其 Proxy-Wasm 子集。
- `akit adapt`：各格式之间互转，以及从 nginx 配置和 Caddyfile 转换。
- FastCGI。

**退出标准**

- 缓存在命中率场景下与 nginx 的 `proxy_cache` 相当，击穿时只回源一次。
- 一个修改请求头的 WASM 插件增加的开销有上限且经过测量；同一个 `.wasm` 能在 fit 和 akit 中运行。
- nginx 转换器能处理 fit 的示例配置，并列出每一条跳过的指令。

**akio 能力**：处理阻塞工作的逻辑池；`Files` 作为内存层的参考。

### 1.0

1.0 是对稳定性的承诺，而不是功能清单。

- 配置 schema 和 Admin API 定为 v1，与 fit 的 v1 相同。
- 对配置读取器、路由器、代理和缓存进行 fuzz。
- 对 TLS 处理、Admin API 和插件沙箱进行外部安全审计。
- 文档：快速上手、由代码生成的配置参考，以及从 fit、nginx、Caddy、Traefik 迁移的指南。

## 1.0 之后

fit 在边缘代理之外的路线，按 akit 接手的顺序排列。每条路线开始时成为 [开发计划](dev-plan.zh-CN.md) 中的一节。
大部分协议工作已经在 akio 中；剩下的是 akit 的配置、角色、Admin API 和 workbench 页面，与 fit 一样。

| 路线 | 交付什么（fit 的文档） | akio 能力 | 状态 |
| --- | --- | --- | --- |
| 日志 | 按大小和时间切割的日志文件、保留策略、时区（[fit](https://github.com/lesismal/fit/blob/main/docs/configuration.zh-CN.md)） | — | 计划中 |
| 访问控制 | 允许与拒绝名单，全局的以及每个 listener、路由、service 和角色的 | accept 时过滤（A3） | 计划中 |
| 控制台与加密的 store | `admin.store`、`admin.console`、登录、`akit store export/import`（[fit](https://github.com/lesismal/fit/blob/main/docs/console.zh-CN.md)） | `akio-crypto` | 计划中 |
| Tunnel 与 LLM | 经 hub 访问 NAT 后的服务；在多台机器间共享 AI 模型（[tunnel](https://github.com/lesismal/fit/blob/main/docs/tunnel.zh-CN.md)、[llm](https://github.com/lesismal/fit/blob/main/docs/llm.zh-CN.md)） | `akio-ws`、`akio-quic`、HTTP 客户端 | 计划中 |
| WebRTC | TURN 服务器；WHIP 和 WHEP，作为 SFU 和作为代理（[fit](https://github.com/lesismal/fit/blob/main/docs/webrtc.zh-CN.md)） | `akio-turn`、`akio-webrtc`、`akio-ice` | 计划中 |
| 流媒体 | RTMP、RTSP、SRT、HTTP-FLV、HLS、WHIP/WHEP，由 `media_auth` 把关（[fit](https://github.com/lesismal/fit/blob/main/docs/streaming.zh-CN.md)） | `akio-media` | 计划中 |
| 媒体库与频道 | 不转码的 HLS/fMP4 点播、频道、签名播放 URL（[fit](https://github.com/lesismal/fit/blob/main/docs/media-library.zh-CN.md)） | `akio-media`；MP4 读取器（待申请） | 计划中 |
| 集群管理 | 一个 akit 从 workbench 管理其他 akit，带健康门槛的分批发布（[fit](https://github.com/lesismal/fit/blob/main/docs/cluster.zh-CN.md)） | HTTP 客户端、`akio-ws` | 计划中 |
| AI 助手 | workbench 的对话面板和 MCP 服务器，基于用户自己的模型 API | HTTP 客户端 | 计划中 |
| BitTorrent | 客户端、`bt_stream`、tracker、发布（[fit](https://github.com/lesismal/fit/blob/main/docs/bittorrent.zh-CN.md)） | `akio-bittorrent` | 计划中 |
| 数据同步 | 从源到目标的管道，带检查点，对事务目标精确一次（[fit](https://github.com/lesismal/fit/blob/main/docs/sync.zh-CN.md)） | HTTP 客户端；线协议在 akit 中实现 | 计划中 |
| Prefork 与 NUMA | 数据面由多个进程服务，并按 NUMA 节点放置 | `akio-prefork`、`Engine::numa_status` | 计划中；优先级低于 fit，因为 akit 没有需要拆分的 GC |
| P2P 与 VPN | NAT 后的主机通过 ICE 或 TURN 相连；基于 TUN 的覆盖网络 | `akio-ice`、`akio-turn`；TUN 设备（待申请） | 跟随 fit：fit 的路线完成后开始 |

## 功能对照

各行与 fit 的 [功能对照](https://github.com/lesismal/fit/blob/main/docs/roadmap.zh-CN.md#功能对照) 对应。"akio" 一列表示
akio 目前是否已具备该功能所需的能力：✓ 表示就绪，或填写
[akio API 需求](akio-api-requests.zh-CN.md) 中的需求编号，— 表示是 akit 自己的工作。

| 功能 | 里程碑 | akio 能力 | akio |
| --- | --- | --- | --- |
| HTTP/1.1、HTTP/2 | M0、M1 | `akio-http` | ✓ |
| HTTP/3（QUIC） | M1 | `akio-h3`、`akio-quic` | ✓ |
| h2c 上游、gRPC 代理 | M1、M3 | HTTP/2 客户端、`akio-grpc` | ✓ |
| WebSocket 代理 | M3 | `akio-ws`、upgrade | ✓ |
| TCP/UDP 代理 | M3 | `Conn`、`hold_reads`、sockmap 中继 | ✓ |
| 按 SNI 路由 TLS | M3 | `akio-mux` | A5 |
| 一个端口多种协议 | M3 | `akio-mux` | ✓ |
| SOCKS 与 CONNECT | M3 | `akio-socks` | ✓ |
| PROXY protocol v1/v2 | M3 | — | A4 |
| 静态文件 | M1 | `Files`、`sendfile` | ✓ |
| FastCGI | M5 | — | — |
| 负载均衡、健康检查 | M1、M2 | HTTP 客户端 | — |
| 不中断地修改配置 | M1、M2 | 运行时监听器 | A1 |
| 自动 HTTPS、按需 TLS | M2 | `akio-tls`、`akio-x509` | A2 |
| 响应缓存 | M5 | 逻辑池 | — |
| 速率与并发限制 | M3 | 以 `limiter` 中间件为起点 | ✓ |
| 认证：Basic、ForwardAuth、JWT、mTLS | M3 | `akio-tls`、`akio-crypto` | ✓ |
| 压缩：gzip、zstd、brotli | M3 | `akio-deflate`、`akio-zstd`、`akio-brotli` | ✓ |
| Kubernetes、Docker、Consul、etcd | M4 | HTTP 客户端 | — |
| Prometheus、OpenTelemetry、仪表盘 | M2、M4 | `Engine::metrics`、遥测 | ✓ |
| Go 模块 → crate（`akit build`）、WASM | M5 | — | — |
| nginx 与 Caddyfile 转换器 | M5 | — | — |
| 允许与拒绝名单 | 1.0 之后 | accept 过滤 | A3 |
| TURN、WHIP 与 WHEP | 1.0 之后 | `akio-turn`、`akio-webrtc` | ✓ |
| RTMP、RTSP、SRT、FLV、HLS | 1.0 之后 | `akio-media` | ✓ |
| 媒体文件点播 | 1.0 之后 | MP4 读取器 | 待申请 |
| BitTorrent 客户端与 tracker | 1.0 之后 | `akio-bittorrent` | ✓ |
| Prefork 与 NUMA | 1.0 之后 | `akio-prefork` | ✓ |

## 性能目标

在压测机上用 fit 的框架和场景测量，对比对象是 nginx、Caddy、Traefik，以及与其基线同一 commit 的 fit。

| 指标 | 目标 |
| --- | --- |
| 每个空闲连接的内存（HTTP/1.1、TLS、WebSocket） | 低于 fit；不高于 nginx |
| 吞吐，静态文件 | 不低于 nginx |
| 吞吐，反向代理 | 高于 fit 和 nginx |
| 负载下的 p99 延迟 | 低于 fit；替换配置时保持平稳 |
| 每请求的内存分配次数 | 每个版本测量；增加需要给出理由 |
| 一次配置替换的开销 | 与打开的连接数无关 |

## 风险与待定问题

- **配置兼容性漂移。** fit 一直在加字段。akit 尽量从 fit 自己的源头（生成的配置参考和模板）读取 fit 的
  schema，并在 CI 中用 `akit validate` 跑 fit 的模板和示例，这样 fit 新增的字段会明确显示为"暂不支持"，
  而不是悄悄漂移。
- **akio 的缺口。** 代理到 M2 时需要运行时监听器（A1）和异步证书选择（A2）。这些先在 akio 中提出；落地之前，
  M1 可以用 `Engine::adopt_tcp` 加 akit 自己的 accept 线程作为有记录的临时方案。
- **不用 serde 解析配置格式。** 依赖精简的规则倾向于 akit 自己写 JSON、YAML 子集和 .conf 读取器；用 serde
  写起来更快。已在 [决定 001](decisions/001-dependency-policy.zh-CN.md) 中定下：.conf 和 JSON 自己写；YAML
  也自己写，由 [决定 005](decisions/005-yaml-reader.zh-CN.md) 在 I24 中决定。
- **ACME。** 在 akit 中基于 `akio-crypto` 和 `akio-x509`（JWS、CSR）实现，如同 fit 基于
  `x/crypto/acme`；akio 可能需要提供 CSR 生成（在 M2 中确认）。
- **WASM 运行时。** wasmtime 快，但体积大，需要 `unsafe` 和 JIT；wasmi 是纯 Rust 的解释器。在 M5 中决定，
  藏在 akit 自己的 trait 后面，并与 fit 的 wazero 对比测量。
- **只有异步实现的生态。** Rust 的 Kubernetes 和 etcd 客户端大多基于 tokio；akit 在 akio 的客户端上实现它
  所需的那一小部分协议，而不是再跑一个运行时。
- **许可证。** 已定：与 fit 一样，采用 Apache-2.0。
