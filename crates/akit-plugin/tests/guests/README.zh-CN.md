# 测试用的 fit guest

[English](README.md) | [简体中文](README.zh-CN.md)

`tests/wasm.rs` 通过 `wasm` 中间件原样运行 fit 自己的测试插件，`tests/proxywasm.rs` 运行 fit 的 Proxy-Wasm
测试过滤器：

| 文件 | fit 中的源码 | 构建方式 | 大小 |
| --- | --- | --- | --- |
| `pw-http_routing.wasm.gz` | `plugin/wasm/testdata/proxywasm/http_routing`（fit `edbe1cfe`）：Proxy-Wasm Go SDK 的示例（SDK `44c7d5847745`），未作修改；按 `:authority` 路由到 canary | Go 1.27.3，SDK 取自 fit 的 `testdata/proxywasm/go.mod`，`GOOS=wasip1 GOARCH=wasm -buildmode=c-shared -trimpath`，`gzip -9 -n` | 0.8 MB |
| `pw-properties.wasm.gz` | `.../proxywasm/properties`：SDK 的示例；缺少属性所指的字段时应答 401 | 同上 | 0.7 MB |
| `pw-http_headers.wasm.gz` | `.../proxywasm/http_headers`：SDK 的示例；修改一个请求字段，并在 `proxy_on_response_headers` 中添加响应字段 | 同上 | 1.3 MB |
| `pw-http_body.wasm.gz` | `.../proxywasm/http_body`：SDK 的示例；把响应体保留到结束，然后追加、前置或替换 | 同上 | 0.7 MB |
| `pw-response.wasm.gz` | `.../proxywasm/response`：fit 自己的过滤器；把每个响应体分段转成大写，读取 `:status`，修改 trailer，或在 `proxy_on_response_headers` 中应答 | 同上 | 0.7 MB |
| `headers-go.wasm.gz` | `plugin/wasm/testdata/headers`（fit `7daee573`；`plugin/wasm` 在构建它的 `edbe1cfe` 上相同）：fit 自己的 ABI，修改字段，应答 `/deny`，以及 `/loop`、`/grow`、`/panic` | Go 1.27.3，`GOOS=wasip1 GOARCH=wasm -buildmode=c-shared`，`gzip -9 -n` | 1.2 MB |

`headers-go.wasm.gz` 是 akio-wasm 提交的 guest（akio 的 `crates/akio-wasm/tests/guests/headers-go.wasm.gz`，
SHA-256 `3c1094fc117430ed8c443030aaf1eadd5d9c5d7a644028042ab3334cb1ca3bbb`）的副本，这样 akit 的测试既不需要 Go
工具链，也不需要指向 akio 检出目录的路径。akio 重新构建它时（`crates/akio-wasm/tests/guests/build.sh <fit 检出目录>`），
再复制一次并运行 `cargo test -p akit-plugin`。

`pw-*` 过滤器从 fit 检出目录的 `plugin/wasm/testdata/proxywasm` 中构建（SDK 取自 Go 的模块缓存或网络）：

```bash
for n in http_routing properties http_headers http_body response; do
  GOOS=wasip1 GOARCH=wasm CGO_ENABLED=0 GOWORK=off GOFLAGS=-mod=mod \
    go build -trimpath -buildmode=c-shared -o /tmp/$n.wasm ./$n &&
  gzip -9 -n -c /tmp/$n.wasm > pw-$n.wasm.gz
done
```
