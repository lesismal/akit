# 039：WebRTC：基于 akio-turn 和 akio-webrtc 的 fit 三个角色、akit 的名字、RFC 的线上协议

[English](039-webrtc.md) | [简体中文](039-webrtc.zh-CN.md)

状态：在 I110 中采纳（2026-11-27），待审阅（[待审阅的决定](../decisions-pending.zh-CN.md)，第 50–54 行）；随 WebRTC
路线的迭代（I110–I114）落地而扩展。

## 背景

fit 的 I87 和 I108–I111 在 fit 的配置中加入了 WebRTC
（[fit 的决定 029](https://github.com/lesismal/fit/blob/main/docs/decisions/029-webrtc.md)）：`webrtc.turn`，基于 fib
`webrtc/turn/turnserver` 的 TURN 服务器；`webrtc.media`，其 PeerConnection 共享一个 UDP 端口的 SFU（fib 的
`webrtc.Mux`），处理器 `whip` 和 `whep` 向它发布、从它播放；`webrtc.proxy`，处理器 `webrtc_proxy` 的 UDP 端口，把其他
媒体服务器放在 fit 后面，并把每个 peer 中继到应答它的那台，按 peer 首个 STUN 请求的 ICE ufrag 找到。akio 有相同的层次：
`akio-turn` 的 `TurnServer`（UDP、TCP、TLS、DTLS）和 `akio-webrtc` 的 ICE lite `RtcServer`，所有 peer 共用一个端口。

处理器的 `auth` 引用 `media_auth` 中的一个策略，由流媒体与媒体鉴权路线读取。fit 的 TURN 角色依赖 fit 向 fib 请求的
钩子（其 A55–A59）：中继端口范围、按数据报计数的用户名带宽、服务器的事件、就地修改的限制、排空、服务器之间共享的
nonce 密钥、流的上限，以及对服务器自身地址的感知端口的检查。akio-turn 在 `8a9601b9`（以及 `a8171fda`）上都没有这些。

## 决定

1. **角色按 fit 的方式读取**（I110）：三个成员的字段、默认值、范围和检查与 fit 相同，位于 `akit-config` 的 `webrtc`
   模块，处理器的参数也在其中，门禁的三个示例有效。在运行它的迭代（I111–I114）之前，`akit run` 拒绝该成员或处理器，
   使配置从不声称多于 akit 能做的。
2. **处理器的 `auth` 作为尚不支持被拒绝**，直到读取 `media_auth`（流媒体与媒体鉴权路线）：否则 akit 找不到的策略会
   让所有人进入。
3. **角色运行在 akio 的服务器上**（`akio-turn`、`akio-webrtc`），位于自己的 crate `akit-webrtc`，如同 `akit-tunnel`
   运行隧道的角色：都是 akio 的 crate，决定 001 允许。
4. **它们对外显示的是 akit 的名字**（决定 037）：TURN 服务器的 `SOFTWARE` 为 `akit`；指标为 `akit_turn_*`、
   `akit_webrtc_media_*` 和 `akit_webrtc_proxy_*`；日志器为 fit 的 `webrtc`。线上协议是 RFC 的（STUN、TURN、ICE、
   DTLS-SRTP、WHIP、WHEP），不带任何网关的名字，因此 fit 的客户端和服务器、浏览器和 pion 都能同样访问 akit 的。
5. **akio-turn 缺少 fit 的钩子之处，向 akio 提出请求**（A256–A258），akit 运行遵守各条规则的、有记录的临时方案，
   或在运行时拒绝该设置：
   - `port_min` 和 `port_max`（A256）：带范围时角色拒绝启动，并记录日志；绑定在防火墙放行范围之外的中继会给客户端
     无人能到达的地址。
   - `user_bandwidth`（A257）：不强制执行；服务器启动时给出警告。allocation 及其 permission 和 channel 仍有上限。
   - 服务器自身的地址（`relay_ip`、`public_ip`）作为 peer（A257）：只能在 `allowed_peers` 随之指明的端口上访问，
     永远不能在服务器的中继端口上访问，因而 akit 的其他监听器不可达；同一 akit 服务器的两个客户端经由一方的中继地址
     和另一方的 server-reflexive 地址互相中继，而不是经由双方的中继地址。
   - 配额、上限和生存期（A258）变化时，与地址一样启动新服务器；用户、secret、peer 规则和 access 就地修改。
   - 排空（A258）：被重新加载替换的服务器在 `-shutdown-timeout` 内继续服务其持有的一切以及任何到达其地址的客户端，
     然后关闭其 allocation。
   - `nonce_secret`、`max_stream_message`（A256）：不生效，给出警告；akio-turn 的 nonce 使用每个服务器的随机密钥，
     其分帧接受最大 64 KiB 的消息。
   - `mptcp`：普通 TCP，如同 fit 在不支持多路径 TCP 的内核上；给出警告。

## 后果

- 带 TURN 服务器的 fit 配置在 akit 上运行，除非它设置了端口范围，上述差异会记录在日志中；I112 在 akio 满足
  A256–A258 后去掉它们。
- 带 `auth` 的 `whip`、`whep` 和 `webrtc_proxy` 等待流媒体与媒体鉴权路线。
