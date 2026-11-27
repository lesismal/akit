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

5. **I153，同 fit 的 I198**：管理线程是角色自己的线程，它记下的客户端每个事件（完成、做种限制、校验结束、磁力链接的元数据）
   都会唤醒它，因此事件之后的动作（下一个排队的种子开始）无需等待；改变列表的每次 API 调用自己执行队列和磁盘检查。没有事件时
   它像 fit 的一样每秒查看一次，处理没有事件的内容：磁盘的空闲空间、`watch_dir`、`alt_speed` 的时间表。每个种子都以暂停状态添加，由调度启动。`min_free_bytes` 读取 akio 的
   `fs::space`（`available`，非 root 进程可用的空间），针对种子的目录或其最近的存在的上级目录；无法得知时不检查。
   .torrent URL 在监听器引擎上用 akio 的 HTTP 客户端获取，重定向由角色跟随，以便重定向到磁力链接时添加该链接。
   `watch_dir` 每次查看一个目录至多 100,000 个条目。分类的 `limits` 和 `media_library` 在 I154 和媒体库路线之前被拒绝。

6. **I154，同 fit 的 I199**：分类的限制和种子自己的限制通过 akio-bittorrent 的单个种子速率和做种限制实现，逐字段以种子
   自己的覆盖分类的；它们的 `max_conns` 和 `upload_slots`，以及客户端的 `access` 和连接限制（I154b），在 akio 的 A284 之前
   被拒绝。备用速率遵循 fit 的规则（跨过午夜的时间段持续到次日，`24:00` 以及作为结束的 `00:00` 表示当天结束时的午夜，API 的
   `on` 和 `off` 在 `auto` 之前覆盖时间表，保存在 `torrents.json` 的 `alt_speed` 中）；时区在角色应用配置时加载，加载不了的
   名字使该次应用被拒绝，如同日志的 `time_zone` 在打开日志时检查。

7. **I156，同 fit 的 I202**：发布的目录像 fit 那样每个 `check_interval` 遍历一次来跟随（目录监视会漏掉网络文件系统或其他主机做的
   修改）；文件在两次遍历一致后才被采用。其种子是混合种子，以做种模式通过只读存储做种，没有做种限制，并与客户端的列表分开。
   `data_dir/publish/<name>/` 下的索引为每个种子记录文件的大小、时间、inode 和设备，因此重启时未变化的内容不经哈希即做种。
   akio-bittorrent 的两个缺口已加入 A288：其 `Builder::build_path` 整体读取目录，因此 `whole_dir` 只发布没有被排除内容的
   目录（原因写在状态中，而不是建立一个包含 `exclude` 本想排除的内容的种子）；构建不能停止，因此重载或停止时让正在进行的构建运行到
   结束并丢弃（fit 的 I209 通过 context 停止构建）。

8. **I157，同 fit 的 I203 和 I210**：`/bittorrent/` 下的端点属于 akit-bittorrent，通过 akit-admin 的 `Options::endpoints`
   提供，使 akit-admin 不依赖这些角色；akit-admin 的 v1 测试把它们留给 akit-bittorrent 的测试，由后者对照 tests/compat/v1。
   客户端的调用（会写状态文件）在 akio 的逻辑池上运行；事件流不等待地写入 akio 的 `BodyStream`，落后 256 KiB 的流被丢弃，
   对应 fit 落后 256 个事件；其心跳是关于时间本身的定时器。指标是管理线程的查看保存的快照，速率按两次查看间隔一秒或更久计算，
   因为 akio-bittorrent 不提供速率。在 akio 的 A288 之前：种子的 tracker 和 web seed 按其 .torrent 文件和磁力链接列出，状态未知
   （这是 fit 对其客户端未运行的种子的应答），重新校验则不带 resume 数据重新添加种子，会校验每个分片但重新连接 peer。顺序下载和
   文件优先级以 A287 为名拒绝，种子的 `max_conns` 和 `upload_slots` 以 A284 为名拒绝。

## 后果

- fit 能运行而设置了 akio 尚不能提供的字段的配置，会按名称被拒绝，而不是缺少它照样运行。
- tracker 的上限变化时其 swarm 从空开始，与 fib 的 A85 之前 fit 的做法相同。
- `akit build -without akit-bittorrent` 构建时不含 `bt_tracker`。
