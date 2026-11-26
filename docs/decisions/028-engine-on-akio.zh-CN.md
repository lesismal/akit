# 028：把 fit 的 `engine` 映射到 akio 的引擎上

[English](028-engine-on-akio.md) | [简体中文](028-engine-on-akio.zh-CN.md)

状态：在 I79 中采纳（2026-11-26）。自 I79b 起 `limits.max_conns` 只统计客户端连接，并支持 `limits.max_pending_bytes`
（[决定 032](032-limits-on-akio-budget.zh-CN.md)）。

## 背景

fit 的 `engine` 设置 fib 的引擎：等待连接事件的 `event_loops`，运行连接读取和回调的 `workers`（以及 `min_workers`、
`task_queue`），`read_buffer_size`、`write_watermark`、`backlog`，事件循环如何等待（`loop_wait`），一次等待收集多少事件
（`poll_batch`），以及 `kernel_tls_offload`。fit-examples 的 `limits` 示例设置了其中大部分，而 akit 把整个部分按“尚不支持”拒绝，
因此该示例无法在 akit 上运行（[门禁](../compatibility.zh-CN.md#门禁)）。

akio 的引擎结构不同（[akio 的架构](https://github.com/lesismal/akio/blob/main/docs/architecture.zh-CN.md)）：每个监听器有自己的
I/O 线程，既等待连接的事件，*也*运行它们的回调；逻辑池只运行可能阻塞的工作（`spawn_blocking`：打开文件、插件、磁盘缓存、
bcrypt）。它有含义与 fib 相同的读缓冲、写水位和 backlog，但没有关于等待如何挂起、一次取多少事件的设置。

## 决定

akit 读取 `engine`，在启动时据此设置为监听器提供服务的 akio 引擎：

| fit | akio | 省略或为 0 时 |
| --- | --- | --- |
| `event_loops` | 每个 TCP 监听器的 I/O 线程数（`Listener::threads`） | akio 的默认：每核一个 |
| `workers` | 逻辑池的 `max_threads` | akio 的默认：每核 8 个，至少 8 个 |
| `min_workers` | 池的 `min_threads`（`max_threads` 至少与之相同） | 无 |
| `task_queue` | 池的 `queue_capacity` | akio 的默认：65,536 |
| `read_buffer_size` | `Config::read_buffer` | 64 KiB，fit 与 akio 相同 |
| `write_watermark` | `Config::write_high_watermark` | akio 的默认：256 KiB |
| `backlog` | `Config::backlog` | akio 的默认：1024 |

`loop_wait` 和 `poll_batch` 描述的是 fib 的事件循环，akio 没有对应物；`kernel_tls_offload` 要等 akio 的内核 TLS。这三个仍按名称
拒绝为“尚不支持”。

含义变化最大的是 `event_loops`：fib 的事件循环只负责等待并把工作交给 worker，所以 fit 的默认值很小（至多 4 个 CPU 时一个）；akio 的
I/O 线程承担实际工作，所以配置省略它时 akit 保持 akio 的默认值（每核一个），设置了它的配置则让每个监听器得到这么多线程。按 fit 的
小数目写的配置，得到的线程会比 akio 自己选择的少；参考文档（`engine.event_loops` 的说明）写明了这一点。

引擎在 akit 启动时设置一次：重载改变 `engine` 时，其余部分照常应用，并给出警告（“engine takes a restart; the running one
stays”），与 fit 的引擎只构建一次相同。重载新增的监听器使用 akit 启动时的 `event_loops`。

`limits.max_conns` 和 `limits.max_write_queue_bytes` 在启动时作用于同一个引擎，与 fit 相同：分别是 `Config::max_connections` 和
`Config::max_queued_bytes`。akio 把 akit 在该引擎上拨出的连接（到上游的连接）也计入 `max_connections`，但超出时只关闭新接受的
连接（akio API 请求 A215）。

## 影响

- fit-examples 的 `limits` 示例只因 `limits.max_pending_bytes` 被拒绝：这是跨所有连接的字节预算，akio 的引擎还没有
  （akio API 请求 A214）。
- akit 运行的其他引擎（Admin API、provider、ACME、OTLP 的）保持各自较小的设置，就像 fit 的控制台和 tunnel 保持自己的设置一样。
- 无需基准测试：省略 `engine` 时默认值不变。
