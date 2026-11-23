# 待审阅的决定

[English](decisions-pending.md) | [简体中文](decisions-pending.zh-CN.md)

维护者不在时所做的决定，均已按推荐方案实现，等待确认或修改。确认的决定若属于决定记录，移入
[决定记录](decisions/README.zh-CN.md)，否则从这里划掉；被修改的则重做。

| # | 日期 | 位置 | 所做的决定 | 另一种选择 |
| --- | --- | --- | --- | --- |
| 1 | 2026-11-24 | akio，合并 akit 的需求 | 五个 akio 分支（A1/A3、A2、A4/A5、A7、A8）合并进 akio 的 main；akio 的 main 期间已用掉 ADR-093 和 ADR-094，所以 A2 的 ADR 改为 ADR-098（`098-tls-cert-resolver`），A1/A3 的改为 ADR-099（`099-runtime-listeners-accept-filter`），所有引用一并修改 | 保留分支的编号，改 main 的编号（main 的先推送） |
| 2 | 2026-11-24 | akio，合并 A8 | A8 和 main 的 ICMP 工作都声明了 macOS 的 `ioctl` 绑定：删去 A8 的那份。A8 的 `probe_gone` 在非 Linux 上允许 `needless_pass_by_ref_mut`（那里只读） | 按平台改函数签名 |
| 3 | 2026-11-24 | akit，akio API 需求 | I09 和 I14 都把自己的第一个需求编为 A9：I09 的（限制在 TLS 1.2 的 TLS 服务端）保留 A9，I14 的改为 A10–A12 | — |
