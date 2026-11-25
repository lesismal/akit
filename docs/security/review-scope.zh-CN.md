# 外部安全审查：范围

[English](review-scope.md) | [简体中文](review-scope.zh-CN.md)

1.0 外部安全审查的范围，即[路线图](../roadmap.zh-CN.md)要求的对 TLS 处理、Admin API 和插件沙箱的审查：审查者看什么、在哪里、先问什么、
可以略过什么，以及已经知道的问题。[威胁模型](threat-model.zh-CN.md)与本页配套：本次审查检验的就是其中的资产、边界和控制措施；其
[已知的局限](threat-model.zh-CN.md#已知的局限)是已知的，不算发现。

- [审查对象](#审查对象)
- [我们想了解什么](#我们想了解什么)
- [范围之内](#范围之内)
  - [1. TLS 处理](#1-tls-处理)
  - [2. Admin API 与仪表盘](#2-admin-api-与仪表盘)
  - [3. 插件沙箱](#3-插件沙箱)
  - [4. 请求解析与代理的头处理](#4-请求解析与代理的头处理)
  - [5. eBPF sockmap 路径](#5-ebpf-sockmap-路径)
  - [6. 时间允许时：提供者与共享存储](#6-时间允许时提供者与共享存储)
- [模糊测试目标](#模糊测试目标)
- [范围之外](#范围之外)
- [akio](#akio)
- [在 akit 上工作](#在-akit-上工作)
- [发现的问题](#发现的问题)

## 审查对象

- **akit**：维护者在审查开始时指定的提交。在 I80 时，是在 `953cb57` 之上合并了 I77（模糊测试）和 I80（本页）的 `main`。
- **akio**：akit 固定的提交（`Cargo.toml` 中的 `rev`；I80 时为 `0fc8f3a7`，`1.0.0-rc.1`），发布二进制唯一的依赖。
- **平台**：Linux（x86_64 和 aarch64；epoll 和 io_uring；sockmap 转发需要 6.15 或更新的内核），开发用 macOS（kqueue）。发布二进制是
  `scripts/dist.sh` 构建的静态 musl 版本，镜像由 `Dockerfile` 构建。
- **配置**：fit 的配置（[决定 002](../decisions/002-config-compatibility.zh-CN.md)），见[配置参考](../reference/config.zh-CN.md)；起点是
  镜像的 `deploy/docker/akit.json` 和[指南](../guides/README.zh-CN.md)。

## 我们想了解什么

1. 客户端、上游、网页或没有令牌的本机用户，能否让 akit 做其配置不允许的事：读取或修改配置、绕过中间件到达路由、读取根目录之外的
   文件、到达正向代理拒绝的地址、为 ask URL 拒绝的名字获取证书，或者拿到密钥？
2. 在 akit 默认设置的限制之内，它们能否让 akit 崩溃、让 I/O 线程挂起或阻塞，或耗尽内存、连接或线程池？
3. akit 是否按预期的方式使用 akio 的 TLS、HTTP、WebAssembly 和 eBPF 部分：它设置的选项、保留默认值的选项、依赖 akio 完成的检查？
4. [威胁模型](threat-model.zh-CN.md)声称的控制措施是否存在，其测试是否真的测试了它们？

## 范围之内

每个领域列出其代码、威胁模型中的对应部分，以及我们首先会问的问题。路径相对于仓库根目录。

### 1. TLS 处理

代码：`crates/akit-tls`（`lib.rs`：`ListenerTls`、`server_configs`、客户端认证；`certs.rs`：每次握手选择的证书；`watch.rs`；`storage.rs`；
`ocsp.rs`；`published.rs`；`upstream.rs`；`acme/`：`mod.rs`、`protocol.rs`、`jws.rs`、`ari.rs`、`on_demand.rs`、`shared.rs`；`shared/`：
Redis、etcd、Consul），`crates/akit-runtime/src/tls.rs`（读取图的证书解析器），`crates/akit-listener/src/lib.rs`（应答 HTTP-01 和
TLS-ALPN-01，带 TLS 绑定监听器），`crates/akit-upstream/src/h2.rs`（基于 TLS 的 h2 上游）。威胁模型：
[来自客户端的 TLS](threat-model.zh-CN.md#来自客户端的-tls)、[证书](threat-model.zh-CN.md#证书)、[上游](threat-model.zh-CN.md#上游)。

- **akit 所用的 akio-tls**：akit 配置的版本、密码套件、ALPN、会话票据、客户端认证模式和每次握手的解析器；early data 保持关闭；
  TLS-ALPN-01 的证书从不返回给普通客户端。akio-tls 自己的握手、记录层和 X.509 验证属于 akio（[见下](#akio)）。
- **证书**：加载并匹配证书与密钥、按 SNI 选择（未知名字返回第一张证书，与 fit 相同）、在流量中替换（`-watch-certs`）、客户端 CA 的变化。
- **ACME**：账户密钥及其存储、JWS 签名、订单与挑战、ARI、续期与退避；明文监听器上的 HTTP-01 应答路径；存储的权限模式和原子写入；
  共享存储的锁，以及实例从存储中信任什么。
- **按需 TLS**：客户端能否让 akit 询问或获取 ask URL 的应答和速率限制之外的名字；等待中握手的上限；ask 客户端的 TLS（在 I80 中修复：
  它没有任何 CA，因此每个 `https://` 询问都失败）。
- **OCSP**：响应经过验证并匹配，来自文件的证书只在响应者是公网地址时才询问，从不在 `nextUpdate` 之后装订。
- **上游 TLS**：始终验证、`tls_server_name`、客户端证书。

### 2. Admin API 与仪表盘

代码：`crates/akit-admin`（`server.rs`：分发器、`guard`、消息体、任务；`configs.rs`：历史记录；`pointer.rs`：JSON 指针和合并补丁；
`cache.rs`：清除；`status.rs`；`dashboard.rs` 及仪表盘的文件；`watch.rs`），`crates/akit-config/src/model.rs`（`admin`），
`crates/akit/src/main.rs`（应用任务的控制线程），`crates/akit-provider/src/merge.rs`（提供者的部分可以包含什么）。威胁模型：
[Admin API 与仪表盘](threat-model.zh-CN.md#admin-api-与仪表盘)。端点列在[兼容性页面](../compatibility.zh-CN.md#admin-api-v1)中，
并由 `tests/compat/v1/admin-api.txt` 冻结。

- **认证**：地址检查（回环地址、Unix 套接字的模式），令牌的读取和比较，没有令牌能访问什么（仪表盘的静态文件）。
- **浏览器**：针对 CSRF 和 DNS 重绑定的 `Origin` 和 `Host` 规则，有令牌和没有令牌时；仪表盘的头以及它如何保存令牌。
- **修改**：消息体及其限制、JSON 指针、合并补丁、`If-Match`、`?dry_run`、回滚和重载；修改能否不经编译就被应用，或者 dry run 是否会
  改变任何东西（绑定监听器、写文件、订购证书）。
- **读取**：`GET /config`、`/config/effective`、`/versions`、`/status`、`/metrics` 和 `/cache` 泄露什么（已知：内联的值不脱敏，L2）。
- fit 的控制台（登录、会话、加密存储）尚未实现，不在范围之内。

### 3. 插件沙箱

代码：`crates/akit-plugin`（`plugin.rs`：加载和线程池的配置；`abi.rs`：fit 的 ABI；`proxywasm.rs`：Proxy-Wasm 的宿主函数和上下文；
`wasm.rs`：中间件、挂起和响应钩子；`encode.rs`、`response.rs`），`crates/akit-config/src/wasm.rs`（限制和默认值）。威胁模型：
[插件](threat-model.zh-CN.md#插件)。决定 [008](../decisions/008-wasm-runtime.zh-CN.md) 和 [011](../decisions/011-proxy-wasm.zh-CN.md)。

- **宿主函数**：访客传入的每个指针和长度都对照其内存检查；访客能读取什么（请求和响应的字段、属性）、能修改什么（字段、伪头、本地响应、
  消息体），以及它写入的内容是否都经过检查才会发出。
- **上限**：时间（调用无法超过的截止时间）、内存、实例、队列、流、消息体、指标；陷入后重新创建的实例不带走任何东西。
- **上下文**：请求的上下文在每种结束方式下都被关闭（完成、客户端离开、响应中断、实例重建）；一个请求的数据能否到达另一个请求的上下文。
- **akit 所用的 akio-wasm**：akit 设置的实例限制和 WASI 配置。解释器自己的验证和执行属于 akio（[见下](#akio)），时间允许时是自然的延伸。

### 4. 请求解析与代理的头处理

代码：`crates/akit-listener/src/lib.rs`（分发器：规范化路径、绝对形式的目标、限制、PROXY protocol），`crates/akit-router`（`path.rs`、
`matcher.rs`、`expr.rs`），`crates/akit-proxy`（`headers.rs`：逐跳字段、转发字段与信任；`lib.rs`；`websocket.rs`、`grpc.rs`、`relay.rs`、
`retry.rs`、`mirror.rs`），`crates/akit-middleware`（`auth.rs`、`jwt.rs`、`jws.rs`、`jwks.rs`、`forward.rs`、`limit.rs`、`rewrite.rs`、
`headers.rs`、`redirect.rs`），`crates/akit-handler`（`static_files.rs`、`fastcgi/`、`forward_proxy.rs`），`crates/akit-l4`（`forward.rs`、
`entry.rs`），`crates/akit-cache`（`cache.rs`、`semantics.rs`）。威胁模型：[客户端](threat-model.zh-CN.md#客户端)、
[上游](threat-model.zh-CN.md#上游)。

- **请求走私与解析不一致**：akit 和上游能否对一个请求的边界或头有不同的理解（HTTP/1.1 到 HTTP/1.1、h2 或 h3 到 HTTP/1.1、WebSocket 和
  CONNECT 升级、gRPC）；HTTP/1 解析本身属于 akio。
- **路由**：akit 路由不同的两个请求对上游而言能否指向同一资源，或者反过来（规范化路径、`%2F`、`strip_prefix`、重写、绝对形式、`Host`
  与 `:authority`）。
- **转发字段的信任**：来自受信任和不受信任对端的 `X-Forwarded-*`、`Forwarded`、PROXY 头；认证中间件为上游设置的字段。
- **认证中间件**：basic auth、JWT（算法、密钥、声明）、JWKS 获取、forward auth、按路由的客户端证书。
- **文件**：静态处理器和 FastCGI 的路径、点文件、`try_files`（符号链接：L7）。
- **正向代理**：目的地规则、DNS 重绑定、主机自己的地址（没有认证、私有地址段可达：L6）。
- **缓存**：键、`Vary`、用户之间共享什么。

### 5. eBPF sockmap 路径

代码：`crates/akit-l4/src/sockmap.rs` 和 `relay.rs`，以及它们在 `crates/akit-listener/src/lib.rs` 中的使用；akit 加载和使用的 akio
`akio-ebpf` 转发程序（`akio::ebpf::Relay`）。[决定 010](../decisions/010-sockmap-relay.zh-CN.md)；威胁模型：
[四层与 sockmap 转发](threat-model.zh-CN.md#四层与-sockmap-转发)。

- 哪些连接会被拼接，在用户态进行的检查（PROXY 头、TLS、`max_conns`、空闲超时、重载时的排空）能否因为拼接而被跳过。
- 加载需要的权限，没有权限时会怎样，加载的程序和 map 能否被其他进程访问。
- 转发的上限（连接对数、套接字的缓冲区）以及达到上限时的回退。
- 在 akio API 需求 A170 被固定之前，akit 通过 akio 文档中隐藏的 `Conn::inner` 读取连接的 TCP 计数器。

### 6. 时间允许时：提供者与共享存储

`crates/akit-provider`（Docker、Kubernetes、Consul、etcd：客户端、解析器、合并）和 `crates/akit-tls/src/shared`（作为证书存储的 Redis、
etcd、Consul）。它们传输方式的默认值是已知的局限（L4）；它们的解析器做了模糊测试。威胁模型：
[提供者与共享存储](threat-model.zh-CN.md#提供者与共享存储)。

## 模糊测试目标

I77 为 akit 拥有的每个解析器加了目标（`fuzz/`，cargo-fuzz，自成一个工作区）；每个目标调用其 crate 的 `fuzzing` 模块，`cargo test` 回放
提交到仓库的语料（`fuzz/corpus/<target>`）。CI 在每次改动时把它们运行固定的时间，每天运行更久（`.github/workflows/fuzz.yml`）。PROXY
protocol 和 ClientHello 的解析器属于 akio-mux，在 akio 中做模糊测试。

| 领域 | 目标 |
| --- | --- |
| 配置读取器和转换器（操作者的输入） | `config_json`、`config_conf`、`config_yaml`、`config_value`、`config_adapt`、`convert_nginx`、`convert_caddyfile` |
| 路由 | `router_expr`、`router_path`、`router_find` |
| 代理的头 | `proxy_headers`、`proxy_grpc_timeout` |
| 中间件 | `jwt_token`、`jwks_keys`、`middleware_fields` |
| 文件和 FastCGI | `static_path`、`fastcgi_paths`、`fastcgi_response` |
| 四层和正向代理 | `l4_hello`、`l4_forward` |
| 缓存 | `cache_semantics`、`cache_disk` |
| TLS 和存储 | `tls_acme`、`tls_redis_reply` |
| Admin API | `admin_pointer` |
| 提供者 | `provider_docker`、`provider_kubernetes`、`provider_consul`、`provider_kv`、`provider_merge` |
| 插件 | `plugin_pairs` |
| 日志和上游 | `observe_access_log`、`observe_escapes`、`observe_tzif`、`upstream_keys` |

```bash
cargo +nightly fuzz run proxy_headers -- -max_total_time=600
```

## 范围之外

- **akio 的内部**（[见下](#akio)）：它的解析器、TLS、X.509、密码算法、QUIC、WebAssembly 解释器、I/O 后端及其 `unsafe`。akit 对它们的
  使用在范围之内。
- **akit 尚未实现的部分**：fit 的控制台、隧道、LLM 网格、WebRTC、流媒体、媒体、集群、AI 助手、BitTorrent、同步、P2P 和 VPN
  （[1.0 之后的路线](../dev-plan.zh-CN.md#10-之后)）；它们的字段作为暂不支持被拒绝。
- **操作者自己的输入**：配置、其文件、环境或带令牌的 Admin API 能做的事是操作者的权力（[威胁模型的假设](threat-model.zh-CN.md#假设)）。
  配置读取器和转换器不能因此崩溃，并且做了模糊测试，但它们不是边界。
- **超出配置限制的拒绝服务**，或关闭了某项限制时的拒绝服务。
- **所有不随发布交付的东西**：`bench/`、`tools/`、`fuzz/`、`tests/`、一致性测试工具、`deploy/` 默认安装之外的部署示例；开发依赖。
- [威胁模型](threat-model.zh-CN.md#已知的局限)中的**已知局限**：发现其中某项比描述的更严重，则在范围之内。

## akio

akio 是 akit 的引擎和唯一的依赖，与 akit 是同一位维护者，有自己的[安全政策](https://github.com/lesismal/akio/blob/main/SECURITY.zh-CN.md)。
它的 1.0 审查是内部进行的：对协议解析器的模糊测试、cargo-audit 和 cargo-deny、[unsafe 审查](https://github.com/lesismal/akio/blob/main/docs/unsafe-review.zh-CN.md)、
Miri、loom、sanitizer，以及对密码算法的[常数时间检查](https://github.com/lesismal/akio/blob/main/docs/constant-time.zh-CN.md)。它的 TLS、
X.509、密码算法和 QUIC 都是 akio 自己写的，不是取自经过广泛审查的库，也没有经过外部审查（威胁模型 L15）。那项审查属于 akio，建议在
akit 的 1.0 依赖它之前进行；如果本次审查可以扩展，我们建议的顺序是 akio-crypto 和 akio-tls（握手、记录层、证书验证），然后是
akio-http 的 HTTP/1 和 HTTP/2 解析器，再然后是 akio-wasm。

## 在 akit 上工作

- 构建和测试：`cargo build --release -p akit`；`cargo test --workspace`（在 Linux 上设置 `AKIO_TEST_BACKEND=uring` 测试 io_uring）；
  `cargo clippy --workspace --all-targets -- -D warnings`；`cargo deny check`；`cargo audit -D warnings`；`python3 -I tools/release-deps.py`。
- 运行：`akit run -c <config>`；`akit validate -c <config>`；设置 `admin` 时 Admin API 在 `127.0.0.1:2019`
  （[Admin API 指南](../guides/admin-api.zh-CN.md)）。
- 架构：[架构](../architecture.zh-CN.md)、[模块](../modules.zh-CN.md)、每次改动都遵循的[开发规则](../rules.zh-CN.md)，以及
  [决定](../decisions/README.zh-CN.md)。
- 负载测试和基准测试只在维护者的基准测试主机上运行（[规则 23](../rules.zh-CN.md)）；欢迎审查者在自己的机器上做负载测试。

## 发现的问题

请按 [SECURITY.zh-CN.md](../../SECURITY.zh-CN.md) 所说私下报告发现的问题，每项附上严重程度、受影响的代码、复现方式（一份配置和一个请求、
一个测试或一个模糊测试输入）以及建议的修复。修复和威胁模型的局限一起在[开发计划的 I80b](../dev-plan.zh-CN.md#i80b-安全加固) 中跟踪；
akio 中的问题交给 akio，并在 akit 移动其固定提交时到达 akit。
