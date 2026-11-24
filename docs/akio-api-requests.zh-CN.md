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
| A3 | 在任何 handler 运行之前过滤已接受的连接 | 已在 akio 的 main（`a9a620ab`），已锁定 | 11 | 访问控制列表 |
| A4 | PROXY 协议 v1/v2：接受时解析，拨号时写出 | 已在 akio 的 main（`a9a620ab`），已锁定，I52 已使用 | 12 | I52 |
| A5 | akio-mux 中按 SNI 路由 TLS 而不终结它的规则 | 已在 akio 的 main（`a9a620ab`），已锁定，I52 已使用其 ClientHello 解析器 | 12 | I52、I53 |
| A6 | `akio-x509` 中生成 CSR（以及 `akio-crypto` 中的 JWS 辅助函数） | 已在 akio 的 main（`afbbb521`），已锁定 | 15 | I31 |
| A7 | 不转码即可切分文件的 MP4（ISO BMFF）读取器 | 已在 akio 的 main（`a9a620ab`），已锁定 | — | 媒体库 |
| A8 | TUN 设备 | 已在 akio 的 main（`a9a620ab`），已锁定 | — | VPN |
| A9 | 把 TLS 服务端限制在 TLS 1.2（`ServerConfigBuilder` 的公开 API） | 已提出（I09） | 17 | I09 |
| A10 | 立即关闭 HTTP 客户端的空闲连接 | 已提出（I14） | 10、11 | I14、I22 |
| A11 | HTTP/1 客户端请求保留自己的 `Host` 字段（给定空 `User-Agent` 时不发送 `User-Agent`） | 已在 akio 的 main（`a9a620ab`），已锁定 | — | I15 |
| A12 | HTTP 客户端的连接超时也作用于单个地址 | 已提出（I14） | 10 | I14、I35 |
| A13 | `Files` 服务调用者给出的路径 | 已在 akio 的 main（`a9a620ab`），已锁定 | 19 | I12、I13 |
| A14 | `Files` 像网关的静态文件 handler 那样应答 | 已提出（I12） | 19 | I12、I13 |
| A15 | `Files` 的内存有界，磁盘池有超时 | 已提出（I12） | 4、11 | I12 |
| A16 | `Files` 在配置的编码中按客户端的 q 值选择压缩副本 | 已在 akio 的 main（`a9a620ab`），已锁定 | 19 | I13 |
| A17 | `Files` 依次尝试候选路径（`try_files`） | 已在 akio 的 main（`a9a620ab`），已锁定 | 19 | I13 |
| A18 | `Files` 列出没有索引文件的目录 | 已在 akio 的 main（`a9a620ab`），已锁定 | 4、11、19 | I13 |
| A19 | HTTP 客户端请求的请求体边产生边发送，带背压 | 已在 akio 的 main（`60547435`），已固定，I16 已使用 | 5、8 | I16、I17、I18、I49、I75 |
| A20 | HTTP 客户端响应边到达边交付，可暂停和恢复读取 | 已在 akio 的 main（`60547435`），已固定，I16 已使用 | 5、8 | I16、I17、I18、I67 |
| A21 | 服务端请求可暂停其客户端的读取，并在客户端排空时得到通知 | 已在 akio 的 main（`60547435`），已固定，I16 已使用 | 5、8 | I16、I47、I75 |
| A22 | 在运行中的引擎上移除和排空 UDP 监听器 | 已提出（I22） | 10、11 | I22、I28 |
| A23 | Unix 域流式监听器（可设套接字文件权限） | 已提出（I26） | 11 | I26、I29 |
| A24 | 关闭连接的 HTTP/1 响应，并停止读取已拒绝的请求体 | 已提出（I26） | 7、10、11 | I26、I27、I41 |
| A29 | 不分配内存的应答钩子：监听器的观察者，带着请求设下的几个字被调用 | 已提出（I37） | 13 | I21、I37、I64 |
| A30 | 监听器接受的打开连接数，与拨出的连接和 UDP 对端分开 | 已提出（I37） | — | I37 |
| A32 | `akio-regex` 模式在哪里匹配、它的分组，以及替换 | 已在 akio 的 main（`e9891b52`），已锁定 | 19 | I40、I48 |
| A33 | 被中间件修改的请求字段和请求目标（`Context::req_mut`） | 已提出（I40） | 12、19 | I40、I43、I44 |
| A35 | 按长度分帧的流式 HTTP/1.1 响应 | 已提出（I16） | 5 | I16、I67 |
| A36 | HTTP 客户端请求的响应头超时和空闲超时 | 已在 akio 的 main（`4f7169c4`），已锁定，I17b 已使用 | 10 | I17b、I18 |
| A37 | 可在任意时刻取消的 HTTP 客户端请求 | 已在 akio 的 main（`4f7169c4`），已锁定，I17b 已使用 | 3 | I17b、I45 |
| A38 | 能得知客户端离开的分离服务端请求；可在运行时修改的写停滞超时 | 已在 akio 的 main（`4f7169c4`），已锁定，I17b 已使用 | 3、10 | I17b、I49 |
| A45 | 公开的定时器：延迟过后运行一次回调，可从任何线程调度 | 已提出（I35） | 1、11 | I35、I31、I52、I54 |
| A48 | 获知请求的结束：响应已完整发出，或等待中客户端已离开 | 已提出（I41） | 3、11、13 | I41、I17、I21、I37 |
| A49 | 由中间件限制请求体大小，无论谁读取请求体 | 已提出（I41） | 7、11 | I41 |
| A51 | `H2Client` 的流式请求：重置、超时、请求 trailer，以及遇到 `GOAWAY` 时报告失败而不是被遗忘 | 已在 akio 的 main（`f6750e93`，ADR-110），已锁定，I18b 已使用 | 3、10 | I18b、I50 |
| A52 | `H2Client` 按流的背压：暂停流式响应的读取，报告上传队列 | 已在 akio 的 main（`f6750e93`，ADR-110），已锁定，I18b 已使用 | 8 | I18b、I50 |
| A53 | `H2Client` 的连接：遵守并报告服务器的流数上限、连接超时、ALPN 回退到 HTTP/1.1 | 已在 akio 的 main（`f6750e93`，ADR-110），已锁定，I18b 已使用 | 10、11 | I18b、I54 |
| A54 | 空闲的 HTTP/1 连接在宽限期后排空，而非立即关闭 | 已提出（I28） | 10 | I28 |
| A57 | 握手之外可用的根证书：用系统的根证书校验证书链 | 已提出（I32） | 1、19 | I32 |
| A58 | HTTP 服务能看到 TLS 连接协商出的 ALPN 协议（或关闭这样的连接） | 已提出（I32） | 19 | I32、I34 |
| A63 | `akio-crypto` 中的 bcrypt 密码哈希 | 已在 akio 的 main（`4f7169c4`），已锁定 | 1、4 | I42 |
| A64 | 每个服务端请求上的客户端已验证证书 | 已在 akio 的 main（`4f7169c4`），已锁定 | — | I42 |
| A65 | 由中间件挂起、之后带着上下文恢复的请求 | 已在 akio 的 main（`4f7169c4`），已锁定 | 1、3、4 | I42、I43 |
| A66 | `H2Client` 的请求遵守 `response_header_timeout`、`response_idle_timeout` 和 `CancelToken` | 已在 akio 的 main（`d27e6a20`，ADR-111），已锁定，I18b 已使用 | 3、10 | I17b、I18b |
| A75 | 挂起消息体流式读取的请求，并暂停其消息体的读取直到恢复 | 已提出（I43） | 5、7、8 | I42、I43、I44 |
| A78 | 每个连接（TCP 或 UDP 对端）各自的空闲超时，由其处理器设置 | 已提出（I51） | 10、11 | I51、I55 |
| A79 | 带各自连接超时的 `Engine::dial` 与 `dial_udp` | 已提出（I51） | 10 | I51、I54、I55 |
| A90 | 由调用方自己的响应过滤器设置流式响应的编码，以及保留到响应发出为止的值 | 已在 akio 的 main（`c5cf4353`），已锁定 | 8、11 | I47 |
| A91 | `Files` 为内存中的文件保存压缩副本 | 已在 akio 的 main（`c5cf4353`），已锁定 | 4、12 | I47 |
| A92 | 从磁盘文件发送的消息体按流编码压缩 | 已提出（I47） | 1、5 | I47 |
| A93 | HTTP/1 客户端在 `101 Switching Protocols` 时交出连接 | 已在 akio 的 main（`c5cf4353`），已锁定 | 8、9 | I49、I55 |
| A94 | 对 HTTP/2 扩展 CONNECT 的流式 `2xx` 应答即两半各自关闭的隧道 | 已在 akio 的 main（`c5cf4353`），已锁定 | 9 | I49 |
| A105 | 请求的对端是受信任 PROXY 头所指的客户端（akio-http） | 已在 akio 的 main（`e9891b52`），已锁定 | 19 | I52 |
| A108 | HTTP/1 客户端的 TLS 服务器名，代替 URL 的主机名 | 已在 akio 的 main（`e9891b52`），已锁定 | 19 | I54 |
| A109 | 构建 TLS 配置时检查客户端证书的私钥与叶证书相符 | 已在 akio 的 main（`e9891b52`），已锁定 | 19 | I54 |
| A111 | 响应过滤器看到流式消息体发出的字节及其结束方式（消息体旁路） | 已在 akio 的 main（`e9891b52`），已锁定 | 5、6 | I67 |
| A112 | 按 RFC 9110 要求接收方接受的三种格式解析 HTTP 日期 | 已在 akio 分支 `claude/a111-body-tap`（`0e36f8d6`）上实现，已锁定 | 15 | I67 |
| A113 | 在内层过滤器之后运行的响应过滤器（外层过滤器） | 已在 akio 分支 `claude/a111-body-tap`（`0e36f8d6`）上实现，已锁定 | 5 | I67 |
| A117 | 从 `akio` 同时取得 `akio::Pool` 与 `PoolConfig`：应用自己的、有上限的阻塞工作线程池 | 已提出（I68）；期间 akit-cache 直接依赖 `akio-pool` | 4 | I68 |
| A135 | 拨号 Unix 域流套接字，以及在其上的 HTTP 客户端（akio、akio-http、akio-sys） | 已在 akio 分支 `claude/a135-unix-dial`（`0f2c419b`，ADR-120）上实现，已锁定 | 1、2 | I58 |

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

## A10 关闭 HTTP 客户端的空闲连接

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

## A24 关闭连接的 HTTP/1 响应

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

**问题。** fit 的 `fit_connections_open` 统计监听器上打开的客户端连接。akio 的 `Telemetry::open_connections`
统计所有打开的连接，包括引擎拨出的（akit 在监听器的引擎上拨出的上游连接池连接）和 UDP 对端，所以 akit 的这个
指标会多出它的上游连接；而按循环的计数（`accepted`、`dialed`、`closed`）也不区分关闭的连接属于哪一种。

**证据。** `crates/akio-core/src/telemetry.rs`（`a9a620ab`）：`conn_opened(limited)` 对每种连接都加到同一个
`open` 计数（`akio-epoll/src/reactor.rs` 和 `akio-uring/src/reactor.rs` 对拨出的连接以 `limited` 为 false 调用
它）；`conn_closed` 不分种类地减去。

**建议的 API。** `Telemetry::open_accepted()`（或 `LoopCounters` 中按循环的 `accepted_open` 指标）：现在打开的
已接受 TCP 连接，不含拨出的连接和 UDP 对端。

**akit 的用法。** `akit_listener::metrics::Scrape` 把它写作 `fit_connections_open`，替代 `open_connections`。

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

## A35 按长度分帧的流式 HTTP/1.1 响应

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

## A49 由中间件限制请求体大小

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
I17 的 `Fit-Error` 尾部字段结束）。

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
