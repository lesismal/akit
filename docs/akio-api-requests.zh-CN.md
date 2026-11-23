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
| A1 | 在运行中的引擎上添加、移除和排空 TCP 监听器 | 进行中，akio 分支 | 10、11 | I22、I28 |
| A2 | 每次握手由回调选择服务端证书，回调可以稍后完成 | 进行中，akio 分支 | 1、4 | I30、I34 |
| A3 | 在任何 handler 运行之前过滤已接受的连接 | 进行中，akio 分支 | 11 | 访问控制列表 |
| A4 | PROXY 协议 v1/v2：接受时解析，拨号时写出 | 进行中，akio 分支 | 12 | I52 |
| A5 | akio-mux 中按 SNI 路由 TLS 而不终结它的规则 | 进行中，akio 分支 | 12 | I52、I53 |
| A6 | `akio-x509` 中生成 CSR（以及 `akio-crypto` 中的 JWS 辅助函数） | 已在 akio 分支 `claude/x509-csr`（`7ea64069`）完成，未合并 | 15 | I31 |
| A7 | 不转码即可切分文件的 MP4（ISO BMFF）读取器 | 进行中，akio 分支 | — | 媒体库 |
| A8 | TUN 设备 | 进行中，akio 分支 | — | VPN |
| A9 | 把 TLS 服务端限制在 TLS 1.2（`ServerConfigBuilder` 的公开 API） | 已提出（I09） | 17 | I09 |
| A10 | 立即关闭 HTTP 客户端的空闲连接 | 已提出（I14） | 10、11 | I14、I22 |
| A11 | HTTP/1 客户端请求保留自己的 `Host` 字段（给定空 `User-Agent` 时不发送 `User-Agent`） | 已在 akio 分支 `claude/i15-proxy-699f18`（`552fe283`）完成，未合并 | — | I15 |
| A12 | HTTP 客户端的连接超时也作用于单个地址 | 已提出（I14） | 10 | I14 |

## A1 运行中引擎上的 TCP 监听器

**问题。** `EngineBuilder::listen` 在 `build` 之前添加 TCP 监听器；`Engine` 在运行时有 `bind_udp`，但 TCP 没有对应的
方法，也没有办法关闭一个监听器并排空它的连接。fit 每次重新加载配置和每次 Admin API 修改都会替换监听器（fib 的
`AddBinding`、`RemoveBinding`、`ShutdownBinding`）。

**证据。** `crates/akio/src/lib.rs`：`EngineBuilder::listen`、`Engine::bind_udp`、
`Engine::adopt_tcp`（为在别处接受的流提供服务）。

**建议的 API。** `Engine::add_listener(Listener) -> io::Result<ListenerId>`、
`Engine::remove_listener(id)`（停止接受）、`Engine::shutdown_listener(id, drain: Duration)`
（停止接受，排空期过后关闭它的连接），监听器的后端和加速选项与构建时相同。

**akit 的用法。** I22 和 I28 在每次替换时对比新旧监听器的差异。在此之前的临时措施：akit 自己的监听 socket，由一个
accept 线程把流交给 `adopt_tcp`，并在 progress 中记录。

## A2 每次握手异步选择证书

**问题。** `ServerConfigBuilder::sni_cert` 在构建配置时就固定了证书。自动 HTTPS 会在证书续期时替换它们，按需 TLS
必须在握手期间签发证书，而且不能阻塞 I/O 线程。

**建议的 API。** 服务端配置上的解析器 `fn resolve(&self, hello: &ClientHello, done:
CertDone)`，`done` 可以稍后从其他线程以证书或告警完成；握手等待期间不占用线程。缓存命中的证书走同步快速路径。

**akit 的用法。** I30 从它的证书管理器提供证书；I34 按需签发。I09 用 `sni_cert` 从文件中选证书，它按名字精确
查找：第一个证书（即默认证书）以外的证书中的通配符名字（`*.example.com`）会被拒绝为尚不支持，直到解析器（其
`CertStore` 接受 `*.` 名字）进入 akio 的 `main`。

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

**已完成**（akio ADR-093）：`akio_x509::CsrParams::sign` 生成请求，`CertificateRequest` 解析并检查请求。JWS 不需要
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
