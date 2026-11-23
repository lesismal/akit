# 001：依赖：akio、标准库，以及少数其他

[English](001-dependency-policy.md) | [简体中文](001-dependency-policy.zh-CN.md)

状态：已采纳（2026-11-23），由用户在 I01 之前确认。

## 背景

fit 依赖 fib 和 Go 标准库，少数例外各自记录在一份决定中（ACME 的协议包、wazero）。akio 的库没有必需的第三方依赖
（[akio ADR-005](https://github.com/lesismal/akio/blob/main/docs/adr/005-dependency-policy.zh-CN.md)）：TLS、QUIC、
压缩、正则和加密都是它自己的。Rust 的标准库比 Go 的小（没有 JSON、HTTP 和加密），所以引入 serde、基于 tokio 的客户端
或 ring 之类 crate 的诱惑更大。

## 决定

- **数据面 crate 只依赖 akio 和 `std`。** akio 已有的就用（`akio-crypto`、`akio-x509`、`akio-regex`、各压缩器、
  HTTP 客户端）；它缺少而又属于网络库的，向 akio 提出需求（[akio API 需求](../akio-api-requests.zh-CN.md)）。
- **二进制在数据面路径上链接的任何 crate 中都没有异步运行时**（tokio、async-std、smol）。Kubernetes、etcd、Docker
  等的客户端基于 akio 的客户端编写，只实现 akit 用到的那部分协议。
- **JSON 和 .conf 的配置读取器由 akit 自己实现**，从构造上保证严格，并做 fuzzing。YAML 在 I24 中决定：是为 fit 接受的
  子集自己写一个读取器，还是用一个 crate，依据大小、`unsafe` 和严格程度。
- **例外**只在自己写会差很多的地方允许（预期的例外是 WASM 运行时），放在 akit 自己的 trait 后面，每个都附一份决定
  说明，写明 crate、它的 `unsafe` 和依赖数量，以及它会阻塞时如何让它远离 I/O 线程。
- **开发依赖**（测试服务器、属性测试、基准）不受限制，但不进入发布的二进制；cargo-deny 检查许可证和重复依赖。

## 影响

akit 是一个小而可审计的二进制，底下只有一个引擎，akio 在 akit 需要的地方成长。代价是格式和控制面客户端要多写 akit
自己的代码，早期迭代也更慢。
