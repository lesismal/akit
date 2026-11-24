# 待审阅的决定

[English](decisions-pending.md) | [简体中文](decisions-pending.zh-CN.md)

维护者不在时所做的决定，均已按推荐方案实现，等待确认或修改。确认的决定若属于决定记录，移入
[决定记录](decisions/README.zh-CN.md)，否则从这里划掉；被修改的则重做。

| # | 日期 | 位置 | 所做的决定 | 另一种选择 |
| --- | --- | --- | --- | --- |
| 1 | 2026-11-24 | akio，合并 akit 的需求 | 五个 akio 分支（A1/A3、A2、A4/A5、A7、A8）合并进 akio 的 main；akio 的 main 期间已用掉 ADR-093 和 ADR-094，所以 A2 的 ADR 改为 ADR-098（`098-tls-cert-resolver`），A1/A3 的改为 ADR-099（`099-runtime-listeners-accept-filter`），所有引用一并修改 | 保留分支的编号，改 main 的编号（main 的先推送） |
| 2 | 2026-11-24 | akio，合并 A8 | A8 和 main 的 ICMP 工作都声明了 macOS 的 `ioctl` 绑定：删去 A8 的那份。A8 的 `probe_gone` 在非 Linux 上允许 `needless_pass_by_ref_mut`（那里只读） | 按平台改函数签名 |
| 3 | 2026-11-24 | akit，akio API 需求 | I09 和 I14 都把自己的第一个需求编为 A9：I09 的（限制在 TLS 1.2 的 TLS 服务端）保留 A9，I14 的改为 A10–A12 | — |
| 4 | 2026-11-24 | akit，合并 I22 与 I10 | 增加服务 h3 的监听器的重载被拒绝（"not supported yet"，保留正在运行的配置）：I22 为新监听器准备的临时办法只接管 TCP 连接，而 HTTP/3 监听器需要引擎中的 UDP 监听器（akio 的 A1）。合并时还修复了 `Listeners`，改由 I10 的 `addrs` 读取地址，因为 h3 监听器会在引擎中多加一个 UDP 监听器 | 先服务新监听器的 TCP 一侧，HTTP/3 等 A1 |
| 5 | 2026-11-24 | akit，I15 | I15 保持 `doing`：它的三个 `Host` 测试等待 akio 的 A11（写在 akio 分支 `claude/i15-proxy-699f18` 上）；下一组把它合并进 akio 并移动 akit 的锁定 | 在 akit 中改写 `Host`（规则 21：akio 欠缺的加进 akio） |
| 6 | 2026-11-24 | akit，I13 | akio-http 的 `Files` 不能尝试候选路径，也不能列出目录：I13 现在支持 `precompressed`，`try_files` 和 `browse` 按暂不支持拒绝（akio 需求 A16–A18），保持 `doing`；I13 会话在 akio 的一个分支上实现 A13 和 A16–A18，在移动 akit 的 akio 锁定时与 A11 一起合并，届时再完成 I13 的 akit 部分。结果：I13 在 akio `a9a620ab` 上完成（akit 分支 `claude/i13-on-pin`）；超出计划的是，A13 还让 handler 支持了 `strip_prefix` 并使用规范路径，因此 `/x/../ok.txt` 像 fit 的整套服务一样被提供（I12 时答 400） | 在 akit 中围绕 `Files` 实现 `try_files` 和目录列表（规则 21：akio 欠缺的加进 akio） |
| 7 | 2026-11-24 | akit，合并 I16 | I13 和 I16 都把自己的 akio 需求编为 A16–A18。I13 的保留 A16–A18，因为它的 akio 分支（ADR-100）已在使用；I16 的改为 A19–A21。I16 在 `doing` 状态下合并，因为它的 `limits.max_body_bytes` 已完成并测试；流式转发等待 A19–A21 | 改 I13 的编号，连同它的 akio 分支 |
| 8 | 2026-11-24 | akio，推送合并 | akio 的 main 在 gpu51 上 clippy 和 epoll 测试通过后、io_uring 测试之前就移到了 `9d216f79`（合并了 A1–A5、A7、A8）：HTTPArena 的基准测试占着主机的独占锁好几个小时，而每个合并的分支都在各自的会话中通过了 io_uring。io_uring 测试仍在运行，若失败则在 main 上修复 | 等 io_uring 测试通过后再推送 |
