# 从 fit 迁移

[English](migrating-from-fit.md) | [简体中文](migrating-from-fit.zh-CN.md)

akit 是在 [akio](https://github.com/lesismal/akio) 上用 Rust 重建的 fit：同样的配置、同样的三种格式、同样的 Admin API、同样的命令
和参数、指标名和访问日志字段（[决定 002](../decisions/002-config-compatibility.zh-CN.md)）。把配置从 fit 迁到 akit，本应只是换一个
程序。挡在中间的是 akit 尚不支持的部分，akit 会按名称指出而不是忽略它们；本指南列出这些部分，[兼容性页面](../compatibility.zh-CN.md)
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

- **整个部分**：`access`（允许和拒绝列表）、`pprof`，以及网关之外的角色：`llm`、`tunnel`、`webrtc`、`media_auth`、`streaming`、`media_library`、`sync` 和
  `bittorrent`。[路线图](../roadmap.zh-CN.md)说明各自何时到来。
- **处理器**：媒体、BitTorrent 和 sync 的处理器：`hls`、`httpflv`、`vod`、`whip`、`whep`、`stream_whip`、`stream_whep`、
  `webrtc_proxy`、`websockify`、`media_pages`、`media_tokens`、`bt_publish`、`bt_stream`、`bt_tracker` 和 `sync_ingest`。fit 的
  每个中间件都已支持。
- **akit 读取的部分中的字段**：`engine` 的 `loop_wait`、`poll_batch` 和 `kernel_tls_offload`（`engine` 的其余字段映射到 akio 的引擎上，见[参考](../reference/config.zh-CN.md#引擎)）；所有的
  `access` 和 `mptcp`；`tls.kernel_tls_offload`；`upstreams[].deny_internal`；proxy 的 `splice_uploads`；Kubernetes provider 的
  `allow_external_name_services`；以及 `admin` 中除 `address`、`token_file`、`history`、`max_config_bytes`、`max_conns` 和
  `disable_dashboard` 以外的一切：没有 TLS，因而不能监听回环以外的地址，也没有控制台、store、助手或集群。
- **取值**：`static` 的 `disk_io auto`（akit 只在文件池上访问磁盘）。
- **`.conf` 文件中**：`import` 和 `include`；用 `-c` 指定每个文件，或指定目录。
- **命令和参数**：akit 有 `run`、`validate`、`adapt`、`fmt`、`build` 和 `version`；`run` 的 `-admin` 和 `-timeout` 被拒绝。fit 的
  `check`、`hash-password`、`token`、`store`、`sync`、`bt` 和 `media-token` 暂时没有；`basic_auth` 的 bcrypt 哈希可以用
  `htpasswd -B` 生成。

## 保持不变的，以及需要留意的

- **默认值的名称沿用 fit 的**：Docker 标签以 `fit.` 开头，Consul 标签以 `fit` 开头，KV 的键以 `fit/` 开头，粘性 cookie 为
  `fit_sticky`，遥测的服务名为 `fit`，Kubernetes 控制器为 `lesismal.github.io/fit`。akit 和 fit 若在同一集群或同一 Docker 主机上
  并存，会认领同样的标签和 GatewayClass：给其中一个设置自己的前缀或 `controller_name`。
- **ACME 的存储**默认是 `~/.local/share/akit`，而不是 fit 的目录。要沿用 fit 的账户和证书，把 `acme.storage` 指向 fit 的目录，或把
  `shared_storage` 指向 fit 的存储。
- **平台**：Linux（akio 使用 io_uring，5.19 及以上，或 epoll），以及用于开发的 macOS。不支持 Windows。
- **为 fit 构建的 WebAssembly 插件**无需修改即可运行，但运行在 akio-wasm 的解释器上，而不是 wazero 的编译器上：每次调用开销更大
  （[WebAssembly](wasm.zh-CN.md)）。

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
akit fmt -w -c /etc/fit
```
