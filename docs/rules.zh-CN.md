# akit 开发规则

[English](rules.md) | [简体中文](rules.zh-CN.md)

akit 的每次改动都遵守这些规则。它们是 [路线图设计原则](roadmap.zh-CN.md#设计原则) 在代码层面的具体要求，改编自
[fit 的规则](https://github.com/lesismal/fit/blob/main/docs/rules.zh-CN.md) 和
[akio 的规则](https://github.com/lesismal/akio/blob/main/docs/rules.zh-CN.md)，二者分别为 Go 网关和 akit 底下的引擎
坚持同样的底线。

## 1. 不阻塞 I/O 线程

1. **akio I/O 线程上的任何代码都不阻塞。** handler、中间件、上游回调和完成回调不做同步磁盘 I/O（`std::fs`、
   `File::open`、`metadata`），不做 DNS 解析（`to_socket_addrs`），不 `thread::sleep`，不等待 channel、条件变量、
   join handle 或 future，不做阻塞 `connect`，也不持有其他线程可能长时间持有的锁。每次测试都开启 akio 的慢回调检测，
   它会报告漏网之鱼。
2. **上游请求一律用 akio 的异步客户端**（`Client::request`、`H2Client::request`、H3 客户端），配完成回调。请求路径上
   不用阻塞的 HTTP 客户端。
3. **handler 返回后才应答时**，显式保持所需的东西存活（detach 的响应、`Conn` 的克隆、指向图的 `Arc`），并在客户端
   离开时取消对应的上游工作。
4. **确实只能阻塞的工作**（磁盘缓存、DNS、ACME、WASM、缓存未命中时读文件）用 `spawn_blocking` 放到 akio 的逻辑线程池，
   或放到 akit 自己的有界池里，并设超时。池满（`Overloaded`）时要处理，不能 unwrap。池之间的等待只能单向。

## 2. body 流式、有界

5. **请求体和响应体都流式处理**（`BodyMode::Stream`、`on_body`、客户端的流式响应），边收边转发。
6. **交给回调的数据只在本次调用内有效。** 要在调用之后保留就复制，最好复制到 `akio-buf` 的池化缓冲区。
7. **只有确实需要完整内容的功能才缓冲 body，而且必须设上限**，超限时返回 413，或改为直通。
8. **背压联动。** 一侧待发字节超过水位时，对另一侧 `hold_reads`，收到 `on_drain` 后恢复。系统里不允许出现无界的
   队列或缓冲。

## 3. 其他协议同理

9. **WebSocket、gRPC 流、CONNECT、SOCKS 和 TCP/UDP 隧道都是两侧回调驱动**，双向背压，同样不阻塞、不缓冲整条流。

## 4. 超时与上限

10. **每个外部交互都有超时**：连接、首字节、空闲、总时长。
11. **每种资源都有上限，并有安全的默认值**：连接数、流数、会话数、header 大小、body 大小、并发数，以及以对端输入为键
    的表的大小。配置里什么都不写也受限；“不限”必须显式选择。

## 5. 热路径

12. **能在编译配置时做的事不留到请求时。** middleware 链、匹配器、正则和固定的 header 都在编译时准备好。请求期只读
    不可变的图。
13. **每个请求：不加全局锁，不增减共享的引用计数，尽量少分配内存。** 图的指针按 I/O 线程缓存；计数器按线程或分片；
    能在编译时格式化的内容不在请求时格式化。

## 6. Rust

14. **akit 中没有 `unsafe`。** 每个 crate 都是 `#![forbid(unsafe_code)]`。需要 `unsafe` 的东西属于 akio，经过它的
    unsafe 审查。
15. **对端或配置的输入不会引发 panic。** 对客户端、上游或配置文件能控制的数据，不用 `unwrap`、`expect`、可能越界的
    索引或可能溢出的算术；错误是值。回调里的 panic 会拖垮一个 I/O 线程上的所有连接，所以它总是 bug，fuzzing 专门
    寻找它。
16. **数据面上没有异步运行时。** 数据面链接的 akit crate 中不用 tokio、async-std 或 `block_on`；需要它的依赖要么拒绝，
    要么只留在工具里。
17. **`cargo fmt`、`cargo clippy --all-targets -- -D warnings` 和 `cargo deny check` 都要通过。** MSRV 与 akio 相同。

## 7. 验证

18. **每个数据面功能都要有测试**：慢客户端、慢上游、大 body、中途断开，以及它引入的上限（略低于、等于和超过），在
    epoll 和 io_uring 上都要测。新代码和它的测试在同一个改动里提交。
19. **与 fit 共有的行为用 fit 的测试来检验。** 从 fit 移植的功能要一并移植它的测试和示例；fit 模板中的配置必须通过
    `akit validate`，或者以指出该字段的“not supported yet”失败。
20. **引入新依赖要在 `docs/decisions/` 写一份决定说明**
    （[决定 001](decisions/001-dependency-policy.zh-CN.md)）。

## 8. akio 不足时

21. **akio 暂时做不到的，加到 akio 里，不在 akit 里绕过。** 把它写进 [akio API 需求](akio-api-requests.zh-CN.md)
    及其英文版：问题、证据、建议的 API、akit 的用法。akit 的开发会话可以自己在 akio 中实现，并遵守 akio 自己的规则
    （每个后端上的测试、它的 unsafe 审查、热路径上同一轮的 A/B）。不依赖它的工作照常推进；临时措施也必须遵守其他
    所有规则，并在 progress 文件中记录，注明将来由哪项需求替换。

## 9. akio 版本

22. **akit 按提交哈希依赖 akio**，即 `git = "https://github.com/lesismal/akio", rev = "<hash>"`，不用 path 依赖，也不在
    akit 中提交 `[patch]`。升级到 akio 的某个改动，要等该提交进入 akio 的 `main` 且检查通过后再更新 `rev`。只有所需的
    版本已经有 tag 时才用 tag。

## 10. 压测

23. **压测和基准只在 Linux 的压测机上进行**，即 fit 和 akio 共用的那台安静主机，不在别处。不采用 macOS、Docker
    Desktop 或有负载的主机上的数字；主机有负载时只报告同一轮 A/B 的比值。同一时间只有一个会话使用压测机。
24. **验证按改动大小。**

    | 改动 | 基准 |
    | --- | --- |
    | 文档、测试、控制面（配置、Admin API、证书、provider） | 不跑 |
    | 数据面但不在热路径上 | 不跑，或跑该 crate 的微基准 |
    | 热路径 | 在压测机上同一轮交替进行的 A/B，只跑涉及的场景 |
    | 里程碑结束 | 完整基准框架，所有服务器（包括 fit），并写报告 |

25. **测试按影响范围。** 开发过程中，测试改动的 crate 以及依赖它们的 crate；整个工作区（macOS、Linux epoll、Linux
    io_uring）在推送前跑一次。同时跑完整测试的会话最多两个。基准在后台运行，在推送被测改动之前读取结果。

## 11. 文档

26. **每份文档都是双语的**：英文的 `X.md` 和中文的 `X.zh-CN.md`，各自在标题下方链接对方，在同一个提交中一起修改。
    一边新增的章节、表格行或状态，另一边也要新增。
27. **计划始终如实。** 迭代的状态在两份 dev-plan 文件中同时更新，它构建的内容写进两份 progress 文件，都在完成它的
    提交里。

## 12. 压测报告

28. **每份完整基准报告都要说明 akit 所处的位置**：它在每个负载和协议上相对 nginx、Caddy、Traefik 和 fit 排第几，
    各服务器按 fit 的 benchreport 评分排名，使两个项目的报告读起来一致。
