# 036：控制台、按 fit 发布时原样提供的 fit 页面，以及等待 akio 的 store

[English](036-console.md) | [简体中文](036-console.zh-CN.md)

状态：在 I90 中接受（2026-11-26），等待评审（[等待评审的决定](../decisions-pending.zh-CN.md)，第 16 行）；由 I91（页面）和
I92（带 TLS 的 Admin API）扩展。

## 背景

fit 的 I104（[fit 的决定 027](https://github.com/lesismal/fit/blob/main/docs/decisions/027-console-and-store.zh-CN.md)）在其
Admin API 之外给了 fit 两样东西：`admin.console`，需要登录的网页，从中读取和修改整个配置；以及 `admin.store`，以 fit 自己的
`fitstore` 格式密封保存配置的文件。fit 的控制台此后发展成了工作台（`app/workbench`）：纯 HTML、CSS 和 JavaScript，共 2.5 MB，
嵌入在 fit 的二进制中，调用控制台的 API（`/api/login`、`/api/session`、`/api/workbench`、`/api/m/<模块>/...`），其中配置本身的
模块就是在进程内调用的 Admin API 端点。akit 的 Admin API 即 fit 的（决定 006），运行在自己的 akio 引擎上。akit 和 fit 都采用
Apache License 2.0。

## 决定

1. **控制台运行在自己的引擎上**，一个 I/O 线程，HTTP/1.1，同 Admin API（决定 006），而不是像 fit 自 2026-11-02 起那样运行在
   监听器的引擎上：理由与 fit 给出的相同（监听器失败的配置仍能访问控制台），也使控制台不多的连接和它的池与监听器的分开限制。
   它在 Admin API 之前绑定，使它拿不到的地址在启动时就让 akit 停止。
2. **在同一个请求上于进程内调用 Admin API。** 发往 `/api/m/admin/<路径>` 的请求，其目标被改写为 `/<路径>`（akio-http 的
   `Context::req_mut`），交给 Admin API 的分发，跳过它的令牌和 `Host` 检查，由控制台的会话、CSRF 令牌和来源检查代替；修改像 API
   自己的修改一样交给控制线程，以 `console` 为来源应用。与 fit 的 recorder 不同，请求和回答都不复制；不论由谁写出回答，控制台的
   响应头和它对修改的日志（一个响应过滤器）都随回答一起。
3. **沿用 fit 的名字，使 fit 的页面和脚本原样可用**：cookie `fit_console_<端口>`、请求头 `X-Fit-Csrf`、端点及其 JSON、消息。
4. **登录即 fit 的**（`login` 包，连同其测试一起移植）：按地址和按用户的锁定、按 SHA-256 保存的会话、CSRF 令牌。密码在控制台
   自己的池上检查（`akio::Pool`，一个线程、16 个等待，再多 503），从不在 I/O 线程上。
5. **在 akio 的 A232 之前，连接像 Admin API 的那样受限。** fit 在没有登录的连接打开 5 秒后关闭它，并在超过 `max_conns` 时关闭
   最旧的这类连接以腾出位置，为已登录的连接保留四分之一名额；akio-http 不给处理函数其连接的句柄。在此之前：`max_conns` 在
   accept 时限制，akio 的请求头（10 秒）、请求体（30 秒）和空闲（2 分钟）超时，对没有有效会话的请求的回答会关闭其连接。
6. **只有有 store 时才能修改密码**，同 fit：没有 `admin.store` 时账户端点回答 409。store（I93）等待 akio-crypto 的 Argon2id
   （A230）和 XChaCha20-Poly1305（A231）：没有它们就无法读取 fit 的格式，而 akit 自己的格式会是 fit 打不开的 store。威胁模型的
   L5（密钥未加密存放）仍为接受：fit 的 store 保存配置和控制台用户，不保存证书存储，而 akit 与 fit 共用证书存储（I62），加密的
   store 会让 fit 读不了它。
7. **在 akio 能关闭回显之前（A233），`akit hash-password` 会显示在终端输入的密码**；提示中说明这一点以及如何隐藏。

## 影响

- 在控制台中修改的配置会运行，并以控制台为来源记入 `/versions`；重启时再从配置文件开始，同没有 store 的 fit。
- fit 的控制台用户及其哈希在 akit 中原样可用。
- 在 A232 之前，没有登录的客户端最多可占用 `max_conns` 个连接，每个最长为 akio 的请求头超时。
