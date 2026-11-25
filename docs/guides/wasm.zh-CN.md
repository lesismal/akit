# WebAssembly 插件

[English](wasm.md) | [简体中文](wasm.zh-CN.md)

`wasm` 中间件在路由的请求上运行 WebAssembly 插件：按 fit 的 ABI（`fit_on_request`）构建的 fit 插件，以及用 Envoy 的 SDK 构建的
Proxy-Wasm 插件，都无需修改即可运行。它们在 akio-wasm 的实例池上运行，从不在服务连接的线程上，每次调用的时间和内存都有上限；
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
