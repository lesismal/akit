# 011：Proxy-Wasm 子集，与 fit 相同，运行在 akio 的挂起响应头和响应体过滤器上

[English](011-proxy-wasm.md) | [简体中文](011-proxy-wasm.zh-CN.md)

状态：I72 中接受（2026-11-25）。

## 背景

I72 运行 fit 所运行的 Proxy-Wasm 过滤器：fit 的
[决策 018](https://github.com/lesismal/fit/blob/main/docs/decisions/018-proxy-wasm-subset.zh-CN.md)
选定了 Proxy-Wasm v0.2.x 的请求阶段，
[决策 023](https://github.com/lesismal/fit/blob/main/docs/decisions/023-proxy-wasm-response.zh-CN.md)
在 fib 能挂起响应头（A41）、并把响应体交给别处的调用处理（A46）之后，加入了响应回调。akit 在 akio-wasm 的
`InstancePool` 上运行插件（[决策 008](008-wasm-runtime.zh-CN.md)），从不在 I/O 线程上运行（规则 1），所以它的
响应回调需要 akio 提供同样的两样东西：akio API 请求 A166 和 A167，实现为 akio 的 ADR-128
（`Context::hold_response_head`、`Context::add_body_filter`），自 `dad77a7a` 起已固定。

## 决定

- **子集与 fit 相同。** 导出 `proxy_abi_version_0_2_0` 或 `_0_2_1` 的模块是 Proxy-Wasm 过滤器，必须导出
  `proxy_on_context_create`、`proxy_on_request_headers`、`proxy_on_done`、`proxy_on_log`、`proxy_on_delete`
  和 `proxy_on_memory_allocate`。每个实例用 `vm_config` 和 `config` 启动它的根上下文，插件拒绝其中任何一个，
  加载即失败。每个请求是一个独立的上下文；宿主模块 `env` 提供 fit 的函数和 fit 的状态码：日志、本地响应
  （带 gRPC 状态）、按值和按键值对读写的头部表（请求的，含伪头部；响应的；trailer）、作为缓冲区的 VM 配置、
  插件配置和响应体、属性（配置给出的、`plugin_name`、请求的和 `response.code`），以及每个实例的指标（最多
  256 个）。对外调用、共享数据和队列、定时器、外部函数、请求的 body 和 trailer、设置属性，均回答
  `Unimplemented` 或 `NotFound`。暂停请求而不应答的过滤器，按 `on_error` 失败。`response`、`max_streams` 和
  `max_body_bytes` 的含义与 fit 相同。
- **上下文保存在其实例上。** 上下文存放在实例 store 的状态中，所以陷阱之后重建的实例不带任何上下文，与 fit
  的 worker 一样。被跟随的响应的回调，是排队给持有其上下文的那个实例的任务（`InstancePool::submit_to`），
  放在该实例自己的队列里，队列大小为 `max_streams` 的每个流四个调用（与 fit 相同）；它们先于新请求运行。
  无法排队的任务（其实例已重建、池已关闭）视为回调失败：响应失败，或在 `on_error: skip` 时不经插件继续。
  请求的上下文在最后一个回调结束时关闭（done、log、delete），或者在响应的最后一个钩子被丢弃时由排队的任务关闭，
  所以响应被切断、客户端离开或实例重建，都不会留下打开的上下文。
- **响应阶段运行在 akio 的钩子上。** 导出 `proxy_on_response_headers`（且 `response` 不是 `none`）的过滤器，
  其每个被跟随的响应的头部会被挂起（`hold_response_head`，最长 `wait_timeout` + `timeout` + 1 秒，池会先给出
  回答）；导出 `proxy_on_response_body` 或 `proxy_on_response_trailers` 且 `response: all` 的过滤器，其响应体
  经过过滤（`add_body_filter`），所以 `response: headers` 时文件响应体仍走 `sendfile`。请求回调对响应字段所做
  的修改，由一个响应过滤器在头部被挂起之前完成。

## akit 与 fit 的不同

- **`proxy_on_response_headers` 发出的本地响应会被发送**，替换原响应（`HeldHead::respond`），与 Envoy 一样：
  原响应还一点都没有发出。fib 无法替换被挂起的头部，所以 fit 让这样的响应失败；在响应体或 trailer 回调中
  发出的本地响应，两者都让响应失败。
- **响应体回调运行期间写入的数据由 akio 暂存，最多 4 MiB**（fit 的 fib：64 KB），计入流的排队字节数，
  所以依据排队字节数控制自身节奏的生产者（代理的上游）会像面对慢客户端一样等待；`max_body_bytes` 仍然限制
  一次交给插件的数据量。
- **已释放配置的插件会继续为它跟随的响应运行**，就像 fit 持有配置直到这些响应结束一样，但最多 10 分钟
  （规则 11）；之后它的池关闭。
- **akit 自己的上限和检查**（规则 11）：不是 token 的字段名、含换行或 NUL 的值、不是 UTF-8 的文本、不是
  origin-form 路径的 `:path` 和不是 token 的 `:method` 会被拒绝（`BadArgument`），而 fit 会照样存下；一个
  回调中对某个头部表做超过 1024 次修改会让调用失败；`max_streams` 最多 65536，`max_body_bytes` 最多 4 GiB。
  请求的 `host` 字段就是它的 `:authority`，与 Go 的请求保存方式一致。Proxy-Wasm 调用的那些导出函数，其类型在
  实例启动时检查：`proxy_on_request_headers` 只接受两个参数的过滤器加载失败，而 wazero 会照样调用它。
- 低于 info 级别的插件日志被丢弃，与 fit 的 ABI（I71）相同：akit 没有插件专用的日志器。

## 影响

- 用 Proxy-Wasm Go SDK 为 Envoy 编写的过滤器可原样运行：akit 的测试运行 SDK 的 `http_routing`、`properties`、
  `http_headers` 和 `http_body` 示例，以及 fit 自己的 `response` 过滤器，它们由 Go 构建并以 gzip 压缩后提交
  （`crates/akit-plugin/tests/guests`，4.3 MB），这样测试不需要 Go 工具链。
- 除 A165–A167 外，不需要修改 akio。
- 经过跟随响应的过滤器的一个响应，除请求的调用外，还要在池上付出一次头部挂起和每个响应体分段一次调用的开销；
  具体多少，与 fit 的 wazero 相比、与不用插件相比，由基准测试任务测量（规则 30；开发计划中的 WASM 一行）。
