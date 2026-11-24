# 007：OpenTelemetry：fit 的 tracer，通过 akio 的 HTTP 客户端导出

[English](007-opentelemetry.md) | [简体中文](007-opentelemetry.zh-CN.md)

状态：I64 中接受（2026-11-24）。

## 背景

I64 加入 fit 的 `telemetry`：按 W3C `traceparent` 和 `sample_ratio` 为每个被采样的请求生成一个
server span，把它的 `traceparent` 传给上游，并把 span 和 `/metrics` 提供的指标以 JSON 通过
OTLP/HTTP 导出。fit 没有用 OpenTelemetry SDK
（[fit 的决策 015](https://github.com/lesismal/fit/blob/main/docs/decisions/015-opentelemetry-without-the-sdk.md)）：
未被采样的请求只多一次字段查找；被采样请求的 span 以 OTLP JSON 写入有界、分片的缓冲区，由一个
goroutine 导出。

akio 已经有一套追踪：akio-log 的 span，akio-http 为每个请求创建的 `http.server.request` span
（父 span 取自 `traceparent`），以及 akio-export：它安装一个进程级 tracer，在自己的线程上通过
OTLP（protobuf 或 JSON，HTTP 或 gRPC）推送 span、日志和指标。规则 21 要求 akio 做不到的事情加到
akio 里，而不是在 akit 中绕过，所以问题是 akit 是否应建立在这套追踪之上。

## 决策

- **akit 把 fit 的 tracer 移植到 `akit-observe`（`trace`），不打开 akio 的请求 span，也不用
  akio-export 的 tracer。** akio 的那套做的是它该做的事，但不是 fit 的 `telemetry`：
  - span 打开时，akio-http 为每个请求创建请求 span，无论是否被采样（一次装箱及其 id）；而 fit
    中未被采样的请求只多一次字段查找（规则 13）；
  - 它在响应头发出时结束，没有响应体大小、akit 意义上的路由名、监听器和上游地址，而这些 fit
    的 span 都有（`fit.listener`、`fit.route`、`fit.upstream.address`、
    `http.response.body.size`），也只有 akit 知道；
  - akio-export 的队列按 span 数和批次限界，失败后退避重试；fit 的按字节限界
    （`max_queue_bytes`），每个 `interval` 或攒到 256 KiB 时导出，失败的导出计数后丢弃，计数是
    fit 的 `fit_trace_spans_total`；
  - akio-export 的 tracer 是进程级的，只能安装一次；akit 的是一个值，由各个图共享，测试中想建
    几个就建几个。

  这些都不是 akio 引擎的缺口，而是网关自己的协议逻辑，就像访问日志的格式（I21）一样，所以不向
  akio 提请求。
- **导出走 akio 的 HTTP 客户端**（`akio_http::client::Client`），跑在一个只有一个客户端线程的独立
  akio 引擎上（`akit_observe::Otlp`），与 ACME 客户端（I31）相同：`headers_file`、`ca_file`、
  `cert_file`、`key_file` 和 `timeout` 与 fit 一致。tracer 和指标导出器各自的线程在那里等待应答，
  从不是 I/O 线程（规则 1）。
- **请求路径上**：监听器对每个请求询问图的 tracer（一次字段查找；带两个 `traceparent` 字段的请求
  按没有处理，与 fit 相同）；被采样的请求把 `traceparent` 存为 context local，代理用它替换客户端
  的；它的 span 由访问日志和指标已经共用的响应钩子结束，先编码进线程自己的缓冲区，再在只为这次
  复制持有的锁下复制进一个分片。
- **指标**与 fit 相同：抓取的各个指标族（`Scrape::write_metrics`）经 `MetricWriter` 写成
  Prometheus 文本（`TextWriter`）或 OTLP JSON（`OtlpWriter`）：counter 为累积单调 sum，gauge，
  显式边界的 histogram。akio 自己的引擎指标只出现在 Prometheus 抓取中，就像 fit 的 Go 运行时
  指标那样。

## 后果

- akit 的 span 和指标在名称和属性上与 fit 相同：为 fit 配置的仪表盘和 collector 以同样方式读取
  akit 的（scope 是 `fit`，`service.name` 默认为 `fit`）。
- akio 的请求 span 仍可由使用 akio-log 打开它的人使用；akit 不打开，所以一个请求不会被追踪两次。
- 导出协议只有 HTTP 上的 JSON，与 fit 相同；akio-export 支持的 gRPC 和 protobuf 不提供。
- 被采样的请求要分配它的 `traceparent` 和响应钩子中它的那份；未被采样的请求与以前相比没有更多
  开销（如果有访问日志或指标，钩子本来就在）。
