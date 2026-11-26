# 037：akit 的名字：对外凡写 "fit" 的名字都改为 "akit"，不保留别名

[English](037-akit-names.md) | [简体中文](037-akit-names.zh-CN.md)

状态：已采纳（2026-11-26），由维护者决定；I97。它推翻了决定 002、006、007、008 和 036 保留 fit 名字的部分，并修订这些决定。

## 背景

akit 移植 fit（决定 002），此前凡是 akit 对外显示名字的地方都沿用 fit 的名字，使 fit 的工具原样可用于 akit：Admin API 的
`Fit-Config-Version` 和 `Fit-Dry-Run`，控制台的 cookie `fit_console_<端口>` 和请求头 `X-Fit-Csrf`（决定 036 第 3 条），
`Fit-Error` trailer，指标 `fit_*`，WebAssembly 插件 ABI（`fit_on_request` 和宿主模块 `fit`，决定 008），以网关命名的默认值
（`fit.` 下的 Docker 标签、`fit/` 下的 KV 键、粘性 cookie `fit_sticky`、realm `fit`、遥测服务名 `fit`、Kubernetes 控制器
`lesismal.github.io/fit`），默认配置文件 `fit.json`，以及写着 "fit" 的控制台页面（fit 的工作台）。于是 akit 的用户在 akit
自己的界面上看到 fit 的名字，而 akit 与 fit 并排运行时会拿走对方的标签、类和键。akit 还没有发布过，所以它的名字现在改动不会
破坏任何人的 akit。

## 决定

1. **对外凡写 "fit" 的名字都改为 "akit"**：请求头字段、cookie、指标、trace 属性、插件 ABI、以网关命名的默认值、CLI 的格式名、
   默认配置文件、akit 写出的文件名，以及控制台的页面。**不保留旧名字作为别名**：akit 不接受、不读取、也不回答 fit 的名字。
2. **不变的部分**：配置的字段名（没有一个写着 "fit"；akit 读取 fit 的配置格式，决定 002）、fit 的配置格式本身（`.conf`、
   YAML、JSON），以及 I93 要按 fit 的格式读写的 `fitstore` store 格式（决定 036 第 6 条）；Proxy-Wasm 的 ABI（`proxy_*`，自成
   一套标准，决定 011）；以及在文档、注释和消息中指 fit（即规范）的 "fit"（"与 fit 相同"、"fit 的 `X`"），还有 `akit adapt`
   的转换警告，它们是 fit 的，逐字节对照 fit 的 golden 文件检查。
3. **改名的内容**：

   | 位置 | fit 的 | akit 的 |
   | --- | --- | --- |
   | Admin API 的请求头字段 | `Fit-Config-Version`、`Fit-Dry-Run` | `Akit-Config-Version`、`Akit-Dry-Run` |
   | 中途断开的消息体的 trailer（代理、FastCGI） | `Fit-Error` | `Akit-Error` |
   | 控制台的 cookie 和 CSRF 请求头 | `fit_console_<端口>`、`X-Fit-Csrf` | `akit_console_<端口>`、`X-Akit-Csrf` |
   | 导出下载的文件名 | `fit-<成员>-v<n>.<格式>` | `akit-<成员>-v<n>.<格式>` |
   | Prometheus 指标（17 个指标族，`/metrics` 与 OTLP） | `fit_*` | `akit_*` |
   | OTLP：instrumentation scope、span 属性 | `fit`、`fit.listener`、`fit.route`、`fit.upstream.address` | `akit`、`akit.listener`、`akit.route`、`akit.upstream.address` |
   | `Cache-Status` 的缓存名 | `fit; hit`、`fit; fwd=...` | `akit; hit`、`akit; fwd=...` |
   | WASM 插件 ABI | 导出 `fit_on_request`，模块 `fit` | 导出 `akit_on_request`，模块 `akit`（fit 的 guest 包中的 `fit_sync*` 也改为 `akit_sync*`） |
   | 默认值（9 个字段） | `label_prefix` 和 `tag_prefix` 为 `fit`；Consul KV、etcd 和 `acme.shared_storage` 的 `prefix` 为 `fit/`；`controller_name` 为 `lesismal.github.io/fit`；`realm` 和 `telemetry.service_name` 为 `fit`；`sticky_cookie.name` 为 `fit_sticky` | `akit`、`akit/`、`lesismal.github.io/akit`、`akit`、`akit_sticky` |
   | Kubernetes Ingress 注解 | `fit.lesismal.github.io/*` | `akit.lesismal.github.io/*` |
   | `jwt` 的质询 | `Bearer realm="fit"` | `Bearer realm="akit"` |
   | `run`、`validate`、`adapt`、`fmt` 的默认配置文件 | `fit.json` | `akit.json` |
   | `adapt` 对 `.conf` 格式的命名（`-from`、`-to`） | `fit` | `akit` |
   | 控制台的页面 | fit 的工作台，写着 "fit" | 同样的文件，改名之后（见下） |

   akit 自己的日志、记录器名、`User-Agent`（`akit-health-check`）、ACME 存储（`~/.local/share/akit`）、环境变量和 Docker 镜像
   本来就是 akit 的。
4. **控制台的页面是经过确定性变换改名的 fit 工作台。** `tools/console-pages.py` 在 gate 固定的提交处复制 fit 的
   `app/workbench/web`，对 `vendor/` 以外的每个文本文件依次应用其 `RENAMES`：`FIT_*` 改为 `AKIT_*`，大写字母或 `-` 之前的 `Fit`
   （请求头字段、页面的全局对象）改为 `Akit`，令牌前缀 `fitt1.` 和 `fite1.`，"a fit" 改为 "an akit"，指网关的复数 "fits" 改为
   "akits"，其余不在更长单词中的 `fit` 一律改为 `akit`；`KEEP` 保留 `auto-fit`（CSS）、"as you see fit" 和 `fitstore`。同一变换把
   fit 的配置 schema 写为 `console/schema.json`，由控制台提供，使以英文描述为键的中文描述仍然对得上。被改动的每个文件开头都有
   一条说明，这是 Apache-2.0 第 4(b) 条的要求，`console/CHANGES` 列出这些改名；fit 的 `LICENSE` 原样保留。`--check`（在 `make
   compat-check` 中）检查每个文件都是改名变换的不动点、`schema.json` 是改名后的 fit schema，并在有 fit 克隆时检查每个文件都与
   改名后的 fit 文件逐字节相同；一项测试检查页面发送和读取的是服务器的名字。
5. **WebAssembly 测试插件按 akit 的 ABI 重新构建。** fit 的 guest 不再能在 akit 中加载（有测试说明这一点）。
   `tools/plugin-guest.sh` 在 gate 固定的提交处，用 fit 的 `plugin/wasm/guest` 和 `plugin/wasm/testdata/headers` 构建
   `crates/akit-plugin/tests/guests/headers-akit.wasm.gz`：只把 `//go:wasmimport` 和 `//go:wasmexport` 的名字改名（`fit` 改为
   `akit`，`fit_*` 改为 `akit_*`），其他不变，使用 Go 1.27.3（`GOOS=wasip1 GOARCH=wasm -buildmode=c-shared -trimpath`、
   `gzip -9 -n`）；`--check` 重新构建并比较。fit 自己的构建保留为 `headers-fit.wasm.gz`，用于拒绝它的测试和基准测试中的 fit 服务器。
6. **v1 基线在首次发布之前重写一次。** Admin API 的基线（`tests/compat/v1/admin-api.txt`）手工改名其中两个请求头字段，并有注释
   说明；配置的基线（`config.txt`）由 `python3 tools/v1-freeze.py --write-baseline` 重写，它现在写出的是当前表面减去冻结后标记的
   新增项，并从 `tools/config-reference-notes.json` 的 `defaults` 取 akit 的默认值（改变 9 项）；`tools/v1-freeze.py` 中的两个
   校验和是新文件的。`make v1-bless` 仍然拒绝破坏性改动：这是 akit 在 1.0 之前的最后一次。

## 影响

- **fit 的工具不能原样用于 akit。** fit 的 Grafana 仪表盘和告警规则读取 `fit_*` 指标；fit 的脚本发送 `Fit-Config-Version`、
  `Fit-Dry-Run` 和 `X-Fit-Csrf`；fit 的 WebAssembly 插件导出 `fit_on_request` 并从 `fit` 导入；fit 的容器带有 `fit.` 标签，
  它的 Consul 服务带有 `fit` tag，它的 KV 与 etcd 键和共享证书位于 `fit/` 下，它的 Ingress 带有 `fit.lesismal.github.io/`
  注解，它的 GatewayClass 指定 `lesismal.github.io/fit`。这些都要改，或者在配置中写明 fit 的名字：`label_prefix fit`、
  `tag_prefix fit`、`prefix fit/`、`controller_name lesismal.github.io/fit`、`realm fit`、`service_name fit`、
  `sticky_cookie { name fit_sticky }`。[迁移指南](../guides/migrating-from-fit.zh-CN.md)列出了要改的内容。
- akit 与 fit 并排运行时，默认不再拿走对方的标签、键、类或 cookie；通过 store 共享证书需要两边设置相同的
  `shared_storage.prefix`。
- 从 fit 换到 akit 时，客户端的粘性会话和登录不会保留：cookie 的名字不同。
- gate 仍然原样读取 fit 的配置文件；它对默认值检查的是 akit 的默认值。
- gate 固定的 fit 提交移动时，页面随之移动：重新运行 `tools/console-pages.py` 和 `tools/plugin-guest.sh`，并检查改名规则是否
  覆盖了 fit 新增的名字。
