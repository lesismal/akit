# akit 开发进度

[English](progress.md) | [简体中文](progress.zh-CN.md)

本文件按迭代记录实际完成的内容，以及下一次会话从哪里继续。[开发计划](dev-plan.zh-CN.md) 说明每个迭代应该做
什么；本文件说明实际做了什么，以及与计划不同的地方。

## 当前状态

| | |
| --- | --- |
| 最后更新 | 2026-11-27 |
| 里程碑 | M0 基础已完成（I01–I08）；M1 代理核心已完成（I09–I25、I17b、I18b；[M1 报告](../bench/results/m1-report-2026-11-25.zh-CN.md)）；M2 动态配置与 TLS 已完成（I26–I38）；M3 网关功能已完成（I39–I56、I51b；[M3 报告](../bench/results/m3-report-2026-11-25.zh-CN.md)）；M4 平台集成已完成（I57–I66；[M4 报告](../bench/results/m4-report-2026-11-25.zh-CN.md)）；M5 缓存与扩展已完成（I67–I76；[M5 报告](../bench/results/m5-report-2026-11-25.zh-CN.md)）；1.0 已完成：I77、I78、I79、I79b、I80；I80b 进行中（威胁模型中属于 akit 的局限已完成，等待审查的发现）；1.0 之后，日志已完成（I81–I83），访问控制列表已完成（I85–I87）；akit 的名字已完成（I97，决定 037：对外凡写 "fit" 的名字都改为 "akit"）；隧道与 LLM（I98–I109，决定 038）完成到 I103b 和 I102d（I102c 的 WebSocket 链路基于 akio 的 A236）。 |
| akio | akio `main` 上的 `a8171fda`（`1.0.0-rc.1`，包含 akit 锁定的所有请求，见 [akio API 请求](akio-api-requests.zh-CN.md)，以及 akio-wasm（ADR-125）和 ADR-130 的共享 I/O 线程组），在 `Cargo.toml` 中按 `rev` 锁定 |
| fit | 作为规格；本计划基于 `7daee573` 阅读 |
| 可用功能 | 控制台（`admin.console`，I89–I91：bcrypt 登录之后的 fit 工作台页面，以 akit 的名字（I97），经 TLS 提供，带访问控制列表，其连接按 fit 的规则受限，基于 akio 的 A232）以及非回环地址上带 TLS 的 Admin API（I92），两者的证书在文件变化时重新读取（I94）；加密的 store（I93–I93c）：逐字节同 fit 的 `fitstore`，`admin.store`，`akit store export` 和 `import`，`akit run` 把配置保存在其中（启动时合并到配置文件之上，每次修改都写回，SIGHUP 时重新读取，`admin` 留在引导文件中），控制台的密码修改保存在其中，Admin API 的 `store` 导出和导入，以及 `akit run -admin`（带临时密码的控制台，配置旁边的 store 及其密钥文件，经 `-c` 给出的 `.store`），fit 与 akit 可轮流从同一个 store 运行；`akit hash-password`；`akit run` 和 `akit validate`，带 fit 的参数和退出码，按 fit 的写法转换和格式化配置的 `akit adapt` 与 `akit fmt`（I73），把其他 crate 的模块构建进 akit、或少带自带模块的 `akit build`（I70），按 fit 的方式由 `akit adapt -from` 转换的 nginx 配置与 Caddyfile（I74），JSON、`.conf` 和 YAML 配置，文件和目录合并；SIGHUP 重载配置（错误的配置保留正在运行的配置；基于 akio 的 A1 保留（按 fit 的 `binding.same`：证书文件和 ACME 名字可以变）、增加、修改和排空监听器，包括启动时的监听器，服务 h3 和 `udp` 的监听器也一样，其 UDP 一侧被排空（akio 的 A22））；SIGINT 和 SIGTERM 时优雅关闭；HTTP/1.1、h2c、TLS（SNI、ALPN、客户端认证；按 fit 的方式每次握手选择证书，重载或 `-watch-certs` 替换证书而不关闭监听器）和 HTTP/3 监听器，TCP 一侧带 `Alt-Svc`；基于规范化路径的路由器 v1，带 fit 的匹配器（方法、头、query、远端 IP、路径正则）和表达式；路由中间件：`rewrite`（前缀和正则）、`redirect`（含正则的分组）、`headers`、`rate_limit`、`concurrency_limit`、`body_limit`、`basic_auth`（bcrypt，在 akio 的线程池上校验）、`client_cert`（按路由的 mTLS）、`forward_auth`（经 akio 的客户端询问认证服务，期间挂起请求）、`circuit_breaker`、`compress`（按 q 值选择 zstd、brotli 和 gzip，流式消息体逐次写入编码）和 `jwt`（用 akio-crypto 校验 JSON Web Token，密钥来自 PEM 文件、密钥文件或经 akio 的客户端获取并轮换的 JWK 集）；`respond`、`static`（条件请求和范围请求，文件在 akio 的线程池上打开，按 q 值选择预压缩副本，内存中文件的压缩副本，`try_files`，目录列表，`strip_prefix`）和 `proxy` handler（fit 的转发字段、可信代理、客户端的 `Host`、双向流式转发 body 并联动背压，双向透传 trailer，`timeout` 和上游的 `response_timeout`、`response_header_timeout`、`response_idle_timeout`，超时回答 504，客户端无论何时离开都放弃上游请求，带预算的 `retry`，`timeout` 覆盖所有尝试；按权重随机选择的 `upstreams`，所选上游没有地址在轮转中时接管请求的 `failover`，接收消息体完整到达的请求副本的 `mirror`（同时最多 256 个，请求从不等待副本）；经 HTTP/1.1、h2c 和 h2 访问上游，h2 支持 fit 的 `tls_server_name`、`tls_ca_file` 和客户端证书）；所有监听器和代理上的 `limits.max_body_bytes`（413），以及 `limits.write_stall_timeout`（重载也可修改）；fit 的 json、text 和 line 格式的访问日志，以及 akit 自身的日志（fit 的 `log`、其日志器、作为 `engine` 的 akio 记录），写入按大小和时间切割、保留并压缩的文件，重载就地改变，并可在 Admin API 的 `/logs` 中列出、切割和清理（I81–I83）；上游连接池，fit 的九种负载均衡策略（轮询、加权轮询、最少连接、随机、p2c，以及按客户端 IP、路径、头或 cookie 的一致性哈希）和会话保持 cookie，跳过被被动健康检查（`max_fails`、`fail_timeout`）和主动健康检查（`health_check`：http 和 tcp 探测）下线的地址，未变上游的连接池、连接和健康状态在重载后保留（I38）；回环地址上的 Admin API（`GET /config`，也可按 JSON 指针读取，`POST /load`，用 `PUT`、`PATCH` 和 `DELETE /config` 按指针编辑，`POST /rollback/{n}`，`POST /reload`，都带 `If-Match`，除回滚外都可 `?dry_run=1`，`GET /versions`；有 `token_file` 时每个请求都要带 bearer 令牌），修改与 SIGHUP 一样作为重载应用，两者共用一份历史记录；`run -watch` 在配置文件每次变化时应用它们（fit 的文件 provider，轮询）；Admin API 的 `GET /metrics` 提供 fit 的 Prometheus 指标（有 `metrics` 时按监听器和路由的请求、上游和后端、证书、连接、重载、akio 的引擎）；Admin API 的 `/dashboard/` 提供 fit 的只读仪表盘，基于 `GET /status` 和 `GET /config/effective`（I63）；为监听器在 `tls.acme` 中所列的名字通过 HTTP-01 或 TLS-ALPN-01 用 ACME 获取证书，保存在存储中，按有效期或 CA 的续期信息（ARI）续期，并装订其 OCSP 响应，也在握手时为 ask URL 允许的名字获取证书（`tls.on_demand`，带限流，握手等待时不占住 I/O 线程）；默认 HTTPS（`auto_https`）：没有写监听器的配置以其路由主机名的证书经 HTTPS 服务，经 HTTP 重定向（308）到 HTTPS，空的 `tls` 得到其路由的名字；四层 `services`（I51）：`tcp` 和 `udp` 监听器代理到上游的地址，双向联动背压，空闲超时，`max_conns`，重载后排空；按 TLS ClientHello 的服务器名共用一个 `tcp` 监听器的服务，不终结 TLS 直接透传（I52），以及 `tcp` 与 HTTP 监听器从受信任代理接收、并向服务的上游发送的 PROXY protocol 头（v1、v2）；一个端口上与 HTTP 并列的 `tcp`（I53），按客户端先发的字节区分；正向代理（I55）：监听器 `socks` 上的 SOCKS5、SOCKS4 与 4a CONNECT，单独或在一个端口上与 HTTP、`tcp` 并列，以及 `forward_proxy` 处理器的 HTTP/1.1 CONNECT 隧道，带 fit 对客户端和目的地的规则（地址在连接建立后判定）、连接超时与空闲超时；WebSocket 代理（I49）：HTTP/1.1 upgrade 与 HTTP/2、HTTP/3 扩展 CONNECT 代理到 HTTP/1.1 上游，双向联动背压，`tunnel_idle_timeout`；gRPC 调用代理（I50）到 h2 和 h2c 上游，双向流式传输并带 trailer，受 `grpc-timeout` 约束，失败时以 gRPC 状态应答；响应缓存（I67、I68）：zone 共享的有界内存层上的 RFC 9111，流式消息体在发出的同时保存，缓存锁、stale-while-revalidate 与 stale-if-error，在自己的线程池上读写的磁盘层，以及经 Admin API 的 `POST /cache/purge` 按 URL、URL 前缀、标签或 zone 清除（I69）；provider 框架（I57）：provider 提供的部分按 fit 的规则合并进基础配置，各自作为新版本应用，`GET /config/effective` 给出每部分的来源；Docker provider（I58）：来自运行中容器标签的路由、上游和服务，经 akio 的 HTTP 客户端通过守护进程的 Unix 套接字（或 TCP）跟随其事件；Kubernetes provider（I59、I60）：经 HTTP/2 或 HTTP/1.1 list 并 watch akit 的 GatewayClass 的 Gateway 及其 HTTPRoute、GRPCRoute、TLSRoute、TCPRoute 和 UDPRoute，以及 akit 的 IngressClass 的 Ingress，作为监听器、路由和服务发往其 Service 的就绪端点，其 TLS Secret 写成证书文件，并写回 Gateway API 的状态；Consul catalog、Consul KV 与 etcd provider（I61），以阻塞查询和 watch 跟随变化；多台主机上的实例经 Redis、etcd 或 Consul 共享证书（`acme.shared_storage`，I62），带租约锁，挑战由 CA 到达的那个实例应答；OpenTelemetry（I64）：按 `traceparent` 和 `telemetry.traces.sample_ratio` 追踪请求，上游收到 akit 的 span，span 和指标通过 OTLP/HTTP 导出；`fastcgi` handler（I75）：经 TCP 或 Unix 域套接字连接 PHP-FPM 等 FastCGI 应用，有界连接池，双向流式传输消息体并联动背压，`try_files`、点文件和 fit 的超时；`wasm` 中间件（I71）：akit ABI 的 WebAssembly 插件（fit 的 ABI 改名之后：`akit_on_request`、模块 `akit`，I97）运行在 akio-wasm 的实例池上，有时间和内存上限，从不在 I/O 线程上运行；访问控制列表（I85–I87）：客户端地址的允许和拒绝列表，全局、按监听器、路由和服务，以及 Admin API 上，写在配置中或列表文件中（地址、前缀、区间、`ipfilter.dat` 和 `.p2p` 行，与证书一起被监视），由 akio 的过滤器在 accept 时拒绝、逐请求回答 403，或在知道第 4 层连接的服务后关闭；SOCKS 客户端在发出请求时检查；兼容性门禁：80 个夹具，56 个有效，23 个只因暂不支持被拒绝（19 个字段），1 个跳过，没有失败，每个 fit 模板都有效；配置 schema 和 Admin API 冻结为 v1（I79，[政策](compatibility.zh-CN.md#v1哪些保持稳定)），由 `tools/v1-freeze.py` 和 `v1_surface_is_frozen` 检查；fit 改名字段的旧名按 fit 的警告读取；`limits.max_header_bytes`、`max_conns`、`max_write_queue_bytes` 和 `engine`（决定 028）；代理一致性测试套件与 h2spec（I25）；读取并检查 fit 的 `tunnel` 和 `llm` 角色（I98），网关与各角色之间一个地址只给一个服务器（I98）；链路 token（`akitt1.`，fit 的证明，I99）经 Admin API 的 `/tokens` 和 `akit token` 签发、列出、过期和撤销（I100）；各角色由 `akit run` 在网关旁、在其引擎上运行（I100）；隧道的 hub、node 和 proxy（I101）经 TCP、TLS（I102，用 pin 或 CA，保存 hub 的证书）、QUIC（I102b）和 WebSocket（I102c，`ws` 和 `wss`），用 token 或共享密码注册，用 `body_password` 密封，可与 fit 自己的 hub、node 和 proxy 互连（决定 038）；hub 供页面使用的调用和状态、token 在过期时关闭其链路（I103）；hub 的控制台套接字和 proxy 的 web 代理，密封方式同 fit（I103b）；角色的控制台（其证书在文件变化时重新读取）、hub 的页面（I109）、隧道 node 和 proxy 的页面（I109b）和 LLM 角色的页面（I109c，及其方法，I109d），也在 `admin.console` 上显示；hub 的 QUIC 握手按 fit 的方式受限（`handshake_timeout`、`max_handshakes`，过半后要求 Retry，akio 的 A245），其监听器的证书在文件变化时重新读取（I102d） |
| 下一步 | 外部安全审查（[范围](security/review-scope.zh-CN.md)），其发现完成 I80b；fit 对决定 035 所提议字段（CRL 文件、WASM 摘要）的答复；1.0 之后的各条线（[开发计划](dev-plan.zh-CN.md#10-之后)）：控制台与加密的 store 已完成（I89–I94、I93b、I93c）；fit 的 Admin API 对没有令牌的连接的规则，基于控制台的 slots（akio 的 A232）（日志已完成：I81–I83；访问控制列表已完成：I85–I87）；Gateway API 一致性测试的运行（tests/conformance，由维护者运行） |
| akio 需求 | A1–A24、A29、A30、A32、A33、A35–A38、A45、A48、A49、A51–A54、A57、A58、A63–A66、A75、A78、A79、A90–A94、A105、A108、A109、A111–A113、A117、A132–A135、A148、A165–A172、A180、A200、A214、A215、A220、A230–A238、A245 和 A247–A249 已进入 akio 的 main 并锁定；A225（外部审查，L15）已提出（[akio API 请求](akio-api-requests.zh-CN.md)） |
| 流媒体与媒体认证 | [I120–I126](dev-plan.zh-CN.md#流媒体与媒体认证)，fit 的 I115–I117（决定 040）：I120（配置中的 `media_auth` 和 `streaming`）、I121（策略、检查、`akit media-token`）I122（角色的媒体中心、RTMP、RTSP 和 SRT）、I123（`httpflv`、`hls`）、I124（WebRTC 端口、`stream_whip`、`stream_whep`）、I125（`media_pages`）和 I126（`whip`、`whep` 和 `webrtc_proxy` 上的 `auth`）已完成；媒体库的 `libraries`、`vod` 和 `channels` 等待其那条线；akio 请求 A263–A268 |

## 日志

### 2026-11-28：真正跑在 io_uring 上的 io_uring 测试

- akit 的 io_uring 测试只设了 `AKIO_TEST_BACKEND=uring`，只有自己选后端的测试文件读它；
  akit-tunnel、akit-llm、akit-bittorrent 的测试，TURN 的 TCP 监听器，以及所有运行 `akit`
  二进制的测试都不指定后端，只有 akio 的 `AKIO_DEFAULT_BACKEND`（ADR-029）能切换它们，所以它们
  一直跑在 epoll 上（在基准测试主机上按打开的 io_uring 描述符数确认）。CI 的 io_uring 一行、
  规则 25、评审范围和一致性测试的 README 现在两个变量都设。
- `akit run` 原本完全无法跑在 io_uring 上：akit-listener 和 akit-admin 的 `Options::backend`
  默认是 epoll，且总是显式交给 akio，而显式指定的后端不会被 akio 的环境变量覆盖。两者现在是
  `Option<Backend>`，默认 `None`，只有设置时才指定，因此 `akit run` 的网关监听器、Admin API
  和控制台遵循 akio 的默认和 `AKIO_DEFAULT_BACKEND`；原来自己选后端的测试仍然选择（`Some`）。
  逐对端的 UDP 监听器（HTTP/3、`udp` 服务）仍在 akio 的 epoll UDP 循环上。不加配置字段：见待
  决策第 80 行。
- 在基准测试主机上，设了 `AKIO_DEFAULT_BACKEND=uring` 的 `akit run` 持有 17 个 io_uring 描述符和
  1 个 epoll；不设时为 0 和 18。两个变量都设的全工作区测试（网关第一次跑在 io_uring 上）只失败了
  FastCGI 的 `keep_alive` 和 akit-admin 的 `connections_answered_past_their_time`（单独跑 10 次全过），
  都是已知问题；epoll 下只失败了 `bodies_through_the_proxy`（已修）和
  `hash_password_from_standard_input`（修复中）。

### 2026-11-28：I126，`webrtc_proxy` 上的媒体认证

- I114 的 `webrtc_proxy` 进入 main 后，读取其 `auth` 和 `action`（有 `auth` 无 `action` 即 fit 的 "auth needs action"），处理器在
  媒体服务器看到 offer 之前用 `akit_media::http::HandlerAuth` 检查它：流为 POST 路径解码后的最后一段，必须是流的名称（否则在检查
  之前答复 400，与 fit 相同）；会话的请求不需要凭据即可继续。
- 测试：移植 fit 的 `TestProxyAuth`（`crates/akit-webrtc/tests/auth.rs`：401、403，不是流名称时 400，被拒绝的不会到达媒体服务器；
  token、经由线程池的 bcrypt 用户和 Basic 被放行；会话的 DELETE 不需要凭据）；akit-config 的 `handlers`。

### 2026-11-27：I114，代理的 WHIP 和 WHEP

- `akit_webrtc::proxy`，角色 `webrtc.proxy`（fit 的 `proxy.go`、`relay.go`）：引擎上的一个 UDP 监听器（成员的 access 作为其过滤器）；
  会话及其 ufrag（每个会话至多 4 个）和其 peer 被中继到的地址；新 peer 地址的首个数据报必须是 USERNAME 以某会话 ufrag 开头、
  MESSAGE-INTEGRITY 由其 pwd 签名的 STUN Binding 请求（否则丢弃并计数：非 STUN、未知 ufrag、完整性）；其中继是引擎上自己的
  UDP 套接字（`Engine::bind_udp`，来自 `relay_ip`，从随机端口起尝试 `port_min`..`port_max` 中的端口，跳过已占用的，范围用尽时以
  `no_port` 丢弃），双向中继该五元组，至多 `max_relays` 个、每个会话 8 个；静默 `idle_timeout` 的 peer 由 akio 对其连接的空闲
  超时关闭；没有任何中继的会话在 `session_timeout` 后被遗忘（最后一个中继结束时重新设定定时器）；DELETE 遗忘会话，其中继
  在一秒后关闭。`/metrics`：fit 的代理指标族，`akit_webrtc_proxy_*`。
- 处理器 `webrtc_proxy`（fit 的 `proxyhttp.go`）：offer 去掉 candidate 后发给 upstream 选出的媒体服务器（akit-upstream 的池、akio
  的客户端、`timeout`），受 `max_sessions`（503，`Retry-After: 1`）和 `max_sdp_bytes`（413；应答超出时 502）限制；201 的应答中
  candidate 换成代理端口的（以及其 `c=` 行），其 ufrag 和 pwd 与 `media_addrs` 或其最佳 host 或 server-reflexive candidate（IPv4
  优先）对应记下，`Location` 为端点路径加会话 ID，媒体服务器会话 URL 之下的 `Link` 头移到 akit 的之下；会话 URL 及其下的路径
  交给同一个媒体服务器（DELETE 遗忘会话；带 ICE 重启 offer 的 PATCH 记下其凭据，trickle 的 candidate 回答 204）；akit 无法采用的
  应答回答 502，并在媒体服务器上挂断该会话。`akit run` 运行该角色并提供该处理器：`webrtc` 不再有任何部分被拒绝。
- `webrtc.proxy`、`webrtc.media` 和 `webrtc.turn` 的 UDP 端口与 HTTP 监听器一样使用配置的 `engine.event_loops` 个 I/O 线程（为 0 时
  每核一个）：在 16 核主机上，一个测试进程中的三个 akit 在默认设置下用尽了文件描述符。
- 测试（`tests/proxy.rs`，回环上的三个 akit）：一个 akit 代理端口之后的两个 akit 媒体服务器，各有一个发布者和一个观看者，媒体经
  中继流动，经 akit 的 DELETE；应答只写出代理的端口；非 Binding 请求的数据报以及未知 ufrag 的 Binding 请求被丢弃；
  `max_sessions`；媒体服务器的 404 被传回；只有一个端口的范围中继一个地址并丢弃下一个；`session_timeout`；代理未运行。SDP
  改写（fit 的 `TestRewriteSDP`）、去掉的 candidate、路径和 `Link` 头以及请求 ufrag 的单元测试。媒体测试的辅助代码移到
  `tests/support`。

### 2026-11-27：按长度分帧、客户端已读完整后离开的代理响应体按完成结束

- `proxy`：HTTP/1.1 客户端读完流式响应 `Content-Length` 声明的全部字节后，可能在上游的结束到达 akit
  之前就关闭连接，而它的离开先放弃了上游请求：响应体未完成就被丢弃，body tap 听到的是被截断，
  `cache` 因此不保存一个客户端已完整拿到的响应。现在，按长度分帧且每个字节都已写出的流，在交换提前结束时
  （客户端离开或其他任何错误）按完成结束；未写满长度的仍被截断。这就是 `akit-cache` 的
  `bodies_through_the_proxy` 在 io_uring 上的失败（在 gpu51 上单独运行 30 次失败 10 次，HTTP/1.1 上的
  `/large` 被当作截断）；`a_framed_body_read_whole_before_its_end_is_kept` 用一个放行后才结束响应体的
  h2c 上游让这个顺序必然出现。

### 2026-11-27：node 和 proxy 的隧道按打开的顺序列出

- `the_proxys_pages` 在负载下偶尔失败：它相隔 5 ms 打开两条隧道后，把两者的 `created` 当作字符串比较。`created` 是去掉了毫秒
  末尾零的 RFC 3339（与 fit 的 JSON 写法相同，100 ms 写作 `…:05.1Z`），按文本排序并不正确（`…:05.1Z` 排在 `…:05.105Z`
  之后，`…:05Z` 排在 `…:05.005Z` 之后）。列表本身按墙上时钟的毫秒、再按隧道 ID 排序：同一毫秒内会相等（而跨链路时 ID
  并不代表先后），时钟回拨时也会出错。现在隧道表在接收隧道时给它们编号，并按这个顺序列出，如同 fit 按其单调时钟排序；
  `created` 仍是页面显示的值。测试在第一条隧道回显之后才打开第二条，不再 sleep，并按各自承载的字节数区分它们。

### 2026-11-27：隧道的测试绑定端口 0

- `tunnels_sealed_by_the_body_password` 在 gpu51 的一次完整运行中（epoll）失败过一次："forward 127.0.0.1:3921: Address
  already in use"。隧道的测试先绑定端口 0 再释放来挑选端口，然后让角色去绑定；其间别的进程可能占用它。现在测试中 akit
  自己的监听器都绑定端口 0，测试再询问它们实际监听的位置：hub 的监听器从其状态取得，forward 从 proxy 的状态取得，console
  socket 从 hub 取得，web proxy 从 proxy 取得，角色的控制台从 `Manager::console_addr` 取得（新增：`Consoles::addr`，由
  `RoleConsoles` 回答）。连接到 hub 的 node 或 proxy 在 hub 绑定端口之后才应用。无人应答的 hub 用端口 1。仍须在绑定前写明
  端口的保留挑选的端口：fit 二进制的监听器、通过页面添加的 forward（与 fit 一样以地址命名）、以及 hub 的 socket 所拨的控制台
  （与 fit 一样拨其成员的地址）。web proxy 测试的读取把对端已重置的连接视为空（此时 macOS 拒绝设置超时）。

### 2026-11-27：隧道 proxy 的节点不再被自身的 apply 丢掉

- `tunnels_with_tokens_plain_and_sealed` 在 gpu51 上偶尔失败（以 `AKIO_TEST_BACKEND=uring` 单独运行 60 次失败 5 次，都在
  第二轮、即加密的那一轮；隧道的测试不读这个变量，其引擎实际跑在 epoll 上）："no tunnel opened"，proxy 以没有任何 hub
  有节点 `lab` 为由拒绝每个连接，而两者都已注册。proxy 的 apply 先启动新 hub 的 uplink、释放 uplinks 的锁，之后才忘掉被
  停止或重新连接的 hub 的节点。hub 足够快时，新 uplink 在这之间就已注册，hub 的节点列表也已送到：刚收下的列表随即被忘掉，
  而只要节点不变，hub 不会再发别的列表。现在节点在释放锁之前就被忘掉，`set_nodes` 持有该锁直到收下列表（如同 fit 的
  `setNodes` 持有它自己的锁），因此为新 uplink 收下的列表总在其后；断开链路的 hub 也在同一把锁下忘掉其节点，且仅当其
  uplink 仍属于该 proxy。修复后：epoll 上单独运行 250 次、io_uring（`AKIO_DEFAULT_BACKEND=uring`）上 50 次，全部通过。

### 2026-11-27：I152，BitTorrent 客户端

- `bittorrent.client`（`akit_bittorrent::client`）由 `akit run` 运行在 akio-bittorrent 的 `Client` 上，在 akio 的 A283 之前使用自己的
  引擎：其种子像 fit 一样保存在 `data_dir` 中（`torrents.json` 和 `torrents/<id>.torrent`，先写临时文件、同步再改名，akit 不读取的
  成员原样写回），akio 的 resume 数据在 `resume/` 中；启动时恢复且不重新校验；读不出的状态文件（不是 JSON、更高的版本、超过
  100,000 个种子、重复或格式错误的 ID、相对目录、超过 64 MiB）使角色不启动，而不是被覆盖；自身文件读不出的种子带着错误继续列出。
  重载原地修改速率和做种限制；`listen`、`encryption`、`dht`、`pex`、`utp`、`lsd`、`disk_workers`、`data_dir`、
  `max_conns_per_torrent` 或 `upload_slots` 的变化从保存的状态建立新客户端，无法建立时（端口被占用）恢复旧客户端。DHT 只在 `dht`
  开启时从 fit 的公共路由节点引导，其名字在自己的线程上解析。
- 其 API（`ClientApi`，供 I157 的 Admin API 使用）：以 .torrent 文件或磁力链接添加（分别至多 128 MiB 和 64 KiB，受 `max_torrents`
  限制）、删除时保留或删除数据、暂停、继续、列出、读取；列表的每次变化立即保存。客户端的事件（元数据、完成、做种限制）只记下发生了
  什么；保存线程每 `save_interval` 写入（磁力链接的 .torrent、`completed`、被做种限制暂停的种子保存为暂停）。控制台的
  `/api/torrents` 和 `/api/torrents/<id>`，在 akio 的池上应答。
- peer ID 为 `-AK0001-`（决定第 75 行）。
- 测试：`client::tests`（fit 的角色测试：两个角色之间下载、重启时不再读取任何分片、磁力链接的 .torrent 被保存并从中恢复、原地和以新
  客户端重载、端口被占用、损坏的状态文件、读取并写回 fit 的状态文件、损坏的种子仍被列出、API 的拒绝、暂停、继续、连同数据删除、控制台
  路由）；`tests/fit_interop.rs` 使用 fit 的二进制：akit 的客户端经由 fit 的 tracker 从 fit 的下载，fit 的也从 akit 的下载；
  `tests/aria2c_interop.rs`（`AKIT_ARIA2C`）：aria2c 1.37 经由 akit 的 HTTP tracker 从 akit 的客户端下载（aria2c 只在开启 DHT 时才
  以 UDP 访问 tracker），在基准主机上的 epoll 和 io_uring 下均通过。`akit_bittorrent::tracker::service` 在配置的路由之外的监听器上
  提供 `bt_tracker`。
- 待审查的决定：第 75 行。

### 2026-11-27：I113，WHIP 和 WHEP：媒体服务器

- `akit_webrtc::media`，`akit run` 运行的角色 `webrtc.media`：akio-webrtc 的 ICE lite `RtcServer` 位于引擎的一个 UDP 监听器上
  （其 candidate 为 `public_ips`、监听地址，或通配地址时每个启用接口的地址；端口 0 先解析为一个空闲端口），成员的 access 作为
  其接受过滤器。会话同 fit：以发布者发送段中第一个列于 `codecs` 的编解码器为每类媒体应答（fit 的 `pickCodecs`，akio 的
  `accept_offer_with`，A259），其观看者使用相同的编解码器；发布者处于 RTP 模式，每个包到达时转发到每个观看者同类的媒体行
  （`Peer::write_rtp`，A260），受 `max_bitrate` 限制、两秒突发量（超出即丢弃并计数），并以 REMB 告知
  （`set_receive_bitrate_cap`，A261）；观看者的关键帧请求以及每个新连接，至多每个 `key_frame_interval` 以 PLI 交给发布者一次；
  `ice_timeout` 作为 peer 的 consent 超时（`set_ice_timeouts(min(8 秒, ice_timeout), ice_timeout)`，A261）；`connect_timeout` 是
  会话自己的定时器。各项限制（PeerConnection、流、每个流的观看者、offer 大小）同 fit 拒绝（503 带 `Retry-After: 5`、409、
  404、413、415、422）。发布者离开时结束其观看者。保持 `listen` 和 `public_ips` 的重新加载保留会话，改变它们的则结束会话。
  `/metrics`：fit 的媒体指标族，`akit_webrtc_media_*`。
- 处理器 `whip` 和 `whep`（`akit_webrtc::handler`，fit 的 `handler.go`）：端点的 POST 和 OPTIONS，会话 URL 的 DELETE 和 OPTIONS
  （PATCH 为 405，同 fit），`Location` 为 POST 的路径加会话 ID，`Link` 头带 `ice_servers` 以及带 `webrtc.turn` secret 生成的限时
  凭据的 `turn_urls`（用户名 `<expiry>:akit-<id>`）。offer 是路由缓冲的请求体，按 `max_sdp_bytes` 检查；处理器内联应答。它们
  按所服务的引擎找到媒体服务器。`akit run` 提供它们（`webrtc_proxy` 等待 I114）。
- 测试（`tests/media.rs`，akio-webrtc 的 `Endpoint` 经 akit 监听器发布和播放）：发布者的音视频到达两个观看者；DELETE 一个观看者，
  然后 DELETE 发布者并结束其观看者；各项限制；HTTP 边界；保留会话的重新加载和重启；中途断开的观看者不拖住别人；
  `connect_timeout`；`max_bitrate`。SDP 读取、所选编解码器、`Link` 头和码率桶的单元测试。fit 的真实 WHIP/WHEP 客户端：fit 的
  媒体测试使用 Go 写的 fib PeerConnection，此处未运行（决定第 54 行）。

### 2026-11-27：I112，基于 akio 钩子的 TURN 服务器

- 基于 akio 的 A256-A258（`d8d51f61`）：中继绑定在 `port_min`..`port_max` 内（范围用尽时回答 508），nonce 用
  `nonce_secret` 生成，TCP 或 TLS 客户端的消息受 `max_stream_message` 限制；`user_bandwidth` 是 fit 的限速器
  （`akit_webrtc::limiter`：每个用户名和方向一个容量为一秒的令牌桶，至多 65,536 个，已填满的每 10 秒清除）作用于 akio-turn
  的 `on_relay`；事件（`on_event`）被计数并在 debug 级别记录；重新加载的配额、上限和生存期在运行中的服务器上设置
  （`set_limits`），不再启动新服务器；被重新加载替换的服务器进入排空（`drain`：新 allocation 以 508 拒绝），直到
  `-shutdown-timeout` 后关闭（不像 fit 那样在清空时提前结束）；服务器自身的地址可在其中继端口上（`holds_relay`）以及
  `allowed_peers` 列出的端口上访问。I111 临时方案的警告去掉，`mptcp` 的保留。
- `/metrics`：fit 的 TURN 指标族，名为 `akit_turn_*`：allocation、待定 allocation、permission、channel、排空中的服务器、按方向
  的中继字节数、丢弃（带宽）、未中继、分配、刷新、按原因释放、按错误码拒绝、认证失败、被拒绝的 peer、重启
  （`akit_turn_relayed_datagrams_total` 去掉，因 fit 计字节数）。
- 测试：fit 的 `TestCaps`（508 被计数，就地提高上限）、`TestBandwidth`、`TestOwnAddress`（两个客户端经对方的中继地址互相
  中继）、`TestReload`（就地修改配额，排空中的服务器回答 508）、端口范围（取其端口，用尽时 508，删除后再次可用）、
  `TestLimiter`；fib 互通测试（`tests/fib_turn.rs`）对 akit 和 fit 结果相同，在 macOS 上运行。

### 2026-11-27：I125，媒体页面

- `media_pages`（`akit_media::pages`）：fit 的页面，即门禁固定版本的 `handler/mediapages/web`，由 `tools/media-pages.py` 按控制台
  的改名规则（决定 037）复制，`make compat-check` 将其与 fit 改名后的文件逐字节比较；hls.js 用控制台的那份。首页带入前缀和标题，
  ETag 和 304，内容安全策略列出各端点的其他源；`api/config` 与 fit 相同（键按顺序）；基于策略用户的登录（`Policy::login`，bcrypt
  用户在线程池上检查，请求等待），答复策略签名的 token 和 cookie `akit_media_token`（TLS 下带 Secure）；`api/logout`。
- 房间 API（`pages::rooms`）：join、members（心跳）、leave 和事件流；成员在 `member_ttl` 内无心跳即被移除，房间为空后被遗忘，
  由房间自己每三分之一 `member_ttl` 触发的定时器完成；同一引擎上一个前缀的房间由服务它的各个配置共享，重载时保留。观看者的流
  写入时不等待：排队超过 64 KiB 后等待排空（akio 的 `BodyStream::on_drain`，它可能在注册等待者时立即运行，在观看者的锁下处理），
  只保留最新的房间状态和有界的聊天事件队列（超过 256 个事件或 1 MiB 即切断），`stall_timeout` 内没有排空则被切断。
- 聊天（`pages::chat`，fit 的 I118）：文本消息和文件，每个房间保留最新 64 条，hub 的文件字节有上限（64 MiB，丢弃最旧的文件），
  下载的 `Content-Disposition` 与 Go 的 `mime.FormatMediaType` 写法相同。
- 在参考文档中记录的差异：文件的请求体由监听器整体读入（在其 `max_body_bytes` 之内）后才检查成员，而 fit 在读取前预留字节；重载
  不排空事件流，它们在新配置下继续。`libraries`、`vod` 和 `channels` 作为尚未支持拒绝；`api/library` 和 `api/play` 答复 404。
- 测试：`crates/akit-media/tests/pages.rs`，移植 fit 的 `TestPages`、`TestLogin`、`TestParams`、`TestLoginWithoutUsers`、
  `TestRoom`、`TestRoomTTLAndLimits`、`TestWatcherCutAndSlow`（不读取的观看者被切断，另一个看到每个成员的最终状态）、
  `TestReload`、`TestPagesConfigDefaults`、`TestPagesOff`、`TestChatText`、`TestChatBounds`、`TestChatOff`、`TestChatFile`、
  `TestChatFileTooLarge`、`TestChatHistoryCap` 和 `TestChatFileEviction`；workers 检查的用例。门禁接受 fit-examples 的
  `media-pages`；`video-site` 只因 `channels` 和 `libraries` 被拒绝。

### 2026-11-27：I126，`whip` 和 `whep` 上的媒体认证

- akit-config 读取 `whip` 和 `whep` 的 `auth`（配置中的一个策略，跨文件检查，并像 `httpflv` 那样交给处理器）；`webrtc_proxy`
  的 `auth` 和 `action` 在角色 `webrtc.proxy` 运行（I114）之前仍作为尚未支持拒绝。
- `akit_media::http::HandlerAuth`：处理器的策略，在引擎就绪后找到，每个请求无锁读取，由 `httpflv`、`hls`、`stream_whip`、
  `stream_whep`、`whip` 和 `whep` 共用。所指定策略不存在（其配置无法编译）的处理器现在以不可用（503）拒绝每个请求，而不是像
  I123 的处理器那样不经检查放行。
- akit-webrtc 的 `whip` 和 `whep`（akit-webrtc 现在依赖 akit-media）：POST 在检查流名称之后、读取 offer 之前经过守卫（对该名称的
  publish 或 play），与 fit 相同；OPTIONS 和会话的 DELETE 不需要凭据。
- 测试：`crates/akit-webrtc/tests/auth.rs`，移植 fit 的 `TestHandlerAuth`（使用 akio-webrtc 的 `Endpoint`，角色 `media_auth` 和
  `webrtc.media` 在同一引擎上）：带质询的 401 和带 `invalid_token` 的 401、403，以 token、Basic、bcrypt 密码（经由线程池）、
  查询参数中 akit 签名的 token 得到 201；重载就地修改策略，`public` 开放 play；不带 `auth` 的重载开放路由。akit-config 的
  `handlers`（`auth` 指向不存在的策略）；门禁接受 fit-examples 的 `media-pages`，只剩 `media_pages` 本身。

### 2026-11-27：I124，流媒体 III：`streaming.webrtc`、`stream_whip` 和 `stream_whep`

- 角色的 WebRTC 端口：监听器引擎的一个 UDP 监听器上的 akio-webrtc ICE lite `RtcServer`（受角色的 `access` 约束，重载时重新设置），
  承载 akio-media 的 WHIP 和 WHEP 会话（`RtcMedia`、位于 fit 的 `/whip/`、`/whep/` 和 `/rtc/session/` 的 `WhipServer`），支持
  `max_sessions`（服务器的会话数和对端数）、`connect_timeout`、作为 `Link` 头字段的 `ice_servers`（fib 的 `linkHeaders`），
  host 候选与 fib 的 `hostAddrs` 相同：`public_ips` 加上端口；通配地址时为每个已启用接口上不是回环、链路本地或组播的地址。任何
  字段变化都会重启它，与 fit 的 `rtcKey` 相同；角色的 `Current` 把它交给处理器。`akit run` 不再拒绝 `streaming.webrtc`。
- `stream_whip` 和 `stream_whep`（akit-config 的 `StreamRtc`：`stream`；`prefix` 默认为 `/whip/` 和 `/whep/`；`auth` 对照策略
  检查）：fit 的 `rtcHandler`，POST 经过 I121 的守卫（publish 或 play）后以服务器自己的路径交给服务器，其答复的 `Location` 改写到
  POST 的路径之下（响应过滤器）；会话的 PATCH、DELETE 和 OPTIONS 不需要凭据（32 位十六进制）；空的或超过 `max_offer_bytes` 的
  offer 或 PATCH 请求体答复 400；前缀之外或没有流时 404；角色未运行或没有 `streaming.webrtc` 时 503。akio 服务器的 CORS 头关闭，
  与 fit 相同。
- 临时做法：`play_wait` 不起作用（对没有推流者的流的 WHEP offer 立即 404，设置时记录一条警告），推流者离开时观众立即结束
  （akio A267）；端口为 0 时在监听器绑定之前用探测套接字找端口（akio A268）。
- 测试：`crates/akit-media/tests/rtc.rs`，经 akit 的监听器、角色在同一引擎上，在回环上使用 akio-webrtc 的 `Endpoint`（fit 的
  `TestStreamWHIPWHEP`、`TestHandlerPaths` 和 `TestNotRunning`）：带质询的 401，只能观看的 token 为 403，没有媒体、为空或超过
  上限的 offer 为 400，DELETE 不存在的会话为 404；RTMP 流的 WHEP 观众收到其媒体，会话位于 `/whep/live/cam/<id>` 并带 ETag，
  DELETE 结束它；WHIP 推流者的 H.264 可通过 RTMP（拉入测试自己的 hub）和 HLS 播放，DELETE 其会话结束该流；端点的 OPTIONS 列出
  POST；没有 `streaming.webrtc` 时 503。`crates/akit/tests/media.rs`：ffmpeg 的 WHIP 复用器向 `akit run` 和 fit 的二进制推流，
  结果相同：没有 token 时被拒绝，ffprobe 通过 RTMP 和 HLS 播放。另有 akit-config 的 `rtc_handlers`、workers 检查的两个用例，
  门禁中 fit-examples 的 `streaming` 有效。

### 2026-11-27：I123，流媒体 II：`httpflv` 和 `hls`

- akit-config 按 fit 的方式读取这两个处理器的参数（`prefix`，读取后以 `/` 结尾，`auth`、`wait`、`cors`；HLS 的 `format`、
  `target_duration`、在 100ms 到 5s 之间的 `part_duration`、至多 100 的 `playlist_size`、`max_bytes`、`play_wait`、
  `idle_timeout`），每个 `auth` 跨文件对照配置的策略检查，并把策略的配置交给处理器。
- `akit_media::handlers`（在 `STANDARD` 中，`akit build` 的 `akit-media`）：处理器在引擎就绪后（`EngineSlot::when_set`）
  找到角色的槽位和其策略，在 `media_auth` 角色尚未应用该策略时按配置编译它，并在每个请求中无锁地读取两者。路径中没有流时
  答复 404（fit 的 `validStream`），角色未运行时答复 503，每个请求都经过 I121 的守卫；然后交给 akio-media 的 `HttpFlv`
  （HTTP-FLV 和 WebSocket-FLV）或 `Hls`，角色的 hub 换了时重新创建（每个 hub 有编号）。`hls` 保留一个 fMP4 和一个 MPEG-TS 的
  `Hls`，播放列表按 `?format=`、分片按扩展名选择格式，同一引擎上参数相同的处理器共享它们，使重载保留分片；带 `?token=` 获取的
  播放列表给其中的每个 URI 加上 `token=`（响应过滤器，fit 的 `withQuery`）。有前缀时，调用 akio-media 之前把路径设为去掉前缀的
  路径，访问日志随后写的也是它（akio A266，与一个 `Hls` 提供两种格式一起）。
- 测试：`crates/akit-media/tests/handlers.rs` 经由 akit 的监听器、角色在同一引擎上：通过 RTMP 推送的流可用 bearer token 和
  `?token=` 通过 HTTP-FLV 播放，不带凭据时答复带质询和处理器 CORS 的 401，路径中没有流时答复 404；HLS fMP4 和 MPEG-TS 的播放
  列表会转交 token，其分片带 token 可播放、不带时被拒绝；没有角色时答复 503。`crates/akit/tests/media.rs`：ffprobe 经由
  `akit run` 和 fit 的二进制同样可以播放 ffmpeg RTMP 流的 HTTP-FLV 和 HLS（fMP4、MPEG-TS）。fit-examples 的 `streaming` 现在只因
  `stream_whip` 和 `stream_whep` 被拒绝。

### 2026-11-27：I122，流媒体 I：角色的媒体中心及其 RTMP、RTSP 和 SRT 监听器

- `akit_media::streaming`，即 `akit run` 中的 `streaming` 角色（在 `ROLES` 中位于 `media_auth` 之后）：带有 fit 上限
  （`max_streams`、`max_players`、`gop_cache_bytes`）的 akio-media hub，其监听器加到监听器的引擎上（`Engine::add_listener`）：
  RTMP（`RtmpServer`：`handshake_timeout`、`idle_timeout`、`play_wait`）、RTSP（`RtspServer`，交织 RTP，请求 UDP 的 SETUP
  答复 461：`session_timeout`、`play_wait`）和 SRT（UDP 监听器上的 `SrtServer`：`latency`、`passphrase`、`max_conns`、
  `peer_idle_timeout`、`play_wait`）。RTMP 和 RTSP 的 `max_conns` 由角色自己包在 akio-media 外面的接受器实现：超出时连接一打开
  就关闭，对端离开时计数减一（`Conn::on_peer_gone`），与 fit 的 `connLimit` 相同。
- `auth` 在 hub 的钩子中、在 I/O 线程上检查，监听器的策略经由每线程缓存读取：RTMP 和 RTSP 用查询参数中的 `token`，或 `user` 和
  `pass`；SRT 在服务器的连接钩子中用 `s=`（token）或 `u=` 和 `s=`（用户的密码），或普通 stream ID 的查询参数，以 1401、1403 或
  1402 拒绝（fit 的代码）。尚未记住的 bcrypt 用户先以“再试一次”拒绝一次，同时池检查其密码。处理器的协议直接通过钩子（它们事先
  检查），每个放行的推流都会计数。
- 重载与 fit 相同：`auth` 和 `access` 就地修改（`Engine::set_listener_filter`），其他字段变化的监听器重启（两者可能共享地址时
  先关旧的，新的启动失败时重启旧的），hub 的上限变化时 hub 和每个监听器都重启（`akit_streaming_restarts_total`）。角色的指标经由 `Role::metrics`（WebRTC 路线加入的角色钩子） 出现在 `/metrics` 上（`akit_streaming_streams{state}`、`_players`、`_published_total`、
  `_auth_refused_total{protocol,reason}`、`_restarts_total`）。
- 临时做法，各有一个 akio 请求：RTMP 的拒绝采用 akio-media 的状态（`NetStream.Publish.Denied`、`NetStream.Play.StreamNotFound`），
  其握手期限在 C2 结束（A263）。RTSP 只从 URL 的查询参数中取凭据，拒绝时答复不带质询的 403，并在 ffmpeg 下又暴露出两个差距
  （A264，已扩充）：akio-media 的 `Content-Base` 省去了查询参数，因此播放的 SETUP 和 PLAY 不带凭据（播放需要策略的公开播放）；
  ffmpeg 在带查询参数的 ANNOUNCE 之后发出的 SETUP 把轨道写在查询参数之后（`.../cam?token=T/streamid=0`），得到 404，因此 ffmpeg
  无法用查询参数中的 token 推流（fit 两者都接受；akio 自己的 RTSP 客户端可以推流）。SRT 的 `r=live/cam?token=T` 以 1400 拒绝，
  而不是在带查询参数的路径下推流（A265）。在 I124 之前 `akit run` 拒绝 `streaming.webrtc`。
- 测试（`crates/akit-media/tests/streaming.rs`，基于 akio-media 自己的客户端，每次等待都以 hub 或客户端的状态为条件并带期限，
  连续运行十次均通过）：RTMP 推流不带凭据时、以及 token 不可推送的流被拒绝，带 token 和用户查询密码时被接受，并能回放；RTSP 带
  查询 token 推流、不带时被拒绝，没有公开播放时播放被拒绝、有时被接受；SRT 的 1401、1403、1400 和 1404，带 token 和带用户的推流，
  普通 stream ID 的播放；`max_conns`；访问控制拒绝、重载放行下一个连接；hub 上限变化结束流；就地修改 `auth` 时推流者保持连接。
  `crates/akit/tests/media.rs`：ffmpeg 通过 RTMP（不带 token 时被拒绝）以及 RTSP 和 SRT（若其构建带 libsrt）推送测试图案，ffprobe
  通过 RTMP、RTSP 和 SRT 播放；同样的命令对 fit 的二进制（`AKIT_FIT_BIN`，从 fit `b6c5cebf` 的源码构建）给出相同的结果，只有
  ffmpeg 带查询 token 的 RTSP 推流例外（见上）；`/metrics` 列出角色的指标。

### 2026-11-27：I121，媒体认证 II：策略、检查、`akit media-token`

- 把 fit 的 `mediaauth` 做成新 crate `akit-media`（决定 040）：按 SHA-256 查找的静态 token；akit 以 fit 的格式签发的 token
  （`fm1.`、base64url 的声明、在由密钥和策略名以 fit 的标签派生的密钥下的 HMAC-SHA256）；明文密码以常数时间比较的用户，或在
  crate 自己的有界池上检查 bcrypt 哈希的用户（CPU 数的四分之一、最多 256 个等待、5 秒，与 fit 相同；验证正确的密码以策略随机
  密钥下的 HMAC 记住；未知用户对照一个诱饵哈希检查，使拒绝耗时相同）；公开的动作；与 fit 相同的 `*` 模式匹配。
- 按引擎和名字查找的策略（`auth::registry`），在多次重载之间是同一个对象：`media_auth` 角色（`akit_media::role::MEDIA_AUTH`，
  位于 `ROLES` 首位）应用每个配置，先编译所有策略再修改任何一个，并把被省略的策略标为已移除（持有者得到 503）；先完成准备的
  模块按 fit 的 `Provision` 用正在编译的配置编译其策略。`secret_file` 在每次应用时读取，并通过 `Role::files` 报告，因此其变化时
  `-watch-certs` 会重新应用配置。状态经由控制线程所交换内容的每线程缓存读取（`swap::Swap`），因此检查不取锁、不触碰共享计数。
- HTTP（`akit_media::http`）：请求的凭据（`Bearer`、`Basic`、`?token=`、cookie `akit_media_token`），查询参数中的每个 `token`
  和 `sig` 在目标中写为 `REDACTED` 供访问日志使用，守卫（401 带 `Bearer realm="akit"`，凭据无效时带 `error="invalid_token"`，
  403，503 带 `Retry-After`；bcrypt 检查期间挂起请求，在池上恢复），以及播放列表转交的访问凭据。
- `akit media-token`（`-c`、`-policy`、`-publish`、`-play`、`-ttl`、`-subject`），与 fit 相同。
- 测试：移植了 fit 的 `TestMatch`、`TestAllow`、`TestPublic`、`TestBcrypt`、`TestIssue`、`TestRegistry`、密钥文件和
  `TestGuard`（`crates/akit-media/src/auth/tests.rs`、`tests/guard.rs`，基于 akio HTTP）；接受 fit 二进制签发的 token（`fit
  media-token`，fit 从 `b6c5cebf` 的源码构建），且 akit 对相同声明签出相同的字节；`akit media-token` 的 token 被接受，其用法错误
  与 fit 相同（`crates/akit/tests/media.rs`）。
- 未在此完成：媒体页面所用的 fit `Login`（I125）、密钥环与轮换（fit 的 I119，媒体库那条线）、签名的播放 URL。

### 2026-11-27：I120，流媒体与媒体认证 I：配置中的 `media_auth` 和 `streaming`

- 路线开始：在计划中把 fit 的 I115–I117 拆为 I120–I126（决定 040），依据 `config/mediaauth.go`、`config/streaming.go`、
  `mediaauth`、`streaming` 和 `handler/mediapages`。
- `akit-config` 的 `media` 模块按 fit 的方式读取这两个成员并做 fit 的检查：名字、token 及其 SHA-256、用户和 bcrypt 哈希、至少
  16 字节的密钥、流模式、监听器的地址（参与“一个地址一个服务器”的检查，`streaming.rtmp listens on TCP :1935 already`）、SRT
  口令的长度、`public_ips`、`ice_servers` 及其 `Link` 安全性、至多 64 KiB 的 `max_offer_bytes`，以及监听器引用的策略（读完所有
  文件后检查）。RTSP 的 `realm` 读取时默认值为 akit 的（决定 037）。
- 作为“尚不支持”拒绝：`keys`、`url_signing`、`channels`、`push`（后续路线）、`mptcp`、RTMP 的 `max_message_bytes`（akio A263：
  akio-media 的 RTMP 服务器的上限固定为 16 MiB），以及 RTSP 的 `auth_methods`、`realm` 和 `idle_timeout`（akio A264：没有用于
  Digest 和 Basic 的请求钩子，也没有无会话连接的空闲上限）。`akit run` 在 I121 之前拒绝 `media_auth`，在 I122 运行 `streaming`
  之前拒绝它；`akit validate` 检查两者。
- 门禁：fit-examples 的 `streaming` 只因其处理器被拒绝；`bittorrent`、`media-pages` 和 `video-site` 不再因 `media_auth` 被拒绝。
  配置参考包含这两个成员的表格，v1 冻结把 59 个字段标为新增。
- 测试：移植了 fit 的 `TestMediaAuth` 和 `TestStreaming`（`crates/akit-config/src/media/tests.rs`）、角色地址的冲突，以及每个
  读取字段的样例（`tests/compat`）。

### 2026-11-27：I139，用户、角色和审计日志

- 角色（`akit_cluster::roles`，fit 的 `cluster/roles.go`）：viewer、operator 和 admin，集群范围或在分组上（每个用户 64 个，256 个用户），
  成员上的角色是集群角色与其所在分组角色中最高的，未列出的控制台用户有默认角色，`admin.cluster.admins` 中的用户无论角色如何都是集群范围的
  admin；在设置角色之前是开放的（每个用户都是 admin）。保存为文档的 `access`，即 fit 的，文档不再因此只读（文档按版本 3 写出）；分组改名
  或删除时跟随。
- 每个端点都像 fit 那样检查：读取需要在某处有角色，修改需要集群范围的 admin，自行检查所涉对象的端点除外：成员的添加、修改、删除或草稿测试
  （其 admin）、按保存状态测试（operator）、读取（viewer）；节点切换器（viewer，修改需 admin）；操作（每个成员的 operator）；同步、期望配置、
  导入和比较（viewer）；发布（模板或同步发布需成员的 operator，复制和补丁需 admin，复制来源需 viewer；模板草稿需集群范围的 admin）；静默
  （其成员或其规则分组的 operator）。拒绝以 403 回答所需的角色及范围（fit 的 `ErrForbidden`）。控制台用户名 `Admin API` 被拒绝。
- 有角色的管理端自身的控制台：读取、集群模块的端点、节点切换器、修改密码、文本工具和试运行对任何人开放；其余需要集群范围的 admin，经由
  `akit-admin` 控制台向集群模块询问的 `Module::console_gate`（fit 的 `ConsoleGate`）。
- `GET /api/me`（用户在集群、每个分组和每个成员上的角色）、`GET`/`PUT /api/access`（每个控制台用户、管理员、分组；`If-Match`；至少保留
  一个集群范围的 admin），记入审计日志为 `access.update`。审计日志的筛选和 JSON 行自 I132 起已有。
- 测试：`roles::tests`（角色的解析、检查、保存、改名、拒绝说明）；akit 的 `roles_and_audit`（开放，然后设置角色、没有 admin 时拒绝；viewer
  和分组的 operator：分组、操作、读取、节点切换器、成员、静默以及管理端自身控制台的允许或拒绝；分组改名；重启后保留；审计日志的 JSON 行）。

### 2026-11-27：I138，漂移和告警；akit 上的一致性检查

- 每个 akit 上的一致性检查（`akit_admin::check`，fit 的 `admin/drift.go`）：`GET /config/check` 回答运行中的状态与其所依据配置之间的
  差异，每一项是带修复的漂移或说明：需要重启才生效的设置（配置警告中列出的那些，对照与历史一起保留的版本 1）、监听是否绑定以及动态
  端口、所需与所提供的证书、上游是否运行以及是否所有后端都已下线、角色的设置与运行、缓存区的使用与保留、提供者提供和略去的内容。
  运行中的状态从 `GET /status` 和 `GET /node` 的写出函数读取；值回答时已脱敏。`POST /config/reconcile` 以检查的 ETag 作
  `If-Match`（没有则 428），重新检查并把所列修复（删除放最后，从数组末尾开始）作为新版本应用。能力 `config.check`，于是管理端的轮询会
  读取漂移，发布的漂移门槛也适用于 akit 成员；控制台的一致性页面可用。v1 新增。
- 告警（`akit_cluster::alerts`、`alerteval`，fit 的 `cluster/alerts.go`、`alerteval.go` 和 `alertapi.go`）：fit 的六条默认规则（成员
  下线、按模板或按检查的漂移、发布失败、证书临近过期、后端下线、重载失败），针对某个分组或全部成员，各自先待定满其时间，然后触发，按
  重复间隔再次发送，条件消失后解除（并发送给要求的 webhook）；按规则、成员或两者静默；64 条规则、16 个 webhook、256 个静默、跟踪 2,048
  个告警并保留 500 个已解除的。保存在存储的 `cluster.alerts` 分区，即 fit 的文档。规则在管理端自己的线程上评估：一次轮询结束、规则、
  webhook 或静默改变、成员的模板被读取或发布结束时，以及其自身最早的时间到来时；受模板管理的成员在运行配置改变时与模板对照读取，每次
  轮询一次。
- Webhook（`akit_cluster::webhooks`，fit 的 `webhooks.go`）：JSON、Slack、Discord、飞书、钉钉、企业微信和自定义模板；https，或到回环
  地址的 http；用密钥签名（`X-Akit-Signature`、`X-Akit-Timestamp`；飞书和钉钉用各自的签名）；每个一个发送者，在 akio 的客户端上，最多
  64 条通知等待，每条在其超时之内；URL 的路径和查询以及密钥从不回答或记录；测试回答其结果。
- 漂移（`GET /api/drift`、`POST /api/drift/fix`，fit 的 `drift.go`）：每个成员的模板漂移和检查漂移；修复会为所列成员计划一次同步发布。
- 端点：`/api/alerts`、`/summary`、`/config`（带 ETag；修改时用 `If-Match`）、`/rules`、`/webhooks`（以及 `/<name>/test`）、
  `/silences`；修改在管理端的线程池上进行，记入审计日志（`alert.*`）。
- 测试：`check::tests`、`alerts::tests`、`alerteval::tests`（告警的生命周期、其时间、每种规则的发现）、`webhooks::tests`（检查、渲染、
  签名、聊天服务的拒绝）；akit 的 `consistency_check`（一处漂移、其修复的 reconcile、428、未提供的修复）和 `alerts_and_drift`（默认规则、
  测试 webhook 并核对签名、成员下线触发并发送、静默及结束静默、模板漂移由发布修复、检查发现的漂移触发规则、告警在重启后保留）。

### 2026-11-27：I137，带健康门槛的发布

- 发布（`akit_cluster::rollouts` 和 `rollrun`，fit 的 `cluster/rollouts.go`、`rollrun.go`、`rollsteer.go`、`rolldraft.go` 和
  `rollapi.go`）：模板的发布，或模板草稿的发布（发布开始时保存为模板，如果计划之后模板已改动则拒绝）；与模板同步；把一个成员配置的部分
  复制到其他成员；部分补丁（不含变量）。计划读取每个成员：被占用的成员，以及没有列出 `config.if_match`、`config.dry_run` 和
  `config.versions` 的成员，不就绪；其期望配置、发生变化的部分及其同步状态，以及以读到的 ETag 作 `If-Match` 的试运行，决定它是
  就绪、未变化还是出错。每个发布最多 8 MiB 的配置和差异；计划一小时后过期。
- 策略：一次全部、滚动（按数量或百分比），或金丝雀，之后等待人工继续；批次之间的暂停（最多 24 小时）；门槛失败时回滚该批，或回滚所有已
  应用的成员。每个成员以计划时的 ETag 作 `If-Match` 应用；412 或丢失的回答由其 `GET /node` 的 ETag 判定（已应用、因计划后被改动而
  跳过，或不确定）。门槛在 `wait_ms` 或 5xx 时间窗之后检查：成员可达且运行新的 ETag；在列出 `config.check` 的成员上漂移不多于之前；
  时间窗内由 `/status` 的 `requests` 计算的 5xx 比例，从 `min_requests` 起判断，时间窗在读取到目前为止的请求计数之后开始，此时该批显示
  `gating`（不统计请求的成员在应用任何东西之前就让该批失败）。
  回滚为以新 ETag 作 `If-Match` 的 `POST /rollback/{n}`；通过门槛的批次的成员会被告知管理端在其上拥有什么。
- 操控：暂停（当前批次之后）、继续、继续金丝雀之后（go）、中止、回滚（暂停、等待或结束之后）和丢弃（计划）；每一项都记入审计日志
  （`rollout.plan`、`rollout.start`、`rollout.<action>`、带每个成员结果的 `rollout.end`）。发布占用其成员，排斥操作和其他发布。
- 按 fit 的写法有类型地保存在存储文档的 `rollouts` 中（成员的配置为 base64，与 Go 写 `[]byte` 一样），随文档的每次写入一起写，一次
  只排队一个写入。启动时，执行中的发布在停下处暂停，计划中的过期，回滚中的失败。上限：同时 4 个计划中或进行中，保留 50 个，每个成员
  20 个错误。
- 线程：计划和执行各占一个自己的线程，在 akio 的客户端上调用成员并等待回答，用 scoped 线程同时 `max_concurrency` 个；批次之间的
  暂停和门槛的时间窗在条件变量上等待，由暂停、中止和管理端关闭唤醒（关闭时最多等待 2 秒）。端点：`GET`/`POST /api/rollouts`、
  `GET`/`DELETE /api/rollouts/<id>`、`POST /api/rollouts/<id>/{start,pause,resume,go,abort,rollback}`；会写入的端点在管理端的
  线程池上运行。
- 混合集群：从 akit 成员复制时，其 `[redacted]` 密钥被掩码，每个目标保留自己的（决定 041 第 9 点）；fit 的管理端能读取 akit 管理端
  写下的发布。
- 测试：`rollouts::tests`（策略，即 fit 的 `TestStrategies`；按 Go 写法的时间和数字；保存形式读回；重启时的状态转换；过期和
  裁剪；部分和密钥）和 `rollrun::tests`（变化、请求体、请求计数、密钥）；akit 的 `rollouts_with_gates`（滚动批次；未变化的计划，
  然后丢弃；金丝雀、go、暂停和继续；计划后被改动的成员被跳过；回滚；无法判断的 5xx 门槛和失败并回滚的 5xx 门槛；重启时进行中的发布
  处于暂停并仍占用成员，然后中止；草稿保存为模板；补丁）和 `mixed_clusters_with_fit`（模板在 fit 成员上发布、从 fit 复制到 akit、
  fit 的管理端读取这些发布）。

### 2026-11-27：I136，模板，以及成员知道管理端拥有什么

- 模板（`akit_cluster::templates`，fit 的 `cluster/templates.go`）：配置的一个片段，其顶层成员整体拥有（`admin` 和
  `schema_version` 除外），`listeners`、`routes`、`services` 和 `upstreams` 的元素按名字拥有（有名字、各一次、名字不含变量）；变量
  `${node.name}`、`${node.id}`、`${node.hostname}`、`${node.labels.<key>}` 和分组的变量，`$${` 即原样的 `${`；挂到分组（32 个）、
  有顺序（−1,000 到 1,000），最多 64 KiB 和 256 个部分，最多 128 个模板。密钥（fit 的规则）回答为 `[secret]`，原样发回时保留；
  文件路径给出警告。在存储的文档中有类型，按 fit 的写法；分组改名或删除时跟随。端点：`GET`/`POST /api/templates`（带 `from`
  时，由立即读取的成员配置中的部分生成）、`GET`/`PUT`/`DELETE /api/templates/<name>`。
- 每个成员与其模板对照（`akit_cluster::sync`，fit 的 `render.go` 和 `sync.go`）：其模板渲染出的部分，后面的模板胜出
  （`overrides`）；期望配置，即成员自己的配置，把这些部分放到各自位置，新元素放在末尾；每个部分为已同步、未同步（`missing`、
  `template_changed`）或冲突（`own_value`、`changed_here`、`removed_here`），对照成员保存的该部分上次同步时的哈希；不再被任何模板
  拥有的部分被释放。`POST /api/sync`（`nodes`、`template` 或全部成员；同时 4 个；30 秒内未读到的为 pending）、`GET /api/sync`
  （摘要，不含值）、`GET /api/desired/<name>`、`POST /api/import`（成员的部分，供模板使用）、`POST /api/compare`（两个成员逐部分
  比较）、`POST /api/text/render` 和 `/parse`（模板配置与 `.conf`、YAML 或 JSON 互转，经 `akit_config::adapt`）。成员按
  `max_concurrency` 同时读取，每个回答发起下一次读取。
- 告知成员管理端在其上拥有什么（`PUT /node/managed`，`If-Match`，412 后再试一次）：在成员、分组或模板的每次修改之后，一次一个
  刷新，期间的修改在其后再运行一次（不采用 fit 半秒的延迟：不用与时间无关的定时器）；页面发起的同步也会告知。
- 每个 akit 上（`akit_admin::managed`，fit 的 `admin/managed.go`）：`GET`、`PUT` 和 `DELETE /node/managed`，能力
  `node.managed`；记录（管理端的节点 ID 和主机、每个部分及其模板和哈希）按 fit 的方式检查，保存在存储的 `managed` 分区（fit 的），
  没有存储时保存在内存；对照 API 回答的配置说明每个部分是否存在、是否已修改。标记为 v1 新增。
- 混合集群：哈希是 fit 的（`managed.Hash`：规范 JSON，键排序，数字按原样，Go 的转义），已与 Go 自己的输出核对；akit 成员回答的
  密钥是脱敏的（决定 035），所以对 akit 成员，部分在两边都脱敏后比较并保存哈希（决定 041 第 8 点），对 fit 成员则整体比较。
- 测试：`templates::tests`（变量、配置、密钥、检查）、`sync::tests`（按顺序和变量渲染、每种同步状态、记录、期望配置、脱敏的密钥、
  片段）、`jv::tests`（Go 的哈希）、`managed::tests`；akit 的 `templates_and_sync`（模板在两个成员上的路由：告知、同步、修改为冲突
  并在成员上标记已修改、`from`、导入、比较、文本标签、密钥、分组改名、删除模板后成员不再拥有任何东西）和
  `mixed_clusters_with_fit`（fit 成员被比较并被告知）。

### 2026-11-27：I135，分组和对多个成员的操作

- 分组（`akit_cluster::groups`，fit 的 `cluster/groups.go`）：标签选择器（Kubernetes 的等值条件 `key=value`、`key==value`、
  `key!=value`、`key`、`!key`；按 fit 的写法写回；最多 1 KiB、32 个条件；空选择器不选任何成员）、按名字列出的成员，或两者；
  描述，以及给 I136 模板用的变量（64 个，名字为字母、数字和 `_`，值最多 4 KiB）。成员关系由当前标签计算，从不保存：标签改变
  会把成员移入或移出；没有对应成员的名字被保留并显示为缺失；成员改名或删除时，在列出它的分组中改名或移除；分组改名或删除时，
  在 fit 保存的模板中改名或移除。分组现在在存储的文档中有类型，akit 不读的部分原样保留。端点：`GET`/`POST /api/groups`、
  `GET`/`PUT`/`DELETE /api/groups/<name>`（分组及其成员的状态）、`POST /api/select`（表单的预览）；最多 256 个分组。每个成员的
  状态列出其分组，`/api/me` 列出分组。
- 操作（`akit_cluster::ops`，fit 的 `cluster/ops.go`）：`POST /api/ops`（202）作用于分组、选中的成员或分组中的成员：`reload`
  （`POST /reload`）、`rollback`（回滚到保存的、ETag 与运行中不同的最新版本，带 `If-Match`；没有则 `no_earlier_config`）、
  `check`（`GET /config/check`）、`purge`（按键、前缀、标签或全部，按 fit 的方式检查：每个列表最多 64 个绝对 URL 或标签，每项
  2 KiB）和 `test`（`GET /node`）；先检查每个成员的能力（`unsupported`），其拒绝按 fit 的 `replyProblem` 读取（`refused`，配置
  错误中的第一个）；`GET /api/ops` 和 `/api/ops/<id>` 跟踪它们。同时最多 `max_concurrency` 个成员，每个回答发起下一个成员的
  调用（没有独立线程），同时 4 个操作（429），一个成员同时一个（409），保留 32 个。结束的操作记入审计日志（`op.<name>`、每个
  成员的结果、前后的配置版本、`ok`、`partial` 或 `failed`），不改变成员的 ETag；其成员立即重新轮询。
- 测试：`groups::tests`（fit 的选择器和检查）、`ops::tests`（fit 的 purge 检查、拒绝）；akit 的 `groups_and_operations`（按
  选择器和名字的分组及缺失的名字、预览、标签改变、对有成员下线的分组 reload、无可回滚版本的 rollback 以及经切换器修改后成功的
  rollback、purge、check、test、错误的请求、审计、删除成员和分组）。

### 2026-11-27：I134，从管理端打开成员自己的页面

- 节点切换器的转发（`akit_cluster::pass`，fit 的 `cluster/passthrough.go`）：控制台经模块 `cluster` 提供
  `/api/n/<member>/workbench`、`/node` 和 `/m/<module>/<path>`（模块自己的 `/api/m/cluster/n/...` 回答 404），经成员的控制台
  （带管理端的会话）或其 Admin API（带 token）转发给成员。不转发：成员的用户和账户（403）、debug 和 dashboard（404）、模块
  `cluster`、`ai` 和 `llm.*`（403）、以 token 访问的成员的角色模块（404）、流（501）、为 `.`、`..`、空或含 `\`、NUL 的路径段
  （404）。上限同 fit：每个成员 3 个调用、16 个等待（超过调用超时回答 503），总共 64 个（429），请求体在 `max_response_bytes`
  之内（由控制台接收：`Options::cluster_max_body`）。
- 转发的请求头：`Content-Type`、`If-Match`、`If-None-Match`、`Upload-Offset`、`Upload-Length`、`Akit-Store-Password`（同时作为
  fit 的 `Fit-Store-Password`）；返回的响应头：`ETag`、`Akit-Config-Version` 和 `Akit-Dry-Run`（fit 成员的 `Fit-Config-Version` 和
  `Fit-Dry-Run` 以 akit 的名字返回，决定 041）、`Retry-After`、`Allow`、`Upload-Offset`、`Upload-Length`、附件的
  `Content-Disposition`；浏览器会渲染的内容类型改为 `application/octet-stream`。成员的 401 和要求修改密码的 403 变成带问题的 502。
- 写操作（GET、HEAD、文本转换和试运行之外的一切）在成员的 schema 摘要（按最近一次轮询）与管理端的（`Options::schema`，即
  控制台的）不同，或管理端的成员只读时被拒绝（409）；否则记入审计（`node.api`、结果、前后的配置版本、请求体的 SHA-256，从不
  记录请求体），最多 64 个等待（429）。`/workbench` 立即读取成员的 `/node`，schema 变化时重新轮询，回答带 `read_only` 及原因
  （`not_read`、`version`、`manager`）的成员，以及控制台成员中可转发的模块。没有 `GET /schema` 的成员用管理端的 schema 绘制
  （`Akit-Schema-From: manager`）。fit 成员只读打开：它们的 schema 是 fit 的。
- 测试：`pass::tests`（fit 的路由和写判断、fit 的头以 akit 的名字返回）；akit 的 `node_switcher`（经控制台和 token：带 `If-Match`
  修改路由、过期 ETag 412、试运行、不转发的内容、审计）和 `mixed_clusters_with_fit`（fit 成员只读、`Fit-Config-Version` 和
  `Fit-Dry-Run` 被转换、写被拒绝）。

### 2026-11-27：akio 锁定在 `d8d51f61`：A256-A261

- akio：akio main 上的 `d8d51f61`，akio-turn 中有 A256-A258（ADR-186：中继端口范围、nonce secret、流消息上限；`on_relay`、
  `on_event`、`holds_relay` 和更多计数；在运行中的服务器上设置限制和凭据、排空），akio-webrtc 和 akio-rtp 中有 A259-A261
  （ADR-187：`accept_offer_with`、`Peer::write_rtp`、`set_receive_bitrate_cap`、`set_ice_timeouts`）。akit 的 `rev` 与
  `akit_runtime::AKIO_REV` 随之移动；akit 无需改动即可构建。I112 和 I113 采用它们。

### 2026-11-27：WebRTC 路线在 I111 之后暂停；I113 的 akio 请求

- 本次工作中该路线在 I111 之后暂停：I112 等待 akio 的 A256-A258；I113（WHIP 和 WHEP，基于 akio-webrtc `RtcServer` 的
  SFU）和 I114（代理）尚未开始。为 I113 阅读 akio-webrtc `8a9601b9` 时发现三处缺口，已写成带有计划临时方案的 akio 请求，
  使 I113 不被阻塞：A259，`RtcServer` 上每个 peer 自己的配置（会话应答使用的编解码器；临时方案：配置闭包读取调用线程上
  设置的会话编解码器）；A260，供 SFU 使用的 RTP 转发（临时方案：转发帧，`on_media_data` 到 `write_media`）；A261，向发布者
  发送 REMB 以及按 peer 设置的 ICE consent 超时（临时方案：以丢弃强制执行 `max_bitrate`，不告知；使用 akio 的 consent
  超时）。决定第 53 行。

### 2026-11-27：I111，TURN 服务器

- `akit_webrtc::turn`，`akit run` 运行的角色 `webrtc.turn`（`akit::ROLES`）：akio-turn 的 `TurnServer`，`SOFTWARE` 为
  `akit`，引擎上为 `listen` 的每个地址一个 UDP 监听器、`listen_tcp` 的每个地址一个 TCP 监听器、`listen_tls` 的每个地址一个
  TLS 监听器（akio-turn 的 `stream()` 置于 akio-tls 之下；证书由 akit-tls 加载，每次握手从最近一次应用的证书中选择，并经
  `Role::files` 报告给 `-watch-certs`）；TCP allocation 以 442 拒绝，同 fit。用户的密钥（密码，或十六进制的密钥）和 secret 的
  限时凭据（akio-turn 的 `ephemeral_auth`）在 realm 内经 akit 的 `AuthFn` 读取；peer 规则经其 peer 过滤器读取，与 fit 的
  `peerAllowed` 一样感知端口：先 `allowed_peers`，然后 `denied_peers` 和 akio-turn 对内部地址的拒绝。每个监听器的接受过滤器
  是成员的 access，然后是全局的，每次应用时重新设置。
- 重新加载就地修改用户、secret、peer 规则、access 和证书；其他任何变化（地址、realm、中继地址、配额、上限、生存期、
  nonce secret、流的上限、多路径 TCP）启动新服务器：同一地址上先关闭旧服务器（新服务器无法启动时重新启动旧的），其他
  地址上与旧服务器并存，旧服务器在 `-shutdown-timeout` 内继续服务，然后由定时器关闭。
- 在 akio 的 A256-A258 之前的临时方案（决定 039，第 51 行）：运行时拒绝 `port_min` 和 `port_max`；`user_bandwidth`、
  `nonce_secret`、`max_stream_message` 和 `mptcp` 在服务器启动时说明且不生效；服务器自身的地址只能在 `allowed_peers` 列出的
  端口上访问；配额、上限和生存期启动新服务器；被替换的服务器在排空期间接受新客户端。akio-turn 的中继套接字绑定在引擎的
  UDP 客户端线程上：配置没有 UDP 监听器时只有一个（应由基准测试任务测量，见基准测试表中的行）。
- `/metrics` 加入运行中角色的指标（`Role::metrics`、宿主的 `Meters`，写在其余指标之后）：`akit_turn_allocations`、
  `akit_turn_draining_servers`、`akit_turn_relayed_datagrams_total{direction}`、`akit_turn_auth_failures_total` 和
  `akit_turn_restarts_total`。OTLP 导出尚不包含角色的指标。
- 测试（`crates/akit-webrtc/tests/turn.rs`，移植 fit 的 `turn_test.go`，客户端基于 akio-turn 的编解码器，经 UDP、TCP 和
  TLS）：以密码、密钥和 secret 的凭据经 indication 和 channel 中继；被拒绝的凭据（错误密码、未知用户、过期、另一个
  secret）；配额 486 和 508；上限 508；私有 peer 以及就地修改的规则；自身地址；就地重新加载、同一地址上重启、其他地址上
  排空；被拒绝的重新加载保留服务器（端口范围、已被占用的地址）；TCP 和 TLS；access；akio-turn 自己的客户端
  （`client::Client`）经 indication 和 channel。`tests/fib_turn.rs`（没有 `AKIT_FIB_TURN` 时忽略）：fib 的 TURN 客户端，即 fit
  的，来自 `tests/interop/fib-turn`（基于 fib `webrtc/turn` 的 Go 程序），经 UDP、TCP 和 TLS，以密码、密钥和 secret，经
  indication 和 channel，以及被拒绝的凭据；设置 `AKIT_FIT_BIN` 时，fit 自己的服务器对同样的场景给出相同的回答（在 macOS
  上对照从 `b6c5cebf` 构建的 fit 运行）。
- 同一地址上的重启可能丢失 akio 关闭旧 UDP 监听器套接字期间（在其线程的下一轮）发来的数据报（在 gpu51 的 io_uring 上
  出现过一次）：客户端会重传（RFC 8489），测试的客户端也会重传。

### 2026-11-27：I110，配置中的 `webrtc`

- `akit-config` 的 `webrtc` 模块：`webrtc.turn`、`webrtc.media` 和 `webrtc.proxy`，字段、默认值、范围和检查与 fit 相同
  （`config/webrtc.go`）：监听列表被检查并加入"一个地址一个服务器"的检查（`listen`、`webrtc.media` 和 `webrtc.proxy` 为
  UDP，`listen_tcp` 和 `listen_tls` 为 TCP），`tls` 只与 `listen_tls` 一起，用户带密码或 32 位十六进制的密钥，`relay_ip`
  和 `public_ip`（不能是通配地址，同一地址族），端口范围、配额和生存期（`max_lifetime` 不低于 `default_lifetime`），
  `max_stream_message` 至少 2048，peer 规则同 fit 的 `ParsePeerRule`（前缀、地址、地址加端口），可安全放入 `Link` 头的
  ICE 服务器和 `turn_urls`（`turn_urls` 需要 `webrtc.turn` 的 secret），`codecs` 属于 fit 的五种之一。处理器 `whip`、
  `whep`（`stream`，1 到 128 个 `[A-Za-z0-9._~-]`）和 `webrtc_proxy`（`upstream`、地址或 IP 形式的 `media_addrs`、
  `timeout`、`max_sdp_bytes`、`action`）是 `Handler` 的变体；它们的 `auth`（一个 `media_auth` 策略）作为尚不支持被拒绝，
  并指明模块（决定 039，第 50 行）。
- 新 crate `akit-webrtc`，属于标准模块 crate（`akit build -without` 接受它）：三个处理器像 fit 一样预备（`webrtc_proxy`
  查找其 upstream），在其角色未运行时像 fit 一样回答 503。`akit run` 拒绝这三个成员以及使用这些处理器的路由，并指明运行
  它们的迭代；`akit validate` 检查它们。
- 门禁：`webrtc-turn`、`whip-whep` 和 `webrtc-proxy` 有效；`media-pages` 只因其 `auth` 和 `media_auth` 被拒绝（80 个中
  59 个有效）；89 个字段标为支持并标为 v1 新增；配置参考的 WebRTC 章节和三个处理器的表。测试：移植的 fit
  `config/webrtc_test.go`（成员、媒体、代理）、`bindings_test.go` 和 `more_test.go` 的 WebRTC 用例，以及处理器的参数，位于
  `webrtc/tests.rs`；`binds.rs` 的各类冲突测试加入 WebRTC 各类；`akit run` 的拒绝。

### 2026-11-27：WebRTC 路线规划为 I110–I114

- WebRTC 路线（1.0 之后，第 5 条）按 fit 的 I87 和 I108–I111 拆为五次迭代（[计划](dev-plan.zh-CN.md#webrtc)、
  [决定 039](decisions/039-webrtc.zh-CN.md)）：I110，配置中的 `webrtc`（`webrtc.turn`、`webrtc.media`、`webrtc.proxy`，
  处理器 `whip`、`whep` 和 `webrtc_proxy`）；I111，基于 akio-turn、经 UDP、TCP 和 TLS 的 TURN 服务器；I112，基于
  akio-turn 所缺钩子（A256–A258）的 TURN 服务器；I113，WHIP 和 WHEP，基于 akio-webrtc `RtcServer` 的 SFU；I114，
  代理的 WHIP 和 WHEP。I115–I119 为该路线预留。其基准测试行写明要测量的内容。

### 2026-11-27：I151，BitTorrent tracker

- `bittorrent.tracker`（`akit_bittorrent::tracker`，新 crate，[决定 042](decisions/042-bittorrent.zh-CN.md)）由
  `akit run` 运行，以 akio-bittorrent 的 `TrackerServer` 作为存储：HTTP 的 announce 和 scrape 经由路由的 `bt_tracker`
  处理器（现由 akit-config 读取并在注册表中找到；角色未运行时 503），UDP（BEP 15）在其 `listen` 上，是监听器引擎上的
  一个绑定，角色的访问控制在 akio 的过滤器处执行，随重载移动（端口被占用时保留原来的）。应答与 fib 相同（决定第 78 行）：
  按 fib 的方式读取查询及其 failure reason、`unregistered torrent`、`tracker is full`、`min interval`、`external ip`、
  `peers6` 以及紧凑或字典形式的 peer 列表、默认 50 个 peer、至多 `max_numwant`、UDP 应答不超过 1,400 字节、`max_scrape`
  及 UDP 的 74 个、连接 ID 是以跨重载保留的密钥对分钟和地址计算的 HMAC、伪造的 ID 被丢弃。允许列表可内联也可在文件中
  （在控制线程上读取，像 `-watch-certs` 监视证书一样被监视，错误的行拒绝重载）；HTTP 被角色的访问控制以 failure
  `refused` 拒绝并计数；统计（announce、scrape、按原因计的拒绝）；控制台的 `/api/stats`、`/api/torrents`（允许列表的）、
  `/api/torrents/<hash>`。
- 请求从以引擎为键的公告板读取角色的快照，经由每个线程自己的缓存，仅在应用配置改变它时刷新：每个请求没有锁，也不触及
  共享的引用计数。
- 在 akio 的 A285 之前的临时做法：akit 自己的 HTTP 和 UDP 实现（第 78 行）；新的上限建立新存储（第 77 行）；没有 peer
  计数，公开 tracker 的种子不列出（第 79 行）。
- `akit-bittorrent` 加入 `STANDARD`（`akit build -without akit-bittorrent` 不包含它）。
- 测试：`tracker::tests`（fit 的 tracker 测试：用 akio-bittorrent 的 tracker 客户端经 HTTP 和 UDP announce 和 scrape、
  错误查询和上限、存储的上限、允许列表及其文件、访问控制、手写的 UDP（垃圾数据、伪造 ID、1,400 字节的应答、74 个
  hash）、重载、逐字节发送的客户端、中断的客户端、角色停止、控制台路由、两个 akio-bittorrent 客户端经由它互相找到）；
  akit 的 `tests/bittorrent.rs`：`akit run` 下的 tracker，以及使用 fit 的二进制（`AKIT_FIT_BIN`，从 fit 源码构建）时
  两个 fit 客户端只经由 akit 的 tracker 互相找到，一个经 HTTP、一个经 UDP，文件完整下载。
- 待审查的决定：第 77–79 行。

### 2026-11-27：I150，配置中的 `bittorrent`

- `bittorrent.client` 和 `bittorrent.tracker` 与其余部分一样严格读取（`akit_config::bittorrent`），字段、默认值、上限和检查与 fit
  相同（`config/bittorrent.go`、`config/bittorrent_tracker.go`）：客户端的 `listen` 和 `data_dir`（必填，`data_dir` 不能为空白）、
  `download_dir`（默认 `<data_dir>/downloads`）、`encryption`（`prefer`）、`dht`、`pex`、`utp`（开）、`lsd`（关）、
  `disk_workers`（至多 64）、`save_interval`（5 秒）、`limits`（速率、`max_conns_per_torrent` 50、`upload_slots` 4、`seed_ratio`、
  `seed_time`、`max_torrents` 1,000，至多 100,000）和 `console`；tracker 的 `listen`、`interval`（30 分钟）、`min_interval`（不超过
  interval）、`peer_ttl`（1 小时，长于 interval）、`max_torrents`、`max_peers_per_torrent`、`max_numwant`、`max_scrape`（fit 的默认值
  和最大值）、`allow`（40 或 64 位十六进制的 info hash，至多 1,048,576 个）、`allow_files`、`access` 和 `console`。客户端的端口以 TCP
  和 UDP、tracker 的以 UDP、它们的控制台以 TCP 进入"一个地址一个服务器"的检查。
- 以"尚不支持"拒绝，各自由其迭代或 akio 请求解决：客户端的 `categories`、`watch_dir`、`watch_interval`、`watch_max_files`、
  `url_timeout`（I153）、`alt_speed`（I154）、`access`、`disk_queue_bytes` 以及限制 `max_conns`、`max_half_open`（akio 的 A284）、
  `max_active_downloads`、`max_active_seeds`、`min_free_bytes`（I153）、`publish`（I156）、`rss`（I159）；tracker 的 `max_peers`
  （A285）、`auth`（一个 `media_auth` 策略）和 `store`（I159）。在 I151 和 I152 运行它们之前，`akit run` 拒绝含有任一角色的配置。
- 提出 akio 请求 A283–A288：客户端运行在调用者的引擎上、按地址拒绝 peer 并在运行时修改限制、原地修改 tracker 存储的上限、磁力链接的
  存储目录、等待分片的读取器、tracker 的状态和原地重新校验。
- 测试：`bittorrent::tests`（fit 的 `TestConfigChecks`、`TestBitTorrentTrackerChecks` 和 `TestBitTorrentTrackerBinds`，以及以"尚不
  支持"拒绝的字段）；兼容性闸门为读取的每个字段取样；akit 的 `roles_are_refused_until_they_run`。闸门的 `bittorrent` 示例仍被拒绝
  （其 `media_auth`、`media_library`、`bt_*` 处理器以及上述字段）。
- 待审查的决定：第 76 行。

### 2026-11-27：规划 BitTorrent 路线（I150–I159）

- 1.0 之后的第十条路线按 fit 的 I196–I210 拆分为迭代（[计划](dev-plan.zh-CN.md#bittorrent)）：
  配置中的角色（I150）、基于 akio-bittorrent 的 tracker（I151）和客户端（I152）、添加种子（I153）、
  时间表和访问控制（I154）、`bt_stream`（I155）、发布（I156）、Admin API 和事件（I157）、页面
  （I158）、RSS 和共享的 tracker 存储（I159）。fit 的 I205（互通和故障测试）放入每个角色自己的迭代；
  fit 的 I206（集群视图）等待集群管理路线。基准表中有一行说明要测量什么。

### 2026-11-27：I132 和 I133，集群管理端、其成员和集群页面

- `akit-cluster`（新，fit 的包 `cluster`，其 I141 和 I142）：由 `admin.cluster` 打开的管理端，`akit run` 在监听器的引擎上启动它，
  成员保存在存储的 `cluster` 分区（`admin.store` 或 `-admin` 保存的存储；没有存储则拒绝启动），并在引擎之前关闭它，注销成员的
  控制台会话（最多 2 秒）。存储的文档是 fit 的（`member::Document`）：读取版本 1 到 3，写版本 2，每个列表都在，akit 尚未用到的
  部分（分组、模板、发布、`link_cert`、成员的 `link`）按读到的样子保留；更新的版本或设置了用户角色的文档只读（决定第 73 行），
  不是管理端文档的分区原样不动。成员按 fit 的 `Member.check` 检查（名字、Kubernetes 式标签、`https` 或回环 `http`、控制台或
  Admin API 访问及其凭据、`ca` 或 `fingerprint`），凭据从不出现在回答或日志中。
- 每个成员的传输（`transport`）：akio 的异步 HTTP 客户端，调用、回答头和连接（最多 10 秒）受 `timeout` 约束，正文受
  `max_response_bytes` 约束，一条空闲连接，不恢复会话；TLS 由 pin（akio-tls 的 `pin_sha256`）、成员的 CA 或系统 CA（启动时读取
  一次）校验。轮询器（`node`，独立线程，同时最多 `max_concurrency` 个轮询，按 fit 的方式等待间隔）：`GET /node`，成员列出时再
  读 `/status` 和 `/config/check`，每次调用由前一次的回答发起；控制台登录，遇 401 重新登录，等待中的调用共用一次登录，被拒绝的
  密码等待退避（一分钟起加倍到 15 分钟），成员的 429 按其 `Retry-After`，临时密码被报告并注销其会话；fit 的问题类型和消息，
  其中去掉成员的凭据；状态（up、error、down、unknown）带 fit 的成员字段。fit 成员的版本显示为 `fit <版本>`；CSRF 令牌同时放在
  `X-Akit-Csrf` 和 `X-Fit-Csrf` 中（决定第 72 行）。
- **权宜做法（akio 请求 A277）**：akio 的客户端不给出响应的对端证书链，并丢弃它拒绝的证书，因此成员的证书由一次独立的探测
  读取：一条不做校验的 TLS 连接，发送不带凭据的请求，读取 `tls_info` 的证书链，再用 pin 或 akio-x509 的校验器对照 CA 和 URL 的
  主机检查。它在成员第一次轮询、每次测试以及经 TLS 的调用失败后运行，把后者区分为带证书的 `tls_untrusted` 或
  `tls_pin_mismatch`。
- 控制台模块 `cluster`（`api`）：`GET`/`POST /api/nodes`、`GET`/`PUT`/`DELETE /api/nodes/<name>`（If-Match 成员的 ETag，写入在
  管理端的池上进行，一次一个，16 个等待）、`POST /api/nodes/<name>/test` 和 `/api/test`（同时 4 个）、`GET
  /api/nodes/<name>/read/{node,status,versions,check}`（同时 8 个，成员未列出的能力回答 502）、`/api/state`、`/api/audit`
  （用户、节点、操作或其前缀；JSON lines）、`/api/me`（I139 之前每个控制台用户都是集群范围的管理员）。审计日志记录添加、修改
  （字段名）、删除和测试，1,000 条、约 2 MiB，带文档的修订号。`admin.console` 提供该模块（`Options::cluster`），工作台因而画出
  集群菜单；控制台中配置以外的请求体上限由 64 KiB 提高到 128 KiB（fit 集群模块的上限）。
- I133：在浏览器中针对一个有一个 akit 成员在线、一个下线的管理端检查了概览、节点和成员页面；`docs/guides/cluster.md`。菜单中
  后面的项（分组、模板、发布、漂移和告警、用户和审计、指标）在 I134–I139c 之前回答 "no such endpoint"。
- 测试：`akit-cluster` 的单元测试（按 fit 检查成员、文档的保留与上限、cookie、`fill`、fit 的版本）；akit 的
  `tests/cluster.rs`：`manager_reads_akit_members`（通过控制台和 token、错误的密码、下线的成员、读取、草稿和修改的测试、编辑和
  删除、任何回答和日志中都没有凭据、管理端重启后成员仍在、停掉的成员显示为下线）、`members_over_tls`（不受信任时带指纹、由其
  CA 信任、由其 pin 信任、不匹配的 pin），以及设置 `AKIT_FIT_BIN` 时的 `mixed_clusters_with_fit`（akit 管理端通过控制台和
  token 管理 fit 成员、fit 管理端通过 token 管理 akit 成员、akit 管理端写的存储由 fit 管理端读取）。

### 2026-11-27：I131，成员需要什么

- Admin API 的 `GET /node`，因而也有控制台的 `/api/node` 和 `/api/m/admin/node`（`akit_admin::node`，fit 的 `admin/node.go`）：
  节点 ID 及其保存位置、主机名、`akit_version`、fit 的 `fib_version`（为 `akio <提交>`）和空的 `go_version`（决定第 71 行）、
  操作系统和架构、启动时间和运行时长、运行中配置的版本、ETag、时间和来源、运行中的角色、schema 的版本和摘要（控制台 schema
  的 `sha256:`），以及用 fit 的能力名列出 akit 提供的功能（`node`、`config.if_match`、`config.dry_run`、`config.text`、
  `config.effective`、`config.versions`、`config.transfer`、`config.reload`、`status`、`cache`、`cache.prefix`、`logs`、
  `tokens`、`schema`；有时才列 `metrics`、`store`、`console` 和 `status.requests`）；响应头带 ETag 和
  `Akit-Config-Version`。akit 提供之前不列：`config.check`、`logs.recent`、`node.managed`、媒体、同步和 BitTorrent。
  `GET /schema` 即控制台的 schema。
- 节点 ID（fit 的 `NodeID`）：`admin.node_id`，或只生成一次的随机 UUIDv4，保存在存储的 `node` 分区（fit 的，因此共用存储的
  fit 和 akit 共用它），没有存储时保存在配置旁的 `akit.node-id`（经一个文件改名覆盖写入）；文件中的 ID 被并入还没有 ID 的存储；
  两处都无法写入时不保存，并给出消息。`Configs::use_node`、`node`；`akit run` 在提供服务前设置它，带 akit 的版本和 akio 的
  提交；`Options::node` 读取运行中的角色以及运行中的配置是否统计请求。
- `GET /node` 和 `/schema` 标记为 Admin API 的 v1 新增。
- 测试：`node::tests`（fit 的 `TestNodeIDKept`、`TestNewNodeID`、`TestUseNodeKeeps`）；控制台的 `node_and_schema`（fit 的
  `TestNode`，经 API 和控制台）；akit 的 `node_id_kept_across_restarts`（无存储、并入存储、`admin.node_id`）。

### 2026-11-27：I130，配置中的集群管理

- `admin.node_id`、`admin.cluster` 和 `admin.cluster_member` 按 fit 的 `config/cluster.go` 读取和检查（`akit_config::cluster`）：
  节点 ID 的模式；管理端的 `poll_interval` 和 `timeout`（10s，至少 1s）、`max_nodes`（256，最多 10,000）、`max_concurrency`（8，
  最多 256）、`max_response_bytes`（4 MiB，16 KiB 到 64 MiB）、`link_address`（主机和端口）、`link_url`（wss，需 `link_address`）
  和 `admins`（控制台用户，各一次，至多 64 个），需要 `admin.console`；成员的 `manager`、`fingerprint`，以及 `token`（`akite1.`，
  即 fit 的 `fite1.` 改名）或 `token_file`，二者不能同时设置；消息与 fit 的相同。在 I132 之前 `akit run` 拒绝 `admin.cluster`，在
  I139b 之前拒绝 `admin.cluster_member`，并指名迭代（决定第 74 行）。
- 15 个字段标记为 v1 新增；兼容性清单和配置参考重新生成，两种语言都有 `admin.cluster` 和 `admin.cluster_member` 小节。
- 测试：`akit-config` 的 `tests/cluster.rs`（fit 的 `TestClusterErrors`、`TestClusterLinkSettings`）和 `cluster::tests`
  （`TestClusterDefaults`、辅助函数）；akit 的 `cluster_is_refused_until_it_runs`。

### 2026-11-27：集群管理路线的计划（I130–I139c）

- fit 的 I140–I150 分十二次迭代（[计划](dev-plan.zh-CN.md#集群管理)、[决定 041](decisions/041-cluster-management.zh-CN.md)）：
  配置（I130）、成员需要什么（I131）、管理端及其成员（I132）、页面（I133）、节点切换器（I134）、分组和操作（I135）、模板（I136）、
  带健康门槛的发布（I137）、漂移和告警（I138）、角色和审计日志（I139）、NAT 之后的成员（I139b）、指标、日志和 CLI（I139c）。
  fit 的 I153 随 AI 助手路线进行。成员的 API 和存储的 `cluster` 文档是 fit 的，使 akit 和 fit 成员可以共用任一种管理端。
  等待审阅的决定：第 70–74 行。

### 2026-11-27：I109b，隧道 node 和 proxy 的页面；角色控制台的证书重新读取

- node 的模块（`akit_tunnel::node` 的页面，fit 的 `tunnel/node/api.go`）和 proxy 的模块（`akit_tunnel::proxy` 的，fit 的
  `tunnel/proxy/api.go`），由 I109 的控制台在 `/api/` 以及 `/api/m/tunnel.node/` 或 `/api/m/tunnel.proxy/` 提供：其状态；
  添加、修改（保留密码以及页面不显示的设置，令牌替换密码、密码替换令牌，`quic_pin` 让位于 `pin`）和删除 hub，配置会拒绝的
  端点在此即被拒绝（fit 的 `EditHub`，`akit_tunnel::pages`）；设置 node 的服务；proxy 经某 hub 重新获取 node 列表、经它设置
  某 node 的服务、其 meta、添加和删除转发；按 hub 和 ID 关闭隧道。角色自身的 apply 失败时编辑以 400 应答，同 fit
  （决定第 60 行）。
- 状态显示而角色此前未保存的内容：每个 uplink 的状态，同 fit 的 `uplink.Status`（按位置排列的链路及各自拨号的协议和地址、
  `since`、连接池、端点、密码只显示是否设置、令牌的 ID）；每条隧道的视图（其 hub、node 的地址、本地和远端地址、打开时间、
  双向字节数、node 的连接耗时）；node 到其服务的连接被测量（`service_rtt`，按服务保存，服务删除或地址变化时清除），随 ping
  发给 hub，再由 hub 交给 proxy；proxy 保存每个 hub 的 node，同 fit 的 `NodeList`（其地址、往返时间和服务连接耗时，取自
  hub 的列表，按 epoch 和 version 保留最新的，以及 ping 的应答），以及它们在去往各 node 的隧道中的份额（fit 的
  `Weights`）。proxy 现在按 fit 的 `pick` 选择隧道经过的 hub（fit 的 `Cost`、`Weights` 和 `Choose`，
  `akit_tunnel::proxy::pick`），转发在目标变化时保留其监听器，无法监听的转发连同原因列出。
- `akit run` 提供 `tunnel.node.console` 和 `tunnel.proxy.console`：所有角色的页面都已提供。
- 角色控制台的证书在其文件变化时重新读取（决定第 45 和 61 行）：`-watch-certs` 与 `admin.console` 的证书一起检查它们，
  SIGHUP 时读取，每个控制台对新连接提供新证书，控制台及其会话保留
  （`akit_roles::pages::Consoles::watch_certs`、`RoleConsoles::watch_certs`）。
- 测试：`pages::tests`（fit 的 `TestEndpointAddress`、`TestEditHubRefusesFragment`、`TestEditHub`、`TestDeleteHub`），
  `proxy::pick::tests`（`TestChoose`、`TestWeights`）；`tests/peer_pages.rs`：在 akit 的 hub、node 和 proxy 上移植 fit
  node 的 `TestPages` 以及 proxy 的 `TestProxyAPI` 和 `TestPick`，并在设置 `AKIT_FIT_BIN` 时让 akit 的 node 和 proxy
  连到 fit 的 hub（proxy 的页面显示 fit hub 的 node 及其往返时间和服务连接耗时，经 fit 的 hub 设置 node 的服务，从 node 的
  页面关闭 fit hub 路由的隧道）；akit-admin 的 `a_role_consoles_tls_reload`；akit 的 `node_and_proxy_pages_under_akit_run`
  和 `a_role_consoles_certificate_renewed_without_a_restart`。
- 等待审阅的决定：第 60、61 行。

### 2026-11-27：I109d，LLM 角色供页面使用的方法

- 每个 LLM 角色自身页面的 mesh 方法（`akit_llm::role::admin`，fit 的 `mesh.RegisterAdmin`、`role/settings.go`、
  `role/tokens.go`），只允许调用者 `local`：`/mesh/peers`、`/mesh/dial`（令牌替换密码、密码替换令牌，都不给则保留原有的，
  至多 64 个目标）、`/mesh/undial`、`/mesh/kick`、`/mesh/listen`（空密码保留当前密码，各角色分别处理，除非
  `clear_passwords`；保留已生成的自签名证书；非回环地址只能经 TLS；无法绑定时以 5000 应答并说明原因）、`/mesh/limits/get`
  与 `/set`（写入时省去默认值）、`/mesh/tokens/list`、`/create`、`/expiry` 和 `/revoke`；每次修改都是对成员的编辑
  （`role::edit_member`），应用后才应答。CA 文件在角色自己的池上读取（同 fit：16 个运行、256 个等待，满时 503）。
- proxy 的方法（`akit_llm::proxy::methods`，fit 的 `llm/proxy`）：`/proxy/models`（每个模型及其路由）、`/proxy/stats`
  （自启动起每个模型的调用、错误、响应字节和进行中的调用，至多 1,024 个名字，其余记为 `other`）、`/proxy/keys/list`、
  `/create`（`ah-` 加 32 个随机字节，只显示一次）和 `/delete`（至多 256 个 key）、`/proxy/profile/set`、
  `/proxy/node/list`（每个 hub 的节点列表，取自其推送并在其连上时拉取，以及直连的节点及其链路和流量）、`/proxy/api/get`
  与 `/set`（其证书在池上读取）。key 的最近使用和调用次数在内存中计数。
- 身份的 `http_url`：角色控制台的 URL，由控制台服务器告诉角色其监听地址（`akit_roles::pages::Module::console_url`，
  `RoleConsoles` 在控制台启动和停止时调用），在此之前取自成员的 `console`。
- 测试：`role::admin::tests` 与 `proxy::methods::tests`（fit 的 `TestEditMesh`、`TestAdminTokens`、`TestLimitsSettings`、
  `TestStatsOverflow`、`TestCatalogOrder`、`newKey`、`setAPI`）；`tests/page_methods.rs`（在 akit 的角色上移植 fit 的
  `TestRegisterAdmin`、`TestAdminEdges`、`TestKeysMethods`、`TestProxyRoleEdges`，编辑由与 `akit run` 相同的控制线程写入；
  控制台的端口在启动时才选定，其 URL 进入 `http_url`）；`tests/pages.rs`（经 node 的控制台调用这些方法）；
  `tests/fit_pages.rs`（用 fit 的二进制：akit hub 页面签发的令牌被 fit 的 node 接受，撤销后链路关闭；akit 的 proxy 从页面
  拨号到 fit 的 hub，列出其节点、模型和一次调用）。
- 等待审阅的决定：第 63、64 行。
### 2026-11-27：I109c，LLM 角色的页面；拆出 I109d

- LLM 角色的模块（`akit_llm::pages`，fit 的 `llm/internal/webui`），由 I109 的控制台在 `/api/` 和 `/api/m/llm.<role>/` 提供：
  `GET /api/me`（身份、用户、proxy 的 `api_url`）；对角色路由器的每个方法提供 `POST /api/rpc<method>`，经 `Local`（I106b）以
  角色自己的页面身份调用，应答其 JSON，或 `{"error", "code"}` 及 fit 按代码给出的状态（`status_of`，超时 504），非 JSON 的请求体
  400；对其流方法提供 `POST /api/stream<method>`，以 Server-Sent Events 应答（每帧一个 data 事件，空帧为 `null`，`end` 带结果，
  `error` 带 `{code, message}`，在任何帧之前的拒绝为 JSON），同时至多 16 个（429），每帧只在浏览器取走后才还给流（排队超过
  64 KiB 后，等队列排空），每次等待至多 30 秒，浏览器离开时取消流。修改由方法自己写回（`role::Base::edit`）。每个角色只创建
  一次其模块（`Base::pages`，`Role::module`）。
- `akit run` 提供 LLM 角色的 `console`；隧道 node 和 proxy 的在 I109b 之前仍被拒绝。LLM 页面调用而 akit 的角色尚缺的方法
  （`/mesh/dial`、`listen`、`limits`、`tokens`……，proxy 的 `/proxy/*`）在 I109d 之前应答 404（决定第 47 行）。
- 测试：`pages::tests`（akit-llm）：在测试自己的路由器上移植的 fit `TestRPC`、`TestStatusOf`、`TestStream`、`TestStreamLimit`、
  `TestStreamSlowBrowser` 以及 `TestSSEEnds` 的空结果，一个停滞超过超时的浏览器；`tests/pages.rs`：node 的页面（其身份、保存进
  成员的模型、一次拒绝、在任何帧之前被拒绝的流、随角色停止的控制台）以及 hub 的页面经 `/hub/node/call` 到达其 node。fit 的
  `TestRPCBusy` 没有对应：akit 的方法不等待页面自己的池。
- 等待审阅的决定：第 47 行。

### 2026-11-27：I109，角色的控制台和 hub 的页面；拆出 I109b 和 I109c

- 角色的控制台（`akit_admin::console::RoleConsoles`，fit 的 `app/roleconsole`）：角色的 `console` 由控制台服务器（I90）提供，
  以角色的模块代替 Admin API：fit 的工作台页面，在控制台的登录、CSRF token、连接规则和访问控制之后，模块位于 `/api/` 和
  `/api/m/<role>/`，cookie 为 `akit_<role>_<port>`，请求体至多 1 MiB。每个控制台随角色的成员移动（角色管理器在角色 apply 之后
  通过 `akit_roles::pages::Consoles` 调用）：设置变化时重新启动，其访问控制（控制台自己的，然后是全局的）由每次应用的配置
  设置；启动失败时保留之前的控制台继续运行，并作为角色的警告。`admin.console` 将运行中角色的模块与自己的模块一起列出
  （`/api/workbench`），并在 `/api/m/<role>/` 提供。
- 角色的模块（`akit_roles::pages`，fit 的 `console.Module` 和 `tunnel/internal/common`）：`Role::module`，按 fit 处理器解码的方式
  读取请求的 JSON，`{"error"}` 应答，角色成员的编辑（`pages::edit`，fit 的 `common.Edit`：应用后应答 `{"ok", "warnings"}`，
  修改或配置不能加载时 400，所指对象不存在时 404，控制线程繁忙时 503），以及链路 token 的端点（`pages::tokens`，fit 的
  `TokenRoutes`），其值辅助函数从 akit-admin 移到 `akit_roles::linktoken`。
- hub 的页面（`akit_tunnel::hub::pages`，fit 的 `tunnel/hub/api.go`）：其状态、token，添加、修改（保留证书文件）和删除监听器，
  设置（设置或清除密码，移动或删除控制台套接字），刷新 node 或为其设置服务（node 拒绝时 502），踢掉 peer，关闭隧道；每个
  修改都记录其用户。
- `akit run` 提供 `tunnel.hub.console`，因而也提供承载 hub 页面的 `console_socket`（决定第 35 行的结果）。在 I109b 之前拒绝
  `tunnel.node.console` 和 `tunnel.proxy.console`，在 I109c 之前拒绝此前被读取后忽略的 LLM 角色的 `console`（决定第 46 行）。
- 测试：`tests/pages.rs`（akit-tunnel）：fit 的 `TestModule`（roleconsole）、`TestPages`，以及针对 akit 自己的 node 和 proxy 的
  `TestPagesPeers`，token 端点，随成员移动的控制台；针对 fit 的二进制：akit hub 的页面经其控制台套接字和 fit 的 web 代理
  （工作台、登录、状态）。`role_pages_under_akit_run`（akit）：`akit run` 下 hub 的控制台和 `admin.console` 中的模块，在 hub
  页面上添加的监听器进入配置的历史。`consoles_move_with_their_roles`（akit-roles）。
- 等待审阅的决定：第 45 和 46 行。

### 2026-11-27：A247–A249 的临时做法删除

- LLM node 使用 akio 的 A247–A249（决定第 40 行的结果）：`akit_llm::exec::is_root` 调用 akio-sys 的 `process::euid`
  （在 node 启动时读取 `/proc/self` 或运行 `id -u` 的 `init_euid` 已删除）；`Killer::kill` 用 `process::kill_group` 从任意线程
  立即向作业的进程组发送 SIGKILL，在保证组长尚未被回收的锁内进行（在池上运行的 `kill -KILL -- -<pgid>` 已删除，`Killer` 和
  `Jobs` 不再需要池）；`exec::output` 单独运行的命令（硬件和 GPU 的查询）也在自己的进程组中启动；Linux 的磁盘在 node 的池上
  用 `fs::space` 读取，同 fit 的 `Statfs`（Linux 上不再运行 `df`；macOS 保留 `df -k -P -l`，同 fit）。`hw::collect`、
  `gpu::collect` 和 `exec::output` 不再接收池参数：它们会阻塞，在 node 的池上运行。
- 测试：`akit-llm` 的测试未改动，在 macOS 上通过。

### 2026-11-27：akio 锁定在 `a8171fda`：A247–A249

- akio：akio main 上的 `a8171fda`，包含 A247（`akio_sys::process::euid`）、A248（`process::kill_group`，即 killpg，以及
  `process::SIGKILL`）和 A249（`fs::space`，文件系统的大小与可用空间）。akit 的 `rev` 与 `akit_runtime::AKIO_REV` 随之移动；
  akit 无需改动即可构建。

### 2026-11-27：LLM mesh 监听器的证书重新读取；一个会话测试中的竞争

- 决定 44 留给 LLM 轨道的部分：mesh 监听器的 `cert_file` 和 `key_file` 在每次应用配置时重新读取（角色的文件变化后
  `akit run -watch-certs` 会重新应用配置），变化的证书此后经 akio-tls 的 `CertStore` 就地提供：监听器及其链路不变，新链路看到新的指纹。
  无法加载的文件（证书已写入、密钥尚未写入）保留正在使用的证书，并记录日志。hub、node 和 proxy 经 `Role::files`（I102d 新增）报告这些
  文件，由 `-watch-certs` 监视。测试：`mesh::tests::cert_files_read_again`、`tests/hub_manage.rs` 的 `listener_files_reported`。
- `wire::session::tests::cancel_idle_and_close` 在 gpu51 上失败（io_uring 13 次中 10 次，epoll 3 次中 1 次，akio `6151f5c5` 与
  `8a9601b9` 上都是）：流的两端在相同的 `stream_idle` 之后让流超时，打开方的超时会取消流（发送 stream-cancel 帧），因此打开方的计时器先触发时
  响应方得到 `Canceled`，与 fit 相同，fit 的 `TestStreamIdleTimeout` 接受两者之一。会话没有问题；测试只期望响应方自己的超时。现在与 fit 的
  测试一样接受任一端的超时，并且也检查打开方（它自己的超时，或经响应方的错误结束帧带来的超时）。

### 2026-11-27：I108，LLM node 的管理：硬件、GPU、固定动作和作业

- 硬件（`akit_llm::hw`，fit 的 `hwinfo_*.go`）：node 启动时及每隔 `refresh_interval` 在 node 的池（2 个运行、16 个等待）上读取，
  无需 root，macOS 和 Linux 皆可：Linux 上读 procfs（内核、发行版、CPU、负载、运行时间、内存），macOS 上用 `sysctl`、`sw_vers`、
  `vm_stat` 和 `system_profiler`（只读一次），`nvidia-smi` 的 GPU，磁盘（来自 `df -k -P`，akio 的 A249 之前的临时做法；Linux 上取
  `/proc/mounts` 中 fit 所列类型的文件系统），地址来自 akio-sys 的网络接口；读不到的部分略去。node 的信息携带它，变化时（包括硬件）
  推送，与 fit 相同。
- GPU（`akit_llm::gpu`，fit 的 `llm/internal/gpu`）：七个厂商的目录（工具、作为常量的安装脚本、作为 argv 模板的设置；脚本的变量为
  `AKIT_LLM_GPU_DATA_DIR`，决定 038），Linux 上读 PCI 总线找出厂商，查询并解析每个已安装的工具（nvidia-smi 的 CSV、rocm-smi 的 JSON、
  xpu-smi、system_profiler），或保留原始输出，状态缓存 5 秒、在池上一次只读一个；设置的值经检查（有界整数、选项）后放入单个参数，
  配置且非 root 时加 `sudo -n`。
- 命令和作业（`akit_llm::exec`、`akit_llm::jobs`、`akit_llm::actions`）：fit 的十一个固定动作，参数按整体匹配的模式检查（akio-regex），
  从不经 shell，只有常量脚本例外；命令在自己的进程组中运行，其输出（两个流按顺序，经 socket pair）存入保留最后 1 MiB、偏移为绝对值的
  缓冲；作业在自己的池上运行，最多 8 个（再多被拒绝，限流），保留 200 个，`job_timeout` 和取消会杀掉进程组（组长立即，组内其余由池上的
  `kill(1)`，A248 之前的临时做法；是否 root 来自 A247 的临时做法）；`/node/job/follow` 在输出写入时发送新的输出，至少间隔 100 毫秒，
  没有输出时每 30 秒发送状态，受流的信用约束。
- 方法（`node::manage`）：模型列表（key 打码）、保存（空的或打码的 key 保留原有的；最多 256 个）、删除（同名多 API 时需给出 API）和
  检查，名称，GPU 的状态、目录、安装和设置（index 为 -1 时作用于该厂商的每块 GPU），动作和作业，供 node 的页面以及设置了
  `allow_hub_manage` 时的 hub 调用（每次调用时检查）；设置和上游上限仅供其页面，写入成员时略去默认值。
- 测试：fit 的动作、命令、输出缓冲、作业、GPU 和目录测试（`node_test.go`、`gpu_test.go`、`fake_test.go`），使用替身工具，从不运行真实
  工具（`exec::tools_in`，如同 fit 的测试设置 `PATH`）；`tests/node_manage.rs`：fit 的 `manage_test.go` 和 node 的 GPU 方法（访问控制、
  经方法运行的作业、名称、设置、上游上限、模型编辑及上限、检查、GPU 状态、设置和安装、硬件重新读取）；akit 的 hub 管理 akit 的 node。
  对 fit 的二进制（`tests/fit_manage.rs`）：fit 的 hub 经其控制台（`/api/rpc/hub/node/call`）管理 akit 的 node（模型、一个作业、目录、
  保存一个模型到 akit 的成员）。

### 2026-11-27：I106b，hub 供页面使用的调用、node 列表的上限、路由计数

- 每个角色的方法经 `akit_llm::local::Local`（`Role::api`，即 fit 以调用方 `local` 进行的 `Router.Invoke` 和 `OpenLocal`）到达
  其自己的页面：一个内存中的会话（`wire::pipe`），因此页面的调用或流与经 mesh 的行为一致，走同样的方法。修改配置的方法通过
  `Base::edit`（fit 的 `Base.Edit`）进行：控制线程把成员的修改作为来自角色页面的配置应用，应用之后才应答；修改本身的错误保留其错误码，
  被拒绝的配置是 bad request（决定 38）。
- 路由计数（`mesh::routes`，fit 的 `mesh.Routes`）：角色按路由（from、to、model；单独统计 1,024 条，其余合并为 `other`）转发的调用，
  字节随传输从流中读取；由 node（调用方到上游，上游的错误状态计为失败）、hub（proxy 到 node）和 proxy（其 agent 到 hub 或 node，
  每次尝试）计数。每个角色的 `/mesh/status`（仅自己的页面）：其身份、链路和路由（fit 的 `MeshStatus`）。
- hub：`/hub/nodes`（fit 的 `HubNode`，在线在前）、`/hub/node/call`（node 的非流管理方法，90 秒内转发；离线、禁止和其他错误按 fit
  的 `manageErr` 的措辞）和 `/hub/node/stream`（在两条流的信用下逐帧转发 `/node/job/follow`，页面一端结束即取消 node 的流）、
  `/hub/settings/get` 和 `set`（`max_nodes`，默认值不写入成员）、`/hub/profile/set`。node 列表与 fit 相同：每个 node 的硬件不含磁盘和地址、
  16 块 GPU、64 个模型、其链路及来自路由的流量；编码后不超过 4 MiB（省略末尾的 node）；只在变化时序号递增；变化后 200 毫秒、且距上次至少
  一秒推送，有 proxy 连接时每 5 秒推送一次。
- `akit_llm::hw::HwInfo`：fit 的 `HWInfo`，按 node 信息携带的格式读写。
- 测试：路由（fit 的 `routes_test.go`），node 列表的摘要和上限（`nodelist_test.go`）；`tests/hub_manage.rs`：一个替身 node，其调用和
  流被转发、拒绝、离线，对端调用页面的方法被拒绝；列表的序号；设置和名称写回，编辑失败时的应答（fit 的 `TestRole`）；`tests/e2e.rs`：
  端到端调用之后每个角色的路由。对 fit 的二进制（`tests/fit_manage.rs`）：akit 的 hub 管理 fit 的 node（其模型列表、运行并跟随一个作业、
  保存一个模型）。

### 2026-11-27：I102d，hub 的 QUIC 握手限制和监听器证书的重新读取

- hub 的 `quic` 监听器基于 akio 的 A245（ADR-185），同 fit：每次握手受 `tunnel.hub.quic.handshake_timeout`（省略时 10 秒）限制，
  握手未按时完成的连接被丢弃；进行中的握手至多 `max_handshakes` 个（省略时 64），过半后要求新客户端以 Retry 证明其地址，占满后
  其 Initial 被丢弃，直到某个握手结束（计入 akio 的 `refused_connections`）。握手完成后 `handshake_timeout` 内未打开链路流的连接
  被关闭（fit 的 `streamWait`）：计时器用 akio 的，检查由处理器通过 akio-quic 的 `Connection::notify` 完成。
- `tls`、`wss` 或 `quic` 监听器的证书在每次应用 hub 的成员时读取：其 `cert_file` 和 `key_file`，或 hub 自己的证书（`quic.cert`，
  或保存在配置旁的文件）。保留的监听器若证书有变，通过 akio-tls 的 `CertStore`（TCP 和 QUIC 都一样）就地为之后的握手提供新证书：
  监听器不重新打开，其上的链路继续；无法加载的文件保留正在使用的证书，角色的警告说明原因。读取的文件及其 SHA-256（无法读取的为
  全零）经新方法 `akit_roles::host::Role::files`（默认为空）报告，`akit run` 与监听器自己的证书文件一起监视它们（`-watch-certs`）：
  文件变化并稳定后，同监听器的证书一样再次应用配置。hub 的状态显示每个监听器自己的证书指纹（fit 的 `ListenerView`）。没有地方保存
  证书的 hub 每次运行只生成一次。
- 未在此完成：LLM mesh 监听器证书的重新读取（待审决定第 44 行），因为另一改动同时在编辑 akit-llm 的角色。`Role::files` 即其报告途径。
- 测试：在 hub 的服务器上移植 fit 的 `TestServerIdleConn`、`TestListenerRetry` 以及 akio 的中继测试（`tests/quic_bounds.rs`：
  停滞的握手在 300 毫秒超时时被丢弃，其后没有流的连接被关闭，`max_handshakes` 为 2：过半时 Retry、等到上限解除，默认值同 fit）；
  fit 的 `TestListenerCerts`（`a_listeners_certificate_is_read_again`：`tls`、`quic` 和 `wss` 监听器的指纹，记下的文件，续期后
  经 TLS 和 QUIC 的新链路得到新证书、旧链路继续、监听器地址不变，密钥不在时保留正在使用的证书）；端到端，`akit run` 无需重启即
  提供 hub 监听器续期的证书（`a_hub_listeners_certificate_renewed_without_a_restart`，`crates/akit/tests/watch_certs.rs`）。对 fit 的
  二进制（`AKIT_FIT_BIN`，`with_fits_node_and_proxy_over_quic`）：fit 的 node 和 proxy 经 QUIC 连接设置了限制的 akit hub，回显
  8 MiB，并固定续期后的证书。
- 待审决定：第 42 至 44 行。
- 无需测量：握手限制和证书都不在热路径上（规则 24）。

### 2026-11-27：akit 使用 akio 的 A238：立即结束 WebSocket

- 基于 akio 的 `WsSender::abort`（A238，ADR-184），同 fit 的 `Abort`：`akit_tunnel::ws::WsWire::close`（失败的链路、踢掉、
  吊销的令牌、停止的角色）和 hub 拒绝的链路的 WebSocket 立即结束，丢弃已排队的数据，不发关闭帧；`close_after_send` 仍在已排队
  数据之后发送关闭帧。hub 的控制台 socket 对拒绝的 WebSocket（超过 `max_conns` 或同一地址超过四分之一）、未按时证明密码的、
  消息解不开的，以及 socket 停止时的所有 WebSocket 直接中止；proxy 的 web 代理在 hub 的 hello 或某条消息解不开、或本地连接已
  不在时中止其 WebSocket。TCP 一侧已关闭的 WebSocket 仍收到关闭帧（1000），同 fit。LLM mesh 拨号方的强制关闭（对端失联或
  太慢）也直接中止，同 fit 的 `forceClose`，它所需的 1 秒关闭超时已去掉。
- 测试：`ws_links_that_fail_end_at_once`（短于帧头的链路帧：hub 的 WebSocket 立即结束、没有关闭帧；用临时方案的关闭帧时
  失败）；控制台 socket 的 `aborts`（fit 的 `TestSocketAborts`）检查两个 WebSocket 都以 1006 结束。
- 待审决定第 20、26、36 行写上了结果。

### 2026-11-27：akio 锁定在 `8a9601b9`：A238 与 A245

- akio：akio main 上的 `8a9601b9`，包含 A238（`WsSender::abort`：立即结束 WebSocket，不发关闭帧，丢弃已排队的数据，
  ADR-184）和 A245（akio-quic 服务器的握手上限 `ServerConfig::handshake_timeout` 与 `max_handshakes`，过半后要求 Retry，
  ADR-185）。akit 的 `rev` 与 `akit_runtime::AKIO_REV` 随之移动；akit 无需改动即可构建。A238 的临时方案保留到替换它们的改动，
  A245 由 I102d 使用。

### 2026-11-27：I93b 和 I93c，带加密 store 的 `akit run`，以及 `akit run -admin`

- **配置保存在 store 中**（I93b），同 fit 的 `cli/run.go`、`cli/store.go` 和 `admin/bootstrap.go`（`akit_admin::bootstrap`、
  `crates/akit/src/boot.rs`）：`admin.store` 指定的 store 是 akit 运行所依据的基础配置，每次启动时把其内容合并到配置文件之上（对象
  逐成员合并，其他值和列表整体替换，以 store 为准），合并改变了它时写回，不存在时由配置文件播种；引导文件的 `admin` 被拆出，从不保存、
  显示，也不从发来的配置中取（设置它的配置被拒绝，400），而是在每个配置加载时作为最后一个成员放回，使每个错误仍在原位置。
  `Configs::use_store`：每个应用的基础配置（Admin API、控制台、回滚、SIGHUP、角色对自身的编辑）都在控制线程上写入 store（密封、
  同步、重命名），除非 store 已保存该配置；写不进去的仍然运行，并说明未保存。SIGHUP 重新读取 store（`akit store import` 给了它新的
  salt 时重新派生密钥）；有 store 时拒绝 `-watch`。启动时由配置文件写入的 store，若之后启动失败，则恢复原样（或删除），使失败的
  首次启动不会留下一个在下次启动时以其值为准的 store；打不开的 store（密码错误、改动了一个字节）使 akit 停止。没有 store 时，角色
  把生成的东西保存在配置旁边；有 store 时保存在配置中。
- **控制台的密码**（I93b），同 fit 的 `admin/console.go`：`POST /api/m/admin/account/password`（当前密码按登录的方式检查，一个地址
  同时只检查一个，失败计入锁定）和 `/users/password`（另一个用户的），新的 bcrypt 哈希在控制台的登录池上生成并写入 store、配置旁边，
  该用户的其他会话结束，记录到 `admin` 日志器；没有 store 时 409，写入失败时 500（什么都不变）；重启时 store 中的哈希替换引导文件
  中的。`GET /api/m/admin/users` 回答 `"store": true`。
- **`POST /config/export` 和 `/config/import` 的 `store` 格式**（I93b），同 fit 的 `admin/transfer.go`：用请求的密码或正在运行的
  store 的密钥密封的 `fitstore` 文件（整个配置的导出带上修改过密码的控制台用户），以及用 `Akit-Store-Password` 或 store 的密码
  打开的文件，作为配置或其 `?members=` 应用，保留正在运行的 `admin`。密钥在 API 的线程池上派生，全进程一次一个（其余回答 503 并带
  `Retry-After`），要求超过 akit 写入代价两倍的文件事先被拒绝（`store::unseal`）；store 的密钥和密码以 `Sealer` 的形式到达线程池，
  与 store 分开。
- **`akit run -admin`**（I93c），同 fit：配置省略时补上 `admin` 和 `admin.console` 及其默认值，配置保存在 store 中：`admin.store`
  指定的、经 `-c` 给出的 `.store` 文件（两个则拒绝），或配置文件旁边的（`site.json` 对应 `site.store`，目录对应 `<目录>.store`，
  没有文件时为 `akit.store`），存在后或由 `-admin` 创建后使用，其密码为 `$AKIT_STORE_PASSWORD` 或旁边的密钥文件（`site.store.key`，
  52 个 base32 字符，在它和 store 都不存在时以 `O_EXCL` 创建，0600，从不打印）。没有控制台用户时，用户 `admin` 带临时密码
  `admin`，须在 `-timeout`（30m；`-timeout` 需要 `-admin` 且大于 0）内修改：用它的会话得到 `must_change_password` 和
  `password_expires`，只能读取自己是谁并修改密码（其他回答 403），其间 Admin API 拒绝所有请求；超时后该密码无法再登录（401，
  `password_expired`）。`akit store export` 和 `import` 与 `run` 一样找到 store。
- **移植的 fit 测试**：`cli/store_test.go`（`TestServeStore`、`TestStoreCommand`、`TestServeStoreMergeUnwritable`、
  `TestServeStoreFailedFirstStart`）和 `cli/adminflag_test.go`（`TestServeAdminFlag`）在 `crates/akit/tests/store.rs` 中；
  `admin/accounts_test.go`、`accounts_edge_test.go`、`transfer_test.go` 和 `transfer_edge_test.go` 中与 store 有关的用例
  （`TestConsolePasswordChange`、`Reset`、`NoStore`、`StoreFails`、`Lockout`、`ResetLockedOut`、`Bodies`、`ResetStoreFails`、
  `TestTemporaryPassword`、`TestConsoleStartsTemporary`、`TestExportImport`、`TestImportStoreBounded`、
  `TestSetConsoleUsersWithoutStore`）在 akit-admin 的 `tests/console.rs` 中；`TestMergeConfig`、`TestWithDefaultConsole` 和
  `TestWithConsoleUsers` 作为 `bootstrap` 的单元测试。不同之处见决定 33。
- **与 fit 互通**：`tests/fitstore` 增加一个由 fit 自己的代码重新打开、写回配置并修改密码的 store（`writeback.store`，
  `akit_vectors_test.go`），akit 能打开；akit 这样写的 store，以及 akit 写回的 fit 的 store，fit 能打开（`TestAkitWrote`）。
  `AKIT_FIT_BIN` 指定 fit 的二进制（在门禁锁定的提交上 `go build ./cmd/fit`）时，fit 和 akit 以 `run -admin` 轮流从同一个 store
  及其密钥文件运行，各自提供对方在控制台中修改的配置，并用对方修改的密码登录（`fit_and_akit_share_a_store`），并各自导出对方的 store。
- **决定** 30 到 33（[待审阅](decisions-pending.zh-CN.md)）：fit 的名字改名（`AKIT_STORE_PASSWORD`、`Akit-Store-Password`、
  `akit.store`），密钥文件同 fit；导出的密钥派生与导入的一起受限；用户和配置的写入用两把锁；测试的不同之处。
- 测试：macOS（kqueue）上 `make check` 通过，只有文档测试因其他工作树在共享 target 目录中构建同名 crate 而失败（E0460/E0463），
  在单独的 target 目录中单独运行时通过；gpu51 上 clippy `-D warnings` 以及 release 模式的整个工作区在 epoll 和
  `AKIO_TEST_BACKEND=uring` 下都通过（epoll 那次运行早于两个按 `store` 格式的消息更新的 akit-admin 测试，io_uring 那次包含了它们）。不做基准（控制面，
  规则 30）；基准表中控制台的一行保持不变。

### 2026-11-27：I103b，控制台套接字和 web 代理

- `akit_tunnel::wscrypt`：逐字节等同 fit 的 `tunnel/internal/wscrypt`：密码的 PBKDF2（`fit-tunnel-ws`，10,000 轮，SHA-256），
  双方各 16 个随机字节，每个方向一个由 HKDF-SHA256 在两者之下派生的密钥，AES-256-GCM，以消息序号为 nonce；不能作为下一条打开的
  消息结束会话。测试核对一组由 fit 自己的 `wscrypt.go` 密封的向量。
- hub 的 `console_socket`（`akit_tunnel::hub::socket`，fit 的 `tunnel/hub/socket.go`）：一个 akio-http 升级的监听器，任意路径，
  握手限制同 `ws` 链路，hub 的访问策略在 akio 的过滤器上。每个 WebSocket 先收到 hub 的 hello，再收 proxy 的；其第一条能打开的
  消息证明密码；同时至多 `max_conns` 个，其中来自同一地址的未证明者至多四分之一，未证明者 10 秒后关闭。其第一批字节触发拨向 hub 的
  `console.address`（未指定的主机即回环地址），5 秒内；期间到达的数据等待（至多 1 MiB），WebSocket 的读取被暂停。控制台的字节
  密封发送，每条消息 64 KiB。socket 的密码是它自己的或 hub 的；新地址重新监听并关闭已有 WebSocket，新密码或新上限作用于此后打开的
  WebSocket；hub 的状态加入 fit 的 `socket`。
- proxy 的 `web_proxies`（`akit_tunnel::proxy::web`，fit 的 `tunnel/proxy/webproxy.go`）：每个一个本地监听器，由 proxy 的访问策略
  过滤；每条连接（读取先暂停）拥有自己的一个到 hub 套接字的 WebSocket（akio-ws 的客户端，`User-Agent: akit`，拨号和握手 10 秒内）；
  收到 hub 的 hello 后，发出 proxy 的 hello 和一条空的密封消息（证明），然后是浏览器已发的数据。同时至多 `max_conns` 条；按 `listen`
  在多次应用间保留的 web 代理，对此后接受的连接采用新的密码、URL 和上限；被移除的关闭其连接。`ProxyApi::web_proxies`（proxy 的
  `Role::api`，fit 的 `WebProxies`）。
- 背压（规则 8 和 9）：两个中继都按另一侧的队列调节每一侧（`akit_tunnel::pace::Gate`，fit 的 `common.Gate`，水位线是 fit 的
  256 KiB `Watermark`），基于 akio 的 `hold_reads` 和 `on_drain` 以及 akio-ws 的 A236（`hold_reads`、`on_drain`、`set_relayed`）；
  两侧都标为中继。**临时方案（akio 请求 A238）**：被拒绝或失败的 WebSocket 在已排队数据之后以关闭帧关闭，而 fit 直接中止（决定第
  20 行）。
- `akit run` 在 I109 提供角色页面之前拒绝 tunnel 各角色的 `console`（此前读取后忽略）；需要 `console` 的 hub 的 `console_socket`
  随之被拒绝。proxy 的 `web_proxies` 可运行。待审阅的决定：第 35–37 行。
- 测试：移植了 fit 的 `socket_test.go`（`hub::socket::tests`：控制台连上之前分多条消息发送的请求、超出 `max_conns` 的 WebSocket、
  错误的 hello、没有控制台；未证明的 WebSocket 超时关闭且按地址受限、证明被保留且不转发；不能打开的消息、socket 被移除；hub 的密码、
  新密码、新地址）和 `webproxy_test.go`（`proxy::web::tests`：经代理取页面、错误密码、`max_conns`、代理被移除、无处可连的 socket、
  错误 URL；对照测试自己的 WebSocket 服务端逐步检查会话：证明在先、早到的字节按序、不能打开的消息、会话建立前已离开的本地连接、
  错误的 hello）。端到端（`tests/console.rs`）：双向回显 24 MiB 以及八条同时进行，停止读取的读者压住控制台（macOS 上 96 MiB 中约写出
  3 MiB），任一侧切断连接。对照 fit 的二进制（`b6c5cebf`，`AKIT_FIT_BIN`，CI 中跳过）：经 akit 的 web 代理取 fit 的 hub 的控制台
  （页面和 `workbench.js`），以及经 fit 的 web 代理和 akit 的 hub 套接字回显 8 MiB。
- 下一步：I102d；I109 提供套接字承载的页面。

### 2026-11-27：I105–I107，LLM 的 node、hub 和 proxy；拆分出 I106b

- 角色（`akit_llm::{node,hub,proxy}`）已注册到 `akit run`（`akit::ROLES`）：它们共有的部分是 `akit_llm::role::Base`：成员的 `id`
  （首次启动时生成，`<role>-<16 位十六进制>`，写回成员）和 `name`；访问策略（成员自己的，然后是全局的）用作监听器的过滤器；`limits`；
  `listen` 及其 `self_signed` 证书，生成一次并保存在配置旁（`akit.llm.<role>.pem`，0600；配置旁无处保存时写入成员）；`peers`；`tokens`，
  token 的第一个成员写回其记录，过期 token 的链路由每次应用配置时重新设置的清扫关闭。角色 hello 时的版本是 `akit <version>`。
- node（I105）：其模型的状态（`ready`、`disabled`、`error`），每 60 秒（出错时 15 秒）用 akio 客户端探测上游的模型列表来检查（401、403
  和 5xx 视为失败，丢弃过时的探测）；其信息（`/node/info`）以及向其 hub 和 proxy 的推送（`/node/update`，间隔 200 毫秒，只在变化时，带递增
  的序号；对新连接的对端立即推送）；`/node/proxy`：在模型的 `max_inflight` 和 node 的上限内占用模型，各 API 的上游 URL 和认证（OpenAI；
  Anthropic 及其版本；Gemini 及其 `alt`；Azure OpenAI 的部署；Ollama；Cohere），在 JSON 正文、表单字段或 Gemini 批量请求中把模型改名为上游
  的名字，请求正文内联或从流的帧流式上传（上游等待超过 256 KiB 时按窗口压住，借助请求正文的 `on_drain`），响应头和正文到达即在流的信用下
  发出（否则暂停上游的读取），`first_byte_timeout`（同时约束连接：akio-http 的客户端没有自己的连接超时）、`idle_timeout`、`timeout`，
  首字节之前保活，上游连不上时把模型置为错误。其信息中的硬件在 I108 之前只有最少的部分（主机、系统、CPU）。
- hub（I106）：node 表（`max_nodes`，遗忘最久之前见到的离线 node；同一次运行中序号更旧的 node 信息丢弃），其在线 node 的模型在变化时推送给
  proxy（间隔 200 毫秒），`/hub/models`，node 列表（`/hub/node/list` 及其推送，每个 node 64 个模型），以及 `/hub/proxy`：选择提供该模型且
  就绪、负载最低的 node，相同时轮询，由两个 pump 在各流的信用下转发调用（proxy 流式上传的正文到 node，node 的帧到 proxy），在没有任何数据
  到达 proxy 且其正文未被取走时换下一个 node（最多 3 个）。
- proxy（I107）：`api.address` 上供 agent 使用的 API，监听器引擎上独立的 akio-http 监听器（配 `cert_file` 时用 TLS，超过 `max_conns` 的
  连接在 accept 时关闭，角色的访问策略，`max_body_bytes`）；OpenAI 和 Anthropic（`/v1/models`）、Gemini（`/v1beta/models`）和 Ollama
  （`/api/tags`）的模型列表；五种 API 的调用，key 取自 `Authorization: Bearer`、`x-api-key`、`x-goog-api-key` 或 Gemini 的 `key`，其
  SHA-256 与每个 key 以常数时间比较；直连 node 和 hub 的模型目录（推送而来，并在对端连接时读取）；路由：先是就绪的直连 node（负载低者优先），
  再是 hub，响应头之前最多 4 条；超过 1 MiB 的正文以 64 KiB 的帧流式发送；响应带 `X-Akit-Node` 流式写给 agent，agent 跟不上时压住流的帧；
  `max_inflight`（429）、`first_byte_timeout`（503）和 `timeout`（504）；各 API 的错误格式和状态码。
- 测试：表单与 JSON 改写、上游 URL、认证头、端点和错误格式的单元测试；端到端（`tests/e2e.rs`）：agent 经 akit 的 proxy、hub 和 node 到
  模拟的 OpenAI 兼容上游（模型改名、上游的 key、流式事件、经 hub 流式传输的 3 MiB 正文、错误的 key、未知模型、另一种 API 的模型、错误的
  JSON、未知路径），直连 node 优先，上游已死时回答 503，过期 token 的链路被关闭。对 fit 的二进制（`tests/fit_roles.rs`，`AKIT_FIT_BIN`）：
  fit 的 hub 位于 akit 的 node 和 proxy 之间，akit 的 hub 位于 fit 的 node 和 proxy 之间，以及 akit 的 proxy 直连 fit 的 node，各自做同样的调用。
- mesh：对端上线回调在 hello 的应答发出之后才运行，使其发送的内容（hub 的 `/node/info`、proxy 的模型）到达已登记该链路的对端。
- 剩余：I106b（hub 供页面使用的调用、node 列表的字节上限、路由计数）、I108（node 的管理、其硬件）、I109（角色的页面）。

### 2026-11-27：I104，LLM mesh：基于 akio-ws 的 wire、链路和 mesh

- `akit-llm`（新）：fit 的 `llm/internal/wire` 和 `mesh`，线上逐字节一致。帧（`wire::frame`）：fit 的头，调用、打开流和推送带方法名，
  1 KiB 起在更小时用 gzip（akio-deflate），编码后或解压后最大 16 MiB。会话（`wire::session`）：奇偶流 ID，带超时（并通知对端取消）的调用，
  推送，带往返时间计量的 ping，排空，`max_inbound`（超出即限流），每个方向 256 KiB 窗口、消费一半即归还信用的流，空闲超时（定时器按剩余
  时间重新设置），保活，`Pump`。fit 的 goroutine 在这里是回调：调用的处理函数在就绪时通过 `Reply` 应答（未应答即丢弃时回内部错误）；流的
  读者在 `on_data` 中拿到帧并用 `release` 归还；没有信用的发送方（`try_send`）由 `on_credit` 告知何时重试。推送在链路的线程上按到达顺序
  处理，因此没有排队的（fit 的 1,024 个队列无需保存任何东西），出站链路的 hello 应答在读下一帧之前登记对端（fit 在 `caller` 中的两秒等待
  无需等待）。
- 链路（`wire::conn`）：监听器引擎上的 akio-ws。监听器是一个独立的 akio 监听器，只在 `/ws` 上提供 akio-http 的升级（其他路径 404），
  经 TLS（TLS 1.3，无 ticket）或在回环上明文；升级之后是 fit 的握手（32 字节挑战、token 的 `FTK1` 应答或密码的 HMAC 加 nonce、带监听方
  证明的 `0x01` 确认、4001 和 4003 关闭）；最多 256 条未认证链路、单个地址 16 条，从 TCP accept 起计（akio 的 `on_accept`/`on_close`），
  10 秒内未认证即关闭；`max_conns`。每条链路按 fit 的 `Conn.send` 节流：排队超过 64 KiB 时暂停其流，并暂停读取至多 ping 间隔与慢超时中
  较短者的四分之一，直到 akio-ws 的 `on_drain`（A236）；链路设为中继（`set_relayed`），使引擎只在链路自己的循环上发送时才检查的暂停不会压住
  由其他链路喂数据的链路。心跳：空闲一个间隔后、发送过帧后、或往返时间已过两个间隔时发 ping；三个间隔无帧或 ping 等满慢超时即断开链路。
  拨号方固定指纹的 `wss://` 链路使用 akio-tls 的 `pin_sha256`（A235），指纹在握手中检查。
- mesh（`mesh`）：基于定时器和回调的 fit `Mesh`：监听器（`set_listen`，回环之外用 TLS）、按全抖动退避保持连接的拨号目标（hello 被拒绝后
  更久）、双向 hello 及 fit 的检查（接受的角色、非自身角色、非自身、以另一角色持有的 ID、密码对应的角色、token 绑定第一个用它 hello 的 ID、
  目标须具有的角色）、按对端 ID 的链路池、在未排空的链路间轮询、`call`、`open_stream`、`push`、`peers`、`targets`、`kick`、`configure`
  （未变化的部分及其链路保持不动）、`shutdown` 等待进行中的调用和流（由链路的会话通知的条件变量，不轮询）。`Router` 带每个方法的角色和超时
  （`Answer`），页面的 `local` 角色始终接受。角色 hello 时的版本是 `akit <version>`；对端只显示它。
- 测试（36 个）：fit 的帧、会话、流控、边界和异步测试（信用、慢读者只压住自己的流、取消到达处理函数、空闲、保活、关闭、推送顺序、ping、
  `max_inbound`、排空、暂停的流、对端的协议错误、超出窗口、ID 回绕、pump 把写方压到慢读者的速度）；链路（双向密码、token、错误密码、
  `max_conns`、accept 钩子、404、单地址的未认证链路、慢超时断开沉默的对端、未完成的握手以 4001 关闭）；mesh（一条链路上的双向调用、角色、
  方法超时、流、按序推送、被踢掉的链路重新拨号、拒绝角色、密码和以另一角色声明的 ID、token 绑定其成员及吊销的 token 关闭其链路、两条链路的
  池和排空、不 hello 的链路被关闭、带指纹和错误指纹的 TLS、`configure`）。对 fit 的二进制（`tests/fit_mesh.rs`，用 `AKIT_FIT_BIN` 运行）：
  akit 的 node 用共享密码、用 fit 签发的 token（写成 `akitt1.`）以及经固定指纹的 TLS 连接 fit 的 hub，错误的指纹和错误的密码都连不上；
  fit 的 node 用角色密码、用 akit 签发的 token（写成 `fitt1.`）以及经固定指纹的 TLS 连接 akit 的 hub，推送其信息并应答 `/node/info`。
- 临时方案：拨号方用 close 帧断开已死或过慢的链路，该帧排在等待的数据之后，akio-ws 的客户端在其关闭超时（这里 1 秒）后关闭 TCP 连接；
  监听方通过连接句柄立即中止。akio API 请求 A238 请求 `WsSender::abort`。自 akio `8a9601b9` 起已删除：拨号方直接中止，1 秒的关闭超时去掉。
- 下一步：其上的角色，I105（node）、I106（hub）、I107（proxy）。

### 2026-11-27：I103，hub 的调用和状态、token 过期；拆分出 I103b

- hub 的调用（fit 的 `tunnel/hub/api.go` 及其调用的 `hub.go` 中的方法），在 `akit_tunnel::hub::HubApi` 上，这是 hub 通过 `Role::api`
  交给其页面的句柄（akit-roles 的 `Role` 新增的方法，默认 `None`，以及 `Manager::api`）：`status`，即 fit 的 `GET /api/status` 返回
  的 JSON（名字、版本、监听器及对端拨号用的 URL 和证书指纹、设置、对端及其链路的位置、token、往返时间和隧道数、隧道及其服务、node
  连到的地址、打开时间和双向传递的字节数、`password_peers`）；`refresh_node`（在 node 位置最低的链路上调用 `node/info`，采纳其应答
  并通知 proxy）、`set_services`（`node/setServices`，node 仅在 `allow_hub_services` 时接受并写回自己的配置成员）、`kick`（同 fit，
  以 JSON 字符串形式的原因发送通知，发出后关闭每条链路，包括仍在注册的链路）、`close_tunnel`（通知两端）。被踢掉的 node 或 proxy
  在其 uplink 的状态中说明（`Uplink::kicked`），并在 `retry_interval` 后重新连接，同 fit。
- token 过期：hub 对过期 token 的清理在配置中的 token 每次变化时重新安排（新的一代取代之前安排的清理），使 token 的链路在其过期时
  关闭，同 fit 注册表的定时器；此前第一次清理在 hub 启动一分钟后才进行，与 token 无关。被吊销 token 的链路立即关闭（注册表的
  closer，I101）。
- 拆分：控制台套接字和 web 代理是 I103b。套接字承载的控制台是 hub 的页面，由 I109 提供。待审决定：第 21 行。
- 测试：`the_hubs_calls_and_status`（一个带 node、proxy 和一条隧道的 hub 的状态；重新查询 node 的信息，proxy 被拒绝；通过像
  `akit run` 那样应用 node 编辑的控制线程设置服务；从 hub 关闭隧道使客户端连接结束；踢掉 proxy 后它重新连接）、
  `a_tokens_expiry_closes_its_links`（fit 的 `TestTokenExpires`：node 连上两秒后过期的 token 关闭其链路，且不再连上；对端上列出
  token，没有使用密码的对端）。
- 下一步：I103b 或 I102d。

### 2026-11-27：I102c，WebSocket 链路

- `ws://` 和 `wss://` 链路（`akit_tunnel::ws`，fit 的 `tunnel/internal/transport/ws`），基于 akio 的 A236：hub 的 `ws` 或 `wss`
  监听器在其路径上提供 akio-http 的升级（默认 `/tunnel`；其他路径回 404），只用 HTTP/1.1，握手头部限 16 KiB 和 10 秒，接受任何
  `Origin`；`wss` 监听器出示自己的证书或 hub 的证书，ALPN `http/1.1`，TLS 1.3。node 或 proxy 用 akio-ws 的客户端拨号
  （`User-Agent: akit`），握手在 `dial_timeout` 内完成，`wss` 拨号像 `tls` 拨号一样检查 hub 的证书（通过 `pin_sha256` 的 pin、
  `ca_file` 或系统根证书）。接受至多 2 MiB 的消息；一次发送拆成每条至多 1 MiB 的二进制消息，同一次发送的消息保持在一起；不压缩。
  1000、1001、1005、1006 以外的关闭码作为链路的错误，同 fit。
- `ws::WsWire` 是基于 `WsSender` 的链路 `Wire`，同 `ConnWire` 之于 TCP 连接：WebSocket 的连接设为中继（`set_relayed`），队列上限
  为 `LINK_QUEUE`（`set_max_queued_bytes`），`hold_reads`、`queued`、`on_drain` 和 `peer` 映射到 sender 上。hub 按与 TCP 链路相同的
  限制接收 WebSocket 的链路（`accept_wire`），访问策略在 TCP 监听器上的 akio 过滤器处执行。
- **临时做法（akio 请求 A238）**：akio-ws 没有不做握手的关闭，所以 `WsWire::close`（失败的链路、踢掉、吊销的令牌、停止的角色）
  在已排队数据之后发送关闭帧，而 fit 是直接中止；A238 请求 `WsSender::abort`。自 akio `8a9601b9` 起已删除：`WsWire::close`
  直接中止，被拒绝链路的 WebSocket 也一样。
- 拆分：hub 的 QUIC 握手限制（`handshake_timeout`、`max_handshakes` 的 Retry，akio-quic 的服务端尚不支持）和监听器证书文件变化时
  重新读取移到 I102d。akio API 请求 A236 标为完成。待审决定：第 20 行。
- 测试：经 `ws://`（一条隧道 12 MiB、四条隧道同时、其他路径回 404 且连不上、默认路径上用 `body_password` 密封）、经 `wss://` 用
  hub 的 pin（8 MiB）和错误的 pin（打不开任何东西），以及一个停止读取的读者只压住自己的隧道、同一链路上另一条隧道照常回显
  （`ws_links_a_slow_reader_holds_its_tunnel_alone`）。对照 fit 的二进制（`b6c5cebf`，会话临时目录中的脚本，不在 CI 中）：fit 的
  hub 配 akit 的 node 和 proxy、akit 的 hub 配 fit 的，经 `ws` 和 `wss`（pin），明文和密封：八种组合各回显 4 MiB。
- 下一步：I103。

### 2026-11-27：A234、A235 和 A237 的临时做法删除

- A234：每条隧道链路的 wire 在链路建立时以 fit 的 `LinkQueue`（`msg::LINK_QUEUE`，64 个 4 MiB 窗口加 4 MiB 控制消息余量，约
  260 MiB）为上限，同 fit 的 `link.New`（`Wire::set_max_queued`，带一个什么也不做的默认实现，映射到
  `Conn::set_max_queued_bytes`）。8 MiB 高水位删除（`link::HIGH_WATER`、`Link::congested`、`Link::when_drained`），随之删除它对
  隧道端和 hub 所读链路的压制：链路上等待的数据只由隧道的窗口约束，同 fit。QUIC 链路的流缓冲（8 MiB，`quic::STREAM_BUFFER`）
  和其 wire 的溢出队列合起来是 `LINK_QUEUE`。链路仍是中继连接（`set_relayed`）：akio 在 `write_high_watermark` 处的暂停会让
  互相写的两条链路停住。
- A235：TLS 和 QUIC 链路把 pin 交给 akio-tls（`pin_sha256`）并立即启动：不匹配的证书使握手失败，链路在此期间排队的内容不会
  发出。`tls::when_handshaken` 及其在 akio 定时器线程上每 5 ms 的轮询删除，`Link::deferred`、`Link::start` 以及 QUIC 拨号方在
  `on_connected` 中对叶证书的检查一并删除。
- A237：`akit_observe::stop_client_engine` 重新在调用线程上关闭引擎并等待（I/O 线程上不等待）；akio 修复之前会中止的
  akit-listener `acme_shared` 在 macOS 上通过。待审决定：第 18 行的结果。
- akio API 请求 A234、A235 和 A237 标为完成。
- 测试：链路把 wire 的上限设为 `LINK_QUEUE`；TLS 和 QUIC 的端到端测试（pin、错误的 pin 打不开任何东西、QUIC 上超过流缓冲的
  12 MiB）不加修改通过。

### 2026-11-27：I93，加密 store 的格式和 `akit store`；拆分出 I93b 和 I93c

- `akit_admin::store`：逐字节移植 fit 的 `admin/store`。`fitstore` 文件（保留魔数 `FITSTORE`：格式是 fit 的），59 字节的头（版本、
  Argon2id 的时间、内存和 lane 数、盐、nonce），内容以 akio-crypto 的 XChaCha20-Poly1305（A231）密封，密钥来自 akio-crypto 的
  Argon2id（A230），头作为附加数据。读取版本 1 到 3；写入版本 2，带 sections 时为 3，akit 不读 sections，但逐字节保留（fit 的
  assistant 和 cluster 把各自的内容放在那里）。配置按写入时的样子保存：内容的成员由本模块自己的严格 JSON 读取器找出（RFC 8259，同
  Go 的 `json.Valid`），不是 JSON 的配置在写入任何东西之前被拒绝。头中 Argon2id 的限制：store 自己的文件为时间 16、1 GiB，要解封的
  文件（导入）为 fit 参数的两倍；最多 64 MiB。写入：新的随机 nonce，在 store 旁边创建 0600 的临时文件，写入、同步、重命名覆盖，
  再同步目录；出错时 store 保持原样。导出用自己的密码（`seal`、`unseal`）或 store 的密钥密封。打开 store 时派生密钥（64 MiB、
  相当一部分秒数），只在控制面上进行。
- 按 fit 的方式读取并检查 `admin.store`（`path` 必填，`password`、`password_file`、`password_env` 恰好一个；从不显示密码）；
  `akit validate` 接受它，门禁的 `examples/console` 现在有效。在 I93b 把配置保存在其中之前，`akit run` 拒绝它，如同拒绝尚不运行的角色。
- `akit store export -c <config>` 打印 store 中的配置；`akit store import -c <config> <files>` 用引导文件的 `admin` 检查这些文件中的
  配置（像 `akit validate` 一样编译），拒绝其中的 `admin`，并以新的盐把它封存为 store，旧 store 能打开时保留其控制台用户。即 fit 的
  `fit store`。
- 互通：`crates/akit-admin/tests/fitstore` 存放 fit 自己的 `admin/store` 代码写的 store（版本 1、2 和带 sections 的 3，一个使用
  fit 自己的 Argon2id 参数，一个导出文件）和 `vectors.json`；`opens_what_fit_wrote` 打开每一个，`keeps_what_it_does_not_read`
  把版本 3 的写回并保留其 sections 和用户。反方向：`akit_writes_for_fit` 在设置 `AKIT_FITSTORE_OUT` 时写出 akit 的 store（低成本参数、
  fit 自己的参数、sections、导出文件）和清单，同目录 `akit_vectors_test.go` 中的 `TestAkitWrote` 在 fit 的 `admin/store` 包内运行，
  在本机用 fit 的代码打开：四个都能打开。移植了 fit 的 store 测试（`store::tests`，18 个），akit 的 CLI 测试中有
  `store_export_and_import`（也导出 fit 写的 store）。
- 拆分：I93b（`akit run` 使用 store：合并并写回基础配置、写入每个应用的配置、SIGHUP、`admin` 取自引导文件、控制台的密码修改、
  Admin API 的 `store` 格式）和 I93c（`run -admin`、用 `-c` 给出的 `.store`、配置文件旁边的 store 及其密钥文件）。等待评审的决定：
  第 19 行。
- 基准：无（规则 24：控制面）；开发计划中的行写明了要测量的内容（启动时的密钥派生、4 MiB 配置的一次写入）。

### 2026-11-27：akio 锁定在 `6151f5c5`：A234–A237

- akio：akio main 上的 `6151f5c5`，包含 A234（连接自己的发送队列上限，ADR-181）、A235（akio-tls 与 akio-quic
  客户端的服务器证书指纹固定，ADR-182）、A236（WebSocket 的读取暂停、排空通知与对端地址，ADR-183）和 A237
  （`Engine::wait` 可在线程局部变量的析构函数中调用）。akit 的 `rev` 与 `akit_runtime::AKIO_REV` 随之移动；akit
  无需改动即可构建。A234、A235 与 A237 的临时方案保留到替换它们的迭代。

### 2026-11-27：akio 锁定在 `9e21ada7`：A220、A230–A233

- akio：akio main 上的 `9e21ada7`，包含 A220（UDP 监听器、运行时绑定的 UDP 套接字和 QUIC 服务器上的 accept 过滤器，akio 的
  ADR-178）、A230 和 A231（akio-crypto 的 Argon2id、BLAKE2b 和 XChaCha20-Poly1305，ADR-179）、A232（连接句柄、关闭期限、监听器的
  accept 和 close 钩子，ADR-180）和 A233（关闭终端回显），以及逻辑线程池的改动（已接纳的工作穿过已满的线程池继续、等待中的池线程
  有线程顶替、压力监视线程）。akit 的 `rev` 和 `akit_runtime::AKIO_REV` 移到它；akit 无需任何改动即可在其上构建。
- **权宜实现（akio 需求 A237）**：akio 的 `Engine::wait` 现在读取 akio-pool 的一个线程局部变量，在另一个线程局部变量的析构中等待时它会
  panic（从而中止进程）：最后一个引用是线程缓存的运行时图会在那里丢弃它的客户端引擎（`acme_shared` 因此中止）。OTLP、provider 和共享
  ACME 存储的客户端引擎由 `akit_observe::stop_client_engine` 停止，它在为此启动的线程上等待（在 I/O 线程上则不等待），直到 akio 用
  `try_with` 读取它的线程局部变量。
- akit-middleware 的 `basic_auth_checks_off_the_io_thread` 在 gpu51 上的 release 构建中每次都失败，两种后端都是（单独运行 10 次失败
  10 次）：其成本为 8 的 bcrypt 检查在 debug 构建中约一秒，在这里却比不需要检查的请求的 20 ms 发送间隔还短，检查先通过了。测试现在取
  从 8 起、在本机哈希耗时达到 200 ms 的最低成本；此后在两种后端上单独运行都是 10 次通过 10 次。
- A220：监听器的 UDP 套接字在 akio 的过滤器处采用其访问策略（`ListenerAccess::datagrams`）：HTTP/3 的是监听器自己的列表然后全局列表，
  除非某个路由有自己的列表（此时它的请求逐个决定）；`udp` 监听器的是其会话所去服务的策略。被拒绝对端的数据报在建立 QUIC 连接或会话之前
  被丢弃，与 fit 相同。重载在保留的套接字上替换过滤器（`Engine::set_listener_filter`、`UdpBinding::set_filter`），新绑定的套接字从第一个
  数据报起就带过滤器（`Engine::bind_udp_filtered`）。权宜实现删除：HTTP/3 分发器的逐请求检查（`Dispatcher::unfiltered`）和 `udp` 入口在
  会话打开时的检查；决定 034 的第 6、9 点被替换，配置参考去掉两条说明。隧道 hub 的 QUIC 监听器也在过滤器处拒绝（`bind_udp_filtered`，
  由 `apply` 替换），hub 在链路的流打开时的检查删除。测试：`h3_peers_are_filtered`（被拒绝的对端得不到回答并计入 filtered，路由自己的列表
  逐请求决定，重载替换和绑定）、`udp_sessions`（服务自己的列表双向覆盖，被拒绝的客户端不建立任何会话，重载替换）、隧道的
  `quic_links_refused_at_the_filter`。
- A232：控制台的连接规则与 fit 相同（`limitHandler`），基于 akio 的连接句柄。监听器的 `on_accept` 为每个连接计数（`console::slots`，
  即 fit 的 `login.Slots` 和 `Peers`，连同其测试移植：`max_conns` 的四分之一留给已登录的连接，除本机外每个客户端最多占四分之一，
  新连接关闭占用最多的客户端最旧的未认证连接）并设置其期限为 5 秒后（`ConnHandle::close_at`）；`on_close` 释放其名额。带有效会话的
  请求（包括成功的登录）取消期限（此后两次请求之间保留 2 分钟，即 akio-http 的 keep-alive 超时）；在会话曾保留的连接上没有有效会话的
  请求重新设置期限。连接时间用尽时正在处理的请求在其后同样长的时间内得到回答，并带上 `Connection: close`。引擎的上限是 `max_conns`
  的两倍，留给正在关闭的连接。权宜实现删除：没有有效会话的回答、登出和 429 不再关闭连接。决定 036 的第 5 条被替换，配置参考中
  `admin.console.max_conns` 的说明删除。测试：fit 的 `TestConsoleIdleTimeouts`、`TestConsoleUnauthBoundFromOpen`、
  `TestConsoleUnauthBoundServing`（其宽限期用 `Options::console_grace` 单独设置，使负载高的主机上检查所需的时间不影响结果）和
  `TestLimitHandlerEvicts`。未移植：fit 的 Admin API 对没有令牌的连接的规则（决定 006 列出了它们），现在同样的 slots 可以承载它们。
- A233：`akit hash-password` 在终端上把终端置于原始模式（`akio_sys::terminal::raw`，其 guard 负责恢复），并像 fit 的 `readMasked`
  那样读两次密码：每个字符显示一个 `*`，退格删去一个字符（其全部字节），Ctrl-U 删去全部，丢弃转义序列，Ctrl-C、Ctrl-D 或输入结束为
  “interrupted”，最多保留 1024 字节。关于密码会被显示的说明删除；从管道仍读一行。移植了 fit 的 `TestHashPasswordTerminal`
  （`asks_twice_masked`）。

### 2026-11-27：I102b，QUIC 链路；本路线停在 I102c，等待 akio 的 A236

- `quic://` 链路（`akit_tunnel::quic`）：akio-quic，与 TLS 链路相同的配置（ALPN `fit-tunnel`、TLS 1.3、不恢复会话），每个连接
  一条双向流，由拨号方打开并以 fit 的版本字节（1）开头，hub 去掉它；此外不加任何内容，所以 fit 和 akit 的两端可以互连。hub 的
  `quic` 监听器是运行时绑定在监听器引擎上的 UDP 套接字（`Engine::bind_udp`；akio 在运行时添加 TCP 监听器，不添加 UDP 的），
  由一个 `QuicServer` 服务，至多容纳 hub 的链路数加上其 `max_handshakes`；其连接在流打开时按访问策略和 hub 的限制检查（akio 的
  A220 会在握手之前拒绝它们）。`idle_timeout` 和 `keep_alive` 交给 QUIC 传输；node 或 proxy 的 `quic_insecure` 接受任何证书，
  其 `pin` 在 `on_connected` 中、流打开之前检查（A235 的临时做法，但无需轮询）。
- 背压：流的发送缓冲就是链路的高水位（8 MiB）；akio-quic 超过它而拒绝的发送在 wire 自己的队列中等待，至多再 24 MiB（TCP 链路
  的连接在高水位之上能接受的量），在 `on_stream_writable` 时发出，之后被拥塞链路压住的喂给方恢复；压住链路的读取即压住流的读取，
  其流量控制窗口让对端停下。所有来自连接处理器之外的调用都在 `without_driving` 中进行，使处理器不会在链路或 wire 的锁下运行。
- WebSocket 链路是 I102c，等待 akio API 请求 A236：akio-ws 的 `WsSender` 既没有 `hold_reads`，也没有排空回调和对端地址，所以
  WebSocket 链路既压不住对端也压不住喂给它的一方（规则 8 和 9）。I104 的 mesh 链路是 WebSocket，同样在等待它。本路线停在这里：
  I102c、I103（其控制台套接字是 WebSocket）和 I104–I109 仍为 `todo`。hub 的 QUIC `handshake_timeout`、`max_handshakes` 的 Retry
  以及监听器证书文件变化时重新读取移到了 I102c。
- 测试：用 hub 的 pin 走 QUIC（12 MiB，超过流的缓冲，以及同时四条隧道）、`quic_insecure`、错误的 pin（什么都打不开）；手工对照
  fit 的二进制：fit 的 hub 走 QUIC 配 akit 的 node 和 proxy（pin 住它），以及 akit 的 hub 配 fit 的，两个方向都用 token。
- 下一步：akio 的 A236，然后 I102c；或者 I103 中不需要 WebSocket 的部分。

### 2026-11-27：I102，TLS 链路和 hub 的证书

- `tls://` 链路：akio-tls，只用 TLS 1.3，ALPN `fit-tunnel`，不恢复会话（`resumption(false)`、`session_tickets(false)`）。hub 的
  TLS 监听器提供其 `cert_file` 和 `key_file`（在应用成员时读取），或 hub 自己的证书：`quic.cert` 和 `quic.key`，或者一个只生成
  一次的自签名证书（P-256，十年，用于 `localhost` 和 hub 的名字），保存在配置旁，即 `akit.json` 旁的 `akit.tunnel.hub.pem`，
  权限 0600（`Host::kept_file`，I100 的前缀），其指纹写入日志；没有可以保存在旁边的配置文件时，每次启动都重新生成，日志会说明。
  node 或 proxy 的链路按 `pin`、`ca_file`（在建立 uplink 时于控制线程读取；fit 在每次拨号时读取）或系统根证书检查证书，从不
  依据地址中的任何内容。
- pin 的临时做法（直到 akio 的 A235）：akio-tls 要么检查证书链，要么什么都不检查，所以带 pin 的链路以 `insecure_skip_verify`
  进行握手（仍用叶证书的密钥检查服务器的签名），链路以 `deferred` 方式建立（不发送任何东西，包括其 hello），akio 的定时器
  线程每 5 毫秒轮询一次 `tls_info`，直到握手完成（限 `dial_timeout`），再以常数时间比较叶证书的 SHA-256 与 pin；之后链路才启动；
  不匹配则在不发送任何东西的情况下关闭连接，提示 “the hub's certificate does not match the pin”。
- WebSocket 和 QUIC 链路拆到 I102b：它们的监听器和端点在角色应用时仍给出警告。监听器的证书文件在应用成员时读取，文件变化时
  不会重新读取（I102b）。
- 测试：用 hub 的 pin 经 TLS（传输 4 MiB）、错误的 pin（什么都打不开）、重新启动的 hub 指纹不变、由 `ca_file` 检查的 CA 签名的
  监听器证书（此时不保存证书）；用 fit 的二进制手工检查：fit 的 hub 经 TLS 配合用 pin 固定它的 akit node 和 proxy，以及 akit 的
  hub 配合 fit 的 node 和 proxy，两个方向都用 token。
- 下一步：I102b（WebSocket 和 QUIC 链路）或 I103。

### 2026-11-27：I101，基于 TCP 的隧道链路、注册和隧道

- 新 crate `akit-tunnel`（fit 的 `tunnel`）：`crypt`（fit 的 `bodycrypt`）、`msg`（fit 的 `proto`：方法、以 fit 的字段名编码为 JSON
  的消息、密码的 MAC 和 hub 的证明、token 的记录）、`link`（fit 的 `link`：帧、调用、回复、通知、版本 3 和 4 的 hello、密封的
  正文）、`end`（fit 的 `tunnel`：每条隧道的一端，4 MiB 窗口，连接队列低于 1 MiB 时每 256 KiB 确认一次）、`uplink`（node 或
  proxy 到 hub 的链路池：按顺序拨号，用 token 或密码注册，在为 hub 服务之前检查 hub 的证明，ping，`retry_interval` 后重拨）、
  `hub`、`node` 和 `proxy`。`akit run` 运行这三个角色（`akit::ROLES`）；只有角色的配置会得到一个用于拨号的 I/O 线程。
- 基于 akio：一切都是回调。链路在其连接的 `on_data` 中读取；需要等待的调用（node 拨号服务、hub 经 node 打开隧道、proxy 经
  hub 打开隧道）由结束等待的回调通过 `Responder` 回答；调用的超时、重试、ping、空闲清理和 token 过期都运行在 akio 的定时器
  线程上（`after_send`）；拨号经过 akio 的解析器和 Happy Eyeballs。链路和隧道的连接是 akio 的“中继”连接（`set_relayed`，即 fit
  的 `SetWriteBackpressure(false)`）：否则链路两端互相写时，各自在 akio 的写水位处停止读取并互相等待（由密封 token 的测试发现，
  它在 64 KiB 处卡住）。改由窗口和链路的高水位来调节：排队超过 8 MiB 的链路会压住喂给它的一方（隧道的连接，或 hub 读取的那条
  链路），直到它排空（`on_drain`）。
- 与 fit 的不同：fit 把链路的发送队列上限提高到其 64 条隧道的窗口之和（260 MiB）；akio 对整个引擎只有一个上限
  （`limits.max_write_queue_bytes`，32 MiB），所以链路在超过 8 MiB 时压住喂给它的一方（已写成 akio API 请求 A234，即单个连接
  自己的上限；不阻塞任何工作）。TLS、WebSocket 和 QUIC 链路属于 I102：这些协议的监听器或端点在角色应用时给出警告并被跳过。
  hub 对对端的调用（查询、设置服务、踢掉）、token 过期时关闭其链路以及控制台套接字属于 I103；被撤销的 token 的链路在这里已经
  立即关闭（登记表的关闭回调）。
- 互通性（决定 038），用 fit 自己在 `7daee573` 的二进制手工检查（会话临时目录中的一个脚本，不在 CI 中）：fit 的 hub 配 akit 的
  node 和 proxy，以及 akit 的 hub 配 fit 的 node 和 proxy，基于 TCP、使用 token，明文和 `body_password` 密封：各自回显一兆字节。
- 测试：fit 的 `bodycrypt` 和 `proto.Prove` 向量（密码和 token 下密封的正文、hub 的证明）逐字节一致；内存中的链路（调用、通知、
  明文和密封、两端不匹配、超过上限的帧）；在一个 akio 引擎上的端到端测试（`tests/e2e.rs`）：密码下 16 MiB 和同时 8 条隧道、
  密封下 8 MiB、明文和密封的 token 及其成员绑定、错误的密码、两个成员共用的 token；经由二进制
  （`crates/akit/tests/tunnel.rs`）：带 Admin API 和 hub 的 `akit run`，用 `akit token create` 为由第二个 `akit run` 运行的 node 和
  proxy 签发 token，经它们的隧道，`akit token list` 显示绑定的成员，`akit token revoke` 关闭 proxy 的链路。
- 需要测量（规则 30，基准表中的新行）：一条和 64 条隧道的吞吐、每秒打开的隧道、hub 每 GB 的 CPU、每条隧道和链路的内存，
  对比 fit。
- 下一步：I102，TLS、WebSocket 和 QUIC 链路。

### 2026-11-27：I100，`akit run` 中的角色

- `akit-roles` 的 `host`，即 fit 的 `app`：一个 `Manager`，在每次应用配置时于监听器之后，启动配置设置的每个角色，把配置
  应用到每个运行中的角色，并停止配置去掉的角色；角色失败是重载回答中的警告，网关照常运行。角色得到一个 `Host`：监听器的
  akio 引擎（不另建引擎）、排空超时（`-shutdown-timeout`）、它保存在配置旁的文件的前缀（`akit.json` 旁的
  `akit.tunnel.hub.pem`，决定 038），以及修改自身成员的 `edit`：编辑通过控制线程的队列送达，在那里像任何配置一样以成员路径
  为来源应用，其警告或拒绝再告知角色。akit 运行的角色是 `akit::ROLES`；设置了其他成员的配置被 `run`、其重载和 Admin API
  拒绝，但不被 `validate` 拒绝（自 I98 起，编译配置时就拒绝了角色：现已移出 `compile_config`）。角色在引擎之前停止。
- 一个地址，一个服务器（fit 的 I162，`config/bindings.go`）：配置监听的每个地址——提供服务的监听器（包括默认 HTTPS
  生成的）、`admin`、`admin.console` 以及各角色的（控制台、`listen`、`api`、hub 的监听器和控制台套接字、proxy 的转发和
  web 代理）——规范化（所有地址、`localhost`、IPv4 映射）后在 `akit-config` 的 `binds` 中统一检查一次，冲突报在两者中
  后出现的那个上，并指出先出现的那个。akit 以前不做任何检查：两个监听器在同一端口的配置（fit 在锁定的提交中拒绝）现在也
  被拒绝。fit 为其他进程增加的绑定时探测没有移植：akio 的监听器照旧绑定。
- Admin API 的 `GET /tokens`、`GET` 和 `POST /tokens/{成员}`、`PATCH` 和 `DELETE /tokens/{成员}/{id}`（fit 的
  `admin/tokens.go`）：读取在 API 的线程上进行，来自运行中配置的成员，以及角色运行时其登记表（最后使用、链路）；修改在
  控制线程上作为对成员 `tokens` 的编辑进行，记录到 `admin` 日志器。与 fit 不同，无论角色是否运行，只要运行中的配置设置了
  该成员，它们就可用（fit 在角色运行前回答 404）。标为 v1 新增。`akit token create|list|expiry|revoke`（fit 的
  `cli/token.go`）通过 akio 的 HTTP 客户端调用它们，API 的地址、令牌和 TLS 从配置的 `admin` 读取。
- 测试：宿主（占位角色的启动、迁移、失败、停止；拒绝没有角色运行的成员；编辑）、地址冲突（fit 的
  `TestBindsEveryKindClashes`、`TestBindsNormalized` 和隧道的冲突）、对 Admin API 端到端测试 token 端点（签发且只显示一次、
  只保存哈希、列出、修改过期时间、拒绝、撤销）、包含新端点的 v1 接口。
- 下一步：I101，基于 TCP 的隧道链路。CLI 的端到端测试随它一起，待 hub 运行之后。

### 2026-11-27：I99，链路 token

- 新 crate `akit-roles`（fit 的 `app`，I100 加入宿主）：`linktoken`，即 fit 的 `internal/linktoken`（fit 的决定 035）。token
  为 `akitt1.<id>.<secret>`；其 client、stored、server 和 body 密钥以及双方的证明与 fit 逐字节相同，基于 akio-crypto 的
  HMAC-SHA256、HKDF 和 SHA-256（决定 038）。监听方的 token 登记表把成员绑定到它首次连接所用的 token，拒绝未知、过期或
  已满的 token 以及绑定到其他 token 的成员，关闭被撤销、哈希改变或过期的 token 的链路（`set`），以及在链路打开期间过期的
  token 的链路（`sweep`，角色每隔 `next_sweep`（至多一分钟）调用一次：fit 的计时器在这里由角色负责）；链路的 `Use` 在被
  丢弃时让登记表忘记它。签发、修改过期时间（`ttl` 或 `expires_at`）、撤销和绑定都是对成员 token 列表的编辑，由调用方作为
  配置应用。
- 与计划的不同：Admin API 的 `/tokens` 和 `akit token` 移到 I100，与读取其角色登记表的宿主一起；fit 的 CLI 调用 Admin API，
  akit 的也将如此。
- 测试：移植 fit 的 `TestSecretRoundTrip`、`TestHandshake`、`TestRegistry`、`TestRegistryListOrder`、`TestRegistryFull`、
  `TestParseTTL`、`TestManager` 和 `TestBind`；fit 自己的测试向量（一个哈希、一个 body 密钥、拨号方和监听方的证明，由一个
  调用 fit `7daee573` 中该包的 Go 程序打印）逐字节一致（`fits_vectors`）。
- 下一步：I100，`akit run` 中的角色。

### 2026-11-27：I98，配置中的 `tunnel` 和 `llm`

- 隧道与 LLM 路线规划为 I98–I109（开发计划）；[决定 038](decisions/038-tunnel-and-llm.zh-CN.md)：akit 显示名字之处用
  akit 的名字，线上协议与 fit 逐字节相同，使 akit 和 fit 的角色能够互连。
- `akit-config` 的 `roles` 模块：六个成员（`tunnel.hub`、`tunnel.node`、`tunnel.proxy`、`llm.hub`、`llm.node`、`llm.proxy`），
  字段、默认值（与 fit 一样用方法给出）、范围和检查与 fit 相同（`config/tunnel.go`、`config/llm.go`、`config/linktoken.go`）：
  端点被拆解，拒绝查询、片段或用户信息（I160 的攻击）；拒绝非回环地址上没有 `body_password` 的 tcp 和 ws、非回环地址上的
  `quic_insecure`、非回环地址上没有 TLS 的 LLM 监听器和对端；`quic_pin` 带 fit 的警告读取；token 的成员只绑定一次；各种
  API 的模型。链路 token 为 `akitt1.<id>.<secret>`（决定 037）；`fitt1.` 被拒绝。
- 在运行角色的迭代完成之前，`akit run` 拒绝设置了角色的配置（`roles_not_running`）；`akit validate` 接受 fit-examples 的
  六个示例（门禁：55 个有效，24 个仅因尚不支持被拒绝）。两个地址的冲突是 I100 的（fit 的 I162）。
- 测试：移植 fit 的 `TestLLMChecks`、`TestLLMMembers`、`TestLLMDefaults`、`TestTunnelChecks`、`TestTunnelNames`、
  `TestLinkTokenChecks` 和 `tunnel/config_test.go`（`roles/tests.rs`）；门禁为 376 个字段各取两份只在该字段上不同的配置
  （`tests/compat/tests/support/roles.rs`）；376 个字段标为 v1 新增；配置参考的“隧道”和 “LLM” 两章。
- 下一步：I99，链路 token。

### 2026-11-27：I94，重新读取 Admin API 和控制台的证书

- 先于 I93 完成：I93 等待 akio 的 A230 和 A231，而 I94 不需要它们。
- 同 fit 的 `admin/tls.go`：`admin.cert_file`、`key_file`、`client_ca_file` 以及 `admin.console.cert_file`、`key_file`
  在变化时重新读取，新连接使用加载的结果，无需重启，也不关闭任何连接；加载失败的文件（证书已写入而密钥尚未写入、没有
  证书的客户端 CA 文件）记录到 `admin` 日志器（`<admin|console>: <原因>; the running certificate stays`），在再次变化之前
  不再尝试。
- 做法（`crates/akit-admin/src/certs.rs`）：每个服务器的 acceptor 是一个 `Swap`，它为每个连接的传输层询问由最近加载的
  配置构建的 akio-tls `Tls`（akio 的 `Acceptor::transport`，每个连接调用一次）；控制线程替换它，锁只在替换时持有。
  变化的客户端 CA 文件也会生效，这是只用证书解析器做不到的。不需要修改 akio。
- 与 fit 的不同：fit 在握手时查看文件的大小和时间，最多每秒一次；查看文件会阻塞，而 akit 的握手在 I/O 线程上运行
  （规则 1），所以文件由监听器的证书监视器在控制线程上查看（`akit_tls::FileWatcher`：带 `-watch-certs`（默认开启）时每个
  `-watch-interval` 一次，文件保持 500ms 不变，内容与已加载的比较），SIGHUP 时立即重新读取。fit 只在启动时读取控制台的
  证书；akit 与 API 的一样重新读取。`akit_admin::Server::watch_certs` 和 `reload_certs`；由 `akit run` 调用。
- 测试：移植 fit 的 `TestTLSReload`，用于 API（`tls_reload`：续期后在 SIGHUP 时读取；经监视器时无法加载的密钥被拒之门外
  且不再重试；文件修好并稳定后加载，之前不加载）和控制台（`tests/console.rs` 中的 `tls_reload`：经监视器读取改名替换的
  续期证书，缺少密钥的证书被拒，并检查 `admin` 日志器的行）；`tls_reload_client_ca`（替换客户端 CA：另一个 CA 的客户端
  被接受，旧 CA 的被拒绝；没有证书的文件保留正在使用的配置）；端到端：`akit run` 无需重启即提供续期的 Admin API 证书，
  写了一半的续期保留正在使用的证书（`tests/watch_certs.rs` 中的 `admin_certificate_renewed_without_a_restart`）。配置参考的
  注释、Admin API 指南（之前仍说 API 没有 TLS）和控制台指南。
- 检查：macOS 上的 `make check`（fmt、clippy、整个 workspace：1,353 个测试通过，deny、文档、兼容性、打包）；gpu51 上
  epoll 和 io_uring 的 clippy 与整个 workspace（`--test-threads=4`），各通过 1,353 个测试，没有失败或挂起。。
- 无需测量：控制面（规则 24）；连接在 accept 时取的锁只属于 API 和控制台自己，从不属于监听器。

### 2026-11-26：I97，akit 的名字

- [决定 037](decisions/037-akit-names.zh-CN.md)（维护者的决定）：对外凡写 "fit" 的名字都改为 "akit"，不保留别名。改名的有：
  Admin API 的 `Akit-Config-Version` 和 `Akit-Dry-Run`；`Akit-Error` trailer（代理、FastCGI）；控制台的 `akit_console_<端口>`
  和 `X-Akit-Csrf`；导出的 `akit-<成员>-v<n>.<格式>`；17 个指标族 `akit_*`；OTLP scope `akit` 和 span 属性 `akit.listener`、
  `akit.route`、`akit.upstream.address`；`Cache-Status` 的 `akit`；WebAssembly ABI（`akit_on_request`、模块 `akit`）；九个
  默认值（`label_prefix`、`tag_prefix` 为 `akit`；KV、etcd 和共享存储的前缀为 `akit/`；`controller_name` 为
  `lesismal.github.io/akit`；`realm` 和 `service_name` 为 `akit`；`akit_sticky`）、Ingress 注解 `akit.lesismal.github.io/*`、
  `jwt` 的 realm；默认配置文件 `akit.json`；`adapt` 的格式名 `akit`（`-from`、`-to`）。保留的有：配置的字段名和格式、`fitstore`
  （I93）、Proxy-Wasm 的 `proxy_*`、文档和消息中指 fit 的 "fit"，以及 `akit adapt` 的转换警告（与 fit 的逐字节相同）。
- 控制台的页面：`tools/console-pages.py` 复制 fit 的工作台并应用一组确定、列出的改名规则（`RENAMES`、`KEEP`；写入
  `console/CHANGES`），每个被改动的文件带一条说明（Apache-2.0 第 4(b) 条），保留 fit 的 `LICENSE`；fit 的 schema 同样改名，写为
  `console/schema.json`，由 `GET /api/m/admin/schema` 提供。`--check` 检查每个文件都是改名规则的不动点，并在有 fit 克隆时检查与
  改名后的 fit 文件逐字节相同。69 个文件中改动了 50 个。在 Browser 面板中检查：不带 `-c` 的 `akit run` 读取了 `akit.json`；登录页
  和标题显示 "akit"；对 `limits` 的一次表单编辑经检查（`?dry_run=1`）后作为版本 2 应用；页面读取了 `Akit-Config-Version`；用 curl
  检查：cookie 是 `akit_console_<端口>`，带 `X-Fit-Csrf` 的 POST 被以 403 拒绝、带 `X-Akit-Csrf` 的得到回答，导出名为
  `akit-config-v2.json`，试运行回答 `Akit-Dry-Run: 1`。
- WebAssembly 测试插件：`tools/plugin-guest.sh` 在固定的提交处用 fit 的 guest 包和 `testdata/headers` 构建 `headers-akit.wasm.gz`，
  只把 `//go:wasmimport` 和 `//go:wasmexport` 的名字改名（Go 1.27.3、`-trimpath`，可复现；`--check`）；fit 的构建保留为
  `headers-fit.wasm.gz`，一项测试表明 akit 拒绝它，基准测试中的 fit 服务器运行它（`bench/lib.sh`、`bench/modules.sh`、
  `bench/fitbench/akit.go`）。基准中 akit 的配置为 `bench/configs/akit/akit.json`。
- v1 基线重写一次：`tools/v1-freeze.py --write-baseline` 现在写出当前表面减去已标记的新增项，并从 `tools/config-reference-notes.json`
  新增的 `defaults` 取 akit 的默认值（`config.txt` 改变 9 项）；`admin-api.txt` 的两个请求头字段手工改名；校验和已更新。配置参考
  显示 akit 的默认值，并在注释中给出 fit 的默认值。
- 文档：决定 037，修订决定 002、006、007、008、032 和 036，迁移指南中 fit 用户要改的内容表，WebAssembly 指南的 ABI 一节，控制台、
  Traefik、nginx 和 Caddy 指南，兼容性页面，参考，路线图，基准 README，compose 示例，开发计划（I97）。
- 测试：macOS 上 `make check`（fmt、clippy、整个工作区、deny、文档、compat（页面与 fit 的逐字节比较）、
  打包）；gpu51 上 clippy，以及 epoll 和 io_uring 上的整个工作区（`--test-threads=4`），每次 1,348 个测试通过，没有失败或挂起。
  无需测量：改名不触及热路径；下一次 `wasm` 基准中 akit 运行 `headers-akit`，fit 运行 `headers-fit`，是同一个插件。

### 2026-11-26：I92，带 TLS 的 Admin API；本路线停在 I93

- 读取 `admin.cert_file`、`key_file` 和 `client_ca_file`，同 fit 的 `checkAdminAddress`：非回环地址只有在有令牌和 TLS 时才允许（错误
  中列出缺少的项），两个文件成对出现，`client_ca_file` 只能与它们同用，Unix 套接字上三者都不允许；`mptcp` 仍被拒绝。API 经 TLS 提供
  （`akit_tls::server_config`，TLS 1.2 和 1.3），设置了 `client_ca_file` 时用它验证每个客户端的证书（akio-tls 的客户端认证，必需）；
  主机为空的地址监听所有地址；在 TLS 上，守卫把 API 自身的来源视为 `https://`。用 `make v1-bless` 标记（3 项新增）。威胁模型的 L1
  关闭（带 TLS 和登录的控制台，I90；带 TLS 的 API，I92），并新增控制台控制措施一节；L5 仍为接受（决定 036 第 6 条）。
- 测试：移植后的 fit `TestTLS`、`TestTLSClientCA`（没有证书、另一个 CA 签发的、正确的）和 `TestTLSStartErrors`
  （`crates/akit-admin/tests/admin.rs`）；配置检查。`TestTLSReload` 等 I94：文件在 akit 启动时读取。
- 本路线停在 I93，加密的 store：fit 的 `fitstore` 格式用 Argon2id 派生密钥、用 XChaCha20-Poly1305 密封，而 akio-crypto（`077a6710`）
  两者都没有：akio 请求 [A230](akio-api-requests.zh-CN.md#a230-akio-crypto-中的-argon2id) 和
  [A231](akio-api-requests.zh-CN.md#a231-akio-crypto-中的-xchacha20-poly1305)。在此之前控制台修改密码的端点回答 409，导入导出页面的
  `store` 格式被拒绝。I94（文件变化时重新读取证书）在计划中排在 I93 之后，不依赖 akio 即可实现。

### 2026-11-26：I91，控制台的页面

- 原样提供 fit 的工作台（[决定 036](decisions/036-console.zh-CN.md)，第 8 条）：`tools/console-pages.py` 在门禁固定的提交（`7daee573`）上
  把 fit 的 `app/workbench/web`（69 个文件，2.5 MB）和其许可证复制到 `crates/akit-admin/console`，并写出 `console/files.rs`，由其中的
  `include_bytes!` 编进 akit；从内存提供，带 fit 的内容类型和 `Cache-Control: no-cache`，`/` 即 `index.html`。`make compat-check` 运行其
  `--check`，一个测试把编进的文件与该目录比对；固定提交移动时由 `tests/compat/update-fixtures.sh` 运行它。`GET /api/m/admin/schema`
  回答门禁的 `fit.schema.json`。
- Admin API 的文本端点，即 fit 的（`admin/check.go`、`admin/transfer.go`），在 `akit_admin::text` 中：`POST /config/render` 和
  `/config/parse`（`.conf`、YAML、JSON，通过 `akit_config::adapt::document`）、`PUT /config/text`（`members=`、`dry_run`、`If-Match`；
  检查文本设置的成员是否属于该页面）、`POST /config/export`（不含 `admin`，秘密像读取时那样被遮蔽）和 `/config/import`（保留 `admin`）。
  文本中的 `[redacted]` 秘密像 `PUT /config` 那样放回；其错误定位到文本中的行（新增的 `akit_config::position_of`），同 fit 的 adapter。
  在 I93 之前拒绝 `store` 格式。标记为 v1 新增。冻结测试现在从 `Api::serve` 读取分发器的路径（I90 把它们移到了那里）。没有请求体的修改
  现在会得到回答（此前会挂起）。
- 在浏览器中检查（Browser 面板，带控制台的 `akit run`）：登录、来自 `/status` 的概览、日志页面（其文件、**立即切割**）、`.conf` 标签页
  （渲染与解析、定位到行的解析错误）、`limits` 的表单编辑、版本与回滚；每次修改都记入 `admin` 日志记录器。akit 拒绝的 fit 功能的菜单，
  以及 akit 缺少其端点的页面（`/node`、`/config/check`），按决定 036 所说显示。
- 测试：fit 的 `TestTextRenderParse`、`TestTextParseErrors`、`TestTextPartSave`、`TestTextWholeConfig` 和 `transfer_test.go` 中文本格式的
  用例（`crates/akit-admin/tests/admin.rs`），`pages_and_schema`（`tests/console.rs`）。指南的页面一节。下一步：I92。

### 2026-11-26：I90，控制台的服务器

- `akit_admin::console::server`（fit 的 `app/console` 和 `admin/console.go`；[决定 036](decisions/036-console.zh-CN.md)）：
  akio-http 运行在自己的引擎上，一个 I/O 线程，HTTP/1.1，用 `cert_file` 和 `key_file` 提供 TLS（`akit_tls::server_config`，
  TLS 1.2 和 1.3）；`max_conns` 在 accept 时限制；`admin.console.access`、再是全局访问控制，在 accept 时决定
  （`Graph::console_access`，每次应用配置都重新设置，`Server::set_console_access`；`admin_warning` 不把它算在内）。设置了
  `admin.console` 时由 `Server::start` 与 Admin API 一起启动，先绑定；`akit run` 打印 `akit: console on <url>`。
- fit 的端点：`POST /api/login`（4 KiB 以内的 JSON；先由登录模块准入，再在自己的池上检查，`akio::Pool`，一个线程、16 个等待，
  超出 503；锁定时 429，带 `Retry-After` 和 `retry_after`）、`POST /api/logout`、`GET /api/session`、`GET /api/workbench`
  （`{"user", "modules": ["admin"]}`）、`GET /api/m/admin/users`、修改密码的端点（409：没有 store），以及
  `/api/m/admin/<路径>`：在同一个请求上调用 Admin API 的分发，改写其目标（`Context::req_mut`），跳过其令牌和 `Host` 检查，所应用的
  配置来源为 `console`（`Shared` 增加了来源；`Submit` 在内部变为 `Arc`，两者共用）。fit 的 cookie（`fit_console_<端口>`、
  `HttpOnly`、`SameSite=Strict`，TLS 上 `Secure`）、`X-Fit-Csrf`、来源检查和每个回答上 fit 的响应头。`admin` 日志记录器：登录及其
  来源、失败、登出，以及每次修改所应用的版本或其状态（一个响应过滤器，使写出回答者记录它）。
- 在 akio 的 [A232](akio-api-requests.zh-CN.md#a232-由处理函数设置-http-连接的空闲超时并关闭它以及已接受连接的表) 之前的
  权宜实现：没有登录的连接没有 5 秒的限制，超过 `max_conns` 时也不驱逐；对没有有效会话的请求的回答会关闭其连接。
- 测试：`crates/akit-admin/tests/console.rs`（fit 的 `TestConsoleLoginAndEdit`、`TestConsoleLoginRefused`、`TestConsoleSessions`、
  `TestConsoleFailureTableBounded`，TLS，accept 时的访问控制及其替换）；`crates/akit/tests/console.rs` 在 `akit run` 下（通过控制台
  的登录和加载记入日志，全局访问控制和 `admin.console.access` 随加载变化）。指南 `docs/guides/console.zh-CN.md`；兼容性页面的
  Admin API 一节。下一步：I91。

### 2026-11-26：I89，控制台的配置与登录

- 按 fit 的 `config.Console` 读取 `admin.console`（`akit-config`）：`address`（127.0.0.1:2020）、`cert_file` 和 `key_file`
  （成对出现；非回环地址没有它们时被拒绝）、`users`（至少一个；有名字，不重复；哈希以 bcrypt 的 `$2` 开头）、
  `session_timeout`（30m）、`session_lifetime`（12h）、`max_sessions`（64）、`max_conns`（32）、`access`；`mptcp` 以“尚不支持”
  拒绝。只有 `admin`、没有监听器和路由的配置原本就有效。用户的 `Debug` 不含其哈希。用 `make v1-bless` 标记（16 项新增）；门禁的
  `examples/console` 现在只因 `admin.store` 被拒绝（`make compat-bless`）；配置参考新增控制台一节，其表格由工具生成。
- `akit_admin::console::login`，即 fit 的 `login` 包：bcrypt 检查（`akio_crypto::bcrypt`），同时最多两个，未知的名字以用户的代价
  对一个假哈希检查；按地址锁定（IPv6 按 /64），加倍直到一小时，一个地址同一时间只检查一个，最多记录 4,096 个地址，表满也从不提前
  结束锁定；用户连续失败 10 次后其检查被延缓，不是用户的名字合并计数；会话为 32 个随机字节，按其 SHA-256 保存，空闲、超过总时长、
  登出或作为最旧者时结束；每个会话一个以常数时间比较的 CSRF 令牌；cookie 名带端口。时间为距一年前某个起点的时长，使从未设置的
  时间早已过去，同 Go 的零时间。移植后的 fit `login` 测试通过（15 个），使用假时钟。
- `akit hash-password`（`-cost`，10）：标准输入的第一行；从终端输入时询问两次。fit 会关闭终端回显，这需要 `tcsetattr`（unsafe）：
  akio 请求 [A233](akio-api-requests.zh-CN.md#a233-关闭终端回显以读取密码)。在此之前的权宜实现：从终端输入时密码按输入显示，
  提示中说明这一点以及如何隐藏。
- 为本路线写的 akio 请求：A230（Argon2id）和 A231（XChaCha20-Poly1305），I93 等待它们；A232（控制台的连接限制，用于 I90）和 A233。
- 测试：akit-config、akit-admin 的 `console`、akit 的 `cli` 和门禁的 `cargo test`；clippy 和 fmt。下一步：I90。

### 2026-11-26：akio 锁定到 `077a6710`：A200 与 A180

- akio：akio main 上的 `077a6710`，包含 A200（absolute-form 请求目标，akio 的 ADR-177）和 A180（WebAssembly guest
  调用便宜 44%：fit 的 Go `headers` guest 在 gpu51 上每次调用从 9,089 降到 5,068 ns，wazero 编译器 3,520 ns），
  以及 akio main 上的改名（`handlers_on_logical_pool`、work-stealing 调度），akit 没有用到旧名字。除 A220 和 A225
  外，akit 的所有请求都已进入 akio 的 main 并锁定。
- A200：删除 akit-listener 的 `absolute_form` 改写（`Dispatcher::call` 和 `body_mode` 中）以及路由请求的主机覆盖：
  akio 以 origin-form 给出请求，主机取自目标。访问日志写 `Request::raw_target()`，即原样的目标，与 fit 的 `RequestURI`
  相同（`every_request_has_its_line` 增加一个 absolute-form 请求）。与 fit 的一处差异：主机为空的 http URI（`GET http:///x`）
  由 akio 回答 400（RFC 9110 第 4.2.1 节），而 Go 的服务器按 `Host` 字段路由它；`absolute_form_targets_name_their_host`
  写明了这一点（[待审决定](decisions-pending.zh-CN.md)第 15 行）。计划中监听器检查的基准行随之删除。
- A180：akit 无需改动；`wasm` 场景的数字留给基准任务。

### 2026-11-26：I80b、日志（I81–I83）与访问控制列表（I85–I87）合并

- akio：未变，`6bcd8ad6`。三个分支一起合并：计划的索引和路线表、状态行和日志保留三者的内容；门禁的字段清单、
  配置参考（读取 fit 的 485 个字段）和 v1 新增项（57 个）按合并后的源重新生成；威胁模型的 L6 写明访问控制列表已实现。
  下一个可用的迭代编号是 I89（为两条路线预留的 I84 和 I88 未使用）。
- 测试：macOS 上 `make check`；gpu51 上 clippy，以及 epoll 和 io_uring 下的整个工作区。io_uring 运行中，
  akit-middleware 的 `basic_auth_checks_off_the_io_thread`、`breaker` 的 `stops_and_resumes_a_proxied_upstream`
  和 `compress_proxied` 在另一个会话的 io_uring 测试同时运行时失败；单独运行时 3 次全过。

### 2026-11-26：I80b，安全加固：威胁模型的局限（进行中）

- **L2，遮蔽内联的秘密**（`crates/akit-admin/src/redact.rs`）：`GET /config` 及其指针、`/versions/{n}` 和
  `/config/effective`（因而仪表盘也是）对 fit 的 `secretName` 认定为秘密的成员下的每个字符串答 `[redacted]`（fit 控制台、
  助手和集群管理的规则：`password_hash`、`secret`、`token`、`Authorization`、`*_key` 等；不含 `*_file`、`max_*`；保留
  `rate_limit` 的 `key` 选择器；推流目标的 `url` 遮蔽）。没有秘密的配置逐字节返回；有秘密的按 fit 写出编辑后配置的方式写出；
  `ETag` 是配置的。`POST /load`、`PUT` 和 `PATCH` 在请求体送回 `[redacted]` 处放回运行中配置的秘密（按成员、`name` 或下标
  匹配，与 fit 的 `restoreSecrets` 相同），其位置上没有秘密的拒绝（400）。fit 自己的 Admin API 不遮蔽：
  [决定 035](decisions/035-admin-api-redacts-secrets.zh-CN.md)、兼容性页面和 Admin API 指南写明了这一点。
- **L3**：`akit_config::hardening::secret_files` 对 `key_file`、`tls_key_file`、`token_file`、`password_file`、
  `hmac_secret_file` 或 `kubeconfig` 所指、其组或其他用户有任何权限的文件发出警告（每个文件一次），在 `akit run` 启动时、
  `akit validate` 中，以及每次应用或检查修改时的 `warnings` 中（为此 runner 的 `apply` 和 `check` 接收配置的 JSON）。
  `FileStorage` 把其根目录及其下被写入的每个目录在对他人开放时收紧为 0700（尽力而为），并在每次重命名后同步目录。
- **L4**：`hardening::cleartext` 对以下情况发出警告：不在环回上的 `http://` ACME 目录；不在环回上明文访问的共享存储（未设
  `tls` 的 Redis，经 `http://` 的 etcd 或 Consul：其密码或令牌以及其中保存的私钥）；经 `http://` 发送的 Consul 令牌或 etcd
  密码；经 `tcp://` 的 Docker。Kubernetes provider 在找到集群时，对不在环回上的 kubeconfig `http://` 服务器或
  `insecure-skip-tls-verify` 写日志（`RestConfig::exposed`）。只是警告：fit 能运行的一个也不拒绝。
- **L6、L13**：配置参考中 `forward_proxy`（没有认证；除非拒绝否则私有网段可达，并给出要写的 `deny` 行）和 `compress`
  （BREACH）处的说明。计划中提到的正向代理指南并不存在：参考文档的那一节就是代理的文档。
- **L7**：fit 的 `static` 有意跟随链接，无论指向何处，且没有选项；akit 保持一致，在 `static` 处写明并有测试
  （`symlinks_out_of_the_root_followed`）。
- **L12**：fit 的配置没有请求头或请求体超时的字段；按现状关闭。
- **L14**：`cache_purge_guarded`：不带令牌、令牌错误、带 `Origin`（即使是 API 自身的来源）时，`POST /cache/purge` 被拒绝且
  不清除任何东西；带令牌时清除。
- **已移出**：L8（akio-tls 已有 `client_crls` 和 `client_ocsp_policy`；fit 没有字段）和 L9 交给 fit，在决定 035 中提议
  `tls.client_auth.crl_files` 和 `module_sha256`；L15 作为
  [A225](akio-api-requests.zh-CN.md#a225-对-akio-的密码学tlshttp-解析器和-webassembly-的外部安全审查) 向 akio 提出。
  威胁模型中已由 akio 锁定和 I79b 关闭的 L10 和 L11 也已标明。
- 文档：[威胁模型](security/threat-model.zh-CN.md)（控制措施和测试、各局限的状态）、[审查范围](security/review-scope.zh-CN.md)、
  决定 035、[待审决定](decisions-pending.zh-CN.md)第 14 行、开发计划中 I80b 的状态。
- 测试：macOS 上的 `make check`（fmt、clippy、工作区测试、deny、文档、兼容性、打包）。gpu51 上的运行（clippy、epoll 和 io_uring 上的工作区测试）没有进行：akio 的基准测试独占了 `/root/bench.lock`；合并前需补上。
- 下一步：外部审查的发现，它们完成 I80b。

### 2026-11-26：I83，重载时的日志文件，以及 Admin API 的 `/logs`

- **重载**：`Root::set_config`，即 fit 的 `SetConfig`：就地改变级别（以及 akio 的最高级别）、时区，以及日志每个文件的切割和保留
  设置；不再提到的文件继续写，不再切割。`akit run` 的重载对 SIGHUP、`-watch`、证书文件和 Admin API 都调用它，并在日志器的输出
  或格式改变时提示一次（“log's outputs and formats take a restart; the running ones stay”）；访问日志的文件自 I81 起已就地
  采用新的切割设置和时区。
- **Admin API**：`GET /logs`（`{"outputs": [...]}`：`log` 和访问日志的每个文件输出，其日志器、格式、时区、填入默认值的
  `rotate`、当前文件和切割出的文件及其大小和时间、总量、丢弃的写入和最后的错误；每个输出最多 1,000 个文件，超出时写
  `"truncated"`），`POST /logs/rotate`（`{"path"}`）和 `POST /logs/cleanup`（`{"path", "dry_run"}`，应答
  `{"removed": [...]}`），名字和内容与 fit 相同；不是运行中配置的文件输出的路径答 400，所以不会列出或删除别的东西。每个端点都在
  API 引擎的逻辑池上执行（akio-http 的 `detach_to_pool`），从不在它的 I/O 线程上。`Options::logs` 把 akit 的 `Files` 交给
  服务器。这四个端点在 `tests/compat/v1/additions.txt` 中标为 v1 新增。时间用 `akit_observe::time::rfc3339_nano` 写。
- **测试**：akit-admin 的 `logs` 单元测试（请求内容；在 1,000 个文件处截断的列表及其总量）；带新增项的
  `v1_surface_is_frozen`；akit 的 `log` 端到端测试（`reload_changes_levels_zone_and_rotation_in_place`：从 UTC 改为东八区的
  本地时区，akio 的行改为 debug，与访问日志共用的文件改为 300 字节切割，再改格式并得到提示；
  `the_admin_api_lists_rotates_and_cleans_up`：列表、立即切割、什么也不删的 dry run 以及删除所列文件的清理、拒绝非配置中的路径）。
- **本路线的测试运行**：macOS 上 `make check`（fmt、clippy、整个工作区、deny、文档、门禁和 v1 冻结）；第一次运行中
  akit-listener 的 `udp_sessions_expire` 失败一次（机器负载下会话的空闲计时，本路线未触及的测试），单独运行 5 次全过，随后整个
  工作区以 `--no-fail-fast` 全部通过。gpu51 上 clippy，以及 epoll 和 io_uring（`--test-threads=4`）下的整个工作区，每个测试
  二进制都经由 `tools/test-timeout.sh`：全部通过，没有挂起。
- **文档**：Admin API 指南和兼容性页面列出 `/logs`；从 fit、nginx 和 Caddy 迁移的指南不再说 `log` 和 `access_log.rotate`
  尚不支持。
- **与计划的不同**：fit 工作台的“日志”页面（I157）按计划等控制台路线；它的端点在这里完成。应答中的时间为 UTC（fit 用其进程的
  时区）。本路线用三次迭代完成；I84 未使用。
- **下一步**：访问控制列表路线、安全审查、I80b。

### 2026-11-26：I82，akit 自身的日志：fit 的 `log`

- **配置**：按 fit 的 `config.Log` 读取 `log`（`level`、`levels`、`output`、`format`、`time_zone`、`rotate`，以及带自己的
  `level`、`output`、`format`、`rotate` 的 `loggers`），`level_of`、`output_of`、`format_of`、`rotate_of` 和
  `file_outputs` 与 fit 相同；按 fit 的检查和消息：未知日志器、不是级别的级别、日志器的级别也写在 `levels` 中、`rotate` 的检查、
  以不同切割设置共用文件的日志器（按 fit 的顺序及其特点：写日志主文件的日志器必须重复它的 `rotate`），以及访问日志以不同 `rotate`
  或时区共用日志的文件（`sharing_problems`）；`log.time_zone` 与访问日志的一样加载。`LOGGERS` 为 fit 的十四个名字。
- **akit-observe 的 `logger`**：`Root`，每个日志器各有级别、格式和输出，文件经由 I81 的 `Files`（每个路径一个写入端，与访问日志
  共用）；fib 的 `line` 以及 log/slog 的 `text` 和 `json`（RFC 3339 带纳秒，数字不加引号），时间按时区；默认 debug 和 info 写
  标准输出、其余写标准错误；`install`、`log`、`enabled`、`lines`（给各 crate 的回调）以及 `error!`、`warn!`、`info!`、
  `debug!` 宏；`install` 之前，在标准错误上写 `akit: <消息>`。`bridge_akio` 安装 akio 的日志器：akio 的记录属于 `engine`，
  warn 和 error 照原级别，info 和 debug 按 debug。
- **消息**：ACME、OCSP 和共享存储（`tls`），Docker、Kubernetes、Consul 和 etcd（`provider`），OTLP 导出（`telemetry`），
  FastCGI 的 stderr（`fastcgi`），访问日志的丢弃（`access_log`），文件写入端和 sockmap 中继（`engine`），指标的溢出以及没有
  `rotate` 的文件输出（`server`）；akit-tls、akit-provider、akit-handler 和 akit-l4 因此依赖 akit-observe。`akit run` 在访问
  日志之前创建日志（访问日志共用它的文件），并在所有退出路径上最后关闭它。
- **测试**：akit-observe 的 `logger::tests`（两种时区下的三种格式、转义的行、属性；级别和日志器自己的文件；与访问日志共用的写入
  端）；akit-config 的 `log`（默认值和每个日志器取到的设置、每项检查、共用的文件、fit 的 `logging` 示例）；akit 的 `log` 端到端
  测试（默认以 `line` 在标准错误上写警告；日志和访问日志写同一个文件，JSON、UTC，每行完整；`akit validate` 拒绝未知时区）。
- **门禁与文档**：`log` 的字段加入 `supported.txt` 并标为 v1 新增；fit-examples 的 `logging` 通过（现在 80 个中 49 个有效）；
  门禁对参考文档的检查把映射的值（`log.loggers.*`）算作对象；配置参考的“日志”一章和表格，两种语言，并附关于格式、级别中的
  `engine` 与各 crate 消息以及时区的说明。
- **与计划的不同**：akio 的 info 记录按引擎的 debug 写，各 crate 回调的行不带级别，按 warn 写（决策 033 第 8、9 点）；akit 命令
  本身的输出行保持原样，包括 fit 经由日志器写的那几行（第 10 点）。fit 的 `plugin` 日志器暂无消息。
- **下一步**：I83，重载时的日志文件以及 Admin API 的 `/logs`。

### 2026-11-26：I81，日志文件：按大小和时间切割、保留、压缩

- **计划**：日志路线在两份计划中拆成 I81–I83（对应 fit 的 I155–I157；fit 工作台的“日志”页面等控制台路线，它的 Admin API
  在 I83），并在索引中加行，在基准表中加入 I81 一行。[决策 033](decisions/033-log-files.zh-CN.md)。
- **akit-observe 的 `files`**：`Files`，每个清理过的路径一个写入端，并计数持有它的日志（`LogFile`，drop 时释放）；1 MiB 或
  256 个缓冲的队列，超出时 `write` 和 `write_owned` 拒绝（`Refused::Full`，并计数），由写入端自己的线程写盘。按 `max_bytes`
  和从某时区午夜起的 `interval` 周期切割（`period_start`，即 fit 的 `periodStart`，在 UTC 和固定偏移下测试一小时、一刻钟和
  两天）；切割出的文件名用 UTC 并加 `-N`；切割失败一分钟后重试；打开时、切割后和每十分钟按时间、数量和总量从最旧的删起，只删
  写入端生成的文件名；用 akio-deflate 在第一次需要时启动的压缩线程上 gzip（队列 16，最多等一分钟），临时文件以独占方式创建；
  为 I83 提供 `rotate_now`、`cleanup_now(dry_run)`、`sync` 和 `info`。文件只在是只有一个链接的普通文件、且就是该名字原先
  指向的文件时才打开（akio-sys 没有 `O_NOFOLLOW`：决策 033 第 4 点），用 `O_NONBLOCK` 防 FIFO；新文件用 `O_EXCL` 创建；
  目录可被他人写入时给出警告。为固定偏移加入 `Zone::fixed`。
- **访问日志**：`AccessLog::to_file` 把每次刷出的缓冲不复制地写入写入端，被拒绝的缓冲按行计为丢弃。`akit run` 通过 `Files`
  打开文件输出，只在没有 `rotate` 时提示，重载就地改变文件的切割设置和时区（`output`、`format`、`flush_interval`、
  `buffer_bytes` 仍需重启）。
- **配置**：`Rotate`（fit 的指针字段写成 `Option`）和 `Rotation`（填入默认值，即 fit 的 `Rotate.Equal`），按 fit 的检查读取
  `access_log.rotate`（负的限制、小于 1s 或超过 24h 又不是整天数的 interval）；为 I82 的检查提供 `is_file_output`、
  `output_path`（Go 的 `filepath.Clean`）和 `zone_name`。
- **测试**：akit-observe 的 `files::tests`（按大小、大于 `max_bytes` 的行、在 UTC-4 午夜以及时区改变时切换周期、
  `period_start`、按数量、总量和时间保留并含 dry run、gzip、每个路径一个写入端且拒绝其他设置、拒绝符号链接和硬链接并跳过切割名处的
  链接、队列满时计数、文件名）；akit-config 的 `access_log`（`rotate` 的默认值、字段、`.conf` 的大小和错误）；akit 的
  `access_log` 端到端测试（`a_rotated_file_keeps_every_line`：60 个请求写进切割并压缩的文件，每行只出现在一个文件里；
  `reload_changes_the_retention`：SIGHUP 把 `max_files` 从 100 改为 2）。
- **门禁与文档**：`access_log.rotate` 及其字段加入 `supported.txt`，标为 v1 新增（`make v1-bless`）；配置参考的访问日志一章
  （切割、命名、保留和一个示例）以及 `access_log.rotate` 的表，两种语言；`tools/config-reference.py` 保留 fit 描述中的动词
  （“until they fit”）。
- **与计划的不同**：无。写入端关闭时最多等 30 秒让其文件的压缩完成，所以 `akit run` 停止后不留临时文件（fit 不等它的池）。
- **下一步**：I82，akit 自身的日志。

### 2026-11-26：I87，访问控制列表 III：Admin API 与配置参考

- **`admin.access`** 按 fit 的检查读取，Admin API 在 Unix 套接字上时按 fit 的措辞拒绝它。运行时图编译 API 的策略，即 `admin.access`
  然后全局访问列表（`Graph::admin_access`），它的文件与其余文件一起被监视；没有 `admin` 的配置，其全局访问列表仍适用于 akit 启动时的
  API，与 fit 相同。
- **在 accept 时决定，并随每次应用的配置生效**：`akit run` 用该策略构建 API 的监听器（`Options::access`，akio 的过滤器），并在每次
  应用配置后（经 Admin API、SIGHUP、`-watch`、provider 或被监视的文件）重新设置（`Server::set_access`），尽管 `admin` 的其余部分需要重启；
  只改 `admin.access` 不会被警告为需要重启（`admin_warning`）。Unix 套接字的对端从不检查。见决定 034 第 11 点。
- **文档**：配置参考中访问控制列表一章涵盖 Admin API（附 fit 的建议：在 `admin.access` 中放行 `127.0.0.1` 和 `::1`），Admin API 一章
  和指南也写明这一点并附示例；迁移指南不再把 `access` 列为不支持；路线图中的访问控制路线已完成。威胁模型的 L6 由 I80b 更新。
- **兼容性门禁**：读取 5 个字段（`admin.access` 及其四个字段），用 `make v1-bless` 标记。fit 的 `access` 字段中，各角色及其控制台的
  （`admin.console.access`、`llm`、`tunnel`、`webrtc`、`streaming`、`bittorrent`）随各自的路线实现，目前随这些部分整体被拒绝。

测试（移植自 fit）：akit 的端到端测试 `admin_access.rs`（`TestAdminAccess`：`admin.access` 拒绝、全局访问列表生效、`admin.access`
的 allow 保留本地访问，分别经 Admin API 的加载和 SIGHUP；`TestAdminAccessFile`：拒绝文件被监视，变化时被应用；
`TestAdminAccessUnixSocket`：Unix 套接字上的错误，以及其对端从不被拒绝）；akit-admin 的 `admin_access_takes_no_restart`；门禁的样例；
配置参考和 Admin API 指南中的示例。

I85–I87 的检查：macOS 上 `make check`（整个分支）；基准主机上 clippy，以及 epoll 和 io_uring（`--test-threads=4`）上的整个工作区，
每个测试二进制都经 `tools/test-timeout.sh` 运行：两个后端各 153 个测试二进制全部通过，没有失败或挂起。

### 2026-11-26：I86，访问控制列表 II：服务与正向代理

- **`services[].access`** 按 fit 的检查读取；运行时图按 fit 的 `compileAccess` 编译每个监听器上各服务的策略（服务自己的列表、
  监听器的、全局的）（[决定 034](decisions/034-access-lists.zh-CN.md) 第 8–10 点）。
- **在 accept 时**：只有一个不带 `server_names` 的服务、独占端口且不接受 PROXY 头的 `tcp` 监听器，按该服务的策略，与 I85 一样通过
  akio 的过滤器。
- **之后，在 akit-l4 的入口中**（`Entry::with_access`、`Entry::admits`）：服务按服务器名挑选、与 HTTP 或 SOCKS 共用端口、或接受
  PROXY 头的监听器，在知道服务后（读完 PROXY 头和 ClientHello 之后）检查每个连接，被拒绝的连接在拨号上游之前关闭。这也补上了 I85
  留下的空缺：接受 PROXY 头的 `tcp` 监听器现在遵循访问控制。
- **权宜实现（akio 需求 A220）**：来自被拒绝客户端的 `udp` 会话在打开时被关闭，数据报被丢弃；之后的每个数据报再打开并关闭一个会话，
  不记住该对端。
- **SOCKS**：监听器之后才决定时，监听器的 `socks` 在每个客户端发出请求时向当前运行时图询问所遵循的策略（`Rules::socks_gated`，
  即 fit 的 `SOCKSAccess`）。
- **文档**：配置参考的这一章涵盖服务和 SOCKS，示例中加入一个服务，并为 `services[].access` 加了说明；扩展了决定 034。
- **兼容性门禁**：读取 5 个字段（`services[].access` 及其四个字段），用 `make v1-bless` 标记。

测试（移植自 fit）：akit-listener 的 `access_l4.rs`（`TestAccessService`：一个 `tcp` 监听器和一个与 HTTP 共用的监听器，服务的列表
双向覆盖全局列表，HTTP 一侧回答 403 或 200；按服务器名挑选、带自己列表的服务；接受 PROXY 头的 `tcp` 监听器拒绝头中指明的地址；
`udp` 服务的拒绝与放行；与 HTTP 共用端口的 SOCKS 的拒绝与放行）；akit-config 的 `access.rs` 和 `services.rs`；门禁的样例；
配置参考的示例。macOS 上 `make check`；Linux 上的运行见 I87 的记录。

### 2026-11-26：I85，访问控制列表 I：列表、监听器和路由

- **akit-access**，一个独立的 crate（[决定 034](decisions/034-access-lists.zh-CN.md)），对应 fit 的 `access` 包：把内联条目和
  列表文件（地址和前缀、区间、级别不超过 127 的 eMule `ipfilter.dat` 行、PeerGuardian `.p2p` 行，每个文件最多 64 MiB、每行最多
  16 KiB，坏行在错误中写明文件和行号）编译成按地址族排序并合并的区间，用二分查找；`Policy::chain` 把适用的列表按最具体在前组合起来，
  每个列表按 fit 的方式作决定或不表态。
- **配置**：顶层（只能由一个文件给出）、监听器和路由上的 `access`（`akit_config::Access`），每个条目按 fit 的路由器解析方式检查，
  每个文件路径不能为空；`.conf` 和 YAML 的读法与 JSON 相同。
- **运行时图**按 fit 的 `compileAccess` 编译每个监听器的访问控制（`akit_runtime::access::ListenerAccess`）：监听器自己的列表和全局列表
  就是全部、且没有 PROXY 头指明客户端时，在 accept 时决定，否则逐请求决定。列表文件在此时读取（读不了的文件使配置失败，`validate`
  也是如此），它们的摘要加入 `cert_files`，因此文件变化时 `run -watch-certs` 会重新应用配置。
- **监听器**：akio 的 accept 过滤器（A3）在构建监听器的 TCP 一侧时装上，每次重载在保留的监听器上替换（`Listeners::gate`），不重新绑定。
  路由自己的列表、或接受 PROXY 头的监听器拒绝的请求，在其处理器运行之前回答 fit 的 `403 forbidden`，按其路由记录日志和计数，
  请求体不被缓冲；路径没有规范形式的请求在回答 400 之前先被拒绝，与 fit 相同。
- **权宜实现（akio 需求 A220）**：akio 的 UDP 监听器不支持 accept 过滤器，因此 HTTP/3 请求逐个按 TCP 一侧在 accept 时的策略检查，
  回答 403，而 fit 会丢弃被拒绝对端的数据报（`Dispatcher::unfiltered`）。在 I86 之前，`udp` 监听器和接受 PROXY 头的 `tcp`
  监听器不受访问控制约束，`services[].access` 作为尚不支持被拒绝。
- **文档**：配置参考中关于访问控制列表的一章（两种语言），附一个由 `akit validate` 检查的示例（示例沙箱中加入 `lists/deny.txt`），
  以及 `listeners[].access` 的说明；A220；决定 034；工作区结构和架构文档中加入 akit-access。
- **兼容性门禁**：读取 15 个字段（`access`、`listeners[].access`、`routes[].access` 及其各自的四个字段），用 `make v1-bless` 标记；
  门禁的结果不变（没有夹具设置 `access`：fit 的模板中它是注释掉的）。

测试（移植自 fit）：akit-access 的单元测试（`TestPolicyLayers`、`TestChainEmpty`、`TestDenyBeatsAllowInOneList`、`TestEdges`、
`TestSetAgainstLinear`、`TestFiles`、`TestCompileErrors`、`TestRangeOf`、`TestParseLine`、`TestP2PAndDATFiles`、`TestRangesAgainstLinear`，
以及一个含 100,000 个前缀的列表）；akit-config 的 `access.rs`；akit-listener 的 `access.rs`（`TestAccessRefusedAtAccept`、
`TestAccessRoute`、`TestAccessFollowsReload`、`TestAccessProxyProtocol`、列表文件被监视及读取失败、HTTP/3 逐请求拒绝）；门禁的样例；
配置参考的示例。macOS 上 `make check`；Linux 上的运行（epoll、io_uring）见 I87 的记录，它运行了整个分支。

### 2026-11-26：akio 锁定到 `6bcd8ad6`：尚未完成的请求、I79b 与 A169/A171 开销修复

- akio：akio main 上的 `6bcd8ad6`，合并了尚未完成的请求的 akio 分支（`claude/akit-open-requests`：
  A12、A22、A24、A29、A30、A35、A45、A49、A54、A75、A78、A79、A117、A148、A170）、I79b 的分支
  （`claude/i79b-akio`：A168、A214、A215），以及去掉 A169 和 A171 开销的分支（akio 的 ADR-176；akio
  的 main 当天也采用了同样的 HTTP/2 默认值，并把 `run_on_pool` 改名为 `handlers_on_logical_pool`）。
  akit 的 `rev` 和 `akit_runtime::AKIO_REV` 移到这个提交；[akio API 请求](akio-api-requests.zh-CN.md)
  中除 A180 和 A200 外的所有请求都已进入 akio 的 main 并锁定。只能针对 akio 分支构建的分支，现在针对锁定的提交构建。
- 决定：[待审决定](decisions-pending.zh-CN.md)第 11 行已更新（A/B 已完成，开销已去掉），第 13 行记录本次合并。
- 测试：macOS 上 `make check`；gpu51 上 clippy，以及 epoll 和 io_uring 下的整个工作区。完整运行中
  `sni_passthrough_spliced`（`sockmap`，两个后端）和 io_uring 下 FastCGI 的 `cut_connections` 各失败一次；
  单独运行时分别在每个后端 5 次全过、3 次全过：是共享主机负载下的时序问题，与合并无关。
- 开发计划的基准表删掉了一次早先合并留下的缓存、FastCGI 和 WASM 旧行副本；状态表“可用功能”一行，中文删掉了一段重复，英文补上了缺的分隔符。

### 2026-11-26：I79b，网关示例的最后几个字段

- **先做 akio**（akio 分支 `claude/i79b-akio`，从 akio 的 main `1b85fd83` 切出，尚未合并或锁定；akit 的 `Cargo.toml`
  仍锁定 `0fc8f3a7`）：A214 `Config::max_pending_bytes`（akio 的 ADR-172：设置预算时，一个监视线程每 1–10 毫秒汇总
  各 I/O 线程的排队字节 gauge；连接通过每个后端都会询问的 `ConnInner::read_held` 暂停，并经由各自的事件循环唤醒；
  `budget_pauses`、`pending_bytes`），A215 `Config::max_accepted_connections`（ADR-173，基于 A30 的
  `Telemetry::open_accepted` 计数器；该分支按 `claude/akit-open-requests` 上的原样引入这些改动，便于两者合并），A168
  `HttpConfig::http1`（ADR-174：`false` 时只以先验知识提供 HTTP/2，其他内容收到 `GOAWAY`）。各后端都有测试；accept
  和读路径的同轮 A/B 记在 akio 的 docs/BENCHMARKS 中。
- **`limits.max_pending_bytes`** 像 fit 一样读取（字节数，或 `"25%"` 这样的内存份额，默认内存的一半；错误信息与 fit
  相同），即 `akit_config::Memory`，启动时映射到 akio 的预算上；重载改变它时给出警告。份额所对应的内存由 akit 像 fit 的
  `sysmem` 那样读取（Linux 上读 `/proc/meminfo` 和 cgroup 的上限；其他系统用 fit 的回退值 256 MiB），见
  [决定 032](decisions/032-limits-on-akio-budget.zh-CN.md)。`/metrics` 的 `fit_reads_paused_total{reason="budget"}`
  即 akio 的计数。
- **`limits.max_conns`** 只统计客户端连接：用 akio 的 `max_accepted_connections` 代替 `max_connections`，akit 拨出的
  上游连接不再占用客户端的名额。
- **只有 `h2c` 的监听器**（`protocols: [h2c]` 或 `[h2c, tcp]`）：与 fit 相同，只提供先验知识的 HTTP/2；在共享端口上，
  它的 HTTP 规则只匹配 preface。
- **门禁**：之前 80 个夹具，45 个有效，34 个只因暂不支持被拒绝（26 个字段）；之后 48 个有效，31 个被拒绝（24 个字段）：
  `grpc`、`limits` 和 `proxy` 有效，只有 After-1.0 路线的示例被拒绝（`bittorrent`、`console`、`llm-*`、`logging`、
  `media-pages`、`streaming`、`sync-*`、`tunnel-*`、`video-site`、`webrtc-*`、`whip-whep`）。`limits.max_pending_bytes`
  已记入 `tests/compat/v1/additions.txt`（`make v1-bless`）；配置参考中 `max_conns` 的说明已删除，`max_pending_bytes` 和
  `protocols` 有了说明。

测试（移植自 fit）：akit-config 的 `limits.rs`（`TestMemory`、`TestLimits`）、`json_config.rs`（只有 h2c，以及与 `tcp`
共存）、`v1.rs`（`.conf` 写的 fit-examples `limits`）；akit-listener 的 `limits.rs`（`TestPendingBytesBudget`；引擎设置）、
`listeners.rs`（`TestProtocols`）、`reload.rs`（重载新增只有 h2c 的监听器）、`one_port.rs`（只有 h2c 与 `tcp` 共存）、`sysmem`（单元测试）；akit-proxy 的
`proxy.rs`（上游连接打开时的 `max_conns`）、`grpc.rs`（只有 h2c 的监听器上的调用）；akit-compat 的 `gate.rs`（该字段的样例）；akit 的 `admin.rs`。在 macOS 和基准
主机上检查，akio 的测试与 akit 的 workspace 在 epoll 与 io_uring 上都跑过。akio 的同轮 A/B：HTTP/1.1、pipelined、h2c 与
TCP echo 在两项设置关闭与开启时都与 akio 的 main 相差 1% 以内；其中 `connect` 用例受客户端在回环上复用 TIME_WAIT 端口的
限制，测不到 accept 路径。`akit` 的 `watch` 测试（`each_edit_is_applied`）先等 stdout 的一行再等 stderr 的一行，负载重的
主机上两者可能交换顺序；现在接受任意顺序。

与计划的不同：范围内没有。akit 不做基准（规则 30）：[基准表](dev-plan.zh-CN.md#基准与优化状态)中有 I79b 的一行。

### 2026-11-26：I80，为安全审查做好准备

- akio：未变，`0fc8f3a7`。除决定 030 外，没有占用 akio API 需求、ADR 或决定编号。
- **[威胁模型](security/threat-model.zh-CN.md)**及其英文版：本模型中的 akit（一个进程，数据面在 akio 的 I/O 线程上，控制面在其之外，
  akio 是发布二进制唯一的依赖）；资产（密钥、ACME 账户密钥、Admin API 的令牌、akit 持有的凭据、配置、流量、路由决定、可用性、主机）；
  参与者与信任边界（客户端、上游、操作者、本机进程、网页、提供者和存储、akit 调用的服务、插件）；假设（配置和主机是受信任的）；然后逐个
  边界列出每种攻击及 akit 的控制措施，写明代码和测试（列出的每个测试都核实过存在）；以及 **15 项已知局限**（L1–L15），每项附计划：
  属于 akit 的在 I80b，控制台和访问列表路线，akio A24 和 A214，以及 akio 自己的外部审查（L15：akio 的 TLS、X.509、密码算法和 QUIC
  经过了 akio 的内部审查，没有经过外部审查）。
- **[审查范围](security/review-scope.zh-CN.md)**及其英文版：审查对象（akit 的提交、akio 的固定提交、平台、配置）、问题、范围之内的
  各领域及其代码和首先要问的问题（TLS 处理，包括 akit 所用的 akio-tls、证书、ACME、按需 TLS 和 OCSP；Admin API 与仪表盘；插件沙箱；
  请求解析与代理的头处理；eBPF sockmap 路径；时间允许时的提供者与共享存储），按领域列出的 I77 的 35 个模糊测试目标，范围之外的内容
  （akio 的内部、akit 尚未实现的部分、操作者自己的输入、超出限制的拒绝服务、不随发布交付的东西），关于 akio 及我们建议的审查顺序的
  一节，如何构建和运行 akit，以及如何报告发现。
- **[SECURITY.zh-CN.md](../SECURITY.zh-CN.md)**及其英文版，参照 akio 的：支持的版本（1.0 之前是最新提交；从 1.0 起是最新次版本，以及
  在 3 个月内为前一个次版本提供高危和严重级别的修复）、经 GitHub 私下报告、响应目标、协调披露、范围和已有的加固。两个 README 都链接它。
- **供应链**（[决定 030](decisions/030-supply-chain-checks.zh-CN.md)）：`cargo deny check` 无问题（它唯一的警告是允许了 Apache-2.0 却
  没有 crate 匹配，在 `deny.toml` 中以 `unused-allowed-license = "allow"` 及其理由处理），`cargo audit -D warnings` 无问题（锁文件中
  74 个 crate，没有安全公告，没有忽略任何一项）。新的工作流 `.github/workflows/security.yml` 在每次推送到 `main`、每个拉取请求和每天
  运行两者，以及 `tools/release-deps.py`：它遍历发布二进制的依赖图（normal 和 build 边，除 `cfg(loom)` 外的每个目标平台的 `cfg`），
  遇到没有决定的注册表 crate 即失败。I80 时二进制的依赖图有 40 个包，都属于 akit 和 akio，没有来自 crates.io 的；锁文件中的 30 个
  注册表 crate 是开发依赖和 akio 的 loom 模型。
- **发现并修复了一个缺陷**：使用 `https://` ask URL 的按需 TLS 从未工作过。它的客户端用 `ClientConfig::builder()` 构建，而 akio-tls 的
  这个构建器起始时没有任何 CA，因此每次询问的握手都失败，每个按需名字都被拒绝（失败时是关闭的）。`OnDemand::new` 现在给它系统的 CA，
  在编译配置时读取（`RootCerts::system`），从不在 I/O 线程上；`OnDemand::with_roots` 从调用方获取 CA。新测试 `https_ask` 用测试 CA
  经 HTTPS 提供 ask URL：给了该 CA 时得到应答，没有 CA 时被拒绝，`http://` 的 ask URL 不读取 CA；没有修复时测试失败。
- 调查中发现、在 I80 中记为局限而未修改的（I80b）：Admin API 读取配置时不脱敏；不检查密钥文件的权限；允许明文的控制面传输；正向代理的
  私有地址段可达且没有认证（与 fit 相同）；指向静态根目录之外的符号链接；不检查客户端证书的吊销；没有 WASM 模块摘要；没有测试令牌之后的
  `POST /cache/purge`。
- 基准测试：无（文档、CI 和控制面的修复；规则 24 的“无”）。
- 测试：`cargo test -p akit-tls`（包括 `https_ask` 在内的按需测试）；在 macOS 上运行 fmt、clippy、`cargo deny check`、
  `cargo audit -D warnings`、`tools/release-deps.py` 和 `scripts/check-docs.sh`。

### 2026-11-26：实现 akio 需求 A78、A22、A170、A24、A49、A35、A148，去掉相应的临时方案

- akio：已在 akio 分支 `claude/akit-open-requests` 上实现（`22bc4402`；ADR-155、ADR-156、ADR-157），尚未进入
  `main`。akit 已使用；`Cargo.toml` 中的 `rev` 在该分支进入 akio 的 main 之前仍是 akio main 的 `0fc8f3a7`，所以本次提交只能对着
  该分支构建（由协调者移动锁定）。akio 的一项破坏性变更：`akio_http::BodyEvent` 新增 `TooLarge`（该枚举是穷尽的），
  akit 的每个请求体 sink 现在都匹配它（代理、WebSocket 隧道、FastCGI 处理器、Admin API 和测试中的 sink）。
- A24：带 `Connection: close` 的 HTTP/1 响应发送后关闭连接，暂停读取，排在它后面的流水线请求不再处理。Admin API
  的 401（`guard`）和 `read_body` 的 413 都带它，与 fit 的 Admin API 关闭这类连接相同；A24 的注释已去掉。
- A49：`Context::limit_body` 为读取请求体的任何一方限制请求体。akit-listener 对每个流式请求体调用
  `limit_body(max_body_bytes)`（CONNECT 的除外：隧道或扩展 CONNECT 的流不是请求体），取代局部值 `MAX_BODY_BYTES`；
  该局部值连同它的所有读取方（代理、FastCGI 处理器、缓存自己的请求）已从 akit-runtime 中去掉。`body_limit` 对每个
  请求调用 `limit_body(max_bytes)`；声明长度超限或缓冲后超限的请求体仍由中间件用 fit 的措辞应答（akio 自己的 413 写的
  是 "Payload Too Large"），流式时带 `Connection: close`，不再读取其余部分。代理收到 `TooLarge`：响应尚未开始时
  应答 413，无论如何都放弃上游请求（取消它，这会截断已开始的响应，也不算地址的失败）；FastCGI 处理器同样以 413 结束
  其交换。
- A35：流式 HTTP/1.1 响应保留 `Content-Length` 并按它分帧：代理在所有版本上都保留上游给出的长度（带尾部字段的响应在
  HTTP/1.1 上仍分块发送，只有分块才能携带尾部字段），FastCGI 处理器保留应用给出的长度（不是数字的长度会被去掉）。
  被截断的响应体会在不足长度时关闭连接。
- A148：FastCGI 处理器的 `SERVER_ADDR` 和 `SERVER_PORT` 是请求到达的地址（`Request::local`），与 fit 取自 TCP
  连接的相同；没有该地址时（Unix 域套接字，端口为 0）照旧。在 HTTP/3 上是 UDP 套接字的地址，而 fit 此时
  `SERVER_ADDR` 为空。
- A78：akit-l4 给每个中继的两个连接、每个 UDP 会话的两侧设置服务的空闲超时，并把它们发出的数据也算作活动
  （`Conn::set_idle_timeout`、`Conn::set_idle_counts_writes`），由 akio 在各自的 I/O 线程上关闭它们。在 akio 定时器
  上扫描所有连接的清扫已去掉（每个数据块上的 `Relay::touch` 也随之去掉）；只为已拼接的连接保留，akio 看不到它们的
  字节（它们在 akio 中的空闲超时被关闭）。akit-listener 不再把引擎的 UDP 空闲超时提高到各服务的值（`udp_idle`，
  启动时和重载时）：它只用于 HTTP/3 的对端。
- A22：重载可以移除和修改服务 h3 或 `udp` 的监听器，包括 akit 启动时的：`ReloadError::Udp` 已去掉。引擎自己的 UDP
  监听器用 `Engine::shutdown_listener` 排空，重载绑定的套接字用 `UdpBinding::shutdown` 排空（不再连同其连接一起
  关闭）：新对端被丢弃，HTTP/3 连接收到 `GOAWAY` 并完成其请求。被同一地址上的监听器替换的最多排空
  `UDP_REPLACED_DRAIN`（2 秒），因为这期间内核散列到它的套接字上的新对端得不到应答；没有替代者的被移除监听器排空整个
  排空超时（[待审决定](decisions-pending.zh-CN.md)第 12 行）。
- A170：akit-l4 的 sockmap 用 `Relay::tunnel` 拼接（首次写入中它没写出的部分经引擎发送），并用 `Tunnel` 跟踪拼接：
  `moved` 用于空闲超时，`close_after_forward` 用于一侧结束（在 akio-ebpf 的排空线程上），以及 `late_payload`。akit 的
  `Spliced`、它的计数、它在 akio 定时器上的排空以及 `Conn::inner` 都已去掉。以监听器读取的 PROXY 头开始的连接也会被
  拼接：隧道的基准计数包含了用户态读取的字节。[决策 010](decisions/010-sockmap-relay.zh-CN.md) 已更新。
- 测试：akit-admin 的 `token`（排在 401 后面的流水线请求不被处理）与 `loads_refused`（413 之后关闭）；
  akit-middleware 的 `body_limit_h2`（h2c：低于、等于和超过限制，声明长度和逐渐增长，发给缓冲请求体的处理器和代理）
  与 `body_limit_closes_http1`；akit-handler 的 `streamed_body_limit` 与 `server_address`（以及 `params` 中的
  `SERVER_ADDR`、`SERVER_PORT`）；akit-proxy 的 `stream_download` 与 `broken_download`（按长度分帧）；
  akit-listener 的 `tcp_one_way_is_not_idle` 与 `udp_listener_removed`（`l4`），`reload_drains_h3` 与
  `reload_knows_h3_listeners`（`h3`：移除启动时的 h3 监听器，以及在原地址上替换它；进行中的请求得到应答）；
  `client_proxy_header_spliced`（`sockmap`，只有在 Linux 上以 root 运行时才真正拼接）。在 macOS 上对着该 akio 分支
  （命令行传入的 cargo `[patch]`，未提交）运行整个工作区：1138 个通过，1 个忽略；clippy 在 macOS 和
  `x86_64-unknown-linux-gnu`（检查只在 Linux 上编译的代码）上都通过，fmt 通过。`build_with_a_module_and_serve`
  （`crates/akit/tests/build.rs`）失败，因为它另建一个包、对着锁定的 akio 构建，锁定移动后即可通过。
- 待测量（基准任务，规则 30）：`tcp` 和 `udp` 服务的吞吐量和连接速率，对比 `dad77a7a`（akio 每个连接的空闲定时器、
  对写入的采样，取代 akit 的清扫和每个数据块一次时钟写入）；HTTP/1.1 上已知长度的代理下载（按长度分帧，没有分块头）；
  带 `body_limit` 的代理上传（由 akio 而不是代理计数）；sockmap 场景（`Tunnel` 的排空线程每 1 ms 轮询，取代 akit 在
  akio 定时器上的退避）；HTTP/3 负载下的重载（不丢请求，被替换监听器的 2 秒排空）。

### 2026-11-26：实现 akio 需求 A12、A79、A45、A117、A30、A54、A29、A75，去掉相应的临时方案

- akio：已在 akio 分支 `claude/akit-open-requests` 上实现（`57a942af`、`22e038cc`、`8caab91c`、`a88c5d7b`；
  ADR-154、ADR-158、ADR-159），尚未进入 `main`。akit 已使用；`Cargo.toml` 中的 `rev` 在该分支进入 akio 的 main
  之前仍是 akio main 的 `0fc8f3a7`，所以本次提交只能对着该分支构建（由协调者移动锁定）。
- A117：akit-cache 的磁盘池改用 `akio::Pool`（从门面取得 `akio::{Pool, PoolConfig}`）；工作区和 akit-cache 不再
  依赖 `akio-pool`。
- A30：`fit_connections_open` 取自 akio 的 `open_accepted`：只计接受的连接，不再把引擎上拨出的上游连接也算进去。
- A12、A79：每次拨号都带自己的连接超时（`Engine::dial_with`；`HappyEyeballs` 拨单个地址时也用它的
  `connect_timeout`），所以 akit-listener 不再把引擎的 `connect_timeout` 设为各上游中最短的那个；超时更长的上游
  不再被截短。akit-l4 用 `dial_with` 和上游的 `connect_timeout` 拨出 TCP 上游的 IP 字面量；探测仍以自己的定时器
  约束整个检查的超时（含解析）。
- A45：删除 akit 自己的定时器线程 `akit_upstream::timer`；原来的调用方改用 `akio::timer::after_send`：健康探测、
  h2 连接池的空闲清扫、缓存的锁与磁盘读取超时、JWK 集的刷新与重新获取、FastCGI 的清扫、WebSocket 隧道的空闲超时、
  四层服务的开场超时与空闲清扫、重载移除的服务的排空，以及 sockmap 的排空（以后可能由 A170 取代）。任务已无意义的
  定时器会被取消（`SendTimer::cancel`）：探测有结果后其超时、服务接走连接或连接关闭后四层的开场超时、获取结束后缓存锁
  的超时、读取有结果后磁盘读取的超时。akit-middleware 与 akit-proxy 为此依赖 `akio`（原先只是开发依赖）。
- A54：被排空的监听器上的空闲 HTTP/1 连接在 akio 的 `HttpConfig::idle_drain_grace`（250 ms，akio 的默认值；fit
  没有这个选项）内收到的请求，以 `Connection: close` 应答：`crates/akit/tests/reload.rs` 把客户端的任何错误都算作
  失败，去掉了重试计数。
- A75：`forward_auth`、`basic_auth`、`jwt` 与 `wasm` 不再为挂起请求而要求完整读取流式消息体：等待期间暂停消息体的
  读取，之后流式交给处理器（`jwt` 中间件的 `may_wait` 随之删除）。对尚未读取的消息体，Proxy-Wasm 过滤器的
  `end_of_stream` 按 fit 的方式由字段得出（`Content-Length` 大于 0 或带 `Transfer-Encoding`）。
- A29：请求的访问日志行与计数由监听器的应答观察者（`HttpAcceptor::observer`、`akio_h3::serve_with_observer`）
  凭请求携带的两个字写出（`Context::set_observed`：图的 ID；指标槽位、路由下标与是否记日志），不分配内存；
  请求被重载越过时，`GraphCell::with_id` 找到它分派时所用的图（若该图已被丢弃：用当前图的日志和注册表，不写路由名）。
  仍用应答过滤器的：被追踪的请求（I64：span 的 ID 与追踪器放不进两个字），以及路由可能改写目标的记日志请求
  （`rewrite`、`wasm`：`Middleware::changes_target`、`CompiledRoute::keeps_target`），其日志行写的是监听器分派时
  的目标（fit 的 `RequestURI`），照旧复制一份。观察者看到的是经过所有过滤器、正要发出的应答。认证出的 `user` 仍不记入
  日志（两个字装不下名字）。
- 测试：akit-listener 的 `upstreams` 中的 `connect_timeout`（单个地址受 300 ms 约束，而引擎的是 10 s；另一个上游的
  1.5 s 不被截短）；akit-middleware 的 `forward_auth` 中的 `forward_auth_streams_the_body`（1 MiB 经
  `forward_auth` 流式到达代理的上游，完整且有序，上游在后一半发出前就已收到请求；用原来的整体读取会失败）；去掉重试的
  重载测试；访问日志、指标与追踪的测试覆盖两种路径。在 macOS 上对着该 akio 分支（命令行传入的 cargo `[patch]`，
  未提交）运行整个工作区：1131 个通过，1 个忽略；`build_with_a_module_and_serve`（`crates/akit/tests/build.rs`）失败，
  因为它另建一个包、对着锁定的 akio 构建，而那里还没有这些 API，锁定移动后即可通过。fmt 与 clippy 通过。
- 待测量（基准任务，规则 30）：开启访问日志、开启 `metrics` 时的 proxy 与 static 场景，对比 `dad77a7a`（观察者取代
  每个请求一个装箱的过滤器）；跨越重载的负载（观察者查找被越过的图时要取单元的锁）。

### 2026-11-26：I77，Fuzz

- akio：未变，`dad77a7a`。一个请求 **A200**（按 RFC 9112 的要求处理 absolute-form 请求目标），只提出未实现；下面
  akit-listener 的改写是它的临时方案。
- `fuzz/`（cargo-fuzz，自成一个 workspace，nightly）：**35 个目标**，akit 自己的每个解析器至少一个，每个调用所在 crate
  的 `fuzzing` 模块（feature `fuzzing`，`#[doc(hidden)]`，不进发布构建）。和 akio 一样，每个 crate 的 `fuzzing::tests`
  在 stable 上重放 `fuzz/corpus/<target>`，所以 `cargo test` 在每个平台上检查每个种子和每个曾经失败的输入。
  - 配置读取器（fit 的 `FuzzLoad`、`FuzzAdapt`、`FuzzConvert`）：`config_json`、`config_conf`、`config_yaml`、
    `config_value`（I02–I26 的目标，改为调用 crate 的模块）、`convert_nginx` 和 `convert_caddyfile`（输出 JSON，警告注明
    行号，读取时不 panic）、`config_adapt`（`akit adapt`：以每种格式写出的配置读回同一个配置）。
  - 路由器（`FuzzParseExpr`）：`router_expr`（表达式打印回的表达式解析为同一个表达式、匹配同样的请求）、`router_path`
    （规范路径是不动点，没有点段或空段）、`router_find`（路由器对照逐条尝试每个路由的参考实现）。
  - 代理（`FuzzHopByHop`、`FuzzForwarded`、`FuzzGRPCTimeout`）：`proxy_headers`（两个方向都不留逐跳字段或 `Connection`
    点名的字段，转发字段是代理自己的，`Forwarded` 元素没有控制字符，不断行）、`proxy_grpc_timeout`。
  - 缓存（`FuzzDirectives`）：`cache_semantics`（指令、`Vary`、标签、新鲜度、前置条件、失效的位置、purge 键）、
    `cache_disk`（磁盘层文件的读写两个方向）。
  - L4（`FuzzReadSNI`）：`l4_hello`（无论怎样切分，ClientHello 选中的服务相同）、`l4_forward`（正向代理的规则：内部地址
    或本机地址除非 `allow` 包含，否则不放行）。PROXY protocol 和 ClientHello 的解析器属于 akio-mux，在 akio 中 fuzz
    （`proxy_header`、`mux_tls_hello`）。
  - 处理器（`FuzzReader`）：`fastcgi_response`（整体或分段读取得到相同的记录、CGI 头）、`fastcgi_paths`
    （`SCRIPT_FILENAME` 和 `PATH_TRANSLATED` 在根目录下）、`static_path`（静态处理器对原始路径的扫描与对解码后路径的扫描
    一致；CONNECT 目标）。
  - 中间件：`jwt_token`（各处取到的 token，以及用测试密钥签名、因此会被检查的 claims）、`jwks_keys`（JWK 集合和 PEM
    密钥）、`middleware_fields`（Basic 凭据、限流器的客户端对照 fit 在 `X-Forwarded-For` 上的回溯、`Accept-Encoding`、
    路径转义、任意字节路径的 rewrite）。
  - 以及：`observe_access_log`（一条日志就是一行，JSON 格式时是一个对象）、`observe_escapes`（JSON、Go 和 logfmt 的引号
    转义，`traceparent`）、`observe_tzif`；`tls_redis_reply`、`tls_acme`（存储的证书、ARI 时间）；
    `provider_kubernetes`、`provider_docker`、`provider_consul`、`provider_kv`、`provider_merge`（无论 API 发来什么，
    写出的部分都能被合并读取，合并到配置上后 akit 能读取）；`admin_pointer`（JSON 指针、merge patch）；`upstream_keys`
    （哈希策略和粘性 cookie 选中在线的地址）；`plugin_pairs`（Proxy-Wasm 序列化的头映射）。
- 已提交的种子语料（`fuzz/corpus`）：每个目标手写的种子、fit 自己的 fuzz 语料（`testdata/fuzz`，已解码）、fit 的转换器
  测试文件和 akit 的转换器表、akio 的 `mux_tls_hello` 中的 hello；再加上下面几次运行的发现，用 libFuzzer 的
  `-set_cover_merge=1` 最小化，手写种子和回归种子原样保留：22,682 个文件，12 MB。`fuzz/run-all.sh [SECS] [PAR] [targets]` 运行它们（`merge` 把一次运行的发现并回语料）。
- CI：`ci.yml` 的 fuzz 任务把每个目标运行 60 秒，每次四个；新的 `fuzz.yml` 每周日和发布 tag 时把每个目标运行十分钟，
  分四片，增长的语料通过 Actions 缓存带到下一次，失败的输入作为 artifact 保留（fit 的 `fuzz.yml`）。
- 运行：在 gpu51 上（`flock -s`）每个目标 300 秒，每次八个（2026-11-26），执行次数从 6.6 万（`router_expr`，要编译路径
  正则）到 8600 万（`proxy_grpc_timeout`）；akit 的代码中没有崩溃、泄漏或超时；更正 `admin_pointer` 的判定条件后，它和新增的 `plugin_pairs` 又各运行了
  300 秒（940 万和 5050 万次执行）。之前在 macOS 本地每个目标运行了 15 秒。
- 发现，每个都已修复并有回归测试：
  - **绕过路由保护**：目标为 absolute-form 的请求（`GET http://a/admin/x HTTP/1.1`）按整个目标选路由，没有路径路由匹配，
    于是由兜底路由接收，而代理向上游请求 `/admin/x`。fit（Go 的服务器）按其路径选路由并取其主机（RFC 9112 第 3.2.2 节）。
    现在监听器在路由前把这样的请求改写为 origin-form，主机取目标中的主机，`call` 和 `body_mode` 都如此
    （`absolute_form`）；`path_confusion` 增加了 absolute-form 目标，并新增 `absolute_form_targets_name_their_host` 和单元
    测试 `absolute_form_targets`。发现于 `cache_semantics` 收到一个不是路径的请求路径时。这是 akio 实现 A200 之前的临时
    方案；基准状态表中的一行写明了留给基准任务的同轮 A/B。
  - **`rewrite` 损坏路径**：路径中同时有非 UTF-8 字节和替身字符所占块（U+10FF80 到 U+10FFFF）中的字符时，该字符被变成
    一个字节。现在替身取第 15、16 私用平面中路径和替换文本都没有用到的块（`stand_in_base`）；占满所有块的路径保持不变。
    `characters_of_the_stand_ins_block_are_kept`；由 `middleware_fields` 发现。
  - 不是缺陷，记录在此：当主机匹配两个优先级相同的 `**.` 模式时，路由器先尝试较长的后缀，并跳过较短后缀下更长的路径
    前缀，但较短后缀下的精确路径仍然胜出；fit 也是如此（`router.go` 中提前 `break`），所以 akit 保持一致（规则 19），
    `router_find` 只检查选中的路由确实匹配。值得在 fit 中看一看。
  - 其余是目标自身的判定条件有误，已更正：fit（和 Go 一样）为每个非法字节写一个 U+FFFD；TZif 偏移超过一天是允许的
    （RFC 8536 只是建议），日志时间会饱和；`-` 在对象中是成员名（RFC 6901）。
- 为目标改为 `pub(crate)`：缓存的 `same_origin`、`url_key`、磁盘的 `encode` 和 `decode`；FastCGI 的 `header_end`、
  `parse_head`、`origin_form`、`unescape` 以及 `try_files` 的纯计算步骤 `candidate`；静态处理器的 `scan`；正向代理的
  `target`；中间件的 `credentials`、`client_ip`、`negotiate`、`Rewrite::path`；akit-l4 的 `Opening`；Consul 的
  `Instance::from_json`、etcd 的 `watch_message`；Admin API 指针的 `string`；Proxy-Wasm 的 `window`。没有新依赖
  （libfuzzer-sys 仍只在 `fuzz/` 中）。
- 已检查：所涉 13 个 crate 的 `cargo test`（macOS）、fmt、带和不带 `fuzzing` 的 clippy、`cargo deny`、文档检查。
- 应测量的内容（规则 30）：监听器的 absolute-form 检查，HTTP/1.1 上 fixed 场景的同轮 A/B（每个请求一次比较；预计没有
  开销）。

### 2026-11-26：I79，v1 冻结与兼容性

- **政策**，[docs/compatibility.zh-CN.md](compatibility.zh-CN.md#v1哪些保持稳定) 及其英文版，即 akit 所提供部分的 fit v1：覆盖的范围
  （配置 schema 版本 1 及 akit 的扩展字段、Admin API v1、命令行、指标、访问日志），不覆盖的（消息、仪表盘、WASM 宿主函数和 Proxy-Wasm
  子集、转换器的覆盖范围、akit 各 crate 的 Rust API），1.x 版本可以做什么，`schema_version`，改名字段与弃用如何进行，Admin API v1 的端点
  （fit 的 `/tokens`、`/media/...` 和 `/streaming/...` 在 akit 运行这些部分之前答 404，届时加入），以及新版本附带什么。门禁的页面保留为后半部分。
- **冻结**，[tests/compat/v1](../tests/compat/v1)（[决定 029](decisions/029-v1-freeze.zh-CN.md)）：`config.txt` 是配置的范围（5,181 项：
  fit 的 schema 版本 1 的每个字段及其类型、必填性和默认值，枚举的取值，8 个弃用的旧名，akit 读取的 428 个字段，`sockmap` 扩展字段），由
  `tools/v1-freeze.py --write-baseline` 生成；`admin-api.txt` 是 Admin API 的范围（26 个端点、仪表盘的 2 个不覆盖项、8 个头字段、`dry_run`，以及
  变更、错误和 `GET /versions` 应答的 27 个成员）；`additions.txt` 为空，用来记录此后标记的改动（`added`、`retired`）。`tools/v1-freeze.py`
  （CI 的 lint 任务、`make compat-check`）保存两个基线的 SHA-256，配置范围出现任何未标记的差异即失败；`make v1-bless` 标记兼容的增加，只要有
  破坏 v1 的改动就拒绝（在副本上试过：不再支持某个字段、改变默认值、增加必填字段、修改基线，都会失败；新支持的字段在标记前失败）。akit-admin
  的 `v1_surface_is_frozen`（CI 的测试任务）对每个冻结路径探测每种方法，读取分发器在 `server.rs` 中写出的路径，并检查头字段和应答成员（试过：
  退役一个端点、增加一个端点、退役一个成员，都会失败）。
- **改名的字段**：fit 的 I77 改名的旧名（`cache` 的 `max_size`、`max_entry_size`、`disk.max_size`；`compress` 的 `min_length`；`jwt` 的
  `jwks_refresh`；`wasm` 的 `max_memory`）akit 原本就读取，现在在旧名的键处按其文件、行和列给出 fit 的警告（`Config::warnings`、
  `akit_config::Warnings`，不论内容都视为相等，使配置按含义比较）；同时给出两个名字是 fit 的错误。`akit validate` 和 `akit run` 在启动和每次
  重载时打印它们，Admin API 在变更应答的 `warnings` 中返回它们。
- **门禁**（其余部分见 I79b）：读取 `limits.max_header_bytes`（akio 的 HTTP/1、HTTP/2 和 HTTP/3 上限，答 431；默认为 fit 的 1 MiB，akio 的是
  64 KiB；固定在每个监听器的绑定中，重载改变它时重新绑定）、`limits.max_conns` 和 `limits.max_write_queue_bytes`（akio 的 `max_connections` 和
  `max_queued_bytes`，启动时生效），并把 `engine` 映射到 akio 的引擎上（[决定 028](decisions/028-engine-on-akio.zh-CN.md)：`event_loops` 是每个
  监听器的 I/O 线程数，`workers`、`min_workers` 和 `task_queue` 设定逻辑池，`read_buffer_size`、`write_watermark`、`backlog`；`loop_wait`、
  `poll_batch` 和 `kernel_tls_offload` 被拒绝）。重载改变启动时生效的设置时，会在警告中说明。参考文档新增“引擎”一章，并对每处差异加了说明。

门禁的数字：之前 80 个夹具，45 个有效，34 个只因暂不支持被拒绝，被拒绝的字段 30 个，1 个跳过；之后有效和被拒绝的夹具不变，被拒绝的字段
26 个。每个 fit 模板（6 个）之前和现在都有效；仍被拒绝的示例是 `limits` 和 `proxy`（只因 `limits.max_pending_bytes`，akio API 请求 A214）、
`grpc`（只有 h2c，A168），以及 1.0 之后路线的示例（`bittorrent`、`console`、`llm-*`、`logging`、`media-pages`、`streaming`、`sync-*`（16 个）、
`tunnel-*`、`video-site`、`webrtc-*`、`whip-whep`）。

检查：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`，以及 akit-config（新增 `tests/v1.rs`、`limits.rs`）、akit-runtime、
akit-listener（新增 `tests/limits.rs`：在 HTTP/1.1 和 h2c 上低于、超过上限以及默认值时的 431，超过 `max_conns` 的连接被关闭、有连接离开后又有
名额，引擎的设置）、akit-admin（`v1_surface_is_frozen`）、akit-compat 和 akit（`validate_warns_of_renamed_fields`、`loads_return_warnings`、
文档中的示例）的测试，在 macOS 上，以及在基准主机上用 epoll 和 io_uring。

与计划的不同：

- 完成标准在 I79 之前就已成立（每个模板都有效）；I79 还支持了网关示例中 akio 能提供的字段，并把需要 akio 的部分拆到 I79b（A214、A168、A215）。
- 在 1.0 中不覆盖 akit 各 crate 的 Rust API，而 fit 覆盖其 Go 的 `module` 包。
- Admin API 的检查只把关三种应答的成员，不包括 `/status`、`/config/effective` 和 `/cache` 的（决定 029）。
- 无需基准测试：控制面（规则 24）。数据面上改变的是监听器引擎的默认值：`max_connections` 为 100,000（之前不限制），并且也计入上游连接（A215），
  `max_queued_bytes` 为 32 MiB（之前 64 MiB），请求头至多 1 MiB（之前是 akio 的 64 KiB）。对 akit 运行 100,000 个及以上连接的基准任务应设置
  `limits.max_conns`。

### 2026-11-26：I78，配置参考与指南

- **配置参考**，[docs/reference/config.zh-CN.md](reference/config.zh-CN.md) 及其英文版：akit 读取的每个对象（62 张表），每个字段的
  类型、默认值和说明，这些对象中按“尚不支持”拒绝的字段，akit 读取方式与 fit 不同时的说明，以及 akit 的扩展字段
  （`listeners[].sockmap`）；配置的每一部分一章，各带示例。表格由 `tools/config-reference.py`（`make reference`）根据 fit 固定
  版本的 schema（类型、默认值以及 fit 的说明，改为就 akit 而言；akit 沿用 fit 的默认名称，如 `fit_sticky`、`fit/`，这些保持原样）、
  `tests/compat/supported.txt` 和 `tools/config-reference-notes.json`（扩展字段和说明，中英双语）生成；表格周围的说明和示例为手写。
- **检查**，都在 CI 中：`config-reference.py --check`（lint 任务，以及 `make docs-check`）在表格不是生成器写出的内容、或 akit
  读取的某个对象在任一语言中没有表格时失败；兼容性门禁中的 `reference_matches_akit_config` 把表格与 `model.rs` 中 akit-config 的
  列表对应起来（读取的字段都显示为已读取，被读取对象的其他字段都显示为拒绝，每个处理器和中间件模块与 `model.rs` 一致，显示的扩展
  字段正是 `*_AKIT_ONLY` 列表）；`crates/akit/tests/docs_examples.rs` 把每个 Markdown 文档中的每个配置示例（此提交共 154 个：
  `.conf`、YAML 和 JSON，以及迁移指南中先经 `akit adapt -from` 转换的 nginx 配置和 Caddyfile）在一个放有其所需文件的目录中
  （测试 CA 签发的证书、`public/`、一个 fit ABI 插件和一个 Proxy-Wasm 插件）交给 `akit validate`，并检查每个文档与其中文版展示的
  示例相同。
- **指南**，[docs/guides](guides/README.zh-CN.md)，中英双语：入门；反向代理与负载均衡；静态文件与 PHP；HTTPS 与证书；Admin API 与
  重载；缓存；WebAssembly 插件（注明 fit 的 `llm` 尚不支持）；从 fit 迁移（akit 尚不运行的部分、akit 沿用的 fit 默认名称、ACME
  自己的存储目录）、从 nginx 和 Caddy 迁移（`akit adapt -from`，并给出它为指南中的示例写出的内容）以及从 Traefik 迁移。README 链接
  了它们，不再说还没有构建任何东西；`akit run -h` 不再说 `.conf` 以后才支持。
- 编写时发现、已写入文档而未修改代码的：`akit adapt -from nginx` 把 `error_log`（`-from caddyfile` 把全局 `log`）转换为 fit 的
  `log`，而 akit 按“尚不支持”拒绝它；Admin API 的 `POST /reload` 重新应用运行中的配置，不读取文件（SIGHUP 才读取）。

与计划不同之处：

- 中文参考包含每张表，列出每个字段的类型、默认值、状态和 akit 的说明，但不含 fit 的字段说明（fit 的 schema 中是英文）；每张表
  链接到英文版的同一张表，与 fit 自己的中文参考一样。
- 没有基准测试：只有文档和测试（规则 24）。不需要新的决定、akio 需求或 ADR。

### 2026-11-25：I25，M1 一致性测试与报告

细节和 *akit 的现状* 见 [M1 报告](../bench/results/m1-report-2026-11-25.zh-CN.md)。

- h2spec 2.0.0，全部章节，在开发用的 Mac 上：h2c 和 TLS 上的 h2 都是 145/145，用 `respond` 应答以及经 `proxy` 到 HTTP/1.1
  上游。
- 代理一致性测试套件 `crates/akit/tests/conformance.rs`（23 个测试，通过二进制运行）：逐个用例移植 fit 在 HTTP/1.1 上的
  `TestProxyConformance`，以及基于手写 h2c 帧的 HTTP/2 部分（RFC 9113 第 8 节的畸形请求被重置或以 400 应答且从不转发，cookie
  片段被连接，`:authority` 变为 `Host`，返回中没有连接专用字段，1xx、204、304 和 502，遵守窗口）。在 macOS 和 Linux 上通过；
  akit-proxy 的测试在 epoll 和 io_uring 上通过。
- 在 akit 中发现并修复：字段值中的 NUL、CR 或 LF（akio 的 HTTP/2 服务端接受）以及两个 `Host` 字段（其 HTTP/1 解析器接受）曾被
  转发；akit-proxy 在复制字段的循环中以 400 拒绝这两类请求（`Refused::BadFieldValue`、`Refused::TwoHosts`），作为 akio API 需求
  A169 锁定之前的临时措施，与 fit 检查字段名的做法一样。`akit validate` 现在像 `akit run` 一直以来那样拒绝只有 `h2c` 的监听器
  （A168）：兼容性门禁把 fit-examples 的 `grpc` 改为 not supported yet（80 个夹具：45 个有效，34 个只因暂不支持被拒绝，1 个
  跳过）。客户端的 HTTP 连接空闲 120 秒后才关闭，与 fit 一致（akio 的默认值是 75 秒：10 万空闲连接时 akit 关闭了三分之一）。
- 在 akio 中发现，各自在单独的 akio 分支上修复，未进入 akio 的 main，也未锁定（规则 22），各自需要 akio 的同轮 A/B：A169，畸形
  请求（`claude/a169-request-checks`，`813f53ea`；其第四部分——流式 body 中错误 chunk 大小应答 400——未实现）；A171，HTTP 客户端
  的定时器任务把每个请求保留到超时过去，akit 的代理因此每个请求增长约 240 字节，一次运行中达 1.8 GiB
  （`claude/a171-client-timer-cancel`，`90adec65`）；A172，akio-tls 让 server name 为 IP 地址（wrk 会发送）的 ClientHello 握手
  失败，HTTPS/1.1 因此没有应答（`claude/a172-sni-ip-literal`，`d23c9368`）。A168（只提供 h2c）已提出。
- 基准主机上的基准测试，fit 的 harness `7daee573`，三次运行：akit `00d9f18` 与全部五个服务器（1 万空闲连接，以及 10 万），以及
  带三个 akio 修复构建的 `9d9321e` 与 fit、nginx 一起（`bench/results/m1-2026-11-25`、`-idle100k`、`-akio-fixes`）。akit 在
  HTTP/1.1、HTTPS/1.1、h2c 和 h2 上的 fixed 和 static 都排第一，只有 h2c 上的 static 除外（fit 的 98%）；代理排第三（每秒 24.5 万、
  23 万、23.8 万和 22.1 万请求，最佳的 86–93%，每请求 CPU 27.5 µs，nginx 18.2）。空闲连接，1 万和 10 万时每个的字节数：HTTP/1.1
  1,144 和 1,132（fit 1,497 和 1,441，nginx 556 和 530）；TLS 12,942 和 12,872（fit 9,879 和 8,593，nginx 29,924 和 29,583）；
  WebSocket 在 1 万时 4,452（fit 5,916，nginx 22,275）。
- 基准配置与 fit 对齐：proxy 路由的 60 秒超时和连接池的 `max_fails 0`，以及到 fitbench WebSocket 后端的 `/ws` 和 `ws` 空闲
  类型（I49）。
- M1 退出标准满足。路线图的性能目标中，静态文件达到；代理吞吐量（高于 fit 和 nginx）和 p99（低于 fit），以及空闲 HTTP/1.1
  （不高于 nginx）和 TLS（低于 fit）内存尚未达到：开发计划中基准任务的各行说明了要测什么。

与计划的差异：

- I25 运行了基准和内存测量，这是该迭代的要求，也得到了用户的允许。
- 按当前提交的 akit，其 HTTPS/1.1 和代理内存的数字是锁定的 akio 下的结果（HTTPS/1.1 没有应答，1.8 GiB）；报告在旁边给出了带
  akio 修复的 akit 的结果。
- h2c 上的 fixed 比 M0 基线低 7–8%，fit 和 nginx 没有这种变化；`7c835ad` 对 `9d9321e` 的同轮 A/B 留给基准任务。
- Traefik 在 proxy 场景中没有启动：主机上另一个进程占用了 8444 端口。
- `akit-proxy` 的 `split::failover` 在 io_uring 上失败过一次（failover 的第一个请求得到 502），单独重跑十次都通过：被动健康
  检查中一个罕见的竞争，与 I25 无关。
- 在基准主机上检查：所改 crate 的测试经 `tools/test-timeout.sh` 在 epoll 和 io_uring 上运行，全部通过，只有 `akit` 的
  `reloads_under_traffic_with_no_failed_request`（I22 的）在其他会话的基准占用主机时于 epoll 上 6 次失败 3 次，基线
  `2a10480` 同样 8 次失败 1 次；在较空闲的主机上与基线交替运行，各 10 次全部通过。

### 2026-11-25：I76，M5 退出

- **M5 已完成**：[M5 报告](../bench/results/m5-report-2026-11-25.zh-CN.md)（及其[英文版](../bench/results/m5-report-2026-11-25.md)），
  含 *akit 的位置*；每条退出标准都成立。在基准测试主机上测量，每次运行都独占主机的锁，akit 为本提交的工作区，akio 为
  `dad77a7a`，fit 为 `7daee573`。
- **缓存对照 nginx 的 `proxy_cache`**：akit 加入基准框架的 `cache-hot` 和 `cache-zipf`（`bench/fitbench/akit.go`、
  `bench/configs/akit/fit.json`：路由 `cache`，1 GiB 的区域，与 fit 相同）。命中相同或更多（`cache-zipf` 99.98%，nginx
  99.86%，fit 99.97%），三者中第一：HTTP/1.1 上每秒 589,973 个请求（nginx 398,691，fit 526,977），h2c 上 2,086,275（nginx
  337,106，fit 1,694,120），100,000 个路径时 564,220（nginx 322,575，fit 505,880）。
- **击穿**：`bench/stampede.sh`（新增；`bench/stampede/`）让 1,000 个客户端同时向 nginx（`proxy_cache_lock on`）、fit 和 akit
  请求一个冷键，慢源站（一秒）统计被取的次数：各取一次，HTTP/1.1 和 h2c 上各三个键，每个客户端都回答 200；对照组
  `disable_lock` 的 akit 为 1,000 和 829 次。
- **WASM 开销**：`wasm` 场景和两个服务器 `akit-wasm`、`fit-wasm`，运行 fit 自己的 `headers` 测试插件，同一个 `.wasm`
  （`bench/configs/akit/wasm.json`、`bench/configs/akit/fit-wasm.conf`，各服务器自己的配置之外的第二个配置文件；`lib.sh`
  解压 guest；`run.sh` 传入 `-akit-wasm-image`）。HTTP/1.1 上插件让 akit 每个请求多花 14.7 µs CPU（每秒 312,679 个请求，
  不带插件时 831,888），fit 6.8 µs（452,353 对 785,657）；h2c 上 2,048 个同时进行的请求超出 1,024 的队列，两者都立即以
  503 回答多出的部分，内存保持平稳。开销是 fit 的两倍：akio-wasm 解释执行，wazero 编译执行，已作为 **A180** 向 akio 提出
  （akio 的 W5，附这些数字）。
- **nginx 转换器**：`convert::tests::nginx_samples_list_every_skipped_directive`（`akit-config`）：fit 的三个样例配置和 CLI
  测试转换的另外三个 nginx 配置全部能转换，每条警告都写明行号，在它们 56 个块的开头和顶层放一条任何转换器都不认识的
  指令，它总被列出（在其行号处，或在整体略过的块中），输出在其他方面不变；静默丢弃未知指令的转换器无法通过。
  `nginx::convert` 拆分出 `convert_parsed`，接受已读出的指令。
- **FastCGI**：I75 的 PHP-FPM 测试（fit 的 `TestPHPFPM`，`php_fpm`）在主机上的 `php:8.4-fpm` 镜像（PHP 8.4.28）中运行，
  epoll 和 io_uring 上都通过。
- **akit 的位置**（完整基准框架，所有服务器，HTTP/1.1 和 h2c）：fixed（822,825 和 5,584,430）、HTTP/1.1 上的 static
  （785,510）和三个缓存运行中第一；h2c 上的 static 第二（fit 的 99%）；proxy 中第三（nginx 的 88% 和 93%），1.8 GiB 的内存
  峰值来自 akio 的 HTTP/1 客户端定时器，由 I25 发现（A171）；空闲连接在 HTTP/1.1 上次于 nginx（1,136 字节），TLS 上次于
  fit（12,954）。
- 基准框架：`BENCH_DIR` 让一个会话在主机上与其他会话分开部署；在 git worktree 中要设置 `FIT_REPO`。开发计划中缓存和 WASM
  的基准行标为部分已测，并写明剩下的。
- 已检查：macOS 上的 `cargo fmt`、`cargo clippy --all-targets -D warnings` 和 `cargo deny check`，`akit-config` 和 `akit`
  的测试；基准测试主机上经 `tools/test-timeout.sh` 在 epoll 和 io_uring 上运行 `akit-config` 和 `akit`。

### 2026-11-25：I56，M3 退出

- 路线图中 M3 的退出标准满足；[M3 报告](../bench/results/m3-report-2026-11-25.zh-CN.md) 列出了每条标准及其检查方式、
  每个中间件与 fit 的对比、包含所有服务端的完整基准套件，以及 *akit 所处的位置*。akio 未变（`dad77a7a`）；没有 akio
  API 需求，没有决策记录。
- 没有模块阻塞 I/O 线程：`crates/akit/tests/workers.rs`，移植自 fit 的 `test/workers`。`akit::STANDARD` 注册的每个
  模块（14 个中间件、5 个处理器）都处理一个其应答要等某个慢的部分 600 ms 的请求（每个中间件后面的 `proxy` 上游、
  `forward_auth` 的鉴权服务、FastCGI 应用、`forward_proxy` 隧道的另一端、akio 池上的 bcrypt、`wasm` 池上 fit 的插件）；
  akio 的慢回调检测器在运行时（`Telemetry::live`）设为 200 ms，必须没有任何记录，且应答在慢的部分之后才到。
  `every_module_covered` 对没有用例的模块失败；让一个中间件睡 300 ms，测试就会失败并指出它。fit 排除了 `forward_proxy`
  和 `wasm`；akit 两个都覆盖。
- 慢对端：`websocket_slow_upstream`（客户端 64 MiB 的洪流被不读的上游压住，之后完整且按序送达）和 `udp_slow_peers`
  （对一个会话晚应答的上游不拖慢其他会话；被狂发而不读的客户端保持其会话），与 I49–I51 的测试并列。来自 h2c 和 h3 的
  WebSocket、双向的 gRPC 和双向的 TCP 已经有这类测试；单端口（I53、I55 的 `sniff_clear` 和 `socks_beside_https`）也有。
- 基准：`bench/modules.sh`（以及 `modulesreport.py` 和 `configs/modules`）提供同一份配置，每个中间件各在一条路由上，
  放在代理到 nginx 的前面，由 fit 以及 epoll 和 io_uring 上的 akit 提供（`crates/akit/examples/bench_serve.rs`：
  `akit run` 的路径，可选后端），wrk 按 fit 的 CPU 划分，3 轮。每个中间件给 akit 增加的开销与给 fit 的每请求相差不到
  1 µs，只有 gzip（+6.7 µs，fit +3.9）、`wasm`（+13.6，fit +6.4）和 `forward_auth`（+20.4，fit +17.0）例外。完整基准
  套件（`m3-2026-11-25`）：akit 在 fixed 的 HTTP/1.1（830k）、h2c（5.57M）和 h2（5.13M）以及 static 的 HTTP/1.1
  （788k）和 h2（2.70M）上第 1，在 static h2c（fit 的 98%）和空闲内存（1,141 和 12,959 字节）上第 2，在代理上第 3
  （领先者的 88–95%，每请求 CPU 27.8 µs，fit 21.2）。
- 发现：HTTPS/1.1 没有应答（akio-tls 拒绝 wrk 的 IP 字面量 SNI），代理的内存每次运行涨到 1.8 GiB（akio-http 客户端
  的定时器保留每个请求的任务）：即 I25 的 akio API 需求 A172 和 A171，已在 akio 的分支上修复，尚未进入 akio 的 main；
  没有再次提出。
- 与计划不同：模块基准经回环走监听器（akio 没有 fit 的 `BenchmarkModule` 所用的进程内请求上下文），因此把每个中间件
  的开销读作它给代理路由增加的 CPU，fit 也按同样方式测。开发计划基准表中 I40–I45、I47、I48、I67 和 I71 的行已更新。
- 检查：`akit`、`akit-proxy` 和 `akit-listener` 的 `cargo fmt --check`、`cargo clippy --all-targets -D warnings`，以及
  它们在 macOS 上的测试；在 gpu51 上，`workers`、`websocket` 和 `l4` 测试二进制在 epoll 和 io_uring 上经
  `tools/test-timeout.sh` 运行，全部通过。
- 下一步：M2 的 I38；I77（fuzz），I56 完成后已解锁。

### 2026-11-25：I66，M4 退出

- akio：未变，`dad77a7a`；没有 akio API 需求、akio ADR 或决策记录（A194–A199、ADR 146–149 和
  决策 020–021 未使用）。没有改动 akit 代码。
- **Gateway API 一致性测试套件通过**：Gateway API v1.3.0（fit 锁定的版本）上的 `GATEWAY-HTTP`
  core，通过 33，失败 0，跳过 0，第一次运行即通过（[报告](../tests/conformance/reports/gateway-http-v1.3.0-2026-11-25.yaml)，
  与 fit 的 I61 通过的 33 个测试相同）。在基准测试主机上持共享锁由 `tests/conformance/run.sh` 运行：
  `7daee573` 上 fit 的套件模块（`go test -c`），一次性的 kind 集群（`akit-i66`，v0.30.0，
  Kubernetes v1.34.0，与主机上另一个 kind 集群使用不同的 Pod 和 Service 子网），实验通道的 CRD，
  集群外 akit 的 release 二进制。其余 38 个测试（扩展特性、TLSRoute、UDPRoute、mesh）因未声明而
  跳过，与 fit 相同。第二次带扩展 HTTP 特性、仅为了解情况的运行在主机的锁上排在其他会话的基准测试
  之后等待了一个多小时，已放弃（见 [M4 报告](../bench/results/m4-report-2026-11-25.zh-CN.md)）。
  只在 epoll 上：`akit run` 没有切换到
  io_uring 的开关（akio 的 `Backend` 在代码中设置，只有测试会这样做）。
- **Docker Compose 示例**：`deploy/compose`（`compose.yaml`、`akit.conf`、`test.sh`、中英文
  README），fit 的 `fit-examples/docker-compose` 的 akit 版，标签在 `akit.` 下。`test.sh` 检查两个
  副本都作答、其他主机得到配置中的 404、扩容后第三个副本作答、应用的主机在其容器停止后得到 404、
  启动后得到 200；`AKIT_IMAGE` 运行在别处构建的镜像，`AKIT_PORT` 改变端口。它在基准测试主机上
  通过（Docker Compose 2.40.3，`ubuntu:24.04` 上的 release 二进制，因该主机没有 Dockerfile 所需的
  buildx）。
- **CI**：与 fit 相同的 "Gateway API conformance (kind)" 作业（检出 `7daee573` 上 fit 的套件模块、
  kind、CRD、带 `CAP_NET_BIND_SERVICE` 的 akit、`run.sh`，报告和 akit 的日志保存为 artifact），以及
  `packaging` 作业中在其构建的镜像上运行的 "Docker Compose example" 步骤。两者都尚未在 GitHub 上
  运行（没有推送）。
- 文档：`tests/conformance/README*`（结果、CI 作业、与另一个 kind 集群并存时的子网）、
  `deploy/README*`（Compose 示例）、带 *akit 的位置* 的 [M4 报告](../bench/results/m4-report-2026-11-25.zh-CN.md)；
  计划的基准测试状态表新增 "M4 退出" 行（完整测试框架和空闲的 provider，留给基准测试任务；规则
  30：这里不运行基准测试）。
- 检查：上述两次在基准测试主机上的运行；`scripts/check-docs.sh`。没有改动 Rust 代码，因此没有运行
  cargo test。已清理（规则 29）：基准测试主机上的 kind 集群、其路由、Compose 项目、镜像和
  `/root/akit-i66`。

### 2026-11-25：I38，M2 退出：频繁替换、健康检查时间窗、只写主机名的证书

- akio：未变，`dad77a7a`。akio API 需求 **A10**（立即关闭 HTTP 客户端的空闲连接，I14 提出）已在 akio 分支
  `claude/a10-close-idle`（`af959290`，基于 akio 的 `e79cc740`；未推送、未合并）上实现：`Client::close_idle`，
  在 macOS 和 gpu51（epoll、io_uring）上测试。没有新的 akio 需求、akio ADR 或 akit 决策记录（I38 分到的编号
  A187–A193、ADR-142–145 和决策 018–019 都没有用）。
- 测试在 `crates/akit/tests` 中，辅助代码在 `tests/m2/`（上游与客户端，以 `akit run` 或进程内方式运行的 akit，Pebble）：
  - **频繁替换**（`churn.rs`），第一条标准。流量：HTTP/1.1 每个请求一个连接（两个客户端，各每 10 ms 一次）、
    一个全程繁忙的 keep-alive 连接（经代理和不经代理）和一个两次请求之间空闲半秒的 keep-alive 连接；
    预知 HTTP/2，全程一个连接、同时四个流；经代理的慢速流式响应（1 秒内 20 KiB，每个跨越数百次替换）；
    经代理的 WebSocket 和经 `tcp` 服务的连接，都全程打开、每 20 ms 回显一次。每次替换修改 `/v` 的回答；每第 20
    次修改被代理上游的地址和负载均衡策略；从第 10 次起每 100 次增加一个监听器并检查它，50 次后移除。
    `churn_through_sighup_and_the_admin_api`：`akit run`，2000 次替换（`AKIT_CHURN_SWAPS`），每次应用完再做下一次，
    依次经 SIGHUP、Admin API 的 `POST /load` 和按指针编辑（`PATCH /config/routes/0/handler/params`）；
    之后 `fit_config_reloads_total{result="success"}` 计入全部替换、没有失败，进程的描述符数（Linux 上取自 `/proc`，
    其他系统取自 `lsof`）回到流量开始前的 16 个以内；基数在先增加再移除一次监听器之后取（akio 第一次为运行时监听器
    创建每线程的管道并保留，macOS 上为 28 个；运行时增加的监听器存在期间在每个 I/O 线程上占一个套接字和一个 poller）。
    `churn_in_process`：同样的流量经 `Listeners::reload`，在 `AKIO_TEST_BACKEND` 指定的后端上（即 io_uring 上的
    频繁替换），每次替换都保留流量使用的监听器，之后被替换的图全部退役（`GraphCell::retiring` 为 0）。两者都检查：
    没有请求失败，没有连接在客户端之下被关闭（结束时 HTTP/2 连接仍打开），每种流量都持续进行，最后一个配置的回答
    已生效，两个上游接受的连接数不超过替换次数的一半。
  - **健康检查时间窗**（`health_window.rs`），第三条：每 10 ms 一个请求，经代理发往两个地址的上游，带 http
    `health_check`（间隔 200 ms，超时 100 ms，`fails` 和 `passes` 都为 2）；一个地址的探测路径挂起，而其他请求
    照常回答，如 fit 的端到端 `TestHealthCheckRotation`。它必须在 `fails × interval + timeout + interval`（700 ms）
    之内、以及下线期间（包括期间重载同一配置）不再收到请求，并在重新回答后的 `passes × interval + interval`（600 ms）
    之内重新收到请求，各留 2 秒给负载高的主机，期间没有请求失败。经 `akit run` 时，该地址的 `fit_upstream_backend_up`
    在 `GET /metrics` 上依次为 1、0、1；进程内在 `AKIO_TEST_BACKEND` 的后端上运行（探测跑在该引擎上）。
  - **只写主机名的证书**（`acme_m2.rs`），第二条：`akit run` 运行只写了主机名的配置（另外写了 CA 的目录和端口，
    因为 Pebble 不是 Let's Encrypt，测试也不占用 80 和 443）：证书经默认 HTTPS 生成的 `http` 监听器上的 HTTP-01
    取得，出现在存储中，在 `https` 上经 HTTP/1.1 和 h2 提供（证书链检查到 Pebble 的根，也单独检查到签发它的中间
    证书），`http` 重定向到它。`m2/pebble.rs` 从二进制运行 Pebble，或在设置 `AKIT_PEBBLE_DOCKER=1` 时在 Docker 中运行
    （Let's Encrypt 的 `ghcr.io/letsencrypt/pebble:2.10.1` 和 `pebble-challtestsrv`，使用主机网络，测试结束时删除）；
    设置 `AKIT_PEBBLE_REQUIRED` 时没有 Pebble 测试即失败。
- **已修复：重载时未变的上游保留连接池**（fit 的 `Env.Previous` 和 `upstreamKey`，`runtime/graph.go`）。频繁替换测试
  发现每次替换都重新拨号所有上游：akit 在每次重载时重建所有连接池，被替换连接池的空闲连接一直保持到 `idle_timeout`
  （A10），于是 2000 次替换打开约 7000 个上游连接，耗尽了 macOS 的临时端口；上游的主动和被动健康状态也在每次重载时
  重新开始（被探测下线的地址回到轮转，直到再次探测失败）。现在 `akit_runtime::Previous::of(graph)` 和
  `Env::with_previous` 把运行中的图的连接池交给 `compile`，新图对配置和 TLS 文件校验和都相同的上游沿用原连接池
  （`Pool::share`，fit 的 `Acquire`）；图清理时释放它的连接池（`Pool::release`），持有某个连接池的最后一个图关闭它。
  `akit run` 的替换（SIGHUP、Admin API、`-watch`、提供者）传入运行中的图；dry run 和 `akit validate` 不共享。
  测试：fit 的 `TestSharedPools`（`akit-runtime/tests/upstream_tls.rs`）：未变时共享，CA 文件或客户端证书改变时
  新建连接池，最后一个持有者释放时连接池关闭；频繁替换的连接计数（2000 次替换 406 个连接，不共享时约 7000 个）；
  健康检查时间窗中地址下线期间的重载。在 akit 锁定 A10、`Pool::close` 调用 `Client::close_idle` 之前，改变了的上游
  被替换的连接池仍把空闲连接保持到 `idle_timeout`；在此期间频繁替换测试的 `app` 设置 1 秒的 `idle_timeout`。
- **频繁替换测试的其他发现**（fit 的约定，akit 照此实现）：经 `tcp` 服务的连接在第一次替换后正好
  `-shutdown-timeout`（5 秒）被关闭，尽管它的监听器和服务都没有变。重载会在排空超时内继续处理被替换的图仍在服务的
  内容，超时后关闭它的 `tcp` 服务仍在转发的连接（fit 的 `TestTCPReload`，I51）；fit 的代理此时也会切断它的请求和
  WebSocket 转发（`Proxy.Drain`），akit 尚未实现（已在 I49 中记录）。因此长连接在替换掉它的图之后至少保持排空超时那么
  久：频繁替换测试使用比测试时长更长的排空超时（300 秒），在此之下每个连接都必须保持整个测试。另外，测试自己背靠背的
  短连接耗尽了 macOS 的临时端口（TIME_WAIT），现在每 10 ms 一个。
- 结果（各 2000 次替换；macOS，调试构建）：经 `akit run`，5.0 秒，约 117,000 个请求得到回答（短连接 817、keep-alive
  42,981、h2c 72,832、慢速流 5、WebSocket 回显 214、`tcp` 回显 216），上游连接 406 个，没有失败；进程内，4.8 秒，约
  123,000 个，406 个，没有失败。健康检查时间窗：探测停止回答后 0.48–0.51 秒下线（时间窗 0.7 秒），重新回答后
  0.33–0.37 秒恢复（时间窗 0.6 秒）。
  gpu51 上（Linux，调试构建，主机同时在跑其他会话的任务）：epoll，经 `akit run` 4.8 秒、约 37,000 个请求，进程内 5.1 秒、约
  29,000 个，各 395 个上游连接；io_uring 进程内 5.8 秒、约 24,600 个，391 个（同一轮中 `akit run` 在 epoll 上，4.8 秒，约
  28,000 个）；没有失败，没有连接被断开，描述符数回到原处。gpu51 上的健康检查时间窗：0.45–0.49 秒下线，0.39–0.40 秒
  恢复，epoll 和 io_uring 都是。从 Docker 中的 Pebble 取得证书：每个测试 1.3–2.9 秒内签发、提供并重定向，两种后端都是；
  没有遗留容器。
- **Let's Encrypt staging 证书**，即标准本身，于 2026-11-26 由维护者的会话运行通过（没有会话注册或指向域名）：在一台
  公网 Debian 12 主机上（Linux 6.1，x86_64；fit 自己的 staging 运行用的那台），DuckDNS 域名
  `akit-test.duckdns.org`，端口 80 和 443 空闲。`akit` 和 `acme_m2` 测试二进制由本分支（akio `dad77a7a`）在 gpu51 上的
  `rust:1-bookworm` 容器中构建（glibc 2.36，与该主机相同），复制到主机上；以 root 运行：

  ```sh
  AKIT_ACME_STAGING_NAME=akit-test.duckdns.org acme_m2 only_a_host_name_from_lets_encrypt_staging --exact --nocapture
  ```

  配置只有路由的主机名和 staging 目录（`https://acme-staging-v02.api.letsencrypt.org/directory`），使用默认端口。
  证书由 `C=US, O=Let's Encrypt, CN=(STAGING) Baloney Bulgur YE2` 签发，`https` 经 HTTP/1.1 和 h2 提供它（证书链检查到
  中间证书），`http` 以 308 重定向：1 个通过，11.48 秒。之后主机上的文件已删除。再次运行时，在一台有公网域名指向、
  互联网能经 80 端口访问的主机上以 root 运行：
  `AKIT_ACME_STAGING_NAME=NAME cargo test -p akit --test acme_m2 only_a_host_name_from_lets_encrypt_staging -- --nocapture`。
- 未覆盖的部分及原因：`akit run` 没有选择引擎后端的设置（总是 epoll，macOS 上是 kqueue），所以经 SIGHUP 和 Admin API
  的频繁替换只在 epoll 上运行，io_uring 上的频繁替换在进程内运行；io_uring 字段应由 fit 先增加（决策 002）。改变了的
  监听器（而非保留的）会被替换并排空连接；那条路径由 I22 和 I28 的测试覆盖，不属于频繁替换测试。
- 待测量（基准任务，规则 30）：开发计划中新增的“有流量时频繁替换配置”一行：每 10 ms 替换一次配置时的 fixed 和 proxy
  场景，对照不替换，以及替换 10 万次后 akit 的 RSS 和描述符数；完整测试套件的 M2 报告（规则 24）。
- 检查：macOS 上 `cargo fmt --check`、`cargo clippy --workspace --all-targets -D warnings`、文档检查和
  `make compat-check`，以及 akit-upstream、akit-runtime、akit-listener、akit-proxy、akit-l4 和 akit 的测试（全部通过）；
  gpu51 上在共享锁下运行 fmt、workspace clippy，以及经 `tools/test-timeout.sh`（`--test-threads=1`，没有 `HANG:`）在
  epoll 和 io_uring 上的 workspace 测试：除经 `akit run` 的频繁替换测试的描述符检查外全部通过，该检查计入了最后一次
  替换增加的 `extra` 监听器和 akio 一次性创建的管道（测试已如上修正）；之后在两种后端上重新运行 M2 测试，全部通过。
  akio 分支：在 macOS 和 gpu51（epoll、io_uring）上运行 akio-http 的 `client` 测试和 clippy。没有运行基准（规则 30）。

### 2026-11-25：I72，WASM II：Proxy-Wasm 子集

- akio：未变，`dad77a7a`（akio-wasm，ADR-125，以及 ADR-128 的挂起响应头和响应体过滤器，akio API 需求
  A165–A167）；没有新的 akio API 需求，没有 akio 分支。
- `akit-plugin`：Proxy-Wasm 过滤器，fit 运行的那个子集（[决定 011](decisions/011-proxy-wasm.zh-CN.md)），移植自 fit
  的 `plugin/wasm/proxywasm.go` 和 `response.go`：导出 `proxy_abi_version_0_2_0` 或 `_0_2_1`（以及
  `proxy_on_context_create`、`proxy_on_request_headers`、`proxy_on_done`、`proxy_on_log`、`proxy_on_delete`、
  `proxy_on_memory_allocate`，其类型在实例启动时检查）的模块，与 fit ABI 的插件运行在同一个 `InstancePool` 上。
  每个实例用 `vm_config` 和 `config` 启动它的根上下文（`proxy_on_vm_start`、`proxy_on_configure`；被拒绝则加载
  失败）。每个请求是一个独立的上下文，在池上创建并调用 `proxy_on_request_headers`：插件对 `:method`、`:path`、
  `:authority`（或 `host`）和请求字段的修改通过 `Context::req_mut` 成为请求的修改；它的本地响应带着字段和 gRPC
  状态发出；暂停而不应答时按 `on_error` 失败。宿主模块 `env`（`crates/akit-plugin/src/proxywasm.rs`）：
  `proxy_log`、`proxy_send_local_response`、按值和按键值对读写的头部表（请求、响应、trailer）、
  `proxy_get_buffer_bytes` 和 `proxy_set_buffer_bytes`（VM 配置和插件配置、响应体）、`proxy_get_property`
  （`properties`、`plugin_name`、`request.*`、`source.address`、`response.code`）、指标（每个实例 256 个）、
  `proxy_set_effective_context` 和 `proxy_done`；对外调用、共享数据和队列、定时器、外部函数和设置属性回答
  `Unimplemented`。
- 响应回调（`crates/akit-plugin/src/response.rs`）：跟随响应的过滤器在其实例上保持请求的上下文打开，每个实例最多
  `max_streams` 个（超出时应答 503），其回调是排队给该实例的任务（`submit_to`，实例队列每个流四个调用）。
  `proxy_on_response_headers` 在被挂起的头部上运行（`Context::hold_response_head`，A166），头部改动后继续、被插件的
  本地响应替换，或失败；`proxy_on_response_body` 和 `proxy_on_response_trailers` 在响应体过滤器上运行
  （`Context::add_body_filter`，A167），被暂停的响应体最多保留 `max_body_bytes`，trailer 在最后一次调用中。
  `response: headers` 不注册响应体过滤器，`none` 什么都不跟随。失败的回调，或无法排队的回调（其实例已重建、池已
  关闭），让响应失败，或在 `on_error: skip` 时不经插件继续。上下文在最后一个回调结束时关闭，或者在响应的最后一个
  钩子被丢弃时（响应被切断、客户端离开）由排队的任务关闭；已释放配置的池在关闭前等待它跟随的响应，最多 10 分钟。
- 配置（`akit-config`）：读取 `vm_config`、`properties`（`path` 必填，`value`）、`response`（`all`、`headers`、
  `none`）、`max_streams`（默认 1024，最多 65536）和 `max_body_bytes`（默认 1 MiB，最多 4 GiB）；fit 的 `wasm`
  的每个参数现在都已读取。
- 测试：fit 的 `TestProxyWasmRouting`、`TestProxyWasmProperties`、`TestProxyWasmResponseHeaders`、
  `TestProxyWasmResponseBody`、`TestProxyWasmResponseTrailers`、`TestProxyWasmResponseFails`、
  `TestProxyWasmMaxStreams` 和 `TestProxyWasmResponseClientGone`，经 akit 的监听器在 epoll 和 io_uring 上运行
  （`crates/akit-plugin/tests/proxywasm.rs`），fit 的测试过滤器原样运行：Proxy-Wasm Go SDK 的 `http_routing`、
  `properties`、`http_headers` 和 `http_body` 示例以及 fit 的 `response` 过滤器，用 Go 1.27.3 构建并以 gzip 压缩后
  提交（`tests/guests/pw-*.wasm.gz`，4.3 MB；构建方法见 `tests/guests/README.zh-CN.md`）。`respcall_test.go` 的
  情形用测试自己编码的过滤器覆盖：头部、响应体和 trailer 回调 trap 或过晚应答，在 `fail` 和 `skip` 下；被暂停的
  响应体；`proxy_on_done` trap；请求阶段的改写、本地响应、暂停和 trap；加载错误。每个跟随响应的测试都在一个实例、
  `max_streams: 1` 下检查没有遗留打开的上下文（fit 统计配置的持有数）。规则 18 的：像代理那样控制节奏写出的
  4 MiB 响应体，经过修改每个分段的过滤器，发给读得很慢的客户端；`max_body_bytes` 以下、恰好和超出；响应被挂起时
  客户端离开；以及经 h2c 的过滤器（`:authority`、被挂起的头部被修改和替换、流式响应体及其 trailer、失败响应的流被
  重置）。crate 内：`TestHostProxyWasm`、`TestHostProxyWasmResponse`、`TestHostModules`、`TestPairs`、
  `TestBodyBuffer`，以及各项上限。
- 兼容性：`wasm` 中间件的 7 个 Proxy-Wasm 字段已支持（417 个字段）；门禁的结果不变（没有夹具使用它们）。
- 与 fit 的不同（决定 011）：`proxy_on_response_headers` 发出的本地响应替换原响应发送（fit 让响应失败：fib 无法
  替换被挂起的头部）；期间写入的数据由 akio 暂存最多 4 MiB（fib 64 KB）；已释放的配置最多等待 10 分钟让其跟随的
  响应结束；字段名、值、`:path` 和 `:method` 会被检查（`BadArgument`），一个回调中对某个头部表超过 1024 次修改
  会让调用失败；fit 的 `TestResponseBigBuffer`（在调用之间保留 64 KiB 缓冲区）在 akit 中无可测试之处：akit 把每段的
  缓冲区直接交出。
- 待测量（基准测试任务，规则 30）：开发计划的 WASM 一行：请求经过 SDK 的 `http_routing`（fit 的
  `BenchmarkProxyWasm`），响应经过 fit 的 `response` 过滤器和 SDK 的 `http_headers`，对照 fit 和无插件。在 debug
  构建中，解释器在 Go guest 里把几百 KiB 的响应体转成大写，用时超过默认 100 ms 的 `timeout`
  （`large_body_slow_client` 设为 20 秒）：值得在 release 下测量。
- 检查：macOS 上 `cargo fmt --check`、`cargo clippy --workspace --all-targets -D warnings`、`cargo deny check`、
  `make compat-check`、文档检查，以及 `akit-plugin`、`akit-config`、`akit-compat`、`akit-runtime`、`akit-middleware`
  和 `akit` 的测试；gpu51（Rust 1.100）上 fmt、整个工作区的 clippy，以及同样这些 crate 经 `tools/test-timeout.sh`
  在 epoll 和 io_uring 上的测试（各 63 个测试二进制，全部通过），之后加入 h2 测试，`akit-plugin` 在两个后端上各再跑
  三轮：io_uring 的一轮中 `request_phase` 失败一次，失败在构建监听器的 engine（`akit_listener::engine(...).unwrap()`，
  任何插件运行之前；gpu51 同时有其他会话在运行），该二进制随后在 io_uring 上六次运行全部通过。

### 2026-11-25：I51b，eBPF sockmap 转发

- 配置：`listeners[].sockmap`，布尔值，省略时为 false：fit 没有的 akit 扩展字段
  （[决策 010](decisions/010-sockmap-relay.zh-CN.md)）。JSON、YAML 和 `.conf` 都能读取（`.conf` 的读取器和写出器从
  akit 扩展字段的列表 `conf/convert.rs` 中的 `EXTENSIONS` 取得它的类型），协议中没有 `tcp` 的监听器写了它会被拒绝。
  akit-config 把它与 fit 的字段分开列出（`LISTENER_AKIT_ONLY`）；兼容性门禁的 `akit_config_lists_match_the_inventory`
  检查这样的字段没有一个是 fit 已有的。`docs/compatibility.md` 增加了 akit 扩展字段一节。
- `akit-l4` 的 `sockmap` 模块：akio 的 `ebpf::Relay` 每个进程加载一次（`akit_l4::sockmap::load`），在 akit-listener
  绑定或重新加载监听器、且有监听器带 `sockmap` 时，于控制线程上加载（校验器在那里运行，从不在 I/O 线程上）；转发无法
  加载时（没有 `CAP_BPF`/`CAP_NET_ADMIN`、内核早于 6.15、不是 Linux），在标准错误上说明一次，这些监听器在用户态转发。
  监听器的标志放在图中它的 `Entry` 里，所以重新加载改变它时对新连接生效，不需要重新绑定监听器。
- 拼接：上游连接打开时（在它的 I/O 线程上，客户端的读取仍被暂停），服务的 PROXY 头和客户端之前发来的数据（按服务器名
  分流的监听器上的 ClientHello、暂停生效前的一次读取）在两个套接字都进入映射表之后、内核转发任何字节之前，直接写到上游的
  套接字（`Relay::splice_inner_with`）；然后恢复客户端的读取，只有结束和错误还会到达 akit。任何使连接不能拼接的情况
  （传输层、映射表已满、发送缓冲区不能一次写完）都让它留在 I51 的用户态转发中，不丢数据：没写出去的部分像以前一样经引擎
  发送。两侧都打开时处理器仍从已拼接的连接读到数据（拼接接管前已完成的读取），则关闭两侧，而不冒顺序错乱的风险。
  `Service::spliced()` 统计已拼接的连接。
- 计数：已拼接连接的空闲超时依据套接字的 TCP 计数（`akio::ebpf::byte_counts`：收到的字节），由服务的清扫在 I/O 线程
  之外、不持有其锁时读取。一侧结束时，另一侧要等到它的 `written` 计数达到拼接时的值，加上结束一侧此后收到的字节、减去
  用户态已读取的部分（由 akit 统计），才被关闭；在 akit 的定时线程上轮询（1 ms，翻倍直到 100 ms），或者在它停止增长达到
  服务的空闲超时后关闭；更早关闭会丢掉内核转发中仍持有的数据。
- 临时方案（规则 21）：akio 没有公开的方法从 `Conn` 取得其套接字的计数，也没有先写数据再拼接的方法；akit-l4 使用
  `Conn::inner`（公开但在 akio 文档中隐藏，akio-socks 也这样用），记录为 **akio API 需求 A170**，它也要求给出 akio-mux
  读取的 PROXY 头的长度：在此之前，监听器读取了 PROXY 头的连接在用户态转发（否则计数对不上）。没有修改 akio，没有新
  依赖；akio 仍在 `dad77a7a`。
- 与计划的差异：范围内没有。字段属于监听器（与计划所说一致），而不是服务；`udp` 监听器从不拼接（转发只针对 TCP）。
  监听器 `socks` 上 akio-socks 的 `CONNECT` 隧道没有拼接：akio-socks 有自己的 `sockmap` 选项，留待以后修改。
- 测试：`crates/akit-listener/tests/sockmap.rs`：拼接与回显（一个连接同时双向各 4 MB，同时 20 个连接），32 MB 到来时
  客户端暂不读取（内核发送缓冲区已满，即 6.15 修复的情形），客户端结束和上游结束时 8 MB 的每个字节都先转发完，空闲超时由
  任一方向的字节维持、安静的连接在超时后被关闭，服务的 PROXY 头 v1 和 v2 先发出，按 SNI 透传的 TLS 在 ClientHello 之后
  拼接（以及兜底服务），客户端的 PROXY 头使连接留在用户态。每个测试都检查：转发能加载时连接被拼接，不能加载时没有一个被
  拼接：`AKIO_TEST_EXPECT_EBPF=1` 使转发未加载的运行失败，`AKIT_TEST_SOCKMAP=userspace` 使转发已加载的运行失败。
  `crates/akit-config/tests/services.rs`：三种格式中的字段、`akit adapt` 写回的结果和检查。
- 验证：在 macOS 上，`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings` 以及 akit-l4、akit-config、
  akit-compat、akit-runtime 和 akit-listener 的测试（转发不能加载：所有连接都在用户态转发），`cargo deny check`、文档和
  兼容性检查。在 gpu51（Linux 7.0）上持有基准锁，在私有网络命名空间中（`unshare -n`，只有回环，不接触主机的接口）：
  clippy，以及同样这些 crate 在 epoll 和 io_uring 下以 root 并设置 `AKIO_TEST_EXPECT_EBPF=1` 的测试（每个 sockmap 测试的
  连接都被拼接），还有去掉所有权限（`setpriv --bounding-set -all --inh-caps -all`，`CapEff` 为 0）并设置
  `AKIT_TEST_SOCKMAP=userspace` 的 sockmap 测试（回退）：全部通过；sockmap 测试在四种模式下各运行三次。
- 待测（基准任务，规则 30）：开发计划中 eBPF sockmap 转发一行（与不带它的同一监听器和 fit 对比吞吐与 CPU、每秒连接数，
  以及 10,000 个已拼接连接时的清扫与排空）。

### 2026-11-25：I71，WASM I：运行时与 fit 的 ABI

- akio：akit 锁定到 akio `main` 的 `dad77a7a`：在 `1ae0a05f`
  （`claude/i71-akio-base`）之上加了 akio-wasm W6；`1ae0a05f` 合并了 `akio-wasm` W1–W4（`claude/akio-wasm-w4-445f36`，
  `a2373416`，ADR-125，A165）与 A33/A48（`claude/a33-a48`，`2afb267b`，ADR-126、ADR-127），二者都基于 `main` 的
  `4989cd8b`；变更日志和 ADR 索引按并集合并。它们进入 akio 的 `main` 后，要再改一次 `rev`（规则 22）。
- 运行时是 akio 自己的解释器 `akio-wasm`（[决定 008](decisions/008-wasm-runtime.zh-CN.md)）：没有 `unsafe`，没有
  第三方 crate，不需要决策 001 的例外。
- `akit-plugin`：`wasm` 中间件（`http.middleware.wasm`），如 fit 的 `plugin/wasm`，使用 fit 自己的 ABI：编译配置时
  读取（最多 64 MiB）并解码插件，加载同一代码的配置共享同一个模块；`fit_on_request` 和宿主模块 `fit`（`header_get`、
  `header_set`、`header_add`、`header_del`、`request_get`、`respond`、`log`）基于 akio-wasm 的 `Linker`，带 WASI
  preview 1（输出丢弃），未定义的导入仅在被调用时 trap；调用运行在 `akio_wasm::InstancePool` 上（`instances`、
  `queue`、作为每个任务截止时间的 `timeout`、`wait_timeout`、作为每个实例内存上限的 `max_memory_bytes`），从不在
  I/O 线程上。请求按插件看到的样子复制、挂起，并从池的完成回调恢复：插件修改后的请求字段通过 `Context::req_mut`
  成为请求的字段（`akit_runtime::set_request_headers`，与 A33 之后的其他中间件一样），然后发出插件的应答及其设置的响应字段，
  或者请求继续，由响应过滤器把插件的响应字段应用到发出的任何响应上。队列满、等待超过 `wait_timeout`、池关闭时
  应答 503；trap、panic、超过截止时间或内存上限、违反 ABI、实例无法重建时应答 500，`on_error: skip` 时请求不经插件
  继续；等待期间客户端离开的请求不调用插件。配置释放时，池在自己的线程上关闭（清理可能在 I/O 线程上运行）。插件
  info 及以上级别的日志写到标准错误，每秒 100 行。
- akit 在 fit 之外的上限：不是 token 的字段名、含 CR、LF 或 NUL 的值、不是 UTF-8 的名或值、一次调用中对请求字段
  超过 256 次修改，都会使调用失败；`instances` 最多 1024，`queue` 最多 1 Mi，`max_memory_bytes` 最多 4 GiB，实例
  启动（Go 的 `_initialize`）最多 10 秒。
- 配置：`path`、`config`、`instances`、`queue`、`timeout`、`wait_timeout`、`max_memory_bytes`（及 `max_memory`）、
  `on_error`；Proxy-Wasm 的 `vm_config`、`properties`、`response`、`max_streams` 和 `max_body_bytes` 作为暂不支持
  拒绝，Proxy-Wasm 插件加载失败并指明 I72。
- 测试：fit 的 `TestHeaders`、`TestRespond`、`TestFailures`、`TestNeverBlocks`、`TestParams`、`TestLoadErrors` 和
  `TestCalls`（fit 的 ABI），经 akit 的监听器在 epoll 和 io_uring 上运行（`crates/akit-plugin/tests/wasm.rs`），
  fit 的 Go 测试插件原样运行（akio-wasm 提交的 guest，复制到 `crates/akit-plugin/tests/guests`）；还有前置 `headers`
  中间件、中途断开和缓慢的客户端、监听器上限以内和以上的请求体。crate 内：`TestHostABI`（每个宿主函数及其失败，
  经测试自己的 guest 调用）、`TestShared`、`TestClientGone`、`TestInstanceLost` 和 `TestCloseQueued`（作为对池结果
  的判定）。行为异常的模块由 `akit_plugin::encode` 编码，即 fit 的 `encode_test.go`。TinyGo guest 未构建
  （akio-wasm W3 没有提交它）。
- 兼容性：`wasm` 的字段已支持（410 个字段）；`examples/wasm` 有效（80 个夹具：46 个有效，33 个只因暂不支持被拒绝，
  1 个跳过，没有失败）。
- 与 fit 仍有的不同：Proxy-Wasm（I72）；插件日志没有自己的 logger（尚不读取 `log.loggers`），debug 行被丢弃；路由
  的 handler 会流式读取请求体时，请求体仍会整体读完再交给插件所在的路由（A75）。
- 待测量（基准测试任务，规则 30；akio 的 W5）：请求经过 fit 的 `headers` 插件和空插件，对比不经插件和 fit；单独的
  guest 调用对比 wazero；超出实例数时的池；trap 之后重建实例（开发计划的基准测试行）。

### 2026-11-25：I74，nginx 与 Caddyfile 转换器

- `akit adapt -from nginx` 和 `-from caddyfile`（`akit_config::convert`）：移植 fit 的
  `adapter/nginx` 和 `adapter/caddyfile`，转换 fit 能转换的子集。nginx：http 块中的 server（listen、
  server_name、TLS 证书），各种 location 及其嵌套，proxy_pass 和 upstream 块（负载均衡、backup、
  超时），root、alias、index、try_files、return、rewrite、limit_req 及其 zone、client_max_body_size、
  add_header、proxy_set_header、gzip、error_log 和 access_log。Caddyfile：fit 有对应位置的全局选项，
  snippet 与 import，`{$NAME}` 占位符和 heredoc，站点地址与默认 HTTPS，按 Caddy 块的收窄方式求交的
  命名匹配器，handle、handle_path 和 route，reverse_proxy（上游、健康检查、transport）、file_server、
  respond、php_fastcgi、root、try_files、redir、rewrite、uri、header、request_header、encode、
  basic_auth、forward_auth、request_body、tls 和 log。每个未转换的指令都是一条带行号的警告，与 fit
  一致。JSON 就是 Go 的 `json.Marshal` 对相同值的输出；正则表达式按 Go 的 `regexp/syntax` 的方式检查，
  警告中逐字引用它的错误。转换后的配置按 `akit validate` 的方式检查，问题定位到源文件的行；文件按原样
  读取（与 fit 一样不跟随 nginx 的 `include`），最多 64 MiB。
- 与 fit 的不同：不是 UTF-8 的配置被拒绝（fit 会转换其中的无效字节）；在另一个 server 以明文监听的地址上
  写 `listen ... quic` 会让 fit panic，akit 则不带证书地转换；不检查 Go 对正则表达式编译后大小的上限。
- 校验器关于主机名的消息按 fit 的方式给主机名加引号（`"a..b" is not a valid host name`），不再转义。
- 测试：由 fit 自己写出的 golden 文件（`tools/adapt-golden`）：两个转换器各自处理其语料（1290 个 nginx
  配置和 1197 个 Caddyfile：fit 的样例、assistant 的迁移用例、基准测试的配置、文档中的 Caddyfile、fuzz
  种子、转换器测试中的每个字符串，以及样例和种子在十一个位置截断后的版本），akit 的 JSON、警告和错误
  与 fit 逐字节相同；`akit adapt -from` 把 10 个样例转换为三种格式（30 个用例），标准输出和标准错误逐字节
  相同；每个转换后的配置，akit 的检查与 fit 的一样接受或拒绝，只有 akit 有意不同的检查除外（在测试中
  列出）。移植了 fit 转换器的单元测试（解析器、位置、匹配、单位、上限），以及 fit 两个转换器的端到端
  `TestServe`（`akit run` 运行转换后的配置：文件、响应、重定向、替换前缀的代理），并验证损坏的配置
  （语料中的每个输入在各处截断、逐个去掉每个括号和引号，超过 10 000 个输入）不会 panic。
- 发现但留待后续：akit 不拒绝与监听器端口冲突的 Admin API 地址（fit 会报
  `listener "x" listens on TCP :2019 already ...`）。
- 无 akio API 需求（A168–A170 未使用）；无新依赖。无基准测试：控制面（规则 24）。
- 已检查：macOS 上 `make check`。gpu51 上用 epoll 和 `AKIO_TEST_BACKEND=uring` 经
  `tools/test-timeout.sh` 运行所涉 crate（`akit-router`、`akit-config`、`akit`）的测试，排在基准测试锁
  之后（`/root/akit-i74.log`）。

### 2026-11-25：I70，`akit build`

- `akit build`（`crates/akit/src/build_cli.rs`），即 fit 的 `fit build`，参数、消息和退出码相同：
  `-o`、`-with`（可重复：`name`、来自 crates.io 的 `name@version`、`name=dir`，以及 fit 的 Go 模块
  用不到的 `name=git+url#reference`）、`-without`（akit 的模块 crate：`akit-handler`、
  `akit-middleware`、`akit-cache`、`akit-proxy`）、`-akit`（akit 仓库的提交或标签、它的另一个仓库，
  或一个目录；省略时为构建本 akit 所用的源码，就像 fit 基于自己的版本构建）和 `-keep`。
- `akit::build`（`crates/akit/src/build.rs`），即 fit 的 `plugin/build`：生成一个 Cargo 包，其中
  `main.rs` 用各 crate 的 `MODULES` 列表调用 `akit::main`（决策 003：组合列表而不是 import），
  `Cargo.toml` 使用 akit 的 release 配置并自成一个工作区；用 `PATH` 上的 `cargo`（或 `CARGO` 指定的）
  构建，从 cargo 的 JSON 消息中找到可执行文件并复制到 `-o`。从 akit 的目录构建时使用其
  `Cargo.lock`，并为可能通过 git 依赖 akit 的模块用该目录替换 akit 的仓库（`[patch]`，即 fit 的
  `go mod edit -replace`）。
- `akit` crate 既是库也是可执行文件：`akit::main(version, lists)` 是命令行（`src/main.rs`，现为库的
  `cli` 模块，留在原处以便其他分支对它的修改能合并；那些分支合并后可改名为 `cli.rs`），
  `akit::STANDARD` 是 akit 自带的列表，`src/bin/akit.rs` 是可执行文件。`akit-runtime` 重新导出
  `akio_http` 和 `akit_config`（作为 `config`），因此模块 crate 只需依赖它。
- `akit-config`：模型不认识的模块名不再被拒绝，而是保存为 `Handler::Module`（`ModuleRef`：名字、
  是否为中间件、书写时的参数），由注册表在编译时以 fit 的消息在 fit 的路径上报告
  （`routes[0].handler.module: there is no module "nope" in http.handlers (it has …)`），此前 akit
  报告的是 "there is no handler module"。fit 有而 akit 尚未支持的名字仍按尚未支持拒绝。
- 模块开发指南 [模块](modules.zh-CN.md)，以及示例模块 `crates/akit/tests/testdata/hello-module`
  （即 fit 的 `testdata/hello`：添加 `X-Hello` 的 `hello` 中间件，严格解码其参数）。
- 测试：移植了 fit 的 `TestParseModule`、`TestImports`、`TestBuildFlags`、`TestBuildErrors`（无法获取的
  依赖在 cargo 离线时报错）和 `TestBuildNoWorkingDirectory`；以及 fit 的 `TestBuildWithModule`：用本检出
  构建带示例模块、不带 `akit-cache` 的 akit（debug 构建，target 目录保留在 `target/tmp` 下），其可执行
  文件报告自己的版本、检查模块的参数、拒绝 `cache`，并通过该模块为一条路由提供服务。
- 未完成：其他种类的模块（provider、四层 handler）；针对 provider 的 `-without`（provider 是内置的）；
  被中断的构建会留下临时包。
- 无基准测试：命令行与控制面（规则 24）。
- 已检查：macOS 上的 `make check`。gpu51 上，受影响的 crate（`akit-config`、`akit-runtime`、
  `akit-middleware`、`akit-admin`、`akit-compat`、`akit`）在 epoll 和 `AKIO_TEST_BACKEND=uring` 下经由
  `tools/test-timeout.sh` 运行，在共享锁下排在基准测试之后：日志 `/root/akit-i70.log`。

### 2026-11-25：I65，打包

- 发布包（`scripts/dist.sh`、`make dist`）：`akit_<version>_<os>_<arch>.tar.gz`，内含 LICENSE 和 README，另有
  `checksums.txt`，与 fit 的 `dist.sh` 相同。Linux amd64 和 arm64 是用 `rust-lld` 链接的静态 musl 二进制，任何
  主机无需交叉工具链即可构建（这台 Mac 构建了全部四个，macOS 的用 Xcode 的链接器）；构建时的 `AKIT_VERSION`
  决定 `akit version` 打印的版本，对应 fit 的 `-X main.version`（[决策 009](decisions/009-packaging.zh-CN.md)）。
  `scripts/check-dist.sh`（`make dist-check`）检查校验和、每个包里的文件、每个二进制的平台、Linux 二进制是否静态，
  并运行本机的那个。
- 容器镜像（`Dockerfile`、`make image`）：沿用 fit 的，musl 二进制在构建机的平台上交叉编译并用 `rust-lld` 链接
  （多平台构建无需模拟），放在 distroless 的 static 镜像上，以用户 65532 运行，`/etc/akit/akit.json` 处有一个占位
  配置。已在本机构建并运行：14.1 MB，`akit version` 正常，占位页面可访问。
- fit 的 Helm chart（`deploy/helm/akit`）和 systemd unit（`deploy/systemd`：`akit.service`、`akit@.service`、
  sysusers，以及 fit 的 `test.sh`），换成 akit 的名字和路径；chart 的控制器是 `lesismal.github.io/akit`。
  `deploy/README.md` 记录了它们，以及发布需要维护者做的事。
- `release.yml`：打 `v*` 标签时，在 Linux 和 macOS runner 上构建二进制，检查后与打包好的 chart 一起计算校验和，
  附到 GitHub Release；把 linux/amd64 和 linux/arm64 镜像推送到 ghcr.io；标签版本与 chart 的 `appVersion` 不一致
  时拒绝。手动运行时只构建。CI 的 `packaging` 任务构建静态二进制，以 root 运行 systemd unit 的测试，构建并运行
  镜像，并以三种方式渲染 chart，用 akit 校验其配置。
- 测试：`crates/akit/tests/packaging.rs`（占位配置有效且能服务；unit、Dockerfile 和 chart 使用 akit 的命令和
  相同的配置路径；`akit version`）；`make check` 中的 `make packaging-check`（脚本能解析，装了 shellcheck 时
  运行它）。
- 留给维护者：发布（第一个标签；把 ghcr.io 的包设为公开）、签名（镜像的 cosign 或来源证明，校验和之外的签名，
  macOS 二进制的公证）、chart 仓库。本机没有运行：发布工作流、CI 的 packaging 任务、`helm lint`、
  `systemd-analyze`（这台 Mac 上没有 helm 和 systemd）。
- 没有 akio API 请求；没有新依赖。没有基准测试（规则 30）：基准表新增的一行要测量静态二进制使用 musl 的分配器
  与 glibc 构建相比是否损失吞吐量。

### 2026-11-25：实现 akio 需求 A33 与 A48：就地修改请求，获知请求的结束

- akio（分支 `claude/a33-a48`，`2afb267b`，ADR-126 与 ADR-127，尚未进入 `main`），已在此锁定：
  `Context::req_mut`（A33）；`Context::on_end`、`Detached::on_end` 和 `End { Sent, Gone }`，每个钩子恰好运行一次，
  `on_gone` 与它们并存（A48）。在 akio 中经 HTTP/1.1、HTTP/2 和 HTTP/3，在 epoll 和 io_uring 上测试。
- A33：`headers` 中间件（I40）、`basic_auth` 和客户端证书检查（I42）、ForwardAuth（I43）、JWT（I44）以及缓存的
  重新验证（I67）就地修改请求字段；`rewrite`（I40）修改请求目标，保留 query；akit-listener 把请求目标的路径规范化
  （I11），如同 fit 的 listener 修改请求的路径和 `RequestURI`。临时方案随之去掉：`REQUEST_FIELDS` 和
  `request_headers`（handler 读取 `ctx.req().headers`），`CANONICAL_PATH`（handler 读取 `ctx.path()`，代理发送
  请求目标）。akio 读取请求的地方也看到修改：`static` handler 的条件请求和范围请求字段、响应过滤器。访问日志写的是
  listener 留下的请求目标，只为记录日志的请求复制一份，因为 fit 的 `rewrite` 不改动 `RequestURI`。
- A48：`concurrency_limit`（I41）在 `on_end` 时归还请求的名额：流式响应在消息体结束时，而不是响应头发出时；等待中的
  请求在客户端离开时立即归还，而不是等到 handler 放开它（fit 的 `TestConcurrencyClientGone` 现在与 fit 一致）。
- 测试：`akit-middleware` 的 `tests/limits.rs`：客户端离开时立即归还名额，此时 handler 仍持有请求，且只归还一次；
  流式响应在响应头之后仍占着名额直到完成，被截断的流式响应在客户端离开时归还。`headers`、`rewrite`、认证中间件、
  缓存、代理、listener 的规范化路径、`static` 和 `fastcgi` 的现有测试在就地修改请求后都通过。
- 与 fit 仍有的不同：访问日志、指标和 span 把流式响应的耗时算到响应头，大小写的是 `Content-Length`（fit：在
  `OnFinish` 时，写出的字节数）：`End` 不带字节数；留作后续（记录在 A48 中）。
- 待测量（基准任务，规则 30）：带 `concurrency_limit` 的 proxy 场景（钩子：每个请求一个 `Arc`、两次装箱和一次
  对端离开登记，原先是一个装箱的过滤器），带 `headers` 的（字段就地修改，而不是编码进 local 再由代理解码），以及
  开启访问日志的（每个记录日志的请求复制一次请求目标）。

### 2026-11-25：实现 akio 需求 A57、A58、A14、A15 与 A92，解除相应的拒绝

- akio（分支 `claude/a57-a58-a14-a15-a92`，`7d1b1670`，ADR-123 与 ADR-124，尚未进入 `main`）：
  `RootCerts::store()`（A57）；`ServerConfigBuilder::handshake_only_alpn`（A58）；`Files` 的
  `index_files`、`redirect_directories`、`mime_types`（`MimeTypes::system`）、`default_type`、`sniff`、
  `error_response`、`serve_prefixed`，以及 403、只打开普通文件、带尾斜杠的文件回 404、允许 `:`（A14）；
  `max_cache_bytes` 与 `disk_timeout`（A15）；设置了流编码的文件消息体在磁盘池上编码（A92）。akit 锁定到它。
- `static`（I12、I13）：`index` 接受 fit 的列表（`[]` 表示没有），读取 `max_cache_bytes`（64 MiB）与
  `default_type`（按 Go 的 `mime.ParseMediaType` 检查）；不带斜杠的目录被重定向（301，保留查询串，
  `try_files` 时不重定向），带斜杠的文件回 404，不可读的文件回 403，FIFO 立即回 404（从不打开），类型用
  Go 的表加系统的 `mime.types`，没有 `default_type` 时嗅探，错误正文是 fit 的（`404 Not Found\n`），
  `strip_prefix` 之下的目录列表以完整路径为标题，文件池在 fit 的 10 秒内未回答的请求回 503。测试移植自
  fit 的 `TestServe`、`TestNoIndex`、`TestPermission`、`TestCacheLimits`、`TestDefaultType`、
  `TestStripPrefix` 与 `TestErrors`。
- I32：没有给出根证书时，`Ocsp` 对系统根证书验证来自文件的证书，与 fit 的 `x509.SystemCertPool`
  相同；TLS 监听器上协商 `acme-tls/1` 的连接在握手完成后关闭、不提供任何服务（I32、I34），在真实引擎上测试。
- I47：`compress` 也压缩从磁盘发送的静态文件（三种编码，HTTP/1.1 与 HTTP/2），直到消息体发送完才释放名额。
- 兼容性：支持 `max_cache_bytes` 与 `default_type`（371 个字段）。
- 与 fit 仍有的差别：文件池超时的 503 在 akio 中测试（挂起的磁盘），没有通过 akit 测试；重定向的
  `Location` 是原样发送的路径，没有解码。
- 待测量（基准任务，规则 30）：带 `compress` 从磁盘发送静态文件（分片在文件池上读取并编码）与 fit 对比；
  `Files` 改动后静态 handler 的热路径（缓存命中、双环缓存）。

### 2026-11-25：实现 akio 需求 A9、A23 和 A134，解除相应的拒绝

- akio（分支 `claude/a134-a23-a9`，`7521e6b6`，ADR-121 与 ADR-122），本仓库已锁定：
  `Listener::unix` / `unix_from_std` / `unix_mode`（A23）；两个 TLS 构建器上公开的 `tls12_only`（A9）；
  HTTP/2 或 HTTP/3 流上的 CONNECT 立即分派，由 `connect_proxy` 在该流上中继，`H2Client` 也能发送这样的
  CONNECT，客户端的 END_STREAM 通过 `Conn::shutdown_write_after_send` 传下去（A134）。
- A9：接受 `listeners[].tls.max_version: "1.2"`；akit-tls 用 `tls12_only` 构建监听器的 TCP 配置（重载时会
  比较它），HTTP/3 仍为 TLS 1.3。
- A23：接受 `admin.address: "unix/<path>"`（只有 `"unix/"` 时按 fit 的消息拒绝）：Admin API 监听在权限为
  0600 的套接字上，替换残留的套接字，拒绝正在使用的，那里不检查 `Host`（fit 的 `loopback` 为 false），API
  停止时删除文件；`akit run` 打印 `admin API on unix/<path>`。
- A134：`forward_proxy` 不再对 h2 或 h3 流上的 CONNECT 应答 501（I55 记录的权宜之计），而是像 fit 一样建立
  隧道。
- 测试：fit 的 `TestTLSMaxVersion`（TCP 部分：HTTP/1.1 和 h2 上为 TLS 1.2，只支持 TLS 1.3 的客户端被拒绝；
  `crates/akit/tests/https.rs`）、`TestUnixSocket`（`crates/akit-admin/tests/admin.rs`）和
  `TestConnectH2C`（`crates/akit-handler/tests/forward_proxy.rs`，另有 h2c 上被拒绝的目的地），以及配置用例。
- 兼容性闸门：`examples/admin` 有效（80 个样例：42 个有效，37 个仅因尚不支持而被拒绝，1 个跳过，无失败）。
- 未测量（规则 30）：h2 和 h3 上的 CONNECT 中继相对 HTTP/1.1 的表现，留给基准任务。

### 2026-11-25：I62，共享证书存储

- `akit-config` 读取 fit 的 `acme.shared_storage`（fit 的决策 014），包括 `redis`、`etcd` 和 `consul`
  三种类型以及 `prefix`、`lock_ttl` 和 `request_timeout` 的全部字段，带 fit 的默认值、检查、消息和位置
  （`checkSharedStorage`、`checkRedisStorage`：恰好一种类型，`lock_ttl` 至少 10s，Redis 地址是主机和端口，
  `username` 需要 `password_file`，TLS 文件需要 `tls`，`storage` 和 `shared_storage` 不能同时设置）。
  模型在 `shared_storage.rs` 中。
- `akit-tls`：`SharedStore`，一个声明自己 `shared` 的 `Storage`（trait 新增的默认方法，另有 `close`），
  对应 fit 的 `sharedstore.Store`。Redis 以 RESP2 经 akio 建立的至多 4 个连接访问（设置 `tls` 时走 TLS，
  建立时 `AUTH` 和 `SELECT`），其 I/O 线程只向有界收件箱追加数据；etcd 经其 v3 JSON 网关，一次一个端点，
  先认证，令牌被拒时再认证一次；Consul 的 KV 存储带 ACL 令牌和数据中心。都在存储自己的 akio 引擎上；
  调用在调用者（控制面）的线程上等待，各受 `request_timeout` 和截止时间约束。锁都是租约式的：带随机令牌的
  `SET NX PX` 和检查令牌的脚本；etcd 租约加 create-revision 事务；`Behavior: delete` 的 Consul 会话；
  每把锁各有一个线程每隔 `lock_ttl` 的三分之一续期，`unlock` 和 `close` 时归还。值至多 4 MiB，列举至多
  100,000 个键，同时至多持有 256 把锁。
- 使用共享存储时，ACME 在挑战进行期间把每个挑战的应答保存在其中（`challenges/<CA>/http-01/<token>`、
  `challenges/<CA>/tls-alpn-01/<name>`），并写一个“挑战进行中”标记（两分钟，每 30s 重写）；在 CA 检查前
  等待 1.25s。其他实例只在标记有效时（至多每秒读一次）查找挑战，查找在自己的线程上进行，同时至多 16 个、
  每个客户端（IPv4 地址或 IPv6 /64）至多 4 个，各在 5s 内；查找落空的名字一秒内不再查找。明文监听器把
  自己没有的令牌请求分离出来，由查找结果应答（没有时回答 404，而不是交给路由，与 fit 相同）；只提供
  `acme-tls/1` 的握手经 akio 的 `CertDone` 等待 TLS-ALPN-01 查找，从不占住 I/O 线程。
- 测试：fit 的 `sharedstore` 测试（RESP2 回复，经 TLS 并带用户和数据库的 Redis，其拨号错误和各种回复，
  繁忙的连接池，etcd 的端点、应答和认证，Consul 的，存储自身的错误和锁）在假服务器上运行；fit 的
  `TestStores`（键共享，锁轮流获取、超过 TTL 时续期、被遗弃后失效）在具有 Redis、etcd 和 Consul 语义的假
  服务器上运行；fit 的 `shared_test.go`（查找的总量和按客户端的上限、标记门、落空记录、监听器应答另一实例的
  TLS-ALPN-01 挑战、内存中的优先）；以及在 Pebble 上，两个管理器共用一个 Redis，CA 只能到达第二个：经
  HTTP-01 和 TLS-ALPN-01 都只得到一张证书、只签发一次。
- 兼容性门禁：`acme.shared_storage` 及其 26 个字段已支持；`acme-shared-storage` 示例有效（80 个夹具，
  42 个有效，37 个只因暂不支持被拒绝，1 个跳过）。
- 没有 akio API 请求（A159–A161 未使用）；没有新依赖。不做基准测试：属于控制面（规则 24）。

### 2026-11-25：I73，`akit adapt` 与 `akit fmt`

- `akit adapt`（`crates/akit/src/adapt.rs`）：fit 的 `fit adapt`，参数（`-c`，可重复或用 `-` 表示标准输入，
  `-from json|yaml|fit`、`-to json|yaml|fit`、`-o`）、错误和退出码与 fit 一致：配置按 `akit validate`
  的方式读取并检查（不编译），再按 fit 的写法逐字节写出。akit 暂不运行的字段原样转换并给出警告，
  因此 fit 能读的配置都能转换。`-from nginx` 和 `-from caddyfile` 暂不支持（I74）。
- `akit fmt`（fit 没有）：把配置的每个文件按其自身格式重新写出，写法同 `akit adapt`，输出到标准输出、
  原地写回（`-w`），或列出会变化的文件（`-l`）；先把所有文件一起检查。注释不会保留，所以 `-w`
  不改动含注释的文件并报出其名字；JSON 文件以一个换行结尾，因此格式化是稳定的。
- `akit_config::adapt`：fit 的 adapter set（单个 JSON 文件的字符串和数字保持原样，YAML 或 `.conf`
  文件的按 Go 的 `json.Marshal` 写出，多个文件按 fit 的规则合并，冲突消息一致）及其三个写出器：
  Go 的 `json.Indent`（与 fit 一样保留结尾空白）；`adapter/yaml.FromJSON`，移植了 go-yaml v3.0.5
  的 emitter 和 resolver 中用到的部分（块风格、四种标量风格及其分析，会被读成布尔、数字或时间戳的
  字符串按 Go 的 `strconv` 和 `time.Parse` 规则加引号）；以及 `adapter/fitconf.FromJSON` 及其紧凑化，
  类型取自 `.conf` 读取器使用的字段清单（简写、字节单位、按模块的 params）。
- `.conf` 读取器把 float64 放不下的数字（`1e400`）读成字符串，与 fit 的 `isNumber` 一致。
- 测试：由 fit 自己写出的 golden 文件（`tools/adapt-golden/run.sh`，带 fit 的 `modules/standard`）：
  `fit adapt` 处理 akit 保存的 fit 模板（单文件与 `conf.d`，三种格式）、74 个示例和 `cli/testdata`
  配置，各转换为三种格式（249 个用例），以及 fit 的两个写出器处理 3819 个文档（用例的输出、fit
  写出器的测试，以及由各写出器特殊处理的字符串和数字构造的文档）；akit 的输出在每个用例上逐字节相同。
  每个模板和示例经 YAML 与 `.conf` 往返后得到相同的配置，再次写出得到相同的文本。移植了 fit 的
  `adapt` 命令行测试，并测试了合并、注释和 `fmt`。
- 无基准测试：控制面（规则 24）。
- 已检查：macOS 上 `make check`；gpu51 上用 epoll 和 `AKIO_TEST_BACKEND=uring` 经
  `tools/test-timeout.sh` 运行所涉 crate（`akit-config`、`akit`）的测试：全部通过，只有 `akit` 的
  `reloads_under_traffic_with_no_failed_request` 在 io_uring 下失败一次，这是负载下已知的偶发失败，单独重跑 3/3 通过。

### 2026-11-25：I60，Kubernetes Gateway API

- `akit-config`：`kubernetes` provider 不再必须设置 `ingress`：它与 fit 的一样总是服务 Gateway API，
  `ingress` 再加上 Ingress。读取 `addresses`，检查与 fit 相同（IP 地址或主机名，不带通配符）。
  `allow_external_name_services` 仍暂不支持（它需要上游的 `deny_internal`）。
- `akit-provider::kubernetes`：在 I59 的转换器上移植 fit 的 `gateway.go`、`grpc.go`、`l4.go` 和
  `status.go`（新文件 `gateway.rs`、`status.rs`）：`controller_name` 的 GatewayClass 的 Gateway，
  按创建时间排序，每个监听器成为 akit 在其端口和协议族上的一个监听器（HTTP；HTTPS 带其 Secret 的证书，
  与按服务器名区分的 TLS 透传共用一个端口；TCP；UDP，名为 `<name>-<port>-udp`），按 fit 的方式被拒绝
  （未知协议、没有端口、在 TLS 监听器上终结 TLS）、冲突（端口已被另一协议族占用）或引用未解析
  （certificateRefs、路由类型）；HTTPRoute（每个 match 一条路由：Exact、PathPrefix 和
  RegularExpression 路径，精确匹配的头和查询参数，方法，以 API 的优先级作为路由优先级，主机名与监听器的
  取交集，`*.` 写作 `**.`；RequestHeaderModifier、ResponseHeaderModifier、URLRewrite、
  RequestRedirect、一个 RequestMirror；带权重的后端；按 API 的规定回答 500 和 503）和 GRPCRoute
  （方法作为路径，经 h2c）；TLSRoute、TCPRoute 和 UDPRoute 成为服务（TLSRoute 的主机名作为其
  `server_names`，监听器的每个名字只归一条路由，多个后端合为一个带权重的上游）；按 section 和端口的
  parentRefs，allowedRoutes 的命名空间（Same、All、按 Namespace 标签的 Selector）和类型，对其他命名
  空间中 Service 和 Secret 的 ReferenceGrant。akit 的 GatewayClass、Gateway（写入 `addresses`）、
  监听器（supportedKinds、attachedRoutes、Accepted、Programmed、ResolvedRefs、Conflicted）以及每条
  路由在 akit 每个 Gateway 上的状态（Accepted、ResolvedRefs、PartiallyInvalid；保留其他控制器的条目；
  最多 32 个）以 JSON merge patch 写回，patch 写明生成时的 resource version，只在有差异时写，状态不变
  的条件保留其时间。写入在 provider 的线程上逐个进行，最新的一组替换上一组剩下的；因对象已变（409）或
  已删除而被拒绝的写入留给 watch 的下一次变化，其他失败只报告一次。客户端新增 `PATCH .../status`。
  所 watch 的类型与 fit 相同，TLSRoute 取 v1、v1alpha3 或 v1alpha2，ReferenceGrant 取 v1beta1 或
  v1，以服务器提供的第一个为准；只有设置了 `ingress` 才 watch Ingress 和 IngressClass。服务器不提供的
  类型与 fit 一样提示 "(is the Gateway API installed?)"。
- 测试：`crates/akit-provider/src/kubernetes/gateway_tests.rs` 移植 fit 的
  `TestTranslateGateway`、`TestTranslateOptions`、`TestPrecedence`、`TestIntersect`、
  `TestSelects`、`TestFilters`、`TestMirror`、`TestHTTPS`、`TestGRPCRoute`、`TestL4Routes`、
  `TestTranslateEdges`（ExternalName Service 被拒绝，因为 `allow_external_name_services` 暂不支持）、
  `TestSecretTooLarge`、`TestStatus`、`TestStatusEdges` 以及新类型的 `TestDecodeKinds`，每个部分都
  检查为 akit 接受的配置；`TestIngressPorts` 和 `TestIngressEdges` 补上了占用端口的 Gateway；
  `status.rs` 的 `TestObjectPath`。`crates/akit-provider/tests/kubernetes.rs`：假 API 服务器应用状态
  merge patch（resource version 不同时返回 409），并运行 Gateway API 版的 fit `TestProvider`（两个
  版本都不提供的类型、分页、经 watch 来去的路由）和 `TestProviderStatus`（状态写一次后不再写，被拒绝的
  写入再次进行，v1alpha2 的 TLSRoute，HTTPS 监听器的证书文件随其 Secret 写入和删除）。
- 一致性测试套件：[tests/conformance](../tests/conformance/README.zh-CN.md) 按 fit 决策 012 的方式，
  用 fit 的套件模块针对 `akit run` 和 kind 集群运行（`run.sh`，akit 的 GatewayClass `akit`，controller
  为 `lesismal.github.io/akit`）。它需要集群，由维护者运行；`GATEWAY-HTTP` 尚未针对 akit 运行。
- 兼容性门禁：支持 `providers[].kubernetes.addresses`；`make compat-bless`：
  `examples/provider-kubernetes` 有效（80 个夹具：42 个有效，37 个只因暂不支持被拒绝，1 个跳过，没有
  失败）。清单：370 个已支持。
- 没有 akio 需求（A156–A158 未使用）：akio 的客户端已能经 HTTP/2 和 HTTP/1.1 发送带消息体的 PATCH。
  控制面：不做基准测试（规则 24）。
- 检查：macOS 上 `make check`；gpu51 上在基准锁下，`akit-provider`、`akit-config`、`akit-compat` 和
  `akit` 在 epoll 和 `AKIO_TEST_BACKEND=uring` 下通过，每个测试二进制都经 `tools/test-timeout.sh`
  运行（规则 31）。

### 2026-11-24：I61，Consul 与 etcd provider

- `akit-config` 读取 fit 的 `providers` 中 `consul`、`consul_kv` 和 `etcd` 三种类型的全部字段，
  默认值和检查、报错信息与位置都与 fit 相同（`checkProviders`：每个只有一种类型、名字的格式、
  不能用基础配置来源的名字、跨文件唯一；`checkEndpoint`、`checkTLSFiles`、标签前缀、`username`
  与 `password_file` 成对）；`docker` 和 `kubernetes` 由 I58 和 I59 读取。三种类型加入它们共用的
  模型（`providers.rs`），各自的设置在 `providers_kv.rs` 中。名字的问题
  报在 provider 对象的位置上，fit 报在名字上。
- `akit-provider`：`watch`，fit 的 `provider/internal/watch` 跟随器，跑在自己的线程上：打开源，
  在第二个线程上 watch，watch 就位后、变化后（去抖 200ms）以及每个 `refresh_interval` 读取整个
  源，部分有变化（或到了间隔）时发送，每个问题在持续期间只记录一次，失败后间隔 1s 到 30s 重试；
  `Stop` 取消一轮中的等待和 akio 请求（经其 `CancelToken`），`stop` 返回前汇合所有线程。`api`：
  每个 provider 一个 akio 引擎（一个客户端线程），TLS 来自文件，请求从 provider 自己的线程发出并
  等待，每个都有超时，应答最多 32 MiB。`kvpart`（fit 的）：按键的顺序合并各键的部分，
  `max_keys`。`labels`：fit 的 `provider/labels`（也供 Docker provider 使用，I58）。`consul`：
  catalog provider（用阻塞查询轮询 `/v1/catalog/services` 和 `/v1/health/state/any`，按标签启用的
  服务用 `?passing` 读取，标签当作 label，`max_services`）和 KV provider（`/v1/kv/<prefix>
  ?recurse`，值为 base64），每次 watch 时从 `token_file` 读取 token，`dc`；索引回退则从头开始，
  小于 1 当作 1，一次轮询中的查询间隔至少 1s。`etcd`：按 fit 的方式使用 etcd v3 的 JSON 网关
  （fit 的决策 013），而不是计划中所写的 gRPC：在前缀的范围上带 `limit` 调用 `/v3/kv/range`，
  `/v3/watch` 流式返回，在客户端的 I/O 线程上逐行解析（每行最多 32 MiB），须在
  `request_timeout` 内创建，被取消的 watch（如压缩之后）重新开始，依次使用各个端点，设置了
  `username` 时从 `/v3/auth/authenticate` 获取 token。比 fit 多一点：watch 请求
  `progress_notify`，静默 15 分钟后放弃（规则 10）。`API_KINDS` 为 `Kinds` 提供这三种类型。
- `akit run`：按类型创建配置中列出的 provider，加入 `Configs` 并启动（`akit: provider <name>
  watching ...`，与 fit 的措辞相同），按 I57 的方式应用它们的部分，akit 退出前停止它们。与 fit
  一样只在启动时读取。
- 测试：`crates/akit-provider` 移植了 fit 的 `TestFollower`、`TestFollowerInterval`、
  `TestFollowerWatchEnds`、`TestFollowerReadFailsAfterChange`、`TestPart`、`TestParse`、
  `TestParseRefused`，Consul 的 `TestTranslate`、`TestCatalog`、`TestCatalogMaxServices`、
  `TestKV`、`TestEscapePath`、`TestKinds`、`TestOpenFails`、`TestCatalogReadFails`、
  `TestKVReadFails`、`TestGetFails`、`TestPollIndexZero` 和 `TestTranslateNoAddress`，以及 etcd 的
  `TestProvider`、`TestProviderMaxKeys`、`TestProviderWatchNotCreated`、
  `TestProviderBadPassword`、`TestRangeEnd`、`TestReadWatch`、`TestReadWatchNoResult`、
  `TestKind`、`TestOpenFails` 和 `TestCallFails`，针对跑在自己线程上的 Consul 假服务（阻塞查询）
  和 etcd 网关假服务（流式 watch）；`crates/akit-config/tests/providers.rs` 移植 fit 的 provider
  用例；`crates/akit/tests/providers.rs`，端到端：`akit run` 提供某个 Consul 键中的路由，键变化后
  提供新的路由。
- 兼容性门禁：新增 34 个字段；`make compat-bless`：`examples/provider-consul`、
  `provider-consul-kv` 和 `provider-etcd` 有效（80 个夹具：41 个有效，38 个只因暂不支持被拒绝，
  1 个跳过，没有失败，已合入 I58 和 I59）。
- 没有 akio 请求（A141–A143 未用）：akio 的 HTTP 客户端已能流式接收应答、按 token 取消并为每个
  阶段计时。属于控制面：无需基准（规则 24）。
- 已检查（合入 main 之后：I55、I58、I59、I75、规则 31）：macOS 上 `make check`；gpu51 上持 bench
  锁，`akit-provider`、`akit-config`、`akit` 和 `akit-compat` 在 epoll 和
  `AKIO_TEST_BACKEND=uring` 上通过，每个测试二进制都经 `tools/test-timeout.sh` 运行（规则 31）。

### 2026-11-24：I59，Kubernetes Ingress

- `akit-config` 读取 fit 的 `providers`（新文件 `providers.rs`：`Provider`、`ProviderKind`，fit 的
  `checkProviders`：每个只有一种类型，名字合法、唯一且不是基础配置的来源名）以及 `kubernetes` 类型的
  `kubeconfig`、`context`、`controller_name`、`namespaces`、`listen_host`、`backends`、`max_objects`、
  `request_timeout`、`ingress`（`http_port`、`https_port`）和 `certificates_dir`，带 fit 的默认值和检查。
  暂不支持：没有 `ingress` 的 kubernetes provider（Gateway API，I60）、`addresses`（Gateway 的状态，I60）
  和 `allow_external_name_services`（需要上游的 `deny_internal`）；consul、consul_kv 和 etcd
  类型（I61）。该类型是同一 `providers.rs` 中与 I58 的 Docker 并列的一个 `ProviderKind` 变体。门禁中的 `provider-kubernetes` 示例现在只因 `addresses` 被拒绝。
- `akit-provider::kubernetes`，即 fit 处理 Ingress 的 `provider/kubernetes`：基于 akio 客户端的 akit
  自有客户端（fit 的决策 011），按每页 500 个分页 list 每种对象（每页最多 64 MiB，continue 令牌过期时
  重新 list），再从 list 的资源版本 watch（bookmark，每次 watch 五分钟，十分钟没有任何数据即放弃，
  410 时重新 list）；服务器是 `https` 且选择 `h2` 时经一个 HTTP/2 连接，否则经 HTTP/1.1（明文，或 ALPN
  选择了它）；kubeconfig（context、server、CA、`tls-server-name`、`insecure-skip-tls-verify`、客户端证书、
  `token` 或每分钟重读的 `tokenFile`；拒绝 exec 和 auth-provider 插件）、pod 的服务账号，或 `$KUBECONFIG`
  与 `~/.kube/config`。watch 事件到达时在 provider 引擎的 I/O 线程上解码并应用到该类型的存储（短暂加锁；
  单个事件最多 16 MiB；超过 `max_objects` 的被略去并报告）；provider 自己的线程负责 list、重新 watch、
  退避、变化后等待 `debounce`、在部分变化时以及每个 `resync` 发送。转换器即 fit 的 `ingress.go` 加上它
  用到的 `gateway.go` 部分：控制器为 `controller_name` 的 IngressClass 的 Ingress（类为默认时也包括未指定
  类的），限于所监视的命名空间；每个路径成为 `<name>-<http_port>` 上的路由，其 `tls` 主机还在
  `<name>-<https_port>` 上，Secret 的证书链和密钥写入按哈希命名的文件（不再引用时删除；没有
  `certificates_dir` 时用 provider 自己的目录，停止时删除）；fit 的优先级、路径类型和注解
  （`fit.lesismal.github.io/`：`ssl-redirect`、`rewrite-target`、`backend-protocol`、`limit-rps`、
  `proxy-body-size`、`allow-source-range`、`request-timeout`）；Service 端口成为其就绪端点或 cluster IP
  的上游，由注解或 `appProtocol` 决定 h2c。略去的内容在持续期间只报告一次。与 fit 一样，akit 不写
  Ingress 的状态。
- `akit run` 启动其启动配置所列的 provider（fit 的 `startProviders`：
  `akit: provider k8s watching Kubernetes for the Ingresses of ...`），关闭时停止它们；改变 `providers`
  的重载会给出警告（需要重启）。
- 测试：`crates/akit-provider/src/kubernetes/tests.rs` 移植 fit 的 `TestIngress`、`TestIngressPorts` 和
  `TestIngressEdges`（Gateway 占用端口的部分除外，I60），每个部分都检查为 akit 接受的配置，以及
  `TestParseSize`、`TestKubeconfig`、`TestLoadConfigEdges`、`TestTokenRotates`、`TestCertFiles`、
  `TestDecodeKinds` 和 `TestInformerWatch`（事件按各种大小切分）；`crates/akit-provider/tests/kubernetes.rs`
  让 provider 对接一个基于 akio HTTP 服务器的假 API 服务器（fit 的 `apiServer`）：经 HTTP/2 的
  `TestProvider` 和 `TestProviderIngress`（分页、过旧的 watch、结束的 watch、Secret 的字段选择器），经明文
  和 TLS 的 HTTP/1.1，未提供的对象类型，`TestProviderUnauthorized`，`TestProviderSetupFails`，以及端到端的
  Ingress TLS（文件写入与删除）。没有 kind 集群：假服务器是 fit 的做法，也不需要 Docker。
- 控制面：没有基准测试（规则 24）。

### 2026-11-24：I55，SOCKS 与 CONNECT

- `akit-config` 读取 fit 的正向代理 `config.ForwardProxy`：监听器的 `socks`（配合 `socks` 协议，
  `Protocol::Socks`）和 `forward_proxy` 处理器的参数（`clients`、`allow`、`deny`、`ports`、
  `connect_timeout`、`idle_timeout`；`ForwardProxy`、`Destination`），带 fit 的默认值（10 秒、5 分钟）和检查、
  消息与位置："socks needs the listener's socks"、"socks is for a listener whose protocols include socks"、
  `checkForwardProxy` 的各条，以及 "a socks listener, not routes"（位置在路由上，fit 指向监听器名，与 akit 对
  tcp、udp 的处理相同）。`"socks"` 和 `forward_proxy` 不再是“暂不支持”。
- `akit-l4`：`forward`，即 fit 的 `l4.Rules` 与 `internal/netguard`：拨号前按所请求的内容判定目的地
  （`Rules::dest`，fit 的 verdict），连接建立后按地址判定（`Rules::resolved`：`deny` 的网段、对 `allow` 未列出的
  名字只认 `allow` 的网段，以及对回环、链路本地、组播、未指定、元数据地址和本机自身地址的拒绝，除非 `allow` 中
  有位于其内的网段写明它）；`Rules::socks` 构建 akio-socks 的服务器（不带认证的 CONNECT；BIND 与 UDP ASSOCIATE
  以应答 7 拒绝），`Rules::proxy_config` 构建其隧道的配置（握手在 10 秒加 `connect_timeout` 内完成、
  `connect_timeout`、`idle_timeout`）。
- `akit-listener`：只提供 `socks` 的监听器直接提供 SOCKS 服务器；与 HTTP 或 `tcp` 并列时，由 akio-mux 把
  SOCKS5 或 SOCKS4 问候路由给它（I53 留给 I55 的那条路由），PROXY 头之后同样如此。其隧道在图的槽位所得到的引擎上
  拨号（新增 `Graph::engine`）。重载改变监听器的 `socks` 时会重新绑定它，与 fit 的 `binding.same` 比较规则一致。
- `akit-handler`：`forward_proxy`，即 fit 的 `handler/forward`：先检查客户端（`Context::peer`，有 PROXY 头时为
  头中的客户端）和所写的目的地（403），再交给使用同一规则的 akio-socks `connect_proxy_with`（连接建立后被拒绝的
  名字或无法到达的目的地为 502，超过 `connect_timeout` 为 504）；其他方法和扩展 CONNECT 为 405，目标不是
  `host:port` 为 400。仅支持 HTTP/1.1：h2 或 h3 流上的 CONNECT 应答 501，作为 akio API 需求 A134 之前的权宜之计。
- akio：A132 和 A133 在 akio 分支 `claude/a132-a133-socks-rules`（`9b096d0d`，基于 `e9891b52`；ADR-119）上
  实现并已锁定：`ProxyConfig::connected`（隧道的上游连接打开后，按所请求的名字和两端地址判定）、
  `ProxyConfig::connect_timeout`（应答 SOCKS5 4、HTTP 504），以及 SOCKS 访问钩子从受信任 PROXY 头取得客户端。
  已提出 A134（经 HTTP/2 和 HTTP/3 的 CONNECT）。
- 测试：`crates/akit-config/tests/forward_proxy.rs` 移植 fit 的 "listeners of many protocols, and socks" 用例，
  以及字段默认值和处理器参数；`crates/akit-l4/src/forward.rs` 移植 fit 的 `TestRulesDest`、`TestRulesInternal`、
  `TestInternal` 和 `TestRefused`；`crates/akit-listener/tests/socks.rs`：按地址和按名字的 CONNECT、SOCKS4 与 4a、
  BIND 与 UDP ASSOCIATE 被拒绝、每种拒绝（目的地收不到任何数据）、慢上游与慢客户端下双向各 8 MiB、一端中途断开、
  空闲与连接超时、PROXY 头中的客户端、改变规则的重载；`one_port.rs`：fit 的 `TestSniffClear` 与
  `TestOnePortManyProtocols` 中的 SOCKS（与 HTTPS、h2、TLS 透传和 `tcp` 并列；只有 HTTPS 与 SOCKS）；
  `crates/akit-handler/tests/forward_proxy.rs`：fit 的 `TestConnect` 与 `TestConnectIdleTimeout`、405、连接超时、
  h2c 应答 501、慢对端与中途断开。
- 兼容性门禁：新增 15 个字段；`make compat-bless`：`examples/socks` 和 `examples/forward-proxy` 有效（80 个夹具：
  35 个有效，44 个只因暂不支持被拒绝，1 个跳过，没有失败）。
- 已检查：macOS 上的 `make check`；gpu51 上在基准锁下，在 epoll 和 `AKIO_TEST_BACKEND=uring` 上运行所涉及 crate
  （akit-config、akit-l4、akit-listener、akit-handler、akit-runtime、akit、akit-compat）以及 akio-socks 的测试。
- 待测量，由维护者的基准任务进行（规则 30）：见开发计划中 I55 的基准行。

### 2026-11-24：I75，FastCGI

- `fastcgi` handler（`crates/akit-handler/src/fastcgi/`），对应 fit 的 `handler/fastcgi`：
  由 FastCGI 应用（如 PHP-FPM）应答请求。请求的 CGI/1.1 参数（fit 的 `params.go`：请求本身、
  `root` 下的 `SCRIPT_NAME`、`SCRIPT_FILENAME`、`PATH_INFO`、`PATH_TRANSLATED`，`REMOTE_ADDR`、
  `HTTPS`、`REDIRECT_STATUS`，每个字段作为 `HTTP_*`，但 `Proxy`（httpoxy）和名字带 `_` 的字段除外，
  字段取 `headers` 中间件修改后的值，最后是 `env`）和消息体以记录发给应用，响应的 CGI 头给出状态
  （`Status`，有 `Location` 时为 302）和字段，逐跳字段被丢弃。路径按 `split_path`（不区分大小写）
  拆成脚本和路径信息；以 `/` 结尾的路径运行 `index`；`try_files` 在 akio 的逻辑线程池上查看磁盘
  （`Context::executor`），从不在 I/O 线程上。点文件：路径中有以点开头的段的脚本回答 404
  （`/.well-known/` 除外），除非 `dotfiles` 为 `allow`，`try_files` 跳过它；含 `..` 段、NUL 或
  反斜杠（无论是否转义）回答 400。
- 双向流式传输并联动背压（`exchange.rs`），与代理的 relay 相同：超过 64 KiB 或未声明长度的消息体
  到达即以 `STDIN` 记录上送，应用连接中等待的数据超过 64 KiB 时暂停读客户端，排空后恢复（连接
  就绪前到达的数据先保留，同时暂停读）；超过监听器的 `max_body_bytes` 回答 413。结束与响应头在
  同一次读取中到达的响应整体发送并带长度；其余到达即流式发送，等待客户端的数据超过 64 KiB 时
  暂停读应用连接，客户端排空后恢复。开始后中断的响应：HTTP/1.1 关闭客户端连接，HTTP/2 和 HTTP/3
  重置流，未声明长度的则以 fit 的 `Fit-Error` trailer 结束。
- 连接（`pool.rs`），fit 的连接池：经 akio 的引擎拨号（主机名由 akio 的解析器在 I/O 线程之外解析，
  `unix/` 加路径为 Unix 域套接字），每个连接同时只承载一个请求，最多 `max_conns`（64）个，最多
  保留 `max_idle_conns`（16）个空闲连接并在 `idle_timeout` 后关闭，最多 `max_pending`（1,024）个
  请求等待连接，超过回答 503。无消息体、方法可安全重复的请求若取到应用未应答就关闭的空闲连接
  （PHP-FPM 的 `pm.max_requests`），在新连接上重发一次。空闲连接上的记录、结束之后的记录、或未按
  FastCGI 规定结束的请求都会关闭连接。超时：`connect_timeout`、`response_header_timeout`、
  `response_idle_timeout`（不计客户端压住响应的时间）和 `timeout`，与空闲连接和拨号的超时一起，由每个
  handler 每 100 ms 在 akit 的定时器线程上的一次巡检检查（A45 的临时方案），而不是在定时器线程所有使用者
  共用的表中为每个请求放一个任务；都回答 504。客户端任何时候离开都会结束交换并关闭其连接。应用写到
  标准错误的内容在 akio 的逻辑线程池上记录，每秒最多 100 行。
- akio：拨号 Unix 域套接字是 akio API 需求 **A135**（`Engine::dial_unix`，ADR-120），由 I58 在
  akio 分支 `claude/a135-unix-dial`（`0f2c419b`，基于 `e9891b52` 的两个提交）上实现；akit 锁定该提交（所有
  akio crate、`AKIO_REV`、版本测试）。handler 不知道请求到达的地址，因此 `SERVER_ADDR` 为空，
  `SERVER_PORT` 取自 `Host`（或 80、443），与 fit 在连接不是 TCP 时相同：akio API 需求 **A148**。
- 配置：`address`（主机和端口，或 `unix/` 加路径）、`root`（绝对路径）、`split_path`、`index`、
  `try_files`、`dotfiles`、`env`、`max_conns`、`max_idle_conns`、`max_pending`、
  `connect_timeout`、`response_header_timeout`、`response_idle_timeout`、`timeout`、
  `idle_timeout` 和 `max_header_bytes`，带 fit 的默认值、检查和消息。兼容性门禁：新增 18 个字段；
  `make compat-bless`：`examples/fastcgi` 有效（80 个夹具：34 个有效，45 个只因暂不支持被拒绝，
  1 个跳过，没有失败）。
- 测试：`crates/akit-handler/tests/fastcgi.rs` 运行自己的脚本化 FastCGI 应用（如 fit 的
  `fakeApp`），移植 `TestParams`、`TestKeepAlive`、`TestLargeBodies`（上传 10.5 MiB、chunked、
  向慢客户端下载 16 MiB）、`TestTimeoutsAndLimits`、`TestCutConnections`、`TestTryFilesAndUnix`
  （也经 Unix 域套接字）、`TestDotfiles`、`TestResponses`、`TestIdleConnections`、`TestQueued`、
  `TestClientsGo`、`TestBodies`、`TestOverallTimeout`、`TestNoApplication`、`TestStaleIdle`、
  `TestProvisionErrors` 和 `TestDotfilesProvision`，经 HTTP/1.1 和 h2c；单元测试移植 `TestSplit`、
  `TestRecords` 和 `TestParamsTLS`。fit 的 `TestPHPFPM`（其端到端测试，PHP-FPM 运行脚本：参数、
  表单、3 MB 上传、状态和 cookie、重定向、4 MiB 下载、前端控制器）是 `php_fpm`，经 Unix 域套接字
  对 `AKIT_PHP_FPM` 所指的 PHP-FPM 二进制运行，没有时跳过。
- 未完成：本机和 gpu51 都没有安装 PHP-FPM，`php_fpm` 尚未运行：计划中的“对 php-fpm 的测试通过”
  有待于此。在不区分大小写的文件系统上，static handler 还不会拒绝 `split_path` 所列扩展名的文件
  （fit 的 `dotfile.AddScripts`）；fit 的 Windows 规则（8.3 短名、别名）不适用于 akit。fit 的
  `TestDrainAndCleanup` 未原样移植：空闲连接在配置释放时（`on_cleanup`，其最后一个请求之后）关闭，
  没有切断繁忙连接的 drain。fit 的 `cover_test.go`（其内部实现）未移植。
- 检查：macOS 上 `make check`；gpu51 上在 bench 锁下，`akit-handler`、`akit-config`、
  `akit-compat` 和 `akit` 的测试在 epoll 和 `AKIO_TEST_BACKEND=uring` 上运行。
- 无基准测试（规则 30）：应测量的内容见开发计划中 FastCGI 的基准测试行。

### 2026-11-24：I58，Docker provider

- `akit-config` 读取 fit 的 `providers`（`crates/akit-config/src/providers.rs`）：`name` 和
  `docker`（`endpoint`、`expose_all`、`network`、`use_published_ports`、`label_prefix`、
  `max_containers`、`refresh_interval`、`request_timeout`），带 fit 的默认值和 `checkProviders`
  的消息（恰好一种类型、名字的格式、不是基础配置的来源、跨文件唯一、`unix://<path>` 或
  `tcp://<host>:<port>`、前缀的格式）。`consul`、`consul_kv`、`etcd` 和 `kubernetes` 仍为暂不支持
  （I59–I61：各加一个 `ProviderKind` 变体）。在 akit 启动时读取：改变它们的重载会警告需要重启。
- `akit-provider`：`labels` 即 fit 的 `provider/labels`（从前缀下的标签读出路由、服务和上游的
  设置，并生成它们构成的部分，相同的标签生成相同的部分），I61 中也用于 Consul 的标签。`docker`
  即 fit 的 `provider/docker`：把容器的标签变成上游（副本按 Compose 项目和服务或 `name` 归组）、
  路由和四层服务，被略过的容器各报告一次；`GET /events`（容器事件，跳过 `exec_` 事件）和
  `GET /containers/json` 走 akio 的 HTTP 客户端，借助 A135 经 Unix 套接字，或经 TCP。事件流的
  读取和列表的收集由 akio I/O 线程上的回调完成，它们只解析收到的内容（一个事件最多 1 MiB，
  一次列表最多 16 MiB，错误应答的消息体最多 4 KiB）并通知 provider 自己的线程；该线程按 fit 的
  时序工作（事件后防抖 200 ms，不论有没有事件都每隔 `refresh_interval` 列一次，守护进程失败后
  从 1 s 到 30 s 退避，重连后再次发送部分）并发送部分。`request_timeout` 限制一次列表和事件流的
  响应头；事件流本身可以长时间无数据（定期刷新会发现守护进程已不在）。provider 跑在自己的引擎上
  （一个客户端线程），与 OTLP 导出器相同。
- `akit run` 按顺序创建配置列出的 provider，向 `Configs` 登记其名字，并在 I57 的 `Updates` 上启动
  它们（fit 的 `startProviders`，以及它的“built without the ... provider”消息），关闭时停止它们。
- akio A135（ADR-120），在 akio 分支 `claude/a135-unix-dial`（`0f2c419b`，基于 `e9891b52`）上
  实现并已锁定：`Engine::dial_unix`（epoll 和 io_uring）、`UNIX_ADDR`、
  `akio_sys::connect_unix_nonblocking`、`Client::with_unix_socket`。
- 测试：`crates/akit-provider` 移植了 `TestTranslate`、`TestTranslateOptions`、
  `TestTranslateEdges`、`TestProvider`、`TestProviderRefresh`、`TestProviderNoDaemon`、
  `TestProviderListFails`、`TestClientFails`（经 TCP）、`TestReadEvents`、`TestKind`，以及标签的
  `TestParse` 和 `TestParseRefused`，对接 Unix 套接字上的假守护进程，客户端跑在被测后端上；超过
  上限的事件和不是 JSON 的事件会结束事件流。`crates/akit-config/tests/providers.rs` 移植了
  `config_test.go` 中关于 provider 的用例。`crates/akit/tests/docker.rs` 用 fit 的 Docker Compose
  示例的配置对接假守护进程：服务随容器的启动和停止被路由，兜底路由保持不变。
- 兼容性门禁：多 11 个字段；`examples/docker-compose` 和 `examples/provider-docker` 有效
  （80 个夹具：35 个有效，44 个只因暂不支持被拒绝，1 个跳过）。
- 无基准测试（规则 24：控制面）。

### 2026-11-24：I69，缓存 III：清除与标签

- 标签，同 fit 的 `tagsOf`：保存的响应以其 `Cache-Tag` 字段（逗号分隔）和 `Surrogate-Key` 字段
  （空格分隔）的成员为标签，每个只记一次，至多 32 个，每个不超过 128 字节，计入 zone 的大小。内存层
  16 个分片各自在分片锁下维护按标签的键索引，保存带标签的响应不需要 zone 级的锁（rules.md 规则 13）；
  磁盘层的索引维护自己的一份，读回时由文件中的字段重建（文件格式不变）。
- `akit_cache::purge(zone, &Selection)`，同 fit 的 `PurgeBy`：从内存和磁盘中删除一个 zone（`""` 为
  所有 zone）保存的内容，按 URL（`http` 或 `https` 绝对 URL，主机大小写不限，删除所有变体）、按 URL
  前缀（`https://example.com/img/`；带用户信息或片段的前缀被拒绝）、按标签，或全部删除；返回删除的
  键数，与 fit 的计数相同。查找键时只在查找期间持有各分片的锁和磁盘索引的锁（前缀需要遍历键）；文件
  像淘汰时一样在磁盘线程池上删除。清除会丢弃被删内容仍在排队的写入，同失效处理；全部清除会丢弃此前
  排队的所有写入和此后读回的所有文件，被清除的内容不会在磁盘上重新出现。`akit_cache::zones()` 即
  fit 的 `Zones`。
- Admin API：`GET /cache`（每个 zone 的 `zone`、`keys`、`bytes`，有磁盘层时还有 `disk_entries`、
  `disk_bytes`、`disk_path`）和 `POST /cache/purge`（`{"zone", "urls", "prefixes", "tags",
  "all"}`，JSON，不超过 `max_config_bytes`，像配置一样流式读入；应答 `{"purged": n}`；没有缓存使用的
  zone 返回 404，什么也没指定或 URL 无效返回 400，不以 JSON 发送返回 415），同 fit 的
  `admin/cache.go`。两者都在 API 的 I/O 线程上处理；`akit-admin` 为此依赖 `akit-cache`，同 fit 的
  admin 包导入其 cache 包。
- 测试：`crates/akit-cache/tests/cache.rs` 移植 fit 的 `TestPurge`（URL、标签、前缀、全部，内存与
  磁盘，被拒绝的输入和 `Zones`），另加只在磁盘上的键被清除，以及磁盘线程池被挂起时清除丢弃排队的写入；
  单元测试移植 `TestTagsOf`，并覆盖标签索引在变体替换、淘汰、删除和全部清除时的变化，以及 URL 的键；
  `crates/akit-admin/tests/cache.rs` 针对实际服务的监听器移植 `TestCachePurge`，另加 415 和 405 应答。
  没有新的配置字段：`supported.txt` 为 I69 加了注释，门禁报告和字段清单不变。
- 没有 akio 需求（A144–A146 未使用）。已检查：macOS 上的 `make check`；gpu51 上在 bench 锁下，
  `akit-cache`、`akit-admin` 和 `akit` 在 epoll 和 `AKIO_TEST_BACKEND=uring` 上的测试。
- 没有基准测试（规则 30）。待测量：带标签响应（0 与 32 个标签）相对无标签响应的保存开销，以及在服务
  期间对大 zone 按前缀和按标签清除（分片锁和磁盘索引锁的持有时间）；已记入开发计划的响应缓存一行。

### 2026-11-24：I64，OpenTelemetry

- `akit-config` 读取 fit 的 `telemetry`（`endpoint`、`headers_file`、`ca_file`、`cert_file`、
  `key_file`、`service_name`、`timeout`、`traces.sample_ratio`、`traces.max_queue_bytes`、
  `traces.interval`、`metrics.interval`），默认值和检查、报错信息与位置都与 fit 相同
  （`checkEndpoint`、`checkTLSFiles`、"set traces, metrics or both"、0 到 1 的比例），只能由一个
  文件给出；`parse_headers` 即 fit 的 `ParseHeaders`。
- `akit-observe`：`trace`，fit 的 tracer：按 fit 的方式读取 `traceparent`（更高版本读到 00 的
  部分为止；无效的不跟随），跟随已采样的调用方，未采样的调用方从不追踪，其余按 `sample_ratio`
  （线程自己的 wyrand）；span 按 fit 的 OTLP JSON 编码（`GET route`、server 类型、fit 的属性、
  5xx 状态为 error、`url.path` 百分号解码）进线程自己的缓冲区，再复制进 16 个分片之一，
  `max_queue_bytes` 限制总量（超出则丢弃并计数），由自己的线程每个 `interval` 或攒到 256 KiB
  时导出；`close` 导出剩余的。`otlp`：`Otlp`，即接收端（头、来自文件的 TLS、带
  `service.name` 和 `service.version` 的 resource），通过 akio 的 HTTP 客户端在一个只有一个
  客户端线程的独立引擎上 POST；失败在持续期间只记一次日志，恢复时也记一次；`MetricWriter`
  （Prometheus 文本或 `OtlpWriter`，即 fit 的 OTLP 指标）和 `MetricsExporter`，每个
  `metrics.interval` 一次，关闭时再导出一次。没有 akio 需求：不使用 akio-export 和 akio-http 的
  请求 span（[决策 007](decisions/007-opentelemetry.zh-CN.md)）。
- `akit-listener`：分发器对每个请求询问图的 tracer（一个 `traceparent` 字段或没有）；被追踪的
  请求得到 `akit.traceparent`（`akit_runtime::TRACEPARENT`），它的 span 由访问日志和指标共用的
  响应钩子结束，带上代理的上游地址。`Scrape::write_metrics` 经 `MetricWriter` 写出，带
  `fit_trace_spans_total`。`akit-proxy` 用 akit 的 `traceparent` 替换客户端的发给上游，放在最后，
  与 fit 相同；`tracestate` 以及未追踪请求的 `traceparent` 原样传递。`akit-runtime`：
  `Env::with_tracer`、`Graph::tracer`；`telemetry.metrics` 与 `metrics` 一样统计请求。
- `akit run`：导出器按 akit 启动时的 `telemetry` 启动（读不了的文件使其退出）；重载改变
  `telemetry` 时给出警告（"takes a restart"）；指标导出器和 tracer 在监听器之后依次关闭，所以
  每个已结束的 span 都会导出。
- 测试：`crates/akit-observe/tests/telemetry.rs` 移植 fit 的 `TestSample`、`TestTracerExport`、
  `TestTracerBounds`、`TestTracerTicks`、`TestTracerFlushesWhenFull`、`TestSpanProtocols`、
  `TestExporterErrors`、`TestExportFailures`、`TestMetricsExporter` 和
  `TestMetricsExporterTicks`（用自己的接收端），另有八个线程同时结束 span 的测试；单元测试
  `TestParseTraceparent`、`TestSampledFlagShort` 和 `TestOTLPWriterIgnores`；
  `crates/akit-config/tests/telemetry.rs` 移植 fit 的 "telemetry settings" 用例和
  `TestParseHeaders`；`crates/akit/tests/telemetry.rs` 端到端：`akit run` 追踪调用方采样的请求，
  上游在调用方的 trace 中收到 akit 的 span，未采样的 `traceparent` 原样传递，span（代理的、直接
  应答的、404）和指标在 akit 停止时到达 collector。
- 兼容性门禁：多 14 个字段；`make compat-bless`：`examples/telemetry` 有效（80 个夹具：31 个
  有效，48 个只因暂不支持被拒绝，1 个跳过，没有失败）。
- 已检查：macOS 上 `make check`；gpu51 上持 bench 锁，工作区测试在 epoll 和
  `AKIO_TEST_BACKEND=uring` 上全部通过（兼容性门禁的样例在那次运行开始后才加入，第二次运行在两种
  后端上都通过）。
- 待测（交给维护者的基准任务，规则 30）：见开发计划中 I64 的基准行。

### 2026-11-24：I63，仪表盘

- `akit-admin` 在 `/dashboard/` 提供 fit 的仪表盘：fit 的三个文件（`index.html`、`app.js`、`app.css`，
  取自 fit `7daee573`，名字改为 akit），编进二进制；纯 HTML、CSS 和 JavaScript，不从别处取任何东西，只发 GET。
  标签页：概览（配置版本、重载、连接、路由、在线后端、证书、监听器、来源）、路由、四层服务、上游及其后端健康状态、
  证书、版本（每个版本可查看）。响应带 fit 的头（`DASHBOARD_CSP`、`nosniff`、`X-Frame-Options: DENY`、
  `no-referrer`、`no-cache`）和内容类型；`/dashboard` 与 `/dashboard/index.html` 重定向到 `/dashboard/`（301），
  同 Go 的文件服务；其他方法 405。
- 守卫同 fit：仪表盘文件不含数据，不带令牌也提供（页面会要令牌，存于标签页的 `sessionStorage` 并随请求发送）；
  带 `Origin` 的请求只有来自 API 自身源的 GET 或 HEAD 才放行，因此页面只读，什么也写不了。
  `admin.disable_dashboard`（现已支持）不提供这些页面（404）。
- `GET /status`，fit 的格式：运行中配置的版本、应用时间和来源，取自历史；然后是 `akit_admin::Options::status`
  （`Live`），`akit run` 将其设为 akit-listener 的 `Scrape::write_status`：重载（成功、失败），正在服务的监听器
  （名字、绑定地址、协议，取自 `Listeners::view`，每次重载更新的 `ServedView`），连接（打开、拒绝、待发字节，取自
  akio 的遥测），按名字排列的上游及其进行中请求数和每个后端的地址、是否在线、`probed_down`、进行中、成功和失败数，
  按监听器再按到期时间排列的证书（名字：DNS 名，然后 IP 地址，否则通用名；签发者、序列号、`not_before`、
  `not_after`），以及有 `metrics` 时自启动以来按类别统计的已答请求（I37 的 `Metrics::totals`）。
  `akit_tls::CertificateInfo` 现在带名字、签发者和 `not_before`。
- `GET /config/effective`，fit 对无 provider 配置的格式（provider 由 I57 带来）：版本、ETag、`precedence`
  （只有配置自身的来源）、运行中的配置，以及每个顶层成员和 `listeners`、`routes`、`upstreams`、`services`
  每个元素的来源，同 fit 合并时列出基础配置的方式。它的其他方法仍编辑成员 `effective`。
- 测试：`akit-admin/tests/dashboard.rs` 移植 fit 的 `TestDashboard`、`TestDashboardToken`、
  `TestDashboardDisabled` 和 `TestDashboardReadOnly`（文件检查手写，不用正则库），并测试单独的 `/status`、带写入器的
  `/status` 和 `/config/effective`；`akit/tests/metrics.rs` 在运行中的服务上移植 `TestStatus`（两个监听器，一个带
  TLS；一个上游带一个存活和一个失效后端，失效的因失败被移出；证书；按类别的请求），以及监听器视图随重载更新；
  `akit/tests/admin.rs` 通过 `akit run` 检查页面、`/status` 和 `/config/effective`；`akit-config` 读取
  `disable_dashboard`；`akit-tls` 按 Go 的方式写 IP 名。已在浏览器中对 `akit run` 检查：每个标签页都显示其数据，
  控制台无错误。
- 兼容性门禁：`admin.disable_dashboard` 已支持（249 个字段）。
- 已检查：macOS 上 `make check`；gpu51 上在 bench 锁下，所涉 crate（akit-tls、akit-config、akit-listener、
  akit-admin、akit）的测试，epoll 与 io_uring，全部通过，只有 I22 的
  `reloads_under_traffic_with_no_failed_request`（`akit/tests/reload.rs`）在 io_uring 上失败一次（main 上也如此），
  单独重跑五次全部通过。
- 与 fit 的差异：`/status` 没有 `node` 能力：akit 没有 `GET /node`（fit 的 `TestStatus` 检查 `/node` 列出
  `status.requests`）。时间带毫秒。`/config/effective` 在 I57 之前只有一个来源。仪表盘属于控制面：不需要基准测试。

### 2026-11-24：I57，provider 框架与生效配置

- `akit-provider` 实现 fit 的合并（`provider.Parse`、`Merge`、`Merged.Errors`；fit 决策 009）：
  provider 提供的部分是只含 `listeners`、`routes`、`upstreams` 和 `services` 对象数组的 JSON
  对象，至多 16 MiB（`MAX_PART_BYTES`），为空即不提供任何内容；注释和尾随逗号同文件中一样被抹去。
  `merge` 把各部分的元素按原文接在基础配置每个数组的元素之后，按 provider 的优先顺序逐个加入；
  名字已被基础配置或更靠前的 provider 占用的具名元素被略去，记为 shadowed，并注明占用者。基础配置
  的文本、下标和位置保持不变，没有新增内容时直接共用。`Merged::errors` 把合并后配置的错误映射回
  原处：基础配置中的错误给出它在那里的行列，provider 元素中的错误给出它在该部分中的路径和位置，并
  附 `(provided by <name>)`。
- fit 在 goroutine 上运行 provider，在另一个 goroutine 上应用它们的部分；akit 两者都没有：provider
  （`Provider` trait：`start`、`stop`）在 akio 的客户端、逻辑池或自己的线程上监视，通过从不阻塞的
  `Update` 发送部分。`Updates` 每个 provider 至多保留一个部分，新的替换仍在等待的旧部分，每一批部分
  只唤醒控制线程一次。`Kinds` 是 fit 的 kind 注册表，像 akit 的模块注册表一样由各 provider crate 的
  列表构建。
- `akit-admin` 的 `Configs` 在它应用的每个配置（API 的、文件的、回滚的或重载的）中合并 provider 的
  部分，让它们走同一条路径；`add_provider`（fit 的命名规则：不能是基础配置的来源，不能重复）和
  `provide`（把部分合并进正在运行的基础配置，作为来源为该 provider 的新版本应用，基础配置及其 ETag
  不变；同样的部分不会重复应用；被拒绝的部分保留该 provider 上一次的内容；空部分撤走它的元素）。
  每个版本记下基础配置的来源（`file`、`admin`，回滚后是被回滚到的版本的来源；重载和证书文件的重载
  沿用它，同 fit 的 `Reload`）、生效配置、各部分来源和被 shadowed 的元素。`GET /config`、它的指针、
  ETag 和回滚仍然只针对基础配置。由于 provider 的名字也是来源，`Version::source` 和
  `Outcome::source` 改为 `Arc<str>`；重载和回滚改走 `Configs::reapply` 与 `Configs::rollback`。
- `GET /config/effective` 按 fit 的 `Effective` 应答：版本、基础配置的 ETag、优先顺序（基础配置的
  来源在前）、生效配置（同 Go 一样紧凑输出）、每个成员和数组元素的来源（合并后配置中的指针、来源、
  provider 部分中的指针），以及被 shadowed 的元素；在 API 的 I/O 线程上处理，只取两把短锁。此前该
  路径按指向配置的指针读取。
- `akit run` 在控制线程上接收 provider 的部分（`Control::Provide`，每轮每个 provider 至多一个，然后
  处理其他工作），停止时丢弃仍在等待的部分。目前还没有运行任何 provider：`providers` 字段及各 kind
  随 I58–I61 加入，在那之前仍是暂不支持（门禁没有新增字段；`make compat-bless` 和字段清单没有变化）。
- 测试：`crates/akit-provider` 移植 `TestMerge`、`TestMergeIntoEmpty`、`TestParseRefused`、
  `TestErrors`（fit 的行列）、`TestPosition`、`TestParseRefusedMore`、`TestErrorsOne`、
  `TestOffsetPastEnd`、`TestRun`、`TestRunCanceledWhileApplying` 和 `TestKinds`，并覆盖恰好在、低于和
  超过上限的部分，以及在各自线程上全速发送的 provider；`crates/akit-admin/tests/providers.rs` 针对
  实际服务的监听器移植 `TestProviders`（另加重载和 dry run）与 `TestFileAndProviders`。JSON 语法错误
  的措辞来自 akit-config，而不是 `encoding/json`。未移植：fit 的重载计数（akit 在 `akit run` 中计数）。
- 没有 akio 需求（A123–A125 未使用）。已检查：macOS 上的 `make check`；gpu51 上在 bench 锁下，
  `akit-provider`、`akit-admin` 和 `akit` 在 epoll 和 `AKIO_TEST_BACKEND=uring` 上的测试。
- 没有基准测试（规则 24：控制面）。

### 2026-11-24：I68，缓存 II：磁盘层、缓存锁、陈旧响应

- 缓存锁（`crates/akit-cache/src/lock.rs`），同 fit 的 `lock.go`：一个 GET 未命中，而同一个键已有
  别的请求在取时，它等那次获取，最多 `lock_timeout`（5 秒），并用其保存的结果应答；`disable_lock`
  让每个未命中都自己去取。等待的请求被挂起（akio 的 `Context::suspend`，ADR-109），不占线程；获取
  一结束（响应已保存或确知不保存：头部已定、消息体已保存、放弃或被截断），就在结束它的线程上恢复。
  超时是每次获取一个定时器，跑在 akit 的定时器线程上（A45 的权宜之计），为剩下的等待者重新设置。
  上限：每次获取 4,096 个等待者（超过则自己去取）；超过两倍 `lock_timeout` 的获取视为已放弃并被替换。
  消息体本会流式交给处理器的 GET 或 HEAD 改为整体读入（在监听器的 `max_body_bytes` 之内），以便能
  等待（akio 不能挂起消息体流式传输的请求，ADR-109）。
- 陈旧响应：带校验器的，或 `stale-if-error` 允许的，由缓存自己重新获取，其请求与其他请求一起等待：
  一个缓存自己的请求，即缓存所见的客户端请求，去掉范围与前置条件，用 akio 的
  `transport::dispatch_conn` 经过缓存后面的处理器，交给一个只看着响应经过（流式响应经缓存的消息体
  tap）然后丢弃的 sink。它的 304 使响应重新新鲜；它的失败（无响应、被截断、500、502、503 或 504，
  无论是否流式）在 `stale-if-error` 窗口内以陈旧响应应答（`detail=stale-if-error`）。在
  `stale-while-revalidate` 窗口内的响应立即应答（`detail=stale-while-revalidate`），并在后台以同样
  方式重新获取，每个键一次、每个 zone 最多 256 个。RFC 5861 的指令优先于 `stale_while_revalidate`
  和 `stale_if_error` 参数；`must-revalidate`、`proxy-revalidate` 和 `s-maxage` 排除两者。I67 中
  期望 `fwd=stale; stored` 的三个测试现在期望 fit 由缓存自己获取后给出的应答。
- 磁盘层（`crates/akit-cache/src/disk.rs`），同 fit 的 `disk.go`：内存中保存的每个响应也写到磁盘
  （每个响应一个文件，以其键和变体的 SHA-256 命名，先写到 `tmp` 再改名到位），内存不再持有的响应
  在其请求挂起期间读回并重新放入内存（`detail=disk`）；读取失败或超过 5 秒（如磁盘卡住）时，请求按
  未命中继续。内存中的索引不碰磁盘就能说出磁盘上有什么；zone 打开目录时从目录读回索引（不完整或
  不属于缓存的文件被删除，`tmp` 被清空），期间请求照常进行，只是在磁盘上找到的更少。写入和删除
  有序，所以在键被删除（失效）之前排队的写入不会把它带回来。上限：按最近最少使用淘汰的
  `max_bytes`（1 GiB）和 `max_entries`（100,000），等待写入的响应 64 MiB，每个文件的元数据 1 MiB。
  文件格式是 akit 自己的（头部、键、时间、字段和变体的请求字段，然后是消息体）；不读取 fit 的文件。
- 每个文件操作都在磁盘线程池上运行，这是 akit 自己的有界线程池（`akio_pool::Pool`，至少 16 个、
  每个 CPU 4 个线程，排队 4,096 个任务，排队超过超时的任务作废），与 akio 的逻辑线程池分开：从不在
  I/O 线程上（rules.md 规则 1）。测试中对线程池之外的文件操作计数，每个磁盘测试之后都是 0。锁定的
  提交上 `akio` 门面 crate 没有重新导出 `Pool`，所以 akit-cache 直接依赖 `akio-pool`：这是一个权宜
  之计，记为 akio API 需求 **A117**。
- zone：一个 zone 采用最后为它编译的缓存的磁盘；磁盘目录变了的 zone 采用新目录并读回；配置去掉磁盘
  的 zone 不再有磁盘；另一个 zone 的磁盘正在使用的目录被拒绝（`disk: the directory … is zone "a"'s`）。
  目录在编译配置时创建。
- 配置：读取 `stale_while_revalidate`、`stale_if_error`（0 即没有）和 `disk`（`path` 必填，
  `max_bytes` 或 `max_size`，`max_entries`）；`cache` 不再有被拒绝的字段。兼容性门禁：多 7 个字段；
  `make compat-bless`：`examples/cache` 有效（80 个夹具：31 个有效，48 个只因暂不支持被拒绝，1 个
  跳过，没有失败）。
- 测试：`crates/akit-cache/tests/cache.rs` 移植了 fit 的 `TestStampede`（100 个客户端，一次获取；
  以及经代理、流式）、`TestLockNotKept`、`TestLockTimeout`、`TestDisableLock`、
  `TestStaleWhileRevalidate`、`TestStaleIfError`（以及经代理，其 500 为流式）、`TestDiskRestart`、
  `TestDiskPromotion`、`TestDiskNeverBlocks`（把线程池卡住模拟磁盘挂起：超时后请求按未命中应答，
  期间 I/O 线程照常应答其他请求）、`TestDiskInvalidate`、`TestDiskZones`、条目下的文件消失，以及
  参数；单元测试移植了 `TestParseFile`、`TestDiskAdd`、`TestDiskBounds` 和 `TestDiskBrokenFiles`。
- 未完成：按键、标签和前缀清除（I69，连同 fit 的 `Cache-Tag`）；fit 的 `TestWaiterGone` 和
  `TestDiskReadClientGone` 本身（客户端已离开的等待者被恢复后应答无处可去；客户端已离开时跳过磁盘
  读取，未测试）；缓存自己的请求带上 akit 监听器记录的内容（TLS、规范路径、消息体上限），但不带路由
  参数或其他中间件的 local。
- 已检查：macOS 上的 `make check`；gpu51 上持基准锁，`akit-config`、`akit-cache`、`akit-compat` 和
  `akit` 的 clippy 与测试，epoll 和 `AKIO_TEST_BACKEND=uring` 两种后端，全部通过，只有
  `akit --test reload` 在 epoll 上失败一次（流量下的重载，不涉及缓存，I52 之前就见过在负载下失败），
  随后在两种后端上各三次全部通过。
- 无基准测试（规则 30）：要测量的内容见开发计划中响应缓存的基准行。

### 2026-11-24：I34，按需 TLS

- `akit-config` 读取 fit 的 `acme.on_demand`（`ask` 必填，须是 http 或 https URL；`rate` 10、`interval` 1m、
  `max_names` 1000、`timeout` 10s、`idle_timeout` 168h，写 0 取默认值）和 `tls.on_demand`，检查和提示同 fit：
  `on_demand` 需要 `acme.on_demand`，要求客户端证书的监听器需要回答 HTTP-01，只有 `on_demand` 的 `tls` 不算空（默认
  HTTPS 不给它名字；`"on_demand": false` 仍算空）。
- `akit-tls`：fit 的 `tlsmgr/ondemand.go` 放在新的 `acme::on_demand` 模块。`tls.on_demand` 打开的监听器，对客户端
  要的、文件和 `tls.acme` 都没有的名字，从证书解析器（akio 的 A2，ADR-098）调用 `Acme::on_demand`。已有证书的名字在
  调用中作答；其他名字把握手的 `CertDone` 留在该名字下等待并返回，所以从不占住 I/O 线程。ask URL（查询里加上
  `domain=<name>`）经 akio-http 的异步客户端询问，客户端在自己的引擎上；回调接收该名字，ACME 工作线程像获取列出的
  名字一样获取证书，处理完时回答所有等它的握手。同一名字的握手等同一张证书、同一次询问。
- fit 的各项上限都在握手的调用中检查，超出的握手立即失败：拒绝记住一分钟（最多 10,000 个），同时最多 8 个询问，平均
  每秒 5 个、一次最多 20 个（`MAX_ASKS`、`ASK_EVERY`、`ASK_BURST`），每 `interval` 最多 `rate` 个，总共 `max_names`
  个；每个名字最多 1,024 个等待的握手（`MAX_WAITERS`，另有 akio 的 `max_pending_cert_resolutions`）。监听器的
  `cert_resolver_timeout` 是 `acme.on_demand.timeout`，握手超时是它再加 10s。
- 第一张证书失败的名字被丢弃（等它的握手得知原因）；`idle_timeout` 内没有握手要过的名字由工作线程丢弃（每个名字每秒
  最多记录一次，不加锁），其证书留在存储中，给下一个被允许的握手。重载列出的名字归配置所有；不列任何名字的重载保留按需
  接收的名字。按需监听器为任何名字回答 TLS-ALPN-01。
- 测试：`akit-tls` 的单元测试移植 fit 的 `TestOnDemandRefused`、`RateLimited`、`MaxNames`、`Dropped`、`Names`、
  `Idle`、`IdleTimer` 和 `AskBounds`（ask URL 以及挂起或切断连接的 CA 用几行 std 写成；测试可以拨动的时钟）；
  `akit-listener/tests/on_demand.rs` 针对 Pebble 用真实握手移植 `TestOnDemandAllowed`（同时四个握手，一次询问，一张
  证书；询问被挂起时，二十个对文件中名字的握手和请求立即完成，被拒绝的名字失败），并用握手检查超时和限流；
  `akit-config` 的测试和门禁的样例覆盖这些字段。
- 兼容性门禁：`acme.on_demand`、它的六个字段和 `listeners[].tls.on_demand` 加入 `tests/compat/supported.txt`；
  `examples/acme-on-demand` 有效（80 个夹具：31 个有效，48 个只因暂不支持被拒绝，1 个跳过，没有失败）。
- 检查：macOS 上 `make check`（带 Pebble）；gpu51 上在 bench 锁下，epoll 和 `AKIO_TEST_BACKEND=uring` 下，akit-config、
  akit-tls、akit-runtime、akit-listener、akit-compat 和 akit 的 clippy 与测试（要求 Pebble）：全部通过。
- 没有 akio 请求（A120–A122 未使用）。
- 待测量：无；没有 `tls.on_demand` 的监听器照旧选择证书（规则 24：控制面）。

### 2026-11-24：I53，一个端口多种协议

- `akit-config` 接受监听器 `protocols` 中与 HTTP 并列的 `tcp`（fit 的 `config.Sniffs`），不再报告为暂不
  支持：明文（`http/1.1`、`h2c`）和带 `tls`（`h2`，以及同端口 UDP 上的 `h3`）的监听器都可以，也可带
  `proxy_protocol`；路由服务其 HTTP，服务接管其他连接。fit 在一个端口上区分的第三种协议 `socks` 仍为暂不
  支持：它属于 I55（正向代理，`listeners[].socks`），本迭代完成标准中的“以及 SOCKS”也随之归入 I55。
- `akit-listener` 用一个 akio TCP 监听器服务这样的监听器，其 acceptor 是 akio-mux 的 `Mux`（`sniff`，
  即 fit 的 `Server.sniff`），监听器带 `proxy_protocol` 时位于 PROXY protocol 层之后。带 `tls` 时：
  服务器名由该监听器的某个服务（按当前图）接管的完整 ClientHello（`Rule::TlsHello`）交给 akit-l4 的
  acceptor，透传给那个服务；其他 TLS 作为 HTTPS 服务（按 ALPN 选 h2 或 HTTP/1.1）；为读 hello 最多读
  64 KiB（`MAX_CLIENT_HELLO`）。明文时：TLS 交给 akit-l4 的 acceptor（按名字选服务，否则交给没有
  `server_names` 的服务），HTTP/1.1（带 `h2c` 时还有 HTTP/2 的 preface，`Rule::Http`）由 HTTP 服务。
  其余交给没有 `server_names` 的服务，没有则关闭。10 s 内（`OPENING_TIMEOUT`，akio-mux 自己的定时器）
  没发来足以区分的字节的客户端被关闭。单独的 `tcp` 监听器和单独的 HTTP 监听器照旧服务，不嗅探；三者现在
  在构建引擎和重载时共用一个 `tcp_listener`。端口为 0 的 `tcp` 加 `h3` 监听器和 HTTP 加 `h3` 一样取一个
  对两者都空闲的端口。没有 akio 需求：akio-mux 已经具备所需的一切。
- 测试：`crates/akit-listener/tests/one_port.rs` 移植 fit 的 `TestSniffClear`（一个端口上的 HTTP/1.1、
  h2c、按名字透传的 TLS 和原始 TCP；不含其中的 SOCKS）和 `TestTLSTerminatedWithoutNames`（服务旁按 ALPN
  服务的 HTTPS），并增加：HTTPS 旁按名字透传的 TLS（有、无兜底服务）、没有 `h2c` 时 HTTP/2 的 preface
  交给服务、首字节仍可能是几种协议时客户端被挂起、嗅探字节之前的 PROXY 头、同一端口上的 `tcp` 与 `h3`，
  以及为 HTTP 监听器加上 `tcp` 的重载。`crates/akit-config/tests/services.rs` 检查 fit 的 “mixed”
  监听器所代表的配置。未测：静默客户端在 10 s 嗅探超时后被关闭（akio-mux 自己的测试覆盖该超时；fit 未测）。
- 兼容性门禁：没有新增字段（`protocols` 已支持）；`make compat-bless` 没有改变任何夹具的结果。
- 已检查：macOS 上的 `make check`；gpu51 上持 bench 锁运行 `akit-config`、`akit-l4` 和 `akit-listener`
  （epoll 和 `AKIO_TEST_BACKEND=uring` 上的测试）。
- 待测（维护者的基准任务，规则 30）：共享端口上嗅探的开销，见开发计划中 I51–I53 的基准一行：`http/1.1`、
  `h2c` 加 `tcp` 的监听器与单独 HTTP 监听器的每秒连接数及 HTTP/1.1、h2c 请求速率，以及其上按名字透传 TLS
  与单独 `tcp` 监听器的对比。

### 2026-11-24：I52，SNI 分流与 PROXY protocol

- `akit-config` 读取 fit 的 `services[].server_names`、`services[].proxy_protocol`（`v1`、`v2`）和
  `listeners[].proxy_protocol.trusted`（地址与 CIDR 范围），检查与提示同 fit：服务器名须是主机名或 `*.` 下的名字，
  不能是 IP 地址，不能重复，同一监听器上不能已属另一个服务；带 `udp` 监听器的服务不能有这两个字段；`trusted`
  必填且不能为空；只收 UDP 的监听器不能有 `proxy_protocol`。拒绝 `**.` 通配（fit 的永远不会匹配）。`access`
  仍为暂不支持。
- `akit-l4`：图中每个 `tcp` 或 `udp` 监听器一个 `Entry`（`Graph::entry`，取代 `Graph::service`）：按精确名与
  `*.` 通配（一层标签）找服务，不区分大小写、忽略结尾的点，没有 `server_names` 的服务作为兜底。按名字分流时，
  TCP 连接在交给服务之前先读 ClientHello，用 akio-mux 的解析器（A5 的 `tls::parse_client_hello`；hello 可以跨多次
  读取与多个记录），最多 64 KiB（`MAX_CLIENT_HELLO`），限时 10 s（`OPENING_TIMEOUT`，在 akit 的定时线程上：A45 之前
  的权宜之计）；读到的字节原样先发给上游。没有服务接的名字、不是 TLS 的客户端、畸形或过长的 hello 交给兜底服务，
  没有兜底则关闭。
- PROXY protocol，基于 akio-mux 的 `ProxyProtocol`（A4）：`akit-listener` 把它放在带 `proxy_protocol` 的 `tcp`
  监听器或 TCP 上的 HTTP 监听器（在 TLS 之前）前面，信任其 `trusted` 地址：来自这些地址的连接必须在 10 s 内、
  4,096 字节（`MAX_PROXY_HEADER`）以内以 v1 或 v2 头开头，否则关闭；其他来源的连接照常服务。此后中转的客户端
  就是头里的客户端（`ip_hash` 按它选择），带 `proxy_protocol` 的服务在每个上游连接上最先发送一个头，写明该客户端
  和它所连的地址（头里的目的地址，否则是监听器的地址），IPv4 映射地址还原为 IPv4。`proxy_protocol` 变化的监听器
  在重载时重新绑定（`same`）。
- HTTP 监听器：请求的客户端就是头里的客户端（`X-Forwarded-For`、访问日志、`remote_ip`、限流、`ip_hash`），这需要
  akio：**A105**（ADR-118），在 akio 分支 `claude/a105-proxy-peer`（`e3f03a32`，基于 akio main `c5cf4353`）上实现：
  akio-http 的 `Request::peer` 取 `akio_mux::proxy::client_addr`。akit 锁定该提交；分支合并到 akio 的 main 后改锁
  main。
- 测试：`crates/akit-listener/tests/sni_proxy.rs` 移植 fit 的 `TestSNIRouting`（TLS 后端检查 ClientHello 原样到达，
  并用自己的证书完成握手）、`TestSNIHalfHello`（以及逐字节发送的 hello）、`TestProxyProtocolToUpstream`、
  `TestProxyProtocolFromClient`、`TestProxyHeaderThenHello` 和 `TestProxyProtocolHTTPEdges` 中关停的部分，另有给服务
  改名的重载，以及被截断和伪造的头（错误的地址族、端口、超过上限的长度、v2 版本 3、错误的命令、客户端中途断开的
  头），都被关闭且不拨上游；`crates/akit-proxy/tests/proxy_protocol.rs` 移植 `TestProxyProtocolHTTP` 和
  `TestProxyProtocolHTTPEdges` 的其余部分（头分多次写入、`UNKNOWN`、错误或缺失的头）；
  `crates/akit-l4/tests/entry.rs` 移植 `TestEntryPick`；`crates/akit-config/tests/services.rs` 覆盖字段与检查。
  fit 的解析器测试（`TestReadSNI`、`TestParseProxy`、`TestProxyRoundTrip`）由 akio-mux 自己覆盖（ADR-095）。
  `TestTLSTerminatedWithoutNames` 属于 I53。未测：永远不完整的 hello 在 10 s 开场超时后被关闭（fit 也未测）。
- 兼容性门禁：增加 4 个字段；`make compat-bless`：`examples/l4-tcp` 和 `examples/proxy-protocol` 有效（80 个夹具：
  27 个有效，52 个只因暂不支持被拒绝，1 个跳过，没有失败）。
- 检查：macOS 上 `make check`；gpu51 上持基准锁，akio 分支的 akio-http 与 akio-mux（clippy，epoll 与
  `AKIO_TEST_BACKEND=uring` 下的测试），以及 akit 整个 workspace（clippy，两种后端下的测试），`sni_proxy` 每种后端
  各跑五次：全部通过，只有三个测试在整套测试的负载下失败过一次、单独运行五次全部通过（epoll 下的 `akit --test
  reload`；io_uring 下的 `akit-admin --test admin` 与 `akit-proxy --test h2 host_header`，引擎未能启动，I52 之前
  已出现过）。akio 的公开 API 检查未能运行：gpu51 上没有 `cargo-public-api`（akio-http 的公开项未变）。
- 待测（维护者的基准任务，规则 30）：见开发计划中 I51–I53 的基准一行。

### 2026-11-24：I67，缓存 I：内存层与 RFC 9111

- `akit-cache` 实现 fit 的 `cache` 中间件（`http.middleware.cache`，已在 `akit` 中注册）：按 fit 的
  `semantics.go` 所述的 RFC 9111 规则保存共享缓存可以保存的 GET 响应（请求和响应的 `Cache-Control`、
  `Expires`、由 `Last-Modified` 或 `default_ttl` 得到的启发式寿命、可存储的状态码、`Vary` 每个键最多 8 个
  变体，带 `Set-Cookie`、trailer 和 `Vary: *` 的不保存，带 `Authorization` 的只在 `public`、`s-maxage` 或
  `must-revalidate` 时保存），回答时带 `Age` 和 fit 的 `Cache-Status`；HEAD 也由它们回答；`If-None-Match`
  和 `If-Modified-Since` 由它们回答 304；支持请求的 `no-cache`、`max-age`、`min-fresh`、`max-stale`、
  `only-if-cached`（504）和 `Pragma: no-cache`；带 `Range` 的请求绕过缓存；POST、PUT、DELETE 等成功后删除
  该 URI 以及同主机的 `Location` 和 `Content-Location`。
- 带校验器的过期响应带着校验器向后发送（经 `proxy`，它转发被修改的字段，即 I40 的 local）；上游的 304 使它
  重新新鲜（按 RFC 9111 第 3.2 节合并字段），客户端由它回答，客户端自己的校验器匹配时回答 304。
- 内存层：每个 zone 一个存储，分 16 个分片，各有锁、`max_bytes` 的一份和最近最少使用顺序（以下标链接的
  链表，不用 unsafe）；消息体、字段和键都计入；同名缓存共享 zone，重载后保留（弱引用持有，没有缓存使用的
  zone 会被释放）。
- 响应在发出的同时保存，从不为此扣住：完整响应取其消息体；流式响应（代理超过 64 KiB 的，I16）由消息体旁路
  逐次复制，只在流完整结束时保存（未被截断、不以 trailer 结束、长度与声明一致）。缓存的响应过滤器是外层
  过滤器，因此放在 `compress` 前面时每种编码各是一个变体。同时进行的复制所占内存以 `max_bytes` 的四分之一
  为上限。
- 带认证中间件（`basic_auth`、`client_cert`、`forward_auth`、`jwt`）的路由，其缓存把每个请求都当作带
  `Authorization`，在编译配置时确定：fit 为认证中间件放行的每个请求做标记，对这些中间件而言结果相同，且
  每个请求没有开销。
- 配置：`zone`、`max_bytes`（`max_size`）、`max_entry_bytes`（`max_entry_size`，至多为 `max_bytes` 的
  十六分之一）、`default_ttl`；`disable_lock` 和 `lock_timeout` 被读取并保留给 I68 的缓存锁（在此之前每次
  未命中都各自获取）；`stale_while_revalidate`、`stale_if_error` 和 `disk` 以暂不支持拒绝（I68）。兼容性
  门禁的 `examples/cache` 现在只因 stale 窗口被拒绝。
- akio：提出并实现了 **A111**（`Response::tap_body`、`BodyTap`、`TapEnd`）、**A112**
  （`akio_http::parse_http_date`，RFC 9110 的三种格式）和 **A113**（`Context::add_outer_response_filter`），
  即 akio 分支 `claude/a111-body-tap` 上的 ADR-116，提交 `0e36f8d6`（基于 akio main `c5cf4353`），未推送。
  akit 锁定该提交；它进入 akio 的 main 后即可构建（已通过本地未提交的 `[patch]` 测试）。
- 测试：`crates/akit-cache/tests/cache.rs` 移植了 fit 的 `cache_test.go`（命中、可存储性、新鲜度、
  default_ttl、vary、条件请求、重新验证、请求指令、失效、消息体、客户端离开、压缩、参数、zone）和
  `auth_test.go`，覆盖 HTTP/1.1 和 h2c，以及代理的消息体（流式和 1 MiB 的被保存，被截断的不保存）和经代理的
  重新验证；语义、存储（淘汰、变体、zone）和同源位置的单元测试。fit 的回答来自缓存锁之处（请求等待时
  重新获取过期响应），测试期望 I67 的 `fwd=stale; stored` 并注明。
- 未完成：缓存锁、stale-while-revalidate、stale-if-error 和磁盘层（I68）；清除（I69）；akio 从磁盘文件
  发送的消息体不保存；fit 为自行认证请求的 handler 所做的标记（akit 没有这样的 handler）。
- 不做基准测试（规则 30）：要测的内容在开发计划的基准表中。

### 2026-11-24：I48，fit 的其余中间件

- 除 `cache`（I67）和 `wasm`（I71）外，fit 的中间件中 akit 缺少的是 `rewrite` 的 `regexp` 与 `replacement`，
  以及 `redirect` 的 `to` 中正则的分组，它们在等 akio API 需求 A32。I48 在 akio 中实现了 A32（ADR-115，分支
  `claude/a32-regex-captures`，`26c500b5`，基于 `c5cf4353`）：akio-regex 的 `find`、`find_iter`、`captures`、
  `expand` 和 `replace_all`，采用 Go `regexp` 的最左优先匹配与分组（重复按 Go 的形状编译，因此可匹配空文本的重复
  中的分组与 Go 一致）以及 Go 的模板规则，由带捕获的 Pike VM 以线性时间完成；在 132 000 个随机用例上与 Go 的
  `regexp` 核对。akit 锁定到该提交（`Cargo.toml`、`AKIO_REV`）。
- `akit-config` 读取 `rewrite` 的 `regexp`（与路由器一样编译，Go 的 ASCII 字符类）和 `replacement`（没有 regexp
  时报 "a replacement needs a regexp"，同 fit 的 `Provision`），并接受 `redirect` 的 `to` 中的分组；"尚不支持"
  的拒绝和 `REWRITE_FIT_ONLY` 已去掉。
- `akit-middleware` `rewrite`：按 fit 的顺序去前缀、替换、加前缀；解码后路径中的每处匹配被替换（Go 的
  `ReplaceAllString`），补回开头的 `/`，路径重新转义。不是 UTF-8 的解码路径保留其字节：正则运行期间，无效序列的
  每个字节用一个私用区字符代替，正如 Go 把它读作一个字符。`redirect`：`to` 中有 `$` 时，先填入在转义路径上匹配的
  分组，再填占位符，同 fit 的先 `Expand` 后 `expand`；由分组造成的开放重定向仍答 400。
- 移植了 fit 的 `TestRewrite`、`TestRedirect` 和 `TestParamErrors` 中关于正则的用例（`TestRedirect` 中有两个路径
  在到达中间件前已被 akit-listener 规范化或拒绝，与已有用例的说明相同）；路径重写、空匹配和非 UTF-8 字节的单元
  测试。
- 兼容性门禁：`rewrite.regexp` 与 `rewrite.replacement` 已支持；fit 的 `redirect` 和 `rewrite` 示例有效（27 个
  有效，52 个只因尚不支持被拒绝）。除 `cache` 和 `wasm` 外不再拒绝任何中间件。
- 未测量（规则 30）：开发计划中的基准行"重写与重定向的正则"。

### 2026-11-24：I54，上游 TLS

- `akit-config` 读取上游的 fit 字段 `tls_server_name`、`tls_ca_file`、`tls_cert_file` 和 `tls_key_file`
  （不再作为“尚未支持”拒绝），检查和位置与 fit 一致：除私钥外都需要 `"protocol": "h2"`，证书需要私钥，
  私钥也需要证书。
- `akit-tls` 的 `UpstreamTls::load`：以 CA 文件中的证书代替系统根证书，加载客户端证书及其私钥（未给 CA
  文件时保留系统根证书），ALPN 为 `h2` 和 `http/1.1`，与 fit 一致；在编译配置时读取（文件大小上限与监听器
  的相同）。加载失败的文件使配置失败，路径为 `upstreams[i].tls_ca_file`，或（凡与证书或私钥有关的）
  `upstreams[i].tls_cert_file`，与 fit 的 `upstream.New` 一致；不属于该证书的私钥在 akio 构建配置时发现
  （A109）。
- 图（`akit-runtime`）用这份配置创建 h2 连接池（`Pool::new_with_tls`，使随连接池启动的探测也用它：
  `with_tls` 无法改动已与探测器共享的连接池），并把这些文件加入图的 `cert_files`：`run -watch-certs`
  像监听器的文件一样监视它们，变化时重新应用配置，新连接池重新读取（对应 fit 的 `upstreamKey`；akit 的
  连接池不在图之间共享，任何变化都会新建）。
- `akit-upstream`：`tls_server_name` 是在 SNI 中发送并用于校验证书的名字，HTTP/2 上（`H2Client::connect`
  的名字参数）和服务器在 ALPN 中选择 HTTP/1.1 的地址上（akio 的 `Client::with_tls_server_name`，A108）都
  如此，两者的探测也一样。
- akio：A108（`Client::with_tls_server_name`）和 A109（`ClientConfigBuilder::build` 检查客户端证书的私钥
  与叶证书相符），ADR-117，在 akio 分支 `claude/a108-a109-upstream-tls`（`252fba42`，基于 `c5cf4353`，
  未推送）；在它合入 akio 的 main 之前，akit 锁定到该提交。
- 测试：fit 的 `TestValidate` “upstream tls without h2”，含位置（`akit-config/tests/upstreams.rs`）；
  fit 的 `TestTLSCAFileErrors` 和 `TestTLSCertFileErrors`（`akit-tls/tests/upstream.rs`）；fit 的
  `TestCompileFails` 中的 `tls_ca_file` 以及 `TestSharedPools` 中被监视的文件
  （`akit-runtime/tests/upstream_tls.rs`）；fit 的 `TestClientCertificate` 和 `TestProbeProtocols` 在 h2
  上带 `tls_ca_file` 与 `tls_server_name`，错误或缺少服务器名，以及 HTTP/1.1 回退保留名字和证书，探测也
  一样（`akit-proxy/tests/upstream_tls.rs`）。
- 兼容性：四个字段加入 `tests/compat/supported.txt`（3821 个字段中支持 225 个）；没有用例变化。
- 未做基准测试（规则 30）：上游 TLS 在每个连接建立时设置，不在请求路径上；除 dev plan 中已有的 h2 上游
  条目外，没有新的需要测量的内容。

### 2026-11-24：I50，gRPC 代理

- 从 M3 提前完成，基于 I18 和 I18b 的 HTTP/2 上游；不需要新的 akio 需求（为它预留的 A102–A104
  未使用）。
- `akit-proxy` 的 `grpc` 模块，参照 fit 的 `handler/proxy/grpc.go`：gRPC 调用是只有一个
  `Content-Type`、其值为 `application/grpc` 或其子类型（`+proto`、`;...`；不含
  `application/grpc-web`）的 HTTP/2 或 HTTP/3 `POST`，走代理的普通转发：消息到达即双向流式传输，
  联动背压（有 64 KiB 等待发往上游时挂起客户端的读，有 64 KiB 等待发往客户端时关闭上游流的窗口），
  上游的 trailer 跟在响应之后。无论声明多长，请求体都从请求头起就流式交给代理（fit 的
  `StreamRequestBodyAtHeader`），所以等待第一条消息应答的客户端能得到应答。与 fit 一样，调用需要
  `h2` 或 `h2c` 上游。
- 代理额外做的：向上游发送 `TE: trailers`；`grpc-timeout`（八位数字加单位）像代理的 `timeout`
  一样约束调用，两者取较短者；路由的较短时，以调用的 `grpc-timeout` 告知上游，向上取整（fit 的
  `callTimeout`、`encodeGRPCTimeout`）。响应从不收齐后整体发送（fit 的 `blockResponse` 关闭）：
  trailers-only 响应（响应头中带 `grpc-status`）按原样以单个 HEADERS 帧转发，其他响应无论多短都
  流式发送。
- 按 gRPC 的方式应答失败（fit 的 `grpcFail`、`grpcBroken`）：没有得到响应的调用以其自身内容类型的
  trailers-only `200` 应答，`grpc-status` 和 `grpc-message` 取自代理本会发送的 HTTP 状态：504 和
  408 为 `DEADLINE_EXCEEDED`，413 为 `RESOURCE_EXHAUSTED`，400 为 `INTERNAL`，其余（502、503）为
  `UNAVAILABLE`。中途中断的响应在已到达的消息之后以 `UNAVAILABLE` trailer 结束，超时则为
  `DEADLINE_EXCEEDED`。消息文本是 fit 的，`fit:` 换成 `akit:`。
- 被代理自身期限（其 `timeout`，或调用的 `grpc-timeout`）截断的请求，无论是否 gRPC，不再计入其
  地址的被动健康检查（`Exchange::gave_up`），与 fit 的 `timedOut` 一样让地址留在轮转中。
- `akit_runtime::Env::with_upstreams` 改为公开，供在手工建立的连接池上配置的模块使用（在 I54 的
  `tls_*` 字段之前，`h2` 测试的连接池由此获得测试 CA）。
- 配置：没有新字段（`examples/grpc` 原本就有效）；`supported.txt` 记下 I50。
- 测试（fit 的 `grpc_test.go`，移植在 `crates/akit-proxy/tests/grpc.rs`，用 akio-grpc 的客户端和
  服务器代替 fit 所用的 fib 的）：一元调用（回显、200 KB、上游的状态和 trailer、期限传到上游）、
  向下 2000 条消息、向上 500 条、逐条应答的 50 条（其中有 300 KB 的）、向不读的客户端和不读的上游
  各 32 MB、没有上游、上游中途离开、被 `grpc-timeout` 和路由超时截断的调用（上游仍在轮转中），
  各自经 h2c 和带 TLS 的 h2；以及响应中途到期、发往上游的字段、trailers-only 应答的转发，和辅助
  函数（fit 的 `TestGRPCHelpers`）。
- 未完成：针对 grpc-go 的运行（计划的完成标准）：fit 的测试在这里跑在 akio-grpc 上，而 grpc-go
  需要 CI 没有的 Go 工具链（见开发计划 I50 的拆分一行）。
- 没有基准测试（规则 30）：对比 fit 的每秒 gRPC 调用数和一个流上的每秒消息数已写进开发计划的
  基准测试表。

### 2026-11-24：I46，加权上游、镜像、故障转移

- `akit-config` 读取 `proxy` handler 的 `upstreams`（`Split`：`upstream`，`weight` 取 1 到 1000，
  两者必填）、`failover` 和 `mirror`（`Mirror`：必填的 `upstream`，以及取 0 到 100 的 `percent`，
  省略时为 100），带 fit 的检查和消息：`upstream` 与 `upstreams` 互为替代（只设一个），且至少要有
  其一。它们离开 `PROXY_FIT_ONLY`，其中只剩 `splice_uploads`。`MAX_MIRRORED` 即 fit 的
  `maxMirrored`，256。
- `akit-upstream` 的 `Split` 即 fit 的 `pickPool`：一个上游，或按权重随机抽取其中一个（均衡器的
  每线程 wyrand，Lemire 归约）；当所选上游没有地址在轮转中（被动健康检查或探测，`Pool::available`）
  而故障转移上游有时，改用故障转移上游。一次选择不分配内存也不加锁（`tests/no_alloc.rs`）；只有一个
  上游且没有故障转移的代理不抽取随机数。`split::chance` 按镜像的百分比抽取。
- `akit-proxy`：handler 持有 `Split` 代替原来的连接池，每个请求先选上游；地址、会话保持 cookie、
  重试（I45，留在所选上游之内）、WebSocket 和访问日志中的地址都随所选上游。每个上游名字在 handler
  provision 时查找，错误落在其字段上（`upstreams[1].upstream`、`failover`、`mirror.upstream`），
  与 fit 的 `provisionSplit` 相同。
- `mirror.rs`：消息体完整到达的请求（流式的不算，它只能读一次），按 `percent` 选中后，在请求发往
  上游之前把副本发给镜像，与 fit 的 `copyTo` 相同：方法、目标和出站字段（去掉 `Trailer`：副本按
  长度发送），消息体的副本（最多 64 KiB），不带客户端的取消令牌，使用镜像上游自己的超时。副本的
  响应由一个连接池交换读取并丢弃，该交换持有一个原子计数的名额：同时超过 256 个副本时不再复制请求，
  因此慢镜像最多占用 256 个连接或等待中的请求和 16 MiB 消息体，也从不拖慢请求。
- 测试：`akit-config` 的 `tests/proxy.rs` `split`（fit 的 `TestSplitParams` 中解码器的部分）；
  `akit-upstream` 的 `tests/split.rs`（按权重的份额、故障转移、权重之后的故障转移、百分比）；
  `akit-proxy` 中名额上限和副本的单元测试，以及经 akit 监听器的 `tests/split.rs`：fit 的
  `TestSplitWeights`、`TestSplitFailover`、`TestMirror`、`TestMirrorDoesNotSlow`（镜像不回应任何
  副本：400 个请求照样得到回答，副本停在上限）、`TestMirrorPercent` 和 `TestSplitParams`（名字）；
  故障转移替代拆分中的一个上游、流式消息体不复制、镜像不可达。
- 兼容性门禁：`tests/compat/supported.txt` 新增 7 个字段（以及门禁的样例、`SPLIT_FIELDS` 和
  `MIRROR_FIELDS`）；`make compat-bless`：`examples/load-balancing` 有效（80 个夹具：26 个有效，
  53 个只因暂不支持被拒绝，1 个跳过，没有失败）。顺带修复了状态表“可用功能”中错乱的中间件列表和门禁
  计数。
- 没有 akio API 需求：连接池的异步交换、`available` 和均衡器的随机数生成器已经足够。未做基准测试
  （规则 30）；开发计划的基准表为它加了一行。

### 2026-11-24：I44，JWT 与 JWKS

- `akit-config` 读取 fit 的 `jwt`（`jwks_url`、`jwks_refresh_interval` 及其旧名
  `jwks_refresh`、`public_key_files`、`hmac_secret_file`、`algorithms`、`issuer`、`audience`、
  `leeway`、`token_from`、`claims_headers`）为 `Handler::Jwt`，带 fit 的 `Provision` 中不需要读文件的
  检查及其消息：需要一个密钥；`jwks_url` 是带主机的 http 或 https URL；`algorithms` 是 fit 的十三种
  之一（`JWT_ALGORITHMS`）；`token_from` 为 `header`、`header:Name`、`cookie:name` 或 `query:name`；
  `claims_headers` 的字段是 HTTP token 且指明一个 claim；`jwks_refresh_interval` 省略或为 0 时为 1h。
  同时给出刷新间隔的两个名字是错误，与 fit 相同（akit 没有警告，旧名被静默接受）。它离开
  `FIT_MIDDLEWARE`。
- `akit-middleware` `jws`：按 fit 的 `parseToken` 拆开令牌（最多 16 KiB，三段，base64url，JSON 头中
  `alg` 和 `kid` 为字符串，不能有 `crit`），用 akio-crypto 校验：RS256/384/512 和 PS256/384/512
  （`rsa::PublicKey`，PSS 盐长等于哈希长度）、ES256/384/512（P-256、P-384、P-521，定长 `r || s`）、
  EdDSA（Ed25519）和 HS256/384/512（完整标签，常量时间比较）。密钥只匹配其类型和长度，带 `alg` 的
  JWK 只用于该算法，HMAC 密钥只能是配置自己的密钥；`none` 从不接受。密钥来自 PEM 文件（`PUBLIC KEY`、
  `RSA PUBLIC KEY`、`CERTIFICATE`；其他块跳过）和至少 32 字节的 HMAC 密钥文件，在中间件装配时读取
  （每个最多 1 MiB）；JWK 集按 fit 的 `parseJWKS` 解析（用于加密的、其他类型的、HMAC 的或无法解析的
  密钥被略过；拒绝小于 2048 位的 RSA 和不在曲线上的 EC 点）。
- `akit-middleware` `jwt`：按 fit 的方式从 `Authorization: Bearer`、字段、cookie 或 query 参数取令牌；
  `exp` 必需、`nbf` 可选，都在 `leeway` 内，`iss` 和 `aud` 按 fit 的 `check` 检查；回答 401 和
  `Bearer realm="fit"`（发送了令牌时加 `error="invalid_token"`）。校验过的令牌按 SHA-256 保存在
  I/O 线程自己的缓存里（每线程 1024 个），在密钥的代数不变时有效（每个请求一次原子读），每次使用都
  检查 `exp`/`nbf`，其 `claims_headers` 字段的值一并保存：再次使用的令牌只需一次哈希和一次查找。
  `claims_headers` 把字段设为 claim（字符串原样，其他按 Go 的 `json.Marshal` 写出，并处理为可作字段
  值），之前先删除客户端发来的这些字段的所有写法。
- `akit-middleware` `jwks`：引擎就绪时（引擎槽的 `when_set`，现在在 akit-upstream 中公开）经 akio 的
  客户端在 akit 的引擎上获取 JWK 集，此后每 `jwks_refresh_interval` 一次，令牌指明集合没有的 kid 时也
  获取，最多每 10s 一次（fit 的 `minRefetch`）；最多 1 MiB，只接受 200，10s，最多十次重定向。在客户端的
  回调中解析；刷新和延迟的获取是 akit 定时器线程上的任务。获取失败时保留原有密钥。令牌指明未知 kid
  的请求，以及首次获取完成前的任何请求，挂起等待获取（`Context::suspend`，A65），同时最多 1024 个
  （超过时回答 401），并在获取结束的线程上重新校验；基于已被替换的集合检查过的请求立即重新校验。
  配置释放时，取消进行中的获取，并用现有密钥校验仍在等待的请求。
- body 流式交给 handler 的请求无法挂起（ADR-109）：只有请求可能等待集合时（首次获取前，或 kid 不在
  集合中），`jwt` 的 `body_mode` 才在监听器的 `max_body_bytes` 内要求整个 body；其他请求照常流式。
  A75 可让它们也流式。
- 测试：单元测试覆盖密钥集查找、令牌拆分和被拒绝的 claims、算法与匹配的密钥、Go 的 JSON 写法、
  NumericDate、JWK 密钥和集合、claims 与时钟、令牌的位置、缓存（其他密钥、另一个中间件、上限）、
  密钥文件的错误、基于过期集合的等待、等待者上限以及获取失败时保留密钥。`tests/jwt.rs` 经 akit 的
  监听器：fit 的 `TestJWTAlgorithms`、`TestJWTClaims`（以及缓存中的令牌过期）、`TestJWTTokenFrom`、
  `TestJWTClaimsHeaders`、`TestJWTParams` 和 `TestJWTMore` 的参数部分、`TestJWKSRotation`、
  `TestJWKSRefresh`（带 `TestJWKSGet` 的失败：错误 JSON、500、过大、中断）、
  `TestJWTClientGoneWaiting`；JWK 集不可达后恢复；200 KiB 的 body 经 `jwt` 到 `proxy` handler，其中
  一个等待集合。fit 每次运行生成 RSA 密钥；akio-crypto 不生成 RSA 密钥，所以 `tests/data` 中有两个
  2048 位的测试密钥。
- 兼容性门禁：`tests/compat/supported.txt` 增加 13 个字段；`make compat-bless`：`examples/jwt` 有效，
  `examples/whip-whep` 不再因 `jwt` 被拒绝（80 个夹具：23 个有效，56 个只因暂不支持被拒绝，1 个跳过，
  没有失败）。顺带修正状态表“可用功能”中重复的门禁计数。
- 与 fit 的不同：claims 不传给后续中间件，用户也不进入访问日志（A29），与 I42 相同；令牌的 JSON 可含
  akit-config 读取器允许的内容（注释、尾随逗号），不能有重复的键；拒绝来自 PEM 文件的小于 2048 位的
  RSA 密钥（akio-crypto 的下限）；缓存按 I/O 线程而不是 16 个共享分片；可能等待的请求的流式 body 先被
  整个读入（A75）。
- 待测量，在维护者的基准任务中（规则 30）：见开发计划中 I44 的基准行。

### 2026-11-24：I49，WebSocket 代理

- 从 M3 提前完成，基于本迭代加进 akio 的 akio API 需求 **A93** 和 **A94**（ADR-113，akio 分支
  `claude/a93-a94-tunnels`，提交 `30cdf93a`，基于 akio main `f6750e93`）：akio-http 的客户端在 `101` 时交出连接
  （`ClientRequest::upgrade`、`ResponseHandler::on_upgrade`），对 HTTP/2 扩展 CONNECT 的流式 `2xx` 应答即两半
  各自关闭的隧道（HTTP/3 本就如此）。akit 锁定该提交；协调会话会把它与 A66 一起合入 akio 的 main，再把锁定移到
  合并后的提交。
- `akit-proxy` 的 `websocket` 模块，仿照 fit 的 `handler/proxy/upgrade.go`：请求切换协议的 HTTP/1.1 请求
  （`Connection: upgrade` 且 `Upgrade` 不是 `h2c`）照原样作为 upgrade 发往上游；HTTP/2 或 HTTP/3 中对
  `websocket` 的扩展 CONNECT 作为它所代表的 HTTP/1.1 握手发出（`GET`，akit 自己的 `Sec-WebSocket-Key`）；对其他
  协议的扩展 CONNECT 回答 501，无法切换协议的 h2 或 h2c 上游回答 502。字段与普通代理请求一样发往上游（转发字段、
  可信代理、`Host`），地址由上游的负载均衡和粘性 cookie 选择。
- 切换：HTTP/1.1 客户端的连接在握手时被接管（`Context::upgrade_raw`），上游切换后收到上游的 `101` 及其字段；
  HTTP/2 或 HTTP/3 客户端收到带这些字段（去掉 `Sec-WebSocket-Accept`）的流式 `200`。上游连接离开连接池
  （`akit-upstream` 的 `Exchange::on_upgrade`）：立即归还其名额，打开的 WebSocket 不会占满 `max_conns` 或
  `max_pending`；关闭前它计入该地址的连接数（`least_conn`）。
- 隧道：字节在读到它的回调中继续发出；等待发往一侧的数据超过 64 KiB（fit 的 `tunnelWatermark`）时暂停另一侧的
  读取（HTTP/1.1 连接的、HTTP/2 或 HTTP/3 流的 `BodyReads`、上游连接的），直到那一侧排空。切换前客户端发来的
  字节最多保留 64 KiB，期间暂停其读取。一侧结束后，另一侧在发完已发数据后结束；客户端离开时关闭上游连接，或放弃
  握手（其 `CancelToken`）。`tunnel_idle_timeout`（未设置或为 0 时取 fit 的 10 分钟）关闭两个方向都没有数据的
  隧道，在 akit 的计时线程上检查。
- 不切换的应答交给客户端：HTTP/2 和 HTTP/3 经由代理的普通中继（超过 64 KiB 时带背压流式发送），但 `2xx` 会被
  客户端当作已打开的隧道，因此回答 502 并关闭上游连接；HTTP/1.1 则写在 akit 已接管的连接上，按长度（或连接关闭）
  分帧，64 KiB 以内整体发送（被截断时回答 502），超过时带背压流式发送（被截断时同样截断）。握手没有应答时按 fit
  回答 502、503 或 504。
- 配置：代理的 `tunnel_idle_timeout`（`akit_config::DEFAULT_TUNNEL_IDLE_TIMEOUT`）。
- 测试（移植 fit 的 `websocket_test.go`、`websocket_ext_test.go`、`websocket_unix_test.go` 和
  `upgrade_test.go`，在 `crates/akit-proxy/tests/websocket.rs`）：带子协议的回显与双向同时 16 MiB、拒绝与无上游、
  空闲超时、WebSocket 离开连接池、对不读取的客户端灌入 64 MiB（HTTP/1.1、h2c（其流窗口）、HTTP/3（暂停其流的
  读取））、按长度、分块和连接关闭分帧的大拒绝应答且后端仍在轮转中、慢客户端的小拒绝应答、被截断的拒绝应答、
  拒绝应答途中离开的客户端、经 h2c、h2 和 h3 到 akio WebSocket 服务端的 WebSocket，以及不切换协议就应答扩展
  CONNECT 的上游。
- 未做：HTTP/1.1 WebSocket 的访问日志行（其 `101` 由 akit 写在接管的连接上，不经过写日志行的 akio 响应路径）；
  fit 在重载时排空已打开的隧道（它们一直运行到结束或空闲超时；引擎关闭时关闭它们）。
- 不做基准测试（规则 30）：需要测量的内容写在开发计划的基准表中。

### 2026-11-24：I47，压缩

- `akit-config` 把 fit 的 `compress` 中间件（`encodings`、`level`、`gzip_level`、`brotli_level`、`zstd_level`、
  `min_bytes` 及其旧名 `min_length`、`max_in_flight`）读入 `Handler::Compress`，级别按 fit 的 `Provision` 计算
  （gzip/brotli/zstd：fastest 1/1/1，default 5/4/3，better 7/6/6，按编号给出的级别取而代之），并做 fit 的检查：
  编码只能是 `br`、`zstd`、`gzip`，不得重复，级别在范围内。`compress` 离开 `FIT_MIDDLEWARE`；`static` 处理器的
  `compress` 离开 `STATIC_FIT_ONLY`。
- `akit-middleware` 的 `compress`，仿照 fit 的 `middleware/compress`：一个响应过滤器在提供的编码中按 q 值选出客户端
  最想要的（`x-gzip` 即 gzip，`*` 代表其余，相同时取先提供的）；只压缩状态码带消息体且不是 206、没有
  `Content-Encoding`（`identity` 除外）或 `Content-Range`、没有 `no-transform`、类型在 fib 的可压缩列表中、声明的长度
  （字段给出的，或完整消息体的）不小于 `min_bytes` 的响应。它加上 `Vary: Accept-Encoding`（HEAD 只加这个），删除
  `Content-Length`，把强 `ETag` 改为弱 `ETag`。完整的消息体在过滤器中一次压缩；流式的（代理的，I16）由 akio 逐次写入
  编码并刷新，上游发来的每一块都能立即解码到达客户端，处于转发的联动背压之下。流使用 fit 的窗口（2^18）。同时压缩
  的响应最多 `max_in_flight` 个：流式响应通过 akio 的 `Response::keep_until_sent` 占住名额，直到其 `BodyStream`
  结束或被丢弃（客户端离开）。
- `static` 处理器的 `compress`：akio 的 `Files::compress` 把内存中的文件也按每种编码各存一份压缩副本，在客户端首次
  首选该编码时，在 akio 的磁盘线程池上以 gzip 9、brotli 9、zstd 19 做一次；副本的标签是原文件的加上 `-编码`，没有
  `Accept-Ranges`；范围请求、小文件、不可压缩的类型以及节省不到二十分之一的副本都给原文件；磁盘上的孪生文件优先。
- akio：提出并实现了 **A90**（`akio_http::Coding`、`Response::set_stream_coding`、`is_streaming` 与
  `keep_until_sent`：akit 自己的过滤器设置流的编码，原先只有 akio 自己的 `compress` 中间件能做到）和 **A91**
  （`Files::compress`），在 akio 的分支 `claude/a90-a91-compress` 上（`b5cd3f25`，ADR-112，基于 akio main
  `f6750e93`），尚未合并。akit 的 `Cargo.toml` 仍锁定 `4f7169c4`：本提交要在锁定版本移到合并后的 akio 提交后才能
  构建；构建和测试用的是本地未提交的 `[patch]`。只提出未实现：**A92**，从磁盘文件发送的消息体也要编码。
- 测试：协商（fit 的 `TestNegotiate`）、适用性与 `Vary`（fit 的 `TestEligibleAndVary`）、可压缩类型和完整消息体解码
  的单元测试。`tests/compress.rs` 经 akit 的监听器，走 HTTP/1.1 和 h2c：fit 的 `TestCompress`（协商表、不压缩的响应、
  HEAD、弱标签）、`TestCompressStream`（每种编码，第二个事件写出前第一个事件已解码到达）、`TestCompressBounded`
  （超过 `max_in_flight` 不压缩；客户端离开的流归还名额）、`TestCompressReleased`、`TestCompressLevels` 和
  `TestCompressParams`；`min_bytes` 以下和恰好等于；经代理：上游停顿前的事件在停顿结束前解码到达，按长度给出的消息体
  被压缩，半秒不读的客户端完整拿到 4 MiB 消息体，中途离开的客户端归还名额；中间件放在 `static` 处理器前；经
  `static` 处理器的 fit `TestCompressCopies`。akio 中：自有过滤器设置编码、其守卫保留到流结束，以及副本。
- 兼容性门禁：`tests/compat/supported.txt` 增加 11 个字段；`make compat-bless`：`examples/compress` 有效（80 个
  夹具：23 个有效，56 个只因暂不支持被拒绝，1 个跳过，没有失败）。
- 与 fit 的差异：akio 从磁盘发送的消息体（大于 `max_file_bytes` 的静态文件）不压缩，而 fit 的中间件边读边压缩
  （A92）；流式消息体每次写入都刷新，而 fit 在处理器刷新时才刷新，所以大量小写入的流压缩率较低；静态副本不计入
  `max_cache_bytes`，akit 还没有这个字段（A15）；fit 的 `TestCacheOutside` 等待 `cache` 中间件（I67）。
- 待测（在维护者的基准任务中，规则 30）：见开发计划中 I47 的基准行。

### 2026-11-24：I45，重试、熔断、按路由超时

- `akit-config` 读取代理的 `retry`（`attempts`，1 到 10，默认 2；`statuses`，每个都是 5xx；`budget_ratio`，
  0 到 1，默认 0.2；`budget_min_per_second`，不小于 0，默认 10），得到 `Proxy::retry`（`Retry`）；读取
  `circuit_breaker` 中间件（`window`、`min_requests`、`error_ratio`、`statuses`、`slow_threshold`、
  `slow_ratio`、`open_duration`、`half_open_requests`，默认值同 fit），得到 `Handler::CircuitBreaker`
  （`CircuitBreaker`）。检查与报错同 fit（"404 is not a 5xx status"、"600 is not a status"、其标签的范围）。
- 重试（`akit-proxy` 的 `retry.rs`），同 fit 的 `retry.go`：连接建不起来（被拒、不可达、连接超时）或该地址已有
  `max_pending` 个请求在等的请求，无论方法都重发；幂等方法（GET、HEAD、OPTIONS、TRACE、PUT、DELETE）的请求
  在响应开始前失败（连接池已关闭时除外）或得到 `statuses` 中的状态时也重发。只重放 body 整体到达的请求（旁边
  保留的模板是最多 64 KiB 的 body 的一份拷贝）；body 流式转发的请求（超过 64 KiB、没有声明长度或带 trailer）
  只发一次。客户端已离开或超过路由 `timeout` 的请求也不重发。下一次尝试在上一次的连接池回调里发起，此时还没有
  任何东西写给客户端：由新的 `Relay` 接管客户端，按上游重新选地址并重新计算会话保持 cookie，同 fit 的重试。
  fit 的轮询序号属于上游，akit 的属于每个 I/O 线程，所以只要还有没试过的地址，就跳过已试过的。被重试的状态若
  body 较小就读完丢弃，以保留连接；较大的就取消。访问日志记录最终应答的地址。
- 预算同 fit（以及 Finagle 和 Linkerd）：每个请求增加 `budget_ratio` 次重试，每秒增加 `budget_min_per_second`
  次，最多十秒的量（至少 10），每次重试消耗一次。fit 的互斥锁在这里换成原子的 `f64`：向满的预算（健康上游的
  预算）存入只读不写；只有失败的尝试才会取用，取用时为累计的时间拿一把小锁。
- 按路由超时：代理的 `timeout`（I17）现在从代理收到请求时开始计时，覆盖所有尝试：每次尝试得到剩余的时间，
  超过后不再尝试（504）。
- 熔断器（`akit-middleware` 的 `breaker.rs`），同 fit 的 `middleware/breaker`：关闭、打开（503，带
  `Retry-After`）、半开时放行 `half_open_requests` 个探测请求，窗口分十步。请求的结果由响应过滤器读取，它在
  整体、代理或流式响应的头发出时看到状态；客户端离开的探测请求归还名额。fit 每个请求取两次的互斥锁，在熔断器
  关闭时换成原子状态和按步标记的原子计数；只有改变状态和半开时的放行才取锁。
- 测试：`crates/akit-proxy/tests/retry.rs` 移植 fit 的 `TestRetryConnectFailure`、
  `TestRetryNotResentOnceSent`（PUT 恰好到达上游四次：akio 的客户端不像 fib 那样自己重发）、
  `TestRetryStatuses` 和 `TestRetryBudget`，并增加：流式 body 不被重放，路由超时覆盖所有尝试，会话保持
  cookie 指向应答的地址，没有 `retry` 时不重试；`retry.rs` 的单元测试覆盖预算、方法和错误。fit 的
  `TestBreakerTransitions`、`HalfOpenLimit`、`Window`、`SlowAndStatuses` 和 `LateOutcome` 在 `breaker.rs`
  中用测试时钟运行；`crates/akit-middleware/tests/breaker.rs` 让 `respond` handler 和被代理上游的失败打开
  熔断器（之后上游收不到请求），用探测请求关闭它，并统计慢的代理响应。`crates/akit-config/tests/proxy.rs`
  和 `tests/breaker.rs` 移植 `TestRetryParams` 和 `TestBreakerParams`。fit 的 `TestRouteTimeout` 已随 I17
  移植（`tests/failure.rs`）。
- 兼容性门禁：新增 13 个字段；`make compat-bless`：`examples/circuit-breaker` 有效，
  `examples/load-balancing` 不再因 `retry` 被拒绝（80 个夹具：23 个有效，56 个只因暂不支持被拒绝，1 个跳过，
  没有失败）。没有 akio 需求（A87–A89 未使用）。
- 已检查：macOS 上 `make check`；gpu51 上持基准锁，在 epoll 和 `AKIO_TEST_BACKEND=uring` 下运行 akit-config、
  akit-proxy、akit-middleware、akit-runtime、akit-upstream 和 akit-compat 的测试：全部通过。
- 待测量，留给维护者的基准任务（规则 30）：见开发计划中 I45 的基准行。

### 2026-11-24：I18b，HTTP/2 上游 II：由 akio 重置、计时和控制节奏的流

- akio 移到 `d27e6a20`（规则 22）：A51–A53（ADR-110，`f6750e93`）以及本迭代的会话加入 akio 的 A66
  （ADR-111：`H2Client::request` 与 `open` 遵守 `response_header_timeout`、`response_idle_timeout` 和
  `CancelToken`，每一种都只重置该流）。
- `akit_upstream::h2` 去掉 I18 与 I17b 的临时方案。被放弃的流（响应头之后客户端离开、请求体未结束即被丢弃、
  其 `CancelToken`）由 akio 重置，以 `Cancelled` 失败，不算地址的失败；其连接继续承载其他流（不再退役）。
  `response_timeout`、`response_header_timeout` 和 `response_idle_timeout` 由 akio 负责
  （`ClientRequest::timeout` 与这两个字段），因此按地址的定时检查只关闭空闲连接，且只在地址有空闲连接时运行。
  被暂停的下载保持其流的窗口关闭（`H2Stream::hold_reads`；`HELD_LIMIT` 已去掉）；上传按
  `H2Upload::queued_bytes` 和 `on_drain` 控制节奏，与 HTTP/1.1 完全一样（h2 上的 `Upload::queued_bytes`、
  `on_drain`）；请求 trailer 结束请求体（`finish_with_trailers`）。
- 连接按服务器的 `SETTINGS_MAX_CONCURRENT_STREAMS` 确定容量（`H2Client::max_concurrent_streams`；服务器
  说明之前或未设上限时为 `STREAMS_PER_CONN`，即 100），地址的打开连接少于 `max_conns` 时拨新连接；超过后流在
  最空闲的连接上于 akio 中等待。不再打开的连接（收到 `GOAWAY` 后：`is_open`）不再接收新流，最后一个流结束后
  关闭。池的槽位仍把一个地址限制在 `max_conns` × 100 个流。
- 拨号受上游的 `connect_timeout` 约束（`H2Options::happy_eyeballs`），探测的拨号受检查的超时约束，作用于名字解析出
  多个地址时竞速的每个地址（单个地址使用引擎的超时，与 HTTP/1.1 一样：A12）。`h2` 在 ALPN
  中提供 `h2` 和 `http/1.1`，与 fit 一样；选择 `http/1.1` 的服务器让 akio 的拨号以 `NotH2` 失败，此后该地址由池
  的 HTTP/1 客户端（使用同一 TLS 配置）经 TLS 以 HTTP/1.1 访问，包括等待该次拨号的请求；其探测随之改变。
- 测试：`akit-upstream` `tests/h2.rs`（连接上的流达到服务器的上限，此处为 10；交换的超时、响应头超时和取消
  令牌各自重置其流而保留连接；已取消令牌的请求从不发送，且不算失败）；`akit-proxy` `tests/h2.rs`
  （`request_trailers` 不再忽略；慢客户端暂停 h2 下载后再完整取得，替代暂停上限临时方案的测试；服务器选择
  HTTP/1.1 的 `h2` 上游）；`akit-proxy` `tests/stream.rs`，针对测试自带的 h2c 上游（std 套接字，遵守流控）的
  fit 流式测试：向慢客户端下载、向慢上游上传各 64 MiB，按长度与分块，来自 HTTP/1.1 和 h2c 客户端，常驻内存的
  增长与 HTTP/1.1 一样有界（在 Linux 上检查）。
- 未测量（规则 30）：代理到 h2c 上游（开发计划的基准表，其行已更新：akio 的每流定时器、被暂停的下载和有节奏的
  上传，与 HTTP/1.1 对比）。

### 2026-11-24：I51，TCP 与 UDP 代理

- 从 M3 提前完成。`akit-config` 读取 fit 的 `services`（`name`、`listeners`、`upstream`、`idle_timeout`、
  `max_conns`），得到 `Config::services`；`listeners[].protocols` 接受 `tcp`（单独使用：与其他协议共用一个端口
  属于 I53，报告为暂不支持）和 `udp`（单独使用，同 fit 的要求）。检查与报错同 fit：名字有效且不重复，至少一个
  监听器，每个都是 `tcp` 或 `udp` 监听器且只属于一个服务，上游存在，每个 `tcp` 和 `udp` 监听器都有服务接管，
  路由不能挂在这种监听器上，不服务 HTTP 的监听器不能有 `tls`。`server_names` 和 `proxy_protocol`（I52）以及
  `access` 报告为暂不支持；带 `server_names` 的服务可以像 fit 那样共用一个 `tcp` 监听器，因此门禁只报告这些字段。
- `akit-l4`，仿照 fit 的 `l4` 包：每个配置中的服务对应一个 `Service`，带其上游的连接池，放在图中
  （`Graph::services`、`Graph::service(listener)`）；`ClientAcceptor` 服务 `tcp` 监听器（akio 的 TCP 监听器）或
  `udp` 监听器（`Listener::udp`，每个客户端地址是一个对端连接），每个连接在打开时交给当前图为该监听器指定的
  服务。在 akio 引擎上拨号上游时（`Engine::dial`、`dial_udp`；主机名由 `akio::dns` 解析，TCP 时由
  `HappyEyeballs` 在上游的 `connect_timeout` 内对其地址竞速）暂停读取客户端。数据在收到它的回调里继续发出；
  未发出的超过 64 KiB（`WATERMARK`）时暂停发送方的读取，直到另一侧 `on_drain`；两个连接都设为
  `set_relayed`。一侧结束后，在其已发数据发完后关闭另一侧（`close_after_send`）。UDP：每个数据报原样作为一个
  数据报转发，发不出就丢弃，上游套接字打开前最多保留 64 KiB；ICMP 拒绝会结束会话并计入该地址的失败。
- 地址：`Pool::stream_pick`（按上游的 `load_balancing`，`ip_hash` 按客户端地址，跳过下线的地址）在该地址上
  计一个连接，供 `least_conn` 读取，直到 `Pool::stream_done` 像请求那样记录结果（`max_fails`）；连接失败时换
  下一个选中的地址重试，最多三个。
- 限制：每个服务的 `max_conns`（10,000），超出的连接在接受时关闭（UDP 数据报丢弃）；空闲超时（TCP 10 分钟、
  UDP 1 分钟，或 `idle_timeout`）由每个服务在 akit 的定时线程（`akit_upstream::timer`，现已公开）上扫描，间隔为
  较短者的四分之一，在 10 毫秒到 1 秒之间；akio 的 UDP 对端超时在启动和每次重载时提高到各服务中最长的。两者都是
  akio API 需求 A45 和 **A78**（每个连接各自的空闲超时）之前的权宜之计。拨号 IP 字面量时由引擎的连接超时限定，
  与 HTTP 相同（A12；**A79** 请求带各自超时的 `Engine::dial`）。
- 监听器：与 HTTP 的监听器一起构建和重载（`akit_listener::engine`、`Listeners::reload`）；保留下来的 `tcp` 或
  `udp` 监听器把新连接交给新图的服务，旧图的服务仍在代理的连接在排空超时（`-shutdown-timeout`）后关闭，与 fit
  的重载相同。akit 启动时就有的 `udp` 监听器不能被重载移除或修改（akio A22），与 h3 相同。引擎关闭时立即关闭
  中转（`on_shutdown`）。
- 未完成：计划中的 eBPF sockmap 中转移到了 **I51b**：fit 没有让监听器开启它的字段，需要 akit 自己的字段（或先在
  fit 中加），而且它的空闲超时要依据套接字的字节计数。
- 测试：`crates/akit-listener/tests/l4.rs` 移植了 fit 的 `TestTCPEcho`、`TestTCPLarge`（双向各 32 MB）、
  `TestTCPSlowClient` 和 `TestTCPSlowUpstream`（64 MB 被挡住）、`TestTCPHangUp`、`TestTCPConnectFailure`、
  `TestTCPEngineStopped`、`TestTCPIdleTimeout`、`TestTCPMaxConns`、`TestTCPReload`、`TestUDPEcho`（200 个会话）、
  `TestUDPSessionsExpire`、`TestUDPMaxConns`、`TestUDPRefused` 和 `TestUDPReload`，另有主机名上游和地址上的计数；
  `crates/akit-config/tests/services.rs` 覆盖字段、默认值和检查。fit 的 `TestTCPDenyInternal` 等待
  `deny_internal`，SNI、PROXY 和协议嗅探的测试等待 I52 和 I53。
- 兼容性门禁：多支持 6 个字段；`make compat-bless`：`examples/l4-udp` 有效，`examples/l4-tcp` 只因
  `services[].server_names` 和 `proxy_protocol` 被拒绝（80 个夹具：19 个有效，60 个只因暂不支持被拒绝，1 个跳过，
  没有失败）。
- 验证：macOS 上 `make check`（fmt、clippy、工作区测试、`cargo deny`、文档和兼容性清单）；gpu51 上在基准锁下，
  分别用 epoll 和 `AKIO_TEST_BACKEND=uring` 运行 akit-config、akit-upstream、akit-l4、akit-runtime、
  akit-listener、akit-compat 和 akit 的测试：全部通过。打开几百个套接字的两个测试单独运行（gpu51 核数多，每个
  引擎的线程也多，所有测试的引擎同时打开时超过了 1,024 个文件的软限制）。
- 待测（维护者的基准任务，规则 30）：见开发计划中 I51–I53 的基准行。

### 2026-11-24：I17b，反向代理 III b：响应头与空闲超时、任意时刻的取消、写停滞

- 基于已锁定的 akio `4f7169c4`（A36–A38，ADR-108）。配置：读取上游的 `response_header_timeout` 和
  `response_idle_timeout`（`Upstream::response_header_timeout`、`response_idle_timeout`；省略或为 0 时取 fit 的
  60s，`DEFAULT_RESPONSE_HEADER_TIMEOUT`、`DEFAULT_RESPONSE_IDLE_TIMEOUT`）以及 `limits.write_stall_timeout`
  （`Limits::write_stall_timeout`，fit 的 30s，`DEFAULT_WRITE_STALL_TIMEOUT`）；这三个字段都不再被拒绝。
- `akit-upstream`：连接池的客户端把这两个超时作为默认值（`ClientOptions`），因此每个 HTTP/1.1 请求超过它们时以
  `ClientError::HeaderTimeout` 或 `IdleTimeout` 失败，并计入该地址的失败（`Pool::response_header_timeout`、
  `response_idle_timeout`）。请求可以带一个 `CancelToken`（已重新导出）；被取消时以 `ClientError::Cancelled`
  失败，`Error::is_upstream_failure` 和连接池都不把它计入该地址的失败。在 h2 上 akio 的客户端三者都不遵守
  （akio API 请求 A66），因此 `akit_upstream::h2` 的权宜定时检查除了截止时间外，每 100ms 还检查每个流的响应头
  （从请求体结束算起）、body 中途的静默（下载被暂停时停止计时）和它的令牌。
- `akit-proxy`：每个被代理的请求都带一个 `CancelToken`，由 `Context::on_gone` 取消（在 `detach` 之前注册，随流式
  响应一起转移）：客户端在响应头之前离开、经 HTTP/1.1 在上传中途离开或在下载中途离开，都会放弃上游请求，无论它
  处于哪个阶段，包括等待连接时；交换会报告它主动放弃，因此该地址不被计为失败。`status_of` 对任何超时都回答 504
  （`ClientError::is_timeout`）；响应头之后的空闲超时像 I17 的中断 body 一样截断响应。
- `akit-listener`：引擎的 `write_timeout` 就是配置的 `limits.write_stall_timeout`，`Listeners::reload` 把新值设到
  akio 的实时设置上（`Live::set_write_timeout`），已打开的连接也生效。`akit-runtime`：`Graph::write_stall_timeout`。
- 测试：`akit-proxy/tests/failure.rs` 移植了 `TestResponseHeaderTimeout`（超时后 504，经 HTTP/1.1 和 h2c，上游
  请求被放弃；长 body 不受它约束）、`TestResponseIdleTimeout`（有长度和无长度的 body 被截断，HTTP/1.1 和 h2c，
  h2c 的 `Fit-Error`）、`TestClientAbort` 的其余部分（经 HTTP/1.1 和 h2c 在响应头之前离开，经 HTTP/1.1 在上传
  中途离开，不计失败），以及一个更久不读的客户端不会被空闲超时截断、一个什么都不读的客户端被写停滞超时关闭；
  `tests/h2.rs` 对 h2c 上游做同样三项；`akit-upstream/tests/pool.rs` 测试响应头超时和被取消的请求（已发送的和
  等待中的）；`akit-listener/tests/reload.rs` 测试启动时设置的写超时及重载对它的修改；`akit-config` 测试这些
  配置字段及其错误。兼容性门禁的样例覆盖这三个字段：`examples/grpc` 现在有效（19 个有效，60 个被拒绝，被拒绝
  字段 58 个）。
- 与 fit 的差异：
  - `write_stall_timeout` 是 akio 引擎的写超时：连接在最后一次进展之后的一到两个超时之间被关闭，HTTP/2 是整个
    连接被关闭而不是重置单个流，HTTP/3 不受它约束（akio 的写超时只针对 TCP）。
  - 经 h2 上游时，首部超时、空闲超时和取消在一个检查周期（100ms）内被发现，因此被放弃的流仍任其运行，其连接
    停收新流（A51、A66）。
  - 在等待连接时被取消的请求，在该地址的队列里保留位置直到有空闲连接，然后不发送、立即失败。
- 一处无关的测试修正：`akit-upstream` 的 `an_upstream_that_refuses_connections` 不再假定测试线程的轮询从第一个
  地址开始。
- 留给基准测试任务：每个被代理的请求现在创建一个 `CancelToken`（一次分配）并注册一个 `on_gone` 回调；经 h2c 时
  每个 DATA 帧重新启用空闲超时（读一次时钟、一次存储）。[基准表](dev-plan.zh-CN.md#基准与优化状态)中 proxy
  场景的一行写明了这些。

### 2026-11-24：I33，默认 HTTPS

- fit 的 `auto_https`（`config/autohttps.go`），放在 `akit-config` 新的 `auto_https` 模块：该字段（`disable`、
  `disable_redirects`、`http_port`、`https_port`；不写时开启、带重定向、端口 80 和 443，写 0 取默认值）和 fit 的
  `Effective`。没有写监听器的配置得到一个 `:https_port` 上的 `https` 监听器（HTTP/1.1 和 h2），其 `tls.acme` 是所有路由的
  主机名，除非关闭重定向，还得到一个 `:http_port` 上的 `http` 监听器；路由可以指名这两个监听器。写出的监听器若 `tls` 为空
  （没有证书、没有名字，包括 `acme: []`），得到它所服务路由的主机名。名字按 fit 的 `AutoHTTPSName`：两个标签以上，小写，
  每个一次，不含通配符、IP 地址和私有名字（`localhost`、`local`、`internal`、`home.arpa`、`invalid` 及其子域名）。
- 在读配置时一次算好：`Config::listeners` 存放实际服务的监听器，写出的或生成的（fit 保留写出的，在需要时调用
  `Effective`），所以运行时像其他监听器一样绑定、重载时比较和服务它们；`Config::auto_https` 保留该字段。生成的 `http`
  监听器带 `Listener::redirect_https`，即它重定向到的端口；图不给它路由。
- 重定向（fit 的 `redirectHTTPS`）：在这样的监听器上，没有路由接收的请求（HTTP-01 的质询除外，它们像在所有明文监听器上
  一样先被回答）得到 308，指向 `https://` 加去掉端口的请求主机（IPv6 地址加方括号；没有 `Host` 时取目标的 authority）、
  非 443 时加 `:port`，以及相同的目标（绝对形式目标的路径和 query）；没有主机时回答 400 和 fit 的 body。监听器的端口
  只在没有路由接收请求时读取，所以有路由的请求不比以前多做工作。
- fit 的检查和消息：没有监听器也没有主机名（`at least one listener is needed, or a route that matches a host name for
  HTTPS by default`），关闭时没有监听器，HTTP 端口等于 HTTPS 端口（报在 `auto_https.http_port`），以及得不到名字的空
  `tls`（报在该 `tls`）。没有监听器的配置和空 `tls` 的"暂不支持"拒绝已去掉。**与 fit 不同**：既没有监听器也没有路由的
  配置仍被接受（akit 只启动 Admin API），fit 则拒绝，除非它有控制台或角色。
- 测试：`akit-config` 的名字单元测试（fit 的 `TestAutoHTTPSName`）；`tests/auto_https.rs` 移植 fit 的 `TestEffective`
  以及 `config_test.go` 和 `more_test.go` 的检查，带位置；`akit-listener` 的重定向位置单元测试（fit 的
  `TestRedirectEdges`）和 `tests/auto_https.rs`：fit 的 `TestAutoHTTPSRedirect`（生成的监听器和端口，GET 和 POST 的
  308，带端口的主机和 IPv6，未知令牌的质询路径，绝对形式的目标），不重定向，写出的空 `tls` 监听器，以及 fit 的
  `TestAutoHTTPSFromPebble`：只写一个主机名的配置通过生成的 `http` 监听器上的 HTTP-01 从 Pebble 获取证书并在 `https`
  上服务，`http` 重定向其余请求。
- 兼容性门禁：`auto_https` 及其四个字段加入 `tests/compat/supported.txt`；`make compat-bless`：`examples/auto-https`
  有效（80 个夹具：19 个有效，60 个只因暂不支持被拒绝，1 个跳过，没有失败）。
- 检查：macOS 上 `make check`（带 Pebble）；gpu51 上在 bench 锁下，epoll 和 `AKIO_TEST_BACKEND=uring` 下，akit-config、
  akit-compat、akit-tls、akit-runtime、akit-listener 和 akit 的测试：全部通过。没有 akio 请求。
- 待测量：无；有路由的请求走与以前相同的路径（规则 24：控制面，重定向不在热路径上）。

### 2026-11-24：I43，ForwardAuth

- `akit-config` 把 fit 的 `forward_auth`（`upstream`、`uri`、`request_headers`、`copy_headers`、`timeout`）读成
  `Handler::ForwardAuth`，带 fit 的 `Provision` 的检查和提示：`uri` 是路径，可带查询（以 `/` 开头，无控制字符，
  路径中的 `%` 转义有效），字段名是 HTTP token，`timeout` 省略或为 0 时为 10s。它从 `FIT_MIDDLEWARE` 移除。
  `upstream` 是否为配置中的 upstream，与代理一样在中间件 provision 时检查。
- `akit-middleware` 的 `forward`：经该 upstream 的连接池、按其负载均衡向认证服务发送对 `uri` 的 `GET`（路径按
  Go 的写法转义），带上请求的字段（`request_headers` 列出的那些；省略时为除逐跳字段、`Connection` 列出的字段以及
  `Content-Length`、`Content-Type` 和 `Expect` 之外的全部）、它的 `Host`，以及 `X-Forwarded-Method`、`-Proto`、
  `-Host`、`-Uri` 和 `-For`。请求被挂起（`Context::suspend`，akio 的 A65），从连接池的回调中恢复，等待期间不占
  I/O 线程。2xx 应答放行请求，并带上 `copy_headers` 列出的字段，此前先移除客户端发来的这些字段的各种写法（与
  `basic_auth` 的 `user_header` 相同）；应答的消息体小时读完丢弃，否则放弃。其他应答交给客户端：状态、除逐跳
  字段外的字段，以及 64 KiB 以内的消息体（更大的放弃，应答不带消息体，与 fit 相同）。超过 `timeout`（即请求在
  连接池上的超时）回 504；认证服务连不上或失败回 502；其间连接池已关闭回 503。请求等待时客户端离开会取消认证
  请求（`Context::on_gone` 与 akio 的 `CancelToken`，A37/A38），不计入认证服务的健康状况。
- 消息体本会流式交给处理器的请求不能挂起（ADR-109）：与 `basic_auth` 一样，`forward_auth` 的 `body_mode` 要求
  整体读取（在监听器的 `max_body_bytes` 之内）。已向 akio 提出 **A75**（挂起消息体流式读取的请求，并暂停其读取）。
- 测试：字段名、默认发送的字段、origin form 和失败状态的单元测试。`tests/forward_auth.rs` 经 akit 的监听器，
  认证服务跑在普通线程上：fit 的 `TestForwardAuth`（带认证服务的字段和它看到的内容放行，各种写法的伪造字段，
  带字段和消息体转交的重定向，丢弃 200 KiB 消息体的 401，超时后 504）、`TestForwardAuthUnreachable`（502）、
  `TestForwardAuthClientGone`（认证服务看到它的请求被放弃）、`TestForwardAuthSlowService`（128 个请求同时等待
  半秒，此时另一条路由在 250 ms 内应答）和 `TestForwardAuthParams`；`request_headers` 限制发送的内容，检查
  之前整体读取的 chunked 消息体，以及 fit 的 `examples/forward-auth` 的形态：`proxy` handler 之前的
  `forward_auth`，应用收到复制的字段和 200 KiB 的消息体。fit 的 `TestForwardAuthAuthenticates` 在 akit 中还没有对应（与 I42 一样，请求
  不标记为已认证）。
- 兼容性门禁：`tests/compat/supported.txt` 新增 7 个字段；`make compat-bless`：`examples/forward-auth` 有效
  （80 个夹具：19 个有效，60 个只因暂不支持被拒绝，1 个跳过，没有失败）。
- 与 fit 的不同：本会流式的请求消息体在检查前整体读取（A75）；配置被替换时 fit 对等待中检查的排空，在这里是连接池
  的排空（检查完成后由旧图的处理器作答）；请求不为访问日志或缓存标记为已认证（与 I42 相同）。
- 留给维护者的基准任务测量（规则 30）：见开发计划中 I43 的基准行。

### 2026-11-24：I29，文件 provider

- `akit run -watch`（fit 的文件 provider，`provider/file`，对应 fit 的 `run -watch`）：每隔
  `-watch-interval`（1s）查看一次配置的文件，变更稳定 500 ms 之后，且文件内容与上次应用的不同
  （文件名和内容的 SHA-256）时，像 SIGHUP 一样应用它们：通过 `Configs::update` 生成来源为 `file`
  的新版本，与 SIGHUP 和 Admin API 走同一条路径（先输出 `akit: FILES changed; reloading`，然后是
  fit 的重载输出）。按 fit 的决定（fit 的决策 006）它轮询大小和修改时间，而不是向系统要事件：
  不引入依赖，在所有系统上表现一致，被改名替换到位的文件和原地写入的文件都能看到。`-c` 指定的
  路径对应的文件在每次查看时重新查找（`akit_config::config_files`），所以目录里新增或移走文件
  也算变更。错误的修改只报告一次（fit 的输出和错误），正在运行的配置保留到下一次修改；读不了的
  文件（被删除、正在替换）只报告一次（`akit: watching FILES: ...`），恢复后再应用；只是 touch，
  或者改了又改回去，不会应用任何东西。SIGHUP 会把它读到并应用的内容告诉监视器，监视器不会再应用
  一次。所有文件合计最多读 64 MiB（`akit_config::MAX_CONFIG_BYTES`），与 fit 相同。
- `akit_admin::Watcher`（`crates/akit-admin/src/watch.rs`），即 fit 的 `file.Watcher`，带
  `WatchOptions`（防抖时间、最多读取多少、路径如何展开），`tick(now, apply)` 返回
  `WatchReport`（`Unreadable(WatchError)` 或 `Failed`），以及 `applied(files)`。它和 fit 一样
  从"文件刚刚变过"的状态开始，所以在读取配置之后、监视器启动之前的修改也会被应用。
- `akit-config`：`ConfigFile`（文件的路径和字节）、`read_config_files`（与之前一样有上限）和
  `load_config_files`（从已读取的文件得到配置及其 JSON 文档）；`load_files` 和 `from_files`
  基于它们实现。`akit run` 在启动和 SIGHUP 时只读一次文件，并加载读到的内容，使监视器比较的
  正好是已应用的内容。
- 测试：`akit-admin/tests/watch.rs`（12 个，fit 的 `provider/file` 测试：修改、改名替换到位、
  touch、监视前的修改、连续写入只应用一次、错误修改只报告一次且下一次修改被应用、文件删除后恢复、
  文件过大、以其他方式应用过的内容、目录中新增和移走文件、akit 自己的目录读取、默认值、找不到的
  文件、把目录当文件读）；`akit/tests/watch.rs`（2 个，用 `akit run -watch` 端到端，对应 fit 的
  `TestServeWatch`、`TestWatch` 和 `TestConfigDirectory`：每次修改都生效，SIGHUP 应用的修改不会
  再次应用，错误修改只报告一次且正在运行的配置继续服务，删除的文件被报告、恢复后应用，touch 被
  忽略；由 `.conf` 和 YAML 文件组成的目录，新增、移走和写坏文件，按文件名报告）。`cli.rs`：
  `-watch` 不再是用法错误。
- 验证：macOS 上 `make check`；akit-config、akit-admin 和 akit 的测试在 Linux（基准测试主机
  gpu51）上用 epoll 和 io_uring 运行。
- 与计划和 fit 的差异：
  - 监视器在控制线程的循环里被驱动，与 `-watch-certs` 的监视器并排（共用 `-watch-interval`），
    而不是在自己的 goroutine 上：它发起的重载、SIGHUP 的和 Admin API 的在那里依次执行，都不会
    阻塞 I/O 线程（规则 1）。
  - fit 在配置保存在它的 store 中时拒绝 `-watch`；akit 还没有 store。
  - 没有配置字段：文件 provider 和 fit 一样是一个参数，所以 `tests/compat/supported.txt` 不变。
    Unix 控制套接字（A23）不在此范围内。
- 没有基准测试：控制面（规则 24）。

### 2026-11-24：I42，Basic 认证与 mTLS

- 在 I41 之后从 M3 提前完成。`akit-config` 读取 fit 的 `basic_auth`（`realm`、带 `username` 和 `password_hash`
  的 `users`、`user_header`、`forward_authorization`）和 `client_cert`（`optional`、`subject_header`），得到
  `Handler::BasicAuth` 和 `ClientCert`，检查与报错同 fit 的 `Provision`（realm 不含引号、反斜杠或控制字符；至少
  一个用户；用户名非空且不含冒号或控制字符，不重复；哈希须能被 `akio_crypto::bcrypt` 解析，并给出原因；字段名须是
  HTTP token）。它们从 `FIT_MIDDLEWARE` 中移除；akit-config 为哈希依赖 akio-crypto。
- akio（规则 21）：I42 需要的先进入 akio，分支 `claude/a63-a65-auth`，提交 `5e0d433c`（ADR-109），基于 akio 的 main
  `afbbb521`：**A63** akio-crypto 中的 bcrypt；**A64** `Request::peer_certificates`，每个 HTTP/1.1、HTTP/2 和 HTTP/3
  请求上客户端的已验证证书链（`TlsInfo::peer_certificates`、`akio_tls::peer_chain`、
  `akio_quic::Connection::peer_certificates`）；**A65** `Context::suspend`、`Suspended::resume` 和
  `Context::suspend_to_pool`。附带测试，在 macOS 和 gpu51（epoll 与 io_uring）上运行。**尚未进入 akio 的 main，也未推送**：
  akit 的 `Cargo.toml` 仍锁定 `afbbb521`，本提交要等锁定移到合入后的 akio 提交才能构建（这里通过本地、未提交的
  `[patch]` 基于 `5e0d433c` 构建和测试）。
- `akit-middleware` 的 `auth`：
  - `basic_auth`：按 Go 的 `Request.BasicAuth` 读取 HTTP Basic 凭据；401 带
    `WWW-Authenticate: Basic realm="…", charset="UTF-8"` 和 fit 的响应体。尚未记住的密码在 akio 的逻辑线程池上校验：
    请求被挂起（`suspend_to_pool`），之后恢复进入下一个中间件和处理器，或回 401。未知用户的密码同样对第一个用户的
    哈希校验，使拒绝耗时相同。通过的密码按用户记为随机密钥下的 HMAC-SHA-256，无锁读取（四个字的 seqlock），以常数
    时间比较，因此该用户之后的请求就地放行。与 fit 的线程池一样：最多 1024 个校验等待或进行，等待超过 5 秒的回 503
    并带 `Retry-After: 1`，线程池满时也回 503。放行后请求去掉 `Authorization`（除非 `forward_authorization`），并在
    删除客户端发来的该字段的各种写法（`X_User`、`x_user` 等，同 fit 的 `fieldSet`：名字比较忽略大小写，`_` 视为 `-`）
    之后把 `user_header` 设为用户名。
  - 消息体流式交给处理器的请求不能挂起（ADR-109）：密码尚未记住时，`basic_auth` 的 `body_mode` 要求整体读取消息体
    （在监听器的 `max_body_bytes` 之内），否则沿用处理器的方式。在 `body_mode` 与 `call` 之间密码被遗忘（该用户存入了
    另一个密码）时回 503。
  - `client_cert`：没有已验证证书（未发送，或明文连接）时回 403 `Forbidden`，除非 `optional`；`subject_header` 设为
    叶证书的主题，写法同 Go 的 `pkix.Name.String`（最具体的 RDN 在前，多值 RDN 以 `+` 连接，Go 的转义，其他类型按
    OID），控制字符按 fit 的 `headerSafe` 替换；无论有无证书，都先删除客户端发来的该字段的各种写法。同一连接连续的
    请求只解析一次主题（按共享证书链做的每线程缓存）。
- 测试：凭据、字段写法、记住的 HMAC、校验数量上限、`headerSafe`（fit 的 `TestHeaderSafe`）、名字（与 Go 的输出核对）
  和 OID 的单元测试。`tests/auth.rs` 经由 akit 的监听器：fit 的 `TestBasic`（拒绝、记住密码、按用户、各种写法的伪造
  字段）、`TestBasicForwardAuthorization`、`TestBasicParams`、`TestClientCert` 以及 `TestAuthenticated` 中的情形
  （`verify_if_given` 的 TLS 监听器：已验证、无证书、可选、明文、前面有 `basic_auth`），h2 和 h3 上的证书；同一个
  HTTP/2 连接上一个慢校验进行时其他流照常作答（校验不在 I/O 线程上）；首次校验密码的请求带消息体经过代理。fit 的
  `TestBasicBusy` 由数量上限的单元测试和 akio 自己的 503 测试覆盖。
- 兼容性门禁：`tests/compat/supported.txt` 新增 12 个字段；`make compat-bless`：`examples/basic-auth`、
  `examples/client-cert` 和 `examples/mtls` 有效（80 个夹具：18 个有效，61 个只因暂不支持被拒绝，1 个跳过，没有失败）。
- 检查（基于 akio `5e0d433c`）：macOS 上的 `make check`（fmt、clippy、整个 workspace 的测试、锁定在 `afbbb521` 的锁文件
  上的 `cargo deny`、文档与兼容性清单）；gpu51 上在 bench 锁下，用 epoll 和 `AKIO_TEST_BACKEND=uring` 运行 akio-crypto、
  akio-tls、akio-quic、akio-http、akio-h3 以及 akit-config、akit-middleware、akit-compat、akit-runtime、akit-listener、
  akit-proxy 的测试：全部通过。
- 与 fit 的不同：访问日志行中尚无请求认证为谁（`user`）：在 akio A29 之前，akit 的日志行无法带上请求设置的内容；
  也尚未为缓存把请求标记为已认证（I67）。在 `body_mode` 与 `call` 之间被遗忘的密码回 503 而不是重新校验。
- 待测量（维护者的基准任务，规则 30）：见开发计划中 I42 的基准行。

### 2026-11-24：I32，ACME II：TLS-ALPN-01、续期、OCSP

- **TLS-ALPN-01**（RFC 8737），与 fit 相同：`acme.challenges` 接受 `tls-alpn-01`，默认两者都用，HTTP-01 在前，和 fit
  一样。挑战的证书用它自己的 P-256 密钥自签名，只为该名字签发，带关键的 acmeIdentifier 扩展，内容是密钥授权的
  SHA-256（`akio_x509::CertificateParams` 加自定义扩展）；挑战进行期间保存在内存中（`Acme::alpn_challenge`，一个
  `Published` 映射，无锁读取）。列出名字的 TLS 监听器在 ALPN 中最后提供 `acme-tls/1`，这样只有单独提供它的客户端会
  协商出它；这样的握手得到监听器所列名字的挑战证书，否则什么也得不到（`ListenerTls::get`）。先尝试的挑战随每次连续失败
  依次后移（fit 的 `attempt`），所以 TLS-ALPN-01 失败后接着用 HTTP-01。
- **续期**：按有效期，带抖动和退避，和 I31 一样；现在还按 **ARI**（RFC 9773，fit 的 `ari.go`，`acme/ari.rs`）：每次
  运行让某个名字有了证书、并开始提供后，向 CA 的 `renewalInfo` 询问它（ID 由颁发者密钥标识和序列号组成），在
  `Retry-After` 所说的时间再问（1 分钟到 24 小时，没说时 6 小时；测试用 `Acme::set_ari_bounds`）；证书在建议窗口内随机
  选一个时间到期，窗口已开始则立即到期；ARI 认为已到期的已存证书不再从存储中取用。`Acme::renew_at` 与
  `Acme::renewal_info` 给出时间。
- **OCSP stapling**（`akit_tls::Ocsp`，fit 的 `tlsmgr.OCSP`，`ocsp.rs`）：每个指定了响应者、证书链中带颁发者的证书按
  证书链的 SHA-256 保存，重载生成的各个运行时图共用，最后一个 `Staple` 释放时丢弃。控制面的两个工作线程获取响应
  （akio-http 的客户端，用它自己的引擎，10 秒，64 KiB，不跟随重定向），先用 akio-x509 的 `OcspResponse::verify` 校验再
  装订（`CertifiedKey::with_ocsp_response`）；在有效期过半时重新获取（没有 next update 时一小时），失败按 ACME 的方式
  退避，过了 next update 就不再装订；吊销或未知：不装订。握手从一个 `Published` 映射读取装订后的证书。ACME 获取的
  证书与其响应一起发布，任何一个都不会单独提供；响应者说已吊销的证书立即重新获取，不取用存储中的那个。管理器拥有装订器
  （`Acme::ocsp`），所以 `ListenerTls::load_with` 也通过它为监听器来自文件的证书装订，`Env` 和 `akit run` 都没有改。
- **来自文件的证书**：与 fit 相同，只有证书链能用受信任的根证书校验通过时才访问它们的响应者，只访问公网地址（除非根证书
  允许私有地址），只用 `http`，按校验过的地址连接，名字放在 `Host` 中。fit 默认的根证书是系统的，而 akio-tls 不在握手
  之外提供它们：**临时方案**，在 akio A57 之前，来自文件的证书只在给出根证书时装订（`Ocsp::trust_roots`），`akit run`
  没有给出，所以它为 ACME 的证书装订、不为文件中的证书装订（每个证书链说明一次原因）。
- akio 需求：**A57**（握手之外可用的系统根证书）和 **A58**（HTTP 服务能看到协商出的 ALPN，或由 akio 关闭协商出
  `acme-tls/1` 的连接）。在 A58 之前，协商出 `acme-tls/1` 后又发送 HTTP/1 请求的客户端会得到服务（fit 会关闭它）；
  握手只给了它挑战的自签名证书。
- 配置：`acme.challenges` 接受 `tls-alpn-01`；默认两者都用。兼容性门禁中 `acme.challenges` 的用例现在按给定顺序读取
  两者；没有新增字段，也没有新拒绝的字段。
- 测试：挑战证书的单元测试（SAN、关键扩展、摘要），fit 的 `TestCertsALPN`（带 acme 名字的 `ListenerTls`：`acme-tls/1`
  放在最后，只对单独提供它、且名字在列表中的握手给出挑战证书），吊销路径（已存证书不被加载、重新获取、内存中的那个仍在
  提供），ARI 的证书 ID（RFC 9773 的例子）、`Retry-After`、RFC 3339 与 HTTP 日期、窗口、公网地址和固定地址的响应者
  URL。`akit-tls/tests/ocsp.rs`（9 个），fit 的 `ocsp_test.go`，对接测试自己的响应者：装订、过半刷新、响应者宕机时
  保留、过期后丢弃、吊销被报告、证书链共用、最多同时两个获取且 `close` 迅速、受信任根证书下装订文件证书并归还、不受信任
  时不访问（只说一次）、除非允许否则不访问回环地址；以及在握手中为要求的客户端（`ocsp_stapling`）装订，不为没有响应的
  证书装订。`akit-tls/tests/ari.rs`（2 个），fit 的 `TestARIWindowBegun` 与 `TestARIWindowLater`，对接明文的 CA。
  `akit-listener/tests/acme.rs`（新增 4 个，Pebble，epoll 和 io_uring）：fit 的 `TestPebbleTLSALPN`（TLS 监听器在
  Pebble 的 TLS-ALPN-01 端口上，没有明文监听器；之后挑战证书不再保留；单独提供 `acme-tls/1` 的客户端随后被拒绝）、
  `TestChallengeFallback`、30 秒的证书被续期、提供给新的握手并保存，以及 `TestPebbleARI`（窗口在有效期内；移到过去后
  证书被续期）。Pebble 的测试辅助在它挑选的端口被并行测试占用时换端口重新启动（I31 的辅助代码就有的偶发失败）。
- 检查：macOS 上的 `make check`（运行了 Pebble 测试）；gpu51 上 akit-config、akit-tls、akit-runtime、akit-listener、
  akit 和 akit-compat 的测试，epoll 与 io_uring，设置了 `$AKIT_PEBBLE_REQUIRED`，全部通过。
- 与计划和 fit 的不同：
  - 在 akio A57 之前，不用系统根证书为文件中的证书装订（见上）。
  - 在 akio A58 之前，`acme-tls/1` 连接在握手后不被关闭（见上）。
  - fit 的 OCSP 获取走环境变量指定的代理；akit 的不走（不读取 `HTTP_PROXY` 等），来自文件的证书的响应者必须是
    `http`。
  - Pebble 为短期证书建议（ARI）的窗口覆盖整个有效期，所以续期测试中的证书立即由 ARI 续期；仅按有效期续期由
    `renew_at` 的单元测试覆盖，和 I31 一样。
  - ARI 在证书开始提供之后才询问（fit 在之前询问），所以 CA 不可达时不会延迟来自存储的证书。
  - 没有测试 TLS 1.2 客户端的装订（akio 的客户端无法限定在 1.2；akio 自己测试了 1.2 的装订）。
- 基准任务应测量：与 I31 对比每秒完整 TLS 握手数，有和没有装订的响应（证书选择现在要读 ALPN 列表，对带响应者的文件
  证书还要读第二个 `Published` 映射），以及带响应时握手的大小。

### 2026-11-24：I18，HTTP/2 与 h2c 上游

- `upstreams[].protocol` 接受 `h2c` 和 `h2`（`akit_config::UpstreamProtocol`）；`h2` 使用系统的根证书并通过 ALPN
  提供 `h2`（它的 `tls_*` 字段属于 I54；`Pool::with_tls` 可给连接池另一份 TLS 配置，测试就是这样做的）。
- `akit_upstream::h2`：每个地址的 h2 连接，基于 akio 的 `H2Client`。请求在第一个有空闲流的连接上取一个流，没有时
  拨一个新连接；等待正在拨号的连接的请求在它打开后拿到它，或拿到它的错误。每个连接最多 `STREAMS_PER_CONN`（100）
  个流，`max_conns` 通过一个地址上进行中的流数（`max_conns` × 100）限制连接数；`max_pending`、负载均衡（最少连接
  按流计数）、被动和主动健康检查、`response_timeout`、指标以及关闭时的排空都与 HTTP/1.1 相同。承载完最后一个流的
  连接留给下一个请求，最多 `max_idle_conns` 个，各保留 `idle_timeout`。
- h2 或 h2c 上游的主动健康检查经 HTTP/2 探测，与 fit 按上游协议探测一致：使用自己的连接，收到响应头后即关闭。
- `Exchange` 的 body 现在是 akit_upstream 自己的类型，两种协议相同：`Upload`（HTTP/1 的 `RequestBody` 或 h2 流的
  body）和 `Head`；`Download` 可持有两者的响应。代理（`relay.rs`）除类型外没有变化：发往 h2c 上游的请求以其 `Host`
  作为 HTTP/2 的 `:authority`，字段、body 和 trailer 与 HTTP/1.1 相同。
- 针对 akio 的 h2 客户端所缺能力的权宜之计，每项都有界并列在模块文档中，直到 akio API 请求 A51–A53（I18b）：放弃的流
  （客户端离开、超时、客户端太慢）任其运行并丢弃其事件，其连接不再接收新流，并在其他流结束后关闭（A51：无法重置）；
  h2 上的 `response_timeout` 由 akit 定时线程上的任务每 100 ms 检查一次，每个有连接的地址一个任务，它也关闭超过
  `idle_timeout` 的空闲连接（A51：`open` 没有超时）；暂停的 h2 下载仍会继续到达，再超过 `HELD_LIMIT`（4 MiB）后被切断
  （A52：流窗口被立即归还）；经 h2 的上传不控制节奏，akio 队列中等待的数据受请求体上限约束（A52）；发往 h2 上游的请求
  trailer 被丢弃（A51）；每个连接 100 个流，因为服务器的 `SETTINGS_MAX_CONCURRENT_STREAMS` 不会被报告（A53）；拨号受
  引擎连接超时约束，且 `h2` 不回退到 HTTP/1.1（A53）。某个流以 closed 失败（`GOAWAY` 之后）的连接不再接收新流。
- 测试：`akit-upstream` 的 `tests/h2.rs`（经 h2c 的请求，多个流共享一个连接、超过 100 个时用第二个连接，`max_conns`
  和 `max_pending`，拒绝连接的地址，`idle_timeout` 和 `max_idle_conns: 0` 关闭空闲连接，流式交换，其超时放弃流并让
  连接停收新流，整体请求的超时，连接池排空，经 h2c 的探测让地址下线再恢复）；`akit-proxy` 的 `tests/h2.rs`，fit 的 `eachProto` 测试移植到 h2c 上游，
  客户端为 HTTP/1.1 和 h2c（`TestForwarding`、`TestHopByHopRequest`、`TestForwardedUntrusted` 和 `Trusted`、
  `TestHostHeader`、`TestManyRequests`、`TestHead`、`TestPassiveHealth`、`TestStreamDownload`、`TestStreamUpload`、
  响应方向的 `TestTrailers`、`TestUpstreamBreaksMidBody`、`TestClientAbort`），代理的 `timeout`，暂停下载的权宜之计，
  以及一次经 TLS 的 h2 交换；`request_trailers` 被忽略，注明 A51。`TestResponseHeaderTimeout` 和
  `TestResponseIdleTimeout` 仍属于 I17b（A36）。
- 兼容性门禁：`examples/grpc` 不再拒绝 `upstreams[].protocol`（被拒绝的字段从 66 个变为 65 个；它仍拒绝
  `response_idle_timeout`）。
- 未测量（规则 30）：代理到 h2c 上游与到 HTTP/1.1 上游的对比，以及租用流时的按地址锁（见开发计划的基准表）。

### 2026-11-24：I36，负载均衡 v2

- 配置：`upstreams[].load_balancing` 接受 fit 的另外六种策略（`random`、`p2c`、`ip_hash`、`uri_hash`、
  `header_hash`、`cookie_hash`；`LoadBalancing::{Random, P2c, IpHash, UriHash, HeaderHash, CookieHash}`、
  `is_hash`），以及 `hash_key`（`Upstream::hash_key`）和 `sticky_cookie`（`akit_config::StickyCookie`：`name`，
  默认 `fit_sticky`（`DEFAULT_STICKY_COOKIE_NAME`），`path`，默认 `/`，`max_age`，`secure`），带 fit 的检查和
  消息：`header_hash` 和 `cookie_hash` 需要一个合法 token 的 `hash_key`，其他策略不接受它；cookie 名必须是
  token，路径以 `/` 开头且不含 `;`。
- `akit_upstream::balance`，即 fit 的 `upstream/hash.go`：**random** 在在线的地址中按权重随机抽取（全部下线时
  在全部地址中）；**p2c** 抽取两个不同的地址，取按权重计进行中请求（busy 加 waiting，与 `least_conn` 读的一样）
  较少的那个，下线的选择从在线的地址中重抽。每个线程用自己的 wyrand 生成器（用 std 的随机哈希密钥做种子），
  抽取不写任何共享数据（规则 13）。
- **一致性哈希**：每个地址按每单位权重 160 个点放在环上，位置是 fit 的哈希（FNV-1a 加 MurmurHash3 的收尾）
  `address#i`，按哈希再按地址排序，在编译配置时构建；键落到哈希处或之后的第一个点，该点地址下线时往后找第一个
  地址在线的点（全部下线时不遍历整个环）。键：`ip_hash` 用客户端 IP（IPv4 映射的 IPv6 地址按 IPv4），按文本
  哈希而不格式化到缓冲区；`uri_hash` 用路由匹配到的路径和 query；`header_hash` 用该头的第一个值；
  `cookie_hash` 用该 cookie，按 fit 的 `cookieValue` 读取。没有键的请求按轮询。`Balancer::keyed`、
  `Balancer::key` 和 `Balancer::pick_keyed` 接收地址、哈希请求的 `Keys` 并按哈希选择。
- **会话保持 cookie**（`akit_upstream::sticky`）：cookie 指向一个在线地址的请求去那里；其他请求的响应带上指向
  它所去地址的 `Set-Cookie`（在上游自己的之后），值为 fit 的 `StickyID`（地址 SHA-256 的前 64 位的十六进制，
  基于 akio-crypto），因此地址相同的 akit 和 fit 一致。排好序供二分查找的 id 以及每个地址完整的 `Set-Cookie`
  值都在编译时生成（规则 12）。
- 连接池：`Pool::pick_for(&Keys)`（先会话保持 cookie，再均衡器）、`Pool::set_cookie`、`Pool::is_sticky`；
  `Pool::pick` 含义不变（没有请求：哈希策略按轮询）。代理用请求的对端地址、规范化路径、query 和头字段（中间件
  修改后的）选择地址，并把要加的 `Set-Cookie` 交给 relay，只在需要设置时才复制。
- 测试：`akit-upstream/tests/hash.rs`（13 个），移植 fit 的 `TestRandom`、`TestP2C`、`TestP2COneUp`、
  `TestPickSkipsDownNewPolicies`、`TestWeightedRandomAllDown`、`TestHashDistribution`、`TestHashStable`、
  `TestHashKeys`、`TestHashRingEdges`、`TestSticky`（在一个连接池上，一个地址因失败下线）和
  `TestStickyNilRequest`；环的大小有界；以及 fit 自己的 `upstream` 包对相同键的选择（四个带权重地址上的 ip、uri
  和头哈希，以及 `StickyID`），akit 的结果完全一致。`akit-upstream/tests/no_alloc.rs`，即 fit 的
  `TestNewPoliciesAllocs`：九种策略的选择在有键、有地址下线时都不分配内存，连接池的会话保持选择和
  `set_cookie` 也不分配（用 `stats_alloc` 计数，开发依赖，与 akit-router 一样）。`akit-proxy/tests/balance.rs`
  （3 个），移植 fit 的 `TestStickyCookie`（整体发送和流式发送的响应）和 `TestHeaderHash`，以及端到端的 uri
  和 ip 哈希。`akit-config/tests/upstreams.rs`：各策略、fit 的 `TestHashedPolicyConfig`、cookie 的默认值和取值。
  门禁为六个新路径增加了样例。
- 已检查：macOS 上 `make check`；Linux（基准测试主机）上 akit-config、akit-upstream、akit-proxy 和
  akit-compat 的测试，epoll 和 io_uring。门禁（`make compat-bless`）：`examples/l4-udp` 不再拒绝 `ip_hash`，
  `examples/load-balancing` 不再拒绝 `sticky_cookie`（被拒字段 66 到 64）；它们仍拒绝其他字段。没有运行
  基准测试（规则 30）；需要测量的内容在开发计划的状态表中。
- 与 fit 的差异：
  - 环最多 `MAX_RING_POINTS`（2^20）个点：权重总和超过 6,553 时，每单位权重少于 160 个点，键分布稍不均匀；
    fit 的环没有上限。低于这个值时，键落点与 fit 完全一致。
  - `uri_hash` 哈希路由匹配到的规范化路径（按发送的形式，百分号编码），fit 哈希 Go 解码后的 `URL.Path`；路径
    含转义的键落点可能与 fit 不同。
  - 随机抽取来自每线程的 wyrand 生成器，而不是 Go 的 `math/rand/v2`；份额相同。

### 2026-11-24：I28，运行时的监听差量

- I22 已在 akio 的运行时监听器（A1）上移动监听器：新的和改变的在替换前添加（地址被占用时重载失败且什么也不变），移除的和
  改变的在替换后按 `-shutdown-timeout` 排空，Linux 上同一地址的替换者加在旧监听器旁边，Admin API 的修改（I26、I27）和
  `-watch-certs` 也走同一个 `Listeners::reload`。I28 让“不变”与 fit 一致：`crates/akit-listener/src/reload.rs` 的
  `same` 就是 fit 的 `binding.same`，比较名字、地址、协议、`limits.max_body_bytes`、是否 TLS、`tls.max_version` 和
  `client_auth.mode`，`ListenerTls::same_config` 比较 ALPN、h3 和客户端 CA 的内容。证书文件或 `tls.acme` 名字改变的
  TLS 监听器会被保留，连同其套接字和连接，之后的握手从其管理器拿到新证书（I30、I31），以前则会重新打开；CA 换到内容相同的
  另一个文件也保留。被保留的监听器此后按新配置运行，下一次重载与它比较。
- 测试，来自 fit 的 `listener/reload_test.go`：`akit-listener/tests/reload.rs` 中，到被保留监听器的 keep-alive 连接跨越
  多次重载保持打开，每次由新图应答（fit 的 `TestReload`）；`listeners_added_and_removed_under_traffic_fail_no_request`，
  即 I28 的测试：四个客户端通过保持的和新建的连接访问被保留的监听器，同时十次重载每次增加一个监听器再移除，路由每次都变，
  没有失败的请求，它们看到的版本只增不减。`tests/certs.rs` 中的 `other_certificate_files_keep_the_listener`：另外的
  证书文件、来自另一个文件的相同 CA 保留监听器（下一次握手拿到新证书，之前建立的连接继续服务），改变 `client_auth.mode`
  或去掉 TLS 则重新打开。fit 的 `TestReloadFails`、`TestReloadSameAddress` 和 `TestPlan` 已随 I22 和 I27 移植。
- akio：A22（在运行中的引擎上移除 UDP 监听器）仍不在 akio 的 main 上，所以移除或改变 akit 启动时服务 h3 的监听器的重载
  仍被拒绝。新提出 A54：排空时 akio-http 立即关闭空闲的 HTTP/1 连接，客户端此刻在上面发出的请求会遇到关闭；排空时给一个
  宽限期就能以 `Connection: close` 应答它。在此之前，端到端测试的客户端像以前一样重发这种请求。
- 检查：macOS 上的 `make check`；Linux（基准测试主机）上 epoll 和 io_uring 的 `akit-listener`、`akit-runtime` 和
  `akit` 测试。没有基准测试：热路径没有变（规则 30）。
- 遗留：服务 h3 的启动监听器（A22）；被排空监听器的 keep-alive 关闭竞争（A54）。

### 2026-11-24：I31，ACME I：账户、订单、HTTP-01

- `akit_tls::Acme`，即 fit 的 `tlsmgr.ACME`（fit 决策 007），配一个 akit 自己的 ACME 客户端（RFC 8555）：基于
  akio-crypto 的 JWS（`acme/jws.rs`：P-256 账户密钥、它的 JWK 和 RFC 7638 指纹、扁平序列化的 ES256），基于
  akio-x509 的 CSR（`CsrParams`，akio A6），以及 CA 协议（`acme/protocol.rs`：目录、有上限的 nonce 池、`badNonce`
  时换新 nonce 重试、problem 文档、同意条款的 `newAccount`、`newOrder`、授权、`Retry-After`、finalize、PEM 格式的证书）。
- 请求经由 akio-http 的异步客户端，运行在管理器自己的引擎上（一个客户端线程，第一次需要 CA 时创建）：回调只把响应交给
  一个通道，工作线程以 50 ms 为片等待，因此 `close` 能立即结束进行中的工作。响应上限 1 MiB，每个请求受
  `acme.request_timeout`（30 s）限制，每个订单受 `acme.timeout`（10 分钟）限制；查看订单或授权之间的等待遵循
  `Retry-After`，限定在 1 s 到 30 s 之间。
- 管理器：`manage(names)`（名字转为小写、去掉末尾的点，最多 `MAX_NAMES` 个），第一个名字被管理时启动
  `acme.max_concurrent` 个工作线程（2），每个取最先到期的名字。一个名字只获取一次，保存在存储中
  （`certificates/<ca>/<name>.pem`，证书链和私钥在同一个 PEM 中；账户密钥在 `acme/<ca>/account.pem`；`<ca>` 是 fit 对
  目录 URL 的 `caKey`），并一直从存储中使用，直到需要续期：到期前剩三分之一有效期时，按序列号最多再提前五十分之一
  （fit 的 `defaultRenewAt`）。共用存储的实例会锁住 `issue/<ca>/<name>` 和 `account/<ca>`，拿到锁后再查一次。失败后
  一分钟重试，每次翻倍，最多一小时，并随机减去最多十分之一（`set_backoff` 可改上下限），并报告（`last_error` 和日志）。
  重载去掉的名字不再提供证书；它们的证书留在存储中。
- I/O 线程读取的内容都不加锁（规则 1 和 13）：证书和 HTTP-01 应答各是一个 `published::Published` 映射，由工作线程
  整体替换，并像运行时图单元一样按 epoch 在每个线程缓存。
- `tls.acme`：`ListenerTls::load_with(tls, protocols, acme)` 和 `ListenerTls::get`，即 fit 的 `tlsmgr.Certs`：请求所列
  名字的握手，一旦有证书就得到它（不带名字的客户端，在没有文件证书的监听器上，得到第一个名字的证书）；否则得到文件中的
  证书。`tls` 可以只列名字而没有证书。`ListenerCerts`（I30）现在从当前图读取监听器的 `ListenerTls`。
- 每个明文监听器都应答 HTTP-01：分发器在路由之前，对请求 `/.well-known/acme-challenge/<token>`（规范化路径）且该挑战
  正在进行的请求，以其 key authorization 应答 `200 text/plain`；其他 token 照常走路由，与 fit 相同。
- `akit run` 用启动时的 `acme` 创建一个管理器（`Env::with_acme`）；每次应用配置（启动、SIGHUP、Admin API、
  `-watch-certs`）后，在图成为当前图时让它管理监听器所列的名字（`Graph::acme_names`）；`acme` 改变时警告需要重启；
  关闭时关闭管理器。`akit validate` 不获取证书。
- 配置：`acme`（`directory`、`email`、`ca_file`、`storage`、`max_concurrent`、`timeout`、`request_timeout`、
  `challenges`）和 `listeners[].tls.acme`，带 fit 的检查（目录是 http 或 https URL；挑战已知且不重复；名字是合法主机名、
  不是通配符、不是 IP 地址、不重复；要求客户端证书的监听器需要 HTTP-01）。`challenges` 接受 `http-01`；`tls-alpn-01`
  （I32）、`shared_storage` 和 `on_demand`（I34）作为暂不支持被拒绝。兼容性门禁多了十个字段：12 个有效夹具（现在包括
  `http3`），67 个只因暂不支持被拒绝。
- akio：A6 的 CSR（分支 `claude/x509-csr`）合并到 akio 的 main（`60547435`）上，得到 akio 分支 `claude/i31-acme-csr`
  上的 `a49aaae3`，其 ADR 改号为 107（main 上 093 已被占用）；akit 锁定到它。**尚未推送**：构建时需从包含该提交的
  akio 检出获取（或先把它推送到 akio 的 main）。
- 测试：JWS（签名后用公钥验证；RFC 7638 指纹）、problem 文档、`caKey`、续期时间、退避和 `Published`（跨线程读取、
  嵌套读取）的单元测试。`akit-config/tests/acme.rs`（6 个）：默认值和取值、拒绝的和暂不支持的、只能由一个文件给出、
  有无证书的 `tls.acme`、fit 对名字的检查。`akit-listener/tests/acme.rs`（2 个，epoll 和 io_uring），像 fit 的
  `internal/pebbletest` 那样运行 Pebble v2.10.1（`support/pebble.rs`；没有其二进制时跳过，用 `$AKIT_PEBBLE_BIN` 指定
  位置）：一个在 Pebble HTTP-01 端口上的明文监听器和一个带 `tls.acme`、没有文件证书的 TLS 监听器；获取前没有证书，之后
  获取到证书并在握手中提供，按 Pebble 的根证书验证（名字大小写不同也可）；挑战结束后该路径回到路由；在同一存储上的新
  管理器在 Pebble 停止后提供同一证书；去掉的名字不再提供证书。还有 CA 不可达：报告失败、再次尝试、`close` 立即返回。
- 已检查：macOS 上的 `make check`（设置 `$AKIT_PEBBLE_BIN`，Pebble 测试会运行）；gpu51 上 akit-config、akit-tls、
  akit-runtime、akit-listener、akit 和 akit-compat 的测试，epoll 和 io_uring，设置 `$AKIT_PEBBLE_REQUIRED`，全部通过。
- 与计划和 fit 的不同：
  - TLS-ALPN-01、OCSP stapling 和 ARI 属于 I32；按需 TLS 属于 I34；共享存储属于 I62。在 I32 之前，fit 默认的
    `challenges`（先 HTTP-01 后 TLS-ALPN-01）只有 HTTP-01。
  - 默认存储是 `$XDG_DATA_HOME/akit` 或 `~/.local/share/akit`（fit 是 `.../fit`）。
  - 管理器的线程是 akit 自己的（控制面），不是 akio 的逻辑线程池：它们会为 CA 等待数秒，也会等待存储的锁。
  - ACME 的证书还没有列入指标的证书列表（I37 列出的是文件中的证书）。
- 基准任务应测量的：分发器在明文监听器上的 HTTP-01 检查（`akit run` 有管理器时每个请求一次前缀比较）：HTTP/1.1 和
  h2c 上的 fixed，与 I31 之前的 main 做同轮 A/B。

### 2026-11-24：I27，Admin API II：局部修改、回滚、认证

- 配置：读取 `admin.token_file`（`Admin::token_file`；空串即没有）。非回环地址仍被拒绝：fit 在那里还要求 TLS
  （`cert_file`、`key_file`），而 akit 还不能通过 TLS 提供 API；错误信息如是说明。
- `akit-admin`：`PUT`、`PATCH`（`application/merge-patch+json` 或 `application/json`）和
  `DELETE /config[/<pointer>]`，`POST /rollback/{n}` 和 `POST /reload`，与 fit 一样：设置、新增或追加（`-`）一个值，
  按 RFC 7396 合并，删除；配置本身不能删除；整份放入的配置按写入时的样子保存，编辑过的配置按 fit 的写法重新写出
  （`Value::to_json`）。请求体与加载一样在 `max_config_bytes` 之内流式读入（413、415）；指针和 `?dry_run` 在 I/O
  线程上检查，修改作为 `Job`（现在是一个 `Op`：load、put、patch、delete、rollback、reload）交给控制线程。
  `Configs::change` 在为应用排序的锁下、比较过 `If-Match` 之后（412 时什么都不调用）从正在运行的配置做出修改，
  并发写入不会丢失；`Configs::update` 就是数据固定的它。`Refused::Request` 带指针的 400/404 和
  “version n is not kept”；回滚的来源是 `SOURCE_ROLLBACK`（`rollback`），重载的来源是 `admin`，与 fit 一样。
  `pointer.rs` 增加 `put`、`remove`、`merge_patch` 和 `lookup_mut`，错误信息与 fit 相同。
- 试运行（`POST /load`、`PUT`、`PATCH`、`DELETE` 和 `POST /reload` 上的 `?dry_run=1` 或 `true`；除 `0`、`false`
  或不带之外的值回 400）：`Mode::DryRun` 走应用的路径但不产生效果；回答带 `dry_run`、正在运行的版本和 ETag、
  `new_etag`、`unchanged`、应用时会增加和移除的监听器及其提示，以及 `Fit-Dry-Run: 1`（`DRY_RUN_HEADER`）；被拒绝的
  试运行与应用的回答逐字相同。新增的 `akit_listener::Listeners::plan` 说明 `reload` 会打开和关闭什么而不改变任何
  东西：解析新监听器的地址，除非正在运行的监听器占着它，否则短暂绑定一下；`akit run` 的 `Runner::check` 用空的
  引擎槽和自己的注册表编译后调用它，不计入重载。
- 令牌：API 启动时读取（去掉首尾空白；为空或读不到则启动失败）；此后每个请求都要带
  `Authorization: Bearer <token>`，以常数时间比较，否则回 401 并带 `WWW-Authenticate`；有令牌时不再检查回环
  `Host`，`Origin` 仍然拒绝，与 fit 的 guard 一样。
- 测试：`akit-admin/tests/admin.rs`（21 个，新增 11 个），移植了 fit 的 `edit_test.go`、`dryrun_test.go` 和
  `TestToken`：按指针编辑并得到 fit 预期的配置；被拒绝的编辑（无效地址带 fit 的行列号、不存在的模块、未知字段、
  不是 JSON、404 的指针、415、405、根不能删除）；ETag 过期时各种修改都被拒绝；16 个并发写入者（8 个带 `If-Match`，
  遇 412 重试，8 个不带、只追加）各被应用一次；回滚（作为新版本、来源 `rollback`、已不保留的版本、`x`）；重载；
  试运行（回答、头、与应用相同的拒绝、被占用的地址、错误的 `dry_run`，然后用试运行给出的 ETag 应用）；移除监听器
  的试运行不释放任何东西；令牌；编辑请求体低于、等于和超过限制，以及中途断开。单元测试：编辑树（fit 的
  `TestEditTree` 和 `TestPointer`）、`Configs::change` 和令牌比较。`akit-listener/tests/reload.rs`：
  `Listeners::plan`。`akit-config/tests/admin.rs`：`token_file`。`akit/tests/admin.rs`：`akit run` 端到端地带令牌
  做一次编辑、一次试运行和一次回滚。门禁中 `admin.token_file` 的样例。
- 已检查：macOS 上 `make check`；Linux（基准测试主机）上 akit-admin、akit-config、akit-listener、akit 和
  akit-compat 的测试，epoll 和 io_uring，全部通过（更早的一次 epoll 运行碰到 `akit-listener` 已知的偶发失败 `unchanged_listeners_are_kept_and_new_ones_opened_and_closed`）。
- 与 fit 的差异：
  - API 没有 TLS（`cert_file`、`key_file`、`client_ca_file`），因此无论有没有令牌都不能用非回环地址；还没有 Unix
    套接字（A23）。
  - 401 之后不像 fit 那样关闭连接（akio API 需求 A24，与 413 一样）。
  - 没有移植 fit 的连接限制器（`max_conns` 的四分之一留给出示过令牌的客户端，未认证连接 5 秒后关闭，每个对端一份
    配额）：限制仍是引擎的 `max_connections`。
  - 编辑的请求体用 akit 的 JSON 读取器解码，它允许注释和尾随逗号，而 fit 的 `encoding/json` 拒绝它们。
  - akit-config 接受没有监听器的配置，fit 拒绝：`TestDryRun` 中第一个被拒绝的用例没有移植。
  - fit 的 `/config/text`、`/node`、store 和 console 不在 akit 中。
- 基准测试：无（规则 24：控制面）。

### 2026-11-24：I17，反向代理 III：超时、取消、trailer

- 配置：读取上游的 `response_timeout`（`Upstream::response_timeout`，省略或为 0 时取 fit 的 1h，即
  `akit_config::DEFAULT_RESPONSE_TIMEOUT`）和代理的 `timeout`（`Proxy::timeout`，省略或为 0 时没有）；
  `response_header_timeout` 和 `response_idle_timeout` 仍以暂不支持拒绝（A36）。
- `akit-upstream`：连接池用上游的 `response_timeout` 限制每次交换（`Pool::response_timeout`）；请求自己的超时也
  起作用，取两者中较短的（以前是替换连接池的）。`Exchange::gave_up`：在响应之前因客户端离开而放弃请求的交换，不计为
  该地址的失败。
- `akit-proxy`，与 fit 的 `exchange` 相同：
  - 超时：超时的连接（`ClientError::Io`，类型为 `TimedOut`，akio 以此报告 `connect_timeout`）回答 504，请求超时
    原本就是如此。代理的 `timeout` 设到每个请求上，连接池取它和 `response_timeout` 中较短的：在响应头之前超过则 504；
    之后超过则截断响应。
  - 截断（因超时或上游中途断开）：关闭 HTTP/1 客户端的连接，重置声明了长度的 HTTP/2 或 HTTP/3 流，未声明长度的则以
    fit 的 `Fit-Error: upstream failed mid-body` 尾部字段结束（`relay::BROKEN_TRAILER`）。
  - 取消：下载中途离开的客户端原本就能察觉（下一次写失败；被挂起的下载立即得知，因为 akio 在关闭时调用
    `on_drain`）。经 HTTP/2 或 HTTP/3 在上传中途离开的客户端会丢弃其请求体接收器（`Sink` 在 `End` 之前的
    `Drop`），从而丢弃上游请求未完成的请求体（关闭其连接），交换声明是自己放弃的，所以不计为该地址的失败。
  - trailer，与 fit 相同：请求声明的名字（`headers::trailer_names`，即 fit 的 `trailerNames`）放在它的
    `Trailer` 里发往上游，之后只跟随这些 trailer（`headers::announced`）；声明了 trailer 的请求按 chunked 发往
    上游，请求体整体到达的也是（经 `Client::preload` 作为流写出），没有声明的则不发送任何 trailer。流式响应保留上游的
    `Trailer` 声明；整体发送的响应（64 KiB 以内）若以 trailer 结束，则带着声明改为流式发送，使 HTTP/1.1 也能携带
    （akio 只在 HTTP/2 上发送整体响应的 trailer）。
- 测试：`akit-proxy/tests/failure.rs`，移植自 fit 的 `failure_test.go`：`connect_timeout`（backlog 已满的监听器；
  在满 backlog 会拒绝连接的系统上跳过，如 macOS）、`timeout_before_head`（`response_timeout`、`timeout` 以及两者
  中较短的：超过后 504，上游的连接被关闭）、`cut_short_mid_body`（用 `response_timeout` 代替空闲超时的
  `TestResponseIdleTimeout`，以及经 h2c 的 `TestUpstreamBreaksMidBody`：HTTP/1.1 的 body 不完整地结束，h2c 上
  有长度的失败，没有长度的以 `Fit-Error` 结束）、`client_abort`（经 HTTP/1.1 和 h2c 下载中途、经 h2c 上传中途：
  上游看到连接关闭，其失败计数保持为 0）、`trailers`（`TestTrailers`：声明的请求 trailer 发往上游，未声明的被丢弃，
  响应 trailer 经 HTTP/1.1 和 h2c 返回，小响应和流式响应都有）。`trailer_names` 和 `announced` 的单元测试，I/O
  超时的 `status_of`，配置字段及其错误，连接池的 `response_timeout` 与请求自身超时的关系，`forwarding` 检查没有
  body 的 GET 发往上游时不带分帧。兼容性门禁的样例覆盖这两个字段。
- 验证：macOS 上的 `make check`；Linux（gpu51）上 epoll 和 `AKIO_TEST_BACKEND=uring` 下 `akit-proxy`、
  `akit-upstream` 和 `akit-config` 的测试。
- 与计划和 fit 的差异（I17b，两个计划文件中都有）：
  - fit 的 `response_header_timeout` 和 `response_idle_timeout` 需要 akio 客户端按请求的响应头超时和空闲超时
    （akio API 请求 A36）：`TestResponseHeaderTimeout` 未移植，`TestResponseIdleTimeout` 以 `response_timeout`
    代替移植。
  - 在响应头之前（请求体已全部发出）离开的客户端，以及上传中途离开的 HTTP/1 客户端，要到上游回答或
    `response_timeout` 到期才会被察觉：akio 的客户端在响应头之前没有可取消请求的句柄（A37），分离的服务端请求得不到
    客户端离开的消息，且代理挂起的应答握着连接时，akio 的 HTTP/1 会话会保留流式请求的请求体接收器（A38，由
    `client_abort` 发现）。`TestClientAbort` 的“响应头之前”情形和它的 HTTP/1 上传未移植。
  - fit 的 `limits.write_stall_timeout` 仍被拒绝：akio 引擎的 `write_timeout` 是同一个界限，但重载时不能修改
    （A38）；在此之前，停止读取的客户端由 akio 的默认值（60s）限制。
  - 请求的 trailer 只经 HTTP/1.1 测试：akio 的 h2 客户端不发送 trailer。
- 给基准测试任务：热路径上没有变化，只是每个被代理的请求多一次 `Trailer` 查找、共享状态多一个字段；
  [基准表](dev-plan.zh-CN.md#基准与优化状态)中 proxy 场景那一行写明了带 trailer 的请求。

### 2026-11-24：I41，限流、并发与大小限制

- 在 I40 之后提前从 M3 中做。`akit-config` 把 fit 的 `rate_limit`、`concurrency_limit` 和 `body_limit` 中间件
  （`middleware/limit`）读成 `Handler::RateLimit`、`ConcurrencyLimit` 和 `BodyLimit`，带 fit 的 `Provision` 的检查和
  消息，以及 fit 的默认值（burst：速率向上取整；key 为 `ip`；`ipv6_prefix` 64；`max_keys` 100000；状态码 429）；新增
  读取 JSON 数字的解码器，用于 `rate`。它们从 `FIT_MIDDLEWARE` 中移除。
- `akit-middleware` 的 `limit`：
  - `rate_limit`：每个键一个令牌桶，以每秒 `rate` 的速度补充，最多 `burst` 个，与 fit 一样以纳令牌计；没有令牌的请求
    以 `status` 回答，带 `Retry-After`（整秒，向上取整）。键：客户端地址（IPv6 地址截到 `ipv6_prefix` 位；在
    `trusted_proxies` 之后时，取 `X-Forwarded-For` 中最后一个不是可信代理的地址，按 fit 的 `clientIP` 遍历且不分配
    内存）、某个头的第一个值（按带密钥的哈希）或路由。令牌桶与 fit 一样：按键的哈希分为 64 个分片，每个分片一个 map 和
    一把自己的锁，由各 I/O 线程共享（按线程分片会让连接分布在多个线程上的客户端得到成倍的速率），最多 `max_keys` 个键，
    超过时新键淘汰所查看的 8 个中桶已满的一个，否则淘汰其中第一个。查找不分配内存：键是地址或 64 位随机密钥的 SipHash
    （fit 保留 128 字节以内的值原样、更长的取哈希），分片的 map 直接使用这个哈希。
  - `concurrency_limit`：路由进行中请求的一个原子计数；超过 `max` 的请求立即以 `status` 回答。请求的名额是响应过滤器
    中的一个守卫：响应发出时归还，或请求没有响应就结束时归还（过滤器随请求或其 `Detached` 一起被丢弃）。
  - `body_limit`：声明的请求体超过 `max_bytes` 的请求在处理器运行之前回 413（fit 的 `Request Entity Too Large`），
    其请求体按流读取，一点也不缓冲；缓冲的请求体超过时，在到齐后同样处理；没有声明长度、流式交给处理器的请求体，
    把监听器的 `MAX_BODY_BYTES` 本地值调低到该限制，由代理据此限制（超过时 413）。
- 临时方案，直到 akio 支持（规则 21；见 [akio API 需求](akio-api-requests.zh-CN.md)）：
  - A48：akio 既不告诉服务响应已完整发出，也不告诉它请求等待期间客户端已离开。因此流式响应（代理超过 64 KiB 的响应）
    在响应头发出时归还并发名额，客户端已离开的请求一直占着名额，直到处理器放开请求（代理的上游回答或失败）。响应过滤器
    是每个请求一个装箱闭包加一次 `Arc` 克隆。
  - A49：中间件无法限制它自己不读的流式请求体。流式读取请求体的处理器按 `MAX_BODY_BYTES` 限制（代理如此；也只有它）；
    413 不关闭 HTTP/1 连接，akio 会读完并丢弃剩下的请求体（A24）。
- 测试：`akit-middleware` 在测试控制的时钟上的单元测试：移植 fit 的 `TestRateBucket`、`TestRateSlow`、`TestRateKeys`、
  `TestClientIP`、`TestRateMemoryBounded`、`TestEvictPrefersFull`、`TestRateIPv6Prefix` 和 `TestPeerAndName`，以及
  速率、burst 和时钟的极值（规则 15）。`tests/limits.rs` 通过 akit 的监听器：一次突发后 429 并带 `Retry-After: 100`；
  按头分键并用 503；fit 的 `TestConcurrency`（立即回答的请求归还名额；到达限制后再来一个被拒绝；稍后回答的或未回答就
  放开的请求腾出名额）；akit 版本的 fit `TestConcurrencyClientGone`；fit 的 `TestBody` 和 `TestBodyOverNetwork`
  （低于、等于和超过限制，按长度和 chunked，发给整体读取请求体的处理器和流式转发的代理）；fit 的 `TestParamErrors`。
- 兼容性门禁：`tests/compat/supported.txt` 中的 16 个字段；`make compat-bless`：`examples/rate-limit` 和
  `examples/body-limit` 有效（80 个夹具：13 个有效，66 个只因暂不支持被拒绝，1 个跳过，没有失败）。
- 已检查：macOS 上 `make check`；基准机上，带 epoll 和 `AKIO_TEST_BACKEND=uring` 运行受影响 crate 的测试
  （`cargo test -p akit-middleware -p akit-config -p akit-compat`）。
- 与 fit 的不同：用作键的头值不论长短都按 64 位带密钥哈希保存；声明长度还超过监听器 `max_body_bytes` 的请求体由监听器
  先回答（akio 的 `Payload Too Large`），中间件看不到。
- 待测量，由维护者的基准任务完成（规则 30）：见开发计划中 I41 的基准行。

### 2026-11-24：I35，主动健康检查

- 配置：读取 `upstreams[].health_check` 及其十个字段（`type`、`path`、`host`、`port`、`expect_status`、`interval`、
  `timeout`、`jitter`、`fails`、`passes`）到 `akit_config::HealthCheck`（`Upstream::health_check`、`HealthCheckType`、
  `DEFAULT_HEALTH_INTERVAL` 5s、`DEFAULT_HEALTH_TIMEOUT` 2s、`DEFAULT_HEALTH_THRESHOLD` 2；不写 jitter 时为 interval
  的十分之一；0 表示默认值，同 fit 的 `orDefault`），带 fit 的检查和消息：tcp 检查上拒绝 http 专用字段，path 以 `/`
  开头，host 是主机名或 IP 地址（不带通配符），timeout 不长于 interval，jitter 小于 interval（字段未写时在检查对象的
  位置报告，同 fit）。`akit_config::duration::format` 按 Go 的方式打印时长，用于这些消息。
- `akit_upstream::probe`，在 akio 客户端上实现的 fit 探测器：每个地址每 `interval` 加一段随机抖动探测一次，从一次探测
  开始到下一次开始计；同一地址的探测依次进行。**http**：发送 `GET path`，带 `User-Agent: akit-health-check` 和检查的
  `Host`，经由探测器自己的、不保留空闲连接的 akio-http `Client`（探测看的是地址是否接受新连接，也从不排在连接池的请求
  后面）；从响应头读状态码，放弃响应体（`ResponseBody::cancel`）；200 到 399 通过，或 `expect_status` 列出的状态码；
  请求的超时（由 akio 从开始计，含连接）就是检查的超时。**tcp**：连接建立即通过（`HappyEyeballs::connect`，在 akio 的
  解析器上解析名字），随即关闭；超时由探测器的定时器把守，因为 akio 拨号单个地址时用的是引擎的连接超时（A12）。`port`
  探测每个地址所在主机的另一个端口。
- 状态：fit 的 `record`：连续 `fails` 次失败让地址退出轮转（`Health::set_probed_down`），连续 `passes` 次通过让它
  回来，与当前状态一致的结果终结与之相反的连续计数。`Health` 把被动和主动状态分开保存，同 fit 的 `storeDown`：探测
  让地址下线时 `down_until` 为 `u64::MAX`，否则为被动窗口的结束；一个响应清除失败计数，不清除探测结果。选择地址读取的
  仍是原来那一个原子量，所以负载均衡器、`Pool::is_down`、`available` 和指标的 `backend up` 都能看到两者。连接池新增
  `health_check()` 和 `is_probed_down(i)`（fit 的 `ProbedDown`）。
- 生命周期：连接池的引擎就位时探测开始：`EngineSlot::set` 运行槽为空时创建的连接池登记的回调（`akit run` 先建连接池后
  建引擎），用已填充的槽创建的连接池则立即开始。探测器弱引用其连接池：连接池关闭（其运行时图退役）或被丢弃后，不再开始
  探测，进行中探测的结果被丢弃。重载产生的新连接池开始时所有地址都在轮转中，与 I20 的被动状态一样（fit 在重载间保留两者；
  akit 的连接池尚不在运行时图之间共享状态）。
- 线程（规则 1）：没有线程等待探测。每次探测由 `akit_upstream::timer` 的任务启动，这是 akio API 需求 **A45**（公开的
  定时器；在锁定的提交上 akio 的定时器都是内部的）之前的临时方案：akit 自己的一个线程，待运行任务的堆上限 65,536（每个
  被探测的地址最多两个：下一次探测和当前探测的超时），任务只启动探测或判定探测失败。结果在 akio 的回调里到达（I/O 线程、
  解析器线程或定时器线程），在那里记录并安排下一次探测。
- 测试：`akit-upstream/tests/probe.rs`（6 个），把 fit 的 `record`、`TestProberEdges` 和端到端的
  `TestHealthCheckRotation` 移植到真实套接字上的连接池：探测路径挂起的地址在 `fails × interval + timeout + interval`
  内退出轮转，下线期间收不到请求，并在 `passes × interval + interval` 内回来；http 探测的 `Host`、`User-Agent`、默认和
  列出的状态码，以及不读取巨大的响应体（关闭其连接）；探测检查 `port` 的 tcp 探测，端口拒绝连接时下线、重新监听时回来，
  地址自己的端口照常服务；被拒绝的 tcp 探测；关闭的连接池停止探测；探测等待引擎槽；没有检查的连接池从不探测；`Health`
  中探测与失败分开保存。`akit-config/tests/health_check.rs`（4 个）：默认值、每个字段、fit 的 "health checks" 用例及其
  位置、错误的值。门禁为这 11 个路径加了样例。
- 已验证：macOS 上 `make check`；Linux（基准主机）上 epoll 和 io_uring 的 akit-config、akit-upstream、akit-compat 和 akit-runtime
  测试。门禁（`make compat-bless`）：`examples/grpc`、`examples/l4-tcp`、`examples/load-balancing`、`examples/proxy` 和
  `examples/webrtc-proxy` 不再因 `health_check` 被拒绝（被拒字段从 72 到 71）；它们仍因其他字段被拒绝。没有运行基准（规则 30）；要测量的内容在开发计划的状态表中。
- 与 fit 的差异：
  - 探测与连接池一样是明文 TCP 上的 HTTP/1.1：fit 的探测跟随上游的协议和 TLS，它们随 I17 和 I18 到来，届时探测也要跟随。
  - 在 akio A45 之前，探测的定时器是 akit 自己的线程；探测先结束时它的超时任务仍待运行（届时什么也不做），因为这个临时
    方案不能取消。
  - 探测状态在重载后不保留（被动状态也不保留，I20）。

### 2026-11-24：I37，Prometheus 指标

- `akit_observe::metrics`（只依赖 std），即 fit `observe` 的指标：`Histogram`（fit 的 `DurationBuckets`，按线程
  分片）、`TextWriter`（Prometheus 文本格式 0.0.4，fit 的转义和数字格式，`CONTENT_TYPE`）、`Metrics`（注册表）、
  `Lease`（一个图从中取得的东西）、`UpstreamStats` 和 `BackendCounters`、`ConfigStats`（重载、版本、应用时间）。
- 按监听器和路由统计请求，与 fit 相同：图编译时，它服务的每个 `(监听器, 路由)` 取得一个槽（`Lease::route`；名字为
  路由名，没有名字时为 `#` 加序号，没有路由接收的请求为空，即 fit 的 `metricsName`），图退役时交还（模块清理，
  `akit_runtime::METRICS`），所以重载保留的路由计数继续，移除的路由不再被抓取；交还的槽的计数仍算在
  `Metrics::totals` 中。计数器按线程保存（`Metrics::shard`，分片数为主机线程数，最多 64；槽在线程首次使用时每次
  分配 64 个）：请求路径上没有锁，也没有共享的引用计数（规则 13）。同时最多统计 65,536 对（规则 11）。
- 监听器（`Dispatcher`）对在处理器返回前就应答的请求（`Handler::inline` 为真的处理器，即 fit 的 `module.Inline`：
  `respond`；以及 404 和 400）在调用返回后从 `Context::status` 计数，不用钩子，不分配内存；其他请求由发送其应答的
  线程上的应答过滤器计数，有访问日志时就用访问日志那一个（两者共用一个装箱闭包：akio API 需求 A29 会去掉它）。
- 上游，与 fit 相同：连接池统计其应答时间（`Pool::response_times`），以及每个后端结束和失败的交换
  （`backend_requests`，与被动健康检查计入的结果相同），并给出每个后端未结束的请求（`backend_active`）。通过
  `Lease::upstream`，计数器按上游名和地址与所有含该上游的图共享，所以跨重载延续（akit 为每个图新建连接池；fit
  共享连接池）。
- 证书：`akit_tls::CertificateInfo::of` 给出证书的名字（第一个 DNS 名，否则 common name）、序列号（同 Go 的
  `Text(16)`）和过期时间；抓取时从运行中图的证书读取（`Graph::certificates`，即 I30 的 `Certs`），所以证书监视
  应用的续期立即可见。
- 配置：`metrics`（`{}`，没有设置）读入 `Config::metrics`；没有 `admin` 时报 fit 的错误“the Admin API serves
  metrics: set admin too”。
- `akit_listener::metrics::Scrape` 写出抓取内容，fit 的指标族、fit 的顺序和说明：`fit_http_requests_total`、
  `fit_http_request_duration_seconds`（有 `metrics` 时）、`fit_upstream_requests_in_flight`、
  `fit_upstream_response_duration_seconds`、`fit_upstream_backend_requests_total`、
  `fit_upstream_backend_requests_in_flight`、`fit_upstream_backend_up`、`fit_tls_certificate_not_after_seconds`、
  `fit_connections_open`、`fit_connections_refused_total`、`fit_pending_bytes`、`fit_reads_paused_total`、
  `fit_config_reloads_total`、`fit_config_version`、`fit_config_last_applied_timestamp_seconds`、
  `fit_access_log_dropped_total`；然后是 akio 的引擎指标（`akio_*`，按 I/O 线程），对应 fit 的 Go 运行时指标
  （`go_*`）。`akit run` 为进程建一个注册表，并与已应用配置的计数（`ConfigStats`）一起放进 I26 的 `Runner`：
  每个配置都经过 `Runner::apply`（或在它之前、在 `reload` 或 `apply_text` 中失败），所以 SIGHUP、Admin API 的
  加载和 I30 的证书重载都被统计。Admin API 在 `GET /metrics` 提供抓取内容（`akit_admin::Options::metrics`）。
- 测试：`akit/tests/metrics.rs`，移植 fit 的 `TestMetrics` 和 `TestMetricsOff`（像 fit 的测试那样解析抓取内容，
  检查 fit 检查的每个样本及其标签和值：保留路由的重载前后按路由和类别的请求、未匹配的 404、耗时、上游的应答
  时间、每个后端的成功和失败、进行中和在线状态、TLS 监听器的证书、重载和版本、因预算暂停的读取、各个指标和
  akio 的指标族存在、旧图退役后改名的路由消失、总数保留），以及访问日志与指标共用一个钩子；`akit/tests/admin.rs`：带 `admin` 和
  `metrics` 的 `akit run` 在 `GET /metrics` 提供抓取内容（内容类型、请求、一次 SIGHUP、一次 Admin API 加载和一次被拒的
  加载都被统计）。单元测试：桶、
  类别、数字格式、转义、槽的延续、交还和复用、共享的上游计数器、配置统计（`akit-observe`），序列号
  （`akit-tls`）；`akit-config/tests/metrics.rs`（需要 admin、没有设置、只能由一个文件给出）；门禁的样本。
- 已与 I26、I30 和 I40 合并（akit main `4afa34a` 加 I30，`131908f`）。
- 已检查：macOS 上的 `make check`；Linux（基准测试主机）上 epoll 和 io_uring 下的整个工作区测试，除两个与 I37
  无关的不稳定测试外全部通过：epoll 下 `akit-proxy` 的 `bad_gateway` 和 `hop_by_hop_request` 同时失败过一次
  （`bad_gateway` 释放的端口被另一个测试的上游占用；重跑 14 个全部通过），io_uring 下 I22 的
  `reloads_under_traffic_with_no_failed_request`（`akit/tests/reload.rs`）5 次中失败 1 次，与不含 I37 的合并基线
  `131908f` 相同（那里 4 次中 1 次）。门禁
  （`make compat-bless`）：`metrics` 已支持；`examples/metrics` 有效（80 个夹具：11 个有效，68 个只因暂不支持被
  拒绝，1 个跳过，没有失败）。
- 与计划和 fit 的差异：
  - akit 的轮询按 I/O 线程各自轮转，所以移植的测试持续发送代理请求直到两个后端都被试过，并检查实际看到的
    计数，而 fit 的测试对每个后端恰好发一个。
  - `fit_connections_open` 包含 akit 在监听器引擎上拨出的上游连接和 UDP 对端，与 akio 的 `open_connections`
    相同（akio API 需求 A30）。`fit_reads_paused_total` 的 `reason="budget"` 为 0：akit 还没有
    `limits.max_pending_bytes`。
  - 在 akio 的客户端支持流式（A19–A21）之前，上游的应答时间是读完整个应答的时间；fit 计的是应答头。
  - 跨两次重载的进行中请求，若第一次移除其路由、第二次把它的槽给了另一对，会计入那一对（fit 的计数器由垃圾
    回收，没有这样的窗口）。
  - 没有追踪 span（`fit_trace_spans_total`：I64），也没有 apps、vod 或 websockify 的指标，akit 还没有这些。
- akio API 需求：A29（不分配内存的应答钩子）和 A30（监听器接受的打开连接数）。
- 基准测试任务应测量：fixed 和 proxy 场景下开关 `metrics` 的同轮 A/B，以及同时开启访问日志；64 个线程、大量
  路由时一次抓取的耗时。

### 2026-11-24：I30，证书管理与存储

- 证书管理器就是 akio-tls 的 `CertResolver`（akio A2，ADR-098），相当于 fit 的 `tlsmgr.Manager` 之于 Go 的
  `GetCertificate`：每次完整握手都向它要证书，它在调用中从内存作答（或稍后从其他线程作答，按需 TLS 即 I34 会用到）。
  现在每个 TLS 监听器的配置都围绕一个管理器构建（`ServerConfig::builder_with_resolver`），不再用 I09 的 `sni_cert` 表。
- `akit_tls::Certs`，对应 fit 的 `tlsmgr.Static`：监听器的证书（`CertifiedKey`），按叶子证书中的名字（小写 DNS 名、
  IP 地址）以及每个 `*.` 名字的父域建立索引。握手得到：按名字精确匹配或通过单层通配符匹配、且客户端能接受（其签名
  方案，以及 TLS 1.2 下与密钥类型相符的 ECDHE 套件，同 Go 的 `SupportsCertificate`）的第一个证书；否则该名字的第一个
  证书；否则所有证书中的第一个；只有一个证书时它服务所有名字。因此任何证书中的通配符现在都可用（I09 拒绝第一个之后
  的证书中的通配符），同一名字有 RSA 和 ECDSA 两个证书时客户端得到它支持的那个，与 fit 相同（I09 给第一个）。
- `akit_tls::ListenerTls`：一次加载的监听器 `tls`（证书、客户端 CA、协议），带所读每个文件的 SHA-256；
  `server_configs(manager)` 围绕一个管理器构建其 TCP 和 h3 配置；`same_config` 比较证书以外的一切。
  `server_config` 和 `server_configs(tls, protocols)` 保留，服务加载的证书。
- 重载不丢握手：每个 TLS 监听器的管理器 `akit_runtime::ListenerCerts` 提供 cell 当前图为该监听器（按名字）加载的
  证书，像请求一样通过 I/O 线程的缓存读图，不加锁。若重载后监听器配置相同、客户端 CA 相同（包括其文件内容），监听器、
  它的 socket 和连接都保留，下一次握手得到新证书；进行中的握手保留已拿到的证书。重载新开的监听器在切换前服务自己那张
  图的证书；正在排空的监听器在当前图已没有它时服务自己的证书。客户端 CA 变化会重开监听器（新的在旧的旁边，旧的排空），
  因为配置持有它们。这修正了 I22 中被保留的 TLS 监听器仍用旧证书的问题。
- `run -watch-certs`（默认开启，与 fit 相同）和 `-watch-interval`（1s）：`akit_tls::FileWatcher`，对应 fit 的
  `tlsmgr.FileWatcher`，在主线程循环中查看运行中配置加载的每个证书、私钥和 CA 文件（`Graph::cert_files`）的大小和
  修改时间，变化稳定（500 ms）且内容不同后，把运行中的配置作为来源为 `certificates`
  （`akit_admin::SOURCE_CERTIFICATES`）的新版本重新应用，像 SIGHUP 和 Admin API 一样经由 `Configs::update`，对应 fit 的
  `Configs.Reapply`（`akit: certificate files changed; reloading`，然后 `akit: reloaded certificate files (config
  version N)`）。无论配置从哪条路来，`Runner::apply` 都记下运行中的图加载的文件。失败的一对文件（私钥尚未写入）只报告一次，文件再变化前不重试；不可读的文件只报告一次。`-watch`
  （配置文件，I29）仍被拒绝。
- 存储：`akit_tls::Storage`，对应 fit 的 `tlsmgr.Storage`（load、store、delete、list、lock、unlock），用截止时间
  代替 context；其方法会阻塞，在 I/O 线程之外调用。`FileStorage` 对应 fit 的 `FileStorage`：根目录下每个键一个文件
  （检查键：没有空、`.` 或 `..` 段，没有反斜杠或 NUL），先写临时文件、sync 再 rename；值最大 4 MiB；目录 0700、文件
  0600；`list` 排序，跳过锁和临时文件。锁是 `locks/` 下用 `create_new` 创建的文件，持有期间每个锁一个线程每 5 s
  touch 一次，30 s 未被 touch（持有者已停止）即被删除，等待者每秒检查一次直到截止时间；同时最多持有 256 个（规则
  11）。ACME（I31）是它的第一个用户；`acme.storage` 和 `shared_storage` 在那之前仍被拒绝。
- 测试：`akit-tls` 单元测试（按 fit 的 `TestStatic` 选择证书：精确、通配符、仅单层、顺序、客户端的签名方案；Go 的
  支持规则）；`tests/storage.rs`（4 个，fit 的存储测试：键、值、大小、权限、list、截止时间、同一目录上两个存储轮流
  持有一个锁、等待新鲜的锁、接管过期的锁）；`tests/watch.rs`（2 个，fit 的 watcher 测试：续期只应用一次、忽略 touch
  和改回、失败的一对只报告一次、不可读的文件、其他途径应用的配置）；`tests/files.rs`（任何证书中的通配符、文件摘要和
  `same_config`）。`akit-listener/tests/certs.rs`（3 个，epoll 和 io_uring）：按 SNI 选择任何证书中的通配符；流量下
  替换证书（四个客户端一直做完整握手，重载保留监听器，之后服务新证书，没有失败的请求，之前建立的连接仍可用）；新的客户端
  CA 重开监听器。`akit/tests/watch_certs.rs`（2 个，用 `akit run` 端到端）：续期的文件无需重启即被服务且监听器保留，
  私钥尚未写入时失败一次、写入后应用；以及 `-watch-certs=false`。`cli.rs`：错误的 `-watch-interval` 退出码 2。
- 验证：macOS 上 `make check`；Linux（基准机 gpu51）上 akit-tls、akit-runtime、akit-listener 和 akit 的测试，epoll
  全部通过，io_uring 除 `akit-listener/tests/reload.rs`（明文 HTTP，其 `get` 读超时）外全部通过；该测试在负载
  200 到 480 的机器上于 main `1a5fccf` 失败得一样频繁（各 5 次中 2 次）：是 I30 之前就有的负载下偶发失败。合并 main（I26、I40）后：macOS 上 `make check`，以及 gpu51（负载约 10）上 akit
  和 akit-listener 的测试，epoll 和 io_uring 全部通过，包括 `reload.rs`。
- 与计划和 fit 的差异：
  - fit 的 `Certs`（ACME 与文件合用）、OCSP stapling（`Staple`）和共享存储（Redis、etcd、Consul）属于 I31、I32 和
    I62；trait 已为它们准备好。
  - watcher 运行在控制线程的循环（在 I26 的 `Control` 通道上 `recv_timeout`）中，而不是单独的 goroutine；它发起的
    重载、SIGHUP 的重载和 Admin API 的加载在那里依次进行。
  - 客户端恢复的会话不发送证书，所以在票据过期前保留首次握手的证书，与 Go 相同。
- 基准任务应测量：一个和十个证书的监听器每秒完整 TLS 握手数，对比 I09 的 `sni_cert` 和 fit；每次握手选择证书的开销
  （一次读图和一次 `Arc` 克隆）。

### 2026-11-24：I40，重写、跳转与头处理

- 在 I39 之后提前完成的 M3 迭代。路由的 `middleware` 被读取：`akit-config` 把每个模块引用（`module`、
  `params`）解码到 `Route::middleware`，即 `akit_config::Handler` 的中间件变体（`Rewrite`、`Redirect`、
  `Headers`：handler 和中间件共用一种参数类型，因为 akit-runtime 的注册表用同一种类型配置所有模块），
  检查和错误消息与 fit 的 `Provision` 相同；fit 的其他中间件模块以“暂不支持”拒绝，并给出模块名
  （`FIT_MIDDLEWARE`）。`akit-runtime` 的 `compile` 从注册表加载每条路由的中间件，错误位于
  `routes[i].middleware[j].params.<字段>`，并在编译时把它们一次性包在 handler 外面（规则 12），第一个在最外层。
  可执行文件的注册表包含 `akit_middleware::MODULES`。
- `akit-middleware` 实现了 fit 的 `rewrite`、`redirect` 和 `headers`（`middleware/rewrite`、
  `middleware/headers`）：
  - `rewrite`：`strip_prefix`（路径在段边界上以之开头的第一个前缀）和 `add_prefix`，作用于解码后的路径，
    新路径按 Go 的方式重新转义（`/api/a%2Fb` 去掉 `/api` 后是 `/a/b`；`!` 写成 `%21`）。新路径写入
    `CANONICAL_PATH`：`proxy` handler 把它（连同原样的 query）发给上游，`static` handler 用它查找文件，
    其后的中间件也以它为准。
  - `redirect`：`to` 中的 `{scheme}`、`{host}`、`{path}`、`{query}` 和 `{uri}` 在编译配置时拆分；`status`；
    `regexp`（`akio-regex`，与路由器编译路径正则的方式相同）匹配 Go 的 `URL.EscapedPath` 给出的转义路径
    （原样的 `\` 是 `%5C`），不匹配的请求交给 handler；fit 的开放重定向检查（`otherHost`、`hasScheme`），
    回答 `400 bad redirect target`。
  - `headers`：`request` 和 `response`，各自依次 `delete`（名字，或以 `*` 结尾的前缀）、`set` 和 `add`，
    名字按 Go 的 `CanonicalHeaderKey` 规范化。响应的修改是一个 akio 响应过滤器，因此作用于该路由的所有响应：
    立即回答的、之后从完成回调回答的（代理的）以及流式的。
- 临时方案，直到 akio 能做到（规则 21；都在 [akio API 需求](akio-api-requests.zh-CN.md) 中）：
  - A32：`akio-regex` 只回答模式是否匹配，不回答在哪里匹配，也不捕获分组。所以 rewrite 的 `regexp` 和
    `replacement`，以及 `to` 中对 redirect 正则分组的引用（`$1`、`${name}`），在读取配置时以“暂不支持”拒绝；
    `$$` 表示 `$`。
  - A33：akio 的 `Context` 只以共享引用提供请求。`headers` 中间件对请求的修改保存在上下文的 local 中
    （`akit_runtime::REQUEST_FIELDS`，由 `set_request_headers` 写入），由 `akit_runtime::request_headers`
    读取，`proxy` handler 把它发给上游；akio 自己读取的请求内容（`static` handler 的条件请求和范围请求字段）
    仍是客户端发来的样子。路径重写通过 `CANONICAL_PATH` 完成，与 I11 的规范化路径相同。
- 测试：`akit-middleware` 的 `tests/middleware.rs` 通过 akit 的监听器运行：移植了 fit 的 `TestRewrite`、
  `TestRedirect`、`TestRedirectHeaderIsItsOwn`、`TestHeaders`（每个两次，值不共享）、`TestResponseOfAnyKind`
  （立即回答和代理的响应）以及两个包的 `TestParamErrors`；需要 A32 的用例检查为以“暂不支持”拒绝；上游看到的
  重写后路径和修改后的请求字段，带 1 MiB 的 body；中间件按顺序运行（先 rewrite 再 redirect）；中途离开的客户端；
  路径转义、`otherHost`、`hasScheme`、占位符和请求字段 local 的单元测试。
- 兼容性门禁：`tests/compat/supported.txt` 中的 22 个字段；门禁把被拒绝的中间件模块记为
  `routes[].middleware[]<模块>`，把 rewrite 被拒绝的字段连同其模块记录；`make compat-bless`：
  `examples/headers` 有效，其余示例只因所用的中间件模块被拒绝（80 个夹具：10 个有效，69 个只因暂不支持被拒绝，
  1 个跳过，没有失败）。
- 已检查：macOS 上的 `make check`；基准主机上，相关 crate 的测试（`cargo test -p akit-middleware -p akit-config
  -p akit-runtime -p akit-router -p akit-proxy -p akit-compat`），epoll 和 `AKIO_TEST_BACKEND=uring` 都跑过。
- 与 fit 的不同：经过 akit 的监听器时，路径在任何中间件看到之前已被规范化，与 fit 的监听器相同，所以 fit 的
  `path makes //` 用例端到端回答 `302 /evil.com/x`（挨着 `/` 的 `\` 被监听器以 400 拒绝）。头名字的错误
  报告在 `request` 或 `response` 对象上，与 fit 相同。
- 待测量，由维护者的基准任务完成（规则 30）：见开发计划中 I40 的基准行。

### 2026-11-24：I26，Admin API I：加载、读取、版本

- 配置：`admin` 读入 `akit_config::Admin`（`address`、`history`、`max_config_bytes`、`max_conns`；
  `DEFAULT_ADMIN_*`，fit 的默认值：127.0.0.1:2019、10、4 MiB、16；0 即默认值）。按 fit 检查地址：主机不是
  `localhost` 或回环地址时拒绝（fit 在非回环地址上要求 `token_file`、`cert_file` 和 `key_file`，它们随 I27 到来）；
  `unix/<path>` 作为尚不支持拒绝（akio API 需求 A23）；`admin` 只能由一个文件给出。其余 admin 字段作为尚不支持拒绝。
- `akit_config::value`（新增）：`Value`，成员保持顺序、数字保持原写法的 JSON 值；`parse`、`to_json`（fit 的
  Admin API 写法：每层缩进两个空格）、`standardize`（注释和尾随逗号替换为空格，其余字节原位不动，与 fit 的
  `config.Standardize` 一样），以及 `json_of`：把配置的各个文件作为一个 JSON 文档（单个 JSON 文件标准化；多个文件，
  或 YAML、`.conf`，按 fit 的 adapter 的方式合并后写出）。`load_files` 同时读出配置和该文档。
- `akit-admin`（决定 006）：`Configs` 是历史记录（从 1 编号，保留最近 `history` 个；ETag 取 SHA-256 的前 96 位，
  用 akio-crypto 计算；`If-Match` 和 `If-None-Match` 按 fit 的方式比较），也是每个配置都要经过的唯一路径
  `Configs::update`，一次一个；`Server` 是运行在自己的 akio 引擎上的 akio-http（一个 I/O 线程，HTTP/1.1，
  `max_conns` 作为引擎的 `max_connections`，fit 的超时）：`GET /config`（ETag、`Fit-Config-Version`，
  `If-None-Match` 命中回 304）、`GET /config/<pointer>`（RFC 6901，fit 的消息）、`POST /load`（`application/json`，
  在 `max_config_bytes` 之内流式读入，413/415，`If-Match` 不符回 412）、`GET /versions`、`GET /versions/{n}`；
  fit 的防护（带 `Origin` 回 403，同源读请求除外；`Host` 必须是回环主机）。加载请求被分离，作为 `Job` 交给控制线程
  （有界队列，满了回 503），由它应用并回答。回答是 fit 的 JSON：
  `{"version","etag","listeners_added","listeners_removed","warnings"}`，配置错误为
  `{"error","errors":[{"path","line","column","message"}]}`。
- `akit run`：有 `admin` 时，在监听器之后启动 API（`akit: admin API on ADDR`）；信号和加载共用一个有界控制通道，
  由主循环依次应用：加载经 `Runner::apply_text`（解析、编译、`Listeners::reload`、访问日志的级别和时区），SIGHUP 经
  同一个 `Runner::apply`，各自成为新版本（`akit: reloaded FILE (config version N)`、`akit: loaded a config from
  the admin API (config version N)`）。修改 `admin` 的加载会提示需要重启；访问日志的修改与之前一样提示。关闭时先停
  API，队列中仍在等待的加载回 503。
- 测试：`akit-admin/tests/admin.rs`（10 个），移植 fit 的 `admin_test.go`：加载与读回（整份、按指针、按版本、304、
  HEAD、404、405），带注释的配置保存为标准 JSON，被拒绝的加载（未知字段，回答与 fit 完全一致；不存在的模块；地址被
  占用；类型不对；按长度和分块两种方式超限；未结束的 JSON；I27 的端点和试运行回 501），`If-Match`，拒绝浏览器，
  历史记录，8 个并发加载各得不同版本，慢客户端与中途断开的客户端，连接数上限，监听器的增删与 admin 提示；历史记录、
  ETag、指针和回环主机的单元测试。`akit-config/tests/admin.rs`（4 个）：各字段、fit 的地址检查、只能由一个文件给出、
  `load_files`；`value` 的单元测试。`akit/tests/admin.rs`：`akit run` 端到端，一次加载和一次 SIGHUP 在同一份历史中
  编号。门禁为这五个字段加了样例。
- 模糊测试：新增目标 `config_value`（在 CI 中与其他目标并列）：标准化保持长度和值不变，写出的值能解析回自身；
  45 秒 300 万次运行，无崩溃。
- 已检查：macOS 上 `make check`；Linux（基准主机）上 akit-config、akit-admin、akit、akit-compat、akit-runtime 和
  akit-listener 的测试：io_uring 全部通过；epoll 除 I22 的同地址替换测试（`akit-listener` 和 `akit` 的
  `tests/reload.rs`）外全部通过，这两个测试在 main `1a5fccf` 上也会偶尔失败（那里 6 次中 1 次，这里 3 次中 1 次）：
  请求在被替换的监听器停止接受之前的瞬间到达它，得到其图的 404。
  门禁（`make compat-bless`）：支持 `admin` 及其四个字段；`examples/admin` 现在只因 Unix 套接字被拒绝，
  `examples/console` 因 `admin.console` 和 `admin.store` 被拒绝，`bittorrent`、`metrics`、`proxy` 和 sync 示例
  不再因 admin 被拒绝（被拒绝的字段从 58 变为 60，因为 `admin` 拆成了它的子字段）。
- 与计划和 fit 的不同：
  - 还不支持 Unix 套接字：akio 的监听器只接受 `SocketAddr`（akio API 需求 A23）。
  - 超过 `max_config_bytes` 的加载回 413，但 akio-http 会继续读取请求体的剩余部分（丢弃）并保留连接，而 fit 会关闭
    连接（akio API 需求 A24）；慢客户端受请求体超时约束。
  - fit 的令牌和 Unix 套接字测试没有移植（I27、A23）；`TestWarnings` 中访问日志和 limits 的提示也没有移植：akit 的
    提示来自 `akit run` 自己的应用步骤。
  - `PUT`/`PATCH`/`DELETE /config`、`POST /rollback/{n}`、`POST /reload` 和 `?dry_run=1` 在 I27 之前回 501；
    `/config/effective`（providers）属于 I57，目前按指针读取。
  - 版本历史中的 `applied` 是 UTC 的 RFC 3339，精确到毫秒；fit 的是 Go 的 `time.Time`（纳秒，本地时区偏移）。
- 为 I37（指标）预留：`akit_admin::Options::metrics`，`GET /metrics` 用它写出的内容回答（`METRICS_CONTENT_TYPE`；
  未设置时回 404）。合并时，`akit run` 把 I37 的 `Scrape::write` 传给它，并在每次 Admin API 加载后调用
  `ConfigStats::reloaded`（`Job::run` 返回结果，未应用时为 `None`），与其 SIGHUP 重载一样。
- 基准测试：无（规则 24：控制面）。

### 2026-11-24：I20，被动健康检查

- 配置：`upstreams[].max_fails`（从 0 起；省略时为 1，0 表示所有地址一直参与轮转）和
  `upstreams[].fail_timeout`（省略或为 0 时 10s）读入 `akit_config::Upstream`（`max_fails`、
  `fail_timeout`；`DEFAULT_MAX_FAILS`、`DEFAULT_FAIL_TIMEOUT`），与 fit 相同。
- `akit_upstream::health::Health`，连接池每个地址一个，对应 fit 的 `Backend`：在 `fail_timeout`
  内（窗口从第一次失败开始）失败 `max_fails` 次，该地址从最后一次失败起退出轮转 `fail_timeout`；
  它的一次响应会清除失败计数并立即让它回到轮转；不同窗口里的失败不累加。失败即
  `Error::is_upstream_failure` 所说的：客户端的错误（连接被拒或超时、连接断开、响应格式错误或过大、
  交换超时）以及等待连接超时；`Busy`、`Closed`、`NoEngine` 和错误的目标不算。任何响应都算成功，
  不论状态码，与 fit 相同。连接池在调用请求回调之前记录结果，因此回调看到的是这次交换之后的健康状态。
- 负载均衡器跳过已下线的地址，与 fit 相同：`Balancer::pick_up(load, up)`（`pick(load)` 不变，
  视所有地址为在线）。轮询和最少连接把槽的轮次移过被跳过的地址；加权轮询多抽取其轮次中的条目，
  于是下线地址的份额按权重分给其他地址，而加锁形式（轮次太长无法预先算出）只在在线地址上运行
  nginx 的算法。所有地址都下线时忽略健康状态，选择与不看健康状态时相同。`Pool::pick`（签名不变）
  传入每个地址的健康状态；连接池新增 `is_down(i)`、`available()`（fit 的 `Available`）、
  `max_fails()` 和 `fail_timeout()`。
- 状态（规则 13）：健康状态属于地址，由所有 I/O 线程共享，与 fit 相同，这样一个线程看到的失败会让
  该地址对所有线程下线。一次选择对它查看的每个地址读一个原子量，只有地址曾下线时才读时钟（每次选择
  最多一次）；一次成功读两个原子量，地址没有失败计数时什么也不写。只有失败以及失败之后的第一次成功
  才会获取该地址自己的互斥锁：没有全局锁，成功请求的路径上没有锁。时间是自连接池创建以来的纳秒数
  （`Instant`）。
- 测试：`akit-upstream/tests/health.rs`（9 个），移植 fit 的 `health_test.go` 和
  `TestWeightedRoundRobinDown`：在测试控制的时钟上测 `max_fails`（计数、窗口、下线 `fail_timeout`
  后恢复、`max_fails` 为 0 和 1）；成功清除计数；每种策略跳过下线地址（精确份额）、全部下线（忽略健康
  状态）、窗口过后恢复；最少连接跳过空闲但下线的地址；加权轮询有下线地址（按权重分配；全部下线时
  精确按权重），有和没有预算轮次两种情况；在真实套接字上通过连接池：fit 的 `TestLeaveAndRejoin`
  （开始拒绝连接的地址在第一次失败后下线，下线期间没有请求到达它，`fail_timeout` 后恢复）、所有地址
  下线时仍会选择、通过连接池计数 `max_fails` 3。`akit-config/tests/upstreams.rs`：这两个字段、默认值
  和范围。门禁对这两个字段的样例。`akit-upstream/tests/pool.rs` 中拒绝连接地址的测试设置
  `max_fails` 0，使其交替顺序不依赖时序。
- 变基到 akit main `69e255e`（akio `a9a620ab`）：冲突只在配置导出、状态行和生成的兼容性文件中，已重新生成。
- 已检查：macOS 上的 `make check`；Linux（基准测试主机）上 akit-config 和 akit-upstream 的测试，
  epoll 和 io_uring，全部通过。门禁（`make compat-bless`）：`upstreams[]` 的 37 个字段中支持 12 个；
  `examples/load-balancing` 和 `examples/proxy` 不再拒绝 `max_fails` 和 `fail_timeout`（被拒字段
  从 60 降到 58）。
- 与计划和 fit 的差异：
  - 未移植 fit 对有地址下线时选择的内存分配检查（`TestPickSkipsDown` 的 `AllocsPerRun`）（计数分配器
    需要 `unsafe`）；选择在构造上不分配内存。
  - fit 的主动健康检查（`health_check`、`probedDown`）属于 I35；`Health` 还没有探测状态，
    `health_check` 仍被拒绝。
  - akio 的客户端缓冲整个响应体，因此回调运行时结果已知；I16 流式传输响应体后，中途断开的响应体
    应像 fit 的 `Done(b, true)` 那样算作失败（届时 I16 需要一种记录方式：一个不会重复计数的
    `Pool` 方法）。
- 基准测试任务应测量：每种策略的 `Pool::pick`，无地址下线、一个下线和全部下线，1 个和所有核
  （时钟读取和额外抽取）；请求路径上记录一次成功（两个原子加载）；一个地址拒绝连接时与 fit 对比的
  代理运行，检查错误保持在每个 `fail_timeout` 一次。

### 2026-11-24：I22 改用 akio 的 A1：所有监听器都可重载，重载可新增 h3

- `Listeners::reload`（`crates/akit-listener/src/reload.rs`）现在用 akio 的运行时监听器（A1，ADR-099）迁移监听器，
  不再用 `adopt_tcp` 临时方案，其 accept 线程随之去掉。换入之前，新增或修改的监听器的 TCP 一侧用
  `Engine::add_listener` 添加（用 `Options` 的后端，与启动监听器一样有自己的 I/O 线程），所以地址被占用仍会让重载
  失败且什么都不改变（为它添加的监听器会再移除）。换入之后，移除或修改的监听器用 `Engine::shutdown_listener` 在
  `-shutdown-timeout` 内排空（fit 也排空退役的 binding）：它的每个连接都被要求关闭（HTTP/1 在当前响应后带
  `Connection: close`，HTTP/2 发 `GOAWAY`），到期仍打开的连接被关闭。`Listeners::new` 接受 `Options`，并从
  `Engine::listeners` 读取监听器的 id。
- akit 启动时的监听器与其他监听器一样可以重载：修改（协议、`tls`、限制）或移除，不再被拒绝。替换同一地址上监听器的
  新监听器加在旧的旁边：Linux 上 akio 的监听器在旧的排空期间共享端口（`SO_REUSEPORT`），不会拒绝任何连接。macOS 上
  akio 的监听器不共享端口：旧的先停止监听（在其 I/O 线程关闭套接字期间最多重试一秒），与 fit 在这类系统上的重载一样，
  若之后重载失败也无法恢复。
- 重载时的 HTTP/3：重载新增的服务 `h3` 的监听器，TCP 一侧来自 `add_listener`，HTTP/3 用 `Engine::bind_udp` 绑定在
  TCP 得到的端口上（端口为 0 时与启动时一样探测一个两者都空闲的端口），运行在引擎的 UDP 线程上：最后一个服务 h3 的
  启动监听器的线程，或没有这样的监听器时 `engine` 现在添加的一个 UDP 线程（`udp_client_threads(1)`）。之后的重载移除它
  时将其关闭（`UdpBinding::close`，其连接一并关闭）。akio 只能添加和移除 TCP 监听器：服务 h3 的启动监听器（引擎构建时
  就有其 UDP 监听器）不能移除或修改，这样的重载被拒绝（`ReloadError::Udp`，"… serves h3 and is removed or changed,
  which needs a restart …"），保留正在运行的配置。已向 akio 提出 A22。
- `lib.rs`：`build` 拆出 `tcp_listener` 和 `h3_acceptor`，`reload` 也使用它们。
- 测试：`akit-listener/tests/reload.rs`（3 个）：监听器的保留、新增、同一地址上的修改和移除，被移除监听器上的空闲
  连接被排空，地址可再次使用；akit 启动时的监听器在其地址上被修改和被移除；什么都不改变的拒绝（在添加另一个新监听器
  之后遇到被占用的地址、只有 h2c）。`akit-listener/tests/h3.rs` 的 `reload_knows_h3_listeners`：重载新增的 h3 监听器
  通过 HTTP/3 应答，通过 HTTP/1.1 应答时带 `Alt-Svc`；移除启动时的 h3 监听器被拒绝且纪元不变；新增的那个再被移除。
  `akit/tests/reload.rs`，有流量时的端到端测试：客户端现在遵从 `Connection: close`（被排空的监听器的应答），`web` 监听
  每个配置都写明的固定端口，在 Linux 上它在流量下于该地址被替换（修改协议，再改回），没有失败的请求；原先需要拒绝启动
  监听器的那一步去掉了。排空会立即关闭空闲的 HTTP/1 保活连接（akio-http 的 `on_shutdown`），客户端已在其上
  发出的请求会碰上关闭而得不到应答；测试的客户端像 HTTP 客户端对幂等请求那样（RFC 9112 第 9.3.1 节）在新连接上重发
  并计数（每次替换每个客户端至多一次）；其他任何错误仍使测试失败。
- 已检查：macOS 上的 `make check`；Linux（基准主机）上 epoll 和 io_uring 的 akit 和 akit-listener 测试，端到端测试
  另外再跑五次。
- 遗留：服务 h3 的启动监听器（A22）；被移除监听器上换入之后才到达的请求仍找不到同名监听器而得到 404（其连接已被要求
  关闭）；保留的 TLS 监听器沿用启动时的证书（A2 的 resolver，I30）；不重试的客户端在其所用监听器被移除或替换时
  会遇到上述保活关闭竞争。

### 2026-11-24：I13，静态文件 II

- 在 akio `a9a620ab`（A13 和 A16–A18，akio ADR-100）上，`static` handler 按 fit 在 I13 的全部参数设置 akio-http 的
  `Files`：
  - `precompressed`（akit-config 的 `Static::precompressed`、`Encoding`；在 `precompressed[i]` 报 `"deflate" is not
    one of "br", "zstd", "gzip"`）设置 `Files::twins`：在列出的编码中，按客户端的 q 值选它最偏好的编码的副本（`name.br`、
    `name.zst`、`name.gz`）（RFC 9110：`q=0` 拒绝该编码，`*` 代表未列出的编码，名称不区分大小写），q 值相同时按列出的
    顺序；未写时没有副本，也没有 `Vary`。来自内存，或超过 `max_file_bytes` 时来自磁盘并带副本的长度；`Content-Encoding`、
    `Vary: Accept-Encoding`、原文件的 `Content-Type`、副本自己的 `ETag`（条件请求作用于它）、副本的范围请求；副本被修改或
    删除时与普通文件一样跟进。
  - `try_files` 设置 `Files::try_paths`：`{path}` 代表请求的路径，以 `/` 结尾的候选指目录，用它的索引文件（或目录列表）
    提供，不重定向；提供第一个存在的，都不存在时答 404。候选把路径解析成什么会被保留（最多 4096 个路径），在
    `check_interval` 内被信任，所以最近解析过的路径在 I/O 线程上应答；其余在 akio 的文件池上解析。像 fit 一样检查：每一项
    以 `/` 或 `{path}` 开头，`dotfiles` 为 `deny` 时除 `/.well-known/` 下外不指向点文件。空列表与 fit 一样什么也找不到
    （每个请求都是 404）。
  - `browse` 和 `browse_max_entries`（未写或为 0 时 10000）设置 `Files::list_directories`：对没有索引文件的目录给出 fit
    的页面，每个请求都在池上读取。
  - `dotfiles` 为 `deny` 时还设置 `Files::hide`，于是点文件段所指的 `try_files` 候选被跳过，与 fit 的 handler 一样。
  - 提供的是路由匹配所用的规范路径（akit-listener 的 `CANONICAL_PATH`，A13 的 `Files::serve`）：仍在根目录内的 `..`
    （`/x/../ok.txt`）被去掉后提供文件，与 fit 的监听器和 handler 一起时一样；I12 时答 400。
  - `strip_prefix`：从该路径按原样去掉，没有该前缀的路径答 404，其余部分照旧检查后提供；像 fit 一样报
    `"assets" does not start with /`。只有带前缀的 handler 或路径不是规范形式的请求才复制路径；否则 `Files` 从请求中读取。
- 大文件的 `sendfile`：I12 起已经有了。`Files` 把大于 `max_file_bytes` 的文件（或副本）从保留的句柄以 `FileBody` 发送，
  akio 在明文 HTTP/1 上用 `sendfile` 发送；在 TLS 和 h2 上 akio 分块读取。
- 兼容性门禁：`strip_prefix` 和 `try_files` 也已支持（共 72 个字段）；fit 的 `examples/static` 只因 `max_cache_bytes` 和
  `middleware` 被拒绝，`examples/bittorrent` 不再因 `strip_prefix` 被拒绝（`make compat-bless`）。`compress` 仍被拒绝
  （I47 的压缩），`max_cache_bytes` 和 `default_type` 等待 A15 和 A14。
- 没有添加基准场景：fit 的测试框架没有针对副本、`try_files` 或目录列表的场景（它的配置不提供任何预压缩内容，见基准
  README），因此 akit 的也没有（规则 30：功能任务不跑基准）。
- 测试（`akit-handler/tests/static_files.rs`，16 个）：`precompressed`，移植 fit 的 TestPrecompressed，11 个
  `Accept-Encoding` 字段全部采用 fit 的应答，每种表示各自的 ETag 和 `If-None-Match`、副本的范围请求、`HEAD`、来自磁盘的
  副本、只列一种编码、不列编码（没有副本，没有 `Vary`）；`precompressed_follows_the_disk`；`large_from_disk` 包含 fit 的
  大副本；`try_files`（fit 的 TestTryFiles 和 TestTryFilesUnsafe，每个路径请求两次以经过 `Files` 保留的内容，空列表，
  被跳过的点文件候选）；`browse`（fit 的 TestBrowse、TestBrowseMaxEntries、TestBrowseOrder）；`strip_prefix`（fit 的
  TestStripPrefix）；`config_errors` 包含 fit 的 TestFeatureErrors、TestDotfilesConfig 和 `strip_prefix` 的检查；`traversal` 现在
  检查 `/x/../ok.txt` 被提供。另有对
  已解码路径的点文件检查（`Files::hide` 所用）的单元测试。
- 已检查：macOS 上的 `make check`；Linux 上 akit-handler、akit-config、akit-compat 的测试（epoll）与 akit-handler 的测试（io_uring）通过。
- 与计划和 fit 的不同之处：
  - 计划中的"有基准场景使用它"：如上，没有；基准表（dev-plan）写明静态文件尚未做基准。
  - A14（I12 的）：`Files` 对不带斜杠请求的目录直接提供（fit 用 301 重定向），在 `strip_prefix` 之下也是；没有候选找到
    路径时的 404 用它自己的文本；`strip_prefix` 之下的目录列表以前缀之后的路径为标题（`Index of /`，没有 `../` 链接），
    fit 以整个路径为标题（`Index of /assets/`）。已加入 A14。
  - `strip_prefix` 从发送时的路径（百分号编码）去掉，fit 从解码后的路径去掉：请求中用转义写出的前缀（`/%61ssets/`）在
    akit 中是 404。
  - 带 `try_files` 时的 `/../secret`：由 akio 的解析器以它自己的文本答 400，与 I12 的路径穿越测试记录的一样。
- 给基准任务：在 fit 的测试框架有对应场景之前没有；之后测量来自内存的副本和来自磁盘的大副本相对原文件的表现、从 `Files`
  保留的内容解析 `try_files` 相对线程池的表现，以及每个请求按 `Accept-Encoding` 排序的开销。

### 2026-11-24：I21，访问日志

- `akit-config` 读取 fit 的 `access_log`：`output`（`stdout`，默认，`stderr` 或文件路径）、`flush_interval`（1s）、
  `buffer_bytes`（4 MiB；与 fit 相同，0 表示默认值）、`level`（`info`、`warn`、`error`：写出的最小状态码，0、400 或
  500）、`format`（`json`、`text`、`line`）和 `time_zone`（按原样保存）。`rotate` 仍以暂不支持拒绝（fit `logs` 包的轮转）。
  与 fit 的 adapter 合并文件一样，第二个文件再给出 `access_log` 会被拒绝：`also given in a.json: only the lists at the
  top of a config (listeners, routes, upstreams, services, providers) may be split across files`。JSON、YAML 和
  `.conf` 都一样。
- `akit-observe`（只依赖 std）：
  - `AccessLog`：日志行在完成响应的线程上格式化，写入该线程的缓冲（分片：每个写日志的线程一个，最多 64 个，取 2
    的幂；更多的线程共用），每个分片有自己的互斥锁，只有该线程和写线程（每次刷新一次）会取。分片攒到 64 KiB（fit 的
    `flushBytes`）时经有界通道交给写线程，写线程每个刷新间隔在分片锁下经同一通道取走各分片的内容，所以同一线程的
    日志行保持顺序。等待写出的加上分片里的，以 `buffer_bytes` 为界，与 fit 相同；超出的行被丢弃并计数，从不等待。写线程
    每次刷新报告一次丢弃数（stderr 上 `akit: access log: N lines dropped, the output was too slow`，fit 的措辞）；输出
    失败时该缓冲的行丢失，计入丢弃。`close` 写出剩余内容并 join 写线程；之后的行被丢弃。`set_min_status` 和
    `set_zone` 就地修改级别和时区（重载时用）。
  - `Format`、`Record` 和 `append_line`：fit 的三种格式，字段及顺序与 fit 相同（`time`、`listener`、有路由时的
    `route`、`client`、设置时的 `user`、`proto`、`method`、`host`、`uri`、`status`、`bytes_in`、`bytes_out`、
    `duration`、代理时的 `upstream` 和 `upstream_duration`、`user_agent`、`referer`）；秒数六位小数，舍入到微秒；
    JSON 字符串同 fit 的 `appendString`（非 UTF-8 字节写成 U+FFFD）；logfmt 值原样或按 Go 的 `strconv.Quote` 加引号；
    `text` 和 `line` 的级别按状态码（INFO，4xx 为 WARN，5xx 为 ERROR），`line` 的级别补齐到五个字符。时间是带毫秒的
    RFC 3339，用所设时区（UTC 为 `Z`，否则如 `+08:00`），`line` 为 `2006-01-02 15:04:05.000`；时区偏移每个分片每秒
    查一次。
  - `Zone`：`local`（或空：`TZ`，否则 `/etc/localtime`，否则 UTC，同 Go 的 `time.Local`）、`utc`，或从系统 zoneinfo
    目录（Go 的目录列表）读取的 IANA 名称；TZif 读取器（版本 1 到 4），最后一次跳变之后的时间用文件尾的 POSIX `TZ`
    规则（`Mm.w.d`、`Jn`、`n`，时刻最多 167 小时，没有日期的规则用美国规则）。会检查名称不会越出目录。`akit run` 和
    `akit validate` 都会加载它，所以错误的值报为 `access_log.time_zone: "Nowhere/City" is not a time zone: local,
    utc or an IANA name such as Asia/Shanghai`，即 fit 的措辞。
  - `with_upstream`、`upstream`、`dispatch_logged`、`logged`：用线程局部变量传递 handler 知道的信息；`Name`，内联
    保存的短名字（监听器或路由的名字，带到日志行时不单独分配）；`open_output`。
- `akit-runtime`：`Env::with_access_log` 把进程的日志交给带 `access_log` 的配置编译出的图（`Graph::access_log`）；
  没有它的配置不写日志。
- `akit-listener`：图有日志时，`Dispatcher` 为每个请求注册一个响应过滤器（`Context::add_response_filter`），包括
  坏路径的 400 和 404，持有本线程的分片、开始时间、监听器名和路由名。akio 在发送响应的线程上运行它（I/O 线程，或
  分离的 handler 应答所在的线程），在那里根据请求和响应生成日志行（`access_record`）：客户端（映射到 IPv6 的 IPv4
  地址写成 IPv4）、`HTTP/1.0` 到 `HTTP/3.0`、`Host`（或绝对形式目标的 authority）、原样的请求目标、`bytes_in`（已读
  的请求体，否则 `Content-Length`）、`bytes_out`（`HEAD`、1xx、204 和 304 为 0；文件的长度；响应体的长度；流式响应体在
  发出响应头时还不知道字节数，取其 `Content-Length` 或 0）。从未发出的响应（连接已断）没有日志行，与 fit 相同。handler
  在 `dispatch_logged` 中运行。
- `akit-proxy`：对要写日志的请求，自己选地址（`Pool::pick`，再 `send`），并在 `with_upstream(地址, 耗时)` 中应答，
  使日志行带上 `upstream` 和 `upstream_duration`；不写日志的请求照旧，不做任何复制。
- `akit run`：在编译图之前打开日志（输出打不开时打印 `akit run: access log: <错误>` 并以 1 退出），与 fit 一样提示没有
  `rotate` 的文件输出会无限增长，并在引擎停止后关闭它（关闭超时时也一样），因此每个已完成的响应都有日志行。重载会设置
  新的级别和时区；`output`、`format`、`flush_interval` 或 `buffer_bytes` 的变化需要重启，并会提示；重载时新加的
  `access_log` 也一样。
- 兼容性：`supported.txt` 标记了 `access_log` 及其六个字段；门禁映射了 `ACCESS_LOG_FIELDS` 和 `ACCESS_LOG_FIT_ONLY`，
  并为每个字段取样。`make compat-bless`：`examples/logging` 不再因 `access_log` 被拒绝（仍因 `log` 被拒绝）；被拒绝的
  字段从 66 个变为 65 个。
- 测试：`akit-observe` 的单元测试（日期、时间、时长、JSON 和 logfmt 转义、南北半球的 POSIX 规则、带文件尾的 TZif
  文件以及截断的文件、系统存在时的 `Asia/Shanghai` 和 `America/New_York`、会越出 zoneinfo 目录的名称、`Name`、
  线程局部传递）和 `tests/access_log.rs`（9 个，来自 fit 的 `accesslog_test.go`）：每种格式的每个字段，逐字比较；就地
  修改级别和时区；刷新间隔；缓冲满时不等间隔就写出；**写入比流量慢**：输出在第一次写入时被阻塞，4 个线程写 10,000
  行，立即返回（2 秒内），超出缓冲的行被丢弃，写出加丢弃等于写入总数，每次丢弃都有报告，没有被截断的行；输出失败；
  8 个线程各自保持顺序；追加写文件。`akit-config/tests/access_log.rs`（5 个）：默认值、每个字段、每类错误及其路径、
  YAML 和 `.conf`、只能由一个文件给出。`akit/tests/access_log.rs`（2 个，用 `akit run` 端到端）：一个 respond 路由、
  一个被代理的 `POST`（`upstream`、`upstream_duration`、`bytes_in`）、一个 404、一个 400、一个 `HEAD`、一个 HTTP/1.0
  请求，逐行检查字段，刷新间隔是一小时但关闭时全部写出；重载为 `level: error` 后不再写 4xx 的行。写入慢的测试发现了
  第一版的一个缺陷：写线程按间隔刷新时到达的关闭消息被当作缓冲处理而丢失，关闭就永远等待。
- 与计划和 fit 的不同：除 `akit-observe` 外还涉及 `akit-config`、`akit-runtime`、`akit-listener`、`akit-proxy` 和
  `akit`。日志行写入分发请求的线程的分片，而不是按客户端地址选分片（fit 按地址分片以保持一个连接的日志顺序；这里一个
  连接由一个 I/O 线程服务，不用哈希就能做到）。被代理请求的日志行在上游响应到达的线程（连接池的线程）上生成，并在那里
  锁分发线程的分片：此时分片锁在两个线程间有竞争。没有实现 fit 的已写缓冲空闲列表（每 64 KiB 日志新建一个缓冲）。在有
  认证（I42）之前 `user` 从不设置。流式响应的 `bytes_out` 是其 `Content-Length`，而不是实际写出的字节数：akio-http 只给
  响应过滤器看响应头。日志行在之后注册的过滤器运行之前生成，所以等压缩（I47）注册过滤器后，`bytes_out` 会是压缩前的
  响应体大小，而 fit 记的是写出的大小。logfmt 加引号时近似 Go 的 `unicode.IsPrint`（控制字符、分隔符和常见格式字符会
  转义）。没有 fit 的 `log` 段（fit 自身的日志，以及它与访问日志共享的文件 sink）。
- 已检查：macOS 上 `make check`；Linux（gpu51）上涉及的 crate（akit-observe、akit-config、akit-runtime、
  akit-listener、akit-proxy、akit、akit-compat），epoll 和 io_uring。
- 留给压测任务（本任务未跑任何压测，规则 30）：fixed `respond` 路由和代理在日志关闭、写 `/dev/null`、写文件三种情况下
  的同轮 A/B，`json` 和 `line` 各一次；每个请求的开销：响应过滤器（每个请求一个装箱闭包，akio-http 的 API：若开销明显，
  就向 akio 提无分配完成钩子的 API 需求）、分片锁（代理请求时有竞争）和格式化；默认 4 MiB 缓冲以及磁盘慢时满负载下
  丢弃的行数。

### 2026-11-24：I16，反向代理 II：流式 body 与联动背压

- 基于 akio `60547435`（ADR-101）完成，A19–A21 已在其中落地（最初在这里写成 A16–A18，合并时重新编号）。锁定的
  提交移到这里（`Cargo.toml`、`akit_runtime::AKIO_REV`、`akit version` 的那一行）。等待期间先完成了 body 限制
  （见下文）。
- `akit-upstream`：`Pool::stream(backend, req, impl Exchange)`，请求体边产生边写出（`Exchange::on_request_body`，
  akio 的 `RequestBody`）、响应边读边到达（`on_head` 带一个可暂停、恢复上游读取或取消的 `Download`，`on_data`、
  `on_end`，`on_error` 带连接池的 `Error`）的交换，基于 akio 的 `Client::request_with`。它和 `send` 一样受
  `max_conns` 和 `max_pending` 约束、参与负载均衡和计时，并占用其地址直到响应体完整，因此 `max_conns` 和
  `least_conn` 把长时间的下载算作忙碌，和 fit 一样。响应头之后中途断开的响应体算作该地址的失败，如 fit 的
  `Done(b, true)`，除非是代理取消的（其客户端离开了）：这是 I20 留下的问题。
- `akit-listener`：`Dispatcher` 在 `body_mode` 中路由（在当前图上查找路由，和 `call` 一样；两者之间的重载可能把流式
  请求体交给缓冲的路由，它会看到空的请求体）。handler 以流的方式接收请求体的路由就这样得到请求体，除非请求声明的长度
  超过 `max_body_bytes`，此时和以前一样读取并由 akio 答 413。请求体以流的方式传递的请求在
  `akit_runtime::MAX_BODY_BYTES`（一个 context local，和 `TLS` 一样）中带着这个限制，因为 akio 对流式请求体不设限制。
- `akit-proxy`（`relay.rs`），和 fit 的 `exchange` 一样：
  - 超过 64 KiB 或没有声明长度（分块，或 HTTP/2 和 HTTP/3 没有 `Content-Length`）的请求体以流的方式传递（fit 的
    `StreamThreshold`）；更小的整体到达并整体发往上游。流式请求体按到达时的分帧发往上游：`Content-Length` 或分块
    （没有长度的 HTTP/2 请求体按分块发送，和 fit 的测试期望一致），分块时带上它的 trailer。
  - 上游连接就绪之前到达的部分在 relay 中等待，同时暂停读取客户端。之后每块写进 `RequestBody`；排队超过 64 KiB
    （fit 的 `uploadPending`）时暂停读取客户端（`BodyReads`），直到队列排空（`RequestBody::on_drain`）。增长超过
    `max_body_bytes` 的请求体答 413，并丢弃其上游请求（关闭其连接）。上游在请求体传完之前应答则结束上传；之后到达的
    部分被丢弃，不再暂停读取。
  - 长度在 64 KiB 以内（fit 上游的 `StreamThreshold`）的响应，以及 `HEAD` 的响应或没有 body 的状态，收齐后整体发送；
    其他响应边到达边发送（`stream_response`）。等待客户端的部分超过 64 KiB 时暂停读取上游（`Download::hold_reads`），
    直到客户端取走（`BodyStream::on_drain`）。客户端离开时取消上游响应。中途断开的响应关闭客户端的连接。分块响应体
    之后转发上游的 trailer。
  - 两侧线程之间每个请求一把锁，调用 akio 时从不持有（akio 的调用可能回调）；请求体的句柄交给结束它的一侧独占。
- 测试：`akit-proxy/tests/stream.rs`，移植 fit 的 `TestStreamDownload` 和 `TestStreamUpload`：每个方向 64 MiB 的
  质数长度模式数据，按长度和分块，HTTP/1.1 和 h2c（测试自带一个最小的 h2c 客户端，按要求慢慢读取，只为读过的部分
  归还窗口）；慢的一方每 4 MiB 暂停 20 ms；逐字节检查；进程的常驻内存（`/proc/self/statm`，每 5 ms 采样；计数分配器
  需要 `unsafe`，规则 14）的上升必须小于 16 MiB，在 Linux 上检查。`broken_download`，移植 fit 的
  `TestUpstreamBreaksMidBody`（HTTP/1.1）：客户端的连接在响应体结束之前关闭。`proxy.rs` 的上游也能读取分块请求体，
  因为 `body_limit` 的分块上传现在按分块发往上游。
- 已检查：macOS 上的 `make check`；Linux（gpu51）上用 epoll 和 `AKIO_TEST_BACKEND=uring` 运行
  `cargo test --workspace`（`--no-fail-fast`）：除 io_uring 下
  `akit-listener` 的 `reload.rs` 外全部通过；它大约一半的运行中有一个测试失败（刚重载的监听器上的 GET 被截断，
  `reload.rs:72`），akit 的 `main`（`3631362`，akio `a9a620ab`）在那里失败得一样频繁，所以不是 I16 引入的。流式测试中
  进程常驻内存在任一方向传输 64 MiB 时最多上升 4 MiB，两种后端都是如此。h2c 下载到测试的慢客户端用了 1 到 20 秒，
  HTTP/1.1 不到 1 秒（测试的客户端每次只归还一帧的窗口）；没有进一步测量（规则 30）。
- 与计划和 fit 的差异：
  - 流式发送的 HTTP/1.1 响应按分块发送，去掉 `Content-Length`：akio 还不能按长度为流式 HTTP/1.1 响应分帧（akio API
    需求 A35）。HTTP/2 和 HTTP/3 保留它。整体发送的响应（64 KiB 以内）在所有协议上都保留它。
  - fit 的 splice 上传（`spliceMinBody`，用 splice(2) 从客户端的 socket 到上游的 socket）没有移植：akio 在服务端
    连接和客户端连接之间没有 splice。应由基准测试判断是否值得提一个 akio 需求。
  - 停止读取的客户端会占住其上游直到交换超时（1 小时）：fit 的 `write_stall_timeout` 和 `response_idle_timeout`
    属于 I17，客户端在响应之前离开时取消上游请求、以及声明的 trailer 也属于 I17。
  - 没有长度、中途断开的 HTTP/2 响应以丢弃其流结束（akio 关闭连接），而不是 fit 的 `Fit-Error` trailer（I17，随
    trailer 一起）。
  - h2 上游（`H2Client` 还没有流式接口）属于 I18。
- 给基准测试任务：双向大 body 且对端缓慢时每个进行中的代理请求占用的内存，以及大上传和大下载对比 fit 的吞吐；64 KiB
  水位（fit 的）对比更大的水位；`body_mode` 中第二次路由查找对带请求体的请求的开销。

A19–A21 落地之前（本条目当时为 `doing`）：

- fit 的 body 限制。`akit_config::Limits` 读取 `limits`：`max_body_bytes`（fit 的
  int64，至少为 0；省略或为 0 时 16 MiB），其他字段（`max_pending_bytes`、`max_header_bytes`、`max_conns`、
  `max_write_queue_bytes`、`write_stall_timeout`）逐个拒绝为尚不支持。`limits` 只能由配置的一个文件给出，第二个文件
  中给出时用 fit 的消息拒绝（`adapter.Set.conflict`：“also given in …”）。每个 `Listener` 都带有这个值
  （`Listener::max_body_bytes`），和 fit 的 `runtime.Listener` 一样，因此改变它的重载会改变每个监听器，正如 fit 的
  `binding.same` 比较它（akit 启动时的监听器此时会被拒绝，直到 A1）。
- `akit-listener` 把它设为 akio 在 HTTP/1.1（`HttpConfig::max_body_bytes`）、HTTP/2（`H2Config::max_body_bytes`）
  和 HTTP/3（`H3Config::max_body_bytes`）上的限制：声明长度超过它的请求体在读取前就答 413，分块的请求体在超过时答
  413（HTTP/1.1 上 akio 在 413 之后关闭连接，仍在发送的客户端可能看到连接被重置，fit 的测试也允许这样）。此前 akit
  使用 akio 的默认值：HTTP/1.1 和 HTTP/2 为 1 MiB，HTTP/3 为 8 MiB，fit 接受的请求体（最多 16 MiB）会被拒绝。
- 测试：`akit-config/tests/limits.rs`（3 个）：默认值、每个监听器都带有的值、0、负数或字符串作为错误拒绝、其他字段
  尚不支持、fit 没有的字段是错误、只能由一个文件给出（该文件的值作用于其他文件的监听器）。
  `akit-listener/tests/listeners.rs`：`body_limit`，移植 fit 的 `TestBodyLimit`（限制为 100000 时，1000 和 90000
  字节答 200，150000 答 413；HTTP/1.1 上按长度分帧和分块，以及 h2c），以及 `body_limit_default`（HTTP/1.1 和 h2c
  接受 2 MiB，16 MiB + 1 答 413）。`akit-proxy/tests/proxy.rs`：`body_limit`，移植 fit 的 `TestStreamedBodyLimit`
  （限制为 200000 时，150000 字节完整到达上游，300000 答 413；HTTP/1.1 上按长度和分块，以及 h2c；超过限制的都没有
  到达上游）。兼容性门禁的样例读取 `limits` 和 `limits.max_body_bytes`。
- 已检查：macOS 上的 `make check`；Linux（gpu51）上用 epoll 和 `AKIO_TEST_BACKEND=uring` 运行
  `cargo test -p akit-config -p akit-listener -p akit-proxy`。`make compat-bless`：报告中的 `limits` 行变成每个被
  拒绝的字段一行（`examples/limits`、`examples/proxy`），`examples/body-limit` 不再拒绝 `limits`（仍拒绝
  `routes[].match.method` 和 `routes[].middleware`）；被拒绝的字段 70 个，原为 66 个；其他结果不变。
- HTTP/3 的限制已设置，但只通过共用的代码测试（h3 测试客户端只发 GET）。启动时不打印 `limits` 字段（fit 的 `-v`
  限制行随其余字段一起实现）。

### 2026-11-24：I39，匹配器 v2 与表达式

- 从 M3 提前做：它唯一的依赖 I11 已完成。`akit-router` 有了 fit 的匹配器（`router/matchers.go`），即
  `akit_router::Matcher`：`methods`（按原样比较）、`headers`（名字不区分大小写，字段的值按原样比较；没列出值时任意
  值都行）、`query`（键和值解码后比较，`+` 作空格，不分配内存；无效的转义什么都不匹配）、`remote_ip`（地址和 CIDR
  前缀，`parse_prefix` 同 Go 的 `netip` 加上 fit 的规则：映射到 IPv6 的 IPv4 地址算 IPv4，用 IPv6 写的 IPv4 前缀被
  拒绝，前缀按位数掩码；对端地址先去掉映射）、`path_regexp`（基于 `akio-regex`，决策 001：线性时间，按 Go 的 ASCII
  `\d \w \s \b` 编译；不是 UTF-8 的路径按 Go 正则的读法，每个无效字节当作一个 U+FFFD，经 `is_match_fmt`，不分配内存）
  和 `expr`，即 fit 的表达式语言（`router/expr.go`，`akit_router::Expr`）：`Host`、`Path`、`PathPrefix`、
  `PathRegexp`、`Method`、`Header`、`Query` 和 `RemoteIP` 用 `&&`、`||`、`!` 和括号组合，字符串按 Go 的方式加引号
  （`strconv.Unquote`，含 `\x` 和八进制转义，所以参数是字节），最长 64 KiB，嵌套最深 100 层，错误信息与 fit 相同
  （`at byte N: ...`），打印出的规范形式也相同。`&&` 和 `||` 的链保持扁平，所以求值、打印和释放表达式的递归深度
  只有括号和 `!` 那么深（几千个匹配器的链并不深）。
- 路由器（`Router::find_request`，对 `akit_router::Request` 泛型：方法、host、路径、query、对端地址和头字段）按
  fit 的方式运行路由的匹配器：每个 host 和路径模式下的路由按优劣排序（优先级、host 和路径的具体程度，然后匹配器
  多的，然后先写的路由），在第一个没有匹配器的路由之后截断；只有在路由的 host 和路径匹配、且它能胜过已找到的最佳
  路由时才运行它的匹配器，所以已有更高优先级的路由匹配时，低优先级路由的匹配器不会运行。`Router::find(host, path)`
  保留，相当于没有其他内容的 GET（`Parts::new`）。所有匹配器都在编译配置时构建（规则 12），按 fit 的图的顺序先便宜
  后昂贵（方法、远端 IP、头、query、路径正则、表达式；一个表达式算一个匹配器）。
- `akit-config` 读取 `match.path_regexp`、`method`、`header`、`query`、`remote_ip` 和 `expr`，检查同 fit
  （`config/validate.go`：方法和头名必须是 token，query 键不能为空，正则、前缀和表达式必须能构建，用的是路由器自己的
  构建函数：akit-config 现在依赖 akit-router）。`akit-runtime` 把它们构建进 `CompiledRoute::matchers`，并提供
  `Graph::route_request`；`akit-listener` 的 `Dispatcher` 用它路由，从 akio 的上下文读取方法、`Host`、query、对端
  地址和头字段。
- 测试：移植了 fit 的 `TestMatchers`、`TestMatcherErrors`、`TestMatcherOrder`（其 `panicMatcher` 用例用一个统计
  读取它的匹配器次数的请求来做）、`TestParseExpr`、`TestParseExprErrors`、`TestExprMatch`、`TestQueryHexEscapes`、
  `TestSameBucket`、带匹配器的 `TestPathTwice`，以及 `FuzzParseExpr` 的种子和更多用例作为往返测试（`akit-router`
  `src/tests_matchers.rs`），另有 query 键值对、前缀、ASCII 字符类和非 UTF-8 路径的细节测试；`tests/no_alloc.rs`
  扩展了 fit 的 `TestMatchersAllocs` 和 `TestExprAllocs`（再加一个路径正则）：每项 100 次查找 0 次分配；fit 的配置
  测试 "route matchers"（`akit-config` `tests/json_config.rs`）；`akit-listener` `tests/listeners.rs` 的
  `route_matchers`：每种匹配器都通过 HTTP/1.1 请求测试，query 和方法也通过 h2c 测试。
- 兼容性门禁：`tests/compat/supported.txt` 加了这六个字段；`make compat-bless`：`examples/routing` 现在有效，
  `match.method` / `match.remote_ip` 不再让 body-limit、cache、forward-proxy、sync-wasm-sink 和 video-site 被拒绝
  （80 个夹具：9 个有效，70 个只因暂不支持被拒绝，1 个跳过，没有失败）。
- 已检查：macOS 上 `make check`；基准测试主机上 `cargo test -p akit-router -p akit-config -p akit-runtime -p
  akit-listener`（epoll）和 `AKIO_TEST_BACKEND=uring cargo test -p akit-listener`。
- 与 fit 的不同：路径正则用 `akio-regex` 的语法，是 Go 语法的子集：Unicode 字符类（`\pL`）、POSIX 字符类
  （`[[:alpha:]]`）和 `\Q...\E` 在读取配置时被拒绝，正则错误信息的末尾是 akio-regex 的（`unclosed group at byte 1`），
  不是 Go 的。Go 的 `%q` 引号写法是近似的：Rust 没有 Unicode 类别，所以“可打印”是排除控制字符、U+0020 以外的空白
  和常见格式字符（打印出的形式总能解析回同样的表达式）。错误路径中的 map 键按 akit 的形式写成 `header["Bad Name"]`，
  fit 写成 `header.Bad Name`。与 fit 相同，`Header("Host")` 永远不匹配（Go 的服务器把 `Host` 从头字段中拿掉）。
- 待测（维护者的基准任务，规则 30）：同样路由上带匹配器、表达式和路径正则的 `find_request` 对比 `find`
  （`benches/lookup.rs` 中的微基准），以及 fixed 场景下 I39 对比其基线的 A/B。

### 2026-11-24：I22，SIGHUP 重载

- 之后已改用 akio 的 A1（见上一条）：这里描述的 `adopt_tcp` 临时方案和对启动监听器的拒绝已去掉。
- `akit run` 收到 SIGHUP 时重载：先打印 `akit: reloading <文件>`，重新读取并编译同样的 `-c` 路径，若编译成功且其
  监听器都能提供服务，就换入新图（`GraphCell::swap`），打印移除的监听器（`akit: listener "x" on ADDR removed`）和
  新增的监听器（与启动时同样的行），然后打印 `akit: reloaded <文件>`。读取或编译失败、或监听器无法提供服务的配置，
  保留正在运行的配置：stderr 上先打印 `akit: reload failed; the running config stays`，再按 fit 的方式打印错误
  （与 `akit run` 启动时一样）。新图的上游连接池通过正在运行的引擎拨号。
- 退役：控制线程在每次换入后调用 `GraphCell::collect`，并在 `retiring()` 大于 0 期间每 100 ms 调用一次，其间带超时
  等待信号通道（决策 004）；被替换的图在没有请求再用它时，在这里清理其模块（上游连接池）。
- 监听器（`akit_listener::Listeners`，`crates/akit-listener/src/reload.rs`），按 fit 的 `listener.Server.Reload`
  的方式迁移：新图中名字、地址、协议和 `tls` 都相同的监听器保留，套接字保持打开，从换入起它的请求走新图的路由
  （`Dispatcher` 每个请求按监听器名字读取当前图）。其余的：
  - 新增：采用计划为 A1 指定的临时方案。akit 在换入之前绑定自己的套接字（std 的 `TcpListener`），所以地址被占用或
    akit 无法提供服务的监听器会让重载失败且什么都不改变（为它绑定的套接字会再关闭）；每个监听器一个 accept 线程
    （控制面线程），等到换入之后，把每个连接用 `Engine::adopt_tcp` 交给引擎，用的是与引擎自身监听器相同的 HTTP 或
    TLS acceptor。这些连接运行在引擎的拨号线程上（最后一个启动监听器的线程）。
  - 移除：这样的套接字在换入之后关闭（其 accept 线程停止，由它自己发起的一个连接唤醒）；它已接受的连接保持打开。
    同一地址上被修改的监听器保留其套接字，从换入起按新设置提供服务。
  - 引擎构建时就有的监听器在 A1 落地之前无法关闭：移除或修改这类监听器的重载被拒绝，
    `akit: reload failed; the running config stays: listener "web" on ADDR is removed or changed, which needs a restart: akit cannot close a listener it started with yet (akio API request A1)`。
    fit 会先绑定新的，再排空旧的。
- `-watch` 和 `-watch-interval` 仍然拒绝（属于 I29 的文件 provider）；它们的用法说明现在只写 I29。
- 测试：`akit/tests/reload.rs`，端到端测试：`akit run` 运行时四个客户端线程持续向其监听器发送 keep-alive 请求
  （每个连接 20 个），期间 SIGHUP 修改路由并新增一个监听器、一个损坏的配置、一个移除启动监听器的配置（被拒绝）、
  移除新增的监听器，然后连续 20 次重载交替新增和移除它；客户端看到每个新的响应体，没有任何请求失败，SIGTERM 仍以
  0 退出。`akit-listener/tests/reload.rs`（2 个）：监听器的保留、新增、同一地址上的修改和移除，被移除监听器上的连接
  保持打开、地址可再次使用，旧图由 `collect` 退役；什么都不改变的拒绝（纪元、监听器和响应都与之前相同）：移除或修改
  启动监听器、在绑定另一个新监听器之后遇到被占用的地址、只有 h2c。
- 已检查：macOS 上 clippy（`-D warnings`）以及 akit、akit-listener 和 akit-runtime 的测试；Linux（基准主机）上
  epoll 和 io_uring 的同样测试，全部通过。
- 与计划和 fit 的差异：
  - 重载不能移除或修改启动监听器（A1，见上）；只有重载新增的监听器可以。
  - 保留的 TLS 监听器沿用启动时的证书：其 TLS 配置固定在引擎的 acceptor 中（fit 保留的监听器会采用新图的证书）。
    配置未变而证书变化属于 I30（`-watch-certs`），A2 的 resolver 使证书可以替换。
  - 被移除监听器上打开的连接不会像 fit 那样在旧图上排空：它们保持打开，换入之后它们发出的请求在新图中找不到这个
    名字的监听器，得到 404。
  - 引擎的连接超时（上游 `connect_timeout` 中最短的，A12 的临时方案）在启动时固定：缩短它的重载要重启后才生效。
  - fit 在 `reloaded` 之后打印配置版本；akit 在 Admin API（I26）之前没有版本。
- 留给 A1 的：akit 锁定带有 `Engine::add_listener`、`remove_listener` 和 `shutdown_listener` 的 akio 之后，
  `Listeners::reload` 把监听器加到引擎上，而不是接管它们的连接（像启动监听器一样有自己的线程和加速选择），也能移除并
  排空启动监听器（拒绝随之取消），并像 fit 一样在关闭超时内排空被移除监听器的连接；accept 线程随之去掉。

### 2026-11-24：I10，HTTP/3 入口

- `akit-config` 读取 fit 的 `h3` 协议（`Protocol::H3`），带 fit 的检查 `h3 needs tls`；监听器可以把它与
  `http/1.1`、`h2` 一起列出，也可以单独列出。`tcp`、`udp` 和 `socks` 仍以暂不支持拒绝。
- `akit-tls`：`server_configs` 只读一次监听器的文件，由此生成两份 akio-tls 配置：`tcp`（即 I09 的
  `server_config`，它现在返回这一份），以及服务 `h3` 的监听器的 `h3`：同样的证书、同样按 SNI 选择、同样的客户端
  认证，只用 TLS 1.3，ALPN 只有 `h3`（fib 的 `http3.ConfigureTLS` 把 `h3` 放在最前，后面保留 TCP 的协议，HTTP/3
  客户端不会请求它们）。图保留两者（`Graph::tls`、
  `Graph::h3_tls`），因此 `akit validate` 照旧检查它们。
- `akit-listener`：服务 `h3` 的监听器在其 TCP 一侧所用的端口上再加一个基于 akio-h3 的 UDP 监听器
  （`akio_h3::serve_with`，用图中 `h3` TLS 经 `akio_quic::server_config_from` 生成的配置），服务同一个
  `Dispatcher`（按 akio-h3 作为 `Host` 给出的 `:authority` 路由，规范化路径，400 和 404 与 TCP 上相同）。它的 TCP
  dispatcher 给每个响应（包括 400 和 404）加上 fib 的 `http3.AltSvc` 值 `alt-svc: h3=":<端口>"; ma=86400`
  （`Context::add_response_header`；该值在建立监听器时格式化一次）。端口为 0 且同时服务 TCP 和 h3 的监听器先找一个
  两者都空闲的端口（UDP 同号端口也能绑定的 TCP 端口），对应 fit 在 TCP 得到的端口上绑定 UDP；若其间端口被占用，
  就换一个端口重新构建引擎，最多 8 次（fit 的 `maxPortZeroBinds`）。`akit_listener::addrs` 给出每个监听器监听的地址
  （`tcp`、`udp`）；`akit run` 打印 TCP 的地址，只服务 h3 的监听器打印 UDP 的地址，与 fit 打印 `Addr` 相同。
- akio：`akio-h3` 和 `akio-quic` 是新的工作区依赖，在锁定的提交上；没有缺少的东西，没有新的 API 需求。
- 兼容性门禁：`listeners[].protocols` 接受 `h3`（`supported.txt` 中注明；没有新字段）。`make compat-bless`：
  `examples/http3` 不再因 `h3` 协议被拒绝（被拒字段 60 个，原为 61）；它仍因 `acme`、`tls.acme`、代理和中间件被拒绝。
  `docs/compatibility.md` 的示例行改用 `udp` 协议。
- 测试：`akit-config`（h3 与 TCP 协议一起及单独读取，`h3 needs tls`，重复列出）；`akit-tls`（`h3` 配置的 ALPN，没有
  h3 时没有它，问题的路径相同）；`akit-listener/tests/h3.rs`（6 个：经 h3 与经 h2、HTTP/1.1 访问同一路由，`Alt-Svc`
  指向 UDP 端口即 TCP 端口，404 上也有，h3 自身没有，经 h3 按 host 路由、按 SNI 选择证书，经 h3 的规范化路径和 404，
  未知名字被拒绝；只有 h3 时只监听 UDP；没有 h3 时没有 `Alt-Svc` 也没有 UDP；固定端口；经 h3 的客户端认证 require 和
  verify_if_given；非 QUIC 的数据报不影响其他客户端）；`akit/tests/h3.rs`（用 `akit run` 端到端：经 h3 访问路由，h2
  和 HTTP/1.1 上的 `Alt-Svc`）。I10 的完成标准由 `the_same_route_over_h3_and_alt_svc_on_h2` 和 `akit` 的测试满足。
- 与计划的不同：除 `akit-listener` 外还涉及 `akit-config`、`akit-tls`、`akit-runtime` 和 `akit`。HTTP/3 的限制用
  akio-h3 的默认值（8 MiB 请求体、64 KiB 字段段、QUIC 的空闲超时和 10,000 个连接），与 TCP 监听器用 akio-http 的默认值
  一样，直到 akit 读取 fit 的监听器限制（`max_header_bytes`、`max_body_bytes`；fit 的 HTTP/3 取监听器的值，默认 1 MiB
  和 16 MiB）。自己设置了 `Alt-Svc` 的响应（`respond` 路由的 `headers`）保留自己的值，而 fit 的钩子会覆盖它：
  akio-http 的 `add_response_header` 让位于响应自己的字段，而响应过滤器每个请求要多一个装箱闭包。UDP 监听器无论 TCP
  用什么后端，都运行在 akio 的按对端 UDP 循环上（epoll，macOS 上为 kqueue）。端口 0 在 akio 绑定之前探测，而不是先绑定
  TCP（akio 在构建引擎时绑定所有监听器）。
- 已检查：macOS 上的 `make check`；改动的 crate（akit-config、akit-tls、akit-runtime、akit-listener、akit、
  akit-compat）在 Linux（gpu51）上的 epoll 和 io_uring 测试，全部通过。
- 留给基准测试任务：HTTP/3 与 fit 的对比（固定 `respond` 路由上的每秒请求数和延迟，用 h2load 或 fitbench 的 h3
  客户端；压测框架还没有 akit 的 h3 场景）、每秒新建 QUIC 握手数，以及 `Alt-Svc` 字段在 HTTP/1.1 和 h2 热路径上的
  开销（有无 `h3` 的监听器的同轮 A/B）。

### 2026-11-24：akio 锁定到 `a9a620ab`；I15 完成

- akio 锁定从 `50a82315` 移到 akio `main` 上的 `a9a620ab`：A1–A5、A7 和 A8（2026-11-24 合并）、A11（I15 的
  `Host`）、A13 和 A16–A18（I13 的 `Files`）。所有 akio crate 随之移动，`akit_runtime::AKIO_REV` 也一样。
- I15 为 `done`：它的三个 A11 测试（`forwarding`、`request_fields_in_order`、`host_header`）不再忽略，并且通过。
- 已检查：macOS 上的 `make check`；基准主机上 epoll 和 io_uring 下整个 workspace 的测试。

### 2026-11-24：I15，反向代理 I：请求与头处理（曾为 doing：等待 akio A11）

- 状态为 `doing` 而非 `done`：代理已实现，fit 的头处理测试已移植，但其中三个（上游看到的 `Host`，以及不凭空添加
  `User-Agent`）只有在 akio 的 A11 上才能通过，而 A11 在 akio 的独立分支上，尚未合并：akio `claude/i15-proxy-699f18`，
  提交 `552fe283`（`ClientRequest::headers` 自带的 `Host` 代替 URL 的发出；空的 `User-Agent` 表示不发送）。在锁定的
  akio（`50a82315`）上，akio 的客户端丢弃请求自带的 `Host`（改发上游的地址），并把空的 `User-Agent` 原样发出；这三个
  测试标为 `#[ignore]`，注明 A11。A11 进入 akio 的 `main`、锁定版本移过去、这三个测试不再忽略之后，I15 即为 `done`。
- `akit_config::Proxy`：`proxy` handler 的参数，检查和消息与 fit 相同：`upstream`；`trusted_proxies`，地址和网段按
  fit 的 `parsePrefix` 解析（`akit_config::Network`：单个地址是只含它的网段，IPv4 映射地址按 IPv4 处理，清除主机位，
  前缀长度按 Go 的语法），错误为 `trusted_proxies[i]` 处的 `"nope" is not an IP address or a network, ...`；
  `host_header`，`"a b" is not a host`；两者都没写时为 `upstream or upstreams is needed`。`upstream` 是否指向配置中的
  某个上游在 handler 准备时检查（`params.upstream` 处的 `there is no upstream named "nope"`），此时各文件的上游已合并，
  与 fit 的检查时机相同。`upstreams`、`failover`、`mirror`、`retry`、`timeout`、`tunnel_idle_timeout` 和
  `splice_uploads` 报为暂不支持。
- `akit-proxy`（`MODULES`，`http.handlers.proxy`，已加入 `akit` 的注册表）：每个请求在 I/O 线程上构造发给上游的请求
  （方法、目标、字段、请求体），分离上下文，把请求交给上游的连接池（I14）；连接池的完成回调通过分离的上下文作答。
  全程不阻塞（规则 1–3）；已退役图的连接池会排空进行中的请求，因此不需要持有图。失败按 fit 的 `statusOf` 作答：
  连接池已满或已关闭为 `503 Service Unavailable`，超时为 `504 Gateway Timeout`，其他为 `502 Bad Gateway`；字段名
  不是 token 时为 400。
- 字段处理（`headers.rs`），对应 fit 的 `outgoingFramed` 和 `responseBlock`：请求字段按到达时的顺序和大小写转发，
  `Host` 在最前（客户端的，或 `host_header`）；去掉逐跳字段（`Connection` 及其列出的字段、`Keep-Alive`、
  `Proxy-Connection`、`Proxy-Authenticate`、`Proxy-Authorization`、`TE`、`Trailer`、`Transfer-Encoding`、
  `Upgrade`）、`Content-Length` 和 `Expect`（由 akio 为请求体分帧）以及伪字段；`X-Forwarded-For`、
  `X-Forwarded-Proto`、`X-Forwarded-Host` 和 `Forwarded`（RFC 7239）由代理自己设置，放在客户端字段之后；若客户端
  地址在 `trusted_proxies` 中，则把它追加到其发来的 `X-Forwarded-For` 和 `Forwarded`，并保留其
  `X-Forwarded-Proto` 和 `-Host`。客户端没有发送的 `User-Agent` 不会被添加。响应字段按上游的顺序和大小写返回，去掉
  逐跳字段、其 `Connection` 列出的字段和 `Content-Length`（由 akio 重新分帧），HEAD 的响应除外，保留上游的
  `Content-Length`。
- 发出的目标是路由匹配时的规范化路径（akit-listener 的 `CANONICAL_PATH`，现定义在 akit-runtime）加上原样的查询串，
  这样为绕过路由而拼写的路径以路由看到的形式到达上游；绝对形式的目标以 origin 形式发出。TLS 监听器上的请求
  `X-Forwarded-Proto` 为 `https`：akit-listener 的分发器为这类请求设置上下文本地值 `akit_runtime::TLS`（akio 的上下文
  尚不提供请求是否经 TLS 到达）。
- 测试：移植了 fit 的头处理测试（`akit-proxy/tests/proxy.rs`，经 HTTP/1.1 和 h2c 到一个基于 std 套接字、按到达原样
  报告请求头的 HTTP/1.1 上游）：`TestForwarding`、`TestHopByHopRequest`、`TestHopByHopResponse`、
  `TestForwardedUntrusted`、`TestForwardedTrusted`、`TestHostHeader` 与 `TestFramedHostHeader`、`TestBadGateway`、
  `TestManyRequests`（同时 64 个）、`TestHead`、`TestConfigErrors`、`TestFramedRequestHeader`、
  `TestFramedResponseHeader`、`TestFramedResponseConnectionFields`，以及发出的规范化路径；单元测试（`headers.rs`）：
  `TestFramedFieldNames`、`TestForwardedElement`、`TestRefusedHeaderName`，以及各类请求的字段；`tests/tls.rs`：TLS
  监听器后的 `https`；`akit-config/tests/proxy.rs`：参数、fit 的消息、暂不支持的字段、网段。未移植：
  `TestFramedRequestReadAgain` 和 `TestFramedRequestHeaderMiddleware`（fit 用 map 构造的请求和中间件，akit 还没有：
  I40），`TestForwardedNotIP`（akit 的对端总是 IP 地址；`forwarded_element` 覆盖了 `for=unknown`）。
- 兼容性门禁：支持 5 个代理字段（共 42 个）；8 个夹具有效（新增三个 `conf.d` 模板），71 个只因暂不支持被拒绝
  （`make compat-bless`，已审阅其差异）。报告中有一行是代理的 `timeout` 被记为
  `examples/load-balancing: ... handler<fastcgi>.params.timeout`：akit-config 的错误路径与 fit 一样不写模块名，门禁
  把 fastcgi 和 proxy 都有的参数映射到第一个；门禁判断该夹具只因暂不支持被拒绝，这一点是对的。
- 基准配置（`bench/configs/akit/fit.json`、`bench/fitbench/akit.go`）：fit 的 `proxy` 路由（`/proxy`、`/proxy/*`、
  `/api/*`）指向 fitbench 的后端，连接池设置与 fit 相同，并加入 `proxy` 场景；fit 的 `timeout 60s` 和 `max_fails 0`
  akit 还没有（I17、I20）。未运行基准测试（规则 30）。
- 已检查：macOS 上锁定的 akio 下的 `make check`（三个 A11 测试被忽略）；通过命令行给出的 `[patch]`（从不提交）对 akio
  的 A11 分支运行 akit-proxy 的测试，包括被忽略的；Linux（基准主机）上 epoll 和 io_uring：A11 分支上 akio-http 的
  测试，以及基于它的 akit-config、akit-runtime、akit-listener、akit-handler、akit-proxy、akit 和 akit-compat 的
  测试，包括被忽略的。
- 与计划的不同：
  - 请求体整体传递（akio 的缓冲请求体，复制进发给上游的请求）：流式传输是 I16。fit 的 64KB 阈值、splice 上传和
    trailer 属于 I16 和 I17。
  - `User-Agent` 规则同样需要 A11（空的 `User-Agent` 表示不发送）：并入 A11，而不是另提需求。
  - 已知的偶发失败，与 I15 无关：在负载较高的 `make check` 中，`akit-upstream` 的 `max_conns_and_max_pending`
    有时看到 `in_flight() == 1`，因为连接池先调用请求的回调、再把它计为完成；该测试应等待计数归零（或连接池先计数）。
- 基准任务应测量的内容：A11 进入锁定版本后，HTTP/1.1、h2c、HTTPS/1.1 和 h2 上的 `proxy` 场景（fitbench
  `-scenarios proxy`），与 fit、nginx、Caddy 和 Traefik 对比；代理对每个请求新建字段列表、复制请求体和响应字段，
  TLS 请求还会设置一个上下文本地值（一次分配）：与 fit 写入池化缓冲区的 `outgoingFramed` 对比做性能剖析。

### 2026-11-24：I19，负载均衡 v1

- 配置：读取 `upstreams[].weights`（每个 1 到 1000，每个地址一个）和 `upstreams[].load_balancing`
  （`round_robin`、`weighted_round_robin`、`least_conn`）到 `akit_config::Upstream`（`weights`、
  `load_balancing`、`weight(i)`），默认值与 fit 相同（轮询；有地址权重不为 1 时为加权轮询），检查和消息也与 fit
  相同：每个权重的范围、权重与地址数量相同、`round_robin` 不带权重、策略是 fit 的九种之一。fit 的其余六种策略
  （`random`、`p2c` 和各种哈希）以及 `hash_key` 报为暂不支持（I36）。
- `akit_upstream::balance::Balancer`，位于 `Pool::pick` 之后（签名不变；连接池新增 `load_balancing()` 和
  `weight(i)`）：轮询；nginx 的平滑加权轮询，一轮预先算好（权重除以最大公约数，最多 65536 项，同 fit 的
  `maxSchedule`），每次选择取一项；最少连接，选按权重计进行中请求最少的地址（忙碌加等待，即连接池为 `max_conns`
  按地址维护的计数），比较时不做除法，平局从每次选择都前移的起点开始找。
- 每个 I/O 线程一份状态（规则 13）：fit 为每个上游保存一个所有 goroutine 共享的轮次；akit 给每个均衡器每核一个
  独占缓存行的槽位（并行度向上取整到 2 的幂），每个线程从第一次选择时分到的槽位取轮次。每个线程看到的序列与 fit
  完全相同，无论哪些线程共用槽位，份额在整轮上都精确；各槽位从一轮中不同的位置开始，避免所有线程的第一个请求都落到
  第一个地址。必须共享的只读不写：最少连接读取每个地址的计数。一轮太长无法预先算出的加权轮询，在每个槽位自己的
  当前权重上运行 nginx 的算法，持有的是该槽位自己的锁（没有全局锁）。选择不分配内存。
- 测试：`akit-upstream/tests/balance.rs`（9 个），移植自 fit 的 `balance_test.go`：轮询和平滑加权轮询的序列（因
  各槽位起点不同，按 fit 序列的轮转检查）和精确份额、有公约数的权重生成的调度、权重太多无法预先调度时（按权重精确）、
  8 个线程同时选择（每种策略份额精确）、最少连接的空闲、忙碌和加权情况、单个地址；连接池遵循配置，以及最少连接读取
  两个真实上游上被挂起的请求（权重 2 和 1 时为 6 和 3）。`akit-config/tests/upstreams.rs`：`load_balancing`
  （默认值、三种策略、fit 的检查和消息、暂不支持的策略）。门禁为两个字段加了样例。
- 已检查：macOS 上 `make check`；Linux（基准测试主机）上 akit-config、akit-upstream（epoll 和 io_uring）和
  akit-compat 的测试，全部通过。fit 的消息及其行列号，用 `fit validate` 对同样的文件核对过。门禁
  （`make compat-bless`）：`upstreams` 38 个字段中支持 11 个（共 39 个）；`examples/load-balancing` 不再因
  `weights` 和 `load_balancing` 被拒绝，`examples/l4-udp` 指出缺少的是 `ip_hash` 策略。
- 与计划和 fit 的不同：
  - 轮次按线程而不是按上游：所有线程合起来看，轮询在每个线程的整轮上公平，而不是严格依次，所以不同线程上的两个请求
    可能同时发往同一个地址。fit 单 goroutine 下的序列在每个线程内成立。
  - fit 的被动和主动健康状态（跳过已下线的地址，`TestWeightedRoundRobinDown`）没有移植：那是 I20。fit 的分配测试
    （`TestPickAllocs`）也没有移植（计数分配器需要 `unsafe`）；选择按构造不分配内存。
  - 最少连接从连接池接收请求（`send`）时开始计数，而不是从 fit 的 `Pick`：像 `Pool::request` 那样选择后立即发送
    的调用方看到的结果相同。
- 基准测试任务应测量：各策略下 `Pool::pick` 在 1 核和全部核上的表现（每线程槽位对比单个共享计数器），以及按各策略
  在多个上游地址上运行代理，与 fit 对比，并检查负载下的份额。

### 2026-11-24：I12，静态文件 I

- 基于 akio-http `Files` 的 `static` handler（`akit-handler`，`static_files.rs`）。`Files` 负责提供：`ETag` 和
  `Last-Modified`，`If-Match`、`If-None-Match`、`If-Modified-Since`、`If-Unmodified-Since`，带 `If-Range` 的范围请求
  （多个范围用 `multipart/byteranges`），带长度的 `HEAD`，按扩展名的 `Content-Type`，目录的索引文件，不超过
  `max_file_bytes` 的文件放在内存中、更大的从磁盘发送（从保留的句柄 `sendfile`）。只有它在信任窗口内已知的文件才在
  I/O 线程上应答；每次 `stat`、`open` 和读取都在 akio 的文件池上运行（16 个线程、4096 个排队，超出时答 503 并带
  `Retry-After`），通过分离的 context 从那里应答（规则 1 和 4）。在 Linux 上、`check_interval` 不为 0 时，它像 fit
  一样通过 inotify 跟踪变化；其他系统上它在 `check_interval` 内信任已知的内容；为 0 时每个请求都查看磁盘。
- akit 在 `Files` 之前按 fit 的 handler 加上的检查：只接受 `GET` 和 `HEAD`（405，带 `Allow: GET, HEAD`）；带 `..`
  段、NUL 或反斜杠的路径，无论是否转义，答 400（fit 的 `dotfile.Scan`，边扫描边解码，不分配内存）；`dotfiles` 为
  默认的 `deny` 时，以点开头的段答 404，但根目录下的 `/.well-known/` 除外。它的简短应答与 fit 相同
  （`"404 Not Found\n"` 等），只构建一次。
- 配置：`root`（必填；在 provisioning 时检查它是目录，并转为绝对路径）、`index`（一个名字；fit 的检查：是文件名，
  `dotfiles` 为 `deny` 时不以点开头）、`max_file_bytes`（1 MiB）、`dotfiles`、`disk_io`（`pool`；在工作线程上访问
  磁盘的 `auto` 报为暂不支持）、`check_interval`（5s）。报为暂不支持的：`strip_prefix`（A13）、`max_cache_bytes`
  （A15）、`default_type` 和不止或少于一个名字的 `index` 列表（A14）；`precompressed`、`compress`、`try_files`、
  `browse` 和 `browse_max_entries`（I13）。兼容性门禁：支持 8 个 static 字段（共 45 个）；fit 使用 `static` 的示例
  现在只因这些字段被拒绝（`make compat-bless`）。
- 基准配置像 fit 一样从挂载的 `/srv/www` 提供 fit 的 `static` 和 `site` 路由，`static` 加入了 akit 的 fitbench
  场景（`bench/configs/akit/fit.json`、`bench/fitbench/akit.go`、bench README）。没有运行基准测试（规则 30）。
- 测试：`akit-handler/tests/static_files.rs`（11 个），把 fit 的 `static_test.go` 移植为经过 akit 监听器的测试：
  提供文件（类型、索引、404、405、HEAD，每个两次以覆盖缓存）、索引文件、路径穿越（12 种尝试，转义与否，由监听器或
  handler 答 400，dotfiles 与 `.well-known`，`dotfiles` 为 `allow`）、来自内存和磁盘的条件请求与范围请求、从磁盘
  发送的大文件、`check_interval` 为 0 时被修改和删除的文件、配置错误与暂不支持的内容、同一根目录上 fit 的基准路由；
  另外：卡住的磁盘（没有写者的 FIFO）只占用一个文件池线程，其他连接上的 16 个请求照常从内存应答；4 MiB 文件发给慢
  读者和中途离开的客户端；流水线请求按顺序应答。路径扫描的单元测试（穿越、dotfiles、普通路径）。
- 已检查：macOS 上的 `make check`；Linux（基准主机）上 akit-handler、akit-config、akit-listener、akit-runtime 和
  akit-compat 的测试，epoll 和 io_uring，全部通过。
- 与计划和 fit 的不同之处，直到 akio 的 `Files` 有了 A13–A15 所要求的功能：
  - A13：handler 按发送的原始路径查找，而不是监听器的规范路径，因此仍在根目录内的 `..`（`/x/../ok.txt`）答 400，
    而 fit 的整套服务会提供 `/ok.txt`；`strip_prefix` 被拒绝。
  - A14：不带斜杠的目录直接提供，而不是重定向（301）；带结尾斜杠的文件被提供，而不是 404；即使没有设置
    `precompressed`，`.br`、`.zst` 或 `.gz` 副本也会提供给接受它的客户端，并且总是发送 `Vary: Accept-Encoding`；类型
    来自 `Files` 的 22 个扩展名的表，其余为 `application/octet-stream`，不嗅探；不允许读取的文件答 404 而不是 403；
    根目录下的 FIFO 或设备会被打开；带 `:` 的名字答 404；`Files` 自己的 404 写 `Not Found`。移植的测试检查 akit
    目前的回答并注明该需求。
  - A15：`Files` 占用的内存按单个文件（`max_file_bytes`）有界，但总量无界；在其磁盘池中等待的请求没有超时（池的
    队列有界，超出时答 503）。
  - `Files` 会合并相距不到 80 字节的范围，并对从磁盘发送的文件的多个范围用 multipart 应答，而 fit 会发送整个文件；
    它的 `ETag` 由大小和修改时间的秒组成，fit 的由大小和纳秒组成。在 macOS 上，fit 对大小写与磁盘上名字不同的路径
    答 404；`Files` 会提供它。
  - 文件池是 `Files` 自己的 akio 池，而不是引擎的逻辑池：`Files` 把它的磁盘工作放在那里。
- 给基准任务：HTTP/1.1、h2c、HTTPS/1.1 和 h2 上的 `static` 场景（fitbench 的 1 KiB `small.html`，来自内存），
  对比 fit、nginx 和 Caddy；`download` 和 `site`（来自磁盘的文件、`sendfile`、文件池），在它们的路由和协议加入
  akit 的场景之后；每个请求中路径扫描和 `Files` 分片读锁（命中时）的开销。

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
- 同一轮 A/B（`bench/ab.sh 8a7a8b8 ebcfe02`，`ab-20261124-015157`：I11 的基点对比 I11，fixed 场景，3 轮、
  每轮 10 秒）：HTTP/1.1 每秒 825k 对 824k 请求（-0.1%），h2c 5979k 对 5927k（-0.9%），每请求 CPU 相差 3% 以内，p99 相同：
  都在噪声范围内，符合预期，因为单条无 host 路由的查找只占请求耗时（微秒级）中的几纳秒。第一次尝试
  （`ab-20261124-010304`）因基准测试主机磁盘已满而失败；第二次发现 `bench/ab.sh` 可能用 A 的 crate 构建 B
  （git archive 的提交时间 mtime 加上共享的 target 目录），已在 `bench/lib.sh` 中改为以当前时间解包修复。
- 待测量（由维护者的基准任务进行；规则 30：功能任务不跑基准）：路由查找 v0 对比 v1
  （`cargo bench -p akit-router --bench lookup`），以及 I11 对比其基点的 fixed 场景 A/B。上面的数字是在规则 30
  之前测得的，在这些任务重测之前视为暂定。
- 已检查：macOS 上的 `make check`；基准测试主机上 Linux（epoll）的
  `cargo test -p akit-router -p akit-runtime -p akit-listener`，以及 `AKIO_TEST_BACKEND=uring cargo test -p akit-listener`。
- 与计划和 fit 的不同：没有 matcher（I39）。错误的转义（`%zz`）回答 `400 bad request path`；fit 的 Go 服务器
  在 fit 看到之前就用它自己的 400 拒绝。akio 的请求目标无法改写，所以规范路径通过上下文局部值
  `akit_listener::CANONICAL_PATH` 交给 handler，只在它与发来的路径不同时设置；I12 和 I15 读取它（或者 akio
  增加改写目标的方法）。字面的 `\` 在规范路径中保持原样，而 Go 会写成 `%5C`。绝对形式的请求
  （`GET http://host/path`）仍按 akio 的 `path()`（整个 URI）路由，与 I11 之前相同。

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
