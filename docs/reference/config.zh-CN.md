# 配置参考

[English](config.md) | [简体中文](config.zh-CN.md)

<!-- BEGIN GENERATED 与 END GENERATED 之间的表格由 tools/config-reference.py 根据 fit 固定版本的 schema、
tests/compat/supported.txt（与 akit-config 的字段列表保持一致）和 tools/config-reference-notes.json 生成，
请勿手工修改；重新生成：python3 tools/config-reference.py。表格之外的说明和示例为手写，每个示例都由
crates/akit/tests/docs_examples.rs 以 akit validate 检查。 -->

akit 配置的每个字段：配置的每一部分是做什么的、一个示例，以及一张字段表，列出每个字段的类型、默认值和说明。akit
读取 fit 的配置（[决定 002](../decisions/002-config-compatibility.zh-CN.md)）：字段、默认值和含义都相同，格式也是同样的三种，
[.conf 文件](https://github.com/lesismal/fit/blob/main/docs/reference/fit.zh-CN.md)、YAML 或 JSON，都可以拆分到多个文件中。
akit 与 fit 不同之处，在表格中以 **akit：** 标出。

<!-- BEGIN GENERATED: summary -->
fit 的配置共有 3129 个字段（fit `7daee573`），akit 读取其中 **379** 个，其余按名称拒绝为“尚不支持”（not supported yet），另有 **1** 个 akit 自己的字段。akit 读取的 62 个对象，下文各有一张表。
<!-- END GENERATED -->

字段说明来自 fit 代码中的 `doc` 标签，是英文的，所以每个字段的完整说明在[英文版](config.md)的同一张表中；本页的表格给出每个字段的
类型、默认值、是否支持，以及 akit 与 fit 的差别。表格的读法：

- 不写的字段取其默认值（若有）；标为“必填”的字段必须写。
- 时长是 `"1.5s"`、`"2m"` 这样的字符串；“字节数或百分比”是字节数，或 `"50%"` 这样的内存份额；在 `.conf` 文件中大小可以写成
  `64KiB`、`1GiB`。
- “对象”链接到该对象自己的表。
- **尚不支持**（not supported yet）：fit 读取而 akit 尚未读取的字段。akit 按名称拒绝它，报告
  `not supported yet: <路径>` 并且不启动；akit 从不读取一个字段后又忽略它。[兼容性页面](../compatibility.zh-CN.md)
  列出 fit 的每个字段以及 akit 是否读取，包括 akit 整体拒绝的部分中的字段。
- **akit 扩展**：fit 没有的字段（[见下文](#akit-的扩展)）。

本页每个示例都是完整的配置，由一个测试以 `akit validate` 检查。示例中的文件相对于 akit 运行时所在的目录：`public/`（要提供的目录）、
`certs/example.com.pem` 和 `certs/example.com.key`（证书及其私钥）、`certs/ca.pem`（CA）、`admin.token`（令牌）和
`plugins/headers.wasm`（WebAssembly 插件）。`akit validate` 与 `akit run` 一样打开配置中指定的文件：证书无法加载、静态根目录不存在、
插件无法编译都是错误。

## 目录

- [配置](#配置)
- [监听器](#监听器)：[`listeners[]`](#cfg-listeners)、[`tls`](#cfg-listeners-tls)、[`tls.certificates[]`](#cfg-listeners-tls-certificates)、[`tls.client_auth`](#cfg-listeners-tls-client-auth)、[`socks`](#cfg-listeners-socks)、[`proxy_protocol`](#cfg-listeners-proxy-protocol)
- [路由](#路由)：[`routes[]`](#cfg-routes)、[`match`](#cfg-routes-match)、[`handler`](#cfg-routes-handler)、[`middleware[]`](#cfg-routes-middleware)
- [上游](#上游)：[`upstreams[]`](#cfg-upstreams)、[`sticky_cookie`](#cfg-upstreams-sticky-cookie)、[`health_check`](#cfg-upstreams-health-check)
- [服务](#服务)
- [访问日志](#访问日志)
- [遥测](#遥测)
- [上限](#上限)
- [Admin API](#admin-api)
- [默认 HTTPS](#默认-https)
- [指标](#指标)
- [ACME](#acme)：[`acme`](#cfg-acme)、[`on_demand`](#cfg-acme-on-demand)、[`shared_storage`](#cfg-acme-shared-storage)
- [Provider](#provider)：[`docker`](#cfg-providers-docker)、[`consul`](#cfg-providers-consul)、[`consul_kv`](#cfg-providers-consul-kv)、[`etcd`](#cfg-providers-etcd)、[`kubernetes`](#cfg-providers-kubernetes)
- [处理器](#处理器)：[`respond`](#respond)、[`static`](#static)、[`proxy`](#proxy)、[`fastcgi`](#fastcgi)、[`forward_proxy`](#forward_proxy)
- [中间件](#中间件)：[`basic_auth`](#basic_auth)、[`body_limit`](#body_limit)、[`cache`](#cache)、[`circuit_breaker`](#circuit_breaker)、[`client_cert`](#client_cert)、[`compress`](#compress)、[`concurrency_limit`](#concurrency_limit)、[`forward_auth`](#forward_auth)、[`headers`](#headers)、[`jwt`](#jwt)、[`rate_limit`](#rate_limit)、[`redirect`](#redirect)、[`rewrite`](#rewrite)、[`wasm`](#wasm)
- [akit 的扩展](#akit-的扩展)
- [akit 尚不读取的部分](#akit-尚不读取的部分)

## 配置

配置是一个对象，其中没有必填的部分：只有一条路由、且其 match 指定了主机名的配置，会以 HTTPS 提供服务，并为该主机申请证书
（[默认 HTTPS](#默认-https)）。名称精确匹配；未知字段、类型错误的值和重复的键都是错误，每个都报告其文件、行和列，akit 不会启动
（重载时则保留正在运行的配置）。

```fit
listener web :8080 http/1.1 h2c

route hello {
	match { path /hello }
	handler respond { body "hello from akit\n" }
}
```

同一配置的 YAML 写法：

```yaml
listeners:
  - name: web
    address: :8080
    protocols: [http/1.1, h2c]
routes:
  - name: hello
    match:
      path: [/hello]
    handler:
      module: respond
      params:
        body: "hello from akit\n"
```

以及 JSON 写法，即 Admin API 读写的形式：

```json
{
  "listeners": [{"name": "web", "address": ":8080", "protocols": ["http/1.1", "h2c"]}],
  "routes": [
    {"name": "hello", "match": {"path": ["/hello"]},
     "handler": {"module": "respond", "params": {"body": "hello from akit\n"}}}
  ]
}
```

`akit adapt` 与 `fit adapt` 一样在三者之间转换，`akit fmt` 把配置的各文件按各自的格式重新写出。`.conf` 文件的 `import` 和
`include` 尚不支持：在命令行上给出每个文件（`-c` 可以重复给出，也可以指定目录）。

### `config`

<!-- BEGIN GENERATED: config -->
<a id="cfg-config"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-config)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `access` | 对象 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `access_log` | [对象](#cfg-access-log) |  |  |
| `acme` | [对象](#cfg-acme) |  |  |
| `admin` | [对象](#cfg-admin) |  |  |
| `auto_https` | [对象](#cfg-auto-https) |  |  |
| `bittorrent` | 对象 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `engine` | 对象 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `limits` | [对象](#cfg-limits) |  |  |
| `listeners` | [对象](#cfg-listeners)的列表 |  |  |
| `llm` | 对象 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `log` | 对象 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `media_auth` | 对象的列表 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `media_library` | 对象的列表 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `metrics` | 对象 |  |  |
| `pprof` | 对象 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `providers` | [对象](#cfg-providers)的列表 |  |  |
| `routes` | [对象](#cfg-routes)的列表 |  |  |
| `schema_version` | 整数，1 到 1 | `1` |  |
| `services` | [对象](#cfg-services)的列表 |  |  |
| `streaming` | 对象 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `sync` | 对象 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `telemetry` | [对象](#cfg-telemetry) |  |  |
| `tunnel` | 对象 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `upstreams` | [对象](#cfg-upstreams)的列表 |  |  |
| `webrtc` | 对象 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
<!-- END GENERATED -->

## 监听器

监听器是一个地址以及在该地址上提供的协议。路由和服务按 `name` 引用监听器；不指定监听器的路由服务于所有监听器。

- **协议。** 无 TLS 时为 `http/1.1` 和 `h2c`（明文 HTTP/2）；有 `tls` 时为 `h2`（经 ALPN 选择的 HTTP/2）和 `h3`（基于 QUIC 的
  HTTP/3，使用同一端口的 UDP，并以 `Alt-Svc` 告知客户端）。`tcp` 把原始连接代理给[服务](#服务)，`udp` 代理数据报，`socks` 是
  SOCKS5 和 SOCKS4 正向代理。不写协议的监听器提供 `http/1.1`，有 TLS 时提供 `http/1.1` 和 `h2`。多个协议可以共用一个端口：
  akit 按客户端最先发送的内容区分它们。
- **TLS。** `tls` 使监听器的 HTTP 成为 HTTPS，证书可以来自文件、来自 [ACME](#acme) 为列出的名称申请，或在握手时申请
  （`on_demand`）。空的 `tls`（`{}`）为该监听器所服务路由的主机名获取证书。`client_auth` 要求客户端出示证书（双向 TLS）。证书
  文件变化时会重新加载（`akit run -watch-certs`，默认开启），不会关闭监听器。
- **在代理之后。** `proxy_protocol` 接受其信任的代理发来的 PROXY 协议头，并把每个连接当作来自该头指明的客户端。
- **在内核中。** akit 自己的 `sockmap` 在能加载 akio 的 eBPF 中继时，于内核中拼接 `tcp` 监听器被代理的连接。

重载时，地址、协议和 TLS 设置都未改变的监听器连同其连接一起保留；改变了的监听器重新绑定，旧的排空。

```fit
listener web :8080 http/1.1 h2c

listener secure :8443 http/1.1 h2 h3 {
	tls certs/example.com.pem certs/example.com.key
}

listener internal 127.0.0.1:8081 { proxy_protocol { trusted 10.0.0.0/8 } }

route { listeners secure; handler respond { body secure } }
route { listeners web internal; handler respond { body plain } }
```

### `listeners[]`

<!-- BEGIN GENERATED: listeners[] -->
<a id="cfg-listeners"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-listeners)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `address` | 字符串 | 必填 |  |
| `name` | 字符串 | 必填 |  |
| `access` | 对象 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `mptcp` | 布尔值 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `protocols` | 字符串：`http/1.1`、`h2c`、`h2`、`h3`、`tcp`、`udp`、`socks`的列表 |  | **akit：**只有 `h2c` 而没有 `http/1.1` 时按“尚不支持”拒绝：akio 的服务器在 h2c 之外总是同时接受 HTTP/1.1（akio API 请求 A168）。 |
| `proxy_protocol` | [对象](#cfg-listeners-proxy-protocol) |  |  |
| `sockmap` | 布尔值 | `false` | **akit 扩展**（[决定 010](../decisions/010-sockmap-relay.zh-CN.md)）。该监听器接受的 `tcp` 服务连接，在能加载 akio 的 eBPF sockmap 中继时（Linux 6.15 及以上，具备 `CAP_BPF` 和 `CAP_NET_ADMIN`）在内核中直接拼接，否则在用户态转发。只用于协议包含 `tcp` 的监听器。重载改变它时，只影响此后接受的连接。fit 把它当作未知字段拒绝。 |
| `socks` | [对象](#cfg-listeners-socks) |  |  |
| `tls` | [对象](#cfg-listeners-tls) |  |  |
<!-- END GENERATED -->

### `listeners[].tls`

<!-- BEGIN GENERATED: listeners[].tls -->
<a id="cfg-listeners-tls"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-listeners-tls)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `acme` | 字符串的列表 |  |  |
| `certificates` | [对象](#cfg-listeners-tls-certificates)的列表 |  |  |
| `client_auth` | [对象](#cfg-listeners-tls-client-auth) |  |  |
| `kernel_tls_offload` | 布尔值 |  | **尚不支持**（not supported yet）：按名称拒绝。**akit：**随 `engine` 的设置一起支持。 |
| `max_version` | 字符串：`1.2`、`1.3` |  |  |
| `on_demand` | 布尔值 |  |  |
<!-- END GENERATED -->

### `listeners[].tls.certificates[]`

<!-- BEGIN GENERATED: listeners[].tls.certificates[] -->
<a id="cfg-listeners-tls-certificates"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-listeners-tls-certificates)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `cert_file` | 字符串 | 必填 |  |
| `key_file` | 字符串 | 必填 |  |
<!-- END GENERATED -->

### `listeners[].tls.client_auth`

<!-- BEGIN GENERATED: listeners[].tls.client_auth -->
<a id="cfg-listeners-tls-client-auth"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-listeners-tls-client-auth)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `ca_files` | 字符串的列表 | 必填 |  |
| `mode` | 字符串：`require`、`verify_if_given` | `"require"` |  |
<!-- END GENERATED -->

### `listeners[].socks`

与 [`forward_proxy`](#forward_proxy) 处理器的参数是同一个对象。

```fit
listener egress 127.0.0.1:1080 socks {
	socks { clients 127.0.0.1/32; allow *.example.com; ports 443 }
}
```

<!-- BEGIN GENERATED: listeners[].socks -->
<a id="cfg-listeners-socks"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-listeners-socks)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `clients` | 字符串的列表 | 必填 |  |
| `allow` | 字符串的列表 |  |  |
| `connect_timeout` | 时长 | `"10s"` |  |
| `deny` | 字符串的列表 |  |  |
| `idle_timeout` | 时长 | `"5m"` |  |
| `ports` | 整数的列表 |  |  |
<!-- END GENERATED -->

### `listeners[].proxy_protocol`

<!-- BEGIN GENERATED: listeners[].proxy_protocol -->
<a id="cfg-listeners-proxy-protocol"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-listeners-proxy-protocol)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `trusted` | 字符串的列表 | 必填 |  |
<!-- END GENERATED -->

## 路由

路由说明它接受哪些请求（`match`）、请求按顺序经过的[中间件](#中间件)，以及应答的[处理器](#处理器)。每个请求只交给一条路由：

1. 请求所到监听器上、其 `match` 与请求匹配的路由：`match` 的每个字段都必须满足，同一字段内满足所列值之一即可；
2. 其中 `priority` 最高的；
3. 优先级相同时，主机最具体的（精确，然后 `*.`，然后 `**.`，然后不限主机），然后路径最具体的（精确，然后最长前缀），然后设置
   其他匹配条件最多的，然后最先写的。

没有路由接受的请求答 404。不写的 `match` 字段匹配一切；`path` 接受精确路径和以 `*` 结尾的前缀；`path_regexp` 接受 Go 语法的
正则表达式（与 fit 相同，由 akit 转换）；`expr` 用 `&&`、`||`、`!` 和括号组合匹配条件，如
`Host("a.example") && (PathPrefix("/api/") || Method("POST"))`。路由在加载配置时编译成匹配器，而不是在每个请求时逐条查找。

任何路由看到路径之前，路径先被规范化，路由的中间件和处理器拿到的是同一路径：去掉点段（`/a/./b/../c` 即 `/a/c`），重复的斜杠
合为一个；`..` 越过 `/` 的路径，以及解码后的 `%2F` 或 `%5C` 构成空段或点段的路径，答 400。

在 `.conf` 文件中，`site` 把同一组主机名的路由写在一起：其中每个 `route` 取 site 的主机和自己的路径，site 的中间件在路由自己的
中间件之前。

```fit
upstream api {
	addresses 127.0.0.1:9000
}

site example.com {
	route /api/* {
		name api-writes
		match { method POST PUT DELETE }
		middleware basic_auth {
			users {
				username admin
				password_hash $2a$14$Zkx19XLiW6VYouLHR5NmfOFU0z2GTNmpkT/5qqR7hx4IjWJPDhjvG
			}
		}
		handler proxy { upstream api }
	}
	route /api/* {
		name api
		handler proxy { upstream api }
	}
	route {
		name home
		handler respond { body home }
	}
}
```

### `routes[]`

<!-- BEGIN GENERATED: routes[] -->
<a id="cfg-routes"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `handler` | [对象](#cfg-routes-handler) | 必填 |  |
| `access` | 对象 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `listeners` | 字符串的列表 |  |  |
| `match` | [对象](#cfg-routes-match) |  |  |
| `middleware` | [对象](#cfg-routes-middleware)的列表 |  |  |
| `name` | 字符串 |  |  |
| `priority` | 整数 |  |  |
<!-- END GENERATED -->

### `routes[].match`

<!-- BEGIN GENERATED: routes[].match -->
<a id="cfg-routes-match"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-match)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `expr` | 字符串 |  |  |
| `header` | 值为字符串的列表的映射 |  |  |
| `host` | 字符串的列表 |  |  |
| `method` | 字符串的列表 |  |  |
| `path` | 字符串的列表 |  |  |
| `path_regexp` | 字符串的列表 |  |  |
| `query` | 值为字符串的列表的映射 |  |  |
| `remote_ip` | 字符串的列表 |  |  |
<!-- END GENERATED -->

### `routes[].handler`

一个模块：`module` 指定模块，`params` 是该模块自己的参数，见[处理器](#处理器)一章。

<!-- BEGIN GENERATED: routes[].handler -->
<a id="cfg-routes-handler"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-handler)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `module` | 字符串：`bt_publish`、`bt_stream`、`bt_tracker`、`fastcgi`、`forward_proxy`、`hls`、`httpflv`、`media_pages`、`media_tokens`、`proxy`、`respond`、`static`、`stream_whep`、`stream_whip`、`sync_ingest`、`vod`、`webrtc_proxy`、`websockify`、`whep`、`whip` | 必填 |  |
| `params` | 对象 |  |  |
<!-- END GENERATED -->

### `routes[].middleware[]`

一个模块：`module` 指定模块，`params` 是该模块自己的参数，见[中间件](#中间件)一章。

<!-- BEGIN GENERATED: routes[].middleware[] -->
<a id="cfg-routes-middleware"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-middleware)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `module` | 字符串：`basic_auth`、`body_limit`、`cache`、`circuit_breaker`、`client_cert`、`compress`、`concurrency_limit`、`forward_auth`、`headers`、`jwt`、`rate_limit`、`redirect`、`rewrite`、`wasm` | 必填 |  |
| `params` | 对象 |  |  |
<!-- END GENERATED -->

## 上游

上游是一组有名字的服务器，[proxy](#proxy) 处理器、[`forward_auth`](#forward_auth) 和[服务](#服务)把请求发给它。

- **负载均衡**（`load_balancing`）：`round_robin` 轮流；`weighted_round_robin` 按 `weights` 轮流；`least_conn` 发给进行中请求
  最少的地址；`random` 随机；`p2c` 随机选两个取较空闲者；`ip_hash`、`uri_hash`、`header_hash` 和 `cookie_hash` 对相同的客户端
  地址、URI、头部或 cookie 选相同的地址（后两者由 `hash_key` 指定）。`sticky_cookie` 用 akit 的 cookie 让每个客户端固定在一个
  地址上。
- **连接**：每个地址至多 `max_conns` 个连接，超出后至多 `max_pending` 个请求排队，至多 `max_idle_conns` 个空闲连接保留
  `idle_timeout`。
- **超时**：`connect_timeout`、`response_header_timeout`（到响应开始）、`response_idle_timeout`（响应体中的静默）和
  `response_timeout`（整个响应）。
- **健康**：被动地，在 `fail_timeout` 内失败 `max_fails` 次的地址被摘除 `fail_timeout`；主动地，`health_check` 以 HTTP 或 TCP
  探测每个地址。
- **协议**：`http/1.1`、`h2c`，或基于 TLS 的 `h2`，可设 `tls_server_name`、`tls_ca_file` 和客户端证书（`tls_cert_file`、
  `tls_key_file`）。

请求经 akio 的异步 HTTP 客户端发往上游：没有 I/O 线程会等待上游。

```fit
listener web :8080

upstream app 10.0.0.1:8080 10.0.0.2:8080 {
	load_balancing least_conn
	connect_timeout 3s
	max_fails 3
	fail_timeout 30s
	health_check { path /healthz; interval 5s }
}

route { handler proxy { upstream app } }
```

### `upstreams[]`

<!-- BEGIN GENERATED: upstreams[] -->
<a id="cfg-upstreams"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-upstreams)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `addresses` | 字符串的列表 | 必填 |  |
| `name` | 字符串 | 必填 |  |
| `connect_timeout` | 时长 | `"10s"` |  |
| `deny_internal` | 布尔值 |  | **尚不支持**（not supported yet）：按名称拒绝。**akit：**随 Kubernetes provider 的 ExternalName Service（`allow_external_name_services`）一起支持。 |
| `fail_timeout` | 时长 | `"10s"` |  |
| `hash_key` | 字符串 |  |  |
| `health_check` | [对象](#cfg-upstreams-health-check) |  |  |
| `idle_timeout` | 时长 | `"60s"` |  |
| `load_balancing` | 字符串：`round_robin`、`weighted_round_robin`、`least_conn`、`random`、`p2c`、`ip_hash`、`uri_hash`、`header_hash`、`cookie_hash` |  |  |
| `max_conns` | 整数，≥ 0 | `10000` |  |
| `max_fails` | 整数，≥ 0 | `1` |  |
| `max_idle_conns` | 整数，≥ 0 | `32` |  |
| `max_pending` | 整数，≥ 0 | `50000` |  |
| `protocol` | 字符串：`http/1.1`、`h2c`、`h2` | `"http/1.1"` |  |
| `response_header_timeout` | 时长 | `"60s"` |  |
| `response_idle_timeout` | 时长 | `"60s"` |  |
| `response_timeout` | 时长 | `"1h"` |  |
| `sticky_cookie` | [对象](#cfg-upstreams-sticky-cookie) |  |  |
| `tls_ca_file` | 字符串 |  |  |
| `tls_cert_file` | 字符串 |  |  |
| `tls_key_file` | 字符串 |  |  |
| `tls_server_name` | 字符串 |  |  |
| `weights` | 整数，1 到 1000的列表 |  |  |
<!-- END GENERATED -->

### `upstreams[].sticky_cookie`

<!-- BEGIN GENERATED: upstreams[].sticky_cookie -->
<a id="cfg-upstreams-sticky-cookie"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-upstreams-sticky-cookie)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `max_age` | 时长 |  |  |
| `name` | 字符串 | `"fit_sticky"` |  |
| `path` | 字符串 | `"/"` |  |
| `secure` | 布尔值 |  |  |
<!-- END GENERATED -->

### `upstreams[].health_check`

<!-- BEGIN GENERATED: upstreams[].health_check -->
<a id="cfg-upstreams-health-check"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-upstreams-health-check)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `expect_status` | 整数，100 到 599的列表 |  |  |
| `fails` | 整数，≥ 0 | `2` |  |
| `host` | 字符串 |  |  |
| `interval` | 时长 | `"5s"` |  |
| `jitter` | 时长 |  |  |
| `passes` | 整数，≥ 0 | `2` |  |
| `path` | 字符串 | `"/"` |  |
| `port` | 整数，1 到 65535 |  |  |
| `timeout` | 时长 | `"2s"` |  |
| `type` | 字符串：`http`、`tcp` | `"http"` |  |
<!-- END GENERATED -->

## 服务

服务在四层代理：`tcp` 监听器的连接逐字节转发，`udp` 监听器的数据报按客户端建立会话，发给上游的地址，按上游的负载均衡和
健康状况选择。TLS 连接可以按其请求的名称（`server_names`）区分，并且不终止 TLS 直接透传；`proxy_protocol` 告诉上游客户端是谁。
背压双向联动：一侧不读，另一侧的读取就暂停。

```fit
listener postgres :5432 tcp
listener dns :53 udp
listener tls :8443 tcp

upstream db 10.0.0.5:5432
upstream resolvers 10.0.0.53:53 10.0.0.54:53
upstream mail 10.0.0.25:8443

service postgres { listeners postgres; upstream db; idle_timeout 30m }
service dns { listeners dns; upstream resolvers }
service mail { listeners tls; upstream mail; server_names mail.example.com }
```

### `services[]`

<!-- BEGIN GENERATED: services[] -->
<a id="cfg-services"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-services)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `listeners` | 字符串的列表 | 必填 |  |
| `name` | 字符串 | 必填 |  |
| `upstream` | 字符串 | 必填 |  |
| `access` | 对象 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `idle_timeout` | 时长 |  |  |
| `max_conns` | 整数，≥ 0 | `10000` |  |
| `proxy_protocol` | 字符串：`v1`、`v2` |  |  |
| `server_names` | 字符串的列表 |  |  |
<!-- END GENERATED -->

## 访问日志

每个请求一行，写到标准输出、标准错误或文件，经过有界的缓冲：输出跟不上时，超出缓冲的行被丢弃并计数，而不是拖住请求。文件
由 akit 自己的线程写入，从不由 I/O 线程写。`format` 决定行的写法：`json`（默认，每行一个对象）、`text`（每部分写成
`key=value`，即 logfmt）或 `line`（时间和级别，然后字段写成 `key=value`）。请求的级别取决于响应的状态码：400 以下为 info，4xx
为 warn，5xx 为 error；`level` 只写该级别及以上的请求。`rotate` 尚不支持：请用系统的工具轮转文件，例如 logrotate 的
`copytruncate`。

```fit
listener web :8080

access_log {
	output stdout
	flush_interval 1s
	level warn
	format line
	time_zone UTC
}

route { handler respond { body hello } }
```

### `access_log`

<!-- BEGIN GENERATED: access_log -->
<a id="cfg-access-log"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-access-log)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `buffer_bytes` | 整数，≥ 0 | `4194304` |  |
| `flush_interval` | 时长 | `"1s"` |  |
| `format` | 字符串：`json`、`text`、`line` | `"json"` |  |
| `level` | 字符串：`info`、`warn`、`error` | `"info"` |  |
| `output` | 字符串 | `"stdout"` |  |
| `rotate` | 对象 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `time_zone` | 字符串 |  |  |
<!-- END GENERATED -->

## 遥测

基于 OTLP/HTTP 的 OpenTelemetry：每个被采样的请求一个 span，W3C trace context 继续传给上游，akit 的指标定期导出，都发给
collector。在 akit 启动时读取。

```fit
listener web :8080
route { handler respond }

telemetry {
	endpoint http://otel-collector:4318
	service_name edge
	traces { sample_ratio 0.1 }
	metrics { interval 30s }
}
```

### `telemetry`

<!-- BEGIN GENERATED: telemetry -->
<a id="cfg-telemetry"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-telemetry)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `ca_file` | 字符串 |  |  |
| `cert_file` | 字符串 |  |  |
| `endpoint` | 字符串 | `"http://127.0.0.1:4318"` |  |
| `headers_file` | 字符串 |  |  |
| `key_file` | 字符串 |  |  |
| `metrics` | [对象](#cfg-telemetry-metrics) |  |  |
| `service_name` | 字符串 | `"fit"` |  |
| `timeout` | 时长 | `"10s"` |  |
| `traces` | [对象](#cfg-telemetry-traces) |  |  |
<!-- END GENERATED -->

### `telemetry.traces`

<!-- BEGIN GENERATED: telemetry.traces -->
<a id="cfg-telemetry-traces"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-telemetry-traces)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `interval` | 时长 | `"5s"` |  |
| `max_queue_bytes` | 整数，≥ 0 | `4194304` |  |
| `sample_ratio` | 数，0 到 1 |  |  |
<!-- END GENERATED -->

### `telemetry.metrics`

<!-- BEGIN GENERATED: telemetry.metrics -->
<a id="cfg-telemetry-metrics"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-telemetry-metrics)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `interval` | 时长 | `"30s"` |  |
<!-- END GENERATED -->

## 上限

限制客户端能让 akit 为其保留多少资源，作用于所有监听器。akit 读取 `max_body_bytes`（超出的请求体答 413）和
`write_stall_timeout`（客户端可以不读响应多久）；fit 的其余上限尚不支持，由 akit 自己的界限代替：akio 每个连接的水位、联动的
背压，以及每个上游、路由和监听器的上限。[`body_limit`](#body_limit) 中间件可以进一步限制某条路由的请求体。

```fit
listener web :8080
route { handler respond }

limits {
	max_body_bytes 100MiB
	write_stall_timeout 1m
}
```

### `limits`

<!-- BEGIN GENERATED: limits -->
<a id="cfg-limits"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-limits)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `max_body_bytes` | 整数，≥ 0 | `16777216` |  |
| `max_conns` | 整数，≥ 0 | `100000` | **尚不支持**（not supported yet）：按名称拒绝。 |
| `max_header_bytes` | 整数，≥ 0 | `1048576` | **尚不支持**（not supported yet）：按名称拒绝。 |
| `max_pending_bytes` | 字节数或百分比 | `"50%"` | **尚不支持**（not supported yet）：按名称拒绝。 |
| `max_write_queue_bytes` | 整数，≥ 0 | `33554432` | **尚不支持**（not supported yet）：按名称拒绝。 |
| `write_stall_timeout` | 时长 | `"30s"` |  |
<!-- END GENERATED -->

## Admin API

Admin API 读取和替换运行中的配置：对 `/config/` 及其后的 JSON 指针执行 `GET`、`PUT`、`PATCH` 和 `DELETE`，以 `POST /load`
替换整个配置，以 `POST /reload` 重新读取配置文件，用 ETag（`If-Match`）防止更新丢失，查看已应用的配置（`/versions`），并以
`POST /rollback/{n}` 回滚到其中之一。带 `?dry_run=1` 的修改走完应用的完整流程，像真正应用一样应答，但不应用任何东西。它还在
`/metrics` 提供指标，在 `/config/effective` 提供合并了 provider 的生效配置，在 `/status` 提供状态，以 `POST /cache/purge` 清除
缓存，并在 `/dashboard/` 提供只读的仪表盘。不写 `admin` 时它关闭；不写地址时监听 `127.0.0.1:2019`。经 API 的修改与 SIGHUP
的重载一样应用，并共用同一份历史。

akit 只在回环地址或 Unix 套接字上提供 API，因为它还不能经 TLS 提供 API；`token_file` 要求每个请求带上 bearer 令牌。控制台、
store、助手和集群管理尚不支持。[Admin API 指南](../guides/admin-api.zh-CN.md)有完整的介绍。

```fit
admin {
	address 127.0.0.1:2019
	token_file admin.token
	history 20
}

route { match { host example.com }; handler respond { body hello } }
```

### `admin`

<!-- BEGIN GENERATED: admin -->
<a id="cfg-admin"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-admin)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `access` | 对象 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `address` | 字符串 | `"127.0.0.1:2019"` | **akit：**只能是回环地址或 Unix 套接字（`unix/<路径>`，权限设为 0600）：非回环地址需要 TLS，而 akit 的 Admin API 尚未提供 TLS，因此被拒绝。 |
| `assistant` | 对象 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `cert_file` | 字符串 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `client_ca_file` | 字符串 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `cluster` | 对象 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `cluster_member` | 对象 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `console` | 对象 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `disable_dashboard` | 布尔值 |  |  |
| `history` | 整数，≥ 0 | `10` |  |
| `key_file` | 字符串 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `max_config_bytes` | 整数，≥ 0 | `4194304` |  |
| `max_conns` | 整数，≥ 0 | `16` |  |
| `mptcp` | 布尔值 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `node_id` | 字符串 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `store` | 对象 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `token_file` | 字符串 |  | **akit：**在 akit 启动时读取。 |
<!-- END GENERATED -->

## 默认 HTTPS

不写监听器的配置在 443 上以 HTTPS 提供服务，证书由 [ACME](#acme) 为其路由匹配的主机名申请；并在 80 上提供 HTTP，用于跳转到
HTTPS（308）和应答 ACME 的 HTTP-01 验证。写成空 `tls` 的监听器获得其所服务路由的证书。`auto_https` 可以关闭它、关闭跳转或
改变端口。公共 CA 不签发证书的名称（`localhost`、IP 地址、`*.internal`、通配符）不会获得证书。详见
[HTTPS 指南](../guides/https.zh-CN.md)。

```fit
auto_https { disable_redirects true; https_port 8443 }
route { match { host example.com }; handler respond { body hello } }
```

### `auto_https`

<!-- BEGIN GENERATED: auto_https -->
<a id="cfg-auto-https"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-auto-https)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `disable` | 布尔值 |  |  |
| `disable_redirects` | 布尔值 |  |  |
| `http_port` | 整数，0 到 65535 | `80` |  |
| `https_port` | 整数，0 到 65535 | `443` |  |
<!-- END GENERATED -->

## 指标

按监听器和路由统计请求数和耗时，供 Admin API 的 `/metrics` 以 Prometheus 文本格式提供；不开启它时，`/metrics` 仍提供 akit 的
其余指标（上游、证书、连接、重载、akio 引擎）。它让每个请求多一点开销，所以不写时关闭，并且需要 `admin`。它没有字段：`{}`
即开启。

```fit
admin {}
metrics {}
listener web :8080
route { handler respond }
```

## ACME

如何用 ACME 申请和续期证书，用于默认 HTTPS 和监听器 `tls.acme` 列出的名称：CA 的目录（默认是 Let's Encrypt）、账户的邮箱、
账户和证书保存在哪里（一个目录，或供多台主机上的实例共享的 Redis、etcd 或 Consul，即 `shared_storage`）、应答哪些验证
（HTTP-01 和 TLS-ALPN-01），以及 `on_demand`：在握手时申请证书前先询问谁。证书按其有效期或 CA 的续期信息（ARI）续期，并装订
OCSP 响应。在 akit 启动时读取；申请、续期以及等待证书的握手都不会占用 I/O 线程。

```fit
acme {
	email ops@example.com
	directory https://acme-staging-v02.api.letsencrypt.org/directory
	storage acme
	on_demand { ask http://localhost:5555/allowed }
}

listener https :443 { tls { on_demand true } }

route { handler respond { body hello } }
```

### `acme`

<!-- BEGIN GENERATED: acme -->
<a id="cfg-acme"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-acme)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `ca_file` | 字符串 |  |  |
| `challenges` | 字符串：`http-01`、`tls-alpn-01`的列表 | `["http-01", "tls-alpn-01"]` |  |
| `directory` | 字符串 | `"https://acme-v02.api.letsencrypt.org/directory"` |  |
| `email` | 字符串 |  |  |
| `max_concurrent` | 整数，≥ 0 | `2` |  |
| `on_demand` | [对象](#cfg-acme-on-demand) |  |  |
| `request_timeout` | 时长 | `"30s"` |  |
| `shared_storage` | [对象](#cfg-acme-shared-storage) |  |  |
| `storage` | 字符串 |  | **akit：**不写时为 `$XDG_DATA_HOME/akit` 或 `~/.local/share/akit`：akit 自己的目录，而不是 fit 的；除非 `storage` 指向同一目录，两者不共享账户和证书。 |
| `timeout` | 时长 | `"10m"` |  |
<!-- END GENERATED -->

### `acme.on_demand`

<!-- BEGIN GENERATED: acme.on_demand -->
<a id="cfg-acme-on-demand"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-acme-on-demand)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `ask` | 字符串 | 必填 |  |
| `idle_timeout` | 时长 | `"168h"` |  |
| `interval` | 时长 | `"1m"` |  |
| `max_names` | 整数，≥ 0 | `1000` |  |
| `rate` | 整数，≥ 0 | `10` |  |
| `timeout` | 时长 | `"10s"` |  |
<!-- END GENERATED -->

### `acme.shared_storage`

`redis`、`etcd` 和 `consul` 之一。一个实例持有存储中的租约锁申请证书，其他实例从存储中取用；每个验证的应答也保存在那里，
因此 CA 访问到哪个实例，哪个实例就能应答。

```fit
acme {
	email ops@example.com
	shared_storage {
		redis { address redis:6379 }
	}
}

route { match { host example.com }; handler respond { body hello } }
```

<!-- BEGIN GENERATED: acme.shared_storage -->
<a id="cfg-acme-shared-storage"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-acme-shared-storage)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `consul` | [对象](#cfg-acme-shared-storage-consul) |  |  |
| `etcd` | [对象](#cfg-acme-shared-storage-etcd) |  |  |
| `lock_ttl` | 时长 | `"30s"` |  |
| `prefix` | 字符串 | `"fit/"` |  |
| `redis` | [对象](#cfg-acme-shared-storage-redis) |  |  |
| `request_timeout` | 时长 | `"10s"` |  |
<!-- END GENERATED -->

### `acme.shared_storage.redis`

<!-- BEGIN GENERATED: acme.shared_storage.redis -->
<a id="cfg-acme-shared-storage-redis"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-acme-shared-storage-redis)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `address` | 字符串 | `"127.0.0.1:6379"` |  |
| `ca_file` | 字符串 |  |  |
| `cert_file` | 字符串 |  |  |
| `db` | 整数，≥ 0 |  |  |
| `key_file` | 字符串 |  |  |
| `password_file` | 字符串 |  |  |
| `tls` | 布尔值 |  |  |
| `username` | 字符串 |  |  |
<!-- END GENERATED -->

### `acme.shared_storage.etcd`

<!-- BEGIN GENERATED: acme.shared_storage.etcd -->
<a id="cfg-acme-shared-storage-etcd"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-acme-shared-storage-etcd)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `ca_file` | 字符串 |  |  |
| `cert_file` | 字符串 |  |  |
| `endpoints` | 字符串的列表 | `["http://127.0.0.1:2379"]` |  |
| `key_file` | 字符串 |  |  |
| `password_file` | 字符串 |  |  |
| `username` | 字符串 |  |  |
<!-- END GENERATED -->

### `acme.shared_storage.consul`

<!-- BEGIN GENERATED: acme.shared_storage.consul -->
<a id="cfg-acme-shared-storage-consul"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-acme-shared-storage-consul)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `ca_file` | 字符串 |  |  |
| `cert_file` | 字符串 |  |  |
| `datacenter` | 字符串 |  |  |
| `endpoint` | 字符串 | `"http://127.0.0.1:8500"` |  |
| `key_file` | 字符串 |  |  |
| `token_file` | 字符串 |  |  |
<!-- END GENERATED -->

## Provider

Provider 从配置之外添加监听器、路由、上游和服务，并跟随其变化：Docker 容器的标签、Consul 目录的标签、Consul KV 或 etcd 中的
键，以及 Kubernetes 集群的 Gateway API 资源和 Ingress。配置中写的内容优先于所有 provider，排在前面的 provider 优先于后面的；
Admin API 的 `/config/effective` 给出合并的结果及每一部分的来源。每个 provider 的部分作为它自己的版本应用。在 akit 启动时读取。

```fit
listener web :80

provider { docker { endpoint unix:///var/run/docker.sock } }
provider kv { etcd { endpoints http://etcd:2379; prefix akit/ } }
```

### `providers[]`

<!-- BEGIN GENERATED: providers[] -->
<a id="cfg-providers"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-providers)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `consul` | [对象](#cfg-providers-consul) |  |  |
| `consul_kv` | [对象](#cfg-providers-consul-kv) |  |  |
| `docker` | [对象](#cfg-providers-docker) |  |  |
| `etcd` | [对象](#cfg-providers-etcd) |  |  |
| `kubernetes` | [对象](#cfg-providers-kubernetes) |  |  |
| `name` | 字符串 |  |  |
<!-- END GENERATED -->

### `providers[].docker`

<!-- BEGIN GENERATED: providers[].docker -->
<a id="cfg-providers-docker"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-providers-docker)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `endpoint` | 字符串 | `"unix:///var/run/docker.sock"` |  |
| `expose_all` | 布尔值 |  |  |
| `label_prefix` | 字符串 | `"fit"` |  |
| `max_containers` | 整数，≥ 0 | `1000` |  |
| `network` | 字符串 |  |  |
| `refresh_interval` | 时长 | `"30s"` |  |
| `request_timeout` | 时长 | `"10s"` |  |
| `use_published_ports` | 布尔值 |  |  |
<!-- END GENERATED -->

### `providers[].consul`

<!-- BEGIN GENERATED: providers[].consul -->
<a id="cfg-providers-consul"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-providers-consul)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `ca_file` | 字符串 |  |  |
| `cert_file` | 字符串 |  |  |
| `datacenter` | 字符串 |  |  |
| `endpoint` | 字符串 | `"http://127.0.0.1:8500"` |  |
| `expose_all` | 布尔值 |  |  |
| `key_file` | 字符串 |  |  |
| `max_services` | 整数，≥ 0 | `1000` |  |
| `refresh_interval` | 时长 | `"30s"` |  |
| `request_timeout` | 时长 | `"10s"` |  |
| `tag_prefix` | 字符串 | `"fit"` |  |
| `token_file` | 字符串 |  |  |
<!-- END GENERATED -->

### `providers[].consul_kv`

<!-- BEGIN GENERATED: providers[].consul_kv -->
<a id="cfg-providers-consul-kv"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-providers-consul-kv)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `ca_file` | 字符串 |  |  |
| `cert_file` | 字符串 |  |  |
| `datacenter` | 字符串 |  |  |
| `endpoint` | 字符串 | `"http://127.0.0.1:8500"` |  |
| `key_file` | 字符串 |  |  |
| `max_keys` | 整数，≥ 0 | `1000` |  |
| `prefix` | 字符串 | `"fit/"` |  |
| `refresh_interval` | 时长 | `"30s"` |  |
| `request_timeout` | 时长 | `"10s"` |  |
| `token_file` | 字符串 |  |  |
<!-- END GENERATED -->

### `providers[].etcd`

<!-- BEGIN GENERATED: providers[].etcd -->
<a id="cfg-providers-etcd"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-providers-etcd)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `ca_file` | 字符串 |  |  |
| `cert_file` | 字符串 |  |  |
| `endpoints` | 字符串的列表 | `["http://127.0.0.1:2379"]` |  |
| `key_file` | 字符串 |  |  |
| `max_keys` | 整数，≥ 0 | `1000` |  |
| `password_file` | 字符串 |  |  |
| `prefix` | 字符串 | `"fit/"` |  |
| `refresh_interval` | 时长 | `"30s"` |  |
| `request_timeout` | 时长 | `"10s"` |  |
| `username` | 字符串 |  |  |
<!-- END GENERATED -->

### `providers[].kubernetes`

列出并监视 akit 的 GatewayClass 下的 Gateway API Gateway 及其 HTTPRoute、GRPCRoute、TLSRoute、TCPRoute 和 UDPRoute，以及 akit
的 IngressClass 下的 Ingress，把请求发给其 Service 的就绪端点；Gateway API 的状态会写回。

```fit
provider k8s {
	kubernetes {
		namespaces default
		ingress { http_port 8080; https_port 8443 }
	}
}
```

<!-- BEGIN GENERATED: providers[].kubernetes -->
<a id="cfg-providers-kubernetes"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-providers-kubernetes)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `addresses` | 字符串的列表 |  |  |
| `allow_external_name_services` | 布尔值 |  | **尚不支持**（not supported yet）：按名称拒绝。 |
| `backends` | 字符串：`endpoints`、`cluster_ip` | `"endpoints"` |  |
| `certificates_dir` | 字符串 |  |  |
| `context` | 字符串 |  |  |
| `controller_name` | 字符串 | `"lesismal.github.io/fit"` |  |
| `ingress` | [对象](#cfg-providers-kubernetes-ingress) |  |  |
| `kubeconfig` | 字符串 |  |  |
| `listen_host` | 字符串 |  |  |
| `max_objects` | 整数，≥ 0 | `10000` |  |
| `namespaces` | 字符串的列表 |  |  |
| `request_timeout` | 时长 | `"10s"` |  |
<!-- END GENERATED -->

### `providers[].kubernetes.ingress`

<!-- BEGIN GENERATED: providers[].kubernetes.ingress -->
<a id="cfg-providers-kubernetes-ingress"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-providers-kubernetes-ingress)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `http_port` | 整数 | `80` |  |
| `https_port` | 整数 | `443` |  |
<!-- END GENERATED -->

## 处理器

路由的 `handler` 应答其请求：下列之一，由 `module` 指定，带有其 `params`。以 `akit build` 编入 akit 的其他 crate 的模块
（[模块](../modules.zh-CN.md)）以同样方式指定，其参数由模块自己检查。

<!-- BEGIN GENERATED: modules:routes[].handler -->
| 模块 | 状态 |
| --- | --- |
| `bt_publish` | **尚不支持**：按名称拒绝 |
| `bt_stream` | **尚不支持**：按名称拒绝 |
| `bt_tracker` | **尚不支持**：按名称拒绝 |
| `fastcgi` | 已支持：[参数](#cfg-routes-handler-fastcgi-params) |
| `forward_proxy` | 已支持：[参数](#cfg-routes-handler-forward-proxy-params) |
| `hls` | **尚不支持**：按名称拒绝 |
| `httpflv` | **尚不支持**：按名称拒绝 |
| `media_pages` | **尚不支持**：按名称拒绝 |
| `media_tokens` | **尚不支持**：按名称拒绝 |
| `proxy` | 已支持：[参数](#cfg-routes-handler-proxy-params) |
| `respond` | 已支持：[参数](#cfg-routes-handler-respond-params) |
| `static` | 已支持：[参数](#cfg-routes-handler-static-params) |
| `stream_whep` | **尚不支持**：按名称拒绝 |
| `stream_whip` | **尚不支持**：按名称拒绝 |
| `sync_ingest` | **尚不支持**：按名称拒绝 |
| `vod` | **尚不支持**：按名称拒绝 |
| `webrtc_proxy` | **尚不支持**：按名称拒绝 |
| `websockify` | **尚不支持**：按名称拒绝 |
| `whep` | **尚不支持**：按名称拒绝 |
| `whip` | **尚不支持**：按名称拒绝 |
<!-- END GENERATED -->

### `respond`

固定的应答：健康检查、维护页面、其他路由都不接受的请求的 404。

```fit
listener web :8080

route {
	match { path /health }
	handler respond { status 200; body ok; headers { Cache-Control no-store } }
}
```

<!-- BEGIN GENERATED: routes[].handler<respond>.params -->
<a id="cfg-routes-handler-respond-params"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-handler-respond-params)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `body` | 字符串 |  |  |
| `headers` | 值为字符串的映射 |  |  |
| `status` | 整数，100 到 599 | `200` |  |
<!-- END GENERATED -->

### `static`

从 `root` 提供文件，小文件从内存提供，支持条件请求和范围请求、目录列表（`browse`）、预压缩的文件、保存在内存中的文件的压缩
副本，以及用于单页应用的 `try_files`。文件在 akio 的文件池上打开和读取，从不在 I/O 线程上。路径中有以点开头的段
（`/.git/config`、`/.env`）答 404，`/.well-known/` 除外；`dotfiles: allow` 提供这些文件。详见[静态文件指南](../guides/static-files.zh-CN.md)。

```fit
route {
	match { host example.com }
	handler static {
		root public
		try_files {path} {path}/ /index.html
		precompressed br gzip
		compress zstd br gzip
	}
}
```

<!-- BEGIN GENERATED: routes[].handler<static>.params -->
<a id="cfg-routes-handler-static-params"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-handler-static-params)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `root` | 字符串 | 必填 |  |
| `browse` | 布尔值 |  |  |
| `browse_max_entries` | 整数，≥ 0 | `10000` |  |
| `check_interval` | 时长 | `"5s"` |  |
| `compress` | 字符串：`zstd`、`br`、`gzip`的列表 |  |  |
| `default_type` | 字符串 |  |  |
| `disk_io` | 字符串：`pool`、`auto` | `"pool"` | **akit：**只接受 `pool`：akit 只在 akio 的文件池上访问磁盘，从不在 I/O 线程上，`auto` 按“尚不支持”拒绝。 |
| `dotfiles` | 字符串：`deny`、`allow` | `"deny"` |  |
| `index` | 字符串的列表 | `["index.html"]` |  |
| `max_cache_bytes` | 整数，≥ 0 | `67108864` |  |
| `max_file_bytes` | 整数，≥ 0 | `1048576` |  |
| `precompressed` | 字符串：`br`、`zstd`、`gzip`的列表 |  |  |
| `strip_prefix` | 字符串 |  |  |
| `try_files` | 字符串的列表 |  |  |
<!-- END GENERATED -->

### `proxy`

按名称代理到一个[上游](#上游)；或以 `upstreams` 按权重把请求分给多个上游，如金丝雀发布。`failover` 指定在第一个上游没有可用
地址时接手的上游，`mirror` 把一部分请求复制到别处（只复制请求体完整到达的请求，同时至多 256 个，从不等待其结果），`retry` 在
预算内重发失败的请求。请求体和响应体双向流式转发，背压联动；WebSocket（HTTP/1.1 升级以及 HTTP/2、HTTP/3 的扩展 CONNECT）和
gRPC 调用也同样代理。客户端离开时，发往上游的请求随即放弃。详见[反向代理指南](../guides/reverse-proxy.zh-CN.md)。

```fit
upstream app 10.0.0.1:8080
upstream app-canary 10.0.0.2:8080
upstream app-backup 10.0.0.3:8080

route {
	match { host example.com; path /api/* }
	handler proxy {
		upstreams { upstream app; weight 95 }
		upstreams { upstream app-canary; weight 5 }
		failover app-backup
		host_header api.internal
		retry { attempts 2; statuses 502 503 }
		timeout 30s
	}
}
```

<!-- BEGIN GENERATED: routes[].handler<proxy>.params -->
<a id="cfg-routes-handler-proxy-params"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-handler-proxy-params)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `failover` | 字符串 |  |  |
| `host_header` | 字符串 |  |  |
| `mirror` | [对象](#cfg-routes-handler-proxy-params-mirror) |  |  |
| `retry` | [对象](#cfg-routes-handler-proxy-params-retry) |  |  |
| `splice_uploads` | 布尔值 | `true` | **尚不支持**（not supported yet）：按名称拒绝。**akit：**暂无计划：`splice(2)` 需要 `unsafe` 或 akio 的 API。 |
| `timeout` | 时长 |  |  |
| `trusted_proxies` | 字符串的列表 |  |  |
| `tunnel_idle_timeout` | 时长 | `"10m"` |  |
| `upstream` | 字符串 |  |  |
| `upstreams` | [对象](#cfg-routes-handler-proxy-params-upstreams)的列表 |  |  |
<!-- END GENERATED -->

#### `proxy` 的 `upstreams[]`

<!-- BEGIN GENERATED: routes[].handler<proxy>.params.upstreams[] -->
<a id="cfg-routes-handler-proxy-params-upstreams"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-handler-proxy-params-upstreams)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `upstream` | 字符串 | 必填 |  |
| `weight` | 整数，1 到 1000 | 必填 |  |
<!-- END GENERATED -->

#### `proxy` 的 `mirror`

<!-- BEGIN GENERATED: routes[].handler<proxy>.params.mirror -->
<a id="cfg-routes-handler-proxy-params-mirror"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-handler-proxy-params-mirror)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `upstream` | 字符串 | 必填 |  |
| `percent` | 数，0 到 100 | `100` |  |
<!-- END GENERATED -->

#### `proxy` 的 `retry`

<!-- BEGIN GENERATED: routes[].handler<proxy>.params.retry -->
<a id="cfg-routes-handler-proxy-params-retry"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-handler-proxy-params-retry)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `attempts` | 整数，1 到 10 | `2` |  |
| `budget_min_per_second` | 数，≥ 0 | `10` |  |
| `budget_ratio` | 数，0 到 1 | `0.2` |  |
| `statuses` | 整数的列表 |  |  |
<!-- END GENERATED -->

### `fastcgi`

经 TCP 或 Unix 套接字，通过有界的连接池，从 FastCGI 应用（如 PHP-FPM）获取应答，请求体和响应体双向流式转发。`try_files`
把不对应脚本的路径交给前端控制器，这是大多数 PHP 框架需要的。路径中有以点开头的段的脚本不运行，答 404；`dotfiles: allow`
运行它。

```fit
route {
	match { host example.com }
	handler fastcgi {
		address unix//run/php/php-fpm.sock
		root /var/www/app/public
		try_files {path} {path}/index.php /index.php
	}
}
```

<!-- BEGIN GENERATED: routes[].handler<fastcgi>.params -->
<a id="cfg-routes-handler-fastcgi-params"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-handler-fastcgi-params)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `address` | 字符串 | 必填 |  |
| `root` | 字符串 | 必填 |  |
| `connect_timeout` | 时长 | `"5s"` |  |
| `dotfiles` | 字符串：`deny`、`allow` | `"deny"` |  |
| `env` | 值为字符串的映射 |  |  |
| `idle_timeout` | 时长 | `"60s"` |  |
| `index` | 字符串 | `"index.php"` |  |
| `max_conns` | 整数，≥ 0 | `64` |  |
| `max_header_bytes` | 整数，≥ 0 | `65536` |  |
| `max_idle_conns` | 整数，≥ 0 | `16` |  |
| `max_pending` | 整数，≥ 0 | `1024` |  |
| `response_header_timeout` | 时长 | `"60s"` |  |
| `response_idle_timeout` | 时长 | `"60s"` |  |
| `split_path` | 字符串的列表 | `[".php"]` |  |
| `timeout` | 时长 | `"10m"` |  |
| `try_files` | 字符串的列表 |  |  |
<!-- END GENERATED -->

### `forward_proxy`

为 `clients` 列出的客户端提供 HTTP CONNECT 代理，目的地由 `allow` 和 `deny` 决定；监听器的 [`socks`](#cfg-listeners-socks)
以 SOCKS 做同样的事。除非 `allow` 指明，环回、链路本地（包括 `169.254.169.254`）、未指定和组播地址以及本机地址都被拒绝；
主机名按其连接到的地址判断，在发送任何内容之前。

```fit
listener web :8080

route {
	match { method CONNECT }
	handler forward_proxy { clients 10.0.0.0/8; allow *.example.com; ports 443 }
}
```

<!-- BEGIN GENERATED: routes[].handler<forward_proxy>.params -->
<a id="cfg-routes-handler-forward-proxy-params"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-handler-forward-proxy-params)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `clients` | 字符串的列表 | 必填 |  |
| `allow` | 字符串的列表 |  |  |
| `connect_timeout` | 时长 | `"10s"` |  |
| `deny` | 字符串的列表 |  |  |
| `idle_timeout` | 时长 | `"5m"` |  |
| `ports` | 整数的列表 |  |  |
<!-- END GENERATED -->

## 中间件

路由的 `middleware` 是下列模块的列表，每个由 `module` 指定，带有其 `params`；请求在到达处理器前按顺序经过它们，中间件也可以
自己应答，例如认证拒绝时。每条路由的中间件链在加载配置时构建。

<!-- BEGIN GENERATED: modules:routes[].middleware[] -->
| 模块 | 状态 |
| --- | --- |
| `basic_auth` | 已支持：[参数](#cfg-routes-middleware-basic-auth-params) |
| `body_limit` | 已支持：[参数](#cfg-routes-middleware-body-limit-params) |
| `cache` | 已支持：[参数](#cfg-routes-middleware-cache-params) |
| `circuit_breaker` | 已支持：[参数](#cfg-routes-middleware-circuit-breaker-params) |
| `client_cert` | 已支持：[参数](#cfg-routes-middleware-client-cert-params) |
| `compress` | 已支持：[参数](#cfg-routes-middleware-compress-params) |
| `concurrency_limit` | 已支持：[参数](#cfg-routes-middleware-concurrency-limit-params) |
| `forward_auth` | 已支持：[参数](#cfg-routes-middleware-forward-auth-params) |
| `headers` | 已支持：[参数](#cfg-routes-middleware-headers-params) |
| `jwt` | 已支持：[参数](#cfg-routes-middleware-jwt-params) |
| `rate_limit` | 已支持：[参数](#cfg-routes-middleware-rate-limit-params) |
| `redirect` | 已支持：[参数](#cfg-routes-middleware-redirect-params) |
| `rewrite` | 已支持：[参数](#cfg-routes-middleware-rewrite-params) |
| `wasm` | 已支持：[参数](#cfg-routes-middleware-wasm-params) |
<!-- END GENERATED -->

### `basic_auth`

要求用户名和密码，在 akio 的线程池上（从不在 I/O 线程上）对照 bcrypt 哈希（`htpasswd -B` 可以生成）检查。

```fit
route {
	match { host example.com; path /admin/* }
	middleware basic_auth {
		realm admin
		users {
			username alice
			password_hash $2a$14$Zkx19XLiW6VYouLHR5NmfOFU0z2GTNmpkT/5qqR7hx4IjWJPDhjvG
		}
		user_header X-User
	}
	handler respond { body hello }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<basic_auth>.params -->
<a id="cfg-routes-middleware-basic-auth-params"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-middleware-basic-auth-params)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `users` | [对象](#cfg-routes-middleware-basic-auth-params-users)的列表 | 必填 |  |
| `forward_authorization` | 布尔值 |  |  |
| `realm` | 字符串 | `"fit"` |  |
| `user_header` | 字符串 |  |  |
<!-- END GENERATED -->

#### `basic_auth` 的 `users[]`

<!-- BEGIN GENERATED: routes[].middleware[]<basic_auth>.params.users[] -->
<a id="cfg-routes-middleware-basic-auth-params-users"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-middleware-basic-auth-params-users)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `password_hash` | 字符串 | 必填 |  |
| `username` | 字符串 | 必填 |  |
<!-- END GENERATED -->

### `body_limit`

路由自己的请求体上限，低于监听器的 `limits.max_body_bytes`。

```fit
listener web :8080

route {
	match { path /upload }
	middleware body_limit { max_bytes 100000000 }
	handler respond { body ok }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<body_limit>.params -->
<a id="cfg-routes-middleware-body-limit-params"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-middleware-body-limit-params)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `max_bytes` | 整数，≥ 0 | 必填 |  |
<!-- END GENERATED -->

### `cache`

按 RFC 9111 的规则保存响应：在由一个 zone 的路由共享的有界内存层中，以及（设置 `disk` 时）在自己的线程池上写入磁盘；多个
请求同时要同一内容时只取一次（缓存锁），其余等待；可以在刷新时、或上游失败时以过期内容应答。经 Admin API 的
`POST /cache/purge` 按 URL、URL 前缀、标签或 zone 清除。详见[缓存指南](../guides/caching.zh-CN.md)。

```fit
listener web :8080

upstream app 127.0.0.1:9000

route {
	match { path /assets/* }
	middleware cache {
		zone assets
		max_bytes 256MiB
		default_ttl 10m
		stale_while_revalidate 1m
		disk { path cache/assets; max_bytes 10GiB }
	}
	handler proxy { upstream app }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<cache>.params -->
<a id="cfg-routes-middleware-cache-params"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-middleware-cache-params)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `default_ttl` | 时长 |  |  |
| `disable_lock` | 布尔值 |  |  |
| `disk` | [对象](#cfg-routes-middleware-cache-params-disk) |  |  |
| `lock_timeout` | 时长 | `"5s"` |  |
| `max_bytes` | 整数，≥ 0 | `134217728` |  |
| `max_entry_bytes` | 整数，≥ 0 | `1048576` |  |
| `max_entry_size` | 整数，≥ 0 | `1048576` | 已弃用。 |
| `max_size` | 整数，≥ 0 | `134217728` | 已弃用。 |
| `stale_if_error` | 时长 |  |  |
| `stale_while_revalidate` | 时长 |  |  |
| `zone` | 字符串 | `"default"` |  |
<!-- END GENERATED -->

#### `cache` 的 `disk`

<!-- BEGIN GENERATED: routes[].middleware[]<cache>.params.disk -->
<a id="cfg-routes-middleware-cache-params-disk"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-middleware-cache-params-disk)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `path` | 字符串 | 必填 |  |
| `max_bytes` | 整数，≥ 0 | `1073741824` |  |
| `max_entries` | 整数，≥ 0 | `100000` |  |
| `max_size` | 整数，≥ 0 | `1073741824` | 已弃用。 |
<!-- END GENERATED -->

### `circuit_breaker`

路由的请求失败或变慢过多时立即答 503，之后放过少量请求再试。

```fit
listener web :8080

upstream app 127.0.0.1:9000

route {
	middleware circuit_breaker {
		window 10s
		error_ratio 0.5
		slow_threshold 2s
		open_duration 30s
	}
	handler proxy { upstream app }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<circuit_breaker>.params -->
<a id="cfg-routes-middleware-circuit-breaker-params"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-middleware-circuit-breaker-params)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `error_ratio` | 数，0 到 1 | `0.5` |  |
| `half_open_requests` | 整数，≥ 1 | `5` |  |
| `min_requests` | 整数，≥ 1 | `20` |  |
| `open_duration` | 时长 | `"30s"` |  |
| `slow_ratio` | 数，0 到 1 | `0.5` |  |
| `slow_threshold` | 时长 |  |  |
| `statuses` | 整数的列表 |  |  |
| `window` | 时长 | `"10s"` |  |
<!-- END GENERATED -->

### `client_cert`

在 `tls.client_auth` 的 mode 为 `verify_if_given` 的监听器上，要求某条路由的客户端出示证书，并把证书主题传下去。

```fit
listener mtls :8443 http/1.1 h2 {
	tls {
		certificates { cert_file certs/example.com.pem; key_file certs/example.com.key }
		client_auth { ca_files certs/ca.pem; mode verify_if_given }
	}
}

upstream app 127.0.0.1:9000

route {
	match { path /internal/* }
	middleware client_cert { subject_header X-Client-Subject }
	handler proxy { upstream app }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<client_cert>.params -->
<a id="cfg-routes-middleware-client-cert-params"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-middleware-client-cert-params)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `optional` | 布尔值 |  |  |
| `subject_header` | 字符串 |  |  |
<!-- END GENERATED -->

### `compress`

按客户端的 `Accept-Encoding` 及其 q 值，以 zstd、brotli 或 gzip 压缩响应，流式响应体逐次写入逐次编码。

```fit
listener web :8080

route {
	middleware compress { encodings zstd br gzip; min_bytes 1024 }
	handler static { root public }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<compress>.params -->
<a id="cfg-routes-middleware-compress-params"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-middleware-compress-params)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `brotli_level` | 整数，1 到 11 |  |  |
| `encodings` | 字符串：`zstd`、`br`、`gzip`的列表 | `["zstd", "gzip", "br"]` |  |
| `gzip_level` | 整数，1 到 9 |  |  |
| `level` | 字符串：`fastest`、`default`、`better` | `"fastest"` |  |
| `max_in_flight` | 整数，≥ 1 | `1024` |  |
| `min_bytes` | 整数，≥ 0 | `1400` |  |
| `min_length` | 整数，≥ 0 | `1400` | 已弃用。 |
| `zstd_level` | 整数，1 到 22 |  |  |
<!-- END GENERATED -->

### `concurrency_limit`

路由同时进行的请求至多 `max` 个；再多一个立即以 `status` 应答。

```fit
listener web :8080

upstream app 127.0.0.1:9000

route {
	middleware concurrency_limit { max 100 }
	handler proxy { upstream app }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<concurrency_limit>.params -->
<a id="cfg-routes-middleware-concurrency-limit-params"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-middleware-concurrency-limit-params)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `max` | 整数，≥ 1 | 必填 |  |
| `status` | 整数 | `429` |  |
<!-- END GENERATED -->

### `forward_auth`

经 akio 的客户端询问认证服务请求能否通过，期间请求挂起：2xx 放行，并把 `copy_headers` 指定的字段复制到请求上；其他应答
返回给客户端。

```fit
listener web :8080

upstream auth 127.0.0.1:9091
upstream app 127.0.0.1:9000

route {
	middleware forward_auth {
		upstream auth
		uri /verify
		copy_headers Remote-User Remote-Email
	}
	handler proxy { upstream app }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<forward_auth>.params -->
<a id="cfg-routes-middleware-forward-auth-params"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-middleware-forward-auth-params)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `upstream` | 字符串 | 必填 |  |
| `copy_headers` | 字符串的列表 |  |  |
| `request_headers` | 字符串的列表 |  |  |
| `timeout` | 时长 | `"10s"` |  |
| `uri` | 字符串 | `"/"` |  |
<!-- END GENERATED -->

### `headers`

设置、添加和删除进入的请求和发出的响应的头部字段。

```fit
listener web :8080

route {
	middleware headers {
		request { set { X-Forwarded-Proto https } }
		response {
			set { Strict-Transport-Security "max-age=31536000" }
			delete Server
		}
	}
	handler respond { body hello }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<headers>.params -->
<a id="cfg-routes-middleware-headers-params"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-middleware-headers-params)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `request` | [对象](#cfg-routes-middleware-headers-params-request) |  |  |
| `response` | [对象](#cfg-routes-middleware-headers-params-response) |  |  |
<!-- END GENERATED -->

#### `headers` 的 `request` 和 `response`

<!-- BEGIN GENERATED: routes[].middleware[]<headers>.params.request -->
<a id="cfg-routes-middleware-headers-params-request"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-middleware-headers-params-request)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `add` | 值为字符串的列表的映射 |  |  |
| `delete` | 字符串的列表 |  |  |
| `set` | 值为字符串的映射 |  |  |
<!-- END GENERATED -->

`response` 的字段相同：

<!-- BEGIN GENERATED: routes[].middleware[]<headers>.params.response -->
<a id="cfg-routes-middleware-headers-params-response"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-middleware-headers-params-response)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `add` | 值为字符串的列表的映射 |  |  |
| `delete` | 字符串的列表 |  |  |
| `set` | 值为字符串的映射 |  |  |
<!-- END GENERATED -->

### `jwt`

用 akio-crypto 验证 JSON Web Token，密钥来自 PEM 文件、一个 secret，或由 akio 的客户端获取并定期刷新的 JWK 集合，并把声明作为
头部字段传下去。

```fit
listener web :8080

route {
	middleware jwt {
		jwks_url https://auth.example.com/.well-known/jwks.json
		issuer https://auth.example.com/
		audience api
		claims_headers { sub X-User }
	}
	handler respond { body hello }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<jwt>.params -->
<a id="cfg-routes-middleware-jwt-params"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-middleware-jwt-params)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `algorithms` | 字符串：`RS256`、`RS384`、`RS512`、`PS256`、`PS384`、`PS512`、`ES256`、`ES384`、`ES512`、`EdDSA`、`HS256`、`HS384`、`HS512`的列表 |  |  |
| `audience` | 字符串的列表 |  |  |
| `claims_headers` | 值为字符串的映射 |  |  |
| `hmac_secret_file` | 字符串 |  |  |
| `issuer` | 字符串 |  |  |
| `jwks_refresh` | 时长 | `"1h"` | 已弃用。 |
| `jwks_refresh_interval` | 时长 | `"1h"` |  |
| `jwks_url` | 字符串 |  |  |
| `leeway` | 时长 |  |  |
| `public_key_files` | 字符串的列表 |  |  |
| `token_from` | 字符串 | `"header"` |  |
<!-- END GENERATED -->

### `rate_limit`

把每个客户端（或某个键的每个值）限制为每秒 `rate` 个请求，突发至多 `burst` 个；保存的键的数量受 `max_keys` 限制。

```fit
listener web :8080

route {
	middleware rate_limit { rate 10; burst 20 }
	handler respond { body hello }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<rate_limit>.params -->
<a id="cfg-routes-middleware-rate-limit-params"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-middleware-rate-limit-params)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `rate` | 数 | 必填 |  |
| `burst` | 整数，≥ 0 |  |  |
| `ipv6_prefix` | 整数，0 到 128 | `64` |  |
| `key` | 字符串 | `"ip"` |  |
| `max_keys` | 整数，≥ 0 | `100000` |  |
| `status` | 整数 | `429` |  |
| `trusted_proxies` | 字符串的列表 |  |  |
<!-- END GENERATED -->

### `redirect`

以跳转应答，跳转到固定的 URL，或由正则表达式的分组构成的 URL。

```fit
route {
	match { host www.example.com }
	middleware redirect { to https://example.com{uri}; status 301 }
	handler respond
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<redirect>.params -->
<a id="cfg-routes-middleware-redirect-params"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-middleware-redirect-params)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `to` | 字符串 | 必填 |  |
| `regexp` | 字符串 |  |  |
| `status` | 整数 | `302` |  |
<!-- END GENERATED -->

### `rewrite`

在处理器之前改写路径：去掉或加上前缀，或替换正则表达式匹配的部分。

```fit
listener web :8080

upstream app 127.0.0.1:9000

route {
	match { path /api/* }
	middleware rewrite { strip_prefix /api }
	handler proxy { upstream app }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<rewrite>.params -->
<a id="cfg-routes-middleware-rewrite-params"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-middleware-rewrite-params)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `add_prefix` | 字符串 |  |  |
| `regexp` | 字符串 |  |  |
| `replacement` | 字符串 |  |  |
| `strip_prefix` | 字符串的列表 |  |  |
<!-- END GENERATED -->

### `wasm`

在 akio-wasm 的实例池上不加修改地运行 fit ABI 或 proxy-wasm 的 WebAssembly 插件，时间和内存都有上限，从不在 I/O 线程上。详见
[WebAssembly 指南](../guides/wasm.zh-CN.md)。

```fit
listener web :8080

route {
	middleware wasm {
		path plugins/headers.wasm
		instances 4
		timeout 50ms
	}
	handler respond { body hello }
}
```

<!-- BEGIN GENERATED: routes[].middleware[]<wasm>.params -->
<a id="cfg-routes-middleware-wasm-params"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-middleware-wasm-params)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `path` | 字符串 | 必填 | **akit：**插件在 akio-wasm 的实例池上运行，从不在 I/O 线程上；fit 的插件无需修改即可运行。 |
| `config` | 字符串 |  |  |
| `instances` | 整数，≥ 0 |  |  |
| `max_body_bytes` | 整数，≥ 0 | `1048576` |  |
| `max_memory` | 整数，≥ 0 | `134217728` | 已弃用。 |
| `max_memory_bytes` | 整数，≥ 0 | `134217728` |  |
| `max_streams` | 整数，≥ 0 | `1024` |  |
| `on_error` | 字符串：`fail`、`skip` | `"fail"` |  |
| `properties` | [对象](#cfg-routes-middleware-wasm-params-properties)的列表 |  |  |
| `queue` | 整数，≥ 0 | `1024` |  |
| `response` | 字符串：`all`、`headers`、`none` | `"all"` |  |
| `timeout` | 时长 | `"100ms"` |  |
| `vm_config` | 字符串 |  |  |
| `wait_timeout` | 时长 | `"1s"` |  |
<!-- END GENERATED -->

#### `wasm` 的 `properties[]`

<!-- BEGIN GENERATED: routes[].middleware[]<wasm>.params.properties[] -->
<a id="cfg-routes-middleware-wasm-params-properties"></a>

各字段的说明见英文版的[同一张表](config.md#cfg-routes-middleware-wasm-params-properties)。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `path` | 字符串的列表 | 必填 |  |
| `value` | 字符串 |  |  |
<!-- END GENERATED -->

## akit 的扩展

少数字段是 akit 自己的，用于 akio 能做而 fit 的引擎做不到的事。每个都有一份决定说明其原因，并将提交给 fit；在 fit 接受之前，
设置了它的配置只有 akit 能用，因为 fit 把它当作未知字段拒绝。

<!-- BEGIN GENERATED: extensions -->
| 字段 | 类型 | 引入于 | 决定 |
| --- | --- | --- | --- |
| `listeners[].sockmap` | 布尔值 | I51b | [010](../decisions/010-sockmap-relay.zh-CN.md): `tcp` 监听器的连接在能加载 akio 的 eBPF sockmap 中继时于内核中拼接 |
<!-- END GENERATED -->

## akit 尚不读取的部分

fit 配置的以下部分被整体按名称拒绝为“尚不支持”：`log`、`access`、`engine`、`pprof`、`llm`、`tunnel`、`webrtc`、`media_auth`、
`streaming`、`media_library`、`sync` 和 `bittorrent`，以及 fit 的媒体、BitTorrent 和 sync 处理器。在 akit 读取的部分中，上面
表格里标为 **尚不支持** 的字段也同样被拒绝。[路线图](../roadmap.zh-CN.md)说明各部分何时到来，[兼容性页面](../compatibility.zh-CN.md)
列出每个字段。
