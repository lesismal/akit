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
| [`fitbench/akit.go`](fitbench/akit.go) | 作为 fitbench 服务器的 akit：复制到 fit 的 `bench/cmd/fitbench` 中，把 akit 加入服务器列表，从镜像 `akit-bench:<tag>` 运行；以及 `wasm` 场景和提供它的服务器 `akit-wasm`、`fit-wasm` |
| [`configs/akit/akit.json`](configs/akit/akit.json) | akit 在框架中的配置，复制到 fit 的 `bench/configs/akit`：fit 的 `bench/configs/fit/fit.conf` 提供的路由中 akit 已能提供的部分 |
| [`configs/akit/wasm.json`](configs/akit/wasm.json)、[`configs/akit/fit-wasm.conf`](configs/akit/fit-wasm.conf) | `wasm` 场景的路由，分别给 akit 和 fit，都是服务器自己的配置之外的第二个配置文件 |
| [`stampede.sh`](stampede.sh)、[`stampede`](stampede) | 击穿检查：许多客户端、一个冷键、一个计数自己被取了几次的慢源站，测 nginx、fit 和 akit |
| [`lib.sh`](lib.sh) | 各脚本共用的部署代码 |
| [`run.sh`](run.sh) | 一次框架运行 |
| [`ab.sh`](ab.sh)、[`abreport.py`](abreport.py) | 两个 akit 构建在同一次运行中的对比 |
| [`modules.sh`](modules.sh)、[`modulesreport.py`](modulesreport.py)、[`configs/modules`](configs/modules) | 每个中间件与 fit 的对比，akit 在 epoll 和 io_uring 上（I56） |
| [`results`](results) | 入库的运行，每个都带结果、报告、参数和 akit 提交；运行取回到 `results/run-<time>`（不入库），要保留时改名，如 `baseline-2026-11-24` |

fit 自己的文件不做改动。脚本把 `FIT_REV` 处的 fit 源码树（取自 akit 旁边的 fit 仓库，或 `FIT_REPO`；在 akit 的
git worktree 里要设置它）上传到主机一次，每次把 akit 的文件加进去（连同 `wasm` 场景的插件，即 fit 的 `headers`，按各服务器的 ABI：akit 的从
`crates/akit-plugin/tests/guests/headers-akit.wasm.gz` 解压，fit 的从 `headers-fit.wasm.gz` 解压，[决定 037](../docs/decisions/037-akit-names.zh-CN.md)），在主机上用 Go 构建 fitbench、benchreport、idleconn 和 fit；把所要求的 ref 处
的 akit 上传并在主机上用 cargo 构建，放进基于 `ubuntu:24.04`（fit 所用的基础镜像）的独立镜像。构建以共享方式持有
主机的 `/root/bench.lock`；测量运行通过主机的 `quiet.sh` 独占持有，它还会在运行期间停下主机自身的服务。

## 运行

```bash
bench/run.sh main
```

启动一次运行：`main` 处的 akit（git ref，或代表工作区的 `work`），以及 nginx、Caddy、Traefik 和 fit，并返回其 id。
`bench/run.sh -report ID` 把它取回到 `bench/results/ID` 并打印 fit 的 benchreport 表格；`-wait ID` 先等它结束。选项
传给 fitbench：`-servers`（`nginx,caddy,traefik,fit,akit`）、`-scenarios`（`fixed,static,proxy`，以及
`cache-hot`、`cache-zipf`、`wasm`）、`-protocols`（`http/1.1,h2c,https/1.1,h2`）、`-idle`（`http1,tls,ws`），以及用于其他
fitbench 参数的 `-args`，例如 `-args '-duration 15s -warmup 5s'`。`BENCH_DIR`（默认 `/root/akit-bench`）是脚本在主机上
部署和运行的目录；一个会话改动框架而另一个会话正在用它时，要设一个自己的。

默认值是 akit 目前能提供的：HTTP/1.1、h2c、HTTPS/1.1 和 h2（TLS 自 I09 起）上的 fixed、static 和 proxy 场景（static
自 I12 起，与 fit 一样从挂载在 `/srv/www` 的 fitbench 文件提供；proxy 自 I15 起，转发到 fitbench 的后端），以及空闲的
HTTP/1.1、TLS 和 WebSocket 连接（WebSocket 自 I25 起，与 fit 的路由 `ws` 一样代理到 fitbench 的 WebSocket 后端）。akit 也提供缓存场景（自 I76 起）`cache-hot` 和 `cache-zipf`，与 fit 和 nginx 一样：路由
`cache`，1 GiB 的区域，开启缓存锁，位于 fitbench 的缓存源站之前，源站被取的次数给出每次运行的命中率；与 fit 的框架一样，
只有 `-scenarios` 点名时才运行它们。其他服务器运行它们能提供的所有场景；akit 的其他场景（HTTP/3、网站类负载）记为跳过
（"akit cannot serve it"）。每个迭代把它新提供的内容加到 `fitbench/akit.go` 和 `configs/akit/akit.json`，并把协议加入默认值。
测 10 万空闲连接的内存（与 fit 的 M1 报告一样）：`-scenarios fixed -protocols http/1.1 -idle http1,tls
-args '-idle-connections 100000'`。

## WASM 插件的开销

```bash
bench/run.sh -servers akit-wasm,fit-wasm -scenarios fixed,wasm -protocols http/1.1,h2c -idle "" work
```

测量一个修改头部的 WebAssembly 插件给每个请求增加的开销，在 akit 和 fit 中，用同一个插件：fit 自己的测试插件
（`plugin/wasm/testdata/headers`，由 Go 为 `wasip1` 构建，按各服务器 ABI 的名字：决定 037），它设置和删除请求字段，并添加三个响应字段。`wasm` 场景是放在
`wasm` 中间件之后的固定响应，用中间件的默认值（每个 CPU 一个实例，队列 1024，每次调用 100 ms）；在同一个容器中测量的
`fixed` 是基线。服务器 `akit-wasm` 和 `fit-wasm` 是带框架配置的 akit 和 fit，各自另加一个添加路由 `/wasm` 的配置文件
（[`configs/akit/wasm.json`](configs/akit/wasm.json)、[`configs/akit/fit-wasm.conf`](configs/akit/fit-wasm.conf)），这样
fit 自己的配置保持原样，没有点名它们的运行也不加载插件。

## 击穿检查

```bash
bench/stampede.sh work
```

让许多客户端同时向缓存请求一个冷键（默认 1,000 个：HTTP/1.1 上每个一条连接，h2c 上 10 条连接每条 100 个流），而源站
（[`stampede/origin.py`](stampede/origin.py)）要一秒才回答；源站统计到达它的获取次数。它依次运行开启 `proxy_cache_lock`
的 nginx、fit、akit，以及开启 `disable_lock` 的 akit（对照组，每个客户端都会到达源站），每个单独占用服务器的 CPU，每个
服务器和协议用三个冷键，每个键打印一行：服务器、协议、回答 200 的请求数和源站的计数。它等到结束（约两分钟，独占主机
的锁），把这些行保存在 `results/stampede-<time>`。选项：`-clients`、`-keys`、`-delay`。

## 对比两个构建

```bash
bench/ab.sh main work
```

在同一次运行中对比两个 akit 构建：只有 akit，HTTP/1.1 和 h2c 上的 fixed 场景，A 与 B 交替 3 轮，每次预热 2s 后测量
10s，每一轮都持有锁。它返回一个 id；`bench/ab.sh -report ID` 用 fit 的 abreport 汇总已完成的部分：每秒请求数、每个
请求的 CPU 和 p99，只有超过 1%（`AB_NOISE`）且超出各轮波动时才标记变化；`-wait ID` 等到结束。选项：`-scenarios`、
`-protocols`、`-rounds`、`-duration`、`-warmup`、`-args`，以及 `-env-a` 和 `-env-b`：为一侧的 akit 设置一个环境变量
（`VAR=VALUE`）。A 和 B 是同一个 ref 时只构建一次 akit，两侧运行同一个二进制，因此
`bench/ab.sh -env-a AKIO_DEFAULT_SCHEDULING=event-loops work work` 只比较这一设置。

## 每个中间件与 fit 的对比

```bash
bench/modules.sh main
```

提供 [`configs/modules/modules.json`](configs/modules/modules.json)：akit 和 fit 运行同一份配置，每个中间件各在
一条路由上（主机名 `<module>.bench`），放在到后端（nginx，[`configs/modules/backend.conf`](configs/modules/backend.conf)，
1 KiB JSON）的代理前面，旁边是只有代理的路由（`none`）和固定应答（`respond`）。它用 wrk（7 个线程、256 个连接）对每条路由
预热 2s 后施压 8s，CPU 划分与 fit 的相同（服务端 0-6，wrk 在 fitbench 的 loadgen 镜像中占 7-13，后端 14-15），共 3 轮，每轮
轮换服务端的顺序。akit 从它的 `bench_serve` 示例运行（[`crates/akit/examples/bench_serve.rs`](../crates/akit/examples/bench_serve.rs)），
走的是 `akit run` 的路径，分别在 epoll（`akit-epoll`）和 io_uring（`akit-uring`）上，后者 `akit run` 还不能选择；fit 从
`FIT_REV` 处的二进制运行。`bench/modules.sh -report ID` 用 `modulesreport.py` 汇总：每个模块、每个服务端各轮的每秒请求数、
wrk 的 p99 以及服务端每个请求的 CPU（取自 `/proc/PID/stat`）的中位数，以及每个中间件比该服务端的 `none` 多出的开销。
选项：`-rounds`、`-duration`、`-servers`、`-modules`。

## akit 的配置与 fit 的不同之处

| 设置 | fit | akit |
| --- | --- | --- |
| 8081 端口 | 只有 h2c | HTTP/1.1 和 h2c：akio 的服务端总是也接受 HTTP/1.1；没有运行往那里发 HTTP/1.1 |
| 监听 backlog | `engine.backlog`，主机的 `somaxconn` | akio 的默认值；akit 还没有 `engine` 设置 |
| 连接数 | `limits.max_conns 1000000` | akit 自己还没有限制 |
| 工作线程 | fib 的，每个 CPU 一个，各监听器共享 | akio 的 I/O 线程，每个监听器按容器绑定的 CPU 每个一个；一次运行只给一个监听器施压，由它的线程干活 |
| Admin API | 在 127.0.0.1:9005，用于统计 goroutine | 没有：akit 没有 goroutine，fitbench 也不为它统计 |
