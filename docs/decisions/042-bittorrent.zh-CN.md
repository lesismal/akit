# 042：BitTorrent：基于 akio-bittorrent 的 fit 角色

[English](042-bittorrent.md) | [简体中文](042-bittorrent.zh-CN.md)

状态：于 I150–I151 采纳（2026-11-27），等待审查（[待审查的决定](../decisions-pending.zh-CN.md)，
第 75–79 行）；随 BitTorrent 路线（I150–I159）各迭代落地而扩展。

## 背景

fit 的 I196–I210 为 fit 的配置增加了两个角色（[fit 的决定
037](https://github.com/lesismal/fit/blob/main/docs/decisions/037-bittorrent.md)）：`bittorrent.client`，
下载并做种、种子保存在其 `data_dir` 中的客户端；`bittorrent.tracker`，基于 HTTP（`bt_tracker` 处理器）
和 UDP 的 tracker；以及 `bt_stream`、发布、Admin API 的 `/bittorrent/...`、页面和 RSS。fit 在向 fib 提出
所缺内容（fib 的 A75–A79、A85–A88）之后，基于 fib 的 `bittorrent` 包构建了它们。akio 有自己的
BitTorrent crate，akio-bittorrent：bencode、v1/v2/混合种子、磁力链接、HTTP 和 UDP tracker（客户端和服务器）、
带 MSE 的 peer 协议、uTP、DHT、LSD、web seed，以及带快速续传的客户端，与 libtorrent 和 aria2 线上兼容。

## 决定

1. **akit 的角色运行在 akio-bittorrent 上**，位于独立的 crate `akit-bittorrent` 中，由二进制注册：其角色
   （`TRACKER`，客户端从 I152 起）和处理器模块（`bt_tracker`；`bt_stream` 和 `bt_publish` 随其迭代），与
   akit 的其他模块 crate 一起列在 `STANDARD` 中。akio-bittorrent 属于 akio：与所有 akio crate 同一仓库、
   同一锁定版本的新依赖（规则 22），本说明即其决定说明（规则 20）。
2. **角色与 fit 相同**：配置的字段、默认值、上限和检查（I150），线上的应答与拒绝（fit 的 failure reason、
   `min interval`、默认 50 个 peer、UDP 应答不超过 1,400 字节、伪造的 UDP 连接 ID 被丢弃），控制台的路由，
   以及客户端在 `data_dir` 中的状态文件（`torrents.json`、`torrents/<id>.torrent`），使数据目录可在 fit 与
   akit 之间迁移（I152）。
3. **akio-bittorrent 缺少的内容向 akio 提出**（A283–A288），只在合理时才有临时做法：在 A283 之前客户端
   使用自己的引擎；在 A284 之前客户端的 `access`、`max_conns`、`max_half_open` 和 `disk_queue_bytes` 以
   "尚不支持"拒绝（第 76 行）；在 A285 之前 tracker 的 `max_peers` 被拒绝，存储上限的变化建立新存储
   （第 76、77 行）；在 akio 的应答与 fit 一致之前，tracker 的 HTTP 和 UDP 由 akit 自己的代码按 fib 的规则
   应答，akio-bittorrent 的 `TrackerServer` 作为存储（A285，第 78 行）。
4. **测试只在回环地址上**：两个 akio-bittorrent 客户端、fit 自己的二进制（`AKIT_FIT_BIN`）、已安装时的
   aria2c；DHT 和 LSD 关闭；从不访问公共 tracker 或 DHT 路由节点。

5. **I153，同 fit 的 I198**：管理线程是角色自己的线程，像 fit 的一样每秒查看一次，磁力链接的元数据到达时立即查看；
   改变列表的每次 API 调用也会执行队列和磁盘检查。每个种子都以暂停状态添加，由调度启动。`min_free_bytes` 读取 akio 的
   `fs::space`（`available`，非 root 进程可用的空间），针对种子的目录或其最近的存在的上级目录；无法得知时不检查。
   .torrent URL 在监听器引擎上用 akio 的 HTTP 客户端获取，重定向由角色跟随，以便重定向到磁力链接时添加该链接。
   `watch_dir` 每次查看一个目录至多 100,000 个条目。分类的 `limits` 和 `media_library` 在 I154 和媒体库路线之前被拒绝。

## 后果

- fit 能运行而设置了 akio 尚不能提供的字段的配置，会按名称被拒绝，而不是缺少它照样运行。
- tracker 的上限变化时其 swarm 从空开始，与 fib 的 A85 之前 fit 的做法相同。
- `akit build -without akit-bittorrent` 构建时不含 `bt_tracker`。
