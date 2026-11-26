# akit 需要 akio 提供的 API

[English](akio-api-requests.md) | [简体中文](akio-api-requests.zh-CN.md)

本文列出 akit 需要 [akio](https://github.com/lesismal/akio) 提供的东西、原因，以及 akit 打算怎么用
（[规则 21](rules.zh-CN.md#8-akio-不足时)）。文中的名字和签名只是建议，最终形式由 akio 决定。akit 的开发会话可以
自己在 akio 中实现，并遵守 akio 的规则。

这第一份清单来自对照 fit 向 fib 提出的需求
（[fib API 需求](https://github.com/lesismal/fit/blob/main/docs/fib-api-requests.zh-CN.md)）阅读 akio `1.0.0-rc.1`
（提交 `342e14d3`）。每一项都要由第一个需要它的迭代先对照 akio 的代码确认，再修改 akio：某个 API 可能已经以别的名字
存在。

2026-11-23，用户决定 A1–A8 现在就全部加进 akio，而不是等 akit 的迭代用到时再加。每项都在 akio 的独立分支上
实现，动手前先对照 akio 的代码确认；这里的状态先记分支，再记 akit 锁定的 akio 提交。

## 概览

| ID | API | 状态 | 解决的规则 | akit 迭代 |
| --- | --- | --- | --- | --- |
| A1 | 在运行中的引擎上添加、移除和排空 TCP 监听器 | 已在 akio 的 main（`a9a620ab`），已锁定，I22 已使用 | 10、11 | I22、I28 |
| A2 | 每次握手由回调选择服务端证书，回调可以稍后完成 | 已在 akio 的 main（`a9a620ab`），已锁定 | 1、4 | I30、I34 |
| A3 | 在任何 handler 运行之前过滤已接受的连接 | 已在 akio 的 main（`a9a620ab`），已锁定，由 I85 使用 | 11 | I85–I87 |
| A4 | PROXY 协议 v1/v2：接受时解析，拨号时写出 | 已在 akio 的 main（`a9a620ab`），已锁定，I52 已使用 | 12 | I52 |
| A5 | akio-mux 中按 SNI 路由 TLS 而不终结它的规则 | 已在 akio 的 main（`a9a620ab`），已锁定，I52 已使用其 ClientHello 解析器 | 12 | I52、I53 |
| A6 | `akio-x509` 中生成 CSR（以及 `akio-crypto` 中的 JWS 辅助函数） | 已在 akio 的 main（`afbbb521`），已锁定 | 15 | I31 |
| A7 | 不转码即可切分文件的 MP4（ISO BMFF）读取器 | 已在 akio 的 main（`a9a620ab`），已锁定 | — | 媒体库 |
| A8 | TUN 设备 | 已在 akio 的 main（`a9a620ab`），已锁定 | — | VPN |
| A9 | 把 TLS 服务端限制在 TLS 1.2（`ServerConfigBuilder` 的公开 API） | 已在 akio 的 main（`4989cd8b`），已锁定 | 17 | I09 |
| A10 | 立即关闭 HTTP 客户端的空闲连接 | 已进入 akio 的 main 并锁定（自 `0fc8f3a7` 起） | 10、11 | I14、I22、I38 |
| A11 | HTTP/1 客户端请求保留自己的 `Host` 字段（给定空 `User-Agent` 时不发送 `User-Agent`） | 已在 akio 的 main（`a9a620ab`），已锁定 | — | I15 |
| A12 | HTTP 客户端的连接超时也作用于单个地址 | 已进入 akio 的 main 并锁定（自 `6bcd8ad6` 起，akio 的 ADR-154） | 10 | I14、I35 |
| A13 | `Files` 服务调用者给出的路径 | 已在 akio 的 main（`a9a620ab`），已锁定 | 19 | I12、I13 |
| A14 | `Files` 像网关的静态文件 handler 那样应答 | 已在 akio 的 main（`4989cd8b`），已锁定 | 19 | I12、I13 |
| A15 | `Files` 的内存有界，磁盘池有超时 | 已在 akio 的 main（`4989cd8b`），已锁定 | 4、11 | I12 |
| A16 | `Files` 在配置的编码中按客户端的 q 值选择压缩副本 | 已在 akio 的 main（`a9a620ab`），已锁定 | 19 | I13 |
| A17 | `Files` 依次尝试候选路径（`try_files`） | 已在 akio 的 main（`a9a620ab`），已锁定 | 19 | I13 |
| A18 | `Files` 列出没有索引文件的目录 | 已在 akio 的 main（`a9a620ab`），已锁定 | 4、11、19 | I13 |
| A19 | HTTP 客户端请求的请求体边产生边发送，带背压 | 已在 akio 的 main（`60547435`），已固定，I16 已使用 | 5、8 | I16、I17、I18、I49、I75 |
| A20 | HTTP 客户端响应边到达边交付，可暂停和恢复读取 | 已在 akio 的 main（`60547435`），已固定，I16 已使用 | 5、8 | I16、I17、I18、I67 |
| A21 | 服务端请求可暂停其客户端的读取，并在客户端排空时得到通知 | 已在 akio 的 main（`60547435`），已固定，I16 已使用 | 5、8 | I16、I47、I75 |
| A22 | 在运行中的引擎上移除和排空 UDP 监听器 | 已进入 akio 的 main 并锁定（自 `6bcd8ad6` 起，akio 的 ADR-155） | 10、11 | I22、I28 |
| A23 | Unix 域流式监听器（可设套接字文件权限） | 已在 akio 的 main（`4989cd8b`），已锁定 | 11 | I26、I29 |
| A24 | 关闭连接的 HTTP/1 响应，并停止读取已拒绝的请求体 | 已进入 akio 的 main 并锁定（自 `6bcd8ad6` 起，akio 的 ADR-157） | 7、10、11 | I26、I27、I41 |
| A29 | 不分配内存的应答钩子：监听器的观察者，带着请求设下的几个字被调用 | 已进入 akio 的 main 并锁定（自 `6bcd8ad6` 起，akio 的 ADR-158） | 13 | I21、I37、I64 |
| A30 | 监听器接受的打开连接数，与拨出的连接和 UDP 对端分开 | 已进入 akio 的 main 并锁定（自 `6bcd8ad6` 起） | — | I37 |
| A32 | `akio-regex` 模式在哪里匹配、它的分组，以及替换 | 已在 akio 的 main（`e9891b52`），已锁定 | 19 | I40、I48 |
| A33 | 被中间件修改的请求字段和请求目标（`Context::req_mut`） | 已在 akio 的 main（`dad77a7a`），已锁定 | 12、19 | I40、I43、I44、I71 |
| A35 | 按长度分帧的流式 HTTP/1.1 响应 | 已进入 akio 的 main 并锁定（自 `6bcd8ad6` 起，akio 的 ADR-157） | 5 | I16、I67 |
| A36 | HTTP 客户端请求的响应头超时和空闲超时 | 已在 akio 的 main（`4f7169c4`），已锁定，I17b 已使用 | 10 | I17b、I18 |
| A37 | 可在任意时刻取消的 HTTP 客户端请求 | 已在 akio 的 main（`4f7169c4`），已锁定，I17b 已使用 | 3 | I17b、I45 |
| A38 | 能得知客户端离开的分离服务端请求；可在运行时修改的写停滞超时 | 已在 akio 的 main（`4f7169c4`），已锁定，I17b 已使用 | 3、10 | I17b、I49 |
| A45 | 公开的定时器：延迟过后运行一次回调，可从任何线程调度 | 已进入 akio 的 main 并锁定（自 `6bcd8ad6` 起，akio 的 ADR-154） | 1、11 | I35、I31、I52、I54 |
| A48 | 获知请求的结束：响应已完整发出，或等待中客户端已离开 | 已在 akio 的 main（`dad77a7a`），已锁定 | 3、11、13 | I41、I17、I21、I37 |
| A49 | 由中间件限制请求体大小，无论谁读取请求体 | 已进入 akio 的 main 并锁定（自 `6bcd8ad6` 起，akio 的 ADR-157） | 7、11 | I41 |
| A51 | `H2Client` 的流式请求：重置、超时、请求 trailer，以及遇到 `GOAWAY` 时报告失败而不是被遗忘 | 已在 akio 的 main（`f6750e93`，ADR-110），已锁定，I18b 已使用 | 3、10 | I18b、I50 |
| A52 | `H2Client` 按流的背压：暂停流式响应的读取，报告上传队列 | 已在 akio 的 main（`f6750e93`，ADR-110），已锁定，I18b 已使用 | 8 | I18b、I50 |
| A53 | `H2Client` 的连接：遵守并报告服务器的流数上限、连接超时、ALPN 回退到 HTTP/1.1 | 已在 akio 的 main（`f6750e93`，ADR-110），已锁定，I18b 已使用 | 10、11 | I18b、I54 |
| A54 | 空闲的 HTTP/1 连接在宽限期后排空，而非立即关闭 | 已进入 akio 的 main 并锁定（自 `6bcd8ad6` 起） | 10 | I28 |
| A57 | 握手之外可用的根证书：用系统的根证书校验证书链 | 已在 akio 的 main（`4989cd8b`），已锁定 | 1、19 | I32 |
| A58 | HTTP 服务能看到 TLS 连接协商出的 ALPN 协议（或关闭这样的连接） | 已在 akio 的 main（`4989cd8b`），已锁定 | 19 | I32、I34 |
| A63 | `akio-crypto` 中的 bcrypt 密码哈希 | 已在 akio 的 main（`4f7169c4`），已锁定 | 1、4 | I42 |
| A64 | 每个服务端请求上的客户端已验证证书 | 已在 akio 的 main（`4f7169c4`），已锁定 | — | I42 |
| A65 | 由中间件挂起、之后带着上下文恢复的请求 | 已在 akio 的 main（`4f7169c4`），已锁定 | 1、3、4 | I42、I43 |
| A66 | `H2Client` 的请求遵守 `response_header_timeout`、`response_idle_timeout` 和 `CancelToken` | 已在 akio 的 main（`d27e6a20`，ADR-111），已锁定，I18b 已使用 | 3、10 | I17b、I18b |
| A75 | 挂起消息体流式读取的请求，并暂停其消息体的读取直到恢复 | 已进入 akio 的 main 并锁定（自 `6bcd8ad6` 起，akio 的 ADR-159） | 5、7、8 | I42、I43、I44 |
| A78 | 每个连接（TCP 或 UDP 对端）各自的空闲超时，由其处理器设置 | 已进入 akio 的 main 并锁定（自 `6bcd8ad6` 起，akio 的 ADR-155） | 10、11 | I51、I55 |
| A79 | 带各自连接超时的 `Engine::dial` 与 `dial_udp` | 已进入 akio 的 main 并锁定（自 `6bcd8ad6` 起，akio 的 ADR-154） | 10 | I51、I54、I55 |
| A90 | 由调用方自己的响应过滤器设置流式响应的编码，以及保留到响应发出为止的值 | 已在 akio 的 main（`c5cf4353`），已锁定 | 8、11 | I47 |
| A91 | `Files` 为内存中的文件保存压缩副本 | 已在 akio 的 main（`c5cf4353`），已锁定 | 4、12 | I47 |
| A92 | 从磁盘文件发送的消息体按流编码压缩 | 已在 akio 的 main（`4989cd8b`），已锁定 | 1、5 | I47 |
| A93 | HTTP/1 客户端在 `101 Switching Protocols` 时交出连接 | 已在 akio 的 main（`c5cf4353`），已锁定 | 8、9 | I49、I55 |
| A94 | 对 HTTP/2 扩展 CONNECT 的流式 `2xx` 应答即两半各自关闭的隧道 | 已在 akio 的 main（`c5cf4353`），已锁定 | 9 | I49 |
| A105 | 请求的对端是受信任 PROXY 头所指的客户端（akio-http） | 已在 akio 的 main（`e9891b52`），已锁定 | 19 | I52 |
| A108 | HTTP/1 客户端的 TLS 服务器名，代替 URL 的主机名 | 已在 akio 的 main（`e9891b52`），已锁定 | 19 | I54 |
| A109 | 构建 TLS 配置时检查客户端证书的私钥与叶证书相符 | 已在 akio 的 main（`e9891b52`），已锁定 | 19 | I54 |
| A111 | 响应过滤器看到流式消息体发出的字节及其结束方式（消息体旁路） | 已在 akio 的 main（`e9891b52`），已锁定 | 5、6 | I67 |
| A112 | 按 RFC 9110 要求接收方接受的三种格式解析 HTTP 日期 | 已在 akio 的 main（`e9891b52`，ADR-116），已锁定 | 15 | I67 |
| A113 | 在内层过滤器之后运行的响应过滤器（外层过滤器） | 已在 akio 的 main（`e9891b52`，ADR-116），已锁定 | 5 | I67 |
| A117 | 从 `akio` 同时取得 `akio::Pool` 与 `PoolConfig`：应用自己的、有上限的阻塞工作线程池 | 已进入 akio 的 main 并锁定（自 `6bcd8ad6` 起） | 4 | I68 |
| A132 | 代理隧道的上游连接建立后，按所请求的名字和两端地址检查；SOCKS 客户端取受信任 PROXY 头所指的客户端 | 已在 akio 的 main（`8453b60c`），已锁定 | 1、19 | I55 |
| A133 | 代理自己的连接超时，并给出应答（SOCKS5 4、HTTP 504） | 已在 akio 的 main（`8453b60c`），已锁定 | 10 | I55 |
| A134 | 经 HTTP/2 和 HTTP/3 的 CONNECT 隧道（RFC 9113 §8.5、RFC 9114 §4.4） | 已在 akio 的 main（`4989cd8b`），已锁定 | 9、19 | I55 |
| A135 | 拨号 Unix 域流套接字，以及在其上的 HTTP 客户端（akio、akio-http、akio-sys） | 已在 akio 的 main（`0f2c419b`），已锁定 | 1、2 | I58 |
| A148 | 请求到达的地址（监听器的本地地址），HTTP/1.1、h2 和 h3 | 已进入 akio 的 main 并锁定（自 `6bcd8ad6` 起，akio 的 ADR-157） | — | I75 |
| A165 | 插件用的 WebAssembly 运行时：沙箱解释器、宿主 API、WASI preview 1、实例池；Proxy-Wasm 需要的响应钩子 | 已在 akio 的 main（`dad77a7a`），已锁定 | 1、4、11 | I71、I72 |
| A166 | 由别处进行的调用决定时扣住响应头（fit 向 fib 提出的 A41） | 已在 akio 的 main（`dad77a7a`），已锁定 | 1、4 | I72 |
| A167 | 由别处进行的调用逐次过滤响应 body，trailer 在最后一次调用中（fit 向 fib 提出的 A46） | 已在 akio 的 main（`dad77a7a`），已锁定 | 1、4、5、8 | I72 |
| A168 | 只提供先验知识 HTTP/2 的 HTTP 服务端（带 `h2c` 而没有 `http/1.1` 的监听器） | 已进入 akio 的 main 并锁定（自 `6bcd8ad6` 起，akio 的 ADR-174） | 19 | I05、I25、I79b |
| A169 | akio 的服务端拒绝 RFC 9112、RFC 9113 和 RFC 9114 规定为畸形的请求：两个 `Host` 字段、HTTP/2 与 HTTP/3 字段名和值中的非法字节、流式 body 的分帧错误以 400 应答 | 已进入 akio 的 main 并锁定（自 `0fc8f3a7` 起） | 15、19 | I25 |
| A171 | HTTP 客户端请求结束时取消其定时器任务，而不是保留到超时时间 | 已进入 akio 的 main 并锁定（自 `0fc8f3a7` 起） | 11、13 | I15、I18、I25 |
| A172 | TLS 服务端把 server name 为 IP 地址的 ClientHello 视为没有名字，而不是使握手失败 | 已进入 akio 的 main 并锁定（自 `0fc8f3a7` 起） | 19 | I09、I25 |
| A170 | 不经 `Conn::inner` 读取连接的 TCP 计数（以及未读字节），并通过引擎自己的 API 先写数据再拼接 | 已进入 akio 的 main 并锁定（自 `6bcd8ad6` 起，akio 的 ADR-156） | 1、14 | I51b |
| A180 | 让 akio-wasm 的调用更便宜，并与 wazero 对比测量（akio 的 W5） | 已进入 akio 的 main 并锁定（自 `077a6710` 起） | 13 | I71、I72、I76 |
| A200 | 按 RFC 9112 的要求处理 absolute-form 请求目标：以其路径作为请求的路径，以其 authority 作为主机 | 已进入 akio 的 main 并锁定（自 `077a6710` 起，akio 的 ADR-177） | 15、19 | I77 |
| A214 | 所有连接合计排队字节的预算，超出时暂停每个连接的读取（fit 的 `limits.max_pending_bytes`） | 已进入 akio 的 main 并锁定（自 `6bcd8ad6` 起，akio 的 ADR-172） | 8、11 | I79、I79b |
| A215 | 只计入被接受的连接、不计入引擎拨出的连接的连接上限（fit 的 `limits.max_conns`） | 已进入 akio 的 main 并锁定（自 `6bcd8ad6` 起，akio 的 ADR-173） | 11 | I79、I79b |
| A220 | UDP 和数据报监听器上的 accept 过滤器：在为被拒绝的对端建立连接（`udp` 会话、QUIC 连接）之前丢弃它的数据报 | 已进入 akio 的 main 并锁定（自 `9e21ada7` 起，akio 的 ADR-178） | 1、11 | I85、I86 |
| A225 | 对 akio-crypto、akio-tls（连同 akio-x509）、akio-http 的 HTTP/1 和 HTTP/2 解析器以及 akio-wasm 的外部安全审查（不是 API） | 已提出（I80b）：威胁模型的 L15 | 15 | I80、I80b |
| A230 | akio-crypto 中的 Argon2id（RFC 9106）及 BLAKE2b：fit 的 `fitstore` 格式的密钥派生 | 已提出（I93）：I93 等待它 | 15、20 | I93 |
| A231 | akio-crypto 中的 XChaCha20-Poly1305（HChaCha20 和 24 字节 nonce）：fit 的 `fitstore` 格式的 AEAD | 已提出（I93）：I93 等待它 | 15、20 | I93 |
| A232 | 由处理函数设置 HTTP 连接的空闲超时并关闭它，并把已接受的连接告诉应用：fit 对未登录连接的限制 | 已进入 akio 的 main 并锁定（自 `9e21ada7` 起，akio 的 ADR-180） | 10、11 | I90 |
| A233 | 关闭终端回显（原始模式）以读取密码 | 已提出（I89）：在此之前密码按输入显示，提示中说明 | 14 | I89 |
| A234 | 单个连接自己的发送队列上限，可高于或低于引擎的 `max_queued_bytes` | 已提出（I101）：在此之前链路在超过 8 MiB 时压住喂给它的一方 | 8, 11 | I101, I104 |
| A235 | 在握手中按 SHA-256（pin）检查服务器证书、代替证书链和名称检查的客户端 TLS 配置 | 已提出（I102）：在此之前于握手完成后、发送任何字节前检查 | 1, 15 | I102, I104 |
| A236 | 在 `WsSender` 上压住和恢复 WebSocket 的读取、得知其发送队列排空、取得对端地址（akio-ws 的客户端和 akio-http 的升级都要） | 已提出（I102c）：隧道的 WebSocket 链路和 LLM mesh 在等待它 | 8, 9 | I102c, I104 |
| A237 | 在线程局部变量的析构中调用 `Engine::wait`（以及 `Pool::is_current`）也安全 | 已提出（2026-11-27）：在此之前 akit 的客户端引擎在专门的线程上等待 | 1, 15 | — |

## A1 运行中引擎上的 TCP 监听器

**问题。** `EngineBuilder::listen` 在 `build` 之前添加 TCP 监听器；`Engine` 在运行时有 `bind_udp`，但 TCP 没有对应的
方法，也没有办法关闭一个监听器并排空它的连接。fit 每次重新加载配置和每次 Admin API 修改都会替换监听器（fib 的
`AddBinding`、`RemoveBinding`、`ShutdownBinding`）。

**证据。** `crates/akio/src/lib.rs`：`EngineBuilder::listen`、`Engine::bind_udp`、
`Engine::adopt_tcp`（为在别处接受的流提供服务）。

**建议的 API。** `Engine::add_listener(Listener) -> io::Result<ListenerId>`、
`Engine::remove_listener(id)`（停止接受）、`Engine::shutdown_listener(id, drain: Duration)`
（停止接受，排空期过后关闭它的连接），监听器的后端和加速选项与构建时相同。

**akit 的用法。** I22 和 I28 在每次替换时对比新旧监听器的差异。I22 已在 `a9a620ab` 上这样做
（`akit_listener::Listeners::reload`）：换入前 `add_listener`，换入后以关闭超时 `shutdown_listener`；之前用的
`adopt_tcp` 临时措施已去掉。UDP 监听器还不能移除（A22）。

## A2 每次握手异步选择证书

**问题。** `ServerConfigBuilder::sni_cert` 在构建配置时就固定了证书。自动 HTTPS 会在证书续期时替换它们，按需 TLS
必须在握手期间签发证书，而且不能阻塞 I/O 线程。

**建议的 API。** 服务端配置上的解析器 `fn resolve(&self, hello: &ClientHello, done:
CertDone)`，`done` 可以稍后从其他线程以证书或告警完成；握手等待期间不占用线程。缓存命中的证书走同步快速路径。

**akit 的用法。** 基于 `a9a620ab`（ADR-098）的 I30：每个 TLS 监听器的配置都用
`ServerConfig::builder_with_resolver` 构建，其解析器 `akit_runtime::ListenerCerts` 从当前图的 `akit_tls::Certs`
内联作答（fit 的选择方式：精确或通配符名字，再看客户端支持什么，依据 `ClientHelloInfo` 的签名方案和套件），因此重载或
`-watch-certs` 可以替换证书而不关闭监听器，任何证书中的通配符都可用（I09 的 `sni_cert` 权宜做法拒绝第一个之后的）。
I34 为按需接收的名字稍后完成 `CertDone`：名字被拒绝时在 ask 客户端的线程上，证书获取后在 ACME 工作线程上。

## A3 过滤已接受的连接

**问题。** fit 的允许和拒绝列表在接受时就拒绝客户端，早于任何 handler 或 TLS 工作（fib 的 `Binding.Filter`，fit
清单中的 A73 和 A74）。

**建议的 API。** `Listener::filter(Fn(&SocketAddr) -> bool)`，在 I/O 线程上于 acceptor 之前调用，附带被拒连接数的
计数器；可在运行时替换。

## A4 PROXY 协议

**问题。** 位于负载均衡器后面的四层和 HTTP 监听器从 PROXY 头中取得客户端地址；上游连接也可能需要发送 PROXY 头。

**建议的 API。** 一个监听器选项，在 acceptor 运行之前读取并检查 v1 或 v2 头（带超时和大小上限），并在 `Conn` 上
提供这些地址；一个拨号选项，用于写出 PROXY 头。

## A5 akio-mux 中的 SNI 路由

**问题。** `akio-mux` 有 `Rule::Tls`，它匹配任何 ClientHello。TLS 直通需要服务器名，要从可能跨越多次读取的
ClientHello 中读出，并且不终结 TLS。

**建议的 API。** `Rule::TlsSni(matcher)`，或者一个拿到解析出的 SNI 和 ALPN 的决策回调，沿用 `Mux` 已有的嗅探上限。

## A6 CSR 与 JWS

**问题。** akit 的 ACME 客户端需要为它的密钥生成 PKCS#10 CSR，以及 JWS 签名（ES256、RS256）。

**已完成**（akio ADR-107，在其分支上编号为 093）：`akio_x509::CsrParams::sign` 生成请求，`CertificateRequest` 解析并检查请求。JWS 不需要
新东西：`akio_crypto::pkey::PrivateKey::sign` 已经给出 JWS 所用原始格式的 ES256/384/512、EdDSA 和 RS256。

## A7 MP4 读取器

**问题。** fit 的媒体库不转码，直接从 MP4 文件切出 HLS 和 fMP4 分片（fib A64）。媒体库路线开始时需要。

## A8 TUN 设备

**问题。** fit 的 VPN 路线在引擎上运行 TUN 接口。等这条路线在 akit 中开始时需要，在 fit 之后。

## A9 限制在 TLS 1.2 的 TLS 服务端

**问题。** fit 的 `listeners[].tls.max_version` 接受 `1.2`，用于只卸载 TLS 1.2 的网卡（Go 的
`tls.Config.MaxVersion`）。akio-tls 的 `ServerConfigBuilder` 有 `tls13_only`，但只能用 `__tls12_only` 把服务端限制在
TLS 1.2，而它是隐藏的，标明只供测试和互通工具使用。

**证据。** `50a82315` 的 `crates/akio-tls/src/config.rs`：`ServerConfigBuilder::tls13_only`、
`ServerConfigBuilder::__tls12_only`（`#[doc(hidden)]`）。

**建议的 API。** `ServerConfigBuilder::max_version(TlsVersion)`（以及 `min_version`），或公开的
`tls12_only(bool)`；`ClientConfigBuilder` 上也一样。

**akit 的用法。** I09 读取 `max_version`，把 `1.2` 拒绝为尚不支持；有了这个 API，akit-tls 按监听器的 `tls` 设置它。

**已实现**（akio ADR-121）：`ServerConfigBuilder::tls12_only(bool)` 与 `ClientConfigBuilder::tls12_only(bool)`。
`max_version` 为 `1.2` 时 akit-tls 在监听器的 TCP 配置上设置它；HTTP/3 仍为 TLS 1.3。

## A10 关闭 HTTP 客户端的空闲连接

**状态。** 已进入 akio 的 main（`0fc8f3a7`，`Client::close_idle`）并锁定。

**问题。** 图退役时，它的上游连接池会排空并关闭（I14，决策 004）。最后一个请求完成后，akio-http 的 `Client`
保持的空闲连接也应当关闭，就像 fit 的连接池关闭 fib 的客户端那样。akio 的客户端无法关闭它们：它们只在
`ClientOptions::idle_timeout`（fit 默认 60s）之后、服务端关闭它们时或引擎关闭时才关闭；丢弃 `Client` 也关不掉，
因为每个空闲连接的 handler 都持有它的一个克隆。

**依据。** `50a82315` 的 `crates/akio-http/src/client.rs`：`Pool`（空闲连接）是 `Inner` 私有的；`ClientConn`
持有 `client: Option<Client>`；只有 `DialAcceptor::on_shutdown` 和空闲清扫会关闭空闲连接。

**建议的 API。** `Client::close_idle(&self)`：立即关闭所有空闲连接，并让正在使用的连接不再回到空闲池（各自在
响应收完后关闭），如同 Go 的 `Transport.CloseIdleConnections` 和引擎关闭时所做的。不阻塞。

**akit 的用法。** `Pool::close` 在连接池排空后调用它。在此之前，已关闭连接池的空闲连接最多保持到它的
`idle_timeout`（记录在 I14 的进度条目中）。

**再次遇到（I38）。** 频繁替换测试：每次修改上游的重载都会替换它的连接池，被替换连接池的空闲连接一直保持到
`idle_timeout`；每分钟数千次重载会留下数千个打开的上游连接（I38 像 fit 一样把未变上游的连接池交给下一个图共享，
只剩下改变了的上游）。

**已实现（I38）。** 在 akio 分支 `claude/a10-close-idle` 上，提交 `af959290`（基于 `e79cc740`），未推送、未合并：
`Client::close_idle` 立即关闭空闲连接，并设置客户端的 `shutting_down` 标志（引擎优雅关闭所用的那个），使收完响应的
连接关闭而不再回到空闲池；请求路径不变。由 `client` 的 `close_idle_closes_idle_connections_now` 测试（macOS，以及
gpu51 的 epoll 和 io_uring）。akio 的 main 有了它、akit 的 `rev` 移过去之后，akit 在 `Pool::close` 中调用它。

## A11 请求自己的 `Host` 字段

**问题。** 反向代理会把客户端的 `Host` 传给上游，除非配置要求改写（fit 的代理，I15）。akio-http 的 HTTP/1
客户端总是根据请求 URL 写 `Host`，并丢弃请求自带的 `Host` 字段。

**依据。** `50a82315` 的 `crates/akio-http/src/client.rs`，`Client::attempt`：请求头按 `parse_url` 得到的
`Host: {host_header}` 写出，名为 `host` 的请求字段被跳过。

**建议的 API。** `ClientRequest::headers` 中设置的 `Host` 字段取代从 URL 得到的那个（URL 仍决定连到哪里），
或者提供 `ClientRequest::host(name)` 设置方法。

**akit 的用法。** I15 的代理通过上游连接池发送客户端的 `Host`，或其参数设定的 `Host`。

**已完成（I15）。** 在 akio 分支 `claude/i15-proxy-699f18`，提交 `552fe283`，未合并：`ClientRequest::headers`
中的 `Host` 字段代替从 URL 得到的那个发出（连接的地址与 TLS 请求的名称仍由 URL 决定），重定向到同源时保留，
重定向到别的源时去掉。同时，为了 fit 的代理不发送客户端没有发送的 `User-Agent`：值为空的 `User-Agent` 字段表示
不发送，也不添加 akio 的默认值（`akio-http`），与 Go 的客户端一致。akio-http 的测试在 macOS 和 Linux（epoll 与
io_uring）上通过。akit 的 I15 用到这两点；相关测试在 akit 的固定版本移到含 A11 的 akio 提交之前保持忽略。

## A12 HTTP 客户端的连接超时作用于每次连接

**状态。** 已在 akio 分支 `claude/akit-open-requests` 上实现（`22e038cc`，akio 的 ADR-154）；akit 已使用，尚未移动锁定。`akio::dns::HappyEyeballs` 用 `Engine::dial_with` 和它的 `connect_timeout` 拨出每一次尝试，单个地址也不例外，所以 akio-http 的 `Client` 与 `H2Client` 对任意数目的地址都遵守各自的连接超时。akit 不再把引擎的 `connect_timeout` 设为各上游中最短的那个（akit-listener 的 `build`）；akit-listener 的 `upstreams` 测试中的 `connect_timeout` 检查单个地址的上游受自己的超时约束，无论比另一个上游的短还是长。

**问题。** fit 按每个上游的 `connect_timeout` 限制建立到上游的连接所用的时间。akio-http 的客户端通过
`ClientOptions::happy_eyeballs.connect_timeout` 接受这个值，但 Happy Eyeballs 竞速器只在一个名字解析出两个及以上
地址时才使用它：单个地址（IP 字面量、只有一条记录的名字），或竞速器忙不过来而改为依次拨号时，用 `Engine::dial`
拨号，受引擎的 `Config::connect_timeout` 限制。

**依据。** `50a82315` 的 `crates/akio/src/dns.rs`：`HappyEyeballs::dial` 对单个地址直接返回
`engine.dial(addrs[0], ...)`，`in_turn` 在 `Outcome::Busy` 时用 `engine.dial` 拨号。

**建议的 API。** `Engine::dial_timeout(addr, timeout, acceptor, on_error)`（或给 `dial` 加超时），由
`HappyEyeballs::dial` 用于每次尝试，使它的 `connect_timeout` 无论有几个地址都成立。

**akit 的用法。** 每个上游的连接池设置自己的超时。在此之前，`akit_listener::engine` 把引擎的
`connect_timeout` 设为各上游中最短的那个；各上游一致时即为各自的值（记录在 I14 的进度条目中）。

## A13 `Files` 服务调用者给出的路径

**问题。** fit 的 `static` handler 在根目录下查找去掉 `strip_prefix` 后的请求路径；akit 的监听器按请求的规范路径
（去掉点段，I11）路由，并把它存在 context 的 local 里，因为 akio 的请求目标无法改写。akio-http 的 `Files` 从路由参数
`path`（只有 akio 自己的 `Router` 会设置）或原样发送的请求目标读取路径，所以 akit 两者都无法交给它。

**依据。** `50a82315` 的 `crates/akio-http/src/files.rs`，`Files::relative`：先取 `ctx.param_raw("path")`，否则用
`ctx.path()`；`Context::set_params` 是 `pub(crate)`。

**建议的 API。** `Files::serve(&self, ctx: &mut Context, path: &str)`，即给定路径的 `Service::call`（路径仍由
`Files` 做百分号解码和检查），或者公开设置请求路由参数的方法。

**akit 的用法。** `static` handler 传入去掉 `strip_prefix` 的规范路径；在此之前 `strip_prefix` 被拒绝为尚不支持，
而带有 `..` 段但仍在根目录内的路径（`/x/../ok.txt`）——监听器按 `/ok.txt` 路由、fit 的整套服务会提供它——由 handler
答 400，与 fit 的 handler 单独时的回答相同。

**已完成**（akio ADR-100，已在 akio 的 main `a9a620ab`，已锁定）：A13 为 `Files::serve(ctx, path)`；A16 为
`Files::twins(&[&str])` 和按 q 值选择副本；A17 为 `Files::try_paths(&[&str])`，配合用于 fit 的 dotfiles 的
`Files::hide(f)`；A18 为 `Files::list_directories(max_entries)`，页面与 fit 的逐字节相同。akit 的 `static` handler
自 I13 起使用它们：它提供规范路径（因此 `/x/../ok.txt` 被提供，与 fit 的整套服务一样），`strip_prefix`、`precompressed`、
`try_files`、`browse` 和 `browse_max_entries` 已支持。

## A14 `Files` 像网关的静态文件 handler 那样应答

**问题。** fit 的 `static` handler（akit 移植它的测试，规则 19）对一些请求的回答与 `Files` 不同，而 `Files` 没有
相应的设置：

- 按顺序尝试的索引文件列表，或者没有（`index: []`）；`Files` 只接受一个名字；
- 不带结尾斜杠请求的目录被重定向（301，`Location` 加上斜杠并保留查询串），使相对链接能正确解析；`Files` 直接提供
  索引文件；
- 带结尾斜杠请求的文件（`/a.txt/`）答 404；`Files` 丢掉空段后提供它；
- （已随 A16 完成：`Files::twins`，akit 按 `precompressed` 设置）压缩副本只对配置列出的编码提供，也只有这时才加
  `Vary: Accept-Encoding`；
- 内容类型来自完整的表（Go 的 `mime.TypeByExtension`：`.xml` 是 `text/xml; charset=utf-8`，`.avif` 也有，Linux 上
  还有 `/etc/mime.types`）、配置的 `default_type`，否则从前 512 字节嗅探；`Files` 只有 22 个扩展名的固定表，其余都是
  `application/octet-stream`；
- 不允许读取的文件答 403；`Files` 对任何 `stat` 错误都答 404；
- 只提供普通文件：`Files` 会打开任何不是目录的东西，所以根目录下的 FIFO 会占住它磁盘池的一个线程，直到有写者出现
  （akit 的测试 `a_hung_disk_holds_no_io_thread` 正依赖于此）；
- 带 `:` 的名字和其他名字一样；`Files` 对它答 404；
- 错误响应体如 `"404 Not Found\n"`；`Files` 发送 `"Not Found"`（没有 `try_files` 候选找到路径时也是）；
- `strip_prefix` 之下的目录列表以请求的整个路径为标题（`Index of /assets/`，带 `../` 链接）；`Files::serve` 以给它的
  路径为标题（`Index of /`，没有 `../`）：与路径一起给出标题（或前缀）即可（I13）。

**依据。** `50a82315` 的 `crates/akio-http/src/files.rs`：`Inner::index: String`、`Files::relative`（过滤空段，
拒绝 `:`）、`Inner::serve_disk`（目录提供索引文件；`fs::metadata` 出错一律答 404）、`Inner::load_tracked`（总是
查找 `TWINS`）、`Cached::new`（总是加 `Vary`）、`Files::mime`、`Inner::read`（对任何非目录调用 `fs::read`）。

**建议的 API。** `Files` 的构建设置：`index_files(&[&str])`、`redirect_directories(bool)`、`twins(&[&str])`
（编码及其顺序；为空则不再查找）、`mime(fn(&Path) -> Option<&'static str>)` 或一张表、`default_type(&str)` 和
`sniff(bool)`；以及固定行为：`PermissionDenied` 答 403，既不是普通文件也不是目录的答 404，带结尾斜杠的文件答 404，
允许 `:`，并用 `error_body(fn(u16) -> Response)` 生成简短的错误响应。

**akit 的用法。** `static` handler 按 fit 的参数（`index`、`precompressed`、`default_type`）和 fit 的固定行为设置它们；
在此之前，这些差异记录在 I12 的进度条目中，它的测试检查 akit 目前的回答并注明本请求，多于或少于一个名字的 `index`
列表和 `default_type` 被拒绝为尚不支持。

**已完成**（akio ADR-124，在 akio 的分支 `claude/a57-a58-a14-a15-a92` 的 `7d1b1670`，尚未合并，
已锁定）：`Files::index_files`、`redirect_directories`、`mime_types`（`MimeTypes::system`，Go 的表加系统的
`mime.types`）、`default_type`、`sniff`（Go 的 `DetectContentType`）、`error_response` 与
`serve_prefixed`；不可读的文件回 403，不是普通文件的（从不打开）以及带尾斜杠请求的文件回 404，允许 `:`。
akit 的 `static` handler 接受 fit 的 `index` 列表与 `default_type`，像 fit 一样重定向，回答 fit 的简短正文，
并以完整路径为 `strip_prefix` 之下的目录列表命名。

## A15 `Files` 的内存有界，磁盘池有超时

**问题。** fit 在内存中最多保存 `max_cache_bytes`（默认 64 MiB）的文件，在文件池中等待超过池超时的请求答 503，
因此不会无限等待卡住的磁盘（规则 4）。`Files` 限制它保存的每个文件的大小（`max_cached_bytes`）和较大文件打开的
句柄数（`max_open_files`），但不限制总量：根目录下每个不超过该大小的文件最终都可能进入内存；而在它磁盘池（16 个
线程、4096 个排队）中排队的请求会一直等到磁盘返回。

**依据。** `50a82315` 的 `crates/akio-http/src/files.rs`：`shards` 映射随每个读入的文件增长
（`Inner::load_tracked`）；`disk_pool()` 是 `max_threads` 16、`queue_capacity` 4096 的 `Pool`，`try_execute` 只在
队列满时拒绝。

**建议的 API。** `Files::max_cache_bytes(n)`（内存中的条目，与打开的句柄一样按最近最少使用淘汰），以及
`Files::disk_timeout(d)`：在 `d` 内未从磁盘池得到回答的请求答 503 并带 `Retry-After`，它的任务之后运行时不再回答。

**akit 的用法。** `static` handler 设置 fit 的 `max_cache_bytes`（及其默认值）和池超时；在此之前
`max_cache_bytes` 被拒绝为尚不支持，`Files` 占用的内存只受根目录下不超过 `max_file_bytes` 的文件总量限制（记录在
I12 的进度条目中）。

**已完成**（akio ADR-124，分支 `claude/a57-a58-a14-a15-a92` 的 `7d1b1670`，已锁定）：
`Files::max_cache_bytes`（默认 64 MiB，计入孪生文件与压缩副本，CLOCK 淘汰）与 `Files::disk_timeout`
（超时回 503 并带 `Retry-After`）。akit 的 `static` handler 接受 fit 的 `max_cache_bytes`，在文件池上最多
等待 fit 的 10 秒。

## A16 在配置的编码中按 q 值选择压缩副本

**问题。** fit 的 `static` handler 只为 `precompressed` 列出的编码提供文件的 `.br`、`.zst` 或 `.gz` 副本，并按客户端
`Accept-Encoding` 的 q 值（RFC 9110 第 12.5.3 节）选它最偏好的一个，相同时取列表中靠前的；`q=0` 的编码被拒绝，`*`
代表字段未列出的编码，编码名不区分大小写。`Files` 取它固定顺序 `br`、`zstd`、`gzip` 中第一个出现在字段里的，不看
q 值：`br;q=0` 得到客户端拒绝的 `br` 副本；`br;q=0.5, gzip` 得到 `br`；`*` 得不到任何副本；`GZIP` 和
`gzip ;q=1`（参数前有空白）也得不到。

**依据。** `50a82315` 的 `crates/akio-http/src/files.rs`：`pick` 按 `,` 切分字段，再把
`tok.trim().split(';').next()` 与 `TWINS` 的名字比较，区分大小写，不去掉名字后的空白，也不读 `q`；`TWINS` 固定为
`[("br", "br"), ("zstd", "zst"), ("gzip", "gz")]`。akit 的测试 `precompressed`
（`crates/akit-handler/tests/static_files.rs`）检查这些应答。

**建议的 API。** 配合 A14 的 `twins(&[&str])`（编码按相同 q 值时的偏好排列；为空时不查找副本也不加 `Vary`），
`pick` 按 q 值（编码自身的，否则 `*` 的，0 表示拒绝）不区分大小写地给磁盘上的副本排序，以配置的顺序打破平局，
没有 q 值大于 0 的副本时提供原文件。

**akit 的用法。** `static` handler 传入 `precompressed` 的列表。在此之前 akit 接受 `precompressed`，`Files` 按上述
方式提供它找到的副本；差异记录在 I13 的进度条目中，测试在每一处注明本请求。

## A17 依次尝试候选路径（`try_files`）

**问题。** fit 的 `try_files` 提供列表中第一个存在的路径：`{path}` 代表请求的路径，以 `/` 结尾的路径表示由索引文件
（或目录列表，A18）提供的目录；都不存在时答 404，此时不重定向不带斜杠的目录。`["{path}", "{path}/", "/index.html"]`
用来提供单页应用。带 `..` 段的候选，或 `dotfiles` `deny` 隐藏的点文件候选，被跳过。`Files` 只提供请求给出的路径，
它的 404 就是最终应答：akit 无法问它某个路径是否存在，也无法让它提供另一个路径（A13），除非在 I/O 线程上做
`Files` 在它的池中做的磁盘工作。

**依据。** `50a82315` 的 `crates/akio-http/src/files.rs`：`Service::call` 从 `Files::relative`（路由参数 `path`
或请求目标）取路径，`Inner::serve_disk` 找不到时自己答 404；没有接受路径或把未命中告诉调用者的方法。

**建议的 API。** `Files::try_paths(&[&str])`：带 `{path}` 占位符的候选模板，在磁盘池上依次解析（与路径一样连同解析
结果缓存），提供第一个是文件或有索引文件的目录的候选；不安全的候选（`..`、NUL、`\`），或被钩子拒绝的候选
（`hide(fn(&str) -> bool)`，用于 fit 的 dotfiles），被跳过；都没有时答 404。或者更通用的：配合 A13 的 `serve` 的
`Files::serve_first(ctx, &[PathBuf])`。

**akit 的用法。** `static` handler 传入 `try_files` 和它的点文件检查。在此之前 akit 像 fit 一样检查 `try_files`
（每一项以 `/` 或 `{path}` 开头；`dotfiles` 为 `deny` 时不指向点文件），然后拒绝为尚不支持，并注明 A13 和本请求。

## A18 为没有索引文件的目录生成列表

**问题。** fit 的 `browse` 为带尾部斜杠请求、没有索引文件的目录生成列表：一个 HTML 页面（`Index of /files/`，除根
目录外有 `../` 链接，目录在前，再按名字排序，每个名字做 HTML 转义，`href` 是相对的、百分号转义的单个段，大小和
UTC 修改时间），最多读取 `browse_max_entries` 项（默认 10000；还有更多时在最后一段说明），以点开头的名字无论
`dotfiles` 怎么设都不列出，`Content-Type: text/html; charset=utf-8` 和 `Cache-Control: no-cache`，每次都在文件池上
读取。`Files` 对没有索引文件的目录在它的磁盘池上答 404，akit 既无法把这个 404 与其它 404 区分，也无法在那里列出
目录。

**依据。** `50a82315` 的 `crates/akio-http/src/files.rs`：`fs::metadata(dir/index)` 失败或不是文件时，
`Inner::serve_disk` 返回 `Response::text(404, "Not Found")`。

**建议的 API。** `Files::list_directories(max_entries)`（默认关闭），在磁盘池上按上述方式应答，排序前先限制条目数
（规则 4 和 11）；或者一个钩子 `on_directory(fn(&Path, &Request) -> Response)`，对没有索引文件的目录在磁盘池上
运行，由调用者生成页面。

**akit 的用法。** `static` handler 为 `browse` 和 `browse_max_entries` 打开它。在此之前 `browse` 为 `true` 时被拒绝
为尚不支持并注明本请求；`browse` 为 `false`（fit 的默认值）和 `browse_max_entries` 被接受。

## A19 请求体边产生边发送的 HTTP 客户端请求

**问题。** 反向代理把客户端的请求体边到达边发给上游（规则 5），从不整体缓冲，并在上游跟不上时停止从客户端读取
（规则 8）。akio-http 的 HTTP/1 客户端只接受完整的请求体：代理必须先把每个上传整体保存在内存中，每个请求最多
`limits.max_body_bytes`（默认 16 MiB），上游才能看到第一个字节。

**依据。** `50a82315` 的 `crates/akio-http/src/client.rs`：`ClientRequest::body` 是 `Vec<u8>`；`Client::request`
把头和整个请求体编码进一个缓冲区（`Pending::bytes`），`ClientConn::begin` 用一次 `send_owned` 发出。没有分块的请求体，
也无法得知连接的写队列已排空。（客户端之下的 akio-core 已有所需的东西：`Conn::on_drain` 和写高水位，`conn.rs`。）

**建议的 API。** 请求体是一个流，而不是字节：

- `ClientRequest::streamed(len: Option<u64>)`（或 `ClientBody::Stream { len }`）：请求体稍后写入，已知 `len` 时用
  `Content-Length` 分帧，否则分块。
- `Client::request_streamed(req, on_ready, done)`：头写出后（连接建立后、等到池中的连接后、复用的连接已失效而重试
  后；请求体一旦开始发送，流式请求就不再重试，Go 的客户端也不重试这种请求），在连接的 I/O 线程上调用
  `on_ready(RequestBody)`。`RequestBody` 是 `Send` 的，提供 `write(&[u8]) -> Result<(), Closed>`（复制进连接的队列，
  分块时加上分块帧）、`queued_bytes()`、`on_drain(f)`（队列降到低水位以下时在 I/O 线程上运行 `f`）、`finish()`
  （最后一块，或检查已写满 `len` 字节）和 `finish_with_trailers(Headers)`；未完成就丢弃它会关闭连接，请求以
  `ClientError::Closed` 失败。`done` 和现在一样得到响应，有了 A20 则得到响应头。
- 响应可能在请求体完成前就开始或整个到达（上游提前答 413）：此时 `done` 运行，之后的写入失败。

**akit 的用法。** I16 的代理以流的方式接收客户端的请求体（`BodyMode::Stream`、`on_body`），把每块写进上游的
`RequestBody`，在 `queued_bytes` 高于水位时暂停读取客户端（A21），并在 `on_drain` 中恢复。上游连接池
（`akit-upstream`，它封装 `Client::request`）、I17 的取消（丢弃请求体）、I18 的 h2 上游（`H2Client` 上的同样接口）、
I49（WebSocket）和 I75（FastCGI）也会用到。akio 在 `60547435` 中实现了它（ADR-101：`ClientRequest::streamed`、
`RequestBody`，其 `on_drain` 以连接队列排空为准）；I16 通过 `akit_upstream::Pool::stream` 按上面的方式使用。

## A20 边到达边交付的 HTTP 客户端响应

**问题。** 反向代理把上游的响应边到达边发回，并在客户端跟不上时停止读取上游（规则 5 和 8）。akio-http 的客户端把
每个响应体整体缓冲（不超过 `ClientOptions::max_body_bytes`，默认 64 MiB），然后只回调一次：经过代理的下载整个
保存在内存中，超过 64 MiB 的响应失败，慢客户端也无法让上游慢下来。

**依据。** `50a82315` 的 `crates/akio-http/src/client.rs`：`ClientConn` 把每个响应体字节追加到
`ClientResponse::body`（`RState::Body`、`RState::Chunked`、`RState::UntilClose`），在 `complete` 中调用回调；从不
暂停连接的读取。

**建议的 API。** `Client::request_with(req, handler: impl ResponseHandler)`（`H2Client` 上也一样）：

- `on_head(&mut self, head: ResponseHead, body: ResponseBody)`：响应头到达后得到状态码、原因短语和字段（之前的
  1xx 临时响应照旧跳过或交付）；`ResponseBody` 是 `Send` 的，提供 `hold_reads(bool)`（停止和恢复读取连接，h2 则是该
  流的流量控制窗口）和 `cancel()`（关闭连接，或重置流）。
- `on_data(&mut self, chunk: &[u8])`：读到的每一段响应体，已去掉分块帧，只在本次调用内有效（规则 6）。
- `on_end(&mut self, trailers: &Headers)` 和 `on_error(&mut self, e: ClientError)`；另有两次读取之间的超时（fit 的
  `response_idle_timeout`，I17）。
- 这种响应不受 `ClientOptions::max_body_bytes` 限制，由 handler 决定。
- `on_end` 之后连接回到 keep-alive 池，和现在回调之后一样。

**akit 的用法。** I16 的代理在 `on_head` 中开始客户端的响应（`Detached::stream_response`），把每块写进它的
`BodyStream`，在客户端的 `queued_bytes` 高于水位时调用 `hold_reads(true)`，在客户端排空时（A21）调用
`hold_reads(false)`。I17（trailer、空闲超时、取消）、I18（h2 上游）和缓存（I67，响应经过时存储）也会用到。akio 在
`60547435` 中实现了它（ADR-101：`Client::request_with`、`ResponseHandler`、`ResponseBody`；`H2Client` 上还没有，
读取之间的空闲超时也还没有，分别随 I18 和 I17 而来）；I16 按上面的方式使用。

## A21 可暂停其客户端读取、并在客户端排空时得到通知的服务端请求

**问题。** 联动背压（规则 8）的另一半是被代理请求的客户端一侧：上游跟不上时停止读取客户端的请求体；客户端跟不上
响应时，在它赶上时得到通知，以便恢复上游。akio-http 的服务端把流式请求体按客户端发送的速度交给 `on_body`，流式响应
（`BodyStream`）只能轮询（`queued_bytes`）；上下文和流都接触不到连接的 `hold_reads` 和 `on_drain`。

**依据。** `50a82315` 的 `crates/akio-http/src/context.rs`：`Context` 和 `Detached` 无法访问连接；`BodyStream` 只有
`write`、`queued_bytes` 和 `finish`。`crates/akio-http/src/session.rs` 的 `State::Stream`：每次读到的数据立即交给
sink。`crates/akio-http/src/h2.rs` 的 `on_data`：无论 sink 是否跟得上，DATA 一到就补充流的接收窗口（“请求体被
立即消费”）。akio-core 在每个连接上两半都有（`ConnInner::hold_reads`、`ConnInner::on_drain`，
`crates/akio-core/src/conn.rs`），升级后的连接用的 `akio_http::raw` 也有。

**建议的 API。**

- `Context::body_reads() -> BodyReads`（用于 `BodyMode::Stream`；`Send`，`detach` 后由 handler 保存）：`hold(bool)`。
  在 HTTP/1 上暂停连接的读取（因而也暂停其后流水线中的请求，这是必须的）；在 h2 和 h3 上停止补充该流的流量控制
  窗口，其他流照常进行。
- `BodyStream::on_drain(f)`：响应排队的字节降到低水位以下时（已经低于则立即）或连接关闭时，在连接的 I/O 线程上
  运行 `f`；在 h2 和 h3 上，是流的发送窗口和连接的队列允许继续写入时。
- 流式传输中请求体超过限制：akio 已允许 handler 提前应答或丢弃 `Detached`（关闭连接），无需更多。

**akit 的用法。** I16 的代理在上游 `RequestBody::queued_bytes`（A19）高于水位时暂停读取客户端的请求体，并在它的
`on_drain` 中恢复；在客户端 `BodyStream::queued_bytes` 高于水位时暂停读取上游（A20），并在
`BodyStream::on_drain` 中恢复。压缩（I47）和 FastCGI（I75）也会用到。akio 在 `60547435` 中实现了它（ADR-101：
`Context::body_reads`、`BodyReads::hold`、`BodyStream::on_drain`，HTTP/1.1、HTTP/2 和 HTTP/3 都有）；I16 按上面的
方式使用。

## A22 移除运行中引擎的 UDP 监听器

**状态。** 已在 akio 分支 `claude/akit-open-requests` 上实现（`22bc4402`，akio 的 ADR-155）；akit 已使用，尚未移动锁定。`Engine::remove_listener` 与 `Engine::shutdown_listener` 接受按对端区分的 UDP 监听器（新对端被丢弃，已有对端被排空，到期关闭），`UdpBinding::shutdown` 排空运行时绑定的套接字，akio-h3 向正在排空的监听器上的连接发送 `GOAWAY`。akit-listener 的重载像对待 TCP 监听器一样移除和修改服务 `h3` 或 `udp` 的监听器：拒绝（`ReloadError::Udp`）已去掉；引擎自己的 UDP 监听器用 `shutdown_listener` 排空，重载绑定的用 `UdpBinding::shutdown` 排空。在原地址上被替换的 UDP 监听器最多排空 `UDP_REPLACED_DRAIN`（2 秒），因为这期间内核散列到它的套接字上的新对端得不到应答；没有替代者的被移除监听器排空整个排空超时。测试：akit-listener 的 `h3` 中的 `reload_knows_h3_listeners` 与 `reload_drains_h3`（移除启动时的 h3 监听器，以及在原地址上替换它；进行中的请求得到应答），以及其 `l4` 中的 `udp_listener_removed`。

**问题。** A1 能在运行时添加、移除和排空 TCP 监听器；交给 builder 的 UDP 监听器不能移除
（`Engine::remove_listener` 返回 `Unsupported`）。akit 在这样的监听器上提供 HTTP/3，所以移除或修改 akit 启动时就服务
`h3` 的监听器的重载（或 Admin API 修改）在重启之前都被拒绝。运行时新增的 HTTP/3 用 `Engine::bind_udp`，其
`UdpBinding::close` 立即关闭所有连接，不排空。

**证据。** `crates/akio/src/lib.rs`（`a9a620ab`）：`Engine::unlisten` 拒绝 `Kind::Tcp` 以外的一切；
`Engine::add_listener` 拒绝 UDP（运行时用 `bind_udp` 绑定 UDP）；`crates/akio-core/src/group.rs` 的
`UdpBinding::close`。

**建议的 API。** `Engine::remove_listener` 和 `Engine::shutdown_listener(id, drain)` 也支持 UDP 监听器（按对端的
UDP：不再接收新对端，对已有的调用 `on_shutdown`，到期关闭剩下的），`Engine::add_listener` 也接受 UDP 监听器；或者
含义相同的 `UdpBinding::shutdown(drain)`。

**akit 的用法。** `akit_listener::Listeners::reload`（I22、I28）像移除 TCP 一侧那样移除并排空监听器的 HTTP/3，
`ReloadError::Udp` 的拒绝随之去掉。

## A23 Unix 域流式监听器

**问题。** `admin.address` 为 `unix/<path>` 时，fit 的 Admin API 监听 Unix 套接字（权限设为 0600，替换残留的
套接字文件，拒绝仍在使用的），这样只有属主的进程能访问，无需端口和令牌。akio 的监听器只接受 `SocketAddr`：TCP 或 UDP。
在 akio 支持之前，akit 把 `unix/` 地址作为尚不支持拒绝。

**证据。** `crates/akio/src/lib.rs`（`a9a620ab`）：`Listener::tcp(SocketAddr)`、
`Listener::tcp_from_std(std::net::TcpListener)`、`Listener::udp(SocketAddr)`；没有 Unix 域监听器，
`Conn::peer_addr` 是 `SocketAddr`。

**建议的 API。** `Listener::unix(path)`（以及 `Listener::unix_from_std(UnixListener)`），服务同样的 acceptor
（akio-http 的 `serve`），其连接的对端地址为占位值或枚举；监听前设置套接字文件的权限（`Listener::mode(0o600)`），
残留文件由调用方或某个选项删除。支持 epoll、kqueue 和 io_uring。

**akit 的用法。** `admin.address` 为 `unix/<path>` 时，`akit_admin::Server::start` 在该路径上监听，与 fit 的
`admin.Start` 一样；那里不检查 Host，与 fit 一样。文件提供者的控制套接字（I29）也可能用到。

**已实现**（akio ADR-121）：`Listener::unix(path)`、`Listener::unix_from_std(UnixListener)` 与
`Listener::unix_mode(mode)`（在 `listen` 之前设置），支持 epoll、kqueue 和 io_uring；残留的套接字文件被替换，
正在使用的或不是套接字的文件返回 `AddrInUse`；地址为 `UNIX_ADDR`；文件留给调用方处理。akit 的 Admin API 在此
监听（0600），关闭时删除该文件。

## A24 关闭连接的 HTTP/1 响应

**状态。** 已在 akio 分支 `claude/akit-open-requests` 上实现（`22bc4402`，akio 的 ADR-157）；akit 已使用，尚未移动锁定。带 `Connection: close` 的 HTTP/1 响应无论从哪条路径发出，发送后都会关闭连接、暂停读取（不再读取被拒绝的请求体），也不再处理排在它后面的流水线请求。Admin API 的 401（`guard`）和 `read_body` 的 413 都带 `Connection: close`，与 fit 相同；`body_limit` 对流式请求体的 413 也是如此。测试：akit-admin 的 `token`（排在 401 后面的流水线请求不被处理）与 `loads_refused`（413 之后关闭连接，不再读取其余请求体），以及 akit-middleware 的 `body_limit_closes_http1`。

**问题。** 服务在读完请求体之前拒绝请求（流式读取中发现请求体超限而回 413，或流式路由上 `Content-Length` 已经超限），
无法在回答后关闭连接：akio-http 只根据请求决定是否保持连接，继续读取请求体的剩余部分（交给没人读的 sink 或丢弃），
并保留连接等待下一个请求。服务设置的 `Connection: close` 会被发出，但不生效。fit 的 Admin API 会关闭这样的连接
（Go 的 `MaxBytesReader`），客户端无法让服务器读取它已拒绝的无限长请求体；在 akio-http 下唯一的限制是请求体超时，
发送快的客户端永远触发不了。

**证据。** `crates/akio-http/src/session.rs`（`a9a620ab`）：`keep_alive` 来自请求的 `Connection` 和版本；
`State::Stream` 在响应之后仍继续交付数据块；`crates/akio-http/src/message.rs` 的 `Response::encode` 原样写出服务的
`Connection` 字段，`Responder::send` 只在 `!keep_alive` 时关闭连接。

**建议的 API。** 响应带 `Connection: close` 时生效（发送后关闭，并停止读取请求体），或提供
`Context::close_after_response()` / `Detached::close_after(..)`；对流式请求体，提供让 sink 停止交付的方法
（`ctx.stop_body()`），之后响应发出即关闭连接。

**akit 的用法。** Admin API 对超过 `max_config_bytes` 的加载回 413（I26）后关闭连接，与 fit 一样；
对没有令牌的请求回 401（I27）后也关闭连接，让客户端不能在一个连接上反复尝试令牌；I41 在数据面上的
大小限制同样如此。

## A29 不分配内存的应答钩子

**状态。** 已在 akio 分支 `claude/akit-open-requests` 上实现（`8caab91c`，akio 的 ADR-158）；akit 已使用，尚未移动锁定。`akio_http::ResponseObserver`、`HttpAcceptor::observer`、`Context::set_observed` 与 `akio_h3::serve_with_observer`。akit-listener 的 `Dispatcher::track` 让监听器的观察者凭两个字（图的 ID；指标槽位、路由下标和是否记日志）写访问日志行并计数，不分配内存。被追踪的请求（其 span 的 ID 放不进两个字）和路由可能改写目标的记日志请求（`rewrite`、`wasm`：`Middleware::changes_target`；日志行写的是监听器分派时的目标）仍用应答过滤器。认证出的 `user` 仍不记入日志：两个字装不下名字。

**问题。** 访问日志（I21）和指标（I37）要在发送应答的线程上得知每个应答，不论是哪个线程（代理的请求在上游
完成后才应答）。唯一的办法是 `Context::add_response_filter`，它把闭包装箱：带着任何东西（开始时间、指标槽、
日志分片）的闭包每个请求都要分配一次内存，而规则 13 要求热路径避免这样。fit 的分发器也用钩子计数，但 Go 的闭包
不比交给它的 `OnFinish` 多花什么。akit 已经在处理器返回后读 `Context::status`，不用钩子就能统计在处理器返回前
应答的请求（fit 的 inline 处理器、`respond`、404）；分离的应答（代理、在池上读取的静态文件）没有别的办法。

**证据。** `crates/akio-http/src/context.rs`（`a9a620ab`）：`add_response_filter` 执行
`self.fin.filters.push(Box::new(f))`；`Finish::filters` 内联保存两个，但每个都是 `Box<dyn FnOnce>`。akio-http
自己的 `HttpMetrics::middleware`（`observe.rs`）也这样分配，并且每个请求克隆一次 `Arc`。

**建议的 API。** 每个服务一个观察者，在构建监听器时设一次，请求在其 `Finish` 中内联携带几个字：

```rust
pub trait ResponseObserver: Send + Sync + 'static {
    /// 应答正在发送（在发送它的线程上），带着请求设下的字。
    fn on_response(&self, resp: &Response, req: &Request, words: [u64; 2], started: Instant);
}
akio_http::serve_with(cfg, svc).observer(Arc<dyn ResponseObserver>);
ctx.set_observed(words: [u64; 2]); // 不分配；此时记下 started
```

分离的上下文保留这些字，在其应答发送时调用同一个观察者。

**akit 的用法。** `akit_listener::Dispatcher` 把路由的指标槽和日志分片的序号设为这些字，其观察者写访问日志行
并统计请求：每个请求既不分配内存，也不碰引用计数。装箱的过滤器（`Dispatcher::track`）随之去掉；I64 的追踪也用
同一个钩子。

## A30 监听器接受的打开连接数

**状态。** 已在 akio 分支 `claude/akit-open-requests` 上实现（`57a942af`）；akit 已使用，尚未移动锁定。`Telemetry::open_accepted`（及 `Metrics::open_accepted`）：akit-listener 的抓取据此写 `akit_connections_open`（fit 的 `fit_connections_open`），只计接受的连接。

**问题。** fit 的 `fit_connections_open` 统计监听器上打开的客户端连接。akio 的 `Telemetry::open_connections`
统计所有打开的连接，包括引擎拨出的（akit 在监听器的引擎上拨出的上游连接池连接）和 UDP 对端，所以 akit 的这个
指标会多出它的上游连接；而按循环的计数（`accepted`、`dialed`、`closed`）也不区分关闭的连接属于哪一种。

**证据。** `crates/akio-core/src/telemetry.rs`（`a9a620ab`）：`conn_opened(limited)` 对每种连接都加到同一个
`open` 计数（`akio-epoll/src/reactor.rs` 和 `akio-uring/src/reactor.rs` 对拨出的连接以 `limited` 为 false 调用
它）；`conn_closed` 不分种类地减去。

**建议的 API。** `Telemetry::open_accepted()`（或 `LoopCounters` 中按循环的 `accepted_open` 指标）：现在打开的
已接受 TCP 连接，不含拨出的连接和 UDP 对端。

**akit 的用法。** `akit_listener::metrics::Scrape` 把它写作 `akit_connections_open`（fit 的 `fit_connections_open`），替代 `open_connections`。

## A32 正则表达式在哪里匹配、它的分组，以及替换

**问题。** fit 的 `rewrite` 中间件替换路径中正则表达式的每一处匹配（`regexp` 和 `replacement`，用 `$1` 和
`${name}` 表示分组），`redirect` 中间件用一次匹配的分组构造 URL（`"to": "/new/$1"`）。akio-regex 只回答模式是否
出现在文本中：既不找出位置，也不捕获分组。akit 以“暂不支持”拒绝这些字段（I40）。

**证据。** `crates/akio-regex/src/lib.rs`（`a9a620ab`）：`Regex` 只有 `is_match`、`is_match_fmt` 和
`nfa_is_match`；crate 文档说分组会被解析，但“不捕获任何内容”。Pike VM（`crates/akio-regex/src/exec.rs`）
不为线程保存位置。

**建议的 API。** 在 `Regex` 上，采用 Go 的 `regexp` 和 `regex` crate 的最左优先语义，像现在一样线性时间（带捕获槽的
Pike VM，或先用 DFA 找出匹配的结束和开始，再只为这次匹配计算槽）：

- `find_at(&self, text: &str, start: usize) -> Option<Range<usize>>` 和 `find_iter`；
- `captures(&self, text: &str) -> Option<Captures>`（每个分组的范围，按序号和名字）；`group_names`；
- `expand(&self, caps: &Captures, template: &str, text: &str, out: &mut String)`，模板规则与 Go 的
  `Regexp.Expand` 相同（`$1`、`${1}`、`$name`、`${name}`、`$$`；未知或未匹配的分组为空；格式不对的 `$` 是它自己），
  以及与 Go 的 `ReplaceAllString` 相同的 `replace_all(&self, text, template) -> Cow<str>`，包括空匹配。

**akit 的用法。** `rewrite` 的 `regexp` 和 `replacement`（在解码后的路径上 `replace_all`），以及 `redirect` 在
`to` 中引用的分组（在转义路径上 `captures`，再 `expand`），都在编译配置时准备好；akit-config 中“暂不支持”的拒绝
随之去掉。

**状态。** 已由 I48 在 akio 中实现（ADR-115，`26c500b5`）：按建议提供 `find`、`find_at`、`find_iter`、`captures`、
`expand` 和 `replace_all`，匹配与分组采用 Go 的最左优先语义（重复按 Go 的形状编译），在 132 000 个随机用例上与
Go 的 `regexp` 核对一致。

## A33 被中间件修改的请求：字段和请求目标

**问题。** 中间件在 handler 看到请求之前修改它：fit 的 `headers` 中间件设置、添加和删除请求字段，`rewrite`
中间件修改路径，之后的中间件（ForwardAuth 复制的字段、JWT 的 claims 字段）也会修改。akio-http 的 `Context`
只以共享引用提供请求（`req()`），字段和请求目标都不能修改。

**证据。** `crates/akio-http/src/context.rs`（`a9a620ab`）：`Context` 把请求放在私有的 `ReqCell` 中（在
`Detached` 或升级共享它之前归 `Context` 所有），只提供 `req(&self) -> &Request`、`path`、`header` 和 `local`；
没有 `&mut` 访问。`Request` 的 `headers` 和 `target` 是公有字段。

**建议的 API。** `Context::req_mut(&mut self) -> Option<&mut Request>`，请求被共享之后（`detach` 或升级之后）
返回 `None`；或者更窄的 `Context::request_headers_mut` 和 `Context::set_target(&mut self, target: &str)`。之后
akio 读取请求的地方（`Files` 的条件请求和范围请求字段、`path()`、路由器的参数）都能看到修改。

**akit 的用法。** `headers` 中间件在调用 handler 之前就地修改请求字段，`rewrite` 中间件修改请求目标；临时方案
随之去掉：保存在上下文 local 中的请求字段（`akit_runtime::REQUEST_FIELDS`，由 `request_headers` 读取），以及
保存在 `CANONICAL_PATH` 中的重写后路径（它只留给 I11 的规范化路径，或者也去掉）。在此之前，修改后的字段只到达
`proxy` handler 的上游（记录在 I40 的进度条目中）。

**已完成**（akio ADR-126，分支 `claude/a33-a48` 的 `2afb267b`，已锁定）：`Context::req_mut() ->
Option<&mut Request>`，请求被共享（`detach`、升级）之后返回 `None`；路由参数仍读取它们匹配时的路径；挂起的请求恢复时
带着完整的请求，可以修改。akit 的 `headers`、`basic_auth`、客户端证书、ForwardAuth 和 JWT 中间件以及缓存都就地修改
请求字段（`akit_runtime::set_request_headers` 替换它们）；`rewrite` 修改请求目标（`set_request_path`），akit-listener
把请求目标的路径规范化，于是 `REQUEST_FIELDS`、`request_headers` 和 `CANONICAL_PATH` 都去掉了：handler 读取
`ctx.req()` 和 `ctx.path()`。访问日志写的是 listener 留下的请求目标（fit 的 `RequestURI`，它的 `rewrite` 不改动它），
只为记录日志的请求复制一份。
I71 的 `wasm` 中间件在从插件的池恢复的请求上，通过它设置插件修改过的请求字段。

## A35 按长度分帧的流式 HTTP/1.1 响应

**状态。** 已在 akio 分支 `claude/akit-open-requests` 上实现（`22bc4402`，akio 的 ADR-157）；akit 已使用，尚未移动锁定。`stream_response` 按响应头给出的 `Content-Length` 为 HTTP/1.1 响应体分帧：不足该长度的响应体会关闭连接，超出长度的写入被拒绝，流式编码或响应体过滤器会去掉该长度并改用分块。代理在所有版本上都保留上游给出的长度（但在 HTTP/1.1 上带尾部字段的响应仍分块发送，只有分块才能携带尾部字段），FastCGI 处理器也保留应用给出的长度（不是数字的长度会被去掉）。测试：akit-proxy 的 `stream` 中的 `stream_download` 与 `broken_download`（按长度分帧；不足长度时被截断并关闭连接）。

**问题。** 流式转发长度已知的上游响应时，代理应像 fit 那样带着这个长度发出：客户端由此知道还有多少（下载进度、范围的
结尾），并能从分帧上分辨被截断的响应体和完整的响应体。akio-http 的服务端在 keep-alive 连接上把每个流式 HTTP/1.1 响应
（`stream_response`）都按 chunked 发送；随它给出的 `Content-Length` 字段会和 `Transfer-Encoding: chunked` 一起发出，
这是无效的，所以 akit 去掉了它。HTTP/2 和 HTTP/3 保留该字段，它们的 DATA 帧本身就足以分帧。

**证据。** `crates/akio-http/src/context.rs`（`60547435`）：只要 HTTP/1 响应是 keep-alive 的 1.1，`start_stream`
就设置 `Transfer-Encoding: chunked`，不看它的头部；`BodyStream::write` 把每块按 chunk 分帧。`message.rs` 的
`Response::encode` 不给流式响应加 `Content-Length`，但保留给定的。

**建议的 API。** `stream_response` 在 HTTP/1 上尊重给定头部中的 `Content-Length`：不用 chunked 分帧，各块原样写出，
`finish` 检查长度是否写满（没写满时关闭连接，被截断的响应体就该如此），超出长度的写入被拒绝。如果从头部读字段太隐晦，
也可以是含义相同的 `Context::stream_response_sized(status, headers, len: u64)`。

**akit 的用法。** I16 的代理把长度已知、超过 64 KiB 的响应流式发给 HTTP/1.1 客户端时带上它的 `Content-Length`（现在
按 chunked 发送，去掉该字段：记录在 I16 的进度条目中）。缓存（I67）在边读边提供已存储的响应时也会用到。

## A45 公开的定时器

**状态。** 已在 akio 分支 `claude/akit-open-requests` 上实现（`22e038cc`，akio 的 ADR-154）；akit 已使用，尚未移动锁定。`akio::timer::after_send` 与 `SendTimer::cancel`，akio 的一个有上限的定时器线程。akit 自己的定时器线程（`akit_upstream::timer`）已删除：探测、h2 连接池的空闲清扫、缓存的锁与磁盘超时、JWK 集的获取、FastCGI 的清扫、WebSocket 隧道的空闲超时、四层服务的开场超时、清扫与排空以及 sockmap 的排空都改用 akio 的，并在等待的事已无意义时取消定时器（探测已有结果、连接已被接走、获取已结束、磁盘读取已完成）。

**问题。** 主动健康检查（I35）每个 interval 探测上游的每个地址一次，TCP 探测还必须在超时到达时判为失败：需要有东西在
一段延迟之后运行回调。akio 有定时器，但在锁定的提交上没有一个是公开的：I/O 循环的时间轮（`akio-timer`）是内部的，
akio-http 的定时器线程（`crate::timer::after`，用来设置其客户端的请求超时）是 `pub(crate)`。网关还有更多用处：ACME 的
续期和重试（I31）、带退避的重试（I54）、控制面上的任何周期任务，否则每一处都要各自启动一个线程。

**证据。** `60547435` 上：`crates/akio-http/src/timer.rs`（`pub(crate) fn after`）；`crates/akio-timer`（"akio
internals ... (use `akio`)"），没有任何地方重新导出。akio 的 `claude/akio-all` 分支有一个 `akio_core::timer::after(delay, f)`，
挂在调用它的 I/O 线程的时间轮上，不在 main 上。

**建议的 API。** `akio::timer::after(delay, f: impl FnOnce() + Send + 'static) -> TimerHandle`：延迟过后在定时器线程
（或某个引擎的 I/O 线程）上运行 `f`，从不阻塞它；以及 `TimerHandle::cancel`。绑定引擎的形式 `Engine::after` 会在引擎的
某个 I/O 线程上运行 `f`，并在引擎停止时丢弃它。和 akio 的一切一样有界：待运行定时器有上限，超过时返回错误。

**akit 的用法。** `akit_upstream::timer` 是临时方案：akit 自己的一个线程，持有一个待运行任务的堆，上限 65,536（每个被
探测的地址最多有两个待运行任务：它的下一次探测和当前探测的超时）。有了这个 API 就替换它，探测的超时也改为取消，而不是
留着运行。

## A48 获知请求的结束：响应已完整发出，或客户端已离开

**问题。** fit 的 `concurrency_limit` 把请求计为进行中，直到它的响应发出或客户端离开，用的是 fib 的 `OnFinish` 和
`OnCancel`。akio-http 两者都不告诉服务：请求上唯一的钩子是响应过滤器（`Context::add_response_filter`），在响应头发出时
运行。流式响应（`stream_response`，akit 的代理对超过 64 KiB 的响应体都用它）在响应体之前就运行它，于是请求在响应体
还在发送时就离开了限制；而客户端在请求以 detach 方式等待时（上游很慢）离开，请求在处理器放开 `Detached` 之前什么也
听不到，于是它的名额一直占着，直到上游回答或超时。过滤器的闭包还是装箱的，每个请求一次分配（A29 已记录）。

**证据。** `crates/akio-http/src/context.rs`（`60547435`）：`Finish::apply` 在 `Out::send` 和 `start_stream` 中、
即发送响应头时运行过滤器；`BodyStream::finish` 和它的析构不运行请求的任何东西；`Detached` 和 `BodyStream` 在被使用之前
无从得知连接或流已关闭（`BodyStream::write` 只在写入时返回错误）。

**建议的 API。** `Context::on_end(f: impl FnOnce(End) + Send)`，在请求结束时运行一次：响应完整写出后（流式响应的最后一个
字节交给连接）为 `End::Sent`，连接或流先关闭时为 `End::Gone`，无论请求是在应答中、已 detach 还是都不是。接受几个字而
不是装箱闭包的形式（如 A29 的观察者）可以不经过分配器。`Detached::on_gone(f)`（或可轮询的 `is_gone()`）让等待中的
处理器在客户端离开时放弃上游的工作。

**akit 的用法。** `concurrency_limit` 在 `on_end` 时归还请求的名额，与 fit 的 `OnFinish`/`OnCancel` 一样；现在它在响应头
发出时，或请求未回答就被丢弃时归还（I41 进度条目中记录的临时方案）。代理（I17）在 `Gone` 时取消上游请求；访问日志
（I21）和指标（I37）可以把流式响应的耗时算到它结束。

**已完成**（akio ADR-127，分支 `claude/a33-a48` 的 `2afb267b`，已锁定）：`Context::on_end(f) -> bool`、
`Detached::on_end(f)` 和 `End { Sent, Gone }`，每个钩子恰好运行一次（响应完整交给连接后为 `Sent`，流式响应在完成时；
客户端先离开时为 `Gone`，请求等待或流式发送时都能得知，或者响应被放弃）；`on_gone` 与它们并存。`concurrency_limit`
在 `on_end` 时归还名额。代理已经在 `on_gone` 时取消上游请求（A38）。仍未完成：访问日志、指标和 span 把流式响应的耗时
算到响应头，大小写的是它的 `Content-Length`；fit 在 `OnFinish` 时写出它们，带着已写出的字节数，而 `End` 不带这个数
（body tap 可以计数，但每个流式响应都要付出代价）：留作后续。

## A49 由中间件限制请求体大小

**状态。** 已在 akio 分支 `claude/akit-open-requests` 上实现（`22bc4402`，akio 的 ADR-157）；akit 已使用，尚未移动锁定。`Context::limit_body` 为读取请求体的任何一方限制请求体。超过限制时，流式请求体的 sink 收到新增的 `BodyEvent::TooLarge` 后被丢弃，请求以 413 应答，除非处理器已经拿走了响应。`BodyEvent` 新增变体是 akio 的破坏性变更（该枚举是穷尽的）：akit 的每个 sink 都匹配了它（代理、WebSocket 隧道、FastCGI 处理器、Admin API 和测试中的 sink）。akit-listener 用 `limit_body(max_body_bytes)` 限制每个流式请求体，但 CONNECT 的除外（隧道或扩展 CONNECT 的流不是请求体）；`body_limit` 对每个请求调用 `limit_body(max_bytes)`，在此之前由自己用 fit 的措辞应答声明长度超限或缓冲后超限的请求体（akio 自己的 413 写的是 "Payload Too Large"）；代理和 FastCGI 处理器收到 `TooLarge` 后应答 413，若响应已开始则截断响应，并放弃上游请求。akit-runtime 的局部值 `MAX_BODY_BYTES`（处理器曾据此限制流式请求体）已去掉。测试：akit-middleware 的 `body_limit`、`body_limit_streamed`、`body_limit_h2`（h2c：低于、等于和超过限制，声明长度和逐渐增长，发给缓冲请求体的处理器和代理）与 `body_limit_closes_http1`；akit-handler 的 `streamed_body_limit`（FastCGI 处理器，HTTP/1.1 和 h2c）。

**问题。** fit 的 `body_limit`（fib 的 `Context.LimitBody`）为读取请求体的任何处理器限制请求体：流式请求体超过限制时，
在处理器的 `OnBody` 中以 `ErrBodyTooLarge` 失败并回 413，HTTP/1 连接关闭而不是继续读剩下的部分。在 akio-http 中，只有
监听器的 `HttpConfig::max_body_bytes` 限制缓冲的请求体；流式请求体（`BodyMode::Stream`）不受限制地交给处理器的 sink，
中间件看不到也无法包装这个 sink（`Context::on_body` 会替换它；sink 是私有的）。

**证据。** `crates/akio-http/src/context.rs`（`60547435`）：`on_body` 保存一个 sink，`deliver` 是 crate 私有的；
`crates/akio-http/src/session.rs`：`max_body_bytes` 的检查只作用于缓冲的请求体及其声明的长度。

**建议的 API。** `Context::limit_body(&mut self, max: u64)`，可在处理器设置 sink 之前或之后调用，降低本请求的上限：
超过后 akio-http 以错误事件（`BodyEvent::TooLarge`，或与 `Chunk`、`End` 并列的 `Err`）结束交付，除非响应已经开始，
否则回 413，并在 HTTP/1 上于回答发出后关闭连接而不是继续读（配合 A24）；HTTP/2 和 HTTP/3 重置该流。

**akit 的用法。** `body_limit` 对每个请求调用它，去掉临时方案：没有声明长度的流式请求体只由读取 akit-runtime 的
`MAX_BODY_BYTES` 本地值的处理器（代理）限制，中间件会调低该值；缓冲的请求体在全部到达后检查（I41 进度条目中记录的
临时方案）。监听器自己对流式请求体的 `max_body_bytes` 也改用它。

## A36 HTTP 客户端请求的响应头超时和空闲超时

**问题。** fit 用三种方式限制一次上游交换（它的 `response_header_timeout`、`response_idle_timeout` 和
`response_timeout`）：上游开始响应最多可用多久（超过则 504），响应体中途最多可静默多久（超过则截断响应），以及整个
交换。akio 的 HTTP/1 客户端只有最后一种（`ClientRequest::timeout`，对 `request_with` 计到 `on_end` 为止）：代理无法
对迟迟不开始响应的上游回答 504 而又让长下载继续，也无法在整整一小时到期前切断中途停滞的上游。akit 在 I/O 线程上没有
自己的定时器可用（akio 的定时器是内部的），而 akit 自己的定时器线程还需要在响应头到来前取消请求，akio 同样做不到
（A37）。

**证据。** `crates/akio-http/src/client.rs`（`60547435`）：`ClientRequest` 只有 `timeout`；`ClientError::Timeout`
是唯一的超时错误。ADR-101 写明流式响应“没有空闲间隔超时”。

**建议的 API。** `ClientRequest::header_timeout(d)`：从请求头写出（流式请求体可能仍在发送）到响应头到达；超过则请求以
`ClientError::Timeout`（或 `HeaderTimeout` 类）失败并关闭其连接。`ClientRequest::idle_timeout(d)`：两次读取响应体
之间的最长间隔；超过则以同样的错误调用 `on_error` 并关闭连接。两者都适用于 `request`、`request_with` 和
`request_streamed`，`H2Client` 也一样（I18）。响应体被挂起（`ResponseBody::hold_reads`）的时间不算空闲：是客户端自己
选择停止读取。

**akit 的用法。** I17b：上游的 `response_header_timeout` 和 `response_idle_timeout`（fit 的默认值 60s）设到每个
被代理的请求上；响应头超时回答 504，空闲超时像中途断开那样截断响应（对 HTTP/2 和 HTTP/3 上未声明长度的响应体，用
I17 的 `Akit-Error`（fit 的 `Fit-Error`）尾部字段结束）。

## A37 可在任意时刻取消的 HTTP 客户端请求

**问题。** 客户端离开时，代理放弃它的上游请求（fit 的 `OnCancel`）。akio 的客户端在响应头到来后可以放弃响应
（`ResponseBody::cancel`），流式请求可以通过丢弃未完成的 `RequestBody` 放弃；但请求体整体发出的请求（大多数请求）在
响应头到来前没有句柄：`Client::request` 和 `request_with` 什么也不返回。上游还在处理时离开的客户端，会让上游继续工作，
并占着 akit 的一条连接和一个 `max_conns` 名额，直到上游回答或 `response_timeout` 到期（默认 1h）。

**证据。** `crates/akio-http/src/client.rs`（`60547435`）：`request`、`request_with` 和 `request_streamed` 返回
`()`；`ResponseBody::cancel` 随 `on_head` 才到。

**建议的 API。** `request_with`（和 `request`）返回 `RequestHandle`（`Clone + Send`），带 `cancel()`：请求发出前
取消则直接丢弃；发出后则关闭其连接（HTTP/2 上重置其流）；处理器收到一次 `on_error(ClientError::Cancelled)`，除非它已
收到 `on_end` 或 `on_error`。从别的线程取消时投递到连接的 I/O 线程，和 `ResponseBody::cancel` 一样。

**akit 的用法。** I17b：代理把句柄和它的应答放在一起，在客户端离开时取消（A38）；I45 的重试用它取消放弃的那次尝试。

## A38 能得知客户端离开的分离服务端请求；可在运行时修改的写停滞超时

**问题。** 服务端的两个缺口，都与离开或停下的客户端有关：

1. 分离的请求（`Context::detach`）在写之前得不到客户端离开的任何消息：等待上游响应头的代理无法为已离开的客户端取消
   上游请求（A37）。更糟的是，在 HTTP/1 上，请求体流式到达（`BodyMode::Stream`）的请求在连接关闭时仍保留其请求体
   接收器：HTTP/1 会话把请求的上下文（也就包括接收器）放在它的状态里，而只要分离的应答还握着连接，会话就一直存在，
   所以接收器既没有得到通知，也没有被丢弃。HTTP/2 会丢弃已关闭连接的流以及它们的接收器，akit 的代理（I17）正是靠这个
   取消 h2 或 h3 客户端的上传；中途离开的 HTTP/1 上传客户端要到交换结束才会被察觉。
2. fit 的 `write_stall_timeout`（30s）：响应等待客户端取走已发送内容的最长时间。akio 的 `Config::write_timeout`
   正是这个界限（有输出排队而一字节也写不出），但它属于整个引擎，构建时固定（`Live` 能改 `idle_timeout` 等，不能改它）：
   akit 无法在重载时应用配置的值。

**证据。** `crates/akio-http/src/session.rs`（`60547435`）：`Session::on_close` 保留 `State::Stream { ctx, .. }`
不动；`h2.rs` 的 `on_close` 会清空流。`context.rs`：`Detached` 只有 `respond` 和 `stream_response`。`akio-core` 的
`telemetry::Live` 没有 `set_write_timeout`。由 akit 的 `failure.rs` 中客户端中途离开上传的测试发现（I17）。

**建议的 API。** (1) `Detached::on_gone(f)`（分离前为 `Context::on_gone`）：请求的连接关闭、或其流在响应完成前被
重置时，`f` 在连接的 I/O 线程上运行一次；如果已经发生则立即运行。HTTP/1 会话在连接关闭时丢弃流式请求的接收器（或以
`BodyEvent::Aborted` 结束它），和 HTTP/2 一样。(2) `Live::set_write_timeout(Option<Duration>)`，与 `idle_timeout`
相同。

**akit 的用法。** I17b：代理注册 `on_gone` 来取消上游请求（A37），且不把它计为上游的失败，补上 fit 的
`TestClientAbort` 中“响应头之前”和 HTTP/1“上传中途”两种情况；`limits.write_stall_timeout` 在启动和重载时设置引擎的
写超时。I49 的 WebSocket 和隧道转发也用同样的方式得知客户端离开。

## A54 空闲的 HTTP/1 连接在宽限期后排空，而非立即关闭

**状态。** 已在 akio 分支 `claude/akit-open-requests` 上实现（`8caab91c`）；akit 已使用，尚未移动锁定。`HttpConfig::idle_drain_grace`（默认 250 ms；fit 没有这个选项，akit 用默认值）。akit 的重载测试（`crates/akit/tests/reload.rs`）不再把在排空中的监听器的空闲连接上发出的请求算作重试：客户端的任何错误都算失败。

**问题。** 监听器排空时（`Engine::shutdown_listener`、`Engine::shutdown`），akio-http 立即关闭空闲的 HTTP/1
keep-alive 连接。客户端可能已经在这条连接上发出了下一个请求，或者正在发：请求遇到关闭而得不到应答，客户端看到的是
重置或文件结束。HTTP/1 无法告诉空闲的客户端它的连接要关了（HTTP/2 有 `GOAWAY`），所以关闭本身与线路上每个在途请求
竞争。akit 排空重载移除或改变的监听器（I22、I28），所以此刻正在用被移除或被替换的监听器的客户端，除非重发请求（HTTP
客户端对幂等请求可以这样做，RFC 9112 第 9.3.1 节），就会看到一个失败的请求。

**证据。** `crates/akio-http/src/session.rs`（`afbbb521`）：`on_shutdown` 运行 `Session::shutdown`，其 HTTP/1
路径是 `h1_shutdown`；响应顺序中没有响应时，`reserve_response` 立即交出一个槽位，`fill_close` 关闭连接。akit 的
`crates/akit/tests/reload.rs`（I22）统计其客户端在这种关闭后重发的请求（每次替换每个客户端至多一个），因为不重发就会
失败。

**建议的 API。** 排空时给空闲的 HTTP/1 连接一个宽限期：空闲连接最多保持打开这么久，期间到达的请求以
`Connection: close` 应答，响应后关闭连接；期间没有请求到达则在宽限期结束时关闭。期间收到部分请求的连接视为忙碌。
宽限期不超过排空的截止时间：`ServerConfig::idle_drain_grace(Duration)`（默认几百毫秒，0 为现在的行为），或
`shutdown_listener` 的一个参数。排空开始时已在套接字接收缓冲区里的请求无论如何都会被读取并应答。

**akit 的用法。** `akit_listener::Listeners::reload` 以 `-shutdown-timeout` 排空移除和改变的监听器时用它；
`akit/tests/reload.rs` 中的重发去掉，其客户端把每个错误都计为失败。它把竞争缩小到只在宽限期之后才发下一个请求的客户端，
只有空闲了那么久的客户端才会这样。

## A51 `H2Client` 的流式请求：重置、超时、trailer、`GOAWAY`

**问题。** 到 h2 上游的代理以流的方式转发每个交换（`H2Client::open`），和 HTTP/1 一样
（`Client::request_with`，A19、A20）。HTTP/1 路径具备、h2 路径缺少的有四点：

1. **无法重置。** `open` 返回的 `H2Upload` 只有 `write` 和 `finish`：客户端已离开时，代理无法重置上游的流
   （`RST_STREAM CANCEL`）。`H2Conn::cancel` 存在，但只有整体请求的超时能用到它。
2. **没有超时。** 只有 `request` 会启用 `ClientRequest::timeout`；`open` 忽略它。
3. **没有请求 trailer。** `H2Upload` 无法用结尾的 HEADERS 帧结束请求体，因此请求的 trailer（包括 gRPC 的，I50）
   无法到达 h2 上游。
4. **遇到 `GOAWAY` 时被遗忘。** `GOAWAY` 判定不再处理的流（高于其最后流 id）会被移除并让其回调失败，但流式请求的
   `on_event` 不会收到通知：它会永远等待。而且在 `GOAWAY` 之后客户端仍报告 `is_open`，而每个新流都以 `Closed` 失败。

**证据。** akio `afbbb521` 的 `crates/akio-http/src/h2client.rs`：`open` 返回 `H2Upload { write, finish }`；
只有 `request` 为超时调用 `crate::timer::after`；`H2Conn::cancel` 是私有的；`handle` 的 `GOAWAY` 分支调用
`s.cb.take()`，从不调用 `s.events`；`is_open` 就是 `conn.is_open()`。由 I18 经 h2c 的代理发现
（`crates/akit-upstream/src/h2.rs`）。

**建议的 API。** `H2Upload::cancel()`（或 `open` 同时返回的句柄上的同名方法）：以 `CANCEL` 重置该流，
`on_event` 收到一次 `H2Event::Error(ClientError::Cancelled)`，除非它已结束。`open` 像 `request` 一样启用
`req.timeout`：超时后重置并报 `Error(Timeout)`。`H2Upload::finish_with_trailers(Headers)`。遇到 `GOAWAY` 时，
不再处理的流式请求收到 `Error(Closed)`（若有 `Refused` 类别，调用方便知道可以换处重试），且收到 `GOAWAY` 后
`H2Client::is_open`（或新的 `accepts_streams`）为 false。

**akit 的用法。** I18b：`akit_upstream::h2` 去掉它的权宜之计：放弃的流（客户端离开、超时、客户端太慢）被重置，
而不是任其运行、让其连接停收新流并在其他流结束后关闭；用来执行 `response_timeout` 的按地址定时检查不再需要；
请求 trailer 会发往上游；`GOAWAY` 让其不再处理的流失败（502，或 I45 的重试）并让该连接停收新流。I50 的 gRPC
代理四点都需要。

**已完成**（akio ADR-110，已在 akio 的 main `f6750e93`，I18b 已锁定）：`H2Upload::stream()` 给出
`H2Stream`，其 `cancel` 重置该流（`H2Upload::cancel` 亦然）；`open` 启用 `ClientRequest::timeout`；
`H2Upload::finish_with_trailers`；`GOAWAY` 判定不处理的流以及其后的请求以新增的 `ClientError::Refused`
失败，收到 `GOAWAY` 后 `is_open` 为 false。I18b 四项都已使用。

## A52 `H2Client` 按流的背压

**问题。** 规则 8 要求背压联动：慢客户端暂停上游的读取，慢上游暂停客户端的读取。在 HTTP/1 上，akio 的
`ResponseBody::hold_reads` 和 `RequestBody::queued_bytes` / `on_drain` 做到了这一点（A19、A20）。akio 的 h2
客户端两者都没有：

1. DATA 一到就把每个流的接收窗口还回去（每帧都 `window_update`），所以无论消费方如何，服务器都以连接允许的
   最快速度发送流式响应。客户端较慢的代理只能缓冲它，或放弃这个流。
2. `H2Upload::write` 无界排队且不报告任何信息：字节在流的 `upload` 队列中等待窗口打开，生产方无从得知有多少。

**证据。** akio `afbbb521` 的 `crates/akio-http/src/h2client.rs`：`handle` 的 `DATA` 分支对每个不结束流的帧调用
`window_update(&mut out, sid, total)`；`H2Upload::write` 推入 `UpShared::q`，`pump` 再把它移入
`CStream::upload`（一个 `VecDeque<u8>`），外部没有任何计数。

**建议的 API。** 在 `open` 建立的流上：`hold_reads(bool)`：暂停期间不归还该流的窗口（已在途的 DATA 仍会到达，
至多一个窗口）；恢复时立即归还已消费的部分。可选 `consume(n)`，供边转交字节边归还窗口的调用方使用。在
`H2Upload` 上：`queued_bytes()`（已接受、尚未放入 DATA 帧的字节）和 `on_drain(f)`（降到 0 时调用一次，或立即调用），
并且超过 `max_queued_bytes` 时 `write` 失败，与 `RequestBody` 一致。

**akit 的用法。** I18b：代理的转发在客户端落后时暂停 h2 下载，并按队列控制上传节奏，与 HTTP/1 完全相同；akit 的
权宜之计（暂停的下载超过 `HELD_LIMIT`（4 MiB）后被切断，上传队列可增长到请求体上限）不再需要。

**已完成**（akio ADR-110，`f6750e93`）：`H2Stream::hold_reads` 在暂停期间不归还流的窗口；
`H2Upload::try_write`、`queued_bytes` 和 `on_drain`，受 `H2Options::max_queued_bytes` 限制。未做：
`consume(n)`。I18b 用它们暂停下载、为上传控制节奏，与 HTTP/1.1 一样。

## A53 `H2Client` 的连接：服务器的流数上限、连接超时、ALPN 回退

**问题。** h2 连接池在有空闲流的连接上开流，没有时再拨一个新连接：它必须知道服务器允许多少个流。它还要按上游的
`connect_timeout` 限制连接时间，并且对 fit 的 `h2`，在服务器未通过 ALPN 选择 h2 时回退到 HTTP/1.1。

1. akio 的客户端忽略 `SETTINGS_MAX_CONCURRENT_STREAMS`（设置 3），也不报告它：超出的请求会被服务器拒绝
   （`REFUSED_STREAM`）。akit 假定为 100，即 RFC 9113 允许服务器公布的最小值。
2. `H2Client::connect` 用 `HappyEyeballs::default()` 竞速各地址，调用方无法设置其 `connect_timeout`；`dial` 使用
   引擎的超时（与 HTTP/1 客户端的 A12 相同）。
3. 在 TLS 上，无论 ALPN 选了什么，`dial` 都说 h2；选择 `http/1.1` 的服务器会失败。

**证据。** akio `afbbb521` 的 `crates/akio-http/src/h2client.rs`：`handle` 的 `SETTINGS` 分支匹配 1、4、5 和 8，不含 3；
`connect` 构造 `akio::dns::HappyEyeballs::default()`；`dial` 传入 TLS acceptor，不查看协商出的协议。

**建议的 API。** `H2Client::max_concurrent_streams()`（服务器最新的值，收到其 SETTINGS 之前为 `u32::MAX`）和
`open_streams()`；超出上限的请求由调用方选择在客户端中（有界地）等待，或立即以 `Busy` 失败。
`H2Client::connect_with(engine, host, port, tls, opts, HappyEyeballs, ready)`。经 TLS 拨号时报告 ALPN 选择的协议
（`ready` 收到 `enum { H2(H2Client), Http1(Conn) }`），或在不是 `h2` 时以明确的错误失败。

**akit 的用法。** I18b：`akit_upstream::h2` 按服务器的上限确定每个连接的流数，在上游的 `connect_timeout` 内拨号，
并且对 `h2`，在服务器不说 h2 时回退到 akio 的 HTTP/1 客户端，与 fit 的 `h2` 一致（I54 加入 `tls_*` 字段）。

**已完成**（akio ADR-110，`f6750e93`）：`H2Client::max_concurrent_streams`、`open_streams` 和
`waiting_streams`，超过上限的请求在客户端中等待（`H2Options::max_pending_streams`，否则
`ClientError::Busy`）；`connect` 使用 `H2Options::happy_eyeballs`；TLS 上若 ALPN 没有选择 `h2`，
拨号以 `NotH2 { alpn }` 失败。未做：`connect_with` 与 `enum { H2, Http1 }`（ADR-110 已否决：竞速属于
`H2Options`，回退时拨 akio 的 HTTP/1 客户端）。I18b 按服务器的上限确定连接容量，在 `connect_timeout`
内对地址竞速（单个地址仍用引擎的超时，A12），遇到 `NotH2` 时对 `h2` 地址经 TLS 说 HTTP/1.1。

## A57 握手之外可用的根证书

**问题。** fit 只在来自文件的证书的证书链能用系统根证书（`x509.SystemCertPool`）校验通过时，才为它装订 OCSP 响应：
用别人给的内容写成的证书（租户的 Kubernetes Secret）不能让网关去访问它指定的响应者，除非它是公共 CA 签发的。akit-tls
用 akio-x509 的 `Verifier` 校验证书链，它接受 `akio_x509::RootStore`；akio-tls 能加载系统根证书（`RootCerts::system()`，
ADR-010 与 ADR-059：macOS 上读钥匙串，其他系统读发行版的证书包），但这个存储是 `RootCerts` 私有的，只能在握手中使用。

**证据。** `crates/akio-tls/src/config.rs`（`afbbb521`）：`pub struct RootCerts(akio_x509::RootStore)` 没有访问方法；
`RootCerts::system()` 用私有的 `crate::system::load()` 构建存储。由 I32（`akit-tls/src/ocsp.rs`）发现。

**建议的 API。** `RootCerts::store(&self) -> &akio_x509::RootStore`（或 `impl AsRef<RootStore>`、
`From<RootCerts> for RootStore`）；或与 akio-tls 共用加载器和缓存的 `akio_x509::RootStore::system()`。

**akit 的用法。** `Ocsp` 在自己的工作线程上用 `RootCerts::system()` 的存储校验来自文件的证书，和 fit 的 `OCSP`
默认的做法一样；运维给出的根证书（`Ocsp::trust_roots`）仍然是替代。在此之前，来自文件的证书只在给出根证书时装订，
所以 `akit run` 为 ACME 的证书装订、不为文件中的证书装订（I32 进展记录中的临时方案）。

**已完成**（akio ADR-123，分支 `claude/a57-a58-a14-a15-a92` 的 `7d1b1670`，已锁定）：
`RootCerts::store()` 与 `From<RootCerts> for RootStore`。akit 的 `Ocsp` 在没有给出根证书（`trust_roots`）时，
对系统根证书验证来自文件的证书，与 fit 相同。

## A58 HTTP 服务能看到 TLS 连接协商出的 ALPN 协议

**问题。** 通过 TLS-ALPN-01 获取证书的监听器在 ALPN 中把 `acme-tls/1` 放在最后（RFC 8737），这样只有单独提供它的、
正在检查挑战的 CA 会协商出它。这样的连接只用于握手：fit 在握手完成后关闭它（fib 的服务端看到 `NegotiatedProtocol`，
不在上面提供 HTTP）。akio-http 的服务端靠前言而不是 ALPN 识别 HTTP/2，也不把 TLS 连接的任何信息交给服务，所以协商出
`acme-tls/1` 后又发送 HTTP/1 请求的客户端会得到服务。这不会泄露什么（握手已经给了它挑战的自签名证书），但和 fit
的做法不同，按需 TLS（I34）也有同样的情况。

**证据。** `crates/akio-http/src/session.rs` 与 `context.rs`（`afbbb521`）：`Service` 无法访问连接的 `TlsInfo`
（`alpn`）；`akio-tls` 的 `Connection::alpn()` 是有的。

**建议的 API。** 要么 `Context::tls()`（请求所在连接协商出的 ALPN 协议和 SNI 名称，明文时为 `None`），要么 `HttpConfig`
的一个设置：握手完成后，关闭协商出的协议不是服务端所讲协议（`http/1.1`、`h2`、无）的 TLS 连接。

**akit 的用法。** 分发器不作应答地关闭 ALPN 为 `acme-tls/1` 的连接（或由 akio 关闭），和 fit 的监听器一样；目前它
被当作 HTTP/1.1 服务。

**已完成**（akio ADR-123，分支 `claude/a57-a58-a14-a15-a92` 的 `7d1b1670`，已锁定），作为 TLS 设置
`ServerConfigBuilder::handshake_only_alpn`：协商到 `acme-tls/1` 的连接在握手完成后关闭，它发送的内容都不会
被处理。akit 的 TLS 监听器设置了它。

## A63 bcrypt 密码哈希

**问题。** fit 的 `basic_auth` 用 bcrypt 哈希（`htpasswd -B` 和 fit 自己的 `hash-password` 生成的 `$2a$`、
`$2b$`、`$2y$`）校验密码。`akio-crypto` 有 PBKDF2 和 HKDF，没有 bcrypt，网关也不应自带一份 Blowfish。

**依据。** `afbbb521` 的 `crates/akio-crypto/src/lib.rs` 和 `kdf.rs`：没有 bcrypt 或 Blowfish。

**建议的 API。** `akio_crypto::bcrypt`：`Hash::parse(&str) -> Result<Hash, ParseError>`（错误中给出原因）、
以常数时间比较的 `Hash::verify(&self, password) -> bool`、`cost`、`Display`；`hash(password, cost)` 与
`hash_with_salt`。与 Go 一致：只读密码的前 72 字节，各次版本号同样处理。

**akit 的用法。** akit-config 用 `Hash::parse` 检查 `password_hash` 并说明不是哈希的原因；`basic_auth` 在
akio 的逻辑线程池上用 `verify` 校验密码（A65）。

**已完成。** 按建议实现，见 `5e0d433c`（ADR-109）：Go、`htpasswd -B` 与 OpenBSD 的测试向量；Blowfish 的表
由 π 的各位数字生成，并与 Go 的表核对。

## A64 每个服务端请求上的客户端已验证证书

**问题。** fit 的 `client_cert` 只放行客户端发送了、且监听器已验证（`tls.client_auth`）证书的请求，并把
证书主题传下去。akio-http 的处理器拿到的是 `Context` 而不是连接；`akio_tls::TlsInfo` 不带对端证书链，
`akio_quic::Connection` 也不提供。

**依据。** `afbbb521` 的 `crates/akio-tls/src/lib.rs`：`TlsInfo` 有 ALPN、SNI、版本、早期数据和 OCSP，没有
证书链，尽管协议核心保存了它（`peer_certificates`，会话恢复时也会还原）；`crates/akio-http/src/message.rs`：
`Request` 只有 `peer`。

**建议的 API。** `Request::peer_certificates: Option<Arc<[Vec<u8>]>>`：客户端发送并经握手验证的证书链，
设置在 HTTP/1.1、HTTP/2 和 HTTP/3 请求上，同一连接的请求共享；另有 `TlsInfo::peer_certificates` 和
`akio_quic::Connection::peer_certificates`。

**akit 的用法。** `client_cert` 拒绝没有证书的请求（403，除非 `optional`），并把叶证书的主题按 Go 的
`pkix.Name.String` 的写法写入 `subject_header`。

**已完成。** 按建议实现，见 `5e0d433c`（ADR-109），另有 `akio_tls::peer_chain`（`PeerChain`），服务端在连接
握手完成后取一次；以 TLS 1.3 早期数据到达的请求没有证书。

## A65 由中间件挂起、之后带着上下文恢复的请求

**问题。** 一次 bcrypt 校验要花几毫秒到几十毫秒的 CPU，`basic_auth` 不能在 I/O 线程上校验密码（规则 1）。
fit 在线程池里校验，请求在此期间等待，之后继续执行下一个中间件和处理器。在 akio 中，中间件可以分离响应并在
线程池里作答（`Context::detach_to_pool`），却无法继续执行链：下一个处理器要的是 `Context`，分离的请求没有。
`run_on_pool` 则把监听器的所有请求都搬到线程池。ForwardAuth（I43）有同样的需求：请求等待授权服务器，之后
继续。

**依据。** `afbbb521` 的 `crates/akio-http/src/context.rs` 和 `pool.rs`：`detach` 给出 `Detached`（只能作答
或流式作答）；`call_on_pool` 把整个上下文移到线程池，但它是 crate 私有的，且只用于开了 `run_on_pool` 的监听器
的全部请求。

**建议的 API。** `Context::suspend(&mut self) -> Option<Suspended>` 与
`Suspended::resume(self, impl FnOnce(&mut Context))`，可在任意线程调用，并像服务端在 `call` 之后那样收尾；
`Context::suspend_to_pool(work)`，线程池满时回 `503`。

**akit 的用法。** `basic_auth` 挂起密码尚未记住的请求，在线程池上校验，再恢复它进入下一个中间件和处理器，
或回 401。I43 的 ForwardAuth 在授权请求完成时恢复请求。

**已完成。** 按建议实现，见 `5e0d433c`（ADR-109）。消息体流式交给处理器的请求不能挂起（akio 在 `call` 之后把
消息体交给该上下文，与 `run_on_pool` 的界线相同）：`basic_auth` 只对带消息体、且密码尚未校验过的请求要求整体
读取消息体（在监听器的 `max_body_bytes` 之内）。以后的版本可以让挂起的请求改为暂停其消息体的读取。

## A75 挂起消息体流式读取的请求

**状态。** 已在 akio 分支 `claude/akit-open-requests` 上实现（`a88c5d7b`，akio 的 ADR-159）；akit 已使用，尚未移动锁定。`Context::suspend` 暂停流式消息体的读取，直到恢复后的处理器接手。`forward_auth`、`basic_auth`、`jwt` 与 `wasm` 不再为挂起请求而要求完整读取消息体；akit-middleware 测试中的 `forward_auth_streams_the_body` 检查 1 MiB 的上传流式到达代理的上游，完整且有序，上游在客户端发出后一半之前就已收到请求。

**问题。** `forward_auth`（I43）和 `basic_auth`（I42）在等待期间挂起请求（`Context::suspend`，A65），之后
恢复它进入下一个中间件和处理器。akio 不能挂起消息体流式交给处理器的请求（`BodyMode::Stream`）：它在 `call`
之后把消息体交给该上下文，上下文因此不能移动。于是这两个中间件都要求整体读取这样的消息体（在监听器的
`max_body_bytes` 之内，默认 16 MiB，或配置所设的值）：在它们之后，代理的上传不再流式，占用的内存是整个消息体
而不是一次读取的量（规则 5 和 8）。fit 的 `forward_auth` 让消息体保持流式：请求被保留等待，消息体不读。

**依据。** `4f7169c4` 的 `crates/akio-http/src/context.rs`：`self.streamed` 时 `Context::suspend` 返回
`None`；`Service::body_mode` 在 `call` 之前询问，一个尚不知道是否会挂起的中间件只能对所有消息体都要求整体
读取。

**建议的 API。** 让 `Context::suspend` 也能挂起消息体流式读取的请求：从挂起起暂停消息体的读取（如同
`BodyReads::hold(true)`），直到恢复后的上下文的处理器注册接收端（`on_body`）并放开读取；此前已到的数据
（至多一次读取的量）留给它。客户端在此期间离开时，像其他挂起的请求一样运行该上下文的 `on_gone`。

**akit 的用法。** `forward_auth`，以及密码尚未记住时的 `basic_auth`，不再要求整体读取消息体：消息体流式
读取的请求被挂起并暂停读取，检查放行后，其后的代理再流式上传。

## A66 `H2Client` 的请求遵守首部超时、空闲超时和 `CancelToken`

**问题。** ADR-108（A36、A37）为 akio 的 HTTP/1 客户端加上了请求的 `response_header_timeout`、
`response_idle_timeout` 和 `cancel`（`CancelToken`），但没有包括 `H2Client`：它忽略这三个字段，就像忽略
`streamed` 一样。到 h2 上游的代理用 fit 同样的超时约束每个交换，并在客户端离开时放弃请求，无论请求处于哪个阶段；
在 h2 上它没有可以交给的东西。A51 要求的是流式请求的重置和整体超时；这里是 HTTP/1 客户端现在能做的其余部分。

**证据。** akio `4f7169c4`：`ClientRequest::response_header_timeout`、`response_idle_timeout` 和 `cancel`
的文档写明"仅 HTTP/1 客户端"；`crates/akio-http/src/h2client.rs` 一个都不读（ADR-108 的"被否决：本次变更不含
`H2Client`"）。由 I17b 经 h2c 的代理发现（`crates/akit-upstream/src/h2.rs`）。

**建议的 API。** 不需要新 API：`H2Client::open` 和 `request` 像 `Client` 一样遵守请求的这三个字段。首部超时
从请求的 END_STREAM（请求体结束）算到最终响应的 HEADERS；空闲超时是两个 DATA 帧之间的最长间隔，在该流的读取
被暂停时停止计时（A52）；令牌被取消时以 `CANCEL` 重置该流。每种情况都以 `ClientError::HeaderTimeout`、
`IdleTimeout` 或 `Cancelled`（`H2Event::Error`）让该流失败一次，连接留给它的其他流。

**akit 的用法。** I18b：`akit_upstream::h2` 的按地址定时检查不再逐流检查首部、静默和令牌（I17b 的权宜之计，
在一个 100ms 的 `TICK` 之内），因其中之一而放弃的流会被重置，而不是任其运行、让其连接停收新流。

**已完成**（akio ADR-111，`d27e6a20`，由 I18b 的会话实现）：按建议，不加新 API。`H2Client::request`
与 `open` 遵守这三个字段：响应头超时从请求的 END_STREAM 到最终的 HEADERS，空闲超时为 DATA 帧之间的间隔
（暂停读取的时间不计），`CancelToken` 现在也覆盖 h2 流；每一种都只重置该流，并以 `HeaderTimeout`、
`IdleTimeout` 或 `Cancelled` 让请求失败一次。I18b 去掉了按地址定时检查中的这些检查。

## A78 每个连接各自的空闲超时

**状态。** 已在 akio 分支 `claude/akit-open-requests` 上实现（`22bc4402`，akio 的 ADR-155）；akit 已使用，尚未移动锁定。`Conn::set_idle_timeout` 与 `Conn::set_idle_counts_writes`。akit-l4 给每个中继的客户端连接和上游连接、每个 UDP 会话的两侧设置服务的空闲超时，并把它们发出的数据也算作活动，由 akio 在各自的 I/O 线程上关闭它们。服务对所有连接的扫描已去掉，只为已拼接的连接保留（akio 看不到它们的字节；它们在 akio 中的空闲超时被关闭；A170 的 `Tunnel::moved`）。akit-listener 不再把引擎的 UDP 空闲超时提高到各服务中最长的那个（`udp_idle`，启动时和重载时）：它只用于 HTTP/3 的对端。测试：akit-listener 的 `l4` 中除 `tcp_idle_timeout` 与 `udp_sessions_expire` 外新增的 `tcp_one_way_is_not_idle`（只下载和只上传的连接都能活过空闲超时，安静下来后关闭）。

**问题。** fit 的四层服务（I51）会关闭在服务的 `idle_timeout` 内两个方向都没有数据的连接或 UDP 会话：默认
TCP 10 分钟、UDP 1 分钟，每个服务各自设定。akio 只有整个引擎的空闲超时：`Config::idle_timeout`（TCP，只算
入站数据）和 `Config::udp_idle_timeout`（UDP 对端，监听器的和 `dial_udp` 的都算），后者也约束 HTTP/3 监听器的
对端。fit 底下的 fib 可以按连接设置（`SetUDPIdleTimeout(-1)` 为服务自己计时的会话关掉引擎的超时）。

**依据。** `4f7169c4` 的 `crates/akio-core/src/config.rs`（`idle_timeout`、`udp_idle_timeout`，作用于引擎
的所有连接）；`Live::set_udp_idle_timeout` 改的是引擎的值，不是某个连接的。`Conn` 没有对应的设置方法。

**建议的 API。** `Conn::set_idle_timeout(Option<Duration>)`，适用于 TCP 连接和 UDP 对端（接受的或拨出的），
可在任意线程调用，为该连接覆盖引擎的值；`None` 为它关掉超时。另加一个选项把出站数据也算作活动
（`Conn::set_idle_counts_writes(bool)`），因为中继只有在两边都不发送时才算空闲；或者由 akit 在中继里同时
刷新两个连接，再由每个连接的计时器读取。

**akit 的用法。** `akit_l4` 按服务的设置为每个连接和会话设定空闲超时，由 akio 在其自己的 I/O 线程上关闭。在此
之前的权宜之计：每个服务在 akit 的定时线程（`akit_upstream::timer`，A45 的权宜之计）上扫描自己打开的连接，
间隔为较短空闲超时的四分之一（10 毫秒到 1 秒之间）；akit 把引擎的 `udp_idle_timeout` 提高到各服务中最长的那个
（`Live::set_udp_idle_timeout`，每次重载时更新），以免 akio 先关闭会话。

## A79 带各自连接超时的 `Engine::dial`

**状态。** 已在 akio 分支 `claude/akit-open-requests` 上实现（`22e038cc`，akio 的 ADR-154）；akit 已使用，尚未移动锁定。`akio::DialOptions` 与 `Engine::dial_with`：akit-l4 以上游的 `connect_timeout` 拨出 TCP 上游的 IP 字面量（`dial_udp` 不需要：UDP 没有握手）。

**问题。** 四层服务（I51）像 fit 那样在上游的 `connect_timeout` 内拨号上游地址。`Engine::dial` 和
`Engine::dial_udp` 用的是引擎的 `Config::connect_timeout`，所有拨号共用一个；`akio::dns::HappyEyeballs` 每次
尝试有自己的超时，但只有一个地址时同样用引擎的。这是 A12 在非 HTTP 拨号上的缺口。

**依据。** `4f7169c4` 的 `crates/akio/src/lib.rs`（`Engine::dial`、`dial_udp`、`dial_from` 都不接受超时）；
`crates/akio/src/dns.rs`（`HappyEyeballs::dial` 对单个地址直接调用 `engine.dial`）。

**建议的 API。** `Engine::dial_with(addr, DialOptions { connect_timeout, .. }, acceptor, on_error)`，以及让
`HappyEyeballs::dial` 对单个地址也使用它的 `connect_timeout`。

**akit 的用法。** `akit_l4` 在上游的 `connect_timeout` 内拨号每个地址。在此之前，引擎的连接超时取各上游中最短
的（`akit_listener::engine`，与 A12 相同），主机名解析出的多个地址用 `HappyEyeballs` 以上游的超时竞速。

## A90 由调用方自己的过滤器设置流式响应的编码

**问题。** fit 的 `compress` 中间件（I47）边写边压缩流式响应（代理的响应），有自己的协商（在它提供的编码中按 q 值，
相同时按顺序）、自己的适用条件（除 206 外所有带消息体的状态码，`no-transform`、`Content-Range`、声明长度小于
`min_bytes`）、弱标签，以及同时压缩的响应数上限（`max_in_flight`），每个名额在响应结束时归还。akio 的 `compress`
中间件的选择与之不同，而且只有它能给流式消息体设置编码。

**依据。** `4f7169c4` 的 `crates/akio-http/src/message.rs`（`Response::stream_coding` 和 `streaming` 是
`pub(crate)`）、`crates/akio-http/src/coding.rs`（`StreamCoding` 是 `pub(crate)`）、`crates/akio-http/src/middleware.rs`
（由 `compress` 的过滤器设置）。没有任何机制告诉过滤器流式响应的消息体何时结束。

**建议的 API。** 公开的 `Coding`（gzip、deflate、br、zstd，带级别和窗口）、`Response::is_streaming`、
`Response::set_stream_coding(Option<Coding>)`，以及 `Response::keep_until_sent(value)`：随响应保留到发出为止的值
（流式响应保留到其 `BodyStream` 结束或被丢弃），它被丢弃时归还名额。

**akit 的用法。** `akit-middleware` 的 `compress` 过滤器像 fit 一样选择编码，在流式头部上设置编码并把名额的守卫保留
在那里；完整消息体由它自己用 akio 的压缩器压缩。**已在** akio 分支 `claude/a90-a91-compress`（`b5cd3f25`，ADR-112）
上实现，尚未合并。

## A91 `Files` 为内存中的文件保存压缩副本

**问题。** fit 静态处理器的 `compress` 把内存中的文件也按客户端要的每种编码各存一份压缩副本，在文件线程池上以压得小
的级别做一次，此后直接提供，任何请求都不必再压缩它。

**依据。** `4f7169c4` 的 `crates/akio-http/src/files.rs` 提供磁盘上的压缩孪生文件（`twins`，ADR-100），把小文件
保存在内存中（`max_cached_bytes`），但不做副本。

**建议的 API。** `Files::compress(&["br", "zstd", "gzip"])`：内存中足够大、类型可压缩的文件，每种编码一份副本，
在客户端首次首选该编码时在磁盘线程池上制作，随文件的缓存项保存；有自己的标签，不支持范围；孪生文件优先。

**akit 的用法。** `static` 处理器把它的 `compress` 传给它。**已在** akio 分支 `claude/a90-a91-compress`（`b5cd3f25`，
ADR-112）上实现，尚未合并。未做：把副本计入缓存的内存上限（A15）。

## A92 从磁盘文件发送的消息体按流编码压缩

**问题。** fit 的 `compress` 中间件压缩处理器写出的一切，包括从磁盘读取的大静态文件。akio 发送文件消息体
（`Response::file`，大于 `max_cached_bytes` 的文件）用 `sendfile(2)` 或在传输层读取；过滤器无法给它设置编码，
也不能在 I/O 线程上读取文件来编码（规则 1）。

**依据。** `4f7169c4` 的 `crates/akio-http/src/message.rs`（`FileBody`）、`context.rs` 和 `h2.rs`（文件消息体原样
发送）；`Response::set_stream_coding`（A90）只作用于流式消息体。

**建议的 API。** 设置了流编码的文件消息体在磁盘线程池上分块读取并编码，各块像流式消息体那样写出，受连接的背压约束
（队列排空后再读下一块），并删除其 `Content-Length`。

**akit 的用法。** `compress` 中间件像对流式消息体一样给文件消息体设置编码。在此之前，来自磁盘的消息体不压缩，进度
文件把它列为与 fit 的差异。

**已完成**（akio ADR-124，分支 `claude/a57-a58-a14-a15-a92` 的 `7d1b1670`，已锁定）：响应过滤器设置了
流编码的文件消息体，在磁盘池上以 64 KiB 分片读取、编码并写出，受连接的背压控制。akit 的 `compress` 中间件
也压缩从磁盘发送的消息体。

## A93 HTTP/1 客户端在 `101` 时交出连接

**问题。** WebSocket 代理（I49）把客户端的握手作为 HTTP/1.1 upgrade 发往上游，上游回答 `101 Switching
Protocols` 后，把它的连接与客户端的接起来，如同 fit 的 `pool.SendUpgrade` 和 `resp.Tunnel()`。akio-http 的
`Client` 无法交出连接：它把 `101` 当作没有响应体的响应，结束交换并把连接放回 keep-alive 池，之后到达的字节被当作
垃圾并关闭连接。

**依据。** `f6750e93` 的 `crates/akio-http/src/client.rs`，`ClientConn::advance`（`101` 被视为 `no_body`，
`complete` 把连接放回池中；`RState::Idle` 收到没人要的字节时关闭连接）。`ResponseHandler` 和 `ResponseBody` 都不
交出连接。

**建议的 API。** `ClientRequest::upgrade(protocol)` 与 `ResponseHandler::on_upgrade(&ResponseHead, &RawConn) ->
Option<Box<dyn RawHandler>>`：对请求升级的请求收到 `101` 时，请求结束，连接连同服务端在头部之后发送的内容作为
字节流交给返回的处理器（`RawConn`：`send`、`hold_reads`、`on_drain`、`queued_bytes`）。

**akit 的用法。** `akit_upstream::Exchange::on_upgrade` 把它传下去，归还该地址的名额，并在连接关闭前把它计入该地址
的连接数；`akit_proxy` 的 `websocket` 把它与客户端一侧接起来，双向背压。已由本迭代在 akio 中完成（ADR-113，分支
`claude/a93-a94-tunnels`，`30cdf93a`），待合入 akio 的 main。

## A94 对 HTTP/2 扩展 CONNECT 的流式 `2xx` 应答即隧道

**问题。** HTTP/2 或 HTTP/3 客户端用扩展 CONNECT（RFC 8441、RFC 9220）打开 WebSocket；代理要等上游切换后才能
带着上游的字段应答，因此不能用 `Context::upgrade_websocket`（它立即以自己的选择应答，并在服务端终结 WebSocket）。
akio 的 HTTP/2 服务端已把扩展 CONNECT 作为流式请求分发（其 DATA 交给 `on_body`，可用 `BodyReads` 暂停），可由
流式响应应答，但只有 `upgrade_websocket` 会把流标记为隧道：客户端一半仍开着时结束的流式 `200` 以
`RST_STREAM NO_ERROR` 收尾，截掉客户端最后的字节（其关闭帧的回应）。

**依据。** `f6750e93` 的 `crates/akio-http/src/h2.rs`，`Core::finish_if_done`（只有 `st.tunnel` 会让对端一半保持
打开；只有 `upgrade_websocket` 所用的 `start_tunnel` 设置它）。HTTP/3（`akio-h3`）本就让流的两个方向分开。

**建议的 API。** 不需要新 API：对扩展 CONNECT 的流式（`stream_response`）`2xx` 应答使该流成为隧道，与
`upgrade_websocket` 的 `200` 相同，对端一直不结束其一半时同样延迟后重置。

**akit 的用法。** HTTP/2 或 HTTP/3 WebSocket 的客户端一侧就是它的流式请求和一个流式 `200`。已由本迭代在 akio 中
完成（ADR-113，与 A93 一起）。

## A108 HTTP/1 客户端的 TLS 服务器名

**问题。** fit 的 `tls_server_name`（I54）是校验 `h2` 上游证书所用、并在 SNI 中发送的名字，代替每个地址的
主机名：经 `10.0.0.5:443` 访问、证书却写着 `app.internal` 的上游。`H2Client::connect` 把 TLS 名字与主机分开
传入，但在 ALPN 中选择 HTTP/1.1 的地址及其探测走的是 akio-http 的 `Client`，它总用 URL 的主机名；URL 也不能
改写成那个名字，因为连接就是发往 URL 的主机。

**依据。** `c5cf4353` 的 `crates/akio-http/src/client.rs`，`Client::dial`（`Tls::client(cfg, &key.host,
acceptor)`）；`Client`、`ClientOptions`、`ClientRequest` 都无法设置该名字。

**建议的 API。** `Client::with_tls_server_name(name)`：每个 `https://` 连接都在 SNI 中发送 `name` 并以它校验
证书；连接仍按主机和端口入池；空名字恢复 URL 的主机名。

**akit 的用法。** `h2` 连接池的 HTTP/1 客户端（回退用）及其探测的客户端都带上上游的 `tls_server_name`。
已由本迭代在 akio 中完成（ADR-117，分支 `claude/a108-a109-upstream-tls`，`252fba42`），待合入 akio 的 main。

## A109 构建配置时检查客户端证书的私钥

**问题。** fit 用 Go 的 `tls.LoadX509KeyPair` 加载 `tls_cert_file` 和 `tls_key_file`，它拒绝不属于该证书的
私钥，并在编译配置时在该字段上报告。akio-tls 的 `ClientConfigBuilder::build` 解析私钥后没有像
`ServerConfigBuilder::build` 那样与叶证书比对，不匹配的一对也能构建出来，之后每次索要证书的握手都失败。

**依据。** `c5cf4353` 的 `crates/akio-tls/src/config.rs`，`ClientConfigBuilder::build`
（`CertKey::new(chain, key.parse())`；服务端调用会比对公钥的 `certified_key`）。

**建议的 API。** 不需要新 API：`ClientConfigBuilder::build` 像服务端一样检查客户端证书链，失败时报
`private key does not match the certificate`。

**akit 的用法。** `akit_tls::UpstreamTls::load` 在编译配置时于 `upstreams[i].tls_cert_file` 报告不匹配，与 fit
一致。已由本迭代在 akio 中完成（ADR-117，与 A108 一起）。

## A111 响应过滤器在流式消息体发出时看到它

**问题。** 响应缓存（I67）与 fit 一样，在响应发往客户端的同时保留一份，全部发完后才存下，从不扣住响应。
响应过滤器能看到流式响应（`Context::stream_response`、`Detached::stream_response`，即代理超过 64 KiB 的
消息体）的响应头，却看不到它的消息体：写入从 `BodyStream::write` 直接进入传输层，也没有办法知道消息体是
完整结束、被截断还是带 trailer 结束。

**证据。** 在 `c5cf4353`：`crates/akio-http/src/context.rs`，`start_stream` 与
`BodyStream::write` / `finish` / `finish_with_trailers`。

**建议的 API。** `Response::tap_body(impl BodyTap)`，由过滤器设置在流式响应头上；
`BodyTap::data(&[u8])` 在每次写入经过流编码之后调用，`BodyTap::end(TapEnd)` 最后调用一次
（`Finished`、`Trailers`、`Cut`）。

**akit 的用法。** 缓存的过滤器给可存储的流式响应设置旁路，复制经过的字节（每个响应和每个 zone 都有上限），
在 `Finished` 时存下。已由本迭代在 akio 中完成（ADR-116，与 A112、A113 一起）。

## A112 三种格式的 HTTP 日期

**问题。** 缓存要读 `Date`、`Expires`、`Last-Modified` 和 `If-Modified-Since`，RFC 9110 第 5.6.7 节要求
接收方接受三种格式，fit 用 Go 的 `http.ParseTime` 正是如此。akio 只在 `files.rs` 中私有地解析 IMF-fixdate。

**建议的 API。** `akio_http::parse_http_date(&str) -> Option<u64>`。

**akit 的用法。** 用于缓存读取的每个日期。已由本迭代在 akio 中完成（ADR-116）。

## A113 在内层过滤器之后运行的响应过滤器

**问题。** 过滤器按注册顺序运行，中间件在调用所包裹的处理器之前注册的过滤器，先于其内层中间件的运行：放在
`compress` 前面的缓存会在响应被压缩之前存下它。fit 的响应头钩子由内向外运行；akit 的中间件依赖 akio 现有
的顺序，不能对所有人改变。

**建议的 API。** `Context::add_outer_response_filter(f)`：外层过滤器在所有普通过滤器之后运行，后添加的先运行。

**akit 的用法。** 缓存的过滤器是外层过滤器，因此 `compress` 选择的每种编码各是一个变体。已由本迭代在 akio 中
完成（ADR-116）。

## A105 请求的对端是受信任 PROXY 头所指的客户端

**问题。** 位于负载均衡器后面的 HTTP 监听器像 fit 的 `proxy_protocol` 那样从 PROXY 头取得客户端地址：头所指的
客户端就是请求的客户端，体现在 `X-Forwarded-For`、访问日志、`remote_ip` 匹配、限流与 `ip_hash` 中，而 akit 都从
`Request::peer` 读取。akio-mux 的 `ProxyProtocol` 层（A4）读出头并挂到连接上，但 akio-http 用套接字的对端填
`Request::peer`，请求处理函数又看不到连接，所以 akit 拿不到这个头。

**证据。** 在 `c5cf4353`：`crates/akio-http/src/session.rs` 构造 `Request { peer: c.peer_addr(), .. }`，并把
`c.peer_addr()` 交给 HTTP/2 会话；ADR-095 把“在那里使用 `proxy::client_addr`”留给以后的改动。

**建议的 API。** 不需要新 API：`Request::peer`（以及 `Context::peer`）取 `akio_mux::proxy::client_addr(conn)`，
即该层挂上的头若给出了地址就取其源地址，否则取套接字的对端；`Conn::peer_addr` 不变。

**akit 的用法。** `akit-listener` 在带 `proxy_protocol` 的监听器（TCP 上的 HTTP，在 TLS 之前）前面放
`ProxyProtocol`，信任其 `trusted` 地址；此后所有读取客户端地址的功能读到的都是头里的地址。已由 I52 在 akio 中
完成（ADR-118，分支 `claude/a105-proxy-peer`，`e3f03a32`），待合并到 akio 的 main。

## A117 从 `akio` 门面取得应用自己的有界线程池

**状态。** 已在 akio 分支 `claude/akit-open-requests` 上实现（`57a942af`）；akit 已使用，尚未移动锁定。从门面取得 `akio::{Pool, PoolConfig, Rejected, Job}`：akit-cache 的磁盘池使用 `akio::Pool`，akit 不再依赖 `akio-pool`。

**问题。** 规则 4 要求会阻塞的工作在 akio 的逻辑线程池或 akit 自己的有界线程池上运行。缓存的磁盘层（I68）需要
一个与逻辑线程池分开的线程池，这样卡住的磁盘既不会拖住逻辑线程池上的处理器，也不会拖住其他东西，正如 fit 的
缓存有自己的文件池、akio 的 `Files` 有自己的 `disk_pool`。akio 有这个线程池（`akio_pool::Pool`，位于
`Conn::spawn_blocking` 之后），`akio` 门面重新导出了 `PoolConfig` 和 `PoolStats`，却没有导出 `Pool`，所以应用
能构造一个用不上的 `PoolConfig`。

**证据。** 在 `e9891b52`：`crates/akio/src/lib.rs` 中 `use akio_core::{.., Pool, ..}`（私有）与
`pub use akio_core::{.., PoolConfig, PoolStats, ..}` 并列；`akio-pool` 的清单称其为 "akio internals ...
(use `akio`)"。

**建议的 API。** 从 `akio` 执行 `pub use akio_core::Pool`（连同 `Pool::try_execute` 接收并退回的 `Rejected`
与 `Job`），并在文档中说明它是供应用自己的阻塞工作使用的线程池。

**akit 的用法。** `akit-cache` 的磁盘线程池（`disk.rs`）：用 `Pool::new` 建立至少 16 个、每个 CPU 4 个线程、
队列 4,096 的线程池，每个文件操作都用 `try_execute` 提交。期间 akit-cache 直接依赖 `akio-pool`，与所有 akio
crate 同一个提交（权宜之计，记录在进度日志中）；此需求进入 akio 的 main 后改用 `akio::Pool`。

## A135 拨号 Unix 域流套接字，以及在其上的 HTTP 客户端

**问题。** Docker 提供者（I58）通过 Engine API 列出容器、跟随守护进程的事件，守护进程默认在
Unix 域套接字 `/var/run/docker.sock` 上提供这个 API。规则 2 要求 akit 的请求走 akio 的异步客户端；
akio 的引擎只能拨 TCP，它的 HTTP 客户端会解析 URL 的主机名，所以 akit 只能用自己的阻塞客户端
才能连到这个套接字。

**证据。** 在 `e9891b52`：`Engine::dial` 和 `dial_from` 接受 `SocketAddr`；
`akio_sys::connect_nonblocking` 只创建 `AF_INET`/`AF_INET6` 套接字；epoll 和 io_uring 的
`finish_connect` 按 IP 地址读取对端和本端地址；`Client::dial` 解析 `key.host` 再拨这些地址。

**建议的 API。** `Engine::dial_unix(path, acceptor, on_error)`：像拨出的 TCP 连接一样服务的 Unix
域连接（`peer_addr` 和 `local_addr` 是固定的 `UNIX_ADDR`）；以及
`Client::with_unix_socket(Option<PathBuf>)`：该客户端的每个请求都发往这个套接字，URL 的主机仍是
`Host` 字段，也仍是空闲连接的键。

**akit 如何使用。** 对 `unix://` 端点，`akit_provider::docker` 用 `with_unix_socket` 构建客户端，
在其上发出 `GET /events` 和 `GET /containers/json`，回调在 akio 的 I/O 线程上。已由 I58 在 akio
中完成（ADR-120，分支 `claude/a135-unix-dial`，`0f2c419b`），待合并到 akio 的 main。

## A148 请求到达的地址

**状态。** 已在 akio 分支 `claude/akit-open-requests` 上实现（`22bc4402`，akio 的 ADR-157）；akit 已使用，尚未移动锁定。`Request::local` 与 `Context::local_addr`。FastCGI 处理器的 `SERVER_ADDR` 和 `SERVER_PORT` 是请求到达的地址，与 fit 取自 TCP 连接的相同；没有该地址时（端口为 0：Unix 域套接字）照旧为空，端口取 `Host` 字段的端口。在 HTTP/3 上 akio 给出 UDP 套接字的地址，akit 会使用它，而 fit（只从 TCP 连接取地址）此时 `SERVER_ADDR` 为空。测试：处理器 `params` 中的 `server_address`，以及 akit-handler 的 `fastcgi` 中的 `params`。

**问题。** FastCGI 应用通过 CGI 的 `SERVER_ADDR` 和 `SERVER_PORT`（PHP 的 `$_SERVER['SERVER_ADDR']`）读取请求到达
的地址和端口，fit 从连接的本地地址取得它们。基于 akio `Context` 的 handler 能看到请求的对端（`Request::peer`），
却看不到请求所在连接的本地地址，所以 akit 的 `fastcgi` handler（I75）发送空的 `SERVER_ADDR`，`SERVER_PORT` 取自
`Host` 字段（或 80、443），fit 只在连接不是 TCP 时才这样做。绑定在多个地址（`0.0.0.0`、`[::]`）上的监听器同样
无法区分；访问日志的 `server_addr` 和基于它的匹配器也是如此。

**证据。** 在 `e9891b52`：`crates/akio-http/src/message.rs` 有 `Request::peer`，没有本地地址；`Context` 有
`peer()`，没有另一端的地址；`akio_core::Conn::local_addr` 存在，但请求处理器看不到连接。

**建议的 API。** `Request::local: SocketAddr`（或 `Context::local_addr()`），与 `peer` 一样填入：HTTP/1.1 和
h2 上是连接的本地地址，h3 上是 UDP 套接字的地址（没有时为未指定地址，如虚拟连接）。

**akit 的用法。** `fastcgi` handler 的 `SERVER_ADDR` 和 `SERVER_PORT`，与 fit 一致；以后任何指明请求到达地址的
字段或匹配器。

## A132 代理隧道的上游连接建立后再检查

**问题。** fit 的正向代理（监听器的 `socks`、`forward_proxy` 处理器）按目的主机名解析出的地址判定它，并且因名字
而异：`allow` 中写明的名字只会被 `deny` 网段或对内部地址（回环、链路本地、组播、未指定、本机自身）的拒绝所拒绝，
其他名字则需要某个 `allow` 网段包含其地址。本机自身地址只有在打开的连接上才看得出来：来源地址等于目的地址。
fit 在连接建立后、发送任何数据之前判定，这样 DNS 无法让检查与拨号得到不同答案。akio-socks 有
`ProxyConfig::destinations`，它看到解析后的每个地址，却不知道名字和本地地址；还有 `Socks::allow`，它看到的
是未解析的名字。在受信任的 PROXY 头之后，fit 检查的是该头所指的 SOCKS 客户端；akio-socks 的访问钩子看到的是
套接字的对端。

**证据。** 在 `e9891b52`：`crates/akio-socks/src/lib.rs` 中，`Destinations::allows(SocketAddr)` 在拨号前
过滤解析出的地址；`Request::client` 是 `c.peer_addr()`。

**建议的 API。** `ProxyConfig::connected: Option<Connected>`，`Connected = Arc<dyn Fn(&Dest, SocketAddr,
SocketAddr) -> bool>`：上游连接打开后在其 I/O 线程上调用，参数是所请求的目的地以及远端和本地地址；返回 `false`
时关闭连接，请求按不允许失败（SOCKS5 2、SOCKS4 `0x5b`、HTTP 502），不再尝试其他地址。`Request::client` 为
`akio_mux::proxy::client_addr(conn)`。

**akit 的用法。** `akit_l4::forward::Rules::proxy_config` 把 `destinations` 设为 `Any`，在访问钩子（HTTP 则在
处理器）中按所请求的目的地判定请求，在 `connected` 中判定地址（fit 的 `Rules.resolved` 与
`netguard.Refused`）。由 I55 在 akio 中实现（ADR-119，分支 `claude/a132-a133-socks-rules`，`9b096d0d`），
待合入 akio 的 main。

## A133 代理自己的连接超时

**问题。** fit 的代理有 `connect_timeout`（10 秒）：解析与连接最多用这么久，超时后 HTTP 应答 `504`、SOCKS5 应答
“主机不可达”，与 SOCKS 握手的超时分开。akio-socks 的 `handshake_timeout` 覆盖从问候到连接的全过程，超时只关闭
客户端而不应答。

**证据。** 在 `e9891b52`：`crates/akio-socks/src/guard.rs` 只有握手与空闲超时；超时触发时调用 `client.close()`。

**建议的 API。** `ProxyConfig::connect_timeout: Option<Duration>`，从 `CONNECT` 请求到上游连接建立；超过后
应答 SOCKS5 4、SOCKS4 `0x5b`、HTTP `504`，然后关闭。

**akit 的用法。** 每个正向代理的 `connect_timeout`，握手超时为 10 秒（fit 的 `OpeningTimeout`）加上它。与 A132
一起由 I55 在 akio 中实现（ADR-119）。

## A134 经 HTTP/2 和 HTTP/3 的 CONNECT 隧道

**问题。** fit 的 `forward_proxy` 在 HTTP/1.1 连接、h2 或 h2c 流（RFC 9113 §8.5）和 h3 流（RFC 9114 §4.4）上
都能建立 CONNECT 隧道。akio-http 只把扩展 CONNECT（带 `:protocol`，A94）的 HTTP/2 流作为隧道分发；akio-socks
的 `connect_proxy` 用 `Context::upgrade_raw` 建立隧道，而它只支持 HTTP/1：在 HTTP/2 流上返回 `false`，请求永远
得不到应答。

**证据。** 在 `e9891b52`：`crates/akio-http/src/h2.rs` 只在 `req.protocol.is_some()` 时设置
`st.connect`/`streaming`；`Context::upgrade_raw` 的文档写明 “HTTP/1 only; `false` otherwise”；
`connect_proxy_with` 忽略其返回值。

**建议的 API。** HTTP/2 或 HTTP/3 流上的普通 CONNECT 立即分发，其 DATA 作为请求体流式送达；对它流式发出的
`2xx` 打开一条两个方向分别关闭的隧道（与 A94 对扩展 CONNECT 的处理相同）；`connect_proxy` 以与 HTTP/1.1 隧道
相同的背压、超时和检查把这样的流中继到上游。

**akit 的用法。** `forward_proxy` 处理器把每个 CONNECT 交给 `connect_proxy`；在此之前，HTTP/2 或 HTTP/3 流上的
CONNECT 应答 501（权宜之计，记录在进度日志中）。

**已实现**（akio ADR-122）：akio-http 立即分派 HTTP/2 的 CONNECT（只带 `:method` 和 `:authority`，目标为
authority），akio-h3 在收到头部时分派；流式的 `2xx` 即为隧道。`connect_proxy` 在该流上中继，检查、超时、状态码
与 HTTP/1.1 隧道相同，双向背压；客户端的 END_STREAM 关闭上游的发送方向（新增的
`Conn::shutdown_write_after_send`）。`H2Client` 也能发送这样的 CONNECT。501 已去掉。

## A165 插件用的 WebAssembly 运行时

**问题。** fit 在路由的每个请求上运行 WASM 插件（决策 017：wazero，fit 自己的 ABI；决策 018 和 023：Proxy-Wasm
子集，其请求和响应回调），在自己的池上运行，有时间和内存上限。akit 不使用 `unsafe`，也没有异步运行时（规则 1 和
21），无法自带 WebAssembly 运行时，而 akio 还没有。

**证据。** 在 `8453b60c`：akio 中没有任何 crate 解码或运行 WebAssembly；akio-http 有同步的响应过滤器
（`add_response_filter`）和 body tap（ADR-116），但没有能扣住响应头、或让响应 body 经过别处进行的调用、直到该
调用应答的机制。

**建议的 API。** 在 akio 的
[ADR-125](https://github.com/lesismal/akio/blob/main/docs/adr/125-wasm-runtime-for-plugins.zh-CN.md)
中设计（提议中；合并前在 akio 分支 `claude/wasm-design-95c5ec` 上）：新 crate `akio-wasm`，
`#![forbid(unsafe_code)]`，无依赖，一个 WebAssembly 2.0 核心（不含 SIMD）的解释器（即 Go、TinyGo 和 Rust guest
生成的内容）；`Module`、`Linker`、`Instance`，有内存和栈上限，以及按基本块检查的每次调用预算（截止时间、fuel、
中断）；WASI preview 1 子集；`InstancePool`，即拥有实例的线程，带有界的共享队列和每实例队列。I72 的响应阶段需要
akio-http 的两个钩子（H1：扣住响应头，直到别处的调用把它交回；H2：流式 body 由别处进行的调用逐次过滤），即 fit 向
fib 提出的 A41 和 A46，在 I72 开始时单独提出。里程碑 W1–W6 见 ADR；I71 需要 W1–W4 和 A33。

**akit 的用法。** `akit-plugin` 在 `akio-wasm` 的安全 API 上以 akit 的名字实现 fit 的 ABI（模块 `akit`，决定 037；`akit_sync` 的名字被调用
时 trap）和 Proxy-Wasm 的宿主函数，挂起每个请求（`Context::suspend`，ADR-109），调用返回时从池上恢复它，就像 fit
的 `wasm` 中间件在 fib 上所做的那样。

**已实现 W1–W4**（akio ADR-125，已接受；分支 `claude/akio-wasm-w4-445f36`，`a2373416`，与 A33 一起合并在
`claude/i71-akio-base`（`1ae0a05f`）上；akit 锁定其上加了 W6 的 `dad77a7a`，已在 akio 的 `main` 中）：解码器和校验器、解释器、宿主
API（`Linker`、`Caller`、类型化函数）和 WASI preview 1，以及 `InstancePool`（每个任务有截止时间，队列满时拒绝并
交回任务，trap 后重建实例）。I71 在其上运行 fit 的 ABI（`akit-plugin`）；W5（测量其开销）和 W6、以及响应钩子
（H1、H2）留给 I72 和基准测试任务。

## A166 由别处进行的调用决定时扣住响应头

**状态。** 已在 akio 分支 `claude/akio-wasm-w6-afc4f2` 上实现（`dad77a7a`），即 akio 的
[ADR-128](https://github.com/lesismal/akio/blob/main/docs/adr/128-held-response-heads-and-body-filters.zh-CN.md)，
ADR-125 的里程碑 W6；已在 akio 的 main（`dad77a7a`），已锁定。

**问题。** fit 在插件的池上调用 Proxy-Wasm 插件的 `proxy_on_response_headers`（决策 023，借助 fib 的 A41
`OnHeaderAsync`）。akio-http 的响应钩子，即响应过滤器（`add_response_filter`、`add_outer_response_filter`），在
确定响应头的地方运行（常常是 I/O 线程）并当场决定：akit 不能在那里运行插件（规则 4），也无法扣住响应头直到池应答。

**证据。** 在 akio `1ae0a05f`：`Finish::apply` 在 `Out::send` 和 `start_stream` 中同步运行过滤器，它们一返回响应
头就发出；没有任何机制扣住它。

**实现。** `Context::hold_response_head(timeout, f: impl FnOnce(HeldHead, &Request) + Send)`。`f` 在响应头本该
发出的地方运行，在普通响应过滤器和附加字段之后、外层过滤器之前；它把 `HeldHead` 交出去后返回。`HeldHead` 提供
`status`、`headers`、`headers_mut`、`body`（整体 body）、`is_live`，以及可从任意线程调用的 `resume()`、
`respond(Response)`（以本地响应取而代之：与 fib 不同，akio 可以做到，因为什么都还没发出）和 `fail()`；被丢弃即失败。
失败或截止时间会切断响应（关闭 HTTP/1 连接，重置 h2 与 h3 的流，`End::Gone`）。期间整体响应整体等待，流式 body 写入
的内容最多扣住 4 MiB（计入 `BodyStream::queued_bytes`，按其限速的生产者像遇到慢客户端一样等待）。对 `CONNECT` 和升级
的响应不经过它，与 fib 相同。HTTP/1.1、h2 和 h3 一致。

**akit 的用法。** 对导出 `proxy_on_response_headers` 的 Proxy-Wasm 插件，`akit-plugin` 为它放行的每个请求注册
一个扣住钩子，并在钩子中把调用排给持有该请求上下文的实例（`InstancePool::submit_to`），带上响应头的字段；完成回调应用
插件的修改并恢复，或以 `respond` 发出其本地响应，或按 `on_error` 使响应失败。请求阶段记录的对响应的修改先行应用（与
现在一样用响应过滤器）。

**I72 起已使用**（`crates/akit-plugin/src/response.rs`）：如上，只为导出 `proxy_on_response_headers` 的过滤器
注册扣住，最长 `wait_timeout` + `timeout` + 1 秒；它发出的本地响应用 `HeldHead::respond` 发送
（[决定 011](decisions/011-proxy-wasm.zh-CN.md)）。

## A167 由别处进行的调用过滤响应 body

**状态。** 已在 akio 分支 `claude/akio-wasm-w6-afc4f2` 上实现（`dad77a7a`），即 akio 的 ADR-128，ADR-125 的
里程碑 W6；已在 akio 的 main（`dad77a7a`），已锁定。

**问题。** fit 在 body 写出时于插件的池上调用 `proxy_on_response_body` 和 `proxy_on_response_trailers`（决策
023，借助 fib 的 A46 `OnBodyFilterAsync`）。akio-http 的 body 钩子，即流编码（ADR-112）和 body tap（ADR-116），在
写 body 的线程上运行，而 tap 只能看到：没有机制让别处进行的调用替换 body、把它扣到末尾或修改其 trailer。

**证据。** 在 akio `1ae0a05f`：`BodyStream::write` 立即编码并写出；`BodyTap::data` 得到一份拷贝，不返回任何东西。

**实现。** `Context::add_body_filter(impl BodyFilter)`：
`BodyFilter::filter(&mut self, BodyChunk { data, end, trailers }, done: FilterDone)` 与
`BodyFilter::abort(&mut self)`；从任意线程调用 `FilterDone::done(data, trailers)` 或 `fail()`。一次一个调用，带上次
以来写入的全部内容；期间写入的内容被扣住（最多 4 MiB），交给下一次调用。最后一次调用（`end`）带 trailer（没有则为
空），可以修改或添加。输出交给下一个 body 过滤器，再经过流编码（因此 `compress` 编码的是插件的输出，插件看到未编码的
body）和 tap；由客户端限速（连接积压超过 256 KiB 时下一次调用等待）。整体响应以流的形式、不带 `Content-Length`，在一次
最后调用中经过它；文件 body 在磁盘池上读入过滤器。调用失败、`FilterDone` 被丢弃或超过上限都会切断响应；`abort` 得知
在最后一次调用之前结束的响应。对 `HEAD`、`1xx`、`204`、`304`、`CONNECT` 和升级不调用。`on_end`（A48）在最后一次调用
的输出发出之后运行，done、log 与 delete 回调就在那时进行。HTTP/1.1、h2 和 h3 一致。

**akit 的用法。** 对导出 `proxy_on_response_body` 或 `proxy_on_response_trailers` 的 Proxy-Wasm 插件
（`response: all`），`akit-plugin` 添加一个 body 过滤器，其调用排给持有该请求上下文的实例：`proxy_on_response_body`
处理插件扣住的内容（暂停时以空内容应答，到末尾时以扣住的全部内容应答，最多为插件的 `max_body_bytes`），最后一次调用
以 trailer 调用 `proxy_on_response_trailers`。`on_end` 把 `proxy_on_done`、`proxy_on_log` 和 `proxy_on_delete`
排入队列。`response: headers` 不注册 body 过滤器，文件 body 因此仍用 `sendfile`。

## A168 只提供先验知识的 HTTP/2

**状态。** 已在 akio 分支 `claude/i79b-akio` 上实现（I79b，2026-11-26），尚未进入 akio 的 main：`HttpConfig::http1`（默认 `true`；为 `false`
时，不以 preface 开头的连接收到 `GOAWAY` `PROTOCOL_ERROR` 并被关闭，也不接受 `Upgrade: h2c`），即 akio 的 ADR-174。akit 的
I79b 分支为只有 `h2c` 的监听器设置它，并去掉“尚不支持”；akit 的 main 随锁定版本的更新使用它。

**问题。** fit 中 `protocols: [h2c]` 的监听器只提供先验知识的 HTTP/2，不提供其他协议，fit 基准配置的 8081 端口就是这样。
akio-http 的服务端对不以 HTTP/2 前言开头的连接总是按 HTTP/1.1 处理，因此 akit 在启动时拒绝这样的监听器
（`not supported yet: protocols h2c without http/1.1`），其基准配置在 `h2c` 旁边还列出了 `http/1.1`。

**证据。** 在 akio `dad77a7a`：`HttpConfig` 有 `h2: Option<H2Config>` 可关闭 HTTP/2，但没有关闭 HTTP/1 的设置；
`Session` 把没有前言的连接都当作 HTTP/1 读取。

**建议的 API。** `HttpConfig::http1: bool`（默认 `true`）：为 `false` 时，不以 HTTP/2 前言开头的连接被关闭（或者像 Go 的
仅 h2c 服务端那样，在缺失的前言之后回一个 GOAWAY），`Upgrade: h2c` 请求也不接受。

**akit 将如何使用。** `akit-listener` 的 `http_config` 按监听器是否列出 `http/1.1` 设置 `http1`，并去掉其 "not supported
yet"。在此之前 `akit validate` 也报告同样的错误（I25 发现它此前并未报告）。

## A169 akio 的服务端拒绝畸形请求

**状态。** 前三部分已进入 akio 的 main（`0fc8f3a7`）并锁定；第四部分未实现。原定进入 akio main 之前做的 HTTP/2 请求路径同轮 A/B 在合并前没有做，作为后续补做（见[待审决策](decisions-pending.zh-CN.md)）。

**问题。** I25 的一致性测试套件（`crates/akit/tests/conformance.rs`）发现，akio 的服务端会把 RFC 规定为畸形的请求交给应用，
于是 akit 的代理把它们转发给 HTTP/1.1 上游：

1. HTTP/1：带两个 `Host` 字段的请求被正常处理（RFC 9112 第 3.2 节要求 400）。上游可能按另一个路由。
2. HTTP/2：含 NUL、CR 或 LF 的字段值，以及含 token 以外字节（空格、非 ASCII 字节）的字段名被接受（RFC 9113 第 8.2.1 节
   规定这样的请求是畸形的）。akio 的 HTTP/1 客户端写给上游时丢掉了 CR 和 LF，但写出了 NUL。
3. HTTP/3：同上（RFC 9114 第 4.2 节）。
4. HTTP/1 的流式 body：请求已分派后出现错误的 chunk 大小时，连接被关闭而不作应答；fit 以 400 应答（RFC 9112："SHOULD
   respond with a 400"）。

**证据。** 在 akio `dad77a7a`：`Session::on_head` 只用一个 bool 记录 `host`；`h2.rs` 的 `build_request` 和 trailer
解码只检查名字是否含大写字母，不检查值；akio-h3 的 `header_list` 也一样；`Session` 的 `State::Stream` 在 `ChunkDecoder`
出错时直接关闭。

**实现（第 1–3 部分）。** `Session::on_head` 统计 `Host` 字段数，任何版本中多于一个都以 400 应答。HTTP/2 的请求和 trailer
字段：普通字段名只能含 0x21–0x40 和 0x5B–0x7E 范围内除 `:` 以外的字节，每个值（伪首部字段的值也算）不能含 NUL、CR 或 LF，
否则以 `PROTOCOL_ERROR` 重置该流。akio-h3 的请求字段同样处理。

**建议（第 4 部分）。** 流式请求 body 的分帧在响应尚未写出任何内容之前出错时，以 400 应答并关闭连接，并告知 handler 其请求已
离开（A38 的 `on_gone`），使它放弃对上游的交换。

**akit 如何使用。** 锁定之后，akit-proxy 的临时措施（`headers::outgoing` 中的 `Refused::BadFieldValue` 和
`Refused::TwoHosts`，在复制字段的循环中以 400 应答）可以像 fit 对字段名的检查那样作为纵深防御保留，也可以去掉；有了第 4 部分，
一致性测试中错误 chunk 大小的用例将像 fit 一样期望 400。

## A171 客户端请求结束时取消其定时器任务

**状态。** 已进入 akio 的 main（`0fc8f3a7`）并锁定，与 main 上防 panic 的定时器线程合在一起。原定进入 akio main 之前做的同轮 A/B 在合并前没有做，作为后续补做（见[待审决策](decisions-pending.zh-CN.md)）。

**问题。** I25 的第一次运行测得 akit 的代理在每秒 23 万请求时峰值内存为 1.8 GiB（fit 28–73 MiB，nginx 约 200 MiB）。
无论连接池的上限如何，内存每个请求增长约 240 字节，负载结束后也不回落：`akio_http::timer::after` 为每个带
`timeout` 的客户端请求（`Client::request`）以及每个 h2 流的超时和 header、idle 检查（`H2Client`）在唯一的定时器线程上
排入一个装箱任务，这个任务一直留到延迟过去，并通过其 `Weak` 保留请求的 `Exchange` 内存（对 h2 请求则保留其回调和流
控制）。按 fit 的 60 秒路由超时，就是一分钟的请求，在这个速率下约 1400 万个；按 akit 对一次交换的默认上限 1 小时，
就是一小时的请求。这个队列也是无界的（规则 11）。

**证据。** 在 akio `dad77a7a`：`timer.rs` 只有 `after`，没有取消的办法；`client.rs` 的 exchange 为每个请求安排
`give_up(GaveUp::Timeout)`；`h2client.rs` 为每个请求和每个流的阶段安排任务。在基准主机上，`9d9321e` 的 akit 代理到
本地后端，wrk 用 256 个连接压 10 秒：锁定的 akio 下，其 RSS 在 2–10 秒依次为 104、198、306、437、518 MiB，负载后
停在 525 MiB；带 A171 时始终为 17–18 MiB，每秒 24.6 万请求，对比 23.8 万。

**实现。** `timer::after` 返回一个 key，`timer::cancel(key)` 立即丢弃该任务；`Exchange` 在被丢弃时取消其超时任务，
h2 流的控制取消其待执行的检查和流式请求的超时，完整的 h2 请求在回调运行时取消其超时。已取消任务的堆条目在数量超过
存活任务后（超过 4096 时）被清理。

**akit 如何使用。** 无需改动；akit 的代理、forward auth、镜像和健康探测都通过 akio 带超时的客户端发送，锁定之后不再
按请求保留内存。在此之前，akit 的代理在负载下会保留其路由超时窗口内的请求。

## A172 server name 为 IP 地址的 ClientHello

**状态。** 已进入 akio 的 main（`0fc8f3a7`）并锁定。

**问题。** fit 的基准 harness 用 wrk 对 `https://127.0.0.1:8443/` 施加 HTTPS/1.1 负载。wrk（OpenSSL）即使在 URL
的主机是 IP 地址时也把它放进 ClientHello 的 `server_name`，而 RFC 6066 不允许这样做。akio-tls 的服务端以
`decode_error` 告警回应这样的 ClientHello，因此所有请求都失败了：I25 的第一次运行中，akit 在每个场景的 HTTPS/1.1
上都是每秒 0 请求（各约 200 万次连接错误），而 Go、OpenSSL 和 nginx 的服务端——因此 fit、nginx 和 Caddy——都把这样的
名字视为没有名字。末尾带点的主机名也以同样方式失败。

**证据。** 在 akio `dad77a7a`：`proto/msgs.rs` 的 `parse_host_name` 拒绝能解析为 `IpAddr` 或以 `.` 结尾的名字，
其单元测试固定了这一行为。`openssl s_client -servername 127.0.0.1` 得到告警 50；用 `-servername localhost`，
以及 wrk 访问 `https://localhost:...` 时，akit 正常应答。

**实现。** 作为 server name 的 IP 字面量被视为没有名字（握手继续，选用默认证书，`server_name()` 为 `None`）；
末尾的点被去掉；空名字或非 ASCII 名字仍使握手失败。

**akit 如何使用。** 无需改动：没有 SNI 时 akit 的证书选择本来就回退到默认证书。锁定之后，harness 的 HTTPS/1.1
场景就能测到 akit。

**I72 起已使用**（`crates/akit-plugin/src/response.rs`）：如上，只是上下文在最后一次调用中关闭（done、log、delete），
或者在响应的钩子被丢弃时由排队的任务关闭，而不是从 `on_end` 中关闭。

## A170 连接的 TCP 计数，以及先写数据再拼接

**状态。** 已在 akio 分支 `claude/akit-open-requests` 上实现（`22bc4402`，akio 的 ADR-156）；akit 已使用，尚未移动锁定。`akio::ebpf::{Relay::tunnel, Tunnel, TunnelSide, conn_byte_counts, ByteCounts::unread, ByteCounts::consumed, SpliceError}` 以及 akio-mux 的 `ProxyHeader::wire_len`。akit-l4 用 `Relay::tunnel` 拼接（`first` 中拼接未写出的部分按 `SpliceError::written` 经引擎发送），并用 `Tunnel::moved`（空闲超时，由服务的扫描读取）、`Tunnel::close_after_forward`（一侧结束，在 akio-ebpf 的排空线程上）和 `Tunnel::late_payload` 跟踪已拼接的连接；`Conn::inner`、akit 自己的计数和它在 akio 定时器上的排空都已去掉。以监听器读取的 PROXY 头开始的连接也会被拼接：隧道的基准计数包含了用户态读取的字节。[决策 010](decisions/010-sockmap-relay.zh-CN.md) 已相应更新。测试：akit-listener 的 `sockmap`，在 Linux 上以 root 运行（`client_proxy_header_spliced` 现在期望拼接）。

**问题。** `tcp` 服务在上游连接打开后用 `akio::ebpf::Relay` 拼接每个连接。已拼接连接的字节不会到达它的处理器，因此
akit 读取套接字的计数（`akio::ebpf::byte_counts`）来判断空闲超时，并且只在内核把一侧收到的数据都转发完之后才关闭另一侧。
`byte_counts` 接受 `&std::net::TcpStream`；需要先写数据的拼接（服务的 PROXY 头、akit 为选择服务而读取的 ClientHello）
要用 `Relay::splice_inner_with`，它接受 `&ConnInner`；从 `Conn<H>` 到这两者的唯一途径是 `Conn::inner`，它在 akio 的
文档中被隐藏，用途是 akio 自己的协议 crate。为 `CONNECT` 隧道做同样事情的 akio-socks 把它的拼接控制（`SpliceCtl` 及其
排空线程）保持为私有。此外，为了知道内核为一侧转发了多少字节，akit 统计用户态从客户端读取的字节；akio-mux 的
`ProxyProtocol` 读取的 PROXY 头被消耗掉了，却没有说明它有多长，所以 akit 对这样的连接在用户态转发。

**证据。** 在 `dad77a7a`：`crates/akio-ebpf/src/relay.rs` 有 `byte_counts(&TcpStream)` 和
`Relay::splice_inner_with(&ConnInner, &ConnInner, ..)`；`crates/akio-core/src/conn.rs` 在 `Conn` 上有
`#[doc(hidden)] pub fn inner(&self) -> &Arc<ConnInner>`；`akio` 没有重新导出 `ConnInner`；
`crates/akio-mux/src/proxy.rs` 跳过 `skip` 字节后移交连接，却没有记录 `skip`。

**建议的 API。** `Conn::byte_counts() -> io::Result<ByteCounts>`（或 `ebpf::conn_byte_counts(&Conn<H>)`），
并给 `ByteCounts` 增加 `unread`（`SIOCINQ`），这样代理可以把拼接时的基准取为 `received - unread`，而不必统计用户态读了
多少；`Relay::splice_conns_with(&Conn<A>, &Conn<B>, first: &[u8])`，在转发的字节之前把 `first` 写到 `b` 的套接字，
并返回两侧的基准计数；以及 akio-mux 附加的 PROXY 头旁边给出它在线上的长度（`ProxyHeader::wire_len`）。更好的做法是把
akio-socks 的拼接隧道（计数、空闲探测、关闭时的排空）作为公开类型放进 akio-ebpf。

**akit 的用法。** akit-l4 的 `sockmap` 模块改用这些 API，不再使用 `Conn::inner`，并且对带 `proxy_protocol` 的监听器的
连接也进行拼接。

## A180 让 akio-wasm 的调用更便宜，并与 wazero 对比测量（akio 的 W5）

**完成。** 已进入 akio 的 main（`077a6710`）并锁定：fit 的 Go `headers` guest 的一次调用在 gpu51 上从 9,089 ns 降到 5,068 ns（wazero 编译器 3,520 ns），见 akio 的 docs/BENCHMARKS。akit 的 `wasm` 场景待基准任务重新测量。

**状态。** 由 I76 提出（2026-11-25），给出 akio 的 W5（ADR-125 第 8 节，"对照 fit（wazero）的性能优化"，在 akio 的
开发计划中仍未完成）的第一批数字。akit 中没有任何东西等待它：开销有界且已测量，M5 的标准成立。

**问题。** 一个请求经过 `wasm` 中间件，在 akit 中的开销约为同一个 `.wasm` 在 fit 中的两倍。fit 在 wazero 的编译器上运行
guest；akit 在 akio-wasm 的解释器上运行（没有 `unsafe`，所以没有编译器），插件占了 akit 处理这个请求的大部分开销。

**证据。** [M5 报告](../bench/results/m5-report-2026-11-25.zh-CN.md#修改头部的-wasm-插件)：fit 自己修改头部的测试插件
（`plugin/wasm/testdata/headers`，Go 的 `wasip1` guest，gzip 后 1.2 MB），固定响应位于使用默认值（7 个实例，队列 1,024）
的 `wasm` 中间件之后，在基准测试主机上用 fitbench，256 条 HTTP/1.1 连接。akit：每秒 312,679 个请求，每个 22.25 µs
CPU，同一容器中不带插件时每秒 831,888 个、每个 7.52 µs，所以插件每个请求 14.7 µs。fit：每秒 452,353 个、每个 14.92 µs，
对比 785,657 个、8.15 µs，即 6.8 µs。带插件时 akit 的内存峰值 81 MiB，是 fit 的 235 MiB 的三分之一。akit 的 14.7 µs 由什么
组成（交给池再交回、复制请求、guest 自己的指令、宿主调用）还没有分开测量。

**建议的 API。** 无新 API：W5 对 akio-wasm 解释器和 `InstancePool` 的优化，先分开测量（实例上单独一次 guest 调用，对照
wazero 的解释器和编译器，如 fit 的 `BenchmarkGuestCall`；池的交接与唤醒），再处理剖析指出的部分：分派循环、每个基本块
的预算检查、宿主调用的参数传递、内存边界检查；以及让进入 Go `wasip1` guest
的导出函数变得更便宜的办法。

**akit 如何使用。** 原样使用：`akit-plugin` 在 `akio_wasm::InstancePool` 上运行每个 guest；调用变便宜后 akit 无需改动，
用 `bench/run.sh -servers akit-wasm,fit-wasm -scenarios fixed,wasm` 重新测量。

## A200 按 RFC 9112 的要求处理 absolute-form 请求目标

**完成。** 已进入 akio 的 main（`077a6710`，akio 的 ADR-177）并锁定：akio 以 origin-form 给出 absolute-form 目标，用目标的主机替换 `Host`，`Request::raw_target()` 给出原样的目标。akit-listener 的临时改写已删除，访问日志写 `raw_target()`。与 fit 的一处差异：主机为空的 http URI（`http:///x`）由 akio 回答 400（RFC 9110 第 4.2.1 节），fit 按 `Host` 字段路由它（[待审决定](decisions-pending.zh-CN.md)第 15 行）。

**状态。** 由 I77 提出（2026-11-26）。在此之前 akit-listener 改写这样的请求，这是一个临时方案，记录在
[进度](progress.zh-CN.md)中。

**问题。** HTTP/1.1 服务器必须接受 absolute-form 的请求目标（`GET http://a.example/admin/x HTTP/1.1`），并且作为源服务器，
以目标的 authority 作为请求的主机，忽略 `Host`（RFC 9112 第 3.2.2 节）；Go 的服务器就是这样做的（`r.URL.Path` 是
`/admin/x`，`r.Host` 是 `a.example`），所以 fit 按其路径为这样的请求选择路由。akio 的 HTTP/1 服务器接受这个目标并把它
原样保留：`Request::path()` 是 `http://a.example/admin/x`，`Host` 仍是该字段的值。按 `path()` 路由的路由器（akit 的或
akio-http 自己的）认为请求不属于任何它知道的路径，而把 origin-form 发给上游的代理（akit 的代理就是这样做的，也必须这样
做）会请求 `/admin/x`：fuzz 发现，这样写的请求绕过了保护 `/admin/*` 的路由，经由兜底路由到达了上游。

**证据。** 在 `dad77a7a`（以及 `111004f8`）：`crates/akio-http/src/message.rs` 中 `Request::path()` 是目标中 `?`
之前的部分；`crates/akio-http/src/parse.rs` 原样复制请求行中的目标（`new_target`）。在临时方案之前，akit 的测试
`path_confusion`（`crates/akit-listener/tests/listeners.rs`）对 `GET http://a/admin/secret` 由兜底路由作答（200）。

**建议的 API。** HTTP/1 服务器把 absolute-form 目标拆开：`Request` 的 `target` 为 origin-form（`/admin/x?q`），主机为目标的
authority（去掉用户信息），设置为 `Host`，或由 `Request::authority()` 给出并优先于 `Host`；需要时为日志保留 absolute-form
（`Request::raw_target()`）。authority-form 的目标只留给 `CONNECT`。

**akit 的用法。** 去掉 akit-listener 的 `absolute_form` 改写（在 `Dispatcher::call` 和 `body_mode` 中）；路由、代理、缓存键和
访问日志按 akio 给出的请求读取。

## A214 所有连接合计排队字节的预算

**状态。** 已在 akio 分支 `claude/i79b-akio` 上实现（I79b，2026-11-26），尚未进入 akio 的 main：`Config::max_pending_bytes`（akio 的
ADR-172；一个监视线程每 1–10 毫秒汇总各 I/O 线程的队列 gauge，连接通过 `ConnInner::read_held` 暂停，并经由各自的事件循环唤醒；
`LoopCounters::budget_pauses`、`Telemetry::pending_bytes`），akio 的 docs/BENCHMARKS 中有同轮 A/B。akio 没有把内存份额换算成
字节的辅助函数：由 akit 读取（决定 032）。akit 的 I79b 分支把 `limits.max_pending_bytes` 映射到它上面；锁定版本更新之前，akit
的 main 仍拒绝该字段。

**问题。** fit 的 `limits.max_pending_bytes` 约束所有连接中等待客户端取走的响应字节总量：超出时每个连接都停止读取，直到有一些被取走
（默认是 fit 可用内存的 "50%"，可以是字节数或比例；对应 fib 的 `Config.MaxPendingBytes`）。它使许多慢客户端合起来不能占用每个连接
水位所允许的全部内存：100,000 个连接、每个 256 KiB 的水位，最多可排队 25 GiB。

**证据。** 在 akio `dad77a7a`（以及 `main` 的 `111004f8`），`akio_core::Config` 有 `write_high_watermark`（按连接暂停读取）和
`max_queued_bytes`（按连接拒绝发送），遥测也汇总了排队的字节（`queued_bytes`），但没有任何东西按这个总和暂停读取。akio 的路线图在
背压下列出了“引擎级的 `max_pending_bytes`”，开发计划也勾选了它，但代码中没有这个名字或含义的设置。akit 无法在其上自行实现：那需要
从拥有这些连接的 I/O 线程之外暂停每个连接的读取，这是 akio 的职责（规则 21）。

**建议的 API。** `Config::max_pending_bytes: Option<usize>`（默认 `None`，可在运行时重载）：当引擎所有连接排队的字节总数达到或
超过它时，每个连接在当前这次读取之后暂停读取，就像水位暂停单个连接那样；总数回落到比如四分之三以下时恢复。在水位的计数
（`backpressure`）旁边增加一个由它引起的暂停计数，以便 akit 的 `akit_reads_paused_total{reason="budget"}` 报告它；再提供一个把内存
比例换算为字节的辅助函数（机器的内存，或更低的 cgroup 限制），或由 akit 自己换算。

**akit 将如何使用。** `akit-listener` 的 `engine_settings` 在启动时按 `limits.max_pending_bytes` 设置它（字节数，或内存比例，默认
"50%"，读不到内存时 256 MiB），与 fit 相同；`supported.txt` 标记该字段，`make v1-bless` 标记这项增加，门禁中的 `limits` 和 `proxy`
示例变为有效。

## A215 只计入被接受连接的连接上限

**状态。** 已在 akio 分支 `claude/i79b-akio` 上实现（I79b，2026-11-26），尚未进入 akio 的 main：`Config::max_accepted_connections`（akio 的
ADR-173），基于 A30 的 `Telemetry::open_accepted` 计数器。akit 的 I79b 分支用 `limits.max_conns` 设置它以代替
`max_connections`，参考文档中的说明已删除；锁定版本更新之前，akit 的 main 仍使用 `max_connections`。

**问题。** fit 的 `limits.max_conns` 约束“监听器同时保持打开的客户端连接，所有监听器合计”。akit 的上游连接池在为监听器服务的同一个
引擎上拨号，所以使用 akio 的上限时，繁忙代理的上游连接会占用客户端的名额：在默认的 100,000 下，若有 30,000 个上游连接打开，第
70,001 个客户端就会被拒绝。

**证据。** 在 akio `dad77a7a`，`Config::max_connections` 的计数是“引擎范围的，包括拨出的连接和 UDP 对端”，虽然超出时只关闭新接受的连接。

**建议的 API。** `Config::max_accepted_connections: usize`（默认 0，即不限制；与 `max_connections` 一样可在运行时重载）：引擎各监听器
接受的 TCP 连接超过它时，新连接在 `accept` 之后立即关闭并计为 `rejected`，与引擎拨出多少连接无关。

**akit 将如何使用。** `engine_settings` 用 `limits.max_conns` 设置它，取代 `max_connections`，参考文档中关于 `limits.max_conns` 的说明随之删除。

## A220 UDP 和数据报监听器上的 accept 过滤器

**完成。** 已进入 akio 的 main（`9e21ada7`，akio 的 ADR-178）并锁定：UDP 监听器上的 `Listener::filter`、`Engine::bind_udp_filtered`、`UdpBinding::set_filter`，以及 akio-quic 的服务器在 Initial 包建立连接之前再次询问它。akit-listener 给监听器的 UDP 套接字（HTTP/3 的，或 `udp` 服务的）设置其对端所遵循的策略，每次重载在保留的套接字上重新设置；HTTP/3 分发器的检查（`Dispatcher::unfiltered`）和 `udp` 会话打开时的检查已删除，隧道 hub 的 QUIC 监听器也在过滤器处拒绝。

**状态。** 由 I85 提出（2026-11-26）。在此之前，akit 在不需要它的地方先行拒绝，这一权宜实现记录在[开发进度](progress.zh-CN.md)中：
HTTP/3 请求由监听器的分发器回答 403（I85），`udp` 会话在打开时被关闭（I86）。

**问题。** fit 的访问控制列表在为被拒绝的 UDP 对端建立会话或 QUIC 连接之前就丢弃它的数据报（fib 的 `Binding.Filter`，fit 的 A73），
就像在 accept 时关闭被拒绝的 TCP 连接一样。akio 的 accept 过滤器（A3，ADR-099）只用于 TCP 监听器：在 UDP 或数据报监听器上
`Listener::filter` 会以 `InvalidInput` 使构建失败，`Engine::set_listener_filter` 回答 `Unsupported`。因此被拒绝客户端的 QUIC
握手会完整进行（它发送的每个 Initial 包都要做 TLS 工作），之后它的请求才被回答 403；被拒绝的 `udp` 客户端的每个数据报都会打开
一个连接，再由 akit 立即关闭。

**证据。** 在 akio `6bcd8ad6` 中，`Engine::swap_filter` 拒绝 `Tcp` 以外的任何 `Kind`（`crates/akio/src/lib.rs`），`Listener::filter`
的文档写明“epoll、kqueue 和 io_uring 上的 TCP 监听器”。

**建议的 API。** 同样的 `Fn(&SocketAddr) -> bool`，对尚无连接的对端的每个数据报询问：`Listener::udp` 上的 `Listener::filter`
（以及 `Engine::bind_udp` 绑定的监听器上，通过该调用或 `UdpBinding` 上的 `filter`），由 `Engine::set_listener_filter` 和
`UdpBinding::set_filter` 替换；被拒绝的数据报被丢弃并计入 `LoopCounters::filtered`，不记住该对端（这样策略变化立即生效，也不会随着
被拒绝的对端增多而增长，同 fit 在 I195 中的决定 7）。akio-quic 的服务器在连接的第一个 Initial 包上询问它。

**akit 将如何使用。** akit-listener 把监听器 TCP 一侧的访问策略也装到它的 UDP 一侧上，HTTP/3 分发器不再逐个检查请求
（`Dispatcher::unfiltered`），akit-l4 的 `udp` 会话不再在打开时检查。

## A225 对 akio 的密码学、TLS、HTTP 解析器和 WebAssembly 的外部安全审查

**状态。** 由 I80b 为威胁模型的 L15 提出（2026-11-26）。不是 API：是由 akio 委托或参与的审查。akit 中没有任何东西等待它；
akit 自己的外部审查（[审查范围](security/review-scope.zh-CN.md)）不包括 akio 的内部。

**问题。** akit 提供的每个连接都经过 akio 自己的 TLS 1.2/1.3（akio-tls）、X.509 验证（akio-x509）、密码学（akio-crypto）、
QUIC，以及 HTTP/1 和 HTTP/2 解析器（akio-http），每个 `wasm` 插件都经过 akio-wasm 的解释器。它们都不是经过广泛审查的库：
都是在 akio 中编写的，只经过 akio 的内部审查。

**证据。** akio 1.0 的检查：对协议解析器的模糊测试、cargo-audit 和 cargo-deny、unsafe 审查、Miri、loom、sanitizer，以及对
密码学的常数时间检查（akio 的 `docs/unsafe-review.md`、`docs/constant-time.md`）。[威胁模型](security/threat-model.zh-CN.md#已知的局限)
把缺少外部审查列为 L15；审查范围的 [akio](security/review-scope.zh-CN.md#akio) 一节建议在 akit 的 1.0 依赖它们之前进行审查。

**建议的 API。** 无。一次外部审查，顺序为：akio-crypto 和 akio-tls 连同 akio-x509（握手、记录层、证书和 CRL 验证、常数时间
路径），然后是 akio-http 的 HTTP/1 和 HTTP/2 解析器（请求走私、HPACK、流量控制），然后是 akio-wasm（校验、沙箱的边界）。
其发现按 akio 的安全策略在 akio 中修复，报告从 akio 的文档中链接。

**akit 将如何使用。** 锁定带有修复的 akio 提交（规则 22），关闭威胁模型的 L15，并从威胁模型和审查范围链接报告。

## A230 akio-crypto 中的 Argon2id

**状态。** 由 I93 提出（2026-11-26）。I93（加密的 store）等待它：没有它就无法读取 fit 的 store 格式。

**问题。** fit 以自己的文件格式 `fitstore` 密封保存其配置（fit 的决定 027、`admin/store`），akit 要读写同样的文件，使 store 能在
fit 和 akit 之间迁移（`admin.store`、`akit store export` 和 `import`）。密钥由 store 的密码通过 Argon2id（RFC 9106）派生，使用文件头
中写明的时间、内存和线程数（fit 写入时间 3、64 MiB、4 线程，即 RFC 9106 的第二推荐设置，读取时最多接受时间 16、1 GiB）。
akio-crypto 有 HKDF 和 PBKDF2（`kdf`）、bcrypt，但没有 Argon2，也没有 Argon2 所依赖的 BLAKE2b。

**证据。** 在 akio `077a6710` 中：`crates/akio-crypto/src/kdf.rs` 有 `hkdf`、`hkdf_extract`、`hkdf_expand` 和 `pbkdf2_hmac`；akio
的代码中没有任何地方提到 Argon2 或 BLAKE2。fit 的 `admin/store/store.go` 调用 `golang.org/x/crypto/argon2` 的
`argon2.IDKey(password, salt, time, memoryKiB, threads, 32)`。

**建议的 API。** 在 `akio_crypto::kdf` 中：`argon2id(password: &[u8], salt: &[u8], time: u32, memory_kib: u32, lanes: u8,
out: &mut [u8]) -> Result<(), Error>`（Argon2 版本 0x13，RFC 9106，无密钥和关联数据，同 Go 的 `IDKey`），检查 RFC 9106 的边界
（盐至少 8 字节、每个 lane 至少 8 KiB 内存、`time` 至少为 1），出错时返回错误而非 panic；内存一次性、可失败地分配，使过大的
`memory_kib` 成为错误而不是中止；各 lane 依次或在线程上计算，结果相同。同时提供 BLAKE2b（RFC 7693），即 `hash::Blake2b`
（可变输出长度，Argon2 的 H' 需要）。用 RFC 9106 的测试向量和 Go 的 `argon2.IDKey` 在 fit 参数下的输出测试，读取密码的部分为常数时间。

**akit 将如何使用。** akit-admin 的 `store` 在启动时于控制线程上派生一次 store 的密钥（不到一秒、64 MiB，从不在 I/O 线程上），
派生前按 fit 的边界检查文件头中的参数；发给 Admin API 的导入文件在派生任何东西之前被限制在 fit 代价的两倍以内，同 fit 的 `Unseal`。

## A231 akio-crypto 中的 XChaCha20-Poly1305

**状态。** 由 I93 与 A230 一起提出（2026-11-26）。I93 等待它。

**问题。** `fitstore` 用 XChaCha20-Poly1305 密封其内容（24 字节的 nonce，每次写入随机生成，59 字节的文件头作为附加数据）。
akio-crypto 有 12 字节 nonce 的 ChaCha20-Poly1305（RFC 8439，`chacha::ChaCha20Poly1305`），但没有扩展 nonce 的构造
（先 HChaCha20 再 ChaCha20-Poly1305），因此既不能打开也不能写入 fit 的 store。

**证据。** 在 akio `077a6710` 中：`crates/akio-crypto/src/chacha/mod.rs` 有 `NONCE_LEN = 12`、`ChaCha20`、`Poly1305` 和
`ChaCha20Poly1305`，没有 HChaCha20 或 XChaCha20。fit 的 store 调用 `golang.org/x/crypto/chacha20poly1305` 的 `chacha20poly1305.NewX(key)`。

**建议的 API。** `chacha::hchacha20(key: &[u8; 32], nonce: &[u8; 16]) -> [u8; 32]`，以及 `chacha::XChaCha20Poly1305`：
`new(&[u8; 32])`、`XNONCE_LEN = 24`，和 `ChaCha20Poly1305` 相同的 `encrypt`、`decrypt` 及原地方法（子密钥由 HChaCha20 对 nonce
前 16 字节计算，内部 nonce 为四个零字节加其后 8 字节），同 draft-irtf-cfrg-xchacha 和 Go 的 `NewX`。用该草案的测试向量、
Wycheproof 的 `xchacha20_poly1305_test.json` 和 Go 的输出测试。

**akit 将如何使用。** akit-admin 的 `store` 用 A230 派生的密钥打开 store，以文件头作为附加数据，每次写入用新的随机 nonce
（`akio_crypto::rand`）密封，同 fit 的 `Store.seal`。

## A232 由处理函数设置 HTTP 连接的空闲超时并关闭它，以及已接受连接的表

**完成。** 已进入 akio 的 main（`9e21ada7`，akio 的 ADR-180）并锁定：`ConnHandle`（`close`、`close_at`、`close_after`、`cancel_close`、`set_idle_timeout`）、`Listener::on_accept` 和 `on_close`，以及 akio-http 的 `Context::connection()`。控制台在其上实现 fit 的规则：没有有效会话的连接在 accept 5 秒后关闭（此时正在处理的请求在其后同样长的时间内得到回答并带上 `Connection: close`），有会话的连接在两次请求之间保留 2 分钟，超过 `max_conns` 时为新连接关闭占用最多的客户端最旧的未认证连接（`console::slots`，即 fit 的 `login.Slots`）。权宜实现（对没有有效会话的请求的回答关闭其连接）已删除。Admin API 仍在 accept 时按 `max_conns` 限制：fit 的 Admin API 对没有令牌的连接的规则尚未移植。

**状态。** 由 I90 提出（2026-11-26）。没有它控制台也能工作；在此之前，它的连接像 Admin API 的那样受限（一个权宜实现，
记录在[开发进度](progress.zh-CN.md)中）。

**问题。** fit 的控制台（以及自 fit 的 I161 起它的 Admin API）限制未登录客户端的连接：连接在被接受 5 秒后关闭，不论它发送
什么，直到带有效会话的请求到来；此后两次请求之间保持 2 分钟，会话结束时重新回到 5 秒规则。`max_conns` 个连接都已打开时，
新连接会关闭持有未登录连接最多的客户端（回环地址算作一个客户端）最旧的未登录连接，并为已登录的连接保留四分之一的名额，
而不是拒绝新连接。这样，未登录的客户端占不住控制台不多的连接，不断重连的攻击者也挡不住运维人员。akio-http 的服务器对所有
连接一视同仁地使用 `HttpConfig` 的超时（请求头、请求体、keep-alive），处理函数也拿不到自己的连接：既不能改变它的空闲超时，
也不能稍后关闭它；也没有任何东西把每个被接受的连接告诉应用，让应用选择关闭哪一个。

**证据。** 在 akio `077a6710` 中：akio-core 的 `Conn` 有 `set_idle_timeout` 和 `close`，但 akio-http 的 `Context` 两者都不提供
（没有 `conn()` 访问器）；`Listener::filter`（A3）只看到对端地址，并回答是或否。

**建议的 API。** `Context::connection(&self) -> ConnHandle`，一个可廉价克隆、可在任何线程使用的句柄，带
`set_idle_timeout(Option<Duration>)`、`close()` 和 `id()`；以及一个 `Listener::on_accept(Fn(ConnHandle, SocketAddr))` 钩子（或把句柄
交给过滤器），使应用能维护自己的已打开连接表并关闭它选中的那个。在 accept 时设置、由处理函数取消的截止时间
（`ConnHandle::close_at(Instant)`）可以不用应用自己的计时器就实现 5 秒规则。

**akit 将如何使用。** 控制台（以及 Admin API，同 fit 的 I161）维护其连接表：每个连接在 accept 后 5 秒关闭，除非带有效会话的请求
取消它；有效会话把空闲时间设为 2 分钟；超过 `max_conns` 时，关闭持有最多未登录连接的客户端最旧的未登录连接以腾出位置。

## A233 关闭终端回显以读取密码

**状态。** 由 I89 提出（2026-11-26）。没有它 `akit hash-password` 也能工作：从终端输入时密码会按输入原样显示，提示中说明这一点
以及如何隐藏（一个权宜实现，记录在[开发进度](progress.zh-CN.md)中）。

**问题。** `fit hash-password` 以原始模式、无回显地从终端读取密码，每个字符显示一个 `*`（golang.org/x/term 的 `MakeRaw`）。
这需要 `tcgetattr` 和 `tcsetattr`，它们需要 akit 禁止的 `unsafe`（libc）；akio-sys 没有终端相关的调用。

**证据。** 在 akio `077a6710` 中，`crates/akio-sys/src/process.rs` 有 `allowed_cpus`、`pin_current_thread`、`signal_pipe` 和 `kill`；
akio-sys 中没有任何地方提到 `termios`。

**建议的 API。** 在 akio-sys 中：`terminal::raw(fd: BorrowedFd<'_>) -> io::Result<RawGuard>`（或 `no_echo`），把终端置于原始模式
（或关闭 `ECHO` 和 `ICANON`），在 guard 被丢弃时恢复，对不是终端的描述符返回错误。

**akit 将如何使用。** `akit hash-password` 像 fit 那样在关闭回显的情况下从终端读取密码，每个字符写一个 `*`，关于密码被显示的说明随之去掉。

## A234 单个连接自己的发送队列上限

**状态。** 由 I101 提出（2026-11-27）。没有工作在等待它：在此之前隧道链路用自己的高水位调节喂给它的一方（记录在
[进度](progress.zh-CN.md)中的临时做法）。

**问题。** fit 把每条隧道链路的发送队列上限提高到其隧道可能在途的量：64 条隧道各 4 MiB 的窗口加上控制消息的余量
（`proto.LinkQueue`，约 260 MiB，fib 的 `SetQueueLimit`），使链路永远不会让其隧道窗口允许的发送失败。akio 对一个引擎的所有
连接只有一个上限 `Config::max_queued_bytes`，akit 用 `limits.max_write_queue_bytes`（默认 32 MiB）设置它：网关的客户端和隧道
链路共用它，超过它的链路发送失败并关闭。LLM mesh 的链路（I104）承载每个 256 KiB 窗口的流，最多 `limits.max_inflight` 条，
也会遇到同样的上限。

**证据。** 在 akio `077a6710` 中，`akio-core/src/config.rs` 有 `max_queued_bytes`（默认 64 MiB），`ConnInner::send` 超过它时以
`Overloaded` 失败；`Conn` 和 `ConnInner` 都没有设置单个连接上限的方法。

**建议的 API。** `Conn::set_max_queued_bytes(&self, n: Option<usize>)`（以及 `ConnInner` 上的同名方法）：用该连接自己的上限
代替引擎的，`None` 恢复为引擎的；从下一次发送起生效。

**akit 如何使用。** 隧道链路的连接取 fit 的 `LinkQueue`，去掉其 8 MiB 高水位（`akit_tunnel::link::HIGH_WATER`），使其隧道只由
窗口调节，与 fit 相同；LLM 链路取其流的窗口乘以 `max_inflight`。

## A235 akio-tls 客户端中固定的服务器证书

**状态。** 由 I102 提出（2026-11-27）。没有工作在等待它：在此之前隧道的 TLS 链路在握手完成之后、发送任何字节之前检查 pin
（记录在[进度](progress.zh-CN.md)中的临时做法）。

**问题。** fit 的隧道和 LLM mesh 在握手中按 SHA-256 检查 hub（或监听方）的自签名证书，代替证书链和名称（`certpin.Pinned`：
Go 的 `VerifyPeerCertificate` 配合 `InsecureSkipVerify`）：不匹配的证书使握手失败。akio-tls 的 `ClientConfigBuilder` 要么按根证书
检查证书链和名称，要么什么都不检查（`insecure_skip_verify`，但仍用叶证书的密钥检查服务器的 CertificateVerify）；它没有让调用方
选择证书的钩子，也没有握手完成时的回调。因此 akit 在 akio 的定时器线程上轮询 `tls_info` 直到握手完成，把叶证书与 pin 比较，
然后才启动链路：这是可靠的（签名证明了持有密钥），但它是轮询，而且本应失败的握手完成了。

**证据。** 在 akio `077a6710` 中，`akio-tls/src/config.rs` 的 `ClientConfigBuilder` 有 `roots`、`client_cert`、
`insecure_skip_verify` 等，`proto/client.rs` 的 `peer_certificate` 要么 `verify_chain`，要么在 `insecure` 下取 `leaf_key`。

**建议的 API。** `ClientConfigBuilder::pin_sha256(pins: Vec<[u8; 32]>)`：服务器的叶证书必须哈希为其中之一（常数时间比较）；
不检查证书链、名称和日期；否则握手以 `bad_certificate` 失败。akio-quic 的客户端（其 TLS 是 akio-tls 的）同样如此。

**akit 如何使用。** 隧道的 TLS 链路（以及 I102b 的 QUIC 链路）和 LLM mesh 的 `wss://` 链路（I104）把 pin 交给客户端配置并立即
启动链路，与 `ca_file` 时相同；`crate::tls::when_handshaken` 随之去掉。

## A236 压住 WebSocket 的读取、得知其发送队列排空、取得对端地址

**状态。** 由 I102c 提出（2026-11-27）。I102c（隧道的 `ws://` 和 `wss://` 链路）和 I104（LLM mesh，其链路是 WebSocket）在
等待它。

**问题。** 隧道链路和 LLM mesh 链路承载许多流，流的字节来自其他连接：链路的队列超过高水位时，喂给它的一方被压住，队列排空后
恢复；链路交付的数据写不出去时（慢服务、hub 另一侧拥塞的链路），压住链路自己的读取，让对端的窗口停住它（规则 8 和 9；fit 的
`transport.Conn` 在 WebSocket 链路上和 TCP 链路上一样有 `SetReadPaused` 和 `OnDrain`）。akio-ws 的 `WsSender` 只有
`queued_bytes`：无法停止读取 WebSocket，队列排空时没有回调，也没有对端地址（hub 按地址的限制和访问策略需要它）。在其上的链路
只能丢弃或轮询。

**证据。** 在 akio `077a6710`（以及 akio main 的 `88618b83`）中，`akio-ws/src/session.rs` 的 `WsSender` 有 `send_*`、`ping`、
`pong`、`close`、`subprotocol`、`is_open` 和 `queued_bytes`；`WsHandler` 有 `on_open`、`on_message`、`on_message_ref`、
`on_pong` 和 `on_close`；akio-ws 和 akio-http 的升级胶水（`Context::upgrade_websocket`）都不对底层连接调用 `hold_reads` 或
`on_drain`。

**建议的 API。** 在 `WsSender` 上：`hold_reads(&self, hold: bool)`（TCP 或 TLS 上的 WebSocket 用连接的，HTTP/2 或 HTTP/3 上的
用流的）、`on_drain(&self, f: impl FnOnce() + Send + 'static)`（队列为空时调用一次，如 `Conn::on_drain`）、
`peer_addr(&self) -> Option<SocketAddr>`，以及 `set_relayed(&self, bool)`（边读边写的中继需要它，如 `Conn::set_relayed`）。
或者 `WsHandler::on_drain(&mut self, ws: &WsSender)`，在超过水位的队列排空时调用。

**akit 如何使用。** WebSocket 链路的 `Wire`（`akit_tunnel::link::Wire`）把 `hold_reads`、`on_drain`、`queued` 和 `peer` 映射到
它们上，如 `ConnWire` 映射到 TCP 连接上那样；LLM mesh 的会话在某条流的读者慢过其窗口时压住链路的读取。

## A237 在线程局部变量的析构中调用 `Engine::wait` 也安全

**状态。** 2026-11-27 akit 移到 akio `9e21ada7` 时提出。在此之前，akit 的客户端引擎（OTLP 的、provider 的、共享 ACME 存储的）在为此
启动的线程上等待（`akit_observe::stop_client_engine`），这一权宜实现记录在[开发进度](progress.zh-CN.md)中。

**问题。** 自 `9e21ada7` 起，`Engine::wait` 先用 `Pool::is_current` 询问自己是否运行在引擎的池线程上，它用 `with` 读取 akio-pool 的
线程局部变量 `CURRENT`。线程退出时，其线程局部变量按不确定的顺序销毁；由另一个线程局部变量的析构函数丢弃的引擎（akit 的情况：
线程缓存的运行时图，其最后一个引用丢弃客户端引擎的所有者）会发现 `CURRENT` 已经销毁，`with` panic，而线程局部析构中的 panic 会使
进程中止。akit 的 `acme_shared` 测试在 macOS 上因此中止（`a_challenge_nobody_has_is_404`）；持有运行时图最后一个引用而退出的 I/O 线程
也可能如此。

**证据。** 在 akio `9e21ada7` 中：`crates/akio/src/lib.rs` 的 `Engine::wait` 调用 `self.pool.is_current()`；
`crates/akio-pool/src/lib.rs` 的 `Pool::is_current` 是 `CURRENT.with(...)`，`CURRENT` 是 `RefCell<Option<Weak<Pool>>>`（它有析构
函数，所以随线程销毁）。`timer::on_loop` 读取的是没有析构函数的 `Cell<u64>`，是安全的。

**建议的 API。** 不需要新 API：`Pool::is_current`（以及 `on_pool_thread`，和等待或 join 路上读取的任何线程局部变量）改用 `try_with`，
已销毁的线程局部变量视为“不是池线程”。

**akit 如何使用。** `stop_client_engine` 恢复为在调用线程上 `shutdown` 和 `wait`。
