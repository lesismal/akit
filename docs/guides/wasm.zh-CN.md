# WebAssembly 插件

[English](wasm.md) | [简体中文](wasm.zh-CN.md)

`wasm` 中间件在路由的请求上运行 WebAssembly 插件：akit ABI 的插件（即 fit 的 ABI 改名之后，[见下文](#akit-的-abi)），以及用
Envoy 的 SDK 构建的 Proxy-Wasm 插件（无需修改）。它们在 akio-wasm 的实例池上运行，从不在服务连接的线程上，每次调用的时间和内存都有上限；
trap 或超时的插件被停止，其实例被替换。[配置参考](../reference/config.zh-CN.md#wasm)列出每个字段。

## 路由上的插件

```fit
listener web :8080

upstream app 127.0.0.1:9000

route {
	middleware wasm {
		path plugins/headers.wasm
		config "X-Plugin: on"
	}
	handler proxy { upstream app }
}
```

`path` 是模块，在加载配置时读取并编译：无法加载的插件和其他错误一样，是配置的错误。`config` 是插件读取的字符串，用于插件自己的
设置。

## 上限

每个插件有 `instances` 个实例（默认为 akit 可用的 CPU 数，至多 16），每个实例的内存至多 `max_memory_bytes`。请求等待实例至多
`wait_timeout`，至多 `queue` 个请求等待；超出任一项时立即答 503。一次调用至多运行 `timeout`；`on_error` 决定调用失败的后果：
`fail` 答 500，`skip` 让请求不经插件继续。

```fit
listener web :8080

route {
	middleware wasm {
		path plugins/headers.wasm
		instances 8
		queue 256
		timeout 20ms
		wait_timeout 200ms
		max_memory_bytes 64MiB
		on_error skip
	}
	handler respond { body hello }
}
```

## akit 的 ABI

akit 自己 ABI 的插件是一个 WASI reactor（`-buildmode=c-shared`，因而导出 `_initialize`），导出 `akit_on_request`，并从模块 `akit`
导入宿主函数。它就是 fit 的 ABI（[fit 的 `plugin/wasm/abi.go`](https://github.com/lesismal/fit/blob/main/plugin/wasm/abi.go)），
只是 fit 的名字改了名：fit 的 `fit_on_request` 和模块 `fit` 不被读取（[决定 037](../decisions/037-akit-names.zh-CN.md)）。既不导出
`akit_on_request`、也不导出 Proxy-Wasm ABI 版本的插件，在加载配置时被拒绝。

```text
export  akit_on_request() i32          0：请求继续；1：插件已回答该请求

import  akit.header_get(kind, name, name_len, buf, buf_len) i32
                                       把字段的第一个值写入 buf（至多 buf_len 字节），返回其长度，
                                       没有时返回 -1；kind 0 是请求的字段，1 是插件迄今设置的响应字段
        akit.header_set(kind, name, name_len, value, value_len)
        akit.header_add(kind, name, name_len, value, value_len)
        akit.header_del(kind, name, name_len)
        akit.request_get(field, buf, buf_len) i32
                                       同 header_get，field 0 是方法，1 是（解码后的）路径，
                                       2 是查询串，3 是主机，4 是客户端地址，5 是插件的 config
        akit.respond(status, body, body_len)
                                       以 status（200 到 599）、body（至多 1 MiB）和插件设置的
                                       响应字段回答
        akit.log(level, msg, msg_len)  把 msg 写入 akit 的日志：-4 debug、0 info、4 warn、8 error
```

字符串是指向插件内存的指针和长度。名字或值至多 64 KiB，一次调用对响应字段至多做 256 处修改，对请求字段也一样；字段名必须是
HTTP token，值中不能有换行或 NUL；超出任何一项，调用失败，按 `on_error` 处理。

fit 的 Go 包 `plugin/wasm/guest` 把其中 `//go:wasmexport` 和 `//go:wasmimport` 的名字从 `fit` 改为 `akit`（`fit_*` 改为 `akit_*`）
之后，即可构建这样的插件；包中其他内容不变。`tools/plugin-guest.sh` 用 fit 的测试插件 `plugin/wasm/testdata/headers` 为 akit 的
测试这样构建：

```bash
sed -i -e 's#^//go:wasmexport fit_#//go:wasmexport akit_#' \
       -e 's#^//go:wasmimport fit #//go:wasmimport akit #' \
       -e 's#^//go:wasmimport fit_sync #//go:wasmimport akit_sync #' plugin/wasm/guest/*.go
GOOS=wasip1 GOARCH=wasm go build -buildmode=c-shared -o plugin.wasm .
```

为 fit 构建、未经改名的插件不能在 akit 中加载，为 akit 构建的插件也不能在 fit 中加载。

## Proxy-Wasm

Proxy-Wasm 插件从 `config` 获得插件配置，从 `vm_config` 获得 VM 配置；`properties` 在请求自身的属性之外增加插件按路径读取的
属性。`response` 决定插件获得响应的哪些回调（`all`、`headers` 或 `none`）；看到响应体的插件至多保留其中 `max_body_bytes`，一个
实例同时至多跟随 `max_streams` 个响应。

```fit
listener web :8080

upstream app 127.0.0.1:9000

route {
	middleware wasm {
		path plugins/http_headers.wasm
		config "{\"header\": \"x-routed\", \"value\": \"yes\"}"
		properties { path tenant id; value acme }
		response headers
	}
	handler proxy { upstream app }
}
```

## 开销

经过插件的请求要交给实例池再交回，调用在 akio-wasm 的解释器中运行：fit 的 `headers` 插件让 akit 每个请求多花约 15 µs CPU，是
fit 的两倍，因为 fit 的 wazero 会编译插件（[M5 报告](../../bench/results/m5-report-2026-11-25.md)）。插件用于 akit 的中间件做不到
的事；头部、改写和认证，内置中间件的开销要小得多。

## LLM

fit 的 `llm` 功能（在多台机器间共享 AI 模型并提供给 agent）尚不支持：akit 按名称拒绝 `llm`。[路线图](../roadmap.zh-CN.md)把它
排在 1.0 之后。
