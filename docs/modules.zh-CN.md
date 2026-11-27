# 模块与 `akit build`

[English](modules.md) | [简体中文](modules.zh-CN.md)

akit 的 handler 与中间件都是模块：每个模块在其种类的命名空间里有一个 ID
（`http.handlers.respond`、`http.middleware.headers`），以及一个编译配置时由注册表调用的构造函数
（[决策 003](decisions/003-module-registry-and-traits.zh-CN.md)）。`akit build` 构建一个 akit
可执行文件，在 akit 自带模块之外加入其他 crate 的模块，或少带一些自带模块，就像 fit 的
`fit build` 对 Go 模块所做、xcaddy 对 Caddy 所做的那样。

## 编写模块

模块 crate 是一个依赖 `akit-runtime` 的 Rust 库，在其根部导出它的模块列表：

```rust
pub static MODULES: &[Info<HandlerParams>] = &[Info::middleware(
    "http.middleware.hello",
    "Adds X-Hello to every response of its routes.",
    new,
)];
```

Rust 没有 `init`：没有什么会自行注册。`akit build` 生成的 `main` 把每个 crate 的 `MODULES` 交给
akit 的注册表；可执行文件启动时，注册表拒绝重复的 ID 或不在其种类命名空间里的 ID。

[`crates/akit/tests/testdata/hello-module`](../crates/akit/tests/testdata/hello-module) 中的示例是
一个完整的模块，也是 `akit build` 的测试所构建进去的那个。它的组成：

- **`Cargo.toml`** 只依赖 `akit-runtime`。`akit-runtime` 重新导出模块从其他部分需要的东西：
  `akit_runtime::akio_http`（handler 是 akio-http 的 `Service`）和 `akit_runtime::config`（参数）。
  你自己的 crate 依赖 akit 的 git 仓库，取你要构建的那个修订：

  ```toml
  [dependencies]
  akit-runtime = { git = "https://github.com/lesismal/akit", rev = "<commit>" }
  ```

  从 akit 的目录构建时，会用该目录替换这个仓库（Cargo 的 `[patch]`），因此模块和它被构建进的
  akit 总是共用同一个 `akit-runtime`。从 akit 的某个修订构建时使用那个修订：让你的 crate 用同一个。
- **构造函数** `fn(&Provision<HandlerParams>) -> Result<Provisioned<dyn Middleware>,
  ProvisionError>`（handler 则为 `dyn Handler`）在编译配置时运行一次。它检查参数并构建服务所需的
  一切；请求只读取它构建好的东西（规则 12 和 13）。
- **参数。** akit 的配置模型认识 akit 自己的模块；对任何其他名字，它把引用保存为
  `config::Handler::Module(ModuleRef { name, middleware, params })`，参数保持书写时的样子
  （`config::value::Value`：对象保留成员顺序，数字按书写保留）。构造函数自己解码它们，并像 akit
  解码自己的参数一样严格：不认识的字段就是错误。`ProvisionError::field("greeting", "…")` 在该字段
  的路径上报告错误，与 akit 报告自己的错误相同（`routes[0].middleware[0].params.greeting: …`）。
- **handler** 对每个请求都在 akio 的 I/O 线程上运行，因此绝不阻塞（规则 1）：不做磁盘或网络
  I/O，不等待，不长时间持有锁。只能阻塞的工作用 `spawn_blocking` 交给 akio 的逻辑池。中间件的
  `wrap` 在每条路由编译时调用一次，返回运行在下一个 handler 之前的 handler。
- 对客户端或配置发来的内容**不 panic**（规则 15）：错误是值，与 akit 一样。akit 在自己的 crate
  中禁止 `unsafe`；模块也应如此。

模块不能使用 fit 有而 akit 尚未支持的名字（`hls`、`wasm` 等）：akit 的配置模型把它们作为
"not supported yet" 拒绝。akit 可执行文件中没有对应模块的名字，会在编译配置时报告，与 fit 的报告
相同：`routes[0].handler.module: there is no module "nope" in http.handlers (it has "fastcgi", …)`。

## 构建

```bash
akit build -with akit-hello=crates/akit/tests/testdata/hello-module -o akit-hello
```

这会生成一个 Cargo 包（一个 `main.rs` 和一个 `Cargo.toml`），用 `PATH` 上的 `cargo`（或 `CARGO`
指定的）以 release 模式构建，并把可执行文件写到 `-o`。这个可执行文件就是 akit，拥有所有命令，
它的注册表在 akit 自带模块之外还有 `-with` 所列 crate 的模块；`akit version` 显示它构建时的版本。

| 参数 | |
| --- | --- |
| `-o file` | 要写出的可执行文件；省略时为 `akit` |
| `-with crate` | 要构建进去的 crate，每个重复一次：`name`、`name@version`（crates.io）、源码在 `dir` 中的 `name=dir`，或在 git 仓库中的 `name=git+url#reference`（提交、标签 `v…` 或分支；没有 `#` 时为默认分支） |
| `-without crate` | 去掉 akit 自带的一个模块 crate，每个重复一次：`akit-handler`（`respond`、`static`、`fastcgi`、`forward_proxy`）、`akit-middleware`（除 `cache` 外的所有中间件）、`akit-cache`（`cache`）、`akit-proxy`（`proxy`）、`akit-webrtc`（`whip`、`whep`、`webrtc_proxy`）或 `akit-media`（`httpflv`、`hls`） |
| `-without crate` | 去掉 akit 自带的一个模块 crate，每个重复一次：`akit-handler`（`respond`、`static`、`fastcgi`、`forward_proxy`）、`akit-middleware`（除 `cache` 外的所有中间件）、`akit-cache`（`cache`）、`akit-proxy`（`proxy`）、`akit-webrtc`（`whip`、`whep`、`webrtc_proxy`）或 `akit-bittorrent`（`bt_tracker`） |
| `-akit v` | 构建所基于的 akit：`github.com/lesismal/akit` 的一个修订（提交，或标签 `v…`）、它的另一个仓库 `git+url#reference`，或存放其源码的目录（检出的根目录或其 `crates/akit`）；省略时为构建本 akit 所用的源码 |
| `-keep dir` | 在 `dir` 中生成包并保留；否则使用临时目录并在结束后删除 |

从 akit 的目录构建时使用该检出的 `Cargo.lock`，因此用 akit 测试时的版本构建。Cargo 自己的设置
照常生效：`CARGO_TARGET_DIR` 在多次构建之间保留编译好的 crate（第一次构建要编译整个 akit 和
akio；之后用同一目录的构建只编译很少），`CARGO_BUILD_TARGET` 为另一个系统构建（可执行文件位于
该目标的目录中，`-o` 会取到它）。

退出码与 akit 的一致：写出可执行文件时为 0，构建失败时为 1（cargo 的输出说明原因），参数错误时为 2。

## 尚未支持

- 其他种类的模块：provider、四层 handler、匹配器。akit 的 provider 是内置的，`-without` 只去掉
  模块 crate（fit 的 `-without` 还接受它的 provider 和 app 包）。
- WebAssembly 插件（I71、I72），无需构建即可加载模块。
