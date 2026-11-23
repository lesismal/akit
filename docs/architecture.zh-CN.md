# akit 架构

[English](architecture.md) | [简体中文](architecture.zh-CN.md)

akit 怎样组成，一个请求怎样穿过它。这是 [开发计划](dev-plan.zh-CN.md) 要达到的目标；代码与本页不一致时，
在同一次改动中修正其中之一。

## 1. 分层

```text
┌──────────────────────────────────────────────────────────────┐
│ akit binary: run · validate · adapt · fmt · store · build     │
├───────────────────────────── control plane ──────────────────┤
│ akit-config   readers (.conf, YAML, JSON) → model → validate  │
│ akit-admin    Admin API, file provider                        │
│ akit-provider Docker, Kubernetes, Consul, etcd                │
│ akit-tls      certificate manager, ACME, OCSP                 │
│ akit-runtime  registry · compile · Graph · atomic swap        │
├───────────────────────────── data plane ─────────────────────┤
│ akit-listener → akit-router → akit-middleware → handlers      │
│   (akit-handler, akit-proxy, akit-cache, akit-plugin)         │
│ akit-upstream pools · balancing · health                      │
│ akit-l4       TCP/UDP proxy · SNI · PROXY protocol            │
│ akit-observe  logs · metrics · tracing                        │
├──────────────────────────────────────────────────────────────┤
│ akio: Engine · Conn · logical pool · akio-http/h3/ws/grpc ·   │
│       akio-tls · akio-mux · akio-socks · compressors · …      │
└──────────────────────────────────────────────────────────────┘
```

控制面从不在 akio 的 I/O 线程上运行；数据面从不解析配置，也从不等待控制面。

## 2. 线程

| 线程 | 运行什么 | 从不 |
| --- | --- | --- |
| akio I/O 线程（默认每核一个，按监听器组划分） | accept、TLS、HTTP 解析、路由、中间件、代理、上游回调 | 阻塞、长时间持锁、在缓存未命中时读文件 |
| akio 逻辑池 | `spawn_blocking` 的工作：打开文件、磁盘缓存、密码哈希、WASM 调用 | 等待 I/O 线程 |
| 控制线程 | 读取配置、编译、替换、Admin API 的处理、provider watch 的处理 | 服务流量 |
| 有界写入器 | 访问日志和错误日志、OTLP 导出 | 让队列无限增长 |

Admin API 本身由 akio 服务（在它自己的监听器上）；请求引发的工作（例如一次编译）交给控制线程，完成后再应答。

## 3. 图

`compile(config)` 把配置中的每个模块 provision 成一个 `Graph`：监听器的设置、构建好匹配器和中间件链的路由、
service、上游连接池、正在使用的证书。图一旦发布就不可变。

- **发布**：控制线程把新的 `Arc<Graph>` 存入一个原子指针，并递增一个 epoch。
- **读取**：每个 I/O 线程保留自己的 `Arc<Graph>` 和它看到的 epoch。请求检查 epoch（对一个所有线程都读、但每个
  请求都不写的缓存行做一次 relaxed load），只在它变化时刷新副本。同一线程上的请求借用该线程的副本：每个请求
  不递增任何共享的引用计数（[规则 13](rules.zh-CN.md#5-热路径)）。
- **保留**：比一次替换活得更久的请求或连接（正在传输的代理响应、WebSocket、TCP 隧道）持有自己的
  `Arc<Graph>` 克隆，在开始时取一次。
- **释放**：最后一个克隆被丢弃时，清理图中的模块：连接池排空，文件关闭。会阻塞的清理交给逻辑池。

监听器在新旧图之间按地址和传输方式比较：未变的保留 socket，新增的绑定，移除的排空
（[A1](akio-api-requests.zh-CN.md#a1-运行中引擎上的-tcp-监听器)）。

## 4. 一个被代理的 HTTP 请求

1. akio 在一个 I/O 线程上 accept 连接；akio-mux（如有配置）嗅探协议；`akio-tls` 终结 TLS；`akio-http` 解析
   请求头。
2. 线程所持图中的路由器选出一条路由：先是监听器的路由，再按优先级匹配 host、path 和其他匹配器。没有内存分配。
3. 路由的中间件链（在编译时构建）依次执行。需要等待的中间件（ForwardAuth、WASM 调用）把响应分离出来，在完成时
   恢复它。
4. `proxy` 处理器从 service 的负载均衡器取一个服务器，从上游连接池取一个连接，发送请求头。`on_body` 送来的
   请求体分片一到就转发。
5. 当上游的发送队列超过水位线时，暂停读取客户端连接；`on_drain` 恢复读取。响应体反方向同理。
6. 超时（连接、响应头、空闲、总时长）由同一线程上 akio 的定时器触发；客户端离开会取消上游请求。
7. 访问日志行推入线程的缓冲区，由日志写入器写出。

## 5. 四层

TCP 路由在两个 `Conn` 之间中继字节，两侧背压联动；当监听器要求且内核允许时，中继交给 akio 的 eBPF sockmap，
离开用户态。UDP 路由维护一张有上限的对端会话表，带空闲超时。TLS 透传读取 ClientHello 中的 SNI 而不终结 TLS
（[A5](akio-api-requests.zh-CN.md#a5-akio-mux-中的-sni-路由)）。

## 6. 配置

一个模型，三个读取器（.conf、YAML、JSON），全部严格：未知字段、重复键或类型错误都会报错，并给出路径、行和列。
模型就是 fit 的模型（[决定 002](decisions/002-config-compatibility.zh-CN.md)）；fit 有而 akit 暂不支持的
字段会以"暂不支持"拒绝，绝不忽略。`akit validate` 用一个空引擎编译配置，与 `fit validate` 的做法相同。

## 7. 错误与上限

每条路径上的错误都是值；对端或配置能控制的任何东西都不会引发 panic。每张以对端输入为键的表（UDP 会话、限流键、
缓存条目、粘性会话）都有上限和淘汰规则，每个上限都有默认值，列在配置参考中。

## 8. 测试

- 每个 crate 有单元测试；行为相同的地方移植 fit 的表驱动测试。
- 端到端测试启动二进制，面对真实上游、慢对端和被切断的连接，在 epoll 和 io_uring 上运行。
- fit 兼容性门禁用 `akit validate` 跑 fit 的模板和示例。
- 每个解析器都有 fuzz target；基准测试框架在压测机上运行。
