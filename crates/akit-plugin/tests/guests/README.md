# fit's guest, for the tests

[English](README.md) | [简体中文](README.zh-CN.md)

`tests/wasm.rs` runs fit's own test plugin through the `wasm` middleware, unchanged, and
`tests/proxywasm.rs` fit's Proxy-Wasm test filters:

| File | Source in fit | Built with | Size |
| --- | --- | --- | --- |
| `pw-http_routing.wasm.gz` | `plugin/wasm/testdata/proxywasm/http_routing` (fit at `edbe1cfe`): the Proxy-Wasm Go SDK's example (SDK at `44c7d5847745`), unchanged; routes to the canary by `:authority` | Go 1.27.3, the SDK from fit's `testdata/proxywasm/go.mod`, `GOOS=wasip1 GOARCH=wasm -buildmode=c-shared -trimpath`, `gzip -9 -n` | 0.8 MB |
| `pw-properties.wasm.gz` | `.../proxywasm/properties`: the SDK's example; answers 401 without the field a property names | as above | 0.7 MB |
| `pw-http_headers.wasm.gz` | `.../proxywasm/http_headers`: the SDK's example; changes a request field and adds response fields in `proxy_on_response_headers` | as above | 1.3 MB |
| `pw-http_body.wasm.gz` | `.../proxywasm/http_body`: the SDK's example; holds the response body to its end and appends, prepends or replaces | as above | 0.7 MB |
| `pw-response.wasm.gz` | `.../proxywasm/response`: fit's own filter; upper-cases each body part, reads `:status`, changes the trailer, or answers from `proxy_on_response_headers` | as above | 0.7 MB |
| `headers-go.wasm.gz` | `plugin/wasm/testdata/headers` (fit at `7daee573`; `plugin/wasm` is the same at `edbe1cfe`, which built it): fit's own ABI, edits fields, answers `/deny`, and `/loop`, `/grow`, `/panic` | Go 1.27.3, `GOOS=wasip1 GOARCH=wasm -buildmode=c-shared`, `gzip -9 -n` | 1.2 MB |

`headers-go.wasm.gz` is akio-wasm's committed guest (akio's `crates/akio-wasm/tests/guests/headers-go.wasm.gz`,
SHA-256 `3c1094fc117430ed8c443030aaf1eadd5d9c5d7a644028042ab3334cb1ca3bbb`), copied so that akit's
tests need no Go toolchain and no path into akio's checkout. When akio rebuilds it
(`crates/akio-wasm/tests/guests/build.sh <fit checkout>`), copy it again and run
`cargo test -p akit-plugin`.

The `pw-*` filters are built from a fit checkout, in `plugin/wasm/testdata/proxywasm` (the SDK
from Go's module cache or the network):

```bash
for n in http_routing properties http_headers http_body response; do
  GOOS=wasip1 GOARCH=wasm CGO_ENABLED=0 GOWORK=off GOFLAGS=-mod=mod \
    go build -trimpath -buildmode=c-shared -o /tmp/$n.wasm ./$n &&
  gzip -9 -n -c /tmp/$n.wasm > pw-$n.wasm.gz
done
```
