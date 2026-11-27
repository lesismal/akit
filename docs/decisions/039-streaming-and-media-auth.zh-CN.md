# 039：流媒体与媒体认证：fit 的成员、akio-media 的媒体中心、fit 的 token

[English](039-streaming-and-media-auth.md) | [简体中文](039-streaming-and-media-auth.zh-CN.md)

状态：在 I120 中采纳（2026-11-27），等待评审（[等待评审的决定](../decisions-pending.zh-CN.md)第 55–59 行）；随流媒体与媒体认证
路线（I120–I126）各次迭代落地而扩充。

## 背景

fit 的 I115–I117 为其配置增加了两个成员（[fit 的决定 030](https://github.com/lesismal/fit/blob/main/docs/decisions/030-media-auth-and-pages.zh-CN.md)）：
`media_auth`，即规定谁可以推送和播放哪些流的具名策略（静态 token、用户、为 token 签名的密钥、公开的动作）；以及 `streaming`，
即基于 fib 的 `media/hub` 的媒体中心，带有 RTMP、RTSP 和 SRT 监听器以及通过 HTTP-FLV、HLS 和 WHEP 播放的处理器；还有媒体
页面。fit 后来在同样的成员上加入了属于其他路线的内容：密钥环和处理器 `media_tokens`（fit 的 I119）、签名的播放 URL、频道和推送
目标（媒体库）。akio 拥有 fib 所具有的媒体栈：akio-media 的 hub、RTMP、RTSP 和 SRT 服务器、HTTP-FLV 和 HLS、WHIP 和 WHEP。fit
的角色对 fib 的一些要求尚不在 akio-media 的 API 中。

## 决定

1. **这两个成员按 fit 的方式读取**（I120）：`media_auth` 和 `streaming` 的字段、默认值、范围和检查与 fit 相同，位于
   `akit-config` 的 `media` 模块。属于后续路线的内容（`keys`、`url_signing`、`channels`、`push`）以及 akio-media 尚做不到的
   内容（见下）按名字作为“尚不支持”拒绝，使配置所说的永远不多于 akit 所做的。在运行某成员的迭代之前，`akit run` 拒绝它。
2. **一个 crate，`akit-media`**，包含策略、角色和处理器，基于 akio-media（akio 的 crate，与其余部分同一提交：不是新种类的依赖）。
3. **akit 对外显示的名字是 akit 的**（决定 037）：token 可放在 cookie `akit_media_token` 中，命令为 `akit media-token`，401 的
   质询为 `Bearer realm="akit"`，指标为 `akit_streaming_*`；akit 读取 RTSP 的 `realm` 时其默认值为 `akit`。
4. **akit 签发的 token 与 fit 的逐字节相同**：`fm1.<声明>.<mac>`，声明为 fit 的 JSON（`sub`、`exp`、`pub`、`play`）的无填充
   base64url，MAC 为 `fm1.` 与声明在一个密钥下的 HMAC-SHA256，该密钥由策略的密钥和名字以 fit 的标签（`fit media token`）派生。
   与隧道的线上常量（决定 038）一样，前缀和标签是一种格式、从不显示，因此保持 fit 的：给定相同的密钥和策略名，fit 的
   `media-token` 签发的 token 为 akit 所接受，akit 签发的也为 fit 所接受。
5. **bcrypt 不在 I/O 线程上运行，与 fit 相同**：`password_hash` 用户的密码在 `akit-media` 自己的有界池上检查（CPU 数的四分之一、
   最多 256 个等待、5 秒，fit 的数字），验证正确后以策略随机密钥下的 HMAC 记住。HTTP 请求挂起（`Context::suspend`）等待检查；
   必须立即答复的监听器钩子以协议的重试方式拒绝并启动检查（I122）。
6. **按引擎和名字查找的策略**：策略在多次重载之间是同一个对象，由 `media_auth` 角色就地修改，`akit run` 在引用它的角色和处理器
   之前应用该角色；在角色应用新策略之前完成准备的处理器，按 fit 的 `Provision` 用正在编译的配置编译它。状态经由控制线程所交换
   内容的每线程缓存读取，因此检查不取锁、不触碰共享计数。
7. **akio-media 不足之处的临时做法**，各有一个 akio 请求（规则 21），在此之前拒绝它所需要的字段：RTMP 的 `max_message_bytes`
   （A263）、RTSP 的 Digest 和 Basic、`auth_methods`、`realm` 以及无会话连接的 `idle_timeout`（A264）。在 A264 之前，RTSP 从
   URL 的查询参数中取凭据（token，或用户和密码），在 hub 的钩子中检查。

## 其他选择

- **token 用 akit 的名字**（`am1.`、akit 的标签）：fit 和 akit 将不接受彼此的 token，只为了没人看得到的名字。拒绝，与隧道相同。
- **基于 akio 的 `RtspConfig::credentials` 实现 Digest 和 Basic**：它要求每个请求都认证，且不告诉钩子用户是谁，因此无法遵循策略
  的公开播放、token 和按用户的授权。在 akio 提供请求钩子（A264）之前拒绝。

## 后果

- fit 的 `media-token` 签发的 token 可用于 akit，`akit media-token` 签发的可用于 fit；cookie 不能在两者之间通用（决定 037）。
- 只会 Digest 或 Basic 的 RTSP 客户端在 A264 之前无法推送或播放受 `auth` 保护的流；ffmpeg 和大多数播放器可以使用 URL 中的
  `?token=` 或 `?user=&pass=`。
