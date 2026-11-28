# 从 fit 迁移

[English](migrating-from-fit.md) | [简体中文](migrating-from-fit.zh-CN.md)

akit 是在 [akio](https://github.com/lesismal/akio) 上用 Rust 重建的 fit：同样的配置、同样的三种格式、同样的 Admin API、同样的命令
和参数以及访问日志字段（[决定 002](../decisions/002-config-compatibility.zh-CN.md)），凡是 fit 的名字写着 "fit" 的地方，akit 用自己
的名字（[决定 037](../decisions/037-akit-names.zh-CN.md)）。把配置从 fit 迁到 akit，本应只是换一个程序，再加上[下面这些名字](#akit-的名字)。
其余挡在中间的是 akit 尚不支持的部分，akit 会按名称指出而不是忽略它们；本指南列出这些部分，[兼容性页面](../compatibility.zh-CN.md)
列出 fit 的每个字段以及 akit 是否读取。

## 先检查配置

```bash
akit validate -c /etc/fit
```

它像 `akit run` 一样读取配置并打开其中指定的文件，输出 `valid`，或者每个问题及其文件、行和列。akit 尚不支持的字段也是其中之一：

```text
fit.conf:12:2: access: not supported yet
fit.conf:40:4: routes[3].handler.module: not supported yet: the "hls" handler
```

删除或替换每一项，再次检查。akit 拒绝的东西从不会被读取后又忽略，所以通过检查的配置在 akit 支持的范围内与 fit 的运行方式相同。

## akit 尚不运行的部分

- **整个部分**：`pprof`，以及网关之外的角色：`llm`、`tunnel`、`webrtc`、`media_auth`、`streaming`、`media_library`、`sync` 和
  `bittorrent`。[路线图](../roadmap.zh-CN.md)说明各自何时到来。
- **处理器**：媒体、BitTorrent 和 sync 的处理器：`hls`、`httpflv`、`vod`、`whip`、`whep`、`stream_whip`、`stream_whep`、
  `webrtc_proxy`、`websockify`、`media_pages`、`media_tokens`、`bt_publish`、`bt_stream`、`bt_tracker` 和 `sync_ingest`。fit 的
  每个中间件都已支持。
- **akit 读取的部分中的字段**：`engine` 的 `loop_wait`、`poll_batch` 和 `kernel_tls_offload`（`engine` 的其余字段映射到 akio 的引擎上，见[参考](../reference/config.zh-CN.md#引擎)）；所有的
  `mptcp`；`tls.kernel_tls_offload`；`upstreams[].deny_internal`；proxy 的 `splice_uploads`；Kubernetes provider 的
  `allow_external_name_services`；以及 `admin` 中除 `address`、`token_file`、`history`、`max_config_bytes`、`max_conns`、
  `disable_dashboard`、`access`、`cert_file`、`key_file`、`client_ca_file`、`console`、`store`、`cluster` 和 `cluster_member` 以外的一切：没有助手。
- **取值**：`static` 的 `disk_io auto`（akit 只在文件池上访问磁盘）。
- **`.conf` 文件中**：`import` 和 `include`；用 `-c` 指定每个文件，或指定目录。
- **命令和参数**：akit 有 `run`（带 fit 的 `-admin` 和 `-timeout`）、`validate`、`adapt`、`fmt`、`build`、`hash-password`、
  `media-token`、`token`、`bt`、`cluster`、`store` 和 `version`。fit 的 `check` 和 `sync` 暂时没有。fit 保存的 store（及其密钥文件）在 akit
  中原样可用；其密码的环境变量为 `AKIT_STORE_PASSWORD`（fit 的 `FIT_STORE_PASSWORD`）。

## akit 的名字

fit 的名字写着 "fit" 的地方，akit 的名字写 "akit"，而且不把 fit 的名字作为别名接受（[决定 037](../decisions/037-akit-names.zh-CN.md)）。
fit 的用户要改这些：

| 内容 | fit | akit | 要改的 |
| --- | --- | --- | --- |
| Docker 标签（`providers[].docker.label_prefix`） | `fit.enable`、`fit.routes.<r>.host` 等 | `akit.enable`、`akit.routes.<r>.host` 等 | 标签，或设置 `label_prefix fit` |
| Consul tag（`providers[].consul.tag_prefix`） | `fit.enable=true` 等 | `akit.enable=true` 等 | tag，或设置 `tag_prefix fit` |
| Consul KV 和 etcd 的键（`prefix`） | `fit/` | `akit/` | 键，或设置 `prefix fit/` |
| 共享证书（`acme.shared_storage.prefix`） | `fit/` | `akit/` | 要共享 fit 的证书，设置 `prefix fit/` |
| Kubernetes 控制器（`controller_name`） | `lesismal.github.io/fit` | `lesismal.github.io/akit` | GatewayClass 和 IngressClass，或设置 `controller_name` |
| Kubernetes Ingress 注解 | `fit.lesismal.github.io/*` | `akit.lesismal.github.io/*` | 注解 |
| 粘性 cookie（`sticky_cookie.name`） | `fit_sticky` | `akit_sticky` | 要保留客户端的会话，设置 `name fit_sticky` |
| `basic_auth` 的 `realm`；`jwt` 的质询 | `fit` | `akit` | `realm fit`（`jwt` 的不可设置） |
| `telemetry.service_name`；OTLP scope；span 属性 | `fit`；`fit`；`fit.listener`、`fit.route`、`fit.upstream.address` | `akit`；`akit`；`akit.listener`、`akit.route`、`akit.upstream.address` | `service_name fit`；查询和仪表盘 |
| Prometheus 指标 | `fit_*` | `akit_*` | 仪表盘、告警和 recording rule |
| `Cache-Status` 的缓存名 | `fit; hit` | `akit; hit` | 读取它的程序 |
| 中途断开的消息体的 trailer | `Fit-Error` | `Akit-Error` | 读取它的程序 |
| Admin API 的请求头字段 | `Fit-Config-Version`、`Fit-Dry-Run` | `Akit-Config-Version`、`Akit-Dry-Run` | 脚本和客户端 |
| 控制台的 cookie 和 CSRF 请求头 | `fit_console_<端口>`、`X-Fit-Csrf` | `akit_console_<端口>`、`X-Akit-Csrf` | 脚本；需要重新登录 |
| 配置导出的文件名 | `fit-config-v<n>.json` | `akit-config-v<n>.json` | 处理它的程序 |
| WebAssembly 插件 | 导出 `fit_on_request`，从 `fit` 导入 | 导出 `akit_on_request`，从 `akit` 导入 | 改名后重新构建（[WebAssembly](wasm.zh-CN.md#akit-的-abi)） |
| 默认配置文件 | `fit.json` | `akit.json` | 文件名，或 `-c fit.json` |
| `adapt` 的 `.conf` 格式 | `-from fit`、`-to fit` | `-from akit`、`-to akit` | 脚本 |

配置的字段名沿用 fit 的，`.conf`、YAML 和 JSON 格式是 fit 的，Proxy-Wasm 过滤器无需修改即可运行。akit 和 fit 并存时，不再拿走
对方的标签、键、GatewayClass 或 cookie。

## 其他需要留意的

- **ACME 的存储**默认是 `~/.local/share/akit`，而不是 fit 的目录。要沿用 fit 的账户和证书，把 `acme.storage` 指向 fit 的目录，或把
  `shared_storage` 指向 fit 的存储。
- **平台**：Linux（akit 运行在 akio 的 epoll 后端上；环境中设置 akio 的 `AKIO_DEFAULT_BACKEND=uring` 时运行在
  io_uring 上，5.19 及以上，这是测试和诊断用的开关，不是配置字段），以及用于开发的 macOS。不支持 Windows。
- **fit ABI 的 WebAssembly 插件**要按 akit 的名字重新构建（见上文）；Proxy-Wasm 过滤器无需修改即可运行。它们运行在 akio-wasm 的
  解释器上，而不是 wazero 的编译器上：每次调用开销更大（[WebAssembly](wasm.zh-CN.md)）。

## akit 自己的字段

`listeners[].sockmap` 用 akio 的 eBPF sockmap 中继，在内核中拼接 `tcp` 监听器被代理的连接（[决定 010](../decisions/010-sockmap-relay.zh-CN.md)）。
fit 把它当作未知字段拒绝：设置了它的配置只有 akit 能用。

```fit
listener db :5432 tcp { sockmap true }

upstream postgres 10.0.0.5:5432

service postgres { listeners db; upstream postgres }
```

## 转换格式

`akit adapt` 和 `akit fmt` 写出配置的方式与 `fit adapt` 和 `fit fmt` 相同，所以两者都可以在 `.conf`、YAML 和 JSON 之间转换配置：

```bash
akit adapt -c fit.conf -to yaml -o fit.yaml
akit adapt -c fit.yaml -to akit -o akit.conf
akit fmt -w -c /etc/fit
```
