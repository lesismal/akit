# 030：供应链检查：cargo-deny、每日的 cargo-audit，以及发布二进制的依赖图

[English](030-supply-chain-checks.md) | [简体中文](030-supply-chain-checks.zh-CN.md)

状态：在 I80 中采纳（2026-11-26）。

## 背景

I80 要求在外部安全审查之前 `cargo audit` 和 `cargo deny` 无告警。akit 自 I01 起就在 CI 中运行 cargo-deny（`deny.toml`：
advisories、bans、licenses、sources），但只在推送和拉取请求时运行，所以针对锁定 crate 发布的安全公告要等到下一次改动才会被发现。
cargo-audit 则从未运行。

两个工具都读取 RustSec，它收录的是 crates.io 上的 crate。akit 的发布二进制一个也不链接：`cargo tree -e normal,build -p akit`
只到达 akio 的各个 crate，来自 akio 仓库的固定提交（[决定 001](001-dependency-policy.zh-CN.md)，规则 22）。`Cargo.lock` 中来自
注册表的 crate（I80 时共 30 个）是开发依赖（`stats_alloc`）以及 akio 的 loom 模型（`loom` 及其依赖，位于 akio 的 `cfg(loom)`
之后，只有它的 loom 测试会设置）。因此 audit 无告警并不说明二进制运行的代码如何：那是 akio 的代码，akio 在自己的 CI 中运行
cargo-deny、cargo-audit、unsafe 清单、Miri、loom、sanitizer 和模糊测试（[akio 的安全政策](https://github.com/lesismal/akio/blob/main/SECURITY.zh-CN.md)）。
akit 的检查能守住的，是二进制保持这种状态。

fit 对应的是在其模块图上运行 `govulncheck`；fit 没有单独检查其二进制的依赖保持精简，因为 Go 的依赖随 `go.mod` 一起评审。

## 决定

- **单独的工作流 `.github/workflows/security.yml`**，在每次推送到 `main`、每个拉取请求、每天 UTC 04:17 以及手动触发时运行：
  `cargo deny check`（四项检查全部运行，与 CI 的 `deny` 任务相同）、`cargo audit -D warnings`（被撤回、无人维护和不健全的 crate
  也算失败），以及 `tools/release-deps.py`。每日运行用来在 akit 没有改动时发现新的安全公告。
- **不忽略任何安全公告。** 需要例外时，同时写入 `deny.toml` 的 `[advisories] ignore` 和 `.cargo/audit.toml`（在此之前不存在），
  各自写明原因以及该 crate 到 akit 的依赖路径，与 akio 的做法相同。
- **`tools/release-deps.py` 为二进制守住决定 001**：它从 `akit` 出发，沿 normal 和 build 边遍历 `cargo metadata` 解析出的依赖图，
  包括每个目标平台的 `cfg`（只有其他操作系统才链接的依赖也算），但不包括仅测试使用的 cfg（`cfg(loom)`），遇到 `ALLOWED` 列表未列出的
  注册表 crate 即失败。列入该列表的 crate 要写明采纳它的决定。I80 时列表为空：40 个包，全部属于 akit 或 akio。
- **`deny.toml` 的许可证列表保留 Apache-2.0**，尽管目前没有 crate 按它计算（每个双许可证的 crate 都按 MIT 计算），并设置
  `unused-allowed-license = "allow"`，使运行结果无告警，将来只用 Apache-2.0 的 crate 也无需修改列表。
- **akio 的安全公告通过固定提交到达 akit**：akio 发布自己的公告（GitHub Security Advisories 和 RustSec），akit 把 `rev` 移到修复后的
  提交（规则 22），并在自己的公告中说明（[SECURITY.zh-CN.md](../../SECURITY.zh-CN.md)）。

## 影响

- 针对开发依赖的安全公告会让每日运行失败，即便二进制不受影响；通过更新锁文件修复，或按上述方式写明原因后忽略。这是一道无条件的、
  干净的关卡的代价。
- 向二进制加入注册表 crate 的拉取请求会失败，直到该 crate 有了决定和 `ALLOWED` 条目：决定 001 要求的例外现在由检查把关，而不只靠评审。
- 两个工具都看不到 akio 代码中的漏洞；外部审查的范围（[审查范围](../security/review-scope.zh-CN.md)）说明它覆盖 akio 的哪些部分。
