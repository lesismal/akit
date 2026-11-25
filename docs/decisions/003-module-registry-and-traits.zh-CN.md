# 003：模块注册表与数据面 trait

[English](003-module-registry-and-traits.md) | [简体中文](003-module-registry-and-traits.zh-CN.md)

状态：在 I03 中采纳（2026-11-23）。

## 背景

fit 的 `module` 包在 Go 的 `init` 函数中把模块注册进一个全局 map，用反射构建模块（一个零值结构体，导出字段由参数解码
而来），并通过类型断言（`Handler`、`Provisioner`、`Cleaner`、`Drainer`、`Inline`……）得知模块能做什么。它的
handler 就是 fib 的 handler；middleware 在路由编译时包装 handler 一次。Rust 没有 `init`、没有反射，在 akit 的
MSRV（1.85）下也不能把一个 trait 对象向上转换为另一个；akit 的规则要求没有全局可变状态、不因配置输入而 panic，且
数据面在 akio 的 I/O 线程上内联运行。

## 决定

- **数据面 trait 建立在 akio 之上**（`akit_runtime::traits`）。`Handler` 是 akio-http 的 `Service`（`call`、
  `body_mode`、`streams_bodies`），正如 fit 的 handler 是 fib 的；它在 I/O 线程上运行，要么应答要么 detach，从不
  阻塞。`HandlerService` 把它交给 akio。`Middleware` 沿用 fit：`wrap(next) -> handler`，在编译时调用一次（规则
  12）；`chain` 构建路由的链，第一个 middleware 在最外层；`AkioMiddleware` 适配 akio 的逐请求 middleware。
  `Matcher`、`Router`、`Service`（fit 的含义：挑选上游，不是 akio 的 `Service`）、`Upstream` 和 `Listener` 读取请求的
  akio `Context` 或给出 akio 的 `AnyAcceptor`；`Service`、`Upstream` 和 `Listener` 与 fit 一样是暂定的，直到 I05、
  I14 和 I19。
- **注册是每个 crate 的一个 `static` 列表。** crate 导出 `static MODULES: &[Info<P>]`，每项是一个 ID
  （`http.handlers.respond`）、一行说明和一个带类型的构造函数；二进制用它链接的列表构建一个 `Registry` 并传给编译。
  没有全局变量，也不用 `inventory`/`linkme`。格式错误的 ID、重复的 ID、或不在其种类命名空间中的 ID 是
  `RegistryError`（fit 会 panic），由对二进制注册表的测试发现。
- **命名空间就是种类。** `http.handlers` 存放 handler，`http.middleware` 存放 middleware；构造函数的类型说明是哪一种
  （`Constructor::Handler`/`Middleware`），因此 fit 的 “is not a handler” 错误不会发生。新的种类（L4 handler 等）
  增加一个 `Kind` 和一个命名空间。
- **构造函数一步完成解码和 provision**：`fn(&Provision<P>) -> Result<Provisioned<T>, ProvisionError>`。`P` 是参数类型：
  akit-runtime 不了解配置模型。I04 把 `P` 设为 akit-config 的参数值，并像 fit 的 `Load` 那样，把 `ProvisionError`
  （一条消息，可能带一个字段）转换成位于参数路径上的配置错误。
- **清理是构造函数附加的闭包**（`Provisioned::on_cleanup`），记录在该配置的 `Modules` 中；释放或丢弃时按与 provision
  相反的顺序运行清理，每个一次。编译失败时丢弃其 `Modules`，清理已 provision 的部分。清理可能在 I/O 线程上运行
  （图的最后一个持有者释放它，I04），因此从不阻塞。
- **错误沿用 fit 的措辞**：`there is no module "nope" in http.handlers (it has "proxy", "respond")`，以及 provision
  错误自己的消息；路径和位置由调用方给出。

## 影响

- 每个模块 crate 导出它的 `static` 列表，`akit build`（I70）组合列表而不是 import。自 I70 起，`akit` crate 是一个库，
  可执行文件以及构建生成的每个可执行文件都用自己的列表调用它的 `akit::main(version, lists)`（[模块](../modules.zh-CN.md)）；
  配置模型把不认识的模块保存为 `Handler::Module`，参数保持书写时的样子，因此注册表在编译时用 fit 的消息报告未知的名字，
  与 fit 的 `module.Load` 一致，而构建进去的模块自己解码参数。
- fit 的可选接口变成 trait 上带默认实现的方法（`Handler` 可增加 `inline`、`unwrapped` 等）或 `Provisioned` 上的字段
  （drain 钩子，I17），而不是类型断言。
- `Env` 目前为空；I04 随需要它们的迭代加入 fit 的 `module.Env` 所含内容（上游池、引擎、对图的持有）。
- akit-runtime 依赖 `akio-http`（与 `akio` 同一个 akio 提交），以使用 `Service`、`Context` 和 `Middleware`。
