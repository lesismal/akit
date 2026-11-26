# fit's guest, for the tests

[English](README.md) | [简体中文](README.zh-CN.md)

`tests/wasm.rs` runs fit's own test plugin through the `wasm` middleware, built for akit's ABI (fit's
with its names renamed, [decision 037](../../../../docs/decisions/037-akit-names.md)), and
`tests/proxywasm.rs` fit's Proxy-Wasm test filters, unchanged:

| File | Source in fit | Built with | Size |
| --- | --- | --- | --- |
| `pw-http_routing.wasm.gz` | `plugin/wasm/testdata/proxywasm/http_routing` (fit at `edbe1cfe`): the Proxy-Wasm Go SDK's example (SDK at `44c7d5847745`), unchanged; routes to the canary by `:authority` | Go 1.27.3, the SDK from fit's `testdata/proxywasm/go.mod`, `GOOS=wasip1 GOARCH=wasm -buildmode=c-shared -trimpath`, `gzip -9 -n` | 0.8 MB |
| `pw-properties.wasm.gz` | `.../proxywasm/properties`: the SDK's example; answers 401 without the field a property names | as above | 0.7 MB |
| `pw-http_headers.wasm.gz` | `.../proxywasm/http_headers`: the SDK's example; changes a request field and adds response fields in `proxy_on_response_headers` | as above | 1.3 MB |
| `pw-http_body.wasm.gz` | `.../proxywasm/http_body`: the SDK's example; holds the response body to its end and appends, prepends or replaces | as above | 0.7 MB |
| `pw-response.wasm.gz` | `.../proxywasm/response`: fit's own filter; upper-cases each body part, reads `:status`, changes the trailer, or answers from `proxy_on_response_headers` | as above | 0.7 MB |
| `headers-akit.wasm.gz` | `plugin/wasm/testdata/headers` and `plugin/wasm/guest` (fit at `7daee573`, the gate's pin) with the `//go:wasmexport` and `//go:wasmimport` names renamed: akit's ABI (`akit_on_request`, module `akit`), edits fields, answers `/deny`, and `/loop`, `/grow`, `/panic` | `tools/plugin-guest.sh`: Go 1.27.3, `GOOS=wasip1 GOARCH=wasm -buildmode=c-shared -trimpath`, `gzip -9 -n` | 1.2 MB |
| `headers-fit.wasm.gz` | the same plugin as fit builds it, unchanged (fit at `7daee573`; `plugin/wasm` is the same at `edbe1cfe`, which built it): fit's ABI (`fit_on_request`, module `fit`), which akit refuses | Go 1.27.3, `GOOS=wasip1 GOARCH=wasm -buildmode=c-shared`, `gzip -9 -n` | 1.2 MB |

`headers-akit.wasm.gz` is made by `tools/plugin-guest.sh` from a fit checkout (beside akit's, or
`FIT_DIR`) at the gate's pin: fit's guest package and test plugin, with only the ABI's names
renamed (`fit` to `akit`, `fit_*` to `akit_*`); `tools/plugin-guest.sh --check` builds it again and
compares. Run it again when the pin moves.

`headers-fit.wasm.gz` is akio-wasm's committed guest (akio's `crates/akio-wasm/tests/guests/headers-go.wasm.gz`,
SHA-256 `3c1094fc117430ed8c443030aaf1eadd5d9c5d7a644028042ab3334cb1ca3bbb`), fit's own build: the
test that akit refuses fit's names runs it, and the benchmark's fit server (`bench/modules.sh`,
`bench/lib.sh`).

The `pw-*` filters are built from a fit checkout, in `plugin/wasm/testdata/proxywasm` (the SDK
from Go's module cache or the network):

```bash
for n in http_routing properties http_headers http_body response; do
  GOOS=wasip1 GOARCH=wasm CGO_ENABLED=0 GOWORK=off GOFLAGS=-mod=mod \
    go build -trimpath -buildmode=c-shared -o /tmp/$n.wasm ./$n &&
  gzip -9 -n -c /tmp/$n.wasm > pw-$n.wasm.gz
done
```
