# 038：隧道与 LLM 功能：fit 的角色、akit 的名字、fit 的线上协议

[English](038-tunnel-and-llm.md) | [简体中文](038-tunnel-and-llm.zh-CN.md)

状态：在 I98 中接受（2026-11-27），等待审阅（[等待审阅的决定](../decisions-pending.zh-CN.md)，第 17 行）；隧道与 LLM
路线的迭代（I98–I109）完成时陆续补充。

## 背景

fit 的 I105–I107 和 I159–I162 在 fit 的配置中加入了六个角色：隧道的 `tunnel.hub`、`tunnel.node` 和 `tunnel.proxy`，经由
hub 访问 NAT 之后的 TCP 服务；LLM 功能的 `llm.hub`、`llm.node` 和 `llm.proxy`，在多台机器之间共享 AI 模型
（[fit 的决定 028](https://github.com/lesismal/fit/blob/main/docs/decisions/028-llm-and-tunnel.zh-CN.md)）。它们的链路用监听方
签发的 token `fitt1.<id>.<secret>` 认证（[fit 的决定 035](https://github.com/lesismal/fit/blob/main/docs/decisions/035-link-tokens.zh-CN.md)），
运行 fit 自己的协议：隧道基于 TCP、TLS、WebSocket 和 QUIC 的帧与注册（ALPN `fit-tunnel`），以及 LLM mesh 基于 WebSocket
的帧、会话和流。这些协议在用户看不到的地方带有 fit 的名字：token 证明和密封正文所用的 HKDF 与 HMAC 标签
（`fit link token server key`、`fit-tunnel-link v4 token dialer to listener`、`fit tunnel register v1`、`fit llm mesh v1`、
`fit llm wire listener`），以及 ALPN。

[决定 037](037-akit-names.zh-CN.md) 让 akit 对外显示的每个名字都写 "akit"，不留别名，并保留属于 fit 格式而非名字的东西
（配置字段、`fitstore`、Proxy-Wasm 的 ABI）。akit 的 node 能否连接 fit 的 hub，取决于线上协议落在这条线的哪一边。

## 决定

1. **角色按 fit 的方式读取**（I98）：六个成员的字段、默认值、范围和检查与 fit 相同，位于 `akit-config` 的 `roles` 模块，
   门禁的六个示例有效。在运行某个角色的迭代（I100–I109）之前，`akit run` 拒绝设置了该角色的配置，使配置永远不会说得比
   akit 做得多。
2. **akit 显示的名字是 akit 的**（决定 037）：链路 token 为 `akitt1.<id>.<secret>`（不读取 `fitt1.`）；proxy 的响应头为
   `X-Akit-Node`；akit 写的文件为 `akit.json` 旁的 `akit.tunnel.hub.pem` 和 `akit.llm.<角色>.pem`；自签名证书的主题写
   "akit"；LLM 页面的 cookie 为 `akit_llm_<角色>`；GPU 脚本的变量为 `AKIT_LLM_GPU_DATA_DIR`。日志器沿用 fit 的 `tunnel`
   和 `llm`。LLM proxy 的 API key 保留其 `ah-` 前缀，它不指名任何网关。
3. **线上协议与 fit 逐字节相同**：帧、字段顺序和大小、方法名（`/mesh/hello`、`/hub/proxy` 等）、WebSocket 路径
   （`/tunnel`、`/ws`）、关闭码、ALPN `fit-tunnel`，以及所有 HKDF 和 HMAC 标签。它们是协议标识，从不显示，与 `fitstore`
   和 Proxy-Wasm 的 ABI 一样，按决定 037 第 2 点保留格式的做法保留。**akit 的 node 或 proxy 能连接 fit 的 hub，fit 的
   node 或 proxy 也能连接 akit 的 hub**；LLM mesh 的角色同样可以混用，每一方都像 fit 那样检查对方的证书和 token。
4. **凭据是 token 的 secret，而不是其前缀。** 监听方只保存由 secret 单独派生的密钥的 SHA-256，拨号方发送 token 的 ID 和
   证明，从不发送 token 本身。因此 fit 签发的 token 在 akit 的监听方保存其条目（`id`、`hash`，其余照 fit 写的）后即可
   连接；拿到 fit 签发的 token 的 akit node，只要把它写成 `akitt1.<id>.<secret>`（ID 和 secret 不变）即可连接 fit 的 hub。
   改写这个前缀就是 akit 的名字在线上协议边缘的全部代价，其他都没有差别。
5. **角色在 hello 中给出的版本是 akit 的**（`akit <版本>`），就像 fit 的给出 fit 的；对端如何使用它，由移植该部分的迭代
   写在这里。隧道的对端只报告它（hub 的对端列表、对端的状态），不与任何东西比较，与 fit 相同（I101）；mesh 的由 I104 决定。

## 其他选择

- **线上也用 akit 的名字**（`akit-tunnel`、`akit link token ...`）：为了没人看得到的标签，akit 的角色将无法与 fit 的角色
  互连，akit 的测试也无法用 fit 的测试向量检查其证明。不采用。
- **把 `fitt1.` 当作 `akitt1.` 的别名读取**：决定 037 拒绝别名；前缀是用户改写一次的配置值。

## 影响

- 混合部署可行：fit 的 hub 配 akit 的 node，akit 的 hub 配 fit 的 proxy，无论是迁移期间还是长期如此。互通性用 fit 自己的
  测试向量（token 的证明和密钥，I99）检查，帧的字节固定之处用 fit 的编码检查。
- fit 对其协议的修改，akit 会跟随，就像跟随配置格式的修改一样。
- 交给 fit node 的 `akitt1.` token 在那里要写成 `fitt1.`：fit 的检查拒绝其他前缀。
