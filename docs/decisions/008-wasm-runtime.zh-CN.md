# 008：WASM 插件运行在 akio-wasm 上，使用 fit 的 ABI

[English](008-wasm-runtime.md) | [简体中文](008-wasm-runtime.zh-CN.md)

状态：I71 中接受（2026-11-25）；ABI 的名字由[决定 037](037-akit-names.zh-CN.md)修订。

## 背景

I71 运行 fit 的 WebAssembly 插件：路由的 `wasm` 中间件在编译配置时加载模块，并为每个请求调用它，在沙箱中运行，
有时间和内存上限，从不在 I/O 线程上运行。fit 在 wazero 上运行插件
（[fit 的决策 017](https://github.com/lesismal/fit/blob/main/docs/decisions/017-wasm-runtime.zh-CN.md)），
这是一个编译成机器码的纯 Go 运行时。[决策 001](001-dependency-policy.zh-CN.md) 把 WASM 运行时列为 akit 依赖
（akio 和 `std`）的预期例外：放在 akit 自己的 trait 后面，并用一份决策说明 crate、它的 `unsafe` 和依赖数量。

akio 的 [ADR-125](https://github.com/lesismal/akio/blob/main/docs/adr/125-wasm-runtime-for-plugins.zh-CN.md)
权衡过 Rust 的运行时：wasmtime（Cranelift）编译成机器码，需要 `unsafe` 和可执行内存，并带来一百多个 crate；
wasmi 是成熟的解释器，但其执行器使用 `unsafe`，而且是第三方依赖。二者都不能在没有 `unsafe` 的情况下运行
（规则 14），也都是规则 21 要求交给 akio 的那类依赖。

## 决定

- **运行时是 akio 自己的 `akio-wasm`**（akio ADR-125，akit 的 API 请求 A165）：WebAssembly 2.0 核心（不含
  SIMD）的解释器，`#![forbid(unsafe_code)]`，除 `akio-crypto`（WASI 的 `random_get`）外没有依赖。它只解码和
  校验模块一次（`Module`），创建有内存上限和调用深度上限的实例（`InstanceLimits`），每次调用都在预算下运行
  （每执行一段指令检查一次截止时间、中断），为 reactor 实现 WASI preview 1，并在 `InstancePool` 上运行插件：
  拥有实例的线程、宁可拒绝任务也不等待的有界队列、每个任务的截止时间、trap 之后重建的实例。它属于 akio，
  因此不需要决策 001 的例外，也不需要在它前面放一个 akit 的 trait。
- **ABI 是 fit 的，名字是 akit 的**（`akit_on_request` 和宿主模块 `akit`，即由决定 037 改名的 fit 的 `fit_on_request`
  和 `fit`），在 `akit-plugin` 中基于 `akio-wasm` 的 `Linker` 实现：fit 的插件把名字改名后重新构建即可运行；akit 的测试运行
  如此构建的 fit 的 Go 测试插件（`tools/plugin-guest.sh`）。Proxy-Wasm（fit 的决策 018）在 I72 中实现（[决定 011](011-proxy-wasm.zh-CN.md)）。
- **请求挂起等待**（`Context::suspend`，ADR-109）：它的调用等待实例并在实例上运行，之后从池的完成回调继续；
  请求字段通过 `Context::req_mut`（A33）修改。

## 影响

- 解释器比 wazero 的编译器慢：ADR-125 的试验测得 wazero 的解释器每次 guest 调用的开销约为其编译器的八倍
  （fit 的基准上 29.6 us 对 3.6 us）。akio-wasm 的开销，以及请求经过一个空插件与不经过插件的对比，由基准测试
  任务测量（akio 的 W5；规则 30），不在本决策中。
- 不生成机器码，因此没有 `unsafe`、没有 W^X 内存、二进制中也没有编译器；插件只能触及自己的内存和交给它的宿主函数。
- fit 的 Go 插件带着 Go 的运行时：每个实例的 `_initialize` 运行数百万条指令，在创建实例时（编译配置时，
  或 trap 之后）付出，而不是每个请求都付出。
