# 安全政策

[English](SECURITY.md) | [简体中文](SECURITY.zh-CN.md)

akit 是反向代理、TLS 终结器和 API 网关：它面向互联网，持有私钥，转发凭据。本页说明如何报告安全问题、哪些版本会得到修复，以及 akit
如何约束自己的代码和依赖。akit 防御什么、怎样防御，见[威胁模型](docs/security/threat-model.zh-CN.md)；1.0 的外部审查覆盖什么，见
[审查范围](docs/security/review-scope.zh-CN.md)。

## 支持的版本

akit 处于 1.0 之前（`0.0.x`）。在 1.0.0 发布之前，只有 `main` 上的最新提交会得到修复；没有需要回移修复的发布版本。

从 1.0.0 起：

| 版本 | 修复 |
| --- | --- |
| 当前主版本的最新次版本 | 所有安全修复，以补丁版本发布 |
| 它之前的一个次版本 | 高危和严重级别的安全修复，在较新的次版本发布后的 3 个月内 |
| 更早的版本 | 不修复：请升级（1.x 内的升级保持配置和 Admin API 客户端可用，见[兼容性](docs/compatibility.zh-CN.md#v1哪些保持稳定)） |

为堵住漏洞而必须收紧默认限制或改变默认值的修复，可以在 1.x 版本中进行；发布说明会写明（见[1.x 版本可以做什么](docs/compatibility.zh-CN.md#1x-版本可以做什么)）。

## 报告漏洞

**请不要为安全问题创建公开的 issue、拉取请求或讨论。**

请通过 GitHub 的[私密漏洞报告](https://github.com/lesismal/akit/security/advisories/new)私下报告（“Security”标签页 →
“Report a vulnerability”）。请包括：

- akit 的版本或提交、操作系统和内核，以及 I/O 后端（epoll、io_uring、kqueue）；
- 能复现问题的最小配置（把密钥替换掉），以及触发它的请求、连接或文件：一条 `curl` 命令、一个脚本、一份抓包或一个测试用例；
- 攻击者能得到什么：内存或数据泄露、请求被走私或绕过检查被路由、认证被绕过、限制被绕过（在配置的限制之内造成拒绝服务）、I/O 线程
  panic 或挂起、在插件沙箱之内或之外执行代码；
- 是否希望署名致谢，以及署什么名字。

akio 本身的问题（HTTP/1、HTTP/2、HTTP/3 或 QUIC 的解析、TLS、X.509、密码算法、WebAssembly 解释器、I/O 后端）属于 akio：请按
[akio 的政策](https://github.com/lesismal/akio/blob/main/SECURITY.zh-CN.md)报告。如果不确定属于哪个项目，报告给 akit，我们会转交。

## 处理流程

| 步骤 | 目标 |
| --- | --- |
| 确认收到报告 | 3 个工作日内 |
| 初步评估（接受、需要更多信息、不是漏洞） | 7 天内 |
| 已接受报告的修复或缓解措施 | 高危和严重级别 30 天内，其他尽力而为 |
| 公开安全公告 | 修复发布时，与报告者协调 |

已接受的问题以 GitHub Security Advisories 发布。akit 遵循协调披露：请在公开细节之前给我们合理的时间发布修复（默认 90 天，如果漏洞
已被利用则更短）。akio 中的修复在 akit 移动其固定的 akio 提交时到达 akit（[规则 22](docs/rules.zh-CN.md)）；安全公告会同时写明两者。

## 范围

范围之内：akit 的代码（`crates/*`）、它构建的发布二进制和容器镜像（`scripts/dist.sh`、`Dockerfile`），以及它们各自的默认配置，在
支持的平台上的行为。特别是：

- 来自客户端的请求、连接、数据报或 TLS 握手使 akit 崩溃、挂起或阻塞 I/O 线程，或越过配置设定的限制或默认限制；
- 请求的路由、转发或应答绕过了路由的中间件（认证、访问检查、限制），或上游对请求的理解与 akit 不同（请求走私、头注入）；
- Admin API 或仪表盘在不满足其配置要求（回环地址、令牌）的情况下可被访问或修改配置；
- 私钥、ACME 账户密钥、令牌或密码被泄露：出现在应答、日志、指标、`GET /config`、错误消息中，或写入权限过宽的文件；
- WebAssembly 插件越出其内存、时间预算或被授予的宿主函数；
- 提供者（Docker、Kubernetes、Consul、etcd）或共享证书存储的应答使 akit 崩溃或越出其边界。

范围之外：

- 需要超过配置限制的资源才能造成的拒绝服务，或者配置主动放弃了某项限制；
- 已经能写入 akit 的配置、文件或环境的操作者（配置是受信任的：它可以代理到任何地方、读取进程能读取的文件、加载插件）；
- `bench/` 和 `tools/` 下的基准测试工具、`fuzz/` 下的模糊测试目标、`tests/` 中的测试服务器，以及 `deploy/` 下的部署示例（其默认安装
  内容除外）；
- akio 中的漏洞（按上文报告给 akio），或从不进入发布二进制的开发依赖中的漏洞；
- [威胁模型](docs/security/threat-model.zh-CN.md#已知的局限)已列为已知局限并给出计划的加固缺失。

## 已有的加固

- **akit 中没有 `unsafe`**：每个 crate 都是 `#![forbid(unsafe_code)]`（[规则 14](docs/rules.zh-CN.md)）。`unsafe` 只存在于 akio 中，
  经过其 [unsafe 审查](https://github.com/lesismal/akio/blob/main/docs/unsafe-review.zh-CN.md)。
- **发布二进制只有一个依赖**：akio，按提交哈希固定（[规则 22](docs/rules.zh-CN.md)）；没有任何来自 crates.io 的 crate 链接进来
  （[决定 001](docs/decisions/001-dependency-policy.zh-CN.md)、[决定 030](docs/decisions/030-supply-chain-checks.zh-CN.md)）。akio 的
  TLS、X.509 和密码算法都是它自己的实现，并有常数时间检查（[akio 的常数时间页面](https://github.com/lesismal/akio/blob/main/docs/constant-time.zh-CN.md)）。
- **`cargo deny check` 和 `cargo audit -D warnings`** 在每次推送和拉取请求时以及每天在 CI 中运行
  （[`.github/workflows/security.yml`](.github/workflows/security.yml)）：RustSec 安全公告、被撤回和无人维护的 crate、许可证、来源
  （只允许 crates.io 和 akio 的仓库）以及禁用的 crate（不用异步运行时）。
- **对端或配置的输入不会引发 panic**（[规则 15](docs/rules.zh-CN.md)），由模糊测试搜寻：akit 拥有的每个解析器都有一个目标，其语料由
  `cargo test` 回放。
- **每项资源都有带安全默认值的上限**（[规则 11](docs/rules.zh-CN.md)），每次与外部的交互都有超时（[规则 10](docs/rules.zh-CN.md)），
  没有任何东西阻塞 I/O 线程（[规则 1](docs/rules.zh-CN.md)），每次测试运行都开启 akio 的慢回调检测器。
