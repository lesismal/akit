# 032：启动时生效的限制落在 akio 引擎的预算上，以及只有 h2c 的监听器

[English](032-limits-on-akio-budget.md) | [简体中文](032-limits-on-akio-budget.zh-CN.md)

状态：在 I79b 中采纳（2026-11-26）。

## 背景

[决定 028](028-engine-on-akio.zh-CN.md) 把 `limits.max_conns` 和 `limits.max_write_queue_bytes` 映射到 akio 的引擎上，
留下三件事给 akio：`limits.max_pending_bytes`，即所有客户端合计等待字节的预算（akio API 请求 A214）；只统计客户端连接的
`limits.max_conns`，而 akio 的 `max_connections` 把 akit 在同一引擎上拨出的上游连接也算在内（A215）；以及只有 `h2c` 的
监听器，akio 的 HTTP 服务器无法在不接受 HTTP/1.1 的情况下提供（A168）。fit-examples 的 `limits`、`proxy` 和 `grpc` 示例因此被
拒绝。akio 实现了这三项（其 ADR-172、ADR-173 和 ADR-174）。

## 决定

1. **`limits.max_pending_bytes`** 即 akio 的 `Config::max_pending_bytes`，在构建监听器的引擎时设置，与 fit 的 fib 在启动时
   读取一样；重载改变它时给出警告。akit 像 fit 一样读取它：字节数，或内存份额（`"25%"`，像 fit 一样按百分之一的百分之一
   取整保存），省略或为 0 时为内存的一半。
2. **份额所对应的内存**由 akit 读取，与 fit 的 `internal/sysmem` 相同：在 Linux 上读取 `/proc/meminfo` 的 `MemTotal`，若
   cgroup 的 `memory.max`（v2）或 `memory.limit_in_bytes`（v1）更低则取之，只在构建引擎时用 `std::fs` 读一次（绝不在 I/O
   线程上）。其他系统上 akit 不读取（macOS 的 `sysctl` 需要 `unsafe`，akit 没有，akio 的请求也未涵盖），份额按 fit 的回退值
   256 MiB 计。不增加依赖。
3. **`limits.max_conns`** 即 akio 的 `Config::max_accepted_connections`；统计所有连接的 akio `max_connections` 不再设置，
   与 I79 之前相同。
4. **只有 `h2c` 的监听器**把 akio-http 的 `HttpConfig::http1` 设为 `false`；在与 `tcp` 或 `socks` 共享的端口上，它的 HTTP
   规则是 akio-mux 的 `Rule::Http2`（只匹配 preface），与 fit 的 mux 匹配 `HTTP2()` 相同。发送其他内容的客户端收到 akio 的
   `GOAWAY` 并被关闭（fit 不发送任何内容直接关闭），或者在与 `tcp` 共享端口时交给监听器的服务，与 fit 相同。
5. **指标。** `akit_reads_paused_total{reason="budget"}` 即 akio 的 `budget_pauses`。

## 影响

- fit-examples 中 After-1.0 路线之外的配置都不再被拒绝（见门禁报告）。
- 预算会被 akio 两次汇总之间（约一毫秒）入队的数据略微超过；每个连接仍受 `max_write_queue_bytes` 约束。
- 预算也会暂停上游连接的读，正是这一点约束了代理的队列：fit 的 fib 同样暂停所有连接。
- 在 macOS 上默认预算是 256 MiB 而不是内存的一半：macOS 是开发平台，以字节数设置该字段的配置不受影响。
- 测量由基准任务负责（规则 30）：fixed 和 proxy 场景在默认预算与不设预算下的对比，以及大量慢客户端下与 fit 的对比。
