# 004：发布图、按请求读取、由控制面退役

[English](004-graph-publication-and-retirement.md) | [简体中文](004-graph-publication-and-retirement.zh-CN.md)

状态：在 I04 中采纳（2026-11-23）。

## 背景

fit 对图做引用计数，最后一个连接结束时释放图。在 akit 中，规则 13 禁止每个请求递增一个所有线程共享的引用计数；
而 akio 的 handler 在 I/O 线程上内联运行，控制面没有办法在空闲线程上运行任务。每个线程缓存当前图解决了前一个
问题；但如果释放跟着最后一个 `Arc` 走，空闲线程的缓存就会让旧图以及它持有的模块（上游连接池、文件、插件）
无限期地存活。

## 决定

- **`GraphCell`** 发布图：`swap` 在锁内替换当前图并递增一个 epoch。
- **读取经过每个线程的缓存**（`with`）：请求加载 epoch，只在 epoch 变化时刷新本线程的 `Arc`。每个线程有一个自己的
  槽位，请求在图上运行期间把它标为“进入”，并记下最后看到的 epoch；这是每个请求仅有的写，写在没有其他线程写的
  缓存行上。
- **图由控制面退役**（`collect`），而不是由最后一个 `Arc`：当每个线程的槽位都在它之外或已越过它、且没有 `Hold`
  保留它时，被替换的图退役，其模块只清理一次。槽位的标记与 epoch 在两侧都是顺序一致的先存后读，所以请求永远
  不会在 `collect` 已退役的图上开始。图的内存在最后一个缓存丢弃其 `Arc` 时释放；它的资源在退役时释放。
- **`Hold`** 让图在 handler 调用之后继续存活：之后才应答的东西（detach 的响应、被代理的 body、WebSocket、隧道）
  在 `with` 内取一次，用完丢弃。所属 cell 已不存在的图，由最后一个 `Hold` 退役。
- **handler 参数是 akit-config 的类型化值**（`HandlerParams = akit_config::Handler`）：akit-config 在读取配置时带位置
  解码并检查参数；模块的构造函数接收类型化的值，只报告 provision 时发现的问题（`ProvisionError::field`），放在
  `routes[i].handler.params.<field>`。

## 影响

- 控制面在每次替换后调用 `collect`，并定时调用直到 `retiring()` 为 0：这个定时器归 I22（SIGHUP）和 I28（Admin API
  替换）。
- 每个工作会超出 handler 调用的模块（I15–I17 的代理、I49 的 WebSocket、I51 的隧道）都要取 `Hold`；不取的模块可能在
  其模块被清理后仍在运行。
- 清理在控制线程上运行（或随最后一个 `Hold`，可能在 I/O 线程上，决定 003 允许），且从不阻塞。
- 每个请求：一次 epoch 加载、对本线程槽位的几次写，没有分配、没有锁、没有共享的读-改-写；
  在 `crates/akit-runtime/benches/graph_read.rs` 中测量。
