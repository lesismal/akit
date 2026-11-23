# akit 基准测试框架

[English](README.md) | [简体中文](README.zh-CN.md)

akit 用 fit 的测试框架测量，而不是它的副本：相同的场景、配置、负载生成器和报告，只是把 akit 作为一个被测服务器
加在 nginx、Caddy、Traefik 和 fit 旁边（框架做什么见
[fit 的 bench/README.md](https://github.com/lesismal/fit/blob/main/bench/README.zh-CN.md)）。结果只来自基准
测试主机（[规则 23](../docs/rules.zh-CN.md#10-负载测试)）。

## akit 如何加入 fit 的框架

| 文件 | 是什么 |
| --- | --- |
| [`FIT_REV`](FIT_REV) | 所用框架的 fit 提交：与兼容性夹具锁定的提交相同 |
| [`fitbench/akit.go`](fitbench/akit.go) | 作为 fitbench 服务器的 akit：复制到 fit 的 `bench/cmd/fitbench` 中，把 akit 加入服务器列表，从镜像 `akit-bench:<tag>` 运行 |
| [`configs/akit/fit.json`](configs/akit/fit.json) | akit 在框架中的配置，复制到 fit 的 `bench/configs/akit`：fit 的 `bench/configs/fit/fit.conf` 提供的路由中 akit 已能提供的部分 |
| [`lib.sh`](lib.sh) | 下面两个脚本共用的部署代码 |
| [`run.sh`](run.sh) | 一次框架运行 |
| [`ab.sh`](ab.sh)、[`abreport.py`](abreport.py) | 两个 akit 构建在同一次运行中的对比 |
| [`results`](results) | 入库的运行，每个都带结果、报告、参数和 akit 提交；运行取回到 `results/run-<time>`（不入库），要保留时改名，如 `baseline-2026-11-24` |

fit 自己的文件不做改动。脚本把 `FIT_REV` 处的 fit 源码树（取自 akit 旁边的 fit 仓库，或 `FIT_REPO`）上传到主机
一次，每次把 akit 的两个文件加进去，在主机上用 Go 构建 fitbench、benchreport、idleconn 和 fit；把所要求的 ref 处
的 akit 上传并在主机上用 cargo 构建，放进基于 `ubuntu:24.04`（fit 所用的基础镜像）的独立镜像。构建以共享方式持有
主机的 `/root/bench.lock`；测量运行通过主机的 `quiet.sh` 独占持有，它还会在运行期间停下主机自身的服务。

## 运行

```bash
bench/run.sh main
```

启动一次运行：`main` 处的 akit（git ref，或代表工作区的 `work`），以及 nginx、Caddy、Traefik 和 fit，并返回其 id。
`bench/run.sh -report ID` 把它取回到 `bench/results/ID` 并打印 fit 的 benchreport 表格；`-wait ID` 先等它结束。选项
传给 fitbench：`-servers`（`nginx,caddy,traefik,fit,akit`）、`-scenarios`（`fixed,static,proxy`）、`-protocols`
（`http/1.1,h2c,https/1.1,h2`）、`-idle`（`http1,tls`），以及用于其他 fitbench 参数的 `-args`，例如
`-args '-duration 15s -warmup 5s'`。

默认值是 akit 目前能提供的：HTTP/1.1、h2c、HTTPS/1.1 和 h2（TLS 自 I09 起）上的 fixed 和 static 场景（static 自
I12 起，与 fit 一样从挂载在 `/srv/www` 的 fitbench 文件提供），以及空闲的 HTTP/1.1 和 TLS 连接。其他服务器运行它们能
提供的所有场景；akit 的其他场景在实现它们的迭代之前记为跳过（"akit cannot serve it"）：代理在 I15，HTTP/3 在 I10，
缓存在 I69。每个迭代把它新提供的内容加到 `fitbench/akit.go` 和 `configs/akit/fit.json`，并把
协议加入默认值。

## 对比两个构建

```bash
bench/ab.sh main work
```

在同一次运行中对比两个 akit 构建：只有 akit，HTTP/1.1 和 h2c 上的 fixed 场景，A 与 B 交替 3 轮，每次预热 2s 后测量
10s，每一轮都持有锁。它返回一个 id；`bench/ab.sh -report ID` 用 fit 的 abreport 汇总已完成的部分：每秒请求数、每个
请求的 CPU 和 p99，只有超过 1%（`AB_NOISE`）且超出各轮波动时才标记变化；`-wait ID` 等到结束。选项：`-scenarios`、
`-protocols`、`-rounds`、`-duration`、`-warmup`、`-args`。

## akit 的配置与 fit 的不同之处

| 设置 | fit | akit |
| --- | --- | --- |
| 8081 端口 | 只有 h2c | HTTP/1.1 和 h2c：akio 的服务端总是也接受 HTTP/1.1；没有运行往那里发 HTTP/1.1 |
| 监听 backlog | `engine.backlog`，主机的 `somaxconn` | akio 的默认值；akit 还没有 `engine` 设置 |
| 连接数 | `limits.max_conns 1000000` | akit 自己还没有限制 |
| 工作线程 | fib 的，每个 CPU 一个，各监听器共享 | akio 的 I/O 线程，每个监听器按容器绑定的 CPU 每个一个；一次运行只给一个监听器施压，由它的线程干活 |
| Admin API | 在 127.0.0.1:9005，用于统计 goroutine | 没有：akit 没有 goroutine，fitbench 也不为它统计 |
