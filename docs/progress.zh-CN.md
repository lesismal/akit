# akit 开发进度

[English](progress.md) | [简体中文](progress.zh-CN.md)

本文件按迭代记录实际完成的内容，以及下一次会话从哪里继续。[开发计划](dev-plan.zh-CN.md) 说明每个迭代应该做
什么；本文件说明实际做了什么，以及与计划不同的地方。

## 当前状态

| | |
| --- | --- |
| 最后更新 | 2026-11-24 |
| 里程碑 | M0 基础已完成（I01–I08）；M1 代理核心：I09–I24 和 I17b 已完成（I18b 等待 akio）；M2 的 I26–I28、I30–I32、I35–I37 和 M3 的 I39–I42 已提前完成。 |
| akio | akio `main` 上的 `4f7169c4`（`1.0.0-rc.1`，带 A1–A8、A11、A13、A16–A21、A36–A38 和 A63–A65），在 `Cargo.toml` 中按 `rev` 锁定 |
| fit | 作为规格；本计划基于 `7daee573` 阅读 |
| 可用功能 | `akit run` 和 `akit validate`，带 fit 的参数和退出码，JSON、`.conf` 和 YAML 配置，文件和目录合并；SIGHUP 重载配置（错误的配置保留正在运行的配置；基于 akio 的 A1 保留（按 fit 的 `binding.same`：证书文件和 ACME 名字可以变）、增加、修改和排空监听器，包括启动时的监听器，重载可新增 h3 监听器；服务 h3 的启动监听器要重启才能变）；SIGINT 和 SIGTERM 时优雅关闭；HTTP/1.1、h2c、TLS（SNI、ALPN、客户端认证；按 fit 的方式每次握手选择证书，重载或 `-watch-certs` 替换证书而不关闭监听器）和 HTTP/3 监听器，TCP 一侧带 `Alt-Svc`；基于规范化路径的路由器 v1，带 fit 的匹配器（方法、头、query、远端 IP、路径正则）和表达式；路由中间件：`rewrite`（前缀）、`redirect`、`headers`、`rate_limit`、`concurrency_limit`、`body_limit`、`basic_auth`（bcrypt，在 akio 的线程池上校验）和 `client_cert`（按路由的 mTLS）；`respond`、`static`（条件请求和范围请求，文件在 akio 的线程池上打开，按 q 值选择预压缩副本，`try_files`，目录列表，`strip_prefix`）和 `proxy` handler（fit 的转发字段、可信代理、客户端的 `Host`、双向流式转发 body 并联动背压，双向透传 trailer，`timeout` 和上游的 `response_timeout`、`response_header_timeout`、`response_idle_timeout`，超时回答 504，客户端无论何时离开都放弃上游请求；经 HTTP/1.1、h2c 和 h2 访问上游）；所有监听器和代理上的 `limits.max_body_bytes`（413），以及 `limits.write_stall_timeout`（重载也可修改）；fit 的 json、text 和 line 格式的访问日志；上游连接池，fit 的九种负载均衡策略（轮询、加权轮询、最少连接、随机、p2c，以及按客户端 IP、路径、头或 cookie 的一致性哈希）和会话保持 cookie，跳过被被动健康检查（`max_fails`、`fail_timeout`）和主动健康检查（`health_check`：http 和 tcp 探测）下线的地址；回环地址上的 Admin API（`GET /config`，也可按 JSON 指针读取，`POST /load`，用 `PUT`、`PATCH` 和 `DELETE /config` 按指针编辑，`POST /rollback/{n}`，`POST /reload`，都带 `If-Match`，除回滚外都可 `?dry_run=1`，`GET /versions`；有 `token_file` 时每个请求都要带 bearer 令牌），修改与 SIGHUP 一样作为重载应用，两者共用一份历史记录；Admin API 的 `GET /metrics` 提供 fit 的 Prometheus 指标（有 `metrics` 时按监听器和路由的请求、上游和后端、证书、连接、重载、akio 的引擎）；为监听器在 `tls.acme` 中所列的名字通过 HTTP-01 或 TLS-ALPN-01 用 ACME 获取证书，保存在存储中，按有效期或 CA 的续期信息（ARI）续期，并装订其 OCSP 响应；兼容性门禁：80 个夹具，19 个有效，60 个只因暂不支持被拒绝，1 个跳过，没有失败 |
| 下一步 | akio 有了 A51–A53 和 A66 后做 I18b |
| akio 需求 | A1–A8、A11、A13、A16–A21、A36–A38、A63–A65 已在 akio 的 main 并已锁定；A9、A10、A12、A14、A15、A22–A24、A29、A30、A32、A33、A35、A45、A48、A49、A51–A54、A57、A58 和 A66 已提出（[akio API 需求](akio-api-requests.zh-CN.md)） |

## 日志

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
