# fit's guest, for the tests

[English](README.md) | [简体中文](README.zh-CN.md)

`tests/wasm.rs` runs fit's own test plugin through the `wasm` middleware, unchanged:

| File | Source in fit | Built with | Size |
| --- | --- | --- | --- |
| `headers-go.wasm.gz` | `plugin/wasm/testdata/headers` (fit at `7daee573`; `plugin/wasm` is the same at `edbe1cfe`, which built it): fit's own ABI, edits fields, answers `/deny`, and `/loop`, `/grow`, `/panic` | Go 1.27.3, `GOOS=wasip1 GOARCH=wasm -buildmode=c-shared`, `gzip -9 -n` | 1.2 MB |

It is akio-wasm's committed guest (akio's `crates/akio-wasm/tests/guests/headers-go.wasm.gz`,
SHA-256 `3c1094fc117430ed8c443030aaf1eadd5d9c5d7a644028042ab3334cb1ca3bbb`), copied so that akit's
tests need no Go toolchain and no path into akio's checkout. When akio rebuilds it
(`crates/akio-wasm/tests/guests/build.sh <fit checkout>`), copy it again and run
`cargo test -p akit-plugin`.
