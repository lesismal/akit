# 006：Admin API 运行在自己的 akio 引擎上，由控制线程应用

[English](006-admin-api.md) | [简体中文](006-admin-api.zh-CN.md)

状态：于 I26 采纳（2026-11-24）；其请求头字段由[决定 037](037-akit-names.zh-CN.md)改名（`Akit-Config-Version`、
`Akit-Dry-Run`：发送 fit 名字的 fit 客户端不能用于 akit）。

## 背景

I26 加入 fit 的 Admin API：`GET /config`（整份，或按 JSON 指针取一个值）、`POST /load`、`GET /versions` 和
`GET /versions/{n}`，带 ETag、`If-Match` 和 `If-None-Match`
（[fit 的决定 005](https://github.com/lesismal/fit/blob/main/docs/decisions/005-admin-api.zh-CN.md)）。
fit 用 net/http 在自己的 goroutine 上提供它，不占 fib 的 worker，因为应用配置要读文件、绑定监听器，这些会阻塞。
akit 没有 goroutine，也没有异步运行时（规则 1 和 16）：它要决定由什么提供 API、在哪里应用配置，以及两者如何
衔接而不在 I/O 线程上阻塞等待。

## 决定

- **API 由 akio-http 提供，运行在自己的 akio 引擎上，只有一个 I/O 线程**，由 `akit_admin::Server::start`
  在监听器的引擎之外构建并启动：数据面繁忙不会拖住 API，API 的限制就是其引擎的限制。只支持 HTTP/1.1（每个连接
  一次一个请求），`max_conns` 作为引擎的 `max_connections`（超出的连接在接受时即关闭），请求头 64 KiB，
  请求头 10 秒、请求体 60 秒无字节、空闲 60 秒超时，与 fit 一样。
- **读请求在 I/O 线程上回答**，数据来自历史记录 `akit_admin::Configs`，I/O 线程只在克隆某个版本的 `Arc` 时
  持有它（规则 1：不长时间持锁）。
- **加载的请求体在 `max_config_bytes` 之内流式读入**（超出回 413，依据 `Content-Length` 或边读边判断），然后分离
  （`Context::detach`），作为 `Job` 经有界队列交给控制线程（`try_send`；满了回 503）。控制线程就是 `akit run`
  已经在处理信号的主循环，用与 SIGHUP 相同的代码应用它：解析、编译，然后 `akit_listener::Listeners::reload`
  （I22），并在那里回答（`Detached::respond` 可从任意线程调用）。配置一次应用一个，`If-Match` 与被替换的配置
  比较，并发写入不会丢失。
- **每个配置都经过 `Configs::update`**，无论来自文件（SIGHUP，来源 `file`）还是 API（来源 `admin`）：统一编号，
  启动时为 1，每应用一个加 1；保留最近 `admin.history`（10）个。
- **配置按写入时的样子保存**：加载的请求体和单个 JSON 文件保存为标准 JSON（注释和尾随逗号替换为空格，其余字节
  原位不动，与 fit 的 `config.Standardize` 一样）；多个文件，或一个 YAML、`.conf` 文件，保存为按 fit 的写法写出的
  一个 JSON 文档（`akit_config::value::json_of`）。ETag 是该文本 SHA-256 的前 96 位（akio-crypto），与 fit 一样。
- **与 fit 一样拒绝网页**：带 `Origin` 的请求回 403，来自 API 自身源的读请求除外；`Host` 必须是回环主机。
  地址必须是回环地址（fit 在非回环地址上要求令牌和 TLS，见下文）；Unix 套接字要等 akio（A23）。
- **沿用 fit 的路径和载荷**，包括版本头（fit 的 `Fit-Config-Version`，自决定 037 起为 akit 的 `Akit-Config-Version`）；编辑、回滚、
  `POST /reload` 和试运行在 I27 之前回 501。

I27 更新（2026-11-24）：

- **每个修改都是控制线程执行的 `Job`**：`POST /load`，`PUT`、`PATCH` 和 `DELETE /config[/<pointer>]`，
  `POST /rollback/{n}` 和 `POST /reload`。请求体与加载一样流式读入，指针和 `?dry_run` 在 I/O 线程上检查；控制线程
  在 `Configs::change` 内、在为应用排序的锁下、比较过 `If-Match` 之后，从正在运行的配置做出修改，因此每个写入者的
  编辑都从前一个配置开始，ETag 过期的写入者得到 412，什么都不会被调用。编辑后的配置按 fit 的写法重新写出
  （`Value::to_json`）；整份配置（`POST /load`、`PUT /config`）按写入时的样子保存。回滚把第 `n` 版的配置作为新版本
  应用，来源 `rollback`；重载把正在运行的配置再应用一次，来源 `admin`，与 fit 一样。
- **试运行走应用的路径但不产生效果**：配置被解析，并用空的引擎槽（其连接池不拨号、不探测）和自己的注册表（其计数器
  不进入任何抓取）编译，`Listeners::plan` 列出 `Listeners::reload` 会打开和关闭的监听器：解析每个新监听器的地址，
  除非正在运行的监听器占着它，否则短暂绑定一下再放开。什么都不保存、不计数；回答带正在运行的版本和 ETag、
  该配置应用后会有的 ETag，以及 `Akit-Dry-Run: 1`（fit 的 `Fit-Dry-Run`，由决定 037 改名）。
- **令牌**（`admin.token_file`）在 API 启动时读取，去掉首尾空白，为空则拒绝；此后每个请求都要带
  `Authorization: Bearer <token>`，以常数时间比较，否则回 401 并带 `WWW-Authenticate`；这时与 fit 一样不再检查
  `Host`，`Origin` 仍然拒绝。非回环地址仍被拒绝：fit 在那里还要求 TLS（`cert_file`、`key_file`），而 akit 还不能
  通过 TLS 提供 API。
- 没有从 fit 搬过来的：它的连接限制器（`max_conns` 的四分之一留给出示过令牌的客户端，未认证的连接 5 秒后关闭；akit
  仍用引擎的 `max_connections`），以及 401 之后关闭连接，它与 413 一样要等 A24。

## 影响

- 配置有 `admin` 时，akit 多运行一个只有一个线程的 akio 引擎。
- 加载要排在 SIGHUP 重载之后，反之亦然：只有一个控制线程。
- akio-http 还不能在拒绝请求体的响应之后关闭连接（A24）：超过 `max_config_bytes` 的请求体剩余部分会被读取并丢弃，
  受请求体超时约束，而 fit 会关闭连接。
- 对 `admin` 的修改在重启后生效，修改它的加载会给出提示，与 fit 一样。
