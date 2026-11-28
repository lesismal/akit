# 041：集群管理：fit 的管理端、fit 与成员间的线上协议、akit 的名字

[English](041-cluster-management.md) | [简体中文](041-cluster-management.zh-CN.md)

状态：在路线计划中接受（2026-11-27），等待审阅（[等待审阅的决定](../decisions-pending.zh-CN.md)，第 70–74 行）；路线的
迭代（I130–I139c）完成时陆续补充。

## 背景

fit 的 I140–I150 让设置了 `admin.cluster` 的 fit 成为其他 fit（其成员）的管理端
（[fit 的决定 033](https://github.com/lesismal/fit/blob/main/docs/decisions/033-cluster-management.zh-CN.md)）。管理端把成员、
凭据、分组、模板、发布和审计日志保存在其存储的 `cluster` 分区，轮询每个成员的 `GET /node`、`/status` 和
`/config/check`，并通过成员自己的 API 修改成员：用户名和密码登录的控制台，或带 token 的 Admin API。无法直接访问的成员
主动外连到管理端（I149）。工作台的集群页面画出这一切；I91 已把这些页面随 fit 的其他页面复制进 akit（按
[决定 037](037-akit-names.zh-CN.md) 改名）。

有两件事决定 akit 如何跟随：保留 fit 管理端与成员之间协议的哪些部分，以及其中哪些名字属于 akit。

## 决定

1. **路线是 fit 的，分十二次迭代**（I130–I139c，开发计划的"集群管理"），按 fit 的顺序；fit 的 I153（跨集群的助手）随
   需要它的 AI 助手路线进行。管理端是一个独立的 crate `akit-cluster`，由 `admin.console` 作为控制台模块 `cluster` 提供。
2. **成员的回答是 fit 的协议**：路径（`/node`、`/status`、`/config/check`、`/schema`、`/api/login`、`/api/logout`、
   `/api/node`、`/api/m/admin/...`）、JSON 成员、能力名（`config.dry_run`、`status.requests` 等）、问题类型、存储的
   `node` 和 `cluster` 分区以及集群文档的结构。它们是协议标识，正如决定 038 第 3 点保留隧道的标识。以 fit 自己命名的成员
   按决定 037 处处改名，因为页面就是这样读取的：`GET /node` 回答 `akit_version`，而不是 `fit_version`。页面与之并列显示的
   `fib_version` 和 `go_version`（fit 的引擎和工具链）填 akit 的：`akio <提交>` 和空字符串（akit 的构建不记录编译器版本）。
3. **管理端以同样方式对待 fit 和 akit 成员。** akit 的管理端把会话的 CSRF 令牌同时放在 `X-Akit-Csrf` 和 fit 的
   `X-Fit-Csrf` 中，两边各读自己的；读到 fit 成员的 `fit_version` 时把版本记为 `fit <版本>`，让页面显示成员是哪一种。fit
   的管理端通过 akit 成员的 Admin API（`admin_api`，token）访问它：akit 的控制台只读 `X-Akit-Csrf`（决定 037：不留别名），
   所以登录到 akit 控制台的 fit 管理端能读不能写。
4. **存储可以共用**：`cluster` 文档是 fit 的，读取版本 1 到 3，没有角色时写版本 2，有角色时写版本 3，与 fit 相同；akit
   管理端尚未用到的部分（在其迭代之前的分组、模板、发布、链接、角色）按读到的样子保留并原样写回，使存储在 akit 与 fit 管理端
   之间移动时不丢失它们。更新版本的文档只读显示，与 fit 相同。
5. **akit 显示的名字是 akit 的**（决定 037）：配置旁保存节点 ID 的文件为 `akit.node-id`；注册 token 和凭据为 `akite1.` 和
   `akitc1.`（fit 的 `fite1.` 和 `fitc1.`）；管理端的 `User-Agent` 为 `akit-cluster-manager`。token 在 akit 与 fit 之间移动
   时改写其前缀，正如决定 038 第 4 点对链路 token 所说。

6. **I132 构建的管理端**：`akit run` 在监听器的引擎上启动它，没有存储时拒绝 `admin.cluster`；独立的轮询线程等待间隔并发起
   异步调用；存储在管理端自己的池上写入，一次一个。在 akio 的客户端给出响应的证书链和被拒绝的证书之前（A277），成员的证书由
   一次不做校验、不发送凭据的探测读取。测试记入审计，同 fit 的 I148 的决定。控制台中配置以外的请求体上限增至 128 KiB，即 fit
   该模块的上限。

7. **节点切换器（I134）**把 fit 的头以 akit 的名字返回（`Fit-Config-Version` 作为 `Akit-Config-Version`，`Fit-Dry-Run` 作为
   `Akit-Dry-Run`），并把存储密码同时放在 `Akit-Store-Password` 和 `Fit-Store-Password` 中，使 akit 的页面能读取 fit 成员；fit 成员
   的 schema 是 fit 的，所以其页面只读打开，正如 fit 的 I143 对管理端不认识的任何 schema 的处理。

8. **akit 成员上的模板（I136）**：akit 成员回答的配置中密钥已脱敏（决定 035），所以对 akit 成员（其 `GET /node` 回答
   `akit_version`），管理端在两边都脱敏后比较部分并保存其哈希，对 fit 成员则整体比较；akit 成员的 `GET /node/managed` 按其 API
   回答的配置计算哈希，因此两边一致。只在模板中修改密钥，不会让 akit 成员显示为未同步。

9. **发布（I137）** 采用 fit 的：计划、策略、门槛、操控，以及文档的 `rollouts`，两种管理端都能读取。有三点是 akit 自己的。
   发布的计划和执行各占一个自己的线程，在那里等待成员的回答，从不在 I/O 线程上（规则 1）。从 akit 成员复制时，其配置把密钥回答为
   `[redacted]`，复制会把它们掩码为模板的 `[secret]`，于是每个目标成员保留自己的密钥，fit 成员也不会被写入 `[redacted]` 这个词。
   漂移门槛只用于列出 `config.check` 的成员，与 fit 相同：fit 成员，以及从 I138 起的 akit 成员。

10. **漂移和告警（I138）** 采用 fit 的：一致性检查的差异和修复（`GET /config/check`、`POST /config/reconcile`）、规则、静默和 webhook，
    以及存储的 `cluster.alerts` 分区，两种管理端都能读取。akit 的检查从 `GET /status` 和 `GET /node` 的写出函数读取运行中的状态，
    需要重启才生效的设置即配置警告中列出的那些；其值回答时密钥已脱敏（决定 035）。webhook 的通知写 `akit-cluster` 和 “akit cluster”，
    签名在 `X-Akit-Signature` 和 `X-Akit-Timestamp` 中（决定 037：对外名字）。规则在一次轮询结束、有东西改变或最早的时间到来时评估，
    而不是 fit 每秒一次的定时（`tick_ms` 回答 0）；受管成员在其运行配置改变时与模板对照读取，不像 fit 那样每十分钟再读一次。

11. **角色（I139）** 采用 fit 的：viewer、operator 和 admin，集群范围或在分组上，文档的 `access`（按版本 3 写出），每个端点都像 fit 那样
    检查，拒绝时以 403 回答所需的角色。管理端自身的控制台经集群模块把关（`akit_roles::pages::Module::console_gate`，即 fit 的
    `ConsoleGate`）。fit 的角色对其助手的限制回答为空（`assistant`），因为 akit 的管理端没有助手；审计日志的条目不带所改内容的逐行差异。


12. **链接（I139b）** 与 fit 逐字节相同：`/cluster/link` 上经 TLS 的 WebSocket，管理端保存为文档 `link_cert` 并由成员固定的自签名证书，
    消息、标签 `fit cluster link v1` 和 fit 的证明（决定 038），以及以 fit 的前缀 `fitc1.` 在线上发送并保存的成员凭据。人使用的
    注册令牌在 akit 上是 `akite1.`，在 fit 上是 `fite1.`：由另一方管理端生成的令牌改写为成员自己的前缀，其 ID 和秘密不变。管理端每 15 秒
    对每条链接发送 ping，45 秒内没有收到消息就结束它（akio 的 WebSocket 处理器看不到 ping，只看到 pong），成员对管理端亦然；成员在
    从 1 秒倍增到 1 分钟并带抖动的退避之后重新拨号。成员用其 Admin API 经由到自己监听的一跳回答管理端的调用（决定待定行 93，直到 akio 的 A306）。
## 备选方案
13. **指标、日志和命令行（I139c）** 采用 fit 的：每个成员在 `GET /logs/recent` 上的最近 1,000 条消息的环形缓冲（能力 `logs.recent`，
    fit 的记录和编号），每个成员 360 次轮询的序列及其速率，证书清单，跟踪的游标（每个成员的最后编号，JSON 再用不带填充的 URL
    base64 编码），以及管理端 Admin API 上的 `/cluster/...` 端点，以集群范围管理员 `Admin API` 的身份行事，控制台不提供。
    `akit cluster` 用该 API 访问 akit 或 fit 管理端；它像 fit 一样每秒读取一次来跟踪发布、每两秒读取一次日志：协议没有它们的流。

- **线上也用 akit 的名字**（只用 `akit_version`、只用 `X-Akit-Csrf`）：一种管理端就无法管理另一种成员。管理端一侧拒绝；
  成员控制台一侧保留，因为决定 037 已改名其 CSRF 头。
- **akit 成员同时回答 fit 的 `fit_version`**：这是别名，决定 037 拒绝；fit 管理端对 akit 成员不显示版本。

## 后果

- akit 和 fit 成员可以共用任一种管理端：akit 管理端以控制台或 token 访问两者；fit 管理端以 token 访问 akit 成员。
- fit 管理端的节点切换器把 akit 成员视为另一种 schema（其摘要来自 akit 改名后的 schema）：在那里只读，正如 fit 的 I143 对
  任何它不认识的 schema 的处理。
