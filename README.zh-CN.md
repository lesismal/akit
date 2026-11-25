# akit

[English](README.md) | [简体中文](README.zh-CN.md)

akit 是用 Rust 基于 [akio](https://github.com/lesismal/akio) 编写的反向代理、负载均衡、TLS 终结、静态文件服务和
API 网关，定位与 nginx、Caddy、Traefik 和 [fit](https://github.com/lesismal/fit) 相同。它是在 akio 上重建的 fit，
akio 是 fit 的引擎 [fib](https://github.com/lesismal/fib) 在 Rust 中的对应物：功能相同，配置文件相同、三种格式相同，
Admin API 相同；没有垃圾回收，回调直接运行在每核一线程的 I/O 线程上，每个监听器各自选择 io_uring 或 epoll，自身没有
一行 `unsafe`。

**状态：1.0 之前。** 里程碑 M0 到 M5 已完成：代理核心、动态配置与 TLS、网关功能、provider、缓存和 WebAssembly
插件（[开发进度](docs/progress.zh-CN.md)）。1.0 把 fit 的配置和 Admin API 冻结为 v1；fit 在边缘代理之外的各条路线在其后。

## 文档

- [入门](docs/guides/getting-started.zh-CN.md)和[指南](docs/guides/README.zh-CN.md)：反向代理、静态文件、HTTPS、
  Admin API、缓存、WebAssembly 插件，以及从 fit、nginx、Caddy 和 Traefik 迁移。
- [配置参考](docs/reference/config.zh-CN.md)：每个字段、其类型和默认值，以及 akit 与 fit 的不同之处。
- [路线图](docs/roadmap.zh-CN.md)：目标、设计原则、里程碑，以及与 fit 对照的功能地图。
- [开发计划](docs/dev-plan.zh-CN.md)：拆分成迭代的里程碑，每个迭代一次会话。
- [开发进度](docs/progress.zh-CN.md)：已完成的内容，以及从哪里继续。
- [架构](docs/architecture.zh-CN.md)：crate、线程、运行时图，以及一个请求经过的路径。
- [开发规则](docs/rules.zh-CN.md)：每次改动都必须遵守的规则。
- [akio API 需求](docs/akio-api-requests.zh-CN.md)：akit 需要而 akio 还没有的东西。
- [决定](docs/decisions/README.zh-CN.md)：依赖策略、与 fit 的配置兼容，以及之后做出的其他决定。

## 它的样子

fit 能运行的配置，例如 fit 的 [hello](https://github.com/lesismal/fit-examples/blob/main/hello/fit.conf)，
不加修改就能运行：

```fit
listener web {
	address :8080
	protocols http/1.1 h2c
}

route hello {
	match {
		path /hello
	}
	handler respond {
		body "hello from akit\n"
	}
}
```

```bash
akit validate -c hello.conf
akit run -c hello.conf
```

## 构建

需要 Rust 1.85 或更高版本（akio 的 MSRV）。`make build` 构建 `target/release/akit`；`make check` 运行 fmt、clippy、
测试、cargo-deny 和双语文档检查，`make test-changed` 只测试改动波及的 crate。akio 按 `Cargo.toml` 锁定的提交从
GitHub 获取。Linux 是一等平台；macOS 用于开发；不支持 Windows。

## 许可证

Apache License 2.0，见 [LICENSE](LICENSE)。
