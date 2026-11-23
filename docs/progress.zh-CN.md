# akit 开发进度

[English](progress.md) | [简体中文](progress.zh-CN.md)

本文件按迭代记录实际完成的内容，以及下一次会话从哪里继续。[开发计划](dev-plan.zh-CN.md) 说明每个迭代应该做
什么；本文件说明实际做了什么，以及与计划不同的地方。

## 当前状态

| | |
| --- | --- |
| 最后更新 | 2026-11-24 |
| 里程碑 | M0 基础已完成（I01–I08）；M1 代理核心：I09–I12、I14、I19、I22、I23、I24 已完成，I15 进行中（等待 akio 的 A11），I13 进行中（try_files 和 browse 等待 akio 的 A17 和 A18）。 |
| akio | akio `main` 上的 `50a82315`（`1.0.0-rc.1`），在 `Cargo.toml` 中按 `rev` 锁定 |
| fit | 作为规格；本计划基于 `7daee573` 阅读 |
| 可用功能 | `akit run` 和 `akit validate`，带 fit 的参数和退出码，JSON、`.conf` 和 YAML 配置，文件和目录合并；SIGHUP 重载配置（错误的配置保留正在运行的配置；监听器保留、增加和移除，启动时的监听器要等锁定到带 A1 的 akio）；SIGINT 和 SIGTERM 时优雅关闭；HTTP/1.1、h2c、TLS（SNI、ALPN、客户端认证）和 HTTP/3 监听器，TCP 一侧带 `Alt-Svc`；基于规范化路径的路由器 v1；`respond`、`static`（条件请求和范围请求，文件在 akio 的线程池上打开）和 `proxy` handler（fit 的转发字段、可信代理；锁定到 A11 后传客户端的 `Host`）；上游连接池，轮询、加权轮询和最少连接；兼容性门禁：80 个夹具，8 个有效，71 个只因暂不支持被拒绝，1 个跳过，没有失败 |
| 下一步 | 把 akio 锁定到带 A1–A8 和 A11 的 akio main，然后 I15 完成（其 `Host` 测试）、I22 改用 A1；再是 I13、I16、I17、I20、I21 |
| akio 需求 | A1–A8 已合并进 akio 的 main（2026-11-24）；A11 在 akio 分支 `claude/i15-proxy-699f18` 上；A9、A10、A12–A18 已提出（[akio API 需求](akio-api-requests.zh-CN.md)） |

## 日志

### 2026-11-24：I13，静态文件 II（进行中：try_files 和 browse 等待 akio）

- `precompressed`（akit-config 的 `Static::precompressed`、`Encoding`）：fit 的 `br`、`zstd`、`gzip` 列表，每个值像
  fit 一样检查（在 `precompressed[i]` 报 `"deflate" is not one of "br", "zstd", "gzip"`）。akio-http 的 `Files`
  把它在文件旁找到的副本（`name.br`、`name.zst`、`name.gz`）提供给 `Accept-Encoding` 中列出相应编码的客户端：来自
  内存，或超过 `max_file_bytes` 时来自磁盘并带副本的长度；`Content-Encoding`、`Vary: Accept-Encoding`、原文件的
  `Content-Type`、副本自己的 `ETag`（条件请求作用于它）、副本的范围请求；副本被修改或删除时与普通文件一样跟进。
  handler 的代码没有变化：无论配置怎么写，`Files` 都这样做。
- 大文件的 `sendfile`：I12 起已经有了。`Files` 把大于 `max_file_bytes` 的文件（或副本）从保留的句柄以 `FileBody`
  发送，akio 在明文 HTTP/1 上用 `sendfile` 发送；在 TLS 和 h2 上 akio 分块读取。akit 无需添加什么。
- `try_files`：像 fit 一样检查（每一项以 `/` 或 `{path}` 开头；`dotfiles` 为 `deny` 时除 `/.well-known/` 下外不指向
  点文件），然后拒绝为尚不支持，并注明 akio API 需求 A13 和 A17：`Files` 只提供请求给出的路径并自己答 404，akit
  既不能尝试另一个路径，也无法在不做 `Files` 在其池中所做的磁盘工作的情况下得知路径不存在。
- `browse`（与 I12 的 `disk_io` 一样被读取）：`false`（fit 的默认值）被接受；`true` 被拒绝为尚不支持并注明 A18：`Files` 在它的磁盘池上对没有索引文件
  的目录答 404，akit 无法在那里列出目录。`browse_max_entries` 被接受（不小于 0 的整数；只限制目录列表）。
  `compress` 仍被拒绝（I47 的压缩）。
- 已提出：A16（在配置的编码中按 q 值选择副本）、A17（`Files` 上的 `try_files`）、A18（`Files` 上的目录列表），见
  [akio API 需求](akio-api-requests.zh-CN.md)。
- 兼容性门禁：支持 11 个 static 字段（共 55 个）；fit 的 `examples/static` 现在只因 `browse`、`max_cache_bytes` 和
  `middleware` 被拒绝（`make compat-bless`）。
- 没有添加基准场景：fit 的测试框架没有针对副本、`try_files` 或目录列表的场景（它的配置不提供任何预压缩内容，见基准
  README），因此 akit 的也没有。
- 测试（`akit-handler/tests/static_files.rs`，现在 13 个）：`precompressed`，移植 fit 的 TestPrecompressed（11 个
  `Accept-Encoding` 字段、每种表示各自的 ETag 和 `If-None-Match`、副本的范围请求、`HEAD`、来自磁盘的副本、只列一种
  编码、不列编码）；`precompressed_follows_the_disk`（`check_interval` 为 0 时副本被重写、删除和新增）；
  `large_from_disk` 现在包含 fit 来自磁盘的大副本及其长度；`config_errors` 包含 fit 的 TestFeatureErrors 和
  TestDotfilesConfig 中关于 `try_files`、`browse` 和 `precompressed` 的部分，以及被拒绝为尚不支持和被接受的内容。
- 已检查：macOS 上的 clippy（`-D warnings`）和 `make check`；Linux（基准主机）上 akit-handler、akit-config 和
  akit-compat 的测试，epoll 和 io_uring，全部通过。
- 与计划和 fit 的不同之处：
  - 未完成：`try_files` 和 `browse`（A13、A17、A18），因此 I13 仍是 `doing`；回退的测试等待它们。
  - A16：`Files` 按 `br`、`zstd`、`gzip` 的顺序取第一个出现在字段里的编码，不看 q 值，也不看 `precompressed` 列了
    什么：`br;q=0` 得到 `br` 副本，`br;q=0.5, gzip` 得到 `br`（fit 发送 gzip），`*` 得不到副本（fit 发送列表中第一个），
    `GZIP` 和 `gzip ;q=0.8` 得不到副本。移植的测试对每种情况检查 akit 的回答并注明 A16。
  - A14（I12 的）：没有 `precompressed` 时仍提供副本，并带 `Vary`。
- 给基准任务：在 fit 的测试框架有对应场景之前没有；之后测量来自内存的副本和来自磁盘的大副本相对原文件的表现，以及每个
  请求匹配 `Accept-Encoding` 的开销。

### 2026-11-24：I22，SIGHUP 重载

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

### 2026-11-24：I15，反向代理 I：请求与头处理（doing：等待 akio A11）

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
