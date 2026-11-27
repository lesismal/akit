# 决定

[English](README.md) | [简体中文](README.zh-CN.md)

简短记录影响 akit 的决定：一项依赖、一种格式、之后的工作必须遵循的一个设计选择。每个决定一个文件，`NNN-title.md`，
中文版为 `NNN-title.zh-CN.md`，按顺序编号，从不重新编号。被取代的决定保留，在状态中写明由哪一个取代。

每份说明都有**状态**（提议中、在某个迭代中采纳、由 NNN 取代）、**背景**、**决定**和**影响**。fit 做过同样决定的，
说明中链接 fit 的那一份，并写明 akit 是否沿用。

| 编号 | 决定 | 状态 |
| --- | --- | --- |
| [001](001-dependency-policy.zh-CN.md) | 依赖：akio、标准库，以及少数其他 | 已采纳 |
| [002](002-config-compatibility.zh-CN.md) | akit 读取 fit 的配置 | 已采纳 |
| [003](003-module-registry-and-traits.zh-CN.md) | 模块注册表与数据面 trait | 已采纳 |
| [004](004-graph-publication-and-retirement.zh-CN.md) | 发布图、按请求读取、由控制面退役 | 已采纳 |
| [005](005-yaml-reader.zh-CN.md) | YAML 由 akit 自己的读取器读取 | 已采纳 |
| [006](006-admin-api.zh-CN.md) | Admin API 运行在自己的 akio 引擎上，由控制线程应用 | 已采纳 |
| [007](007-opentelemetry.zh-CN.md) | OpenTelemetry：fit 的 tracer，通过 akio 的 HTTP 客户端导出 | 已采纳 |
| [008](008-wasm-runtime.zh-CN.md) | WASM 插件运行在 akio-wasm 上，使用 fit 的 ABI | 已采纳 |
| [009](009-packaging.zh-CN.md) | 打包：用 rust-lld 链接的静态 musl 二进制，沿用 fit 的镜像、chart 和 unit | 已采纳 |
| [010](010-sockmap-relay.zh-CN.md) | eBPF sockmap 转发，由监听器的 `sockmap` 开启，属于 akit 扩展 | 已采纳 |
| [011](011-proxy-wasm.zh-CN.md) | Proxy-Wasm 子集，与 fit 相同，运行在 akio 的挂起响应头和响应体过滤器上 | 已采纳 |
| [028](028-engine-on-akio.zh-CN.md) | 把 fit 的 `engine` 映射到 akio 的引擎上 | 已采纳 |
| [029](029-v1-freeze.zh-CN.md) | v1 冻结：基线、标记的改动和两项检查 | 已采纳 |
| [030](030-supply-chain-checks.zh-CN.md) | 供应链检查：cargo-deny、每日的 cargo-audit，以及发布二进制的依赖图 | 已采纳 |
| [032](032-limits-on-akio-budget.zh-CN.md) | 启动时生效的限制落在 akio 引擎的预算上，以及只有 h2c 的监听器 | 已采纳 |
| [033](033-log-files.zh-CN.md) | 日志文件的切割与保留，与 fit 相同 | 已采纳 |
| [034](034-access-lists.zh-CN.md) | 基于 akio accept 过滤器的访问控制列表 | 已采纳 |
| [035](035-admin-api-redacts-secrets.zh-CN.md) | Admin API 遮蔽秘密，akit 对暴露的秘密发出警告，以及 I80b 的其余局限 | 待审阅 |
| [036](036-console.zh-CN.md) | 控制台运行在自己的引擎上，按 fit 发布时原样提供 fit 的页面，store 等待 akio | 待审阅；名字由 037 修订 |
| [037](037-akit-names.zh-CN.md) | akit 的名字：对外凡写 "fit" 的名字都改为 "akit"，不保留别名 | 已采纳 |
| [038](038-tunnel-and-llm.zh-CN.md) | 隧道与 LLM 功能：fit 的角色、akit 的名字、fit 的线上协议 | 待审阅 |
| [039](039-webrtc.zh-CN.md) | WebRTC：基于 akio-turn 和 akio-webrtc 的 fit 三个角色、akit 的名字、RFC 的线上协议 | 待审阅 |
