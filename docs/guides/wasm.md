# WebAssembly plugins

[English](wasm.md) | [简体中文](wasm.zh-CN.md)

The `wasm` middleware runs a WebAssembly plugin on a route's requests: fit's plugins, built against
fit's ABI (`fit_on_request`), and Proxy-Wasm plugins, as Envoy's SDKs build them, run unchanged.
They run on akio-wasm's pool of instances, never on the threads that serve connections, each call
bounded in time and memory; a plugin that traps or runs past its time is stopped and its instance
replaced. The [reference](../reference/config.md#wasm) has every field.

## A plugin on a route

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

`path` is the module, read and compiled when the config is loaded: a plugin that does not load is an
error of the config, as any other. `config` is a string the plugin reads for settings of its own.

## Bounds

Each plugin has `instances` instances (the CPUs akit has, at most 16, by default), each with at most
`max_memory_bytes` of memory. A request waits for an instance at most `wait_timeout`, with at most
`queue` requests waiting; past either it is answered 503 at once. A call runs at most `timeout`;
`on_error` says what a call that fails leads to: `fail` answers 500, `skip` lets the request go on
without the plugin.

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

A Proxy-Wasm plugin is given its plugin configuration in `config` and its VM configuration in
`vm_config`; `properties` adds properties it reads by path besides the request's own. `response`
says which of the response's callbacks it is given (`all`, `headers` or `none`); a plugin that sees
the response's body holds at most `max_body_bytes` of it, and an instance follows at most
`max_streams` responses at once.

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

## What it costs

A request through a plugin is handed to the pool and back, and the call runs in akio-wasm's
interpreter: fit's `headers` plugin costs akit about 15 µs of CPU a request, twice what it costs fit,
whose wazero compiles plugins (the [M5 report](../../bench/results/m5-report-2026-11-25.md)). Use a
plugin for what akit's middleware cannot do; for headers, rewrites and auth, the built-in middleware
costs far less.

## LLM

fit's `llm` feature, which shares AI models among machines and serves them to agents, is not
supported yet: akit refuses `llm` by name. The [roadmap](../roadmap.md) has it after 1.0.
