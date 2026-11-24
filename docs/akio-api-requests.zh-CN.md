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
| A4 | PROXY 协议 v1/v2：接受时解析，拨号时写出 | 已在 akio 的 main（`a9a620ab`），已锁定 | 12 | I52 |
| A5 | akio-mux 中按 SNI 路由 TLS 而不终结它的规则 | 已在 akio 的 main（`a9a620ab`），已锁定 | 12 | I52、I53 |
| A6 | `akio-x509` 中生成 CSR（以及 `akio-crypto` 中的 JWS 辅助函数） | 已在 akio 分支 `claude/x509-csr`（`7ea64069`）完成，未合并 | 15 | I31 |
| A7 | 不转码即可切分文件的 MP4（ISO BMFF）读取器 | 已在 akio 的 main（`a9a620ab`），已锁定 | — | 媒体库 |
| A8 | TUN 设备 | 已在 akio 的 main（`a9a620ab`），已锁定 | — | VPN |
| A9 | 把 TLS 服务端限制在 TLS 1.2（`ServerConfigBuilder` 的公开 API） | 已提出（I09） | 17 | I09 |
| A10 | 立即关闭 HTTP 客户端的空闲连接 | 已提出（I14） | 10、11 | I14、I22 |
| A11 | HTTP/1 客户端请求保留自己的 `Host` 字段（给定空 `User-Agent` 时不发送 `User-Agent`） | 已在 akio 的 main（`a9a620ab`），已锁定 | — | I15 |
| A12 | HTTP 客户端的连接超时也作用于单个地址 | 已提出（I14） | 10 | I14 |
| A13 | `Files` 服务调用者给出的路径 | 已在 akio 的 main（`a9a620ab`），已锁定 | 19 | I12、I13 |
| A14 | `Files` 像网关的静态文件 handler 那样应答 | 已提出（I12） | 19 | I12、I13 |
| A15 | `Files` 的内存有界，磁盘池有超时 | 已提出（I12） | 4、11 | I12 |
| A16 | `Files` 在配置的编码中按客户端的 q 值选择压缩副本 | 已在 akio 的 main（`a9a620ab`），已锁定 | 19 | I13 |
| A17 | `Files` 依次尝试候选路径（`try_files`） | 已在 akio 的 main（`a9a620ab`），已锁定 | 19 | I13 |
| A18 | `Files` 列出没有索引文件的目录 | 已在 akio 的 main（`a9a620ab`），已锁定 | 4、11、19 | I13 |
| A19 | HTTP 客户端请求的请求体边产生边发送，带背压 | 已提出（I16） | 5、8 | I16、I17、I18、I49、I75 |
| A20 | HTTP 客户端响应边到达边交付，可暂停和恢复读取 | 已提出（I16） | 5、8 | I16、I17、I18、I67 |
| A21 | 服务端请求可暂停其客户端的读取，并在客户端排空时得到通知 | 已提出（I16） | 5、8 | I16、I47、I75 |
| A22 | 在运行中的引擎上移除和排空 UDP 监听器 | 已提出（I22） | 10、11 | I22、I28 |
| A23 | Unix 域流式监听器（可设套接字文件权限） | 已提出（I26） | 11 | I26、I29 |
| A24 | 关闭连接的 HTTP/1 响应，并停止读取已拒绝的请求体 | 已提出（I26） | 7、10、11 | I26、I41 |
| A29 | 不分配内存的应答钩子：监听器的观察者，带着请求设下的几个字被调用 | 已提出（I37） | 13 | I21、I37、I64 |
| A30 | 监听器接受的打开连接数，与拨出的连接和 UDP 对端分开 | 已提出（I37） | — | I37 |
| A32 | `akio-regex` 模式在哪里匹配、它的分组，以及替换 | 已提出（I40） | 19 | I40、I48 |
| A33 | 被中间件修改的请求字段和请求目标（`Context::req_mut`） | 已提出（I40） | 12、19 | I40、I43、I44 |

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
I34 将在签发线程中稍后完成 `CertDone`。

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
I49（WebSocket）和 I75（FastCGI）也会用到。在此之前，akit 的代理整体转发请求体，不超过 `limits.max_body_bytes`
（记录在 I16 的进度条目中）。

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
`hold_reads(false)`。I17（trailer、空闲超时、取消）、I18（h2 上游）和缓存（I67，响应经过时存储）也会用到。在此之前，
akit 的代理整体发回响应，每个最多 akio 的 64 MiB（记录在 I16 的进度条目中）。

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
`BodyStream::on_drain` 中恢复。压缩（I47）和 FastCGI（I75）也会用到。在此之前，akit 在限制内整体缓冲被代理的请求体
和响应体（记录在 I16 的进度条目中）。

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

**akit 的用法。** Admin API 对超过 `max_config_bytes` 的加载回 413（I26）后关闭连接，与 fit 一样；I41 在数据面上的
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
