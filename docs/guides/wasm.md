# WebAssembly plugins

[English](wasm.md) | [简体中文](wasm.zh-CN.md)

The `wasm` middleware runs a WebAssembly plugin on a route's requests: plugins of akit's ABI, which
is fit's with its names renamed ([below](#akits-abi)), and Proxy-Wasm plugins, as Envoy's SDKs build
them, unchanged.
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

## akit's ABI

A plugin of akit's own ABI is a WASI reactor (`-buildmode=c-shared`, so that it exports
`_initialize`) that exports `akit_on_request` and imports its host functions from the module `akit`.
This is fit's ABI ([fit's `plugin/wasm/abi.go`](https://github.com/lesismal/fit/blob/main/plugin/wasm/abi.go))
with fit's names renamed: fit's `fit_on_request` and module `fit` are not read
([decision 037](../decisions/037-akit-names.md)). A plugin that exports neither `akit_on_request` nor
a Proxy-Wasm ABI version is refused when the config loads.

```text
export  akit_on_request() i32          0: the request goes on; 1: the plugin answered it

import  akit.header_get(kind, name, name_len, buf, buf_len) i32
                                       the field's first value written to buf, up to buf_len
                                       bytes, and its length, or -1; kind 0 is the request's
                                       fields, 1 the response's as the plugin set them so far
        akit.header_set(kind, name, name_len, value, value_len)
        akit.header_add(kind, name, name_len, value, value_len)
        akit.header_del(kind, name, name_len)
        akit.request_get(field, buf, buf_len) i32
                                       as header_get, for field 0 the method, 1 the path
                                       (decoded), 2 the query, 3 the host, 4 the client's
                                       address, 5 the plugin's config
        akit.respond(status, body, body_len)
                                       answer with status (200 to 599), body (1 MiB at most)
                                       and the response's fields the plugin set
        akit.log(level, msg, msg_len)  write msg to akit's log: -4 debug, 0 info, 4 warn, 8 error
```

Strings are a pointer and a length into the plugin's memory. A name or value is at most 64 KiB, a
call makes at most 256 changes to the response's fields and as many to the request's, and a field
name must be an HTTP token and a value hold no line break or NUL; past any of these the call fails,
and goes as `on_error` says.

fit's Go package `plugin/wasm/guest` builds such a plugin once its `//go:wasmexport` and
`//go:wasmimport` names are renamed from `fit` to `akit` (and `fit_*` to `akit_*`); nothing else in
it changes. `tools/plugin-guest.sh` does this for akit's tests, with fit's test plugin
`plugin/wasm/testdata/headers`:

```bash
sed -i -e 's#^//go:wasmexport fit_#//go:wasmexport akit_#' \
       -e 's#^//go:wasmimport fit #//go:wasmimport akit #' \
       -e 's#^//go:wasmimport fit_sync #//go:wasmimport akit_sync #' plugin/wasm/guest/*.go
GOOS=wasip1 GOARCH=wasm go build -buildmode=c-shared -o plugin.wasm .
```

A plugin built for fit, unchanged, does not load in akit, nor one built for akit in fit.

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
