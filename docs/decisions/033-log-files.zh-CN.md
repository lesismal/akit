# 033：日志文件的切割与保留，与 fit 相同

[English](033-log-files.md) | [简体中文](033-log-files.zh-CN.md)

状态：在 I81 中采纳（2026-11-26）；第 7 至 10 点在 I82 中采纳（2026-11-26）。

## 背景

fit 自身的日志和访问日志都可以写文件；自 fit 的 I155–I156 起，文件按大小和时间切割，切割出的文件按时间、数量和总量删除，并用
gzip 压缩，周期按日志的时区计算（[fit 的决策 034](https://github.com/lesismal/fit/blob/main/docs/decisions/034-log-files.zh-CN.md)）。
akit 的访问日志（I21）在运行期间一直追加写同一个文件，并把 `rotate` 当作尚不支持而拒绝；akit 也完全没有 `log`。日志路线在
I81–I83 中移植 fit 的 I155–I157。

## 决定

1. **akit 自己的写入端，与 fit 的相同。** akit-observe 的 `Files` 为每个路径保留一个写入端，写该路径的所有日志共用（访问日志，
   以及 I82 起 akit 自己的日志器），对应 fit 的 `logs.Files`。写入只把缓冲放入队列就返回；超过 1 MiB（或 256 个缓冲）的队列时
   拒绝并计数，从不等待，所以 I/O 线程、请求和日志器都不会等磁盘（规则 1 和 11）。写入端自己的线程负责写盘、切割和清理；访问
   日志的写线程把每次刷出的缓冲原样交给它，不复制，被拒绝的缓冲按其中的行数计为丢弃。关闭写入端时写完队列中的内容，并等待其文件
   的压缩完成，最多 30 秒。
2. **切割、命名和保留与 fit 完全相同。** 在一次写入会使文件超过 `max_bytes` 之前切割（大于它的缓冲独占一个文件，所以访问日志的
   单位是一次刷出的批次，如 fit 的 I156 所述），也在 `interval` 的下一个周期里的第一次写入时切割；周期是从日志时区的午夜起整数个
   interval（整天数的周期按 1970-01-01 起的日数分组）。切割出的文件名为 `<path>.<yyyy-mm-ddThh-mm-ss.mmm>`，即切割时刻的 UTC
   时间，被占用时加 `-1`、`-2`，压缩后加 `.gz`。切割失败时保留原文件，一分钟内不再按大小或时间重试。打开时、每次切割后和每十分钟
   执行保留策略：早于 `max_age` 的，再是超过 `max_files` 的，再从最旧的删到不超过 `max_total_bytes`；只删除切割从该路径生成的
   文件名，绝不删除正在写的文件。`rotate` 的字段、默认值和检查与 fit 的 `config.Rotate` 相同。
3. **压缩在文件集自己的线程上进行**，在第一个文件需要压缩时启动，队列 16 个，最多等待一分钟（相当于 fit 只有一个 worker 的
   `internal/filepool`）：用 akio-deflate 压缩到以独占方式创建的临时文件，改名，再删除未压缩的文件；失败时保留未压缩的文件并说明
   原因。不增加依赖（决策 001）。
4. **不跟随链接，而不用 `O_NOFOLLOW`。** fit 用 `O_NOFOLLOW|O_NONBLOCK` 打开文件。akio-sys 没有 `O_NOFOLLOW`，akit 既不用
   `unsafe` 也不用 `libc`；所以先不跟随链接地查看文件（`symlink_metadata`），它必须是只有一个链接的普通文件，再以 `O_NONBLOCK`
   （akio-sys 提供）打开，使放在该名字处的 FIFO 不能阻塞打开，打开后它还必须是该名字原先指向的同一个文件（设备号和 inode）。
   其间被换成的链接虽被打开，但从不写或读，并被拒绝。新文件以及压缩的临时文件用 `O_EXCL`（`create_new`）创建，它不跟随链接。
   这与 `O_NOFOLLOW` 的效果相同，所以不向 akio 提需求。目录可被他人写入时给出警告，与 fit 相同。
5. **时区来自系统的 zoneinfo**，与 I21 为访问日志所做的决定相同：fit 内嵌 Go 的 `time/tzdata`；akit 读取
   `/usr/share/zoneinfo` 等目录（akit-observe 的 `Zone`），所以在容器中使用 IANA 名称需要 zoneinfo，未知的名称使配置失败。
6. **重载就地改变切割设置和时区**，与 fit 相同：写入端保留已打开的文件，在下一次写入时采用新设置；路径以及日志的格式需要重启。

7. **akit 自身的日志就是 fit 的 `log`**（I82）：按 fit 的名字划分日志器，各有级别（重载可改变）、格式和输出（或取日志的），
   格式为 fib 的 `line` 以及 log/slog 的 `text` 和 `json`，每行写出日志器的名字，时间按 `log.time_zone`。每个进程一个日志，由
   `akit run` 安装；在此之前以及在 `akit validate` 中，info 及以上的消息以 `akit: <消息>` 写到标准错误，正如 fit 的默认日志器
   写 `fit: <消息>`。消息由记录它的线程格式化并写出：文件经由写入端（从不等待），否则写标准输出或标准错误。
8. **akio 的记录属于 `engine` 日志器**，经由 akio 的 `log` 门面（`akio::log`，每个进程一个日志器）：警告和错误保持其级别；info
   和 debug 记录（即引擎的启动和停止）按 debug 写，正如 fib 只在要求时（`logStatus`）才写引擎行，所以 `akit run` 的标准输出仍只
   有它的命令输出行。akio 的最高级别跟随 `engine` 日志器的级别，低于它的记录从不格式化。
9. **akit 各 crate 的消息经由各自的日志器**：证书（`tls`：ACME、OCSP、共享存储）、provider（`provider`）、OTLP 导出
   （`telemetry`）、FastCGI 应用的标准错误（`fastcgi`）、访问日志丢弃的行（`access_log`）、写入端的失败和 sockmap 中继
   （`engine`）、指标的溢出和没有 `rotate` 的文件（`server`）。各 crate 的行回调不带级别，所以这些行按 warn 写，日志级别设为 warn
   时也不会丢失；为每条消息定级别，留待逐条移植 fit 对它们的划分。
10. **akit 命令本身的输出行保持原样**：`akit: listener …`、`akit: reloaded …`、`akit: admin API on …` 等照旧打印，正如 fit 的
    `cli` 用 `fmt` 打印自己的行；fit 经由 `admin` 和 `tls` 日志器写的少数几行（`admin API on`、`certificate files changed`）在
    akit 中保持原样，工具和测试会读取它们。

## 影响

- 磁盘慢时的代价至多是队列：每个文件 1 MiB，位于访问日志自身的缓冲之后。
- 两个日志共用的文件只有一个写入端和一种策略；I82 拒绝给同一路径两个 `rotate` 或两个时区的配置，与 fit 相同。
- `"rotate": {}` 即采用 fit 默认值的有界策略；没有 `rotate` 的文件输出会一直增长，`akit run` 启动时会提示。
- 要测量的内容写在开发计划的基准表中（I81 一行）：功能任务不跑基准（规则 30）。
